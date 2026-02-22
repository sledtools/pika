import SwiftUI
import MarkdownUI
import WebKit

// WKWebView requires a resolvable HTTPS baseURL for loadHTMLString to allow
// fetching external subresources (images, scripts, etc.). The domain must
// actually resolve — non-routable origins like localhost or .invalid break
// asset loading. We use a domain we control that won't serve unexpected content.
// TODO: Change to a pika related domain
private let webViewBaseURL = URL(string: "https://webview.benthecarman.com")!

// MARK: - Pika-prompt model

struct PikaPrompt: Decodable {
    let title: String
    let options: [String]
}

/// Parses message content into segments: plain markdown text and pika-* code blocks.
enum MessageSegment: Identifiable {
    case markdown(String)
    case pikaPrompt(PikaPrompt)
    case pikaHtml(id: String?, html: String, state: String?)

    var id: String {
        switch self {
        case .markdown(let text): return "md-\(text.hashValue)"
        case .pikaPrompt(let prompt): return "prompt-\(prompt.title.hashValue)"
        case .pikaHtml(let id, let html, _):
            if let id { return "html-\(id)" }
            return "html-\(html.hashValue)"
        }
    }
}

func parseMessageSegments(_ content: String, htmlState: String? = nil) -> [MessageSegment] {
    var segments: [MessageSegment] = []
    let pattern = /```pika-([\w-]+)(?:[ \t]+(\S+))?\n([\s\S]*?)```/
    var remaining = content[...]

    while let match = remaining.firstMatch(of: pattern) {
        let before = String(remaining[remaining.startIndex..<match.range.lowerBound])
        if !before.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            segments.append(.markdown(before))
        }

        let blockType = String(match.output.1)
        let blockId = match.output.2.map(String.init)
        let blockBody = String(match.output.3).trimmingCharacters(in: .whitespacesAndNewlines)

        switch blockType {
        case "prompt":
            if let data = blockBody.data(using: .utf8),
               let prompt = try? JSONDecoder().decode(PikaPrompt.self, from: data) {
                segments.append(.pikaPrompt(prompt))
            }
        case "html":
            segments.append(.pikaHtml(id: blockId, html: blockBody, state: htmlState))
        case "html-update", "html-state-update", "prompt-response":
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

struct GroupedChatMessage: Identifiable {
    let senderPubkey: String
    let senderName: String?
    let senderNpub: String
    let senderPictureUrl: String?
    let isMine: Bool
    var messages: [ChatMessage]

    var id: String { messages.first?.id ?? senderPubkey }
}

enum GroupedBubblePosition {
    case single
    case first
    case middle
    case last
}

// MARK: - Message group row

struct MessageGroupRow: View {
    let group: GroupedChatMessage
    var showSender: Bool = false
    let onSendMessage: @MainActor (String, String?) -> Void
    let replyTargetsById: [String: ChatMessage]
    var onTapSender: (@MainActor (String) -> Void)?
    var onJumpToMessage: ((String) -> Void)? = nil
    var onReact: ((String, String) -> Void)?
    @Binding var activeReactionMessageId: String?
    var onLongPressMessage: ((ChatMessage) -> Void)? = nil
    var onDownloadMedia: ((String, String) -> Void)? = nil
    var onTapImage: ((ChatMediaAttachment) -> Void)? = nil

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
                MessageBubbleStack(
                    group: group,
                    onSendMessage: onSendMessage,
                    replyTargetsById: replyTargetsById,
                    onReact: onReact,
                    onJumpToMessage: onJumpToMessage,
                    activeReactionMessageId: $activeReactionMessageId,
                    onLongPressMessage: onLongPressMessage,
                    onDownloadMedia: onDownloadMedia,
                    onTapImage: onTapImage
                )
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Spacer(minLength: 24)
        }
    }

    private var outgoingRow: some View {
        HStack(alignment: .bottom, spacing: 8) {
            Spacer(minLength: 48)

            VStack(alignment: .trailing, spacing: 3) {
                MessageBubbleStack(
                    group: group,
                    onSendMessage: onSendMessage,
                    replyTargetsById: replyTargetsById,
                    onReact: onReact,
                    onJumpToMessage: onJumpToMessage,
                    activeReactionMessageId: $activeReactionMessageId,
                    onLongPressMessage: onLongPressMessage,
                    onDownloadMedia: onDownloadMedia,
                    onTapImage: onTapImage
                )
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

// MARK: - Message bubble stack

private struct MessageBubbleStack: View {
    let group: GroupedChatMessage
    let onSendMessage: @MainActor (String, String?) -> Void
    let replyTargetsById: [String: ChatMessage]
    var onReact: ((String, String) -> Void)?
    var onJumpToMessage: ((String) -> Void)? = nil
    @Binding var activeReactionMessageId: String?
    var onLongPressMessage: ((ChatMessage) -> Void)? = nil
    var onDownloadMedia: ((String, String) -> Void)? = nil
    var onTapImage: ((ChatMediaAttachment) -> Void)? = nil

    var body: some View {
        VStack(alignment: group.isMine ? .trailing : .leading, spacing: 2) {
            ForEach(Array(group.messages.enumerated()), id: \.element.id) { index, message in
                MessageBubble(
                    message: message,
                    position: bubblePosition(at: index, count: group.messages.count),
                    onSendMessage: onSendMessage,
                    replyTargetsById: replyTargetsById,
                    onReact: onReact,
                    onJumpToMessage: onJumpToMessage,
                    activeReactionMessageId: $activeReactionMessageId,
                    onLongPressMessage: onLongPressMessage,
                    onDownloadMedia: onDownloadMedia,
                    onTapImage: onTapImage
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

// MARK: - Reaction chips

struct ReactionChips: View {
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
                                .foregroundStyle(reaction.reactedByMe ? .white : .primary)
                        }
                    }
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(
                        reaction.reactedByMe
                            ? Color.blue.opacity(0.85)
                            : Color(uiColor: .systemGray5)
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

// MARK: - Media attachment view

struct MediaAttachmentView: View {
    let attachment: ChatMediaAttachment
    let isMine: Bool
    var maxMediaWidth: CGFloat = 240
    var maxMediaHeight: CGFloat = .infinity
    var onDownload: (() -> Void)? = nil
    var onTapImage: (() -> Void)? = nil

    private var isImage: Bool {
        attachment.mimeType.hasPrefix("image/")
    }

    private var aspectRatio: CGFloat {
        if let w = attachment.width, let h = attachment.height, w > 0, h > 0 {
            return CGFloat(w) / CGFloat(h)
        }
        return 4.0 / 3.0
    }

    private var imageSize: CGSize {
        let w = maxMediaWidth
        let h = w / aspectRatio
        if h > maxMediaHeight {
            return CGSize(width: maxMediaHeight * aspectRatio, height: maxMediaHeight)
        }
        return CGSize(width: w, height: h)
    }

    var body: some View {
        if isImage {
            imageContent
        } else {
            fileRow
        }
    }

    @ViewBuilder
    private var imageContent: some View {
        if let localPath = attachment.localPath {
            CachedAsyncImage(url: URL(fileURLWithPath: localPath)) { image in
                image
                    .resizable()
                    .scaledToFill()
            } placeholder: {
                imagePlaceholder
            }
            .frame(width: imageSize.width, height: imageSize.height)
            .clipped()
            .contentShape(Rectangle())
            .onTapGesture { onTapImage?() }
            .allowsHitTesting(onTapImage != nil)
        } else {
            Button {
                onDownload?()
            } label: {
                ZStack {
                    imagePlaceholder
                    VStack(spacing: 6) {
                        Image(systemName: "arrow.down.circle")
                            .font(.title)
                            .foregroundStyle(.white)
                        Text(attachment.filename)
                            .font(.caption2)
                            .foregroundStyle(.white.opacity(0.8))
                            .lineLimit(1)
                    }
                }
            }
            .buttonStyle(.plain)
            .frame(width: imageSize.width, height: imageSize.height)
        }
    }

    private var imagePlaceholder: some View {
        Rectangle()
            .fill(isMine ? Color.white.opacity(0.15) : Color.gray.opacity(0.2))
            .frame(width: imageSize.width, height: imageSize.height)
    }

    private var fileRow: some View {
        HStack(spacing: 10) {
            Image(systemName: "doc")
                .font(.title3)
                .foregroundStyle(isMine ? .white.opacity(0.8) : .secondary)

            VStack(alignment: .leading, spacing: 2) {
                Text(attachment.filename)
                    .font(.subheadline)
                    .foregroundStyle(isMine ? .white : .primary)
                    .lineLimit(1)
                Text(attachment.mimeType)
                    .font(.caption2)
                    .foregroundStyle(isMine ? .white.opacity(0.6) : .secondary)
            }

            Spacer(minLength: 0)

            if attachment.localPath == nil {
                Button {
                    onDownload?()
                } label: {
                    Image(systemName: "arrow.down.circle")
                        .font(.title3)
                        .foregroundStyle(isMine ? .white : .blue)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: maxMediaWidth)
    }
}

// MARK: - Message bubble

private struct MessageBubble: View {
    let message: ChatMessage
    let position: GroupedBubblePosition
    let onSendMessage: @MainActor (String, String?) -> Void
    let replyTargetsById: [String: ChatMessage]
    var onReact: ((String, String) -> Void)?
    var onJumpToMessage: ((String) -> Void)? = nil
    @Binding var activeReactionMessageId: String?
    var onLongPressMessage: ((ChatMessage) -> Void)? = nil
    var onDownloadMedia: ((String, String) -> Void)? = nil
    var onTapImage: ((ChatMediaAttachment) -> Void)? = nil

    @State private var isBeingPressed = false

    private let roundedCornerRadius: CGFloat = 16
    private let groupedCornerRadius: CGFloat = 6

    private let reactionChipOverlap: CGFloat = 10

    private var hasMedia: Bool { !message.media.isEmpty }
    private var hasText: Bool {
        !message.displayContent.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        let hasReactions = !message.reactions.isEmpty
        let segments = parseMessageSegments(message.displayContent, htmlState: message.htmlState)

        VStack(alignment: message.isMine ? .trailing : .leading, spacing: 0) {
            if let replyToId = message.replyToMessageId {
                ReplyPreviewCard(
                    replyToMessageId: replyToId,
                    target: replyTargetsById[replyToId],
                    isMine: message.isMine,
                    onTap: onJumpToMessage
                )
                .padding(.bottom, 3)
            }

            if hasMedia {
                mediaBubble(segments: segments)
            } else {
                ForEach(segments) { segment in
                    switch segment {
                    case .markdown(let text):
                        markdownBubble(text: text)
                    case .pikaPrompt(let prompt):
                        PikaPromptView(prompt: prompt, message: message, onSelect: {
                            onSendMessage($0, nil)
                        })
                    case .pikaHtml(_, let html, let state):
                        PikaHtmlView(html: html, htmlState: state, onSendMessage: {
                            onSendMessage($0, nil)
                        })
                    }
                }
            }

            EmptyView()
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

            if hasReactions {
                Spacer().frame(height: reactionChipOverlap + 4)
            }
        }
        .contentShape(Rectangle())
        .scaleEffect(isBeingPressed ? 0.96 : 1.0)
        .animation(.spring(response: 0.25, dampingFraction: 0.7), value: isBeingPressed)
        .onLongPressGesture(minimumDuration: 0.3, maximumDistance: 44) {
            handleLongPress()
        } onPressingChanged: { pressing in
            isBeingPressed = pressing
        }
        .opacity(activeReactionMessageId == message.id ? 0 : 1)
        .animation(.easeInOut(duration: 0.15), value: activeReactionMessageId == message.id)
    }

    @ViewBuilder
    private func mediaBubble(segments: [MessageSegment]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(message.media, id: \.originalHashHex) { attachment in
                MediaAttachmentView(
                    attachment: attachment,
                    isMine: message.isMine,
                    onDownload: {
                        onDownloadMedia?(message.id, attachment.originalHashHex)
                    },
                    onTapImage: {
                        onTapImage?(attachment)
                    }
                )
                .overlay(alignment: .bottomTrailing) {
                    if !hasText {
                        Text(timestampText)
                            .font(.caption2)
                            .foregroundStyle(.white.opacity(0.78))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 4)
                            .background(.black.opacity(0.4), in: Capsule())
                            .padding(6)
                    }
                }
            }

            if hasText {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(segments) { segment in
                        if case .markdown(let text) = segment {
                            Markdown(text)
                                .markdownTheme(message.isMine ? .pikaOutgoing : .pikaIncoming)
                                .multilineTextAlignment(.leading)
                        }
                    }
                    Text(timestampText)
                        .font(.caption2)
                        .foregroundStyle(message.isMine ? Color.white.opacity(0.78) : Color.secondary.opacity(0.9))
                }
                .padding(.horizontal, 12)
                .padding(.top, 6)
                .padding(.bottom, 6)
            }
        }
        .background(hasText ? (message.isMine ? Color.blue : Color.gray.opacity(0.2)) : Color.clear)
        .clipShape(UnevenRoundedRectangle(cornerRadii: bubbleRadii, style: .continuous))
    }

    private func markdownBubble(text: String) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Markdown(text)
                .markdownTheme(message.isMine ? .pikaOutgoing : .pikaIncoming)
                .multilineTextAlignment(.leading)

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

    private func handleLongPress() {
        guard onLongPressMessage != nil else { return }
        let impactFeedback = UIImpactFeedbackGenerator(style: .medium)
        impactFeedback.impactOccurred()
        onLongPressMessage?(message)
    }
}

// MARK: - Reply preview

private struct ReplyPreviewCard: View {
    let replyToMessageId: String
    let target: ChatMessage?
    let isMine: Bool
    var onTap: ((String) -> Void)? = nil

    var body: some View {
        Group {
            if target != nil {
                Button {
                    onTap?(replyToMessageId)
                } label: {
                    content
                }
                .buttonStyle(.plain)
            } else {
                content
            }
        }
    }

    private var content: some View {
        HStack(spacing: 8) {
            Rectangle()
                .fill(isMine ? Color.white.opacity(0.8) : Color.blue.opacity(0.9))
                .frame(width: 2)
            VStack(alignment: .leading, spacing: 2) {
                Text(senderLabel)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(isMine ? Color.white.opacity(0.86) : Color.secondary)
                    .lineLimit(1)
                Text(snippet)
                    .font(.caption)
                    .foregroundStyle(isMine ? Color.white.opacity(0.8) : Color.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(
            isMine ? Color.white.opacity(0.14) : Color.black.opacity(0.08),
            in: RoundedRectangle(cornerRadius: 10, style: .continuous)
        )
    }

    private var senderLabel: String {
        guard let target else { return "Original message" }
        if target.isMine {
            return "You"
        }
        if let name = target.senderName, !name.isEmpty {
            return name
        }
        return String(target.senderPubkey.prefix(8))
    }

    private var snippet: String {
        guard let target else { return "Original message not loaded" }
        let trimmed = target.displayContent.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            return "(empty message)"
        }
        if let first = trimmed.split(separator: "\n").first {
            let text = String(first)
            if text.count > 80 {
                return String(text.prefix(80)) + "…"
            }
            return text
        }
        if trimmed.count > 80 {
            return String(trimmed.prefix(80)) + "…"
        }
        return trimmed
    }
}

func deliveryText(_ d: MessageDeliveryState) -> String {
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
    let htmlState: String?
    let onSendMessage: @MainActor (String) -> Void

    @State private var contentHeight: CGFloat = 100
    @State private var showFullScreen = false

    var body: some View {
        PikaWebView(html: html, htmlState: htmlState, contentHeight: $contentHeight, onSendMessage: onSendMessage, interactive: false)
            .frame(height: min(contentHeight, 400))
            .clipShape(RoundedRectangle(cornerRadius: 16))
            .background(Color.gray.opacity(0.1))
            .clipShape(RoundedRectangle(cornerRadius: 16))
            .contentShape(Rectangle())
            .onTapGesture { showFullScreen = true }
            .fullScreenCover(isPresented: $showFullScreen) {
                PikaHtmlFullScreen(html: html, htmlState: htmlState, onSendMessage: onSendMessage, isPresented: $showFullScreen)
            }
    }
}

private struct PikaHtmlFullScreen: View {
    let html: String
    let htmlState: String?
    let onSendMessage: @MainActor (String) -> Void
    @Binding var isPresented: Bool

    var body: some View {
        NavigationStack {
            PikaFullScreenWebView(html: html, htmlState: htmlState, onSendMessage: onSendMessage)
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
    let htmlState: String?
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

        if let state = htmlState {
            context.coordinator.pendingState = state
        }

        let finalHtml: String
        let trimmed = html.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("<!") || trimmed.lowercased().hasPrefix("<html") {
            finalHtml = html
        } else {
            finalHtml = """
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
        }
        webView.loadHTMLString(finalHtml, baseURL: webViewBaseURL)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        guard let state = htmlState, state != context.coordinator.lastInjectedState else { return }
        if !context.coordinator.pageLoaded {
            context.coordinator.pendingState = state
            return
        }
        context.coordinator.lastInjectedState = state
        let escaped = state.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "'", with: "\\'")
            .replacingOccurrences(of: "\n", with: "\\n")
        webView.evaluateJavaScript("window.pikaState && window.pikaState(JSON.parse('\(escaped)'))")
    }
}

struct PikaWebView: UIViewRepresentable {
    let html: String
    let htmlState: String?
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

        if let state = htmlState {
            context.coordinator.pendingState = state
        }

        let binding = $contentHeight
        context.coordinator.onHeightChange = { height in
            Task { @MainActor in
                binding.wrappedValue = height
            }
        }
        let debugOverlay = """
        <div id="_pika_dbg" style="display:none;position:fixed;top:0;left:0;right:0;padding:4px 8px;font:16px monospace;color:#f44;background:rgba(0,0,0,0.8);z-index:99999;pointer-events:none"></div>
        <script>
        var _d=document.getElementById('_pika_dbg');
        window.onerror=function(m,u,l){_d.style.display='block';_d.textContent='ERR: '+m+' ('+u+':'+l+')';};
        window.addEventListener('unhandledrejection',function(e){_d.style.display='block';_d.textContent='REJECT: '+e.reason;});
        </script>
        """
        let finalHtml: String
        let trimmed = html.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("<!") || trimmed.lowercased().hasPrefix("<html") {
            if let range = html.range(of: "</body>", options: .caseInsensitive) {
                finalHtml = html[html.startIndex..<range.lowerBound] + debugOverlay + html[range.lowerBound...]
            } else {
                finalHtml = html + debugOverlay
            }
        } else {
            finalHtml = """
            <!DOCTYPE html>
            <html>
            <head>
            <meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1">
            <style>
            :root { color-scheme: light dark; }
            body { margin: 8px; font-family: -apple-system, sans-serif; background: transparent; }
            </style>
            </head>
            <body>\(html)\(debugOverlay)</body>
            </html>
            """
        }
        webView.loadHTMLString(finalHtml, baseURL: webViewBaseURL)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        guard let state = htmlState, state != context.coordinator.lastInjectedState else { return }
        if !context.coordinator.pageLoaded {
            // Page still loading — stash for didFinish to inject.
            context.coordinator.pendingState = state
            return
        }
        context.coordinator.lastInjectedState = state
        let escaped = state.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "'", with: "\\'")
            .replacingOccurrences(of: "\n", with: "\\n")
        webView.evaluateJavaScript("window.pikaState && window.pikaState(JSON.parse('\(escaped)'))")
    }

    class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
        let onSendMessage: @MainActor (String) -> Void
        var onHeightChange: ((CGFloat) -> Void)?
        var lastInjectedState: String?
        var pendingState: String?
        var pageLoaded = false

        init(onSendMessage: @escaping @MainActor (String) -> Void) {
            self.onSendMessage = onSendMessage
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

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            pageLoaded = true
            // Measure content height once after load to size the frame without
            // a continuous observer that causes layout feedback loops.
            webView.evaluateJavaScript("document.documentElement.scrollHeight") { [weak self] result, _ in
                if let height = result as? CGFloat, height > 0 {
                    self?.onHeightChange?(height)
                }
            }
            // Inject pending state after initial page load (handles case where
            // updateUIView fires before the page is ready).
            if let state = pendingState {
                pendingState = nil
                lastInjectedState = state
                let escaped = state.replacingOccurrences(of: "\\", with: "\\\\")
                    .replacingOccurrences(of: "'", with: "\\'")
                    .replacingOccurrences(of: "\n", with: "\\n")
                webView.evaluateJavaScript("window.pikaState && window.pikaState(JSON.parse('\(escaped)'))")
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
