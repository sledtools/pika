import UserNotifications
import Foundation
import Intents
import Security

class NotificationService: UNNotificationServiceExtension {

    private var contentHandler: ((UNNotificationContent) -> Void)?
    private var bestAttemptContent: UNMutableNotificationContent?

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        self.contentHandler = contentHandler
        bestAttemptContent = request.content.mutableCopy() as? UNMutableNotificationContent

        guard let content = bestAttemptContent,
              let eventJson = request.content.userInfo["nostr_event"] as? String else {
            contentHandler(request.content)
            return
        }

        guard let nsec = SharedKeychainHelper.getNsec() else {
            contentHandler(request.content)
            return
        }

        let appGroup = Bundle.main.infoDictionary?["PikaAppGroup"] as? String ?? "group.org.pikachat.pika"
        let keychainGroup = Bundle.main.infoDictionary?["PikaKeychainGroup"] as? String ?? ""

        let dataDir = FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: appGroup)!
            .appendingPathComponent("Library/Application Support").path

        switch decryptPushNotification(dataDir: dataDir, nsec: nsec, eventJson: eventJson, keychainGroup: keychainGroup) {
        case .content(let msg):
            if msg.isGroup {
                content.title = msg.groupName ?? "Group"
                content.subtitle = msg.senderName
                content.body = msg.content
            } else {
                content.title = msg.senderName
                content.body = msg.content
            }
            content.userInfo["chat_id"] = msg.chatId
            content.threadIdentifier = msg.chatId

            if let urlStr = msg.senderPictureUrl, let url = URL(string: urlStr) {
                Self.downloadAvatar(url: url) { image in
                    let updated = Self.applyCommNotification(
                        to: content,
                        senderName: msg.senderName,
                        senderPubkey: msg.senderPubkey,
                        chatId: msg.chatId,
                        senderImage: image
                    )
                    contentHandler(updated)
                }
            } else {
                let updated = Self.applyCommNotification(
                    to: content,
                    senderName: msg.senderName,
                    senderPubkey: msg.senderPubkey,
                    chatId: msg.chatId,
                    senderImage: nil
                )
                contentHandler(updated)
            }
        case .suppress, nil:
            // Suppress: self-message, call signal, already processed, or decrypt failure.
            // Deliver a fresh empty content so iOS has no alert to display.
            contentHandler(UNMutableNotificationContent())
        }
    }

    /// Create an INSendMessageIntent so iOS shows the sender's avatar as the notification icon.
    private static func applyCommNotification(
        to content: UNMutableNotificationContent,
        senderName: String,
        senderPubkey: String,
        chatId: String,
        senderImage: INImage?
    ) -> UNNotificationContent {
        let handle = INPersonHandle(value: senderPubkey, type: .unknown)
        let sender = INPerson(
            personHandle: handle,
            nameComponents: nil,
            displayName: senderName,
            image: senderImage,
            contactIdentifier: nil,
            customIdentifier: senderPubkey
        )
        let intent = INSendMessageIntent(
            recipients: nil,
            outgoingMessageType: .outgoingMessageText,
            content: nil,
            speakableGroupName: nil,
            conversationIdentifier: chatId,
            serviceName: nil,
            sender: sender,
            attachments: nil
        )
        if let senderImage {
            intent.setImage(senderImage, forParameterNamed: \.sender)
        }
        let interaction = INInteraction(intent: intent, response: nil)
        interaction.direction = .incoming
        interaction.donate(completion: nil)
        let updated = try? content.updating(from: intent)
        return updated ?? content
    }

    /// Download an image and return it as an INImage.
    private static func downloadAvatar(url: URL, completion: @escaping (INImage?) -> Void) {
        let task = URLSession.shared.dataTask(with: url) { data, _, _ in
            guard let data, !data.isEmpty else {
                completion(nil)
                return
            }
            completion(INImage(imageData: data))
        }
        task.resume()
    }

    override func serviceExtensionTimeWillExpire() {
        if let contentHandler, let bestAttemptContent {
            contentHandler(bestAttemptContent)
        }
    }
}

/// Reads the nsec from the shared keychain access group.
enum SharedKeychainHelper {
    private static let service = "com.pika.app"
    private static let account = "nsec"

    static func getNsec() -> String? {
        let accessGroup = Bundle.main.infoDictionary?["PikaKeychainGroup"] as? String ?? ""
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        if !accessGroup.isEmpty {
            query[kSecAttrAccessGroup as String] = accessGroup
        }
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess,
              let data = item as? Data,
              let nsec = String(data: data, encoding: .utf8),
              !nsec.isEmpty else {
            return nil
        }
        return nsec
    }
}
