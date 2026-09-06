#!/usr/bin/env bash
# ledger-check.sh — GATE-LEDGER: the issue ledgers are a tracker, not prose. Every row is checkable.
#
# Peter (2026-09-05): each track keeps its arch ledger, one over-arching LEDGER.md holds what crosses
# arches; the arc that fixes/flies/drops an item ticks it in the same commit. A rule like that rots
# exactly when sessions are busiest — PCIE-RP-RECOVERY.md claimed "no reboot facility of any kind" for
# a day after FADTRESET landed — unless a gate holds it. This is the gate. Same standard as GATE-FAMILY:
# it proved it can fail by TREE MUTATION before it shipped (fixtures listed at the bottom).
#
# CONTRACT (agreed rmbp 11 ↔ orin 13, 2026-09-05):
#   * A ledger file is any of docs/dev/LEDGER.md, docs/dev/OS/*-ledger.md that EXISTS in this tree.
#     A missing one is SKIPPED with a line (LEDGER.md reaches a track only at its trunk sync).
#   * A ledger TABLE is a markdown table whose header has a `status` column. Its rows are ledger rows.
#   * id: first cell, `^[A-Z]+[0-9]+` (a cross-ref suffix `(→ S<n>)` is allowed after it). Unique per file.
#   * status: must BEGIN with one of  open | fixed-unflown | flown | landed | dropped  (bold allowed;
#     free text allowed after " — " or ", "). Nothing else — "standing", "relayed", "recorded" are not states.
#   * owner: if the table has an `owner` column, the cell ∈ {orin, pi, rmbp, shared-gate} (first word).
#   * cross-refs: `→ S<n>` / `→ P<n>` anywhere in a row must resolve to an id in docs/dev/LEDGER.md
#     (checked only when LEDGER.md is present in the tree).
#   * shas: every 7–8 hex token that `git cat-file -e` recognises as a commit is fine; one that does
#     not exist is RED. A `fixed-unflown` / `flown` / `landed` row's shas must be ancestors of some head
#     in {hw-rmbp, hw-jetson, hw-pi4, main, origin/hw-*, origin/main} — a fix nobody can fetch is not fixed.
#   * evidence: any `unaos-bench/scratch` path in a row is RED (evidence outside git); every
#     `docs/...` path in a row must exist in the tree.
#   * Prose stays GREEN: ids and paths OUTSIDE a ledger table are never judged.
#
# usage: ledger-check.sh [repo-root]        exit 0 green · 1 red · 2 no verdict (control probe failed)
set -uo pipefail
ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$ROOT" || exit 2
python3 - "$ROOT" <<'PY'
import re, sys, os, subprocess, glob
root = sys.argv[1]
ENUM = ("open", "fixed-unflown", "flown", "landed", "dropped")
OWNERS = {"orin", "pi", "rmbp", "shared-gate"}
FETCHED = ("fixed-unflown", "flown", "landed")
files = [p for p in ["docs/dev/LEDGER.md"] + sorted(glob.glob("docs/dev/OS/*-ledger.md")) if os.path.exists(p)]
skipped = [p for p in ["docs/dev/LEDGER.md"] if not os.path.exists(p)]
red = []
def say(*a): print("GATE-LEDGER:", *a)

def tables(text):
    """yield (header_cells, [(lineno, cells)]) for every markdown table."""
    lines = text.split("\n"); i = 0
    while i < len(lines) - 1:
        if lines[i].lstrip().startswith("|") and re.match(r"^\s*\|[\s:|-]+\|\s*$", lines[i+1]):
            hdr = [c.strip().lower() for c in lines[i].strip().strip("|").split("|")]
            rows = []; j = i + 2
            while j < len(lines) and lines[j].lstrip().startswith("|"):
                rows.append((j + 1, [c.strip() for c in lines[j].strip().strip("|").split("|")])); j += 1
            yield hdr, rows; i = j
        else:
            i += 1

def col(hdr, name):
    for k, h in enumerate(hdr):
        if h.startswith(name): return k
    return None

def sha_exists(s):
    return subprocess.run(["git", "cat-file", "-e", s + "^{commit}"], capture_output=True).returncode == 0
def heads():
    out = subprocess.run(["git", "for-each-ref", "--format=%(refname:short)", "refs/heads/hw-*", "refs/heads/main",
                          "refs/remotes/origin/hw-*", "refs/remotes/origin/main"], capture_output=True, text=True).stdout.split()
    return out
HEADS = heads()
def reachable(s):
    return any(subprocess.run(["git", "merge-base", "--is-ancestor", s, h], capture_output=True).returncode == 0 for h in HEADS)

# STRICT — WIRED, NOT REMEMBERED (pi 7, 2026-09-06, turning rmbp 13's own criterion back on it).
# The deferral above is only honest if something forces the deferred refs to resolve SOMEWHERE, and
# the first cut left that to a landing seat exporting UNAOS_LEDGER_STRICT=1 by hand. There is exactly
# ONE invocation of this script (arroyo's `check_both`, no environment), and no landing-specific gate
# command for the export to live in -- so "the landing runs strict" was a remembered step, which is
# the same shape as the norm-only exits this whole change was chosen over. A backstop nobody is wired
# to run is a backstop that runs never.
#
# THE TRIGGER IS THE BRANCH, and it is semantic rather than heuristic: **the trunk enforces, track
# branches defer.** On a track branch a reference to another seat's row is unresolvable by
# construction and deferring is correct. On the TRUNK it is not: trunk is where everything lands, so a
# trunk row pointing at something not on trunk IS a dangling reference, whoever wrote it. The landing
# merges to trunk and runs the trunk battery there, so strict arrives exactly when and where the refs
# became resolvable, with nobody remembering anything.
#
# REJECTED — pi 7's proposal, and it was close: auto-strict when the tree carries rows from two or
# more distinct seat prefixes. It reads as structural but it keeps a false-red window: a track branch
# that syncs trunk inherits another seat's prefix (hw-rmbp gains SO rows the moment orin lands), and a
# reference to a THIRD seat's unlanded row then reds on a branch that could never have carried it. The
# branch test has no such window because it does not try to infer the landing from the contents.
#
# ⚠ THE TRIGGER IS A BRANCH NAME, AND THIS REPO HAS RENAMED ITS TRUNK ONCE (pi 7's residual, taken).
# CLAUDE.md carries a standing instruction to VERIFY which ref is trunk rather than trust it -- the
# retired `UnaOS-gemini` staging name is still a live ref on origin and is NOT main's tip. If the trunk
# is renamed again and nobody sets `UNAOS_LEDGER_TRUNK`, strict silently stops firing and every tree
# defers forever: "a backstop that runs never", returning through the rename door. What keeps it merely
# quiet rather than silent is the DEFERRED line naming the regime in force -- a seat standing on a
# renamed trunk reads "branch `<newname>` is a track branch" and has the contradiction in front of them.
# Whoever renames the trunk sets `UNAOS_LEDGER_TRUNK` here, or changes this default in the same commit.
#
# `UNAOS_LEDGER_STRICT=1` still forces strict anywhere (and `=0` suppresses it, trunk included, for a
# trunk that is mid-landing and knows it). `UNAOS_LEDGER_TRUNK` names the trunk branch -- it defaults
# to `main`, is the one knob the go-red proof turns, and is why that proof can run on a track branch.
TRUNK = os.environ.get("UNAOS_LEDGER_TRUNK", "main")
_branch = subprocess.run(["git", "rev-parse", "--abbrev-ref", "HEAD"],
                         capture_output=True, text=True).stdout.strip()
_env_strict = os.environ.get("UNAOS_LEDGER_STRICT")
if _env_strict == "0":
    STRICT, STRICT_WHY = False, "suppressed by UNAOS_LEDGER_STRICT=0"
elif _env_strict == "1":
    STRICT, STRICT_WHY = True, "forced by UNAOS_LEDGER_STRICT=1"
elif _branch == TRUNK:
    STRICT, STRICT_WHY = True, f"automatic: on the trunk branch `{TRUNK}`, where every ref must resolve"
else:
    STRICT, STRICT_WHY = False, f"off: branch `{_branch}` is a track branch, cross-branch refs deferred"
deferred = []
ledger_ids = set()
if "docs/dev/LEDGER.md" in files:
    for hdr, rows in tables(open("docs/dev/LEDGER.md").read()):
        if col(hdr, "status") is None: continue
        for _, cells in rows:
            m = re.match(r"([A-Z]+[0-9]+)", cells[0])
            if m: ledger_ids.add(m.group(1))
    # P-ROWS ARE BULLETS, NOT TABLE ROWS — and the resolver could not see them (rmbp 13, 2026-09-06).
    # The cross-ref regex has always accepted `→ P<n>` as a reference, but `ledger_ids` was built ONLY
    # from tables with a `status` column, and the protocol rows live in LEDGER.md as `- **P14** — …`
    # bullets. So every `→ P<n>` that has ever been written resolved against an id set containing ZERO
    # P ids and RED-LINED — a false red on a row that exists, in the gate whose job is telling those
    # apart. Found by this gate reding a `→ P15` cross-ref to a P-row filed in the same commit. The
    # id-space the gate accepts and the id-space it can resolve have to be the same one.
    for _m in re.finditer(r"^-\s+\*\*([A-Z]+[0-9]+)\*\*", open("docs/dev/LEDGER.md").read(), re.M):
        ledger_ids.add(_m.group(1))

def _check_ref(ref, where, rid, red, deferred, ledger_ids, files, STRICT):
    """One home for the resolve/defer/red decision, so the TABLE scan and the BULLET scan below
    cannot drift apart — two copies of this logic is how one of them silently stops matching."""
    if "docs/dev/LEDGER.md" not in files or ref in ledger_ids:
        return
    pfx = re.match(r"([A-Z]+)", ref).group(1)
    if pfx in ("SR", "SO", "SP") and not STRICT:
        deferred.append(f"{where}: {rid} cross-ref → {ref} DEFERRED — {pfx} rows are branch-local; resolves when that seat's ledger lands (UNAOS_LEDGER_STRICT=1 to require it now)")
    else:
        red.append(f"{where}: {rid} cross-ref → {ref} does not resolve in docs/dev/LEDGER.md")

rows_seen = 0
for path in files:
    text = open(path).read(); ids = set()
    for hdr, rows in tables(text):
        st = col(hdr, "status")
        if st is None: continue
        ow = col(hdr, "owner")
        for ln, cells in rows:
            rows_seen += 1
            where = f"{path}:{ln}"
            m = re.match(r"([A-Z]+[0-9]+)", cells[0])
            if not m:
                red.append(f"{where}: row id `{cells[0][:30]}` does not match ^[A-Z]+[0-9]+"); continue
            rid = m.group(1)
            if rid in ids: red.append(f"{where}: duplicate id {rid}")
            ids.add(rid)
            status_raw = cells[st] if st < len(cells) else ""
            status = re.sub(r"[*_`]", "", status_raw).strip()
            head = re.split(r"\s+—|,|\s+\(|\s+/|\s+until|\s+—", status)[0].strip().lower()
            if head not in ENUM:
                red.append(f"{where}: {rid} status `{status_raw[:40]}` does not begin with one of {'|'.join(ENUM)}")
            if ow is not None and ow < len(cells):
                first = re.sub(r"[*_`]", "", cells[ow]).strip().split()
                if not first or first[0].strip(",;") not in OWNERS:
                    red.append(f"{where}: {rid} owner `{cells[ow][:30]}` not in {sorted(OWNERS)}")
            rowtext = " | ".join(cells)
            # SEAT-PREFIXED IDS (three-seat vote 2026-09-06: pi 7 proposed, orin 15 and rmbp 12
            # agreed; S1-S32 freeze, new shared rows take SP<n> pi / SR<n> rmbp / SO<n> orin).
            # Sequential allocation is STRUCTURALLY broken across unpushed branches -- a reserved
            # gap only works if every seat can see it, and none can; two collisions in one night.
            # The id check above already passes them (^[A-Z]+[0-9]+). THIS resolver did not: the
            # old r"→\s*([SP][0-9]+)" could not match "→ SP32" (after S comes P, not a digit), so
            # a prefixed cross-ref was SILENTLY NOT CHECKED -- not red, skipped. A check that
            # cannot fire, in the gate whose whole job is that they can.
            # MENTION vs REFERENCE (pi 7, 2026-09-06). A checker scanning free text cannot tell a
            # MENTION of an id from a REFERENCE to one: fixtures, examples and quoted commit
            # messages are all live input to this resolver. The escape is the ARROW GLYPH and it
            # is now a CONTRACT, not an accident: the UNICODE arrow below is a reference the gate
            # must resolve; an ASCII "->" is a mention and is invisible here. Cite fixtures and
            # examples with "->". pi 7 hit this by quoting this gate's own SP99 go-red fixture
            # ⚠ CORRECTED 2026-09-06 (pi 7, on their own claim; this seat had propagated it): that
            # SP99 sat in LEDGER.md's header, and until the fix above LEDGER.md's own refs were
            # EXEMPT — so it would have passed silently, forever, not red-lined. The mention-vs-
            # reference hazard is real and the arrow contract stands, but the incident that
            # illustrated it did not actually fire. It fires NOW, which is the better reason to keep
            # citing fixtures with "->": the exemption that made it inert is gone.
            # into a ledger header, where the sentence documenting the test became a failing
            # input to the test.
            # CROSS-BRANCH REFS ARE NOT DANGLING REFS (pi 7 found the collision, rmbp 13 settled it,
            # 2026-09-06). A seat-prefixed row lives on ONE branch until the landing merges the
            # ledgers, so a reference to it is unresolvable HERE by construction and resolvable
            # THERE by construction. The live instance: `| A36 (→ SR2) |` on hw-jetson, where SR2
            # lives on hw-rmbp -- zero SR rows in any of that tree's three ledger files.
            #
            # THE PART THAT MADE THIS A RULE CHANGE RATHER THAN A ONE-OFF: orin did nothing wrong.
            # The id contract three lines up SANCTIONS the suffix form (`^[A-Z]+[0-9]+` "a cross-ref
            # suffix `(→ S<n>)` is allowed after it"), while LEDGER P14 said a cross-ref to an
            # unfolded row stays PROSE. An id-suffix cross-ref cannot be prose without breaking the
            # id convention, so the two rules collided and the sanctioned one lost -- silently today
            # (older resolver skipped it), RED tomorrow (this one finds it). Green now, red later, on
            # a row whose author followed the documented form.
            #
            # THE SPLIT: shared ids (`S<n>`, `P<n>`) live in EVERY tree's LEDGER.md -- measured, 27
            # to 31 S-rows on main, hw-jetson, hw-pi4 and hw-rmbp alike -- so a `→ S<n>` that does
            # not resolve is a real dangling ref and stays RED. Seat-prefixed ids (`SR`/`SO`/`SP`)
            # are branch-local by construction (SR appears only on hw-rmbp, SO only on hw-jetson),
            # so an unresolved one is DEFERRED: printed, counted, named in the summary -- never
            # silently skipped, which is the failure this gate exists to not repeat.
            #
            # REJECTED, and why, so nobody re-proposes it: "defer only when the prefix has ZERO rows
            # in this tree" is a sharper discriminator and would still catch a typo like `→ SR99` on
            # hw-rmbp. It false-reds in the PARTIAL FOLD window -- SR1 landed, SR2 not yet, a ref to
            # SR2 from a tree that now has one SR row -- which is precisely the surprise-mid-landing
            # this change exists to prevent. Never false-red; catch the typos where they are
            # catchable instead:
            #
            # `UNAOS_LEDGER_STRICT=1` turns every DEFERRED into a RED. **The landing runs it.** After
            # a merge all three seats' ledgers are in one tree, every seat-prefixed ref is resolvable,
            # and a typo that rode along for a week surfaces there -- at the one moment it can be
            # told apart from a legitimate cross-branch reference.
            for ref in re.findall(r"→\s*((?:S[PRO]?|P)[0-9]+)", rowtext):  # see _check_ref below
                # LEDGER.md'S OWN CROSS-REFS WERE NEVER RESOLVED (pi 7 found it, rmbp 13 fixed it,
                # 2026-09-06). This condition used to carry `and path != "docs/dev/LEDGER.md"`, which
                # exempted the over-arching ledger from the check every arch ledger is subjected to —
                # the one file all three seats write to, and the one every arch ledger is resolved
                # AGAINST. Proved by mutation on both trees, not by reading: `→ S777` injected into
                # LEDGER.md passed at exit 0; the same ref in an arch ledger red at exit 1. Every
                # `→ S<n>` written into LEDGER.md this session had been unchecked.
                #
                # The exemption was defensible when it was written: without seat prefixes there was no
                # way to tell a self-reference from a cross-branch one, so resolving LEDGER.md would
                # have red-lined legitimate refs to rows on other branches. `SR`/`SO`/`SP` plus the
                # branch-triggered strict/deferred split solve exactly that, so the clause is now
                # obsolete rather than load-bearing — a cross-branch ref from LEDGER.md defers like any
                # other, and a dangling one reds like any other.
                _check_ref(ref, where, rid, red, deferred, ledger_ids, files, STRICT)
            if "unaos-bench/scratch" in rowtext:
                red.append(f"{where}: {rid} cites evidence outside git (unaos-bench/scratch)")
            for dp in re.findall(r"`?(docs/[A-Za-z0-9_./-]+\.md)", rowtext):
                if not os.path.exists(dp):
                    red.append(f"{where}: {rid} evidence path {dp} does not exist in the tree")
            # ARTIFACT DIGESTS ARE NOT COMMITS, and a row legitimately cites them: a BLOB sha
            # (`git rev-parse <commit>:<path>`), an objcopy/sha256 of a built image, a kernel8.img
            # digest. Two reddened this gate in one session and both "fixes" were to damage the
            # evidence to satisfy the checker -- pad the token, or reword around it.
            # The escape is EXPLICIT and author-declared: prefix the hash with a label and a colon
            # (`sha256:731c8f5b`, `blob:311bccea`, `img:d73a8981`). An INFERRED label -- "skip if
            # the word `img` appears nearby" -- was tried first and rejected: it would silently
            # stop checking a real commit sha in any row that happened to mention an image, which
            # is a check that cannot fire. An UNLABELLED hex token is still a short commit sha and
            # must resolve.
            # DEDUP: collect first, report each distinct sha ONCE. The escape check is
            # per-OCCURRENCE (a sha may appear both declared and bare in one row, and the bare
            # occurrence is still a reference), but the FINDING is per-sha. Switching this loop
            # from set(findall) to finditer to add the escape silently dropped that dedup and a
            # row citing one bad sha twice reported it twice -- duplicate findings are how a gate
            # teaches people to skim its output.
            bare = set()
            for m in re.finditer(r"(?<![0-9A-Za-z])([0-9a-f]{7,8})(?![0-9A-Za-z])", rowtext):
                if re.search(r"[A-Za-z][A-Za-z0-9_-]*:$", rowtext[max(0, m.start() - 24):m.start()]):
                    continue   # author-declared artifact digest, not a commit
                bare.add(m.group(1))
            for s in sorted(bare):
                if not sha_exists(s):
                    red.append(f"{where}: {rid} names sha {s} which is not a commit in this repo")
                elif head in FETCHED and not reachable(s):
                    red.append(f"{where}: {rid} is `{head}` but sha {s} is not an ancestor of any track head")

# Evidence excerpts (pi 6, 2026-09-05): a serial capture is append-only across many boots, so an excerpt
# without its BOOT ANCHOR is unidentifiable. Every *.log under docs/dev/evidence must carry one:
# aarch64 `size 0x…` (the loader's kernel8 size line), x86 `img=[…` (the WXN mapped span), or the Orin's
# UEFI loader identity `KELF min=0x… max=0x…` (orin 13, 2026-09-05 — the Orin has no kernel8 size line).
for lg in sorted(glob.glob("docs/dev/evidence/**/*.log", recursive=True)):
    try: body = open(lg, errors="replace").read()
    except OSError: body = ""
    if not re.search(r"size 0x[0-9a-fA-F]+|img=\[0x[0-9a-fA-F]+|KELF min=0x[0-9a-fA-F]+ max=0x[0-9a-fA-F]+", body):
        red.append(f"{lg}: evidence excerpt carries no boot anchor (`size 0x…`, `img=[…` or `KELF min=0x… max=0x…`) — unidentifiable")

# RULINGS.md (pi 6, 2026-09-05): rulings get reversed (the cube, EVAC); an append-only quote file lets a
# reader find only the dead one. Every R-row carries status ∈ {live, superseded, retracted} and, when
# not live, a `superseded-by` that resolves to another R-id (or the word `retracted`).
if os.path.exists("docs/dev/RULINGS.md"):
    rtext = open("docs/dev/RULINGS.md").read(); rids = set(); rrows = []
    for hdr, rows in tables(rtext):
        st = col(hdr, "status"); sb = col(hdr, "superseded")
        if st is None: continue
        for ln, cells in rows:
            m = re.match(r"(R[0-9]+)", cells[0])
            if not m: red.append(f"docs/dev/RULINGS.md:{ln}: row id `{cells[0][:20]}` is not R<n>"); continue
            rids.add(m.group(1)); rrows.append((ln, m.group(1), cells, st, sb))
        rows_seen += len(rows)
    for ln, rid, cells, st, sb in rrows:
        status = re.sub(r"[*_`]", "", cells[st]).strip().lower() if st < len(cells) else ""
        if status not in ("live", "superseded", "retracted"):
            red.append(f"docs/dev/RULINGS.md:{ln}: {rid} status `{status[:20]}` not in live|superseded|retracted")
        if status == "superseded":
            tgt = re.findall(r"R[0-9]+", cells[sb]) if (sb is not None and sb < len(cells)) else []
            if not tgt or any(t not in rids for t in tgt):
                red.append(f"docs/dev/RULINGS.md:{ln}: {rid} is superseded but names no existing R<n> in superseded-by")

# BULLET ROWS ARE ROWS TOO (pi 7's class, second half, 2026-09-06). The scan above walks TABLE rows,
# so `docs/dev/LEDGER.md`'s protocol entries — `- **P14** — …` bullets, 15 of them — were never
# scanned for cross-refs at all: a `→ S<n>` written inside a P-row has never been resolved. That is the
# same shape as the two defects fixed today (a `→ P<n>` the resolver accepted but could never resolve;
# LEDGER.md's own table refs exempted): **the id-space the gate accepts, the id-space it can resolve,
# and the file-space it actually scans have to be the same three sets.** Routed through `_check_ref` so
# this half and the table half cannot drift. Measured before enabling: the 15 bullets carry 2 refs, both
# `→ SR1`, which resolves here and defers correctly on a branch without it.
if "docs/dev/LEDGER.md" in files:
    for _ln, _line in enumerate(open("docs/dev/LEDGER.md").read().split("\n"), 1):
        _m = re.match(r"^-\s+\*\*([A-Z]+[0-9]+)\*\*", _line)
        if not _m:
            continue
        for _ref in re.findall(r"→\s*((?:S[PRO]?|P)[0-9]+)", _line):
            _check_ref(_ref, f"docs/dev/LEDGER.md:{_ln}", _m.group(1), red, deferred, ledger_ids, files, STRICT)

for p in skipped: say(f"SKIP {p} — not in this tree (arrives at the trunk sync)")
if rows_seen == 0:
    say("NO VERDICT — no ledger rows found in", files or "(no ledger files)"); sys.exit(2)
if deferred:
    say(f"DEFERRED — {len(deferred)} cross-branch cross-ref(s); NOT findings. Strict is {STRICT_WHY};"
        f" these become reds automatically when this lands on `{TRUNK}`:")
    for d in deferred: print("   ", d)
# DEDUPE, for the reason f9255b68 deduped shas: a row citing the same missing id three times printed
# three identical findings, and duplicate findings are how a gate teaches people to skim its output.
# Order-preserving so the first occurrence still reads in file order.
red = list(dict.fromkeys(red))
deferred = list(dict.fromkeys(deferred))
if red:
    say(f"RED — {len(red)} finding(s) across {len(files)} file(s), {rows_seen} rows:")
    for r in red: print("   ", r)
    sys.exit(1)
_defnote = f", {len(deferred)} cross-branch ref(s) deferred" if deferred else ""
say(f"OK — {rows_seen} rows in {len(files)} ledger file(s) + RULINGS: ids unique, status ∈ enum, owners known, cross-refs resolve{_defnote}, shas exist, evidence in git and anchored, rulings live or superseded-by a real R<n>")
PY
# GO-RED PROOF (tree mutation, run before shipping; each reverted after):
#   duplicate id           -> RED naming the line       status "standing"      -> RED (outside the enum)
#   `→ S999` in a row      -> RED (dangling), when LEDGER.md is present
#   sha deadbeef1 in a row -> RED (not a commit)         `~/unaos-bench/scratch/x` in a row -> RED
#   owner "peter"          -> RED                        `S99` in a PARAGRAPH  -> GREEN (prose control)
#   evidence/*.log without `size 0x`/`img=[` -> RED      RULINGS R-row status `pending` -> RED
#   RULINGS `superseded` with no R<n> in superseded-by -> RED
#   `→ SO99` in a row      -> DEFERRED, exit 0, PRINTED (SO is branch-local; hw-jetson owns it)
#   the same under UNAOS_LEDGER_STRICT=1 -> RED, exit 1   (the landing's setting)
#   `→ SR2` on hw-rmbp     -> resolves, neither red nor deferred (the control: the check still fires
#                             where the target is local, which is the half a blanket skip would lose)
