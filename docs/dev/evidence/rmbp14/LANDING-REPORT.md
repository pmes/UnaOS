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

**orin 16 — the focus seat — is ARCHIVED.** The `arroyo` shared-file heads-up could not be delivered
(`send_message` refused: "Session is archived"). Not unarchived by this seat: `isArchived` is the flag
and Peter is the authority (memory `only-peter-closes-a-seat`). The FOCUS itself is unchanged — a
closed session is not a reassignment — so this seat stays support. Surfaced to Peter, not acted on.
The hunk it would have warned about: one contiguous block in `check_both` after the `knob-hygiene.sh`
call, plus one word on the `_nrc` failure line.

## Flagged, not taken

* `STRUCTURAL_GATES.md` documented four gates while six are wired. GATE-APPEND and GATE-ROOTS have no
  section on the standard the file sets. Their invariant/control/GO-RED live in their `arroyo` comment
  blocks and script headers; the trailer now says so instead of claiming GATE-ROOTS is "in flight".
  Writing those two sections is a real job and belongs to a seat that scopes it, not to this arc.
* The 104 `TODO` registry rows are a backlog with owners other than this seat. The 39 with no Pi-live
  cfg site are the cheap end.
