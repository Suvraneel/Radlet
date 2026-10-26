#!/usr/bin/env bash
# Sets up signed updates, once. Run it on your Mac, from the repository:
#
#   mac/scripts/setup-updates.sh
#
# 1. Creates Radlet's EdDSA signing key in your login keychain, or reuses it.
# 2. Writes the public key into mac/Resources/Info.plist (SUPublicEDKey).
# 3. Stores the private key as the repository's SPARKLE_ED_PRIVATE_KEY
#    secret with GitHub's gh tool, without leaving a copy on disk.
#
# Then commit Info.plist. Releases from then on carry an appcast.xml, and
# copies of Radlet built with the key offer them as updates.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
ACCOUNT=radlet

swift package --package-path mac resolve > /dev/null
BIN="$(find mac/.build/artifacts -maxdepth 4 -type d -path '*/Sparkle/bin' | head -1)"
if [[ ! -x "$BIN/generate_keys" ]]; then
  echo "Sparkle's tools weren't found under mac/.build/artifacts." >&2
  exit 1
fi

"$BIN/generate_keys" --account "$ACCOUNT" > /dev/null
PUBLIC="$("$BIN/generate_keys" --account "$ACCOUNT" -p)"
/usr/libexec/PlistBuddy -c "Set :SUPublicEDKey $PUBLIC" mac/Resources/Info.plist
echo "Public key written to mac/Resources/Info.plist: $PUBLIC"

if ! command -v gh > /dev/null; then
  echo
  echo "GitHub's gh tool isn't installed, so the private key isn't on GitHub yet."
  echo "Install it (brew install gh), run gh auth login, then run this script again."
  exit 1
fi
TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
"$BIN/generate_keys" --account "$ACCOUNT" -x "$TEMP/key"
gh secret set SPARKLE_ED_PRIVATE_KEY < "$TEMP/key"
echo "Private key stored as the SPARKLE_ED_PRIVATE_KEY secret."
echo "Now commit mac/Resources/Info.plist."
