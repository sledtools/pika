import CoreImage
import CoreImage.CIFilterBuiltins
import PhotosUI
import SwiftUI
import UIKit

struct MyNpubQrSheet: View {
    let npub: String
    let profile: MyProfileState
    let nsecProvider: @MainActor () -> String?
    let onRefreshProfile: @MainActor () -> Void
    let onSaveProfile: @MainActor (_ name: String, _ about: String) -> Void
    let onUploadPhoto: @MainActor (_ data: Data, _ mimeType: String) -> Void
    let onLogout: @MainActor () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var showNsec = false
    @State private var showLogoutConfirm: Bool
    @State private var selectedPhoto: PhotosPickerItem?
    @State private var isLoadingPhoto = false
    @State private var nameDraft = ""
    @State private var aboutDraft = ""
    @State private var didSyncDrafts = false
    @State private var didCopyNpub = false
    @State private var didCopyNsec = false
    @State private var npubCopyResetTask: Task<Void, Never>?
    @State private var nsecCopyResetTask: Task<Void, Never>?

    init(
        npub: String,
        profile: MyProfileState,
        nsecProvider: @MainActor @escaping () -> String?,
        onRefreshProfile: @MainActor @escaping () -> Void,
        onSaveProfile: @MainActor @escaping (_ name: String, _ about: String) -> Void,
        onUploadPhoto: @MainActor @escaping (_ data: Data, _ mimeType: String) -> Void,
        onLogout: @MainActor @escaping () -> Void,
        showLogoutConfirm: Bool = false
    ) {
        self.npub = npub
        self.profile = profile
        self.nsecProvider = nsecProvider
        self.onRefreshProfile = onRefreshProfile
        self.onSaveProfile = onSaveProfile
        self.onUploadPhoto = onUploadPhoto
        self.onLogout = onLogout
        self._showLogoutConfirm = State(initialValue: showLogoutConfirm)
    }

    private var hasProfileChanges: Bool {
        normalized(nameDraft) != normalized(profile.name)
            || normalized(aboutDraft) != normalized(profile.about)
    }

    @ViewBuilder
    private var photoSection: some View {
        Section {
            VStack(spacing: 12) {
                AvatarView(
                    name: profile.name.isEmpty ? nil : profile.name,
                    npub: npub,
                    pictureUrl: profile.pictureUrl,
                    size: 96
                )

                if isLoadingPhoto {
                    ProgressView()
                }

                PhotosPicker(selection: $selectedPhoto, matching: .images) {
                    Label("Upload New Photo", systemImage: "photo")
                }
                .buttonStyle(.bordered)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 6)
        }
    }

    @ViewBuilder
    private var profileSection: some View {
        Section("Profile") {
            TextField("Name", text: $nameDraft)
                .textInputAutocapitalization(.words)
                .autocorrectionDisabled(false)

            TextField("About", text: $aboutDraft, axis: .vertical)
                .lineLimit(3...6)

            Button("Save Changes") {
                onSaveProfile(nameDraft, aboutDraft)
            }
            .disabled(!hasProfileChanges)
        }
    }

    @ViewBuilder
    private var npubSection: some View {
        Section {
            HStack(alignment: .center, spacing: 12) {
                Text(npub)
                    .font(.system(.footnote, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .accessibilityIdentifier(TestIds.chatListMyNpubValue)

                Spacer()

                copyAccessory(
                    copied: didCopyNpub,
                    testId: TestIds.chatListMyNpubCopy,
                    accessibilityLabel: didCopyNpub ? "Copied npub" : "Copy npub"
                ) {
                    copyToClipboard(npub, kind: .npub)
                }
            }
        } header: {
            Text("Public Key")
        } footer: {
            Text("Share your npub with people you trust.")
        }
    }

    @ViewBuilder
    private var qrSection: some View {
        Section("QR Code") {
            if let img = qrImage(from: npub) {
                HStack {
                    Spacer()
                    Image(uiImage: img)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                        .frame(width: 220, height: 220)
                        .background(.white)
                        .clipShape(.rect(cornerRadius: 12))
                        .accessibilityIdentifier(TestIds.chatListMyNpubQr)
                    Spacer()
                }
            } else {
                Text("Could not generate QR code.")
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func nsecSection(_ nsec: String) -> some View {
        Section {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                if showNsec {
                    Text(nsec)
                        .font(.system(.footnote, design: .monospaced))
                        .lineLimit(1)
                        .truncationMode(.middle)
                } else {
                    Text(String(repeating: "•", count: 24))
                        .font(.system(.footnote, design: .monospaced))
                }

                Spacer()

                Button {
                    showNsec.toggle()
                } label: {
                    Image(systemName: showNsec ? "eye.slash" : "eye")
                }
                .accessibilityIdentifier(TestIds.myNpubNsecToggle)
            }
            .accessibilityIdentifier(TestIds.myNpubNsecValue)

            HStack {
                Spacer()
                copyAccessory(
                    copied: didCopyNsec,
                    testId: TestIds.myNpubNsecCopy,
                    accessibilityLabel: didCopyNsec ? "Copied nsec" : "Copy nsec"
                ) {
                    copyToClipboard(nsec, kind: .nsec)
                }
            }
        } header: {
            Text("Private Key (nsec)")
        } footer: {
            Text("Keep this private. Anyone with your nsec can control your account.")
        }
    }

    @ViewBuilder
    private var logoutSection: some View {
        Section {
            Button("Log out", role: .destructive) {
                showLogoutConfirm = true
            }
            .accessibilityIdentifier(TestIds.chatListLogout)
        } footer: {
            Text("You can log back in with your nsec.")
        }
    }

    @ViewBuilder
    private var notificationsSection: some View {
        Section {
            NavigationLink("Notifications") {
                NotificationSettingsView()
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        photoSection
        profileSection
        npubSection
        qrSection
        if let nsec = nsecProvider() {
            nsecSection(nsec)
        }
        notificationsSection
        logoutSection
    }

    var body: some View {
        NavigationStack {
            List { content }
            .listStyle(.insetGrouped)
            .navigationTitle("Profile")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Close") {
                        dismiss()
                    }
                    .accessibilityIdentifier(TestIds.chatListMyNpubClose)
                }
            }
            .task {
                onRefreshProfile()
                syncDraftsIfNeeded(force: false)
            }
            .onChange(of: selectedPhoto) { _, item in
                handlePhotoSelection(item)
            }
            .onChange(of: profile) { _, _ in
                syncDraftsIfNeeded(force: !hasProfileChanges)
            }
            .onDisappear {
                npubCopyResetTask?.cancel()
                nsecCopyResetTask?.cancel()
            }
            .confirmationDialog("Log out?", isPresented: $showLogoutConfirm, titleVisibility: .visible) {
                Button("Log out", role: .destructive) {
                    onLogout()
                    dismiss()
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("You can log back in with your nsec.")
            }
        }
    }

    private func syncDraftsIfNeeded(force: Bool) {
        if !didSyncDrafts || force {
            nameDraft = profile.name
            aboutDraft = profile.about
            didSyncDrafts = true
        }
    }

    private enum CopyKind {
        case npub
        case nsec
    }

    @MainActor
    private func copyToClipboard(_ value: String, kind: CopyKind) {
        UIPasteboard.general.string = value

        switch kind {
        case .npub:
            didCopyNpub = true
            npubCopyResetTask?.cancel()
            npubCopyResetTask = Task { @MainActor in
                try? await Task.sleep(nanoseconds: 1_200_000_000)
                didCopyNpub = false
            }
        case .nsec:
            didCopyNsec = true
            nsecCopyResetTask?.cancel()
            nsecCopyResetTask = Task { @MainActor in
                try? await Task.sleep(nanoseconds: 1_200_000_000)
                didCopyNsec = false
            }
        }
    }

    @ViewBuilder
    private func copyAccessory(
        copied: Bool,
        testId: String,
        accessibilityLabel: String,
        onCopy: @escaping () -> Void
    ) -> some View {
        HStack(spacing: 8) {
            if copied {
                Text("Copied")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.green)
            }

            Button(action: onCopy) {
                Image(systemName: copied ? "checkmark.circle.fill" : "doc.on.doc")
                    .font(.body.weight(.semibold))
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .accessibilityIdentifier(testId)
            .accessibilityLabel(accessibilityLabel)
        }
        .animation(.easeInOut(duration: 0.15), value: copied)
    }

    private func handlePhotoSelection(_ item: PhotosPickerItem?) {
        guard let item else { return }
        isLoadingPhoto = true

        Task {
            defer {
                Task { @MainActor in
                    isLoadingPhoto = false
                    selectedPhoto = nil
                }
            }

            guard let data = try? await item.loadTransferable(type: Data.self), !data.isEmpty else {
                return
            }
            let mimeType = item.supportedContentTypes.first?.preferredMIMEType ?? "image/jpeg"
            await MainActor.run {
                onUploadPhoto(data, mimeType)
            }
        }
    }

    private func normalized(_ value: String) -> String {
        value.trimmingCharacters(in: .whitespacesAndNewlines)
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

#if DEBUG
#Preview("Profile") {
    MyNpubQrSheet(
        npub: PreviewAppState.sampleNpub,
        profile: PreviewAppState.chatListPopulated.myProfile,
        nsecProvider: { nil },
        onRefreshProfile: {},
        onSaveProfile: { _, _ in },
        onUploadPhoto: { _, _ in },
        onLogout: {}
    )
}

#Preview("Profile - Logout Confirm") {
    MyNpubQrSheet(
        npub: PreviewAppState.sampleNpub,
        profile: PreviewAppState.chatListPopulated.myProfile,
        nsecProvider: { "nsec1previewexample" },
        onRefreshProfile: {},
        onSaveProfile: { _, _ in },
        onUploadPhoto: { _, _ in },
        onLogout: {},
        showLogoutConfirm: true
    )
}
#endif
