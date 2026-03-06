import SwiftUI

extension ChatMediaAttachment: Identifiable {
    public var id: String { originalHashHex }
}

enum ImageViewerTransition {
    @MainActor static var sourceFrame: CGRect = .zero
}

struct FullscreenImageViewer: View {
    let attachments: [ChatMediaAttachment]
    @State var currentId: String
    let onDismiss: () -> Void
    @State private var dragOffset: CGSize = .zero
    @State private var isDismissing = false
    @State private var backgroundOpacity: Double = 0.0
    @State private var dismissScale: CGFloat = 1.0
    @State private var zoomScales: [String: CGFloat] = [:]
    private let sourceFrame = ImageViewerTransition.sourceFrame

    private var isZoomed: Bool {
        (zoomScales[currentId] ?? 1.0) > 1.01
    }

    init(attachment: ChatMediaAttachment, onDismiss: @escaping () -> Void) {
        self.attachments = [attachment]
        self._currentId = State(initialValue: attachment.id)
        self.onDismiss = onDismiss
    }

    init(
        attachments: [ChatMediaAttachment], selected: ChatMediaAttachment,
        onDismiss: @escaping () -> Void
    ) {
        self.attachments = attachments
        self._currentId = State(initialValue: selected.id)
        self.onDismiss = onDismiss
    }

    private var currentAttachment: ChatMediaAttachment? {
        attachments.first { $0.id == currentId } ?? attachments.first
    }

    var body: some View {
        GeometryReader { geo in
            ZStack {
                Color.black
                    .opacity(backgroundOpacity)
                    .ignoresSafeArea()

                TabView(selection: $currentId) {
                    ForEach(attachments) { attachment in
                        imageContent(attachment: attachment, geo: geo)
                            .tag(attachment.id)
                    }
                }
                .tabViewStyle(
                    .page(
                        indexDisplayMode: attachments.count > 1
                            ? .automatic : .never))
                .mask(dismissClipMask(geo: geo))
                .scaleEffect(dismissScale)
                .offset(x: dragOffset.width, y: dragOffset.height)
                .simultaneousGesture(makeDismissGesture(geo: geo))
            }
            .overlay(alignment: .top) {
                controlsBar(topInset: geo.safeAreaInsets.top, geo: geo)
                    .opacity(isDismissing ? 0 : backgroundOpacity)
            }
        }
        .ignoresSafeArea()
        .preferredColorScheme(.dark)
        .onAppear {
            withAnimation(.easeIn(duration: 0.2)) {
                backgroundOpacity = 1.0
            }
        }
    }

    // MARK: - Controls

    private func controlsBar(topInset: CGFloat, geo: GeometryProxy) -> some View {
        HStack {
            Button { performDismiss(geo: geo) } label: {
                Image(systemName: "xmark")
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.white)
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }

            Spacer()

            Text(currentAttachment?.filename ?? "")
                .font(.headline)
                .foregroundStyle(.white)
                .lineLimit(1)

            Spacer()

            ShareLink(
                item: URL(
                    fileURLWithPath: currentAttachment?.localPath ?? "")
            ) {
                Image(systemName: "square.and.arrow.up")
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.white)
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
        }
        .padding(.horizontal, 8)
        .padding(.top, topInset)
        .background(
            LinearGradient(
                colors: [.black.opacity(0.5), .clear],
                startPoint: .top,
                endPoint: .bottom
            )
            .ignoresSafeArea(edges: .top)
        )
    }

    private func performDismiss(geo: GeometryProxy) {
        animateDismissToSource(geo: geo)
    }

    private func animateDismissToSource(geo: GeometryProxy) {
        let screenCenter = CGPoint(
            x: geo.size.width / 2, y: geo.size.height / 2)

        if sourceFrame != .zero {
            let targetScale = sourceFrame.width / geo.size.width
            let targetOffset = CGSize(
                width: sourceFrame.midX - screenCenter.x,
                height: sourceFrame.midY - screenCenter.y)
            withAnimation(.easeInOut(duration: 0.25)) {
                dragOffset = targetOffset
                dismissScale = targetScale
                backgroundOpacity = 0
            }
        } else {
            withAnimation(.easeOut(duration: 0.2)) {
                backgroundOpacity = 0
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
            onDismiss()
        }
    }

    // MARK: - Dismiss clip mask

    private func dismissClipMask(geo: GeometryProxy) -> some View {
        let hasSource = sourceFrame != .zero
        let targetScale = hasSource
            ? max(sourceFrame.width / geo.size.width, 0.05) : 1.0
        let progress: CGFloat =
            (hasSource && targetScale < 0.99)
            ? min(max((1.0 - dismissScale) / (1.0 - targetScale), 0), 1.0)
            : 0.0

        let screenW = geo.size.width
        let screenH = geo.size.height
        let sourceAspect = sourceFrame.width / max(sourceFrame.height, 1)
        let screenAspect = screenW / screenH

        let targetClipW = sourceAspect > screenAspect ? screenW : screenH * sourceAspect
        let targetClipH = sourceAspect > screenAspect ? screenW / sourceAspect : screenH

        let clipW = screenW + progress * (targetClipW - screenW)
        let clipH = screenH + progress * (targetClipH - screenH)
        let cornerRadius = progress * 12

        return RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
            .frame(width: clipW, height: clipH)
    }

    // MARK: - Dismiss gesture

    private func makeDismissGesture(geo: GeometryProxy) -> some Gesture {
        let screenCenter = CGPoint(
            x: geo.size.width / 2, y: geo.size.height / 2)
        let hasSource = sourceFrame != .zero
        let targetScale = hasSource
            ? max(sourceFrame.width / geo.size.width, 0.05) : 0.8

        return DragGesture(minimumDistance: 20)
            .onChanged { value in
                guard !isZoomed else { return }

                if !isDismissing {
                    guard abs(value.translation.height)
                        > abs(value.translation.width)
                    else { return }
                    isDismissing = true
                }

                let distance = hypot(
                    value.translation.width, value.translation.height)
                let t = min(distance / 300, 1.0)

                dragOffset = value.translation
                backgroundOpacity = max(0, 1.0 - t)
                dismissScale = 1.0 - t * (1.0 - targetScale)
            }
            .onEnded { value in
                guard isDismissing else { return }

                let distance = hypot(
                    value.translation.width, value.translation.height)
                let predicted = value.predictedEndTranslation
                let predictedDistance = hypot(
                    predicted.width, predicted.height)

                if distance > 100 || predictedDistance > 300 {
                    if hasSource {
                        let targetOffset = CGSize(
                            width: sourceFrame.midX - screenCenter.x,
                            height: sourceFrame.midY - screenCenter.y)
                        withAnimation(.easeInOut(duration: 0.25)) {
                            dragOffset = targetOffset
                            dismissScale = targetScale
                            backgroundOpacity = 0
                        }
                    } else {
                        withAnimation(.easeOut(duration: 0.2)) {
                            dragOffset = predicted
                            backgroundOpacity = 0
                        }
                    }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                        onDismiss()
                    }
                } else {
                    isDismissing = false
                    withAnimation(.spring(response: 0.3, dampingFraction: 0.8))
                    {
                        dragOffset = .zero
                        dismissScale = 1.0
                        backgroundOpacity = 1.0
                    }
                }
            }
    }

    // MARK: - Image content

    @ViewBuilder
    private func imageContent(
        attachment: ChatMediaAttachment, geo: GeometryProxy
    ) -> some View {
        if let localPath = attachment.localPath {
            ImagePage(localPath: localPath) { scale in
                zoomScales[attachment.id] = scale
            }
            .frame(maxWidth: geo.size.width, maxHeight: geo.size.height)
        } else {
            ProgressView()
                .tint(.white)
        }
    }
}

// MARK: - ImagePage

private struct ImagePage: View {
    let localPath: String
    let onZoomScaleChange: (CGFloat) -> Void
    @State private var image: UIImage?

    var body: some View {
        Group {
            if let image {
                ZoomableImageView(
                    image: image, onZoomScaleChange: onZoomScaleChange)
            } else {
                ProgressView()
                    .tint(.white)
            }
        }
        .task {
            image = UIImage(contentsOfFile: localPath)
        }
    }
}

// MARK: - Over Full Screen Presentation

extension View {
    func overFullScreenCover<Item: Identifiable, Content: View>(
        item: Binding<Item?>,
        @ViewBuilder content: @escaping (Item) -> Content
    ) -> some View {
        background(
            OverFullScreenPresenter(item: item, sheetContent: content)
        )
    }
}

private struct OverFullScreenPresenter<Item: Identifiable, SheetContent: View>:
    UIViewControllerRepresentable
{
    @Binding var item: Item?
    let sheetContent: (Item) -> SheetContent

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIViewController(context: Context) -> UIViewController {
        let vc = UIViewController()
        vc.view.backgroundColor = .clear
        return vc
    }

    func updateUIViewController(
        _ controller: UIViewController, context: Context
    ) {
        if let item = item, context.coordinator.hosting == nil {
            let hosting = UIHostingController(
                rootView: AnyView(sheetContent(item)))
            hosting.modalPresentationStyle = .overFullScreen
            hosting.view.backgroundColor = .clear
            context.coordinator.hosting = hosting
            DispatchQueue.main.async {
                guard controller.presentedViewController == nil else { return }
                controller.present(hosting, animated: false)
            }
        } else if let item = item, let hosting = context.coordinator.hosting {
            hosting.rootView = AnyView(sheetContent(item))
        } else if item == nil, let hosting = context.coordinator.hosting {
            hosting.dismiss(animated: false)
            context.coordinator.hosting = nil
        }
    }

    class Coordinator {
        var hosting: UIHostingController<AnyView>?
    }
}
