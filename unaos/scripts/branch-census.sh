#!/usr/bin/env bash
# branch-census.sh — GATE-BRANCH: an executor branch is CLOSED by a line in the registry, not by a
# seat remembering to fold it. Called by ledger-check.sh; runs standalone for the same verdict.
#
# WHY THIS IS A SIBLING OF ledger-check.sh AND NOT A SECTION INSIDE IT. GATE-LEDGER's population is
# the markdown ROWS in the tree's ledger and queue files; this gate's population is the REFS in the
# object store, which no tree contains. Different corpus, different failure, and this one has to be
# runnable on its own in under a second (the census below is 548 refs) without paying for the ledger
# parse. ledger-check.sh calls it and folds its exit code in, so `arroyo check` still runs one gate.
#
# WHAT IT IS FOR (BRANCHCENSUS, docs/dev/evidence/branch-census-0917/VERDICTS.md §Findings, 935d099a).
# 206 executor refs were an ancestor of NO track branch. 177 rows of judgement later: 123 folded, 19
# superseded, 7 docs-superseded, 5 WIP survivors — and 23 rows LOST, 19 of them SHARED code or shared
# docs. Fourteen of the 23 are ONE DAY: a seat cut nine executors, hit its credit limit between their
# reports and the fold, and wrote a complete handoff naming every branch — on `hw-jetson`'s copy of
# `orin-queue.md`, where trunk cannot see it. FONTS2X was found by Peter looking at blocky glass.
# The census's own conclusion: the seat's proposed close-out RULE would have made 21 of 23 visible
# and prevented none, because (1) where the close-out is written is not part of a rule, and (2) one
# branch NAME is not one TIP — MATRIXPAR is `ce0d992b` on origin and `5c1f13d0` locally, two versions
# of one arroyo change, neither folded. So this walks refs PER REF, never per name, and the record it
# reads lives in git at a path every track shares.
#
# CONTRACT.
#   * POPULATION: every `refs/heads/exec-*` and every `refs/remotes/origin/exec-*`, enumerated by one
#     `git for-each-ref`. REACHED = the ref's tip is an ancestor of (or equal to) one of the eight
#     track refs {main, hw-rmbp, hw-jetson, hw-pi4} × {local, origin}. A reached ref is closed by git
#     itself and needs NO registry line.
#   * REGISTRY: docs/dev/exec-branches.txt, one line per ref, five fields:
#         <ref> <tip-sha> <state> <fold-sha-or-owner> <date>
#     `<ref>` is the FULL refname. state ∈ IN-FLIGHT · FOLDED-BY-HAND · SUPERSEDED · WIP-SURVIVOR ·
#     LOST, and may be written with its argument appended (`FOLDED-BY-HAND:<sha>`, `LOST:<queue>`) —
#     in that spelling field 4 is `-` or the same argument. Field 4 is the argument: the fold sha for
#     FOLDED-BY-HAND/SUPERSEDED, the owning queue path for LOST, the seat for IN-FLIGHT, `-` for a
#     WIP survivor. Comments start `#`. Blank lines ignored.
#   * AN UNREACHED TIP WITH NO LINE IS RED. That is the whole gate; everything below is the registry
#     keeping itself honest.
#   * STALE, AND IT CUTS BOTH WAYS. A CLOSED line (folded/superseded/wip/lost) records the tip it
#     JUDGED: if the ref has moved, the verdict is about a sha that is no longer there — RED, saying
#     so. An IN-FLIGHT line records where the executor was CUT, and an executor's branch is supposed
#     to move, so the test is forward-only: the recorded sha must still be an ancestor of the tip. A
#     rewind out from under an IN-FLIGHT line is RED. And a LOST line whose ref is NOW REACHED is a
#     lie in the other direction — RED, asking for FOLDED-BY-HAND:<sha>. The allowlist is falsifiable
#     in both directions, the standard GATE-LEDGER's registered exceptions already set.
#   * A FOLD SHA MUST BE FETCHABLE. FOLDED-BY-HAND/SUPERSEDED's argument must resolve to a commit in
#     this repo AND be an ancestor of some track ref — "folded onto a branch nobody can fetch" is the
#     defect, not the cure (GATE-LEDGER says the same of a `flown` row's shas). Rows the census
#     closed on an adjudication RECORD rather than one fold commit carry the commit that CARRIES that
#     record — 935d099a (VERDICTS.md) or 3efde934 (orin22/DISPOSITION.md) — and the registry says so
#     per line, because a path is not checkable from a tree that has not synced trunk and a sha is.
#   * AN EXECUTOR DOES NOT RUN TWO WEEKS. An IN-FLIGHT line whose date is more than 14 days old is
#     RED: either it finished and nobody wrote the fold sha, or it died.
#   * LOST IS OWED, NOT RED — BY DEFAULT, AND IT IS PRINTED EVERY RUN. A LOST line is a JUDGED,
#     OWNED debt: it names the queue that owes the rescue, and it prints on every single run with its
#     ref, tip and subject. The thing this gate exists to stop is the SILENT kind — a tip nobody has
#     judged — and that is red out of the box. Redding the 39 already-judged ones by default would
#     paint every seat's `arroyo check` red for work already written down, which is how a gate teaches
#     people to skim it. `UNAOS_BRANCH_STRICT=1` arms them (the posture is printed either way), and
#     when the queues have worked the debt to zero, arming strict by ancestry the way GATE-LEDGER
#     does — trunk enforces, tracks defer — is the next cut.
#   * NOT ARMED WITHOUT ITS REGISTRY. A tree with no docs/dev/exec-branches.txt prints one line
#     saying it is not armed and exits 0 — the same shape as GATE-LEDGER skipping a ledger file the
#     track has not synced yet. It is never a silent zero.
#
# ⛔ WHAT THIS GATE IS NOT. It does not judge whether a fold is CORRECT or COMPLETE — patch-id
# equality, hunk reading and the probes that produced those 177 verdicts are a census's work, not a
# gate's. It answers exactly one question per ref: is this tip on a track, and if not, has somebody
# written down what happened to it. A green here means "no executor tip is unaccounted for", never
# "nothing was lost" — 39 refs are accounted for AS LOST right now, and the census line says so.
#
# usage: branch-census.sh [repo-root]        exit 0 green · 1 red · 2 no verdict (control probe failed)
set -uo pipefail
ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$ROOT" || exit 2
python3 - "$ROOT" <<'PY'
import os, re, subprocess, sys, time, datetime
root = sys.argv[1]
t0 = time.time()

REGISTRY  = "docs/dev/exec-branches.txt"
PATTERNS  = ["refs/heads/exec-*", "refs/remotes/origin/exec-*"]
TRACKREFS = ["refs/heads/main", "refs/heads/hw-rmbp", "refs/heads/hw-jetson", "refs/heads/hw-pi4",
             "refs/remotes/origin/main", "refs/remotes/origin/hw-rmbp",
             "refs/remotes/origin/hw-jetson", "refs/remotes/origin/hw-pi4"]
QUEUES    = ["docs/dev/QUEUE.md", "docs/dev/OS/rmbp-queue.md",
             "docs/dev/OS/orin-queue.md", "docs/dev/OS/pi-queue.md"]
STATES    = ("IN-FLIGHT", "FOLDED-BY-HAND", "SUPERSEDED", "WIP-SURVIVOR", "LOST")
INFLIGHT_DAYS = 14
STRICT = os.environ.get("UNAOS_BRANCH_STRICT", "") == "1"

# THE ESCAPE (SR11), the same declared form GATE-LEDGER uses, and deliberately restated here rather
# than shared: this gate quotes COMMIT SUBJECTS, which is the richest source of the harness
# fault-scan family in the whole repo — a branch called `exec-orin-panicloc` prints `panicked at` in
# a subject and a ledger-check log fed to `scan_serial_faults` would be mis-scored. Eight lines
# duplicated beats a bash heredoc importing a python module from its sibling; if one changes, both do.
FAULT_TOKENS = re.compile(r"-> FAIL|FAIL ::|FAIL — |PANIC|panicked at |EXCEPTION:")
notation = 0
def _q(s):
    global notation
    out = FAULT_TOKENS.sub(lambda m: "⟨q:" + m.group(0)[0] + "·" + m.group(0)[1:] + "⟩", s)
    out = out.replace("|", "¦")
    if out != s:
        notation += 1
    return out
def emit(s): print(_q(s))
def say(*a): emit("GATE-BRANCH: " + " ".join(str(x) for x in a))
def detail(s): emit("    " + s)

def git(*a):
    return subprocess.run(["git", "-C", root] + list(a), capture_output=True, text=True)

# ---------------------------------------------------------------- the census, one for-each-ref
o = git("for-each-ref", "--format=%(refname)\t%(objectname)\t%(objectname:short)\t%(subject)", *PATTERNS)
if o.returncode != 0:
    say("NO VERDICT — `git for-each-ref` failed:", o.stderr.strip()[:200])
    sys.exit(2)
refs = {}
for line in o.stdout.splitlines():
    if not line.strip():
        continue
    f = line.split("\t", 3)
    refs[f[0]] = (f[1], f[2], f[3] if len(f) > 3 else "")

t = git("for-each-ref", "--format=%(refname)\t%(objectname)", *TRACKREFS)
tracks = {}
for line in t.stdout.splitlines():
    n, s = line.split("\t")
    tracks[n] = s
if not tracks:
    # A tree with no track ref at all cannot answer "is this tip on a track". That is a NO VERDICT,
    # never a green: a gate whose corpus is empty must not report the emptiness as a pass.
    say("NO VERDICT — none of the eight track refs resolve in this repo; reachability is unanswerable")
    sys.exit(2)
reached = set()
for sha in sorted(set(tracks.values())):
    r = git("for-each-ref", "--merged", sha, "--format=%(refname)", *PATTERNS)
    reached.update(x for x in r.stdout.split() if x)

# ONE `git rev-list` FOR EVERY FOLD SHA, and it is the difference between a gate people run and a
# gate people skip. The first cut asked `cat-file -e` + up to four `merge-base --is-ancestor` per
# registered fold sha: 159 distinct shas, ~600 subprocesses, 9.4 s wall. The history reachable from
# the four tracks is 5,322 commits and costs 0.07 s to list ONCE — after which "is this sha on a
# track" is a prefix test in memory, because a short sha is a prefix of the full one. Measured on
# this box: 9.4 s -> 0.5 s for the same verdict on the same registry.
_TRACKSET = None
_ontrack = {}
def ontrack(sha):
    """resolves to a commit AND is an ancestor of some track ref."""
    global _TRACKSET
    if sha in _ontrack:
        return _ontrack[sha]
    if _TRACKSET is None:
        _TRACKSET = set(git("rev-list", *sorted(set(tracks.values()))).stdout.split())
    v = bool(re.fullmatch(r"[0-9a-f]{7,40}", sha)) and any(f.startswith(sha) for f in _TRACKSET)
    _ontrack[sha] = v
    return v

def ancestor(a, b):
    return git("merge-base", "--is-ancestor", a, b).returncode == 0

# ---------------------------------------------------------------- the registry
regpath = os.path.join(root, REGISTRY)
if not os.path.exists(regpath):
    say("NOT ARMED — %s is absent from this tree (it reaches a track at its trunk sync); "
        "%d exec ref(s), %d unreached, 0 judged" % (REGISTRY, len(refs), len(refs) - len(reached & set(refs))))
    sys.exit(0)

red, owed, notes = [], [], []
entries, dup = {}, []
for n, raw in enumerate(open(regpath, encoding="utf-8").read().splitlines(), 1):
    line = raw.split("#", 1)[0].strip() if not raw.lstrip().startswith("#") else ""
    if not line:
        continue
    f = line.split()
    if len(f) != 5:
        red.append("%s:%d: a registry line has %d field(s), not 5 — "
                   "`<ref> <tip-sha> <state> <fold-sha-or-owner> <date>`: %s" % (REGISTRY, n, len(f), line[:70]))
        continue
    ref, tip, state, arg, date = f
    if ":" in state:                      # the `FOLDED-BY-HAND:<sha>` spelling
        state, inline = state.split(":", 1)
        if arg == "-":
            arg = inline
        elif inline != arg:
            red.append("%s:%d: the state names `%s` and field 4 names `%s` — one ref, one argument"
                       % (REGISTRY, n, inline, arg))
            continue
    if state not in STATES:
        red.append("%s:%d: state `%s` is outside the enum %s" % (REGISTRY, n, state, " · ".join(STATES)))
        continue
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", date):
        red.append("%s:%d: date `%s` is not YYYY-MM-DD" % (REGISTRY, n, date))
        continue
    if ref in entries:
        dup.append("%s:%d: %s is registered twice (first at line %d) — one line per ref"
                   % (REGISTRY, n, ref, entries[ref][0]))
        continue
    entries[ref] = (n, tip, state, arg, date)
red.extend(dup)

def clears(what):
    return "clears by: " + what

TODAY = datetime.date.today()

def judge(ref, tip, subject, is_reached, ent):
    """The whole verdict for one ref, as a pure function of its census row and its registry line.
       Returns (kind, message) with kind in ok | red | owed. The control probe below calls THIS."""
    if ent is None:
        if is_reached:
            return ("ok", "")
        return ("red", "UNREGISTERED — on no track and in no registry line. " +
                clears("a line in %s: `%s %s <state> <arg> %s`" % (REGISTRY, ref, tip[:8], TODAY)))
    n, rtip, state, arg, date = ent
    if state == "LOST" and is_reached:
        return ("red", "the line says LOST but the tip IS on a track now — a rescue landed and the "
                       "registry still says it did not. " + clears("FOLDED-BY-HAND:<the fold sha>"))
    if is_reached:
        return ("ok", "")
    # tip agreement
    if state == "IN-FLIGHT":
        if not (rtip == tip[:len(rtip)] or ancestor(rtip, tip)):
            return ("red", "STALE — the branch moved OFF its registered cut `%s` (tip `%s`); an "
                           "in-flight line may only move FORWARD. " % (rtip, tip[:8]) +
                    clears("re-cut the line at the new base, or close it with a fold sha"))
        try:
            age = (TODAY - datetime.date.fromisoformat(date)).days
        except ValueError:
            age = 0
        if age > INFLIGHT_DAYS:
            return ("red", "IN-FLIGHT for %d days (limit %d) — an executor does not run two weeks; "
                           "it either folded or died. " % (age, INFLIGHT_DAYS) +
                    clears("FOLDED-BY-HAND:<sha> · SUPERSEDED:<sha> · LOST:<queue>"))
        return ("ok", "")
    if not (rtip == tip[:len(rtip)] or tip == rtip[:len(tip)]):
        return ("red", "STALE — the line judged `%s`, the ref's tip is `%s`; the verdict is about a "
                       "sha that is no longer there. " % (rtip, tip[:8]) +
                clears("re-judge the new tip and rewrite the line"))
    if state in ("FOLDED-BY-HAND", "SUPERSEDED"):
        if not ontrack(arg):
            return ("red", "%s names `%s`, which is not a commit on any track ref — a fold nobody "
                           "can fetch is not a fold. " % (state, arg) +
                    clears("the real fold sha, or LOST:<queue>"))
        return ("ok", "")
    if state == "WIP-SURVIVOR":
        return ("ok", "")
    if state == "LOST":
        if arg not in QUEUES:
            return ("red", "LOST names `%s`, which is not one of the four queue files — a loss with "
                           "no queue has no owner. " % arg +
                    clears(" · ".join(QUEUES)))
        return ("owed", "LOST, owed by %s" % arg)
    return ("ok", "")

# ---------------------------------------------------------------- CONTROL PROBE, before the verdict
# The gate proves it can FAIL before it is allowed to say green — the standard GATE-FAMILY and
# GATE-LEDGER set. Both probes run the real `judge` over fabricated census rows, so a refactor that
# neuters the verdict (an early `return ok`, a swallowed exception) exits 2 and never prints a pass.
try:
    _probe = [
        (judge("refs/heads/exec-CONTROL-PROBE", "0" * 40, "control", False, None)[0], "red"),
        (judge("refs/heads/exec-CONTROL-PROBE", "0" * 40, "control", True, None)[0], "ok"),
    ]
except Exception as e:                   # a throw inside the verdict is a NO VERDICT, never a red:
    _probe = [("threw: %s" % e, "red")]   # exit 1 would report a crash as a finding about the refs
if [a for a, b in _probe] != [b for a, b in _probe]:
    say("NO VERDICT — the control probe did not judge its own fixtures: got %s, expected %s"
        % ([a for a, b in _probe], [b for a, b in _probe]))
    sys.exit(2)

# ---------------------------------------------------------------- the walk
registered = reached_registered = 0
for ref in sorted(refs):
    tip, short, subject = refs[ref]
    ent = entries.get(ref)
    is_reached = ref in reached
    if ent is not None:
        registered += 1
        if is_reached:
            reached_registered += 1
    kind, msg = judge(ref, tip, subject, is_reached, ent)
    if kind == "ok":
        continue
    line = "%s  %s  %s — %s" % (ref, short, subject[:60], msg)
    (red if kind == "red" else owed).append(line)

for ref, (n, rtip, state, arg, date) in sorted(entries.items()):
    if ref not in refs:
        notes.append("%s:%d: %s no longer exists (%s %s, %s) — the line is the only record left"
                     % (REGISTRY, n, ref, state, arg, date))

if STRICT:
    red.extend(owed)
    owed = []

wall = time.time() - t0
unreached = len(refs) - len(reached & set(refs))
say("refs=%d reached=%d unreached=%d registered=%d (of them reached=%d) owed=%d red=%d "
    "strict=%s wall=%.1fs" % (len(refs), len(reached & set(refs)), unreached, registered,
                              reached_registered, len(owed), len(red),
                              "on" if STRICT else "off (UNAOS_BRANCH_STRICT=1 arms LOST)", wall))
for n in notes:
    detail(n)
if owed:
    say("OWED — %d ref(s) registered LOST; NOT findings, each names the queue that owes the rescue:" % len(owed))
    for l in owed:
        detail(l)

def _notation_note():
    if notation:
        print("GATE-BRANCH: NOTATION — %d emitted line(s) carried a token from the harness fault-scan "
              "family or a markdown cell delimiter, and were printed in the declared escaped form "
              "(SR11: ⟨q:X·…⟩ and ¦)" % notation)

if red:
    say("RED — %d finding(s):" % len(red))
    for l in red:
        detail(l)
    _notation_note()
    sys.exit(1)
say("OK — every exec tip is on a track or named by a registry line; no stale line, no fold sha off "
    "the tracks, no executor in flight past %d days" % INFLIGHT_DAYS)
_notation_note()
PY
# GO-RED PROOF (rmbp, 2026-09-22, BRANCHGATE). Every case below was EXECUTED against the seeded
# registry on this box, each mutation reverted from a scratch copy afterwards and the file compared
# byte-for-byte (`cmp` IDENTICAL) before the next. The registry's own 206 seeded lines are the corpus;
# the scratch copy is ~/unaos-bench/scratch/rmbp-0915/branchgate-logs/, nothing under /tmp.
#   THE SEEDING, and it is the measurement that chose the default posture:
#     seeded registry, strict OFF  -> rc 0, census `owed=41 red=0`    (41 refs = the census's 23 LOST
#                                     ROWS; a row that names a local and an origin ref is TWO REFS,
#                                     which is the MATRIXPAR finding restated by the gate itself)
#     the same, UNAOS_BRANCH_STRICT=1 -> rc 1, `red=41`
#     then `exec-orin-fonts2x` (+origin) LOST -> FOLDED-BY-HAND 582278cd, the REAL carry onto
#     hw-rmbp by QUARRYCLICK -> rc 1, `red=39`, and FONTS2X names itself in no finding. Two lines
#     edited, two reds cleared, nothing else moved. Strict OFF on the shipped file -> rc 0, owed=39.
#   THE RED CLASSES, one mutation each, all at strict OFF so the red is the gate's own default:
#     A  a line deleted (exec-orin-blitstall, local)   -> RED UNREGISTERED, naming the line to write
#     B  a closed line's tip sha rewritten              -> RED STALE, "a sha that is no longer there"
#     C  an IN-FLIGHT line dated 2026-09-01             -> RED, "IN-FLIGHT for 21 days (limit 14)"
#     D  an IN-FLIGHT cut sha that is not an ancestor   -> RED STALE, "moved OFF its registered cut"
#     E  FOLDED-BY-HAND ad62cf09 (a real commit, on no track) -> RED, "a fold nobody can fetch"
#     F  LOST docs/dev/OS/NOTAQUEUE.md                  -> RED, "a loss with no queue has no owner"
#     G  a LOST line added for a REACHED ref            -> RED, "the tip IS on a track now"
#     H  a four-field line                              -> RED, naming the five-field shape
#     I  the same ref registered twice                  -> RED, naming both line numbers
#     J  state `relayed`                                -> RED, outside the enum (+ its ref unregistered)
#     K  date `soon`                                    -> RED, not YYYY-MM-DD
#   THE MUST-PASS HALVES, without which the reds above prove nothing:
#     N  the shipped registry, unmutated                -> rc 0, red=0, 548 refs walked
#     L  the registry moved out of the tree             -> `NOT ARMED`, rc 0, and it SAYS so
#     M  the verdict neutered on a scratch copy of THIS
#        script (`if ent is None and False:`)           -> rc 2 NO VERDICT, never a green and never a
#                                                          red: the control probe catches the throw
#     an IN-FLIGHT line 21 days old on a REACHED ref    -> rc 0 BY DESIGN. A branch parked at the
#        track tip holds no commits of its own, so there is nothing to lose; the age rule is about
#        work sitting unfolded, not about a stale name.
#   SR11 ESCAPE, measured rather than assumed: over all 548 current subjects, 2 carry the fault-scan
#     family (`PANICARM`, `PANICLOC`) and 0 carry a pipe — and BOTH sit past the 60-char subject cut,
#     so the emitted corpus produces 0 hits today. A zero from a corpus that cannot produce a hit is
#     a fact about the corpus, so the escape was fired through the shipped script on a path that can:
#     a registry line with state `PANIC` -> `state ⟨q:P·ANIC⟩ is outside the enum` + the NOTATION line.
#   PERFORMANCE, on this box, 548 refs / 215 registry lines: 0.5 s wall, printed in the census line
#     every run. The first cut was 9.4 s and the whole difference is the one `git rev-list` above.
