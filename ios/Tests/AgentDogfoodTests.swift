import XCTest
@testable import Pika

final class AgentDogfoodTests: XCTestCase {
    func testDogfoodEligibilityRequiresLocalNsecLogin() {
        let npub = "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq6jv4kk"

        XCTAssertTrue(
            isDogfoodAgentEligible(
                npub: npub,
                auth: .loggedIn(npub: npub, pubkey: String(repeating: "a", count: 64), mode: .localNsec)
            )
        )

        XCTAssertFalse(
            isDogfoodAgentEligible(
                npub: npub,
                auth: .loggedIn(
                    npub: npub,
                    pubkey: String(repeating: "a", count: 64),
                    mode: .bunkerSigner(bunkerUri: "bunker://example")
                )
            )
        )

        XCTAssertFalse(
            isDogfoodAgentEligible(
                npub: npub,
                auth: .loggedIn(
                    npub: npub,
                    pubkey: String(repeating: "a", count: 64),
                    mode: .externalSigner(pubkey: String(repeating: "a", count: 64), signerPackage: "pkg", currentUser: "user")
                )
            )
        )

        XCTAssertFalse(isDogfoodAgentEligible(npub: nil, auth: .loggedOut))
        XCTAssertFalse(isDogfoodAgentEligible(npub: "   ", auth: .loggedOut))
    }

    func testDogfoodButtonStateReflectsBusyFlag() {
        XCTAssertEqual(
            makeDogfoodAgentButtonState(isBusy: false),
            DogfoodAgentButtonState(title: "Start Personal Agent", isBusy: false)
        )
        XCTAssertEqual(
            makeDogfoodAgentButtonState(isBusy: true),
            DogfoodAgentButtonState(title: "Starting Personal Agent...", isBusy: true)
        )
    }
}
