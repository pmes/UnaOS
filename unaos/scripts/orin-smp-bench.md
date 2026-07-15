# ORIN-SMP bench card (phase 1: CORE3-class audit + born-fixed PSCI re-derive)

**Status: NO METAL LEG THIS ARC — metal-blocked upstream.** The Orin PSCI `CPU_ON` bring-up
(`smp_virt.rs`) is parked on an external Tegra BL31/MCE fault: the first `CPU_ON` on real Orin
silicon triggers a fatal CBB-fabric RAS Uncorrectable Error and powers the box off (see
`docs/dev/OS/01_BOOT_HAL/arch_arm64.md` "JM5 result"). This arc is a **QEMU + disassembly** deliverable
only. The tegra image DCEs `smp_virt` (it is never called on the tegra path — `tegra_early_stop` is a
single-core terminus), so the fix leaves the tegra kernel **byte-unchanged** and there is nothing new
to flash. Do not stage or flash a metal image for this arc.

## What was verified (no card to run on metal)

- **Disassembly (proof of record; the hazard is QEMU-invisible).** Retained `virt` build,
  `llvm-objdump -d` of `__secondary_rust_virt`: the MMU-on point is `msr SCTLR_EL2`; the core id is
  derived AFTER it (`mrs MPIDR_EL1`), and every `[sp]` store/reload of the derived id is MMU-on. The
  advisory context-id x0 is never spilled. See §ORIN-SMP for the exact addresses (pre- and post-fix).
- **`./arroyo check`** — green both arches, with and without `UNAOS_TEGRA=1`.
- **`UNAOS_GICV3=1 ./arroyo test-arm`** — the only path that actually runs `__secondary_rust_virt`:
  3/3 secondaries online (each re-derives its own linear index from MPIDR affinity), BSP→AP SGIs 3/3,
  AP→BSP delivered, CAPSTONE 6/6.
- **Plain `./arroyo test-arm`** — GICv2 single-core, byte-unaffected (smp_virt is runtime-gated on
  `gic::is_v3()`).

## When the Orin `CPU_ON` firmware wall clears (future runbook)

The bring-up is already born fixed. To bench on Orin then: build `UNAOS_TEGRA=1 ./arroyo esp-jetson`,
wire the `tegra_early_stop` SMP kick-off in (currently omitted by design), stage the ESP per
`~/unaos-bench/flash/README.md` (never flash from `target/`), and expect each secondary to print
`AARCH64 SMP: AP <n> online (aff=…)` with its correct multi-cluster affinity and 6/6 cores online —
never a duplicate/phantom "core 0" (that would be an id-corruption re-appearance).
