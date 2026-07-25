STATUS: PROPOSED

# PROPOSAL: kepler-fence pull 21 - K-GPU-4 milestone 1 — Falcon IMEM/DMEM access probe

## Context
From sitting #23, PGRAPH enablement (PMC_ENABLE bit 12) left the fence wall intact with an identical signature, refuting the engine-off theory. The K-GPU-4 cleanroom arc begins. 

Before we can upload from-scratch Falcon microcode, we must verify if the IMEM and DMEM ports (0x400180/184 and 0x4001C0/1C4) are actually writable when the engine is powered but uninitialized.

## Implementation Plan
Lane: `unaos/crates/kernel/src/drivers/gpu/kepler.rs` ONLY.

1. **Keep existing witness rematch**: Leave the pull 20 witness rematch block in place to provide a baseline check every boot.
2. **IMEM Probe**:
   - Write `IMEMC` (0x400180) = `0 | (1 << 24)` (auto-increment enabled, offset 0).
   - Readback `IMEMC` and log: `:: kepler: falcon imemc wr=01000000 rb=XXXXXXXX ::`
   - Write 4 sentinel words to `IMEMD` (0x400184): `0xDEADBEEF, 0xCAFEF00D, 0x12345678, 0xA5A55A5A`.
   - Re-write `IMEMC` = `0 | (1 << 24)` to reset the offset.
   - Read `IMEMD` back 4 times and log: `:: kepler: falcon imem rb w0=XXXXXXXX w1=XXXXXXXX w2=XXXXXXXX w3=XXXXXXXX ::`
3. **DMEM Probe**:
   - Same sequence, using `DMEMC` (0x4001C0) and `DMEMD` (0x4001C4).
   - Log: `:: kepler: falcon dmemc wr=01000000 rb=XXXXXXXX ::`
   - Log: `:: kepler: falcon dmem rb w0=... w1=... w2=... w3=... ::`
4. **Zero Execution**:
   - No writes to `CPUCTL` (0x400100) or `BOOTVEC` (0x400104). The Falcon stays halted.
5. **No Restore**:
   - These are engine-local scratch writes. No restore is necessary.

## Compliance Gates
* ZERO execution of the falcon engine.
* Run syntax/build check: `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo check`
* Verify cleanly on default `./arroyo test` and `./arroyo test-arm`.
* Builder path build: `UNAOS_USBDEBUG=1 UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo esp-x86`
* Strings proof check: verify the new `falcon imem` and `falcon dmem` markers are in `target/x86_64_esp/kernel.elf`.
* Clean working tree with scratch files deleted.
* Commit ALL docs+code; no push. Report "PUSH OWED: 7" (incremented).
