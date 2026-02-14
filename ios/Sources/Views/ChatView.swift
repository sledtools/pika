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
                        ForEach(groupedMessages(chat)) { group in
                            MessageGroupRow(group: group, showSender: chat.isGroup, onSendMessage: onSendMessage)
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

    private func groupedMessages(_ chat: ChatViewState) -> [GroupedChatMessage] {
        let membersByPubkey = Dictionary(uniqueKeysWithValues: chat.members.map { ($0.pubkey, $0) })
        var groups: [GroupedChatMessage] = []

        for message in chat.messages {
            if let lastIndex = groups.indices.last,
               groups[lastIndex].senderPubkey == message.senderPubkey,
               groups[lastIndex].isMine == message.isMine {
                groups[lastIndex].messages.append(message)
                continue
            }

            let member = membersByPubkey[message.senderPubkey]
            groups.append(
                GroupedChatMessage(
                    senderPubkey: message.senderPubkey,
                    senderName: message.senderName ?? member?.name,
                    senderNpub: member?.npub ?? message.senderPubkey,
                    senderPictureUrl: member?.pictureUrl,
                    isMine: message.isMine,
                    messages: [message]
                )
            )
        }

        return groups
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
                TextEditor(text: $messageText)
                    .frame(minHeight: 36, maxHeight: 150)
                    .fixedSize(horizontal: false, vertical: true)
                    .scrollContentBackground(.hidden)
                    .onKeyPress(.return, phases: .down) { keyPress in
                        if keyPress.modifiers.contains(.shift) {
                            return .ignored
                        }
                        sendMessage()
                        return .handled
                    }
                    .overlay(alignment: .topLeading) {
                        if messageText.isEmpty {
                            Text("Message")
                                .foregroundStyle(.tertiary)
                                .padding(.leading, 5)
                                .padding(.top, 8)
                                .allowsHitTesting(false)
                        }
                    }
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
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 20))
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

// MARK: - Message grouping

private struct GroupedChatMessage: Identifiable {
    let senderPubkey: String
    let senderName: String?
    let senderNpub: String
    let senderPictureUrl: String?
    let isMine: Bool
    var messages: [ChatMessage]

    var id: String { messages.first?.id ?? senderPubkey }
}

private enum GroupedBubblePosition {
    case single
    case first
    case middle
    case last
}

private struct MessageGroupRow: View {
    let group: GroupedChatMessage
    var showSender: Bool = false
    let onSendMessage: @MainActor (String) -> Void

    private let avatarSize: CGFloat = 24
    private let avatarGutterWidth: CGFloat = 28

    var body: some View {
        Group {
            if group.isMine {
                outgoingRow
            } else {
                incomingRow
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var incomingRow: some View {
        HStack(alignment: .bottom, spacing: 8) {
            AvatarView(
                name: group.senderName,
                npub: group.senderNpub,
                pictureUrl: group.senderPictureUrl,
                size: avatarSize
            )
            .frame(width: avatarGutterWidth, alignment: .leading)
            .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 3) {
                if showSender {
                    Text(displaySenderName)
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
                MessageBubbleStack(group: group, onSendMessage: onSendMessage)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Spacer(minLength: 24)
        }
    }

    private var outgoingRow: some View {
        HStack(alignment: .bottom, spacing: 8) {
            Spacer(minLength: 24)

            VStack(alignment: .trailing, spacing: 3) {
                MessageBubbleStack(group: group, onSendMessage: onSendMessage)
                if let delivery = group.messages.last?.delivery {
                    Text(deliveryText(delivery))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        }
    }

    private var displaySenderName: String {
        if let name = group.senderName, !name.isEmpty {
            return name
        }
        if group.senderNpub.count <= 12 { return group.senderNpub }
        return String(group.senderNpub.prefix(12)) + "..."
    }
}

private struct MessageBubbleStack: View {
    let group: GroupedChatMessage
    let onSendMessage: @MainActor (String) -> Void

    var body: some View {
        VStack(alignment: group.isMine ? .trailing : .leading, spacing: 2) {
            ForEach(Array(group.messages.enumerated()), id: \.element.id) { index, message in
                MessageBubble(
                    message: message,
                    position: bubblePosition(at: index, count: group.messages.count),
                    onSendMessage: onSendMessage
                )
                .id(message.id)
            }
        }
    }

    private func bubblePosition(at index: Int, count: Int) -> GroupedBubblePosition {
        guard count > 1 else { return .single }
        if index == 0 { return .first }
        if index == count - 1 { return .last }
        return .middle
    }
}

private struct MessageBubble: View {
    let message: ChatMessage
    let position: GroupedBubblePosition
    let onSendMessage: @MainActor (String) -> Void

    private let roundedCornerRadius: CGFloat = 16
    private let groupedCornerRadius: CGFloat = 6

    var body: some View {
        let segments = parseMessageSegments(message.displayContent)
        ForEach(segments) { segment in
            switch segment {
            case .markdown(let text):
                markdownBubble(text: text)
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
    }

    private func markdownBubble(text: String) -> some View {
        VStack(alignment: message.isMine ? .trailing : .leading, spacing: 3) {
            Markdown(text)
                .markdownTheme(message.isMine ? .pikaOutgoing : .pikaIncoming)
                .multilineTextAlignment(message.isMine ? .trailing : .leading)

            Text(timestampText)
                .font(.caption2)
                .foregroundStyle(message.isMine ? Color.white.opacity(0.78) : Color.secondary.opacity(0.9))
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .background(message.isMine ? Color.blue : Color.gray.opacity(0.2))
        .clipShape(UnevenRoundedRectangle(cornerRadii: bubbleRadii, style: .continuous))
    }

    private var timestampText: String {
        Date(timeIntervalSince1970: TimeInterval(message.timestamp))
            .formatted(date: .omitted, time: .shortened)
    }

    private var bubbleRadii: RectangleCornerRadii {
        if message.isMine {
            return .init(
                topLeading: roundedCornerRadius,
                bottomLeading: roundedCornerRadius,
                bottomTrailing: tailRadius(for: .bottom),
                topTrailing: tailRadius(for: .top)
            )
        }
        return .init(
            topLeading: tailRadius(for: .top),
            bottomLeading: tailRadius(for: .bottom),
            bottomTrailing: roundedCornerRadius,
            topTrailing: roundedCornerRadius
        )
    }

    private enum TailEdge {
        case top
        case bottom
    }

    private func tailRadius(for edge: TailEdge) -> CGFloat {
        switch (position, edge) {
        case (.single, _):
            return roundedCornerRadius
        case (.first, .top):
            return roundedCornerRadius
        case (.first, .bottom):
            return groupedCornerRadius
        case (.middle, _):
            return groupedCornerRadius
        case (.last, .top):
            return groupedCornerRadius
        case (.last, .bottom):
            return roundedCornerRadius
        }
    }
}

private func deliveryText(_ d: MessageDeliveryState) -> String {
    switch d {
    case .pending: return "Pending"
    case .sent: return "Sent"
    case .failed(let reason): return "Failed: \(reason)"
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
private enum ChatViewPreviewData {
    static let incomingGroup = GroupedChatMessage(
        senderPubkey: PreviewAppState.samplePeerPubkey,
        senderName: "Anthony",
        senderNpub: PreviewAppState.samplePeerNpub,
        senderPictureUrl: "https://blossom.nostr.pub/8dbc6f42ea8bf53f4af89af87eb0d9110fcaf4d263f7d2cb9f29d68f95f6f8ce",
        isMine: false,
        messages: [
            ChatMessage(
                id: "incoming-1",
                senderPubkey: PreviewAppState.samplePeerPubkey,
                senderName: "Anthony",
                content: "First incoming bubble in a grouped run.",
                displayContent: "First incoming bubble in a grouped run.",
                mentions: [],
                timestamp: 1_709_100_001,
                isMine: false,
                delivery: .sent
            ),
            ChatMessage(
                id: "incoming-2",
                senderPubkey: PreviewAppState.samplePeerPubkey,
                senderName: "Anthony",
                content: "Second message should visually join with the first.",
                displayContent: "Second message should visually join with the first.",
                mentions: [],
                timestamp: 1_709_100_002,
                isMine: false,
                delivery: .sent
            ),
        ]
    )

    static let outgoingGroup = GroupedChatMessage(
        senderPubkey: PreviewAppState.samplePubkey,
        senderName: nil,
        senderNpub: PreviewAppState.sampleNpub,
        senderPictureUrl: nil,
        isMine: true,
        messages: [
            ChatMessage(
                id: "outgoing-1",
                senderPubkey: PreviewAppState.samplePubkey,
                senderName: nil,
                content: "I can meet outside in five.",
                displayContent: "I can meet outside in five.",
                mentions: [],
                timestamp: 1_709_100_010,
                isMine: true,
                delivery: .sent
            ),
            ChatMessage(
                id: "outgoing-2",
                senderPubkey: PreviewAppState.samplePubkey,
                senderName: nil,
                content: "If you're near ana's market, I'll find you.",
                displayContent: "If you're near ana's market, I'll find you.",
                mentions: [],
                timestamp: 1_709_100_011,
                isMine: true,
                delivery: .pending
            ),
        ]
    )
}

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

#Preview("Chat - Grouped") {
    NavigationStack {
        ChatView(
            chatId: "chat-grouped",
            state: ChatScreenState(chat: PreviewAppState.chatDetailGrouped.currentChat),
            onSendMessage: { _ in }
        )
    }
}

#Preview("Message Group - Incoming") {
    MessageGroupRow(group: ChatViewPreviewData.incomingGroup, showSender: true, onSendMessage: { _ in })
        .padding(16)
}

#Preview("Message Group - Outgoing") {
    MessageGroupRow(group: ChatViewPreviewData.outgoingGroup, showSender: true, onSendMessage: { _ in })
        .padding(16)
}
#endif
