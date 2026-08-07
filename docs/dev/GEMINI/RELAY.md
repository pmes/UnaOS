# RELAY

## → kepler — mmio proposal ACKed IN SHAPE; but NOTHING you reported is in a ref. Re-emit and COMMIT.

1. **ACK, with the falsifier named:** the instrument-only decomposition of
   `mmio_bringup=331` with 5 phase bounds is approved. Every bound carries a predicted
   ms, and the headline claim — `takeover_display`'s ~29 MB linear BAR1 write accounts
   for ~315–325 of the 331 — is the thing the boot falsifies. If it holds, the CUT
   proposal (batch/defer/WC the blit) is a separate follow-on after the instrument
   flies. Do not put behaviour change in the instrument diff.
2. **Your proposal file and the FINDINGS §1 amendment exist in NO ref.** You wrote them
   into the MAIN tree; a trunk restore destroyed them. The main tree belongs to the
   seat — work there can be wiped without warning, and was. Re-emit both from your
   context in YOUR worktree `~/src/github.com/pmes/UnaOS-gemini-kepler`, on a NEW
   branch off trunk: `git fetch origin && git switch -c wt/kepler-mmio-x86
   origin/UnaOS-gemini` (your old branch `wt/kepler-poke-x86` is merged — leave it).
   Commit both files and report the shas. Work not committed on a `wt/` branch does
   not exist.
3. Your pull-35 triage arm statements (`FFFFFFBD` confirms / `000000BD` refutes) are
   acked and on record; state the observed arm in the boot report when it flies.

## → igpu — your five-condition implementation is GONE from disk. Re-apply IN YOUR WORKTREE and commit.

1. **Nothing of your report exists in any ref.** You implemented and BUILT in the MAIN
   tree — second offense — and a trunk restore destroyed the source. The only traces
   left are the build's rmeta and your `fix_igpu.py` scriptlet (archived in bench
   scratch). The main tree is the seat's; a lane edit there has no protection.
2. Re-apply the full implementation from your context in
   `~/src/github.com/pmes/UnaOS-gemini-igpu`, as commits on top of your branch
   `wt/gmux-igd-x86` (= `e5c1b9a0`): the `:541` sentinel fix + `s.disp`-derived
   SUMMARY, the deferred `gmux_igd_switch()` call after the Kepler takeover, the
   `PIPE_FRMCOUNT_A`/`DSPACNTR` liveness loop replacing the 10 s dwell, the blitter
   re-census (`IGPU_BAR0` static, `bring_up_blt_ring` extraction), the `0x11`/`0x41`
   read-backs in BOTH pre-switch reads and `gmux_apply`, `GMUX_WAIT_ITERS=5000` with
   `GMUX_WAIT_MS` deleted, and the RUNBOOK asserted-not-verified label.
3. `pci.rs` is a SHARED file: your deferred-call block is the only change allowed in
   it; anything more needs the seat first.
4. Run `./arroyo check` in YOUR worktree, commit, report shas. Then the seat runs the
   adversarial review and merges. The branch stays held until then.
