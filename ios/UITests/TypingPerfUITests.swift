import XCTest
import QuartzCore
import os.signpost

final class TypingPerfUITests: XCTestCase {
    private let typingSignpostLog = OSLog(subsystem: "org.pikachat.pika.uitests", category: .pointsOfInterest)

    func testTypingLatencySummary() throws {
        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launchEnvironment["PIKA_UI_TYPING_PERF_APP_SIGNPOSTS"] = ProcessInfo.processInfo.environment["PIKA_UI_TYPING_PERF_APP_SIGNPOSTS"] ?? "1"
        app.launch()

        ensureLoggedIn(app)
        let myNpub = openProfileAndReadNpub(app)
        closeProfile(app)
        openNewChatFromChatList(app)
        openNoteToSelfChat(app, npub: myNpub)

        let composer = waitForChatComposer(app, timeout: 10)
        XCTAssertTrue(composer.waitForExistence(timeout: 10), "Composer missing")
        composer.tap()

        let warmupChars = envInt("PIKA_UI_TYPING_PERF_WARMUP", defaultValue: 10)
        let measuredChars = envInt("PIKA_UI_TYPING_PERF_CHARS", defaultValue: 120)
        let startDelayMs = envInt("PIKA_UI_TYPING_PERF_START_DELAY_MS", defaultValue: 0)
        let sampleText = makeSampleText(length: warmupChars + measuredChars)
        let keypressSignposts = envBool("PIKA_UI_TYPING_PERF_SIGNPOSTS", defaultValue: true)

        if startDelayMs > 0 {
            Thread.sleep(forTimeInterval: TimeInterval(startDelayMs) / 1_000.0)
        }

        var keypressTimesMs: [Double] = []
        keypressTimesMs.reserveCapacity(measuredChars)

        for (idx, ch) in sampleText.enumerated() {
            let isMeasured = idx >= warmupChars
            let measuredIdx = idx - warmupChars
            var signpostID: OSSignpostID?
            if keypressSignposts, isMeasured {
                let id = OSSignpostID(log: typingSignpostLog)
                os_signpost(.begin, log: typingSignpostLog, name: "composer_keypress", signpostID: id, "index=%{public}d", measuredIdx)
                signpostID = id
            }

            let start = CACurrentMediaTime()
            composer.typeText(String(ch))
            let elapsedMs = (CACurrentMediaTime() - start) * 1_000.0
            if isMeasured {
                keypressTimesMs.append(elapsedMs)
            }
            if let signpostID {
                os_signpost(.end, log: typingSignpostLog, name: "composer_keypress", signpostID: signpostID, "index=%{public}d", measuredIdx)
            }
        }

        let avg = keypressTimesMs.reduce(0.0, +) / Double(max(1, keypressTimesMs.count))
        let best = keypressTimesMs.min() ?? 0
        let worst = keypressTimesMs.max() ?? 0
        print(
            String(
                format: "PIKA_TYPING_PERF avg_ms=%.3f best_ms=%.3f worst_ms=%.3f samples=%d warmup=%d",
                avg,
                best,
                worst,
                keypressTimesMs.count,
                warmupChars
            )
        )
    }

    func testTypingBulkSummary() throws {
        let app = XCUIApplication()
        app.launchEnvironment["PIKA_UI_TEST_RESET"] = "1"
        app.launchEnvironment["PIKA_DISABLE_NETWORK"] = "1"
        app.launchEnvironment["PIKA_UI_TYPING_PERF_APP_SIGNPOSTS"] = ProcessInfo.processInfo.environment["PIKA_UI_TYPING_PERF_APP_SIGNPOSTS"] ?? "1"
        app.launch()

        ensureLoggedIn(app)
        let myNpub = openProfileAndReadNpub(app)
        closeProfile(app)
        openNewChatFromChatList(app)
        openNoteToSelfChat(app, npub: myNpub)

        let composer = waitForChatComposer(app, timeout: 10)
        XCTAssertTrue(composer.waitForExistence(timeout: 10), "Composer missing")
        composer.tap()

        let chars = envInt("PIKA_UI_TYPING_PERF_CHARS", defaultValue: 120)
        let text = makeSampleText(length: chars)

        let signpostID = OSSignpostID(log: typingSignpostLog)
        os_signpost(.begin, log: typingSignpostLog, name: "composer_bulk_typeText", signpostID: signpostID, "chars=%{public}d", chars)
        let start = CACurrentMediaTime()
        composer.typeText(text)
        let totalMs = (CACurrentMediaTime() - start) * 1_000.0
        os_signpost(.end, log: typingSignpostLog, name: "composer_bulk_typeText", signpostID: signpostID, "chars=%{public}d", chars)

        let perCharMs = totalMs / Double(max(1, chars))
        print(String(format: "PIKA_TYPING_BULK total_ms=%.3f per_char_ms=%.3f chars=%d", totalMs, perCharMs, chars))
    }

    private func envInt(_ key: String, defaultValue: Int) -> Int {
        let raw = ProcessInfo.processInfo.environment[key] ?? ""
        guard let parsed = Int(raw), parsed > 0 else { return defaultValue }
        return parsed
    }

    private func envBool(_ key: String, defaultValue: Bool) -> Bool {
        let raw = (ProcessInfo.processInfo.environment[key] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if raw.isEmpty { return defaultValue }
        if raw == "1" || raw == "true" || raw == "yes" || raw == "on" { return true }
        if raw == "0" || raw == "false" || raw == "no" || raw == "off" { return false }
        return defaultValue
    }

    private func makeSampleText(length: Int) -> String {
        let seed = "the_quick_brown_fox_jumps_over_the_lazy_dog_0123456789 "
        guard length > 0 else { return "" }
        var out = ""
        out.reserveCapacity(length)
        while out.count < length {
            out += seed
        }
        return String(out.prefix(length))
    }

    private func ensureLoggedIn(_ app: XCUIApplication) {
        let createAccount = app.buttons.matching(identifier: "login_create_account").firstMatch
        if createAccount.waitForExistence(timeout: 2) {
            createAccount.tap()
        }
        let chatsNavBar = app.navigationBars["Chats"]
        XCTAssertTrue(chatsNavBar.waitForExistence(timeout: 15), "Chats screen not visible")
    }

    private func openProfileAndReadNpub(_ app: XCUIApplication) -> String {
        let myNpubBtn = app.buttons.matching(identifier: "chatlist_my_npub").firstMatch
        XCTAssertTrue(myNpubBtn.waitForExistence(timeout: 5), "Profile button missing")
        myNpubBtn.tap()

        let profileNav = app.navigationBars["Profile"]
        XCTAssertTrue(profileNav.waitForExistence(timeout: 5), "Profile sheet missing")

        let npubValue = app.staticTexts.matching(identifier: "chatlist_my_npub_value").firstMatch
        XCTAssertTrue(npubValue.waitForExistence(timeout: 5), "npub value missing")
        let npub = npubValue.label
        XCTAssertTrue(npub.hasPrefix("npub1"), "Expected npub1..., got: \(npub)")
        return npub
    }

    private func closeProfile(_ app: XCUIApplication) {
        let close = app.buttons.matching(identifier: "chatlist_my_npub_close").firstMatch
        if close.exists {
            close.tap()
        } else {
            app.navigationBars["Profile"].buttons.element(boundBy: 0).tap()
        }
    }

    private func openNewChatFromChatList(_ app: XCUIApplication) {
        let newChat = app.buttons.matching(identifier: "chatlist_new_chat").firstMatch
        XCTAssertTrue(newChat.waitForExistence(timeout: 5), "New chat button missing")
        newChat.tap()

        let nav = app.navigationBars["New Chat"]
        if nav.waitForExistence(timeout: 2) {
            return
        }

        let menuItem = app.buttons["New Chat"].firstMatch
        XCTAssertTrue(menuItem.waitForExistence(timeout: 5), "New Chat menu item missing")
        menuItem.tap()
        XCTAssertTrue(nav.waitForExistence(timeout: 10), "New Chat screen not visible")
    }

    private func openNoteToSelfChat(_ app: XCUIApplication, npub: String) {
        let peer = app.descendants(matching: .any).matching(identifier: "newchat_peer_npub").firstMatch
        XCTAssertTrue(peer.waitForExistence(timeout: 10), "Peer npub field missing")
        peer.tap()
        peer.typeText(npub)

        let start = app.buttons.matching(identifier: "newchat_start").firstMatch
        XCTAssertTrue(start.waitForExistence(timeout: 5), "Start chat button missing")
        start.tap()
    }

    private func waitForChatComposer(_ app: XCUIApplication, timeout: TimeInterval) -> XCUIElement {
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
}
