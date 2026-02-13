import SwiftUI

struct ChatListView: View {
    let state: ChatListViewState
    let onLogout: @MainActor () -> Void
    let onOpenChat: @MainActor (String) -> Void
    let onNewChat: @MainActor () -> Void
    let nsecProvider: @MainActor () -> String?
    @State private var showMyNpub = false

    var body: some View {
        List(state.chats, id: \.chatId) { chat in
            let displayName = chat.peerName ?? truncatedNpub(chat.peerNpub)
            let subtitle = chat.peerName != nil ? truncatedNpub(chat.peerNpub) : nil

            let row = HStack(spacing: 12) {
                AvatarView(
                    name: chat.peerName,
                    npub: chat.peerNpub,
                    pictureUrl: chat.peerPictureUrl
                )

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
                Button("Logout") { onLogout() }
                    .accessibilityIdentifier(TestIds.chatListLogout)
            }
            ToolbarItem(placement: .topBarTrailing) {
                if let npub = state.myNpub {
                    Button {
                        showMyNpub = true
                    } label: {
                        Image(systemName: "person.circle")
                    }
                    .accessibilityLabel("My npub")
                    .accessibilityIdentifier(TestIds.chatListMyNpub)
                    .sheet(isPresented: $showMyNpub) {
                        MyNpubQrSheet(npub: npub, nsecProvider: nsecProvider)
                    }
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    onNewChat()
                } label: {
                    Image(systemName: "square.and.pencil")
                }
                .accessibilityLabel("New Chat")
                .accessibilityIdentifier(TestIds.chatListNewChat)
            }
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
            nsecProvider: { nil }
        )
    }
}
#endif
