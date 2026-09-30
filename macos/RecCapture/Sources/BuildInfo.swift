import CryptoKit
import Foundation

/// Identify the actual executable even when built directly with SwiftPM. An
/// installation revision is reported only for a matching binary SHA-256.
enum HelperBuildInfo {
    static func version(executable: URL) throws -> String {
        let fingerprint = SHA256.hash(data: try Data(contentsOf: executable))
            .map { String(format: "%02x", $0) }.joined()
        let manifest = executable.deletingLastPathComponent().appendingPathComponent("agent-work-recorder-build.json")
        var revision = "unverified (no matching installation manifest)"
        if let data = try? Data(contentsOf: manifest),
           let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let binaries = object["binaries"] as? [String: String],
           binaries["rec-capture"] == fingerprint, let value = object["revision"] as? String {
            revision = value
        }
        return "rec-capture 0.1.0\nExecutable: \(executable.path)\nSHA256: \(fingerprint)\nSource revision: \(revision)"
    }
}
