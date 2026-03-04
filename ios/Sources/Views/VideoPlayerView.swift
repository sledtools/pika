import SwiftUI
import AVKit

// MARK: - Video attachment view

struct VideoAttachmentView: View {
    let attachment: ChatMediaAttachment
    let isMine: Bool
    var maxMediaWidth: CGFloat = 240
    var maxMediaHeight: CGFloat = .infinity
    var onDownload: (() -> Void)? = nil

    @State private var thumbnail: UIImage?
    @State private var showPlayer = false

    private var aspectRatio: CGFloat {
        if let w = attachment.width, let h = attachment.height, w > 0, h > 0 {
            return CGFloat(w) / CGFloat(h)
        }
        return 16.0 / 9.0
    }

    private var videoSize: CGSize {
        let w = maxMediaWidth
        let h = w / aspectRatio
        if h > maxMediaHeight {
            return CGSize(width: maxMediaHeight * aspectRatio, height: maxMediaHeight)
        }
        return CGSize(width: w, height: h)
    }

    var body: some View {
        if let localPath = attachment.localPath {
            thumbnailView(localPath: localPath)
                .overlay {
                    if attachment.uploadProgress != nil {
                        UploadProgressOverlay()
                    } else {
                        // Play button overlay
                        Image(systemName: "play.circle.fill")
                            .font(.system(size: 44))
                            .foregroundStyle(.white.opacity(0.9))
                            .shadow(radius: 4)
                    }
                }
                .contentShape(Rectangle())
                .onTapGesture {
                    guard attachment.uploadProgress == nil else { return }
                    showPlayer = true
                }
                .fullScreenCover(isPresented: $showPlayer) {
                    VideoPlayerSheet(
                        url: URL(fileURLWithPath: localPath),
                        isPresented: $showPlayer
                    )
                }
        } else {
            // Auto-downloading: show placeholder with spinner
            ZStack {
                placeholder
                ProgressView().tint(.white)
            }
            .frame(width: videoSize.width, height: videoSize.height)
        }
    }

    @ViewBuilder
    private func thumbnailView(localPath: String) -> some View {
        if let thumbnail {
            Image(uiImage: thumbnail)
                .resizable()
                .scaledToFill()
                .frame(width: videoSize.width, height: videoSize.height)
                .clipped()
        } else {
            placeholder
                .frame(width: videoSize.width, height: videoSize.height)
                .onAppear {
                    generateThumbnail(from: localPath)
                }
        }
    }

    private var placeholder: some View {
        Rectangle()
            .fill(isMine ? Color.white.opacity(0.15) : Color.gray.opacity(0.2))
            .frame(width: videoSize.width, height: videoSize.height)
    }

    private func generateThumbnail(from path: String) {
        let url = URL(fileURLWithPath: path)
        Task.detached(priority: .userInitiated) {
            let asset = AVURLAsset(url: url)
            let generator = AVAssetImageGenerator(asset: asset)
            generator.appliesPreferredTrackTransform = true
            generator.maximumSize = CGSize(width: 480, height: 480)

            do {
                let cgImage = try generator.copyCGImage(at: .zero, actualTime: nil)
                let uiImage = UIImage(cgImage: cgImage)
                await MainActor.run {
                    thumbnail = uiImage
                }
            } catch {
                // Thumbnail generation failed — placeholder stays
            }
        }
    }
}

// MARK: - Fullscreen video player

struct VideoPlayerSheet: View {
    let url: URL
    @Binding var isPresented: Bool
    @State private var player: AVPlayer?

    var body: some View {
        ZStack(alignment: .topLeading) {
            Color.black.ignoresSafeArea()

            if let player {
                VideoPlayer(player: player)
                    .ignoresSafeArea()
            }

            Button {
                player?.pause()
                isPresented = false
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(.title)
                    .foregroundStyle(.white.opacity(0.8))
                    .padding(16)
            }
        }
        .onAppear {
            let avPlayer = AVPlayer(url: url)
            player = avPlayer
            avPlayer.play()
        }
        .onDisappear {
            player?.pause()
            player = nil
        }
    }
}
