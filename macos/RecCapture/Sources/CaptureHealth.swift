import CoreVideo
import Foundation

/// Source callbacks and encoded heartbeat copies have independent counters.
/// All access is on CaptureSession.queue. Pixel heuristics never change availability.
final class NativeCaptureHealth {
    let began = ProcessInfo.processInfo.systemUptime
    var framesReceived = 0
    var lastFrame: Double?
    var lastSample: Double?
    var targetAvailable: Bool?
    var captureError: String?
    var visualWarning: String?
    private var fingerprint: UInt64?
    private var unchangedSince: Double?
    private var lastPixelCheck = -Double.infinity

    func sourceFrame(_ pixel: CVPixelBuffer, now: Double = ProcessInfo.processInfo.systemUptime) {
        framesReceived += 1; lastFrame = now; lastSample = now
        guard now - lastPixelCheck >= 1 else { return }
        lastPixelCheck = now
        CVPixelBufferLockBaseAddress(pixel, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pixel, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(pixel) else { return }
        let width = CVPixelBufferGetWidth(pixel), height = CVPixelBufferGetHeight(pixel)
        let stride = CVPixelBufferGetBytesPerRow(pixel)
        let bytes = base.assumingMemoryBound(to: UInt8.self)
        var dark = 0, hash: UInt64 = 14695981039346656037
        for y in 0..<18 { for x in 0..<32 {
            let offset = min(height - 1, y * height / 18) * stride + min(width - 1, x * width / 32) * 4
            if max(bytes[offset], bytes[offset + 1], bytes[offset + 2]) <= 16 { dark += 1 }
            for channel in 0..<3 { hash = (hash ^ UInt64(bytes[offset + channel])) &* 1099511628211 }
        } }
        if fingerprint != hash { unchangedSince = now; fingerprint = hash }
        if dark >= 519 { visualWarning = "source pixels are mostly dark; inspect a frame (a dark page may be legitimate)" }
        else if now - (unchangedSince ?? now) >= 5 { visualWarning = "source pixels appear unchanged for 5 seconds (a static page may be legitimate)" }
        else { visualWarning = nil }
    }
    func idle(now: Double = ProcessInfo.processInfo.systemUptime) { lastSample = now }
    func snapshot(written: Int, validated: Bool, origin: Double?, now: Double = ProcessInfo.processInfo.systemUptime) -> [String: Any] {
        let age = lastSample.map { max(0, now - $0) }
        let state: String
        if captureError != nil { state = "capture_error" }
        else if targetAvailable == false { state = "target_lost" }
        else if !validated || targetAvailable == nil { state = "unverified" }
        else if age.map({ $0 > 3 }) ?? true { state = "missing_frames" }
        else if visualWarning != nil { state = "visual_warning" }
        else { state = "receiving" }
        var health: [String: Any] = ["state": state, "first_frame": validated ? "validated" : "pending", "frames_received": framesReceived, "frames_written": written, "intervals": []]
        health["target_available"] = targetAvailable ?? NSNull() as Any
        health["last_frame_ms"] = lastFrame.map { Int(max(0, $0 - (origin ?? began)) * 1000) } ?? NSNull() as Any
        health["last_frame_age_ms"] = lastFrame.map { Int(max(0, now - $0) * 1000) } ?? NSNull() as Any
        health["last_sample_age_ms"] = lastSample.map { Int(max(0, now - $0) * 1000) } ?? NSNull() as Any
        health["capture_error"] = captureError ?? NSNull() as Any
        health["visual_warning"] = visualWarning ?? NSNull() as Any
        return health
    }
}
