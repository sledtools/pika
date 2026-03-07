import XCTest
@testable import Pika

final class AgentTests: XCTestCase {
    func testAgentEligibilityRequiresLocalNsecLogin() {
        let npub = "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq6jv4kk"

        XCTAssertTrue(
            isAgentEligible(
                npub: npub,
                auth: .loggedIn(npub: npub, pubkey: String(repeating: "a", count: 64), mode: .localNsec)
            )
        )
        XCTAssertFalse(
            isAgentEligible(
                npub: npub,
                auth: .loggedIn(
                    npub: "npub1differentaccountqqqqqqqqqqqqqqqqqqqqqqqqqqqqj6y0r7",
                    pubkey: String(repeating: "a", count: 64),
                    mode: .localNsec
                )
            )
        )

        XCTAssertFalse(
            isAgentEligible(
                npub: npub,
                auth: .loggedIn(
                    npub: npub,
                    pubkey: String(repeating: "a", count: 64),
                    mode: .bunkerSigner(bunkerUri: "bunker://example")
                )
            )
        )

        XCTAssertFalse(
            isAgentEligible(
                npub: npub,
                auth: .loggedIn(
                    npub: npub,
                    pubkey: String(repeating: "a", count: 64),
                    mode: .externalSigner(pubkey: String(repeating: "a", count: 64), signerPackage: "pkg", currentUser: "user")
                )
            )
        )

        XCTAssertFalse(isAgentEligible(npub: nil, auth: .loggedOut))
        XCTAssertFalse(isAgentEligible(npub: "   ", auth: .loggedOut))
    }

    func testAgentButtonStateReflectsBusyFlag() {
        XCTAssertEqual(
            makeAgentButtonState(isBusy: false),
            AgentButtonState(title: "Start Agent", isBusy: false)
        )
        XCTAssertEqual(
            makeAgentButtonState(isBusy: true),
            AgentButtonState(title: "Starting Agent...", isBusy: true)
        )
    }
}
