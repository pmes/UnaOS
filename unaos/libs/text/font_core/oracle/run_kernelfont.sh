#!/bin/sh
# KERNELFONT (rmbp-ledger B359) M4: the login-screen mock in Chromium vs the kernel's text engine (font_core::ui).
# Each job is one string of the login screen / menu bar in the DejaVu file and size the kernel's `video::text`
# draws it at (Ui 13 px = font_size 13 at 96 ppi; Ui 13.5 px = the rMBP's cap; Chrome = DejaVu Sans Bold 14 px;
# Body = DejaVu Sans Mono 11.5 px). Chromium: chromium_oracle.js (same @font-face file, hinting none, grayscale AA).
# Usage: oracle/run_kernelfont.sh [outdir]   (default: $TMPDIR/kernelfont-oracle)
set -e
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-${TMPDIR:-/tmp}/kernelfont-oracle}
mkdir -p "$out"
D=/usr/share/fonts/truetype/dejavu
S="$D/DejaVuSans.ttf"; B="$D/DejaVuSans-Bold.ttf"; M="$D/DejaVuSansMono.ttf"
{
  n=0
  for spec in "ui13:$S:13" "ui135:$S:13.5" "chrome14:$B:14" "body115:$M:11.5"; do
    id=${spec%%:*}; rest=${spec#*:}; font=${rest%:*}; size=${rest##*:}
    while IFS= read -r t; do
      n=$((n+1)); printf '%s_%02d\t%s\t%s\t%s\n' "$id" "$n" "$font" "$size" "$t"
    done <<'STRINGS'
UnaOS
Log In
Name
Password
Enter or Log In   Tab switches
hold Shift after login to reset display settings
Shift after login: reset display
root
peter
Quarry  File  Edit  View  Window  Help
12:34
Settings - Display - Font
The quick brown fox jumps over the lazy dog 0123456789
STRINGS
  done
} > "$out/jobs.tsv"
PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH:-/opt/pw-browsers} NODE_PATH=${NODE_PATH:-/opt/node22/lib/node_modules} \
  node "$here/chromium_oracle.js" "$out/jobs.tsv" "$out"
cd "$here/../../../../.."
KERNELFONT_ORACLE_DIR="$out" cargo test --release -p font_core --test kernelfont -- --nocapture
