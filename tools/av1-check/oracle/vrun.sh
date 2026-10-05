#!/bin/sh
# The AVCODEC2 video oracle: decode <clip.mp4|webm> with av1-check (frame N = the Nth shown frame),
# have Chromium play the same file in a <video>, seek to frame N (requestVideoFrameCallback reports
# the presented mediaTime) and screenshot it 1:1, then compare.
# usage: vrun.sh <clip> <workdir> <fps> <frame>[,<frame>...]
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
f=$1; work=$2; fps=$3; frames=$4
mkdir -p "$work"
"$root/target/release/av1-check" "$f" --png "$work/dec_" --frames "$frames" > "$work/decode.log"
NODE_PATH=${NODE_PATH:-/opt/node22/lib/node_modules} PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers} \
  node "$here/vshot.cjs" "$f" "$work/chr_" "$fps" "$frames" > "$work/shots.log"
for n in $(echo "$frames" | tr ',' ' '); do
  p=$(printf %03d "$n")
  printf "frame %s " "$n"
  node "$here/compare.cjs" "$work/dec_$p.png" "$work/chr_$p.png"
done
