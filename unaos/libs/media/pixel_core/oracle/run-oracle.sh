#!/bin/sh
# PIXELCORE oracle run: Chromium screenshots each file (<img> at 1:1 on background BG), then
# pixel-check scores pixel_core's decode composited on the same background against it.
#   oracle/run-oracle.sh <outdir> <bg r,g,b> [--orient] <files...>
# Prints one line per file: max_abs_diff, % exact channel bytes, PSNR over RGB.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../../.." && pwd)
out=$1; bg=$2; shift 2
orient=""
if [ "${1:-}" = "--orient" ]; then orient="--orient"; shift; fi
( cd "$root" && cargo build --release -q -p pixel-check )
NODE_PATH=${NODE_PATH:-/opt/node22/lib/node_modules} PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers} \
  node "$here/chromium-oracle.cjs" "$out" "$bg" "$@" >/dev/null
for f in "$@"; do
  shot="$out/$(basename "$f").shot.png"
  if [ -f "$shot" ]; then "$root/target/release/pixel-check" --compare "$f" "$shot" "$bg" $orient || true
  else echo "$f CHROMIUM-REFUSED"; fi
done
