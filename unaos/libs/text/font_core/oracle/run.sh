#!/bin/sh
# FONTCORE Chromium oracle, end to end: job list -> Chromium rasters + measureText -> FONTCORE comparison.
# Needs python3 + fontTools (job filtering), node + playwright with the preinstalled Chromium.
# Usage: oracle/run.sh [outdir]   (default: $TMPDIR/fontcore-oracle)
set -e
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-${TMPDIR:-/tmp}/fontcore-oracle}
mkdir -p "$out"
python3 "$here/make_jobs.py" > "$out/jobs.tsv"
PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers} NODE_PATH=${NODE_PATH:-/opt/node22/lib/node_modules} \
  node "$here/chromium_oracle.js" "$out/jobs.tsv" "$out"
cd "$here/../../../../.."
FONTCORE_ORACLE_DIR="$out" cargo test --release -p font_core --test oracle -- --nocapture
