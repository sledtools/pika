import Foundation

struct LoginViewState: Equatable {
    let creatingAccount: Bool
    let loggingIn: Bool
}

struct ChatListViewState: Equatable {
    let chats: [ChatSummary]
    let myNpub: String?
    let myProfile: MyProfileState
    let timezoneDisplay: TimezoneDisplay
}

struct NewChatViewState: Equatable {
    let isCreatingChat: Bool
    let isFetchingFollowList: Bool
    let followList: [FollowListEntry]
}

struct NewGroupChatViewState: Equatable {
    let isCreatingChat: Bool
    let isFetchingFollowList: Bool
    let followList: [FollowListEntry]
}

struct ChatScreenState: Equatable {
    let chat: ChatViewState?
    let timezoneDisplay: TimezoneDisplay
}

struct GroupInfoViewState: Equatable {
    let chat: ChatViewState?
}
