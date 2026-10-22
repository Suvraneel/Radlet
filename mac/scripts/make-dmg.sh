#!/usr/bin/env bash
# Builds Veer.app and packs it for the Releases page:
#   dist/Veer-<version>.dmg      open it and drag Veer to Applications
#   dist/Veer-<version>.zip      the same app, zipped
#   dist/Veer-<version>.sha256   checksums of both
#
#   mac/scripts/make-dmg.sh                  this Mac's architecture
#   UNIVERSAL=1 mac/scripts/make-dmg.sh      Apple silicon + Intel
#   VERSION=0.2.0 mac/scripts/make-dmg.sh    stamp a version (else Info.plist's)
#   SKIP_BUILD=1 mac/scripts/make-dmg.sh     pack the dist/Veer.app already built
#
# Other build-app.sh options (SIGN_IDENTITY, FFMPEG_DIR, ...) pass through.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  bash mac/scripts/build-app.sh
fi

APP=dist/Veer.app
if [[ ! -d "$APP" ]]; then
  echo "There's no $APP to pack. Build it first, or leave out SKIP_BUILD." >&2
  exit 1
fi
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
NAME="Veer-$VERSION"

# The disk image shows the app next to a link to Applications.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/Veer.app"
ln -s /Applications "$STAGE/Applications"

rm -f "dist/$NAME.dmg" "dist/$NAME.zip" "dist/$NAME.sha256"
hdiutil create -volname "Veer $VERSION" -srcfolder "$STAGE" -format UDZO -ov "dist/$NAME.dmg"
ditto -c -k --keepParent "$APP" "dist/$NAME.zip"
(cd dist && shasum -a 256 "$NAME.dmg" "$NAME.zip" > "$NAME.sha256")

echo "Packed:"
cat "dist/$NAME.sha256"
