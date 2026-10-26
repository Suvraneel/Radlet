#!/usr/bin/env bash
# Notarizes a Developer ID build packed by make-dmg.sh, then staples the
# ticket to the disk image and the app and repacks the zip and checksums,
# so both open without a Gatekeeper warning, even offline.
#
#   NOTARY_KEY_FILE=AuthKey.p8 NOTARY_KEY_ID=… NOTARY_ISSUER_ID=… mac/scripts/notarize.sh
#   NOTARY_PROFILE=radlet mac/scripts/notarize.sh    # a notarytool keychain profile
#
# The first form uses an App Store Connect API key (the release workflow's
# secrets); the second, credentials saved with `xcrun notarytool store-credentials`.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

APP=dist/Radlet.app
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
NAME="Radlet-$VERSION"
DMG="dist/$NAME.dmg"
if [[ ! -f "$DMG" ]]; then
  echo "There's no $DMG: run make-dmg.sh with a SIGN_IDENTITY first." >&2
  exit 1
fi

if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  AUTH=(--keychain-profile "$NOTARY_PROFILE")
elif [[ -n "${NOTARY_KEY_FILE:-}" && -n "${NOTARY_KEY_ID:-}" && -n "${NOTARY_ISSUER_ID:-}" ]]; then
  AUTH=(--key "$NOTARY_KEY_FILE" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")
else
  echo "Set NOTARY_PROFILE, or NOTARY_KEY_FILE, NOTARY_KEY_ID and NOTARY_ISSUER_ID." >&2
  exit 1
fi

# The disk image holds the app, so one submission covers both.
echo "Submitting $DMG to Apple's notary service (this usually takes a few minutes)…"
RESULT="$(mktemp)"
trap 'rm -f "$RESULT"' EXIT
xcrun notarytool submit "$DMG" "${AUTH[@]}" --wait --timeout 1h --output-format json > "$RESULT" || true
STATUS="$(plutil -extract status raw -o - "$RESULT" 2> /dev/null || echo unknown)"
ID="$(plutil -extract id raw -o - "$RESULT" 2> /dev/null || echo)"
if [[ "$STATUS" != "Accepted" ]]; then
  echo "Notarization finished as \"$STATUS\"." >&2
  cat "$RESULT" >&2
  if [[ -n "$ID" ]]; then xcrun notarytool log "$ID" "${AUTH[@]}" >&2 || true; fi
  exit 1
fi

xcrun stapler staple "$DMG"
xcrun stapler staple "$APP"
xcrun stapler validate "$DMG"
spctl --assess --type execute --verbose "$APP"

# The zip was made before stapling; make it again so it carries the ticket.
rm -f "dist/$NAME.zip" "dist/$NAME.sha256"
ditto -c -k --keepParent "$APP" "dist/$NAME.zip"
(cd dist && shasum -a 256 "$NAME.dmg" "$NAME.zip" > "$NAME.sha256")
echo "Notarized and stapled:"
cat "dist/$NAME.sha256"
