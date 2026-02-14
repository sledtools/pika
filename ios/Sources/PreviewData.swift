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
            busy: BusyState(creatingAccount: false, loggingIn: true, creatingChat: false, fetchingFollowList: false)
        )
    }

    static var chatListEmpty: AppState {
        base(
            rev: 10,
            router: Router(defaultScreen: .chatList, screenStack: []),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile,
            chatList: []
        )
    }

    static var chatListPopulated: AppState {
        base(
            rev: 11,
            router: Router(defaultScreen: .chatList, screenStack: []),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile,
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

    static var chatListLongNames: AppState {
        base(
            rev: 12,
            router: Router(defaultScreen: .chatList, screenStack: []),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile,
            chatList: [
                chatSummary(
                    id: "chat-long-1",
                    name: "Alexandria Catherine Montgomery-Smythe",
                    lastMessage: "This is a deliberately long message preview to verify truncation.",
                    unread: 120
                ),
                chatSummary(
                    id: "chat-long-2",
                    name: "VeryVeryVeryLongDisplayNameWithoutSpaces",
                    lastMessage: "Short msg",
                    unread: 0
                ),
            ]
        )
    }

    static var creatingChat: AppState {
        base(
            rev: 13,
            router: Router(defaultScreen: .newChat, screenStack: [.newChat]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile,
            busy: BusyState(creatingAccount: false, loggingIn: false, creatingChat: true, fetchingFollowList: false)
        )
    }

    static var newChatIdle: AppState {
        base(
            rev: 14,
            router: Router(defaultScreen: .newChat, screenStack: [.newChat]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile
        )
    }

    static var chatDetail: AppState {
        base(
            rev: 30,
            router: Router(defaultScreen: .chat(chatId: "chat-1"), screenStack: [.chat(chatId: "chat-1")]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile,
            currentChat: chatViewState(id: "chat-1", name: "Justin", failed: false)
        )
    }

    static var chatDetailFailed: AppState {
        base(
            rev: 31,
            router: Router(defaultScreen: .chat(chatId: "chat-1"), screenStack: [.chat(chatId: "chat-1")]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile,
            currentChat: chatViewState(id: "chat-1", name: "Justin", failed: true)
        )
    }

    static var chatDetailEmpty: AppState {
        base(
            rev: 32,
            router: Router(defaultScreen: .chat(chatId: "chat-empty"), screenStack: [.chat(chatId: "chat-empty")]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile,
            currentChat: ChatViewState(
                chatId: "chat-empty",
                isGroup: false,
                groupName: nil,
                members: [MemberInfo(pubkey: samplePeerPubkey, npub: samplePeerNpub, name: "Empty Chat", pictureUrl: nil)],
                isAdmin: false,
                messages: [],
                canLoadOlder: false
            )
        )
    }

    static var chatDetailLongThread: AppState {
        base(
            rev: 33,
            router: Router(defaultScreen: .chat(chatId: "chat-long"), screenStack: [.chat(chatId: "chat-long")]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile,
            currentChat: chatViewStateLongThread()
        )
    }

    static var chatDetailGrouped: AppState {
        base(
            rev: 34,
            router: Router(defaultScreen: .chat(chatId: "chat-grouped"), screenStack: [.chat(chatId: "chat-grouped")]),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile,
            currentChat: chatViewStateGrouped()
        )
    }

    static var toastVisible: AppState {
        base(
            rev: 40,
            router: Router(defaultScreen: .chatList, screenStack: []),
            auth: .loggedIn(npub: sampleNpub, pubkey: samplePubkey),
            myProfile: sampleProfile,
            chatList: chatListPopulated.chatList,
            toast: "Network connection lost."
        )
    }

    static let sampleFollowList: [FollowListEntry] = [
        FollowListEntry(pubkey: samplePeerPubkey, npub: samplePeerNpub, name: "Justin", pictureUrl: "https://blossom.nostr.pub/8dbc6f42ea8bf53f4af89af87eb0d9110fcaf4d263f7d2cb9f29d68f95f6f8ce"),
        FollowListEntry(pubkey: sampleThirdPubkey, npub: sampleThirdNpub, name: "benthecarman", pictureUrl: nil),
        FollowListEntry(pubkey: "aabbccdd00112233aabbccdd00112233aabbccdd00112233aabbccdd00112233", npub: "npub14wavxd9qqpy3x64hkvajjrf9s67qfze2gs3a2pxhzu3fjlf90xesqa2haj", name: nil, pictureUrl: nil),
    ]

    private static func base(
        rev: UInt64,
        router: Router,
        auth: AuthState,
        myProfile: MyProfileState = .init(name: "", about: "", pictureUrl: nil),
        busy: BusyState = BusyState(creatingAccount: false, loggingIn: false, creatingChat: false, fetchingFollowList: false),
        chatList: [ChatSummary] = [],
        currentChat: ChatViewState? = nil,
        followList: [FollowListEntry] = [],
        toast: String? = nil
    ) -> AppState {
        AppState(
            rev: rev,
            router: router,
            auth: auth,
            myProfile: myProfile,
            busy: busy,
            chatList: chatList,
            currentChat: currentChat,
            followList: followList,
            toast: toast
        )
    }

    private static func chatSummary(id: String, name: String?, lastMessage: String, unread: UInt32) -> ChatSummary {
        ChatSummary(
            chatId: id,
            isGroup: false,
            groupName: nil,
            members: [MemberInfo(pubkey: samplePeerPubkey, npub: samplePeerNpub, name: name, pictureUrl: nil)],
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
                senderName: nil,
                content: "Hey! Are we still on for today?",
                displayContent: "Hey! Are we still on for today?",
                mentions: [],
                timestamp: 1_709_000_001,
                isMine: true,
                delivery: .sent
            ),
            ChatMessage(
                id: "m2",
                senderPubkey: samplePeerPubkey,
                senderName: name,
                content: "Yep. See you at the relay.",
                displayContent: "Yep. See you at the relay.",
                mentions: [],
                timestamp: 1_709_000_050,
                isMine: false,
                delivery: .sent
            ),
            ChatMessage(
                id: "m3",
                senderPubkey: samplePubkey,
                senderName: nil,
                content: failed ? "This one failed to send." : "On my way.",
                displayContent: failed ? "This one failed to send." : "On my way.",
                mentions: [],
                timestamp: 1_709_000_100,
                isMine: true,
                delivery: failed ? .failed(reason: "Network timeout") : .pending
            ),
        ]

        return ChatViewState(
            chatId: id,
            isGroup: false,
            groupName: nil,
            members: [MemberInfo(pubkey: samplePeerPubkey, npub: samplePeerNpub, name: name, pictureUrl: nil)],
            isAdmin: false,
            messages: messages,
            canLoadOlder: true
        )
    }

    private static func chatViewStateLongThread() -> ChatViewState {
        let messages = (0..<20).map { idx in
            let text = idx.isMultiple(of: 3)
                ? "A long message intended to wrap across multiple lines for layout validation."
                : "Message \(idx + 1)"
            return ChatMessage(
                id: "m\(idx)",
                senderPubkey: idx.isMultiple(of: 2) ? samplePubkey : samplePeerPubkey,
                senderName: idx.isMultiple(of: 2) ? nil : "Peer",
                content: text,
                displayContent: text,
                mentions: [],
                timestamp: Int64(1_709_000_200 + idx),
                isMine: idx.isMultiple(of: 2),
                delivery: .sent
            )
        }

        return ChatViewState(
            chatId: "chat-long",
            isGroup: false,
            groupName: nil,
            members: [MemberInfo(pubkey: samplePeerPubkey, npub: samplePeerNpub, name: "Long Thread", pictureUrl: nil)],
            isAdmin: false,
            messages: messages,
            canLoadOlder: true
        )
    }

    private static func chatViewStateGrouped() -> ChatViewState {
        let messages: [ChatMessage] = [
            ChatMessage(
                id: "gm1",
                senderPubkey: samplePeerPubkey,
                senderName: "Anthony",
                content: "hello",
                displayContent: "hello",
                mentions: [],
                timestamp: 1_709_001_000,
                isMine: false,
                delivery: .sent
            ),
            ChatMessage(
                id: "gm2",
                senderPubkey: samplePeerPubkey,
                senderName: "Anthony",
                content: "how are you",
                displayContent: "how are you",
                mentions: [],
                timestamp: 1_709_001_005,
                isMine: false,
                delivery: .sent
            ),
            ChatMessage(
                id: "gm3",
                senderPubkey: samplePubkey,
                senderName: nil,
                content: "lmk when you are here and I will find you",
                displayContent: "lmk when you are here and I will find you",
                mentions: [],
                timestamp: 1_709_001_020,
                isMine: true,
                delivery: .sent
            ),
            ChatMessage(
                id: "gm4",
                senderPubkey: samplePubkey,
                senderName: nil,
                content: "I am out by ana's market",
                displayContent: "I am out by ana's market",
                mentions: [],
                timestamp: 1_709_001_030,
                isMine: true,
                delivery: .pending
            ),
            ChatMessage(
                id: "gm5",
                senderPubkey: sampleThirdPubkey,
                senderName: "benthecarman",
                content: "We got locked out",
                displayContent: "We got locked out",
                mentions: [],
                timestamp: 1_709_001_040,
                isMine: false,
                delivery: .sent
            ),
            ChatMessage(
                id: "gm6",
                senderPubkey: sampleThirdPubkey,
                senderName: "benthecarman",
                content: "Nvm",
                displayContent: "Nvm",
                mentions: [],
                timestamp: 1_709_001_045,
                isMine: false,
                delivery: .sent
            ),
            ChatMessage(
                id: "gm7",
                senderPubkey: samplePeerPubkey,
                senderName: "Anthony",
                content: "https://raw.githubusercontent.com/shabegom/buttons/refs/heads/main/README.md",
                displayContent: "https://raw.githubusercontent.com/shabegom/buttons/refs/heads/main/README.md",
                mentions: [],
                timestamp: 1_709_001_080,
                isMine: false,
                delivery: .sent
            ),
        ]

        return ChatViewState(
            chatId: "chat-grouped",
            isGroup: true,
            groupName: "hackathon2",
            members: [
                MemberInfo(
                    pubkey: samplePeerPubkey,
                    npub: samplePeerNpub,
                    name: "Anthony",
                    pictureUrl: "https://blossom.nostr.pub/8dbc6f42ea8bf53f4af89af87eb0d9110fcaf4d263f7d2cb9f29d68f95f6f8ce"
                ),
                MemberInfo(
                    pubkey: sampleThirdPubkey,
                    npub: sampleThirdNpub,
                    name: "benthecarman",
                    pictureUrl: nil
                ),
            ],
            isAdmin: true,
            messages: messages,
            canLoadOlder: true
        )
    }

    static let sampleNpub = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y"
    static let samplePubkey = "11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c"
    static let samplePeerNpub = "npub1y2z0c7un9dwmhk4zrpw8df8p0gh0j2x54qhznwqjnp452ju4078srmwp70"
    static let samplePeerPubkey = "2284fc7b932b5dbbdaa2185c76a4e17a2ef928d4a82e29b812986b454b957f8f"
    static let sampleThirdNpub = "npub1rtrxx9eyvag0ap3v73c4dvsqq5d2yxwe5d72qxrfpwe5svr96wuqed4p38"
    static let sampleThirdPubkey = "1f7f5f6d64e8de7184f4ad14a2fdbef674e7dc86d51a0d65704fbfdbb6c42cb7"
    static let sampleProfile = MyProfileState(
        name: "Paul Miller",
        about: "Building Marmot over Nostr.",
        pictureUrl: "https://blossom.nostr.pub/8dbc6f42ea8bf53f4af89af87eb0d9110fcaf4d263f7d2cb9f29d68f95f6f8ce"
    )
}
#endif
