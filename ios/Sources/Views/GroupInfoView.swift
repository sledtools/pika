import SwiftUI
import UIKit

struct GroupInfoView: View {
    let state: GroupInfoViewState
    let onAddMembers: @MainActor ([String]) -> Void
    let onRemoveMember: @MainActor (String) -> Void
    let onLeaveGroup: @MainActor () -> Void
    let onRenameGroup: @MainActor (String) -> Void
    @State private var npubInput = ""
    @State private var showScanner = false
    @State private var isEditing = false
    @State private var editedName = ""

    var body: some View {
        if let chat = state.chat {
            List {
                Section("Group Name") {
                    if isEditing {
                        HStack {
                            TextField("Group name", text: $editedName)
                                .textFieldStyle(.roundedBorder)
                            Button("Save") {
                                let trimmed = editedName.trimmingCharacters(in: .whitespacesAndNewlines)
                                if !trimmed.isEmpty {
                                    onRenameGroup(trimmed)
                                }
                                isEditing = false
                            }
                            .buttonStyle(.bordered)
                        }
                    } else {
                        HStack {
                            Text(chat.groupName ?? "Group")
                                .font(.headline)
                            Spacer()
                            if chat.isAdmin {
                                Button("Edit") {
                                    editedName = chat.groupName ?? ""
                                    isEditing = true
                                }
                                .font(.subheadline)
                            }
                        }
                    }
                }

                Section("Members (\(chat.members.count + 1))") {
                    HStack(spacing: 8) {
                        Image(systemName: "person.fill")
                            .foregroundStyle(.blue)
                        Text("You")
                            .font(.body.weight(.medium))
                        Spacer()
                        Text("Admin")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                    ForEach(chat.members, id: \.pubkey) { member in
                        HStack(spacing: 8) {
                            AvatarView(
                                name: member.name,
                                npub: member.npub,
                                pictureUrl: member.pictureUrl,
                                size: 28
                            )
                            VStack(alignment: .leading, spacing: 1) {
                                Text(member.name ?? truncated(member.npub))
                                    .font(.body)
                                    .lineLimit(1)
                                if member.name != nil {
                                    Text(truncated(member.npub))
                                        .font(.caption2)
                                        .foregroundStyle(.tertiary)
                                        .lineLimit(1)
                                }
                            }
                            Spacer()
                        }
                        .swipeActions(edge: .trailing) {
                            if chat.isAdmin {
                                Button(role: .destructive) {
                                    onRemoveMember(member.pubkey)
                                } label: {
                                    Label("Remove", systemImage: "person.badge.minus")
                                }
                            }
                        }
                    }
                }

                if chat.isAdmin {
                    Section("Add Member") {
                        HStack(spacing: 8) {
                            TextField("Peer npub", text: $npubInput)
                                .textInputAutocapitalization(.never)
                                .autocorrectionDisabled()
                                .textFieldStyle(.roundedBorder)
                                .accessibilityIdentifier(TestIds.groupInfoAddNpub)

                            Button("Add") {
                                let normalized = PeerKeyValidator.normalize(npubInput)
                                guard PeerKeyValidator.isValidPeer(normalized) else { return }
                                onAddMembers([normalized])
                                npubInput = ""
                            }
                            .buttonStyle(.bordered)
                            .disabled(!PeerKeyValidator.isValidPeer(PeerKeyValidator.normalize(npubInput)))
                            .accessibilityIdentifier(TestIds.groupInfoAddButton)
                        }
                    }
                }

                Section {
                    Button(role: .destructive) {
                        onLeaveGroup()
                    } label: {
                        HStack {
                            Image(systemName: "rectangle.portrait.and.arrow.right")
                            Text("Leave Group")
                        }
                    }
                    .accessibilityIdentifier(TestIds.groupInfoLeave)
                }
            }
            .navigationTitle("Group Info")
            .navigationBarTitleDisplayMode(.inline)
            .sheet(isPresented: $showScanner) {
                QrScannerSheet { scanned in
                    npubInput = scanned
                }
            }
        } else {
            ProgressView("Loading...")
        }
    }

    private func truncated(_ npub: String) -> String {
        if npub.count <= 20 { return npub }
        return String(npub.prefix(12)) + "..." + String(npub.suffix(4))
    }
}

#if DEBUG
#Preview("Group Info") {
    NavigationStack {
        GroupInfoView(
            state: GroupInfoViewState(chat: nil),
            onAddMembers: { _ in },
            onRemoveMember: { _ in },
            onLeaveGroup: {},
            onRenameGroup: { _ in }
        )
    }
}
#endif
