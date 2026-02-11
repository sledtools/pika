import SwiftUI

struct ChatView: View {
    let manager: AppManager
    let chatId: String
    @State private var messageText = ""

    var body: some View {
        if let chat = manager.state.currentChat, chat.chatId == chatId {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 8) {
                        ForEach(chat.messages, id: \.id) { msg in
                            MessageRow(message: msg)
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                }
                .modifier(FloatingInputBarModifier(content: { messageInputBar(chat: chat) }))
            }
            .navigationTitle(chat.peerName ?? chat.peerNpub)
            .navigationBarTitleDisplayMode(.inline)
        } else {
            VStack(spacing: 10) {
                ProgressView()
                Text("Loading chat…")
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func sendMessage(chat: ChatViewState) {
        let trimmed = messageText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        manager.dispatch(.sendMessage(chatId: chat.chatId, content: trimmed))
        messageText = ""
    }

    @ViewBuilder
    private func messageInputBar(chat: ChatViewState) -> some View {
        HStack(spacing: 10) {
            TextField("Message", text: $messageText)
                .onSubmit { sendMessage(chat: chat) }
                .accessibilityIdentifier(TestIds.chatMessageInput)

            Button(action: { sendMessage(chat: chat) }) {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.title2)
            }
            .disabled(messageText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            .accessibilityIdentifier(TestIds.chatSend)
        }
        .modifier(GlassInputModifier())
    }
}

private struct GlassInputModifier: ViewModifier {
    func body(content: Content) -> some View {
        if #available(iOS 26.0, *) {
            content
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .glassEffect(.regular.interactive(), in: .capsule)
                .padding(12)
        } else {
            content
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(.ultraThinMaterial, in: Capsule())
                .padding(12)
        }
    }
}

private struct FloatingInputBarModifier<Bar: View>: ViewModifier {
    @ViewBuilder var content: Bar

    func body(content view: Content) -> some View {
        if #available(iOS 26.0, *) {
            view.safeAreaBar(edge: .bottom) { content }
        } else {
            view.safeAreaInset(edge: .bottom) {
                VStack(spacing: 0) {
                    Divider()
                    content
                }
            }
        }
    }
}

private struct MessageRow: View {
    let message: ChatMessage

    var body: some View {
        HStack {
            if message.isMine { Spacer(minLength: 0) }
            VStack(alignment: message.isMine ? .trailing : .leading, spacing: 3) {
                Text(message.content)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(message.isMine ? Color.blue : Color.gray.opacity(0.2))
                    .foregroundStyle(message.isMine ? Color.white : Color.primary)
                    .clipShape(RoundedRectangle(cornerRadius: 16))

                if message.isMine {
                    Text(deliveryText(message.delivery))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            if !message.isMine { Spacer(minLength: 0) }
        }
    }

    private func deliveryText(_ d: MessageDeliveryState) -> String {
        switch d {
        case .pending: return "Pending"
        case .sent: return "Sent"
        case .failed(let reason): return "Failed: \(reason)"
        }
    }
}
