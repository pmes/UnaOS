#!/bin/sh
# FONTBIDI (SR56) Chromium oracle, end to end: Noto fonts (fetched, sha-checked) -> job list -> Chromium rasters,
# measureText and grapheme visual order -> font_core comparison (rasters per script/size) and a refreshed frozen
# slice. Needs node + playwright with the preinstalled Chromium (/opt/pw-browsers), python3, curl.
# Usage: oracle/run_fontbidi.sh [outdir]   (default: $TMPDIR/fontbidi-oracle)
set -e
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-${TMPDIR:-/tmp}/fontbidi-oracle}
mkdir -p "$out/fonts"
grep '^https://raw.githubusercontent.com/notofonts' "$here/vectors.txt" | while read -r url sha rest; do
  f="$out/fonts/$(basename "$url")"
  [ -f "$f" ] && [ "$(sha256sum "$f" | cut -c1-64)" = "$sha" ] && continue
  curl -fsSL -o "$f" "$url"
  [ "$(sha256sum "$f" | cut -c1-64)" = "$sha" ] || { echo "sha256 mismatch: $f" >&2; exit 1; }
done
python3 "$here/fontbidi_jobs.py" "$out/fonts" > "$out/jobs.tsv"
PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers} NODE_PATH=${NODE_PATH:-/opt/node22/lib/node_modules} \
  node "$here/chromium_fontbidi.js" "$out/jobs.tsv" "$out"
cd "$here/../../../../.."
FONTBIDI_ORACLE_DIR="$out" FONTBIDI_FROZEN_OUT="$out/fontbidi_chrome.tsv" \
  cargo test --release -p font_core --test fontbidi_oracle -- --nocapture --test-threads 1
echo "frozen slice: $out/fontbidi_chrome.tsv (copy to tests/data/ after an intended change)"
