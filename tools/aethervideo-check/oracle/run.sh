#!/usr/bin/env bash
# AETHERVIDEO (SR39) oracle: Aether + Stria vs Chromium on oracle/page.html at frame N.
#   tools/aethervideo-check/oracle/run.sh <workdir> [N]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
work="${1:?workdir}"; n="${2:-7}"
export PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers}"
bin="${AVC_BIN:-$root/target/debug/aethervideo-check}"
mkdir -p "$work"
"$bin" make "$work" 10 10 320 240
node "$here/encode.cjs" "$work"
"$bin" mux "$work/chunks.bin" "$work/pattern-vp9.webm" 320 240 10
port="${PORT:-8765}"
(python3 "$here/serve.py" "$port" "$work" >/dev/null 2>&1 & echo $! > "$work/http.pid")
sleep 1
sed "s/{{PORT}}/$port/" "$here/page.html" > "$work/page.html"
node "$here/shot.cjs" "$work/page.html" "$n" 10 "$work/chromium.png" 660 500 | tee "$work/chromium.jsonl"
"$bin" render "$work/page.html" "$work/aether.png" --frame "$n" --fps 10 --width 660 --height 500 --cache "$work/media-cache" | tee "$work/aether.jsonl"
"$bin" compare "$work/aether.png" "$work/chromium.png" 0,0,320,240 --counter "$n" || true
"$bin" compare "$work/aether.png" "$work/chromium.png" 330,0,300,100 || true
"$bin" compare "$work/aether.png" "$work/chromium.png" 0,250,320,240 --counter "$n" || true
"$bin" compare "$work/aether.png" "$work/chromium.png" 330,110,300,54 || true
"$bin" compare "$work/aether.png" "$work/chromium.png" 330,250,320,240 --counter "$n" || true
"$bin" compare "$work/aether.png" "$work/chromium.png" 0,0,660,500 || true
kill "$(cat "$work/http.pid")" 2>/dev/null || true
