"""Relocate private FFmpeg dylibs and create the macOS bundle metadata."""
import pathlib
import json
import os
import plistlib
import shutil
import subprocess
import sys

app = pathlib.Path(sys.argv[1])
resources = app / "Contents/Resources"
tools = resources / "bin"
libraries = resources / "lib"

# Include dependency attributions from the exact locked target dependency graph.
if sys.argv[3] != "arm64":
    raise SystemExit("Only Apple Silicon (arm64) bundles are supported.")
rust_target = "aarch64-apple-darwin"
rustc = subprocess.check_output(["rustup", "which", "--toolchain", "stable", "rustc"], text=True).strip()
metadata = json.loads(subprocess.check_output(
    ["rustup", "run", "stable", "cargo", "metadata", "--locked", "--offline",
     "--format-version", "1", "--filter-platform", rust_target],
    env=dict(os.environ, RUSTC=rustc), text=True,
))
attributions = []
for package in metadata["packages"]:
    if not package.get("source"):
        continue
    directory = pathlib.Path(package["manifest_path"]).parent
    license_files = [item for item in directory.iterdir() if item.is_file()
                     and item.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT"))]
    if not license_files:
        raise SystemExit(f"Missing dependency license: {package['name']}")
    destination = resources / "licenses/rust" / f"{package['name']}-{package['version']}"
    destination.mkdir(parents=True)
    for item in license_files:
        shutil.copy2(item, destination / item.name)
    attributions.append({"name": package["name"], "version": package["version"], "license": package["license"]})
(resources / "licenses/rust-dependencies.json").write_text(json.dumps(attributions, indent=2) + "\n")
for binary in [*tools.iterdir(), *libraries.iterdir()]:
    if binary.is_symlink():
        continue
    output = subprocess.check_output(["otool", "-L", str(binary)], text=True)
    for line in output.splitlines()[1:]:
        dependency = line.strip().split(" (", 1)[0]
        if dependency.startswith(("/System/Library/", "/usr/lib/")):
            continue
        if binary.parent == libraries and pathlib.Path(dependency).name == binary.name:
            continue
        name = pathlib.Path(dependency).name
        if not (libraries / name).exists():
            raise SystemExit(f"Unbundled dependency: {binary.name}: {dependency}")
        relative = "@loader_path/" if binary.parent == libraries else "@executable_path/../lib/"
        subprocess.run(["install_name_tool", "-change", dependency, relative + name, str(binary)], check=True)
    if binary.parent == libraries:
        subprocess.run(["install_name_tool", "-id", "@loader_path/" + binary.name, str(binary)], check=True)

with (app / "Contents/Info.plist").open("wb") as stream:
    plistlib.dump({
        "CFBundleIdentifier": "io.github.kuil09.agent-work-recorder",
        "CFBundleName": "Agent Work Recorder",
        "CFBundleDisplayName": "Agent Work Recorder",
        "CFBundleExecutable": "RecorderSetup",
        "CFBundleIconFile": "Recorder.icns",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": sys.argv[2],
        "CFBundleVersion": sys.argv[2],
        "LSMinimumSystemVersion": "14.0",
        "LSArchitecturePriority": [sys.argv[3]],
        "NSHighResolutionCapable": True,
        "NSHumanReadableCopyright": "2026 kuil09. FFmpeg is licensed under LGPL 2.1 or later.",
    }, stream)

(resources / "licenses/NOTICE.txt").write_text(
    "Agent Work Recorder is MIT licensed. FFmpeg 8.0.3 is included under LGPL 2.1 or later.\n"
    "FFmpeg is a separate executable with dynamically linked libraries. No GPL or nonfree\n"
    "components are enabled. The corresponding unmodified source archive, license,\n"
    "configuration, and build script are included in this directory.\n"
    "Source: https://ffmpeg.org/releases/ffmpeg-8.0.3.tar.xz\n"
    "Rebuild on macOS using the included build-media.sh from the project source tree.\n"
    "These media tools support local H.264/AAC MP4 remux and inspection only.\n",
    encoding="utf-8",
)
