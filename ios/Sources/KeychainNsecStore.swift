import Foundation
import Security
import os.log

private let keychainLog = Logger(subsystem: "com.pika.app", category: "Keychain")

/// Stores the nsec in the iOS Keychain, with an automatic file-based fallback
/// when the keychain is unavailable (e.g. simulator builds without entitlements,
/// which fail with errSecMissingEntitlement / -34018).
final class KeychainNsecStore {
    private let service = "com.pika.app"
    private let account = "nsec"

    /// Lazily determined: `true` when keychain operations return `-34018`.
    /// Once set, all subsequent operations use the file fallback.
    /// Only allowed on simulator builds — production builds crash on keychain failure.
    private var useFileFallback: Bool = false

    // MARK: - Public API

    func getNsec() -> String? {
        if useFileFallback {
            return fileGet()
        }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecSuccess, let data = item as? Data,
           let nsec = String(data: data, encoding: .utf8), !nsec.isEmpty {
            keychainLog.info("getNsec: found stored nsec (keychain)")
            return nsec
        }
        if status == -34018 {
            switchToFileFallback(context: "getNsec")
            return fileGet()
        }
        keychainLog.warning("getNsec: no nsec found (OSStatus=\(status))")
        return nil
    }

    func setNsec(_ nsec: String) {
        if useFileFallback {
            fileSet(nsec)
            return
        }
        let data = Data(nsec.utf8)
        let baseQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]

        let addQuery = baseQuery.merging([
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]) { $1 }
        let status = SecItemAdd(addQuery as CFDictionary, nil)
        if status == errSecSuccess {
            keychainLog.info("setNsec: stored via SecItemAdd (keychain)")
            return
        }
        if status == errSecDuplicateItem {
            let attrs: [String: Any] = [kSecValueData as String: data]
            let updateStatus = SecItemUpdate(baseQuery as CFDictionary, attrs as CFDictionary)
            if updateStatus == errSecSuccess {
                keychainLog.info("setNsec: updated via SecItemUpdate (keychain)")
            } else {
                keychainLog.error("setNsec: SecItemUpdate failed (OSStatus=\(updateStatus))")
            }
            return
        }
        if status == -34018 {
            switchToFileFallback(context: "setNsec")
            fileSet(nsec)
            return
        }
        keychainLog.error("setNsec: SecItemAdd failed (OSStatus=\(status))")
    }

    func clearNsec() {
        // Clear both stores so state is consistent regardless of which was active.
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let status = SecItemDelete(query as CFDictionary)
        keychainLog.info("clearNsec: keychain OSStatus=\(status)")

        if let url = fileFallbackURL() {
            try? FileManager.default.removeItem(at: url)
            keychainLog.info("clearNsec: removed file fallback")
        }
    }

    /// Switch to the file-based fallback. Only allowed on simulator — production builds
    /// must have working keychain access, so we crash immediately to surface the issue.
    private func switchToFileFallback(context: String) {
        #if targetEnvironment(simulator)
        keychainLog.warning("\(context): keychain unavailable (OSStatus=-34018), switching to file fallback")
        useFileFallback = true
        #else
        fatalError("Keychain unavailable (errSecMissingEntitlement / -34018) during \(context). "
                   + "This must not happen in a production build — check entitlements and provisioning.")
        #endif
    }

    // MARK: - File fallback (Application Support / .pika_nsec, simulator only)

    private func fileFallbackURL() -> URL? {
        guard let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else {
            return nil
        }
        return dir.appendingPathComponent(".pika_nsec")
    }

    private func fileGet() -> String? {
        guard let url = fileFallbackURL() else { return nil }
        guard let data = try? Data(contentsOf: url),
              let nsec = String(data: data, encoding: .utf8), !nsec.isEmpty else {
            keychainLog.warning("getNsec: no nsec found (file fallback)")
            return nil
        }
        keychainLog.info("getNsec: found stored nsec (file fallback)")
        return nsec
    }

    private func fileSet(_ nsec: String) {
        guard let url = fileFallbackURL() else {
            keychainLog.error("setNsec: could not determine file fallback path")
            return
        }
        do {
            let dir = url.deletingLastPathComponent()
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try Data(nsec.utf8).write(to: url, options: [.atomic, .completeFileProtection])
            keychainLog.info("setNsec: stored via file fallback")
        } catch {
            keychainLog.error("setNsec: file fallback write failed: \(error.localizedDescription)")
        }
    }
}

