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
CONTROL PROBES (exit 2, NO verdict): the directory has ≥ 1 sheet; a synthetic sheet with a bad anchor and a duplicate
id is caught by the same functions that scan the real sheets.
usage: blueprint-check.py <repo root>   (exit 0 clean; 1 findings; 2 control failed or no sheets)
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
            "| BP-probe-1 | a | `unaos/arroyo` | x |\n| BP-probe-1 | b | `unaos/zz_nowhere.rs` | y |\n| BP-other-2 | c | `unaos/arroyo:99999999` | z |\n")
        f, _ = scan(root, t, ["INDEX.md", "probe.md"])
        want = ("duplicate id BP-probe-1", "zz_nowhere.rs does not exist", "does not carry this sheet's stem", "past the file's")
        return all(any(w in x for x in f) for w in want)
    finally:
        for n in ("INDEX.md", "probe.md"):
            try: os.remove(os.path.join(t, n))
            except OSError: pass
        try: os.rmdir(t)
        except OSError: pass


def main():
    root = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ".")
    bdir = os.path.join(root, "docs", "dev", "blueprint")
    if not control(root):
        print("BLUEPRINT: control FAILED — the scanner did not catch the synthetic sheet -> NO VERDICT"); return 2
    if not os.path.isdir(bdir):
        # The sheets land in the commit after the gate (2026-10-19); until then the gate says so and does not red the
        # tree. The sheets' commit removes this arm: from then on a missing directory is NO VERDICT (rc=2).
        print("BLUEPRINT: no docs/dev/blueprint/ directory yet -> OWED (the sheets' commit makes this NO VERDICT)"); return 0
    names = sorted(n for n in os.listdir(bdir) if n.endswith(".md"))
    sheets = [n for n in names if n != "INDEX.md"]
    if not sheets:
        print("BLUEPRINT: no sheets -> NO VERDICT"); return 2
    findings, ids = scan(root, bdir, names)
    for f in findings:
        print(f)
    print("BLUEPRINT: sheets=%d ids=%d anchors-checked findings=%d -> %s" % (len(sheets), len(ids), len(findings), "FAIL" if findings else "PASS"))
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
