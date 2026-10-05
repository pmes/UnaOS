#!/bin/sh
# SVGCORE test vectors: the resvg test suite's SVG files (+ its fonts) at the pinned commit, fetched by URL and
# checked by sha256 against tests/vectors.txt. Nothing is committed. Usage:
#   ./fetch-vectors.sh [dir]        (default: <repo>/target/svg-vectors)
# then run the gate with SVGCORE_VECTORS=<dir>. Offline, the vector tests print SKIP and pass.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
DIR=${1:-$root/target/svg-vectors}
list="$here/tests/vectors.txt"
BASE=$(sed -n '1s/.*\(https:[^<]*\)<relpath>.*/\1/p' "$list")
export DIR BASE
mkdir -p "$DIR"
grep -v '^#' "$list" | awk -F'\t' '{print $2, $3}' | xargs -P 16 -n 2 sh -c '
  rel=$1; sha=$2; f="$DIR/$rel"
  if [ -f "$f" ] && [ "$(sha256sum "$f" | cut -c1-64)" = "$sha" ]; then exit 0; fi
  mkdir -p "$(dirname "$f")"
  u=$(printf %s "$rel" | sed "s/%/%25/g; s/#/%23/g; s/ /%20/g; s/?/%3F/g")
  if ! curl -fsS --retry 2 -o "$f.part" "$BASE$u"; then echo "fetch failed: $rel" >&2; rm -f "$f.part"; exit 0; fi
  if [ "$(sha256sum "$f.part" | cut -c1-64)" = "$sha" ]; then mv "$f.part" "$f"; else echo "sha256 mismatch: $rel" >&2; rm -f "$f.part"; fi
' sh
total=$(grep -vc '^#' "$list")
have=$(grep -v '^#' "$list" | awk -F'\t' '{print $2}' | while read -r rel; do [ -f "$DIR/$rel" ] && echo x; done | wc -l)
echo "svg vectors: $have of $total in $DIR"
