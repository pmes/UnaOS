#[macro_use]
pub mod serial;
pub mod cache;
pub mod percpu;
pub mod memory;
pub mod pci;
pub mod exceptions;
pub mod gic;
pub mod timer;
#[cfg(feature = "baremetal")]
pub mod boot;
#[cfg(feature = "baremetal")]
pub mod mailbox;
#[cfg(feature = "baremetal")]
pub mod smp;
// JC2: PSCI CPU_ON SMP on the QEMU `virt` GICv3 path (the non-Pi sibling of `smp`). Compiled out of
// every Pi image (baremetal implies pi); the kick-off in `main.rs` is additionally runtime-gated on
// `gic::is_v3()`, so the GICv2 virt run stays single-core.
#[cfg(not(feature = "pi"))]
pub mod smp_virt;
// JC3: the QEMU `virt`/UEFI EL2 -> EL1 drop (the non-Pi sibling of `boot`'s `drop_to_el1`/`enable_mmu`).
// `virt`-only (baremetal implies pi; tegra keeps its own EL2 regime in `mmu_tegra` this arc) — it un-gates
// the scheduler on the `virt` boot core so CAPSTONE runs at EL1.
#[cfg(all(target_arch = "aarch64", not(feature = "pi"), not(feature = "tegra")))]
pub mod boot_virt;
// JM6: the Jetson Orin (Tegra234) EL2 -> EL1 drop — the tegra analogue of `boot_virt`, reusing
// `mmu_tegra`'s already-built identity L1 for the EL1 regime so the boot core runs the scheduler +
// CAPSTONE at EL1 on silicon. `tegra`-only (mirrors `boot_virt`'s `virt`-only role).
#[cfg(all(target_arch = "aarch64", feature = "tegra"))]
pub mod boot_tegra;
// The aarch64 kernel-thread scheduler + sync primitives. Compiled for the Pi bare-metal path (its
// original home), the `virt` path (JC3, which drops EL2 -> EL1 via `boot_virt`), AND, since JM6, the
// `tegra` path (which drops EL2 -> EL1 via `boot_tegra`) — so the scheduler can run at EL1 on the Orin
// boot core. Its EL0/user machinery (`spawn_user*`, the user trampoline, slot teardown) stays
// `#[cfg(feature = "baremetal")]` (single-core Orin *userspace* is a follow-on arc).
#[cfg(any(
    feature = "baremetal",
    all(target_arch = "aarch64", not(feature = "pi"), not(feature = "tegra")),
    all(target_arch = "aarch64", feature = "tegra")
))]
pub mod sched;
// JETSON-EL0 (M1b): widened from `baremetal` to also cover `tegra_el0`. The module body is unchanged
// except for the `uslots` facade (below) it now names instead of `super::boot` — the Pi keeps the
// BCM2711 slot system in `boot.rs`, the Orin gets the tegra port in `mmu_tegra_el0.rs`, and syscall.rs
// compiles against whichever one the active feature selects.
#[cfg(feature = "aarch64_el0")]
pub mod syscall;
// BANDY-1 (ROADMAP §3b arc 1): the on-UnaOS SMessage bus — v1 subset codec (wire layer of the
// SYS_MSEND/SYS_MRECV transport in syscall.rs). Gated like syscall.rs — JETSON-EL0 (M1b) widened both
// from `baremetal` to `any(baremetal, tegra_el0)` together, since the bus IS syscall.rs's wire layer.
#[cfg(feature = "aarch64_el0")]
pub mod bus;
// JM3: Jetson Orin Nano (Tegra234) kernel-owned MMU. The tegra/UEFI build maps RAM Normal-WB + the
// Tegra device windows Device-nGnRE before touching any peripheral MMIO (the R4 UARTC-fault fix).
// (ORIN-NET-3 also compiles it for the `pcie3` virt witness — `ps_widen_witness` exercises the real
// `map_mmio_window` reach ceiling there; on virt the L1 statics are inert, not the active regime.)
#[cfg(any(feature = "tegra", feature = "pcie3"))]
pub mod mmu_tegra;
// JETSON-EL0 (M1b): the EL0 user address-space machinery for the tegra EL1 regime — the M6d slot system
// (`boot.rs`) re-implemented against `mmu_tegra`'s `L1_EL1` instead of the Pi's BCM2711 identity map.
// See the module header for the three places it necessarily diverges from `boot.rs`.
#[cfg(feature = "tegra_el0")]
pub mod mmu_tegra_el0;

// JETSON-EL0 (M1b): THE FACADE. `syscall.rs` and `sched.rs` consume one user-address-space API — the
// slot pool, the user window VA, the W^X leaf flips, the FB surface hole, ASID teardown. Two modules
// implement it: `boot.rs` for the Pi (BCM2711, `baremetal`) and `mmu_tegra_el0.rs` for the Orin
// (Tegra234, `tegra_el0`). Routing the consumers through this one re-export is what let the EL0 chain
// reach the Orin WITHOUT editing a single line of logic in either consumer — `syscall.rs`'s 308 call
// sites changed module path only (`super::boot::` -> `super::uslots::`), and `sched.rs`'s five changed
// the same way. The two features are mutually exclusive in practice (`baremetal` implies `pi`,
// `tegra_el0` implies `tegra`, and `pi`/`tegra` are mutually exclusive), but `baremetal` is written to
// win the arm explicitly so a nonsensical both-on build picks one deterministically instead of
// colliding on a duplicate glob import.
#[cfg(feature = "aarch64_el0")]
pub mod uslots {
    #[cfg(feature = "baremetal")]
    pub use super::boot::*;
    #[cfg(all(feature = "tegra_el0", not(feature = "baremetal")))]
    pub use super::mmu_tegra_el0::*;
}
// JB1a: bounded read-only FDT walker — prints the BPMP IPC geometry (shmem/mboxes/reserved-memory)
// from the firmware DTB, the verified starting line for the BPMP IVC arc (JB1).
// (ORIN-NET-1/-2 also reuse this walker's `Fdt`/`for_each_prop` for their read-only PCIe census, so it
// is compiled for the `pcieprobe`/`pcie2` virt witness builds as well as every tegra build.)
// (PI-GENET reuses this walker's `Fdt`/`for_each_prop` to DTB-resolve the GENET register base on the
// Pi bare-metal build, so it is compiled for `genet` too.)
#[cfg(any(feature = "tegra", feature = "pcieprobe", feature = "pcie2", feature = "genet"))]
pub mod fdt_tegra;
// JB1b: the BPMP IVC command channel (shmem queues + HSP doorbell) — first light is MRQ_PING; the
// transport every partition-ungate MRQ (XUSB, nvdisplay) rides on.
#[cfg(feature = "tegra")]
pub mod bpmp_tegra;
// JB2b: platform-attach of the shared xHCI driver at the ungated XUSB block (raw MMIO, polled) +
// the EL1 keyboard-pump task — the Orin USB-keyboard first-light arc.
#[cfg(feature = "tegra")]
pub mod xusb_tegra;
// JB3: the NISO1 SMMU (dual MMU-500) — read-only probe of the stream-match state that the JB2c
// metal verdict indicted for the total XUSB DMA-write drop (ENABLE_SLOT watchdogs, 0 events).
#[cfg(feature = "tegra")]
pub mod smmu_tegra;
// JD1: inherit the firmware's live scanout framebuffer (DTB `simple-framebuffer` handoff) so fbcon
// mirrors the boot log + CAPSTONE onto the Orin panel — the fix for the BltOnly-GOP dead end (JM7).
#[cfg(feature = "tegra")]
pub mod display_tegra;
// ORIN-SMP-2 (JM5 `CPU_ON` firmware-wall INVESTIGATION): the `UNAOS_SMPPROBE=<n>` boot-time probe.
// Tegra-only AND `smpprobe`-gated, so it is compiled out of every default image (the `smpprobe`-off
// tegra binary stays byte-identical to baseline). Wired into `tegra_early_stop` after JM4.
#[cfg(all(feature = "tegra", feature = "smpprobe"))]
pub mod smpprobe; #[cfg(all(feature = "tegra", feature = "selfup"))] pub mod selfup_tegra; #[cfg(all(feature = "tegra", any(feature = "ga10bprobe1", feature = "ga10bprobe2", feature = "ga10bprobe3")))] pub mod ga10b_probe; #[cfg(all(feature = "tegra", feature = "tcuprobe"))] pub mod hsp_tegra; // ORIN-SELFUP: the self-update core (staged whole-ESP payload -> sha256 verify -> proven FAT write -> warm-reboot hook), wired into tegra_early_stop after ORIN-INSTALL-2; appended to THIS line for knob-off byte-identity (inline fns with Location-bearing ops live below). GA10B-PROBE1: the first read-only GA10B iGPU probe rung (BPMP rail gate -> risk-ordered announce-first BAR0 reads -> SYSTEM_OFF), its own gated pub mod also appended to THIS line for knob-off byte-identity (its Location-bearing ops live in the module). TCURX (orin 14): the Tegra234 HSP shared-mailbox / TCU RX read-only probe (`tcuprobe`, implies `tegra`), its gated pub mod appended to THIS line before the comment for the same knob-off byte-identity (P7: the attribute goes BEFORE the first `//`; after it the mod is prose). GA10B-PROBE3 (orin 16): rungs 3/3b of the GA10B ladder live in the SAME `ga10b_probe.rs`, so the module gate's `any(...)` gained `ga10bprobe3` in place — no new line, no `Location` shift; `ga10bprobe3b` implies `ga10bprobe3` and therefore needs no term of its own here.
// ORIN-REBOOT (watchdog half): the `UNAOS_ORINWDT=1` Tegra234 TKE boot watchdog — a wedged
// Orin boot self-resets (POR on the 5th WDT0 expiration) instead of sitting dark for bench
// hands. Tegra-only AND `orinwdt`-gated (`orinwdt` implies `tegra`), so it is compiled out
// of every default image. Armed early in `tegra_early_stop`, disarmed on the EL1 terminus.
#[cfg(all(feature = "tegra", feature = "orinwdt"))]
pub mod wdt_tegra;

// PI-V3D-1: the VideoCore VI (V3D 4.2) GPU foundation — firmware power/clock, MMIO probe, the
// V3D-private MMU, and a minimal GPU buffer-clear. Pi bare-metal AND `v3d`-gated, so it is compiled
// out of every default image (the `v3d`-off kernel8 binary stays byte-identical to baseline). QEMU
// raspi4b does not model V3D: the probe detects absence and degrades gracefully; all positive
// verification is attended metal. Enable via `UNAOS_V3D=1 UNAOS_PI=1 ./arroyo kernel8`.
#[cfg(all(feature = "baremetal", feature = "v3d"))]
pub mod v3d;

// PI-USB-1: the BCM2711 PCIe root complex + VL805 xHCI attach — the RC that hosts ALL four Pi USB-A
// ports (via the single VIA VL805 endpoint, 1106:3483). Pi bare-metal AND `piusb`-gated, so the module
// + its call site (mailbox.rs tail) vanish knob-off and the kernel8 binary stays byte-identical to
// baseline. QEMU raspi4b does NOT model the PCIe RC: the bring-up detects absence (poison/all-ones RC
// identity) and degrades gracefully; positive verification is attended metal. Enable via
// `UNAOS_PIUSB=1 UNAOS_PI=1 ./arroyo kernel8`. See arch_arm64.md §PI-USB.
#[cfg(all(feature = "baremetal", feature = "piusb"))]
pub mod piusb;

// ORIN-NET-1 (`UNAOS_PCIEPROBE=1`) + ORIN-NET-2 (`UNAOS_PCIE2=1`): read-only PCIe root-complex + NIC
// recon (arch/aarch64/pcie_probe.rs). NET-1 = DTB census of every `pcie@` controller + a guarded,
// poison-rejecting config-space liveness read for firmware-ENABLED, already-mapped controllers. NET-2
// = controller-0 link state (DBI PCIe-capability Link Status) + config/ECAM aperture mapping (the
// existing kernel page-table path) + bus-0/bus-1 walk. Both wired into `tegra_early_stop` (metal) and
// the virt GICv3 path (QEMU graceful-skip witness). Feature-gated only — NOT tegra-runtime-gated — so
// the module compiles for both the tegra and the `virt` builds. With BOTH knobs OFF (default) the
// module + all call sites vanish and every image is byte-identical to baseline.
#[cfg(any(feature = "pcieprobe", feature = "pcie2"))]
pub mod pcie_probe;

// ORIN-NET-4 (`UNAOS_NET4=1`, implies `pcie3`): the Realtek RTL8168/8111 GbE driver + smoltcp bind —
// the Orin's first network path (arch/aarch64/rtl8168_tegra.rs). Claims controller-0's downstream
// device (bus1:dev0:fn0 = 0x10ec:0x8168) through NET-3's PS-widened ECAM, maps its register BAR,
// drives the C+ RX/TX descriptor rings, reads the MAC, and binds a smoltcp phy::Device. The
// MMIO/DMA + smoltcp adapter are additionally `tegra`-gated (QEMU models no Tegra234 RC); a net4/virt
// build only prints one witness line. Feature-gated only — compiles for both builds. Default OFF =>
// module + call sites vanish, byte-identical to baseline.
// NET-PHY: the shared smoltcp phy::Device adapter moved to the crate root (`crate::net_phy`) so the x86
// default net stack (smolnet) can share it too — see crates/kernel/src/net_phy.rs. rtl8168_tegra.rs
// (net4) and virtio_net.rs (vnet) bind over `crate::net_phy::SmoltcpPhy`.

#[cfg(feature = "net4")]
pub mod rtl8168_tegra;

// ORIN-SDMMC-1 (`UNAOS_SDMMC=1`): the Tegra234 microSD-slot SDMMC controller READ-ONLY recon
// (arch/aarch64/sdmmc_tegra.rs) — the installer line's first rung. Resolves the controller from the live
// DTB, poison-honest CAPS probe, SDHCI identification ladder (CID/CSD/capacity), and a sector-0 read to
// classify MBR/GPT/FAT — no card write anywhere. The controller MMIO is additionally `tegra`-gated (QEMU
// models no Tegra234 SDMMC); a sdmmc/virt build only prints one honest compiled-present witness line.
// Feature-gated only — compiles for both builds. Default OFF => module + call sites vanish, byte-identical.
#[cfg(feature = "sdmmc")]
pub mod sdmmc_tegra;

// AARCH64-VNET (`UNAOS_VNET=1`): a virtio-net-mmio driver on the QEMU `virt` machine + smoltcp bind
// (arch/aarch64/virtio_net.rs). The QEMU-testable proof of the aarch64 smoltcp seam NET-4 built — the
// identical ring → phy::Device → Interface → ICMP-echo shape, driven with REAL packets over QEMU
// user-mode networking (slirp) against a device QEMU actually models (unlike the Tegra234 RC NET-4
// needs). Scans the fixed virtio-mmio window, drives the LEGACY transport, and runs an ICMP-ping
// witness. Feature-gated only. Default OFF => module + call site vanish, smoltcp not pulled,
// byte-identical to baseline. See arch_arm64.md §AARCH64-VNET.
#[cfg(feature = "vnet")]
pub mod virtio_net;

// PI-GENET (`UNAOS_GENET=1`): the BCM2711 on-board Gigabit Ethernet (Broadcom GENET v5) driver +
// smoltcp bind (arch/aarch64/genet.rs) — the Pi's FIRST network path, riding the shared net_phy seam
// vnet/net4 already use. DTB-resolves the register base, poison-honest probes SYS_REV_CTRL to classify
// whether the build models GENET, brings up UMAC + external RGMII PHY + TDMA/RDMA rings, and binds a
// smoltcp Device + DHCP/ping. The MMIO/DMA metal half is additionally `all(baremetal, aarch64)`-gated,
// so a genet/non-baremetal build compiles only an honest witness line. Feature-gated only. Default
// OFF => module + call site vanish, smoltcp not pulled, byte-identical. See arch_arm64.md §PI-GENET.
#[cfg(feature = "genet")]
pub mod genet;

pub fn init() {
    serial_println!(":: AARCH64 Core Hardware Init ::");
    boot_diagnostics();
    // Per-CPU data for the boot core (TPIDR_EL2) before anything can take an interrupt, so an IRQ
    // handler that resolves `percpu::this_cpu()` (the SGI path) always has a valid block. Secondary
    // cores do their own `percpu::init` in `smp::__secondary_rust`.
    percpu::init(0);
    // Bring up interrupts, mirroring the x86 init order (IDT -> APIC -> timer -> sti):
    //   1. install the exception vectors (and, at EL2, route async exceptions to EL2);
    //   2. bring up the GICv2 (distributor + this core's CPU interface);
    //   3. arm the generic timer (enables its PPI at the GIC);
    //   4. unmask IRQ in PSTATE — the heartbeat starts here.
    exceptions::install();
    gic::init();
    timer::init();
    // One-shot self-test: confirm the timer asserts and the GIC latches its PPI before we unmask
    // (the analogue of the x86 APIC/SMP smoke tests; ~one tick period, invaluable on the
    // serial-less Pi where this rides the fbcon boot log).
    timer::diagnose();
    exceptions::enable_irq();
    // Confirm the timer IRQ actually delivers before committing the idle path to WFI. On hardware
    // where it doesn't (an unverified GIC routing), this leaves `hlt()` on its poll-spin fallback so
    // the GUI stays responsive instead of freezing in a wake-less WFI.
    timer::verify_live();
}

/// Read-only boot probe: dump the Exception Level we were handed off at, the generic-timer
/// frequency, the MMU state, and DAIF — all from system registers (zero MMIO, so it cannot fault
/// before exception vectors exist). This grounded the GIC/timer bring-up. Firmware hands every core
/// off at EL2; the bare-metal build then drops to EL1 (see `boot::drop_to_el1`) before this runs, so
/// it prints EL=1, while the UEFI/QEMU-virt build stays at EL2. CNTFRQ differs by board (QEMU
/// 62.5 MHz vs Pi 4 54 MHz), so the tick interval is computed from it at runtime.
fn boot_diagnostics() {
    let current_el: u64;
    let cntfrq: u64;
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) current_el, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) cntfrq, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nomem, nostack, preserves_flags));
    }
    // CurrentEL holds the EL in bits [3:2]. The MMU/cacheability is governed by SCTLR for the EL we
    // actually run at, so read SCTLR_EL2 when at EL2 (reading SCTLR_EL1 there would be misleading —
    // it describes a translation regime we aren't using).
    let el = (current_el >> 2) & 0b11;
    let sctlr: u64;
    unsafe {
        if el == 2 {
            core::arch::asm!("mrs {}, SCTLR_EL2", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        } else {
            core::arch::asm!("mrs {}, SCTLR_EL1", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        }
    }
    serial_println!(
        ":: AARCH64 boot diag: EL={}  CNTFRQ={} Hz  MMU={}  DAIF(DAIF)={:#06b} ::",
        el,
        cntfrq,
        if sctlr & 1 != 0 { "on" } else { "off" },
        (daif >> 6) & 0b1111,
    );
    // UVUG7-W — WITNESS-STATE HONESTY. The `witness` cargo feature is DEFAULT-QUIET: `./arroyo kernel8`
    // (the flashable media command) leaves it OFF, so every `#[cfg(feature = "witness")]` call site —
    // including both `[uvug7]` sites — is compiled clean out of the image. Without this line a default
    // boot log is INDISTINGUISHABLE from a witness-armed boot whose gate never became true, and a
    // missing `[uvugN]` reads as a live bug rather than as absent code. That ambiguity is exactly what
    // let P53's "[uvug7] never printed" mystery survive four metal boots (P53-P56) before the P57
    // capture — staged from a witness-armed image — printed all four `[uvug7]` lines normally.
    // Unconditional and self-describing, so the answer is always in the log itself.
    serial_println!(
        ":: AARCH64 build: witness={} — witness-gated [uvugN]/fixture lines are {} in this image ::",
        if cfg!(feature = "witness") { "on" } else { "off" },
        if cfg!(feature = "witness") { "PRESENT" } else { "ABSENT BY CONSTRUCTION (not failures)" },
    );
}

pub fn hlt_loop() -> ! {
    loop {
        hlt();
    }
}

pub fn hlt() {
    // Interrupt-driven idle WHEN the timer IRQ is confirmed delivering (`timer::verify_live`): WFI
    // parks the core until the next tick (it wakes on a pending physical interrupt even with
    // PSTATE.I set, so a panic-time hlt_loop still halts cleanly). If liveness was NOT confirmed —
    // an untested GIC path where the PPI never reaches the CPU — WFI would have no wake source and
    // sleep forever, freezing the polled main loop; fall back to a light poll-spin so input keeps
    // being serviced (the pre-interrupt behavior, trading idle power for liveness).
    //
    // HID-REGRESS-B12: `is_live()` tracks the BOOT CORE's timer, which the Jetson JM6 EL2->EL1 drop
    // disables (`set_not_live`). A GICv3 SECONDARY that armed its OWN local tick (JC3) still has a live
    // per-core wake source, so it too may WFI (bounded to one ~4 ms tick) — otherwise it busy-spins its
    // `run()` steal loop post-drop and the shared-lock churn starves the boot core's xHCI HID poll.
    if timer::is_live() || timer::this_core_has_local_tick() {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
    } else {
        core::hint::spin_loop();
    }
}

/// SERFIX — non-blocking by construction: the acquisition, its degradation and its census all live
/// in `serial::poll_input_nonblocking` (see the SERFIX block in arch/aarch64/serial.rs for why this
/// must never become a blocking `SERIAL_PORT.lock()` again).
pub fn poll_input() -> Option<u8> {
    serial::poll_input_nonblocking()
}

/// Make CPU writes to `[addr, addr+len)` visible to a non-coherent scan-out engine — the Pi 4's
/// VideoCore HVS reads the framebuffer straight from RAM and does not snoop the A72 data cache, so
/// after the GUI flushes pixels into a (cacheable) framebuffer they must be cleaned to the Point of
/// Coherency or the display shows stale rows. Called from the shared video path (`FrameBuffer`/
/// `fbcon`); a `DC CVAC` sweep on aarch64, a no-op on the cache-coherent x86 framebuffer and inert
/// in QEMU. See arch/aarch64/cache.rs for why this is mandatory on metal but invisible in QEMU.
#[inline]
pub fn flush_framebuffer_range(addr: usize, len: usize) {
    cache::clean_range(addr, len);
}

/// Monotonic tick counter since boot. Arch-neutral entry point (mirrors x86_64); now backed by the
/// generic-timer heartbeat (~250 Hz) rather than the old 0 stub.
pub fn ticks() -> u64 {
    timer::ticks()
}

/// Milliseconds since boot, derived DIRECTLY from the free-running virtual counter (CNTVCT_EL0) and
/// its rate (CNTFRQ_EL0): `ms = CNTVCT / (CNTFRQ/1000)`. Frequency-independent and core-count
/// independent, correct on every path (pi / QEMU virt / tegra) with no cfg split.
///
/// UVUG-7 (P52, metal): the old `ticks()*4` assumed a single 250 Hz tick source. But `ticks()`
/// (timer::TICKS) is bumped by EVERY core's periodic timer IRQ (see `timer::on_tick`), so on 4-core
/// BCM2711 metal it advanced ~4x faster than 250 Hz — making `ms()` run ~4x fast, which drove
/// typematic key-repeat ~4x too fast (one keypress emitted ~7 chars). CNTVCT counts at CNTFRQ
/// regardless of how many cores tick or whether the tick IRQ is delivered at all, so this is immune
/// to both faults. It also fixes QEMU raspi4b, where the tick IRQ is never delivered and `ticks()`
/// (hence the old `ms()`) stayed frozen at 0.
#[inline]
pub fn ms() -> u64 {
    let khz = timer::cntfrq() / 1000;
    if khz == 0 { 0 } else { now_cycles() / khz }
}

/// Free-running virtual cycle counter (CNTVCT_EL0). Monotonic and interrupt-flag-independent, like
/// x86 rdtsc — the portable timebase for bounding hardware busy-waits (see `now_cycles` on x86_64).
/// Runs at CNTFRQ_EL0 (~62.5 MHz under QEMU virt, 54 MHz on the Pi 4), NOT GHz, so its budget is in
/// its own units. The generic timer is up and delivery-confirmed on both paths (`timer::verify_live`),
/// and the bare-metal EL1 drop sets CNTVOFF_EL2=0 so CNTVCT shares the physical timebase (CNTPCT).
#[inline]
pub fn now_cycles() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack, preserves_flags));
    }
    v
}

/// Fixed FALLBACK busy-wait budget in `now_cycles()` (CNTVCT) units, and the whole budget off-tegra. NOT "~2.5 s at a ~60 MHz generic-timer rate", as this line claimed until 2026-08-25: no part in this tree runs at 60 MHz, and a fixed CYCLE count is a different DURATION on every one of them — 2.78 s on the Pi 4 (CNTFRQ=54000000 Hz on its own boot diag; BOTBOUND's boot 11 blew a 2000 ms budget printing `spent_ms=2779` against 150e6/54e6 = 2777.8 ms, agreement to the millisecond), 2.4 s on QEMU virt (62.5 MHz), and 4.8 s on the Jetson Orin (CNTFRQ=31250000 Hz, capture-proven over three boots — 92% past the figure this line advertised). `hw_wait_budget()` derives a DURATION from CNTFRQ on tegra; this constant is what that derivation falls back to when CNTFRQ_EL0 reads 0, and what it still returns verbatim off-tegra. ⚠ ONE LINE: this file is compiled knob-off and panic `Location` records embed line numbers.
pub const HW_WAIT_BUDGET: u64 = 150_000_000;

/// Busy-wait budget in `now_cycles()` (CNTVCT) units. Arch-neutral mirror of x86_64's `hw_wait_budget`,
/// which is an honest `tsc_hz * HW_WAIT_SECONDS` once calibrated. The aarch64 twin needs no calibration
/// — CNTFRQ_EL0 states the rate exactly — but only tegra takes it; see the tail block for why not all.
#[inline]
pub fn hw_wait_budget() -> u64 {
    hw_wait_budget_derived()
}

/// WEDGE-7 — the RAII form of [`without_interrupts`], for spans that cannot be expressed as a
/// closure. `without_interrupts` is fine when the masked region is a block; it is not usable when
/// the mask must live exactly as long as a lock guard whose scope the caller controls (see
/// `video::wm::table`). Same save/restore semantics, same nesting correctness: an inner guard
/// restores to "still masked" rather than unconditionally unmasking.
///
/// Public and arch-neutral by re-export (`crate::arch::IrqMask`), because the code that needs it
/// (`video/wm.rs`) compiles on both arches. The x86_64 twin lives in that arch's `mod.rs`.
pub struct IrqMask(u64);

/// WEDGE-8 (F3): true when IRQs are masked on this core (`PSTATE.I` set). The block layer uses it
/// to pick between "wait politely for the xHCI loan" (unmasked, schedulable) and "refuse instantly
/// with `Busy`" (masked — a masked wait on a preemptible holder is the F3 deadlock). Arch-neutral
/// by re-export; the x86_64 twin reads `RFLAGS.IF`.
#[inline]
pub fn irqs_masked() -> bool {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
    }
    daif & (1 << 7) != 0
}

impl IrqMask {
    #[inline]
    pub fn new() -> Self {
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
            core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags));
        }
        IrqMask(daif)
    }
}

impl Default for IrqMask {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for IrqMask {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            core::arch::asm!("msr daif, {}", in(reg) self.0, options(nomem, nostack, preserves_flags))
        };
    }
}

pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    // Save DAIF, mask IRQ (daifset #2 = the I bit), run, then RESTORE the saved state. Restoring
    // (rather than blindly `daifclr`) keeps nested calls correct: an inner call must not re-enable
    // interrupts that an outer call had masked. Harmless today (aarch64 runs polled with no
    // interrupt sources) but correct for when a GIC/timer lands.
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
        core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags));
    }
    let ret = f();
    unsafe {
        core::arch::asm!("msr daif, {}", in(reg) daif, options(nomem, nostack, preserves_flags));
    }
    ret
}

// ── HWBUDGET: the CNTFRQ-derived busy-wait budget (tail-defined per the Location-shift convention
// this file and `timer.rs` already follow; the non-tegra arm is byte-identical to the constant it
// replaced, so the pi and QEMU-virt images stay byte-identical).
//
// The bug: [`HW_WAIT_BUDGET`] is a CYCLE count, and every caller reads it as a DURATION ("~2 s",
// "~2.5 s", "~8.3 s on a failing transfer" — see `drivers/xhci/mod.rs` and `main.rs`). Those two
// only agree at one frequency, and it is not a frequency any part in this tree runs at. The Orin
// is the worst-off: CNTFRQ_EL0 = 31_250_000 Hz makes 150e6 cycles 4.8 s, 92% past the "~2.5 s"
// the constant advertised. `arch/aarch64/xusb_tegra.rs` had already worked this out in-lane and
// carries the correct "~4.8 s at Orin's 31.25 MHz"; the arch constant had not caught up.
//
// The fix is the one x86_64 already made: state the budget as WALL CLOCK and multiply by the
// measured rate (`tsc_hz * HW_WAIT_SECONDS` there, `cntfrq * HW_WAIT_MS / 1000` here). aarch64
// needs no calibration pass to do it — CNTFRQ_EL0 *is* the rate, architecturally.
//
// ⚠ WHY THIS IS TEGRA-GATED, AND WHAT THAT COSTS. Deriving on every aarch64 part would be more
// correct and is NOT what this commit does, because it would silently RE-TIME the Pi 4 and QEMU
// virt: at 54 MHz a 150e6-cycle budget is the 2.78 s BOTBOUND measured on boot 11, and any whole-
// second target re-times it (2 s → -28%, 3 s → +8%). Re-timing the Pi's BOT pump is a behavioural
// change to a platform this seat holds no boot evidence for and does not own; it belongs to the
// pi seat, with its own metal gate. So: tegra derives, everyone else keeps the exact cycle count
// they have always had. This is a deliberate asymmetry, flagged here so the next reader does not
// mistake it for an oversight — the honest one-line summary is that the CONSTANT is wrong
// everywhere and is only FIXED where the seat that owns the silicon can prove the fix.
//
// ⚠ WHY 4800 ms AND NOT 2500. Because 4.8 s is what the Orin has ALWAYS waited (31_250_000 × 4800
// / 1000 = 150_000_000 exactly — the derivation reproduces today's budget to the cycle), and no
// evidence in this tree says the Orin's storage path wants 2.5 s. Picking the doc's aspirational
// "~2.5 s" would have been a real 48% shortening of every xHCI/BOT/FS wait on this platform,
// dressed up as a comment fix. The correct Orin duration is genuinely open; this commit refuses to
// settle it silently and preserves the measured status quo instead. What it BUYS is that the
// number is now a duration: a Tegra part that clocks CNTFRQ differently (Tegra silicon has shipped
// 19.2 MHz and 31.25 MHz) gets 4.8 s, where the raw constant would have handed it 7.8 s.
//
// CNTFRQ_EL0 = 0 falls back to [`HW_WAIT_BUDGET`] rather than deriving a budget of ZERO, which
// would fail every bounded wait instantly. That is not hypothetical on this platform: `timer::init`
// carries a whole tegra-gated witness (`CNTFRQ-SUB`) for a firmware that never programmed it, and
// its own comment names "every wall-clock budget derived from it" as the blast radius. This is that
// derivation, so it takes the same guard — and the fallback is exactly the cycle count that shipped
// before, so an unprogrammed CNTFRQ behaves precisely as it did.
#[cfg(feature = "tegra")]
const HW_WAIT_MS: u64 = 4800;

/// Backing derivation for [`hw_wait_budget`]. Split out so the tegra-only CNTFRQ read is defined at
/// the tail of the file, where adding it cannot shift a panic `Location` line number above it.
#[cfg(feature = "tegra")]
#[inline]
fn hw_wait_budget_derived() -> u64 {
    let hz = timer::cntfrq();
    if hz != 0 {
        hz.saturating_mul(HW_WAIT_MS) / 1000
    } else {
        HW_WAIT_BUDGET
    }
}

/// Off-tegra: the fixed cycle count, unchanged and un-re-timed. See the block above.
#[cfg(not(feature = "tegra"))]
#[inline]
fn hw_wait_budget_derived() -> u64 {
    HW_WAIT_BUDGET
}

/// [`flush_framebuffer_range`]'s STRIDED twin: clean `rows` runs of `row_len` bytes, each `stride`
/// apart, for the same reason and to the same Point of Coherency — the RECT form a damage-tracked
/// present wants, where cleaning the full-width scanlines the rect sits in would touch several times
/// the bytes the caller actually wrote. aarch64 answers with `cache::clean_rows`: a per-row
/// `DC CVAC` sweep with ONE trailing `DSB` for the whole rect rather than one per row.
///
/// This is the facade `video::framebuffer::flush_rect`'s doc comment has named since it was written;
/// until now it did not exist and that call site open-coded a `#[cfg(target_arch)]` pair instead.
///
/// APPENDED AT THE FILE TAIL, deliberately: this module is compiled into the knob-off `kernel8.img`
/// whose byte-identity is a standing proof, and panic `Location` records embed line numbers, so a
/// function inserted mid-file would move every record below it (PARITY.md 5.3). Nothing is below
/// this, so nothing moves — the same reasoning `video/mod.rs` gives for its own tail block.
#[inline]
pub fn flush_framebuffer_rows(addr: usize, row_len: usize, rows: usize, stride: usize) {
    cache::clean_rows(addr, row_len, rows, stride);
}
