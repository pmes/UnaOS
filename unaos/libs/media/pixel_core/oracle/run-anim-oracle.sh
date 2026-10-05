#!/bin/sh
# ANIMWEBP oracle run: Chromium's WebCodecs ImageDecoder writes every composited frame of each file
# (chromium-anim-raw.cjs, raw RGBA via VideoFrame.copyTo), then pixel-check scores pixel_core's frames
# against them: frame counts, exact frames, max abs diff split opaque / translucent.
#   oracle/run-anim-oracle.sh <outdir> <files...>
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../../.." && pwd)
out=$1; shift
( cd "$root" && cargo build --release -q -p pixel-check )
NODE_PATH=${NODE_PATH:-/opt/node22/lib/node_modules} PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers} \
  node "$here/chromium-anim-raw.cjs" "$out" "$@"
for f in "$@"; do "$root/target/release/pixel-check" --compare-raw "$f" "$out" || true; done
