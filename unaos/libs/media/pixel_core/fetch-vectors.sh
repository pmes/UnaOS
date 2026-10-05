#!/bin/sh
# PIXELCORE: fetch the KAT vectors listed in tests/vectors.txt into tests/vectors/ (gitignored), verifying
# each file's sha256. Re-runnable; files already present with the right digest are skipped. Offline =>
# the tests print SKIP for whatever is missing. Lines: <sha256> <relative path> <url>
set -u
cd "$(dirname "$0")"
fail=0
while read -r sum rel url; do
  case "$sum" in ''|'#'*) continue ;; esac
  dst="tests/vectors/$rel"
  if [ -f "$dst" ] && [ "$(sha256sum "$dst" | cut -d' ' -f1)" = "$sum" ]; then continue; fi
  mkdir -p "$(dirname "$dst")"
  if curl -sSfL --retry 2 -o "$dst.part" "$url"; then
    got=$(sha256sum "$dst.part" | cut -d' ' -f1)
    if [ "$got" = "$sum" ]; then mv "$dst.part" "$dst"; else echo "SHA MISMATCH $rel ($got)"; rm -f "$dst.part"; fail=1; fi
  else
    echo "FETCH FAILED $rel"; rm -f "$dst.part"; fail=1
  fi
done < tests/vectors.txt
exit $fail
