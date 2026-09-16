#!/usr/bin/env bash
# lba32-check.sh — GATE-LBA32: a 64-bit LBA may not be narrowed to 32 bits with `as`.
#
# WHY THIS EXISTS (LEDGER SR15, orin ledger A57). Three files in this tree have now paid for the
# same defect: a `u64` sector number handed to a 32-bit field with `as`. `as` on an out-of-range
# value does not fail, it TRUNCATES. LBA `0x1_0000_0000` becomes `0` — THE BOOT SECTOR — so a read
# returns the wrong sector as if it were right and a write destroys the partition table and returns
# success. A57 folded `arch/aarch64/sdmmc_tegra.rs`'s six sites onto `sd_block_arg`; BLOCKSMALL
# folded `drivers/block.rs`'s eight onto `read10_lba32`; EMMC2LBA folded `drivers/emmc2.rs`'s two
# onto `card_block_arg`. SR15's own closing sentence is the brief for this file: "a grep gate over
# the call shape … would make it mechanical and is not built."
#
# It is also the only thing that CAN guard the call sites. SR15 states the bound it hit: every one
# of those fixtures is a known-answer test on the HELPER, and reverting one CALL SITE to a bare
# `as u32` is invisible to all of them, because the `lba >= dev.num_blocks` / `lba >= card.num_blocks`
# geometry bound one line earlier refuses an out-of-range LBA with the same error — so a wrapped site
# and a refusing site are indistinguishable from any caller on any reachable input. No behavioural
# leg can close that gap. A structural one can, and this is it.
#
# WHAT IT ASSERTS. Over `crates/kernel/src/drivers/**` and `crates/kernel/src/fs/**`, with comments
# and string literals stripped: every `<expr> as u32` whose operand names an LBA — an identifier
# token matching `lba` or `sector`, in any case, that is not SCREAMING_SNAKE (a const is a sector
# SIZE, never a sector NUMBER) — is a FINDING, unless the enclosing function is registered in
# `scripts/lba32.registry`. Both of the shapes the two ledger rows name are caught: the bare
# `lba as u32` and the byte-offset `(lba * 512) as u32`.
#
# THE REGISTRY IS NOT AN ALLOWLIST, AND ITS ROWS ARE RE-CHECKED EVERY RUN. A row names a function
# whose narrowing is proven by a refusal INSIDE that same function — the `sd_block_arg` shape, where
# `if lba > u32::MAX as u64 { … }` stands above `Some(lba as u32)`. The gate re-reads the registered
# function's body and requires a refusal token in it (`u32::try_from`, `try_into`, `> u32::MAX`,
# `>= u32::MAX`, `checked_mul`); a helper gutted back to a bare cast loses its cover and the row
# becomes a finding. That is what keeps the registry from laundering the defect it exists to record.
#
# THE CONTROLS, AND WHY THEY ARE SYNTHETIC. A scan that matched nothing would report zero findings,
# and zero findings reads as a clean tree (STRUCTURAL_GATES.md §The control probe) — which is the
# gate's own failure mode here, because the tree IS clean the day this lands. Eight controls run
# over an in-memory synthetic file before any verdict is given, three that MUST fire and five that
# MUST NOT, each killing one stage of the analyser if it breaks:
#
#   1. MUST FIRE  `ctrl_bare`     — `let arg = lba as u32;`             the whole point of the gate.
#   2. MUST FIRE  `ctrl_mul`      — `let arg = (lba * 512) as u32;`     A57's byte-offset shape; dies
#                                                                      if the parenthesised-operand
#                                                                      walk-back breaks.
#   3. MUST FIRE  `ctrl_gutted`   — REGISTERED, but its body carries no refusal token. Dies if the
#                                   registry stops re-checking its own rows, i.e. if it degrades
#                                   into an allowlist.
#   4. MUST NOT   `ctrl_helper`   — REGISTERED and carrying `> u32::MAX` above `Some(lba as u32)`:
#                                   the sanctioned shape, which must not be reported.
#   5. MUST NOT   `ctrl_tryfrom`  — `u32::try_from(lba)`: the other sanctioned shape, which forms no
#                                   cast at all.
#   6. MUST NOT   `ctrl_comment`  — a bare cast inside `//`. Dies if comment stripping breaks, which
#                                   is the dominant population: this tree's prose discusses the
#                                   defect far more often than its code commits it.
#   7. MUST NOT   `ctrl_string`   — a bare cast inside a string literal (this tree prints the defect
#                                   on the wire, `wrapto=`/`wrapblk=`). Dies if literal stripping
#                                   breaks.
#   8. MUST NOT   `ctrl_const`    — `SECTOR_BYTES as u32`: a sector SIZE. Dies if the const exclusion
#                                   breaks, and without it this gate fires eight times on a clean
#                                   tree and is skipped within the week.
#
# A control failure is a BROKEN GATE, not a clean tree, and gives NO VERDICT.
#
# LIMITS OF THE APPROXIMATION, STATED. This is a lexical scan, not rustc.
#   * An LBA held in a variable whose name says nothing (`n`, `start`, `v`) is invisible. The scan
#     is over the NAMING CONVENTION this tree actually uses, and every site in both ledger rows
#     obeys it; a rename is the way past this gate and is a reviewable diff.
#   * A cast split across two source lines is invisible (none exists in the scanned trees:
#     `grep -c 'as$' ` over them is 0).
#   * Coverage is per FUNCTION, not per site: a registered helper that grows a second, unguarded
#     cast is not reported. Helpers here are three to ten lines and the registry names each one's
#     measured body; a helper that grows is a review, not a gate bypass.
#   * `arch/**` is out of scope by construction — the brief scopes this to `drivers/**` and `fs/**`.
#     `arch/aarch64/sdmmc_tegra.rs::sd_block_arg` (A57) is the same shape and is guarded the same
#     way; it is named in the registry header so a reader knows it was measured, not missed.
#   Every limit biases toward SILENCE, which is why controls 1-3 are mandatory.
#
# EXIT: 0 clean (or only registered findings) · 1 an unregistered finding · 2 a control failed or
# the source root is missing, NO VERDICT GIVEN.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="${1:-$HERE/../crates/kernel/src}"
REGISTRY="${LBA32_REGISTRY:-$HERE/lba32.registry}"

if [ ! -d "$SRC" ]; then
    echo "lba32: NO VERDICT — source root not found: $SRC" >&2
    exit 2
fi

python3 - "$SRC" "$REGISTRY" <<'PY'
import os, re, sys

SRC = os.path.abspath(sys.argv[1])
REGISTRY = sys.argv[2]
ROOTS = ("drivers", "fs")

# ---------------------------------------------------------------- comment + literal stripping
def strip_noncode(text):
    """Replace comment and string/char-literal bytes with spaces, preserving newlines and columns.

    Columns are preserved so a reported line number and the reported source text agree with the
    file a reader opens; newlines so line numbers are the file's own."""
    out = list(text)
    i, n = 0, len(text)
    def blank(a, b):
        for k in range(a, min(b, n)):
            if text[k] != '\n':
                out[k] = ' '
    while i < n:
        c = text[i]
        if c == '/' and i + 1 < n and text[i + 1] == '/':
            j = text.find('\n', i)
            j = n if j < 0 else j
            blank(i, j); i = j
        elif c == '/' and i + 1 < n and text[i + 1] == '*':
            depth, j = 1, i + 2
            blank(i, i + 2)
            while j < n and depth > 0:
                if text[j] == '/' and j + 1 < n and text[j + 1] == '*':
                    depth += 1; blank(j, j + 2); j += 2; continue
                if text[j] == '*' and j + 1 < n and text[j + 1] == '/':
                    depth -= 1; blank(j, j + 2); j += 2; continue
                blank(j, j + 1); j += 1
            i = j
        elif c == 'r' and text[i:i + 2] == 'r#' or (c == 'r' and i + 1 < n and text[i + 1] == '"'):
            # raw string: r"…", r#"…"#, r##"…"##
            j = i + 1
            hashes = 0
            while j < n and text[j] == '#':
                hashes += 1; j += 1
            if j >= n or text[j] != '"':
                i += 1; continue
            close = '"' + '#' * hashes
            end = text.find(close, j + 1)
            end = n if end < 0 else end + len(close)
            blank(i, end); i = end
        elif c == '"':
            j = i + 1
            while j < n:
                if text[j] == '\\':
                    j += 2; continue
                if text[j] == '"':
                    j += 1; break
                j += 1
            blank(i, j); i = j
        elif c == "'":
            # A char literal is at most `'\u{1F}'`; a lifetime (`'a`) has no closing quote nearby.
            m = re.match(r"'(?:\\.[^']*|[^'\\])'", text[i:i + 12])
            if m:
                blank(i, i + m.end()); i += m.end()
            else:
                i += 1
        else:
            i += 1
    return ''.join(out)

# ---------------------------------------------------------------- operand walk-back
IDENT_CHARS = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.:")

def operand_before(line, at):
    """The text of the expression immediately left of `as` at index `at`, or None.

    `)` walks back to its matching `(` (that is A57's `(lba * 512) as u32`); `]` to its `[`;
    otherwise a run of identifier/path/field characters (`self.lba`, `hdr.start_lba`)."""
    j = at - 1
    while j >= 0 and line[j] in " \t":
        j -= 1
    if j < 0:
        return None
    end = j + 1
    if line[j] in ")]":
        opener = "(" if line[j] == ")" else "["
        closer = line[j]
        depth = 0
        while j >= 0:
            if line[j] == closer:
                depth += 1
            elif line[j] == opener:
                depth -= 1
                if depth == 0:
                    break
            j -= 1
        if j < 0:
            return None
        # A call's argument list is not the operand: `foo(x)` — take the callee with it.
        k = j - 1
        while k >= 0 and line[k] in IDENT_CHARS:
            k -= 1
        return line[k + 1:end]
    while j >= 0 and line[j] in IDENT_CHARS:
        j -= 1
    if j + 1 == end:
        return None
    return line[j + 1:end]

LBA_WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
LBA_NAME = re.compile(r"(?i)(lba|sector)")

def names_an_lba(expr):
    """True if `expr` names an LBA: a non-const identifier token containing `lba` or `sector`."""
    for w in LBA_WORD.findall(expr):
        if not LBA_NAME.search(w):
            continue
        if w.isupper():          # SECTOR_BYTES / SECTOR_SIZE — a size, never a number
            continue
        return True
    return False

FN_DECL = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")
AS_U32 = re.compile(r"\bas\s+u32\b")
REFUSAL = ("u32::try_from", "try_into", "> u32::MAX", ">= u32::MAX", "checked_mul")

def analyse(files):
    """files: {relpath: source text}. Returns (findings, fn_bodies, census).

    A finding is (relpath, lineno, fnname, operand, source line, stripped)."""
    findings, bodies, casts = [], {}, 0
    for rel, text in sorted(files.items()):
        stripped = strip_noncode(text)
        raw_lines = text.split('\n')
        lines = stripped.split('\n')
        fn_at = {}
        cur = "<file scope>"
        for ln, line in enumerate(lines, 1):
            m = FN_DECL.search(line)
            if m:
                cur = m.group(1)
                bodies.setdefault((rel, cur), [])
            fn_at[ln] = cur
            if cur != "<file scope>":
                bodies.setdefault((rel, cur), []).append(line)
        for ln, line in enumerate(lines, 1):
            for m in AS_U32.finditer(line):
                casts += 1
                expr = operand_before(line, m.start())
                if expr and names_an_lba(expr):
                    findings.append((rel, ln, fn_at[ln], expr.strip(),
                                     raw_lines[ln - 1].strip(), line))
    return findings, bodies, casts

def read_registry(path):
    rows = {}
    if not os.path.exists(path):
        return rows
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, _, reason = line.partition("  ")
        key = key.strip()
        if "::" not in key:
            print(f"lba32: NO VERDICT — registry row is not <path>::<fn>: {line}", file=sys.stderr)
            sys.exit(2)
        rows[tuple(key.rsplit("::", 1))] = reason.strip()
    return rows

def covered(key, rows, bodies):
    """A registry row covers a finding only while the registered function still refuses."""
    if key not in rows:
        return False, "unregistered"
    body = "\n".join(bodies.get(key, []))
    if not any(tok in body for tok in REFUSAL):
        return False, "registered, but the function carries no refusal any more"
    return True, rows[key]

# ---------------------------------------------------------------- CONTROL (synthetic, in-memory)
CTRL_SRC = {
    "ctrl.rs": '''
fn ctrl_bare(lba: u64) -> u32 {
    let arg = lba as u32;
    arg
}
fn ctrl_mul(lba: u64) -> u32 {
    let arg = (lba * 512) as u32;
    arg
}
fn ctrl_gutted(lba: u64) -> Option<u32> {
    Some(lba as u32)
}
fn ctrl_helper(lba: u64) -> Option<u32> {
    if lba > u32::MAX as u64 {
        return None;
    }
    Some(lba as u32)
}
fn ctrl_tryfrom(lba: u64) -> Option<u32> {
    u32::try_from(lba).ok()
}
fn ctrl_comment(lba: u64) -> u32 {
    // let arg = lba as u32;
    /* let arg = (lba * 512) as u32; */
    u32::try_from(lba).unwrap_or(0)
}
fn ctrl_string(lba: u64) -> u32 {
    serial_println!("the old shape was lba as u32 and it wrapped onto 0");
    u32::try_from(lba).unwrap_or(0)
}
fn ctrl_const(bs: u64) -> u32 {
    let _ = bs;
    SECTOR_BYTES as u32
}
'''
}
CTRL_REGISTRY = {("ctrl.rs", "ctrl_gutted"): "synthetic: registered with no refusal",
                 ("ctrl.rs", "ctrl_helper"): "synthetic: registered and refusing"}
CTRL_MUST_FIRE = {"ctrl_bare", "ctrl_mul", "ctrl_gutted"}
CTRL_MUST_NOT = {"ctrl_helper", "ctrl_tryfrom", "ctrl_comment", "ctrl_string", "ctrl_const"}

cf, cb, _ = analyse(CTRL_SRC)
fired = set()
for rel, ln, fn, expr, raw, _s in cf:
    ok, _why = covered((rel, fn), CTRL_REGISTRY, cb)
    if not ok:
        fired.add(fn)
missing = CTRL_MUST_FIRE - fired
spurious = CTRL_MUST_NOT & fired
if missing or spurious:
    print("lba32: NO VERDICT — the control fixture did not behave:")
    for f in sorted(missing):
        print(f"   control MUST fire and did not: {f}")
    for f in sorted(spurious):
        print(f"   control MUST NOT fire and did: {f}")
    print("   a broken analyser reports zero findings, and zero findings is not a clean tree")
    sys.exit(2)

# ---------------------------------------------------------------- the tree
files = {}
for root in ROOTS:
    base = os.path.join(SRC, root)
    if not os.path.isdir(base):
        print(f"lba32: NO VERDICT — scan root missing: {base}", file=sys.stderr)
        sys.exit(2)
    for dp, _dn, fns in os.walk(base):
        for fn in fns:
            if fn.endswith(".rs"):
                p = os.path.join(dp, fn)
                files[os.path.relpath(p, SRC)] = open(p, encoding="utf-8", errors="replace").read()

rows = read_registry(REGISTRY)
findings, bodies, casts = analyse(files)

unregistered, registered = [], []
for f in findings:
    rel, ln, fn = f[0], f[1], f[2]
    ok, why = covered((rel, fn), rows, bodies)
    (registered if ok else unregistered).append((f, why))

print(f"lba32: scanned {len(files)} files under {'/, '.join(ROOTS)}/ — {casts} `as u32` casts, "
      f"{len(findings)} on an LBA-named operand ({len(registered)} registered, "
      f"{len(unregistered)} not); controls {len(CTRL_MUST_FIRE)}/{len(CTRL_MUST_FIRE)} fired, "
      f"{len(CTRL_MUST_NOT)}/{len(CTRL_MUST_NOT)} silent")
for (rel, ln, fn, expr, raw, _s), why in registered:
    print(f"   registered  {rel}:{ln}  {fn}()  `{expr} as u32`  — {why}")
for (rel, ln, fn, expr, raw, _s), why in unregistered:
    print(f"❌ FINDING    {rel}:{ln}  {fn}()  `{raw}`  — {why}")

if unregistered:
    print("lba32: a 64-bit LBA is narrowed to 32 bits with `as`. `as` TRUNCATES: LBA 0x1_0000_0000")
    print("       becomes 0, the boot sector. Route it through the file's refusal helper")
    print("       (`read10_lba32` / `lba_arg` / `card_block_arg` / `sd_block_arg`), or register the")
    print("       enclosing function in scripts/lba32.registry with the measurement. LEDGER SR15.")
    sys.exit(1)
sys.exit(0)
PY
