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
- **A default-quiet knob has two polarities and the gate must compile the one
  that SHIPS** (orin 20, 2026-09-07). `witness` is armed for exactly the four
  battery commands (`unaos/arroyo:44`) and left OFF for every boot/media
  command, so `esp-jetson`, `esp-arm`, `esp-x86`, `kernel8` and `vm-image` all
  build witness-FREE — and until this arc all 47 board legs of
  `KERNEL_CFG_MATRIX` carried it ON. Nothing anywhere compiled a BOARD feature
  set (`tegra`, `pi`, `baremetal`, `tegra_el0`, `bsptick`, `bsprun`) with the
  knob OFF, which is every configuration that reaches a card. It is not enough
  that *some* leg is witness-free: five derived `x86-mix-N` legs and both
  default legs in `check_both` already were, but they are x86 or carry no board
  feature, and the arm-only board features are dropped from
  `x86_cfg_universe` by construction — so the coverage read as present and was
  absent where it mattered. **The generalisation: for a knob whose OFF state is
  the shipped state, coverage of the ON state is coverage of a build nobody
  boots.** Read the polarity, not the leg count — `./arroyo check` now prints
  the census (ON/OFF split by arch, plus the witness-free legs by name) so a
  future gap is a line in the log instead of a near-miss. This is the sibling of
  the **Full-knob gate** rule above, running the other way: that one says an
  ARMED knob needs the gate run armed; this one says a DEFAULT-OFF knob needs a
  leg that compiles it off *with the board*. Both were paid for the same way —
  orin 19's BATTERY1S1 would have shipped a link with eight undefined symbols
  on `UNAOS_TEGRA_EL0=1 ./arroyo esp-jetson`, and a reader in review caught it,
  not an instrument.

## Gates

- **QEMU verbs exit at COMPLETION + GRACE, not at a wall; the DONE gate keeps
  the wall** (Peter, 2026-09-08; orin 22). Every QEMU verb in `unaos/arroyo`
  used to sit out a blind `sleep`, and no verb was special about it — measured
  on this box, three `kernel8-test 300` runs were over at +11.0 / +45.3 / +13.7 s
  and then idled 85–96% of the QEMU span (orin 21 buildperf §2). All of them now
  run through `qemu_wait_or_complete`, which stops when the verb's OWN checker
  says the run finished — mbench's shipped `Matcher.complete()` predicate over
  the spec that verb already replays, never an invented marker — then holds
  `UNAOS_QEMU_GRACE` (default 20 s, the measured load spread) with every FORBID
  still live, and never exceeds the verb's `secs` in either mode. **A verb with
  no declared completion source pays the full wall, and every non-completing
  outcome pays it too** — `nosignal`, a cap reached, a broken waiter. A gate that
  cannot say when a run finished must never shorten it; that branch shipping
  wrong for one afternoon is what this clause is made of.
  **`UNAOS_QEMU_FULL=1` restores the whole wall and is the form an arc's DONE
  gate runs.**
- **A fast capture is sound for pass/fail and is a FLOOR for anything monotonic**
  (same ruling). Completion means every REQUIRE and COUNT has already landed, so
  a fast run cannot be short of a witness and cannot shorten a failing run at all.
  What it does drop is TIME: an accumulator's high-water value (`[u7stk] hw=` and
  its kind) read from a fast capture is a lower bound, not a final value, and a
  periodic instrument's soak shrinks with it (`[pstrip] rollup` fires once per
  10 s — 28 windows on a 300 s wall, 2–3 after a graced exit). Measure
  accumulators and soaks under `UNAOS_QEMU_FULL=1` only. **The pair that makes
  this concrete: a fault emitted INSIDE the grace reds both modes; a fault
  emitted BEYOND it reds only the full wall.** That second case is hidden by
  design and is the whole price of fast mode.
- **A harness never writes into its own evidence; it writes BESIDE it** (Peter's
  amendment, 2026-09-08; orin 22). The run stamp naming a capture's mode does not
  go into the serial log — that log is the thing mbench and `scan_serial_faults`
  then judge, and a harness line in it could match a FORBID (reddening healthy
  runs) or a REQUIRE/COMPLETE (satisfying a witness the guest never printed). The
  first shape of this arc did append a trailer and proposed a gate to keep it
  inert; the gate was the tell. `arroyo` writes `<logfile>.run` instead, the log
  stays pure guest bytes, and no spec author ever has to think about it.
- **A capture's mode is read THREE-VALUED: fast, full, or unknown** (same
  amendment). `unknown` covers absent, unreadable, malformed **and stale**, it
  must be sayable in a verdict line (`[mode unknown: …]`), and **every consumer
  treats unknown as NOT-FULL** and refuses to certify a tail clean or read a final
  accumulator off that capture. A reader that infers "not fast, therefore full"
  has collapsed the third value in the unsafe direction. Staleness is detected,
  not assumed away: the sidecar carries the log's byte length and sha256,
  recorded **after QEMU exited and was `wait`ed, immediately before the replay** —
  written at the exit decision instead, it would mismatch on every run (late
  flush, teardown) and a check that fires every time gets deleted within the week.

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

