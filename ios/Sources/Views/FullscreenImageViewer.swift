import SwiftUI

extension ChatMediaAttachment: Identifiable {
    public var id: String { originalHashHex }
}

struct FullscreenImageViewer: View {
    let attachments: [ChatMediaAttachment]
    @State var currentId: String
    let onDismiss: () -> Void
    @State private var dragOffset: CGSize = .zero
    @State private var isDismissing = false
    @State private var backgroundOpacity: Double = 0.0
    @State private var zoomScales: [String: CGFloat] = [:]

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
                .offset(x: dragOffset.width, y: dragOffset.height)
                .simultaneousGesture(dismissGesture)
            }
            .overlay(alignment: .top) {
                controlsBar(topInset: geo.safeAreaInsets.top)
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

    private func controlsBar(topInset: CGFloat) -> some View {
        HStack {
            Button { performDismiss() } label: {
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

    private func performDismiss() {
        withAnimation(.easeOut(duration: 0.2)) {
            backgroundOpacity = 0
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
            onDismiss()
        }
    }

    // MARK: - Dismiss gesture

    private var dismissGesture: some Gesture {
        DragGesture(minimumDistance: 20)
            .onChanged { value in
                guard !isZoomed else { return }

                if !isDismissing {
                    guard abs(value.translation.height)
                        > abs(value.translation.width)
                    else { return }
                    isDismissing = true
                }

                dragOffset = value.translation
                let distance = hypot(
                    value.translation.width, value.translation.height)
                backgroundOpacity = max(0, 1.0 - distance / 300)
            }
            .onEnded { value in
                guard isDismissing else { return }

                let distance = hypot(
                    value.translation.width, value.translation.height)
                let predicted = value.predictedEndTranslation
                let predictedDistance = hypot(
                    predicted.width, predicted.height)

                if distance > 100 || predictedDistance > 300 {
                    withAnimation(.easeOut(duration: 0.2)) {
                        dragOffset = predicted
                        backgroundOpacity = 0
                    }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
                        onDismiss()
                    }
                } else {
                    isDismissing = false
                    withAnimation(.spring(response: 0.3, dampingFraction: 0.8))
                    {
                        dragOffset = .zero
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
