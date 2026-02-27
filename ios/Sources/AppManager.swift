import Foundation
import Observation

protocol AppCore: AnyObject, Sendable {
    func dispatch(action: AppAction)
    func listenForUpdates(reconciler: AppReconciler)
    func state() -> AppState
    func setVideoFrameReceiver(receiver: VideoFrameReceiver)
    func sendVideoFrame(payload: Data)
}

extension FfiApp: AppCore {}

enum StoredAuthMode: Equatable {
    case localNsec
    case bunker
}

struct StoredAuth: Equatable {
    let mode: StoredAuthMode
    let nsec: String?
    let bunkerUri: String?
    let bunkerClientNsec: String?
}

protocol AuthStore: AnyObject {
    func load() -> StoredAuth?
    func saveLocalNsec(_ nsec: String)
    func saveBunker(bunkerUri: String, bunkerClientNsec: String)
    func clear()
    func getNsec() -> String?
}

final class KeychainAuthStore: AuthStore {
    private let localNsecStore: KeychainNsecStore
    private let bunkerClientNsecStore: KeychainNsecStore
    private let defaults = UserDefaults.standard
    private let modeKey = "pika.auth.mode"
    private let bunkerUriKey = "pika.auth.bunker_uri"

    init(keychainGroup: String? = nil) {
        localNsecStore = KeychainNsecStore(account: "nsec", keychainGroup: keychainGroup)
        bunkerClientNsecStore = KeychainNsecStore(account: "bunker_client_nsec", keychainGroup: keychainGroup)
    }

    func load() -> StoredAuth? {
        guard let modeRaw = defaults.string(forKey: modeKey) else {
            if let nsec = localNsecStore.getNsec(), !nsec.isEmpty {
                return StoredAuth(mode: .localNsec, nsec: nsec, bunkerUri: nil, bunkerClientNsec: nil)
            }
            return nil
        }

        switch modeRaw {
        case "local_nsec":
            guard let nsec = localNsecStore.getNsec(), !nsec.isEmpty else { return nil }
            return StoredAuth(mode: .localNsec, nsec: nsec, bunkerUri: nil, bunkerClientNsec: nil)
        case "bunker":
            let bunkerUri = defaults.string(forKey: bunkerUriKey)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            let clientNsec = bunkerClientNsecStore.getNsec()?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            guard !bunkerUri.isEmpty, !clientNsec.isEmpty else { return nil }
            return StoredAuth(
                mode: .bunker,
                nsec: nil,
                bunkerUri: bunkerUri,
                bunkerClientNsec: clientNsec
            )
        default:
            return nil
        }
    }

    func saveLocalNsec(_ nsec: String) {
        let trimmed = nsec.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        localNsecStore.setNsec(trimmed)
        bunkerClientNsecStore.clearNsec()
        defaults.removeObject(forKey: bunkerUriKey)
        defaults.set("local_nsec", forKey: modeKey)
    }

    func saveBunker(bunkerUri: String, bunkerClientNsec: String) {
        let uri = bunkerUri.trimmingCharacters(in: .whitespacesAndNewlines)
        let nsec = bunkerClientNsec.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !uri.isEmpty, !nsec.isEmpty else { return }
        bunkerClientNsecStore.setNsec(nsec)
        localNsecStore.clearNsec()
        defaults.set(uri, forKey: bunkerUriKey)
        defaults.set("bunker", forKey: modeKey)
    }

    func clear() {
        localNsecStore.clearNsec()
        bunkerClientNsecStore.clearNsec()
        defaults.removeObject(forKey: modeKey)
        defaults.removeObject(forKey: bunkerUriKey)
    }

    func getNsec() -> String? {
        guard let stored = load(), stored.mode == .localNsec else { return nil }
        return stored.nsec
    }
}

@MainActor
@Observable
final class AppManager: AppReconciler {
    private static let migrationSentinelName = ".migrated_to_app_group"
    private(set) var core: AppCore
    var state: AppState
    private var lastRevApplied: UInt64
    private let authStore: AuthStore
    /// True while we're waiting for a stored session to be restored by Rust.
    var isRestoringSession: Bool = false
    private let callAudioSession = CallAudioSessionCoordinator()

    init(core: AppCore, authStore: AuthStore) {
        self.core = core
        self.authStore = authStore

        let initial = core.state()
        self.state = initial
        self.lastRevApplied = initial.rev
        callAudioSession.apply(activeCall: initial.activeCall)

        core.listenForUpdates(reconciler: self)

        PushNotificationManager.shared.onTokenReceived = { [weak self] token in
            self?.dispatch(.setPushToken(token: token))
        }
        PushNotificationManager.shared.onReregisterRequested = { [weak self] in
            self?.dispatch(.reregisterPush)
        }

        if let stored = authStore.load() {
            isRestoringSession = true
            switch stored.mode {
            case .localNsec:
                if let nsec = stored.nsec, !nsec.isEmpty {
                    core.dispatch(action: .restoreSession(nsec: nsec))
                } else {
                    isRestoringSession = false
                }
            case .bunker:
                if let bunkerUri = stored.bunkerUri, !bunkerUri.isEmpty,
                   let clientNsec = stored.bunkerClientNsec, !clientNsec.isEmpty {
                    core.dispatch(action: .restoreSessionBunker(bunkerUri: bunkerUri, clientNsec: clientNsec))
                } else {
                    isRestoringSession = false
                }
            }
            PushNotificationManager.shared.requestPermissionAndRegister()
        }
    }

    convenience init() {
        let fm = FileManager.default
        let keychainGroup = Bundle.main.infoDictionary?["PikaKeychainGroup"] as? String ?? ""
        let dataDirUrl = Self.resolveDataDirURL(fm: fm)
        let dataDir = dataDirUrl.path
        let authStore = KeychainAuthStore(keychainGroup: keychainGroup)

        // One-time migration: move existing data from the old app-private container
        // to the shared App Group container so the NSE can access the MLS database.
        Self.migrateDataDirIfNeeded(fm: fm, newDir: dataDirUrl)

        // UI tests need a clean slate and a way to inject relay overrides without relying on
        // external scripts.
        let env = ProcessInfo.processInfo.environment
        let uiTestReset = env["PIKA_UI_TEST_RESET"] == "1"
        if uiTestReset {
            authStore.clear()
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
        core.setExternalSignerBridge(bridge: IOSExternalSignerBridge())
        self.init(core: core, authStore: authStore)
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
            let existing = authStore.load()?.nsec ?? ""
            if existing.isEmpty && !nsec.isEmpty {
                authStore.saveLocalNsec(nsec)
            }
        } else if case .bunkerSessionDescriptor(_, let bunkerUri, let clientNsec) = update {
            if !bunkerUri.isEmpty, !clientNsec.isEmpty {
                authStore.saveBunker(bunkerUri: bunkerUri, bunkerClientNsec: clientNsec)
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
                authStore.saveLocalNsec(nsec)
            }
            state.rev = updateRev
            callAudioSession.apply(activeCall: state.activeCall)
        case .bunkerSessionDescriptor(_, let bunkerUri, let clientNsec):
            if !bunkerUri.isEmpty, !clientNsec.isEmpty {
                authStore.saveBunker(bunkerUri: bunkerUri, bunkerClientNsec: clientNsec)
            }
            state.rev = updateRev
            callAudioSession.apply(activeCall: state.activeCall)
        }

        syncAuthStoreWithAuthState()
    }

    func dispatch(_ action: AppAction) {
        core.dispatch(action: action)
    }

    func login(nsec: String) {
        if !nsec.isEmpty {
            authStore.saveLocalNsec(nsec)
        }
        dispatch(.login(nsec: nsec))
        PushNotificationManager.shared.requestPermissionAndRegister()
    }

    func loginWithBunker(bunkerUri: String) {
        dispatch(.beginBunkerLogin(bunkerUri: bunkerUri))
    }

    func loginWithNostrConnect() {
        dispatch(.beginNostrConnectLogin)
    }

    func resetNostrConnectPairing() {
        dispatch(.resetNostrConnectPairing)
    }

    func logout() {
        authStore.clear()
        dispatch(.logout)
    }

    var isDeveloperModeEnabled: Bool {
        state.developerMode
    }

    func enableDeveloperMode() {
        dispatch(.enableDeveloperMode)
    }

    func wipeLocalDataForDeveloperTools() {
        authStore.clear()
        ensureMigrationSentinelExists()
        dispatch(.wipeLocalData)
    }

    func onForeground() {
        NSLog("[PikaAppManager] onForeground dispatching Foregrounded")
        dispatch(.foregrounded)
    }

    func onOpenURL(_ url: URL) {
        if let npub = Self.parseChatDeepLink(url) {
            NSLog("[PikaAppManager] onOpenURL dispatching CreateChat for: \(npub)")
            dispatch(.createChat(peerNpub: npub))
            return
        }

        guard isExpectedNostrConnectCallback(url) else {
            NSLog("[PikaAppManager] onOpenURL ignored unexpected URL: \(url.absoluteString)")
            return
        }
        NSLog("[PikaAppManager] onOpenURL dispatching NostrConnectCallback: \(url.absoluteString)")
        dispatch(.nostrConnectCallback(url: url.absoluteString))
    }

    nonisolated static func parseChatDeepLink(_ url: URL) -> String? {
        guard url.host?.lowercased() == "chat" else { return nil }
        let npub = url.pathComponents.dropFirst().first ?? ""
        guard isValidPeerKey(input: npub) else { return nil }
        return npub
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
        authStore.getNsec()
    }

    /// Moves existing data from the old app-private Application Support directory
    /// to the shared App Group container. Runs once; a sentinel file prevents re-runs.
    private static func migrateDataDirIfNeeded(fm: FileManager, newDir: URL) {
        let sentinel = newDir.appendingPathComponent(Self.migrationSentinelName)
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

    private static func resolveDataDirURL(fm: FileManager) -> URL {
        let appGroup = Bundle.main.infoDictionary?["PikaAppGroup"] as? String ?? "group.org.pikachat.pika"
        if let groupContainer = fm.containerURL(forSecurityApplicationGroupIdentifier: appGroup) {
            return groupContainer.appendingPathComponent("Library/Application Support")
        }
        // Fallback for simulator builds where CODE_SIGNING_ALLOWED=NO
        // means entitlements aren't embedded and the app group container
        // is unavailable. NSE won't work but the app itself runs fine.
        return fm.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
    }

    private func ensureMigrationSentinelExists() {
        let fm = FileManager.default
        let dataDirUrl = Self.resolveDataDirURL(fm: fm)
        try? fm.createDirectory(at: dataDirUrl, withIntermediateDirectories: true)
        let sentinel = dataDirUrl.appendingPathComponent(Self.migrationSentinelName)
        if !fm.fileExists(atPath: sentinel.path) {
            fm.createFile(atPath: sentinel.path, contents: nil)
        }
    }

    private func syncAuthStoreWithAuthState() {
        guard case .loggedIn(_, _, let mode) = state.auth else { return }

        switch mode {
        case .localNsec:
            if authStore.load()?.mode != .localNsec {
                authStore.clear()
            }
        case .bunkerSigner(let bunkerUri):
            let clientNsec = authStore.load()?.bunkerClientNsec ?? ""
            if !clientNsec.isEmpty {
                authStore.saveBunker(bunkerUri: bunkerUri, bunkerClientNsec: clientNsec)
            }
        case .externalSigner:
            break
        }
    }

    private func isExpectedNostrConnectCallback(_ url: URL) -> Bool {
        guard url.host?.lowercased() == "nostrconnect-return" else { return false }
        guard let scheme = url.scheme?.lowercased() else { return false }
        let expectedScheme =
            IOSExternalSignerBridge
                .callbackScheme(forBundleIdentifier: Bundle.main.bundleIdentifier)
                .lowercased()
        return scheme == expectedScheme
    }
}

private extension AppUpdate {
    var rev: UInt64 {
        switch self {
        case .fullState(let s): return s.rev
        case .accountCreated(let rev, _, _, _): return rev
        case .bunkerSessionDescriptor(let rev, _, _): return rev
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
    // Default external signer support to enabled, matching Android behavior.
    // If tooling or a user config sets an explicit value, keep it.
    if obj["enable_external_signer"] == nil {
        obj["enable_external_signer"] = true
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
