#!/usr/bin/env python3
"""
orin-specscore.py — score a jetson capture against a witness spec AND say, for
every rule, whether that rule COULD have fired on the image the capture came from.

WHY THIS EXISTS.  `mbench.py --replay` already prints a per-rule verdict table and
is the semantics of record; this tool does not re-implement any of it (it imports
mbench and feeds the same `Directive` objects, so the two can never drift).  What
mbench cannot say is the thing that matters most on a spec this large:

    a rule that CANNOT FIRE is indistinguishable, in mbench's table, from a rule
    that passed.

`FORBID \\[wedge4\\] preempt-in-section core=[0-9]+` prints `✅ 0 hits` on every
capture ever taken -- not because the invariant held, but because the literal
`preempt-in-section` is not in the image's `.rodata` at all.  Sixteen such rows
read as sixteen passes.  That is coverage claimed and not held, and this tree has
shipped that failure class before.

So this tool adds ONE column.  It is measured from TWO artifacts, because neither
alone can answer the question:

    the SOURCE says what text this kernel is CAPABLE of printing -- every format
    string, minus its `{}` fields, is a set of `.rodata` chunks;
    the IMAGE says which of those sites SURVIVED the build.

A rule is reachable exactly when some format string that can produce its text has
its fingerprint -- its longest literal chunk -- in the image.  Four classes:

    IMG      every literal run of the pattern is contiguous in the image.  The
             rule can fire and `strings` can confirm it.

    WIRE     a run is not contiguous in the image, but a format string that CAN
             compose it is.  `live=FROZEN` is not in `.rodata` because the
             emitter is `live={}` with `"LIVE"`/`"FROZEN"` chosen at print time
             (`display_tegra.rs`), so that string exists ONLY ON THE WIRE.  The
             rule can fire; `strings` will never show it, and hunting for it with
             `strings` and concluding the image is broken is a trap this column
             exists to close.

    DEAD     a format string that could produce the text exists in the SOURCE,
             but no such site is in THIS image: the feature that emits it was not
             compiled in.  The rule cannot fire on this image.

    FOREIGN  no format string in the kernel source can put this text on the wire
             at all.  Either it comes from somewhere else (firmware and
             bootloader lines are not in `kernel.elf` by construction), or the
             source has moved away from the shape the rule was written against.
             The second case is sometimes DELIBERATE -- a FORBID keyed on a
             PRE-fold wire shape is a stale-image detector, and being unmatchable
             by a current image IS its assertion -- so FOREIGN is a READ-THIS
             class, not automatically a defect.

DEAD or FOREIGN on a FORBID is not a false red; it is simply not coverage, and a
clean row there means nothing.  On a REQUIRE it is worse: the rule will red a
healthy boot on CONFIGURATION rather than on behaviour.  Both are reported; only
a rule that scored nothing BECAUSE nothing could score it moves the exit code.

VERDICT.  The capture verdict is mbench's, unchanged, and this tool never turns a
FAIL into a PASS.  It adds one code:

    0 PASS   1 FAIL   2 usage/spec error   3 TRUNCATED   (all mbench's)
    4 PASS-BUT-VACUOUS -- the capture passed, but at least one FAILABLE rule
      (REQUIRE/FORBID) could not have fired on this image, so the pass is not the
      coverage the table appears to claim.  `--no-coverage-gate` restores the
      plain mbench code.

Typical use, the moment a capture lands:

    scripts/orin-specscore.py ~/jetson-serial.log \\
        --spec scripts/specs/jetson-sync1.spec \\
        --image ~/unaos-bench/flash/orin/conwin1-20260901T0031Z-93825ea/

`--image` accepts the staged flash directory (it finds `kernel.elf`) or the ELF
itself.  Without `--image` the tool is exactly `mbench --replay` plus a warning
that the reachability column is the whole point.
"""

import argparse
import os
import re
import sys

# Importing mbench would drop a `scripts/__pycache__/` into the repo, and `.gitignore`
# has no rule for it -- an untracked directory appearing in `git status` every time the
# bench scores a capture is how a real change gets missed in the noise.
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mbench  # noqa: E402  -- the semantics of record; never re-implemented here

RC_VACUOUS = 4

# Reachability classes, in the order they are reported.
IMG, WIRE, DEAD, FOREIGN, NOANCHOR = "IMG", "WIRE", "DEAD", "FOREIGN", "NO-ANCHOR"

ANCHOR_MIN = 5         # a cover contributing less than this is not evidence
SHORT = 4              # a field value below this is noise, not a `.rodata` string
MAX_BRANCHES = 64      # alternation expansion bound

# A cover must account for MOST of the run, not merely ANCHOR_MIN of it.  An
# absolute floor lets a 33-character run be "covered" by 15 characters of generic
# chunk and field credit borrowed from an unrelated emitter -- which is how
# `[wedge4] preempt-in-section core=` found a home in `:: PIUSB: [piusb40]
# readcap-wedge — err={:?} data=[...]`.  Half the run is the threshold.
MIN_COVER_FRAC = 0.5

# A literal run shorter than this is not evidence either way -- `-> ` and `::`
# occur in every image -- so runs below it are dropped before classification.
RUN_MIN = 3

META = set(".^$*+?()[]{}|\\")


def literal_runs(pat):
    """The maximal LITERAL substrings of a regex -- the text the wire must carry
    verbatim.  Character-class contents and `{n,m}` counts are not literal, and
    an escaped metacharacter (`\\[`) is."""
    runs, cur, i, n = [], [], 0, len(pat)
    while i < n:
        c = pat[i]
        if c == "\\" and i + 1 < n:
            nxt = pat[i + 1]
            if nxt in "[]().*+?^$|{}\\/-":       # an escaped literal
                cur.append(nxt)
            # else: a class shorthand (\d, \s, \S ...) -- ends the run
            else:
                runs.append("".join(cur))
                cur = []
            i += 2
            continue
        if c == "[":                              # character class: not literal
            runs.append("".join(cur))
            cur = []
            j = i + 1
            if j < n and pat[j] == "^":
                j += 1
            if j < n and pat[j] == "]":
                j += 1
            while j < n and pat[j] != "]":
                j += 2 if pat[j] == "\\" else 1
            i = j + 1
            continue
        if c == "{":                              # repetition count: not literal
            runs.append("".join(cur))
            cur = []
            j = pat.find("}", i)
            i = (j + 1) if j != -1 else i + 1
            continue
        if c in META:                             # group / alternation / quantifier
            runs.append("".join(cur))
            cur = []
            i += 1
            continue
        cur.append(c)
        i += 1
    runs.append("".join(cur))
    return [r for r in runs if len(r.strip()) >= RUN_MIN]


def rust_string_literals(text):
    """Yield the CONTENTS of every string literal in Rust source, skipping
    comments.  The source is the authority on what text the kernel CAN emit, but
    only its string literals are -- this tree's comments quote wire text
    constantly (`// no [wedge4] preempt-in-section line`), and a rule backed only
    by a comment is backed by nothing."""
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            i = n if j == -1 else j + 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            j, depth = i + 2, 1                     # Rust block comments nest
            while j < n and depth:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            i = j
            continue
        if c == "r" and i + 1 < n and text[i + 1] in '"#':   # raw string
            j = i + 1
            hashes = 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                close = '"' + "#" * hashes
                k = text.find(close, j + 1)
                if k == -1:
                    break
                yield text[j + 1:k]
                i = k + len(close)
                continue
        if c == '"':
            j, buf = i + 1, []
            while j < n:
                if text[j] == "\\":
                    # keep the escape's payload out of the literal text: a `\n`
                    # in the source is a newline on the wire, never the two
                    # characters a pattern could match.
                    j += 2
                    buf.append("\x00")
                    continue
                if text[j] == '"':
                    break
                buf.append(text[j])
                j += 1
            yield "".join(buf)
            i = j + 1
            continue
        i += 1


def load_source(root):
    """Every string literal in the kernel source, paired with the `.rodata` chunks
    it leaves behind.  The source is the authority on what text this kernel CAN
    print; the image is the authority on which of those sites survived the build."""
    out = []
    for dirpath, _dirs, files in os.walk(root):
        for f in files:
            if not f.endswith(".rs"):
                continue
            p = os.path.join(dirpath, f)
            with open(p, "rb") as fh:
                text = fh.read().decode("utf-8", "replace")
            for lit in rust_string_literals(text):
                chs = chunks_of(lit)
                if chs:
                    out.append((lit, chs))
    return out


_FIELD = re.compile(r"\{[^{}]*\}")


def chunks_of(literal):
    """The literal text a format string leaves in `.rodata`: everything outside
    its `{}` fields.  `\\x00` marks an escape the scanner could not render, and
    is a boundary for the same reason a field is."""
    out = []
    for part in _FIELD.split(literal):
        out.extend(p for p in part.split("\x00") if p)
    return out


def coverable(run, chunks, blob, img):
    """Can the wire produced by this format string contain `run` verbatim?

    The wire is chunk0 + value0 + chunk1 + value1 + ...  `run` is a contiguous
    slice of that, so it must be either wholly inside one chunk, or a suffix of
    one chunk followed by whole chunks (values between them) and ending inside a
    value or at a prefix of the next chunk.

    THE CONDITION THAT MAKES THIS AN ORACLE AND NOT A TAUTOLOGY: the format
    string must contribute at least ANCHOR_MIN characters of its OWN literal
    text to the cover.  Without it every pattern is "coverable" by every format
    string that has a field in it -- a field expands to arbitrary text, so a run
    lying entirely inside one would always match.  That is the check that keeps
    a rule written against a shape the source no longer emits (the pre-fold
    `…live); first key`, where the current format puts `path=…; ` in between)
    from being explained away as ordinary print-time composition."""
    for c in chunks:
        if run in c:
            return True, len(run)

    def value_credit(text):
        """A `{}` field's VALUE is often itself a string literal -- `"FROZEN"`,
        `"caller-pinned"`, `"RAST-PAINTED-OVERWRITTEN"` -- so it is `.rodata` too
        and counts toward the cover.  `[orinrast] census … -> {}` contributes only
        the 4-byte chunk `' -> '`; it is the VERDICT WORD after it that makes the
        match specific, and without this credit that rule reads as unemittable.

        THE VALUE MUST BE IN THE IMAGE, not merely in the source.  Crediting a
        source-only value is how `TEGRA-SD: REFUSED to publish` -- whose whole
        emitter is `#[cfg]`-erased from both staged images -- talks its way into
        looking reachable: the words exist in the tree, just not in the build."""
        t = text.strip()
        if len(t) < SHORT or t not in blob:
            return 0
        return len(t) if t.encode("utf-8", "replace") in img else 0

    n = len(chunks)
    for k in range(n):
        ck = chunks[k]
        for a in range(len(ck)):
            head = ck[a:]
            if not head or not run.startswith(head):
                continue
            mass, pos, ci = len(head), len(head), k + 1
            while pos < len(run) and ci < n:
                idx = run.find(chunks[ci], pos)
                if idx == -1:
                    break                       # run ends inside this field
                mass += len(chunks[ci]) + value_credit(run[pos:idx])
                pos = idx + len(chunks[ci])
                ci += 1
            if pos < len(run):                  # trailing field text
                mass += value_credit(run[pos:])
            if mass >= max(ANCHOR_MIN, MIN_COVER_FRAC * len(run)):
                return True, mass
    return False, 0


# A format string's FINGERPRINT is its LONGEST literal chunk, and that is the only
# chunk worth testing against the image.  Its short chunks (` ::`, ` (`, ` -> `,
# ` present=`) are shared with half the tree, so "some chunk of this emitter is in
# the image" is satisfied by coincidence -- which is exactly how the `orinconwin`
# format read as present in an image that does not contain `[orinconwin] win=`.


GRAM = 4


def build_index(literals):
    """gram -> literal indices.  Without it every run is compared against all
    22k source literals and a bench run takes the better part of a minute."""
    idx = {}
    for i, (_lit, chs) in enumerate(literals):
        for c in chs:
            for k in range(len(c) - GRAM + 1):
                idx.setdefault(c[k:k + GRAM], set()).add(i)
    return idx


def emitter_status(run, literals, img, blob, index):
    """(class, detail) for ONE literal run.

    FOREIGN  no format string in the kernel source can put this text on the wire
    DEAD     one can, but none of those format strings is in this image
    WIRE     a format string that can produce it IS in this image"""
    near = set()
    for k in range(len(run) - GRAM + 1):
        near |= index.get(run[k:k + GRAM], frozenset())
    candidates = []
    for i in near:
        lit, chs = literals[i]
        ok, _mass = coverable(run, chs, blob, img)
        if ok:
            candidates.append((lit, chs))
    if not candidates:
        return FOREIGN, (f"no format string in the kernel source can put {run!r} on the wire -- "
                         f"this text is not something this kernel emits (firmware/bootloader "
                         f"output, or a shape the source has moved away from)")
    for lit, chs in candidates:
        fingerprint = max(chs, key=len)
        if fingerprint.encode("utf-8", "replace") in img:
            return WIRE, f"emitter {_short(lit)!r} is in this image"
    best = max(candidates, key=lambda lc: max((len(c) for c in lc[1]), default=0))
    return DEAD, (f"the only site(s) that can emit {run!r} -- e.g. {_short(best[0])!r} -- "
                  f"are NOT in this image: compiled out of this build")


def _short(s, n=58):
    s = s.replace("\x00", "\\")
    return s if len(s) <= n else s[:n] + "…"


def branches(pat, depth=0):
    """Expand `(a|b)` groups into the alternative patterns they stand for.

    An alternation is a DISJUNCTION: `(CAPSTONE COMPLETE|\\[orinbsprun\\] ...)`
    fires if EITHER side can, so flattening it into one literal-run set and
    demanding all of them be present makes a live rule read DEAD.  `orinbsprun`
    is `#[cfg]`-erased from both staged images and `CAPSTONE COMPLETE` is in
    both; that REQUIRE is satisfiable and must not be reported otherwise."""
    i, n = 0, len(pat)
    while i < n:
        c = pat[i]
        if c == "\\":
            i += 2
            continue
        if c == "[":                          # skip a character class wholesale
            j = i + 1
            if j < n and pat[j] == "^":
                j += 1
            if j < n and pat[j] == "]":
                j += 1
            while j < n and pat[j] != "]":
                j += 2 if pat[j] == "\\" else 1
            i = j + 1
            continue
        if c == "(":
            j, level = i + 1, 1               # find this group's matching close
            while j < n and level:
                if pat[j] == "\\":
                    j += 2
                    continue
                if pat[j] == "[":
                    k = j + 1
                    while k < n and pat[k] != "]":
                        k += 2 if pat[k] == "\\" else 1
                    j = k + 1
                    continue
                level += (pat[j] == "(") - (pat[j] == ")")
                j += 1
            inner = pat[i + 1:j - 1]
            if inner.startswith("?"):         # (?:...) (?i) -- not an alternation
                i = j
                continue
            alts, level, start, k = [], 0, 0, 0
            while k < len(inner):             # split inner on TOP-LEVEL `|`
                ch = inner[k]
                if ch == "\\":
                    k += 2
                    continue
                if ch == "[":
                    m = k + 1
                    while m < len(inner) and inner[m] != "]":
                        m += 2 if inner[m] == "\\" else 1
                    k = m + 1
                    continue
                if ch == "(":
                    level += 1
                elif ch == ")":
                    level -= 1
                elif ch == "|" and level == 0:
                    alts.append(inner[start:k])
                    start = k + 1
                k += 1
            alts.append(inner[start:])
            if len(alts) > 1 and depth < 4:
                out = []
                for a in alts:
                    for sub in branches(pat[:i] + a + pat[j:], depth + 1):
                        out.append(sub)
                        if len(out) >= MAX_BRANCHES:
                            return out
                return out
            i = j
            continue
        i += 1
    return [pat]


_ORDER = {IMG: 0, WIRE: 1, DEAD: 2, FOREIGN: 3, NOANCHOR: 4}


def classify(pattern, img, literals, blob, index):
    """(class, detail) for one directive pattern.  The pattern is reachable if
    ANY of its alternation branches is, so branches are scored and the best one
    wins -- flattening `(CAPSTONE COMPLETE|\\[orinbsprun\\] ...)` into one run set
    and demanding all of them makes a live rule read DEAD."""
    best = None
    for b in branches(pattern):
        got = classify_branch(b, img, literals, blob, index)
        if best is None or _ORDER[got[0]] < _ORDER[best[0]]:
            best = got
        if best[0] == IMG:
            break
    return best


def classify_branch(pattern, img, literals, blob, index):
    """A branch is only as reachable as its WEAKEST literal run: the wire must
    carry every one of them, so the run that scores worst decides."""
    runs = literal_runs(pattern)
    if not runs:
        return NOANCHOR, "pattern carries no literal run -- neither artifact can speak to it"

    worst, detail = IMG, ""
    for run in runs:
        if run.encode("utf-8", "replace") in img:
            cls, why = IMG, ""
        else:
            cls, why = emitter_status(run, literals, img, blob, index)
            if cls == WIRE:
                why = (f"{run!r} is not contiguous in the image -- composed at print time, "
                       f"so only the wire can carry it and `strings` cannot confirm it; " + why)
        if _ORDER[cls] > _ORDER[worst]:
            worst, detail = cls, why
    return worst, detail


def find_kernel(path):
    if os.path.isdir(path):
        cand = os.path.join(path, "kernel.elf")
        if os.path.exists(cand):
            return cand
        raise SystemExit(f"orin-specscore: no kernel.elf under {path}")
    return path


def main():
    ap = argparse.ArgumentParser(
        description="score a jetson capture and report, per rule, whether it could have fired")
    ap.add_argument("capture", help="the serial capture to score (a finished log)")
    ap.add_argument("--spec", required=True, help="the witness spec")
    ap.add_argument("--image", help="staged flash dir, or the kernel.elf the capture came from")
    ap.add_argument("--source", default=None,
                    help="kernel source root the image was built from "
                         "(default: crates/kernel/src beside this script's repo)")
    ap.add_argument("--no-coverage-gate", action="store_true",
                    help="report vacuity but keep mbench's exit code")
    ap.add_argument("--accept-dead", default="",
                    help="comma-separated list of rules whose unfireability is known and "
                         "accepted for this image (e.g. one that keys on FIRMWARE output, which "
                         "is not in kernel.elf by construction). Each item is either a SPEC LINE "
                         "NUMBER or a SUBSTRING OF THE PATTERN; prefer the substring for anything "
                         "written down, because line numbers move whenever the spec gains a "
                         "comment. Accepted rules are still listed, they just do not trip the "
                         "coverage gate. Deliberately a HARNESS argument and NOT spec syntax -- "
                         "the exemption is the operator's, per image, and reviewable in the "
                         "command line rather than buried in the spec.")
    ap.add_argument("--quiet-optional", action="store_true",
                    help="omit OPTIONAL/PENDING rows that neither hit nor are DEAD")
    args = ap.parse_args()

    try:
        directives = mbench.parse_spec(args.spec)
    except mbench.SpecError as e:
        print(f"orin-specscore: spec error: {e}", file=sys.stderr)
        return mbench.RC_ERROR

    matcher = mbench.Matcher(directives)
    with open(args.capture, "rb") as f:
        data = f.read()
    for raw in data.splitlines():
        matcher.feed_raw(raw)
    matcher.unterminated = bool(data) and not data.endswith(b"\n")
    verdict, rc = matcher.run_verdict()

    img = literals = blob = index = None
    if args.image:
        kelf = find_kernel(args.image)
        with open(kelf, "rb") as f:
            img = f.read()
        root = args.source or os.path.join(
            os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
            "crates", "kernel", "src")
        if not os.path.isdir(root):
            raise SystemExit(f"orin-specscore: no kernel source at {root} (pass --source)")
        literals = load_source(root)
        blob = "\x00".join(lit for lit, _ in literals)
        index = build_index(literals)

    reach = {}
    if img is not None:
        for d in directives:
            if d.builtin:
                continue
            reach[id(d)] = classify(d.pattern, img, literals, blob, index)

    kinds = sorted(directives, key=lambda d: (mbench.KIND_ORDER[d.kind], d.spec_line))
    print(f"── orin-specscore ── {os.path.basename(args.capture)}"
          f"  vs  {os.path.basename(args.spec)}")
    if img is not None:
        print(f"   image: {kelf}")
    else:
        print("   image: (none given) -- reachability UNKNOWN; every '0 hits' row below "
              "may be a rule that cannot fire")
    print()

    # Accept a spec LINE NUMBER or a SUBSTRING OF THE PATTERN.  Line numbers are what
    # the report prints and are the obvious thing to copy, but they move the moment
    # anyone adds a comment to the spec -- and this file is mostly comment.  A recorded
    # exemption set keyed on line numbers silently stops matching after an edit and the
    # gate goes quiet, which is the same failure this whole tool exists to prevent, so
    # anything written down for reuse should be keyed on the pattern.
    accept_lines, accept_subs = set(), []
    for tok in args.accept_dead.split(","):
        tok = tok.strip()
        if not tok:
            continue
        bare = tok[1:] if tok[:1] == "L" and tok[1:].isdigit() else tok
        if bare.isdigit():
            accept_lines.add(int(bare))
        else:
            accept_subs.append(tok)

    def is_accepted(d):
        return d.spec_line in accept_lines or any(s in d.pattern for s in accept_subs)

    vacuous_failable, vacuous_other, vacuous_accepted = [], [], []
    for d in kinds:
        cls, detail = reach.get(id(d), (None, ""))
        failable = d.kind in ("REQUIRE", "COUNT", "FORBID")
        if (args.quiet_optional and not failable and not d.hits
                and cls != DEAD and d.kind != "COMPLETE"):
            continue
        tag = "" if cls is None else f"[{cls:^9}]"
        print(f"  {d.glyph()} {d.label():<11} {tag} {d.pattern}")
        print(f"       {d.note()}")
        if cls in (WIRE, DEAD, FOREIGN) and detail:
            print(f"       reach: {detail}")
        if cls in (DEAD, FOREIGN) and not d.hits:
            if is_accepted(d):
                vacuous_accepted.append(d)
            else:
                (vacuous_failable if failable else vacuous_other).append(d)

    req = [d for d in directives if d.kind in ("REQUIRE", "COUNT")]
    got = sum(1 for d in req if d.satisfied())
    forb = sum(d.hits for d in directives if d.kind == "FORBID")
    print("  ─────")
    print(f"  {verdict} — {got}/{len(req)} required witnesses, {forb} forbidden hit(s), "
          f"{matcher.lineno} lines scanned")

    if img is None:
        print("  ⚠ no --image: the coverage question was not asked, let alone answered.")
        return rc

    print()
    print("  ── COVERAGE ─────────────────────────────────────────────────────────")
    if vacuous_failable:
        print(f"  ❌ {len(vacuous_failable)} FAILABLE rule(s) could NOT have fired on this image."
              "  Their clean rows are")
        print("     vacuous: they scored nothing because nothing could score them.")
        for d in vacuous_failable:
            print(f"       {d.kind:<8} L{d.spec_line:<5} {d.pattern}")
    else:
        print("  ✅ every failable rule (REQUIRE/FORBID) is reachable on this image.")
    if vacuous_accepted:
        print(f"  ◦  {len(vacuous_accepted)} unfireable rule(s) accepted for this image by "
              "--accept-dead (not gated):")
        for d in vacuous_accepted:
            print(f"       {d.kind:<8} L{d.spec_line:<5} {d.pattern}")
    if vacuous_other:
        print(f"  ◦  {len(vacuous_other)} OPTIONAL/PENDING rule(s) also cannot fire on this image "
              "(they cannot fail,")
        print("     but a ⏳ on one of them can never be promoted from this image):")
        for d in vacuous_other:
            print(f"       {d.kind:<8} L{d.spec_line:<5} {d.pattern}")
    n_split = sum(1 for d in directives if reach.get(id(d), (None,))[0] == WIRE)
    if n_split:
        print(f"  ⚠  {n_split} rule(s) scored WIRE: the emitter is in the image, but the matched")
        print("     text is composed at print time, so `strings` cannot confirm it and only the")
        print("     wire can. The anchor proves the instrument is compiled in; it does not prove")
        print("     the format string joins the pieces in the order the pattern wants.")

    if vacuous_failable and rc == mbench.RC_PASS and not args.no_coverage_gate:
        print()
        print("  ❌ PASS-BUT-VACUOUS: the capture passed, but the pass does not carry the")
        print("     coverage the table appears to claim.  (--no-coverage-gate to override.)")
        return RC_VACUOUS
    return rc


if __name__ == "__main__":
    sys.exit(main())
