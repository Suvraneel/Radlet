#!/usr/bin/env bash
# Records a real `radlet serve` session for the Swift contract tests:
#   mac/scripts/contract-session.sh target/release/radlet OUT_DIR
# writes OUT_DIR/session.jsonl. CI runs this with the freshly built helper,
# then `RADLET_CONTRACT_DIR=OUT_DIR swift test`, so the app's decoders are
# checked against what the engine actually sends.
set -euo pipefail
RADLET="$1"
OUT="$2"
mkdir -p "$OUT"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
printf '1\n00:00:01,000 --> 00:00:02,500\nHello <i>there</i>\n\n' > "$WORK/subs.srt"
printf '# Notes\n\nSome *text*.\n' > "$WORK/notes.md"
printf 'not really a png' > "$WORK/broken.png"
# A 1x1 PNG, and a three-page PDF made from it.
printf 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC' \
  | base64 --decode > "$WORK/dot.png"
"$RADLET" combine "$WORK/dot.png" "$WORK/dot.png" "$WORK/dot.png" --out "$WORK/pdf" > /dev/null
PDF="$(ls "$WORK"/pdf/*.pdf | head -n 1)"
mkdir -p "$WORK/Album" && cp "$WORK/dot.png" "$WORK/Album/dot.png"
{
  printf '{"id":1,"method":"hello"}\n'
  printf '{"id":2,"method":"inspect","params":{"paths":["%s","%s"]}}\n' "$WORK/subs.srt" "$WORK/notes.md"
  printf '{"id":3,"method":"run","params":{"paths":["%s","%s","%s"],"action":{"kind":"steps","steps":[{"op":"convert","to":"txt"}]}}}\n' \
    "$WORK/subs.srt" "$WORK/notes.md" "$WORK/broken.png"
  printf '{"id":4,"method":"run","params":{"paths":["%s"],"action":{"kind":"checksum"}}}\n' "$WORK/notes.md"
  printf '{"id":5,"method":"teleport"}\n'
  # Tool slots with option windows, for an image and for a PDF.
  printf '{"id":7,"method":"inspect","params":{"paths":["%s"]}}\n' "$WORK/dot.png"
  printf '{"id":8,"method":"inspect","params":{"paths":["%s"]}}\n' "$PDF"
  # A job that makes several files from one: split into pages.
  printf '{"id":9,"method":"run","params":{"paths":["%s"],"action":{"kind":"split_pdf","mode":{"by":"every_page"}},"options":{"output":{"kind":"directory","path":"%s"}}}}\n' \
    "$PDF" "$WORK/split"
  # Save a recipe, then list recipes: built-ins and the saved file.
  printf '{"id":10,"method":"save_recipe","params":{"recipes_dir":"%s","recipe":{"name":"Contract Test","description":"Saved by the contract script","steps":[{"op":"resize","percent":50},{"op":"convert","to":"webp","quality":70},{"op":"strip_metadata"}]}}}\n' \
    "$WORK/recipes"
  printf '{"id":11,"method":"recipes","params":{"recipes_dir":"%s"}}\n' "$WORK/recipes"
  # What two files reveal (the Privacy check window).
  printf '{"id":12,"method":"privacy","params":{"paths":["%s","%s"]}}\n' "$WORK/dot.png" "$PDF"
  # A folder: conversions of what's inside it.
  printf '{"id":13,"method":"inspect","params":{"paths":["%s"]}}\n' "$WORK/Album"
  # Wait (up to 30 s) for all three jobs to finish: shutdown cancels running jobs.
  for _ in $(seq 300); do
    [ "$(grep -c '"event":"job_finished"' "$OUT/session.jsonl" 2>/dev/null || true)" -ge 3 ] && break
    sleep 0.1
  done
  printf '{"id":6,"method":"shutdown"}\n'
} | "$RADLET" serve > "$OUT/session.jsonl"
[ "$(grep -c '"event":"job_finished"' "$OUT/session.jsonl")" -eq 3 ]
! grep -q '"cancelled":true' "$OUT/session.jsonl"
echo "Recorded $(wc -l < "$OUT/session.jsonl") lines to $OUT/session.jsonl"
