#!/usr/bin/env bash
# append-position.sh — GATE-APPEND: a statement appended AFTER a line's first `//` compiles nothing.
#
# LEDGER P7 is the rule: when a change must be LINE-NEUTRAL (because `panic::Location` embeds source
# line numbers and an inserted line moves every panic site below it, breaking a knob-off byte-identity
# proof), the new statement is folded onto an existing line — and **it must go BEFORE that line's first
# `//`**. After it, the statement is part of the comment. It compiles nothing, `./arroyo check` stays
# green, `strings` finds no witness, and the arc reports a pass for code that was never built.
#
# WHY A GATE AND NOT A HABIT. This is the failure every same-line append is inspected for by hand, and
# hand inspection is exactly what does not scale across three seats and nine executors. It bit a real
# patch this round: ROOTFS's first build put a `#[cfg] pub use` after a trailing `//` and only the
# compiler's unrelated complaint surfaced it. Every seat that reviews an append is currently re-deriving
# this check by eye, per patch, forever.
#
# ⚠ WHY THE NAIVE FORM IS UNSHIPPABLE, MEASURED RATHER THAN ARGUED. "Flag `#[cfg(` appearing after a
# `//`" finds SEVEN lines in this tree and **every one is prose** — doc comments quoting cfg expressions
# (`video/strip.rs:62`, `video/dock.rs:46`, `arch/aarch64/smmu_tegra.rs:86`), some inside backtick spans
# that OPEN on a previous line, which a per-line backtick strip cannot see. A gate that reds on a
# sentence teaches people to disable it — knob-hygiene.sh's own header says so, and it is the same tree.
#
# THE DISCRIMINATOR: the trap is a STATEMENT, and an appended statement is the LAST thing on the line,
# so the comment text ends in `;`. Prose that merely names a cfg does not end in a semicolon. That one
# extra condition takes the tree from seven false positives to zero while still firing on both real
# trap shapes (the ROOTFS `mount(...); // ... #[cfg(...)] crate::a::b::bind(&mut mt);` form and the
# BSPRUN `} // ... #[cfg(...)] fn(cpu);` form).
#
# CONTROL PROBE, and no verdict without it. A scanner that matched nothing would report "0 violations"
# in the same words a clean tree earns. So the detector is run against BUILT-IN fixtures in both
# directions before it is allowed to scan: three prose lines that must stay silent (one of them an
# UNBACKTICKED cfg mention, the hardest case) and two trap lines that must fire. If either direction
# misbehaves the gate exits 2 — no verdict — rather than passing.
#
# usage: append-position.sh [kernel-src-dir]
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="${1:-$HERE/../crates/kernel/src}"

python3 - "$SRC" <<'PY'
import re, sys, pathlib

def _comment_of(line):
    # A `//` inside a string literal is not a comment; blank the literals first. Code quoted in
    # prose lives in backticks and is legitimate, so strip those spans too.
    s = re.sub(r'"(\\.|[^"\\])*"', '""', line)
    i = s.find('//')
    return re.sub(r'`[^`]*`', '', s[i+2:]) if i >= 0 else None

def offending(line):
    """TRAP 1 — a statement APPENDED after the line's first marker."""
    c = _comment_of(line)
    if c is None:
        return None
    c = c.rstrip()
    return c.strip()[:70] if ('#[cfg(' in c and c.endswith(';')) else None

def mirror(line):
    """TRAP 2 — P7'S MIRROR, and it cost a whole arc before anyone saw it (orin 16 found it,
    2026-09-06). Trap 1 is a statement appended AFTER a marker. This is a marker INSERTED AHEAD
    of code that was already live: the A10 fold put its `// A10 (MENUBAR2)` prose at column 139
    of the Orin key drain, and TABKEY's `#[cfg(feature = "tegra_el0")] if …wc_shell_focus_key(ev)
    { continue; }` at column ~1734 fell inside it, because a `//` runs to end of line. The Orin's
    TAB door was DEAD TEXT for the entire arc.

    EVERY OTHER SIGNAL SAID FINE, which is why it needs a gate rather than care: the line count
    did not change, so the knob-off byte-identity proof stayed green; `check` stayed green because
    the callee is a `pub fn` in a lib crate and loses no dead-code warning; and `git grep` still
    printed the line, which is HOW THE KEYDOORS AUDIT SCORED THAT DOOR WIRED. Only a column-aware
    read finds it.

    DISCRIMINATOR: an UNBACKTICKED `#[cfg(` in the comment text with a statement terminator (`{`
    or `;`) AFTER it. Prose names a cfg; prose does not then open a block or end a statement.
    Measured: 0 false positives across this tree, and it fires on the real F0 line."""
    c = _comment_of(line)
    if c is None or '#[cfg(' not in c:
        return None
    j = c.index('#[cfg(')
    return c.strip()[:70] if ('{' in c[j:] or ';' in c[j:]) else None

GOOD = [
    'foo(); // see `#[cfg(feature = "x")]` above, quoted in prose',
    '// end #[cfg(not(feature = "z"))] M2b block',
    '//! whose armed call site is `#[cfg(all(feature = "a"))]` and more',
]
BAD = [
    'mt.mount("/fat", x); // ROOTFS #[cfg(feature = "y")] crate::a::b::bind(&mut mt);',
    '} // A34 #[cfg(all(feature = "tegra"))] bsprun_el0_first_run(cpu);',
]
# The mirror control is the REAL F0 line's shape: prose inserted ahead of a live statement, with
# the statement's own trailing marker left dangling after it.
BAD_MIRROR = [
    'key_repainted = true; // A10 — Esc dismisses an open menu #[cfg(feature = "tegra_el0")] if a::b::wc_shell_focus_key(ev) { continue; } // TABKEY',
]
for g in GOOD:
    if offending(g) is not None:
        print(f"GATE-APPEND: control FAILED — the detector fires on PROSE: {g!r}. No verdict.", file=sys.stderr)
        sys.exit(2)
for b in BAD:
    if offending(b) is None:
        print(f"GATE-APPEND: control FAILED — the detector is BLIND to a trap: {b!r}. No verdict.", file=sys.stderr)
        sys.exit(2)
for g in GOOD:
    if mirror(g) is not None:
        print(f"GATE-APPEND: control FAILED — the MIRROR detector fires on PROSE: {g!r}. No verdict.", file=sys.stderr)
        sys.exit(2)
for b in BAD_MIRROR:
    if mirror(b) is None:
        print(f"GATE-APPEND: control FAILED — the MIRROR detector is BLIND to a swallowed statement. No verdict.", file=sys.stderr)
        sys.exit(2)

root = pathlib.Path(sys.argv[1])
if not root.is_dir():
    print(f"GATE-APPEND: NO VERDICT — {root} is not a directory", file=sys.stderr); sys.exit(2)
hits, scanned = [], 0
for p in sorted(root.rglob('*.rs')):
    scanned += 1
    for n, line in enumerate(p.read_text(errors='ignore').splitlines(), 1):
        o = offending(line)
        if o:
            hits.append(f"{p}:{n}: statement APPENDED after the line's first `//` — {o}")
        m = mirror(line)
        if m and not o:
            hits.append(f"{p}:{n}: statement SWALLOWED by a marker inserted ahead of it — {m}")
if hits:
    print(f"GATE-APPEND: RED — {len(hits)} statement(s) lost to a comment marker:")
    for h in hits:
        print("   ", h)
    print("    Such a statement is part of the comment: it compiles nothing and every gate stays green.")
    print("    APPENDED — move it BEFORE the line's first `//` (LEDGER P7).")
    print("    SWALLOWED — a marker was inserted AHEAD of live code; put the comment at the END of the")
    print("    line, after every statement, or the code it overran is silently dead.")
    sys.exit(1)
print(f"GATE-APPEND: OK — {scanned} files, no statement hiding after a comment marker, none swallowed by one inserted ahead of it (4 controls fired)")
PY
