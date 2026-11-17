#!/usr/bin/env python3
"""blueprint-check.py — GATE-BLUEPRINT: the architecture sheets under docs/dev/blueprint/ stay true to the tree.

WHY (Peter, 2026-10-19: "do we have a set of blueprints for you and the executors to look over? it should keep the os
architecture stable and work as a front-side gate so that coding efforts are not wasted on some accidental tangent" /
"the blueprints need to be easily digestible so that only what's necessary is contextualized"). A sheet an arc reads
before its first edit is a claim about the tree; a claim that ages silently is worse than none (LAWS §5, wire beats
comment). This gate makes the sheets mechanical: every anchor they cite exists at the cited line, every invariant id
is unique and well-formed, every sheet is listed and short, and the status vocabulary rule holds in them.

WHAT IT CHECKS (exit 1 on any finding; each finding is one line `blueprint: <file>:<line> <what>`):
  1. docs/dev/blueprint/INDEX.md exists and lists every `*.md` sheet in the directory (and nothing that is not there).
  2. Every sheet is at most MAX_LINES lines (digestible: an executor reads only its sheets, whole).
  3. Every invariant id matches `BP-<sheet>-<n>` where <sheet> is the sheet's own file stem; ids are unique
     across all sheets; a sheet has at least one.
  4. Every backticked anchor that names a tree path (`unaos/…`, `libs/…`, `handlers/…`, `vessels/…`, `tools/…`,
     `docs/…`, `builder/…`, optionally `:<line>` or `:<a>-<b>`) exists; a cited line is within the file's length.
  5. No `proven` / `verified` / `confirmed` in a sheet (the status-check vocabulary: those words need an ST row).
  6. No `- ` row holds a copy group (BLUEPRINTDEDUPONFOLD, LEDGER SR1020): blueprint-dedup.py's `copy_groups()`, imported by
     path, finds none in the row save a refused group named on KNOWN_REFUSED; INDEX.md's "§5 row grammar" quotes that
     detector's regexes verbatim (the doc is the regexes: an edit to one without the other is a finding).
CONTROL PROBES (exit 2, NO verdict): the directory has ≥ 1 sheet; a synthetic sheet with a bad anchor and a duplicate
id (and a row with a clause pasted twice) is caught by the same functions that scan the real sheets.
usage: blueprint-check.py <repo root>   (exit 0 clean; 1 findings; 2 control failed or no sheets)
       blueprint-check.py [<repo root>] --self-test   (check 6 on a fixture: handlers.md's AETHERMAC-RUN row with a clause copied)
"""
import os, re, sys

MAX_LINES = 120
ROOTS = ("unaos/", "libs/", "handlers/", "vessels/", "tools/", "docs/", "builder/")
ANCHOR = re.compile(r"`((?:%s)[^`\s:]+)(?::(\d+)(?:-(\d+))?)?`" % "|".join(r.rstrip("/") for r in ROOTS))
BPID = re.compile(r"\bBP-([a-z0-9]+)-(\d+)\b")
CLAIM = re.compile(r"\b(proven|verified|confirmed)\b", re.I)


def scan(root, bdir, names):
    """Return the findings for the sheets `names` (file names) under `bdir`."""
    out, seen = [], {}
    index = os.path.join(bdir, "INDEX.md")
    if not os.path.isfile(index):
        return ["blueprint: INDEX.md missing"], seen
    itext = open(index, encoding="utf-8").read()
    out.extend(grammar_quoted(itext))
    for n in names:
        if n != "INDEX.md" and n not in itext:
            out.append("blueprint: INDEX.md:0 sheet %s not listed" % n)
    for m in re.finditer(r"\b([a-z0-9-]+\.md)\b", itext):
        if m.group(1) != "INDEX.md" and m.group(1) not in names:
            out.append("blueprint: INDEX.md:0 lists %s which does not exist" % m.group(1))
    for n in names:
        p = os.path.join(bdir, n)
        lines = open(p, encoding="utf-8").read().split("\n")
        if len(lines) > MAX_LINES:
            out.append("blueprint: %s:%d sheet is %d lines, max %d" % (n, len(lines), len(lines), MAX_LINES))
        stem = n[:-3].lower()
        ids_here = 0
        for i, l in enumerate(lines, 1):
            if n != "INDEX.md" and CLAIM.search(l):
                out.append("blueprint: %s:%d claim word %r (no ST row in a sheet)" % (n, i, CLAIM.search(l).group(1)))
            if n != "INDEX.md" and l.startswith("- "): out.extend(rowcopies(n, i, l))
            for m in BPID.finditer(l):
                sid, num = m.group(1), m.group(2)
                if n == "INDEX.md":
                    continue
                key = "BP-%s-%s" % (sid, num)
                if l.lstrip().startswith("|") and l.lstrip("| ").startswith(key):
                    # a definition row: the first cell is the id
                    if sid != stem:
                        out.append("blueprint: %s:%d id %s does not carry this sheet's stem %r" % (n, i, key, stem))
                    if key in seen:
                        out.append("blueprint: %s:%d duplicate id %s (first at %s)" % (n, i, key, seen[key]))
                    seen[key] = "%s:%d" % (n, i)
                    ids_here += 1
            for m in ANCHOR.finditer(l):
                path, a, b = m.group(1), m.group(2), m.group(3)
                f = os.path.join(root, path)
                if not os.path.exists(f):
                    out.append("blueprint: %s:%d anchor %s does not exist" % (n, i, path))
                    continue
                if a and os.path.isfile(f):
                    try:
                        nl = sum(1 for _ in open(f, "rb"))
                    except OSError:
                        nl = 0
                    hi = int(b) if b else int(a)
                    if int(a) < 1 or hi > nl:
                        out.append("blueprint: %s:%d anchor %s:%s past the file's %d lines" % (n, i, path, a if not b else a + "-" + b, nl))
        if n != "INDEX.md" and ids_here == 0:
            out.append("blueprint: %s:0 no BP-%s-<n> invariant defined" % (n, stem))
    return out, seen


def control(root):
    import tempfile
    t = tempfile.mkdtemp(prefix="bpctl.", dir=os.path.join(root, "unaos", "target") if os.path.isdir(os.path.join(root, "unaos", "target")) else None)
    try:
        open(os.path.join(t, "INDEX.md"), "w").write("probe.md\n")
        open(os.path.join(t, "probe.md"), "w").write(
            "| BP-probe-1 | a | `unaos/arroyo` | x |\n| BP-probe-1 | b | `unaos/zz_nowhere.rs` | y |\n| BP-other-2 | c | `unaos/arroyo:99999999` | z |\n"
            "- PROBE-1 (SR1 owed): PROBE-2 BUILT by SR9 (one two three four five six seven eight nine ten); PROBE-2 BUILT by SR9 (one two three four five six seven eight nine ten).\n")
        try:
            f, _ = scan(root, t, ["INDEX.md", "probe.md"])
        except Exception:
            return False   # blueprint-dedup.py not importable: no verdict
        want = ("duplicate id BP-probe-1", "zz_nowhere.rs does not exist", "does not carry this sheet's stem", "past the file's", "copy group")
        return all(any(w in x for x in f) for w in want)
    finally:
        for n in ("INDEX.md", "probe.md"):
            try: os.remove(os.path.join(t, n))
            except OSError: pass
        try: os.rmdir(t)
        except OSError: pass


def main():
    root = os.path.abspath(([a for a in sys.argv[1:] if not a.startswith("--")] or ["."])[0])
    bdir = os.path.join(root, "docs", "dev", "blueprint")
    if not control(root):
        print("BLUEPRINT: control FAILED — the scanner did not catch the synthetic sheet -> NO VERDICT"); return 2
    if not os.path.isdir(bdir):
        print("BLUEPRINT: no docs/dev/blueprint/ directory -> NO VERDICT"); return 2
    if "--self-test" in sys.argv: return selftest(root, bdir)
    names = sorted(n for n in os.listdir(bdir) if n.endswith(".md"))
    sheets = [n for n in names if n != "INDEX.md"]
    if not sheets:
        print("BLUEPRINT: no sheets -> NO VERDICT"); return 2
    findings, ids = scan(root, bdir, names)
    for f in findings:
        print(f)
    print("BLUEPRINT: sheets=%d ids=%d anchors-checked findings=%d -> %s" % (len(sheets), len(ids), len(findings), "FAIL" if findings else "PASS"))
    return 1 if findings else 0



# ---- BLUEPRINTDEDUPONFOLD (LEDGER SR1020): check 6, no copy group in a row (named tail block) ----
# The detector is blueprint-dedup.py's, imported by path (nonet-reach.py's idiom): ONE detector, two entry points.
# KNOWN_REFUSED: (row prefix, SR id) of a group the dedup REFUSED and kept (no drop keeps its proof); only a refused group
# of that id in that row passes. A row is deleted from here when its pair is cleaned by hand (BLUEPRINT-DEDUP-REST).
KNOWN_REFUSED = [("- NLE5b (SR204 owed)", "SR775"), ("- VUG-DOC (LEDGER SR480)", "SR663")]
GRAMMAR = ("ANCH", "STUTTER", "KEY", "SRID", "BUILTID", "BUILTKEY", "OWEDKEY")   # the regexes INDEX.md quotes
_DEDUP = []


def load_dedup():
    if not _DEDUP:
        import importlib.util
        spec = importlib.util.spec_from_file_location("blueprint_dedup", os.path.join(os.path.dirname(os.path.abspath(__file__)), "blueprint-dedup.py"))
        m = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(m)
        _DEDUP.append(m)
    return _DEDUP[0]


def rowcopies(n, i, row, allow=None):
    """Findings for the copy groups in one `- ` row of sheet n (line i), the allowlisted refused groups excepted."""
    allow = KNOWN_REFUSED if allow is None else allow
    c = row.find(":")
    pre = row[:c] if 0 < c < 80 else row[:60]
    out = []
    for an, verdict in load_dedup().copy_groups(row):
        if verdict == "refused" and any(row.startswith(p) and an.split()[-1] == sr for p, sr in allow):
            continue
        out.append("blueprint: %s:%d copy group %r (%s) in row %r: run blueprint-dedup.py on it" % (n, i, an, verdict, pre))
    return out


def grammar_quoted(itext):
    """A finding per detector regex whose pattern INDEX.md does not quote verbatim in backticks."""
    dd = load_dedup()
    return ["blueprint: INDEX.md:0 the §5 row grammar does not quote %s's pattern `%s` (blueprint-dedup.py and the doc drifted)" % (k, getattr(dd, k).pattern)
            for k in GRAMMAR if "`%s`" % getattr(dd, k).pattern not in itext]


def selftest(root, bdir):
    """Check 6 on a fixture: the real handlers.md AETHERMAC-RUN row with its first `BUILT by SRnnn (...)` clause (>= 40
    chars) copied in right after itself must give ONE copy-group finding naming the row and the anchor; the real sheets
    give none; with KNOWN_REFUSED emptied they give exactly the allowlisted refused rows. exit 0 all hold, 1 otherwise."""
    dd = load_dedup()
    names = sorted(x for x in os.listdir(bdir) if x.endswith(".md"))
    real, _ = scan(root, bdir, names)
    lines = open(os.path.join(bdir, "handlers.md"), encoding="utf-8").read().split("\n")
    pre = "- AETHERMAC-RUN (SR205 owed)"
    i = [k for k, l in enumerate(lines) if l.startswith(pre)][0]
    row, pr = lines[i], dd.pairs(lines[i])
    m = [m for m in dd.ANCH.finditer(row) if row[m.end():m.end() + 2] == " (" and m.end() + 1 in pr and pr[m.end() + 1] - m.end() >= 40][0]
    e = pr[m.end() + 1] + 1
    caught = rowcopies("handlers.md", i + 1, row[:e] + "; " + row[m.start():e] + row[e:])
    bare = []
    for n in names:
        if n != "INDEX.md":
            for k, l in enumerate(open(os.path.join(bdir, n), encoding="utf-8").read().split("\n"), 1):
                if l.startswith("- "):
                    bare += rowcopies(n, k, l, allow=[])
    for f in caught + real:
        print(f)
    ok = (len(caught) == 1 and repr(pre) in caught[0] and repr(m.group(0)) in caught[0] and not real
          and len(bare) == len(KNOWN_REFUSED) and all(any(repr(p) in b and sr in b and "(refused)" in b for b in bare) for p, sr in KNOWN_REFUSED))
    print("BLUEPRINT-SELFTEST: fixture handlers.md:%d + a copy of %r -> %d finding(s); real sheets -> %d; KNOWN_REFUSED emptied -> %d refused row(s) of %d -> %s"
          % (i + 1, m.group(0), len(caught), len(real), len(bare), len(KNOWN_REFUSED), "PASS" if ok else "FAIL"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
