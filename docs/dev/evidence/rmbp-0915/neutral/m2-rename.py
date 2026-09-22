#!/usr/bin/env python3
"""NEUTRAL M2 — the colon witness families that name a board, and the ONE table that moves them.

`--apply`  rewrites the file set.
`--prove`  re-reads `git diff` and requires every changed line pair to be reproduced by this table.
`--count`  prints old/new counts per family over the whole repo (minus .git).

WHY THE TABLE IS THE SCRIPT (M1's lesson (a), 6977d374): a rename batch must move EVERY SPELLING of
a token, and M1 found the second spelling was regex-escaping (`\\[orinstkdepth\\]`). M2's families
carry no regex metacharacter, so escaping is NOT the second spelling here — the second spelling is
the DROPPED `:: ` PREFIX. `jetson-sync1.spec` writes three LIVE rules as bare `TEGRA-SD:` /
`TEGRA-UNAFS:`, and two subsystem docs hand the operator `awk 'index($0,"PINSTALL:")'`. A
`sed 's/:: TEGRA-SD:/:: SDMMC:/'` moves the emitter and leaves those behind SILENTLY, because a
FORBID that can no longer match still reads green and a PENDING was never required. So PASS_B is
enumerated per file with an expected count, and a miscount is a hard error rather than a warning.

WHAT THE TABLE DOES NOT MOVE, and the rule that decides it: the rename moves WIRE TOKENS — what a
capture carries and what a gate greps. It does NOT move ARC NAMES (`PI-DESK` the 2026-08-12 arc,
`PI-RAST`, `ORIN-DESKFURN` as a knob-doc heading) or KNOB NAMES (`UNAOS_PIUSB`, `piinstall`,
`sdmmc`, `pirast` — LAWS §4: a rename batch keeps knobs). `ORIN-DESKFURN`/`ORIN-RENDER` move ONLY
where they sit inside witness MESSAGE TEXT, which is PASS_C's two anchors.
"""
import os, re, subprocess, sys

ROOT = os.path.dirname(os.path.abspath(__file__))   # docs/dev/evidence/rmbp-0915/neutral
for _ in range(5):                                  # -> the repo root
    ROOT = os.path.dirname(ROOT)

# ---------------------------------------------------------------- PASS A: the `:: NAME:` form
PASS_A = [
    (":: TEGRA-UNAFS:", ":: UNAFS:"),
    (":: PIINSTALL:",   ":: INSTALL:"),
    (":: PINSTALL:",    ":: INSTALL:"),
    (":: TEGRA-SD:",    ":: SDMMC:"),
    (":: piusb27:",     ":: usb27:"),
    (":: piusb28:",     ":: usb28:"),
    (":: PI-RAST:",     ":: RAST:"),
    (":: PI-DESK:",     ":: DESK:"),
    (":: PIUSB:",       ":: USB:"),
]

# Named pin files outside the kernel. The kernel half of the set is COMPUTED (every shared .rs that
# carries a PASS_A key), so a file a peer adds between the census and the apply cannot be missed.
PASS_A_PINS = [
    "unaos/scripts/specs/x86-default.spec",
    "unaos/scripts/specs/x86-install.spec",
    "unaos/scripts/specs/jetson-sync1.spec",
    "unaos/scripts/specs/battery-control.capture",
    "unaos/scripts/banner-cert.sh",
    "unaos/scripts/orin-specscore.py",
    "unaos/arroyo",
    "tools/serial-analyzer.py",
    "docs/dev/OS/07_USB_STORAGE/usb_xhci.md",
    "docs/dev/OS/08_VIDEO/v3d.md",
    "docs/dev/OS/08_VIDEO/rasterizer.md",
    "docs/dev/OS/08_VIDEO/engine.md",
    "docs/dev/OS/08_VIDEO/PARITY.md",
    "docs/dev/OS/10_INSTALL/partition-install.md",
    "docs/dev/OS/10_INSTALL/SITTING-1.md",
    "docs/dev/OS/01_BOOT_HAL/arch_arm64.md",
]

# ------------------------------------- PASS B: the SECOND SPELLING — same family, no `:: ` prefix
# (path, old, new, expected occurrences). An expected-count miss is fatal.
PASS_B = [
    ("unaos/scripts/specs/jetson-sync1.spec", "TEGRA-SD: REFUSED to publish",        "SDMMC: REFUSED to publish",        2),
    ("unaos/scripts/specs/jetson-sync1.spec", "TEGRA-UNAFS: native unafs volume",    "UNAFS: native unafs volume",       1),
    ("unaos/scripts/specs/jetson-sync1.spec", "TEGRA-UNAFS: mount on TegraSd FAILED","UNAFS: mount on TegraSd FAILED",   1),
    ("unaos/scripts/specs/jetson-sync1.spec", "set for TEGRA-SD: two CONSECUTIVE",   "set for SDMMC: two CONSECUTIVE",   1),
    ("unaos/scripts/orin-specscore.py",       "`TEGRA-SD: REFUSED to publish`",      "`SDMMC: REFUSED to publish`",      1),
    ("docs/dev/OS/10_INSTALL/SITTING-1.md",   "'PINSTALL: census part='",            "'INSTALL: census part='",          1),
    ("docs/dev/OS/10_INSTALL/SITTING-1.md",   'index($0,"PINSTALL")',                'index($0,"INSTALL")',              1),
    # the token forces the article; declared here so --prove still reproduces the line exactly
    ("docs/dev/OS/10_INSTALL/SITTING-1.md",   "a `PINSTALL` line",                   "an `INSTALL` line",                1),
    ("docs/dev/OS/10_INSTALL/partition-install.md", 'index($0,"PINSTALL:")',         'index($0,"INSTALL:")',             2),
    ("docs/dev/OS/08_VIDEO/PARITY.md",        "`PI-DESK: desktop armed`",            "`DESK: desktop armed`",            1),
]

# ------------------------------- PASS C: the two arc names that sit INSIDE witness message text
PASS_C = [
    ("unaos/crates/kernel/src/main.rs", "(ORIN-DESKFURN entered", "(DESKFURN entered", 1),
    ("unaos/crates/kernel/src/main.rs", "(ORIN-RENDER entered",   "(RENDER entered",   1),
]

KSRC = "unaos/crates/kernel/src"


def kernel_files():
    out = []
    for dp, dn, fn in os.walk(os.path.join(ROOT, KSRC)):
        for f in sorted(fn):
            if not f.endswith(".rs"):
                continue
            rel = os.path.relpath(os.path.join(dp, f), ROOT)
            if rel.startswith(os.path.join(KSRC, "arch") + os.sep):
                continue                      # arch/ is not this seat's — board files may keep board names
            txt = open(os.path.join(ROOT, rel), encoding="utf-8").read()
            if any(o in txt for o, _ in PASS_A):
                out.append(rel)
    return out


def apply():
    files = kernel_files() + PASS_A_PINS
    seen = set()
    order = [f for f in files if not (f in seen or seen.add(f))]
    print("== PASS A — the `:: NAME:` form ==")
    for rel in order:
        p = os.path.join(ROOT, rel)
        txt = open(p, encoding="utf-8").read()
        hits = []
        for old, new in PASS_A:
            n = txt.count(old)
            if n:
                txt = txt.replace(old, new)
                hits.append(f"{old} x{n}")
        if hits:
            open(p, "w", encoding="utf-8").write(txt)
            print(f"  {rel}: " + ", ".join(hits))

    for label, table in (("PASS B — the second spelling (no `:: ` prefix)", PASS_B),
                         ("PASS C — arc name inside witness message text", PASS_C)):
        print(f"== {label} ==")
        for rel, old, new, want in table:
            p = os.path.join(ROOT, rel)
            txt = open(p, encoding="utf-8").read()
            got = txt.count(old)
            if got != want:
                sys.exit(f"FATAL {rel}: {old!r} found {got}x, table says {want}x — the table is stale, nothing further written")
            open(p, "w", encoding="utf-8").write(txt.replace(old, new))
            print(f"  {rel}: {old!r} x{got} -> {new!r}")


def substitute(line):
    for old, new in PASS_A:
        line = line.replace(old, new)
    for _, old, new, _ in PASS_B + PASS_C:
        line = line.replace(old, new)
    return line


def prove(base):
    """Every changed line pair must be the table applied to the old line."""
    authored = {
        "docs/dev/OS/rmbp-ledger.md", "docs/dev/OS/rmbp-queue.md",
        "docs/dev/evidence/orin14/NEUTRAL-TABLE.md",
    }
    diff = subprocess.run(["git", "-C", ROOT, "diff", "--unified=0", base, "--"],
                          capture_output=True, text=True).stdout.splitlines()
    cur, files, ins, dels, bad = None, set(), 0, 0, []
    olds, news = [], []

    def flush():
        nonlocal olds, news, bad
        if olds or news:
            if len(olds) == len(news):
                for o, n in zip(olds, news):
                    if substitute(o) != n:
                        bad.append((cur, o, n))
            else:
                for o in olds:
                    bad.append((cur, o, "<no paired + line>"))
        olds, news = [], []

    for ln in diff:
        if ln.startswith("+++ b/"):
            flush(); cur = ln[6:]; files.add(cur); continue
        if ln.startswith("@@"):
            flush(); continue
        if cur in authored or cur is None or cur.startswith("docs/dev/evidence/rmbp-0915/neutral/"):
            continue
        if ln.startswith("-") and not ln.startswith("---"):
            dels += 1; olds.append(ln[1:])
        elif ln.startswith("+") and not ln.startswith("+++"):
            ins += 1; news.append(ln[1:])
    flush()

    pop = sorted(f for f in files if f not in authored
                 and not f.startswith("docs/dev/evidence/rmbp-0915/neutral/"))
    print("NEUTRAL M2 — proof that the RENAME diff is a PURE TOKEN SUBSTITUTION")
    print(f"base {base}, branch exec-rmbp-neutral2")
    print()
    print("Method: for every changed line pair (-,+), apply the M2 substitution table (PASS A's")
    print("`:: NAME:` form, PASS B's PREFIX-LESS second spelling, PASS C's two message-text anchors)")
    print("to the OLD line and require equality with the NEW line. One unexplained line means the")
    print("batch moved something other than a token.")
    print()
    print(f"POPULATION = the {len(pop)} files the rename edits. Files this arc AUTHORS new prose into are")
    print("excluded BY NAME and are not substitutions: " + " ".join(sorted(authored))
          + " docs/dev/evidence/rmbp-0915/neutral/")
    print()
    for f in pop:
        print(f"    {f}")
    print()
    print(f"  changed lines: -{dels} / +{ins}  ({'insertions == deletions' if ins == dels else 'MISMATCH'})")
    print(f"  lines NOT explained by the substitution table: {len(bad)}")
    for f, o, n in bad[:20]:
        print(f"    {f}\n      - {o}\n      + {n}")
    return 0 if (ins == dels and not bad) else 1


def count():
    olds = [o for o, _ in PASS_A] + [b[1] for b in PASS_B] + [c[1] for c in PASS_C]
    news = [n for _, n in PASS_A] + [b[2] for b in PASS_B] + [c[2] for c in PASS_C]
    for label, toks in (("OLD", olds), ("NEW", news)):
        for t in dict.fromkeys(toks):
            r = subprocess.run(["git", "-C", ROOT, "grep", "-cF", "--", t], capture_output=True, text=True)
            tot = sum(int(l.rsplit(":", 1)[1]) for l in r.stdout.splitlines() if l.strip())
            print(f"{label:3s} {t:38s} {tot}")


if __name__ == "__main__":
    a = sys.argv[1] if len(sys.argv) > 1 else "--count"
    if a == "--apply":
        apply()
    elif a == "--prove":
        sys.exit(prove(sys.argv[2] if len(sys.argv) > 2 else "acd102d7"))
    else:
        count()
