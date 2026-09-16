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

**Where they run.** Most run inside `check_both` in `unaos/arroyo`, after the
compile legs; `test` and `test-arm` do not run those. Each has its own failure
line in `check_both` so that a red is attributed to the gate that produced it.
GATE-TESTTRUNC and GATE-KNOBOFF below are the two exceptions, and their sections
say so: both are held to the same standard — invariant, control, recorded go-red,
legitimate update — but one asserts a property of a QEMU RUN and the other a
property of a pair of BUILDS, so they live on the x86 `test` legs and in the
`knoboff` verb respectively, and `check` cannot see either.

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
registry row names a knob that is not in `_feats` (`STALE`), or a knob is
both armed and registered as unarmed (`CONTRADICTION`).

**STALE is DEFERRED on a track branch and red on the trunk**, for an ordering
constraint rather than out of leniency: the registry lives on `hw-rmbp` while knobs
are added on every branch, so a correct, evidenced row can name a knob that has not
merged yet (orin 17's seven, 2026-09-06). Strict would red this branch for being
early; waiting would deliver the gate's answer after the commit that needed it. A
deferred row is LISTED on every run and becomes a red automatically on the trunk,
where every branch's `_feats` is present and a row nothing matches really is dead —
the same mechanism and the same trunk trigger as GATE-LEDGER above, with
`UNAOS_K8REACH_STRICT=1`/`=0` and `UNAOS_K8REACH_TRUNK` as its knobs. **GO-RED proof, recorded
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

### SECOND CUT — 2026-09-15, LEDGERGATES (LEDGER SR13, SR11, SR12 + LAWS §3 Queues)

Three defects **of the gate itself**, and the queue files it was owed. Each fix
carries the mutation that proves it can fail; all mutations were executed and
reverted, and `git status` was clean afterwards.

**1. The strict trigger is decided from CONTENT, not from a ref name (SR13).**
The trigger armed on `_branch == TRUNK` where `_branch` is `git rev-parse
--abbrev-ref HEAD` — which returns the literal string `HEAD` in *any* detached
checkout, **including one sitting at trunk's own sha**. That is the exact shape
every executor and every peer-gating seat works in: orin 25 gated a landing four
times from a detached worktree, got the deferring posture every time, and only
saw the strict verdict after exporting the variable by hand — the remembered
step the trigger exists to replace. Strict now arms when HEAD's sha is contained
in the trunk ref (`git merge-base --is-ancestor HEAD main`, then `origin/main`),
or by `UNAOS_LEDGER_STRICT=1`, and **every run prints the posture**:
`strict=by-env` · `by-ancestry` · `off`, each with its reason. The DIRECTION is
the whole safety argument and it is written out in the script:
head-contained-in-trunk arms; trunk-contained-in-head — a post-fold track tip,
SR13's own `a51a0396` counter-example — does **not**, so the false-red the
"zero rows of that prefix" discriminator was turned down for cannot return. It
is wider than equality by exactly one case, a checkout of an older trunk commit,
which was trunk content and wants strict. An unresolvable `UNAOS_LEDGER_TRUNK`
is now *printed* as the reason strict is off, so the rename door is loud rather
than silent. **Go-red, two throwaway detached worktrees:** at trunk's sha
`strict=by-ancestry` rc=0 (the pre-fix script printed no posture at all); at a
track sha `strict=off` rc=0; `UNAOS_LEDGER_STRICT=1` from that same track
worktree `strict=by-env`; `UNAOS_LEDGER_STRICT=0` at trunk's sha suppresses;
`UNAOS_LEDGER_TRUNK=nosuchref` prints the unresolvable-trunk reason.

**2. One general escape for quoted material, at the output boundary (SR11).**
SR11 names three instances of this gate's notation colliding with its subject and
predicts a fourth; two already had an input-side special case, and the row's own
verdict is that the fix is *one general escape*, not a third. It is placed where
it cannot be forgotten — **every line the gate emits** passes through it. Two
collisions, both measured rather than reasoned: (i) the harness fault-scan family
(`arroyo`'s `FAULT_PATTERNS`: `-> FAIL`, `FAIL ::`, `FAIL —`, `PANIC`, `panicked
at`, `EXCEPTION:`), which the gate could emit by *quoting a ledger cell*, so a
ledger-check log concatenated into a run log or pasted into a row is scored as a
kernel fault; (ii) the markdown cell delimiter — the gate printed the status enum
pipe-separated, so its own findings could not be pasted into a ledger cell
without shifting that row's columns, which is B63 pointing the other way. The
escape is declared and visible (`⟨q:P·ANIC⟩`, `¦`), never silent mangling, and
its use is COUNTED and reported on the last line of either verdict. The
diagnostics also now separate the enum and the owner set with `·` and name the
delimiter **by name** rather than by glyph. **Go-red / control, one injected row
(`| S999 | … | peter PANIC | — | standing -> FAIL :: PANIC | … |`), same tree:**
pre-fix script → 2 output lines matching `FAULT_PATTERNS` and 1 carrying a pipe;
post-fix script → **0 and 0**, with the finding still readable and still red
(rc=1 both). What this does **not** fix: the authoring side. A raw pipe inside a
cell is still refused (B63 stands, B24 still registered).

**3. A deferral must be keepable (SR12).** A cross-ref could defer forever with
nobody owing it and nothing expiring it — SO6 deferred on every run, on every
seat, for four days, and the row existed on no ref and had never been written. A
deferring row must now name an OWNER (a track: `rmbp` · `orin` · `pi` · `trunk`;
the `owner` column counts) and an EXPIRY (a date, a commit sha this repo
resolves, or a blocking id written `blocked on <ID>` / `until <ID>` /
`expiry=…`); a deferral missing either is RED **naming the missing half**.
Grandfathering is the `FIELDCOUNT_REG` mechanism — printed every run, must reach
zero, stale entries red — and `DEFERRAL_REG` is **empty**, which is a
measurement: this tree carries zero deferrals in either posture. **Go-red:** a
row carrying `→ SO99` with no owner and no expiry → rc=1 naming both halves; the
same ref with `owner rmbp, expiry 2026-10-01, blocked on SO99` → DEFERRED,
printed, rc=0. The peer-resolution half SR12 also proposed is implemented, but
for the queue citations below, where it splits waiting from never.

**4. The four queue files (LAWS §3 Queues, R45).** LAWS said "warning only until
`ledger-check.sh` learns the queue files"; it now names this gate. Two checks,
deliberately only two — a queue is an ORDER, not a tracker, so it has no status
enum, no owner column and no field count to assert, and importing the ledger
contract wholesale would red honest rows in three seats' files.

* **(a) No conflict marker**, in the queues **and** the ledgers **and**
  RULINGS.md (9 files in this tree; the count is printed in the census). The gate
  passed rc=0 on a `LEDGER.md` carrying three markers a fold had committed
  (hw-jetson `4465eb20`, fixed `7006857f`) because markers sit outside any table
  row. **Go-red:** one marker shape per file in one run (`<<<<<<< HEAD` in
  `QUEUE.md`, a bare `=======` in `LEDGER.md`, `>>>>>>> exec-probe` in
  `orin-ledger.md`) → rc=1, each named with file, line and marker text; reverted
  → rc=0.
* **(b) Every ledger id a queue row cites exists.** The queue's own header says
  "Every row cites its ledger id"; nothing checked it. **The pattern was measured
  before it was chosen, and the measurement removed a prefix:** the proposed
  `(S|SO|SP|SR|A|B|E)[0-9]+` matches 592 tokens / 186 distinct over the four
  files, and `E` matched **only** `error[E0080]` — rustc's diagnostic code, cited
  twice as go-red evidence — while the three real `E` ids are cited by no queue
  row at all. Two false findings, zero true ones, so `E` was dropped; the
  surviving population is **590 citations, 185 distinct**. The id set is the
  union of every ledger file in the tree (table rows *and* P-bullets), because
  `A` is orin's arch prefix and `B` is rmbp's and the trunk queue cites both.
  **Three verdicts, which is SR12's split applied where it is affordable:**
  resolves here → OK; resolves on one of the nine enumerated heads →
  DEFERRED-KEEPABLE, printed with the ref that carries it (5 of these on trunk
  today — the jetson rows `QUEUE.md` §5 itself labels "NOT YET LANDED"; reding
  those would red the trunk for saying something true); resolves **nowhere** →
  RED. **Go-red:** `· B9999 …` appended to `QUEUE.md` → rc=1, "a citation to
  nothing"; `· B70 …` (a real row) → rc=0.

**The grandfather list, and what arming this found.** `docs/dev/OS/orin-queue.md`
cites **35 ledger ids that exist in no ledger file on any head** — measured over
all nine refs, orin's own `hw-jetson` included, so these are rows that were never
written, not cross-branch artefacts; 202 of that file's 373 citations point at
them. That is SO6's shape at scale. They are REGISTERED in `QUEUECITE_REG`
rather than red, for the reason this gate's own ABSENCE note already records
paying once: a gate that reds another seat's file on the day it ships is worse
than the defect. The registration is **falsifiable in both directions that
matter** — it is printed every run as a collapsed census line (35 ids with their
occurrence counts, not 202 lines: LAWS §5's "22 names on every run trains the eye
to skip the region"), it must reach zero, and it goes **RED** the moment one of
the ids starts resolving, locally or on a peer head. **Go-red:** appending
`- **A66** — go-red probe row` to `orin-ledger.md` → rc=1, "stale queue-citation
registration … resolves in this tree's ledgers now".
**IDLE IS NOT STALE, and the first cut got this wrong:** an unmatched
registration was red-lined the way `FIELDCOUNT_REG` does, and a detached
worktree at trunk's own sha then went RED 33 times — because `main`'s copy of
`orin-queue.md` simply predates those citations. Queue files differ by branch, so
"matched nothing here" is a fact about which commit is checked out. Idle entries
are counted in the census instead.

**Census.** Every run, green or red, prints the strict posture with its reason,
then one CENSUS line: queue files scanned, files in the conflict-marker scan,
queue citations and distinct ids, deferred-keepable and grandfathered counts,
cross-ref deferrals and their grandfathered count. SR13's lesson in one line —
the only seat who ever saw strict armed was the one seat for whom the trigger
was not broken.

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

## GATE-TESTTRUNC — a run that stopped early cannot be a pass

**Where this one runs, because it is the exception to the header above.** Not in
`check_both`: it asserts a property of a QEMU RUN, so it lives on the x86 `test`
legs (`test_x86_64` in `unaos/arroyo`, and therefore `test`, `test-fat` and
`test-selfhost`, which all enter through it). `./arroyo check` cannot see it and
is not asked to.

**Invariant.** `./arroyo test` exits 0 only if `target/serial.log` contains the
`COMPLETE` marker declared in `unaos/scripts/specs/x86-test.spec` — the last
fixture of the x86 boot ladder. A capture without it is `-> TRUNCATED`, rc=1.

**Why a gate.** The verdict on this path was `scan_serial_faults`, and it is
NEGATIVE-ONLY: it reports the absence of fault text. A log that stops before the
fault would have been printed satisfies that perfectly, so a boot that ran out of
wall was indistinguishable from a boot that went well. The orin session measured
the consequence twice on one tree (`docs/dev/QUEUE.md` §5, the 2026-09-13 RASTWIN
row): idle, the rast demo took 3,553 ms, the boot reached the failing fixture and
`./arroyo test` exited **1**; under load the demo took 6,662 ms, the boot never
got there, and the SAME command on the SAME tree exited **0**. The polarity is
what makes it a gate rather than a nicety — a loaded box is exactly what a
session running several executors produces, so the harness went quiet about reds
at the moment reds became most likely. No amount of negative evidence fixes that;
only a positive claim that the run reached its end does.

**Mechanism.** `x86_test_completion` (arroyo) runs `scripts/qemu_await.py
--settled` over the FINISHED capture and reads back one `SETTLED status=` value.
`--settled` is the existing completion waiter's other mode: same `mbench.Matcher`,
same `COMPLETE` directives, asked of a capture that is already over instead of one
still being written. It exists because `test` does not own its QEMU — the wall is
`builder/src/main.rs`'s `thread::sleep` — so there is nothing for a tail to
shorten, while the question a tail answers is exactly the one the verdict lacked.
The verdict is three-valued on that status: `complete` hands the log to the
unchanged fault scan; `truncated`, `nosignal` and a broken checker are each rc=1
with their own reason on stdout. The fault scan is checked FIRST, matching
`mbench.run_verdict`'s recorded precedence (a fault is positive evidence and a
short log never excuses it), so one capture cannot be called TRUNCATED here and
FAIL by the replay.

**Control.** The spec's zero is distinguishable from a rotted pattern because the
same spec over the same box produces both outcomes on demand, and both were run:
a 120 s wall settles `complete` naming the log line, an 8 s wall settles
`truncated`. A stale marker would report `truncated` for both. The marker itself
was measured rather than read out of the source — a default boot (1537 lines) and
a `UNAOS_WC=1` boot (2149 lines, 612 more, all of them EARLIER) end their ladder
at the same zeolite block, which is why a compositor line would have been the
wrong choice and would have red-flagged every healthy default run.

**Goes red when** the capture does not reach the marker. **GO-RED proof by
mutation, the wall being the thing mutated:** `UNAOS_WC=1 ./arroyo test 8` — a
wall that cannot reach the marker — exits **1** with
`❌ x86_64 test -> TRUNCATED (did not reach :: zeolite: metrics …forwarded upstream :: in 8s; rc=1)`
and a sidecar reading `completion=truncated` / `complete_line=-`; the same tree at
`UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 120` exits **0** with
`completion=complete` / `complete_line=1984`. Before the change, the 8 s run
exited 0.

**Companion, same commit:** `scan_serial_faults` on a MISSING log returned 0 — the
scan's one input absent, answered with the same 0 a spotless boot gets (LEDGER
S8). It now returns 1 naming the path. Proved by lifting the function into a probe
harness: missing log → rc=1 with the two-line reason; a clean capture → rc=0 and a
capture carrying `-> FAIL` → rc=1, both unchanged, so the pattern list itself did
not move.

**Second control — a WEDGE, not a wall (LADDERTAIL, rmbp seat, 2026-09-15).** The
mutation above moves the CLOCK, so on its own it proves only that the marker
notices a wall running out. The capture this gate was really written against is a
DEADLOCKED FIXTURE — `docs/dev/QUEUE.md` §5's LOCKFIX row, where a non-reentrant
spin `Mutex` in `click_pointer_pos` wedged the battery task and took `LOCKFIX-B1`
and `APPPIN` off the wire while the run still printed `✅ Test run complete` and
returned **0**. It returned 0 because that tree PREDATED this gate; its own log
says `⚡ test: no completion signal declared for this verb`. Replayed at
`31c7e5cb` against the UNCHANGED spec, that same capture is refused:

| capture | `--settled` | `mbench --replay` |
|---|---|---|
| `lockfix-logs/serial-GORED.log` (wedged) | `status=truncated stopped_at=1652` **rc=3** | `✂️ TRUNCATED` **rc=3** |
| `UNAOS_WC=1` full wall, 2148 lines | `status=complete complete_at=1980` **rc=0** | `✅ PASS` **rc=0** |
| default boot, full wall, 1531 lines | `status=complete complete_at=1370` **rc=0** | `✅ PASS` **rc=0** |

A wedge and a short wall are ONE failure to this marker, so **the LOCKFIX row's
owed fix is already delivered by this gate** — no new directive was needed.

**Why the coverage reaches the whole ladder, as a number rather than a claim.**
The marker is emitted AFTER the ladder's last verdict on BOTH configurations —
measured at `31c7e5cb`: default, last verdict `:: SOCK-4: … :: PASS ::` @1351 vs
marker @1370; `UNAOS_WC=1`, @1969 vs @1980. The ladder and zeolite are one task's
output in that order, so no wall reaches the marker while cutting a fixture, and
any cut — clock or wedge — loses the marker too. This is also why `APPPIN`, the
declared last leg of `witness_battery` and the obvious candidate for a tail
assertion, is the wrong line to assert: present @1887 under `UNAOS_WC=1`, ABSENT
from the default boot.

**A REQUIRE in `x86-test.spec` WAS measurably inert, and is not any more — FIXED
(LADDERTAIL, second commit).** `qemu_await.py`'s `settled()` answered on
`matcher.markers()` alone and never called `Matcher.complete()`, so REQUIRE and
COUNT could not reach `./arroyo test`'s verdict: a positive witness added to this
spec would have sat in the file looking load-bearing while being incapable of
reddening anything — LAWS §5's "a check that cannot fire is an absent one", made
worse by the function's own docstring promising the opposite predicate. `settled()`
now gates on `matcher.complete()`, so both of `qemu_await.py`'s modes score the
same three things and TWO MODES, ONE PREDICATE is true of the code as well as of
the header.

**GO-RED for that fix, replay-only (no QEMU), recorded here.** One unsatisfiable
`REQUIRE :: NOSUCHFIXTURE-ZZZ: … :: PASS ::` appended to a COPY of the spec, asked
of the SAME healthy `UNAOS_WC=1` capture; plus the unchanged spec over that capture
and over the wedged one, which must not move:

| probe | before | after |
|---|---|---|
| bogus REQUIRE, healthy capture | `status=complete complete_at=1980` rc=**0** | `status=truncated reason=short-witnesses:1 stopped_at=2148` rc=**3** |
| unchanged spec, healthy capture | `status=complete complete_at=1980` rc=0 | `status=complete complete_at=1980` rc=**0** |
| unchanged spec, wedged capture | `status=truncated stopped_at=1652` rc=3 | `status=truncated stopped_at=1652` rc=**3** |

The two refusals stay DISTINGUISHABLE, which is the point of the new field: a hole
in the ladder reports `reason=short-witnesses:<n>` and names the short patterns on
stderr, a short wall reports only `stopped_at`. **Blast radius is nil today and
that was checked rather than assumed:** `x86_test_completion` (`arroyo:3194`) is the
only `--settled` caller in the tree and `x86-test.spec` is the only spec it passes
— 0 REQUIRE/COUNT — so every current verdict is byte-identical; `mbench.py
--self-test` 35/35. The fix is NOT what closes the LOCKFIX row (the ordering above
already did); it removes the trap waiting for the next seat that reaches for a tail
witness.

**And not every tail witness is assertable even then.** `[ptrdead] backlog` is a
known Class-3 flake under host load (`docs/dev/FIXTURE_FLAKES.md`): asserting it
would convert a loaded-box flake into a TRUNCATED verdict, re-importing the
inconclusiveness this gate exists to remove.

**Legitimate update.** When the boot grows a fixture after zeolite, the marker
becomes EARLY rather than wrong — it stops covering the new tail and never
false-reds. Move it in the commit that adds the fixture, re-measure the default
and `UNAOS_WC=1` runs, and rewrite the MEASURED block in the spec with the new
line numbers. Do not add a second `COMPLETE` to cover two configurations: markers
are OR-ed (`Matcher.complete()` takes `any(d.hits …)`), so a second one can only
weaken the file. Lengthening a wall (`./arroyo test 90`) is the right response to
a red on a loaded box; deleting the marker is not.

---

## GATE-BANNERCERT — the banner and the artifact must agree

**Where this one runs, because it is the second exception to the header above.**
Not in `check_both`: it asserts a property of a BUILT IMAGE, so it lives inside
the media verbs of `unaos/arroyo`, after the kernel has been written and before
the media is announced. `./arroyo check` cannot see it and is not asked to.

**ALL FOUR media verbs are wired** (BANNERCERT2, 2026-09-15 — the first three
months of this gate's life were x86 only, which was the wrong way round, since
the incident that created it was on the jetson path):

| verb | artifact certified | banner list passed |
|---|---|---|
| `esp_x86` | `target/x86_64_esp/kernel.elf` (the staged ELF the firmware loads) | `${_feats%,}` |
| `esp_arm` | `target/aarch64_esp/kernel.elf` | `$(arm_features)` |
| `esp_jetson` | `target/aarch64_esp/kernel.elf` | `$(arm_features)` |
| `kernel8` | `$KERNEL8_DIR/kernel8.img` (the FLAT binary) | `$K8_FEATS` |

Three details in that table are load-bearing.

*Which artifact on the Pi.* Both the ELF
(`target/aarch64-base/release/unaos-kernel`) and the flat `kernel8.img` the
`llvm-objcopy` produces from it are on disk. The ELF is the easier target and the
WEAKER question: it carries non-allocatable sections — debug info, symbol and
string tables — whose bytes the VideoCore ROM never loads, so a token found only
there would certify a string that is not in the booted image. `-O binary` emits
exactly the allocatable sections, which is exactly what the Pi executes, and
`LC_ALL=C grep -a -o -F` reads a flat binary as happily as an ELF. The image is
certified: same question, strictly stronger.

*Which list is the claim on aarch64, and why it is not `$_feats`.* The aarch64
kernel is compiled from `$(arm_features)`, not `$KERNEL_FEATURES` — that function
STRIPS twenty x86-only names (`smolnet`, `kbdwit`, `sdhcblk`, `deadman`, the
wifi/bt families, `gen7`, `noaspm` …) so aarch64 media stay byte-identical
whether or not the x86 track armed them, and `build_kernel_aarch64` prints the
stripped list as `⚡ aarch64 effective features:` exactly when it differs from the
banner. That effective line is what an operator is told to read for an aarch64
build, and it is what cargo was handed, so it is the claim under test. Feeding
`$_feats` instead would red every aarch64 build on a documented, measured,
byte-identity-enforced strip — a gate nobody would keep.

*Why `esp_jetson` certifies INSIDE the function.* `esp_jetson_img` is a bare call
to `esp_jetson`, the dispatcher accepts six spellings across the two jetson
verbs, and the 2026-09-13 incident was precisely a second step rebuilding
`kernel.elf` over the first one's. Certifying inside the function means every
spelling and every caller gets the same verdict on the same bytes, and
`jetson_card_image` can only ever copy an ELF that has already answered for its
banner.

**Invariant.** For every feature named on the `⚡ kernel features:` line of a
build, the artifact that build produced contains that feature's CERTIFYING
STRING — a literal that is in the image if and only if the feature is compiled
in. For a small set of features the banner did NOT name, the artifact does not
contain their control string.

**Why a gate.** The banner is a claim made by the knob→feature map at the top of
`arroyo`, evaluated at script load; the artifact is what cargo actually built.
Nothing in the tree connected the two. `arroyo:1147` armed the GA10B rung-5
ignition with `[ "${1:-}" = "esp-jetson" ]` against SIX accepted spellings of the
jetson verbs, so five of six built a feature set the banner did not describe —
and the `esp-jetson-img` step that follows `esp-jetson` in the same chain rebuilt
the parent feature alone and overwrote `kernel.elf` (`docs/dev/QUEUE.md` §5, the
2026-09-13 row). Every instrument in the tree was green: the diff was green,
`./arroyo check` was green, and the banner printed the rung. `LC_ALL=C grep -a -o
-F` on the built ELF found the rung's bytes 0 against a good card's 79. A card cut
from that media boots, prints nothing, and looks like a clean flight — one power
cycle spent on nothing, with clean-looking evidence. That is LAWS §5: an
instrument's presence is proven in the artifact, never in the diff, the check or
the banner. This gate is that proof, charged automatically at every x86 media
build instead of by an operator who remembers to grep.

**Mechanism.** `unaos/scripts/banner-cert.sh <artifact> <banner-feature-list>`.
`esp_x86` passes `${_feats%,}` — the very string the banner line was composed
from, not `$KERNEL_FEATURES` and not the knob line, because the banner is the
claim under test. The script carries the token registry as a table IN THE SCRIPT:
`feature|token|cond|state`, one row per feature, and asks
`LC_ALL=C grep -a -o -F -- "<token>" <artifact> | wc -l` for each. It prints one
line per feature — `feature=<f> witness=<token> hits=<n> -> OK|MISSING|…` — then
the control rows, then a one-line summary. Tokens are cut at the first `{`
because `format_args!` splits a format string into the literal pieces between its
holes, and every token must be ≥ 9 bytes or LLVM immediate-encodes it out of
`.rodata`; the script refuses to run at all (exit 2) if a shorter one is
registered, so a broken row cannot read as a clean build. Two TOKEN qualifiers
exist: a leading `!` inverts a row (the token must be ABSENT when the feature is
on, for the features whose only gated literal is the not-compiled-in message), and
a leading `@boot ` routes the check to `EFI/BOOT/BOOTX64.EFI` beside the artifact
(`unaos_ivb` is a cross-crate boot-info ABI knob whose code is in the bootloader,
not the kernel, and whose witness is therefore in the other binary).

`cond` — "what else must be true for this literal to exist at all" — has three
TERM forms beyond a bare feature name, each added because a real row needed it,
and all three print UNVERIFIABLE (loud, named, never a pass and never a red) when
unsatisfied. A cond this grammar cannot parse is neither lenient nor strict — it
is a row whose meaning nobody knows — so the script exits **2** naming the row and
the term before printing one verdict, exactly as a sub-9-byte token does:

* `a+b` — an OR-GROUP. The shape of a module with two INDEPENDENTLY GATED
  CALLERS. `ga10b_fw` is gated on `ga10bprobe5` alone, but its only callers are
  `ga10b_ignite` (under `ga10bprobe5a`) and the QEMU fixture at `main.rs:1749`
  (under `witness`); with neither compiled in the linker garbage-collects the
  whole module and its `.rodata` with it. A feature can be in the cargo feature
  set, compile, and still put NOTHING in the artifact because nothing calls it —
  so a row's cond is seeded from the CALL SITES, not from the `pub mod` line.
* `!a` — a NEGATED term: the literal exists only when `a` is OFF, because `a`
  makes the code it lives in unreachable. `witness`'s token sits in
  `kernel_main`'s post-GUI tail, and `main.rs:79` declares in its own `cfg_attr`
  that `baremetal`, `bootlog`, `usbdebug` and `tegra` each make that tail
  unreachable; the compiler is told so and deletes it. A `UNAOS_WITNESS=1
  ./arroyo kernel8` image therefore carries the entire witness battery and cannot
  carry that string. Copy the cond from the `cfg_attr`; do not guess it — and then
  read the code the `cfg_attr` points AT, which is the next bullet's whole story.
* `!a@except:<t>+<t>…` — a NEGATED term WITH AN EXCEPTION (CERTCOND,
  2026-09-15). `a` makes the literal unreachable EXCEPT on a build satisfying
  EVERY term after `@except:`. A term is either an ARCH (`x86_64`, `aarch64`) or a
  FEATURE; `+` is AND here, not OR, because the spec is ONE configuration copied
  out of the source's `not(all(…))`. The arch is answered by **the artifact's own
  ELF header** — `e_machine`, two little-endian bytes at offset `0x12` (`0x3E`
  x86-64, `0xB7` aarch64) — and never by an argument, so a `cond` can be wrong
  about the source, which a human can check, and can never be wrong about the
  bytes under test, which is the thing the gate exists to interrogate. A flat
  image (`kernel8.img`) has no header: the caller may pass the arch as argument 3,
  and the census line says which of the two happened on every run. With no header
  and no argument the arch is `unknown`, every arch term is UNSATISFIED, and the
  row stays UNVERIFIABLE rather than being guessed in either direction.

  **WHY IT IS A CONJUNCTION AND NOT A BARE ARCH, which is the part worth
  keeping.** The obvious spelling is `!usbdebug@aarch64`, read "usbdebug only
  kills this literal on aarch64", and it is wrong for the only row that needs it —
  wrong in the REDDENING direction. `main.rs:~1155` gates the usbdebug terminal
  loop, the thing that actually deletes the post-GUI tail, on
  `all(feature = "usbdebug", not(all(target_arch = "x86_64", feature = "wc")))`.
  The exemption is an arch AND a feature together, and the loop still compiles on
  x86_64 WITHOUT `wc` — the knob's original purpose, pre-GUI bring-up on a card
  with no compositor at all. A bare-arch term would mark that build's `witness`
  row checkable, find the literal correctly absent, and print MISSING: a false red
  on a real configuration, where the over-refusal it replaces is only a loud
  silence. LAWS §5, wrong-strict is worse than wrong-lenient. The term copies the
  `not(all(…))` it comes from, so it cannot be wrong in a way the source is not.

**AND THE RULE THAT CAME OUT OF NEEDING IT: A `cfg_attr` IS A LINT DIRECTIVE, NOT
THE GATE.** `main.rs:79` is an `allow(unreachable_code)` naming four features that
CAN make the tail unreachable. It is deliberately coarse — a lint allowed too
widely costs nothing — and BANNERCERT2 copied it faithfully into the cond, which
was the right instinct applied to a document that was never a contract. The gate
is each feature's OWN early-exit, and one of the four is narrower than the
`cfg_attr` says. So: a negated term is seeded from the `#[cfg]` on the code that
does the deleting; a `cfg_attr` or a comment is a POINTER to that code, never the
source. This is step 6b of the script's seeding recipe.

**WHAT IT FOUND ON ITS FIRST ARMED RUN, and fixed in the same arc.** `esp-x86` on
the rmbp flight-7 knob line: 27 of 28 banner features certified in the artifact,
and `sdwrite` did not. A real banner lie, not a bad token. `arroyo:1992` appends
`sdwrite` to `$_feats` unconditionally, so the banner names it on every verb — but
the x86 MEDIA kernel is compiled by `builder/src/main.rs`, which composes its OWN
feature list from env knobs and had no `sdwrite` entry. The same `esp-x86` log
printed both lists and they differed by exactly that name: arroyo's
`⚡ kernel features:` ended `…,gmux_igd,sdwrite` (28), the builder's own
`   kernel features:` ended `…,gmux_igd,smolnet` (27). In the artifact,
`SDWRITE-POSTURE` had 0 hits while `:: USBREG` — printed from the same
`witness`-gated `unafsroot_selftest` — had 3, so the enclosing code was live and
the feature simply was not compiled in: every x86 media image cut since A60 landed
shipped without it while the build log said otherwise. This is the `rastmc` defect
that `builder/src/main.rs` already records in its own comment ("printed in the
feature banner, `strings` on the ELF had no `RAST-MC` in it"), and the s42/INSTGUI
and GMUX-IGD lesson, a third time — and the first one a gate found rather than a
person. The ruling was that arroyo is right and `sdwrite` rides every image, so
the builder gained
`if std::env::var("UNAOS_NOSDWRITE").is_err() { feats.push("sdwrite"); }` beside
its siblings and the cert reads 28/28. This is the argument for a standing gate
rather than for a remembered grep: three occurrences, two found by hand years
apart, the third found on the first run of the thing that looks every time.

**Registered divergences, and why the table is empty again.** A feature the banner names
that the artifact provably does not carry, whose fix is owned elsewhere, can be
entered in `bc_registered` with the whole finding, the exact fix and an owner. Such
a row prints
`-> MISSING (REGISTERED DIVERGENCE — not a finding, and it must reach zero)` with
its reason, is counted on its own field of the summary, and does not red the
build; an UNREGISTERED MISSING still reds. The shape is GATE-LEDGER's registered
field-count exception, verbatim, and it is held to the same standard: not a
finding, not a pass, and it must reach zero. `sdwrite` sat in it for exactly as
long as it took to get the ruling above, then came out in the commit that fixed
it — that round trip is the whole intended lifetime of a row: register, fix,
delete.

THE SECOND OCCUPANT, and it made the same round trip inside two commits.
BANNERCERT2's first armed aarch64 run — `./arroyo esp-arm` with NO knobs, the most
default build this tree has — found that **every aarch64 banner named `ehcihid`
and no aarch64 artifact could carry one byte of it**. `drivers/mod.rs:9` gates the
whole module `#[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]`;
`arroyo:337` appends the feature default-on; and `arm_features`, which strips
x86-only names from the aarch64 cargo line precisely so aarch64 media stay
byte-identical, stripped its own TWIN `kbdwit` and not the driver that twin
instruments. Measured: the row token 0 hits AND
`LC_ALL=C grep -a -o -F 'EHCI-HID'` 0 — an absent feature, not a rotted token.

It was REGISTERED rather than fixed on the spot for one reason, and the reason is
worth keeping because it is the only case where a strip is not free: every one of
`arm_features`' other strips was added while its feature was OFF by default, so
nothing observable moved. `ehcihid` is default-ON, so removing the name shifts
cargo's `-Cmetadata` for the DEFAULT aarch64 build and re-hashes every Pi and Orin
card cut after it. The executor put the finding, the exact one-line fix and the
owner in the registry and asked. **The ruling (Peter, 2026-09-15): the recorded
card shas are history in MANIFEST files, not a contract; a lie in the banner is.**
`arm_features` gained `f="${f//,ehcihid,/,}"` beside the kbdwit strip, the
registry row came out in the same commit, and the emitted CODE is unchanged — the
module was never in an aarch64 image to begin with. Two occupants, two round
trips, an empty table after each: register, fix, delete.

**Control.** Three separate ways a zero is kept distinguishable from a rotted
pattern. (1) The OFF side: five features the flight line does not arm carry
control rows, asserted ABSENT on every build that does not name them, so a
pattern that matched nothing would still have to explain why the five it is
supposed to miss are the only ones missing. (2) NO VERDICT is loud and is not
zero: a banner feature with no row exits **2** by name, and a row whose artifact
is not on disk does the same — an unchecked check is never silently skipped
(LAWS §5). (3) The one feature with no certifiable literal in the tree, `wedge2`
(the knob only re-times an existing path and adds no gated string), is a `-`
row that prints `NOWITNESS` with its reason and is counted in the summary, rather
than being quietly absent from the table.

**Seeding a row is a MEASUREMENT, not a reading.** Find a literal whose only
occurrences sit under `#[cfg(feature = "<f>")]`, or inside a module whose
`pub mod` is so gated, or under a feature `<f>` implies in `Cargo.toml`; cut it at
the first `{`; then build media with the feature armed and prove hits > 0. Two
further traps, both met and both now numbered steps of the script's recipe:
NEVER seed from a literal a `const fn` consumes (`ga10bprobe5` was seeded on the
64-hex-character vendor digest at `ga10b_fw.rs:75`, which is the argument to
`const fn hx(s: &str) -> [u8; 32]` — only the 32 decoded bytes ever reach an
image, so that row measured 0 on a build that carried the feature and would have
measured 0 forever), and mind the two `cond` forms above, which exist because a
feature can be compiled in and its literal still absent.

The `state` column records what proved the row: `measured` (the 28 x86 rows, from
the rmbp flight-7 artifact) or `measured(N)`, which also carries the HIT COUNT the
proving artifact returned, so a later re-seed that changes the count is visible
without rebuilding the old image. **The table is 48 rows: 47 measured, 1 NOWITNESS
(`wedge2`), and NO `unmeasured-here` row left** — BANNERCERT2 measured every
aarch64 row against a real artifact (`esp-arm` with no knobs; `esp-jetson` on the
orin flight line from the newest staged MANIFEST, plus one `UNAOS_GA10B_PROBE5=1`
run for the rung-5 pair; `kernel8` on LAWS §9's Pi desktop line of record).

**Goes red when** the banner names a feature the artifact does not carry (MISSING,
rc=1), the artifact carries a feature the banner never named (LEAK, rc=1), or the
gate could not reach a verdict (rc=2). **GO-RED proof by mutation, both failure
modes run:** with the flight-7 knob line on `esp-x86` — the same command that had
just exited **0** — the registry row for `nvidia-kepler-fifo` was re-pointed at
`[NVIDIA] Starting PFIFO initialisation`, one letter off the literal in the image.
The verb printed
`feature=nvidia-kepler-fifo witness=[NVIDIA] Starting PFIFO initialisation hits=0 -> MISSING`
and exited **1**; the registered `sdwrite` row on the same run stayed a divergence
rather than masking it, so one registered row does not blunt the others. Reverted,
the script is byte-identical to the pre-mutation file and the verb exits 0. The
rc=2 mode was run separately: the script against the same artifact with a banner
naming an unknown feature prints
`feature=notafeature witness=<UNREGISTERED> hits=- -> UNREGISTERED` and exits
**2**.

**THE `wc` ROW, RE-SEEDED — AND THE TRAP IS THAT IT WAS MEASURED CORRECT
(SMALLFIX3, 2026-09-15).** The row shipped seeded on
`[wc-x] activate DECLINE reason=fb-not-ready latch=released`, a literal in
`video/desktop_uefi.rs::activate`. That function is `wc` code by module gate, and
the seeding run measured it present, so the row passed every test this recipe
had. It was still wrong, and it BLOCKED THE FLIGHT IMAGE BUILD: QUARRYX86 hit
`feature=wc … hits=0 -> MISSING` on
`UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_QEMU_FULL=1 ./arroyo test-fat sf 200` at
`6f45030c`, exiting 1 before QEMU ever started, and every leaner `UNAOS_WC=1`
build would have done the same.

The cause is step 5's trap with one more turn on it. `activate` has exactly ONE
caller — the Kepler takeover — and `main.rs:1141` says so in prose. The seeding
build was the rmbp flight-7 knob line, which arms `UNAOS_KEPLER_TAKEOVER=1`; the
caller was compiled, the linker kept the function, and the token measured 1. Drop
the kepler knobs — which is what QEMU does, because there is no Kepler there —
and the linker garbage-collects the whole function. **Measured on two x86
artifacts built the same day, from the same track tip:**

| token | flight-7 (kepler armed) | `UNAOS_WC=1 UNAOS_QUARRY=1` (no kepler) |
|---|---|---|
| `[wc-x]` | 26 | 26 |
| `wc-x] activate` | 6 | **0** |
| `[wc-x] activate DECLINE reason=fb-not-ready latch=released` | 1 | **0** |
| `[wc-x] desktop-app DECLINE reason=no-storage name=/` | 1 | 1 |

`[wc-x]` at 26 in BOTH is the control: the feature is compiled and printing on
either line, so this was never a banner lie — it is a row pointed at a function
one particular knob line happens to keep. That is a DIFFERENT fact from the
:1147 defect this gate exists for, and the output could not tell them apart.

The row is now
`wc|[wc-x] desktop-app DECLINE reason=no-storage name=/|-|measured(1)`, seeded
from `desktop_uefi::desktop_app_service`, whose call site (`main.rs:5999`) is
gated `#[cfg(feature = "wc")]` and nothing more — so the literal exists on every
build that arms the knob and no build that does not. `cond` stays `-` because
there is no second gate to name. Two rules came out of it and are now steps 7(a)
and 7(b) of the script's recipe: **a row's token belongs in the code path the
feature's OWN knob makes live with nothing else armed**, and **a row measured
only on a rich knob line is measured on the configuration least likely to expose
this defect** — measure the leanest arming build, or measure both and record that
they agree.

**The 48-row audit that came with it.** Every row's token was measured against
BOTH x86 artifacts above and cross-read against the live cert's verdict on the
flight-7 banner (27 OK, 0 MISSING, 0 LEAK, 1 UNVERIFIABLE, rc 0). No second row
has the re-seeded row's shape: of the 28 features the flight-7 banner names, 27
certify with hits ≥ 1 and the 28th is `unaos_ivb`, whose `@boot ` token lives in
`EFI/BOOT/BOOTX64.EFI` and certifies there (a raw grep of the KERNEL elf reads 0
for it, which is the routing working, not a finding). The 20 aarch64-only rows
measure 0 on both x86 artifacts and appear on no x86 banner, which is what they
should do.

⚠ **ONE FINDING, REGISTERED RATHER THAN FIXED, AND IT RUNS THE OTHER WAY:
`witness` IS NOT CERTIFIED ON THE FLIGHT-7 LINE, AND ITS TOKEN IS THERE.** The
row's cond is `!baremetal,!bootlog,!usbdebug,!tegra`, copied by BANNERCERT2 from
`main.rs:79`'s `cfg_attr`, so the flight-7 line — which arms `UNAOS_USBDEBUG=1` —
prints `-> UNVERIFIABLE (its literal needs 'NOT usbdebug' …)` and greps nothing.
But the literal measures **1 hit** on that very artifact, because the usbdebug
terminal loop that would delete the post-GUI tail is itself compiled out on
x86 + `wc` (`main.rs:1155`:
`#[cfg(all(feature = "usbdebug", not(all(target_arch = "x86_64", feature = "wc"))))]`).
So the cond over-refuses on exactly the configuration the rMBP flies: a row that
would have certified prints a loud nothing instead. It is not a red and not a
MISSING — it is coverage silently narrower than the table claims, which is this
file's own "a check that cannot fire is an absent one" seen from one step back.
Fixing it needs a `cond` term that can say "`usbdebug` only kills this literal
when NOT (x86 and `wc`)", i.e. an arch-aware conjunction the cond grammar does
not have; adding one is a gate-language change and is not folded into a row
re-seed. ~~**Registered here, and in `docs/dev/QUEUE.md` §5 on the row this arc
ticks. Owner: whoever next extends the cond grammar.**~~

✓ **FIXED (CERTCOND, 2026-09-15, branch `exec-rmbp-certcond` off `bc9cf442` —
re-derive the sha at the land). The grammar gained the term, the row was
re-seeded, and THE FLIGHT-7 CERT NOW READS 28/28 WITH `unverifiable=0`.** The cond
is `!baremetal,!bootlog,!usbdebug@except:x86_64+wc,!tegra`, state `measured(1)`,
and the `witness` row on the flight-7 line is
`feature=witness witness=:: U1a: no application processors online — ring-3 demo SKIPPED :: hits=1 -> OK`
where it printed UNVERIFIABLE for two arcs. Summary, same command, before → after:
`ok=27 … unverifiable=1` → `ok=28 missing/leak=0 registered-divergences=0
unverifiable=0 nowitness=0 noverdict=0`. The coverage the table claimed and the
coverage it had are now the same number, which is the whole content of the fix:
nothing about the artifact changed, only what the gate is willing to say about it.

**GO-RED, three mutations on the SAME ARTIFACT with no rebuild** — the point of
running them on one ELF is that the only variable is the gate:

| # | mutation | result | rc |
|---|---|---|---|
| a | cond → `!usbdebug@except:aarch64+wc` (right shape, WRONG ARCH) | the `witness` row returns to `-> UNVERIFIABLE`, the census naming the arch it read from the header: *"the artifact's arch is x86_64, not aarch64"*; summary `ok=27 … unverifiable=1` | **0** |
| b | cond → `!usbdebug@onlyon:x86_64+` (UNPARSEABLE) | `❌ banner-cert: NO VERDICT — the cond for 'witness' is not in the grammar: term '!usbdebug@onlyon:x86_64+': the only qualifier after '@' is 'except:'`, printed BEFORE any row verdict | **2** |
| c | CONTROL, the `sdwrite` shape: one letter off a token (`posture=` → `pasture=`) | `feature=sdwrite … hits=0 -> MISSING`, `witness` still OK | **1** |

(a) is rc 0 on purpose and that is the design, not a weak go-red: an UNVERIFIABLE
has never been a red in this gate and must not become one — it is a statement that
the gate declined to look, and the row LINE plus the `unverifiable=` field are
where it is counted. (c) is the control that keeps (a) and (b) honest: the same
script on the same bytes still reds a real absence, so the two new exits are new
behaviour and not a gate that stopped looking.

**THREE MORE PROBES ON THE ARCH READER ITSELF, because a new input channel that is
never fed a wrong value is an unchecked one.** Same artifact, plus a flat copy of
it made with `llvm-objcopy -O binary`:

| probe | census line | `witness` row |
|---|---|---|
| FLAT image, no arch argument | `arch=unknown (FLAT image (no ELF header) and the caller named no arch …)` | UNVERIFIABLE — *"the artifact's arch is unknown, not x86_64"* |
| FLAT image, caller passes `x86_64` | `arch=x86_64 (FLAT image (no ELF header); the caller named this arch in argument 3)` | `hits=1 -> OK` |
| ELF, caller LIES and passes `aarch64` | `⚠ argument 3 says 'aarch64' and the ELF header says 'x86_64' — the HEADER wins`, then `arch=x86_64 (read from the artifact's OWN ELF header …)` | `hits=1 -> OK` |

The third is the one that matters and is why the reader is a reader and not a
parameter: an argument that disagrees with the bytes is announced and discarded.
The first is the conservative direction on purpose — with nothing to read the arch
from, the row declines rather than guessing, and rc stays 0.

**THE SWEEP THE FIX OWED: is any OTHER row over-refusing?** The defect is
"UNVERIFIABLE on an artifact where the row's token measures > 0", and every row of
the 48 was measured against the SAME flight-7 ELF this arc built, then cross-read
against its cond's verdict on that build's banner. **`witness` was the only one,
and after the fix there is none.** Eight other rows have a cond that goes
unsatisfied on that banner, and all eight measure **0** hits: the `tegra` family
(`bsptick`, `bsprun`, `sdmmc`, `ga10bprobe5a`) and the `baremetal` family (`smp7`,
`vugpar`, `genet`, `nettest`). None of them rides an x86 banner, so a real x86 run
never consults their conds at all, and their zeros are the OFF side working rather
than the same defect. Every one of the 28 rows the banner DOES name now measures
≥ 1 — `unaos_ivb` included, at 1 hit in `EFI/BOOT/BOOTX64.EFI` where its `@boot `
token routes it. The remaining 0-hit rows are all off-banner aarch64 and Pi rows,
which is what they should read on an x86 artifact. No row is registered as a known
over-refusal, because there is none left to register.

⚠ **ONE THING NOT TAKEN, and it is one line in a file outside this arc's set.**
`arroyo`'s `kernel8` call site passes two arguments, so the FLAT `kernel8.img` is
certified with `arch=unknown`. Nothing regresses — the only arch-qualified term in
the table is on `witness`, whose cond hits `!baremetal` first on every Pi build and
never reaches it — but an arch-qualified term added to any Pi row would silently
stay UNVERIFIABLE there. The fix is to pass `aarch64` as argument 3 from that call
site (and `x86_64`/`aarch64` from the ELF verbs is unnecessary: they are read from
the header). Owner: whoever next touches `unaos/arroyo`'s media verbs.

**AND `smolnet` WAS THE SAME DEFECT, FOUND THE SAME DAY — the datum came from
SMALLFIX, measured twice, and it is the reason 7(a)/7(b) are rules rather than an
anecdote.** `UNAOS_IVB=1 ./arroyo esp-x86` — no other knobs — exited 1 with
`feature=smolnet witness=:: SOCK-3: no free address-space slot hits=0 -> MISSING`.
Same shape as `wc`, different knob in the caller's gate. The literal lives in
`arch/x86_64/syscall.rs::sock3_launcher`, which IS gated
`all(feature = "smolnet", target_arch = "x86_64")` — the row looked right — but
its only two callers (`main.rs:902`, `:909`) are
`all(target_arch = "x86_64", feature = "witness", feature = "smolnet")`, and
`witness` is OFF for every media verb (LAWS §5's default-quiet-knob rule). So a
DEFAULT-ON feature that rides every x86 media banner had its certifying string in
a function no media build links. `smoltcp` measures **137** hits on that same
ELF — the control proving the feature was compiled and the row simply pointed at
dropped code.

Re-seeded on `:: SOCK-1: smoltcp icmp echo` from `smolnet::witness_tick`, whose
caller `drivers/e1000.rs:1194` sits inside `service_net` under that same
`all(feature = "smolnet", target_arch = "x86_64")` and nothing else — the boot
connectivity witness, reached on every default boot's service pass. **Both
polarities measured:**

| artifact | `:: SOCK-1: smoltcp icmp echo` | `:: SOCK-3: no free address-space slot` | `smoltcp` |
|---|---|---|---|
| `UNAOS_IVB=1 esp-x86` (witness-free, the build that red) | **1** | 0 | 137 |
| flight-7 `esp-x86` | 1 | 1 | 147 |
| `UNAOS_WC=1 UNAOS_QUARRY=1` (no kepler) | 1 | 1 | 147 |
| `esp-arm` — `arm_features` STRIPS `smolnet`: the OFF control | **0** | 0 | **0** |

The aarch64 row is the OFF side and it is a real control, not an assumption: the
strip is `arroyo`'s first `arm_features` line, and `smoltcp` at 0 there
corroborates that the whole stack is absent rather than the pattern being broken.

**Legitimate update.** A new knob in `arroyo`'s map needs a row here in the same
commit — the next build that arms it exits 2 by name until it has one, and that
is the intended failure, not a nuisance. When a token's source line is deleted or
reworded, the row is re-seeded by the recipe above (measure, do not guess) in the
commit that moves the literal. Do NOT "fix" a MISSING by deleting the row: a row
removed is a feature nothing checks, which is the exact state that shipped the
:1147 card. Widening the control list is always safe; narrowing it costs a
control and should be argued for.

---

## GATE-FC2 — a module may not be declared wider than every path that can reach it

**Invariant.** For every non-inline `mod` / `pub mod` declaration reachable from
the two crate roots (`crates/kernel/src/lib.rs` for `unaos_kernel`,
`crates/kernel/src/main.rs` for the binary): if the module is referenced at all,
and EVERY reference site sits under some cfg predicate the declaration does not
already carry, the declaration is the wider one and the gate reds. Exceptions are
registered, with their measurement, in `unaos/scripts/fc2.registry`.

**Why a gate.** This tree states its knob discipline on CALL SITES — the site is
`#[cfg]`-gated, the arm degrades to a shim, knob-off is byte-identical — and the
DECLARATION is the half nothing checked. `pub mod foo;` with no cfg compiles
`foo.rs` into every image, statics linked and `panic::Location` lines counted,
even when every path that can reach it is behind `target_arch` or a `feature`.
LEDGER S5 asked for the check by name after three instances were found by eye
(`flight_recorder`, `dock`, `pulsewin`); S3 is the same complaint about
`flight_recorder` specifically, and S21 called it "the fourth confirmed instance".
Found by eye is the problem: all three named instances were ALREADY gated at
`d6b3c9a7` — `flight_recorder` since `77c61e3a`, its own landing commit — while
the rows still read ARMED, and the tree meanwhile held four instances no row
named. A gate measures all 162 declarations on every `check`; a reader measures
the three they remember.

**Mechanism.** `unaos/scripts/fc2-check.sh`. It walks the module tree from both
crate roots, resolving `<name>.rs`, `<name>/mod.rs`, `#[path]` and inline-module
directories, and gives every FILE the conjunction of the declaration cfgs above
it — which is how a site in `arch/x86_64/serial.rs` is known to be x86 with no cfg
of its own. It then resolves every `::`-path and every (brace-expanded) `use` in
the tree to an absolute module path and credits it as a reference, carrying the
union of its file's inherited cfg and the innermost enclosing `#[cfg]`s on its
line. Four things in this tree make the naive form wrong, and each cost a false
finding before it was handled:

  * **Comments.** 21 of the 27 raw `flight_recorder` hits are prose. Stripped
    first, exactly as GATE-KNOB must.
  * **`cfg_if!` arms.** `arch/mod.rs` declares `x86_64` and `aarch64` in the two
    arms of one `cfg_if!`. Handled generically, the second arm UNIONS with the
    first and every file under `arch/aarch64/` inherits `target_arch = "x86_64"`
    as well — which reported `net_sntp` as x86-only on the strength of sites in
    `arch/aarch64/genet.rs`.
  * **Glob re-exports.** That same file does `pub use aarch64::*;`, so the tree
    spells the Orin's drivers `arch::xusb_tegra`, never `arch::aarch64::xusb_tegra`.
    Unrewritten, that module reads as 3 consumers instead of 20.
  * **Cargo feature implication.** `facet = ["quarry"]`, `gen7 = ["intel-ivb"]`,
    `nvidia-kepler-ce = ["nvidia-kepler"]`, `piinstall_confirm → piinstall`: four
    declarations that already carry their consumers' atom, spelled through Cargo
    rather than through the cfg. Atom sets are closed under `[features]` on both
    sides. Note the asymmetry this exposes, which the gate then relies on:
    implication carries FEATURES, never `target_arch`, which is why `install::pi`
    needed an arch term even though `piinstall ⇒ baremetal ⇒ pi`.

Every remaining approximation biases toward SILENCE and the script's header says
so: `any(...)` contributes only the atoms common to all its arms, a site reachable
only through a cfg'd caller in another file reads as ungated, and an inline module
is censused but never a finding.

**Control.** Two, and they run before any verdict. (1) The FIXTURE, because this
gate's failure mode is a quiet zero: the analyser runs over a synthetic crate
carrying two deliberate instances — `ctrl_fc2`, a bare unconditional declaration,
and `ctrl_fold`, a declaration with a non-cfg attribute FOLDED onto its line —
plus one deliberate non-instance, `ctrl_ok`, already gated. It must report exactly
`{ctrl_fc2, ctrl_fold}`. `ctrl_fold` is in the fixture because the scanner WAS
blind to that shape: anchored at `pub mod`, it stopped seeing a module the moment
the gate's own prescribed fix was applied, and folding a cfg onto `splash` took the
census from 162 declarations to 161 in silence. **The control is proven by
mutation, not by argument:** restoring that `pub mod`-anchored regex in a copy of
the script and running it against this tree exits **2**, printing
`The analyser reported: ['ctrl_fc2']` and `A zero here would read as a clean tree.
It is a broken gate.` (2) Three TREE PROBES the parser
must rediscover: `arch::x86_64` carries `target_arch = "x86_64"` from its `cfg_if!`
arm; some `flight_recorder` reference under `arch/x86_64/` carries an INHERITED x86
cfg; and `flight_recorder` has NO reference in `fs/fat.rs`, which names it six
times in prose. Plus `facet = ["quarry"]` must be readable out of `Cargo.toml`, and
at least one glob re-export must be found. Any control failure exits **2** — no
verdict — and `check` prints that a gate which gave no verdict is not a pass.

**Goes red when** a declaration is wider than the intersection of its references'
cfgs and the module is not in the registry (exit 1), or a control fails (exit 2).

**GO-RED proof, recorded in this gate's landing commit:** reverting the arch term
on `install/mod.rs:51` — `all(target_arch = "aarch64", feature = "piinstall_confirm")`
back to `feature = "piinstall_confirm"` — takes the probe from rc=0 to rc=1 naming
`install::clone` with its four sites in `install/pi.rs`, and `./arroyo check` from
rc=0 to rc=1 through this gate's own failure line; restoring the term returns both
to green. That mutation is also the update recipe, run backwards.

**What it found on its first run** (at `d6b3c9a7`; census 162 declarations / 162
referenced / 4 findings): none of the three instances the ledger rows name — all
three were already gated — and four the rows did not. Two were fixed on their
declaration lines in the landing commit (`install::pi`, then `install::clone`,
which the first fix exposed by giving `install/pi.rs` an inherited arch cfg); three
are registered (`splash`, `rtpi`, `video::desktop_firmware`), each because the
declaration carries a written ruling this gate is not entitled to overturn.

**Legitimate update.** Narrow the declaration to the union of its references'
cfgs, **on one line** — `#[cfg(...)] pub mod x;`, or edit an existing attribute
line in place — so no `panic::Location` below it moves: LAWS §5 names a cfg'd-out
`pub mod` DECLARATION as the one exception (the file is never lexed) and the
module root as not. Or register the module in `unaos/scripts/fc2.registry` with
the measurement behind it. The registry is not an allowlist to grow: its header
says it must reach zero, and a row is legitimate only while the declaration
carries a deliberate written decision about what the module's absence means. "It
builds fine" is not a reason; the gate already knows it builds.

---

## GATE-KNOBBUILD — the knob→builder wiring probe, and the day it red-lined about nothing

**Where it runs.** Inside `check_kernel_cfg` in `unaos/arroyo`, after the
knob→leg coverage check and immediately before GATE-KNOBPARITY, which extends it
and shares its `return 1`.

**Invariant.** Every `UNAOS_*` knob in arroyo's top-level `_feats` map whose
feature is named by a LITERAL x86 cfg leg (the derived `x86-mix-N` legs are
excluded: their unions hold features media never arms) is READ by
`builder/src/main.rs`. x86 MEDIA features are rebuilt by the builder from its OWN
env map, so a knob wired here alone lights the `⚡ kernel features:` banner while
the kernel the metal boots carries nothing. arroyo warned about that in prose
four times and the warnings still produced a fifth victim (`rastmc`,
2026-08-27), which is why it is a check. aarch64 needs no twin: its build invokes
cargo directly and prints the effective-features line when it differs from the
banner.

**Mechanism.** One `sed` parses arroyo's own map for
`[ -n "${UNAOS_X:-}" ] && _feats="${_feats}<names>,"`, and each matched knob whose
feature is in the literal-x86 set must appear as a quoted string in
`builder/src/main.rs`. Note WHAT is grepped: the env var name in the builder, not
the `feats.push`. A rename on one side only is the failure it is built for.

**Control.** `UNAOS_WC->wc` must be parsed out of the knob map. If it is not, the
map parser is broken and the probe gives NO verdict, loudly, rather than
reporting an empty difference as a clean tree.

**LEDGER SO6 — the control's finest hour, and the defect it exposed.** On
2026-09-06 `unaos/arroyo check` run from the REPO ROOT red-lined this probe with
no defect behind it. `${BASH_SOURCE[0]}` is the path the script was INVOKED with,
so from the repo root it is RELATIVE — and the probe runs after arroyo has cd'd
internally, where `unaos/arroyo` no longer resolves. `sed` read nothing, the
control refused a verdict, and a build was lost to a red about nothing. **It
failed loudly instead of passing silently, and that is the whole argument for
the control probe.** Fixed the same day by `981463ea`: the path is
`$WORKSPACE_DIR/$(basename "${BASH_SOURCE[0]}")`, absolute by construction
(`WORKSPACE_DIR` is computed by `cd`-ing `dirname "$0"` at load, before any
internal `cd`), with `basename` keeping the rename case. The builder path was
already anchored the same way. What `981463ea` did NOT ship is the proof below.

**Goes red when** a mapped, x86-leg-named knob is unread by the builder (the
`rastmc` shape), or the control misses (no verdict).

**GO-RED proof, three states × BOTH invocation directories, measured 2026-09-15
at `2946ea30`.** The drill runs against a scratch MIRROR of the tree — `crates`,
`scripts` and `arroyo` symlinked to the real ones, `builder/src/main.rs` a
writable copy, and a harness copy of arroyo whose cfg-leg `cargo check` line is
stubbed to `true` with one extra `cfgonly` dispatch arm. That is the shape this
file's own standard section prescribes, and it means the mutation never touches
the tree and no cargo runs. Invoked as `./unaos/arroyo-h cfgonly` from the mirror
root and as `./arroyo-h cfgonly` from its `unaos/`:

| state | mirror root | from `unaos/` |
| --- | --- | --- |
| clean | rc=0, `✅ knob→builder wiring OK` | rc=0, same line |
| builder stops reading the knob (`UNAOS_WC` renamed to `UNAOS_WINCOMP` in the copy, `feats.push("wc")` left in place) | rc=1, `❌ … UNAOS_WC->wc — mapped in arroyo, named by a literal x86 leg, and UNREAD by builder/src/main.rs` | rc=1, identical |
| map line broken (the harness's own `UNAOS_WC` map entry renamed) | rc=1, `❌ … control probe FAILED — UNAOS_WC->wc not parsed from this file's knob map` | rc=1, identical |

Same verdict text and same exit code from both directories in all three states,
which is the SO6 property under test; the copy was restored and diffed back to
byte-identical with the tree's file after the drill.

**Legitimate update.** A knob whose feature an x86 leg names gets its builder env
line in the same commit. Excluding a knob from the probe is not an update path —
GATE-KNOBPARITY exists because the ONE idiom this `sed` parses was already too
narrow once (`sdwrite`), and narrowing it further is the direction that produced
both victims.

---

## GATE-KNOBPARITY — the three feature vocabularies must agree

**Where it runs.** Inside `check_both`'s kernel-cfg-coverage gate in
`unaos/arroyo`, immediately after the existing `knob→builder wiring` probe and
sharing that function's `return 1`. It is an EXTENSION of that probe, not a
second stage, on purpose: both answer the same question about the same two files,
and `check` should carry ONE knob-wiring verdict rather than two that can
disagree about which is authoritative. The work itself is a script,
`unaos/scripts/knob-parity.sh`, only so that it can be run — and gone red — by
hand. It builds nothing; it reads three files.

**Invariant.** Every feature name arroyo can append to `$_feats` with NO
environment set (default-on: `[ -z "${UNAOS_NO…:-}" ]`, or unguarded) is pushed by
`builder/src/main.rs`. Such a name is on the `⚡ kernel features:` banner of every
x86 media verb by construction, and the builder's list is the only one that
reaches the kernel an x86 media image boots, so a name in the first and not the
second is a lie on every card cut from that tree.

**Why a gate, and why the probe above could not be it.** The wiring probe parses
ONE idiom — `[ -n "${UNAOS_X:-}" ] && _feats="${_feats}<names>,"` — and demands
that every such knob whose feature a literal x86 cfg leg names be read by the
builder. It caught `rastmc`. It could not catch `sdwrite`, and the reason is
purely structural: `sdwrite` is not armed by a positive knob at all
(`arroyo:2000` appends it default-on), so it never matched the `-n` pattern. It
rode the banner of every verb and every x86 media image cut since A60 shipped
without it. GATE-BANNERCERT found that on the ARTIFACT on its first armed run —
but one artifact at a time, and only for a feature that was on that run's banner
AND had a token row. This gate is the vocabulary half of the same answer: it
compares the sets and answers for every name at once, with no build.

**Mechanism.** Three sets, counted before anything is quantified:
(a) every name arroyo can append to `$_feats`, with line numbers — both idioms,
plus the `case`-arm appends (`ga10bprobe5a`), the `&& { … }` compound ones
(`bsprun`) and `esp_jetson`'s in-function forcing; (b) every
`feats.push("<name>")` in `builder/src/main.rs`; (c) every feature declared in
`crates/kernel/Cargo.toml`'s `[features]`. It prints |a| |b| |c| and the three
difference sets unfiltered, then reds on a\b restricted to the default-on subset
of (a). At the fold: a=146 (5 default-on), b=82, c=185; a\b=64, a\c=0, b\c=0.

**Why only that subset reds.** a\b at large is 64 names and is NOT a defect —
most of arroyo's map is knob-gated and much of it is aarch64-only (`tegra`, the
ga10b probes, the whole orin ladder), where the build invokes cargo directly and
the builder is not in the path. Deciding which of those SHOULD be in the builder
needs the cfg matrix, which is the wiring probe's job. The default-on subset needs
no judgement: those names are on every x86 media banner with no env set at all.
a\c and b\c should both be EMPTY; a non-empty one is a feature name Cargo does not
declare, i.e. a path that has never been built.

**Control.** Six control probes, each asserting a name this tree provably
contains: `wc`, `sdwrite` and `tegra` in (a) (the last proves the in-function
`esp_jetson` appends are seen), `smolnet` in the default-on SUBSET of (a) (the
classifier, not the extractor), `wc` in (b), `witness` in (c). A miss exits **2**
with no verdict, because a parser that broke reads exactly like a tree that is
clean. `sdwrite` is deliberately NOT a control on the builder side — the go-red
drill removes that very line and must produce a finding, not a broken gate.

**Goes red when** a default-on arroyo name is not pushed by the builder (rc=1),
or a control probe misses (rc=2). **GO-RED proof by mutation:** comment out
`builder/src/main.rs`'s `feats.push("sdwrite")` line and the script prints
`❌ knob-parity: sdwrite — default-on in arroyo (arroyo:2000,2009 …) and NOT
pushed by builder/src/main.rs` and exits **1**; restored, `git diff` on that file
is empty and it exits **0**. THE FIRST RUN OF THAT DRILL CAME BACK GREEN, and the
reason is worth keeping: a plain `grep feats.push` matches the COMMENTED-OUT
line. That is this gate's own failure mode seen from the inside — a name present
in the file and compiled into nothing — so the extractor now truncates every line
at its first `//` before matching, and a commented-out push reads as unwired,
which is what it is.

**Legitimate update.** A new default-on knob needs its builder line in the same
commit. Widening (a)'s extractor is always safe. Narrowing the red — e.g.
excluding a name because "it is aarch64-only" — costs the gate its whole point
and must be argued for; the correct fix for an aarch64-only default-on name is
that it should not be appended at top level in the first place.

---

## GATE-KNOBOFF — a byte-identity verdict is only as good as its cache state

**Where it runs.** Not in `check_both`. This one lives in the `knoboff` verb
(`./arroyo knoboff <feature> [baseline-ref]`), which compares the knob-off
loadable image of your tree against a baseline's. `check` never runs it; a brief
asks for it by name and the executor quotes its exit status.

**Invariant.** The two images a `knoboff` verdict is computed from were each
produced by exactly one `unaos-kernel` compile, in this process, against a cache
in which every dependency was already built — and the `include_bytes!`d
`unaos/target/user_blob.bin` was the same file for both.

**Why a gate.** The verb already built both images in ONE directory, which
removes the path delta (SO34). It said nothing about the cache, and the cache
delta was built into the phase order: on a fresh per-caller key the BASELINE
build is the one that pays for every dependency and build-std crate, and the tree
build it is compared against then runs with all of that warm. Cold against warm,
on every first run, in the one direction the tool could not see. The rmbp seat
measured what that is worth on 2026-09-16 (SMALLFIX, QUEUE §5): one pristine tree
with one banner hashed `9d3103c3…` for `kernel8.img` against a cold target dir and
`0b6f2381…` against a warm one, **2638 bytes apart**, with warm→warm and cold→cold
each reproducing. A comparison across that boundary answers a question about the
cache in the words of a question about the source.

The second half was found by the census on its own first run and is worse,
because it is silent. Cargo fingerprints on mtime, `git reset --hard` rewrites
only the files that actually differ between two commits, and switching feature
sets back and forth re-links a `deps/unaos-kernel-<metadata>` artifact an earlier
run produced. So a docs-only or arroyo-only diff makes cargo compile **nothing**:
measured here, the second `./arroyo knoboff deadman` against a warm key finished
in **1.87 s having invoked no compiler at all**, compared three images objcopied
out of .elf files the previous run had left behind — one of them the cold-built
baseline — and printed PASS. A gate whose green means "I did not look" is the
`a-check-that-cannot-fire` shape with a clean exit status.

**Mechanism.** Three parts, all in `unaos/arroyo`'s `knoboff` block.

1. *The census.* `_knoboff_flat` takes a 7th argument and captures THAT build's
   cargo output to its own file before it joins the shared run log, so its
   `Compiling <crate>` lines can be attributed to the build whose verdict depends
   on them. The run prints, unconditionally and before the verdict,
   `knoboff: warm=<yes|no> compiled_baseline=[…|…] compiled_tree=[…|…]  (x86|arm)`.
2. *Warm first, score second — conditionally.* `_knoboff_scored_pair` builds,
   reads its own census, and if that census is anything other than exactly one
   `unaos-kernel` compile it discards those images, re-dirties the two crate roots
   (`_knoboff_touch_kernel`) and scores a SECOND build, which now has every
   dependency warm. A first pass that already says `unaos-kernel` alone IS that
   warm compile and is scored as it stands, so the extra compile is paid on cold
   and no-op runs and on no others. The armed control build is not scored for
   identity, only for inequality, and is left alone — arming a feature may
   legitimately pull in an optional dependency.
3. *The `include_bytes!` payload.* `unaos/target/user_blob.bin` is produced by
   `build_user_blob`, not by cargo; it survives `git clean -fd` (ignored) and no
   phase of `knoboff` rebuilds it. Its sha256 is recorded immediately before each
   scored build and a difference is exit 2, naming the file — a byte delta with no
   source behind it would otherwise be scored as the executor's. Under the default
   feature sets the site is `baremetal`-gated and both sides read `absent`, which
   is the ordinary case; under `UNAOS_TEGRA_EL0=1 UNAOS_KNOBOFF_WITH_ENV=1` it
   reaches the image.

**Control.** The existing armed probe is unchanged and still required: arming the
feature must move at least one image or there is no verdict. The warm-cache
assertion adds its own, which is the census line itself — it is printed on every
run, including the green ones, so a `0` quoted without `warm=yes` beside it is
visibly an incomplete quotation rather than a silent one.

**The `-s` trap, recorded because it cost a cut.** The first cut tested the
census file with `[ -s ]`. `paste -sd, -` emits a lone newline for empty input, so
the one-byte file that means "cargo compiled nothing" passed `-s`, and the no-op
half of the gate could not fire. It printed `warm=yes compiled_tree=[<none>|<none>]`
— its own output contradicting its own verdict — on its first run.
`_knoboff_census_empty` tests CONTENT.

**Goes red when** a scored build compiles any crate but `unaos-kernel`, or
compiles nothing, or the user_blob moves between the two scored builds: exit
`2 — NO VERDICT: cache not warm (rebuilt: <crates>)`, which is never a pass.
**GO-RED proof, measured 2026-09-16 on `hw-rmbp` at `56103466`, five states:**

| # | state | result |
|---|-------|--------|
| a1 | warm-up retry removed (harness mutation), fresh per-caller key = genuinely cold | **exit 2**, `warm=no`, census named 43 crates (`alloc,bincode,…,x86_64`) on the baseline and `<none>` on the tree — i.e. the exact cold-vs-stale pair the old code passed |
| a2 | same key, mutation removed | **exit 0**, `warm=yes`, `compiled_baseline=[unaos-kernel\|unaos-kernel] compiled_tree=[unaos-kernel\|unaos-kernel]` |
| b1 | dependency touch injected under the scratch worktree (`crates/net/src/lib.rs`, mtime +2 h) | **exit 0** — the retry ABSORBS it (`↻ baseline: first pass compiled [net,unaos-kernel\|…]`, scored pass kernel-only). This is the must-PASS fixture: an arc that edits a dep gets a verdict, not a refusal |
| b2 | `Compiling` parser mutated to mis-name every crate | **exit 2** naming `unaos-kernel-mutant` |
| c | control still reds a real move: `xhcikbd`, HEAD `d78241fa` vs baseline `d863eab2` | **exit 1**, `warm=yes`, control fired on both arches, x86 1246172 B and arm 1120995 B differing, both sizes changed |

A known-identical pair (the docs-only `56103466` against its parent `707c293d`)
returns **exit 0** with `warm=yes`. The reproducibility the discipline buys is
visible across the table: the scored images are the same bytes from a cold key,
from a warm key and from a third caller key on the same box (x86
`1262919d…`/1537216, arm `243e2e99…`/1608416), where before the fix the same
source pair scored `6c88e301…` cold.

**Cost**, before and after on the same box state (`deadman`, the docs-only pair,
20 cores under load ~12): COLD 115.61 s → 165.57 s, the `+49.96 s` being the one
extra kernel-only compile per arch and nothing else; WARM 1.87 s → 95.23 s. The
warm pair is not a regression to argue about — the 1.87 s it replaces is the run
that compiled nothing and scored the previous run's artifacts.

**Legitimate update.** Widening what counts as warm is the dangerous direction and
needs a measurement, not an argument: the whole gate is the claim that a verdict
computed across a cache boundary is void, and SMALLFIX's 2638 bytes is the price
of being wrong about it. Adding a crate to the allowed census (there is no
allowlist today, only `unaos-kernel`) would have to show that crate cannot reach
the image. The honest standing limit is the opposite one and is not a bug: an arc
whose diff makes a NON-kernel workspace crate recompile on BOTH passes gets NO
VERDICT here rather than a wrong one, because the instrument has no power to
separate the dep's codegen from the knob's. Score that arc on the armed artifact
instead. Do NOT relax the refusal to get a number out of the tool — a `0` whose
cache state is unknown is exactly the green SMALLFIX proved means nothing.
## GATE-BATTERY-EVIDENCE — a leg judges the capture it keeps

**Where it runs.** `battery()`'s `_step` in `unaos/arroyo`. Not in `check_both`:
it asserts a property of the HARNESS rather than of the tree, which makes
GATE-TESTTRUNC its sibling, and it is held to the same standard here.

**Invariant.** The file a battery leg takes its verdict from is the file that leg
leaves behind, and it holds only bytes that leg's own command produced.

**Why a gate, and what it cost (LEDGER SR10).** `_step` accepts
`logfile[:verdictfile]`, because `kernel8-test` does not echo its serial capture
to stdout. The step's stdout was kept under `target/battery/`; the VERDICT was
taken from `test_kernel8`'s fixed `target/serial-pi.log`, read where it lay. Two
consequences, in opposite directions, out of the same sharing:

* **Forwards.** The next `kernel8-test` overwrites it — and the standing rule for
  a red leg under load is *re-run it alone*, so the prescribed diagnostic is what
  destroys the capture that would say why. Measured 2026-09-10: battery step log
  mtime 12:02:45, verdict file mtime 12:04:35, i.e. the re-run's bytes, not the
  battery's. The pi4 leg was twice written off as a load flake with nobody able
  to read its evidence. What the battery DID keep is the step's stdout, which for
  that leg is mbench's verdict TABLE — the one file whose FORBID rows quote
  `-> FAIL` as pattern text, which is why the scan was pointed away from it.
* **Backwards.** Bytes the leg's own command never wrote could reach its verdict:
  anything left in the shared path between one leg and the next is judged as if
  it had come off that leg's wire.

**Mechanism.** When a step names a verdict file distinct from its log, `_step`
now (1) removes the shared path and its `.run` sidecar BEFORE running the
command, so the leg judges only its own run, and (2) copies the capture to
`<step>.serial.log` beside the step log afterwards, sidecar beside it at
`<step>.serial.log.run`, and every scan reads THE COPY. `cp`, not `mv`: the
callee's own path stays where its verb documents it. The sidecar travels beside
the capture and is never appended into it — a harness writes beside its evidence
(LAWS §5), and a `mode=` / `log_sha256=` line inside a serial log is a line
mbench would then judge. A red step names both files. `test_kernel8` already
`rm -f`s that log itself, so (1) costs the real leg nothing; it moves the
invariant off a callee that may change and onto the battery that depends on it.

**Control.** A leg whose OWN run puts `-> FAIL` on the wire must still red — the
fourth row of the table below. Without it the first three rows are satisfiable by
a harness that judges nothing at all.

**Goes red when** a leg's command exits nonzero, its pattern is absent from the
capture, or `FAULT_PATTERNS` matches in it. A leg that produced no capture leaves
the copy ABSENT, the `awk` scans then fail to open it and the leg reds — the
honest verdict for a leg with no evidence.

**GO-RED proof, by execution; no QEMU and no build.** A scratch harness extracts
the real `_step` out of a given `arroyo` and drives it with stubs: leg-A writes a
clean capture into the shared path and is judged; a later writer then appends
`fixture: -> FAIL` to that shared path AFTER leg-A's verdict — the "re-run the
red leg alone" diagnostic, or simply the next leg; leg-B then makes its own clean
run. Same harness, same stubs, run against the base file and the fixed one:

| assertion | base `2946ea30` | fixed |
| --- | --- | --- |
| leg-A green, its own run being clean | PASS | PASS |
| leg-B green, the poison not being leg-B's evidence | **FAIL** | PASS |
| the file leg-A JUDGED still holds leg-A's bytes | **FAIL** | PASS |
| CONTROL: a leg whose own run emits a fault line still reds | PASS | PASS |
| harness exit | 1 | 0 |

The third row is measured as a sha of leg-A's judged file at its verdict and
again at the end of the battery: `209829f0…` then `bd955032…` at the base,
`209829f0…` both times after the fix.

**Legitimate update.** A new leg needing a separate verdict file names it the
same way and inherits both halves. Pointing a leg's verdict back at a fixed
shared path, or at any path a second leg also writes, reintroduces this exactly;
the capture a leg is judged on belongs under `target/battery/` with that leg's
own name on it.
## GATE-LBA32 — a 64-bit LBA may not be narrowed to 32 bits with `as`

**Invariant.** Over `crates/kernel/src/drivers/**` and `crates/kernel/src/fs/**`,
with comments and string literals stripped: every `<expr> as u32` whose operand
names an LBA — an identifier token containing `lba` or `sector`, in any case, that
is not SCREAMING_SNAKE — is a finding, unless the function enclosing it is
registered in `unaos/scripts/lba32.registry` **and still carries a refusal**.

**Why a gate, and this one is unusual: it is the only thing that can guard these
call sites at all.** Three files in this tree have paid for the same defect — a
`u64` sector number handed to a 32-bit field with `as`. `as` on an out-of-range
value does not fail, it TRUNCATES: LBA `0x1_0000_0000` becomes `0`, THE BOOT
SECTOR, so a read returns the wrong sector as if it were right and a write
destroys the partition table and returns success. Orin ledger **A57** folded
`arch/aarch64/sdmmc_tegra.rs`'s six sites onto `sd_block_arg`; **SR15**
(BLOCKSMALL) folded `drivers/block.rs`'s eight onto `read10_lba32`; **SR15**
(EMMC2LBA) folded `drivers/emmc2.rs`'s two onto `card_block_arg`. Each shipped a
known-answer fixture — and SR15 states, in its own row, the bound every one of
them hits: *reverting one CALL SITE to a bare cast is invisible to the fixture*,
because the `lba >= num_blocks` geometry bound one line earlier refuses an
out-of-range LBA with the same error, so a wrapped site and a refusing site are
indistinguishable from any caller on any reachable input. **No behavioural leg can
close that gap.** SR15's closing sentence asked for this gate by name: "a grep
gate over the call shape … would make it mechanical and is not built."

**Mechanism.** `unaos/scripts/lba32-check.sh`. It strips comments and string,
raw-string and char literals (preserving columns and newlines, so a reported line
number and the source a reader opens agree), then for every `as u32` it walks
BACKWARDS from the keyword to find the operand: a `)` walks to its matching `(`
with the callee, which is how A57's `(lba * 512) as u32` is caught; otherwise a run
of identifier/path/field characters, which catches `hdr.start_lba as u32`. The
enclosing function is the last `fn <name>` above the line. Two exclusions are
load-bearing and both are in the control fixture: SCREAMING_SNAKE names (a
`SECTOR_BYTES`/`SECTOR_SIZE` is a sector SIZE, never a sector NUMBER — without this
the gate fires eight times on a clean tree and is skipped within the week), and
anything inside a comment or a literal (this tree *prints* the defect on the wire,
`wrapto=`/`wrapblk=`, and discusses it in prose far more often than its code
commits it).

**The registry is re-checked, not trusted.** A row names a function whose
narrowing is proven by a refusal INSIDE that same function — the `sd_block_arg`
shape, `if lba > u32::MAX as u64 { … }` standing above `Some(lba as u32)`. Every
run re-reads the registered body and requires a refusal token (`u32::try_from`,
`try_into`, `> u32::MAX`, `>= u32::MAX`, `checked_mul`); a helper gutted back to a
bare cast loses its cover and the row becomes a finding. That is what stops the
registry laundering the defect it exists to record.

**Control.** Eight, over a synthetic in-memory file, before any verdict — three
that MUST fire and five that MUST stay silent, each killing one stage of the
analyser: `ctrl_bare` (the bare cast), `ctrl_mul` (the parenthesised byte-offset
shape), `ctrl_gutted` (REGISTERED but with no refusal left — the control that
proves the registry is re-checked rather than obeyed); and `ctrl_helper`
(registered and refusing), `ctrl_tryfrom`, `ctrl_comment`, `ctrl_string`,
`ctrl_const`. Any control wrong exits **2** — no verdict — and `check` prints that
a gate which gave no verdict is not a pass.

**Goes red when** an LBA-named operand is narrowed with `as u32` in an
unregistered function, or in a registered one that no longer refuses (exit 1), or
a control misbehaves (exit 2).

**GO-RED proof, recorded in this gate's landing commit, two independent
mutations.** (1) SOURCE: `drivers/emmc2.rs`'s `read_block_512` call site reverted
from `card_block_arg("CMD17", card.block_addressing, lba)?` to `lba as u32` takes
the probe from **rc=0** (`1 on an LBA-named operand (1 registered, 0 not)`) to
**rc=1**, naming `drivers/emmc2.rs:740 read_block_512() — unregistered`; restoring
the call returns it to rc=0. That is the exact mutation no fixture in this tree can
see, which is the gate's whole reason to exist. (2) WIRING: `LBA32_REGISTRY=/dev/null
./arroyo check` against the UNMUTATED tree strips the one legitimate row, so the
probe reds on `card_block_arg` itself and `check` reds through this gate's own
failure line — proving the probe's exit code reaches `check`'s verdict without
recompiling anything.

**What it found on its first run** (at the EMMC2LBA commit; census 41 files, 562
`as u32` casts, 1 LBA-named, 1 registered, 0 unregistered): a clean tree, which is
the expected and the dangerous answer — hence the three must-fire controls. The
registry's header names what was measured clean and therefore absent
(`read10_lba32` and `lba_arg` use `u32::try_from` and form no cast at all;
`fs/fat.rs`'s four hits are the const exclusion; `fs/bootdisk.rs`,
`drivers/ahci.rs` and `drivers/xhci/**` have none) so nobody re-derives it.

**Legitimate update.** Route the narrowing through the transport's refusal helper
(`read10_lba32` / `lba_arg` / `card_block_arg` / `sd_block_arg`), or — if the
function IS that helper — add a row to `unaos/scripts/lba32.registry` citing what
`bash unaos/scripts/lba32-check.sh` printed for it. A second row in the same file
is the state A57 found and fixed (six sites, two shapes) and should be a fold onto
one helper instead.

---

## GATE-FOREMAN — a second implementation of the verdict table must agree with the first

**Invariant.** `tools/foreman`'s `verdict` module and `unaos/scripts/mbench.py`
produce the **same exit code and the same rendered verdict table, byte for byte**,
over a shared corpus: mbench's eight canned self-test fixtures, one added pair
that reaches the multi-shortfall branch, and every checked-in
`unaos/scripts/specs/*.spec` crossed with every `unaos/target/serial*.log` present
in the checkout. `mbench.py` is the source of truth; foreman follows.

**Why a gate, and the shape it closes.** The agreement test already existed —
`tools/foreman/tests/agreement.rs`, written with the module it guards. It was
named nowhere in `unaos/arroyo`, so no verb ran it, and it had been **red** since
QEMU-FAST landed. That is this tree's unnamed-root shape for a third kind of
root: GATE-ROOTS closes it for binaries (a binary is nobody's dependency, so a
binary no leg names is never type-checked), GATE-SPECROOTS closes it for specs (a
spec nothing replays pins lines nothing evaluates), and a **checked-in test that
no gate invokes** is the same defect one layer up — strictly worse than no test,
because it reads as coverage in the tree while catching nothing. The three
divergences it was sitting on are not cosmetic: mbench had learned QEMU-FAST's
`[mode unknown: …]` verdict-line suffix — the three-valued capture-mode read
every consumer must treat as NOT-FULL when it says `unknown` — and SPECRUN's
` (pinned at <spec>:<line>)` coordinate and `FIRST-SHORTFALL <spec>:<line>` line,
which `arroyo`'s `test`/`test-fat` tails quote verbatim so the verb and the table
can never disagree about which pin came up short. foreman had learned none of
them. A second opinion that silently prints a different table is worth less than
no second opinion.

**Mechanism.** A `check_both` leg runs `cargo test -p foreman --test agreement`
from the repo-root workspace, the same convention GATE-CORE uses for
`midden_core`. The test shells out to `python3 unaos/scripts/mbench.py --replay
<log> --spec <spec> --quiet` for each pair and compares `(rc, table)` against
`verdict::evaluate` + `verdict::render_table` on the same pair, reporting **every**
mismatching pair with the first differing line of each, not just the first. Cost
is ~3 s. `TMPDIR` is pinned into the tree's own `target/` by the leg: the test
writes its canned fixtures through `std::env::temp_dir()` and LAWS forbids `/tmp`
on this bench. That directory can never enlarge the corpus — the capture scan
matches `unaos/target/serial*.log` only, and the fixtures are named `good.log`,
`t-cut.log` and the like.

**Control.** Two, and they are the reason this test cannot pass vacuously.
(a) The test **skips loudly** — `SKIP:` on stderr and a return, never a quiet
green — if `mbench.py` is absent or `python3` cannot run it; (b) it asserts
`checked > 0` with the message *"the agreement corpus was empty — nothing was
proved"*, so a corpus that collapses to zero pairs is a failure rather than a
pass. The canned half is written by the test itself and is therefore always
present, which is what makes (b) meaningful in a checkout with no capture in it.
The hand-rolled SHA-256 the sidecar identity check needs has its own control, and
it needs one: the corpus carries no valid `.run` sidecar, so a **wrong** digest
would read as a STALE sidecar and the two tools would still agree. It is
known-answer tested against the FIPS 180-4 vectors plus the empty message, and
the streaming path is asserted equal to the one-shot over a million-byte input.

**Goes red when** either implementation's table or exit code moves without the
other's, on any pair. `check` then prints, through this gate's own failure line,
`check FAILED — foreman's verdict table no longer agrees with mbench's`.

**GO-RED proof, executed in this gate's landing worktree, two independent
mutations, each reverted.** The precondition is itself the first measurement: at
the base commit, with one bench capture staged at `unaos/target/serial.log`, the
test is **red on 21/21 pairs** (`cargo test -p foreman --test agreement` rc=101).
With the three strings taught, it is **green, 21/21 pairs identical** (rc=0,
`agreement: 21 (log, spec) pairs — exit code and verdict table identical`).
(1) `render_table`'s two lines that append `run_mode_note` removed — the exact
state the tree shipped in — take it back to **21/21 red** (rc=101), every pair
naming the `[mode unknown: no run sidecar]` suffix that mbench prints and foreman
does not. (2) `Directive::note`'s `MISSING` arm reverted to drop `self.origin()`
reds **14/21**, naming `0 hits — MISSING (pinned at selftest.spec:2)` against a
bare `0 hits — MISSING`. The two counts differ because the mode suffix is on
every verdict line and the pin only on a row that came up short, which is the
distinction the pin was introduced to make.

**Legitimate update.** Change `mbench.py` first — it is the reference — then teach
`tools/foreman/src/verdict.rs` the same output and re-run the leg. The direction is
not negotiable: foreman following mbench is the invariant, and a change made only
in foreman reds this gate by construction. A new output shape that no pair in the
corpus can reach gets a fixture in `tools/foreman/tests/agreement.rs` in the same
commit — a branch no fixture executes is the vacuum this gate exists to refuse,
and the `(N further pinned line(s) also short)` companion to `FIRST-SHORTFALL`
needed exactly that pair. **What this gate does NOT assert:** that anything on any
bench runs `foreman`. Nothing does today, measured unfiltered rather than sampled:
`grep -rl foreman` over the whole bench directory, scratch worktrees excluded,
returns five files and zero invocations — `media-writer.sh` and two dated backups
citing `verdict.rs` for their exit-code contract, one capture header using the word
as an executor's role name, and one commit-log field in a flash MANIFEST. It is
named in no script, no playbook and no verb, and its live claim on the tree is its
preflight, which is why every
`.spec` in this repo is written look-around free (LAWS §5 Specs and scorers). If
the second implementation is ever retired under R16, this gate goes with it.

---

**Landed but not yet sectioned here:** GATE-ROOTS (`scripts/check-roots.sh`, every
binary target is a named root of `check`) and GATE-APPEND (`scripts/append-position.sh`,
LEDGER P7's trailing-comment trap) are both wired into `check_both` and green; their
invariant, control and GO-RED live in their `arroyo` comment blocks and in the
scripts' headers until a seat gives each a section on the standard above.
