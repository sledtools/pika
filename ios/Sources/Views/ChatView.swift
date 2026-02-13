import SwiftUI

struct ChatView: View {
    let chatId: String
    let state: ChatScreenState
    let onSendMessage: @MainActor (String) -> Void
    @State private var messageText = ""
    @State private var scrollViewHeight: CGFloat = 0
    @State private var bottomMarkerMaxY: CGFloat = .infinity
    @State private var isAtBottom = true

    private static let bottomAnchorId = "chat-bottom-anchor"

    var body: some View {
        if let chat = state.chat, chat.chatId == chatId {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 8) {
                        ForEach(chat.messages, id: \.id) { msg in
                            MessageRow(message: msg)
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)

                    BottomAnchor()
                        .id(Self.bottomAnchorId)
                }
                .coordinateSpace(name: "chat-scroll")
                .background(
                    GeometryReader { geo in
                        Color.clear.preference(key: ScrollViewHeightKey.self, value: geo.size.height)
                    }
                )
                .onPreferenceChange(ScrollViewHeightKey.self) { newHeight in
                    scrollViewHeight = newHeight
                    updateIsAtBottom()
                }
                .onPreferenceChange(BottomMarkerKey.self) { newMaxY in
                    bottomMarkerMaxY = newMaxY
                    updateIsAtBottom()
                }
                .onChange(of: chat.messages.count) { _, _ in
                    if isAtBottom {
                        scrollToBottom(proxy, animated: true)
                    }
                }
                .task(id: chat.chatId) {
                    isAtBottom = true
                    await Task.yield()
                    scrollToBottom(proxy, animated: false)
                }
                .overlay(alignment: .bottomTrailing) {
                    if !isAtBottom {
                        Button {
                            scrollToBottom(proxy, animated: true)
                        } label: {
                            Image(systemName: "arrow.down")
                                .font(.footnote.weight(.semibold))
                                .padding(10)
                        }
                        .foregroundStyle(.primary)
                        .background(.ultraThinMaterial, in: Circle())
                        .overlay(Circle().strokeBorder(.quaternary, lineWidth: 0.5))
                        .padding(.trailing, 16)
                        .padding(.bottom, 76)
                        .accessibilityLabel("Scroll to bottom")
                    }
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

    private func sendMessage() {
        let trimmed = messageText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        onSendMessage(trimmed)
        messageText = ""
    }

    private func updateIsAtBottom() {
        guard scrollViewHeight > 0 else { return }
        let threshold: CGFloat = 40
        isAtBottom = bottomMarkerMaxY <= scrollViewHeight + threshold
    }

    private func scrollToBottom(_ proxy: ScrollViewProxy, animated: Bool) {
        let action = {
            proxy.scrollTo(Self.bottomAnchorId, anchor: .bottom)
        }
        if animated {
            withAnimation(.easeOut(duration: 0.2)) {
                action()
            }
        } else {
            action()
        }
    }

    @ViewBuilder
    private func messageInputBar(chat: ChatViewState) -> some View {
        HStack(spacing: 10) {
            TextField("Message", text: $messageText)
                .onSubmit { sendMessage() }
                .accessibilityIdentifier(TestIds.chatMessageInput)

            Button(action: { sendMessage() }) {
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
        content
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(.ultraThinMaterial, in: Capsule())
            .padding(12)
    }
}

private struct FloatingInputBarModifier<Bar: View>: ViewModifier {
    @ViewBuilder var content: Bar

    func body(content view: Content) -> some View {
        view.safeAreaInset(edge: .bottom) {
            VStack(spacing: 0) {
                Divider()
                content
            }
        }
    }
}

private struct BottomAnchor: View {
    var body: some View {
        GeometryReader { geo in
            Color.clear
                .preference(
                    key: BottomMarkerKey.self,
                    value: geo.frame(in: .named("chat-scroll")).maxY
                )
        }
        .frame(height: 1)
    }
}

private struct ScrollViewHeightKey: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

private struct BottomMarkerKey: PreferenceKey {
    static var defaultValue: CGFloat = .infinity
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
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
                    .contextMenu {
                        Button {
                            UIPasteboard.general.string = message.content
                        } label: {
                            Label("Copy", systemImage: "doc.on.doc")
                        }
                    }

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

#if DEBUG
#Preview("Chat") {
    NavigationStack {
        ChatView(
            chatId: "chat-1",
            state: ChatScreenState(chat: PreviewAppState.chatDetail.currentChat),
            onSendMessage: { _ in }
        )
    }
}

#Preview("Chat - Failed") {
    NavigationStack {
        ChatView(
            chatId: "chat-1",
            state: ChatScreenState(chat: PreviewAppState.chatDetailFailed.currentChat),
            onSendMessage: { _ in }
        )
    }
}

#Preview("Chat - Empty") {
    NavigationStack {
        ChatView(
            chatId: "chat-empty",
            state: ChatScreenState(chat: PreviewAppState.chatDetailEmpty.currentChat),
            onSendMessage: { _ in }
        )
    }
}

#Preview("Chat - Long Thread") {
    NavigationStack {
        ChatView(
            chatId: "chat-long",
            state: ChatScreenState(chat: PreviewAppState.chatDetailLongThread.currentChat),
            onSendMessage: { _ in }
        )
    }
}
#endif
