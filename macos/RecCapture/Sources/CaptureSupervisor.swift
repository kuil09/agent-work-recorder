import Darwin
import Foundation

/// Native assertions cannot be caught by Swift do/catch. Keep them out of the public helper.
/// The worker is the same executable and inherits the same sandbox, identity and file handles.
/// This is crash containment, NOT privilege escalation or a ScreenCaptureKit fallback.
enum CaptureSupervisor {
    static let workerCommand = "_capture-worker"
    static let parentVariable = "_REC_CAPTURE_SUPERVISOR_PID"
    static let liveCommands = ["start", "list-windows", "list-apps", "list-displays", "diagnose"]

    struct Outcome {
        let status: Int32
        let signal: Int32?
        var diagnostic: CaptureDiagnostic? {
            guard let signal else { return nil }
            if signal == SIGINT || signal == SIGTERM {
                return CaptureDiagnostic(code: "capture-interrupted", message: "The native capture worker was interrupted (signal \(signal)); no completed MP4 is claimed.")
            }
            return CaptureDiagnostic(code: "native-capture-crashed", message: "The native capture worker terminated with signal \(signal). The public helper returns a normal error instead of propagating the abort. Review the last capture-diagnostic stage and run rec-capture diagnose --window-id ID in the same authorized interactive context. A WindowServer/ScreenCaptureKit initialization failure is possible; this is not a confirmed permission denial. No app/display fallback was selected.")
        }
        var exitCode: Int32 { signal == nil ? status : 1 }
    }

    static func runChild(executable: URL, arguments: [String],
                         input: FileHandle = .standardInput,
                         output: FileHandle = .standardOutput,
                         error: FileHandle = .standardError,
                         forwardSignals: Bool = false) throws -> Outcome {
        let child = Process()
        child.executableURL = executable; child.arguments = arguments
        child.standardInput = input; child.standardOutput = output; child.standardError = error
        var environment = ProcessInfo.processInfo.environment
        environment[parentVariable] = String(getpid())
        child.environment = environment
        try child.run()
        var sources: [DispatchSourceSignal] = []
        if forwardSignals {
            // Install only in the supervisor, after spawn: the worker retains default handlers.
            for number in [SIGINT, SIGTERM] {
                Darwin.signal(number, SIG_IGN)
                let source = DispatchSource.makeSignalSource(signal: number, queue: .global())
                source.setEventHandler { if child.isRunning { Darwin.kill(child.processIdentifier, number) } }
                source.resume(); sources.append(source)
            }
        }
        defer { for source in sources { source.cancel() } }
        child.waitUntilExit()
        return Outcome(status: child.terminationStatus,
            signal: child.terminationReason == .uncaughtSignal ? child.terminationStatus : nil)
    }

    static func run(_ arguments: [String]) throws -> Int32 {
        guard let executable = Bundle.main.executableURL else {
            throw CaptureDiagnostic(code: "helper-executable-unavailable", message: "Cannot resolve the current helper executable.")
        }
        let outcome = try runChild(executable: executable, arguments: [workerCommand] + arguments, forwardSignals: true)
        if let diagnostic = outcome.diagnostic { reportCaptureError(diagnostic) }
        return outcome.exitCode
    }
}

/// A killed supervisor must not leave a recording worker behind. Also bound native startup hangs.
/// This is not crash recovery: an interrupted raw MP4 may not be usable.
enum CaptureWorkerLifetime {
    private static let lock = NSLock()
    private static var ready = false
    private static var timer: DispatchSourceTimer?

    static func markReady() { lock.lock(); ready = true; lock.unlock() }

    static func start() throws {
        guard let value = ProcessInfo.processInfo.environment[CaptureSupervisor.parentVariable],
              let parent = Int32(value), parent > 1, getppid() == parent else {
            throw CaptureDiagnostic(code: "internal-worker-invocation", message: "The capture worker must be launched by rec-capture, not called directly.")
        }
        let deadline = ProcessInfo.processInfo.systemUptime + 16
        let source = DispatchSource.makeTimerSource(queue: .global(qos: .userInitiated))
        source.schedule(deadline: .now() + 0.25, repeating: 0.25)
        source.setEventHandler {
            if getppid() != parent {
                fputs("[capture-supervisor-lost] Supervisor exited; stopping the worker.\n", stderr)
                _exit(1)
            }
            lock.lock(); let started = ready; lock.unlock()
            if !started && ProcessInfo.processInfo.systemUptime >= deadline {
                reportCaptureError(CaptureDiagnostic(code: "native-startup-timeout", message: "Native discovery/capture did not become ready within 16 seconds. Review capture-diagnostic stages in the same authorized desktop context; no fallback was selected."))
                _exit(1)
            }
        }
        timer = source; source.resume()
    }
}

func reportCaptureError(_ error: Error) {
    var event: [String: Any] = ["event": "error", "message": error.localizedDescription]
    if let diagnostic = error as? CaptureDiagnostic { event["code"] = diagnostic.code }
    emit(event)
    fputs(error.localizedDescription + "\n", stderr)
}
