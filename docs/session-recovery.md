# PID inspection failure and an unlinked recorder socket

This guide addresses [issue #7](https://github.com/kuil09/agent-work-recorder/issues/7).
It is not an automatic recovery feature. Reinstalling `rec` does not reconnect an
already-running daemon to a socket pathname that has been unlinked, and an incomplete
MP4 is not necessarily repairable. Do not report a recording as complete based only
on the presence of `raw.mp4`.

## What changed

`rec` now checks a positive PID with the native `kill(pid, 0)` call, without a shell
command or a dependency on the `kill` executable in PATH. Signal 0 delivers no signal.

| Probe result | Interpretation and behavior |
| --- | --- |
| Success | The process exists and can be inspected; attempt the existing session |
| `ESRCH` | The process is reported absent; lookup returns no active session, **without deleting anything** |
| `EPERM`, `EACCES` | Access is denied, not proof of death; return an explicit diagnostic and preserve the Run |
| Other errors | Liveness is unknown; return an explicit diagnostic and preserve the Run |
| Zero or out-of-range PID | Invalid session data, not a process-group target; no syscall or deletion |

The platform distinction is defined by Darwin's public [errno header](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/errno.h)
and [kill implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c).
An existence probe is not a process-identity check. A sandbox may also hide a process;
for this reason **lookup is read-only even after ESRCH**.

The old best-effort stale-session cleanup inside `load_session()` has been removed.
There is no longer a sequence where deleting the session can fail but deleting its
socket can still succeed. Session read errors are reported instead of being hidden
by `Path::exists()`. Explicit successful-stop session cleanup reports errors other
than an already absent file.

A stale session record can be replaced when a new Run starts under the existing
single-Run lock. Lookup itself does not clean up an old socket, raw media or metadata.
Keep evidence before starting a replacement Run. Do not remove `run.lock`: unlinking
an in-use lock file can defeat the single-Run lock.

The shared PID predicate used by the daemon's test-owner checks is conservative:
permission/unknown failures do not falsely reap a live test or authorize early stop.
No production environment variable can override the PID check.

## A restricted command failed, but the endpoint still exists

Stop issuing recording commands from the restricted execution context. Use the
same authorized host context as `rec start`, with the same HOME/TMPDIR and helper
installation. This means a host-approved invocation, not a sandbox escape, TCC reset,
`sudo`, or a permission grant to unrelated software.

Retry an annotation or `rec stop` only from that context and only for a Run you own.
A test command may already have executed before a recorder error; do not blindly
rerun a side-effecting `rec test`. PID permission failure is not Screen Recording
permission failure. The fixed CLI's diagnostic says the session/socket were left
unchanged; it does not claim that the Run is absent.

## An older build already unlinked a live Run's socket

Symptoms are a surviving session/daemon and `connect recorder daemon: No such file
or directory`. The absence of the pathname does not prove the daemon exited.
Do not create a placeholder socket/file, rebind the pathname, delete the session,
start another Run, or kill all processes named `rec`/`rec-capture`.

### 1. Inventory the exact Run without changing it

From the authorized host context, read `~/.agent-recorder/session`. Record its
`run_id`, `pid`, `socket`, `workdir` and `output`. Inspect the daemon and its descendants:

```bash
cat "$HOME/.agent-recorder/session"
ps -axo pid=,ppid=,uid=,lstart=,command=
```

Confirm that the PID still belongs to the expected `rec daemon --config ...` process
under your user, and that the config path and Run ID match. PIDs can be reused; a
PID alone, an executable name alone or a successful `kill -0` is not sufficient.
Do not signal anything if identity or ownership is uncertain.

The daemon's `--config` path identifies the original Run directory. Do not assume
the current shell's `$TMPDIR` is the same. Read that `args.json` and `session.json`;
verify the Run ID, output path and requested capture options. Follow PPID relationships
to identify the exact `rec-capture` supervisor and `_capture-worker` (or the older
unsupervised helper). Record their PIDs and start times. Check whether a `rec test`
process and its child command remain active; allow them to finish or interrupt only
their verified owner before proceeding. This guide is not permission to terminate
someone else's session or test.

### 2. Preserve evidence before sending signals

Create a private, new recovery directory. Set `RUN_DIR` and `RUN_ID` from the verified
inventory, not from an unrelated shell or a guessed path:

```bash
umask 077
RECOVERY_DIR=$(mktemp -d "$HOME/rec-recovery.XXXXXX")
cp -p "$HOME/.agent-recorder/session" "$RECOVERY_DIR/session.before.json"
# RUN_DIR must be the verified directory containing this daemon's args.json.
cp -R "$RUN_DIR" "$RECOVERY_DIR/run.before"
# Copy the matching log if it exists; keep and report any copy error.
cp -p "$HOME/.agent-recorder/logs/$RUN_ID.log" "$RECOVERY_DIR/daemon.before.log"
```

Do not delete or truncate the original files. This is a **live, potentially incomplete
snapshot**: the writer may still be modifying `raw.mp4`. It is not yet a playable
recording or a consistent final backup. Check that copies succeeded before proceeding.
The files may contain private screenshots, command arguments, output and repository
information; do not automatically upload them.

### 3. Stop the verified daemon first; allow the native writer a chance to finish

For the ScreenCaptureKit path, the helper reads control input from a pipe owned by the
daemon. When that pipe closes, its existing EOF path calls `CaptureSession.stop()`.
The issue-5 worker's parent is its supervisor, not the Rust daemon. Therefore leave
the capture supervisor/worker alive initially and request termination of **only the
verified Rust daemon**, using its positive PID:

```bash
# DAEMON_PID must still match the inspected Run, user, command and start time.
/bin/kill -TERM "$DAEMON_PID"
```

Recheck process identity immediately before signaling; do not use negative PIDs,
`killpg`, `pkill`, `killall` or a blanket `kill -9`. No signal number should be inferred
from the `run_id`. If signaling is denied, stop and use the approved host workflow;
do not escalate privileges automatically.

Observe the previously recorded helper PIDs and the Run log. Give the native writer
an opportunity to close normally, up to its usual finalization timeout (30 seconds).
EOF-based finalization is **best effort**: it does not run the dead Rust daemon's
chapter mux/publication path, and raw MP4 finalization can still fail. Killing the
capture worker or supervisor first prevents this opportunity. The screenshot/Cua
fallback is different: its frames are assembled by the Rust daemon, so daemon
termination does not automatically assemble its PNGs into an MP4.

If a verified helper remains stuck, record that fact and preserve another snapshot
before stopping just that helper. Prefer SIGTERM; forced termination is a last resort
with explicit operator approval, after rechecking identity. It may leave an unusable
MP4. Do not imply that SIGTERM to the capture worker guarantees a flushed file.

### 4. Copy stable files and validate; never overwrite evidence

After the exact daemon/helper processes have stopped, retain the original Run files
and make a second copy under a different name:

```bash
cp -R "$RUN_DIR" "$RECOVERY_DIR/run.after"
cp -p "$HOME/.agent-recorder/logs/$RUN_ID.log" "$RECOVERY_DIR/daemon.after.log"
ffprobe -v error -show_streams -show_format -of json \
  "$RECOVERY_DIR/run.after/raw.mp4"
ffmpeg -nostdin -v error -i "$RECOVERY_DIR/run.after/raw.mp4" \
  -map 0:v:0 -map '0:a?' -f null -
```

Successful structural/decoding checks do not prove correct target isolation, complete
narrative, requested audio, A/V synchronization, or preservation of the last frames.
Inspect playback and compare with the recorded options. If FFprobe reports a missing
`moov` atom or invalid input, a plain remux is not a repair: preserve everything and
report recovery failure. Do not fabricate a replacement recording.

Only when the copied raw file is valid may you create a **separate salvaged copy**:

```bash
ffmpeg -nostdin -n -i "$RECOVERY_DIR/run.after/raw.mp4" \
  -map 0:v:0 -map '0:a?' -c copy -movflags +faststart \
  "$RECOVERY_DIR/salvaged.mp4"
```

`-n` prevents overwriting an existing file. Label this as salvaged raw footage, not a
successful `rec stop` result. It may lack chapters/final metadata and may contain an
incomplete final scene. Do not claim recovery of missing audio or UI evidence.

### 5. Keep cleanup separate from recovery

Do not unlink any socket while its owner might still be running. Confirm that the
specific processes have exited in the authorized context; EPERM/uncertain inspection
means **do not clean up**. Archive the stale session record and matching logs first.
Leave the raw Run directory intact. The fixed loader leaves stale records in place;
a new Run can replace the stale session record under the normal lock once the old
daemon is gone. There is no requirement to delete `run.lock` or all temporary sockets.
Any optional stale-file removal must target only the verified old Run and report
permission/I/O errors rather than hiding them. PID reuse or uncertain ownership is
reason to preserve files for manual review, not to broaden cleanup.

## Automated checks and their limits

`cargo test --all-targets` includes native PID validation and deterministic injected
errno tests with real Unix listener paths and unchanged session/socket inode checks.
`python3 tests/check_session_liveness.py` uses a test-only dynamic library to inject
EPERM/EACCES/EIO/ESRCH into real CLI processes. It checks denied note/start/stop,
subsequent host connectivity and finalization, plus denied test-owner inspection in
the daemon. All sessions use isolated HOME/TMPDIR and synthetic capture.

These tests do not recreate Codex's actual sandbox/TCC attribution and do not recover
an existing damaged user recording. Confirm the host-context regression on the
affected Mac after installing the fixed build. The recovery procedure above is a
best-effort operator runbook, not a demonstrated recovery of the Run in issue #7.
