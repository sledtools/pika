import Foundation
import Observation

@MainActor
@Observable
final class AppManager: AppReconciler {
    let rust: FfiApp
    var state: AppState
    private var lastRevApplied: UInt64
    private var resyncInFlight: Bool = false
    private var maxRevSeenDuringResync: UInt64 = 0

    private let nsecStore = KeychainNsecStore()

    init() {
        let fm = FileManager.default
        let dataDirUrl = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let dataDir = dataDirUrl.path

        // UI tests need a clean slate and a way to inject relay overrides without relying on
        // external scripts.
        let env = ProcessInfo.processInfo.environment
        if env["PIKA_UI_TEST_RESET"] == "1" {
            nsecStore.clearNsec()
            try? fm.removeItem(at: dataDirUrl)
        }
        try? fm.createDirectory(at: dataDirUrl, withIntermediateDirectories: true)

        // Optional relay override (matches `tools/run-ios` environment variables).
        let relays = (env["PIKA_RELAY_URLS"] ?? env["PIKA_RELAY_URL"])?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let kpRelays = (env["PIKA_KEY_PACKAGE_RELAY_URLS"] ?? env["PIKA_KP_RELAY_URLS"])?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !relays.isEmpty || !kpRelays.isEmpty {
            let relayItems = relays
                .split(separator: ",")
                .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
                .filter { !$0.isEmpty }
            var kpItems = kpRelays
                .split(separator: ",")
                .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
                .filter { !$0.isEmpty }

            // Default key-package relays to the general relay list if not specified.
            if kpItems.isEmpty {
                kpItems = relayItems
            }

            let obj: [String: Any] = [
                // Ensure tests/dev overrides can re-enable networking even if a prior run wrote
                // `disable_network=true` into `pika_config.json`.
                "disable_network": false,
                "relay_urls": relayItems,
                "key_package_relay_urls": kpItems,
            ]

            if let data = try? JSONSerialization.data(withJSONObject: obj, options: []) {
                let path = dataDirUrl.appendingPathComponent("pika_config.json")
                try? data.write(to: path, options: .atomic)
            }
        }

        let rust = FfiApp(dataDir: dataDir)
        self.rust = rust

        let initial = rust.state()
        self.state = initial
        self.lastRevApplied = initial.rev

        rust.listenForUpdates(reconciler: self)

        if let nsec = nsecStore.getNsec(), !nsec.isEmpty {
            rust.dispatch(action: .restoreSession(nsec: nsec))
        }
    }

    nonisolated func reconcile(update: AppUpdate) {
        Task { @MainActor [weak self] in
            self?.apply(update: update)
        }
    }

    private func apply(update: AppUpdate) {
        let updateRev = update.rev

        // Side-effect updates must not be lost: `AccountCreated` carries an `nsec` that isn't in
        // AppState snapshots (by design). Store it even if the update is stale w.r.t. rev.
        if case .accountCreated(_, let nsec, _, _) = update {
            let existing = nsecStore.getNsec() ?? ""
            if existing.isEmpty && !nsec.isEmpty {
                nsecStore.setNsec(nsec)
            }
        }

        // After a resync, older updates can still be in-flight on the MainActor queue.
        // Drop them. Only treat *forward* gaps as a reason to resync.
        if updateRev <= lastRevApplied {
            return
        }
        // While resyncing, drop updates but remember the newest rev we've observed so we can
        // resync again if the snapshot is behind (prevents falling permanently behind).
        if resyncInFlight {
            maxRevSeenDuringResync = max(maxRevSeenDuringResync, updateRev)
            return
        }
        if updateRev > lastRevApplied + 1 {
            maxRevSeenDuringResync = max(maxRevSeenDuringResync, updateRev)
            requestResync()
            return
        }

        lastRevApplied = updateRev
        switch update {
        case .fullState(let s):
            state = s
        case .accountCreated(_, let nsec, _, _):
            // Required by spec-v2: native stores nsec; Rust never persists it.
            if !nsec.isEmpty {
                nsecStore.setNsec(nsec)
            }
            state.rev = updateRev
        case .routerChanged(_, let router):
            state.router = router
            state.rev = updateRev
        case .authChanged(_, let auth):
            state.auth = auth
            state.rev = updateRev
        case .chatListChanged(_, let list):
            state.chatList = list
            state.rev = updateRev
        case .currentChatChanged(_, let chat):
            state.currentChat = chat
            state.rev = updateRev
        case .toastChanged(_, let toast):
            state.toast = toast
            state.rev = updateRev
        }
    }

    private func requestResync() {
        if resyncInFlight { return }
        resyncInFlight = true
        Task.detached(priority: .userInitiated) { [rust] in
            let snapshot = rust.state()
            await MainActor.run {
                self.state = snapshot
                self.lastRevApplied = max(self.lastRevApplied, snapshot.rev)
                let maxSeen = self.maxRevSeenDuringResync
                self.maxRevSeenDuringResync = 0
                self.resyncInFlight = false

                // If newer updates arrived while the snapshot was in-flight and the snapshot is
                // behind, resync again (coalesced) rather than dropping ourselves out of date.
                if maxSeen > snapshot.rev {
                    self.maxRevSeenDuringResync = maxSeen
                    self.requestResync()
                }
            }
        }
    }

    func dispatch(_ action: AppAction) {
        rust.dispatch(action: action)
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
}

private extension AppUpdate {
    var rev: UInt64 {
        switch self {
        case .fullState(let s): return s.rev
        case .accountCreated(let rev, _, _, _): return rev
        case .routerChanged(let rev, _): return rev
        case .authChanged(let rev, _): return rev
        case .chatListChanged(let rev, _): return rev
        case .currentChatChanged(let rev, _): return rev
        case .toastChanged(let rev, _): return rev
        }
    }
}
