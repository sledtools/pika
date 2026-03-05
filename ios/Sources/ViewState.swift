import Foundation

struct LoginViewState: Equatable {
    let creatingAccount: Bool
    let loggingIn: Bool
}

struct ChatListViewState: Equatable {
    let chats: [ChatSummary]
    let myNpub: String?
    let myProfile: MyProfileState
    let dogfoodAgentButton: DogfoodAgentButtonState?
}

struct DogfoodAgentButtonState: Equatable {
    let title: String
    let isBusy: Bool
}

struct NewChatViewState: Equatable {
    let isCreatingChat: Bool
    let isFetchingFollowList: Bool
    let followList: [FollowListEntry]
    let myNpub: String?
}

struct NewGroupChatViewState: Equatable {
    let isCreatingChat: Bool
    let isFetchingFollowList: Bool
    let followList: [FollowListEntry]
    let myNpub: String?
}

struct ChatScreenState: Equatable {
    let chat: ChatViewState?
}

struct GroupInfoViewState: Equatable {
    let chat: ChatViewState?
}