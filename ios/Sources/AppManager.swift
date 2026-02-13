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

    init(core: AppCore, nsecStore: NsecStore) {
        self.core = core
        self.nsecStore = nsecStore

        let initial = core.state()
        self.state = initial
        self.lastRevApplied = initial.rev

        core.listenForUpdates(reconciler: self)

        if let nsec = nsecStore.getNsec(), !nsec.isEmpty {
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
        case .accountCreated(_, let nsec, _, _):
            // Required by spec-v2: native stores nsec; Rust never persists it.
            if !nsec.isEmpty {
                nsecStore.setNsec(nsec)
            }
            state.rev = updateRev
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
}

private extension AppUpdate {
    var rev: UInt64 {
        switch self {
        case .fullState(let s): return s.rev
        case .accountCreated(let rev, _, _, _): return rev
        }
    }
}
