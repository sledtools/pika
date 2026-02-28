import AVFoundation
import CoreVideo
import os
import SwiftUI

/// Coordinates the full video call pipeline: camera capture → encode → Rust core,
/// and Rust core → decode → display. Manages lifecycle based on call state.
@MainActor
@Observable
final class VideoCallPipeline {
    private static let log = Logger(subsystem: "chat.pika", category: "VideoCallPipeline")
    private static let keyframeRequestMinIntervalSeconds: CFAbsoluteTime = 1.5
    private(set) var remotePixelBuffer: CVPixelBuffer?
    private(set) var decoderCounters = VideoDecoderCounters()
    private(set) var stalenessKeyframeRequests: UInt64 = 0
    private var captureManager: VideoCaptureManager?
    private var decoder: VideoDecoderRenderer?
    private var core: (any AppCore)?
    private var isActive = false
    private var lastRemoteFrameTime: CFAbsoluteTime = 0
    private var lastKeyframeRequestAt: CFAbsoluteTime = 0
    private var pendingStalenessSignal = false
    private var stalenessTimer: Timer?

    var localCaptureSession: AVCaptureSession? {
        captureManager?.captureSession
    }

    init() {}

    /// Call once at app startup to provide the core handle.
    func configure(core: any AppCore) {
        self.core = core
    }

    /// Start the video pipeline for an active video call.
    /// Note: this starts the decoder/receiver only. Camera capture is managed
    /// by `syncCapture(enabled:)` which is driven by Rust-owned `is_camera_enabled` state.
    func start() {
        guard !isActive, let core else { return }
        isActive = true

        // Decoder: receives decrypted NALUs from Rust → decoded CVPixelBuffer
        let dec = VideoDecoderRenderer()
        dec.onDecodedFrame = { [weak self] pixelBuffer in
            guard let self else { return }
            self.lastRemoteFrameTime = CFAbsoluteTimeGetCurrent()
            self.remotePixelBuffer = pixelBuffer
        }
        dec.onDecoderReset = { [weak self] reason, counters in
            guard let self else { return }
            self.decoderCounters = counters
            switch reason {
            case .formatChange:
                break
            case .decodeFailure(let status):
                Self.log.error("video decoder reset after decode failure status=\(status)")
                self.requestKeyframe(reason: "decode_failure_\(status)")
            }
        }
        decoder = dec

        // Register decoder as the video frame receiver with Rust core
        core.setVideoFrameReceiver(receiver: dec)

        // Start staleness timer: clear remote frame if no new frames for 1s
        stalenessTimer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.checkRemoteFrameStaleness()
            }
        }
    }

    /// Stop the video pipeline when the call ends or transitions away from video.
    func stop() {
        guard isActive else { return }
        isActive = false

        stalenessTimer?.invalidate()
        stalenessTimer = nil
        captureManager?.stopCapture()
        captureManager = nil
        decoder = nil
        remotePixelBuffer = nil
        decoderCounters = VideoDecoderCounters()
        stalenessKeyframeRequests = 0
        lastKeyframeRequestAt = 0
        pendingStalenessSignal = false
    }

    func switchCamera() {
        captureManager?.switchCamera()
    }

    /// React to call state changes. Starts/stops the pipeline automatically.
    func syncWithCallState(_ call: CallState?) {
        guard let call, call.isVideoCall, call.isLive else {
            stop()
            return
        }
        // Start the decoder/receiver pipeline if not already running
        if !isActive {
            start()
        }
        // Pause/resume camera capture based on Rust-owned camera enabled state
        syncCapture(enabled: call.isCameraEnabled)
        captureManager?.updateAdaptationMetrics(
            debug: call.debug,
            staleVideoSignal: consumePendingStalenessSignal()
        )
    }

    private func syncCapture(enabled: Bool) {
        if enabled {
            if captureManager == nil, let core {
                let cap = VideoCaptureManager(core: core)
                cap.startCapture()
                captureManager = cap
            }
        } else {
            captureManager?.stopCapture()
            captureManager = nil
        }
    }

    private func checkRemoteFrameStaleness() {
        guard remotePixelBuffer != nil else { return }
        let elapsed = CFAbsoluteTimeGetCurrent() - lastRemoteFrameTime
        if elapsed > 1.0 {
            remotePixelBuffer = nil
            pendingStalenessSignal = true
            requestKeyframe(reason: "remote_stale_\(Int(elapsed * 1000))ms")
        }
    }

    var videoDebugSummary: String? {
        if decoderCounters.formatResets == 0
            && decoderCounters.decodeFailureResets == 0
            && decoderCounters.decodeFailures == 0
            && stalenessKeyframeRequests == 0
        {
            return nil
        }
        return "vdec fmt \(decoderCounters.formatResets) dec \(decoderCounters.decodeFailureResets) fail \(decoderCounters.decodeFailures) waitdrop \(decoderCounters.droppedUntilKeyframe) kreq \(stalenessKeyframeRequests)"
    }

    private func requestKeyframe(reason: String) {
        let now = CFAbsoluteTimeGetCurrent()
        if now - lastKeyframeRequestAt < Self.keyframeRequestMinIntervalSeconds {
            return
        }
        lastKeyframeRequestAt = now
        core?.requestVideoKeyframe(reason: reason)
        if stalenessKeyframeRequests < UInt64.max {
            stalenessKeyframeRequests += 1
        }
    }

    private func consumePendingStalenessSignal() -> Bool {
        let signal = pendingStalenessSignal
        pendingStalenessSignal = false
        return signal
    }
}
