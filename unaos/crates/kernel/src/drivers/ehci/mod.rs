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
    /// EHCI-KEYUP: the PREVIOUS boot-keyboard report's six keycode slots (bytes 2..8), the state
    /// [`decode_boot_keyboard`] diffs the current report against to synthesise release edges. Per
    /// ENDPOINT rather than global because a machine can carry more than one keyboard interface and
    /// each reports its own full pressed-key set; a shared array would let one keyboard's report
    /// manufacture releases for another's held keys. All zeros = nothing held (the idle report), which
    /// is also the correct initial value: before the first report there is nothing to release.
    kbd_prev_keys: [u8; 6],
    /// ALLKEYS: the modifier byte (`report[0]`) of the last ACCEPTED report on this endpoint. The
    /// dead-endpoint flush has no report to read — the device is gone — so it folds each stranded
    /// key against this to reproduce the shifted ascii the press delivered. Written only on an
    /// accepted (>= 8-byte) report, so a refused short report cannot corrupt it. 0 = nothing held.
    kbd_prev_mods: u8,
    /// ALLKEYS P1: this keyboard's lock-LED bitmap — bit 0 Num, bit 1 Caps, bit 2 Scroll (the HID
    /// LED page Output report, `xhci::HID_LOCK_KEYS`). It is BOTH halves of caps lock at once: the
    /// decoder reads bit 1 to pick the case, and the same byte is what SET_REPORT ships to light
    /// the key. One byte for both is what keeps the lit LED and the typed case from ever disagreeing
    /// — an operator whose LED says caps but whose keys type lowercase has no way to tell which one
    /// is lying, so there is deliberately only one truth here.
    ///
    /// Per ENDPOINT, for the same reason `kbd_prev_keys` is: two keyboards have two Caps Lock keys
    /// and two LEDs, and each USB keyboard latches its own. 0 at arm time = all locks off, which is
    /// the state a freshly-configured HID device is in (SET_CONFIGURATION resets its LEDs), so the
    /// software state and the hardware agree from the first report without an explicit sync.
    kbd_leds: u8,
    /// ALLKEYS P1: the EP0 addressing tuple and interface number for THIS endpoint's device —
    /// everything `set_hid_leds` needs to send SET_REPORT back to it. Captured at arm time because
    /// the service loop, where a lock-key press is detected, has no other route to them: it walks
    /// `int_eps` and the enumeration `Target` is long out of scope by then. `Target` is `Copy` and
    /// eight bytes, so carrying it costs nothing.
    kbd_target: Target,
    kbd_intf: u8,
    /// ALLKEYS P1: does this keyboard still accept the LED SET_REPORT? Latched FALSE by the first
    /// refusal and never retried.
    ///
    /// This is a COST bound, not a correctness one, and the cost it bounds is severe. A STALL on a
    /// control request halts EP0, after which every later request to the device just runs out the
    /// `hw_wait_budget()` — about two seconds. Enumeration is long over by the time a lock key is
    /// pressed, so a halted EP0 harms nothing else on this device; but WITHOUT this latch a
    /// keyboard with no settable Output report would spend ~2 s inside `service` on EVERY press of
    /// Caps Lock, stalling the whole main loop each time. The operator would see the machine freeze
    /// for two seconds whenever they touched the key — far worse than the dark LED being fixed.
    ///
    /// One failed transfer per keyboard per boot is the whole exposure. The CASE half is completely
    /// unaffected: `kbd_leds` is toggled by the decoder before this is ever consulted, so caps lock
    /// keeps changing the case of what is typed on a keyboard whose LED cannot be driven.
    kbd_led_ok: bool,
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
    /// KBDWIT-2 — service passes that found this endpoint's qTD still ACTIVE. The denominator for
    /// [`IntEp::kbdwit_walks`]; a poll count, not a time.
    #[cfg(feature = "kbdwit")]
    kbdwit_polls: u32,
    /// KBDWIT-2 — of those passes, the ones on which the CONTROLLER's split-progress words moved.
    ///
    /// This is the term the s58 question actually turns on and the one the original dump could not
    /// supply. `overlay[4]`/`overlay[5]` are qTD buffer pointers 1 and 2, which for a split
    /// transaction carry C-prog-mask and FrameTag/S-bytes (EHCI 1.0 §3.5.4) — words only the host
    /// controller writes, and only while it is executing start-/complete-splits against THIS queue
    /// head. Sampling them once says nothing (any value could be residue); sampling them every poll
    /// and counting the CHANGES separates the two states the deadline dump conflates:
    ///
    ///   * `walks > 0` — the controller is traversing to this QH and transacting on the wire every
    ///     few frames. An endpoint with `reports=0` and `walks>0` is being polled and answered with
    ///     NAK: the host side is doing its job and the silence is device-side or stimulus-side.
    ///   * `walks == 0` over thousands of polls — the controller never touches this QH. Orphaned
    ///     from the frame list, unreachable through the TT, or a schedule that is not traversing
    ///     it. THAT is a host-side fault and it convicts without needing anyone to press a key.
    ///
    /// Aliasing cannot manufacture a false `walks=0`: FrameTag is the frame number's low bits at
    /// each start-split and C-prog-mask advances within a frame, so a stationary pair across a
    /// multi-second window of ~1 kHz polls is not a sampling artefact. It can slightly under-count
    /// (two polls inside one frame see one change), which costs precision on the ratio and never
    /// the zero/non-zero verdict this exists for.
    #[cfg(feature = "kbdwit")]
    kbdwit_walks: u32,
    /// KBDWIT-2 — the previous poll's `(overlay[4], overlay[5])`, packed, for the change test.
    #[cfg(feature = "kbdwit")]
    kbdwit_split_prev: u64,
    /// KBDWIT-2 — OR of every split-progress word pair observed. Prints the bits the controller
    /// actually set, so `walks=0` can be read against "and it never set a single progress bit
    /// either" rather than against a change count alone.
    #[cfg(feature = "kbdwit")]
    kbdwit_split_or: u64,
    /// KBDWIT-2 — the `SILENCE-BROKE` one-shot. At most one such line per endpoint per boot.
    #[cfg(feature = "kbdwit")]
    kbdwit_broke: bool,
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
// The line carries TWO cuts of the same boot and they are not addable. `[]` is the partition —
// disjoint phase classes that sum to `enum`. `{}` (M7: `xfer`/`ass`/`act`) is an OVERLAPPING
// view of the transport, which runs inside every one of those classes. Adding a `{}` term to
// the `[]` sum double-counts; the braces are there so a reader cannot do it by accident.
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
    // ── EPACE-TRIM M7 (GR19) — the OVERLAPPING transport view ────────────────────────────────
    // These three are NOT members of the `[]` bracket and must never be added to it. A control
    // transfer runs *inside* whichever class happens to be open — hubpwr's PORT_POWER writes,
    // hubrst's GET_PORT_STATUS polls, hidcfg's descriptor reads, resid's addressing traffic —
    // so this is a second, crosscutting cut of the SAME milliseconds. It is printed in `{}` to
    // keep the two views visually un-addable, and it exists to answer one question the `[]`
    // view cannot: of the ~54 ms of pure wire time inside `enum` on the s73 baseline, how much
    // is the device answering and how much is this driver's own per-stage ASE toggle?
    //   `xfer` — wall time inside `control()`, every EP0 transfer in the driver, with `n=` the
    //            transfer count. Charged on BOTH transports (chain and overlay-direct).
    //   `ass`  — the two bounded USBSTS.ASS handshakes overlay_txn runs per stage (ASE 0→1 and
    //            1→0). Overlay-direct ONLY: on QEMU's chain path this stays 0 while `xfer`
    //            counts, which is the honest reading, not a broken counter.
    //   `act`  — the bounded wait for the overlay token's Active bit to clear, i.e. the device
    //            and the wire. Overlay-direct only, same caveat.
    // `xfer - ass - act` is the driver-side setup/teardown and the serial cost of any witness
    // line a failing transfer prints.
    xfer_cy: u64,
    xfer_n: u32,
    ass_cy: u64,
    act_cy: u64,
    /// EPACE-TRIM M8 — how many single control transfers crossed the `M8_SLOW_MS` threshold on
    /// this controller. Counts every crossing, including the ones past the `M8_SLOW_CAP` print
    /// cap, so the cap can never turn a flood into a silence.
    slow_n: u32,
}

impl Epace {
    const fn new() -> Self {
        Epace {
            cy: [0; N_EPACE],
            n: [0; N_EPACE],
            xfer_cy: 0,
            xfer_n: 0,
            ass_cy: 0,
            act_cy: 0,
            slow_n: 0,
        }
    }
    /// Close a span opened at `t0` (a `now_cycles()` reading) into class `class`.
    fn add(&mut self, class: usize, t0: u64) {
        self.cy[class] = self.cy[class]
            .wrapping_add(crate::arch::now_cycles().wrapping_sub(t0));
        self.n[class] = self.n[class].saturating_add(1);
    }
}

/// M7 — close a span opened at `t0` into one of the overlapping transport accumulators. Free
/// function rather than an `Epace` method so it can be called while `self` is otherwise borrowed.
#[inline]
fn epace_accum(slot: &mut u64, t0: u64) {
    *slot = slot.wrapping_add(crate::arch::now_cycles().wrapping_sub(t0));
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

/// EPACE-TRIM M8 — the per-transfer anomaly threshold, in milliseconds. See the rationale on
/// `Controller::slow_xfer_witness`; in one line, it is ~62× the measured healthy per-transfer
/// cost on metal (0.13 ms), 2.1× under the most diluted form of the anomaly it must catch
/// (52 ms / 3 transfers = 17 ms), and ~2× above the worst transfer QEMU was measured to produce.
const M8_SLOW_MS: u64 = 8;
/// Print cap per controller per boot. Crossings past it are counted, never printed — the FTDI
/// console is a boot-time cost (~0.19 s of drain), so a pathological device must not flood it.
const M8_SLOW_CAP: u32 = 8;

/// `M8_SLOW_MS` in TSC cycles. Same shape as `ehci_scout::ms_cycles` (private there), including
/// the pre-calibration fallback, so the threshold means the same thing this driver's settles do.
fn m8_threshold_cy() -> u64 {
    let hz = crate::arch::x86_64::apic::tsc_hz();
    if hz != 0 {
        hz.saturating_mul(M8_SLOW_MS) / 1000
    } else {
        2_300_000u64.saturating_mul(M8_SLOW_MS) // ~2.3e6 cycles/ms fallback
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
    /// BT-L0 — the ACTUAL parent (hub address, downstream port) of the device currently being
    /// enumerated, as opposed to the split-transaction TT it is reached through. They are the
    /// same thing only below a high-speed hub, which is precisely the assumption the recon's
    /// §3 note 1 caught the M1 witness making. Carried on the controller rather than added to
    /// `enumerate_at_zero`'s signature so the knob-off build is untouched: the field, its one
    /// writer in `bring_up_hub` and its one reader in the M1 witness all vanish with the knob.
    /// `(0, 0)` at depth 0, where the root witness prints no parent at all.
    #[cfg(feature = "bt")]
    bt_parent: (u8, u8),
    /// MTFIX — has this controller's single `bt_slot` already been handed to a radio? The slot's
    /// QH stays linked in the periodic chain for the life of the boot, so it can be armed exactly
    /// once; see `bt_arm_events`.
    #[cfg(feature = "bt")]
    bt_evt_armed: bool,
}

// Raw pointers to identity-mapped DMA memory; access is serialized by the EHCI_HID mutex.
unsafe impl Send for Controller {}

/// BT-L0 — `HCI_Reset`: OGF 0x03 (Controller & Baseband) / OCF 0x0003 => opcode 0x0C03. Zero
/// parameters. A ROM-level command: it answers before any Broadcom patchram (`.hcd`) blob is
/// loaded, which is what lets this arc test the recon's P7 for free.
#[cfg(feature = "bt")]
const BT_HCI_RESET: u16 = 0x0C03;
/// BT-L0 — `HCI_Read_Local_Version_Information`: OGF 0x04 (Informational) / OCF 0x0001 =>
/// opcode 0x1001. Zero parameters; 9 return parameters. Also ROM-level.
#[cfg(feature = "bt")]
const BT_HCI_READ_LOCAL_VERSION: u16 = 0x1001;
/// BT-L1 — `HCI_Read_BD_ADDR`: OGF 0x04 (Informational) / OCF 0x0009 => opcode 0x1009. Zero
/// parameters; returns status(1) + BD_ADDR(6, little-endian). THE identity read a stack starts
/// from — the radio's own public device address. Mandatory command, present before any patchram.
#[cfg(feature = "bt")]
const BT_HCI_READ_BD_ADDR: u16 = 0x1009;
/// BT-L1 — `HCI_Read_Buffer_Size`: OGF 0x04 / OCF 0x0005 => opcode 0x1005. Zero parameters;
/// returns status(1) + HC_ACL_Data_Packet_Length(2) + HC_SCO_Data_Packet_Length(1) +
/// HC_Total_Num_ACL_Data_Packets(2) + HC_Total_Num_SCO_Data_Packets(2) = 8 return bytes. The
/// ACL/SCO packet length + count any future data path sizes its flow control from. Mandatory.
#[cfg(feature = "bt")]
const BT_HCI_READ_BUFFER_SIZE: u16 = 0x1005;
/// BT-L1 — `HCI_Read_Local_Supported_Features`: OGF 0x04 / OCF 0x0003 => opcode 0x1003. Zero
/// parameters; returns status(1) + LMP_Features(8). The "LE Supported (Controller)" bit is
/// byte 4 bit 6 (mask 0x40; BlueZ `LMP_LE`) and "BR/EDR Not Supported" is byte 4 bit 5 (0x20;
/// `LMP_NO_BREDR`). L0 read BT 4.0 from the version; this PROVES LE from the feature mask rather
/// than inferring it from the core spec number. Mandatory.
#[cfg(feature = "bt")]
const BT_HCI_READ_LOCAL_FEATURES: u16 = 0x1003;
/// BT-L1 — `HCI_Read_Local_Supported_Commands`: OGF 0x04 / OCF 0x0002 => opcode 0x1002. Zero
/// parameters; returns status(1) + Supported_Commands(64) = 65 return bytes. That reply is 70
/// bytes on the wire (event header 2 + CmdComplete prefix 3 + 65), which at the event endpoint's
/// 16-byte max packet spans FIVE interrupt-IN transfers — the multi-packet event reassembly this
/// arc adds to `bt_hci_command`. Mandatory.
#[cfg(feature = "bt")]
const BT_HCI_READ_LOCAL_COMMANDS: u16 = 0x1002;
/// BT-L1 — `HCI_Set_Event_Mask`: OGF 0x03 (Controller & Baseband) / OCF 0x0001 => opcode 0x0C01.
/// Eight-byte parameter (the event mask); returns status(1). This arc's FIRST write command.
#[cfg(feature = "bt")]
const BT_HCI_SET_EVENT_MASK: u16 = 0x0C01;
/// BT-L1 — the event mask this arc writes: the Bluetooth Core RESET DEFAULT
/// (0x0000_1FFF_FFFF_FFFF — events through bit 44), little-endian on the wire. Writing the
/// controller's OWN reset default is the safest possible first write: it re-affirms current state,
/// so the command is idempotent and cannot disturb bring-up, while still exercising the
/// CommandComplete write path end to end. The mask is not persistent hardware state (an HCI_Reset
/// restores it), so nothing is left changed for a later boot.
#[cfg(feature = "bt")]
const BT_EVENT_MASK: [u8; 8] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x1F, 0x00, 0x00];
/// BT-L2 — the event mask an LE SCAN needs, and the reason L2 must rewrite one L1 already wrote.
///
/// L1's value is the Bluetooth Core RESET DEFAULT, and the reset default does **not** include
/// **LE Meta Event (bit 61)** — every LE Advertising Report is delivered as an LE Meta Event
/// (event code 0x3E), so with the default mask a scan runs, finds devices, and reports *nothing*:
/// a clean, silent, entirely wrong "no devices found". Bit 61 lives in octet 7 (bits 56-63) at
/// bit 5 => 0x20, giving 0x2000_1FFF_FFFF_FFFF, little-endian on the wire. Everything the reset
/// default enabled stays enabled; this only ADDS the LE meta channel.
///
/// PROVENANCE FOR A LATER ARC — L2 does NOT put this mask back. When the scan ends, the widened
/// mask (LE Meta enabled) is left in place on the controller and the event ENDPOINT is quiesced,
/// so the controller has a channel it may emit on and nothing is reading it. That combination is
/// harmless exactly as long as the endpoint stays quiesced: the qTD is inactive, so an LE Meta
/// Event has nowhere to land and the controller is not issuing INs. Any arc that RE-ARMS this
/// endpoint inherits the widened mask, not the reset default — it will see LE Meta traffic it did
/// not ask for unless it writes its own `HCI_Set_Event_Mask` first. Narrowing it here instead
/// would cost another command round-trip on every boot to undo a state nothing currently reads.
#[cfg(feature = "bt")]
const BT_EVENT_MASK_LE: [u8; 8] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x1F, 0x00, 0x20];
/// BT-L2 — `HCI_LE_Set_Event_Mask`: OGF 0x08 (LE Controller) / OCF 0x0001 => opcode 0x2001.
/// Eight-byte parameter (the LE event mask); returns status(1). The SECOND gate in front of an
/// advertising report: `Set_Event_Mask` bit 61 opens the LE Meta channel, this mask selects which
/// LE sub-events travel down it.
#[cfg(feature = "bt")]
const BT_HCI_LE_SET_EVENT_MASK: u16 = 0x2001;
/// BT-L2 — the LE event mask this arc writes: bits 0..4, the Bluetooth Core reset default for the
/// LE event mask (LE Connection Complete, **LE Advertising Report (bit 1)**, LE Connection Update
/// Complete, LE Read Remote Features Complete, LE Long Term Key Request). Bit 1 is the one this
/// arc needs; the other four are the spec default and are left as the controller already has them,
/// so the write cannot narrow a mask a later arc will want. Deliberately NOT all-ones: bits above
/// 4 are undefined on a 4.0 controller and could earn an Invalid-HCI-Parameters status.
#[cfg(feature = "bt")]
const BT_LE_EVENT_MASK: [u8; 8] = [0x1F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
/// BT-L2 — `HCI_LE_Set_Scan_Parameters`: OGF 0x08 / OCF 0x000B => opcode 0x200B. Seven parameter
/// bytes: LE_Scan_Type(1) LE_Scan_Interval(2, LE) LE_Scan_Window(2, LE) Own_Address_Type(1)
/// Scanning_Filter_Policy(1). Returns status(1).
#[cfg(feature = "bt")]
const BT_HCI_LE_SET_SCAN_PARAMS: u16 = 0x200B;
/// BT-L2 — `HCI_LE_Set_Scan_Enable`: OGF 0x08 / OCF 0x000C => opcode 0x200C. Two parameter bytes:
/// LE_Scan_Enable(1) Filter_Duplicates(1). Returns status(1). **This is the command that must run
/// on every exit path**: a radio left scanning burns power and floods the event endpoint for the
/// rest of the boot, on the same controller as the internal keyboard and trackpad.
#[cfg(feature = "bt")]
const BT_HCI_LE_SET_SCAN_ENABLE: u16 = 0x200C;
/// BT-L2 — scan type 0x00 = PASSIVE. A passive scanner listens only; it never transmits SCAN_REQ,
/// so it cannot be observed by the devices it discovers and cannot collide on the advertising
/// channels. The cost is that SCAN_RSP payloads (where some devices put their name) are not
/// solicited — names then come only from the advertising PDU itself. That is the right trade for a
/// bring-up arc: discovery must not perturb the room.
#[cfg(feature = "bt")]
const BT_LE_SCAN_TYPE_PASSIVE: u8 = 0x00;
/// BT-L2 — scan interval and window, in units of 0.625 ms (Bluetooth Core, Vol 2 Part E). Both are
/// 0x0060 = 96 => **60 ms**, and window == interval means CONTINUOUS scanning: the radio listens
/// 100 % of the time inside the bounded window, hopping to the next advertising channel each
/// interval. Why these numbers:
///   * continuous (window == interval) is the only duty cycle that makes a *short* bounded window
///     honest — at 50 % duty a device could advertise entirely inside our deaf half and the arc
///     would report an empty room it never actually listened to;
///   * 60 ms per channel rotates all three advertising channels (37/38/39) in 180 ms, so a
///     `BT_L2_SCAN_MS`-long window covers each channel several times over;
///   * it is comfortably inside the spec range 0x0004..=0x4000 and is a value real stacks use.
#[cfg(feature = "bt")]
const BT_LE_SCAN_INTERVAL: u16 = 0x0060;
#[cfg(feature = "bt")]
const BT_LE_SCAN_WINDOW: u16 = 0x0060;
/// BT-L2 — own address type 0x00 = PUBLIC. The radio's own BD_ADDR (the one L1 read) is used as
/// the scanner address. Passive scanning never transmits, so this field selects nothing that goes
/// on air here; public is the honest declaration and matches the address L1 witnessed.
#[cfg(feature = "bt")]
const BT_LE_OWN_ADDR_PUBLIC: u8 = 0x00;
/// BT-L2 — scanning filter policy 0x00 = accept all advertising packets (no white list). The white
/// list is empty on a freshly reset controller, so any other policy would filter everything out.
#[cfg(feature = "bt")]
const BT_LE_SCAN_FILTER_ALL: u8 = 0x00;
/// BT-L2 — HCI event code for an LE Meta Event (Bluetooth Core, Vol 4 Part E) and the subevent
/// code for LE Advertising Report.
#[cfg(feature = "bt")]
const BT_EVT_LE_META: u8 = 0x3E;
#[cfg(feature = "bt")]
const BT_LE_SUBEVT_ADV_REPORT: u8 = 0x02;
/// BT-L2 — AD structure types carrying a device name (Bluetooth Core Supplement / Assigned
/// Numbers, Generic Access Profile): 0x08 Shortened Local Name, 0x09 Complete Local Name.
#[cfg(feature = "bt")]
const BT_AD_NAME_SHORT: u8 = 0x08;
#[cfg(feature = "bt")]
const BT_AD_NAME_COMPLETE: u8 = 0x09;
/// BT-L2 — the BOUNDED scan window, in milliseconds of wall clock. 500 ms is the whole of what
/// this arc costs the boot beyond a handful of control transfers, and it is chosen against the
/// advertising intervals real devices use: connectable-discoverable advertisers (phones, watches,
/// headphones in pairing or background mode) sit in the 20-300 ms band, so a 500 ms continuous
/// listen sees each of them several times, while a device on a 1.28 s low-power interval may be
/// missed — which is why the rollup reports a WINDOW, never a room. Enlarging it buys diminishing
/// discovery for linear boot time; the constant is here so that trade is a one-line decision.
#[cfg(feature = "bt")]
const BT_L2_SCAN_MS: u64 = 500;
/// BT-L2 — cap on DISTINCT devices held in the scan table (and therefore on witness lines). Bench
/// rooms with a dozen live radios are ordinary; 16 covers that with slack. Reports for a
/// seventeenth distinct address are COUNTED and the rollup says the table truncated — silent
/// truncation would read as "that is all there was".
#[cfg(feature = "bt")]
const BT_L2_MAX_DEV: usize = 16;
/// BT-L2 — cap on the local-name bytes kept per device. AD names run to 29 bytes; 24 keeps the
/// witness line one serial line without eliding the distinguishing part of a real name. A name cut
/// at the cap is printed with a trailing `~`.
#[cfg(feature = "bt")]
const BT_L2_NAME_MAX: usize = 24;

// ---------------------------------------------------------------------------------------------
// BT-L3 — CONNECT to one LE peer, and always let go.
//
// L3 runs AFTER L2's mandatory `HCI_LE_Set_Scan_Enable(disable)` has been CONFIRMED. Initiating
// while a scan is enabled is a state this arc declines to enter: the Core spec permits a
// controller to refuse `HCI_LE_Create_Connection` with Command Disallowed (0x0C) while scanning,
// and a refusal there would be indistinguishable from a controller that cannot connect at all.
// So the order is: scan -> disable (confirmed) -> connect -> disconnect.
// ---------------------------------------------------------------------------------------------

/// BT-L3 — `HCI_LE_Create_Connection`: OGF 0x08 (LE Controller) / OCF 0x000D => opcode 0x200D.
/// Twenty-five parameter bytes (see `bt_l3_connect`). **It does NOT return a Command Complete** —
/// it returns a **Command Status** (event 0x0F), because the command's real result arrives later
/// as an `LE Connection Complete` meta event. That single fact is why L3 cannot reuse
/// `bt_hci_command_ex`, which matches only event 0x0E, and why `bt_l3_await` exists.
#[cfg(feature = "bt")]
const BT_HCI_LE_CREATE_CONN: u16 = 0x200D;
/// BT-L3 — `HCI_LE_Create_Connection_Cancel`: OGF 0x08 / OCF 0x000E => opcode 0x200E. Zero
/// parameters; returns status(1) in a **Command Complete**. THE command that makes L3 safe: an
/// issued-but-unresolved `Create_Connection` leaves the controller in the Initiating state, and a
/// controller in that state refuses further LE commands with Command Disallowed for the rest of
/// the boot. Every path that issued a create and did not see it resolve must cancel.
#[cfg(feature = "bt")]
const BT_HCI_LE_CREATE_CONN_CANCEL: u16 = 0x200E;
/// BT-L3 — `HCI_Disconnect`: OGF 0x01 (Link Control) / OCF 0x0006 => opcode 0x0406. Three
/// parameter bytes: Connection_Handle(2, LE) Reason(1). Like Create_Connection it answers with a
/// **Command Status**; the real result is the `Disconnection Complete` event (0x05).
#[cfg(feature = "bt")]
const BT_HCI_DISCONNECT: u16 = 0x0406;
/// BT-L3 — disconnect reason 0x13 = Remote User Terminated Connection. The Core spec restricts
/// the reason a host may send on `HCI_Disconnect` to a short list (0x05, 0x13-0x15, 0x1A, 0x29,
/// 0x3B); 0x13 is the ordinary "we are done with this link" value and is what the peer's stack
/// will surface to its own user as a clean teardown rather than a supervision-timeout loss.
#[cfg(feature = "bt")]
const BT_HCI_REASON_REMOTE_USER_TERM: u8 = 0x13;
/// BT-L3 — HCI event codes: Command Status, and Disconnection Complete.
#[cfg(feature = "bt")]
const BT_EVT_CMD_STATUS: u8 = 0x0F;
#[cfg(feature = "bt")]
const BT_EVT_DISCONN_COMPLETE: u8 = 0x05;
/// BT-L3 — LE Meta subevent 0x01 = LE Connection Complete.
///
/// IT IS ALREADY ENABLED, and no new mask write is needed. L2 wrote `HCI_LE_Set_Event_Mask` =
/// 0x1F, i.e. bits 0..4 of the LE event mask; bit **0** is LE Connection Complete (bit 1 is LE
/// Advertising Report, which is the one L2 needed, bits 2/3/4 are Connection Update Complete,
/// Read Remote Features Complete and Long Term Key Request). L2 took the LE reset default whole
/// rather than the single bit it wanted, which is exactly why L3 inherits a usable channel. The
/// outer `HCI_Set_Event_Mask` bit 61 (LE Meta) that carries all of them was likewise widened by
/// L2 and is not narrowed on the way out. L3 therefore adds ZERO mask writes and states that
/// inheritance in its first witness line rather than re-writing a mask to be sure.
#[cfg(feature = "bt")]
const BT_LE_SUBEVT_CONN_COMPLETE: u8 = 0x01;
/// BT-L3 — the advertising PDU type L3 will connect to: `ADV_IND` (Event_Type 0x00) only.
///
/// Of the five Event_Type values an advertising report can carry, only two are connectable at all:
/// `ADV_IND` (0x00, connectable undirected) and `ADV_DIRECT_IND` (0x01, connectable **directed**).
/// `ADV_SCAN_IND` (0x02) and `ADV_NONCONN_IND` (0x03) are non-connectable by definition, and
/// `SCAN_RSP` (0x04) is not an advertisement at all. `ADV_DIRECT_IND` is excluded on purpose: it
/// names an initiator address in its payload, and that address is not ours — connecting to a
/// device that is actively soliciting a *different* peer is an intrusion, and the controller would
/// in any case ignore our CONNECT_IND. So: 0x00, and nothing else.
#[cfg(feature = "bt")]
const BT_L3_ADV_CONNECTABLE: u8 = 0x00;

// ============================ WHO L3 IS ALLOWED TO CONNECT TO ==============================
// Picking the first `ADV_IND` heard REACHES INTO THE ROOM. On a bench with neighbours that is a
// stranger's phone, a tracker, or — the sharp case — another machine's BLE keyboard or mouse,
// which a CONNECT_IND takes away from its owner for as long as the link is held. Two independent
// reviews reached the same conclusion, so the mechanism is built here and defaulted safe.

/// BT-L3 — **PEER NAME FILTER.** When `Some(s)`, L3 connects only to a peer whose ADVERTISED LOCAL
/// NAME contains `s` as a case-insensitive substring; every other candidate is counted and
/// witnessed as skipped. When `None`, selection falls back to first-heard filtered by
/// `BT_L3_RSSI_FLOOR` below.
///
/// RULING — white board Q6, answered: **the bench connects to Peter's own speaker, an Ultimate Ears
/// MEGABOOM.** By NAME rather than by BD_ADDR, because the address is not known and a name filter
/// is what makes the run reproducible for whoever is at the bench — turn the speaker on, boot,
/// and it is the peer. Changing the target is ONE EDIT OF ONE LINE.
///
/// WHAT THIS DEPENDS ON, stated because it is the thing that can make a correct build connect to
/// nothing: the name must be in an AD structure this arc can HEAR. L2 scans PASSIVELY — it sends no
/// SCAN_REQ — so a name that a device carries only in its SCAN_RSP (`Event_Type` 0x04) is reachable
/// here only when some OTHER nearby device solicits it and our controller happens to be listening.
/// The name decode is L2's, unchanged and shared (the `BT_AD_NAME_COMPLETE`/`BT_AD_NAME_SHORT` walk
/// in `bt_le_drain`), and it accepts a name from ANY report type — so a scan response overheard in
/// the window does supply one. What this arc will NOT do is switch to an ACTIVE scan to guarantee
/// it: active scanning TRANSMITS a SCAN_REQ to every advertiser in the room, which is a larger
/// decision than this arc's brief and is Peter's to make.
///
/// Matching is on the name as DECODED AND CAPPED at `BT_L2_NAME_MAX` (24 bytes). A device whose
/// name is longer than that and whose match would fall past the cut is reported as cut (`~(cut)`)
/// on its witness line, so a false miss is visible rather than silent.
#[cfg(feature = "bt")]
const BT_L3_PEER_NAME: Option<&str> = Some("MEGABOOM");

/// BT-L3 — RSSI floor in dBm, applied ONLY when `BT_L3_PEER_NAME` is `None` (a name filter already
/// names its peer, and the right peer across the room is still the right peer).
///
/// -60 dBm is roughly arm's length to a couple of metres for a typical BLE advertiser: it admits a
/// device on the bench and excludes most of what is merely in the building. It is a MITIGATION,
/// not a guarantee — RSSI is not distance, a high-power advertiser two rooms away can clear it and
/// a shielded one on the desk can fail it. It is worth having on its own terms because the failure
/// it prevents is the one that matters: silently connecting to the loudest stranger.
///
/// An advertising report may report RSSI as 127 = NOT AVAILABLE. A floor cannot be applied to an
/// unknown value, and admitting unknowns would make the rule decorative, so 127 is SKIPPED and
/// counted under its own name.
#[cfg(feature = "bt")]
const BT_L3_RSSI_FLOOR: i8 = -60;
/// BT-L3 — the RSSI value an advertising report uses for "not available" (Bluetooth Core).
#[cfg(feature = "bt")]
const BT_L3_RSSI_NA: i8 = 127;

// BT-L3 — the per-candidate verdicts, one per distinct device, printed on that device's own L2
// witness line. They exist so a capture answers "why not that one?" for EVERY device in the room:
// a peer that was not selected is otherwise indistinguishable from a peer that was never heard.
#[cfg(feature = "bt")]
const BT_L3_V_NOT_CONNECTABLE: &str = "not-connectable(no ADV_IND heard from it)";
#[cfg(feature = "bt")]
const BT_L3_V_ATYPE: &str = "SKIP:identity-address-type(0x02/0x03 cannot go in a create)";
#[cfg(feature = "bt")]
const BT_L3_V_NO_NAME: &str = "SKIP:no-name-advertised";
#[cfg(feature = "bt")]
const BT_L3_V_NAME_MISMATCH: &str = "SKIP:name-mismatch";
#[cfg(feature = "bt")]
const BT_L3_V_RSSI_NA: &str = "SKIP:rssi-unavailable(the floor cannot be applied)";
#[cfg(feature = "bt")]
const BT_L3_V_BELOW_FLOOR: &str = "SKIP:below-rssi-floor";
#[cfg(feature = "bt")]
const BT_L3_V_SELECTED: &str = "SELECTED";
#[cfg(feature = "bt")]
const BT_L3_V_ALSO_MATCHED: &str = "also-matched(another device answers the same name; not used)";
/// BT-L3 — connection interval min/max, in units of 1.25 ms (Core range 0x0006..=0x0C80, i.e.
/// 7.5 ms..4.0 s). 0x0018 = 24 => **30 ms**, 0x0028 = 40 => **50 ms**. A range rather than a point
/// so the peer's controller can pick something it already runs; 30-50 ms is the ordinary
/// interactive band (it is what a keyboard or a watch negotiates) and is short enough that the
/// link is established and torn down inside L3's bounded window. Min <= Max, as the spec requires.
#[cfg(feature = "bt")]
const BT_L3_CONN_INTERVAL_MIN: u16 = 0x0018;
#[cfg(feature = "bt")]
const BT_L3_CONN_INTERVAL_MAX: u16 = 0x0028;
/// BT-L3 — slave latency, in connection events (Core range 0..=0x01F3, further constrained by
/// `(1 + latency) * interval_max * 2 <= timeout`). **Zero**: L3 holds the link for milliseconds,
/// so there is no power to save by letting the peer skip events, and zero removes the constraint
/// interaction entirely.
#[cfg(feature = "bt")]
const BT_L3_CONN_LATENCY: u16 = 0x0000;
/// BT-L3 — supervision timeout, in units of 10 ms (Core range 0x000A..=0x0C80, i.e. 100 ms..32 s).
/// 0x0064 = 100 => **1000 ms**. The spec's constraint is
/// `timeout > (1 + latency) * interval_max * 2`; with latency 0 and interval_max 50 ms that floor
/// is 100 ms, so 1000 ms clears it by 10x. It is deliberately NOT the minimum: a timeout at the
/// floor makes an ordinary retransmission look like a dropped link, and a spurious
/// connection-timeout would be reported by this arc as a peer failure it did not cause.
#[cfg(feature = "bt")]
const BT_L3_SUPERVISION_TIMEOUT: u16 = 0x0064;
/// BT-L3 — Minimum/Maximum_CE_Length, in units of 0.625 ms (Core range 0x0000..=0xFFFF). Both
/// **zero** = no preference; the controller sizes each connection event itself. This arc moves no
/// ACL data, so any CE length we asked for would be an invented constraint on a controller that
/// knows its own scheduling better than we do.
#[cfg(feature = "bt")]
const BT_L3_CE_LENGTH_MIN: u16 = 0x0000;
#[cfg(feature = "bt")]
const BT_L3_CE_LENGTH_MAX: u16 = 0x0000;
/// BT-L3 — bounded wait for a **local** answer (a Command Status, or a Command Complete for the
/// cancel), in ms. These events are generated by the controller itself with no air time involved,
/// so a controller that has not answered in 300 ms is not busy, it is not answering.
#[cfg(feature = "bt")]
const BT_L3_CMD_MS: u64 = 300;
/// BT-L3 — bounded wait for `LE Connection Complete` after the create is accepted, in ms.
///
/// This one DOES include air time. While initiating, the controller scans continuously (window ==
/// interval, the same 60 ms L2 uses) and sends CONNECT_IND on the first matching `ADV_IND` it
/// hears, so establishment normally costs one advertising interval — 20-300 ms for the devices
/// this arc can see at all, since the peer was selected from a report heard inside L2's 500 ms
/// window. 1200 ms covers the slowest of those five times over. Beyond it the honest reading is
/// that the peer stopped advertising between L2's scan and L3's create, which is a real and
/// ordinary outcome, not a bug — and the cancel path exists precisely for it.
#[cfg(feature = "bt")]
const BT_L3_CONN_MS: u64 = 1200;
/// BT-L3 — bounded wait for `Disconnection Complete` after the disconnect is accepted, in ms. A
/// teardown is one LL_TERMINATE_IND on the next connection event; at the 30-50 ms interval
/// negotiated above, 600 ms is more than ten events.
#[cfg(feature = "bt")]
const BT_L3_DISC_MS: u64 = 600;
/// BT-L3 — structural cap on events drained while awaiting ONE specific event, on top of each
/// wait's wall-clock deadline. Same role as `BT_EVT_MAX` for commands: a controller that streams
/// unrelated events must not let a loop whose per-iteration bound keeps being satisfied run past
/// its window. Larger than `BT_EVT_MAX` because L3 waits through a window in which the controller
/// legitimately emits Command Status, vendor events and (on the cancel path) a second meta event.
#[cfg(feature = "bt")]
const BT_L3_EVT_MAX: u32 = 16;

/// BT-L1 — reassembly cap for one HCI event that spans multiple event-endpoint packets. The event
/// endpoint's max packet is 16 B (census: `IN1/int/16`), but an HCI event runs up to 2 + 255 B;
/// the USB transport delivers it as ceil(len/mps) interrupt-IN transfers. 260 covers the largest
/// defined event (an LE Advertising Report) with headroom, so L2 (LE scan) extends without
/// resizing. An event whose declared length exceeds this is reported truncated and the reassembler
/// BREAKS WITHOUT DRAINING the remainder — so the toggle's relationship to the device is lost and the
/// caller must stop issuing commands, not continue. (Review C2: an earlier comment here claimed the
/// remainder was drained to keep sync; it is not, and the two `trunc` branches show it. Unreachable
/// today — the largest possible event is 2+255=257 < 260 — but whoever resizes this cap for L2 must
/// either add the drain or keep honouring the stop.)
#[cfg(feature = "bt")]
const BT_EVT_ASM_MAX: usize = 260;
/// BT-L0 — HCI event code for Command Complete (Bluetooth Core, Vol 4 Part E).
#[cfg(feature = "bt")]
const BT_EVT_CMD_COMPLETE: u8 = 0x0E;
/// BT-L0 — the Bluetooth SIG company identifier for Broadcom. THE deliverable of this arc: a
/// value that cannot be produced by our own code, by a timing artefact, or by a hopeful
/// default — it can only have come off the radio.
#[cfg(feature = "bt")]
const BT_MFG_BROADCOM: u16 = 0x000F;
/// BT-L0 — structural cap on events drained while awaiting one Command Complete. This is the
/// SECOND bound on that loop (each individual read already carries `wait_bounded`'s deadline);
/// it exists so a controller that streams unrelated events cannot spin the boot indefinitely
/// inside a loop whose per-iteration bound would keep being satisfied.
#[cfg(feature = "bt")]
const BT_EVT_MAX: u32 = 8;
/// BT-L0B — how many bytes of a candidate's CONFIGURATION descriptor this driver will read.
/// Exactly `qh::Buf256`, the EP0 data-stage buffer: the descriptor is read into `data_buf` and
/// walked in place, so the buffer's size IS the bound. A Bluetooth composite's wTotalLength is
/// ~180-220 B on the devices this arc targets; anything past 256 B is reported as truncated by
/// the census rather than silently dropped.
#[cfg(feature = "bt")]
const BT_CFG_MAX: u16 = 256;
/// BT-L0B — bound on the interface CENSUS: interface descriptors recorded and printed. Every
/// alternate setting counts as one entry (that is the point — the alt fan-out of the SCO
/// interface is exactly the descriptor-layout question the census exists to answer). Twelve
/// covers a 4-interface BT composite with a 6-alt SCO interface with room to spare; past that
/// the census says how many it dropped.
#[cfg(feature = "bt")]
const BT_CENSUS_MAX: usize = 12;
/// BT-L0B — bound on endpoints listed per interface in the census line.
#[cfg(feature = "bt")]
const BT_EP_MAX: usize = 8;

/// BT-L0B — one interface descriptor as the census/selection walk sees it.
///
/// `int_in`/`bulk_in`/`bulk_out` are the SELECTION evidence (endpoint numbers, 0 = absent);
/// `eps` is the CENSUS evidence (every endpoint of this interface, in descriptor order,
/// verbatim). The two are kept apart on purpose: selection must never quietly depend on a field
/// the printed line does not show.
#[cfg(feature = "bt")]
#[derive(Clone, Copy, Default)]
struct BtIntf {
    num: u8,
    alt: u8,
    cls: u8,
    sub: u8,
    pro: u8,
    neps: u8,
    int_in: u8,
    int_mps: u16,
    int_iv: u8,
    bulk_in: u8,
    bulk_out: u8,
    /// (bEndpointAddress, bmAttributes & 0x3, wMaxPacketSize & 0x7FF)
    eps: [(u8, u8, u16); BT_EP_MAX],
    nep: u8,
}

/// BT-L0B — the CANDIDATE GATE, run on the 64-byte view `configure_hid` already holds.
///
/// Purely a filter, and deliberately loose: it decides only whether this device is worth ONE
/// extra EP0 transfer (the full-descriptor re-read) plus a census. It is NOT the claim rule —
/// `bt_probe`'s evidence-based selection is. Loose means: subclass 0x01 (RF) + protocol 0x01
/// (Bluetooth), with the class byte either the spec's 0xE0 (Wireless Controller, Bluetooth
/// Core Vol 4 Part B) or 0xFF (vendor-classed, which is how the Broadcom parts behind Apple's
/// hub present). A HID keyboard is 0x03/0x01/0x01 and is NOT matched by this — the class byte
/// is what excludes it.
///
/// Returns false => `bt_probe` returns immediately with no wire traffic and no output, so every
/// non-candidate device on the bus behaves byte-for-byte as it did before this arc.
#[cfg(feature = "bt")]
fn bt_cfg_has_candidate(cfg: &[u8]) -> bool {
    let mut off = 0usize;
    while off + 2 <= cfg.len() {
        let len = cfg[off] as usize;
        if len == 0 {
            break;
        }
        if cfg[off + 1] == 0x04
            && off + 9 <= cfg.len()
            && cfg[off + 6] == 0x01
            && cfg[off + 7] == 0x01
            && (cfg[off + 5] == 0xE0 || cfg[off + 5] == 0xFF)
        {
            return true;
        }
        off += len;
    }
    false
}

/// BT-L0 — one interrupt-IN endpoint armed for SYNCHRONOUS use.
///
/// Deliberately NOT pushed into `Controller::int_eps`: that list is drained by `service()`,
/// which would run HCI event packets through the HID report decoders. This endpoint is read
/// inline by the L0 sequence and then deactivated. It owns the pool's dedicated `bt_slot` —
/// NOT one of the `MAX_INT_EPS` (6) HID slots — since MTFIX: on Boot AN it consumed the
/// fourth of four HID slots and the internal trackpad, enumerated last, fell off the end.
#[cfg(feature = "bt")]
struct BtEvtEp {
    qh: *mut Qh,
    qtd: *mut Qtd,
    qtd_phys: u64,
    buf: *mut u8,
    buf_phys: u64,
    mps: u16,
}

/// BT-L2 — outcome of ONE reassembled HCI event read off the event endpoint.
///
/// `Idle` is the case that only exists because L2 reads on a DEADLINE rather than on a command:
/// the first packet's budget expired with **the transfer still armed**. The endpoint is still
/// byte-synchronised and the toggle is unadvanced, so the caller may either poll it again or hand
/// it to the next command as pre-armed — what it must NOT do is arm a second transfer over it.
/// `Stop` means the endpoint is no longer usable (halted, or a timeout part-way through an event,
/// which loses the toggle's relationship to the device).
#[cfg(feature = "bt")]
enum BtEvt {
    /// A complete event of `len` bytes sits in the caller's reassembly buffer. `trunc` = the event
    /// declared more than the buffer holds. No transfer is armed.
    Got { len: usize, trunc: bool },
    /// First-packet budget expired. THE TRANSFER IS STILL ARMED; `0` is the qTD token as read.
    Idle(u32),
    /// The EVENT ENDPOINT is unusable — **no further EVENT READ may be issued on it**.
    ///
    /// This forbids reads on the interrupt-IN event endpoint. It does NOT forbid EP0: the
    /// mandatory `HCI_LE_Set_Scan_Enable(disable)` is a control-OUT on a different endpoint, and
    /// it is the write that actually stops the radio, so it still goes out on every path that
    /// could have started a scan. What it may not do is *read the reply*. The two `Stop` causes
    /// are handled differently by `bt_le_scan`:
    ///
    /// * **halted** (`QTD_ERR_MASK`) — the endpoint retired the transfer. The disable is sent with
    ///   `bt_hci_send` alone and witnessed as explicitly UNREAD; no `CommandComplete` is claimed,
    ///   and no stall clear is attempted (this arc does not re-open a halted endpoint).
    /// * **mid-event timeout** — the endpoint is fine, the *event* is lost, and the transfer is
    ///   STILL ARMED (see `bt_read_full_event`). That is the ordinary pre-armed hand-off: the
    ///   disable's `bt_hci_command_ex` consumes the outstanding transfer instead of arming over it.
    Stop,
}

/// BT-L2 — one distinct device seen during the scan window.
///
/// Keyed by (address, address type): a device that changes its resolvable-private address mid-scan
/// is genuinely a different address on the air, and this table reports the air, not a guess about
/// identity. `rssi` is the LATEST report's value (not a peak or an average — an average over an
/// unknown number of channel dwells would be a statistic this arc has not earned).
#[cfg(feature = "bt")]
#[derive(Clone, Copy)]
struct BtDev {
    addr: [u8; 6],
    atype: u8,
    evt: u8,
    rssi: i8,
    name: [u8; BT_L2_NAME_MAX],
    nlen: u8,
    /// Set when the name was cut at `BT_L2_NAME_MAX`.
    ncut: bool,
    reports: u16,
    /// BT-L3 — STICKY: this address was heard advertising CONNECTABLY (`ADV_IND`, `Event_Type`
    /// 0x00) at least once in the window. `evt` above is last-report-wins and cannot answer this:
    /// a device that advertises connectably and then has a scan response overheard would end the
    /// window looking like a `SCAN_RSP` and be refused a connection it was soliciting.
    conn_seen: bool,
}

#[cfg(feature = "bt")]
impl Default for BtDev {
    fn default() -> Self {
        BtDev {
            addr: [0; 6],
            atype: 0,
            evt: 0,
            rssi: 127, // 127 = RSSI not available (Bluetooth Core)
            name: [0; BT_L2_NAME_MAX],
            nlen: 0,
            ncut: false,
            reports: 0,
            conn_seen: false,
        }
    }
}

/// BT-L3 — case-insensitive ASCII substring match, for the peer NAME filter.
///
/// ASCII-only folding on purpose: advertised Local Names are UTF-8 on the air, and a correct
/// Unicode case fold is a table this driver has no business carrying. Every byte outside ASCII is
/// compared verbatim, so a non-ASCII name still matches itself exactly — what it will not do is
/// match a differently-cased form of itself, which is a miss and never a false hit.
///
/// An EMPTY needle matches everything, so the caller must treat `Some("")` as "no filter" rather
/// than let it silently admit the whole room; it is refused explicitly at the call site.
#[cfg(feature = "bt")]
fn bt_name_contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return needle.is_empty();
    }
    let fold = |b: u8| if b.is_ascii_uppercase() { b + 32 } else { b };
    for start in 0..=(hay.len() - needle.len()) {
        let mut all = true;
        for k in 0..needle.len() {
            if fold(hay[start + k]) != fold(needle[k]) {
                all = false;
                break;
            }
        }
        if all {
            return true;
        }
    }
    false
}

/// BT-L3 — which event `bt_l3_await` is looking for. Every L3 wait names its target explicitly
/// rather than "the next event", because the controller legitimately interleaves others: a
/// Command Status for a command we already read, a vendor event, and on the cancel path an
/// `LE Connection Complete` that reports the cancellation.
#[cfg(feature = "bt")]
#[derive(Clone, Copy)]
enum BtL3Want {
    /// Command Status (0x0F) whose echoed Command_Opcode matches.
    CmdStatus(u16),
    /// Command Complete (0x0E) whose echoed Command_Opcode matches.
    CmdComplete(u16),
    /// LE Meta Event (0x3E) with this Subevent_Code.
    LeMeta(u8),
    /// Any event with this event code.
    Evt(u8),
}

/// BT-L3 — outcome of one bounded `bt_l3_await`.
///
/// `Timeout` means the wall-clock window (or the structural event cap) expired without the wanted
/// event. **A transfer may still be armed** — `armed` carries that forward exactly as `BtEvt::Idle`
/// does, and the next `bt_read_full_event` consumes it. `Stop` means the event endpoint is no
/// longer readable (`BtEvt::Stop`: halted, or an event lost mid-reassembly). As in L2, `Stop`
/// forbids further EVENT READS but says nothing about EP0 — which is what lets the mandatory
/// cancel/disconnect still be SENT on a path that can no longer read.
#[cfg(feature = "bt")]
enum BtL3Await {
    /// The wanted event sits in the caller's reassembly buffer, `len` bytes.
    Got(usize),
    Timeout,
    Stop,
}

/// BT-L3 — the facts a wait learns that its CALLER did not ask for, carried across every wait of
/// one L3 run. Three separate defects live here, and they are one structure because they are one
/// problem: `bt_l3_await` walks past every event that is not the `want`, and some of those events
/// are load-bearing.
///
/// * `live_handle` — **THE CANCEL RACE.** The likely ordering of a lost cancel is NOT the one the
///   first cut of this arc handled. Per Core Vol 4 Part E, `HCI_LE_Create_Connection_Cancel`
///   answers with Command Complete **status 0x0C (Command Disallowed)** once the controller is no
///   longer Initiating — which is exactly the state it is in when the connection HAS established.
///   The `LE Connection Complete` (status 0x00, real handle) is then already queued AHEAD of the
///   cancel's Command Complete, so the wait for the Command Complete reads the meta event FIRST,
///   fails the `want` match, and would step over it. Stepping over that event throws away the only
///   handle by which the link could ever be released: `bt_quiesce_events` deactivates the event
///   qTD immediately afterwards, so no `Disconnection Complete` is ever read and the link survives
///   until the PEER's supervision timeout or a power cycle — while the tally certifies
///   `left_outstanding=none`. So: any `LE Connection Complete` with status 0x00 that a wait walks
///   past is LATCHED here, and the teardown consults the latch before it concludes anything.
/// * `resolved_nonzero` — an `LE Connection Complete` with a NONZERO status that was walked past.
///   No link exists, but the create RESOLVED (the controller left the Initiating state to send
///   it), which is the other thing a 0x0C answer to the cancel can mean.
/// * `blind` — set whenever a wait ended without having read its whole window: a truncated event
///   was stepped over undecoded, or the structural `BT_L3_EVT_MAX` cap ended the wait early. It is
///   what makes the *absence* of a latch admissible as evidence or not. Without it, "no connection
///   event was walked past" would be asserted by a loop that may simply have stopped looking.
/// * `stopped` — the event endpoint became unreadable (`BtEvt::Stop`: a halt, or an event lost
///   mid-reassembly). ONCE LATCHED, NO FURTHER READ IS ATTEMPTED. This is not a tidiness rule: a
///   later `bt_read_full_event` sees `armed == false` after a halt cleared it, re-arms, and writes
///   a fresh `QTD_ACTIVE` overlay — which clears the QH's Halted bit while the DEVICE's STALL
///   condition is untouched. The teardown COMMANDS still go out (they ride EP0, which `Stop` says
///   nothing about); only the reads are refused, and the witnesses say so.
#[cfg(feature = "bt")]
#[derive(Clone, Copy, Default)]
struct BtL3State {
    live_handle: Option<u16>,
    resolved_nonzero: bool,
    blind: bool,
    stopped: bool,
}

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

    /// EPACE-TRIM M7 (GR19) — the transport meter. Every EP0 control transfer in this driver
    /// goes through this one function, so this is the only place that can price the transport as
    /// a whole without double-counting. It measures `control_txn` and nothing else: the settles
    /// that bracket transfers at the call sites (T_RSTRCY, SET_ADDRESS recovery, pwr2good) stay
    /// outside, which is what makes `xfer=` subtractable from `resid=` by hand.
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
        let t0 = crate::arch::now_cycles();
        // M8: snapshot the two overlapping stage meters so this transfer's OWN share of them is
        // a subtraction, not a new clock. Two u64 reads; nothing here touches the wire.
        let ass0 = self.pace.ass_cy;
        let act0 = self.pace.act_cy;
        let r = self.control_txn(t, bm_req, b_req, w_value, w_index, w_length, dir_in);
        // One `now_cycles()` read serves both the M7 accumulator and the M8 threshold test —
        // inlining `epace_accum` here keeps the arithmetic bit-identical to what M7 has been
        // printing while avoiding a second rdtsc on every transfer.
        let xfer_cy = crate::arch::now_cycles().wrapping_sub(t0);
        self.pace.xfer_cy = self.pace.xfer_cy.wrapping_add(xfer_cy);
        self.pace.xfer_n = self.pace.xfer_n.saturating_add(1);
        self.slow_xfer_witness(t, bm_req, b_req, w_value, w_index, w_length, xfer_cy, ass0, act0);
        r
    }

    /// EPACE-TRIM M8 (GR18) — the resolution fix the enum46 verdict names as owed.
    ///
    /// M7 proved the `05ac:8510` (controller [0], addr 2, HS, behind the RMH) spends ~52 ms
    /// NAKing across its three enumeration control transfers, and that none of it is ours:
    /// `ass=0`, `wait_bounded` has no poll grain, RL=0 already retries maximally, and both
    /// software settles are pinned to the USB 2.0 minimum. What M7 **cannot** say is WHICH
    /// request eats it — `act_cy` is one per-controller accumulator, so "the time sits in address
    /// assignment" is a window-level inference. That distinction is exactly what decides BUY-2
    /// (dropping the 8-byte MPS0 pre-read for HS targets, which USB 2.0 §5.5.3 makes redundant
    /// at high speed): if the 52 ms lives in `0x80/0x06 GET_DESCRIPTOR(8)`, BUY-2 buys ~52 ms;
    /// if it lives in `0x00/0x05 SET_ADDRESS` or the post-address `GET_DESCRIPTOR(18)`, BUY-2
    /// buys nothing and must not be taken. This function is the measurement and nothing else —
    /// no pacing, no transfer logic, no retry behaviour is touched by it.
    ///
    /// PREDICTION (falsifiable, for the next metal boot): exactly one-ish line, on controller
    /// **[0]**, addr 0 or 2, `05ac:8510`'s window, with `xfer=` at or near 52 ms and `act=`
    /// accounting for essentially all of it — and **zero lines on controller [1]**, whose 82
    /// control transfers cost 11 ms total (0.13 ms each). A line on [1] falsifies the verdict's
    /// central claim (that the 52 ms is one device's own answer latency and not a driver-side
    /// per-transfer cost), and would mean the threshold or the meter is wrong.
    ///
    /// Threshold — 8 ms, chosen to sit in the empty middle of a two-order-of-magnitude gap:
    ///   * healthy per-transfer cost on this driver is **0.13 ms** (controller [1]: 11 ms across
    ///     82 transfers, n=3 boots), so 8 ms is ~62× the healthy mean — no healthy transfer can
    ///     reach it, and healthy boots print ZERO lines. That is what keeps this off the FTDI
    ///     console budget (~0.19 s of boot spent draining it; this must not add to it).
    ///   * the anomaly is 52 ms across at most three transfers. Even the *most* diluted case —
    ///     the NAK time spread perfectly evenly, ~17 ms each — clears 8 ms by 2.1×, so the
    ///     threshold cannot hide the phenomenon it was built to name. (The window holds exactly
    ///     three transfers; there is no dilution past that for the margin to erode.)
    ///   * 8 ms is also far below the 2 s `hw_wait_budget()`, so a transfer that times out is
    ///     reported here too (in addition to its own STOP-NOTE) rather than silently skipped.
    /// Both software settles in the enumeration window (10 ms T_RSTRCY, 2 ms SET_ADDRESS
    /// recovery) sit at the CALL SITES, outside `control()`, so they cannot trip this.
    ///
    /// Why 8 and not the 5 this arc started from — measured, not guessed. The instrument was
    /// falsified both ways before landing, by temporarily moving the two constants:
    ///   * at **1 ms** `./arroyo test` printed all 8 of QEMU's control transfers, the slowest at
    ///     4 ms (chain mode, so `ass=0 act=0` there — the honest reading, not a dead meter). That
    ///     is the distribution 5 ms would have to clear.
    ///   * at **5 ms** two consecutive QEMU runs of the same code disagreed: zero lines, then one
    ///     (`GET_DESCRIPTOR(8)` at 5 ms). A threshold that flaps run-to-run on an unloaded host
    ///     is inside the platform's jitter band, and a spurious QEMU line is worse than useless
    ///     here — a reader could mistake it for the metal finding this instrument exists to make.
    ///   * at **8 ms** QEMU is silent with ~2× headroom over its measured worst transfer, while
    ///     metal keeps 2.1× of margin in the other direction. Both ends of the gap are paid for.
    ///   * with the cap temporarily at 3 the overflow line printed "8 … 3 printed, 5 suppressed",
    ///     so the escape valve is exercised, not merely compiled.
    ///
    /// Bounded output: at most `M8_SLOW_CAP` lines per controller per boot, with `seq=k/cap` in
    /// every line so a truncated capture is self-describing; crossings past the cap are still
    /// counted and reported once by the EPACE summary site. A pathological device therefore
    /// costs a bounded number of serial lines, not a flood.
    #[allow(clippy::too_many_arguments)]
    unsafe fn slow_xfer_witness(
        &mut self,
        t: &Target,
        bm_req: u8,
        b_req: u8,
        w_value: u16,
        w_index: u16,
        w_length: u16,
        xfer_cy: u64,
        ass0: u64,
        act0: u64,
    ) {
        if xfer_cy < m8_threshold_cy() {
            return;
        }
        self.pace.slow_n = self.pace.slow_n.saturating_add(1);
        if self.pace.slow_n > M8_SLOW_CAP {
            return; // counted, not printed — the summary site reports the overflow once.
        }
        let (xv, xu) = epace_fmt(xfer_cy);
        let (av, au) = epace_fmt(self.pace.ass_cy.wrapping_sub(ass0));
        let (cv, cu) = epace_fmt(self.pace.act_cy.wrapping_sub(act0));
        let spd = match t.eps {
            QH_EPS_HIGH => "HS",
            QH_EPS_LOW => "LS",
            _ => "FS",
        };
        // Stage count is exact and free: SETUP + STATUS always, DATA only when wLength > 0
        // (`control_txn` above). The per-stage ass/act splits are the accumulator deltas.
        let stg = if w_length > 0 { 3 } else { 2 };
        serial_println!(
            ":: EHCI-HID: [{}] EPACE-TRIM M8 SLOW-XFER addr={} hub={}.{} spd={} bmreq={:#04x} breq={:#04x} wval={:#06x} widx={:#06x} wlen={} stg={} xfer={}{} act={}{} ass={}{} seq={}/{} == witness ::",
            self.idx, t.addr, t.hub_addr, t.hub_port, spd,
            bm_req, b_req, w_value, w_index, w_length, stg,
            xv, xu, cv, cu, av, au,
            self.pace.slow_n, M8_SLOW_CAP
        );
    }

    /// One synchronous EP0 control transfer through the shared QH (the EHCI analogue of xHCI's
    /// `sync_control`: main-loop context, never inside an interrupt). Returns the transferred
    /// data-stage byte count. Bounded — a wedged Active bit is a traced Err, never a hang.
    unsafe fn control_txn(
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
        // continuous). HS targets get S-mask 0x01 (one start per frame → ~1 ms per control
        // transfer); FS/LS-behind-TT get the same S-mask 0x01 plus the split completion mask
        // (SSPLIT µframe 0, CSPLITs 2-4). The async ring stays programmed-but-disabled (ASE is
        // never set).
        // (BT-L0 instrument note: this block used to open "HS targets get S-mask 0xFF (every
        // µframe...)", which the `masks` expression four lines below has never done — it writes
        // 0x01 for both speed classes, as the probe-14c sentence immediately after this one
        // requires. The 0xFF sentence predated probe-14c and survived it; it is deleted rather
        // than annotated, because a reader checking the S-mask argument against it would
        // conclude the periodic-QH discipline had been violated.)
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
        // EPACE-TRIM M7 (GR19) — the two ASS handshakes and the completion wait are metered
        // separately. The s73 baseline puts ~54 ms of pure control-transfer time inside `enum`,
        // 46 ms of it on a SINGLE device (controller 0's 05ac:8510 at addr 2, ttyUSB0.log L15641
        // → L15642: 58 ms for three transfers, minus 10 ms T_RSTRCY and 2 ms SET_ADDRESS
        // recovery), while the same three transfers against the RMH one tier up cost 2 ms
        // (L15638 → L15639). Two hypotheses fit that, and they want opposite fixes: the ASE
        // 0→1/1→0 toggle this function runs PER STAGE costs a frame boundary each way (EHCI 1.0
        // §4.8.2 lets the controller defer the ASS transition), which would be ~6 handshakes per
        // transfer and is ours to hoist; or the device NAKs its way through address assignment,
        // which is the device's own and not trimmable. `ass` vs `act` separates them. Per the
        // ledger's law an undecomposed constant is not trimmed — so this arc measures it and
        // does not touch the toggle.
        let ass_t0 = crate::arch::now_cycles();
        let ass_on = wait_bounded(|| {
            mmio_read32(self.op + OP_USBSTS).unwrap_or(0) & (1 << 15) != 0
        });
        epace_accum(&mut self.pace.ass_cy, ass_t0);
        let act_t0 = crate::arch::now_cycles();
        let done = wait_bounded(|| {
            core::ptr::read_volatile(&(*qh).overlay[2]) & QTD_ACTIVE == 0
        });
        epace_accum(&mut self.pace.act_cy, act_t0);
        let cmd2 = mmio_read32(self.op + OP_USBCMD).unwrap_or(0);
        let _ = mmio_write32(self.op + OP_USBCMD, cmd2 & !CMD_ASE);
        let ass_off_t0 = crate::arch::now_cycles();
        let ass_off = wait_bounded(|| {
            mmio_read32(self.op + OP_USBSTS).unwrap_or(0) & (1 << 15) == 0
        });
        epace_accum(&mut self.pace.ass_cy, ass_off_t0);
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
        //
        // BUY-2 (GR18) — **high speed does not need this request at all.** USB 2.0 §5.5.3 fixes
        // the default control pipe's maximum data payload at 64 bytes for a high-speed device;
        // there is no other legal MPS0 at HS. The pre-read is a full/low-speed concern only
        // (§5.5.3: FS may be 8, 16, 32 or 64; LS is 8), which is exactly what the `else` branch
        // of `t.mps0` above already encodes.
        //
        // The evidence that made it worth taking is Boot V's M8 line, metal, n=1:
        //   :: EHCI-HID: [0] EPACE-TRIM M8 SLOW-XFER addr=0 hub=0.0 spd=HS bmreq=0x80 breq=0x06
        //      wval=0x0100 widx=0x0000 wlen=8 stg=3 xfer=50ms act=50ms ass=0ms seq=1/8 == witness ::
        // — the `05ac:8510`'s ~50 ms of NAK sat in THIS request, wholly in `act` (the device's
        // own answer latency), with `ass=0` acquitting our per-stage ASE toggle. The enum46
        // verdict (§5, BUY-2) held the trim until M8 named the request; M8 named it.
        //
        // Falsifier, and why the assumption is self-policing rather than silent: a HS device
        // with MPS0 != 64 violates §5.5.3, but we do not have to take the spec's word for it —
        // the 18-byte device descriptor read below carries `bMaxPacketSize0` at offset 7 anyway.
        // The cross-check after that read compares it against the 64 assumed here, prints a
        // witness line naming the offending device, and corrects `t.mps0` before any further
        // transfer. Cost: one byte compare on a buffer we already read.
        //
        // Also moved, deliberately: this request doubled as the liveness probe ("never answered
        // GET_DESCRIPTOR(8)"). For HS targets the first failure point is now SET_ADDRESS, whose
        // own failure path below is equally loud (`address N BURNED`).
        //
        // PREDICTION for the next metal boot (falsifiable, and the M8 instrument stays armed to
        // decide it either way):
        //   * the `05ac:8510`'s enum window shrinks by what M8 measured — ~50 ms — if the NAK
        //     belonged to the REQUEST. `EPACE: [0] enum=` ~285 → ~235 ms, `{xfer=}` on [0] loses
        //     one transfer (`n=28` → `27`) and ~50 ms, `act=` ~57 → ~7 ms.
        //   * `BPACE: ehci-hid-done d=` drops from ~1450 toward **~1400 ms**.
        //   * the M8 SLOW-XFER line naming `wlen=8` **DISAPPEARS** — the request is no longer
        //     sent to HS targets, so it cannot be slow.
        //   * **the falsifier that decides bought-vs-moved:** if a ~50 ms M8 line REAPPEARS on
        //     [0] naming `breq=0x06 wlen=18` (or `breq=0x05 wlen=0`), the NAK belonged to the
        //     device's first-request SLOT rather than to GET_DESCRIPTOR(8) specifically — the
        //     50 ms was moved, not bought, `enum=` stays ~285, and BUY-2's saving is 0. That is
        //     a real finding either way and it is the one this edit is instrumented to make.
        //   * structural, gating regardless of any ms above: `M2 armed keyboard addr=6 ep=IN3`
        //     still present and identical on [1]; `M1 hub-downstream device addr=2 05ac:8510`
        //     still present on [0]. A trim that loses a device reads faster for the worst reason.
        //   * zero `BUY-2 FALSIFIED` lines, and zero `BUY-2 suspect` short-descriptor lines. One
        //     of either means a HS device on this bench does not honour §5.5.3 and the skip must
        //     be reverted for it.
        let hs_skip_preread = eps == QH_EPS_HIGH;
        if !hs_skip_preread {
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
            // BUY-2's second falsifier arm, and the honest limit of the first. A HS device whose
            // real MPS0 were below 64 would end this IN on a short packet at its true MPS0 —
            // i.e. it lands HERE, not on the `d[7] != 64` cross-check below, which never gets to
            // run. `n` is then the device's actual MPS0, so this line names the number too.
            serial_println!(
                ":: EHCI-HID: [{}] address {} BURNED (short device descriptor: {} bytes){} ::",
                self.idx, addr, n,
                if hs_skip_preread {
                    " — BUY-2 suspect: on a HS target this is the shape a real bMaxPacketSize0 < 64 makes (USB 2.0 §5.5.3 forbids it); the byte count IS the device's MPS0, and the skipped 8-byte pre-read would have learned it"
                } else {
                    ""
                }
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
        // BUY-2's self-policing half. `d[7]` is bMaxPacketSize0 — the same field the skipped
        // 8-byte pre-read would have carried, arriving here for free. If a high-speed device
        // ever reports anything but 64 it has violated USB 2.0 §5.5.3, and the assumption above
        // would otherwise have been wrong in silence for every transfer after this one. Name it
        // and correct the stored MPS0 now: `bring_up_hub`/`configure_hid` below are the next
        // users of `t.mps0`, so the correction lands before any further wire traffic. Only the
        // legal set is accepted, exactly as the pre-read's own filter did.
        if hs_skip_preread && d[7] != 64 {
            serial_println!(
                ":: EHCI-HID: [{}] BUY-2 FALSIFIED addr={} {:04x}:{:04x} spd=HS reports bMaxPacketSize0={} — USB 2.0 §5.5.3 permits only 64 at high speed; the skipped 8-byte pre-read would have caught this, MPS0 corrected for subsequent transfers == witness ::",
                self.idx, addr, vid, pid, d[7]
            );
            let reported = d[7] as u16;
            if [8u16, 16, 32, 64].contains(&reported) {
                t.mps0 = reported;
            }
        }
        // The M1 witness. At depth 0 this line IS the topology fork decision (design §2.4).
        if depth == 0 {
            serial_println!(
                ":: EHCI-HID: [{}] M1 root device addr={} {:04x}:{:04x} class={:#04x} speed={} -> TOPOLOGY {} == witness ::",
                self.idx, addr, vid, pid, class, speed,
                if class == 0x09 { "A (hub tier / RMH)" } else { "B (direct device)" }
            );
        } else {
            // BT-L0 instrument fix (recon §3 note 1), knob-gated so the default line is
            // byte-identical. The trailing `(hub {} port {})` prints `hub_addr`/`hub_port`,
            // which are the split-transaction TT fields and are DELIBERATELY ZERO for a
            // high-speed child — so `addr=4 0424:2512 ... (hub 0 port 0)` has always read as
            // "on hub 0" when that device is on hub 1. Under `bt` the line names the actual
            // PARENT (tracked in `bt_parent`, stamped by `bring_up_hub` immediately before the
            // recursion) and labels the TT separately, which is also what makes the recon's P3
            // (TT must be the SMSC hub 4, never the FS Broadcom hub 5) readable straight off
            // this line for every device, not just the Bluetooth one.
            #[cfg(not(feature = "bt"))]
            serial_println!(
                ":: EHCI-HID: [{}] M1 hub-downstream device addr={} {:04x}:{:04x} class={:#04x} speed={} (hub {} port {}) == witness ::",
                self.idx, addr, vid, pid, class, speed, hub_addr, hub_port
            );
            #[cfg(feature = "bt")]
            serial_println!(
                ":: EHCI-HID: [{}] M1 hub-downstream device addr={} {:04x}:{:04x} class={:#04x} speed={} depth={} (parent hub {} port {}) tt=(hub {} port {}) == witness ::",
                self.idx, addr, vid, pid, class, speed, depth,
                self.bt_parent.0, self.bt_parent.1, hub_addr, hub_port
            );
        }

        if class == 0x09 {
            // Metal (probe-14e): the internal keyboard/trackpad sit behind an SMSC 0424:2512
            // hub which itself hangs off the RMH — depth 2 is the real internal topology.
            //
            // BT-L0: the Bluetooth radio sits one tier BELOW that. It is a two-device unit —
            // the Broadcom hub `0a5c:4500` (addr 5, depth 2, FULL SPEED) with the HCI
            // controller as its downstream child at depth 3 — so reaching the radio means
            // bringing up a hub AT depth 2, i.e. a cap of 3. The cap and the TT-inheritance
            // fix in `bring_up_hub` below are ONE change: lifting the cap alone would program
            // the splits against a hub that has no TT and the radio would read as dead. Both
            // are knob-gated together for this arc so a no-BT boot is provably unchanged; the
            // TT fix is a real bug fix that wants to be ungated in a follow-up once metal has
            // proven it (it can only ever matter below a non-high-speed hub, which is exactly
            // the tier the cap has been hiding).
            #[cfg(not(feature = "bt"))]
            const HUB_DEPTH_CAP: u8 = 2;
            #[cfg(feature = "bt")]
            const HUB_DEPTH_CAP: u8 = 3;
            if depth >= HUB_DEPTH_CAP {
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
        // Two USB 2.0 minima back to back, and GR19 re-derived both rather than trim them:
        // `pwr2good_ms` is the hub's own bPwrOn2PwrGood (§11.23.2.1, in 2 ms units — all three
        // hubs on this machine declare 100 ms), and the `+ 100` is T_ATTDB (§7.1.7.3), the
        // connect debounce that for an ALREADY-attached downstream device starts at power-good
        // and must complete before the port reset below. `hubpwr=200ms(n=1)` / `400ms(n=2)` is
        // 600 ms of this class on the s73 baseline and every millisecond of it is spec floor.
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
            // (~600 ms) and its loud exit are unchanged. The cost is T_DRST + GRAIN·poll_steps
            // against the old flat 50: cheaper for any port that clears well inside 50 ms, and
            // dearer only past it — which is exactly where the `rst_ms >= 50` threshold below
            // starts printing, so the band where this trim stops paying can never be silent.
            // (M5 shipped with GRAIN = 10; M6, immediately below, took it to 2 after the capture
            // showed every port reporting exactly one 10 ms step. The threshold moved from a step
            // count to a millisecond figure in the same change, so it no longer tracks the grain.)
            settle_ms(T_DRST_MS);
            // Bounded reset-completion poll (explicit loop: each probe is itself a control
            // transfer, so the generic wait_bounded closure can't drive it). ~600 ms worst case.
            //
            // EPACE-TRIM M6 (GR19) — the poll's own GRANULARITY was the last constant in this
            // class, and M5's capture convicted it on the first boot that carried M5. All six
            // downstream ports, on all three post-M5 boots of rmbp-gr16-s73/ttyUSB0.log
            // (L15641, L15668, L15671, L15674, L15677, L15680 and the same six in the two
            // preceding boots), read `PORT_RESET cleared after ~20 ms (T_DRST floor 10 ms +
            // 1 poll step(s))`. One step — never zero, never two — on every port of every boot
            // is the M5 signature one level down: the true clear point lies inside (10, 20] ms
            // and the 10 ms grain is rounding it UP to 20. `hubrst=20ms(n=1)` / `100ms(n=5)`
            // (L15743-15744) is that rounding, six times over.
            //
            // A finer grain is nearly free here because each probe is a hub-addressed
            // GET_PORT_STATUS costing ~0.15 ms on this silicon: the six no-connection probes
            // for ports 3-7 between L15672 (t=1794ms) and L15674 (t=1815ms) fit inside the 1 ms
            // that timeline leaves over the 20 ms reset. So 2 ms resolves the same interval to
            // five buckets for at most ~0.6 ms of extra transfers per port. The wall-clock
            // budget is deliberately unchanged — 10 + 300 x 2 = 610 ms against M5's
            // 10 + 60 x 10 = 610 ms — and so is the loud exit.
            const T_DRST_MS: u64 = 10; // USB 2.0 §11.5.1.5: minimum hub-driven reset duration
            const GRAIN_MS: u64 = 2; // M6 poll resolution (was 10)
            const POLL_STEPS: u32 = 300; // x GRAIN_MS = the same ~600 ms budget M5 had
            let mut status = 0u32;
            let mut ok = false;
            let mut poll_steps = 0u32;
            for step in 0..POLL_STEPS {
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
                settle_ms(GRAIN_MS);
                poll_steps = step + 1;
            }
            // The observed reset duration, in ms — meaningful ONLY on the `ok` branch (see below).
            let rst_ms = T_DRST_MS + poll_steps as u64 * GRAIN_MS;
            // T_RSTRCY (USB 2.0 §7.1.7.5) is NOT paid here: the `settle_ms(10)` at the bottom of
            // this loop body, immediately before `enumerate_at_zero`, already is it and predates
            // M5 (only hub-addressed ClearPortFeature traffic sits between the two points). An
            // extra one here would have been a second recovery interval, not a restored one.
            //
            // The M5/M6 report. Four cases, and the timeout case must not be read as a
            // measurement: `poll_steps` counts sleeps, so on a budget exhaustion it says
            // POLL_STEPS whether the bit never cleared or GET_PORT_STATUS itself was failing.
            // Only the `ok` branches are allowed to talk about reset timing, and the thresholds
            // are stated in MILLISECONDS so they survive the next change of grain.
            if !ok {
                serial_println!(
                    ":: EHCI-HID: [{}] EPACE-TRIM M5 TRIPWIRE — hub {} port {} PORT_RESET did not clear inside the ~600 ms poll budget (status {:#010x}); this is a timeout, NOT a reset-time measurement — the poll may also have been failing to read == witness ::",
                    self.idx, hub.addr, port, status
                );
            } else if rst_ms >= 50 {
                // >=, not >: at exactly 50 ms the poll has reached the constant M5 replaced, so
                // the boundary band is loud rather than silent.
                serial_println!(
                    ":: EHCI-HID: [{}] EPACE-TRIM M5 TRIPWIRE — hub {} port {} took ~{} ms to clear PORT_RESET, at or past the 50 ms constant M5 replaced == witness ::",
                    self.idx, hub.addr, port, rst_ms
                );
            } else if rst_ms >= 20 {
                // M6's own tripwire, and the one that decides whether M6 was worth landing. The
                // s73 baseline read exactly 20 ms on every port under a 10 ms grain; if the
                // finer grain still lands at or past 20, the grain was NOT quantizing — this
                // hub really does hold reset that long and M6 bought nothing on this port. That
                // is a legitimate outcome, so the line is a named tripwire rather than a
                // failure: it can fire on healthy hardware, and its absence is the trim paying.
                serial_println!(
                    ":: EHCI-HID: [{}] EPACE-TRIM M6 TRIPWIRE — hub {} port {} took ~{} ms to clear PORT_RESET ({} x {} ms poll steps past the {} ms T_DRST floor); at or past the 20 ms the 10 ms grain reported, so M6's finer grain bought nothing here == witness ::",
                    self.idx, hub.addr, port, rst_ms, poll_steps, GRAIN_MS, T_DRST_MS
                );
            } else if poll_steps > 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] hub {} port {} PORT_RESET cleared after ~{} ms (T_DRST floor {} ms + {} x {} ms poll step(s)) ::",
                    self.idx, hub.addr, port, rst_ms, T_DRST_MS, poll_steps, GRAIN_MS
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
            #[cfg(not(feature = "bt"))]
            let (ha, hp) = if child_eps == QH_EPS_HIGH { (0, 0) } else { (hub.addr, port as u8) };
            // BT-L0 — the TT-inheritance fix (recon §3). The rule above is only true when THIS
            // hub is itself high speed. USB 2.0 §11.14 puts the transaction translator in the
            // hub whose UPSTREAM connection is high speed and whose downstream ports are
            // full/low speed. A hub that trains at full speed has no TT at all: it is just
            // another full-speed device on the bus segment its nearest high-speed ancestor's TT
            // already serves, and every device below it is served by that same TT.
            //
            // On this machine that is not hypothetical. The Broadcom Bluetooth hub `0a5c:4500`
            // (addr 5) trains at FULL SPEED behind the SMSC `0424:2512` (addr 4, HS) on port 1.
            // Programming `HubAddr=5` for its children names a hub that owns no TT, the
            // controller drives SSPLIT/CSPLIT at the wrong address, and the Bluetooth radio
            // reads as dead rather than as mis-addressed — which is why this fix must land in
            // the same change as the depth-cap lift above.
            //
            // `Target` already carries the TT it was itself reached through (`hub_addr`/
            // `hub_port`), so a non-HS hub simply passes its own TT down unchanged; the
            // recursion therefore carries the nearest high-speed ancestor's TT to any depth.
            // For the Bluetooth controller this yields `(4, 1)` — the SMSC hub and the port
            // that leads to the full-speed segment — never `(5, …)`.
            #[cfg(feature = "bt")]
            let (ha, hp) = if child_eps == QH_EPS_HIGH {
                (0, 0)
            } else if hub.eps == QH_EPS_HIGH {
                (hub.addr, port as u8) // this hub is high speed — it owns the TT
            } else {
                (hub.hub_addr, hub.hub_port) // FS/LS hub — no TT here; carry the ancestor's down
            };
            // BT-L0 instrument fix (recon §3 note 1): stamp the ACTUAL parent for the M1 witness,
            // which until now printed the TT fields under a topology label. Set immediately
            // before the recursion so it is never stale.
            #[cfg(feature = "bt")]
            {
                self.bt_parent = (hub.addr, port as u8);
            }
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
        // BT-L0 — the class-0xE0 recognition arm. Placed HERE, ahead of the "nothing to arm"
        // exit, because a Bluetooth device has no HID interface at all and would otherwise be
        // enumerated, logged and dropped exactly as the recon §2b describes. The Bluetooth USB
        // transport (Bluetooth Core, Vol 4 Part B) puts the HCI transport on an interface of
        // class 0xE0 / subclass 0x01 / protocol 0x01 — an interface the spec does not oblige to
        // be first, and whose class triple the SCO interface shares (BT-L0B: `bt_probe` selects
        // by the interrupt-IN endpoint, not by descriptor order). `bt_probe` re-walks the config
        // descriptor this function already read — and, for a Bluetooth candidate only, re-reads
        // it in full first, which is safe HERE because the HID walk above has already finished
        // and nothing below reads `cfg` again. The HID walk itself is untouched. It owns
        // SET_CONFIGURATION for the device it claims and returns true when it did, at which
        // point there is nothing further for the HID path to do.
        #[cfg(feature = "bt")]
        if self.bt_probe(t, cfg, config_value) {
            return;
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
            // MTFIX: the witness and the bootlog stamp below are now GATED on the arm having
            // happened — see `arm_interrupt_ep`'s return value.
            if !self.arm_interrupt_ep(t, ep, mps.min(64), proto == 1, proto == 2, None, intf) {
                continue;
            }
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
        // MTFIX: everything below — the bootlog milestone and both `== witness` lines — is the
        // report of an endpoint that IS armed. Boot AN printed all of it for an endpoint the
        // exhausted slot pool had just skipped.
        if !self.arm_interrupt_ep(t, ep, mps.min(64), false, false, Some(layout), intf) {
            return;
        }
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

    /// ALLKEYS P1 — push a keyboard's lock-LED bitmap to the device: SET_REPORT, bmRequestType
    /// 0x21 (host->device | class | interface recipient), bRequest 0x09, wValue 0x0200
    /// (report type Output (0x02) in the high byte, report ID 0 in the low), wIndex = interface,
    /// one data byte OUT carrying bit 0 Num / bit 1 Caps / bit 2 Scroll. Byte-for-byte the request
    /// the xHCI path sends (`xhci::set_hid_leds`) — the LED is the same HID class request whichever
    /// controller carries the keyboard, and this being a MIRROR rather than a variation is the
    /// point: an operator must not be able to tell which controller a keyboard is on.
    ///
    /// The payload goes through `self.data_buf`, this controller's EP0 data staging buffer, which
    /// the OUT data stage in `control_txn` reads from. That is safe here and only here: `service`
    /// runs in main-loop context (never in an interrupt), the buffer is idle between control
    /// transfers, and this call site sits AFTER the endpoint walk has dropped its borrow — the
    /// same position, and the same reasoning, as the `mtraw` mode restore just below.
    ///
    /// BEST-EFFORT, ALWAYS. A NAK, STALL, or EP0 timeout is logged and swallowed: plenty of
    /// keyboards have no settable Output report (and the internal rMBP keyboard's answer is exactly
    /// what this arc's metal round is meant to find out). The caller has already updated the
    /// software bitmap, so a refused LED costs the operator a dark key and nothing else — caps lock
    /// still changes the case of what they type. It must never cost a keystroke.
    ///
    /// Returns whether the device accepted it, so the caller can latch a refusing keyboard off
    /// (`IntEp::kbd_led_ok`) rather than paying an EP0 timeout on every subsequent lock press.
    unsafe fn set_hid_leds(&mut self, t: &Target, intf: u8, leds: u8) -> bool {
        self.data_buf.write(leds);
        let caps = (leds >> 1) & 1;
        match self.control(t, 0x21, 0x09, 0x0200, intf as u16, 1, false) {
            Ok(_) => {
                serial_println!(
                    ":: EHCI-HID: [{}] ALLKEYS caps={} leds={:#04x} SET_REPORT ok addr={} intf={} == witness ::",
                    self.idx, caps, leds, t.addr, intf
                );
                true
            }
            // F6: a plain NAK/STALL/timeout is a device declining the LED — latch off, keep typing.
            // But `Err("hse")` is NOT that: `control` -> `chain_txn` returns it on a Host System
            // Error, which that path's own contract says WEDGES the controller (only a full HCRESET
            // recovers it — RS alone does not). Reading a wedged controller as "device declined an
            // LED" would make this witness actively false at the moment the keyboard died, so the two
            // are named apart. Exposure is small — metal settles into `overlay_mode` at enumeration,
            // so chain mode (the only producer of "hse") is QEMU-only — and recovery is not this
            // path's job: the enumeration-time handler owns HCRESET. This surfaces it honestly rather
            // than swallowing it; the LED still latches off either way so no 2 s retry follows.
            Err("hse") => {
                serial_println!(
                    ":: EHCI-HID: [{}] ALLKEYS caps={} leds={:#04x} SET_REPORT HSE addr={} intf={} — controller wedged (needs HCRESET), NOT a device LED decline ::",
                    self.idx, caps, leds, t.addr, intf
                );
                false
            }
            Err(e) => {
                serial_println!(
                    ":: EHCI-HID: [{}] ALLKEYS caps={} leds={:#04x} SET_REPORT nak addr={} intf={} ({}) — LED latched off, case still tracked ::",
                    self.idx, caps, leds, t.addr, intf, e
                );
                false
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

    // ================================ BT-L0 ==================================================
    // "Does the radio answer?" — the Bluetooth analogue of BCMA S1, and nothing more. It reaches
    // the HCI controller behind the Broadcom hub, issues TWO ROM-level HCI commands over the
    // control endpoint, and reads the replies off the interrupt-IN event endpoint.
    //
    // TRANSPORT SCOPE, and why it is deliberately this narrow (recon §4): the Bluetooth USB
    // transport puts HCI commands on the CONTROL endpoint, HCI events on an INTERRUPT-IN
    // endpoint, and ACL data on a BULK pair. This arc uses the first two ONLY. It must: this
    // Panther Point's ASYNC schedule master-aborts its first schedule fetch in every
    // configuration tried across 13 metal probes (PROBE-14, `control_txn` above), and bulk
    // conventionally lives on the async schedule. Both transfers used here are already
    // metal-proven on this exact controller — `control()` does control transfers with TT splits,
    // and the periodic interrupt-IN path is what carries the internal keyboard. No new transfer
    // primitive, no bulk endpoint, no async-schedule use. The bulk endpoints are NAMED in the
    // witness (they exist and a later ACL arc needs them) and never touched.
    //
    // BOUNDING: this runs on metal during boot, so a radio that never answers must cost a
    // bounded delay and not a hang. Every wait here is `wait_bounded` (the driver's standard
    // TSC-backed `hw_wait_budget()`), and the event-drain loop is additionally bounded by a
    // structural cap on the number of events read per command. Worst case with a dead radio is
    // therefore `BT_EVT_MAX` budgets per command, and the path still returns.

    /// BT-L0 — the class-0xE0 recognition arm and the whole L0 sequence.
    ///
    /// Returns true iff this device was claimed as a Bluetooth HCI controller (in which case
    /// SET_CONFIGURATION has been issued by this function and the HID path must not run).
    ///
    /// `cfg` aliases `self.data_buf`, which EVERY control transfer overwrites. So the descriptor
    /// walk is done FIRST, in full, into plain locals, and `cfg` is never read again after the
    /// last transfer this function issues. That ordering is load-bearing, not stylistic.
    ///
    /// BT-L0B refines that: the incoming `cfg` is the HID path's 64-byte-capped view, which on a
    /// Bluetooth composite is a stub. Phase 0 gates on the stub, re-reads the descriptor in full
    /// into the same buffer, and rebinds `cfg` to the full view; the parameter is dead from that
    /// point on. The re-read is a control transfer and therefore itself destroys the parameter's
    /// contents — which is why it happens before anything else is collected, and why it happens
    /// only for a candidate device.
    #[cfg(feature = "bt")]
    unsafe fn bt_probe(&mut self, t: &Target, cfg: &[u8], config_value: u8) -> bool {
        // ---- phase 0: candidate gate + FULL descriptor (BT-L0B) -----------------------------
        // Two facts about the descriptor this function is handed, both load-bearing:
        //
        //   * it is `configure_hid`'s buffer, and that read is CAPPED AT 64 BYTES
        //     (`total = wTotalLength.min(64)`). That cap is a HID-path economy which predates
        //     this arc and which this arc does NOT touch — the keyboard/hub walk above it is
        //     metal-proven. But a Bluetooth composite's wTotalLength is ~180-220 B, so what
        //     arrives here is a STUB of the device: interfaces and endpoints past byte 64 do
        //     not exist as far as the old walk was concerned.
        //   * `cfg` aliases `self.data_buf`, so any control transfer destroys it.
        //
        // So: gate on the stub (no traffic, no output, non-candidates unchanged), then for a
        // candidate re-read the descriptor IN FULL. `data_buf` is `Buf256`, so up to 256 B lands
        // without changing the HID path's cap by one byte. This is safe at this call site
        // specifically: `configure_hid` finished its own descriptor walk into `found[]` BEFORE
        // calling us and never reads `cfg` again afterwards — only `nfound`/`config_value`.
        if cfg.len() < 4 || !bt_cfg_has_candidate(cfg) {
            return false;
        }
        let wtotal = (cfg[2] as u16) | ((cfg[3] as u16) << 8);
        let want = wtotal.min(BT_CFG_MAX);
        let mut over = wtotal > BT_CFG_MAX;
        let mut got_short: Option<u16> = None;
        let have = cfg.len() as u16;
        // The parameter `cfg` is DEAD past this point; the shadow is the only descriptor read.
        let cfg: &[u8] = if want > have {
            match self.control(t, 0x80, 6, 0x0200, 0, want, true) {
                Ok(n) if n >= 9 => {
                    // A short control IN (9 <= n < want) is a TRUNCATED census, not a complete one —
                    // review C4: without this the walk would read fewer bytes than the descriptor
                    // declares and the census would claim the whole device silently.
                    if (n as u16) < want {
                        over = true;
                        got_short = Some(n as u16);
                    }
                    core::slice::from_raw_parts(self.data_buf, (n as usize).min(BT_CFG_MAX as usize))
                }
                _ => {
                    serial_println!(
                        ":: bt-l0: [{}] addr {} full config re-read (wTotalLength={}) FAILED — only the 64-byte HID-path view exists; claimed and stopped ::",
                        self.idx, t.addr, wtotal
                    );
                    return true;
                }
            }
        } else {
            cfg
        };

        // ---- phase 1: census + EVIDENCE-BASED selection (no wire traffic) -------------------
        // Bluetooth Core, Vol 4 Part B names the HCI transport interface class 0xE0 (Wireless
        // Controller) / subclass 0x01 (RF) / protocol 0x01 (Bluetooth), and puts HCI COMMANDS on
        // the control endpoint and HCI EVENTS on an interrupt-IN endpoint. What the spec does
        // NOT promise is that this interface comes first in the configuration descriptor, nor
        // that it is the only one wearing that class triple: the SCO audio interface is
        // isochronous and, on real parts, carries the same triple. A first-match-by-class latch
        // therefore lands on an interface that has no event endpoint BY DESIGN — which is
        // exactly what Boot AM printed (`intf 1 ... NO interrupt-IN event endpoint`), and the
        // old `bt_intf.is_none()` guard then made that first mistake permanent.
        //
        // So the rule here is EVIDENCE, not order: among alt-0 interfaces, take one that
        // actually carries an interrupt-IN endpoint. Two tiers, tried in order:
        //   tier 1 — class 0xE0/0x01/0x01 with an interrupt-IN endpoint. The spec device.
        //   tier 2 — class 0xFF/0x01/0x01 (vendor-classed, subclass/protocol still RF/Bluetooth)
        //            carrying the full HCI transport endpoint fingerprint: interrupt-IN (events)
        //            + bulk-IN + bulk-OUT (ACL). This is not a spec claim and is not labelled as
        //            one; it is a structural match, and it exists because the Broadcom parts
        //            behind Apple's hub report a vendor device class (0xff was read off addr 8
        //            on Boot AM) rather than 0xE0. Requiring all three endpoints AND the
        //            RF/Bluetooth subclass+protocol pair is what keeps it from claiming an
        //            unrelated vendor interface.
        // Anything else: print the census and stop, exactly as today.
        let mut alts = [0u8; 16];
        let mut off = 0usize;
        while off + 2 <= cfg.len() {
            let len = cfg[off] as usize;
            if len == 0 {
                break;
            }
            if cfg[off + 1] == 0x04 && off + 9 <= cfg.len() {
                let i = (cfg[off + 2] as usize) & 0xF;
                alts[i] = alts[i].saturating_add(1);
            }
            off += len;
        }

        let mut tbl = [BtIntf::default(); BT_CENSUS_MAX];
        let mut n_intf = 0usize;
        let mut dropped = 0usize;
        let mut cur: Option<usize> = None;
        let mut off = 0usize;
        while off + 2 <= cfg.len() {
            let len = cfg[off] as usize;
            if len == 0 {
                break;
            }
            match cfg[off + 1] {
                0x04 if off + 9 <= cfg.len() => {
                    if n_intf < BT_CENSUS_MAX {
                        tbl[n_intf] = BtIntf {
                            num: cfg[off + 2],
                            alt: cfg[off + 3],
                            neps: cfg[off + 4],
                            cls: cfg[off + 5],
                            sub: cfg[off + 6],
                            pro: cfg[off + 7],
                            ..Default::default()
                        };
                        cur = Some(n_intf);
                        n_intf += 1;
                    } else {
                        // Past the table: stop attributing endpoints, do not mis-file them.
                        cur = None;
                        dropped += 1;
                    }
                }
                0x05 if off + 7 <= cfg.len() => {
                    if let Some(k) = cur {
                        let f = &mut tbl[k];
                        let ep = cfg[off + 2];
                        let attr = cfg[off + 3] & 0x3;
                        let mps = ((cfg[off + 4] as u16) | ((cfg[off + 5] as u16) << 8)) & 0x7FF;
                        if (f.nep as usize) < BT_EP_MAX {
                            f.eps[f.nep as usize] = (ep, attr, mps);
                            f.nep += 1;
                        }
                        match (attr, ep & 0x80 != 0) {
                            // interrupt IN — the HCI event endpoint, the one this arc reads.
                            (3, true) if f.int_in == 0 => {
                                f.int_in = ep & 0xF;
                                f.int_mps = mps;
                                f.int_iv = cfg[off + 6];
                            }
                            // bulk — the ACL data pair. Recorded so the witness can state that
                            // they exist (a later arc needs them), and, at tier 2, as part of
                            // the transport fingerprint; NOT armed, NOT configured, and not
                            // reachable at all without the async schedule this silicon cannot
                            // run.
                            (2, true) => f.bulk_in = ep & 0xF,
                            (2, false) => f.bulk_out = ep & 0xF,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            off += len;
        }

        // The CENSUS: one line per interface descriptor, printed before any decision is taken,
        // so the next capture convicts descriptor-layout questions from the wire instead of
        // from a hypothesis. Bounded by `BT_CENSUS_MAX` x `BT_EP_MAX`, and reached only by a
        // candidate device.
        for k in 0..n_intf {
            let f = tbl[k];
            serial_print!(
                ":: bt-l0: [{}] census addr={} intf={} alt={} alts={} class={:#04x}/{:#04x}/{:#04x} neps={} eps=[",
                self.idx, t.addr, f.num, f.alt, alts[(f.num as usize) & 0xF], f.cls, f.sub, f.pro,
                f.neps
            );
            for j in 0..(f.nep as usize) {
                let (ep, attr, mps) = f.eps[j];
                serial_print!(
                    "{}{}/{}/{}{}",
                    if ep & 0x80 != 0 { "IN" } else { "OUT" },
                    ep & 0xF,
                    match attr {
                        0 => "ctl",
                        1 => "iso",
                        2 => "blk",
                        _ => "int",
                    },
                    mps,
                    if j + 1 < f.nep as usize { " " } else { "" }
                );
            }
            serial_println!("] == witness ::");
        }
        if dropped > 0 || over {
            serial_println!(
                ":: bt-l0: [{}] census addr={} INCOMPLETE — {} interface descriptor(s) past the {}-entry table{}{} ::",
                self.idx, t.addr, dropped, BT_CENSUS_MAX,
                if wtotal > BT_CFG_MAX {
                    "; wTotalLength exceeds the 256-byte control buffer, tail unread"
                } else {
                    ""
                },
                if got_short.is_some() { "; short control read, tail unread" } else { "" }
            );
            if let Some(g) = got_short {
                serial_println!(
                    ":: bt-l0: [{}] census addr={} short read got={} want={} ::",
                    self.idx, t.addr, g, want
                );
            }
        }

        let (mut sel, mut tier) = (BtIntf::default(), 0u8);
        let mut saw_e0 = false;
        for k in 0..n_intf {
            let f = tbl[k];
            if f.alt != 0 {
                continue;
            }
            let spec = f.cls == 0xE0 && f.sub == 0x01 && f.pro == 0x01;
            saw_e0 |= spec;
            let t_k = if spec && f.int_in != 0 {
                1u8
            } else if f.cls == 0xFF
                && f.sub == 0x01
                && f.pro == 0x01
                && f.int_in != 0
                && f.bulk_in != 0
                && f.bulk_out != 0
            {
                2u8
            } else {
                continue;
            };
            if tier == 0 || t_k < tier {
                tier = t_k;
                sel = f;
            }
        }
        if tier == 0 {
            // No interface carries an HCI event endpoint under either rule. Preserve the old
            // outcome exactly: a device that DOES wear the spec triple is claimed (it is not a
            // HID device and must not fall through to the HID path); one that only tripped the
            // vendor half of the gate is handed back, unclaimed, as it was before this arc.
            serial_println!(
                ":: bt-l0: [{}] addr {} — NO interface carries an interrupt-IN HCI event endpoint (spec 0xE0/0x01/0x01, or vendor 0xFF/0x01/0x01 with int-IN + bulk pair); census above is the wire truth; {} ::",
                self.idx, t.addr,
                if saw_e0 { "claimed and stopped" } else { "not claimed" }
            );
            return saw_e0;
        }
        let intf = sel.num;
        let (evt_ep, evt_mps, evt_interval) = (sel.int_in, sel.int_mps, sel.int_iv);
        let (bulk_in, bulk_out) = (sel.bulk_in, sel.bulk_out);
        serial_println!(
            ":: bt-l0: [{}] claim addr={} intf={} alt=0 class={:#04x}/{:#04x}/{:#04x} evt_ep=IN{} -> selected by ENDPOINT EVIDENCE, tier {} ({}) == witness ::",
            self.idx, t.addr, intf, sel.cls, sel.sub, sel.pro, evt_ep, tier,
            if tier == 1 {
                "spec: Bluetooth Core Vol 4 Part B class triple + interrupt-IN"
            } else {
                "vendor-classed: RF/Bluetooth subclass+protocol + int-IN/bulk-IN/bulk-OUT HCI fingerprint"
            }
        );

        // ---- phase 2: reachability witness (still no wire traffic) --------------------------
        // The recon's P3 lives on this line. `tt=(hub 4 port 1)` is the SMSC hub — the nearest
        // HIGH-SPEED ancestor, which is the only hub on this path that owns a transaction
        // translator. `tt=(hub 5 …)` would mean the split-transaction bug is back: the Broadcom
        // hub trains at FULL SPEED and has no TT, so splits aimed at it cannot complete and the
        // radio would read as dead rather than as mis-addressed.
        let spd = match t.eps {
            QH_EPS_HIGH => "HS",
            QH_EPS_LOW => "LS",
            _ => "FS",
        };
        serial_println!(
            ":: bt-l0: [{}] reachability addr={} spd={} intf={} class={:#04x}/{:#04x}/{:#04x} evt_ep=IN{} mps={} interval={} bulk_in=IN{} bulk_out=OUT{} parent=(hub {} port {}) tt=(hub {} port {}) -> {} == witness ::",
            self.idx, t.addr, spd, intf, sel.cls, sel.sub, sel.pro, evt_ep, evt_mps, evt_interval, bulk_in, bulk_out,
            self.bt_parent.0, self.bt_parent.1, t.hub_addr, t.hub_port,
            if t.eps == QH_EPS_HIGH {
                "TT-NONE(high-speed device)"
            } else if t.hub_addr == self.bt_parent.0 {
                "TT-IS-PARENT(parent hub is high speed)"
            } else {
                "TT-INHERITED(parent hub is not high speed; TT is the nearest HS ancestor)"
            }
        );

        // ---- phase 3: configure, arm the event endpoint, talk ------------------------------
        // From here on `cfg` is DEAD — this transfer overwrites the buffer it aliases.
        if self.control(t, 0x00, 9, config_value as u16, 0, 0, false).is_err() {
            serial_println!(
                ":: bt-l0: [{}] addr {} SET_CONFIGURATION({}) failed — no HCI possible ::",
                self.idx, t.addr, config_value
            );
            return true;
        }
        let Some(e) = self.bt_arm_events(t, evt_ep, evt_mps) else { return true };
        let mut toggle = false; // DTC=1 on the QH: software owns the toggle; first IN is DATA0.
        // THE ONE `armed` FLAG for this radio, threaded through every L0/L1/L2 command. It says
        // whether an interrupt-IN transfer is outstanding on the event endpoint; nothing may
        // `bt_arm_read` while it is true. It is false here because nothing has been armed yet.
        let mut armed = false;

        // HCI_Reset — OGF 0x03 / OCF 0x0003 => opcode 0x0C03, zero parameters. ROM-level: it
        // answers before any patchram blob is loaded, which is what makes P7 free to test.
        // BT-L2 STAGE GUARD (review note 2, inherited from L1): L1 ran unconditionally — even
        // where L0 had timed out — on a toggle whose relationship to the device was then unknown.
        // Its writes were idempotent so the blast radius was nil, but L2 arms a REPEATED event
        // stream on the controller that also carries the internal keyboard and trackpad. So each
        // stage now records whether it CONFIRMED, and the scan does not start unless they all did.
        let mut reset_ok = false;
        let mut ver_ok = false;
        let mut rp = [0u8; 16];
        match self.bt_hci_command(t, intf, &e, &mut toggle, BT_HCI_RESET, &[], &mut rp, &mut armed) {
            Some(n) if n >= 1 => {
                reset_ok = rp[0] == 0;
                serial_println!(
                    ":: bt-l0: [{}] HCI_Reset (0x0C03) -> CmdComplete status={:#04x} -> {} == witness ::",
                    self.idx, rp[0],
                    if rp[0] == 0 { "OK" } else { "NONZERO-STATUS" }
                );
            }
            Some(_) => serial_println!(
                ":: bt-l0: [{}] HCI_Reset (0x0C03) -> CmdComplete with NO status byte -> MALFORMED ::",
                self.idx
            ),
            None => {
                // L0 STOP (finding 3). HCI_Reset drawing no reply is not a row to note and walk
                // past: the very first command on this endpoint did not complete, so either a
                // transfer is still outstanding (`armed`) or the toggle's relationship to the
                // device is unknown — and every command after it would be issued into that. The
                // stage guard below would already have blocked the L2 scan; this stops the L0/L1
                // traffic too. `bt_quiesce_events` writes the qTD token to 0, which is what
                // DISARMS the outstanding transfer before this function returns.
                serial_println!(
                    ":: bt-l0: [{}] HCI_Reset (0x0C03) -> NO-RESPONSE (bounded wait expired) — L0 STOP: no further HCI command is issued on this radio (armed={}), and the event endpoint is quiesced ::",
                    self.idx, armed
                );
                self.bt_quiesce_events(&e);
                return true;
            }
        }

        // HCI_Read_Local_Version_Information — OGF 0x04 / OCF 0x0001 => opcode 0x1001, zero
        // parameters. Return parameters, in order: status(1) HCI_Version(1) HCI_Revision(2)
        // LMP_Version(1) Manufacturer_Name(2) LMP_Subversion(2) = 9 bytes.
        //
        // `Manufacturer_Name` is the deliverable. It is a Bluetooth SIG company identifier;
        // Broadcom is 0x000F. That field cannot be produced by our own code, by a timing
        // artefact, or by a hopeful default — it can only come off the radio.
        let mut rp2 = [0u8; 16];
        match self.bt_hci_command(
            t, intf, &e, &mut toggle, BT_HCI_READ_LOCAL_VERSION, &[], &mut rp2, &mut armed,
        ) {
            Some(n) if n >= 9 => {
                ver_ok = rp2[0] == 0;
                let manufacturer = (rp2[5] as u16) | ((rp2[6] as u16) << 8);
                let hci_rev = (rp2[2] as u16) | ((rp2[3] as u16) << 8);
                let lmp_subver = (rp2[7] as u16) | ((rp2[8] as u16) << 8);
                if manufacturer == BT_MFG_BROADCOM {
                    serial_println!(
                        ":: bt-l0: HCI local version — hci_ver={:#04x} hci_rev={:#06x} lmp_ver={:#04x} manufacturer={:#06x} lmp_subver={:#06x} -> BROADCOM ::",
                        rp2[1], hci_rev, rp2[4], manufacturer, lmp_subver
                    );
                } else {
                    serial_println!(
                        ":: bt-l0: HCI local version — hci_ver={:#04x} hci_rev={:#06x} lmp_ver={:#04x} manufacturer={:#06x} lmp_subver={:#06x} -> UNEXPECTED-MFG({:#06x}) ::",
                        rp2[1], hci_rev, rp2[4], manufacturer, lmp_subver, manufacturer
                    );
                }
                // P6 is a MEASUREMENT, not a prediction (recon §5) — HCI_Version resolves which
                // Broadcom part this is, and the recon explicitly declines to guess it. Printed
                // separately from the verdict line so a reader cannot mistake the mapping for
                // evidence: the mapping is spec (Bluetooth Core, Assigned Numbers), the number
                // is wire.
                serial_println!(
                    ":: bt-l0: [{}] HCI_Version {:#04x} => core spec {} (status={:#04x}) == witness ::",
                    self.idx, rp2[1],
                    // Bluetooth SIG Assigned Numbers, HCI Version — the pre-arc table was shifted
                    // one slot (0x06 printed "3.0+HS"); review-corrected against the SIG list.
                    match rp2[1] {
                        0x02 => "1.2",
                        0x03 => "2.0+EDR",
                        0x04 => "2.1+EDR",
                        0x05 => "3.0+HS",
                        0x06 => "4.0",
                        0x07 => "4.1",
                        0x08 => "4.2",
                        0x09 => "5.0",
                        0x0A => "5.1",
                        _ => "unmapped",
                    },
                    rp2[0]
                );
            }
            Some(n) => serial_println!(
                ":: bt-l0: HCI local version — SHORT-REPLY ({} return byte(s), 9 required) -> MALFORMED ::",
                n
            ),
            None => serial_println!(
                ":: bt-l0: HCI local version — NO-RESPONSE (bounded wait expired) ::"
            ),
        }

        // ---- BT-L1: the first real command/event round-trips beyond the version read ---------
        // A small command TABLE, issued in order through the now-armed event endpoint on the same
        // running toggle. Read-only identity/params first, then the one WRITE (Set_Event_Mask).
        // Each row: issue, bounded-wait its CommandComplete, decode status + payload, witness with
        // the `bt-l1:` prefix. A None (bounded wait expired) is a CLEAN STOP naming the command —
        // the sequence breaks rather than hanging or improvising. L2 (LE scan) extends this table.
        //
        // Every command here is a MANDATORY HCI command (present before any Broadcom patchram
        // `.hcd`), so an "unknown command" status would be a genuine finding, not an expected
        // firmware gate — it is witnessed as UNKNOWN-CMD and reported, never patched around.
        let l1: [(u16, &str, &[u8]); 5] = [
            (BT_HCI_READ_BD_ADDR, "HCI_Read_BD_ADDR", &[]),
            (BT_HCI_READ_BUFFER_SIZE, "HCI_Read_Buffer_Size", &[]),
            (BT_HCI_READ_LOCAL_FEATURES, "HCI_Read_Local_Supported_Features", &[]),
            (BT_HCI_READ_LOCAL_COMMANDS, "HCI_Read_Local_Supported_Commands", &[]),
            (BT_HCI_SET_EVENT_MASK, "HCI_Set_Event_Mask", &BT_EVENT_MASK),
        ];
        // BT-L2 stage guard, continued: `l1_ok` falls to false on ANY row that did not come back
        // with a well-formed status=0x00 reply; `le_supported` is read from the LMP feature mask
        // rather than inferred from the 4.0 version number, because a scan on a controller whose
        // own feature mask denies LE is a command sequence with no defensible expectation.
        let mut l1_ok = true;
        let mut le_supported = false;
        for &(opcode, name, params) in l1.iter() {
            // 68 bytes holds the largest L1 return payload — Read_Local_Supported_Commands'
            // status(1) + Supported_Commands(64) = 65 — with slack; every other command is far
            // smaller.
            let mut rp = [0u8; 68];
            let Some(n) = self.bt_hci_command(
                t, intf, &e, &mut toggle, opcode, params, &mut rp, &mut armed,
            ) else {
                // Bounded wait expired: name the command and STOP the L1 sequence. Not a hang,
                // not forced — the event path or a firmware gate is the suspect (see predictions).
                serial_println!(
                    ":: bt-l1: [{}] {} ({:#06x}) -> NO-RESPONSE (bounded wait expired) — L1 STOP ::",
                    self.idx, name, opcode
                );
                l1_ok = false;
                break;
            };
            if n < 1 {
                serial_println!(
                    ":: bt-l1: [{}] {} ({:#06x}) -> CmdComplete with NO status byte -> MALFORMED ::",
                    self.idx, name, opcode
                );
                l1_ok = false;
                continue;
            }
            let status = rp[0];
            if status != 0 {
                l1_ok = false;
            }
            // 0x01 = Unknown HCI Command. Called out explicitly because for a MANDATORY command it
            // is the clean-room / patchram boundary signal (docs/MANIFESTO/CLEAN_ROOM_POLICY.md),
            // not an ordinary error.
            if status == 0x01 {
                serial_println!(
                    ":: bt-l1: [{}] {} ({:#06x}) -> status=0x01 UNKNOWN-CMD — a mandatory command was refused; possible patchram gate (STOP, do not add firmware) ::",
                    self.idx, name, opcode
                );
            }
            match opcode {
                // Read_BD_ADDR: status(1) + BD_ADDR(6, little-endian, LSB first on the wire).
                // Rendered MSB-first (the human notation), so the OUI is the leading three octets.
                BT_HCI_READ_BD_ADDR if n >= 7 => serial_println!(
                    ":: bt-l1: [{}] HCI_Read_BD_ADDR (0x1009) status={:#04x} bd_addr={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} oui={:02x}:{:02x}:{:02x} == witness ::",
                    self.idx, status, rp[6], rp[5], rp[4], rp[3], rp[2], rp[1], rp[6], rp[5], rp[4]
                ),
                // Read_Buffer_Size: status(1) acl_len(2,LE) sco_len(1) acl_num(2,LE) sco_num(2,LE).
                BT_HCI_READ_BUFFER_SIZE if n >= 8 => {
                    let acl_len = (rp[1] as u16) | ((rp[2] as u16) << 8);
                    let sco_len = rp[3];
                    let acl_num = (rp[4] as u16) | ((rp[5] as u16) << 8);
                    let sco_num = (rp[6] as u16) | ((rp[7] as u16) << 8);
                    serial_println!(
                        ":: bt-l1: [{}] HCI_Read_Buffer_Size (0x1005) status={:#04x} acl_len={} acl_num={} sco_len={} sco_num={} == witness ::",
                        self.idx, status, acl_len, acl_num, sco_len, sco_num
                    );
                }
                // Read_Local_Supported_Features: status(1) + LMP_Features(8). LE-supported is
                // byte 4 bit 6 (mask 0x40, BlueZ LMP_LE); BR/EDR Not Supported is byte 4 bit 5
                // (0x20). The full 8-byte mask is printed verbatim so the capture can be
                // re-decoded if a bit position is ever questioned.
                BT_HCI_READ_LOCAL_FEATURES if n >= 9 => {
                    let f = &rp[1..9];
                    let le = f[4] & 0x40 != 0;
                    let no_bredr = f[4] & 0x20 != 0;
                    le_supported = le; // BT-L2 stage guard reads this, not the version number.
                    serial_println!(
                        ":: bt-l1: [{}] HCI_Read_Local_Supported_Features (0x1003) status={:#04x} lmp_features=[{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}] LE(controller)={} BR/EDR-not-supported={} == witness ::",
                        self.idx, status, f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7], le, no_bredr
                    );
                }
                // Read_Local_Supported_Commands: status(1) + Supported_Commands(64). The full
                // 64-octet bitfield is the wire truth L2 decodes for LE-scan support; printed
                // whole, bounded. `n` may be 65 (all octets) when the reassembly fit.
                // Review C1: the guard is `n >= 65` (status + the full 64-octet bitfield), NOT `n >= 1`.
                // This is the ONLY command in the table that needs multi-packet reassembly, so a partial
                // reassembly is exactly the failure this arc's central mechanism exists to prevent — and
                // with a `>= 1` guard it would have printed a normal `== witness ::` line with a short
                // `n=` instead of falling to MALFORMED. A witness that cannot fail on the one case it was
                // built to prove is not a witness.
                BT_HCI_READ_LOCAL_COMMANDS if n >= 65 => {
                    serial_print!(
                        ":: bt-l1: [{}] HCI_Read_Local_Supported_Commands (0x1002) status={:#04x} n={} cmds=[",
                        self.idx, status, n.saturating_sub(1)
                    );
                    for j in 1..n {
                        serial_print!("{:02x}{}", rp[j], if j + 1 < n { " " } else { "" });
                    }
                    serial_println!("] == witness ::");
                }
                // Set_Event_Mask: status(1) only. The first WRITE — witnessed with its status; a
                // 0x00 proves the write path end to end, and the mask is the reset default so the
                // command is idempotent and leaves no persistent state changed.
                BT_HCI_SET_EVENT_MASK if n >= 1 => serial_println!(
                    ":: bt-l1: [{}] HCI_Set_Event_Mask (0x0C01) status={:#04x} -> {} (mask=reset-default 0x00001FFFFFFFFFFF, idempotent) == witness ::",
                    self.idx, status,
                    if status == 0 { "OK" } else { "NONZERO-STATUS" }
                ),
                _ => {
                    // A reply too short for its own decoder is a MALFORMED row, and the L2 stage
                    // guard must see it as one — including the `n < 65` reassembly failure that
                    // review C1 routed here on purpose.
                    l1_ok = false;
                    serial_println!(
                        ":: bt-l1: [{}] {} ({:#06x}) status={:#04x} -> SHORT-REPLY ({} return byte(s)) -> MALFORMED ::",
                        self.idx, name, opcode, status, n
                    );
                }
            }
        }

        // ---- BT-L2: LE scan — the first thing this radio does that a person can see -----------
        // THE GUARD (review note 2). A scan is not another idempotent write: it turns on a
        // REPEATED event stream on the controller that also carries the internal keyboard and the
        // trackpad. So it starts only from a fully confirmed base — every preceding stage came
        // back well-formed with status 0x00 — and only on a controller whose own LMP feature mask
        // claims LE. Anything else prints why and leaves the radio exactly as L1 left it.
        if !(reset_ok && ver_ok && l1_ok) {
            serial_println!(
                ":: bt-l2: [{}] LE scan NOT STARTED — a preceding stage did not confirm (reset_ok={} version_ok={} l1_ok={}); the radio is left as L1 left it ::",
                self.idx, reset_ok, ver_ok, l1_ok
            );
        } else if !le_supported {
            serial_println!(
                ":: bt-l2: [{}] LE scan NOT STARTED — LMP feature mask reports LE(controller)=false; no LE command is defensible on this part ::",
                self.idx
            );
        } else {
            self.bt_le_scan(t, intf, &e, &mut toggle, &mut armed);
        }

        // Quiesce: the event endpoint stays LINKED in the frame list (its slot is owned for the
        // boot and any later `arm_interrupt_ep` chains correctly behind it — the same state a
        // retired `dead` endpoint leaves), but its transfer is deactivated so the controller
        // stops issuing INs against a device nothing is reading.
        self.bt_quiesce_events(&e);
        true
    }

    /// BT-L2 — issue one LE bring-up command and witness its status.
    ///
    /// Returns the CommandComplete status byte, or None when no well-formed reply arrived (already
    /// witnessed). A status of 0x01 (Unknown HCI Command) is called out separately: on an LE
    /// command it is the patchram/`.hcd` FIRMWARE BOUNDARY (`docs/MANIFESTO/CLEAN_ROOM_POLICY.md`)
    /// — this arc witnesses it and stops, and adds no firmware path.
    #[cfg(feature = "bt")]
    #[allow(clippy::too_many_arguments)]
    unsafe fn bt_l2_cmd(
        &mut self,
        t: &Target,
        intf: u8,
        e: &BtEvtEp,
        toggle: &mut bool,
        armed: &mut bool,
        opcode: u16,
        name: &str,
        params: &[u8],
    ) -> Option<u8> {
        let mut rp = [0u8; 16];
        let Some(n) =
            self.bt_hci_command_ex(t, intf, e, toggle, opcode, params, &mut rp, armed)
        else {
            // `bt_hci_command_ex` returns None for an EP0 SEND FAILURE as well as for a send that
            // drew no reply, and does not distinguish them in its return. Say so rather than assert
            // the wait expired: the EP0 failure witnesses itself on its own line if it occurred.
            serial_println!(
                ":: bt-l2: [{}] {} ({:#06x}) -> NO-RESPONSE — either the bounded wait expired with no CommandComplete, or the EP0 control-OUT failed (which prints its own line above) ::",
                self.idx, name, opcode
            );
            return None;
        };
        if n < 1 {
            serial_println!(
                ":: bt-l2: [{}] {} ({:#06x}) -> CmdComplete with NO status byte -> MALFORMED ::",
                self.idx, name, opcode
            );
            return None;
        }
        let st = rp[0];
        if st == 0x01 {
            serial_println!(
                ":: bt-l2: [{}] {} ({:#06x}) -> status=0x01 UNKNOWN-CMD — this controller refuses an LE command; that is the patchram/.hcd firmware boundary (docs/MANIFESTO/CLEAN_ROOM_POLICY.md). STOP — no firmware path is added here ::",
                self.idx, name, opcode
            );
        }
        serial_println!(
            ":: bt-l2: [{}] {} ({:#06x}) status={:#04x} -> {} == witness ::",
            self.idx, name, opcode, st,
            if st == 0 { "OK" } else { "FAIL" }
        );
        Some(st)
    }

    /// BT-L2 — LE SCAN: open the LE event channel, scan passively for a bounded window, report the
    /// devices heard, and turn the radio back off.
    ///
    /// The order is forced by the spec and by what L1's review found. `HCI_Set_Event_Mask` comes
    /// FIRST because L1 wrote the reset default, and the reset default does not include LE Meta
    /// Event (bit 61) — every advertising report rides that one bit, so without this write the
    /// scan below would run correctly, hear everything, and report a silent, entirely wrong empty
    /// room. `HCI_LE_Set_Event_Mask` then selects the Advertising Report sub-event within that
    /// channel. Only then are scan parameters set and scanning enabled.
    ///
    /// **Scanning is disabled on every exit path that could have enabled it** — including the
    /// unconfirmed one, where the enable's CommandComplete never arrived and the controller must
    /// therefore be assumed to be scanning. A radio left scanning burns power and floods the event
    /// endpoint for the rest of the boot, on the same EHCI controller as the internal keyboard and
    /// trackpad. The paths that return BEFORE the enable command never enabled anything and have
    /// nothing to undo.
    #[cfg(feature = "bt")]
    unsafe fn bt_le_scan(
        &mut self,
        t: &Target,
        intf: u8,
        e: &BtEvtEp,
        toggle: &mut bool,
        armed: &mut bool,
    ) {
        // TWO GUARDS ARE LOAD-BEARING HERE, and both are outside this function:
        //
        // 1. THE L2 STAGE GUARD in `bt_probe` — this is reached only when `reset_ok && ver_ok &&
        //    l1_ok && le_supported`. Every one of those required a well-formed status=0x00 reply,
        //    which means every preceding command RETIRED its read: that is the only reason `armed`
        //    can be relied on to describe the endpoint truthfully on entry. (It is now threaded in
        //    from `bt_probe` rather than assumed false, so even a path that changes is correct.)
        // 2. `bt_quiesce_events` in `bt_probe`, AFTER this returns — it writes the qTD token to 0,
        //    which is what disarms whatever transfer is still outstanding when this function ends.
        //    Nothing in here needs to un-arm on the way out; nothing in here may leak an armed
        //    transfer to a LATER subsystem either, because that quiesce is unconditional.

        // ---- 1. HCI_Set_Event_Mask — open the LE Meta Event channel (bit 61) -----------------
        match self.bt_l2_cmd(
            t, intf, e, toggle, armed,
            BT_HCI_SET_EVENT_MASK, "HCI_Set_Event_Mask(+LE-Meta)", &BT_EVENT_MASK_LE,
        ) {
            Some(0) => serial_println!(
                ":: bt-l2: [{}] event mask=0x20001FFFFFFFFFFF — LE Meta Event (bit 61) ENABLED (L1 wrote the reset default 0x00001FFFFFFFFFFF, which does NOT carry it) == witness ::",
                self.idx
            ),
            _ => {
                serial_println!(
                    ":: bt-l2: [{}] LE scan NOT STARTED — the event mask could not be widened to carry LE Meta Events; a scan behind this would report nothing and mean nothing ::",
                    self.idx
                );
                return;
            }
        }

        // ---- 2. HCI_LE_Set_Event_Mask — select the Advertising Report sub-event ---------------
        match self.bt_l2_cmd(
            t, intf, e, toggle, armed,
            BT_HCI_LE_SET_EVENT_MASK, "HCI_LE_Set_Event_Mask", &BT_LE_EVENT_MASK,
        ) {
            Some(0) => serial_println!(
                ":: bt-l2: [{}] LE event mask=0x000000000000001F — LE Advertising Report (bit 1) ENABLED == witness ::",
                self.idx
            ),
            _ => {
                serial_println!(
                    ":: bt-l2: [{}] LE scan NOT STARTED — the LE event mask was not accepted ::",
                    self.idx
                );
                return;
            }
        }

        // ---- 3. HCI_LE_Set_Scan_Parameters — passive, continuous, public address --------------
        // LE_Scan_Type(1) LE_Scan_Interval(2, LE) LE_Scan_Window(2, LE) Own_Address_Type(1)
        // Scanning_Filter_Policy(1).
        let sp: [u8; 7] = [
            BT_LE_SCAN_TYPE_PASSIVE,
            BT_LE_SCAN_INTERVAL as u8,
            (BT_LE_SCAN_INTERVAL >> 8) as u8,
            BT_LE_SCAN_WINDOW as u8,
            (BT_LE_SCAN_WINDOW >> 8) as u8,
            BT_LE_OWN_ADDR_PUBLIC,
            BT_LE_SCAN_FILTER_ALL,
        ];
        match self.bt_l2_cmd(
            t, intf, e, toggle, armed,
            BT_HCI_LE_SET_SCAN_PARAMS, "HCI_LE_Set_Scan_Parameters", &sp,
        ) {
            Some(0) => serial_println!(
                ":: bt-l2: [{}] scan parameters — type=PASSIVE(listen only, no SCAN_REQ) interval={:#06x}(={}us) window={:#06x}(={}us) => CONTINUOUS (window==interval) own_addr=PUBLIC filter_policy=ACCEPT-ALL == witness ::",
                self.idx,
                BT_LE_SCAN_INTERVAL, BT_LE_SCAN_INTERVAL as u32 * 625,
                BT_LE_SCAN_WINDOW, BT_LE_SCAN_WINDOW as u32 * 625
            ),
            _ => {
                serial_println!(
                    ":: bt-l2: [{}] LE scan NOT STARTED — scan parameters were not accepted; nothing was enabled ::",
                    self.idx
                );
                return;
            }
        }

        // ---- 4. HCI_LE_Set_Scan_Enable(enable) ------------------------------------------------
        // LE_Scan_Enable(1) Filter_Duplicates(1). Duplicate filtering ON: the controller then
        // reports each advertiser once per enable, which is what makes a bounded window's report
        // count a measure of DEVICES rather than of how chatty the room is.
        let (drain, must_disable) = match self.bt_l2_cmd(
            t, intf, e, toggle, armed,
            BT_HCI_LE_SET_SCAN_ENABLE, "HCI_LE_Set_Scan_Enable(enable)", &[0x01, 0x01],
        ) {
            Some(0) => {
                serial_println!(
                    ":: bt-l2: [{}] scan ENABLED — passive, filter_duplicates=on, bounded window={}ms == witness ::",
                    self.idx, BT_L2_SCAN_MS
                );
                (true, true)
            }
            Some(_) => {
                // An explicit nonzero status means the controller REFUSED to start: nothing is
                // scanning, so there is nothing to turn off.
                serial_println!(
                    ":: bt-l2: [{}] scan NOT enabled — the controller returned a nonzero status; no scan ran and nothing needs disabling ::",
                    self.idx
                );
                (false, false)
            }
            None => {
                // `bt_l2_cmd` returns None for BOTH an EP0 send failure and a send with no
                // CommandComplete — it cannot tell them apart, so this line must not claim the
                // packet went out (it previously did). Either way the conservative reading is the
                // same and it is the one that governs: if the packet DID reach the radio, the radio
                // may be scanning. Do NOT drain (the event path is the suspect), but DO disable —
                // an unconfirmed enable is exactly the case the "off on every exit path" rule
                // exists for, and the disable is harmless if nothing ever started.
                serial_println!(
                    ":: bt-l2: [{}] scan enable UNCONFIRMED — no CommandComplete came back, and an EP0 send failure is indistinguishable here (it prints its own line above if it happened). The controller must therefore be ASSUMED to be scanning, so the disable below runs anyway ::",
                    self.idx
                );
                (false, true)
            }
        };

        // ---- 5. drain LE Advertising Reports for the bounded window ---------------------------
        // `ep_halted` = the drain ended on `BtEvt::Stop` from a real endpoint halt, which is the
        // one state in which the disable below may not read its own reply.
        let mut ep_halted = false;
        // BT-L3 — the peer the drain picked, if any. `None` whenever the drain did not run.
        let mut peer: Option<([u8; 6], u8)> = None;
        if drain {
            let (h, p) = self.bt_le_drain(e, toggle, armed);
            ep_halted = h;
            peer = p;
        }

        // ---- 6. HCI_LE_Set_Scan_Enable(disable) — the mandatory exit --------------------------
        // RECONCILIATION with `BtEvt::Stop` ("do not issue further commands"): Stop forbids further
        // EVENT READS on the interrupt-IN endpoint, not this EP0 control-OUT — and the EP0 write is
        // the thing that actually stops the radio, so it goes out on every path that could have
        // started a scan. Only the READ is conditional:
        //   * halted endpoint  -> send only, and witness explicitly that nothing was read. No stall
        //     clear and no toggle reset is attempted: re-opening a halted endpoint is a decision
        //     with its own evidence requirements and this arc does not make it.
        //   * everything else (including a mid-event timeout Stop, and a window that simply
        //     expired) -> the transfer is still ARMED and `armed` carries it forward; the pre-armed
        //     hand-off in `bt_hci_command_ex` consumes it rather than arming a second qTD over it.
        // BT-L3 gate: `scan_off_confirmed` is true ONLY where the disable came back with an
        // explicit status 0x00. It is the entry condition for L3 — see the block after this one.
        let mut scan_off_confirmed = false;
        if must_disable {
            if ep_halted {
                let sent = self.bt_hci_send(t, intf, BT_HCI_LE_SET_SCAN_ENABLE, &[0x00, 0x00]);
                serial_println!(
                    ":: bt-l2: [{}] scan disable SENT UNREAD — the event endpoint HALTED during the drain, so NO CommandComplete was read for HCI_LE_Set_Scan_Enable(0x200C) enable=0 and none is claimed; reading a halted endpoint is exactly what BtEvt::Stop forbids. The EP0 control-OUT, which is what stops the radio, was {} ::",
                    self.idx,
                    if sent { "SENT successfully" } else { "REFUSED by EP0 (see the line above)" }
                );
            } else {
                let mut rp = [0u8; 16];
                match self.bt_hci_command_ex(
                    t, intf, e, toggle,
                    BT_HCI_LE_SET_SCAN_ENABLE, &[0x00, 0x00], &mut rp, armed,
                ) {
                    Some(n) if n >= 1 => {
                        scan_off_confirmed = rp[0] == 0;
                        serial_println!(
                            ":: bt-l2: [{}] scan DISABLED — HCI_LE_Set_Scan_Enable(0x200C) enable=0 status={:#04x} -> {} == witness ::",
                            self.idx, rp[0],
                            if rp[0] == 0 { "OK" } else { "NONZERO-STATUS" }
                        );
                    }
                    _ => serial_println!(
                        ":: bt-l2: [{}] scan disable UNCONFIRMED — no CommandComplete for HCI_LE_Set_Scan_Enable(0x200C) enable=0. The EP0 write is what stops the radio and it was attempted (an EP0 failure prints its own line above); what is missing is the confirmation, not the attempt ::",
                        self.idx
                    ),
                }
            }
        }

        // ---- 7. BT-L3 — connect to the selected peer, and always let go -----------------------
        // THE L3 GATE, in the same spirit as L2's stage guard and stricter for the same reason: a
        // create is not an idempotent write, it puts the controller into the Initiating state and
        // a create that is never resolved leaves it there for the rest of the boot, refusing later
        // LE commands with Command Disallowed. So L3 runs only when ALL of:
        //   * a peer was heard (`peer.is_some()`)   — otherwise there is nothing to connect to;
        //   * the event endpoint is not halted      — L3 must be able to READ its own events, and
        //                                             `BtEvt::Stop` forbids reads on a halted one;
        //   * the scan disable returned status 0x00 — a controller still scanning may refuse the
        //                                             create, and that refusal would be
        //                                             indistinguishable from a real one.
        // Every other combination prints which condition failed and issues no create at all —
        // which is also the only way to have nothing outstanding by construction.
        match peer {
            Some(p) if !ep_halted && scan_off_confirmed => {
                self.bt_l3_connect(t, intf, e, toggle, armed, p);
            }
            Some(_) => serial_println!(
                ":: bt-l3: [{}] connect NOT ATTEMPTED — a peer was selected but the entry conditions do not hold (event_endpoint_halted={} scan_off_confirmed={}); NO HCI_LE_Create_Connection was issued, so nothing is outstanding == witness ::",
                self.idx, ep_halted, scan_off_confirmed
            ),
            None => serial_println!(
                ":: bt-l3: [{}] connect NOT ATTEMPTED — no connectable peer was selected during the scan window; NO HCI_LE_Create_Connection was issued, so nothing is outstanding == witness ::",
                self.idx
            ),
        }
    }

    /// BT-L3 — cycles for `ms` milliseconds of wall clock, on the same terms `bt_le_drain` uses.
    ///
    /// UNCALIBRATED FALLBACK, stated honestly: with `tsc_hz() == 0` there is no cycles->time mapping
    /// at all, so no fallback is `ms` in wall-clock terms. `hw_wait_budget()` in that state returns
    /// the fixed 2.5e9-cycle guess (NOT 2 s of anything); a quarter of it is ~0.27 s on the 2.3 GHz
    /// bench part. Every L3 window then collapses to that same quarter-budget regardless of `ms`,
    /// which is a bounded guess of the right ORDER and is not claimed to be more. The witness lines
    /// print the MEASURED elapsed time, so an uncalibrated run cannot masquerade as a calibrated one.
    #[cfg(feature = "bt")]
    fn bt_l3_budget(ms: u64) -> u64 {
        let hz = crate::arch::x86_64::apic::tsc_hz();
        if hz != 0 {
            (hz / 1000).saturating_mul(ms)
        } else {
            crate::arch::hw_wait_budget() / 4
        }
    }

    /// BT-L3 — drain reassembled events until ONE matching `want` arrives, or the wall-clock budget
    /// (or the structural event cap) expires.
    ///
    /// THE ARMED INVARIANT IS PRESERVED BY CONSTRUCTION AND NOT BY CARE: this function never calls
    /// `bt_arm_read`. Every read goes through `bt_read_full_event`, which arms only under
    /// `if !*armed` and clears `*armed` only where a transfer actually retired (a completed qTD, or
    /// a `QTD_ERR_MASK` halt) — and hands it forward on both timeout paths. The same one `armed`
    /// flag `bt_probe` created is threaded in and out; L3 mints none of its own.
    ///
    /// `seen` accumulates every whole event reassembled, matching or not, so the L3 tally can say
    /// how much traffic it walked past rather than implying the wanted event was the only one.
    ///
    /// `st` IS NOT BOOKKEEPING. Everything this function walks past is discarded, and one of the
    /// events it walks past — an `LE Connection Complete` carrying a live handle — is the only
    /// thing by which a link can ever be released. `st` is where a walked-past event is latched so
    /// the caller can consult it before concluding; see `BtL3State` for the ordering that makes
    /// this the LIKELY path rather than a corner. `st.stopped` also makes the "no more reads"
    /// rule structural: once an unreadable endpoint has been seen, this function returns without
    /// touching it again, so no later wait can re-arm a qTD over a halt and clear the QH's Halted
    /// bit behind a device STALL that is still set.
    #[cfg(feature = "bt")]
    #[allow(clippy::too_many_arguments)]
    unsafe fn bt_l3_await(
        &mut self,
        e: &BtEvtEp,
        toggle: &mut bool,
        armed: &mut bool,
        want: BtL3Want,
        budget_cy: u64,
        seen: &mut u32,
        st: &mut BtL3State,
        asm: &mut [u8],
    ) -> BtL3Await {
        // A latched halt is permanent for the rest of L3. NO READ IS ATTEMPTED — not even an arm.
        if st.stopped {
            return BtL3Await::Stop;
        }
        let t0 = crate::arch::now_cycles();
        let mut here = 0u32;
        loop {
            let el = crate::arch::now_cycles().wrapping_sub(t0);
            if el >= budget_cy {
                return BtL3Await::Timeout;
            }
            if here >= BT_L3_EVT_MAX {
                // The structural cap ended the wait while events were still arriving. The window
                // was NOT read to term, so the absence of a latch below proves nothing.
                st.blind = true;
                return BtL3Await::Timeout;
            }
            let len = match self.bt_read_full_event(
                e,
                toggle,
                armed,
                budget_cy - el,
                // FINDING 2: the continuation phase is bounded by what REMAINS of this wait's own
                // window, not by a fresh `hw_wait_budget()` per packet. Without this, one wait
                // could outlast its stated budget by several seconds and the arc's worst case was
                // a claim rather than a bound.
                budget_cy - el,
                asm,
            ) {
                BtEvt::Got { len, trunc } => {
                    if trunc {
                        // A truncated event cannot be matched against `want` without guessing at
                        // the part that did not fit. Counted and stepped over, never decoded —
                        // and because it MIGHT have been the connection event, this wait can no
                        // longer be cited as having seen everything.
                        st.blind = true;
                        here += 1;
                        *seen += 1;
                        continue;
                    }
                    len
                }
                // The window expired with nothing on the wire. The transfer stays armed and is
                // handed forward through `armed` — the teardown command consumes it.
                BtEvt::Idle(_) => return BtL3Await::Timeout,
                BtEvt::Stop => {
                    st.stopped = true;
                    st.blind = true;
                    return BtL3Await::Stop;
                }
            };
            here += 1;
            *seen += 1;
            if len < 2 {
                continue; // zero-length packet: not an event
            }
            let pkt = &asm[..len];
            // Command Status: EventCode(1)=0x0F Param_Total_Length(1)=4 Status(1)
            //   Num_HCI_Command_Packets(1) Command_Opcode(2, LE)  => opcode at [4..6].
            // Command Complete: EventCode(1)=0x0E Param_Total_Length(1)
            //   Num_HCI_Command_Packets(1) Command_Opcode(2, LE)  => opcode at [3..5].
            let hit = match want {
                BtL3Want::CmdStatus(op) => {
                    pkt[0] == BT_EVT_CMD_STATUS
                        && len >= 6
                        && ((pkt[4] as u16) | ((pkt[5] as u16) << 8)) == op
                }
                BtL3Want::CmdComplete(op) => {
                    pkt[0] == BT_EVT_CMD_COMPLETE
                        && len >= 5
                        && ((pkt[3] as u16) | ((pkt[4] as u16) << 8)) == op
                }
                BtL3Want::LeMeta(sub) => pkt[0] == BT_EVT_LE_META && len >= 3 && pkt[2] == sub,
                BtL3Want::Evt(code) => pkt[0] == code,
            };
            if hit {
                return BtL3Await::Got(len);
            }
            // ---- THE LATCH (FINDING 1) --------------------------------------------------------
            // Not the event this wait wanted, so the loop is about to step over it. If it is an
            // `LE Connection Complete`, stepping over it silently is what leaks a link to a
            // stranger's device for the rest of the boot. The 21-byte length is the same one the
            // decoders above require; a shorter one cannot be trusted to carry a handle, and it
            // set `blind` — the honest reading is "something connection-shaped went past and could
            // not be read", not "nothing was there".
            if pkt[0] == BT_EVT_LE_META && len >= 3 && pkt[2] == BT_LE_SUBEVT_CONN_COMPLETE {
                if len >= 21 {
                    if pkt[3] == 0x00 {
                        // A LIVE HANDLE. Latched only if one is not already held: the first is
                        // the one this arc created, and a second would be a link this arc did not
                        // ask for and cannot release with a single handle anyway.
                        if st.live_handle.is_none() {
                            st.live_handle =
                                Some(((pkt[4] as u16) | ((pkt[5] as u16) << 8)) & 0x0FFF);
                        }
                    } else {
                        // No link, but the create RESOLVED: the controller left the Initiating
                        // state in order to send this.
                        st.resolved_nonzero = true;
                    }
                } else {
                    st.blind = true;
                }
            }
        }
    }

    /// BT-L3 — CONNECT to one LE peer, and always let go.
    ///
    /// The whole of L3 is one command with a deferred answer, plus the two commands that undo it.
    /// The structure is dictated by which of those two undo commands applies:
    ///
    /// * a create that RESOLVED into a live connection is released with `HCI_Disconnect`;
    /// * a create that DID NOT resolve is withdrawn with `HCI_LE_Create_Connection_Cancel`.
    ///
    /// and by the fact that between those two states there is a genuine race — the cancel may lose,
    /// in which case the link IS live and the right teardown is a disconnect.
    ///
    /// THE RACE HAS TWO ORDERINGS AND THE LIKELIER ONE IS NOT THE OBVIOUS ONE. The obvious one is
    /// cancel -> Command Complete 0x00 -> `LE Connection Complete` 0x00 with a handle, and it is
    /// handled inline below. The likelier one is the reverse: once the connection has established
    /// the controller is no longer Initiating, so it answers the cancel with Command Complete
    /// **0x0C (Command Disallowed)** — with the `LE Connection Complete` carrying the real handle
    /// already queued AHEAD of it. The wait for that Command Complete therefore reads the meta
    /// event first, and a wait discards everything that is not what it asked for. `BtL3State` is
    /// where that discard was turned into a latch; every teardown decision consults it before
    /// concluding, and `left_outstanding=none` is only printed when it has.
    ///
    /// MUST-NOT-APPEAR, and the tally exists to make it visible: this function ending with a live
    /// connection or an unresolved create. `left_outstanding=` on the tally line is that condition;
    /// it reads `none` on every correct path.
    #[cfg(feature = "bt")]
    #[allow(clippy::too_many_arguments)]
    unsafe fn bt_l3_connect(
        &mut self,
        t: &Target,
        intf: u8,
        e: &BtEvtEp,
        toggle: &mut bool,
        armed: &mut bool,
        peer: ([u8; 6], u8),
    ) {
        let (addr, atype) = peer;
        let mut asm = [0u8; BT_EVT_ASM_MAX];
        let mut seen = 0u32; // whole events reassembled across every L3 wait
        let mut attempted = 0u32; // HCI_LE_Create_Connection packets sent
        let mut completed = 0u32; // LE Connection Complete with status 0x00
        let mut disconnected = 0u32; // Disconnection Complete with status 0x00
        let mut cancels = 0u32; // HCI_LE_Create_Connection_Cancel packets sent
        // The two facts the arc must end with FALSE.
        let mut live = false; // a connection is established and not yet released
        let mut outstanding = false; // a create was issued and has not been seen to resolve
        let mut handle = 0u16;
        // Everything the waits learn that their caller did not ask for: a walked-past connection
        // event, a walked-past resolution, whether any wait ended blind, and whether the event
        // endpoint has become unreadable. See `BtL3State`.
        let mut st3 = BtL3State::default();
        let t0 = crate::arch::now_cycles();

        // ---- 1. HCI_LE_Create_Connection ------------------------------------------------------
        // 25 parameter bytes, in order:
        //   LE_Scan_Interval(2) LE_Scan_Window(2) Initiator_Filter_Policy(1) Peer_Address_Type(1)
        //   Peer_Address(6) Own_Address_Type(1) Conn_Interval_Min(2) Conn_Interval_Max(2)
        //   Conn_Latency(2) Supervision_Timeout(2) Minimum_CE_Length(2) Maximum_CE_Length(2)
        // Every value is justified at its constant. The two that are decided here:
        //   * LE_Scan_Interval/Window reuse L2's 0x0060/0x0060 — 60 ms, window == interval, so the
        //     initiator listens continuously. The argument is L2's, unchanged: at a lower duty the
        //     peer could advertise entirely inside the deaf half and a bounded window would report
        //     a failure it never listened for.
        //   * Initiator_Filter_Policy 0x00 = USE THE PEER ADDRESS IN THIS COMMAND (0x01 would use
        //     the white list, which is empty on a freshly reset controller and would match nothing).
        //   * Own_Address_Type 0x00 = PUBLIC: the BD_ADDR L1 read and witnessed. Unlike the passive
        //     scan, an initiator DOES transmit, so this field now decides what goes on the air —
        //     and the honest value is the address this machine actually owns.
        let cp: [u8; 25] = [
            BT_LE_SCAN_INTERVAL as u8,
            (BT_LE_SCAN_INTERVAL >> 8) as u8,
            BT_LE_SCAN_WINDOW as u8,
            (BT_LE_SCAN_WINDOW >> 8) as u8,
            0x00, // Initiator_Filter_Policy: use Peer_Address below, not the white list
            atype,
            addr[0], addr[1], addr[2], addr[3], addr[4], addr[5],
            BT_LE_OWN_ADDR_PUBLIC,
            BT_L3_CONN_INTERVAL_MIN as u8,
            (BT_L3_CONN_INTERVAL_MIN >> 8) as u8,
            BT_L3_CONN_INTERVAL_MAX as u8,
            (BT_L3_CONN_INTERVAL_MAX >> 8) as u8,
            BT_L3_CONN_LATENCY as u8,
            (BT_L3_CONN_LATENCY >> 8) as u8,
            BT_L3_SUPERVISION_TIMEOUT as u8,
            (BT_L3_SUPERVISION_TIMEOUT >> 8) as u8,
            BT_L3_CE_LENGTH_MIN as u8,
            (BT_L3_CE_LENGTH_MIN >> 8) as u8,
            BT_L3_CE_LENGTH_MAX as u8,
            (BT_L3_CE_LENGTH_MAX >> 8) as u8,
        ];
        serial_println!(
            ":: bt-l3: [{}] create parameters — scan_interval={:#06x}(={}us) scan_window={:#06x}(={}us) filter_policy=USE-PEER-ADDRESS peer={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}/{} own_addr=PUBLIC conn_interval={:#06x}..{:#06x}(={}..{}us) latency={} supervision_timeout={:#06x}(={}ms) ce_len={:#06x}..{:#06x}(controller's choice); LE Connection Complete (meta subevent 0x01) rides the LE event mask 0x1F L2 already wrote (bit 0) and the LE Meta bit 61 L2 already opened — L3 writes NO mask == witness ::",
            self.idx,
            BT_LE_SCAN_INTERVAL, BT_LE_SCAN_INTERVAL as u32 * 625,
            BT_LE_SCAN_WINDOW, BT_LE_SCAN_WINDOW as u32 * 625,
            addr[5], addr[4], addr[3], addr[2], addr[1], addr[0],
            // Only 0x00/0x01 can reach here: the selection filter refuses the identity forms
            // (0x02/0x03), which is the whole reason this gloss has no third arm.
            if atype == 0x00 { "public" } else { "random" },
            BT_L3_CONN_INTERVAL_MIN, BT_L3_CONN_INTERVAL_MAX,
            BT_L3_CONN_INTERVAL_MIN as u32 * 1250, BT_L3_CONN_INTERVAL_MAX as u32 * 1250,
            BT_L3_CONN_LATENCY,
            BT_L3_SUPERVISION_TIMEOUT, BT_L3_SUPERVISION_TIMEOUT as u32 * 10,
            BT_L3_CE_LENGTH_MIN, BT_L3_CE_LENGTH_MAX
        );

        if !self.bt_hci_send(t, intf, BT_HCI_LE_CREATE_CONN, &cp) {
            // The EP0 control-OUT itself failed and witnessed its own line. NOTHING reached the
            // radio, so the controller is not initiating and there is nothing to cancel.
            serial_println!(
                ":: bt-l3: [{}] HCI_LE_Create_Connection (0x200D) NOT SENT — the EP0 control-OUT failed (its own line is above). The command never reached the radio, so no create is outstanding and no cancel is owed == witness ::",
                self.idx
            );
            self.bt_l3_tally(t0, seen, attempted, completed, disconnected, cancels, live, outstanding);
            return;
        }
        attempted += 1;
        // FROM THIS INSTANT the controller may be initiating. `outstanding` is true until something
        // is OBSERVED to resolve it — not until we believe it did.
        outstanding = true;

        // ---- 2. Command Status for 0x200D -----------------------------------------------------
        // Create_Connection answers with Command Status, never Command Complete: the real result is
        // the LE Connection Complete meta event below.
        let mut create_accepted = false;
        // The `bt_l3_await` result is BOUND before the match rather than used as the scrutinee:
        // the arms read `asm`, and a scrutinee's temporaries (here the `&mut asm` reborrow) live
        // for the whole match expression. Same reason at every other L3 wait.
        let r = self.bt_l3_await(
            e, toggle, armed,
            BtL3Want::CmdStatus(BT_HCI_LE_CREATE_CONN),
            Self::bt_l3_budget(BT_L3_CMD_MS),
            &mut seen, &mut st3, &mut asm,
        );
        match r {
            BtL3Await::Got(_) => {
                let st = asm[2];
                if st == 0x00 {
                    create_accepted = true;
                    serial_println!(
                        ":: bt-l3: [{}] HCI_LE_Create_Connection (0x200D) -> CommandStatus status={:#04x} -> ACCEPTED, the controller is now INITIATING == witness ::",
                        self.idx, st
                    );
                } else {
                    // An explicit nonzero Command Status means the command was REJECTED: the
                    // controller did not enter the Initiating state, so there is nothing to cancel.
                    // 0x0C = Command Disallowed, 0x01 = Unknown HCI Command (the patchram boundary),
                    // 0x12 = Invalid HCI Parameters (a parameter above would be wrong, not the peer).
                    outstanding = false;
                    serial_println!(
                        ":: bt-l3: [{}] HCI_LE_Create_Connection (0x200D) -> CommandStatus status={:#04x} -> REFUSED{} — the controller did NOT enter the Initiating state, so no cancel is owed == witness ::",
                        self.idx, st,
                        match st {
                            0x01 => " (UNKNOWN-CMD: this controller's ROM does not carry the command; that is the patchram/.hcd firmware boundary, docs/MANIFESTO/CLEAN_ROOM_POLICY.md — no firmware path is added here)",
                            0x0C => " (COMMAND-DISALLOWED: the controller is in a state that forbids it)",
                            0x12 => " (INVALID-HCI-PARAMETERS: one of the parameters witnessed above is out of range for this part)",
                            _ => "",
                        }
                    );
                }
            }
            BtL3Await::Timeout => serial_println!(
                ":: bt-l3: [{}] HCI_LE_Create_Connection (0x200D) -> NO CommandStatus within {}ms — the command went out on EP0 and the controller MAY be initiating, so the create is treated as OUTSTANDING and the cancel below runs == witness ::",
                self.idx, BT_L3_CMD_MS
            ),
            BtL3Await::Stop => serial_println!(
                ":: bt-l3: [{}] HCI_LE_Create_Connection (0x200D) -> event endpoint became UNREADABLE before any CommandStatus. The create is treated as OUTSTANDING; the cancel below is SENT on EP0 (which the halt did not touch) and its reply is not read == witness ::",
                self.idx
            ),
        }

        // ---- 3. LE Connection Complete (meta subevent 0x01) -----------------------------------
        // Layout: 0x3E, Param_Total_Length, Subevent(0x01), Status(1), Connection_Handle(2, LE),
        // Role(1), Peer_Address_Type(1), Peer_Address(6), Conn_Interval(2), Conn_Latency(2),
        // Supervision_Timeout(2), Master_Clock_Accuracy(1) — 21 bytes on the wire.
        if create_accepted {
            let r = self.bt_l3_await(
                e, toggle, armed,
                BtL3Want::LeMeta(BT_LE_SUBEVT_CONN_COMPLETE),
                Self::bt_l3_budget(BT_L3_CONN_MS),
                &mut seen, &mut st3, &mut asm,
            );
            match r {
                BtL3Await::Got(len) if len >= 21 => {
                    let st = asm[3];
                    let h = ((asm[4] as u16) | ((asm[5] as u16) << 8)) & 0x0FFF;
                    let iv = (asm[14] as u16) | ((asm[15] as u16) << 8);
                    let lat = (asm[16] as u16) | ((asm[17] as u16) << 8);
                    let sto = (asm[18] as u16) | ((asm[19] as u16) << 8);
                    // Whatever the status, the create has RESOLVED: the controller left the
                    // Initiating state to send this event. Nothing to cancel either way.
                    outstanding = false;
                    if st == 0x00 {
                        completed += 1;
                        live = true;
                        handle = h;
                        serial_println!(
                            ":: bt-l3: [{}] LE Connection Complete — status={:#04x} handle={:#06x} role={} peer={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}/{} interval={:#06x}(={}us) latency={} supervision_timeout={:#06x}(={}ms) mca={:#04x} -> CONNECTED == witness ::",
                            self.idx, st, h,
                            if asm[6] == 0x00 { "MASTER(initiator)" } else { "SLAVE" },
                            asm[13], asm[12], asm[11], asm[10], asm[9], asm[8],
                            if asm[7] == 0x00 { "public" } else if asm[7] == 0x01 { "random" } else { "reserved" },
                            iv, iv as u32 * 1250, lat, sto, sto as u32 * 10, asm[20]
                        );
                    } else {
                        serial_println!(
                            ":: bt-l3: [{}] LE Connection Complete — status={:#04x} -> NOT CONNECTED{}. The create RESOLVED (the controller left the Initiating state to send this), so no cancel is owed == witness ::",
                            self.idx, st,
                            match st {
                                // 0x02 here is NOT "the create was cancelled": no cancel has been
                                // issued at this point in the arc, and the only cancel this arc
                                // ever sends is section 4a below. Reaching it here means the
                                // CONTROLLER dropped the create for a reason of its own.
                                0x02 => " (UNKNOWN-CONNECTION-IDENTIFIER: the controller dropped the create without a cancel from us — this arc has issued none yet)",
                                0x3E => " (CONNECTION-FAILED-TO-BE-ESTABLISHED: the peer answered the CONNECT_IND and then went silent)",
                                _ => "",
                            }
                        );
                    }
                }
                BtL3Await::Got(len) => serial_println!(
                    ":: bt-l3: [{}] LE Connection Complete SHORT-EVENT ({} bytes, 21 required) -> MALFORMED — the create is treated as OUTSTANDING (this event cannot be trusted to say it resolved) and the cancel below runs == witness ::",
                    self.idx, len
                ),
                BtL3Await::Timeout => serial_println!(
                    ":: bt-l3: [{}] NO LE Connection Complete within {}ms — the controller is still INITIATING. The ordinary reading is that the peer stopped advertising between the scan and the create. The create is OUTSTANDING and MUST be cancelled == witness ::",
                    self.idx, BT_L3_CONN_MS
                ),
                BtL3Await::Stop => serial_println!(
                    ":: bt-l3: [{}] event endpoint became UNREADABLE while awaiting LE Connection Complete — the create is OUTSTANDING and the cancel is SENT UNREAD below == witness ::",
                    self.idx
                ),
            }
        }

        // ---- 3b. RECONCILE THE LATCH, BEFORE ANY CANCEL IS CONSIDERED --------------------------
        // Sections 2 and 3 each walk past every event that is not the one they asked for. If one
        // of those was an `LE Connection Complete` with status 0x00, a LINK EXISTS and the right
        // teardown is a disconnect, not a cancel — issuing a cancel against an established link is
        // both useless and the shape that produced the leak this fix exists for. Consulted here so
        // that `outstanding` is resolved before section 4a can act on a stale reading of it.
        if !live {
            if let Some(h) = self.bt_l3_claim_latched(&mut st3, "the create/connect waits") {
                completed += 1;
                live = true;
                handle = h;
                outstanding = false;
            } else if outstanding && st3.resolved_nonzero {
                outstanding = false;
                serial_println!(
                    ":: bt-l3: [{}] create RESOLVED OUT OF BAND — an LE Connection Complete with a NONZERO status was walked past by an earlier wait. No link exists, but the controller left the Initiating state to send it, so no cancel is owed == witness ::",
                    self.idx
                );
            }
        }

        // ---- 4a. MANDATORY TEARDOWN: withdraw an unresolved create -----------------------------
        // An outstanding create is not a loose end, it is a STUCK CONTROLLER: the Initiating state
        // persists and later LE commands are refused with Command Disallowed for the rest of the
        // boot. The cancel goes out on every path that reached here with `outstanding` true —
        // including the ones where the endpoint can no longer be read, because the cancel rides EP0
        // and `BtEvt::Stop` forbids reads, not writes (the same reconciliation L2 made for the scan
        // disable).
        if outstanding {
            if !self.bt_hci_send(t, intf, BT_HCI_LE_CREATE_CONN_CANCEL, &[]) {
                serial_println!(
                    ":: bt-l3: [{}] HCI_LE_Create_Connection_Cancel (0x200E) NOT SENT — the EP0 control-OUT failed (its own line is above). THE CREATE REMAINS OUTSTANDING and this controller will refuse later LE commands == witness ::",
                    self.idx
                );
            } else {
                cancels += 1;
                // The cancel answers with a Command Complete (status only), and BOTH of its
                // statuses are ambiguous until the latch is consulted:
                //
                //   0x00 — the create was withdrawn and an `LE Connection Complete` reporting the
                //          cancellation (status 0x02) should follow. It may instead carry status
                //          0x00 and a real handle: the cancel lost the race by a hair.
                //   0x0C — Command Disallowed. This does NOT mean "there was no create". It means
                //          the controller is not Initiating RIGHT NOW, and the commonest reason
                //          for that is THE CONNECTION ALREADY ESTABLISHED — in which case the
                //          `LE Connection Complete` carrying the handle was queued AHEAD of this
                //          Command Complete and the wait below has already walked past it. That is
                //          the likelier of the two orderings, and reading 0x0C as "nothing to
                //          cancel" is what leaked a live link while the tally said `none`.
                //
                // Both branches therefore consult `st3` before concluding anything.
                //
                // FINDING 3: whether a read was even ATTEMPTED is a separate fact from whether one
                // succeeded, and the SENT UNREAD witnesses used to conflate them. A halt latched by
                // an earlier section makes `bt_l3_await` return `Stop` without touching the
                // endpoint — which is the correct behaviour (re-arming over a halt clears the QH's
                // Halted bit behind a device STALL that is still set) but is NOT the same event as
                // a read that was tried and found the endpoint dead.
                let read_attempted = !st3.stopped;
                let r = self.bt_l3_await(
                    e, toggle, armed,
                    BtL3Want::CmdComplete(BT_HCI_LE_CREATE_CONN_CANCEL),
                    Self::bt_l3_budget(BT_L3_CMD_MS),
                    &mut seen, &mut st3, &mut asm,
                );
                match r {
                    BtL3Await::Got(len) if len >= 6 => {
                        let st = asm[5];
                        serial_println!(
                            ":: bt-l3: [{}] HCI_LE_Create_Connection_Cancel (0x200E) -> CmdComplete status={:#04x} -> {} == witness ::",
                            self.idx, st,
                            match st {
                                0x00 => "ACCEPTED (an LE Connection Complete reporting the cancellation should follow)",
                                0x0C => "COMMAND-DISALLOWED (the controller is not Initiating — either it never was, or the connection has ALREADY ESTABLISHED; the latch below decides which)",
                                _ => "UNEXPECTED-STATUS",
                            }
                        );
                        if st == 0x0C {
                            // FINDING 1. The Command Complete arrived, so any `LE Connection
                            // Complete` the controller queued ahead of it has ALREADY been walked
                            // past by the wait above and sits in the latch. Consult it before
                            // deciding what 0x0C meant.
                            if let Some(h) = self.bt_l3_claim_latched(&mut st3, "the cancel's own wait") {
                                completed += 1;
                                live = true;
                                handle = h;
                                outstanding = false;
                            } else if st3.resolved_nonzero {
                                outstanding = false;
                                serial_println!(
                                    ":: bt-l3: [{}] COMMAND-DISALLOWED explained — an LE Connection Complete with a NONZERO status was walked past, so the create had already resolved without a link. Nothing is outstanding and nothing is live == witness ::",
                                    self.idx
                                );
                            } else {
                                outstanding = false;
                                serial_println!(
                                    ":: bt-l3: [{}] COMMAND-DISALLOWED read as NEVER-INITIATING — no LE Connection Complete of any status was walked past by any wait of this run, and an established connection would have queued one AHEAD of this Command Complete. {} == witness ::",
                                    self.idx,
                                    if st3.blind {
                                        "CAVEAT, and it is the whole of the evidence: at least one wait ended BLIND (a truncated event stepped over, an unreadable endpoint, or the event cap reached), so this run cannot prove it saw everything. If a link was established it is NOT released by this arc"
                                    } else {
                                        "Every wait of this run read its window to term, so the absence of that event is evidence and not merely silence"
                                    }
                                );
                            }
                        } else if st == 0x00 {
                            // THE RACE. A cancel can lose: the CONNECT_IND may already have been
                            // answered, in which case this meta event carries status 0x00 and a
                            // real handle — the link IS live and must be disconnected, not left.
                            let r2 = self.bt_l3_await(
                                e, toggle, armed,
                                BtL3Want::LeMeta(BT_LE_SUBEVT_CONN_COMPLETE),
                                Self::bt_l3_budget(BT_L3_CMD_MS),
                                &mut seen, &mut st3, &mut asm,
                            );
                            match r2 {
                                BtL3Await::Got(len) if len >= 21 => {
                                    let cst = asm[3];
                                    outstanding = false;
                                    if cst == 0x00 {
                                        completed += 1;
                                        live = true;
                                        handle = ((asm[4] as u16) | ((asm[5] as u16) << 8)) & 0x0FFF;
                                        serial_println!(
                                            ":: bt-l3: [{}] CANCEL LOST THE RACE — LE Connection Complete status=0x00 handle={:#06x} arrived in reply to the cancel: the link was already established. It is LIVE and is disconnected below == witness ::",
                                            self.idx, handle
                                        );
                                    } else {
                                        serial_println!(
                                            ":: bt-l3: [{}] create WITHDRAWN — LE Connection Complete status={:#04x}{} after the cancel; the controller has left the Initiating state and nothing is outstanding == witness ::",
                                            self.idx, cst,
                                            if cst == 0x02 { " (UNKNOWN-CONNECTION-IDENTIFIER, the spec's cancellation status)" } else { "" }
                                        );
                                    }
                                }
                                BtL3Await::Got(len) => serial_println!(
                                    ":: bt-l3: [{}] post-cancel LE Connection Complete SHORT-EVENT ({} bytes, 21 required) -> MALFORMED; the cancel returned status 0x00 so the create is believed withdrawn, but this arc did not READ the confirmation and says so — treated as STILL OUTSTANDING == witness ::",
                                    self.idx, len
                                ),
                                BtL3Await::Timeout => serial_println!(
                                    ":: bt-l3: [{}] cancel ACCEPTED but NO LE Connection Complete followed within {}ms. The withdrawal is unconfirmed — treated as STILL OUTSTANDING rather than assumed clean == witness ::",
                                    self.idx, BT_L3_CMD_MS
                                ),
                                BtL3Await::Stop => serial_println!(
                                    ":: bt-l3: [{}] cancel ACCEPTED but the event endpoint became UNREADABLE before its LE Connection Complete — treated as STILL OUTSTANDING == witness ::",
                                    self.idx
                                ),
                            }
                        }
                    }
                    BtL3Await::Got(len) => serial_println!(
                        ":: bt-l3: [{}] HCI_LE_Create_Connection_Cancel (0x200E) -> CmdComplete SHORT-EVENT ({} bytes, 6 required) -> MALFORMED; the create is treated as STILL OUTSTANDING == witness ::",
                        self.idx, len
                    ),
                    BtL3Await::Timeout => serial_println!(
                        ":: bt-l3: [{}] HCI_LE_Create_Connection_Cancel (0x200E) SENT but NO CmdComplete within {}ms. The EP0 write is what withdraws the create and it was attempted; what is missing is the confirmation, not the attempt — treated as STILL OUTSTANDING == witness ::",
                        self.idx, BT_L3_CMD_MS
                    ),
                    BtL3Await::Stop => serial_println!(
                        ":: bt-l3: [{}] HCI_LE_Create_Connection_Cancel (0x200E) SENT UNREAD — {}, so no CmdComplete is claimed. The EP0 write, which is what withdraws the create, went out; treated as STILL OUTSTANDING == witness ::",
                        self.idx,
                        if read_attempted {
                            "a read was ATTEMPTED and the event endpoint proved unreadable"
                        } else {
                            "NO READ WAS ATTEMPTED: an earlier section already found the event endpoint unreadable and that fact is latched, so this arc does not re-arm a transfer over a halt (which would clear the QH's Halted bit while the device's STALL stands)"
                        }
                    ),
                }
            }
        }

        // ---- 4a-bis. RECONCILE THE LATCH ONE LAST TIME -----------------------------------------
        // The cancel's own waits walk past events too, and its short-event / timeout / unreadable
        // branches all leave without consulting the latch. This is the last point at which a
        // walked-past handle can still be turned into a disconnect, so it is checked here rather
        // than trusted to the branches above. Cheap, and the alternative is a leaked link.
        if !live {
            if let Some(h) = self.bt_l3_claim_latched(&mut st3, "the teardown's waits") {
                completed += 1;
                live = true;
                handle = h;
                outstanding = false;
            }
        }

        // ---- 4b. MANDATORY TEARDOWN: release a live connection ---------------------------------
        if live {
            if self.bt_l3_disconnect(
                t, intf, e, toggle, armed, handle, &mut seen, &mut st3, &mut asm,
            ) {
                disconnected += 1;
                live = false;
            }
        }

        self.bt_l3_tally(t0, seen, attempted, completed, disconnected, cancels, live, outstanding);
    }

    /// BT-L3 — take a latched live handle, if one is held, and witness the recovery.
    ///
    /// Consumes the latch (`take`), so the several places that consult it cannot double-count the
    /// same connection. `at` names WHICH group of waits walked the event past, because that is the
    /// diagnostic content: it says which ordering the controller actually produced.
    #[cfg(feature = "bt")]
    fn bt_l3_claim_latched(&self, st: &mut BtL3State, at: &str) -> Option<u16> {
        let h = st.live_handle.take()?;
        serial_println!(
            ":: bt-l3: [{}] LATCHED LINK RECOVERED — an LE Connection Complete with status=0x00 handle={:#06x} was walked past by {} because it was not the event that wait asked for. THIS IS THE CANCEL RACE IN ITS LIKELIER ORDERING: the connection established, so the controller was no longer Initiating and answered the cancel with Command Disallowed, having already queued this event ahead of it. Discarding it would have left a LIVE LINK to the peer for the rest of the boot (the event qTD is deactivated straight after L3, so no Disconnection Complete could ever be read). The handle is adopted: the create RESOLVED, nothing is outstanding, and the link is DISCONNECTED below == witness ::",
            self.idx, h, at
        );
        Some(h)
    }

    /// BT-L3 — release one live connection. Returns whether a `Disconnection Complete` with status
    /// 0x00 was OBSERVED for this handle; the caller keeps `live` true on anything else, so an
    /// unconfirmed teardown shows up on the tally as the must-not-appear condition it is.
    ///
    /// `HCI_Disconnect` answers with a Command Status; the link is not down until the
    /// `Disconnection Complete` event (0x05) arrives. Both are bounded.
    #[cfg(feature = "bt")]
    #[allow(clippy::too_many_arguments)]
    unsafe fn bt_l3_disconnect(
        &mut self,
        t: &Target,
        intf: u8,
        e: &BtEvtEp,
        toggle: &mut bool,
        armed: &mut bool,
        handle: u16,
        seen: &mut u32,
        st: &mut BtL3State,
        asm: &mut [u8],
    ) -> bool {
        // Connection_Handle(2, LE) Reason(1).
        let dp: [u8; 3] = [handle as u8, (handle >> 8) as u8, BT_HCI_REASON_REMOTE_USER_TERM];
        if !self.bt_hci_send(t, intf, BT_HCI_DISCONNECT, &dp) {
            serial_println!(
                ":: bt-l3: [{}] HCI_Disconnect (0x0406) handle={:#06x} NOT SENT — the EP0 control-OUT failed (its own line is above). THE CONNECTION IS STILL LIVE == witness ::",
                self.idx, handle
            );
            return false;
        }
        serial_println!(
            ":: bt-l3: [{}] HCI_Disconnect (0x0406) SENT — handle={:#06x} reason={:#04x} (REMOTE-USER-TERMINATED) == witness ::",
            self.idx, handle, BT_HCI_REASON_REMOTE_USER_TERM
        );
        let r = self.bt_l3_await(
            e, toggle, armed,
            BtL3Want::CmdStatus(BT_HCI_DISCONNECT),
            Self::bt_l3_budget(BT_L3_CMD_MS),
            seen, st, asm,
        );
        match r {
            BtL3Await::Got(_) if asm[2] == 0x00 => serial_println!(
                ":: bt-l3: [{}] HCI_Disconnect -> CommandStatus status=0x00 -> ACCEPTED, awaiting Disconnection Complete == witness ::",
                self.idx
            ),
            BtL3Await::Got(_) => {
                serial_println!(
                    ":: bt-l3: [{}] HCI_Disconnect -> CommandStatus status={:#04x} -> REFUSED. THE CONNECTION IS STILL LIVE and this arc has no second lever for it == witness ::",
                    self.idx, asm[2]
                );
                return false;
            }
            BtL3Await::Timeout => serial_println!(
                ":: bt-l3: [{}] HCI_Disconnect -> NO CommandStatus within {}ms; the EP0 write went out, so the teardown may still be in flight — the Disconnection Complete wait below is the decider == witness ::",
                self.idx, BT_L3_CMD_MS
            ),
            BtL3Await::Stop => {
                serial_println!(
                    ":: bt-l3: [{}] HCI_Disconnect SENT UNREAD — the event endpoint is not readable, so neither its CommandStatus nor its Disconnection Complete can be observed. The EP0 write, which is what tears the link down, went out; this arc CANNOT CONFIRM it == witness ::",
                    self.idx
                );
                return false;
            }
        }
        // Disconnection Complete: 0x05, Param_Total_Length(1)=4, Status(1), Connection_Handle(2),
        // Reason(1) — 6 bytes on the wire.
        let r = self.bt_l3_await(
            e, toggle, armed,
            BtL3Want::Evt(BT_EVT_DISCONN_COMPLETE),
            Self::bt_l3_budget(BT_L3_DISC_MS),
            seen, st, asm,
        );
        match r {
            BtL3Await::Got(len) if len >= 6 => {
                let st = asm[2];
                let h = ((asm[3] as u16) | ((asm[4] as u16) << 8)) & 0x0FFF;
                let ok = st == 0x00 && h == handle;
                serial_println!(
                    ":: bt-l3: [{}] Disconnection Complete — status={:#04x} handle={:#06x} reason={:#04x} -> {} == witness ::",
                    self.idx, st, h, asm[5],
                    if ok {
                        "DISCONNECTED (the link is released)"
                    } else if st == 0x00 {
                        "HANDLE MISMATCH — a different connection was released; ours is STILL LIVE"
                    } else {
                        "NONZERO-STATUS — the link is NOT released"
                    }
                );
                ok
            }
            BtL3Await::Got(len) => {
                serial_println!(
                    ":: bt-l3: [{}] Disconnection Complete SHORT-EVENT ({} bytes, 6 required) -> MALFORMED; the release is NOT confirmed == witness ::",
                    self.idx, len
                );
                false
            }
            BtL3Await::Timeout => {
                serial_println!(
                    ":: bt-l3: [{}] NO Disconnection Complete within {}ms — the disconnect was accepted but the release is NOT confirmed. The link will in any case drop on its own supervision timeout ({}ms) == witness ::",
                    self.idx, BT_L3_DISC_MS, BT_L3_SUPERVISION_TIMEOUT as u32 * 10
                );
                false
            }
            BtL3Await::Stop => {
                serial_println!(
                    ":: bt-l3: [{}] event endpoint became UNREADABLE before Disconnection Complete — the release is NOT confirmed == witness ::",
                    self.idx
                );
                false
            }
        }
    }

    /// BT-L3 — the end-of-stage tally. ONE line, and the only line a reader needs to decide whether
    /// the arc let go: `left_outstanding=` reads `none` on every correct path, and names what is
    /// left on every incorrect one. The audited zeros are printed even when zero, because a counter
    /// that only appears when nonzero cannot be read as evidence that nothing happened.
    #[cfg(feature = "bt")]
    #[allow(clippy::too_many_arguments)]
    fn bt_l3_tally(
        &self,
        t0: u64,
        events: u32,
        attempted: u32,
        completed: u32,
        disconnected: u32,
        cancels: u32,
        live: bool,
        outstanding: bool,
    ) {
        // FINDING 5: `unwrap_or(0)` printed `elapsed=0ms` on exactly the run this doc-comment said
        // could not masquerade — the UNCALIBRATED one, where `epace_ms` returns None. A fabricated
        // zero in milliseconds is indistinguishable from an L3 that did nothing. `epace_fmt` is the
        // rest of this file's answer: raw cycles with a `cy` unit when the TSC rate is unknown.
        let (elapsed, unit) = epace_fmt(crate::arch::now_cycles().wrapping_sub(t0));
        serial_println!(
            ":: bt-l3: [{}] L3 tally — elapsed={}{} events_read={} connections_attempted={} connections_completed={} disconnections_confirmed={} cancels_issued={} left_outstanding={} == witness ::",
            self.idx, elapsed, unit, events, attempted, completed, disconnected, cancels,
            // FINDING 4: the fourth arm of this match, `(true, true)`, had NO PRODUCER. Every site
            // that sets `live` also resolves `outstanding` in the same breath — a connection event
            // is precisely what takes the controller out of the Initiating state — and `outstanding`
            // is never set again after the create. Rather than leave a dead arm asserting a
            // condition the code cannot reach, `live` is matched first and swallows both: if a link
            // is held, that is the headline whatever `outstanding` says.
            match (live, outstanding) {
                (false, false) => "none",
                (true, _) => "A LIVE CONNECTION — the teardown was not confirmed. THIS IS THE MUST-NOT-APPEAR CONDITION",
                (false, true) => "AN UNRESOLVED HCI_LE_Create_Connection — the controller may still be INITIATING and will refuse later LE commands. THIS IS THE MUST-NOT-APPEAR CONDITION",
            }
        );
    }

    /// BT-L2 — read LE Advertising Reports off the event endpoint for a BOUNDED wall-clock window
    /// and build the distinct-device table.
    ///
    /// The window is why `bt_read_full_event` takes a budget: `hw_wait_budget()` is two seconds
    /// per silent read, so a drain built on the L0/L1 read primitive would cost seconds in a quiet
    /// room. Here each first-packet read is bounded by what REMAINS of the window, so an empty
    /// room costs exactly `BT_L2_SCAN_MS` and no more — and the one transfer left armed when the
    /// window expires is handed forward (`armed`) to the disable command rather than abandoned.
    ///
    /// WHAT L2 COSTS, stated as a bound and not as the happy path: the DRAIN is capped at
    /// `BT_L2_SCAN_MS`, but the commands around it are not. Each of the five bring-up commands and
    /// the mandatory disable reads its CommandComplete on the full `hw_wait_budget()` (~1.1 s at
    /// the bench part's 2.3 GHz, up to ~2.5 s under TCG) for its FIRST packet, so a radio that
    /// stops answering can add up to roughly one budget per outstanding command — the disable alone
    /// is ~2.5 s worst case. The scan window is bounded; the L2 STAGE is bounded by those budgets,
    /// on the order of seconds, not by 500 ms and not by any "≤800 ms" figure.
    ///
    /// Nothing is printed inside the loop: serial at 115200 is far slower than the event stream,
    /// so a per-report print would make the instrument change what it measures. The table is
    /// collected first and witnessed after, which also lets a name arriving in a later report be
    /// attached to a device first heard without one.
    /// Returns whether the drain ended on a HALTED event endpoint (`BtEvt::Stop` from
    /// `QTD_ERR_MASK`). The caller needs that fact to decide whether the mandatory scan-disable may
    /// read its own `CommandComplete` — see `BtEvt::Stop`.
    ///
    /// BT-L3 — also returns the PEER L3 will try to connect to: `(address, address type)`, chosen
    /// AFTER the window from the merged device table by the selection pass below. `None` when
    /// nothing passed the filters — and then L3 issues no create at all, so there is nothing to
    /// cancel or disconnect.
    ///
    /// The filters, and why selection is not made inside the drain loop: the primary rule is
    /// `BT_L3_PEER_NAME` (Peter's ruling, white board Q6 — connect to HIS speaker, by advertised
    /// name), and a device's Local Name may arrive in a LATER report than its first sighting, or in
    /// a scan response overheard from someone else's active scan. A first-heard rule evaluated
    /// in-loop would judge a device on a name it had not yet said. The name decode is the AD walk
    /// below, reused unchanged — there is no second name parser.
    #[cfg(feature = "bt")]
    unsafe fn bt_le_drain(
        &mut self,
        e: &BtEvtEp,
        toggle: &mut bool,
        armed: &mut bool,
    ) -> (bool, Option<([u8; 6], u8)>) {
        // Window in TSC units. `tsc_hz()` is 0 only if calibration failed or ran too early.
        //
        // UNCALIBRATED FALLBACK, stated honestly: with `tsc_hz() == 0` there is no cycles->time
        // mapping at all, so no fallback can be `BT_L2_SCAN_MS` in wall-clock terms — the best
        // available is a deliberately chosen CYCLE count. `hw_wait_budget()` in that state returns
        // the fixed `HW_WAIT_BUDGET` = 2.5e9-cycle guess (NOT 2 s of anything), so a quarter of it
        // is 625e6 cycles: ~0.27 s on the 2.3 GHz bench part, ~0.13 s at 5 GHz, ~0.63 s at 1 GHz.
        // That is the same ORDER as the nominal 500 ms window across the plausible clock range,
        // which is the whole of the claim — it is a bounded guess, not a 500 ms window.
        //
        // The rollup below prints the window through `epace_ms`, which also needs `tsc_hz()`; with
        // it zero the rollup reads `window=0ms(nominal 500ms)`. THAT PAIR IS THE UNCALIBRATED
        // SIGNATURE — a zero window in the witness means the TSC was uncalibrated, never that the
        // drain did not run.
        let hz = crate::arch::x86_64::apic::tsc_hz();
        let win_cy = if hz != 0 {
            (hz / 1000).saturating_mul(BT_L2_SCAN_MS)
        } else {
            crate::arch::hw_wait_budget() / 4
        };
        let t0 = crate::arch::now_cycles();

        let mut devs = [BtDev::default(); BT_L2_MAX_DEV];
        let mut ndev = 0usize;
        let mut dropped = 0u32; // reports whose address the table had no room for
        let mut reports = 0u32; // advertising reports decoded
        let mut events = 0u32; // whole events reassembled
        let mut other = 0u32; // events that were not LE Advertising Reports
        let mut malformed = 0u32; // events that claimed to be but did not parse
        let mut multi = 0u32; // events declaring Num_Reports > 1
        let mut extra = 0u32; // reports inside those events that were NOT decoded
        let mut halted = false;
        // BT-L3 — the selected advertiser, and nothing else about it. Filled AFTER the window by
        // the selection pass over the merged device table, not inside the drain loop.
        let mut peer: Option<([u8; 6], u8)> = None;
        let mut asm = [0u8; BT_EVT_ASM_MAX];

        loop {
            let el = crate::arch::now_cycles().wrapping_sub(t0);
            if el >= win_cy {
                break;
            }
            // The DRAIN's continuations keep the full budget deliberately: an advertising report
            // arriving in the last milliseconds of the window is worth finishing, and the drain is
            // the one caller that has no teardown behind it waiting on the clock. L2's stated
            // bound is the one it has always had — see the cost paragraph on this function.
            let (len, trunc) = match self.bt_read_full_event(
                e,
                toggle,
                armed,
                win_cy - el,
                crate::arch::hw_wait_budget(),
                &mut asm,
            ) {
                BtEvt::Got { len, trunc } => (len, trunc),
                // Window expired with nothing on the wire. The transfer stays armed (`*armed`);
                // the disable command consumes it.
                BtEvt::Idle(_) => break,
                BtEvt::Stop => {
                    halted = true;
                    break;
                }
            };
            if len < 2 {
                continue; // zero-length packet: not an event
            }
            events += 1;
            if trunc {
                // REASSEMBLY TRUNCATION IS UNREACHABLE FOR A SPEC-CONFORMING EVENT. An HCI event is
                // at most EventCode(1) + Parameter_Total_Length(1) + 255 = 257 bytes, and the
                // reassembly cap `BT_EVT_ASM_MAX` is 260 — so `trunc` can only be set by an event
                // that declared more than the spec allows, or by the packet-count ceiling. What
                // that means for the witness: `malformed=` in the rollup is driven by the PARSE
                // GUARDS below (num==0, len<13, a data length past the event), not by reassembly.
                // A nonzero `malformed=` is a statement about event CONTENT, not about buffering.
                malformed += 1;
                continue;
            }
            let pkt = &asm[..len];
            // LE Meta Event: EventCode(1)=0x3E Parameter_Total_Length(1) Subevent_Code(1) ...
            if pkt[0] != BT_EVT_LE_META || len < 4 || pkt[2] != BT_LE_SUBEVT_ADV_REPORT {
                other += 1;
                continue;
            }
            // LE Advertising Report: Num_Reports(1) then, per report, Event_Type(1)
            // Address_Type(1) Address(6) Length_Data(1) Data(Length_Data) RSSI(1).
            let num = pkt[3];
            if num == 0 {
                malformed += 1;
                continue;
            }
            if num > 1 {
                // The spec renders the fields as parallel arrays for Num_Reports > 1; controllers
                // in practice emit exactly one. Rather than guess a layout this arc has not seen
                // on the wire, the FIRST report is decoded and the remainder are COUNTED and named
                // in the rollup.
                multi += 1;
                extra += (num - 1) as u32;
            }
            if len < 13 {
                malformed += 1;
                continue;
            }
            let evt_type = pkt[4];
            let atype = pkt[5];
            let mut addr = [0u8; 6];
            addr.copy_from_slice(&pkt[6..12]);
            let dlen = pkt[12] as usize;
            if len < 13 + dlen + 1 {
                malformed += 1;
                continue;
            }
            let data = &pkt[13..13 + dlen];
            let rssi = pkt[13 + dlen] as i8;
            reports += 1;

            // BT-L3 — THE PEER IS NOT CHOSEN HERE. It is chosen after the window, off the merged
            // device table below, and the reason is the NAME: a device's Local Name may arrive in
            // a LATER report than its first sighting (or in an overheard scan response), so a
            // first-heard rule evaluated inside this loop would judge a device on a name it had
            // not yet said. All this loop records is the sticky fact the table cannot otherwise
            // keep — that this address was heard advertising CONNECTABLY at least once — because
            // `devs[i].evt` is last-report-wins and a later SCAN_RSP would erase it.

            // AD structures: a sequence of (Length(1), AD_Type(1), AD_Data(Length-1)). Walk far
            // enough to find a local name; a Complete Local Name (0x09) ends the walk, a Shortened
            // one (0x08) is kept but the walk continues in case the complete name follows.
            let mut name = [0u8; BT_L2_NAME_MAX];
            let mut nlen = 0usize;
            let mut ncut = false;
            let mut off = 0usize;
            while off + 2 <= dlen {
                let l = data[off] as usize;
                if l == 0 || off + 1 + l > dlen {
                    break; // 0 = end of significant part; over-long = malformed tail, stop
                }
                let ty = data[off + 1];
                if ty == BT_AD_NAME_COMPLETE || ty == BT_AD_NAME_SHORT {
                    let src = &data[off + 2..off + 1 + l];
                    // An EMPTY name field (Length==1: type byte only, no data) is legal on the air
                    // and carries no name. Without this guard a Complete Local Name of zero bytes
                    // ERASED a Shortened name captured earlier in the same walk — a device that
                    // advertises "Pete" then an empty complete name would print name=(none). An
                    // empty field is skipped; a COMPLETE one still ends the walk, because the
                    // device has told us there is no longer name to wait for.
                    if !src.is_empty() {
                        let take = src.len().min(BT_L2_NAME_MAX);
                        name[..take].copy_from_slice(&src[..take]);
                        nlen = take;
                        ncut = src.len() > BT_L2_NAME_MAX;
                    }
                    if ty == BT_AD_NAME_COMPLETE {
                        break;
                    }
                }
                off += 1 + l;
            }

            // Merge into the distinct-device table, keyed by (address, address type).
            let mut hit = None;
            for i in 0..ndev {
                if devs[i].addr == addr && devs[i].atype == atype {
                    hit = Some(i);
                    break;
                }
            }
            match hit {
                Some(i) => {
                    devs[i].reports = devs[i].reports.saturating_add(1);
                    devs[i].rssi = rssi; // latest, not an average this arc has not earned
                    devs[i].evt = evt_type;
                    // STICKY, unlike `evt`: connectability is a fact about the device, and a later
                    // SCAN_RSP or ADV_NONCONN_IND from the same address does not un-say it.
                    devs[i].conn_seen |= evt_type == BT_L3_ADV_CONNECTABLE;
                    if devs[i].nlen == 0 && nlen > 0 {
                        devs[i].name = name;
                        devs[i].nlen = nlen as u8;
                        devs[i].ncut = ncut;
                    }
                }
                None if ndev < BT_L2_MAX_DEV => {
                    devs[ndev] = BtDev {
                        addr,
                        atype,
                        evt: evt_type,
                        rssi,
                        name,
                        nlen: nlen as u8,
                        ncut,
                        reports: 1,
                        conn_seen: evt_type == BT_L3_ADV_CONNECTABLE,
                    };
                    ndev += 1;
                }
                None => dropped += 1,
            }
        }

        let elapsed = epace_ms(crate::arch::now_cycles().wrapping_sub(t0)).unwrap_or(0);

        // ---- BT-L3: SELECT THE PEER, off the merged table --------------------------------------
        // After the window, not inside it, and the NAME is why. A device's Local Name may arrive in
        // a later report than its first sighting, or in a scan response overheard from someone
        // else's active scan; the table has merged all of that by now, so every candidate is judged
        // on everything it said rather than on the first thing it said. It is also free: nothing
        // prints inside the drain loop (see this function's note on serial cost), and this pass
        // runs once over at most `BT_L2_MAX_DEV` entries with the radio already quiet.
        //
        // A VERDICT PER CANDIDATE, carried onto that device's own witness line below, so a capture
        // answers "why not that one?" for every device in the room without a second pass by hand.
        let mut verdict = [BT_L3_V_NOT_CONNECTABLE; BT_L2_MAX_DEV];
        let mut considered = 0u32; // devices that were connectable and could be judged at all
        let mut matched = 0u32; // devices that passed every filter (only the first is used)
        for i in 0..ndev {
            let d = devs[i];
            if !d.conn_seen {
                continue; // verdict stays NOT_CONNECTABLE
            }
            // Only Public (0x00) and Random (0x01) may go into `HCI_LE_Create_Connection`'s
            // Peer_Address_Type. 0x02/0x03 are the RESOLVED IDENTITY forms: a 4.0 part does not
            // accept them there, and this arc has no resolving list to have produced one honestly.
            // Posting one raw would be an out-of-range parameter dressed as a peer.
            if d.atype != 0x00 && d.atype != 0x01 {
                verdict[i] = BT_L3_V_ATYPE;
                continue;
            }
            considered += 1;
            match BT_L3_PEER_NAME {
                // THE NAME FILTER — Peter's ruling. Only a device whose advertised Local Name
                // contains the target is eligible, however loud or however early anything else is.
                Some(want) if !want.is_empty() => {
                    if d.nlen == 0 {
                        verdict[i] = BT_L3_V_NO_NAME;
                        continue;
                    }
                    if !bt_name_contains_ci(&d.name[..d.nlen as usize], want.as_bytes()) {
                        verdict[i] = BT_L3_V_NAME_MISMATCH;
                        continue;
                    }
                }
                // No name filter: the RSSI floor is the whole of the mitigation, so it applies.
                // (`Some("")` falls here on purpose — an empty needle matches everything, which is
                // "no filter" written by accident, and it is not honoured as one.)
                _ => {
                    if d.rssi == BT_L3_RSSI_NA {
                        verdict[i] = BT_L3_V_RSSI_NA;
                        continue;
                    }
                    if d.rssi < BT_L3_RSSI_FLOOR {
                        verdict[i] = BT_L3_V_BELOW_FLOOR;
                        continue;
                    }
                }
            }
            matched += 1;
            if peer.is_none() {
                peer = Some((d.addr, d.atype));
                verdict[i] = BT_L3_V_SELECTED;
            } else {
                // A second device answering to the same name is not an error, but connecting to
                // both is not on offer and picking silently would hide the ambiguity.
                verdict[i] = BT_L3_V_ALSO_MATCHED;
            }
        }

        // ---- witness: one line per distinct device -------------------------------------------
        for i in 0..ndev {
            let d = devs[i];
            // BD_ADDR travels little-endian (LSB first) and is rendered MSB-first, the human
            // notation — the same order L1's `bd_addr=` line uses.
            serial_print!(
                ":: bt-l2: [{}] dev {:02} addr={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} type={} evt={} rssi=",
                self.idx, i + 1,
                d.addr[5], d.addr[4], d.addr[3], d.addr[2], d.addr[1], d.addr[0],
                match d.atype {
                    0x00 => "public",
                    0x01 => "random",
                    0x02 => "public-identity",
                    0x03 => "random-identity",
                    _ => "reserved",
                },
                match d.evt {
                    0x00 => "ADV_IND",
                    0x01 => "ADV_DIRECT_IND",
                    0x02 => "ADV_SCAN_IND",
                    0x03 => "ADV_NONCONN_IND",
                    0x04 => "SCAN_RSP",
                    _ => "reserved",
                }
            );
            if d.rssi == 127 {
                serial_print!("n/a");
            } else {
                serial_print!("{}dBm", d.rssi);
            }
            serial_print!(" reports={} name=", d.reports);
            if d.nlen == 0 {
                serial_print!("(none)");
            } else {
                serial_print!("\"");
                for j in 0..d.nlen as usize {
                    let b = d.name[j];
                    // Names are UTF-8 on the air; the serial witness is ASCII, so anything outside
                    // printable ASCII is shown as '.' rather than corrupting the line.
                    serial_print!("{}", if (0x20..0x7F).contains(&b) { b as char } else { '.' });
                }
                serial_print!("\"{}", if d.ncut { "~(cut)" } else { "" });
            }
            // BT-L3 — the verdict for THIS device, decided above. `~(cut)` next to a
            // `SKIP:name-mismatch` is the one combination worth reading twice: the match was tried
            // against a name this arc truncated at BT_L2_NAME_MAX, so it may be a false miss.
            serial_println!(" l3={} == witness ::", verdict[i]);
        }

        // ---- witness: the rollup --------------------------------------------------------------
        serial_println!(
            ":: bt-l2: [{}] LE scan rollup — window={}ms(nominal {}ms) distinct_devices={} adv_reports={} events={} non_adv_events={} malformed={} multi_report_events={}(extra_reports_not_decoded={}) {} == witness ::",
            self.idx, elapsed, BT_L2_SCAN_MS, ndev, reports, events, other, malformed, multi, extra,
            if dropped > 0 {
                "table TRUNCATED at the cap — further distinct addresses were heard and are NOT listed"
            } else {
                "table complete (no truncation)"
            }
        );
        if dropped > 0 {
            serial_println!(
                ":: bt-l2: [{}] LE scan TRUNCATION — {} report(s) named address(es) past the {}-device table cap; the device list above is a PREFIX of what was on the air, not all of it ::",
                self.idx, dropped, BT_L2_MAX_DEV
            );
        }
        if halted {
            serial_println!(
                ":: bt-l2: [{}] LE scan ENDED EARLY — the event endpoint stopped being usable mid-window; the counts above cover only the part of the window that ran ::",
                self.idx
            );
        }
        // ZERO DEVICES is only a statement about the AIR if the window actually ran to term. On a
        // halt the drain stopped early and the endpoint, not the room, is the story — the
        // ENDED EARLY line above already says so, and claiming silence on top of it would be a
        // second, wrong explanation for the same zero. The window quoted is the MEASURED `elapsed`,
        // not the nominal constant, so a short window cannot masquerade as a full one.
        if ndev == 0 && !halted {
            // The failure mode L1's review warned about (a masked LE Meta channel) is ruled out by
            // construction here: both mask writes above returned status 0x00 and are witnessed, or
            // this drain never ran. So zero means nothing was heard, not that nothing was routed.
            serial_println!(
                ":: bt-l2: [{}] LE scan found ZERO devices. Both the Event Mask (LE Meta, bit 61) and the LE Event Mask (Advertising Report, bit 1) were written and CONFIRMED above, so this is silence on the air across the {}ms measured (nominal {}ms) — not a masked event stream. A bounded window is not a survey: devices advertising slower than it can be missed ::",
                self.idx, elapsed, BT_L2_SCAN_MS
            );
        }
        // BT-L3 — witness the PICK before anything is done with it, so a capture always shows which
        // address L3 aimed at (or that it had nothing to aim at) independently of what happened next.
        //
        // THE SELECTION RULE ITSELF is witnessed first, unconditionally, because a capture must
        // say WHO WAS ELIGIBLE before it says who was picked — a run that connected to nobody and
        // a run that was forbidden from connecting to anybody are different runs.
        match BT_L3_PEER_NAME {
            Some(want) if !want.is_empty() => serial_println!(
                ":: bt-l3: [{}] peer rule — NAME FILTER ARMED, name=\"{}\" (case-insensitive substring of the advertised Local Name; white board Q6, Peter's ruling: the bench connects to HIS OWN speaker and to nothing else). The RSSI floor is NOT applied — a named peer across the room is still the right peer. The scan is PASSIVE, so a name carried only in a SCAN_RSP is heard only if someone else solicits it == witness ::",
                self.idx, want
            ),
            _ => serial_println!(
                ":: bt-l3: [{}] peer rule — NAME FILTER UNSET: the peer is the first connectable advertiser of the window that clears the RSSI floor of {}dBm. That floor is a NEARBY-ONLY mitigation and not an identity check — a loud stranger can clear it, and connecting to a stranger's keyboard takes it from its owner for the duration == witness ::",
                self.idx, BT_L3_RSSI_FLOOR
            ),
        }
        match peer {
            Some((a, ty)) => serial_println!(
                ":: bt-l3: [{}] peer SELECTED addr={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} type={} — heard advertising connectably (ADV_IND, Event_Type 0x00) and passed every filter; considered={} matched={} == witness ::",
                self.idx, a[5], a[4], a[3], a[2], a[1], a[0],
                if ty == 0x00 { "public" } else { "random" },
                considered, matched
            ),
            None => serial_println!(
                ":: bt-l3: [{}] peer NOT SELECTED — no device passed the filters: name={} considered={} matched=0. NO HCI_LE_Create_Connection is issued, nothing is outstanding, and there is nothing to cancel or disconnect. The ordinary reading with a name filter armed is that the named device is OFF or OUT OF RANGE; the per-device l3= verdicts above say which of the two the room looked like == witness ::",
                self.idx,
                match BT_L3_PEER_NAME { Some(w) if !w.is_empty() => w, _ => "(unset)" },
                considered
            ),
        }
        (halted, peer)
    }

    /// BT-L0 — build + link the periodic QH for the HCI event endpoint. Same QH shape and same
    /// frame-list splice as `arm_interrupt_ep` (including the split masks for a FS endpoint
    /// behind a TT), minus the `int_eps` registration: nothing here is ever handed to
    /// `service()`. Returns None (with a trace) on a second arm attempt (`bt_evt_armed` —
    /// the quiesced QH stays linked, so re-arming would self-loop the frame list), on mps=0,
    /// or on a phys-contract violation. It never touches the HID slot pool.
    #[cfg(feature = "bt")]
    unsafe fn bt_arm_events(&mut self, t: &Target, ep: u8, mps: u16) -> Option<BtEvtEp> {
        // MTFIX: the event endpoint owns `bt_slot`, not one of the HID `int_slots` — see the
        // field's doc-comment in `qh.rs` for the Boot AN conviction. The slot is single, and
        // `bt_quiesce_events` leaves its QH LINKED in the periodic chain for the life of the boot
        // (the chain must not be rewritten behind endpoints armed after it), so re-arming it for a
        // second radio would rebuild a QH the controller is still walking AND splice its own
        // physical address in as its own `horiz` successor — a self-loop in the frame list. One
        // arm per controller, refused honestly.
        if self.bt_evt_armed {
            serial_println!(
                ":: bt-l0: [{}] HCI event endpoint slot already owned by an earlier radio on this controller — not armed ::",
                self.idx
            );
            return None;
        }
        let mps = mps.min(INT_BUF_LEN as u16);
        if mps == 0 {
            serial_println!(":: bt-l0: [{}] HCI event endpoint reports mps=0 — not armed ::", self.idx);
            return None;
        }
        let slot = &mut (*self.pool()).bt_slot;
        let (qh, qtd, buf) = (
            &mut slot.qh as *mut Qh,
            &mut slot.qtd as *mut Qtd,
            slot.buf.0.as_mut_ptr(),
        );
        let (Some(qh_phys), Some(qtd_phys), Some(buf_phys)) =
            (phys_of(qh, 32), phys_of(qtd, 32), phys_of(buf, INT_BUF_ALIGN))
        else {
            serial_println!(
                ":: bt-l0: [{}] STOP-NOTE int-EP slot failed the phys/alignment contract — HCI event endpoint not armed ::",
                self.idx
            );
            return None;
        };
        self.bt_evt_armed = true;

        (*qh).ep_chars = (t.addr as u32)
            | ((ep as u32) << 8)
            | t.eps
            | QH_DTC
            | ((mps as u32) << QH_MPS_SHIFT);
        // N1 (see `arm_interrupt_ep`): S-mask/C-mask are microframe masks evaluated within every
        // frame the QH is reached in, so an every-frame frame list stays split-correct. The TT
        // fields are `t.hub_addr`/`t.hub_port` — for the Bluetooth controller these are the
        // INHERITED ones from `bring_up_hub` (the SMSC hub), which is the whole point of this arc.
        let split = if t.eps == QH_EPS_HIGH {
            0
        } else {
            (0x1C << QH_CMASK_SHIFT)
                | ((t.hub_addr as u32) << QH_HUBADDR_SHIFT)
                | ((t.hub_port as u32) << QH_PORT_SHIFT)
        };
        (*qh).ep_caps = QH_MULT1 | (0x01 << QH_SMASK_SHIFT) | split;

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
        Some(BtEvtEp { qh, qtd, qtd_phys, buf, buf_phys, mps })
    }

    /// BT-L0 — arm ONE interrupt-IN transfer on the event endpoint. Does NOT wait.
    ///
    /// BT-L2 split this out of `bt_read_event` so that a read can be *polled to a deadline*
    /// instead of always spending a full `hw_wait_budget()`: the LE-scan drain reads on a
    /// wall-clock window, and at 2 s per silent read (`HW_WAIT_SECONDS`) a bounded window is
    /// impossible without separating the arm from the wait. Arming over an already-armed transfer
    /// would clobber a qTD the controller may be executing — every caller must therefore know
    /// whether one is outstanding (see `BtEvt::Idle`). Mirrors the two transfer modes the driver
    /// self-selects (overlay-direct on this metal, qTD-chain on QEMU).
    #[cfg(feature = "bt")]
    unsafe fn bt_arm_read(&mut self, e: &BtEvtEp, toggle: bool) {
        let dt = if toggle { QTD_DT } else { 0 };
        let total = e.mps as u32;
        if self.overlay_mode {
            (*e.qh).current_qtd = 0;
            (*e.qh).overlay[0] = PTR_TERMINATE;
            (*e.qh).overlay[1] = PTR_TERMINATE;
            (*e.qh).overlay[3] = e.buf_phys as u32;
            (*e.qh).overlay[4] = 0;
            core::ptr::write_volatile(
                &mut (*e.qh).overlay[2],
                QTD_ACTIVE | QTD_CERR3 | (total << QTD_TOTAL_SHIFT) | QTD_PID_IN | QTD_IOC | dt,
            );
        } else {
            write_qtd(e.qtd, PTR_TERMINATE, QTD_PID_IN | QTD_IOC | dt, total, e.buf_phys);
            (*e.qh).overlay[1] = PTR_TERMINATE;
            (*e.qh).overlay[2] = 0;
            (*e.qh).overlay[0] = e.qtd_phys as u32;
        }
    }

    /// BT-L0/L2 — poll the ARMED interrupt-IN transfer for at most `budget` `now_cycles()` units.
    ///
    /// Returns `Some(len)` when the transfer retired (possibly 0 — a zero-length packet retires a
    /// transfer too), or `None` on budget expiry **or** a halted endpoint; `*halted` distinguishes
    /// the two, because they are opposite facts: on expiry the transfer is STILL ARMED and the
    /// endpoint is fine, on a halt the endpoint is retired. Prints nothing on expiry — L2 expires
    /// on purpose, once per scan window — but does witness a halt, which is never routine.
    #[cfg(feature = "bt")]
    unsafe fn bt_wait_read(&mut self, e: &BtEvtEp, budget: u64, halted: &mut bool) -> Option<usize> {
        *halted = false;
        let om = self.overlay_mode;
        let (qh, qtd) = (e.qh, e.qtd);
        let read_tok = || {
            if om {
                core::ptr::read_volatile(&(*qh).overlay[2])
            } else {
                core::ptr::read_volatile(&(*qtd).token)
            }
        };
        // BOUNDED, exactly as `wait_bounded` is, but on a CALLER-SUPPLIED budget: L0/L1 pass
        // `hw_wait_budget()` and get the pre-L2 behaviour byte for byte; L2's drain passes what is
        // left of its scan window. A radio that never answers costs one budget, not a hung boot.
        let start = crate::arch::now_cycles();
        let mut done = false;
        loop {
            if read_tok() & QTD_ACTIVE == 0 {
                done = true;
                break;
            }
            if crate::arch::now_cycles().wrapping_sub(start) >= budget {
                break;
            }
            core::hint::spin_loop();
        }
        let tok = read_tok();
        if !done {
            return None;
        }
        if tok & QTD_ERR_MASK != 0 {
            *halted = true;
            serial_println!(
                ":: bt-l0: [{}] STOP-NOTE HCI event endpoint halted (token={:#010x}) — endpoint retired, not forced ::",
                self.idx, tok
            );
            return None;
        }
        Some(((e.mps as u32).saturating_sub((tok >> QTD_TOTAL_SHIFT) & 0x7FFF)) as usize)
    }

    /// BT-L1/L2 — reassemble ONE complete HCI event off the event endpoint.
    ///
    /// The event endpoint's max packet is 16 B, but an HCI event runs up to 2 + 255 B. One
    /// interrupt-IN transaction is one packet; a whole event is ceil(len/mps) of them, and the data
    /// toggle advances on EVERY packet regardless of event boundaries. So: the first packet gives
    /// the event's total length (`2 + Parameter_Total_Length`) and the rest are read (toggling
    /// each time) until the event is gathered.
    ///
    /// `armed` says a transfer is ALREADY outstanding (L2's drain leaves exactly one behind when
    /// its window expires); it is consumed rather than re-armed, and cleared. `first_budget`
    /// bounds only the FIRST packet; `cont_budget` bounds the CONTINUATION PHASE — every packet
    /// after the first, taken together, not each.
    ///
    /// WHY CONTINUATIONS ARE NOW BOUNDED SEPARATELY, and why that is a defect fix and not a knob:
    /// this function used to give every continuation packet the whole of `hw_wait_budget()`
    /// (~1.1 s calibrated on the bench part, 2.5e9 cycles uncalibrated). A caller that believed it
    /// had bought a 300 ms window could therefore stall for that window PLUS one full budget per
    /// continuation packet — and an `LE Advertising Report` is three or more packets on a 16 B
    /// endpoint. L3 makes up to six bounded waits, so the arc's "3.0 s worst case" was out by
    /// roughly four times the wait budget. Passing the caller's REMAINING window down makes the
    /// bound the caller states the bound it actually gets. The reason continuations were unbounded
    /// in the first place still holds and is preserved: abandoning an event half-read desynchronises
    /// the toggle, so a continuation expiry returns `Stop` (the endpoint is finished with) while a
    /// first-packet expiry returns `Idle` (nothing was lost). Callers that genuinely want the old
    /// behaviour — `bt_hci_command`, whose first-packet budget IS `hw_wait_budget()` — pass it.
    #[cfg(feature = "bt")]
    unsafe fn bt_read_full_event(
        &mut self,
        e: &BtEvtEp,
        toggle: &mut bool,
        armed: &mut bool,
        first_budget: u64,
        cont_budget: u64,
        asm: &mut [u8],
    ) -> BtEvt {
        let cap = asm.len().min(BT_EVT_ASM_MAX);
        if !*armed {
            self.bt_arm_read(e, *toggle);
        }
        // ARMED IS A FACT ABOUT THE CONTROLLER, NOT A WISH. From here a transfer IS outstanding,
        // and `*armed` is cleared only where one of two things actually retired it: a successful
        // `bt_wait_read` (the qTD completed), or a halt (the endpoint retired it itself). It is
        // NEVER cleared on the `!halted` budget expiry: there the qTD is still ACTIVE and
        // CONTROLLER-OWNED, and a cleared flag would let the next caller `bt_arm_read` a second
        // qTD over a live one — a DMA race on the shared buffer plus a toggle desync. This is the
        // invariant `BtEvt::Idle` documents, and it must hold on the MID-EVENT expiry below too,
        // which returns `Stop` rather than `Idle` but leaves the same live transfer behind.
        *armed = true;
        let mut halted = false;
        let Some(n0) = self.bt_wait_read(e, first_budget, &mut halted) else {
            if halted {
                // A halt retires the transfer with the endpoint: nothing is outstanding.
                *armed = false;
                return BtEvt::Stop;
            }
            // Budget expired with the transfer still armed and the toggle unadvanced.
            let tok = if self.overlay_mode {
                core::ptr::read_volatile(&(*e.qh).overlay[2])
            } else {
                core::ptr::read_volatile(&(*e.qtd).token)
            };
            return BtEvt::Idle(tok);
        };
        *armed = false;
        *toggle = !*toggle;
        if n0 == 0 {
            // A zero-length packet retires a transfer with nothing to parse. Not an event.
            return BtEvt::Got { len: 0, trunc: false };
        }
        let take0 = n0.min(e.mps as usize).min(cap);
        core::ptr::copy_nonoverlapping(e.buf, asm.as_mut_ptr(), take0);
        let mut have = take0;
        if have < 2 {
            return BtEvt::Got { len: have, trunc: false }; // malformed: no event header
        }
        let total = 2 + asm[1] as usize; // EventCode(1) Parameter_Total_Length(1) + params
        let mut trunc = false;
        let max_pkts = cap / (e.mps as usize).max(1) + 2;
        let mut pkts = 1;
        // The continuation phase's own clock. `cont_budget` is the budget for the WHOLE phase, so
        // each packet gets what is left of it — an event that arrives one packet at a time cannot
        // multiply the caller's window by its packet count.
        let tc0 = crate::arch::now_cycles();
        while have < total {
            if pkts >= max_pkts {
                trunc = true;
                break;
            }
            let cont_rem =
                cont_budget.saturating_sub(crate::arch::now_cycles().wrapping_sub(tc0));
            self.bt_arm_read(e, *toggle);
            *armed = true;
            let Some(ni) = self.bt_wait_read(e, cont_rem, &mut halted) else {
                if !halted {
                    // The qTD is STILL ACTIVE and controller-owned. `Stop` here means the EVENT is
                    // lost (the toggle's relationship to the device is gone), not that the transfer
                    // is gone — so `*armed` stays TRUE and is handed forward, exactly as on the
                    // `Idle` path. Clearing it here was the bug: the mandatory scan-disable's own
                    // `bt_read_full_event` would then have armed a second qTD over this live one.
                    serial_println!(
                        ":: bt-l0: [{}] STOP-NOTE HCI event IN timed out mid-event ({} of {} bytes) — not forced; the transfer is left ARMED and handed forward ::",
                        self.idx, have, total
                    );
                } else {
                    *armed = false;
                }
                return BtEvt::Stop;
            };
            *armed = false;
            *toggle = !*toggle;
            pkts += 1;
            let ni = ni.min(e.mps as usize);
            if ni == 0 {
                // A short/zero packet before `total` ends the event early — treat what we have as
                // the whole of it rather than reading into the next event.
                break;
            }
            let room = cap - have;
            let store = ni.min(room);
            core::ptr::copy_nonoverlapping(e.buf, asm.as_mut_ptr().add(have), store);
            have += store;
            if store < ni {
                // Buffer full but the event continues; we have already read this packet off the
                // endpoint (sync preserved). Nothing more can be stored.
                trunc = true;
                break;
            }
            if ni < e.mps as usize {
                break; // short packet = last packet of the event
            }
        }
        BtEvt::Got { len: have, trunc }
    }

    /// BT-L0/L1 — issue one HCI command over the control endpoint and drain the event endpoint
    /// until its Command Complete arrives.
    ///
    /// The command rides EP0 exactly as the Bluetooth USB transport specifies: bmRequestType
    /// 0x20 (host-to-device, CLASS, INTERFACE), bRequest 0x00, wValue 0, wIndex = the HCI
    /// interface, data = the HCI command packet `opcode(2, LE) parameter_total_length(1)
    /// parameters(N)`. `params` is empty for the L0 reads and the L1 identity reads, and eight
    /// bytes for `HCI_Set_Event_Mask`; it is bounded by 255 (the length field is one byte) and by
    /// the EP0 data buffer.
    ///
    /// BT-L1 — MULTI-PACKET EVENT REASSEMBLY. The event endpoint's max packet is 16 B, but an HCI
    /// event runs up to 2 + 255 B (`HCI_Read_Local_Supported_Commands` alone is 70). One
    /// `bt_read_event` is one interrupt-IN transaction = one packet; a whole event is
    /// ceil(len/mps) of them, and the data toggle advances on EVERY packet regardless of event
    /// boundaries. So this function reassembles: the first packet gives the event's total length
    /// (`2 + Parameter_Total_Length`), and it keeps reading (toggling each time) until the whole
    /// event is gathered, before parsing. A non-target event (vendor, Command Status) is
    /// reassembled in full and discarded so the endpoint stays byte-synchronised for the next one.
    ///
    /// Returns the matching Command Complete's RETURN PARAMETERS (everything after the opcode
    /// echo) copied into `out`, and the number of bytes copied (`min(actual, out.len())`); None on
    /// send failure or if no matching Command Complete arrived within `BT_EVT_MAX` bounded events.
    #[cfg(feature = "bt")]
    unsafe fn bt_hci_command(
        &mut self,
        t: &Target,
        intf: u8,
        e: &BtEvtEp,
        toggle: &mut bool,
        opcode: u16,
        params: &[u8],
        out: &mut [u8],
        // BOUNCE FIX (finding 3): this used to pass `&mut false`, DISCARDING the armed-out. On the
        // L0/L1 path a command that timed out on its first packet left a live qTD behind and the
        // next command armed a second one over it — the same DMA race + toggle desync as the L2
        // bug, one layer down. `bt_probe` now owns ONE `armed` flag and threads it through every
        // L0/L1/L2 command, so the fact is never dropped on the floor.
        armed: &mut bool,
    ) -> Option<usize> {
        self.bt_hci_command_ex(t, intf, e, toggle, opcode, params, out, armed)
    }

    /// BT-L0/L2 — write ONE HCI command packet into the EP0 data buffer and SEND it. Reads
    /// nothing. Returns whether the control-OUT succeeded (a failure witnesses itself).
    ///
    /// Split out of `bt_hci_command_ex` for the one case where the reply must not be read: when
    /// the event endpoint has HALTED, `BtEvt::Stop` forbids further event reads, but the mandatory
    /// `HCI_LE_Set_Scan_Enable(disable)` still has to reach the radio — and it rides EP0, which the
    /// halt did not touch. See `BtEvt::Stop`.
    #[cfg(feature = "bt")]
    unsafe fn bt_hci_send(&mut self, t: &Target, intf: u8, opcode: u16, params: &[u8]) -> bool {
        // The command packet: opcode(2, LE) parameter_total_length(1) parameters(N), written into
        // the EP0 data buffer `control` sends from. `params` is capped by the length field (255)
        // and by the buffer; L1's largest is the 8-byte event mask, so this never truncates in
        // practice, but the guard keeps a future long-parameter command honest.
        // 253 = the 256-byte EP0 data buffer (`qh::Buf256`) minus the 3-byte command header.
        let plen = params.len().min(255).min(253);
        self.data_buf.write(opcode as u8);
        self.data_buf.add(1).write((opcode >> 8) as u8);
        self.data_buf.add(2).write(plen as u8);
        for (i, &b) in params[..plen].iter().enumerate() {
            self.data_buf.add(3 + i).write(b);
        }
        let wlen = (3 + plen) as u16;
        if self.control(t, 0x20, 0x00, 0, intf as u16, wlen, false).is_err() {
            serial_println!(
                ":: bt-l0: [{}] HCI command {:#06x} — control-OUT failed on EP0 ::",
                self.idx, opcode
            );
            return false;
        }
        true
    }

    /// BT-L2 — `bt_hci_command` with the pre-armed hand-off.
    ///
    /// `armed` in => the LE-scan drain left one interrupt-IN transfer outstanding when its window
    /// expired; that transfer is a perfectly good read and this command's CommandComplete will
    /// land in it, so it is CONSUMED rather than re-armed over. `armed` out => this command left
    /// one outstanding in turn (only possible on the first-packet timeout path). This is what lets
    /// the mandatory `LE_Set_Scan_Enable(disable)` be issued straight out of a drain that ended on
    /// silence, without arming a second qTD over a live one.
    #[cfg(feature = "bt")]
    #[allow(clippy::too_many_arguments)]
    unsafe fn bt_hci_command_ex(
        &mut self,
        t: &Target,
        intf: u8,
        e: &BtEvtEp,
        toggle: &mut bool,
        opcode: u16,
        params: &[u8],
        out: &mut [u8],
        armed: &mut bool,
    ) -> Option<usize> {
        if !self.bt_hci_send(t, intf, opcode, params) {
            return None;
        }
        // Drain: a controller may emit unrelated events (vendor, Command Status) before the
        // Command Complete we asked for. Structurally bounded (`BT_EVT_MAX` whole events), on top
        // of each packet read's own TSC deadline, so a chatty or a mute radio both terminate.
        let mut asm = [0u8; BT_EVT_ASM_MAX];
        for _ in 0..BT_EVT_MAX {
            // ---- reassemble ONE complete event -------------------------------------------------
            let (have, trunc) = match self.bt_read_full_event(
                e,
                toggle,
                armed,
                crate::arch::hw_wait_budget(),
                // A COMMAND's continuation keeps the pre-existing full budget: L0/L1 read
                // 70-byte events (`Read_Local_Supported_Commands`) on this path and their
                // first-packet budget is already the full one, so there is no window to shrink to.
                crate::arch::hw_wait_budget(),
                &mut asm,
            ) {
                BtEvt::Got { len, trunc } => (len, trunc),
                BtEvt::Idle(tok) => {
                    // Same bound, same message, same "not forced" discipline as before L2 split
                    // the read primitives: on a COMMAND the full budget expiring is a timeout.
                    serial_println!(
                        ":: bt-l0: [{}] STOP-NOTE HCI event IN timed out (token={:#010x}) — not forced ::",
                        self.idx, tok
                    );
                    return None;
                }
                BtEvt::Stop => return None,
            };
            if have < 2 {
                continue; // zero-length or headerless packet: nothing to parse; retry
            }
            let pkt = &asm[..have];
            // ---- parse -------------------------------------------------------------------------
            let (code, params_len) = (pkt[0], pkt[1] as usize);
            if code != BT_EVT_CMD_COMPLETE {
                serial_println!(
                    ":: bt-l0: [{}] HCI event {:#04x} plen={} while awaiting CmdComplete for {:#06x} — skipped ::",
                    self.idx, code, params_len, opcode
                );
                continue;
            }
            // Command Complete parameters: Num_HCI_Command_Packets(1) Command_Opcode(2, LE)
            // Return_Parameters(...). The return parameters therefore start at packet offset 5.
            if pkt.len() < 5 {
                continue;
            }
            let echoed = (pkt[3] as u16) | ((pkt[4] as u16) << 8);
            if echoed != opcode {
                serial_println!(
                    ":: bt-l0: [{}] CmdComplete for {:#06x} (ncmd={}) while awaiting {:#06x} — skipped ::",
                    self.idx, echoed, pkt[2], opcode
                );
                continue;
            }
            if trunc {
                serial_println!(
                    ":: bt-l0: [{}] CmdComplete for {:#06x} TRUNCATED — event exceeds the {}-byte reassembly buffer ::",
                    self.idx, opcode, BT_EVT_ASM_MAX
                );
            }
            let ret = &pkt[5..];
            let copy = ret.len().min(out.len());
            out[..copy].copy_from_slice(&ret[..copy]);
            return Some(copy);
        }
        serial_println!(
            ":: bt-l0: [{}] no CmdComplete for {:#06x} within {} bounded events ::",
            self.idx, opcode, BT_EVT_MAX
        );
        None
    }

    /// BT-L0 — stop the event endpoint. The QH stays linked (its static slot is owned for the
    /// boot, and the frame-list chain must not be rewritten behind endpoints armed after it);
    /// clearing Active is what makes the controller skip it, exactly as a retired endpoint.
    #[cfg(feature = "bt")]
    unsafe fn bt_quiesce_events(&mut self, e: &BtEvtEp) {
        if self.overlay_mode {
            core::ptr::write_volatile(&mut (*e.qh).overlay[2], 0);
        } else {
            core::ptr::write_volatile(&mut (*e.qtd).token, 0);
            (*e.qh).overlay[0] = PTR_TERMINATE;
            core::ptr::write_volatile(&mut (*e.qh).overlay[2], 0);
        }
    }
    // ============================== end BT-L0 ================================================

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
        // ALLKEYS P1: the interface this endpoint belongs to, recorded so a lock-key press
        // discovered in the service loop can address SET_REPORT back at it. Both call sites
        // already have it in scope.
        intf: u8,
        // MTFIX: returns whether the endpoint is ACTUALLY armed. Both call sites printed their
        // `== witness` line (and stamped `bootlog`) unconditionally after calling this, so on Boot
        // AN the log carried `M1 armed vendor-multitouch addr=9` on the line immediately AFTER
        // `static int-EP pool exhausted (4) — endpoint skipped`, for the very endpoint that had
        // just been skipped. A witness that fires when the thing it witnesses did not happen is
        // worse than no witness: it is what made "the trackpad is armed and silent" the working
        // theory for a whole sitting. The verdict now comes from the arming path itself.
    ) -> bool {
        if self.int_next >= MAX_INT_EPS {
            serial_println!(
                ":: EHCI-HID: [{}] STOP-NOTE static int-EP pool exhausted ({}) — addr {} intf {} ep=IN{} NOT armed ::",
                self.idx, MAX_INT_EPS, t.addr, intf, ep
            );
            return false;
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
            return false;
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
            kbd_prev_keys: [0; 6],
            kbd_prev_mods: 0,
            // ALLKEYS P1: locks off at arm time — the state SET_CONFIGURATION just left the
            // device in, so software and hardware agree without an explicit sync request.
            kbd_leds: 0,
            kbd_target: *t,
            kbd_intf: intf,
            kbd_led_ok: true,
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
            // KBDWIT-2: the split-progress accumulators start clean. `kbdwit_split_prev` is seeded
            // with the pair the driver leaves behind — `overlay[4] = 0` on the overlay-direct path
            // above, and on qtd-chain the controller loads zeros out of `write_qtd`'s buffer array
            // — so the FIRST controller write already registers as a change rather than being
            // swallowed by an uninitialized baseline. `overlay[5]` is deliberately included in that
            // seed even though this driver never writes it: on overlay-direct it can hold residue
            // from an earlier completed transfer on this QH, and seeding from the live word means a
            // stale value is the baseline, not a phantom "walk".
            #[cfg(feature = "kbdwit")]
            kbdwit_polls: 0,
            #[cfg(feature = "kbdwit")]
            kbdwit_walks: 0,
            #[cfg(feature = "kbdwit")]
            kbdwit_split_prev: ((core::ptr::read_volatile(&(*qh).overlay[4]) as u64) << 32)
                | core::ptr::read_volatile(&(*qh).overlay[5]) as u64,
            #[cfg(feature = "kbdwit")]
            kbdwit_split_or: 0,
            #[cfg(feature = "kbdwit")]
            kbdwit_broke: false,
            #[cfg(feature = "mtraw")]
            rx_total,
            #[cfg(feature = "mtraw_inject")]
            mt_prev: None,
        });
        // MTFIX: armed, linked, and registered with `service()` — the only path that returns true.
        true
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
        // ALLKEYS P1: lock-LED writes discovered during the endpoint walk, deferred until after it.
        // A control transfer needs `&mut self`, which the `iter_mut()` borrow below rules out — the
        // same constraint `mt_dumped` above is working around. Sized `MAX_INT_EPS` so that if two
        // keyboards toggle a lock in the SAME service pass neither write is dropped; `Target` is
        // `Copy`, so the array is plain stack scratch and costs nothing when nothing toggles.
        let mut led_pushes: [Option<(usize, Target, u8, u8)>; MAX_INT_EPS] = [None; MAX_INT_EPS];
        let mut n_led = 0usize;
        #[cfg(feature = "mtraw")]
        let mut mt_dumped = if self.mt_probe.is_some() { Some(self.mt_dumped) } else { None };
        for (ep_i, e) in self.int_eps.iter_mut().enumerate() {
            if e.dead {
                continue;
            }
            let tok = if om {
                core::ptr::read_volatile(&(*e.qh).overlay[2])
            } else {
                core::ptr::read_volatile(&(*e.qtd).token)
            };
            if tok & QTD_ACTIVE != 0 {
                // KBDWIT-2: is the CONTROLLER actually walking to this queue head? Two volatile
                // reads of words only it writes (split progress — see `IntEp::kbdwit_walks`),
                // compared against the previous poll. This runs on every pass, before and after the
                // deadline dump, because the dump's `sched=` verdict and the `SILENCE-BROKE` line's
                // rate both read it. Read-only: nothing here writes a controller-visible word.
                #[cfg(feature = "kbdwit")]
                {
                    let split = ((core::ptr::read_volatile(&(*e.qh).overlay[4]) as u64) << 32)
                        | core::ptr::read_volatile(&(*e.qh).overlay[5]) as u64;
                    e.kbdwit_polls = e.kbdwit_polls.saturating_add(1);
                    e.kbdwit_split_or |= split;
                    if split != e.kbdwit_split_prev {
                        e.kbdwit_walks = e.kbdwit_walks.saturating_add(1);
                        e.kbdwit_split_prev = split;
                    }
                }
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
                // KBDWIT-2: the silence ended, but it ended in a HALT — see
                // `kbdwit_note_silence_end` for why this exit gets its own verdict word instead of
                // sharing `SILENCE-BROKE` with the clean one below.
                #[cfg(feature = "kbdwit")]
                kbdwit_note_silence_end(e, idx, "SILENCE-ENDED-HALTED", tok);
                serial_println!(
                    ":: EHCI-HID: [{}] STOP-NOTE interrupt endpoint halted (token {:#010x}) — endpoint retired, not forced ::",
                    idx, tok
                );
                e.dead = true;
                // EHCI-KEYUP F2: the endpoint is retired for the rest of the boot — this loop
                // `continue`s past a `dead` entry forever after — so any key down at this instant
                // would NEVER receive its release. Flush them. See `flush_held_releases` for why
                // this is the one asymmetric case the poll-gap argument does not cover, and why
                // ring 3 cannot recover from it on its own.
                flush_held_releases(e, idx);
                continue;
            }
            // KBDWIT-2: a clean retirement — the endpoint answered. THE decision-table entry.
            #[cfg(feature = "kbdwit")]
            kbdwit_note_silence_end(e, idx, "SILENCE-BROKE", tok);
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
                    // EHCI-KEYUP: the decoder now carries the previous report's keycodes so it can
                    // emit release edges. `report` is built from `e.buf` through `from_raw_parts`, a
                    // raw pointer with no borrow of `e`, so handing the decoder `&mut e.kbd_prev_keys`
                    // alongside it is not an aliasing violation — the buffer and the diff state are
                    // disjoint memory.
                    // ALLKEYS P1: the decoder also owns the lock-key state now. It reports back
                    // whether this report toggled one; the SET_REPORT that lights the key is queued
                    // for after the loop, where `self` is borrowable again.
                    if decode_boot_keyboard(
                        report,
                        &mut e.kbd_prev_keys,
                        &mut e.kbd_prev_mods,
                        &mut e.kbd_leds,
                    ) && e.kbd_led_ok
                        && n_led < led_pushes.len()
                    {
                        led_pushes[n_led] = Some((ep_i, e.kbd_target, e.kbd_intf, e.kbd_leds));
                        n_led += 1;
                    }
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
        // ALLKEYS P1: the endpoint borrow is released here, so EP0 is usable again — light (or
        // extinguish) the lock LEDs this pass toggled. Best-effort by construction: `set_hid_leds`
        // swallows a refusal, because a keyboard that will not take an Output report must not cost
        // the input path anything, and the software state has already been updated either way (the
        // CASE fold works even on a device with no LED at all).
        for slot in led_pushes.iter().take(n_led) {
            if let Some((ep_i, t, intf, leds)) = *slot {
                if !self.set_hid_leds(&t, intf, leds) {
                    // Refused: latch this keyboard's LED off so a halted EP0 can never cost a
                    // `hw_wait_budget()` stall on a later press. The case fold is untouched.
                    self.int_eps[ep_i].kbd_led_ok = false;
                }
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
// ── KBDWIT-2 (2026-08-06): THE PREDICTED FALSE ALARM ARRIVED, AND THE SECOND SAMPLE IS NOW BUILT ─
//
// The paragraph above predicted its own misreading, and the misreading duly happened. GR16 read
// `rmbp-gr16-s73` boot 9 —
//
//   [ 11125ms] :: KBDWIT: [1] ep=IN3 addr=6 kind=kbd NO-COMPLETIONS class=never-completed
//              quiet=9017ms armed_ms=1740 last_ms=0 now_ms=10757 reports=0 toggle=0 dead=0
//
// — as an s58 recurrence on the wire. It is not. In THAT SAME BOOT, on THAT SAME ARMING, with no
// re-arm and no STOP-NOTE anywhere between them, the same endpoint delivered:
//
//   [215418ms] EHCI-HID: KEY: 'l' (scancode 0xf)
//   [276587ms] :: EHCI-HID: [1] kbd 96 reports, last 00 00 .. == witness ::
//
// Across the whole capture corpus that carries this witness (gr13, s62-probe, s66-cand444,
// gr15-s70, gr16-s73 — 23 boots) EVERY kbd dump reads `reports=0`, and ten of those boots later
// typed fine. The deadline is 4.7-26 s into a boot; nobody types that fast. The instrument was
// measuring the absence of a typist and printing it in the grammar of a fault.
//
// So the fix is the instrument, not the driver — nothing in the EHCI path is convicted by any of
// this, and nothing in it is changed. Two additions, both read-only:
//
//   1. `sched=WALKED|NOT-WALKED` with `polls=`/`walks=`/`split_or=` on line 1. The
//      controller writes C-prog-mask and FrameTag/S-bytes into the QH overlay every time it
//      executes a split against this endpoint; sampling that pair on every service pass and
//      counting the changes answers "is the host side doing its job?" WITHOUT a keypress. It
//      decides the half of the question that was always decidable and was never asked. See
//      `IntEp::kbdwit_walks` for why aliasing cannot fake a zero.
//   2. `SILENCE-BROKE` / `SILENCE-ENDED-HALTED`, the keypress-triggered second sample this comment
//      said it did not build. One latched line at the first non-ACTIVE token after a dump, carrying
//      the elapsed silence and the poll/walk rate that spanned it.
//
// WHICH NON-ACTIVE CLASSES LATCH THAT SECOND LINE, because "the qTD is no longer Active" is not one
// event but two, and they mean opposite things:
//   * A CLEAN retirement (`tok & QTD_ERR_MASK == 0`) prints `SILENCE-BROKE`. The endpoint answered.
//   * A HALT (transaction error, babble, data-buffer error — the classes `QTD_ERR_MASK` covers)
//     prints `SILENCE-ENDED-HALTED`, from inside the same block that emits `STOP-NOTE` and retires
//     the endpoint. The silence ended, but it ended in a fault, and that is NOT the healthy row of
//     the table below.
// They share one latch and sit on opposite sides of the error test, so exactly one of them can ever
// print for a given endpoint. The split is not cosmetic: emitted from a single site above that test
// — as the first cut of this was — a halt would have printed `SILENCE-BROKE` and read as "the
// device answered", inverting the table on exactly the boot this exists to adjudicate. See
// `kbdwit_note_silence_end` for why `reports=` could not have disambiguated it either.
//
// TWO READINGS OF THAT DUMP THAT LOOK LIKE FINDINGS AND ARE NOT. Both were put to this arc from
// the R2 boot ([11156ms], the same capture, one boot later), so they are written down here rather
// than re-litigated:
//
//   * `qtd_tok=0x00000000 qtd_driven=0` is NOT "the overlay never wrote back to the linked qTD",
//     and it does not implicate the qTD list pointer or the horizontal linkage. `qtd_driven=0` is
//     this instrument saying the standalone qTD IS NOT IN THE TRANSFER AT ALL: on the
//     overlay-direct path — which is what `path=overlay-direct` on the line above declares, and
//     what this metal settles into after the qTD-fetch HSE — `arm_interrupt_ep` writes the QH
//     overlay in place and the controller is never handed a qTD address. `ovl0=0x00000001` is
//     PTR_TERMINATE, set deliberately, and `cur=0x00000000` follows from it. The zero word is
//     untouched pool memory; there is no write-back owed and none missing. This is exactly the
//     reading `qtd_driven=` was added to prevent, and it still caught a reader, which is the
//     argument for the flag rather than against it.
//   * `horiz=0x00000001` on IN3 is not a broken chain either. IN3 armed first and took the old
//     frame-list head (T-bit) as its `horiz`; IN1 armed second, prepended, and its
//     `horiz=0x7b479242` points AT the IN3 queue head. `fl0=0x7b479302` is IN1. The chain is
//     fl0 -> IN1 -> IN3 -> terminate, entire and in one direction.
//
// AND THE POSITIVE EVIDENCE THAT SETTLES IT WITHOUT A NEW BOOT. `ovl4=0x00000004 ovl5=0x00000017`
// in boot 9, `ovl4=0x00000004 ovl5=0x00000018` in R2 — the SAME QH position, a different FrameTag.
// Nothing in this driver writes `overlay[5]` on any path and the pool is zeroed, so those bits are
// the host controller's own, written while it executed a start-split against this endpoint. Both
// boots therefore already say WALKED; `sched=` only makes the driver say it out loud instead of
// leaving it to a reader with the EHCI spec open. And R2 corroborates the stimulus reading from the
// other side: it ran to 159290 ms with zero `EHCI-HID: KEY` lines. Nobody typed, and the endpoint
// reported nothing. There is no third thing that needs explaining.
//
// WHAT EACH OUTCOME MEANS, on the next attended boot:
//   * dump `sched=WALKED reports=0`, then `SILENCE-BROKE` on the first key — healthy. This is the
//     baseline, and it is what every boot in the corpus above would have printed.
//   * dump `sched=WALKED reports=0`, keys pressed, NO `SILENCE-BROKE` — the s58 recurrence, with
//     the host side positively excluded: the controller is transacting and the device is not
//     answering (or its answer is being discarded). Device-side, TT, or toggle. That is a real
//     conviction and it is new.
//   * dump `sched=NOT-WALKED` — the controller never reached the QH. Host-side, convicted on the
//     spot, no keypress needed. Read line 5's `fl0`/`horiz` and line 6's `pse`/`pss` next.
//   * `SILENCE-BROKE` with a large `quiet_ms` and no keypress at that instant — the endpoint
//     completed something unprompted (a keep-alive, a resumed stream); the silence was never a
//     fault at all.
//   * `SILENCE-ENDED-HALTED` — the silence ended in a fault, not an answer. Read `tok=` and the
//     decoded `halted=`/`xact=`/`babble=`/`dbuf=` beside it, and the `STOP-NOTE` on the next line;
//     `walks=` then says whether the controller had been transacting right up to the halt. This
//     row does NOT belong to the healthy case above and must never be counted as one.
//
// WHY PER-ENDPOINT, NOT PER-CONTROLLER. During the failure the trackpad is streaming on the SAME
// controller, so any controller-level "is anything completing?" test reads HEALTHY on the exact
// boot that motivated this instrument. The silence clock lives on `IntEp` and is stamped only by
// that endpoint's own completions.
//
// BOUNDS. `kbdwit_fired` latches on the first dump: at most one dump per endpoint per boot, at
// most `MAX_INT_EPS` (6) per controller for a whole boot. `kbdwit_broke` latches the same way, so
// KBDWIT-2 adds at most one further line per endpoint per boot — eight lines, once, per endpoint,
// for the entire boot. No loop, no retry, no wait, no allocation, no register write — every access
// below is a read. Cost on the service path is one bool test plus one `ms()` read before the
// deadline, and one bool test after it fires; KBDWIT-2's sampler adds two volatile reads of
// already-mapped DRAM plus a compare per endpoint per pass, on the ~1 kHz poll, and its counters
// saturate rather than wrap. Note the
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

/// KBDWIT-2 — the silence ended. One latched line, per endpoint, per boot, for the FIRST
/// non-ACTIVE token seen after the deadline dump declared this endpoint silent.
///
/// ### Why the caller splits this into two verdict words instead of one
///
/// The first cut of this line was emitted from a single site above the service loop's
/// `QTD_ERR_MASK` test, i.e. on ANY non-ACTIVE token. That is wrong in the one direction an
/// instrument must never be wrong. A qTD halted by a transaction error, babble or a stall is also
/// non-ACTIVE — the very next block retires the endpoint as a fault and prints `STOP-NOTE` — so a
/// halt would have latched `SILENCE-BROKE` and read as *"the device answered, all is well"*,
/// inverting the decision table in the section comment above on precisely the boot the line was
/// built to adjudicate. And `reports=` could not have rescued the reader: on this metal the genuine
/// keypress case ALSO prints zero there, because `reports` is incremented further down, past the
/// length gate.
///
/// So the caller places the two exits on opposite sides of the error test and names them apart:
///
///   * `SILENCE-BROKE` — a CLEAN retirement, and the only class the decision table's "healthy"
///     row covers. Emitted below the error test.
///   * `SILENCE-ENDED-HALTED` — the token carried `QTD_ERR_MASK`. Emitted inside the error block,
///     alongside `STOP-NOTE`, which it deliberately duplicates the token of: `STOP-NOTE` says the
///     endpoint died and this says how long it had been quiet, over how many polls, with what walk
///     rate — the diagnostic payload that would have been lost by simply moving the latch and
///     letting the halt print nothing.
///
/// They share the one `kbdwit_broke` latch, so they are mutually exclusive and the total is still
/// at most one line per endpoint per boot. An `awk '/SILENCE-/'` finds both; neither can be
/// mistaken for the other.
///
/// `reports_prior=` is named for what it is: the count BEFORE this completion, which is not yet
/// (and may never be) incremented — a zero-length completion still retires the qTD and still ends
/// the silence, but never bumps `reports`. `tok=` is raw and the status bits are decoded beside it,
/// so a reader re-derives the class from the hex without trusting the verdict word.
#[cfg(feature = "kbdwit")]
unsafe fn kbdwit_note_silence_end(e: &mut IntEp, idx: usize, verdict: &str, tok: u32) {
    if !e.kbdwit_fired || e.kbdwit_broke {
        return;
    }
    e.kbdwit_broke = true;
    let now = crate::arch::ms();
    let chars = core::ptr::read_volatile(&(*e.qh).ep_chars);
    serial_println!(
        ":: KBDWIT: [{}] ep=IN{} addr={} {} tok={:#010x} halted={} xact={} babble={} dbuf={} rem={} armed_ms={} now_ms={} quiet_ms={} polls={} walks={} split_or={:#018x} reports_prior={} toggle={} == witness ::",
        idx,
        (chars >> 8) & 0xF,
        chars & 0x7F,
        verdict,
        tok,
        (tok >> 6) & 1,
        (tok >> 3) & 1,
        (tok >> 4) & 1,
        (tok >> 5) & 1,
        (tok >> 16) & 0x7FFF,
        e.kbdwit_armed_ms,
        now,
        now.wrapping_sub(e.kbdwit_armed_ms),
        e.kbdwit_polls,
        e.kbdwit_walks,
        e.kbdwit_split_or,
        e.reports,
        e.toggle as u8,
    );
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
    //
    // KBDWIT-2 adds `sched=`, and it is the field that makes this line answer something. `class=`
    // reports what the DEVICE has delivered, which for a boot keyboard nobody has typed on is
    // "nothing" on a perfectly healthy rig — the ambiguity the section comment above admits it
    // cannot resolve. `sched=` reports what the CONTROLLER has been doing to this queue head, from
    // words only the controller writes (`IntEp::kbdwit_walks`), and that half is decidable here:
    //
    //   sched=WALKED     the controller reached this QH and transacted against it on `walks` of
    //                    `polls` passes. With `reports=0` this reads "polled and NAKed" — the host
    //                    side is working and the silence is the device's or the operator's. It is
    //                    the acquittal the deadline could never previously give.
    //   sched=NOT-WALKED thousands of polls and the controller never touched the QH's split
    //                    progress. A host-side fault, convicted without anyone pressing a key.
    //
    // There is deliberately NO third arm for "not sampled yet". The obvious safety valve — a
    // `polls == 0` case, so a missing measurement could never masquerade as a conviction — was
    // written, and a `strings` pass over the built rlib showed the compiler had deleted it: the
    // sampler runs on the SAME service pass, immediately above the call to this probe, so
    // `polls >= 1` holds by construction at every reachable entry (and `saturating_add` means it
    // can never return to zero). A branch that cannot print is a branch a reader will one day trust
    // as coverage, so it is gone rather than left as decoration. `polls=` is on the line regardless,
    // which is what actually guards against reading a small sample as a verdict.
    serial_println!(
        ":: KBDWIT: [{}] ep=IN{} addr={} kind={} NO-COMPLETIONS class={} sched={} polls={} walks={} split_or={:#018x} quiet={}ms armed_ms={} last_ms={} now_ms={} reports={} toggle={} dead={} == witness ::",
        idx, epn, addr, kind,
        if e.kbdwit_last_ms == 0 { "never-completed" } else { "went-quiet" },
        if e.kbdwit_walks > 0 { "WALKED" } else { "NOT-WALKED" },
        e.kbdwit_polls, e.kbdwit_walks, e.kbdwit_split_or,
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
///
/// EHCI-KEYUP — THIS PATH NOW SYNTHESISES RELEASES, WHICH IS THE HALF IT WAS MISSING.
///
/// Boot AJ (`aj-lockout-forensics.md`, Defect 2a) is what a decoder that emits presses only costs:
/// the rMBP's INTERNAL keyboard is on EHCI, so on that machine no ring-3 app had ever received a
/// single `INPUT_EV_KEY_UP`. `user-vug` clears a held bit only on a release, so the first SPACE
/// latched pause on permanently and the arrow/WASD held bits latched with it — the operator's
/// "the vug froze". The xHCI decoder has always synthesised releases; this is the same logic, and
/// it is deliberately a MIRROR rather than a variation.
///
/// WHAT "MIRROR" MEANS HERE, SCOPED — because the unqualified claim is not true and the difference
/// is one a bench reading could trip over. The mirror is over the EDGE MODEL: which keycodes produce
/// a release, when, and in what order. That half is byte-equivalent to xHCI's — same set diff, same
/// `<= 1` guard, same position-independent `contains` test, same "no `KeyUp` for a modifier", same
/// shift-at-release-time. It is NOT a mirror over ASCII MAPPING: xHCI computes
/// `eff_shift = shift ^ (caps & is_letter)` from the device's `keyboard_leds`, and this decoder has
/// no caps state at all and uses bare `shift`. So under Caps Lock a LETTER's ascii differs between
/// the two controllers — on the press as much as on the release. That is PRE-EXISTING (the press
/// loop was already caps-blind before this arc), it is internally consistent here because both loops
/// share `ascii_of` so a press and its release can never disagree about which character they are
/// about, and it is being closed by the `seat/gr21-allkeys` arc stacked on this one. It is named
/// rather than fixed here on purpose: this commit's contract is the edge model, and widening it to
/// the mapping would put two independent changes in one bisect.
///
/// THE DIFF. A USB boot report is a LEVEL, not an edge: bytes 2..8 carry the FULL set of keycodes
/// currently down (6-key rollover), so any keycode present in the previous report and absent from
/// this one was released. `prev_keys` carries that previous set per endpoint and is rewritten here.
/// The press loop reads the SAME diff in the other direction (see DBLSTROKE below): a keycode absent
/// from the previous report and present in this one is a press EDGE, and only an edge pushes `Key`.
///
/// WHY A MISSED RELEASE CANNOT STRAND A KEY HERE. `service_ehci_hid` arms one transfer per service
/// pass and is polled at frame rate, so reports the device sends between passes are simply never
/// fetched — the CLICK-3 note above is about exactly that. It cannot cost a release: USB is
/// host-polled, so a device with a pending state change holds it until the next IN token rather than
/// losing it, and the re-arm happens in the SAME service pass as this decode. All four poll-gap
/// cases are symmetric — press+release both inside a gap loses both edges; press seen then release
/// seen pairs; release+re-press inside a gap collapses to a hold with no fabricated edge. (This is
/// the property the pointer path lacks, because a button's level can go down again before the next
/// poll.) **The one ASYMMETRIC case is not a poll gap at all: it is endpoint DEATH, and it is closed
/// separately by [`flush_held_releases`].**
///
/// SHORT REPORTS ARE REFUSED WHOLE, and the `< 8` is load-bearing rather than defensive dressing.
/// This guard was `< 3` and `cur_keys` zero-fills the slots a short report does not carry, so a full
/// report followed by a SHORT one emitted a `KeyUp` for everything held in the missing slots — a
/// FABRICATED release, the exact class this arc exists to remove, arriving by a different door (a
/// fabricated SPACE release clears `H_PAUSE`, the next full report re-presses it as a fresh edge,
/// and pause toggles on its own). xHCI cannot do this: it reads a fixed 8-byte staging buffer
/// UNCONDITIONALLY (`from_raw_parts(data_buf_ptr, 8)`), so a short transfer leaves the previous
/// bytes in place and reads as STILL HELD — the conservative direction. Refusing the report whole
/// takes that same direction with no staging buffer: no press, no release, and `prev_keys` is left
/// untouched, so nothing is invented and the next conforming report resolves the truth. Zero
/// behavioural cost on any conforming device — the boot protocol is fixed at 8 bytes and this
/// driver arms these endpoints at `mps=8`.
///
/// MODIFIERS GET NO KeyUp, matching xHCI exactly. `report[0]` is a bitmask, not a keycode; neither
/// decoder has ever pushed `Key` for a bare modifier, so pushing `KeyUp` for one would fabricate a
/// release edge for a press ring 3 never saw. The ascii of a release is picked by
/// `xhci::hid_key_release_ascii`, which folds on Shift and Caps only, as on xHCI. The invariant is
/// exact: **a release resolves to the same ascii its press did** — Shift+`1` that pressed `!`
/// releases `!`. The earlier gloss "consumers match case-insensitively" was true for letters and
/// FALSE for shifted symbols (`!` vs `1`), which is the shifted-symbol strand GR21 closed.
///
/// ── KEYREPEAT-X86 (Boot AL): THIS DECODER NOW FEEDS THE HOST TYPEMATIC TRACKER ────────────────
///
/// The paragraph that stood here said "no typematic interaction exists on this path", called it a
/// fact about the build rather than a judgement call, and closed with *"repeat on this arch is the
/// DEVICE's, carried by the re-reported level."* The build fact was true; the closing sentence was
/// a hypothesis, and Boot AL refuted it — Peter, at the bench: *"so far so good with keys except no
/// key repeat."* Everything else GR21 landed here passes on metal. Repeat does not, because the
/// rMBP's internal keyboard does NOT re-report a key that is held still: no SET_IDLE is sent on
/// this path (see KBDWIT), so the device runs its default report-on-change behaviour and the
/// "re-reported level" the KeyDown loop below relies on for repeat never arrives. One press report,
/// one `Event::Key`, and the shell's line editor advances exactly one character — which is the same
/// symptom, from the same cause, that UVUG-5 was written for on aarch64.
///
/// So the tracker's cfg was widened to cover `x86_64 + ehcihid` (see `pal.rs` §KEYREPEAT-X86) and
/// this decoder feeds it at the REPORT level, exactly as `drivers::xhci` does: the newest ascii
/// pressed THIS report plus the FULL currently-held ascii set, computed through the same
/// `ascii_of` fold the `Event::Key` pushes use, so the armed key and the pushed character are the
/// same byte and a release can never disagree with its press about identity.
///
/// WHY THE FEED IS HERE AND NOT AT THE EVENT QUEUE. That is the whole of UVUG-6: `EventQueue::push`
/// silently DROPS on a full 64-slot ring, so a release learned from the drained event stream can be
/// lost and the tracker would then repeat a key nobody is holding, forever (the P51 wedge). At the
/// report level a release is learned from the armed key being ABSENT from the held set — a fact the
/// queue cannot drop. The three disarm layers and the half-full backpressure guard come with the
/// shared code, already metal-cured on the Pi.
///
/// THE KeyDown/KeyUp EDGES BELOW ARE UNTOUCHED, deliberately: GR21's release synthesis is
/// metal-proven as of Boot AL (SPACE pauses and unpauses, WASD releases, `!` releases `!`,
/// modifiers get no `KeyUp`) and nothing in this arc may perturb it. The tracker is a pure OBSERVER
/// here — it pushes no event from this function; `main`'s x86 pump calls `pal::typematic_tick`.
///
/// ── DBLSTROKE (Boot AN): THE PRESS LOOP WAS LEVEL-TRIGGERED, AND ROLLOVER DOUBLED EVERY KEY ──────
///
/// Peter, at the bench on Boot AN: *"key repeat good but typing fast causes double stroke."* Held-key
/// repeat and normal-speed typing are both correct; typing FAST doubles characters. The capture names
/// the mechanism outright, with no new instrument needed — `EHCI-HID: KEY:` is pushed once per
/// `Event::Key`, and the word "help" typed quickly reads:
///
/// ```text
/// [1228135ms] KEY: 'h'                      report [h]     — press edge
/// [1228275ms] KEY: 'h'   KEY: 'e'           report [h,e]   — 'h' RESTATED, 'e' pressed
/// [1228413ms] KEY: 'e'   KEYUP: 'h'         report [e]     — 'e' RESTATED, 'h' released
/// [1228427ms] KEY: 'l'                      report [l]     — press edge
/// ```
///
/// Two `Event::Key` pushes for one physical press of `h`, and two for `e`. The cause is that the press
/// loop was a LEVEL loop: it pushed `Key(ascii)` for every keycode in every report, and a USB boot
/// report re-states the FULL held set. Fast typing is defined by OVERLAP — the next key goes down
/// before the last one comes up — so every overlapped pair produces a report that re-states the key
/// already down, and that re-statement was delivered as a second press. Type slowly enough that each
/// key is fully released first and every report carries exactly one key, and nothing ever repeats:
/// precisely the reported symptom, precisely bounded.
///
/// It is the PRODUCER, not the console or the line editor: the two pushes are two distinct lines from
/// this function, at two different report timestamps. It is not the typematic tracker either — the
/// doubles are 140 ms apart with no 400 ms delay elapsed, `[keystat] typematic hold end` shows
/// `re-arms=0` for the whole boot, and the tracker pushes nothing from here in any case.
///
/// THE FIX is to push on the press EDGE, which is the contract every consumer in the tree was already
/// written against — `vug.rs` GAME-MODE says so in as many words ("the HID path delivers a Key on the
/// PRESS edge and a KeyUp on the RELEASE edge"), and this decoder's own release loop has always been
/// an edge. The two loops now read the same `prev_keys`/`cur_keys` diff in opposite directions, so a
/// press and its release are the same fact seen twice and cannot disagree about how many there were.
///
/// WHAT PAYS FOR THE REPEAT THE LEVEL LOOP USED TO PROVIDE: the host typematic tracker, which this
/// same function already feeds and which is the ONLY source of repeat that ever reached this machine.
/// The old note here argued the level loop delivered repeat "on a device that does re-report" and so
/// should stay. On this hardware there is no such device — no `SET_IDLE` is sent on this path (KBDWIT)
/// and the internal keyboard runs report-on-change — which is why KEYREPEAT-X86 had to add the tracker
/// at all. Level-driven repeat was therefore never repeat here; it was only ever the doubling, paced
/// by the operator's other fingers rather than by any repeat rate. And a hypothetical idle-re-reporting
/// keyboard is better served by the tracker's 400 ms/40 ms than by an unthrottled poll-rate spew.
///
/// WITNESS (`[keystat] ehci press`). Two bounded counters that decide the NEXT boot either way:
/// `restated=` counts the pushes this edge gate suppressed (non-zero proves the operator typed with
/// overlap and that the old code WOULD have doubled), and `dbl=` is an independent doubled-push
/// detector that watches what this function actually pushes — the same ascii twice inside
/// `DOUBLE_WINDOW_MS`. If Peter still sees doubles with `dbl=0`, the producer is clean and the fault
/// is downstream (console echo or line editor); if `dbl>0` the producer is still doubling and this
/// diagnosis was wrong. Either way the boot is decisive, which the previous boot's instruments were
/// not.
///
/// ── ALLKEYS P1 (GR21): CAPS LOCK, THE HALF THIS DECODER WAS STILL MISSING ──────────────────────
///
/// Peter, at the bench: *"do we have working shift, caps lock etc — we want all the keys working."*
/// Shift worked here; Caps Lock did not, and on THIS machine that is the whole story, because the
/// rMBP's internal keyboard is an EHCI device — so the decoder that had never heard of caps lock
/// was the only one the operator could reach. The xHCI decoder has read a caps-lock bit since
/// HID-LED; this path did not, so the key was inert and the LED never lit.
///
/// BOTH HALVES OR NEITHER, and that is not a stylistic preference. Case logic without an LED gives
/// an operator a keyboard that types capitals with an unlit key — indistinguishable from a stuck
/// Shift. An LED without case logic gives a lit key that types lowercase. Either half alone reads
/// as a BROKEN keyboard, so this decoder toggles the state and the caller lights the key, both
/// driven from the single `kbd_leds` byte that the case fold and SET_REPORT each read.
///
/// The decoder does not itself send SET_REPORT — it cannot. It is called from inside
/// `Controller::service`'s `for e in self.int_eps.iter_mut()`, which holds an exclusive borrow of
/// `int_eps`, while a control transfer needs `&mut self`. So it RETURNS whether the bitmap changed
/// and the caller issues the request after the loop drops that borrow — the same deferral shape the
/// `mtraw` capture-window restore already uses at that site.
///
/// MODIFIER POLICY is `xhci::hid_key_ascii`'s, not this function's, and deliberately so: Ctrl/Alt/
/// GUI folding was absent from BOTH decoders (Cmd-Q typed a bare `q` into the shell line), and
/// fixing it in one place while the other kept a private copy of the fold is exactly how the
/// caps-lock divergence arose. The whole decision now lives in one function both controllers call.
///
/// PER-RELEASE ASCII, and why `prev_mods` exists. A release edge fires exactly once and is never
/// re-sent, so it MUST resolve to the same ascii FAMILY its press did or it strands the key in every
/// held-state consumer (Boot AJ). The live release path has the current `report[0]` in hand; the
/// dead-endpoint flush ([`flush_held_releases`]) does not — the device is gone — so the modifier
/// byte of the last ACCEPTED report is carried in `prev_mods` for it to fold against. Both paths go
/// through `xhci::hid_key_release_ascii`, which depends only on Shift and Caps: Shift+`1` that
/// pressed `!` releases `!`, not `1`.
///
/// Returns `true` if this report toggled a lock key — i.e. the caller owes the device a SET_REPORT.
// ── DBLSTROKE witness state ──────────────────────────────────────────────────────────────────────
//
// Boot totals only; no per-endpoint state, because the operator has one pair of hands and the question
// ("does this decoder push the same character twice for one press?") is about the decoder, not about
// which endpoint carried it. All of it is dead weight on a boot where nobody types: every counter
// stays 0 and not one line is emitted.
//
// This whole block is x86-only by construction — `drivers/mod.rs` gates `pub mod ehci` on
// `all(target_arch = "x86_64", feature = "ehcihid")`, so aarch64 never compiles a byte of it and no
// per-item `#[cfg]` is needed (nor would one be honest: it would imply the file is reachable without
// the feature).

/// DBLSTROKE — `Event::Key` pushes this decoder made, i.e. genuine press edges.
static PRESS_EDGES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBLSTROKE — keycodes seen down in a report that the PREVIOUS report already carried. Under the old
/// level loop each of these was a second `Event::Key` for a key nobody re-pressed; it is now suppressed
/// and counted. Non-zero is the positive evidence that the operator typed with OVERLAP during the run,
/// which is what makes a `dbl=0` result meaningful rather than merely untested.
static RESTATED_PRESSES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBLSTROKE — reports carrying two or more character keys down at once: rollover, i.e. "typing fast"
/// measured rather than inferred from the operator's description.
static ROLLOVER_REPORTS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBLSTROKE — the independent detector: the same ascii pushed twice by this decoder inside
/// [`DOUBLE_WINDOW_MS`]. Survives the fix on purpose — it watches the OUTPUT, so it stays valid however
/// the input side is rewritten, and it is what distinguishes a producer double from a consumer echo.
static DOUBLE_PUSHES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBLSTROKE — ascii of the last `Event::Key` this decoder pushed, +1 (0 = none yet).
static LAST_PUSH_ASCII: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// DBLSTROKE — `arch::ms()` of that push.
static LAST_PUSH_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// DBLSTROKE — inside this window, the same character twice is a machine artefact, not typing. Human
/// double-letters ("ll" in "help", "ee" in "seen") are two presses with a release between them and are
/// paced by fingers: the Boot AN capture puts a deliberate re-tap of the same key ~450 ms apart, and
/// even the machine doubles it exposed were 24..140 ms. 50 ms sits below anything a hand does and above
/// every duplicate the level loop produced.
const DOUBLE_WINDOW_MS: u64 = 50;
/// DBLSTROKE — how many suppressed restatements name themselves individually before the rollup takes
/// over. Enough to see the shape at the bench; bounded so a stuck endpoint cannot make serial the new
/// backpressure.
const RESTATE_LOG_MAX: u32 = 3;
/// DBLSTROKE — after the individual lines, one rollup per this many suppressions. At a fast typist's
/// overlap rate this is a line every few seconds of continuous typing.
const RESTATE_ROLLUP_EVERY: u32 = 32;
/// DBLSTROKE — individually-named doubled pushes before that detector also falls back to counting.
const DOUBLE_LOG_MAX: u32 = 8;

/// DBLSTROKE — record one genuine press edge and run the doubled-push detector over it.
///
/// Called ONLY from the `push_event(Event::Key(..))` site, so what it measures is what ring 3 actually
/// received. If this ever fires after the edge gate, the edge gate is not the whole story and the next
/// boot says so on its own line rather than leaving the bench to re-describe the symptom.
fn note_press_edge(ascii: u8) {
    use core::sync::atomic::Ordering;
    let now = crate::arch::ms();
    PRESS_EDGES.fetch_add(1, Ordering::Relaxed);
    let prev = LAST_PUSH_ASCII.swap(ascii as u32 + 1, Ordering::Relaxed);
    let prev_ms = LAST_PUSH_MS.swap(now, Ordering::Relaxed);
    if prev == ascii as u32 + 1 && now >= prev_ms && now.wrapping_sub(prev_ms) <= DOUBLE_WINDOW_MS {
        let n = DOUBLE_PUSHES.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= DOUBLE_LOG_MAX {
            serial_println!(
                "[keystat] ehci double-push — ascii={:#04x} pushed twice {}ms apart (<= {}ms); the PRODUCER is doubling, not the console (boot dbl={})",
                ascii,
                now.wrapping_sub(prev_ms),
                DOUBLE_WINDOW_MS,
                n
            );
        }
    }
}

/// DBLSTROKE — fold one report's press accounting and emit the bounded rollup.
///
/// `down` counts character keys down in the report (rollover measure); `restated` counts those the
/// previous report already carried (suppressed doubles).
fn note_press_report(down: u32, restated: u32) {
    use core::sync::atomic::Ordering;
    if down >= 2 {
        ROLLOVER_REPORTS.fetch_add(1, Ordering::Relaxed);
    }
    if restated == 0 {
        return;
    }
    let total = RESTATED_PRESSES.fetch_add(restated, Ordering::Relaxed) + restated;
    let first_few = total <= RESTATE_LOG_MAX;
    if first_few || total % RESTATE_ROLLUP_EVERY < restated {
        serial_println!(
            "[keystat] ehci press — edges={} restated={} (+{} this report) rollover_reports={} dbl={} window={}ms",
            PRESS_EDGES.load(Ordering::Relaxed),
            total,
            restated,
            ROLLOVER_REPORTS.load(Ordering::Relaxed),
            DOUBLE_PUSHES.load(Ordering::Relaxed),
            DOUBLE_WINDOW_MS
        );
    }
}

unsafe fn decode_boot_keyboard(
    report: &[u8],
    prev_keys: &mut [u8; 6],
    prev_mods: &mut u8,
    leds: &mut u8,
) -> bool {
    if report.len() < 8 {
        return false; // short/partial boot report — refused whole (keyup F1); see SHORT REPORTS above
    }
    let modifiers = report[0];
    // ALLKEYS P1: the live caps-lock bit feeds the case fold, so the lit key and the typed case
    // are the same fact read twice rather than two states that can drift apart.
    let caps = *leds & 0x02 != 0;
    // The ascii for one keycode under the modifier state in force. One place, so a press and its
    // release can never disagree about which character they are about.
    let ascii_of = |keycode: u8| -> u8 { super::xhci::hid_key_ascii(keycode, modifiers, caps) };
    // ALLKEYS: and the same for a RELEASE, which may never resolve to "no event" for a key that has
    // a character identity — see `hid_key_release_ascii`. A release edge fires once and is never
    // re-sent, so suppressing one strands the key in every consumer that tracks held state, which is
    // the precise failure Boot AJ cost this path.
    let release_ascii_of =
        |keycode: u8| -> u8 { super::xhci::hid_key_release_ascii(keycode, modifiers, caps) };
    // This report's six keycode slots. The `< 8` guard above is what makes this a COMPLETE picture of
    // what is down — every slot comes from the wire, none is invented — which is the precondition the
    // release diff below needs to be sound. `report[2 + i]` cannot panic for the same reason.
    let mut cur_keys = [0u8; 6];
    for (i, slot) in cur_keys.iter_mut().enumerate() {
        *slot = report[2 + i];
    }

    // DBLSTROKE: PRESS EDGES, NOT LEVELS. `prev_keys` is still the PREVIOUS report here (it is
    // rewritten at the end of this function), so `!prev_keys.contains(&keycode)` is exactly "this
    // keycode went down in THIS report". A keycode the previous report already carried is a RESTATED
    // level, not a new press, and pushing `Event::Key` for it is what doubled characters on metal.
    let mut restated_this_report = 0u32;
    let mut down_this_report = 0u32;
    for &keycode in cur_keys.iter() {
        if keycode <= 1 {
            continue; // no key / ErrorRollOver
        }
        let ascii = ascii_of(keycode);
        if ascii == 0 {
            continue;
        }
        down_this_report += 1;
        if prev_keys.contains(&keycode) {
            // Still held from the previous report. Repeat for a held key is the host typematic
            // tracker's job on this path (KEYREPEAT-X86, fed below) — at 400 ms / 40 ms, once — and
            // never the report level's, which is paced by the operator's OTHER fingers.
            restated_this_report += 1;
            continue;
        }
        serial_println!("EHCI-HID: KEY: '{}' (scancode {:#x})", ascii as char, keycode);
        crate::pal::push_event(crate::pal::Event::Key(ascii));
        note_press_edge(ascii);
    }
    note_press_report(down_this_report, restated_this_report);

    // KEYREPEAT-X86: feed the host-side typematic tracker at the REPORT LEVEL — BEFORE the release
    // edges below and before anything else this report will push, so a `KeyUp` the 64-slot ring may
    // later DROP can never strand a held key (UVUG-6's root cause). `newest_press` is the ascii of a
    // keycode that is down NOW and was NOT down in the previous report; `held` is every ascii down
    // now. Both resolve through `ascii_of`, the same fold the `Event::Key` pushes above used, so the
    // tracker arms the exact byte the consumers received. Non-ascii usages (F-keys, the lock keys)
    // are absent from both, exactly as on the xHCI feed — `IDLE_RUN_TO_LATCH` covers the residue.
    // Mirrors `drivers::xhci`'s call site one-for-one; the tracker itself is shared code.
    {
        let mut held: [u8; 6] = [0; 6];
        let mut hn = 0usize;
        let mut newest_press: u8 = 0;
        for &keycode in cur_keys.iter() {
            if keycode <= 1 {
                continue;
            }
            let ascii = ascii_of(keycode);
            if ascii != 0 {
                held[hn] = ascii;
                hn += 1;
                if !prev_keys.contains(&keycode) {
                    newest_press = ascii;
                }
            }
        }
        crate::pal::typematic_note_report(newest_press, &held[..hn]);
    }

    // The release edges. Bounded by six per report, and a human's key releases are human-rate, so
    // the serial line is unconditional like its `KEY:` twin rather than hidden behind `usbdebug` —
    // it is the wire evidence that this path emits releases at all, which is the one thing Boot AJ
    // could not show.
    for &keycode in prev_keys.iter() {
        if keycode <= 1 {
            continue;
        }
        if cur_keys.contains(&keycode) {
            continue; // still held
        }
        let ascii = release_ascii_of(keycode);
        if ascii != 0 {
            serial_println!("EHCI-HID: KEYUP: '{}' (scancode {:#x})", ascii as char, keycode);
            crate::pal::push_event(crate::pal::Event::KeyUp(ascii));
        }
    }

    // ALLKEYS P1: lock-key PRESS edges — a lock usage present now and absent last report. Edge and
    // not level, because a boot report re-states the full held set: a caps key held down for half a
    // second is re-reported every poll, and a level test would toggle the state on every one of
    // those reports, flipping caps tens of times per press and landing on whichever parity the
    // release happened to fall on. The edge fires exactly once per physical press.
    //
    // Note these usages produce NO `Key`/`KeyUp` event and never did — `HID_SCANCODE_TO_ASCII` maps
    // all three to (0,0), so the loops above skip them. A lock key changes the MEANING of later
    // keys; it is not itself a character. Mirrors the xHCI toggle loop, sharing its (usage, bit) table.
    let mut changed = false;
    for &(usage, bit) in super::xhci::HID_LOCK_KEYS.iter() {
        if cur_keys.contains(&usage) && !prev_keys.contains(&usage) {
            *leds ^= bit;
            changed = true;
        }
    }

    *prev_keys = cur_keys;
    // ALLKEYS: remember this accepted report's modifier byte, so a later endpoint-death flush can
    // resolve each stranded key to the same shifted ascii its press produced. Updated ONLY here —
    // past the `< 8` guard — so a refused short report never overwrites the last true modifier state.
    *prev_mods = modifiers;
    changed
}

/// EHCI-KEYUP F2 — release every key this endpoint still believes is DOWN, because no further report
/// will ever arrive to say otherwise.
///
/// THE ONE ASYMMETRIC LOSS. [`decode_boot_keyboard`]'s poll-gap argument is sound and covers every
/// case where reports keep flowing: a release the driver did not fetch is re-read from the next
/// report's level. It says nothing about the endpoint being RETIRED. `service_ehci_hid` sets
/// `e.dead = true` on `tok & QTD_ERR_MASK` (the `STOP-NOTE interrupt endpoint halted` line) and
/// thereafter skips that entry for the rest of the boot — there is no next report, so a key held at
/// that instant is stranded in ring 3 forever.
///
/// AND RING 3 CANNOT SAVE ITSELF FROM IT. `user-vug`'s `H_SAW_KEYUP` belt is deliberately ONE-WAY:
/// once any release has been seen the pause-retire stops firing for the life of the process. So a
/// mid-hold endpoint death puts the operator straight back in Boot AJ — `H_PAUSE` latched, the
/// crystal frozen, kill the app — with the belt disarmed by its own correct behaviour.
///
/// This is xHCI's countermeasure ported to the shape EHCI has. There, `Slot::reset_soft_state` calls
/// `pal::note_keyboard_detached()` under the note that "under `SET_IDLE(0)` that key's `KeyUp` will
/// NEVER arrive", feeding the aarch64 typematic tracker's detach layer. That tracker does not exist
/// on x86 (see the typematic note on the decoder), so the same fact is answered where x86 CAN answer
/// it: emit the releases the device now never will, through the ordinary event path, so every ring-3
/// consumer sees an honest edge rather than an absence it has no way to notice.
///
/// Idempotent by construction — `kbd_prev_keys` is zeroed, so a second call flushes nothing — and a
/// no-op on a pointer endpoint, whose `kbd_prev_keys` no decoder ever writes.
///
/// ALLKEYS: each stranded key resolves through the SAME fold its press used — `hid_key_release_ascii`
/// against the last accepted report's modifier byte (`kbd_prev_mods`) and caps state (`kbd_leds`).
/// The earlier code here used the raw UNSHIFTED byte and reasoned it away with "consumers match
/// case-insensitively" — true for letters, FALSE for shifted symbols: a Shift+`1` that pressed `!`
/// would have flushed `KeyUp('1')`, which no consumer holding a bit for `!` can match, stranding the
/// very key this function exists to release. The invariant is simply: a release resolves to the same
/// ascii its press did. `hid_key_release_ascii` depends only on Shift and Caps, so a suppressing
/// modifier held at the instant of death still yields the shift-only byte the press delivered, never 0.
unsafe fn flush_held_releases(e: &mut IntEp, idx: usize) {
    let caps = e.kbd_leds & 0x02 != 0;
    for i in 0..e.kbd_prev_keys.len() {
        let keycode = e.kbd_prev_keys[i];
        if keycode <= 1 {
            continue;
        }
        let ascii = super::xhci::hid_key_release_ascii(keycode, e.kbd_prev_mods, caps);
        if ascii != 0 {
            serial_println!(
                ":: EHCI-HID: [{}] KEYUP-FLUSH: '{}' (scancode {:#x}) — endpoint retired, release synthesised == witness ::",
                idx, ascii as char, keycode
            );
            crate::pal::push_event(crate::pal::Event::KeyUp(ascii));
        }
    }
    e.kbd_prev_keys = [0; 6];
    e.kbd_prev_mods = 0;
    // KEYREPEAT-X86 — layer 2, and the one hole the ring-3 flush above does NOT close now that this
    // path has a host repeat. The tracker is fed only from `decode_boot_keyboard`, and a retired
    // endpoint produces no further report — so a key armed at the instant of death would never see
    // the ABSENT-from-held-set fact that disarms it, and `typematic_tick` would inject a repeat
    // every `RATE_MS` until the coarse `HOLD_MAX_MS` backstop fired 30 s later. The synthesised
    // `KeyUp`s above cannot substitute: they are EVENT-level and the tracker deliberately does not
    // observe the event stream (that was UVUG-5's dropped-`KeyUp` hole).
    //
    // `pal::note_keyboard_detached` is the seam built for exactly this — xHCI's `reset_soft_state`
    // calls it on the same fact — and it is arch-neutral: a plain generation counter the tracker
    // folds on its next tick, dropping the armed key, the parked lapse and the streaming verdict.
    // Called unconditionally rather than only when something was held: this function runs ONCE per
    // endpoint death (it is idempotent by the zeroing above, and `e.dead` makes the caller skip the
    // entry forever after), so a spurious generation bump costs nothing when nothing is armed.
    crate::pal::note_keyboard_detached();
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
    // BUY-1 (GR18): the port walk moved out of this loop into a second pass, so each woken
    // function's handle (op base + port count, the only two things the walk needs from it) has to
    // outlive the scan. Paired 1:1 with `ctrls`, pushed on the same line — index `i` of one is
    // index `i` of the other, which is what lets phase 2 zip them.
    let mut handles: Vec<(EhciFnHandle, u64)> = Vec::new();
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
                    // EPACE-TRIM M1 + BUY-1: the chain-HSE verdict is NOT read here any more.
                    // The probe that SETS the latch lives in the port walk, and BUY-1 moved the
                    // port walk out of this loop — so a verdict read at construction time would be
                    // read before ANY controller had probed, and controller [1] would run its own
                    // 2 s probe plus the wedged-controller re-init the s58 split priced at ~2.6 s.
                    // That is ~17x the whole BUY-1 saving, in the wrong direction.
                    //
                    // So the read moves to the point of USE — immediately before this controller's
                    // own probe, in phase 2 — where it keeps M1's actual property: it is still read
                    // before THIS controller has probed anything, so a same-controller HSE flip can
                    // never be mistaken for an inherited verdict. The "verdict CARRIED" witness
                    // moves with the read, so a capture still distinguishes measured from inherited.
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
                        // EPACE-TRIM M1 + BUY-1: constructed in chain mode; phase 2 sets this
                        // from CHAIN_HSE_SEEN just before the probe it gates (see above).
                        overlay_mode: false,
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
                        // BT-L0: no parent until `bring_up_hub` stamps one; the depth-0 M1
                        // witness never reads it.
                        #[cfg(feature = "bt")]
                        bt_parent: (0, 0),
                        // MTFIX: the dedicated HCI-event slot is free until a radio claims it.
                        #[cfg(feature = "bt")]
                        bt_evt_armed: false,
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
                    // BUY-1 (GR18) — the T_ATTDB clock starts HERE; the spin is paid in phase 2.
                    // The debounce is owed from the port-power / CF edge this bring-up just
                    // applied, so the clock must start at this point and not later. What phase 2
                    // does is pay whatever is LEFT of it once the other controllers' bring-ups (and,
                    // for every controller after the first, the earlier controllers' whole port
                    // walks) have run — never less than zero remaining, and loud when the overlap
                    // covered part of it. See the phase-2 header for the whole argument.
                    let attdb_t0 = crate::arch::now_cycles();
                    ctrls.push(c);
                    handles.push((h, attdb_t0));
                }
            }
        }
    }

    // ── BUY-1 (GR18): phase 2, the port walks ───────────────────────────────────────────────────
    // The enum46 verdict (§5, BUY-1) named the serialization: on the s73 baseline controller [0]
    // finishes enumerating at [977ms] and controller [1]'s bring-up begins on that same
    // millisecond. Everything in this driver is a synchronous busy-spin — `settle_ms` and
    // `wait_bounded` both spin on the TSC — so there is no way to run two controllers' work at the
    // same time without a concurrency framework this kernel does not have at boot, and the verdict
    // was right that restructuring the recursive enumeration into interleavable state machines is
    // an arc, not an edit.
    //
    // What IS available for the price of a loop split is the one wait that is pure dead time and
    // owed to a clock rather than to a device: T_ATTDB. It does not have to be *spun*; it has to
    // have *elapsed*. So the scan above now stops at the point where each controller's debounce
    // clock starts, and this loop pays only the remainder:
    //
    //   before: [0] bring-up, [0] T_ATTDB 100 ms, [0] port walk, [1] bring-up, [1] T_ATTDB 100 ms, …
    //   after:  [0] bring-up, [1] bring-up, [0] T_ATTDB remainder, [0] port walk, [1] T_ATTDB …
    //
    // [0]'s debounce now runs under [1]'s bring-up (~52 ms of hcrst + smoke on the s73 baseline),
    // and [1]'s runs under the whole of [0]'s port walk (~550 ms) — which is exactly where the
    // `05ac:8510`'s device-floor NAK sits. That NAK is untouched and stays untouched (bootpace.md
    // §8h): this buys back the SERIALIZATION around it, not the device's own answer latency, and
    // the ceiling on the buy is therefore the deferred dead time (100 ms per controller after the
    // first, plus whatever of the first controller's own 100 ms the later bring-ups cover), not
    // the NAK. Predicted: `BPACE: ehci-hid-done d=` ~1444 -> ~1290 ms, `EPACE: [0] rootrst=`
    // 320 -> ~270 ms and `[1] rootrst=` 160 -> ~60 ms, with `[0] enum=`, `{act=}` and the M8
    // `wlen=18` line all unchanged. On a single-controller machine (QEMU: one ich9-usb-ehci1)
    // nothing at all changes — there is no earlier controller to elapse under, `elapsed_ms` is 0,
    // the full 100 ms is spun exactly as before, and the witness line below stays silent.
    //
    // The M4 follow-up's reasoning is why this is a deadline and not a trim, and it survives
    // verbatim: the FIRST look at a root port is the CCS scan below, not `reset_root_port` — this
    // `if` decides whether that function is ever called. M4 shortened `wake_route`'s pre-look
    // settle on the strength of the caller paying T_ATTDB, and the caller that pays it is
    // downstream of a gate that had already sampled CCS. CF 0->1 is a real edge on this path (the
    // firmware-stale HCRESET drops CONFIGFLAG, and the first PORTSC read comes back 0x00001803
    // with CSC latched), so sampling CCS before the debounce has run — inside the 100 ms the
    // debounce is for — would let a port whose CCS has not re-asserted fall through `continue` with
    // no line, no EPACE class and no annotation: a boot that reads FASTER than predicted precisely
    // because the internal keyboard went missing. So the full 100 ms is still paid before the gate,
    // to the millisecond; BUY-1 changes only WHERE the clock ran, never how long it ran for.
    for (c, (h, attdb_t0)) in ctrls.iter_mut().zip(handles.iter()) {
        unsafe {
            let idx = c.idx;
            // USB 2.0 §7.1.7.3 T_ATTDB, the connect debounce owed ahead of the first CCS sample.
            const T_ATTDB_MS: u64 = 100;
            // `None` from `epace_ms` means the TSC rate is not calibrated yet and elapsed time is
            // unknowable — take 0, i.e. pay the whole debounce. The conservative branch is the one
            // that waits longer, never the one that samples early.
            let elapsed_ms = epace_ms(crate::arch::now_cycles().wrapping_sub(*attdb_t0))
                .unwrap_or(0)
                .min(T_ATTDB_MS);
            let owed_ms = T_ATTDB_MS - elapsed_ms;
            let attdb_wait_t0 = crate::arch::now_cycles();
            settle_ms(owed_ms); // the remainder, ahead of the first CCS sample — same guarantee
            c.pace.add(EP_ROOTRST, attdb_wait_t0);
            // The BUY-1 instrument, and the only line this change adds. Silent when the overlap
            // bought nothing (`elapsed_ms == 0`) — which is every single-controller machine,
            // including QEMU, where the pre-BUY-1 log is itself the witness that the full 100 ms
            // was spun. When it does fire it is the arithmetic in full: owed, covered, and paid,
            // so a reader can check that owed == covered + paid rather than take the trim on
            // trust. `rootrst=` in the EPACE line counts only what was PAID, so the two
            // instruments cross-check: the drop in `rootrst=` must equal the ms named here.
            if elapsed_ms > 0 {
                serial_println!(
                    ":: EHCI-HID: [{}] BUY-1 T_ATTDB overlap: {} ms owed, {} ms already elapsed under the earlier controllers' bring-up/port walk, {} ms spun here == witness ::",
                    idx, T_ATTDB_MS, elapsed_ms, owed_ms
                );
            }
            // EPACE-TRIM M1, read at the point of use (see the construction site). Set before the
            // probe below, and before this controller has run a single transfer, so an inherited
            // verdict and a self-measured one can never be confused.
            if !c.overlay_mode && CHAIN_HSE_SEEN.load(core::sync::atomic::Ordering::Relaxed) {
                c.overlay_mode = true;
                serial_println!(
                    ":: EHCI-HID: [{}] chain-HSE verdict CARRIED from an earlier controller — OVERLAY-DIRECT for this port walk (probe + re-init skipped; inference, not a measurement on this function) ::",
                    idx
                );
            }
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
                    // EPACE-TRIM M1: a controller that inherited the verdict a few lines
                    // up (witnessed there) skips the probe AND the wedged-controller
                    // re-init; the s58 split priced that pair at ~2.6 s.
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
                            ehci_scout::wake_route(h, idx);
                            if let Some(sts) = mmio_read32(h.op + OP_USBSTS) {
                                if sts & STS_RW1C != 0 {
                                    let _ = mmio_write32(h.op + OP_USBSTS, sts & STS_RW1C);
                                }
                            }
                            c.pace.add(EP_HCRST, hcrst2_t0);
                            let rootrst2_t0 = crate::arch::now_cycles();
                            // `true`: this path re-routed CONFIGFLAG a few lines up and
                            // returns straight to the port without passing the CCS gate,
                            // so it owns its own T_ATTDB. Unchanged by the M4 follow-up,
                            // and deliberately NOT overlapped by BUY-1 — the edge it
                            // debounces is the one this branch just applied, so there is
                            // no earlier work for it to have elapsed under.
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
        // M7: the overlapping transport view. Printed in `{}`, NEVER inside the `[]` bracket and
        // never summed with it — see the accumulator comment on `Epace`.
        let (xv, xu) = epace_fmt(c.pace.xfer_cy);
        let (av, au) = epace_fmt(c.pace.ass_cy);
        let (cv, cu) = epace_fmt(c.pace.act_cy);
        serial_println!(
            ":: EPACE: [{}] {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) [{}={}{}(n={}) {}={}{}(n={}) {}={}{}(n={}) resid={}{}] {{xfer={}{}(n={}) ass={}{} act={}{}}} == witness ::",
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
            rv, ru,
            xv, xu, c.pace.xfer_n, av, au, cv, cu
        );
        // EPACE-TRIM M8 — the print cap's escape valve. Silent on every boot that stayed inside
        // the cap (including the expected baseline: one line on [0], none on [1]); it exists so
        // that a device pathological enough to exceed the cap reports its true crossing count
        // instead of looking like exactly `M8_SLOW_CAP` slow transfers.
        if c.pace.slow_n > M8_SLOW_CAP {
            serial_println!(
                ":: EHCI-HID: [{}] EPACE-TRIM M8 SLOW-XFER cap reached — {} transfers crossed the {} ms threshold, {} printed, {} suppressed == witness ::",
                c.idx, c.pace.slow_n, M8_SLOW_MS, M8_SLOW_CAP,
                c.pace.slow_n.saturating_sub(M8_SLOW_CAP)
            );
        }
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
