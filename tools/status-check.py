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
#   T6 (CAPTUREPIN, B476) a capture log (f<N>-boot*.log; Orin render<N>-*.log) is wire only when
#      docs/dev/evidence/CAPTURES.pin holds its sha256 and size (unaos/scripts/capture-pin.sh writes the row at the
#      bench); an unpinned or edited log fails, a pin naming no log fails as stale.
#   STATUSWIRE (B476): a flight that HAS a pinned log is quoted from the log; a quote found only in the FLIGHT<N>.md
#      prose fails. A flight with no log (11, 3-7) is still quoted from its FLIGHT md.
#   STATUSORIN (B476): flight `r<N>` (r3b for render3b) is Orin render N: orin*/**/render<N>-*.log,
#      boot-render<N>-*.log and FLIGHT-RESULT-render<N>.md.
#   STATUSWORDS (B476): T5's words also read `verified on (the) metal`, `PASS(ED) on (the / its first) metal`,
#      `PASS(ED)(,) (on) flight N`.
#   tools/status-check.py --selftest   prove T2/T3/T4/T5/T6 go red on fixtures (a line in no log MUST fail)
#   tools/status-check.py --flagged    print every claiming cell without a citation (baseline or not)
#   tools/status-check.py --baseline   print the baseline the current tree would need (for the seat)
import glob
import hashlib
import os
import re
import shutil
import sys
import tempfile

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
HEADER = ["id", "claim", "status", "flight", "line", "set-by", "row-refs"]
STATUSES = {"open", "confirmed", "refuted", "parked", "unflown"}
WORDS = re.compile(r"\b(never flew|flew|unflown|proven|confirmed|refuted|landed on metal"
                   # STATUSWORDS (B476, GATEREVIEW S4): a metal verdict in other words is the same claim
                   r"|verified on (?:the )?metal|pass(?:ed)? on (?:the |its first )?metal|pass(?:ed)?,? (?:on )?flight \d+)\b",
                   re.I)
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


FLIGHT_RE = re.compile(r"f\d+|r\d+[a-z]?")  # STATUSORIN (B476): `r<N>` is Orin render N (render3b -> r3b)


def is_log(path):
    """A capture LOG (wire, pinned by CAPTUREPIN) as opposed to a FLIGHT*.md (the bench's prose about it)."""
    b = os.path.basename(path)
    return bool(re.fullmatch(r"f\d+-boot[^/]*\.log", b) or re.fullmatch(r"(?:boot-)?render\d+[a-z]?-[^/]*\.log", b)
                or re.fullmatch(r"render\d+[a-z]?-boot[^/]*\.log", b))


def flight_files(root, fl):
    """Every capture of flight `fl` (`f<N>` rMBP, `r<N>` Orin render N; a bare int is `f<N>`), sorted."""
    if isinstance(fl, int):
        fl = "f%d" % fl
    out = []
    if fl.startswith("r"):  # STATUSORIN: orin*/ render<N>-*.log, boot-render<N>-*.log, FLIGHT-RESULT-render<N>.md
        rid = fl[1:]
        for p in glob.glob(os.path.join(root, "docs/dev/evidence/orin*/**/*"), recursive=True):
            b = os.path.basename(p)
            if re.fullmatch(r"(?:boot-)?render%s-[^/]*\.log" % re.escape(rid), b) or b == "FLIGHT-RESULT-render%s.md" % rid:
                out.append(p)
        return sorted(out)
    n = int(fl[1:])
    for p in glob.glob(os.path.join(root, "docs/dev/evidence/**/*"), recursive=True):
        b = os.path.basename(p)
        m = re.fullmatch(r"f(\d+)-boot[^/]*\.log", b)
        if m and int(m.group(1)) == n:
            out.append(p)
            continue
        m = re.fullmatch(r"FLIGHT(\d+)(?:-(\d+))?(?:-POSTMORTEM)?\.md", b)  # GATEREVIEW F7: flights 3-7 are postmortems
        if m and int(m.group(1)) <= n <= int(m.group(2) or m.group(1)):
            out.append(p)
    return sorted(out)


PIN = "docs/dev/evidence/CAPTURES.pin"


def load_pins(root, errs):
    """CAPTUREPIN (B476, GATEREVIEW S6): rel -> (sha256, bytes) from the bench's pin file (unaos/scripts/capture-pin.sh)."""
    pins = {}
    path = os.path.join(root, PIN)
    if not os.path.exists(path):
        errs.append("%s: missing — no capture log is wire until the bench pins it" % PIN)
        return pins
    for i, ln in enumerate(open(path, encoding="utf-8"), 1):
        if not ln.strip() or ln.startswith("#"):
            continue
        c = ln.split()
        if len(c) < 4 or not re.fullmatch(r"[0-9a-f]{64}", c[0]) or not c[1].isdigit():
            errs.append("%s:%d: a pin row is `<sha256> <bytes> <path> <provenance…>`" % (PIN, i))
            continue
        if c[2] in pins:
            errs.append("%s:%d: %s pinned twice" % (PIN, i, c[2]))
        pins[c[2]] = (c[0], int(c[1]))
    return pins


def all_logs(root):
    return sorted(p for p in glob.glob(os.path.join(root, "docs/dev/evidence/**/*.log"), recursive=True) if is_log(p))


def check_pins(root, errs):
    """Every capture log must carry a pin row whose sha256 and size match; a pin naming no file is stale."""
    pins = load_pins(root, errs)
    good = set()
    for p in all_logs(root):
        rel = os.path.relpath(p, root)
        blob = open(p, "rb").read()
        pin = pins.get(rel)
        if pin is None:
            errs.append("%s: capture log NOT PINNED — a log is wire only when the bench pins it (unaos/scripts/capture-pin.sh)" % rel)
        elif pin != (hashlib.sha256(blob).hexdigest(), len(blob)):
            errs.append("%s: capture log does not match its pin (sha256/bytes) — the log was edited after capture" % rel)
        else:
            good.add(p)
    for rel in sorted(set(pins) - {os.path.relpath(p, root) for p in all_logs(root)}):
        errs.append("%s: pin row %s names no capture log (stale) — delete the row" % (PIN, rel))
    return good


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
    pinned = check_pins(root, errs)
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
        if fl and not FLIGHT_RE.fullmatch(fl):
            errs.append("%s: flight '%s' is not f<n> (rMBP) or r<n> (Orin render n)" % (where, fl))
            fl = ""
        if st in ("confirmed", "refuted") and (not fl or not line):
            errs.append("%s: %s requires a flight AND a quoted wire line" % (where, st))
        if st == "unflown" and fl:
            errs.append("%s: unflown carries no flight (it names %s)" % (where, fl))
        if line and not fl and st != "unflown":
            errs.append("%s: a quoted line needs its flight" % where)
        if line and len(line.strip()) < 20:  # GATEREVIEW F6: `[` is in every capture; a quote must identify a line
            errs.append("%s: quoted line '%s' is under 20 characters — quote the whole wire line" % (where, line))
        if line and fl:
            if fl not in cache:
                fs = flight_files(root, fl)
                cache[fl] = ([open(p, "rb").read() for p in fs if is_log(p) and p in pinned],
                             [open(p, "rb").read() for p in fs if not is_log(p)], [p for p in fs if is_log(p)])
            logs, prose, anylog = cache[fl]
            q = line.encode("utf-8")
            if not logs and not prose:
                errs.append("%s: flight %s has no capture in git (f<N>-boot*.log / FLIGHT<N>.md; Orin: render<N>-*.log / FLIGHT-RESULT-render<N>.md)%s"
                            % (where, fl, " — its logs are not pinned" if anylog else ""))
            elif anylog and not any(q in b for b in logs):
                # STATUSWIRE (B476, GATEREVIEW S2): a flight that has a log is quoted from the log, never from the prose
                errs.append("%s: line NOT on the wire of %s%s: %s" % (
                    where, fl, " (it is in the FLIGHT prose only — quote the log line)" if any(q in b for b in prose) else "", line[:120]))
            elif not anylog and not any(q in b for b in prose):
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
    def pinrow(d, rel):
        b = open(os.path.join(d, rel), "rb").read()
        with open(os.path.join(d, PIN), "a") as f:
            f.write("%s %d %s by=fixture\n" % (hashlib.sha256(b).hexdigest(), len(b), rel))

    def tree(tsv, ledger_status, log_line="[  1ms] :: real wire line ::", extra=None):
        d = tempfile.mkdtemp(prefix="statuscheck-")
        os.makedirs(os.path.join(d, "docs/dev/OS"))
        os.makedirs(os.path.join(d, "docs/dev/evidence/r/flight7"))
        os.makedirs(os.path.join(d, "docs/dev/evidence/orin16"))
        with open(os.path.join(d, "docs/dev/evidence/r/flight7/f7-boots.log"), "w") as f:
            f.write("noise\n%s\nmore\n" % log_line)
        with open(os.path.join(d, "docs/dev/evidence/r/flight7/FLIGHT7.md"), "w") as f:
            f.write("- prose: `only the bench prose says this` and `:: real wire line ::`\n")
        with open(os.path.join(d, "docs/dev/evidence/r/FLIGHT8.md"), "w") as f:
            f.write("- prose: `a flight eight line with no log at all`\n")
        with open(os.path.join(d, "docs/dev/evidence/orin16/render8-boot1.log"), "w") as f:
            f.write(":: PRTSCR: SCREEN3.PNG 1920x1200 6913793 bytes -> OK ::\n")
        pinrow(d, "docs/dev/evidence/r/flight7/f7-boots.log")
        pinrow(d, "docs/dev/evidence/orin16/render8-boot1.log")
        if extra:
            extra(d)
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
        ("vacuous short quote", "ST1\tc\tconfirmed\tf7\t[\ts\tB1\n", "open", 1, "under 20"),
        ("baseline grandfathers", ok_row, "open — flew once", 0, None, "rmbp-ledger.md B1"),
        ("baseline stale once cited", ok_row, "open — flew ST1", 1, "stale", "rmbp-ledger.md B1"),
        # B476 GATEFIX: the GATEREVIEW plants S2, S4, S6 and the Orin model
        ("S4 verified on metal", ok_row, "flown — verified on metal, PASS on flight 7", 1, "says 'verified on metal'"),
        ("S4 PASS on flight N", ok_row, "flown — the fixture printed PASS, flight 7", 1, "says 'pass, flight 7'"),
        ("S2 prose-only quote", "ST1\tc\tconfirmed\tf7\tonly the bench prose says this\ts\tB1\n", "open", 1, "FLIGHT prose only"),
        ("prose is the capture when no log", "ST1\tc\tconfirmed\tf8\ta flight eight line with no log at all\ts\tB1\n", "open", 0, None),
        ("S6 unpinned hand-made log", "ST1\tc\tconfirmed\tf7\t:: a typed line in a typed log ::\ts\tB1\n", "open", 1, "NOT PINNED",
         None, lambda d: open(os.path.join(d, "docs/dev/evidence/r/flight7/f7-boot9.log"), "w").write(":: a typed line in a typed log ::\n")),
        ("log edited after its pin", ok_row, "open", 1, "does not match its pin",
         None, lambda d: open(os.path.join(d, "docs/dev/evidence/r/flight7/f7-boots.log"), "a").write("edit\n")),
        ("stale pin", ok_row, "open", 1, "names no capture log",
         None, lambda d: open(os.path.join(d, PIN), "a").write("%s 1 docs/dev/evidence/gone/f9-boot1.log by=fixture\n" % ("0" * 64))),
        ("Orin r8 quote", "ST1\tc\tconfirmed\tr8\tSCREEN3.PNG 1920x1200 6913793 bytes -> OK\ts\tB1\n", "open", 0, None),
        ("Orin r8 line in no log", "ST1\tc\tconfirmed\tr8\tSCREEN9.PNG 1920x1200 never printed\ts\tB1\n", "open", 1, "NOT on the wire of r8"),
        ("Orin r3 is not r3b", "ST1\tc\tconfirmed\tr3\tSCREEN3.PNG 1920x1200 6913793 bytes -> OK\ts\tB1\n", "open", 1, "no capture"),
    ]
    fails = 0
    pm = tempfile.mkdtemp(prefix="statuscheck-")  # GATEREVIEW F7: a flight-5 postmortem is that flight's capture
    os.makedirs(os.path.join(pm, "docs/dev/evidence/rmbp9"))
    open(os.path.join(pm, "docs/dev/evidence/rmbp9/FLIGHT5-POSTMORTEM.md"), "w").write("x\n")
    if len(flight_files(pm, 5)) != 1:
        fails += 1
        print("status-check selftest: FAIL postmortem capture — FLIGHT5-POSTMORTEM.md not read as flight 5")
    shutil.rmtree(pm, ignore_errors=True)
    for name, tsv, cell, want, why, *bl in cases:
        d = tree(tsv, cell, extra=bl[1] if len(bl) > 1 else None)
        bp = None
        if bl and bl[0]:
            bp = os.path.join(d, "docs/dev/STATUS.baseline")
            with open(bp, "w") as f:
                f.write("# fixture\n%s\n" % bl[0])
        errs, _r, _f, _b = check(d, bp)
        shutil.rmtree(d, ignore_errors=True)
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
