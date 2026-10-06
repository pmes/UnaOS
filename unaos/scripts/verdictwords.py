#!/usr/bin/env python3
"""GATE-VERDICTWORDS (B472 BOOTVERDICTS, R80): nothing wears a verdict before `TESTS: deferred`.

Reads a boot wire (a bench capture), splits it into boots, and counts every `-> PASS` / `-> FAIL`
line printed before that boot's first `:: TESTS: deferred=` line. R80: nothing tests at boot, so an
arming decision says `-> armed`, `declined`, `skipped reason=` — never PASS or FAIL — and a test
lives behind `tests <name>`. Exit 0 when every boot counts zero, 1 otherwise, 2 on usage.

A boot starts at a `[      ?ms]`-prefixed line that follows a line without that prefix (the
uncalibrated-clock head every boot prints first), or at the top of the file. A boot that never
printed `TESTS: deferred` is counted up to its first `:: TESTS: run` (or its end) and says so.

  verdictwords.py <wire.log> [--max N]   # --max: shrink-only bound (default 0)
  verdictwords.py --selftest             # planted lines: must count 2 then 0
"""
import re
import sys

VERDICT = re.compile(r"-> (PASS|FAIL)\b")
DEFERRED = ":: TESTS: deferred="
RUN = ":: TESTS: run "
UNCAL = re.compile(r"^[\s\x00]*\[\s*\?ms\]")
TAG = re.compile(r":: ([A-Za-z0-9][A-Za-z0-9_.+-]*)")


def boots(lines):
    cur, prev_uncal = [], False
    for i, ln in enumerate(lines, 1):
        unc = bool(UNCAL.match(ln))
        if unc and not prev_uncal and cur:
            yield cur
            cur = []
        cur.append((i, ln))
        prev_uncal = unc
    if cur:
        yield cur


def scan(lines):
    out = []
    for n, boot in enumerate(boots(lines), 1):
        hits, end, how = [], None, "absent"
        for i, ln in boot:
            if DEFERRED in ln:
                end, how = i, "deferred"
                break
            if RUN in ln:
                end, how = i, "run"
                break
            if VERDICT.search(ln):
                m = TAG.search(ln)
                hits.append((i, m.group(1) if m else "?", ln.rstrip("\n")))
        out.append((n, boot[0][0], end, how, hits))
    return out


def report(path, lines, cap):
    worst = 0
    for n, start, end, how, hits in scan(lines):
        tags = sorted({t for _, t, _ in hits})
        print(f"verdictwords: {path} boot={n} from={start} until={how}@{end if end else '-'} "
              f"before_deferred={len(hits)} tags={len(tags)} [{' '.join(tags)}]")
        for i, t, ln in hits:
            print(f"  {i}: {ln.strip()[:160]}")
        worst = max(worst, len(hits))
    ok = worst <= cap
    print(f"verdictwords: worst={worst} max={cap} -> {'clean' if ok else 'REFUSED'}")
    return 0 if ok else 1


def selftest():
    planted = [
        "[      ?ms] :: video: edid present=0 ::\n",
        "[08:00:00Z] :: PLANT: armed line -> PASS ::\n",
        "[08:00:01Z] :: PLANT2: x=1 -> FAIL ::\n",
        "[08:00:01Z] :: PLANT3: x=1 -> armed ::\n",
        "[08:00:02Z] :: TESTS: deferred=1 fire=tests at_boot=0 ::\n",
        "[08:00:03Z] :: PLANT4: after -> PASS ::\n",
        "[      ?ms] :: video: edid present=0 ::\n",
        "[09:00:00Z] :: PLANT5: x=1 -> declined ::\n",
        "[09:00:02Z] :: TESTS: deferred=1 fire=tests at_boot=0 ::\n",
    ]
    r = scan(planted)
    got = [len(h) for *_, h in r]
    tags = [t for _, t, _ in r[0][4]]
    ok = got == [2, 0] and tags == ["PLANT", "PLANT2"]
    print(f"verdictwords selftest: boots={len(r)} counts={got} tags={tags} -> {'ok' if ok else 'BROKEN'}")
    return 0 if ok else 1


def main(argv):
    if argv[1:2] == ["--selftest"]:
        return selftest()
    if len(argv) < 2:
        print(__doc__)
        return 2
    cap = 0
    if "--max" in argv:
        cap = int(argv[argv.index("--max") + 1])
    path = argv[1]
    with open(path, encoding="utf-8", errors="replace") as f:
        return report(path, f.readlines(), cap)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
