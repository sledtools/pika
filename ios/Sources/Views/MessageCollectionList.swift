import SwiftUI
import UIKit

/// A UICollectionView-based message transcript for chat with an accessory-backed composer.
struct MessageCollectionList<AccessoryContent: View>: UIViewControllerRepresentable {
    struct ContentState: Equatable {
        let chat: ChatViewState
        let rows: [ChatView.ChatTimelineRow]
        let activeReactionMessageId: String?
    }

    let rows: [ChatView.ChatTimelineRow]
    let chat: ChatViewState
    let messagesById: [String: ChatMessage]
    let isGroup: Bool
    let accessoryContent: AccessoryContent
    let isInputFocused: Bool

    let onSendMessage: @MainActor (String, String?) -> Void
    var onTapSender: (@MainActor (String) -> Void)?
    var onReact: (@MainActor (String, String) -> Void)?
    var onDownloadMedia: ((String, String) -> Void)?
    var onTapImage: (([ChatMediaAttachment], ChatMediaAttachment) -> Void)?
    var onHypernoteAction: ((String, String, [String: String]) -> Void)?
    var onLongPressMessage: ((ChatMessage, CGRect) -> Void)?
    var onRetryMessage: ((String) -> Void)?
    var onLoadOlderMessages: (() -> Void)?

    @Binding var followsBottom: Bool
    var activeReactionMessageId: String?

    private var contentState: ContentState {
        ContentState(chat: chat, rows: rows, activeReactionMessageId: activeReactionMessageId)
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    func makeUIViewController(context: Context) -> MessageCollectionHostController<AccessoryContent> {
        let viewController = MessageCollectionHostController(
            layout: MessageCollectionList.makeLayout(),
            accessoryContent: accessoryContent
        )
        let collectionView = viewController.collectionView
        collectionView.backgroundColor = .clear
        collectionView.contentInsetAdjustmentBehavior = .automatic
        collectionView.alwaysBounceVertical = true
        collectionView.keyboardDismissMode = .interactive
        collectionView.delegate = context.coordinator
        collectionView.showsVerticalScrollIndicator = true
        collectionView.alwaysBounceHorizontal = false
        collectionView.onBoundsSizeChange = { [weak coordinator = context.coordinator] _ in
            coordinator?.handleViewportGeometryChange()
        }
        collectionView.onContentSizeChange = { [weak coordinator = context.coordinator] _ in
            coordinator?.handleContentSizeChange()
        }
        viewController.onViewportGeometryChange = { [weak coordinator = context.coordinator] in
            coordinator?.handleViewportGeometryChange()
        }
        viewController.onWillDisappear = { [weak coordinator = context.coordinator] in
            coordinator?.persistCurrentScrollPosition()
        }
        viewController.onJumpToBottomTap = { [weak coordinator = context.coordinator] in
            coordinator?.handleJumpButtonTap()
        }

        context.coordinator.collectionView = collectionView
        context.coordinator.viewController = viewController
        context.coordinator.lastContentState = contentState

        let registration = UICollectionView.CellRegistration<UICollectionViewCell, String> {
            [weak coordinator = context.coordinator] cell, _, itemID in
            guard let coordinator, let row = coordinator.rowsByID[itemID] else { return }
            var background = UIBackgroundConfiguration.clear()
            background.backgroundColor = .clear
            cell.backgroundConfiguration = background
            cell.contentConfiguration = UIHostingConfiguration {
                coordinator.rowContent(for: row, parent: coordinator.parent)
            }
            .minSize(width: 0, height: 0)
            .margins(.all, 0)
        }

        let dataSource = UICollectionViewDiffableDataSource<Int, String>(collectionView: collectionView) {
            collectionView, indexPath, itemID in
            collectionView.dequeueConfiguredReusableCell(
                using: registration,
                for: indexPath,
                item: itemID
            )
        }
        context.coordinator.dataSource = dataSource

        let renderedRows = buildRenderedRows()
        viewController.updateAccessory(rootView: accessoryContent, keepVisible: !isInputFocused)
        viewController.setJumpButtonVisible(!followsBottom, animated: false)
        context.coordinator.applyViewportInsetsIfNeeded()
        context.coordinator.applyRows(renderedRows, animated: false) {
            context.coordinator.markInitialRowsApplied()
        }

        return viewController
    }

    func updateUIViewController(
        _ viewController: MessageCollectionHostController<AccessoryContent>,
        context: Context
    ) {
        let coordinator = context.coordinator
        coordinator.parent = self
        coordinator.collectionView = viewController.collectionView
        coordinator.viewController = viewController

        let wasNearBottom = coordinator.isNearBottom()
        let newRows = buildRenderedRows()
        let newIDs = newRows.map(\.id)
        let updateKind = MessageCollectionLayout.classifyUpdate(oldIDs: coordinator.currentIDs, newIDs: newIDs)
        let anchor = wasNearBottom ? nil : coordinator.captureTopAnchor()
        let contentChanged = coordinator.lastContentState != contentState
        coordinator.lastContentState = contentState

        let accessoryHeightChanged = viewController.updateAccessory(
            rootView: accessoryContent,
            keepVisible: !isInputFocused
        )
        if accessoryHeightChanged, !wasNearBottom {
            coordinator.pendingViewportAnchor = anchor
        }
        viewController.setJumpButtonVisible(!followsBottom, animated: true)

        let viewportChanged = coordinator.applyViewportInsetsIfNeeded()

        let completion = {
            if wasNearBottom {
                let animateToBottom = updateKind == .tailMutation
                coordinator.scrollToBottom(animated: animateToBottom)
            } else if let anchor {
                coordinator.restore(anchor: anchor)
            }
        }

        switch updateKind {
        case .reconfigureOnly:
            let didApplyVisibleRefresh = contentChanged
                ? coordinator.reconfigureVisibleRows(with: newRows, completion: completion)
                : false
            if !didApplyVisibleRefresh && viewportChanged {
                completion()
            }
        case .tailMutation, .structural:
            let animateDifferences = wasNearBottom && updateKind == .tailMutation
            coordinator.applyRows(newRows, animated: animateDifferences, completion: completion)
        }
    }

    static func dismantleUIViewController(
        _ viewController: MessageCollectionHostController<AccessoryContent>,
        coordinator: Coordinator
    ) {
        coordinator.persistCurrentScrollPosition()
    }

    private static func makeLayout() -> UICollectionViewLayout {
        let itemSize = NSCollectionLayoutSize(
            widthDimension: .fractionalWidth(1.0),
            heightDimension: .estimated(44)
        )
        let item = NSCollectionLayoutItem(layoutSize: itemSize)
        let group = NSCollectionLayoutGroup.vertical(layoutSize: itemSize, subitems: [item])
        let section = NSCollectionLayoutSection(group: group)
        section.interGroupSpacing = 0
        return UICollectionViewCompositionalLayout(section: section)
    }

    private func buildRenderedRows() -> [RenderedRow] {
        var rendered = rows.map(RenderedRow.timeline)
        if !chat.typingMembers.isEmpty {
            rendered.append(.typing)
        }
        return rendered
    }

    final class Coordinator: NSObject, UICollectionViewDelegate {
        var parent: MessageCollectionList
        var dataSource: UICollectionViewDiffableDataSource<Int, String>?
        var rowsByID: [String: RenderedRow] = [:]
        var currentIDs: [String] = []
        weak var collectionView: UICollectionView?
        weak var viewController: MessageCollectionHostController<AccessoryContent>?
        private var requestedOldestId: String?
        private var lastAppliedEffectiveInset: UIEdgeInsets?
        var lastContentState: ContentState?
        var pendingViewportAnchor: ScrollAnchor?
        private var pendingInitialScrollPosition: SavedChatTranscriptPosition?
        private var hasAppliedInitialRows = false
        private var isHoldingInitialBottomPin = false

        init(parent: MessageCollectionList) {
            self.parent = parent
            self.pendingInitialScrollPosition =
                ChatTranscriptScrollPositionStore.shared.position(for: parent.chat.chatId) ?? .bottom
        }

        func applyRows(_ rows: [RenderedRow], animated: Bool, completion: (() -> Void)? = nil) {
            currentIDs = rows.map(\.id)
            rowsByID = Dictionary(uniqueKeysWithValues: rows.map { ($0.id, $0) })

            var snapshot = NSDiffableDataSourceSnapshot<Int, String>()
            snapshot.appendSections([0])
            snapshot.appendItems(rows.map(\.id), toSection: 0)
            dataSource?.apply(snapshot, animatingDifferences: animated) {
                completion?()
            }
        }

        @discardableResult
        func reconfigureVisibleRows(with rows: [RenderedRow], completion: (() -> Void)? = nil) -> Bool {
            currentIDs = rows.map(\.id)
            rowsByID = Dictionary(uniqueKeysWithValues: rows.map { ($0.id, $0) })

            guard let dataSource else { return false }
            let visibleIDs = visibleItemIDs()
            guard !visibleIDs.isEmpty else { return false }

            var snapshot = dataSource.snapshot()
            snapshot.reconfigureItems(visibleIDs)
            dataSource.apply(snapshot, animatingDifferences: false) {
                completion?()
            }
            return true
        }

        func scrollToBottom(animated: Bool) {
            guard let collectionView else { return }
            applyEffectiveInsetsIfNeeded()
            collectionView.layoutIfNeeded()
            collectionView.setContentOffset(
                MessageCollectionLayout.bottomContentOffset(
                    contentHeight: collectionView.contentSize.height,
                    boundsHeight: collectionView.bounds.height,
                    topAdjustedInset: collectionView.adjustedContentInset.top,
                    bottomInset: collectionView.contentInset.bottom
                ),
                animated: animated
            )
        }

        @discardableResult
        func applyViewportInsetsIfNeeded() -> Bool {
            applyEffectiveInsetsIfNeeded()
        }

        func handleJumpButtonTap() {
            isHoldingInitialBottomPin = false
            DispatchQueue.main.async {
                self.parent.followsBottom = true
            }
            viewController?.setJumpButtonVisible(false, animated: true)
            scrollToBottom(animated: true)
        }

        func handleViewportGeometryChange() {
            let wasNearBottom = isNearBottom()
            _ = applyEffectiveInsetsIfNeeded()

            if let pendingInitialScrollPosition {
                guard hasAppliedInitialRows,
                      let viewController,
                      viewController.isViewportReadyForInitialBottomPin
                else { return }
                self.pendingInitialScrollPosition = nil
                applyInitialScrollPosition(pendingInitialScrollPosition)
                return
            }

            if let anchor = pendingViewportAnchor {
                pendingViewportAnchor = nil
                restore(anchor: anchor)
                return
            }
            guard isHoldingInitialBottomPin || wasNearBottom else { return }
            scrollToBottom(animated: false)
        }

        func handleContentSizeChange() {
            _ = applyEffectiveInsetsIfNeeded()

            if pendingInitialScrollPosition != nil {
                handleViewportGeometryChange()
                return
            }

            guard isHoldingInitialBottomPin || parent.followsBottom || isNearBottom() else { return }
            scrollToBottom(animated: false)
        }

        func markInitialRowsApplied() {
            hasAppliedInitialRows = true
            handleViewportGeometryChange()
        }

        func persistCurrentScrollPosition() {
            guard collectionView != nil else { return }

            let position: SavedChatTranscriptPosition
            if isNearBottom() {
                position = .bottom
            } else if let anchor = captureTopAnchor() {
                position = .anchor(anchor)
            } else {
                return
            }

            ChatTranscriptScrollPositionStore.shared.set(position, for: parent.chat.chatId)
        }

        func captureTopAnchor() -> ScrollAnchor? {
            guard let collectionView,
                  let dataSource,
                  let indexPath = collectionView.indexPathsForVisibleItems.sorted(by: indexPathSort).first,
                  let itemID = dataSource.itemIdentifier(for: indexPath),
                  let attributes = collectionView.layoutAttributesForItem(at: indexPath)
            else { return nil }

            return ScrollAnchor(
                itemID: itemID,
                distanceFromContentOffset: attributes.frame.minY - collectionView.contentOffset.y
            )
        }

        @discardableResult
        func restore(anchor: ScrollAnchor) -> Bool {
            guard let collectionView,
                  let dataSource,
                  let indexPath = dataSource.indexPath(for: anchor.itemID)
            else { return false }

            applyEffectiveInsetsIfNeeded()
            collectionView.layoutIfNeeded()
            collectionView.scrollToItem(at: indexPath, at: .top, animated: false)
            collectionView.layoutIfNeeded()

            guard let attributes = collectionView.layoutAttributesForItem(at: indexPath) else { return false }

            let minOffsetY = -collectionView.adjustedContentInset.top
            let maxOffsetY = max(
                minOffsetY,
                collectionView.contentSize.height - collectionView.bounds.height + collectionView.contentInset.bottom
            )
            let targetY = min(
                max(attributes.frame.minY - anchor.distanceFromContentOffset, minOffsetY),
                maxOffsetY
            )
            collectionView.setContentOffset(CGPoint(x: 0, y: targetY), animated: false)
            return true
        }

        func collectionView(
            _ collectionView: UICollectionView,
            willDisplay cell: UICollectionViewCell,
            forItemAt indexPath: IndexPath
        ) {
            guard indexPath.item <= 2 else { return }
            guard parent.chat.canLoadOlder else { return }

            let oldestMessageId = parent.chat.messages.first?.id
            guard let oldestMessageId, oldestMessageId != requestedOldestId else { return }
            requestedOldestId = oldestMessageId
            parent.onLoadOlderMessages?()
        }

        func scrollViewDidScroll(_ scrollView: UIScrollView) {
            let nearBottom = isNearBottom()
            if isHoldingInitialBottomPin {
                viewController?.setJumpButtonVisible(false, animated: true)
                return
            }
            viewController?.setJumpButtonVisible(!nearBottom, animated: true)
            if nearBottom != parent.followsBottom {
                DispatchQueue.main.async {
                    self.parent.followsBottom = nearBottom
                }
            }
        }

        func scrollViewWillBeginDragging(_ scrollView: UIScrollView) {
            isHoldingInitialBottomPin = false
        }

        @ViewBuilder
        func rowContent(for row: RenderedRow, parent: MessageCollectionList) -> some View {
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
                            onJumpToMessage: { [self] messageID in
                                jumpToMessage(messageID)
                            },
                            onReact: parent.onReact,
                            activeReactionMessageId: .constant(parent.activeReactionMessageId),
                            onLongPressMessage: parent.onLongPressMessage,
                            onDownloadMedia: parent.onDownloadMedia,
                            onTapImage: parent.onTapImage,
                            onHypernoteAction: parent.onHypernoteAction,
                            onRetryMessage: parent.onRetryMessage
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

        func jumpToMessage(_ messageID: String) {
            guard let dataSource,
                  let collectionView else { return }

            let snapshot = dataSource.snapshot()
            guard let rowID = snapshot.itemIdentifiers.first(where: { rowID in
                guard let row = rowsByID[rowID],
                      case .timeline(let timelineRow) = row,
                      case .messageGroup(let group) = timelineRow
                else { return false }

                return group.messages.contains { $0.id == messageID }
            }),
            let indexPath = dataSource.indexPath(for: rowID)
            else { return }

            collectionView.scrollToItem(at: indexPath, at: .centeredVertically, animated: true)
        }

        private func visibleItemIDs() -> [String] {
            guard let collectionView, let dataSource else { return [] }
            return collectionView.indexPathsForVisibleItems
                .sorted(by: indexPathSort)
                .compactMap { dataSource.itemIdentifier(for: $0) }
        }

        private func applyInitialScrollPosition(_ position: SavedChatTranscriptPosition) {
            switch position {
            case .bottom:
                isHoldingInitialBottomPin = true
                DispatchQueue.main.async {
                    self.parent.followsBottom = true
                }
                viewController?.setJumpButtonVisible(false, animated: false)
                scrollToBottom(animated: false)

            case .anchor(let anchor):
                isHoldingInitialBottomPin = false
                let restored = restore(anchor: anchor)
                DispatchQueue.main.async {
                    self.parent.followsBottom = !restored
                }
                viewController?.setJumpButtonVisible(restored, animated: false)
                if !restored {
                    scrollToBottom(animated: false)
                }
            }
        }

        func isNearBottom() -> Bool {
            guard let collectionView else { return parent.followsBottom }
            return MessageCollectionLayout.isNearBottom(
                contentOffsetY: collectionView.contentOffset.y,
                boundsHeight: collectionView.bounds.height,
                contentHeight: collectionView.contentSize.height,
                topAdjustedInset: collectionView.adjustedContentInset.top,
                bottomInset: collectionView.contentInset.bottom
            )
        }

        @discardableResult
        private func applyEffectiveInsetsIfNeeded() -> Bool {
            guard let collectionView, let viewController else { return false }
            collectionView.layoutIfNeeded()

            let topChromeInset = max(0, collectionView.adjustedContentInset.top - collectionView.contentInset.top)

            let effectiveInset = MessageCollectionLayout.effectiveContentInset(
                boundsHeight: collectionView.bounds.height,
                contentHeight: collectionView.contentSize.height,
                topChromeInset: topChromeInset,
                bottomInset: viewController.bottomViewportInset
            )
            guard effectiveInset != lastAppliedEffectiveInset else { return false }
            lastAppliedEffectiveInset = effectiveInset
            collectionView.contentInset = effectiveInset
            collectionView.verticalScrollIndicatorInsets = .zero
            return true
        }

        private func indexPathSort(_ lhs: IndexPath, _ rhs: IndexPath) -> Bool {
            if lhs.section == rhs.section {
                return lhs.item < rhs.item
            }
            return lhs.section < rhs.section
        }
    }
}

final class MessageCollectionHostController<AccessoryContent: View>: UIViewController {
    fileprivate let collectionView: BoundsAwareCollectionView
    private let accessoryContainerView: InputAccessoryHostingView<AccessoryContent>
    private let jumpButtonChromeView = UIVisualEffectView(effect: UIBlurEffect(style: .systemUltraThinMaterial))
    private let jumpButton = UIButton(type: .system)
    var onViewportGeometryChange: (() -> Void)?
    var onWillDisappear: (() -> Void)?
    var onJumpToBottomTap: (() -> Void)?
    private var lastReportedBottomViewportInset: CGFloat = 0
    private var jumpButtonBottomConstraint: NSLayoutConstraint?
    private var isJumpButtonVisible = false

    var bottomViewportInset: CGFloat {
        max(0, view.bounds.maxY - view.keyboardLayoutGuide.layoutFrame.minY)
    }

    var isViewportReadyForInitialBottomPin: Bool {
        isViewLoaded && view.window != nil && collectionView.bounds.height > 0 && bottomViewportInset > 0
    }

    init(layout: UICollectionViewLayout, accessoryContent: AccessoryContent) {
        self.collectionView = BoundsAwareCollectionView(frame: .zero, collectionViewLayout: layout)
        self.accessoryContainerView = InputAccessoryHostingView(rootView: accessoryContent)
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override var inputAccessoryView: UIView? {
        accessoryContainerView
    }

    override var canBecomeFirstResponder: Bool {
        true
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .clear

        collectionView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(collectionView)
        NSLayoutConstraint.activate([
            collectionView.topAnchor.constraint(equalTo: view.topAnchor),
            collectionView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            collectionView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            collectionView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
        configureJumpButton()

        accessoryContainerView.onHeightChange = { [weak self] in
            self?.updateJumpButtonBottomConstraint()
            self?.onViewportGeometryChange?()
        }
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        if !isFirstResponder {
            becomeFirstResponder()
        }
        DispatchQueue.main.async { [weak self] in
            self?.onViewportGeometryChange?()
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        onWillDisappear?()
        if isFirstResponder {
            resignFirstResponder()
        }
    }

    override func viewSafeAreaInsetsDidChange() {
        super.viewSafeAreaInsetsDidChange()
        onViewportGeometryChange?()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        updateJumpButtonBottomConstraint()
        let bottomViewportInset = self.bottomViewportInset
        guard abs(bottomViewportInset - lastReportedBottomViewportInset) > 0.5 else { return }
        lastReportedBottomViewportInset = bottomViewportInset
        onViewportGeometryChange?()
    }

    @discardableResult
    func updateAccessory(rootView: AccessoryContent, keepVisible: Bool) -> Bool {
        let accessoryHeightChanged = accessoryContainerView.update(rootView: rootView)

        if keepVisible, view.window != nil, !isFirstResponder {
            becomeFirstResponder()
        }
        return accessoryHeightChanged
    }

    func setJumpButtonVisible(_ visible: Bool, animated: Bool) {
        guard visible != isJumpButtonVisible else { return }
        isJumpButtonVisible = visible

        let updates = {
            self.jumpButtonChromeView.alpha = visible ? 1 : 0
            self.jumpButtonChromeView.transform = visible ? .identity : CGAffineTransform(scaleX: 0.9, y: 0.9)
        }

        jumpButtonChromeView.isHidden = false
        jumpButtonChromeView.isUserInteractionEnabled = visible
        jumpButton.accessibilityElementsHidden = !visible

        if animated {
            UIView.animate(
                withDuration: 0.18,
                delay: 0,
                options: [.beginFromCurrentState, .curveEaseInOut]
            ) {
                updates()
            } completion: { _ in
                self.jumpButtonChromeView.isHidden = !visible
            }
        } else {
            updates()
            jumpButtonChromeView.isHidden = !visible
        }
    }

    private func configureJumpButton() {
        jumpButtonChromeView.translatesAutoresizingMaskIntoConstraints = false
        jumpButtonChromeView.layer.cornerRadius = 18
        jumpButtonChromeView.clipsToBounds = true
        jumpButtonChromeView.layer.borderWidth = 0.5
        jumpButtonChromeView.layer.borderColor = UIColor.quaternaryLabel.cgColor
        jumpButtonChromeView.alpha = 0
        jumpButtonChromeView.isHidden = true
        jumpButtonChromeView.isUserInteractionEnabled = false
        view.addSubview(jumpButtonChromeView)

        jumpButton.translatesAutoresizingMaskIntoConstraints = false
        jumpButton.tintColor = .label
        jumpButton.setImage(UIImage(systemName: "arrow.down"), for: .normal)
        jumpButton.setPreferredSymbolConfiguration(
            UIImage.SymbolConfiguration(pointSize: 13, weight: .semibold),
            forImageIn: .normal
        )
        jumpButton.accessibilityLabel = "Scroll to bottom"
        jumpButton.addTarget(self, action: #selector(handleJumpButtonTap), for: .touchUpInside)
        jumpButtonChromeView.contentView.addSubview(jumpButton)

        jumpButtonBottomConstraint = jumpButtonChromeView.bottomAnchor.constraint(
            equalTo: view.keyboardLayoutGuide.topAnchor
        )

        guard let bottomConstraint = jumpButtonBottomConstraint else {
            assertionFailure("jumpButtonBottomConstraint should be configured before activation")
            return
        }

        NSLayoutConstraint.activate([
            jumpButtonChromeView.widthAnchor.constraint(equalToConstant: 36),
            jumpButtonChromeView.heightAnchor.constraint(equalToConstant: 36),
            jumpButtonChromeView.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -16),
            bottomConstraint,
            jumpButton.centerXAnchor.constraint(equalTo: jumpButtonChromeView.contentView.centerXAnchor),
            jumpButton.centerYAnchor.constraint(equalTo: jumpButtonChromeView.contentView.centerYAnchor),
        ])

        updateJumpButtonBottomConstraint()
    }

    private func updateJumpButtonBottomConstraint() {
        jumpButtonBottomConstraint?.constant = 0
    }

    @objc
    private func handleJumpButtonTap() {
        onJumpToBottomTap?()
    }
}

final class InputAccessoryHostingView<AccessoryContent: View>: UIInputView {
    private var hostedView: (UIView & UIContentView)?
    private var lastReportedHeight: CGFloat = 0
    var onHeightChange: (() -> Void)?

    var currentHeight: CGFloat {
        lastReportedHeight
    }

    init(rootView: AccessoryContent) {
        super.init(frame: .zero, inputViewStyle: .default)
        allowsSelfSizing = true
        backgroundColor = .clear
        isOpaque = false
        clipsToBounds = false
        autoresizingMask = [.flexibleHeight]
        update(rootView: rootView)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    @discardableResult
    func update(rootView: AccessoryContent) -> Bool {
        let configuration = UIHostingConfiguration {
            rootView
        }
        .margins(.all, 0)

        if let hostedView {
            hostedView.configuration = configuration
            hostedView.backgroundColor = .clear
        } else {
            let contentView = configuration.makeContentView()
            contentView.translatesAutoresizingMaskIntoConstraints = false
            contentView.backgroundColor = .clear
            contentView.isOpaque = false
            addSubview(contentView)
            NSLayoutConstraint.activate([
                contentView.topAnchor.constraint(equalTo: topAnchor),
                contentView.leadingAnchor.constraint(equalTo: leadingAnchor),
                contentView.trailingAnchor.constraint(equalTo: trailingAnchor),
                contentView.bottomAnchor.constraint(equalTo: bottomAnchor),
            ])
            hostedView = contentView
        }

        invalidateIntrinsicContentSize()
        setNeedsLayout()
        layoutIfNeeded()
        return updatePreferredContentSize()
    }

    override var intrinsicContentSize: CGSize {
        preferredSize(forWidth: bounds.width)
    }

    override func systemLayoutSizeFitting(_ targetSize: CGSize) -> CGSize {
        preferredSize(forWidth: targetSize.width)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        updatePreferredContentSize()
    }

    @discardableResult
    private func updatePreferredContentSize() -> Bool {
        let fittingWidth = max(bounds.width, UIScreen.main.bounds.width)
        let height = preferredSize(forWidth: fittingWidth).height.rounded(.up)
        guard abs(height - lastReportedHeight) > 0.5 else { return false }
        lastReportedHeight = height
        onHeightChange?()
        return true
    }

    private func preferredSize(forWidth width: CGFloat) -> CGSize {
        guard let hostedView else {
            return CGSize(width: UIView.noIntrinsicMetric, height: 0)
        }
        let fittingWidth = width > 0 ? width : UIScreen.main.bounds.width
        let targetSize = CGSize(width: fittingWidth, height: UIView.layoutFittingCompressedSize.height)
        let size = hostedView.systemLayoutSizeFitting(
            targetSize,
            withHorizontalFittingPriority: .required,
            verticalFittingPriority: .fittingSizeLevel
        )
        return CGSize(width: UIView.noIntrinsicMetric, height: ceil(size.height))
    }
}

private final class BoundsAwareCollectionView: UICollectionView {
    var onBoundsSizeChange: ((CGSize) -> Void)?
    var onContentSizeChange: ((CGSize) -> Void)?
    private var lastReportedSize: CGSize = .zero
    private var lastReportedContentSize: CGSize = .zero

    override func layoutSubviews() {
        super.layoutSubviews()
        if contentSize != lastReportedContentSize {
            lastReportedContentSize = contentSize
            onContentSizeChange?(contentSize)
        }
        guard bounds.size != lastReportedSize else { return }
        lastReportedSize = bounds.size
        onBoundsSizeChange?(bounds.size)
    }
}

struct ScrollAnchor {
    let itemID: String
    let distanceFromContentOffset: CGFloat
}

fileprivate enum SavedChatTranscriptPosition {
    case bottom
    case anchor(ScrollAnchor)
}

@MainActor
fileprivate final class ChatTranscriptScrollPositionStore {
    static let shared = ChatTranscriptScrollPositionStore()

    private var positions: [String: SavedChatTranscriptPosition] = [:]

    func position(for chatId: String) -> SavedChatTranscriptPosition? {
        positions[chatId]
    }

    func set(_ position: SavedChatTranscriptPosition, for chatId: String) {
        positions[chatId] = position
    }
}

enum RenderedRow: Identifiable {
    case typing
    case timeline(ChatView.ChatTimelineRow)

    var id: String {
        switch self {
        case .typing:
            return MessageCollectionRowID.typingIndicator
        case .timeline(let row):
            return row.id
        }
    }
}
