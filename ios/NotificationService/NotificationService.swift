import UserNotifications
import Foundation
import ImageIO
import Intents
import Security
import UniformTypeIdentifiers

class NotificationService: UNNotificationServiceExtension {

    private var contentHandler: ((UNNotificationContent) -> Void)?
    private var bestAttemptContent: UNMutableNotificationContent?

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        self.contentHandler = contentHandler
        // Default to empty content so that if the NSE times out we suppress
        // rather than showing the server's generic "New message" fallback.
        bestAttemptContent = UNMutableNotificationContent()

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
            content.body = msg.content
            content.userInfo["chat_id"] = msg.chatId
            content.threadIdentifier = msg.chatId

            // Attach decrypted image thumbnail if available.
            if let imageData = msg.imageData {
                if let attachment = Self.createImageAttachment(data: Data(imageData)) {
                    content.attachments = [attachment]
                }
            }

            if let urlStr = msg.senderPictureUrl, let url = URL(string: urlStr) {
                Self.downloadAvatar(url: url) { image in
                    let updated = Self.applyCommNotification(
                        to: content,
                        senderName: msg.senderName,
                        senderPubkey: msg.senderPubkey,
                        chatId: msg.chatId,
                        isGroup: msg.isGroup,
                        groupName: msg.groupName,
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
                    isGroup: msg.isGroup,
                    groupName: msg.groupName,
                    senderImage: nil
                )
                contentHandler(updated)
            }
        case .callInvite(let chatId, let callId, let callerName, let callerPictureUrl, let isVideo):
            content.title = callerName
            content.body = isVideo ? "Incoming video call" : "Incoming call"
            content.sound = .defaultCritical
            content.userInfo["chat_id"] = chatId
            content.userInfo["call_id"] = callId
            content.threadIdentifier = chatId

            if let urlStr = callerPictureUrl, let url = URL(string: urlStr) {
                Self.downloadAvatar(url: url) { image in
                    let updated = Self.applyCommNotification(
                        to: content,
                        senderName: callerName,
                        senderPubkey: chatId,
                        chatId: chatId,
                        senderImage: image
                    )
                    contentHandler(updated)
                }
            } else {
                let updated = Self.applyCommNotification(
                    to: content,
                    senderName: callerName,
                    senderPubkey: chatId,
                    chatId: chatId,
                    senderImage: nil
                )
                contentHandler(updated)
            }
        case .error(let message):
            let errContent = UNMutableNotificationContent()
            errContent.body = "[error] \(message)"
            contentHandler(errContent)
        case nil:
            // Suppressed: self-message, call signal, non-app MLS message, etc.
            let suppressed = UNMutableNotificationContent()
            suppressed.body = "[suppressed]"
            contentHandler(suppressed)
        }
    }

    /// Create an INSendMessageIntent so iOS shows the sender's avatar as the notification icon.
    private static func applyCommNotification(
        to content: UNMutableNotificationContent,
        senderName: String,
        senderPubkey: String,
        chatId: String,
        isGroup: Bool = false,
        groupName: String? = nil,
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
        // For groups, iOS requires recipients to treat the intent as a group
        // conversation. Provide the sender as a recipient placeholder so that
        // speakableGroupName is used as the title and sender as the subtitle.
        let speakableGroup: INSpeakableString?
        let recipients: [INPerson]?
        if isGroup {
            speakableGroup = INSpeakableString(spokenPhrase: groupName ?? "Group")
            recipients = [sender]
        } else {
            speakableGroup = nil
            recipients = nil
        }
        let intent = INSendMessageIntent(
            recipients: recipients,
            outgoingMessageType: .outgoingMessageText,
            content: nil,
            speakableGroupName: speakableGroup,
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
        return (try? content.updating(from: intent)) ?? content
    }

    /// Save decrypted image data to a temp file and create a notification attachment.
    private static func createImageAttachment(data: Data) -> UNNotificationAttachment? {
        let tmpDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("notif-images", isDirectory: true)
        try? FileManager.default.createDirectory(at: tmpDir, withIntermediateDirectories: true)

        // Detect the actual image type so iOS can decode it correctly.
        var uti: String = UTType.jpeg.identifier
        var ext: String = "jpg"
        if let source = CGImageSourceCreateWithData(data as CFData, nil),
           let detectedUTI = CGImageSourceGetType(source) as String? {
            uti = detectedUTI
            if let type = UTType(detectedUTI), let preferred = type.preferredFilenameExtension {
                ext = preferred
            }
        }

        let fileURL = tmpDir.appendingPathComponent("\(UUID().uuidString).\(ext)")
        do {
            try data.write(to: fileURL)
            return try UNNotificationAttachment(
                identifier: "image",
                url: fileURL,
                options: [UNNotificationAttachmentOptionsTypeHintKey: uti]
            )
        } catch {
            return nil
        }
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
