import XCTest
@testable import Pika

final class VideoAdaptationControllerTests: XCTestCase {
    func testDegradesAcrossPoorSignalsWithoutSkippingTiers() {
        var controller = VideoAdaptationController(
            degradeStreakThreshold: 2,
            upgradeStreakThreshold: 6,
            holdSeconds: 0,
            initialTier: .high
        )

        XCTAssertNil(controller.observe(
            sample: AdaptationSample(rxDropped: 10, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 0
        ))
        XCTAssertNil(controller.observe(
            sample: AdaptationSample(rxDropped: 30, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 1
        ))
        XCTAssertEqual(controller.observe(
            sample: AdaptationSample(rxDropped: 50, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 2
        ), .medium)
        XCTAssertNil(controller.observe(
            sample: AdaptationSample(rxDropped: 70, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 3
        ))
        XCTAssertEqual(controller.observe(
            sample: AdaptationSample(rxDropped: 90, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 4
        ), .low)
    }

    func testHoldWindowPreventsThrashing() {
        var controller = VideoAdaptationController(
            degradeStreakThreshold: 2,
            upgradeStreakThreshold: 2,
            holdSeconds: 10,
            initialTier: .high
        )

        XCTAssertNil(controller.observe(
            sample: AdaptationSample(rxDropped: 0, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 0
        ))
        XCTAssertNil(controller.observe(
            sample: AdaptationSample(rxDropped: 20, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 1
        ))
        XCTAssertEqual(controller.observe(
            sample: AdaptationSample(rxDropped: 40, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 2
        ), .medium)

        // Good signals arrive but should not immediately bounce back within hold window.
        XCTAssertNil(controller.observe(
            sample: AdaptationSample(rxDropped: 41, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 3
        ))
        XCTAssertNil(controller.observe(
            sample: AdaptationSample(rxDropped: 42, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 4
        ))
        XCTAssertNil(controller.observe(
            sample: AdaptationSample(rxDropped: 43, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 11
        ))
        XCTAssertEqual(controller.observe(
            sample: AdaptationSample(rxDropped: 44, videoRxDecryptFail: 0),
            staleVideoSignal: false,
            now: 12
        ), .high)
    }
}
