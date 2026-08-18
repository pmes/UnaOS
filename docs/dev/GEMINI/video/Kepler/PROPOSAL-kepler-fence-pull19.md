STATUS: APPROVED (2026-07-25, coordinator GR4). AMENDMENTS (binding):
(1) The strings proof runs against `target/x86_64_esp/kernel.elf` after
    `UNAOS_USBDEBUG=1 UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1
    UNAOS_KEPLER_FIFO=1 ./arroyo esp-x86` — there is no `arroyo build
    esp-x86` subcommand and no `target/x86_64-unaos` kernel path.
(2) Default `./arroyo test` + `./arroyo test-arm` green are also gates,
    per the brief. Everything else as proposed.

# PROPOSAL: kepler-fence pull 19 - PGRAPH power-on (single PMC write)

## Context
From sitting #21, we found that PMC_ENABLE (register 0x0) has bit 12 (PGRAPH) clear. The PGRAPH engine is powered off, which explains why the fence lane hits a wall (hardware scheduler refuses to fetch runlists when the target engine is off).

## Implementation Plan
Lane: `unaos/crates/kernel/src/gpu/kepler.rs` ONLY.

1. **Print PMC_ENABLE pre-state**: Before writing, read `PMC_ENABLE` and print: `:: kepler: pgraph-enable pre=XXXXXXXX ::`
2. **Enable PGRAPH**: Read `PMC_ENABLE`, set bit 12 (`|= (1 << 12)`), and write it back.
3. **Readback & Verify**: Read `PMC_ENABLE` again and print: `:: kepler: pgraph-enable wrote=XXXXXXXX rb=XXXXXXXX ::`
   * If bit 12 is NOT set in readback, print `:: kepler: pgraph-enable REFUSED ::` and skip the rest of the PGRAPH block recon.
4. **Settle**: Add a bounded settle using the standard idiom: `for _ in 0..2_000_000 { core::hint::spin_loop(); }` (approx. 100ms equivalent).
5. **Re-reconnaissance**: Re-run the ENTIRE pull-18 recon dump of the PGRAPH Falcon block unchanged (both passes, same markers: cpuctl, bootvec, imemc, dmemc, pgraph-stat).
6. **Leave PGRAPH Enabled**: The engine will remain enabled as the normal state (no restore).

## Compliance Gates
* Exactly ONE new register write (PMC_ENABLE, bit-OR).
* Run syntax/build check: `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo check`
* Strings proof check: `UNAOS_IVB=1 UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 ./arroyo build esp-x86` and verify with `strings target/x86_64-unaos/release/unaos-kernel | grep -E "pgraph-enable"`
* Clean working tree with scratch files deleted.
* Commit ALL docs+code; no push. Report "PUSH OWED: 5".
