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

ledger_ids = set()
if "docs/dev/LEDGER.md" in files:
    for hdr, rows in tables(open("docs/dev/LEDGER.md").read()):
        if col(hdr, "status") is None: continue
        for _, cells in rows:
            m = re.match(r"([A-Z]+[0-9]+)", cells[0])
            if m: ledger_ids.add(m.group(1))

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
            # into a ledger header, where the sentence documenting the test became a failing
            # input to the test.
            for ref in re.findall(r"→\s*((?:S[PRO]?|P)[0-9]+)", rowtext):
                if "docs/dev/LEDGER.md" in files and ref not in ledger_ids and path != "docs/dev/LEDGER.md":
                    red.append(f"{where}: {rid} cross-ref → {ref} does not resolve in docs/dev/LEDGER.md")
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
            for m in re.finditer(r"(?<![0-9A-Za-z])([0-9a-f]{7,8})(?![0-9A-Za-z])", rowtext):
                s = m.group(1)
                if re.search(r"[A-Za-z][A-Za-z0-9_-]*:$", rowtext[max(0, m.start() - 24):m.start()]):
                    continue   # author-declared artifact digest, not a commit
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

for p in skipped: say(f"SKIP {p} — not in this tree (arrives at the trunk sync)")
if rows_seen == 0:
    say("NO VERDICT — no ledger rows found in", files or "(no ledger files)"); sys.exit(2)
if red:
    say(f"RED — {len(red)} finding(s) across {len(files)} file(s), {rows_seen} rows:")
    for r in red: print("   ", r)
    sys.exit(1)
say(f"OK — {rows_seen} rows in {len(files)} ledger file(s) + RULINGS: ids unique, status ∈ enum, owners known, cross-refs resolve, shas exist, evidence in git and anchored, rulings live or superseded-by a real R<n>")
PY
# GO-RED PROOF (tree mutation, run before shipping; each reverted after):
#   duplicate id           -> RED naming the line       status "standing"      -> RED (outside the enum)
#   `→ S999` in a row      -> RED (dangling), when LEDGER.md is present
#   sha deadbeef1 in a row -> RED (not a commit)         `~/unaos-bench/scratch/x` in a row -> RED
#   owner "peter"          -> RED                        `S99` in a PARAGRAPH  -> GREEN (prose control)
#   evidence/*.log without `size 0x`/`img=[` -> RED      RULINGS R-row status `pending` -> RED
#   RULINGS `superseded` with no R<n> in superseded-by -> RED
