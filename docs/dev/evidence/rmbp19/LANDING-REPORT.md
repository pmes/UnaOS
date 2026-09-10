# rmbp 19 — landing report (OPEN; written early, per rmbp 17's convention)

The arc is J0: finish step 2 of Peter's three-way sync. The fold was committed and gated by
rmbp 18 at `85bc4769`; the `--no-ff` into trunk had not happened and is this seat's job.
**At the time of writing the merge has NOT been made** — it is held on two decisions that are
Peter's, recorded in §7 below.

## State at open — fresh host `ls-remote`, run that turn (2026-09-10 20:47Z)

    main 751cb816 · hw-rmbp 3f0045de · hw-pi4 247b1b95 · hw-jetson a7a33de1

`git rev-list --left-right --count origin/main...origin/hw-rmbp` = `0 261`.
`git merge-base origin/main origin/hw-rmbp` = `751cb816` — **trunk's tip IS the merge base.**
That matters twice below: it makes the zero-diff shape check safe to read, and it is why the
"clean merge, broken tree" class pi 10 hit cannot form here.

## Pushes named to Peter, batched in the first turn

  * **At open: NONE.** The baton and the resume both said `a7c27009`, `2a08bbb1`, `3f0045de` were
    unpushed. They were on origin — Peter pushed them after rmbp 18 closed. LAWS §3's
    "re-fetch before reporting a push as outstanding" caught it on the first `ls-remote`.
  * **Owed now: ONE**, `git push origin main hw-rmbp`, carrying the merge plus `6fdb9776` and
    `cd905dbf`.

## Baton claims that did not survive verification

| baton said | measured | how |
|---|---|---|
| 3 unpushed commits | 0 — all on origin | host `ls-remote` 20:47Z |
| tip moved 3 commits since orin's ack | **4** | `git rev-list --count 85bc4769..3f0045de` |
| the delta is "all LAWS/RULINGS" | **false** — `unaos/scripts/k8-reach.registry` is in it | `git diff --name-only 85bc4769..3f0045de` |
| (resume) `ledger-check.sh` 447 lines, `DELETION TRIGGER` 2, `A27` 2 | 445, 0, 0 — **and correctly so** | see below |

The last is not a silent drop. Those two entries were REGISTERED WAIVERS for defects in orin's
ledger, each carrying `⚠ DELETION TRIGGER: … DELETE IT IN THE COMMIT THAT FOLDS hw-jetson, never
before`. `85bc4769` is that commit, so their removal is the instruction being obeyed; 447 − 2 = 445.
`FIELDCOUNT_REG` 5 and `ABSENCE_PHRASE` 2 both hold, which is the half of the resume's symbol-count
gate that was still meaningful.

## The round's finding: the landing's own gate reds on trunk, and no landing had ever run it

`./arroyo check` is green on `hw-rmbp` and would have gone RED on `main` on arrival.
**GATE-LEDGER's strict mode is keyed on the BRANCH NAME** — `unaos/scripts/ledger-check.sh:166`,
automatic when `_branch == "main"` — and the gate's own comment at `:296` says of it,
**"The landing runs it."** It had not been run, on this landing or any before it.

    UNAOS_LEDGER_STRICT=1 bash unaos/scripts/ledger-check.sh   ->   REAL_EXIT=1
    GATE-LEDGER: RED — 2 finding(s) across 3 file(s), 278 rows:
        docs/dev/OS/rmbp-ledger.md:52  B22 cross-ref → SO6   does not resolve
        docs/dev/OS/rmbp-ledger.md:118 B88 cross-ref → SP999 does not resolve

**A green `./arroyo check` on a track branch is not evidence about trunk.** That sentence is the
transferable part and both peers took it: pi 10 applied it to their own in-flight trunk merge the
same turn.

### B88 — the fix is not the obvious one

`SP999` is the FABRICATED id rmbp 18 injected as a differential control while proving trunk's older
gate was silent on prefixed cross-refs. The row narrating that control wrote it with the unicode
arrow, which `ledger-check.sh:257` defines as a reference the resolver must resolve; ASCII `->` is a
mention. **Rewriting it as `-> SP999` would have made the sentence lie** — the injected row really
did carry the unicode arrow, that was the experiment. The token is now DESCRIBED rather than
written, which is the remedy B88 already prescribes for its own sha case. orin 25, unprompted:
"I would have accepted `->`. You refused… That is the stricter reading and it is correct."

### B22 / SO6 — a row nobody ever wrote

Not retired, not renumbered. Measured, history rather than heads, with a control so a zero is a fact
about the data:

    git log --all -S'SO6' -- docs/          ->  7 commits mention it
    the ROW form, same probe                ->  0 on every ref, ever
    control: the same probe for SO5, SO7    ->  2 row-commits each
    981463ea "arroyo: fix SO6 (repo-root invocation red-lines the knob->builder probe)" 2026-09-06
    merge-base --is-ancestor: hw-rmbp YES, main NO

The id was allocated in conversation, the defect was diagnosed, the fix was committed, **and the row
was never created.** The gap ran four days. orin 25 found it and granted this seat the single id —
not the range — to file the row inside this landing, so the row arrives with its fix commit instead
of a release later. Grant recorded in both transcripts and quoted in the row.

### Proof, red first, with the mutation

| step | strict exit | rows |
|---|---|---|
| before | 1 — 2 findings | 278 |
| after the B88 fix | 1 — 1 finding | 278 |
| after the SO6 row | **0** | **280** |
| mutation: SO6 row deleted | **1** — B22 dangles again | 279 |
| restored | 0 | 280 |

**The first mutation attempt tested nothing, and is recorded because of it.** Renaming the row's id
to `SO6X` left the gate GREEN. The id extractor is `([A-Z]+[0-9]+)` with **no anchor** — deliberately,
because it must admit the sanctioned `A36 (→ SR2)` suffix form — so `SO6X` still registers as `SO6`.
Deleting the row is the mutation that removes the id. A control that cannot fail is not a control,
and this one was nearly shipped as evidence, which is the same trap B88 records rmbp 16 falling into.

**Second-order, sent to both peers and NOT filed as a row:** a malformed seat-prefixed id silently
ALIASES to its own well-formed prefix — `SP12b` and `SP12`, `SO21b` and `SO21`, are the same id to
this gate and its uniqueness check cannot tell them apart. Not filed because the unanchored match is
deliberate and wrong-strict is worse than wrong-lenient. pi 10 measured their prefix clean (0 hits)
and holds it as a constraint on the next SP id; orin 25 adopted "SO ids get no suffix form, ever" as
a naming constraint rather than a check. **A hazard that is the price of a deliberate leniency
belongs where the ids are allocated, not in the gate.**

## Rows filed this round

| id | what |
|---|---|
| **SO6** | the never-written row, filed under orin 25's grant, `fixed-unflown` |
| **SR11** | a gate whose NOTATION collides with its own SUBJECT MATTER — three instances is a design property |
| **SR12** | a deferral is a promise that something will arrive, and nothing checks that the promise is keepable |

**SR11** is orin 25's reading — the cell delimiter for a row about regexes (B63), hex tokens for a
row about shas, the cross-ref arrow for a row about the cross-ref contract. This seat added the part
that makes it actionable: two of the three already have escapes (`->`, `label:hash`) and the
delimiter has none, so the fix shape is one general escape for quoted material, not a third special
case.

**SR12** is the mechanism SO6 exposed. GATE-LEDGER defers a seat-prefixed cross-ref on a track branch
because the row is branch-local and will resolve when that seat lands. Nothing checks the row exists
ANYWHERE. SO6 deferred on every run, on every seat, for four days. orin 25's non-strict run over the
same tree in the same hour was STRUCTURALLY unable to find it, and said so: two seats, two postures,
only the strict one could see it. **The fix is already written and pointed elsewhere** —
`ledger-check.sh:118` enumerates `refs/remotes/origin/hw-*` and `origin/main` with `git for-each-ref`
for its sha check, and the same enumeration splits DEFERRED-KEEPABLE from RED.

### A shared protocol row is now stale, and this seat did not touch it

LEDGER **P14** said "No gate can fix this: a checker cannot see another branch." That became **false
of this script**, on the strength of `:118`. P14 is a shared row; changing it inside one seat's
landing would be one seat retiring a shared rule on its own discovery, so it was left for the three
seats.

**⇒ CLOSED, and not by the three seats — by Peter, who named BOTH stale sentences himself in pi 10's
session and ordered them removed.** The second, "arrives exactly where the refs became resolvable and
nobody has to remember it", was overtaken by a later ruling rather than wrong when written (orin 25's
framing, and the better one). pi removed both at **`2eb52c77`**, verified from this seat: P14's row
contains neither sentence there, the diff is one line, and **it is UNPUSHED** — `origin/hw-pi4` is
`2461094b` — so both sentences are still live text on every pushed ref including trunk.

**This seat's first probe of that was confounded and is recorded because of it.** `git grep -c … |
wc -l` counts FILES WITH A MATCH, not occurrences, and it reported the first sentence as surviving.
It does survive — **at line 84, in this lane's own SR12 row, which quotes it in order to call it
stale.** A row quoting a stale sentence is indistinguishable from the stale sentence to any search:
SR11's family and SR12's fourth face meeting in one line. **And "count it first" is the rule this
lane wrote into LAWS §5 this week and broke here.**

## LAWS §3 — orin 25's number, corrected by them, with the method written in

§3's virt-coverage argument said "~7,700 changed lines of shared kernel (`shell.rs` 7256,
`main.rs` 405)". orin re-read their own measurement at this landing and found it understated.
Re-derived here per file rather than accepted:

    unaos/crates/kernel/src/shell.rs   4445 + 2811 = 7256
    unaos/crates/kernel/src/fs/vfs.rs   794 +   22 =  816
    unaos/crates/kernel/src/main.rs     362 +   43 =  405
                                                     8477

The rule now names the **paths**, which is the actual fix: a bare `main.rs` basename also matches
`unaos/builder`, `unaos/crates/user-stat`, `unaos/crates/user-vug` and `tools/unafs` — 465 against
the path-scoped 405, and 8537 as a total. **A rule that argues from line counts, containing a
basename, is a number waiting to drift a third time.**

orin traced their own "six-week gap" for SO6 (it is four days) to reading the S27 branch-triage list
that morning and attaching its age to a different finding hours later — a number relayed from memory
instead of the one in front of them.

## Gate results

### x86 own-board legs — this seat's, run ALONE, `REAL_EXIT=0` at `3f0045de`

    check (both arches)                    rc=0
    x86 test 25 (MISSION)                  rc=0
    x86 test-fat sf 300                    rc=0
    x86 usb-write witness                  rc=0
    x86 STAT.ELF off FAT (WINX-2)          rc=0
    x86 VUG.ELF off FAT (WINX-8)           rc=0
    x86 PULSE.ELF off FAT (PULSE-W)        rc=0

Not `./arroyo battery` — R39's six x86 legs plus `check`, which is never platform-scoped. The runner
uses `battery()`'s own names, patterns and fault scan, with **`FAULT_PATTERNS` EXTRACTED from
`arroyo` at runtime rather than hand-copied**, so the scorer cannot drift from the gate. All four
go-red controls fired before the run: the fault scan fires on a seeded `EXCEPTION:` line and stays
quiet on a clean one; the witness scan reds when `MISSION SUCCESS` is absent and passes when present.

`./arroyo check` re-run at the final tip `cd905dbf`: `REAL_EXIT=0`, GATE-FAMILY 10 families,
GATE-APPEND 163 files / 4 controls, GATE-KNOB 165 features, GATE-ROOTS 9 targets, GATE-LEDGER 281
rows zero deferred, k8-reach 126 knobs (11 armed, 115 registered, 104 TODO). **The QEMU legs are held
for the merge result rather than run twice** — §3 names the merge result as where they run.

### orin 25 — GREEN, re-run at each tip, never carried over

    ./arroyo check                         rc=0
    env UNAOS_TEGRA=1 ./arroyo check       rc=0
    env UNAOS_TEGRA=1 ./arroyo esp-jetson  rc=0

Run in a detached worktree at this seat's sha, their `hw-jetson` checkout untouched at `a7a33de1`.
**Recorded in their words, because three rc=0s must not be read as more than they are: "This green
certifies that it compiles and links. It does not certify that it runs."** There is no QEMU machine
for the Jetson; nothing orin can do alone says the Orin boots this tree.

**They re-ran at each tip instead of carrying the green, and it earned itself twice.** At
`3f0045de` GATE-LEDGER read `278 rows, 2 DEFERRED`; at `6fdb9776`, `280 rows, ZERO deferred`; at
`cd905dbf`, `281 rows`. **The verdict was a different verdict each time** — a carried-over green
would have asserted a gate result that no longer described the tree, and the part it would have
missed is the only thing in the delta. Their 32-second run against 11 minutes at `3f0045de`
corroborates the "zero `.rs`" claim from the compile side, measured rather than asserted.

This seat twice declined to tell either peer that their green carried over, and gave them the
measured delta instead. Asserting it would have been the same "registry-only, no re-read" shortcut
this seat had refused from orin an hour earlier, and worse coming from the seat that wants the
landing.

### pi 10 — NO ACK, WITHHELD CORRECTLY

pi had `REAL_EXIT=0`, MBENCH 120/120, 0 fault lines — **at `247b1b95`, pi's own tip, which does not
contain this landing.** Asked for an ack, they refused to let that green float upward: it is a
statement about pi's tree and nothing else. **That is precisely the failure R39 was written about,
declined at the moment it was profitable to take, and it is recorded here as a result and not as an
absence.** They are mid-arc on step 3 of Peter's sequence, three `check` attempts in.

**pi's caveat on their own strict run is the sharpest measurement either seat made today:**
`UNAOS_LEDGER_STRICT=1` on their merged tree returns `REAL_EXIT=0, 130 rows, OK` — from the
**165-line** `ledger-check.sh`, because the 445-line one is on `hw-rmbp` and has not landed. So their
strict green is not evidence about what this gate would say. *"My verification is exactly as strong
as the weakest gate in the fleet."* Seven gates exist only on `hw-rmbp` and reach trunk when this
lands.

**pi's own finding, theirs to file, recorded here because it changes what every seat's next fold must
check:** their trunk merge took pi's DELETIONS and trunk's USES in regions that never textually
conflicted — a clean merge, a broken tree, invisible to git and to review, visible only to a compile.
It cannot form on this merge, and the predicate says why rather than an assurance:
`git merge-base origin/main origin/hw-rmbp` = `751cb816` = trunk's tip, so
`git log --no-merges 751cb816..751cb816` is empty and there is no trunk-side content outside this
branch for a semantic conflict to form against. This lane's exposure to that class was at the fold
`85bc4769`, which was gated.

## §7 — THE MERGE

`084b79ac`, `--no-ff`, parents `751cb816` + `a51a0396`. **Peter cleared the panel skip in his own
session and said "skip land and report".**

The panel review §3 names first was SKIPPED, on his explicit word, and the case put to him was the
merge shape: `git merge-base origin/main hw-rmbp` = `751cb816` = trunk's tip, so trunk was entirely
inside the branch and the merge combines NOTHING NEW. A panel would have reviewed content already
gated in its own round, at a merge that produces no new configuration. This seat recommended the
panel four turns earlier and reversed on that measurement — **the measurement was available before
the recommendation and was not made first**, which is the round's clearest process error.

### Shape, proven, with the control that makes the zero readable

    git log --pretty=%p -1 084b79ac         ->  751cb816 a51a0396     two parents
    git log --no-merges 751cb816..751cb816  ->  0                     zero-diff is safe to read
    git diff a51a0396 084b79ac              ->  0 lines               the arc tip landed whole
    CONTROL: git diff cd905dbf 084b79ac     ->  269 lines             so the zero is about the trees

pi 6's landing-shape check used as a FORWARD predicate rather than an after-the-fact proof, which is
a better use of it than the one it was written for (pi 10's reading).

### Own-board legs ON THE MERGE RESULT — `REAL_EXIT=0`

Run in the TRUNK WORKTREE on `main`, not in this seat's tree, and that is not pedantry about where
"the merge result" lives: **GATE-LEDGER's strict mode auto-arms on the branch name, so this is the
only place it runs in the landing posture rather than being forced with an env var.**

    check (both arches)                    rc=0
    x86 test 25 (MISSION)                  rc=0
    x86 test-fat sf 300                    rc=0
    x86 usb-write witness                  rc=0
    x86 STAT.ELF off FAT (WINX-2)          rc=0
    x86 VUG.ELF off FAT (WINX-8)           rc=0
    x86 PULSE.ELF off FAT (PULSE-W)        rc=0

`GATE-LEDGER: OK — 281 rows in 3 ledger file(s) + RULINGS … cross-refs resolve`, **with no deferred
clause at all**, where the same gate on the track branch before the fix carried `2 cross-branch
ref(s) deferred`. That absence is the proof strict was armed and clean — the trunk red this round
existed to find is gone from the tree it would have appeared on.

### One honesty note on the ack shas

orin 25's third and final ack is at `cd905dbf`; the merged arc tip is `a51a0396`. The delta is
`docs/dev/LEDGER.md` (SR12's fourth face) and this file, two files, zero `.rs`. **It was not counted
as covering `a51a0396` and orin was told so rather than left to assume** — they re-ran three times
across the session specifically to avoid a carried-over green, and the round should not end by
quietly doing to them what they refused to do to themselves.

### What did NOT land with it

**1,479 changed lines of shared kernel and `arch/aarch64` are on trunk with ZERO aarch64 RUNTIME
evidence.** `check` proves they compile, orin's legs prove they link for Tegra, nothing proves they
run: pi owns `kernel8-test` and all three virt legs, and R39 scopes the virt exception to an aarch64
seat's OWN landing. **pi 10's step 3 is the real runtime verdict on this content, and a red there is
a finding about landed code, not a regression from this landing.**

### ⇒ ANSWERED AFTER THE LANDING: pi 10's step 3 came back GREEN

The gap above is closed for the board leg. pi folded trunk into `hw-pi4` at **`2461094b`** — shape
verified from this seat by reading, not by accepting: parents `bce9661d` + `084b79ac`, contains both
`084b79ac` and `751cb816`, and **NOT PUSHED** at the time of writing. Re-derived here at 22:01Z, and
pi 10's framing of it is sharper than "not pushed": `origin/hw-pi4` is `bce9661d`, `2461094b` is not
an ancestor of it, **266 commits unpushed** — while the **SP5 ROW IS reachable on `origin/hw-pi4`**
(1 hit in `docs/dev/LEDGER.md`; control, the SP rows visible there are SP2 SP3 SP4 SP5). **So the
ledger row describing the class is on origin and the fold that proves the remedy works is not. The
two halves got separated**, which is a better instance of the rule than a bare unpushed sha: a
fleet-readable row can point at evidence no other seat can reach.

    pi4 kernel8-test    REAL_EXIT=0 · MBENCH PASS 126/126 required witnesses · 0 forbidden
                        0 fault lines in the capture · CONTROL 168 PASS lines, so the scan can fire

**They closed SR10's trap in the same command that made the evidence** — snapshotting the capture off
the fixed path before anything could overwrite it, which is the row's stated fix arriving before its
fix does. And they ran GATE-LEDGER with **strict forced by hand, citing SR13**, filed less than an
hour earlier.

Their floor prediction held exactly: 120 required witnesses at `247b1b95` → **126** here, predicted
+6 for orin's six new REQUIRE rows, measured +6 — while the absolute they had refused to let anyone
quote (119) was indeed wrong. The delta was right and the caveat was right.

**AND A PREDICTION OF THIS SEAT'S WAS HALF WRONG — scored on both halves, because the first scoring
of it counted only the half that failed.** I told pi to expect their first `./arroyo check` on the
merged tree to be SLOWER and to FIND THINGS.

  * **"find things" — WRONG.** `REAL_EXIT=0`, 81 legs green, 0 red, GATE-FAMILY 10, GATE-APPEND 162
    files / 4 controls, GATE-KNOB 165, GATE-ROOTS 9, GATE-LEDGER 286 rows strict.
  * **"slower" — HELD, and it changed what pi did.** 266 commits of trunk forced a full rebuild, and
    the run took long enough that they held the board legs back rather than run QEMU against a live
    `cargo` build — the contention that killed pi 9's gate three times.

pi scored the second half; this seat had recorded only the first. **A prediction reported as simply
"wrong" when one of its two clauses held and altered a peer's sequencing is a worse record than no
prediction**, and it is the same failure shape as an absence claim that does not name its
enumeration.

**⇒ AND THE VIRT LEGS CLOSED TOO — GREEN. THE GAP IS FULLY ANSWERED.** Reported by pi 10 as their
measurement, on `2461094b`, `UNAOS_QMP_PORT` pinned; this seat did not run them and does not verify
them (R39):

    arm virt v2 (MISSION)   REAL_EXIT=0   "xHCI: >>> MISSION SUCCESS (BOT + CSW) …"  1 occurrence
    arm virt v3 (CAPSTONE)  REAL_EXIT=0   ":: CAPSTONE COMPLETE — all 6 sync primitives …"
    arm usb-write witness   1 match on the write-ok witness
                            CONTROL: 4 usbw lines in the capture, so the assertion can fire

So the ~1,383 changed lines of shared kernel that NO BOARD exercises have been executed and are
green. `kernel8-test` proved the BCM2711 board; the virt legs proved the generic-aarch64 surface.
**This landing's runtime evidence is complete, and none of it was produced by the seat that wanted
it — which is R39 working exactly as intended.**

**⚠ THIS REPORT WAS ONE EDIT FROM CLOSING WITH THAT ITEM MARKED OPEN, AND THE CAUSE IS WORTH MORE
THAN THE CORRECTION.** The legs had been green for forty minutes. pi 10 addressed the "all four
green" message to ONE SEAT — orin's session — while this seat received only the leg-1 message.
**They picked a recipient instead of the population**, which is the same defect this round filed
twice against greps: SR12's fourth face is a seat querying a narrower population than the one that
matters, and this is the same error committed against an audience rather than a file. It was caught
only because pi re-read what they had sent and to whom. **A finding sent to one seat of three is not
reported; it is stored.**

### The mechanism hole the three seats left this morning

pi 10 found it and it is the reason the landing did not wait on them. R39 settled **WHO** runs which
legs; Peter's sequence says **WHEN** — orin lands, rmbp merges trunk and lands, pi merges and gates —
and R39 never re-ordered it. **And no lawful move exists by which a peer produces a board green at
an unlanded peer sha:** a worktree at another seat's branch is what Peter ruled against in pi's
session that day and killed a run over, merging a track into a track is not one of the two sanctioned
kinds, and the tip was unpushed regardless. **A precondition that no permitted action can satisfy is
not a precondition.** Filed here as a hole in what the three seats settled, not as pi's refusal.

## Found after the merge, from orin 25's fourth run: SR13

orin gated every announced sha from a DETACHED worktree, and on their fourth run — at the merge
result — they forced `UNAOS_LEDGER_STRICT=1` by hand rather than relying on the branch name. Their
reason is the finding: **every prior run of theirs was the deferring posture, which is why they could
watch SO6 defer and never watch it red.**

Verified here by execution, with a control:

    detached worktree  (x2)   git rev-parse --abbrev-ref HEAD  ->  HEAD
    trunk worktree, on main   ->  main          this worktree  ->  hw-rmbp

`ledger-check.sh:165` arms strict on `_branch == TRUNK`. **In a detached checkout that is never true,
even at trunk's own sha** — and a detached worktree is the sanctioned shape for gating another seat's
sha. orin used one all session; pi 10 was told to use one.

**The gate's own argument convicts it.** `ledger-check.sh:124` reads "STRICT — WIRED, NOT
REMEMBERED… a backstop nobody is wired to run is a backstop that runs never". The wiring works only
for a seat sitting on the trunk BRANCH — the landing seat verifying its own merge result, which is
the one case that already had it. **Every peer verifying that same sha gets the weaker posture
silently.**

Filed as SR13 with the exact fix: when `_branch` is HEAD, compare `git rev-parse HEAD` to the trunk
ref's sha and arm on EQUALITY, never on ancestry — a detached checkout that merely contains trunk is
a track tip, not trunk. It does not change this landing's verdict: rmbp 19's legs ran in the trunk
worktree ON `main` and got the armed posture, and orin's forced fourth run confirmed 281 rows strict
and clean from a second seat independently.

## What this seat got wrong

**1. Recommended the panel review, then reversed it on a measurement that was available first.**
`merge-base` == trunk's tip was derivable before the recommendation and was not derived until after.
Peter's answer was "skip land and report".

**2. The first mutation control tested nothing.** Renaming the SO6 row's id to `SO6X` left the gate
GREEN — the id extractor is unanchored by design, to admit the sanctioned suffix form, so `SO6X`
still registers as `SO6`. Deleting the row is the mutation that removes the id. Caught only because
the gate stayed silent where it HAD to fire. A control that cannot fail is not a control, and this
lane has now nearly shipped one as evidence in two consecutive rounds.

**3. A `git commit -m` in double quotes let the shell command-substitute a backticked command out of
the message.** SR13's commit `56034937` says "compare c90b72a451fe… to the trunk ref's sha" where it
should say "compare `git rev-parse HEAD`" — a sha that has nothing to do with the fix, reading as an
instruction to compare one specific commit. **LAWS §3 already has the rule — a message carrying code
goes through `git commit -F` — and this seat cited that rule earlier in the same session while
breaking it.** The row and this report were written through a quoted heredoc and are intact;
controls confirm zero 40-hex tokens injected into either. Not amended, so the slip stays legible —
rmbp 18's precedent at `2a08bbb1`, for a commit message that ran ahead of its diff. **B88 records the
identical mechanism biting a control fixture; that makes this the second instance in two rounds, and
the general form is: any shell context that evaluates backticks will eat the command out of prose
about commands.**

**4. A `python3` edit asserted on a two-occurrence anchor and aborted mid-script.** No write happened
only because the write came after the assert — luck of ordering, not design. Scope the anchor to the
line and assert before mutating.

## Push owed

ONE, batched: `git push origin main hw-rmbp`. At the time of writing `origin/main` is `751cb816` and
the landing exists on this box only. Peter pushed `3f0045de` and later `cd905dbf` mid-session without
announcing either, so any successor re-fetches rather than trusting a line in a document.
