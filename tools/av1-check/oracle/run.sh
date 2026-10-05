#!/bin/sh
# The AVCODEC oracle: decode <file.avif> with av1-check, screenshot Chromium rendering the same file
# 1:1 in an 800x600 page, and compare. usage: run.sh <file.avif> <workdir> [av1-check options]
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
f=$1; work=$2; shift 2
mkdir -p "$work"
b=$(basename "$f" .avif)
"$root/target/release/av1-check" "$f" "$work/$b.dec.png" "$@" > "$work/$b.log"
NODE_PATH=${NODE_PATH:-/opt/node22/lib/node_modules} PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers} \
  node "$here/shot.cjs" "$f" "$work/$b.chromium.png" > /dev/null
node "$here/compare.cjs" "$work/$b.dec.png" "$work/$b.chromium.png"
