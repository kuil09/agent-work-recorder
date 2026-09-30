"""Exercise production supervision using a synthetic worker, never a live desktop.

Run on macOS after the helper release build. The shim links the production
CaptureSupervisor.swift; only its worker workload and diagnostic type are fixtures.
No production abort flag, environment escape, or real capture is introduced.
"""
import json
import os
from pathlib import Path
import resource
import select
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / "macos/RecCapture/.build/release/rec-capture"
SHIM = r'''
import Darwin
import Foundation
struct CaptureDiagnostic: LocalizedError {
    let code: String
    let message: String
    var errorDescription: String? { "[\(code)] \(message)" }
}
func emit(_ object: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: object)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}
let args = Array(CommandLine.arguments.dropFirst())
do {
    if args.first == CaptureSupervisor.workerCommand {
        try CaptureWorkerLifetime.start()
        let mode = args.dropFirst().first ?? "idle"
        if mode == "abort" { abort() }
        if mode == "exit7" { exit(7) }
        if mode == "ready" { CaptureWorkerLifetime.markReady() }
        emit(["event": "fixture", "pid": getpid(), "parent": getppid(),
              "sandbox_marker": ProcessInfo.processInfo.environment["CODEX_SANDBOX"] ?? ""])
        if mode == "echo" {
            while let line = readLine() {
                FileHandle.standardOutput.write(Data((line + "\n").utf8))
            }
        } else { RunLoop.main.run() }
    } else { exit(try CaptureSupervisor.run(args)) }
} catch { reportCaptureError(error); exit(1) }
'''


def run(*args, **kwargs):
    return subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          timeout=30, **kwargs)


def events(result):
    return [json.loads(line) for line in result.stdout.splitlines()]


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def wait_gone(pid):
    deadline = time.monotonic() + 5
    while alive(pid) and time.monotonic() < deadline:
        time.sleep(0.05)
    assert not alive(pid), f"worker {pid} survived its supervisor"


def ready_process(binary):
    process = subprocess.Popen([str(binary), "ready"], text=True,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        assert select.select([process.stdout], [], [], 10)[0], "no fixture readiness"
        event = json.loads(process.stdout.readline())
        assert event["event"] == "fixture", event
        assert event["parent"] == process.pid, event
        return process, event["pid"]
    except BaseException:
        process.kill()
        process.communicate(timeout=10)
        raise


def main():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    results = []
    with tempfile.TemporaryDirectory(prefix="rec-supervisor-") as directory:
        temp = Path(directory)
        source = temp / "main.swift"
        source.write_text(SHIM)
        binary = temp / "supervisor-fixture"
        subprocess.run(["swiftc", str(ROOT / "macos/RecCapture/Sources/CaptureSupervisor.swift"),
                        str(source), "-o", str(binary)], check=True, timeout=90)

        crashed = run(str(binary), "abort")
        assert crashed.returncode == 1, crashed
        assert events(crashed)[-1]["code"] == "native-capture-crashed", crashed
        assert run(str(binary), "exit7").returncode == 7
        results.append("SIGABRT contained; ordinary exit status preserved")

        environment = {**os.environ, "CODEX_SANDBOX": "seatbelt"}
        echoed = run(str(binary), "echo", input='{"cmd":"stop"}\n', env=environment)
        assert echoed.returncode == 0, echoed
        assert events(echoed)[0]["sandbox_marker"] == "seatbelt", echoed
        assert events(echoed)[1] == {"cmd": "stop"}, echoed
        results.append("stdin/stdout and sandbox marker inherited (not a real sandbox test)")

        for terminating_signal in (signal.SIGTERM, signal.SIGKILL):
            process, worker = ready_process(binary)
            try:
                process.send_signal(terminating_signal)
                output, error = process.communicate(timeout=10)
                if terminating_signal == signal.SIGTERM:
                    assert process.returncode == 1, (output, error)
                    assert json.loads(output.strip())["code"] == "capture-interrupted", (output, error)
                else:
                    assert process.returncode == -signal.SIGKILL
                wait_gone(worker)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=10)
                if alive(worker):
                    os.kill(worker, signal.SIGKILL)
            results.append(f"supervisor signal {terminating_signal}: no orphan worker")

        timed_out = run(str(binary), "idle")
        assert timed_out.returncode == 1, timed_out
        assert events(timed_out)[-1]["code"] == "native-startup-timeout", timed_out
        results.append("native readiness timeout is bounded")

        isolated = {**os.environ, "HOME": directory, "TMPDIR": directory,
                    "PATH": "/nonexistent", "CODEX_SANDBOX": "seatbelt"}
        help_result = run(str(HELPER), "--help", env=isolated)
        assert help_result.returncode == 0 and "diagnose" in help_result.stdout, help_result
        forbidden = temp / "must-not-exist.mp4"
        invalid = run(str(HELPER), "diagnose", "--output", str(forbidden), env=isolated)
        assert invalid.returncode == 1, invalid
        assert "diagnose accepts only" in invalid.stderr, invalid
        assert not forbidden.exists() and not (temp / ".agent-recorder").exists()
        results.append("real helper help and invalid diagnostic request do not record or mutate sessions")

        subprocess.run(["swiftc", str(ROOT / "tools/repro-window-filter.swift"),
                        "-o", str(temp / "window-filter-repro")], check=True, timeout=90)
        results.append("standalone native reproducer compiles (not executed without authorization)")

    report = {"ok": True, "checks": results, "live_capture_tested": False}
    artifacts = ROOT / "artifacts"
    artifacts.mkdir(exist_ok=True)
    (artifacts / "capture-supervisor-report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
