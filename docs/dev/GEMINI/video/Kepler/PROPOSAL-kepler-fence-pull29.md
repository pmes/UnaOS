STATUS: APPROVED (GR5, 2026-07-25) with FOUR BINDING AMENDMENTS:
1. PLACEMENT — the entire block runs where the relocated s32 recon ran (AFTER
   `hb final`), keeping every s30-proven read ahead of any unverified access.
   Keep the recon-pre/recon-post cpuctl control-bracket discipline: cpuctl
   before the rotated read, and the recovery re-read after the clear attempt
   doubles as recon-post. Print every value raw.
2. ERROR-CLEAR WRITES are write-back-of-observed-bits ONLY — read the
   register, print it, write back exactly the value read (W1C of what is
   actually set). Never a blanket 0xFFFFFFFF, never a constant.
3. DO NOT WRITE THE CTXCTL ENABLE BIT this pull. Step 1 is a READ of
   0x122104 only. If it reads with bit 4 clear, that is the headline fact and
   the enable-set is pull 30's one-line experiment, not an inline extra.
4. ORDER THE NEW SPACES DEFENSIVELY — read 0x122104 (PIBUS) BEFORE touching
   0x409800, and print it immediately; if PIBUS itself returns BADF-family,
   STOP the block there (skip the rotation and clear legs) and let the boot
   continue — do not risk poisoning a second unit blind. A skipped leg prints
   an explicit `:: kepler: pring skip <reason> ::` so the capture is
   self-explaining.
Everything else as proposed, including the rotation target CC_SCRATCH[0]
(0x409800) as the boot's one clean datum.

# PROPOSAL: kepler-fence pull 29 - PRING Poison, Subunit Gating & Truth Recovery

## Context
In s31/s32, our read of `WRCMD_CMD` (`0x409504`) faulted (`BADF1000`), immediately poisoning all subsequent reads of the FECS unit for the remainder of the boot. The coordinator challenged us to explain *why* `0x409504` faults on GK107 (when it works on GK104), how to un-wedge the unit, and how to recover the ground truth for our six target offsets without wasting 6 boots.

## The Theory: Falcon Subunit Clock Gating (`CTXCTL`)
The Falcon MMIO space is partitioned: `0x000..0x3FF` is the base microprocessor (which we proved works via `cpuctl` and mailboxes), while `0x400..0xFFF` belongs to the `CTXCTL` (Context Control) subunit. 
According to `envytools` (`rnndb/bus/pibus.xml`), there are explicit enable bits for these subunits in the `PIBUS` (`0x120000`) space. Specifically, `PIBUS_MMIO_HUB_ENABLE1` (`0x120D04` broadcast, or `0x122104` unicast) contains a `CTXCTL` enable at bit 4. 
**Hypothesis:** On GK107, the BIOS/bootloader does not enable the `CTXCTL` subunit by default. Any access to `0x400+` hits a disabled target, yielding a PRING `badf1000` ("target refused transaction", per `docs/hw/mmio.rst`) and wedging the `PIBUS` connection to that unit. GK104 likely has it enabled by default, or Nouveau explicitly enables it earlier.

## The Theory: PRING Error Clearing
`docs/hw/mmio.rst` dictates that PRING errors trigger a `PBUS` interrupt. The `PIBUS` controller (`0x120000`) has interrupt/fault reporting registers for this ring: `INTR_ADDR` (`0x120120`), `INTR_VALUE` (`0x120124`), and `INTR` (`0x120128`). Furthermore, `PBUS_INTR` (`0x1100`) catches `MMIO_RING_ERR` at bit 2. We can read these to observe the trapped fault, and write 1s to clear them, potentially un-wedging the unit.

## Implementation Plan (Pull 29)
We will combine directions (a), (b), and (c) into a single, cleanroom, read/clear probe:

1. **Pre-Probe Enable Check:** Read `PIBUS_MMIO_HUB_ENABLE1` (`0x122104`) to definitively check if `CTXCTL` (bit 4) is enabled on GK107.
2. **Offset Rotation (The 1-per-boot fallback):** We will change the first offset we read to one of the six remaining targets (e.g., `CC_SCRATCH[0]` at `0x409800`). If it faults, we know the *entire* `0x400+` space is disabled, proving the gating hypothesis. 
3. **The Error Clear & Un-Wedge:** After the first faulting read, we will read `PIBUS INTR_ADDR` (`0x120120`) and `PBUS INTR` (`0x1100`) to observe the recorded fault. We will then write back to them to clear the error.
4. **The Recovery Test:** We will re-read a known-good register (like `cpuctl` `0x409100`) to see if the unit has successfully un-wedged. 

This plan gets us the exact reason for the fault, attempts a live recovery of the unit, and rotates our target to preserve the 1-datum-per-boot minimum. No writes to the FECS unit itself, only to the `PBUS/PIBUS` error-clear registers.

## Deliverables
* Update `kepler.rs` with the `PIBUS` enable check, the offset rotation, and the error-clear sequence.
* Wait for approval before implementation.
