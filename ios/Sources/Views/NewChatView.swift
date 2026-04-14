import SwiftUI
import UIKit

struct NewChatView: View {
    let state: NewChatViewState
    let onCreateChat: @MainActor (String) -> Void
    let onRefreshFollowList: @MainActor () -> Void
    @State private var searchText = ""
    @State private var npubInput = ""
    @State private var showScanner = false
    @State private var showInvalidNpubAlert = false
    @State private var invalidNpubMessage = ""
    @State private var showManualEntrySheet = false

    private var filteredFollowList: [FollowListEntry] {
        let base = state.followList.filter { $0.npub != state.myNpub }
        guard !searchText.isEmpty else { return base }
        let query = searchText.lowercased()
        return base.filter { entry in
            if let name = entry.name, name.lowercased().contains(query) { return true }
            if let username = entry.username, username.lowercased().contains(query) { return true }
            if entry.npub.lowercased().contains(query) { return true }
            if entry.pubkey.lowercased().contains(query) { return true }
            return false
        }
    }

    var body: some View {
        let isLoading = state.isCreatingChat

        List {
            quickActionsSection(isLoading: isLoading)
            followsSection(isLoading: isLoading)
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color(.systemGroupedBackground))
        .navigationTitle("New Chat")
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    guard !isLoading else { return }
                    showManualEntrySheet = true
                } label: {
                    Image(systemName: "keyboard")
                }
                .disabled(isLoading)
                .accessibilityIdentifier(TestIds.newChatManualEntry)
            }
        }
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 0) {
                NativeBottomSearchField(title: "Search follows", text: $searchText)
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
            .padding(.bottom, 8)
            .background(.bar)
        }
        .overlay {
            if isLoading {
                Color.black.opacity(0.15)
                    .ignoresSafeArea()
                    .overlay {
                        ProgressView("Creating chat...")
                            .padding()
                            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
                    }
            }
        }
        .onAppear {
            onRefreshFollowList()
        }
        .sheet(isPresented: $showScanner) {
            QrScannerSheet { scanned in
                handleIncomingPeer(scanned)
            }
        }
        .sheet(isPresented: $showManualEntrySheet, onDismiss: {
            npubInput = ""
        }) {
            manualEntrySheet(isLoading: isLoading)
        }
        .alert("Invalid code", isPresented: $showInvalidNpubAlert) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(invalidNpubMessage)
        }
    }

    private func quickActionsSection(isLoading: Bool) -> some View {
        Section {
            HStack(spacing: 8) {
                NativeQuickActionButton(
                    title: "Paste Code",
                    systemImage: "doc.on.clipboard",
                    isPrimary: true,
                    accessibilityIdentifier: TestIds.newChatPaste
                ) {
                    handlePaste()
                }
                .disabled(isLoading)

                if ProcessInfo.processInfo.isiOSAppOnMac == false {
                    NativeQuickActionButton(
                        title: "Scan Code",
                        systemImage: "qrcode.viewfinder",
                        accessibilityIdentifier: TestIds.newChatScanQr
                    ) {
                        showScanner = true
                    }
                    .disabled(isLoading)
                }
            }
            .padding(.vertical, 8)
        }
    }

    @ViewBuilder
    private func followsSection(isLoading: Bool) -> some View {
        Section {
            if state.isFetchingFollowList && state.followList.isEmpty {
                HStack {
                    Spacer()
                    ProgressView("Loading follows...")
                    Spacer()
                }
            } else if state.followList.isEmpty {
                emptyStateRow(
                    title: "No follows found",
                    message: "Follow people to start chats here."
                )
            } else if filteredFollowList.isEmpty {
                emptyStateRow(
                    title: "No matches found",
                    message: "Try a different search."
                )
            } else {
                ForEach(filteredFollowList, id: \.pubkey) { entry in
                    Button {
                        onCreateChat(entry.npub)
                    } label: {
                        followListRow(entry: entry)
                            .padding(.vertical, 8)
                    }
                    .buttonStyle(.plain)
                    .disabled(isLoading)
                }
            }
        } header: {
            HStack(spacing: 6) {
                Text("Follows")
                if state.isFetchingFollowList {
                    ProgressView()
                        .controlSize(.small)
                }
            }
        }
    }

    private func followListRow(entry: FollowListEntry) -> some View {
        HStack(spacing: 12) {
            AvatarView(
                name: entry.name,
                npub: entry.npub,
                pictureUrl: entry.pictureUrl,
                size: 40
            )

            VStack(alignment: .leading, spacing: 2) {
                if let name = entry.name {
                    Text(name)
                        .font(.body)
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                }
                Text(truncatedNpub(entry.npub))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer()
        }
        .contentShape(Rectangle())
    }

    private func handlePaste() {
        let raw = UIPasteboard.general.string ?? ""
        handleIncomingPeer(raw)
    }

    private func emptyStateRow(title: String, message: String) -> some View {
        VStack(spacing: 6) {
            Text(title)
                .font(.subheadline.weight(.semibold))
            Text(message)
                .font(.footnote)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 20)
    }

    private func handleIncomingPeer(_ input: String) {
        let peer = input.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isValidPeerKey(input: peer) else {
            invalidNpubMessage = "Enter, paste, or scan a valid profile code."
            showInvalidNpubAlert = true
            return
        }
        onCreateChat(peer)
    }

    private func manualEntrySheet(isLoading: Bool) -> some View {
        NavigationStack {
            Form {
                Section {
                    Text("Enter a profile code.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)

                    TextField("Code", text: $npubInput)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier(TestIds.newChatPeerNpub)

                    Button("Start Chat") {
                        let peer = npubInput.trimmingCharacters(in: .whitespacesAndNewlines)
                        handleIncomingPeer(peer)
                        if isValidPeerKey(input: peer) {
                            npubInput = ""
                            showManualEntrySheet = false
                        }
                    }
                    .disabled(npubInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || isLoading)
                    .accessibilityIdentifier(TestIds.newChatStart)
                }
            }
            .navigationTitle("Enter Code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        npubInput = ""
                        showManualEntrySheet = false
                    }
                }
            }
        }
    }

    private func truncatedNpub(_ npub: String) -> String {
        if npub.count <= 20 { return npub }
        return String(npub.prefix(12)) + "..." + String(npub.suffix(4))
    }
}

#if DEBUG
#Preview("New Chat - Loading") {
    NavigationStack {
        NewChatView(
            state: NewChatViewState(
                isCreatingChat: false,
                isFetchingFollowList: true,
                followList: [],
                myNpub: nil
            ),
            onCreateChat: { _ in },
            onRefreshFollowList: {}
        )
    }
}

#Preview("New Chat - Populated") {
    NavigationStack {
        NewChatView(
            state: NewChatViewState(
                isCreatingChat: false,
                isFetchingFollowList: false,
                followList: PreviewAppState.sampleFollowList,
                myNpub: nil
            ),
            onCreateChat: { _ in },
            onRefreshFollowList: {}
        )
    }
}

#Preview("New Chat - Creating") {
    NavigationStack {
        NewChatView(
            state: NewChatViewState(
                isCreatingChat: true,
                isFetchingFollowList: false,
                followList: PreviewAppState.sampleFollowList,
                myNpub: nil
            ),
            onCreateChat: { _ in },
            onRefreshFollowList: {}
        )
    }
}
#endif
