import Foundation

/// Minimal iOS native-audio bridge scaffold.
/// Keeps Rust as policy owner and only forwards operational audio callbacks/events.
final class CallNativeAudioIO: NSObject, AudioPlayoutReceiver, @unchecked Sendable {
    private let core: any AppCore
    private let queue = DispatchQueue(label: "chat.pika.call-native-audio-io")
    private let lock = NSLock()
    private var timer: DispatchSourceTimer?
    private var isRunning = false
    private var isMuted = false

    init(core: any AppCore) {
        self.core = core
        super.init()
        core.setAudioPlayoutReceiver(receiver: self)
    }

    func apply(activeCall: CallState?) {
        let live = activeCall?.isLive ?? false
        let muted = activeCall?.isMuted ?? false

        lock.lock()
        isMuted = muted
        let shouldStart = live && !isRunning
        let shouldStop = !live && isRunning
        if shouldStart {
            isRunning = true
        }
        if shouldStop {
            isRunning = false
        }
        lock.unlock()

        if shouldStart {
            startCaptureLoop()
        } else if shouldStop {
            stopCaptureLoop()
        }
    }

    func onAudioPlayoutFrame(callId: String, pcm: [Int16]) {
        _ = callId
        _ = pcm
        // Step-3 scaffold: playout callback is wired; real render path lands with AVAudioEngine graph.
    }

    private func startCaptureLoop() {
        core.setAudioPlayoutReceiver(receiver: self)
        core.notifyAudioDeviceRouteChanged(route: "ios.default")
        core.notifyAudioInterruptionChanged(interrupted: false, reason: "call_live")

        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now(), repeating: .milliseconds(20))
        t.setEventHandler { [weak self] in
            guard let self else { return }
            self.lock.lock()
            let running = self.isRunning
            let muted = self.isMuted
            self.lock.unlock()
            if !running || muted {
                return
            }
            self.core.sendAudioCaptureFrame(pcm: Array(repeating: 0, count: 960))
        }
        timer = t
        t.resume()
    }

    private func stopCaptureLoop() {
        timer?.cancel()
        timer = nil
        core.notifyAudioInterruptionChanged(interrupted: true, reason: "call_stopped")
    }
}
