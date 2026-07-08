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
/// JB9c (bench 2026-07-08): TRUE again — JB4's verdict was judged on CNR + CPUCTL, BOTH since
/// proven unreliable (CNR = stale latch, CPUCTL = CSB priv-lock read), and today's AO capture
/// shows IFRDMA_CFG0/1 still hold the fw buffer PA in the always-on domain — everything the
/// Falcon ROM needs to self-load after a power-up. Re-run the PG cycle, judged this time by the
/// JB9 witnesses (mailbox retry ladder, ring DMA, AO write-lock state), not the two liars.
/// JB9g: back to FALSE — the JB9c boot answered it (true rail drop, post-OFF=0x0, and the FW
/// still never services; the t234 ROM does NOT re-run IFR autoboot on a bare PG-on). Worse: with
/// the FW un-reloadable from NS, the cycle DESTROYS the inherited firmware the JB9g no-HCRST
/// takeover depends on. Keep false on every inherit-path boot.
pub const JB4_ENABLE: bool = false;

/// Secondary guard for the partition-reset revival lever (bpmp_tegra::jb4_powergate_cycle, boot 2).
/// Stays FALSE even when JB4_ENABLE is true — the seat flips it ONLY for the explicit boot-2 fork
/// (the cycle wipes the partition and requires re-running the JB3 chain afterward).
/// JB9c: answered (see JB4_ENABLE) — back to FALSE; the cycle kills the inherited firmware.
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
pub const JB9_PROBE: bool = true;

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

/// JB9b lever 1 (bench 2026-07-08): the first JB9 boot's forensics converged on a SID MISMATCH —
/// AO `IFRDMA_STREAMID` reads 0x7f (the legacy BYPASS SID) while the MC overrides and the SMMU
/// matching are built around the DTB's 0xe, and MB2's "SMMU external bypass disable" policy
/// refuses bypass-class traffic: silent drop, zero faults, empty rings. Retag the FW side: write
/// the DTB SID into AO IFRDMA_STREAMID (UEFI programs this register with plain NS MMIO, so the
/// write should take). Read-back printed — if it refuses, lever 2 (accept 0x7f at the SMMU)
/// catches the traffic instead.
pub fn jb9b_ao_sid_fix(ao: Option<u64>, sid: u32) {
    if !JB9_PROBE {
        return;
    }
    let Some(base) = ao else {
        serial_println!(":: tegra: JB9b — AO SID fix: SKIP (no AO base from DTB) ::");
        return;
    };
    let r = |off: u64| unsafe { core::ptr::read_volatile((base + off) as *const u32) };
    let w = |off: u64, v: u32| unsafe { core::ptr::write_volatile((base + off) as *mut u32, v) };
    let pre = r(0x1c4);
    serial_println!(
        ":: tegra: JB9b — AO IFRDMA_STREAMID @{:#010x}+0x1c4: pre={:#010x} <- {:#x} ::",
        base,
        pre,
        sid & 0xff
    );
    if pre & 0xff != sid & 0xff {
        w(0x1c4, sid & 0xff);
        dsb();
    }
    serial_println!(
        ":: tegra: JB9b — AO IFRDMA_STREAMID rb={:#010x} ({}) ::",
        r(0x1c4),
        if r(0x1c4) & 0xff == sid & 0xff { "TOOK" } else { "REFUSED" }
    );
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
        let erst_table_phys = &raw mut xhci::ERST_TABLE as u64;
        // One line before the first RUNTIME-register / doorbell-array touch (new offsets within
        // the ungated block — the JX1 discipline: a dead boot's last line names the killer).
        serial_println!(":: tegra: JB2b — programming interrupter + rings (runtime regs) ::");
        x.init_interrupter(event_ring_phys, erst_table_phys);
        x.init_pointers(command_ring_phys);
        x.start();
        *xhci::XHCI_CONTROLLER.lock() = Some(x);
    }

    // JB9i (bench 2026-07-08): evict UEFI's stale device slots. On the inherit path the firmware
    // still holds the slots UEFI enumerated (it was never reset), so our fresh ENABLE_SLOT
    // succeeds (slot 5) but ADDRESS_DEVICE on a device the FW considers already-owned fails with
    // completion code 17 (Parameter Error). DISABLE_SLOT 1..8 is the spec-clean reclaim — a
    // not-enabled slot just completes with code 11 (Slot Not Enabled), harmless.
    if JB9G_NO_HCRST {
        let mut guard = xhci::XHCI_CONTROLLER.lock();
        if let Some(x) = guard.as_mut() {
            for slot in 1u8..=8 {
                let trb = xhci::trb::Trb {
                    parameter: 0,
                    status: 0,
                    control: (10 << 10) | ((slot as u32) << 24), // TRB type 10 = Disable Slot
                };
                let _ = x.send_command(trb);
            }
            // Drain the eight completions (bounded ~100 ms) before enumeration starts, so no
            // stale completion aliases against the enum FSM's own commands.
            let deadline = cntpct().wrapping_add(cntfrq() / 10);
            while cntpct().wrapping_sub(deadline) >= (1u64 << 63) {
                x.poll_events();
                core::hint::spin_loop();
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
