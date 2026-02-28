import XCTest
@testable import Pika

final class VideoDecoderRendererTests: XCTestCase {
    func testDecodeFailureEntersKeyframeRecoveryMode() {
        let decoder = VideoDecoderRenderer()

        XCTAssertTrue(decoder.debugCanAttemptDecode(naluType: 1))
        decoder.debugInjectDecodeFailure(-12909)

        let counters = decoder.currentCounters()
        XCTAssertEqual(counters.decodeFailures, 1)
        XCTAssertEqual(counters.decodeFailureResets, 1)
        XCTAssertFalse(decoder.debugCanAttemptDecode(naluType: 1))
        XCTAssertTrue(decoder.debugCanAttemptDecode(naluType: 5))
    }
}
