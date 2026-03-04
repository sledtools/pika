import SwiftUI
import UIKit
import UniformTypeIdentifiers

final class ShareViewController: UIViewController {
    private let viewModel = ShareExtensionViewModel()
    private var hostController: UIHostingController<ShareExtensionView>?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground

        let host = UIHostingController(
            rootView: ShareExtensionView(
                viewModel: viewModel,
                onCancel: { [weak self] in
                    self?.cancelRequest()
                },
                onSend: { [weak self] in
                    self?.sendSelection()
                },
                onOpenApp: { [weak self] in
                    self?.openAppAndComplete()
                }
            )
        )

        addChild(host)
        host.view.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(host.view)
        NSLayoutConstraint.activate([
            host.view.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            host.view.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            host.view.topAnchor.constraint(equalTo: view.topAnchor),
            host.view.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
        host.didMove(toParent: self)
        hostController = host

        viewModel.load(from: extensionContext)
    }

    private func cancelRequest() {
        let error = NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError)
        extensionContext?.cancelRequest(withError: error)
    }

    private func sendSelection() {
        Task { @MainActor [weak self] in
            guard let self else { return }
            let didQueue = await viewModel.enqueueSelectedShare()
            guard didQueue else { return }
            try? await Task.sleep(nanoseconds: 650_000_000)
            extensionContext?.completeRequest(returningItems: nil)
        }
    }

    private func openAppAndComplete() {
        guard let url = shareDispatchURL() else {
            extensionContext?.completeRequest(returningItems: nil)
            return
        }

        extensionContext?.open(url) { [weak self] didOpen in
            guard let self else { return }

            if didOpen {
                self.extensionContext?.completeRequest(returningItems: nil)
                return
            }

            // Some host apps deny `NSExtensionContext.open`. Fall back to responder-chain open.
            _ = self.openViaResponderChain(url)
            self.extensionContext?.completeRequest(returningItems: nil)
        }
    }

    private func shareDispatchURL() -> URL? {
        let configured = (Bundle.main.infoDictionary?["PikaUrlScheme"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let hasPlaceholder = configured.contains("$(") || configured.contains("${")
        let scheme = (!configured.isEmpty && !hasPlaceholder) ? configured : "pika"

        var components = URLComponents()
        components.scheme = scheme
        components.host = "share-send"
        return components.url
    }

    @discardableResult
    private func openViaResponderChain(_ url: URL) -> Bool {
        let openURLSelector = NSSelectorFromString("openURL:")
        var responder: UIResponder? = self
        while let current = responder {
            if let app = current as? UIApplication {
                app.open(url, options: [:], completionHandler: nil)
                return true
            }
            if current.responds(to: openURLSelector) {
                _ = current.perform(openURLSelector, with: url)
                return true
            }
            responder = current.next
        }
        return false
    }
}

private struct ShareExtensionView: View {
    @ObservedObject var viewModel: ShareExtensionViewModel
    let onCancel: () -> Void
    let onSend: () -> Void
    let onOpenApp: () -> Void

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Share To Pika")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Cancel", action: onCancel)
                    }
                    ToolbarItem(placement: .topBarTrailing) {
                        if viewModel.isSending {
                            ProgressView()
                        } else {
                            Button("Send", action: onSend)
                                .disabled(!viewModel.canSend)
                        }
                    }
                }
        }
    }

    @ViewBuilder
    private var content: some View {
        if viewModel.isShowingQueueProgress {
            ShareQueueProgressView(
                stage: viewModel.sendStage,
                progress: viewModel.sendProgress
            )
            .padding(.horizontal, 24)
        } else if !viewModel.isLoggedIn {
            ShareStateView(
                title: "Sign In Required",
                message: "Open Pika and sign in to share content.",
                systemImage: "person.crop.circle.badge.exclamationmark",
                actionTitle: "Open Pika",
                action: onOpenApp
            )
            .padding(.horizontal, 24)
        } else if viewModel.isLoadingPayload {
            VStack(spacing: 12) {
                ProgressView()
                Text("Loading shared content...")
                    .foregroundStyle(.secondary)
            }
        } else if viewModel.chats.isEmpty {
            ShareStateView(
                title: "No Conversations",
                message: "Start a chat in Pika first, then try sharing again.",
                systemImage: "bubble.left.and.bubble.right"
            )
            .padding(.horizontal, 24)
        } else if viewModel.payload == nil {
            ShareStateView(
                title: "Unsupported Content",
                message: viewModel.errorMessage ?? "Pika supports sharing text, links, and images.",
                systemImage: "exclamationmark.triangle"
            )
            .padding(.horizontal, 24)
        } else {
            List {
                Section("Sharing") {
                    SharePayloadPreview(payload: viewModel.payload)
                }

                Section("Message (Optional)") {
                    TextField("Add a message", text: $viewModel.composeText, axis: .vertical)
                        .lineLimit(1...4)
                }

                Section("Recent Chats") {
                    ForEach(viewModel.filteredChats) { chat in
                        Button {
                            viewModel.selectedChatId = chat.chatId
                        } label: {
                            ShareChatRow(
                                chat: chat,
                                isSelected: viewModel.selectedChatId == chat.chatId
                            )
                        }
                        .buttonStyle(.plain)
                    }
                }

                if let error = viewModel.errorMessage {
                    Section {
                        Text(error)
                            .foregroundStyle(.secondary)
                            .font(.footnote)
                    }
                }
            }
            .listStyle(.insetGrouped)
            .searchable(text: $viewModel.searchText, prompt: "Search")
            .overlay {
                if viewModel.filteredChats.isEmpty {
                    ShareStateView(
                        title: "No Matches",
                        message: "Try a different search term.",
                        systemImage: "magnifyingglass"
                    )
                }
            }
        }
    }
}

private struct ShareStateView: View {
    let title: String
    let message: String
    let systemImage: String
    var actionTitle: String?
    var action: (() -> Void)?

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: systemImage)
                .font(.system(size: 38))
                .foregroundStyle(.secondary)
            Text(title)
                .font(.headline)
            Text(message)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            if let actionTitle, let action {
                Button(actionTitle, action: action)
                    .buttonStyle(.borderedProminent)
                    .padding(.top, 8)
            }
        }
    }
}

private struct ShareQueueProgressView: View {
    let stage: ShareSendStage
    let progress: Double

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: stage == .queued ? "checkmark.circle.fill" : "paperplane.circle.fill")
                .font(.system(size: 40))
                .foregroundStyle(stage == .queued ? .green : .blue)

            Text(title)
                .font(.headline)

            Text(message)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)

            ProgressView(value: progress, total: 1)
                .progressViewStyle(.linear)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
    }

    private var title: String {
        switch stage {
        case .preparing:
            return "Preparing Share"
        case .queueing:
            return "Sending To Queue"
        case .queued:
            return "Queued"
        case .idle:
            return ""
        }
    }

    private var message: String {
        switch stage {
        case .preparing:
            return "Packaging content for background delivery."
        case .queueing:
            return "Saving your share request."
        case .queued:
            return "Your share was queued and will send when Pika processes pending shares."
        case .idle:
            return ""
        }
    }
}

private struct SharePayloadPreview: View {
    let payload: ShareIncomingPayload?

    var body: some View {
        if let payload {
            switch payload {
            case .text(let text):
                Text(text)
                    .lineLimit(4)
            case .url(let text):
                Text(text)
                    .lineLimit(4)
                    .foregroundStyle(.blue)
            case .image(let data, _, _):
                if let image = UIImage(data: data) {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFit()
                        .frame(maxHeight: 140)
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                } else {
                    Label("Image", systemImage: "photo")
                }
            }
        } else {
            Text("No content")
                .foregroundStyle(.secondary)
        }
    }
}

private struct ShareChatRow: View {
    let chat: ShareableChatSummary
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 12) {
            avatar
            VStack(alignment: .leading, spacing: 2) {
                Text(chat.displayName)
                    .font(.headline)
                    .lineLimit(1)
                Text(chat.lastMessagePreview)
                    .foregroundStyle(.secondary)
                    .font(.subheadline)
                    .lineLimit(1)
            }

            Spacer(minLength: 0)

            Image(systemName: isSelected ? "checkmark.circle.fill" : "circle")
                .foregroundStyle(isSelected ? Color.blue : Color.secondary)
        }
    }

    @ViewBuilder
    private var avatar: some View {
        if chat.isGroup {
            ZStack {
                Circle()
                    .fill(Color.blue.opacity(0.15))
                    .frame(width: 36, height: 36)
                Image(systemName: "person.3.fill")
                    .font(.system(size: 14))
                    .foregroundStyle(.blue)
            }
        } else {
            let initials = initials(for: chat)
            ZStack {
                Circle()
                    .fill(Color.gray.opacity(0.2))
                    .frame(width: 36, height: 36)
                Text(initials)
                    .font(.caption)
                    .fontWeight(.semibold)
                    .foregroundStyle(.primary)
            }
        }
    }

    private func initials(for chat: ShareableChatSummary) -> String {
        let candidate = chat.members.first?.name?.trimmingCharacters(in: .whitespacesAndNewlines)
        let source = (candidate?.isEmpty == false) ? candidate! : chat.displayName
        let words = source.split(separator: " ").prefix(2)
        let chars = words.compactMap { $0.first }
        if chars.isEmpty {
            return "?"
        }
        return String(chars).uppercased()
    }
}

private enum ShareSendStage: Equatable {
    case idle
    case preparing
    case queueing
    case queued
}

@MainActor
private final class ShareExtensionViewModel: ObservableObject {
    @Published private(set) var chats: [ShareableChatSummary] = []
    @Published private(set) var payload: ShareIncomingPayload?
    @Published private(set) var isLoadingPayload = false
    @Published private(set) var isSending = false
    @Published var selectedChatId: String?
    @Published var searchText = ""
    @Published var composeText = ""
    @Published var errorMessage: String?
    @Published private(set) var sendStage: ShareSendStage = .idle
    @Published private(set) var sendProgress: Double = 0

    var filteredChats: [ShareableChatSummary] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return chats }
        return chats.filter { chat in
            if chat.displayName.localizedCaseInsensitiveContains(query) {
                return true
            }
            return chat.lastMessagePreview.localizedCaseInsensitiveContains(query)
        }
    }

    var isLoggedIn: Bool {
        ShareQueueManager.isLoggedIn()
    }

    var canSend: Bool {
        isLoggedIn && sendStage == .idle && !isSending && payload != nil && selectedChatId != nil
    }

    var isShowingQueueProgress: Bool {
        sendStage != .idle
    }

    func load(from context: NSExtensionContext?) {
        chats = ShareQueueManager.readChatListCache()
        if selectedChatId == nil {
            selectedChatId = chats.first?.chatId
        }
        sendStage = .idle
        sendProgress = 0

        guard isLoggedIn else {
            payload = nil
            errorMessage = nil
            isLoadingPayload = false
            return
        }

        isLoadingPayload = true
        Task {
            let result = await ShareIncomingPayload.extract(from: context)
            payload = result.payload
            errorMessage = result.errorMessage
            isLoadingPayload = false
        }
    }

    func enqueueSelectedShare() async -> Bool {
        guard let chatId = selectedChatId, let payload else {
            return false
        }

        isSending = true
        defer { isSending = false }

        sendStage = .preparing
        sendProgress = 0.2
        try? await Task.sleep(nanoseconds: 180_000_000)

        do {
            let trimmedCompose = composeText.trimmingCharacters(in: .whitespacesAndNewlines)
            let createdAtMs = UInt64(Date().timeIntervalSince1970 * 1000)
            let requestId = UUID().uuidString
            let request: ShareEnqueueRequest

            sendStage = .queueing
            sendProgress = 0.65

            switch payload {
            case .text(let text):
                request = ShareEnqueueRequest(
                    chatId: chatId,
                    composeText: trimmedCompose,
                    payloadKind: .text,
                    payloadText: text,
                    mediaRelativePath: nil,
                    mediaMimeType: nil,
                    mediaFilename: nil,
                    clientRequestId: requestId,
                    createdAtMs: createdAtMs
                )

            case .url(let text):
                request = ShareEnqueueRequest(
                    chatId: chatId,
                    composeText: trimmedCompose,
                    payloadKind: .url,
                    payloadText: text,
                    mediaRelativePath: nil,
                    mediaMimeType: nil,
                    mediaFilename: nil,
                    clientRequestId: requestId,
                    createdAtMs: createdAtMs
                )

            case .image(let data, let mimeType, let filename):
                let mediaPath = try ShareQueueManager.saveMedia(
                    data,
                    preferredFilename: filename,
                    defaultExtension: "jpg"
                )
                request = ShareEnqueueRequest(
                    chatId: chatId,
                    composeText: trimmedCompose,
                    payloadKind: .image,
                    payloadText: nil,
                    mediaRelativePath: mediaPath,
                    mediaMimeType: mimeType,
                    mediaFilename: filename,
                    clientRequestId: requestId,
                    createdAtMs: createdAtMs
                )
            }

            _ = try ShareQueueManager.enqueue(request)
            sendProgress = 1
            sendStage = .queued
            return true
        } catch {
            errorMessage = "Could not queue share. Please try again."
            sendStage = .idle
            sendProgress = 0
            return false
        }
    }
}

private enum ShareIncomingPayload {
    case text(String)
    case url(String)
    case image(Data, mimeType: String, filename: String)

    struct ExtractResult {
        let payload: ShareIncomingPayload?
        let errorMessage: String?
    }

    static func extract(from context: NSExtensionContext?) async -> ExtractResult {
        guard let context else {
            return ExtractResult(payload: nil, errorMessage: "No share context available.")
        }

        let providers = context.inputItems
            .compactMap { $0 as? NSExtensionItem }
            .flatMap { $0.attachments ?? [] }

        if let provider = providers.first(where: { $0.hasItemConformingToTypeIdentifier(UTType.image.identifier) }) {
            do {
                if let imagePayload = try await loadImage(from: provider) {
                    return ExtractResult(payload: imagePayload, errorMessage: nil)
                }
            } catch {
                return ExtractResult(payload: nil, errorMessage: "Could not load the selected image.")
            }
        }

        if let provider = providers.first(where: { $0.hasItemConformingToTypeIdentifier(UTType.url.identifier) }) {
            do {
                if let value = try await loadString(from: provider, type: .url), !value.isEmpty {
                    return ExtractResult(payload: .url(value), errorMessage: nil)
                }
            } catch {
                return ExtractResult(payload: nil, errorMessage: "Could not load the selected link.")
            }
        }

        if let provider = providers.first(where: { $0.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) }) {
            do {
                if let value = try await loadString(from: provider, type: .plainText), !value.isEmpty {
                    return ExtractResult(payload: .text(value), errorMessage: nil)
                }
            } catch {
                return ExtractResult(payload: nil, errorMessage: "Could not load the selected text.")
            }
        }

        return ExtractResult(payload: nil, errorMessage: "Pika supports sharing text, links, and images.")
    }

    private static func loadString(from provider: NSItemProvider, type: UTType) async throws -> String? {
        let raw = try await loadItem(from: provider, type: type)

        if let url = raw as? URL {
            return url.absoluteString
        }
        if let text = raw as? String {
            return text
        }
        if let text = raw as? NSString {
            return text as String
        }
        return nil
    }

    private static func loadImage(from provider: NSItemProvider) async throws -> ShareIncomingPayload? {
        // Use loadFileRepresentation to get a file URL without decoding the image.
        // This is the preferred path: ImageIO can downsample directly from the file,
        // never loading the full bitmap into memory (~120MB extension limit).
        if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) {
            if let jpeg = try await loadFileAndDownsample(from: provider) {
                let filename = sanitizedFilename(from: provider.suggestedName)
                return .image(jpeg, mimeType: "image/jpeg", filename: filename)
            }
        }

        // Fallback: loadItem can return URL, Data, or UIImage.
        let raw = try await loadItem(from: provider, type: .image)

        if let url = raw as? URL,
           let jpeg = downsampledJPEGData(fromURL: url) {
            return .image(jpeg, mimeType: "image/jpeg", filename: sanitizedFilename(from: url.lastPathComponent))
        }

        if let data = raw as? Data,
           let jpeg = downsampledJPEGData(fromData: data) {
            return .image(jpeg, mimeType: "image/jpeg", filename: sanitizedFilename(from: provider.suggestedName))
        }

        // If the provider returned a UIImage, the bitmap is already in memory.
        // Use jpegData (not pngData) to serialize it with minimal extra allocation,
        // then downsample via ImageIO.
        if let image = raw as? UIImage,
           let jpegData = image.jpegData(compressionQuality: 0.9),
           let jpeg = downsampledJPEGData(fromData: jpegData) {
            return .image(jpeg, mimeType: "image/jpeg", filename: sanitizedFilename(from: provider.suggestedName))
        }

        return nil
    }

    // MARK: - Image helpers

    private static func loadFileAndDownsample(from provider: NSItemProvider) async throws -> Data? {
        try await withCheckedThrowingContinuation { continuation in
            provider.loadFileRepresentation(forTypeIdentifier: UTType.image.identifier) { url, error in
                if let error {
                    continuation.resume(throwing: error)
                    return
                }
                guard let url else {
                    continuation.resume(returning: nil)
                    return
                }
                // The file is temporary and deleted after this callback returns,
                // so we must downsample synchronously here.
                let result = downsampledJPEGData(fromURL: url)
                continuation.resume(returning: result)
            }
        }
    }

    private static func downsampledJPEGData(fromURL url: URL) -> Data? {
        let sourceOptions: [CFString: Any] = [kCGImageSourceShouldCache: false]
        guard let source = CGImageSourceCreateWithURL(url as CFURL, sourceOptions as CFDictionary) else { return nil }
        return downsampledJPEGData(from: source)
    }

    private static func downsampledJPEGData(fromData data: Data) -> Data? {
        let sourceOptions: [CFString: Any] = [kCGImageSourceShouldCache: false]
        guard let source = CGImageSourceCreateWithData(data as CFData, sourceOptions as CFDictionary) else { return nil }
        return downsampledJPEGData(from: source)
    }

    private static func downsampledJPEGData(from source: CGImageSource, maxDimension: CGFloat = 2048) -> Data? {
        let downsampleOptions: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxDimension,
        ]
        guard let cgImage = CGImageSourceCreateThumbnailAtIndex(source, 0, downsampleOptions as CFDictionary) else { return nil }
        return UIImage(cgImage: cgImage).jpegData(compressionQuality: 0.85)
    }

    private static func loadItem(from provider: NSItemProvider, type: UTType) async throws -> NSSecureCoding? {
        try await withCheckedThrowingContinuation { continuation in
            provider.loadItem(forTypeIdentifier: type.identifier, options: nil) { item, error in
                if let error {
                    continuation.resume(throwing: error)
                    return
                }
                continuation.resume(returning: item)
            }
        }
    }


    private static func sanitizedFilename(from proposed: String?) -> String {
        let raw = (proposed ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let base = URL(fileURLWithPath: raw.isEmpty ? "shared-image" : raw).deletingPathExtension().lastPathComponent
        let safe = base.replacingOccurrences(
            of: "[^A-Za-z0-9._-]",
            with: "-",
            options: .regularExpression
        )
        let finalBase = safe.isEmpty ? "shared-image" : safe
        return "\(finalBase).jpg"
    }
}
