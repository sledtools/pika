import SwiftUI

struct ContentView: View {
    @Bindable var manager: AppManager
    @State private var visibleToast: String? = nil
    @State private var navPath: [Screen] = []

    var body: some View {
        let appState = manager.state
        let router = appState.router
        Group {
            switch router.defaultScreen {
            case .login:
                LoginView(
                    state: loginState(from: appState),
                    onCreateAccount: { manager.dispatch(.createAccount) },
                    onLogin: { manager.login(nsec: $0) }
                )
            default:
                NavigationStack(path: $navPath) {
                    screenView(manager: manager, state: appState, screen: router.defaultScreen)
                        .navigationDestination(for: Screen.self) { screen in
                            screenView(manager: manager, state: appState, screen: screen)
                        }
                }
                .onAppear {
                    // Initial mount: seed the path from Rust.
                    navPath = manager.state.router.screenStack
                }
                // Drive native navigation from Rust's router, but avoid feeding those changes
                // back to Rust as "platform pops".
                .onChange(of: manager.state.router.screenStack) { _, new in
                    navPath = new
                }
                .onChange(of: navPath) { old, new in
                    // Ignore Rust-driven syncs.
                    if new == manager.state.router.screenStack { return }
                    // Only report platform-initiated pops (e.g. swipe-back).
                    if new.count < old.count {
                        manager.dispatch(.updateScreenStack(stack: new))
                    }
                }
            }
        }
        .overlay(alignment: .top) {
            if let toast = visibleToast {
                Text(toast)
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 10)
                    .background(.black.opacity(0.82), in: RoundedRectangle(cornerRadius: 10))
                    .padding(.horizontal, 24)
                    .padding(.top, 8)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .accessibilityIdentifier("pika_toast")
                    .onTapGesture { withAnimation { visibleToast = nil } }
                    .allowsHitTesting(true)
            }
        }
        .animation(.easeInOut(duration: 0.25), value: visibleToast)
        .onChange(of: manager.state.toast) { _, new in
            guard let message = new else { return }
            // Show the non-blocking overlay and immediately clear Rust state so it
            // doesn't re-show on state resync. The overlay manages its own lifetime.
            withAnimation { visibleToast = message }
            manager.dispatch(.clearToast)
            // Auto-dismiss after 3 seconds.
            let captured = message
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(3))
                withAnimation {
                    if visibleToast == captured { visibleToast = nil }
                }
            }
        }
    }
}

@ViewBuilder
private func screenView(manager: AppManager, state: AppState, screen: Screen) -> some View {
    switch screen {
    case .login:
        LoginView(
            state: loginState(from: state),
            onCreateAccount: { manager.dispatch(.createAccount) },
            onLogin: { manager.login(nsec: $0) }
        )
    case .chatList:
        ChatListView(
            state: chatListState(from: state),
            onLogout: { manager.logout() },
            onOpenChat: { manager.dispatch(.openChat(chatId: $0)) },
            onNewChat: { manager.dispatch(.pushScreen(screen: .newChat)) },
            onNewGroupChat: { manager.dispatch(.pushScreen(screen: .newGroupChat)) },
            onRefreshProfile: { manager.refreshMyProfile() },
            onSaveProfile: { name, about in
                manager.saveMyProfile(name: name, about: about)
            },
            onUploadProfilePhoto: { data, mimeType in
                manager.uploadMyProfileImage(data: data, mimeType: mimeType)
            },
            nsecProvider: { manager.getNsec() }
        )
    case .newChat:
        NewChatView(
            state: newChatState(from: state),
            onCreateChat: { manager.dispatch(.createChat(peerNpub: $0)) },
            onRefreshFollowList: { manager.dispatch(.refreshFollowList) }
        )
    case .newGroupChat:
        NewGroupChatView(
            state: newGroupChatState(from: state),
            onCreateGroup: { name, npubs in
                manager.dispatch(.createGroupChat(peerNpubs: npubs, groupName: name))
            },
            onRefreshFollowList: { manager.dispatch(.refreshFollowList) }
        )
    case .chat(let chatId):
        ChatView(
            chatId: chatId,
            state: chatScreenState(from: state),
            onSendMessage: { manager.dispatch(.sendMessage(chatId: chatId, content: $0)) },
            onGroupInfo: {
                manager.dispatch(.pushScreen(screen: .groupInfo(chatId: chatId)))
            }
        )
    case .groupInfo(let chatId):
        GroupInfoView(
            state: groupInfoState(from: state),
            onAddMembers: { npubs in
                manager.dispatch(.addGroupMembers(chatId: chatId, peerNpubs: npubs))
            },
            onRemoveMember: { pubkey in
                manager.dispatch(.removeGroupMembers(chatId: chatId, memberPubkeys: [pubkey]))
            },
            onLeaveGroup: {
                manager.dispatch(.leaveGroup(chatId: chatId))
            },
            onRenameGroup: { name in
                manager.dispatch(.renameGroup(chatId: chatId, name: name))
            }
        )
    }
}

private func loginState(from state: AppState) -> LoginViewState {
    LoginViewState(
        creatingAccount: state.busy.creatingAccount,
        loggingIn: state.busy.loggingIn
    )
}

private func chatListState(from state: AppState) -> ChatListViewState {
    ChatListViewState(
        chats: state.chatList,
        myNpub: myNpub(from: state),
        myProfile: state.myProfile
    )
}

private func newChatState(from state: AppState) -> NewChatViewState {
    NewChatViewState(
        isCreatingChat: state.busy.creatingChat,
        isFetchingFollowList: state.busy.fetchingFollowList,
        followList: state.followList
    )
}

private func newGroupChatState(from state: AppState) -> NewGroupChatViewState {
    NewGroupChatViewState(
        isCreatingChat: state.busy.creatingChat,
        isFetchingFollowList: state.busy.fetchingFollowList,
        followList: state.followList
    )
}

private func chatScreenState(from state: AppState) -> ChatScreenState {
    ChatScreenState(chat: state.currentChat)
}

private func groupInfoState(from state: AppState) -> GroupInfoViewState {
    GroupInfoViewState(chat: state.currentChat)
}

private func myNpub(from state: AppState) -> String? {
    switch state.auth {
    case .loggedIn(let npub, _):
        return npub
    default:
        return nil
    }
}

#if DEBUG
#Preview("Logged Out") {
    ContentView(manager: PreviewFactory.manager(PreviewAppState.loggedOut))
}

#Preview("Chat List") {
    ContentView(manager: PreviewFactory.manager(PreviewAppState.chatListPopulated))
}

#Preview("Chat List - Long Names") {
    ContentView(manager: PreviewFactory.manager(PreviewAppState.chatListLongNames))
}

#Preview("Toast") {
    ContentView(manager: PreviewFactory.manager(PreviewAppState.toastVisible))
}
#endif
