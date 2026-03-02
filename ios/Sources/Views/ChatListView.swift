import Foundation
import SwiftUI

struct ChatListView: View {
    let state: ChatListViewState
    let onLogout: @MainActor () -> Void
    let onOpenChat: @MainActor (String) -> Void
    let onArchiveChat: @MainActor (String) -> Void
    let onNewChat: @MainActor () -> Void
    let onNewGroupChat: @MainActor () -> Void
    let onRefreshProfile: @MainActor () -> Void
    let onSaveProfile: @MainActor (_ name: String, _ about: String) -> Void
    let onUploadProfilePhoto: @MainActor (_ data: Data, _ mimeType: String) -> Void
    let isDeveloperModeEnabledProvider: @MainActor () -> Bool
    let onEnableDeveloperMode: @MainActor () -> Void
    let onWipeLocalData: @MainActor () -> Void
    let nsecProvider: @MainActor () -> String?
    @State private var showMyNpub = false

    var body: some View {
        List(state.chats, id: \.chatId) { chat in
            let row = HStack(spacing: 12) {
                if chat.isGroup {
                    groupAvatar(chat)
                } else {
                    AvatarView(
                        name: chat.members.first?.name,
                        npub: chat.members.first?.npub ?? "",
                        pictureUrl: chat.members.first?.pictureUrl
                    )
                }

                VStack(alignment: .leading, spacing: 2) {
                    Text(chat.displayName)
                        .font(.headline)
                        .lineLimit(1)
                    if let subtitle = chat.subtitle {
                        Text(subtitle)
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                            .lineLimit(1)
                    }
                    Text(chat.lastMessagePreview)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())

            Button {
                onOpenChat(chat.chatId)
            } label: {
                if chat.unreadCount > 0 {
                    row.badge(Int(chat.unreadCount))
                } else {
                    row
                }
            }
            .tint(.primary)
            .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                Button(role: .destructive) {
                    onArchiveChat(chat.chatId)
                } label: {
                    Label("Archive", systemImage: "archivebox")
                }
                .tint(.orange)
            }
        }
        .navigationTitle("Chats")
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                if let npub = state.myNpub {
                    Button {
                        showMyNpub = true
                    } label: {
                        AvatarView(
                            name: state.myProfile.name.isEmpty ? nil : state.myProfile.name,
                            npub: npub,
                            pictureUrl: state.myProfile.pictureUrl,
                            size: 28
                        )
                    }
                    .accessibilityLabel("My profile")
                    .accessibilityIdentifier(TestIds.chatListMyNpub)
                    .sheet(isPresented: $showMyNpub) {
                        MyNpubQrSheet(
                            npub: npub,
                            profile: state.myProfile,
                            nsecProvider: nsecProvider,
                            onRefreshProfile: onRefreshProfile,
                            onSaveProfile: onSaveProfile,
                            onUploadPhoto: onUploadProfilePhoto,
                            onLogout: onLogout,
                            isDeveloperModeEnabledProvider: isDeveloperModeEnabledProvider,
                            onEnableDeveloperMode: onEnableDeveloperMode,
                            onWipeLocalData: onWipeLocalData
                        )
                    }
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button {
                        onNewChat()
                    } label: {
                        Label("New Chat", systemImage: "person")
                    }
                    Button {
                        onNewGroupChat()
                    } label: {
                        Label("New Group", systemImage: "person.3")
                    }
                } label: {
                    Image(systemName: "square.and.pencil")
                }
                .accessibilityLabel("New Chat")
                .accessibilityIdentifier(TestIds.chatListNewChat)
            }
        }
    }

    @ViewBuilder
    private func groupAvatar(_ chat: ChatSummary) -> some View {
        ZStack {
            Circle()
                .fill(Color.blue.opacity(0.15))
                .frame(width: 40, height: 40)
            Image(systemName: "person.3.fill")
                .font(.system(size: 16))
                .foregroundStyle(.blue)
        }
    }
}

#if DEBUG
#Preview("Chat List - Empty") {
    NavigationStack {
        ChatListView(
            state: ChatListViewState(
                chats: PreviewAppState.chatListEmpty.chatList,
                myNpub: PreviewAppState.sampleNpub,
                myProfile: PreviewAppState.chatListEmpty.myProfile
            ),
            onLogout: {},
            onOpenChat: { _ in },
            onArchiveChat: { _ in },
            onNewChat: {},
            onNewGroupChat: {},
            onRefreshProfile: {},
            onSaveProfile: { _, _ in },
            onUploadProfilePhoto: { _, _ in },
            isDeveloperModeEnabledProvider: { false },
            onEnableDeveloperMode: {},
            onWipeLocalData: {},
            nsecProvider: { nil }
        )
    }
}

#Preview("Chat List - Populated") {
    NavigationStack {
        ChatListView(
            state: ChatListViewState(
                chats: PreviewAppState.chatListPopulated.chatList,
                myNpub: PreviewAppState.sampleNpub,
                myProfile: PreviewAppState.chatListPopulated.myProfile
            ),
            onLogout: {},
            onOpenChat: { _ in },
            onArchiveChat: { _ in },
            onNewChat: {},
            onNewGroupChat: {},
            onRefreshProfile: {},
            onSaveProfile: { _, _ in },
            onUploadProfilePhoto: { _, _ in },
            isDeveloperModeEnabledProvider: { false },
            onEnableDeveloperMode: {},
            onWipeLocalData: {},
            nsecProvider: { nil }
        )
    }
}

#Preview("Chat List - Long Names") {
    NavigationStack {
        ChatListView(
            state: ChatListViewState(
                chats: PreviewAppState.chatListLongNames.chatList,
                myNpub: PreviewAppState.sampleNpub,
                myProfile: PreviewAppState.chatListLongNames.myProfile
            ),
            onLogout: {},
            onOpenChat: { _ in },
            onArchiveChat: { _ in },
            onNewChat: {},
            onNewGroupChat: {},
            onRefreshProfile: {},
            onSaveProfile: { _, _ in },
            onUploadProfilePhoto: { _, _ in },
            isDeveloperModeEnabledProvider: { false },
            onEnableDeveloperMode: {},
            onWipeLocalData: {},
            nsecProvider: { nil }
        )
    }
}
#endif
