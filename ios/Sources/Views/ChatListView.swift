import SwiftUI

struct ChatListView: View {
    let manager: AppManager
    @State private var showMyNpub = false

    var body: some View {
        List(manager.state.chatList, id: \.chatId) { chat in
            let row = VStack(alignment: .leading, spacing: 4) {
                Text(chat.peerName ?? chat.peerNpub)
                    .font(.headline)
                Text(chat.lastMessage ?? "No messages yet")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
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
}
