import SwiftUI
import UIKit

/// A UITableView-based inverted message list for chat.
///
/// The table is rotated 180° so the "bottom" (newest messages) is at the natural
/// scroll origin. Each cell's content is rotated back. This means older messages
/// are *appended* (not prepended) to the data source, so loading history never
/// causes scroll jumps.
struct InvertedMessageList: UIViewRepresentable {
    let rows: [ChatView.ChatTimelineRow]
    let chat: ChatViewState
    let messagesById: [String: ChatMessage]
    let isGroup: Bool

    // Callbacks
    let onSendMessage: @MainActor (String, String?) -> Void
    var onTapSender: (@MainActor (String) -> Void)?
    var onReact: (@MainActor (String, String) -> Void)?
    var onDownloadMedia: ((String, String) -> Void)?
    var onTapImage: ((ChatMediaAttachment) -> Void)?
    var onHypernoteAction: ((String, String, [String: String]) -> Void)?
    var onLongPressMessage: ((ChatMessage, CGRect) -> Void)?
    var onLoadOlderMessages: (() -> Void)?

    // Scroll state
    @Binding var isAtBottom: Bool
    @Binding var shouldStickToBottom: Bool
    var activeReactionMessageId: String?
    var scrollToBottomTrigger: Int

    private static let cellID = "cell"

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    func makeUIView(context: Context) -> UITableView {
        let tableView = UITableView(frame: .zero, style: .plain)
        tableView.transform = CGAffineTransform(scaleX: 1, y: -1)
        tableView.separatorStyle = .none
        tableView.backgroundColor = .clear
        tableView.showsVerticalScrollIndicator = true
        tableView.keyboardDismissMode = .interactive
        tableView.rowHeight = UITableView.automaticDimension
        tableView.estimatedRowHeight = 80
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: Self.cellID)
        tableView.dataSource = context.coordinator
        tableView.delegate = context.coordinator
        context.coordinator.tableView = tableView

        let invertedRows = buildInvertedRows()
        context.coordinator.invertedRows = invertedRows
        return tableView
    }

    func updateUIView(_ tableView: UITableView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self

        let newInverted = buildInvertedRows()
        let oldIDs = coordinator.invertedRows.map(\.id)
        let newIDs = newInverted.map(\.id)

        if oldIDs == newIDs {
            // Same structure — just reconfigure visible cells for content changes.
            coordinator.invertedRows = newInverted
            if let visible = tableView.indexPathsForVisibleRows, !visible.isEmpty {
                tableView.reconfigureRows(at: visible)
            }
        } else {
            // Compute simple diff.
            let oldSet = Set(oldIDs)
            let newSet = Set(newIDs)
            let insertedIndices = newIDs.enumerated().filter { !oldSet.contains($0.element) }
            let deletedIndices = oldIDs.enumerated().filter { !newSet.contains($0.element) }

            let totalChanges = insertedIndices.count + deletedIndices.count
            if totalChanges > 0 && totalChanges < max(oldIDs.count, newIDs.count) {
                coordinator.invertedRows = newInverted
                tableView.performBatchUpdates {
                    tableView.deleteRows(
                        at: deletedIndices.map { IndexPath(row: $0.offset, section: 0) },
                        with: .none
                    )
                    tableView.insertRows(
                        at: insertedIndices.map { IndexPath(row: $0.offset, section: 0) },
                        with: .none
                    )
                } completion: { _ in
                    if coordinator.parent.shouldStickToBottom {
                        coordinator.scrollToBottom(animated: true)
                    }
                }
            } else {
                coordinator.invertedRows = newInverted
                tableView.reloadData()
                if coordinator.parent.shouldStickToBottom {
                    coordinator.scrollToBottom(animated: false)
                }
            }
        }

        // Handle external scroll-to-bottom trigger.
        if scrollToBottomTrigger != coordinator.lastScrollToBottomTrigger {
            coordinator.lastScrollToBottomTrigger = scrollToBottomTrigger
            coordinator.scrollToBottom(animated: true)
        }
    }

    /// Build the inverted row array: reversed timeline rows, plus typing indicator at index 0.
    private func buildInvertedRows() -> [InvertedRow] {
        var result: [InvertedRow] = []

        // Typing indicator at index 0 (visually at the bottom of the chat).
        if !chat.typingMembers.isEmpty {
            result.append(.typing)
        }

        // Timeline rows in reverse (newest first for the inverted table).
        for row in rows.reversed() {
            result.append(.timeline(row))
        }

        return result
    }

    // MARK: - Coordinator

    final class Coordinator: NSObject, UITableViewDataSource, UITableViewDelegate {
        var parent: InvertedMessageList
        var invertedRows: [InvertedRow] = []
        weak var tableView: UITableView?
        var lastScrollToBottomTrigger: Int = 0
        private var requestedOldestId: String?

        init(parent: InvertedMessageList) {
            self.parent = parent
            self.lastScrollToBottomTrigger = parent.scrollToBottomTrigger
        }

        func scrollToBottom(animated: Bool) {
            guard let tableView, tableView.numberOfRows(inSection: 0) > 0 else { return }
            // In inverted table, row 0 = newest message = visual bottom.
            tableView.scrollToRow(at: IndexPath(row: 0, section: 0), at: .top, animated: animated)
        }

        // MARK: Data Source

        func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
            invertedRows.count
        }

        func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
            let cell = tableView.dequeueReusableCell(withIdentifier: InvertedMessageList.cellID, for: indexPath)
            cell.selectionStyle = .none
            cell.backgroundColor = .clear

            let row = invertedRows[indexPath.row]
            let parent = self.parent

            cell.contentConfiguration = UIHostingConfiguration {
                rowContent(for: row, parent: parent)
                    .scaleEffect(x: 1, y: -1, anchor: .center)
            }
            .minSize(width: 0, height: 0)
            .margins(.all, 0)

            return cell
        }

        // MARK: Delegate — Pagination

        func tableView(_ tableView: UITableView, willDisplay cell: UITableViewCell, forRowAt indexPath: IndexPath) {
            // Trigger pagination when displaying rows near the end (= oldest messages).
            let rowCount = invertedRows.count
            guard rowCount > 0, indexPath.row >= rowCount - 3 else { return }
            guard parent.chat.canLoadOlder else { return }

            // Find the oldest message ID for dedup.
            let oldestMessageId = parent.chat.messages.first?.id
            guard let oldestMessageId, oldestMessageId != requestedOldestId else { return }
            requestedOldestId = oldestMessageId
            parent.onLoadOlderMessages?()
        }

        // MARK: Delegate — Status Bar Tap

        func scrollViewShouldScrollToTop(_ scrollView: UIScrollView) -> Bool {
            // Default behavior scrolls to contentOffset 0 = visual bottom in inverted table.
            // Instead, scroll to the visual top (oldest messages = max contentOffset).
            let maxOffset = scrollView.contentSize.height - scrollView.bounds.height + scrollView.adjustedContentInset.bottom
            if maxOffset > 0 {
                scrollView.setContentOffset(CGPoint(x: 0, y: maxOffset), animated: true)
            }
            return false
        }

        // MARK: Delegate — Scroll Tracking

        func scrollViewDidScroll(_ scrollView: UIScrollView) {
            // In inverted table, contentOffset.y near 0 = at the bottom (newest messages).
            let nearBottom = scrollView.contentOffset.y < 50
            if parent.isAtBottom != nearBottom {
                DispatchQueue.main.async {
                    self.parent.isAtBottom = nearBottom
                }
            }

            // Update sticky mode.
            if nearBottom && !parent.shouldStickToBottom {
                DispatchQueue.main.async {
                    self.parent.shouldStickToBottom = true
                }
            }
        }

        func scrollViewWillBeginDragging(_ scrollView: UIScrollView) {
            // User started scrolling — if they scroll away from bottom, disable sticky.
            let nearBottom = scrollView.contentOffset.y < 50
            if !nearBottom && parent.shouldStickToBottom {
                DispatchQueue.main.async {
                    self.parent.shouldStickToBottom = false
                }
            }
        }

        func scrollViewDidEndDragging(_ scrollView: UIScrollView, willDecelerate decelerate: Bool) {
            if !decelerate {
                updateStickyAfterScroll(scrollView)
            }
        }

        func scrollViewDidEndDecelerating(_ scrollView: UIScrollView) {
            updateStickyAfterScroll(scrollView)
        }

        private func updateStickyAfterScroll(_ scrollView: UIScrollView) {
            let nearBottom = scrollView.contentOffset.y < 50
            if nearBottom != parent.shouldStickToBottom {
                DispatchQueue.main.async {
                    self.parent.shouldStickToBottom = nearBottom
                }
            }
        }

        // MARK: Row Content Builder

        @ViewBuilder
        private func rowContent(for row: InvertedRow, parent: InvertedMessageList) -> some View {
            switch row {
            case .typing:
                TypingIndicatorRow(
                    typingMembers: parent.chat.typingMembers,
                    members: parent.chat.members
                )
                .padding(.horizontal, 12)
                .padding(.vertical, 4)

            case .timeline(let timelineRow):
                Group {
                    switch timelineRow {
                    case .messageGroup(let group):
                        MessageGroupRow(
                            group: group,
                            showSender: parent.isGroup,
                            onSendMessage: parent.onSendMessage,
                            replyTargetsById: parent.messagesById,
                            onTapSender: parent.onTapSender,
                            onJumpToMessage: { [self] messageId in
                                jumpToMessage(messageId)
                            },
                            onReact: parent.onReact,
                            activeReactionMessageId: .constant(parent.activeReactionMessageId),
                            onLongPressMessage: parent.onLongPressMessage,
                            onDownloadMedia: parent.onDownloadMedia,
                            onTapImage: parent.onTapImage,
                            onHypernoteAction: parent.onHypernoteAction
                        )
                    case .unreadDivider:
                        UnreadDividerRow()
                    case .callEvent(let event):
                        CallTimelineEventRow(event: event)
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 4)
            }
        }

        func jumpToMessage(_ messageId: String) {
            guard let tableView else { return }
            guard let index = invertedRows.firstIndex(where: { row in
                if case .timeline(let tr) = row {
                    // Check if any message in the group has this ID.
                    if case .messageGroup(let group) = tr {
                        return group.messages.contains { $0.id == messageId }
                    }
                }
                return false
            }) else { return }
            tableView.scrollToRow(at: IndexPath(row: index, section: 0), at: .middle, animated: true)
        }
    }
}

// MARK: - InvertedRow

/// Wrapper enum to unify timeline rows and the typing indicator in a single data source.
enum InvertedRow: Identifiable {
    case typing
    case timeline(ChatView.ChatTimelineRow)

    var id: String {
        switch self {
        case .typing: return "typing-indicator"
        case .timeline(let row): return row.id
        }
    }
}
