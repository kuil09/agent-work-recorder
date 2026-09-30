import AppKit
import CoreGraphics
import Foundation
import ScreenCaptureKit

/// A diagnostic is evidence about this invocation, not a query of the user's TCC database.
struct CaptureDiagnostic: LocalizedError {
    let code: String
    let message: String
    var errorDescription: String? { "[\(code)] \(message)" }
}

enum CaptureEnvironment {
    /// Environment markers are advisory. Their absence does not prove an unrestricted process.
    static func sandboxHint(_ environment: [String: String]) -> String? {
        if environment["CODEX_SANDBOX"] == "seatbelt" { return "CODEX_SANDBOX=seatbelt" }
        if let value = environment["APP_SANDBOX_CONTAINER_ID"], !value.isEmpty {
            return "APP_SANDBOX_CONTAINER_ID is present"
        }
        return nil
    }

    static func accessFailure(environment: [String: String], userDeclined: Bool = false) -> CaptureDiagnostic {
        if let hint = sandboxHint(environment) {
            return CaptureDiagnostic(code: "execution-environment-restricted", message: "Screen capture is unavailable in a sandbox-indicated execution context (\(hint)). This marker is a hint, not proof of the kernel restriction or of the user's permission setting. Use a host-approved invocation outside the sandbox in the same logged-in macOS desktop session, then repeat discovery and capture there. Do not reset TCC permissions solely because this invocation failed. No sandbox escape or capture-scope expansion is attempted.")
        }
        if userDeclined {
            return CaptureDiagnostic(code: "screen-recording-denied", message: "ScreenCaptureKit reported userDeclined for this invocation. Check Screen Recording access for the responsible terminal/agent app in System Settings > Privacy & Security, then relaunch that app. Also compare an approved non-sandboxed invocation if the failure is environment-specific; the responsible app may differ from the helper's parent process.")
        }
        return CaptureDiagnostic(code: "screen-capture-access-unavailable", message: "CGPreflightScreenCaptureAccess returned false. This alone cannot distinguish missing/denied Screen Recording permission, responsible-app TCC attribution, or an execution-environment restriction. Compare a host-approved invocation outside the sandbox in the same logged-in desktop session. If that also fails, check Screen Recording access for the responsible terminal/agent and relaunch it. Do not infer a permission reset is required.")
    }

    static func classify(_ error: Error, environment: [String: String] = ProcessInfo.processInfo.environment) -> Error {
        if error is CaptureDiagnostic { return error }
        let ns = error as NSError
        if ns.domain == SCStreamErrorDomain && ns.code == SCStreamError.Code.userDeclined.rawValue {
            return accessFailure(environment: environment, userDeclined: true)
        }
        // A framework error is not automatically a permission denial.
        return CaptureDiagnostic(code: "screen-capture-failed", message: "\(ns.domain) (\(ns.code)): \(ns.localizedDescription). Execution restrictions or a framework failure may be involved; this is not a confirmed Screen Recording permission denial.")
    }

    static func validateSession(mainThread: Bool, onConsole: Bool, loginDone: Bool) throws {
        guard mainThread else {
            throw CaptureDiagnostic(code: "gui-thread-required", message: "Initialize AppKit and the capture filter on the main thread.")
        }
        guard onConsole && loginDone else {
            throw CaptureDiagnostic(code: "gui-session-unavailable", message: "No logged-in on-console macOS desktop session is visible to this process. Run discovery and capture in the authorized interactive user's session. A sandbox may hide session information; this is not a TCC denial diagnosis. Do not use sudo or launchctl to bypass the host's execution policy.")
        }
    }

    /// Only called in a supervised worker, before any SCContentFilter is constructed.
    static func prepareGUI() throws {
        let session = CGSessionCopyCurrentDictionary() as? [String: Any]
        try validateSession(mainThread: Thread.isMainThread,
            onConsole: session?[kCGSessionOnConsoleKey as String] as? Bool ?? false,
            loginDone: session?[kCGSessionLoginDoneKey as String] as? Bool ?? false)
        trace(stage: "before-appkit-init")
        let application = NSApplication.shared
        // Establish AppKit's WindowServer connection without activating or creating a window.
        _ = application.setActivationPolicy(.prohibited)
        guard !NSScreen.screens.isEmpty else {
            throw CaptureDiagnostic(code: "gui-session-unavailable", message: "AppKit cannot see a display in this execution context. Use an authorized interactive desktop session; no broader capture fallback is selected.")
        }
        trace(stage: "after-appkit-init")
    }

    static func requireScreenAccess(requestPermission: Bool) throws {
        if CGPreflightScreenCaptureAccess() { return }
        let environment = ProcessInfo.processInfo.environment
        // Avoid repeatedly asking for permission in a known restricted context. Do not attempt escape.
        if requestPermission && sandboxHint(environment) == nil {
            _ = CGRequestScreenCaptureAccess()
            if CGPreflightScreenCaptureAccess() { return }
        }
        throw accessFailure(environment: environment)
    }

    static func windowFilter(_ window: SCWindow) throws -> SCContentFilter {
        guard Thread.isMainThread, NSApp != nil else {
            throw CaptureDiagnostic(code: "gui-not-initialized", message: "AppKit must be initialized on the main thread before creating a window filter.")
        }
        trace(stage: "before-window-filter", windowID: window.windowID)
        // Keep exactly the requested window. Never silently replace this with an app/display filter.
        let filter = SCContentFilter(desktopIndependentWindow: window)
        trace(stage: "after-window-filter", windowID: window.windowID)
        return filter
    }

    static func snapshot(stage: String, windowID: UInt32? = nil) -> [String: Any] {
        var result: [String: Any] = ["stage": stage, "pid": ProcessInfo.processInfo.processIdentifier,
            "parent_pid": getppid(), "os": ProcessInfo.processInfo.operatingSystemVersionString,
            "main_thread": Thread.isMainThread, "appkit_initialized": NSApp != nil,
            "sandbox_hint": sandboxHint(ProcessInfo.processInfo.environment) ?? "none-observed (not proof of unrestricted execution)"]
        if let windowID { result["window_id"] = windowID }
        return result
    }

    static func trace(stage: String, windowID: UInt32? = nil) {
        var value = snapshot(stage: stage, windowID: windowID); value["event"] = "capture-diagnostic"
        if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) {
            // Discovery stdout remains a JSON array. Diagnostics belong on stderr.
            FileHandle.standardError.write(data); FileHandle.standardError.write(Data("\n".utf8))
        }
    }
}

/// Read-only preflight: no permission prompt, recording, media output, or session mutation.
func diagnoseCapture(_ args: Args) throws {
    guard args.output == nil, args.input == nil, args.state == nil, args.app == nil,
          args.display == nil, !args.systemAudio else {
        throw RecorderError("diagnose accepts only an optional --window-id; it never records")
    }
    var report = CaptureEnvironment.snapshot(stage: "preflight", windowID: args.windowId)
    report["event"] = "diagnostic"; report["records_media"] = false
    do {
        report["screen_access_preflight"] = CGPreflightScreenCaptureAccess()
        // Give the sandbox-specific diagnosis before touching AppKit when access is unavailable.
        try CaptureEnvironment.requireScreenAccess(requestPermission: false)
        try CaptureEnvironment.prepareGUI()
        let content = try fetchContent()
        report["display_count"] = content.displays.count
        if let id = args.windowId {
            guard let window = content.windows.first(where: { $0.windowID == id }) else {
                throw CaptureDiagnostic(code: "window-not-found", message: "Window \(id) is not currently available. Repeat list-windows in this same execution context; do not select another target automatically.")
            }
            let filter = try CaptureEnvironment.windowFilter(window)
            report["filter_width"] = filter.contentRect.width
            report["filter_height"] = filter.contentRect.height
            report["window_filter_initialized"] = true
        }
        report["ok"] = true; report["appkit_initialized"] = NSApp != nil
        report["stage"] = "preflight-complete"
        report["message"] = "Preflight succeeded; actual window isolation, first-frame delivery and media validity still require a live recording."
        emit(report)
    } catch {
        let diagnostic = error as? CaptureDiagnostic
        report["ok"] = false; report["code"] = diagnostic?.code ?? "preflight-failed"
        report["message"] = error.localizedDescription
        emit(report)
        exit(1)
    }
}
