import AVFoundation
import AppKit
import CoreGraphics
import CoreMedia
import Darwin
import Foundation
import ScreenCaptureKit

private let outputLock = NSLock()
func emit(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object), let line = String(data: data, encoding: .utf8) else { return }
    outputLock.lock(); defer { outputLock.unlock() }
    fputs(line + "\n", stdout); fflush(stdout)
}
func ensurePermission() throws {
    try CaptureEnvironment.requireScreenAccess(requestPermission: true)
    try CaptureEnvironment.prepareGUI()
}
func fetchContent() throws -> SCShareableContent {
    let done = DispatchSemaphore(value: 0)
    var result: Result<SCShareableContent, Error>?
    SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
        if let content { result = .success(content) }
        else { result = .failure(CaptureEnvironment.classify(error ?? RecorderError("no shareable content"))) }
        done.signal()
    }
    guard done.wait(timeout: .now() + 8) == .success, let result else { throw RecorderError("shareable content discovery timed out") }
    return try result.get()
}
struct Args {
    var command = "start"
    var output: String?
    var runId = "----"
    var windowId: UInt32?
    var display: String?
    var app: String?
    var systemAudio = false
    var input: String?
    var state: String?
    static func parse(_ values: [String]) throws -> Args {
        var a = Args(); var i = 0
        if let first = values.first, !first.hasPrefix("-") { a.command = first; i = 1 }
        while i < values.count {
            let flag = values[i]; i += 1
            if flag == "--system-audio" { a.systemAudio = true; continue }
            guard i < values.count else { throw RecorderError("missing value for \(flag)") }
            let value = values[i]; i += 1
            switch flag {
            case "--output": a.output = value
            case "--run-id": a.runId = value
            case "--window-id":
                guard let id = UInt32(value), id > 0 else { throw RecorderError("invalid window ID") }; a.windowId = id
            case "--display": a.display = value
            case "--app":
                guard !value.trimmingCharacters(in: .whitespaces).isEmpty else { throw RecorderError("application target cannot be empty") }; a.app = value
            case "--input": a.input = value
            case "--state": a.state = value
            default: throw RecorderError("unknown option: \(flag)")
            }
        }
        if a.windowId != nil && (a.app != nil || a.display != nil) { throw RecorderError("window capture cannot be combined with app/display capture") }
        return a
    }
}
func listContent(_ command: String) throws {
    try ensurePermission(); let content = try fetchContent()
    let rows: [[String: Any]]
    switch command {
    case "list-windows":
        rows = content.windows.filter { $0.frame.width >= 80 && $0.frame.height >= 80 }.map {
            ["id": $0.windowID, "app": $0.owningApplication?.applicationName ?? "", "bundle": $0.owningApplication?.bundleIdentifier ?? "", "title": $0.title ?? "", "width": Int($0.frame.width), "height": Int($0.frame.height)]
        }
    case "list-apps":
        rows = content.applications.map { ["name": $0.applicationName, "bundle": $0.bundleIdentifier, "pid": Int($0.processID)] }
    default:
        rows = content.displays.map { ["id": $0.displayID, "width": $0.width, "height": $0.height, "main": $0.displayID == CGMainDisplayID()] }
    }
    FileHandle.standardOutput.write(try JSONSerialization.data(withJSONObject: rows, options: [.prettyPrinted]))
    FileHandle.standardOutput.write(Data("\n".utf8))
}

final class CaptureSession: NSObject, SCStreamOutput, SCStreamDelegate {
    let queue = DispatchQueue(label: "rec.capture.media")
    let overlay: OverlayState
    let url: URL
    var stream: SCStream?
    var writer: MediaWriter?
    var lastBuffer: CVPixelBuffer?
    var timer: DispatchSourceTimer?
    var stopping = false
    var ready = false
    var failure: Error?
    var appPIDs: [pid_t] = []
    let health = NativeCaptureHealth()
    var healthTimer: DispatchSourceTimer?
    var windowID: UInt32?
    var displayID: CGDirectDisplayID?
    var streamError: String?
    init(url: URL, overlay: OverlayState) { self.url = url; self.overlay = overlay }

    func start(_ args: Args) throws {
        try ensurePermission(); let content = try fetchContent()
        let filter: SCContentFilter
        if let id = args.windowId {
            guard let window = content.windows.first(where: { $0.windowID == id }) else { throw RecorderError("window \(id) not found") }
            windowID = id
            filter = try CaptureEnvironment.windowFilter(window)
        } else {
            let displayID: CGDirectDisplayID
            if args.display == nil || args.display == "main" { displayID = CGMainDisplayID() }
            else if let id = UInt32(args.display ?? "") { displayID = id }
            else { throw RecorderError("invalid display ID") }
            self.displayID = displayID
            guard let display = content.displays.first(where: { $0.displayID == displayID }) else { throw RecorderError("display not found") }
            if let query = args.app {
                let exact = content.applications.filter { $0.bundleIdentifier == query || $0.applicationName.lowercased() == query.lowercased() }
                guard !exact.isEmpty else { throw RecorderError("application not found; use rec-capture list-apps and an exact name or bundle ID") }
                guard Set(exact.map { $0.bundleIdentifier }).count == 1 else { throw RecorderError("application name is ambiguous; use its bundle ID") }
                appPIDs = exact.map { $0.processID }
                // App inclusion keeps newly created windows of these applications in scope.
                // Other applications are never added. This filter is restricted to this display.
                filter = SCContentFilter(display: display, including: exact, exceptingWindows: [])
            } else { filter = SCContentFilter(display: display, excludingWindows: []) }
        }
        health.targetAvailable = true
        let scale = CGFloat(filter.pointPixelScale)
        var width = max(2, Int(filter.contentRect.width * scale))
        var height = max(2, Int(filter.contentRect.height * scale))
        width -= width % 2; height -= height % 2
        let config = SCStreamConfiguration()
        config.width = width; config.height = height
        config.minimumFrameInterval = CMTime(value: 1, timescale: 30)
        config.pixelFormat = kCVPixelFormatType_32BGRA
        config.showsCursor = true; config.queueDepth = 8
        config.capturesAudio = args.systemAudio
        config.sampleRate = 48_000; config.channelCount = 2
        config.excludesCurrentProcessAudio = true
        // No microphone capture or microphone output is ever configured.
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        writer = try MediaWriter(url: url, width: width, height: height, systemAudio: args.systemAudio, overlay: overlay)
        let stream = SCStream(filter: filter, configuration: config, delegate: self)
        try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
        if args.systemAudio { try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue) }
        self.stream = stream
        let done = DispatchSemaphore(value: 0); var startError: Error?
        stream.startCapture { error in startError = error; done.signal() }
        guard done.wait(timeout: .now() + 8) == .success else { throw RecorderError("startCapture timed out") }
        if let startError { throw CaptureEnvironment.classify(startError) }
        queue.async { [self] in
            checkTarget()
            let healthTimer = DispatchSource.makeTimerSource(queue: queue)
            healthTimer.schedule(deadline: .now() + 1, repeating: 1)
            healthTimer.setEventHandler { [weak self] in
                guard let self, !self.stopping else { return }
                self.checkTarget(); self.reportHealth()
            }
            self.healthTimer = healthTimer; healthTimer.resume()
        }
        emit(["event": "started", "width": width, "height": height, "system_audio": args.systemAudio])
    }
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard !stopping, failure == nil, CMSampleBufferIsValid(sampleBuffer) else { return }
        do {
            switch type {
            case .screen:
                if let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
                   let status = attachments.first?[.status] as? Int, status != SCFrameStatus.complete.rawValue {
                    if status == SCFrameStatus.idle.rawValue { health.idle() }
                    return
                }
                guard let pixel = sampleBuffer.imageBuffer else { return }
                guard let writer, CVPixelBufferGetWidth(pixel) == writer.width, CVPixelBufferGetHeight(pixel) == writer.height else {
                    throw RecorderError("source frame dimensions do not match the requested capture")
                }
                health.sourceFrame(pixel)
                lastBuffer = pixel
                if timer == nil {
                    render()
                    let timer = DispatchSource.makeTimerSource(queue: queue)
                    timer.schedule(deadline: .now() + 1.0 / 30, repeating: 1.0 / 30)
                    timer.setEventHandler { [weak self] in self?.render() }
                    self.timer = timer; timer.resume()
                }
            case .audio: try writer?.appendAudio(sampleBuffer)
            default: break
            }
        } catch { fail(error) }
    }
    func render() {
        guard !stopping, failure == nil, let pixel = lastBuffer, let writer else { return }
        do {
            let now = CMClockGetTime(CMClockGetHostTimeClock())
            if try writer.appendVideo(pixel, at: now), !ready, let origin = writer.origin {
                ready = true
                CaptureWorkerLifetime.markReady()
                let elapsed = max(0, CMTimeGetSeconds(CMTimeSubtract(CMClockGetTime(CMClockGetHostTimeClock()), origin)) * 1000)
                reportHealth()
                emit(["event": "ready", "elapsed_ms": Int(elapsed)])
            }
        } catch { fail(error) }
    }
    func mediaElapsed() -> Int {
        guard let origin = writer?.origin else { return 0 }
        return Int(max(0, CMTimeGetSeconds(CMTimeSubtract(CMClockGetTime(CMClockGetHostTimeClock()), origin))) * 1000)
    }
    func reportHealth() {
        let origin = writer?.origin.map { CMTimeGetSeconds($0) }
        emit(["event": "health", "elapsed_ms": mediaElapsed(), "health": health.snapshot(written: writer?.videoFrames ?? 0, validated: ready, origin: origin)])
    }
    func checkTarget() {
        let available: Bool?
        if let windowID {
            if let windows = CGWindowListCopyWindowInfo(.optionAll, kCGNullWindowID) as? [[String: Any]] {
                available = windows.contains { ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value == windowID }
            } else { available = nil }
        } else if !appPIDs.isEmpty {
            available = appPIDs.contains { !(NSRunningApplication(processIdentifier: $0)?.isTerminated ?? true) }
        } else if let displayID { available = CGDisplayIsActive(displayID) != 0 }
        else { available = nil }
        if available == false && health.targetAvailable != false {
            overlay.apply(json: ["cmd": "target_lost"])
            emit(["event": "target_lost", "elapsed_ms": mediaElapsed(), "message": "The selected capture target no longer exists; scope is unchanged."])
        }
        health.targetAvailable = available
    }
    func fail(_ error: Error) {
        if failure == nil {
            failure = error; health.captureError = error.localizedDescription
            emit(["event": "error", "elapsed_ms": mediaElapsed(), "message": error.localizedDescription]); reportHealth()
        }
    }
    func stream(_ stream: SCStream, didStopWithError error: Error) {
        queue.async { [self] in
            streamError = error.localizedDescription; health.captureError = streamError
            checkTarget()
            emit(["event": "error", "elapsed_ms": mediaElapsed(), "message": error.localizedDescription])
            reportHealth()
            // Preserve scoped cached pixels and finalize the writer on explicit stop.
        }
    }
    func stop() {
        queue.async { [self] in
            guard !stopping else { return }
            render(); checkTarget(); reportHealth(); stopping = true; timer?.cancel(); timer = nil; healthTimer?.cancel(); healthTimer = nil
            let complete: (Error?) -> Void = { error in
                if let error { emit(["event": "error", "message": error.localizedDescription]); exit(1) }
                emit(["event": "stopped", "path": self.url.path]); exit(0)
            }
            let finish = {
                self.queue.async {
                    guard let writer = self.writer else { complete(RecorderError("writer missing")); return }
                    let now = CMClockGetTime(CMClockGetHostTimeClock())
                    let minimum = CMTimeAdd(writer.lastVideoPTS ?? now, CMTime(value: 1, timescale: 30))
                    writer.finish(at: CMTimeCompare(now, minimum) > 0 ? now : minimum, completion: complete)
                }
            }
            if let stream { stream.stopCapture { _ in finish() } } else { finish() }
        }
    }
}

func stampFrame(_ args: Args) throws {
    guard let input = args.input, let output = args.output, let path = args.state else { throw RecorderError("stamp requires --input, --output and --state") }
    let state = OverlayState(); state.runId = args.runId
    let object = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: path)))
    guard let object = object as? [String: Any] else { throw RecorderError("invalid overlay state") }
    state.loadSnapshot(object)
    guard let source = NSImage(contentsOfFile: input) else { throw RecorderError("cannot read input image") }
    var rect = NSRect(origin: .zero, size: source.size)
    guard let cg = source.cgImage(forProposedRect: &rect, context: nil, hints: nil),
          let context = CGContext(data: nil, width: cg.width, height: cg.height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue) else { throw RecorderError("cannot create image context") }
    context.translateBy(x: 0, y: CGFloat(cg.height)); context.scaleBy(x: 1, y: -1)
    context.draw(cg, in: CGRect(x: 0, y: 0, width: cg.width, height: cg.height))
    NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
    drawOverlay(size: NSSize(width: cg.width, height: cg.height), state: state.snapshot())
    NSGraphicsContext.restoreGraphicsState()
    guard let image = context.makeImage(), let png = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else { throw RecorderError("PNG encoding failed") }
    try png.write(to: URL(fileURLWithPath: output))
}

let helperHelp = """
Usage:
  rec-capture start --output FILE [--app NAME|BUNDLE | --window-id ID | --display main] [--system-audio]
  rec-capture list-windows | list-apps | list-displays
  rec-capture diagnose [--window-id ID]

Use an authorized interactive macOS desktop session for discovery AND recording.
A sandbox/TCC-attribution restriction is not proof of missing Screen Recording permission.
When only sandboxed execution fails, use the host's approved outside-sandbox invocation;
this helper does not bypass restrictions or expand a window request to an app/display.

Diagnose is read-only: no permission prompt, recording, output file or recorder session.
It reports public preflight evidence, thread/AppKit state and advisory sandbox markers.
With --window-id it also constructs that exact filter; it does not verify live recording.
Native initialization runs in a same-executable worker; worker signals become exit 1
with a structured diagnostic. A worker crash may still generate a macOS crash report.

Other commands: stamp --input PNG --output PNG --state JSON; self-test --output MP4
Synthetic self-test media is not live screen-capture or permission evidence.
See docs/macos-capture-diagnostics.md for the minimal reproducer and acceptance checks.
"""

func main() throws {
    var values = Array(CommandLine.arguments.dropFirst())
    if values == ["--help"] || values == ["-h"] { print(helperHelp); return }
    guard !values.isEmpty else { throw RecorderError(helperHelp) }
    let isWorker = values.first == CaptureSupervisor.workerCommand
    if isWorker { values.removeFirst(); try CaptureWorkerLifetime.start() }
    let args = try Args.parse(values)
    if CaptureSupervisor.liveCommands.contains(args.command) && !isWorker {
        exit(try CaptureSupervisor.run(values))
    }
    if args.command == "diagnose" { try diagnoseCapture(args); return }
    if ["list-windows", "list-apps", "list-displays"].contains(args.command) { try listContent(args.command); return }
    if args.command == "stamp" { try stampFrame(args); return }
    if args.command == "self-test" { try syntheticRecording(args); return }
    guard args.command == "start", let output = args.output else { throw RecorderError("start requires --output") }
    let state = OverlayState(); state.runId = args.runId
    let session = CaptureSession(url: URL(fileURLWithPath: output), overlay: state)
    try session.start(args)
    DispatchQueue.global(qos: .userInitiated).async {
        while let line = readLine() {
            guard let data = line.data(using: .utf8), let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
            if object["cmd"] as? String == "stop" { session.stop(); return }
            state.apply(json: object)
        }
        session.stop()
    }
    RunLoop.main.run()
}
do { try main() } catch { reportCaptureError(error); exit(1) }
