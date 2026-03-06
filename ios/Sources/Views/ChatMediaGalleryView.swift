import SwiftUI

struct ChatMediaGalleryView: View {
    let chatId: String
    let items: [MediaGalleryItem]

    @State private var fullscreenAttachment: ChatMediaAttachment?

    private var viewableAttachments: [ChatMediaAttachment] {
        items.compactMap { $0.attachment.localPath != nil ? $0.attachment : nil }
    }

    private let columns = [
        GridItem(.flexible(), spacing: 2),
        GridItem(.flexible(), spacing: 2),
        GridItem(.flexible(), spacing: 2),
    ]

    var body: some View {
        Group {
            if items.isEmpty {
                VStack(spacing: 12) {
                    Image(systemName: "photo.on.rectangle.angled")
                        .font(.system(size: 48))
                        .foregroundStyle(.secondary)
                    Text("No Media")
                        .font(.title3)
                        .fontWeight(.semibold)
                    Text("Photos and videos shared in this chat will appear here.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding()
            } else {
                ScrollView {
                    LazyVGrid(columns: columns, spacing: 2) {
                        ForEach(items, id: \.attachment.originalHashHex) { item in
                            mediaThumbnail(item)
                        }
                    }
                }
            }
        }
        .navigationTitle("Media")
        .navigationBarTitleDisplayMode(.inline)
        .overFullScreenCover(item: $fullscreenAttachment) { attachment in
            FullscreenImageViewer(
                attachments: viewableAttachments,
                selected: attachment,
                onDismiss: { fullscreenAttachment = nil }
            )
        }
    }

    @ViewBuilder
    private func mediaThumbnail(_ item: MediaGalleryItem) -> some View {
        let attachment = item.attachment
        if let localPath = attachment.localPath {
            ThumbnailImage(url: URL(fileURLWithPath: localPath))
                .overlay(
                    GeometryReader { geo in
                        Color.clear
                            .contentShape(Rectangle())
                            .onTapGesture {
                                ImageViewerTransition.sourceFrame = geo.frame(in: .global)
                                fullscreenAttachment = attachment
                            }
                    }
                )
        } else {
            Rectangle()
                .fill(Color(.systemGray5))
                .aspectRatio(1, contentMode: .fit)
                .overlay {
                    Image(systemName: "photo")
                        .foregroundStyle(.secondary)
                }
        }
    }
}

/// Loads a local image thumbnail asynchronously to avoid blocking the main thread.
private struct ThumbnailImage: View {
    let url: URL
    @State private var image: UIImage?

    var body: some View {
        Color(.systemGray5)
            .aspectRatio(1, contentMode: .fit)
            .overlay {
                if let image {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFill()
                }
            }
            .clipped()
        .task(id: url) {
            if let cached = ImageCache.shared.image(for: url) {
                self.image = cached
                return
            }
            let fileUrl = url
            let loaded = await Task.detached {
                guard let data = try? Data(contentsOf: fileUrl),
                      let img = UIImage(data: data) else { return nil as UIImage? }
                return img
            }.value
            if let loaded {
                ImageCache.shared.setImage(loaded, for: url)
                self.image = loaded
            }
        }
    }
}
