#!/bin/bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
VERSION=8.0.3
SHA256=6136812ea6d4e68bdba27e33c2a94382711cdf4f8602ffef056ff792bd6f9818
ARCH=arm64
if [[ "$(uname -m)" != arm64 || "${TARGET_ARCH:-arm64}" != arm64 ]]; then
  printf 'Only Apple Silicon (arm64) builds are supported.\n' >&2
  exit 1
fi
WORK="$ROOT/build/media-$ARCH"
SOURCE="$WORK/ffmpeg-$VERSION"
PREFIX="$WORK/install"
ARCHIVE="$WORK/ffmpeg-$VERSION.tar.xz"
mkdir -p "$WORK"
if [[ ! -f "$ARCHIVE" ]]; then
  curl -fsSL --retry 2 "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz" -o "$ARCHIVE.download"
  mv "$ARCHIVE.download" "$ARCHIVE"
fi
printf '%s  %s\n' "$SHA256" "$ARCHIVE" | shasum -a 256 -c -
BUILD_KEY="$(shasum -a 256 "$0" | cut -d ' ' -f 1)-$ARCH"
if [[ -f "$PREFIX/build-key" && "$(cat "$PREFIX/build-key")" == "$BUILD_KEY" ]]; then
  printf 'Using cached media tools: %s\n' "$PREFIX"
  exit 0
fi
mkdir -p "$SOURCE"
tar -xf "$ARCHIVE" -C "$SOURCE" --strip-components=1
cd "$SOURCE"
make distclean >/dev/null 2>&1 || true
ARCH_ARGS=(--arch="$ARCH")
./configure --prefix="$PREFIX" --cc=clang "${ARCH_ARGS[@]}" \
  --extra-cflags="-arch $ARCH -mmacosx-version-min=14.0" \
  --extra-ldflags="-arch $ARCH -mmacosx-version-min=14.0 -Wl,-headerpad_max_install_names" \
  --disable-autodetect --disable-gpl --disable-nonfree --disable-version3 \
  --enable-shared --disable-static --disable-doc --disable-debug \
  --disable-network --disable-x86asm --disable-everything \
  --enable-ffmpeg --enable-ffprobe --disable-ffplay \
  --enable-protocol=file --enable-demuxer=mov,ffmetadata --enable-muxer=mp4 \
  --enable-parser=h264,aac --enable-decoder=h264,aac \
  --enable-filter=buffer,abuffer,buffersink,abuffersink,null,anull \
  --enable-bsf=aac_adtstoasc,h264_mp4toannexb > "$WORK/configure.log"
make -j "${BUILD_JOBS:-4}" > "$WORK/compile.log" 2>&1
make install > "$WORK/install.log" 2>&1
mkdir -p "$PREFIX/licenses"
cp COPYING.LGPLv2.1 "$PREFIX/licenses/FFmpeg-LGPL-2.1.txt"
cp config.h "$PREFIX/licenses/ffmpeg-config.h"
cp "$ARCHIVE" "$PREFIX/licenses/"
cp "$ROOT/scripts/build-media.sh" "$PREFIX/licenses/"
printf '%s\n' "$BUILD_KEY" > "$PREFIX/build-key"
printf 'Built media tools: %s\n' "$PREFIX"
