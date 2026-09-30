import Foundation
import XCTest
@testable import RecCapture

final class BuildInfoTests: XCTestCase {
    func testRevisionRequiresMatchingExecutableDigest() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let executable = root.appendingPathComponent("rec-capture")
        try Data("abc".utf8).write(to: executable)
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        let missing = try HelperBuildInfo.version(executable: executable)
        XCTAssertTrue(missing.contains("SHA256: \(digest)"))
        XCTAssertTrue(missing.contains("unverified"))
        let manifest = root.appendingPathComponent("agent-work-recorder-build.json")
        let data = try JSONSerialization.data(withJSONObject: [
            "revision": "test-revision", "binaries": ["rec-capture": digest],
        ])
        try data.write(to: manifest)
        XCTAssertTrue(try HelperBuildInfo.version(executable: executable).contains("Source revision: test-revision"))
        try Data("different bytes".utf8).write(to: executable)
        XCTAssertTrue(try HelperBuildInfo.version(executable: executable).contains("unverified"))
    }
}
