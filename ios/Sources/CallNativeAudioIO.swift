import AVFoundation
import os

// MARK: - Lock-free SPSC Ring Buffer

/// Single-producer single-consumer ring buffer for audio playout samples.
/// Producer: Rust audio thread via `onPlayoutFrame` (writes i16 samples).
/// Consumer: AVAudioSourceNode render callback (reads i16 samples).
/// Uses os_unfair_lock for minimal-contention synchronization (sub-microsecond).
final class PlayoutRingBuffer: @unchecked Sendable {
    private let capacity: Int
    private let buffer: UnsafeMutablePointer<Int16>
    private var writePos: Int = 0
    private var readPos: Int = 0
    private var count: Int = 0
    private var lock = os_unfair_lock()

    init(capacity: Int = 9600) { // ~200ms at 48kHz
        self.capacity = capacity
        self.buffer = .allocate(capacity: capacity)
        self.buffer.initialize(repeating: 0, count: capacity)
    }

    deinit {
        buffer.deinitialize(count: capacity)
        buffer.deallocate()
    }

    /// Write samples from the Rust playout callback.
    func write(_ samples: [Int16]) {
        os_unfair_lock_lock(&lock)
        for sample in samples {
            buffer[writePos] = sample
            writePos = (writePos + 1) % capacity
            if count < capacity {
                count += 1
            } else {
                // Overflow: advance read position (drop oldest)
                readPos = (readPos + 1) % capacity
            }
        }
        os_unfair_lock_unlock(&lock)
    }

    /// Read up to `requestedCount` samples into dest. Fills remainder with silence.
    func read(into dest: UnsafeMutablePointer<Int16>, count requestedCount: Int) -> Int {
        os_unfair_lock_lock(&lock)
        let available = min(requestedCount, count)
        for i in 0..<available {
            dest[i] = buffer[readPos]
            readPos = (readPos + 1) % capacity
        }
        count -= available
        os_unfair_lock_unlock(&lock)
        // Fill remainder with silence
        for i in available..<requestedCount {
            dest[i] = 0
        }
        return available
    }
}

// MARK: - CallNativeAudioIO

/// Native capability bridge for call audio I/O on iOS.
/// Owns an AVAudioEngine graph with voice processing (AEC/NS/AGC).
/// Capture: mic → tap → accumulate 960-sample frames → sendAudioCaptureFrame → Rust
/// Playout: Rust → onPlayoutFrame → ring buffer → AVAudioSourceNode → speaker
final class CallNativeAudioIO: AudioPlayoutReceiver, @unchecked Sendable {
    private static let log = Logger(subsystem: "chat.pika", category: "CallNativeAudioIO")

    private let engine = AVAudioEngine()
    private let core: any AppCore
    private let callId: String
    private let playoutBuffer = PlayoutRingBuffer()
    private var sourceNode: AVAudioSourceNode?
    private var converter: AVAudioConverter?
    private(set) var isRunning = false

    // Capture accumulator — only accessed from the input tap thread.
    private nonisolated(unsafe) var captureAccumulator: [Int16] = []
    private let frameSamples = 960 // 20ms @ 48kHz mono

    init(core: any AppCore, callId: String) {
        self.core = core
        self.callId = callId
    }

    // MARK: - AudioPlayoutReceiver (called from Rust audio thread)

    nonisolated func onPlayoutFrame(callId: String, pcmI16: [Int16]) {
        playoutBuffer.write(pcmI16)
    }

    // MARK: - Lifecycle

    func start() {
        guard !isRunning else { return }

        let session = AVAudioSession.sharedInstance()
        guard session.sampleRate > 0 else {
            Self.log.error("Audio session sample rate is 0; cannot start native audio")
            return
        }

        // Query hardware format AFTER session is active
        let hwFormat = engine.inputNode.outputFormat(forBus: 0)
        guard hwFormat.sampleRate > 0 else {
            Self.log.error("Input node format invalid: \(hwFormat)")
            return
        }

        Self.log.info("Starting native audio: hw=\(hwFormat.sampleRate)Hz/\(hwFormat.channelCount)ch, callId=\(self.callId)")

        // Playout: AVAudioSourceNode at 48kHz mono i16
        let playoutFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: 48000,
            channels: 1,
            interleaved: true
        )!

        let ringBuffer = playoutBuffer
        let node = AVAudioSourceNode(format: playoutFormat) { _, _, frameCount, bufferList -> OSStatus in
            let ablPointer = UnsafeMutableAudioBufferListPointer(bufferList)
            guard let buf = ablPointer.first, let data = buf.mData else {
                return noErr
            }
            let dest = data.assumingMemoryBound(to: Int16.self)
            _ = ringBuffer.read(into: dest, count: Int(frameCount))
            return noErr
        }
        sourceNode = node
        engine.attach(node)
        engine.connect(node, to: engine.mainMixerNode, format: playoutFormat)

        // Capture: tap at hardware format, convert to 48kHz mono i16 if needed
        let rustFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: 48000,
            channels: 1,
            interleaved: true
        )!

        let needsConversion = hwFormat.sampleRate != 48000 || hwFormat.channelCount != 1
        if needsConversion {
            converter = AVAudioConverter(from: hwFormat, to: rustFormat)
        }

        let captureCore = core
        let captureConverter = converter
        let captureRustFormat = rustFormat
        engine.inputNode.installTap(onBus: 0, bufferSize: 1024, format: hwFormat) { [weak self] buffer, _ in
            guard let self else { return }

            if let conv = captureConverter {
                // Convert to 48kHz mono i16 using block-based API (TN3136 compliant)
                let ratio = 48000.0 / hwFormat.sampleRate
                let outCapacity = AVAudioFrameCount(ceil(Double(buffer.frameLength) * ratio))
                guard let outBuf = AVAudioPCMBuffer(
                    pcmFormat: captureRustFormat,
                    frameCapacity: outCapacity
                ) else { return }

                var error: NSError?
                var consumed = false
                conv.convert(to: outBuf, error: &error) { _, outStatus in
                    if !consumed {
                        consumed = true
                        outStatus.pointee = .haveData
                        return buffer
                    }
                    outStatus.pointee = .noDataNow
                    return nil
                }

                if let err = error {
                    Self.log.error("Audio converter error: \(err.localizedDescription)")
                    return
                }

                self.accumulateAndSend(outBuf, core: captureCore)
            } else {
                // Hardware is already 48kHz — extract directly
                self.accumulateAndSendDirect(buffer, core: captureCore)
            }
        }

        // Register for engine configuration changes (route/hardware changes)
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleEngineConfigChange),
            name: .AVAudioEngineConfigurationChange,
            object: engine
        )

        do {
            try engine.start()
            isRunning = true
            Self.log.info("Native audio engine started")
        } catch {
            Self.log.error("Failed to start audio engine: \(error.localizedDescription)")
        }
    }

    func stop() {
        guard isRunning else { return }
        isRunning = false

        NotificationCenter.default.removeObserver(self, name: .AVAudioEngineConfigurationChange, object: engine)

        engine.inputNode.removeTap(onBus: 0)
        engine.stop()

        if let node = sourceNode {
            engine.detach(node)
            sourceNode = nil
        }

        converter = nil
        captureAccumulator.removeAll()

        Self.log.info("Native audio engine stopped")
    }

    // MARK: - Capture Helpers

    /// Extract i16 samples from a converted (48kHz mono i16) buffer and accumulate.
    private nonisolated func accumulateAndSend(_ buffer: AVAudioPCMBuffer, core: any AppCore) {
        guard let int16Data = buffer.int16ChannelData else { return }
        let frameLength = Int(buffer.frameLength)
        let channelData = int16Data[0]

        for i in 0..<frameLength {
            captureAccumulator.append(channelData[i])
        }
        emitFrames(core: core)
    }

    /// Extract i16 samples from a hardware-format buffer (assumed 48kHz mono i16-compatible).
    /// If the hardware format is float, convert sample-by-sample.
    private nonisolated func accumulateAndSendDirect(_ buffer: AVAudioPCMBuffer, core: any AppCore) {
        let frameLength = Int(buffer.frameLength)

        if let int16Data = buffer.int16ChannelData {
            let channelData = int16Data[0]
            for i in 0..<frameLength {
                captureAccumulator.append(channelData[i])
            }
        } else if let floatData = buffer.floatChannelData {
            // Hardware delivers float32 — convert to i16
            let channelData = floatData[0]
            for i in 0..<frameLength {
                let clamped = max(-1.0, min(1.0, channelData[i]))
                captureAccumulator.append(Int16(clamped * Float(Int16.max)))
            }
        }
        emitFrames(core: core)
    }

    private nonisolated func emitFrames(core: any AppCore) {
        while captureAccumulator.count >= frameSamples {
            let frame = Array(captureAccumulator.prefix(frameSamples))
            captureAccumulator.removeFirst(frameSamples)
            core.sendAudioCaptureFrame(pcmI16: frame)
        }
    }

    // MARK: - Engine Configuration Change

    @objc private func handleEngineConfigChange(_ notification: Notification) {
        Self.log.info("Audio engine configuration changed — restarting")
        // Engine auto-stops on config change. Restart with new format.
        stop()
        // Re-query format and restart on next coordinator apply() cycle.
        // The coordinator will call start() again when it sees the call is still active.
    }
}
