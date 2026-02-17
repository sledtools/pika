import SwiftUI
import UserNotifications

struct NotificationSettingsView: View {
    @State private var permissionStatus: UNAuthorizationStatus?
    var body: some View {
        List {
            permissionSection
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

            if permissionStatus == .denied {
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
    }

    @ViewBuilder
    private var deviceInfoSection: some View {
        Section {
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
