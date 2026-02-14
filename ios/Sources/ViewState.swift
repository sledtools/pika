import Foundation

struct LoginViewState: Equatable {
    let creatingAccount: Bool
    let loggingIn: Bool
}

struct ChatListViewState: Equatable {
    let chats: [ChatSummary]
    let myNpub: String?
}

struct NewChatViewState: Equatable {
    let isCreatingChat: Bool
}

struct NewGroupChatViewState: Equatable {
    let isCreatingChat: Bool
}

struct ChatScreenState: Equatable {
    let chat: ChatViewState?
}

struct GroupInfoViewState: Equatable {
    let chat: ChatViewState?
}
