STATUS: APPROVED (2026-07-25, coordinator GR4). No amendments — matches the
brief exactly, zero new writes, gates correct. Keep the original per-step
witness markers unchanged between the framing pair, per the brief.

# PROPOSAL: kepler-fence pull 20 - witness-ladder rematch against a live engine

## Context
From sitting #22, PGRAPH power-on (PMC_ENABLE bit 12) took successfully, putting the engine on the pri bus. We will now test the standing fence-wall theory by checking if PFIFO stops stripping VALID/POLL when the target engine is powered on.

## Implementation Plan
Lane: `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.

1. **PGRAPH Enable First**: Keep the pull 19 PGRAPH enable (and its settle loops) exactly as landed, ensuring it runs *before* the witness sequence so PGRAPH is on.
2. **Witness Rematch**: Re-run the established witness sequence verbatim (channel/RAMFC setup, runlist submit, VALID/POLL write, err/stat/discriminator reads) as it ran in s7-s10.
3. **Framing Markers**:
   * Before the sequence: `:: kepler: witness-rematch begin (pgraph on) ::`
   * After the sequence: `:: kepler: witness-rematch end err=X stat=X valid=X ::` (with exact readbacks replacing X).
4. **Zero New Register Writes**: We will only resequence/re-enable the existing witness sequence without introducing any new register writes. No restore of PMC bit 12.

## Compliance Gates
* ZERO new register writes.
* Run syntax/build check: `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo check`
* Verify cleanly on default `./arroyo test` and `./arroyo test-arm`.
* Builder path build: `UNAOS_USBDEBUG=1 UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo esp-x86`
* Strings proof check: verify the two new `witness-rematch` markers are in `target/x86_64_esp/kernel.elf`.
* Clean working tree with scratch files deleted.
* Commit ALL docs+code; no push. Report "PUSH OWED: 6" (incremented).
