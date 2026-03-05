import XCTest
@testable import Pika

final class AgentDogfoodTests: XCTestCase {
    func testWhitelistGateMatchesExpectedUsers() {
        XCTAssertTrue(
            isMicrovmDogfoodWhitelistedNpub(
                "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y"
            )
        )
        XCTAssertTrue(
            isMicrovmDogfoodWhitelistedNpub(
                "npub1rtrxx9eyvag0ap3v73c4dvsqq5d2yxwe5d72qxrfpwe5svr96wuqed4p38"
            )
        )
        XCTAssertTrue(
            isMicrovmDogfoodWhitelistedNpub(
                "npub1p4kg8zxukpym3h20erfa3samj00rm2gt4q5wfuyu3tg0x3jg3gesvncxf8"
            )
        )
        XCTAssertFalse(isMicrovmDogfoodWhitelistedNpub(nil))
        XCTAssertFalse(
            isMicrovmDogfoodWhitelistedNpub(
                "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq6jv4kk"
            )
        )
    }

    func testResolveAgentApiConfigurationPrefersAgentSpecificKeys() {
        let config = resolveAgentApiConfiguration(
            appConfig: [
                "agent_api_url": "https://api.example.com",
                "notification_url": "https://ignored.example.com",
            ],
            env: [:],
            signingNsec: "nsec1test"
        )
        XCTAssertEqual(config?.baseUrl.absoluteString, "https://api.example.com")
        XCTAssertEqual(config?.signingNsec, "nsec1test")
    }

    func testResolveAgentApiConfigurationFallsBackToNotificationUrl() {
        let config = resolveAgentApiConfiguration(
            appConfig: [
                "notification_url": "https://notifs.example.com",
            ],
            env: [:],
            signingNsec: "nsec1test"
        )
        XCTAssertEqual(config?.baseUrl.absoluteString, "https://notifs.example.com")
        XCTAssertEqual(config?.signingNsec, "nsec1test")
    }

    func testResolveAgentApiConfigurationRequiresSigningNsec() {
        let config = resolveAgentApiConfiguration(
            appConfig: [
                "agent_api_url": "https://api.example.com",
            ],
            env: [:],
            signingNsec: nil
        )
        XCTAssertNil(config)
    }
}
