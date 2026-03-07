import Perception
import SwiftUI
import UserNotifications

@MainActor
struct ContentView: View {
    @Perception.Bindable var manager: AppManager
    @State private var visibleToast: String? = nil
    @State private var navPath: [Screen] = []
    @State private var isCallScreenPresented = false
    @State private var videoPipeline = VideoCallPipeline()
    @State private var pendingPeerProfileAction: PendingPeerProfileAction?

    var body: some View {
        WithPerceptionTracking {
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
                            onLogin: { manager.login(nsec: $0) },
                            onBunkerLogin: { manager.loginWithBunker(bunkerUri: $0) },
                            onNostrConnectLogin: { manager.loginWithNostrConnect() },
                            onResetNostrConnectPairing: { manager.resetNostrConnectPairing() }
                        )
                    default:
                        NavigationStack(path: $navPath) {
                            screenView(
                                manager: manager,
                                state: appState,
                                screen: router.defaultScreen,
                                onSetPendingPeerProfileAction: { pendingPeerProfileAction = $0 },
                                onOpenCallScreen: {
                                    isCallScreenPresented = true
                                }
                            )
                            .navigationDestination(for: Screen.self) { screen in
                                screenView(
                                    manager: manager,
                                    state: appState,
                                    screen: screen,
                                    onSetPendingPeerProfileAction: { pendingPeerProfileAction = $0 },
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
                        .onChangeCompat(of: manager.state.router.screenStack) { new in
                            navPath = new
                        }
                        .onChangeCompat(of: navPath, withOld: { old, new in
                            // Ignore Rust-driven syncs.
                            if new == manager.state.router.screenStack { return }
                            // Only report platform-initiated pops (e.g. swipe-back).
                            if new.count < old.count {
                                manager.dispatch(.updateScreenStack(stack: new))
                            }
                        })
                    }
                }
            }
            .overlay(alignment: .top) {
                VStack(spacing: 0) {
                    if appState.updateRequired {
                        updateBanner
                    }
                    toastOverlay
                }
            }
            .animation(.easeInOut(duration: 0.25), value: visibleToast)
            .onAppear {
                videoPipeline.configure(core: manager.core)
                if let call = manager.state.activeCall, call.shouldAutoPresentCallScreen {
                    isCallScreenPresented = true
                }
                videoPipeline.syncWithCallState(manager.state.activeCall)
                visibleToast = manager.state.toast
            }
            .onChangeCompat(of: manager.state.toast) { new in
                withAnimation { visibleToast = new }
            }
            .onChangeCompat(of: manager.state.currentChat?.chatId) { newChatId in
                AppDelegate.activeChatId = newChatId
                handlePendingPeerProfileAction()
            }
            .onChangeCompat(of: manager.state.activeCall, withOld: { old, new in
                videoPipeline.syncWithCallState(new)

                guard let new else {
                    isCallScreenPresented = false
                    // Clear call notifications when the call ends/is rejected.
                    if let chatId = old?.chatId {
                        clearDeliveredNotifications(forChatId: chatId)
                    }
                    return
                }

                guard new.shouldAutoPresentCallScreen else { return }
                let callChanged = old?.callId != new.callId
                let statusChanged = old?.status != new.status
                if callChanged || statusChanged {
                    isCallScreenPresented = true
                }
            })
            .fullScreenCover(isPresented: $isCallScreenPresented) {
                callScreenOverlay(state: manager.state)
                    .overlay(alignment: .top) {
                        toastOverlay
                    }
            }
        }
    }

    @ViewBuilder
    private var updateBanner: some View {
        Link(destination: URL(string: "https://apps.apple.com/app/id6741372509")!) {
            HStack {
                Image(systemName: "arrow.up.circle.fill")
                Text("A new version of Pika is available. Please update.")
                    .font(.subheadline.weight(.medium))
                Spacer()
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(Color.blue)
        }
    }

    @ViewBuilder
    private var toastOverlay: some View {
        if let toast = visibleToast {
            Button {
                manager.dispatch(.clearToast)
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
                onToggleCamera: {
                    manager.dispatch(.toggleCamera)
                },
                onFlipCamera: {
                    videoPipeline.switchCamera()
                },
                onStartAgain: {
                    manager.dispatch(.openChat(chatId: call.chatId))
                    if call.isVideoCall {
                        manager.dispatch(.startVideoCall(chatId: call.chatId))
                    } else {
                        manager.dispatch(.startCall(chatId: call.chatId))
                    }
                },
                onDismiss: {
                    isCallScreenPresented = false
                },
                remotePixelBuffer: videoPipeline.remotePixelBuffer,
                localCaptureSession: videoPipeline.localCaptureSession
            )
        }
    }

    private func handlePendingPeerProfileAction() {
        guard let pendingPeerProfileAction,
              let currentChat = manager.state.currentChat,
              !currentChat.isGroup,
              let peer = currentChat.members.first,
              peer.npub == pendingPeerProfileAction.npub else {
            return
        }

        switch pendingPeerProfileAction.kind {
        case .audio:
            manager.dispatch(.startCall(chatId: currentChat.chatId))
        case .video:
            manager.dispatch(.startVideoCall(chatId: currentChat.chatId))
        }

        self.pendingPeerProfileAction = nil
    }
}

@MainActor
@ViewBuilder
private func screenView(
    manager: AppManager,
    state: AppState,
    screen: Screen,
    onSetPendingPeerProfileAction: @escaping @MainActor (PendingPeerProfileAction?) -> Void,
    onOpenCallScreen: @escaping @MainActor () -> Void
) -> some View {
    switch screen {
    case .login:
        LoginView(
            state: loginState(from: state),
            onCreateAccount: { manager.dispatch(.createAccount) },
            onLogin: { manager.login(nsec: $0) },
            onBunkerLogin: { manager.loginWithBunker(bunkerUri: $0) },
            onNostrConnectLogin: { manager.loginWithNostrConnect() },
            onResetNostrConnectPairing: { manager.resetNostrConnectPairing() }
        )
    case .chatList:
        ChatListView(
            state: chatListState(from: state, manager: manager),
            onLogout: { manager.logout() },
            onOpenChat: { manager.dispatch(.openChat(chatId: $0)) },
            onArchiveChat: { manager.dispatch(.archiveChat(chatId: $0)) },
            onNewChat: { manager.dispatch(.pushScreen(screen: .newChat)) },
            onNewGroupChat: { manager.dispatch(.pushScreen(screen: .newGroupChat)) },
            onEnsureAgent: { manager.ensureAgent() },
            onRefreshProfile: { manager.refreshMyProfile() },
            onSaveProfile: { name, about in
                manager.saveMyProfile(name: name, about: about)
            },
            onUploadProfilePhoto: { data, mimeType in
                manager.uploadMyProfileImage(data: data, mimeType: mimeType)
            },
            isDeveloperModeEnabledProvider: { manager.isDeveloperModeEnabled },
            onEnableDeveloperMode: { manager.enableDeveloperMode() },
            onWipeProfileCache: { manager.wipeProfileCacheForDeveloperTools() },
            onWipeMediaCache: { manager.dispatch(.wipeMediaCache) },
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
            voiceRecording: state.voiceRecording,
            activeCall: state.activeCall,
            callEvents: state.callTimeline.filter { $0.chatId == chatId },
            onVoiceRecordingAction: { action in
                manager.dispatch(action)
            },
            onSendMessage: { message, replyToMessageId in
                manager.dispatch(
                    .sendMessage(
                        chatId: chatId,
                        content: message,
                        kind: nil,
                        replyToMessageId: replyToMessageId
                    )
                )
            },
            onStartCall: { manager.dispatch(.startCall(chatId: chatId)) },
            onStartVideoCall: { manager.dispatch(.startVideoCall(chatId: chatId)) },
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
            },
            onDownloadMedia: { chatId, messageId, hash in
                manager.dispatch(.downloadChatMedia(chatId: chatId, messageId: messageId, originalHashHex: hash))
            },
            onSendMedia: { chatId, data, mimeType, filename, caption in
                manager.dispatch(.sendChatMedia(
                    chatId: chatId,
                    dataBase64: data.base64EncodedString(),
                    mimeType: mimeType,
                    filename: filename,
                    caption: caption
                ))
            },
            onSendMediaBatch: { chatId, items, caption in
                let batchItems = items.map { item in
                    MediaBatchItem(
                        dataBase64: item.data.base64EncodedString(),
                        mimeType: item.mimeType,
                        filename: item.filename
                    )
                }
                manager.dispatch(.sendChatMediaBatch(
                    chatId: chatId,
                    items: batchItems,
                    caption: caption
                ))
            },
            onHypernoteAction: { chatId, actionName, messageId, form in
                manager.dispatch(.hypernoteAction(
                    chatId: chatId,
                    messageId: messageId,
                    actionName: actionName,
                    form: form
                ))
            },
            onSendPoll: { chatId, question, options in
                manager.dispatch(.sendHypernotePoll(
                    chatId: chatId,
                    question: question,
                    options: options
                ))
            },
            onLoadOlderMessages: { chatId, beforeMessageId, limit in
                manager.dispatch(.loadOlderMessages(
                    chatId: chatId,
                    beforeMessageId: beforeMessageId,
                    limit: limit
                ))
            },
            onRetryMessage: { chatId, messageId in
                manager.dispatch(.retryMessage(chatId: chatId, messageId: messageId))
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
                let directChatId = directMessageChatId(for: profile, in: state)
                PeerProfileSheet(
                    profile: profile,
                    onMessage: {
                        onSetPendingPeerProfileAction(nil)
                        if let directChatId {
                            manager.dispatch(.openChat(chatId: directChatId))
                        } else {
                            manager.dispatch(.createChat(peerNpub: profile.npub))
                        }
                        manager.dispatch(.closePeerProfile)
                    },
                    onStartCall: {
                        onSetPendingPeerProfileAction(nil)
                        if let directChatId {
                            manager.dispatch(.startCall(chatId: directChatId))
                        } else {
                            onSetPendingPeerProfileAction(.init(kind: .audio, npub: profile.npub))
                            manager.dispatch(.createChat(peerNpub: profile.npub))
                        }
                        manager.dispatch(.closePeerProfile)
                    },
                    onStartVideoCall: {
                        onSetPendingPeerProfileAction(nil)
                        if let directChatId {
                            manager.dispatch(.startVideoCall(chatId: directChatId))
                        } else {
                            onSetPendingPeerProfileAction(.init(kind: .video, npub: profile.npub))
                            manager.dispatch(.createChat(peerNpub: profile.npub))
                        }
                        manager.dispatch(.closePeerProfile)
                    },
                    onFollow: { manager.dispatch(.followUser(pubkey: profile.pubkey)) },
                    onUnfollow: { manager.dispatch(.unfollowUser(pubkey: profile.pubkey)) },
                    onOpenMediaGallery: {
                        manager.dispatch(.pushScreen(screen: .chatMedia(chatId: chatId)))
                    },
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
            },
            onSaveGroupProfile: { name, about in
                manager.dispatch(.saveGroupProfile(chatId: chatId, name: name, about: about))
            },
            onUploadGroupProfilePhoto: { data, mimeType in
                manager.dispatch(.uploadGroupProfileImage(
                    chatId: chatId,
                    imageBase64: data.base64EncodedString(),
                    mimeType: mimeType
                ))
            },
            onOpenMediaGallery: {
                manager.dispatch(.pushScreen(screen: .chatMedia(chatId: chatId)))
            }
        )
        .sheet(isPresented: Binding(
            get: { state.peerProfile != nil },
            set: { if !$0 { manager.dispatch(.closePeerProfile) } }
        )) {
            if let profile = state.peerProfile {
                let directChatId = directMessageChatId(for: profile, in: state)
                PeerProfileSheet(
                    profile: profile,
                    onMessage: {
                        onSetPendingPeerProfileAction(nil)
                        if let directChatId {
                            manager.dispatch(.openChat(chatId: directChatId))
                        } else {
                            manager.dispatch(.createChat(peerNpub: profile.npub))
                        }
                        manager.dispatch(.closePeerProfile)
                    },
                    onStartCall: {
                        onSetPendingPeerProfileAction(nil)
                        if let directChatId {
                            manager.dispatch(.startCall(chatId: directChatId))
                        } else {
                            onSetPendingPeerProfileAction(.init(kind: .audio, npub: profile.npub))
                            manager.dispatch(.createChat(peerNpub: profile.npub))
                        }
                        manager.dispatch(.closePeerProfile)
                    },
                    onStartVideoCall: {
                        onSetPendingPeerProfileAction(nil)
                        if let directChatId {
                            manager.dispatch(.startVideoCall(chatId: directChatId))
                        } else {
                            onSetPendingPeerProfileAction(.init(kind: .video, npub: profile.npub))
                            manager.dispatch(.createChat(peerNpub: profile.npub))
                        }
                        manager.dispatch(.closePeerProfile)
                    },
                    onFollow: { manager.dispatch(.followUser(pubkey: profile.pubkey)) },
                    onUnfollow: { manager.dispatch(.unfollowUser(pubkey: profile.pubkey)) },
                    onOpenMediaGallery: nil,
                    onClose: { manager.dispatch(.closePeerProfile) }
                )
            }
        }
    case .chatMedia(let chatId):
        ChatMediaGalleryView(
            chatId: chatId,
            items: state.mediaGallery?.items ?? []
        )
        .onAppear {
            manager.dispatch(.loadMediaGallery(chatId: chatId))
        }
        .onDisappear {
            manager.dispatch(.clearMediaGallery)
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
private func chatListState(from state: AppState, manager: AppManager) -> ChatListViewState {
    let myNpub = myNpub(from: state)
    return ChatListViewState(
        chats: state.chatList,
        myNpub: myNpub,
        myProfile: state.myProfile,
        agentButton: state.agentButton
    )
}

@MainActor
private func newChatState(from state: AppState) -> NewChatViewState {
    NewChatViewState(
        isCreatingChat: state.busy.creatingChat,
        isFetchingFollowList: state.busy.fetchingFollowList,
        followList: state.followList,
        myNpub: myNpub(from: state)
    )
}

@MainActor
private func newGroupChatState(from state: AppState) -> NewGroupChatViewState {
    NewGroupChatViewState(
        isCreatingChat: state.busy.creatingChat,
        isFetchingFollowList: state.busy.fetchingFollowList,
        followList: state.followList,
        myNpub: myNpub(from: state)
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

private struct PendingPeerProfileAction: Equatable {
    enum Kind: Equatable {
        case audio
        case video
    }

    let kind: Kind
    let npub: String
}

/// Remove delivered notifications that belong to the given chat.
func clearDeliveredNotifications(forChatId chatId: String) {
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
    case .loggedIn(let npub, _, _):
        return npub
    default:
        return nil
    }
}

@MainActor
private func directMessageChatId(for profile: PeerProfileState, in state: AppState) -> String? {
    let matchesProfile: ([MemberInfo]) -> Bool = { members in
        members.contains { $0.pubkey == profile.pubkey }
    }

    if let currentChat = state.currentChat,
       !currentChat.isGroup,
       matchesProfile(currentChat.members) {
        return currentChat.chatId
    }

    return state.chatList.first(where: { !$0.isGroup && matchesProfile($0.members) })?.chatId
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
