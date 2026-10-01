#!/bin/bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
ARCH=arm64
RUST_TARGET=aarch64-apple-darwin
if [[ "$(uname -m)" != arm64 || "${TARGET_ARCH:-arm64}" != arm64 ]]; then
  printf 'Only Apple Silicon (arm64) builds are supported.\n' >&2
  exit 1
fi
export TARGET_ARCH="$ARCH"
VERSION=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
MODE="${1:-development}"
case "$MODE" in development|release) ;; *) printf 'Usage: %s [development|release]\n' "$0" >&2; exit 1 ;; esac
if [[ "$MODE" == release ]]; then
  : "${SIGN_IDENTITY:?Set a Developer ID Application identity}"
  : "${NOTARY_PROFILE:?Set an existing notarytool Keychain profile}"
  [[ ! -e "$ROOT/dist/Agent-Work-Recorder-$VERSION-macos-$ARCH.dmg" ]] || {
    printf 'Release output already exists; preserve it before creating a new candidate.\n' >&2; exit 1;
  }
fi
export MACOSX_DEPLOYMENT_TARGET=14.0
if ! rustup run stable rustc --version >/dev/null 2>&1; then
  rustup toolchain install stable --profile minimal
fi
rustup target add --toolchain stable "$RUST_TARGET"
RUSTC_PATH=$(rustup which --toolchain stable rustc)
RUSTC="$RUSTC_PATH" rustup run stable cargo build --release --locked --target "$RUST_TARGET"
swift build -c release --triple "$ARCH-apple-macosx14.0" --package-path macos/RecCapture
CAPTURE_DIR=$(swift build -c release --triple "$ARCH-apple-macosx14.0" --package-path macos/RecCapture --show-bin-path)
bash scripts/build-media.sh
WORK=$(mktemp -d "$ROOT/build/package-$ARCH.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/Agent Work Recorder.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin" "$APP/Contents/Resources/lib"
swiftc -target "$ARCH-apple-macosx14.0" -O packaging/RecorderSetup.swift -o "$APP/Contents/MacOS/RecorderSetup"
swift packaging/RenderIcon.swift "$WORK/Recorder.iconset"
iconutil -c icns "$WORK/Recorder.iconset" -o "$APP/Contents/Resources/Recorder.icns"
rm -rf "$WORK/Recorder.iconset"
cp "target/$RUST_TARGET/release/rec" "$CAPTURE_DIR/rec-capture" "$APP/Contents/Resources/bin/"
cp build/media-"$ARCH"/install/bin/ffmpeg build/media-"$ARCH"/install/bin/ffprobe "$APP/Contents/Resources/bin/"
cp -R build/media-"$ARCH"/install/lib/*.dylib "$APP/Contents/Resources/lib/"
cp -R build/media-"$ARCH"/install/licenses "$APP/Contents/Resources/"
cp LICENSE "$APP/Contents/Resources/LICENSE.txt"
cp -R skills/agent-work-recorder "$APP/Contents/Resources/"
python3 scripts/prepare-bundle.py "$APP" "$VERSION" "$ARCH"
for executable in "$APP/Contents/Resources/bin/"*; do chmod 755 "$executable"; done
env PATH=/usr/bin:/bin "$APP/Contents/Resources/bin/rec" --version
env PATH=/usr/bin:/bin "$APP/Contents/Resources/bin/ffmpeg" -version >/dev/null
env PATH=/usr/bin:/bin "$APP/Contents/Resources/bin/ffprobe" -version >/dev/null
"$APP/Contents/Resources/bin/rec-capture" --help >/dev/null
IDENTITY="${SIGN_IDENTITY:--}"
SIGN_ARGS=(--force --sign "$IDENTITY" --options runtime)
if [[ "$MODE" == release ]]; then SIGN_ARGS+=(--timestamp); fi
for library in "$APP/Contents/Resources/lib/"*; do
  [[ -L "$library" ]] && continue
  codesign "${SIGN_ARGS[@]}" "$library"
done
for executable in "$APP/Contents/Resources/bin/"*; do
  codesign "${SIGN_ARGS[@]}" --identifier "io.github.kuil09.agent-work-recorder.$(basename "$executable")" "$executable"
done
codesign "${SIGN_ARGS[@]}" "$APP"
codesign --verify --deep --strict "$APP"
python3 tests/check_macos_package.py "$APP"
mkdir -p dist
if [[ "$MODE" == release ]]; then
  ditto -c -k --keepParent "$APP" "$WORK/app.zip"
  APP_REPORT="$ROOT/dist/Agent-Work-Recorder-$VERSION-macos-$ARCH.app.notary.json"
  xcrun notarytool submit "$WORK/app.zip" --keychain-profile "$NOTARY_PROFILE" --wait --timeout 20m --output-format json > "$APP_REPORT"
  python3 -c 'import json,sys; assert json.load(open(sys.argv[1]))["status"] == "Accepted", "App notarization not accepted"' "$APP_REPORT"
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  rm "$WORK/app.zip"
fi
if [[ "$MODE" == development ]]; then SUFFIX=-DEVELOPMENT-NOT-NOTARIZED; else SUFFIX=-candidate; fi
DMG="$ROOT/dist/Agent-Work-Recorder-$VERSION-macos-$ARCH$SUFFIX.dmg"
[[ ! -e "$DMG" ]] || { printf 'Output already exists: %s\n' "$DMG" >&2; exit 1; }
ln -s /Applications "$WORK/Applications"
hdiutil create -volname 'Agent Work Recorder' -srcfolder "$WORK" -ov -format UDZO "$DMG"
if [[ "$MODE" == release ]]; then
  codesign --force --sign "$SIGN_IDENTITY" --timestamp "$DMG"
  xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait --timeout 20m --output-format json > "$DMG.notary.json"
  python3 -c 'import json,sys; assert json.load(open(sys.argv[1]))["status"] == "Accepted", "Notarization not accepted"' "$DMG.notary.json"
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
  FINAL="$ROOT/dist/Agent-Work-Recorder-$VERSION-macos-$ARCH.dmg"
  [[ ! -e "$FINAL" ]] || { printf 'Output already exists: %s\n' "$FINAL" >&2; exit 1; }
  mv "$DMG" "$FINAL"
  mv "$DMG.notary.json" "$FINAL.notary.json"
  DMG="$FINAL"
fi
(cd "$(dirname "$DMG")" && shasum -a 256 "$(basename "$DMG")") > "$DMG.sha256"
printf 'Created: %s\n' "$DMG"
