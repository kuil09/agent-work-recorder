import Darwin
import Foundation
import ScreenCaptureKit
import XCTest
@testable import RecCapture

final class CaptureEnvironmentTests: XCTestCase {
    func testFalsePreflightIsNotProofOfTCCDenial() {
        let error = CaptureEnvironment.accessFailure(environment: [:])
        XCTAssertEqual(error.code, "screen-capture-access-unavailable")
        XCTAssertTrue(error.message.contains("cannot distinguish"))
        XCTAssertTrue(error.message.contains("execution-environment"))
    }

    func testSandboxMarkerHasDistinctAndQualifiedDiagnosis() {
        let environment = ["CODEX_SANDBOX": "seatbelt"]
        let error = CaptureEnvironment.accessFailure(environment: environment)
        XCTAssertEqual(error.code, "execution-environment-restricted")
        XCTAssertTrue(error.message.contains("hint, not proof"))
        XCTAssertTrue(error.message.contains("host-approved"))
        // Even an SCK denial inside a marked environment must not lead only to TCC reset advice.
        XCTAssertEqual(CaptureEnvironment.accessFailure(environment: environment, userDeclined: true).code,
                       "execution-environment-restricted")
        XCTAssertNil(CaptureEnvironment.sandboxHint(["CODEX_SANDBOX": "unknown"]))
        XCTAssertNil(CaptureEnvironment.sandboxHint(["APP_SANDBOX_CONTAINER_ID": ""]))
    }

    func testExplicitFrameworkDenialDiffersFromGenericFailure() {
        let denied = NSError(domain: SCStreamErrorDomain, code: SCStreamError.Code.userDeclined.rawValue)
        XCTAssertEqual((CaptureEnvironment.classify(denied, environment: [:]) as? CaptureDiagnostic)?.code,
                       "screen-recording-denied")
        let other = NSError(domain: SCStreamErrorDomain, code: 4242)
        XCTAssertEqual((CaptureEnvironment.classify(other, environment: [:]) as? CaptureDiagnostic)?.code,
                       "screen-capture-failed")
        let already = CaptureDiagnostic(code: "gui-session-unavailable", message: "test")
        XCTAssertEqual((CaptureEnvironment.classify(already, environment: [:]) as? CaptureDiagnostic)?.code,
                       already.code)
    }

    func testGUIAndThreadPreconditionsAreRecoverableErrors() {
        XCTAssertNoThrow(try CaptureEnvironment.validateSession(mainThread: true, onConsole: true, loginDone: true))
        for pair in [(false, true, true), (true, false, true), (true, true, false)] {
            XCTAssertThrowsError(try CaptureEnvironment.validateSession(mainThread: pair.0, onConsole: pair.1, loginDone: pair.2)) {
                XCTAssertTrue($0 is CaptureDiagnostic)
            }
        }
    }

    func testOnlyLiveCommandsAreSupervised() {
        for command in ["start", "list-windows", "list-apps", "list-displays", "diagnose"] {
            XCTAssertTrue(CaptureSupervisor.liveCommands.contains(command))
        }
        for command in ["self-test", "stamp", "--help"] {
            XCTAssertFalse(CaptureSupervisor.liveCommands.contains(command))
        }
    }

    func testWorkerAbortBecomesAnOrdinaryError() throws {
        let sink = FileHandle(forWritingAtPath: "/dev/null")!
        defer { try? sink.close() }
        let outcome = try CaptureSupervisor.runChild(executable: URL(fileURLWithPath: "/bin/sh"),
            arguments: ["-c", "ulimit -c 0; kill -ABRT $$"], output: sink, error: sink)
        XCTAssertEqual(outcome.signal, SIGABRT)
        XCTAssertEqual(outcome.exitCode, 1)
        XCTAssertEqual(outcome.diagnostic?.code, "native-capture-crashed")
        XCTAssertTrue(outcome.diagnostic!.message.contains("No app/display fallback"))
    }

    func testSupervisorPreservesControlInputStdoutStderrAndOrdinaryExit() throws {
        let file = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try Data("{\"cmd\":\"stop\"}\n".utf8).write(to: file)
        defer { try? FileManager.default.removeItem(at: file) }
        let input = try FileHandle(forReadingFrom: file)
        defer { try? input.close() }
        let output = Pipe(), error = Pipe()
        let outcome = try CaptureSupervisor.runChild(executable: URL(fileURLWithPath: "/bin/sh"),
            arguments: ["-c", "IFS= read -r line; printf '%s\\n' \"$line\"; printf 'diagnostic\\n' >&2; exit 7"],
            input: input, output: output.fileHandleForWriting, error: error.fileHandleForWriting)
        try output.fileHandleForWriting.close(); try error.fileHandleForWriting.close()
        XCTAssertEqual(String(data: output.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8), "{\"cmd\":\"stop\"}\n")
        XCTAssertEqual(String(data: error.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8), "diagnostic\n")
        XCTAssertNil(outcome.signal); XCTAssertNil(outcome.diagnostic); XCTAssertEqual(outcome.exitCode, 7)
    }

    func testInterruptionIsNotMislabelledAsFrameworkCrash() {
        let outcome = CaptureSupervisor.Outcome(status: SIGTERM, signal: SIGTERM)
        XCTAssertEqual(outcome.diagnostic?.code, "capture-interrupted")
        XCTAssertEqual(outcome.exitCode, 1)
    }

    func testDiagnosticArgumentsAreNotRecordingArguments() throws {
        let args = try Args.parse(["diagnose", "--window-id", "1455"])
        XCTAssertEqual(args.command, "diagnose"); XCTAssertEqual(args.windowId, 1455)
        XCTAssertNil(args.output); XCTAssertFalse(args.systemAudio)
        XCTAssertThrowsError(try Args.parse(["diagnose", "--window-id", "0"]))
    }
}
