STATUS: APPROVED (2026-07-25, coordinator GR4). No amendments — pulse
sequence, unchanged recon+probe, writes confined as briefed. All brief
gates apply.

# PROPOSAL: kepler-fence pull 22 - PGRAPH reset pulse

## Context
From sitting #24, enabling PGRAPH via PMC_ENABLE bit 12 left the Falcon memory ports fully gated (returning BADF1000). A plain enable on an engine disabled at power-on may leave it behind a second gate. We will test standard reset-then-enable initialization discipline by pulsing the PMC bit.

## Implementation Plan
Lane: `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.

1. **PGRAPH Reset Pulse**: Replace the plain PGRAPH enable with a reset pulse sequence.
   - Read `PMC_ENABLE` and log: `:: kepler: pgraph-pulse pre=XXXXXXXX ::`
   - Write `PMC_ENABLE` with bit 12 *cleared*.
   - Read `PMC_ENABLE` and log: `:: kepler: pgraph-pulse off rb=XXXXXXXX ::`
   - Spin-loop for ~100 ms to settle.
   - Write `PMC_ENABLE` with bit 12 *set*.
   - Read `PMC_ENABLE` and log: `:: kepler: pgraph-pulse on rb=XXXXXXXX ::`
   - Spin-loop for ~100 ms to settle again.
2. **Unchanged Recon & Probe**:
   - Re-run the pull-18 Falcon core recon (both passes) completely unchanged.
   - Re-run the pull-21 IMEM/DMEM sentinel probe completely unchanged.
3. **Unchanged Witness Rematch**:
   - Leave the `witness-rematch` block as landed to provide a baseline check on the fence wall.
4. **Writes Confined**:
   - Only `PMC_ENABLE` and the 4 memory ports are written to. Zero execution (no CPUCTL/BOOTVEC).

## Compliance Gates
* ZERO execution of the falcon engine.
* Run syntax/build check: `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo check`
* Verify cleanly on default `./arroyo test` and `./arroyo test-arm`.
* Builder path build: `UNAOS_USBDEBUG=1 UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo esp-x86`
* Strings proof check: verify the new `pgraph-pulse` markers are in `target/x86_64_esp/kernel.elf`.
* Clean working tree with scratch files deleted.
* Commit ALL docs+code; no push. Report "PUSH OWED: 8" (incremented).
