import AVFAudio
import Foundation

@MainActor
final class CallAudioSessionCoordinator {
    private var isActive = false
    private var isVideoCall = false
    private weak var core: (any AppCore)?
    private var nativeAudioIO: CallNativeAudioIO?

    init(core: any AppCore) {
        self.core = core
    }

    func apply(activeCall: CallState?) {
        if shouldActivate(for: activeCall) {
            let wantsVideo = activeCall?.isVideoCall ?? false
            activateIfNeeded(isVideoCall: wantsVideo)
        } else {
            stopNativeAudioIfNeeded()
            deactivateIfNeeded()
            return
        }

        // Start/stop native audio engine based on call status
        if shouldStartNativeAudio(for: activeCall) {
            startNativeAudioIfNeeded(for: activeCall!)
        } else {
            stopNativeAudioIfNeeded()
        }
    }

    private func shouldActivate(for call: CallState?) -> Bool {
        call?.isLive ?? false
    }

    private func shouldStartNativeAudio(for call: CallState?) -> Bool {
        guard let call, call.isLive else { return false }
        switch call.status {
        case .connecting, .active:
            return true
        default:
            return false
        }
    }

    private func startNativeAudioIfNeeded(for call: CallState) {
        if let existing = nativeAudioIO, !existing.isRunning {
            // Engine was stopped (e.g., by a config change). Restart it.
            existing.start()
            return
        }
        guard nativeAudioIO == nil, let core else { return }

        let io = CallNativeAudioIO(core: core, callId: call.callId)
        core.setAudioPlayoutReceiver(receiver: io)
        io.start()
        nativeAudioIO = io
    }

    private func stopNativeAudioIfNeeded() {
        guard nativeAudioIO != nil else { return }
        nativeAudioIO?.stop()
        nativeAudioIO = nil
    }

    private func activateIfNeeded(isVideoCall: Bool) {
        let modeChanged = isActive && self.isVideoCall != isVideoCall
        guard !isActive || modeChanged else { return }
        self.isVideoCall = isVideoCall
        // .videoChat routes to speaker (user holds phone in front of face).
        // .voiceChat routes to earpiece (user holds phone to ear).
        let session = AVAudioSession.sharedInstance()
        do {
            let mode: AVAudioSession.Mode = isVideoCall ? .videoChat : .voiceChat
            var options: AVAudioSession.CategoryOptions = [.allowBluetoothHFP]
            if isVideoCall {
                options.insert(.defaultToSpeaker)
            }
            try session.setCategory(
                .playAndRecord,
                mode: mode,
                options: options
            )
            try session.setActive(true)
            isActive = true
        } catch {
            NSLog("CallAudioSessionCoordinator activate failed: \(error)")
        }
    }

    private func deactivateIfNeeded() {
        guard isActive else { return }
        do {
            try AVAudioSession.sharedInstance().setActive(
                false,
                options: [.notifyOthersOnDeactivation]
            )
            isActive = false
        } catch {
            NSLog("CallAudioSessionCoordinator deactivate failed: \(error)")
        }
    }
}
