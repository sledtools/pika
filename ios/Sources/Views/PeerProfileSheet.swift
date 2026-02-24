import CoreImage
import CoreImage.CIFilterBuiltins
import SwiftUI
import UIKit

struct PeerProfileSheet: View {
    let profile: PeerProfileState
    let onFollow: @MainActor () -> Void
    let onUnfollow: @MainActor () -> Void
    let onClose: @MainActor () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var didCopyNpub = false
    @State private var copyResetTask: Task<Void, Never>?

    var body: some View {
        NavigationStack {
            List {
                avatarSection
                nameSection
                npubSection
                qrSection
                followSection
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Profile")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Close") {
                        onClose()
                        dismiss()
                    }
                }
            }
            .onDisappear {
                copyResetTask?.cancel()
            }
        }
    }

    @ViewBuilder
    private var avatarSection: some View {
        Section {
            VStack(spacing: 8) {
                AvatarView(
                    name: profile.name,
                    npub: profile.npub,
                    pictureUrl: profile.pictureUrl,
                    size: 96
                )
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 6)
        }
    }

    @ViewBuilder
    private var nameSection: some View {
        if profile.name != nil || profile.about != nil || profile.avatarModelUrl != nil {
            Section("Profile") {
                if let name = profile.name {
                    HStack {
                        Text("Name")
                            .foregroundStyle(.secondary)
                        Spacer()
                        Text(name)
                    }
                }
                if let about = profile.about {
                    Text(about)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                if let avatarUrl = profile.avatarModelUrl {
                    HStack {
                        Text("Avatar")
                            .foregroundStyle(.secondary)
                        Spacer()
                        Text(avatarUrl)
                            .font(.caption)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .foregroundStyle(.blue)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var npubSection: some View {
        Section {
            HStack(alignment: .center, spacing: 12) {
                Text(profile.npub)
                    .font(.system(.footnote, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)

                Spacer()

                HStack(spacing: 8) {
                    if didCopyNpub {
                        Text("Copied")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.green)
                    }
                    Button {
                        UIPasteboard.general.string = profile.npub
                        didCopyNpub = true
                        copyResetTask?.cancel()
                        copyResetTask = Task { @MainActor in
                            try? await Task.sleep(nanoseconds: 1_200_000_000)
                            didCopyNpub = false
                        }
                    } label: {
                        Image(systemName: didCopyNpub ? "checkmark.circle.fill" : "doc.on.doc")
                            .font(.body.weight(.semibold))
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                }
                .animation(.easeInOut(duration: 0.15), value: didCopyNpub)
            }
        } header: {
            Text("Public Key")
        }
    }

    @ViewBuilder
    private var qrSection: some View {
        Section("QR Code") {
            if let img = qrImage(from: profile.npub) {
                HStack {
                    Spacer()
                    Image(uiImage: img)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                        .frame(width: 220, height: 220)
                        .background(.white)
                        .clipShape(.rect(cornerRadius: 12))
                    Spacer()
                }
            } else {
                Text("Could not generate QR code.")
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private var followSection: some View {
        Section {
            if profile.isFollowed {
                Button("Unfollow", role: .destructive) {
                    onUnfollow()
                }
            } else {
                Button("Follow") {
                    onFollow()
                }
            }
        }
    }

    private func qrImage(from text: String) -> UIImage? {
        let data = Data(text.utf8)
        let filter = CIFilter.qrCodeGenerator()
        filter.setValue(data, forKey: "inputMessage")
        guard var output = filter.outputImage else { return nil }
        output = output.transformed(by: CGAffineTransform(scaleX: 10, y: 10))
        let ctx = CIContext()
        guard let cg = ctx.createCGImage(output, from: output.extent) else { return nil }
        return UIImage(cgImage: cg)
    }
}
