import XCTest
@testable import Pika

final class KeychainNsecStoreTests: XCTestCase {

    // MARK: - Production mode: file fallback must be denied

    /// Simulates the production (device) code path: `fileFallbackAllowed: false`.
    /// When the keychain is unavailable (-34018, which is the case on simulator builds
    /// without entitlements), the store must trigger the crash handler rather than
    /// silently writing the nsec to a plaintext file.
    func testProductionModeDeniesFileFallback_onSet() {
        let store = KeychainNsecStore(fileFallbackAllowed: false)

        var deniedMessage: String?
        store.onFileFallbackDenied = { msg in
            deniedMessage = msg
        }

        // On the simulator the keychain returns -34018, which triggers switchToFileFallback.
        // With fileFallbackAllowed=false this must call onFileFallbackDenied (or fatalError).
        store.setNsec("nsec1testproductioncrash")

        XCTAssertNotNil(deniedMessage,
            "Production mode must trigger crash handler when keychain is unavailable, not silently fall back to file")
        XCTAssertTrue(deniedMessage?.contains("must not happen in a production build") == true)
    }

    func testProductionModeDeniesFileFallback_onGet() {
        let store = KeychainNsecStore(fileFallbackAllowed: false)

        var deniedMessage: String?
        store.onFileFallbackDenied = { msg in
            deniedMessage = msg
        }

        // getNsec hits the keychain first (-34018 on sim) → switchToFileFallback → denied.
        let result = store.getNsec()

        XCTAssertNotNil(deniedMessage,
            "Production mode must trigger crash handler on get when keychain is unavailable")
        XCTAssertNil(result, "getNsec must not return a value when fallback is denied")
    }

    // MARK: - Simulator mode: file fallback works

    func testSimulatorModeFallsBackToFile() {
        let store = KeychainNsecStore(fileFallbackAllowed: true)

        store.setNsec("nsec1simfallbacktest")

        let store2 = KeychainNsecStore(fileFallbackAllowed: true)
        let restored = store2.getNsec()

        // On simulator with -34018 both stores fall back to file.
        // The nsec should round-trip.
        XCTAssertEqual(restored, "nsec1simfallbacktest",
            "Simulator file fallback must persist and restore the nsec")

        // Cleanup
        store2.clearNsec()
        XCTAssertNil(KeychainNsecStore(fileFallbackAllowed: true).getNsec())
    }

    // MARK: - Default init resolves correctly

    func testDefaultInitAllowsFallbackOnSimulator() {
        // We're running on the simulator, so the default should allow fallback.
        let store = KeychainNsecStore()
        XCTAssertTrue(store.fileFallbackAllowed,
            "Default init on simulator must allow file fallback")
    }
}
