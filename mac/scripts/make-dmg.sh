#!/usr/bin/env bash
# Builds Radlet.app and packs it for the Releases page:
#   dist/Radlet-<version>.dmg      open it and drag Radlet to Applications
#   dist/Radlet-<version>.zip      the same app, zipped
#   dist/Radlet-<version>.sha256   checksums of both
#
#   mac/scripts/make-dmg.sh                  this Mac's architecture
#   UNIVERSAL=1 mac/scripts/make-dmg.sh      Apple silicon + Intel
#   VERSION=0.2.0 mac/scripts/make-dmg.sh    stamp a version (else Info.plist's)
#   SKIP_BUILD=1 mac/scripts/make-dmg.sh     pack the dist/Radlet.app already built
#
# Other build-app.sh options (SIGN_IDENTITY, FFMPEG_DIR, ...) pass through.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  bash mac/scripts/build-app.sh
fi

APP=dist/Radlet.app
if [[ ! -d "$APP" ]]; then
  echo "There's no $APP to pack. Build it first, or leave out SKIP_BUILD." >&2
  exit 1
fi
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
NAME="Radlet-$VERSION"

rm -f "dist/$NAME.dmg" "dist/$NAME.zip" "dist/$NAME.sha256"

# The styled window (background with an arrow, icon places, Radlet's icon on
# the disk) comes from dmgbuild (MIT), kept in its own Python environment
# under target/. Without Python 3 or a network the image is plain instead.
DMGBUILD="${DMGBUILD:-}"
if [[ -z "$DMGBUILD" ]]; then
  VENV=target/dmgbuild
  if [[ ! -x "$VENV/bin/dmgbuild" ]] && command -v python3 > /dev/null; then
    python3 -m venv "$VENV" > /dev/null 2>&1 && "$VENV/bin/pip" install --quiet "dmgbuild==1.6.7" > /dev/null 2>&1 || true
  fi
  if [[ -x "$VENV/bin/dmgbuild" ]]; then DMGBUILD="$VENV/bin/dmgbuild"; fi
fi

if [[ -n "$DMGBUILD" ]]; then
  "$DMGBUILD" -s mac/dmg/settings.py -D app="$APP" -D dir="$ROOT/mac/dmg" "Radlet $VERSION" "dist/$NAME.dmg"
else
  echo "dmgbuild isn't available, so the disk image has no styled window." >&2
  STAGE="$(mktemp -d)"
  trap 'rm -rf "$STAGE"' EXIT
  ditto "$APP" "$STAGE/Radlet.app"
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -volname "Radlet $VERSION" -srcfolder "$STAGE" -format UDZO -ov "dist/$NAME.dmg"
fi
ditto -c -k --keepParent "$APP" "dist/$NAME.zip"
(cd dist && shasum -a 256 "$NAME.dmg" "$NAME.zip" > "$NAME.sha256")

echo "Packed:"
cat "dist/$NAME.sha256"
