# RELAY

## → igpu

Scope is decided: **A — Flight 1 grows to full IVB display bring-up; the serial link is
the debug path.** Ladder: `docs/dev/GEMINI/video/iGUI/LADDER-igpu-bringup.md`. Full fix
review: `~/unaos-bench/scratch/gr20/review-igpu-fixes.md`.

**Your Flight 1a plan: ACK with 5 amendments, then proceed.**
1. **BLOCKING — DEFECT-3 is missing from your plan.** The DDC read index: upstream
   apple-gmux reads the owner back at the WRITE index `0x28`; your `0x29` is uncited —
   read back at `0x28`, do not `#[cfg]`-gate `0x29` into permanence. Put the
   `pre-switch state: DDC= DISP= EXT=` decision line in rung 0's census (reads-only,
   rides Flight 1a free; `0xFF` at a `+1` index convicts the `+1` model).
2. Also missing: DEFECT-5 residue — stale `ms()`-deadline doc claims at igpu.rs:259 and
   286–288, duplicate `#[cfg]` at 262–263.
3. One clock: your plan moves waits to TSC but fixes the dwell on `arch::ms()` — which
   loses 13 ms/boot vs the wall. Dwell deadline on the same TSC basis; state the ~10 s
   dwell cost in the flight's prediction; gmux_igd media is a special flight, not
   regression media — say so in the RUNBOOK.
4. Your forced self-test only exercises the plain pre-image path — route one synthetic
   through the special-handler dispatch (else the PP_CONTROL/DSPACNTR handlers cannot
   fire until a real 1c failure). `LADDER highest=NN/10` prints on EVERY exit path,
   failures included, with `why=`.
5. Base: `git checkout -B wt/gmux-igd-x86 seat/gr20-igpu-rebase` (= `6d328b54`, your five
   commits on current trunk, gate 11/11), then the fixes, then Flight 1a. Work only in
   your own worktree, never the main tree. Seat re-reviews.
