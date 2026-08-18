STATUS: APPROVED (2026-07-25, coordinator GR4). No amendments — sentinel
probe at both real bases with correct AINCW/AINCR discipline, zero
execution. (Note: the old-base probes and dense fal-base dumps are
gated-off since the s26 ring trim — "as landed" means they stay gated;
do not re-enable them.)

# PROPOSAL: kepler-fence pull 24 - Sentinel port probe at real Falcon bases

## Context
From sitting #26, the real Falcon bases for GK107 context switching have been found: FECS at `0x409000` and GPCCS at `0x41A000`. Both returned valid live state bits (`cpuctl=00000010`, likely halted). We will now retarget the pull-21 sentinel probe to these two valid bases to prove the memory upload paths (IMEM and DMEM) before advancing to from-scratch microcode execution.

## Implementation Plan
Lane: `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.

1. **Keep Existing Baseline**:
   - The PGRAPH reset pulse, old 0x400100/0x400180 core probes, new 0x409000/0x41A000 base recons, and the witness rematch block stay exactly as landed.

2. **Retargeted Sentinel Probe**:
   - Inside the loop over the two bases (`0x409000` and `0x41A000`), after the read-only recon prints, perform the sentinel port probe.
   - **IMEM Probe**:
     - Write `base+0x180` (IMEMC) = `1 << 24` (AINCW - auto-increment on writes, offset 0).
     - Readback IMEMC and log: `:: kepler: fal-port b=XXXXXX imemc wr=01000000 rb=XXXXXXXX ::`
     - Write 4 sentinels to `base+0x184` (IMEMD): `0xDEADBEEF, 0xCAFEF00D, 0x12345678, 0xA5A55A5A`.
     - Write `base+0x180` (IMEMC) = `1 << 25` (AINCR - auto-increment on reads, offset 0).
     - Read IMEMD 4 times and log: `:: kepler: fal-port b=XXXXXX imem rb w0=XXXXXXXX w1=XXXXXXXX w2=XXXXXXXX w3=XXXXXXXX ::`
   - **DMEM Probe**:
     - Write `base+0x1C0` (DMEMC) = `1 << 24` (AINCW).
     - Readback DMEMC and log: `:: kepler: fal-port b=XXXXXX dmemc wr=01000000 rb=XXXXXXXX ::`
     - Write 4 sentinels to `base+0x1C4` (DMEMD): `0xDEADBEEF, 0xCAFEF00D, 0x12345678, 0xA5A55A5A`.
     - Write `base+0x1C0` (DMEMC) = `1 << 25` (AINCR).
     - Read DMEMD 4 times and log: `:: kepler: fal-port b=XXXXXX dmem rb w0=XXXXXXXX w1=XXXXXXXX w2=XXXXXXXX w3=XXXXXXXX ::`

3. **Zero Execution**:
   - No writes to CPUCTL (`base+0x100`) or BOOTVEC (`base+0x104`). The Falcon remains halted. No restore is necessary.

## Compliance Gates
* ZERO execution of the falcon engine (NO CPUCTL/BOOTVEC writes).
* All docs+code committed, scratch deleted.
* No push. Report "PUSH OWED: 10" (incremented).
* (Coordinator runs all builds and gates at land-review).
