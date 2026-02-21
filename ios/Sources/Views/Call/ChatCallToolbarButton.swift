import SwiftUI

@MainActor
struct ChatCallToolbarButton: View {
    let callForChat: CallState?
    let hasLiveCallElsewhere: Bool
    let onStartCall: @MainActor () -> Void
    let onOpenCallScreen: @MainActor () -> Void

    @State private var showMicDeniedAlert = false

    private var hasLiveCallForChat: Bool {
        callForChat?.isLive ?? false
    }

    private var isDisabled: Bool {
        !hasLiveCallForChat && hasLiveCallElsewhere
    }

    private var symbolName: String {
        hasLiveCallForChat ? "phone.fill" : "phone"
    }

    var body: some View {
        Button {
            handleTap()
        } label: {
            Image(systemName: symbolName)
                .font(.body.weight(.semibold))
        }
        .disabled(isDisabled)
        .accessibilityIdentifier(hasLiveCallForChat ? TestIds.chatCallOpen : TestIds.chatCallStart)
        .alert("Microphone Permission Needed", isPresented: $showMicDeniedAlert) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("Microphone permission is required for calls.")
        }
    }

    private func handleTap() {
        if hasLiveCallForChat {
            onOpenCallScreen()
            return
        }

        Task { @MainActor in
            let granted = await CallMicrophonePermission.ensureGranted()
            if granted {
                onStartCall()
                onOpenCallScreen()
            } else {
                showMicDeniedAlert = true
            }
        }
    }
}
