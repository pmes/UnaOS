# LAWS.md — standing operational laws

Durable process laws for all UnaOS sessions, moved here from session memory so
they are versioned and reviewable. Session memory keeps only pointers.
`CLAUDE.md` covers layout, lanes, and arc discipline; this file records the
laws minted at the bench and at the seat. Each entry names its origin date.


## /tmp — NEVER. Peter's standing ruling.

**Do not write anything to `/tmp` in this project: no scratch files, no diffs, no
build output, no disassembly, no scratchpads.** Use `~/unaos-bench/scratch/<arc>/`.

**Why, and the second reason is the serious one.** `/tmp` is cleared at 3 days,
so a snapshot taken there is a snapshot with an expiry nobody records — the rmbp
seat lost its executor scratchpads to exactly this and only the git worktrees
survived. But `/tmp` here is also **RAM-backed**, and building under it
**OOM-killed the harness** once. That is not a lost file; that is a dead session.

**Why this is in LAWS.md and not a resume.** This ruling existed since at least
2026-08-19 and was recorded ONLY in `unaos-pi4-resume.md` — one track's private
file. The other two seats never saw it, and both broke it: rmbp lost work to the
3-day clear, and orin wrote a 15 MB disassembly into `/tmp` and then reported the
consequence upward as a discovery. **A standing ruling that lives in one track's
resume is not a rule, it is a local habit.** If a ruling binds every seat, it
belongs here, on the day it is made.

## Direction

Design laws — they bind what the code may assume, not how a session runs.
Everything else in this file is subordinate to them.

- **Boot cold, boot dumb, presume nothing about the machine** (Peter,
  2026-09-08, to the orin seat; the sentence above is his, recorded as a gist
  rather than word-for-word in the orin 22 bulletin §1, and the two quotations
  below are verbatim). The card is the hard drive, and every boot is stone
  cold: no prefs, no special checks, no accumulated knowledge of the box —
  even though it is the same box every time. Four consequences, in the order
  they were broken. **(1) The kernel finds the disk that has THIS kernel on
  it, by content**: it compares a window of its own running image against the
  candidate files on every block source it enumerated, and roots the OS layout
  on the disk that matches. Nothing else identifies the disk. **(2) No board,
  bus, slot, card serial, card geometry, knob or BOOT METHOD appears in that
  decision.** The boot method is a presumption like any other — a design in
  which the loader hands the kernel the volume it was loaded from works under
  UEFI and not under Pi firmware, which lets the machine decide what the
  kernel is allowed to know. Peter, to the pi seat the same day: *"WTF does it
  matter what method I choose to boot? You are assuming too much."* **(3)
  Every disk driver the board has is in the default image.** A driver behind a
  knob is the same presumption one layer up — that the operator knows which
  disk they used — and it is not academic: with `sdmmc` opt-in, the
  `BlockHandle::TegraSd` variant exists only under that feature, so a plain
  image could not see the slot at all. Default-on with a named opt-out, never
  opt-in. **(4) Nothing found is a witnessed refusal, never a guess**: no
  fallback to a table of volumes the machine may not have, and the wire
  carries the refusal with its reason. **Incident**: orin 19–21 built the
  opposite and none of it flew — root bound by name to one SoC's SD slot
  (`sdmmc_root_bind`, `locate_on(TegraSd)`, `fat::mount_source(TegraSd)`,
  baton orin-22 D1); `boot_medium_verdict`, a guard asking "is the slot card
  the card I booted from?", a question only a board with exactly this slot can
  be asked (D2); this bench's card serials as kernel `const`s with
  compile-time assertions (`RENDER9_LOADER_SERIAL`, `RENDER9_CARD_ESP_VOL_ID`,
  `RENDER10_STAGED_ESP_VOL_ID`, D3); `/boot` and `/apps` bound to
  `FatBackend::new_tegra_sd` (D4); and knobs and check legs named for the
  board (`UNAOS_SDMMCROOT`, `arm-tegra-sdmmcroot`, D5). Two more of the same
  shape were caught mid-round rather than in the tree: the loader-serial
  hand-off of (2), and the opt-in driver of (3). Peter, verbatim, on why it
  matters: *"We're making an OS for computers. The Orin is one specific
  computer we are doing early development on, and you are hard-coding it in
  while we are removing hard-coding."*

## Verification

- **Verify before claiming owed** (2026-07-17). Never write an "owed /
  pending / operator must" line without first running the check that would
  falsify it. Inherited baton claims are hypotheses until re-verified.
- **No deferred verification** (2026-07-22). Owed verification (builds,
  citation checks, log reads) runs the moment it is noticed — in the
  background if long — and never surfaces as new work while the operator is
  driving something else.
- **Full-knob gate** (2026-07-22). A PASS on knob/feature-gated code requires
  (1) the gate run with every relevant knob armed, and (2) proof the code is
  in the builder-path artifact (`strings kernel.elf | grep <probe-tag>`).
  The builder has its own env→feature map that can silently drop features;
  `./arroyo check` alone proves nothing about optional features.
- **Null hypothesis is our code** (2026-07-22). Our code / boot-chain /
  sequence theories outrank hardware-, firmware-, and environment-blame
  theories by default. Bench cross-checks are proposed neutrally as
  discriminators, without a stated lean toward the hardware branch.
- **The wire may not lose lines** (2026-07-29). Serial output is the evidence
  every gate is counted from, so the transport is held to a stricter standard
  than what it reports on: a line that cannot be written is DEFERRED, and a
  line that is genuinely lost is COUNTED and announced on the wire
  (`[serial] dropped N lines`). Silent loss is forbidden — a missing `PASS`
  must never be indistinguishable from a fixture that never ran, and a
  regression's `FAIL` must never be able to evaporate. Enforced every run by
  the SERWIT-1 fixture; see
  [`docs/dev/OS/02_KERNEL_CORE/serial_transport.md`](OS/02_KERNEL_CORE/serial_transport.md).
- **A wrapped record is not a truncated one** (2026-08-31). The UEFI console
  the bootloader logs to is sometimes 80 columns wide — the loader never calls
  `SetMode`, so the width is inherited firmware state — and at 80 columns the
  firmware hard-wraps every write with a real CRLF. No bytes are lost, but a
  line-oriented read loses the tail, so `awk '/pattern/'` reports a witness
  that is present as a witness that was cut off. Orin 11 spent a session on
  an identity line that appeared to end at the word `max_vaddr` while the
  value was on the wire throughout. Read bootloader-window captures through
  `~/unaos-bench/tools/unwrap80.sh` (bench-side, outside the repo)
  — it is a no-op on a wide-console capture, so there is no cost to always
  using it. The image-identity witness itself is held under 80 columns so it
  never needs the tool; see
  [`docs/dev/OS/01_BOOT_HAL/bootloader_spec.md`](OS/01_BOOT_HAL/bootloader_spec.md) §4.
- **A flake is an observation, not a re-run** (2026-08-18). An intermittently
  red gate is diagnosed against the fixture-flake corpus —
  [`docs/dev/FIXTURE_FLAKES.md`](FIXTURE_FLAKES.md) — before it is re-run:
  match the witness text, capture what the entry asks for, then re-run. New
  classes are recorded there rather than carried in session memory.
- **Default-quiet boot** (2026-07-18). Confirmed test families are not
  re-run on default boots; batteries live behind knobs (QEMU gates arm
  them). Gate, never delete.
- **A spec must be look-around free — `foreman` refuses the dialect and its
  preflight is all-or-nothing** (pi 8, source text `pi4-regression.spec:626-629`
  at `hw-pi4 059e04db`). Rust's `regex` rejects `(?=…) (?!…) (?<=…) (?<!…)` and
  backreferences, so one such pattern in a spec makes `preflight_spec`
  (`tools/foreman/src/main.rs:114`, ahead of `parse_spec` at `:115`) abort the
  whole run with a named-line report and print no verdict table. Use the
  documented prefix-factored form instead. **The rule is recorded HERE because
  of how it was broken:** it existed only as a comment at the head of ONE arch's
  spec, riding an unlanded arc, so it was unreachable from trunk and from the
  other tracks — and the seat that violated it could not have read it. A rule
  enforced by a comment in a file only one seat can see is not a rule. Census
  2026-09-07: one non-comment look-around across eleven specs.
- **A zero-hit grep bounds only the tree you ran it in** (pi 8, 2026-09-07).
  Two seats greped the same path for the same sentence and got 1 and 0 — both
  correct, because the text rode 36 unlanded commits. Absence proven in your
  worktree is absence *there*; before calling a citation wrong, establish which
  tree it was written against.
- **"Sourced from code" is not "verified end-to-end" — reading a function is a
  citation too** (pi 8's formulation, orin 19's error, rmbp 15's catch;
  2026-09-07). A seat could not resolve a citation, went to the source, read
  `parse_spec_bytes` and correctly found that one bad pattern aborts before the
  builtin forbids are installed — then reported the consequence as silent
  vacuum. It is not: the caller preflights first and refuses loudly. The
  function was read; the PATH FROM ENTRY POINT TO BEHAVIOUR was not. Going to
  the source is right and does not by itself settle severity: ask "could this
  path have been reached", not only "is this sourced". One
  `grep -n 'parse_spec' main.rs` would have answered it and nobody ran it.
  ⚠ Record this one as the COUNTER-EXAMPLE it is — an instrument built not to
  fail silently (`457ed7c5` SPECFLIGHT), which worked. A ledger that collects
  only failures teaches a fleet the wrong thing about its own instruments.
- **A wrapper that swallows the exit status is a gate that cannot fire**
  (2026-09-07, pi 8's near-miss, caught by them before it went out). A pipe
  reports the LAST stage's status: `cmd --check ... | head` yields `head`'s
  exit, so `cmd ... | head && echo APPLIES CLEAN` prints APPLIES CLEAN over a
  `git apply` that failed. pi 8 sent themselves that green, caught it only by
  re-running for the exit code alone, and reported the method error beside the
  result. **The defect class is identical to a check whose pattern can never
  match** — in both, the reading is produced by the harness rather than by the
  thing under test, and both read as success. Any pipeline whose verdict is a
  claim about an earlier stage must capture THAT stage's status (`PIPESTATUS`,
  a temp file, or no pipe at all); a sentence of the form "X applies" / "X
  passes" is worth exactly the exit code it was read from, and if you cannot
  name that exit code you do not have the claim.
- **Require a PROPERTY, never a LIMITATION** (pi 9's nomination, adopted by
  orin 22, 2026-09-08). A spec row exists to make a defect impossible to ship
  green. A REQUIRE keyed on a line the system emits only when it FAILS inverts
  that: the row makes the gate the defect's advocate. Green then certifies the
  limitation, the floor number counts it as coverage, and the fix that removes
  the limitation must argue its way past a green gate and a floor decrement in
  order to delete its own tripwire. The instance was caught before it was
  written. While the Pi was expected to come out with no root, pi 9's grant to
  the orin 22 root arc asked for a `REQUIRE` on `[vfs] root -> NONE reason=…`
  and moved the Pi floor 120 → 121; when the mechanism changed the same hour
  and the Pi was expected to mount after all, the condition was withdrawn and
  the floor stayed 120. The discriminator is one question — **what does this
  row certify when it is GREEN?** If the answer is "the system is limited",
  the row is upside down. The failure mode belongs to a FORBID, and the shape
  that gets both is already in the tree: `pi4-regression.spec:2032` REQUIREs
  `PASS` OR a stated skip, and at `:2052` the row
  `FORBID aclsym: .*vfs\.aclsym skipped`
  closes the skip arm (the eighth directive, `42eb2736`). REQUIRE
  catches the family disappearing; FORBID catches it taking the wrong arm;
  neither substitutes, and neither requires the defect.
- **The early-stop terminus kills everything below it** (orin 22's DISPOSE
  survey, 2026-09-08; the same shape as pi 8's bit-3 witness the day before).
  On a `tegra` image `kernel_main` diverges into `tegra_early_stop(boot_info)`
  at `unaos/crates/kernel/src/main.rs:190`, and that function returns `!`
  (declared at `:2029`), so EVERY un-gated line below the call is dead on that
  board while compiling clean, type-checking on both arches, and passing every
  QEMU leg — `tegra` is off in every QEMU build, so no gate this fleet runs by
  default can observe the difference. The measured instance: `main.rs:263`'s
  boot-volume-serial publish, taken to be the kernel's one source for the
  loader's volume, sits below the divergence AND is `x86_64`-gated, so on the
  Orin it ran zero times; a second publish had been folded into
  `tegra_early_stop` in `72e2ecff` for exactly this reason, and a brief
  written from the first site alone was wrong. The same terminus is why the
  Orin runs no U-series battery at all (~47 legs, baton orin-22 C8) — a port,
  not a knob. Note that the call site is ALREADY commented as unreachable
  (`main.rs:185-188`) and was missed anyway, so the comment is not the
  control. **A change to the shared entry path names the terminus above it and
  proves reachability on the DIVERGING board by `strings` on the artifact that
  will be flashed AND by a line on that board's wire — never by a `cfg` that
  reads correctly, and never by a green `check`.**

## Bench and media

- **Flash staging** (2026-07-15). No path under any `target/` is ever handed
  off as a flash source — `target/` is shared scratch and concurrent builds
  clobber it within minutes. Bench media is copied to
  `~/unaos-bench/flash/<platform>/<artifact>-<UTCstamp>-<git7>.<ext>` with a
  MANIFEST line (sha256, branch@commit, session, knobs), re-hashed after the
  copy, and the staged path + sha is what gets handed off. Full rule:
  `~/unaos-bench/flash/README.md`.
- **Bench process is standing** (2026-07-19). Every metal session executes
  the bench-process file of record at pickup, unprompted (bench-state scan,
  capture verification, card-watch armed). Batons carry arc content only.
- **The operator owns the sitting** (2026-07-16). The runbook schedule bounds
  the evidence, not the bench session. Capture stays armed between tests;
  teardown happens only when the operator ends the bench.
- **Check, don't ask** (2026-07-16). At the bench, state that a one-second
  command can answer (`ls /Volumes/`, `lsof <dev>`) is checked, not asked.
  Mid-sitting replies are one line.
- **Tight-loop standing approval** (2026-07-19). Within a metal sitting, the
  loop is the approval: fix arcs for observed divergences, knob-gated
  diagnostics, and the obvious next rung of a just-proven line are spawned
  without re-asking. Destructive-media boots and genuinely new lanes still
  need a fresh explicit go.

## Throughput

- **Work the jobs — idleness is the failure state** (2026-08-19, Peter,
  recurring). A baton's named arcs spawn in the seat's first turn; the
  baton's assignment is the go. At every turn end, if running executors are
  below the floor (3, up to 6 for Pi/Orin benches) while undone work exists
  anywhere (baton-named arcs → verdicts to fold → lens follow-ups → owed
  list → `wip/` → queue), the seat spawns to the floor before replying.
  A question pending with the operator blocks only its dependent work,
  never the rest of the floor. "Standing by", "awaiting your go", and
  equivalents are banned phrases — each is itself the violation. An empty
  floor is legitimate only when proven that turn (quote the exhausted
  queue/owed list) or under the operator's explicit hold.

## Arcs and executors

- **The brief is the only channel into a running executor** (orin 22,
  2026-09-08). There is no message path into an executor once it is running,
  so a brief that turns out to be wrong cannot be amended — it can only be
  replaced, and everything the executor has done to that point is spend with
  no product. It cost three restarts of one arc inside a single hour:
  BOOTROOT v1 killed at ~4 minutes when a peer's lane grant arrived carrying
  conditions; v2 stopped when the mechanism itself changed (a ruling that the
  boot method may not be load-bearing, plus DISPOSE finding the brief's
  central call site dead on the board — the terminus rule above); v3 stopped
  in setup when a peer's blocker landed (the driver was opt-in, so the plain
  image could not see the disk). Every one of those facts existed and was
  reachable BEFORE the spawn, and not one of them was found by the executor.
  Three rules follow. **(1) Close the peer round first.** Before spawning any
  executor that touches a shared file or carries a design decision, the grants
  AND the facts those peers hold are settled; a grant with conditions is not
  closed until its conditions are in the brief. **(2) Batch the peer asks into
  the same turn as the read-first, and spawn in the next.** This does not
  suspend the Throughput floor and is not a licence to stand by — the arcs
  whose briefs depend on nothing pending spawn in the first turn as always;
  only the design-bearing one waits, and it waits one turn, not a round.
  **(3) Give the brief a written amendment channel.** The brief names a
  `BRIEF-AMENDMENT.md` in the executor's own scratch directory
  (`~/unaos-bench/scratch/<arc>/<exec>/`) and instructs the executor to
  re-read it at every commit boundary. It does not buy a stop-free round — an
  executor already past the boundary an amendment invalidates still has to be
  replaced — but it turns the cheap corrections into edits and leaves the
  restarts for the ones that are genuinely structural.

## Coordination

- **Wire beats comment** (rmbp 16, orin 22, 2026-09-08). A source comment that
  asserts a fact about the MACHINE is a claim like any other, but unlike code
  it is never re-executed: it ages silently and reads with the authority of
  the file it sits in.
  `unaos/crates/kernel/src/arch/aarch64/sdmmc_tegra.rs:36-37`, in the recon
  driver's list of documented vendor-quirk assumptions, states that "the
  bootloader read the card to boot". For the slot card on the render9 flight
  that is false — the loader's volume serial is not that card's, and the boot
  medium that flight was USB. rmbp 16 built a blocker premise on the comment,
  was shown the render9 wire line, and withdrew the premise in one message,
  inside an hour of the comment first misleading a seat. Two rules. **A
  comment that asserts a machine fact carries the wire line or capture that
  proved it, or says `unverified`** — and its repair is scheduled the turn the
  comment is found wrong, by the arc that found it. **In a disagreement
  between prose and a capture, the capture settles it**, and the seat holding
  the capture quotes the line rather than summarising it; this one closed in a
  single exchange because the line itself was sent.

## Code and history

- **Never trash code** (2026-07-16). Code is judged on its merits — wrong,
  broken, or refuted is trash; stopped, superseded, or unfinished is an
  asset. Archive and catalog with disposition "available for reuse".
- **Never `git stash`** (2026-07-05). The four worktrees share one object
  store and the stash stack is global; concurrent sessions race it. Use
  `git show`, scratch checkouts, or throwaway worktrees for A/B baselines.
- **Durability** (2026-07-17). Work is durable only once its branch is on
  origin. Full push line (all branches) after every landing; feature branches
  backed up periodically; WIP committed before any handoff.
- **Landing-merge shape check** (pi 6, 2026-09-05, at LANDING-2 `d11cd56e`). After every
  `--no-ff` landing, prove two facts with commands, in this order: (1) two parents —
  `git log --pretty=%p -1 <merge>` prints the trunk tip AND the arc tip (a `checkout -b` during a
  conflicted merge once dropped `MERGE_HEAD` and left trunk on a single-parent commit; the next
  sync re-conflicted 386 commits); (2) `git diff <arc-tip> <merge> | wc -l` = 0 is SAFE **if and
  only if** `git log --no-merges --oneline <merge-base>..<trunk-tip>` is EMPTY — trunk contributed no
  original work since the base. Without (2)'s second command, a zero diff against a non-ancestor
  parent is indistinguishable from wholesale loss of trunk-only content. Quote both in the landing
  report.

Operational trap details (serial-log handling, media clobbers, fixture
state, TCC, port collisions) live in the session-memory hazards ledger.

## Ledgers — one per arch, one over-arching (Peter, 2026-09-05) — gated by GATE-LEDGER (`unaos/scripts/ledger-check.sh`, rmbp e693056a; go-red by tree mutation, nine states)

- **Audits and inventories are high value. Re-derivation is the waste.** Every finding lands on
  exactly one list the turn it is found: the arch ledger (`docs/dev/OS/<track>-ledger.md`) when it
  lives in that arch's lane, `docs/dev/LEDGER.md` when it lives in a shared file, affects more than
  one board, or is a gate/process rule.
- **The arc that fixes, flies, or drops an item ticks it in the same commit** (SECURITY.md's rule).
- **Every audit or inventory is briefed with the ledger** and reports only what is NEW or CHANGED.
  An audit that re-finds known items was mis-briefed.
- A seat that finds something in another lane records it on `LEDGER.md` with the owner AND messages
  that seat the same turn (see COORDINATION).

