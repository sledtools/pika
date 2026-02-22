import SwiftUI

extension ChatMediaAttachment: Identifiable {
    public var id: String { originalHashHex }
}

struct FullscreenImageViewer: View {
    let attachment: ChatMediaAttachment
    @Environment(\.dismiss) private var dismiss
    @State private var dragOffset: CGSize = .zero
    @State private var backgroundOpacity: Double = 1.0

    var body: some View {
        let dragProgress = min(abs(dragOffset.height) / 300, 1.0)

        NavigationStack {
            GeometryReader { geo in
                ZStack {
                    Color.black
                        .ignoresSafeArea()

                    if let localPath = attachment.localPath {
                        CachedAsyncImage(url: URL(fileURLWithPath: localPath)) { image in
                            image
                                .resizable()
                                .scaledToFit()
                                .frame(maxWidth: geo.size.width, maxHeight: geo.size.height)
                        } placeholder: {
                            ProgressView()
                                .tint(.white)
                        }
                        .offset(dragOffset)
                        .scaleEffect(1.0 - dragProgress * 0.2)
                        .gesture(
                            DragGesture()
                                .onChanged { value in
                                    dragOffset = value.translation
                                    let progress = min(abs(value.translation.height) / 300, 1.0)
                                    backgroundOpacity = 1.0 - progress
                                }
                                .onEnded { value in
                                    if abs(value.translation.height) > 100 {
                                        dismiss()
                                    } else {
                                        withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) {
                                            dragOffset = .zero
                                            backgroundOpacity = 1.0
                                        }
                                    }
                                }
                        )
                    }
                }
                .opacity(backgroundOpacity)
            }
            .navigationTitle(attachment.filename)
            .navigationBarTitleDisplayMode(.inline)
            .toolbarColorScheme(.dark, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbarBackground(Color.black, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button {
                        dismiss()
                    } label: {
                        Image(systemName: "xmark")
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    ShareLink(item: URL(fileURLWithPath: attachment.localPath ?? "")) {
                        Image(systemName: "square.and.arrow.up")
                    }
                }
            }
        }
    }
}
