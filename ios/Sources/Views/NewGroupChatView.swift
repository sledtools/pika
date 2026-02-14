import SwiftUI
import UIKit

struct NewGroupChatView: View {
    let state: NewGroupChatViewState
    let onCreateGroup: @MainActor (String, [String]) -> Void
    @State private var groupName = ""
    @State private var npubInput = ""
    @State private var members: [String] = []
    @State private var showScanner = false

    var body: some View {
        let isLoading = state.isCreatingChat

        VStack(spacing: 16) {
            TextField("Group name", text: $groupName)
                .textFieldStyle(.roundedBorder)
                .disabled(isLoading)
                .accessibilityIdentifier(TestIds.newGroupName)

            VStack(alignment: .leading, spacing: 8) {
                Text("Members (\(members.count))")
                    .font(.subheadline.weight(.medium))

                HStack(spacing: 8) {
                    TextField("Peer npub", text: $npubInput)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .textFieldStyle(.roundedBorder)
                        .disabled(isLoading)
                        .accessibilityIdentifier(TestIds.newGroupPeerNpub)

                    Button("Add") {
                        addMember()
                    }
                    .buttonStyle(.bordered)
                    .disabled(!PeerKeyValidator.isValidPeer(PeerKeyValidator.normalize(npubInput)) || isLoading)
                    .accessibilityIdentifier(TestIds.newGroupAddMember)
                }

                HStack(spacing: 8) {
                    Button("Scan QR") { showScanner = true }
                        .buttonStyle(.bordered)
                        .disabled(isLoading)

                    Button("Paste") {
                        let raw = UIPasteboard.general.string ?? ""
                        npubInput = PeerKeyValidator.normalize(raw)
                    }
                    .buttonStyle(.bordered)
                    .disabled(isLoading)

                    Spacer()
                }

                if !members.isEmpty {
                    ForEach(members, id: \.self) { npub in
                        HStack {
                            Text(truncated(npub))
                                .font(.footnote)
                                .foregroundStyle(.secondary)
                            Spacer()
                            Button {
                                members.removeAll { $0 == npub }
                            } label: {
                                Image(systemName: "xmark.circle.fill")
                                    .foregroundStyle(.secondary)
                            }
                            .disabled(isLoading)
                        }
                        .padding(.horizontal, 4)
                    }
                }
            }

            Button {
                onCreateGroup(groupName.trimmingCharacters(in: .whitespacesAndNewlines), members)
            } label: {
                if isLoading {
                    HStack(spacing: 8) {
                        ProgressView().tint(.white)
                        Text("Creating...")
                    }
                } else {
                    Text("Create Group")
                }
            }
            .buttonStyle(.borderedProminent)
            .disabled(groupName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || members.isEmpty || isLoading)
            .accessibilityIdentifier(TestIds.newGroupCreate)

            Spacer()
        }
        .padding(16)
        .navigationTitle("New Group")
        .sheet(isPresented: $showScanner) {
            QrScannerSheet { scanned in
                npubInput = scanned
            }
        }
    }

    private func addMember() {
        let normalized = PeerKeyValidator.normalize(npubInput)
        guard PeerKeyValidator.isValidPeer(normalized) else { return }
        if !members.contains(normalized) {
            members.append(normalized)
        }
        npubInput = ""
    }

    private func truncated(_ npub: String) -> String {
        if npub.count <= 20 { return npub }
        return String(npub.prefix(12)) + "..." + String(npub.suffix(4))
    }
}

#if DEBUG
#Preview("New Group") {
    NavigationStack {
        NewGroupChatView(
            state: NewGroupChatViewState(isCreatingChat: false),
            onCreateGroup: { _, _ in }
        )
    }
}
#endif
