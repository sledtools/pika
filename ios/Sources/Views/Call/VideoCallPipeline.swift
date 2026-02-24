import AVFoundation
import CoreVideo
import RealityKit
import SwiftUI

/// Coordinates the full video call pipeline: camera capture → encode → Rust core,
/// and Rust core → decode → display. Manages lifecycle based on call state.
/// Also supports avatar calls where the remote peer sends avatar animation data
/// instead of H.264 video.
@MainActor
@Observable
final class VideoCallPipeline {
    private(set) var remotePixelBuffer: CVPixelBuffer?
    private var captureManager: VideoCaptureManager?
    private var decoder: VideoDecoderRenderer?
    private var core: (any AppCore)?
    private var isActive = false
    private var lastRemoteFrameTime: CFAbsoluteTime = 0
    private var stalenessTimer: Timer?

    // Avatar call state
    private(set) var avatarRenderer: AvatarCallRenderer?
    private(set) var avatarEntity: Entity?
    private(set) var isAvatarCall = false
    private(set) var avatarLoadProgress: Double = 0
    private(set) var avatarStatus: String = ""
    private var avatarModelLoadTask: Task<Void, Never>?

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

    /// Start the avatar pipeline for an avatar call. Downloads the .glb model
    /// and registers an AvatarFrameReceiver with Rust core.
    func startAvatar(peerNpub: String, avatarModelUrl: String) {
        guard !isActive, let core else { return }
        isActive = true
        isAvatarCall = true

        avatarLoadProgress = 0
        avatarStatus = "Downloading..."
        avatarModelLoadTask = Task {
            guard let url = URL(string: avatarModelUrl) else {
                self.avatarStatus = "Invalid avatar URL"
                return
            }
            do {
                let localPath = try await AvatarModelCache.shared.loadOrDownload(
                    npub: peerNpub,
                    url: url
                ) { [weak self] progress in
                    Task { @MainActor [weak self] in
                        self?.avatarLoadProgress = progress
                    }
                }
                guard !Task.isCancelled else { return }
                self.avatarStatus = "Loading scene..."
                NSLog("[AvatarPipeline] Download complete, loading scene from \(localPath.lastPathComponent)")

                // Load via RealityKit which has native glTF/glb support (iOS 18+)
                guard #available(iOS 18.0, *) else {
                    self.avatarStatus = "Avatar calls require iOS 18+"
                    NSLog("[AvatarPipeline] Entity(contentsOf:) requires iOS 18+, cannot load .glb")
                    return
                }
                let entity = try await Entity(contentsOf: localPath)

                guard !Task.isCancelled else { return }
                let bounds = entity.visualBounds(relativeTo: nil)
                NSLog("[AvatarPipeline] Entity loaded, bounds center=(\(bounds.center.x),\(bounds.center.y),\(bounds.center.z)) extents=(\(bounds.extents.x),\(bounds.extents.y),\(bounds.extents.z))")
                self.avatarStatus = "Preparing renderer..."

                let renderer = AvatarCallRenderer(entity: entity)
                self.avatarEntity = entity
                self.avatarRenderer = renderer
                core.setAvatarFrameReceiver(receiver: renderer)

                let jawInfo = renderer.jawJointIndex != nil
                    ? "jaw: \(renderer.jointNames[renderer.jawJointIndex!])"
                    : "no jaw joint"
                self.avatarStatus = "\(renderer.jointNames.count) joints, \(jawInfo), \(renderer.animationNames.count) anims"

                // Clear status after 5 seconds
                Task { @MainActor in
                    try? await Task.sleep(for: .seconds(5))
                    if self.avatarRenderer != nil {
                        self.avatarStatus = ""
                    }
                }
            } catch {
                self.avatarStatus = "Failed: \(error.localizedDescription)"
                NSLog("[AvatarPipeline] Failed to load avatar model: \(error)")
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

        // Avatar cleanup
        avatarModelLoadTask?.cancel()
        avatarModelLoadTask = nil
        avatarRenderer = nil
        avatarEntity = nil
        isAvatarCall = false
        avatarLoadProgress = 0
        avatarStatus = ""
    }

    func switchCamera() {
        captureManager?.switchCamera()
    }

    /// React to call state changes. Starts/stops the pipeline automatically.
    func syncWithCallState(_ call: CallState?, peerAvatarModelUrl: String? = nil) {
        guard let call, call.isVideoCall, call.isLive else {
            stop()
            return
        }

        // If the call transitioned to avatar mode after being started as a regular
        // video call (e.g. bot accepted with avatar0), restart as avatar pipeline.
        if isActive && !isAvatarCall && call.isAvatarCall {
            stop()
        }

        if !isActive {
            if call.isAvatarCall, let avatarUrl = peerAvatarModelUrl, !avatarUrl.isEmpty {
                startAvatar(peerNpub: call.peerNpub, avatarModelUrl: avatarUrl)
            } else if !call.isAvatarCall {
                start()
            }
        }

        // No camera capture for avatar calls
        if !call.isAvatarCall {
            syncCapture(enabled: call.isCameraEnabled)
        }
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
        }
    }
}
