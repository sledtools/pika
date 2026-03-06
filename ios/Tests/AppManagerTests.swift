import XCTest
@testable import Pika

private func makeTestState(rev: UInt64, toast: String? = nil) -> AppState {
    AppState(
        rev: rev,
        router: Router(defaultScreen: .chatList, screenStack: []),
        auth: .loggedOut,
        myProfile: MyProfileState(name: "", about: "", pictureUrl: nil),
        busy: BusyState(
            creatingAccount: false,
            loggingIn: false,
            creatingChat: false,
            startingPersonalAgent: false,
            fetchingFollowList: false
        ),
        chatList: [],
        currentChat: nil,
        followList: [],
        peerProfile: nil,
        activeCall: nil,
        callTimeline: [],
        toast: toast,
        developerMode: false,
        voiceRecording: nil
    )
}

final class AppManagerTests: XCTestCase {
    func makeState(rev: UInt64, toast: String? = nil) -> AppState {
        AppState(
            rev: rev,
            router: Router(defaultScreen: .chatList, screenStack: []),
            auth: .loggedOut,
            myProfile: MyProfileState(name: "", about: "", pictureUrl: nil),
            busy: BusyState(
                creatingAccount: false,
                loggingIn: false,
                creatingChat: false,
                startingPersonalAgent: false,
                fetchingFollowList: false
            ),
            chatList: [],
            currentChat: nil,
            followList: [],
            peerProfile: nil,
            activeCall: nil,
            callTimeline: [],
            toast: toast,
            developerMode: false,
            voiceRecording: nil
        )
    }

    func testInitRestoresSessionWhenNsecExists() async {
        let core = MockCore(state: makeState(rev: 1))
        let store = MockAuthStore(stored: StoredAuth(mode: .localNsec, nsec: "nsec1test", bunkerUri: nil, bunkerClientNsec: nil))

        _ = await MainActor.run { AppManager(core: core, authStore: store) }

        XCTAssertEqual(core.dispatchedActions, [.restoreSession(nsec: "nsec1test")])
    }

    func testInitRestoresSessionWhenBunkerStored() async {
        let core = MockCore(state: makeState(rev: 1))
        let store = MockAuthStore(
            stored: StoredAuth(
                mode: .bunker,
                nsec: nil,
                bunkerUri: "bunker://abc?relay=wss://relay.example.com",
                bunkerClientNsec: "nsec1client"
            )
        )

        _ = await MainActor.run { AppManager(core: core, authStore: store) }

        XCTAssertEqual(
            core.dispatchedActions,
            [.restoreSessionBunker(bunkerUri: "bunker://abc?relay=wss://relay.example.com", clientNsec: "nsec1client")]
        )
    }

    func testApplyFullStateUpdatesState() async {
        let core = MockCore(state: makeState(rev: 1, toast: "old"))
        let store = MockAuthStore()
        let manager = await MainActor.run { AppManager(core: core, authStore: store) }

        let newState = makeState(rev: 2, toast: "new")
        await MainActor.run { manager.apply(update: .fullState(newState)) }

        let observed = await MainActor.run { manager.state }
        XCTAssertEqual(observed, newState)
    }

    func testApplyDropsStaleFullState() async {
        let initial = makeState(rev: 2, toast: "keep")
        let core = MockCore(state: initial)
        let store = MockAuthStore()
        let manager = await MainActor.run { AppManager(core: core, authStore: store) }

        let stale = makeState(rev: 1, toast: "stale")
        await MainActor.run { manager.apply(update: .fullState(stale)) }

        let observed = await MainActor.run { manager.state }
        XCTAssertEqual(observed, initial)
    }

    func testAccountCreatedStoresNsecEvenWhenStale() async {
        let core = MockCore(state: makeState(rev: 5))
        let store = MockAuthStore()
        let manager = await MainActor.run { AppManager(core: core, authStore: store) }

        await MainActor.run {
            manager.apply(update: .accountCreated(rev: 3, nsec: "nsec1stale", pubkey: "pk", npub: "npub"))
        }

        XCTAssertEqual(store.stored?.nsec, "nsec1stale")
        let observedRev = await MainActor.run { manager.state.rev }
        XCTAssertEqual(observedRev, 5)
    }

    func testDogfoodButtonHiddenForBunkerAuthWithoutLocalNsec() async {
        var state = makeState(rev: 1)
        state.auth = .loggedIn(
            npub: "npub1bunkeruser",
            pubkey: "pubkey1bunkeruser",
            mode: .bunkerSigner(bunkerUri: "bunker://signer.example")
        )
        let core = MockCore(state: state)
        let store = MockAuthStore(
            stored: StoredAuth(
                mode: .bunker,
                nsec: nil,
                bunkerUri: "bunker://signer.example",
                bunkerClientNsec: "nsec1client"
            )
        )
        let manager = await MainActor.run { AppManager(core: core, authStore: store) }

        let buttonState = await MainActor.run {
            manager.dogfoodAgentButtonState(for: "npub1bunkeruser")
        }

        XCTAssertNil(buttonState)
    }

    func testDogfoodButtonVisibleWhenLocalSigningNsecExists() async {
        var state = makeState(rev: 1)
        state.auth = .loggedIn(
            npub: "npub1localuser",
            pubkey: "pubkey1localuser",
            mode: .localNsec
        )
        let core = MockCore(state: state)
        let store = MockAuthStore(
            stored: StoredAuth(
                mode: .localNsec,
                nsec: "nsec1localuser",
                bunkerUri: nil,
                bunkerClientNsec: nil
            )
        )
        let manager = await MainActor.run { AppManager(core: core, authStore: store) }

        let buttonState = await MainActor.run {
            manager.dogfoodAgentButtonState(for: "npub1localuser")
        }

        XCTAssertEqual(buttonState, DogfoodAgentButtonState(title: "Start Personal Agent", isBusy: false))
    }

    func testDogfoodButtonBusyUsesPersonalAgentFlagOnly() async {
        var state = makeState(rev: 1)
        state.auth = .loggedIn(
            npub: "npub1localuser",
            pubkey: "pubkey1localuser",
            mode: .localNsec
        )
        state.busy = BusyState(
            creatingAccount: false,
            loggingIn: false,
            creatingChat: true,
            startingPersonalAgent: false,
            fetchingFollowList: false
        )
        let core = MockCore(state: state)
        let manager = await MainActor.run { AppManager(core: core, authStore: MockAuthStore()) }

        let notBusy = await MainActor.run {
            manager.dogfoodAgentButtonState(for: "npub1localuser")
        }
        XCTAssertEqual(notBusy, DogfoodAgentButtonState(title: "Start Personal Agent", isBusy: false))

        var stateBusy = state
        stateBusy.busy = BusyState(
            creatingAccount: false,
            loggingIn: false,
            creatingChat: false,
            startingPersonalAgent: true,
            fetchingFollowList: false
        )
        let busyCore = MockCore(state: stateBusy)
        let busyManager = await MainActor.run { AppManager(core: busyCore, authStore: MockAuthStore()) }
        let busy = await MainActor.run {
            busyManager.dogfoodAgentButtonState(for: "npub1localuser")
        }
        XCTAssertEqual(busy, DogfoodAgentButtonState(title: "Starting Personal Agent...", isBusy: true))
    }
}

final class ChatDeepLinkTests: XCTestCase {
    // A valid 64-char hex pubkey (always passes isValidPeerKey).
    private let validNpub = String(repeating: "a", count: 64)

    func testParseChatDeepLink_validNpub() {
        let url = URL(string: "pika://chat/\(validNpub)")!
        XCTAssertEqual(AppManager.parseChatDeepLink(url), validNpub)
    }

    func testParseChatDeepLink_validNpubWithTrailingSlash() {
        let url = URL(string: "pika://chat/\(validNpub)/")!
        XCTAssertEqual(AppManager.parseChatDeepLink(url), validNpub)
    }

    func testParseChatDeepLink_wrongHost() {
        let url = URL(string: "pika://nostrconnect-return/\(validNpub)")!
        XCTAssertNil(AppManager.parseChatDeepLink(url))
    }

    func testParseChatDeepLink_invalidNpub() {
        let url = URL(string: "pika://chat/garbage")!
        XCTAssertNil(AppManager.parseChatDeepLink(url))
    }

    func testParseChatDeepLink_missingPath() {
        let url = URL(string: "pika://chat")!
        XCTAssertNil(AppManager.parseChatDeepLink(url))
    }

    func testShareDispatchDeepLink_validHost() {
        let url = URL(string: "pika://share-send")!
        XCTAssertTrue(AppManager.isShareDispatchDeepLink(url))
    }

    func testShareDispatchDeepLink_wrongHost() {
        let url = URL(string: "pika://chat/abc")!
        XCTAssertFalse(AppManager.isShareDispatchDeepLink(url))
    }

    func testOnOpenURL_dispatchesCreateChat() async {
        let core = MockCore(state: makeTestState(rev: 1))
        let store = MockAuthStore()
        let manager = await MainActor.run { AppManager(core: core, authStore: store) }

        let url = URL(string: "pika://chat/\(validNpub)")!
        await MainActor.run { manager.onOpenURL(url) }

        XCTAssertEqual(core.dispatchedActions, [.createChat(peerNpub: validNpub)])
    }
}

final class MockCore: AppCore, @unchecked Sendable {
    private let stateValue: AppState
    private(set) var dispatchedActions: [AppAction] = []
    weak var reconciler: AppReconciler?
    private(set) var videoFrameReceiver: VideoFrameReceiver?
    private(set) var sentVideoFrames: [Data] = []

    init(state: AppState) {
        self.stateValue = state
    }

    func dispatch(action: AppAction) {
        dispatchedActions.append(action)
    }

    func listenForUpdates(reconciler: AppReconciler) {
        self.reconciler = reconciler
    }

    func state() -> AppState {
        stateValue
    }

    func setVideoFrameReceiver(receiver: VideoFrameReceiver) {
        videoFrameReceiver = receiver
    }

    func sendVideoFrame(payload: Data) {
        sentVideoFrames.append(payload)
    }
}

final class MockAuthStore: AuthStore {
    var stored: StoredAuth?

    init(stored: StoredAuth? = nil) {
        self.stored = stored
    }

    func load() -> StoredAuth? {
        stored
    }

    func saveLocalNsec(_ nsec: String) {
        stored = StoredAuth(mode: .localNsec, nsec: nsec, bunkerUri: nil, bunkerClientNsec: nil)
    }

    func saveBunker(bunkerUri: String, bunkerClientNsec: String) {
        stored = StoredAuth(mode: .bunker, nsec: nil, bunkerUri: bunkerUri, bunkerClientNsec: bunkerClientNsec)
    }

    func getNsec() -> String? {
        guard stored?.mode == .localNsec else { return nil }
        return stored?.nsec
    }

    func clear() {
        stored = nil
    }
}
