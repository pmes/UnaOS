#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# status-check — GATE-STATUS (STATUSTABLE, rmbp-ledger B425). Peter, 2026-10-06: "how come there isn't
# one status table to rule them all so a false claim such as in this wifi situation can never be made?"
#
# The ONE table is docs/dev/STATUS.tsv (tab-separated, header row first, no prose cells):
#   id  claim  status  flight  line  set-by  row-refs
# What this gate refuses (exit 1, each failure named):
#   T1 a malformed row: id not `ST<n>` or repeated; status outside open|confirmed|refuted|parked|unflown;
#      a flight not `f<n>`; an empty claim or set-by; a row-ref that names no ledger row.
#   T2 `confirmed` / `refuted` without BOTH a flight and a quoted line.
#   T3 a quoted line not found VERBATIM (bytes, awk index semantics) in that flight's capture:
#      docs/dev/evidence/**/f<N>-boot*.log or FLIGHT<N>.md (FLIGHT<a>-<b>.md covers a..b). A row that
#      quotes a line is checked whatever its status; a flight with no capture in git is itself a failure.
#   T4 `unflown` with a flight (a claim that never flew has no flight to cite).
#   T5 a status cell of docs/dev/OS/rmbp-ledger.md or docs/dev/LEDGER.md, or a STATE line of
#      docs/dev/QUEUE.md / docs/dev/OS/*-queue.md, that says flew / never flew / unflown / proven /
#      confirmed / refuted / landed on metal and cites no `ST<n>` id of the table. The ledger's own enum
#      word at the head of a cell (`fixed-unflown`, GATE-LEDGER's vocabulary) is not a claim; the prose
#      after it is. Cells that predate the gate are grandfathered by key in docs/dev/STATUS.baseline;
#      the baseline only shrinks: a key whose cell now cites a row (or no longer claims) is a stale
#      row and fails until it is deleted. A NEW claiming cell never gets a baseline row.
#
#   tools/status-check.py              check the tree (exit 0 clean, 1 failures, 2 control failed)
#   tools/status-check.py --selftest   prove T2/T3/T4/T5 go red on fixtures (a line in no log MUST fail)
#   tools/status-check.py --flagged    print every claiming cell without a citation (baseline or not)
#   tools/status-check.py --baseline   print the baseline the current tree would need (for the seat)
import glob
import hashlib
import os
import re
import sys
import tempfile

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
HEADER = ["id", "claim", "status", "flight", "line", "set-by", "row-refs"]
STATUSES = {"open", "confirmed", "refuted", "parked", "unflown"}
WORDS = re.compile(r"\b(never flew|flew|unflown|proven|confirmed|refuted|landed on metal)\b", re.I)
ENUM_HEAD = re.compile(r"^\W*(open|fixed-unflown|flown|landed|dropped)\b\W*", re.I)
CITE = re.compile(r"\bST(\d+)\b")
STATE_LINE = re.compile(r"^[#*\s>·-]*(?:[A-Z]+\s+)?STATE\b")
LEDGERS = ["docs/dev/OS/rmbp-ledger.md", "docs/dev/LEDGER.md"]
QUEUES = ["docs/dev/QUEUE.md"] + sorted(
    os.path.relpath(p, ROOT) for p in glob.glob(os.path.join(ROOT, "docs/dev/OS/*-queue.md")))


def split_row(line):
    # markdown table cells; `\|` inside a cell is not a separator
    body = line.strip()
    if body.startswith("|"):
        body = body[1:]
    if body.endswith("|"):
        body = body[:-1]
    return [c.strip() for c in re.split(r"(?<!\\)\|", body)]


def ledger_cells(root, rel):
    """Yield (key, row_id, status_cell) for every table row with a `status` column."""
    path = os.path.join(root, rel)
    if not os.path.exists(path):
        return
    col = None
    with open(path, encoding="utf-8", errors="replace") as f:
        for ln in f:
            if not ln.lstrip().startswith("|"):
                col = None
                continue
            cells = split_row(ln)
            low = [c.lower() for c in cells]
            if "id" in low and "status" in low:
                col = low.index("status")
                continue
            if col is None or set(ln.strip()) <= set("|-: "):
                continue
            if len(cells) <= col:
                continue
            rid = cells[0].strip("* ")
            yield ("%s %s" % (os.path.basename(rel), rid), rid, cells[col])


def ledger_ids(root):
    ids = set()
    for rel in glob.glob(os.path.join(root, "docs/dev/LEDGER.md")) + glob.glob(
            os.path.join(root, "docs/dev/OS/*-ledger.md")):
        for _k, rid, _s in ledger_cells(root, os.path.relpath(rel, root)):
            ids.add(rid)
    return ids


def queue_lines(root, rel):
    path = os.path.join(root, rel)
    if not os.path.exists(path):
        return
    with open(path, encoding="utf-8", errors="replace") as f:
        for ln in f:
            if STATE_LINE.match(ln):
                h = hashlib.sha1(ln.rstrip("\n").encode()).hexdigest()[:12]
                yield ("%s @%s" % (os.path.basename(rel), h), ln.rstrip("\n"))


def claims(text):
    m = ENUM_HEAD.match(text)
    if m and m.group(1).lower() != "open":
        text = text[m.end():]
    return [w.lower() for w in WORDS.findall(text)]


def flight_files(root, n):
    out = []
    for p in glob.glob(os.path.join(root, "docs/dev/evidence/**/*"), recursive=True):
        b = os.path.basename(p)
        m = re.fullmatch(r"f(\d+)-boot[^/]*\.log", b)
        if m and int(m.group(1)) == n:
            out.append(p)
            continue
        m = re.fullmatch(r"FLIGHT(\d+)(?:-(\d+))?\.md", b)
        if m and int(m.group(1)) <= n <= int(m.group(2) or m.group(1)):
            out.append(p)
    return sorted(out)


def load_table(root, errs):
    path = os.path.join(root, "docs/dev/STATUS.tsv")
    rows = []
    if not os.path.exists(path):
        errs.append("STATUS.tsv: missing")
        return rows
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    if not lines or lines[0].split("\t") != HEADER:
        errs.append("STATUS.tsv:1: header must be %s" % "\\t".join(HEADER))
        return rows
    for i, ln in enumerate(lines[1:], 2):
        cells = ln.split("\t")
        if len(cells) != len(HEADER):
            errs.append("STATUS.tsv:%d: %d cells, want %d" % (i, len(cells), len(HEADER)))
            continue
        rows.append((i, dict(zip(HEADER, cells))))
    return rows


def check(root, baseline_path=None, flagged_only=False, emit_baseline=False):
    errs = []
    rows = load_table(root, errs)
    known = ledger_ids(root)
    ids = {}
    cache = {}
    for i, r in rows:
        where = "STATUS.tsv:%d %s" % (i, r["id"])
        if not re.fullmatch(r"ST\d+", r["id"]):
            errs.append("%s: id must be ST<n>" % where)
        elif r["id"] in ids:
            errs.append("%s: id repeats line %d" % (where, ids[r["id"]]))
        else:
            ids[r["id"]] = i
        st, fl, line = r["status"], r["flight"], r["line"]
        if st not in STATUSES:
            errs.append("%s: status '%s' not in %s" % (where, st, "/".join(sorted(STATUSES))))
        if not r["claim"] or not r["set-by"]:
            errs.append("%s: claim and set-by are required" % where)
        if fl and not re.fullmatch(r"f\d+", fl):
            errs.append("%s: flight '%s' is not f<n>" % (where, fl))
            fl = ""
        if st in ("confirmed", "refuted") and (not fl or not line):
            errs.append("%s: %s requires a flight AND a quoted wire line" % (where, st))
        if st == "unflown" and fl:
            errs.append("%s: unflown carries no flight (it names %s)" % (where, fl))
        if line and not fl and st != "unflown":
            errs.append("%s: a quoted line needs its flight" % where)
        if line and fl:
            n = int(fl[1:])
            if n not in cache:
                cache[n] = [open(p, "rb").read() for p in flight_files(root, n)]
            if not cache[n]:
                errs.append("%s: flight %s has no capture in git (f%d-boot*.log / FLIGHT%d.md)" % (where, fl, n, n))
            elif not any(line.encode("utf-8") in blob for blob in cache[n]):
                errs.append("%s: line NOT on the wire of %s: %s" % (where, fl, line[:120]))
        for ref in [x.strip() for x in r["row-refs"].split(",") if x.strip()]:
            if ref not in known:
                errs.append("%s: row-ref %s names no ledger row" % (where, ref))

    base = set()
    if baseline_path and os.path.exists(baseline_path):
        with open(baseline_path, encoding="utf-8") as f:
            base = {l.strip() for l in f if l.strip() and not l.startswith("#")}
    flagged = []
    seen = set()
    sources = [(k, s) for rel in LEDGERS for k, _rid, s in ledger_cells(root, rel)]
    sources += [(k, s) for rel in QUEUES for k, s in queue_lines(root, rel)]
    for key, text in sources:
        seen.add(key)
        words = claims(text)
        if not words:
            continue
        cited = ["ST" + c for c in CITE.findall(text)]
        good = [c for c in cited if c in ids]
        bad = [c for c in cited if c not in ids]
        if bad:
            errs.append("%s: cites %s, not a row of STATUS.tsv" % (key, ",".join(bad)))
        if good:
            if key in base:
                errs.append("%s: cites %s now — delete its STATUS.baseline row (stale)" % (key, good[0]))
            continue
        flagged.append(key)
        if key not in base:
            errs.append("%s: says '%s' and cites no ST row of docs/dev/STATUS.tsv" % (key, words[0]))
    for key in sorted(base - set(flagged) - seen):
        errs.append("STATUS.baseline: %s names no claiming cell any more — delete the row (stale)" % key)
    for key in sorted((base & seen) - set(flagged)):
        if not any(key in e for e in errs):
            errs.append("STATUS.baseline: %s no longer claims uncited — delete the row (stale)" % key)
    if flagged_only:
        for k in flagged:
            print(k)
    if emit_baseline:
        for k in flagged:
            print(k)
    return errs, rows, flagged, base


def selftest():
    """Fixtures: each must go red for the reason named, and a good fixture must pass."""
    def tree(tsv, ledger_status, log_line="[  1ms] :: real wire line ::"):
        d = tempfile.mkdtemp(prefix="statuscheck-")
        os.makedirs(os.path.join(d, "docs/dev/OS"))
        os.makedirs(os.path.join(d, "docs/dev/evidence/r/flight7"))
        with open(os.path.join(d, "docs/dev/evidence/r/flight7/f7-boots.log"), "w") as f:
            f.write("noise\n%s\nmore\n" % log_line)
        with open(os.path.join(d, "docs/dev/OS/rmbp-ledger.md"), "w") as f:
            f.write("| id | item | owner | flies-on | status | evidence | closed by |\n|---|---|---|---|---|---|---|\n")
            f.write("| B1 | thing | rmbp | rmbp | %s | e | — |\n" % ledger_status)
        with open(os.path.join(d, "docs/dev/STATUS.tsv"), "w") as f:
            f.write("\t".join(HEADER) + "\n" + tsv)
        return d
    ok_row = "ST1\tthe thing works\tconfirmed\tf7\t:: real wire line ::\tseat 2026-10-06\tB1\n"
    cases = [
        ("good", ok_row, "open — confirmed ST1", 0, None),
        ("line in no log", "ST1\tc\tconfirmed\tf7\t:: a line nobody printed ::\ts\tB1\n", "open", 1, "NOT on the wire"),
        ("confirmed without flight", "ST1\tc\tconfirmed\t\t\ts\t\n", "open", 1, "requires a flight"),
        ("unflown with flight", "ST1\tc\tunflown\tf7\t\ts\t\n", "open", 1, "unflown carries no flight"),
        ("flight without capture", "ST1\tc\tconfirmed\tf99\t:: real wire line ::\ts\t\n", "open", 1, "no capture"),
        ("uncited cell", ok_row, "open — the upload never flew", 1, "cites no ST row"),
        ("cites missing row", ok_row, "open — flew ST9", 1, "not a row"),
        ("enum head is no claim", ok_row, "fixed-unflown — built", 0, None),
        ("bad status", "ST1\tc\tsettled\t\t\ts\t\n", "open", 1, "not in"),
    ]
    fails = 0
    for name, tsv, cell, want, why in cases:
        d = tree(tsv, cell)
        errs, _r, _f, _b = check(d, None)
        got = 1 if errs else 0
        hit = why is None or any(why in e for e in errs)
        if got != want or not hit:
            fails += 1
            print("status-check selftest: FAIL %s — want rc=%d (%s), got %s" % (name, want, why, errs))
        else:
            print("status-check selftest: ok   %s%s" % (name, (" — " + errs[0]) if errs else ""))
    if fails:
        print("status-check selftest: %d fixture(s) did not behave -> FAIL" % fails)
        return 2
    print("status-check selftest: %d/%d fixtures behave -> PASS" % (len(cases), len(cases)))
    return 0


def main(argv):
    if "--selftest" in argv:
        return selftest()
    errs, rows, flagged, base = check(ROOT, os.path.join(ROOT, "docs/dev/STATUS.baseline"),
                                      "--flagged" in argv, "--baseline" in argv)
    if "--flagged" in argv or "--baseline" in argv:
        return 0
    for e in errs:
        print("status-check: " + e)
    by = {}
    for _i, r in rows:
        by[r["status"]] = by.get(r["status"], 0) + 1
    print("status-check: rows=%d %s claiming-uncited=%d baseline=%d -> %s" % (
        len(rows), " ".join("%s=%d" % kv for kv in sorted(by.items())), len(flagged), len(base),
        "FAIL" if errs else "PASS"))
    return 1 if errs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
