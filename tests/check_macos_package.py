#!/usr/bin/env python3
"""Check a real bundle with isolated HOME and no Homebrew on PATH.

The setup, shared-library loading and CLI/daemon/media finalization are real.
Capture is a synthetic native-writer fixture; this does not test screen permission.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def run(argv, env=None):
    result = subprocess.run([str(arg) for arg in argv], env=env, text=True,
                            capture_output=True, timeout=60)
    assert result.returncode == 0, (argv, result.stdout, result.stderr)
    return result.stdout


if len(sys.argv) > 1 and sys.argv[1] == "--capture-helper":
    args = sys.argv[2:]
    output = args[args.index("--output") + 1]
    print(json.dumps({"event": "ready", "elapsed_ms": 0}), flush=True)
    for line in sys.stdin:
        if json.loads(line)["cmd"] == "stop":
            command = [os.environ["REC_PACKAGE_NATIVE"], "self-test", "--output", output]
            if "--system-audio" in args:
                command.append("--system-audio")
            run(command)
            print(json.dumps({"event": "stopped", "path": output}), flush=True)
            sys.exit(0)
    sys.exit(1)

app = Path(sys.argv[1]).resolve()
tools = app / "Contents/Resources/bin"
setup = app / "Contents/MacOS/RecorderSetup"
assert (app / "Contents/Resources/licenses/ffmpeg-8.0.3.tar.xz").is_file()
assert (app / "Contents/Resources/licenses/rust-dependencies.json").is_file()
with tempfile.TemporaryDirectory(prefix="rec-package-check-", dir="/private/tmp") as directory:
    home = Path(directory) / "home with spaces"
    home.mkdir()
    bin_dir = home / ".local/bin"
    bin_dir.mkdir(parents=True)
    (bin_dir / "rec").write_text("old command\n")
    (bin_dir / "rec-capture").symlink_to("/old/missing-helper")
    original = "# User configuration\nexport REC_PACKAGE_FIXTURE=preserved\n"
    (home / ".zprofile").write_text(original)
    (home / ".bash_profile").write_text(original)
    env = dict(os.environ, HOME=str(home), PATH="/usr/bin:/bin")
    run([setup, "--check-setup", home, app], env)
    backups = sorted(home.rglob("*.rec-backup-*"))
    assert len(backups) == 4, backups
    assert any(p.is_file() and p.read_text() == "old command\n" for p in backups)
    assert any(p.is_symlink() and os.readlink(p) == "/old/missing-helper" for p in backups)
    run([setup, "--check-setup", home, app], env)
    assert sorted(home.rglob("*.rec-backup-*")) == backups
    for profile in (home / ".zprofile", home / ".bash_profile"):
        text = profile.read_text()
        assert text.startswith(original) and text.count("# Agent Work Recorder commands") == 1
    assert (bin_dir / "rec").resolve() == tools / "rec"
    assert run(["/bin/zsh", "-l", "-c", "command -v rec"], env).strip() == str(bin_dir / "rec")
    assert run(["/bin/bash", "--login", "-c", "command -v rec"], env).strip() == str(bin_dir / "rec")
    run([bin_dir / "rec", "--help"], env)
    for name in ("ffmpeg", "ffprobe"):
        run([tools / name, "-version"], env)

    temporary = home / "tmp"
    temporary.mkdir()
    helper = home / "capture-helper"
    # JSON quoting produces safe Python literals for paths with spaces.
    helper.write_text("#!/usr/bin/python3\nimport os\nos.execv(" + repr(sys.executable) +
                      ", [" + repr(sys.executable) + ", " + repr(str(Path(__file__).resolve())) +
                      ", '--capture-helper', *__import__('sys').argv[1:]])\n")
    helper.chmod(0o700)
    env.update(TMPDIR=str(temporary), REC_CAPTURE=str(helper),
               REC_PACKAGE_NATIVE=str(tools / "rec-capture"))
    output = home / "review.mp4"
    run([bin_dir / "rec", "start", "--app", "fixture", "--system-audio",
         "--no-git-context", "--output", output], env)
    try:
        run([bin_dir / "rec", "checkpoint", "Package media verification"], env)
        run([bin_dir / "rec", "stop"], env)
    finally:
        # Only this temporary HOME is touched, even on a failed assertion.
        subprocess.run([str(bin_dir / "rec"), "stop", "--abandon-test"], env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    info = json.loads(run([tools / "ffprobe", "-v", "error", "-show_streams",
                           "-show_chapters", "-of", "json", output], env))
    assert {stream["codec_name"] for stream in info["streams"]} >= {"h264", "aac"}
    assert any("Package media verification" in chapter["tags"]["title"]
               for chapter in info["chapters"])
print("PASS: CLI setup, backup, repeat setup, zsh/bash PATH, private dylibs, H.264/AAC and chapters without Homebrew")
