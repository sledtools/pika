import SwiftUI
import UIKit

/// A UITableView-based inverted message list for chat.
///
/// The table is flipped via scaleY:-1 so the "bottom" (newest messages) is at the
/// natural scroll origin. Each cell's content is flipped back. This means older
/// messages are *appended* (not prepended) to the data source, so loading history
/// never causes scroll jumps.
///
/// Uses `UITableViewDiffableDataSource` for correct, animation-free diff updates.
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

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    func makeUIView(context: Context) -> UITableView {
        let tableView = UITableView(frame: .zero, style: .plain)
        tableView.transform = CGAffineTransform(scaleX: 1, y: -1)
        tableView.separatorStyle = .none
        tableView.backgroundColor = .clear
        tableView.showsVerticalScrollIndicator = true
        tableView.alwaysBounceVertical = false
        tableView.keyboardDismissMode = .interactive
        tableView.rowHeight = UITableView.automaticDimension
        tableView.estimatedRowHeight = 80
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "cell")
        tableView.delegate = context.coordinator
        context.coordinator.tableView = tableView

        // Set up diffable data source.
        let dataSource = UITableViewDiffableDataSource<Int, String>(tableView: tableView) {
            [weak coordinator = context.coordinator] tableView, indexPath, itemId in
            let cell = tableView.dequeueReusableCell(withIdentifier: "cell", for: indexPath)
            guard let coordinator else { return cell }
            cell.selectionStyle = .none
            cell.backgroundColor = .clear

            if let row = coordinator.rowsByID[itemId] {
                cell.contentConfiguration = UIHostingConfiguration {
                    coordinator.rowContent(for: row, parent: coordinator.parent)
                        .scaleEffect(x: 1, y: -1, anchor: .center)
                }
                .minSize(width: 0, height: 0)
                .margins(.all, 0)
            }
            return cell
        }
        dataSource.defaultRowAnimation = .none
        context.coordinator.dataSource = dataSource

        // Apply initial snapshot.
        let invertedRows = buildInvertedRows()
        context.coordinator.applyRows(invertedRows, animated: false)

        return tableView
    }

    func updateUIView(_ tableView: UITableView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self

        let newInverted = buildInvertedRows()
        let newIDs = newInverted.map(\.id)

        if newIDs != coordinator.currentIDs {
            // Structural change — apply new snapshot.
            let stickyBottom = shouldStickToBottom
            coordinator.applyRows(newInverted, animated: false) {
                if stickyBottom {
                    coordinator.scrollToBottom(animated: false)
                }
            }
        } else if let dataSource = coordinator.dataSource {
            // Same structure — reconfigure visible cells for content changes
            // (delivery state, reactions, typing indicator updates, etc.)
            // Use the diffable data source's reconfigure API which is safe
            // during layout passes, unlike UITableView.reconfigureRows.
            coordinator.rowsByID = Dictionary(uniqueKeysWithValues: newInverted.map { ($0.id, $0) })
            var snapshot = dataSource.snapshot()
            let visibleIDs = tableView.indexPathsForVisibleRows?
                .compactMap { snapshot.itemIdentifiers(inSection: 0).indices.contains($0.row) ? snapshot.itemIdentifiers(inSection: 0)[$0.row] : nil }
                ?? []
            if !visibleIDs.isEmpty {
                snapshot.reconfigureItems(visibleIDs)
                dataSource.apply(snapshot, animatingDifferences: false)
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

    final class Coordinator: NSObject, UITableViewDelegate {
        var parent: InvertedMessageList
        var dataSource: UITableViewDiffableDataSource<Int, String>?
        var rowsByID: [String: InvertedRow] = [:]
        var currentIDs: [String] = []
        weak var tableView: UITableView?
        var lastScrollToBottomTrigger: Int = 0
        private var requestedOldestId: String?

        init(parent: InvertedMessageList) {
            self.parent = parent
            self.lastScrollToBottomTrigger = parent.scrollToBottomTrigger
        }

        func applyRows(_ rows: [InvertedRow], animated: Bool, completion: (() -> Void)? = nil) {
            currentIDs = rows.map(\.id)
            rowsByID = Dictionary(uniqueKeysWithValues: rows.map { ($0.id, $0) })

            var snapshot = NSDiffableDataSourceSnapshot<Int, String>()
            snapshot.appendSections([0])
            snapshot.appendItems(rows.map(\.id), toSection: 0)
            dataSource?.apply(snapshot, animatingDifferences: animated) {
                completion?()
            }
        }

        func scrollToBottom(animated: Bool) {
            guard let tableView,
                  let dataSource,
                  dataSource.snapshot().numberOfItems > 0 else { return }
            // In inverted table, row 0 = newest message = visual bottom.
            tableView.scrollToRow(at: IndexPath(row: 0, section: 0), at: .top, animated: animated)
        }

        // MARK: Delegate — Pagination

        func tableView(_ tableView: UITableView, willDisplay cell: UITableViewCell, forRowAt indexPath: IndexPath) {
            let snapshot = dataSource?.snapshot()
            let rowCount = snapshot?.numberOfItems ?? 0
            guard rowCount > 0, indexPath.row >= rowCount - 3 else { return }
            guard parent.chat.canLoadOlder else { return }

            let oldestMessageId = parent.chat.messages.first?.id
            guard let oldestMessageId, oldestMessageId != requestedOldestId else { return }
            requestedOldestId = oldestMessageId
            parent.onLoadOlderMessages?()
        }

        // MARK: Delegate — Status Bar Tap

        func scrollViewShouldScrollToTop(_ scrollView: UIScrollView) -> Bool {
            let maxOffset = scrollView.contentSize.height - scrollView.bounds.height + scrollView.adjustedContentInset.bottom
            if maxOffset > 0 {
                scrollView.setContentOffset(CGPoint(x: 0, y: maxOffset), animated: true)
            }
            return false
        }

        // MARK: Delegate — Scroll Tracking

        func scrollViewDidScroll(_ scrollView: UIScrollView) {
            let nearBottom = scrollView.contentOffset.y < 50
            if parent.isAtBottom != nearBottom {
                DispatchQueue.main.async {
                    self.parent.isAtBottom = nearBottom
                }
            }
            if nearBottom && !parent.shouldStickToBottom {
                DispatchQueue.main.async {
                    self.parent.shouldStickToBottom = true
                }
            }
        }

        func scrollViewWillBeginDragging(_ scrollView: UIScrollView) {
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
        func rowContent(for row: InvertedRow, parent: InvertedMessageList) -> some View {
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
            guard let dataSource, let tableView else { return }
            let snapshot = dataSource.snapshot()
            let items = snapshot.itemIdentifiers
            guard let index = items.firstIndex(where: { id in
                if let row = rowsByID[id], case .timeline(let tr) = row,
                   case .messageGroup(let group) = tr {
                    return group.messages.contains { $0.id == messageId }
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
