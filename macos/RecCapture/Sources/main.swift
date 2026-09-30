import AVFoundation
import AppKit
import CoreGraphics
import CoreMedia
import Darwin
import Foundation
import ScreenCaptureKit

final class OverlayState: @unchecked Sendable {
    let lock = NSLock()
    var runId = "----"
    var step: UInt32 = 0
    var action: String?
    var verdict: String?
    var cardKind: String?
    var cardBody: String?
    var cardUntil: Date?
    var gitTitle: String?
    var gitRepo: String?
    var gitBranch: String?
    var gitCommit: String?
    var gitTree: String?
    var gitUntil: Date?
    var targetLost = false

    func snapshot() -> OverlayState {
        lock.lock()
        defer { lock.unlock() }
        let copy = OverlayState()
        copy.runId = runId
        copy.step = step
        copy.action = action
        copy.verdict = verdict
        copy.cardKind = cardKind
        copy.cardBody = cardBody
        copy.cardUntil = cardUntil
        copy.gitTitle = gitTitle
        copy.gitRepo = gitRepo
        copy.gitBranch = gitBranch
        copy.gitCommit = gitCommit
        copy.gitTree = gitTree
        copy.gitUntil = gitUntil
        copy.targetLost = targetLost
        return copy
    }

    func apply(json: [String: Any]) {
        lock.lock()
        defer { lock.unlock() }
        let cmd = json["cmd"] as? String ?? ""
        if let run = json["run_id"] as? String { runId = run }
        if let step = json["step"] as? UInt32 { self.step = step }
        else if let step = json["step"] as? Int { self.step = UInt32(step) }
        switch cmd {
        case "hud":
            if let a = json["action"] as? String { action = a }
            if json["verdict"] is NSNull { verdict = nil }
            else if let v = json["verdict"] as? String { verdict = v }
        case "card":
            cardKind = json["kind"] as? String
            cardBody = json["body"] as? String
            cardUntil = Date().addingTimeInterval(4.0)
        case "git":
            gitTitle = json["title"] as? String
            gitRepo = json["repository"] as? String
            gitBranch = json["branch"] as? String
            gitCommit = json["commit"] as? String
            gitTree = json["working_tree"] as? String
            gitUntil = Date().addingTimeInterval(4.5)
        case "target_lost":
            targetLost = true
        case "target_restored":
            targetLost = false
        default:
            break
        }
    }
}

func emit(_ obj: [String: Any]) {
    guard JSONSerialization.isValidJSONObject(obj),
          let data = try? JSONSerialization.data(withJSONObject: obj, options: []),
          let line = String(data: data, encoding: .utf8) else { return }
    fputs(line + "\n", stdout)
    fflush(stdout)
}

func failPermission() -> Never {
    emit(["event": "permission-denied"])
    fputs(
        "Screen Recording permission is required.\n\nSystem Settings >\nPrivacy & Security >\nScreen & System Audio Recording\n",
        stderr
    )
    exit(2)
}

func ensurePermission() {
    if CGPreflightScreenCaptureAccess() { return }
    CGRequestScreenCaptureAccess()
    // Grant is not immediate; caller still may fail until relaunch.
    if !CGPreflightScreenCaptureAccess() {
        failPermission()
    }
}

func fetchContent() throws -> SCShareableContent {
    let sem = DispatchSemaphore(value: 0)
    var result: Result<SCShareableContent, Error>!
    SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
        if let content {
            result = .success(content)
        } else {
            result = .failure(error ?? NSError(domain: "rec-capture", code: 1, userInfo: [
                NSLocalizedDescriptionKey: "no shareable content",
            ]))
        }
        sem.signal()
    }
    if sem.wait(timeout: .now() + 8) == .timedOut {
        throw NSError(domain: "rec-capture", code: 1, userInfo: [
            NSLocalizedDescriptionKey: "timed out fetching shareable content",
        ])
    }
    return try result.get()
}

func listWindows() throws {
    ensurePermission()
    let content = try fetchContent()
    let rows: [[String: Any]] = content.windows.compactMap { w in
        let frame = w.frame
        if frame.width < 80 || frame.height < 80 { return nil }
        return [
            "id": w.windowID,
            "app": w.owningApplication?.applicationName ?? "",
            "bundle": w.owningApplication?.bundleIdentifier ?? "",
            "title": w.title ?? "",
            "width": Int(frame.width),
            "height": Int(frame.height),
        ]
    }
    let data = try JSONSerialization.data(withJSONObject: rows, options: [.prettyPrinted])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

func listDisplays() throws {
    ensurePermission()
    let content = try fetchContent()
    let main = CGMainDisplayID()
    let rows: [[String: Any]] = content.displays.map { d in
        [
            "id": d.displayID,
            "width": d.width,
            "height": d.height,
            "main": d.displayID == main,
        ]
    }
    let data = try JSONSerialization.data(withJSONObject: rows, options: [.prettyPrinted])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

final class CaptureSession: NSObject, SCStreamOutput, SCStreamDelegate {
    let outputURL: URL
    let overlay: OverlayState
    let videoQueue = DispatchQueue(label: "rec.capture.video")
    var stream: SCStream?
    var writer: AVAssetWriter?
    var writerInput: AVAssetWriterInput?
    var adaptor: AVAssetWriterInputPixelBufferAdaptor?
    var startedWriting = false
    var lastBuffer: CVPixelBuffer?
    var lastPTS: CMTime = .zero
    var freezeTimer: DispatchSourceTimer?
    var width = 0
    var height = 0
    let stopLock = NSLock()
    var stopping = false

    init(outputURL: URL, overlay: OverlayState) {
        self.outputURL = outputURL
        self.overlay = overlay
    }

    func start(windowId: UInt32?, display: String?) throws {
        ensurePermission()
        let content = try fetchContent()
        let filter: SCContentFilter
        if let windowId {
            guard let window = content.windows.first(where: { $0.windowID == windowId }) else {
                throw NSError(domain: "rec-capture", code: 3, userInfo: [
                    NSLocalizedDescriptionKey: "window \(windowId) not found",
                ])
            }
            filter = SCContentFilter(desktopIndependentWindow: window)
        } else {
            let main = CGMainDisplayID()
            let displayObj: SCDisplay?
            if display == nil || display == "main" {
                displayObj = content.displays.first(where: { $0.displayID == main }) ?? content.displays.first
            } else if let id = UInt32(display ?? "") {
                displayObj = content.displays.first(where: { $0.displayID == id })
            } else {
                displayObj = content.displays.first(where: { $0.displayID == main })
            }
            guard let displayObj else {
                throw NSError(domain: "rec-capture", code: 3, userInfo: [
                    NSLocalizedDescriptionKey: "display not found",
                ])
            }
            filter = SCContentFilter(display: displayObj, excludingWindows: [])
        }

        var w = 0
        var h = 0
        if #available(macOS 14.0, *) {
            let scale = CGFloat(filter.pointPixelScale)
            w = Int(filter.contentRect.width * scale)
            h = Int(filter.contentRect.height * scale)
        } else {
            w = Int(filter.contentRect.width)
            h = Int(filter.contentRect.height)
        }
        if w < 2 || h < 2 {
            w = 1280
            h = 720
        }
        w -= w % 2
        h -= h % 2
        width = w
        height = h

        let config = SCStreamConfiguration()
        config.width = w
        config.height = h
        config.minimumFrameInterval = CMTime(value: 1, timescale: 30)
        config.pixelFormat = kCVPixelFormatType_32BGRA
        config.showsCursor = true
        config.queueDepth = 8
        if #available(macOS 14.0, *) {
            config.capturesAudio = false
        }

        try FileManager.default.createDirectory(
            at: outputURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        if FileManager.default.fileExists(atPath: outputURL.path) {
            try FileManager.default.removeItem(at: outputURL)
        }

        let writer = try AVAssetWriter(outputURL: outputURL, fileType: .mp4)
        let settings: [String: Any] = [
            AVVideoCodecKey: AVVideoCodecType.h264,
            AVVideoWidthKey: w,
            AVVideoHeightKey: h,
            AVVideoCompressionPropertiesKey: [
                AVVideoAverageBitRateKey: max(2_000_000, w * h * 3),
                AVVideoExpectedSourceFrameRateKey: 30,
                AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
            ],
        ]
        let input = AVAssetWriterInput(mediaType: .video, outputSettings: settings)
        input.expectsMediaDataInRealTime = true
        let adaptor = AVAssetWriterInputPixelBufferAdaptor(
            assetWriterInput: input,
            sourcePixelBufferAttributes: [
                kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
                kCVPixelBufferWidthKey as String: w,
                kCVPixelBufferHeightKey as String: h,
                kCVPixelBufferIOSurfacePropertiesKey as String: [:] as [String: Any],
            ]
        )
        guard writer.canAdd(input) else {
            throw NSError(domain: "rec-capture", code: 4, userInfo: [
                NSLocalizedDescriptionKey: "cannot add video input",
            ])
        }
        writer.add(input)
        self.writer = writer
        self.writerInput = input
        self.adaptor = adaptor

        let stream = SCStream(filter: filter, configuration: config, delegate: self)
        try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: videoQueue)
        self.stream = stream

        let sem = DispatchSemaphore(value: 0)
        var startError: Error?
        stream.startCapture { error in
            startError = error
            sem.signal()
        }
        if sem.wait(timeout: .now() + 8) == .timedOut {
            throw NSError(domain: "rec-capture", code: 5, userInfo: [
                NSLocalizedDescriptionKey: "startCapture timed out",
            ])
        }
        if let startError {
            let ns = startError as NSError
            if ns.domain == "com.apple.ScreenCaptureKit.SCStreamErrorDomain" ||
                ns.localizedDescription.lowercased().contains("denied")
            {
                failPermission()
            }
            throw startError
        }
        emit(["event": "started", "width": w, "height": h])
        emit(["event": "ready"])
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen else { return }
        guard let pixel = sampleBuffer.imageBuffer else { return }
        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        append(pixel: pixel, pts: pts)
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        overlay.lock.lock()
        overlay.targetLost = true
        overlay.lock.unlock()
        emit(["event": "target_lost", "message": error.localizedDescription])
        startFreezeClock()
    }

    func append(pixel: CVPixelBuffer, pts: CMTime) {
        stopLock.lock()
        let stoppingNow = stopping
        stopLock.unlock()
        if stoppingNow { return }

        guard let writer, let writerInput, let adaptor else { return }
        if !startedWriting {
            writer.startWriting()
            writer.startSession(atSourceTime: pts)
            startedWriting = true
        }
        guard writerInput.isReadyForMoreMediaData else { return }
        lastBuffer = pixel
        lastPTS = pts
        let composed = composite(pixel) ?? pixel
        _ = adaptor.append(composed, withPresentationTime: pts)
    }

    func startFreezeClock() {
        freezeTimer?.cancel()
        let timer = DispatchSource.makeTimerSource(queue: videoQueue)
        timer.schedule(deadline: .now() + 1.0 / 30.0, repeating: 1.0 / 30.0)
        timer.setEventHandler { [weak self] in
            guard let self, let last = self.lastBuffer else { return }
            let next = CMTimeAdd(self.lastPTS, CMTime(value: 1, timescale: 30))
            self.append(pixel: last, pts: next)
        }
        freezeTimer = timer
        timer.resume()
    }

    func stop(completion: @escaping () -> Void) {
        stopLock.lock()
        if stopping {
            stopLock.unlock()
            return
        }
        stopping = true
        stopLock.unlock()
        freezeTimer?.cancel()
        freezeTimer = nil
        let finishWriter = { [weak self] in
            guard let self else { completion(); return }
            self.writerInput?.markAsFinished()
            self.writer?.finishWriting {
                emit(["event": "stopped", "path": self.outputURL.path])
                completion()
            }
        }
        if let stream {
            stream.stopCapture { _ in finishWriter() }
        } else {
            finishWriter()
        }
    }

    func composite(_ src: CVPixelBuffer) -> CVPixelBuffer? {
        let w = CVPixelBufferGetWidth(src)
        let h = CVPixelBufferGetHeight(src)
        var dst: CVPixelBuffer?
        let attrs: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
            kCVPixelBufferCGImageCompatibilityKey: true,
            kCVPixelBufferCGBitmapContextCompatibilityKey: true,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as [CFString: Any],
        ]
        let status = CVPixelBufferCreate(
            kCFAllocatorDefault, w, h, kCVPixelFormatType_32BGRA,
            attrs as CFDictionary, &dst
        )
        guard status == kCVReturnSuccess, let dst else { return nil }

        CVPixelBufferLockBaseAddress(src, .readOnly)
        CVPixelBufferLockBaseAddress(dst, [])
        defer {
            CVPixelBufferUnlockBaseAddress(src, .readOnly)
            CVPixelBufferUnlockBaseAddress(dst, [])
        }

        let srcAddr = CVPixelBufferGetBaseAddress(src)!
        let dstAddr = CVPixelBufferGetBaseAddress(dst)!
        let srcStride = CVPixelBufferGetBytesPerRow(src)
        let dstStride = CVPixelBufferGetBytesPerRow(dst)
        for row in 0..<h {
            memcpy(dstAddr.advanced(by: row * dstStride), srcAddr.advanced(by: row * srcStride), min(srcStride, dstStride))
        }

        guard let ctx = CGContext(
            data: dstAddr,
            width: w,
            height: h,
            bitsPerComponent: 8,
            bytesPerRow: dstStride,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
        ) else { return dst }

        let ns = NSGraphicsContext(cgContext: ctx, flipped: true)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = ns
        drawOverlay(size: NSSize(width: w, height: h), state: overlay.snapshot())
        NSGraphicsContext.restoreGraphicsState()
        return dst
    }
}

func drawOverlay(size: NSSize, state: OverlayState) {
    let idFont = NSFont.monospacedSystemFont(ofSize: 24, weight: .bold)
    let kindFont = NSFont.monospacedSystemFont(ofSize: 18, weight: .bold)
    let bodyFont = NSFont.systemFont(ofSize: 17, weight: .medium)
    let metaFont = NSFont.monospacedSystemFont(ofSize: 15, weight: .regular)
    let pad: CGFloat = 14
    let lineH: CGFloat = 26

    let hudId = String(format: "%@:%03u", state.runId, state.step)
    let kind = state.cardKind
    let eventBody = state.cardBody
    let action = state.action
    let verdict = state.verdict
    let gitLine = gitOneLine(state)
    let lost = state.targetLost

    var rows: CGFloat = 3
    if lost { rows += 1 }
    let barH = pad * 2 + rows * lineH + 4
    let bar = NSRect(x: 0, y: 0, width: size.width, height: barH)

    NSColor(white: 0.05, alpha: 0.92).setFill()
    bar.fill()
    NSColor.white.setStroke()
    let edge = NSBezierPath()
    edge.move(to: NSPoint(x: 0, y: bar.maxY))
    edge.line(to: NSPoint(x: size.width, y: bar.maxY))
    edge.lineWidth = 2
    edge.stroke()

    var y = pad
    let idAttr: [NSAttributedString.Key: Any] = [.font: idFont, .foregroundColor: NSColor.white]
    (hudId as NSString).draw(at: NSPoint(x: pad, y: y), withAttributes: idAttr)

    if let kind, !kind.isEmpty {
        let kindColor: NSColor
        switch kind {
        case "CHECKPOINT":
            kindColor = NSColor(calibratedRed: 1, green: 0.78, blue: 0.2, alpha: 1)
        case "EXPECT":
            kindColor = NSColor(calibratedRed: 0.45, green: 0.75, blue: 1, alpha: 1)
        case "OBSERVE":
            kindColor = NSColor(calibratedRed: 0.45, green: 1, blue: 0.55, alpha: 1)
        case "NOTE":
            kindColor = NSColor(white: 0.85, alpha: 1)
        default:
            kindColor = NSColor.white
        }
        (kind as NSString).draw(
            at: NSPoint(x: pad + 150, y: y + 3),
            withAttributes: [.font: kindFont, .foregroundColor: kindColor]
        )
    }

    if let verdict, !verdict.isEmpty {
        let color: NSColor
        if verdict.contains("FAIL") {
            color = NSColor(calibratedRed: 1, green: 0.35, blue: 0.35, alpha: 1)
        } else if verdict.contains("PASS") {
            color = NSColor(calibratedRed: 0.45, green: 1, blue: 0.55, alpha: 1)
        } else if verdict.contains("UNCERTAIN") {
            color = NSColor(calibratedRed: 1, green: 0.85, blue: 0.2, alpha: 1)
        } else {
            color = .white
        }
        let v = verdict as NSString
        let vw = v.size(withAttributes: [.font: kindFont]).width
        v.draw(
            at: NSPoint(x: max(pad, size.width - pad - vw), y: y + 3),
            withAttributes: [.font: kindFont, .foregroundColor: color]
        )
    }

    y += lineH
    let body = eventBody?.isEmpty == false ? eventBody! : (action ?? "")
    if !body.isEmpty {
        let shown = truncate(body, 110)
        (shown as NSString).draw(
            at: NSPoint(x: pad, y: y),
            withAttributes: [.font: bodyFont, .foregroundColor: NSColor.white]
        )
    }

    y += lineH
    (truncate(gitLine, 120) as NSString).draw(
        at: NSPoint(x: pad, y: y),
        withAttributes: [.font: metaFont, .foregroundColor: NSColor(white: 0.8, alpha: 1)]
    )

    if lost {
        y += lineH
        ("CAPTURE TARGET LOST" as NSString).draw(
            at: NSPoint(x: pad, y: y),
            withAttributes: [
                .font: kindFont,
                .foregroundColor: NSColor(calibratedRed: 1, green: 0.35, blue: 0.35, alpha: 1),
            ]
        )
    }

}

func gitOneLine(_ state: OverlayState) -> String {
    if let r = state.gitRepo, let b = state.gitBranch, let c = state.gitCommit {
        let tree = state.gitTree ?? ""
        let title = state.gitTitle.map { "\($0)  " } ?? ""
        return "\(title)\(r)  \(b)  \(c)  \(tree)".trimmingCharacters(in: .whitespaces)
    }
    return "Git: unavailable"
}

func truncate(_ s: String, _ n: Int) -> String {
    if s.count <= n { return s }
    return String(s.prefix(n - 1)) + "…"
}

func wrap(_ s: String, font: NSFont, width: CGFloat) -> [String] {
    var lines: [String] = []
    for raw in s.split(separator: "\n", omittingEmptySubsequences: false) {
        var current = ""
        for word in raw.split(separator: " ", omittingEmptySubsequences: false) {
            let next = current.isEmpty ? String(word) : current + " " + word
            let w = (next as NSString).size(withAttributes: [.font: font]).width
            if w > width, !current.isEmpty {
                lines.append(current)
                current = String(word)
            } else {
                current = next
            }
        }
        lines.append(current)
    }
    return Array(lines.prefix(8))
}

struct Args {
    var command = "start"
    var output: String?
    var runId = "----"
    var windowId: UInt32?
    var display: String?
    var input: String?
    var state: String?
}

func parseArgs() -> Args {
    var a = Args()
    var it = CommandLine.arguments.dropFirst().makeIterator()
    if let first = CommandLine.arguments.dropFirst().first, !first.hasPrefix("-") {
        a.command = first
        _ = it.next()
    }
    // re-parse simply
    let args = Array(CommandLine.arguments.dropFirst())
    var i = 0
    if i < args.count, !args[i].hasPrefix("-") {
        a.command = args[i]
        i += 1
    }
    while i < args.count {
        let t = args[i]
        i += 1
        func take() -> String {
            guard i < args.count else { return "" }
            let v = args[i]
            i += 1
            return v
        }
        switch t {
        case "--output": a.output = take()
        case "--run-id": a.runId = take()
        case "--window-id": a.windowId = UInt32(take())
        case "--display": a.display = take()
        case "--input": a.input = take()
        case "--state": a.state = take()
        default: break
        }
    }
    return a
}

func stampFrame(input: String, output: String, statePath: String?, runId: String) {
    let overlay = OverlayState()
    overlay.runId = runId
    if let statePath, let data = try? Data(contentsOf: URL(fileURLWithPath: statePath)),
       let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    {
        overlay.apply(json: obj)
        if let run = obj["run_id"] as? String { overlay.runId = run }
        if let step = obj["step"] as? Int { overlay.step = UInt32(step) }
        if let a = obj["action"] as? String { overlay.action = a }
        if let v = obj["verdict"] as? String { overlay.verdict = v }
        if let k = obj["kind"] as? String { overlay.cardKind = k }
        if let b = obj["body"] as? String { overlay.cardBody = b }
        if let t = obj["title"] as? String { overlay.gitTitle = t }
        if let r = obj["repository"] as? String { overlay.gitRepo = r }
        if let br = obj["branch"] as? String { overlay.gitBranch = br }
        if let c = obj["commit"] as? String { overlay.gitCommit = c }
        if let wt = obj["working_tree"] as? String { overlay.gitTree = wt }
        if let showGit = obj["show_git"] as? Bool, showGit {
            overlay.gitUntil = Date().addingTimeInterval(60)
        }
        if let showCard = obj["show_card"] as? Bool, showCard {
            overlay.cardUntil = Date().addingTimeInterval(60)
        }
        if let lost = obj["target_lost"] as? Bool { overlay.targetLost = lost }
    }
    guard let src = NSImage(contentsOf: URL(fileURLWithPath: input)) else {
        fputs("stamp: cannot read \(input)\n", stderr)
        exit(1)
    }
    var rect = NSRect(origin: .zero, size: src.size)
    guard let cg = src.cgImage(forProposedRect: &rect, context: nil, hints: nil) else {
        fputs("stamp: no cgImage\n", stderr)
        exit(1)
    }
    let w = cg.width
    let h = cg.height
    guard let colorSpace = cg.colorSpace ?? CGColorSpace(name: CGColorSpace.sRGB),
          let ctx = CGContext(
            data: nil,
            width: w,
            height: h,
            bitsPerComponent: 8,
            bytesPerRow: 0,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
          )
    else {
        fputs("stamp: no context\n", stderr)
        exit(1)
    }
    ctx.translateBy(x: 0, y: CGFloat(h))
    ctx.scaleBy(x: 1, y: -1)
    ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
    let ns = NSGraphicsContext(cgContext: ctx, flipped: true)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = ns
    drawOverlay(size: NSSize(width: w, height: h), state: overlay.snapshot())
    NSGraphicsContext.restoreGraphicsState()
    guard let outImg = ctx.makeImage() else {
        fputs("stamp: makeImage failed\n", stderr)
        exit(1)
    }
    let dest = URL(fileURLWithPath: output)
    let rep = NSBitmapImageRep(cgImage: outImg)
    guard let png = rep.representation(using: .png, properties: [:]) else {
        fputs("stamp: png encode failed\n", stderr)
        exit(1)
    }
    do {
        try png.write(to: dest)
    } catch {
        fputs("stamp: write \(error.localizedDescription)\n", stderr)
        exit(1)
    }
}

if CommandLine.arguments.count < 2 {
    fputs("usage: rec-capture start --output FILE [--run-id ID] [--window-id N | --display main]\n", stderr)
    fputs("       rec-capture list-windows | list-displays | stamp --input PNG --output PNG --state JSON\n", stderr)
    exit(1)
}

let args = parseArgs()
switch args.command {
case "list-windows":
    do { try listWindows() } catch {
        emit(["event": "error", "message": error.localizedDescription])
        fputs(error.localizedDescription + "\n", stderr)
        exit(1)
    }
    exit(0)
case "list-displays":
    do { try listDisplays() } catch {
        emit(["event": "error", "message": error.localizedDescription])
        fputs(error.localizedDescription + "\n", stderr)
        exit(1)
    }
    exit(0)
case "stamp":
    guard let input = args.input, let output = args.output else {
        fputs("stamp requires --input and --output\n", stderr)
        exit(1)
    }
    stampFrame(input: input, output: output, statePath: args.state, runId: args.runId)
    exit(0)
case "start":
    break
default:
    fputs("usage: rec-capture start --output FILE [--run-id ID] [--window-id N | --display main]\n", stderr)
    fputs("       rec-capture list-windows | list-displays | stamp --input PNG --output PNG --state JSON\n", stderr)
    exit(1)
}

guard let output = args.output else {
    fputs("--output is required\n", stderr)
    exit(1)
}

let overlay = OverlayState()
overlay.runId = args.runId
let session = CaptureSession(outputURL: URL(fileURLWithPath: output), overlay: overlay)

do {
    try session.start(windowId: args.windowId, display: args.display)
} catch {
    emit(["event": "error", "message": error.localizedDescription])
    fputs(error.localizedDescription + "\n", stderr)
    exit(1)
}

DispatchQueue.global(qos: .userInitiated).async {
    while let line = readLine(strippingNewline: true) {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { continue }
        guard let data = trimmed.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { continue }
        let cmd = obj["cmd"] as? String ?? ""
        if cmd == "stop" {
            DispatchQueue.main.async {
                session.stop {
                    exit(0)
                }
            }
            return
        }
        overlay.apply(json: obj)
        if let run = obj["run_id"] as? String { overlay.runId = run }
    }
    DispatchQueue.main.async {
        session.stop { exit(0) }
    }
}

RunLoop.main.run()
