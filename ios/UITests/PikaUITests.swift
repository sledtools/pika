import XCTest
import UIKit

/// Platform-hosted UI smoke for navigation, persistence, layout, and deep-link capability.
///
/// Core Rust messaging/profile semantics should stay owned by `rust/tests` and deterministic
/// `pikahut` selectors, not by this XCTest layer.
final class PikaUITests: XCTestCase {
    private let localUiTestNsec = "nsec1ds2my67qq6ls28vyms056cwz460w6nkpemxmqejgk77d6xw277as0692cq"
    private let localUiTestNpub = "npub1q49v9chr3cqt2gectr0g98aj4sw3dch8tpnur0yl9q9gxg26e2ysdl24fy"

    private func dismissSystemOpenAppAlertIfPresent(timeout: TimeInterval = 5) {
        let app = XCUIApplication()
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let deadline = Date().addingTimeInterval(timeout)

        while Date() < deadline {
            let appAlert = app.alerts.firstMatch
            if appAlert.exists {
                let cancel = appAlert.buttons["Cancel"]
                if cancel.exists {
                    cancel.tap()
                    return
                }
                let open = appAlert.buttons["Open"]
                if open.exists {
                    open.tap()
                    return
                }
                appAlert.buttons.element(boundBy: 0).tap()
                return
            }

            let sbAlert = springboard.alerts.firstMatch
            if sbAlert.exists {
                let cancel = sbAlert.buttons["Cancel"]
                if cancel.exists {
                    cancel.tap()
                    return
                }
                let open = sbAlert.buttons["Open"]
                if open.exists {
                    open.tap()
                    return
                }
                sbAlert.buttons.element(boundBy: 0).tap()
                return
            }

            Thread.sleep(forTimeInterval: 0.1)
        }
    }

    /// Dismiss the non-blocking toast overlay if present. Returns the toast message, or nil.
    private func dismissPikaToastIfPresent(_ app: XCUIApplication, timeout: TimeInterval = 0.5) -> String? {
        // New: non-blocking overlay with accessibility identifier.
        let overlay = app.staticTexts.matching(identifier: "pika_toast").firstMatch
        if overlay.waitForExistence(timeout: timeout) {
            let msg = overlay.label
            overlay.tap() // dismiss by tapping
            return msg.isEmpty ? nil : msg
        }

        // Legacy fallback: modal alert (kept for backwards compatibility during transition).
        let alert = app.alerts["Pika"]
        guard alert.waitForExistence(timeout: 0.1) else { return nil }

        let msg = alert.staticTexts
            .allElementsBoundByIndex
            .map(\.label)
            .filter { !$0.isEmpty && $0 != "Pika" }
            .joined(separator: " ")

        let ok = alert.buttons["OK"]
        if ok.exists { ok.tap() }
        else { alert.buttons.element(boundBy: 0).tap() }
        return msg.isEmpty ? nil : msg
    }

    private func dismissAllPikaToasts(_ app: XCUIApplication, maxSeconds: TimeInterval = 10) -> [String] {
        let deadline = Date().addingTimeInterval(maxSeconds)
        var messages: [String] = []
        while Date() < deadline {
            if let msg = dismissPikaToastIfPresent(app, timeout: 0.25) {
                messages.append(msg)
                continue
            }
            Thread.sleep(forTimeInterval: 0.1)
        }
        return messages
    }

    private func waitForLoginScreen(_ app: XCUIApplication, timeout: TimeInterval = 15) {
        let createAccount = app.buttons.matching(identifier: "login_create_account").firstMatch
        let loginSubmit = app.buttons.matching(identifier: "login_submit").firstMatch
        let advancedButton = app.buttons["Advanced"].firstMatch
        let deadline = Date().addingTimeInterval(timeout)

        while Date() < deadline {
            if createAccount.exists || loginSubmit.exists || advancedButton.exists {
                return
            }
            Thread.sleep(forTimeInterval: 0.1)
        }

        XCTFail("Login screen not shown")
    }

    private func swipeLoginScreenUp(_ app: XCUIApplication) {
        let collectionView = app.collectionViews.firstMatch
        if collectionView.exists {
            collectionView.swipeUp()
        } else {
            app.swipeUp()
        }
    }

    private func expandAdvancedLoginOptionsIfNeeded(_ app: XCUIApplication, timeout: TimeInterval = 5) {
        let nostrConnectButton = app.buttons.matching(identifier: "login_nostr_connect_submit").firstMatch
        if nostrConnectButton.exists {
            return
        }

        let advancedButton = app.buttons["Advanced"].firstMatch
        let deadline = Date().addingTimeInterval(timeout)

        while Date() < deadline {
            if nostrConnectButton.exists {
                return
            }

            if advancedButton.exists {
                if !advancedButton.isHittable {
                    swipeLoginScreenUp(app)
                    Thread.sleep(forTimeInterval: 0.2)
                    continue
                }

                advancedButton.tap()
                if nostrConnectButton.waitForExistence(timeout: 1) {
                    return
                }
            }

            swipeLoginScreenUp(app)
            Thread.sleep(forTimeInterval: 0.1)
        }
    }

    private func revealLoginAdvancedControlIfNeeded(
        _ app: XCUIApplication,
        control: XCUIElement,
        maxSwipes: Int = 6
    ) {
        for _ in 0..<maxSwipes {
            if control.exists && control.isHittable {
                return
            }

            swipeLoginScreenUp(app)
            Thread.sleep(forTimeInterval: 0.2)
        }
    }

    private func openNewChatFromChatList(_ app: XCUIApplication, timeout: TimeInterval = 10) {
        let chatsNav = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNav.waitForExistence(timeout: timeout))

        // Toolbar menu buttons in the simulator can report bogus accessibility scrolling behavior.
        // Bypass the toolbar item's AX node entirely and tap the trailing corner of the main window.
        let window = app.windows.element(boundBy: 0)
        window.coordinate(withNormalizedOffset: CGVector(dx: 0.94, dy: 0.08)).tap()

        let nav = app.navigationBars["New Chat"]
        if nav.waitForExistence(timeout: 2) {
            return
        }

        // Master behavior: toolbar Menu requires selecting the "New Chat" action.
        let menuItem = app.buttons["New Chat"].firstMatch
        XCTAssertTrue(menuItem.waitForExistence(timeout: 5), "New Chat menu item did not appear")
        menuItem.tap()
        XCTAssertTrue(nav.waitForExistence(timeout: timeout))
    }

    private func openAgentProvisioningFromChatList(_ app: XCUIApplication, timeout: TimeInterval = 20) {
        let chatsNav = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNav.waitForExistence(timeout: timeout))

        let window = app.windows.element(boundBy: 0)
        let deadline = Date().addingTimeInterval(timeout)

        while Date() < deadline {
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.94, dy: 0.08)).tap()

            let agentMenuItem = app.buttons.matching(identifier: "chatlist_agent").firstMatch
            if agentMenuItem.waitForExistence(timeout: 1.0) {
                agentMenuItem.tap()

                let openclawChoice = app.buttons["OpenClaw"].firstMatch
                if openclawChoice.waitForExistence(timeout: 1.0) {
                    openclawChoice.tap()
                }
                return
            }

            // Dismiss the menu before retrying. The agent item appears only after
            // the allowlist probe resolves and the chat list re-renders.
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.10, dy: 0.20)).tap()
            Thread.sleep(forTimeInterval: 0.5)
        }

        XCTFail("Agent menu item did not appear on chat list")
    }

    private func copyMyCodeAndCloseProfile(
        _ app: XCUIApplication,
        profileNavBar: XCUIElement? = nil,
        timeout: TimeInterval = 5
    ) {
        let copyCode = app.descendants(matching: .any).matching(identifier: "chatlist_my_npub_copy").firstMatch
        XCTAssertTrue(copyCode.waitForExistence(timeout: timeout))
        copyCode.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()

        let close = app.buttons.matching(identifier: "chatlist_my_npub_close").firstMatch
        if close.exists {
            close.tap()
        } else if let profileNavBar {
            profileNavBar.buttons.element(boundBy: 0).tap()
        } else {
            app.navigationBars["Profile"].buttons.element(boundBy: 0).tap()
        }
    }

    private func copyMyCodeAndReadValue(
        _ app: XCUIApplication,
        profileNavBar: XCUIElement? = nil,
        timeout: TimeInterval = 5
    ) -> String? {
        UIPasteboard.general.string = ""

        let copyCode = app.descendants(matching: .any).matching(identifier: "chatlist_my_npub_copy").firstMatch
        XCTAssertTrue(copyCode.waitForExistence(timeout: timeout))
        copyCode.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()

        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if let value = UIPasteboard.general.string, !value.isEmpty {
                let close = app.buttons.matching(identifier: "chatlist_my_npub_close").firstMatch
                if close.exists {
                    close.tap()
                } else if let profileNavBar {
                    profileNavBar.buttons.element(boundBy: 0).tap()
                } else {
                    app.navigationBars["Profile"].buttons.element(boundBy: 0).tap()
                }
                return value
            }
            Thread.sleep(forTimeInterval: 0.1)
        }

        let close = app.buttons.matching(identifier: "chatlist_my_npub_close").firstMatch
        if close.exists {
            close.tap()
        } else if let profileNavBar {
            profileNavBar.buttons.element(boundBy: 0).tap()
        } else {
            app.navigationBars["Profile"].buttons.element(boundBy: 0).tap()
        }
        return nil
    }

    private func createNoteToSelfViaPaste(_ app: XCUIApplication, timeout: TimeInterval = 10) {
        openNewChatFromChatList(app, timeout: timeout)

        let paste = app.buttons.matching(identifier: "newchat_paste").firstMatch
        XCTAssertTrue(paste.waitForExistence(timeout: timeout))
        paste.tap()
    }

    private func loginOfflineTestIdentity(_ app: XCUIApplication, timeout: TimeInterval = 15) {
        let chatsNavBar = app.navigationBars["Chats"]
        if chatsNavBar.exists {
            return
        }

        let loginNsec = app.secureTextFields.matching(identifier: "login_nsec_input").firstMatch
        let loginSubmit = app.buttons.matching(identifier: "login_submit").firstMatch
        if loginNsec.waitForExistence(timeout: 5) && loginSubmit.waitForExistence(timeout: 5) {
            loginNsec.tap()
            loginNsec.typeText(localUiTestNsec)
            loginSubmit.tap()
        } else {
            let createAccount = app.buttons.matching(identifier: "login_create_account").firstMatch
            XCTAssertTrue(createAccount.waitForExistence(timeout: 5), "Login screen not shown")
            createAccount.tap()
        }

        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: timeout), "Chat list not shown after login")
    }

    private func createNoteToSelfChat(
        _ app: XCUIApplication,
        peerNpub: String? = nil,
        timeout: TimeInterval = 10
    ) {
        openNewChatFromChatList(app, timeout: timeout)

        let manualEntry = app.buttons.matching(identifier: "newchat_manual_entry").firstMatch
        XCTAssertTrue(manualEntry.waitForExistence(timeout: timeout))
        manualEntry.tap()

        let peer = app.descendants(matching: .any).matching(identifier: "newchat_peer_npub").firstMatch
        XCTAssertTrue(peer.waitForExistence(timeout: timeout))
        peer.tap()
        peer.typeText(peerNpub ?? localUiTestNpub)

        let start = app.buttons.matching(identifier: "newchat_start").firstMatch
        XCTAssertTrue(start.waitForExistence(timeout: timeout))
        XCTAssertTrue(start.isEnabled, "Start button should be enabled for note-to-self chat")
        start.tap()
    }

    private func waitForChatComposer(_ app: XCUIApplication, timeout: TimeInterval = 10) -> XCUIElement {
        let textView = app.textViews.matching(identifier: "chat_message_input").firstMatch
        let textField = app.textFields.matching(identifier: "chat_message_input").firstMatch
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if textView.exists { return textView }
            if textField.exists { return textField }
            Thread.sleep(forTimeInterval: 0.1)
        }
        return textView
    }

    private func isElementVisibleInViewport(
        _ element: XCUIElement,
        app: XCUIApplication,
        minVisibleHeight: CGFloat = 20
    ) -> Bool {
        guard element.exists else { return false }
        let frame = element.frame
        guard !frame.isEmpty, !frame.isNull else { return false }
        let viewport = app.windows.element(boundBy: 0).frame
        let visible = frame.intersection(viewport)
        guard !visible.isNull else { return false }
        return visible.height >= minVisibleHeight && visible.width >= 20
    }

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

    private func loadDotenv() -> [String: String] {
        var merged: [String: String] = [:]

        // 1) Bundled env copied during build phase.
        if let bundleUrl = Bundle(for: type(of: self)).url(forResource: "env", withExtension: "test"),
           let data = try? String(contentsOf: bundleUrl, encoding: .utf8) {
            merged.merge(parseDotenv(data)) { _, new in new }
        }

        // 2) Source tree overrides for local simulator runs.
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // UITests/
            .deletingLastPathComponent() // ios/
            .deletingLastPathComponent() // repo root
        for fileName in [".env", ".env.local"] {
            let url = repoRoot.appendingPathComponent(fileName)
            if let data = try? String(contentsOf: url, encoding: .utf8) {
                merged.merge(parseDotenv(data)) { _, new in new }
            }
        }

        return merged
    }

    private func loginWithTestNsecIfNeeded(
        _ app: XCUIApplication,
        testNsec: String,
        timeout: TimeInterval = 20
    ) {
        let chatsNavBar = app.navigationBars["Chats"]
        if chatsNavBar.waitForExistence(timeout: 5) {
            return
        }

        let loginNsec = app.secureTextFields.matching(identifier: "login_nsec_input").firstMatch
        let loginSubmit = app.buttons.matching(identifier: "login_submit").firstMatch
        XCTAssertTrue(loginNsec.waitForExistence(timeout: 10), "Login nsec field not shown")
        XCTAssertTrue(loginSubmit.waitForExistence(timeout: 10), "Login submit button not shown")

        loginNsec.tap()
        loginNsec.typeText(testNsec)
        loginSubmit.tap()

        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: timeout), "Chat list not shown after login")
    }

    // Platform-hosted shell smoke only: this covers iOS rendering/input/logout wiring, not the
    // canonical Rust-owned session semantics.
    func testCreateAccount_noteToSelf_sendMessage_and_logout() throws {
        let app = XCUIApplication()
        // Keep this test deterministic/offline.
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launch()
        loginOfflineTestIdentity(app)

        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 15))
        createNoteToSelfChat(app)
        // Note-to-self is synchronous; navigation to the chat happens immediately.

        // Send a message and ensure it appears.
        let msgField = app.textViews.matching(identifier: "chat_message_input").firstMatch
        let msgFieldFallback = app.textFields.matching(identifier: "chat_message_input").firstMatch
        let composer = msgField.exists ? msgField : msgFieldFallback
        XCTAssertTrue(composer.waitForExistence(timeout: 10))
        composer.tap()

        let msg = "hello from ios ui test"
        composer.typeText(msg)

        let send = app.buttons.matching(identifier: "chat_send").firstMatch
        XCTAssertTrue(send.waitForExistence(timeout: 5))
        send.tap()

        // Bubble text may not be visible if the keyboard overlaps; existence is enough.
        XCTAssertTrue(app.staticTexts[msg].waitForExistence(timeout: 10))

        // Back to chat list and logout.
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 10))

        let myNpubBtn = app.buttons.matching(identifier: "chatlist_my_npub").firstMatch
        XCTAssertTrue(myNpubBtn.waitForExistence(timeout: 5))
        myNpubBtn.tap()
        XCTAssertTrue(app.navigationBars["Profile"].waitForExistence(timeout: 5))

        let logout = app.descendants(matching: .any).matching(identifier: "chatlist_logout").firstMatch
        if !logout.exists {
            for _ in 0..<4 where !logout.exists {
                app.swipeUp()
            }
        }
        XCTAssertTrue(logout.waitForExistence(timeout: 5))
        logout.tap()

        let confirmLogout = app.buttons.matching(identifier: "chatlist_logout_confirm").firstMatch
        XCTAssertTrue(confirmLogout.waitForExistence(timeout: 5))
        confirmLogout.tap()

        XCTAssertTrue(app.staticTexts["Pika"].waitForExistence(timeout: 10))
    }

    // Platform-hosted relaunch smoke: this proves iOS auth-store/app-shell restore wiring, while
    // Rust-owned restore semantics live below this layer.
    func testSessionPersistsAcrossRelaunch() throws {
        let app = XCUIApplication()

        // --- First launch: clean slate, create account + chat ---
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launch()
        loginOfflineTestIdentity(app)

        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 15), "Chat list not shown after account creation")
        createNoteToSelfChat(app)

        // Send a message so we have something to verify after relaunch.
        let msgField = app.textViews.matching(identifier: "chat_message_input").firstMatch
        let msgFieldFallback = app.textFields.matching(identifier: "chat_message_input").firstMatch
        let composer = msgField.exists ? msgField : msgFieldFallback
        XCTAssertTrue(composer.waitForExistence(timeout: 10))
        composer.tap()
        composer.typeText("persist-test")

        let send = app.buttons.matching(identifier: "chat_send").firstMatch
        XCTAssertTrue(send.waitForExistence(timeout: 5))
        send.tap()

        XCTAssertTrue(app.staticTexts["persist-test"].waitForExistence(timeout: 10),
                       "Message not visible before relaunch")

        // Give the keychain write a moment to complete (it happens via async callback).
        Thread.sleep(forTimeInterval: 1.0)

        // --- Second launch: no reset, should restore session ---
        app.terminate()

        // Clear the reset flag so the second launch preserves keychain + data.
        app.launchEnvironment.removeValue(forKey: "PIKA_UI_TEST_RESET")
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launch()

        // We should land on the chat list, NOT the login screen.
        let loginBtn = app.buttons.matching(identifier: "login_create_account").firstMatch
        let chatsNavBar2 = app.navigationBars["Chats"]

        // Wait for either chat list or login to appear.
        let deadline = Date().addingTimeInterval(15)
        var landedOnChatList = false
        var landedOnLogin = false
        while Date() < deadline {
            if chatsNavBar2.exists {
                landedOnChatList = true
                break
            }
            if loginBtn.exists {
                landedOnLogin = true
                break
            }
            Thread.sleep(forTimeInterval: 0.1)
        }

        if landedOnLogin {
            // Check for error toasts that might explain why we're logged out.
            let toast = dismissPikaToastIfPresent(app, timeout: 2)
            XCTFail("Session was NOT restored after relaunch — landed on login screen. Toast: \(toast ?? "none")")
            return
        }

        XCTAssertTrue(landedOnChatList, "Neither chat list nor login appeared within 15s")

        // Verify the chat is still there.
        let chatCell = app.staticTexts["persist-test"]
        XCTAssertTrue(chatCell.waitForExistence(timeout: 10),
                       "Chat with 'persist-test' message not found after relaunch")
    }

    func testChatLayout_reopenAndFocusComposer_keepsLatestMessageVisible() throws {
        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launch()
        loginOfflineTestIdentity(app)

        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 15))
        createNoteToSelfChat(app)

        let composer = waitForChatComposer(app, timeout: 10)
        XCTAssertTrue(composer.exists, "Composer missing after opening chat")

        let nonce = UUID().uuidString.prefix(6)
        let messages = (0..<4).map { "layout-\(nonce)-\($0)" }
        let send = app.buttons.matching(identifier: "chat_send").firstMatch

        for msg in messages {
            composer.tap()
            composer.typeText(msg)
            XCTAssertTrue(send.waitForExistence(timeout: 5))
            send.tap()
            XCTAssertTrue(app.staticTexts[msg].waitForExistence(timeout: 10))
        }

        guard let latestMessage = messages.last else {
            XCTFail("Missing test message")
            return
        }

        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 10))

        let chatCell = app.staticTexts[latestMessage].firstMatch
        XCTAssertTrue(chatCell.waitForExistence(timeout: 10), "Expected chat preview in list")
        chatCell.tap()

        let reopenedComposer = waitForChatComposer(app, timeout: 10)
        XCTAssertTrue(reopenedComposer.exists, "Composer missing after reopening chat")

        let latestBubble = app.staticTexts[latestMessage].firstMatch
        XCTAssertTrue(latestBubble.waitForExistence(timeout: 10))
        XCTAssertTrue(
            isElementVisibleInViewport(latestBubble, app: app),
            "Latest message is off-screen right after navigating to chat"
        )

        reopenedComposer.tap()
        Thread.sleep(forTimeInterval: 0.5)

        XCTAssertTrue(
            isElementVisibleInViewport(latestBubble, app: app),
            "Latest message jumped off-screen after focusing the composer"
        )
    }

    func testLongPressMessage_showsActionsAndDismisses() throws {
        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launch()
        loginOfflineTestIdentity(app)

        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 15))
        createNoteToSelfChat(app)

        let msgField = app.textViews.matching(identifier: "chat_message_input").firstMatch
        let msgFieldFallback = app.textFields.matching(identifier: "chat_message_input").firstMatch
        let composer = msgField.exists ? msgField : msgFieldFallback
        XCTAssertTrue(composer.waitForExistence(timeout: 10))
        composer.tap()

        let msg = "longpress-ui-test-message"
        composer.typeText(msg)

        let send = app.buttons.matching(identifier: "chat_send").firstMatch
        XCTAssertTrue(send.waitForExistence(timeout: 5))
        send.tap()
        XCTAssertTrue(app.staticTexts[msg].waitForExistence(timeout: 10))

        // Long-press message text to open reactions + action card.
        let sentMessage = app.staticTexts[msg].firstMatch
        sentMessage.press(forDuration: 1.0)

        let reactionBar = app.descendants(matching: .any).matching(identifier: "chat_reaction_bar").firstMatch
        if !reactionBar.waitForExistence(timeout: 2) {
            // Retry with explicit coordinate press when XCTest misses the first long-press.
            sentMessage.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).press(forDuration: 1.2)
        }
        XCTAssertTrue(reactionBar.waitForExistence(timeout: 5))

        let actionCard = app.descendants(matching: .any).matching(identifier: "chat_action_card").firstMatch
        XCTAssertTrue(actionCard.waitForExistence(timeout: 5))

        let copy = app.buttons.matching(identifier: "chat_action_copy").firstMatch
        XCTAssertTrue(copy.waitForExistence(timeout: 5))

        // Tap outside overlays to dismiss.
        let copyButton = app.buttons.matching(identifier: "chat_action_copy").firstMatch
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.08, dy: 0.40)).tap()
        if copyButton.exists {
            // Fallback tap point if the first tap lands near overlay content.
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.92, dy: 0.72)).tap()
        }

        let dismissDeadline = Date().addingTimeInterval(2)
        while Date() < dismissDeadline, copyButton.exists {
            Thread.sleep(forTimeInterval: 0.1)
        }
        XCTAssertFalse(copyButton.exists)
    }

    // Legacy `testE2E_*` names are retained because `just ios-ui-e2e-local`
    // selects these methods directly. In steady-state policy they are
    // local-fixture bot/media flows, not part of the default `ios-ui-test`
    // simulator suite.
    func testE2E_deployedRustBot_pingPong() throws {
        let env = ProcessInfo.processInfo.environment
        let dotenv = loadDotenv()
        let botNpub = env["PIKA_UI_E2E_BOT_NPUB"] ?? dotenv["PIKA_UI_E2E_BOT_NPUB"] ?? ""
        let testNsec = env["PIKA_UI_E2E_NSEC"]
            ?? env["PIKA_TEST_NSEC"]
            ?? dotenv["PIKA_UI_E2E_NSEC"]
            ?? dotenv["PIKA_TEST_NSEC"]
            ?? ""
        let relays = env["PIKA_UI_E2E_RELAYS"] ?? dotenv["PIKA_UI_E2E_RELAYS"] ?? ""
        let kpRelays = env["PIKA_UI_E2E_KP_RELAYS"] ?? dotenv["PIKA_UI_E2E_KP_RELAYS"] ?? ""

        // Fixture-backed UI runs should be explicit. Defaults hide misconfiguration and cause flaky hangs.
        if botNpub.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_BOT_NPUB"); return }
        if testNsec.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_NSEC (or PIKA_TEST_NSEC)"); return }
        if relays.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_RELAYS"); return }
        if kpRelays.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_KP_RELAYS"); return }

        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_RELAY_URLS"] = relays
        app.launchEnvironment["PIKA_KEY_PACKAGE_RELAY_URLS"] = kpRelays
        app.launch()

        // If we land on Login, prefer logging into a stable allowlisted identity when provided.
        let createAccount = app.buttons.matching(identifier: "login_create_account").firstMatch
        if createAccount.waitForExistence(timeout: 5) {
            if !testNsec.isEmpty {
                let loginNsec = app.secureTextFields.matching(identifier: "login_nsec_input").firstMatch
                let loginSubmit = app.buttons.matching(identifier: "login_submit").firstMatch
                XCTAssertTrue(loginNsec.waitForExistence(timeout: 5))
                XCTAssertTrue(loginSubmit.waitForExistence(timeout: 5))
                loginNsec.tap()
                loginNsec.typeText(testNsec)
                loginSubmit.tap()
            } else {
                createAccount.tap()
            }
            // No blocking toasts to dismiss; navigation happens automatically.
        }

        // Chat list.
        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 10))

        // Start chat with deployed bot.
        openNewChatFromChatList(app, timeout: 10)

        let manualEntry = app.buttons.matching(identifier: "newchat_manual_entry").firstMatch
        XCTAssertTrue(manualEntry.waitForExistence(timeout: 10))
        manualEntry.tap()

        let peer = app.descendants(matching: .any).matching(identifier: "newchat_peer_npub").firstMatch
        XCTAssertTrue(peer.waitForExistence(timeout: 10))
        peer.tap()
        peer.typeText(botNpub)

        let start = app.buttons.matching(identifier: "newchat_start").firstMatch
        XCTAssertTrue(start.waitForExistence(timeout: 10))
        start.tap()

        // Chat creation runs asynchronously (key package fetch + group setup).
        // The user stays on NewChat with a loading spinner; on success the app navigates
        // directly to the chat screen. Check for error toasts while waiting.
        let composerDeadline = Date().addingTimeInterval(10)
        var chatCreationFailed = false
        let msgField = app.textViews.matching(identifier: "chat_message_input").firstMatch
        let msgFieldFallback = app.textFields.matching(identifier: "chat_message_input").firstMatch
        while Date() < composerDeadline {
            // Check if chat screen appeared.
            if msgField.exists || msgFieldFallback.exists { break }

            // Check for error toasts.
            if let errorMsg = dismissPikaToastIfPresent(app, timeout: 0.5) {
                if errorMsg.lowercased().contains("failed") ||
                    errorMsg.lowercased().contains("invalid peer key package") ||
                    errorMsg.lowercased().contains("could not find peer key package")
                {
                    XCTFail("E2E failed during chat creation: \(errorMsg)")
                    chatCreationFailed = true
                    break
                }
            }
            Thread.sleep(forTimeInterval: 0.5)
        }
        if chatCreationFailed { return }

        // Send deterministic probe.
        let nonce = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
        let probe = "ping:\(nonce)"
        let expect = "pong:\(nonce)"

        let composer = msgField.exists ? msgField : msgFieldFallback
        XCTAssertTrue(composer.waitForExistence(timeout: 10))
        composer.tap()
        composer.typeText(probe)

        let send = app.buttons.matching(identifier: "chat_send").firstMatch
        XCTAssertTrue(send.waitForExistence(timeout: 10))
        send.tap()

        // Expect deterministic ack from the bot.
        XCTAssertTrue(app.staticTexts[expect].waitForExistence(timeout: 10))
    }

    func testE2E_openclawAgent_createAndReply() throws {
        let env = ProcessInfo.processInfo.environment
        let dotenv = loadDotenv()
        let testNsec = env["PIKA_UI_E2E_NSEC"]
            ?? env["PIKA_TEST_NSEC"]
            ?? dotenv["PIKA_UI_E2E_NSEC"]
            ?? dotenv["PIKA_TEST_NSEC"]
            ?? ""
        let relays = env["PIKA_UI_E2E_RELAYS"] ?? dotenv["PIKA_UI_E2E_RELAYS"] ?? ""
        let kpRelays = env["PIKA_UI_E2E_KP_RELAYS"] ?? dotenv["PIKA_UI_E2E_KP_RELAYS"] ?? ""
        let agentApiUrl = env["PIKA_AGENT_API_URL"]
            ?? env["PIKA_SERVER_URL"]
            ?? dotenv["PIKA_AGENT_API_URL"]
            ?? dotenv["PIKA_SERVER_URL"]
            ?? ""

        if testNsec.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_NSEC (or PIKA_TEST_NSEC)"); return }
        if relays.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_RELAYS"); return }
        if kpRelays.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_KP_RELAYS"); return }

        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_RELAY_URLS"] = relays
        app.launchEnvironment["PIKA_KEY_PACKAGE_RELAY_URLS"] = kpRelays
        if !agentApiUrl.isEmpty {
            app.launchEnvironment["PIKA_AGENT_API_URL"] = agentApiUrl
        }
        app.launch()

        loginWithTestNsecIfNeeded(app, testNsec: testNsec)
        openAgentProvisioningFromChatList(app, timeout: 30)

        let composerDeadline = Date().addingTimeInterval(120)
        let msgField = app.textViews.matching(identifier: "chat_message_input").firstMatch
        let msgFieldFallback = app.textFields.matching(identifier: "chat_message_input").firstMatch

        while Date() < composerDeadline {
            if msgField.exists || msgFieldFallback.exists { break }

            if let errorMsg = dismissPikaToastIfPresent(app, timeout: 0.5) {
                XCTFail("OpenClaw E2E failed while provisioning agent: \(errorMsg)")
                return
            }

            let retry = app.buttons["Try Again"].firstMatch
            if retry.exists {
                let visibleTexts = app.staticTexts.allElementsBoundByIndex
                    .map(\.label)
                    .filter { !$0.isEmpty }
                    .joined(separator: " | ")
                XCTFail("OpenClaw provisioning entered error state: \(visibleTexts)")
                return
            }

            Thread.sleep(forTimeInterval: 0.5)
        }

        let composer = msgField.exists ? msgField : msgFieldFallback
        XCTAssertTrue(composer.waitForExistence(timeout: 5), "Timed out waiting for OpenClaw chat to open")

        let nonce = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
        let expected = "IOS_OPENCLAW_E2E_OK token=\(nonce)"
        let probe = "Reply with exactly: \(expected)"

        composer.tap()
        composer.typeText(probe)

        let send = app.buttons.matching(identifier: "chat_send").firstMatch
        XCTAssertTrue(send.waitForExistence(timeout: 10))
        send.tap()

        let replyDeadline = Date().addingTimeInterval(90)
        while Date() < replyDeadline {
            if app.staticTexts[expected].exists {
                return
            }

            if let errorMsg = dismissPikaToastIfPresent(app, timeout: 0.5) {
                XCTFail("OpenClaw E2E failed after sending probe: \(errorMsg)")
                return
            }

            Thread.sleep(forTimeInterval: 0.5)
        }

        XCTFail("Timed out waiting for OpenClaw reply: \(expected)")
    }

    func testE2E_hypernoteDetailsAndCodeBlock() throws {
        let env = ProcessInfo.processInfo.environment
        let botNpub = env["PIKA_UI_E2E_BOT_NPUB"] ?? ""
        let testNsec = env["PIKA_UI_E2E_NSEC"] ?? env["PIKA_TEST_NSEC"] ?? ""
        let relays = env["PIKA_UI_E2E_RELAYS"] ?? ""
        let kpRelays = env["PIKA_UI_E2E_KP_RELAYS"] ?? ""

        if botNpub.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_BOT_NPUB"); return }
        if testNsec.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_NSEC (or PIKA_TEST_NSEC)"); return }
        if relays.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_RELAYS"); return }
        if kpRelays.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_KP_RELAYS"); return }

        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_RELAY_URLS"] = relays
        app.launchEnvironment["PIKA_KEY_PACKAGE_RELAY_URLS"] = kpRelays
        app.launch()

        // Login.
        let createAccount = app.buttons.matching(identifier: "login_create_account").firstMatch
        if createAccount.waitForExistence(timeout: 5) {
            if !testNsec.isEmpty {
                let loginNsec = app.secureTextFields.matching(identifier: "login_nsec_input").firstMatch
                let loginSubmit = app.buttons.matching(identifier: "login_submit").firstMatch
                XCTAssertTrue(loginNsec.waitForExistence(timeout: 5))
                XCTAssertTrue(loginSubmit.waitForExistence(timeout: 5))
                loginNsec.tap()
                loginNsec.typeText(testNsec)
                loginSubmit.tap()
            } else {
                createAccount.tap()
            }
        }

        // Chat list.
        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 10))

        // Start chat with bot.
        openNewChatFromChatList(app, timeout: 10)

        let manualEntry = app.buttons.matching(identifier: "newchat_manual_entry").firstMatch
        XCTAssertTrue(manualEntry.waitForExistence(timeout: 10))
        manualEntry.tap()

        let peer = app.descendants(matching: .any).matching(identifier: "newchat_peer_npub").firstMatch
        XCTAssertTrue(peer.waitForExistence(timeout: 10))
        peer.tap()
        peer.typeText(botNpub)

        let start = app.buttons.matching(identifier: "newchat_start").firstMatch
        XCTAssertTrue(start.waitForExistence(timeout: 10))
        start.tap()

        // Wait for chat creation.
        let composerDeadline = Date().addingTimeInterval(10)
        var chatCreationFailed = false
        let msgField = app.textViews.matching(identifier: "chat_message_input").firstMatch
        let msgFieldFallback = app.textFields.matching(identifier: "chat_message_input").firstMatch
        while Date() < composerDeadline {
            if msgField.exists || msgFieldFallback.exists { break }
            if let errorMsg = dismissPikaToastIfPresent(app, timeout: 0.5) {
                if errorMsg.lowercased().contains("failed") ||
                    errorMsg.lowercased().contains("invalid peer key package") ||
                    errorMsg.lowercased().contains("could not find peer key package")
                {
                    XCTFail("E2E failed during chat creation: \(errorMsg)")
                    chatCreationFailed = true
                    break
                }
            }
            Thread.sleep(forTimeInterval: 0.5)
        }
        if chatCreationFailed { return }

        let composer = msgField.exists ? msgField : msgFieldFallback
        XCTAssertTrue(composer.waitForExistence(timeout: 10))

        // ── Details probe ──
        composer.tap()
        composer.typeText("hn:details")
        let send = app.buttons.matching(identifier: "chat_send").firstMatch
        XCTAssertTrue(send.waitForExistence(timeout: 10))
        send.tap()

        // Wait for the details component to appear.
        let details = app.descendants(matching: .any).matching(identifier: "hypernote_details").firstMatch
        XCTAssertTrue(details.waitForExistence(timeout: 10), "Details component did not appear")

        let summary = app.descendants(matching: .any).matching(identifier: "hypernote_details_summary").firstMatch
        XCTAssertTrue(summary.exists, "Summary should be visible")

        let body = app.descendants(matching: .any).matching(identifier: "hypernote_details_body").firstMatch
        XCTAssertFalse(body.exists, "Body should be collapsed initially")

        // Tap to expand.
        summary.tap()
        XCTAssertTrue(body.waitForExistence(timeout: 5), "Body should appear after expanding")

        // Tap to collapse.
        summary.tap()
        let collapseDeadline = Date().addingTimeInterval(5)
        while Date() < collapseDeadline, body.exists {
            Thread.sleep(forTimeInterval: 0.1)
        }
        XCTAssertFalse(body.exists, "Body should be hidden after collapsing")

        // ── Code block probe ──
        composer.tap()
        composer.typeText("hn:codeblock")
        XCTAssertTrue(send.waitForExistence(timeout: 10))
        send.tap()

        let codeblock = app.descendants(matching: .any).matching(identifier: "hypernote_codeblock").firstMatch
        XCTAssertTrue(codeblock.waitForExistence(timeout: 10), "Code block did not appear")

        let lang = app.descendants(matching: .any).matching(identifier: "hypernote_codeblock_lang").firstMatch
        XCTAssertTrue(lang.exists, "Language label should be visible")
        XCTAssertEqual(lang.label, "rust", "Language should be 'rust'")

        let copyBtn = app.descendants(matching: .any).matching(identifier: "hypernote_codeblock_copy").firstMatch
        XCTAssertTrue(copyBtn.exists, "Copy button should be visible")

        // Tap copy and check for "Copied" indicator.
        copyBtn.tap()
        let copied = app.descendants(matching: .any).matching(identifier: "hypernote_codeblock_copied").firstMatch
        XCTAssertTrue(copied.waitForExistence(timeout: 5), "Copied indicator should appear after tapping copy")
    }

    func testE2E_multiImageGrid() throws {
        let env = ProcessInfo.processInfo.environment
        let dotenv = loadDotenv()
        let botNpub = env["PIKA_UI_E2E_BOT_NPUB"] ?? dotenv["PIKA_UI_E2E_BOT_NPUB"] ?? ""
        let testNsec = env["PIKA_UI_E2E_NSEC"]
            ?? env["PIKA_TEST_NSEC"]
            ?? dotenv["PIKA_UI_E2E_NSEC"]
            ?? dotenv["PIKA_TEST_NSEC"]
            ?? ""
        let relays = env["PIKA_UI_E2E_RELAYS"] ?? dotenv["PIKA_UI_E2E_RELAYS"] ?? ""
        let kpRelays = env["PIKA_UI_E2E_KP_RELAYS"] ?? dotenv["PIKA_UI_E2E_KP_RELAYS"] ?? ""

        if botNpub.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_BOT_NPUB"); return }
        if testNsec.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_NSEC (or PIKA_TEST_NSEC)"); return }
        if relays.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_RELAYS"); return }
        if kpRelays.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_KP_RELAYS"); return }

        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_RELAY_URLS"] = relays
        app.launchEnvironment["PIKA_KEY_PACKAGE_RELAY_URLS"] = kpRelays
        app.launch()

        // Login.
        let createAccount = app.buttons.matching(identifier: "login_create_account").firstMatch
        if createAccount.waitForExistence(timeout: 5) {
            if !testNsec.isEmpty {
                let loginNsec = app.secureTextFields.matching(identifier: "login_nsec_input").firstMatch
                let loginSubmit = app.buttons.matching(identifier: "login_submit").firstMatch
                XCTAssertTrue(loginNsec.waitForExistence(timeout: 5))
                XCTAssertTrue(loginSubmit.waitForExistence(timeout: 5))
                loginNsec.tap()
                loginNsec.typeText(testNsec)
                loginSubmit.tap()
            } else {
                createAccount.tap()
            }
        }

        // Chat list.
        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 10))

        // Start chat with bot.
        openNewChatFromChatList(app, timeout: 10)

        let manualEntry = app.buttons.matching(identifier: "newchat_manual_entry").firstMatch
        XCTAssertTrue(manualEntry.waitForExistence(timeout: 10))
        manualEntry.tap()

        let peer = app.descendants(matching: .any).matching(identifier: "newchat_peer_npub").firstMatch
        XCTAssertTrue(peer.waitForExistence(timeout: 10))
        peer.tap()
        peer.typeText(botNpub)

        let start = app.buttons.matching(identifier: "newchat_start").firstMatch
        XCTAssertTrue(start.waitForExistence(timeout: 10))
        start.tap()

        // Wait for chat creation.
        let composerDeadline = Date().addingTimeInterval(10)
        var chatCreationFailed = false
        let msgField = app.textViews.matching(identifier: "chat_message_input").firstMatch
        let msgFieldFallback = app.textFields.matching(identifier: "chat_message_input").firstMatch
        while Date() < composerDeadline {
            if msgField.exists || msgFieldFallback.exists { break }
            if let errorMsg = dismissPikaToastIfPresent(app, timeout: 0.5) {
                if errorMsg.lowercased().contains("failed") ||
                    errorMsg.lowercased().contains("invalid peer key package") ||
                    errorMsg.lowercased().contains("could not find peer key package")
                {
                    XCTFail("E2E failed during chat creation: \(errorMsg)")
                    chatCreationFailed = true
                    break
                }
            }
            Thread.sleep(forTimeInterval: 0.5)
        }
        if chatCreationFailed { return }

        let composer = msgField.exists ? msgField : msgFieldFallback
        XCTAssertTrue(composer.waitForExistence(timeout: 10))

        // Ask the bot to send 3 images.
        composer.tap()
        composer.typeText("media_batch:3")

        let send = app.buttons.matching(identifier: "chat_send").firstMatch
        XCTAssertTrue(send.waitForExistence(timeout: 10))
        send.tap()

        // Wait for the media grid to appear (bot encrypts, uploads, and sends images).
        let mediaGrid = app.descendants(matching: .any).matching(identifier: "chat_media_grid").firstMatch
        XCTAssertTrue(
            mediaGrid.waitForExistence(timeout: 30),
            "Media grid should appear after bot sends multi-image message"
        )
    }

    /// Platform-hosted shell smoke only: this covers iOS text-entry/scanner-style
    /// deep-link handling and chat routing, not the canonical Rust-owned deep-link
    /// normalization/chat-state contract.
    func testChatDeepLink_opensChat() throws {
        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launch()
        loginOfflineTestIdentity(app)

        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 15))
        let myNpub = localUiTestNpub

        // Navigate to New Chat and paste the full deep-link URL (as if scanned
        // from the QR code). normalizePeerKey strips the pika://chat/ prefix.
        openNewChatFromChatList(app, timeout: 10)

        let manualEntry = app.buttons.matching(identifier: "newchat_manual_entry").firstMatch
        XCTAssertTrue(manualEntry.waitForExistence(timeout: 10))
        manualEntry.tap()

        let peer = app.descendants(matching: .any).matching(identifier: "newchat_peer_npub").firstMatch
        XCTAssertTrue(peer.waitForExistence(timeout: 10))
        peer.tap()
        peer.typeText("pika://chat/\(myNpub)")

        let start = app.buttons.matching(identifier: "newchat_start").firstMatch
        XCTAssertTrue(start.waitForExistence(timeout: 5))
        XCTAssertTrue(start.isEnabled, "Start button should be enabled after entering deep-link URL")
        start.tap()

        // Note-to-self is synchronous offline; we should land in a chat.
        let composer = waitForChatComposer(app, timeout: 30)
        XCTAssertTrue(composer.exists, "Deep link URL did not create a chat")
    }

    func testInterop_nostrConnectLaunchesPrimal() throws {
        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launchEnvironment["PIKA_ENABLE_EXTERNAL_SIGNER"] = "1"
        app.launchEnvironment["PIKA_UI_TEST_CAPTURE_OPEN_URL"] = "1"
        app.launch()

        waitForLoginScreen(app)
        let nostrConnectButton = app.buttons.matching(identifier: "login_nostr_connect_submit").firstMatch
        expandAdvancedLoginOptionsIfNeeded(app)
        revealLoginAdvancedControlIfNeeded(app, control: nostrConnectButton)
        XCTAssertTrue(
            nostrConnectButton.waitForExistence(timeout: 10),
            "Missing Nostr Connect login button\n\(app.debugDescription)"
        )
        nostrConnectButton.tap()
        dismissSystemOpenAppAlertIfPresent()
        // Let async bridge callbacks run; harness verifies URL emission via marker file.
        Thread.sleep(forTimeInterval: 2.0)
    }
}
