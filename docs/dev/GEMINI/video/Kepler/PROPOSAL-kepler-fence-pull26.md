STATUS: APPROVED (2026-07-25, coordinator GR4). No amendments — one write,
honest-null on refusal, baseline untouched.

# PROPOSAL: kepler-fence pull 26 - Clear DMACTL REQUIRE_CTX

## Context
In sitting #28, the execution of the first ucode was refused despite successful memory port operation, image uploading (including page 0 padding), and correct execution triggers. `CPUCTL` latched the start trigger but the core stayed stopped.
The post-sweep revealed the blocker: `DMACTL (base+0x10C)` read as `0x00000001`, where bit 0 is `REQUIRE_CTX`. The Falcon demands a bound context before it will execute. Nouveau clears this exact bit on the no-context path before starting a falcon.

## Implementation Plan
Lane: `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.

1. **Keep Existing Baseline**:
   - The PGRAPH reset pulse, old-base probes (gated), fal-base recon, fal-port probes, the image A/B loop structure, and the post-sweep remain exactly as landed.

2. **DMACTL Mask-Clear**:
   - Inserted immediately before the execution start (before `BOOTVEC=0` and `CPUCTL=2`).
   - Read `DMACTL` (`base + 0x10C`): 
     - Print `:: kepler: dmactl pre=XXXXXXXX ::`.
   - Mask-clear bit 0: write `dmactl_pre & !1` to `base + 0x10C`.
   - Readback `DMACTL`:
     - Print `:: kepler: dmactl post=XXXXXXXX ::`.
   - If bit 0 remains set (`(dmactl_post & 1) != 0`):
     - Print `:: kepler: dmactl REFUSED ::`.
     - Skip the start sequence (honest null, no fallback execution logic).

3. **Execution Restart**:
   - If `DMACTL` clears successfully, proceed to execute Image A exactly as landed.
   - Wait for the halt poll.
   - The verdict will be based on `MAILBOX0` leaving the `0xA5A50000` seed. Image B remains a conditional fallback if Image A fails for other reasons.

## Compliance Gates
* Honest null implementation: If `DMACTL` refuses to clear, the start is skipped entirely (no blind register stomping).
* Only one new write (`DMACTL`).
* All docs+code committed, scratch deleted.
* No push. Report "PUSH OWED: 12".
* (Coordinator runs all builds and gates at land-review).
