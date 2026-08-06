// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! EHCI-3 — a minimal, polling-first EHCI HID driver (feature `ehcihid`). EHCI-4 M1: DEFAULT-ON
//! on x86 (the internal keyboard is metal-proven to type; usb_xhci.md §10a); `UNAOS_NOEHCIHID=1`
//! opts out => this module + every call site unlink, byte-identical to the pre-fold no-EHCI media.
//!
//! Purpose: the 2012 rMBP's internal keyboard + trackpad are real USB devices asleep behind the
//! Panther Point EHCI companions on NON-switchable ports (EHCI-2 metal census: censusB = one
//! connected device per function, J-state, EHCI-owned). PORTSW routes only the *switchable*
//! shared ports to xHCI, so the two stacks own disjoint ports by hardware — this driver runs
//! permanently alongside the xHCI driver and never touches an xHCI register.
//!
//! Shape (design doc EHCI3-DESIGN): builds on the shared EHCI-2 wake (`ehci_scout::wake_run`/
//! `wake_route` — one wake path, not two), adds the transaction-initiating layer: port reset, an
//! async schedule with ONE reusable control QH for synchronous enumeration, a periodic frame
//! list with one interrupt QH per HID endpoint, and a main-loop `service_ehci_hid()` poll that
//! feeds boot reports through the same decode idiom (and scancode table) as the xHCI HID path.
//! No interrupts: no USBINTR write, no IDT vector, no MSI.
//!
//! Topology fork (the M1 evidence gate): the first GET_DESCRIPTOR on the root-port device
//! decides on a serial line whether it is the integrated Rate-Matching Hub (bDeviceClass 0x09 →
//! Topology A: hub walk + split transactions to the FS/LS children through the RMH's TT) or a
//! direct HID device (Topology B: no splits). The QH builder parameterizes hub-addr/port/
//! S-mask/C-mask — zero for B, populated for A — so both branches share every other line of
//! machinery. QEMU exercises B only (its usb-kbd trains high-speed on the root port; QEMU has
//! no TT/HS-hub model — the FS usb-hub cannot even coexist with firmware on the EHCI bus), so
//! Topology A is metal-first by construction.
//!
//! WRITE SURFACE (tripwire-grade; the EHCI-2 wake surface plus, all declared): PORTSC.PR (RW1C
//! bits masked), USBCMD.ASE/PSE, PERIODICLISTBASE / ASYNCLISTADDR / CTRLDSSEGMENT=0, USBSTS RW1C
//! acks, the driver's own frame-list/QH/qTD/buffer DMA memory, EP0 device + hub-class requests,
//! and (Peter-approved surface extension, 2026-07-16) the EHCI functions' own PCI COMMAND
//! Memory-Space + Bus-Master enables — read-checked, set only if clear, traced. Never any xHCI
//! register, never a switchable-mask port, never an unlisted register. Every MMIO access is
//! translate()-guarded; every wait is bounded; a stuck handshake is a traced STOP-NOTE.

pub mod qh;

use super::ehci_scout::{
    self, mmio_read32, mmio_write32, settle_ms, wait_bounded, EhciFnHandle, OP_PORTSC0, OP_USBCMD,
    OP_USBSTS,
};
use crate::arch::pci::{read_config_16, read_config_32, write_config_32};
use alloc::vec::Vec;
use qh::*;
use spin::Mutex;

/// EHCI class code (base 0x0C Serial Bus, subclass 0x03 USB, prog-IF 0x20 EHCI).
const EHCI_CLASS: u8 = 0x0C;
const EHCI_SUBCLASS: u8 = 0x03;
const EHCI_PROGIF: u8 = 0x20;

/// Operational registers this driver adds beyond the scout's set.
const OP_CTRLDSSEGMENT: u64 = 0x10;
const OP_PERIODICLISTBASE: u64 = 0x14;
const OP_ASYNCLISTADDR: u64 = 0x18;

/// USBCMD bits.
const CMD_RS: u32 = 1 << 0;
const CMD_HCRESET: u32 = 1 << 1;
const CMD_PSE: u32 = 1 << 4;
const CMD_ASE: u32 = 1 << 5;

/// PORTSC bits (see ehci_scout for the census decode of the same register).
const PORT_CCS: u32 = 1 << 0;
const PORT_PED: u32 = 1 << 2;
const PORT_PR: u32 = 1 << 8;
const PORT_OWNER: u32 = 1 << 13;
/// PORTSC RW1C change bits (CSC/PEC/OCC) — masked off every read-modify-write so a routine
/// PR/PP write never silently acknowledges a latched change (the EHCI-2 discipline).
const PORT_RW1C: u32 = (1 << 1) | (1 << 3) | (1 << 5);

/// USBSTS RW1C status bits acked during polling (USBINT/ERR/port-change/rollover/host-error/
/// async-advance). Ack-only — USBINTR is never written (polling model).
const STS_RW1C: u32 = 0x3F;
/// USBSTS **Periodic** Schedule Status, EHCI 1.0 §2.3.2 bit 14 (read-only; tracks USBCMD.PSE
/// with a lag). The name is right and always was; this comment used to read "Async Schedule
/// Status", which is bit 15. Worth correcting rather than tolerating: the M3 trim in c90599f1
/// turns on reasoning about precisely this bit — it skips the PSS-disable wait on the HSE path —
/// and a reader checking that argument against a comment naming the wrong schedule would
/// conclude the trim was unsound.
const STS_PSS: u32 = 1 << 14;
const STS_HCHALTED: u32 = 1 << 12;
const STS_HSE: u32 = 1 << 4; // Host System Error (DMA master/target abort) — halts the HC

/// One endpoint-addressing tuple for the shared control QH: everything dword 1/2 of a QH needs.
/// `hub_addr`/`hub_port` are the TT fields — zero on Topology B / high-speed targets, the RMH's
/// address + downstream port on Topology A (that parameterization IS the branch reachability).
#[derive(Clone, Copy)]
struct Target {
    addr: u8,
    mps0: u16,
    eps: u32, // one of QH_EPS_FULL/LOW/HIGH
    hub_addr: u8,
    hub_port: u8,
}

/// One armed HID interrupt-IN endpoint: its periodic QH, single re-armed qTD, report buffer,
/// and the software-tracked data toggle (QH runs DTC=1 so the toggle is explicit here, never
/// hidden controller state).
struct IntEp {
    qh: *mut Qh,
    qtd: *mut Qtd,
    qtd_phys: u64,
    buf: *mut u8,
    buf_phys: u64,
    mps: u16,
    toggle: bool,
    is_kbd: bool,
    is_rel_mouse: bool,
    /// EHCI-4 M2: Some for a report-protocol pointer (the trackpad path) — the field map parsed
    /// from the interface's HID report descriptor. `is_kbd`/`is_rel_mouse` are false then; the
    /// service loop decodes X/Y/buttons from this layout instead of a fixed boot-report offset.
    layout: Option<ReportLayout>,
    reports: u32,
    dead: bool,
    /// CLICK-1: previous report's button bitmask, for button-DOWN edge detection (one
    /// `pal::Event::Button` per press, nothing on release/hold).
    prev_buttons: u8,
    /// CLICK-3: `arch::ms()` at the previous report consumed on THIS endpoint (any report — motion,
    /// button, or idle keep-alive). The re-press recovery below reads the SILENCE between reports,
    /// so it must be stamped by every report, not only by button ones. 0 = no report yet.
    last_report_ms: u64,
    /// KBDWIT — `arch::ms()` when this endpoint was ARMED. The silence clock's origin for an
    /// endpoint that has never completed anything (the s58 keyboard's exact state). Deliberately
    /// separate from `last_report_ms`, which only the POINTER paths stamp (`note_buttons`) and so
    /// stays 0 forever on a keyboard — reusing it would have made every keyboard look silent from
    /// boot regardless of how many reports it delivered.
    #[cfg(feature = "kbdwit")]
    kbdwit_armed_ms: u64,
    /// KBDWIT — `arch::ms()` at the last COMPLETION on this endpoint (any retired qTD, report
    /// bytes or not), stamped by the service loop itself rather than by any decoder, so no report
    /// layout can affect whether the endpoint counts as alive. 0 = nothing has ever completed.
    #[cfg(feature = "kbdwit")]
    kbdwit_last_ms: u64,
    /// KBDWIT — the one-shot latch. Set by the first (and only) dump this endpoint will ever emit.
    #[cfg(feature = "kbdwit")]
    kbdwit_fired: bool,
    /// KBDWIT — FRINDEX sampled at ARM time, the baseline the dump's `adv=` is measured against.
    /// This is the instrument's only real answer to "is the periodic schedule advancing?": the
    /// arm→fire window is >= `KBDWIT_QUIET_MS`, so a frame counter that has not moved across it is
    /// unambiguously frozen, whereas two reads taken microseconds apart inside the dump cannot
    /// distinguish a frozen counter from one that simply has not ticked yet (FRINDEX advances every
    /// 125 us). `None` = the baseline MMIO read failed, and `adv` is not computed at all rather than
    /// computed from a fabricated zero. Costs exactly one MMIO read per endpoint (<= 4 per
    /// controller) on the arming path, of a register in the BAR that path is already driving.
    #[cfg(feature = "kbdwit")]
    kbdwit_armed_frindex: Option<u32>,
    /// MT-INVESTIGATION (IVY, `mtraw` only): bytes ONE armed transfer may accept — `mps` for every
    /// endpoint except the vendor-multitouch one, which is armed for the whole (grown) receive
    /// buffer so the controller accumulates a >MPS raw frame into it. See `arm_interrupt_ep`.
    #[cfg(feature = "mtraw")]
    rx_total: u32,
    /// MT-INVESTIGATION (IVY, `mtraw_inject` sub-knob only): previous decoded first-finger absolute
    /// position, for turning TYPE2 absolute coordinates into pointer DELTAS. `None` until the first
    /// touching frame and again on finger-up, so a lift never emits a jump.
    #[cfg(feature = "mtraw_inject")]
    mt_prev: Option<(i32, i32)>,
}

// ======================================================================================
// CLICK-3 — "a second stationary click is ignored" (metal, s41 rMBP; Peter at the trackpad:
// *click, pause, click* loses the second click, while *click, slide, click* works).
//
// MECHANISM (traced end to end; the loss is BEFORE `push_event`):
//   * parse: the pointer paths below emit `pal::Event::Button` on a button-DOWN EDGE only —
//     `buttons & 0x01 != 0 && prev_buttons & 0x01 == 0` — with `prev_buttons` carried per endpoint.
//   * `pal::push_event` / `EventQueue` have NO dedup and NO single-slot latch (pal.rs push/pop are a
//     plain ring with drop accounting), and the consumers act on every Button they see
//     (`vug::drain_input` exits on any Button; the x86 console loop ignores Button entirely).
//   * So the only state anywhere on the path that (a) gates a click and (b) is cleared by POINTER
//     MOTION is `prev_buttons`: a motion report carries `buttons == 0x00`, which resets the latch.
//     That is exactly the observed asymmetry.
//
// WHY THE LATCH GOES STALE: the interrupt endpoint is armed for ONE report per service pass, and
// `service_ehci_hid` is polled from the console frame loop (main.rs) — i.e. at frame rate, orders of
// magnitude slower than the endpoint's interval. A report is a LEVEL (the current button state), so
// a release that lands in the gap between two polls is superseded by whatever the pad reports next:
// miss the release and `prev_buttons` stays latched at 0x01 forever, and every subsequent stationary
// press fails the edge test. Any motion at all clears it — hence "slide to register a click".
//
// FIX (consumer-side of the transfer machinery; the EHCI transfer path itself is untouched): keep
// the edge test, and ADD a re-press recovery that reads the SILENCE between reports. A held button
// either re-reports at the endpoint's rate (so consecutive pressed reports arrive far closer than
// `CLICK_REPRESS_QUIET_MS`) or reports nothing at all until release (so no pressed report arrives
// during the hold). Neither can produce a pressed report separated from the previous report by a
// long quiet gap — but a *new press after a missed release* always is. So: a report that still reads
// "primary down" after ≥ `CLICK_REPRESS_QUIET_MS` of endpoint silence is a NEW press. No protection
// is weakened and no held-button case gains a spurious repeat.
// ======================================================================================

/// CLICK-3 — endpoint silence (ms) after which a still-pressed report counts as a NEW press rather
/// than a hold. Sits well above any plausible poll interval for this path (the console frame loop
/// services EHCI HID every frame, tens of ms at worst) and well below the human gap in a
/// click-pause-click (hundreds of ms), so it cannot alias either case into the other.
const CLICK_REPRESS_QUIET_MS: u64 = 120;

/// CLICK-3 witness counters (usbdebug builds only) — presses OBSERVED at parse vs Button events
/// DELIVERED to the event queue, plus how many of those deliveries only the re-press recovery
/// caught (i.e. clicks that were silently lost before this arc). Read via the `:: PTR:` line.
#[cfg(feature = "usbdebug")]
static PTR_PRESS_SEEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "usbdebug")]
static PTR_PRESS_DELIVERED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "usbdebug")]
static PTR_PRESS_RECOVERED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

impl IntEp {
    /// CLICK-3 — the ONE button-transition decision every EHCI pointer path shares (trackpad 0x02,
    /// parsed report-pointer, boot mouse). Stamps the report clock, decides whether this report is a
    /// primary-button PRESS, and updates `prev_buttons`. Returns `true` when the caller owes exactly
    /// one `pal::Event::Button`. Release and hold return `false` — the emit contract is unchanged.
    fn note_buttons(&mut self, buttons: u8, idx: usize) -> bool {
        let now = crate::arch::ms();
        let prev = self.prev_buttons;
        // Silence since the PREVIOUS report on this endpoint (before this one is stamped). `== 0`
        // means "no previous report", which cannot be a re-press.
        let quiet = self.last_report_ms != 0
            && now.wrapping_sub(self.last_report_ms) >= CLICK_REPRESS_QUIET_MS;
        self.last_report_ms = now;
        self.prev_buttons = buttons;
        let down = buttons & 0x01 != 0;
        let edge = down && prev & 0x01 == 0;
        // The recovery arm: still down, was down, and the endpoint was silent across the gap.
        let repress = down && prev & 0x01 != 0 && quiet;
        #[cfg(feature = "usbdebug")]
        {
            use core::sync::atomic::Ordering::Relaxed;
            if down {
                PTR_PRESS_SEEN.fetch_add(1, Relaxed);
            }
            if edge || repress {
                PTR_PRESS_DELIVERED.fetch_add(1, Relaxed);
                if repress {
                    PTR_PRESS_RECOVERED.fetch_add(1, Relaxed);
                }
                serial_println!(
                    ":: PTR: [{}] press seen={} delivered={} recovered={} ({}) == witness ::",
                    idx,
                    PTR_PRESS_SEEN.load(Relaxed),
                    PTR_PRESS_DELIVERED.load(Relaxed),
                    PTR_PRESS_RECOVERED.load(Relaxed),
                    if repress { "re-press after quiet gap" } else { "down edge" },
                );
            }
        }
        #[cfg(not(feature = "usbdebug"))]
        let _ = idx;
        edge || repress
    }
}

/// EHCI-4 M2 — the minimal field map a HID **pointer** report exposes, extracted by
/// `parse_report_descriptor`. NOT a general HID stack: it captures exactly what a mouse / tablet /
/// trackpad report needs to move a cursor — the X and Y axes (Generic Desktop usages 0x30/0x31:
/// bit offset + size + relative-vs-absolute), the button bitfield, an optional Report ID prefix,
/// and (as a witness only) any Digitizer contact-count / finger field. Everything else in the
/// descriptor is skipped.
#[derive(Clone, Copy, Default)]
struct ReportLayout {
    /// 0 => reports carry no Report ID prefix byte; else the ID this layout decodes.
    report_id: u8,
    /// X/Y came from a Relative Input item (a mouse) → signed deltas → `pal::Event::Mouse`;
    /// false = Absolute (a tablet / the Apple trackpad) → `pal::Event::MouseAbsolute`.
    relative: bool,
    has_xy: bool,
    x_off: u16,
    x_size: u8,
    y_off: u16,
    y_size: u8,
    btn_off: u16,
    btn_count: u8,
    /// Digitizer finger/contact-count field (usage 0x0D:0x54), witness-only. 0 size => absent.
    finger_off: u16,
    finger_size: u8,
    /// Total report body bits seen (after any Report ID byte) — a sanity witness.
    total_bits: u16,
    /// EHCI-5: true = the Apple vendor-defined MULTITOUCH interface (Report ID 0x44, usage page
    /// 0xFF00, opaque Input blob, no standard X/Y). Set ONLY after the standard X/Y gate finds
    /// nothing, so the standard pointer path is never perturbed. The descriptor does NOT describe
    /// the finger byte layout — the service loop decodes the first finger at the HYPOTHESIS
    /// offsets (`VMT_FINGER_*`), a metal-verified guess whose values a sitting adjusts.
    vendor_mt: bool,
}

// ── EPACE — the ehci-hid phase accumulator ──────────────────────────────────────────────────────
// The metal BPACE ledger (2026-07-30) read `ehci-hid-done d=6324ms`: 93% of the old 7.3 s boot
// block in ONE bucket, entered and left with nothing stamped in between. This instrument splits
// that bucket. It deliberately does NOT add per-phase `bootpace::record` stamps: the hub walk is
// per-port × per-tier and could overflow the 64-slot ring, whose drop-NEWEST policy would then
// silently destroy every later boot tag — a worse ledger in exchange for a better one.
//
// Design: cycle-count accumulators per phase CLASS, kept on the Controller, printed as one
// summary line per controller right before `:: EHCI-HID: end`. Spans are measured around the
// phase call sites, so a class contains everything its phase did — settles, MMIO polls, control
// transfers AND the serial printing of its own witness lines. Nested classes (hubpwr/hubrst/
// hidcfg) accumulate inside the top-level `enum` span; `resid=` prints `enum` minus its named
// parts, so unattributed time is a visible number, never a silent absence.
//
// Instrument honesty (the can-this-lie-while-looking-right check):
//   * Same clock as the code under measurement — `now_cycles()`/rdtsc, converted at PRINT time
//     via `apic::tsc_hz()`, the exact rate `settle_ms` itself uses. If calibration is wrong the
//     settles and this report are wrong TOGETHER, which keeps the ratios truthful; `hz=0`
//     (pre-calibration) prints raw cycles with a `cy` suffix rather than a fabricated ms.
//   * Self-check against the enclosing instrument: the final line prints `init=` (this module's
//     own entry→exit span), which must match the independent BPACE `ehci-hid-done d=` to within
//     the print cost of the EPACE lines themselves. Disagreement means one of the two is lying.
//   * This instrument can execute in every state it reports on: the accumulators are plain
//     memory writes, and the print site sits on the one unconditional path out of `init`.
const EP_WAKE: usize = 0; // wake_run only: PMCSR D0 + legsup + RS. CONFIGFLAG and the pre-look
                          // settle live in wake_route, which every caller runs inside an
                          // EP_HCRST span — they are charged to `hcrst`, never here.
const EP_HCRST: usize = 1; // quiesce_if_firmware_stale + RS restart + wake_route (CF + pre-look
                           // settle), including the probe-14 full re-init
const EP_SMOKE: usize = 2; // the 5 periodic DMA smoke passes
const EP_ROOTRST: usize = 3; // the pre-scan T_ATTDB debounce (once per controller, ahead of the
                             // CCS gate) + reset_root_port's paced reset attempts
const EP_HSEPROBE: usize = 4; // the probe-14 bare GET_DESCRIPTOR(8) transport probe
const EP_ENUM: usize = 5; // top-level enumerate_at_zero span (contains the three below)
const EP_HUBPWR: usize = 6; // …hub PORT_POWER writes + the pwr2good settle
const EP_HUBRST: usize = 7; // …hub downstream-port reset + completion poll + change acks
const EP_HIDCFG: usize = 8; // …configure_hid: config/report descriptors + boot-proto + arming
const N_EPACE: usize = 9;
const EPACE_TAGS: [&str; N_EPACE] =
    ["wake", "hcrst", "smoke", "rootrst", "hseprobe", "enum", "hubpwr", "hubrst", "hidcfg"];

#[derive(Clone, Copy)]
struct Epace {
    cy: [u64; N_EPACE],
    n: [u32; N_EPACE],
}

impl Epace {
    const fn new() -> Self {
        Epace { cy: [0; N_EPACE], n: [0; N_EPACE] }
    }
    /// Close a span opened at `t0` (a `now_cycles()` reading) into class `class`.
    fn add(&mut self, class: usize, t0: u64) {
        self.cy[class] = self.cy[class]
            .wrapping_add(crate::arch::now_cycles().wrapping_sub(t0));
        self.n[class] = self.n[class].saturating_add(1);
    }
}

/// Cycles → whole ms at print time, `None` when the TSC rate is still unknown (pre-calibration) —
/// the caller then prints raw cycles rather than a fabricated millisecond (the `[vugfps]` lesson).
fn epace_ms(cy: u64) -> Option<u64> {
    let hz = crate::arch::x86_64::apic::tsc_hz();
    if hz == 0 { None } else { Some(cy.saturating_mul(1000) / hz) }
}

fn epace_fmt(cy: u64) -> (u64, &'static str) {
    match epace_ms(cy) {
        Some(ms) => (ms, "ms"),
        None => (cy, "cy"),
    }
}

/// One woken, schedule-bearing EHCI function. All DMA structures live in the static
/// `qh::DMA_POOLS` (kernel image, low physical); every `*_phys` field is the page-table-
/// resolved physical address the controller is actually programmed with (probe-5 discipline —
/// never the heap virt==phys shortcut).
pub struct Controller {
    idx: usize,
    op: u64,
    bus: u8,
    dev: u8,
    func: u8,
    /// Dummy async head (H=1, permanently inactive) + the work QH transfers run on.
    async_head: *mut Qh,
    head_phys: u64,
    async_qh: *mut Qh,
    qh_phys: u64,
    /// The three reusable control-transfer qTDs (SETUP/DATA/STATUS) + their buffers. One
    /// synchronous transfer at a time — enumeration is strictly one-device-at-a-time (the same
    /// invariant the xHCI enum FSM enforces), so reuse is safe by construction.
    qtd_setup: *mut Qtd,
    qtd_setup_phys: u64,
    qtd_data: *mut Qtd,
    qtd_data_phys: u64,
    qtd_status: *mut Qtd,
    qtd_status_phys: u64,
    setup_buf: *mut u8,
    setup_buf_phys: u64,
    data_buf: *mut u8,
    data_buf_phys: u64,
    frame_list: *mut u32,
    frame_list_phys: u64,
    int_next: usize,
    periodic_on: bool,
    /// Probe-14 self-adaptation: false = standard qTD-chain transfers (QEMU's model requires
    /// fetched qTDs); true = overlay-direct (this metal HSEs on the qTD-fetch burst write —
    /// flipped automatically on the first chain HSE, permanent for the controller's lifetime).
    overlay_mode: bool,
    /// N2: driver-owned address allocator — EHCI has no controller slot model. Monotonic;
    /// a failed enumeration BURNS its address (never reused for a possibly-half-addressed
    /// device — mirror of dispose_downstream_slot's honesty). The 7-bit space bounds this at
    /// 127 devices per controller per boot; with 2 root ports + a ≤8-port RMH tier and no
    /// hot-plug rescan in this arc, exhaustion is unreachable in practice and traced if hit.
    next_addr: u8,
    int_eps: Vec<IntEp>,
    /// EPACE phase accumulators (see the module comment above the struct).
    pace: Epace,
    /// MT-INVESTIGATION (IVY, `mtraw` knob only): the trackpad target the raw-mode probe armed,
    /// remembered so the service loop can restore the known-good pointer mode over EP0 once the
    /// capture window closes. `None` until the probe runs, and again after the restore.
    #[cfg(feature = "mtraw")]
    mt_probe: Option<(Target, u8)>,
    /// MT-INVESTIGATION: reports hex-dumped so far in the raw capture window.
    #[cfg(feature = "mtraw")]
    mt_dumped: u32,
}

// Raw pointers to identity-mapped DMA memory; access is serialized by the EHCI_HID mutex.
unsafe impl Send for Controller {}

pub static EHCI_HID: Mutex<Option<Vec<Controller>>> = Mutex::new(None);

/// EPACE-TRIM M1 — the chain-HSE verdict, carried across controllers.
///
/// The first EPACE metal split (s58, 2026-08-01) read `hseprobe=2000ms(n=1)` on BOTH
/// controllers: 4.0 s of the 6.32 s `ehci-hid-done` block was the probe-14 transport probe
/// burning one full `hw_wait_budget()` per controller — on silicon whose answer we already
/// knew, because the qTD-fetch HSE is a property of the 7-series PCH DMA path, not of one
/// EHCI function. Both functions sit on the same die; thirteen metal probes produced no case
/// where one function chain-fetched and the other did not.
///
/// So: the FIRST controller still runs the probe (the probe IS the platform check — QEMU's
/// hcd-ehci requires chain mode and never HSEs, so a hardcoded overlay default would break
/// the only other platform this driver runs on). When it HSEs, this latch is set, and every
/// LATER controller is born in overlay-direct mode: no probe, no wedged-controller HCRESET
/// re-init, no doubled root-port reset. Witnessed with a "verdict carried" line so a capture
/// always distinguishes measured-on-this-controller from inherited.
///
/// Instrument honesty: carrying a verdict is an inference, and the line says so. The
/// falsifying case — a machine whose functions genuinely differ — would show as a controller
/// that fails overlay transfers after inheriting the verdict; its enumeration witnesses
/// (`enumeration aborted`, `EP0 … error`) are already unconditional, so the wrong inheritance
/// cannot fail silently.
static CHAIN_HSE_SEEN: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// EPACE-TRIM M4 ANNOTATION — deliberately not called a tripwire. `wake_route` trims its pre-look
/// settle from 150 ms to 20 ms on PPC=0 silicon (no port-power edge was applied, so none is owed —
/// see the comment there), and every root-port path that reports a port failing to come up appends
/// this so a reader does not have to re-derive which settle was in force. What it is NOT is a
/// discriminator: PPC is a property of the silicon, not of the boot, so on the bench this string
/// is present on every failure line and absent on none. It narrows a diagnosis; it cannot falsify
/// the trim, and the §8c falsifier list does not pretend otherwise.
fn m4_note() -> &'static str {
    if ehci_scout::PWR_SETTLE_TRIMMED.load(core::sync::atomic::Ordering::Relaxed) {
        " [EPACE-TRIM M4 annotation: the pre-look settle was the trimmed 20 ms (PPC=0) — a constant on this silicon, so this is context, not evidence]"
    } else {
        ""
    }
}

/// PCI COMMAND register: Memory Space (bit 1) + Bus Master (bit 2). Read-checked, set only if
/// clear (Peter-approved write-surface extension): without BME the controller can never fetch a
/// single QH, so this is a hard precondition traced honestly rather than assumed.
unsafe fn ensure_bus_master(bus: u8, dev: u8, func: u8, idx: usize) {
    let cmd = read_config_32(bus, dev, func, 0x04);
    if cmd & 0x6 == 0x6 {
        serial_println!(
            ":: EHCI-HID: [{}] PCI COMMAND={:#06x} — memory-space + bus-master already enabled ::",
            idx,
            cmd & 0xFFFF
        );
        return;
    }
    // Preserve the status half (hi16 is RW1C — writing back read 1s would ack them; the dword
    // write only carries the low half's new value, status bits are written as read... so mask
    // the RW1C status half to 0 instead: config writes to STATUS are write-1-to-clear).
    write_config_32(bus, dev, func, 0x04, (cmd & 0x0000_FFFF) | 0x6);
    let after = read_config_32(bus, dev, func, 0x04);
    serial_println!(
        ":: EHCI-HID: [{}] PCI COMMAND {:#06x}->{:#06x} (memory-space + bus-master enabled; declared surface extension) ::",
        idx,
        cmd & 0xFFFF,
        after & 0xFFFF
    );
}

impl Controller {
    /// Program the still-disabled schedules: CTRLDSSEGMENT=0 (32-bit addressing — Panther Point
    /// HCCPARAMS advertises nothing else), the all-Terminate frame list, and the single
    /// self-linked head-of-reclamation control QH. Enables ASE; PSE waits until the first
    /// interrupt endpoint exists (an empty periodic walk buys nothing).
    unsafe fn init_schedules(&mut self) {
        let _ = mmio_write32(self.op + OP_CTRLDSSEGMENT, 0);
        // Static pools are zeroed at link time — set every frame-list entry to Terminate.
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), PTR_TERMINATE);
        }
        let _ = mmio_write32(self.op + OP_PERIODICLISTBASE, self.frame_list_phys as u32);

        // Linux-shaped async ring: inactive dummy HEAD (H=1) -> work QH -> head. The head
        // carries the reclamation bit and NEVER a transfer (probe-7 metal finding: an active
        // self-linked H-QH master-aborts Panther Point's async engine; QEMU tolerated it).
        let head = self.async_head;
        (*head).horiz = (self.qh_phys as u32) | PTR_TYPE_QH;
        (*head).ep_chars = QH_HEAD | QH_DTC | QH_EPS_HIGH | (64 << QH_MPS_SHIFT);
        (*head).ep_caps = QH_MULT1;
        (*head).overlay[0] = PTR_TERMINATE;
        (*head).overlay[1] = PTR_TERMINATE;
        (*head).overlay[2] = QTD_HALTED; // permanently idle (Linux marks its dummy head halted)
        let qh = self.async_qh;
        (*qh).horiz = (self.head_phys as u32) | PTR_TYPE_QH;
        (*qh).ep_chars = QH_DTC | QH_EPS_HIGH | (64 << QH_MPS_SHIFT); // rewritten per target
        (*qh).ep_caps = QH_MULT1;
        (*qh).overlay[0] = PTR_TERMINATE;
        (*qh).overlay[1] = PTR_TERMINATE;
        (*qh).overlay[2] = 0; // inactive token — controller skips until a transfer is primed
        let _ = mmio_write32(self.op + OP_ASYNCLISTADDR, self.head_phys as u32);

        // ASE is NOT set here. Real Intel EHCI parks async traversal on an empty schedule
        // (EHCI 4.8.3 empty-schedule detection: one H-bit QH with an inactive overlay) and
        // never re-walks when a transfer is later primed — the first rMBP probe showed exactly
        // that (SETUP qTD stayed Active, zero error bits, wake clean). QEMU re-walks every
        // frame and masked it. So the async schedule is enabled per transfer, with bounded
        // USBSTS.ASS handshakes — the Linux ehci-hcd idiom.
        // Evidence line: virt AND page-table-resolved phys of the QH the controller will
        // fetch (probe-5: static-pool DMA, low physical), + CTRLDSSEGMENT read-back.
        serial_println!(
            ":: EHCI-HID: [{}] schedules armed (static pool): framelist phys={:#x} async head={:#x} work QH={:#x} CTRLDSSEGMENT={:#x} (dummy-head ring, ASE per-transfer, PSE deferred) ::",
            self.idx,
            self.frame_list_phys,
            self.head_phys,
            self.qh_phys,
            mmio_read32(self.op + OP_CTRLDSSEGMENT).unwrap_or(u32::MAX)
        );
    }

    /// Probe-2 metal finding (2026-07-16): Apple EFI drives the internal keyboard over these
    /// EHCI functions pre-boot and leaves USBCMD.PSE=1 behind — the shared wake's RS=1 then set
    /// the controller fetching firmware's STALE frame list (its memory long reclaimed) →
    /// garbage pointers → USBSTS Host System Error → HCHalted, RS dropped, every transfer
    /// timed out (USBSTS=0x0000f01e/f01f, both functions). This is exactly the pre-approved
    /// HCRESET trigger (Peter item (c): reset only on M1-observed inconsistent state, tracing
    /// the inconsistency). Detect stale schedule enables / a latched HSE, trace them verbatim,
    /// then stop + HCRESET so the controller is programmed from its true default state.
    /// Returns true when a reset was performed (caller must re-start RS and re-route CF —
    /// HCRESET clears both). A clean controller (QEMU; a warm re-init) is left untouched.
    unsafe fn quiesce_if_firmware_stale(&mut self) -> bool {
        let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
        let sts = mmio_read32(self.op + OP_USBSTS).unwrap_or(0);
        let stale = cmd & (CMD_PSE | CMD_ASE) != 0 || sts & STS_HSE != 0;
        if !stale {
            return false;
        }
        serial_println!(
            ":: EHCI-HID: [{}] firmware-stale controller state: USBCMD={:#010x} (PSE={} ASE={}) USBSTS={:#010x} (HSE={}) — pre-approved HCRESET path (traced inconsistency) ::",
            self.idx, cmd,
            (cmd >> 4) & 1, (cmd >> 5) & 1, sts, (sts >> 4) & 1
        );
        // Stop first (RS=0 + schedule enables off), bounded wait for halt (no-op if HSE
        // already halted it), then reset and wait for HCRESET to self-clear.
        let _ = mmio_write32(self.op + OP_USBCMD, cmd & !(CMD_RS | CMD_PSE | CMD_ASE));
        let halted = wait_bounded(|| {
            mmio_read32(self.op + OP_USBSTS).unwrap_or(0) & STS_HCHALTED != 0
        });
        let _ = mmio_write32(self.op + OP_USBCMD, CMD_HCRESET);
        let reset_done = wait_bounded(|| {
            mmio_read32(self.op + OP_USBCMD).unwrap_or(CMD_HCRESET) & CMD_HCRESET == 0
        });
        serial_println!(
            ":: EHCI-HID: [{}] HCRESET: halted={} reset-cleared={} USBCMD={:#010x} USBSTS={:#010x} (defaults; RS + CONFIGFLAG re-applied next) ::",
            self.idx, halted, reset_done,
            mmio_read32(self.op + OP_USBCMD).unwrap_or(0),
            mmio_read32(self.op + OP_USBSTS).unwrap_or(0)
        );
        true
    }

    /// Enable/disable the periodic schedule with a bounded wait for USBSTS.PSS (bit 14) to
    /// agree with USBCMD.PSE (EHCI 4.8 discipline). Returns whether the status bit followed.
    unsafe fn set_periodic_schedule(&mut self, on: bool) -> bool {
        let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
        let want = if on { cmd | CMD_PSE } else { cmd & !CMD_PSE };
        let _ = mmio_write32(self.op + OP_USBCMD, want);
        wait_bounded(|| {
            let sts = mmio_read32(self.op + OP_USBSTS).unwrap_or(0);
            ((sts & STS_PSS) != 0) == on
        })
    }

    /// One synchronous EP0 control transfer through the shared QH (the EHCI analogue of xHCI's
    /// `sync_control`: main-loop context, never inside an interrupt). Returns the transferred
    /// data-stage byte count. Bounded — a wedged Active bit is a traced Err, never a hang.
    unsafe fn control(
        &mut self,
        t: &Target,
        bm_req: u8,
        b_req: u8,
        w_value: u16,
        w_index: u16,
        w_length: u16,
        dir_in: bool,
    ) -> Result<u32, &'static str> {
        // Retarget the shared QH. C-bit (control-endpoint) only for FS/LS targets — that is
        // what makes the controller drive the SSPLIT/CSPLIT control dance through the TT named
        // by hub_addr/hub_port on Topology A; both fields stay 0 on Topology B.
        // PROBE-8 / metal finding: EP0 control transfers run on the PERIODIC engine. The async
        // engine on this Panther Point master-aborts its very first schedule fetch in every
        // configuration tried (heap + static DMA, active-H-QH + Linux-shaped dummy-head ring,
        // post-HCRESET, VT-d off, BME on — probes 1-7), while the periodic engine DMAs the same
        // pool cleanly. EHCI QHs are engine-agnostic (4.10) — a control QH executes identically
        // from the frame list; only the service cadence differs (S-mask-paced instead of
        // continuous). HS targets get S-mask 0xFF (every µframe → ~1 ms per control transfer);
        // FS/LS-behind-TT keep the split masks (SSPLIT µframe 0, CSPLITs 2-4). The async ring
        // stays programmed-but-disabled (ASE is never set).
        // Periodic-QH discipline (probe-14c: RL must be 0 on periodic QHs — the RL=4 async
        // idiom made the QH invisible to the periodic scheduler; S-mask 0x01 is the exact
        // shape every passing smoke used).
        let qh = self.async_qh;
        let mut chars = (t.addr as u32)
            | t.eps
            | QH_DTC
            | ((t.mps0 as u32) << QH_MPS_SHIFT);
        if t.eps != QH_EPS_HIGH {
            chars |= QH_CTL_EP;
        }
        (*qh).ep_chars = chars;
        let masks = if t.eps == QH_EPS_HIGH {
            0x01 << QH_SMASK_SHIFT
        } else {
            (0x01 << QH_SMASK_SHIFT) | (0x1C << QH_CMASK_SHIFT)
        };
        (*qh).ep_caps = QH_MULT1
            | masks
            | ((t.hub_addr as u32) << QH_HUBADDR_SHIFT)
            | ((t.hub_port as u32) << QH_PORT_SHIFT);

        // PROBE-14 / metal finding: OVERLAY-DIRECT transactions. Across 13 metal probes the
        // one operation never seen to succeed — and present in every failure — is the qTD
        // fetch → overlay load, the controller's only multi-dword BURST WRITE (burst reads,
        // dword token write-backs, payload reads, and live-port transactions all passed the
        // smoke battery). So no qTD is ever handed to the controller: software pre-loads the
        // QH overlay with each stage's token/buffer (exactly the shape every passing smoke
        // used) and runs SETUP / DATA / STATUS as three sequential overlay transactions.
        // Setup packet.
        let sb = self.setup_buf;
        sb.write(bm_req);
        sb.add(1).write(b_req);
        sb.add(2).write(w_value as u8);
        sb.add(3).write((w_value >> 8) as u8);
        sb.add(4).write(w_index as u8);
        sb.add(5).write((w_index >> 8) as u8);
        sb.add(6).write(w_length as u8);
        sb.add(7).write((w_length >> 8) as u8);

        // Mode selection: QEMU's hcd-ehci only executes fetched qTDs (it ignores software-
        // primed overlays), while this metal HSEs on the qTD fetch. Chain mode runs first;
        // an HSE'd chain transfer flips the controller to overlay-direct permanently and
        // retries (the failed SETUP never reached the wire — clean retry).
        if !self.overlay_mode {
            // Chain mode; an HSE propagates to the caller, which must FULLY re-init the
            // controller (HCRESET — an HSE'd controller is wedged; RS alone does not
            // recover it, probe-14 finding) and set overlay_mode before retrying.
            return self.chain_txn(t, bm_req, b_req, w_length, dir_in);
        }

        // Three overlay-direct stages. DT: SETUP=0, DATA starts 1 (controller maintains the
        // toggle in the overlay token across packets within a stage), STATUS=1 opposite PID.
        self.overlay_txn(bm_req, b_req, "SETUP", QTD_PID_SETUP, 8, self.setup_buf_phys, t.addr)?;
        let mut got = 0u32;
        if w_length > 0 {
            let data_pid = if dir_in { QTD_PID_IN } else { QTD_PID_OUT };
            got = self.overlay_txn(
                bm_req, b_req, "DATA", data_pid | QTD_DT, w_length as u32, self.data_buf_phys, t.addr,
            )?;
        }
        let status_pid = if w_length == 0 || !dir_in { QTD_PID_IN } else { QTD_PID_OUT };
        self.overlay_txn(bm_req, b_req, "STATUS", status_pid | QTD_DT, 0, 0, t.addr)?;
        Ok(got)
    }

    /// Chain-mode EP0 transfer (the standard qTD-chain shape; QEMU's model requires it). On a
    /// Host System Error the caller switches to overlay-direct. Returns bytes transferred.
    unsafe fn chain_txn(
        &mut self,
        t: &Target,
        bm_req: u8,
        b_req: u8,
        w_length: u16,
        dir_in: bool,
    ) -> Result<u32, &'static str> {
        let qh = self.async_qh;
        let (setup, data, status) = (self.qtd_setup, self.qtd_data, self.qtd_status);
        let status_pid = if w_length == 0 || !dir_in { QTD_PID_IN } else { QTD_PID_OUT };
        write_qtd(status, PTR_TERMINATE, status_pid | QTD_DT | QTD_IOC, 0, 0);
        let first_after_setup = if w_length > 0 {
            let data_pid = if dir_in { QTD_PID_IN } else { QTD_PID_OUT };
            write_qtd(data, self.qtd_status_phys as u32, data_pid | QTD_DT, w_length as u32, self.data_buf_phys);
            self.qtd_data_phys as u32
        } else {
            self.qtd_status_phys as u32
        };
        write_qtd(setup, first_after_setup, QTD_PID_SETUP, 8, self.setup_buf_phys);

        (*qh).overlay[1] = PTR_TERMINATE;
        core::ptr::write_volatile(&mut (*qh).overlay[2], 0);
        core::ptr::write_volatile(&mut (*qh).overlay[0], self.qtd_setup_phys as u32);
        let old_head = core::ptr::read_volatile(self.frame_list);
        (*qh).horiz = old_head;
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), (self.qh_phys as u32) | PTR_TYPE_QH);
        }
        // EPACE-TRIM M2 — the sub-split of the probe's budget burn. The s58 metal split read
        // `hseprobe=2000ms(n=1)`: exactly one full `hw_wait_budget()` consumed somewhere in
        // this function, but WHICH of the three bounded waits burned it — the PSS enable
        // handshake, the completion wait, or the PSS disable on an already-wedged engine — is
        // not decomposable from the outside. Per the ledger's own law, a constant that has
        // not been decomposed must not be trimmed: these three timers aim the trim. Printed
        // only on the failure exits (HSE / timeout), so QEMU's healthy chain path stays quiet.
        let en_t0 = crate::arch::now_cycles();
        let _ = self.set_periodic_schedule(true);
        let en_cy = crate::arch::now_cycles().wrapping_sub(en_t0);
        let wait_t0 = crate::arch::now_cycles();
        let done = wait_bounded(|| {
            let st = core::ptr::read_volatile(&(*status).token);
            if st & QTD_ACTIVE == 0 {
                return true;
            }
            let hse = mmio_read32(self.op + OP_USBSTS).unwrap_or(0) & STS_HSE != 0;
            hse || (core::ptr::read_volatile(&(*setup).token) & QTD_HALTED != 0)
                || (w_length > 0 && core::ptr::read_volatile(&(*data).token) & QTD_HALTED != 0)
        });
        let wait_cy = crate::arch::now_cycles().wrapping_sub(wait_t0);
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), old_head);
        }
        // EPACE-TRIM M3 — the s59 sub-split named the guilty wait: `sched-en=0ms done-wait=0ms
        // sched-dis=2000ms`. The HSE latches instantly and the completion wait exits on it; the
        // full budget was burned waiting for USBSTS.PSS to clear on an engine the HSE has wedged
        // — a handshake that cannot complete, ahead of a caller that responds to Err("hse") with
        // a full HCRESET (which clears PSE/PSS at defaults anyway). On the HSE path: write PSE
        // off (tidy, harmless) and skip the PSS wait. The healthy path (QEMU, and any silicon
        // that chain-fetches) keeps the full EHCI 4.8 handshake unchanged.
        let hse_latched = mmio_read32(self.op + OP_USBSTS).unwrap_or(0) & STS_HSE != 0;
        let dis_t0 = crate::arch::now_cycles();
        if !self.periodic_on {
            if hse_latched {
                let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
                let _ = mmio_write32(self.op + OP_USBCMD, cmd & !CMD_PSE);
            } else {
                let _ = self.set_periodic_schedule(false);
            }
        }
        let dis_cy = crate::arch::now_cycles().wrapping_sub(dis_t0);
        (*qh).horiz = PTR_TERMINATE;
        core::ptr::write_volatile(&mut (*qh).overlay[0], PTR_TERMINATE);

        let sts = mmio_read32(self.op + OP_USBSTS).unwrap_or(0);
        if sts & STS_HSE != 0 {
            let (ev, eu) = epace_fmt(en_cy);
            let (wv, wu) = epace_fmt(wait_cy);
            let (dv, du) = epace_fmt(dis_cy);
            serial_println!(
                ":: EHCI-HID: [{}] chain HSE sub-split: sched-en={}{} done-wait={}{} sched-dis={}{} == witness ::",
                self.idx, ev, eu, wv, wu, dv, du
            );
            // Leave the HSE LATCHED: the caller's quiesce path keys its full HCRESET off it
            // (probe-14b: acking here left the controller wedged and the re-reset PR stuck).
            return Err("hse");
        }
        if !done {
            let (ev, eu) = epace_fmt(en_cy);
            let (wv, wu) = epace_fmt(wait_cy);
            let (dv, du) = epace_fmt(dis_cy);
            serial_println!(
                ":: EHCI-HID: [{}] chain timeout sub-split: sched-en={}{} done-wait={}{} sched-dis={}{} == witness ::",
                self.idx, ev, eu, wv, wu, dv, du
            );
            serial_println!(
                ":: EHCI-HID: [{}] STOP-NOTE EP0 chain timeout addr={} req={:#04x}/{:#04x} setup-token={:#010x} — not forced ::",
                self.idx, t.addr, bm_req, b_req,
                core::ptr::read_volatile(&(*setup).token)
            );
            return Err("timeout");
        }
        for (name, q) in [("SETUP", setup), ("DATA", data), ("STATUS", status)] {
            if name == "DATA" && w_length == 0 {
                continue;
            }
            let tok = core::ptr::read_volatile(&(*q).token);
            if tok & QTD_ERR_MASK != 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] EP0 chain {} error addr={} req={:#04x}/{:#04x} token={:#010x} (halted/xact — likely STALL) ::",
                    self.idx, name, t.addr, bm_req, b_req, tok
                );
                return Err("stall");
            }
        }
        let residual = if w_length > 0 {
            (core::ptr::read_volatile(&(*data).token) >> QTD_TOTAL_SHIFT) & 0x7FFF
        } else {
            0
        };
        Ok((w_length as u32).saturating_sub(residual))
    }

    /// One overlay-direct transaction stage on the (already retargeted) work QH: software
    /// writes the token/buffer straight into the overlay — the controller never fetches a qTD
    /// (the burst-write class 13 metal probes indicted). Splices the QH into the frame list,
    /// runs the periodic engine, bounded-waits on the token, unsplices. Returns bytes done.
    unsafe fn overlay_txn(
        &mut self,
        bm_req: u8,
        b_req: u8,
        stage: &str,
        pid_dt: u32,
        len: u32,
        buf_phys: u64,
        addr: u8,
    ) -> Result<u32, &'static str> {
        let qh = self.async_qh;
        (*qh).current_qtd = 0;
        (*qh).overlay[0] = PTR_TERMINATE;
        (*qh).overlay[1] = PTR_TERMINATE;
        (*qh).overlay[3] = buf_phys as u32;
        (*qh).overlay[4] = 0;
        core::ptr::write_volatile(
            &mut (*qh).overlay[2],
            QTD_ACTIVE | QTD_CERR3 | (len << QTD_TOTAL_SHIFT) | pid_dt | QTD_IOC,
        );
        // Probe-14e: overlay-direct rides the ASYNC engine — the periodic engine skips
        // SETUP-PID overlays on this silicon (14d), and async only ever died at the qTD
        // fetch, which overlay-direct never performs. The work QH is already ring-linked
        // behind the dummy head; ASE is toggled per stage (bounded ASS handshakes).
        (*qh).horiz = (self.head_phys as u32) | PTR_TYPE_QH;
        let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
        let _ = mmio_write32(self.op + OP_USBCMD, cmd | CMD_ASE);
        // USBSTS bit 15 is Async Schedule Status (EHCI 1.0 §2.3.2) — the correct bit to
        // handshake an ASE toggle. It was previously read into a local named `pss_*` and
        // printed as `PSS on=/off=`, i.e. a field labelled PERIODIC reporting the ASYNC
        // schedule, in the one path a reader only ever reaches while something is already
        // wrong. Bit 14 is PSS; see `STS_PSS`.
        let ass_on = wait_bounded(|| {
            mmio_read32(self.op + OP_USBSTS).unwrap_or(0) & (1 << 15) != 0
        });
        let done = wait_bounded(|| {
            core::ptr::read_volatile(&(*qh).overlay[2]) & QTD_ACTIVE == 0
        });
        let cmd2 = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
        let _ = mmio_write32(self.op + OP_USBCMD, cmd2 & !CMD_ASE);
        let ass_off = wait_bounded(|| {
            mmio_read32(self.op + OP_USBSTS).unwrap_or(0) & (1 << 15) == 0
        });
        let tok = core::ptr::read_volatile(&(*qh).overlay[2]);

        if !done {
            serial_println!(
                ":: EHCI-HID: [{}] STOP-NOTE EP0 {} timeout addr={} req={:#04x}/{:#04x} token={:#010x} USBCMD={:#010x} USBSTS={:#010x} ASS on={} off={} — not forced ::",
                self.idx, stage, addr, bm_req, b_req, tok,
                mmio_read32(self.op + OP_USBCMD).unwrap_or(0),
                mmio_read32(self.op + OP_USBSTS).unwrap_or(0),
                ass_on, ass_off
            );
            return Err("timeout");
        }
        if tok & QTD_ERR_MASK != 0 {
            serial_println!(
                ":: EHCI-HID: [{}] EP0 {} error addr={} req={:#04x}/{:#04x} token={:#010x} (halted/xact — likely STALL) ::",
                self.idx, stage, addr, bm_req, b_req, tok
            );
            return Err("stall");
        }
        Ok(len.saturating_sub((tok >> QTD_TOTAL_SHIFT) & 0x7FFF))
    }

    /// Debounce + reset + enable one root port. Returns true when the port enabled on EHCI
    /// (PED=1 ⇒ a high-speed-capable device trained). PED=0 after a clean reset is the
    /// no-companion release case — paced retries (the xHCI metal lesson), then an honest STOP.
    ///
    /// `debounce` pays the USB 2.0 §7.1.7.3 T_ATTDB connect debounce here. It is `false` for the
    /// main port walk, which pays it once ahead of the CCS gate that decides whether this function
    /// is called at all (see the M4 follow-up there) — the debt is paid exactly once, earlier. It
    /// is `true` for the probe-14 re-init path, which re-routes CONFIGFLAG and comes straight back
    /// to a known-connected port without passing the gate: that caller owns its own debounce.
    unsafe fn reset_root_port(&mut self, port: u32, debounce: bool) -> bool {
        let addr = self.op + OP_PORTSC0 + 4 * port as u64;
        if debounce {
            settle_ms(100); // USB 2.0 TATTDB connect debounce (xHCI metal lesson, transport-free)
        }
        for (attempt, pace) in [(1u32, 0u64), (2, 200), (3, 400), (4, 600)] {
            if pace != 0 {
                settle_ms(pace);
            }
            let before = mmio_read32(addr).unwrap_or(0);
            if before & PORT_CCS == 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] port {} connect dropped during reset sequence (PORTSC={:#010x}){} ::",
                    self.idx, port, before, m4_note()
                );
                return false;
            }
            // Assert reset: PR=1 with PED cleared, RW1C change bits masked. Hold >= 50 ms.
            let _ = mmio_write32(addr, (before & !PORT_RW1C & !PORT_PED) | PORT_PR);
            settle_ms(50);
            let held = mmio_read32(addr).unwrap_or(0);
            let _ = mmio_write32(addr, held & !PORT_RW1C & !PORT_PR);
            let cleared = wait_bounded(|| mmio_read32(addr).unwrap_or(PORT_PR) & PORT_PR == 0);
            settle_ms(10); // post-reset recovery before trusting PED
            let after = mmio_read32(addr).unwrap_or(0);
            serial_println!(
                ":: EHCI-HID: [{}] port {} reset attempt {}: PORTSC {:#010x} -> {:#010x} (PR-cleared={} PED={} owner={}) ::",
                self.idx, port, attempt, before, after, cleared,
                (after >> 2) & 1,
                if after & PORT_OWNER != 0 { "companion" } else { "EHCI" }
            );
            if cleared && after & PORT_PED != 0 {
                return true;
            }
            if after & PORT_OWNER != 0 {
                break; // released toward a companion that does not exist — no retry will help
            }
        }
        serial_println!(
            ":: EHCI-HID: [{}] STOP-NOTE port {} did not enable on EHCI after paced retries — FS/LS-on-root-port release case (no companion on this silicon); reported, not forced{} ::",
            self.idx, port, m4_note()
        );
        false
    }

    /// Probe-13: the pass-3 smoke shape (pre-loaded 0-length IN to a bogus address — pure
    /// pipeline + write-back, no real listener) re-run while a port is ENABLED. Every
    /// pre-enable smoke passed and every live-port transfer HSE'd; this isolates "DMA while a
    /// port is active" as the failing ingredient. Returns whether HSE fired (and restores
    /// running state if it did).
    unsafe fn live_port_smoke(&mut self, tag: &str) -> bool {
        let qh = self.async_qh;
        (*qh).horiz = PTR_TERMINATE;
        (*qh).ep_chars = 42 | (1 << 8) | QH_DTC | QH_EPS_HIGH | (8 << QH_MPS_SHIFT);
        (*qh).ep_caps = QH_MULT1 | 0x01;
        (*qh).overlay[0] = PTR_TERMINATE;
        (*qh).overlay[1] = PTR_TERMINATE;
        core::ptr::write_volatile(&mut (*qh).overlay[2], QTD_ACTIVE | (1 << 10) | QTD_PID_IN);
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), (self.qh_phys as u32) | PTR_TYPE_QH);
        }
        let _ = self.set_periodic_schedule(true);
        settle_ms(5);
        let sts = mmio_read32(self.op + OP_USBSTS).unwrap_or(0);
        let _ = self.set_periodic_schedule(false);
        for i in 0..1024 {
            core::ptr::write_volatile(self.frame_list.add(i), PTR_TERMINATE);
        }
        let tok = core::ptr::read_volatile(&(*qh).overlay[2]);
        (*qh).overlay[0] = PTR_TERMINATE;
        serial_println!(
            ":: EHCI-HID: [{}] live-port smoke ({}): USBSTS={:#010x} HSE={} HCHalted={} post-token={:#010x} == witness ::",
            self.idx, tag, sts, (sts >> 4) & 1, (sts >> 12) & 1, tok
        );
        let hse = sts & STS_HSE != 0;
        if hse {
            let _ = mmio_write32(self.op + OP_USBSTS, sts & STS_RW1C);
            let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
            let _ = mmio_write32(self.op + OP_USBCMD, (cmd & !CMD_PSE) | CMD_RS);
            let _ = wait_bounded(|| {
                mmio_read32(self.op + OP_USBSTS).unwrap_or(STS_HCHALTED) & STS_HCHALTED == 0
            });
        }
        hse
    }

    /// N2 address allocation. The failure paths after this call BURN the address by
    /// construction: `next_addr` is monotonic and nothing ever hands an address back.
    fn alloc_addr(&mut self) -> Option<u8> {
        if self.next_addr > 127 {
            serial_println!(
                ":: EHCI-HID: [{}] STOP-NOTE 7-bit address space exhausted (127 enumerations this boot) ::",
                self.idx
            );
            return None;
        }
        let a = self.next_addr;
        self.next_addr += 1;
        Some(a)
    }

    /// Enumerate the device currently answering address 0 (one at a time, strictly). `depth`
    /// bounds the hub recursion at the RMH tier (root=0; children of the RMH=1; deeper hubs are
    /// out of this arc's scope and traced as skipped).
    unsafe fn enumerate_at_zero(&mut self, eps: u32, hub_addr: u8, hub_port: u8, depth: u8) {
        let mut t = Target {
            addr: 0,
            mps0: if eps == QH_EPS_HIGH { 64 } else { 8 },
            eps,
            hub_addr,
            hub_port,
        };

        // 8-byte device-descriptor header first: learns the real bMaxPacketSize0 before any
        // longer read (the XENUM-3 short-read trap — a FS MPS0 is 8/16/32, never the 64 guess).
        if self.control(&t, 0x80, 6, 0x0100, 0, 8, true).is_err() {
            serial_println!(
                ":: EHCI-HID: [{}] enumeration aborted: device at addr 0 (via hub {} port {}) never answered GET_DESCRIPTOR(8) ::",
                self.idx, hub_addr, hub_port
            );
            return;
        }
        let mps0 = *self.data_buf.add(7) as u16;
        if [8u16, 16, 32, 64].contains(&mps0) {
            t.mps0 = mps0;
        }

        let Some(addr) = self.alloc_addr() else { return };
        if self.control(&t, 0x00, 5, addr as u16, 0, 0, false).is_err() {
            serial_println!(
                ":: EHCI-HID: [{}] address {} BURNED (SET_ADDRESS failed; possibly half-addressed device — address never reused, state cleared) ::",
                self.idx, addr
            );
            return;
        }
        settle_ms(2); // SET_ADDRESS recovery interval (USB 9.2.6.3)
        t.addr = addr;

        // Full device descriptor: the M1 branch-decider evidence.
        let Ok(n) = self.control(&t, 0x80, 6, 0x0100, 0, 18, true) else {
            serial_println!(
                ":: EHCI-HID: [{}] address {} BURNED (full device descriptor unreadable post-address) ::",
                self.idx, addr
            );
            return;
        };
        if n < 18 {
            serial_println!(
                ":: EHCI-HID: [{}] address {} BURNED (short device descriptor: {} bytes) ::",
                self.idx, addr, n
            );
            return;
        }
        let d = core::slice::from_raw_parts(self.data_buf, 18);
        let class = d[4];
        let vid = (d[8] as u16) | ((d[9] as u16) << 8);
        let pid = (d[10] as u16) | ((d[11] as u16) << 8);
        let speed = match t.eps {
            QH_EPS_HIGH => "HS",
            QH_EPS_LOW => "LS",
            _ => "FS",
        };
        // The M1 witness. At depth 0 this line IS the topology fork decision (design §2.4).
        if depth == 0 {
            serial_println!(
                ":: EHCI-HID: [{}] M1 root device addr={} {:04x}:{:04x} class={:#04x} speed={} -> TOPOLOGY {} == witness ::",
                self.idx, addr, vid, pid, class, speed,
                if class == 0x09 { "A (hub tier / RMH)" } else { "B (direct device)" }
            );
        } else {
            serial_println!(
                ":: EHCI-HID: [{}] M1 hub-downstream device addr={} {:04x}:{:04x} class={:#04x} speed={} (hub {} port {}) == witness ::",
                self.idx, addr, vid, pid, class, speed, hub_addr, hub_port
            );
        }

        if class == 0x09 {
            // Metal (probe-14e): the internal keyboard/trackpad sit behind an SMSC 0424:2512
            // hub which itself hangs off the RMH — depth 2 is the real internal topology.
            if depth >= 2 {
                serial_println!(
                    ":: EHCI-HID: [{}] hub at depth {} (addr {}) — beyond the internal tier; skipped ::",
                    self.idx, depth, addr
                );
                return;
            }
            self.bring_up_hub(&t, depth);
        } else {
            let hidcfg_t0 = crate::arch::now_cycles();
            self.configure_hid(&t);
            self.pace.add(EP_HIDCFG, hidcfg_t0);
        }
    }

    /// Topology A: enumerate the hub (the RMH on metal), walk its downstream ports, and
    /// enumerate each connected child through the hub's TT.
    unsafe fn bring_up_hub(&mut self, hub: &Target, depth: u8) {
        if self.control(hub, 0x00, 9, 1, 0, 0, false).is_err() {
            serial_println!(":: EHCI-HID: [{}] hub addr {} SET_CONFIGURATION failed ::", self.idx, hub.addr);
            return;
        }
        // USB2 hub descriptor (class 0xA0, type 0x29): bNbrPorts at [2], bPwrOn2PwrGood at [5].
        let Ok(n) = self.control(hub, 0xA0, 6, 0x2900, 0, 9, true) else {
            serial_println!(":: EHCI-HID: [{}] hub addr {} hub-descriptor read failed ::", self.idx, hub.addr);
            return;
        };
        if n < 7 {
            serial_println!(":: EHCI-HID: [{}] hub addr {} short hub descriptor ({} bytes) ::", self.idx, hub.addr, n);
            return;
        }
        let nbr_ports = (*self.data_buf.add(2)).min(15);
        let pwr2good_ms = (*self.data_buf.add(5) as u64) * 2;
        serial_println!(
            ":: EHCI-HID: [{}] hub addr {}: {} downstream ports (pwr-on 2 good {} ms) — walking ::",
            self.idx, hub.addr, nbr_ports, pwr2good_ms
        );

        // Power every port first (SET_PORT_FEATURE PORT_POWER=8; harmless where always-on).
        let hubpwr_t0 = crate::arch::now_cycles();
        for port in 1..=nbr_ports as u16 {
            let _ = self.control(hub, 0x23, 3, 8, port, 0, false);
        }
        settle_ms(pwr2good_ms + 100);
        self.pace.add(EP_HUBPWR, hubpwr_t0);

        for port in 1..=nbr_ports as u16 {
            // GET_PORT_STATUS: wPortStatus (lo16) + wPortChange (hi16).
            let Ok(4) = self.control(hub, 0xA3, 0, 0, port, 4, true) else { continue };
            let st = (*self.data_buf as u32)
                | ((*self.data_buf.add(1) as u32) << 8)
                | ((*self.data_buf.add(2) as u32) << 16)
                | ((*self.data_buf.add(3) as u32) << 24);
            if st & 1 == 0 {
                continue; // no connection
            }
            // Reset the downstream port (SET_PORT_FEATURE PORT_RESET=4), bounded completion.
            let hubrst_t0 = crate::arch::now_cycles();
            if self.control(hub, 0x23, 3, 4, port, 0, false).is_err() {
                self.pace.add(EP_HUBRST, hubrst_t0);
                continue;
            }
            // EPACE-TRIM M5 (GR18) — this was a blind `settle_ms(50)` in front of a poll that
            // already existed. The blind constant was measurably never the bound: EPACE reads
            // `hubrst=50ms(n=1)` on controller 0 and `hubrst=250ms(n=5)` on controller 1 — 50.0 ms
            // per port, to the millisecond, on every boot of rmbp-gr16-s73 (ttyUSB0.log L8302-8303
            // and the same pair in boots 2/3/4/5/6/8). Exactly 50 ms per port means the poll's
            // FIRST probe always found PORT_RESET already clear, i.e. the loop below has never
            // once iterated and the true reset time is somewhere under 50 ms, unmeasured. A
            // constant that hides the number it is standing in for is exactly what the poll is
            // for. So: start at the USB 2.0 §11.5.1.5 T_DRST floor (10 ms — the minimum a hub may
            // drive reset for) and let the existing bounded poll measure the remainder. The budget
            // (~600 ms) and its loud exit are unchanged. The cost is 10 + 10·poll_steps ms against
            // the old flat 50: cheaper for any port that clears inside 40 ms, equal at 40-50, and
            // dearer only past 50 — which is exactly where the `>= 4` threshold below starts
            // printing, so the band where this trim stops paying can never be silent.
            settle_ms(10);
            // Bounded reset-completion poll (explicit loop: each probe is itself a control
            // transfer, so the generic wait_bounded closure can't drive it). ~600 ms worst case.
            let mut status = 0u32;
            let mut ok = false;
            let mut poll_steps = 0u32;
            for step in 0..60u32 {
                if let Ok(4) = self.control(hub, 0xA3, 0, 0, port, 4, true) {
                    status = (*self.data_buf as u32)
                        | ((*self.data_buf.add(1) as u32) << 8)
                        | ((*self.data_buf.add(2) as u32) << 16)
                        | ((*self.data_buf.add(3) as u32) << 24);
                    if status & (1 << 4) == 0 {
                        ok = true; // PORT_RESET no longer asserted
                        break;
                    }
                }
                settle_ms(10);
                poll_steps = step + 1;
            }
            // T_RSTRCY (USB 2.0 §7.1.7.5) is NOT paid here: the `settle_ms(10)` at the bottom of
            // this loop body, immediately before `enumerate_at_zero`, already is it and predates
            // M5 (only hub-addressed ClearPortFeature traffic sits between the two points). An
            // extra one here would have been a second recovery interval, not a restored one.
            //
            // The M5 report. Three cases, and the timeout case must not be read as a measurement:
            // `poll_steps` counts sleeps, so on a budget exhaustion it says 60 whether the bit
            // never cleared or GET_PORT_STATUS itself was failing. Only the `ok` branch is
            // allowed to talk about reset timing.
            if !ok {
                serial_println!(
                    ":: EHCI-HID: [{}] EPACE-TRIM M5 TRIPWIRE — hub {} port {} PORT_RESET did not clear inside the ~600 ms poll budget (status {:#010x}); this is a timeout, NOT a reset-time measurement — the poll may also have been failing to read == witness ::",
                    self.idx, hub.addr, port, status
                );
            } else if poll_steps >= 4 {
                // >= 4, not > 4: at 4 steps the poll has already reached the 50 ms the trim
                // replaced, so the boundary band is loud rather than silent.
                serial_println!(
                    ":: EHCI-HID: [{}] EPACE-TRIM M5 TRIPWIRE — hub {} port {} took ~{} ms to clear PORT_RESET, at or past the 50 ms constant M5 replaced == witness ::",
                    self.idx, hub.addr, port, 10 + poll_steps * 10
                );
            } else if poll_steps > 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] hub {} port {} PORT_RESET cleared after ~{} ms (T_DRST floor 10 ms + {} poll step(s)) ::",
                    self.idx, hub.addr, port, 10 + poll_steps * 10, poll_steps
                );
            }
            // Ack the change bits we may have latched (C_PORT_CONNECTION=16, C_PORT_RESET=20).
            let _ = self.control(hub, 0x23, 1, 16, port, 0, false);
            let _ = self.control(hub, 0x23, 1, 20, port, 0, false);
            self.pace.add(EP_HUBRST, hubrst_t0);
            if !ok || status & (1 << 1) == 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] hub {} port {} did not enable after reset (status {:#010x}) — skipped ::",
                    self.idx, hub.addr, port, status
                );
                continue;
            }
            // wPortStatus bit9 = low speed, bit10 = high speed, neither = full speed.
            let child_eps = if status & (1 << 9) != 0 {
                QH_EPS_LOW
            } else if status & (1 << 10) != 0 {
                QH_EPS_HIGH
            } else {
                QH_EPS_FULL
            };
            // A FS/LS child is reached via THIS hub's TT: hub_addr = the hub, port = this
            // port. A HS child needs no TT (fields stay zero).
            let (ha, hp) = if child_eps == QH_EPS_HIGH { (0, 0) } else { (hub.addr, port as u8) };
            // T_RSTRCY (USB 2.0 §7.1.7.5): the 10 ms of reset recovery owed before the device is
            // addressed. Correctly placed — the only traffic between the reset completing and
            // here is hub-addressed ClearPortFeature. EPACE-TRIM M5 shortened the pre-poll sleep
            // above; this interval is what keeps the recovery paid, and it predates M5.
            settle_ms(10);
            self.enumerate_at_zero(child_eps, ha, hp, depth + 1);
        }
    }

    /// Topology B leaf (or an RMH child): read the config, boot-protocol every boot-capable HID
    /// interface, and arm one periodic interrupt QH per HID endpoint.
    unsafe fn configure_hid(&mut self, t: &Target) {
        let Ok(n) = self.control(t, 0x80, 6, 0x0200, 0, 9, true) else {
            serial_println!(":: EHCI-HID: [{}] addr {} config-descriptor header read failed ::", self.idx, t.addr);
            return;
        };
        if n < 9 {
            return;
        }
        let total =
            (((*self.data_buf.add(2) as u16) | ((*self.data_buf.add(3) as u16) << 8)).min(64)) as u16;
        let config_value = *self.data_buf.add(5);
        if self.control(t, 0x80, 6, 0x0200, 0, total, true).is_err() {
            return;
        }

        // Walk interface/endpoint descriptors — the same walk as xHCI's parse_hid_config, but
        // collecting EVERY HID interrupt-IN endpoint: the Apple internal keyboard+trackpad are
        // one composite device with multiple HID interfaces.
        let cfg = core::slice::from_raw_parts(self.data_buf, total as usize);
        // (proto, ep, mps, interval, intf, report_desc_len) — report_desc_len from the interface's
        // HID class descriptor (0x21), needed to GET_DESCRIPTOR(Report) on the non-boot path (M2).
        let mut found: [Option<(u8, u8, u16, u8, u8, u16)>; 4] = [None; 4];
        let mut nfound = 0;
        let (mut off, mut in_hid, mut proto, mut intf) = (0usize, false, 0u8, 0u8);
        let mut report_len = 0u16; // pending HID report-descriptor length for the current interface
        while off + 2 <= cfg.len() {
            let len = cfg[off] as usize;
            if len == 0 {
                break;
            }
            match cfg[off + 1] {
                0x04 if off + 8 <= cfg.len() => {
                    intf = cfg[off + 2];
                    in_hid = cfg[off + 5] == 0x03;
                    proto = cfg[off + 7];
                    report_len = 0;
                }
                // HID class descriptor (0x21): its first subordinate descriptor is the Report
                // descriptor (type 0x22); wDescriptorLength (bytes 7..8) is what we read on the M2
                // non-boot path. Guard the type byte so a vendor layout can't mis-seed the length.
                0x21 if in_hid && off + 9 <= cfg.len() && cfg[off + 6] == 0x22 => {
                    report_len = (cfg[off + 7] as u16) | ((cfg[off + 8] as u16) << 8);
                }
                0x05 if in_hid && off + 7 <= cfg.len() => {
                    let ep = cfg[off + 2];
                    if ep & 0x80 != 0 && cfg[off + 3] & 0x3 == 3 && nfound < 4 {
                        let mps = ((cfg[off + 4] as u16) | ((cfg[off + 5] as u16) << 8)) & 0x7FF;
                        found[nfound] = Some((proto, ep & 0xF, mps, cfg[off + 6], intf, report_len));
                        nfound += 1;
                    }
                }
                _ => {}
            }
            off += len;
        }
        if nfound == 0 {
            serial_println!(
                ":: EHCI-HID: [{}] addr {} has no HID interrupt-IN endpoint — nothing to arm ::",
                self.idx, t.addr
            );
            return;
        }
        if self.control(t, 0x00, 9, config_value as u16, 0, 0, false).is_err() {
            serial_println!(":: EHCI-HID: [{}] addr {} SET_CONFIGURATION failed ::", self.idx, t.addr);
            return;
        }

        for slot in found.iter().flatten() {
            let (proto, ep, mps, interval, intf, report_len) = *slot;
            // Boot interfaces (proto 1 = keyboard, 2 = mouse) take SET_PROTOCOL(boot) and decode
            // through the fixed boot-report layout. A non-boot interface (proto 0 — the Apple
            // trackpad, and QEMU's usb-tablet) is a REPORT-protocol pointer: read + parse its HID
            // report descriptor (M2) and decode X/Y/buttons from the parsed field map instead.
            if proto != 1 && proto != 2 {
                self.configure_report_pointer(t, ep, mps, interval, intf, report_len);
                continue;
            }
            // SET_PROTOCOL(boot): bmRequestType 0x21, bRequest 0x0B, wValue 0 (=Boot), wIndex =
            // interface — the exact request the xHCI path sends (set_hid_boot_protocol).
            if self.control(t, 0x21, 0x0B, 0, intf as u16, 0, false).is_err() {
                serial_println!(
                    ":: EHCI-HID: [{}] addr {} intf {} SET_PROTOCOL(boot) refused — skipped (R3) ::",
                    self.idx, t.addr, intf
                );
                continue;
            }
            self.arm_interrupt_ep(t, ep, mps.min(64), proto == 1, proto == 2, None);
            serial_println!(
                ":: EHCI-HID: [{}] M2 armed {} addr={} ep=IN{} mps={} interval={} (boot protocol) == witness ::",
                self.idx,
                if proto == 1 { "keyboard" } else { "boot-mouse" },
                t.addr, ep, mps, interval
            );
            // GUI-WITNESS: an internal-HID interrupt endpoint is armed. On the rMBP this is the
            // keyboard/trackpad-input path; a silent boot otherwise can't tell on-panel whether input
            // ever came up.
            crate::bootlog::record(if proto == 1 { "ehci:kbd-armed" } else { "ehci:mouse-armed" });
        }
    }

    /// M2 — the trackpad (report-protocol pointer) path. A non-boot HID interface exposes its
    /// report format only through its HID **report descriptor**, so: GET_DESCRIPTOR(Report),
    /// parse it for the X/Y/buttons field map (`parse_report_descriptor`), leave the interface in
    /// its native **report** protocol (no SET_PROTOCOL(boot) — that is the boot-only request), and
    /// arm the interrupt-IN QH with the parsed layout. The verbatim descriptor bytes are dumped on
    /// serial (the doc's 0262 capture slot; QEMU's usb-tablet stands in for the mechanics). If no
    /// X/Y variable field is found the endpoint is skipped with an honest trace, never mis-armed.
    unsafe fn configure_report_pointer(
        &mut self,
        t: &Target,
        ep: u8,
        mps: u16,
        interval: u8,
        intf: u8,
        report_len: u16,
    ) {
        if report_len == 0 {
            serial_println!(
                ":: EHCI-HID: [{}] addr {} intf {} non-boot HID but no report-descriptor length in the HID descriptor — skipped ::",
                self.idx, t.addr, intf
            );
            return;
        }
        // GET_DESCRIPTOR(Report): bmRequestType 0x81 (in | standard | INTERFACE recipient),
        // bRequest 6, wValue 0x2200 (type 0x22 Report, index 0), wIndex = interface. Bounded by the
        // 256-byte control buffer (Buf256) — a longer descriptor is read short and traced.
        let want = report_len.min(256);
        let Ok(got) = self.control(t, 0x81, 6, 0x2200, intf as u16, want, true) else {
            serial_println!(
                ":: EHCI-HID: [{}] addr {} intf {} GET_DESCRIPTOR(Report) failed — skipped ::",
                self.idx, t.addr, intf
            );
            return;
        };
        let n = (got as usize).min(want as usize).min(256);
        let desc = core::slice::from_raw_parts(self.data_buf, n);
        // Verbatim capture for the doc (the exact 0262 report descriptor is metal-first). Bounded
        // dump: the leading bytes, hex, on one line — enough to reconstruct the field map.
        dump_report_descriptor(self.idx, t.addr, intf, report_len, desc);
        let Some(layout) = parse_report_descriptor(desc) else {
            serial_println!(
                ":: EHCI-HID: [{}] addr {} intf {} report descriptor has no X/Y pointer field (parsed {} of {} B) — not a cursor device; skipped ::",
                self.idx, t.addr, intf, n, report_len
            );
            return;
        };
        // EHCI-TRACKPAD M1: the Apple vendor-multitouch interface stays silent until the bcm5974
        // "Wellspring" mode switch. Fire it BEFORE arming so the first polls catch the stream. The
        // switch is gated on `vendor_mt` (Report ID 0x44 + vendor page 0xFF00) — QEMU's usb-tablet
        // is a standard absolute pointer, never `vendor_mt`, so QEMU never takes this path. A
        // STALL/timeout on any stage is non-fatal (traced, then arm regardless).
        //
        // MT-INVESTIGATION (IVY): the `mtraw` knob swaps this ONE call for the raw-mode probe. The
        // swap lives here rather than inside `bcm5974_mode_switch` so that knob-off the switch
        // function keeps its exact name and body — default media stay byte-identical, symbols
        // included, which a build-hash comparison of both trees confirmed.
        if layout.vendor_mt {
            #[cfg(not(feature = "mtraw"))]
            self.bcm5974_mode_switch(t, intf);
            #[cfg(feature = "mtraw")]
            self.bcm5974_mt_raw_probe(t, intf);
        }
        self.arm_interrupt_ep(t, ep, mps.min(64), false, false, Some(layout));
        // GUI-WITNESS: the report-protocol pointer (the rMBP trackpad, incl. the Apple
        // vendor-multitouch interface) is armed — the trackpad-input milestone.
        crate::bootlog::record("ehci:trackpad-armed");
        if layout.vendor_mt {
            // EHCI-5 M1: the Apple vendor-multitouch interface (Report ID 0x44, page 0xFF00). The
            // descriptor does not describe the finger layout — arm to CAPTURE the raw body and
            // decode the first finger at the HYPOTHESIS offsets (confirmed/corrected at the sitting).
            serial_println!(
                ":: EHCI-HID: [{}] M1 armed vendor-multitouch addr={} ep=IN{} mps={} interval={} id={:#04x} body={}b (capture; hypothesis X@{} Y@{} le16, touch@{}) == witness ::",
                self.idx, t.addr, ep, mps, interval, layout.report_id, layout.total_bits,
                VMT_FINGER_ABS_X, VMT_FINGER_ABS_Y, VMT_FINGER_TOUCH,
            );
        } else {
            serial_println!(
                ":: EHCI-HID: [{}] M2 armed report-pointer addr={} ep=IN{} mps={} interval={} ({}; X@{}/{}b Y@{}/{}b btn@{}x{} id={} body={}b{}) == witness ::",
                self.idx, t.addr, ep, mps, interval,
                if layout.relative { "relative" } else { "absolute" },
                layout.x_off, layout.x_size, layout.y_off, layout.y_size,
                layout.btn_off, layout.btn_count, layout.report_id, layout.total_bits,
                if layout.finger_size != 0 { ", multitouch" } else { "" },
            );
        }
    }

    /// EHCI-TRACKPAD M1 — the Apple "Wellspring" vendor mode switch. A standard HID class
    /// feature-report read-modify-write (see the PROVENANCE note on `BCM5974_MODE_READ_REQ` for
    /// why every value here is spec-derived or metal-observed, never taken from GPLv2 driver
    /// code), run over EP0 through the same overlay-direct / chain-mode control path every other
    /// request uses:
    ///   1. GET_REPORT(Feature): bmRequestType 0xA1 (IN|CLASS|INTERFACE), bRequest 0x01,
    ///      wValue 0x0300 (Feature report, id 0), wIndex 0, read 8 bytes into `data_buf`.
    ///   2. `data_buf[0] = 0x01` (VENDOR/wellspring mode; 0x08 is the NORMAL single-touch mode).
    ///   3. SET_REPORT(Feature): bmRequestType 0x21 (OUT|CLASS|INTERFACE), bRequest 0x09,
    ///      wValue 0x0300, wIndex 0, write the same 8 bytes back.
    /// Each stage's status is logged. Any stall/timeout is NON-FATAL: a firmware that already
    /// streams needs no switch, so a failed handshake must not un-arm the endpoint — we trace and
    /// let the caller arm regardless. Only ever called on a recognised `vendor_mt` interface, so
    /// QEMU (whose usb-tablet is a standard absolute pointer) never reaches this code.
    ///
    /// MT-INVESTIGATION (IVY): this is also the state the `mtraw` raw-mode probe RESTORES to when
    /// its capture window closes. The probe is selected at the CALL SITE (a `#[cfg]` pair there),
    /// deliberately — so that knob-off this function's name and body stay verbatim what they were
    /// and default media are byte-identical, symbol names included.
    unsafe fn bcm5974_mode_switch(&mut self, t: &Target, intf: u8) {
        // Stage 1 — read the current feature report.
        let read = self.control(
            t, 0xA1, BCM5974_MODE_READ_REQ, BCM5974_MODE_REQ_VALUE, BCM5974_MODE_REQ_INDEX,
            BCM5974_MODE_LEN, true,
        );
        match read {
            Ok(got) => {
                let n = (got as usize).min(BCM5974_MODE_LEN as usize);
                let cur = if n > 0 { *self.data_buf } else { 0 };
                serial_println!(
                    ":: EHCI-HID: [{}] M1 bcm5974 GET_REPORT(feature) addr={} intf={} got={}b byte0={:#04x} == witness ::",
                    self.idx, t.addr, intf, got, cur
                );
            }
            Err(e) => {
                // A device may not answer the read yet still accept the write, so a failed GET is
                // not a reason to skip the SET. Seed the buffer to a known state and press on.
                serial_println!(
                    ":: EHCI-HID: [{}] M1 bcm5974 GET_REPORT(feature) addr={} intf={} FAILED ({}) — writing anyway ::",
                    self.idx, t.addr, intf, e
                );
                for k in 0..BCM5974_MODE_LEN as usize {
                    self.data_buf.add(k).write(0);
                }
            }
        }
        // Stage 2 — flip byte 0 to the raw-multitouch selector.
        self.data_buf.write(BCM5974_MODE_VENDOR);
        // Stage 3 — write the report back (SET_REPORT, class, interface recipient).
        match self.control(
            t, 0x21, BCM5974_MODE_WRITE_REQ, BCM5974_MODE_REQ_VALUE, BCM5974_MODE_REQ_INDEX,
            BCM5974_MODE_LEN, false,
        ) {
            Ok(_) => serial_println!(
                ":: EHCI-HID: [{}] M1 bcm5974 SET_REPORT(feature) addr={} intf={} mode={:#04x} — multitouch stream requested == witness ::",
                self.idx, t.addr, intf, BCM5974_MODE_VENDOR
            ),
            Err(e) => serial_println!(
                ":: EHCI-HID: [{}] M1 bcm5974 SET_REPORT(feature) addr={} intf={} FAILED ({}) — endpoint armed, stream may stay silent ::",
                self.idx, t.addr, intf, e
            ),
        }
    }

    /// MT-INVESTIGATION (IVY) — write ONE value into byte 0 of the 8-byte mode feature report and
    /// read it straight back. Returns the readback byte, or `None` if either leg failed.
    ///
    /// The read-pause-write ordering is the protocol fact taken from FreeBSD wsp.c (BSD-2-Clause;
    /// see the PROVENANCE block on `BCM5974_MODE_NORMAL`): the current report is fetched, a pause
    /// is taken, and only then is the modified report written. wsp pauses for a quarter second; we
    /// use a much shorter settle because our control path is synchronous and already-completed by
    /// the time it returns — the point of the pause is that the write must not race the read's
    /// completion, which our blocking `control` already guarantees, so this is belt-and-braces.
    #[cfg(feature = "mtraw")]
    unsafe fn bcm5974_mode_write(&mut self, t: &Target, val: u8) -> Option<u8> {
        // Read-modify-write: fetch the live report so the seven bytes we do NOT own are preserved.
        let read_ok = self
            .control(
                t, 0xA1, BCM5974_MODE_READ_REQ, BCM5974_MODE_REQ_VALUE, BCM5974_MODE_REQ_INDEX,
                BCM5974_MODE_LEN, true,
            )
            .is_ok();
        if !read_ok {
            for k in 0..BCM5974_MODE_LEN as usize {
                self.data_buf.add(k).write(0);
            }
        }
        ehci_scout::settle_ms(5); // wsp-documented: pause between reading and writing the mode
        self.data_buf.write(val);
        if self
            .control(
                t, 0x21, BCM5974_MODE_WRITE_REQ, BCM5974_MODE_REQ_VALUE, BCM5974_MODE_REQ_INDEX,
                BCM5974_MODE_LEN, false,
            )
            .is_err()
        {
            return None;
        }
        ehci_scout::settle_ms(5);
        // Read back so the witness records what the DEVICE thinks its mode is, not what we asked.
        match self.control(
            t, 0xA1, BCM5974_MODE_READ_REQ, BCM5974_MODE_REQ_VALUE, BCM5974_MODE_REQ_INDEX,
            BCM5974_MODE_LEN, true,
        ) {
            Ok(got) if got > 0 => Some(*self.data_buf),
            _ => None,
        }
    }

    /// MT-INVESTIGATION (IVY) — attempt the documented RAW (multitouch sensor) mode and open a
    /// bounded capture window on whatever the endpoint then streams.
    ///
    /// Sequence (all four steps are wsp.c protocol facts, not copied code):
    ///   1. write the NORMAL/HID selector 0x08 — wsp sets the OFF value first, unconditionally;
    ///   2. pause;
    ///   3. write the RAW selector 0x01 and read the mode byte back;
    ///   4. arm the service loop to hex-dump the first `MT_RAW_DUMP_MAX` reports, after which it
    ///      restores the known-good pointer mode so the trackpad keeps working for the rest of the
    ///      sitting.
    ///
    /// Failure is NON-FATAL throughout: if either write stalls we log it and restore pointer mode
    /// immediately, exactly as the default path tolerates a failed handshake.
    #[cfg(feature = "mtraw")]
    unsafe fn bcm5974_mt_raw_probe(&mut self, t: &Target, intf: u8) {
        let off = self.bcm5974_mode_write(t, BCM5974_MODE_NORMAL);
        serial_println!(
            ":: EHCI-MT: [{}] mode-try val={:#04x} readback={} addr={} intf={} (step 1/2: normal) == witness ::",
            self.idx, BCM5974_MODE_NORMAL,
            match off { Some(v) => v, None => 0xFF }, t.addr, intf
        );
        ehci_scout::settle_ms(50); // wsp takes a long pause between the OFF and ON writes
        let on = self.bcm5974_mode_write(t, BCM5974_MODE_VENDOR);
        serial_println!(
            ":: EHCI-MT: [{}] mode-try val={:#04x} readback={} addr={} intf={} (step 2/2: raw sensor) == witness ::",
            self.idx, BCM5974_MODE_VENDOR,
            match on { Some(v) => v, None => 0xFF }, t.addr, intf
        );
        match on {
            Some(_) => {
                // Capture window open — the service loop dumps and then restores.
                self.mt_probe = Some((*t, intf));
                self.mt_dumped = 0;
            }
            None => {
                serial_println!(
                    ":: EHCI-MT: [{}] raw mode-set FAILED — restoring pointer mode immediately ::",
                    self.idx
                );
                self.bcm5974_mode_switch(t, intf);
                serial_println!(":: EHCI-MT: [{}] mode-restored == witness ::", self.idx);
            }
        }
    }

    /// MT-INVESTIGATION (IVY) — close the capture window: put the pad back into the mode whose
    /// 8-byte relative stream the landed pointer path decodes. Called from the service loop AFTER
    /// its endpoint iteration has finished, so no endpoint borrow is live across the EP0 traffic.
    #[cfg(feature = "mtraw")]
    unsafe fn bcm5974_mt_restore(&mut self) {
        if let Some((t, intf)) = self.mt_probe.take() {
            self.bcm5974_mode_switch(&t, intf);
            serial_println!(
                ":: EHCI-MT: [{}] mode-restored addr={} intf={} after {} raw report(s) == witness ::",
                self.idx, t.addr, intf, self.mt_dumped
            );
        }
    }

    /// Build + link one periodic interrupt QH and arm its first qTD.
    ///
    /// N1 — split-compatibility of the every-frame frame list (EHCI 4.12.2): S-mask and C-mask
    /// are *microframe* masks evaluated within EACH frame the QH is reached in, not frame
    /// selectors. Linking the QH from every frame-list entry therefore stays split-correct: on
    /// a FS/LS endpoint the controller issues the start-split in µframe 0 (S-mask 0x01) and
    /// polls complete-splits in µframes 2-4 (C-mask 0x1C) of every frame — a 1 ms service rate
    /// that over-serves an 8 ms boot-HID interval but is legal and loses nothing. On a HS
    /// endpoint (Topology B) C-mask must be zero and S-mask alone paces one transaction per
    /// frame. The simplification is deliberate and verified against 4.12.2, not inherited
    /// silently; bInterval striding is a later refinement (design R8) if TT load ever demands.
    unsafe fn arm_interrupt_ep(
        &mut self,
        t: &Target,
        ep: u8,
        mps: u16,
        is_kbd: bool,
        is_rel: bool,
        layout: Option<ReportLayout>,
    ) {
        if self.int_next >= MAX_INT_EPS {
            serial_println!(
                ":: EHCI-HID: [{}] static int-EP pool exhausted ({}) — endpoint skipped ::",
                self.idx, MAX_INT_EPS
            );
            return;
        }
        let slot = &mut (*self.pool()).int_slots[self.int_next];
        let (qh, qtd, buf) = (
            &mut slot.qh as *mut Qh,
            &mut slot.qtd as *mut Qtd,
            slot.buf.0.as_mut_ptr(),
        );
        let (Some(qh_phys), Some(qtd_phys), Some(buf_phys)) = (
            phys_of(qh, 32),
            phys_of(qtd, 32),
            // MT-INVESTIGATION (IVY): `INT_BUF_ALIGN` is 64 knob-off (verbatim what this line
            // always passed) and 1024 under `mtraw`, where the grown receive buffer must be
            // page-crossing-free for the single qTD buffer pointer to cover it.
            phys_of(buf, INT_BUF_ALIGN),
        ) else {
            serial_println!(
                ":: EHCI-HID: [{}] STOP-NOTE int-EP slot failed the phys/alignment contract — endpoint skipped ::",
                self.idx
            );
            return;
        };
        self.int_next += 1;

        // MT-INVESTIGATION (IVY) — how many bytes ONE armed transfer may accept.
        //
        // Knob-off this is `mps`, verbatim what it has always been: one MPS-sized transaction per
        // service pass, which is all a HID boot report or an 8-byte 0x02 trackpad report needs.
        //
        // Knob-on, for the vendor-multitouch endpoint ONLY, it becomes the full receive buffer.
        // This is the answer to "how do >MPS reports arrive on our int-IN path": EHCI does the
        // reassembly IN HARDWARE. A qTD's Total Bytes To Transfer field (EHCI 3.5.3) is not a
        // packet size — the controller keeps issuing MPS-sized IN transactions against the SAME
        // qTD, advancing the buffer pointer, until either `total` bytes have moved or the device
        // returns a SHORT packet (which retires the qTD and leaves the residue in Total Bytes).
        // So a raw frame larger than MPS needs NO software reassembly and no transfer-layer
        // change: it needs the qTD to be armed for more than one packet's worth, and a buffer big
        // enough to land in. With `total == mps` (the pre-arc arming) the controller stops after
        // exactly one packet and the rest of the frame is lost — which is precisely why the probe
        // arc predicted a TRUNCATED capture. `report.len()` at the far end is therefore the true
        // frame length whenever the frame is short of `total`, exactly the datum the decoder
        // length-validates on.
        #[cfg(not(feature = "mtraw"))]
        let rx_total = mps as u32;
        #[cfg(feature = "mtraw")]
        let rx_total = if layout.as_ref().is_some_and(|l| l.vendor_mt) {
            INT_BUF_LEN as u32
        } else {
            mps as u32
        };

        (*qh).ep_chars = (t.addr as u32)
            | ((ep as u32) << 8)
            | t.eps
            | QH_DTC
            | ((mps as u32) << QH_MPS_SHIFT);
        let split = if t.eps == QH_EPS_HIGH {
            0
        } else {
            (0x1C << QH_CMASK_SHIFT)
                | ((t.hub_addr as u32) << QH_HUBADDR_SHIFT)
                | ((t.hub_port as u32) << QH_PORT_SHIFT)
        };
        (*qh).ep_caps = QH_MULT1 | (0x01 << QH_SMASK_SHIFT) | split;

        // First transfer of a freshly-configured interrupt endpoint is DATA0, armed in
        // whichever mode enumeration settled on (overlay-direct on this metal, qTD-chain on
        // QEMU — see Controller::overlay_mode).
        if self.overlay_mode {
            let _ = (qtd, qtd_phys); // slot storage retained; the controller never sees it
            (*qh).current_qtd = 0;
            (*qh).overlay[0] = PTR_TERMINATE;
            (*qh).overlay[1] = PTR_TERMINATE;
            (*qh).overlay[3] = buf_phys as u32;
            (*qh).overlay[4] = 0;
            core::ptr::write_volatile(
                &mut (*qh).overlay[2],
                QTD_ACTIVE | QTD_CERR3 | (rx_total << QTD_TOTAL_SHIFT) | QTD_PID_IN | QTD_IOC,
            );
        } else {
            write_qtd(qtd, PTR_TERMINATE, QTD_PID_IN | QTD_IOC, rx_total, buf_phys);
            (*qh).overlay[1] = PTR_TERMINATE;
            (*qh).overlay[2] = 0;
            (*qh).overlay[0] = qtd_phys as u32;
        }

        // Link: new QH points at the current chain head, then every frame-list entry points at
        // the new QH (entries were Terminate or the old head — both cases are one word).
        let fl = self.frame_list;
        let old_head = core::ptr::read_volatile(fl);
        (*qh).horiz = old_head;
        for i in 0..1024 {
            core::ptr::write_volatile(fl.add(i), (qh_phys as u32) | PTR_TYPE_QH);
        }

        if !self.periodic_on {
            let cmd = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
            let _ = mmio_write32(self.op + OP_USBCMD, cmd | CMD_PSE);
            self.periodic_on = true;
        }

        // KBDWIT: the FRINDEX baseline, read here — after the QH is linked and PSE is on, so the
        // window `adv=` measures is genuinely "armed and expected to complete". Hoisted out of the
        // struct literal below so it cannot be entangled with the `int_eps` borrow. One MMIO read
        // per endpoint on the arming path; `None` is carried honestly rather than defaulted to 0.
        #[cfg(feature = "kbdwit")]
        let kbdwit_fr0 = mmio_read32(self.op + KBDWIT_OP_FRINDEX);
        self.int_eps.push(IntEp {
            qh,
            qtd,
            qtd_phys,
            buf,
            buf_phys,
            mps,
            toggle: false,
            is_kbd,
            is_rel_mouse: is_rel,
            layout,
            reports: 0,
            dead: false,
            prev_buttons: 0,
            last_report_ms: 0,
            // KBDWIT: stamp the silence clock's origin at the moment the endpoint becomes armed —
            // i.e. after the QH is linked and PSE is on, so the interval this witness measures is
            // genuinely "armed and expected to complete", never "still being set up".
            #[cfg(feature = "kbdwit")]
            kbdwit_armed_ms: crate::arch::ms(),
            #[cfg(feature = "kbdwit")]
            kbdwit_last_ms: 0,
            #[cfg(feature = "kbdwit")]
            kbdwit_fired: false,
            #[cfg(feature = "kbdwit")]
            kbdwit_armed_frindex: kbdwit_fr0,
            #[cfg(feature = "mtraw")]
            rx_total,
            #[cfg(feature = "mtraw_inject")]
            mt_prev: None,
        });
    }

    /// The controller's static DMA pool (index-bound checked at construction).
    fn pool(&self) -> *mut qh::DmaPool {
        unsafe { &mut DMA_POOLS[self.idx] as *mut qh::DmaPool }
    }

    /// Main-loop poll: ack USBSTS, then for each armed endpoint consume a completed report,
    /// decode it through the boot-report layouts (same table + logic as the xHCI HID path), and
    /// re-arm. The EHCI analogue of the xHCI `queue_keyboard_read` re-arm idiom.
    unsafe fn service(&mut self) {
        if let Some(sts) = mmio_read32(self.op + OP_USBSTS) {
            if sts & STS_RW1C != 0 {
                let _ = mmio_write32(self.op + OP_USBSTS, sts & STS_RW1C);
            }
        }
        let idx = self.idx;
        let om = self.overlay_mode;
        // KBDWIT: hoisted for the same reason `idx`/`om` are — the endpoint loop below holds an
        // exclusive borrow of `self.int_eps`, so the probe cannot reach through `self` for the
        // operational-register base or the frame-list head and must be handed them as scalars.
        #[cfg(feature = "kbdwit")]
        let (kw_op, kw_fl) = (self.op, self.frame_list as *const u32);
        // MT-INVESTIGATION (IVY): local mirror of the capture-window counter, so the endpoint
        // iteration below keeps its exclusive borrow of `int_eps` (the EP0 restore runs after).
        #[cfg(feature = "mtraw")]
        let mut mt_dumped = if self.mt_probe.is_some() { Some(self.mt_dumped) } else { None };
        for e in self.int_eps.iter_mut() {
            if e.dead {
                continue;
            }
            let tok = if om {
                core::ptr::read_volatile(&(*e.qh).overlay[2])
            } else {
                core::ptr::read_volatile(&(*e.qtd).token)
            };
            if tok & QTD_ACTIVE != 0 {
                // KBDWIT: still armed, nothing came back this pass — the only state from which the
                // s58 silence is observable. The probe self-bounds (one dump per endpoint per boot)
                // and returns after a single bool test once it has fired or before its deadline.
                #[cfg(feature = "kbdwit")]
                kbdwit_probe(e, idx, om, kw_op, kw_fl, tok);
                continue;
            }
            // KBDWIT: the qTD retired — a COMPLETION, whether or not it carried report bytes.
            // Stamped here, above every decoder, so no report layout, length gate or `dead` path
            // can influence whether this endpoint counts as alive.
            #[cfg(feature = "kbdwit")]
            {
                e.kbdwit_last_ms = crate::arch::ms();
            }
            if tok & QTD_ERR_MASK != 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] STOP-NOTE interrupt endpoint halted (token {:#010x}) — endpoint retired, not forced ::",
                    idx, tok
                );
                e.dead = true;
                continue;
            }
            // MT-INVESTIGATION (IVY): bytes actually received = armed total minus the residue the
            // controller left in Total Bytes To Transfer. Knob-off the armed total IS `e.mps`, so
            // the expression is unchanged; knob-on, on the vendor-multitouch endpoint, it is the
            // buffer size and the difference is the true length of a multi-packet raw frame.
            // (Written as a `#[cfg]` PAIR rather than one hoisted local because hoisting would
            // keep the value live across the decode block and cost 16 bytes of `.text` knob-off —
            // this arc's default media must stay byte-identical.)
            #[cfg(not(feature = "mtraw"))]
            let len = (e.mps as u32).saturating_sub((tok >> QTD_TOTAL_SHIFT) & 0x7FFF) as usize;
            #[cfg(feature = "mtraw")]
            let len = e.rx_total.saturating_sub((tok >> QTD_TOTAL_SHIFT) & 0x7FFF) as usize;
            if len > 0 {
                // Boot reports are ≤ 8 B; a parsed report-pointer report can be longer (the
                // buffer is 64 B), so cap by kind.
                // MT-INVESTIGATION: knob-on the layout cap becomes the (grown) buffer length —
                // still a hard cap, never larger than the allocation, so the slice below can
                // never run off the buffer even if the controller reported nonsense residue.
                #[cfg(not(feature = "mtraw"))]
                let cap = if e.layout.is_some() { len.min(64) } else { len.min(8) };
                #[cfg(feature = "mtraw")]
                let cap = if e.layout.is_some() { len.min(INT_BUF_LEN) } else { len.min(8) };
                let report = core::slice::from_raw_parts(e.buf, cap);
                e.reports = e.reports.wrapping_add(1);
                if let Some(l) = e.layout {
                    if l.vendor_mt {
                        // M1 (RMBP-FIX, 2026-07-18): the raw-report dump exists ONLY to capture the
                        // opaque stream's byte layout, and that characterization is COMPLETE. Bound it
                        // hard — usbdebug builds only, first 4 reports total per device — so it can
                        // never flood the framebuffer console (~100+ heap-allocating lines/sec under
                        // touch, the "machine appears hung" defect). Its `String` alloc is off the
                        // default hot path ENTIRELY: on a GUI/default build this whole `#[cfg]` block
                        // (and `dump_vendor_report`) is compiled out — zero dumps, zero allocation.
                        #[cfg(feature = "usbdebug")]
                        if e.reports <= 4 {
                            dump_vendor_report(idx, e.reports, report);
                        }
                        // MT-INVESTIGATION (IVY, `mtraw` only): the capture window. Hex-dump at
                        // most `MT_RAW_DUMP_MAX` reports of at most `MT_RAW_DUMP_BYTES` bytes each
                        // — bounded twice over, because the FTDI console is a 64 KiB drop-oldest
                        // ring and an unbounded dump evicts the boot log that gives it context.
                        // The pointer decode below still runs on these reports: if the raw mode
                        // never engaged they are ordinary 0x02 relative reports and the cursor
                        // keeps moving; if it DID engage, `decode_trackpad_rel`'s length + ID gate
                        // rejects them and nothing is pushed. Either way no clamp is weakened.
                        #[cfg(feature = "mtraw")]
                        if let Some(n) = mt_dumped.as_mut() {
                            if *n < MT_RAW_DUMP_MAX {
                                *n += 1;
                                dump_raw_report(idx, *n, &report[..report.len().min(MT_RAW_DUMP_BYTES)]);
                                // MT-INVESTIGATION (IVY, decode prep): run the TYPE2 decoder on
                                // the SAME bounded first-N frames and print one witness line. The
                                // decoder is total — a non-raw (HID-mode) report simply fails its
                                // length gate and the line says so — so this cannot misread the
                                // 8-byte 0x02 stream as finger data.
                                dump_type2_frame(idx, e.mps, report);
                            }
                        }
                        // MT-INVESTIGATION (IVY, `mtraw_inject` sub-knob ONLY, default OFF): turn
                        // the first finger's ABSOLUTE position into pointer deltas. Deliberately
                        // gated behind a second knob: the pointer path stays 0x02-driven until
                        // metal proves raw mode is stable, so the default `mtraw` build DECODES
                        // and WITNESSES without ever touching the event queue.
                        #[cfg(feature = "mtraw_inject")]
                        mt_inject_first_finger(report, &mut e.mt_prev);
                        // M2 (RMBP-FIX silicon retarget): after the bcm5974 mode switch the internal
                        // trackpad does NOT stream the descriptor's opaque 0x44 / 511-byte multitouch
                        // frame — that hypothesis is REFUTED on this device path (the decode it drove,
                        // `decode_vendor_first_finger` + `VMT_FINGER_*`, is KEPT below as documented
                        // history + self-test, never as the live path). Ground truth from silicon: it
                        // streams 8-byte Report ID 0x02 reports — [0]=id, [1]=buttons (0x00 up /
                        // 0x01 down), [2]=dx i8, [3]=dy i8, [4..8] zero/unknown. Decode those straight
                        // into the RELATIVE pointer path (the same `pal::Event::Mouse` seam the
                        // boot-mouse path uses). Length-checked + ID-gated inside `decode_trackpad_rel`:
                        // a short or non-0x02 report yields None → no event, no state change.
                        if let Some((buttons, dx, dy)) = decode_trackpad_rel(report) {
                            // Bounded one-line format witness on the first decoded report.
                            if e.reports == 1 {
                                serial_println!(
                                    ":: EHCI-HID: [{}] trackpad format witness: 8-byte id=0x02 rel — buttons={:#04x} dx={} dy={} == witness ::",
                                    idx, buttons, dx, dy
                                );
                            }
                            if dx != 0 || dy != 0 {
                                crate::pal::push_event(crate::pal::Event::Mouse { x: dx, y: dy });
                            }
                            // CLICK-1 (metal verdict): emit ONE `Event::Button` per button-DOWN
                            // edge (0x00 -> 0x01 on this pad) — the click observable: a click
                            // while vug/pulse runs exits the demo like a keystroke. Release
                            // emits nothing. Serial line per press (human-rate, bounded).
                            // CLICK-3: the edge test plus re-press recovery (see `note_buttons`) —
                            // this is the path the rMBP internal trackpad takes, and the one where
                            // the stale latch swallowed every stationary second click.
                            if e.note_buttons(buttons, idx) {
                                crate::pal::push_event(crate::pal::Event::Button(buttons));
                                serial_println!(
                                    ":: EHCI-HID: [{}] trackpad click (button-down edge, buttons={:#04x}) == witness ::",
                                    idx, buttons
                                );
                            }
                        }
                    } else {
                        // M2 report-pointer path: decode X/Y/buttons from the parsed field map.
                        // Relative axes (a mouse) → pal::Event::Mouse; absolute (tablet / trackpad)
                        // → MouseAbsolute — the SAME pointer-event path the xHCI HID stack delivers.
                        let (x, y, buttons, fingers) = decode_report_pointer(report, &l);
                        if l.relative {
                            if x != 0 || y != 0 {
                                crate::pal::push_event(crate::pal::Event::Mouse { x, y });
                            }
                        } else if x != 0 || y != 0 {
                            crate::pal::push_event(crate::pal::Event::MouseAbsolute { x, y });
                        }
                        // CLICK-1: primary-button DOWN edge → one Button event (same semantic as
                        // the trackpad path above).
                        let btn = (buttons & 0xFF) as u8;
                        if e.note_buttons(btn, idx) {
                            crate::pal::push_event(crate::pal::Event::Button(btn));
                        }
                        if e.reports == 1 || e.reports % 32 == 0 {
                            serial_println!(
                                ":: EHCI-HID: [{}] report-pointer {} reports, last {} x={} y={} buttons={:#04x} fingers={} == witness ::",
                                idx, e.reports,
                                if l.relative { "rel" } else { "abs" },
                                x, y, buttons, fingers
                            );
                        }
                    }
                } else if e.is_kbd {
                    decode_boot_keyboard(report);
                    if e.reports == 1 || e.reports % 32 == 0 {
                        serial_println!(
                            ":: EHCI-HID: [{}] kbd {} reports, last {:02x} {:02x} .. == witness ::",
                            idx, e.reports, report[0], report.get(2).copied().unwrap_or(0)
                        );
                    }
                } else if e.is_rel_mouse && len >= 3 {
                    let (dx, dy) = (report[1] as i8 as i32, report[2] as i8 as i32);
                    if dx != 0 || dy != 0 {
                        crate::pal::push_event(crate::pal::Event::Mouse { x: dx, y: dy });
                    }
                    // CLICK-1: boot-mouse buttons live in report[0]; primary DOWN edge → Button.
                    if e.note_buttons(report[0], idx) {
                        crate::pal::push_event(crate::pal::Event::Button(report[0]));
                    }
                    if e.reports == 1 || e.reports % 32 == 0 {
                        serial_println!(
                            ":: EHCI-HID: [{}] mouse {} reports, last dx={} dy={} buttons={:#04x} == witness ::",
                            idx, e.reports, dx, dy, report[0]
                        );
                    }
                }
            }
            // Re-arm in the controller's transfer mode: flip the software toggle (QH_DTC —
            // the toggle lives here), then either rewrite the overlay in place
            // (overlay-direct; no qTD fetch — this metal) or refresh + point at the qTD.
            e.toggle = !e.toggle;
            let dt = if e.toggle { QTD_DT } else { 0 };
            // MT-INVESTIGATION (IVY): re-arm for the SAME total the endpoint was armed with. Each
            // statement is a `#[cfg]` PAIR whose knob-off member is the ORIGINAL expression,
            // verbatim and in place. Every less repetitive shape tried here (one hoisted local, or
            // a local inside each branch) changes `service_ehci_hid`'s register allocation — same
            // instruction count, same symbol size, but NOT byte-identical, which this arc's default
            // media must be. The duplication is the price of that guarantee.
            if om {
                (*e.qh).overlay[0] = PTR_TERMINATE;
                (*e.qh).overlay[1] = PTR_TERMINATE;
                (*e.qh).overlay[3] = e.buf_phys as u32;
                (*e.qh).overlay[4] = 0;
                #[cfg(not(feature = "mtraw"))]
                core::ptr::write_volatile(
                    &mut (*e.qh).overlay[2],
                    QTD_ACTIVE | QTD_CERR3 | ((e.mps as u32) << QTD_TOTAL_SHIFT) | QTD_PID_IN | QTD_IOC | dt,
                );
                #[cfg(feature = "mtraw")]
                core::ptr::write_volatile(
                    &mut (*e.qh).overlay[2],
                    QTD_ACTIVE | QTD_CERR3 | (e.rx_total << QTD_TOTAL_SHIFT) | QTD_PID_IN | QTD_IOC | dt,
                );
            } else {
                #[cfg(not(feature = "mtraw"))]
                write_qtd(e.qtd, PTR_TERMINATE, QTD_PID_IN | QTD_IOC | dt, e.mps as u32, e.buf_phys);
                #[cfg(feature = "mtraw")]
                write_qtd(e.qtd, PTR_TERMINATE, QTD_PID_IN | QTD_IOC | dt, e.rx_total, e.buf_phys);
                (*e.qh).overlay[1] = PTR_TERMINATE;
                core::ptr::write_volatile(&mut (*e.qh).overlay[2], 0);
                core::ptr::write_volatile(&mut (*e.qh).overlay[0], e.qtd_phys as u32);
            }
        }
        // MT-INVESTIGATION (IVY): the endpoint borrow is released here, so the EP0 restore is safe
        // to run. Close the capture window as soon as it is full — the pad goes back to the mode
        // the landed pointer path decodes, and the probe never fires again this boot.
        #[cfg(feature = "mtraw")]
        if let Some(n) = mt_dumped {
            self.mt_dumped = n;
            if n >= MT_RAW_DUMP_MAX {
                self.bcm5974_mt_restore();
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════════════════════
// KBDWIT — the one-shot, per-endpoint EHCI interrupt-silence witness.
//
// THE OBSERVATION (metal, 2012 rMBP, build s58, 2026-08-01). On two consecutive boots the USB
// KEYBOARD produced ZERO interrupt completions for the entire boot, while the TRACKPAD — same
// physical device chain, same hub, same TT, same EHCI function — streamed normally. The kernel
// recorded `ehci:kbd-armed`, so the interrupt endpoint WAS armed; nothing ever came back on it.
// No halt STOP-NOTE was emitted, so the qTD was not retired in error either. It did not recur on
// s59 or s60 on the same hardware. Cause unknown. This instrument exists to convict or acquit on
// the next recurrence; it is NOT a fix and NOT a recovery.
//
// WHAT IT IS: a SNAPSHOT taken at a deadline, not an alarm. Once per boot, per endpoint, when an
// armed interrupt endpoint has gone `KBDWIT_QUIET_MS` without a single completion (or that long
// since its last one), every register and descriptor word that separates the candidate failure
// modes is dumped raw, and the endpoint latches silent forever.
//
// IT THEREFORE PRINTS ON HEALTHY BOOTS TOO, and that is deliberate. An idle boot keyboard
// genuinely completes nothing — no SET_IDLE is sent on this path and no key is pressed — so its
// dump is the ACQUITTAL BASELINE: qTD Active, CERR=3, CERR unburned, PSE/PSS on, FRINDEX advanced.
// The convicting boot is then read by DIFFERENCE against the baseline the same instrument printed
// on every other boot. A witness that only spoke once it had already decided what was wrong could
// not do that, and could not be checked for its own health.
//
// FOR THAT REASON THE HEADER CARRIES NO VERDICT WORD. It reads `NO-COMPLETIONS`, a statement of
// fact, with `class=never-completed` or `class=went-quiet` — never "SILENT", which an `awk
// /KBDWIT/` over a perfectly healthy capture would read as a recurrence that did not happen.
//
// WHAT IT CAN AND CANNOT CONVICT — the limit of a deadline this early. The probe fires at
// armed + `KBDWIT_QUIET_MS`, i.e. seconds into boot and BEFORE the operator has touched anything,
// and the one-shot latch is spent there. So:
//   * IT CAN CONVICT, from this dump alone: a QH orphaned from the frame list (line 5 `fl0` plus
//     line 4 `horiz`/`qh`), a QH programmed with the wrong device address or endpoint number
//     (line 5, decoded from the controller's own words), a periodic schedule that is off or not
//     running (line 6 `pse`/`pss`), a halted or host-errored controller (line 6 `hch`/`hse`), a
//     frozen frame counter (line 7 `adv=0x0000`), and a wedged split transaction with the error
//     counter burned down (line 3 `cerr`, line 4 `ovl4`).
//   * IT CANNOT SEPARATE, at this deadline, an idle keyboard from the s58 recurrence. Both show
//     `class=never-completed` with the qTD Active, CERR=3 and a healthy controller, because a
//     boot keyboard nobody has typed on completes nothing either. Every hypothesis that only
//     manifests at an UNANSWERED KEYPRESS — device-side silence, a data-toggle desync that leaves
//     the qTD Active forever — produces a picture identical to the healthy baseline here.
//     A clean dump is therefore NOT an acquittal of that class; it is silence about it. Convicting
//     those would need a second, keypress-triggered sample, which this arc does not build.
//
// WHY PER-ENDPOINT, NOT PER-CONTROLLER. During the failure the trackpad is streaming on the SAME
// controller, so any controller-level "is anything completing?" test reads HEALTHY on the exact
// boot that motivated this instrument. The silence clock lives on `IntEp` and is stamped only by
// that endpoint's own completions.
//
// BOUNDS. `kbdwit_fired` latches on the first dump: at most one dump per endpoint per boot, at
// most `MAX_INT_EPS` (4) per controller for a whole boot. No loop, no retry, no wait, no
// allocation, no register write — every access below is a read. Cost on the service path is one
// bool test plus one `ms()` read before the deadline, and one bool test after it fires. Note the
// path this rides is `service()`, the POST-boot main-loop poll — NOT the `init()` bring-up block
// the EPACE ledger measures and this seat just trimmed 6324 ms -> 2010 ms — so the deadline
// cannot land inside that budget at all.
//
// INSTRUMENT HONESTY — asked of every field, "can this be wrong in a way that still looks right?"
//   * Every controller-visible word is printed RAW as well as decoded, so a decode bug here can
//     never destroy the evidence: a reader with the EHCI spec re-derives each flag from the hex.
//   * IDENTITY (device address, endpoint number, MPS, speed, TT hub/port) is decoded from the
//     QH's OWN `ep_chars`/`ep_caps` — the words the controller is executing — not from the
//     driver's software copy. `kind=` is the software belief, printed alongside. If the driver
//     and the controller disagree about which endpoint this is, the capture shows it.
//   * The TOKEN is printed three ways: `seen=` (what the service loop tested this pass) plus both
//     live words, `ovl_tok=` (the controller's working copy in the QH overlay) and `qtd_tok=`
//     (the standalone qTD), with `path=` naming which of the two the driver actually drives.
//     `seen` differing from the live word means the token is CHANGING — a frozen source printed
//     as a live value is the failure mode this seat has been bitten by repeatedly.
//   * `qtd_driven=` guards `qtd_tok=`. On the overlay-direct path (this metal) the standalone qTD
//     is NEVER handed to the controller and `write_qtd` is never called for an interrupt slot, so
//     `qtd_tok` is untouched zeroed pool memory that decodes to `active=0 halted=0` — "completed
//     cleanly", flatly contradicting the header three lines above it. A value meaning NOT DRIVEN
//     must never sit unmarked in the position of a measurement, so `qtd_driven=0` says so.
//   * FRINDEX advancement is measured from a BASELINE STAMPED AT ARM TIME (`kbdwit_armed_frindex`)
//     to the dump — a real multi-second window — not from a pair of reads taken microseconds
//     apart. The earlier design bracketed the dump's own serial output on the theory that it cost
//     milliseconds at 115200 baud; on this rig there is no 16550, the fbcon mirror is detached
//     before the first probe-reaching call, and the remaining sinks are try_lock ring memcpys, so
//     seven lines cost TENS OF MICROSECONDS. FRINDEX ticks every 125 us, so that pair returned the
//     same microframe on a perfectly healthy controller and the one line meant to answer "is the
//     periodic schedule advancing?" printed a bare zero on every metal boot — dead exactly where
//     it was needed, alive only under QEMU where a real UART exists. The `post=` sample is kept
//     (it costs nothing and does resolve where printing is genuinely slow) but it is no longer the
//     tell: `adv=` is. Its magnitude is WRAP-AMBIGUOUS by construction — FRINDEX is 14 bits and
//     wraps every 2.048 s, while the window is >= `KBDWIT_QUIET_MS` — so `adv` answers "did the
//     frame counter move at all?", not "by how much". Stated here rather than inferred, because a
//     reader who took `adv` for a rate would be wrong by whole wraps.
//   * Every MMIO read carries an `ok=` flag, because `mmio_read32` returns an Option and a
//     failed read rendered as `0x00000000` would read as "halted clear, host error clear, all
//     well" — the most dangerous possible lie this dump could tell.
//   * `ms()` is the calibrated APIC tick; before calibration it degrades to ~1 ms/tick. The
//     deadline and the elapsed prints both use it, so they degrade TOGETHER and the ratios stay
//     truthful.
// ══════════════════════════════════════════════════════════════════════════════════════════════

/// KBDWIT — silence (ms) after arming, or after the endpoint's last completion, at which the
/// snapshot is taken. Sits well past every bring-up settle on this path (the whole EHCI HID
/// `init()` block now costs ~2.0 s end to end) so a dump can never catch a schedule that is merely
/// still starting, and well inside any boot capture so it always lands in the log.
#[cfg(feature = "kbdwit")]
const KBDWIT_QUIET_MS: u64 = 4000;

/// KBDWIT — USBSTS bit 15, Async Schedule Status (EHCI 1.0 §2.3.2): the controller is traversing
/// the asynchronous list. Genuinely absent from the module's shared register set — its only other
/// readers, both in `overlay_txn` (mod.rs:812 and mod.rs:820), spell it as a bare `1 << 15` — so it
/// needs a name here. Periodic Schedule Status is the neighbouring bit 14, which this probe takes
/// from the module's own `STS_PSS`: that constant is `1 << 14` and correct per EHCI 1.0 §2.3.2,
/// and trunk `8b112c64` is the prose fix for its doc comment, so there is nothing for this
/// instrument to work around and no reason to duplicate it.
///
/// Left KBDWIT-local rather than promoted to a shared `STS_ASS` beside `STS_PSS`: that const block
/// is the one region of this file trunk is concurrently editing, and this arc was verified
/// collision-free against it. Promoting it — and folding in those two bare literals — is the right
/// follow-up, on a branch that is not racing that hunk.
#[cfg(feature = "kbdwit")]
const KBDWIT_STS_ASS: u32 = 1 << 15;
/// KBDWIT — operational-register offset of FRINDEX (EHCI 1.0 §2.3.4), the frame index the
/// periodic traversal is driven from. This witness is the driver's first consumer of a "is the
/// schedule actually advancing?" reading, so the offset appears here rather than in the module's
/// shared register set.
#[cfg(feature = "kbdwit")]
const KBDWIT_OP_FRINDEX: u64 = 0x0C;

/// KBDWIT — decode a qTD token's PID field (bits 9:8, EHCI 1.0 §3.5.3).
#[cfg(feature = "kbdwit")]
fn kbdwit_pid(tok: u32) -> &'static str {
    match (tok >> 8) & 0x3 {
        0 => "OUT",
        1 => "IN",
        2 => "SETUP",
        _ => "rsvd",
    }
}

/// KBDWIT — decode a QH's endpoint-speed field (`ep_chars` bits 13:12, EHCI 1.0 §3.6.2).
#[cfg(feature = "kbdwit")]
fn kbdwit_eps(chars: u32) -> &'static str {
    match (chars >> 12) & 0x3 {
        0 => "full",
        1 => "low",
        2 => "high",
        _ => "rsvd",
    }
}

/// KBDWIT — the probe. Called from the service loop for an endpoint whose qTD is STILL ACTIVE
/// (nothing completed this pass); dumps once and latches. See the section comment above for the
/// observation this exists for, its bounds, and the honesty argument for each field.
#[cfg(feature = "kbdwit")]
unsafe fn kbdwit_probe(e: &mut IntEp, idx: usize, om: bool, op: u64, fl: *const u32, seen: u32) {
    // One-shot, cheapest test first: after the single dump this endpoint will ever emit, the whole
    // probe is one predictable branch on the service path.
    if e.kbdwit_fired {
        return;
    }
    let now = crate::arch::ms();
    // Reference = this endpoint's last completion, or its arming instant if it has never completed
    // anything (the s58 keyboard's exact state). A 0 reference means the stamp was never taken —
    // unreachable, since `arm_interrupt_ep` stamps `kbdwit_armed_ms` — but treating it as "no
    // reference" rather than as time zero keeps an unbounded `now` from reading as a silence.
    let since = if e.kbdwit_last_ms != 0 { e.kbdwit_last_ms } else { e.kbdwit_armed_ms };
    if since == 0 || now.wrapping_sub(since) < KBDWIT_QUIET_MS {
        return;
    }
    e.kbdwit_fired = true;

    // ── sample everything BEFORE printing, so the whole dump describes one instant ──────────────
    let fr_a = mmio_read32(op + KBDWIT_OP_FRINDEX);
    let t_a = crate::arch::ms();
    let sts = mmio_read32(op + OP_USBSTS);
    let cmd = mmio_read32(op + OP_USBCMD);
    let chars = core::ptr::read_volatile(&(*e.qh).ep_chars);
    let caps = core::ptr::read_volatile(&(*e.qh).ep_caps);
    let horiz = core::ptr::read_volatile(&(*e.qh).horiz);
    let cur = core::ptr::read_volatile(&(*e.qh).current_qtd);
    let ovl0 = core::ptr::read_volatile(&(*e.qh).overlay[0]);
    let ovl1 = core::ptr::read_volatile(&(*e.qh).overlay[1]);
    let ovl2 = core::ptr::read_volatile(&(*e.qh).overlay[2]);
    let ovl3 = core::ptr::read_volatile(&(*e.qh).overlay[3]);
    // overlay[4]/overlay[5] are qTD buffer pointers 1 and 2, which for a SPLIT transaction carry
    // the controller's split progress state — C-prog-mask (buf1 bits 7:0) and FrameTag/S-bytes
    // (buf2 bits 4:0 / 11:5), EHCI 1.0 §3.5.4. The rMBP keyboard is a low/full-speed device behind
    // a TT, so a wedged split shows up HERE and nowhere else.
    //
    // THEY ARE NOT EQUALLY TRUSTWORTHY, and the difference matters on the metal path:
    //   * `ovl4` IS cleared before every (re-)arm, by a DIFFERENT mechanism per path. On
    //     overlay-direct, `arm_interrupt_ep` (1587) and the service-loop re-arm (1877) each write
    //     `overlay[4] = 0` directly; both sites are inside the `overlay_mode` branch, so on
    //     qtd-chain the driver never writes this QH's `overlay[4]` at all — there the controller
    //     LOADS the overlay from the qTD, and `write_qtd` (qh.rs:185) sets
    //     `buf = [buf_phys, 0, 0, 0, 0]`. Either way the word is zero going into the transfer, so
    //     a non-zero value is progress the controller made on THIS transfer. Read it as evidence.
    //     (Note for anyone re-deriving this: the driver's third `overlay[4] = 0`, at mod.rs:799,
    //     is in `overlay_txn` and lands on the CONTROL QH — it has nothing to do with `e.qh`. And
    //     `init_schedules` writes only `overlay[0]/[1]/[2]`; it never touches `overlay[4]`.)
    //   * `ovl5` IS NEVER WRITTEN BY THIS DRIVER, anywhere. On the qtd-chain path `write_qtd`
    //     zeroes `buf[1..5]` and the controller copies them into the overlay when it fetches the
    //     qTD, so it happens to be clean there. On OVERLAY-DIRECT — the mode this metal settles
    //     into — no qTD is ever fetched, the driver writes the overlay in place, and nothing
    //     clears `overlay[5]`. It can therefore hold FrameTag/S-bytes the controller wrote during
    //     an EARLIER, SUCCESSFULLY COMPLETED transfer on this same QH. A reader who took a
    //     non-zero `ovl5` for split progress on the STALLED transfer would be chasing a phantom.
    //     Printed because the raw word is still worth having; read as residue, not as evidence,
    //     whenever `path=overlay-direct`.
    // Deliberately NOT fixed by zeroing `overlay[5]` at re-arm: that is a WRITE on the transfer
    // path, and this witness is read-only by construction. Flagged for a separate arc.
    let ovl4 = core::ptr::read_volatile(&(*e.qh).overlay[4]);
    let ovl5 = core::ptr::read_volatile(&(*e.qh).overlay[5]);
    let qtok = core::ptr::read_volatile(&(*e.qtd).token);
    let fl0 = core::ptr::read_volatile(fl);
    let qh_phys = phys_of(e.qh as *const Qh, 32).unwrap_or(0);

    // The token the CONTROLLER is executing, per the mode this controller settled into.
    let live = if om { ovl2 } else { qtok };
    let addr = chars & 0x7F;
    let epn = (chars >> 8) & 0xF;
    let kind = match e.layout {
        Some(l) if l.vendor_mt => "vendor-mt",
        Some(l) if l.relative => "rptr-rel",
        Some(_) => "rptr-abs",
        None if e.is_kbd => "kbd",
        None if e.is_rel_mouse => "boot-mouse",
        None => "unknown",
    };

    // 1/7 — the header. NO VERDICT WORD: `NO-COMPLETIONS` is a fact, and `class=` distinguishes
    // "never completed anything" from "completed, then stopped" WITHOUT asserting which of those
    // is a fault. `class=never-completed` is the shared picture of an idle keyboard and of the s58
    // recurrence — see WHAT IT CAN AND CANNOT CONVICT in the section comment. Do not read this
    // line alone as a recurrence; the evidence is on lines 2..7.
    //
    // `class=` keys off the same `kbdwit_last_ms == 0` sentinel `since` does above, and inherits
    // its one ambiguity: a completion stamped at `arch::ms() == 0` would be indistinguishable from
    // "never completed". Unreachable here — enumeration alone is ~1.8 s in before an endpoint can
    // complete anything — but `class=` PRINTS the sentinel as a verdict where `since` only branches
    // on it, so it is written down rather than left implicit.
    serial_println!(
        ":: KBDWIT: [{}] ep=IN{} addr={} kind={} NO-COMPLETIONS class={} quiet={}ms armed_ms={} last_ms={} now_ms={} reports={} toggle={} dead={} == witness ::",
        idx, epn, addr, kind,
        if e.kbdwit_last_ms == 0 { "never-completed" } else { "went-quiet" },
        now.wrapping_sub(since),
        e.kbdwit_armed_ms, e.kbdwit_last_ms, now,
        e.reports, e.toggle as u8, e.dead as u8,
    );
    // 2/7 — the token, raw, from all three vantage points (see the honesty note on `seen=`).
    // `qtd_driven=0` marks `qtd_tok` as NOT DRIVEN: on the overlay-direct path the controller is
    // never given that qTD, so the word is untouched zeroed pool memory and must not be decoded —
    // it would read `active=0 halted=0`, i.e. "completed cleanly", contradicting line 1.
    serial_println!(
        ":: KBDWIT: [{}] ep=IN{} path={} seen={:#010x} ovl_tok={:#010x} qtd_tok={:#010x} qtd_driven={} live={:#010x} == witness ::",
        idx, epn,
        if om { "overlay-direct" } else { "qtd-chain" },
        seen, ovl2, qtok, (!om) as u8, live,
    );
    // 3/7 — the live token decoded. `rem` is Total Bytes To Transfer still outstanding: equal to
    // the armed total means not one byte moved. `tog` is the qTD Data Toggle (token bit 31) — so
    // named, not `dt`, because line 7 already reports an elapsed-milliseconds field and one
    // `awk '/dt=/'` must not match two unrelated quantities in the same dump.
    serial_println!(
        ":: KBDWIT: [{}] ep=IN{} tok active={} halted={} dbuf={} babble={} xact={} missed={} split={} ping={} pid={} cerr={} ioc={} tog={} rem={} == witness ::",
        idx, epn,
        (live >> 7) & 1, (live >> 6) & 1, (live >> 5) & 1, (live >> 4) & 1,
        (live >> 3) & 1, (live >> 2) & 1, (live >> 1) & 1, live & 1,
        kbdwit_pid(live), (live >> 10) & 3, (live >> 15) & 1, (live >> 31) & 1,
        (live >> 16) & 0x7FFF,
    );
    // 4/7 — the queue head verbatim. `ovl4` is split progress on THIS transfer; `ovl5` is residue
    // the driver never clears on the overlay-direct path — read the block above before using it.
    serial_println!(
        ":: KBDWIT: [{}] ep=IN{} qh={:#010x} chars={:#010x} caps={:#010x} horiz={:#010x} cur={:#010x} ovl0={:#010x} ovl1={:#010x} ovl3={:#010x} ovl4={:#010x} ovl5={:#010x} == witness ::",
        idx, epn, qh_phys, chars, caps, horiz, cur, ovl0, ovl1, ovl3, ovl4, ovl5,
    );
    // 5/7 — identity + addressing decoded from the controller's OWN words, plus the linkage check:
    // `fl0` is frame-list entry 0, the head of the periodic chain the controller walks. If neither
    // it nor any `horiz` in that chain reaches `qh`, this endpoint is orphaned from the schedule
    // and no amount of healthy controller state could ever have completed it.
    serial_println!(
        ":: KBDWIT: [{}] ep=IN{} mps={} eps={} dtc={} smask={:#04x} cmask={:#04x} tt=hub{}:port{} mult={} buf_phys={:#010x} qtd_phys={:#010x} fl0={:#010x} == witness ::",
        idx, epn,
        (chars >> 16) & 0x7FF, kbdwit_eps(chars), (chars >> 14) & 1,
        caps & 0xFF, (caps >> 8) & 0xFF, (caps >> 16) & 0x7F, (caps >> 23) & 0x7F,
        (caps >> 30) & 3,
        e.buf_phys, e.qtd_phys, fl0,
    );
    // 6/7 — controller state. `ok=0` means the MMIO read itself failed and the hex is a
    // placeholder, NOT a set of clear status bits.
    serial_println!(
        ":: KBDWIT: [{}] ep=IN{} usbsts={:#010x} usbcmd={:#010x} ok={} hch={} hse={} pss={} ass={} rs={} pse={} ase={} == witness ::",
        idx, epn,
        sts.unwrap_or(0), cmd.unwrap_or(0),
        (sts.is_some() && cmd.is_some()) as u8,
        (sts.unwrap_or(0) & STS_HCHALTED != 0) as u8,
        (sts.unwrap_or(0) & STS_HSE != 0) as u8,
        (sts.unwrap_or(0) & STS_PSS != 0) as u8,
        (sts.unwrap_or(0) & KBDWIT_STS_ASS != 0) as u8,
        (cmd.unwrap_or(0) & CMD_RS != 0) as u8,
        (cmd.unwrap_or(0) & CMD_PSE != 0) as u8,
        (cmd.unwrap_or(0) & CMD_ASE != 0) as u8,
    );
    // 7/7 — is the periodic schedule advancing? THE TELL IS `adv`, measured from the arm-time
    // baseline across the whole >= KBDWIT_QUIET_MS silence window:
    //
    //     adv=0x0000  the frame counter has not moved in seconds — the schedule is FROZEN.
    //     adv!=0      it moved. The MAGNITUDE IS NOT A RATE: FRINDEX is 14 bits and wraps every
    //                 2.048 s while this window spans several wraps, so `adv` is `(fire - arm)`
    //                 modulo 0x4000 and nothing more.
    //
    // WHICH WAY THIS FIELD CAN LIE — and it is the opposite of the usual worry. A stopped counter
    // gives `arm == fire`, hence `adv = 0` unconditionally, so `adv != 0` is unfalsifiable proof of
    // movement: there is NO false-clean mode. The only error is a false FROZEN — a healthy counter
    // whose advance across the window happens to land on an exact multiple of 16384 microframes
    // reads `adv=0x0000`. That is a FALSE ALARM (1 in 16384), never a missed fault. So `adv=0x0000`
    // is worth a second boot before it is worth a conviction; `adv!=0` needs no corroboration.
    //
    // `post` is a second sample taken after lines 1..6 have been emitted. It is NOT the tell and
    // must not be read as one: on this rig printing costs tens of microseconds against FRINDEX's
    // 125 us tick, so `post == fire` and `post_ms=0` are the NORMAL healthy result. It is retained
    // only because it costs nothing and does resolve on a platform whose console is genuinely slow.
    //
    // The two flags are SPLIT along exactly that line, so the non-load-bearing sample can never
    // discredit the load-bearing one: `ok=` covers `arm` and `fire` — i.e. it guards `adv`, and
    // `ok=0` means `adv` is a placeholder, not a measurement — while `post_ok=` covers `post`
    // alone. A single `ok=` over all three would let a failed `post` read stamp "placeholder" on a
    // perfectly valid `adv`, discarding the tell on account of the field just declared not to be it.
    let fr_post = mmio_read32(op + KBDWIT_OP_FRINDEX);
    let t_b = crate::arch::ms();
    let adv = match (e.kbdwit_armed_frindex, fr_a) {
        (Some(arm), Some(fire)) => fire.wrapping_sub(arm) & 0x3FFF,
        _ => 0,
    };
    serial_println!(
        ":: KBDWIT: [{}] ep=IN{} frindex arm={:#06x} fire={:#06x} post={:#06x} ok={} post_ok={} adv={:#06x} post_ms={} == witness ::",
        idx, epn,
        e.kbdwit_armed_frindex.unwrap_or(0) & 0x3FFF,
        fr_a.unwrap_or(0) & 0x3FFF,
        fr_post.unwrap_or(0) & 0x3FFF,
        (e.kbdwit_armed_frindex.is_some() && fr_a.is_some()) as u8,
        fr_post.is_some() as u8,
        adv,
        t_b.wrapping_sub(t_a),
    );
}

/// Boot-keyboard report decode — the same layout, scancode table, and Event delivery as the
/// xHCI keyboard path (xhci mod.rs event dispatch), so a key is a key whichever controller
/// carried it. Table shared via pub(crate) rather than duplicated.
unsafe fn decode_boot_keyboard(report: &[u8]) {
    if report.len() < 3 {
        return;
    }
    let shift = report[0] & 0x22 != 0; // L-Shift (bit 1) or R-Shift (bit 5)
    for &keycode in report.iter().skip(2) {
        if keycode <= 1 {
            continue; // no key / ErrorRollOver
        }
        if (keycode as usize) < super::xhci::HID_SCANCODE_TO_ASCII.len() {
            let (unshifted, shifted) = super::xhci::HID_SCANCODE_TO_ASCII[keycode as usize];
            let ascii = if shift { shifted } else { unshifted };
            if ascii != 0 {
                serial_println!("EHCI-HID: KEY: '{}' (scancode {:#x})", ascii as char, keycode);
                crate::pal::push_event(crate::pal::Event::Key(ascii));
            }
        }
    }
}

// ======================================================================================
// M2 — HID report-descriptor parsing for the trackpad (report-protocol pointer) path.
// A DELIBERATELY minimal parser: it walks the short-item stream (HID 1.11 §6.2.2.2) tracking
// only the state needed to place a pointer report's X/Y axes, button bitfield, optional Report
// ID, and a Digitizer contact-count field. Long items (0xFE) and anything that is not a Generic
// Desktop X/Y / Button / Digitizer-count Input field are skipped — this is not a general HID
// stack, it is exactly enough to move a cursor from what a mouse / tablet / trackpad reports.
// ======================================================================================

/// HID usage pages we key on.
const UP_GENERIC_DESKTOP: u16 = 0x01;
const UP_BUTTON: u16 = 0x09;
const UP_DIGITIZER: u16 = 0x0D;
/// EHCI-5: the Apple vendor-defined usage page. The 2012 rMBP internal trackpad's interface 1 is
/// Apple **vendor multitouch** (Report ID 0x44, usage page 0xFF00) — an opaque Input blob with no
/// standard Generic Desktop X/Y, so `parse_report_descriptor`'s X/Y gate correctly finds nothing.
/// We recognize this page as the multitouch signature and decode its first finger by HYPOTHESIS.
const UP_VENDOR: u16 = 0xFF00;

/// Least opaque-vendor Input size (bits) that counts as a real multitouch blob rather than a
/// stray one-byte vendor field — 8 bytes. Below this we do NOT claim the interface is a trackpad.
const VMT_MIN_VENDOR_BITS: u32 = 64;

// EHCI-TRACKPAD M1 — the Apple "Wellspring" vendor mode switch (CONFIRMED AT METAL 2026-07-18).
// The Apple internal trackpad (05ac:0262) enumerates and ARMS, but its vendor interface stays
// SILENT until this class feature-report handshake flips it out of the single-touch
// compatibility mode into the vendor stream.
//
// PROVENANCE (cleanroom — UnaOS is GPL-3.0-or-later and the Linux bcm5974 driver is GPLv2-only,
// so NO Linux driver code is copied or paraphrased here; only protocol facts are used):
//   * 0x01 GET_REPORT / 0x09 SET_REPORT / wValue 0x0300 (report type 3 = Feature, report id 0)
//     are verbatim USB HID Class Definition 1.11 §7.2 values — open specification, not driver code.
//   * The 8-byte feature-report length and the NORMAL mode byte 0x08 are OUR OWN metal
//     observation: the GET_REPORT witness on this exact 0262 returned got=8b byte0=0x08.
//   * The VENDOR mode byte 0x01 is confirmed by OUR OWN metal result — after writing it, the
//     endpoint streamed 736+ reports where it had previously been silent (see doc §10e).
//   * wIndex 0 is likewise confirmed by that same successful sitting. The interface number is
//     logged alongside so a future sitting can retry with `intf` if index 0 ever STALLs.
const BCM5974_MODE_READ_REQ: u8 = 0x01; // HID class GET_REPORT
const BCM5974_MODE_WRITE_REQ: u8 = 0x09; // HID class SET_REPORT
const BCM5974_MODE_REQ_VALUE: u16 = 0x0300; // wValue: report type 3 (Feature), report id 0
const BCM5974_MODE_REQ_INDEX: u16 = 0x0000; // wIndex: bcm5974 REQUEST_INDEX (NOT the intf number)
const BCM5974_MODE_LEN: u16 = 8; // feature report length
const BCM5974_MODE_VENDOR: u8 = 0x01; // byte 0 = raw multitouch (wellspring) mode
                                      // (0x08 would be the NORMAL single-touch compatibility mode)

// MT-INVESTIGATION (IVY, 2026-07-25) — the NORMAL/HID-mode selector, and the ordered switch
// sequence the raw probe below uses.
//
// PROVENANCE (cleanroom): FreeBSD `sys/dev/usb/input/wsp.c`, the Wellspring touchpad driver, is
// **SPDX-License-Identifier: BSD-2-Clause** (Copyright (c) 2012 Huang Wen Hui) — permissively
// licensed and therefore a lawful reference for a GPL-3.0-or-later kernel. Only PROTOCOL FACTS are
// taken from it; no code is copied. The Linux `bcm5974` driver (GPLv2-only, incompatible with our
// GPL-3.0-or-later) was NOT consulted. Facts used, all from wsp.c's TYPE2 parameter block, which is
// the generation covering the 2012 retina MacBook Pro (Wellspring 7, 05ac:0262):
//   * feature-report size 8, request index 0, switch byte index 0 — matches what this driver
//     already sends and what our own metal GET_REPORT observed.
//   * raw/sensor-mode ON selector = 0x01; HID/normal-mode OFF selector = 0x08.
//   * the mode is set to the OFF value FIRST and only then to the ON value, with a pause between
//     reading the current report and writing the new one. Our current single write skips both.
//   * the raw stream is a BARE header+fingers packet with NO leading HID Report ID byte; TYPE2's
//     header (offset to finger[0]) is 30 B and each finger record is 28 B, so a legal raw frame
//     length satisfies `len >= 30 + 28` and `(len - 30) % 28 == 0`.
//   * the driver's receive buffer is 1024 B — i.e. raw frames are far larger than the 64 B our
//     interrupt buffer holds, so a raw frame will arrive TRUNCATED here until the buffer grows.
// Our own metal observation adds the negative: the 8-byte Report-ID-0x02 packets we currently
// stream are the HID-mode shape, not the raw shape — consistent with the mode switch never having
// taken effect on this device.
#[cfg(feature = "mtraw")]
const BCM5974_MODE_NORMAL: u8 = 0x08;
/// MT-INVESTIGATION: how many reports the raw probe hex-dumps before restoring pointer mode.
/// Deliberately tiny — the FTDI console is a 64 KiB drop-oldest ring, and a Wellspring endpoint
/// under a resting hand streams at ~100 reports/s; four dumps is a capture, forty is a flood that
/// evicts the boot log that gives them context.
#[cfg(feature = "mtraw")]
const MT_RAW_DUMP_MAX: u32 = 4;
/// MT-INVESTIGATION: hard cap on bytes hex-dumped per report. Stays 64 even though the knob-on
/// receive buffer is 1024 B: the point of the hex dump is the frame's HEAD (header + finger[0]),
/// and a full 1024-byte line would evict the boot log from the 64 KiB drop-oldest FTDI ring. The
/// `len=` field on the dump line still reports the TRUE received length, and the decode witness
/// below reports the finger data from the whole frame.
#[cfg(feature = "mtraw")]
const MT_RAW_DUMP_BYTES: usize = 64;

// ---------------------------------------------------------------------------------------------
// MT-INVESTIGATION (IVY) — Wellspring TYPE2 RAW FRAME LAYOUT.
//
// PROVENANCE (cleanroom): FreeBSD `sys/dev/usb/input/wsp.c`, SPDX-License-Identifier
// BSD-2-Clause, Copyright (c) 2012 Huang Wen Hui — permissively licensed, so lawful to take
// protocol facts from for this GPL-3.0-or-later kernel. NO code is copied; only the numbers and
// the validation rule below, each re-derived from the named declaration. The Linux `bcm5974`
// driver is GPLv2-only (incompatible with GPL-3.0-or-later) and was NOT consulted.
//
// wsp.c declarations relied on, and what each gives us:
//   * `#define FINGER_TYPE2 (15 * 2)`      -> 30 bytes of header before finger[0].
//   * `#define FSIZE_TYPE2  (14 * 2)`      -> 28 bytes per finger record.
//   * `wsp_tp[TYPE2] = { .offset = FINGER_TYPE2, .fsize = FSIZE_TYPE2, .delta = 0, ... }`
//                                          -> no extra delta between header and finger[0]
//                                             (TYPE4 has `.delta = 2`; TYPE2 does not).
//   * `#define BUTTON_TYPE2 15`            -> the integrated-button byte is header offset 15,
//     and `wsp_intr_callback` reads the finger COUNT at `params->tp->button - 1`, i.e. offset 14.
//   * `struct tp_header` (packed, LE): flag@0, sn0@1, wFixed0@2, dwSn1@4, dwFixed1@8,
//     wLength@12, nfinger@14, ibt@15, wUnknown[6]@16, q1@28, q2@29 — 30 bytes, which is exactly
//     `FINGER_TYPE2` and independently confirms both the header size and the nfinger/ibt offsets.
//   * `struct tp_finger` (packed, LE, all int16): origin@0, abs_x@2, abs_y@4, rel_x@6, rel_y@8,
//     tool_major@10, tool_minor@12, orientation@14, touch_major@16, touch_minor@18,
//     unused[2]@20, pressure@24, multi@26 — 28 bytes, exactly `FSIZE_TYPE2`.
//   * `wsp_intr_callback` length gate:
//         len >= offset + fsize  AND  (len - offset) % fsize == 0
//     i.e. at least one whole finger record and no partial trailing record.
//   * `#define MAX_FINGERS 16` with `ntouch` range-checked to `[0, MAX_FINGERS]` — the clamp we
//     mirror, plus our own additional clamp to the number of records the frame actually carries.
//   * `#define WSP_BUFFER_MAX 1024` — the receive-buffer size (see `qh::INT_BUF_LEN`).
//   * finger presence: wsp treats `f->touch_major != 0` as the finger being in contact.
//   * `sc->pos_y[i] = -f->abs_y` — wsp NEGATES Y for its pointer path (the sensor's Y grows the
//     opposite way from screen Y). We report `abs_y` VERBATIM in the witness (so the metal capture
//     is raw ground truth) and apply the negation only in the opt-in injection path below.
//
// The raw frame carries NO leading HID Report ID byte — offsets here are from byte 0 of the frame.
/// Bytes of header before finger[0] (`FINGER_TYPE2`, corroborated by `struct tp_header`'s size).
#[cfg(feature = "mtraw")]
const WSP2_HDR_LEN: usize = 30;
/// Bytes per finger record (`FSIZE_TYPE2`, corroborated by `struct tp_finger`'s size).
#[cfg(feature = "mtraw")]
const WSP2_FSIZE: usize = 28;
/// Header offset of the finger count (`tp_header.nfinger`, = `BUTTON_TYPE2 - 1`).
#[cfg(feature = "mtraw")]
const WSP2_NFINGER_OFF: usize = 14;
/// Header offset of the integrated-button byte (`tp_header.ibt`, = `BUTTON_TYPE2`).
#[cfg(feature = "mtraw")]
const WSP2_BUTTON_OFF: usize = 15;
/// Finger-record offset of `abs_x` (int16 LE).
#[cfg(feature = "mtraw")]
const WSP2_F_ABS_X: usize = 2;
/// Finger-record offset of `abs_y` (int16 LE).
#[cfg(feature = "mtraw")]
const WSP2_F_ABS_Y: usize = 4;
/// Finger-record offset of `touch_major` (int16 LE); non-zero == the finger is in contact.
#[cfg(feature = "mtraw")]
const WSP2_F_TOUCH_MAJOR: usize = 16;
/// Hard clamp on decoded fingers (`MAX_FINGERS`). Hostile/garbled input can put anything in the
/// count byte; the decoder additionally clamps to the records the frame's LENGTH can hold, so the
/// two together make an out-of-bounds read unreachable.
#[cfg(feature = "mtraw")]
const WSP2_MAX_FINGERS: usize = 16;

// EHCI-5 vendor-multitouch decode HYPOTHESIS (bcm5974 TYPE2 lead — CONFIRM AT METAL).
// The Apple 0x44 report is opaque: its HID descriptor gives the total report size, not which
// bytes are the finger's X/Y. These are BYTE offsets into the report BODY (after the leading
// 0x44 Report ID byte is stripped) where the FIRST finger's fields are HYPOTHESIZED to sit,
// following the public bcm5974 TYPE2 finger record (abs_x@+2, abs_y@+4 signed, touch_major@+16,
// pressure@+24; a ~30 B header precedes finger[0]). bcm5974 reads a SEPARATE raw interface with
// NO Report ID, so the header length and finger layout here are UNCONFIRMED — the attended
// sitting's raw-byte capture (dump_vendor_report) confirms or corrects EXACTLY these lines.
const VMT_HDR_LEN: usize = 30; // HYPOTHESIS: bytes before finger[0]
const VMT_FINGER_ABS_X: usize = VMT_HDR_LEN + 2; // HYPOTHESIS: signed le16
const VMT_FINGER_ABS_Y: usize = VMT_HDR_LEN + 4; // HYPOTHESIS: signed le16
const VMT_FINGER_TOUCH: usize = VMT_HDR_LEN + 16; // HYPOTHESIS: touch_major (>0 == finger present)

/// Hard cap on the per-Input-item field-loop trip count (HARDENING — the driver is default-ON, so
/// this parser runs on ANY plugged USB device's descriptor). `Report Count` is a global item read
/// VERBATIM from the descriptor and its 4-byte form (0x97) can carry up to 0xFFFF_FFFF; an
/// unclamped `for j in 0..report_count` lets an ~11-byte hostile descriptor drive a multi-billion-
/// iteration loop during enumeration → boot stall (DoS). A report is delivered into the 64-byte
/// interrupt buffer (`Buf64` = 512 bits), so a *legitimate* Input item packs at most 512 one-bit
/// fields; anything larger cannot fit a real report and is a malformed/hostile descriptor. We cap
/// the loop (and the bit-offset advance) at this bound — the field map for a real pointer is
/// unaffected, a hostile count is clamped and the device is decoded on what actually fits (or
/// skipped as non-pointer). Mirrors the existing `report_count.min(32)` button clamp.
const MAX_REPORT_FIELDS: u32 = 512;

/// Read `size` bits little-endian (LSB-first, HID packing) starting at bit `off` from `data`.
fn extract_bits(data: &[u8], off: u16, size: u8) -> u32 {
    let mut v = 0u32;
    for i in 0..(size as u16).min(32) {
        let bit = off + i;
        let byte = (bit / 8) as usize;
        if byte >= data.len() {
            break;
        }
        if data[byte] & (1 << (bit % 8)) != 0 {
            v |= 1 << i;
        }
    }
    v
}

/// Sign-extend a `size`-bit field to i32 (for a Relative axis; Absolute axes stay unsigned).
fn sign_extend(v: u32, size: u8) -> i32 {
    if size > 0 && size < 32 && v & (1 << (size - 1)) != 0 {
        (v | (!0u32 << size)) as i32
    } else {
        v as i32
    }
}

/// Parse a HID report descriptor into the pointer field map (`ReportLayout`). Returns `None` if
/// no variable X/Y field is present (i.e. not a cursor device). Fields past whatever bytes we were
/// handed (the 256-byte control-read cap) are simply not seen — a truncated tail ends the walk
/// cleanly. Single-report assumption: on a new Report ID the body bit-offset restarts (a
/// multi-report multitouch descriptor decodes its first pointer report; deeper multitouch is a
/// metal follow-up, flagged in usb_xhci.md §10b).
unsafe fn parse_report_descriptor(desc: &[u8]) -> Option<ReportLayout> {
    let mut l = ReportLayout::default();
    let mut usage_page = 0u16;
    let mut report_size = 0u32;
    let mut report_count = 0u32;
    // Local usages queued for the next Main item (we only ever need a handful).
    let mut usages: [u16; 16] = [0; 16];
    let mut nusg = 0usize;
    let mut usage_min = 0u16;
    let mut usage_max = 0u16;
    let mut bit_off: u16 = 0;
    // EHCI-5: track the Apple vendor-multitouch signature — an opaque variable Input on the
    // vendor usage page (0xFF00) of non-trivial size. Recognized only if the standard X/Y gate
    // below finds nothing (so a real pointer is never diverted to the vendor path).
    let mut saw_vendor_input = false;
    let mut vendor_bits: u32 = 0;
    let mut i = 0usize;
    while i < desc.len() {
        let b = desc[i];
        if b == 0xFE {
            break; // long item — not used by these devices
        }
        let size = match b & 0x03 {
            3 => 4,
            s => s as usize,
        };
        if i + 1 + size > desc.len() {
            break; // truncated (the read cap) — stop cleanly
        }
        let mut data: u32 = 0;
        for k in 0..size {
            data |= (desc[i + 1 + k] as u32) << (8 * k);
        }
        match b & 0xFC {
            // ---- Global items ----
            0x04 => usage_page = data as u16,                 // Usage Page
            0x74 => report_size = data,                       // Report Size
            0x94 => report_count = data,                      // Report Count
            0x84 => {
                l.report_id = data as u8; // Report ID — a report body follows the ID byte
                bit_off = 0;
            }
            // ---- Local items ----
            0x08 => {
                if nusg < usages.len() {
                    usages[nusg] = data as u16;
                    nusg += 1;
                }
            }
            0x18 => usage_min = data as u16, // Usage Minimum
            0x28 => usage_max = data as u16, // Usage Maximum
            // ---- Main: Input ----
            0x80 => {
                let is_const = data & 0x01 != 0; // Constant (padding) — reserve space, no field
                let is_var = data & 0x02 != 0;
                let is_rel = data & 0x04 != 0;
                // HARDENING: clamp the field-loop trip count against a hostile Report Count (see
                // MAX_REPORT_FIELDS) — an unclamped 0..report_count is a plug-in DoS on the
                // default-ON driver. The bit-offset advance below uses the same clamped count.
                let count = report_count.min(MAX_REPORT_FIELDS);
                if !is_const && is_var {
                    for j in 0..count {
                        let usage = if (j as usize) < nusg {
                            usages[j as usize]
                        } else if nusg > 0 {
                            usages[nusg - 1]
                        } else if usage_max >= usage_min {
                            usage_min.wrapping_add(j as u16)
                        } else {
                            0
                        };
                        let f_off = bit_off + (j as u16) * (report_size as u16);
                        match (usage_page, usage) {
                            (UP_GENERIC_DESKTOP, 0x30) => {
                                l.has_xy = true;
                                l.relative = is_rel;
                                l.x_off = f_off;
                                l.x_size = report_size as u8;
                            }
                            (UP_GENERIC_DESKTOP, 0x31) => {
                                l.y_off = f_off;
                                l.y_size = report_size as u8;
                            }
                            (UP_DIGITIZER, 0x54) => {
                                // Contact Count (finger count) — witness only.
                                l.finger_off = f_off;
                                l.finger_size = report_size as u8;
                            }
                            _ => {}
                        }
                    }
                    // Buttons: a variable Input on the Button page, one bit per button.
                    if usage_page == UP_BUTTON && l.btn_count == 0 {
                        l.btn_off = bit_off;
                        l.btn_count = report_count.min(32) as u8;
                    }
                }
                // EHCI-5-fix: the Apple vendor-multitouch Input on the real 05ac:0262 intf1 is
                // declared as an ARRAY (`81 00`), not a Variable — so the field-mapping block above
                // (gated on `is_var`) never sees it, and the interface was mis-skipped as
                // "no X/Y → not a cursor device". Recognize the vendor-page (0xFF00) signature for
                // ANY non-Constant Input (Array OR Variable) so the endpoint is ARMED, not skipped.
                // A Constant (padding) item is excluded; the standard X/Y gate still wins first, so
                // no real pointer is ever diverted onto the vendor path.
                if !is_const && usage_page == UP_VENDOR {
                    saw_vendor_input = true;
                    vendor_bits = vendor_bits.saturating_add(report_size.saturating_mul(count));
                }
                // Advance by the CLAMPED count (and saturate the u16) so a hostile Report Count
                // neither loops nor overflows the running bit offset.
                let advance = (report_size.min(u16::MAX as u32) as u16)
                    .saturating_mul(count.min(u16::MAX as u32) as u16);
                bit_off = bit_off.saturating_add(advance);
                l.total_bits = l.total_bits.max(bit_off);
                // Local state is cleared after every Main item (HID 1.11 §6.2.2.8).
                nusg = 0;
                usage_min = 0;
                usage_max = 0;
            }
            // Output / Feature main items also clear locals and advance nothing we track.
            0x90 | 0xB0 => {
                nusg = 0;
                usage_min = 0;
                usage_max = 0;
            }
            _ => {}
        }
        i += 1 + size;
    }
    if l.has_xy && l.x_size > 0 && l.y_size > 0 {
        Some(l)
    } else if saw_vendor_input && l.report_id != 0 && vendor_bits >= VMT_MIN_VENDOR_BITS {
        // EHCI-5: no standard pointer field, but the Apple vendor-multitouch signature is present
        // (a Report ID + an opaque variable Input on page 0xFF00, non-trivial size). Recognize it
        // so the endpoint is ARMED (not skipped) and the service loop can capture + decode the
        // first finger at the HYPOTHESIS offsets. `has_xy` is false here, so no standard pointer is
        // ever diverted onto this path.
        l.vendor_mt = true;
        Some(l)
    } else {
        None
    }
}

/// Decode one report through a parsed `ReportLayout`. Returns (x, y, buttons, fingers): X/Y are
/// sign-extended for a Relative layout (a mouse's deltas) and unsigned for Absolute (a tablet /
/// trackpad's coordinates). A report whose leading ID byte does not match this layout's Report ID
/// is ignored (returns zeros).
fn decode_report_pointer(report: &[u8], l: &ReportLayout) -> (i32, i32, u8, u8) {
    let body: &[u8] = if l.report_id != 0 {
        if report.is_empty() || report[0] != l.report_id {
            return (0, 0, 0, 0);
        }
        &report[1..]
    } else {
        report
    };
    let x_raw = extract_bits(body, l.x_off, l.x_size);
    let y_raw = extract_bits(body, l.y_off, l.y_size);
    let (x, y) = if l.relative {
        (sign_extend(x_raw, l.x_size), sign_extend(y_raw, l.y_size))
    } else {
        (x_raw as i32, y_raw as i32)
    };
    let buttons = if l.btn_count > 0 {
        extract_bits(body, l.btn_off, l.btn_count) as u8
    } else {
        0
    };
    let fingers = if l.finger_size > 0 {
        extract_bits(body, l.finger_off, l.finger_size) as u8
    } else {
        0
    };
    (x, y, buttons, fingers)
}

/// Read a little-endian `u16` at BYTE offset `off`, or `None` if the two bytes are not fully
/// present — a short/malformed report can never read out of bounds.
fn read_le16(data: &[u8], off: usize) -> Option<u16> {
    let b = data.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// RMBP-FIX (2026-07-18): the LIVE trackpad decoder. After the bcm5974 mode switch, the internal
/// trackpad on this device path streams 8-byte **Report ID 0x02** relative reports (characterized on
/// silicon — 736+ reports observed): `[0]` = report id (0x02), `[1]` = buttons (`0x00` up / `0x01`
/// down, confirmed via a held press), `[2]` = dx int8, `[3]` = dy int8, `[4..=5]` zero, `[6..=7]`
/// unknown. Returns `(buttons, dx, dy)` with dx/dy sign-extended from int8, or `None` if the report
/// is shorter than 4 bytes OR its id byte is not `0x02` (tolerant: a stray/other-format report is
/// ignored — no event, no state change). This SUPERSEDES the refuted `decode_vendor_first_finger`
/// 0x44/multitouch hypothesis below.
const TRACKPAD_REPORT_ID: u8 = 0x02;
fn decode_trackpad_rel(report: &[u8]) -> Option<(u8, i32, i32)> {
    if report.len() < 4 || report[0] != TRACKPAD_REPORT_ID {
        return None;
    }
    let buttons = report[1];
    let dx = report[2] as i8 as i32;
    let dy = report[3] as i8 as i32;
    Some((buttons, dx, dy))
}

/// MT-INVESTIGATION (IVY) — what `decode_wellspring_type2` extracts from one raw TYPE2 frame.
/// Only the fields the arc actually needs: the frame-level count/button, and the FIRST finger's
/// position + contact state (deeper multitouch is a later arc; the decoder validates the whole
/// frame's shape either way, so it is a per-record loop away).
#[cfg(feature = "mtraw")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Wsp2Frame {
    /// Fingers reported, clamped to `WSP2_MAX_FINGERS` AND to the records the frame can hold.
    fingers: u8,
    /// `tp_header.ibt` — the integrated-button byte (this pad's click lives in the trackpad).
    button: u8,
    /// finger[0] `abs_x` (int16 LE, sign-extended). Zero when `fingers == 0`.
    x0: i32,
    /// finger[0] `abs_y` VERBATIM (int16 LE, sign-extended; wsp negates this for its pointer path
    /// — we do not, so the witness prints sensor ground truth). Zero when `fingers == 0`.
    y0: i32,
    /// finger[0] `touch_major` (int16 LE, sign-extended); non-zero == in contact.
    touch0: i32,
}

/// MT-INVESTIGATION (IVY) — decode one Apple Wellspring **TYPE2 raw multitouch frame**.
///
/// See the `WSP2_*` PROVENANCE block for the cleanroom source (FreeBSD wsp.c, BSD-2-Clause) and
/// the per-field citations. This function is TOTAL and HOSTILE-INPUT-SAFE — the interrupt endpoint
/// hands it whatever the device (or a malfunctioning/malicious device) put on the wire:
///
///   * `None` unless the length passes wsp's own gate — at least a header plus one whole finger
///     record, and no partial trailing record. This is also what rejects the ordinary 8-byte
///     Report-ID-0x02 HID-mode report, so a pad that never left HID mode decodes to `None` rather
///     than to garbage finger data.
///   * the finger count is read from the frame but never trusted: it is clamped to
///     `WSP2_MAX_FINGERS` and, independently, to the number of records the frame's LENGTH can
///     hold — so it can only ever name records that exist.
///   * every field read goes through `read_le16`, which returns `None` rather than reading past
///     the end. There is no indexing that a short frame could drive out of bounds.
///
/// Emits NO events and mutates NO state — decode + witness only (see `mt_inject_first_finger` for
/// the opt-in pointer path).
#[cfg(feature = "mtraw")]
fn decode_wellspring_type2(frame: &[u8]) -> Option<Wsp2Frame> {
    // wsp_intr_callback's gate, verbatim in intent: one whole finger record minimum, and the
    // post-header remainder must divide evenly into finger records.
    if frame.len() < WSP2_HDR_LEN + WSP2_FSIZE {
        return None;
    }
    if (frame.len() - WSP2_HDR_LEN) % WSP2_FSIZE != 0 {
        return None;
    }
    // Records the frame can actually hold — the clamp that makes the count byte harmless.
    let records = (frame.len() - WSP2_HDR_LEN) / WSP2_FSIZE;
    // Safe: the length gate above guarantees at least `WSP2_HDR_LEN` (30) bytes, and both offsets
    // are < 30. Read through `get` regardless, so the two facts never have to be re-proved.
    let button = *frame.get(WSP2_BUTTON_OFF)?;
    let declared = *frame.get(WSP2_NFINGER_OFF)? as usize;
    let fingers = declared.min(WSP2_MAX_FINGERS).min(records);
    if fingers == 0 {
        // A well-formed frame reporting no fingers: valid, just empty. (All fingers lifted.)
        return Some(Wsp2Frame { fingers: 0, button, x0: 0, y0: 0, touch0: 0 });
    }
    // finger[0] begins immediately after the header (`.delta = 0` for TYPE2).
    let f0 = WSP2_HDR_LEN;
    let x0 = read_le16(frame, f0 + WSP2_F_ABS_X)? as i16 as i32;
    let y0 = read_le16(frame, f0 + WSP2_F_ABS_Y)? as i16 as i32;
    let touch0 = read_le16(frame, f0 + WSP2_F_TOUCH_MAJOR)? as i16 as i32;
    Some(Wsp2Frame { fingers: fingers as u8, button, x0, y0, touch0 })
}

/// MT-INVESTIGATION (IVY) — one bounded witness line per captured frame on the LIVE path. Called
/// only from inside the probe's `MT_RAW_DUMP_MAX` capture window, so it is ring-safe at the
/// endpoint's ~100 reports/s. Prints the not-a-raw-frame case too: on a pad that never left HID
/// mode that line IS the finding.
///
/// `mps` is printed alongside `len` because their relationship is the load-bearing evidence for the
/// buffer growth: `len > mps` means the controller accumulated a MULTI-PACKET frame into the grown
/// buffer (exactly what the pre-arc `total == mps` arming could never produce), while `len == mps`
/// on a raw-shaped frame would mean we are still capped at one packet.
#[cfg(feature = "mtraw")]
fn dump_type2_frame(idx: usize, mps: u16, frame: &[u8]) {
    match decode_wellspring_type2(frame) {
        Some(f) => serial_println!(
            ":: EHCI-MT: [{}] type2 frame len={} mps={} fingers={} x0={} y0={} touch0={} button={:#04x} == witness ::",
            idx, frame.len(), mps, f.fingers, f.x0, f.y0, f.touch0, f.button
        ),
        None => serial_println!(
            ":: EHCI-MT: [{}] type2 frame len={} mps={} fingers=n/a — not a TYPE2 raw frame (needs {}+{}*n bytes; HID-mode shape decodes here) == witness ::",
            idx, frame.len(), mps, WSP2_HDR_LEN, WSP2_FSIZE
        ),
    }
}

/// MT-INVESTIGATION (IVY, `mtraw_inject` sub-knob ONLY — default OFF, and OFF is the shipping
/// behaviour until metal proves raw mode is stable) — drive the pointer from the first finger.
///
/// TYPE2 coordinates are ABSOLUTE sensor units, while the landed EHCI pointer seam is
/// `pal::Event::Mouse { x, y }` RELATIVE deltas, so we difference against the previous frame.
/// `prev` is cleared whenever the finger is absent (`touch_major == 0`, wsp's own contact test) or
/// the frame is not decodable, so a lift-and-replace never emits a jump. Deltas are clamped to a
/// sane per-frame magnitude — a garbled coordinate must not fling the cursor.
#[cfg(feature = "mtraw_inject")]
fn mt_inject_first_finger(frame: &[u8], prev: &mut Option<(i32, i32)>) {
    let Some(f) = decode_wellspring_type2(frame) else {
        *prev = None;
        return;
    };
    if f.fingers == 0 || f.touch0 == 0 {
        *prev = None;
        return;
    }
    // wsp uses `-abs_y` for its pointer path (sensor Y grows opposite to screen Y); apply that
    // ONLY here, so the witness above keeps reporting the raw sensor value.
    let (x, y) = (f.x0, -f.y0);
    if let Some((px, py)) = *prev {
        let dx = (x - px).clamp(-MT_INJECT_MAX_STEP, MT_INJECT_MAX_STEP);
        let dy = (y - py).clamp(-MT_INJECT_MAX_STEP, MT_INJECT_MAX_STEP);
        if dx != 0 || dy != 0 {
            crate::pal::push_event(crate::pal::Event::Mouse { x: dx, y: dy });
        }
    }
    *prev = Some((x, y));
}

/// MT-INVESTIGATION (`mtraw_inject`): per-frame delta clamp. The TYPE2 sensor spans a few thousand
/// units edge to edge and streams at ~100 Hz, so a real swipe moves tens of units per frame; this
/// bound is generous for real motion and hard against a garbled coordinate flinging the cursor.
#[cfg(feature = "mtraw_inject")]
const MT_INJECT_MAX_STEP: i32 = 128;

/// EHCI-5 (REFUTED-HYPOTHESIS HISTORY — kept per the never-trash rule, exercised by
/// `vendor_multitouch_selftest`, NO LONGER the live decode path): decode the FIRST finger of an
/// Apple vendor-multitouch (`0x44`) report at the HYPOTHESIS offsets. The 0x44 / 511-byte
/// multitouch-frame model was REFUTED on the metal rMBP trackpad path (RMBP-FIX, 2026-07-18) — the
/// device streams 8-byte Report ID 0x02 relative reports instead (see `decode_trackpad_rel`). This
/// function and its `VMT_FINGER_*` constants remain as the documented reverse-engineering trail.
/// Returns `(present, abs_x, abs_y)` where `present` is the touch field being non-zero and
/// abs_x/abs_y are the signed le16 finger coordinates. Returns `None` if the report is too short to
/// reach the finger record (every read is bounds-checked — a short/malformed 0x44 report never
/// reads out of bounds or emits garbage motion). Only the first finger is decoded; further finger
/// records are IGNORED (multitouch gestures are out of scope). The leading `report_id` prefix byte
/// is stripped first, mirroring `decode_report_pointer`. The offset VALUES are a metal-verified
/// hypothesis (bcm5974 TYPE2 lead); this function's MECHANICS are exercised by the self-test.
fn decode_vendor_first_finger(report: &[u8], report_id: u8) -> Option<(bool, i32, i32)> {
    let body: &[u8] = if report_id != 0 {
        if report.is_empty() || report[0] != report_id {
            return None;
        }
        &report[1..]
    } else {
        report
    };
    let present = read_le16(body, VMT_FINGER_TOUCH)? != 0;
    let abs_x = read_le16(body, VMT_FINGER_ABS_X)? as i16 as i32;
    let abs_y = read_le16(body, VMT_FINGER_ABS_Y)? as i16 as i32;
    Some((present, abs_x, abs_y))
}

/// Verbatim (bounded) hex dump of a report descriptor — the doc's 0262 capture slot. Prints the
/// first up-to-48 bytes on one serial line (enough to reconstruct the pointer field map); the
/// declared length is stated so a truncated read is obvious.
unsafe fn dump_report_descriptor(idx: usize, addr: u8, intf: u8, report_len: u16, desc: &[u8]) {
    let mut hex = alloc::string::String::new();
    for (k, b) in desc.iter().take(48).enumerate() {
        if k > 0 {
            hex.push(' ');
        }
        let hi = b >> 4;
        let lo = b & 0xF;
        hex.push(char::from_digit(hi as u32, 16).unwrap());
        hex.push(char::from_digit(lo as u32, 16).unwrap());
    }
    serial_println!(
        ":: EHCI-HID: [{}] addr {} intf {} report descriptor ({} of {} B){}: {} ::",
        idx, addr, intf, desc.len(), report_len,
        if desc.len() < report_len as usize { " [truncated at read cap]" } else { "" },
        hex
    );
}

/// EHCI-5: verbatim hex dump of one raw Apple vendor-multitouch (0x44) report body — the attended
/// sitting's reverse-engineering evidence. The `0x44` report is opaque (the HID descriptor does
/// not describe which bytes are the finger's X/Y), so the sitting reads THESE bytes to confirm or
/// correct the `VMT_FINGER_*` HYPOTHESIS offsets. Dumps the whole captured slice (≤ 64 B, one
/// interrupt packet) so the finger record near byte ~30+ is visible. Same hex idiom as
/// `dump_report_descriptor`. RMBP-FIX (2026-07-18): gated to the `usbdebug` build and called only for
/// the first 4 reports per device — the byte characterization is complete, so on a default/GUI build
/// this (heap-allocating) dump is compiled out entirely and never touches the hot path.
#[cfg(feature = "usbdebug")]
fn dump_vendor_report(idx: usize, count: u32, report: &[u8]) {
    let mut hex = alloc::string::String::new();
    for (k, b) in report.iter().enumerate() {
        if k > 0 {
            hex.push(' ');
        }
        hex.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        hex.push(char::from_digit((b & 0xF) as u32, 16).unwrap());
    }
    serial_println!(
        ":: EHCI-HID: [{}] vendor-multitouch raw report #{} ({} B): {} == witness ::",
        idx, count, report.len(), hex
    );
}

/// MT-INVESTIGATION (IVY) — hex-dump one report of the raw-mode capture window.
///
/// Kept separate from `dump_vendor_report` on purpose: that one is the `usbdebug` characterization
/// dump and is already spoken for, while this line carries the `EHCI-MT:` prefix the sitting reads
/// for. The `len=` field is the load-bearing datum — per FreeBSD wsp.c (BSD-2-Clause) a TYPE2 raw
/// frame is `30 + 28*n` bytes with NO Report ID byte, so a length of 8 with a leading 0x02 means
/// the pad is still in HID mode, whereas a length pinned at the endpoint's max packet size means a
/// raw frame arrived and our 64 B interrupt buffer TRUNCATED it.
#[cfg(feature = "mtraw")]
fn dump_raw_report(idx: usize, count: u32, report: &[u8]) {
    let mut hex = alloc::string::String::new();
    for (k, b) in report.iter().enumerate() {
        if k > 0 {
            hex.push(' ');
        }
        hex.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        hex.push(char::from_digit((b & 0xF) as u32, 16).unwrap());
    }
    serial_println!(
        ":: EHCI-MT: [{}] raw-report #{} len={} bytes={} == witness ::",
        idx, count, report.len(), hex
    );
}

/// Parser hardening self-test (runs once at driver init, since the driver is default-ON). Feeds
/// the report parser a HOSTILE descriptor — `Report Count = 0xFFFF_FFFF` (4-byte form 0x97) on an
/// X field — and asserts it returns *bounded* instead of spinning a multi-billion-iteration loop.
/// PRE-FIX this call never returns (the boot hangs); POST-FIX (MAX_REPORT_FIELDS clamp) it returns
/// immediately as `None` (only X, no Y → not a pointer). A legit X/Y descriptor is parsed alongside
/// to prove the clamp does not break the real path. One serial witness line either way.
unsafe fn parser_selftest() {
    // Usage Page(Generic Desktop), Usage(X), Report Size(8), Report Count(0xFFFFFFFF), Input(Var).
    let hostile: [u8; 13] = [
        0x05, 0x01, 0x09, 0x30, 0x75, 0x08, 0x97, 0xFF, 0xFF, 0xFF, 0xFF, 0x81, 0x02,
    ];
    let hostile_bounded = parse_report_descriptor(&hostile).is_none();
    // Usage Page(GD), Usage(X), Usage(Y), Report Size(16), Report Count(2), Input(Var).
    let legit: [u8; 12] = [
        0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x75, 0x10, 0x95, 0x02, 0x81, 0x02,
    ];
    let legit_ok = parse_report_descriptor(&legit)
        .map(|l| l.has_xy && l.x_size == 16 && l.y_size == 16 && l.x_off == 0 && l.y_off == 16)
        .unwrap_or(false);
    serial_println!(
        ":: EHCI-HID: report-parser self-test: hostile report_count clamped (bounded={}, cap={}), legit X/Y parse ok={} == witness ::",
        hostile_bounded, MAX_REPORT_FIELDS, legit_ok
    );
    vendor_multitouch_selftest();
    // MT-INVESTIGATION (IVY, `mtraw` only): the ONLY QEMU-provable witness for the raw TYPE2
    // decoder — QEMU has no Wellspring pad, so a synthetic frame stands in. Compiled out (and so
    // silent) on a default build.
    #[cfg(feature = "mtraw")]
    wellspring_type2_selftest();
}

/// MT-INVESTIGATION (IVY) self-test (runs once at driver init under `mtraw`). Feeds
/// `decode_wellspring_type2` a SYNTHETIC TYPE2 raw frame built at the cited wsp.c offsets — 30-byte
/// header + TWO 28-byte finger records — and asserts the frame-level count and finger[0]'s
/// position/contact come back exactly. Then the hostile cases, because this decoder runs on
/// whatever a device puts on the wire: a lying finger count (0xFF in a frame that holds two
/// records → clamped to 2, never an out-of-bounds read), the ordinary 8-byte HID-mode 0x02 report
/// (rejected by the length gate — must NOT be misread as finger data), a length with a partial
/// trailing record (rejected), and an empty-but-well-formed frame (accepted, zero fingers).
#[cfg(feature = "mtraw")]
unsafe fn wellspring_type2_selftest() {
    // One well-formed 2-finger frame: 30 + 2*28 = 86 bytes.
    const N: usize = WSP2_HDR_LEN + 2 * WSP2_FSIZE;
    let mut frame = [0u8; N];
    frame[WSP2_NFINGER_OFF] = 2;
    frame[WSP2_BUTTON_OFF] = 0x01; // integrated button held
    let f0 = WSP2_HDR_LEN;
    let f1 = WSP2_HDR_LEN + WSP2_FSIZE;
    frame[f0 + WSP2_F_ABS_X..f0 + WSP2_F_ABS_X + 2].copy_from_slice(&1500i16.to_le_bytes());
    frame[f0 + WSP2_F_ABS_Y..f0 + WSP2_F_ABS_Y + 2].copy_from_slice(&(-2000i16).to_le_bytes());
    frame[f0 + WSP2_F_TOUCH_MAJOR..f0 + WSP2_F_TOUCH_MAJOR + 2]
        .copy_from_slice(&90i16.to_le_bytes());
    frame[f1 + WSP2_F_ABS_X..f1 + WSP2_F_ABS_X + 2].copy_from_slice(&(-400i16).to_le_bytes());
    frame[f1 + WSP2_F_ABS_Y..f1 + WSP2_F_ABS_Y + 2].copy_from_slice(&3100i16.to_le_bytes());
    frame[f1 + WSP2_F_TOUCH_MAJOR..f1 + WSP2_F_TOUCH_MAJOR + 2]
        .copy_from_slice(&70i16.to_le_bytes());
    let good = decode_wellspring_type2(&frame);

    // Hostile finger count: the frame holds two records, the count byte claims 255.
    let mut liar = frame;
    liar[WSP2_NFINGER_OFF] = 0xFF;
    let clamped = decode_wellspring_type2(&liar).map(|f| f.fingers) == Some(2);

    // The ordinary HID-mode report — must be REJECTED, not misread.
    let hid = [0x02u8, 0x00, 0x05, 0xFB, 0x00, 0x00, 0x00, 0x00];
    let hid_rejected = decode_wellspring_type2(&hid).is_none();
    // A length with a partial trailing finger record — rejected.
    let ragged = [0u8; WSP2_HDR_LEN + WSP2_FSIZE + 7];
    let ragged_rejected = decode_wellspring_type2(&ragged).is_none();
    // Well-formed, all fingers lifted — accepted with zero fingers, no coordinates.
    let empty = [0u8; WSP2_HDR_LEN + WSP2_FSIZE];
    let empty_ok = decode_wellspring_type2(&empty)
        == Some(Wsp2Frame { fingers: 0, button: 0, x0: 0, y0: 0, touch0: 0 });

    let (fingers, x0, y0, touch0, button) = match good {
        Some(f) => (f.fingers, f.x0, f.y0, f.touch0, f.button),
        None => (0, 0, 0, 0, 0),
    };
    let ok = good == Some(Wsp2Frame { fingers: 2, button: 0x01, x0: 1500, y0: -2000, touch0: 90 })
        && clamped
        && hid_rejected
        && ragged_rejected
        && empty_ok;
    serial_println!(
        ":: EHCI-MT: type2 self-test fingers={} x0={} y0={} touch0={} button={:#04x} ok={} (len={} hdr={} fsize={}; count-clamp={} hid-reject={} ragged-reject={} empty-ok={}) == witness ::",
        fingers, x0, y0, touch0, button, ok, N, WSP2_HDR_LEN, WSP2_FSIZE,
        clamped, hid_rejected, ragged_rejected, empty_ok
    );
}

/// EHCI-5 self-test (runs once at driver init). The ONLY QEMU-provable witness for the vendor
/// path — QEMU has no Apple trackpad, so synthetic Apple-style data stands in. Asserts both
/// milestones: RECOGNITION (M1) — a vendor-page (0xFF00) opaque variable Input with a Report ID
/// and non-trivial size parses to a `vendor_mt` layout (NOT `None`, NOT the standard X/Y path);
/// and DECODE (M2) — two synthetic 0x44 reports (finger A -> B, one negative coordinate) decode
/// to the expected first-finger positions and relative delta, a finger-up report reads absent, and
/// a too-short report decodes to `None` (bounds-safe). This proves the decode MECHANICS; the OFFSET
/// VALUES the layout/decode use remain a metal-verified hypothesis (`VMT_FINGER_*`).
unsafe fn vendor_multitouch_selftest() {
    // M1 recognition. Usage Page(Vendor 0xFF00), Usage(0x01), Report ID(0x44), Report Size(8),
    // Report Count(64), Input(Data,Var,Abs) — the signature (512-bit opaque blob, no X/Y).
    let vendor_desc: [u8; 13] = [
        0x06, 0x00, 0xFF, 0x09, 0x01, 0x85, 0x44, 0x75, 0x08, 0x95, 0x40, 0x81, 0x02,
    ];
    let recognized = parse_report_descriptor(&vendor_desc);
    let id = recognized.map(|l| l.report_id).unwrap_or(0);
    let vendor_ok = recognized
        .map(|l| l.vendor_mt && !l.has_xy && l.report_id == 0x44)
        .unwrap_or(false);

    // M2 decode. Two synthetic 0x44 reports built AT the hypothesis offsets (so the test tracks
    // the same `VMT_FINGER_*` constants the decoder uses). Finger A -> B, with B's abs_x negative
    // to exercise signed le16. Body = report[1..]; write at `1 + VMT_FINGER_*`.
    let mut ra = [0u8; 49];
    ra[0] = 0x44;
    ra[1 + VMT_FINGER_ABS_X..1 + VMT_FINGER_ABS_X + 2].copy_from_slice(&100i16.to_le_bytes());
    ra[1 + VMT_FINGER_ABS_Y..1 + VMT_FINGER_ABS_Y + 2].copy_from_slice(&200i16.to_le_bytes());
    ra[1 + VMT_FINGER_TOUCH..1 + VMT_FINGER_TOUCH + 2].copy_from_slice(&10u16.to_le_bytes());
    let mut rb = [0u8; 49];
    rb[0] = 0x44;
    rb[1 + VMT_FINGER_ABS_X..1 + VMT_FINGER_ABS_X + 2].copy_from_slice(&(-50i16).to_le_bytes());
    rb[1 + VMT_FINGER_ABS_Y..1 + VMT_FINGER_ABS_Y + 2].copy_from_slice(&6000i16.to_le_bytes());
    rb[1 + VMT_FINGER_TOUCH..1 + VMT_FINGER_TOUCH + 2].copy_from_slice(&10u16.to_le_bytes());
    // Finger-UP: B's coords with the touch field cleared → present must read false.
    let mut ru = rb;
    ru[1 + VMT_FINGER_TOUCH..1 + VMT_FINGER_TOUCH + 2].copy_from_slice(&0u16.to_le_bytes());
    // Too-short/malformed report: must decode to None (bounds-safe, never OOB).
    let short = [0x44u8; 10];

    let da = decode_vendor_first_finger(&ra, 0x44);
    let db = decode_vendor_first_finger(&rb, 0x44);
    let du = decode_vendor_first_finger(&ru, 0x44);
    let ds = decode_vendor_first_finger(&short, 0x44);
    let (dx, dy) = match (da, db) {
        (Some((_, ax, ay)), Some((_, bx, by))) => (bx - ax, by - ay),
        _ => (0, 0),
    };
    let decode_ok = da == Some((true, 100, 200))
        && db == Some((true, -50, 6000))
        && matches!(du, Some((false, _, _)))
        && ds.is_none()
        && dx == -150
        && dy == 5800;

    // EHCI-5-fix M1': the REAL 05ac:0262 intf1 descriptor captured on metal (2026-07-17 rMBP
    // 3-leg sitting). Note the Input is an ARRAY (`81 00`), not a Variable — the exact shape that
    // the pre-fix `is_var`-gated recognizer skipped. This must now recognize as `vendor_mt`
    // (id 0x44, no X/Y), proving the arming-order fix reaches the ARM path in the real path.
    //   06 00 ff  Usage Page (Vendor 0xFF00)     26 ff 00  Logical Maximum 255
    //   09 01     Usage 0x01                      85 44     Report ID 0x44
    //   a1 03     Collection (Report)             75 08     Report Size 8
    //   06 00 ff  Usage Page (Vendor 0xFF00)      96 ff 01  Report Count 511
    //   09 01     Usage 0x01                       81 00     Input (Data,Array,Abs)  <-- ARRAY
    //   15 00     Logical Minimum 0               c0        End Collection
    let real_desc: [u8; 27] = [
        0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x03, 0x06, 0x00, 0xFF, 0x09, 0x01, 0x15, 0x00,
        0x26, 0xFF, 0x00, 0x85, 0x44, 0x75, 0x08, 0x96, 0xFF, 0x01, 0x81, 0x00, 0xC0,
    ];
    let real_ok = parse_report_descriptor(&real_desc)
        .map(|l| l.vendor_mt && !l.has_xy && l.report_id == 0x44)
        .unwrap_or(false);

    serial_println!(
        ":: EHCI-HID: vendor-multitouch self-test: recognized={} (id={:#04x}, min-bits={}), real-array-descriptor recognized={}, first-finger decode dx={} dy={} ok={} == witness ::",
        vendor_ok, id, VMT_MIN_VENDOR_BITS, real_ok, dx, dy, decode_ok
    );
}

/// Probe-3/4 evidence: dump VT-d (DMAR) state, READ-ONLY. Probe 3 showed both EHCI functions'
/// first DMA fetch master-aborting (USBSTS HSE + halt) on a freshly HCRESET controller while
/// xHCI DMAs the same heap fine — the signature of per-function DMA blocking, i.e. an IOMMU
/// left translating by Apple EFI (which drove the pre-boot keyboard over these very
/// functions). This dump answers it with registers instead of a theory: DMAR present? each
/// DRHD's translation-enable (GSTS.TES), fault status (FSTS), and any latched fault-recording
/// entries — which name the faulting source BDF, reason, and address. Zero writes.
unsafe fn dmar_report() {
    let Some(dmar) = crate::arch::x86_64::acpi::find_acpi_table(b"DMAR") else {
        serial_println!(":: EHCI-HID: DMAR: no ACPI DMAR table — VT-d not described; IOMMU theory falsified ::");
        return;
    };
    let len = mmio_read32(dmar + 4).unwrap_or(0) as u64; // SDT header length
    let haw = mmio_read32(dmar + 36).unwrap_or(0);
    serial_println!(
        ":: EHCI-HID: DMAR present @ {:#x} len={} host-addr-width={} flags={:#04x} ::",
        dmar, len, (haw & 0xFF) + 1, (haw >> 8) & 0xFF
    );
    // Remapping structures start at offset 48: u16 type, u16 length; type 0 = DRHD with the
    // remapping-unit register base at +8.
    let mut off = 48u64;
    let mut unit = 0;
    while off + 4 <= len {
        let head = mmio_read32(dmar + off).unwrap_or(0);
        let (typ, slen) = (head & 0xFFFF, (head >> 16) & 0xFFFF);
        if slen == 0 {
            break;
        }
        if typ == 0 {
            let base = (mmio_read32(dmar + off + 8).unwrap_or(0) as u64)
                | ((mmio_read32(dmar + off + 12).unwrap_or(0) as u64) << 32);
            let cap_lo = mmio_read32(base + 0x08).unwrap_or(0);
            let cap_hi = mmio_read32(base + 0x0C).unwrap_or(0);
            let gsts = mmio_read32(base + 0x1C).unwrap_or(0);
            let fsts = mmio_read32(base + 0x34).unwrap_or(0);
            let cap = (cap_lo as u64) | ((cap_hi as u64) << 32);
            let fro = ((cap >> 24) & 0x3FF) * 16; // fault-recording offset, 128-bit units
            let nfr = ((cap >> 40) & 0xFF) + 1;
            serial_println!(
                ":: EHCI-HID: DMAR DRHD[{}] base={:#x} GSTS={:#010x} (TES={}) FSTS={:#010x} CAP={:#018x} NFR={} ::",
                unit, base, gsts, (gsts >> 31) & 1, fsts, cap, nfr
            );
            // Probe-9: Protected Memory Regions — the one DMA gate that operates with
            // translation OFF (VT-d 10.4.16-20). PMEN.EPM=1 with PLMR/PHMR covering DRAM
            // blocks device DMA exactly like the observed master aborts; every OS clears it
            // at IOMMU handoff. Read-only dump: enable/status + both regions' bounds.
            let pmen = mmio_read32(base + 0x64).unwrap_or(0);
            serial_println!(
                ":: EHCI-HID: DMAR DRHD[{}] PMEN={:#010x} (EPM={} PRS={}) PLMR {:#010x}..{:#010x} PHMR {:#010x}_{:08x}..{:#010x}_{:08x} == witness ::",
                unit, pmen, (pmen >> 31) & 1, pmen & 1,
                mmio_read32(base + 0x68).unwrap_or(0),
                mmio_read32(base + 0x6C).unwrap_or(0),
                mmio_read32(base + 0x74).unwrap_or(0),
                mmio_read32(base + 0x70).unwrap_or(0),
                mmio_read32(base + 0x7C).unwrap_or(0),
                mmio_read32(base + 0x78).unwrap_or(0)
            );
            // Device scopes: which BDFs this unit owns (why xHCI may be exempt). Raw bytes of
            // the DRHD structure past the 16-byte header: scope entries are (type, len, rsvd,
            // enum-id, start-bus, path pairs...).
            let mut soff = off + 16;
            while soff < off + slen as u64 {
                let d0 = mmio_read32(dmar + soff).unwrap_or(0);
                let d1 = mmio_read32(dmar + soff + 4).unwrap_or(0);
                let slen2 = (d0 >> 8) & 0xFF;
                serial_println!(
                    ":: EHCI-HID: DMAR DRHD[{}] scope: type={} len={} start-bus={} path[0]={:#04x},{:#04x} ::",
                    unit, d0 & 0xFF, slen2, (d0 >> 24) & 0xFF, d1 & 0xFF, (d1 >> 8) & 0xFF
                );
                if slen2 == 0 {
                    break;
                }
                soff += slen2 as u64;
            }
            for i in 0..nfr.min(4) {
                let fr = base + fro + i * 16;
                let lo = (mmio_read32(fr).unwrap_or(0) as u64)
                    | ((mmio_read32(fr + 4).unwrap_or(0) as u64) << 32);
                let hi = (mmio_read32(fr + 8).unwrap_or(0) as u64)
                    | ((mmio_read32(fr + 12).unwrap_or(0) as u64) << 32);
                if hi >> 63 != 0 {
                    let sid = hi & 0xFFFF;
                    serial_println!(
                        ":: EHCI-HID: DMAR DRHD[{}] FAULT[{}]: source {:02x}:{:02x}.{} reason={:#04x} addr={:#x} == witness ::",
                        unit, i, (sid >> 8) & 0xFF, (sid >> 3) & 0x1F, sid & 0x7,
                        (hi >> 32) & 0xFF, lo & !0xFFF
                    );
                }
            }
            unit += 1;
        }
        off += slen as u64;
    }
    if unit == 0 {
        serial_println!(":: EHCI-HID: DMAR table has no DRHD units ::");
    }
}

/// Probe-5 evidence, read-only: decode the function's PCI STATUS error bits (Received Master
/// Abort / Received Target Abort / Signaled Target Abort / parity) — these name what the
/// fabric did to the controller's DMA request — then dump all 64 config dwords for offline
/// analysis against the Intel 7-series datasheet.
unsafe fn pci_evidence(bus: u8, dev: u8, func: u8, idx: usize) {
    let sc = read_config_32(bus, dev, func, 0x04);
    let status = (sc >> 16) as u16;
    serial_println!(
        ":: EHCI-HID: [{}] PCI STATUS={:#06x} RMA={} RTA={} STA={} SSE={} DPE={} MDPE={} == witness ::",
        idx, status,
        (status >> 13) & 1, // Received Master Abort
        (status >> 12) & 1, // Received Target Abort
        (status >> 11) & 1, // Signaled Target Abort
        (status >> 14) & 1, // Signaled System Error
        (status >> 15) & 1, // Detected Parity Error
        (status >> 8) & 1   // Master Data Parity Error
    );
    for row in 0..8u8 {
        let base = row * 32;
        serial_println!(
            ":: EHCI-HID: [{}] CFG {:#04x}: {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} ::",
            idx, base,
            read_config_32(bus, dev, func, base),
            read_config_32(bus, dev, func, base + 4),
            read_config_32(bus, dev, func, base + 8),
            read_config_32(bus, dev, func, base + 12),
            read_config_32(bus, dev, func, base + 16),
            read_config_32(bus, dev, func, base + 20),
            read_config_32(bus, dev, func, base + 24),
            read_config_32(bus, dev, func, base + 28)
        );
    }
}

/// EHCI-3 bring-up: walk PCI for EHCI functions, run the SHARED EHCI-2 wake on each (one wake
/// path — wake_run + wake_route from ehci_scout), arm schedules, reset + enumerate the
/// connected root ports. Runs at PCI-init time, after the scout modes and BEFORE the PORTSW
/// flip / xhci::init (the internal HID sit on non-switchable EHCI-only ports, so the two
/// stacks' port sets are disjoint by hardware — PORTSW-1 §7f).
pub fn init() {
    serial_println!(":: EHCI-HID: begin (EHCI-3 driver, polling model, knob-gated) ::");
    // EPACE: the module's own entry→exit span — the self-check target for the per-phase split.
    let init_t0 = crate::arch::now_cycles();
    // Hardening self-test up front (default-ON driver parses ANY device's descriptor): proves the
    // report-parser is bounded against a hostile Report Count before we enumerate anything.
    unsafe { parser_selftest() };
    let selftest_cy = crate::arch::now_cycles().wrapping_sub(init_t0);
    let mut ctrls: Vec<Controller> = Vec::new();
    // Probe-13 (b), Peter-approved 2026-07-17: the RCBA CG (clock gating) base + a one-shot
    // flag — the clear fires only if the live-port smoke actually HSEs.
    let rcba = unsafe { (read_config_32(0, 31, 0, 0xF0) as u64) & !0x3FFF };
    let mut cg_cleared = false;

    for bus in 0u8..=255 {
        for dev in 0u8..=31 {
            let vendor = unsafe { read_config_16(bus, dev, 0, 0x00) };
            if vendor == 0xFFFF {
                continue;
            }
            let ht = ((unsafe { read_config_32(bus, dev, 0, 0x0C) } >> 16) & 0xFF) as u8;
            let max_func = if ht & 0x80 != 0 { 7 } else { 0 };
            for func in 0..=max_func {
                if func != 0 && unsafe { read_config_16(bus, dev, func, 0x00) } == 0xFFFF {
                    continue;
                }
                let class_reg = unsafe { read_config_32(bus, dev, func, 0x08) };
                if ((class_reg >> 24) & 0xFF) as u8 != EHCI_CLASS
                    || ((class_reg >> 16) & 0xFF) as u8 != EHCI_SUBCLASS
                    || ((class_reg >> 8) & 0xFF) as u8 != EHCI_PROGIF
                {
                    continue;
                }
                let idx = ctrls.len();
                unsafe {
                    ensure_bus_master(bus, dev, func, idx);
                    // The one shared wake path (idempotent when EHCI-2 mode already ran it).
                    let wake_t0 = crate::arch::now_cycles();
                    let Some(h): Option<EhciFnHandle> = ehci_scout::wake_run(bus, dev, func, idx)
                    else {
                        continue;
                    };
                    let wake_cy = crate::arch::now_cycles().wrapping_sub(wake_t0);
                    // EPACE-TRIM M1: latch the birth mode BEFORE construction, so a same-
                    // controller HSE flip can never be mistaken for an inherited verdict.
                    let born_overlay =
                        CHAIN_HSE_SEEN.load(core::sync::atomic::Ordering::Relaxed);
                    if born_overlay {
                        serial_println!(
                            ":: EHCI-HID: [{}] chain-HSE verdict CARRIED from an earlier controller — OVERLAY-DIRECT from birth (probe + re-init skipped; inference, not a measurement on this function) ::",
                            idx
                        );
                    }
                    if h.addr64 != 0 {
                        serial_println!(
                            ":: EHCI-HID: [{}] note: controller advertises 64-bit addressing; CTRLDSSEGMENT pinned to 0 (all DMA < 4 GiB) ::",
                            idx
                        );
                    }
                    if idx >= MAX_CONTROLLERS {
                        serial_println!(
                            ":: EHCI-HID: [{}] more EHCI functions than static DMA pools ({}) — skipped ::",
                            idx, MAX_CONTROLLERS
                        );
                        continue;
                    }
                    let pool = &mut DMA_POOLS[idx];
                    let (
                        Some(fl_phys),
                        Some(head_phys),
                        Some(qh_phys),
                        Some(su_phys),
                        Some(da_phys),
                        Some(st_phys),
                        Some(sb_phys),
                        Some(db_phys),
                    ) = (
                        phys_of(pool.frame_list.as_ptr(), 4096),
                        phys_of(&pool.qh_head, 32),
                        phys_of(&pool.qh_ctrl, 32),
                        phys_of(&pool.qtd_setup, 32),
                        phys_of(&pool.qtd_data, 32),
                        phys_of(&pool.qtd_status, 32),
                        phys_of(&pool.setup_buf, 64),
                        phys_of(&pool.data_buf, 64),
                    )
                    else {
                        serial_println!(
                            ":: EHCI-HID: [{}] STOP-NOTE static DMA pool failed the phys/alignment contract — controller skipped ::",
                            idx
                        );
                        continue;
                    };
                    let mut c = Controller {
                        idx,
                        op: h.op,
                        bus,
                        dev,
                        func,
                        async_head: &mut pool.qh_head as *mut Qh,
                        head_phys,
                        async_qh: &mut pool.qh_ctrl as *mut Qh,
                        qh_phys,
                        qtd_setup: &mut pool.qtd_setup as *mut Qtd,
                        qtd_setup_phys: su_phys,
                        qtd_data: &mut pool.qtd_data as *mut Qtd,
                        qtd_data_phys: da_phys,
                        qtd_status: &mut pool.qtd_status as *mut Qtd,
                        qtd_status_phys: st_phys,
                        setup_buf: pool.setup_buf.0.as_mut_ptr(),
                        setup_buf_phys: sb_phys,
                        data_buf: pool.data_buf.0.as_mut_ptr(),
                        data_buf_phys: db_phys,
                        frame_list: pool.frame_list.as_mut_ptr(),
                        frame_list_phys: fl_phys,
                        int_next: 0,
                        periodic_on: false,
                        // EPACE-TRIM M1: born overlay-direct when an earlier controller
                        // already proved the chain-HSE on this die (see CHAIN_HSE_SEEN).
                        overlay_mode: born_overlay,
                        next_addr: 1,
                        int_eps: Vec::new(),
                        pace: {
                            let mut p = Epace::new();
                            p.cy[EP_WAKE] = wake_cy;
                            p.n[EP_WAKE] = 1;
                            p
                        },
                        #[cfg(feature = "mtraw")]
                        mt_probe: None,
                        #[cfg(feature = "mtraw")]
                        mt_dumped: 0,
                    };
                    // Firmware-stale detection BEFORE any schedule programming: probe 2 showed
                    // Apple EFI leaves PSE=1 behind (its pre-boot keyboard), which HSE-halts
                    // the controller once RS runs over the reclaimed frame list. After the
                    // pre-approved HCRESET the controller is at defaults (halted, CF=0) — the
                    // bases are then programmed in the halted state (textbook), RS re-started,
                    // and CF re-routed via the shared wake_route.
                    let hcrst_t0 = crate::arch::now_cycles();
                    let did_reset = c.quiesce_if_firmware_stale();
                    c.init_schedules();
                    if did_reset {
                        let cmd = mmio_read32(h.op + OP_USBCMD).unwrap_or(0);
                        let _ = mmio_write32(h.op + OP_USBCMD, cmd | CMD_RS);
                        let running = wait_bounded(|| {
                            mmio_read32(h.op + OP_USBSTS).unwrap_or(STS_HCHALTED)
                                & STS_HCHALTED
                                == 0
                        });
                        serial_println!(
                            ":: EHCI-HID: [{}] post-HCRESET restart: RS=1 running={} ::",
                            idx, running
                        );
                    }
                    ehci_scout::wake_route(&h, idx);
                    // Clear any latched RW1C status (incl. a pre-existing HSE) before the
                    // first transfer, so a fresh error is unambiguously ours.
                    if let Some(sts) = mmio_read32(h.op + OP_USBSTS) {
                        if sts & STS_RW1C != 0 {
                            let _ = mmio_write32(h.op + OP_USBSTS, sts & STS_RW1C);
                        }
                    }
                    c.pace.add(EP_HCRST, hcrst_t0);
                    let smoke_t0 = crate::arch::now_cycles();
                    // Probe-6/10 discriminators, two 5 ms periodic smoke passes:
                    //  (1) all-Terminate frame list — one 4-byte frame-list read per frame,
                    //      the simplest upstream read (probe-6: PASSED on metal).
                    //  (2) frame list -> one INACTIVE QH (halted token, nothing to execute) —
                    //      forces the 32/48-byte burst QH fetch but requires ZERO write-back.
                    //      Clean => burst reads fine, the wall is DMA WRITES; HSE => burst
                    //      reads themselves are gated (4-byte reads pass regardless).
                    // Pass 3 (probe-11): the WRITE discriminator. The QH's overlay is
                    // PRE-LOADED with an active zero-length interrupt-IN to a nonexistent
                    // address (no qTD fetch, no overlay load) — the controller executes the
                    // wire transaction (nobody answers, XactErr) and must then WRITE the
                    // token back. HSE here = upstream DMA writes are gated, QED (probes 1-10
                    // proved every read class passes). A written-back token instead would
                    // falsify the write theory on the spot.
                    // Passes 4/5 (probe-12): payload-read isolation. 4 = zero-length OUT to
                    // the bogus address (controls for OUT direction, still no buffer);
                    // 5 = 8-byte OUT (forces the controller to READ the payload buffer to
                    // transmit — the ONE access class every failing SETUP starts with and
                    // pass 3 never exercised). HSE on 5 alone names the wall: payload-read
                    // DMA gated while structure reads/writes + wire transactions all pass.
                    for (pass, arm_qh) in [(1u32, false), (2, true), (3, true), (4, true), (5, true)] {
                        if arm_qh {
                            let qh = c.async_qh; // idle work QH, reused as the inert target
                            let self_buf = c.setup_buf_phys as u32;
                            (*qh).horiz = PTR_TERMINATE;
                            (*qh).ep_chars = (42) // bogus device address, EP 1
                                | (1 << 8)
                                | QH_DTC
                                | QH_EPS_HIGH
                                | (8 << QH_MPS_SHIFT);
                            (*qh).ep_caps = QH_MULT1 | 0x01; // S-mask µframe 0
                            (*qh).overlay[0] = PTR_TERMINATE;
                            (*qh).overlay[1] = PTR_TERMINATE;
                            (*qh).overlay[2] = match pass {
                                2 => QTD_HALTED, // inactive — fetch, no work, no write
                                // Pre-loaded active tokens, CERR=1 (fail fast), bogus addr:
                                3 => QTD_ACTIVE | (1 << 10) | QTD_PID_IN, // 0-len IN
                                4 => QTD_ACTIVE | (1 << 10) | QTD_PID_OUT, // 0-len OUT, no buffer
                                _ => {
                                    // 8-byte OUT — forces the payload buffer READ.
                                    (*qh).overlay[3] = self_buf; // buffer page 0 = setup_buf
                                    QTD_ACTIVE | (1 << 10) | QTD_PID_OUT | (8 << QTD_TOTAL_SHIFT)
                                }
                            };
                            for i in 0..1024 {
                                core::ptr::write_volatile(
                                    c.frame_list.add(i),
                                    (c.qh_phys as u32) | PTR_TYPE_QH,
                                );
                            }
                        }
                        let cmd = mmio_read32(h.op + OP_USBCMD).unwrap_or(0);
                        let _ = mmio_write32(h.op + OP_USBCMD, cmd | CMD_PSE);
                        ehci_scout::settle_ms(5);
                        let sts = mmio_read32(h.op + OP_USBSTS).unwrap_or(0);
                        let _ = mmio_write32(h.op + OP_USBCMD, cmd & !CMD_PSE);
                        let _ = wait_bounded(|| {
                            mmio_read32(h.op + OP_USBSTS).unwrap_or(0) & (1 << 14) == 0
                        });
                        serial_println!(
                            ":: EHCI-HID: [{}] periodic DMA smoke pass {} ({}): USBSTS={:#010x} HSE={} HCHalted={} post-token={:#010x} == witness ::",
                            idx, pass,
                            match pass {
                                1 => "empty frame list",
                                2 => "inactive-QH burst fetch, zero writeback",
                                3 => "preloaded 0-len IN bogus -> forced token WRITE-back",
                                4 => "preloaded 0-len OUT bogus (no buffer)",
                                _ => "preloaded 8-byte OUT bogus -> forced PAYLOAD READ",
                            },
                            sts, (sts >> 4) & 1, (sts >> 12) & 1,
                            core::ptr::read_volatile(&(*c.async_qh).overlay[2])
                        );
                        if arm_qh {
                            for i in 0..1024 {
                                core::ptr::write_volatile(c.frame_list.add(i), PTR_TERMINATE);
                            }
                        }
                        if sts & STS_HSE != 0 {
                            // Ack + restart so later steps still report honestly.
                            let _ = mmio_write32(h.op + OP_USBSTS, sts & STS_RW1C);
                            let cmd2 = mmio_read32(h.op + OP_USBCMD).unwrap_or(0);
                            let _ = mmio_write32(h.op + OP_USBCMD, (cmd2 & !CMD_PSE) | CMD_RS);
                            let _ = wait_bounded(|| {
                                mmio_read32(h.op + OP_USBSTS).unwrap_or(STS_HCHALTED) & STS_HCHALTED == 0
                            });
                        }
                    }
                    c.pace.add(EP_SMOKE, smoke_t0);
                    // EPACE-TRIM M4 follow-up (GR18 review finding 1). The FIRST look at a root
                    // port is this scan, not `reset_root_port` — this `if` decides whether that
                    // function is ever called. M4 shortened `wake_route`'s pre-look settle on the
                    // strength of the caller paying T_ATTDB, and the caller that pays it is
                    // downstream of a gate that had already sampled CCS. CF 0->1 is a real edge on
                    // this path (the firmware-stale HCRESET drops CONFIGFLAG, and the first PORTSC
                    // read comes back 0x00001803 with CSC latched), so sampling CCS ~49 ms after
                    // it — inside the 100 ms the debounce is for — would let a port whose CCS has
                    // not re-asserted fall through `continue` with no line, no EPACE class and no
                    // annotation: a boot that reads FASTER than predicted precisely because the
                    // internal keyboard went missing. So the debounce is paid HERE, ahead of the
                    // gate, and dropped from `reset_root_port` for this path: the same 100 ms, at
                    // the point that actually needs it, and `rootrst=` keeps its old total.
                    let attdb_t0 = crate::arch::now_cycles();
                    settle_ms(100); // USB 2.0 §7.1.7.3 T_ATTDB, ahead of the first CCS sample
                    c.pace.add(EP_ROOTRST, attdb_t0);
                    for port in 0..h.n_ports {
                        let portsc = mmio_read32(h.op + OP_PORTSC0 + 4 * port as u64).unwrap_or(0);
                        if portsc & PORT_CCS == 0 || portsc & PORT_OWNER != 0 {
                            // Loud, because this is the branch a too-short debounce would take.
                            serial_println!(
                                ":: EHCI-HID: [{}] port {} not walked: PORTSC={:#010x} CCS={} owner={} (post-T_ATTDB sample){} ::",
                                idx, port, portsc, portsc & PORT_CCS,
                                if portsc & PORT_OWNER != 0 { "companion" } else { "EHCI" },
                                if portsc & PORT_CCS == 0 { m4_note() } else { "" }
                            );
                            continue;
                        }
                        let rootrst_t0 = crate::arch::now_cycles();
                        let root_ok = c.reset_root_port(port, false);
                        c.pace.add(EP_ROOTRST, rootrst_t0);
                        if root_ok {
                            // Transport probe (probe-14): one bare chain-mode GET_DESCRIPTOR(8)
                            // to addr 0. An HSE means this silicon aborts the qTD-fetch burst
                            // write — flip to OVERLAY-DIRECT and FULLY re-init (an HSE'd
                            // controller is wedged; HCRESET, bases, RS, CF, port — all redone).
                            // EPACE-TRIM M1: a controller born overlay-direct (verdict carried
                            // from an earlier function on this die — witnessed at construction)
                            // skips the probe AND the wedged-controller re-init; the s58 split
                            // priced that pair at ~2.6 s.
                            if !c.overlay_mode {
                                let probe_t = Target {
                                    addr: 0, mps0: 64, eps: QH_EPS_HIGH, hub_addr: 0, hub_port: 0,
                                };
                                let hseprobe_t0 = crate::arch::now_cycles();
                                let probe_res = c.control(&probe_t, 0x80, 6, 0x0100, 0, 8, true);
                                c.pace.add(EP_HSEPROBE, hseprobe_t0);
                                if let Err("hse") = probe_res {
                                    c.overlay_mode = true;
                                    CHAIN_HSE_SEEN
                                        .store(true, core::sync::atomic::Ordering::Relaxed);
                                    serial_println!(
                                        ":: EHCI-HID: [{}] qTD-fetch HSE — OVERLAY-DIRECT mode + full HCRESET re-init (probe-14 silicon finding) ::",
                                        idx
                                    );
                                    let hcrst2_t0 = crate::arch::now_cycles();
                                    let _ = c.quiesce_if_firmware_stale(); // HSE latched -> resets
                                    c.init_schedules();
                                    let cmd = mmio_read32(h.op + OP_USBCMD).unwrap_or(0);
                                    let _ = mmio_write32(h.op + OP_USBCMD, cmd | CMD_RS);
                                    let _ = wait_bounded(|| {
                                        mmio_read32(h.op + OP_USBSTS).unwrap_or(STS_HCHALTED)
                                            & STS_HCHALTED == 0
                                    });
                                    ehci_scout::wake_route(&h, idx);
                                    if let Some(sts) = mmio_read32(h.op + OP_USBSTS) {
                                        if sts & STS_RW1C != 0 {
                                            let _ = mmio_write32(h.op + OP_USBSTS, sts & STS_RW1C);
                                        }
                                    }
                                    c.pace.add(EP_HCRST, hcrst2_t0);
                                    let rootrst2_t0 = crate::arch::now_cycles();
                                    // `true`: this path re-routed CONFIGFLAG a few lines up and
                                    // returns straight to the port without passing the CCS gate,
                                    // so it owns its own T_ATTDB. Unchanged by the M4 follow-up.
                                    let root2_ok = c.reset_root_port(port, true);
                                    c.pace.add(EP_ROOTRST, rootrst2_t0);
                                    if !root2_ok {
                                        continue;
                                    }
                                }
                            }
                            let _ = (&cg_cleared, rcba); // probe-13 levers retired (smokes all passed)
                            let enum_t0 = crate::arch::now_cycles();
                            c.enumerate_at_zero(QH_EPS_HIGH, 0, 0, 0);
                            c.pace.add(EP_ENUM, enum_t0);
                        }
                    }
                    ctrls.push(c);
                }
            }
        }
    }

    // Probe-4 evidence dump: VT-d state AFTER the enumeration attempts, so any DMA fault our
    // transfers raised is latched in the fault-recording registers (read-only).
    // EPACE: the whole evidence block (DMAR + PCI STATUS/CFG + RCBA) is one span. It is almost
    // pure serial output, so its number doubles as the measured print cost of ~70 witness lines
    // at this baud — the calibration for how much of every OTHER phase is serial time.
    let evid_t0 = crate::arch::now_cycles();
    unsafe { dmar_report() };

    // Probe-5 evidence: PCI STATUS decode (received/signaled abort bits name the failure class
    // at the fabric level) + full config-space dump of each EHCI function for offline
    // comparison against the 7-series datasheet. Read-only.
    for c in ctrls.iter() {
        unsafe { pci_evidence(c.bus, c.dev, c.func, c.idx) };
    }
    unsafe {
        // Working-DMA reference: the xHCI function's config space (progIF 0x30), tagged [90+].
        for dev in 0u8..=31 {
            let class_reg = read_config_32(0, dev, 0, 0x08);
            if class_reg >> 8 == 0x0C0330 {
                pci_evidence(0, dev, 0, 90 + dev as usize);
            }
        }
        // PCH RCBA window, READ-ONLY: Function Disable / Backed-Up Control / clock gating —
        // the registers Apple EFI is most likely to have left gating the EHCI DMA engines.
        // RCBA base from the LPC bridge (0:31.0) config 0xF0 (bit 0 = enable).
        let rcba_reg = read_config_32(0, 31, 0, 0xF0);
        let rcba = (rcba_reg as u64) & !0x3FFF;
        serial_println!(
            ":: EHCI-HID: RCBA reg={:#010x} base={:#x} en={} ::",
            rcba_reg, rcba, rcba_reg & 1
        );
        if rcba_reg & 1 == 1 && rcba != 0 {
            // Probe-11: the chipset-config VIRTUAL CHANNEL block (V0/V1/VCp at the RCBA head)
            // — a misconfigured isoch/private channel would abort device WRITES specifically.
            for off in [0x0000u64, 0x0014, 0x0018, 0x001C, 0x0020, 0x0024, 0x0028, 0x0030] {
                if let Some(v) = mmio_read32(rcba + off) {
                    serial_println!(":: EHCI-HID: RCBA+{:#06x} = {:#010x} ::", off, v);
                }
            }
            for off in [0x3400u64, 0x3404, 0x3410, 0x3414, 0x3418, 0x341C, 0x3420, 0x3428, 0x342C, 0x3430, 0x3434] {
                if let Some(v) = mmio_read32(rcba + off) {
                    serial_println!(":: EHCI-HID: RCBA+{:#06x} = {:#010x} ::", off, v);
                }
            }
        }
    }

    let evid_cy = crate::arch::now_cycles().wrapping_sub(evid_t0);

    // ── EPACE report ────────────────────────────────────────────────────────────────────────────
    // One line per controller + one closing line. `resid=` is `enum` minus its named parts —
    // control-transfer time for the descriptor reads plus recursion overhead; a big resid is a
    // finding, not a rounding error. The closing line's `init=` must match the independent BPACE
    // `ehci-hid-done d=` (minus these prints' own cost) or one of the two instruments is lying.
    for c in ctrls.iter() {
        let named_enum_parts =
            c.pace.cy[EP_HUBPWR] + c.pace.cy[EP_HUBRST] + c.pace.cy[EP_HIDCFG];
        let resid = c.pace.cy[EP_ENUM].saturating_sub(named_enum_parts);
        let (rv, ru) = epace_fmt(resid);
        let mut parts_ms: [u64; N_EPACE] = [0; N_EPACE];
        let mut unit = "ms";
        for k in 0..N_EPACE {
            let (v, u) = epace_fmt(c.pace.cy[k]);
            parts_ms[k] = v;
            unit = u;
        }
        serial_println!(
            ":: EPACE: [{}] {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) [{}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) resid={}{}] == witness ::",
            c.idx,
            EPACE_TAGS[EP_WAKE], parts_ms[EP_WAKE], unit, c.pace.n[EP_WAKE],
            EPACE_TAGS[EP_HCRST], parts_ms[EP_HCRST], unit, c.pace.n[EP_HCRST],
            EPACE_TAGS[EP_SMOKE], parts_ms[EP_SMOKE], unit, c.pace.n[EP_SMOKE],
            EPACE_TAGS[EP_ROOTRST], parts_ms[EP_ROOTRST], unit, c.pace.n[EP_ROOTRST],
            EPACE_TAGS[EP_HSEPROBE], parts_ms[EP_HSEPROBE], unit, c.pace.n[EP_HSEPROBE],
            EPACE_TAGS[EP_ENUM], parts_ms[EP_ENUM], unit, c.pace.n[EP_ENUM],
            EPACE_TAGS[EP_HUBPWR], parts_ms[EP_HUBPWR], unit, c.pace.n[EP_HUBPWR],
            EPACE_TAGS[EP_HUBRST], parts_ms[EP_HUBRST], unit, c.pace.n[EP_HUBRST],
            EPACE_TAGS[EP_HIDCFG], parts_ms[EP_HIDCFG], unit, c.pace.n[EP_HIDCFG],
            rv, ru
        );
    }
    {
        let init_cy = crate::arch::now_cycles().wrapping_sub(init_t0);
        let (st, su) = epace_fmt(selftest_cy);
        let (ev, eu) = epace_fmt(evid_cy);
        let (iv, iu) = epace_fmt(init_cy);
        serial_println!(
            ":: EPACE: selftest={}{} evid={}{} init={}{} hz={} == the ehci-hid d= split ::",
            st, su, ev, eu, iv, iu,
            crate::arch::x86_64::apic::tsc_hz()
        );
    }

    let n = ctrls.len();
    let armed: usize = ctrls.iter().map(|c| c.int_eps.len()).sum();
    *EHCI_HID.lock() = Some(ctrls);
    serial_println!(
        ":: EHCI-HID: end ({} controllers, {} HID endpoints armed) ::",
        n, armed
    );
}

/// Main-loop service hook (the EHCI analogue of `service_hubs`): poll every armed HID endpoint,
/// decode + deliver completed reports, re-arm. Cheap when nothing completed.
pub fn service_ehci_hid() {
    let mut g = EHCI_HID.lock();
    let Some(ctrls) = g.as_mut() else { return };
    for c in ctrls.iter_mut() {
        unsafe { c.service() };
    }
}
