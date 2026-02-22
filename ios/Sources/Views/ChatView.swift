import SwiftUI
import MarkdownUI
import PhotosUI

struct ChatView: View {
    let chatId: String
    let state: ChatScreenState
    let activeCall: CallState?
    let callEvents: [CallTimelineEvent]
    let onSendMessage: @MainActor (String, String?) -> Void
    let onStartCall: @MainActor () -> Void
    let onStartVideoCall: @MainActor () -> Void
    let onOpenCallScreen: @MainActor () -> Void
    let onGroupInfo: (@MainActor () -> Void)?
    let onTapSender: (@MainActor (String) -> Void)?
    let onReact: (@MainActor (String, String) -> Void)?
    let onTypingStarted: (@MainActor () -> Void)?
    let onDownloadMedia: (@MainActor (String, String, String) -> Void)?
    let onSendMedia: (@MainActor (String, Data, String, String, String) -> Void)?
    @State private var selectedPhotoItem: PhotosPickerItem?
    @State private var messageText = ""
    @State private var isAtBottom = true
    @State private var shouldStickToBottom = true
    @State private var isUserScrolling = false
    @State private var activeReactionMessageId: String?
    @State private var contextMenuMessage: ChatMessage?
    @State private var showContextActionCard = false
    @State private var showContextEmojiPicker = false
    @State private var showMentionPicker = false
    @State private var mentionQuery = ""
    @State private var insertedMentions: [(display: String, npub: String)] = []
    @State private var replyDraftMessage: ChatMessage?
    @State private var fullscreenImageAttachment: ChatMediaAttachment?
    @FocusState private var isInputFocused: Bool

    private let scrollButtonBottomPadding: CGFloat = 12
    private let bottomVisibilityTolerance: CGFloat = 100
    private let bottomAnchorId = "bottom-anchor"

    init(
        chatId: String,
        state: ChatScreenState,
        activeCall: CallState?,
        callEvents: [CallTimelineEvent],
        onSendMessage: @escaping @MainActor (String, String?) -> Void,
        onStartCall: @escaping @MainActor () -> Void,
        onStartVideoCall: @escaping @MainActor () -> Void,
        onOpenCallScreen: @escaping @MainActor () -> Void,
        onGroupInfo: (@MainActor () -> Void)? = nil,
        onTapSender: (@MainActor (String) -> Void)? = nil,
        onReact: (@MainActor (String, String) -> Void)? = nil,
        onTypingStarted: (@MainActor () -> Void)? = nil,
        onDownloadMedia: (@MainActor (String, String, String) -> Void)? = nil,
        onSendMedia: (@MainActor (String, Data, String, String, String) -> Void)? = nil
    ) {
        self.chatId = chatId
        self.state = state
        self.activeCall = activeCall
        self.callEvents = callEvents
        self.onSendMessage = onSendMessage
        self.onStartCall = onStartCall
        self.onStartVideoCall = onStartVideoCall
        self.onOpenCallScreen = onOpenCallScreen
        self.onGroupInfo = onGroupInfo
        self.onTapSender = onTapSender
        self.onReact = onReact
        self.onTypingStarted = onTypingStarted
        self.onDownloadMedia = onDownloadMedia
        self.onSendMedia = onSendMedia
    }

    var body: some View {
        if let chat = state.chat, chat.chatId == chatId {
            loadedChat(chat)
        } else {
            loadingView
        }
    }

    @ViewBuilder
    private func loadedChat(_ chat: ChatViewState) -> some View {
        VStack(spacing: 8) {
            if let liveCall = callFor(chat), liveCall.isLive {
                ActiveCallPill(
                    call: liveCall,
                    peerName: chatTitle(chat),
                    onTap: {
                        onOpenCallScreen()
                    }
                )
                .padding(.horizontal, 12)
                .padding(.top, 2)
            }
            messageList(chat)
        }
        .modifier(FloatingInputBarModifier(content: { messageInputBar(chat: chat) }))
        .blur(radius: contextMenuMessage == nil ? 0 : 24)
        .allowsHitTesting(contextMenuMessage == nil)
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

                ToolbarItem(placement: .topBarTrailing) {
                    ChatCallToolbarButton(
                        callForChat: callFor(chat),
                        hasLiveCallElsewhere: hasLiveCallElsewhere(chat: chat),
                        onStartCall: {
                            onStartCall()
                        },
                        onStartVideoCall: {
                            onStartVideoCall()
                        },
                        onOpenCallScreen: {
                            onOpenCallScreen()
                        }
                    )
                }
            }
        }
        .overlay {
            if let message = contextMenuMessage {
                GeometryReader { geo in
                    ZStack {
                        Color.clear
                            .contentShape(Rectangle())
                            .ignoresSafeArea()
                            .onTapGesture {
                                withAnimation(.easeOut(duration: 0.2)) {
                                    contextMenuMessage = nil
                                    activeReactionMessageId = nil
                                    showContextActionCard = false
                                }
                            }

                        VStack(alignment: message.isMine ? .trailing : .leading, spacing: 12) {
                            QuickReactionBar(
                                onSelect: { emoji in
                                    onReact?(message.id, emoji)
                                    withAnimation(.easeOut(duration: 0.18)) {
                                        contextMenuMessage = nil
                                        activeReactionMessageId = nil
                                        showContextActionCard = false
                                    }
                                },
                                onMore: {
                                    withAnimation(.easeOut(duration: 0.18)) {
                                        showContextActionCard = false
                                    }
                                    showContextEmojiPicker = true
                                },
                                onActions: {
                                    withAnimation(.easeOut(duration: 0.18)) {
                                        showContextActionCard.toggle()
                                    }
                                }
                            )

                            FocusedMessageCard(
                                message: message,
                                maxWidth: min(geo.size.width * 0.82, 360),
                                maxHeight: geo.size.height * 0.4
                            )

                            if showContextActionCard {
                                MessageActionCard(
                                    onCopy: {
                                        UIPasteboard.general.string = message.displayContent
                                        withAnimation(.easeOut(duration: 0.15)) {
                                            contextMenuMessage = nil
                                            activeReactionMessageId = nil
                                            showContextActionCard = false
                                        }
                                    },
                                    onReply: {
                                        replyDraftMessage = message
                                        isInputFocused = true
                                        withAnimation(.easeOut(duration: 0.15)) {
                                            contextMenuMessage = nil
                                            activeReactionMessageId = nil
                                            showContextActionCard = false
                                        }
                                    },
                                    onSaveMedia: message.media.first(where: {
                                        $0.mimeType.hasPrefix("image/") && $0.localPath != nil
                                    }) != nil ? {
                                        for attachment in message.media {
                                            guard attachment.mimeType.hasPrefix("image/"),
                                                  let path = attachment.localPath,
                                                  let image = UIImage(contentsOfFile: path)
                                            else { continue }
                                            UIImageWriteToSavedPhotosAlbum(image, nil, nil, nil)
                                        }
                                        withAnimation(.easeOut(duration: 0.15)) {
                                            contextMenuMessage = nil
                                            activeReactionMessageId = nil
                                            showContextActionCard = false
                                        }
                                    } : nil
                                )
                            }
                        }
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: message.isMine ? .topTrailing : .topLeading)
                        .padding(.top, max(geo.safeAreaInsets.top + 14, 34))
                        .padding(.horizontal, 20)
                        .padding(.bottom, 24)
                    }
                }
                .transition(.opacity.combined(with: .scale(scale: 0.95, anchor: .top)))
            }
        }
        .sheet(isPresented: $showContextEmojiPicker) {
            if let message = contextMenuMessage {
                EmojiPickerSheet { emoji in
                    onReact?(message.id, emoji)
                    showContextEmojiPicker = false
                    withAnimation(.easeOut(duration: 0.18)) {
                        contextMenuMessage = nil
                        activeReactionMessageId = nil
                        showContextActionCard = false
                    }
                }
                .presentationDetents([.medium, .large])
            }
        }
        .fullScreenCover(item: $fullscreenImageAttachment, onDismiss: nil) { attachment in
            FullscreenImageViewer(attachment: attachment)
        }
    }

    @ViewBuilder
    private func messageList(_ chat: ChatViewState) -> some View {
        let messagesById = Dictionary(uniqueKeysWithValues: chat.messages.map { ($0.id, $0) })
        GeometryReader { scrollGeo in
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 0) {
                        LazyVStack(spacing: 8) {
                            ForEach(timelineRows(chat)) { row in
                                switch row {
                                case .messageGroup(let group):
                                    MessageGroupRow(
                                        group: group,
                                        showSender: chat.isGroup,
                                        onSendMessage: onSendMessage,
                                        replyTargetsById: messagesById,
                                        onTapSender: onTapSender,
                                        onJumpToMessage: { messageId in
                                            withAnimation(.easeOut(duration: 0.2)) {
                                                proxy.scrollTo(messageId, anchor: .center)
                                            }
                                        },
                                        onReact: onReact,
                                        activeReactionMessageId: $activeReactionMessageId,
                                        onLongPressMessage: { message in
                                            isInputFocused = false
                                            withAnimation(.spring(response: 0.3, dampingFraction: 0.78)) {
                                                activeReactionMessageId = message.id
                                                contextMenuMessage = message
                                                showContextActionCard = true
                                            }
                                        },
                                        onDownloadMedia: onDownloadMedia.map { callback in
                                            { messageId, hash in callback(chatId, messageId, hash) }
                                        },
                                        onTapImage: { attachment in
                                            fullscreenImageAttachment = attachment
                                        }
                                    )
                                case .callEvent(let event):
                                    CallTimelineEventRow(event: event)
                                }
                            }

                            if !chat.typingMembers.isEmpty {
                                TypingIndicatorRow(
                                    typingMembers: chat.typingMembers,
                                    members: chat.members
                                )
                                .transition(.opacity.combined(with: .move(edge: .bottom)))
                            }
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 10)
                        .animation(.easeInOut(duration: 0.2), value: chat.typingMembers.map(\.pubkey))

                        GeometryReader { geo in
                            Color.clear.preference(
                                key: BottomVisibleKey.self,
                                value: geo.frame(in: .named("chatScroll")).minY
                            )
                        }
                        .frame(height: 1)
                        .id(bottomAnchorId)
                    }
                }
                .scrollDismissesKeyboard(.interactively)
                .coordinateSpace(name: "chatScroll")
                .defaultScrollAnchor(.bottom)
                .simultaneousGesture(
                    DragGesture(minimumDistance: 1)
                        .onChanged { _ in
                            if !isUserScrolling {
                                isUserScrolling = true
                            }
                        }
                        .onEnded { _ in
                            isUserScrolling = false
                        }
                )
                .onDisappear {
                    isUserScrolling = false
                }
                .onPreferenceChange(BottomVisibleKey.self) { minY in
                    guard let minY else { return }
                    let isNearBottom = minY < scrollGeo.size.height + bottomVisibilityTolerance
                    if isAtBottom != isNearBottom {
                        isAtBottom = isNearBottom
                    }
                    // Only user-initiated scrolling can disable sticky mode.
                    if isNearBottom {
                        if !shouldStickToBottom {
                            shouldStickToBottom = true
                        }
                    } else if isUserScrolling, shouldStickToBottom {
                        shouldStickToBottom = false
                    }
                }
                .onChange(of: chat.messages.last?.id) { oldMessageId, newMessageId in
                    guard newMessageId != oldMessageId else { return }
                    guard shouldStickToBottom else { return }
                    scrollToBottom(using: proxy, animated: true)
                }
                .onChange(of: chat.chatId) { _, _ in
                    shouldStickToBottom = true
                    scrollToBottom(using: proxy, animated: false)
                }
                .onAppear {
                    if shouldStickToBottom {
                        scrollToBottom(using: proxy, animated: false)
                    }
                }
                .overlay(alignment: .bottomTrailing) {
                    if !isAtBottom {
                        Button {
                            shouldStickToBottom = true
                            scrollToBottom(using: proxy, animated: true)
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
        }
    }

    private func scrollToBottom(using proxy: ScrollViewProxy, animated: Bool) {
        DispatchQueue.main.async {
            if animated {
                withAnimation(.easeOut(duration: 0.2)) {
                    proxy.scrollTo(bottomAnchorId, anchor: .bottom)
                }
            } else {
                proxy.scrollTo(bottomAnchorId, anchor: .bottom)
            }
        }
    }

    private var loadingView: some View {
        VStack(spacing: 10) {
            ProgressView()
            Text("Loading chat...")
                .foregroundStyle(.secondary)
        }
    }

    private func chatTitle(_ chat: ChatViewState) -> String {
        if chat.isGroup {
            return chat.groupName ?? "Group"
        }
        return chat.members.first?.name ?? chat.members.first?.npub ?? ""
    }

    private func timelineRows(_ chat: ChatViewState) -> [ChatTimelineRow] {
        var entries: [ChatTimelineEntry] = []
        entries.reserveCapacity(chat.messages.count + callEvents.count)
        entries.append(contentsOf: chat.messages.enumerated().map { index, message in
            .message(index: index, message: message)
        })
        entries.append(contentsOf: callEvents.enumerated().map { index, event in
            .callEvent(index: index, event: event)
        })
        entries.sort {
            let lhsTimestamp = $0.timestamp.timeIntervalSince1970
            let rhsTimestamp = $1.timestamp.timeIntervalSince1970
            if lhsTimestamp == rhsTimestamp {
                if $0.tieBreak == $1.tieBreak {
                    return $0.order < $1.order
                }
                return $0.tieBreak < $1.tieBreak
            }
            return lhsTimestamp < rhsTimestamp
        }

        let membersByPubkey = Dictionary(uniqueKeysWithValues: chat.members.map { ($0.pubkey, $0) })
        var rows: [ChatTimelineRow] = []
        rows.reserveCapacity(entries.count)

        for entry in entries {
            switch entry {
            case .callEvent(_, let event):
                rows.append(.callEvent(event))
            case .message(_, let message):
                if let lastIndex = rows.indices.last,
                   case .messageGroup(var group) = rows[lastIndex],
                   group.senderPubkey == message.senderPubkey,
                   group.isMine == message.isMine {
                    group.messages.append(message)
                    rows[lastIndex] = .messageGroup(group)
                    continue
                }

                let member = membersByPubkey[message.senderPubkey]
                rows.append(
                    .messageGroup(
                        GroupedChatMessage(
                            senderPubkey: message.senderPubkey,
                            senderName: message.senderName ?? member?.name,
                            senderNpub: member?.npub ?? message.senderPubkey,
                            senderPictureUrl: member?.pictureUrl,
                            isMine: message.isMine,
                            messages: [message]
                        )
                    )
                )
            }
        }

        return rows
    }

    private enum ChatTimelineEntry {
        case message(index: Int, message: ChatMessage)
        case callEvent(index: Int, event: CallTimelineEvent)

        var timestamp: Date {
            switch self {
            case .message(_, let message):
                return Date(timeIntervalSince1970: TimeInterval(message.timestamp))
            case .callEvent(_, let event):
                return event.date
            }
        }

        var tieBreak: Int {
            switch self {
            case .callEvent:
                return 0
            case .message:
                return 1
            }
        }

        var order: Int {
            switch self {
            case .message(let index, _):
                return index
            case .callEvent(let index, _):
                return index
            }
        }
    }

    private enum ChatTimelineRow: Identifiable {
        case messageGroup(GroupedChatMessage)
        case callEvent(CallTimelineEvent)

        var id: String {
            switch self {
            case .messageGroup(let group):
                return "msg:\(group.id)"
            case .callEvent(let event):
                return "call:\(event.id)"
            }
        }
    }

    private func sendMessage() {
        let trimmed = messageText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        var wire = trimmed
        for mention in insertedMentions {
            wire = wire.replacingOccurrences(of: mention.display, with: "nostr:\(mention.npub)")
        }
        onSendMessage(wire, replyDraftMessage?.id)
        messageText = ""
        insertedMentions = []
        replyDraftMessage = nil
    }

    private func callFor(_ chat: ChatViewState) -> CallState? {
        guard activeCall?.chatId == chat.chatId else { return nil }
        return activeCall
    }

    private func hasLiveCallElsewhere(chat: ChatViewState) -> Bool {
        guard let activeCall else { return false }
        return activeCall.chatId != chat.chatId && activeCall.isLive
    }

    private func replySenderLabel(_ message: ChatMessage) -> String {
        if message.isMine {
            return "You"
        }
        if let name = message.senderName, !name.isEmpty {
            return name
        }
        return String(message.senderPubkey.prefix(8))
    }

    private func replySnippet(_ message: ChatMessage) -> String {
        let trimmed = message.displayContent.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return "(empty message)" }
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

            if let replying = replyDraftMessage {
                HStack(spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Replying to \(replySenderLabel(replying))")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.secondary)
                        Text(replySnippet(replying))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    Spacer()
                    Button {
                        replyDraftMessage = nil
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(.body)
                            .foregroundStyle(.tertiary)
                    }
                    .buttonStyle(.plain)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(.ultraThinMaterial)
                .overlay(alignment: .leading) {
                    Rectangle()
                        .fill(Color.blue)
                        .frame(width: 3)
                }
                .padding(.horizontal, 12)
            }

            HStack(spacing: 10) {
                if onSendMedia != nil {
                    PhotosPicker(selection: $selectedPhotoItem, matching: .images) {
                        Image(systemName: "plus.circle.fill")
                            .font(.title2)
                    }
                    .tint(.secondary)
                }

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
                        if !newValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                            onTypingStarted?()
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
            .onChange(of: selectedPhotoItem) { _, item in
                guard let item else { return }
                Task {
                    defer { selectedPhotoItem = nil }
                    guard let data = try? await item.loadTransferable(type: Data.self) else { return }

                    // Determine MIME type from supported content types
                    let mimeType: String
                    if let contentType = item.supportedContentTypes.first {
                        mimeType = contentType.preferredMIMEType ?? "image/jpeg"
                    } else {
                        mimeType = "image/jpeg"
                    }

                    // Determine filename from MIME type
                    let ext = switch mimeType {
                    case "image/png": "png"
                    case "image/gif": "gif"
                    case "image/webp": "webp"
                    case "image/heic": "heic"
                    default: "jpg"
                    }
                    let filename = "photo.\(ext)"

                    // Use message text as caption if non-empty
                    let caption = messageText.trimmingCharacters(in: .whitespacesAndNewlines)
                    if !caption.isEmpty {
                        messageText = ""
                    }

                    onSendMedia?(chatId, data, mimeType, filename, caption)
                }
            }
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
    let onActions: () -> Void

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
            Button {
                onActions()
            } label: {
                Image(systemName: "ellipsis")
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
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(TestIds.chatReactionBar)
    }
}

private struct MessageActionCard: View {
    let onCopy: () -> Void
    let onReply: () -> Void
    var onSaveMedia: (() -> Void)? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Button {
                onReply()
            } label: {
                Label("Reply", systemImage: "arrowshape.turn.up.left")
                    .font(.body.weight(.medium))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.plain)

            Button {
                onCopy()
            } label: {
                Label("Copy", systemImage: "doc.on.doc")
                    .font(.body.weight(.medium))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier(TestIds.chatActionCopy)

            if let onSaveMedia {
                Button {
                    onSaveMedia()
                } label: {
                    Label("Save Photo", systemImage: "square.and.arrow.down")
                        .font(.body.weight(.medium))
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(14)
        .frame(width: 220, alignment: .leading)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .shadow(color: .black.opacity(0.18), radius: 10, y: 6)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(TestIds.chatActionCard)
    }
}

private struct FocusedMessageCard: View {
    let message: ChatMessage
    let maxWidth: CGFloat
    let maxHeight: CGFloat

    private var hasMedia: Bool { !message.media.isEmpty }
    private var hasText: Bool {
        !message.displayContent.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(alignment: message.isMine ? .trailing : .leading, spacing: 0) {
            if hasMedia {
                ForEach(message.media, id: \.originalHashHex) { attachment in
                    MediaAttachmentView(
                        attachment: attachment,
                        isMine: message.isMine,
                        maxMediaWidth: maxWidth,
                        maxMediaHeight: maxHeight
                    )
                    .overlay(alignment: .bottomTrailing) {
                        if !hasText {
                            Text(Date(timeIntervalSince1970: TimeInterval(message.timestamp)).formatted(date: .omitted, time: .shortened))
                                .font(.caption2)
                                .foregroundStyle(.white.opacity(0.78))
                                .padding(.horizontal, 8)
                                .padding(.vertical, 4)
                                .background(.black.opacity(0.4), in: Capsule())
                                .padding(6)
                        }
                    }
                }
            }

            if hasText || !hasMedia {
                VStack(alignment: message.isMine ? .trailing : .leading, spacing: 6) {
                    if hasText {
                        if isLikelyLongMessage {
                            ScrollView(showsIndicators: false) {
                                markdownContent
                            }
                            .frame(maxHeight: maxHeight)
                        } else {
                            markdownContent
                        }
                    }

                    Text(Date(timeIntervalSince1970: TimeInterval(message.timestamp)).formatted(date: .omitted, time: .shortened))
                        .font(.caption2)
                        .foregroundStyle(message.isMine ? Color.white.opacity(0.78) : Color.secondary.opacity(0.9))
                }
                .padding(.horizontal, 12)
                .padding(.top, hasMedia && hasText ? 6 : 8)
                .padding(.bottom, 6)
            }
        }
        .background(hasMedia && !hasText ? Color.clear : (message.isMine ? Color.blue : Color(uiColor: .systemGray5)))
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .frame(maxWidth: maxWidth, alignment: message.isMine ? .trailing : .leading)
    }

    private var markdownContent: some View {
        Markdown(message.displayContent)
            .markdownTheme(message.isMine ? .pikaOutgoing : .pikaIncoming)
            .multilineTextAlignment(.leading)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var isLikelyLongMessage: Bool {
        let lineCount = message.displayContent.split(whereSeparator: \.isNewline).count
        return message.displayContent.count > 240 || lineCount > 6
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
        #if compiler(>=6.2)
        if #available(iOS 26.0, *) {
            content
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .glassEffect(.regular.interactive(), in: RoundedRectangle(cornerRadius: 20))
                .padding(12)
        } else {
            content
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 20))
                .padding(12)
        }
        #else
        content
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 20))
            .padding(12)
        #endif
    }
}

private struct FloatingInputBarModifier<Bar: View>: ViewModifier {
    @ViewBuilder var content: Bar

    func body(content view: Content) -> some View {
        #if compiler(>=6.2)
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
        #else
        view.safeAreaInset(edge: .bottom) {
            VStack(spacing: 0) {
                Divider()
                content
            }
        }
        #endif
    }
}

// Extracted to MessageBubbleViews.swift:
// PikaPrompt, MessageSegment, parseMessageSegments,
// GroupedChatMessage, GroupedBubblePosition,
// MessageGroupRow, MessageBubbleStack, MessageBubble,
// MediaAttachmentView, ReplyPreviewCard, deliveryText,
// PikaPromptView, PikaHtmlView, PikaWebView, Theme extensions

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
                replyToMessageId: nil,
                mentions: [],
                timestamp: 1_709_100_001,
                isMine: false,
                delivery: .sent,
                reactions: [],
                media: [],
                pollTally: [],
                myPollVote: nil,
                htmlState: nil
            ),
            ChatMessage(
                id: "incoming-2",
                senderPubkey: PreviewAppState.samplePeerPubkey,
                senderName: "Anthony",
                content: "Second message should visually join with the first.",
                displayContent: "Second message should visually join with the first.",
                replyToMessageId: nil,
                mentions: [],
                timestamp: 1_709_100_002,
                isMine: false,
                delivery: .sent,
                reactions: [],
                media: [],
                pollTally: [],
                myPollVote: nil,
                htmlState: nil
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
                replyToMessageId: nil,
                mentions: [],
                timestamp: 1_709_100_010,
                isMine: true,
                delivery: .sent,
                reactions: [],
                media: [],
                pollTally: [],
                myPollVote: nil,
                htmlState: nil
            ),
            ChatMessage(
                id: "outgoing-2",
                senderPubkey: PreviewAppState.samplePubkey,
                senderName: nil,
                content: "If you're near ana's market, I'll find you.",
                displayContent: "If you're near ana's market, I'll find you.",
                replyToMessageId: nil,
                mentions: [],
                timestamp: 1_709_100_011,
                isMine: true,
                delivery: .pending,
                reactions: [],
                media: [],
                pollTally: [],
                myPollVote: nil,
                htmlState: nil
            ),
        ]
    )
}

#Preview("Chat") {
    NavigationStack {
        ChatView(
            chatId: "chat-1",
            state: ChatScreenState(chat: PreviewAppState.chatDetail.currentChat),
            activeCall: nil,
            callEvents: [],
            onSendMessage: { _, _ in },
            onStartCall: {},
            onStartVideoCall: {},
            onOpenCallScreen: {}
        )
    }
}

#Preview("Chat - Failed") {
    NavigationStack {
        ChatView(
            chatId: "chat-1",
            state: ChatScreenState(chat: PreviewAppState.chatDetailFailed.currentChat),
            activeCall: nil,
            callEvents: [],
            onSendMessage: { _, _ in },
            onStartCall: {},
            onStartVideoCall: {},
            onOpenCallScreen: {}
        )
    }
}

#Preview("Chat - Empty") {
    NavigationStack {
        ChatView(
            chatId: "chat-empty",
            state: ChatScreenState(chat: PreviewAppState.chatDetailEmpty.currentChat),
            activeCall: nil,
            callEvents: [],
            onSendMessage: { _, _ in },
            onStartCall: {},
            onStartVideoCall: {},
            onOpenCallScreen: {}
        )
    }
}

#Preview("Chat - Long Thread") {
    NavigationStack {
        ChatView(
            chatId: "chat-long",
            state: ChatScreenState(chat: PreviewAppState.chatDetailLongThread.currentChat),
            activeCall: nil,
            callEvents: [],
            onSendMessage: { _, _ in },
            onStartCall: {},
            onStartVideoCall: {},
            onOpenCallScreen: {}
        )
    }
}

#Preview("Chat - Grouped") {
    NavigationStack {
        ChatView(
            chatId: "chat-grouped",
            state: ChatScreenState(chat: PreviewAppState.chatDetailGrouped.currentChat),
            activeCall: nil,
            callEvents: [],
            onSendMessage: { _, _ in },
            onStartCall: {},
            onStartVideoCall: {},
            onOpenCallScreen: {}
        )
    }
}

#Preview("Chat - Media") {
    NavigationStack {
        ChatView(
            chatId: "chat-media",
            state: ChatScreenState(chat: PreviewAppState.chatDetailMedia.currentChat),
            activeCall: nil,
            callEvents: [],
            onSendMessage: { _, _ in },
            onStartCall: {},
            onStartVideoCall: {},
            onOpenCallScreen: {},
            onDownloadMedia: { chatId, messageId, hash in
                print("Download: \(chatId)/\(messageId)/\(hash)")
            }
        )
    }
}

#Preview("Message Group - Incoming") {
    MessageGroupRow(group: ChatViewPreviewData.incomingGroup, showSender: true, onSendMessage: { _, _ in }, replyTargetsById: [:], activeReactionMessageId: .constant(nil))
        .padding(16)
}

#Preview("Message Group - Outgoing") {
    MessageGroupRow(group: ChatViewPreviewData.outgoingGroup, showSender: true, onSendMessage: { _, _ in }, replyTargetsById: [:], activeReactionMessageId: .constant(nil))
        .padding(16)
}
#endif

// MARK: - Typing Indicator

private struct TypingIndicatorRow: View {
    let typingMembers: [TypingMember]
    let members: [MemberInfo]

    private let avatarSize: CGFloat = 24
    private let avatarGutterWidth: CGFloat = 28

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            if let first = typingMembers.first,
               let member = members.first(where: { $0.pubkey == first.pubkey }) {
                AvatarView(
                    name: member.name,
                    npub: member.npub,
                    pictureUrl: member.pictureUrl,
                    size: avatarSize
                )
                .frame(width: avatarGutterWidth, alignment: .leading)
            } else {
                Color.clear
                    .frame(width: avatarGutterWidth, height: avatarSize)
            }

            VStack(alignment: .leading, spacing: 3) {
                TypingBubble()
                Text(typingLabel)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }

            Spacer(minLength: 24)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var typingLabel: String {
        let names = typingMembers.compactMap { tm -> String? in
            if let n = tm.name, !n.isEmpty { return n }
            if let m = members.first(where: { $0.pubkey == tm.pubkey }) {
                return m.name ?? String(m.npub.prefix(8))
            }
            return String(tm.pubkey.prefix(8))
        }
        switch names.count {
        case 0: return ""
        case 1: return "\(names[0]) is typing"
        case 2: return "\(names[0]) and \(names[1]) are typing"
        default: return "\(names[0]) and \(names.count - 1) others are typing"
        }
    }
}

private struct TypingBubble: View {
    var body: some View {
        TimelineView(.animation) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            HStack(spacing: 4) {
                ForEach(0..<3, id: \.self) { i in
                    Circle()
                        .fill(Color.secondary.opacity(0.5))
                        .frame(width: 7, height: 7)
                        .offset(y: dotOffset(time: t, index: i))
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(Color(.systemGray5), in: RoundedRectangle(cornerRadius: 16))
    }

    private func dotOffset(time: Double, index: Int) -> CGFloat {
        let period: Double = 1.2
        let delay = Double(index) * 0.2
        let phase = (time + delay).truncatingRemainder(dividingBy: period) / period
        return -4 * sin(phase * 2 * .pi)
    }
}
