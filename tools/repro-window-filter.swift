// Minimal issue #5 reproducer. It discovers and constructs ONE window filter, never records.
// Build: swiftc tools/repro-window-filter.swift -o /tmp/rec-window-filter-repro
// Run in a host-approved interactive session with Screen Recording access:
//   /tmp/rec-window-filter-repro WINDOW_ID
// Optional historical baseline (may SIGABRT, intentionally NOT a production option):
//   /tmp/rec-window-filter-repro WINDOW_ID --without-appkit
import AppKit
import CoreGraphics
import Foundation
import ScreenCaptureKit

func trace(_ stage: String) {
    let value: [String: Any] = ["stage": stage, "pid": ProcessInfo.processInfo.processIdentifier,
        "main_thread": Thread.isMainThread, "appkit_initialized": NSApp != nil,
        "os": ProcessInfo.processInfo.operatingSystemVersionString]
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardError.write(data); FileHandle.standardError.write(Data("\n".utf8))
}
let arguments = Array(CommandLine.arguments.dropFirst())
guard (arguments.count == 1 || (arguments.count == 2 && arguments[1] == "--without-appkit")),
      let first = arguments.first, let id = UInt32(first), id > 0 else {
    fputs("usage: rec-window-filter-repro WINDOW_ID [--without-appkit]\n", stderr); exit(2)
}
trace("before-initialization")
if !arguments.contains("--without-appkit") {
    _ = NSApplication.shared.setActivationPolicy(.prohibited)
    _ = NSScreen.screens
}
trace("after-initialization")
guard CGPreflightScreenCaptureAccess() else {
    fputs("Capture access unavailable in this invocation; TCC vs execution restriction is not determined. No permission prompt.\n", stderr); exit(1)
}
let done = DispatchSemaphore(value: 0)
var content: SCShareableContent?
var discoveryError: Error?
SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { value, error in
    content = value; discoveryError = error; done.signal()
}
guard done.wait(timeout: .now() + 8) == .success,
      let window = content?.windows.first(where: { $0.windowID == id }) else {
    fputs("Window unavailable: \(discoveryError?.localizedDescription ?? "not found or discovery timed out")\n", stderr); exit(1)
}
trace("before-window-filter")
let filter = SCContentFilter(desktopIndependentWindow: window)
trace("after-window-filter")
print("Filter initialized: \(filter.contentRect). This does not establish live capture success.")
