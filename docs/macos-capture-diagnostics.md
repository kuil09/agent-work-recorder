# macOS capture context and window-filter initialization

This document tracks [issue #5](https://github.com/kuil09/agent-work-recorder/issues/5).
The report describes a window-filter `CGS_REQUIRE_INIT` assertion on macOS 26.5.2,
while app capture works and sandboxed discovery fails. These observations do not prove
that the user has denied Screen Recording permission, or that every SCK path is broken.

## Run discovery and capture in the same authorized context

Use a logged-in, on-console macOS desktop session with Screen Recording access for the
responsible terminal/agent app. If discovery fails only inside the coding agent sandbox,
request the host's approved outside-sandbox execution for the specific discovery and
recording commands. Do not disable the sandbox globally, attempt an escape, run sudo,
or reset the TCC database automatically. A child process cannot grant itself permission.
The new supervisor inherits restrictions; it is not an outside-sandbox launcher.

```bash
# Read-only: no permission prompt, recording or output file.
rec-capture diagnose

# In the SAME approved context, obtain a CURRENT window ID.
rec-capture list-windows
rec-capture diagnose --window-id 1455

# 1455 is the report's example, not a stable ID. Replace it with the selected current ID.
rec start --title "Issue 5 window verification" --window-id 1455
rec note "Check that only the explicitly selected window is visible"
# Perform and inspect the intended UI interaction; do not infer PASS from command success.
rec checkpoint "Window-only capture acceptance"
rec stop
```

Read stderr and the exit status. Diagnostic stdout is JSON, and discovery success stdout
remains a JSON array. Stage evidence is written as `capture-diagnostic` JSON on stderr.
The normal `rec` daemon retains this in `~/.agent-recorder/logs/<RUNID>.log`.
`rec-capture --help` describes the diagnostic command without accessing the desktop.
Keep HOME/TMPDIR consistent across a Run; do not delete another Run's session or lock.

## Interpret the evidence, not just the word 'permission'

| Code | What was observed | Next action |
| --- | --- | --- |
| `execution-environment-restricted` | Capture access unavailable with a known environment marker such as `CODEX_SANDBOX=seatbelt` | Compare a host-approved invocation outside that sandbox. The marker is advisory and can be missing or stale; it does not prove the kernel cause or a granted TCC permission. |
| `screen-recording-denied` | SCK explicitly returned `SCStreamError.Code.userDeclined` with no recognized sandbox marker | Check the responsible app's Screen Recording access and relaunch it. Responsible-app attribution is not established just by the parent PID. |
| `screen-capture-access-unavailable` | CoreGraphics preflight returned false without sufficient evidence of cause | Compare execution contexts first. Check permissions if the authorized invocation also fails. A Boolean preflight cannot distinguish missing access, denial, attribution, or unknown sandbox restrictions. |
| `gui-session-unavailable` | No logged-in console session or AppKit display visible to this process | Use the authorized interactive session. A sandbox may also hide this information; this is not a TCC-denial conclusion. |
| `gui-thread-required` / `gui-not-initialized` | GUI/filter precondition not satisfied | This is a helper initialization defect, not an instruction to change permissions. |
| `native-capture-crashed` | Supervised native worker terminated by a signal | Public helper returns normal exit 1. Preserve the last stage and crash report; do not broaden capture. |
| `native-startup-timeout` | Native startup did not produce readiness within 16 seconds | Preserve diagnostics and inspect the execution context; do not assume permission denial. |
| `screen-capture-failed` | Other framework failure | Preserve the underlying domain/code; do not relabel every SCK error as userDeclined. |

No public API used here exposes an authoritative global sandbox/TCC verdict. **Unknown
must stay unknown.** Merely setting an environment marker is not a sandbox experiment.
No environment variables are unset to circumvent restrictions.

## Initialization and containment

Live commands initialize `NSApplication.shared`, set a non-activating `.prohibited`
activation policy, and enumerate `NSScreen.screens` on the main thread before creating
a window filter. No window is opened or activated. This is the public AppKit startup
path missing from the original CLI; it is a targeted mitigation, not proof that this
macOS regression is universally fixed. No private `CGS*`/`SLS*` initialization is used.

The actual filter remains `SCContentFilter(desktopIndependentWindow: selectedWindow)`.
The stages before/after AppKit and before/after filter creation include OS version,
main-thread state, AppKit initialization state and the requested window ID. They do not
include the full environment or window contents.

An assertion abort is not a Swift throwable error. The public helper therefore supervises
a worker using the **same executable**, arguments, sandbox, identity and stdio. It converts
worker signals into an ordinary error rather than catching SIGABRT inside the damaged
process. A worker may still generate a macOS crash report. The worker watches its parent's
lifetime so a killed supervisor does not leave recording running, and startup is bounded.
This is containment, not session recovery or a promise that interrupted MP4s are playable.
Synthetic self-tests and image stamping do not enter the live GUI preflight.

## Minimal reproduction: initialize versus do not initialize

Compile the independent reproducer, then run both variants only in an authorized context:

```bash
swiftc tools/repro-window-filter.swift -o /tmp/rec-window-filter-repro
/tmp/rec-window-filter-repro 1455
/tmp/rec-window-filter-repro 1455 --without-appkit
```

The baseline deliberately omits AppKit initialization and **may abort**; it is not used by
the production helper. Both variants run on the main thread, discover the specified window,
construct only that filter, print stage diagnostics, and create no recording. Neither
requests permission. Permission attribution may differ for this separately compiled binary;
a preflight failure is inconclusive, not a valid with/without-AppKit comparison.

If only the baseline aborts with matching access/target context, that supports the missing
initialization hypothesis. If both abort after AppKit initialization succeeds, preserve
both logs and the crash report for an Apple framework regression report. Filter creation
success by itself does not establish first-frame delivery or correct media output.

## Alternatives considered

Apple also exposes display-based inclusion of a selected set of windows. That is not the
same capture geometry/lifetime contract as a desktop-independent window: moving a window
off the selected display removes it from that output. It is not substituted automatically.
An application filter includes other windows and potentially their audio, so an app capture
alternative requires the user's explicit approval of the broader scope. Screen capture is
not a safe fallback for a failed window request.

## Acceptance gates and verification limits

CI checks buildability, diagnostic classification, explicit thread/session guards, a real
subprocess SIGABRT becoming an ordinary supervisor result, stdio/exit preservation, and
existing media/CLI regressions. These are not live ScreenCaptureKit acceptance tests.

On the reported Mac, verify all of the following before closing #5:

1. In the authorized non-sandboxed context, current window discovery and `diagnose --window-id`
   succeed; stage output shows main thread, initialized AppKit and successful filter creation.
2. Record that one window and play the final MP4. Put visibly different content in a second
   window of the same app and another app; confirm neither appears. Confirm first frame,
   Run/Step readability and a successful explicit stop. No auto-selected app/display target.
3. Compare sandboxed and approved execution. Retain the actual diagnostics and markers; do
   not claim a known cause when the available observation is only a false preflight.
4. In a separate controlled permission-denial scenario, check the error category without
   resetting the user's existing TCC database. Record whether SCK or CoreGraphics supplied
   the result; an ambiguous CoreGraphics result must not be labelled a confirmed denial.
5. Native worker failure must be returned to the caller as a normal helper error; no process
   remains recording after its supervisor is terminated. Raw data is never claimed complete.

The authoring environment is Linux, not the reported macOS 26.5.2 interactive session.
Do not equate a green CI run or a synthetic fixture with completion of these gates.

## Primary API references

- [NSApplication.shared](https://developer.apple.com/documentation/appkit/nsapplication/shared)
- [CGPreflightScreenCaptureAccess](https://developer.apple.com/documentation/coregraphics/cgpreflightscreencaptureaccess())
- [SCK userDeclined](https://developer.apple.com/documentation/screencapturekit/scstreamerror/code/userdeclined)
- [Apple WWDC22: window versus display inclusion](https://developer.apple.com/videos/play/wwdc2022/10155/)
