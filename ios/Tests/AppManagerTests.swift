import XCTest
@testable import Pika

final class AppManagerTests: XCTestCase {
    private func makeState(rev: UInt64, toast: String? = nil) -> AppState {
        AppState(
            rev: rev,
            router: Router(defaultScreen: .chatList, screenStack: []),
            auth: .loggedOut,
            myProfile: MyProfileState(name: "", about: "", pictureUrl: nil),
            busy: BusyState(creatingAccount: false, loggingIn: false, creatingChat: false, fetchingFollowList: false),
            chatList: [],
            currentChat: nil,
            followList: [],
            toast: toast
        )
    }

    func testInitRestoresSessionWhenNsecExists() async {
        let core = MockCore(state: makeState(rev: 1))
        let store = MockNsecStore(nsec: "nsec1test")

        _ = await MainActor.run { AppManager(core: core, nsecStore: store) }

        XCTAssertEqual(core.dispatchedActions, [.restoreSession(nsec: "nsec1test")])
    }

    func testApplyFullStateUpdatesState() async {
        let core = MockCore(state: makeState(rev: 1, toast: "old"))
        let store = MockNsecStore()
        let manager = await MainActor.run { AppManager(core: core, nsecStore: store) }

        let newState = makeState(rev: 2, toast: "new")
        await MainActor.run { manager.apply(update: .fullState(newState)) }

        let observed = await MainActor.run { manager.state }
        XCTAssertEqual(observed, newState)
    }

    func testApplyDropsStaleFullState() async {
        let initial = makeState(rev: 2, toast: "keep")
        let core = MockCore(state: initial)
        let store = MockNsecStore()
        let manager = await MainActor.run { AppManager(core: core, nsecStore: store) }

        let stale = makeState(rev: 1, toast: "stale")
        await MainActor.run { manager.apply(update: .fullState(stale)) }

        let observed = await MainActor.run { manager.state }
        XCTAssertEqual(observed, initial)
    }

    func testAccountCreatedStoresNsecEvenWhenStale() async {
        let core = MockCore(state: makeState(rev: 5))
        let store = MockNsecStore()
        let manager = await MainActor.run { AppManager(core: core, nsecStore: store) }

        await MainActor.run {
            manager.apply(update: .accountCreated(rev: 3, nsec: "nsec1stale", pubkey: "pk", npub: "npub"))
        }

        XCTAssertEqual(store.nsec, "nsec1stale")
        let observedRev = await MainActor.run { manager.state.rev }
        XCTAssertEqual(observedRev, 5)
    }
}

final class MockCore: AppCore, @unchecked Sendable {
    private let stateValue: AppState
    private(set) var dispatchedActions: [AppAction] = []
    weak var reconciler: AppReconciler?

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
}

final class MockNsecStore: NsecStore {
    var nsec: String?

    init(nsec: String? = nil) {
        self.nsec = nsec
    }

    func getNsec() -> String? {
        nsec
    }

    func setNsec(_ nsec: String) {
        self.nsec = nsec
    }

    func clearNsec() {
        nsec = nil
    }
}
