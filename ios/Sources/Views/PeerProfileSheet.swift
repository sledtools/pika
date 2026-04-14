import SwiftUI
import UIKit

struct PeerProfileSheet: View {
    let profile: PeerProfileState
    let onMessage: @MainActor () -> Void
    let onStartCall: @MainActor () -> Void
    let onStartVideoCall: @MainActor () -> Void
    let onFollow: @MainActor () -> Void
    let onUnfollow: @MainActor () -> Void
    let onOpenMediaGallery: (@MainActor () -> Void)?
    let onClose: @MainActor () -> Void
    private let cachedProfileQr: UIImage?

    @Environment(\.dismiss) private var dismiss
    @State private var didCopyNpub = false
    @State private var copyResetTask: Task<Void, Never>?
    @State private var showCallPermissionDeniedAlert = false
    @State private var attemptedCallType: AttemptedCallType = .audio

    private enum AttemptedCallType {
        case audio
        case video
    }

    init(
        profile: PeerProfileState,
        onMessage: @MainActor @escaping () -> Void,
        onStartCall: @MainActor @escaping () -> Void,
        onStartVideoCall: @MainActor @escaping () -> Void,
        onFollow: @MainActor @escaping () -> Void,
        onUnfollow: @MainActor @escaping () -> Void,
        onOpenMediaGallery: (@MainActor () -> Void)?,
        onClose: @MainActor @escaping () -> Void
    ) {
        self.profile = profile
        self.onMessage = onMessage
        self.onStartCall = onStartCall
        self.onStartVideoCall = onStartVideoCall
        self.onFollow = onFollow
        self.onUnfollow = onUnfollow
        self.onOpenMediaGallery = onOpenMediaGallery
        self.onClose = onClose
        self.cachedProfileQr = QRCodeImage.make(
            from: profile.profileCode.isEmpty ? profile.npub : profile.profileCode
        )
    }

    private var profileCode: String {
        profile.profileCode.isEmpty ? profile.npub : profile.profileCode
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    profileHeader
                }
                .listRowBackground(Color.clear)

                actionSection
                shareSection
            }
            .scrollContentBackground(.hidden)
            .background(Color(.systemGroupedBackground))
            .navigationTitle("Profile")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        onClose()
                        dismiss()
                    } label: {
                        Image(systemName: "xmark")
                            .font(.body.weight(.semibold))
                            .frame(width: 30, height: 30)
                            .background(Color(.tertiarySystemFill), in: Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Close profile")
                }
            }
            .onDisappear {
                copyResetTask?.cancel()
            }
            .alert("Permission Needed", isPresented: $showCallPermissionDeniedAlert) {
                Button("OK", role: .cancel) {}
            } message: {
                Text(permissionDeniedMessage)
            }
        }
    }

    private var permissionDeniedMessage: String {
        switch attemptedCallType {
        case .audio:
            return "Microphone permission is required for voice calls."
        case .video:
            return "Microphone and camera permissions are required for video calls."
        }
    }

    private var profileHeader: some View {
        VStack(spacing: 10) {
            AvatarView(
                name: profile.name,
                npub: profile.npub,
                pictureUrl: profile.pictureUrl,
                size: 104
            )
            .frame(maxWidth: .infinity)

            if let name = profile.name {
                Text(name)
                    .font(.title2.weight(.bold))
                    .frame(maxWidth: .infinity)
            }

            if let about = profile.about, !about.isEmpty {
                Text(about)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
                    .padding(.horizontal, 24)
            }
        }
    }

    @ViewBuilder
    private var actionSection: some View {
        Section {
            HStack(spacing: 8) {
                NativeQuickActionButton(title: "Message", systemImage: "message") {
                    onMessage()
                }

                NativeQuickActionButton(title: "Voice", systemImage: "phone") {
                    attemptedCallType = .audio
                    CallPermissionActions.withMicPermission(
                        onDenied: { showCallPermissionDeniedAlert = true },
                        action: onStartCall
                    )
                }

                NativeQuickActionButton(title: "Video", systemImage: "video") {
                    attemptedCallType = .video
                    CallPermissionActions.withMicAndCameraPermission(
                        onDenied: { showCallPermissionDeniedAlert = true },
                        action: onStartVideoCall
                    )
                }

                NativeQuickActionButton(
                    title: profile.isFollowed ? "Unfollow" : "Follow",
                    systemImage: profile.isFollowed ? "person.badge.minus" : "person.badge.plus",
                    isPrimary: !profile.isFollowed
                ) {
                    if profile.isFollowed {
                        onUnfollow()
                    } else {
                        onFollow()
                    }
                }
            }
            .padding(.vertical, 8)
        }

        if let onOpenMediaGallery {
            Section {
                Button {
                    dismiss()
                    onOpenMediaGallery()
                } label: {
                    Label("Photos & Videos", systemImage: "photo.on.rectangle.angled")
                }
                .buttonStyle(.plain)
                .foregroundStyle(.primary)
            }
        }
    }

    @ViewBuilder
    private var shareSection: some View {
        Section {
            if let img = cachedProfileQr {
                Image(uiImage: img)
                    .interpolation(.none)
                    .resizable()
                    .scaledToFit()
                    .frame(width: 220, height: 220)
                    .background(.white)
                    .clipShape(.rect(cornerRadius: 12))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 8)
            } else {
                Text("Could not generate QR code.")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 24)
            }

            HStack(alignment: .center, spacing: 12) {
                Text(profileCode)
                    .font(.system(.footnote, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: .infinity, alignment: .leading)

                Button {
                    UIPasteboard.general.string = profileCode
                    didCopyNpub = true
                    copyResetTask?.cancel()
                    copyResetTask = Task { @MainActor in
                        try? await Task.sleep(nanoseconds: 1_200_000_000)
                        didCopyNpub = false
                    }
                } label: {
                    Image(systemName: didCopyNpub ? "checkmark.circle.fill" : "doc.on.doc")
                        .font(.body.weight(.semibold))
                        .foregroundStyle(didCopyNpub ? Color.green : Color.accentColor)
                        .frame(width: 32, height: 32)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(didCopyNpub ? "Copied code" : "Copy code")
            }
            .animation(.easeInOut(duration: 0.15), value: didCopyNpub)
        } header: {
            Text("Profile Code")
        } footer: {
            Text("Use this profile code to start a conversation.")
        }
    }
}
