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
//     whose more aggressive ExitBootServices teardown powers the USB2 pads DOWN. So the ports read
//     electrically dead (PORTSC=0x2a0, CCS=0) at JB2b attach. JB2c (`jb2c_padctl_powerup`, below)
//     re-programs the pad power-up sequence here BEFORE the attach — padctl is always-powered
//     (outside the XUSB PG toggle, in the GiB-0 device map) so this is NOT a JX1 EL3-fatal class
//     (the Step-0 probe read the block without fault). Still never assert TEGRA234_RESET_XUSB_PADCTL
//     (it would wipe all padctl state) and never set VBUS_OVERRIDE (device-mode fake, wrong for a
//     host port; the P3768 devkit's 5V rail is regulator-always-on with no GPIO enable).
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

/// A keyboard whose interrupt-IN read is ARMED: `keyboard_state == 3` is set exactly when the
/// device-level SET_CONFIGURATION completed and `queue_keyboard_read` pushed the first Normal TRB
/// (drivers/xhci/mod.rs, the HID SET_CONFIGURATION COMPLETE branch). Returns (slot, root port).
fn keyboard_armed(x: &xhci::XhciController) -> Option<(u8, u8)> {
    for (i, s) in x.slots.iter().enumerate() {
        if s.active && s.is_keyboard && s.keyboard_state == 3 {
            return Some((i as u8, s.port_id));
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// JB2c: re-program the Tegra234 XUSB padctl USB2 pads that the firmware teardown powered down.
//
// All offsets/bits are from Linux drivers/phy/tegra/xusb-tegra186.c (`tegra186_utmi_bias_pad_power_on`
// / `tegra186_utmi_pad_power_on`, tegra234 SoC data), verified against live mainline 2026-07-06 and
// cross-corroborated against the JB2c Step-0 silicon readback (which nailed every power-down bit).
// ---------------------------------------------------------------------------------------------

/// XUSB padctl base — always-powered (outside the XUSB power-gate), in the GiB-0 Device-nGnRE window
/// mmu_tegra maps at both EL2 and EL1. The Step-0 probe read it here without an EL3 fault.
const PADCTL: u64 = 0x0352_0000;

// Register offsets from PADCTL.
const PAD_MUX: u64 = 0x004; // USB2 lane mux (per-port; already XUSB=0x55 on this silicon)
const PORT_CAP: u64 = 0x008; // per-port capability (port*4 shift; already HOST 0x111)
const BIAS_PAD_CTL0: u64 = 0x284;
const BIAS_PAD_CTL1: u64 = 0x288;
const BIAS_PAD_CTL2: u64 = 0x28C;
/// OTG_PADx_CTL0 = 0x88 + x*0x40; CTL1 = 0x8C + x*0x40 (0x40 per-pad stride; x=0..3).
const fn otg_ctl0(pad: u64) -> u64 {
    0x088 + pad * 0x40
}
const fn otg_ctl1(pad: u64) -> u64 {
    0x08C + pad * 0x40
}

// OTG_PADx_CTL0 bits.
const OTG_PD: u32 = 1 << 26; // USB2_OTG_PD — main pad power-down
const OTG_PD_ZI: u32 = 1 << 29; // USB2_OTG_PD_ZI
const TERM_SEL: u32 = 1 << 25; // TERM_SEL
// OTG_PADx_CTL1 bits.
const OTG_PD_DR: u32 = 1 << 2; // USB2_OTG_PD_DR — driver power-down
const TERM_RANGE_ADJ: u32 = 0xf << 3; // [6:3]
const RPD_CTRL: u32 = 0x1f << 26; // [30:26]
// BIAS_PAD_CTL0 bits.
const BIAS_PAD_PD: u32 = 1 << 11;
const HS_DISCON_LEVEL: u32 = 0x7 << 3; // [5:3], set to 0x7
// BIAS_PAD_CTL1 bits.
const TRK_START_TIMER: u32 = 0x7f << 12; // [18:12], value 0x1e
const TRK_DONE_RESET_TIMER: u32 = 0x7f << 19; // [25:19], value 0x0a
const PD_TRK: u32 = 1 << 26; // USB2_PD_TRK
const TRK_COMPLETED: u32 = 1 << 31; // USB2_TRK_COMPLETED (write-1-to-clear)
// BIAS_PAD_CTL2 bits.
const CYA_TRK_CODE_UPDATE_ON_IDLE: u32 = 1 << 31;
// PORT_CAP / PAD_MUX field values (2-bit fields; HOST/XUSB = 0b01).
const PORT_CAP_HOST: u32 = 0b01;
const PAD_MUX_XUSB: u32 = 0b01;

/// The HOST-capable pads: 0/1/2 (pad 1 is the RTS5420 hub upstream per Linux; covering 0/1/2 catches
/// whichever physical Type-A connector the device lands on). Pad 3 is disabled — leave it alone.
const HOST_PADS: [u64; 3] = [0, 1, 2];

#[inline]
fn pr32(off: u64) -> u32 {
    unsafe { core::ptr::read_volatile((PADCTL + off) as *const u32) }
}
#[inline]
fn pw32(off: u64, v: u32) {
    unsafe { core::ptr::write_volatile((PADCTL + off) as *mut u32, v) }
}
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

/// Poll BIAS_PAD_CTL1.TRK_COMPLETED with a bounded `us` budget. Warn-only (Linux's readl_poll_timeout
/// on this bit is non-fatal too): returns whether it set; the caller proceeds either way.
fn poll_trk_completed(us: u64) -> bool {
    let ticks = cntfrq().saturating_mul(us) / 1_000_000;
    let start = cntpct();
    loop {
        if pr32(BIAS_PAD_CTL1) & TRK_COMPLETED != 0 {
            return true;
        }
        if cntpct().wrapping_sub(start) >= ticks {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// JB2c: re-program the USB2 UTMI pads NVIDIA's ExitBootServices teardown powered down (JB2b root
/// cause: every OTG pad PD/PD_ZI/PD_DR + the shared BIAS_PAD_PD/PD_TRK set, so no root port ever
/// leaves RxDetect and CCS never asserts). Pre-drop at EL2, over the proven BPMP channel (step 1 =
/// the bias-pad tracking clock) + direct padctl MMIO (steps 2-9), following Linux
/// `tegra186_utmi_{bias_,}pad_power_on` for tegra234. Programs the HOST pads 0/1/2; leaves pad 3
/// (disabled) alone; leaves VBUS alone (rails are always-on).
///
/// Writes are read-modify-write, so the firmware's resident fuse calibration in the fields we do not
/// name (HS_CURR_LEVEL, HS_SQUELCH_LEVEL) is PRESERVED — strictly better than the mainline
/// clear-then-reprogram cold path, which we cannot run without a fuse read (a later refinement; the
/// dossier explicitly accepts uncalibrated values for first light). Parity delta vs mainline: Linux
/// disables the trk clock after tracking (tegra234 `trk_hw_mode=false`); we leave it on — harmless
/// once TRK_COMPLETED is W1C'd, and one fewer MRQ on the scarce attended boot.
///
/// Best-effort + bounded: the tracking poll is warn-only, so a stuck bias pad costs microseconds,
/// not the boot. padctl is always-powered (Step-0 read it clean), so these writes are NOT a JX1
/// EL3-fatal class — the pre-write banner is discipline, not a live hazard.
pub fn jb2c_padctl_powerup(chan: &super::bpmp_tegra::Chan) {
    // Step 1: the bias-pad tracking clock (BPMP MRQ_CLK). Best-effort; tracking degrades to a
    // warn-only timeout if this failed, and the PD clears still power the pads.
    super::bpmp_tegra::jb2c_usb2_trk_clk(chan);

    // First padctl WRITE of the whole kernel — a new address class, so announce before the touch
    // (the JX1 discipline: a dead boot's last line names the killer). Step-0 proved reads are clean.
    serial_println!(
        ":: tegra: JB2c — padctl @{:#x} pad power-up (pads 0/1/2); pre PAD_MUX={:#010x} PORT_CAP={:#010x} ::",
        PADCTL,
        pr32(PAD_MUX),
        pr32(PORT_CAP),
    );

    // Steps 2-3: PAD_MUX -> XUSB, PORT_CAP -> HOST for the host pads. Idempotent no-ops on this
    // silicon (already 0x55 / 0x111 per Step-0), written anyway for a clean-slate boot; RMW so a
    // uniform 0x55/0x111 is left byte-identical regardless of the exact field width.
    let mut mux = pr32(PAD_MUX);
    let mut cap = pr32(PORT_CAP);
    for &p in &HOST_PADS {
        mux = (mux & !(0b11 << (p * 2))) | (PAD_MUX_XUSB << (p * 2));
        cap = (cap & !(0b11 << (p * 4))) | (PORT_CAP_HOST << (p * 4));
    }
    pw32(PAD_MUX, mux);
    pw32(PORT_CAP, cap);
    dsb();

    // Steps 4-5: per-pad OTG CTL0/CTL1 config. CTL0: clear PD_ZI, set TERM_SEL (HS_CURR_LEVEL left as
    // the firmware calibrated it — the dossier accepts 0, the resident fuse value is better). CTL1:
    // clear TERM_RANGE_ADJ + RPD_CTRL (dossier: nominal termination is fine for first light).
    for &p in &HOST_PADS {
        let c0 = otg_ctl0(p);
        pw32(c0, (pr32(c0) & !OTG_PD_ZI) | TERM_SEL);
        let c1 = otg_ctl1(p);
        pw32(c1, pr32(c1) & !(TERM_RANGE_ADJ | RPD_CTRL));
    }
    dsb();

    // Steps 6-8: the shared bias pad + tracking, ONCE (Linux runs this on the first pad only).
    // 6. BIAS_PAD_CTL1 tracking timers: TRK_START_TIMER=0x1e, TRK_DONE_RESET_TIMER=0x0a.
    pw32(
        BIAS_PAD_CTL1,
        (pr32(BIAS_PAD_CTL1) & !(TRK_START_TIMER | TRK_DONE_RESET_TIMER)) | (0x1e << 12) | (0x0a << 19),
    );
    // 7. BIAS_PAD_CTL0: power the bias pad up (clear BIAS_PAD_PD), HS_DISCON_LEVEL=0x7 (HS_SQUELCH
    //    preserved). udelay(1) to settle.
    pw32(
        BIAS_PAD_CTL0,
        (pr32(BIAS_PAD_CTL0) & !(BIAS_PAD_PD | HS_DISCON_LEVEL)) | (0x7 << 3),
    );
    dsb();
    udelay(1);
    // 8. Tracking: clear PD_TRK to start, poll TRK_COMPLETED (~200 us, warn-only), W1C it, clear CYA.
    pw32(BIAS_PAD_CTL1, pr32(BIAS_PAD_CTL1) & !PD_TRK);
    dsb();
    let completed = poll_trk_completed(200);
    pw32(BIAS_PAD_CTL1, pr32(BIAS_PAD_CTL1) | TRK_COMPLETED); // write-1-to-clear the sticky flag
    // tegra234: trk_update_on_idle -> clear CYA_TRK_CODE_UPDATE_ON_IDLE; trk_hw_mode=false so
    // USB2_TRK_HW_MODE (bit0) stays 0 (this RMW does not touch it).
    pw32(BIAS_PAD_CTL2, pr32(BIAS_PAD_CTL2) & !CYA_TRK_CODE_UPDATE_ON_IDLE);
    dsb();
    udelay(2);
    serial_println!(
        ":: tegra: JB2c — bias pad up, tracking {} (BIAS_CTL0={:#010x} CTL1={:#010x}) ::",
        if completed { "COMPLETED" } else { "timeout (proceeding)" },
        pr32(BIAS_PAD_CTL0),
        pr32(BIAS_PAD_CTL1),
    );

    // Step 9: the two clears that light each port — USB2_OTG_PD (CTL0) + USB2_OTG_PD_DR (CTL1).
    for &p in &HOST_PADS {
        let c0 = otg_ctl0(p);
        pw32(c0, pr32(c0) & !OTG_PD);
        let c1 = otg_ctl1(p);
        pw32(c1, pr32(c1) & !OTG_PD_DR);
    }
    dsb();

    // Step 10: VBUS untouched. Report each pad's final CTL0/CTL1 — PD/PD_ZI/PD_DR should now read 0.
    for &p in &HOST_PADS {
        serial_println!(
            ":: tegra: JB2c — pad {} up: CTL0={:#010x} CTL1={:#010x} (PD b26/PD_ZI b29/PD_DR b2 -> 0) ::",
            p,
            pr32(otg_ctl0(p)),
            pr32(otg_ctl1(p)),
        );
    }
    serial_println!(":: tegra: JB2c — padctl USB2 pad power-up done -> PASS ::");
}

/// JB2b: attach the shared xHCI driver at the raw XUSB MMIO base and pump the polled enumeration
/// until a USB keyboard's interrupt-IN read is armed (or the window closes). Pre-drop, EL2 — the
/// JM4 timer is live here, which is what wakes the driver's bounded `crate::hlt()` sync pumps
/// (hub bring-up, SET_PROTOCOL). Every wait in the driver is budgeted, so the worst case (dead
/// DMA, wedged port) is a few bounded timeouts and the boot proceeds to the JM6b drop unchanged.
///
/// Returns Some((slot, port)) iff a keyboard is armed — the caller's cue to spawn the EL1 pump.
///
/// Deliberately NOT run: `service_storage` / `service_ftdi`. The boot stick will enumerate (its
/// slot configures; that is fine and visible in the log) but its SCSI/BOT bring-up is the JB3
/// arc, not this one — the BOT pump is the driver's heaviest synchronous path and the keyboard
/// does not need it.
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

pub fn jb3_fpci_enable() {
    let r = |off: u64| unsafe { core::ptr::read_volatile((XUSB_FPCI + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe {
        core::ptr::write_volatile((XUSB_FPCI + off) as *mut u32, v)
    };
    serial_println!(":: tegra: JB3 — FPCI @{:#010x} first touch ::", XUSB_FPCI);
    let cfg0 = r(0x0);
    let cfg1 = r(0x4);
    let cfg4 = r(0x10);
    let cfg5 = r(0x14);
    serial_println!(
        ":: tegra: JB3 — FPCI CFG_0={:#010x} CFG_1={:#010x} (io={} mem={} busmaster={}) CFG_4={:#010x} CFG_5={:#010x} ::",
        cfg0,
        cfg1,
        cfg1 & 1,
        (cfg1 >> 1) & 1,
        (cfg1 >> 2) & 1,
        cfg4,
        cfg5
    );
    // BAR0 first if the teardown wiped it (Linux order), then the enables. (Boot-10 rb
    // showed the BAR0 field clamps to 128 KiB granularity — bit 16 is RO — so the complex
    // decodes from 0x0360_0000; expected, not fought.)
    if cfg4 & !0xf == 0 {
        w(0x10, 0x0361_0000);
    }
    // Boot-11: BAR2 (XUSB_CFG_7 @0x1c) routes the ARU/mailbox window @0x365_0000 — wiped by
    // the same teardown (boot-10 proved BAR0 + CFG_1 were). Program it BEFORE any 0x365_0000
    // touch (an unrouted window is the JX1 EL3-fatal class).
    let cfg7 = r(0x1c);
    if cfg7 & !0xf == 0 {
        w(0x1c, 0x0365_0000);
    }
    w(0x4, cfg1 | 0b111); // IO_SPACE_EN | MEM_SPACE_EN | BUS_MASTER_EN
    serial_println!(
        ":: tegra: JB3 — FPCI enable: CFG_1 rb={:#010x} CFG_4 rb={:#010x} CFG_7 {:#010x}->rb={:#010x} ::",
        r(0x4),
        r(0x10),
        cfg7,
        r(0x1c)
    );
}

/// JB3 boot-11: the ARU/BAR2 window (0x365_0000, routed by CFG_7 above) — the controller-side
/// DMA/stream-id config (`IFRDMA_CFG0/1`, `STREAMID_FIELD` @ +0x0e0/0xe4/0xe8) is
/// "firmware-initialized" state (xhci-tegra never rewrites it) — exactly the class this
/// board's ExitBootServices teardown wipes. Dump it; if the stream-id field reads torn (0),
/// program the DTB SID into it. Then send the firmware the `MSG_ENABLED` mailbox handshake
/// exactly as Linux tegra_xusb_enable_firmware_messages does (owner-claim -> DATA_IN ->
/// CMD |= INT_EN|DEST_FALC), bounded-polling DATA_OUT for the ACK — log-heavy, every raw
/// word printed; a missing ACK is telemetry, not a crash.
const XUSB_BAR2: u64 = 0x0365_0000;

pub fn jb3_aru_probe(xusb_sid: u32) {
    let r = |off: u64| unsafe { core::ptr::read_volatile((XUSB_BAR2 + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe {
        core::ptr::write_volatile((XUSB_BAR2 + off) as *mut u32, v)
    };
    serial_println!(":: tegra: JB3 — ARU/BAR2 @{:#010x} first touch ::", XUSB_BAR2);
    let (c0, c1, sidf) = (r(0x0e0), r(0x0e4), r(0x0e8));
    serial_println!(
        ":: tegra: JB3 — ARU IFRDMA_CFG0={:#010x} CFG1={:#010x} STREAMID_FIELD={:#010x} ::",
        c0,
        c1,
        sidf
    );
    if sidf == 0 {
        w(0x0e8, xusb_sid & 0xff);
        serial_println!(
            ":: tegra: JB3 — ARU STREAMID_FIELD <- {:#x}: rb={:#010x} ::",
            xusb_sid & 0xff,
            r(0x0e8)
        );
    }
    // Mailbox: cmd 0x004, data_in 0x008, data_out 0x00c, owner 0x010. OWNER_SW=2.
    // MBOX_CMD_MSG_ENABLED=5 in DATA_IN[31:24]; CMD gets INT_EN(b31)|DEST_FALC(b27).
    let own0 = r(0x010);
    w(0x010, 2);
    let own1 = r(0x010);
    serial_println!(
        ":: tegra: JB3 — MBOX owner {:#x}->claim rb={:#x} cmd={:#010x} data_out={:#010x} ::",
        own0,
        own1,
        r(0x004),
        r(0x00c)
    );
    if own1 == 2 {
        w(0x008, 5u32 << 24); // MSG_ENABLED
        w(0x004, r(0x004) | (1 << 31) | (1 << 27));
        // Bounded ACK poll (~200 µs of reads) — log the final raw state either way.
        let mut spins = 0u32;
        while spins < 50_000 && r(0x00c) == 0 {
            spins += 1;
        }
        serial_println!(
            ":: tegra: JB3 — MBOX MSG_ENABLED sent: cmd={:#010x} data_out={:#010x} owner={:#x} (spins {}) ::",
            r(0x004),
            r(0x00c),
            r(0x010),
            spins
        );
        w(0x010, 0); // release
    } else {
        serial_println!(":: tegra: JB3 — MBOX owner claim did not take; skipping send ::");
    }
}

/// JB3 boot-12: the Falcon — on XUSB the Falcon microcontroller IS the xHC command engine,
/// and boot-11's verdict (mailbox hardware alive, owner claim takes, firmware never answers,
/// ARU config wiped) says the EBS teardown halted it. CSB access (per xhci-tegra t234):
/// page = csb_addr>>9 -> BAR2+0x9c, then BAR2+0x2000+(csb_addr&0x1ff). CPUCTL @ CSB 0x100
/// (STARTCPU=b1, HALTED=b4, STOPPED=b5), BOOTVEC @ 0x104. Firmware header via the
/// FW_SCRATCH ioctl (BAR2+0x1000, type 17<<24, reply @ BAR2+0x1c): word 2 = boot_codetag
/// (the restart vector the ROM loader uses). If halted/stopped: BOOTVEC <- boot_codetag,
/// CPUCTL <- STARTCPU, re-read — the exact tegra_xusb_load_firmware_rom start sequence.
pub fn jb3_falcon() {
    let r = |off: u64| unsafe { core::ptr::read_volatile((XUSB_BAR2 + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe {
        core::ptr::write_volatile((XUSB_BAR2 + off) as *mut u32, v)
    };
    let csb_r = |addr: u32| {
        w(0x9c, (addr >> 9) & 0x7f_ffff);
        r(0x2000 + (addr & 0x1ff) as u64)
    };
    let csb_w = |addr: u32, v: u32| {
        w(0x9c, (addr >> 9) & 0x7f_ffff);
        w(0x2000 + (addr & 0x1ff) as u64, v);
    };
    let fw_hdr = |word_off: u32| {
        w(0x1000, (17u32 << 24) | word_off);
        r(0x1c)
    };
    let cpuctl = csb_r(0x100);
    let bootvec = csb_r(0x104);
    // header words: [0] boot_loadaddr_in_imem, [1] boot_codedfi_offset, [2] boot_codetag,
    // [3] boot_codesize; fwimg_created_time further in — read a few for identity.
    let (h0, h2, h3) = (fw_hdr(0x0), fw_hdr(0x8), fw_hdr(0xc));
    serial_println!(
        ":: tegra: JB3 — FALCON CPUCTL={:#010x} (halted={} stopped={}) BOOTVEC={:#010x} fw[0]={:#010x} codetag={:#010x} codesize={:#010x} ::",
        cpuctl,
        (cpuctl >> 4) & 1,
        (cpuctl >> 5) & 1,
        bootvec,
        h0,
        h2,
        h3
    );
    if cpuctl & ((1 << 4) | (1 << 5)) != 0 || cpuctl == 0 {
        let vec = if h2 != 0 && h2 != 0xffff_ffff { h2 } else { bootvec };
        csb_w(0x104, vec);
        csb_w(0x100, 1 << 1); // CPUCTL_STARTCPU
        // settle a moment, then observe
        let mut spins = 0u32;
        while spins < 100_000 && csb_r(0x100) & ((1 << 4) | (1 << 5)) != 0 {
            spins += 1;
        }
        serial_println!(
            ":: tegra: JB3 — FALCON restart: BOOTVEC<-{:#x} STARTCPU; CPUCTL rb={:#010x} (spins {}) ::",
            vec,
            csb_r(0x100),
            spins
        );
    } else {
        serial_println!(":: tegra: JB3 — FALCON already running; no restart ::");
    }
}

// ---------------------------------------------------------------------------------------------
// JB4-prep: the XUSB Falcon revival readiness check — COMPILE-GATED (feature="tegra") + DORMANT.
//
// JB3 pinned the root cause five layers past the SMMU: every fabric layer (SMMU, MC-SID, FPCI,
// ARU) is restored + fault-free, but the Falcon microcontroller — the xHC command engine — is
// halted (CSB CPUCTL/BOOTVEC read 0xffffffff), so nothing DMAs. JB4-prep is the OFFLINE research +
// revival code for the attended bench that follows. What the Phase-A research + adversarial verify
// pinned from primary sources (full dossier + per-claim confidence in arch_arm64.md §JB4-prep):
//
//   * The dead CSB is a HALTED FALCON CORE, not a code bug. The BAR2 CSB aperture UnaOS uses is
//     byte-for-byte the mainline t234 path (ARU_C11_CSBRANGE @0x9c page-select, CSB_BASE_ADDR
//     @0x2000); the clock (269) is already enabled by JB1c and usb@3610000 has no `resets`. So the
//     Falcon's internal CSB slave floats all-ones because the core is not executing — while the ARU
//     wrapper (mailbox, FW-header ioctl) outside the core keeps answering. (Verify: the "missing
//     clock / wrong routing" and "MRQ_RESET clears it" hypotheses were both REFUTED.)
//   * On t234 the Falcon AUTO-BOOTS its resident firmware; the OS NEVER restarts it via CSB.
//     tegra234_soc.firmware=NULL → tegra_xusb_init_ifr_firmware() only polls xHCI USBSTS.CNR until
//     the Falcon self-boots and clears Controller-Not-Ready. The CSB BOOTVEC+CPUCTL_STARTCPU
//     restart exists only for chips WITH .firmware — it is the WRONG path for t234 (and unreachable
//     through the dead window). So the ONE true "Falcon is up" signal here is USBSTS.CNR clearing.
//   * The only lever that re-runs the Falcon's on-power-up self-boot is an MRQ_PG partition OFF->ON
//     (bpmp_tegra::jb4_powergate_cycle) — the boot-2 fork, which wipes the partition and needs the
//     JB3 chain re-run after. The cheapest real lever is Peter's zero-code FIRMWARE-SLOT ROLLBACK
//     (bench boot 0), whose gentler EBS exit leaves the Falcon running.
//
// So jb4_falcon_revive is a NON-DESTRUCTIVE READINESS CHECK + honest STOP: baseline read, a witness
// clock/power re-assert (proves it is not a dropped clock), a bounded USBSTS.CNR poll (the true
// signal), and — only if CNR clears — the MSG_ENABLED handshake. If CNR stays set it STOPs and
// names the two real levers (the MRQ_PG partition cycle, or the rollback). It performs the same
// readiness check the seat re-runs AFTER the boot-2 partition cycle. DORMANCY is two-layer:
// feature="tegra" keeps it out of every QEMU regression (non-tegra builds byte-identical), and
// `JB4_ENABLE` gates every register touch at run time. This arc ships it FALSE and never wires it
// into tegra_early_stop; the seat flips JB4_ENABLE and adds the call at the bench. Depends on JB3
// having run first (BAR2/FPCI routed, ARU restored).
// ---------------------------------------------------------------------------------------------

/// JB4 master run-time guard. FALSE this arc → dormant (jb4_falcon_revive prints one line and
/// returns without touching a register). The seat flips it to true — together with wiring the
/// call into tegra_early_stop — at the attended bench.
/// JB5 probe media: back to FALSE — the JB4 boots (1+2) answered their question (a real MRQ_PG
/// partition cycle does NOT re-run the Falcon self-boot); the JB5 bisect wants a pure chain.
pub const JB4_ENABLE: bool = false;

/// Secondary guard for the partition-reset revival lever (bpmp_tegra::jb4_powergate_cycle, boot 2).
/// Stays FALSE even when JB4_ENABLE is true — the seat flips it ONLY for the explicit boot-2 fork
/// (the cycle wipes the partition and requires re-running the JB3 chain afterward).
pub const JB4_ALLOW_PG_CYCLE: bool = false;

// Falcon CSB register address (mainline xhci-tegra XUSB_FALC_CPUCTL), read through the BAR2 ARU
// CSB-range page mechanism (csb_r) for the log only — NOT written (the CSB restart is the wrong
// path for t234; see the block comment). CPUCTL bits: STATE_HALTED b4, STATE_STOPPED b5.
const FALC_CPUCTL: u32 = 0x100;

/// The xHCI operational USBSTS.CNR — the ONE true "Falcon has booted its resident firmware" signal
/// on t234 (mainline tegra_xusb_wait_for_falcon polls exactly this). op_base = XUSB_HOST +
/// CAPLENGTH; USBSTS @ op+0x04; CNR (Controller-Not-Ready) = bit 11. `Some(true)` = not ready,
/// `Some(false)` = ready (Falcon up), `None` = the block's cap word is dead (not ungated/alive).
fn xusb_cnr_set() -> Option<bool> {
    let cap0 = unsafe { core::ptr::read_volatile(XUSB_HOST as *const u32) };
    if cap0 == 0 || cap0 == 0xFFFF_FFFF {
        return None;
    }
    let caplength = (cap0 & 0xff) as u64;
    let usbsts = unsafe { core::ptr::read_volatile((XUSB_HOST + caplength + 0x04) as *const u32) };
    Some((usbsts >> 11) & 1 != 0)
}

/// JB4-prep: non-destructive readiness check for the halted XUSB Falcon. `chan` = the proven BPMP
/// channel; `ids` = the XUSB resource ids JB1c resolved from the live DTB (its power domains are
/// re-asserted as a witness). Runs AFTER the JB3 chain (needs BAR2/FPCI routed + ARU restored). It
/// makes NO CSB writes (the t234 Falcon self-boots; the CSB restart is the wrong path); the true
/// signal is USBSTS.CNR. The seat calls this both standalone (boot 1, expected STOP) and after the
/// boot-2 partition cycle + JB3-chain re-run (expected PASS if the Falcon re-booted).
pub fn jb4_falcon_revive(chan: &super::bpmp_tegra::Chan, ids: &super::fdt_tegra::XusbIds) {
    if !JB4_ENABLE {
        serial_println!(":: tegra: JB4 — falcon revive DORMANT (JB4_ENABLE=false); no writes ::");
        return;
    }
    let r = |off: u64| unsafe { core::ptr::read_volatile((XUSB_BAR2 + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe { core::ptr::write_volatile((XUSB_BAR2 + off) as *mut u32, v) };
    let csb_r = |addr: u32| {
        w(0x9c, (addr >> 9) & 0x7f_ffff);
        r(0x2000 + (addr & 0x1ff) as u64)
    };
    let fw_hdr = |byte_off: u32| {
        w(0x1000, (17u32 << 24) | byte_off); // FW_IOCTL_CFGTBL_READ (empirically works, JB3 boot-12)
        r(0x1c) // ARU_SMI_ARU_FW_SCRATCH_DATA0
    };
    serial_println!(":: tegra: JB4 — falcon readiness: ARU/BAR2 @{:#010x} (JB4_ENABLE=true) ::", XUSB_BAR2);

    // 1. Baseline: firmware image still resident (header readable)? mailbox owner? CSB alive? CNR?
    let codetag = fw_hdr(0x08); // header boot_codetag @0x08
    let codesize = fw_hdr(0x0c); // header boot_codesize @0x0c (0xc85f when resident, JB3 boot-12)
    let owner0 = r(0x010);
    let cpuctl0 = csb_r(FALC_CPUCTL); // read-only witness; 0xffffffff while the core is halted
    let cnr0 = xusb_cnr_set();
    serial_println!(
        ":: tegra: JB4 — baseline: fw codetag={:#010x} codesize={:#010x} mbox_owner={:#x} CSB CPUCTL={:#010x} USBSTS.CNR={} ::",
        codetag, codesize, owner0, cpuctl0,
        match cnr0 { Some(true) => "1 (not ready)", Some(false) => "0 (READY)", None => "n/a (cap dead)" }
    );
    if codesize == 0 || codesize == 0xffff_ffff {
        serial_println!(
            ":: tegra: JB4 — fw header not readable (codesize={:#010x}); the ARU wrapper is torn down -> STOP: re-run the JB3 FPCI/ARU restore first ::",
            codesize
        );
        return;
    }

    // 2. Witness re-assert of the Falcon clock tree + power domains (IMEM-preserving; ON only). NOT
    //    expected to fix anything — the Falcon clock 269 is already enabled by JB1c — its value is
    //    to prove the dead CSB is not a dropped-clock case. See bpmp_tegra::jb4_reassert_falcon.
    super::bpmp_tegra::jb4_reassert_falcon(chan, ids);

    // 3. Poll USBSTS.CNR — the ONE true readiness signal (mainline waits up to 200 ms for the
    //    Falcon to self-boot and clear it). Bounded 300 ms on CNTPCT. On the non-destructive path
    //    this will stay set (the core is halted); after the boot-2 partition cycle + JB3 re-run it
    //    should clear if the Falcon re-loaded its resident image.
    let deadline = cntpct().wrapping_add(cntfrq().saturating_mul(300) / 1000);
    let mut ready = false;
    loop {
        if xusb_cnr_set() == Some(false) {
            ready = true;
            break;
        }
        if cntpct().wrapping_sub(deadline) < (1u64 << 63) {
            break; // deadline passed (wrap-safe)
        }
        core::hint::spin_loop();
    }
    let cpuctl_now = csb_r(FALC_CPUCTL);
    serial_println!(
        ":: tegra: JB4 — post-reassert: CNR {} CSB CPUCTL={:#010x} (halted={} stopped={}) ::",
        if ready { "CLEARED (Falcon up)" } else { "still set" },
        cpuctl_now, (cpuctl_now >> 4) & 1, (cpuctl_now >> 5) & 1
    );
    if !ready {
        serial_println!(
            ":: tegra: JB4 — Falcon still Not-Ready (CNR set) -> STOP: on t234 the Falcon boots its resident carveout image on power-up (IFR) and this non-secure window cannot restart a halted core (the CSB BOOTVEC/STARTCPU path is wrong for t234). Levers: MRQ_PG OFF->ON partition cycle + re-run the JB3 chain (jb4_powergate_cycle, boot 2), or the firmware-slot rollback (boot 0) ::"
        );
        return;
    }

    // 4. CNR clear -> the Falcon is running. Confirm with the MSG_ENABLED handshake (owner-claim ->
    //    DATA_IN -> CMD|=INT_EN|DEST_FALC), bounded ACK poll. Mirrors jb3_aru_probe / mainline
    //    tegra_xusb_enable_firmware_messages (OWNER_SW=2). A live Falcon services this; a false CNR
    //    would show as no ACK here.
    let own_pre = r(0x010);
    w(0x010, 2); // OWNER_SW
    if r(0x010) != 2 {
        serial_println!(
            ":: tegra: JB4 — CNR clear but mailbox owner claim did not take (owner={:#x}); re-check -> STOP ::",
            r(0x010)
        );
        return;
    }
    w(0x008, 5u32 << 24); // DATA_IN: MBOX_CMD_MSG_ENABLED (5) in [31:24]
    w(0x004, r(0x004) | (1 << 31) | (1 << 27)); // CMD: INT_EN(b31) | DEST_FALC(b27)
    let mut spins = 0u32;
    while spins < 250_000 && r(0x010) == 2 && r(0x00c) == 0 {
        spins += 1;
    }
    let (cmd_rb, dout_rb, own_rb) = (r(0x004), r(0x00c), r(0x010));
    if own_rb == 2 {
        w(0x010, 0); // release if the firmware never did
    }
    let fw_serviced = own_rb != 2 || dout_rb != 0;
    serial_println!(
        ":: tegra: JB4 — MSG_ENABLED: cmd={:#010x} data_out={:#010x} owner {:#x}->{:#x} (spins {}) -> {} ::",
        cmd_rb, dout_rb, own_pre, own_rb, spins,
        if fw_serviced {
            "RUNNING + firmware servicing (Falcon revived) -> PASS"
        } else {
            "CNR clear but firmware silent on the mailbox -> STOP (re-run the JB3 chain / rollback)"
        }
    );
}

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

/// JB5 run E: release the SS ELPG clamps — the InitHw step jb2c never covered. UEFI's
/// UsbPadCtlDxe DeInitHw (EBS) powers USB3 down per port: set SSPX_ELPG_CLAMP_EN_EARLY, 200 µs,
/// set SSPX_ELPG_CLAMP_EN, 350 µs, set SSPX_ELPG_VCORE_DOWN — so every UnaOS boot inherits the SS
/// partition CLAMPED with VCORE down, and nothing in the JB chain ever released it. InitHw's
/// InitUsb3PadX clears all three per port in XUSB_PADCTL_ELPG_PROGRAM_1_0 (@0x24; t186+ layout:
/// port i uses bits 3i CLAMP_EN / 3i+1 CLAMP_EN_EARLY / 3i+2 VCORE_DOWN). Clear the 4 ports' 12
/// bits (RMW), print pre/post. (UsbPadCtlTegra234.c, source-verified 2026-07-08.)
pub fn jb5_elpg_release() {
    if !JB5_PROBE {
        return;
    }
    let r = |off: u64| unsafe { core::ptr::read_volatile((PADCTL + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe {
        core::ptr::write_volatile((PADCTL + off) as *mut u32, v)
    };
    let pre = r(0x24);
    w(0x24, pre & !0xfff); // SSP0..SSP3 × {CLAMP_EN, CLAMP_EN_EARLY, VCORE_DOWN}
    serial_println!(
        ":: tegra: JB5 — ELPG_PROGRAM_1 SS clamp release: {:#010x} -> rb={:#010x} (12 low bits cleared) ::",
        pre,
        r(0x24)
    );
}

/// JB5 run D: the XhciControllerDxe FPCI config, verbatim from source (XhciControllerPrivate.h:
/// CFG_1@0x04, CFG_4@0x10, CFG_7@0x1c, AXI_CFG_0@0xF8; CNR=b11, HCE=b12): program BAR0/BAR2
/// bases, AXI_CFG_0 <- 0x5 (never written by any prior UnaOS boot), CFG_1 |= mem+busmaster —
/// then poll the TRUE readiness signal, CSB CPUCTL != 0xffffffff, for ~300 ms (UEFI polls CNR
/// for 200 ms, but CNR is a proven stale latch on this board). Returns cap0-alive.
pub fn jb5_fpci_uefi_and_poll() -> bool {
    if !JB5_PROBE {
        return false;
    }
    let r = |off: u64| unsafe { core::ptr::read_volatile((XUSB_FPCI + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe {
        core::ptr::write_volatile((XUSB_FPCI + off) as *mut u32, v)
    };
    serial_println!(":: tegra: JB5 — XhciControllerDxe FPCI replay @{:#010x} ::", XUSB_FPCI);
    let (cfg1, cfg4, cfg7, axi) = (r(0x4), r(0x10), r(0x1c), r(0xf8));
    serial_println!(
        ":: tegra: JB5 — post-cycle FPCI: CFG_1={:#010x} CFG_4={:#010x} CFG_7={:#010x} AXI_CFG_0={:#010x} ::",
        cfg1, cfg4, cfg7, axi
    );
    w(0x10, 0x0361_0000); // CFG_4: BAR0
    w(0x1c, 0x0365_0000); // CFG_7: BAR2 (ARU/CSB window)
    w(0xf8, 0x5); // AXI_CFG_0 <- 0x5 — the UEFI write no UnaOS boot has ever made
    w(0x4, r(0x4) | 0b110); // CFG_1: MEM_SPACE(b1) | BUS_MASTER(b2), exactly UEFI's two bits
    serial_println!(
        ":: tegra: JB5 — FPCI programmed: CFG_1 rb={:#010x} CFG_4 rb={:#010x} CFG_7 rb={:#010x} AXI_CFG_0 rb={:#010x} ::",
        r(0x4), r(0x10), r(0x1c), r(0xf8)
    );
    let cap0 = unsafe { core::ptr::read_volatile(XUSB_HOST as *const u32) };
    if cap0 == 0 || cap0 == 0xFFFF_FFFF {
        serial_println!(":: tegra: JB5 — cap0={:#010x} (dead post-replay) -> STOP ::", cap0);
        return false;
    }
    // The Falcon self-boot window: poll CPUCTL through the freshly-routed BAR2 CSB page window.
    let br = |off: u64| unsafe { core::ptr::read_volatile((XUSB_BAR2 + off) as *const u32) };
    let bw = |off: u64, v: u32| unsafe {
        core::ptr::write_volatile((XUSB_BAR2 + off) as *mut u32, v)
    };
    let csb_cpuctl = || {
        bw(0x9c, (0x100u32 >> 9) & 0x7f_ffff);
        br(0x2000 + (0x100 & 0x1ff) as u64)
    };
    let deadline = cntpct().wrapping_add(cntfrq().saturating_mul(300) / 1000);
    let mut cpuctl = csb_cpuctl();
    while cpuctl == 0xFFFF_FFFF && cntpct().wrapping_sub(deadline) >= (1u64 << 63) {
        core::hint::spin_loop();
        cpuctl = csb_cpuctl();
    }
    let cl = (cap0 & 0xff) as u64;
    let sts = unsafe { core::ptr::read_volatile((XUSB_HOST + cl + 0x04) as *const u32) };
    serial_println!(
        ":: tegra: JB5 — Falcon-boot poll: CPUCTL={:#010x} ({}) USBSTS={:#010x} (CNR={} HCE={}) ::",
        cpuctl,
        if cpuctl != 0xFFFF_FFFF { "ALIVE — self-boot ran" } else { "still 0xffffffff after 300ms" },
        sts, (sts >> 11) & 1, (sts >> 12) & 1
    );
    true
}

/// JB5 run C: bounded ~300 ms settle (CNTPCT, mainline waits up to 200 ms for the Falcon
/// self-boot) then re-witness — if the Linux-order power-up edge kicked the boot-ROM, CPUCTL
/// flips from 0xffffffff to sane bits within this window.
pub fn jb5_settle_witness(tag: &str) {
    if !JB5_PROBE {
        return;
    }
    let deadline = cntpct().wrapping_add(cntfrq().saturating_mul(300) / 1000);
    while cntpct().wrapping_sub(deadline) >= (1u64 << 63) {
        core::hint::spin_loop();
    }
    jb5_witness(tag);
}

pub fn jb2b_attach(dma_coherent: Option<bool>) -> Option<(u8, u8)> {
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

    // The exact sequence the PCIe paths run (arch/aarch64/pci.rs), minus discovery/bus-master —
    // a platform controller has no config space; DMA mastering came with the BPMP ungate.
    xhci::init(XUSB_HOST); // halt + HCRST + CNR wait (Falcon survives; see header)
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
        let erst_table_phys = &raw mut xhci::ERST_TABLE as u64;
        // One line before the first RUNTIME-register / doorbell-array touch (new offsets within
        // the ungated block — the JX1 discipline: a dead boot's last line names the killer).
        serial_println!(":: tegra: JB2b — programming interrupter + rings (runtime regs) ::");
        x.init_interrupter(event_ring_phys, erst_table_phys);
        x.init_pointers(command_ring_phys);
        x.start();
        *xhci::XHCI_CONTROLLER.lock() = Some(x);
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
    let deadline = cntpct().wrapping_add(cntfrq().saturating_mul(60));
    loop {
        let armed = {
            let mut guard = xhci::XHCI_CONTROLLER.lock();
            let x = guard.as_mut().unwrap();
            x.poll_events();
            x.service_hubs();
            x.service_hid_setproto();
            x.service_slot_disposal();
            x.service_enum();
            keyboard_armed(x)
        };
        if let Some((slot, port)) = armed {
            serial_println!(
                ":: tegra: JB2b — keyboard ARMED (slot {}, root port {}) -> PASS ::",
                slot,
                port
            );
            return Some((slot, port));
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
        if let Some(x) = xhci::XHCI_CONTROLLER.lock().as_mut() {
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
