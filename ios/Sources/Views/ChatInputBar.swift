import SwiftUI
import PhotosUI
import UniformTypeIdentifiers

struct ChatInputBar: View {
    @Binding var messageText: String
    @Binding var selectedPhotoItem: PhotosPickerItem?
    @Binding var showFileImporter: Bool
    let showAttachButton: Bool
    let showMicButton: Bool
    @FocusState.Binding var isInputFocused: Bool
    let onSend: () -> Void
    let onStartVoiceRecording: () -> Void

    @State private var showPhotoPicker = false

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            if showAttachButton {
                Menu {
                    // TODO: Contact
                    // TODO: Stickers

                    Button {
                        showFileImporter = true
                    } label: {
                        Label("File", systemImage: "doc")
                    }

                    Button {
                        showPhotoPicker = true
                    } label: {
                        Label("Photos & Videos", systemImage: "photo.on.rectangle")
                    }
                } label: {
                    Image(systemName: "plus")
                        .font(.body.weight(.semibold))
                        .frame(width: 52, height: 52)
                }
                .tint(.secondary)
                .modifier(GlassCircleModifier())
                .photosPicker(
                    isPresented: $showPhotoPicker,
                    selection: $selectedPhotoItem,
                    matching: .any(of: [.images, .videos])
                )
            }

            HStack(spacing: 10) {
                TextEditor(text: $messageText)
                    .focused($isInputFocused)
                    .frame(minHeight: 36, maxHeight: 150)
                    .fixedSize(horizontal: false, vertical: true)
                    .scrollContentBackground(.hidden)
                    .onAppear {
                        if ProcessInfo.processInfo.isiOSAppOnMac {
                            isInputFocused = true
                        }
                    }
                    .onKeyPress(.return, phases: .down) { keyPress in
                        if keyPress.modifiers.contains(.shift) {
                            return .ignored
                        }
                        onSend()
                        return .handled
                    }
                    .overlay(alignment: .topLeading) {
                        if messageText.isEmpty {
                            Text("Message")
                                .foregroundStyle(.tertiary)
                                .padding(.leading, 5)
                                .padding(.top, 8)
                                .allowsHitTesting(false)
                        }
                    }
                    .accessibilityIdentifier(TestIds.chatMessageInput)

                let isEmpty = messageText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                if isEmpty, showMicButton {
                    Button {
                        onStartVoiceRecording()
                    } label: {
                        Image(systemName: "mic.fill")
                            .font(.title2)
                    }
                    .transition(.scale.combined(with: .opacity))
                } else {
                    Button(action: { onSend() }) {
                        Image(systemName: "arrow.up.circle.fill")
                            .font(.title2)
                    }
                    .disabled(isEmpty)
                    .accessibilityIdentifier(TestIds.chatSend)
                    .transition(.scale.combined(with: .opacity))
                }
            }
            .animation(.easeInOut(duration: 0.15), value: messageText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            .modifier(GlassInputModifier())
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 12)
    }
}

// MARK: - Glass modifiers (shared)

struct GlassInputModifier: ViewModifier {
    func body(content: Content) -> some View {
        #if compiler(>=6.2)
        if #available(iOS 26.0, *) {
            content
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .glassEffect(.regular.interactive(), in: RoundedRectangle(cornerRadius: 20))
        } else {
            content
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 20))
        }
        #else
        content
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 20))
        #endif
    }
}

struct GlassCircleModifier: ViewModifier {
    func body(content: Content) -> some View {
        #if compiler(>=6.2)
        if #available(iOS 26.0, *) {
            content
                .glassEffect(.regular.interactive(), in: Circle())
        } else {
            content
                .background(.ultraThinMaterial, in: Circle())
        }
        #else
        content
            .background(.ultraThinMaterial, in: Circle())
        #endif
    }
}

// MARK: - Previews

#if DEBUG
private struct ChatInputBarPreview: View {
    @State var messageText = ""
    @State var selectedPhotoItem: PhotosPickerItem?
    @State var showFileImporter = false
    @FocusState var isInputFocused: Bool

    let showAttach: Bool
    let showMic: Bool

    var body: some View {
        ChatInputBar(
            messageText: $messageText,
            selectedPhotoItem: $selectedPhotoItem,
            showFileImporter: $showFileImporter,
            showAttachButton: showAttach,
            showMicButton: showMic,
            isInputFocused: $isInputFocused,
            onSend: {},
            onStartVoiceRecording: {}
        )
    }
}

#Preview("Input Bar — Full") {
    VStack {
        Spacer()
        ChatInputBarPreview(showAttach: true, showMic: true)
    }
    .background(Color(uiColor: .systemBackground))
}

#Preview("Input Bar — No Attach") {
    VStack {
        Spacer()
        ChatInputBarPreview(showAttach: false, showMic: false)
    }
    .background(Color(uiColor: .systemBackground))
}

#Preview("Input Bar — With Text") {
    VStack {
        Spacer()
        ChatInputBarPreview(showAttach: true, showMic: true)
    }
    .background(Color(uiColor: .systemBackground))
    .onAppear {}
}
#endif
