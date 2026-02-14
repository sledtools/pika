import Foundation
import SwiftUI

struct ChatView: View {
    let chatId: String
    let state: ChatScreenState
    let onSendMessage: @MainActor (String) -> Void
    let onGroupInfo: (@MainActor () -> Void)?
    @State private var messageText = ""
    @State private var scrollPosition: String?
    @State private var isAtBottom = false

    private let scrollButtonBottomPadding: CGFloat = 12

    init(chatId: String, state: ChatScreenState, onSendMessage: @escaping @MainActor (String) -> Void, onGroupInfo: (@MainActor () -> Void)? = nil) {
        self.chatId = chatId
        self.state = state
        self.onSendMessage = onSendMessage
        self.onGroupInfo = onGroupInfo
    }

    var body: some View {
        if let chat = state.chat, chat.chatId == chatId {
            ScrollView {
                VStack(spacing: 0) {
                    LazyVStack(spacing: 8) {
                        ForEach(chat.messages, id: \.id) { msg in
                            MessageRow(message: msg, showSender: chat.isGroup)
                                .id(msg.id)
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                }
                .scrollTargetLayout()
            }
            .scrollPosition(id: $scrollPosition, anchor: .bottom)
            .onChange(of: scrollPosition) { _, newPosition in
                guard let bottomId = chat.messages.last?.id else {
                    isAtBottom = true
                    return
                }
                isAtBottom = newPosition == bottomId
            }
            .overlay(alignment: .bottomTrailing) {
                if let bottomId = chat.messages.last?.id, !isAtBottom {
                    Button {
                        withAnimation(.easeOut(duration: 0.2)) {
                            scrollPosition = bottomId
                        }
                    } label: {
                        Image(systemName: "arrow.down")
                            .font(.footnote.weight(.semibold))
                            .padding(10)
                    }
                    .foregroundStyle(.primary)
                    .background(.ultraThinMaterial, in: Circle())
                    .overlay(Circle().strokeBorder(.quaternary, lineWidth: 0.5))
                    .padding(.trailing, 16)
                    .padding(.bottom, scrollButtonBottomPadding)
                    .accessibilityLabel("Scroll to bottom")
                }
            }
            .modifier(FloatingInputBarModifier(content: { messageInputBar(chat: chat) }))
            .navigationTitle(chatTitle(chat))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if chat.isGroup {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button {
                            onGroupInfo?()
                        } label: {
                            Image(systemName: "info.circle")
                        }
                        .accessibilityIdentifier(TestIds.chatGroupInfo)
                    }
                }
            }
        } else {
            VStack(spacing: 10) {
                ProgressView()
                Text("Loading chat...")
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func chatTitle(_ chat: ChatViewState) -> String {
        if chat.isGroup {
            return chat.groupName ?? "Group"
        }
        return chat.members.first?.name ?? chat.members.first?.npub ?? ""
    }

    private func sendMessage() {
        let trimmed = messageText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        onSendMessage(trimmed)
        messageText = ""
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

private struct MessageRow: View {
    let message: ChatMessage
    var showSender: Bool = false

    var body: some View {
        HStack {
            if message.isMine { Spacer(minLength: 0) }
            VStack(alignment: message.isMine ? .trailing : .leading, spacing: 3) {
                if showSender && !message.isMine {
                    Text(message.senderName ?? String(message.senderPubkey.prefix(8)))
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                }

                MarkdownMessageContent(message: message, isMine: message.isMine)
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

private struct MarkdownAstDocument: Decodable {
    let type: String
    let children: [MarkdownAstNode]

    static func decode(_ json: String?) -> MarkdownAstDocument? {
        guard let json, !json.isEmpty else {
            return nil
        }
        return try? JSONDecoder().decode(MarkdownAstDocument.self, from: Data(json.utf8))
    }
}

private struct MarkdownAstNode: Decodable {
    let type: String
    let value: String?
    let children: [MarkdownAstNode]?
}

private struct MarkdownMessageContent: View {
    let message: ChatMessage
    let isMine: Bool

    var body: some View {
        if let doc = MarkdownAstDocument.decode(message.markdownAstJson), !doc.children.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(doc.children.enumerated()), id: \.offset) { _, node in
                    blockView(node)
                }
            }
        } else {
            Text(message.content)
        }
    }

    @ViewBuilder
    private func blockView(_ node: MarkdownAstNode) -> some View {
        switch node.type {
        case "paragraph":
            inlineText(nodes: node.children ?? [])
        case "code_block":
            codeBlock(node.value ?? "")
        case "text", "strong":
            inlineText(nodes: [node])
        default:
            if let children = node.children, !children.isEmpty {
                inlineText(nodes: children)
            } else if let value = node.value, !value.isEmpty {
                Text(value)
            }
        }
    }

    private func inlineText(nodes: [MarkdownAstNode]) -> Text {
        var rendered = Text("")
        for index in nodes.indices {
            let node = nodes[index]
            if index > 0, shouldInsertLineBreak(before: node, previous: nodes[index - 1]) {
                rendered = rendered + Text("\n")
            }
            rendered = rendered + inlineText(node)
            if index + 1 < nodes.count, shouldInsertLineBreak(after: node, next: nodes[index + 1]) {
                rendered = rendered + Text("\n")
            }
        }
        return rendered
    }

    private func shouldInsertLineBreak(before node: MarkdownAstNode, previous: MarkdownAstNode) -> Bool {
        if node.type == "hard_break" || previous.type == "hard_break" {
            return false
        }
        guard isLabelStrongNode(node) else {
            return false
        }
        let previousText = flattenedText(previous)
        guard let last = previousText.last else {
            return false
        }
        return !last.isWhitespace
    }

    private func shouldInsertLineBreak(after node: MarkdownAstNode, next: MarkdownAstNode) -> Bool {
        if node.type == "hard_break" || next.type == "hard_break" {
            return false
        }
        guard isLabelStrongNode(node) else {
            return false
        }
        let nextText = flattenedText(next)
        guard let first = nextText.first else {
            return false
        }
        return !first.isWhitespace
    }

    private func isLabelStrongNode(_ node: MarkdownAstNode) -> Bool {
        guard node.type == "strong" else {
            return false
        }
        let label = flattenedText(node).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !label.isEmpty else {
            return false
        }
        guard label.hasSuffix(":") else {
            return false
        }
        return label.count <= 64
    }

    private func flattenedText(_ node: MarkdownAstNode) -> String {
        switch node.type {
        case "text":
            return node.value ?? ""
        case "hard_break":
            return "\n"
        default:
            if let value = node.value {
                return value
            }
            if let children = node.children {
                return children.map(flattenedText).joined()
            }
            return ""
        }
    }

    private func inlineText(_ node: MarkdownAstNode) -> Text {
        switch node.type {
        case "text":
            return Text(node.value ?? "")
        case "strong":
            return inlineText(nodes: node.children ?? []).bold()
        case "hard_break":
            return Text("\n")
        default:
            if let children = node.children, !children.isEmpty {
                return inlineText(nodes: children)
            }
            return Text(node.value ?? "")
        }
    }

    @ViewBuilder
    private func codeBlock(_ value: String) -> some View {
        Text(value)
            .font(.system(.callout, design: .monospaced))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(
                isMine ? Color.white.opacity(0.2) : Color.black.opacity(0.08),
                in: RoundedRectangle(cornerRadius: 8)
            )
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
