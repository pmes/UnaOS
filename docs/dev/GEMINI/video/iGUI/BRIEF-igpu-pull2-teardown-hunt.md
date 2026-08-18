STATUS: BRIEF — awaiting Gemini proposal (`PROPOSAL-igpu-pull2.md`, this directory)

# BRIEF — iGPU pull 2: who tears down scanout? (teardown hunt)

Coordinator-authored (2026-07-22, post sitting #6, Peter ruled: hunt the
teardown first; from-scratch modeset only if this exonerates our code).

## The question

Sitting #6 boot 1b/2b: at probe time ALL iGPU pipes/planes are disabled
(`CONF=0`, `CNTR=0`, `SURF=0`, `DP_A=0x1C`) and Kepler's heads are equally
dead — yet GOP lit the panel to boot-pick us, and the panel goes black almost
instantly on every boot. SOMETHING disables scanout between "GOP done" and
"our probe runs". Per the null-hypothesis law, our boot chain is the prime
suspect; firmware teardown at ExitBootServices is the alternative.

## What pull 2 does (read-only, three-point trace)

Dump the minimal scanout state — `PIPEACONF/B/C` enable bit, `DSPACNTR/B/C`
enable bit, `DSPASURF`, `DP_A` — at three points:
1. **Bootloader, BEFORE ExitBootServices** (GOP still owner). Expect live.
2. **Bootloader, immediately AFTER ExitBootServices**, before any kernel
   handoff work. Live here = firmware innocent.
3. **Kernel, igpu.rs probe** (already exists — the sitting-#6 baseline).

Deltas localize the killer: dead 1→2 = firmware at EBS; dead 2→3 = OUR early
boot (then bisect: identify which of our init steps between handoff and the
probe touches the iGPU's world — candidate suspects to enumerate in the
proposal: fb console writes to 0x90020000, PCI BAR/command-register writes on
00:02.0, MMIO window remaps covering the GTT range).

## Proposal must state

- Exactly where in the bootloader the two dump points go and how output gets
  to serial at each (pre-EBS can use UEFI console; post-EBS needs our serial).
- The precise register list (keep it to the ~8 regs above; cite the IVB PRM).
- Confirmation the pass is read-only (no fixes in this pull — localize first).
- Lane note: this touches the x86 bootloader path — flag the exact files in
  the proposal so the integrator can clear the lane before code.

## Standing rules

Read-only. Full-knob land-review law (gate with `UNAOS_IVB` armed +
strings-proof in builder-path kernel.elf — note the bootloader dumps must be
proven present in BOOTX64.EFI too, not just kernel.elf). Metal owed:
sitting #7 rides with kepler pull 8. Cleanroom: IVB PRM citations only.
