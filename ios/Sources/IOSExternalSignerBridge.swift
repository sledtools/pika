import Foundation
import UIKit

final class IOSExternalSignerBridge: ExternalSignerBridge, @unchecked Sendable {
    private var nostrConnectCallbackUrl: String {
        let scheme = Self.callbackScheme(forBundleIdentifier: Bundle.main.bundleIdentifier)
        return "\(scheme)://nostrconnect-return"
    }

    static func callbackScheme(forBundleIdentifier bundleIdentifier: String?) -> String {
        guard let bundleIdentifier else { return "pika" }
        if bundleIdentifier.hasSuffix(".dev") { return "pika-dev" }
        if bundleIdentifier.hasSuffix(".pikatest") { return "pika-test" }
        return "pika"
    }

    func openUrl(url: String) -> ExternalSignerResult {
        let trimmed = url.trimmingCharacters(in: .whitespacesAndNewlines)
        let launchUrl = withNostrConnectCallback(trimmed)
        guard !launchUrl.isEmpty, let parsed = URL(string: launchUrl) else {
            return ExternalSignerResult(
                ok: false,
                value: nil,
                errorKind: .invalidResponse,
                errorMessage: "Invalid URL"
            )
        }
        let debugSummary = Self.nostrConnectDebugSummary(for: launchUrl)
        NSLog("%@", "[PikaSignerBridge] openUrl: \(debugSummary)")

        if ProcessInfo.processInfo.environment["PIKA_UI_TEST_CAPTURE_OPEN_URL"] == "1" {
            if let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first {
                let marker = docs.appendingPathComponent("ui_test_open_url.txt")
                try? launchUrl.write(to: marker, atomically: true, encoding: .utf8)
            }
        }

        if Thread.isMainThread {
            guard UIApplication.shared.canOpenURL(parsed) else {
                return ExternalSignerResult(
                    ok: false,
                    value: nil,
                    errorKind: .signerUnavailable,
                    errorMessage: "No app can handle URL"
                )
            }
            UIApplication.shared.open(parsed, options: [:], completionHandler: nil)
            return ExternalSignerResult(ok: true, value: nil, errorKind: nil, errorMessage: nil)
        }

        let sema = DispatchSemaphore(value: 0)
        var canOpen = false
        DispatchQueue.main.async {
            canOpen = UIApplication.shared.canOpenURL(parsed)
            if canOpen {
                // Do not block Rust on user-facing app-switch confirmation.
                // Rust owns the handshake timeout and will surface failures.
                UIApplication.shared.open(parsed, options: [:], completionHandler: nil)
            }
            sema.signal()
        }

        let wait = sema.wait(timeout: .now() + 2)
        if wait == .timedOut {
            return ExternalSignerResult(
                ok: false,
                value: nil,
                errorKind: .timeout,
                errorMessage: "Timed out scheduling URL open"
            )
        }
        if !canOpen {
            return ExternalSignerResult(
                ok: false,
                value: nil,
                errorKind: .signerUnavailable,
                errorMessage: "No app can handle URL"
            )
        }
        return ExternalSignerResult(ok: true, value: nil, errorKind: nil, errorMessage: nil)
    }

    private func withNostrConnectCallback(_ raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.lowercased().hasPrefix("nostrconnect://") else {
            return raw
        }

        if var components = URLComponents(string: trimmed) {
            var queryItems = components.queryItems ?? []
            let hasCallback = queryItems.contains(where: { $0.name.caseInsensitiveCompare("callback") == .orderedSame })
            if !hasCallback {
                queryItems.append(URLQueryItem(name: "callback", value: nostrConnectCallbackUrl))
                components.queryItems = queryItems
            }
            return components.string ?? trimmed
        }

        // Fallback path when URLComponents cannot parse signer-provided URLs.
        if trimmed.range(of: "(^|[?&])callback=", options: [.regularExpression, .caseInsensitive]) != nil {
            return trimmed
        }
        let encoded = nostrConnectCallbackUrl.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed)
            ?? nostrConnectCallbackUrl
        let separator = trimmed.contains("?") ? "&" : "?"
        return "\(trimmed)\(separator)callback=\(encoded)"
    }

    private static func nostrConnectDebugSummary(for raw: String) -> String {
        guard let components = URLComponents(string: raw) else {
            return "invalid_url"
        }
        let scheme = components.scheme ?? "url"
        let host = components.host ?? "<unknown>"
        let queryItems = components.queryItems ?? []
        let keys = Set(queryItems.map { $0.name.lowercased() })
        let relayCount = queryItems.filter {
            $0.name.caseInsensitiveCompare("relay") == .orderedSame
        }.count
        let callbackScheme = queryItems.first {
            $0.name.caseInsensitiveCompare("callback") == .orderedSame
        }
        .flatMap { item in
            guard let value = item.value else { return nil }
            return URL(string: value)?.scheme
        } ?? "-"
        return "scheme=\(scheme) host=\(host) keys=\(keys.sorted().joined(separator: ",")) relays=\(relayCount) callback_scheme=\(callbackScheme)"
    }

    func requestPublicKey(currentUserHint _: String?) -> ExternalSignerHandshakeResult {
        ExternalSignerHandshakeResult(
            ok: false,
            pubkey: nil,
            signerPackage: nil,
            currentUser: nil,
            errorKind: .signerUnavailable,
            errorMessage: "Amber signer bridge is unavailable on iOS"
        )
    }

    func signEvent(
        signerPackage _: String,
        currentUser _: String,
        unsignedEventJson _: String
    ) -> ExternalSignerResult {
        unsupported()
    }

    func nip44Encrypt(
        signerPackage _: String,
        currentUser _: String,
        peerPubkey _: String,
        content _: String
    ) -> ExternalSignerResult {
        unsupported()
    }

    func nip44Decrypt(
        signerPackage _: String,
        currentUser _: String,
        peerPubkey _: String,
        payload _: String
    ) -> ExternalSignerResult {
        unsupported()
    }

    func nip04Encrypt(
        signerPackage _: String,
        currentUser _: String,
        peerPubkey _: String,
        content _: String
    ) -> ExternalSignerResult {
        unsupported()
    }

    func nip04Decrypt(
        signerPackage _: String,
        currentUser _: String,
        peerPubkey _: String,
        payload _: String
    ) -> ExternalSignerResult {
        unsupported()
    }

    private func unsupported() -> ExternalSignerResult {
        ExternalSignerResult(
            ok: false,
            value: nil,
            errorKind: .signerUnavailable,
            errorMessage: "Amber signer bridge is unavailable on iOS"
        )
    }
}
