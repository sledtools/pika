import AVFoundation
import os
import VideoToolbox

/// Thread-safe container for state shared between @MainActor and the capture queue.
private final class SharedCaptureState: @unchecked Sendable {
    private let lock = NSLock()
    private var _compressionSession: VTCompressionSession?
    private var _targetFps: Int32 = 30
    private let _core: (any AppCore)?

    init(core: (any AppCore)?) {
        _core = core
    }

    var compressionSession: VTCompressionSession? {
        get { lock.withLock { _compressionSession } }
        set { lock.withLock { _compressionSession = newValue } }
    }

    var targetFps: Int32 {
        get { lock.withLock { _targetFps } }
        set { lock.withLock { _targetFps = max(1, newValue) } }
    }

    var core: (any AppCore)? { _core }

    /// Atomically read and clear the compression session.
    func takeCompressionSession() -> VTCompressionSession? {
        lock.withLock {
            let s = _compressionSession
            _compressionSession = nil
            return s
        }
    }
}

enum VideoQualityTier: Int, CaseIterable {
    case low = 0
    case medium = 1
    case high = 2

    var lowerTier: VideoQualityTier {
        VideoQualityTier(rawValue: max(VideoQualityTier.low.rawValue, rawValue - 1)) ?? .low
    }

    var higherTier: VideoQualityTier {
        VideoQualityTier(rawValue: min(VideoQualityTier.high.rawValue, rawValue + 1)) ?? .high
    }
}

struct VideoQualitySettings {
    let bitrateBps: Int
    let fps: Int32
    let keyframeIntervalFrames: Int
    let profile: CFString
}

struct AdaptationSample {
    let rxDropped: UInt64
    let videoRxDecryptFail: UInt64
}

struct VideoAdaptationController {
    let degradeStreakThreshold: Int
    let upgradeStreakThreshold: Int
    let holdSeconds: CFAbsoluteTime

    private(set) var tier: VideoQualityTier = .high
    private var lastSample: AdaptationSample?
    private var poorSignalStreak = 0
    private var goodSignalStreak = 0
    private var lastTierChangeAt: CFAbsoluteTime = 0

    init(
        degradeStreakThreshold: Int = 2,
        upgradeStreakThreshold: Int = 6,
        holdSeconds: CFAbsoluteTime = 4.0,
        initialTier: VideoQualityTier = .high
    ) {
        self.degradeStreakThreshold = degradeStreakThreshold
        self.upgradeStreakThreshold = upgradeStreakThreshold
        self.holdSeconds = holdSeconds
        tier = initialTier
        lastTierChangeAt = -holdSeconds
    }

    mutating func observe(sample: AdaptationSample, staleVideoSignal: Bool, now: CFAbsoluteTime) -> VideoQualityTier? {
        defer { lastSample = sample }
        guard let previous = lastSample else { return nil }

        let dropDelta = sample.rxDropped >= previous.rxDropped
            ? sample.rxDropped - previous.rxDropped
            : 0
        let decryptDelta = sample.videoRxDecryptFail >= previous.videoRxDecryptFail
            ? sample.videoRxDecryptFail - previous.videoRxDecryptFail
            : 0

        let poorSignal = staleVideoSignal || decryptDelta > 0 || dropDelta >= 12
        let goodSignal = !poorSignal && decryptDelta == 0 && dropDelta <= 2

        if poorSignal {
            poorSignalStreak += 1
            goodSignalStreak = 0
        } else if goodSignal {
            goodSignalStreak += 1
            poorSignalStreak = 0
        } else {
            poorSignalStreak = 0
            goodSignalStreak = 0
        }

        if poorSignalStreak >= degradeStreakThreshold
            && now - lastTierChangeAt >= holdSeconds
            && tier != .low
        {
            poorSignalStreak = 0
            tier = tier.lowerTier
            lastTierChangeAt = now
            return tier
        }

        if goodSignalStreak >= upgradeStreakThreshold
            && now - lastTierChangeAt >= holdSeconds
            && tier != .high
        {
            goodSignalStreak = 0
            tier = tier.higherTier
            lastTierChangeAt = now
            return tier
        }

        return nil
    }
}

/// Manages camera capture and H.264 encoding for video calls.
/// Captures from the front camera at 720p 30fps, encodes to H.264 Annex B NALUs,
/// and pushes them to Rust core via `ffiApp.sendVideoFrame()`.
@MainActor
final class VideoCaptureManager: NSObject {
    let captureSession = AVCaptureSession()
    private let videoOutput = AVCaptureVideoDataOutput()
    private let processingQueue = DispatchQueue(label: "pika.video.capture", qos: .userInteractive)
    private var currentCameraPosition: AVCaptureDevice.Position = .front
    private var isRunning = false

    private let shared: SharedCaptureState
    private var adaptationController = VideoAdaptationController()

    private nonisolated(unsafe) static let log = Logger(subsystem: "chat.pika", category: "VideoCaptureManager")

    init(core: (any AppCore)?) {
        self.shared = SharedCaptureState(core: core)
        super.init()
    }

    private static func settings(for tier: VideoQualityTier) -> VideoQualitySettings {
        switch tier {
        case .high:
            return VideoQualitySettings(
                bitrateBps: 1_500_000,
                fps: 30,
                keyframeIntervalFrames: 60,
                profile: kVTProfileLevel_H264_Main_AutoLevel
            )
        case .medium:
            return VideoQualitySettings(
                bitrateBps: 900_000,
                fps: 24,
                keyframeIntervalFrames: 48,
                profile: kVTProfileLevel_H264_Main_AutoLevel
            )
        case .low:
            return VideoQualitySettings(
                bitrateBps: 500_000,
                fps: 15,
                keyframeIntervalFrames: 30,
                profile: kVTProfileLevel_H264_Baseline_AutoLevel
            )
        }
    }

    func startCapture() {
        guard !isRunning else { return }
        isRunning = true

        setupCaptureSession()
        setupEncoder()

        processingQueue.async { [weak self] in
            self?.captureSession.startRunning()
        }
    }

    func stopCapture() {
        guard isRunning else { return }
        isRunning = false

        processingQueue.async { [weak self] in
            self?.captureSession.stopRunning()
        }

        if let session = shared.takeCompressionSession() {
            VTCompressionSessionInvalidate(session)
        }
    }

    func switchCamera() {
        let newPosition: AVCaptureDevice.Position = currentCameraPosition == .front ? .back : .front
        currentCameraPosition = newPosition

        processingQueue.async { [weak self] in
            guard let self else { return }
            self.captureSession.beginConfiguration()
            // Remove existing input
            for input in self.captureSession.inputs {
                self.captureSession.removeInput(input)
            }
            // Add new input
            if let device = self.camera(for: newPosition),
               let input = try? AVCaptureDeviceInput(device: device) {
                if self.captureSession.canAddInput(input) {
                    self.captureSession.addInput(input)
                }
            }
            // Re-apply orientation and mirroring for the new connection
            if let connection = self.videoOutput.connection(with: .video) {
                if connection.isVideoOrientationSupported {
                    connection.videoOrientation = .portrait
                }
                if connection.isVideoMirroringSupported {
                    connection.isVideoMirrored = newPosition == .front
                }
            }
            self.captureSession.commitConfiguration()
        }
    }

    func updateAdaptationMetrics(debug: CallDebugStats?, staleVideoSignal: Bool) {
        guard isRunning, let debug else { return }
        let sample = AdaptationSample(
            rxDropped: debug.rxDropped,
            videoRxDecryptFail: debug.videoRxDecryptFail
        )
        let previousTier = adaptationController.tier
        if let nextTier = adaptationController.observe(
            sample: sample,
            staleVideoSignal: staleVideoSignal,
            now: CFAbsoluteTimeGetCurrent()
        ) {
            let reason = nextTier.rawValue < previousTier.rawValue ? "degrade" : "upgrade"
            applyQualityTier(nextTier, reason: reason)
        }
    }

    // MARK: - Private

    private func setupCaptureSession() {
        captureSession.beginConfiguration()
        // Prevent AVCaptureSession from reconfiguring the AVAudioSession — the
        // CallAudioSessionCoordinator already sets .playAndRecord / .voiceChat
        // for duplex voice and we don't want video capture to override that.
        captureSession.automaticallyConfiguresApplicationAudioSession = false
        captureSession.sessionPreset = .hd1280x720

        // Remove old inputs/outputs
        for input in captureSession.inputs { captureSession.removeInput(input) }
        for output in captureSession.outputs { captureSession.removeOutput(output) }

        // Camera input
        guard let device = camera(for: currentCameraPosition),
              let input = try? AVCaptureDeviceInput(device: device) else {
            captureSession.commitConfiguration()
            return
        }
        if captureSession.canAddInput(input) {
            captureSession.addInput(input)
        }

        // Configure frame rate
        let initialFps = Self.settings(for: adaptationController.tier).fps
        shared.targetFps = initialFps
        do {
            try device.lockForConfiguration()
            let frameDuration = CMTime(value: 1, timescale: initialFps)
            device.activeVideoMinFrameDuration = frameDuration
            device.activeVideoMaxFrameDuration = frameDuration
            device.unlockForConfiguration()
        } catch {
            Self.log.error("failed to set frame rate: \(error.localizedDescription)")
        }

        // Video output
        videoOutput.videoSettings = [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        ]
        videoOutput.setSampleBufferDelegate(self, queue: processingQueue)
        videoOutput.alwaysDiscardsLateVideoFrames = true
        if captureSession.canAddOutput(videoOutput) {
            captureSession.addOutput(videoOutput)
        }

        // Set orientation and mirroring on the capture connection
        if let connection = videoOutput.connection(with: .video) {
            // Rotate pixels to portrait so the encoded stream matches the phone orientation.
            if connection.isVideoOrientationSupported {
                connection.videoOrientation = .portrait
            }
            if connection.isVideoMirroringSupported {
                connection.isVideoMirrored = currentCameraPosition == .front
            }
        }

        captureSession.commitConfiguration()
    }

    private func setupEncoder() {
        if let oldSession = shared.takeCompressionSession() {
            VTCompressionSessionInvalidate(oldSession)
        }

        // Capture connection rotates pixels to portrait (videoOrientation = .portrait),
        // so the pixel buffers are 720x1280 — encoder dimensions must match.
        let width: Int32 = 720
        let height: Int32 = 1280

        var session: VTCompressionSession?
        let status = VTCompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            width: width,
            height: height,
            codecType: kCMVideoCodecType_H264,
            encoderSpecification: nil,
            imageBufferAttributes: nil,
            compressedDataAllocator: nil,
            outputCallback: nil,
            refcon: nil,
            compressionSessionOut: &session
        )
        guard status == noErr, let session else {
            Self.log.error("VTCompressionSessionCreate failed: \(status)")
            return
        }

        applyEncoderSettings(session: session, tier: adaptationController.tier)

        VTCompressionSessionPrepareToEncodeFrames(session)

        shared.compressionSession = session
    }

    private func applyQualityTier(_ nextTier: VideoQualityTier, reason: String) {
        let settings = Self.settings(for: nextTier)
        shared.targetFps = settings.fps
        Self.log.info(
            "video quality tier=\(nextTier.rawValue) reason=\(reason) bitrate=\(settings.bitrateBps) fps=\(settings.fps)"
        )

        processingQueue.async { [weak self] in
            guard let self else { return }
            if let session = self.shared.compressionSession {
                self.applyEncoderSettings(session: session, tier: nextTier)
            }
            self.applyCaptureFrameRateForCurrentInput(tier: nextTier)
        }
    }

    private nonisolated func applyEncoderSettings(
        session: VTCompressionSession,
        tier: VideoQualityTier
    ) {
        let settings = Self.settings(for: tier)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_RealTime, value: kCFBooleanTrue)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_ProfileLevel, value: settings.profile)
        let bitrate = settings.bitrateBps as CFNumber
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_AverageBitRate, value: bitrate)
        let expectedFrameRate = Int(settings.fps) as CFNumber
        VTSessionSetProperty(
            session,
            key: kVTCompressionPropertyKey_ExpectedFrameRate,
            value: expectedFrameRate
        )
        let keyframeInterval = settings.keyframeIntervalFrames as CFNumber
        VTSessionSetProperty(
            session,
            key: kVTCompressionPropertyKey_MaxKeyFrameInterval,
            value: keyframeInterval
        )
        VTSessionSetProperty(
            session,
            key: kVTCompressionPropertyKey_AllowFrameReordering,
            value: kCFBooleanFalse
        )
    }

    private nonisolated func applyCaptureFrameRateForCurrentInput(tier: VideoQualityTier) {
        let settings = Self.settings(for: tier)
        for input in captureSession.inputs {
            guard let deviceInput = input as? AVCaptureDeviceInput else { continue }
            let device = deviceInput.device
            do {
                try device.lockForConfiguration()
                let frameDuration = CMTime(value: 1, timescale: settings.fps)
                device.activeVideoMinFrameDuration = frameDuration
                device.activeVideoMaxFrameDuration = frameDuration
                device.unlockForConfiguration()
            } catch {
                Self.log.error("failed to apply quality frame rate: \(error.localizedDescription)")
            }
            return
        }
    }

    private func camera(for position: AVCaptureDevice.Position) -> AVCaptureDevice? {
        AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: position)
    }

    private nonisolated func encodeFrame(_ pixelBuffer: CVPixelBuffer, presentationTime: CMTime) {
        guard let session = shared.compressionSession else { return }
        let fps = shared.targetFps

        var flags: VTEncodeInfoFlags = []
        let status = VTCompressionSessionEncodeFrame(
            session,
            imageBuffer: pixelBuffer,
            presentationTimeStamp: presentationTime,
            duration: CMTime(value: 1, timescale: fps),
            frameProperties: nil,
            infoFlagsOut: &flags
        ) { [weak self] status, _, sampleBuffer in
            guard status == noErr, let sampleBuffer else { return }
            self?.handleEncodedFrame(sampleBuffer)
        }

        if status != noErr {
            Self.log.error("encode failed: \(status)")
        }
    }

    private nonisolated func handleEncodedFrame(_ sampleBuffer: CMSampleBuffer) {
        // Extract Annex B NALUs from the CMSampleBuffer
        guard let annexB = sampleBufferToAnnexB(sampleBuffer) else { return }
        shared.core?.sendVideoFrame(payload: annexB)
    }

    /// Convert a CMSampleBuffer with AVCC-formatted H.264 data to Annex B format.
    private nonisolated func sampleBufferToAnnexB(_ sampleBuffer: CMSampleBuffer) -> Data? {
        guard let dataBuffer = CMSampleBufferGetDataBuffer(sampleBuffer) else { return nil }
        guard let formatDesc = CMSampleBufferGetFormatDescription(sampleBuffer) else { return nil }

        var output = Data()

        // Check if keyframe — if so, prepend SPS/PPS
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
        var isKeyframe = false
        if let attachments, CFArrayGetCount(attachments) > 0 {
            let dict = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFDictionary.self)
            let notSync = CFDictionaryGetValue(dict, Unmanaged.passUnretained(kCMSampleAttachmentKey_NotSync).toOpaque())
            isKeyframe = notSync == nil
        }

        if isKeyframe {
            // Extract SPS
            var spsSize = 0
            var spsCount = 0
            var spsPtr: UnsafePointer<UInt8>?
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                formatDesc, parameterSetIndex: 0,
                parameterSetPointerOut: &spsPtr, parameterSetSizeOut: &spsSize,
                parameterSetCountOut: &spsCount, nalUnitHeaderLengthOut: nil
            )
            if let spsPtr, spsSize > 0 {
                let startCode: [UInt8] = [0x00, 0x00, 0x00, 0x01]
                output.append(contentsOf: startCode)
                output.append(spsPtr, count: spsSize)
            }

            // Extract PPS
            var ppsSize = 0
            var ppsPtr: UnsafePointer<UInt8>?
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                formatDesc, parameterSetIndex: 1,
                parameterSetPointerOut: &ppsPtr, parameterSetSizeOut: &ppsSize,
                parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil
            )
            if let ppsPtr, ppsSize > 0 {
                let startCode: [UInt8] = [0x00, 0x00, 0x00, 0x01]
                output.append(contentsOf: startCode)
                output.append(ppsPtr, count: ppsSize)
            }
        }

        // Extract NAL units from data buffer
        var lengthAtOffset = 0
        var totalLength = 0
        var bufferDataPtr: UnsafeMutablePointer<Int8>?
        CMBlockBufferGetDataPointer(dataBuffer, atOffset: 0, lengthAtOffsetOut: &lengthAtOffset,
                                    totalLengthOut: &totalLength, dataPointerOut: &bufferDataPtr)
        guard let bufferDataPtr else { return output.isEmpty ? nil : output }

        var offset = 0
        let nalLengthSize = 4
        while offset < totalLength - nalLengthSize {
            // Read AVCC NALU length (big-endian u32)
            var nalLength: UInt32 = 0
            memcpy(&nalLength, bufferDataPtr + offset, nalLengthSize)
            nalLength = nalLength.bigEndian
            offset += nalLengthSize

            guard nalLength > 0, offset + Int(nalLength) <= totalLength else { break }

            let startCode: [UInt8] = [0x00, 0x00, 0x00, 0x01]
            output.append(contentsOf: startCode)
            output.append(Data(bytes: bufferDataPtr + offset, count: Int(nalLength)))
            offset += Int(nalLength)
        }

        return output.isEmpty ? nil : output
    }
}

extension VideoCaptureManager: AVCaptureVideoDataOutputSampleBufferDelegate {
    nonisolated func captureOutput(
        _ output: AVCaptureOutput,
        didOutput sampleBuffer: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        let presentationTime = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        encodeFrame(pixelBuffer, presentationTime: presentationTime)
    }
}
