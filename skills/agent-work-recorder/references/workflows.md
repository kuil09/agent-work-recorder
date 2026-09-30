# Recording workflows

Read the installed command help first. These examples require permission and a selected,
running capture target. Replace the sample app/project/check with the authorized task.
Never run all target alternatives as consecutive `start` commands.

## Interactive agent task

```bash
rec start --title "Login layout review" --app com.apple.Safari
rec note "Inspecting the login error layout"
# Give the viewer time to read; perform the relevant work with separate tools.
rec expect "Invalid credentials place an error below the login button"
# Perform the actual UI action, then inspect its result before recording a verdict.
```

Do not paste a prewritten PASS. Record the observed result in your own words, use
uncertain when evidence is missing, then mark the review scene and stop:

```bash
rec observe --status uncertain "The spinner did not settle; success is unconfirmed"
# Leave the observation readable before the next event replaces its card.
rec checkpoint "Login response requiring human review"
rec stop
```

## Failure-aware test-only recording

This pattern handles an ordinary nonzero test result under `set -e`; it is not a
recovery daemon, signal-proof supervisor, or guarantee against a machine crash.
It finalizes only after its own start succeeds. Do not run concurrent session-changing
automation. Check Run ownership before manual cleanup following an interruption.

```bash
#!/bin/sh
set -eu
rec start --title "Project tests" --app com.apple.Terminal || exit 1

test_rc=0
rec test --timeout-secs 60 -- cargo test || test_rc=$?

stop_rc=0
rec stop || stop_rc=$?

# Recorder failure takes priority; do not report a usable video without finalization.
if [ "$stop_rc" -ne 0 ]; then
    printf '%s\n' "Recording finalization failed; retain diagnostics and raw files" >&2
    exit "$stop_rc"
fi
exit "$test_rc"
```

This records a mechanical command result, not a UI verdict. The runner forwards output
to its invoking terminal; it cannot guarantee that this terminal is the selected window.
Use `note`/`expect` and actual visual inspection for a UI verification workflow.

Do not use `rec test ... && rec stop`: failure would skip finalization. Do not turn
an unguarded test into an unconditional PASS. Do not automatically repeat a failed
recorder request when its child may already have run.

## Argument forwarding

```bash
rec test npm test -- auth.test.ts
rec test --timeout-secs 60 -- cargo test
rec test -- sh -c 'printf "diagnostic\n"; exit 7'
```

`rec test --help` displays recorder help without starting a command.
`rec test npm --help` executes `npm --help` inside the active Run.
`rec test cargo test --timeout-secs 60` forwards the timeout flag to cargo; it does not
set the recorder timeout. Prefer argv arrays rather than dynamically constructed shell strings.

## Feedback round trip

Given screenshot `7F32:021` and feedback "increase the gap below the button":

1. Inspect the actual screenshot; the ID references an event, not an exact replayable frame.
2. Make the authorized change and start a fresh Run using an appropriate target.
3. Record `rec note "Addressing feedback on 7F32:021: increase error spacing"`.
4. State the criterion, reproduce the UI state, inspect it and record the observation.
5. Mark the final scene, stop, and give the human the new MP4 with the old/new references.

Keep prior evidence intact. A later recording does not retroactively prove an earlier claim.

## Handoff template

```text
Recording: <actual path printed by successful stop>
Run: <actual Run ID>
Review points: <actual Run:Step identifiers and short scene labels>
Command results: <commands, exit statuses, timeout or recorder errors>
Observed: <what was actually inspected; agent claims remain claims>
Not verified: <playback/audio/live capture or other unchecked behavior>
```

Use only observed values. Deliver the MP4; internal state files are not required human
reports. No network upload is part of this workflow by default.
