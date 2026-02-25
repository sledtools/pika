import Combine
import Foundation
import os
import RealityKit
import SwiftUI

/// Viseme frame format (viseme-v1): 19 bytes
/// - version: u8 (0x01)
/// - viseme_id: u8 (0-14 or 0xFF for idle)
/// - viseme_weight: f32 LE (0.0-1.0)
/// - expression_id: u8 (reserved)
/// - expression_weight: f32 LE (reserved)
/// - timestamp_us: u64 LE
struct AvatarVisemeFrame {
    let visemeId: UInt8
    let visemeWeight: Float
    let expressionId: UInt8
    let expressionWeight: Float
    let timestampUs: UInt64

    static let size = 19

    init?(data: Data) {
        guard data.count >= Self.size else { return nil }
        let version = data[data.startIndex]
        guard version == 0x01 else { return nil }
        visemeId = data[data.startIndex + 1]
        visemeWeight = data.withUnsafeBytes { buf in
            buf.loadUnaligned(fromByteOffset: 2, as: Float.self)
        }
        expressionId = data[data.startIndex + 6]
        expressionWeight = data.withUnsafeBytes { buf in
            buf.loadUnaligned(fromByteOffset: 7, as: Float.self)
        }
        timestampUs = data.withUnsafeBytes { buf in
            buf.loadUnaligned(fromByteOffset: 11, as: UInt64.self)
        }
    }
}

/// Manages downloading and caching avatar models keyed by npub.
/// Tracks the source URL so the cache is invalidated when the profile changes.
final class AvatarModelCache: @unchecked Sendable {
    static let shared = AvatarModelCache()

    private let cacheDir: URL = {
        let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first!
            .appendingPathComponent("avatar_models", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }()

    private func modelPath(forNpub npub: String) -> URL {
        cacheDir.appendingPathComponent("\(npub).usdz")
    }

    private func urlMetaPath(forNpub npub: String) -> URL {
        cacheDir.appendingPathComponent("\(npub).url")
    }

    func loadOrDownload(
        npub: String,
        url: URL,
        onProgress: @Sendable @escaping (Double) -> Void = { _ in }
    ) async throws -> URL {
        let path = modelPath(forNpub: npub)
        let metaPath = urlMetaPath(forNpub: npub)

        // Check if cached file matches the current URL
        if FileManager.default.fileExists(atPath: path.path) {
            let cachedUrl = (try? String(contentsOf: metaPath, encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            if cachedUrl == url.absoluteString {
                onProgress(1.0)
                return path
            }
            // URL changed — delete stale cache
            try? FileManager.default.removeItem(at: path)
            try? FileManager.default.removeItem(at: metaPath)
        }

        let (bytes, response) = try await URLSession.shared.bytes(from: url)
        let totalBytes = response.expectedContentLength
        var downloaded: Int64 = 0
        var data = Data()
        if totalBytes > 0 {
            data.reserveCapacity(Int(totalBytes))
        }
        for try await byte in bytes {
            data.append(byte)
            downloaded += 1
            if totalBytes > 0, downloaded.isMultiple(of: 4096) {
                onProgress(Double(downloaded) / Double(totalBytes))
            }
        }
        onProgress(1.0)
        try data.write(to: path, options: .atomic)
        try url.absoluteString.write(to: metaPath, atomically: true, encoding: .utf8)
        return path
    }
}

/// Receives avatar animation frames from Rust and drives a RealityKit model.
/// Implements `AvatarFrameReceiver` (UniFFI callback interface).
final class AvatarCallRenderer: AvatarFrameReceiver {
    let entity: Entity
    private(set) var animationNames: [String] = []
    private(set) var jointNames: [String] = []
    private var availableAnims: [AnimationResource] = []
    private var currentAnimIndex: Int = -1
    private var animationGeneration: Int = 0

    /// The entity that has SkeletalPosesComponent (the skinned mesh).
    private var skinnedEntity: Entity?
    /// Index of the jaw joint in the skeleton, if found.
    private(set) var jawJointIndex: Int?
    /// The jaw joint's original (rest pose) rotation.
    private var jawRestRotation: simd_quatf?
    /// Max jaw open angle in radians (~15 degrees).
    private let maxJawAngle: Float = 15 * (.pi / 180)

    /// Current viseme weight, updated from Rust thread.
    private let _visemeWeight = OSAllocatedUnfairLock(initialState: Float(0))
    var visemeWeight: Float {
        _visemeWeight.withLock { $0 }
    }

    init(entity: Entity) {
        self.entity = entity

        // Find the entity with SkeletalPosesComponent and discover joints
        if #available(iOS 18.0, *) {
            func findSkinned(_ e: Entity) -> Entity? {
                if e.components.has(SkeletalPosesComponent.self) { return e }
                for child in e.children {
                    if let found = findSkinned(child) { return found }
                }
                return nil
            }

            if let skinned = findSkinned(entity) {
                skinnedEntity = skinned
                if let skelPoses = skinned.components[SkeletalPosesComponent.self],
                   let pose = skelPoses.poses.default
                {
                    // Collect all joint names
                    let names = Array(pose.jointNames)
                    jointNames = names

                    // Find jaw joint by name
                    let jawCandidates = ["jaw", "chin", "mouth", "lower_lip"]
                    for (idx, name) in names.enumerated() {
                        let lower = name.lowercased()
                        if jawCandidates.contains(where: { lower.contains($0) }) {
                            jawJointIndex = idx
                            jawRestRotation = pose.jointTransforms[idx].rotation
                            NSLog("[AvatarRenderer] Found jaw joint: \"\(name)\" at index \(idx)")
                            break
                        }
                    }

                    NSLog("[AvatarRenderer] Skeleton has \(names.count) joints")
                    if jawJointIndex == nil {
                        NSLog("[AvatarRenderer] No jaw joint found. Joint names: \(names)")
                    }
                }
            } else {
                NSLog("[AvatarRenderer] No entity with SkeletalPosesComponent found")
            }
        } else {
            NSLog("[AvatarRenderer] SkeletalPosesComponent requires iOS 18+; jaw animation disabled")
        }

        // Store available animations — playback starts when startAnimationCycling() is called
        availableAnims = entity.availableAnimations
        animationNames = availableAnims.map { $0.name ?? "(unnamed)" }
        NSLog("[AvatarRenderer] Available animations: \(animationNames)")
    }

    // MARK: - AvatarFrameReceiver (called from Rust thread)

    nonisolated func onAvatarFrame(callId: String, payload: Data) {
        guard let frame = AvatarVisemeFrame(data: payload) else { return }
        _visemeWeight.withLock { $0 = frame.visemeWeight }

        DispatchQueue.main.async { [weak self] in
            self?.applyJawRotation(frame.visemeWeight)
        }
    }

    // MARK: - Jaw bone rotation

    func applyJawRotation(_ weight: Float) {
        guard #available(iOS 18.0, *) else { return }
        guard let skinned = skinnedEntity,
              let jawIdx = jawJointIndex,
              let restRotation = jawRestRotation,
              var skelPoses = skinned.components[SkeletalPosesComponent.self]
        else { return }

        // Rotate jaw open around X axis (local space) proportional to viseme weight
        let angle = weight * maxJawAngle
        let jawOpen = simd_quatf(angle: angle, axis: SIMD3(1, 0, 0))
        let newRotation = restRotation * jawOpen

        skelPoses.poses.default?.jointTransforms[jawIdx].rotation = newRotation
        skinned.components[SkeletalPosesComponent.self] = skelPoses
    }

    // MARK: - Random animation cycling

    func playRandomAnimation() {
        guard availableAnims.count > 0 else { return }

        // Pick a different animation than the current one
        var idx: Int
        if availableAnims.count == 1 {
            idx = 0
        } else {
            repeat {
                idx = Int.random(in: 0..<availableAnims.count)
            } while idx == currentAnimIndex
        }

        currentAnimIndex = idx
        let anim = availableAnims[idx]
        let duration = anim.definition.duration

        entity.stopAllAnimations()
        entity.playAnimation(anim)
        NSLog("[AvatarRenderer] Playing animation \(idx): \(animationNames[idx]) (duration: \(duration)s)")

        // Schedule next random animation after this one finishes.
        // Fall back to 5s if duration is 0 (some models don't report it).
        let delay = duration > 0.1 ? duration : 5.0
        animationGeneration += 1
        let gen = animationGeneration
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let self, self.animationGeneration == gen else { return }
            self.playRandomAnimation()
        }
    }

    func startAnimationCycling() {
        guard !availableAnims.isEmpty else { return }
        playRandomAnimation()
    }

    func stopAnimations() {
        animationGeneration += 1
        entity.stopAllAnimations()
    }
}

/// SwiftUI view that displays a RealityKit avatar entity.
struct AvatarSceneView: UIViewRepresentable {
    let entity: Entity
    let renderer: AvatarCallRenderer

    func makeUIView(context: Context) -> ARView {
        let arView = ARView(frame: .zero)
        arView.cameraMode = .nonAR
        arView.environment.background = .color(.black)

        // Add lighting
        let directional = DirectionalLight()
        directional.light.intensity = 2000
        directional.light.color = .white
        directional.look(at: .zero, from: SIMD3(0, 2, 4), relativeTo: nil)
        let lightAnchor = AnchorEntity()
        lightAnchor.addChild(directional)
        arView.scene.addAnchor(lightAnchor)

        // Add the model
        let anchor = AnchorEntity()
        anchor.addChild(entity)
        arView.scene.addAnchor(anchor)

        // Frame camera on head/shoulders (upper ~30% of model)
        let bounds = entity.visualBounds(relativeTo: nil)
        let center = bounds.center
        let extents = bounds.extents
        let modelTop = center.y + extents.y / 2
        let modelHeight = extents.y

        NSLog("[AvatarScene] entity bounds center=(\(center.x), \(center.y), \(center.z)) extents=(\(extents.x), \(extents.y), \(extents.z))")

        // Look at the center of the model and frame the whole thing
        let lookTarget = center
        let maxDim = max(extents.x, extents.y, extents.z)
        let halfFov = Float(15 * Double.pi / 180)
        let distance = maxDim / (2 * tan(halfFov))
        let cameraPos = SIMD3<Float>(center.x, center.y, center.z + distance)

        let cameraEntity = PerspectiveCamera()
        cameraEntity.camera.fieldOfViewInDegrees = 30
        let cameraAnchor = AnchorEntity(world: cameraPos)
        cameraAnchor.addChild(cameraEntity)
        cameraEntity.look(at: lookTarget, from: cameraPos, relativeTo: nil)
        arView.scene.addAnchor(cameraAnchor)

        NSLog("[AvatarScene] camera at (\(cameraPos.x), \(cameraPos.y), \(cameraPos.z)) looking at (\(lookTarget.x), \(lookTarget.y), \(lookTarget.z)) distance=\(distance)")

        // Start cycling through animations (must be on main thread for Timer)
        renderer.startAnimationCycling()

        return arView
    }

    func updateUIView(_ uiView: ARView, context: Context) {}
}
