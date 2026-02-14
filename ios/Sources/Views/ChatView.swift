import SwiftUI
import MarkdownUI

struct ChatView: View {
    let chatId: String
    let state: ChatScreenState
    let onSendMessage: @MainActor (String) -> Void
    let onGroupInfo: (@MainActor () -> Void)?
    @State private var messageText = ""
    @State private var scrollPosition: String?
    @State private var isAtBottom = false
    @State private var showMentionPicker = false
    @State private var insertedMentions: [(display: String, npub: String)] = []

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
                            MessageRow(message: msg, showSender: chat.isGroup, onSendMessage: onSendMessage)
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
        var wire = trimmed
        for mention in insertedMentions {
            wire = wire.replacingOccurrences(of: mention.display, with: "nostr:\(mention.npub)")
        }
        onSendMessage(wire)
        messageText = ""
        insertedMentions = []
    }

    @ViewBuilder
    private func messageInputBar(chat: ChatViewState) -> some View {
        VStack(spacing: 0) {
            if showMentionPicker, chat.isGroup {
                MentionPickerPopup(members: chat.members) { member in
                    let displayTag = "@\(member.name ?? String(member.npub.prefix(8)))"
                    // Remove the trailing "@" that triggered the picker.
                    if messageText.hasSuffix("@") {
                        messageText.removeLast()
                    }
                    messageText += "\(displayTag) "
                    insertedMentions.append((display: displayTag, npub: member.npub))
                    showMentionPicker = false
                }
            }

            HStack(spacing: 10) {
                TextField("Message", text: $messageText)
                    .onChange(of: messageText) { _, newValue in
                        if chat.isGroup {
                            let triggered = newValue.hasSuffix("@") &&
                                (newValue.count == 1 || newValue.dropLast().hasSuffix(" "))
                            if triggered {
                                showMentionPicker = true
                            } else if showMentionPicker && !newValue.hasSuffix("@") {
                                showMentionPicker = false
                            }
                        }
                    }
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
}

private struct MentionPickerPopup: View {
    let members: [MemberInfo]
    let onSelect: (MemberInfo) -> Void

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                ForEach(members, id: \.pubkey) { member in
                    Button {
                        onSelect(member)
                    } label: {
                        HStack(spacing: 8) {
                            AvatarView(
                                name: member.name,
                                npub: member.npub,
                                pictureUrl: member.pictureUrl,
                                size: 28
                            )
                            Text(member.name ?? String(member.npub.prefix(12)))
                                .font(.subheadline)
                                .lineLimit(1)
                            Spacer()
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                    }
                    .foregroundStyle(.primary)
                    if member.pubkey != members.last?.pubkey {
                        Divider().padding(.leading, 48)
                    }
                }
            }
        }
        .frame(maxHeight: 180)
        .background(.ultraThinMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .padding(.horizontal, 12)
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

// MARK: - Pika-prompt model

private struct PikaPrompt: Decodable {
    let title: String
    let options: [String]
}

/// Parses message content into segments: plain markdown text and pika-* code blocks.
private enum MessageSegment: Identifiable {
    case markdown(String)
    case pikaPrompt(PikaPrompt)

    var id: String {
        switch self {
        case .markdown(let text): return "md-\(text.hashValue)"
        case .pikaPrompt(let prompt): return "prompt-\(prompt.title.hashValue)"
        }
    }
}

private func parseMessageSegments(_ content: String) -> [MessageSegment] {
    var segments: [MessageSegment] = []
    let pattern = /```pika-(\w+)\n([\s\S]*?)```/
    var remaining = content[...]

    while let match = remaining.firstMatch(of: pattern) {
        let before = String(remaining[remaining.startIndex..<match.range.lowerBound])
        if !before.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            segments.append(.markdown(before))
        }

        let blockType = String(match.output.1)
        let blockBody = String(match.output.2).trimmingCharacters(in: .whitespacesAndNewlines)

        if blockType == "prompt",
           let data = blockBody.data(using: .utf8),
           let prompt = try? JSONDecoder().decode(PikaPrompt.self, from: data) {
            segments.append(.pikaPrompt(prompt))
        } else {
            // Unknown pika block — render as markdown code block
            segments.append(.markdown("```\(blockType)\n\(blockBody)\n```"))
        }

        remaining = remaining[match.range.upperBound...]
    }

    let tail = String(remaining)
    if !tail.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        segments.append(.markdown(tail))
    }

    return segments
}

// MARK: - Message row

private struct MessageRow: View {
    let message: ChatMessage
    var showSender: Bool = false
    let onSendMessage: @MainActor (String) -> Void

    var body: some View {
        HStack {
            if message.isMine { Spacer(minLength: 0) }
            VStack(alignment: message.isMine ? .trailing : .leading, spacing: 3) {
                if showSender && !message.isMine {
                    Text(message.senderName ?? String(message.senderPubkey.prefix(8)))
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                }

                let segments = parseMessageSegments(message.displayContent)
                ForEach(segments) { segment in
                    switch segment {
                    case .markdown(let text):
                        Markdown(text)
                            .markdownTheme(message.isMine ? .pikaOutgoing : .pikaIncoming)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                            .background(message.isMine ? Color.blue : Color.gray.opacity(0.2))
                            .clipShape(RoundedRectangle(cornerRadius: 16))
                    case .pikaPrompt(let prompt):
                        PikaPromptView(prompt: prompt, onSelect: onSendMessage)
                    }
                }
                .contextMenu {
                    Button {
                        UIPasteboard.general.string = message.displayContent
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

// MARK: - Pika prompt view

private struct PikaPromptView: View {
    let prompt: PikaPrompt
    let onSelect: @MainActor (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(prompt.title)
                .font(.subheadline.weight(.semibold))
            ForEach(prompt.options, id: \.self) { option in
                Button {
                    onSelect(option)
                } label: {
                    Text(option)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .background(Color.blue.opacity(0.1))
                        .foregroundStyle(Color.blue)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                }
            }
        }
        .padding(12)
        .background(Color.gray.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: 16))
    }
}

// MARK: - Markdown themes

extension Theme {
    static let pikaOutgoing = Theme()
        .text { ForegroundColor(.white) }
        .link { ForegroundColor(.white.opacity(0.9)) }
        .strong { ForegroundColor(.white) }
        .code { ForegroundColor(.white.opacity(0.9)) }
        .codeBlock { configuration in
            configuration.label
                .padding(8)
                .background(Color.white.opacity(0.15))
                .clipShape(RoundedRectangle(cornerRadius: 8))
        }

    static let pikaIncoming = Theme()
        .text { ForegroundColor(.primary) }
        .codeBlock { configuration in
            configuration.label
                .padding(8)
                .background(Color.gray.opacity(0.15))
                .clipShape(RoundedRectangle(cornerRadius: 8))
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
