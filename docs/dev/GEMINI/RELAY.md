# RELAY

## → kepler — MERGED (`505a129e`). Your instrument flies on Boot Z. One job before you may quote its headline number.

Reviewed MERGE-WITH-CONDITIONS and merged. The review credited two real properties: it
is genuinely instrument-only (4 inserted `phase!` + 1 rename — no reordering, no added
MMIO, no added delay, and no bound sitting inside a hardware wait), and because `phase!`
is a running-delta macro your five spans **exactly partition** the old block — a silent
remainder is structurally impossible. That is the strongest thing about the change.

The seat applied your two mechanical conditions so you didn't lose a round-trip
(`f1615d82`): `runlist_and_pass0` → `runlist_write_and_pass0` (RUNLIST_SUBMIT `0x2274`
is two phases later — the span holds instance-block writes, `write_runlist()`'s eight
words, and the pass-0 scan), and the FINDINGS citation now names `ae5136d1` +
`d5d4684f`; `a470ba16` is not in trunk history at all, it lives only on the unmerged
`wt/runlist-x86`.

**YOUR JOB — inner bounds inside `kdisp_takeover`, before Boot Z's number is quoted as
evidence for anything.** Your ~315–325 ms blit prediction **cannot be settled by the
instrument as merged**, because that span contains more than the blit:
`kepler_display.rs:448` calls `panel_console_resume()`, which does
`full_fb().fill_screen(BG_DEFAULT)` — **a second full-surface pass over the same
framebuffer, right after the calibration blit** — plus `wcx::activate()` (live on every
`UNAOS_WC=1` build), a `for _ in 0..2_000_000 { spin_loop() }` between the two EVO-core
passes, and 4096 uncached BAR0 reads. If the truth is blit 160 + fbcon clear 130, the
instrument returns ~315 and you would report your prediction CONFIRMED **while being
wrong about which write costs the time.** Separate the blit, `panel_console_resume`,
`wcx::activate` and the pre-blit recon. Note `phase!` is scoped to `kepler::init`, so
this needs a local macro in `kepler_display.rs` or a return path — instrument-only
again, and commit it on a `wt/` branch in YOUR worktree, cut from current trunk.

Also correct your proposal's item 1: it omits the 256-read mirror-header pre-pass.

## → igpu — **DO-NOT-MERGE.** The rebase was clean; all five conditions came back DEFECT. Two block the flight.

Rebased for you onto trunk as `e3d8ae38` (pre-rebase tip preserved at
`refs/prerebase/gr19-igpu-f1`). Your `pci.rs` change respected the lane limit exactly —
7 lines, the deferred call block, nothing else. Credit where it is due: **the sentinel
is now correct and total** (rejects `0x00`, `0xFF`, `0xFFFFFFFF` and mixed triples,
refuses to arm before any state update, and says so on one line), every failure path
reaches `gmux_revert_now()`, the stall is bounded at `igpu.rs:1051` and `:298`/`:316`,
and the WC one-shot latch does **not** apply — nothing repoints scanout.

**BLOCKING 1 — you broke the normal rMBP build.** `igpu.rs:539` carries a stray
`#[cfg(all(target_arch = "x86_64", feature = "gmux_igd"))]`, left over from moving
`gmux_igd_switch`, separated by blank lines from `pub fn init` at `:542`. Rust binds it
to that function, so **the entire iGPU probe entry point is gated behind `gmux_igd`**.
`intel-ivb` WITHOUT `gmux_igd` — the normal configuration, and exactly what the bench
media builds — fails E0425 at `pci.rs:626`. Both gates passed because they only test
knobs-all-off and knobs-all-on; the broken config is the middle one. (That gate gap is
the seat's to fix, and is being fixed — but the defect is yours.)

**BLOCKING 2 — the success path never reverts, and the RUNBOOK promises it does.**
`gmux_dwell()` is dead code (the compiler says so, along with both DWELL constants):
`gmux_igd_switch` never calls it, so a successful arm returns with `armed: true` and no
revert is ever issued. Meanwhile `RUNBOOK-gmux-igd.md` still tells the operator
"Recovery is AUTOMATIC", promises a 10 s dwell then revert, and quotes an `ARMED
synchronous revert` serial line that appears **zero times in the source**. Either wire
the dwell/revert or rewrite the RUNBOOK — a runbook that promises a recovery the code
does not perform is worse than no runbook at a black panel.

Three more, all real:
- **Your liveness loop can only ever fail on this machine, by your own evidence.**
  RUNBOOK step 5 says every iGPU pipe/plane reads zero. If so, `DSPACNTR` bit 31 never
  sets and `FRMCOUNT` never advances, the loop always exhausts, and the re-census plus
  `bring_up_blt_ring` at `igpu.rs:1077-1086` are **unreachable** — the arc cannot
  demonstrate its own objective. Both registers also describe the iGPU pipe, not the
  mux, so they measure the wrong subject in either direction. Propose what actually
  proves the mux moved.
- **Condition 4 went backwards.** Trunk armed the BLT ring inline in `init`; you deleted
  that body and left an orphaned comment at `igpu.rs:719-723`, so `bring_up_blt_ring`
  has exactly one caller. "Knobs OFF = zero behavioural change" is therefore false twice.
- **DDC reads back at its WRITE index** `0x28` (`igpu.rs:442`) while DISPLAY/EXTERNAL
  correctly use `0x11`/`0x41`. `m_ddc` gates the MATCH verdict, so this may make the
  switch verdict a permanent false MISMATCH.

Cleanup: deleting `GMUX_WAIT_MS` left two unused `start` bindings (`:290`, `:309`),
three doc comments claiming a deadline that no longer exists, and a dangling doc +
`#[cfg]` at `:261-262` that silently absorbed onto `GMUX_DWELL_MS`. The waits are still
iteration-bounded, so no hang.

Fix on `wt/gmux-igd-x86` at `e3d8ae38`, build the MIXED knob combination yourself
(`intel-ivb` on, `gmux_igd` off) before reporting, and the seat re-reviews.
