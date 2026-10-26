#!/usr/bin/env bash
# Builds dist/Radlet.app: the Swift shell, the Rust `radlet` helper, the
# `radlet-vision` helper (text recognition, background removal), the
# PDFium library (for PDF-to-image) and Sparkle (for updates).
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
  cargo build --release -p radlet-cli --target aarch64-apple-darwin
  cargo build --release -p radlet-cli --target x86_64-apple-darwin
  mkdir -p target/universal
  lipo -create -output target/universal/radlet \
    target/aarch64-apple-darwin/release/radlet target/x86_64-apple-darwin/release/radlet
  HELPER=target/universal/radlet
  SWIFT_FLAGS=(-c release --arch arm64 --arch x86_64)
  PDFIUM_ARCHES=(arm64 x64)
else
  cargo build --release -p radlet-cli
  HELPER=target/release/radlet
  SWIFT_FLAGS=(-c release)
  if [[ "$(uname -m)" == "arm64" ]]; then PDFIUM_ARCHES=(arm64); else PDFIUM_ARCHES=(x64); fi
fi

# SwiftPM points the linker at XCTest folders that only Xcode has, so with
# just the Command Line Tools every product warns that they're missing.
# It's harmless: drop exactly that warning and keep everything else.
without_clt_noise() { sed -E "/ld: warning: search path '[^']*CommandLineTools\/Developer\/[^']*' not found/d"; }
swift build --package-path mac "${SWIFT_FLAGS[@]}" 2>&1 | without_clt_noise
BIN_DIR="$(swift build --package-path mac "${SWIFT_FLAGS[@]}" --show-bin-path)"

APP="$ROOT/dist/Radlet.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Helpers" "$APP/Contents/Resources/Licenses"
cp mac/Resources/Info.plist "$APP/Contents/Info.plist"
if [[ -n "${VERSION:-}" ]]; then
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" \
    -c "Set :CFBundleVersion $VERSION" "$APP/Contents/Info.plist"
fi
cp "$BIN_DIR/Radlet" "$APP/Contents/MacOS/Radlet"
cp "$HELPER" "$APP/Contents/Helpers/radlet"
cp "$BIN_DIR/radlet-vision" "$APP/Contents/Helpers/radlet-vision"
cp THIRD_PARTY.md "$APP/Contents/Resources/Licenses/"
cp mac/Resources/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

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

# Sparkle (MIT), for updates: SwiftPM unpacks its framework under
# mac/.build/artifacts. Radlet isn't sandboxed, so Sparkle's XPC services
# aren't needed (Sparkle's documentation says they can go).
SPARKLE="$(find mac/.build/artifacts -maxdepth 6 -type d -path '*Sparkle.xcframework/macos-*/Sparkle.framework' | head -1)"
if [[ -z "$SPARKLE" ]]; then
  echo "Sparkle.framework wasn't found under mac/.build/artifacts." >&2
  exit 1
fi
mkdir -p "$APP/Contents/Frameworks"
ditto "$SPARKLE" "$APP/Contents/Frameworks/Sparkle.framework"
rm -rf "$APP/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices" \
  "$APP/Contents/Frameworks/Sparkle.framework/XPCServices"
RPATHS="$(otool -l "$APP/Contents/MacOS/Radlet")"
if ! grep -q '@executable_path/../Frameworks' <<< "$RPATHS"; then
  install_name_tool -add_rpath @executable_path/../Frameworks "$APP/Contents/MacOS/Radlet"
fi
cp mac/Resources/Licenses/Sparkle.txt "$APP/Contents/Resources/Licenses/Sparkle.txt"

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
# The compilers already gave each binary a basic signature, and Sparkle came
# signed by its authors: replacing those is expected. Inside out: Sparkle's
# helpers, then the framework, then the app.
sign() { codesign --force "${SIGN_FLAGS[@]}" --sign "$IDENTITY" "$@" 2>&1 | sed '/: replacing existing signature$/d'; }
sign "$APP/Contents/Helpers/"*
FRAMEWORK="$APP/Contents/Frameworks/Sparkle.framework"
sign "$FRAMEWORK/Versions/B/Autoupdate" "$FRAMEWORK/Versions/B/Updater.app"
sign "$FRAMEWORK"
sign --entitlements mac/Resources/Radlet.entitlements "$APP"
codesign --verify --deep --strict "$APP"
echo "Built $APP"
