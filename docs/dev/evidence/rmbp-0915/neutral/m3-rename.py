#!/usr/bin/env python3
"""NEUTRAL M3 — the board-named IDENTIFIERS of B160's status cell, and the ONE table that moves them.

`--apply`  rewrites the file set (refuses to write ANYTHING on a miscount against EXPECT).
`--prove`  re-reads `git diff` and requires every changed line pair to be reproduced by this table.
`--count`  prints old/new word-bounded counts per identifier over the whole repo (git ls-files).
`--half <FILE> <OLD>`  GO-RED: applies the whole table EXCEPT `<OLD>` in `<FILE>`, so one file still
           spells the old identifier while its definition has moved — the check must then fail to
           compile. Never committed; the tree is restored by `git diff | git apply -R` + `--apply`.

THE METHOD IS M2's (`m2-rename.py`), reproduced: the table IS the script; the kernel half of the file
set is COMPUTED (every shared `.rs` under `unaos/crates/kernel/src/` outside `arch/` that carries a
key), the pins outside the kernel are NAMED; every file carries an EXPECTED count and a miscount is
fatal before a single byte is written; `--prove` applies the table to every old line of the diff.

WHAT DIFFERS FROM M2, because these are identifiers and not witness strings: the substitution is
WORD-BOUNDED (`\\bTOKEN\\b`), not `str.replace`, so a longer identifier that CONTAINS a key cannot be
edited by accident. At base 98fd8e66 no such superstring exists (`git grep -F` and `git grep -w -F`
agree, 92 vs 92 lines for `TegraSd`), so the boundary changes no count — it is the guard, not a fix.
Retired names that share a prefix with the family (`tegra_shell_note`, `tegra_shell_live_id`,
`tegra_shell_remint`, `TEGRA_SHELL_BASE/LEN/...` in `main.rs:9567-9579` and `dock.md:225-226`) name
functions APPPIN DELETED; they are history, not identifiers, and the table does not name them.

THE SECOND SPELLING (M1: regex-escaping; M2: the dropped `:: ` prefix). M3's is the IDENTIFIER ON
THE WIRE: `TegraSd` is not only an enum variant, `main.rs` spells it inside four `:: UNAFS:` witness
MESSAGES ("MOUNTED read-only on TegraSd", "mount on TegraSd FAILED", ...), and three live gates pin
that text — `banner-cert.sh:312` (the `sdmmc` artifact row) and `jetson-sync1.spec:593` (PENDING) /
`:619` (FORBID). A rename that moved the variant and not the message would leave the wire naming a
handle that no longer exists; one that moved the message and not the pins would turn the FORBID into
a rule that can never match (reads green) and the banner row into a hard red. So the wire pins are
in EXPECT with their counts like every other file, and `--apply` moves all three in the same pass.

THE RULE FOR EACH NEW NAME (NEUTRAL-TABLE §3a's own precedent, not a new one): strip the board
prefix; if the stripped name is already bound, qualify it by the subsystem the code already uses.
  * `TegraSd` -> `SdMmc`: §3a's proposed name ("sibling of the existing x86 `Sdhc`"), and what the
    handle IS — the card behind the SoC's SD/MMC host (`arch/aarch64/sdmmc_tegra.rs`, the `sdmmc`
    feature, and since M2 the `:: SDMMC:` witness family `:: TEGRA-SD:` became). 0 prior hits outside §3a itself.
  * `tegra_shell_*` -> `shellwin_*`, `TegraShellWin` -> `ShellWin`, `TEGRA_SHELL_PRESENTED` ->
    `SHELLWIN_PRESENTED`: stripping gives `shell_*`, and two of the six are TAKEN — `shell_id`
    (51 hits in main.rs, including `let shell_id = tegra_shell_id(&shellwin);` at :2903, the very
    call site) and `shell_pal` (20, the x86/Pi shell pump's `TargetPal`). §3a met the same case with
    `orin_render_service` -> `render_pass_service` (the bare `render_service` was taken). The
    qualifier is the one the family's UNRENAMED siblings in the same region already carry —
    `shellwin_row`, `shellwin_absent`, `shellwin_launch_owed`, `shellwin_note_mint`,
    `shellwin_quit_note`, `shellwin_scene_live`, `shellwin_service_rearm`, `SHELLWIN_ROW` — and the
    type the six functions take (`&Option<TegraShellWin>`). All eight new names: 0 prior hits.

WHAT THE TABLE DOES NOT MOVE (excluded by the brief, by M2's rule, or by the file's own name):
  * `tegra_el0` (B160's "19") is NOT an identifier: all 19 comment-stripped shared sites are the
    FEATURE NAME (`feature = "tegra_el0"` cfgs and prose naming that feature). A feature name is a
    knob (NEUTRAL-TABLE §4; M2: "a rename batch keeps knobs"), so it is not in this table.
    `tegra_el0_start_maybe` / `"tegra-el0-verdict"` / `tegra_el0_verdict` are the `:: TEGRA-EL0:`
    emitters — Peter's open question (rename-all-four vs machine-tag-by-design) — excluded with it.
  * `:: tegra:`, `:: TEGRA-EL0:`, `:: PIUSB:` x3 in `arch/aarch64/piusb.rs`: excluded by the brief.
  * `arch/` entirely (M2's rule) — the one `TegraSd` there is a comment in `arch/aarch64/sdmmc_tegra.rs`.
  * `unaos/docs/dev/OS/10_INSTALL/orin-unafs-root.md` x3: a file whose NAME carries the board.
  * Records, not code: `docs/dev/LAWS.md` (quotes the orin 19-21 incident by the names it had),
    `docs/dev/LEDGER.md`, `docs/dev/OS/{orin,rmbp}-{ledger,queue}.md`, `docs/dev/evidence/**`.
  * Siblings of the same subsystems that B160's cell does not name (`tegra_sd_info`, `tegra_sd`,
    `TEGRA_SD_*`, `"tegra-sd"`, `tegra_boot_focus`, ...): the cell's M3 is the three counted rows;
    the rest of §3a stays owed, listed in the M3 addendum.
"""
import os, re, subprocess, sys

ROOT = os.path.dirname(os.path.abspath(__file__))   # docs/dev/evidence/rmbp-0915/neutral
for _ in range(5):                                  # -> the repo root
    ROOT = os.path.dirname(ROOT)

BASE = "98fd8e66"

# ------------------------------------------------------------------------------ THE TABLE
TABLE = [
    ("TegraSd",                 "SdMmc"),
    ("TegraShellWin",           "ShellWin"),
    ("tegra_shell_window_open", "shellwin_window_open"),
    ("tegra_shell_mark",        "shellwin_mark"),
    ("tegra_shell_id",          "shellwin_id"),
    ("tegra_shell_pal",         "shellwin_pal"),
    ("tegra_shell_present",     "shellwin_present"),
    ("tegra_shell_pick",        "shellwin_pick"),
    ("TEGRA_SHELL_PRESENTED",   "SHELLWIN_PRESENTED"),
]
PAT = {old: re.compile(r"\b" + re.escape(old) + r"\b") for old, _ in TABLE}

KSRC = "unaos/crates/kernel/src"

# Named pin files outside the kernel (docs, scripts, specs, the kernel manifest).
PINS = [
    "unaos/arroyo",
    "unaos/crates/kernel/Cargo.toml",
    "unaos/scripts/banner-cert.sh",
    "unaos/scripts/k8-reach.registry",
    "unaos/scripts/specs/jetson-sync1.spec",
    "docs/dev/OS/01_BOOT_HAL/arch_arm64.md",
    "docs/dev/OS/05_USER_EXPERIENCE/dock.md",
    "docs/dev/OS/07_USB_STORAGE/sdhc.md",
    "docs/dev/OS/09_FILESYSTEM/layout.md",
    "docs/dev/OS/09_FILESYSTEM/partitions.md",
    "docs/dev/OS/09_FILESYSTEM/vfs.md",
]

# (path, token, expected word-bounded occurrences at BASE). Every file the apply touches is here and
# every count must match EXACTLY, or nothing is written.
EXPECT = [
    # kernel — shared files (computed set must equal this list)
    (f"{KSRC}/drivers/block.rs",      "TegraSd", 17),
    (f"{KSRC}/fs/bootdisk.rs",        "TegraSd", 6),
    (f"{KSRC}/fs/fat.rs",             "TegraSd", 19),
    (f"{KSRC}/fs/unafs.rs",           "TegraSd", 20),
    (f"{KSRC}/fs/vfs.rs",             "TegraSd", 2),
    (f"{KSRC}/install/mod.rs",        "TegraSd", 3),
    (f"{KSRC}/install/partition.rs",  "TegraSd", 2),
    (f"{KSRC}/main.rs",               "TegraSd", 7),
    (f"{KSRC}/shell.rs",              "TegraSd", 2),
    (f"{KSRC}/wifi/firmware.rs",      "TegraSd", 2),
    (f"{KSRC}/main.rs",               "TegraShellWin", 15),
    (f"{KSRC}/main.rs",               "tegra_shell_window_open", 11),
    (f"{KSRC}/main.rs",               "tegra_shell_mark", 3),
    (f"{KSRC}/main.rs",               "tegra_shell_id", 3),
    (f"{KSRC}/main.rs",               "tegra_shell_pal", 6),
    (f"{KSRC}/main.rs",               "tegra_shell_present", 6),
    (f"{KSRC}/main.rs",               "tegra_shell_pick", 8),
    (f"{KSRC}/main.rs",               "TEGRA_SHELL_PRESENTED", 2),
    (f"{KSRC}/video/dock.rs",         "tegra_shell_window_open", 2),
    # pins — the WIRE pins first (the second spelling: the identifier inside witness message text)
    ("unaos/scripts/banner-cert.sh",           "TegraSd", 1),
    ("unaos/scripts/specs/jetson-sync1.spec",  "TegraSd", 3),
    ("unaos/arroyo",                           "TegraSd", 4),
    ("unaos/crates/kernel/Cargo.toml",         "TegraSd", 2),
    ("unaos/scripts/k8-reach.registry",        "TegraSd", 1),
    ("docs/dev/OS/01_BOOT_HAL/arch_arm64.md",  "TegraSd", 4),
    ("docs/dev/OS/05_USER_EXPERIENCE/dock.md", "TegraShellWin", 2),
    ("docs/dev/OS/05_USER_EXPERIENCE/dock.md", "tegra_shell_window_open", 1),
    ("docs/dev/OS/07_USB_STORAGE/sdhc.md",     "TegraSd", 2),
    ("docs/dev/OS/09_FILESYSTEM/layout.md",    "TegraSd", 3),
    ("docs/dev/OS/09_FILESYSTEM/partitions.md","TegraSd", 2),
    ("docs/dev/OS/09_FILESYSTEM/vfs.md",       "TegraSd", 2),
]


def read(rel):
    return open(os.path.join(ROOT, rel), encoding="utf-8").read()


def kernel_files():
    out = []
    for dp, dn, fn in os.walk(os.path.join(ROOT, KSRC)):
        dn.sort()
        for f in sorted(fn):
            if not f.endswith(".rs"):
                continue
            rel = os.path.relpath(os.path.join(dp, f), ROOT)
            if rel.startswith(os.path.join(KSRC, "arch") + os.sep):
                continue                      # M2's rule: arch/ keeps board names (board files)
            txt = read(rel)
            if any(p.search(txt) for p in PAT.values()):
                out.append(rel)
    return out


def substitute(line, skip=None):
    for old, new in TABLE:
        if old != skip:
            line = PAT[old].sub(new, line)
    return line


def _skip_for(rel, half):
    return half[1] if half and rel == half[0] else None


def preflight():
    files = kernel_files() + PINS
    want = {}
    for rel, tok, n in EXPECT:
        want.setdefault(rel, {})[tok] = n
    bad = []
    if sorted(set(files)) != sorted(want):
        bad.append(f"file set: computed+named {sorted(set(files))} != EXPECT {sorted(want)}")
    for rel in sorted(set(files) | set(want)):
        txt = read(rel)
        for old, _ in TABLE:
            got = len(PAT[old].findall(txt))
            exp = want.get(rel, {}).get(old, 0)
            if got != exp:
                bad.append(f"{rel}: {old} found {got}x, table says {exp}x")
    return sorted(set(files)), bad


def apply(half=None):
    files, bad = preflight()
    if bad:
        for b in bad:
            print("FATAL " + b)
        sys.exit("the table is stale — nothing written")
    tot = 0
    for rel in files:
        txt = read(rel)
        skip = _skip_for(rel, half)
        hits = [f"{old} x{len(PAT[old].findall(txt))}" for old, _ in TABLE
                if old != skip and PAT[old].search(txt)]
        new = "\n".join(substitute(l, skip) for l in txt.split("\n"))
        if new != txt:
            open(os.path.join(ROOT, rel), "w", encoding="utf-8").write(new)
            tot += sum(int(h.rsplit("x", 1)[1]) for h in hits)
            print(f"  {rel}: " + ", ".join(hits))
    print(f"== {tot} occurrences over {len(files)} files" + (f" — HALF-APPLIED, `{half[1]}` LEFT OLD in {half[0]}" if half else ""))


def prove(base):
    """Every changed line pair must be the table applied to the old line."""
    authored = {
        "docs/dev/OS/rmbp-ledger.md", "docs/dev/OS/rmbp-queue.md",
        "docs/dev/evidence/orin14/NEUTRAL-TABLE.md",
    }
    ev = "docs/dev/evidence/rmbp-0915/neutral/"
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
                for n in news[len(olds):]:
                    bad.append((cur, "<no paired - line>", n))
        olds, news = [], []

    for ln in diff:
        if ln.startswith("diff --git"):
            flush(); cur = None; continue
        if ln.startswith("+++ b/"):
            flush(); cur = ln[6:]; files.add(cur); continue
        if ln.startswith("@@"):
            flush(); continue
        if cur in authored or cur is None or cur.startswith(ev):
            continue
        if ln.startswith("-") and not ln.startswith("---"):
            dels += 1; olds.append(ln[1:])
        elif ln.startswith("+") and not ln.startswith("+++"):
            ins += 1; news.append(ln[1:])
    flush()

    pop = sorted(f for f in files if f not in authored and not f.startswith(ev))
    occ = 0
    for f in pop:
        old = subprocess.run(["git", "-C", ROOT, "show", f"{base}:{f}"], capture_output=True, text=True).stdout
        occ += sum(len(PAT[o].findall(old)) for o, _ in TABLE)
    print("NEUTRAL M3 — proof that the RENAME diff is a PURE TOKEN SUBSTITUTION")
    print(f"base {base}, branch exec-rmbp-neutral3")
    print()
    print("Method: for every changed line pair (-,+), apply the M3 substitution table (nine")
    print("identifiers, word-bounded) to the OLD line and require equality with the NEW line. One")
    print("unexplained line means the batch moved something other than a token.")
    print()
    print(f"POPULATION = the {len(pop)} files the rename edits. Files this arc AUTHORS new prose into are")
    print("excluded BY NAME and are not substitutions: " + " ".join(sorted(authored)) + " " + ev)
    print()
    for f in pop:
        print(f"    {f}")
    print()
    print(f"  identifier occurrences moved: {occ}")
    print(f"  changed lines: -{dels} / +{ins}  ({'insertions == deletions' if ins == dels else 'MISMATCH'})")
    print(f"  lines NOT explained by the substitution table: {len(bad)}")
    for f, o, n in bad[:20]:
        print(f"    {f}\n      - {o}\n      + {n}")
    return 0 if (ins == dels and not bad) else 1


def count():
    files = subprocess.run(["git", "-C", ROOT, "ls-files"], capture_output=True, text=True).stdout.split()
    txts = []
    for f in files:
        try:
            txts.append(read(f))
        except (UnicodeDecodeError, IsADirectoryError, FileNotFoundError):
            pass
    for label, toks in (("OLD", [o for o, _ in TABLE]), ("NEW", [n for _, n in TABLE])):
        for t in toks:
            p = re.compile(r"\b" + re.escape(t) + r"\b")
            print(f"{label:3s} {t:26s} {sum(len(p.findall(x)) for x in txts)}")


if __name__ == "__main__":
    a = sys.argv[1] if len(sys.argv) > 1 else "--count"
    if a == "--apply":
        apply()
    elif a == "--half":
        apply(half=(sys.argv[2], sys.argv[3]))
    elif a == "--prove":
        sys.exit(prove(sys.argv[2] if len(sys.argv) > 2 else BASE))
    elif a == "--preflight":
        _, bad = preflight()
        print("\n".join(bad) if bad else "preflight OK — every file and count matches EXPECT")
        sys.exit(1 if bad else 0)
    else:
        count()
