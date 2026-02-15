import Foundation
import Observation

protocol AppCore: AnyObject, Sendable {
    func dispatch(action: AppAction)
    func listenForUpdates(reconciler: AppReconciler)
    func state() -> AppState
}

extension FfiApp: AppCore {}

protocol NsecStore: AnyObject {
    func getNsec() -> String?
    func setNsec(_ nsec: String)
    func clearNsec()
}

extension KeychainNsecStore: NsecStore {}

@MainActor
@Observable
final class AppManager: AppReconciler {
    private let core: AppCore
    var state: AppState
    private var lastRevApplied: UInt64
    private let nsecStore: NsecStore
    /// True while we're waiting for a stored session to be restored by Rust.
    var isRestoringSession: Bool = false
    private let callAudioSession = CallAudioSessionCoordinator()

    init(core: AppCore, nsecStore: NsecStore) {
        self.core = core
        self.nsecStore = nsecStore

        let initial = core.state()
        self.state = initial
        self.lastRevApplied = initial.rev
        callAudioSession.apply(activeCall: initial.activeCall)

        core.listenForUpdates(reconciler: self)

        if let nsec = nsecStore.getNsec(), !nsec.isEmpty {
            isRestoringSession = true
            core.dispatch(action: .restoreSession(nsec: nsec))
        }
    }

    convenience init() {
        let fm = FileManager.default
        let dataDirUrl = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let dataDir = dataDirUrl.path
        let nsecStore = KeychainNsecStore()

        // UI tests need a clean slate and a way to inject relay overrides without relying on
        // external scripts.
        let env = ProcessInfo.processInfo.environment
        let uiTestReset = env["PIKA_UI_TEST_RESET"] == "1"
        if uiTestReset {
            nsecStore.clearNsec()
            try? fm.removeItem(at: dataDirUrl)
        }
        try? fm.createDirectory(at: dataDirUrl, withIntermediateDirectories: true)

        // Optional relay override (matches `tools/run-ios` environment variables).
        let relays = (env["PIKA_RELAY_URLS"] ?? env["PIKA_RELAY_URL"])?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let callMoqUrl = (env["PIKA_CALL_MOQ_URL"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let callBroadcastPrefix = (env["PIKA_CALL_BROADCAST_PREFIX"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let moqProbeOnStart = (env["PIKA_MOQ_PROBE_ON_START"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let wantsOverride = uiTestReset
            || !relays.isEmpty
            || !callMoqUrl.isEmpty
            || !callBroadcastPrefix.isEmpty
            || moqProbeOnStart == "1"
        if wantsOverride {
            let resolvedCallMoqUrl = callMoqUrl.isEmpty ? "https://us-east.moq.logos.surf/anon" : callMoqUrl
            let resolvedCallBroadcastPrefix = callBroadcastPrefix.isEmpty ? "pika/calls" : callBroadcastPrefix
            do {
                let relayItems = relays
                    .split(separator: ",")
                    .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
                    .filter { !$0.isEmpty }

                var obj: [String: Any] = [
                    "disable_network": false,
                    "call_moq_url": resolvedCallMoqUrl,
                    "call_broadcast_prefix": resolvedCallBroadcastPrefix,
                ]
                if moqProbeOnStart == "1" {
                    obj["moq_probe_on_start"] = true
                }
                if !relayItems.isEmpty {
                    obj["relay_urls"] = relayItems
                }

                if let data = try? JSONSerialization.data(withJSONObject: obj, options: []) {
                    let path = dataDirUrl.appendingPathComponent("pika_config.json")
                    try? data.write(to: path, options: .atomic)
                }
            }
        }

        let core = FfiApp(dataDir: dataDir)
        self.init(core: core, nsecStore: nsecStore)
    }

    nonisolated func reconcile(update: AppUpdate) {
        Task { @MainActor [weak self] in
            self?.apply(update: update)
        }
    }

    func apply(update: AppUpdate) {
        let updateRev = update.rev

        // Side-effect updates must not be lost: `AccountCreated` carries an `nsec` that isn't in
        // AppState snapshots (by design). Store it even if the update is stale w.r.t. rev.
        if case .accountCreated(_, let nsec, _, _) = update {
            let existing = nsecStore.getNsec() ?? ""
            if existing.isEmpty && !nsec.isEmpty {
                nsecStore.setNsec(nsec)
            }
        }

        // The stream is full-state snapshots; drop anything stale.
        if updateRev <= lastRevApplied { return }

        lastRevApplied = updateRev
        switch update {
        case .fullState(let s):
            state = s
            if isRestoringSession {
                // Clear once we've transitioned away from login (success) or if
                // the router settles on login (restore failed / nsec invalid).
                if s.auth != .loggedOut || s.router.defaultScreen != .login {
                    isRestoringSession = false
                }
            }
            callAudioSession.apply(activeCall: s.activeCall)
        case .accountCreated(_, let nsec, _, _):
            // Required by spec-v2: native stores nsec; Rust never persists it.
            if !nsec.isEmpty {
                nsecStore.setNsec(nsec)
            }
            state.rev = updateRev
            callAudioSession.apply(activeCall: state.activeCall)
        }
    }

    func dispatch(_ action: AppAction) {
        core.dispatch(action: action)
    }

    func login(nsec: String) {
        if !nsec.isEmpty {
            nsecStore.setNsec(nsec)
        }
        dispatch(.login(nsec: nsec))
    }

    func logout() {
        nsecStore.clearNsec()
        dispatch(.logout)
    }

    func onForeground() {
        // Foreground is a lifecycle action; Rust owns state changes and side effects.
        dispatch(.foregrounded)
    }

    func refreshMyProfile() {
        dispatch(.refreshMyProfile)
    }

    func saveMyProfile(name: String, about: String) {
        dispatch(.saveMyProfile(name: name, about: about))
    }

    func uploadMyProfileImage(data: Data, mimeType: String) {
        guard !data.isEmpty else { return }
        dispatch(
            .uploadMyProfileImage(
                imageBase64: data.base64EncodedString(),
                mimeType: mimeType
            )
        )
    }

    func getNsec() -> String? {
        nsecStore.getNsec()
    }
}

private extension AppUpdate {
    var rev: UInt64 {
        switch self {
        case .fullState(let s): return s.rev
        case .accountCreated(let rev, _, _, _): return rev
        }
    }
}
