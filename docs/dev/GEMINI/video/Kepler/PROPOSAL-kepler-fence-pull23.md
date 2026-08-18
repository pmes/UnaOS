STATUS: APPROVED (2026-07-25, coordinator GR4). No amendments — read-only
two-pass recon of both cited bases, zero writes, baselines retained,
envytools citation present. All brief gates apply.

# PROPOSAL: kepler-fence pull 23 - FECS/GPCCS base recon

## Context
From sitting #25, the reset pulse was electrically clean but the 0x400100 Falcon registers remained at BADF1000. This indicates the registers themselves likely do not exist at this base on GK107. According to envytools hwdocs (e.g., GF100+ graphics architecture), the PGRAPH context-switch logic is split into two distinct Falcons: FECS (Front End Context Switch) at `0x409000` and GPCCS (Graphics Processing Cluster Context Switch) at `0x41A000`. We will perform a read-only reconnaissance of these two candidate bases to find the real Falcon.

## Implementation Plan
Lane: `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.

1. **Keep Existing Baseline**: 
   - The PGRAPH reset pulse (`0x000200` PMC_ENABLE bit 12) stays exactly as landed.
   - The original `0x400100` probes and the witness rematch block stay as landed (serving as our baseline).

2. **Read-Only Falcon Base Recon**:
   - Right after the pulse and settle, perform a two-pass dense dump (with the standard ~100 ms spin-loop between passes).
   - For each base in `{0x409000, 0x41A000}`:
     - Read offsets `0x000` through `0x1FC` (step 4).
     - Log: `:: kepler: fal-base b=XXXXXX off=XXX val=XXXXXXXX{abs} ::` (where `{abs}` is `" ABSENT?"` for FFFFFFFF/BAD0xxxx). For pass 2, use `fal-base2`.
     - Log summary verdict line: `:: kepler: fal-base b=XXXXXX verdict cpuctl=XXXXXXXX imemc=XXXXXXXX dmemc=XXXXXXXX ::` (reading base+0x100, base+0x180, base+0x1C0).

3. **Zero New Writes**:
   - We will not propose any new writes to these new bases (no sentinel port probes yet). If the registers return real values, pull 24 will move the sentinel probe there.

## Compliance Gates
* ZERO new writes (read-only recon).
* Run syntax/build check: `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo check`
* Verify cleanly on default `./arroyo test` and `./arroyo test-arm`.
* Builder path build: `UNAOS_USBDEBUG=1 UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo esp-x86`
* Strings proof check: verify the new `fal-base` markers are in `target/x86_64_esp/kernel.elf`.
* Clean working tree with scratch files deleted.
* Commit ALL docs+code; no push. Report "PUSH OWED: 9" (incremented).
