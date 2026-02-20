import SwiftUI
import UserNotifications

@MainActor
struct ContentView: View {
    @Bindable var manager: AppManager
    @State private var visibleToast: String? = nil
    @State private var navPath: [Screen] = []
    @State private var isCallScreenPresented = false

    var body: some View {
        let appState = manager.state
        let router = appState.router

        Group {
            if manager.isRestoringSession {
                LoadingView()
            } else {
                switch router.defaultScreen {
                case .login:
                    LoginView(
                        state: loginState(from: appState),
                        onCreateAccount: { manager.dispatch(.createAccount) },
                        onLogin: { manager.login(nsec: $0) }
                    )
                default:
                    NavigationStack(path: $navPath) {
                        screenView(
                            manager: manager,
                            state: appState,
                            screen: router.defaultScreen,
                            onOpenCallScreen: {
                                isCallScreenPresented = true
                            }
                        )
                        .navigationDestination(for: Screen.self) { screen in
                            screenView(
                                manager: manager,
                                state: appState,
                                screen: screen,
                                onOpenCallScreen: {
                                    isCallScreenPresented = true
                                }
                            )
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
        }
        .overlay(alignment: .top) {
            toastOverlay
        }
        .animation(.easeInOut(duration: 0.25), value: visibleToast)
        .onAppear {
            if let call = manager.state.activeCall, call.status.shouldAutoPresentCallScreen {
                isCallScreenPresented = true
            }
        }
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
        .onChange(of: manager.state.activeCall) { old, new in
            guard let new else {
                isCallScreenPresented = false
                return
            }

            if !new.status.isLive {
                isCallScreenPresented = false
                return
            }

            guard new.status.shouldAutoPresentCallScreen else { return }
            let callChanged = old?.callId != new.callId
            let statusChanged = old?.status != new.status
            if callChanged || statusChanged {
                isCallScreenPresented = true
            }
        }
        .fullScreenCover(isPresented: $isCallScreenPresented) {
            callScreenOverlay(state: manager.state)
        }
    }

    @ViewBuilder
    private var toastOverlay: some View {
        if let toast = visibleToast {
            Button {
                withAnimation {
                    visibleToast = nil
                }
            } label: {
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
                    .allowsHitTesting(true)
            }
            .buttonStyle(.plain)
        }
    }

    @ViewBuilder
    private func callScreenOverlay(state: AppState) -> some View {
        if let call = state.activeCall {
            CallScreenView(
                call: call,
                peerName: callPeerDisplayName(for: call, in: state),
                onAcceptCall: {
                    manager.dispatch(.openChat(chatId: call.chatId))
                    manager.dispatch(.acceptCall(chatId: call.chatId))
                },
                onRejectCall: {
                    manager.dispatch(.rejectCall(chatId: call.chatId))
                },
                onEndCall: {
                    manager.dispatch(.endCall)
                },
                onToggleMute: {
                    manager.dispatch(.toggleMute)
                },
                onStartAgain: {
                    manager.dispatch(.openChat(chatId: call.chatId))
                    manager.dispatch(.startCall(chatId: call.chatId))
                },
                onDismiss: {
                    isCallScreenPresented = false
                }
            )
        }
    }
}

@MainActor
@ViewBuilder
private func screenView(
    manager: AppManager,
    state: AppState,
    screen: Screen,
    onOpenCallScreen: @escaping @MainActor () -> Void
) -> some View {
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
            onArchiveChat: { manager.dispatch(.archiveChat(chatId: $0)) },
            onNewChat: { manager.dispatch(.pushScreen(screen: .newChat)) },
            onNewGroupChat: { manager.dispatch(.pushScreen(screen: .newGroupChat)) },
            onRefreshProfile: { manager.refreshMyProfile() },
            onSaveProfile: { name, about in
                manager.saveMyProfile(name: name, about: about)
            },
            onUploadProfilePhoto: { data, mimeType in
                manager.uploadMyProfileImage(data: data, mimeType: mimeType)
            },
            isDeveloperModeEnabledProvider: { manager.isDeveloperModeEnabled },
            onEnableDeveloperMode: { manager.enableDeveloperMode() },
            onWipeLocalData: { manager.wipeLocalDataForDeveloperTools() },
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
            activeCall: state.activeCall,
            callEvents: state.callTimeline.filter { $0.chatId == chatId },
            onSendMessage: { manager.dispatch(.sendMessage(chatId: chatId, content: $0, kind: nil)) },
            onStartCall: { manager.dispatch(.startCall(chatId: chatId)) },
            onOpenCallScreen: {
                onOpenCallScreen()
            },
            onGroupInfo: {
                manager.dispatch(.pushScreen(screen: .groupInfo(chatId: chatId)))
            },
            onTapSender: { pubkey in
                manager.dispatch(.openPeerProfile(pubkey: pubkey))
            },
            onReact: { messageId, emoji in
                manager.dispatch(.reactToMessage(chatId: chatId, messageId: messageId, emoji: emoji))
            },
            onTypingStarted: {
                manager.dispatch(.typingStarted(chatId: chatId))
            }
        )
        .onAppear {
            clearDeliveredNotifications(forChatId: chatId)
        }
        .onReceive(NotificationCenter.default.publisher(for: UIApplication.willEnterForegroundNotification)) { _ in
            clearDeliveredNotifications(forChatId: chatId)
        }
        .sheet(isPresented: Binding(
            get: { state.peerProfile != nil },
            set: { if !$0 { manager.dispatch(.closePeerProfile) } }
        )) {
            if let profile = state.peerProfile {
                PeerProfileSheet(
                    profile: profile,
                    onFollow: { manager.dispatch(.followUser(pubkey: profile.pubkey)) },
                    onUnfollow: { manager.dispatch(.unfollowUser(pubkey: profile.pubkey)) },
                    onClose: { manager.dispatch(.closePeerProfile) }
                )
            }
        }
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
            },
            onTapMember: { pubkey in
                manager.dispatch(.openPeerProfile(pubkey: pubkey))
            }
        )
        .sheet(isPresented: Binding(
            get: { state.peerProfile != nil },
            set: { if !$0 { manager.dispatch(.closePeerProfile) } }
        )) {
            if let profile = state.peerProfile {
                PeerProfileSheet(
                    profile: profile,
                    onFollow: { manager.dispatch(.followUser(pubkey: profile.pubkey)) },
                    onUnfollow: { manager.dispatch(.unfollowUser(pubkey: profile.pubkey)) },
                    onClose: { manager.dispatch(.closePeerProfile) }
                )
            }
        }
    }
}

@MainActor
private func loginState(from state: AppState) -> LoginViewState {
    LoginViewState(
        creatingAccount: state.busy.creatingAccount,
        loggingIn: state.busy.loggingIn
    )
}

@MainActor
private func chatListState(from state: AppState) -> ChatListViewState {
    ChatListViewState(
        chats: state.chatList,
        myNpub: myNpub(from: state),
        myProfile: state.myProfile
    )
}

@MainActor
private func newChatState(from state: AppState) -> NewChatViewState {
    NewChatViewState(
        isCreatingChat: state.busy.creatingChat,
        isFetchingFollowList: state.busy.fetchingFollowList,
        followList: state.followList
    )
}

@MainActor
private func newGroupChatState(from state: AppState) -> NewGroupChatViewState {
    NewGroupChatViewState(
        isCreatingChat: state.busy.creatingChat,
        isFetchingFollowList: state.busy.fetchingFollowList,
        followList: state.followList
    )
}

@MainActor
private func chatScreenState(from state: AppState) -> ChatScreenState {
    ChatScreenState(chat: state.currentChat)
}

@MainActor
private func groupInfoState(from state: AppState) -> GroupInfoViewState {
    GroupInfoViewState(chat: state.currentChat)
}

/// Remove delivered notifications that belong to the given chat.
private func clearDeliveredNotifications(forChatId chatId: String) {
    let center = UNUserNotificationCenter.current()
    center.getDeliveredNotifications { notifications in
        let ids = notifications
            .filter { $0.request.content.threadIdentifier == chatId }
            .map { $0.request.identifier }
        if !ids.isEmpty {
            center.removeDeliveredNotifications(withIdentifiers: ids)
        }
    }
}

@MainActor
private func myNpub(from state: AppState) -> String? {
    switch state.auth {
    case .loggedIn(let npub, _):
        return npub
    default:
        return nil
    }
}

@MainActor
private func callPeerDisplayName(for call: CallState, in state: AppState) -> String {
    if let currentChat = state.currentChat, currentChat.chatId == call.chatId {
        if currentChat.isGroup {
            return currentChat.groupName ?? "Group"
        }
        if let peer = currentChat.members.first {
            return peer.name ?? shortenedNpub(peer.npub)
        }
    }

    if let summary = state.chatList.first(where: { $0.chatId == call.chatId }) {
        if summary.isGroup {
            return summary.groupName ?? "Group"
        }
        if let peer = summary.members.first {
            return peer.name ?? shortenedNpub(peer.npub)
        }
    }

    return shortenedNpub(call.peerNpub)
}

@MainActor
private func shortenedNpub(_ npub: String) -> String {
    guard npub.count > 16 else { return npub }
    return "\(npub.prefix(8))...\(npub.suffix(4))"
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
