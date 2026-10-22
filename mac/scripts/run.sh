#!/usr/bin/env bash
# Builds Veer, replaces the copy in /Applications and starts it.
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
  dist/Veer.app/Contents/Helpers/veer-vision selftest
fi

# Copy first, so a failed copy never leaves you without the app.
rm -rf /Applications/Veer.app.new
cp -R dist/Veer.app /Applications/Veer.app.new

# Quit the running copy: politely, then firmly.
wait_until_quit() {
  for _ in $(seq 50); do
    pgrep -x Veer > /dev/null || return 0
    sleep 0.1
  done
  return 1
}
if pgrep -x Veer > /dev/null; then
  osascript -e 'with timeout of 5 seconds' -e 'tell application id "com.usemonoma.veer" to quit' \
    -e 'end timeout' > /dev/null 2>&1 || true
  wait_until_quit || { pkill -x Veer 2> /dev/null || true; wait_until_quit; } \
    || { pkill -9 -x Veer 2> /dev/null || true; wait_until_quit || true; }
fi

rm -rf /Applications/Veer.app
mv /Applications/Veer.app.new /Applications/Veer.app
open /Applications/Veer.app
echo "Veer is running from /Applications/Veer.app"
