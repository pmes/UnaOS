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
# CONTRACT, SECOND CUT (rmbp 20, 2026-09-15, LEDGERGATES — LEDGER SR13/SR11/SR12 + LAWS §3 Queues):
#   * STRICT IS DECIDED BY CONTENT, NOT BY A REF NAME (SR13). Strict arms when HEAD's sha is an
#     ancestor of — or equal to — the trunk ref, or when `UNAOS_LEDGER_STRICT=1`. Every run prints
#     `strict=by-env|by-ancestry|off reason=…`, so the posture is never silent.
#   * A DEFERRAL MUST BE KEEPABLE (SR12). A row whose cross-ref DEFERS names an OWNER (a track:
#     rmbp | orin | pi | trunk — the `owner` column counts) and an EXPIRY (a date `YYYY-MM-DD`, a
#     commit sha this repo resolves, or a blocking id written `blocked on <ID>` / `until <ID>` /
#     `expiry=<…>`). Missing either is RED. Today's deferrals are grandfathered ONLY by DEFERRAL_REG.
#   * THE GATE'S OWN OUTPUT IS ESCAPED (SR11). Every emitted line goes through ONE escape for quoted
#     material: the harness fault-scan token family (arroyo's FAULT_PATTERNS) and the markdown cell
#     delimiter are rewritten, so a ledger-check log fed to `scan_serial_faults` cannot be mis-scored
#     and a finding line can be pasted into a ledger cell without shifting its columns.
#   * THE FOUR QUEUE FILES ARE SCANNED (LAWS §3 Queues) — `docs/dev/QUEUE.md` and
#     `docs/dev/OS/{rmbp,orin,pi}-queue.md` — for exactly two things: (a) no git conflict marker, in
#     the queues AND the ledgers AND RULINGS.md; (b) every ledger id a queue row cites exists in some
#     ledger file in this tree, grandfathered by QUEUECITE_REG.
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

# GATE-LEDGER FIELD COUNT (rmbp-ledger B63, built rmbp 17 2026-09-08). A markdown row is its PIPES,
# so a literal `|` inside a cell silently becomes a column boundary — and `\|` does NOT save it in
# this parser. The failure is nasty because it is not silent in a useful way: the row keeps parsing,
# the columns SHIFT, and what goes red is `owner` or `status` naming a fragment of somebody's prose.
# That is a gate reporting a symptom two columns downstream of the defect. Assert the count directly,
# name the sign, and STOP scoring the row — every later check on it would be reading the wrong cells.
#
# REGISTERED EXCEPTIONS, and they are registrations rather than a skip list: each is PRINTED on every
# run, and one that no longer matches a real mismatch goes RED. An allowlist nothing can falsify is
# the thing this lane keeps convicting (rmbp-ledger B95/B96/B98/B101), so this one is falsifiable in
# both directions. Both entries must reach zero; neither is a decision to leave the row broken.
# GATE-LEDGER ABSENCE FORM (rmbp-ledger B107, built rmbp 17 2026-09-09). A row that asserts a thing
# is ABSENT FROM A POPULATION has to name the population and show it was ENUMERATED. Three times in
# one round this seat wrote "nothing does X" after searching one FILE, and the claim was about a
# CAPABILITY; each search was correct about its scope and confidence tracked thoroughness WITHIN the
# scope, which is exactly what a scope error cannot show you. orin 23's form, adopted: an absence is
# written `population=<the command that enumerates it>, hits=0`, or it is written "not found in
# <scope>". The moment it fires is typing the word -- which is what makes it addressable at all.
#
# ⛔ WHAT THIS CHECK IS AND IS NOT, because overselling it would be the defect it exists for.
# It is a LEXICAL SAMPLER over phrasings that were thought of, NOT proof that every absence claim in
# the corpus is enumerated -- "all the ways English says nothing does X" is itself a population that
# cannot be enumerated, so this gate has a blind spot BY CONSTRUCTION and its own summary says so.
# A green here means "no row matched a known absence phrasing without an enumeration", never
# "every absence claim is sound".
#
# MEASURED BEFORE IT SHIPPED, and the measurement chose the pattern list. A WIDE set (no such / is
# not in / none of them / does not exist / zero hits / ...) matched 32 of 213 rows and would have RED-
# LINED 10, of which roughly 8 were FALSE POSITIVES: quoted program output ("no such file or
# directory" in an -ENOENT log line), a correction of a PEER's claim, a statement about a Rust type,
# and rows whose proof was real but not command-shaped. A gate reding honest rows in two other seats'
# files is worse than the defect, so the wide set was DROPPED rather than papered over with
# exceptions. The TIGHT set below matched 9 rows, every one a genuine population-absence claim, and
# all 9 are this lane's own -- zero in orin's or pi's ledgers today.
ABSENCE_PHRASE = re.compile(
    r"\b(no [a-z]+ verb|run by nothing|nothing (?:runs|invokes|opens|does) |exists? only on"
    r"|on no other head|zero commits|no commits on any branch)\b", re.I)
# An enumeration is a COMMAND that walks the population, or the honest downgrade "not found in".
ABSENCE_ENUM = re.compile(
    r"`[^`]*(grep|git log|git grep|git cat-file|git rev-list|ls |find |for [a-z] in|awk)[^`]*`"
    r"|not found in", re.I)
ABSENCE_REG = {
    # Same falsifiable shape as FIELDCOUNT_REG: printed every run, and a registration whose row no
    # longer matches goes RED. Empty today -- every matching row already carries its enumeration.
}
abs_seen = set()
absence = []

FIELDCOUNT_REG = {
    "docs/dev/OS/rmbp-ledger.md:B24": "three injections put TWO rows' content on one line; splitting them is a CONTENT call, and the ledger-cell pipe convention it belongs to is Peter's open decision (rmbp-ledger J6). Registered 2026-09-08 by rmbp 17",
}
fc_seen = set()
registered = []
FETCHED = ("fixed-unflown", "flown", "landed")
files = [p for p in ["docs/dev/LEDGER.md"] + sorted(glob.glob("docs/dev/OS/*-ledger.md")) if os.path.exists(p)]
skipped = [p for p in ["docs/dev/LEDGER.md"] if not os.path.exists(p)]
red = []

# ── SR11: ONE ESCAPE FOR QUOTED MATERIAL, APPLIED AT THE OUTPUT BOUNDARY ────────────────────────
# LEDGER SR11 names three instances of this gate's NOTATION colliding with its SUBJECT MATTER and
# predicts a fourth. Two of them already have a special-case escape on the INPUT side (ASCII `->`
# for a cross-ref mention, `label:hash` for an artifact digest) and the row's own verdict is that
# "the shape of a fix is one general escape for quoted material rather than a third special case".
# This is that escape, and it is put where it cannot be forgotten: the OUTPUT boundary. Every line
# this gate prints goes through it, so gate literals and quoted row text are covered by one
# mechanism and no future finding can smuggle a colliding token out.
#
# WHAT COLLIDES, both halves MEASURED before this shipped rather than reasoned:
#   (1) THE HARNESS FAULT-SCAN FAMILY. `unaos/arroyo`'s FAULT_PATTERNS — the ONE list every serial
#       verdict in the harness reads, and mbench's DEFAULT_FORBIDS beside it — is
#       `-> FAIL | FAIL :: | FAIL — | PANIC | panicked at | EXCEPTION:`. A ledger row whose SUBJECT
#       is that idiom is ordinary in this corpus, and the gate echoes cell text into its findings:
#       a single injected row (status `standing -> FAIL :: PANIC`, owner `peter PANIC`) put TWO
#       gate output lines into that family. A ledger-check log concatenated into a run log, or
#       quoted into a row, is then scored as a kernel fault by a scanner that is right about its
#       pattern and wrong about its input.
#   (2) THE MARKDOWN CELL DELIMITER. This gate's own field-count check exists because a literal
#       pipe inside a cell silently becomes a column boundary (B63) — and the gate's diagnostics
#       printed the status enum pipe-separated, so its OWN output could not be pasted into a ledger
#       cell without shifting that row's columns. The diagnostics now separate with `·` and name the
#       delimiter BY NAME rather than by glyph; this escape is the backstop for anything left.
#
# The escaped form is DECLARED and uniform, never silent mangling: the colliding token T is printed
# as ⟨q:T with a `·` after its first character⟩, and a pipe as `¦`. Both are visible, both survive
# `LC_ALL=C grep -a`, and neither matches the pattern it escapes.
FAULT_TOKENS = re.compile(r"-> FAIL|FAIL ::|FAIL — |PANIC|panicked at |EXCEPTION:")
notation = 0
def _q(s):
    """The one escape. Applied to EVERY emitted line — see the SR11 note above."""
    global notation
    out = FAULT_TOKENS.sub(lambda m: "⟨q:" + m.group(0)[0] + "·" + m.group(0)[1:] + "⟩", s)
    out = out.replace("|", "¦")
    if out != s:
        notation += 1
    return out
def emit(s): print(_q(s))
def say(*a): emit("GATE-LEDGER: " + " ".join(str(x) for x in a))
def detail(s): emit("    " + s)

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
# ⚠⚠ SUPERSEDED IN PART, 2026-09-15 (LEDGER SR13, rmbp 20's LEDGERGATES) — READ THIS BEFORE THE
# THREE NOTES ABOVE, WHICH ARE KEPT BECAUSE THEY ARE THE ARGUMENT THIS FIX INHERITS, NOT BECAUSE
# THEY ARE STILL THE MECHANISM. **The trigger above was a REF NAME, and a ref name is not content.**
# `git rev-parse --abbrev-ref HEAD` returns the literal string `HEAD` in ANY detached checkout —
# including one sitting at trunk's own sha — which is the exact shape every executor and every
# peer-gating seat works in. Measured 2026-09-10 across five live checkouts: only `UnaOS` on `main`
# armed; `UnaOS-rmbp`, `UnaOS-orin`, `UnaOS-hw-pi4` and a detached scratch worktree all did not, and
# none of them was told. The gate's own STRICT — WIRED, NOT REMEMBERED argument convicts it: orin 25
# gated a landing four times from a detached worktree, got the deferring posture every time, and only
# saw the strict verdict on the fourth run after exporting the variable by hand.
#
# THE DECISION IS NOW MADE FROM CONTENT: strict arms when HEAD's sha is an ancestor of — or equal to
# — the trunk ref. The rule is about WHICH DIRECTION the ancestry runs, and the direction is the
# whole safety argument, so it is written out rather than left to be re-derived:
#   * HEAD ⊆ trunk  (trunk CONTAINS head)  → ARM. Everything in this tree is already trunk content,
#     so every cross-branch ref in it is resolvable here by construction, whatever ref name is
#     checked out and whether or not anything is checked out at all.
#   * HEAD ⊇ trunk  (head CONTAINS trunk)  → DO NOT ARM. This is a post-fold track tip, and SR13's
#     own counter-example is the one that rules it out: `hw-rmbp` at `a51a0396` contained trunk
#     `751cb816` for the three and a half hours between the fold and the landing. Arming there would
#     red on peer rows that are legitimately branch-local — the false-red the "zero rows of that
#     prefix" discriminator was already turned down for, and LAWS §5's wrong-strict-is-worse.
# `git merge-base --is-ancestor HEAD <trunk>` tests exactly the first and is false for the second, so
# SR13's "equality, never ancestry" residual is satisfied in substance: the direction it warned
# about is the one this test cannot take. It is WIDER than equality by exactly one case — a checkout
# of an OLDER trunk commit — and that case wants strict too, because that content was trunk content.
#
# The trunk ref is looked up as `UNAOS_LEDGER_TRUNK` (default `main`) and then `origin/<that>`, so a
# tree with no local trunk branch still arms; the rename hazard above is unchanged and is now LOUDER,
# because a trunk ref that resolves to nothing is printed as the reason strict is off instead of
# being invisible behind a name comparison. EVERY RUN PRINTS THE POSTURE AND WHY.
TRUNK = os.environ.get("UNAOS_LEDGER_TRUNK", "main")
_branch = subprocess.run(["git", "rev-parse", "--abbrev-ref", "HEAD"],
                         capture_output=True, text=True).stdout.strip()

def _rev(ref):
    r = subprocess.run(["git", "rev-parse", "--verify", "--quiet", ref + "^{commit}"],
                       capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else ""

def _is_ancestor(a, b):
    return subprocess.run(["git", "merge-base", "--is-ancestor", a, b],
                          capture_output=True).returncode == 0

_head_sha = _rev("HEAD")
_trunk_refs = [r for r in (TRUNK, "origin/" + TRUNK) if _rev(r)]
_armed_by = next((r for r in _trunk_refs if _head_sha and _is_ancestor(_head_sha, r)), None)
_where = f"HEAD {_head_sha[:8] or '(none)'} on `{_branch}`"
_env_strict = os.environ.get("UNAOS_LEDGER_STRICT")
if _env_strict == "0":
    STRICT, STRICT_MODE = False, "off"
    STRICT_WHY = f"strict=off reason=suppressed by UNAOS_LEDGER_STRICT=0 ({_where})"
elif _env_strict == "1":
    STRICT, STRICT_MODE = True, "by-env"
    STRICT_WHY = f"strict=by-env reason=forced by UNAOS_LEDGER_STRICT=1 ({_where})"
elif _armed_by:
    STRICT, STRICT_MODE = True, "by-ancestry"
    STRICT_WHY = (f"strict=by-ancestry reason={_where} is contained in `{_armed_by}` "
                  f"{_rev(_armed_by)[:8]} — this tree is trunk content, so every ref must resolve")
elif not _trunk_refs:
    STRICT, STRICT_MODE = False, "off"
    STRICT_WHY = (f"strict=off reason=trunk ref `{TRUNK}` (and `origin/{TRUNK}`) resolves to nothing "
                  f"in this repo — set UNAOS_LEDGER_TRUNK, or UNAOS_LEDGER_STRICT=1 ({_where})")
else:
    STRICT, STRICT_MODE = False, "off"
    STRICT_WHY = (f"strict=off reason={_where} is NOT contained in {' / '.join(_trunk_refs)} — track "
                  f"content, cross-branch refs deferred (UNAOS_LEDGER_STRICT=1 to require them now)")
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

# ── SR12: A DEFERRAL IS A PROMISE, AND NOTHING CHECKED THAT THE PROMISE WAS KEEPABLE ───────────
# LEDGER SR12's instance: SO6 deferred on every run, on every seat, for four days — and the row
# existed on NO ref and had never been written. The deferral was honest about what it was and said
# so in the gate's own output; what it could not say is whether the thing it was waiting for was
# ever going to arrive. A verdict that cannot tell WAITING from NEVER is not a verdict.
#
# THE CHEAP HALF, taken here: make the AUTHOR carry the promise. A row whose cross-ref defers must
# say who owes it and when it comes due — an OWNER (a track: rmbp | orin | pi | trunk; the row's
# `owner` column counts, since that column already names one) and an EXPIRY (a date, a commit sha
# this repo resolves, or a blocking id). A deferral missing either is RED, with the missing half
# named. That does not prove the target exists — nothing in one tree can — but it converts an
# anonymous, unbounded wait into a claim somebody made, which is the thing that can be checked
# later and the thing SO6 never had.
#
# GRANDFATHERING, and it is the FIELDCOUNT_REG mechanism rather than a skip list: today's deferrals
# are exempt ONLY by an explicit entry in DEFERRAL_REG, each carrying its reason and the words that
# make it falsifiable — it must reach zero, it is PRINTED on every run, and an entry whose row no
# longer defers goes RED as stale. DEFERRAL_REG IS EMPTY, and that is a measurement, not a hope:
# `bash unaos/scripts/ledger-check.sh` on this tree prints no DEFERRED line, and neither does
# `UNAOS_LEDGER_STRICT=1` (rmbp 20, 2026-09-15, at 2051470a — 362 rows, 4 ledger files, zero
# unresolved cross-refs in either posture). The list exists so the next deferral cannot be added
# without one.
DEFERRAL_OWNER = re.compile(r"\b(rmbp|orin|pi|trunk)\b", re.I)
DEFERRAL_EXPIRY_DATE = re.compile(r"\b20[0-9]{2}-[0-9]{2}-[0-9]{2}\b")
DEFERRAL_EXPIRY_ID = re.compile(r"\b(?:blocked on|blocker|until|expiry=)\s*`?([A-Z]+[0-9]+|\S+)", re.I)
DEFERRAL_REG = {
    # "<ledger path>:<row id>": "<reason> — must reach zero"
}
def_seen = set()

def _deferral_promise(rowtext):
    """Return the list of MISSING halves of a deferral's promise: owner, expiry, or neither."""
    missing = []
    if not DEFERRAL_OWNER.search(rowtext):
        missing.append("owner (rmbp · orin · pi · trunk)")
    ok_expiry = bool(DEFERRAL_EXPIRY_DATE.search(rowtext)) or bool(DEFERRAL_EXPIRY_ID.search(rowtext))
    if not ok_expiry:
        for m in re.finditer(r"(?<![0-9A-Za-z])([0-9a-f]{7,8})(?![0-9A-Za-z])", rowtext):
            if sha_exists(m.group(1)):
                ok_expiry = True
                break
    if not ok_expiry:
        missing.append("expiry (a date, a sha this repo resolves, or `blocked on <ID>` / `until <ID>` / `expiry=…`)")
    return missing

def _check_ref(ref, where, rid, red, deferred, ledger_ids, files, STRICT, rowtext="", path=""):
    """One home for the resolve/defer/red decision, so the TABLE scan and the BULLET scan below
    cannot drift apart — two copies of this logic is how one of them silently stops matching."""
    if "docs/dev/LEDGER.md" not in files or ref in ledger_ids:
        return
    pfx = re.match(r"([A-Z]+)", ref).group(1)
    if pfx in ("SR", "SO", "SP") and not STRICT:
        deferred.append(f"{where}: {rid} cross-ref → {ref} DEFERRED — {pfx} rows are branch-local; resolves when that seat's ledger lands (UNAOS_LEDGER_STRICT=1 to require it now)")
        _k = f"{path}:{rid}"
        if _k in DEFERRAL_REG:
            def_seen.add(_k)
            deferred.append(f"{where}: {rid} deferral REGISTERED — {DEFERRAL_REG[_k]}")
            return
        missing = _deferral_promise(rowtext)
        if missing:
            red.append(f"{where}: deferral {rid} → {ref} has no {' and no '.join(missing)} — a deferral nobody owns and nothing expires is kept forever (LEDGER SR12)")
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
            # FIELD COUNT first: every check below indexes cells by header position, so a shifted row
            # makes all of them read the wrong text. Report the row, not its symptom, and move on.
            if len(cells) != len(hdr):
                _k = f"{path}:{rid}"
                _d = len(cells) - len(hdr)
                if _k in FIELDCOUNT_REG:
                    fc_seen.add(_k)
                    registered.append(f"{where}: {rid} fields={len(cells)} header={len(hdr)} ({_d:+d}) REGISTERED — {FIELDCOUNT_REG[_k]}")
                else:
                    _why = ("a literal PIPE character inside a cell splits the row, and a backslash does NOT save it — reword it"
                            if _d > 0 else "a cell is missing; every column needs one, `—` for empty")
                    red.append(f"{where}: {rid} has {len(cells)} fields, header has {len(hdr)} ({_d:+d}) — {_why}")
                continue
            if rid in ids: red.append(f"{where}: duplicate id {rid}")
            _rowtext = " | ".join(cells)
            _ap = ABSENCE_PHRASE.search(_rowtext)
            if _ap and not ABSENCE_ENUM.search(_rowtext):
                _k = f"{path}:{rid}"
                if _k in ABSENCE_REG:
                    abs_seen.add(_k)
                    absence.append(f"{where}: {rid} absence claim {_ap.group(0)!r} REGISTERED — {ABSENCE_REG[_k]}")
                else:
                    red.append(f"{where}: {rid} asserts an absence ({_ap.group(0)!r}) and names no ENUMERATION — give the command that walks the population, or write \"not found in <scope>\"")
            ids.add(rid)
            status_raw = cells[st] if st < len(cells) else ""
            status = re.sub(r"[*_`]", "", status_raw).strip()
            head = re.split(r"\s+—|,|\s+\(|\s+/|\s+until|\s+—", status)[0].strip().lower()
            if head not in ENUM:
                red.append(f"{where}: {rid} status `{status_raw[:40]}` does not begin with one of {' · '.join(ENUM)}")
            if ow is not None and ow < len(cells):
                first = re.sub(r"[*_`]", "", cells[ow]).strip().split()
                if not first or first[0].strip(",;") not in OWNERS:
                    red.append(f"{where}: {rid} owner `{cells[ow][:30]}` not in {' · '.join(sorted(OWNERS))}")
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
                _check_ref(ref, where, rid, red, deferred, ledger_ids, files, STRICT, rowtext, path)
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
            _check_ref(_ref, f"docs/dev/LEDGER.md:{_ln}", _m.group(1), red, deferred, ledger_ids, files, STRICT, _line, "docs/dev/LEDGER.md")

# ── THE QUEUE FILES (LAWS §3 Queues, R45) ──────────────────────────────────────────────────────
# LAWS §3 Queues ended "Enforcer: warning only until `ledger-check.sh` learns the queue files", and a
# warning nobody emits is the "backstop that runs never" this file already argues against twice. The
# queues are learned HERE and for exactly TWO checks, both cheap and both true. The scope is
# deliberately small: a queue is an ORDER, not a tracker, so its rows carry no status enum, no owner
# column and no field count to assert — importing the ledger contract wholesale would red honest
# rows in three seats' files, which is the mistake the ABSENCE pattern list was cut down to avoid.
QUEUE_PATHS = ["docs/dev/QUEUE.md", "docs/dev/OS/rmbp-queue.md",
               "docs/dev/OS/orin-queue.md", "docs/dev/OS/pi-queue.md"]
QUEUES = [p for p in QUEUE_PATHS if os.path.exists(p)]
queue_missing = [p for p in QUEUE_PATHS if not os.path.exists(p)]

# (a) CONFLICT MARKERS — the 2026-09-12 QUEUE §5 row, extended past the ledgers as that row asks.
# `ledger-check.sh` passed rc=0 on a LEDGER.md carrying three git conflict markers that a fold had
# committed (hw-jetson 4465eb20, fixed 7006857f): the markers sit OUTSIDE any table row, so every
# check in this file looked straight past them. LAWS §3 Conflicts already says to count markers
# before `git add`; this makes the count a gate. Scanned over the queues AND the ledgers AND
# RULINGS.md — the whole document set this gate is responsible for.
CONFLICT = re.compile(r"^(<{7} |={7}$|>{7} )")
conflict_scanned = sorted(set(QUEUES + files +
                              (["docs/dev/RULINGS.md"] if os.path.exists("docs/dev/RULINGS.md") else [])))
for p in conflict_scanned:
    for ln, line in enumerate(open(p, errors="replace").read().split("\n"), 1):
        if CONFLICT.match(line):
            red.append(f"{p}:{ln}: git conflict marker {line[:12].rstrip()!r} committed — a fold left it in (LAWS §3 Conflicts: count markers before `git add`)")

# (b) EVERY LEDGER ID A QUEUE ROW CITES MUST EXIST. The queue's own contract says so in its header:
# "Every row cites its ledger id — the ledger holds the finding, this file holds the ORDER." Nothing
# checked it, and the first armed run found that it is not true.
#
# THE PATTERN WAS MEASURED BEFORE IT WAS CHOSEN, and the measurement removed a prefix. The proposed
# set was `(S|SO|SP|SR|A|B|E)[0-9]+`. Over the four queue files at 2051470a it matches 592 tokens,
# 186 distinct. `E` was DROPPED: the only `E<n>` token in any queue file is `error[E0080]`, rustc's
# diagnostic code, cited twice as go-red evidence, while the three real E ids (E1, E2, E3) are cited
# by no queue row at all — so including `E` bought two false findings and zero true ones. The
# surviving set is 590 citations, 185 distinct. `A`/`B` stay in, and they are the reason the id set
# is the UNION of every ledger file in the tree rather than LEDGER.md alone: A is orin's arch prefix
# and B is rmbp's, and the trunk queue cites both by design.
#
# THREE VERDICTS, NOT TWO — and this is SR12's split, applied where it is affordable. An id that
# does not resolve HERE may still be a row somebody has written on their own branch; an id that
# resolves on NO ref is a citation to nothing, which is exactly SO6.
#   * resolves in this tree          -> OK.
#   * resolves on an enumerated head -> DEFERRED-KEEPABLE: printed with the ref that has it, counted,
#                                       never a finding. Measured at 2051470a: 0 here, and 5 on
#                                       `main` (A58, SO34, SO39, SO40, SO41 — all on hw-jetson, all
#                                       named by their own QUEUE rows as "FIXED ON hw-jetson, NOT YET
#                                       LANDED"). Reding those would red the trunk for saying
#                                       something true.
#   * resolves NOWHERE               -> RED, grandfathered only by QUEUECITE_REG below.
QUEUE_CITE = re.compile(r"\b(?:S|SO|SP|SR|A|B)[0-9]+\b")

def _ids_at(ref):
    out = set()
    for p in ["docs/dev/LEDGER.md"] + [f"docs/dev/OS/{s}-ledger.md" for s in ("rmbp", "orin", "pi")]:
        r = subprocess.run(["git", "show", f"{ref}:{p}"], capture_output=True, text=True)
        if r.returncode:
            continue
        out |= {m.group(1) for m in re.finditer(r"^\|\s*\**([A-Z]+[0-9]+)", r.stdout, re.M)}
        out |= {m.group(1) for m in re.finditer(r"^-\s+\*\*([A-Z]+[0-9]+)\*\*", r.stdout, re.M)}
    return out

# THE ID SET THE CITATIONS RESOLVE AGAINST is the union of EVERY ledger file in the tree, table rows
# and P-bullets alike — the same three sets the cross-ref resolver was fixed to keep aligned
# (pi 7's class): the id-space the gate accepts, the id-space it can resolve, and the file-space it
# scans. `ledger_ids` above holds LEDGER.md only, which is right for `→ S<n>` and wrong here.
ledger_all_ids = set()
for _p in files:
    _t = open(_p).read()
    ledger_all_ids |= {m.group(1) for m in re.finditer(r"^\|\s*\**([A-Z]+[0-9]+)", _t, re.M)}
    ledger_all_ids |= {m.group(1) for m in re.finditer(r"^-\s+\*\*([A-Z]+[0-9]+)\*\*", _t, re.M)}

# THE GRANDFATHER LIST, and every entry is a MEASURED fact rather than a decision to look away.
# Registered 2026-09-15 by rmbp 20 (LEDGERGATES) on the check's first armed run. `docs/dev/OS/
# orin-queue.md` cites 35 ledger ids that exist in NO ledger file on ANY of the nine enumerated
# heads — hw-jetson, orin's own branch, included — so this is not a cross-branch artefact but rows
# that were never written. That is SO6's shape at scale and it is orin's to close; it is registered
# rather than red because a gate reding another seat's file on the day it ships is the failure this
# file's own ABSENCE note records paying for once already. MUST REACH ZERO. Falsifiable in both
# directions: each entry is PRINTED every run, and an entry whose id starts resolving anywhere goes
# RED as stale — which is what happens on hw-jetson the moment orin writes the row.
QUEUECITE_REG = {
    f"docs/dev/OS/orin-queue.md:{_i}":
        "orin-queue cites a row that exists in no ledger on any head (measured over 9 heads, "
        "2026-09-15, rmbp 20) — orin's to write or to strike; must reach zero"
    for _i in ("A66 A67 A68 A69 A70 A71 A72 A73 A74 A75 A76 A78 A79 A80 A82 A83 A84 A85 A86 A87 "
               "A89 A90 A91 A92 A94 A95 S33 S34 SO43 SO44 SO45 SO46 SO47 SO48 SO49").split()
}
# COLLAPSED BY (file, id), NOT one line per occurrence: the first cut printed 202 identical-shaped
# REGISTERED lines on every green run, which is LAWS §5's "22 names on every run trains the eye to
# skip the region" four times over. The population is 35 ids, so 35 is what the reader is shown.
qc_seen = set()
queue_deferred = {}
queue_registered = {}
cite_total = 0
cite_distinct = set()
_peer_ids = None
for p in QUEUES:
    for ln, line in enumerate(open(p, errors="replace").read().split("\n"), 1):
        for m in QUEUE_CITE.finditer(line):
            cid = m.group(0)
            cite_total += 1
            cite_distinct.add(cid)
            _k = f"{p}:{cid}"
            if cid in ledger_all_ids:
                # FALSIFIED THE OTHER WAY: checked BEFORE the skip, or a registration for an id that
                # lands in THIS tree's ledger could never go stale — the registration would quietly
                # outlive the defect, which is the "allowlist nothing can falsify" this lane keeps
                # convicting. Local resolution is the commonest way one of these will close.
                if _k in QUEUECITE_REG:
                    qc_seen.add(_k)
                    red.append(f"stale queue-citation registration {_k} — {cid} resolves in this tree's ledgers now; delete the entry")
                continue
            if _peer_ids is None:          # paid for once, and only if something actually misses
                _peer_ids = {h: _ids_at(h) for h in HEADS}
            on = [h for h, s in sorted(_peer_ids.items()) if cid in s]
            if on:
                queue_deferred.setdefault((p, cid), [on, 0, ln])[1] += 1
                if _k in QUEUECITE_REG:
                    qc_seen.add(_k)
                    red.append(f"stale queue-citation registration {_k} — {cid} resolves on {on[0]} now; delete the entry")
            elif _k in QUEUECITE_REG:
                qc_seen.add(_k)
                queue_registered.setdefault((p, cid), [0, ln])[0] += 1
            else:
                red.append(f"{p}:{ln}: cites ledger id {cid}, which exists in no ledger file in this tree and on no enumerated head — a citation to nothing (LEDGER SR12's shape)")
# IDLE IS NOT STALE, and the first cut got this wrong in a way worth recording: it red-lined an
# unmatched registration the way FIELDCOUNT_REG does, and a detached worktree at trunk's own sha
# then went RED 33 times — because `main`'s copy of orin-queue.md simply predates those citations.
# The queue files differ by BRANCH, so "this registration matched nothing here" is a fact about
# which commit is checked out, not about the defect. The entry is FALSIFIED only by the id starting
# to resolve (handled above, and it is the direction that matters: it fires on hw-jetson the moment
# orin writes the row). An IDLE entry is counted in the census instead of reported — visible, and
# not a finding.
queue_reg_idle = sum(1 for _k in QUEUECITE_REG if _k not in qc_seen and _k.split(":")[0] in QUEUES)

# ── THE FOLD SHAPES (QUEUEGATE, rmbp 2026-09-16) ───────────────────────────────────────────────
# Three checks, one occasion. On 2026-09-16 this seat folded 12 executor branches and every defect
# below was CLEAN-AUTO-MERGED into a tree that this script then passed at rc=0. That is the whole
# argument for putting them here rather than in a seat's fold helper: a helper is run by the seat
# that wrote it, and the three findings were caught late, by eye, or at the NEXT fold.
#
#   (a) 1878e035 — two `✓ Executors cut from 160176d2 …` lines in `docs/dev/OS/rmbp-queue.md`'s
#       STATE block, the second a strict PREFIX-VARIANT of the first. Git merged both sides' edits
#       to the same logical line and neither side's text was lost, which is a correct merge of a
#       file and a wrong merge of a STATE.
#   (b) fb29c268 — a fold helper grepped BOTH of those copies into one 64 KB line and spliced it
#       over a conflict block holding three METAL rows; `X86BIND`, `AHCIFLY` and `AHCIWRITE` left
#       the queue (AHCIWRITE one merge earlier, at a84c8f32). Repaired at 60c1954d by rebuilding
#       the rows from git history. ledger-check rc=0 at every step.
#   (d) rmbp-ledger `A1` and `B89` carried appended status text AFTER the row's final pipe. The
#       FIELD COUNT check found them only at a LATER fold: a row whose trailing text happens to
#       land on the header's own column count parses clean, and the text is silently absorbed into
#       the last cell. Live instance, still readable:
#       `git show 754f9107:docs/dev/OS/rmbp-ledger.md | sed -n '21p;123p'`.
#
# ALL THREE ARE CHEAP AND NONE OF THEM IMPORTS THE LEDGER CONTRACT INTO THE QUEUES — the scope
# restraint the queue block above was written with is kept: a queue row still carries no status
# enum, no owner and no field count.
QMARK = "✓·⚠⛔"
_HEAD_RE = re.compile(r"^##\s")
_STATE_HEAD = re.compile(r"^##\s.*\bSTATE\b")
LEDGER_PATHS = set(["docs/dev/LEDGER.md"] + [f"docs/dev/OS/{_s}-ledger.md" for _s in ("rmbp", "orin", "pi")])

def _marked(line):
    """The queue's own row shape: a status mark, then the row. Returns the row, whitespace-collapsed."""
    s = line.strip()
    if not s or s[0] not in QMARK:
        return None
    return re.sub(r"\s+", " ", s[1:]).strip()

def _state_sections(text):
    """[(heading lineno, heading, [(lineno, line)])] for every `## …STATE…` section.

    Used for the `DROPPED <id>` escape below, NOT for the uniqueness check — see _state_dupes.
    """
    out = []
    cur = None
    for i, l in enumerate(text.split("\n"), 1):
        if _HEAD_RE.match(l):
            cur = (i, l, []) if _STATE_HEAD.match(l) else None
            if cur:
                out.append(cur)
        elif cur is not None:
            cur[2].append((i, l))
    return out

# THE LENGTH FLOOR IS THE FALSE-POSITIVE CONTROL. Short marked lines in these files are one-word
# statuses and pointers (`· x86 legs (R39): …`) and two of them sharing an opening is ordinary.
# 40 chars is where a queue line is carrying a claim.
STATE_MIN = 40

def _state_dupes(text):
    """[(line_a, line_b, shorter_text)] — two marked lines, one a PREFIX of the other.

    PREFIX, not equality, and the incident is why: the second `✓ Executors cut from 160176d2 …`
    line was the first with more text appended. An equality test sees two different strings and
    says nothing. The prefix test is the shape a same-line merge actually produces.

    WHOLE FILE, NOT THE `## STATE` SECTION, and the widening was forced by a measurement rather
    than chosen. Keying on the section was the first cut: there is no universal LINE prefix to key
    on (`✓ Executors cut from` is rmbp's alone — QUEUE.md's block opens `✓ main <sha> = …`, orin's
    `✓ hw-jetson <sha> = origin …`, pi's `✓ origin/hw-pi4 <sha>; …`), so the section looked like
    the invariant. It is not: by fb29c268 the duplicated pair had MIGRATED, one copy still in the
    STATE block and one under `## METAL`, and a section-scoped check reports nothing on the very
    tree the row-loss happened in. At f8f8ce8c the surviving line sits under `## METAL` outright.
    The file is the scope.
    MEASURED, and the numbers are why this is affordable: over the last 80 commits on `hw-rmbp`
    exactly 7 carry a finding — 1878e035 (where the duplicate was merged in) through 3164a3c0, the
    contiguous run that ends at the repair 60c1954d — and the other 73 are silent. Over all eight
    track heads (`main`, `hw-jetson`, `hw-pi4`, `hw-rmbp` and their `origin/` counterparts): zero.
    One defect, found on every commit that carried it, no false reds on any seat's file.
    """
    marked = [(i, m) for i, l in enumerate(text.split("\n"), 1)
              for m in (_marked(l),) if m and len(m) >= STATE_MIN]
    found = []
    for a in range(len(marked)):
        for b in range(a + 1, len(marked)):
            ia, la = marked[a]
            ib, lb = marked[b]
            if lb.startswith(la) or la.startswith(lb):
                found.append((ia, ib, la if len(la) <= len(lb) else lb))
    return found

def _show(ref, path):
    """That path's content at that ref, or None when the ref does not carry the file."""
    r = subprocess.run(["git", "show", f"{ref}:{path}"], capture_output=True, text=True)
    return None if r.returncode else r.stdout

def _rows_of(text, path):
    """{row key: first line number}. A ledger row is its ID; a queue row is its first 40 chars."""
    out = {}
    if text is None:
        return None
    for i, l in enumerate(text.split("\n"), 1):
        if path in LEDGER_PATHS:
            m = re.match(r"^\|\s*\**([A-Z]+[0-9]+)", l)
            if m:
                out.setdefault(m.group(1), i)
        else:
            m = _marked(l)
            if m and len(m) >= 20:
                out.setdefault(m[:40], i)
    return out

def _row_losses(W, A, B, M):
    """Rows absent from the working file W that BOTH parents had. Three-way, at row granularity.

    THE RULE IS NARROW ON PURPOSE AND THE WIDE ONE WAS MEASURED AND DROPPED. The wide rule adds
    "present in ONE parent and not in the merge base" — a row added on one side and lost. It is
    correct in principle and it false-reds in practice: over the last 60 merges on `hw-rmbp` the
    narrow rule finds exactly 3 rows and every one is a real loss (`X86BIND` and `AHCIFLY` at
    fb29c268, `AHCIWRITE` at a84c8f32 — the three the repair commit 60c1954d had to rebuild from
    history), while the wide rule finds 6 and the three extra are REWORDS: a row whose first 40
    chars were edited on one side reads as "added there, lost here". A gate reding an honest
    rewrite on every fold is a gate that gets skipped by the second week, which is the trade the
    ABSENCE pattern list above was already cut down for. Narrow catches every row this class has
    ever actually cost, at zero false positives over 60 folds.
    """
    return sorted(k for k in (set(A) & set(B)) if k not in W)

# GO-RED CONTROLS, RUN BEFORE ANY VERDICT AND ON SYNTHETIC INPUT, the way GATE-SPECROOTS does it.
# A scan that matches nothing reports zero, and zero reads as a clean tree; these make the zero
# distinguishable from a broken pattern. Each control asserts BOTH directions — the shape that must
# fire, and the near-miss that must stay silent — because a check that reds on everything is as
# useless as one that reds on nothing. A control failure is exit 2, and a gate that gave no verdict
# is NOT a pass.
_CTL = []
_ctl_state_red = ("## STATE — control fixture\n"
                  "✓ Executors cut from 160176d2 this session (branches exec-rmbp-*, worktrees)\n"
                  "· an unrelated state line, long enough to clear the forty-character floor\n"
                  "✓ Executors cut from 160176d2 this session (branches exec-rmbp-*, worktrees) and two more\n")
_ctl_state_green = ("## STATE — control fixture\n"
                    "✓ Executors cut from 160176d2 this session (branches exec-rmbp-*, worktrees)\n"
                    "· an unrelated state line, long enough to clear the forty-character floor\n"
                    "· a short line\n"
                    "· a short line\n"
                    "prose repeating Executors cut from 160176d2 this session (branches exec-rmbp-*, worktrees)\n"
                    "prose repeating Executors cut from 160176d2 this session (branches exec-rmbp-*, worktrees)\n")
_ctl_state_cross = ("## STATE — control fixture\n"
                    "✓ Executors cut from 160176d2 this session (branches exec-rmbp-*, worktrees)\n"
                    "## METAL — the pair MIGRATED across a heading at fb29c268; the file is the scope\n"
                    "✓ Executors cut from 160176d2 this session (branches exec-rmbp-*, worktrees) and more\n")
if len(_state_dupes(_ctl_state_red)) != 1:
    _CTL.append("STATE-LINE UNIQUENESS: the prefix-variant fixture was not found")
if len(_state_dupes(_ctl_state_cross)) != 1:
    _CTL.append("STATE-LINE UNIQUENESS: the migrated pair was not found; the scan is section-scoped again")
if len(_state_dupes(_ctl_state_green)) != 0:
    _CTL.append("STATE-LINE UNIQUENESS: prose or a short line was judged; only marked rows over the floor are")

_ctl_q = "docs/dev/OS/rmbp-queue.md"
_ctl_A = _rows_of("· X86BIND x86 binds / by content, a row long enough to be a row\n"
                  "· AHCIFLY score the read-only AHCI driver on the rMBP's own metal\n"
                  "· STRUCK a row that one side deliberately removed at this fold\n", _ctl_q)
_ctl_M = dict(_ctl_A)
_ctl_B = {k: v for k, v in _ctl_A.items() if not k.startswith("STRUCK")}
if len(_ctl_B) != len(_ctl_A) - 1:
    _CTL.append("ROW CONTINUITY: the fixture's struck row was not built; the row parser changed shape")
_ctl_W = {}
_ctl_loss = _row_losses(_ctl_W, _ctl_A, _ctl_B, _ctl_M)
if len(_ctl_loss) != 2:
    _CTL.append(f"ROW CONTINUITY: the two-parent fixture yielded {len(_ctl_loss)} loss(es), expected 2")
if any(k.startswith("STRUCK") for k in _ctl_loss):
    _CTL.append("ROW CONTINUITY: a row one parent deliberately struck was reported as a loss")

def _tail_bad(line):
    """A ledger row that does not END with `|` after trimming — trailing text outside the table."""
    s = line.rstrip()
    return bool(re.match(r"^\|\s*\**[A-Z]+[0-9]+", s)) and not s.endswith("|")

# The near-miss half matters most here: the FIELD COUNT check above cannot see this shape when the
# trailing text lands on the header's own column count, because `strip("|")` has no trailing pipe to
# strip and the split comes out the same length. The fixture is exactly that case.
if not _tail_bad("| A1 | item | rmbp | flies | open | ev | closed by 1234abcd, and then prose"):
    _CTL.append("TRAILING TEXT: a row with text after its final pipe was not reported")
if _tail_bad("| A1 | item | rmbp | flies | open | ev | closed by 1234abcd |"):
    _CTL.append("TRAILING TEXT: a well-formed row was reported")
if _tail_bad("prose about A1 that ends without a pipe"):
    _CTL.append("TRAILING TEXT: a PARAGRAPH was judged; prose is never a row")
if _CTL:
    for _c in _CTL:
        say("NO VERDICT — control failed: " + _c)
    say("NO VERDICT — a gate whose control did not fire is not a clean tree")
    sys.exit(2)

# (c) STATE-LINE UNIQUENESS — over the queue files that are in this tree.
state_dupes = 0
for p in QUEUES:
    for ia, ib, txt in _state_dupes(open(p, errors="replace").read()):
        state_dupes += 1
        red.append(f"{p}:{ia} and {p}:{ib}: two queue lines, one a PREFIX of the other — a same-line "
                   f"merge kept BOTH sides and neither was lost, so the merge is clean and the STATE "
                   f"is wrong (1878e035). Keep one: {txt[:60]}…")

# (d) TRAILING TEXT — a ledger row ends at its final pipe or it is not a row.
tail_bad = 0
for p in files:
    for ln, line in enumerate(open(p, errors="replace").read().split("\n"), 1):
        if _tail_bad(line):
            tail_bad += 1
            red.append(f"{p}:{ln}: ledger row does not END with `|` — the text after the final cell "
                       f"delimiter is outside the table and is silently absorbed into the last cell "
                       f"when the count happens to match (A1/B89 at 754f9107): …{line.rstrip()[-48:]}")

# (b) ROW CONTINUITY — ARMED ONLY AT A FOLD, and that is the whole reason it has no false reds.
#
# Comparing every tree against a merge base would judge ordinary editing: queue rows are reworded
# constantly and a reword reads as a deletion at any row granularity. The defect class is narrower
# than that — it is a MERGE silently losing a row — so the check arms exactly there:
#   * a merge IN PROGRESS (`MERGE_HEAD` present) → HEAD and MERGE_HEAD are the two sides. This is
#     the half that makes "run ledger-check BEFORE the commit" worth doing: the fold is judged
#     while it is still a working tree and the repair costs an edit rather than a reset.
#   * HEAD is already a merge → its two parents, judged against the WORKING file, so a repair made
#     after the commit is seen.
#   * anything else → NOT ARMED, and the census says so. Silence about the posture is what SR13
#     cost this repo once already.
# The judged side is always the WORKING file, never HEAD's blob.
rowcont = "not armed (HEAD is not a merge and no merge is in progress)"
row_losses = 0
_mh = _rev("MERGE_HEAD")
if _mh:
    _p1, _p2, _why = _head_sha, _mh, "merge IN PROGRESS (HEAD + MERGE_HEAD)"
else:
    _pp = subprocess.run(["git", "log", "--format=%P", "-1", "HEAD"],
                         capture_output=True, text=True).stdout.split()
    _p1, _p2, _why = (_pp[0], _pp[1], "HEAD is a merge (both parents)") if len(_pp) >= 2 else (None, None, "")
if _p1 and _p2:
    _mbr = subprocess.run(["git", "merge-base", _p1, _p2], capture_output=True, text=True)
    _base = _mbr.stdout.strip() if _mbr.returncode == 0 else None
    # DROPPED IS THE AUTHOR'S ESCAPE, and it is deliberately cheap: a row struck on purpose at a
    # fold is named `DROPPED <id>` in the merge commit message or in the file's own STATE block.
    # It has to be cheap or the gate teaches people to delete rows quietly to keep it green.
    _msg = subprocess.run(["git", "log", "--format=%B", "-1", "HEAD"],
                          capture_output=True, text=True).stdout
    _dropped = set(re.findall(r"DROPPED\s+([A-Za-z0-9_-]+)", _msg))
    for p in sorted(set(QUEUES) | set(files)):
        for _hl, _h, _body in _state_sections(open(p, errors="replace").read()):
            for _i, _l in _body:
                _dropped |= set(re.findall(r"DROPPED\s+([A-Za-z0-9_-]+)", _l))
        _W = _rows_of(open(p, errors="replace").read(), p)
        _A = _rows_of(_show(_p1, p), p)
        _B = _rows_of(_show(_p2, p), p)
        if _A is None or _B is None:
            continue
        _M = _rows_of(_show(_base, p), p) if _base else {}
        for k in _row_losses(_W, _A, _B, _M or {}):
            _first = (k.split() or [""])[0]
            if _first in _dropped or _first.strip("*`") in _dropped or k in _dropped:
                continue
            row_losses += 1
            red.append(f"{p}: row present in BOTH parents and GONE from this tree — "
                       f"{_p1[:8]}:{_A[k]} and {_p2[:8]}:{_B[k]} have {k!r}, the fold does not. "
                       f"Rebuild it from git history, or name it `DROPPED <id>` in the merge message "
                       f"or the STATE block (fb29c268 lost three METAL rows this way)")
    rowcont = f"armed — {_why}, base {(_base or '(none)')[:8]}"

# A REGISTRATION THAT NO LONGER MATCHES ANYTHING IS ITSELF A FINDING — the allowlist has to be
# falsifiable or it becomes the place defects go to be forgotten. Skipped for a file not in this tree.
for _k, _why in ABSENCE_REG.items():
    if _k not in abs_seen and _k.split(":")[0] in files:
        red.append(f"stale absence registration {_k} — the row enumerates now; delete the entry ({_why})")
if absence:
    say(f"ABSENCE — {len(absence)} registered exception(s); NOT findings, and each must reach zero:")
    for a in absence: detail(a)

for _k, _why in FIELDCOUNT_REG.items():
    if _k not in fc_seen and _k.split(":")[0] in files:
        red.append(f"stale field-count registration {_k} — the row parses correctly now; delete the entry ({_why})")

for _k, _why in DEFERRAL_REG.items():
    if _k not in def_seen and _k.split(":")[0] in files:
        red.append(f"stale deferral registration {_k} — that row no longer defers; delete the entry ({_why})")

for p in skipped: say(f"SKIP {p} — not in this tree (arrives at the trunk sync)")
if rows_seen == 0:
    say("NO VERDICT — no ledger rows found in", files or "(no ledger files)"); sys.exit(2)

# ── CENSUS, printed on EVERY run, green or red ─────────────────────────────────────────────────
# SR13's lesson in one line: the posture a gate is running in must be VISIBLE, because the only seat
# who ever saw strict armed was the one seat for whom the trigger was not broken. Same for the two
# populations — a grandfathered count nobody prints is an allowlist nobody audits.
say(STRICT_WHY)
say(f"CENSUS — queue files scanned {len(QUEUES)}/4"
    + (f" (missing here: {', '.join(queue_missing)})" if queue_missing else "")
    + f"; conflict-marker scan over {len(conflict_scanned)} file(s)"
    + f"; queue ledger-id citations {cite_total} ({len(cite_distinct)} distinct)"
    + f", deferred-keepable ids {len(queue_deferred)}, grandfathered ids {len(queue_registered)}"
    + f" (+{queue_reg_idle} registered id(s) not cited in this tree)"
    + f"; cross-ref deferrals {len(deferred)}, grandfathered {len(DEFERRAL_REG)}")
# THE FOLD SHAPES SAY THEIR POSTURE TOO — SR13's lesson applied to the newest checks: a row-
# continuity verdict that is silent about whether it ARMED is indistinguishable from one that ran
# and found nothing, and this one is not armed on most trees by design.
say(f"FOLD SHAPES — STATE-line uniqueness over {len(QUEUES)} queue file(s): {state_dupes} finding(s)"
    f"; trailing text over {len(files)} ledger file(s): {tail_bad} finding(s)"
    f"; row continuity {rowcont}: {row_losses} finding(s)")
if registered:
    say(f"FIELD COUNT — {len(registered)} registered exception(s); NOT findings, and each must reach zero:")
    for r in registered: detail(r)
if queue_registered:
    _byf = {}
    for (fp, cid), (n, ln) in sorted(queue_registered.items()):
        _byf.setdefault(fp, []).append(f"{cid}x{n}")
    say(f"QUEUE CITATIONS — {len(queue_registered)} registered id(s) over {sum(v[0] for v in queue_registered.values())} citation(s); NOT findings, and each must reach zero:")
    for fp, ids in sorted(_byf.items()):
        detail(f"{fp}: {' '.join(ids)}")
        detail(f"{fp}: {QUEUECITE_REG[fp + ':' + ids[0].split('x')[0]]}")
if queue_deferred:
    say(f"QUEUE CITATIONS — {len(queue_deferred)} deferred-keepable id(s); NOT findings, the row is written on another head and arrives at that landing:")
    for (fp, cid), (on, n, ln) in sorted(queue_deferred.items()):
        detail(f"{fp}:{ln}: cites {cid} x{n} — written on {', '.join(on[:3])}")
if deferred:
    say(f"DEFERRED — {len(deferred)} cross-branch cross-ref(s); NOT findings. {STRICT_WHY};"
        f" these become reds automatically when this lands on `{TRUNK}`:")
    for d in deferred: detail(d)
# DEDUPE, for the reason f9255b68 deduped shas: a row citing the same missing id three times printed
# three identical findings, and duplicate findings are how a gate teaches people to skim its output.
# Order-preserving so the first occurrence still reads in file order.
red = list(dict.fromkeys(red))
deferred = list(dict.fromkeys(deferred))
def _notation_note():
    # LAST line of either verdict: the escape is only honest if its use is COUNTED and said out loud.
    if notation:
        print("GATE-LEDGER: NOTATION — %d emitted line(s) carried a token from the harness fault-scan "
              "family or a markdown cell delimiter, and were printed in the declared escaped form "
              "(SR11: \u27e8q:X\u00b7\u2026\u27e9 and \u00a6)" % notation)
if red:
    say(f"RED — {len(red)} finding(s) across {len(files)} file(s), {rows_seen} rows:")
    for r in red: detail(r)
    _notation_note()
    sys.exit(1)
_defnote = f", {len(deferred)} cross-branch ref(s) deferred" if deferred else ""
say(f"OK — {rows_seen} rows in {len(files)} ledger file(s) + RULINGS + {len(QUEUES)} queue file(s): ids unique, field counts match their header, absence claims name an enumeration (lexical sampler — see the header), status ∈ enum, owners known, cross-refs resolve{_defnote}, every deferral names an owner and an expiry, shas exist, evidence in git and anchored, rulings live or superseded-by a real R<n>, no conflict markers, every queue citation resolves, one STATE line per claim, every ledger row ends at its final pipe, no row lost at a fold")
_notation_note()
PY
# GO-RED PROOF (tree mutation, run before shipping; each reverted after):
#   duplicate id           -> RED naming the line       status "standing"      -> RED (outside the enum)
#   `→ S999` in a row      -> RED (dangling), when LEDGER.md is present
#   sha deadbeef1 in a row -> RED (not a commit)         `~/unaos-bench/scratch/x` in a row -> RED
#   owner "peter"          -> RED                        `S99` in a PARAGRAPH  -> GREEN (prose control)
#   evidence/*.log without `size 0x`/`img=[` -> RED      RULINGS R-row status `pending` -> RED
#   RULINGS `superseded` with no R<n> in superseded-by -> RED
#   a literal `|` added inside any cell -> RED naming the row, the counts and the sign (B63)
#   a cell DELETED from a row              -> RED, the negative sign, "a cell is missing"
#   a registered row (B24/C10) unchanged   -> printed as REGISTERED, exit 0, never silent
#   a registration whose row is repaired   -> RED as a stale registration (the allowlist is falsifiable)
#   `→ SO99` in a row      -> DEFERRED, exit 0, PRINTED (SO is branch-local; hw-jetson owns it)
#   the same under UNAOS_LEDGER_STRICT=1 -> RED, exit 1   (the landing's setting)
#   `→ SR2` on hw-rmbp     -> resolves, neither red nor deferred (the control: the check still fires
#                             where the target is local, which is the half a blanket skip would lose)
#
# GO-RED PROOF, SECOND CUT (LEDGERGATES 2026-09-15; every case below executed, then reverted, and
# `git status` clean of the mutations afterwards. The two detached worktrees were throwaways under
# the round's scratch and are removed):
#   SR13  detached worktree AT TRUNK'S SHA      -> `strict=by-ancestry`, rc 0  (pre-fix: printed no
#                                                  posture at all, because `--abbrev-ref HEAD` is
#                                                  the literal `HEAD` there)
#         detached worktree at a TRACK sha      -> `strict=off`, rc 0         (must NOT arm)
#         the same + UNAOS_LEDGER_STRICT=1      -> `strict=by-env`
#         trunk sha  + UNAOS_LEDGER_STRICT=0    -> `strict=off`, suppressed
#         UNAOS_LEDGER_TRUNK=nosuchref          -> `strict=off`, reason names the unresolvable ref
#   SR11  one injected row quoting the verdict idiom in its status and owner cells, same tree:
#         PRE-fix script  -> 2 output lines match arroyo's FAULT_PATTERNS, 1 carries a pipe
#         POST-fix script -> 0 and 0, finding still readable, rc 1 both     (the control is the
#                            pre-fix script: a zero from a scan whose corpus cannot produce a hit
#                            would be a fact about the corpus, not about the fix)
#   SR12  `-> SO99` with no owner and no expiry -> RED, naming BOTH missing halves
#         the same + `owner rmbp, expiry 2026-10-01, blocked on SO99` -> DEFERRED, printed, rc 0
#   QUEUE `<<<<<<< HEAD` / bare `=======` / `>>>>>>> exec-probe`, one per file, one run -> RED x3,
#                            each naming file, line and marker text; reverted -> rc 0
#         `B9999` cited by a new QUEUE.md row   -> RED "a citation to nothing"
#         `B70`   cited by a new QUEUE.md row   -> rc 0 (the must-pass half: a real id resolves)
#         `- **A66** - probe` appended to orin-ledger.md -> RED, stale queue-citation registration
#                            (the allowlist is falsifiable by the id it grandfathers coming true)
#
# GO-RED PROOF, THIRD CUT (QUEUEGATE 2026-09-16; the fold shapes. Every mutation below was executed
# on a SCRATCH COPY of the queue/ledger files, never on the tree's, and each is named in the commit
# body with its exit line):
#   STATE      a duplicated STATE line in rmbp-queue.md (the second a strict prefix-variant of the
#              first, 1878e035's own shape)        -> RED, naming the file and BOTH line numbers
#              the two-section fixture (same line under `## STATE` and `## CLOSE STATE`) -> GREEN
#   ROWCONT    a METAL row present in both parents and deleted from the working file -> RED by name
#              the same row named `DROPPED X86BIND` in the STATE block                 -> GREEN
#              a tree that is not at a fold -> NOT ARMED, printed in the census, never a silent zero
#   TAIL       a ledger row with text after its final pipe, on the header's own column count (so the
#              FIELD COUNT check cannot see it)    -> RED naming the row and the tail
# HISTORY, not synthetic: the three checks were measured against the commits that cost them.
#   `python3` over the four queue files at 1878e035 -> 1 STATE finding; at f8f8ce8c -> 0.
#   The narrow row rule over the last 60 merges on hw-rmbp -> exactly 3 findings, all real
#   (X86BIND + AHCIFLY at fb29c268, AHCIWRITE at a84c8f32), 0 false. The wide rule -> 6, 3 of them
#   rewords; dropped, and the numbers are in _row_losses.
#   `git show 754f9107:docs/dev/OS/rmbp-ledger.md | sed -n '21p;123p'` is A1/B89 with the tail.
