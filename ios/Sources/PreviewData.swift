#if DEBUG
import SwiftUI

@MainActor
enum PreviewFactory {
    static func manager(_ state: AppState) -> AppManager {
        AppManager(core: PreviewCore(state: state), nsecStore: PreviewNsecStore())
    }
}

final class PreviewCore: AppCore, @unchecked Sendable {
    private let stateValue: AppState

    init(state: AppState) {
        self.stateValue = state
    }

    func dispatch(action: AppAction) {}

    func listenForUpdates(reconciler: AppReconciler) {}

    func state() -> AppState {
        stateValue
    }
}

final class PreviewNsecStore: NsecStore {
    func getNsec() -> String? { nil }
    func setNsec(_ nsec: String) {}
    func clearNsec() {}
}

enum PreviewAppState {
    static var loggedOut: AppState {
        base(
            rev: 1,
            router: Router(defaultScreen: .login, screenStack: []),
            auth: .loggedOut
        )
    }

    static var loggingIn: AppState {
        base(
            rev: 2,
            router: Router(defaultScreen: .login, screenStack: []),
            auth: .loggedOut,
            busy: BusyState(creatingAccount: false, loggingIn: true, creatingChat: false)
        )
    }

    static var chatListEmpty: AppState {
        base(
            rev: 10,
            router: Router(defaultScreen: .chatList, screenStack: []),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            chatList: []
        )
    }

    static var chatListPopulated: AppState {
        base(
            rev: 11,
            router: Router(defaultScreen: .chatList, screenStack: []),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            chatList: [
                chatSummary(
                    id: "chat-1",
                    name: "Justin",
                    lastMessage: "See you at the relay.",
                    unread: 2
                ),
                chatSummary(
                    id: "chat-2",
                    name: "Satoshi Nakamoto",
                    lastMessage: "Long time no see.",
                    unread: 0
                ),
                chatSummary(
                    id: "chat-3",
                    name: nil,
                    lastMessage: "npub-only peer",
                    unread: 4
                ),
            ]
        )
    }

    static var creatingChat: AppState {
        base(
            rev: 12,
            router: Router(defaultScreen: .newChat, screenStack: [.newChat]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            busy: BusyState(creatingAccount: false, loggingIn: false, creatingChat: true)
        )
    }

    static var newChatIdle: AppState {
        base(
            rev: 13,
            router: Router(defaultScreen: .newChat, screenStack: [.newChat]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey)
        )
    }

    static var chatDetail: AppState {
        base(
            rev: 20,
            router: Router(defaultScreen: .chat(chatId: "chat-1"), screenStack: [.chat(chatId: "chat-1")]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            currentChat: chatViewState(id: "chat-1", name: "Justin", failed: false)
        )
    }

    static var chatDetailFailed: AppState {
        base(
            rev: 21,
            router: Router(defaultScreen: .chat(chatId: "chat-1"), screenStack: [.chat(chatId: "chat-1")]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            currentChat: chatViewState(id: "chat-1", name: "Justin", failed: true)
        )
    }

    private static func base(
        rev: UInt64,
        router: Router,
        auth: AuthState,
        busy: BusyState = BusyState(creatingAccount: false, loggingIn: false, creatingChat: false),
        chatList: [ChatSummary] = [],
        currentChat: ChatViewState? = nil,
        toast: String? = nil
    ) -> AppState {
        AppState(
            rev: rev,
            router: router,
            auth: auth,
            busy: busy,
            chatList: chatList,
            currentChat: currentChat,
            toast: toast
        )
    }

    private static func chatSummary(id: String, name: String?, lastMessage: String, unread: UInt32) -> ChatSummary {
        ChatSummary(
            chatId: id,
            peerNpub: samplePeerNpub,
            peerName: name,
            peerPictureUrl: nil,
            lastMessage: lastMessage,
            lastMessageAt: 1_709_000_000,
            unreadCount: unread
        )
    }

    private static func chatViewState(id: String, name: String?, failed: Bool) -> ChatViewState {
        let messages: [ChatMessage] = [
            ChatMessage(
                id: "m1",
                senderPubkey: samplePubkey,
                content: "Hey! Are we still on for today?",
                timestamp: 1_709_000_001,
                isMine: true,
                delivery: .sent
            ),
            ChatMessage(
                id: "m2",
                senderPubkey: samplePeerPubkey,
                content: "Yep. See you at the relay.",
                timestamp: 1_709_000_050,
                isMine: false,
                delivery: .sent
            ),
            ChatMessage(
                id: "m3",
                senderPubkey: samplePubkey,
                content: failed ? "This one failed to send." : "On my way.",
                timestamp: 1_709_000_100,
                isMine: true,
                delivery: failed ? .failed(reason: "Network timeout") : .pending
            ),
        ]

        return ChatViewState(
            chatId: id,
            peerNpub: samplePeerNpub,
            peerName: name,
            peerPictureUrl: nil,
            messages: messages,
            canLoadOlder: true
        )
    }

    private static let sampleNpub = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y"
    private static let samplePubkey = "11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c"
    private static let samplePeerNpub = "npub1y2z0c7un9dwmhk4zrpw8df8p0gh0j2x54qhznwqjnp452ju4078srmwp70"
    private static let samplePeerPubkey = "2284fc7b932b5dbbdaa2185c76a4e17a2ef928d4a82e29b812986b454b957f8f"
}
#endif
