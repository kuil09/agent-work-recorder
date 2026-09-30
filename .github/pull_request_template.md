## Summary

## Verification
- [ ] `make test`
- [ ] Automated results are synthetic (fault injection / synthetic capture) unless stated otherwise

## Real-host evidence (required before closing a bug that depends on macOS TCC/sandbox behavior)
Do not use `Fixes`/`Closes` for such an issue until the same authorized execution context has
run `rec-capture list-windows`, `rec-capture diagnose --window-id ID`, a real recording and
playback. Paste the evidence as an issue comment, or leave the issue open with a
"needs host verification" note.
