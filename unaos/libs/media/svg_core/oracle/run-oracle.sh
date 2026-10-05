#!/bin/sh
# SVGCORE oracle run: Chromium (the pre-installed Playwright build) renders every vector as an <img> at its
# reference size on white, then tests/oracle.rs renders the same file with svg_core and scores it.
#   oracle/run-oracle.sh <vectors-dir> <out-dir>
# <vectors-dir> is what fetch-vectors.sh produced (tests/… and fonts/…). Chromium sees ONLY the vector fonts,
# through a private fontconfig (fonts.conf.in), so both renderers draw text from the same files.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
crate=$(cd "$here/.." && pwd)
root=$(cd "$crate/../../../.." && pwd)
vec=$(cd "$1" && pwd)
out=$2
mkdir -p "$out/chrome" "$out/fccache"
sed -e "s#@FONTDIR@#$vec/fonts#" -e "s#@CACHEDIR@#$out/fccache#" "$here/fonts.conf.in" > "$out/fonts.conf"
grep -v '^#' "$crate/tests/vectors.txt" | awk -F'\t' -v V="$vec" '$1 != "font" {print $2"\t"$4"\t"$5"\t"V"/"$2}' > "$out/list.tsv"
FONTCONFIG_FILE="$out/fonts.conf" NODE_PATH=${NODE_PATH:-/opt/node22/lib/node_modules} \
  PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers} \
  node "$here/chromium-svg.cjs" "$out/chrome" "$out/list.tsv" > "$out/chrome.log"
cd "$root"
SVGCORE_VECTORS="$vec" SVGCORE_ORACLE_DIR="$out/chrome" SVGCORE_ORACLE_OUT="$out/rows.tsv" \
  cargo test --release -q -p svg_core --test oracle -- --nocapture 2>&1 | grep -E "^(category|[a-z-]+ +[0-9]|ALL)"
echo "per-file scores: $out/rows.tsv"
