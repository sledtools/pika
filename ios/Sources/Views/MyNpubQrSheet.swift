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
    let isDeveloperModeEnabledProvider: @MainActor () -> Bool
    let onEnableDeveloperMode: @MainActor () -> Void
    let isShowAgentMarketplaceEnabledProvider: @MainActor () -> Bool
    let onSetShowAgentMarketplace: @MainActor (Bool) -> Void
    let onWipeProfileCache: @MainActor () -> Void
    let onWipeMediaCache: @MainActor () -> Void
    let onWipeLocalData: @MainActor () -> Void
    private let cachedNpubQr: UIImage?

    @Environment(\.dismiss) private var dismiss
    @State private var showNsec = false
    @State private var showLogoutConfirm: Bool
    @State private var showWipeLocalDataConfirm = false
    @State private var selectedPhoto: PhotosPickerItem?
    @State private var isLoadingPhoto = false
    @State private var appVersionTapCount = 0
    @State private var developerModeEnabled = false
    @State private var showAgentMarketplace = false
    @State private var nameDraft = ""
    @State private var aboutDraft = ""
    @State private var didSyncDrafts = false
    @State private var didCopyNpub = false
    @State private var didCopyNsec = false
    @State private var didCopyAppVersion = false
    @State private var copyToastMessage: String?
    @State private var npubCopyResetTask: Task<Void, Never>?
    @State private var nsecCopyResetTask: Task<Void, Never>?
    @State private var appVersionCopyResetTask: Task<Void, Never>?
    @State private var copyToastResetTask: Task<Void, Never>?

    init(
        npub: String,
        profile: MyProfileState,
        nsecProvider: @MainActor @escaping () -> String?,
        onRefreshProfile: @MainActor @escaping () -> Void,
        onSaveProfile: @MainActor @escaping (_ name: String, _ about: String) -> Void,
        onUploadPhoto: @MainActor @escaping (_ data: Data, _ mimeType: String) -> Void,
        onLogout: @MainActor @escaping () -> Void,
        isDeveloperModeEnabledProvider: @MainActor @escaping () -> Bool,
        onEnableDeveloperMode: @MainActor @escaping () -> Void,
        isShowAgentMarketplaceEnabledProvider: @MainActor @escaping () -> Bool,
        onSetShowAgentMarketplace: @MainActor @escaping (Bool) -> Void,
        onWipeProfileCache: @MainActor @escaping () -> Void,
        onWipeMediaCache: @MainActor @escaping () -> Void,
        onWipeLocalData: @MainActor @escaping () -> Void,
        showLogoutConfirm: Bool = false
    ) {
        self.npub = npub
        self.profile = profile
        self.nsecProvider = nsecProvider
        self.onRefreshProfile = onRefreshProfile
        self.onSaveProfile = onSaveProfile
        self.onUploadPhoto = onUploadPhoto
        self.onLogout = onLogout
        self.isDeveloperModeEnabledProvider = isDeveloperModeEnabledProvider
        self.onEnableDeveloperMode = onEnableDeveloperMode
        self.isShowAgentMarketplaceEnabledProvider = isShowAgentMarketplaceEnabledProvider
        self.onSetShowAgentMarketplace = onSetShowAgentMarketplace
        self.onWipeProfileCache = onWipeProfileCache
        self.onWipeMediaCache = onWipeMediaCache
        self.onWipeLocalData = onWipeLocalData
        self.cachedNpubQr = QRCodeImage.make(from: npub)
        self._showLogoutConfirm = State(initialValue: showLogoutConfirm)
    }

    private var hasProfileChanges: Bool {
        normalized(nameDraft) != normalized(profile.name)
            || normalized(aboutDraft) != normalized(profile.about)
    }

    private var appVersionDisplay: String {
        let info = Bundle.main.infoDictionary
        let version = info?["CFBundleShortVersionString"] as? String ?? "unknown"
        let build = info?["CFBundleVersion"] as? String ?? "unknown"
        return "v\(version) (\(build))"
    }

    @ViewBuilder
    private var profileEditor: some View {
        Section("Profile") {
            profileNameField
            profileBioEditor
            if hasProfileChanges {
                Button {
                    onSaveProfile(nameDraft, aboutDraft)
                } label: {
                    Text("Save Changes")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
            }
        }
    }

    @ViewBuilder
    private var shareProfileSection: some View {
        Section {
            if let img = cachedNpubQr {
                Image(uiImage: img)
                    .interpolation(.none)
                    .resizable()
                    .scaledToFit()
                    .frame(width: 220, height: 220)
                    .background(.white)
                    .clipShape(.rect(cornerRadius: 12))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 8)
                    .accessibilityIdentifier(TestIds.chatListMyNpubQr)
            } else {
                Text("Could not generate QR code.")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 24)
            }

            publicKeyRow(
                npub,
                copied: didCopyNpub,
                valueTestId: TestIds.chatListMyNpubValue,
                testId: TestIds.chatListMyNpubCopy,
                accessibilityLabel: didCopyNpub ? "Copied code" : "Copy code"
            ) {
                copyToClipboard(npub, kind: .npub)
            }
        } header: {
            Text("Profile Code")
        } footer: {
            Text("Share your profile code so people can start a conversation with you.")
        }
    }

    @ViewBuilder
    private func accountKeySection(_ nsec: String) -> some View {
        Section {
            HStack(spacing: 12) {
                Group {
                    if showNsec {
                        Text(nsec)
                            .font(.system(.footnote, design: .monospaced))
                            .lineLimit(1)
                            .truncationMode(.middle)
                    } else {
                        Text(verbatim: String(repeating: "•", count: 24))
                            .font(.system(.footnote, design: .monospaced))
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                HStack(spacing: 8) {
                    Button {
                        showNsec.toggle()
                    } label: {
                        Image(systemName: showNsec ? "eye.slash" : "eye")
                            .font(.body.weight(.semibold))
                            .foregroundStyle(.tint)
                            .frame(width: 32, height: 32)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier(TestIds.myNpubNsecToggle)
                    .accessibilityLabel(showNsec ? "Hide account private key" : "Show account private key")
                    .accessibilityHint("Reveals or hides your account private key.")

                    copyIconButton(
                        copied: didCopyNsec,
                        testId: TestIds.myNpubNsecCopy,
                        accessibilityLabel: didCopyNsec ? "Copied private key" : "Copy private key"
                    ) {
                        copyToClipboard(nsec, kind: .nsec)
                    }
                }
            }
            .accessibilityIdentifier(TestIds.myNpubNsecValue)
        } header: {
            Text("Account Private Key")
        } footer: {
            Text("Keep this private. Anyone with your account private key can message as you.")
        }
    }

    @ViewBuilder
    private var settingsSection: some View {
        Section("Settings") {
            NavigationLink("Notifications") {
                NotificationSettingsView()
            }
            appVersionRow
            Button("Log out", role: .destructive) {
                showLogoutConfirm = true
            }
            .accessibilityIdentifier(TestIds.chatListLogout)
        }
    }

    @ViewBuilder
    private var developerSection: some View {
        if developerModeEnabled {
            Section {
                Toggle("Show Agent Marketplace", isOn: $showAgentMarketplace)
                    .onChangeCompat(of: showAgentMarketplace) { enabled in
                        onSetShowAgentMarketplace(enabled)
                    }
                developerButton("Wipe Profile Cache") {
                    onWipeProfileCache()
                }
                developerButton("Wipe Media Cache") {
                    onWipeMediaCache()
                }
                developerButton("Wipe All Local Data", role: .destructive) {
                    showWipeLocalDataConfirm = true
                }
            } header: {
                Text("Developer Mode")
            } footer: {
                Text("Show Agent Marketplace reveals experimental agent choices when creating a new agent. Wipe Profile Cache clears cached profiles and pictures. Wipe Media Cache clears the media DB and downloaded files. Wipe All Local Data deletes everything and logs out.")
            }
        }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    profileHeader
                }
                .listRowBackground(Color.clear)

                profileEditor
                shareProfileSection
                if let nsec = nsecProvider() {
                    accountKeySection(nsec)
                }
                settingsSection
                developerSection
            }
            .scrollContentBackground(.hidden)
            .background(Color(.systemGroupedBackground))
            .navigationTitle("Profile")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        dismiss()
                    } label: {
                        Image(systemName: "xmark")
                            .font(.body.weight(.semibold))
                            .frame(width: 30, height: 30)
                            .background(Color(.tertiarySystemFill), in: Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier(TestIds.chatListMyNpubClose)
                    .accessibilityLabel("Close profile")
                    .accessibilityHint("Dismisses the profile sheet.")
                }
            }
            .task {
                developerModeEnabled = isDeveloperModeEnabledProvider()
                showAgentMarketplace = isShowAgentMarketplaceEnabledProvider()
                onRefreshProfile()
                syncDraftsIfNeeded(force: false)
            }
            .onChangeCompat(of: selectedPhoto) { item in
                handlePhotoSelection(item)
            }
            .onChangeCompat(of: profile) {
                syncDraftsIfNeeded(force: !hasProfileChanges)
            }
            .onDisappear {
                npubCopyResetTask?.cancel()
                nsecCopyResetTask?.cancel()
                appVersionCopyResetTask?.cancel()
                copyToastResetTask?.cancel()
            }
            .confirmationDialog("Log out?", isPresented: $showLogoutConfirm, titleVisibility: .visible) {
                Button("Log out", role: .destructive) {
                    onLogout()
                    dismiss()
                }
                .accessibilityIdentifier(TestIds.chatListLogoutConfirm)
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("You can log back in with your private key.")
            }
            .confirmationDialog("Wipe all local data?", isPresented: $showWipeLocalDataConfirm, titleVisibility: .visible) {
                Button("Wipe All Local Data", role: .destructive) {
                    onWipeLocalData()
                    dismiss()
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("This deletes local databases, caches, and local state. This cannot be undone.")
            }
        }
        .overlay(alignment: .bottom) {
            copyToastOverlay
        }
    }

    private var profileHeader: some View {
        VStack(spacing: 10) {
            ZStack(alignment: .bottomTrailing) {
                AvatarView(
                    name: profile.name.isEmpty ? nil : profile.name,
                    npub: npub,
                    pictureUrl: profile.pictureUrl,
                    size: 112
                )

                PhotosPicker(selection: $selectedPhoto, matching: .images) {
                    Image(systemName: "pencil")
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(.tint)
                        .frame(width: 30, height: 30)
                        .background(Color(.systemBackground), in: Circle())
                        .overlay {
                            Circle()
                                .stroke(Color(.separator), lineWidth: 0.5)
                        }
                        .shadow(color: .black.opacity(0.08), radius: 8, y: 2)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Change profile photo")
                .accessibilityHint("Opens your photo library.")
                .offset(x: -4, y: -4)
            }
            .frame(maxWidth: .infinity)

            if isLoadingPhoto {
                ProgressView()
                    .controlSize(.small)
            }

            Text("Tap to change photo")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
    }

    private var profileNameField: some View {
        TextField("Your display name", text: $nameDraft)
            .font(.body)
            .textInputAutocapitalization(.words)
            .autocorrectionDisabled(false)
    }

    private var profileBioEditor: some View {
        ZStack(alignment: .topLeading) {
            TextEditor(text: $aboutDraft)
                .frame(minHeight: 120)
                .scrollContentBackground(.hidden)
                .padding(.horizontal, -5)
                .padding(.vertical, -8)

            if aboutDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                Text("Write something about yourself")
                    .foregroundStyle(.tertiary)
                    .padding(.top, 8)
                    .allowsHitTesting(false)
            }
        }
    }

    private var appVersionRow: some View {
        HStack(spacing: 12) {
            Button {
                handleAppVersionTap()
            } label: {
                HStack {
                    Text("App version")
                        .font(.body)
                        .fontWeight(.regular)
                        .foregroundStyle(.primary)
                    Spacer()
                    Text(appVersionDisplay)
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundStyle(.secondary)
                }
                .font(.body)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier(TestIds.myProfileAppVersionValue)

            copyIconButton(
                copied: didCopyAppVersion,
                testId: TestIds.myProfileAppVersionCopy,
                accessibilityLabel: didCopyAppVersion ? "Copied app version" : "Copy app version"
            ) {
                copyToClipboard(appVersionDisplay, kind: .appVersion)
            }
        }
    }

    private func developerButton(
        _ title: String,
        role: ButtonRole? = nil,
        action: @escaping () -> Void
    ) -> some View {
        Button(role: role, action: action) {
            HStack {
                Text(title)
                    .foregroundStyle(role == .destructive ? .red : .primary)
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func copyIconButton(
        copied: Bool,
        testId: String,
        accessibilityLabel: String,
        onCopy: @escaping () -> Void
    ) -> some View {
        Button(action: onCopy) {
            Image(systemName: copied ? "checkmark.circle.fill" : "doc.on.doc")
                .font(.body.weight(.semibold))
                .foregroundStyle(copied ? Color.green : Color.accentColor)
                .frame(width: 32, height: 32)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(testId)
        .accessibilityLabel(accessibilityLabel)
    }

    private func publicKeyRow(
        _ value: String,
        copied: Bool,
        valueTestId: String,
        testId: String,
        accessibilityLabel: String,
        onCopy: @escaping () -> Void
    ) -> some View {
        HStack(alignment: .center, spacing: 12) {
            Text(value)
                .font(.system(.footnote, design: .monospaced))
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityIdentifier(valueTestId)

            copyIconButton(
                copied: copied,
                testId: testId,
                accessibilityLabel: accessibilityLabel,
                onCopy: onCopy
            )
        }
    }

    private func syncDraftsIfNeeded(force: Bool) {
        if !didSyncDrafts || force {
            nameDraft = profile.name
            aboutDraft = profile.about
            didSyncDrafts = true
        }
    }

    @MainActor
    private func handleAppVersionTap() {
        if developerModeEnabled {
            showCopyToast("Developer mode already enabled")
            return
        }

        appVersionTapCount += 1
        let remaining = max(0, 7 - appVersionTapCount)
        if remaining == 0 {
            developerModeEnabled = true
            onEnableDeveloperMode()
            showCopyToast("Developer mode enabled")
            return
        }

        let noun = remaining == 1 ? "tap" : "taps"
        showCopyToast("\(remaining) \(noun) away from developer mode")
    }

    private enum CopyKind {
        case npub
        case nsec
        case appVersion
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
        case .appVersion:
            didCopyAppVersion = true
            appVersionCopyResetTask?.cancel()
            appVersionCopyResetTask = Task { @MainActor in
                try? await Task.sleep(nanoseconds: 1_200_000_000)
                didCopyAppVersion = false
            }
            showCopyToast("Copied app version")
        }
    }

    @MainActor
    private func showCopyToast(_ message: String) {
        withAnimation {
            copyToastMessage = message
        }
        copyToastResetTask?.cancel()
        copyToastResetTask = Task { @MainActor in
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            withAnimation {
                copyToastMessage = nil
            }
        }
    }

    @ViewBuilder
    private var copyToastOverlay: some View {
        if let message = copyToastMessage {
            Text(message)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.white)
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .background(.black.opacity(0.82), in: Capsule())
                .padding(.bottom, 20)
                .transition(.move(edge: .bottom).combined(with: .opacity))
                .accessibilityIdentifier("my_profile_copy_toast")
                .allowsHitTesting(false)
        }
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
        onLogout: {},
        isDeveloperModeEnabledProvider: { false },
        onEnableDeveloperMode: {},
        isShowAgentMarketplaceEnabledProvider: { false },
        onSetShowAgentMarketplace: { _ in },
        onWipeProfileCache: {},
        onWipeMediaCache: {},
        onWipeLocalData: {}
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
        isDeveloperModeEnabledProvider: { false },
        onEnableDeveloperMode: {},
        isShowAgentMarketplaceEnabledProvider: { false },
        onSetShowAgentMarketplace: { _ in },
        onWipeProfileCache: {},
        onWipeMediaCache: {},
        onWipeLocalData: {},
        showLogoutConfirm: true
    )
}
#endif
