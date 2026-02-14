import SwiftUI
import UIKit

struct NewGroupChatView: View {
    let state: NewGroupChatViewState
    let onCreateGroup: @MainActor (String, [String]) -> Void
    let onRefreshFollowList: @MainActor () -> Void
    @State private var groupName = ""
    @State private var selectedNpubs: [String] = []
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

    private var canCreate: Bool {
        !groupName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !selectedNpubs.isEmpty
            && !state.isCreatingChat
    }

    var body: some View {
        let isLoading = state.isCreatingChat

        List {
            // Group name
            Section {
                TextField("Group name", text: $groupName)
                    .disabled(isLoading)
                    .accessibilityIdentifier(TestIds.newGroupName)
            }

            // Selected members chips
            if !selectedNpubs.isEmpty {
                Section("Selected (\(selectedNpubs.count))") {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            ForEach(selectedNpubs, id: \.self) { npub in
                                selectedChip(npub: npub, isLoading: isLoading)
                            }
                        }
                        .padding(.vertical, 4)
                    }
                }
            }

            // Manual entry section
            Section {
                DisclosureGroup("Add member manually", isExpanded: $showManualEntry) {
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
                            toggleSelection(npub: entry.npub)
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

            // Create button
            Section {
                Button {
                    onCreateGroup(
                        groupName.trimmingCharacters(in: .whitespacesAndNewlines),
                        selectedNpubs
                    )
                } label: {
                    HStack {
                        Spacer()
                        if isLoading {
                            HStack(spacing: 8) {
                                ProgressView().tint(.white)
                                Text("Creating...")
                            }
                        } else {
                            Text("Create Group")
                        }
                        Spacer()
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!canCreate)
                .accessibilityIdentifier(TestIds.newGroupCreate)
            }
        }
        .listStyle(.insetGrouped)
        .searchable(text: $searchText, prompt: "Search follows")
        .navigationTitle("New Group")
        .onAppear {
            onRefreshFollowList()
        }
        .sheet(isPresented: $showScanner) {
            QrScannerSheet { scanned in
                let normalized = PeerKeyValidator.normalize(scanned)
                if PeerKeyValidator.isValidPeer(normalized) && !selectedNpubs.contains(normalized) {
                    selectedNpubs.append(normalized)
                } else {
                    npubInput = scanned
                    showManualEntry = true
                }
            }
        }
    }

    private func followListRow(entry: FollowListEntry) -> some View {
        let isSelected = selectedNpubs.contains(entry.npub)
        return HStack(spacing: 12) {
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

            if isSelected {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(.primary)
            }
        }
        .contentShape(Rectangle())
    }

    private func selectedChip(npub: String, isLoading: Bool) -> some View {
        let entry = state.followList.first { $0.npub == npub }
        let displayName = entry?.name ?? truncatedNpub(npub)
        return HStack(spacing: 4) {
            Text(displayName)
                .font(.caption)
                .foregroundStyle(.primary)
                .lineLimit(1)
            Button {
                selectedNpubs.removeAll { $0 == npub }
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .disabled(isLoading)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(Color(.secondarySystemFill), in: Capsule())
    }

    @ViewBuilder
    private func manualEntryContent(isLoading: Bool) -> some View {
        HStack(spacing: 8) {
            TextField("npub1… or hex pubkey", text: $npubInput)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .disabled(isLoading)
                .accessibilityIdentifier(TestIds.newGroupPeerNpub)

            Button {
                let raw = UIPasteboard.general.string ?? ""
                npubInput = PeerKeyValidator.normalize(raw)
            } label: {
                Image(systemName: "doc.on.clipboard")
            }
            .disabled(isLoading)

            Button {
                showScanner = true
            } label: {
                Image(systemName: "qrcode.viewfinder")
            }
            .disabled(isLoading)

            Button("Add") {
                addManualMember()
            }
            .fontWeight(.medium)
            .disabled(!PeerKeyValidator.isValidPeer(PeerKeyValidator.normalize(npubInput)) || isLoading)
            .accessibilityIdentifier(TestIds.newGroupAddMember)
        }
    }

    private func toggleSelection(npub: String) {
        if let idx = selectedNpubs.firstIndex(of: npub) {
            selectedNpubs.remove(at: idx)
        } else {
            selectedNpubs.append(npub)
        }
    }

    private func addManualMember() {
        let normalized = PeerKeyValidator.normalize(npubInput)
        guard PeerKeyValidator.isValidPeer(normalized) else { return }
        if !selectedNpubs.contains(normalized) {
            selectedNpubs.append(normalized)
        }
        npubInput = ""
    }

    private func truncatedNpub(_ npub: String) -> String {
        if npub.count <= 20 { return npub }
        return String(npub.prefix(12)) + "..." + String(npub.suffix(4))
    }
}

#if DEBUG
#Preview("New Group - Loading") {
    NavigationStack {
        NewGroupChatView(
            state: NewGroupChatViewState(
                isCreatingChat: false,
                isFetchingFollowList: true,
                followList: []
            ),
            onCreateGroup: { _, _ in },
            onRefreshFollowList: {}
        )
    }
}

#Preview("New Group - Populated") {
    NavigationStack {
        NewGroupChatView(
            state: NewGroupChatViewState(
                isCreatingChat: false,
                isFetchingFollowList: false,
                followList: PreviewAppState.sampleFollowList
            ),
            onCreateGroup: { _, _ in },
            onRefreshFollowList: {}
        )
    }
}

#Preview("New Group - Creating") {
    NavigationStack {
        NewGroupChatView(
            state: NewGroupChatViewState(
                isCreatingChat: true,
                isFetchingFollowList: false,
                followList: PreviewAppState.sampleFollowList
            ),
            onCreateGroup: { _, _ in },
            onRefreshFollowList: {}
        )
    }
}
#endif
