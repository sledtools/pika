import SwiftUI
import UserNotifications

struct NotificationSettingsView: View {
    @AppStorage("pika_push_foreground") private var showInForeground = false
    @State private var permissionStatus: UNAuthorizationStatus?
    @State private var isRegistering = false
    @State private var registrationResult: String?
    @State private var isSubscribing = false
    @State private var subscribeResult: String?
    @State private var groupIdInput = ""

    var body: some View {
        List {
            permissionSection
            registrationSection
            subscribeSection
            deviceInfoSection
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Notifications")
        .task {
            await refreshPermissionStatus()
        }
    }

    @ViewBuilder
    private var permissionSection: some View {
        Section {
            HStack {
                Text("Permission")
                Spacer()
                Text(permissionLabel)
                    .foregroundStyle(permissionColor)
            }

            if permissionStatus == .notDetermined || permissionStatus == nil {
                Button("Request Permission") {
                    PushNotificationManager.shared.requestPermissionAndRegister()
                    Task {
                        try? await Task.sleep(for: .seconds(1))
                        await refreshPermissionStatus()
                    }
                }
            } else if permissionStatus == .denied {
                Button("Open Settings") {
                    if let url = URL(string: UIApplication.openSettingsURLString) {
                        UIApplication.shared.open(url)
                    }
                }
            }
        } header: {
            Text("Push Notifications")
        } footer: {
            if permissionStatus == .denied {
                Text("Notifications are disabled. Tap \"Open Settings\" to enable them.")
            }
        }

        Section {
            Toggle("Show While App is Open", isOn: $showInForeground)
        } footer: {
            Text("Display notification banners even when the app is in the foreground.")
        }
    }

    @ViewBuilder
    private var registrationSection: some View {
        Section {
            Button {
                isRegistering = true
                registrationResult = nil
                PushNotificationManager.shared.requestPermissionAndRegister()
                Task {
                    // Wait for APNs token callback + server registration
                    try? await Task.sleep(for: .seconds(3))
                    isRegistering = false
                    if PushNotificationManager.shared.apnsToken != nil {
                        registrationResult = "Registered with real APNs token"
                    } else {
                        registrationResult = "Registered (no APNs token yet)"
                    }
                    await refreshPermissionStatus()
                }
            } label: {
                HStack {
                    Text("Register for Notifications")
                    Spacer()
                    if isRegistering {
                        ProgressView()
                    }
                }
            }
            .disabled(isRegistering)

            if let result = registrationResult {
                Text(result)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        } header: {
            Text("Server Registration")
        } footer: {
            Text("Requests notification permission, gets an APNs token, and registers with the server.")
        }
    }

    @ViewBuilder
    private var subscribeSection: some View {
        Section {
            TextField("Group ID", text: $groupIdInput)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)

            Button {
                let groupId = groupIdInput.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !groupId.isEmpty else { return }
                isSubscribing = true
                subscribeResult = nil
                Task {
                    await PushNotificationManager.shared.subscribeToGroups([groupId])
                    isSubscribing = false
                    subscribeResult = "Subscribed to \(groupId)"
                }
            } label: {
                HStack {
                    Text("Subscribe to Group")
                    Spacer()
                    if isSubscribing {
                        ProgressView()
                    }
                }
            }
            .disabled(isSubscribing || groupIdInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)

            if let result = subscribeResult {
                Text(result)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        } header: {
            Text("Group Subscriptions")
        } footer: {
            Text("Subscribe to a group ID to receive notifications for kind 445 events with matching #h tags.")
        }
    }

    @ViewBuilder
    private var deviceInfoSection: some View {
        Section {
            HStack {
                Text("Device ID")
                Spacer()
                Text(PushNotificationManager.shared.deviceId)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            HStack {
                Text("APNs Token")
                Spacer()
                Text(PushNotificationManager.shared.apnsToken ?? "Not registered")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        } header: {
            Text("Debug Info")
        }
    }

    private var permissionLabel: String {
        switch permissionStatus {
        case .authorized: return "Enabled"
        case .denied: return "Disabled"
        case .provisional: return "Provisional"
        case .ephemeral: return "Ephemeral"
        case .notDetermined, .none: return "Not Requested"
        @unknown default: return "Unknown"
        }
    }

    private var permissionColor: Color {
        switch permissionStatus {
        case .authorized, .provisional, .ephemeral: return .green
        case .denied: return .red
        case .notDetermined, .none: return .secondary
        @unknown default: return .secondary
        }
    }

    private func refreshPermissionStatus() async {
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        permissionStatus = settings.authorizationStatus
    }
}

#if DEBUG
#Preview {
    NavigationStack {
        NotificationSettingsView()
    }
}
#endif
