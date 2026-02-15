import XCTest

final class PikaUITests: XCTestCase {
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

    func testCreateAccount_noteToSelf_sendMessage_and_logout() throws {
        let app = XCUIApplication()
        // Keep this test deterministic/offline.
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launch()

        // If we land on Login, create an account; otherwise we may have restored a prior session.
        let createAccount = app.buttons.matching(identifier: "login_create_account").firstMatch
        if createAccount.waitForExistence(timeout: 2) {
            createAccount.tap()
            // No blocking toasts to dismiss; navigation happens automatically.
        }

        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 15))

        // Fetch our npub from the profile sheet (avoid clipboard access from UI tests).
        let myNpubBtn = app.buttons.matching(identifier: "chatlist_my_npub").firstMatch
        XCTAssertTrue(myNpubBtn.waitForExistence(timeout: 5))
        myNpubBtn.tap()

        let myNpubNavBar = app.navigationBars["Profile"]
        XCTAssertTrue(myNpubNavBar.waitForExistence(timeout: 5))

        let npubValue = app.staticTexts.matching(identifier: "chatlist_my_npub_value").firstMatch
        XCTAssertTrue(npubValue.waitForExistence(timeout: 5))
        let myNpub = npubValue.label
        XCTAssertTrue(myNpub.hasPrefix("npub1"), "Expected npub1..., got: \(myNpub)")

        // Close the sheet.
        let close = app.buttons.matching(identifier: "chatlist_my_npub_close").firstMatch
        if close.exists { close.tap() }
        else { myNpubNavBar.buttons.element(boundBy: 0).tap() }

        // New chat.
        let newChat = app.buttons.matching(identifier: "chatlist_new_chat").firstMatch
        XCTAssertTrue(newChat.waitForExistence(timeout: 5))
        newChat.tap()

        XCTAssertTrue(app.navigationBars["New Chat"].waitForExistence(timeout: 10))

        let peer = app.descendants(matching: .any).matching(identifier: "newchat_peer_npub").firstMatch
        XCTAssertTrue(peer.waitForExistence(timeout: 10))
        peer.tap()
        peer.typeText(myNpub)

        let start = app.buttons.matching(identifier: "newchat_start").firstMatch
        XCTAssertTrue(start.waitForExistence(timeout: 5))
        start.tap()
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

        let logout = app.buttons.matching(identifier: "chatlist_logout").firstMatch
        XCTAssertTrue(logout.waitForExistence(timeout: 5))
        logout.tap()

        XCTAssertTrue(app.staticTexts["Pika"].waitForExistence(timeout: 10))
    }

    func testSessionPersistsAcrossRelaunch() throws {
        let app = XCUIApplication()

        // --- First launch: clean slate, create account + chat ---
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launch()

        let createAccount = app.buttons.matching(identifier: "login_create_account").firstMatch
        XCTAssertTrue(createAccount.waitForExistence(timeout: 5), "Login screen not shown on first launch")
        createAccount.tap()

        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 15), "Chat list not shown after account creation")

        // Get our npub for note-to-self.
        let myNpubBtn = app.buttons.matching(identifier: "chatlist_my_npub").firstMatch
        XCTAssertTrue(myNpubBtn.waitForExistence(timeout: 5))
        myNpubBtn.tap()

        let npubValue = app.staticTexts.matching(identifier: "chatlist_my_npub_value").firstMatch
        XCTAssertTrue(npubValue.waitForExistence(timeout: 5))
        let myNpub = npubValue.label
        XCTAssertTrue(myNpub.hasPrefix("npub1"), "Expected npub1..., got: \(myNpub)")

        // Close sheet.
        let close = app.buttons.matching(identifier: "chatlist_my_npub_close").firstMatch
        if close.exists { close.tap() }
        else { app.navigationBars["Profile"].buttons.element(boundBy: 0).tap() }

        // Create note-to-self chat.
        let newChat = app.buttons.matching(identifier: "chatlist_new_chat").firstMatch
        XCTAssertTrue(newChat.waitForExistence(timeout: 5))
        newChat.tap()

        XCTAssertTrue(app.navigationBars["New Chat"].waitForExistence(timeout: 10))

        let peer = app.descendants(matching: .any).matching(identifier: "newchat_peer_npub").firstMatch
        XCTAssertTrue(peer.waitForExistence(timeout: 10))
        peer.tap()
        peer.typeText(myNpub)

        let start = app.buttons.matching(identifier: "newchat_start").firstMatch
        XCTAssertTrue(start.waitForExistence(timeout: 5))
        start.tap()

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

    func testE2E_deployedRustBot_pingPong() throws {
        // Opt-in test: run it explicitly via xcodebuild `-only-testing:`. This hits public relays,
        // so it is intentionally not part of the deterministic smoke suite.
        let env = ProcessInfo.processInfo.environment
        let botNpub = env["PIKA_UI_E2E_BOT_NPUB"] ?? ""
        let testNsec = env["PIKA_UI_E2E_NSEC"] ?? env["PIKA_TEST_NSEC"] ?? ""
        let relays = env["PIKA_UI_E2E_RELAYS"] ?? ""
        // Public-relay E2E should be explicit. Defaults hide misconfiguration and cause flaky hangs.
        if botNpub.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_BOT_NPUB"); return }
        if testNsec.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_NSEC (or PIKA_TEST_NSEC)"); return }
        if relays.isEmpty { XCTFail("Missing env var: PIKA_UI_E2E_RELAYS"); return }

        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_RELAY_URLS"] = relays
        app.launch()

        // If we land on Login, prefer logging into a stable allowlisted identity when provided.
        let createAccount = app.buttons.matching(identifier: "login_create_account").firstMatch
        if createAccount.waitForExistence(timeout: 5) {
            if !testNsec.isEmpty {
                let loginNsec = app.textFields.matching(identifier: "login_nsec_input").firstMatch
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
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 30))

        // Start chat with deployed bot.
        let newChat = app.buttons.matching(identifier: "chatlist_new_chat").firstMatch
        XCTAssertTrue(newChat.waitForExistence(timeout: 10))
        newChat.tap()

        XCTAssertTrue(app.navigationBars["New Chat"].waitForExistence(timeout: 15))

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
        let composerDeadline = Date().addingTimeInterval(60)
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
        XCTAssertTrue(composer.waitForExistence(timeout: 30))
        composer.tap()
        composer.typeText(probe)

        let send = app.buttons.matching(identifier: "chat_send").firstMatch
        XCTAssertTrue(send.waitForExistence(timeout: 10))
        send.tap()

        // Expect deterministic ack from the bot.
        XCTAssertTrue(app.staticTexts[expect].waitForExistence(timeout: 180))
    }
}
