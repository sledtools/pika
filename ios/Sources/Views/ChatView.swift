import SwiftUI
import MarkdownUI
import WebKit

struct ChatView: View {
    let chatId: String
    let state: ChatScreenState
    let onSendMessage: @MainActor (String) -> Void
    let onGroupInfo: (@MainActor () -> Void)?
    let onTapSender: (@MainActor (String) -> Void)?
    let onReact: (@MainActor (String, String) -> Void)?
    @State private var messageText = ""
    @State private var isAtBottom = true
    @State private var showMentionPicker = false
    @State private var mentionQuery = ""
    @State private var insertedMentions: [(display: String, npub: String)] = []
    @FocusState private var isInputFocused: Bool

    private let scrollButtonBottomPadding: CGFloat = 12

    init(chatId: String, state: ChatScreenState, onSendMessage: @escaping @MainActor (String) -> Void, onGroupInfo: (@MainActor () -> Void)? = nil, onTapSender: (@MainActor (String) -> Void)? = nil, onReact: (@MainActor (String, String) -> Void)? = nil) {
        self.chatId = chatId
        self.state = state
        self.onSendMessage = onSendMessage
        self.onGroupInfo = onGroupInfo
        self.onTapSender = onTapSender
        self.onReact = onReact
    }

    var body: some View {
        if let chat = state.chat, chat.chatId == chatId {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 0) {
                        LazyVStack(spacing: 8) {
                            ForEach(groupedMessages(chat)) { group in
                                MessageGroupRow(group: group, showSender: chat.isGroup, onSendMessage: onSendMessage, onTapSender: onTapSender, onReact: onReact)
                            }
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 10)

                        GeometryReader { geo in
                            Color.clear.preference(
                                key: BottomVisibleKey.self,
                                value: geo.frame(in: .named("chatScroll")).minY
                            )
                        }
                        .frame(height: 1)
                        .id("bottom-anchor")
                    }
                }
                .coordinateSpace(name: "chatScroll")
                .defaultScrollAnchor(.bottom)
                .onPreferenceChange(BottomVisibleKey.self) { minY in
                    // The anchor is visible when its top edge is within the scroll view bounds.
                    // Give some tolerance (100pt) to account for the input bar overlay.
                    if let minY {
                        isAtBottom = minY < UIScreen.main.bounds.height + 100
                    }
                }
                .overlay(alignment: .bottomTrailing) {
                    if !isAtBottom {
                        Button {
                            withAnimation(.easeOut(duration: 0.2)) {
                                proxy.scrollTo("bottom-anchor", anchor: .bottom)
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
            }
            .modifier(FloatingInputBarModifier(content: { messageInputBar(chat: chat) }))
            .navigationTitle(chat.isGroup ? chatTitle(chat) : "")
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
                } else if let peer = chat.members.first {
                    ToolbarItem(placement: .principal) {
                        Button {
                            onTapSender?(peer.pubkey)
                        } label: {
                            HStack(spacing: 8) {
                                AvatarView(
                                    name: peer.name,
                                    npub: peer.npub,
                                    pictureUrl: peer.pictureUrl,
                                    size: 24
                                )
                                Text(chatTitle(chat))
                                    .font(.headline)
                                    .foregroundStyle(.primary)
                            }
                        }
                        .buttonStyle(.plain)
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
                MentionPickerPopup(members: chat.members, query: mentionQuery) { member in
                    let displayTag = "@\(member.name ?? String(member.npub.prefix(8)))"
                    // Remove the "@" + any partial query that triggered the picker.
                    if let atIdx = messageText.lastIndex(of: "@") {
                        messageText = String(messageText[..<atIdx])
                    }
                    messageText += "\(displayTag) "
                    insertedMentions.append((display: displayTag, npub: member.npub))
                    mentionQuery = ""
                    showMentionPicker = false
                }
            }

            HStack(spacing: 10) {
                TextEditor(text: $messageText)
                    .focused($isInputFocused)
                    .frame(minHeight: 36, maxHeight: 150)
                    .fixedSize(horizontal: false, vertical: true)
                    .scrollContentBackground(.hidden)
                    .onAppear {
                        if ProcessInfo.processInfo.isiOSAppOnMac {
                            isInputFocused = true
                        }
                    }
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
                            if let atIdx = newValue.lastIndex(of: "@") {
                                let prefix = newValue[..<atIdx]
                                let isWordStart = prefix.isEmpty || prefix.last == " " || prefix.last == "\n"
                                if isWordStart {
                                    let query = String(newValue[newValue.index(after: atIdx)...])
                                    if !query.contains(" ") {
                                        showMentionPicker = true
                                        mentionQuery = query
                                    } else {
                                        showMentionPicker = false
                                        mentionQuery = ""
                                    }
                                } else if showMentionPicker {
                                    showMentionPicker = false
                                    mentionQuery = ""
                                }
                            } else if showMentionPicker {
                                showMentionPicker = false
                                mentionQuery = ""
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
    let query: String
    let onSelect: (MemberInfo) -> Void

    private var filteredMembers: [MemberInfo] {
        guard !query.isEmpty else { return members }
        let q = query.lowercased()
        return members.filter { member in
            if let name = member.name, name.lowercased().hasPrefix(q) { return true }
            return member.npub.lowercased().hasPrefix(q)
        }
    }

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                ForEach(filteredMembers, id: \.pubkey) { member in
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
                    if member.pubkey != filteredMembers.last?.pubkey {
                        Divider().padding(.leading, 48)
                    }
                }
            }
        }
        .frame(maxHeight: min(CGFloat(filteredMembers.count) * 44, 180))
        .background(.ultraThinMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .padding(.horizontal, 12)
    }
}

private struct QuickReactionBar: View {
    let onSelect: (String) -> Void
    let onMore: () -> Void
    let onCopy: () -> Void

    private let emojis = ["❤️", "👍", "👎", "😂", "😮", "😢"]

    var body: some View {
        HStack(spacing: 8) {
            ForEach(emojis, id: \.self) { emoji in
                Button {
                    onSelect(emoji)
                } label: {
                    Text(emoji)
                        .font(.title2)
                        .frame(width: 36, height: 36)
                }
                .buttonStyle(.plain)
            }
            Button {
                onMore()
            } label: {
                Image(systemName: "plus")
                    .font(.system(size: 16, weight: .medium))
                    .foregroundStyle(.secondary)
                    .frame(width: 32, height: 32)
                    .background(Color.gray.opacity(0.2))
                    .clipShape(Circle())
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
        .shadow(color: .black.opacity(0.18), radius: 12, y: 4)
    }
}

private struct ReactionChips: View {
    let reactions: [ReactionSummary]
    let messageId: String
    var onReact: ((String, String) -> Void)?

    var body: some View {
        HStack(spacing: 4) {
            ForEach(reactions, id: \.emoji) { reaction in
                Button {
                    onReact?(messageId, reaction.emoji)
                } label: {
                    HStack(spacing: 2) {
                        Text(reaction.emoji)
                            .font(.system(size: 13))
                        if reaction.count > 1 {
                            Text("\(reaction.count)")
                                .font(.system(size: 10, weight: .medium))
                                .foregroundStyle(.white)
                        }
                    }
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(
                        reaction.reactedByMe
                            ? Color.blue.opacity(0.85)
                            : Color(white: 0.22)
                    )
                    .clipShape(Capsule())
                    .overlay(
                        Capsule().strokeBorder(Color(uiColor: .systemBackground), lineWidth: 1.5)
                    )
                }
                .buttonStyle(.plain)
            }
        }
    }
}

private struct EmojiPickerSheet: View {
    let onSelect: (String) -> Void
    @State private var searchText = ""
    @Environment(\.dismiss) private var dismiss

    private let recentEmojis = ["❤️", "👍", "👎", "😂", "😮", "😢", "🔥", "🎉", "👀", "🙏", "💯", "🤔"]
    private let allEmojis: [(String, [String])] = [
        ("Smileys", ["😀", "😃", "😄", "😁", "😆", "🥹", "😅", "🤣", "😂", "🙂", "😊", "😇", "🥰", "😍", "🤩", "😘", "😗", "😚", "😙", "🥲", "😋", "😛", "😜", "🤪", "😝", "🤑", "🤗", "🤭", "🤫", "🤔", "🫡", "🤐", "🤨", "😐", "😑", "😶", "🫥", "😏", "😒", "🙄", "😬", "🤥", "😌", "😔", "😪", "🤤", "😴", "😷", "🤒", "🤕", "🤢", "🤮", "🥵", "🥶", "🥴", "😵", "🤯", "🤠", "🥳", "🥸", "😎", "🤓", "🧐", "😕", "🫤", "😟", "🙁", "😮", "😯", "😲", "😳", "🥺", "🥹", "😦", "😧", "😨", "😰", "😥", "😢", "😭", "😱", "😖", "😣", "😞", "😓", "😩", "😫", "🥱", "😤", "😡", "😠", "🤬", "😈", "👿", "💀", "☠️", "💩", "🤡", "👹", "👺", "👻", "👽", "👾", "🤖"]),
        ("Gestures", ["👋", "🤚", "🖐️", "✋", "🖖", "🫱", "🫲", "🫳", "🫴", "👌", "🤌", "🤏", "✌️", "🤞", "🫰", "🤟", "🤘", "🤙", "👈", "👉", "👆", "🖕", "👇", "☝️", "🫵", "👍", "👎", "✊", "👊", "🤛", "🤜", "👏", "🙌", "🫶", "👐", "🤲", "🤝", "🙏", "💪"]),
        ("Hearts", ["❤️", "🧡", "💛", "💚", "💙", "💜", "🖤", "🤍", "🤎", "💔", "❤️‍🔥", "❤️‍🩹", "❣️", "💕", "💞", "💓", "💗", "💖", "💘", "💝"]),
        ("Symbols", ["⭐", "🌟", "✨", "💫", "🔥", "💥", "💯", "💢", "💬", "👁️‍🗨️", "🗨️", "💭", "✅", "❌", "❓", "❗", "‼️", "⁉️", "🔴", "🟠", "🟡", "🟢", "🔵", "🟣", "⚫", "⚪", "🟤"]),
    ]

    var body: some View {
        NavigationView {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if searchText.isEmpty {
                        emojiSection(title: "Recent", emojis: recentEmojis)
                        ForEach(allEmojis, id: \.0) { title, emojis in
                            emojiSection(title: title, emojis: emojis)
                        }
                    } else {
                        let filtered = allEmojis.flatMap { $0.1 }
                        emojiGrid(emojis: filtered)
                    }
                }
                .padding()
            }
            .navigationTitle("Reactions")
            .navigationBarTitleDisplayMode(.inline)
            .searchable(text: $searchText, prompt: "Search emoji")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
    }

    private func emojiSection(title: String, emojis: [String]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.secondary)
            emojiGrid(emojis: emojis)
        }
    }

    private func emojiGrid(emojis: [String]) -> some View {
        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 4), count: 8), spacing: 8) {
            ForEach(emojis, id: \.self) { emoji in
                Button {
                    onSelect(emoji)
                } label: {
                    Text(emoji)
                        .font(.title2)
                }
                .buttonStyle(.plain)
            }
        }
    }
}

private struct BottomVisibleKey: PreferenceKey {
    static var defaultValue: CGFloat? = nil
    static func reduce(value: inout CGFloat?, nextValue: () -> CGFloat?) {
        value = nextValue() ?? value
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
    case pikaHtml(String)

    var id: String {
        switch self {
        case .markdown(let text): return "md-\(text.hashValue)"
        case .pikaPrompt(let prompt): return "prompt-\(prompt.title.hashValue)"
        case .pikaHtml(let html): return "html-\(html.hashValue)"
        }
    }
}

private func parseMessageSegments(_ content: String) -> [MessageSegment] {
    var segments: [MessageSegment] = []
    let pattern = /```pika-([\w-]+)(?:[ \t]+(\S+))?\n([\s\S]*?)```/
    var remaining = content[...]

    while let match = remaining.firstMatch(of: pattern) {
        let before = String(remaining[remaining.startIndex..<match.range.lowerBound])
        if !before.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            segments.append(.markdown(before))
        }

        let blockType = String(match.output.1)
        let blockBody = String(match.output.3).trimmingCharacters(in: .whitespacesAndNewlines)

        switch blockType {
        case "prompt":
            if let data = blockBody.data(using: .utf8),
               let prompt = try? JSONDecoder().decode(PikaPrompt.self, from: data) {
                segments.append(.pikaPrompt(prompt))
            }
        case "html":
            segments.append(.pikaHtml(blockBody))
        case "html-update", "prompt-response":
            break // Consumed by Rust core; silently drop if one slips through.
        default:
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
    var onTapSender: (@MainActor (String) -> Void)?
    var onReact: ((String, String) -> Void)?
    @State private var activeReactionMessageId: String?

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
            .onTapGesture { onTapSender?(group.senderPubkey) }

            VStack(alignment: .leading, spacing: 3) {
                if showSender {
                    Text(displaySenderName)
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .onTapGesture { onTapSender?(group.senderPubkey) }
                }
                MessageBubbleStack(group: group, onSendMessage: onSendMessage, onReact: onReact, activeReactionMessageId: $activeReactionMessageId)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Spacer(minLength: 24)
        }
    }

    private var outgoingRow: some View {
        HStack(alignment: .bottom, spacing: 8) {
            Spacer(minLength: 24)

            VStack(alignment: .trailing, spacing: 3) {
                MessageBubbleStack(group: group, onSendMessage: onSendMessage, onReact: onReact, activeReactionMessageId: $activeReactionMessageId)
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
    var onReact: ((String, String) -> Void)?
    @Binding var activeReactionMessageId: String?

    var body: some View {
        VStack(alignment: group.isMine ? .trailing : .leading, spacing: 2) {
            ForEach(Array(group.messages.enumerated()), id: \.element.id) { index, message in
                MessageBubble(
                    message: message,
                    position: bubblePosition(at: index, count: group.messages.count),
                    onSendMessage: onSendMessage,
                    onReact: onReact,
                    activeReactionMessageId: $activeReactionMessageId
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
    var onReact: ((String, String) -> Void)?
    @Binding var activeReactionMessageId: String?

    private let roundedCornerRadius: CGFloat = 16
    private let groupedCornerRadius: CGFloat = 6

    @State private var showEmojiPicker = false

    private var isShowingReactionBar: Bool {
        activeReactionMessageId == message.id
    }

    private let reactionChipOverlap: CGFloat = 10

    var body: some View {
        let hasReactions = !message.reactions.isEmpty
        let segments = parseMessageSegments(message.displayContent)

        VStack(alignment: message.isMine ? .trailing : .leading, spacing: 0) {
            ForEach(segments) { segment in
                switch segment {
                case .markdown(let text):
                    markdownBubble(text: text)
                case .pikaPrompt(let prompt):
                    PikaPromptView(prompt: prompt, message: message, onSelect: onSendMessage)
                case .pikaHtml(let html):
                    PikaHtmlView(html: html, onSendMessage: onSendMessage)
                }
            }
            .onLongPressGesture {
                let impactFeedback = UIImpactFeedbackGenerator(style: .medium)
                impactFeedback.impactOccurred()
                withAnimation(.spring(response: 0.3, dampingFraction: 0.7)) {
                    activeReactionMessageId = message.id
                }
            }
            .overlay(alignment: message.isMine ? .topTrailing : .topLeading) {
                if isShowingReactionBar {
                    QuickReactionBar(
                        onSelect: { emoji in
                            withAnimation(.easeOut(duration: 0.15)) {
                                activeReactionMessageId = nil
                            }
                            onReact?(message.id, emoji)
                        },
                        onMore: {
                            withAnimation(.easeOut(duration: 0.15)) {
                                activeReactionMessageId = nil
                            }
                            showEmojiPicker = true
                        },
                        onCopy: {
                            UIPasteboard.general.string = message.displayContent
                            withAnimation(.easeOut(duration: 0.15)) {
                                activeReactionMessageId = nil
                            }
                        }
                    )
                    .transition(.scale(scale: 0.5, anchor: message.isMine ? .bottomTrailing : .bottomLeading).combined(with: .opacity))
                    .offset(y: -48)
                }
            }
            .overlay(alignment: message.isMine ? .bottomLeading : .bottomTrailing) {
                if hasReactions {
                    ReactionChips(
                        reactions: message.reactions,
                        messageId: message.id,
                        onReact: onReact
                    )
                    .offset(x: message.isMine ? -12 : 12, y: reactionChipOverlap)
                }
            }
            .sheet(isPresented: $showEmojiPicker) {
                EmojiPickerSheet { emoji in
                    onReact?(message.id, emoji)
                    showEmojiPicker = false
                }
                .presentationDetents([.medium, .large])
            }

            if hasReactions {
                Spacer().frame(height: reactionChipOverlap + 4)
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
    let message: ChatMessage
    let onSelect: @MainActor (String) -> Void

    private var hasVoted: Bool { message.myPollVote != nil }
    private var hasTallies: Bool { !message.pollTally.isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(prompt.title)
                .font(.subheadline.weight(.semibold))
            ForEach(prompt.options, id: \.self) { option in
                let tally = message.pollTally.first(where: { $0.option == option })
                let isMyVote = message.myPollVote == option
                Button {
                    let response = """
                    ```pika-prompt-response
                    {"prompt_id":"\(message.id)","selected":"\(option)"}
                    ```
                    """
                    onSelect(response)
                } label: {
                    HStack {
                        Text(option)
                        Spacer()
                        if let tally {
                            Text("\(tally.count)")
                                .font(.subheadline.weight(.semibold))
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(isMyVote ? Color.blue.opacity(0.25) : Color.blue.opacity(0.1))
                    .foregroundStyle(Color.blue)
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                    .overlay(
                        RoundedRectangle(cornerRadius: 10)
                            .strokeBorder(isMyVote ? Color.blue : Color.clear, lineWidth: 1.5)
                    )
                }
                .disabled(hasVoted)
                if let tally, !tally.voterNames.isEmpty {
                    Text(tally.voterNames.joined(separator: ", "))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .padding(.leading, 12)
                }
            }
        }
        .padding(12)
        .background(Color.gray.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: 16))
    }
}

// MARK: - Pika HTML view

private struct PikaHtmlView: View {
    let html: String
    let onSendMessage: @MainActor (String) -> Void

    @State private var contentHeight: CGFloat = 100
    @State private var showFullScreen = false

    var body: some View {
        PikaWebView(html: html, contentHeight: $contentHeight, onSendMessage: onSendMessage, interactive: false)
            .frame(height: min(contentHeight, 400))
            .clipShape(RoundedRectangle(cornerRadius: 16))
            .background(Color.gray.opacity(0.1))
            .clipShape(RoundedRectangle(cornerRadius: 16))
            .contentShape(Rectangle())
            .onTapGesture { showFullScreen = true }
            .fullScreenCover(isPresented: $showFullScreen) {
                PikaHtmlFullScreen(html: html, onSendMessage: onSendMessage, isPresented: $showFullScreen)
            }
    }
}

private struct PikaHtmlFullScreen: View {
    let html: String
    let onSendMessage: @MainActor (String) -> Void
    @Binding var isPresented: Bool

    var body: some View {
        NavigationStack {
            PikaFullScreenWebView(html: html, onSendMessage: onSendMessage)
                .navigationTitle("HTML")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Done") { isPresented = false }
                    }
                }
        }
    }
}

private struct PikaFullScreenWebView: UIViewRepresentable {
    let html: String
    let onSendMessage: @MainActor (String) -> Void

    func makeCoordinator() -> PikaWebView.Coordinator {
        PikaWebView.Coordinator(onSendMessage: onSendMessage)
    }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        let userContentController = WKUserContentController()
        userContentController.add(context.coordinator, name: "pikaSend")

        let bridgeScript = WKUserScript(
            source: "window.pika = { send: function(text) { window.webkit.messageHandlers.pikaSend.postMessage(text); } };",
            injectionTime: .atDocumentStart,
            forMainFrameOnly: true
        )
        userContentController.addUserScript(bridgeScript)
        config.userContentController = userContentController

        let webView = WKWebView(frame: .zero, configuration: config)
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.navigationDelegate = context.coordinator

        let wrapped = """
        <!DOCTYPE html>
        <html>
        <head>
        <meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1">
        <style>
        :root { color-scheme: light dark; }
        body { margin: 8px; font-family: -apple-system, sans-serif; background: transparent; }
        </style>
        </head>
        <body>\(html)</body>
        </html>
        """
        webView.loadHTMLString(wrapped, baseURL: nil)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {}
}

private struct PikaWebView: UIViewRepresentable {
    let html: String
    @Binding var contentHeight: CGFloat
    let onSendMessage: @MainActor (String) -> Void
    var interactive: Bool = true

    func makeCoordinator() -> Coordinator {
        Coordinator(onSendMessage: onSendMessage)
    }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        let userContentController = WKUserContentController()
        userContentController.add(context.coordinator, name: "pikaSend")

        let bridgeScript = WKUserScript(
            source: "window.pika = { send: function(text) { window.webkit.messageHandlers.pikaSend.postMessage(text); } };",
            injectionTime: .atDocumentStart,
            forMainFrameOnly: true
        )
        userContentController.addUserScript(bridgeScript)
        config.userContentController = userContentController

        let webView = WKWebView(frame: .zero, configuration: config)
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.scrollView.backgroundColor = .clear
        webView.scrollView.isScrollEnabled = false
        webView.isUserInteractionEnabled = interactive
        webView.navigationDelegate = context.coordinator

        let binding = $contentHeight
        context.coordinator.onHeightChange = { height in
            Task { @MainActor in
                binding.wrappedValue = height
            }
        }
        context.coordinator.observeContentSize(webView)

        let wrapped = """
        <!DOCTYPE html>
        <html>
        <head>
        <meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1">
        <style>
        :root { color-scheme: light dark; }
        body { margin: 8px; font-family: -apple-system, sans-serif; background: transparent; }
        </style>
        </head>
        <body>\(html)</body>
        </html>
        """
        webView.loadHTMLString(wrapped, baseURL: nil)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {}

    class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
        let onSendMessage: @MainActor (String) -> Void
        var onHeightChange: ((CGFloat) -> Void)?
        private var observation: NSKeyValueObservation?

        init(onSendMessage: @escaping @MainActor (String) -> Void) {
            self.onSendMessage = onSendMessage
        }

        func observeContentSize(_ webView: WKWebView) {
            observation = webView.scrollView.observe(\.contentSize, options: .new) { [weak self] _, change in
                if let size = change.newValue, size.height > 0 {
                    self?.onHeightChange?(size.height)
                }
            }
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            switch message.name {
            case "pikaSend":
                if let text = message.body as? String {
                    Task { @MainActor in
                        onSendMessage(text)
                    }
                }
            default:
                break
            }
        }

        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            if navigationAction.navigationType == .linkActivated {
                decisionHandler(.cancel)
            } else {
                decisionHandler(.allow)
            }
        }
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
                delivery: .sent,
                reactions: [],
                pollTally: [],
                myPollVote: nil
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
                delivery: .sent,
                reactions: [],
                pollTally: [],
                myPollVote: nil
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
                delivery: .sent,
                reactions: [],
                pollTally: [],
                myPollVote: nil
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
                delivery: .pending,
                reactions: [],
                pollTally: [],
                myPollVote: nil
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
