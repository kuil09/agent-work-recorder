# Failure handling and evidence limits

First distinguish parser errors, recorder failures, child-command failures, and
application behavior. None automatically implies the others. Read stderr before acting.

| Symptom | Action | Do not do |
| --- | --- | --- |
| Unknown command/flag or usage error (2) | Read the installed `rec <COMMAND> --help`; fix argv ordering | Invent `status`, `cancel`, `--json` or `--run-id` |
| No active recording | Start an authorized Run first, unless the task is only help/inspection | Assume note/test implicitly starts capture |
| An active recording already exists | Identify who owns it; use it only with authorization | Kill it or remove its session/lock to make room |
| Missing/ambiguous window | Inspect `rec-capture list-windows` in the approved capture context; select a current numeric ID | Record the app/desktop as an unapproved substitute |
| App missing/ambiguous | Inspect `rec-capture list-apps`; use the exact bundle ID | Assume an app name identifies every process/monitor |
| Capture access unavailable | Run the read-only helper diagnosis and distinguish execution context from actual framework denial | Assume a false preflight means the user must reset permissions |
| `screen-recording-denied` | SCK reported userDeclined; check the responsible host app's permission and relaunch it | Bypass permissions or claim CI proves they work |
| `execution-environment-restricted` | A sandbox marker accompanies failure; compare a host-approved invocation outside the sandbox in the same interactive session | Disable global restrictions, unset markers to evade policy, or automatically reset TCC |
| `gui-session-unavailable` | Use the authorized logged-in console session; restrictions may hide session information | Treat this as proof of TCC denial or run sudo/launchctl to bypass host policy |
| `native-capture-crashed` / `CGS_REQUIRE_INIT` | Preserve the last initialization stage; diagnose the same window ID and report the native failure | Broaden to the app/display without approval or claim the worker's abort was caught in-process |
| rec-capture/ffmpeg/ffprobe missing | Correct the installation/PATH before capture | Install unrelated packages or substitute a fake capture helper |
| A test is still running | Finish it or interrupt its owning process before stop | Start a second test or continually retry stop |
| Child returns nonzero | Record the mechanical result and inspect the cause; still finalize your Run | Convert exit 0 into PASS or use `test && stop` |
| Command finished but its result could not be recorded | Execution may already have happened; inspect recorder and child evidence separately | Automatically rerun a potentially side-effecting command |
| CAPTURE TARGET LOST | Report lost evidence and stop your Run; a later attempt is a new Run | Treat the last frozen image as continued observation |
| Requested audio missing or invalid MP4 | Report finalization failure; retain raw media and logs | Claim a completed recording or silently remove the audio requirement |
| Output already exists | Choose a fresh output before start; preserve the existing file | Delete the unrelated file or force an overwrite |
| Recording is finalizing | Diagnose stop; retry only after addressing a remediable cause | Add new events or assume pause/resume/recovery exists |

## Read-only diagnosis

Check the installed helper's help first; older builds may not have `diagnose`.

```bash
rec-capture --help
rec-capture diagnose
rec-capture diagnose --window-id 1455
```

Replace 1455 with a current, deliberately selected window ID. Diagnosis does not record,
request permission or write media. Window discovery and recording must run in the same
authorized interactive macOS context. When only sandboxed discovery fails, request the
host-approved outside-sandbox invocation for the specific operation; do not attempt an
escape or globally disable sandboxing. The helper's supervisor inherits restrictions.

Environment markers are advisory. A missing marker does not prove the process is
unrestricted. `screen-capture-access-unavailable` deliberately leaves TCC denial,
responsible-app attribution and unknown execution restrictions unresolved. A successful
app capture does not establish that the desktop-independent window-filter path works.
A successful diagnostic does not establish first-frame delivery, window isolation or MP4
validity. The parent PID alone does not establish the app to which TCC attributes access.

Live helper initialization is supervised: worker signals are converted to normal helper
errors. A native worker may still produce a macOS crash report. Preserve that report and
the last `capture-diagnostic` stage from stderr; do not invent a confirmed OS regression.
The source repository's `docs/macos-capture-diagnostics.md` includes a minimal reproducer
and the live acceptance checklist. Do not run the reproducer's deliberately uninitialized
baseline as part of an ordinary recording task.

The shared session is `~/.agent-recorder/session`. Diagnostic logs are
`~/.agent-recorder/logs/<RUNID>.log`. Run working files are under the OS temporary
directory, normally `$TMPDIR/agent-recorder/<RUNID>/` on macOS; the CLI's TMPDIR must
remain consistent across calls. Paths shown here are templates, not existing artifacts.
Do not blindly remove session, lock, sockets or raw media. Successful stop removes Run
temporary files; logs can remain. Failure does not guarantee that raw media is playable.

A full-display/video-only Run can use an already available CuaDriver screenshot fallback.
App, window and requested-audio capture cannot. A fallback is lower-frequency evidence;
do not describe it as native continuous capture. `REC_CAPTURE` is a capture-helper override,
not a way to bypass permissions or replace real evidence with a synthetic fixture.

## Command exit statuses

Normal test termination preserves the child's status. The wrapper uses 124 for timeout,
127 for spawn failure and 128 + signal for signal termination. Recorder errors use 1;
usage errors use 2. A child may return 1, 2, 124 or 127 itself: read diagnostics and the
test result rather than inferring the cause from a number alone.

`rec stop` after a successful stop is not idempotent: it reports no active recording.
After a finalization error, only retry once the specific cause is corrected. A completed
output plus cleanup failure or a damaged capture can require manual diagnosis; no
automatic recovery or output-retargeting command is implemented.

## Evidence inspection

After successful finalization, this is a structural check only:

```bash
ffprobe -v error -show_streams -show_chapters -of json ./review.mp4
```

H.264/AAC tracks and preserved chapters do not prove correct UI layout, intelligible
narrative, correct audio source or A/V synchronization. Inspect those with actual playback
or suitable tools. State precisely which checks were and were not performed.

Synthetic self-tests demonstrate a media pipeline, not ScreenCaptureKit permissions or
live application isolation. A screenshot alone does not prove what happened before/after it.
Human feedback and final acceptance remain external to the recorder.
