# rmbp 17 — landing report (CLOSED 2026-09-09 on Peter's word; successor baton `rmbp-18.md`)

A SUPPORT round from the first turn and it stayed one: **zero executors, zero metal, zero
kernel code.** Peter at open — *"you are at this very moment opening as a supporter of orin's
focus and will continue to do so"* — the cafe trip that would have pivoted the focus to this
track did not happen, so orin held the focus at the main bench throughout.

The round's product is verification, and it came out as **twelve ledger rows (B98–B109), two
working instruments, and one conversation Peter said we had not been having.**

## State at open — fresh host `ls-remote`, run that turn (2026-09-08 23:12Z)

    main c7407753 · hw-pi4 059e04db · hw-jetson 815086e4 · hw-rmbp 67fd8438 (= local)

Two refs had moved since the baton closed: `hw-jetson` (98213b7f → 815086e4, ancestry
confirmed) and `exec-orin22-bootroot` (26f6ee64 → 600887c2, five commits — the INTEGRATE set
this seat owed second eyes on).

## Pushes named to Peter, batched in the first turn

`git push origin hw-rmbp`, and — after B99 — the MATRIXPAR remedy. **The second push line was
WRONG and orin 23 caught it: it was non-fast-forward and could not run.** Corrected to
`git push origin 5c1f13d0:refs/heads/exec-orin22-matrixpar2`. Peter pushed both, and every
commit was verified landed by `merge-base --is-ancestor` PER COMMIT rather than by comparing
tips. **Still open and his alone: retiring origin's old `exec-orin22-matrixpar`, which holds
the pre-amend orphan carrying the rejected P=6 default.**

## What this round produced

**Two instruments, which is the only part that outlives the session:**

* **GATE-LEDGER field-count assert (B63, owed since rmbp 15).** Four live victims; B16 and B40
  repaired, B24 and orin's C10 REGISTERED — printed every run, and a registration whose row
  later parses correctly goes RED as stale. It convicted its own documentation on the commit
  that built it.
* **GATE-LEDGER absence-form check (B107).** A row asserting an absence must name the command
  that enumerated the population. **It convicted its author on its first run** (B34, claiming
  seven knobs "exist only on `hw-jetson`" — a claim about every other head nobody had checked).
  The wide phrase set was measured and DROPPED at ~75% false positives rather than papered over
  with exceptions; the tight set ships with its own blind spot stated in its header.

**Findings handed to other seats, none taken here:** B98 (the clone-merge still live at orin's
tip, with the fix already in their tree — accepted as the design), B101 (a `[pstrip]` FORBID
that four-field adjacency holds up), B102 (Gate 2 prints, Gate 3 says it passed), B103 (CAPSTONE
COMPLETE prints either way), B106 (the flight scorer cannot tell which build booted → STAMP-MATCH),
B108 (two glass defects off Peter's own eyes), B109 (rung 2 targets a registry slot, not a disk).

**Ledger hygiene:** B88 re-enumerated against current refs before it travelled to Peter as an
argument; B90 ticked with the flown friend-diff.

## The peer exchange — this round's dominant activity

Roughly forty messages with orin 23 and pi 9. The exchange, not the solo work, produced almost
everything above. Three of this seat's errors were caught by pi 9 and none by re-reading:
`DEFAULT_FORBIDS`, `orin-specscore.py`, and a delimiter proven on a population of one.

**`mbench` `--explain` was built here, in this tree, at orin's ruling** (their reason beat mine:
a second author in a second tree is the divergence the tool cannot afford). It landed in
`orin-specscore.py` rather than `mbench.py` on pi's flag that the gate's modes are the semantics
of record. Acceptance test — orin's, verbatim — passes: the three builtins printed beside
`REQUIRE CAPSTONE COMPLETE`, the exact question two seats failed to ask, answered with no boot.

## The rules conversation (Peter: we had not had a fair and open one)

pi 9 counted eight rules in one evening and asked whether that was a good sign. **The count was
wrong and the overcount was the finding:** four of the eight were ONE rule at four scales, and
no seat could tell a re-derivation from a new rule until the total was lined up. orin's dedupe
test is the durable answer — **a rule is new only if no existing rule's ENFORCER would have
fired on the incident.**

What this seat contributed: **ADDRESSABILITY** — a rule fires only if the actor knows they are
at the moment it governs, testable at authoring time by asking "name the moment." pi 9 improved
it: **anchor to the UTTERANCE, not the intention; a moment you cannot feel is still addressable
if it has a string in it.** The boundary survives as a warning — errors of OMISSION largely have
no string, except the one that bit both seats, which does: **"would have"**, a counterfactual
about evidence in hand.

**And the honest counterexample, from this round's own freshest finding: point-of-use warnings
are not the answer either.** `dock_tiles`'s SHELLPIN comment states its exact failure mode AT the
site, and the fix was still applied to one of two identical call sites, because the sibling pin
is in another file (B108). Point-of-use fails the population test too.

## What this round got wrong

**The theme is one class, and this seat is the author of most instances of it: a search scoped
to a FILE, a claim stated about a CAPABILITY.**

1. **"No FORBID rescues the CAPSTONE banner"** — grepped `specs/*.spec`, got zero, read the zero
   as absence. `mbench.parse_spec` APPENDS `DEFAULT_FORBIDS` to every spec. orin 23 independently
   "verified" it and reproduced the same zero from the same place: **two seats reading one
   directory is one check run twice.**
2. **"Nothing answers what a spec enforces without a capture"** — `orin-specscore.py` already
   held the object list.
3. **A delimiter proven on `jetson-jd5.spec`**, the one spec with no quoted pattern; the
   falsifier sat in three others.
4. **"Zero rows in orin's or pi's ledgers"** — measured over a copy of the SHARED ledger that was
   **17 commits stale.** Caught by orin running THIS gate, from THIS commit, over THEIR tree.
   **The first time an instrument built here caught an absence claim of this seat's, in another
   seat's file, run by another seat.**
5. **A push line that could not execute** (non-fast-forward), named to Peter as a deliverable.
6. **"Boot 1 would have passed the scorer"** — while holding output that said DEFECT. Not a scope
   error: evidence in hand, not applied.
7. **An over-claim about the rMBP's load address** — cited a spec comment as showing two boots
   with no friend involved; the comment says nothing about what was plugged in.

**One finding was killed before it was sent** (rung 2 "bypasses FRGUARD" — `write_veto` says the
global is refused in exactly one state, so rung 2 goes to the disk the boot volume actually is),
and it is recorded because the next reader would otherwise inherit it from B90's framing.

## Gates

Docs and gate commits only; no `.rs` touched all round. `unaos/scripts/ledger-check.sh` exit 0 at
every commit (214 rows at the time of writing). The two script changes carry their own controls,
named per commit: `--explain`'s acceptance test plus the corpus-wide loop, the clash guard proven
to fire on a synthetic fixture, six refusal controls; the absence check's go-red plus a
same-phrase-with-enumeration GREEN control.

**Stated rather than performed: NO ARROYO VERB REACHES `orin-specscore.py`** (`grep -c
orin-specscore unaos/arroyo` = 0). Running `./arroyo check` and reporting it green would be a
green from a gate blind to the change. orin has taken a discovery-shaped self-test leg as owed.

## Flagged / owed

* **THE TRUNK SYNC — SCOUTED, then aborted clean on the close (B110), so rmbp 18 inherits a map
  rather than an estimate.** orin measured the box and gave a GO with three conditions; the merge
  ran, **TEN of the eleven `.rs` files auto-merged clean** and **four files conflicted in five
  hunks** (`LEDGER.md`, `screenshot.md`, `RULINGS.md`, `prtscr.rs` ×2). `LEDGER.md` union target
  is **45 ids**. **Aborted rather than handed over conflicted — a conflicted working tree is not a
  handoff.** And the conflict paid for itself: it surfaced that **B98's fix is not a design,
  `prtscr.rs:621` on trunk already reads `global.slot_id == usb.slot_id`** (pi 7's `usb_backed`),
  which is the round's own class landing on its author one last time.
* **THE GATES-ONLY LANDING, with Peter, unanswered.** Re-measured this round: seven gates exist
  only on `hw-rmbp` (re-enumerated per file per ref, current), and `ledger-check.sh` is **+287/−6**
  against a tree that synced tonight. pi 9's framing: *not "rmbp is behind on landing" but the
  fleet running without a gate that already works.* Their synthesis, which is the argument:
  **the fleet's gate strength is a function of which unlanded branch you are standing on, and
  the fully-synced track gets the weakest gate of the three.**
* **B90's positive control is owed and now has a design:** friend present AND root refusing
  writes — the only configuration in which staining vector 1 can fire. Prerequisite: the
  `PRTSCR-VOL` witness naming the source and volume, which orin has an executor cutting.
* **Peter's open decisions, surfaced once each:** the ledger-cell pipe convention (B24 is the row
  three injections broke); the B86 Fable-spawn hook; A4 (card as default startup volume).
