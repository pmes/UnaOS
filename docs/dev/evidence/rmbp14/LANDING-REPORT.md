# rmbp 14 — LANDING REPORT (2026-09-06; orin holds the focus; SUPPORT seat, no executors, no metal)

Written before the close rather than at it: sessions die mid-thought, and "I'll write it at close" is
a bet there will be a close. Updated in place while the seat runs on.

**Posture, stated once.** orin holds the focus, inherited from rmbp 13's close and not re-asked
(memory `focus-is-inherited-never-reasked`). Peter's opening instruction this session was
*"remember u are support for orin do not starts jobs"*. That names a scope: **zero executors, zero
arcs, zero spawns** (R22), and everything else — grants, verification, gates, docs — runs at full
pace inside it (R23). No `Agent` call was made this session.

## State at pickup — fresh host check, run in the opening turn

    main f49ea1e7 · hw-jetson c24d9517 · hw-pi4 f25f1601 · hw-rmbp c3a384ff (origin) = c3a384ff (local)

`flatpak-spawn --host git ls-remote --heads origin` (in-sandbox git dies on publickey).
**Pushes Peter owed at open: NONE** — the baton's "SEVEN COMMITS OWED" was written at rmbp 13's close
and Peter pushed them before this session opened; `git log origin/hw-rmbp..hw-rmbp` is empty and the
baton's `d8871c96` is an ancestor of the current tip. Peers moved since the baton was written
(hw-jetson `906e3aef` → `c24d9517`, hw-pi4 `2136ccab` → `f25f1601`), which is the reason the law asks
for the check instead of the relay.

## J3 — SR1's open CLASS, closed at the entrance: GATE-K8REACH

The baton names J3 as the next gate and squarely this lane. It ships as `unaos/scripts/k8-reach.py`
+ `k8-modtree.py` + `k8-reach.registry`, wired into `check_both`, documented in
`docs/dev/STRUCTURAL_GATES.md` §GATE-K8REACH, ticked as rmbp-ledger **B28** and in LEDGER **SR1**.

**What the gate asserts, and the thing it refuses to assert.** SR1's question — "does every
Pi-eligible knob have a `K8_FEATS` arm" — has a mechanical half and a judgment half, and the seat
tried the judgment half first. A classifier was built that resolves each cfg site's arch context
through the MODULE TREE rather than its path (`drivers/gpu/mod.rs` is declared under
`target_arch = "x86_64"` in `drivers/mod.rs`, so all eleven `nvidia-kepler` sites inside it are
x86-only although neither the line nor the path says so). It cut the candidate set from 67 to 50, and
then stopped being able to improve, because the residue is genuinely undecidable by machine: one of
`nvidia-kepler`'s sites is in arch-neutral `video/wm.rs`, and `rastmc`'s single Pi-live site is a call
whose callee is x86-gated. **pi 7's objection is correct and no amount of static analysis retires it.**

So the verdict path asserts the weaker mechanical thing that BOTH known instances would have failed:
every knob is ACCOUNTED FOR — armed in `kernel8()`, or written down in the registry as deliberately
unarmed. The site classifier is kept out of the pass/fail path entirely and shipped as
`--evidence <KNOB>`, so a ruling is a command rather than a squint, and a wrong classification can
never silence a knob.

**The class, measured** (SR1 recorded it as UNMEASURED): 115 knobs in `arroyo`'s general `_feats`
map, **11** with a `K8_FEATS` arm, **104** without — of which **65** have at least one Pi-live cfg
site and **39** have none. The 104 are seeded `TODO`, counted on every run, and are the backlog the
class leaves behind; converting one is a Pi/Orin knowledge call, not this seat's.

**GO-RED, eight states, all executed** (the round's carried insight: the first question is not "does
it pass" but "can it fail, and did I watch it"):

| state | result |
|---|---|
| clean tree | green, exit 0 |
| a new `UNAOS_NEWTHING` line in `_feats`, no arm, no row | **red** `UNREGISTERED` |
| **`UNAOS_PRTSCRST`'s arm deleted from `kernel8()` — the historical instance replayed** | **red** `UNREGISTERED` |
| registry row for a knob no longer in `_feats` | **red** `STALE` |
| registry row for a knob that IS armed | **red** `CONTRADICTION` |
| `_feats` map renamed so the parse finds nothing | exit 2, NO VERDICT |
| `kernel8()` renamed so the bounds do not resolve | exit 2, NO VERDICT |
| registry deleted | exit 2, NO VERDICT |

**The control was rewritten once, and the reason is worth carrying.** The first version required both
SR1 canaries to parse as ARMED. That is a control that blinds the gate to its own founding instances:
de-arming `UNAOS_PRTSCRST` would have produced "no verdict" instead of a red. It now checks the
canaries for PRESENCE in the `_feats` map (the parse whose failure mode is vacuous green) and the arm
parse by COUNT (≥ 20). Replaying the historical instance then reds, as the table records.

**The wiring was proven by execution, not by reading it** (memory `verification-comes-from-execution`):
with one registry row removed, `./arroyo check` itself exited **1** with
`❌ check FAILED — knob hygiene or k8 reachability`. A gate that is green in isolation and unreachable
from `check` is the same defect class this file keeps recording.

**Side finding, from the module-tree resolver refusing to default what it cannot reach:**
`crates/kernel/src/events.rs` is named by no `mod` declaration anywhere in the crate. It is compiled
by nothing. Recorded in B28; not touched — deleting a file is not this arc.

**Two parser defects the resolver had first, both silent when wrong**, and both found by making it
print what it could not account for rather than by re-reading it: several `mod` decls folded onto ONE
line each with its own `#[cfg]` (this repo's own byte-identity idiom, LEDGER P7 —
`arch/aarch64/mod.rs:110` carries three, and a line-anchored regex sees only the first), and a
non-`mod.rs` parent's children living in `<stem>/` (`video/quarry.rs` → `video/quarry/live.rs`).

## Gates — committed as `146c2e84` on `hw-rmbp`, conditioned on these exit statuses

    ./arroyo check                  green, exit 0, both arches — with GATE-K8REACH inside it
    UNAOS_WC=1 ./arroyo test 150    green, exit 0, `wc` in the ⚡ kernel features banner
    ./arroyo test-arm               green, exit 0
    ./arroyo check (mutated tree)   exit 1 — the wiring proof above
    scripts/ledger-check.sh         green — 123 rows, 1 cross-branch ref deferred (B22 → SO6)

`check` was re-run after the last edit to the gate script (`sys.dont_write_bytecode`, so the evidence
mode does not leave `__pycache__` in a checked-out tree) rather than inheriting the earlier green.

## The pi 7 exchange — a peer reading at this seat's sha, which is the mechanism that catches errors

pi 7 answered the registry hand-off with four items. Three changed the tree.

**`events.rs` is worse than "compiled by nothing", and it is deleted (B29).** pi 7 pointed out what
this seat had recorded and then walked past: the file defines `pub fn push_event` / `pub fn pop_event`,
and `pal.rs` defines the LIVE pair called from the serial drain and the HID path. A seat grepping
either name gets two hits and the dead one is readable, editable and plausible, with every gate green
over a patch to it. Verified here before acting rather than accepted on report — 0 `mod events`
declarations, 0 `events::` references, no `include!`/`#[path]`, and the same three zeroes on
`origin/main`, `origin/hw-jetson`, `origin/hw-pi4`. Precedent for the shape: Peter on `video/vug.rs`.

**An `NA` row now cites the command that justified it.** pi 7 turned this seat's own SR1 argument on
the human pass: a reader marking the 39 "no Pi-live site" rows inherits the classifier's blind spots
exactly, because the arch-neutral file and the x86-gated callee are precisely the cases a person also
mis-sorts. The registry header and §GATE-K8REACH now require the `--evidence` output behind an `NA`.

**The denominator is stated instead of quoted.** pi 7 measured 118 arms / 113 distinct knobs on
hw-pi4 `f25f1601` against this seat's 115. Both are right: this tree has 119 append lines carrying 115
distinct names (and all 119 precede `kernel8()`, which is what makes the parse boundary sound). B28
now says the number is DISTINCT NAMES, records the cross-tree pair, and says it is derived per tree
rather than quoted across them.

## Peers

pi 7 told the same turn: the registry's 104 `TODO` rows are theirs to rule on, `--evidence` makes each
one a command, and the 39 with no Pi-live site are the cheap end.

**orin 16 archived mid-write; orin 17 is live and has the heads-up.** The `arroyo` message bounced
("Session is archived"), and this seat surfaced it to Peter as something needing his call. **That flag
was wrong and cost him a round-trip.** orin 16 closed on Peter's own order and handed to orin 17 — a
routine seat rotation, not an anomaly, and the bounce was only an address going stale between the
write and the send. pi 7 caught it and named the successor; this seat verified `orin 17`
(`local_12b0248e…`, `isArchived: false`, running) with its own `list_sessions` call before sending
rather than relaying pi 7's, and delivered the heads-up there.

**The rule that held and the one that did not.** Not unarchiving was right — `isArchived` is the flag
and only Peter reopens a seat (memory `only-peter-closes-a-seat`). Escalating was not: "I cannot
reach the focus seat" is a fact about an address, and the cheap check — is there a successor session? —
belonged before the flag, not after it. A bounce is a routing failure until a lookup says otherwise.

## Flagged, not taken

* `STRUCTURAL_GATES.md` documented four gates while six are wired. GATE-APPEND and GATE-ROOTS have no
  section on the standard the file sets. Their invariant/control/GO-RED live in their `arroyo` comment
  blocks and script headers; the trailer now says so instead of claiming GATE-ROOTS is "in flight".
  Writing those two sections is a real job and belongs to a seat that scopes it, not to this arc.
* The 104 `TODO` registry rows are a backlog with owners other than this seat. The 39 with no Pi-live
  cfg site are the cheap end.

## The support round, after J3 — nine patch reviews, and what they cost each side

Everything below happened after GATE-K8REACH landed, in an exchange with orin 17 that ran the length of
their round. The seat wrote no feature code; the product was **verification and grants**, and it is
recorded here as a list of what was CHECKED rather than what was said.

**Reviewed by applying, never by reading.** CRYSTALFIX, A30FIX, PRTSCLOST, SO9FIX, SHELLBASICS,
PINCONSOLE, the WINID ceiling raise. Every one: stage the base with `git archive`, `git apply`, then run
this seat's own GATE-APPEND on the result — and afterwards on the FOLDED tip, because a fold of green
commits is a new configuration. Two folds were verified by FILE IDENTITY against the trees reviewed
(all five files byte-identical), which is the only check that catches a patch changing on its way in.

**The landing ack was computed, not reviewed.** `git merge-tree --write-tree f49ea1e7 c5048fe6` produced
`c5048fe6`'s exact tree oid at exit 0, and `main`'s three extra commits turned out to be merges OF
hw-jetson, so the trunk held no original content the merge could drop — which was the only thing that
made "byte-identical to hw-jetson" a dangerous claim. `ledger-check` was run on the merged tree under
`UNAOS_LEDGER_STRICT=1`, answering on the actual tip a strictness question this seat had raised against
a preview.

**Two coverage findings, both of the SR1 shape — a gate that greens about nothing.**
* The **x86 gate does not exercise the xHCI keyboard path at all**: `launch_x86_64()` attaches
  `usb-kbd,bus=xhci.0`, but the headless test leg wires HID to EHCI (this round's own log: "usb-kbd
  rides the ehci bus", every HID self-test `EHCI-HID`, no `[hidkeys]`). So a restructure of the
  interrupt-IN TD ring could break x86 keyboard input entirely with every automated leg green. That
  fact, not an opinion about risk, is why the N-TDs fix folds nowhere until the rMBP flies it (B45).
* PINCONSOLE's x86 half ships unexercised for the same kind of reason — the x86 harness never mints a
  console window — which orin stated rather than hid, and which now goes in the commit body.

**Four stale-or-overreaching comments, which is a class and not four accidents** (B33, B35, B39, B47).
`emmc2.rs`'s "no writes" header — repeated INTO a ledger row by this seat before orin caught it;
`winmenu.rs:1274`'s pre-CRYSTALFIX literals; `wcg.rs:4008`'s "eight slots"; and the worst,
`shell.rs:4305`, which asserts *"the review checked all 78 spellings arm by arm"* for a review that had
only ever checked one direction — while two verbs with match arms and no table entry answered "Unknown
command" for their entire existence. The others were out of date; that one claims a verification that
did not happen. **The same error then came for this seat's own writing**: the first draft of the
`--release` clause in `arroyo` said "this leg and the three below it", and the grep says fifteen. It
now carries the command that would refute it.

**The gate corrected its author four times, and each is in the row rather than quietly fixed.** B35 and
B46 claimed `fixed-unflown` over shas on no track head — an accepted patch is not a fixed row. B40
cited scratch paths as evidence, twice; the fix is to cite the COMMAND that regenerates evidence, never
the path. And one commit went in over a red because the gate was chained with `;` rather than `&&`,
which is the baton's "condition on the EXIT STATUS, not on having run it", exactly.

**Two lane rules came out of it, both adopted by both seats.**
1. **The seat whose branch carries the TEXT fixes the text, whoever owns the file.** Twice this round a
   doc fix was assigned to this lane for a line that does not exist on `hw-rmbp` (`winmenu.rs:1274`,
   `x86_64/syscall.rs:6738`). Lane ownership makes the FILE ours; it does not make a line editable that
   is not in the tree.
2. **An ask describes the DIFF — numstat and every call site — not the intent.** PRTSCLOST was asked as
   "counters only" and carried a per-drain TSC stamp; PINCONSOLE was asked as "one statement in
   `x86_render_service`" and carried three call sites, a doc block and a wrapper pair, with "the rest
   stands without it" false because the dock's press LATCHES. Both were granted; the rule is that a lane
   owner reads the diff, so the ask has to survive that reading.

**A divergence found early rather than in a landing window:** `drivers/xhci/mod.rs` differs between
`hw-rmbp` and `hw-jetson` by 12 insertions and 77 deletions, and orin's hunk-alone patch does not apply
here. The J1 reconcile on that file is content-delta work, not a union (B43).

## Close — 2026-09-06

**State at close, fresh host `ls-remote` that turn:** `main c7407753` · `hw-jetson 98ffd63d` ·
`hw-pi4 059e04db` · `hw-rmbp 141cc728`, local = origin, **nothing owed to Peter's push**.

**What this seat produced, all of it inside support scope (zero executors, zero spawns, R22):**
31 ledger rows (B28–B58), one gate landed with its documentation, 13 grants, 14 GATE-APPEND runs on
peers' tips — none of which needed a push, each about ten seconds — and one landing ack computed rather
than reviewed.

**Nothing landed to trunk from this lane.** J1 is 120 commits and still needs an adversarial panel,
which is a fleet, which belongs to the focus seat. `main` moved anyway: orin's 93 commits, acked here.

**Owed into rmbp 15, in the order the baton names them:** the SHELLRELICS and VFSROUTE reviews (both
granted patch-first, both folds blocked on a review this seat performs); the rMBP metal flight that
XHCINTD is blocked on and that nothing else in the fleet can score; and the `arroyo` sweep this seat
owes — eleven stale board-term restatements plus replacing a hand-maintained enumeration with a derived
one.

**The two lane rules that came out of the round, adopted by both seats:** the seat whose branch carries
the TEXT fixes the text, whoever owns the file; and an ask describes the DIFF, not the intent.

**The closing correction is the one worth reading first.** This seat stated the second rule to orin
twice and then granted SHELLRELICS on a description — "15 comment-only line-neutral files" — with
`git show --numstat` one command away. It is 46 files, +1274/−712, with 1,579 changed lines across two
code files. Having a rule and enforcing it on a peer is not the same as applying it to your own work,
and the gap is invisible from inside. That is why the round's product was verification rather than
opinion, and why every grant in it was measured here before it was given.
