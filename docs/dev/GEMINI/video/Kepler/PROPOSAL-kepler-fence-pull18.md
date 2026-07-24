STATUS: PROPOSED

# Proposal: kepler-fence pull 18 — PGRAPH Falcon recon (read-only)

## Context & Objectives
The PFIFO fallback ladder has been fully exhausted (refuted) as of sitting #20. The next hypothesis is that PFIFO refuses work until the PGRAPH engine's Falcon microcontroller boots and signals readiness. This pull implements a read-only ground-truth recon of the PGRAPH and Falcon state before we attempt any microcode upload. 

## Implementation Plan
In `unaos/crates/kernel/src/drivers/gpu/kepler.rs` (likely near the end of initialization/probing, but before the fence timeout loop):

1. **Reprint PMC_ENABLE**:
   Read `NV_PMC_ENABLE` (`0x200`) again and print it, explicitly labeling it to check if PGRAPH (usually bit 12) is enabled.

2. **Falcon Core & Mem Reads (Pass 0 and Pass 1)**:
   We will execute two passes with a bounded delay (`crate::timer::delay_ms(100)`) between them to observe any autonomous ticking.

   In each pass `<P>`:
   - Read `CPUCTL` (`0x400100`) and `BOOTVEC` (`0x400104`) and print:
     `:: kepler: falcon pass<P> cpuctl={:08X} bootvec={:08X} ::`
   - Dump Falcon Core Block (`0x400100` to `0x40011C`):
     Iterate offset by 4. Print `:: kepler: falcon core off={:03X} val={:08X} ::` (appended with `ABSENT?` if `val == 0xFFFFFFFF` or `0xBAD0BA20`).
     After the loop, print `:: kepler: falcon core done rows=8 ::`.
   - Read `IMEMC` (`0x400180`) and `DMEMC` (`0x4001C0`). Print:
     `:: kepler: falcon mem imemc={:08X} dmemc={:08X} ::`
     (with `ABSENT?` tags if applicable).
   - Dump PGRAPH Status Block (`0x400000` to `0x40007C`):
     Iterate offset by 4. Print `:: kepler: pgraph stat off={:03X} val={:08X} ::` (with `ABSENT?` tags if applicable).
     After the loop, print `:: kepler: pgraph stat done rows=32 ::`.

## Gates & Compliance
- **Read-Only**: Zero new writes. All accesses to PGRAPH/Falcon registers are read-only.
- **Full-knob Check**: Run `UNAOS_IVB UNAOS_KEPLER UNAOS_KEPLER_TAKEOVER UNAOS_KEPLER_FIFO ./arroyo check` for both architectures.
- **Strings Proof**: Run `arroyo build esp-x86` and confirm `strings` proof of the new markers in `kernel.elf`.
- **Clean Tree**: Remove scratch files, ensure `git status` is clean, and commit all code and docs.
- **No Push**: Report `PUSH OWED: n`.
