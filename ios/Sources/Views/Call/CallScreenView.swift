import AVFAudio
import Combine
import SwiftUI
import UIKit

@MainActor
struct CallScreenView: View {
    let call: CallState
    let peerName: String
    let onAcceptCall: @MainActor () -> Void
    let onRejectCall: @MainActor () -> Void
    let onEndCall: @MainActor () -> Void
    let onToggleMute: @MainActor () -> Void
    let onStartAgain: @MainActor () -> Void
    let onDismiss: @MainActor () -> Void

    @State private var showMicDeniedAlert = false
    @State private var isSpeakerOn = false
    @State private var isProximityMonitoringEnabled = false
    @State private var isProximityLocked = false

    var body: some View {
        ZStack {
            LinearGradient(
                colors: [Color.black.opacity(0.95), Color.blue.opacity(0.6)],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            .ignoresSafeArea()

            VStack(spacing: 24) {
                header

                Spacer(minLength: 12)

                VStack(spacing: 10) {
                    ZStack {
                        Circle()
                            .fill(Color.white.opacity(0.18))
                            .frame(width: 112, height: 112)

                        Text(String(peerName.prefix(1)).uppercased())
                            .font(.system(size: 42, weight: .bold, design: .rounded))
                            .foregroundStyle(.white)
                    }

                    Text(peerName)
                        .font(.system(.title2, design: .rounded).weight(.semibold))
                        .foregroundStyle(.white)
                        .lineLimit(1)

                    Text(call.status.titleText)
                        .font(.headline)
                        .foregroundStyle(.white.opacity(0.86))
                }

                if let duration = callDurationText(startedAt: call.startedAt), call.isLive {
                    Text(duration)
                        .font(.title3.monospacedDigit().weight(.medium))
                        .foregroundStyle(.white.opacity(0.9))
                }

                if let debug = call.debug {
                    Text(formattedCallDebugStats(debug))
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.white.opacity(0.78))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .background(Color.white.opacity(0.12), in: Capsule())
                }

                Spacer()

                controlRow
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 20)
            .allowsHitTesting(!isProximityLocked)

            if isProximityLocked {
                Color.black
                    .ignoresSafeArea()
                    .allowsHitTesting(true)
            }
        }
        .onAppear {
            updateProximityMonitoring()
        }
        .onChange(of: call.shouldEnableProximityLock) { _, _ in
            updateProximityMonitoring()
        }
        .onReceive(NotificationCenter.default.publisher(for: UIDevice.proximityStateDidChangeNotification)) { _ in
            guard isProximityMonitoringEnabled else { return }
            isProximityLocked = UIDevice.current.proximityState
        }
        .onDisappear {
            stopProximityMonitoring()
        }
        .onAppear { syncSpeakerState() }
        .onReceive(NotificationCenter.default.publisher(for: AVAudioSession.routeChangeNotification)) { _ in
            syncSpeakerState()
        }
        .alert("Microphone Permission Needed", isPresented: $showMicDeniedAlert) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("Microphone permission is required for calls.")
        }
    }

    private var header: some View {
        HStack {
            Button {
                onDismiss()
            } label: {
                Image(systemName: "chevron.down")
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.white)
                    .frame(width: 36, height: 36)
                    .background(Color.white.opacity(0.2), in: Circle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier(TestIds.callScreenDismiss)

            Spacer()
        }
    }

    @ViewBuilder
    private var controlRow: some View {
        switch call.status {
        case .ringing:
            HStack(spacing: 48) {
                CallControlButton(
                    title: "Decline",
                    systemImage: "phone.down.fill",
                    tint: .red
                ) {
                    onRejectCall()
                }
                .accessibilityIdentifier(TestIds.chatCallReject)

                CallControlButton(
                    title: "Accept",
                    systemImage: "phone.fill",
                    tint: .green
                ) {
                    startMicPermissionAction {
                        onAcceptCall()
                    }
                }
                .accessibilityIdentifier(TestIds.chatCallAccept)
            }
        case .offering, .connecting, .active:
            HStack(spacing: 36) {
                CallControlButton(
                    title: call.isMuted ? "Unmute" : "Mute",
                    systemImage: call.isMuted ? "mic.slash.fill" : "mic.fill",
                    tint: call.isMuted ? .orange : .white.opacity(0.25)
                ) {
                    onToggleMute()
                }
                .accessibilityIdentifier(TestIds.chatCallMute)

                CallControlButton(
                    title: isSpeakerOn ? "Speaker" : "Speaker",
                    systemImage: isSpeakerOn ? "speaker.wave.2.fill" : "speaker.fill",
                    tint: isSpeakerOn ? .orange : .white.opacity(0.25)
                ) {
                    toggleSpeaker()
                }
                .accessibilityIdentifier(TestIds.chatCallSpeaker)

                CallControlButton(
                    title: "End",
                    systemImage: "phone.down.fill",
                    tint: .red
                ) {
                    onEndCall()
                }
                .accessibilityIdentifier(TestIds.chatCallEnd)
            }
        case let .ended(reason):
            VStack(spacing: 12) {
                Text(callReasonText(reason))
                    .font(.subheadline)
                    .foregroundStyle(.white.opacity(0.86))

                HStack(spacing: 24) {
                    Button("Done") {
                        onDismiss()
                    }
                    .buttonStyle(.bordered)
                    .tint(.white)

                    Button("Start Again") {
                        startMicPermissionAction {
                            onStartAgain()
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(.green)
                    .accessibilityIdentifier(TestIds.chatCallStart)
                }
            }
        }
    }

    private func syncSpeakerState() {
        let route = AVAudioSession.sharedInstance().currentRoute
        isSpeakerOn = route.outputs.contains { $0.portType == .builtInSpeaker }
    }

    private func toggleSpeaker() {
        let session = AVAudioSession.sharedInstance()
        do {
            try session.overrideOutputAudioPort(isSpeakerOn ? .none : .speaker)
            syncSpeakerState()
        } catch {
            NSLog("Failed to toggle speaker: \(error)")
        }
    }
    private func updateProximityMonitoring() {
        if call.shouldEnableProximityLock {
            startProximityMonitoringIfNeeded()
        } else {
            stopProximityMonitoring()
        }
    }

    private func startProximityMonitoringIfNeeded() {
        guard !isProximityMonitoringEnabled else { return }
        UIDevice.current.isProximityMonitoringEnabled = true
        isProximityMonitoringEnabled = UIDevice.current.isProximityMonitoringEnabled
        isProximityLocked = isProximityMonitoringEnabled && UIDevice.current.proximityState
    }

    private func stopProximityMonitoring() {
        if isProximityMonitoringEnabled || UIDevice.current.isProximityMonitoringEnabled {
            UIDevice.current.isProximityMonitoringEnabled = false
        }
        isProximityMonitoringEnabled = false
        isProximityLocked = false
    }
    private func startMicPermissionAction(_ action: @escaping @MainActor () -> Void) {
        Task { @MainActor in
            let granted = await CallMicrophonePermission.ensureGranted()
            if granted {
                action()
            } else {
                showMicDeniedAlert = true
            }
        }
    }
}

private struct CallControlButton: View {
    let title: String
    let systemImage: String
    let tint: Color
    let action: @MainActor () -> Void

    var body: some View {
        Button {
            action()
        } label: {
            VStack(spacing: 10) {
                Image(systemName: systemImage)
                    .font(.title3.weight(.bold))
                    .foregroundStyle(.white)
                    .frame(width: 66, height: 66)
                    .background(tint, in: Circle())

                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(.white)
            }
        }
        .buttonStyle(.plain)
    }
}

#if DEBUG
#Preview("Call Screen") {
    CallScreenView(
        call: CallState(
            callId: "preview-call",
            chatId: "chat-1",
            peerNpub: "npub1...",
            status: .active,
            isLive: true,
            shouldAutoPresentCallScreen: true,
            shouldEnableProximityLock: true,
            startedAt: Int64(Date().timeIntervalSince1970) - 95,
            isMuted: false,
            debug: CallDebugStats(
                txFrames: 1023,
                rxFrames: 1001,
                rxDropped: 4,
                jitterBufferMs: 25,
                lastRttMs: 32
            )
        ),
        peerName: "Waffle",
        onAcceptCall: {},
        onRejectCall: {},
        onEndCall: {},
        onToggleMute: {},
        onStartAgain: {},
        onDismiss: {}
    )
}
#endif
