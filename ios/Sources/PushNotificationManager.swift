import Foundation
import UIKit
import UserNotifications
import os

/// Manages APNs registration and communication with the notification server.
@MainActor
final class PushNotificationManager: NSObject, ObservableObject {
    static let shared = PushNotificationManager()

    private let logger = Logger(subsystem: "com.justinmoon.pika", category: "push")
    private let serverURL: URL
    private let deviceIdKey = "pika_push_device_id"
    private let subscribedChatsKey = "pika_push_subscribed_chats"

    /// Persistent device ID for this install.
    private(set) var deviceId: String

    /// Chat IDs we've already subscribed to on the server.
    private(set) var subscribedChatIds: Set<String> {
        get {
            Set(UserDefaults.standard.stringArray(forKey: subscribedChatsKey) ?? [])
        }
        set {
            UserDefaults.standard.set(Array(newValue), forKey: subscribedChatsKey)
        }
    }

    /// Last chat count we synced against, to avoid re-diffing on every state update.
    private var lastChatCount: Int = -1

    /// The real APNs device token, set after successful registration.
    @Published private(set) var apnsToken: String?

    /// Whether to show notification banners when the app is in the foreground.
    var showInForeground: Bool {
        get { UserDefaults.standard.bool(forKey: "pika_push_foreground") }
        set { UserDefaults.standard.set(newValue, forKey: "pika_push_foreground") }
    }

    override init() {
        let env = ProcessInfo.processInfo.environment
        let urlString = env["PIKA_NOTIFICATION_URL"] ?? "https://test.notifs.benthecarman.com"
        self.serverURL = URL(string: urlString)!

        // Load or create a stable device UUID
        if let existing = UserDefaults.standard.string(forKey: deviceIdKey) {
            self.deviceId = existing
        } else {
            let newId = UUID().uuidString
            UserDefaults.standard.set(newId, forKey: deviceIdKey)
            self.deviceId = newId
        }

        super.init()
    }

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

        Task {
            await registerDevice(token: token)
        }
    }

    /// Called by AppDelegate when APNs registration fails.
    func didFailToRegisterForRemoteNotifications(error: Error) {
        logger.error("APNs registration failed: \(error.localizedDescription)")
    }

    /// Register this device with the notification server.
    func registerDevice(token: String? = nil) async {
        let deviceToken = token ?? apnsToken ?? deviceId
        let url = serverURL.appendingPathComponent("register")
        let body: [String: String] = [
            "id": deviceId,
            "device_token": deviceToken,
            "platform": "ios"
        ]

        do {
            let result = try await postJSON(url: url, body: body)
            logger.info("Registered device: \(result)")
        } catch {
            logger.error("Failed to register device: \(error.localizedDescription)")
        }
    }

    /// Subscribe this device to a set of group IDs.
    func subscribeToGroups(_ groupIds: [String]) async {
        guard !groupIds.isEmpty else { return }

        let url = serverURL.appendingPathComponent("subscribe-groups")
        let body: [String: Any] = [
            "id": deviceId,
            "group_ids": groupIds
        ]

        do {
            let result = try await postJSON(url: url, body: body)
            logger.info("Subscribed to groups \(groupIds): \(result)")
        } catch {
            logger.error("Failed to subscribe to groups: \(error.localizedDescription)")
        }
    }

    // MARK: - Subscription Sync

    /// Diffs the current chat list against the persisted subscriptions and subscribes/unsubscribes as needed.
    /// Only does work when the chat count changes.
    func syncSubscriptions(chatIds: [String]) {
        guard chatIds.count != lastChatCount else { return }
        lastChatCount = chatIds.count

        let current = Set(chatIds)
        let persisted = subscribedChatIds

        let toSubscribe = Array(current.subtracting(persisted))
        let toUnsubscribe = Array(persisted.subtracting(current))

        if !toSubscribe.isEmpty {
            Task {
                await subscribeToGroups(toSubscribe)
            }
        }

        if !toUnsubscribe.isEmpty {
            Task {
                await unsubscribeFromGroups(toUnsubscribe)
            }
        }

        subscribedChatIds = current
    }

    /// Unsubscribe this device from a set of group IDs.
    func unsubscribeFromGroups(_ groupIds: [String]) async {
        guard !groupIds.isEmpty else { return }

        let url = serverURL.appendingPathComponent("unsubscribe-groups")
        let body: [String: Any] = [
            "id": deviceId,
            "group_ids": groupIds
        ]

        do {
            let result = try await postJSON(url: url, body: body)
            logger.info("Unsubscribed from groups \(groupIds): \(result)")
        } catch {
            logger.error("Failed to unsubscribe from groups: \(error.localizedDescription)")
        }
    }

    /// Unsubscribe from all groups on the server and clear persisted state (for logout).
    func clearSubscriptions() {
        let ids = Array(subscribedChatIds)
        if !ids.isEmpty {
            Task {
                await unsubscribeFromGroups(ids)
            }
        }
        subscribedChatIds = []
        lastChatCount = -1
    }

    // MARK: - Networking

    private func postJSON(url: URL, body: Any) async throws -> String {
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)

        logger.debug("POST \(url.absoluteString) body=\(String(data: request.httpBody!, encoding: .utf8) ?? "")")

        let (data, response) = try await URLSession.shared.data(for: request)
        let statusCode = (response as? HTTPURLResponse)?.statusCode ?? 0
        let responseBody = String(data: data, encoding: .utf8) ?? ""

        logger.debug("Response \(statusCode): \(responseBody)")

        guard (200...299).contains(statusCode) else {
            throw NSError(
                domain: "PushNotificationManager",
                code: statusCode,
                userInfo: [NSLocalizedDescriptionKey: "HTTP \(statusCode): \(responseBody)"]
            )
        }

        return responseBody
    }
}
