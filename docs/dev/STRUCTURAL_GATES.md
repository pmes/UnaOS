# STRUCTURAL_GATES.md — the `arroyo check` invariants that can go red

`./arroyo check` compiles the kernel on both arches across its cfg legs and the
ring-3 crates, and then asserts a small number of **structural** invariants
about the tree that no compiler checks. This file documents those invariants:
what each one asserts, why it is enforced by a gate rather than by convention,
how it goes red, and how it is legitimately updated. The compile legs themselves
(GATE-CFG, GATE-CFG-MIX, GATE-USER, GATE-CORE, GATE-BLOB) are described in the
comment blocks that carry those names in `unaos/arroyo`; this file covers the
structural gates and the standard they are held to.

**The standard, and where it came from.** Before these gates landed, the tree
had exactly one structural check, the knob→leg coverage assertion in
`check_kernel_cfg`, and three seats produced three different wrong explanations
for why it had never gone red — every one of them reached by reading the script's
text. A two-minute harness settled it in a single run: a copy of `arroyo` with
the `cargo check` line stubbed to `true`, sourced, with `check_kernel_cfg` called
and the coverage variable probed, named the legs that were swallowing every
feature. A gate's ability to fail is therefore established by **executing a tree
mutation that must turn it red**, never by an argument about the check's own
structure. Every entry below records that mutation and the observed result, and a
gate without one is not yet a gate.

**The control probe.** A scan that matches nothing reports zero findings, and
zero findings reads as a clean tree. Each gate must make that zero
distinguishable from a broken pattern: it carries a feature or symbol that
certainly exists and refuses to give a verdict when the probe is not seen. A
control failure is a broken gate, not a clean tree, and it is reported as such.

**Where they run.** They run inside `check_both` in `unaos/arroyo`, after the
compile legs; `test` and `test-arm` do not run them. Each has its own failure
line in `check_both` so that a red is attributed to the gate that produced it.

---

## GATE-FAMILY — a per-platform copy cannot be added silently

**Invariant.** The set of platform-split symbol families in the kernel — a base
name plus its `x86_`/`orin_`/`pi_`/`tegra_`/`aarch64_` affixed twins — is exactly
the set recorded in `unaos/arch-families.ledger`.

**Why a gate.** Every per-platform twin in the tree was defensible when it was
written, and the result is one job with N implementations sharing roughly half
of their callees. The lane rule does not cause this: crossing a lane already
works, by grant. What was missing is a price on *not* sharing. A cross-lane edit
costs a negotiation, a recorded grant and a review; a copy costs nothing and
appears in no measurement, so the cheapest correct move was the duplicating one.
The gate is that price, and it is charged at the moment a name is chosen — the
only point at which the fix is still a rename rather than an extraction.

**Mechanism.** `unaos/scripts/arch-families.sh` scans every `fn` name under
`crates/kernel/src`, strips exactly one platform affix (never two — stripping
both would fold `orin_ladder_arm` onto `ladder`), groups names by base, and
diffs the families of size two or more against the ledger. The affix set is
deliberately narrow and `arm` is absent from it on purpose: it collides with the
English verb, and an earlier draft reported a false family on exactly that
collision. A gate with false positives teaches readers to skip the region a real
one appears in.

**Control.** The ledger is the baseline: it records eight families today, so a
scan that silently found no functions would produce a diff of removals, not a
green. There is no separately named probe symbol; the non-empty ledger serves.

**Goes red when** a family grows or appears (a `+` line in the diff), or one
shrinks without the ledger being updated. **GO-RED proof, recorded in
`1ae2489d`:** injecting `fn orin_render_service` takes the `render_service`
family from 2 to 3 and the gate exits 1 naming the new member; reverting the
injection returns it to green.

**Legitimate update.** Growing a family is allowed; it cannot be silent. Run
`unaos/scripts/arch-families.sh --update` and commit the ledger **in the same
commit** as the new symbol, with a commit message that answers the three
questions the failure text asks: what the N implementations share and why it is
not extracted, which axis genuinely differs, and whether a parameter on the
existing member would have served.

---

## GATE-KNOB — a cfg on an undeclared feature is dead code that reads as live

**Invariant.** Every `feature = "X"` named by a cfg in the kernel is declared in
`crates/kernel/Cargo.toml` `[features]`, and every declared feature is named by
at least one cfg (`default` excepted — it is Cargo's own and correct while
empty).

**Why a gate.** A cfg on an undeclared feature is always false. It does not fail
the build, and it did not fail `check`: rustc emits `unexpected cfg condition
value` and the check discarded the warning. The code under such a cfg is dead
while reading as live, on every board, with the `not` arm taken unconditionally.
The first instance was found by the Pi seat (`pidesk`, seven sites in `main.rs`
and `video/menubar.rs` on `hw-pi4`, arch-neutral files that reach x86 at the
next merge); this tree was clean when the gate landed, which is why it could land
green and go red at precisely that merge. The reverse case, a declared feature
named by no cfg, is a knob wired to nothing and is reported as `DEAD`.

**Mechanism.** `unaos/scripts/knob-hygiene.sh` parses `[features]` on one side
and the cfg sites on the other and takes both set differences. **Comments are
stripped before the source scan, and that is the whole difficulty.** The naive
form — set-difference the feature names out of the raw sources — was proposed as
incapable of false positives. Run unmodified on this tree it reds on `pidesk`,
on the strength of a doc comment in `video/menubar.rs` that only quotes the cfg
expression in prose. A gate that reds on a sentence is a gate people turn off,
so prose mentioning a feature is a fixture that must stay green.

**Control.** `wc` and `witness` must be parsed out of `[features]` and found in
at least one cfg. If either side fails, the script exits 2 with no verdict
rather than reporting zero phantoms.

**Trailing-comment phantom (added 2026-09-05, orin 13's finding, LEDGER P7).** A
`#[cfg(...)]` appended after a line's trailing `//` comment is prose: it compiles
nothing and `check` stays green (PRTSCR-ORIN shipped that way for two hours; a
union merge did it again). The script reds any code line whose `#[cfg(` sits after
its first `//`. A line that IS a comment (`//` or `///` first) stays green — that
is the prose fixture, and it is why the check is "code before the comment", not
"cfg after a slash".

**Goes red when** a cfg names an undeclared feature (`PHANTOM`, with its sites
listed), a declared feature is named by no cfg (`DEAD`), or a `#[cfg(` follows a
trailing comment on a code line (`TRAILING`). **GO-RED proof,
recorded in `88fd5175`, four states:** a phantom cfg injected → red naming the
site; prose quoting a feature name → green (the false-positive fixture); a
declared-but-unused feature → red; the clean tree → green.

**Legitimate update.** Declare or delete. For a phantom, either declare the
feature in `[features]` or delete the cfg and keep the arm that was actually
compiling. For a dead knob, add a cfg site or remove the declaration. There is
no allowlist beyond `default`.

---

## GATE-K8REACH — a knob with no `K8_FEATS` arm is unreachable in every Pi image

**Invariant.** Every `UNAOS_*` knob in `arroyo`'s general `_feats` map is
accounted for in the Pi bare-metal image: it has an arm inside `kernel8()`'s
curated `K8_FEATS`, or it has a row in `unaos/scripts/k8-reach.registry` recording
that it deliberately has none.

**Why a gate.** `kernel8()` builds from a CURATED list that deliberately does not
draw from `_feats` — 115 knobs there, and only 11 of them armed here. An omitted
arm is not an error and not a warning: the operator sets `UNAOS_X=1`, flashes,
and the image is byte-identical to the one without it. LEDGER SR1 records two
instances a week apart, by different seats, each of which cost days before anyone
suspected the knob rather than the code: `UNAOS_PRTSCRST` (pi 7 — the Print
Screen gate greened about nothing for a week and blocked S14) and `UNAOS_BOOTLOG`
(orin 15 — a `UNAOS_PIDESK=1` image with no way back to the serial mirror). This
is **not** the same invariant as KNOBLEG below. That one asks whether every
aarch64-qualified feature is COMPILED by some check leg; a knob can be fully
leg-covered and still absent from the image an operator boots. Build coverage is
not operator reachability.

**What it deliberately does not assert.** It does not decide which knobs BELONG
in the Pi image. That judgment is not mechanical, and the tree says so: one of
`nvidia-kepler`'s eleven cfg sites is in arch-neutral `video/wm.rs`, and
`rastmc`'s single Pi-live site is a call whose callee is x86-gated. pi 7's
objection is the right one — by inspection a Pi-meaningful knob that was never
given an arm is indistinguishable from one deliberately omitted. So the gate
asserts the weaker thing that IS mechanical and that both instances would have
failed: the decision is RECORDED. A knob added tomorrow reds until someone rules
on it, and the ruling is a registry row rather than an unwritten intention.

**Mechanism.** `unaos/scripts/k8-reach.py` parses the `_feats` map on one side and
the knobs named inside `kernel8()`'s bounds on the other, and takes the set
differences against the registry. The red conditions are pure set membership, so
the gate itself cannot produce a false positive out of a misjudged site.

**The evidence mode, and why the site classification is not in the verdict.**
`k8-reach.py --evidence <KNOB>` prints every cfg site behind a knob, classified
`PI-LIVE` / `X86` / `PROSE`, so that ruling on a row is a command rather than a
squint. That classification needs three things a grep does not have, and it is
kept OUT of the pass/fail path because each of them is a judgment that can be
wrong: `unaos/scripts/k8-modtree.py` resolves the `target_arch` context the module
tree imposes on each FILE (`drivers/gpu/mod.rs` is declared under
`target_arch = "x86_64"` in `drivers/mod.rs`, so all eleven Kepler sites inside it
are x86-only although neither the line nor the path says so — a cfg'd-out
`pub mod` is never lexed); prose is stripped, because `rtwit.rs:61` names its own
feature in a `//!` line and GATE-KNOB already paid for the lesson that a gate
which reds on a sentence gets turned off; and the arch that governs a site is the
one on the nearest enclosing bracket group, not "does `x86_64` appear on this
line" — `any(all(feature = "tegra", target_arch = "aarch64"), all(feature =
"rastmc", target_arch = "x86_64"))` is aarch64-live for one feature and x86-only
for the other, on one line.

`k8-modtree.py` PRINTS every file it cannot account for (`UNREACHED`,
`UNRESOLVED`, `INLINE`) instead of defaulting it to a context. On this tree that
surfaced `events.rs`, which no `mod` declaration in the crate names — a file that
was not compiled by anything, and whose `push_event`/`pop_event` shadowed the live
`pal.rs` pair a grep would land on beside it. Deleted (rmbp-ledger B29).

**Control.** Checked before any verdict: the `kernel8()` bounds must resolve, the
`_feats` parse must find ≥ 50 knobs INCLUDING both SR1 instances, and the arm
parse must find ≥ 20. A parse that silently found no knobs would report a clean
tree, which is the failure the gate exists to prevent. The canaries are checked
for PRESENCE, not for being armed, so that de-arming one is caught as a red rather
than swallowed as "no verdict" — a control must not blind the gate to the very
instances that created the class.

**Goes red when** a knob has no arm and no registry row (`UNREGISTERED`), a
registry row names a knob that is no longer in `_feats` (`STALE`), or a knob is
both armed and registered as unarmed (`CONTRADICTION`). **GO-RED proof, recorded
in this commit — eight states, all executed:** a new `UNAOS_NEWTHING` line added
to `_feats` → red naming it; **`UNAOS_PRTSCRST`'s arm deleted from `kernel8()`,
i.e. the historical instance replayed → red**; a registry row for a knob that
does not exist → red; a row for an armed knob → red; the clean tree → green; the
`_feats` map renamed so the parse finds nothing → exit 2, NO VERDICT; `kernel8()`
renamed → exit 2; the registry deleted → exit 2. The wiring was proven the same
way rather than by reading it: with one registry row removed, `./arroyo check`
itself exited 1 with `check FAILED — knob hygiene or k8 reachability`.

**Legitimate update.** Arm it or register it. A knob that should reach a Pi image
gets its arm in `kernel8()` beside the `UNAOS_LOGTS` one; a knob that should not
gets a registry row with the reason. The 104 rows seeded on 2026-09-06 are marked
`TODO`, which satisfies the gate and is counted on every run — they are the
backlog SR1's class left behind, not a ruling, and each is converted to `NA <reason>`
(or to an arm) by whoever owns the knob. **An `NA` cites the `--evidence` command that
justified it, not a reason string alone**: a human pass inherits the classifier's blind
spots exactly — the arch-neutral file and the x86-gated callee are what a reader also
mis-sorts — so an `NA` without its command is a second classifier with no proof (pi 7,
2026-09-06). Deleting a row without adding the arm reintroduces the silence.

---

## GATE-LEDGER — the issue ledgers are a tracker, and every row is checkable

**Invariant.** In `docs/dev/LEDGER.md` and every `docs/dev/OS/*-ledger.md`, each
row of a table that has a `status` column has a unique id (`^[A-Z]+[0-9]+`), a
status that begins with one of `open` · `fixed-unflown` · `flown` · `landed` ·
`dropped`, an owner in {orin, pi, rmbp, shared-gate} where the table has an
`owner` column, cross-references (`→ S<n>`) that resolve in `LEDGER.md` — with
seat-prefixed refs (`SR`/`SO`/`SP`) DEFERRED rather than red when they name a row that
is still on another branch, printed and counted every run, and turned back into reds by
`UNAOS_LEDGER_STRICT=1` and, with no variable set at all, by being on the trunk
branch: the trunk enforces, track branches defer. A landing merges to trunk and
runs the battery there, so it gets strictness without anyone remembering to ask
for it — shas
that exist in the repository (and, for a fixed/flown/landed row, are ancestors of
some track head — a fix nobody can fetch is not fixed), and evidence that lives
in git: a `unaos-bench/scratch` path is red, a `docs/…` path must exist.

**Why a gate.** Peter's rule (2026-09-05, `docs/dev/RULINGS.md` R6): one ledger
per arch, one over-arching ledger, and the arc that fixes, flies, or drops an
item ticks it in the same commit. A rule like that rots exactly when sessions are
busiest: `PCIE-RP-RECOVERY.md` said "no reboot facility of any kind" for a day
after FADTRESET landed, and on the day the ledgers were created their two files
already used twelve spellings for about five states, mirrored one item under two
ids with two statuses that disagreed within the hour, and cited eight evidence
files that existed on one machine only. A table in a doc is still prose until
something reads it.

**Mechanism.** `unaos/scripts/ledger-check.sh` parses every markdown table with
a `status` header in the ledger files present in the tree (a missing
`LEDGER.md` is skipped with a line, since it reaches a track only at its trunk
sync) and applies the invariant row by row. **Prose is never judged**: an id, a
sha or a scratch path in a paragraph is not a row. Free text is allowed after the
status word (`open — blocked on Peter's call`). Facts that are not defects have
no state and belong in a list or the subsystem doc, not in a status table.

**Evidence excerpts and rulings (added the same day, pi 6's two objections).**
A serial capture is append-only across many boots — `pi.log` holds nine — so
committing captures is out and citing a line range into an unversioned bench file
is a citation into nothing (`~/unaos-bench` is not a repository). The convention
is the EXCERPT: `docs/dev/evidence/<arc>/<id>-<boot>.log`, tens of KB, immutable,
and every excerpt must carry its boot anchor — the loader's `size 0x…` line on
aarch64, the `WXN-x86 … img=[…` span on x86 — because a range without the anchor
rots the moment the capture grows (orin 12 nearly scored a boot-11 fault as
tonight's that way). The gate reds an anchorless excerpt. `docs/dev/RULINGS.md`
is checked too: every R-row has `status` ∈ live · superseded · retracted, and a
superseded row names a real R-id in `superseded-by` — rulings get reversed (the
cube, EVAC) and an append-only quote file would let a reader find only the dead one.

**Control.** Zero ledger rows found in the files present → exit 2, no verdict.

**Goes red when** an id repeats, a status begins with anything outside the enum,
an owner is unknown, a `→ S<n>` dangles, a sha is not a commit (or a
fixed/flown/landed sha is unreachable from every head), or a row cites evidence
outside git. **GO-RED proof, by tree mutation on the day it shipped, twelve
states:** duplicate id → red naming the line; `standing` as a status → red;
owner `peter` → red; sha `deadbee1` → red; a `~/unaos-bench/scratch` path → red;
a missing `docs/` path → red; `-> S999` with a `LEDGER.md` present → red (**written with an ASCII arrow ON PURPOSE — do not "fix" it to `→`. The resolver matches the UNICODE arrow only, so `→ S999` here would make this sentence, which documents the gate's own test, a failing INPUT to the gate. Unicode arrow = a live cross-REFERENCE the gate must resolve; ASCII arrow = a MENTION. Latent today only because this file is not in the scanned set — `LEDGER.md` + `OS/*-ledger.md` + `RULINGS.md` — and live the moment that widens**); `S99`,
a sha and a scratch path in a PARAGRAPH → green (the prose control); `flown`
with a reachable sha → green; an `evidence/*.log` without `size 0x`/`img=[` → red;
a RULINGS row with status `pending` → red; a `superseded` ruling naming no R-id → red.

**Legitimate update.** Fix the row: pick the enum word, move the evidence into
`docs/dev/evidence/<arc>/`, name the sha that exists, resolve or drop the
cross-reference. There is no allowlist. Agreed rmbp 11 ↔ orin 13, 2026-09-05;
the LAWS §Ledgers paragraph cites this gate only now that it exists.

---

## KNOBLEG — the knob→leg coverage check can now fail

**Invariant.** Every aarch64-qualified kernel feature — one with a cfg site under
`arch/aarch64/` or conjoined with `target_arch = "aarch64"` — is compiled by at
least one board leg of `KERNEL_CFG_MATRIX`, where "compiled by" is the
transitive closure of the leg's feature list over `[features]`; known holes are
allowlisted with a named owner.

**Why a gate, and why it was not one.** The check had printed green on every
run since it was written and its red branch was unreachable, for two reasons.
First, it measured coverage over the union of `KERNEL_CFG_MATRIX`,
`KERNEL_CFG_MIX` and `KERNEL_CFG_SWEEP`, and the `x86-mix-N` legs are manufactured
at runtime by `build_cfg_legs` from feature unions that include aarch64 features,
so every feature was covered by construction. This is why reading could not find
it: the swallowing value is computed, not written, and no aarch64 feature name
appears in `arroyo`'s text. Second, restricting to the board matrix is necessary
and not sufficient, because a literal substring match cannot see Cargo
implications: `aarch64_el0` is named by no leg, yet eleven board legs name
`tegra_el0`, which implies it. The one-line fix alone would red a feature that
is genuinely compiled — a false positive on the one gate whose entire value is
being believed.

**Mechanism.** `check_kernel_cfg` feeds the board legs to
`unaos/scripts/knob-leg-covered.py`, which computes the transitive closure of
each leg's features over `[features]` (dropping `dep:` and cross-crate
`crate/feature` entries, which are not kernel features) and emits the compiled
set. The check then classifies each declared feature by whether any of its cfg
sites is aarch64-qualified, using a same-line-conjunction rule that is
deliberately binding-agnostic — this tree does line-neutral appends, so
positional attribute binding misattributes gates.

**Control.** `vugpar` is named by the `arm-pi` leg. If the compiled set does not
contain it, the leg parser has dropped rows and the check fails itself with no
coverage verdict.

**Goes red when** an aarch64-qualified feature is reached by no board leg and is
not on the allowlist. **GO-RED proof, recorded in `647f485a` — the first time
this check was able to red:** a feature `zzz_armprobe` declared, given a cfg
site under `arch/aarch64/`, and named by no leg → red naming the feature;
reverted → green; the clean tree → green. Coverage after the fix is 142 of 152
features from the 28 board legs; the five uncovered, unallowlisted features are
x86-side and not aarch64-qualified, so this check does not judge them and they
are recorded in the commit rather than absorbed.

**Legitimate update.** Add the feature to the `arm-*` leg that owns its sites,
or — for a hole another track must claim — add it to the allowlist in
`check_kernel_cfg` with its owner named. Removing an allowlist entry without
adding the leg reintroduces a silent hole.

---

**Landed but not yet sectioned here:** GATE-ROOTS (`scripts/check-roots.sh`, every
binary target is a named root of `check`) and GATE-APPEND (`scripts/append-position.sh`,
LEDGER P7's trailing-comment trap) are both wired into `check_both` and green; their
invariant, control and GO-RED live in their `arroyo` comment blocks and in the
scripts' headers until a seat gives each a section on the standard above.
