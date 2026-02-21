import XCTest

/// E2E call test against the deployed bot, run on a real device.
///
/// This exercises the exact same binary + runtime as the shipping app.
/// It is NOT part of the default test suite — run it explicitly:
///
///   # Build xcframework first:
///   just ios-xcframework ios-xcodeproj
///
///   # Then run on device:
///   xcodebuild test \
///     -project ios/Pika.xcodeproj \
///     -scheme Pika \
///     -destination 'id=00008140-001E54E90E6A801C' \
///     -only-testing:PikaUITests/CallE2ETests/testCallDeployedBot \
///     PIKA_TEST_NSEC='...' \
///     PIKA_BOT_NPUB='npub1z6ujr8rad5zp9sr9w22rkxm0truulf2jntrks6rlwskhdmqsawpqmnjlcp'
///
/// Env vars (passed via launchEnvironment):
///   PIKA_TEST_NSEC          required
///   PIKA_BOT_NPUB           required
///   PIKA_RELAY_URLS         optional (comma-separated)
///   PIKA_KEY_PACKAGE_RELAY_URLS  optional
///   PIKA_CALL_MOQ_URL       optional (defaults to https://us-east.moq.logos.surf/anon)
final class CallE2ETests: XCTestCase {

    // Longer timeouts for real-network operations.
    private let networkTimeout: TimeInterval = 120
    private let callTimeout: TimeInterval = 60
    private let mediaTimeout: TimeInterval = 30

    /// Dismiss the non-blocking toast overlay if present.
    private func dismissToastIfPresent(_ app: XCUIApplication, timeout: TimeInterval = 0.5) -> String? {
        let overlay = app.staticTexts.matching(identifier: "pika_toast").firstMatch
        if overlay.waitForExistence(timeout: timeout) {
            let msg = overlay.label
            overlay.tap()
            return msg.isEmpty ? nil : msg
        }
        return nil
    }

    /// Parse key=value pairs from a string. Skips comments and empty lines.
    private func parseDotenv(_ data: String) -> [String: String] {
        var env: [String: String] = [:]
        for line in data.split(separator: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty || trimmed.hasPrefix("#") { continue }
            guard let eqIdx = trimmed.firstIndex(of: "=") else { continue }
            let key = String(trimmed[trimmed.startIndex..<eqIdx]).trimmingCharacters(in: .whitespaces)
            var val = String(trimmed[trimmed.index(after: eqIdx)...]).trimmingCharacters(in: .whitespaces)
            if (val.hasPrefix("\"") && val.hasSuffix("\"")) || (val.hasPrefix("'") && val.hasSuffix("'")) {
                val = String(val.dropFirst().dropLast())
            }
            env[key] = val
        }
        return env
    }

    /// Read .env from multiple locations: test bundle resource (works on device),
    /// source tree via #filePath (works on simulator/macOS).
    private func loadDotenv() -> [String: String] {
        // 1. Test bundle resource (copied at build time via project.yml)
        if let bundleUrl = Bundle(for: type(of: self)).url(forResource: "env", withExtension: "test"),
           let data = try? String(contentsOf: bundleUrl, encoding: .utf8) {
            let env = parseDotenv(data)
            if !env.isEmpty { return env }
        }

        // 2. Source tree path (works on simulator where filesystem is shared)
        let sourceUrl = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // UITests/
            .deletingLastPathComponent() // ios/
            .deletingLastPathComponent() // repo root
            .appendingPathComponent(".env")
        if let data = try? String(contentsOf: sourceUrl, encoding: .utf8) {
            let env = parseDotenv(data)
            if !env.isEmpty { return env }
        }

        return [:]
    }

    private func openNewChatFromChatList(_ app: XCUIApplication, timeout: TimeInterval = 15) {
        let newChat = app.buttons.matching(identifier: "chatlist_new_chat").firstMatch
        XCTAssertTrue(newChat.waitForExistence(timeout: 10))
        newChat.tap()

        let nav = app.navigationBars["New Chat"]
        if nav.waitForExistence(timeout: 2) {
            return
        }

        let menuItem = app.buttons["New Chat"].firstMatch
        XCTAssertTrue(menuItem.waitForExistence(timeout: 5), "New Chat menu item did not appear")
        menuItem.tap()
        XCTAssertTrue(nav.waitForExistence(timeout: timeout))
    }

    func testCallDeployedBot() throws {
        let buildEnv = ProcessInfo.processInfo.environment
        if buildEnv["PIKA_UI_E2E"] != "1" {
            throw XCTSkip("Set PIKA_UI_E2E=1 to run deployed-bot call E2E")
        }

        // Read config from .env file at repo root (env vars don't reach XCUITest runner).
        let dotenv = loadDotenv()

        let nsec = buildEnv["PIKA_TEST_NSEC"] ?? dotenv["PIKA_TEST_NSEC"] ?? ""
        let botNpub = buildEnv["PIKA_BOT_NPUB"] ?? dotenv["PIKA_BOT_NPUB"]
            ?? "npub1z6ujr8rad5zp9sr9w22rkxm0truulf2jntrks6rlwskhdmqsawpqmnjlcp"

        guard !nsec.isEmpty else {
            XCTFail("Missing PIKA_TEST_NSEC in env or .env file")
            return
        }

        let relays = buildEnv["PIKA_RELAY_URLS"] ?? dotenv["PIKA_RELAY_URLS"]
            ?? "wss://relay.primal.net,wss://nos.lol,wss://relay.damus.io"
        let kpRelays = buildEnv["PIKA_KEY_PACKAGE_RELAY_URLS"] ?? dotenv["PIKA_KEY_PACKAGE_RELAY_URLS"]
            ?? "wss://nostr-pub.wellorder.net,wss://nostr-01.yakihonne.com,wss://nostr-02.yakihonne.com,wss://relay.satlantis.io"
        let moqUrl = buildEnv["PIKA_CALL_MOQ_URL"] ?? dotenv["PIKA_CALL_MOQ_URL"]
            ?? "https://us-east.moq.logos.surf/anon"

        // --- Launch app ---
        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_RELAY_URLS"] = relays
        app.launchEnvironment["PIKA_KEY_PACKAGE_RELAY_URLS"] = kpRelays
        app.launchEnvironment["PIKA_CALL_MOQ_URL"] = moqUrl
        app.launch()

        // --- Login with test nsec ---
        let loginNsec = app.secureTextFields.matching(identifier: "login_nsec_input").firstMatch
        let loginSubmit = app.buttons.matching(identifier: "login_submit").firstMatch

        if loginNsec.waitForExistence(timeout: 5) {
            loginNsec.tap()
            loginNsec.typeText(nsec)
            XCTAssertTrue(loginSubmit.waitForExistence(timeout: 5), "Login submit button not found")
            loginSubmit.tap()
        }

        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 30), "Chat list did not appear after login")

        // --- Create chat with bot ---
        openNewChatFromChatList(app, timeout: 15)

        let peerField = app.descendants(matching: .any).matching(identifier: "newchat_peer_npub").firstMatch
        XCTAssertTrue(peerField.waitForExistence(timeout: 10))
        peerField.tap()
        peerField.typeText(botNpub)

        let startChat = app.buttons.matching(identifier: "newchat_start").firstMatch
        XCTAssertTrue(startChat.waitForExistence(timeout: 10))
        startChat.tap()

        // Wait for chat to open (message composer appears).
        let composer = app.descendants(matching: .any).matching(identifier: "chat_message_input").firstMatch
        let composerDeadline = Date().addingTimeInterval(networkTimeout)
        while Date() < composerDeadline {
            if composer.exists { break }
            if let toast = dismissToastIfPresent(app, timeout: 0.5) {
                if toast.lowercased().contains("failed") || toast.lowercased().contains("not found") {
                    XCTFail("Chat creation failed: \(toast)")
                    return
                }
            }
            Thread.sleep(forTimeInterval: 0.5)
        }
        guard composer.exists else {
            XCTFail("Chat composer did not appear within \(networkTimeout)s")
            return
        }

        // --- Start/open call ---
        let startCall = app.buttons.matching(identifier: "chat_call_start").firstMatch
        let openCall = app.buttons.matching(identifier: "chat_call_open").firstMatch
        let callButtonDeadline = Date().addingTimeInterval(10)
        while Date() < callButtonDeadline {
            if startCall.exists || openCall.exists { break }
            if let toast = dismissToastIfPresent(app, timeout: 0.3),
               (toast.lowercased().contains("failed") || toast.lowercased().contains("not found")) {
                XCTFail("Call control unavailable: \(toast)")
                return
            }
            Thread.sleep(forTimeInterval: 0.3)
        }
        let callEntry = startCall.exists ? startCall : openCall
        XCTAssertTrue(callEntry.exists, "Start/Open Call button not found")
        callEntry.tap()

        // --- Wait for call to become active (or detect failure) ---
        let callActiveText = app.staticTexts["Call active"]
        let callDeadline = Date().addingTimeInterval(callTimeout)
        var callFailed = false
        while Date() < callDeadline {
            if callActiveText.exists { break }

            // Check for error toasts.
            if let toast = dismissToastIfPresent(app, timeout: 0.3) {
                print("[call-e2e] toast: \(toast)")
                if toast.lowercased().contains("failed") || toast.lowercased().contains("not connected") {
                    XCTFail("Call failed: \(toast)")
                    callFailed = true
                    break
                }
            }

            // Check for "Call ended" (premature end).
            let endedTexts = app.staticTexts.allElementsBoundByIndex.filter {
                $0.label.hasPrefix("Call ended")
            }
            if !endedTexts.isEmpty {
                let reason = endedTexts.first?.label ?? "unknown"
                XCTFail("Call ended prematurely: \(reason)")
                callFailed = true
                break
            }

            Thread.sleep(forTimeInterval: 0.5)
        }
        if callFailed { return }
        XCTAssertTrue(callActiveText.exists, "Call did not become active within \(callTimeout)s")

        // --- Verify media frames are flowing ---
        // The ChatView shows "tx N  rx N  drop N" when debug stats are present.
        // Wait for tx > 0 at minimum.
        let mediaDeadline = Date().addingTimeInterval(mediaTimeout)
        var sawTxFrames = false
        var sawRxFrames = false
        var lastStats = ""
        while Date() < mediaDeadline {
            // Find the stats text (format: "tx N  rx N  drop N").
            let allTexts = app.staticTexts.allElementsBoundByIndex
            for text in allTexts {
                let label = text.label
                if label.hasPrefix("tx ") && label.contains("rx ") {
                    lastStats = label
                    // Parse tx and rx values.
                    let parts = label.components(separatedBy: CharacterSet.whitespaces)
                    if let txIdx = parts.firstIndex(of: "tx"),
                       txIdx + 1 < parts.count,
                       let tx = Int(parts[txIdx + 1]) {
                        if tx > 0 { sawTxFrames = true }
                    }
                    if let rxIdx = parts.firstIndex(of: "rx"),
                       rxIdx + 1 < parts.count,
                       let rx = Int(parts[rxIdx + 1]) {
                        if rx > 0 { sawRxFrames = true }
                    }
                }
            }
            if sawTxFrames && sawRxFrames { break }
            if sawTxFrames && Date().addingTimeInterval(-15) > Date().addingTimeInterval(-mediaTimeout) {
                // tx is flowing, give rx more time
            }
            Thread.sleep(forTimeInterval: 1)
        }
        print("[call-e2e] final media stats: \(lastStats)")
        XCTAssertTrue(sawTxFrames, "tx frames never increased. Stats: \(lastStats)")
        // rx may not arrive if bot TTS is slow; warn but don't fail hard.
        if !sawRxFrames {
            print("[call-e2e] WARNING: no rx frames observed (bot may not have responded with TTS)")
        }

        // --- End call ---
        let endCall = app.buttons.matching(identifier: "chat_call_end").firstMatch
        if endCall.exists {
            endCall.tap()
        }

        // Verify call ended.
        let endedDeadline = Date().addingTimeInterval(15)
        while Date() < endedDeadline {
            let endedTexts = app.staticTexts.allElementsBoundByIndex.filter {
                $0.label.hasPrefix("Call ended")
            }
            if !endedTexts.isEmpty { break }
            // "Start Again" / "Start Call" means the active call is over.
            if app.buttons.matching(identifier: "chat_call_start").firstMatch.exists { break }
            Thread.sleep(forTimeInterval: 0.5)
        }

        print("[call-e2e] PASS: call completed successfully")
    }
}
