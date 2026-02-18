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
    /// The keychain access group shared between the main app and the NSE.
    /// On simulator, nil (shared groups aren't supported). On device, the full
    /// qualified group: "<TeamID>.<bundle_id>.shared".
    private let accessGroup: String?

    /// Controls whether the file fallback is permitted.
    /// Default: `true` on simulator, `false` on device (compile-time).
    /// Tests can pass `false` to verify the production crash path.
    let fileFallbackAllowed: Bool

    /// Called when file fallback is attempted but `fileFallbackAllowed` is false.
    /// Default: `fatalError()`. Tests replace this to intercept the crash.
    var onFileFallbackDenied: ((String) -> Void)?

    /// Lazily determined: `true` when keychain operations return `-34018`
    /// and fallback is allowed.
    private var useFileFallback: Bool = false

    init(keychainGroup: String? = nil, fileFallbackAllowed: Bool? = nil) {
        #if targetEnvironment(simulator)
        self.accessGroup = nil
        #else
        self.accessGroup = keychainGroup?.isEmpty == false ? keychainGroup : nil
        #endif

        if let explicit = fileFallbackAllowed {
            self.fileFallbackAllowed = explicit
        } else {
            #if targetEnvironment(simulator)
            self.fileFallbackAllowed = true
            #else
            self.fileFallbackAllowed = false
            #endif
        }
    }

    /// Build a base keychain query dict, conditionally including the access group.
    private func baseQuery() -> [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        if let group = accessGroup {
            q[kSecAttrAccessGroup as String] = group
        }
        return q
    }

    // MARK: - Public API

    func getNsec() -> String? {
        if useFileFallback {
            return fileGet()
        }
        var query = baseQuery()
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

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
        let base = baseQuery()

        var addQuery = base
        addQuery[kSecValueData as String] = data
        addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(addQuery as CFDictionary, nil)
        if status == errSecSuccess {
            keychainLog.info("setNsec: stored via SecItemAdd (keychain)")
            return
        }
        if status == errSecDuplicateItem {
            let attrs: [String: Any] = [kSecValueData as String: data]
            let updateStatus = SecItemUpdate(base as CFDictionary, attrs as CFDictionary)
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
        let status = SecItemDelete(baseQuery() as CFDictionary)
        keychainLog.info("clearNsec: keychain OSStatus=\(status)")

        if let url = fileFallbackURL() {
            try? FileManager.default.removeItem(at: url)
            keychainLog.info("clearNsec: removed file fallback")
        }
    }

    /// Switch to the file-based fallback. Only allowed when `fileFallbackAllowed` is true
    /// (simulator by default). Otherwise crashes via `fatalError` — or calls `onFileFallbackDenied`
    /// if set (for test interception).
    private func switchToFileFallback(context: String) {
        let msg = "Keychain unavailable (errSecMissingEntitlement / -34018) during \(context). "
            + "This must not happen in a production build — check entitlements and provisioning."
        guard fileFallbackAllowed else {
            if let handler = onFileFallbackDenied {
                handler(msg)
            } else {
                fatalError(msg)
            }
            return
        }
        keychainLog.warning("\(context): keychain unavailable (OSStatus=-34018), switching to file fallback")
        useFileFallback = true
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
