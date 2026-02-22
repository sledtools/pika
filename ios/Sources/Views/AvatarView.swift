import SwiftUI

struct AvatarView: View {
    let name: String?
    let npub: String
    let pictureUrl: String?
    var size: CGFloat = 44

    var body: some View {
        if let url = pictureUrl.flatMap({ URL(string: $0) }) {
            CachedAsyncImage(url: url) { image in
                image.resizable().scaledToFill()
            } placeholder: {
                initialsCircle
            }
            .frame(width: size, height: size)
            .clipShape(Circle())
        } else {
            initialsCircle
        }
    }

    private var initialsCircle: some View {
        Circle()
            .fill(Color.blue.opacity(0.12))
            .frame(width: size, height: size)
            .overlay {
                Text(initials)
                    .font(.system(size: size * 0.4, weight: .medium))
                    .foregroundStyle(.blue)
            }
    }

    private var initials: String {
        let source = name ?? npub
        return String(source.prefix(1)).uppercased()
    }
}

// MARK: - Cached image loader

final class ImageCache: @unchecked Sendable {
    static let shared = ImageCache()
    private let cache = NSCache<NSURL, UIImage>()

    init() {
        cache.countLimit = 200
    }

    func image(for url: URL) -> UIImage? {
        cache.object(forKey: url as NSURL)
    }

    func setImage(_ image: UIImage, for url: URL) {
        cache.setObject(image, forKey: url as NSURL)
    }
}

@MainActor
final class ImageLoader: ObservableObject {
    @Published var image: UIImage?
    private var url: URL?
    private var task: Task<Void, Never>?

    func load(url: URL) {
        guard self.url != url else { return }
        self.url = url
        task?.cancel()

        if let cached = ImageCache.shared.image(for: url) {
            self.image = cached
            return
        }

        // Local files: read synchronously (tiny resized JPEGs, ~40KB).
        if url.isFileURL {
            if let data = try? Data(contentsOf: url),
               let uiImage = UIImage(data: data) {
                ImageCache.shared.setImage(uiImage, for: url)
                self.image = uiImage
            }
            return
        }

        task = Task {
            do {
                let (data, _) = try await URLSession.shared.data(from: url)
                guard !Task.isCancelled, let uiImage = UIImage(data: data) else { return }
                ImageCache.shared.setImage(uiImage, for: url)
                self.image = uiImage
            } catch {
                // Keep showing placeholder on failure
            }
        }
    }
}

struct CachedAsyncImage<Content: View, Placeholder: View>: View {
    let url: URL
    @ViewBuilder let content: (Image) -> Content
    @ViewBuilder let placeholder: () -> Placeholder

    @StateObject private var loader = ImageLoader()

    var body: some View {
        Group {
            if let uiImage = loader.image {
                content(Image(uiImage: uiImage))
            } else {
                placeholder()
            }
        }
        .onAppear { loader.load(url: url) }
        .onChange(of: url) { _, newUrl in loader.load(url: newUrl) }
    }
}

#if DEBUG
#Preview("Avatar - Initials") {
    AvatarView(name: "Pika", npub: "npub1example", pictureUrl: nil, size: 56)
        .padding()
}
#endif
