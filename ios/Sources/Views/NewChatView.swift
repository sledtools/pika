import SwiftUI
import UIKit

struct NewChatView: View {
    let state: NewChatViewState
    let onCreateChat: @MainActor (String) -> Void
    let onRefreshFollowList: @MainActor () -> Void
    @State private var searchText = ""
    @State private var showManualEntry = false
    @State private var npubInput = ""
    @State private var showScanner = false

    private var filteredFollowList: [FollowListEntry] {
        guard !searchText.isEmpty else { return state.followList }
        let query = searchText.lowercased()
        return state.followList.filter { entry in
            if let name = entry.name, name.lowercased().contains(query) { return true }
            if entry.npub.lowercased().contains(query) { return true }
            if entry.pubkey.lowercased().contains(query) { return true }
            return false
        }
    }

    var body: some View {
        let isLoading = state.isCreatingChat

        List {
            // Manual entry section
            Section {
                DisclosureGroup("Enter npub manually", isExpanded: $showManualEntry) {
                    manualEntryContent(isLoading: isLoading)
                }
            }

            // Follow list section
            if state.isFetchingFollowList && state.followList.isEmpty {
                Section {
                    HStack {
                        Spacer()
                        ProgressView("Loading follows...")
                        Spacer()
                    }
                    .padding(.vertical, 8)
                }
            } else if state.followList.isEmpty {
                Section {
                    Text("No follows found.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                }
            } else {
                Section {
                    ForEach(filteredFollowList, id: \.pubkey) { entry in
                        Button {
                            onCreateChat(entry.npub)
                        } label: {
                            followListRow(entry: entry)
                        }
                        .buttonStyle(.plain)
                        .disabled(isLoading)
                    }
                } header: {
                    if state.isFetchingFollowList {
                        HStack(spacing: 6) {
                            Text("Follows")
                            ProgressView()
                                .controlSize(.small)
                        }
                    } else {
                        Text("Follows")
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .searchable(text: $searchText, prompt: "Search follows")
        .navigationTitle("New Chat")
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
                npubInput = scanned
                showManualEntry = true
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

    @ViewBuilder
    private func manualEntryContent(isLoading: Bool) -> some View {
        let peer = PeerKeyValidator.normalize(npubInput)
        let isValidPeer = PeerKeyValidator.isValidPeer(peer)

        HStack(spacing: 8) {
            TextField("npub1… or hex pubkey", text: $npubInput)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .disabled(isLoading)
                .accessibilityIdentifier(TestIds.newChatPeerNpub)

            Button {
                let raw = UIPasteboard.general.string ?? ""
                npubInput = PeerKeyValidator.normalize(raw)
            } label: {
                Image(systemName: "doc.on.clipboard")
            }
            .disabled(isLoading)
            .accessibilityIdentifier(TestIds.newChatPaste)

            Button {
                showScanner = true
            } label: {
                Image(systemName: "qrcode.viewfinder")
            }
            .disabled(isLoading)
            .accessibilityIdentifier(TestIds.newChatScanQr)
        }

        if !peer.isEmpty && !isValidPeer {
            Text("Enter a valid npub1… or 64-char hex pubkey.")
                .font(.footnote)
                .foregroundStyle(.red)
        }

        Button {
            onCreateChat(peer)
        } label: {
            Text("Start Chat")
                .frame(maxWidth: .infinity)
        }
        .buttonStyle(.borderedProminent)
        .accessibilityIdentifier(TestIds.newChatStart)
        .disabled(!isValidPeer || isLoading)
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
                followList: []
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
                followList: PreviewAppState.sampleFollowList
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
                followList: PreviewAppState.sampleFollowList
            ),
            onCreateChat: { _ in },
            onRefreshFollowList: {}
        )
    }
}
#endif
