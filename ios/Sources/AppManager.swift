import Foundation
import Perception

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

private enum DogfoodAgentFlowState: Equatable {
    case idle
    case ensuring
    case polling
    case openingChat
    case failed

    var buttonState: DogfoodAgentButtonState {
        switch self {
        case .idle:
            return DogfoodAgentButtonState(title: "Start Personal Agent", isBusy: false)
        case .ensuring, .polling, .openingChat:
            return DogfoodAgentButtonState(title: "Starting Personal Agent...", isBusy: true)
        case .failed:
            return DogfoodAgentButtonState(title: "Retry Personal Agent", isBusy: false)
        }
    }
}

@MainActor
@Perceptible
final class AppManager: AppReconciler {
    private static let migrationSentinelName = ".migrated_to_app_group"
    private(set) var core: AppCore
    var state: AppState
    private var lastRevApplied: UInt64
    private let authStore: AuthStore
    /// True while we're waiting for a stored session to be restored by Rust.
    var isRestoringSession: Bool = false
    private var dogfoodAgentFlowState: DogfoodAgentFlowState = .idle
    private var dogfoodAgentTask: Task<Void, Never>?
    private let callAudioSession = CallAudioSessionCoordinator()
    private let agentControlClient: AgentControlClient
    private var lastSharedChatCache: [ShareableChatSummary]
    private var lastShareLoggedInFlag: Bool?
    private static let dogfoodAgentPollMaxAttempts = 60
    private static let dogfoodAgentPollDelayNanos: UInt64 = 2_000_000_000

    init(
        core: AppCore,
        authStore: AuthStore,
        agentControlClient: AgentControlClient = HttpAgentControlClient()
    ) {
        self.core = core
        self.authStore = authStore
        self.agentControlClient = agentControlClient

        let initial = core.state()
        self.state = initial
        self.lastRevApplied = initial.rev
        self.lastSharedChatCache = []
        self.lastShareLoggedInFlag = nil
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

        syncShareExtensionState(from: initial)
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
        let agentApiUrl = (env["PIKA_AGENT_API_URL"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let agentOwnerToken = (env["PIKA_AGENT_OWNER_TOKEN"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        ensureDefaultConfig(
            dataDirUrl: dataDirUrl,
            uiTestReset: uiTestReset,
            relays: relays,
            kpRelays: kpRelays,
            callMoqUrl: callMoqUrl,
            callBroadcastPrefix: callBroadcastPrefix,
            moqProbeOnStart: moqProbeOnStart,
            notificationUrl: notificationUrl,
            agentApiUrl: agentApiUrl,
            agentOwnerToken: agentOwnerToken
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
        syncShareExtensionState(from: state)
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
        cancelDogfoodAgentFlow()
        authStore.clear()
        clearShareExtensionState()
        dispatch(.logout)
    }

    var isDeveloperModeEnabled: Bool {
        state.developerMode
    }

    func enableDeveloperMode() {
        dispatch(.enableDeveloperMode)
    }

    func wipeProfileCacheForDeveloperTools() {
        dispatch(.wipeProfileCache)
    }

    func wipeLocalDataForDeveloperTools() {
        cancelDogfoodAgentFlow()
        authStore.clear()
        clearShareExtensionState()
        ensureMigrationSentinelExists()
        dispatch(.wipeLocalData)
    }

    func onForeground() {
        NSLog("[PikaAppManager] onForeground dispatching Foregrounded")
        dispatch(.foregrounded)
        processPendingShareQueue(openFirstChat: false)
    }

    func onOpenURL(_ url: URL) {
        if let npub = Self.parseChatDeepLink(url) {
            NSLog("[PikaAppManager] onOpenURL dispatching CreateChat for: \(npub)")
            dispatch(.createChat(peerNpub: npub))
            return
        }

        if Self.isShareDispatchDeepLink(url) {
            NSLog("[PikaAppManager] onOpenURL processing pending share queue")
            processPendingShareQueue(openFirstChat: true)
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

    nonisolated static func isShareDispatchDeepLink(_ url: URL) -> Bool {
        url.host?.lowercased() == "share-send"
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

    func dogfoodAgentButtonState(for npub: String?) -> DogfoodAgentButtonState? {
        guard isMicrovmDogfoodWhitelistedNpub(npub) else {
            return nil
        }
        return dogfoodAgentFlowState.buttonState
    }

    func ensureDogfoodAgent() {
        guard dogfoodAgentTask == nil else { return }
        guard isMicrovmDogfoodWhitelistedNpub(currentNpub()) else { return }
        guard let config = resolvedAgentApiConfiguration() else {
            dogfoodAgentFlowState = .failed
            return
        }

        dogfoodAgentTask = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.dogfoodAgentTask = nil }
            await self.runDogfoodAgentFlow(config: config)
        }
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

    private func syncShareExtensionState(from state: AppState) {
        let wasLoggedIn = lastShareLoggedInFlag ?? false
        let isLoggedIn: Bool
        switch state.auth {
        case .loggedOut:
            isLoggedIn = false
        case .loggedIn:
            isLoggedIn = true
        }

        if lastShareLoggedInFlag != isLoggedIn {
            ShareQueueManager.setLoggedIn(isLoggedIn)
            lastShareLoggedInFlag = isLoggedIn
        }

        let projectedChats: [ShareableChatSummary]
        if isLoggedIn {
            projectedChats = state.chatList.map { chat in
                ShareableChatSummary(
                    chatId: chat.chatId,
                    displayName: chat.displayName,
                    isGroup: chat.isGroup,
                    subtitle: chat.subtitle,
                    lastMessagePreview: chat.lastMessagePreview,
                    lastMessageAt: chat.lastMessageAt,
                    members: chat.members.map { member in
                        ShareableMember(
                            npub: member.npub,
                            name: member.name,
                            pictureUrl: member.pictureUrl
                        )
                    }
                )
            }
        } else {
            projectedChats = []
        }

        if projectedChats != lastSharedChatCache {
            ShareQueueManager.writeChatListCache(projectedChats)
            lastSharedChatCache = projectedChats
        }

        if isLoggedIn, !wasLoggedIn {
            processPendingShareQueue(openFirstChat: false)
        }
    }

    private func clearShareExtensionState() {
        ShareQueueManager.setLoggedIn(false)
        ShareQueueManager.writeChatListCache([])
        lastShareLoggedInFlag = false
        lastSharedChatCache = []
    }

    private func processPendingShareQueue(openFirstChat: Bool) {
        guard case .loggedIn = state.auth else { return }
        ShareQueueManager.runMaintenance()
        let pending = ShareQueueManager.dequeueBatch()
        guard !pending.isEmpty else { return }

        var firstOpenedChatId: String?
        for item in pending {
            switch item.kind {
            case .message(let content):
                dispatch(
                    .sendMessage(
                        chatId: item.chatId,
                        content: content,
                        kind: nil,
                        replyToMessageId: nil
                    )
                )
                if openFirstChat, firstOpenedChatId == nil {
                    firstOpenedChatId = item.chatId
                }
                ShareQueueManager.acknowledge(
                    ShareDispatchAck(
                        itemId: item.itemId,
                        status: .acceptedByCore,
                        errorCode: nil,
                        errorMessage: nil
                    )
                )

            case .media(let caption, let mimeType, let filename, let dataBase64):
                dispatch(
                    .sendChatMedia(
                        chatId: item.chatId,
                        dataBase64: dataBase64,
                        mimeType: mimeType,
                        filename: filename,
                        caption: caption
                    )
                )
                if openFirstChat, firstOpenedChatId == nil {
                    firstOpenedChatId = item.chatId
                }
                ShareQueueManager.acknowledge(
                    ShareDispatchAck(
                        itemId: item.itemId,
                        status: .acceptedByCore,
                        errorCode: nil,
                        errorMessage: nil
                    )
                )

            case .mediaBatch(let caption, let items):
                let batchItems = items.map { entry in
                    MediaBatchItem(
                        dataBase64: entry.dataBase64,
                        mimeType: entry.mimeType,
                        filename: entry.filename
                    )
                }
                dispatch(
                    .sendChatMediaBatch(
                        chatId: item.chatId,
                        items: batchItems,
                        caption: caption
                    )
                )
                if openFirstChat, firstOpenedChatId == nil {
                    firstOpenedChatId = item.chatId
                }
                ShareQueueManager.acknowledge(
                    ShareDispatchAck(
                        itemId: item.itemId,
                        status: .acceptedByCore,
                        errorCode: nil,
                        errorMessage: nil
                    )
                )
            }
        }

        if openFirstChat, let chatId = firstOpenedChatId {
            dispatch(.openChat(chatId: chatId))
        }
    }

    private func runDogfoodAgentFlow(config: AgentApiConfiguration) async {
        dogfoodAgentFlowState = .ensuring

        do {
            let ensuredAgentId: String
            do {
                let ensured = try await agentControlClient.ensureAgent(config: config)
                ensuredAgentId = ensured.agentId
            } catch AgentControlClientError.agentExists {
                ensuredAgentId = ""
            }

            dogfoodAgentFlowState = .polling
            let ready = try await pollForReadyAgent(config: config, seedAgentId: ensuredAgentId)

            dogfoodAgentFlowState = .openingChat
            openOrCreateDirectChat(withPeer: ready.agentId)
            dogfoodAgentFlowState = .idle
        } catch is CancellationError {
            // Flow was intentionally interrupted (logout/reset/app state change).
        } catch {
            NSLog("[PikaAppManager] dogfood agent flow failed: \(error)")
            dogfoodAgentFlowState = .failed
        }
    }

    private func pollForReadyAgent(
        config: AgentApiConfiguration,
        seedAgentId: String
    ) async throws -> AgentStateResponse {
        var latestAgentId = seedAgentId

        for attempt in 0..<Self.dogfoodAgentPollMaxAttempts {
            try Task.checkCancellation()

            let state: AgentStateResponse
            do {
                state = try await agentControlClient.getMyAgent(config: config)
            } catch AgentControlClientError.agentNotFound {
                if attempt < Self.dogfoodAgentPollMaxAttempts - 1 {
                    try await Task.sleep(nanoseconds: Self.dogfoodAgentPollDelayNanos)
                    continue
                }
                throw AgentControlClientError.agentNotFound
            }

            latestAgentId = state.agentId
            switch state.state {
            case .ready:
                return state
            case .creating:
                if attempt < Self.dogfoodAgentPollMaxAttempts - 1 {
                    try await Task.sleep(nanoseconds: Self.dogfoodAgentPollDelayNanos)
                }
            case .error:
                throw AgentControlClientError.remote("agent_in_error_state", statusCode: 500)
            }
        }

        throw AgentControlClientError.remote(
            latestAgentId.isEmpty ? "agent_timeout" : "agent_timeout_\(latestAgentId)",
            statusCode: 504
        )
    }

    private func openOrCreateDirectChat(withPeer peerKey: String) {
        let normalized = normalizePeerKey(input: peerKey)
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
        guard !normalized.isEmpty, isValidPeerKey(input: normalized) else {
            dogfoodAgentFlowState = .failed
            return
        }

        if let chatId = existingDirectChatId(forPeer: normalized) {
            dispatch(.openChat(chatId: chatId))
            return
        }
        dispatch(.createChat(peerNpub: normalized))
    }

    private func existingDirectChatId(forPeer peerKey: String) -> String? {
        state.chatList.first { chat in
            guard !chat.isGroup, let member = chat.members.first else {
                return false
            }
            let memberNpub = normalizePeerKey(input: member.npub)
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .lowercased()
            let memberPubkey = member.pubkey.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            return memberNpub == peerKey || memberPubkey == peerKey
        }?.chatId
    }

    private func cancelDogfoodAgentFlow() {
        dogfoodAgentTask?.cancel()
        dogfoodAgentTask = nil
        dogfoodAgentFlowState = .idle
    }

    private func currentNpub() -> String? {
        switch state.auth {
        case .loggedIn(let npub, _, _):
            return npub
        default:
            return nil
        }
    }

    private func resolvedAgentApiConfiguration() -> AgentApiConfiguration? {
        resolveAgentApiConfiguration(
            appConfig: loadAppConfigDictionary(),
            env: ProcessInfo.processInfo.environment,
            signingNsec: getNsec()
        )
    }

    private func loadAppConfigDictionary() -> [String: Any] {
        let fm = FileManager.default
        let dataDir = Self.resolveDataDirURL(fm: fm)
        let configPath = dataDir.appendingPathComponent("pika_config.json")
        guard let data = try? Data(contentsOf: configPath),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return [:]
        }
        return json
    }

    private func isExpectedNostrConnectCallback(_ url: URL) -> Bool {
        guard url.host?.lowercased() == "nostrconnect-return" else { return false }
        guard let scheme = url.scheme?.lowercased() else { return false }
        let expectedScheme = IOSExternalSignerBridge.callbackScheme().lowercased()
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
    notificationUrl: String,
    agentApiUrl: String,
    agentOwnerToken: String
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
        || !agentApiUrl.isEmpty
        || !agentOwnerToken.isEmpty

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
        if !agentApiUrl.isEmpty {
            obj["agent_api_url"] = agentApiUrl
            changed = true
        }
        if !agentOwnerToken.isEmpty {
            obj["agent_owner_token"] = agentOwnerToken
            changed = true
        }
    }

    guard changed else { return }
    guard let out = try? JSONSerialization.data(withJSONObject: obj, options: []) else { return }
    try? out.write(to: path, options: .atomic)
}
