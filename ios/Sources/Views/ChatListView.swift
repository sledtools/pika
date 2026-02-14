import SwiftUI

struct ChatListView: View {
    let state: ChatListViewState
    let onLogout: @MainActor () -> Void
    let onOpenChat: @MainActor (String) -> Void
    let onNewChat: @MainActor () -> Void
    let onNewGroupChat: @MainActor () -> Void
    let nsecProvider: @MainActor () -> String?
    @State private var showMyNpub = false

    var body: some View {
        List(state.chats, id: \.chatId) { chat in
            let displayName = chatDisplayName(chat)
            let subtitle = chatSubtitle(chat)

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
                    Text(displayName)
                        .font(.headline)
                        .lineLimit(1)
                    if let subtitle {
                        Text(subtitle)
                            .font(.caption)
                            .foregroundStyle(.tertiary)
                            .lineLimit(1)
                    }
                    Text(chat.lastMessage ?? "No messages yet")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }

            Group {
                if chat.unreadCount > 0 {
                    row.badge(Int(chat.unreadCount))
                } else {
                    row
                }
            }
            .contentShape(Rectangle())
            .onTapGesture {
                onOpenChat(chat.chatId)
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
                            name: nil,
                            npub: npub,
                            pictureUrl: nil,
                            size: 28
                        )
                    }
                    .accessibilityLabel("My profile")
                    .accessibilityIdentifier(TestIds.chatListMyNpub)
                    .sheet(isPresented: $showMyNpub) {
                        MyNpubQrSheet(
                            npub: npub,
                            nsecProvider: nsecProvider,
                            onLogout: onLogout
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

    private func chatDisplayName(_ chat: ChatSummary) -> String {
        if chat.isGroup {
            return chat.groupName ?? "Group (\(chat.members.count + 1))"
        }
        return chat.members.first?.name ?? truncatedNpub(chat.members.first?.npub ?? "")
    }

    private func chatSubtitle(_ chat: ChatSummary) -> String? {
        if chat.isGroup {
            return "\(chat.members.count + 1) members"
        }
        if chat.members.first?.name != nil {
            return truncatedNpub(chat.members.first?.npub ?? "")
        }
        return nil
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

    private func truncatedNpub(_ npub: String) -> String {
        if npub.count <= 16 { return npub }
        return String(npub.prefix(12)) + "..."
    }
}

#if DEBUG
#Preview("Chat List - Empty") {
    NavigationStack {
        ChatListView(
            state: ChatListViewState(
                chats: PreviewAppState.chatListEmpty.chatList,
                myNpub: PreviewAppState.sampleNpub
            ),
            onLogout: {},
            onOpenChat: { _ in },
            onNewChat: {},
            onNewGroupChat: {},
            nsecProvider: { nil }
        )
    }
}

#Preview("Chat List - Populated") {
    NavigationStack {
        ChatListView(
            state: ChatListViewState(
                chats: PreviewAppState.chatListPopulated.chatList,
                myNpub: PreviewAppState.sampleNpub
            ),
            onLogout: {},
            onOpenChat: { _ in },
            onNewChat: {},
            onNewGroupChat: {},
            nsecProvider: { nil }
        )
    }
}

#Preview("Chat List - Long Names") {
    NavigationStack {
        ChatListView(
            state: ChatListViewState(
                chats: PreviewAppState.chatListLongNames.chatList,
                myNpub: PreviewAppState.sampleNpub
            ),
            onLogout: {},
            onOpenChat: { _ in },
            onNewChat: {},
            onNewGroupChat: {},
            nsecProvider: { nil }
        )
    }
}
#endif
