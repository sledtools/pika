import Foundation
import UIKit
import UserNotifications
import os

/// Manages APNs registration. Business logic (subscription tracking, HTTP calls)
/// lives in Rust `AppCore`; this class only handles platform-specific APNs APIs.
@MainActor
final class PushNotificationManager: NSObject, ObservableObject {
    static let shared = PushNotificationManager()

    private let logger = Logger(subsystem: "com.justinmoon.pika", category: "push")

    /// The real APNs device token, set after successful registration.
    @Published private(set) var apnsToken: String?

    /// Callback invoked when an APNs token is received. Set by AppManager to
    /// forward the token to Rust via `AppAction.setPushToken`.
    var onTokenReceived: ((String) -> Void)?

    /// Request notification permission and register for remote notifications.
    func requestPermissionAndRegister() {
        let center = UNUserNotificationCenter.current()
        center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, error in
            if let error {
                self.logger.error("Notification permission error: \(error.localizedDescription)")
                return
            }
            self.logger.info("Notification permission granted: \(granted)")
            if granted {
                DispatchQueue.main.async {
                    UIApplication.shared.registerForRemoteNotifications()
                }
            }
        }
    }

    /// Called by AppDelegate when APNs returns a device token.
    func didRegisterForRemoteNotifications(deviceToken: Data) {
        let token = deviceToken.map { String(format: "%02x", $0) }.joined()
        logger.info("Got APNs device token: \(token)")
        apnsToken = token
        onTokenReceived?(token)
    }

    /// Called by AppDelegate when APNs registration fails.
    func didFailToRegisterForRemoteNotifications(error: Error) {
        logger.error("APNs registration failed: \(error.localizedDescription)")
    }
}
