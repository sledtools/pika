import AVFAudio
import Foundation

@MainActor
final class CallAudioSessionCoordinator {
    private var isActive = false

    func apply(activeCall: CallState?) {
        if shouldActivate(for: activeCall?.status) {
            activateIfNeeded()
        } else {
            deactivateIfNeeded()
        }
    }

    private func shouldActivate(for status: CallStatus?) -> Bool {
        guard let status else { return false }
        switch status {
        case .offering, .ringing, .connecting, .active:
            return true
        case .ended:
            return false
        }
    }

    private func activateIfNeeded() {
        guard !isActive else { return }
        // Native shim justification: cpal does not reliably configure iOS routing/session mode
        // for duplex voice; we need `.playAndRecord` + activation before the streams start.
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(
                .playAndRecord,
                mode: .voiceChat,
                options: [.defaultToSpeaker, .allowBluetooth]
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
