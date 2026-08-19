# LAWS.md — standing operational laws

Durable process laws for all UnaOS sessions, moved here from session memory so
they are versioned and reviewable. Session memory keeps only pointers.
`CLAUDE.md` covers layout, lanes, and arc discipline; this file records the
laws minted at the bench and at the seat. Each entry names its origin date.

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
- **A flake is an observation, not a re-run** (2026-08-18). An intermittently
  red gate is diagnosed against the fixture-flake corpus —
  [`docs/dev/FIXTURE_FLAKES.md`](FIXTURE_FLAKES.md) — before it is re-run:
  match the witness text, capture what the entry asks for, then re-run. New
  classes are recorded there rather than carried in session memory.
- **Default-quiet boot** (2026-07-18). Confirmed test families are not
  re-run on default boots; batteries live behind knobs (QEMU gates arm
  them). Gate, never delete.

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

Operational trap details (serial-log handling, media clobbers, fixture
state, TCC, port collisions) live in the session-memory hazards ledger.
