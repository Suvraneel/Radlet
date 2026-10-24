#!/usr/bin/env bash
# Builds Radlet, replaces the copy in /Applications and starts it.
#
#   mac/scripts/run.sh            build, install, launch
#   mac/scripts/run.sh --check    also run the Vision helper's self-test
#
# Any options build-app.sh understands (UNIVERSAL=1, SKIP_PDFIUM=1, ...)
# can be set in front, as usual.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

bash mac/scripts/build-app.sh

if [[ "${1:-}" == "--check" ]]; then
  dist/Radlet.app/Contents/Helpers/radlet-vision selftest
fi

# Copy first, so a failed copy never leaves you without the app.
rm -rf /Applications/Radlet.app.new
cp -R dist/Radlet.app /Applications/Radlet.app.new

# Quit a running copy: politely, then firmly.
wait_until_quit() {
  for _ in $(seq 50); do
    pgrep -x "$1" > /dev/null || return 0
    sleep 0.1
  done
  return 1
}
quit_app() {
  local name="$1" id="$2"
  pgrep -x "$name" > /dev/null || return 0
  osascript -e 'with timeout of 5 seconds' -e "tell application id \"$id\" to quit" \
    -e 'end timeout' > /dev/null 2>&1 || true
  wait_until_quit "$name" || { pkill -x "$name" 2> /dev/null || true; wait_until_quit "$name"; } \
    || { pkill -9 -x "$name" 2> /dev/null || true; wait_until_quit "$name" || true; }
}
quit_app Radlet com.usemonoma.radlet
# Radlet was called Veer: two copies would both open a wheel on every drag.
quit_app Veer com.usemonoma.veer

rm -rf /Applications/Radlet.app
mv /Applications/Radlet.app.new /Applications/Radlet.app
open /Applications/Radlet.app
echo "Radlet is running from /Applications/Radlet.app"
if [[ -d /Applications/Veer.app ]]; then
  echo "An older build, Veer.app, is still in Applications. Radlet has moved its settings over; delete Veer.app when you're ready."
fi
