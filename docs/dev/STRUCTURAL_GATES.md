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
GATE-TESTTRUNC below is the exception, and its section says so: it is held to the
same standard — invariant, control, recorded go-red, legitimate update — but it
asserts a property of a QEMU RUN rather than of the tree, so it lives on the x86
`test` legs and `check` cannot see it.

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

`cond` — "what else must be true for this literal to exist at all" — gained two
TERM forms in BANNERCERT2, each because a real row needed it, and both print
UNVERIFIABLE (loud, named, never a pass and never a red) when unsatisfied:

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
  carry that string. Copy the cond from the `cfg_attr`; do not guess it.

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

**Registered divergences, and the ONE row open today.** A feature the banner names
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

ONE ROW IS OPEN, and it is the same class seen from the other arch. BANNERCERT2's
first armed aarch64 run — `./arroyo esp-arm` with NO knobs, the most default build
this tree has — found that **every aarch64 banner names `ehcihid` and no aarch64
artifact can carry one byte of it**. `drivers/mod.rs:9` gates the whole module
`#[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]`; `arroyo:337` appends
the feature default-on; and `arm_features`, which strips twenty x86-only names
from the aarch64 cargo line precisely so aarch64 media stay byte-identical, strips
its own TWIN `kbdwit` at `:2218` and not `ehcihid`. Measured: the row token 0 hits
AND `LC_ALL=C grep -a -o -F 'EHCI-HID'` 0 — an absent feature, not a rotted token.
The fix is one line, `f="${f//,ehcihid,/,}"` beside the kbdwit strip, and it is NOT
free: removing a name from the cargo feature set shifts `-Cmetadata`, so it
re-hashes every aarch64 media image once, breaking every recorded Pi and Orin card
sha. That is the boards' seats' call — exactly as each of the other twenty strips
was a call when it landed — so the row keeps the lie named, counted and un-green
rather than red-lining every aarch64 build for a defect no aarch64 executor
introduced. Owner: the orin and pi seats; rows are in both board queues.

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

**Legitimate update.** A new knob in `arroyo`'s map needs a row here in the same
commit — the next build that arms it exits 2 by name until it has one, and that
is the intended failure, not a nuisance. When a token's source line is deleted or
reworded, the row is re-seeded by the recipe above (measure, do not guess) in the
commit that moves the literal. Do NOT "fix" a MISSING by deleting the row: a row
removed is a feature nothing checks, which is the exact state that shipped the
:1147 card. Widening the control list is always safe; narrowing it costs a
control and should be argued for.

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

**Landed but not yet sectioned here:** GATE-ROOTS (`scripts/check-roots.sh`, every
binary target is a named root of `check`) and GATE-APPEND (`scripts/append-position.sh`,
LEDGER P7's trailing-comment trap) are both wired into `check_both` and green; their
invariant, control and GO-RED live in their `arroyo` comment blocks and in the
scripts' headers until a seat gives each a section on the standard above.
