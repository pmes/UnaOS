// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// JB2b: platform-attach the shared xHCI driver to the Tegra234 XUSB host block and bring a USB
// keyboard to first light, polled — no PCIe, no MSI-X, no interrupts.
//
// The block sits at raw MMIO 0x0361_0000 (GiB 0 — Device-nGnRE in BOTH mmu_tegra tables, so it is
// reachable identically at EL2 pre-drop and EL1 post-drop). It is only touchable AFTER JB1c's BPMP
// ungate (MRQ_PG domains 12+10 + MRQ_CLK; a gated Tegra block is an EL3-fatal CBB abort — the JX1
// lesson), so `jb2b_attach` is gated on JB1c's ALIVE verdict at the call site.
//
// What the platform attach does NOT need (each verified against Linux xhci-tegra / tegra234.dtsi
// / edk2-nvidia before this arc):
//   * Firmware load: on Tegra234 the xHCI Falcon firmware is loaded once by UEFI (UsbFalconLib)
//     and stays RESIDENT — Linux's tegra234 soc data has no `.firmware`; its IFR path only reads
//     the header of the already-running firmware. USBCMD.HCRST resets the xHC state machine, not
//     the Falcon (separate reset domain), so the driver's standard halt+HCRST+CNR init is exactly
//     what Linux runs on t234 too.
//   * padctl/PHY programming: padctl @0x3520000 is a SEPARATE block with its own reset and no PG
//     domain in the JB1c toggle. JB2b BET that "UEFI's pad state survives" — a correct read on the
//     OLD firmware, LOST to the JetPack-6 update (edk2-nvidia "hide device resources at uefi exit"),
//     whose more aggressive ExitBootServices teardown powers the USB2 pads DOWN. JB2c re-programmed
//     the pad power-up sequence before the attach on that world's boots. On the JB9g/h INHERIT path
//     (the shipped recipe) the JB6 bootloader shim makes UEFI skip that teardown entirely, so the
//     pads arrive live and the JB2c re-power-up became dead code — RETIRED in JD4 (the history and
//     the Linux `tegra186_utmi_*_power_on` recipe live in the git record at JD3 and earlier). Still
//     never assert TEGRA234_RESET_XUSB_PADCTL (it would wipe all padctl state) and never set
//     VBUS_OVERRIDE (device-mode fake, wrong for a host port; the P3768 devkit's 5V rail is
//     regulator-always-on with no GPIO enable).
//   * The firmware mailbox (BAR2 @0x3650000): SS clock-scaling/ELPG requests, interrupt-delivered;
//     irrelevant to polled HS enumeration and left masked.
//   * Cache maintenance: tegra234.dtsi marks usb@3610000 `dma-coherent` — XUSB DMA snoops the CPU
//     caches through the fabric, so the driver's Normal-WB heap rings work as-is. The call site
//     probes the LIVE firmware DTB for the prop (verify-don't-assume) and prints the verdict; the
//     ordering half (Normal->Device doorbell publish) is the `dsb st` added to the shared driver.
//
// The remaining known unknown is the NISO1 SMMU: if UEFI left GBPA=ABORT for the XUSB stream id
// (rather than the expected bypass), DMA is silently dropped and enumeration times out BOUNDED
// (healthy PORTSC + command timeouts = that signature). That outcome is an honest-report STOP,
// not a crash: the attach window closes and the boot proceeds to the unchanged JM6b drop chain.

use crate::drivers::xhci;

/// The Tegra234 XUSB host (xHCI) capability base — the block JB1c ungated and JB2a surveyed.
const XUSB_HOST: u64 = 0x0361_0000;

/// Bounded-wait deadline helpers on CNTPCT (the same physical-counter pattern as bpmp_tegra):
/// monotonic, EL-independent, immune to a garbage CNTVOFF.
fn cntpct() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) v, options(nomem, nostack, preserves_flags));
    }
    v
}

fn cntfrq() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) v, options(nomem, nostack, preserves_flags));
    }
    // The same firmware-unset fallback timer::init/verify_live use: a raw 0 would collapse the
    // pump deadline to "now" and end the window after one pass. Orin reads 31.25 MHz on silicon;
    // this is defensive consistency with the siblings, not a live risk.
    if v == 0 { 62_500_000 } else { v }
}

/// USB-HID-MULTI: the armed-keyboard witness is now `XhciController::armed_keyboards` (returns ALL
/// armed HID keyboards, not just the first), driven from the pump loop below. See drivers/xhci/mod.rs.

#[inline]
fn dsb() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Busy-wait `us` microseconds on CNTPCT (EL-independent; needs no timer IRQ, so it is safe pre-drop
/// at EL2 exactly like the bpmp_tegra waits).
fn udelay(us: u64) {
    let ticks = cntfrq().saturating_mul(us) / 1_000_000;
    let start = cntpct();
    while cntpct().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

/// JB2b: attach the shared xHCI driver at the raw XUSB MMIO base and pump the polled enumeration
/// until a USB keyboard's interrupt-IN read is armed (or the window closes). Pre-drop, EL2 — the
/// JM4 timer is live here, which is what wakes the driver's bounded `crate::hlt()` sync pumps
/// (hub bring-up, SET_PROTOCOL). Every wait in the driver is budgeted, so the worst case (dead
/// DMA, wedged port) is a few bounded timeouts and the boot proceeds to the JM6b drop unchanged.
///
/// Returns Some((slot, port)) iff a keyboard is armed — the caller's cue to spawn the EL1 pump.
///
/// JD3: `service_storage` now DOES run in the pump loop below. A mass-storage device behind the
/// hub (the Alcor reader) has its SCSI/BOT bring-up done HERE, pre-drop, while the JM4 timer is
/// live — the only safe place for the driver's heaviest synchronous path, since the post-drop EL1
/// core has no timer to wake the BOT pump's bounded `crate::hlt()` waits. Once a disk is up the
/// pump returns; a keyboard-only boot is unaffected (`service_storage` is a no-op with nothing
/// pending). Still NOT run here: `service_ftdi` (the console-bridge arc, not this one).
/// JB3 boot-10: the FPCI wrapper — Tegra's XUSB host sits behind a fake-PCI config space
/// (`fpci` region @0x360_0000, same ungated partition as the host MMIO), and the FIRST thing
/// Linux xhci-tegra does before touching the controller is
/// `XUSB_CFG_1 |= IO_SPACE_EN | MEM_SPACE_EN | BUS_MASTER_EN` (+ program the BAR in CFG_4).
/// JB2b skipped this and went straight to the 0x361_0000 MMIO — which works for register
/// access, but **BUS_MASTER_EN gates the controller's ability to issue DMA at all**. The
/// boot-2..9 chain proved: SMMU open + translating + fault-free, MC SIDs programmed,
/// coherency ruled out (dc-civac: DRAM genuinely empty) — the one remaining torn-down link
/// is the wrapper's bus-master enable, exactly what an "hide device resources at exit" EBS
/// teardown would clear. Dump CFG_0/1/4, set the enables, re-dump.
const XUSB_FPCI: u64 = 0x0360_0000;

const XUSB_BAR2: u64 = 0x0365_0000;

// ---------------------------------------------------------------------------------------------
// JETSON-XCARVE M1: the relink-experiment pad.
//
// The xHCI-takeover carveout wall (arch_arm64.md §JETSON-XCARVE) is IMAGE-LAYOUT-CORRELATED: the
// leg-23 image faults 4/4 at the JB9i eviction step (fixed fault ADDR 0x800000027767dc80), its
// siblings ~50% or never, 0/19 across all prior sittings — same code, different link layout. This
// pad is the decisive cheap test: a `#[used]` (never GC'd) read-only static in its own named
// section that trivially SHIFTS every downstream section, symbol, and the image's own load extent
// — with ZERO semantic change (the bytes are never read; the takeover code is byte-for-byte the
// same). Rebuild the layout-correlated leg-23 image with `UNAOS_XCARVE_RELINK=1` and re-bench: if
// the fault vanishes or moves, layout correlation is PROVEN on one image (and leg 23 unblocks — its
// SMP question then answers in one clean boot). Knob-off the pad vanishes entirely (byte-identical
// to baseline). The distinct section name makes the delta trivially findable in objdump/readelf.
// 0x4000 (16 KiB) is large enough to move the load extent well past a page and shift the kernel's
// BSS/RODATA statics, small enough to stay inert.
#[cfg(feature = "xcarve_relink")]
#[used]
#[unsafe(link_section = ".xcarve_relink_pad")]
static XCARVE_RELINK_PAD: [u8; 0x4000] = [0xA5; 0x4000];


/// Secondary guard for the partition-reset revival lever (bpmp_tegra::jb4_powergate_cycle, boot 2).
/// Stays FALSE even when JB4_ENABLE is true — the seat flips it ONLY for the explicit boot-2 fork
/// (the cycle wipes the partition and requires re-running the JB3 chain afterward).
/// JB9c: answered (see JB4_ENABLE) — back to FALSE; the cycle kills the inherited firmware.
pub const JB4_ALLOW_PG_CYCLE: bool = false;

/// JB10 tripwire: the destructive MRQ_PG partition cycle (bpmp_tegra::jb4_powergate_cycle,
/// main.rs call site) DESTROYS the inherited firmware — proven post-OFF=0x0, no bare-IFR autoboot
/// on t234 (JB9c). The no-HCRST inherit recipe (JB9G_NO_HCRST) depends on that firmware staying
/// live, so the two can never be co-enabled. This compile-time assertion forces a future bench
/// editor who flips JB4_ALLOW_PG_CYCLE true to consciously also leave the inherit world
/// (JB9G_NO_HCRST=false) — a wrong pairing fails the build, not a boot.
const _: () = assert!(!(JB4_ALLOW_PG_CYCLE && JB9G_NO_HCRST));

// ---------------------------------------------------------------------------------------------
// JB5 (attended bench): the raw-handoff Falcon witness + per-stage bisect.
//
// The JB4 bench + the Linux-source research flipped the frame: Linux never REVIVES the Falcon —
// on t234 it has no code that can (tegra234_soc.firmware=NULL → the driver only polls CNR), and a
// genuine partition power-cycle strands the Falcon for Linux exactly as it did for us (the
// rmmod/insmod xhci-tegra twin: "Falcon state: 0xffffffff", reboot required). Linux works because
// it INHERITS a running Falcon from MB2 (our own MB2 log: "Binary xusb loaded successfully" +
// "Skipping powergate XUSB" — partition left ON, firmware resident). edk2-nvidia has NO XUSB-host
// DXE driver, so the device-discovery EBS teardown never touches the Falcon (dossier correction).
// So WHO halts ours? Two suspects: (a) our own JB chain — every CPUCTL read we ever took came
// AFTER JB1c's MRQs + padctl + SMMU/MC rewires; (b) the generic MdeModulePkg XhciDxe, active at
// EBS because our boot medium hangs off the USB reader. JB5 answers both in one boot per medium:
// witness the Falcon at RAW handoff (before any XUSB-affecting MRQ), then re-witness after every
// stage — if it arrives alive and dies at stage N, stage N is the killer; if it arrives dead on
// the USB-reader boot but alive on the module-microSD boot, the USB boot path is the killer.
//
// The one physical impurity: the CSB/CPUCTL window rides BAR2, and UEFI hands us FPCI with BARs
// unprogrammed + decode disabled — so the "raw" witness needs a minimal BAR2 route first (BAR
// bases + MEM_SPACE_EN only; no bus-master, no io — jb3_fpci_enable's full 0b111 comes later at
// its own checkpoint). Config-space reads (CFG_*) decode regardless. Guarded by a read-only
// MRQ_PG GET_STATE (bpmp_tegra::jb5_pg_on) so a gated partition is a printed SKIP, not the JX1
// EL3-fatal abort. Every touch prints before it happens (JX1 discipline).
// ---------------------------------------------------------------------------------------------

/// JB5 master gate: probe media only. When false every jb5_* is a silent no-op (call sites can
/// stay wired).
pub const JB5_PROBE: bool = true;

/// JB5 run-D/E branch: the destructive UEFI XhciControllerDxe *replay* (jb5_uefi_pg_cycle POWER-
/// CYCLES the XUSB partition, then jb5_elpg_release / jb5_fpci_uefi_and_poll). RETIRED FALSE for
/// JB6: a power-cycle re-runs MB2's one-shot Falcon boot only on a cold partition — but on a JB6
/// boot the Falcon is inherited LIVE (the bootloader's dummy-ACPI shim makes UEFI skip the XUSB
/// teardown), and cycling the domain would KILL it. With this false the branch takes the normal,
/// non-destructive `jb1c_ungate_xusb` path (MRQ_PG-ON is a harmless no-op on an already-ON domain),
/// while JB5_PROBE stays true so the read-only raw-handoff witness + downstream witnesses still run
/// and prove the inherited Falcon is alive (CPUCTL != 0xffffffff). See jetson JB6 baton.
pub const JB5_RUN_E_REPLAY: bool = false;

/// JB10 tripwire (twin of the JB4 one): the JB5 run-D/E branch power-cycles the XUSB partition
/// (bpmp_tegra::jb5_uefi_pg_cycle) and likewise KILLS the inherited firmware. It is mutually
/// exclusive with the no-HCRST inherit recipe — enforced at compile time so it can never be
/// co-enabled with JB9G_NO_HCRST on an inherit-path boot.
const _: () = assert!(!(JB5_RUN_E_REPLAY && JB9G_NO_HCRST));

/// JB5: the least-mutating writes that make the ARU/BAR2 (CSB/CPUCTL/mailbox) window readable at
/// raw handoff: program BAR0/BAR2 bases iff unset, set MEM_SPACE_EN only. The pre-write CFG dump
/// is itself a witness of what UEFI/EBS left (Linux-boot handoffs may differ — compare per boot).
pub fn jb5_bar2_route() {
    if !JB5_PROBE {
        return;
    }
    let r = |off: u64| unsafe { core::ptr::read_volatile((XUSB_FPCI + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe {
        core::ptr::write_volatile((XUSB_FPCI + off) as *mut u32, v)
    };
    serial_println!(":: tegra: JB5 — FPCI CFG first touch @{:#010x} (raw handoff) ::", XUSB_FPCI);
    let (cfg0, cfg1, cfg4, cfg7) = (r(0x0), r(0x4), r(0x10), r(0x1c));
    serial_println!(
        ":: tegra: JB5 — handoff FPCI: CFG_0={:#010x} CFG_1={:#010x} (io={} mem={} busmaster={}) CFG_4={:#010x} CFG_7={:#010x} ::",
        cfg0, cfg1, cfg1 & 1, (cfg1 >> 1) & 1, (cfg1 >> 2) & 1, cfg4, cfg7
    );
    if cfg4 & !0xf == 0 {
        w(0x10, 0x0361_0000);
    }
    if cfg7 & !0xf == 0 {
        w(0x1c, 0x0365_0000);
    }
    if (cfg1 >> 1) & 1 == 0 {
        w(0x4, cfg1 | 0b010); // MEM_SPACE_EN only — bus-master/io stay as handed off
    }
    serial_println!(
        ":: tegra: JB5 — minimal BAR2 route: CFG_1 rb={:#010x} CFG_4 rb={:#010x} CFG_7 rb={:#010x} ::",
        r(0x4), r(0x10), r(0x1c)
    );
}

/// JB5: the one-line state witness — xHCI cap0 + USBSTS (CNR b11 / HCH b0, direct aperture) and,
/// iff BAR2 is routed (self-guarded via CFG), the Falcon CSB CPUCTL + the ARU mailbox owner.
/// CPUCTL is THE discriminator: a running Falcon reads sane low bits; a halted one 0xffffffff.
pub fn jb5_witness(tag: &str) {
    if !JB5_PROBE {
        return;
    }
    let cap0 = unsafe { core::ptr::read_volatile(XUSB_HOST as *const u32) };
    let (usbsts, cnr, hch) = if cap0 == 0 || cap0 == 0xFFFF_FFFF {
        (0u32, 9u32, 9u32) // 9 = n/a (cap dead)
    } else {
        let cl = (cap0 & 0xff) as u64;
        let s = unsafe { core::ptr::read_volatile((XUSB_HOST + cl + 0x04) as *const u32) };
        (s, (s >> 11) & 1, s & 1)
    };
    let fr = |off: u64| unsafe { core::ptr::read_volatile((XUSB_FPCI + off) as *const u32) };
    let (cfg1, cfg7) = (fr(0x4), fr(0x1c));
    if cfg7 & !0xf != 0 && (cfg1 >> 1) & 1 == 1 {
        let br = |off: u64| unsafe { core::ptr::read_volatile((XUSB_BAR2 + off) as *const u32) };
        let bw = |off: u64, v: u32| unsafe {
            core::ptr::write_volatile((XUSB_BAR2 + off) as *mut u32, v)
        };
        bw(0x9c, (0x100u32 >> 9) & 0x7f_ffff); // CSB page for CPUCTL @0x100
        let cpuctl = br(0x2000 + (0x100 & 0x1ff) as u64);
        serial_println!(
            ":: tegra: JB5 — witness[{}]: cap0={:#010x} USBSTS={:#010x} (CNR={} HCH={}) CPUCTL={:#010x} (halted={} stopped={}) mbox_owner={:#x} ::",
            tag, cap0, usbsts, cnr, hch, cpuctl, (cpuctl >> 4) & 1, (cpuctl >> 5) & 1, br(0x010)
        );
    } else {
        serial_println!(
            ":: tegra: JB5 — witness[{}]: cap0={:#010x} USBSTS={:#010x} (CNR={} HCH={}) CPUCTL=n/a (BAR2 unrouted) ::",
            tag, cap0, usbsts, cnr, hch
        );
    }
}

/// JB6 probe (read-mostly): disambiguate why CPUCTL reads 0xffffffff on an inherited-powered XUSB
/// block. Run F showed direct ARU/BAR2 reads decode as 0x0 (block powered), but the CSB window
/// (page-select @0x9c -> data window @0x2000) reads 0xffffffff AND an ARU STREAMID_FIELD write did
/// not stick (wrote 0xe, read 0). Hypothesis: the 0x9c page-select write is not landing either, so
/// the CSB window points nowhere -> 0xffffffff (a stuck-page artifact, NOT a dead Falcon core).
/// This checks whether 0x9c accepts writes, sweeps direct ARU regs, and sweeps a few CSB addresses
/// with a page-select readback each time. STRICTLY non-destructive: the only writes are the same
/// page-select writes jb3_falcon already performs (no resets, no power/clock/PG changes).
pub fn jb6_csb_sweep() {
    if !JB5_PROBE {
        return;
    }
    let r = |off: u64| unsafe { core::ptr::read_volatile((XUSB_BAR2 + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe {
        core::ptr::write_volatile((XUSB_BAR2 + off) as *mut u32, v)
    };
    // (1) Does the CSB page-select register (ARU_C11_CSBRANGE @0x9c) accept writes at all?
    let pre = r(0x9c);
    w(0x9c, 0x1234);
    let rb = r(0x9c);
    w(0x9c, 0);
    serial_println!(
        ":: tegra: JB6 — CSB page-sel 0x9c: pre={:#010x} wrote=0x1234 rb={:#010x} STICKS={} ::",
        pre,
        rb,
        rb == 0x1234
    );
    // (2) Direct ARU/BAR2 register sweep (pure reads) — which decode (non-ff) and are non-zero?
    for &o in &[0x00u64, 0x04, 0x08, 0x0c, 0x10, 0x9c, 0xe0, 0xe4, 0xe8, 0x100, 0x104, 0x110] {
        serial_println!(":: tegra: JB6 — BAR2[{:#05x}] = {:#010x} ::", o, r(o));
    }
    // (3) CSB indirect sweep via 0x9c->0x2000, reading back the page-select each time to see if the
    // window actually re-pages. If page_rb never reflects the write, the window is a stuck page.
    let csb_r = |addr: u32| -> (u32, u32) {
        w(0x9c, (addr >> 9) & 0x7f_ffff);
        (r(0x9c), r(0x2000 + (addr & 0x1ff) as u64))
    };
    for &a in &[0x0000u32, 0x0100, 0x0104, 0x0110, 0x0f00, 0x1000, 0x4_0000] {
        let (pg, val) = csb_r(a);
        serial_println!(
            ":: tegra: JB6 — CSB[{:#07x}] page_want={:#x} page_rb={:#010x} val={:#010x} ::",
            a,
            (a >> 9) & 0x7f_ffff,
            pg,
            val
        );
    }
    w(0x9c, 0);
}

// ---------------------------------------------------------------------------------------------
// JB11 — the Falcon FIRMWARE-HEADER CENSUS: a blob-free identity readout that doubles as a
// CRCR-wall witness. Always-on in the tegra build; read-only but for one documented IOCTL write.
//
// WHY THIS EXISTS. XUSBFW (the reload scaffold below) is permanently STOPped on the blob route:
// the only T234 XUSB firmware NVIDIA ships is a signed nested container (NVGI -> RFFS -> RFRD)
// we will not reverse-engineer, and NO raw-format image exists for this SoC — mainline's
// xhci-tegra requests `nvidia/tegra{124,186,194,210}/xusb.bin` and has no tegra234 entry at all,
// because `tegra234_soc` carries no `.firmware` member. What mainline does instead on t234 is
// exactly what this census does: `tegra_xusb_init_ifr_firmware()` waits for USBSTS.CNR to clear
// and then READS THE RUNNING FALCON'S OWN HEADER over BAR2. MB2/UEFI loads the Falcon from the
// xusb-fw partition and Linux merely inherits it — as do we. So the firmware's identity is
// reachable with zero blob, zero reverse engineering, and zero risky write.
//
// MECHANISM (public GPL, from drivers/usb/host/xhci-tegra.c — free to reimplement):
//   write (FW_IOCTL_CFGTBL_READ << FW_IOCTL_TYPE_SHIFT) | byte_off -> BAR2 + XUSB_BAR2_ARU_FW_SCRATCH
//   read                                                              BAR2 + XUSB_BAR2_ARU_SMI_ARU_FW_SCRATCH_DATA0
// The header is 256 bytes; valid offsets 0x00..=0xFC. The field offsets below are the public
// `struct tegra_xusb_fw_header` order.
//
// WHY THE ONE WRITE CANNOT DISTURB INHERITED CONTROLLER STATE (the no-HCRST / JB9d-GUARD law).
// It targets XUSB_BAR2_ARU_FW_SCRATCH — the firmware's own IOCTL *request* register inside the ARU
// window at XUSB_BAR2 (0x0365_0000). That is not an xHCI register: CRCR, USBCMD, USBSTS, DCBAAP,
// ERSTBA/ERDP and the doorbell array all live in the SEPARATE host aperture at XUSB_HOST
// (0x0361_0000), which this function only ever READS (the CNR poll). There is therefore no path by
// which the scratch write can move the controller's run state, its command-ring pointer, or ring a
// doorbell — "never write CRCR at RS=1" is not merely obeyed here, it is unreachable. Production
// Linux issues this exact write on every t234 boot against a live inherited Falcon. It is also NOT
// the CSB (page-select 0x9c -> window 0x2000, the JB6 aperture) and NOT the FPCI/CFG CSB
// (EL3-FATAL on this block — JB7); it is the first-page ARU range JB6/JB8 proved decodes cleanly.
//
// ALL-ONES IS A RESULT, NOT A FAILURE. If the reads come back 0xffffffff the Falcon is in exactly
// the state JB9d-GUARD already gates on, and this census converts that into a positive
// experimental verdict — obtained with zero risky writes — instead of an absence of evidence.
//
// GATING: ALWAYS-ON in the tegra build, deliberately NOT behind the `xusbfw` feature.
//   * `xusbfw` is the wrong home. That feature exists to arm a DESTRUCTIVE path (STEP 0 is a BPMP
//     Falcon reset) and it is mutually exclusive with the shipped JB9G_NO_HCRST inherit recipe —
//     `reload_entry` returns `InheritActive` and bails BEFORE it ever reaches the aperture. A
//     census parked there would therefore never run on any boot we actually ship. It also needs a
//     bench-only blob path to be interesting at all, while this census needs nothing.
//   * The precedent is JB5/JB6: this block is already censused unconditionally on every tegra boot
//     (`jb5_witness` reads USBSTS + CPUCTL, `jb6_csb_sweep` sweeps the ARU/CSB) and JB9-A's
//     header half is the same ioctl — only fossilised behind `JB9_PROBE = false`. JB11 is that
//     half promoted to always-on and made decisive.
//   * The cost is bounded and small: one CNR poll bounded at ~200 ms (expected to exit on the
//     first read on the inherit path, where the controller is cleanly halted and never reset), plus
//     14 header dwords = 28 MMIO accesses. No allocation, no DMA, no wait that can outlive its
//     deadline.
//   * The value is per-boot: every Orin boot now answers "is the inherited Falcon answering?"
//     with a machine-checkable verdict line, which is the standing question behind the whole
//     JB9/JETSON-XCARVE wall investigation.
// Non-tegra builds are untouched: the whole module is `#[cfg(feature = "tegra")]`.
// ---------------------------------------------------------------------------------------------

/// BAR2 offset of the firmware IOCTL request register (`XUSB_BAR2_ARU_FW_SCRATCH`).
const JB11_ARU_FW_SCRATCH: u64 = 0x1000;
/// BAR2 offset of the firmware IOCTL result register (`XUSB_BAR2_ARU_SMI_ARU_FW_SCRATCH_DATA0`).
const JB11_ARU_FW_SCRATCH_DATA0: u64 = 0x01c;
/// `FW_IOCTL_TYPE_SHIFT` / `FW_IOCTL_CFGTBL_READ` — the config-table read opcode.
const JB11_IOCTL_TYPE_SHIFT: u32 = 24;
const JB11_IOCTL_CFGTBL_READ: u32 = 17;

/// One header dword by byte offset (`0x00..=0xFC`). Write the ioctl request, read the result.
/// Device-nGnRE ordering on the BAR2 aperture is non-reordering, so the read cannot pass the
/// write; this is the same two-access sequence JB8 proved answers on this silicon.
fn jb11_hdr_dword(off: u32) -> u32 {
    unsafe {
        core::ptr::write_volatile(
            (XUSB_BAR2 + JB11_ARU_FW_SCRATCH) as *mut u32,
            (JB11_IOCTL_CFGTBL_READ << JB11_IOCTL_TYPE_SHIFT) | (off & 0xff),
        );
        core::ptr::read_volatile((XUSB_BAR2 + JB11_ARU_FW_SCRATCH_DATA0) as *const u32)
    }
}

/// Decode a 32-bit UNIX epoch to (y, m, d, hh, mm, ss) UTC. Howard Hinnant's `civil_from_days`,
/// integer-only and positive-only (a u32 epoch is always >= 1970-01-01), so it is a handful of
/// divisions — cheap enough to run inline in a boot census.
fn jb11_utc(epoch: u32) -> (u64, u64, u64, u64, u64, u64) {
    let (days, rem) = (epoch as u64 / 86_400, epoch as u64 % 86_400);
    let z = days + 719_468; // days from 0000-03-01 to 1970-01-01
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// JB11: read the resident Falcon's firmware header over BAR2 and witness what it says — or
/// witness that it says nothing, which is the CRCR-wall confirmation. See the block comment above
/// for the mechanism, the write-safety argument, and the always-on gating decision.
///
/// Preconditions are physical and self-checked: the XUSB partition must be ON (the call site is
/// inside `jb5_pg_on`'s proof — the JX1 gated-block rule) and BAR2 must be routed (`jb5_bar2_route`;
/// re-checked here via `jb9_bar2_routed`). Everything else is a bounded read.
pub fn jb11_fw_header_census(tag: &str) {
    // Public `struct tegra_xusb_fw_header` byte offsets.
    const H_BOOT_CODETAG: u32 = 0x08;
    const H_BOOT_CODESIZE: u32 = 0x0c;
    const H_RODATA_IMG_OFFSET: u32 = 0x18;
    const H_FWIMG_CKSUM: u32 = 0x28;
    const H_FWIMG_CREATED_TIME: u32 = 0x2c;
    const H_L2_IMEM_START: u32 = 0x40;
    const H_L2_IMEM_END: u32 = 0x44;
    const H_VERSION_ID: u32 = 0x48;
    const H_FWIMG_LEN: u32 = 0x64;
    const H_MAGIC: u32 = 0x68;

    // JX1 discipline: name the access class BEFORE the first touch, so a boot that dies here has a
    // last serial line that names the killer aperture.
    serial_println!(
        ":: tegra: JB11 [{}] — Falcon fw-header census: BAR2+{:#05x} (IOCTL_CFGTBL_READ) -> BAR2+{:#05x}; no xHCI operational register is written ::",
        tag, JB11_ARU_FW_SCRATCH, JB11_ARU_FW_SCRATCH_DATA0
    );
    if !jb9_bar2_routed() {
        serial_println!(":: tegra: JB11 [{}] — BAR2 unrouted; census SKIPPED (guard) ::", tag);
        return;
    }
    let cap0 = unsafe { core::ptr::read_volatile(XUSB_HOST as *const u32) };
    if cap0 == 0 || cap0 == 0xFFFF_FFFF {
        serial_println!(
            ":: tegra: JB11 [{}] — cap0={:#010x} (host block not alive); census SKIPPED ::",
            tag, cap0
        );
        return;
    }

    // Mirror mainline: wait for USBSTS.CNR (bit 11) to clear before trusting header reads. No CNR
    // wait exists at this point in our boot order — the shared driver's `wait_for_cnr_clear` runs
    // inside `xhci::init`/`init_interrupter`, far downstream, and on the JB9G_NO_HCRST inherit path
    // `xhci::init` is never called at all — so this is the bounded one mainline's IFR path uses
    // (~200 ms). Pure reads of the host aperture; expected to exit on the first poll on an inherit
    // boot (the controller is cleanly halted by UEFI, never reset).
    let usbsts_a = XUSB_HOST + (cap0 & 0xff) as u64 + 0x04;
    let t0 = cntpct();
    let deadline = t0.wrapping_add(cntfrq() / 5);
    let mut usbsts = unsafe { core::ptr::read_volatile(usbsts_a as *const u32) };
    let mut polls = 1u32;
    while (usbsts >> 11) & 1 == 1 {
        if cntpct().wrapping_sub(deadline) < (1u64 << 63) {
            break; // deadline passed (wrap-safe)
        }
        core::hint::spin_loop();
        usbsts = unsafe { core::ptr::read_volatile(usbsts_a as *const u32) };
        polls += 1;
    }
    let cnr = (usbsts >> 11) & 1;
    let waited_us = cntpct().wrapping_sub(t0).saturating_mul(1_000_000) / cntfrq();
    serial_println!(
        ":: tegra: JB11 [{}] — CNR gate: USBSTS={:#010x} CNR={} after {} poll(s) / ~{} us — {} ::",
        tag, usbsts, cnr, polls, waited_us,
        if cnr == 0 {
            "READY (mainline waits for exactly this before reading the header)"
        } else {
            "STILL SET at the ~200 ms bound — the reads below are UNTRUSTED, witnessed anyway"
        }
    );

    // The header. 14 dwords, one ioctl round trip each.
    let codetag = jb11_hdr_dword(H_BOOT_CODETAG);
    let codesize = jb11_hdr_dword(H_BOOT_CODESIZE);
    let rodata_off = jb11_hdr_dword(H_RODATA_IMG_OFFSET);
    let cksum = jb11_hdr_dword(H_FWIMG_CKSUM);
    let created = jb11_hdr_dword(H_FWIMG_CREATED_TIME);
    let imem_start = jb11_hdr_dword(H_L2_IMEM_START);
    let imem_end = jb11_hdr_dword(H_L2_IMEM_END);
    let version_id = jb11_hdr_dword(H_VERSION_ID);
    let fwimg_len = jb11_hdr_dword(H_FWIMG_LEN);
    let magic0 = jb11_hdr_dword(H_MAGIC);
    let magic1 = jb11_hdr_dword(H_MAGIC + 4);

    // The wall signature: EVERY anchor dword all-ones. That is the JB9d-GUARD state, and naming it
    // from the header path is a genuine verdict, not a failed read.
    let walled = [codetag, codesize, version_id, magic0, magic1]
        .iter()
        .all(|&v| v == 0xFFFF_FFFF);
    // The JB8 liveness anchor: a served header has a sane boot_codesize.
    let served = codesize != 0 && codesize != 0xFFFF_FFFF;

    if walled {
        serial_println!(
            ":: tegra: JB11 [{}] — raw: codetag={:#010x} codesize={:#010x} version_id={:#010x} magic={:#010x}{:08x} ::",
            tag, codetag, codesize, version_id, magic1, magic0
        );
        serial_println!(
            ":: tegra: JB11 [{}] — verdict: Falcon not answering — inherited-state wall CONFIRMED from the header path (all-ones on every anchor dword; obtained with zero risky writes) ::",
            tag
        );
        return;
    }
    if !served {
        serial_println!(
            ":: tegra: JB11 [{}] — raw: codetag={:#010x} codesize={:#010x} version_id={:#010x} created={:#010x} magic={:#010x}{:08x} ::",
            tag, codetag, codesize, version_id, created, magic1, magic0
        );
        serial_println!(
            ":: tegra: JB11 [{}] — verdict: header UNSERVED but NOT all-ones — the ARU aperture decodes and the Falcon returns no config table; INCONCLUSIVE (not the wall signature) ::",
            tag
        );
        return;
    }

    // A live Falcon. Decode what it reports.
    let mb = [
        magic0 as u8, (magic0 >> 8) as u8, (magic0 >> 16) as u8, (magic0 >> 24) as u8,
        magic1 as u8, (magic1 >> 8) as u8, (magic1 >> 16) as u8, (magic1 >> 24) as u8,
    ];
    let pr = |b: u8| if (0x20..=0x7e).contains(&b) { b as char } else { '.' };
    let (yy, mo, dd, hh, mi, ss) = jb11_utc(created);
    serial_println!(
        ":: tegra: JB11 [{}] — fw ident: magic=\"{}{}{}{}{}{}{}{}\" ({:#010x}{:08x}) version_id={:#010x} (Version: {:2x}.{:02x}) ::",
        tag, pr(mb[0]), pr(mb[1]), pr(mb[2]), pr(mb[3]), pr(mb[4]), pr(mb[5]), pr(mb[6]), pr(mb[7]),
        magic1, magic0, version_id, (version_id >> 24) & 0xff, (version_id >> 16) & 0xff
    );
    serial_println!(
        ":: tegra: JB11 [{}] — fw built: created={} (epoch) = {}-{:02}-{:02}T{:02}:{:02}:{:02}Z UTC ::",
        tag, created, yy, mo, dd, hh, mi, ss
    );
    serial_println!(
        ":: tegra: JB11 [{}] — fw image: boot_codetag={:#010x} boot_codesize={} B rodata_img_offset={:#010x} fwimg_len={} B cksum={:#010x} ::",
        tag, codetag, codesize, rodata_off, fwimg_len, cksum
    );
    serial_println!(
        ":: tegra: JB11 [{}] — fw L2 IMEM: start={:#010x} end={:#010x} (span={} B) ::",
        tag, imem_start, imem_end, imem_end.wrapping_sub(imem_start)
    );
    serial_println!(
        ":: tegra: JB11 [{}] — verdict: Falcon ANSWERING — the resident firmware served its own header over BAR2; no inherited-state wall on the header path ::",
        tag
    );
}

// JB7 (arc A) — RETIRED: the alternate FPCI/CFG CSB cross-read is EL3-FATAL on the inherited halted
// block. It was meant to disambiguate dead-core vs BAR2-aperture by reading CPUCTL through
// `UsbFalconLib.c::FalconMapReg`'s else-branch (page-select @ XUSB_FPCI+0x41c, data window @
// XUSB_FPCI+0x800 + (addr & 0x1ff), the path FalconUtil uses when XusbHostBase2Addr==0). On the first
// JB7 metal boot (native-microSD, 2026-07-08) the FIRST access to that window trapped to BL31 —
// `Unhandled Exception in EL3`, `esr_el3=0xbe000011` (EC 0x2F = SError, an async CBB/fabric abort),
// `far_el3=0` — and killed the boot before the clock census / JB1c / JB2b. edk2's FalconUtil uses this
// path INSIDE UEFI where the block is managed; post-EBS on our halted block the CFG CSB aperture is
// unrouted, so touching it is the JX1 EL3-fatal class (an SError cannot be guarded from EL2 — chasing
// it is a STOP tripwire). The BAR2 CSB aperture (`jb6_csb_sweep`) is the only usable path and cleanly
// reads 0xffffffff with a working page-select — the dead-core conclusion stands without this
// cross-check, and the two apertures behaving differently (BAR2 decodes-but-floats vs CFG unrouted)
// is itself the finding.

// ---------------------------------------------------------------------------------------------
// JB9: FW-liveness without CPUCTL + DMA-path forensics.
//
// The JB8 metal verdict this arc stands on: the Falcon was NEVER halted — CPUCTL=0xffffffff is a
// CSB priv-lock read (signed FW at raised priv; external CSB reads float all-ones) taken on a
// provably RUNNING core (pre-EBS: HCH=0, CNR=0, USB enumerating), so CPUCTL/BOOTVEC are never a
// liveness witness on this firmware. The real failure is the DMA path: at kernel time the
// register path is healthy (ports link-train to U0, CCS=1 PED=1) but enable-slot watchdog-times-
// out — command-ring fetches / event-ring writebacks never touch DRAM, with a clean SMMU/MC fault
// census. Two probes, in order:
//   A (jb9_fw_alive) — a CPUCTL-free FW-liveness witness: fw-header identity reads through the
//     ARU ioctl (the one aperture JB8 proved answers on a locked core), an ARU scratch heartbeat
//     (two sweeps ~10 ms apart — a live FW that updates any mailbox/scratch word shows as a
//     diff), and the MSG_ENABLED handshake with a PATIENT retry ladder (JB3 tried once with a
//     ~200 µs poll; a busy FW at handoff time deserves 5 spaced attempts over ~50 ms). Run at
//     three points: raw-handoff, post-xhc-restart, post-enum-attempt.
//   B (jb9_stream_dump in smmu_tegra + jb9_fw_sid_view + jb9_ring_scan) — forensics captured
//     WHILE enable-slot is pending (the JB8 log: ~340 ms per watchdog attempt, 3 per port — the
//     pump loop fires the capture at t≈200 ms and t≈5 s into the window): the SMMU binding for
//     the XUSB stream at write time, the MC override/error state at that instant, the SID the FW
//     itself is configured to tag (ARU STREAMID_FIELD), and a near-target scan for command-
//     completion TRBs that landed NEAR the event ring (a stale translation lands DMA at a wrong
//     PA with zero faults — if the write went somewhere nearby, the scan names where).
// Everything is read-only except the mailbox handshake (the same writes jb3_aru_probe already
// makes) and stays inside apertures prior arcs proved safe: ARU/BAR2 first page (JB6/JB8),
// SMMU/MC (JB3), RAM (the scan). No CFG-path CSB, no FPCI BAR dereference, no AO touch — the AO
// IFRDMA_STREAMID read is SKIPped by design (no source-verified register offset; an unverified
// aperture is the JX1 EL3-fatal class).
// ---------------------------------------------------------------------------------------------

/// JB9 master gate, sibling of JB5_PROBE: probe media only. When false every jb9_* is a silent
/// no-op (call sites stay wired).
///
/// JB10: default-OFF now the JB9 verdict is in (USB enumerates on the Orin). This silences the
/// diagnostic suite on a production inherit boot — `jb9_fw_alive` (the FW-liveness witness),
/// `jb9f_inherit_run_probe` (the RS=1/re-halt discriminator that PROVED the no-HCRST recipe), and
/// the `jb9_fw_sid_view`/`jb9_ring_scan`/`jb9_stream_dump` forensic captures in the pump window.
/// It does NOT touch the working recipe: JB9i eviction and the JB9g no-HCRST takeover gate on
/// `JB9G_NO_HCRST`, not this flag, so `jb2b_attach` is byte-identical with the probes off. Flip
/// back to `true` at the next attended bench when a downstream device's failed enumeration needs
/// that telemetry (the forensic kit is deliberately KEPT, not deleted — see arch_arm64.md §JB10).
/// Sibling note: `JB5_PROBE` must STAY true — `jb5_bar2_route` (the load-bearing minimal FPCI
/// decode the inherit boot needs) rides that gate; it is NOT interchangeable with this one.
pub const JB9_PROBE: bool = false;

/// JB9g: take over the inherited (cleanly-halted) controller WITHOUT HCRST — the reset is what
/// kills the Falcon's service loop on this firmware (JB9f: engine posts on bare RS=1; JB9e:
/// dead after HCRST even into an all-low ring). False restores the JB2b halt+HCRST init.
pub const JB9G_NO_HCRST: bool = true;

/// JB9h: skip the ENTIRE JB3 fabric chain (SMMU open / MC SID / FPCI full-enable / ARU restore /
/// locked-CSB restart attempt / padctl re-powerup / JB9b levers) on the inherit path. JB9f proved
/// the inherited fabric passes the FW's DMA as-is at raw handoff; the chain was built for the
/// halted-Falcon world and rewrites that working state — prime suspect for JB9g's still-dead
/// enable-slot (jb3_open_stream flips the SMMU from CLIENTPD=1 pass-through to matched-translate
/// + USFCFG=1 fault-everything-else, on a FW whose true runtime SIDs we've never observed).
pub const JB9H_SKIP_CHAIN: bool = true;

/// The ARU register span the heartbeat sweeps: [0x0, 0x140) — the first-page range jb6_csb_sweep
/// / JB8 metal proved decodes cleanly (reads return data, no fault) on this block.
const JB9_HB_WORDS: usize = 0x140 / 4;

/// BAR2 routed + mem-decode on — the same self-guard jb5_witness uses before an ARU touch.
fn jb9_bar2_routed() -> bool {
    let fr = |off: u64| unsafe { core::ptr::read_volatile((XUSB_FPCI + off) as *const u32) };
    fr(0x1c) & !0xf != 0 && (fr(0x4) >> 1) & 1 == 1
}

/// JB9-A: the CPUCTL-free FW-liveness witness. Prints one FW-ALIVE / FW-SILENT verdict line per
/// probe point; ALIVE = the firmware demonstrably did something (served the header ioctl AND
/// (answered the mailbox OR updated a scratch word between sweeps)).
pub fn jb9_fw_alive(tag: &str) {
    if !JB9_PROBE {
        return;
    }
    if !jb9_bar2_routed() {
        serial_println!(":: tegra: JB9-A [{}] — BAR2 unrouted; SKIP ::", tag);
        return;
    }
    let r = |off: u64| unsafe { core::ptr::read_volatile((XUSB_BAR2 + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe { core::ptr::write_volatile((XUSB_BAR2 + off) as *mut u32, v) };
    let fw_hdr = |byte_off: u32| {
        w(0x1000, (17u32 << 24) | byte_off); // FW_IOCTL_CFGTBL_READ (JB3 boot-12, JB8-proven)
        r(0x1c)
    };

    // 1. FW-header identity: codetag/codesize (the JB8 liveness anchor: 0xc85f when the resident
    //    image answers) + fwimg_checksum @0x28 / fwimg_created_time @0x2c (mainline
    //    tegra_xusb_fw_header layout) — a wrong/floating aperture cannot fake four coherent words.
    let (codetag, codesize) = (fw_hdr(0x08), fw_hdr(0x0c));
    let (cksum, created) = (fw_hdr(0x28), fw_hdr(0x2c));
    let hdr_ok = codesize != 0 && codesize != 0xffff_ffff;
    serial_println!(
        ":: tegra: JB9-A [{}] — fw hdr: codetag={:#010x} codesize={:#010x} checksum={:#010x} created={:#010x} ({}) ::",
        tag, codetag, codesize, cksum, created,
        if hdr_ok { "ioctl SERVED" } else { "ioctl DEAD" }
    );

    // 2. Scratch heartbeat: sweep the proven ARU range twice, ~10 ms apart. Any word a live FW
    //    updates between sweeps is a heartbeat CPUCTL can never show.
    let mut snap = [0u32; JB9_HB_WORDS];
    for (i, s) in snap.iter_mut().enumerate() {
        *s = r(4 * i as u64);
    }
    udelay(10_000);
    let mut changed = 0u32;
    for (i, &s) in snap.iter().enumerate() {
        let now = r(4 * i as u64);
        if now != s {
            changed += 1;
            if changed <= 8 {
                serial_println!(
                    ":: tegra: JB9-A [{}] — heartbeat: BAR2[{:#05x}] {:#010x} -> {:#010x} ::",
                    tag, 4 * i, s, now
                );
            }
        }
    }
    serial_println!(
        ":: tegra: JB9-A [{}] — heartbeat: {} of {} ARU words changed over ~10 ms ::",
        tag, changed, JB9_HB_WORDS
    );

    // 3. MSG_ENABLED, patiently: 5 attempts, each = claim -> send -> ~10 ms ACK poll -> release,
    //    ~10 ms apart (total ~100 ms vs JB3's one ~200 µs try). Serviced = DATA_OUT went nonzero
    //    OR the firmware released/took the owner semaphore itself.
    let mut serviced = false;
    let mut attempts_used = 0u32;
    for attempt in 0..5u32 {
        attempts_used = attempt + 1;
        let own0 = r(0x010);
        w(0x010, 2); // OWNER_SW
        if r(0x010) != 2 {
            // Can't claim — either the FW holds it (itself a liveness datum) or the semaphore
            // is dead. Log once, wait, retry.
            if attempt == 0 {
                serial_println!(
                    ":: tegra: JB9-A [{}] — mbox owner claim refused (pre={:#x} rb={:#x}) ::",
                    tag, own0, r(0x010)
                );
            }
            udelay(10_000);
            continue;
        }
        w(0x008, 5u32 << 24); // DATA_IN: MBOX_CMD_MSG_ENABLED
        w(0x004, r(0x004) | (1 << 31) | (1 << 27)); // CMD: INT_EN | DEST_FALC
        let deadline = cntpct().wrapping_add(cntfrq().saturating_mul(10) / 1000); // ~10 ms
        loop {
            if r(0x00c) != 0 || r(0x010) != 2 {
                serviced = true;
                break;
            }
            if cntpct().wrapping_sub(deadline) < (1u64 << 63) {
                break; // deadline passed (wrap-safe)
            }
            core::hint::spin_loop();
        }
        let (dout, own) = (r(0x00c), r(0x010));
        if own == 2 {
            w(0x010, 0); // release for the next attempt / the firmware
        }
        if serviced {
            serial_println!(
                ":: tegra: JB9-A [{}] — MSG_ENABLED serviced on attempt {}: data_out={:#010x} owner={:#x} ::",
                tag, attempt + 1, dout, own
            );
            break;
        }
        udelay(10_000);
    }
    serial_println!(
        ":: tegra: JB9-A [{}] — verdict: {} (hdr={} heartbeat={} mbox={} attempts={}) ::",
        tag,
        if hdr_ok && (serviced || changed > 0) { "FW-ALIVE" } else { "FW-SILENT" },
        hdr_ok, changed, serviced, attempts_used
    );
}

/// JB9-B: the SID the firmware itself is configured to tag its DMA with — ARU IFRDMA_CFG0/1 +
/// STREAMID_FIELD (BAR2 +0xe0/0xe4/0xe8, the words jb3_aru_probe restores), plus the AO-side
/// IFR-autoboot registers (the JB8 source read: `IFRDMA_CFG0` @AO+0x1bc, `CFG1` @AO+0x1c0,
/// `IFRDMA_STREAMID` @AO+0x1c4 — the SID UEFI told the Falcon's ROM DMA engine to tag with).
/// `ao` = padctl reg region 1 base, DTB-resolved by the caller; None prints a SKIP (never a
/// guessed address — the JX1 rule).
pub fn jb9_fw_sid_view(tag: &str, ao: Option<u64>) {
    if !JB9_PROBE || !jb9_bar2_routed() {
        return;
    }
    let r = |off: u64| unsafe { core::ptr::read_volatile((XUSB_BAR2 + off) as *const u32) };
    serial_println!(
        ":: tegra: JB9-B [{}] — FW SID view (ARU): IFRDMA_CFG0={:#010x} CFG1={:#010x} STREAMID_FIELD={:#010x} ::",
        tag, r(0x0e0), r(0x0e4), r(0x0e8)
    );
    match ao {
        Some(base) => {
            // JX1 discipline: a new address class announces itself BEFORE the first read, so a
            // dead boot's last line names the killer aperture. (padctl AO is always-on and the
            // base is DTB-resolved, but the discipline costs one line.)
            serial_println!(
                ":: tegra: JB9-B [{}] — padctl AO @{:#010x} first touch ::",
                tag, base
            );
            let ar = |off: u64| unsafe { core::ptr::read_volatile((base + off) as *const u32) };
            serial_println!(
                ":: tegra: JB9-B [{}] — FW SID view (AO @{:#010x}): IFRDMA_CFG0={:#010x} CFG1={:#010x} STREAMID={:#010x} ::",
                tag, base, ar(0x1bc), ar(0x1c0), ar(0x1c4)
            );
        }
        None => serial_println!(
            ":: tegra: JB9-B [{}] — AO IFRDMA view: SKIP (padctl reg region 1 not in DTB) ::",
            tag
        ),
    }
}

/// JB9-B: near-target scan — did the event-ring writeback land NEAR where it should have?
/// Scans ±2 MiB of identity-mapped RAM around the event ring for the command-completion
/// fingerprint (TRB qword0 pointing into the command ring's page + TRB type 33 in dword3) — a
/// stale IOVA≠PA translation would deposit exactly that pattern at a wrong PA. Also dumps the
/// event ring's first four TRB slots (did anything land AT target?). Read-only RAM walk, ~4 MiB
/// at 16-byte steps, clamped to the DRAM base so it can never wander into device space.
pub fn jb9_ring_scan(evt_phys: u64, cmd_phys: u64, tag: &str) {
    if !JB9_PROBE || evt_phys == 0 || cmd_phys == 0 {
        return;
    }
    let rd64 = |a: u64| unsafe { core::ptr::read_volatile(a as *const u64) };
    let rd32 = |a: u64| unsafe { core::ptr::read_volatile(a as *const u32) };
    // At target: the ring itself.
    for i in 0..4u64 {
        let t = evt_phys + 16 * i;
        serial_println!(
            ":: tegra: JB9-B [{}] — evt-ring[{}] @{:#x}: {:#010x} {:#010x} {:#010x} {:#010x} ::",
            tag, i, t, rd32(t), rd32(t + 4), rd32(t + 8), rd32(t + 12)
        );
    }
    // Near target: the fingerprint sweep. Orin DRAM starts at 0x8000_0000 (the heap the rings
    // live in is identity-mapped RAM just above it) — clamp so the scan stays in RAM.
    const DRAM_BASE: u64 = 0x8000_0000;
    let start = evt_phys.saturating_sub(2 * 1024 * 1024).max(DRAM_BASE) & !15;
    let end = evt_phys + 2 * 1024 * 1024;
    let cmd_page = cmd_phys & !0xfff;
    let mut hits = 0u32;
    let mut p = start;
    while p < end {
        let qw0 = rd64(p);
        if qw0 >= cmd_page && qw0 < cmd_page + 0x1000 {
            let d3 = rd32(p + 12);
            if (d3 >> 10) & 0x3f == 33 {
                hits += 1;
                if hits <= 4 {
                    let (dir, delta) = if p >= evt_phys {
                        ("+", p - evt_phys)
                    } else {
                        ("-", evt_phys - p)
                    };
                    serial_println!(
                        ":: tegra: JB9-B [{}] — near-target HIT @{:#x} (evt-ring{}{:#x}): {:#010x} {:#010x} {:#010x} {:#010x} ::",
                        tag, p, dir, delta, rd32(p), rd32(p + 4), rd32(p + 8), d3
                    );
                }
            }
        }
        p += 16;
    }
    serial_println!(
        ":: tegra: JB9-B [{}] — near-target scan [{:#x}..{:#x}) cmd-page={:#x}: {} completion-TRB hit(s) ::",
        tag, start, end, cmd_page, hits
    );
}

/// JB9f (bench 2026-07-08): the INHERIT-RUN discriminator. The loader demonstrably uses the
/// engine's DMA (it reads kernel.elf off the USB stick), so the kill is inside
/// exit_boot_services — and the only actor is generic XhciDxe's XhcHaltHC (USBCMD.RS=0, no
/// reset). That leaves UEFI's whole DMA world (DCBAA, command ring, event ring, ERSTBA/ERDP)
/// programmed at handoff, and ERSTBA/ERDP are READABLE. This probe, at raw-handoff before any
/// UnaOS touch: snapshot the inherited event ring, set RS=1 with NO reset, wait ~200 ms — live
/// ports (pads kept up by the JB6 shim) generate Port-Status-Change events autonomously — then
/// re-dump. New TRBs = firmware + DMA are FINE and our own HCRST is what parks the FW (kernel
/// fix: take over without reset). Nothing = the FW parks at RS=0 itself, and preventing the EBS
/// halt (UEFI-side) is the only NS road left. Controller is re-halted afterwards, so the JB
/// chain and attach see the same halted handoff as every prior boot.
pub fn jb9f_inherit_run_probe() {
    if !JB9_PROBE {
        return;
    }
    let cap0 = unsafe { core::ptr::read_volatile(XUSB_HOST as *const u32) };
    if cap0 == 0 || cap0 == 0xFFFF_FFFF {
        serial_println!(":: tegra: JB9f — cap0 dead; SKIP ::");
        return;
    }
    let caplen = (cap0 & 0xff) as u64;
    let op = XUSB_HOST + caplen;
    let rtsoff = unsafe { core::ptr::read_volatile((XUSB_HOST + 0x18) as *const u32) } & !0x1f;
    let ir0 = XUSB_HOST + rtsoff as u64 + 0x20;
    let r32 = |a: u64| unsafe { core::ptr::read_volatile(a as *const u32) };
    let r64 = |a: u64| unsafe { core::ptr::read_volatile(a as *const u64) };
    let w32 = |a: u64, v: u32| unsafe { core::ptr::write_volatile(a as *mut u32, v) };
    let (usbcmd, usbsts) = (r32(op), r32(op + 0x04));
    let erstsz = r32(ir0 + 0x08);
    let erstba = r64(ir0 + 0x10) & !0x3f;
    let erdp = r64(ir0 + 0x18) & !0xf;
    serial_println!(
        ":: tegra: JB9f — inherited: USBCMD={:#010x} USBSTS={:#010x} (HCH={}) ERSTSZ={} ERSTBA={:#x} ERDP={:#x} ::",
        usbcmd, usbsts, usbsts & 1, erstsz, erstba, erdp
    );
    // The inherited ERST entry names UEFI's event ring (boot-services RAM, identity-mapped,
    // dead-but-intact post-EBS). Bail if anything reads unprogrammed.
    if erstba == 0 || erstsz == 0 || erstba >= 1 << 40 {
        serial_println!(":: tegra: JB9f — no inherited interrupter; SKIP ::");
        return;
    }
    let ring_base = r64(erstba) & !0xf;
    let ring_size = (r32(erstba + 8) & 0xffff) as u64;
    serial_println!(
        ":: tegra: JB9f — inherited event ring @{:#x} ({} TRBs); snapshotting @ERDP ::",
        ring_base, ring_size
    );
    if ring_base == 0 || ring_base >= 1 << 40 || ring_size == 0 || ring_size > 4096 {
        serial_println!(":: tegra: JB9f — inherited ERST entry implausible; SKIP ::");
        return;
    }
    // Snapshot 8 TRBs starting at ERDP (wrapping the segment) — where the next posts land.
    let idx0 = if erdp >= ring_base { (erdp - ring_base) / 16 } else { 0 };
    let trb_at = |i: u64| ring_base + ((idx0 + i) % ring_size) * 16;
    let mut before = [0u32; 8];
    for (i, b) in before.iter_mut().enumerate() {
        *b = r32(trb_at(i as u64) + 12); // control word (cycle + type) is the change detector
    }
    // RS=1, no reset — resume the engine on UEFI's own state.
    w32(op, r32(op) | 1);
    let mut spins = 0u32;
    while r32(op + 0x04) & 1 != 0 && spins < 100_000 {
        spins += 1;
    }
    udelay(200_000); // 200 ms — port-status-change events post autonomously if the FW runs
    let mut changed = 0u32;
    for (i, &b) in before.iter().enumerate() {
        let now = r32(trb_at(i as u64) + 12);
        if now != b {
            changed += 1;
            let t = trb_at(i as u64);
            serial_println!(
                ":: tegra: JB9f — NEW TRB @{:#x}: {:#010x} {:#010x} {:#010x} {:#010x} (type={}) ::",
                t, r32(t), r32(t + 4), r32(t + 8), r32(t + 12), (r32(t + 12) >> 10) & 0x3f
            );
        }
    }
    serial_println!(
        ":: tegra: JB9f — inherit-run verdict: {} (USBSTS={:#010x} HCH={} after 200 ms) ::",
        if changed > 0 {
            "ENGINE POSTED ⭐ — FW alive on RS=1, our HCRST is the killer"
        } else {
            "nothing posted — FW parks at the EBS RS=0 halt itself"
        },
        r32(op + 0x04),
        r32(op + 0x04) & 1
    );
    // Halt again — hand the JB chain the same halted controller as every prior boot.
    w32(op, r32(op) & !1);
    let mut spins = 0u32;
    while r32(op + 0x04) & 1 == 0 && spins < 100_000 {
        spins += 1;
    }
}

/// JB9e (bench 2026-07-08): the LOW-RING discriminator. JB9d's bracket proved the mailbox is
/// silent even pre-EBS on a provably-DMA-ing firmware — mailbox silence means nothing. What
/// actually differs between UEFI's working DMA and ours is WHERE the DMA-write targets live:
/// every UEFI buffer is low EfiBootServicesData; our event ring + ERST are statics in the kernel
/// image, which the loader places HIGH (~0x2_5e40_0000, above 4 GiB), while the command ring /
/// DCBAA / contexts are heap allocations at the DRAM base (0x8000_xxxx, 32-bit reachable). The
/// observed failure — command fetch targets low, event writes high, ONLY the event writes never
/// happen — fits "the inherited FPCI/AXI address path drops device writes above 4 GiB" exactly.
/// This probe hand-programs interrupter 0 with an ALL-LOW event ring + ERST (heap), pushes one
/// NOOP command on an all-low command ring, rings doorbell 0, and polls the low ring for the
/// completion TRB. LANDED = the whole JB3→JB9 DMA mystery is the event ring's address; the fix
/// is relocating the event ring/ERST to heap (an event.rs/mod.rs change — outside this lane,
/// STOP-and-report). SILENT = the high-address theory dies too. Runs before jb2b_attach; the
/// attach's own halt+HCRST wipes every register this probe programs.
pub fn jb9e_low_ring_probe() {
    if !JB9_PROBE {
        return;
    }
    let cap0 = unsafe { core::ptr::read_volatile(XUSB_HOST as *const u32) };
    if cap0 == 0 || cap0 == 0xFFFF_FFFF {
        serial_println!(":: tegra: JB9e — cap0 dead; SKIP ::");
        return;
    }
    // Reset to a clean halted state first (the same init jb2b_attach uses).
    xhci::init(XUSB_HOST);

    let caplen = (cap0 & 0xff) as u64;
    let op = XUSB_HOST + caplen;
    let dboff = unsafe { core::ptr::read_volatile((XUSB_HOST + 0x14) as *const u32) } & !0x3;
    let rtsoff = unsafe { core::ptr::read_volatile((XUSB_HOST + 0x18) as *const u32) } & !0x1f;
    let ir0 = XUSB_HOST + rtsoff as u64 + 0x20;
    let r32 = |a: u64| unsafe { core::ptr::read_volatile(a as *const u32) };
    let w32 = |a: u64, v: u32| unsafe { core::ptr::write_volatile(a as *mut u32, v) };
    let w64 = |a: u64, v: u64| unsafe { core::ptr::write_volatile(a as *mut u64, v) };

    // All-low DMA structures, straight off the heap (the DRAM-base pool the command ring already
    // lives in). 64-byte alignment from the allocator satisfies ERST/ring requirements.
    let evt = xhci::ring::allocate_buffer(4096) as u64; // 256 TRBs
    let erst = xhci::ring::allocate_buffer(64) as u64;
    let cmd = xhci::ring::allocate_ring(16) as u64;
    let dcbaa = xhci::ring::allocate_buffer(2048) as u64;
    serial_println!(
        ":: tegra: JB9e — low-ring probe: evt={:#x} erst={:#x} cmd={:#x} dcbaa={:#x} (all must be <4GiB) ::",
        evt, erst, cmd, dcbaa
    );
    if evt == 0 || erst == 0 || cmd == 0 || dcbaa == 0 || evt >= 1 << 32 {
        serial_println!(":: tegra: JB9e — allocation unusable; SKIP ::");
        return;
    }
    unsafe {
        // ERST[0] = { ring_address: evt, size: 256 }
        core::ptr::write_volatile(erst as *mut u64, evt);
        core::ptr::write_volatile((erst + 8) as *mut u32, 256);
        core::ptr::write_volatile((erst + 12) as *mut u32, 0);
        // One NOOP command TRB (type 23), cycle=1, at cmd[0].
        core::ptr::write_volatile(cmd as *mut u64, 0);
        core::ptr::write_volatile((cmd + 8) as *mut u32, 0);
        core::ptr::write_volatile((cmd + 12) as *mut u32, (23 << 10) | 1);
    }
    dsb();
    // Program the halted controller: 1 slot, low DCBAA, low command ring (RCS=1), interrupter 0
    // fully low. No IMAN/INTE — pure polling.
    w32(op + 0x38, 1); // CONFIG.MaxSlotsEn
    w64(op + 0x30, dcbaa); // DCBAAP
    w64(op + 0x18, cmd | 1); // CRCR, RCS=1
    w32(ir0 + 0x08, 1); // ERSTSZ
    w64(ir0 + 0x10, erst); // ERSTBA
    w64(ir0 + 0x18, evt); // ERDP
    dsb();
    w32(op + 0x0, r32(op + 0x0) | 1); // USBCMD.RS
    // Wait for HCH to clear, then ring doorbell 0 (host controller command doorbell).
    let mut spins = 0u32;
    while r32(op + 0x04) & 1 != 0 && spins < 100_000 {
        spins += 1;
    }
    dsb();
    w32(XUSB_HOST + dboff as u64, 0);
    dsb();
    // Poll the LOW event ring for the completion (cycle bit 1 in TRB[0].control), bounded 500 ms.
    let deadline = cntpct().wrapping_add(cntfrq() / 2);
    let mut landed = false;
    loop {
        let ctrl = unsafe { core::ptr::read_volatile((evt + 12) as *const u32) };
        if ctrl & 1 != 0 {
            landed = true;
            break;
        }
        if cntpct().wrapping_sub(deadline) < (1u64 << 63) {
            break;
        }
        core::hint::spin_loop();
    }
    let (d0, d1, d2, d3) = unsafe {
        (
            core::ptr::read_volatile(evt as *const u32),
            core::ptr::read_volatile((evt + 4) as *const u32),
            core::ptr::read_volatile((evt + 8) as *const u32),
            core::ptr::read_volatile((evt + 12) as *const u32),
        )
    };
    serial_println!(
        ":: tegra: JB9e — NOOP completion in LOW event ring: {} — evt[0]={:#010x} {:#010x} {:#010x} {:#010x} (type={}) USBSTS={:#010x} CRCR={:#x} ::",
        if landed { "LANDED ⭐ (high-address event ring was the killer)" } else { "silent (high-address theory refuted)" },
        d0, d1, d2, d3, (d3 >> 10) & 0x3f,
        r32(op + 0x04),
        unsafe { core::ptr::read_volatile((op + 0x18) as *const u64) } & !0x3f
    );
    // Halt again so jb2b_attach's init starts from the same state as every prior boot.
    w32(op + 0x0, r32(op + 0x0) & !1);
    let mut spins = 0u32;
    while r32(op + 0x04) & 1 == 0 && spins < 100_000 {
        spins += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// JETSON-XCARVE M2: the inherited-xHCI-pointer instrumentation.
//
// The carveout wall fires a controller-side FillWrite to the FIXED bad address 0x800000027767dc80
// (bit 63 set + ~9.86 GiB offset, beyond the 8 GiB DRAM top) right after JB9i's DISABLE_SLOT drain.
// The no-HCRST takeover (JB9g) preserves the firmware's live xHCI state, so the controller keeps
// DMA-ing through pointers the firmware programmed — until `jb2b_attach` reprograms DCBAAP / CRCR /
// ERST with OUR values (init_pointers / init_interrupter, inside the unsafe block). This probe runs
// BEFORE that overwrite: it snapshots every inherited pointer the takeover inherits, so a bench diff
// can see which one carries a bit-63 / >DRAM value that a DISABLE_SLOT-driven write could ride to
// 0x800000027767dc80. Pure CPU reads of identity-mapped, dead-but-intact post-EBS RAM (the same
// class JB9f already dereferences) — the fault is a controller DMA WRITE, never a CPU load, so
// reading is safe. Every dereference is plausibility-guarded ([0x8000_0000, 0x2_8000_0000), the
// DRAM aperture — see jbxc_plausible); a raw value that FAILS the guard is still PRINTED (that is
// exactly the bit-63 pointer we are hunting) but never dereferenced. Stable `JBXC:` prefix, raw
// 64-bit hex, so boots diff cleanly. Census v2 also walks each Configured slot's ENDPOINT CONTEXTS
// (EP state / type / TR dequeue pointer) — the class the diagnosis sitting left ambiguous.
// JETSON-XCARVE FIX (census v2 + scrub): the plausibility window is the Orin DRAM aperture proper.
// Lower bound WIDENED to the DRAM base 0x8000_0000 (lens note 2a): a sub-DRAM inherited value must
// never be CPU-dereferenced into the MMIO aperture below DRAM — print it raw, never chase or scrub-
// through it. Upper bound is the real DRAM top 0x2_8000_0000 (8 GiB window), tighter than the old
// 40-bit ceiling and identical to the scrub's poison test so census and scrub agree exactly.
#[cfg(any(feature = "xcarve", feature = "xcarve_scrub"))]
const JBXC_DRAM_BASE: u64 = 0x8000_0000;
#[cfg(any(feature = "xcarve", feature = "xcarve_scrub"))]
const JBXC_DRAM_TOP: u64 = 0x2_8000_0000;

#[cfg(any(feature = "xcarve", feature = "xcarve_scrub"))]
fn jbxc_plausible(a: u64) -> bool {
    // A firmware structure lives in former-EBS RAM inside [0x8000_0000, 0x2_8000_0000); anything
    // with bit 63 set, above the DRAM top, or below the DRAM base is NOT a walkable pointer —
    // print it raw, never chase it, and (in the scrub) neutralize it.
    a >= JBXC_DRAM_BASE && a < JBXC_DRAM_TOP
}

// The controller's actual device-context / input-context stride: 64 bytes when CSZ (HCCPARAMS1
// bit 2) is set, else 32. A wrong stride mis-addresses EVERY endpoint context past the slot
// context, so both the census walk and the scrub read it from the live controller.
#[cfg(any(feature = "xcarve", feature = "xcarve_scrub"))]
fn jbxc_ctx_size() -> u64 {
    let hccp1 = unsafe { core::ptr::read_volatile((XUSB_HOST + 0x10) as *const u32) };
    if (hccp1 >> 2) & 1 == 1 { 64 } else { 32 }
}

#[cfg(feature = "xcarve")]
pub fn jbxc_inherited_dump(cap0: u32) {
    let r32 = |a: u64| unsafe { core::ptr::read_volatile(a as *const u32) };
    let r64 = |a: u64| unsafe { core::ptr::read_volatile(a as *const u64) };

    let caplen = (cap0 & 0xff) as u64;
    let hcs1 = r32(XUSB_HOST + 0x04);
    let hcs2 = r32(XUSB_HOST + 0x08);
    let maxslots = (hcs1 & 0xff) as u64;
    let ctx_size = jbxc_ctx_size(); // 32 or 64 — the real endpoint-context stride (CSZ)
    // Max Scratchpad Buffers: HCSPARAMS2 hi = bits[25:21], lo = bits[31:27] (xHCI 5.3.4).
    let scratch_ct = (((hcs2 >> 21) & 0x1f) << 5) | ((hcs2 >> 27) & 0x1f);
    let op = XUSB_HOST + caplen;
    let rtsoff = r32(XUSB_HOST + 0x18) & !0x1f;
    let ir0 = XUSB_HOST + rtsoff as u64 + 0x20;

    let usbcmd = r32(op);
    let usbsts = r32(op + 0x04);
    let config = r32(op + 0x38);
    let crcr = r64(op + 0x18);
    let dcbaap = r64(op + 0x30);
    let erstsz = r32(ir0 + 0x08);
    let erstba = r64(ir0 + 0x10);
    let erdp = r64(ir0 + 0x18);

    serial_println!(
        "JBXC: inherited op regs — USBCMD={:#010x} USBSTS={:#010x} (HCH={} RS={}) CONFIG={:#010x} (MaxSlotsEn={}) maxslots={} scratch_ct={}",
        usbcmd, usbsts, usbsts & 1, usbcmd & 1, config, config & 0xff, maxslots, scratch_ct
    );
    serial_println!(
        "JBXC: inherited ptrs — DCBAAP={:#018x} CRCR={:#018x} ERSTBA={:#018x} ERDP={:#018x} ERSTSZ={}",
        dcbaap, crcr, erstba, erdp, erstsz
    );

    // ERST walk: each 16-byte entry = { ring_base:u64, ring_size:u32, rsvd:u32 }. Print raw; deref
    // the event ring base only when plausible.
    let erstba_a = erstba & !0x3f;
    if jbxc_plausible(erstba_a) && erstsz != 0 && erstsz <= 16 {
        for i in 0..erstsz as u64 {
            let e = erstba_a + i * 16;
            let rb = r64(e);
            let rs = r32(e + 8);
            serial_println!("JBXC: ERST[{}] ring_base={:#018x} ring_size={}", i, rb, rs);
        }
    } else {
        serial_println!("JBXC: ERST base {:#018x} (sz={}) — not walkable (guard)", erstba_a, erstsz);
    }

    // DCBAA walk: entry 0 = Scratchpad Buffer Array pointer; entries 1..=maxslots = per-slot Device
    // Context base pointers. Print EVERY entry raw (raw is the point — a bit-63 slot pointer is the
    // suspect); deref slot state only when the entry is plausible.
    let dcbaa = dcbaap & !0x3f;
    if jbxc_plausible(dcbaa) {
        let sp_array = r64(dcbaa);
        serial_println!("JBXC: DCBAA base={:#018x}  [0]=scratchpad_array={:#018x}", dcbaa, sp_array);
        // Scratchpad buffer array: `scratch_ct` 64-bit page pointers.
        if jbxc_plausible(sp_array) && scratch_ct != 0 && scratch_ct <= 64 {
            for i in 0..scratch_ct as u64 {
                let p = r64(sp_array + i * 8);
                serial_println!("JBXC: scratchpad[{}] = {:#018x}", i, p);
            }
        } else if scratch_ct != 0 {
            serial_println!("JBXC: scratchpad array {:#018x} (ct={}) — not walkable (guard)", sp_array, scratch_ct);
        }
        // Per-slot device-context pointers. Clamp the print to 64 slots (Tegra234 xHCI advertises
        // far fewer; the clamp only bounds serial volume — note it if it ever bites).
        let n = maxslots.min(64);
        for slot in 1..=n {
            let dcp = r64(dcbaa + slot * 8);
            if dcp == 0 {
                continue; // an unused slot — skip the noise, only nonzero inherited slots matter
            }
            if jbxc_plausible(dcp & !0x3f) {
                // Slot Context is the FIRST context in the device context: dword0 + dword3, with
                // Slot State = dword3[31:27] and Context Entries = dword0[31:27] = the last valid
                // endpoint DCI (xHCI 6.2.2). Read both raw.
                let sc = dcp & !0x3f;
                let d0 = r32(sc);
                let d3 = r32(sc + 0x0c);
                let ctx_entries = (d0 >> 27) & 0x1f; // last valid endpoint index (DCI)
                serial_println!(
                    "JBXC: DCBAA[{}] devctx={:#018x}  slotctx d0={:#010x} d3={:#010x} (slot_state={} ctx_entries={})",
                    slot, dcp, d0, d3, (d3 >> 27) & 0x1f, ctx_entries
                );
                // Census v2: walk this slot's ENDPOINT CONTEXTS — the unwalked class the sitting
                // left ambiguous. Endpoint context for DCI k sits at base + k*ctx_size (the slot
                // context is DCI 0). EP State = EP-ctx dword0[2:0], EP Type = dword1[5:3], and the
                // TR Dequeue Pointer is the raw 64-bit at ep-ctx offset 0x08 (bit0 = DCS). A
                // poisoned TR dequeue pointer would surface HERE, not in the slot context.
                for dci in 1..=ctx_entries as u64 {
                    let ep = sc + dci * ctx_size;
                    let e0 = r32(ep);
                    let e1 = r32(ep + 0x04);
                    let trdp = r64(ep + 0x08);
                    serial_println!(
                        "JBXC:   ep[dci={}] state={} type={} TRDeq={:#018x}{}",
                        dci, e0 & 0x7, (e1 >> 3) & 0x7, trdp,
                        if jbxc_plausible(trdp & !0xf) { "" } else { " <-- IMPLAUSIBLE (bit-63/out-of-DRAM)" }
                    );
                }
            } else {
                serial_println!("JBXC: DCBAA[{}] devctx={:#018x} — not walkable (guard)", slot, dcp);
            }
        }
        if maxslots > 64 {
            serial_println!("JBXC: (DCBAA walk clamped at 64 of {} slots)", maxslots);
        }
    } else {
        serial_println!("JBXC: DCBAA base {:#018x} — not walkable (guard)", dcbaa);
    }
    serial_println!("JBXC: inherited-pointer dump complete (pre-takeover, pre-JB9i)");
}

// ---------------------------------------------------------------------------------------------
// JETSON-XCARVE FIX M2: the least-mutating neutralization (`UNAOS_XCARVE_SCRUB=1`).
//
// The census (v2) records exactly what the firmware left in DRAM. The scrub walks the SAME
// inherited structures BEFORE the no-HCRST takeover reprograms the controller (so it reads the
// firmware's live DCBAAP/ERST from the op/runtime registers, not our replacements) and, for each
// pointer that FAILS the plausibility test — bit-63 set, or outside [0x8000_0000, 0x2_8000_0000) —
// writes the provably-poisoned value out of harm's way so the JB9i DISABLE_SLOT drain has no
// carveout target to ride. It is aimable WITHOUT naming the culprit class in advance: it fires
// ONLY on provably-invalid values (rule a: a sane pointer is NEVER rewritten), every write prints
// a `JBXC-SCRUB:` line with the old raw value (rule b), and if nothing is poisoned it is a no-op
// and says so (rule c). It touches ONLY DRAM structures — DCBAA entries, the scratchpad array, and
// endpoint-context TR dequeue pointers — never the Falcon: no HCRST, no CSB, no controller-control
// register writes (rule d). DISCLOSURE: if the poison is controller-INTERNAL (nothing visible in
// these DRAM structures), the scrub no-ops and the wall persists — that outcome DISCRIMINATES the
// last ambiguity bucket (a valid bench result, not a failed arc).
#[cfg(feature = "xcarve_scrub")]
pub fn jbxc_scrub_inherited(cap0: u32) {
    let r32 = |a: u64| unsafe { core::ptr::read_volatile(a as *const u32) };
    let r64 = |a: u64| unsafe { core::ptr::read_volatile(a as *const u64) };
    let w64 = |a: u64, v: u64| unsafe { core::ptr::write_volatile(a as *mut u64, v) };

    let caplen = (cap0 & 0xff) as u64;
    let hcs1 = r32(XUSB_HOST + 0x04);
    let hcs2 = r32(XUSB_HOST + 0x08);
    let maxslots = (hcs1 & 0xff) as u64;
    let scratch_ct = (((hcs2 >> 21) & 0x1f) << 5) | ((hcs2 >> 27) & 0x1f);
    let ctx_size = jbxc_ctx_size();
    let op = XUSB_HOST + caplen;
    let dcbaap = r64(op + 0x30);

    // A pointer with any low tag/alignment bits masked off is poisoned if it fails the DRAM-window
    // plausibility test. NULL is not "poisoned" (an unused/empty slot) — leave zeros alone.
    let poisoned = |raw: u64, mask: u64| -> bool {
        let p = raw & !mask;
        raw != 0 && !jbxc_plausible(p)
    };

    let mut scrubbed = 0u32;

    // The firmware DCBAA (read from the INHERITED DCBAAP register, pre-takeover). If the base
    // itself is poisoned we cannot safely walk it — record and stop (never dereference a bad base).
    let dcbaa = dcbaap & !0x3f;
    if !jbxc_plausible(dcbaa) {
        serial_println!(
            "JBXC-SCRUB: inherited DCBAA base {:#018x} not in DRAM window — cannot walk; no DRAM structure to scrub",
            dcbaa
        );
        serial_println!("JBXC-SCRUB: complete — 0 value(s) neutralized (base unwalkable)");
        return;
    }

    // DCBAA[0] = scratchpad buffer array pointer. If poisoned, zero the DCBAA[0] entry (the
    // scratchpad array is unused after takeover; a poisoned array pointer must not be reachable).
    let sp_array = r64(dcbaa);
    if poisoned(sp_array, 0x3f) {
        serial_println!("JBXC-SCRUB: DCBAA[0] scratchpad_array poisoned old={:#018x} -> 0 @{:#018x}", sp_array, dcbaa);
        w64(dcbaa, 0);
        scrubbed += 1;
    } else if jbxc_plausible(sp_array & !0x3f) && scratch_ct != 0 && scratch_ct <= 64 {
        // Walk the scratchpad page array; zero any poisoned page pointer in place.
        let base = sp_array & !0x3f;
        for i in 0..scratch_ct as u64 {
            let e = base + i * 8;
            let p = r64(e);
            if poisoned(p, 0xfff) {
                serial_println!("JBXC-SCRUB: scratchpad[{}] poisoned old={:#018x} -> 0 @{:#018x}", i, p, e);
                w64(e, 0);
                scrubbed += 1;
            }
        }
    }

    // Per-slot device-context pointers and their endpoint-context TR dequeue pointers.
    let n = maxslots.min(64);
    for slot in 1..=n {
        let e = dcbaa + slot * 8;
        let dcp = r64(e);
        if dcp == 0 {
            continue; // empty slot — nothing to neutralize
        }
        if poisoned(dcp, 0x3f) {
            // A poisoned device-context pointer: zero the DCBAA slot entry so the DISABLE_SLOT
            // drain has no poisoned context pointer to write through. (We do not walk endpoint
            // contexts off a bad base.)
            serial_println!("JBXC-SCRUB: DCBAA[{}] devctx poisoned old={:#018x} -> 0 @{:#018x}", slot, dcp, e);
            w64(e, 0);
            scrubbed += 1;
            continue;
        }
        // Sane device-context base: walk its endpoint contexts and clamp any poisoned TR dequeue
        // pointer. Context Entries = slot-context dword0[31:27] = last valid DCI.
        let sc = dcp & !0x3f;
        let ctx_entries = (r32(sc) >> 27) & 0x1f;
        for dci in 1..=ctx_entries as u64 {
            let ep = sc + dci * ctx_size;
            let trdp_a = ep + 0x08;
            let trdp = r64(trdp_a);
            if poisoned(trdp, 0xf) {
                // Zero the whole TR dequeue qword (pointer + DCS): a neutralized dequeue pointer
                // has no carveout target; this endpoint is being torn down by the eviction anyway.
                serial_println!(
                    "JBXC-SCRUB: DCBAA[{}] ep[dci={}] TRDeq poisoned old={:#018x} -> 0 @{:#018x}",
                    slot, dci, trdp, trdp_a
                );
                w64(trdp_a, 0);
                scrubbed += 1;
            }
        }
    }

    if scrubbed == 0 {
        serial_println!(
            "JBXC-SCRUB: no poisoned inherited DRAM value found — no-op (poison is controller-internal or unreachable; wall will persist)"
        );
    } else {
        serial_println!("JBXC-SCRUB: complete — {} value(s) neutralized pre-JB9i", scrubbed);
    }
}

// ---------------------------------------------------------------------------------------------
// JETSON-XCARVE-CRCR M1: the command-ring quiesce + re-seat (`UNAOS_XCARVE_CRCRQ=1`).
//
// The elimination verdict (§JETSON-XCARVE fix-sitting) put the FillWrite target in the ONE class
// no DRAM scrub can reach: the controller's INTERNAL command-ring fetch state — the inherited
// Command Ring Pointer / dequeue latch that a CRCR *read* returns as zeros (xHCI 5.4.5). The
// no-HCRST takeover (JB9g) preserves the firmware's live xHCI state. xHCI 5.4.5 says the Command
// Ring Pointer field of CRCR is only LOADED by the controller when the Command Ring is stopped
// (CRR=0). REVIEW-LENS CORRECTION (pre-bench): 5.4.5 ALSO says CRR clears whenever the controller
// halts (HCH=1) — a halted controller cannot read CRR=1 — and both sittings' censuses observed raw
// CRCR=0x0 at HCH=1 (CRR bit 3 = 0). So `init_pointers`' halted-time write DOES latch, the
// dropped-write theory is refuted a priori, and the CA branch below is expected DEAD on this path
// (CRR-before=0 every boot). The lever's value is as a CONFIRMING PROBE: it closes the
// command-ring bucket by silicon observation (and the re-seat guarantees no un-re-seated-ring
// window regardless). CRR-before=1 here would itself be a silicon-erratum finding.
//
// The lever: BEFORE the first command doorbell (i.e. before JB9i), explicitly quiesce and re-seat
// the command ring in spec order (xHCI 4.6.1.2 / 5.4.5):
//   1. read CRCR.CRR. If CRR=1 (ring inherited-live), issue Command Abort (CA=1) as ONE 64-bit
//      write carrying OUR ring pointer + RCS (so if the ring stops mid-write the loaded pointer is
//      valid, never null — the pre-2021 Linux abort bug ff0e50d3564f), then poll CRR->0 bounded.
//   2. WHETHER CRR was 1 or already 0, program CRCR = our_ring|RCS now that CRR=0, so the pointer
//      field is actually loaded. This leaves NO window where a command is issued on an un-re-seated
//      ring — if init_pointers' write already took (CRR was 0) this is an idempotent re-write.
//
// FALCON SAFETY (absolute): CA is a command-ring control op only — it aborts the command ring and
// nothing else. NO HCRST, NO CSB, no write that stops the Falcon service loop (the class JB9f/JB9e
// proved fatal). The controller's own recovery path (`abort_enum_command`) already issues this exact
// CA handshake on this silicon during enumeration, so CA is proven non-fatal here. Every step prints
// under a stable `JBXC-CRCRQ:` prefix (CRR-before, CA issued/skipped, CRR-after, CRCR programmed).
#[cfg(feature = "xcarve_crcrq")]
pub fn jbxc_crcrq_quiesce(cap0: u32, our_cmd_ring: u64) {
    let caplen = (cap0 & 0xff) as u64;
    let op = XUSB_HOST + caplen;
    let crcr = op + 0x18;
    let r64 = |a: u64| unsafe { core::ptr::read_volatile(a as *const u64) };
    let w64 = |a: u64, v: u64| unsafe { core::ptr::write_volatile(a as *mut u64, v) };

    // Our command ring, 64-byte aligned, RCS=1 (a fresh TransferRing starts at cycle 1 — the same
    // value init_pointers programmed as `ring_phys | 1`).
    let reseat = (our_cmd_ring & !0x3f) | 1;

    let crr_before = (r64(crcr) >> 3) & 1;
    serial_println!(
        "JBXC-CRCRQ: pre-JB9i quiesce — CRCR={:#018x} CRR-before={} our_ring={:#018x}",
        r64(crcr), crr_before, reseat
    );

    if crr_before == 1 {
        // Ring inherited-live: abort it. CA=1 (bit 2) + our valid pointer + RCS in one write.
        w64(crcr, reseat | (1 << 2));
        serial_println!("JBXC-CRCRQ: CA issued (Command Abort, CRCR.CA=1) — polling CRR->0");
        let deadline = cntpct().wrapping_add(cntfrq() / 5); // ~200 ms bound
        let mut stopped = false;
        loop {
            if (r64(crcr) >> 3) & 1 == 0 {
                stopped = true;
                break;
            }
            if cntpct().wrapping_sub(deadline) < (1u64 << 63) {
                break; // deadline passed (wrap-safe)
            }
            core::hint::spin_loop();
        }
        if stopped {
            serial_println!("JBXC-CRCRQ: CRR-after={} (ring stopped by abort)", (r64(crcr) >> 3) & 1);
        } else {
            // CRR stuck high: the pointer-field write below will NOT be loaded (5.4.5). Report it
            // honestly; the JB9i drain still runs (this is a knob-gated diagnostic lever, not a
            // gate) and the serial names the stuck-abort exactly.
            serial_println!("JBXC-CRCRQ: !!! CRR STILL 1 after abort timeout — re-seat cannot load (5.4.5); reporting !!!");
        }
    } else {
        serial_println!("JBXC-CRCRQ: CRR-before=0 — no abort needed; re-seating CRCR anyway (no window)");
    }

    // Re-seat: with CRR=0 the Command Ring Pointer field is loaded. Idempotent if init_pointers'
    // write already took. (If CRR is still 1 from a timed-out abort, this write's pointer is ignored
    // per 5.4.5 — only the status bits move — and the JBXC-CRCRQ line above already flagged that.)
    w64(crcr, reseat);
    serial_println!(
        "JBXC-CRCRQ: CRCR re-seated = {:#018x} (CRR-final={}) — command ring points at OUR ring pre-JB9i",
        r64(crcr), (r64(crcr) >> 3) & 1
    );
}

// ---------------------------------------------------------------------------------------------
// XUSBFW: the Tegra234 XUSB Falcon firmware-RELOAD scaffold (feature `xusbfw`, UNAOS_XUSBFW=1).
//
// This is a DESIGN SCAFFOLD, not a working reload. The signed NVGI firmware container is bench-only
// and never enters the tree, so the IMEM/DMEM byte layout is unknown (see the design note) and this
// path STOPs, witnessed, at the load boundary rather than streaming bytes it cannot place. Every
// law's guard is wired even though the destructive steps are stubbed:
//   * BAR2 CSB ONLY — the FPCI/CFG CSB aperture (FPCI+0x41c/+0x800) is EL3-FATAL (JB7); never here.
//   * JB9d-GUARD reconciliation — a Falcon whose CSB reads all-ones cannot be halted/loaded via the
//     CSB (JB6: writes don't stick). Reload's STEP 0 (a host-side BPMP MRQ_RESET, out-of-CSB) must
//     clear that FIRST; this scaffold reuses jb6_csb_sweep's page-select-STICKS test as the gate and
//     emits a witnessed SKIP — the guard's own honest-SKIP philosophy — if the aperture won't answer.
//   * NEVER write CRCR at RS=1 — the scaffold touches no xHCI operational register at all.
//   * Announce every new MMIO class before the first touch (JX1 discipline).
//   * Mutually exclusive with the JB9G_NO_HCRST inherit recipe (STEP 0 wipes inherited state — the
//     whole point). Enforced at RUNTIME here so the scaffold compiles; ARMING the real reset requires
//     BOTH `JB9G_NO_HCRST = false` AND upgrading this to a compile-time assert (the JB4/JB5 pattern).
//
// See ~/.claude/plans/unaos/review/xusb-fw-reload-DESIGN.md.
// ---------------------------------------------------------------------------------------------
#[cfg(feature = "xusbfw")]
pub mod xusbfw {
    use super::{cntpct, jb9_bar2_routed, JB9G_NO_HCRST, XUSB_BAR2};

    // Build-time firmware injection: `pub const XUSB_FW: Option<&[u8]>`, from the OPTIONAL
    // `unaos-xusb-fw` crate — `Some(include_bytes!("<abs path>"))` ONLY when UNAOS_XUSB_FW_PATH is
    // set (read directly from the bunker, never copied into the tree), else `None`. It lives in its
    // own crate rather than a kernel build.rs because a build script's mere PRESENCE changes cargo's
    // -C metadata, which re-lays-out every artifact on every arch even with the knob off (measured:
    // 12b0993c moved knob-off x86 and Pi media, and deleting build.rs restored them byte-for-byte).
    // As an optional dep it is absent from the knob-off build graph entirely, so the default build
    // is both blob-free AND byte-identical to its pre-XUSBFW baseline.
    pub use unaos_xusb_fw::XUSB_FW;

    /// The signed NVIDIA container magic ('NVGI') — the one byte-level fact we assert on the blob.
    /// The container is nested (NVGI -> RFFS -> RFRD -> ... -> tegra_xusb_fw_header + IMEM/DMEM) and
    /// its internal segment layout is NOT reverse-engineered (design note §4) — read-only only.
    const NVGI_MAGIC: [u8; 4] = *b"NVGI";

    /// One witnessed outcome per reload attempt. The scaffold can only ever reach `LoadUnresolved`
    /// (the honest STOP) or one of the earlier skips — never a real start — until the container
    /// layout is resolved and STEP 0 (the BPMP Falcon reset) is implemented.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum ReloadOutcome {
        /// Feature on but UNAOS_XUSB_FW_PATH unset — build.rs emitted `None`. Witnessed no-op.
        NoFirmware,
        /// The blob is present but not an NVGI container — refuse to touch the Falcon.
        BadContainer,
        /// The inherit recipe (JB9G_NO_HCRST) is armed; reload would fight it. Runtime mutual-excl.
        InheritActive,
        /// The BAR2 CSB aperture won't answer (all-ones / page-select won't stick). Guard-aligned
        /// SKIP — STEP 0 (out-of-CSB Falcon reset) is required first and is not yet implemented.
        CsbSilent,
        /// Reached the IMEM/DMEM load boundary: container layout unresolved, so we STOP here rather
        /// than stream bytes we cannot place. This is the scaffold's terminal state by design.
        LoadUnresolved,
    }

    /// Does the BAR2 CSB page-select register (ARU_C11_CSBRANGE @0x9c) accept + read back a write?
    /// This is the exact STICKS test jb6_csb_sweep runs — the discriminator between "the aperture
    /// answers" and "the block reads all-ones / writes don't land". Strictly the same non-destructive
    /// page-select write jb3_falcon already performs (no resets, no power/clock changes).
    fn csb_page_select_sticks() -> bool {
        let r = |off: u64| unsafe { core::ptr::read_volatile((XUSB_BAR2 + off) as *const u32) };
        let w = |off: u64, v: u32| unsafe {
            core::ptr::write_volatile((XUSB_BAR2 + off) as *mut u32, v)
        };
        let pre = r(0x9c);
        w(0x9c, 0x1234);
        let rb = r(0x9c);
        w(0x9c, pre); // restore
        rb == 0x1234
    }

    /// The reload entry point, wired at the top of `jb2b_attach` (before the takeover decision).
    /// SCAFFOLD: performs every guard it can without threading new args, then STOPs witnessed at the
    /// load boundary. Returns the outcome so the caller can (in a future armed build) branch to the
    /// clean reset-path init instead of the no-HCRST inherit takeover.
    pub fn reload_entry(cap0: u32) -> ReloadOutcome {
        // 1. Witnessed no-op when no firmware path was supplied at build time (build.rs -> None).
        let Some(fw) = XUSB_FW else {
            serial_println!("XUSBFW: no firmware path set — reload skipped");
            return ReloadOutcome::NoFirmware;
        };

        // 2. Cheap container sanity: the signed NVGI magic. We do NOT parse the container further
        //    (design note §4: the NVGI/RFFS/RFRD layout is deliberately NOT reverse-engineered).
        if fw.len() < 4 || fw[0..4] != NVGI_MAGIC {
            serial_println!(
                "XUSBFW: firmware present ({} B) but not an NVGI container (magic={:#04x}{:02x}{:02x}{:02x}) — refusing to touch the Falcon",
                fw.len(),
                fw.first().copied().unwrap_or(0),
                fw.get(1).copied().unwrap_or(0),
                fw.get(2).copied().unwrap_or(0),
                fw.get(3).copied().unwrap_or(0),
            );
            return ReloadOutcome::BadContainer;
        }
        serial_println!(
            "XUSBFW: NVGI firmware container present ({} B) — reload scaffold engaged (cap0={:#010x})",
            fw.len(),
            cap0
        );

        // 3. Mutual exclusion with the inherit recipe. STEP 0 of a real reload RESETS the Falcon and
        //    wipes the inherited controller state that JB9G_NO_HCRST exists to preserve — the two can
        //    never run on one boot. Enforced at RUNTIME so this scaffold compiles; ARMING the real
        //    reset requires setting JB9G_NO_HCRST=false AND upgrading this to a compile-time
        //    `const _: () = assert!(!(cfg!(feature="xusbfw") && JB9G_NO_HCRST));` (the JB4/JB5 class).
        if JB9G_NO_HCRST {
            serial_println!(
                "XUSBFW: JB9G_NO_HCRST is armed — a Falcon reset would wipe the inherited state the \
                 no-HCRST recipe preserves; reload is mutually exclusive with inherit and is SKIPPED \
                 (set JB9G_NO_HCRST=false to arm reload — design note §5)"
            );
            return ReloadOutcome::InheritActive;
        }

        // 4. JB9d-GUARD reconciliation. Before ANY CSB write, prove the aperture answers. On the
        //    inherited-halted block CPUCTL reads all-ones and JB6 proved even the 0x9c page-select
        //    won't stick — a firmware stream into that aperture is a poke into a dead block, the exact
        //    class JB9d-GUARD forbids. STEP 0 (a host-side BPMP MRQ_RESET of the Falcon/host reset id,
        //    NEVER padctl) must clear the all-ones state from OUTSIDE the CSB first — that reset, and
        //    the MRQ_PG GET_STATE partition-on proof it must be preceded by (bpmp_tegra::jb5_pg_on
        //    style), are NOT implemented in this scaffold (they need the bpmp channel threaded in).
        if !jb9_bar2_routed() {
            serial_println!("XUSBFW: BAR2 unrouted — cannot reach the CSB; reload SKIPPED (guard)");
            return ReloadOutcome::CsbSilent;
        }
        if !csb_page_select_sticks() {
            serial_println!(
                "XUSBFW: CSB page-select won't stick (Falcon all-ones / not answering) — reload SKIPPED. \
                 A host-side BPMP Falcon reset (STEP 0, out-of-CSB) is required first and is not yet \
                 implemented (JB9d-GUARD-aligned; design note §3)"
            );
            return ReloadOutcome::CsbSilent;
        }

        // 5. The load boundary. With the CSB answering we COULD now halt the Falcon (CPUCTL.HALT),
        //    stream IMEM/DMEM, set BOOTVEC and STARTCPU (design note §1). But the NVGI container's
        //    IMEM/DMEM segment offsets/sizes and boot vector are unknown (NOT reverse-engineered),
        //    so we STOP here rather than stream bytes we cannot place. This is the scaffold's
        //    terminal state — the declared reason the path is not end-to-end in-tree.
        let _ = cntpct; // (bounded-wait helper the armed start()-poll will use)
        serial_println!(
            "XUSBFW: CSB answers — at the IMEM/DMEM load boundary. Container layout unresolved \
             (NVGI/RFFS/RFRD not reverse-engineered); STOP before streaming firmware (design note §4)"
        );
        ReloadOutcome::LoadUnresolved
    }
}

/// `jb9_smmu` = (NISO1 SMMU instance bases, XUSB SID) — threaded in by the caller so the JB9-B
/// forensic captures below can dump the stream binding at enable-slot-pending time.
pub fn jb2b_attach(
    dma_coherent: Option<bool>,
    jb9_bases: &[u64],
    jb9_sid: u32,
    jb9_ao: Option<u64>,
) -> Option<(u8, u8)> {
    serial_println!(
        ":: tegra: JB2b — usb@3610000 dma-coherent: {} ::",
        match dma_coherent {
            Some(true) => "YES (Normal-WB rings, no cache maintenance)",
            Some(false) => "ABSENT from DTB (proceeding; stale-ring stall would implicate this)",
            None => "unresolved (DTB/node not found; proceeding on the Linux-dtsi expectation)",
        }
    );

    // Pre-flight: the same guarded read JB1c used. If the ungate regressed since (or a partial
    // boot re-entered), a dead capability word means STOP here — `xhci::init` would otherwise
    // chase a garbage CAPLENGTH through minutes of bounded timeouts.
    let cap0 = unsafe { core::ptr::read_volatile(XUSB_HOST as *const u32) };
    if cap0 == 0xFFFF_FFFF || cap0 == 0 {
        serial_println!(":: tegra: JB2b — XUSB cap0={:#010x} (not alive); STOP ::", cap0);
        return None;
    }

    serial_println!(
        ":: tegra: JB2b — attaching the shared xHCI driver @{:#x} (platform, polled, no PCIe) ::",
        XUSB_HOST
    );

    // XUSBFW: the Falcon firmware-RELOAD scaffold, run BEFORE the takeover decision (a reload
    // replaces the inherit path — design note §2). SCAFFOLD: witnesses its guards and STOPs at the
    // load boundary; it performs nothing destructive, so it is safe to wire ahead of the inherit
    // takeover. Knob-off (`xusbfw` feature absent) this call + the whole module vanish
    // (byte-identical, blob-free — build.rs emits no firmware bytes without the feature+path).
    #[cfg(feature = "xusbfw")]
    let _ = xusbfw::reload_entry(cap0);

    // JETSON-XCARVE M2: snapshot every inherited xHCI pointer BEFORE the no-HCRST takeover below
    // reprograms DCBAAP/CRCR/ERST — the diagnosis probe for the carveout wall (fault ADDR
    // 0x800000027767dc80 at JB9i). Read-only; prints pre-eviction so even a faulting boot yields the
    // pointers. Knob-off (`xcarve` feature absent) this whole call vanishes (byte-identical).
    #[cfg(feature = "xcarve")]
    jbxc_inherited_dump(cap0);

    // JETSON-XCARVE FIX M2: the aimed neutralization. Runs pre-takeover (so it reads the firmware's
    // live inherited DCBAAP/structures) and pre-JB9i (so the DISABLE_SLOT drain finds no poisoned
    // pointer). Only rewrites values that FAIL the plausibility test — never a sane pointer — and
    // touches DRAM structures only, never the Falcon. Knob-off (`xcarve_scrub` absent) the whole
    // call vanishes (byte-identical; zero `JBXC-SCRUB` strings).
    #[cfg(feature = "xcarve_scrub")]
    jbxc_scrub_inherited(cap0);

    // The exact sequence the PCIe paths run (arch/aarch64/pci.rs), minus discovery/bus-master —
    // a platform controller has no config space; DMA mastering came with the BPMP ungate.
    // JB9g (bench 2026-07-08): NO-HCRST takeover — the arc's payoff lever. JB9f proved the
    // firmware is alive and DMA-capable on the inherited state (bare RS=1 posts PSC events into
    // UEFI's own >4GiB event ring within 200 ms), while every UnaOS attach since JB2b ran
    // halt+HCRST first and got a permanently-non-servicing engine (JB9e: not even a NOOP into an
    // all-low ring). HCRST on this inherited post-EBS state is what kills the Falcon's service
    // loop — so take over the xHCI-legal way that skips it: the controller is already cleanly
    // halted (XhcHaltHC RS=0), reprogram DCBAAP/CRCR/ERST/CONFIG below while halted, then RS=1.
    if JB9G_NO_HCRST {
        let cl = (cap0 & 0xff) as u64;
        let op = XUSB_HOST + cl;
        let r = |a: u64| unsafe { core::ptr::read_volatile(a as *const u32) };
        let w = |a: u64, v: u32| unsafe { core::ptr::write_volatile(a as *mut u32, v) };
        w(op, r(op) & !1); // RS=0 (idempotent if already halted)
        let mut spins = 0u32;
        while r(op + 0x04) & 1 == 0 && spins < 100_000 {
            spins += 1;
        }
        serial_println!(
            ":: tegra: JB9g — no-HCRST takeover: halted (USBSTS={:#010x}), inherited FW state preserved ::",
            r(op + 0x04)
        );
    } else {
        xhci::init(XUSB_HOST); // halt + HCRST + CNR wait — KILLS the inherited Falcon (JB9f/JB9e)
    }
    // JB9-A probe point 2: the FW's liveness right after our takeover (no-HCRST or reset path).
    jb9_fw_alive("post-xhc-restart");
    // JB9-B needs the ring physical bases after the controller lock is released — captured here.
    let mut jb9_evt_phys = 0u64;
    let mut jb9_cmd_phys = 0u64;
    unsafe {
        let mut x = xhci::XhciController::new(XUSB_HOST as usize);
        let (event_ring_phys, command_ring_phys) = {
            let mut cmd_ring_guard = xhci::COMMAND_RING.lock();
            let mut evt_ring_guard = xhci::EVENT_RING.lock();
            *cmd_ring_guard = Some(xhci::ring::TransferRing::new(256));
            *evt_ring_guard = Some(xhci::event::EventRing::new());
            (
                evt_ring_guard.as_mut().unwrap().get_ptr(),
                cmd_ring_guard.as_mut().unwrap().get_ptr(),
            )
        };
        jb9_evt_phys = event_ring_phys;
        jb9_cmd_phys = command_ring_phys;
        // ERST is heap-allocated inside init_interrupter (JETSON-XCARVE: no xHC DMA structure lives in
        // kernel-image .bss; the event ring + ERST now share the HEAP-GUARD-vetted firewall-clean window).
        // One line before the first RUNTIME-register / doorbell-array touch (new offsets within
        // the ungated block — the JX1 discipline: a dead boot's last line names the killer).
        serial_println!(":: tegra: JB2b — programming interrupter + rings (runtime regs) ::");
        // ORIN-X200-1: stamp the xHCI DMA-arming window so the next metal boot can temporally
        // separate the two 0x200-RAS candidates (xHCI vs RTL8168 — see rtl8168_tegra's twin stamp).
        serial_println!(":: X200: xhci pointer-programming begins t={} (cntpct) ::", cntpct());
        x.init_interrupter(event_ring_phys);
        x.init_pointers(command_ring_phys);
        x.start();
        serial_println!(":: X200: xhci RS=1 (controller may DMA from here) t={} (cntpct) ::", cntpct());
        xhci::install(x);
    }

    // JETSON-XCARVE-CRCR M1: quiesce + re-seat the command ring BEFORE the first command doorbell.
    // start() just set RS=1 but rang no doorbell, so JB9i's DISABLE_SLOT below is the FIRST command
    // the controller fetches. Per the review-lens correction at jbxc_crcrq_quiesce's header:
    // CRR-before is EXPECTED 0 (spec 5.4.5 + both censuses), so this is a CONFIRMING PROBE that
    // closes the command-ring bucket by observation — the CA branch is expected dead, and the
    // re-seat guarantees no un-re-seated-ring window either way. Falcon untouched (CA only, no
    // HCRST/CSB). Knob-off (`xcarve_crcrq` absent) this call vanishes (byte-identical to baseline).
    #[cfg(feature = "xcarve_crcrq")]
    jbxc_crcrq_quiesce(cap0, jb9_cmd_phys);

    // JB9i (bench 2026-07-08): evict UEFI's stale device slots. On the inherit path the firmware
    // still holds the slots UEFI enumerated (it was never reset), so our fresh ENABLE_SLOT
    // succeeds (slot 5) but ADDRESS_DEVICE on a device the FW considers already-owned fails with
    // completion code 17 (Parameter Error). DISABLE_SLOT 1..8 is the spec-clean reclaim — a
    // not-enabled slot just completes with code 11 (Slot Not Enabled), harmless.
    if JB9G_NO_HCRST {
        // WEDGE-8 (F3): a claimed loan, not a held lock (single-threaded boot stage; a failed
        // claim just skips the eviction, same as the old `None` case).
        if let Ok(mut x) = xhci::claim() {
            let x = &mut *x;
            for slot in 1u8..=8 {
                let trb = xhci::trb::Trb {
                    parameter: 0,
                    status: 0,
                    control: (10 << 10) | ((slot as u32) << 24), // TRB type 10 = Disable Slot
                };
                // ORIN-USB-FIX: record each eviction TRB's physical address so the event
                // dispatch claims its completion by address — success (reclaimed) and code 11
                // (Slot Not Enabled) are both the expected outcome of this blind 1..8 sweep,
                // and must not read as `COMMAND FAILED` or be re-queued as untracked slots
                // (both happened verbatim on the R22 sitting-2 Orin boots).
                match x.send_command(trb) {
                    Ok(phys) => x.evict_pending[(slot - 1) as usize] = phys,
                    Err(e) => serial_println!(
                        ":: tegra: JB9i — DISABLE_SLOT {} not issued ({}) ::", slot, e),
                }
            }
            // Drain the eight completions (bounded ~100 ms) before enumeration starts, so no
            // stale completion aliases against the enum FSM's own commands.
            let deadline = cntpct().wrapping_add(cntfrq() / 10);
            while cntpct().wrapping_sub(deadline) >= (1u64 << 63) {
                x.poll_events();
                core::hint::spin_loop();
            }
            // ORIN-USB-FIX: retire any claim that never saw its completion inside the drain
            // window — the command ring wraps, so a stale physical address left armed could
            // one day claim an unrelated command's completion. Honest count on the way out.
            let unanswered = x.evict_pending.iter().filter(|&&p| p != 0).count();
            if unanswered != 0 {
                serial_println!(
                    ":: tegra: JB9i — {} eviction completion(s) unanswered within the drain window (claims retired) ::",
                    unanswered);
                x.evict_pending = [0; 8];
            }
            serial_println!(":: tegra: JB9i — inherited-slot eviction: DISABLE_SLOT 1..8 issued + drained ::");
        }
    }

    // Pump the polled enumeration, bounded. 60 s wall-clock, sized to the WORST case, not the
    // happy path: `hw_wait_budget()` is a fixed 150M CNTVCT cycles = ~4.8 s at Orin's 31.25 MHz
    // (double its ~60 MHz design note), and a co-device that stalls ahead of the keyboard in the
    // serialized queue (the boot stick is always plugged) can burn a full retry ladder — up to
    // ~3 x (2.4 s watchdog + 4.8 s command-abort) ≈ 22 s — before `start_next_port` even reaches
    // the keyboard. A 20 s window lost the keyboard to exactly that; 60 s survives two stalled
    // stages plus the keyboard's own. Only a FAILING boot pays the wait (the happy path exits at
    // keyboard-ARMED in a few seconds), and the driver's stage/still-waiting lines keep the
    // serial console visibly alive throughout.
    let pump_start = cntpct();
    let deadline = pump_start.wrapping_add(cntfrq().saturating_mul(60));
    // JD3: once the keyboard is armed, keep pumping for a bounded settle window so a mass-storage
    // device BEHIND the hub (the Alcor reader — it enumerates after the root ports, then its SCSI
    // bring-up runs synchronously) can publish its block device before the EL2->EL1 drop. 8 s
    // comfortably covers the JB10 nested-hub descent + bring-up; a storage-present boot exits the
    // instant the disk is ready, so only a keyboard-only boot pays the full wait once.
    let storage_settle = cntfrq().saturating_mul(8);
    let mut armed: Option<(u8, u8)> = None;
    let mut armed_at: u64 = 0;
    // USB-HID-MULTI: witness EVERY armed HID keyboard, not just the first. `witnessed` is a
    // slot-id bitmask (slots are 1..=few) so a newly-armed second composite keyboard prints its
    // own ARMED line the pass it comes up. The FIRST armed keyboard still drives `armed` (the
    // storage-settle timer + the return value), and its line keeps the exact spec text.
    let mut witnessed: u64 = 0;
    let mut armed_ptr_count: usize = 0;
    // JB9-B capture marks: t≈200 ms (mid the FIRST port's first enable-slot attempt — the JB8
    // log shows ~340 ms per watchdog attempt, so the command is in flight) and t≈5 s (a later
    // port's attempt — same question after several rounds of recovery). Fired OUTSIDE the
    // controller lock; every capture is read-only.
    let mut jb9_marks = [
        (cntfrq() / 5, "t+200ms", false),
        (cntfrq().saturating_mul(5), "t+5s", false),
    ];
    loop {
        if JB9_PROBE {
            let elapsed = cntpct().wrapping_sub(pump_start);
            for m in jb9_marks.iter_mut() {
                if !m.2 && elapsed >= m.0 {
                    m.2 = true;
                    super::smmu_tegra::jb9_stream_dump(jb9_bases, jb9_sid, m.1);
                    jb9_fw_sid_view(m.1, jb9_ao);
                    jb9_ring_scan(jb9_evt_phys, jb9_cmd_phys, m.1);
                }
            }
        }
        let (armed_kbds, ptr_count, storage_slot, storage_ready) = {
            // WEDGE-8 (F3): claim the loan per pass — service_storage's BOT waits must never run
            // under a held lock. Busy cannot occur in this single-threaded pre-drop window, but
            // the claim is fallible by contract; skip the pass if it ever is.
            let Ok(mut x) = xhci::claim() else {
                core::hint::spin_loop();
                continue;
            };
            let x = &mut *x;
            x.poll_events();
            x.service_hubs();
            x.service_hid_setproto();
            x.service_slot_disposal();
            x.service_enum();
            // JD3: bring a mass-storage device up IN this pre-drop window, while the JM4 timer is
            // live. Once the hub descent configures the MSC bulk endpoints, `service_storage` runs
            // the synchronous SCSI bring-up (SET_CONFIGURATION / INQUIRY / READ CAPACITY) and
            // publishes `drivers::block::BLOCK_DEVICE`, so the panel shell's `ls`/`cat` have a real
            // FAT volume behind them post-drop. It MUST run here, not in the post-drop pump: its BOT
            // waits ride `crate::hlt()`, which the timerless EL1 core cannot wake (the JD2/JB2b
            // rule). A no-op until a device is pending, so keyboard-only boots are unaffected.
            x.service_storage();
            (x.armed_keyboards(), x.armed_pointer_count(), x.storage_slot, x.storage_slot != 0 && x.storage_note == "ready")
        };
        armed_ptr_count = ptr_count;
        // USB-HID-MULTI: witness each armed keyboard as it comes up (not only the first), so a boot
        // where two composite keyboards enumerate records BOTH — every one has its interrupt-IN read
        // polled and its keys merged into the shared pal queue. The first still drives the settle
        // timer + the returned handle; its line keeps the exact `-> PASS` spec text.
        for (slot, port) in armed_kbds.iter().copied() {
            if witnessed & (1u64 << slot) != 0 {
                continue;
            }
            witnessed |= 1u64 << slot;
            serial_println!(
                ":: tegra: JB2b — keyboard ARMED (slot {}, root port {}) -> PASS ::",
                slot,
                port
            );
            if armed.is_none() {
                armed = Some((slot, port));
                armed_at = cntpct();
            }
        }
        if let Some(k) = armed {
            if storage_ready {
                serial_println!(
                    ":: tegra: JB2b — HID: {} keyboard(s), {} pointer(s) armed ::",
                    (witnessed.count_ones()) as usize, armed_ptr_count
                );
                serial_println!(
                    ":: tegra: JD3 — mass storage ready (slot {}); panel shell ls/cat live ::",
                    storage_slot
                );
                return Some(k);
            }
            // Keyboard up but no disk yet: give the hubbed MSC a bounded settle window, then drop.
            if cntpct().wrapping_sub(armed_at) >= storage_settle {
                serial_println!(
                    ":: tegra: JB2b — HID: {} keyboard(s), {} pointer(s) armed ::",
                    (witnessed.count_ones()) as usize, armed_ptr_count
                );
                serial_println!(
                    ":: tegra: JD3 — no mass storage within the settle window; proceeding (shell ls/cat report no disk) ::"
                );
                return Some(k);
            }
        }
        if cntpct().wrapping_sub(deadline) < (1u64 << 63) {
            break; // deadline passed (wrap-safe compare)
        }
        core::hint::spin_loop();
    }

    // Honest verdict: no keyboard armed inside the window. Dump the live topology so the dead
    // boot's serial says WHERE enumeration got to (ports seen, slots, stall records).
    serial_println!(":: tegra: JB2b — keyboard NOT armed within the window; topology: ::");
    for line in xhci::usb_summary() {
        serial_println!(":: tegra: JB2b —   {} ::", line);
    }
    None
}

/// JB2b EL1 keyboard pump — a cooperative kernel task spawned (pre-drop) onto the boot core's run
/// queue, dispatched at EL1 by `run_capstone_boot_core`'s drive loop alongside the CAPSTONE tasks.
/// First light: HID reports keep flowing after the EL2->EL1 drop because every xHCI structure is
/// identity-mapped RAM and the MMIO GiB is in the EL1 twin table.
///
/// ONLY `poll_events` here — never the `service_*` pumps. Their bounded waits ride `crate::hlt()`,
/// and at EL1 the pre-drop `timer::LIVE=true` is stale (the drop disabled the timer): a WFI would
/// have NO wake source and park this core forever. `poll_events` is the async half — event drain,
/// HID decode, interrupt-IN re-arm via doorbell — and never waits.
///
/// Busy-poll + `yield_now`, never `sleep_ticks`: the boot-core drive loop dispatches the run queue
/// but drains no sleepers (JC3 semantics), so a slept task would never wake.
pub fn kbd_pump_body(_arg: usize) {
    serial_println!(":: tegra: JB2b — EL1 keyboard pump live (xHCI polled at EL1) ::");
    loop {
        if let Ok(mut x) = xhci::claim() {
            x.poll_events();
        }
        // Drain the pal queue the HID decoder feeds — the same sink the x86 GUI drains — and
        // print each keystroke as the arc's first-light evidence line. Non-key events (a mouse
        // wiggle) are consumed silently; a flood of motion deltas would drown the serial log.
        while let Some(ev) = crate::pal::next_event() {
            if let crate::pal::Event::Key(c) = ev {
                if (32..=126).contains(&c) {
                    serial_println!(":: tegra: JB2b — KEY '{}' ::", c as char);
                } else {
                    serial_println!(":: tegra: JB2b — KEY {:#04x} ::", c);
                }
            }
        }
        super::sched::yield_now();
    }
}
