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

        PushNotificationManager.shared.onTokenReceived = { [weak self] token in
            self?.dispatch(.setPushToken(token: token))
        }

        if let nsec = nsecStore.getNsec(), !nsec.isEmpty {
            isRestoringSession = true
            core.dispatch(action: .restoreSession(nsec: nsec))
            PushNotificationManager.shared.requestPermissionAndRegister()
        }
    }

    convenience init() {
        let fm = FileManager.default
        let appGroup = Bundle.main.infoDictionary?["PikaAppGroup"] as? String ?? "group.org.pikachat.pika"
        let keychainGroup = Bundle.main.infoDictionary?["PikaKeychainGroup"] as? String ?? ""
        let dataDirUrl: URL
        if let groupContainer = fm.containerURL(forSecurityApplicationGroupIdentifier: appGroup) {
            dataDirUrl = groupContainer.appendingPathComponent("Library/Application Support")
        } else {
            // Fallback for simulator builds where CODE_SIGNING_ALLOWED=NO
            // means entitlements aren't embedded and the app group container
            // is unavailable. NSE won't work but the app itself runs fine.
            dataDirUrl = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        }
        let dataDir = dataDirUrl.path
        let nsecStore = KeychainNsecStore(keychainGroup: keychainGroup)

        // One-time migration: move existing data from the old app-private container
        // to the shared App Group container so the NSE can access the MLS database.
        Self.migrateDataDirIfNeeded(fm: fm, newDir: dataDirUrl)

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
        let kpRelays = (env["PIKA_KEY_PACKAGE_RELAY_URLS"] ?? env["PIKA_KP_RELAY_URLS"])?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let callMoqUrl = (env["PIKA_CALL_MOQ_URL"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let callBroadcastPrefix = (env["PIKA_CALL_BROADCAST_PREFIX"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let moqProbeOnStart = (env["PIKA_MOQ_PROBE_ON_START"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let notificationUrl = (env["PIKA_NOTIFICATION_URL"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        ensureDefaultConfig(
            dataDirUrl: dataDirUrl,
            uiTestReset: uiTestReset,
            relays: relays,
            kpRelays: kpRelays,
            callMoqUrl: callMoqUrl,
            callBroadcastPrefix: callBroadcastPrefix,
            moqProbeOnStart: moqProbeOnStart,
            notificationUrl: notificationUrl
        )

        let core = FfiApp(dataDir: dataDir, keychainGroup: keychainGroup)
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
            callAudioSession.apply(activeCall: s.activeCall)
            if isRestoringSession {
                // Clear once we've transitioned away from login (success) or if
                // the router settles on login (restore failed / nsec invalid).
                if s.auth != .loggedOut || s.router.defaultScreen != .login {
                    isRestoringSession = false
                }
            }
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
        PushNotificationManager.shared.requestPermissionAndRegister()
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

    /// Moves existing data from the old app-private Application Support directory
    /// to the shared App Group container. Runs once; a sentinel file prevents re-runs.
    private static func migrateDataDirIfNeeded(fm: FileManager, newDir: URL) {
        let sentinel = newDir.appendingPathComponent(".migrated_to_app_group")
        if fm.fileExists(atPath: sentinel.path) { return }

        let oldDir = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        guard fm.fileExists(atPath: oldDir.path) else {
            // Nothing to migrate – first install.
            try? fm.createDirectory(at: newDir, withIntermediateDirectories: true)
            fm.createFile(atPath: sentinel.path, contents: nil)
            return
        }

        try? fm.createDirectory(at: newDir, withIntermediateDirectories: true)

        // Move each item from old dir to new dir.
        if let items = try? fm.contentsOfDirectory(atPath: oldDir.path) {
            for item in items {
                let src = oldDir.appendingPathComponent(item)
                let dst = newDir.appendingPathComponent(item)
                if fm.fileExists(atPath: dst.path) { continue }
                try? fm.moveItem(at: src, to: dst)
            }
        }

        fm.createFile(atPath: sentinel.path, contents: nil)
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

private func ensureDefaultConfig(
    dataDirUrl: URL,
    uiTestReset: Bool,
    relays: String,
    kpRelays: String,
    callMoqUrl: String,
    callBroadcastPrefix: String,
    moqProbeOnStart: String,
    notificationUrl: String
) {
    // Ensure call config exists even when no env overrides are set (call runtime requires `call_moq_url`).
    // If the file already exists, only fill missing keys to avoid clobbering user/tooling overrides.
    //
    // Important: do NOT write `disable_network` here. Tests rely on `PIKA_DISABLE_NETWORK=1`
    // taking effect when the config file omits `disable_network`.
    let defaultMoqUrl = "https://us-east.moq.logos.surf/anon"
    let defaultBroadcastPrefix = "pika/calls"

    let wantsOverride = uiTestReset
        || !relays.isEmpty
        || !kpRelays.isEmpty
        || !callMoqUrl.isEmpty
        || !callBroadcastPrefix.isEmpty
        || moqProbeOnStart == "1"
        || !notificationUrl.isEmpty

    let path = dataDirUrl.appendingPathComponent("pika_config.json")
    var obj: [String: Any] = [:]
    if let data = try? Data(contentsOf: path),
       let decoded = try? JSONSerialization.jsonObject(with: data, options: []),
       let dict = decoded as? [String: Any] {
        obj = dict
    }

    func isMissingOrBlank(_ key: String) -> Bool {
        guard let raw = obj[key] else { return true }
        let v = String(describing: raw).trimmingCharacters(in: .whitespacesAndNewlines)
        return v.isEmpty || v == "(null)"
    }

    var changed = false

    let resolvedCallMoqUrl = callMoqUrl.isEmpty ? defaultMoqUrl : callMoqUrl
    if isMissingOrBlank("call_moq_url") {
        obj["call_moq_url"] = resolvedCallMoqUrl
        changed = true
    }

    let resolvedCallBroadcastPrefix = callBroadcastPrefix.isEmpty ? defaultBroadcastPrefix : callBroadcastPrefix
    if isMissingOrBlank("call_broadcast_prefix") {
        obj["call_broadcast_prefix"] = resolvedCallBroadcastPrefix
        changed = true
    }

    if wantsOverride {
        let relayItems = relays
            .split(separator: ",")
            .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        var kpItems = kpRelays
            .split(separator: ",")
            .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }

        if kpItems.isEmpty {
            kpItems = relayItems
        }

        if moqProbeOnStart == "1" && (obj["moq_probe_on_start"] as? Bool) != true {
            obj["moq_probe_on_start"] = true
            changed = true
        }

        if !relayItems.isEmpty {
            obj["relay_urls"] = relayItems
            obj["key_package_relay_urls"] = kpItems
            changed = true
        }

        if !notificationUrl.isEmpty {
            obj["notification_url"] = notificationUrl
            changed = true
        }
    }

    guard changed else { return }
    guard let out = try? JSONSerialization.data(withJSONObject: obj, options: []) else { return }
    try? out.write(to: path, options: .atomic)
}
