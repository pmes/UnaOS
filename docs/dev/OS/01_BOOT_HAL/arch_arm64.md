# Architecture Specification: AArch64 (ARM64)

## Status: PRE-ALPHA / PLANNING

## 1. Device Tree (DTB) vs ACPI
Unlike x86, ARM targets often rely on Device Trees.
* **Strategy:** unaOS for ARM will prioritize **ACPI** (ServerReady/SystemReady compliance) to unify the boot flow with x86.
* **Fallback:** For mobile devices (Pixel, etc.), a "Shim Loader" will translate the proprietary DTB into a standardized ACPI format for the kernel.

## 2. Page Size
* **Target:** 16KB Pages.
* **Reason:** Apple Silicon utilizes 16KB pages for performance. To emulate macOS apps efficiently on M-series chips later, our kernel memory manager must support non-4KB page alignment natively.

## 3. Interrupt Controller (GIC) — v2 / v3 parameterization

The AArch64 interrupt controller is the Arm Generic Interrupt Controller (GIC),
the analogue of the x86 local APIC + I/O APIC. A single driver
(`crates/kernel/src/arch/aarch64/gic.rs`) speaks **both** major GIC
architectures behind one public API; the version is chosen at runtime so the
same kernel image runs on either:

* **GICv2** — the QEMU `virt` board default (`gic-version=2`, a GIC-400-class
  model) and the real Raspberry Pi 4 (BCM2711 GIC-400). Two register banks: the
  global **Distributor (GICD)** and a per-core, memory-mapped **CPU interface
  (GICC)**.
* **GICv3** — QEMU `virt` with `gic-version=3`. The memory-mapped GICC is
  replaced by a **system-register CPU interface** (`ICC_*_EL1`), the banked
  SGI/PPI state moves out of the GICD into a per-core **Redistributor (GICR)**,
  and SPIs are routed by MPIDR affinity (ARE=1). The Jetson Orin Nano
  (Tegra234) also has a GICv3, which is why this path exists — but see
  "Orin metal pending" below.

### Version detection

At `gic::init` the driver reads **`ID_AA64PFR0_EL1.GIC` (bits [27:24])**: `0`
selects the GICv2 (memory-mapped) path, non-zero (`0b0001`/`0b0011`) selects the
GICv3 (system-register) path. This is a system-register probe on purpose — it
cannot fault, and a non-zero field is exactly the capability the v3 path depends
on (the `ICC_*_EL1` registers). We deliberately do **not** probe `GICD_PIDR2`:
its GICv3 offset (`0xFFE8`) lies inside the 64 KiB v3 distributor frame but
**outside** the 4 KiB GICv2 distributor, so an MMIO read there raises an
external abort on a v2 machine (confirmed on QEMU `virt` `gic-version=2`).

The **`pi` build hard-pins v2 at compile time** (the GIC-400 is never a GICv3):
detection and the entire v3 code path are `#[cfg(not(feature = "pi"))]`, so
`is_v3()` folds to a compile-time `false` and the Pi image is byte-identical to
the pre-v3 driver. Every v3 addition is additive and dispatch-gated; the shared
v2 code paths are unchanged.

### Register-model differences (what each public entry point dispatches)

| Operation                | GICv2                              | GICv3                                                        |
|--------------------------|------------------------------------|-------------------------------------------------------------|
| Distributor enable       | `GICD_CTLR` bit0                   | `GICD_CTLR` ARE (bit4) **then** EnableGrp1 (bit1), RWP-waited |
| CPU interface            | `GICC` MMIO (PMR/BPR/CTLR)         | `ICC_SRE_EL1` (+ `ICC_SRE_EL2` at EL2), `ICC_PMR_EL1`, `ICC_CTLR_EL1` (EOImode=0), `ICC_IGRPEN1_EL1` |
| Banked enable (SGI/PPI)  | `GICD_ISENABLER0` / `IGROUPR0`     | per-core Redistributor SGI frame (`GICR_ISENABLER0`, `GICR_IGROUPR0`) |
| Timer PPI 30 enable      | GICD                               | this core's Redistributor                                   |
| SPI routing              | `GICD_ITARGETSR` (CPU bitmask)     | `GICD_IROUTER<n>` (MPIDR affinity)                          |
| Acknowledge / EOI        | `GICC_IAR` / `GICC_EOIR`           | `ICC_IAR1_EL1` / `ICC_EOIR1_EL1`                            |
| Send SGI (IPI)           | `GICD_SGIR`                        | `ICC_SGI1R_EL1`                                             |

On the QEMU `virt`/UEFI path the kernel runs at **EL2** with IRQs routed there
(`HCR_EL2.IMO`); EL2 accesses to the `ICC_*_EL1` **physical** interface are
gated by `ICC_SRE_EL2.SRE`, which the v3 CPU-interface bring-up sets in addition
to `ICC_SRE_EL1`.

### Self-SGI delivery smoke

After the GIC is initialized (both versions, QEMU `virt` path only) the boot
core sends a free SGI (INTID 15) to itself and confirms it is delivered through
the IRQ vector, printing `:: GIC self-SGI delivered (v2) ::` /
`(v3) ::`. It runs before the timer is armed and before IRQs are globally
unmasked, so it briefly unmasks `PSTATE.I` itself, then restores `DAIF`. This is
the only IPI exercise available until SMP reaches the `virt`/Orin path
(single-core UEFI today).

### Build knob

`UNAOS_GICV3=1 ./arroyo {arm,test-arm}` appends `-machine gic-version=3` to the
aarch64 `virt` QEMU invocation (QEMU takes the last value of the repeated
machine property). Default stays GICv2. The knob only affects the `virt` runs;
the Pi bare-metal paths (`kernel8*`, QEMU `raspi4b`) are always GICv2.

### Orin metal pending

All GICv3 work to date is **QEMU `virt` (`gic-version=3`) only**. The Jetson
Orin Nano's on-silicon GICv3 (real Redistributor frames, PSCI-woken secondary
cores, cluster affinities) is **not yet brought up** — that is a later,
operator-attended arc. Nothing here should be read as a metal claim: QEMU
models no Tegra machine, and QEMU-green does not imply Orin-correct.
