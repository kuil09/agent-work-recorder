import XCTest
import CoreVideo
@testable import RecCapture

final class CaptureHealthTests: XCTestCase {
    func pixel(_ value: UInt8) -> CVPixelBuffer {
        let pixel = makePixelBuffer(width: 64, height: 36)!
        CVPixelBufferLockBaseAddress(pixel, [])
        memset(CVPixelBufferGetBaseAddress(pixel)!, Int32(value), CVPixelBufferGetBytesPerRow(pixel) * 36)
        CVPixelBufferUnlockBaseAddress(pixel, [])
        return pixel
    }
    func testDarkAndStaticPixelsDoNotDeclareTargetLost() {
        let h = NativeCaptureHealth(); h.targetAvailable = true
        h.sourceFrame(pixel(0), now: 10)
        XCTAssertNotNil(h.visualWarning); XCTAssertEqual(h.targetAvailable, true)
        h.sourceFrame(pixel(80), now: 12)
        XCTAssertNil(h.visualWarning)
        h.sourceFrame(pixel(80), now: 18)
        XCTAssertTrue(h.visualWarning!.contains("unchanged")); XCTAssertEqual(h.targetAvailable, true)
    }
    func testIdleSampleIsSeparateFromSourceFrameAndEncodedCopies() {
        let h = NativeCaptureHealth(); h.sourceFrame(pixel(80), now: 10); h.idle(now: 14)
        let s = h.snapshot(written: 120, validated: true, origin: 10, now: 15)
        XCTAssertEqual(s["frames_received"] as? Int, 1)
        XCTAssertEqual(s["frames_written"] as? Int, 120)
        XCTAssertEqual(s["last_frame_age_ms"] as? Int, 5000)
        XCTAssertEqual(s["last_sample_age_ms"] as? Int, 1000)
    }
    func testNoFirstFrameIsPending() {
        let s = NativeCaptureHealth().snapshot(written: 0, validated: false, origin: nil)
        XCTAssertEqual(s["first_frame"] as? String, "pending")
        XCTAssertTrue(s["last_frame_ms"] is NSNull)
    }
}
