import AVFAudio
import Foundation

@MainActor
final class CallAudioSessionCoordinator {
    private var isActive = false
    private var isVideoCall = false

    func apply(activeCall: CallState?) {
        if shouldActivate(for: activeCall) {
            let wantsVideo = activeCall?.isVideoCall ?? false
            activateIfNeeded(isVideoCall: wantsVideo)
        } else {
            deactivateIfNeeded()
        }
    }

    private func shouldActivate(for call: CallState?) -> Bool {
        call?.isLive ?? false
    }

    private func activateIfNeeded(isVideoCall: Bool) {
        let modeChanged = isActive && self.isVideoCall != isVideoCall
        guard !isActive || modeChanged else { return }
        self.isVideoCall = isVideoCall
        // Native shim justification: cpal does not reliably configure iOS routing/session mode
        // for duplex voice; we need `.playAndRecord` + activation before the streams start.
        //
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
