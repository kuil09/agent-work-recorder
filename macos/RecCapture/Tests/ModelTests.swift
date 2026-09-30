import XCTest
@testable import RecCapture

final class ModelTests: XCTestCase {
    func testAudioIsOptInAndMicrophoneFlagIsRejected() throws {
        XCTAssertFalse(try Args.parse(["start", "--output", "a.mp4"]).systemAudio)
        XCTAssertTrue(try Args.parse(["start", "--output", "a.mp4", "--app", "com.apple.Safari", "--system-audio"]).systemAudio)
        XCTAssertThrowsError(try Args.parse(["start", "--microphone", "yes"]))
        XCTAssertThrowsError(try Args.parse(["start", "--window-id", "5", "--app", "Safari"]))
    }
    func testCardsExpireAndVerdictCanBeCleared() {
        let state = OverlayState(), now = Date(timeIntervalSince1970: 100)
        state.apply(json: ["cmd": "hud", "verdict": "Agent verdict: PASS"], now: now)
        state.apply(json: ["cmd": "card", "kind": "TEST", "body": "Exit: 0"], now: now)
        XCTAssertTrue(state.cardVisible(at: now.addingTimeInterval(3)))
        XCTAssertFalse(state.cardVisible(at: now.addingTimeInterval(4)))
        state.apply(json: ["cmd": "hud", "verdict": NSNull()], now: now)
        XCTAssertNil(state.snapshot().verdict)
        state.apply(json: ["cmd": "git", "commit": "abc"], now: now)
        XCTAssertFalse(state.gitVisible(at: now.addingTimeInterval(5)))
    }
    func testSnapshotDoesNotRenewExpiredCards() {
        let state = OverlayState()
        state.loadSnapshot(["cmd": "card", "body": "old", "card_until_ms": 1000, "git_until_ms": 1000])
        XCTAssertFalse(state.cardVisible(at: Date(timeIntervalSince1970: 2)))
        XCTAssertFalse(state.gitVisible(at: Date(timeIntervalSince1970: 2)))
    }
}
