import PhotosUI
import SwiftUI
import UIKit

struct GroupInfoView: View {
    let state: GroupInfoViewState
    let onAddMembers: @MainActor ([String]) -> Void
    let onRemoveMember: @MainActor (String) -> Void
    let onLeaveGroup: @MainActor () -> Void
    let onRenameGroup: @MainActor (String) -> Void
    let onTapMember: (@MainActor (String) -> Void)?
    let onSaveGroupProfile: @MainActor (String, String) -> Void
    let onUploadGroupProfilePhoto: @MainActor (Data, String) -> Void
    @State private var npubInput = ""
    @State private var showScanner = false
    @State private var isEditing = false
    @State private var editedName = ""
    @State private var copiedGroupId = false
    @State private var showGroupProfileSheet = false

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

                Section("Group ID") {
                    Button {
                        UIPasteboard.general.string = chat.chatId
                        copiedGroupId = true
                        Task {
                            try? await Task.sleep(for: .seconds(2))
                            copiedGroupId = false
                        }
                    } label: {
                        HStack {
                            Text(chat.chatId)
                                .font(.caption.monospaced())
                                .foregroundStyle(.primary)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer()
                            Text(copiedGroupId ? "Copied!" : "Tap to copy")
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .buttonStyle(.plain)
                }

                Section("Members (\(chat.members.count + 1))") {
                    Button {
                        showGroupProfileSheet = true
                    } label: {
                        HStack(spacing: 8) {
                            Image(systemName: "person.fill")
                                .foregroundStyle(.blue)
                            Text("You")
                                .font(.body.weight(.medium))
                            Spacer()
                            if chat.isAdmin {
                                Text("Admin")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    .buttonStyle(.plain)

                    ForEach(chat.members, id: \.pubkey) { member in
                        Button {
                            onTapMember?(member.pubkey)
                        } label: {
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
                                if member.isAdmin {
                                    Text("Admin")
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                            }
                        }
                        .buttonStyle(.plain)
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
                                let normalized = normalizePeerKey(input: npubInput)
                                guard isValidPeerKey(input: normalized) else { return }
                                onAddMembers([normalized])
                                npubInput = ""
                            }
                            .buttonStyle(.bordered)
                            .disabled(!isValidPeerKey(input: normalizePeerKey(input: npubInput)))
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
            .sheet(isPresented: $showGroupProfileSheet) {
                GroupProfileSheet(
                    profile: chat.myGroupProfile,
                    onSave: onSaveGroupProfile,
                    onUploadPhoto: onUploadGroupProfilePhoto
                )
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

struct GroupProfileSheet: View {
    let profile: MyProfileState?
    let onSave: @MainActor (String, String) -> Void
    let onUploadPhoto: @MainActor (Data, String) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var nameDraft = ""
    @State private var aboutDraft = ""
    @State private var selectedPhoto: PhotosPickerItem?
    @State private var isLoadingPhoto = false
    @State private var didSyncDrafts = false

    private var hasChanges: Bool {
        normalized(nameDraft) != normalized(profile?.name ?? "")
            || normalized(aboutDraft) != normalized(profile?.about ?? "")
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    VStack(spacing: 12) {
                        if let url = profile?.pictureUrl, !url.isEmpty {
                            AsyncImage(url: URL(string: url)) { image in
                                image
                                    .resizable()
                                    .scaledToFill()
                            } placeholder: {
                                Image(systemName: "person.crop.circle.fill")
                                    .resizable()
                                    .foregroundStyle(.secondary)
                            }
                            .frame(width: 96, height: 96)
                            .clipShape(Circle())
                        } else {
                            Image(systemName: "person.crop.circle.fill")
                                .resizable()
                                .foregroundStyle(.secondary)
                                .frame(width: 96, height: 96)
                        }

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

                Section("Profile") {
                    TextField("Name", text: $nameDraft)
                        .textInputAutocapitalization(.words)
                        .autocorrectionDisabled(false)

                    TextField("About", text: $aboutDraft, axis: .vertical)
                        .lineLimit(3...6)

                    Button("Save Changes") {
                        onSave(nameDraft, aboutDraft)
                    }
                    .disabled(!hasChanges)
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Group Profile")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Close") {
                        dismiss()
                    }
                }
            }
            .task {
                syncDraftsIfNeeded(force: false)
            }
            .onChange(of: selectedPhoto) { _, item in
                handlePhotoSelection(item)
            }
        }
    }

    private func syncDraftsIfNeeded(force: Bool) {
        if !didSyncDrafts || force {
            nameDraft = profile?.name ?? ""
            aboutDraft = profile?.about ?? ""
            didSyncDrafts = true
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
#Preview("Group Info") {
    NavigationStack {
        GroupInfoView(
            state: GroupInfoViewState(chat: nil),
            onAddMembers: { _ in },
            onRemoveMember: { _ in },
            onLeaveGroup: {},
            onRenameGroup: { _ in },
            onTapMember: nil,
            onSaveGroupProfile: { _, _ in },
            onUploadGroupProfilePhoto: { _, _ in }
        )
    }
}
#endif
