# macOS distribution

## User installation

The distribution supports Apple Silicon Macs only, with macOS 14 or later.
It is a Developer ID signed and notarized DMG containing
`Agent Work Recorder.app` and an Applications shortcut. Users drag the app to
Applications, launch it, click the command setup button, and restart their
terminal or agent. The setup app explains the screen-recording permission for
the terminal/agent host; it does not grant permissions or start a recording.

The app contains `rec`, `rec-capture`, and private FFmpeg/FFprobe executables
and shared libraries. It needs no Homebrew or compiler at runtime. The CLI
resolves its real executable path before finding private media tools, including
when invoked through the installed `~/.local/bin/rec` symlink. This does not
alter PATH or the environment of commands executed by `rec test`.

The setup button creates per-user command symlinks and appends a PATH line to
`.zprofile` and `.bash_profile`. Existing commands and profiles are backed up
with a `.rec-backup-<UUID>` suffix. Repeated setup is idempotent. Symlinked shell
profiles are reported rather than replaced. Launching directly from the DMG
does not enable command setup, avoiding links to a temporary mounted volume.

Agent hosts that do not read shell profiles should use the full CLI path:
`/Applications/Agent Work Recorder.app/Contents/Resources/bin/rec`.
The portable agent skill is included under `Contents/Resources/agent-work-recorder`.

For upgrades, quit the setup app, finish active recording Runs, replace the app
at the same path, and run command setup again. To uninstall, remove only the
command symlinks pointing to this app, remove the marked PATH entry if desired,
and move the app to Trash. Recording outputs, session recovery evidence, and
backups are not removed by setup or app replacement.

## Build a development DMG

```sh
make package
```

Build requirements: macOS 14+, Xcode/Swift, Rust, Python 3, and normal macOS
command-line tools. The initial build downloads the pinned FFmpeg source over
HTTPS and verifies its SHA-256. Subsequent builds reuse a script-keyed cache in
`build/media-arm64`. Build on an Apple Silicon Mac; other architectures are
rejected. macOS 14.0 is the deployment target.

Development builds are ad-hoc signed and clearly named
`dist/Agent-Work-Recorder-<version>-macos-arm64-DEVELOPMENT-NOT-NOTARIZED.dmg`.
They are not public release artifacts. The distribution workflow builds and
checks the Apple Silicon package as a CI artifact; it has no signing credentials and
does not publish a GitHub release.

## Build a signed, notarized DMG

Use an existing **Developer ID Application** identity and `notarytool` Keychain
profile. The DMG/app path does not require a Developer ID Installer certificate.
Keep all private keys and credentials outside the repository.

```sh
SIGN_IDENTITY='Developer ID Application: Your Name (TEAMID)' \
NOTARY_PROFILE='your-existing-profile' \
make release-package
```

The script builds and checks the bundle, signs executables/libraries with
hardened runtime and secure timestamps, notarizes and staples the app, creates
the DMG, then signs, notarizes and staples the DMG. It validates stapling and
Gatekeeper assessment before producing the final filename and SHA-256 file.
An unsuccessful candidate keeps its candidate name. Existing final releases
are never overwritten. Preserve older outputs before rebuilding the same version.

Notarization is an upload to Apple of the application code and bundled source;
user recordings, diagnostic sessions and credentials are excluded. Public
GitHub publication is a separate action using the final accepted DMG and checksum.

## Bundled media and license

FFmpeg 8.0.3 is built from the official source archive with GPL/nonfree/version-3
components and external autodetection disabled. Only local H.264/AAC MP4
inspection and chapter/metadata remux are enabled; recording uses the native
AVFoundation encoder. No external Homebrew dylib paths are retained.

FFmpeg runs as separate executables with shared libraries. The app includes
its LGPL 2.1 license, unmodified corresponding source archive, configuration,
and reproducible build script under `Contents/Resources/licenses`. Agent Work
Recorder remains MIT licensed. The locked Rust dependency graph and its license
texts are included separately under `licenses/rust`. See [FFmpeg license information](https://ffmpeg.org/legal.html).

## Verification boundaries

```sh
python3 tests/check_macos_package.py '/path/to/Agent Work Recorder.app'
```

This check uses an isolated HOME and PATH without Homebrew. It verifies backup
preservation, repeated setup, zsh/bash command discovery, private dylib loading,
and the actual packaged CLI/daemon/finalization path with H.264, AAC and chapters.
The capture fixture uses the real native writer with synthetic pixels/audio;
it does not establish screen permission or live capture correctness.

Release acceptance also includes launching the exact packaged GUI, checking
the install-location guidance, assessing the app and DMG with Gatekeeper, and
validating stapled tickets. macOS 14 runtime and fresh-user permission flows
must be stated separately when not run.

## Local installation evidence (2026-10-01)

The signed, notarized Apple Silicon 0.1.0 DMG was installed into Applications
on the development Mac running macOS 26.6.2. The actual setup button registered
the commands, and a new login shell resolved the installed `rec` symlink.
Gatekeeper accepted the installed app as Notarized Developer ID.

A live app capture of the installed setup app produced a 42.817-second H.264
MP4 at 3456 × 2234. Reviewed frames showed the permission guidance dialog
opening and closing, and the larger bottom annotations. The packaged media
tools preserved the test and checkpoint chapters during finalization.
The recorder reported no active recording after stop. Recordings remain local
and are excluded from Git.

This run reused existing screen-recording permission and kept audio off.
Fresh-user permission onboarding, macOS 14 runtime, and live system audio
were not verified by this installation run.
