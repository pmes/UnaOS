#!/usr/bin/env python3
"""NEUTRAL M2 — every SPEC RULE that names a renamed family must still resolve to a live emitter.

WHY THIS EXISTS. `x86-install.spec`'s twelve `:: PINSTALL:` rules are only replayed by
`./arroyo test-install`, a five-knob gated leg this brief does not run, and `jetson-sync1.spec`'s
three `TEGRA-SD:`/`TEGRA-UNAFS:` rules score against a capture no bench here can produce. A rule
whose gate is not run this session is exactly the rule a rename can break in silence — M1's defect
(`pending 7/32 -> 5/32`, headline verdict unchanged) was of that shape. So the pairing is asserted
STATICALLY at both ends instead: for each rule, the family prefix it pins must have a live
`serial_println!` emitter in the kernel, and the OLD prefix must have none.

This is a source-side claim, not a wire claim, and it is labelled as one. It cannot prove the line
prints; it proves the rule and the emitter did not part company.

usage: m2-rule-resolve.py <base-tree-src> <tip-src>
"""
import os, re, subprocess, sys

ROOT = os.path.dirname(os.path.abspath(__file__))
for _ in range(5):
    ROOT = os.path.dirname(ROOT)

# (spec, rule-prefix OLD, rule-prefix NEW) — the families this milestone moved that carry spec rules
PAIRS = [
    ("unaos/scripts/specs/x86-default.spec", ":: PIUSB:",      ":: USB:"),
    ("unaos/scripts/specs/x86-install.spec", ":: PINSTALL:",   ":: INSTALL:"),
    ("unaos/scripts/specs/jetson-sync1.spec", "TEGRA-SD:",     "SDMMC:"),
    ("unaos/scripts/specs/jetson-sync1.spec", "TEGRA-UNAFS:",  "UNAFS:"),
]
RULE = re.compile(r"^\s*(REQUIRE|FORBID|COUNT\s+\d+|PENDING|OPTIONAL|COMPLETE)\s+(.*)$")


def emitters(src, token):
    """live (non-comment-line) source occurrences of `token` under src, arch/ excluded."""
    n = 0
    for dp, dn, fn in os.walk(src):
        if os.sep + "arch" + os.sep in dp + os.sep:
            continue
        for f in fn:
            if not f.endswith(".rs"):
                continue
            for ln in open(os.path.join(dp, f), encoding="utf-8", errors="replace"):
                if token in ln and not ln.lstrip().startswith("//"):
                    n += 1
    return n


def rules(spec, token):
    out = []
    p = os.path.join(ROOT, spec)
    for i, ln in enumerate(open(p, encoding="utf-8"), 1):
        m = RULE.match(ln)
        if m and token in m.group(2):
            out.append((i, m.group(1), m.group(2).rstrip()))
    return out


def main():
    base, tip = sys.argv[1], sys.argv[2]
    print("NEUTRAL M2 — SPEC RULE <-> EMITTER RESOLUTION (source-side claim, not a wire claim)")
    print(f"base tree {base}\ntip tree  {tip}\n")
    bad = 0
    for spec, old, new in PAIRS:
        rold, rnew = rules(spec, old), rules(spec, new)
        eb_old, eb_new = emitters(base, old), emitters(base, new)
        et_old, et_new = emitters(tip, old), emitters(tip, new)
        print(f"== {spec}   {old!r} -> {new!r}")
        print(f"   rules still on the OLD prefix at tip : {len(rold)}   (want 0)")
        print(f"   rules on the NEW prefix at tip       : {len(rnew)}")
        print(f"   shared emitters  BASE: old={eb_old:3d} new={eb_new:3d}")
        print(f"   shared emitters  TIP : old={et_old:3d} new={et_new:3d}")
        ok = (len(rold) == 0) and len(rnew) > 0 and et_old == 0 and et_new > 0 and eb_old > 0
        print(f"   VERDICT: {'OK — every rule and its emitter moved together' if ok else 'RED'}")
        if not ok:
            bad += 1
        for i, kind, body in rnew:
            print(f"      {spec}:{i} {kind} {body[:110]}")
        print()
    print(f"{'ALL PAIRS RESOLVE' if not bad else str(bad) + ' PAIR(S) RED'}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
