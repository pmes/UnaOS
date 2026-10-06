#!/usr/bin/env bash
# capture-pin.sh — CAPTUREPIN (rmbp-ledger B476, GATEREVIEW S6): the bench lands a serial capture in evidence AND pins it.
#
# WHY. GATE-STATUS (tools/status-check.py) reads `docs/dev/evidence/**/f<N>-boot*.log` (and the Orin `render<N>-*.log`) as
# the WIRE a status row quotes. Before this, any hand-made file of that name was wire: a typed `f24-boot9.log` holding the
# line a claim wanted would have "confirmed" it. Now a capture log is wire only when docs/dev/evidence/CAPTURES.pin holds
# its sha256 and size, and this script is the one writer of a pin row: it copies the bench's own capture file(s) (the
# bridge's dated log under ~/unaos-bench/, several when the host log rotated mid-flight: they are joined IN ORDER) to the
# evidence path, hashes the result and appends `<sha256> <bytes> <path> src=<…> host=<…> utc=<…> by=capture-pin`.
# A log edited after its pin fails the gate (hash), a pin with no log fails (stale), a log with no pin fails (unpinned).
#
# WHAT IT DOES NOT DO. A hand-typed pin row is still possible; it is visible in review as a row without this script's
# `by=capture-pin` provenance pointing at a bench log. The pin makes a forged capture a deliberate act, not an accident.
#
# usage: capture-pin.sh <dest: docs/dev/evidence/…/f<N>-boot*.log | …/render<N>-*.log> <bench log> [<bench log>…]
#        (run from the repo root; exit 0 pinned · 1 refused · 2 usage)
set -u
[ $# -ge 2 ] || { sed -n '2,/^set -u/p' "$0" | sed 's/^# \{0,1\}//' | head -16; exit 2; }
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
dest="$1"; shift
case "$dest" in /*) rel="${dest#"$ROOT"/}";; *) rel="$dest";; esac
case "$rel" in docs/dev/evidence/*) ;; *) echo "capture-pin: refused — $rel is not under docs/dev/evidence/"; exit 1;; esac
b=$(basename "$rel")
[[ $b =~ ^f[0-9]+-boot[^/]*\.log$ || $b =~ ^(boot-)?render[0-9]+[a-z]?-[^/]*\.log$ ]] \
    || { echo "capture-pin: refused — $b is not a capture name (f<N>-boot*.log, render<N>-*.log, boot-render<N>-*.log)"; exit 1; }
for s in "$@"; do [ -f "$s" ] || { echo "capture-pin: refused — no bench log $s"; exit 1; }; done
pin="$ROOT/docs/dev/evidence/CAPTURES.pin"
grep -q " $rel " "$pin" 2>/dev/null && { echo "capture-pin: refused — $rel is already pinned (a capture is pinned once)"; exit 1; }
tmp=$(mktemp); cat "$@" > "$tmp"
if [ -e "$ROOT/$rel" ] && ! cmp -s "$tmp" "$ROOT/$rel"; then
    rm -f "$tmp"; echo "capture-pin: refused — $rel exists and differs from the bench log(s)"; exit 1
fi
mkdir -p "$(dirname "$ROOT/$rel")"; mv "$tmp" "$ROOT/$rel"
sha=$( (sha256sum "$ROOT/$rel" 2>/dev/null || shasum -a 256 "$ROOT/$rel") | cut -d' ' -f1)
bytes=$(wc -c < "$ROOT/$rel" | tr -d ' ')
src=$(for s in "$@"; do printf '%s,' "$(cd "$(dirname "$s")" && pwd)/$(basename "$s")"; done); src=${src%,}
printf '%s %s %s src=%s host=%s utc=%s by=capture-pin\n' "$sha" "$bytes" "$rel" "${src// /_}" "$(hostname -s 2>/dev/null || echo unknown)" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$pin"
echo "capture-pin: pinned $rel sha256=$sha bytes=$bytes (from $# bench log(s))"
