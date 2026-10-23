#!/usr/bin/env bash
# Builds dist/Veer.app: the Swift shell, the Rust `veer` helper, the
# `veer-vision` helper (text recognition, background removal) and the
# PDFium library (for PDF-to-image).
#
#   mac/scripts/build-app.sh                 # this Mac's architecture, ad-hoc signed
#   UNIVERSAL=1 mac/scripts/build-app.sh     # Apple silicon + Intel
#   VERSION=0.2.0 mac/scripts/build-app.sh   # stamp a version into Info.plist
#   SIGN_IDENTITY="Developer ID Application: …" mac/scripts/build-app.sh
#   FFMPEG_DIR=/path/to/lgpl-ffmpeg mac/scripts/build-app.sh   # bundle FFmpeg
#   SKIP_PDFIUM=1 mac/scripts/build-app.sh   # leave out PDF-to-image
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# PDFium (BSD-3-Clause) as built by bblanchon/pdfium-binaries (MIT).
# Pinned for reproducible builds; cached under target/pdfium.
PDFIUM_RELEASE="${PDFIUM_RELEASE:-chromium/8111}"

fetch_pdfium() {
  local arch="$1" dir="target/pdfium/$1"
  if [[ ! -f "$dir/lib/libpdfium.dylib" ]]; then
    mkdir -p "$dir"
    local url="https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_RELEASE/\//%2F}/pdfium-mac-$arch.tgz"
    echo "Downloading PDFium ($arch)…"
    curl -fsSL "$url" | tar xz -C "$dir"
  fi
}

if [[ "${UNIVERSAL:-0}" == "1" ]]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  cargo build --release -p veer-cli --target aarch64-apple-darwin
  cargo build --release -p veer-cli --target x86_64-apple-darwin
  mkdir -p target/universal
  lipo -create -output target/universal/veer \
    target/aarch64-apple-darwin/release/veer target/x86_64-apple-darwin/release/veer
  HELPER=target/universal/veer
  SWIFT_FLAGS=(-c release --arch arm64 --arch x86_64)
  PDFIUM_ARCHES=(arm64 x64)
else
  cargo build --release -p veer-cli
  HELPER=target/release/veer
  SWIFT_FLAGS=(-c release)
  if [[ "$(uname -m)" == "arm64" ]]; then PDFIUM_ARCHES=(arm64); else PDFIUM_ARCHES=(x64); fi
fi

swift build --package-path mac "${SWIFT_FLAGS[@]}"
BIN_DIR="$(swift build --package-path mac "${SWIFT_FLAGS[@]}" --show-bin-path)"

APP="$ROOT/dist/Veer.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Helpers" "$APP/Contents/Resources/Licenses"
cp mac/Resources/Info.plist "$APP/Contents/Info.plist"
if [[ -n "${VERSION:-}" ]]; then
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" \
    -c "Set :CFBundleVersion $VERSION" "$APP/Contents/Info.plist"
fi
cp "$BIN_DIR/Veer" "$APP/Contents/MacOS/Veer"
cp "$HELPER" "$APP/Contents/Helpers/veer"
cp "$BIN_DIR/veer-vision" "$APP/Contents/Helpers/veer-vision"
cp THIRD_PARTY.md "$APP/Contents/Resources/Licenses/"

if [[ "${SKIP_PDFIUM:-0}" != "1" ]]; then
  libs=()
  for arch in "${PDFIUM_ARCHES[@]}"; do
    fetch_pdfium "$arch"
    libs+=("target/pdfium/$arch/lib/libpdfium.dylib")
  done
  if [[ ${#libs[@]} -gt 1 ]]; then
    lipo -create -output "$APP/Contents/Helpers/libpdfium.dylib" "${libs[@]}"
  else
    cp "${libs[0]}" "$APP/Contents/Helpers/libpdfium.dylib"
  fi
  mkdir -p "$APP/Contents/Resources/Licenses/PDFium"
  cp -R "target/pdfium/${PDFIUM_ARCHES[0]}/licenses/." "$APP/Contents/Resources/Licenses/PDFium/"
  cp "target/pdfium/${PDFIUM_ARCHES[0]}/LICENSE" "$APP/Contents/Resources/Licenses/PDFium/pdfium-binaries.txt"
fi

if [[ -n "${FFMPEG_DIR:-}" ]]; then
  # Must be an LGPL build, dynamically linked. See THIRD_PARTY.md.
  cp "$FFMPEG_DIR/ffmpeg" "$FFMPEG_DIR/ffprobe" "$APP/Contents/Helpers/"
fi

IDENTITY="${SIGN_IDENTITY:--}"
if [[ "$IDENTITY" == "-" ]]; then
  # Local builds: ad-hoc signature, no hardened runtime (its library
  # validation would refuse the ad-hoc-signed PDFium library).
  SIGN_FLAGS=(--timestamp=none)
else
  # Release builds: hardened runtime and a secure timestamp, as
  # notarisation requires. Everything is signed by the same team.
  SIGN_FLAGS=(--options runtime --timestamp)
fi
codesign --force "${SIGN_FLAGS[@]}" --sign "$IDENTITY" "$APP/Contents/Helpers/"*
codesign --force "${SIGN_FLAGS[@]}" --entitlements mac/Resources/Veer.entitlements --sign "$IDENTITY" "$APP"
codesign --verify --strict "$APP"
echo "Built $APP"
