#!/usr/bin/env bash
# line-neutral.sh — GATE-LINENEUTRAL (rmbp-ledger B94): says, per changed `.rs` file, whether a commit
# range MOVED source lines above a panic-bearing site — the thing a knob-off byte-identity proof
# depends on and the thing R26's "only a comment, skip the gate" gets wrong in this codebase.
#
# WHY. `panic::Location` embeds the source LINE, so inserting or deleting a line ABOVE any `panic!`,
# `unwrap`, `expect`, `assert!`, `unreachable!`, `todo!` or `unimplemented!` site moves that site's
# literal and the knob-off image is no longer byte-identical to its baseline (arroyo `knoboff` measures
# exactly this). A comment-only diff that adds a line is therefore NOT free; a same-line fold is. This
# script is the mechanical version of the inspection every line-neutral append gets by eye: it takes a
# git range, walks each `.rs` file's hunks, and for every hunk whose added and removed line counts
# differ it reports the first affected line and how many panic-bearing sites lie BELOW it in the new
# file. Zero sites below = the move is harmless to `Location`; N > 0 = those N literals moved.
#
# It is ADVISORY by default (exit 0 with the report): a line-count change is often the right thing
# (a new function at a file's tail, a fixture leg) and only the author knows whether a knob-off
# identity claim rides on that file. `--strict` exits 1 when any non-neutral hunk has panic sites
# below it — for a lane that has promised byte identity on the files it touches. It never edits.
#
# usage: line-neutral.sh <base-ref> [<head-ref>] [--strict] [--paths <dir>]
#   <base-ref>..<head-ref> is the range (head defaults to HEAD); --paths narrows to files under <dir>
#   (default unaos/crates/kernel/src). Output: one line per changed .rs file, then a summary line
#   `GATE-LINENEUTRAL: files=N neutral=N moved=N moved-above-panic=N`.
set -uo pipefail
base=""; head="HEAD"; strict=0; paths="unaos/crates/kernel/src"
while [ $# -gt 0 ]; do
  case "$1" in
    --strict) strict=1;;
    --paths) shift; paths="$1";;
    -h|--help) sed -n '2,25p' "$0"; exit 0;;
    *) if [ -z "$base" ]; then base="$1"; else head="$1"; fi;;
  esac; shift
done
[ -n "$base" ] || { echo "GATE-LINENEUTRAL: usage: line-neutral.sh <base-ref> [<head-ref>] [--strict] [--paths <dir>]" >&2; exit 2; }
git rev-parse --verify -q "$base^{commit}" >/dev/null || { echo "GATE-LINENEUTRAL: control FAILED — $base is not a commit. No verdict." >&2; exit 2; }
git rev-parse --verify -q "$head^{commit}" >/dev/null || { echo "GATE-LINENEUTRAL: control FAILED — $head is not a commit. No verdict." >&2; exit 2; }
python3 - "$base" "$head" "$strict" "$paths" <<'PY'
import re, subprocess, sys
base, head, strict, paths = sys.argv[1], sys.argv[2], sys.argv[3] == "1", sys.argv[4]
PANIC = re.compile(r'\b(panic!|unreachable!|todo!|unimplemented!|assert!|assert_eq!|assert_ne!|debug_assert!|debug_assert_eq!|debug_assert_ne!)\s*\(|\.(unwrap|expect)\s*\(')
def code_part(line):
    # strip string literals then everything after the first `//` — a panic named in prose is not a site
    s = re.sub(r'"(\\.|[^"\\])*"', '""', line)
    i = s.find('//')
    return s if i < 0 else s[:i]
files = subprocess.run(["git", "diff", "--name-only", f"{base}..{head}", "--", paths],
                       capture_output=True, text=True, check=True).stdout.split()
files = [f for f in files if f.endswith(".rs")]
tot = dict(files=0, neutral=0, moved=0, moved_above_panic=0)
for f in files:
    tot["files"] += 1
    diff = subprocess.run(["git", "diff", "-U0", f"{base}..{head}", "--", f], capture_output=True, text=True).stdout
    try:
        new = subprocess.run(["git", "show", f"{head}:{f}"], capture_output=True, text=True, check=True).stdout.split("\n")
    except subprocess.CalledProcessError:
        print(f"  {f}: DELETED in range"); continue
    panic_lines = [i + 1 for i, l in enumerate(new) if PANIC.search(code_part(l))]
    hunks = []
    for m in re.finditer(r'^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@', diff, re.M):
        old_n = int(m.group(2)) if m.group(2) is not None else 1
        new_n = int(m.group(4)) if m.group(4) is not None else 1
        hunks.append((int(m.group(3)), old_n, new_n))
    moved = [(nl, on, nn) for (nl, on, nn) in hunks if on != nn]
    if not moved:
        tot["neutral"] += 1
        print(f"  {f}: neutral ({len(hunks)} hunk(s), every one same-line)")
        continue
    tot["moved"] += 1
    first = min(nl for nl, _, _ in moved)
    below = [p for p in panic_lines if p >= first]
    delta = sum(nn - on for _, on, nn in moved)
    if below:
        tot["moved_above_panic"] += 1
        print(f"  {f}: MOVED {delta:+d} line(s), first at new line {first} — {len(below)} panic-bearing site(s) below it (first: line {below[0]}) -> their `Location` literals moved")
    else:
        print(f"  {f}: moved {delta:+d} line(s), first at new line {first} — no panic-bearing site below it (Location-neutral)")
print(f"GATE-LINENEUTRAL: {base[:8]}..{head[:8]} files={tot['files']} neutral={tot['neutral']} moved={tot['moved']} moved-above-panic={tot['moved_above_panic']}"
      + ("" if not strict else (" — STRICT: RED" if tot['moved_above_panic'] else " — STRICT: OK")))
sys.exit(1 if strict and tot["moved_above_panic"] else 0)
PY
