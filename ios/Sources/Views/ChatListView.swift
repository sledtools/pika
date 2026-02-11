import SwiftUI

struct ChatListView: View {
    let manager: AppManager
    @State private var showMyNpub = false

    var body: some View {
        List(manager.state.chatList, id: \.chatId) { chat in
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
                manager.dispatch(.openChat(chatId: chat.chatId))
            }
        }
        .navigationTitle("Chats")
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Logout") { manager.logout() }
                    .accessibilityIdentifier(TestIds.chatListLogout)
            }
            ToolbarItem(placement: .topBarTrailing) {
                if let npub = myNpub() {
                    Button {
                        showMyNpub = true
                    } label: {
                        Image(systemName: "person.circle")
                    }
                    .accessibilityLabel("My npub")
                    .accessibilityIdentifier(TestIds.chatListMyNpub)
                    .sheet(isPresented: $showMyNpub) {
                        MyNpubQrSheet(npub: npub)
                    }
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    manager.dispatch(.pushScreen(screen: .newChat))
                } label: {
                    Image(systemName: "square.and.pencil")
                }
                .accessibilityLabel("New Chat")
                .accessibilityIdentifier(TestIds.chatListNewChat)
            }
        }
    }

    private func myNpub() -> String? {
        switch manager.state.auth {
        case .loggedIn(let npub, _):
            return npub
        default:
            return nil
        }
    }

    private func truncatedNpub(_ npub: String) -> String {
        if npub.count <= 16 { return npub }
        return String(npub.prefix(12)) + "..."
    }
}
