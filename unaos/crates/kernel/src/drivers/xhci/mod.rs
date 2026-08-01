// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

pub mod trb;
pub mod ring;
pub mod event;
pub mod context;
pub mod dma_coherency;
pub mod ftdi;
// STOR-1: the interrupt-driven storage service task + BlockRequest submit/complete. x86_64 + the
// `irqstorage` knob only — the default build never links it, so the staged storage path is untouched.
#[cfg(all(target_arch = "x86_64", feature = "irqstorage"))]
pub mod irqstorage;

use ring::TransferRing;
use self::trb::Trb;
use self::event::{EventRing, ErstEntry, ErstTable};
use self::context::{InputContext, DeviceContext, CTX_WORDS};
use spin::Mutex;
use alloc::vec::Vec;

/// PIUSB-36 step 3: a dedicated static 512-byte buffer living in the kernel image's `.bss`
/// (physical address typically <4 MiB — a wholly different region from the 32 MiB heap the SCSI
/// data buffer comes from). One-boot experiment scratch only; read-only DMA target for the matrix.
#[cfg(target_arch = "aarch64")]
static mut PIUSB36_STATIC_BUF: [u8; 512] = [0; 512];

/// Flip to `true` to restore the very verbose per-doorbell / per-event xHCI tracing.
/// Left `false` so the serial log shows only milestones and errors.
const XHCI_VERBOSE: bool = false;

/// Verbose xHCI trace: compiles to nothing (optimized out) unless XHCI_VERBOSE is true.
macro_rules! xdbg {
    ($($arg:tt)*) => {
        if XHCI_VERBOSE { serial_println!($($arg)*); }
    };
}

/// USB HID Boot Keyboard Scancode to ASCII mapping.
/// Index is the HID usage ID (0x00..0x67). Returns (unshifted, shifted).
/// 0 means no printable character.
// pub(crate): the EHCI-3 HID path (drivers/ehci) decodes boot-keyboard reports through this same
// table so a key is a key whichever controller carried it (visibility-only change, EHCI-3 arc).
pub(crate) const HID_SCANCODE_TO_ASCII: [(u8, u8); 104] = [
    (0, 0),       // 0x00: Reserved
    (0, 0),       // 0x01: ErrorRollOver
    (0, 0),       // 0x02: POSTFail
    (0, 0),       // 0x03: ErrorUndefined
    (b'a', b'A'), // 0x04
    (b'b', b'B'), // 0x05
    (b'c', b'C'), // 0x06
    (b'd', b'D'), // 0x07
    (b'e', b'E'), // 0x08
    (b'f', b'F'), // 0x09
    (b'g', b'G'), // 0x0A
    (b'h', b'H'), // 0x0B
    (b'i', b'I'), // 0x0C
    (b'j', b'J'), // 0x0D
    (b'k', b'K'), // 0x0E
    (b'l', b'L'), // 0x0F
    (b'm', b'M'), // 0x10
    (b'n', b'N'), // 0x11
    (b'o', b'O'), // 0x12
    (b'p', b'P'), // 0x13
    (b'q', b'Q'), // 0x14
    (b'r', b'R'), // 0x15
    (b's', b'S'), // 0x16
    (b't', b'T'), // 0x17
    (b'u', b'U'), // 0x18
    (b'v', b'V'), // 0x19
    (b'w', b'W'), // 0x1A
    (b'x', b'X'), // 0x1B
    (b'y', b'Y'), // 0x1C
    (b'z', b'Z'), // 0x1D
    (b'1', b'!'), // 0x1E
    (b'2', b'@'), // 0x1F
    (b'3', b'#'), // 0x20
    (b'4', b'$'), // 0x21
    (b'5', b'%'), // 0x22
    (b'6', b'^'), // 0x23
    (b'7', b'&'), // 0x24
    (b'8', b'*'), // 0x25
    (b'9', b'('), // 0x26
    (b'0', b')'), // 0x27
    (b'\n', b'\n'), // 0x28: Return/Enter
    (0x1B, 0x1B), // 0x29: Escape
    (0x08, 0x08), // 0x2A: Backspace
    (b'\t', b'\t'), // 0x2B: Tab
    (b' ', b' '), // 0x2C: Space
    (b'-', b'_'), // 0x2D
    (b'=', b'+'), // 0x2E
    (b'[', b'{'), // 0x2F
    (b']', b'}'), // 0x30
    (b'\\', b'|'), // 0x31
    (0, 0),       // 0x32: Non-US # and ~
    (b';', b':'), // 0x33
    (b'\'', b'"'), // 0x34
    (b'`', b'~'), // 0x35
    (b',', b'<'), // 0x36
    (b'.', b'>'), // 0x37
    (b'/', b'?'), // 0x38
    (0, 0),       // 0x39: Caps Lock
    (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), // 0x3A-0x3F: F1-F6
    (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), // 0x40-0x45: F7-F12
    (0, 0), // 0x46: PrintScreen
    (0, 0), // 0x47: ScrollLock
    (0, 0), // 0x48: Pause
    (0, 0), // 0x49: Insert
    (0, 0), // 0x4A: Home
    (0, 0), // 0x4B: PageUp
    (0x7F, 0x7F), // 0x4C: Delete
    (0, 0), // 0x4D: End
    (0, 0), // 0x4E: PageDown
    // HID-KEYS: arrow keys map into the C0 control range (no printable-ASCII collision), so a
    // console consumer sees a distinct control byte and a game/UI consumer can bind them. Chosen
    // consistent with the table's existing control-code convention (Esc 0x1B, Backspace 0x08,
    // Delete 0x7F): 0x1C..0x1F, otherwise unused here. Shift does not change an arrow.
    (0x1C, 0x1C), // 0x4F: Right Arrow
    (0x1D, 0x1D), // 0x50: Left Arrow
    (0x1E, 0x1E), // 0x51: Down Arrow
    (0x1F, 0x1F), // 0x52: Up Arrow
    (0, 0), // 0x53: Num Lock
    (b'/', b'/'), // 0x54: Keypad /
    (b'*', b'*'), // 0x55: Keypad *
    (b'-', b'-'), // 0x56: Keypad -
    (b'+', b'+'), // 0x57: Keypad +
    (b'\n', b'\n'), // 0x58: Keypad Enter
    (b'1', b'1'), // 0x59: Keypad 1
    (b'2', b'2'), // 0x5A: Keypad 2
    (b'3', b'3'), // 0x5B: Keypad 3
    (b'4', b'4'), // 0x5C: Keypad 4
    (b'5', b'5'), // 0x5D: Keypad 5
    (b'6', b'6'), // 0x5E: Keypad 6
    (b'7', b'7'), // 0x5F: Keypad 7
    (b'8', b'8'), // 0x60: Keypad 8
    (b'9', b'9'), // 0x61: Keypad 9
    (b'0', b'0'), // 0x62: Keypad 0
    (b'.', b'.'), // 0x63: Keypad .
    (0, 0),       // 0x64: Non-US \ and |
    (0, 0),       // 0x65: Application
    (0, 0),       // 0x66: Power
    (b'=', b'='), // 0x67: Keypad =
];


/// Wall-clock budget for hardware handshakes, in `crate::arch::now_cycles()` units (rdtsc cycles on
/// x86_64, CNTVCT ticks on aarch64). Resolved per-arch so the same ~wall-clock window holds despite
/// the very different counter rates. On x86 it is an honest ~2 s once the TSC is calibrated against
/// the ACPI PM timer (see `arch::hw_wait_budget`); a fixed guess otherwise. ~2.5 s under QEMU/TCG.
///
/// Why a cycle budget and not an iteration count: the previous `50_000_000`-*iteration* budget
/// assumed cheap loop turns, but each turn does an uncached MMIO read (~0.5–1 µs on real silicon),
/// so a wedged status bit took ~25 s–3.5 min to time out — indistinguishable from a hang on a
/// serial-less laptop. A free-running counter makes the timeout a real wall-clock bound,
/// independent of MMIO-read latency and of EFLAGS.IF.
#[inline]
fn hw_wait_budget() -> u64 {
    crate::arch::hw_wait_budget()
}

/// Spin until `pred()` returns true or `budget` cycles (of `crate::arch::now_cycles()`) elapse.
/// On timeout it logs `what` and returns `Err(())` so the caller can bail. A throttled "still
/// waiting" breadcrumb shows progress on the (serial-less) framebuffer console so a slow or wedged
/// handshake reads as in-progress rather than frozen. This bounds every hardware wait so a
/// never-flipping status bit can no longer freeze boot silently or for minutes.
fn wait_until<F: Fn() -> bool>(pred: F, budget: u64, what: &str) -> Result<(), ()> {
    let start = crate::arch::now_cycles();
    let progress = budget / 10; // at most ~10 breadcrumb lines across the whole budget
    let mut last_report = start;
    loop {
        if pred() {
            return Ok(());
        }
        let now = crate::arch::now_cycles();
        // wrapping_sub so a 64-bit counter wrap mid-wait cannot prematurely trip the deadline.
        if now.wrapping_sub(start) >= budget {
            serial_println!("xHCI: TIMEOUT (~{} cyc) waiting for {}", budget, what);
            return Err(());
        }
        if progress != 0 && now.wrapping_sub(last_report) >= progress {
            serial_println!("xHCI: still waiting for {} ...", what);
            last_report = now;
        }
        core::hint::spin_loop();
    }
}

/// xHCI BIOS-to-OS handoff (USB Legacy Support, xHCI spec 7.1.1).
///
/// On real x86 hardware the firmware/SMM owns the controller at boot (for legacy USB keyboard
/// emulation) and will keep generating SMIs and fighting the OS for it unless we explicitly
/// claim ownership. We walk the xHCI Extended Capability list for the USB Legacy Support cap
/// (ID 1), set the "HC OS Owned" semaphore, wait for the firmware to drop "HC BIOS Owned", and
/// disable the firmware's legacy SMIs. QEMU does not expose this capability, so this is a clean
/// no-op there — but it is mandatory on metal. Mirrors Linux's `quirk_usb_handoff_xhci`.
fn bios_handoff(base_address: u64) {
    const CAP_ID_LEGACY: u8 = 1;
    const HC_BIOS_OWNED: u32 = 1 << 16;
    const HC_OS_OWNED: u32 = 1 << 24;
    // USBLEGCTLSTS: SMI *enable* bits to clear, and RW1C SMI *status* bits to acknowledge.
    const LEGCTL_DISABLE_SMI: u32 = (0x7 << 1) | (0xff << 5) | (0x7 << 17);
    const LEGCTL_SMI_EVENTS: u32 = 0x7 << 29;

    unsafe {
        // HCCPARAMS1 (CapBase + 0x10): bits 31:16 = xECP, the first extended cap in dword units.
        let hccparams1 = core::ptr::read_volatile((base_address + 0x10) as *const u32);
        let xecp = (hccparams1 >> 16) & 0xFFFF;
        if xecp == 0 {
            return; // no extended capabilities (e.g. QEMU)
        }

        let mut cap = base_address + (xecp as u64) * 4;
        // Bound the walk so a malformed list can't loop forever.
        for _ in 0..256 {
            let val = core::ptr::read_volatile(cap as *const u32);
            let id = (val & 0xFF) as u8;
            let next = ((val >> 8) & 0xFF) as u8;

            if id == CAP_ID_LEGACY {
                let usblegsup = cap;
                let usblegctlsts = cap + 4;
                let legsup = core::ptr::read_volatile(usblegsup as *const u32);
                if legsup & HC_BIOS_OWNED != 0 {
                    serial_println!("xHCI: claiming controller from BIOS (USBLEGSUP)...");
                }
                // Request OS ownership, then wait (bounded) for the BIOS to release it.
                core::ptr::write_volatile(usblegsup as *mut u32, legsup | HC_OS_OWNED);
                let released = wait_until(
                    || unsafe { core::ptr::read_volatile(usblegsup as *const u32) } & HC_BIOS_OWNED == 0,
                    hw_wait_budget(),
                    "BIOS to release xHCI (USBLEGSUP.BIOS_OWNED=0)",
                )
                .is_ok();
                if !released {
                    // Firmware stuck — force ownership: keep OS-owned, clear BIOS-owned.
                    let v = core::ptr::read_volatile(usblegsup as *const u32);
                    core::ptr::write_volatile(usblegsup as *mut u32, (v | HC_OS_OWNED) & !HC_BIOS_OWNED);
                    serial_println!("xHCI: BIOS did not release ownership; forced OS ownership.");
                }
                // Disable the firmware's legacy SMIs and acknowledge any pending SMI status.
                let legctl = core::ptr::read_volatile(usblegctlsts as *const u32);
                core::ptr::write_volatile(
                    usblegctlsts as *mut u32,
                    (legctl & !LEGCTL_DISABLE_SMI) | LEGCTL_SMI_EVENTS,
                );
                serial_println!("xHCI: BIOS->OS handoff complete.");
                return;
            }

            if next == 0 {
                return; // end of list; no legacy cap (nothing to do)
            }
            cap += (next as u64) * 4;
        }
    }
}

pub fn init(base_address: u64) {
    serial_println!("xHCI: Virtual Handoff. Base Address: {:#x}", base_address);

    let cap_ptr = base_address as *const u32;
    let cap_word = unsafe { core::ptr::read_volatile(cap_ptr) };
    let cap_length = (cap_word & 0xFF) as u8;

    let op_base = base_address + cap_length as u64;
    serial_println!("xHCI: CapLength: {}, Operational Base: {:#x}", cap_length, op_base);

    // Claim the controller from the firmware (real hardware) before we touch it.
    bios_handoff(base_address);
    // BPACE (M4): the BIOS→OS handoff. This is the first `hw_wait_budget()`-bounded wait of the
    // USB bring-up (waiting for firmware to drop USBLEGSUP.HC_BIOS_OWNED) and it can legitimately
    // burn the whole ~2 s budget on a machine whose SMM does not let go — on QEMU the capability
    // does not exist and the call returns instantly. `d=` from `portsw` is that handshake alone.
    crate::bootpace::record("xhci-handoff");

    let usbcmd_ptr = op_base as *mut u32;
    let usbsts_ptr = (op_base + 0x04) as *const u32;

    unsafe {
        // Halt Controller
        let cmd = core::ptr::read_volatile(usbcmd_ptr);
        core::ptr::write_volatile(usbcmd_ptr, cmd & !1);

        let _ = wait_until(
            || (core::ptr::read_volatile(usbsts_ptr) & 1) != 0,
            hw_wait_budget(), "USBSTS.HCH=1 (halt)");
        serial_println!("xHCI: Controller Halted.");
        // BPACE (M4): USBSTS.HCH=1 — the controller stopped. Budget-bounded (~2 s x86); a firmware
        // that left the controller running with a live schedule pays real time here.
        crate::bootpace::record("xhci-halt");

        // Reset Controller
        let cmd = core::ptr::read_volatile(usbcmd_ptr);
        core::ptr::write_volatile(usbcmd_ptr, cmd | 2);

        // Intel quirk (Linux XHCI_INTEL_HOST, xhci_reset): wait ~1 ms after setting HCRST
        // before ANY other register access, or the host can — rarely — hang the whole system.
        let t0 = crate::arch::now_cycles();
        let one_ms = (hw_wait_budget() / 2000).max(1);
        while crate::arch::now_cycles().wrapping_sub(t0) < one_ms {
            core::hint::spin_loop();
        }

        let _ = wait_until(
            || (core::ptr::read_volatile(usbcmd_ptr) & 2) == 0,
            hw_wait_budget(), "USBCMD.HCRST=0 (reset)");
        // BPACE (M4): USBCMD.HCRST self-cleared. `d=` from `xhci-halt` is the Intel 1 ms quirk pause
        // plus the chip hardware reset itself, budget-bounded.
        crate::bootpace::record("xhci-hcrst");

        // Wait for Controller Not Ready (CNR) to clear
        let _ = wait_until(
            || (core::ptr::read_volatile(usbsts_ptr) & (1 << 11)) == 0,
            hw_wait_budget(), "USBSTS.CNR=0");
        serial_println!("xHCI: Controller Reset Complete.");
        // BPACE (M4): USBSTS.CNR=0 — the controller is Ready and register programming may begin.
        // Intel clears CNR near-instantly (expect `d=`~0 on the rMBP); the Pi's VL805 holds it for
        // up to ~100s of ms while it loads its firmware (§1a "the CNR wall"), which is exactly the
        // number this stamp exists to make visible rather than inferred.
        crate::bootpace::record("xhci-cnr");
    }

    serial_println!("[XHCI] CONTROLLER RESET.");
}

/// Every PORTSC change bit (all RW1C): CSC(17) PEC(18) WRC(19) OCC(20) PRC(21) PLC(22) CEC(23).
/// LOAD-BEARING on real hardware: per xHCI 4.19.2 a port generates NO further Port Status
/// Change Events until ALL of these read 0 (the PSCEG edge trigger). Leaving any one latched
/// — the old mask was missing WRC, and the old handler cleared only PRC/CSC — silences the
/// port's events forever, which QEMU (an event per bit, no PSCEG model) never shows.
const PORT_CHANGE_BITS: u32 =
    (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);

/// M2 (XENUM-1): how many times a hub-downstream GET_DESCRIPTOR(device) is retried when it reads
/// all-zero / short (the documented vid=0000 hub-downstream intermittency) before the device is
/// left unconfigured. Bounded so a genuinely dead port cannot stall enumeration indefinitely.
const XENUM_DESC_RETRIES: u32 = 4;

/// XENUM-3 M2: how many times a hub-downstream ADDRESS_DEVICE is retried when the controller
/// answers with a non-success completion (metal rMBP: code 17 = Context State Error on the first
/// try behind the VIA hub) before the device is left unaddressed. Mirrors the root-port paced
/// recovery shape (bounded attempts, escalating settle between) so a transient first-address
/// failure no longer strands the device. Bounded so a genuinely dead port cannot loop forever.
const XENUM_ADDR_RETRIES: u32 = 3;

/// XENUM-2: how many hub-port status changes are serviced per main-loop wake. Each serviced change
/// runs synchronous control transfers (GET_PORT_STATUS + reset/enumerate or teardown); bounding the
/// count keeps a flapping downstream port from starving the main loop — leftover changes are
/// re-queued and drained on the next pass (the XENUM-1 bounded/paced discipline).
const HUB_CHANGE_BUDGET: usize = 8;

/// Map a hub downstream-port `wPortChange` bit index to its ClearPortFeature selector, or `None`
/// for a reserved bit. Acking the FULL change word (not just C_PORT_CONNECTION) is load-bearing on
/// real hardware: a USB hub keeps a change-bitmap bit set — and its interrupt-IN Status Change
/// Endpoint re-firing — while ANY C_* feature stays set (USB 2.0 §11.24.2.7 / USB 3.x §10.14.2.6).
///
/// The selectors are NOT simply `16 + bit`. USB 2.0 hubs use contiguous C_PORT_* selectors 16..20
/// for change bits 0..4 (connection/enable/suspend/over-current/reset). SuperSpeed hubs keep bits
/// 0/3/4 (connection/over-current/reset → 16/19/20) but relocate the rest: bit 5 = C_BH_PORT_RESET
/// (29), bit 6 = C_PORT_LINK_STATE (25), bit 7 = C_PORT_CONFIG_ERROR (26); bits 1/2 are reserved.
/// The old `16 + i` loop never reached bits 5..7, so an SS non-connection change — metal rMBP: a
/// card-reader-with-no-card raising C_PORT_LINK_STATE (`wPortChange=0x0040`) — was never acked and
/// the SS hub's Status Change Endpoint stormed the interrupt-IN forever (observed 1158+×).
fn hub_port_change_feature_selector(bit: u16, is_ss: bool) -> Option<u16> {
    match (bit, is_ss) {
        (0, _) => Some(16),     // C_PORT_CONNECTION
        (1, false) => Some(17), // C_PORT_ENABLE     (USB 2.0 only)
        (2, false) => Some(18), // C_PORT_SUSPEND    (USB 2.0 only)
        (3, _) => Some(19),     // C_PORT_OVER_CURRENT
        (4, _) => Some(20),     // C_PORT_RESET
        (5, true) => Some(29),  // C_BH_PORT_RESET   (SuperSpeed)
        (6, true) => Some(25),  // C_PORT_LINK_STATE (SuperSpeed)
        (7, true) => Some(26),  // C_PORT_CONFIG_ERROR (SuperSpeed)
        _ => None,
    }
}

pub static XHCI_CONTROLLER: spin::Mutex<Option<XhciController>> = spin::Mutex::new(None);

/// Human-readable mass-storage enumeration/bring-up state, for the shell `diskinfo` command when no
/// block device is published. Lets a metal storage failure be diagnosed from the interactive shell
/// (the boot enumeration log is wiped once the GUI takes over on the serial-less rMBP).
pub fn storage_diag() -> alloc::string::String {
    match &*XHCI_CONTROLLER.lock() {
        Some(x) => alloc::format!("storage: slot {} — {}", x.storage_slot, x.storage_note),
        None => alloc::string::String::from("storage: xHCI not initialised (USB bring-up skipped?)"),
    }
}

/// Live port + slot summary for the shell `usbinfo` command (metal enumeration diagnosis).
pub fn usb_summary() -> alloc::vec::Vec<alloc::string::String> {
    match &*XHCI_CONTROLLER.lock() {
        Some(x) => x.port_slot_summary(),
        None => alloc::vec::Vec::from([alloc::string::String::from("xHCI not initialised")]),
    }
}

/// Log the USB topology summary to serial exactly once, a short while into the main loop (after boot
/// enumeration has had time to run or stall) — captured in QEMU serial and a metal bootlog/usbdebug
/// build. Fires unconditionally so a stalled enumeration is recorded too; the interactive `usbinfo`
/// shell command reports the same on the serial-less GUI.
pub fn log_summary_once() {
    use core::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    if N.fetch_add(1, Ordering::Relaxed) != 2000 {
        return;
    }
    if let Some(x) = XHCI_CONTROLLER.lock().as_ref() {
        serial_println!("xHCI: === USB topology summary ===");
        for line in x.port_slot_summary() {
            serial_println!("xHCI: {}", line);
        }
        // IVY: BOT pump headroom alongside the topology it was measured on. This is a SNAPSHOT at
        // summary time (the main loop's 2000th pass), not an end-of-run tally — the authoritative
        // worst case is the LAST `:: BOT: … result=OK ::` line in the log, which `note_bot_pump`
        // emits whenever the peak doubles. Both carry route/depth, so a direct-attach boot and a
        // behind-hub boot can be diffed line for line.
        let (peak, budget) = (BOT_PUMP_PEAK.load(Ordering::Relaxed), BOT_PUMP_BUDGET.load(Ordering::Relaxed));
        let n = BOT_PUMP_COUNT.load(Ordering::Relaxed);
        let sum = BOT_PUMP_CYCLES.load(Ordering::Relaxed);
        serial_println!(
            ":: BOT: pump budget={} peak={} sum={} mean={} n={} nowait={} timeouts={} storage_slot={} route={:#x} depth={} tag_mismatch={} bad_sig={} abandoned_in={} abandoned_out={} undrained={} short_in={} short_out={} ev_late={} ev_unaddressed={} db_in={} db_out={} ev_stopped={} ev_stopped_li={} ev_any={} wrap_push={} wrap_db={} w={}/{}/{}/{}/{}/{}/{}/{}/{}/{}/{}/{} result=SUMMARY ::",
            budget, peak, sum, if n != 0 { sum / n } else { 0 },
            n, BOT_PUMP_NOWAIT.load(Ordering::Relaxed),
            BOT_PUMP_TIMEOUTS.load(Ordering::Relaxed),
            x.storage_slot,
            x.slots[x.storage_slot as usize].route_string, x.slots[x.storage_slot as usize].route_depth,
            // BOT-PHASE: the phase-desync census. `tag_mismatch=`/`bad_sig=` were one-off prints
            // with no denominator; `undrained=` is fix 1's own regression witness and MUST read 0.
            BOT_TAG_MISMATCH.load(Ordering::Relaxed), BOT_BAD_SIG.load(Ordering::Relaxed),
            BOT_TD_ABANDONED_IN.load(Ordering::Relaxed), BOT_TD_ABANDONED_OUT.load(Ordering::Relaxed),
            BOT_TD_UNDRAINED.load(Ordering::Relaxed),
            BOT_SHORT_DATA_IN.load(Ordering::Relaxed), BOT_SHORT_DATA_OUT.load(Ordering::Relaxed),
            BOT_EV_LATE_CLAIM.load(Ordering::Relaxed), BOT_EV_UNADDRESSED.load(Ordering::Relaxed),
            // ONSET-2 (M2): boot totals for the doorbell and stopped-event witnesses, plus the
            // log2-millisecond wait histogram `w=b0/b1/…/b11`. Bucket 0 is "under 1 ms", bucket k is
            // 2^(k-1)..2^k - 1 ms, bucket 11 saturates. If the polled-pump reading of the pace is
            // right the mass sits in buckets 0-2 with a handful of high outliers from the device's
            // own media-init latency; a spread across the middle buckets refutes it. `ev_stopped=` /
            // `ev_stopped_li=` are boot totals whose only meaningful reading is the per-recovery
            // delta on the `resync stopev` lines — see there for why 0 is not by itself reassuring.
            BOT_DB_IN.load(Ordering::Relaxed), BOT_DB_OUT.load(Ordering::Relaxed),
            BOT_EV_STOPPED.load(Ordering::Relaxed), BOT_EV_STOPPED_LI.load(Ordering::Relaxed),
            BOT_EV_ANY.load(Ordering::Relaxed),
            // ONSET-3: the ring-wrap population. `wrap_push=` counts every Link crossing on every
            // ring; `wrap_db=` counts the BOT doorbells that announced a TD sitting immediately
            // behind an armed Link — the exact population every gr9 onset was drawn from. Both are
            // denominators: see their statics for the healthy-but-idle readings (both LARGE and
            // non-zero on any boot that moves real I/O) and for what they can and cannot falsify.
            BOT_RING_WRAPS.load(Ordering::Relaxed), BOT_WRAP_DB.load(Ordering::Relaxed),
            BOT_WAIT_BUCKETS[0].load(Ordering::Relaxed), BOT_WAIT_BUCKETS[1].load(Ordering::Relaxed),
            BOT_WAIT_BUCKETS[2].load(Ordering::Relaxed), BOT_WAIT_BUCKETS[3].load(Ordering::Relaxed),
            BOT_WAIT_BUCKETS[4].load(Ordering::Relaxed), BOT_WAIT_BUCKETS[5].load(Ordering::Relaxed),
            BOT_WAIT_BUCKETS[6].load(Ordering::Relaxed), BOT_WAIT_BUCKETS[7].load(Ordering::Relaxed),
            BOT_WAIT_BUCKETS[8].load(Ordering::Relaxed), BOT_WAIT_BUCKETS[9].load(Ordering::Relaxed),
            BOT_WAIT_BUCKETS[10].load(Ordering::Relaxed), BOT_WAIT_BUCKETS[11].load(Ordering::Relaxed));
        // MULTIBLK: the transfer-size census, on its own line so the SUMMARY above stays
        // byte-comparable with pre-arc captures. `single=` counts data stages still issued at one
        // sector (partial-sector RMW head/tails, INQUIRY, READ CAPACITY, REQUEST SENSE); `multi=`
        // counts the genuine multi-block transfers this arc creates. The ratio is the direct read of
        // how much of the boot's I/O the coalescing actually caught; `maxlen=` proves the biggest TD
        // that reached the wire, and `wrapped_tx=` is the ring-wrap population a TIMEOUT-SHAPE line's
        // `wrapped=` must be read against.
        serial_println!(
            ":: BOT: tx single={} multi={} maxlen={} wrapped_tx={} rd_sectors={} wr_sectors={} max_blocks={} result=SIZES ::",
            BOT_TX_SINGLE.load(Ordering::Relaxed), BOT_TX_MULTI.load(Ordering::Relaxed),
            BOT_TX_MAXLEN.load(Ordering::Relaxed), BOT_TX_WRAPPED.load(Ordering::Relaxed),
            BOT_TX_RD_SECTORS.load(Ordering::Relaxed), BOT_TX_WR_SECTORS.load(Ordering::Relaxed),
            STORAGE_MAX_BLOCKS);
    }
}

pub static COMMAND_RING: Mutex<Option<TransferRing>> = Mutex::new(None);
pub static EVENT_RING: Mutex<Option<EventRing>> = Mutex::new(None);

pub static mut ERST_TABLE: ErstTable = ErstTable { entries: [ErstEntry { ring_address: 0, size: 0, _rsvd: 0, _rsvd2: 0 }] };

// Store Physical Address of the Event Ring for Runtime ERDP updates
static mut EVENT_RING_PHYS_BASE: u64 = 0;

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// PIUSB-10: set true once USBSTS.CNR has been observed clear (in `init_interrupter`, immediately
/// before the first op/runtime-register programming). `init_pointers` and `start` refuse to write
/// their registers if this is false, so a controller that never reports Ready (a wedged CNR) fails
/// LOUD and honestly instead of silently dropping every CRCR/DCBAAP/ERST/RS write into a not-ready
/// controller. Defaults true so a path that never runs the wait behaves exactly as before; on x86
/// CNR clears near-instantly, so this is always true and the guards are behaviourally invisible.
static XHCI_CNR_OK: AtomicBool = AtomicBool::new(true);

/// xHCI 5.4.1 / 4.2: after a Chip Hardware Reset (HCRST) software MUST NOT write ANY Doorbell,
/// Operational, or Runtime (interrupter) register until USBSTS.CNR (Controller Not Ready, bit 11)
/// reads 0. Intel clears CNR near-instantly, so on x86 this returns true on the first poll and is a
/// behavioural no-op. The Pi's VL805 holds CNR set for up to ~100s of ms while it loads its internal
/// firmware after HCRST, and — witnessed on metal (boot-P20: USBSTS=0x811, CNR=1) — every op/runtime
/// register write issued while CNR=1 is silently DROPPED (CRCR/DCBAAP/ERST all read back 0, RS never
/// latches). The pre-HCRST reset writes (USBCMD.RS clear to halt, then HCRST) are the ONLY register
/// writes that legitimately precede this wait — per spec 4.2 the CNR wait belongs AFTER HCRST, which
/// is exactly where this runs (the reset path is in `init()` / `reset()`; this gates the ring/
/// interrupter programming that follows). Bounded by `hw_wait_budget()` (~2.5 s aarch64 / ~2 s x86,
/// comfortably over the VL805's fw-load); a few-ms interval between polls. Returns false on timeout
/// so the caller aborts loudly rather than programming a not-ready controller (no hang either way —
/// the budget is a hard wall-clock bound).
fn wait_for_cnr_clear(op_base: usize) -> bool {
    let usbsts = (op_base + 0x04) as *const u32;
    let budget = hw_wait_budget();
    let start = crate::arch::now_cycles();
    // A few-ms pause between polls so a slow fw-load is not hammered with back-to-back MMIO reads.
    let poll_gap = (budget / 800).max(1);
    let mut polls: u64 = 0;
    loop {
        polls += 1;
        if unsafe { core::ptr::read_volatile(usbsts) } & (1 << 11) == 0 {
            #[cfg(target_arch = "aarch64")]
            serial_println!("xHCI: CNR cleared after {} polls", polls);
            return true;
        }
        if crate::arch::now_cycles().wrapping_sub(start) > budget {
            serial_println!(
                "xHCI: FATAL — USBSTS.CNR still 1 after {} polls (~{} cyc); aborting xHCI register programming (spec 5.4.1: op/runtime writes while CNR=1 are dropped)",
                polls, budget
            );
            return false;
        }
        let gap_start = crate::arch::now_cycles();
        while crate::arch::now_cycles().wrapping_sub(gap_start) < poll_gap {
            core::hint::spin_loop();
        }
    }
}

// --- Interrupt-driven xHCI (MSI-X via the local APIC) ---
// These let the interrupt handler acknowledge the interrupter using ONLY raw MMIO and
// lock-free atomics — it must NOT take the XHCI_CONTROLLER / EVENT_RING spin-locks (the
// main loop holds XHCI_CONTROLLER across the synchronous BOT pump, so locking here would
// self-deadlock). The actual event-ring drain stays in the polled context.
/// MMIO address of Interrupter 0's register set (IMAN at +0x00). 0 = not yet initialized.
pub static XHCI_IR0_BASE: AtomicUsize = AtomicUsize::new(0);
/// MMIO address of the operational registers (USBSTS at +0x04). 0 = not yet initialized.
pub static XHCI_OP_BASE: AtomicUsize = AtomicUsize::new(0);
/// Count of xHCI interrupts taken — a diagnostic to confirm the MSI-X path is live.
pub static XHCI_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

// --- IVY: BOT pump headroom accounting -------------------------------------------------
// The metal rMBP sitting of 2026-07-17 saw the FAT DELETE family (U10d / U11m2) fail with a
// BOT-pump TIMEOUT when the card reader sat BEHIND a hub (route 0x2), while the very same
// witnesses passed on a direct root port. The delete family is the LONGEST unbroken run of BOT
// transactions in the storage chain (see `pump_until_bot_done`), so it is the first op to expose
// any per-transfer latency the budget cannot absorb — but nothing in the log said HOW MUCH of the
// budget a transfer actually used, so "the budget is too tight behind a hub" could only ever be
// guessed at. These counters make the next sitting MEASURE it: the pump records the cycles each
// transaction actually consumed, keeps the high-water mark, and prints a `:: BOT: …` witness
// whenever the peak DOUBLES (a handful of lines per boot — log-scale, so default-quiet) and
// unconditionally on a timeout. `route`/`depth` are the storage slot's route string and hub depth,
// so a direct-attach log and a behind-hub log are directly comparable.
/// High-water mark, in `now_cycles` units, of the time ONE BOT stage spent in the pump.
pub static BOT_PUMP_PEAK: AtomicU64 = AtomicU64::new(0);
/// Total BOT pump waits that completed (any stage, any slot).
pub static BOT_PUMP_COUNT: AtomicU64 = AtomicU64::new(0);
/// BOT pump waits that hit the wall-clock deadline.
pub static BOT_PUMP_TIMEOUTS: AtomicU64 = AtomicU64::new(0);
/// The wall-clock budget (cycles) the most recent pump ran under — 0 until the first BOT transfer.
pub static BOT_PUMP_BUDGET: AtomicU64 = AtomicU64::new(0);
/// Pump entries that found NOTHING pending and returned immediately. These are not transfers and
/// must not inflate `n=` — worse, they carry no slot, so the fabricated `route=0 depth=0` they used
/// to be counted under made a behind-hub boot look like it had root-port traffic. Counted here and
/// reported as `nowait=` instead.
pub static BOT_PUMP_NOWAIT: AtomicU64 = AtomicU64::new(0);
/// The peak last PRINTED as a `:: BOT: … result=OK ::` witness — the throttle reference, so a peak
/// that creeps up in small steps still gets reported once it has doubled overall.
static BOT_PUMP_REPORTED: AtomicU64 = AtomicU64::new(0);
/// FRWRITE: the SUM, in `now_cycles` units, of every completed BOT pump wait. `peak=` alone cannot
/// answer the question the 2026-07-26 metal capture posed — "is EVERY transaction slow, or was one
/// outlier slow?" — because the peak witness is doubling-throttled: between a reported peak P and a
/// timeout, every wait could have been anywhere in [0, 2P) and no line would say so. `sum/n` is the
/// MEAN wait, and mean-vs-peak is exactly that discrimination. Reported as `sum=`/`mean=` on the
/// SUMMARY and TIMEOUT lines only, so the per-transfer log stays byte-identical.
pub static BOT_PUMP_CYCLES: AtomicU64 = AtomicU64::new(0);

// --- FTDI TX pump headroom (PH-6) ---
// The FTDI bulk-OUT pump used to be bounded by a raw iteration count (2000 `hlt()` yields), which
// measures nothing and means nothing: on a core whose timer is off, `hlt()` busy-spins and 2000
// passes expire in microseconds — a starved pump and a dead endpoint then produce the identical
// log. Converted to the same `now_cycles`/`hw_wait_budget()` wall-clock deadline the BOT pump uses,
// with the same counters, so the GUI-media boot that reads this log can tell the two apart.
/// High-water mark, in `now_cycles` units, of the time ONE FTDI TX transfer spent in the pump.
static FTDI_PUMP_PEAK: AtomicU64 = AtomicU64::new(0);
/// Total FTDI TX pump waits that completed.
static FTDI_PUMP_COUNT: AtomicU64 = AtomicU64::new(0);
/// FTDI TX pump waits that hit the wall-clock deadline.
static FTDI_PUMP_TIMEOUTS: AtomicU64 = AtomicU64::new(0);
/// The peak last PRINTED as a `:: FTDI: … result=OK ::` witness — the doubling throttle's reference.
static FTDI_PUMP_REPORTED: AtomicU64 = AtomicU64::new(0);

// --- CCSTRIM (2026-08-01): the settle's LATE-ASSERT detector ---
//
// CCSMARGIN (see `SETTLE_MS`) can only time ports that assert CCS *before* the settle deadline.
// A port that asserts one millisecond after it reads `none` — identical to an empty port. That
// blind spot is tolerable when the settle is generously padded and fatal to a TRIM: every
// millisecond taken off the settle moves more of the real population into the un-timeable region,
// and the instrument would go on printing a comfortable `margin_ms` computed from the ports that
// still fit. A trim justified by a witness that cannot see the trim's own failure mode is not
// justified at all.
//
// So the deadline stops being the end of the measurement. Ports that read `none` are latched here,
// and the FIRST connect edge each one delivers afterwards — through the ordinary CSC / hot-plug
// path in `handle_port_status`, which is the path that recovers such a device — prints how long
// after the settle's start it actually arrived. That number is the falsifier: it is exactly the
// `SETTLE_MS` the machine would have needed, measured on the machine, with no upper bound.
//
// Armed once, at the end of the settle. Not reset per boot because there is only one boot.
static CCS_LATE_ARMED: AtomicBool = AtomicBool::new(false);
/// `now_cycles()` at the instant the settle began (i.e. immediately after the port-power loop).
static CCS_SETTLE_START: AtomicU64 = AtomicU64::new(0);
/// The live `SETTLE_MS`, so the late line can restate the budget it beat without importing it.
static CCS_SETTLE_MS_LIVE: AtomicU64 = AtomicU64::new(0);
/// Per root port: "the settle ended without ever seeing CCS=1 here". Cleared on the first connect
/// edge, so each port reports at most once and a later re-plug of the same port stays quiet.
static CCS_UNSEEN: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
/// How long after the settle's start a connect edge still counts as "the settle was too short"
/// rather than "a human plugged something in". Two seconds: the slowest bring-up either seat has
/// ever observed for a device that was physically present at power-on is the Pi's VL805 presenting
/// its root ports, in the high hundreds of milliseconds, and no boot-attached device can plausibly
/// take twice that. Past the window the port is simply disarmed and the ordinary hot-plug lines
/// carry it — an unbounded detector would fire on every hot-plug for the rest of uptime and make
/// the token useless as a wake pattern.
const CCS_LATE_WINDOW_MS: u64 = 2000;

// --- BOT error recovery (USB Mass Storage Bulk-Only Transport 1.0 §5.3.3/§5.3.4, "Reset
// Recovery") ---
//
// Until this arc there was NO error recovery anywhere on the BOT path: a failed stage returned
// `Err`, nothing reset the endpoint or the device, and `block.rs` did not retry — so ONE marginal
// timeout was terminal AND left the device desynchronised (its own state machine still parked in a
// data or CSW phase, with the host's next CBW landing where a CSW was expected). These counters make
// the frequency of recovery visible on metal; they stay at zero on a clean boot, so a non-zero
// reading is itself the finding.
/// Recovery sequences ATTEMPTED (one per failed BOT transaction that was eligible for recovery).
pub static BOT_RECOVER_COUNT: AtomicU64 = AtomicU64::new(0);
/// Recovery sequences that completed every step successfully (and therefore earned the one retry).
pub static BOT_RECOVER_OK: AtomicU64 = AtomicU64::new(0);
/// Post-recovery retries that SUCCEEDED — i.e. transactions this arc rescued from a terminal error.
pub static BOT_RETRY_OK: AtomicU64 = AtomicU64::new(0);
/// Post-recovery retries that failed anyway (the caller still sees the terminal `Err`).
pub static BOT_RETRY_FAIL: AtomicU64 = AtomicU64::new(0);

// --- BOT-RESCUE (2026-07-29): escalation, back-off and surrender ---
//
// The 2026-07-29 metal capture: an Alcor 058f:6362 card reader went fully non-responsive on a
// WRITE(10) — EP0 died with it — while the controller, the event ring and the command ring stayed
// demonstrably healthy (the FTDI slot kept transferring; every command completed cc=1). The
// existing ladder (class reset -> clear-halt x2 -> stop-ep/set-deq x2 -> one retry) therefore ran
// to completion, reported success, retried, failed, and was re-entered by the next block op —
// forever, at ~6 s of busy-spin per stage timeout. The permanence was never the ring and never the
// controller: it was that the ladder had no top and no floor. This is the top (two more rungs) and
// the floor (surrender).
//
/// Consecutive failed recovery+retry cycles on one slot before the ladder escalates past its
/// existing top rung. Two, not one: a single failure is exactly what the existing class-level
/// Reset Recovery exists to absorb (PIUSB-38's induced stall recovers on the first try, and a
/// marginal device on a long cable can lose one transaction and be fine), so escalating on the
/// first would fire the heavy rungs against devices the light one already fixed. Two, not three or
/// more: every extra rung costs a full first-attempt budget (~6 s) of desktop-starving busy-spin,
/// and the failure this arc addresses is PERMANENT — a device that fails the ladder twice in a row
/// has never, in any capture, recovered on the third.
const BOT_RESCUE_N_CONSEC: u32 = 2;
/// Base back-off between recovery attempts, doubling per consecutive failure up to
/// `BOT_RESCUE_BACKOFF_MAX_MS`. A device wedged mid-internal-stall (a flash controller in an
/// erase/wear-levelling window is the usual suspect for a WRITE that kills EP0) is made WORSE by
/// being hammered: each new transaction restarts its command timeout. Spec-scale, not
/// budget-derived — see `settle_ms`.
const BOT_RESCUE_BACKOFF_MS: u64 = 50;
const BOT_RESCUE_BACKOFF_MAX_MS: u64 = 400;
/// Port power-off dwell for escalation (b). USB 2.0 §7.1.7.3 gives a device 100 ms to see VBUS
/// removed and fully de-energise; less risks the device holding internal state across the "cycle"
/// and the whole rung being a no-op.
const BOT_RESCUE_PORT_OFF_MS: u64 = 100;
/// Port power-on settle for escalation (b): hub bPwrOn2PwrGood is at most 255 * 2 ms, and USB 2.0
/// §7.1.7.3 adds 100 ms of attach debounce before a reset may be driven. 300 ms covers the common
/// case without turning a doomed rung into a second multi-second stall.
const BOT_RESCUE_PORT_ON_MS: u64 = 300;
/// Multiplier on `hw_wait_budget()` for a BOT stage's wall-clock wait. THREE on the first attempt
/// (~6 s): a real device can legitimately stall 1–4 s on a write, and shortening this would turn a
/// slow-but-healthy stick into a false failure. ONE (~2 s) for an escalation retry: by then the
/// device has already burned two full ladders' worth of budget without answering, the question the
/// retry asks is "did the heavy reset revive it", and a revived device answers in milliseconds —
/// so the extra 4 s buys no information and is paid in frozen desktop.
const BOT_BUDGET_SCALE_FIRST: u64 = 3;
const BOT_BUDGET_SCALE_ESCALATION: u64 = 1;
/// BOT-RESCUE M3 witness 4: Transfer Events observed for a slot OTHER than the one a BOT stage is
/// waiting on. Monotonic; the pump snapshots it on entry and prints the DELTA on a timeout. The
/// discrimination it buys: a non-zero delta means the event ring, the interrupter and the
/// controller's event delivery were all alive and working for OTHER traffic throughout the wait, so
/// a missing completion for THIS slot is a property of this slot's endpoint or of the device — not
/// a globally wedged interrupter. A zero delta on a boot with other live devices (the FTDI console,
/// a HID) says the opposite and would move the investigation somewhere else entirely. The 2026-07-29
/// capture had to argue this by eye, from the FTDI slot's unrelated log lines.
pub static BOT_FOREIGN_EVENTS: AtomicU64 = AtomicU64::new(0);
/// Escalation rungs attempted (a = Reset Device, b = port power-cycle) and surrenders. Zero on a
/// clean boot, so any non-zero reading is itself the finding.
pub static BOT_RESCUE_RESET_DEVICE: AtomicU64 = AtomicU64::new(0);
pub static BOT_RESCUE_PORT_CYCLE: AtomicU64 = AtomicU64::new(0);
pub static BOT_RESCUE_SURRENDER: AtomicU64 = AtomicU64::new(0);

// --- BOT-PHASE (2026-07-29): the phase-desync witnesses ---
//
// The audit that opened this arc reconstructed, from a corrupted medium, a directory sector holding
// CBW bytes — i.e. the driver had put a Command Block Wrapper where FAT data belonged. The
// mechanism is a DIRTY RING: an error exit from `bot_transfer` used to return with TRBs still
// pushed on the bulk rings and the controller's dequeue pointer parked on them. The next
// transaction's doorbell then replayed that stale payload+CBW into a device whose own BOT phase
// machine was still mid-transfer, and the two state machines slid one phase apart: what the host
// called "data" the device answered as "command", and vice versa. Everything below exists to make
// that condition COUNTABLE rather than reconstructible only from a wrecked filesystem.
//
/// Error exits from `bot_transfer` that left at least one pushed-but-unretired TRB on a bulk ring,
/// split by pipe. These are the transactions that COULD have stranded a CBW; a non-zero reading is
/// expected on any boot that saw a real transport fault and says nothing by itself.
pub static BOT_TD_ABANDONED_IN: AtomicU64 = AtomicU64::new(0);
pub static BOT_TD_ABANDONED_OUT: AtomicU64 = AtomicU64::new(0);
/// The subset of the above for which the ring was NOT successfully resynchronised afterwards — a
/// stranded TRB the controller can still be pointed at when the next doorbell rings. **This is the
/// primary fix's own regression witness: with the single chokepoint in place it must read 0 on
/// every boot.** Counted from the POST-resync scan, which is read out of an endpoint context whose
/// TR Dequeue Pointer field is architecturally defined (the endpoint is Stopped by then) — unlike
/// the pre-resync scan, which on Intel silicon reads a frozen birth value under a Running endpoint
/// (see GUARD-STATE, §14). That is why the undrained counter, not the abandoned counter, is the one
/// with an asserted value.
pub static BOT_TD_UNDRAINED: AtomicU64 = AtomicU64::new(0);
/// Boot totals for the two CSW-validation rejections. Both were one-off `serial_println!`s with no
/// rate attached, so a log could show one and never answer "out of how many?" — the question that
/// separates a single torn read from a systematic overlay. Folded into the BOT SUMMARY line.
pub static BOT_TAG_MISMATCH: AtomicU64 = AtomicU64::new(0);
pub static BOT_BAD_SIG: AtomicU64 = AtomicU64::new(0);
/// Data stages whose Transfer Event residue said FEWER bytes moved than `dCBWDataTransferLength`
/// asked for. On an OUT stage this is a phase slip in the making: the device stopped accepting
/// bytes, so it is NOT in its status phase, and queueing the CSW there is what desynchronises the
/// two machines. Counted for both directions; only OUT is treated as a fault (see `bot_transfer_once`).
pub static BOT_SHORT_DATA_IN: AtomicU64 = AtomicU64::new(0);
pub static BOT_SHORT_DATA_OUT: AtomicU64 = AtomicU64::new(0);
/// Monotonic stage generation. Stamped into every `BotPending` at arm time and printed by every BOT
/// witness, so a completion, a strand line and a timeout can be tied to the SAME stage in a log
/// where TRB ADDRESSES RECUR — a 16-TRB ring at three pushes per transaction repeats an address
/// every ~5 transactions, which is the aliasing this arc's matching change defends against.
static BOT_STAGE_GEN: AtomicU32 = AtomicU32::new(0);
/// Transfer Events that arrived for a `BotPending` which had ALREADY been completed (`done`), and
/// were therefore refused rather than allowed to overwrite the recorded completion code. Non-zero
/// means real event aliasing is happening on this platform and the first-write latch is earning its
/// keep; zero means the rings are draining cleanly. Either way it is a fact, not an inference.
pub static BOT_EV_LATE_CLAIM: AtomicU64 = AtomicU64::new(0);
/// Error completions claimed by the BOT pump WITHOUT a TRB-address match — the narrow residue of
/// the blanket `is_error` claim this arc removed. Only reachable for an error whose TRB pointer
/// addresses nothing in either of this slot's bulk rings (the codes that post no TRB pointer at
/// all: Ring Underrun / Ring Overrun / VF Event Ring Full). A non-zero reading names exactly how
/// often the driver has to fall back on "it can only be ours".
pub static BOT_EV_UNADDRESSED: AtomicU64 = AtomicU64::new(0);

// --- ONSET-2 (M2, 2026-07-30): the witnesses the cold read of `rmbp-gr8` found missing ---
//
// Every counter below states its HEALTHY-BUT-IDLE reading in its own doc comment, because six
// instrument lies have now been caught in this subsystem by applying one rule: a counter whose
// healthy reading is indistinguishable from its interesting reading cannot falsify anything.
// `foreign=` is the current example — it is pinned at 0 by construction on this platform (the pump
// is a synchronous spin that submits no other traffic, so no other slot can have a TRB outstanding
// to complete) and therefore supports no verdict at all. It is KEPT, unchanged, for capture
// comparability; `BOT_EVENTS_SEEN` below is its replacement.
//
/// **Doorbell witness.** Per-pipe count of doorbells the BOT path has written, and the ring enqueue
/// index at the moment of the last one. Until this arc there was no line anywhere in any capture
/// saying a doorbell had been written, so "the doorbell was written and did not take" and "the
/// doorbell was never written" were indistinguishable in every capture ever taken — and that is
/// exactly the discriminator the ranked hypothesis (a doorbell that fails to restart the controller
/// across a Link TRB) turns on.
///
/// HEALTHY-BUT-IDLE READING: both counters advance monotonically, roughly once per transaction each
/// (the CBW and an OUT data stage share the OUT doorbell; the CSW always rings IN). A raw total is
/// therefore uninformative on its own — which is why the pump snapshots both at entry and the
/// timeout line prints the **delta over this wait** (`db_in_d=` / `db_out_d=`). A healthy stage has
/// a delta of at least 1 on the pipe it is waiting on. A timeout with a delta of **0** on the
/// awaited pipe means no doorbell was written for this stage at all; a non-zero delta means one was
/// written and the controller did not act on it. Those are different bugs and this is what tells
/// them apart.
pub static BOT_DB_IN: AtomicU64 = AtomicU64::new(0);
pub static BOT_DB_OUT: AtomicU64 = AtomicU64::new(0);
/// Ring enqueue index at the last doorbell on each pipe, so a timeout can say WHERE the producer
/// was when it last told the controller to look. HEALTHY-BUT-IDLE: cycles through 0..ntrb-1 with the
/// traffic; it is a position, not an alarm, and is read against `trb_idx=` on the TIMEOUT-SHAPE line.
static BOT_DB_IN_IDX: AtomicU32 = AtomicU32::new(0);
static BOT_DB_OUT_IDX: AtomicU32 = AtomicU32::new(0);
/// **Stopped-event census.** Transfer Events carrying completion code 26 (Stopped) and 27 (Stopped —
/// Length Invalid), boot totals. xHCI 1.2 §4.6.9: a Stop Endpoint issued against an endpoint with a
/// TD **in progress** must post a Transfer Event with one of those two codes for the interrupted TD;
/// an endpoint that never fetched the TD has nothing to interrupt and posts nothing. That is the
/// architectural discriminator between "the controller never fetched the work" and "the controller
/// fetched it and the device is NAKing" — the last ambiguity in the onset reading, which the cold
/// read could only call *suggestive*.
///
/// HEALTHY-BUT-IDLE READING: **0**, and — stated because the rule demands it — the "never fetched"
/// reading is **also 0**. This counter discriminates ONLY across a Stop Endpoint issued while a TD
/// is known to be outstanding, which is precisely the recovery window `resync_bulk_ep` prints the
/// delta over. Read anywhere else it says nothing.
///
/// ONSET-3 — THE TWO CODES ARE NOT INTERCHANGEABLE, and the arc learned that the expensive way.
/// gr9 boot 4's recovery posted cc=27 on the IN pipe while that pipe's own strand scan read
/// `gap=0 live=0` and the CSW had not been pushed at all: an idle endpoint. cc=27 means only "the
/// TRB Transfer Length field is invalid" (§6.4.5), which a controller also reports when it is
/// stopped at a position with no computable residual. **cc=26 is the in-progress discriminator;
/// cc=27 is not.** Never read the sum, and never read either without the post-stop TR Dequeue
/// Pointer beside it — that pointer is what names the TD, and it is only defined once the endpoint
/// has left Running (GUARD-STATE).
pub static BOT_EV_STOPPED: AtomicU64 = AtomicU64::new(0);
pub static BOT_EV_STOPPED_LI: AtomicU64 = AtomicU64::new(0);

// --- ONSET-3 (2026-07-30): the cc=26 Stopped event's PAYLOAD, not just its arrival ---
//
// THE MISSING BYTE. A Stopped (26) Transfer Event carries two fields the driver was throwing away:
// the TRB Pointer of the TD it interrupted, and the TRB Transfer Length — which for a Stopped event
// is the **RESIDUE**, the bytes of that TD that had NOT moved when the endpoint was stopped (xHCI 1.2
// §6.4.2.1, and §6.4.5: cc=26 is defined as the code whose length field IS valid, which is precisely
// why 26 and not 27 is worth latching). `handle_event_trb` counted the completion code and dropped
// both. It could not have kept them the ordinary way either: the residue normally rides
// `BotPending::residue`/`residue_seen`, and by the time a recovery's Stop Endpoint posts its event
// `bot_pending` is already `None` — so the latch below is deliberately independent of it.
//
// WHY IT DECIDES SOMETHING. At the gr9 onset the awaited TD was a 512-byte OUT data stage, and the
// question the arc could not answer was whether the device ever entered the data phase at all:
//   * residue == the TD's full length (512) -> the device accepted **ZERO** bytes. It never entered
//     the data phase. That points at the CBW->DATA handoff — and therefore at the two-TDs-
//     outstanding-under-one-doorbell straddle, where the CBW and the data TD sit on the same ring,
//     separated by a Link crossing, with a single doorbell covering both.
//   * residue < 512 -> the device DID enter the data phase and stalled part-way. That points at the
//     device or at the transfer itself, and retires the straddle as the explanation.
// Those are different bugs with different fixes, and one dword tells them apart.
//
/// Number of cc=26 payloads ever latched. **This is the validity flag, and it exists because the
/// three fields below cannot express "no event" any other way** — a residue of 0 is a REAL and
/// meaningful reading ("the device took every byte"), so a zero-initialised residue field would be
/// indistinguishable from it. Read the payload only when this is non-zero, and read it as belonging
/// to THIS recovery only when its delta across the Stop Endpoint window is non-zero (the
/// `resync stopev` line prints `stopev_fresh=` for exactly that).
///
/// HEALTHY-BUT-IDLE READING: **0**, with `stopev_dci=255 stopev_trb=0x0 stopev_res=none` printed
/// beside it — the sentinels below make "never latched" say so in words rather than in a number that
/// could be mistaken for data. A healthy boot never issues a Stop Endpoint against a busy endpoint,
/// so 0 here is the expected reading and is NOT evidence of anything.
pub static BOT_STOPEV_N: AtomicU64 = AtomicU64::new(0);
/// DCI of the endpoint whose TD the last cc=26 interrupted. Sentinel 255 = never latched (a real DCI
/// is 1..=31), so the field is self-describing without consulting `BOT_STOPEV_N`.
static BOT_STOPEV_DCI: AtomicU32 = AtomicU32::new(255);
/// TRB Pointer from the last cc=26 event — the physical address of the interrupted TD's TRB. Read it
/// against the `strand`/`TIMEOUT-TRB` lines' `wait=`/`ctxdeq=`: equal means the controller was
/// stopped on the very TRB the pump was waiting for. Sentinel 0 = never latched.
static BOT_STOPEV_TRB: AtomicU64 = AtomicU64::new(0);
/// TRB Transfer Length (residue, in bytes) from the last cc=26 event. **No sentinel is possible
/// here** — every value 0..=len is a legitimate reading — which is the whole reason `BOT_STOPEV_N`
/// and the `stopev_res=none` spelling exist. Never print this number bare.
static BOT_STOPEV_RES: AtomicU32 = AtomicU32::new(0);

/// ONSET-3: prints a residue byte count, or the spelled sentinel `none` when nothing has ever been
/// latched. A NUMERIC sentinel is unusable for this field: every value `0..=len` is a legitimate
/// residue, and 0 in particular is a real and important reading ("the device took every byte"), so
/// any in-band magic number would be exactly the instrument lie this project has caught seven times.
/// Allocation-free — the recovery path must not depend on the heap.
struct ResidueField(Option<u32>);
impl core::fmt::Display for ResidueField {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{}", v),
            None => f.write_str("none"),
        }
    }
}
/// Every Transfer Event dispatched, of any completion code and any slot. The denominator for the two
/// above, and the thing that makes a zero reading of them mean something.
/// HEALTHY-BUT-IDLE: advances with all USB traffic; only its DELTA over a named window is a reading.
pub static BOT_EV_ANY: AtomicU64 = AtomicU64::new(0);
/// **Event-ring liveness during a wait.** Every event-ring TRB consumed by `pump_until_bot_done`
/// during ONE wait, of any type — command completion, port status change, transfer for any slot.
/// This is `foreign=`'s replacement: unlike `foreign`, it CAN be non-zero on this platform, which is
/// the whole reason a zero reading from it means anything.
/// HEALTHY-BUT-IDLE READING: at least 1 per completed stage (the completion itself, which is what
/// ends the wait). At a timeout, `evts=0` says nothing at all came off the event ring across the
/// whole ~6 s; `evts>0` says the ring was being consumed throughout and only OUR completion never
/// arrived. Both readings are reachable, and they point at different halves of the machine.
/// Counted per-wait in the pump, not stored in a static.
///
/// **Transaction identity on the timeout line.** `dCBWTag`, CDB opcode and LBA of the transaction
/// the pump is waiting on. §15.2's code -> capture -> medium join was reconstructed by hand from a
/// wrecked filesystem; it should be readable straight off the log.
/// HEALTHY-BUT-IDLE: these are identity, not health — they always hold the last transaction's
/// values and are only ever printed on a timeout.
static BOT_LAST_TAG: AtomicU32 = AtomicU32::new(0);
static BOT_LAST_CDB0: AtomicU32 = AtomicU32::new(0);
static BOT_LAST_LBA: AtomicU32 = AtomicU32::new(0);
/// **Per-stage wait histogram**, log2 buckets in MILLISECONDS: bucket 0 = under 1 ms, bucket k>0 =
/// waits of 2^(k-1) .. 2^k - 1 ms, top bucket saturating. The capture carries only `sum`, `peak` and
/// `n`, which cannot answer whether the pace is uniform, bimodal or bursty — and cannot test the
/// reading that the ~1 ms mean is just the 1 kHz APIC tick the polled pump sleeps on (`IRQ_COUNT=0`
/// on every boot, so `hlt()` is woken by the timer and nothing else).
/// HEALTHY-BUT-IDLE READING: if the polled-pump reading is right, essentially every sample lands in
/// buckets 0-2 (under 4 ms) with a handful of high outliers from device-side media-init latency. A
/// spread across the middle buckets would refute it.
static BOT_WAIT_BUCKETS: [AtomicU64; 12] = [
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
];

// --- ONSET-2 (M3): the H1 experiment, behind two knobs that both default to today's behaviour ---
//
// The ranked hypothesis WAS that **the doorbell which must restart the controller across a Link TRB
// does not take**. All three genuine onsets in `rmbp-gr8` are the same shape — an OUT data stage,
// 512 bytes, at ring index 0, i.e. immediately behind a freshly written Link TRB — and boots B and G
// reproduce it deterministically at the same stage index and the same LBA on two different builds.
// Neither knob is a fix; each is a ONE-VARIABLE discriminator, and with both off the image is
// byte-identical to the pre-arc one.
//
// ONSET-3 RETIRES THAT HYPOTHESIS. gr9 boot 4's recovery posts `ev_stopped=1` (cc=26, whose TRB
// Transfer Length is defined VALID) on the OUT pipe, with the post-stop TR Dequeue Pointer sitting
// ON the awaited data TRB, and `db_out_d=0` across the whole ~6 s wait. Per xHCI 1.2 §4.6.9 that
// pair says the controller HAD crossed the Link, HAD fetched the data TD and was executing it, and
// was owed no further doorbell. A missed doorbell is not the mechanism, and no redundant doorbell
// was shipped. What is still open is why the DEVICE did not move the data — which is what the
// `stopev_res=` residue witness (see `BOT_STOPEV_N`) and knob 2 below are now aimed at.
//
/// **Knob 1 (`botring64`, `UNAOS_BOTRING64=1`): the bulk transfer rings' length in TRBs.**
///
/// Changing the ring length changes the wrap FREQUENCY and every wrap POSITION and nothing else. No
/// protection is touched, no command sequence changes, no TD shape changes, and
/// `TransferRing::would_lap`'s two-slot margin scales with the ring by construction
/// (`used + 2 >= n`). Applies to the storage slot's two BULK rings only. EP0, the command ring, the
/// hub interrupt ring and the HID rings are untouched, so the variable stays single.
///
/// ONSET-3 — HOW ITS RESULT MUST BE READ, because gr9's was read too strongly. This knob does NOT
/// remove Link crossings; it makes them ~4x rarer. gr9 boot 1 ran it and reported `wrapped_tx=0`,
/// which was taken as "that boot never wrapped" — it did, an estimated 2-4 times, and `wrapped_tx=`
/// simply counts DATA-stage pushes landing at index 0, none of which happened to be the crossing
/// push. So a clean `botring64` boot is NOT evidence that wraps are harmless; it is evidence that
/// the specific shape (an awaited TD at index 0 behind a Link) occurred less often. From this arc on
/// the result is read against `wrap_push=` on the SUMMARY line, which counts the crossings
/// themselves — without it the experiment has no denominator and cannot conclude anything.
#[cfg(not(feature = "botring64"))]
pub const BOT_RING_TRBS: usize = 16;
#[cfg(feature = "botring64")]
pub const BOT_RING_TRBS: usize = 64;

// The two knob TAGS below exist so `strings` on the metal artifact can settle which experiment the
// media carries. A compiled-in INTEGER (16 vs 64) leaves no text behind, and this project has twice
// shipped a knob that was wired into `arroyo` but not into `builder/` — green everywhere, disabled
// on the media, and invisible until the boot came back identical. One of each pair, and only one, is
// in any given image.
#[cfg(not(feature = "botring64"))]
const BOT_RING_KNOB_TAG: &str = "botring64=off-16trb";
#[cfg(feature = "botring64")]
const BOT_RING_KNOB_TAG: &str = "botring64=ON-64trb";
// BOT-CBW (2026-07-30): no longer a knob — a statement of what the driver ALWAYS does. The tag is
// kept on the KNOBS line because captures are compared across boots and a reader must be able to
// tell a post-fix artifact from a pre-fix one without diffing the source.
const BOT_CBWIOC_KNOB_TAG: &str = "cbw=always-awaited";
// BOOTPACE M2 (2026-07-30): likewise not a knob. The main loop brings the FTDI console up BEFORE it
// runs any storage I/O — `service_storage` holds the deferred SCSI bring-up until the enumeration
// queue has drained, and `service_ftdi` precedes it in both x86 service ladders — so the whole
// storage chain is witnessed live on the wire instead of replayed out of the capture ring. Carried
// on the KNOBS line so a capture can be dated: a log without this field predates the reordering.
const BOT_ORDER_TAG: &str = "order=console-first";
// BOOTPACE M3 (2026-07-30): likewise not a knob. All three synchronous pumps busy-poll the event
// ring for a spec-scale window (~200 µs) BEFORE falling into `hlt()`, so an awaited stage no longer
// costs a full 1 kHz APIC tick just because the controller was answered in microseconds. The hlt
// fallback is unchanged and still the only thing that makes progress under QEMU TCG. Carried on the
// KNOBS line so a capture can be dated: a log without this field has tick-quantised `mean=`.
const BOT_PUMP_TAG: &str = "pump=spin+hlt";

// **The CBW is an awaited stage. Unconditionally. This is a fix, and it is convicted on metal.**
//
// What it replaces: `bot_transfer_once` used to push the CBW with `control: 1 << 10` — Normal type,
// no IOC, no ISP, so it posted no completion at all — and then push the data TRB with NO pump
// between them. For an OUT data stage both TDs then rode the same bulk OUT ring under a SINGLE
// doorbell, and the driver held no witness that the CBW was ever consumed. §14.4's claim that "each
// BOT stage is awaited to completion before the next is queued, so at most ONE TRB is outstanding"
// was false in the source (§16.3). It is true now.
//
// THE METAL EVIDENCE (§17). Two boots, same tree, ONE variable (this behaviour), both forcing the
// flight-recorder RESERVATION path, both at ring=16, both carrying the ONSET-3 ring hardening:
//
//     awaited   n=1108 stages, timeouts=0, wrap_push=81, no io-cause
//     unawaited n=737  stages, timeouts=3, wrap_push=83, io-cause op=write lba=33742
//
// and the onset witness on the failing boot, `resync stopev dci=2 dir=out ev_stopped=1
// stopev_res=512` on a 512-byte OUT data stage: cc=26 is posted only for a TD the controller had
// FETCHED and was executing (xHCI 1.2 §4.6.9), and a residue equal to the full length says the
// DEVICE accepted zero bytes and never entered the data phase. That is the CBW->DATA handoff
// failing, i.e. the straddle: two TDs outstanding on one endpoint under one doorbell, with a Link
// traversal between them at a wrap. Awaiting the CBW removes the straddle and the failure with it.
//
// THE COST, STATED PLAINLY BECAUSE IT IS NOW PERMANENT. The pump is polled and `IRQ_COUNT=0`, so
// `hlt()` wakes on the 1 kHz APIC tick (§16.6): every awaited stage costs at least one tick. Adding
// the CBW takes a transaction from `T + D` to `2T + D` — roughly one extra millisecond each. Storage
// throughput pays for this on every single transaction, forever. It is worth paying: the alternative
// is a transport that silently desynchronises its phase machine against a real device, and the A/B
// above is what that costs instead.

// --- PH-2: runtime CHECK CONDITION handling (SCSI SPC-4 §4.5, USB MSC BOT 1.0 §6.5) ---
//
// A `Failed` CSW is NOT a transport error: the transaction completed, the device rejected the
// command and is now holding sense data (CHECK CONDITION). Until this arc `bot_transfer` returned
// such a result verbatim to the caller and never fetched the sense, so a device left in CHECK
// CONDITION at runtime failed every subsequent command with nothing in the log saying why. The
// exposure is real: the flight recorder writes ~64 KiB to the boot volume on every x86 boot.
// (FRWRITE 2026-07-26 — that is NOT "a ~128-sector WRITE(10) burst", as this comment used to claim.
// `scsi_data_buffer` was 512 bytes (see `configure_bulk_endpoints_sync`) and every block-layer caller
// passed `blocks = 1`, so the recorder's 64 KiB was ~129 SEPARATE single-sector WRITE(10)s, each
// preceded by a single-sector READ(10) for the RMW, on top of the per-cluster zero-fills — several
// hundred BOT transactions, not one burst. See usb_xhci.md §12.
//  MULTIBLK 2026-07-29 SUPERSEDES THE ARITHMETIC, NOT THE READING: the buffer is now 32 KiB
//  (`STORAGE_DATA_BYTES`), `blocks` is a real count, and `fs/fat.rs` coalesces contiguous sector runs
//  and skips the read on a full-sector overwrite. The same 64 KiB reservation is now a couple of
//  dozen transactions rather than several hundred. The EXPOSURE ARGUMENT above is unchanged and
//  still the reason this handler exists — fewer transactions is a smaller target, not no target.)
// These counters stay at zero on a clean boot, so any non-zero reading is itself the finding.
// --- MULTIBLK (2026-07-29): the storage data buffer, and why it is the size and alignment it is ---
//
// Until this arc the storage slot's `scsi_data_buffer` was 512 bytes, so the driver's maximum
// transfer size was ONE sector and every block-layer caller was forced to pass `blocks = 1`. §12.1
// of usb_xhci.md priced that: one flight-recorder reservation is ~730 separate BOT transactions and
// ~1460 awaited Transfer Events, and since each awaited event is an independent chance to hit the
// still-unexplained lost-completion wedge (mechanism M2), the amplification (M1) is what turns a
// low per-transaction hazard into a certainty. Growing the buffer is the structural repair.
//
// SIZE — 32 KiB / 64 sectors. Two reasons, not one:
//   * it is a whole 32 KiB FAT cluster on the sizes real sticks are formatted with, so
//     `zero_cluster` and a cluster-aligned data write each become ONE transaction; and
//   * the SCSI READ(10)/WRITE(10) transfer-length field is 16 bits of BLOCKS, so 64 is nowhere near
//     a CDB limit — the limit we are choosing against is the DMA staging cost, not the protocol.
//
// ALIGNMENT — 64 KiB, deliberately larger than the buffer. xHCI 1.2 §4.11.7.1 requires that a
// Normal TRB's data buffer NOT cross a 64 KiB boundary; a 32 KiB buffer aligned to 64 KiB cannot.
// That is the whole point: it means the data stage stays EXACTLY the shape §12.2 audited and
// cleared — ONE Normal TRB, one TD, one IOC completion event — for every transfer size up to the
// buffer's capacity. §12.6's step 3 proposed a chained multi-TRB TD with the boundary split done by
// hand, and called it "the only part with real xHCI risk"; over-aligning the buffer discharges that
// risk instead of managing it. Do NOT raise STORAGE_DATA_BYTES above STORAGE_DATA_ALIGN without
// reinstating the split, because at that point a single TRB CAN cross the boundary.
/// Size of the per-slot SCSI data-stage staging buffer.
pub const STORAGE_DATA_BYTES: usize = 32 * 1024;
/// Alignment of that buffer — see above: it is the 64 KiB TRB-boundary rule, not a cache concern.
pub const STORAGE_DATA_ALIGN: usize = 64 * 1024;
/// The largest `blocks` count a single READ(10)/WRITE(10) may carry, i.e. what the staging buffer
/// holds. The block layer publishes this to `fs/fat.rs` so callers chunk instead of guessing.
pub const STORAGE_MAX_BLOCKS: u16 = (STORAGE_DATA_BYTES / 512) as u16;

// --- MULTIBLK: M2 shape instrumentation ---
//
// M2 — the lost completion event — is still unexplained, and this arc does not claim to fix it.
// What it CAN do is make the next metal capture able to characterise it, which was impossible while
// every transfer was the same 512-byte single-TRB shape: with only one shape on the wire there is
// nothing for a wedge to correlate WITH. Now that transfer sizes vary by two orders of magnitude,
// these record the shape of the transaction the pump is waiting on, so a TIMEOUT line names it.
/// Stage the pump is waiting on: 1 = DATA, 2 = CSW, 0 = none yet.
static BOT_LAST_STAGE: AtomicU32 = AtomicU32::new(0);
/// Direction of that stage: 0 = none, 1 = IN, 2 = OUT.
static BOT_LAST_DIR: AtomicU32 = AtomicU32::new(0);
/// Byte length of that stage's TD.
static BOT_LAST_LEN: AtomicU32 = AtomicU32::new(0);
/// Index within its transfer ring of the TRB the pump is waiting on.
static BOT_LAST_TRB_IDX: AtomicU32 = AtomicU32::new(0);
/// True if pushing that TRB wrapped the ring — i.e. the push crossed the Link TRB, so the TD sits at
/// index 0 of a fresh lap under the toggled cycle colour. The direct test of "does the wedge
/// correlate with a ring wrap?".
///
/// ONSET-3: this used to be recorded as `idx == 0`, which is ALSO what a virgin ring's very first
/// push returns — no Link crossed, no colour toggled. It now reads
/// `ring::TransferRing::wrapped_on_last_push()`, which is the real predicate. The correction is
/// worth ~one false `wrapped=true` per ring per boot; it does not touch the gr9 readings, where the
/// failing pushes were the 6th wrap on a long-running ring, but a witness that answers a different
/// question at the boundary than in the body cannot be trusted at the boundary.
static BOT_LAST_WRAP: AtomicBool = AtomicBool::new(false);
/// Data stages issued at the legacy single-sector size (<= 512 B).
pub static BOT_TX_SINGLE: AtomicU64 = AtomicU64::new(0);
/// Data stages issued as a genuine multi-block transfer (> 512 B) — the count this arc creates.
pub static BOT_TX_MULTI: AtomicU64 = AtomicU64::new(0);
/// Largest data-stage byte length this boot ever put on the wire.
pub static BOT_TX_MAXLEN: AtomicU64 = AtomicU64::new(0);
/// Data-stage TRB pushes that landed on a ring wrap.
pub static BOT_TX_WRAPPED: AtomicU64 = AtomicU64::new(0);

// --- ONSET-3 (2026-07-30): the ring-wrap population, on both sides of the doorbell ---
//
// THE WRAP CORRELATION IS WEAKER THAN IT WAS REPORTED, and the correction belongs in the source
// because the overstated version was carried to the bench. The gr9 table read: clean boots at
// `wrapped_tx=0` (n=140) and `wrapped_tx=1` (n=62, twice), wedged boot at `wrapped_tx=6` (n=112) —
// presented as "no wraps -> clean". It does not say that. `wrapped_tx=` counts DATA-stage pushes
// that landed at index 0, on one ring. It does NOT count Link crossings, and **no boot in the gr9
// set was free of them**:
//   * boot 1 (`botring64=ON`, 64 TRBs, `wrapped_tx=0`) ran ~71 transactions and ~211 ring pushes
//     across its two bulk rings; at 63 usable slots per ring that is still ~2-4 Link crossings. It
//     crossed the Link and stayed clean. `wrapped_tx=0` meant only that none of its 69 data pushes
//     happened to be the push that crossed.
//   * boot 4 (16 TRBs, `wrapped_tx=6`, wedged) reconstructs from `db_out=58 db_in=95` to ~58
//     transactions and ~171 pushes, i.e. ~11 Link crossings, of which 6 were data pushes.
//   * boot 1 also moved MORE I/O than boot 4 (n=140 vs 112, wr_sectors 444 vs 333), so raw I/O
//     volume does not order the outcomes either.
// What survives is narrow and worth stating exactly: **the wedge has only ever been observed on an
// awaited TD sitting at index 0 immediately after a Link crossing.** Whether Link crossings as such
// carry any hazard is untested, because the count of them has never been in a capture. That is what
// `wrap_push=` fixes; `wrapped_tx=` alone could never have answered it.
//
/// **Every Link crossing, on every ring.** Incremented inside `ring::TransferRing::push` whenever a
/// push steps over the Link TRB and starts a new lap — command ring, EP0 rings, HID/hub rings and
/// both bulk rings alike. Counted in the producer, so it cannot disagree with what the hardware was
/// shown.
///
/// HEALTHY-BUT-IDLE READING: rises monotonically at roughly (pushes / (num_trbs - 1)) on each ring,
/// so on a healthy boot it is a LARGE number and always non-zero — it is a denominator, not an
/// alarm, and no single value of it is a fault. In particular a `botring64` boot will read NON-ZERO
/// here while reading `wrapped_tx=0`, and that pair is the whole point: it is the direct proof that
/// the 64-TRB experiment reduced the wrap RATE and never eliminated Link crossings, which is what
/// the gr9 table was read as showing and did not show.
///
/// What it can falsify: a boot that wedges with `wrap_db=0` while `wrap_push` is large says the
/// wedge happened on a doorbell that did NOT follow a wrap, which retires the wrap correlation
/// outright. Conversely a boot that stays clean through a large `wrap_db` weakens it by the same
/// arithmetic. Neither reading was available before this counter existed.
pub static BOT_RING_WRAPS: AtomicU64 = AtomicU64::new(0);
/// **BOT doorbells rung for a ring whose most recent push crossed the Link.** This is the exact
/// population every gr9 onset was drawn from: the doorbell that announces a TD sitting at index 0
/// of a fresh lap, immediately behind an armed Link. Counted in `bot_doorbell`, on the ring the
/// doorbell targets, so it is one relaxed add on a path already doing an MMIO write.
///
/// HEALTHY-BUT-IDLE READING: **non-zero and growing** on any boot that moves more than a ring's
/// worth of I/O — a bulk ring of 16 TRBs wraps every ~15 pushes, roughly every 5 transactions. It is
/// stated as non-zero deliberately: a counter pinned at 0 on a healthy boot would be indistinguish-
/// able from a counter that is simply never reached, which is the instrument lie this project has
/// now caught six times. Its READING is the ratio `timeouts= / wrap_db=` against `timeouts= /
/// (db_in= + db_out= - wrap_db=)`: if the wrap correlation is real those two hazard rates differ,
/// and if it is coincidence they converge. One boot cannot settle that; the counter is what makes
/// the question answerable at all.
///
/// NOTE — this does NOT count an extra doorbell. ONSET-3's first draft was to ring a redundant
/// doorbell after every wrapped push; the capture retired that idea (`db_out_d=0` across the whole
/// failing wait — no doorbell was owed), and a fix whose rationale has been refuted is not shipped
/// here. `wrap_db` counts the doorbells the driver already rings.
pub static BOT_WRAP_DB: AtomicU64 = AtomicU64::new(0);
/// Sectors moved by IN (read) data stages, and by OUT (write) data stages. `n=` counts pump WAITS
/// (two per transaction), and `single=`/`multi=` count TRANSACTIONS; neither says how much data
/// moved or which direction dominates. M2 is a write-path suspicion, so the read/write split is the
/// first thing a metal capture wants to divide `timeouts=` against.
pub static BOT_TX_RD_SECTORS: AtomicU64 = AtomicU64::new(0);
pub static BOT_TX_WR_SECTORS: AtomicU64 = AtomicU64::new(0);

/// REQUEST SENSE fetches issued from the runtime `Failed`-CSW path (one per Failed CSW handled).
pub static BOT_SENSE_COUNT: AtomicU64 = AtomicU64::new(0);
/// Sense-driven single retries that came back `Passed` — transactions this arc rescued.
pub static BOT_SENSE_RETRY_OK: AtomicU64 = AtomicU64::new(0);
/// Sense-driven single retries that failed anyway (the caller still sees the original failure).
pub static BOT_SENSE_RETRY_FAIL: AtomicU64 = AtomicU64::new(0);
/// Re-entrancy latch for the CHECK CONDITION handler. REQUEST SENSE and the one retry both run
/// through `bot_transfer`; while this is set a `Failed` CSW propagates exactly as it did before
/// this arc. That is what makes the recovery all-or-nothing: one sense, one retry, never a loop.
static BOT_SENSE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Test-only deterministic fault injection (`UNAOS_BOTFAULT=1` -> feature `botfaultinject`).
/// QEMU never times out a BOT transfer, so without this the recovery path is metal-only. Fires
/// EXACTLY ONCE, on the first BOT transaction with an OUT data stage (a WRITE(10) — after storage
/// bring-up and after the FAT mount, so the injection point is deterministic) and at the moment the
/// CSW would be read: the data stage really lands, so the device is genuinely left parked in its CSW
/// phase with a stale CSW pending. That makes the test a real assertion rather than a smoke test —
/// if Reset Recovery did NOT resynchronise the device, the retry's fresh CBW would collect the stale
/// CSW and fail on the tag mismatch.
#[cfg(feature = "botfaultinject")]
static BOT_FAULT_FIRED: AtomicBool = AtomicBool::new(false);

/// PH-2 companion injection under the same `UNAOS_BOTFAULT=1` knob: a deterministic **Failed CSW**,
/// so the runtime CHECK CONDITION path (sense fetch + single retry) is QEMU-provable too. QEMU's
/// usb-storage never rejects a well-formed command, so without this the path is metal-only. Fires
/// EXACTLY ONCE, on the first IN transaction carrying a full block or more (a READ(10) — INQUIRY is
/// 36 B, READ CAPACITY 8 B and REQUEST SENSE 18 B, so bring-up cannot trip it and the first hit is
/// the FAT layer's runtime read). The transaction itself really completed; only the decoded CSW
/// status is rewritten, so the retry runs against a healthy device and must pass.
#[cfg(feature = "botfaultinject")]
static BOT_FAULT_CC_FIRED: AtomicBool = AtomicBool::new(false);
/// Set by the injection above and consumed by the CHECK CONDITION handler: it marks the ONE
/// transaction whose failure was synthetic, whose real sense is therefore NO SENSE.
#[cfg(feature = "botfaultinject")]
static BOT_FAULT_CC_ACTIVE: AtomicBool = AtomicBool::new(false);

/// GUARD-STATE: one-shot latch for `bot_deqprobe`, the per-boot experiment that records whether THIS
/// platform's Output Endpoint Context TR Dequeue Pointer is live under a Running endpoint or frozen
/// at its birth value. Set before the probe runs, so a failure inside it cannot make it run twice.
static BOT_DEQPROBE_DONE: AtomicBool = AtomicBool::new(false);

/// PIUSB-39 witness counters. `MOUSE_REARM_COUNT` = every `queue_mouse_read` the transfer
/// dispatch issued; `MOUSE_DISCARD_REARM_COUNT` = completions the dup-Success guard threw away
/// but which STILL re-armed the interrupt-IN read (the pipeline-preserving exit the P54b metal
/// fact needed). Bumped unconditionally (cheap relaxed adds); only the knob-gated witness prints.
pub static MOUSE_REARM_COUNT: AtomicU64 = AtomicU64::new(0);
/// Completions the dup-Success GUARD discarded and re-armed (mismatching `param`, not the known
/// dup) — the population the P54b fix is about.
pub static MOUSE_DISCARD_REARM_COUNT: AtomicU64 = AtomicU64::new(0);
/// Re-arms driven by a non-halting ERROR completion on the pointer endpoint. A different
/// population from `MOUSE_DISCARD_REARM_COUNT`: counted (and printed) separately so a metal
/// capture can tell which hole it just watched get plugged. Halting errors are NOT counted here —
/// they go to `service_hid_halts`, which prints its own line.
pub static MOUSE_ERROR_REARM_COUNT: AtomicU64 = AtomicU64::new(0);

/// Acknowledge an xHCI interrupt at the hardware level so the interrupter can raise again.
/// Safe to call from interrupt context: it takes NO locks and does NO allocation — it clears
/// IMAN.IP (bit 0, RW1C) preserving IMAN.IE, and USBSTS.EINT (bit 3, RW1C). It does NOT drain
/// the event ring; the main loop / BOT pump owns that (and the controller lock). A spurious
/// interrupt (IP not set) is harmless — clearing an already-clear RW1C bit is a no-op. The
/// caller (the MSI-X handler) issues the local-APIC EOI; this function does not.
pub fn interrupt_ack() {
    let ir0 = XHCI_IR0_BASE.load(Ordering::Acquire);
    if ir0 != 0 {
        unsafe {
            let iman = core::ptr::read_volatile(ir0 as *const u32);
            // Write 1 to IP (clear, RW1C) while preserving IE — never blind-write 0x1,
            // which would clear IE and silence all future interrupts.
            core::ptr::write_volatile(ir0 as *mut u32, iman | 1);
        }
    }
    let op = XHCI_OP_BASE.load(Ordering::Acquire);
    if op != 0 {
        unsafe {
            core::ptr::write_volatile((op + 0x04) as *mut u32, 1 << 3); // USBSTS.EINT
        }
    }
    XHCI_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// THE GREAT UNIFICATION
/// Rings the xHCI Doorbell using raw assembly to ensure
/// strict ordering and immediate execution.
///
/// # Safety
/// Direct MMIO write. The address must be valid.
#[inline(always)]
pub unsafe fn ring_doorbell_asm(doorbell_addr: u64, target: u32) {
    xdbg!("xHCI: Ringing Doorbell at {:#x} with Target {}", doorbell_addr, target);
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    // aarch64 (added for JB2b, live on EVERY aarch64 metal xHCI — virt/Pi included): the fence
    // above lowers to `dmb ish`, which orders Normal (inner-shareable) memory only — the doorbell
    // is Device-nGnRE, which is Outer-Shareable, so without a stronger barrier the controller can
    // observe the doorbell BEFORE the TRB/context bytes it announces and fetch a stale cycle bit.
    // `dsb st` is what Linux's arm64 `writel` (`__iowmb`) issues before every MMIO store for
    // exactly this. Strictly-conservative strengthening on any aarch64 target. x86: compiled out
    // (TSO + the mfence above already order this); QEMU TCG never reorders (metal-only-visible).
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!("dsb st", options(nostack, preserves_flags));
    core::ptr::write_volatile(doorbell_addr as *mut u32, target);
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

/// Write a 64-bit xHCI operational/runtime register as an ordered pair of 32-bit
/// stores — low dword first, then high dword. xHCI 5.1 explicitly permits every
/// 64-bit register (CRCR, DCBAAP, ERSTBA, ERDP) to be accessed as two 32-bit
/// registers, and this is the ONLY portable form.
///
/// PIUSB-21 root cause: on the Pi 4 the VL805 sits behind the brcmstb PCIe root
/// complex, whose BAR window does not carry 8-byte MMIO TLPs. A single AArch64
/// `str x` (64-bit store) is down-converted by the RC and the 32-bit data lane is
/// REPLICATED into both dwords — a `0x02003240` DCBAAP write reads back
/// `0x0200324002003240`, an ERSTBA/ERDP write likewise (`0x0015b7800015b780`,
/// `0x0014fa400014fa40`). The mirrored high dword pushes every controller DMA base
/// above 4 GiB, outside the RC_BAR2 inbound window (RAM @ 0, 4 GiB), so the command
/// ring, ERST, and event ring all fetch from garbage: the ring shows CRR=1 but no
/// command completes and no event is ever posted. Splitting into two 32-bit stores
/// delivers the correct low dword and a true-zero high dword.
///
/// x86 is unaffected either way: Intel/AMD root complexes carry native 64-bit MMIO
/// (no replication), and two 32-bit stores are byte-identical there — this mirrors
/// Linux's universal `lo_hi_writeq`/`xhci_write_64`, so there is no x86 regression.
#[inline(always)]
unsafe fn write_reg64(reg: *mut u64, val: u64) {
    let p = reg as *mut u32;
    core::ptr::write_volatile(p, val as u32);
    core::ptr::write_volatile(p.add(1), (val >> 32) as u32);
}

/// Read a 64-bit xHCI register as two 32-bit loads (low then high), reassembled.
/// The same RC that replicates 64-bit stores also replicates 64-bit LOADS on the
/// Pi (a single `ldr x` returns lo mirrored into hi); two 32-bit loads return the
/// true low and high dwords. Byte-identical on x86.
#[inline(always)]
unsafe fn read_reg64(reg: *const u64) -> u64 {
    let p = reg as *const u32;
    let lo = core::ptr::read_volatile(p) as u64;
    let hi = core::ptr::read_volatile(p.add(1)) as u64;
    (hi << 32) | lo
}

/// Write the Event Ring Dequeue Pointer (ERDP, IR0 +0x18) as a **high-dword-first**
/// pair of 32-bit stores — the reverse of `write_reg64`.
///
/// XHCI-INT root cause (PIUSB-22, the "one report then silent" wall). ERDP is the ONE
/// 64-bit xHCI register with a *latch side effect*: its low dword carries EHB (bit 3,
/// Event Handler Busy, RW1C) and DESI plus the low pointer bits, and the controller
/// re-evaluates its event-ring free space — and clears EHB — the instant the low dword
/// is written. `write_reg64` writes low-then-high (PIUSB-21 order, correct for the
/// write-once init regs CRCR/DCBAAP/ERSTBA where nothing latches mid-pair). Applied to
/// ERDP under the genuine two-store split that PIUSB-21 forces on the brcmstb RC, that
/// order latches a TORN pointer: the low write commits the new low bits + clears EHB
/// while the high dword still holds the previous value, so the controller computes a
/// dequeue pointer with a stale (often mirror-garbage, >4 GiB) high dword, decides the
/// ring is full, and stops posting transfer events — the interrupt-IN HID pipe delivers
/// exactly one report then goes silent. The polled drain papers over it briefly (it reads
/// the cycle bit straight from DRAM regardless of EHB) until the controller's producer
/// catches the stale ERDP and halts. Writing HIGH first, then LOW (EHB + latch) last
/// guarantees the full 64-bit pointer is in place before the controller latches.
///
/// x86 is unaffected: no RC replication, both stores land, and Intel/AMD re-evaluate on
/// the complete pointer either order — byte-visible identical, MISSION gate unchanged.
#[inline(always)]
unsafe fn write_erdp(reg: *mut u64, val: u64) {
    let p = reg as *mut u32;
    core::ptr::write_volatile(p.add(1), (val >> 32) as u32); // high dword first
    core::ptr::write_volatile(p, val as u32);                // low dword (EHB + latch) last
}

/// Direction of a Bulk-Only Transport data stage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction { In, Out, None }

/// Result status decoded from a Command Status Wrapper (CSW).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CswStatus { Passed, Failed, PhaseError, Unknown }

/// Error outcomes from a Bulk-Only Transport transaction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BotError {
    Timeout,
    Stall,
    BadCswSignature,
    TagMismatch,
    TransferError(u8),
    NoDevice,
    /// MULTIBLK: the request itself was inadmissible — a `blocks` count of 0, or one larger than the
    /// SCSI staging buffer can back (`STORAGE_MAX_BLOCKS`). Raised in `scsi_read10`/`scsi_write10`
    /// BEFORE anything is built or queued, so unlike every other variant it is not a transport
    /// fault and must not drag the pipe through Reset Recovery: it never reaches `bot_transfer`.
    BadRequest,
    /// BOT-RESCUE M2: the stage was REFUSED before anything was queued because enqueueing it would
    /// have lapped the controller's TR Dequeue Pointer on that bulk ring (xHCI 1.2 §4.9.1/§4.9.2 —
    /// see `TransferRing::would_lap`). A transport fault like any other from the caller's point of
    /// view: it feeds the same Reset Recovery ladder, which is precisely the right response, since
    /// the only way to reach it is a controller that has stopped consuming the ring.
    RingFull,
}

/// A successful BOT transaction result (CSW decoded).
#[derive(Clone, Copy, Debug)]
pub struct BotResult {
    pub status: CswStatus,
    pub residue: u32,
}

/// In-flight BOT stage state. BOT phases (CBW -> [DATA] -> CSW) are pumped one at a time;
/// the event handler records the completion (or an error) here while the synchronous pump
/// waits. The stage's TRB is matched by its physical address so it is never confused with
/// an unrelated transfer event.
#[derive(Clone, Copy)]
struct BotPending {
    slot_id: u8,
    in_dci: u8,
    out_dci: u8,
    wait_trb_phys: u64,
    done: bool,
    completion_code: u8,
    /// BOT-PHASE fix 4: monotonic stage generation, from `BOT_STAGE_GEN`, stamped when the stage is
    /// armed. TRB physical addresses RECUR — a 16-TRB ring at three pushes per BOT transaction
    /// repeats an address roughly every five transactions — so `wait_trb_phys` alone cannot tell a
    /// live stage's completion from a stale event for a long-dead one at the same slot. The
    /// generation cannot travel on the wire (a Transfer Event carries only the TRB pointer), so it
    /// is not a wire tag; it is (a) the log key that ties a completion, a strand line and a timeout
    /// to ONE stage, and (b) the identity the first-write latch below is defined against.
    generation: u32,
    /// BOT-PHASE fix 3: TRB Transfer Length RESIDUE (untransferred bytes) taken from this stage's
    /// Transfer Event (xHCI 1.2 §6.4.2.1 — for an IN or OUT Normal TRB the event reports what did
    /// NOT move). `run_bot_stage` used to return the completion code alone, so a data stage that
    /// moved fewer bytes than `dCBWDataTransferLength` asked for was indistinguishable from one that
    /// moved all of them — `cc=13 SHORT PACKET` was simply accepted and the CSW queued behind it.
    /// First-write latched via `residue_seen` for the same reason `Ep0Pending::data_seen` is: a
    /// duplicate Success after a Short Packet for the same TD (Panther Point's
    /// XHCI_SPURIOUS_SUCCESS quirk) would otherwise overwrite a real shortfall with 0.
    residue: u32,
    /// True once a Transfer Event has been recorded for this stage, so `residue == 0` is trusted as
    /// "everything moved" rather than "nothing observed yet". Doubles as the first-write latch.
    residue_seen: bool,
}

/// In-flight FTDI console bulk-OUT transfer (U2.5). The FTDI TX is a single bulk-OUT stage — no
/// CBW/CSW dance — so this is a slimmer twin of [`BotPending`]: the awaited Normal TRB is matched by
/// its physical address so the drain pump claims exactly its own completion. Inert (None) unless the
/// FTDI console is draining.
#[derive(Clone, Copy)]
struct FtdiPending {
    slot_id: u8,
    out_dci: u8,
    wait_trb_phys: u64,
    done: bool,
    completion_code: u8,
}

/// In-flight SYNCHRONOUS EP0 control transfer, used during hub bring-up (a main-loop, non-event
/// context). Like `BotPending` but for EP0: the awaited Status TRB is matched by its physical
/// address so the sync pump claims exactly its own completion before the async descriptor FSM
/// runs. Inert (None) during normal root-port enumeration, so that path is untouched.
#[derive(Clone, Copy)]
struct Ep0Pending {
    slot_id: u8,
    wait_trb_phys: u64,
    done: bool,
    completion_code: u8,
    /// XENUM-3 M1: physical address of this transfer's DATA-stage TRB (0 if there is no data
    /// stage). The residual consumer matches the event's TRB pointer against it, so only the real
    /// DATA-stage completion is recorded — Panther Point's XHCI_SPURIOUS_SUCCESS quirk (device
    /// 0x1e31, the 2012 rMBP's controller) can post a duplicate Success after a Short Packet for
    /// the same TD, and an unmatched consumer would let the dup clobber a real short-read residual.
    data_trb_phys: u64,
    /// XENUM-3 M1: TRB Transfer Length residual (untransferred bytes) captured from the DATA-stage
    /// transfer event of a control IN read. A short read leaves this non-zero; sync_control turns it
    /// into the ACTUAL transferred length so the downstream enumerator can reject a partial (e.g.
    /// 8-byte-header-only) descriptor that would otherwise pass the structural bLength/type gate.
    /// First-write latched (see `data_seen`): a duplicate Success for the same TD cannot overwrite it.
    data_residual: u32,
    /// True once a DATA-stage event was observed for this transfer (so a residual of 0 is trusted
    /// as "full read" rather than "no data event seen yet"). Doubles as the first-write latch.
    data_seen: bool,
}

/// In-flight SYNCHRONOUS command (ENABLE_SLOT / ADDRESS_DEVICE / CONFIGURE_ENDPOINT) issued
/// during hub bring-up. The completion is matched by the command TRB's physical address, so the
/// sync pump claims exactly its own command and the async enumeration FSM is left untouched.
#[derive(Clone, Copy)]
struct CmdPending {
    cmd_trb_phys: u64,
    done: bool,
    completion_code: u8,
    slot_id: u8,
}

/// One xHCI Supported Protocol Capability (extended cap ID 2): declares that root ports
/// `port_offset .. port_offset + port_count - 1` (1-based) speak USB `major.minor`. This is the
/// authoritative USB2-vs-USB3 port map — until now the driver inferred port type from the
/// current PORTSC speed, which is only valid while a device is attached and trained. Needed to
/// know which ports may take a SuperSpeed WARM reset (WPR is USB3-only) and to label `usbinfo`.
#[derive(Clone, Copy)]
pub struct PortProtocol {
    pub major: u8,
    pub minor: u8,
    pub port_offset: u8,
    pub port_count: u8,
}

/// Walk the xHCI Extended Capability list (same walk as `bios_handoff`) and collect every
/// Supported Protocol capability (ID 2). Layout per xHCI 7.2: dword0 = CapID | Next<<8 |
/// MinorRev<<16 | MajorRev<<24; dword1 = name string ("USB "); dword2 = Compatible Port
/// Offset (7:0) | Compatible Port Count (15:8). QEMU and real hardware both expose these.
fn parse_supported_protocols(base_address: usize) -> Vec<PortProtocol> {
    const CAP_ID_SUPPORTED_PROTOCOL: u8 = 2;
    let mut out = Vec::new();
    unsafe {
        let hccparams1 = core::ptr::read_volatile((base_address + 0x10) as *const u32);
        let xecp = (hccparams1 >> 16) & 0xFFFF;
        if xecp == 0 {
            return out;
        }
        let mut cap = base_address + (xecp as usize) * 4;
        for _ in 0..256 {
            let val = core::ptr::read_volatile(cap as *const u32);
            let id = (val & 0xFF) as u8;
            let next = ((val >> 8) & 0xFF) as u8;
            if id == CAP_ID_SUPPORTED_PROTOCOL {
                let dw2 = core::ptr::read_volatile((cap + 8) as *const u32);
                let p = PortProtocol {
                    major: (val >> 24) as u8,
                    minor: (val >> 16) as u8,
                    port_offset: (dw2 & 0xFF) as u8,
                    port_count: ((dw2 >> 8) & 0xFF) as u8,
                };
                serial_println!(
                    "xHCI: Supported Protocol: USB {}.{} ports {}..{}",
                    p.major, p.minor >> 4, p.port_offset,
                    p.port_offset as u32 + p.port_count.saturating_sub(1) as u32);
                out.push(p);
            }
            if next == 0 {
                break;
            }
            cap += (next as usize) * 4;
        }
    }
    if out.is_empty() {
        serial_println!("xHCI: no Supported Protocol capabilities found (port types unknown).");
    }
    out
}

pub struct DeviceSlot {
    pub active: bool,
    pub port_id: u8,
    /// USB device-descriptor idVendor / idProduct, captured in the device-descriptor handler (they
    /// were read there and discarded before U2.5). Used to recognise the FTDI FT232 (0403:6001) once
    /// its vendor-specific interface turns up in the config walk. 0/0 until the device descriptor
    /// arrives.
    pub vid: u16,
    pub pid: u16,
    pub input_context: *mut InputContext,
    pub output_context: *mut DeviceContext,
    pub ep0_ring: Option<TransferRing>,
    
    pub bulk_in_ring: Option<TransferRing>,
    pub bulk_out_ring: Option<TransferRing>,
    pub data_buffer: Option<*mut u8>,

    /// Dedicated DMA buffer for pointer (mouse/tablet) interrupt-IN reports. Separate from the
    /// keyboard's `data_buffer` so a composite device (keyboard + mouse in ONE unit, e.g. a wireless
    /// dongle) can have BOTH interrupt endpoints armed at once without their transfers racing into
    /// the same buffer.
    pub mouse_data_buffer: Option<*mut u8>,

    pub is_mouse: bool,
    /// True for a HID BOOT mouse (bInterfaceProtocol == 2): its report is RELATIVE signed deltas
    /// (button, dx:i8, dy:i8[, wheel]). False for the usb-tablet / absolute pointer (protocol 0),
    /// whose report is a 16-bit absolute X/Y. Selects the report decode in `poll_events`.
    ///
    /// LIMITATION: the relative-vs-absolute decision keys only on bInterfaceProtocol, which cleanly
    /// covers boot mice (proto 2 → relative) and the usb-tablet (proto 0 → absolute) — i.e. nearly
    /// every external mouse you'd plug into the Mac. A *non-boot* relative mouse that declares
    /// protocol 0 (report format defined solely by its HID Report Descriptor) is indistinguishable
    /// from an absolute tablet by protocol alone and falls through to the absolute path (same as
    /// before this field existed — not a regression). The robust fix is to parse the HID Report
    /// Descriptor's Input item Relative/Absolute flag; that's a scoped follow-up.
    pub mouse_is_relative: bool,
    pub mouse_ep: u8,
    pub mouse_mps: u16,
    pub mouse_interval: u8,
    /// bInterfaceNumber of the pointer's HID interface — SET_PROTOCOL(boot) wIndex.
    pub mouse_intf: u8,
    pub mouse_state: u8,
    pub mouse_ring: Option<TransferRing>,
    /// Physical address of the interrupt-IN Normal TRB the pointer read was last armed with
    /// (0 = none armed). The interrupt-IN transfer dispatch requires an exact match for the SAME
    /// reason EP0 does (`ep0_expect_phys`): Panther Point (Linux XHCI_SPURIOUS_SUCCESS quirk,
    /// device 0x1e31) can post a duplicate Success event after a Short Packet for the same TD —
    /// and a boot-mouse report is ALWAYS shorter than the endpoint MPS, so this is the periodic
    /// short-packet case the quirk fires on. Without the match, the dup would re-decode the same
    /// report (double cursor motion) AND re-arm a second read (ring over-arm). Set in
    /// `queue_mouse_read`, matched in `poll_events`/`handle_event_trb`.
    pub mouse_expect_phys: u64,
    /// PIUSB-39: physical address of the PREVIOUS armed pointer TRB (the TD that was just
    /// consumed, 0 = none yet). A genuine Panther-Point dup-Success names *that* TD — by then a
    /// fresh read is already armed, so the dup must be discarded WITHOUT re-arming (re-arming
    /// would over-arm the ring: the original UI1-MOUSE M2 hazard). Any OTHER mismatching `param`
    /// is not a dup: it means the completion the endpoint just retired is one we cannot account
    /// for, and the old `return` left the interrupt-IN endpoint permanently un-armed — the P54b
    /// metal fact (mouse dead after an EL0 focus drop, keyboard alive). Those re-arm.
    pub mouse_prev_phys: u64,
    /// Count of REAL (non-dup) pointer reports serviced since arming — drives the bounded serial
    /// mouse-witness (first report + every Nth), never one-line-per-report.
    pub mouse_report_count: u32,
    /// GUI-CLICK-2: previous pointer-button bitmask for this slot, so the decode emits a
    /// `pal::Event::Button` on the button-DOWN edge only (any bit going 0→1) and ignores the
    /// matching release. Mirrors the EHCI press-edge idiom (ehci/mod.rs) and `CLICK1_PREV_MASK`.
    /// 0 = no button held. Shared xHCI code: x86 xHCI mice track this identically.
    pub mouse_prev_buttons: u8,

    pub is_keyboard: bool,
    pub keyboard_ep: u8,
    pub keyboard_mps: u16,
    pub keyboard_interval: u8,
    /// bInterfaceNumber of the keyboard's HID interface — SET_PROTOCOL(boot) wIndex.
    pub keyboard_intf: u8,
    pub keyboard_state: u8,
    pub keyboard_ring: Option<TransferRing>,
    /// Physical address of the interrupt-IN Normal TRB the keyboard read was last armed with
    /// (0 = none armed). The keyboard interrupt-IN dispatch requires an exact match for the SAME
    /// reason the pointer read does (`mouse_expect_phys`): Panther Point (Linux
    /// XHCI_SPURIOUS_SUCCESS quirk, device 0x1e31) can post a duplicate Success event after a
    /// Short Packet for the same TD — and a boot-keyboard report (8 bytes) is ALWAYS shorter than
    /// the endpoint MPS, so this is exactly the periodic short-packet case the quirk fires on.
    /// Without the match, a dup would re-decode the same report (double-injected keystrokes) AND
    /// re-arm a second read (ring over-arm). Harmless for the current EXTERNAL keyboard (no metal
    /// dup observed) but PORTSW-1 brings the INTERNAL keyboard onto this exact path, so the guard
    /// mirrors the pointer path pre-emptively. Set in `queue_keyboard_read`, matched in the
    /// interrupt-IN transfer dispatch. On QEMU (no dup) `param` always matches, so it never trips.
    pub keyboard_expect_phys: u64,
    /// PIUSB-39: physical address of the PREVIOUS armed keyboard TRB. Same discrimination as
    /// `mouse_prev_phys`: a dup-Success for the consumed TD is discarded silently (a read is
    /// already armed), any other mismatch discards the data but RE-ARMS the read.
    pub keyboard_prev_phys: u64,
    /// Count of boot-keyboard interrupt-IN reports serviced since the read was armed. Mirrors
    /// `mouse_report_count`; drives the Pi-side PIUSB-13 `[enum]` first-report witness (the 0→1
    /// edge is "the keyboard is live"). Inert on x86 (nothing reads it there).
    pub keyboard_report_count: u32,
    /// HID-KEYS: the six keycodes (report bytes 2..8) of this slot's PREVIOUS boot-keyboard
    /// report. A boot report carries the FULL set of currently-pressed keys, so a keycode present
    /// last report but absent now IS a release — the decode emits `pal::Event::KeyUp(ascii)` for
    /// each such code (edge-detected here, mirroring the pointer-button press-edge idiom). All-zero
    /// = no keys held. Seeded to zero at enumeration and cleared on slot reuse.
    pub keyboard_prev_keys: [u8; 6],

    /// HID-LED: current keyboard lock-LED bitmap for this slot (USB HID Output report,
    /// LED usage page): bit0 = Num Lock, bit1 = Caps Lock, bit2 = Scroll Lock. Toggled on
    /// the press edge of the corresponding lock key and pushed to the device via SET_REPORT
    /// (0x21/0x09, wValue 0x0200). Caps also feeds the ascii case logic so the lit LED and the
    /// typed case agree. Seeded to zero at enumeration and cleared on slot reuse.
    pub keyboard_leds: u8,

    pub descriptor_buffer: *mut u8,

    /// Physical address of the STATUS TRB of the async EP0 TD the enumeration FSM is
    /// awaiting on this slot (0 = none). The async EP0 dispatch requires an exact match:
    /// Panther Point (Linux XHCI_SPURIOUS_SUCCESS quirk, device 0x1e31) can post a duplicate
    /// Success event after a Short Packet for the same TD, and a state-heuristic dispatch
    /// would re-enter the FSM on it. Sync EP0 TDs (`sync_control`) are claimed separately.
    pub ep0_expect_phys: u64,

    /// True for a device behind a hub (`address_downstream`). Downstream devices share the
    /// hub's ROOT port in `port_id` (needed for the slot context), so this flag — not
    /// port_id — is what distinguishes them from the root device the enumeration FSM owns:
    /// their async completions must not advance the root port queue or trip root recovery.
    pub is_downstream: bool,

    /// xHCI Route String (Slot Context DW0 bits 19:0) this device was addressed with, and its
    /// tier depth (hops from the root hub: 0 = root device / root-port hub, 1 = tier-1 downstream,
    /// …). A downstream HUB stores these so its own `bring_up_hub` can extend the route for its
    /// children: a child on port P of a hub at depth D gets `route | (P << (4*D))`, depth D+1.
    /// Root devices leave these at 0 (route string 0, addressed by the root FSM). Cleared in
    /// `reset_soft_state` so a recycled slot id cannot inherit a dead device's topology.
    pub route_string: u32,
    pub route_depth: u8,

    // Dedicated DMA buffers for Bulk-Only Transport (mass storage). Kept separate from
    // descriptor_buffer / data_buffer so a CBW can't clobber descriptors or HID reports.
    pub cbw_buffer: Option<*mut u8>,       // 31-byte Command Block Wrapper
    pub csw_buffer: Option<*mut u8>,       // 13-byte Command Status Wrapper
    pub scsi_data_buffer: Option<*mut u8>, // data-stage buffer (MULTIBLK: STORAGE_DATA_BYTES, not one block)
    pub bulk_in_ep: u8,                    // bulk IN endpoint address (e.g. 0x81)
    pub bulk_out_ep: u8,                   // bulk OUT endpoint address (e.g. 0x02)
    /// bInterfaceNumber of the Mass-Storage (class 0x08, SCSI Bulk-Only) interface. This is the
    /// `wIndex` of the Bulk-Only Mass Storage Reset class request (USB MSC Bulk-Only Transport 1.0
    /// §3.1) that `recover_bot_full` issues, so BOT error recovery cannot be issued without it.
    /// Captured in the config walk (root and hub-downstream) when the class-0x08 interface is
    /// detected; 0 until then — and 0 is a safe default, because the near-universal single-
    /// interface storage device legitimately uses interface 0.
    pub storage_intf: u8,

    // --- XENUM-2: hub Status Change Endpoint (hot-plug behind a hub) ---
    /// True once this slot has been marked as a USB hub (set in `set_hub_slot_context`). Lets the
    /// transfer-event dispatch recognise the hub's interrupt-IN Status Change Endpoint and the
    /// disconnect path route-scope a teardown to this hub's subtree.
    pub is_hub: bool,
    /// Number of downstream ports this hub reported (from its hub descriptor). Governs the length of
    /// the Status Change bitmap ((nbr_ports+1+7)/8 bytes) and the port iteration on a change.
    pub hub_nbr_ports: u8,
    /// The hub's interrupt-IN Status Change Endpoint address (0 = not configured). One per hub.
    pub hub_int_ep: u8,
    /// Max packet size of the Status Change Endpoint.
    pub hub_int_mps: u16,
    /// Transfer ring for the Status Change Endpoint interrupt-IN reads.
    pub hub_int_ring: Option<TransferRing>,
    /// DMA buffer the Status Change bitmap is read into (separate from `descriptor_buffer`, which
    /// the hub's synchronous control transfers reuse).
    pub hub_change_buffer: Option<*mut u8>,
    /// Physical address of the interrupt-IN Normal TRB the change-bitmap read was last armed with
    /// (0 = none). Matched in the transfer dispatch to reject a Panther-Point dup-Success for an
    /// already-consumed TD — the same guard `mouse_expect_phys` applies to the pointer read.
    pub hub_int_expect_phys: u64,
}

unsafe impl Send for DeviceSlot {}
unsafe impl Sync for DeviceSlot {}

impl DeviceSlot {
    pub fn new() -> Self {
        let desc_layout = core::alloc::Layout::from_size_align(256, 64).unwrap();
        let desc_buffer = unsafe { alloc::alloc::alloc_zeroed(desc_layout) };
        Self {
            active: false,
            port_id: 0,
            vid: 0,
            pid: 0,
            input_context: core::ptr::null_mut(),
            output_context: core::ptr::null_mut(),
            ep0_ring: None,
            bulk_in_ring: None,
            bulk_out_ring: None,
            data_buffer: None,
            mouse_data_buffer: None,
            is_mouse: false,
            mouse_is_relative: false,
            mouse_ep: 0,
            mouse_mps: 0,
            mouse_interval: 0,
            mouse_intf: 0,
            mouse_state: 0,
            mouse_ring: None,
            mouse_expect_phys: 0,
            mouse_prev_phys: 0,
            mouse_report_count: 0,
            mouse_prev_buttons: 0,
            is_keyboard: false,
            keyboard_ep: 0,
            keyboard_mps: 0,
            keyboard_interval: 0,
            keyboard_intf: 0,
            keyboard_state: 0,
            keyboard_ring: None,
            keyboard_expect_phys: 0,
            keyboard_prev_phys: 0,
            keyboard_report_count: 0,
            keyboard_prev_keys: [0; 6],
            keyboard_leds: 0,
            descriptor_buffer: desc_buffer,
            ep0_expect_phys: 0,
            is_downstream: false,
            route_string: 0,
            route_depth: 0,
            cbw_buffer: None,
            csw_buffer: None,
            scsi_data_buffer: None,
            bulk_in_ep: 0,
            bulk_out_ep: 0,
            storage_intf: 0,
            is_hub: false,
            hub_nbr_ports: 0,
            hub_int_ep: 0,
            hub_int_mps: 0,
            hub_int_ring: None,
            hub_change_buffer: None,
            hub_int_expect_phys: 0,
        }
    }

    /// Clear the device-personality fields so a REUSED slot id (a later ENABLE_SLOT may hand out
    /// the same id after DISABLE_SLOT) cannot inherit a dead device's state machine — a stale
    /// `keyboard_state`/`mouse_state` would misroute the new device's EP0 completions. The
    /// transfer rings, DMA buffers and contexts are intentionally LEAKED (forget / pointer drop
    /// without free): the controller may still reference them until its DISABLE_SLOT completes,
    /// so freeing invites use-after-free DMA. Recovery is rare; the leak is bounded and safe.
    /// `descriptor_buffer` is kept — it is allocated once per slot and reused.
    pub fn reset_soft_state(&mut self) {
        self.active = false;
        self.port_id = 0;
        self.vid = 0;
        self.pid = 0;
        if let Some(r) = self.ep0_ring.take() { core::mem::forget(r); }
        if let Some(r) = self.bulk_in_ring.take() { core::mem::forget(r); }
        if let Some(r) = self.bulk_out_ring.take() { core::mem::forget(r); }
        if let Some(r) = self.keyboard_ring.take() { core::mem::forget(r); }
        if let Some(r) = self.mouse_ring.take() { core::mem::forget(r); }
        // XENUM-2: hub Status Change Endpoint ring is leaked like the others (the controller may
        // still reference it until DISABLE_SLOT completes); the change buffer is dropped (leaked).
        if let Some(r) = self.hub_int_ring.take() { core::mem::forget(r); }
        self.is_hub = false;
        self.hub_nbr_ports = 0;
        self.hub_int_ep = 0;
        self.hub_int_mps = 0;
        self.hub_change_buffer = None;
        self.hub_int_expect_phys = 0;
        self.input_context = core::ptr::null_mut();
        self.output_context = core::ptr::null_mut();
        self.data_buffer = None;
        self.mouse_data_buffer = None;
        self.cbw_buffer = None;
        self.csw_buffer = None;
        self.scsi_data_buffer = None;
        self.is_mouse = false;
        self.mouse_is_relative = false;
        self.mouse_ep = 0;
        self.mouse_mps = 0;
        self.mouse_interval = 0;
        self.mouse_intf = 0;
        self.mouse_state = 0;
        self.mouse_expect_phys = 0;
        self.mouse_prev_phys = 0;
        self.mouse_report_count = 0;
        self.mouse_prev_buttons = 0;
        // UVUG-5: a keyboard slot is being torn down (detach / disconnect / enum-recovery). Signal the
        // host-side typematic tracker BEFORE clearing `is_keyboard`, so it can drop a key held at unplug —
        // under SET_IDLE(0) that key's `KeyUp` will NEVER arrive, and without this the repeat synthesiser
        // would inject `Event::Key` forever at the repeat rate. This is the single chokepoint every teardown
        // path funnels through (dispose_disconnected_slots / recovery / dispose_downstream_slot).
        if self.is_keyboard {
            crate::pal::note_keyboard_detached();
        }
        self.is_keyboard = false;
        self.keyboard_ep = 0;
        self.keyboard_mps = 0;
        self.keyboard_interval = 0;
        self.keyboard_intf = 0;
        self.keyboard_state = 0;
        self.keyboard_expect_phys = 0;
        self.keyboard_prev_phys = 0;
        self.keyboard_report_count = 0;
        self.keyboard_prev_keys = [0; 6];
        self.keyboard_leds = 0;
        self.ep0_expect_phys = 0;
        self.is_downstream = false;
        self.route_string = 0;
        self.route_depth = 0;
        self.bulk_in_ep = 0;
        self.bulk_out_ep = 0;
        self.storage_intf = 0;
    }
}

/// JB10 (Tegra-only, HYPOTHESIS — verify on the Orin bench): enumerate a Full-Speed ROOT device
/// whose real `bMaxPacketSize0` > 8 the Linux `xhci_check_maxpacket` way. Instead of reading the
/// full 18-byte descriptor at the guessed MPS0=8 (which babbles: the device answers with a packet
/// larger than 8) and then TEARING the slot down + resetting the port to re-address at MPS0=64,
/// read only the first 8 bytes (one packet, no babble), learn `bMaxPacketSize0`, patch EP0 MPS0 in
/// place with an Evaluate Context command, then read the full descriptor — all on the SAME slot,
/// no DISABLE_SLOT, no port reset. The JB9 bench (serial: port 7, an FS device) showed the babble
/// → DISABLE_SLOT → reset → re-ADDRESS@MPS64 cycle re-addresses correctly yet leaves the device
/// SILENT (dev-desc watchdog-times-out, PORTSC still 0x603 connected) — i.e. the tear-down churn
/// itself is what the FS device does not survive. This flag flips false to fall back to the shared
/// babble→recover path on the same build. Tegra-only: non-tegra builds are byte-identical (the
/// whole path is `#[cfg(feature = "tegra")]`); QEMU cannot exercise it (lenient MPS enforcement).
/// See docs/dev/OS/01_BOOT_HAL/arch_arm64.md §JB10.
#[cfg(feature = "tegra")]
const JB10_FS_EVAL_CTX: bool = true;

pub struct XhciController {
    base_addr: usize,
    op_base: usize,
    pub max_slots: u8,
    pub max_ports: u8,
    pub dcbaap: *mut u64,
    pub slots: Vec<DeviceSlot>,
    pub pending_ports: Vec<u8>,
    /// Connected ports discovered at boot but not yet enumerated. Drained one at a
    /// time (serialized) so the shared enable-slot / configuring-slot state can never
    /// be clobbered by two devices resetting simultaneously.
    pub ports_to_enumerate: Vec<u8>,

    pub configuring_slot: u8,
    pub event_ring_phys_base: u64,

    /// Slot id of the enumerated mass-storage device (0 = none).
    pub storage_slot: u8,
    /// Set once the storage bulk endpoints are configured; the main loop performs the
    /// (synchronous) SCSI bring-up + first read in a safe, non-event context.
    pub storage_pending_bringup: bool,

    // --- U2.5 FTDI USB-serial console (root-port only) ---
    /// Slot whose Configure-Endpoint we issued for the FTDI bulk endpoints (0 = none). Kept
    /// SEPARATE from `configuring_slot` so the completion is not claimed by the storage branch.
    ftdi_configuring_slot: u8,
    /// Slot id of the enumerated FTDI FT232 (0 = none).
    ftdi_slot: u8,
    /// Set once the FTDI bulk endpoints are configured; the main loop runs the (synchronous)
    /// SET_CONFIGURATION + FTDI vendor setup in a safe, non-event context (mirrors storage).
    ftdi_pending_bringup: bool,
    /// In-flight FTDI console bulk-OUT transfer (the drain pump waits on it).
    ftdi_pending: Option<FtdiPending>,
    /// Running total of console bytes drained out the FTDI cable (for the TX-mirror PASS line).
    ftdi_tx_total: u64,
    /// One-shot: the FTDI-TX-mirror PASS line has been printed (after the first backlog drain).
    ftdi_pass_logged: bool,
    /// One-shot: the FTDI-TX-disabled line has been printed (so a wedged sink logs exactly once).
    ftdi_disabled_logged: bool,
    /// Human-readable progress of the mass-storage bring-up, surfaced by the shell `diskinfo`
    /// command. On the serial-less rMBP the boot enumeration log is wiped when the GUI takes over,
    /// so a failed bring-up would otherwise be a silent "no device"; this makes the stall point
    /// (SET_CONFIGURATION / INQUIRY / READ CAPACITY / ...) visible from the interactive shell.
    pub storage_note: &'static str,
    /// Monotonic CBW tag.
    pub bot_tag: u32,
    /// In-flight BOT transaction, populated by the event handler.
    bot_pending: Option<BotPending>,

    // --- BOT-RESCUE: escalation state for the storage pipe ---
    /// M3 witness 6: the pending record of the stage that most recently FAILED, taken out of
    /// `bot_pending` by `run_bot_stage` on its way to reporting the error. Before this the record
    /// was taken and DROPPED before the error propagated, so `recover_bot_full` always read
    /// `bot_pending == None` and its `recover evidence` line printed `pipe=none wait_trb=0x0
    /// stage_done=no stage_cc=0` on every metal capture — a structural lie, not a finding.
    /// Consumed (taken) by `bot_transfer` and handed to recovery as a parameter.
    bot_failed: Option<BotPending>,
    /// Consecutive failed recovery+retry cycles on `storage_slot`. Reset to 0 by ANY transaction
    /// that completes (including one that completes with a `Failed` CSW — a device that answers is
    /// not a device that is wedged). Compared against `BOT_RESCUE_N_CONSEC`.
    bot_fail_streak: u32,
    /// Which escalation rungs have already been spent on the current streak: 0 = none, 1 = (a)
    /// Reset Device tried, 2 = (b) port power-cycle tried. Each rung fires at most once per streak,
    /// so a device that keeps failing walks a -> b -> surrender and never loops.
    bot_rescue_stage: u8,
    /// Slot the ladder has SURRENDERED on (0 = none). While set, `bot_transfer` refuses every
    /// transfer to that slot up front — the guarantee that a sick disk can never again spin the
    /// system at ~6 s per attempt forever. Cleared when the slot is disposed (disconnect) or
    /// re-enumerated, so a replug is a clean slate.
    bot_surrendered_slot: u8,
    /// Live multiplier on `hw_wait_budget()` for `pump_until_bot_done` — `BOT_BUDGET_SCALE_FIRST`
    /// at all times except inside an escalation retry, where it is briefly
    /// `BOT_BUDGET_SCALE_ESCALATION` and restored immediately after.
    bot_budget_scale: u64,

    /// In-flight synchronous EP0 control transfer (hub bring-up). See `Ep0Pending`.
    ep0_pending: Option<Ep0Pending>,
    /// XENUM-3 M1: bytes actually transferred by the most recent `sync_control` IN read (requested
    /// length minus the DATA-stage residual). Only meaningful immediately after a `sync_control`
    /// call returns Ok; the downstream enumerator reads it to detect a short descriptor read.
    last_control_len: u32,
    /// In-flight synchronous command (hub bring-up). See `CmdPending`.
    cmd_pending: Option<CmdPending>,
    /// Slots of hubs detected during enumeration, awaiting (main-loop) bring-up.
    hubs_pending: Vec<u8>,
    /// XENUM-2: (hub_slot, hub_port) pairs a hub's Status Change Endpoint flagged as changed,
    /// awaiting (main-loop, synchronous) servicing — GET_PORT_STATUS + downstream reset/enumerate
    /// (connect) or route-scoped teardown (disconnect). Queued by the transfer-event dispatch (which
    /// only decodes the bitmap + re-arms the read — never runs control transfers), drained by
    /// `service_hub_changes`. Bounded per wake (`HUB_CHANGE_BUDGET`).
    hub_changes_pending: Vec<(u8, u8)>,
    /// Slots whose HID interfaces are configured + reads armed, awaiting a (main-loop, synchronous)
    /// SET_PROTOCOL(boot). A boot-capable HID interface (bInterfaceSubClass 1) powers up in REPORT
    /// protocol, whose reports carry a report ID and a device-defined layout we don't parse; boot
    /// protocol is the fixed [buttons, dx, dy] / [mods, resv, keys..] the decoders expect. Deferred
    /// to the main loop because it is a synchronous EP0 transfer (must not run in the event handler).
    hid_setproto_pending: Vec<u8>,
    /// PIUSB-39 F1: HID interrupt-IN endpoints that completed with a HALTING error code and need
    /// the full un-halt sequence before they can be re-armed. `(slot_id, is_mouse)`. Deferred to
    /// the main loop for the same reason as `hid_setproto_pending`: the recovery is SYNCHRONOUS
    /// (Reset Endpoint + Set TR Dequeue command TRBs, then an EP0 CLEAR_FEATURE) and must never
    /// run re-entrantly inside the event-ring dispatch that noticed the error.
    hid_halt_pending: Vec<(u8, bool)>,
    /// True while a port is mid-enumeration. Enumeration is serialized (one port at a time);
    /// this lets a hot-plug Connect Status Change event know whether to kick `start_next_port`
    /// immediately or just queue the port (the in-flight device's completion will drain it).
    enum_active: bool,
    /// The root port currently being enumerated (0 = none). The USB reset we issue at the start of
    /// enumerating a port can itself assert CSC (Connect Status Change); without this the hot-plug
    /// handler would treat that as a fresh connect and re-queue the same port, double-enumerating it
    /// and starving the ports queued behind it. CSC for `enumerating_port` is a reset side-effect.
    enumerating_port: u8,
    /// USB protocol map of the root ports (Supported Protocol ext-caps, ID 2) — which ports are
    /// USB2 vs USB3. Empty if the controller exposes none (port types then unknown).
    port_protocols: Vec<PortProtocol>,

    /// Which step of root enumeration `enumerating_port` is at ("await-reset", "enable-slot",
    /// "address-device", "dev-desc", "cfg-desc", "configure-eps", "set-config", "idle").
    /// On the serial-less rMBP `enum_active=true` alone says "stuck somewhere"; this + `usbinfo`
    /// says WHERE, which is the difference between guessing and fixing.
    enum_stage: &'static str,
    /// `now_cycles()` when `enum_stage` was last changed — the watchdog's per-stage deadline base.
    enum_stage_set_at: u64,
    /// Physical address of the enum FSM's in-flight command TRB (ENABLE_SLOT / ADDRESS_DEVICE /
    /// Configure-Endpoint), 0 when none. Lets completion dispatch (and failure recovery) match
    /// the root FSM's own command exactly instead of guessing from shared state — the hub path's
    /// sync commands (`cmd_pending`) and any stale/late completions no longer alias.
    enum_cmd_phys: u64,
    /// Number of port resets issued for the current enumeration attempt (retry budget).
    enum_resets: u8,
    /// M1 (XENUM-1): set when `enumerating_port` posts a GENUINE disconnect (CSC with CCS=0)
    /// mid-flight. The USB reset we issue to enumerate a port itself asserts CSC, but with CCS
    /// staying 1 the whole time — so a CCS=0 edge is the unambiguous "the device physically left"
    /// signal that distinguishes a genuine hot re-plug from the self-induced reset artifact. Armed
    /// only by a real CCS=0 edge (never by the reset artifact), so re-queuing off it cannot loop.
    /// Cleared when a new port's enumeration begins.
    enum_saw_disconnect: bool,
    /// M1 (XENUM-1): ports to re-enumerate once the in-flight enumeration settles — a genuine
    /// connect that arrived while a port was mid-enumeration (could not be reset immediately).
    /// Drained into `ports_to_enumerate` at the top of `start_next_port`.
    requeue_after_settle: Vec<u8>,
    /// BOOTPACE M4: the ports queued by the INITIAL boot CCS scan in `start()`, as opposed to by a
    /// hot-plug CSC / unsolicited-reset event. These skip the 100 ms connect debounce: their
    /// connection predates port power and has additionally been held across the pre-scan settle, so
    /// TATTDB's "100 ms of stable attach before the reset" is already satisfied. An entry is
    /// consumed the moment `start_next_port` pops the port — so if that same port is later
    /// re-queued by a genuine (re)attach it is treated as a hot-plug and pays the full debounce,
    /// which is the metal-proven behaviour (the SD-reader FS-chirp failure) and must not be lost.
    boot_scan_ports: Vec<u8>,
    /// The most recent enumeration stall (for `usbinfo`): where a port's enumeration died.
    last_stall: Option<EnumStall>,
    /// Total enumeration stalls since boot.
    stall_count: u32,
    /// Slots torn down by enumeration recovery, awaiting a (main-loop, synchronous)
    /// DISABLE_SLOT — the command must not be issued from the event handler. Each entry is
    /// (slot id, failed attempts so far); retries are bounded so a wedged command ring can't
    /// turn every main-loop iteration into a multi-second sync-pump stall.
    slots_to_disable: Vec<(u8, u8)>,
    /// True while the command ring is stopped/stopping for an abort (`abort_enum_command`).
    /// send_command / run_command_sync refuse new work so nothing rings doorbell 0 mid-abort
    /// (a doorbell on a stopped ring restarts it AT the wedged TRB) — mirrors Linux's
    /// CMD_RING_STATE gating.
    cmd_ring_stopped: bool,
    /// ORIN-USB-FIX: physical addresses of in-flight JB9i inherited-slot-eviction DISABLE_SLOT
    /// TRBs (tegra no-HCRST takeover). Their completions are claimed early in the type-33
    /// dispatch: success (1) = a UEFI-owned slot reclaimed, code 11 (Slot Not Enabled) = the
    /// slot was never enabled — both EXPECTED, neither may print `>>> COMMAND FAILED <<<` nor
    /// fall into the untracked-completion branch (which re-queued a DISABLE_SLOT for the
    /// already-evicted slot on the R22 sitting-2 Orin boots). All-zero (inert) except during
    /// the tegra eviction window; non-tegra paths never populate it.
    pub evict_pending: [u64; 8],
    /// Per-root-port learned EP0 MPS for Full-Speed devices: false = first-guess 8, true = the
    /// port babbled at dev-desc (device's real bMaxPacketSize0 > 8) and retries use 64.
    fs_ep0_mps64: [bool; 32],
}

/// A recorded enumeration stall: which port died, at which stage, why (completion error /
/// watchdog timeout), the completion code if any, and the PORTSC snapshot at stall time.
#[derive(Clone, Copy)]
struct EnumStall {
    port: u8,
    stage: &'static str,
    why: &'static str,
    code: u8,
    portsc: u32,
}

unsafe impl Send for XhciController {}
unsafe impl Sync for XhciController {}

impl XhciController {
    pub unsafe fn new(base_addr: usize) -> Self {
        let cap_ptr = base_addr as *const u32;
        let cap_word = core::ptr::read_volatile(cap_ptr);

        let cap_length = (cap_word & 0xFF) as u8;
        let version = (cap_word >> 16) as u16;

        let op_base = base_addr + cap_length as usize;

        // Log it to verify we aren't seeing ghosts anymore
        serial_println!("xHCI: CapBase={:#x}, OpBase={:#x}, Version={:#x}", base_addr, op_base, version);

        // Read Max Slots and Max Ports from HCSPARAMS1
        let hcsparams1_ptr = (base_addr + 0x04) as *const u32;
        let hcsparams1 = core::ptr::read_volatile(hcsparams1_ptr);
        let max_slots = (hcsparams1 & 0xFF) as u8;
        let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;

        serial_println!("xHCI: MaxSlots={}, MaxPorts={}", max_slots, max_ports);

        let port_protocols = parse_supported_protocols(base_addr);

        let mut slots = Vec::new();
        for _ in 0..=max_slots {
            slots.push(DeviceSlot::new());
        }

        XhciController {
            base_addr,
            op_base,
            max_slots,
            max_ports,
            dcbaap: core::ptr::null_mut(),
            slots,
            pending_ports: Vec::new(),
            ports_to_enumerate: Vec::new(),
            configuring_slot: 0,
            event_ring_phys_base: 0,
            storage_slot: 0,
            storage_pending_bringup: false,
            ftdi_configuring_slot: 0,
            ftdi_slot: 0,
            ftdi_pending_bringup: false,
            ftdi_pending: None,
            ftdi_tx_total: 0,
            ftdi_pass_logged: false,
            ftdi_disabled_logged: false,
            storage_note: "no mass-storage device enumerated",
            bot_tag: 1,
            bot_pending: None,
            bot_failed: None,
            bot_fail_streak: 0,
            bot_rescue_stage: 0,
            bot_surrendered_slot: 0,
            bot_budget_scale: BOT_BUDGET_SCALE_FIRST,
            ep0_pending: None,
            last_control_len: 0,
            cmd_pending: None,
            hubs_pending: Vec::new(),
            hub_changes_pending: Vec::new(),
            hid_setproto_pending: Vec::new(),
            hid_halt_pending: Vec::new(),
            enum_active: false,
            enumerating_port: 0,
            enum_saw_disconnect: false,
            requeue_after_settle: Vec::new(),
            boot_scan_ports: Vec::new(),
            port_protocols,
            enum_stage: "idle",
            enum_stage_set_at: 0,
            enum_cmd_phys: 0,
            enum_resets: 0,
            last_stall: None,
            stall_count: 0,
            slots_to_disable: Vec::new(),
            cmd_ring_stopped: false,
            evict_pending: [0; 8],
            fs_ep0_mps64: [false; 32],
        }
    }

    /// Record an enumeration-stage transition (and its wall-clock timestamp, the watchdog's
    /// deadline base). One line of serial per step — on a metal bootlog photo this is the
    /// step-by-step trace of how far a port got.
    fn set_enum_stage(&mut self, stage: &'static str) {
        self.enum_stage = stage;
        self.enum_stage_set_at = crate::arch::now_cycles();
        if self.enumerating_port != 0 {
            serial_println!("xHCI: [enum port {}] stage -> {}", self.enumerating_port, stage);
        }
    }

    /// Track a command TRB just issued on behalf of the root enumeration FSM, so its
    /// completion (success OR failure) can be matched by address. Callers discriminate by
    /// CALL SITE, not by slot fields: hub-downstream slots carry the hub's ROOT port in
    /// port_id (address_downstream needs it for the slot context), so a port_id comparison
    /// cannot tell the hub path from the root FSM — `configure_hid_endpoints` takes an
    /// explicit `root_fsm` flag instead.
    fn track_enum_cmd(&mut self, phys: u64, stage: &'static str) {
        self.enum_cmd_phys = phys;
        self.set_enum_stage(stage);
    }

    /// USB protocol major revision of a root port: 3, 2, or 0 (unknown). From the Supported
    /// Protocol ext-caps; falls back to the live PORTSC speed field (SS speed IDs are >= 4) when
    /// the controller exposes no protocol capabilities — that fallback only identifies a USB3
    /// port while a SuperSpeed device is attached and trained, but it is better than nothing.
    fn port_major(&self, port_id: u8) -> u8 {
        for p in &self.port_protocols {
            if p.port_count > 0
                && port_id >= p.port_offset
                && (port_id as u32) < p.port_offset as u32 + p.port_count as u32
            {
                return p.major;
            }
        }
        if (self.read_portsc(port_id) >> 10) & 0xF >= 4 { 3 } else { 0 }
    }



    pub fn send_noop_command(&mut self) -> Result<usize, &'static str> {
        COMMAND_RING.lock().as_mut().unwrap().push_noop()
    }

    /// Push a command TRB and ring the host-controller doorbell. Returns the PHYSICAL address of
    /// the pushed TRB — the Command Completion event echoes it, so callers can match their own
    /// completion exactly (the root enumeration FSM tracks it in `enum_cmd_phys`; the sync hub
    /// path computes the same address in `run_command_sync`).
    /// XHCI-COHERENCE: the context-bearing commands (ADDRESS_DEVICE=11, CONFIGURE_ENDPOINT=12,
    /// EVALUATE_CONTEXT=13) carry an Input Context physical address in `parameter`; the controller
    /// DMA-reads that struct when it consumes the command. Clean it to DRAM here — ONE chokepoint
    /// for every context command, whichever builder produced it — so a non-snooping controller reads
    /// the freshly-written context. No-op on coherent x86_64.
    #[inline]
    fn clean_cmd_input_ctx(trb: &Trb) {
        let cmd_type = (trb.control >> 10) & 0x3F;
        if matches!(cmd_type, 11 | 12 | 13) && trb.parameter != 0 {
            dma_coherency::clean(trb.parameter as usize, core::mem::size_of::<InputContext>());
        }
    }

    pub fn send_command(&mut self, trb: Trb) -> Result<u64, &'static str> {
        if self.cmd_ring_stopped {
            return Err("command ring stopped (abort in progress)");
        }
        Self::clean_cmd_input_ctx(&trb);
        let phys = {
            let mut g = COMMAND_RING.lock();
            let ring = g.as_mut().ok_or("command ring not initialised")?;
            let base = ring.get_ptr();
            let idx = ring.push(trb)?;
            base + (idx as u64) * 16
        };
        // Ring the Doorbell for the Host Controller (Slot 0). Target 0 = Command Ring.
        self.ring_doorbell(0, 0);
        Ok(phys)
    }

    /// One-shot host-controller health snapshot for metal bring-up (gated to the usbdebug build):
    /// the first ENABLE_SLOT command froze on the real 2012 rMBP — port (event-ring) events arrive
    /// but the command completion never does — so surface the load-bearing registers to tell a
    /// command-ring/doorbell fault (HCE/HSE/CRR) apart from an event-delivery stall. All reads, no
    /// side effects.
    #[cfg(feature = "usbdebug")]
    pub fn dump_hc_health(&self, label: &str) {
        unsafe {
            let usbcmd = core::ptr::read_volatile(self.op_base as *const u32);
            let usbsts = core::ptr::read_volatile((self.op_base + 0x04) as *const u32);
            let crcr = core::ptr::read_volatile((self.op_base + 0x18) as *const u32);
            let dboff = core::ptr::read_volatile((self.base_addr + 0x14) as *const u32) & !0x3;
            serial_println!(
                "xHCI HEALTH [{}]: USBCMD={:#x}(RS={} INTE={}) USBSTS={:#x}(HCH={} HSE={} EINT={} CNR={} HCE={}) CRCR.CRR={} DBOFF={:#x}",
                label, usbcmd, usbcmd & 1, (usbcmd >> 2) & 1,
                usbsts, usbsts & 1, (usbsts >> 2) & 1, (usbsts >> 3) & 1, (usbsts >> 11) & 1, (usbsts >> 12) & 1,
                (crcr >> 3) & 1, dboff
            );
            let ir0 = XHCI_IR0_BASE.load(Ordering::Acquire);
            if ir0 != 0 {
                let iman = core::ptr::read_volatile(ir0 as *const u32);
                let erdp = core::ptr::read_volatile((ir0 + 0x18) as *const u32);
                serial_println!(
                    "xHCI HEALTH [{}]: IMAN={:#x}(IP={} IE={}) ERDP_lo={:#x}(EHB={})",
                    label, iman, iman & 1, (iman >> 1) & 1, erdp, (erdp >> 3) & 1
                );
            }
        }
    }

    fn read_portsc(&self, port_id: u8) -> u32 {
        unsafe {
            let port_offset = 0x400 + (port_id as usize - 1) * 0x10;
            let portsc_ptr = (self.op_base + port_offset) as *const u32;
            core::ptr::read_volatile(portsc_ptr)
        }
    }

    fn write_portsc(&self, port_id: u8, val: u32) {
        unsafe {
            let port_offset = 0x400 + (port_id as usize - 1) * 0x10;
            let portsc_ptr = (self.op_base + port_offset) as *mut u32;
            core::ptr::write_volatile(portsc_ptr, val);
        }
    }

    /// ONSET-2 (M2 witness 1): the **port register census** — PORTSC with its Port Link State
    /// decoded, PORTPMSC and PORTLI, for every port that reports a device connected.
    ///
    /// **Why this is the highest-value line in the arc.** No capture has ever recorded any port
    /// state at a BOT timeout, which leaves two whole hypothesis families untestable: USB 2.0 link
    /// power management left armed by firmware (the failing device is on a **USB 2.0** root port, so
    /// the relevant LPM is **L1 / PORTPMSC**, not USB 3's U1/U2), and PCH port-mux routing residue
    /// from the XUSB2PR/USB3_PSSEN writes the driver makes at bring-up. Both can be killed or
    /// confirmed by a handful of volatile reads. This function writes nothing: PORTSC has write-1-to
    /// -disable (PED) and write-1-to-reset (PR) semantics and every RW1C change bit besides, so a
    /// witness that only ever reads cannot perturb what it is measuring.
    ///
    /// Printed once at bring-up as the **baseline** the instrument-baseline law requires, and again
    /// on every `TIMEOUT-PIPES`. A register reading with nothing to compare it against cannot
    /// falsify anything — that is how `IMAN=0x3` sat in captures for weeks looking like evidence
    /// until the healthy line was printed beside it and read identically.
    ///
    /// HEALTHY-BUT-IDLE READING, stated per field because that is the rule:
    ///   * `pls=0(U0)` — the link is up and active. **This is the reading that kills the LPM
    ///     hypothesis** if it holds across a timeout. `pls=2(U2)` or `pls=3(U3)` at a timeout on a
    ///     USB 2.0 port would mean the link went to sleep under a driver that programs no LPM state
    ///     and would never bring it back.
    ///   * `ccs=1 ped=1 pp=1 pr=0 oca=0 cas=0` — connected, enabled, powered, not resetting, no
    ///     overcurrent, no Cold Attach Status.
    ///   * `pmsc=0x0` with `l1s=0(invalid)` and `hle=0` — no L1 transaction has ever been attempted
    ///     and hardware LPM is not enabled. Anything else on a port this driver never programmed
    ///     means **firmware** armed it, which is the whole hypothesis.
    ///   * `li=0x0` — PORTLI is the USB 3 Link Error Count and is **reserved on a USB 2.0 port**, so
    ///     0 there is definitional, not a signal. It is printed to prove the register decode and to
    ///     be non-empty on a USB 3 port, and no verdict may ever be taken from it on a USB 2 port.
    ///   * change bits (`csc/pec/prc/plc/cec`) all 0 — nothing has happened to the port since it was
    ///     last acknowledged. A `plc=1` at a timeout would be a Port Link State Change nobody
    ///     consumed, which is the single most interesting thing this line could say.
    fn port_link_witness(&self, why: &str) {
        for port in 1..=self.max_ports {
            let base = self.op_base + 0x400 + (port as usize - 1) * 0x10;
            let (portsc, pmsc, li) = unsafe {
                (core::ptr::read_volatile(base as *const u32),
                 core::ptr::read_volatile((base + 0x04) as *const u32),
                 core::ptr::read_volatile((base + 0x08) as *const u32))
            };
            if portsc & 1 == 0 {
                continue; // CCS clear: nothing attached, nothing to say
            }
            let pls = (portsc >> 5) & 0xF;
            // xHCI 1.2 §5.4.8 Table 5-27.
            let pls_name = match pls {
                0 => "U0", 1 => "U1", 2 => "U2", 3 => "U3-suspend", 4 => "Disabled",
                5 => "RxDetect", 6 => "Inactive", 7 => "Polling", 8 => "Recovery",
                9 => "HotReset", 10 => "Compliance", 11 => "TestMode", 15 => "Resume",
                _ => "reserved",
            };
            // PORTPMSC in its USB 2.0 layout (§5.4.9.1): L1S 2:0, RWE 3, BESL/HIRD 7:4,
            // L1 Device Slot 15:8, HLE 16. On a USB 3 port the same offset is a different register
            // (U1/U2 timeouts), which is why the major version is on the line.
            let l1s = pmsc & 0x7;
            let l1s_name = match l1s {
                0 => "invalid", 1 => "success", 2 => "not-yet", 3 => "not-supported",
                4 => "timeout-error", _ => "reserved",
            };
            serial_println!(
                ":: BOT: portreg why={} port={} usb{} portsc={:#010x} ccs={} ped={} oca={} pr={} pls={}({}) pp={} speed={} cas={} csc={} pec={} prc={} plc={} cec={} pmsc={:#010x} l1s={}({}) rwe={} hird={} l1slot={} hle={} li={:#010x} result=PORTREG ::",
                why, port, self.port_major(port), portsc,
                portsc & 1, (portsc >> 1) & 1, (portsc >> 3) & 1, (portsc >> 4) & 1,
                pls, pls_name, (portsc >> 9) & 1, (portsc >> 10) & 0xF, (portsc >> 24) & 1,
                (portsc >> 17) & 1, (portsc >> 18) & 1, (portsc >> 21) & 1,
                (portsc >> 22) & 1, (portsc >> 23) & 1,
                pmsc, l1s, l1s_name, (pmsc >> 3) & 1, (pmsc >> 4) & 0xF,
                (pmsc >> 8) & 0xFF, (pmsc >> 16) & 1, li);
        }
    }

    /// Safely clear one or more PORTSC change bits (all RW1C). PORTSC has dangerous
    /// write-1 semantics: bit 1 (PED) is write-1-to-DISABLE and bit 4 (PR) is
    /// write-1-to-RESET. A naive `read | change_bit` write-back can therefore disable
    /// or reset the port if those bits read back as 1. This masks off PED, PR, and all
    /// RW1C change bits, then writes 1 only to the requested change bit(s).
    fn clear_port_change(&self, port_id: u8, change_bits: u32) {
        let portsc = self.read_portsc(port_id);
        let preserved = portsc & !(PORT_CHANGE_BITS | (1 << 1) | (1 << 4));
        self.write_portsc(port_id, preserved | (change_bits & PORT_CHANGE_BITS));
    }

    pub fn ring_doorbell(&mut self, slot_id: u8, target: u32) {
        unsafe {
            // 1. Find Doorbell Offset (Offset 0x14 in Cap Regs)
            let dboff_ptr = (self.base_addr + 0x14) as *const u32;
            let dboff = core::ptr::read_volatile(dboff_ptr) & !0x03; // 4-byte aligned

            // 2. Doorbell Register 0 is at Base + DBOFF
            // Each doorbell is 32-bits. Register index is the Slot ID.
            // Slot ID 0 is always the Command Ring.
            let db_addr = self.base_addr + dboff as usize + (slot_id as usize * 4);
            let db_ptr = db_addr as *mut u32;

            // 3. Write the Target using ASM
            // We bypass standard write to ensure ordering.
            ring_doorbell_asm(db_addr as u64, target);

            // DEBUG: DOORBELL ADDRESS VERIFICATION
            xdbg!("xHCI DEBUG: DBOFF Register = {:#x}", core::ptr::read_volatile(dboff_ptr));
            xdbg!("xHCI DEBUG: Calculated DB[0] Addr = {:#x}", self.base_addr + dboff as usize);
            xdbg!("xHCI DEBUG: Actual Write Addr    = {:#x}", db_ptr as usize);

            xdbg!("xHCI: DOORBELL RUNG (Slot {}, Target {}).", slot_id, target);
        }
    }

    pub fn poll_events(&mut self) -> bool {
        let mut any = false;
        while self.drain_event_ring_once() {
            any = true;
        }
        any
    }

    /// Pop and dispatch a single event TRB, then advance the ERDP. Returns false when
    /// the event ring is empty. This is the SINGLE entry point for consuming events —
    /// used by both poll_events() and the synchronous BOT pump — so there is exactly one
    /// ERDP owner and the EVENT_RING lock is never held across dispatch.
    fn drain_event_ring_once(&mut self) -> bool {
        let (trb, dequeue_index) = {
            let mut guard = EVENT_RING.lock();
            let ring = guard.as_mut().unwrap();
            if !ring.has_event() {
                return false;
            }
            let trb = ring.pop().unwrap();
            (trb, ring.dequeue_index)
        }; // EVENT_RING lock released BEFORE dispatch

        xdbg!("xHCI: Event Detected!");
        self.handle_event_trb(trb);
        self.advance_erdp(dequeue_index);
        true
    }

    /// Update the Event Ring Dequeue Pointer to `dequeue_index`, clearing Event Handler Busy.
    fn advance_erdp(&self, dequeue_index: usize) {
        unsafe {
            if EVENT_RING_PHYS_BASE == 0 {
                serial_println!("xHCI: WARNING - EVENT_RING_PHYS_BASE is 0, skipping ERDP update!");
                return;
            }
            let rtsoff = core::ptr::read_volatile((self.base_addr + 0x18) as *const u32) & !0x1F;
            let ir0_base = self.base_addr + rtsoff as usize + 0x20;

            // Acknowledge the interrupter: clear IMAN.IP (bit 0, RW1C) and USBSTS.EINT
            // (bit 3, RW1C). QEMU's xHC will not post the next event until the prior
            // Interrupt Pending is acknowledged, so a tight poll loop can otherwise stall
            // after one event even though the transfer completed.
            let iman = core::ptr::read_volatile(ir0_base as *const u32);
            core::ptr::write_volatile(ir0_base as *mut u32, iman | 1);
            core::ptr::write_volatile((self.op_base + 0x04) as *mut u32, 1 << 3);

            let new_dequeue_ptr = EVENT_RING_PHYS_BASE + (dequeue_index as u64 * 16);
            // Bit 3 (EHB) is write-1-to-clear. High-dword-first (write_erdp) so the
            // controller never latches a torn pointer when PIUSB-21 forces a real 32-bit
            // split on the brcmstb RC — see write_erdp (XHCI-INT).
            write_erdp((ir0_base + 0x18) as *mut u64, new_dequeue_ptr | 8);
            xdbg!("[xhciint] ERDP advanced to {:#x} (EHB cleared, hi-first)", new_dequeue_ptr);
        }
    }

    /// Dispatch a single event TRB (command completion / port status change / transfer).
    fn handle_event_trb(&mut self, trb: Trb) {
        let param = trb.parameter;
        let status = trb.status;
        let control = trb.control;

        xdbg!("xHCI RAW: Param={:#x} Status={:#x} Control={:#x}", param, status, control);

        // Control Field: Bits 15:10 = TRB Type
        let trb_type = (control >> 10) & 0x3F;

                // 2. DISPATCH
                match trb_type {
                    33 => { // COMMAND COMPLETION EVENT
                        let command_ptr = param;
                        let completion_code = (status >> 24) & 0xFF;
                        let slot_id = (control >> 24) & 0xFF;

                        // Bounds: slot_id comes from the controller. A confused/flaky xHC
                        // (exactly what recovery runs against) must not be able to panic the
                        // kernel with an out-of-range slot index.
                        if slot_id as usize >= self.slots.len() {
                            serial_println!("xHCI: command completion with bogus slot {}; ignoring.", slot_id);
                            return;
                        }

                        // Code 24 = Command Ring Stopped (xHCI Table 6-90 — NOT "aborted";
                        // 25 is Command Aborted). A pure ring-state signal: its param is the
                        // ring DEQUEUE pointer, not any command's TRB, so it must never be
                        // matched against pending commands (it can coincide with a freshly
                        // pushed TRB's address). The abort machinery owns the restart.
                        if completion_code == 24 {
                            serial_println!("xHCI: [Event] Command Ring Stopped (dequeue={:#x}).", command_ptr);
                            return;
                        }

                        // ORIN-USB-FIX: a JB9i eviction DISABLE_SLOT claims its own completion
                        // here, matched by TRB address. Both success (slot reclaimed) and code
                        // 11 (Slot Not Enabled — the slot was never in use) are the expected
                        // outcomes of evicting slots 1..8 blind; consume them quietly so they
                        // neither alarm (`COMMAND FAILED`) nor mis-queue a redundant
                        // DISABLE_SLOT via the untracked-completion branch below.
                        if self.evict_pending.iter().any(|&p| p != 0 && p == command_ptr) {
                            for p in self.evict_pending.iter_mut() {
                                if *p == command_ptr {
                                    *p = 0;
                                }
                            }
                            serial_println!(
                                "xHCI: JB9i eviction completion: slot {} code {} ({}).",
                                slot_id, completion_code,
                                match completion_code {
                                    1 => "reclaimed",
                                    11 => "was not enabled — fine",
                                    _ => "tolerated",
                                });
                            return;
                        }

                        // A synchronous command (hub bring-up) claims its OWN completion here,
                        // matched by command-TRB address, before the async enumeration FSM below.
                        // Inert (None) during normal enumeration, so that path is untouched.
                        if let Some(p) = self.cmd_pending {
                            if command_ptr == p.cmd_trb_phys {
                                if let Some(cp) = self.cmd_pending.as_mut() {
                                    cp.completion_code = completion_code as u8;
                                    cp.slot_id = slot_id as u8;
                                    cp.done = true;
                                }
                                return; // consumed by the sync command pump
                            }
                        }

                        serial_println!("xHCI: [Event] Command Completion. Ptr={:#x}, Slot={}, Code={}",
                            command_ptr, slot_id, completion_code);

                        // Does this completion answer the root enumeration FSM's own in-flight
                        // command? Matched by TRB address — a hub-path async Configure-Endpoint
                        // or a stale/late completion can no longer alias into the FSM's
                        // positional guesswork (pop-a-pending-port / assume-address-finished).
                        let ours = self.enum_cmd_phys != 0 && command_ptr == self.enum_cmd_phys;
                        if ours {
                            self.enum_cmd_phys = 0; // consumed (success or failure)
                        }

                        // Completion Code 1 = Success
                        if completion_code == 1 {
                            serial_println!("xHCI: >>> COMMAND SUCCESS <<<");
                            if slot_id > 0 {
                                serial_println!("xHCI: SLOT ID ALLOCATED: {}", slot_id);

                                // UNA-18-ADDRESS: our ENABLE_SLOT completed — the controller
                                // allocated `slot_id` for the port we are enumerating.
                                // Proceed to Address Device.
                                if ours && !self.pending_ports.is_empty() {
                                    let port_to_map = self.pending_ports.pop().unwrap();
                                    serial_println!("xHCI: Proceeding to Address Device (Slot {}, Port {})...", slot_id, port_to_map);
                                    self.address_device(slot_id as u8, port_to_map);
                                }
                                // UNA-21-ACCELERATE: Check if we were configuring endpoints
                                else if self.configuring_slot == slot_id as u8 {
                                    serial_println!("xHCI: Endpoints Configured (Slot {}). Storage ready.", slot_id);
                                    self.configuring_slot = 0;
                                    // Cache the storage slot and defer the SCSI bring-up + read
                                    // to the main loop (a safe, non-event context where the
                                    // synchronous BOT pump can run without re-entrancy).
                                    self.storage_slot = slot_id as u8;
                                    self.storage_pending_bringup = true;
                                    self.storage_note = "endpoints configured; SCSI bring-up pending";
                                    // Storage setup is done; move on to the next connected port.
                                    self.start_next_port();
                                }
                                // U2.5: the FTDI's Configure-Endpoint completed. Cache the slot and
                                // defer SET_CONFIGURATION + the FTDI vendor setup to the main loop
                                // (a safe, non-event context, like storage). Kept in its OWN field
                                // so this is not misclaimed by the storage branch above or the
                                // address-device (`ours`) branch below.
                                else if self.ftdi_configuring_slot == slot_id as u8 {
                                    serial_println!("xHCI: FTDI Endpoints Configured (Slot {}). Console bring-up pending.", slot_id);
                                    self.ftdi_configuring_slot = 0;
                                    self.ftdi_slot = slot_id as u8;
                                    self.ftdi_pending_bringup = true;
                                    self.start_next_port();
                                }
                                else if self.slots[slot_id as usize].mouse_state == 1
                                    || self.slots[slot_id as usize].keyboard_state == 1 {
                                    // The single Configure-Endpoint that programmed this device's HID
                                    // interface(s) completed (root FSM or hub bring-up — keyed on the
                                    // slot's own state, not on enum_cmd_phys, so the hub path keeps
                                    // working). Advance whichever endpoints it covered, then issue
                                    // ONE device-level SET_CONFIGURATION for the whole device.
                                    serial_println!("xHCI: HID Endpoints Configured (Slot {}). Proceeding to Set Configuration...", slot_id);
                                    if self.slots[slot_id as usize].keyboard_state == 1 {
                                        self.slots[slot_id as usize].keyboard_state = 2;
                                    }
                                    if self.slots[slot_id as usize].mouse_state == 1 {
                                        self.slots[slot_id as usize].mouse_state = 2;
                                    }
                                    if ours {
                                        // Only the root FSM's own Configure-Endpoint advances
                                        // the enum stage; a hub-downstream device's must not.
                                        self.enum_cmd_phys = 0;
                                        self.set_enum_stage("set-config");
                                    }
                                    self.send_set_configuration(slot_id as u8, 1);
                                }
                                else if ours {
                                    // UNA-19-IDENTITY: our ADDRESS_DEVICE finished. Guard against a
                                    // slot recovery already tore down (late completion): touching a
                                    // dead slot's EP0 ring would be use-after-dispose.
                                    if self.slots[slot_id as usize].active
                                        && self.slots[slot_id as usize].ep0_ring.is_some() {
                                        serial_println!("xHCI: >>> SLOT {} ENABLED & ADDRESSED <<<", slot_id);
                                        self.begin_device_descriptor(slot_id as u8);
                                    } else {
                                        serial_println!("xHCI: completion for disposed slot {}; ignoring.", slot_id);
                                    }
                                }
                                else {
                                    // Not our tracked command, no state machine claims it: a stale
                                    // completion (e.g. from a port recovery already gave up on).
                                    // Before enum_cmd_phys existed this fell into the positional
                                    // dispatch above and corrupted the FSM. A successful stale
                                    // ENABLE_SLOT still allocated a slot nothing references —
                                    // dispose it, or retries slowly drain the MaxSlots pool.
                                    if !self.slots[slot_id as usize].active {
                                        serial_println!(
                                            "xHCI: untracked completion allocated slot {}; queueing DISABLE_SLOT.",
                                            slot_id);
                                        if !self.slots_to_disable.iter().any(|(s, _)| *s == slot_id as u8) {
                                            self.slots_to_disable.push((slot_id as u8, 0));
                                        }
                                    } else {
                                        serial_println!("xHCI: untracked command completion (slot {}); ignoring.", slot_id);
                                    }
                                }
                            }
                        } else {
                            serial_println!("xHCI: >>> COMMAND FAILED (Code {}) <<<", completion_code);
                            // UNA-19-HALT: Stop on Code 5
                            if completion_code == 5 {
                                serial_println!("xHCI: CRITICAL FAILURE: TRB ERROR (CODE 5).");
                            }
                            // THE deadlock fix: a failed enum command — ADDRESS_DEVICE completing
                            // with USB Transaction Error (4) on silicon, a watchdog-aborted
                            // command (25 = Command Aborted, param = the aborted TRB), ... — used
                            // to be logged and dropped, leaving enum_active=true and every queued
                            // port starved — the exact state photographed on the rMBP. Recover.
                            if ours {
                                self.recover_enumeration("command-failed", completion_code as u8);
                            }
                        }
                    },
                    34 => { // PORT STATUS CHANGE EVENT
                        let port_id = ((param >> 24) & 0xFF) as u8;
                        serial_println!("xHCI: [Event] Port Status Change. Port={}", port_id);
                        self.handle_port_status(port_id);
                    },
                    32 => { // TRANSFER EVENT
                        let transfer_len = status & 0xFFFFFF;
                        let completion_code = (status >> 24) & 0xFF;
                        let slot_id = (control >> 24) & 0xFF; // Slot ID is in Control Bits 31:24
                        let endpoint_id = (control >> 16) & 0x1F; // Endpoint ID in Control Bits 16:20

                        xdbg!("xHCI DEBUG: [Transfer Event] Slot={}, EP={}, Code={}, Len={}",
                            slot_id, endpoint_id, completion_code, transfer_len);

                        // Bounds: slot_id comes from the controller (see the type-33 guard).
                        if slot_id as usize >= self.slots.len() {
                            serial_println!("xHCI: transfer event with bogus slot {}; ignoring.", slot_id);
                            return;
                        }

                        // BOT-RESCUE M3 witness 4: count Transfer Events for OTHER slots while a BOT
                        // stage is waiting (see BOT_FOREIGN_EVENTS). One relaxed increment on a
                        // path that is already doing MMIO and DMA reads; no dispatch decision is
                        // taken from it, so event routing is byte-unchanged.
                        if let Some(p) = self.bot_pending {
                            if p.slot_id != slot_id as u8 {
                                BOT_FOREIGN_EVENTS.fetch_add(1, Ordering::Relaxed);
                            }
                        }

                        // ONSET-2 (M2 witness 3): census by completion code, naming 26 and 27.
                        //
                        // xHCI 1.2 §4.6.9: a Stop Endpoint issued against an endpoint with a TD IN
                        // PROGRESS must post a Transfer Event for the interrupted TD with completion
                        // code 26 (Stopped) or 27 (Stopped — Length Invalid). That is the
                        // architectural discriminator between "the controller never fetched the
                        // work" and "the controller fetched it and the device is NAKing" — the last
                        // ambiguity in the onset reading, which the driver could never speak to
                        // because it never printed stopped-events by name.
                        //
                        // ONSET-3 SHARPENS THE READING, and it is a narrowing, not a widening. The
                        // gr9 capture (boot 4) posts cc=27 on the IN pipe of a recovery whose own
                        // strand scan reads `gap=0 live=0` with the CSW not yet pushed — an endpoint
                        // that was Running but IDLE. So **27 alone does not prove a TD was in
                        // progress**: §6.4.5 defines it as "the TRB Transfer Length field is
                        // invalid", which is exactly what a controller reports when it is stopped at
                        // a position with no computable residual — including an un-produced slot.
                        // Only **cc=26, whose length IS valid**, carries "a TD was interrupted", and
                        // only when read together with the post-stop TR Dequeue Pointer that names
                        // WHICH TRB. Count them separately (as below) and never sum them.
                        //
                        // Boot totals; the reading that means anything is the DELTA `resync_bulk_ep`
                        // prints across its own Stop/Reset Endpoint. Three relaxed adds on a path
                        // that is already doing MMIO and DMA reads; no dispatch decision is taken
                        // from any of them, so event routing is byte-unchanged.
                        BOT_EV_ANY.fetch_add(1, Ordering::Relaxed);
                        match completion_code {
                            26 => {
                                BOT_EV_STOPPED.fetch_add(1, Ordering::Relaxed);
                                // ONSET-3: LATCH THE PAYLOAD, not just the arrival. cc=26 is the one
                                // completion code whose TRB Transfer Length is defined valid, and for
                                // a Stopped event that length is the RESIDUE of the interrupted TD —
                                // the bytes that had not moved. See `BOT_STOPEV_N` for why this
                                // cannot ride `BotPending::residue` (it is `None` by the time a
                                // recovery's Stop Endpoint posts) and for the reading key.
                                //
                                // Last-writer-wins is correct here: a recovery drains the event ring
                                // and then prints, so the value read on the `resync stopev` line is
                                // the most recent event of that window. `BOT_STOPEV_N` is
                                // incremented LAST so any reader that sees a fresh count also sees
                                // the three fields that go with it.
                                BOT_STOPEV_DCI.store(endpoint_id as u32, Ordering::Relaxed);
                                BOT_STOPEV_TRB.store(param, Ordering::Relaxed);
                                BOT_STOPEV_RES.store(transfer_len, Ordering::Relaxed);
                                BOT_STOPEV_N.fetch_add(1, Ordering::Relaxed);
                            }
                            27 => { BOT_EV_STOPPED_LI.fetch_add(1, Ordering::Relaxed); }
                            _ => {}
                        }

                        // Synchronous EP0 control transfer (hub bring-up) claims its OWN Status-TRB
                        // completion here, before the async descriptor FSM below — matched by TRB
                        // address (or any error), so it never disturbs other slots' enumeration.
                        if endpoint_id == 1 && slot_id > 0 {
                            if let Some(p) = self.ep0_pending {
                                if p.slot_id == slot_id as u8 {
                                    let is_match = param == p.wait_trb_phys;
                                    let is_error = completion_code != 1 && completion_code != 13;
                                    // Only consume the event if it is actually ours (the awaited
                                    // Status TRB, or an error). A same-slot non-matching success
                                    // must fall through to the async FSM rather than be dropped.
                                    if is_match || is_error {
                                        if let Some(ep) = self.ep0_pending.as_mut() {
                                            ep.completion_code = completion_code as u8;
                                            ep.done = true;
                                        }
                                        return; // consumed by the sync EP0 pump
                                    }
                                    // XENUM-3 M1: this sync transfer's DATA stage, matched by the
                                    // DATA TRB's physical address (like wait_trb_phys for Status)
                                    // AND first-write latched on data_seen. Both guards defend
                                    // against Panther Point's XHCI_SPURIOUS_SUCCESS quirk (device
                                    // 0x1e31): a duplicate Success after a Short Packet for the same
                                    // TD would otherwise overwrite a real short-read residual with 0,
                                    // last_control_len would read full, and the zeroed-descriptor
                                    // strand M1 exists to catch would go undetected on the exact
                                    // target hardware. Record the residual (TRB Transfer Length
                                    // remaining) so the enumerator learns the ACTUAL transferred
                                    // length, then consume the event — previously it fell through to
                                    // the async FSM and was dropped as "stale/spurious". A matching
                                    // dup (param == data_trb_phys, data_seen already set) is consumed
                                    // without recording; QEMU posts no dup, so gates are
                                    // no-regression only for the latch.
                                    if (completion_code == 1 || completion_code == 13)
                                        && p.data_trb_phys != 0 && param == p.data_trb_phys
                                    {
                                        if let Some(ep) = self.ep0_pending.as_mut() {
                                            if !ep.data_seen {
                                                ep.data_residual = transfer_len;
                                                ep.data_seen = true;
                                            }
                                        }
                                        return; // DATA stage claimed by the sync EP0 pump
                                    }
                                }
                            }
                        }

                        // Bulk-Only Transport routing: if a BOT transaction is in flight on this
                        // slot's bulk endpoints, hand the completion to the synchronous pump.
                        //
                        // BOT-PHASE fix 4 — DE-ALIASING. This claim used to be: match the awaited
                        // TRB address, OR claim ANY error completion on either bulk DCI. The second
                        // half is a blanket claim over a slot's whole bulk traffic, and TRB
                        // addresses recur (16-TRB rings, three pushes per transaction — an address
                        // repeats every ~5 transactions), so between them a STALE event for a
                        // long-retired TD could retire the LIVE stage with someone else's
                        // completion code. Two narrowings, both minimal and both provable from the
                        // event's own fields:
                        //   1. The blanket error claim is gone. An error that names a TRB is now
                        //      matched by address like any other event — a bulk STALL carries its
                        //      TRB pointer, so the property the blanket claim was added for (a
                        //      stalled command must not burn the full pump timeout) is preserved.
                        //      The fallback survives ONLY for an error whose pointer addresses
                        //      nothing in either of this slot's bulk rings — Ring Underrun (21),
                        //      Ring Overrun (22) and VF Event Ring Full (either post no TRB pointer
                        //      or a meaningless one), where "it can only be ours" is the sole
                        //      available attribution. That fallback is COUNTED (`BOT_EV_UNADDRESSED`)
                        //      rather than silent.
                        //   2. First-write latch on `done`. A second event for an already-completed
                        //      stage is refused, not allowed to overwrite the recorded completion
                        //      code, and is counted (`BOT_EV_LATE_CLAIM`).
                        if endpoint_id > 1 && slot_id > 0 {
                            if let Some(p) = self.bot_pending {
                                if p.slot_id == slot_id as u8
                                    && (endpoint_id as u8 == p.in_dci || endpoint_id as u8 == p.out_dci)
                                {
                                    let is_match = param == p.wait_trb_phys;
                                    let is_error = completion_code != 1 && completion_code != 13;
                                    // Does this event name a TRB in either of this slot's bulk
                                    // rings? If it names one and it is not ours, it belongs to a
                                    // retired TD and must not touch the live stage.
                                    let addressed = {
                                        let s = &self.slots[slot_id as usize];
                                        s.bulk_in_ring.as_ref().is_some_and(|r| r.contains(param))
                                            || s.bulk_out_ring.as_ref().is_some_and(|r| r.contains(param))
                                    };
                                    let unaddressed_error = is_error && !addressed;
                                    if is_match || unaddressed_error {
                                        if p.done {
                                            // Fix 4 (2): the stage already has its completion.
                                            BOT_EV_LATE_CLAIM.fetch_add(1, Ordering::Relaxed);
                                            return; // refused, not overwritten
                                        }
                                        if unaddressed_error {
                                            BOT_EV_UNADDRESSED.fetch_add(1, Ordering::Relaxed);
                                        }
                                        if let Some(bp) = self.bot_pending.as_mut() {
                                            bp.completion_code = completion_code as u8;
                                            // Fix 3: carry the residue out of the event instead of
                                            // discarding it. Latched on first write.
                                            if !bp.residue_seen {
                                                bp.residue = transfer_len;
                                                bp.residue_seen = true;
                                            }
                                            bp.done = true;
                                        }
                                        return; // consumed by the BOT pump
                                    }
                                }
                            }
                        }

                        // U2.5: FTDI console bulk-OUT completion. The FTDI slot is distinct from the
                        // storage slot, so slot_id + out_dci disambiguate it from any BOT transfer.
                        // Matched by TRB address (or any error) so the drain pump claims exactly its
                        // own Normal-TRB completion before the async FSM below.
                        if endpoint_id > 1 && slot_id > 0 {
                            if let Some(p) = self.ftdi_pending {
                                if p.slot_id == slot_id as u8 && endpoint_id as u8 == p.out_dci {
                                    let is_match = param == p.wait_trb_phys;
                                    let is_error = completion_code != 1 && completion_code != 13;
                                    if is_match || is_error {
                                        if let Some(fp) = self.ftdi_pending.as_mut() {
                                            fp.completion_code = completion_code as u8;
                                            fp.done = true;
                                        }
                                        return; // consumed by the FTDI TX pump
                                    }
                                }
                            }
                        }

                        // An EP0 transfer for the port being ENUMERATED failed (STALL on a
                        // descriptor fetch / SET_CONFIGURATION, babble, transaction error...).
                        // This was silently dropped before — the enumeration FSM then deadlocked
                        // exactly like a failed command. Recover (retry the port, else advance).
                        // Downstream (behind-hub) slots share the root port number but belong to
                        // the hub path, not the root FSM — never trip root recovery for them.
                        if completion_code != 1 && completion_code != 13
                            && endpoint_id == 1 && slot_id > 0
                            && self.enumerating_port != 0
                            && self.slots[slot_id as usize].port_id == self.enumerating_port
                            && !self.slots[slot_id as usize].is_downstream
                        {
                            serial_println!(
                                "xHCI: EP0 transfer FAILED (slot {}, code {}) during enumeration.",
                                slot_id, completion_code);
                            // Babble (code 3) on EP0 during enumeration = the device's real
                            // bMaxPacketSize0 exceeds the context's. Only Full-Speed has an
                            // ambiguous MPS0 — learn 64 for this port so the retry addresses
                            // with the corrected context (see the mps0 match in address_device).
                            if completion_code == 3 {
                                let p = (self.enumerating_port as usize) & 31;
                                if !self.fs_ep0_mps64[p] {
                                    self.fs_ep0_mps64[p] = true;
                                    serial_println!(
                                        "xHCI: EP0 babble on port {} -> retrying with FS MPS0=64.",
                                        self.enumerating_port);
                                }
                            }
                            self.recover_enumeration("ep0-transfer-failed", completion_code as u8);
                            return;
                        }

                        // XENUM-2 review fold: an ERROR completion (STALL / txn error / ...) on a
                        // hub's Status Change Endpoint would otherwise fall through the success gate
                        // below and the read would never be re-armed — hot-plug on that hub silently
                        // dead until reboot. Trace + re-arm (the TransferRing recycles; CErr=3 lets
                        // the controller retry transient errors itself, so a persistent error here
                        // is rare — the bounded ring keeps a wedged endpoint from looping the CPU).
                        if completion_code != 1 && completion_code != 13
                            && endpoint_id > 1 && slot_id > 0
                            && (slot_id as usize) < self.slots.len()
                        {
                            let s = &self.slots[slot_id as usize];
                            if s.is_hub && s.hub_int_ep != 0
                                && endpoint_id as u8 == (s.hub_int_ep & 0x0F) * 2 + 1
                            {
                                serial_println!(
                                    "xHCI: HUB slot {} status-change read error (code {}); re-arming.",
                                    slot_id, completion_code);
                                self.slots[slot_id as usize].hub_int_expect_phys = 0;
                                self.queue_hub_change_read(slot_id as u8);
                                return;
                            }

                            // PIUSB-39: the SAME hole on the HID interrupt-IN endpoints. An ERROR
                            // completion (STALL / transaction error / babble) on the pointer or
                            // keyboard read falls straight through the success gate below, so the
                            // read was never re-armed and the device went permanently dead — the
                            // other half of the P54b metal fact (the mouse endpoint carries by far
                            // the most traffic, so it is the one that loses this race). Trace and
                            // re-arm, exactly as the hub Status Change Endpoint does; CErr=3 lets
                            // the controller absorb transient errors itself, and the bounded
                            // TransferRing keeps a truly wedged endpoint from looping the CPU.
                            let (m_dci, k_dci, has_mbuf, has_kbuf) = {
                                let s = &self.slots[slot_id as usize];
                                let m = if s.is_mouse && s.mouse_ep != 0 {
                                    Some((s.mouse_ep & 0x0F) * 2 + if (s.mouse_ep & 0x80) != 0 { 1 } else { 0 })
                                } else { None };
                                let k = if s.is_keyboard && s.keyboard_ep != 0 {
                                    Some((s.keyboard_ep & 0x0F) * 2 + if (s.keyboard_ep & 0x80) != 0 { 1 } else { 0 })
                                } else { None };
                                (m, k, s.mouse_data_buffer.is_some() && s.mouse_ring.is_some(),
                                 s.data_buffer.is_some() && s.keyboard_ring.is_some())
                            };
                            // A HALTING code leaves the endpoint in the Halted state, where it
                            // IGNORES the doorbell: a bare re-queue would be a no-op and the
                            // pointer would stay dead anyway. Those need the full un-halt
                            // (Reset Endpoint + Set TR Dequeue + device CLEAR_FEATURE(HALT)),
                            // which is synchronous and must not run inside this dispatch — queue
                            // it for the main loop (`service_hid_halts`). Codes that leave the
                            // endpoint RUNNING (e.g. 21 Ring Underrun / 22 Ring Overrun, 8 NAK,
                            // vendor codes) just need the read re-armed here.
                            // Halting: 2 Data Buffer Error, 3 Babble, 4 USB Transaction Error,
                            // 5 TRB Error, 6 Stall (xHCI 1.1 Table 6-90 / §4.10.2.1).
                            let halting = matches!(completion_code, 2 | 3 | 4 | 5 | 6);
                            if m_dci == Some(endpoint_id as u8) && has_mbuf {
                                Self::hid_error_witness(
                                    "pointer", slot_id, completion_code, halting);
                                if halting {
                                    if !self.hid_halt_pending.contains(&(slot_id as u8, true)) {
                                        self.hid_halt_pending.push((slot_id as u8, true));
                                    }
                                } else {
                                    MOUSE_ERROR_REARM_COUNT.fetch_add(1, Ordering::Relaxed);
                                    self.queue_mouse_read(slot_id as u8);
                                }
                                return;
                            }
                            if k_dci == Some(endpoint_id as u8) && has_kbuf {
                                Self::hid_error_witness(
                                    "keyboard", slot_id, completion_code, halting);
                                if halting {
                                    if !self.hid_halt_pending.contains(&(slot_id as u8, false)) {
                                        self.hid_halt_pending.push((slot_id as u8, false));
                                    }
                                } else {
                                    self.queue_keyboard_read(slot_id as u8);
                                }
                                return;
                            }
                        }

                        // UNA-19-REVEAL: If success or short packet, check buffer
                        if completion_code == 1 || completion_code == 13 {
                            // UNA-21-DEBUG: Force Transition based on Endpoint ID
                            // EP1 = Control (Device Descriptor)
                            // EP3 = Bulk IN (SCSI Read)

                            if endpoint_id == 1 && slot_id > 0 { // EP0 (Control) -> Device Descriptor
                                // TD-identity gate: only the completion of the async EP0 TD we
                                // actually queued may drive the state machine. Panther Point
                                // (Linux XHCI_SPURIOUS_SUCCESS quirk) can post a duplicate
                                // Success event after a Short Packet for the same TD; the old
                                // state-heuristic dispatch would re-enter the FSM on it (e.g.
                                // re-parse stale descriptor bytes and double-Configure). Sync
                                // EP0 TDs (hub bring-up) were already claimed above.
                                let expect = self.slots[slot_id as usize].ep0_expect_phys;
                                if expect == 0 || param != expect {
                                    serial_println!(
                                        "xHCI: stale/spurious EP0 event (slot {}, trb {:#x}, expected {:#x}); ignoring.",
                                        slot_id, param, expect);
                                    return;
                                }
                                self.slots[slot_id as usize].ep0_expect_phys = 0;
                                if self.slots[slot_id as usize].mouse_state == 2
                                    || self.slots[slot_id as usize].keyboard_state == 2 {
                                    // One device-level SET_CONFIGURATION covered every HID interface; arm
                                    // a read on each endpoint that was configured (keyboard into
                                    // data_buffer, pointer into mouse_data_buffer — separate buffers so a
                                    // composite device's two endpoints don't race), then advance ports.
                                    serial_println!("xHCI: >>> HID SET_CONFIGURATION COMPLETE <<<");
                                    if self.slots[slot_id as usize].keyboard_state == 2 {
                                        self.slots[slot_id as usize].keyboard_state = 3;
                                        self.slots[slot_id as usize].keyboard_report_count = 0;
                                        self.queue_keyboard_read(slot_id as u8);
                                    }
                                    if self.slots[slot_id as usize].mouse_state == 2 {
                                        self.slots[slot_id as usize].mouse_state = 3;
                                        // UI1-MOUSE M1: assertable enumeration witness — one line
                                        // naming the detected pointer so a serial-only bench (the
                                        // cursor is invisible over the FTDI cable) proves the real
                                        // Panther-Point xHCI armed the interrupt-IN read. Uncounted
                                        // (`== witness ::` idiom, not `-> PASS`) so no mbench COUNT
                                        // shifts; fires ONLY when a pointer enumerated (silent on
                                        // aarch64 / no-mouse / SKIP_XHCI, which never reach here).
                                        {
                                            let s = &self.slots[slot_id as usize];
                                            serial_println!(
                                                ":: MOUSE-1: HID pointer detected vid:pid={:04x}:{:04x} proto={} {} ep={:#04x} mps={} interval={} == witness ::",
                                                s.vid, s.pid,
                                                if s.mouse_is_relative { 2 } else { 0 },
                                                if s.mouse_is_relative { "relative" } else { "absolute" },
                                                s.mouse_ep, s.mouse_mps, s.mouse_interval
                                            );
                                        }
                                        self.slots[slot_id as usize].mouse_report_count = 0;
                                        self.queue_mouse_read(slot_id as u8);
                                    }
                                    // Boot-capable HID interfaces power up in REPORT protocol (report
                                    // IDs + device-defined layout the decoders don't parse). Queue a
                                    // main-loop SET_PROTOCOL(boot) so this device reports the fixed boot
                                    // layout our keyboard/mouse decoders expect.
                                    if !self.hid_setproto_pending.contains(&(slot_id as u8)) {
                                        self.hid_setproto_pending.push(slot_id as u8);
                                    }
                                    // Only a ROOT device's completed setup advances the port queue.
                                    // A hub-downstream device's SET_CONFIGURATION completes from the
                                    // main-loop hub service — calling start_next_port for it while a
                                    // root port is mid-enumeration would clobber enumerating_port and
                                    // orphan that port's in-flight enumeration.
                                    if !self.slots[slot_id as usize].is_downstream {
                                        self.start_next_port();
                                    }
                                } else {
                                    serial_println!("xHCI: >>> INTERCEPTED DESCRIPTOR EVENT (Slot 1 EP 1) <<<");
                                    unsafe {
                                        let desc_buf = self.slots[slot_id as usize].descriptor_buffer;
                                        // XHCI-COHERENCE: consumer boundary — the descriptor was
                                        // DMA-written by the controller; invalidate before reading. No-op x86.
                                        dma_coherency::inval(desc_buf as usize, 256);
                                        let desc_data = core::slice::from_raw_parts(desc_buf, 256);
                                        let vid = (desc_data[8] as u16) | ((desc_data[9] as u16) << 8);
                                    let pid = (desc_data[10] as u16) | ((desc_data[11] as u16) << 8);

                                    serial_println!(">>> SYSTEM ALERT: NEW HARDWARE DETECTED <<<");
                                    serial_println!(">>> [CONTACT ESTABLISHED] SLOT {}", slot_id);
                                    serial_println!(">>> VENDOR ID : [{:04x}]", vid);
                                    serial_println!(">>> PRODUCT ID: [{:04x}]", pid);

                                    // U2.5: persist idVendor/idProduct on the slot — but ONLY from a
                                    // DEVICE descriptor (bDescriptorType 0x01). This same block also
                                    // handles config-descriptor events, where desc_data[8..12] are
                                    // interface/endpoint bytes, not vid/pid; guarding here keeps the
                                    // FTDI's real 0403:6001 from being clobbered. The FT232's
                                    // vendor-specific interface later keys on these to identify itself.
                                    if desc_data[1] == 0x01 {
                                        self.slots[slot_id as usize].vid = vid;
                                        self.slots[slot_id as usize].pid = pid;
                                    }

                                    // UNA-22-HAUL: Inspect Class Code
                                    let class_code = desc_data[4];
                                    let subclass = desc_data[5];
                                    let protocol = desc_data[6];

                                    serial_println!("xHCI: Device Found. Class={:#x} Sub={:#x} Proto={:#x}",
                                        class_code, subclass, protocol);

                                    if class_code == 0x08 { // 0x08 = Mass Storage (device-level)
                                        serial_println!("xHCI: >>> CARGO DETECTED (MASS STORAGE) <<<");
                                        serial_println!("xHCI: Requesting Configuration Descriptor for bulk endpoints...");
                                        // Route through the config-descriptor parser so the
                                        // real bulk endpoint addresses + MPS drive configure_endpoints.
                                        self.request_configuration_descriptor(slot_id as u8);
                                    } else if class_code == 0x09 {
                                        // USB Hub. Defer the (multi-step, synchronous) hub
                                        // bring-up to the main loop and continue root enumeration.
                                        serial_println!("xHCI: >>> HUB DETECTED (slot {}) <<<", slot_id);
                                        self.hubs_pending.push(slot_id as u8);
                                        self.start_next_port();
                                    } else if class_code == 0x00 {
                                        // Class 0 means "Look at Interface Descriptor" (Common for Flash Drives too)
                                        serial_println!("xHCI: Composite Device. Requesting Configuration Descriptor...");
                                        self.request_configuration_descriptor(slot_id as u8);
                                    } else if desc_data[1] == 0x02 { // Configuration Descriptor Response
                                        serial_println!("xHCI: >>> CONFIGURATION DESCRIPTOR RECEIVED <<<");
                                        // Parse Configuration Descriptor to find HID Interfaces
                                        let mut offset = 0;
                                        let total_length = (desc_data[2] as u16) | ((desc_data[3] as u16) << 8);
                                        serial_println!("xHCI: Configuration Descriptor Total Length: {}", total_length);
                                        
                                        // Track current interface class/protocol while parsing (for the
                                        // serial trace + MSC/FTDI detection). HID interrupt-IN interfaces —
                                        // keyboard AND mouse, on a composite device — are armed by the shared
                                        // `record_hid_interfaces` walk after this loop, the SAME walk the
                                        // hub-downstream path uses; only the MSC/FTDI bulk-endpoint tracking
                                        // is collected inline here.
                                        let mut current_intf_class: u8 = 0;
                                        let mut current_intf_protocol: u8 = 0;
                                        // Mass-storage tracking: collect the bulk IN/OUT
                                        // endpoints during the walk, configure once after.
                                        let mut is_mass_storage = false;
                                        // U2.5: FTDI FT232 tracking — its vendor-specific interface
                                        // (class 0xFF) carries the same bulk IN/OUT pair as MSC, so it
                                        // reuses the bulk-collection arm below.
                                        let mut is_ftdi = false;
                                        let mut bulk_in: Option<(u8, u16)> = None;
                                        let mut bulk_out: Option<(u8, u16)> = None;

                                        while offset < total_length as usize && offset < 256 {
                                            if offset + 1 >= 256 { break; }
                                            let length = desc_data[offset] as usize;
                                            if length == 0 { break; }
                                            let desc_type = desc_data[offset + 1];

                                            if desc_type == 0x04 { // Interface Descriptor
                                                if offset + 7 >= 256 { break; }
                                                current_intf_class = desc_data[offset + 5];
                                                let intf_subclass = desc_data[offset + 6];
                                                current_intf_protocol = desc_data[offset + 7];
                                                serial_println!("xHCI: Interface: Class={:#x} Sub={:#x} Proto={:#x}",
                                                    current_intf_class, intf_subclass, current_intf_protocol);

                                                if current_intf_class == 0x08 {
                                                    // Mass Storage interface (SCSI Bulk-Only, 0x08/0x06/0x50).
                                                    // This device reports class 0 at the device level, so the
                                                    // interface descriptor is the only place to detect it. We
                                                    // collect its bulk endpoints below and configure after the walk.
                                                    serial_println!("xHCI: >>> MASS STORAGE INTERFACE DETECTED (Class 0x08) <<<");
                                                    is_mass_storage = true;
                                                    // PIUSB-38: remember the MSC bInterfaceNumber
                                                    // (descriptor byte +2) — the `wIndex` a Bulk-Only
                                                    // Mass Storage Reset targets during reset recovery.
                                                    self.slots[slot_id as usize].storage_intf = desc_data[offset + 2];
                                                } else if current_intf_class == 0xFF {
                                                    // U2.5: a vendor-specific interface. The FTDI FT232 (device
                                                    // class 0x00 → Composite branch) exposes exactly one such
                                                    // interface with a bulk IN/OUT pair; gate on the persisted
                                                    // idVendor/idProduct so only the real FT232 (0403:6001) is
                                                    // treated as an FTDI console.
                                                    let (vid, pid) = {
                                                        let s = &self.slots[slot_id as usize];
                                                        (s.vid, s.pid)
                                                    };
                                                    if vid == ftdi::FTDI_VID && pid == ftdi::FTDI_PID {
                                                        serial_println!("xHCI: >>> FTDI USB-SERIAL DETECTED (0403:6001) <<<");
                                                        is_ftdi = true;
                                                    }
                                                }
                                            } else if desc_type == 0x05 && (is_mass_storage || is_ftdi) { // Bulk Endpoint (MSC or FTDI)
                                                if offset + 6 >= 256 { break; }
                                                let ep_addr = desc_data[offset + 2];
                                                let ep_attr = desc_data[offset + 3];
                                                // wMaxPacketSize bits 10:0 (mask off HS mult bits 12:11).
                                                let ep_mps = ((desc_data[offset + 4] as u16) | ((desc_data[offset + 5] as u16) << 8)) & 0x07FF;
                                                if (ep_attr & 0x03) == 0x02 { // Bulk transfer type
                                                    if (ep_addr & 0x80) != 0 {
                                                        serial_println!("xHCI: >>> BULK IN EP FOUND: {:#x}, MPS: {} <<<", ep_addr, ep_mps);
                                                        bulk_in = Some((ep_addr, ep_mps));
                                                    } else {
                                                        serial_println!("xHCI: >>> BULK OUT EP FOUND: {:#x}, MPS: {} <<<", ep_addr, ep_mps);
                                                        bulk_out = Some((ep_addr, ep_mps));
                                                    }
                                                }
                                            }
                                            offset += length;
                                        }

                                        // Arm EVERY HID interrupt-IN interface (keyboard and/or pointer)
                                        // via the single shared walk — the SAME walk the hub-downstream
                                        // path uses — then configure them all together in one
                                        // Configure-Endpoint. `desc_buf` points at this slot's descriptor
                                        // buffer (read above as `desc_data`); record_hid_interfaces reads
                                        // it independently and writes only the HID slot fields.
                                        if self.record_hid_interfaces(slot_id as u8, desc_buf as u64) {
                                            self.configure_hid_endpoints(slot_id as u8, true);
                                        }

                                        // Once both bulk directions are known, configure them.
                                        if is_mass_storage {
                                            match (bulk_in, bulk_out) {
                                                (Some((ia, im)), Some((oa, om))) => {
                                                    self.configuring_slot = slot_id as u8;
                                                    self.configure_endpoints(slot_id as u8, ia, im, oa, om);
                                                }
                                                _ => {
                                                    serial_println!("xHCI: Mass storage missing bulk endpoints (in={:?}, out={:?}); skipping device.", bulk_in, bulk_out);
                                                    self.start_next_port();
                                                }
                                            }
                                        } else if is_ftdi {
                                            // U2.5: same bulk-generic Configure-Endpoint as storage, but
                                            // tracked via `ftdi_configuring_slot` so its completion routes
                                            // to the FTDI console bring-up, not the SCSI path. (A device
                                            // is either MSC or FTDI — different interface classes — so
                                            // these two arms are mutually exclusive.)
                                            match (bulk_in, bulk_out) {
                                                (Some((ia, im)), Some((oa, om))) => {
                                                    self.ftdi_configuring_slot = slot_id as u8;
                                                    self.configure_endpoints(slot_id as u8, ia, im, oa, om);
                                                }
                                                _ => {
                                                    serial_println!("xHCI: FTDI missing bulk endpoints (in={:?}, out={:?}); skipping device.", bulk_in, bulk_out);
                                                    self.start_next_port();
                                                }
                                            }
                                        }
                                    }
                                }
                                }
                            } else if endpoint_id > 1 && slot_id > 0 { // Non-EP0 Transfer Event
                                // (BOT completions were already claimed above, before the
                                // success gate, so only HID interrupt reads arrive here.)
                                unsafe {
                                    let slot = &self.slots[slot_id as usize];

                                    // Compute expected DCI for mouse and keyboard
                                    let mouse_dci = if slot.is_mouse && slot.mouse_ep != 0 {
                                        let ep_num = slot.mouse_ep & 0x0F;
                                        let dir_in = (slot.mouse_ep & 0x80) != 0;
                                        Some((ep_num * 2) + if dir_in { 1 } else { 0 })
                                    } else { None };
                                    
                                    let keyboard_dci = if slot.is_keyboard && slot.keyboard_ep != 0 {
                                        let ep_num = slot.keyboard_ep & 0x0F;
                                        let dir_in = (slot.keyboard_ep & 0x80) != 0;
                                        Some((ep_num * 2) + if dir_in { 1 } else { 0 })
                                    } else { None };

                                    // XENUM-2: a hub's interrupt-IN Status Change Endpoint (always IN).
                                    let hub_int_dci = if slot.is_hub && slot.hub_int_ep != 0 {
                                        Some((slot.hub_int_ep & 0x0F) * 2 + 1)
                                    } else { None };

                                    if mouse_dci == Some(endpoint_id as u8) {
                                        // --- POINTER (mouse / tablet) --- reads into its own buffer.
                                        // UI1-MOUSE M2: Panther-Point dup-Success guard
                                        // (XHCI_SPURIOUS_SUCCESS, device 0x1e31). A boot-mouse report
                                        // is ALWAYS shorter than the endpoint MPS, so the controller can
                                        // post a duplicate Success for the SAME TD after the Short
                                        // Packet. Only the completion whose TRB matches the armed read
                                        // is real; re-decoding + re-arming on the dup would double the
                                        // cursor motion and over-arm the interrupt-IN ring — the exact
                                        // EP0 hazard (`ep0_expect_phys`), applied to interrupt-IN. On
                                        // QEMU (no dup) `param` always matches, so this never trips.
                                        //
                                        // PIUSB-39: the guard STAYS (the dup hazard is real), but its
                                        // exit must discard the DATA, never the PIPELINE. The old
                                        // unconditional `return` retired the pointer interrupt-IN
                                        // endpoint forever on ANY mismatched completion — the P54b
                                        // metal fact (after an EL0 app's focus drop the mouse is
                                        // permanently dead while the independently-armed keyboard
                                        // keeps working). Discriminate:
                                        //   * `param == mouse_prev_phys` — a genuine Panther-Point dup
                                        //     for the TD we already consumed. A fresh read is ALREADY
                                        //     armed, so re-arming here would over-arm the ring (the
                                        //     exact UI1-MOUSE M2 hazard). Discard, do not re-arm.
                                        //   * any other mismatch — the endpoint retired a TD we cannot
                                        //     account for; nothing is guaranteed armed. Discard the
                                        //     report, then RE-ARM so the pointer survives.
                                        if slot.mouse_expect_phys != 0 && param != slot.mouse_expect_phys {
                                            let prev = slot.mouse_prev_phys;
                                            let expect = slot.mouse_expect_phys;
                                            let have_buf = slot.mouse_data_buffer.is_some()
                                                && slot.mouse_ring.is_some();
                                            xdbg!("xHCI: stale/spurious pointer event (slot {}, trb {:#x}, expected {:#x}); ignoring.",
                                                slot_id, param, expect);
                                            if param != prev && have_buf {
                                                // Not the known dup — re-arm rather than lose the pointer.
                                                MOUSE_DISCARD_REARM_COUNT.fetch_add(1, Ordering::Relaxed);
                                                self.queue_mouse_read(slot_id as u8);
                                                Self::piusb39_witness("guard");
                                            }
                                            return;
                                        }
                                        if let Some(data_buf_ptr) = slot.mouse_data_buffer {
                                            // XHCI-COHERENCE: consumer boundary — the interrupt-IN
                                            // report was DMA-written; invalidate before decoding. No-op x86.
                                            dma_coherency::inval(data_buf_ptr as usize, 512);
                                            let data_data = core::slice::from_raw_parts(data_buf_ptr, 512);
                                            let _buttons = data_data[0];
                                            // Metal diagnostic (parallels the keyboard dump): show the raw
                                            // pointer report AND the values our boot-layout decode extracts,
                                            // so a serial-less boot can tell whether the deltas are where we
                                            // read them. Skip idle all-zero reports (only real movement/clicks
                                            // print) and throttle via the calibrated ms clock so continuous
                                            // motion yields a few readable lines instead of an unreadable flood.
                                            #[cfg(feature = "usbdebug")]
                                            {
                                                static LAST_PTR_LOG_MS: core::sync::atomic::AtomicU64 =
                                                    core::sync::atomic::AtomicU64::new(0);
                                                let non_idle = (data_data[0] | data_data[1] | data_data[2] | data_data[3]) != 0;
                                                let now = crate::arch::ticks();
                                                if non_idle
                                                    && now.wrapping_sub(LAST_PTR_LOG_MS.load(core::sync::atomic::Ordering::Relaxed)) >= 150
                                                {
                                                    LAST_PTR_LOG_MS.store(now, core::sync::atomic::Ordering::Relaxed);
                                                    if slot.mouse_is_relative {
                                                        serial_println!(
                                                            "USB-DEBUG: ptr report (rel) {:02x} {:02x} {:02x} {:02x} -> dx={} dy={}",
                                                            data_data[0], data_data[1], data_data[2], data_data[3],
                                                            data_data[1] as i8, data_data[2] as i8
                                                        );
                                                    } else {
                                                        serial_println!(
                                                            "USB-DEBUG: ptr report (abs) {:02x} {:02x} {:02x} {:02x} -> x={} y={}",
                                                            data_data[0], data_data[1], data_data[2], data_data[3],
                                                            (data_data[1] as u16) | ((data_data[2] as u16) << 8),
                                                            (data_data[3] as u16) | ((data_data[4] as u16) << 8)
                                                        );
                                                    }
                                                }
                                            }

                                            let rel = slot.mouse_is_relative;
                                            let buttons = data_data[0];
                                            let (last_a, last_b) = if rel {
                                                // HID BOOT mouse: byte0 = buttons, byte1 = dx:i8, byte2 = dy:i8
                                                // (byte3 = wheel, ignored). Signed relative deltas — sign-extend
                                                // i8 -> i32 and emit only on actual motion.
                                                let dx = data_data[1] as i8 as i32;
                                                let dy = data_data[2] as i8 as i32;
                                                if dx != 0 || dy != 0 {
                                                    crate::pal::push_event(crate::pal::Event::Mouse { x: dx, y: dy });
                                                }
                                                (dx, dy)
                                            } else {
                                                // usb-tablet / absolute pointer: byte1-2 = X, byte3-4 = Y (0..32767).
                                                let x = (data_data[1] as u16) | ((data_data[2] as u16) << 8);
                                                let y = (data_data[3] as u16) | ((data_data[4] as u16) << 8);
                                                if x != 0 || y != 0 {
                                                    crate::pal::push_event(crate::pal::Event::MouseAbsolute { x: x as i32, y: y as i32 });
                                                }
                                                (x as i32, y as i32)
                                            };
                                            // `slot` (the shared borrow) is no longer read past here;
                                            // the &mut self accesses below are the count bump + re-arm.

                                            // GUI-CLICK-2 / HID-KEYS: emit a Button on ANY mask
                                            // change — both the press edge (a bit going 0→1) and the
                                            // release edge (a bit going 1→0). The payload is always
                                            // the CURRENT mask, so a consumer that only acts on
                                            // presses stays correct (it sees a nonzero mask on press
                                            // and a mask with the released bit cleared — often 0 — on
                                            // release, which it ignores); a consumer that tracks
                                            // held-state now gets the release it was missing (the
                                            // GUI-CLICK-2b gap). A held button (unchanged mask) still
                                            // does not re-fire. Shared xHCI code: x86 xHCI mice gain
                                            // the same correct release edge (a fix, not a risk —
                                            // EHCI keeps its own emit).
                                            let prev_btn = self.slots[slot_id as usize].mouse_prev_buttons;
                                            if buttons != prev_btn {
                                                #[cfg(feature = "usbdebug")]
                                                serial_println!("[hidkeys] button {:#04x} -> {:#04x} slot={}", prev_btn, buttons, slot_id);
                                                crate::pal::push_event(crate::pal::Event::Button(buttons));
                                            }
                                            self.slots[slot_id as usize].mouse_prev_buttons = buttons;

                                            // UI1-MOUSE M1: bounded serial mouse-witness — first report
                                            // + every 32nd thereafter, NEVER one-per-report (that would
                                            // flood the FTDI on continuous motion). Uncounted
                                            // (`== witness ::`) so no mbench COUNT shifts.
                                            let n = self.slots[slot_id as usize].mouse_report_count.wrapping_add(1);
                                            self.slots[slot_id as usize].mouse_report_count = n;
                                            if n == 1 || n % 32 == 0 {
                                                if rel {
                                                    serial_println!(
                                                        ":: MOUSE-1: {} reports, last dx={} dy={} buttons={:#04x} == witness ::",
                                                        n, last_a, last_b, buttons);
                                                } else {
                                                    serial_println!(
                                                        ":: MOUSE-1: {} reports, last x={} y={} buttons={:#04x} == witness ::",
                                                        n, last_a, last_b, buttons);
                                                }
                                            }

                                            self.queue_mouse_read(slot_id as u8);
                                        }
                                    } else if keyboard_dci == Some(endpoint_id as u8) {
                                        // --- KEYBOARD ---
                                        // Panther-Point dup-Success guard (XHCI_SPURIOUS_SUCCESS,
                                        // device 0x1e31), identical to the pointer path: a boot-kbd
                                        // report is ALWAYS shorter than the endpoint MPS, so the
                                        // controller can post a duplicate Success for the SAME TD
                                        // after the Short Packet. Only the completion whose TRB
                                        // matches the armed read is real; a dup would double-inject
                                        // the keystrokes and over-arm the interrupt-IN ring. On QEMU
                                        // (no dup) `param` always matches, so this never trips.
                                        // PIUSB-39: same pipeline-preserving exit as the pointer path
                                        // (the keyboard carried the identical defect — only its lower
                                        // traffic kept it from being observed on metal).
                                        if slot.keyboard_expect_phys != 0 && param != slot.keyboard_expect_phys {
                                            let prev = slot.keyboard_prev_phys;
                                            let expect = slot.keyboard_expect_phys;
                                            let have_buf = slot.data_buffer.is_some()
                                                && slot.keyboard_ring.is_some();
                                            xdbg!("xHCI: stale/spurious keyboard event (slot {}, trb {:#x}, expected {:#x}); ignoring.",
                                                slot_id, param, expect);
                                            if param != prev && have_buf {
                                                self.queue_keyboard_read(slot_id as u8);
                                            }
                                            return;
                                        }
                                        if let Some(data_buf_ptr) = slot.data_buffer {
                                            // XHCI-COHERENCE: consumer boundary — the boot-keyboard
                                            // report was DMA-written; invalidate before decoding. No-op x86.
                                            dma_coherency::inval(data_buf_ptr as usize, 8);
                                            // PIUSB-13: count every serviced keyboard report (the Pi-side
                                            // `[enum]` observer watches the 0→1 edge for its first-report
                                            // witness). Mirrors `mouse_report_count`; inert on x86.
                                            self.slots[slot_id as usize].keyboard_report_count =
                                                self.slots[slot_id as usize].keyboard_report_count.wrapping_add(1);
                                            let report = core::slice::from_raw_parts(data_buf_ptr, 8);
                                            // Metal diagnostic: dump the raw report bytes so that if a keyboard
                                            // interrupt-IN transfer arrives but decodes to nothing (e.g. the device is
                                            // in HID report protocol rather than boot protocol, which needs
                                            // SET_PROTOCOL(boot)), we still SEE that reports are flowing.
                                            // Skip idle all-zero reports (a composite dongle's keyboard
                                            // interface polls continuously and floods the view) so only
                                            // real keypresses print.
                                            #[cfg(feature = "usbdebug")]
                                            if report[..8].iter().any(|&b| b != 0) {
                                                serial_println!(
                                                    "USB-DEBUG: kbd report {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                                                    report[0], report[1], report[2], report[3], report[4], report[5], report[6], report[7]
                                                );
                                            }
                                            // USB HID Boot Keyboard Report Format:
                                            // Byte 0: Modifier keys (bit 1 = L-Shift, bit 5 = R-Shift)
                                            // Byte 1: Reserved
                                            // Bytes 2-7: Key codes (up to 6 simultaneous keys)
                                            let modifiers = report[0];
                                            let shift = (modifiers & 0x22) != 0; // L-Shift (bit 1) or R-Shift (bit 5)
                                            // HID-LED: current caps-lock LED state feeds the ascii case
                                            // logic so the lit LED and the typed case agree. Caps only
                                            // inverts case for the alphabetic keycodes (0x04..=0x1D =
                                            // a..z); digits/symbols are unaffected (caps XOR shift would
                                            // wrongly shift them). effective_shift = shift ^ (caps & is_letter).
                                            let caps = (self.slots[slot_id as usize].keyboard_leds & 0x02) != 0;

                                            // HID-KEYS: snapshot this report's keycodes and the
                                            // previous report's, so releases (a code present last
                                            // report, absent now) can be edge-detected below. The
                                            // KeyDown loop is unchanged — Key(ascii) still fires for
                                            // every held key each report (natural repeats on resend),
                                            // preserving every existing consumer.
                                            let cur_keys: [u8; 6] = [
                                                report[2], report[3], report[4],
                                                report[5], report[6], report[7],
                                            ];
                                            let prev_keys = self.slots[slot_id as usize].keyboard_prev_keys;

                                            for i in 2..8 {
                                                let keycode = report[i];
                                                if keycode == 0 { continue; } // No key
                                                if keycode == 1 { continue; } // ErrorRollOver

                                                if (keycode as usize) < HID_SCANCODE_TO_ASCII.len() {
                                                    let (unshifted, shifted) = HID_SCANCODE_TO_ASCII[keycode as usize];
                                                    let is_letter = (0x04..=0x1D).contains(&keycode);
                                                    let eff_shift = shift ^ (caps & is_letter);
                                                    let ascii = if eff_shift { shifted } else { unshifted };
                                                    if ascii != 0 {
                                                        serial_println!("xHCI: KEY: '{}' (scancode {:#x})", ascii as char, keycode);
                                                        crate::pal::push_event(crate::pal::Event::Key(ascii));
                                                    }
                                                }
                                            }

                                            // UVUG-6: feed the host-side typematic tracker at the HID REPORT
                                            // level — BEFORE any EVENT_QUEUE push, so a release the queue may
                                            // later DROP (full 64-slot ring) can never strand a held key. Pass
                                            // the newest ascii pressed this report and the FULL currently-held
                                            // ascii set; the tracker disarms the moment its key leaves the set.
                                            #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
                                            {
                                                let mut held: [u8; 6] = [0; 6];
                                                let mut hn = 0usize;
                                                let mut newest_press: u8 = 0;
                                                for i in 2..8 {
                                                    let keycode = report[i];
                                                    if keycode == 0 || keycode == 1 { continue; }
                                                    if (keycode as usize) < HID_SCANCODE_TO_ASCII.len() {
                                                        let (unshifted, shifted) = HID_SCANCODE_TO_ASCII[keycode as usize];
                                                        let is_letter = (0x04..=0x1D).contains(&keycode);
                                                        let eff_shift = shift ^ (caps & is_letter);
                                                        let ascii = if eff_shift { shifted } else { unshifted };
                                                        if ascii != 0 {
                                                            held[hn] = ascii;
                                                            hn += 1;
                                                            if !prev_keys.contains(&keycode) { newest_press = ascii; }
                                                        }
                                                    }
                                                }
                                                crate::pal::typematic_note_report(newest_press, &held[..hn]);
                                            }

                                            // HID-KEYS: key-UP edges. A boot report carries the FULL
                                            // pressed-key set, so any keycode in the previous report
                                            // that is absent now was released — emit KeyUp(ascii)
                                            // once per such code. Shift state at release time is used
                                            // for the ascii (consumers that care about identity match
                                            // case-insensitively; e.g. GAME-MODE lowercases).
                                            for &keycode in prev_keys.iter() {
                                                if keycode == 0 || keycode == 1 { continue; }
                                                if cur_keys.contains(&keycode) { continue; } // still held
                                                if (keycode as usize) < HID_SCANCODE_TO_ASCII.len() {
                                                    let (unshifted, shifted) = HID_SCANCODE_TO_ASCII[keycode as usize];
                                                    let is_letter = (0x04..=0x1D).contains(&keycode);
                                                    let eff_shift = shift ^ (caps & is_letter);
                                                    let ascii = if eff_shift { shifted } else { unshifted };
                                                    if ascii != 0 {
                                                        #[cfg(feature = "usbdebug")]
                                                        serial_println!("[hidkeys] keyup '{}' (scancode {:#x}) slot={}", ascii as char, keycode, slot_id);
                                                        crate::pal::push_event(crate::pal::Event::KeyUp(ascii));
                                                    }
                                                }
                                            }

                                            self.slots[slot_id as usize].keyboard_prev_keys = cur_keys;

                                            // HID-LED: lock-key press edges. A lock key present in this
                                            // report but absent last report is a fresh press — toggle the
                                            // matching LED bit and push the new bitmap to the device via
                                            // SET_REPORT. Caps Lock (0x39, bit1) is the one Peter observed
                                            // never lighting; Num Lock (0x53, bit0) and Scroll Lock (0x47,
                                            // bit2) are toggled the same way for LED/state agreement.
                                            let mut leds_changed = false;
                                            for &(usage, bit) in &[(0x39u8, 0x02u8), (0x53u8, 0x01u8), (0x47u8, 0x04u8)] {
                                                let pressed_now = cur_keys.contains(&usage);
                                                let pressed_before = prev_keys.contains(&usage);
                                                if pressed_now && !pressed_before {
                                                    self.slots[slot_id as usize].keyboard_leds ^= bit;
                                                    leds_changed = true;
                                                }
                                            }
                                            if leds_changed {
                                                let kbd_intf = self.slots[slot_id as usize].keyboard_intf;
                                                self.set_hid_leds(slot_id as u8, kbd_intf);
                                            }

                                            self.queue_keyboard_read(slot_id as u8);
                                        }
                                    } else if hub_int_dci == Some(endpoint_id as u8) {
                                        // --- XENUM-2: hub Status Change Endpoint completion ---
                                        // Panther-Point dup-Success guard (as for the pointer read):
                                        // only the completion whose TRB matches the armed read is real.
                                        if slot.hub_int_expect_phys != 0 && param != slot.hub_int_expect_phys {
                                            xdbg!("xHCI: stale/spurious hub status-change event (slot {}, trb {:#x}, expected {:#x}); ignoring.",
                                                slot_id, param, slot.hub_int_expect_phys);
                                            return;
                                        }
                                        // Copy out what we need, then release the shared borrow: the
                                        // decode + queue + re-arm below take &mut self.
                                        let nbr_ports = slot.hub_nbr_ports;
                                        let change_buf = slot.hub_change_buffer;
                                        if let Some(buf_ptr) = change_buf {
                                            let len = Self::hub_change_bitmap_len(nbr_ports);
                                            // XHCI-COHERENCE: consumer boundary — the status-change
                                            // bitmap was DMA-written; invalidate before reading. No-op x86.
                                            dma_coherency::inval(buf_ptr as usize, len);
                                            let bytes = core::slice::from_raw_parts(buf_ptr, len);
                                            // Bit 0 = the hub itself (over-current / local change).
                                            if (bytes[0] & 1) != 0 {
                                                serial_println!("xHCI: HUB slot {} status-change: hub-local (bit 0).", slot_id);
                                            }
                                            // Bit N = downstream port N changed. Queue each for the
                                            // main-loop service_hub_changes (GET_PORT_STATUS + action).
                                            // Bound the port walk to the bits the (≤8-byte-clamped)
                                            // buffer actually holds: nbr_ports is an UNTRUSTED u8
                                            // from the hub descriptor, and an unbounded walk would
                                            // index bytes[] out of range for bNbrPorts ≥ 8*len —
                                            // a device-supplied kernel panic in event dispatch.
                                            let max_port = nbr_ports.min((len * 8 - 1) as u8);
                                            for port in 1..=max_port {
                                                let bit = port as usize;
                                                if (bytes[bit / 8] & (1 << (bit % 8))) != 0 {
                                                    serial_println!("xHCI: HUB slot {} status-change: port {}", slot_id, port);
                                                    if !self.hub_changes_pending.iter().any(|&e| e == (slot_id as u8, port)) {
                                                        self.hub_changes_pending.push((slot_id as u8, port));
                                                    }
                                                }
                                            }
                                        }
                                        // Re-arm the read so the next change is delivered.
                                        self.slots[slot_id as usize].hub_int_expect_phys = 0;
                                        self.queue_hub_change_read(slot_id as u8);
                                    }
                                    // Bulk (mass storage) completions are handled above via
                                    // bot_pending and never reach here.
                                }
                            }
                        }
                    },
                    _ => {
                        serial_println!("xHCI: [Event] Unknown Type {}. Param={:#x}, Status={:#x}",
                            trb_type, param, status);
                    }
                }
    }

    pub fn read_version(&self) -> u16 {
        unsafe {
            let cap_ptr = self.base_addr as *const u32;
            let cap_word = core::ptr::read_volatile(cap_ptr);
            (cap_word >> 16) as u16
        }
    }

    pub fn reset(&mut self) {
        let usbcmd_ptr = self.op_base as *mut u32;
        let usbsts_ptr = (self.op_base + 0x04) as *const u32; // Status reg is at +0x04

        unsafe {
            serial_println!("xHCI: Asserting HCRST...");
            let cmd = core::ptr::read_volatile(usbcmd_ptr);
            // Write 1 to Bit 1 (HCRST)
            core::ptr::write_volatile(usbcmd_ptr, cmd | 2);

            // Intel quirk (Linux XHCI_INTEL_HOST): ~1 ms after HCRST before ANY register
            // access, or the host can — rarely — hang the whole system.
            let t0 = crate::arch::now_cycles();
            let one_ms = (hw_wait_budget() / 2000).max(1);
            while crate::arch::now_cycles().wrapping_sub(t0) < one_ms {
                core::hint::spin_loop();
            }

            // POLL: Wait for HCRST (Bit 1) to clear (hardware clears it when done)
            let _ = wait_until(
                || (core::ptr::read_volatile(usbcmd_ptr) & 2) == 0,
                hw_wait_budget(), "USBCMD.HCRST=0 (reset)");
            serial_println!("xHCI: Reset Complete.");

            // POLL: Wait for CNR (Controller Not Ready, Bit 11 in USBSTS) to clear
            // The controller needs time to re-initialize after reset.
            let _ = wait_until(
                || (core::ptr::read_volatile(usbsts_ptr) & (1 << 11)) == 0,
                hw_wait_budget(), "USBSTS.CNR=0");
            serial_println!("xHCI: Controller Ready.");
        }
    }

    pub unsafe fn init_pointers(&mut self, ring_phys_addr: u64) {
        // PIUSB-10: if CNR never cleared (init_interrupter aborted), do NOT program CRCR/DCBAAP —
        // the controller is not ready and would silently drop the writes. Fail loud, skip cleanly.
        if !XHCI_CNR_OK.load(Ordering::Acquire) {
            serial_println!("xHCI: init_pointers SKIPPED — controller never left Not-Ready (CNR=1)");
            return;
        }
        unsafe {
            // 1. Allocate and set DCBAAP
            let dcbaap_size = (self.max_slots as usize + 1) * 8;
            let layout = core::alloc::Layout::from_size_align(dcbaap_size, 64).unwrap();
            let dcbaap_ptr = alloc::alloc::alloc_zeroed(layout) as *mut u64;
            self.dcbaap = dcbaap_ptr;

            let dcbaap_reg = (self.op_base + 0x30) as *mut u64;
            write_reg64(dcbaap_reg, dcbaap_ptr as u64);
            serial_println!("xHCI: DCBAAP set to {:#x}", dcbaap_ptr as u64);

            // 1b. SCRATCHPAD BUFFERS (xHCI spec 4.20). If the controller advertises Max Scratchpad
            // Buffers > 0 in HCSPARAMS2, the OS MUST allocate that many page-sized buffers + a
            // Scratchpad Buffer Array of their physical addresses, and point DCBAA[0] at that array
            // — this is the controller's private working memory. Skip it and the controller faults
            // with a Host System Error (USBSTS.HSE) the moment it processes its first command.
            // QEMU's qemu-xhci requests 0 (so this is a clean no-op there); real Intel xHCI (Panther
            // Point / 2012 MacBook Pro) requests several — without them ENABLE_SLOT raised HSE on
            // metal while the command ring was running (CRR=1). DCBAA[0] is otherwise reserved/zero.
            let hcsparams2 = core::ptr::read_volatile((self.base_addr + 0x08) as *const u32);
            let max_scratchpad =
                ((((hcsparams2 >> 21) & 0x1F) << 5) | ((hcsparams2 >> 27) & 0x1F)) as usize;
            if max_scratchpad > 0 {
                // PAGESIZE (op_base + 0x08): bit n set => the controller supports 2^(n+12)-byte
                // pages; the scratchpad buffers must be that size and aligned. Use the smallest
                // supported page size (lowest set bit).
                // XCARVE-4: trust only the spec-sane low bits (4K/8K/16K/32K). An inherited
                // controller taken over without HCRST (Tegra234 JB9G) can read back PAGESIZE with
                // the mandatory 4 KiB bit CLEAR and garbage high bits — the raw lowest-set-bit
                // math then demands an 8 MiB-aligned allocation whose placement overshoots the
                // heap into firewalled carveout DRAM (the boots-13..22 SNOC RAS / sync-fault
                // writer). Spec 5.4.3 makes 4 KiB mandatory, so garbage => 4 KiB fallback.
                let pagesize = core::ptr::read_volatile((self.op_base + 0x08) as *const u32) & 0xFFFF;
                let sane = pagesize & 0x000F;
                let page_bytes: usize =
                    if sane == 0 { 0x1000 } else { 1usize << (sane.trailing_zeros() + 12) };

                // The Scratchpad Buffer Array: one u64 physical pointer per buffer, page-aligned.
                let arr_layout =
                    core::alloc::Layout::from_size_align(max_scratchpad * 8, page_bytes).unwrap();
                let arr = alloc::alloc::alloc_zeroed(arr_layout) as *mut u64;
                let (heap_lo, heap_hi) = crate::allocator::heap_bounds();
                if arr.is_null()
                    || (arr as usize) < heap_lo
                    || (arr as usize) + max_scratchpad * 8 > heap_hi
                {
                    serial_println!(
                        "xHCI: scratchpad: array alloc unusable (arr={:#x} page_bytes={:#x} heap=[{:#x},{:#x})); skipping",
                        arr as u64, page_bytes, heap_lo, heap_hi
                    );
                } else {
                    for i in 0..max_scratchpad {
                        let buf_layout =
                            core::alloc::Layout::from_size_align(page_bytes, page_bytes).unwrap();
                        let buf = alloc::alloc::alloc_zeroed(buf_layout);
                        if buf.is_null() {
                            break;
                        }
                        // Identity-mapped heap: the allocation's virtual address IS its bus/phys address.
                        *arr.add(i) = buf as u64;
                        // XHCI-COHERENCE: the controller DMA-reads/writes each scratchpad buffer as its
                        // private working memory; clean the zeroed buffer to DRAM so a non-snooping
                        // controller does not fault on stale contents. No-op x86.
                        dma_coherency::clean(buf as usize, page_bytes);
                    }
                    *dcbaap_ptr.add(0) = arr as u64;
                    // XHCI-COHERENCE: clean the scratchpad pointer array and the DCBAA[0] entry that
                    // points at it — both are controller-read before/at RS=1. No-op x86.
                    dma_coherency::clean(arr as usize, max_scratchpad * 8);
                    dma_coherency::clean(dcbaap_ptr as usize, core::mem::size_of::<u64>());
                    serial_println!(
                        "xHCI: scratchpad: {} buffer(s) x {} bytes; DCBAA[0]={:#x} (heap PA in [{:#x},{:#x}))",
                        max_scratchpad, page_bytes, arr as u64, heap_lo, heap_hi
                    );
                }
            } else {
                serial_println!("xHCI: scratchpad: controller requests 0 buffers (none needed).");
            }

            // 2. Set Command Ring Control Register (CRCR)
            // OpBase + 0x18.
            // MUST set Bit 0 (RCS - Ring Cycle State) to 1 to match our initial Ring state.
            let crcr_reg = (self.op_base + 0x18) as *mut u64;
            let crcr_value = ring_phys_addr | 1;
            write_reg64(crcr_reg, crcr_value);
            serial_println!("xHCI: CRCR set to {:#x}", crcr_value);
        }
    }

    // Call this AFTER init_pointers but BEFORE run
    pub fn init_interrupter(&mut self, event_ring_phys: u64, erst_table_phys: u64) {
        // PIUSB-10: xHCI 5.4.1/4.2 — the FIRST register programming after HCRST (this writes the
        // interrupter's ERST/ERSTBA/ERDP runtime regs; `init_pointers`/`start` write CRCR/DCBAAP/
        // CONFIG/USBCMD after us). Gate ALL of it on USBSTS.CNR==0 so no write is dropped by a
        // not-ready controller (the Pi VL805 holds CNR during fw-load; Intel clears it instantly, so
        // this is a fast no-op on x86 and its register-write behaviour is byte-identical).
        if !wait_for_cnr_clear(self.op_base) {
            XHCI_CNR_OK.store(false, Ordering::Release);
            return;
        }
        XHCI_CNR_OK.store(true, Ordering::Release);
        unsafe {
            // SAVE THIS for later use in the interrupt/event loop (ERDP updates)
            EVENT_RING_PHYS_BASE = event_ring_phys;

            // XHCI-COHERENCE: zeroed-handoff for the event ring. `EventRing::new()` zeroed the ring
            // into (dirty) cache lines; clean+invalidate so those zeros reach DRAM before the
            // controller DMA-writes events into it, and no stale CPU line shadows the first event.
            // This is the driver-internal replacement for PIUSB-8's external event-ring bridge.
            dma_coherency::clean_inval(
                event_ring_phys as usize,
                event::EVENT_RING_SIZE * core::mem::size_of::<Trb>(),
            );

            // 1. Calculate Runtime Base
            // Read RTSOFF (Offset 0x18 in Capability Regs)
            let rtsoff_ptr = (self.base_addr + 0x18) as *const u32;
            let rtsoff = core::ptr::read_volatile(rtsoff_ptr) & !0x1F; // Clear lower 5 bits? Spec says 32-byte aligned.
            let runtime_base = self.base_addr + rtsoff as usize;

            // Interrupter 0 Base = RuntimeBase + 0x20
            let ir0_base = runtime_base + 0x20;
            serial_println!("xHCI: RuntimeBase={:#x}, IR0 Base={:#x}", runtime_base, ir0_base);

            // 2. Setup the Segment Table (ERST)
            // NOTE: Caller holds the EVENT_RING lock and passes us the phys addr.
            // Do NOT lock EVENT_RING here or we deadlock.
            ERST_TABLE.entries[0] = ErstEntry {
                ring_address: event_ring_phys,
                size: event::EVENT_RING_SIZE as u16, // Must match EVENT_RING_SIZE in event.rs
                _rsvd: 0,
                _rsvd2: 0,
            };
            // XHCI-COHERENCE: producer boundary — the controller DMA-reads the ERST when the
            // interrupter is armed / ERSTBA is written below; clean the table to DRAM. No-op x86.
            dma_coherency::clean(
                core::ptr::addr_of!(ERST_TABLE) as usize,
                core::mem::size_of::<ErstTable>(),
            );
            EVENT_RING_PHYS_BASE = event_ring_phys;

            // 3. Write ERSTSZ (Segment Table Size) - Offset 0x08
            // Value = 1 (We have 1 segment)
            let erstsz_ptr = (ir0_base + 0x08) as *mut u32;
            core::ptr::write_volatile(erstsz_ptr, 1);

            // 4. Write ERSTBA (Segment Table Base Address) - Offset 0x10
            let erstba_ptr = (ir0_base + 0x10) as *mut u64;
            write_reg64(erstba_ptr, erst_table_phys);

            // 5. Write ERDP (Event Ring Dequeue Pointer) - Offset 0x18
            // Initialize to the start of the ring.
            // PRESERVE BIT 3 (EHB - Event Handler Busy)? No, clear it initially.
            let erdp_ptr = (ir0_base + 0x18) as *mut u64;
            // High-dword-first (write_erdp): even at init, guarantee the controller latches a
            // complete pointer under the PIUSB-21 32-bit split — never a stale/mirrored high.
            write_erdp(erdp_ptr, event_ring_phys); // Pointer to the RING, not the table
            serial_println!("[xhciint] ERDP initialized to {:#018x} (hi-first, EHB clear)", event_ring_phys);

            // 5b. IMOD (Interrupter Moderation, +0x04): 0 = no moderation, fire ASAP.
            // (QEMU ignores moderation timing, but set it explicitly for clarity / real HW.)
            core::ptr::write_volatile((ir0_base + 0x04) as *mut u32, 0);

            // Publish the MMIO bases BEFORE enabling the interrupter, so the lock-free MSI-X
            // handler can never load an un-initialized (0) base if an interrupt fires. (Both
            // IMAN.IE here and USBCMD.INTE in start() gate delivery, but publish-before-enable
            // is the correct-by-design ordering regardless of those gates.)
            XHCI_IR0_BASE.store(ir0_base, Ordering::Release);
            XHCI_OP_BASE.store(self.op_base, Ordering::Release);

            // 6. ENABLE the Interrupter (IMAN - Interrupter Management) - Offset 0x00.
            // Bit 0 = IP (Interrupt Pending, RW1C), Bit 1 = IE (Interrupt Enable).
            // Set IE (bit 1); leave IP (bit 0) clear so we don't acknowledge a stale event.
            // QEMU only asserts the IRQ when IMAN.IP & IMAN.IE & USBCMD.INTE all hold, so
            // USBCMD.INTE must also be set (done in start()).
            let iman_ptr = (ir0_base + 0x00) as *mut u32;
            let iman = core::ptr::read_volatile(iman_ptr);
            core::ptr::write_volatile(iman_ptr, (iman & !0x1) | 0x2);

            serial_println!("xHCI: Interrupter 0 enabled (IMAN.IE set, interrupt-driven).");
        }
    }

    pub fn start(&mut self) {
        // PIUSB-10: if CNR never cleared, do NOT set CONFIG/RS=1 on a not-ready controller — fail
        // loud and skip so RS never latches into a controller that dropped its ring/interrupter setup.
        if !XHCI_CNR_OK.load(Ordering::Acquire) {
            serial_println!("xHCI: start SKIPPED — controller never left Not-Ready (CNR=1); RS=1 not issued");
            return;
        }
        unsafe {
            // Program CONFIG.MaxSlotsEn (op_base + 0x38, bits 7:0) BEFORE Run, while the
            // controller is still halted. Without this the controller has zero usable
            // device slots and every Enable Slot command fails.
            let config_ptr = (self.op_base + 0x38) as *mut u32;
            let config = core::ptr::read_volatile(config_ptr);
            core::ptr::write_volatile(config_ptr, (config & !0xFF) | (self.max_slots as u32));
            serial_println!("xHCI: CONFIG register set to {} (MaxSlotsEn).", self.max_slots);

            // Write USBCMD: bit 0 = RS (Run/Stop), bit 2 = INTE (Interrupter Enable).
            // INTE is the global gate for host-system interrupts; without it QEMU never
            // asserts the IRQ regardless of IMAN.IE (xhci_intr_raise requires both).
            //
            // aarch64 (any target, not just Tegra): publish the CPU-prepared DMA structures
            // (DCBAA, scratchpad array+buffers, ERST) BEFORE the Run bit — the controller may
            // fetch them the moment RS lands, and plain Normal-memory stores are not ordered
            // against this Device-nGnRE write (same Normal->Device gap as `ring_doorbell_asm`;
            // `dsb st` = Linux `__iowmb`). x86: no-op.
            #[cfg(target_arch = "aarch64")]
            core::arch::asm!("dsb st", options(nostack, preserves_flags));
            let usbcmd_ptr = self.op_base as *mut u32;
            let cmd = core::ptr::read_volatile(usbcmd_ptr);
            core::ptr::write_volatile(usbcmd_ptr, cmd | 0b101);

            // Wait until USBSTS.HCH (Halted) is 0
            let usbsts_ptr = (self.op_base + 0x04) as *const u32;
            let _ = wait_until(
                || (core::ptr::read_volatile(usbsts_ptr) & 1) == 0,
                hw_wait_budget(), "USBSTS.HCH=0 (run)");
            serial_println!("xHCI: Controller Started!");
            // BPACE (M4): RS=1 latched and USBSTS.HCH cleared — the controller is RUNNING. `d=`
            // from `xhci-ptrs` is CONFIG.MaxSlotsEn plus the run handshake, budget-bounded.
            crate::bootpace::record("xhci-run");

            // PIUSB-9 witness (aarch64-only, minimal — x86 behaviour byte-identical): dump the
            // controller-side view of the command ring + interrupter/event ring the instant RS
            // latched. This is the "at RS=1" snapshot hypothesis (c) reads off. The VL805's
            // ENABLE_SLOT never completes on metal (watchdog cmd=0x2002240 in heap PA); before we
            // even doorbell a command, prove the poll-path plumbing the controller must fetch from:
            //   * CRCR.CRR (bit3): 0 here is expected (ring idle, no command pushed yet); it should
            //     go 1 once the first doorbell lands — the enable-slot watchdog witness reads it there.
            //   * ERSTSZ==1 / ERSTBA==erst_table_phys / ERDP==event_ring_phys: if any reads back 0 or
            //     a value we never wrote, the interrupter is misprogrammed and NO event can post on the
            //     polled path regardless of IMAN — that is hypothesis (c). IMAN.IE is irrelevant to
            //     polling but dumped so a masked-interrupter VL805 quirk is visible if it gates posting.
            //   * DCBAAP readback: the controller DMA-reads this at the first slot command; a 0/torn
            //     value would fail ENABLE_SLOT downstream.
            // All heap pointers must be < 4 GiB and VA==PA (identity map) for the RC_BAR2 inbound
            // window (RAM@0, 4 GiB) to translate them — the audit confirmed the heap is at PA
            // 0x0200_0000..0x0500_0000, so these readbacks double as an inbound-window sanity check.
            #[cfg(target_arch = "aarch64")]
            {
                // PIUSB-21: read 64-bit regs as two 32-bit loads so the witness prints the
                // TRUE hi:lo (a single ldr mirrors lo->hi through the brcmstb RC). The raw
                // single-load readback is dumped alongside so the mirror is still visible.
                let crcr = read_reg64((self.op_base + 0x18) as *const u64);
                let dcbaap = read_reg64((self.op_base + 0x30) as *const u64);
                let dcbaap_raw = core::ptr::read_volatile((self.op_base + 0x30) as *const u64);
                let usbcmd_rb = core::ptr::read_volatile(usbcmd_ptr);
                let usbsts_rb = core::ptr::read_volatile(usbsts_ptr);
                serial_println!(
                    "xHCI: [aarch64] RS=1 witness: USBCMD={:#x}(RS={} INTE={}) USBSTS={:#x}(HCH={} HSE={} CNR={} HCE={}) CRCR={:#018x}(CRR={} CS={} CA={} RCS={}) DCBAAP={:#018x} DCBAAP_raw64={:#018x}",
                    usbcmd_rb, usbcmd_rb & 1, (usbcmd_rb >> 2) & 1,
                    usbsts_rb, usbsts_rb & 1, (usbsts_rb >> 2) & 1, (usbsts_rb >> 11) & 1, (usbsts_rb >> 12) & 1,
                    crcr, (crcr >> 3) & 1, (crcr >> 1) & 1, (crcr >> 2) & 1, crcr & 1, dcbaap, dcbaap_raw
                );
                let ir0 = XHCI_IR0_BASE.load(Ordering::Acquire);
                if ir0 != 0 {
                    let iman = core::ptr::read_volatile(ir0 as *const u32);
                    let erstsz = core::ptr::read_volatile((ir0 + 0x08) as *const u32);
                    let erstba = read_reg64((ir0 + 0x10) as *const u64);
                    let erdp = read_reg64((ir0 + 0x18) as *const u64);
                    serial_println!(
                        "xHCI: [aarch64] RS=1 witness: IR0={:#x} IMAN={:#x}(IP={} IE={}) ERSTSZ={} ERSTBA={:#018x} ERDP={:#018x}(EHB={}) ERST[0].ring={:#018x}",
                        ir0, iman, iman & 1, (iman >> 1) & 1, erstsz, erstba, erdp, (erdp >> 3) & 1,
                        core::ptr::read_unaligned(core::ptr::addr_of!(ERST_TABLE.entries[0].ring_address))
                    );
                }
            }

            // Power on all ports. Use the REAL MaxPorts (HCSPARAMS1 bits 24:31),
            // captured as self.max_ports. The previous code read bits 0:7, which is
            // MaxSlots (64 here) — powering 64 nonexistent ports.
            let max_ports = self.max_ports;
            serial_println!("xHCI: Max Ports = {}", max_ports);

            // CCSTRIM: remember, per port, whether WE applied VBUS here (PP was 0) or found it
            // already on. This is the discriminator the CCSMARGIN line was missing, and it decides
            // what the settle's clock even means for that port — see `SETTLE_MS` below.
            let mut pp_applied = [false; 256];
            for i in 1..=max_ports {
                let port_offset = 0x400 + (i as usize - 1) * 0x10;
                let portsc_ptr = (self.op_base + port_offset) as *mut u32;
                let status = core::ptr::read_volatile(portsc_ptr);

                // Bit 9: PP (Port Power)
                if (status & (1 << 9)) == 0 {
                    serial_println!("xHCI: Powering on Port {}", i);
                    core::ptr::write_volatile(portsc_ptr, status | (1 << 9));
                    pp_applied[i as usize] = true;
                } else {
                    serial_println!("xHCI: Port {} already powered. Status: {:#x}", i, status);
                }
            }
            // BPACE (M4): every root port is powered — the SETTLE'S OWN START. This is the stamp
            // the pre-M4 ledger lacked: without it `xhci-settle`'s `d=` was measured from
            // `pci-usb`, i.e. from a tag recorded AFTER it, and silently contained the entire
            // PCI/EHCI/handoff/reset chain instead of the settle constant. `d=` from `xhci-run` is
            // the PP write loop (one MMIO read + at most one write per port, ~0 ms); `d=` on the
            // NEXT line is now the settle and nothing else.
            crate::bootpace::record("xhci-portpwr");

            // Settle before sampling CCS. A boot-owned USB3 device whose SuperSpeed link dropped on
            // the controller reset (HCRST) needs time to re-train (RxDetect -> Polling -> U0) after
            // its port is powered; the old code read CCS immediately, so a still-training SS link was
            // missed and the device never queued/enumerated. USB2 keyboard/mouse re-detect fast
            // enough to be caught without this — a real USB3 stick was not.
            //
            // BOOTPACE M4: 500 ms -> SETTLE_MS (150 ms), and the constant stops being derived from
            // `hw_wait_budget()`. Two separate changes, both required:
            //
            //   * TIMEBASE. `hw_wait_budget()/4` tied a SPEC number to a POLICY number — exactly
            //     what `cycles_per_ms`'s own doc-comment forbids ("the settles below are SPEC
            //     numbers ... tying one to the other would silently rescale every USB timing
            //     constant the day the timeout policy changed"). It also made the settle's true
            //     wall clock arch-dependent and unprintable: ~500 ms on a calibrated x86, ~694 ms
            //     on the Pi's fixed 150 M-tick guess. Derived from `cycles_per_ms()` it is the same
            //     honest 150 ms everywhere, and `settle_ms=` below can state it without lying.
            //   * LENGTH. USB3 link training reaches U0 in tens of ms typically; the spec's outer
            //     bound is tPollingLFPSTimeout = 360 ms, and that bound is enforced BELOW by
            //     POLLING_DECIDE_MS rather than by padding this settle. So the fast, common case
            //     (link already trained, or a USB2-only machine) pays 150 ms instead of 500 ms,
            //     while a link genuinely still in Polling is given the full spec window before
            //     anything is concluded about it.
            //
            // If the link is still not up when the CCS scan runs, the device is not lost: the CAS /
            // warm-reset rungs immediately below, and the hot-plug CSC path (a late 0->1 CCS edge
            // latches CSC after this scrub and queues the port through `handle_port_status`), both
            // catch it. It enumerates LATER, not never — and "the boot device arrived via the
            // CSC/warm-reset path instead of the initial scan" is the metal tell-tale of a
            // regression here (see usb_xhci.md §2d).
            //
            // CCSTRIM (2026-08-01): 150 ms -> 100 ms, and the number stops being a USB3 guess and
            // becomes the USB 2.0 attach allowance, which is the only external standard that binds
            // this wait at all.
            //
            //   * WHAT THE WAIT ACTUALLY COVERS. Two independent phenomena were folded into one
            //     constant. (a) USB 2.0: VBUS reaching operating level on a port we just powered,
            //     then the device pulling up its speed resistor — TSIGATT, "signalling attach",
            //     which USB 2.0 Table 7-14 caps at 100 ms from VBUS_min. (b) USB 3 link training,
            //     RxDetect -> Polling -> U0, whose outer bound is tPollingLFPSTimeout = 360 ms
            //     (USB 3.2 §6.9). M4 already moved (b) out of here: POLLING_DECIDE_MS below is
            //     defined as 360 − SETTLE_MS, so the USB3 verdict window is 360 ms from port power
            //     NO MATTER what this constant is, and shortening the settle cannot shorten it.
            //     (b) therefore places NO floor here. Only (a) does.
            //
            //   * THE FLOOR IS 100, AND IT IS TSIGATT. A spec-conformant USB 2.0 device may take
            //     its full 100 ms to signal attach after VBUS is valid. Setting this below 100
            //     would be DESIGNING the initial scan to miss a conformant device — a decision no
            //     measurement of one machine's device population can license. 150 carried 50 ms
            //     above that floor as an unstated VBUS-ramp allowance; the ramp only exists for
            //     ports we actually power, and the witness below now prints which ports those were,
            //     so the allowance is no longer paid blind on every boot by every machine.
            //
            //   * WHY NOT LOWER, GIVEN THE MEASUREMENT. GR11's metal reading (rMBP, the
            //     instrument's first ever) was `latest=21 margin_ms=129` against 150 — the last
            //     port to connect did so 21 ms in, 7x inside budget. That invites a much deeper
            //     cut, and the cost model forbids it. A port timed at `t`: if t <= SETTLE_MS it is
            //     caught by the initial scan and, being a boot-scan entry, SKIPS the 100 ms
            //     connect debounce, so enumeration starts at SETTLE_MS. If t > SETTLE_MS it falls
            //     to the CSC/hot-plug path, which is NOT a boot-scan entry and therefore pays the
            //     full TATTDB debounce, so enumeration starts at t + 100. Undershooting by one
            //     millisecond costs a hundred. The trim is worth at most (150 − SETTLE_MS) and
            //     risks +100 whenever the real population's tail exceeds the new value, so the
            //     asymmetry alone rules out chasing the 21.
            //
            //   * n=1, AND THE READING DOES NOT DISCRIMINATE. One boot, one machine, one set of
            //     attached devices. Worse, `latest=21` alone cannot say whether the settle's clock
            //     started at a VBUS transition at all: if firmware had already powered that port,
            //     TSIGATT expired long before we arrived and 21 ms is measuring something else
            //     entirely. That is why `pp=` and the late-assert detector are added in the same
            //     change as the trim — the next capture answers it, this one could not.
            const SETTLE_MS: u64 = 100;
            let per_ms = Self::cycles_per_ms();
            let settle_start = crate::arch::now_cycles();
            let settle = SETTLE_MS * per_ms;

            // CCSMARGIN — measure the phenomenon the constant above covers.
            //
            // Until this arc, NOTHING anywhere recorded WHEN CCS actually asserts. Every capture on
            // either arch showed only that CCS *had* asserted by the end of the settle; the sole
            // failure signal was the boot device arriving late through the CSC/warm-reset path — a
            // pass/fail tell-tale, after the fact, with no headroom number. So neither seat could say
            // whether 150 ms has comfortable margin or is one slow device away from missing a port,
            // and the two seats' risks are not even the same question: x86 asks "has a USB3 link
            // finished Polling", the Pi asks "has the VL805's firmware booted far enough to present
            // its root ports" (observed bring-up costs in the hundreds of ms). One constant covers
            // both, and neither had been measured.
            //
            // So: sample PORTSC once per millisecond inside the wait and remember, per port, the
            // elapsed millisecond at which CCS first reads 1. Bounded by construction — at most one
            // read per NOT-YET-asserted port per sample, at most SETTLE_MS samples, and a port drops
            // out of the sweep the moment it asserts.
            //
            // This changes NOTHING about the settle. The `while` condition, its timebase and its
            // exit are byte-identical to M4's; only the busy-wait's body does work it used to spend
            // in `spin_loop()`. There is no second timebase: `per_ms` is the same
            // `cycles_per_ms()` the settle itself is built from.
            //
            // Readings (the instrument-baseline law — these three must differ, or the witness proves
            // nothing):
            //   * healthy — every physically connected port carries a small `first_assert_ms` and
            //     `margin_ms` is comfortably positive. `p1:0` is COMMON AND BENIGN: it means the port
            //     already read CCS=1 at settle entry, i.e. the device was attached before we powered
            //     the port and never needed the wait at all. That is why `none` and `0` are separate
            //     states here — 0 is a real measurement, and this project has twice been bitten by an
            //     instrument that conflated "measured zero" with "never measured".
            //   * mechanism did not run (no device on any root port) — every port prints `none`,
            //     `latest=none`, and `margin_ms=none`. Printing 150 there would be the instrument
            //     lie: maximum apparent headroom from zero measurements.
            //   * the failure the settle exists to prevent — a port that only just made it, or did
            //     not. `margin_ms` at or below zero means the final at-deadline sweep below was the
            //     first sample to see CCS=1: no headroom at all. A port that asserts LATER than the
            //     deadline cannot be timed by an instrument that stops at the deadline, so it reads
            //     `none` — indistinguishable here from a port with nothing plugged into it, and
            //     disambiguated by the existing tell-tale (§2d): that device shows up through the
            //     CSC / warm-reset path instead of the initial scan.
            //
            // An all-ones PORTSC is discarded rather than believed: 0xFFFF_FFFF is the PCIe
            // "no response / unsupported request" pattern, not a legal PORTSC (bits 27:25 are RsvdZ),
            // and taking it at face value would report CCS=1 on a controller that has not answered —
            // the exact false positive the Pi's VL805-behind-PCIe seat is exposed to.
            const CCS_NEVER: u16 = u16::MAX;
            /// One sweep of every port that has not asserted yet. Reads PORTSC exactly as the CCS
            /// scan below does (a plain volatile load; PORTSC's RW1C bits are NOT disturbed by a
            /// read), stamps `elapsed_ms` on a first CCS=1, and drops the port from the sweep.
            fn ccs_sample(op_base: usize, max_ports: u8, elapsed_ms: u64,
                          first: &mut [u16; 256], pending: &mut u32) {
                for i in 1..=max_ports {
                    let idx = i as usize;
                    if first[idx] != CCS_NEVER {
                        continue;
                    }
                    let portsc = unsafe {
                        core::ptr::read_volatile((op_base + 0x400 + (idx - 1) * 0x10) as *const u32)
                    };
                    if portsc == u32::MAX || (portsc & 1) == 0 {
                        continue;
                    }
                    first[idx] = elapsed_ms.min(CCS_NEVER as u64 - 1) as u16;
                    *pending = pending.saturating_sub(1);
                }
            }
            let mut ccs_first = [CCS_NEVER; 256];
            let mut ccs_pending = max_ports as u32;
            let sample_iv = per_ms.max(1);
            let mut next_sample: u64 = 0;

            while crate::arch::now_cycles().wrapping_sub(settle_start) < settle {
                let elapsed = crate::arch::now_cycles().wrapping_sub(settle_start);
                if ccs_pending > 0 && elapsed >= next_sample {
                    ccs_sample(self.op_base, max_ports, elapsed / sample_iv,
                               &mut ccs_first, &mut ccs_pending);
                    next_sample = elapsed.wrapping_add(sample_iv);
                }
                core::hint::spin_loop();
            }
            // One final sweep AT the deadline, so "asserted too late" is representable at all: an
            // in-loop sample can only ever report elapsed < SETTLE_MS, i.e. a strictly positive
            // margin, and an instrument that cannot print its own failure is not an instrument. This
            // costs one MMIO read per still-unasserted port (microseconds) and cannot shift the
            // millisecond-resolution `d=` of `xhci-settle` below.
            {
                let elapsed = crate::arch::now_cycles().wrapping_sub(settle_start);
                if ccs_pending > 0 {
                    ccs_sample(self.op_base, max_ports, elapsed / sample_iv,
                               &mut ccs_first, &mut ccs_pending);
                }
            }
            // CCSTRIM: the deadline is no longer the end of the measurement. Every port the settle
            // did NOT see is armed here; its first connect edge afterwards is timed and reported by
            // `handle_port_status`, which is the number that says how long the settle should have
            // been on THIS machine. Stored before the scan below so an edge landing during the
            // Polling debounce (up to 360 − SETTLE_MS ms of it) is already covered.
            CCS_SETTLE_START.store(settle_start, Ordering::Relaxed);
            CCS_SETTLE_MS_LIVE.store(SETTLE_MS, Ordering::Relaxed);
            for i in 1..=max_ports {
                CCS_UNSEEN[i as usize]
                    .store(ccs_first[i as usize] == CCS_NEVER, Ordering::Relaxed);
            }
            CCS_LATE_ARMED.store(true, Ordering::Release);
            serial_println!("xHCI: port settle complete before CCS scan (settle_ms={})", SETTLE_MS);
            // BPACE: the fixed pre-enumeration settle (`hw_wait_budget()/4` — ~0.5 s of wall clock
            // and nothing else).
            //
            // M4 correction: this comment used to claim `d=` here was "measured from `pci-usb`",
            // which is impossible — `pci-usb` is recorded LATER, after `pci::init` returns. The
            // ledger's `d=` is always the delta from the PREVIOUS stamp, and before M4 the previous
            // stamp was `smp`: a single 7289 ms bucket that contained the scheduler, the tick-rate
            // window, the EHCI HID bring-up, two PCI config-space walks, the BIOS→OS handoff, the
            // halt/HCRST/CNR chain, the ring programming AND the settle. Nobody could aim a trim at
            // it. With `xhci-portpwr` recorded immediately above, `d=` here is the settle constant
            // alone — which is what a settle-trim arc must have measured before it may touch the
            // number. Absent on a `skip_xhci` build, with every pre-USB tag still present.
            crate::bootpace::record("xhci-settle");

            // CCSMARGIN witness. Emitted AFTER `xhci-settle` on purpose: this line is ~100 characters
            // of UART, and the ledger's `d=` for `xhci-settle` is M4's measurement of the settle
            // constant ALONE. Printing before the stamp would fold this instrument's own serial cost
            // into the number it exists to justify — the recorder reporting itself.
            //
            // `margin_ms` = SETTLE_MS − latest, signed, raw. That is the whole point of the arc: it
            // turns "did it work" into "by how much", and a zero or negative value is the finding.
            // No BPACE tag rides along. The ring stores (cycle, tag) and a stamp's value IS the
            // instant `record()` was called, so the latest assert — which is only identifiable after
            // every port has been swept — cannot be stamped at the moment it happened. A stamp
            // placed here instead would read ~SETTLE_MS on every boot forever: the recorder, not the
            // phenomenon. The number lives in this line, and M4's ring arithmetic (n=31 of CAP=64)
            // is left exactly as it was — `dropped=` stays 0.
            //
            // CCSTRIM adds two things to it, both because the line is now the justification for a
            // TRIMMED constant rather than a description of a padded one:
            //
            //   * `/on` vs `/pre` per port — did WE apply VBUS to this port (PP was 0, so the
            //     settle's clock starts at a real power transition and USB 2.0's 100 ms TSIGATT
            //     allowance is genuinely running), or was it already powered by firmware (VBUS has
            //     been valid for a long time, TSIGATT expired before we existed, and a nonzero
            //     `first_assert_ms` is measuring something other than attach signalling)? Without
            //     it, `latest=21` is uninterpretable: on a `/pre` port it says the settle covers a
            //     phenomenon nobody has named, and on an `/on` port it says a conformant device
            //     used a fifth of its allowance. GR11's reading could not distinguish them, which
            //     is precisely why this arc does not cut below the spec floor.
            //   * a DISTINCT RESULT TOKEN when the margin is gone. A shrinking `margin_ms` is one
            //     integer in a hundred-character line and a capture-grep for it must already know
            //     what "too small" is. `result=CCSMARGIN-TIGHT` / `result=CCSMARGIN-BLOWN` are
            //     states, not numbers — and together with `result=CCSMARGIN-LATE` from the
            //     detector armed above, `result=CCSMARGIN-` matches every failure of this trim and
            //     nothing on a healthy boot.
            {
                use core::fmt::Write as _;
                let mut latest: Option<(u16, u8)> = None;
                let mut line = alloc::string::String::new();
                let _ = write!(line, "xHCI: ccs-margin settle_ms={} ports={} first_assert_ms=[",
                               SETTLE_MS, max_ports);
                for i in 1..=max_ports {
                    if i > 1 {
                        let _ = write!(line, " ");
                    }
                    let pp = if pp_applied[i as usize] { "on" } else { "pre" };
                    match ccs_first[i as usize] {
                        CCS_NEVER => {
                            let _ = write!(line, "p{}:none/{}", i, pp);
                        }
                        v => {
                            let _ = write!(line, "p{}:{}/{}", i, v, pp);
                            if latest.is_none_or(|(l, _)| v > l) {
                                latest = Some((v, i));
                            }
                        }
                    }
                }
                match latest {
                    Some((l, port)) => {
                        let margin = SETTLE_MS as i64 - l as i64;
                        let _ = write!(line, "] latest={} margin_ms={} result=CCSMARGIN", l, margin);
                        serial_println!("{}", line);
                        // The negative case, stated as a state and not left as a small integer.
                        // BLOWN: the at-deadline sweep was the first sample to see CCS=1 — this
                        // port reached the initial scan with no headroom at all, and one slower
                        // boot puts it past the deadline entirely. TIGHT: still positive, but
                        // inside a fifth of the budget, which on a constant this arc just cut to
                        // the USB 2.0 floor means the floor is where the population actually
                        // lives. Both are findings; only BLOWN is a failure.
                        if margin <= 0 {
                            serial_println!(
                                "xHCI: !! ccs-margin BLOWN port={} latest={} settle_ms={} margin_ms={} \
                                 (deadline sweep was the first CCS=1; no headroom) result=CCSMARGIN-BLOWN",
                                port, l, SETTLE_MS, margin);
                        } else if margin <= (SETTLE_MS / 5) as i64 {
                            serial_println!(
                                "xHCI: !! ccs-margin TIGHT port={} latest={} settle_ms={} margin_ms={} \
                                 (under a fifth of budget left) result=CCSMARGIN-TIGHT",
                                port, l, SETTLE_MS, margin);
                        }
                    }
                    // No port ever asserted: nothing was measured, so there is no margin to report.
                    None => {
                        let _ = write!(line, "] latest=none margin_ms=none result=CCSMARGIN");
                        serial_println!("{}", line);
                    }
                }
            }

            // Scrub any latched PORTSC change bits before enumeration: one latched bit gates
            // PSCEG (4.19.2) and blocks ALL Port Status Change events for that port, so a
            // leftover from firmware/HCRST would make our own reset completions invisible.
            // While scanning, revive USB3 links that only a WARM reset can recover:
            //   - CAS=1 (Cold Attach Status, bit 24): far-end terminations were detected while
            //     the port could not handle the attach — exactly what our HCRST does to the
            //     firmware-trained boot port. The port parks at CCS=0 (often RxDetect) until a
            //     warm reset; hot reset cannot clear CAS (4.19.8). This is the "boot device
            //     flaky-absent" signature on the rMBP.
            //   - PLS Compliance(10)/Inactive(6): error states a hot reset cannot exit.
            //   - PLS Polling(7), CAS=0, CCS=0: the Intel Panther Point stuck-in-Polling
            //     erratum (Intel's documented workaround is a warm port reset) — debounced
            //     with a second read ~100 ms later, since a healthy just-attached device can
            //     legitimately transit Polling.
            // The warm resets are fire-and-forget: the CSC/PRC they produce flows through the
            // hot-plug path and queues the port once its link trains.
            let mut polling_candidates: Vec<u8> = Vec::new();
            for i in 1..=max_ports {
                let s = self.read_portsc(i);
                let changes = s & PORT_CHANGE_BITS;
                if changes != 0 {
                    self.clear_port_change(i, changes);
                }
                if self.port_major(i) != 3 {
                    continue;
                }
                let pls = (s >> 5) & 0xF;
                let cas = (s & (1 << 24)) != 0;
                if cas || ((s & 1) == 0 && (pls == 6 || pls == 10)) {
                    serial_println!(
                        "xHCI: USB3 port {} needs a WARM reset (CAS={} PLS={} PORTSC={:#010x}).",
                        i, cas as u8, pls, s);
                    self.write_portsc(i, (1 << 9) | (1u32 << 31));
                } else if (s & 1) == 0 && pls == 7 {
                    polling_candidates.push(i);
                }
            }
            if !polling_candidates.is_empty() {
                // BOOTPACE M4: this debounce now carries the spec floor the settle used to pad.
                // tPollingLFPSTimeout (USB 3.2 §6.9) is 360 ms: a HEALTHY link may legitimately sit
                // in Polling that long, so no port may be DECLARED stuck in Polling — and warm-reset
                // out of a legal state — before 360 ms have passed since its power was applied. The
                // old pairing satisfied that only by accident (500 ms settle + 100 ms = 600 ms).
                // Here it is explicit: the debounce is whatever is left of the 360 ms window after
                // the settle, so shortening the settle cannot shorten the Polling verdict.
                //
                // The device also gets a second chance from it: a link that finishes training
                // during this window reads CCS=1 at the re-check, is NOT warm-reset, and is picked
                // up by the CCS scan below in the ordinary way.
                const POLLING_DECIDE_MS: u64 = 360;
                let dbc_start = crate::arch::now_cycles();
                let dbc = POLLING_DECIDE_MS.saturating_sub(SETTLE_MS) * per_ms;
                while crate::arch::now_cycles().wrapping_sub(dbc_start) < dbc {
                    core::hint::spin_loop();
                }
                for i in polling_candidates {
                    let s = self.read_portsc(i);
                    if (s & 1) == 0 && (s >> 5) & 0xF == 7 && (s & (1 << 24)) == 0 {
                        serial_println!("xHCI: USB3 port {} stuck in Polling (debounced); warm-resetting.", i);
                        self.write_portsc(i, (1 << 9) | (1u32 << 31));
                    }
                }
            }

            // Collect every connected port and enumerate them ONE AT A TIME. Push in
            // reverse so the queue pops in ascending port order.
            self.ports_to_enumerate.clear();
            self.boot_scan_ports.clear();
            for i in (1..=max_ports).rev() {
                let port_offset = 0x400 + (i as usize - 1) * 0x10;
                let portsc_ptr = (self.op_base + port_offset) as *const u32;
                let status = core::ptr::read_volatile(portsc_ptr);

                // Bit 0: CCS (Current Connect Status)
                if (status & 1) != 0 {
                    serial_println!("xHCI: Port {} connected (Status: {:#x}); queued for enumeration.", i, status);
                    // CCSTRIM: the initial scan HAS this port, so it never fell to the recovery
                    // path and owes no late report — disarm it. (A USB3 link that finished
                    // training during the Polling debounce lands here: the settle was short for
                    // it, but it cost nothing, and `CCSMARGIN-LATE` must mean "the scan missed a
                    // port", not "a timer was tight". Its own CSC arrives later and is swallowed
                    // by the active-device case in `handle_port_status`.)
                    CCS_UNSEEN[i as usize].store(false, Ordering::Relaxed);
                    self.ports_to_enumerate.push(i);
                    // BOOTPACE M4: mark this as an INITIAL-SCAN entry. Its connection has been
                    // electrically stable since before we powered the port — the boot device was
                    // attached before the machine was — and it has additionally been powered and
                    // sampled across the settle above. TATTDB's intent is already met, so
                    // `start_next_port` skips the 100 ms connect debounce for it. Hot-plug entries
                    // never land in this list and keep the full debounce.
                    self.boot_scan_ports.push(i);
                }
            }
        }

        // Kick off enumeration of the first connected port (outside the unsafe block).
        self.start_next_port();
    }

    /// Decode + dispatch a root port's latched change bits — from a Port Status Change event
    /// OR the main-loop polling backstop (`service_enum`). Snapshots PORTSC once, W1C-clears
    /// EVERY set change bit immediately, then dispatches on the snapshot. Clearing everything
    /// (not just the bits we act on) is load-bearing: see PORT_CHANGE_BITS — one latched
    /// OCC/PLC/CEC starves all future events for the port on real hardware.
    fn handle_port_status(&mut self, port_id: u8) {
        // Bounds: the id may come straight from a Port Status Change event TRB (controller-
        // provided, same trust model as the slot-id guards). port 0 would underflow the
        // PORTSC offset math AND collide with the "no port" sentinel in enumerating_port;
        // port > MaxPorts would read/W1C-write MMIO beyond the port register array.
        if port_id == 0 || port_id > self.max_ports {
            serial_println!("xHCI: port status change with bogus port {}; ignoring.", port_id);
            return;
        }
        let portsc = self.read_portsc(port_id);
        let changes = portsc & PORT_CHANGE_BITS;
        if changes == 0 {
            return;
        }
        self.clear_port_change(port_id, changes);
        if changes & !((1 << 17) | (1 << 19) | (1 << 21)) != 0 {
            serial_println!(
                "xHCI: [Port {}] change bits {:#x} cleared (PORTSC={:#010x}).",
                port_id, changes, portsc);
        }

        // PRC (21) / WRC (19): a reset completed. A warm reset asserts BOTH on completion
        // (xHCI 4.19.5.1), a hot reset just PRC — treat them as one "reset done" signal.
        if changes & ((1 << 21) | (1 << 19)) != 0 {
            let ped = (portsc & 2) != 0;
            if port_id == self.enumerating_port {
                // Idempotency: only the await-reset stage consumes a reset completion. Real
                // silicon can post PRC and WRC in separate events (or replay a leftover PRC);
                // firing enable_slot twice would double-push pending_ports and desync the
                // command dispatch.
                if self.enum_stage == "await-reset" {
                    if ped {
                        // Reset succeeded. Don't touch the device yet: USB demands ~10 ms of
                        // reset-recovery (TRSTRCY) before it must accept transactions, and
                        // Linux waits ~50 ms. service_enum() issues ENABLE_SLOT after the gate.
                        serial_println!("xHCI: [Port {}] reset complete (PED=1); settling before enable.", port_id);
                        self.set_enum_stage("reset-settle");
                    } else {
                        // PRC/WRC with PED=0 = the reset positively FAILED (4.19.5): recover
                        // now (retry escalates to warm on USB3) instead of burning a watchdog
                        // period waiting for a slot request that can never be made.
                        serial_println!("xHCI: [Port {}] reset completed with PED=0 (failed).", port_id);
                        self.recover_enumeration("reset-failed", 0);
                    }
                } else {
                    serial_println!(
                        "xHCI: [Port {}] reset change in stage '{}'; ignoring (duplicate).",
                        port_id, self.enum_stage);
                }
            } else if ped && (portsc & 1) != 0 {
                // A reset completed on a port we are NOT enumerating (a boot-scan warm reset
                // finishing during another port's enumeration, or a device-initiated reset).
                // Treat like hot-plug, with the same dedupe + has-slot filters as CSC below.
                let has_slot = self.slots.iter().enumerate()
                    .any(|(i, s)| i != 0 && s.active && s.port_id == port_id);
                if !has_slot && !self.ports_to_enumerate.contains(&port_id) {
                    serial_println!("xHCI: [Port {}] unsolicited reset complete; queuing for enumeration.", port_id);
                    self.ports_to_enumerate.push(port_id);
                    if !self.enum_active {
                        self.start_next_port();
                    }
                }
            }
        }

        // CSC (17): connect status changed — hot-plug, disconnect, or a reset side-effect.
        // We queue the port and let the serialized start_next_port() reset/enumerate it —
        // only kicking it here if no enumeration is in flight (else the in-flight device's
        // completion drains the queue), so the one-at-a-time invariant holds.
        if changes & (1 << 17) != 0 {
            if (portsc & 1) != 0 {
                // CCSTRIM: before any policy runs on this edge — is this the connect the pre-scan
                // settle was too short to wait for? `CCS_UNSEEN` was armed at the settle's end for
                // every port it never saw and cleared for every port the initial scan did find, so
                // a surviving flag means exactly one thing: this port missed the scan and is being
                // recovered by the path §2d promises will recover it. The elapsed time is the only
                // direct measurement of how long `SETTLE_MS` should have been on this machine, and
                // it is taken here because an instrument that stops at the deadline can never
                // produce it. Reported once per port; disarmed either way, so a hot-plug hours into
                // uptime cannot resurrect the token.
                if CCS_LATE_ARMED.load(Ordering::Acquire)
                    && CCS_UNSEEN[port_id as usize].swap(false, Ordering::Relaxed)
                {
                    let t_ms = crate::arch::now_cycles()
                        .wrapping_sub(CCS_SETTLE_START.load(Ordering::Relaxed))
                        / Self::cycles_per_ms();
                    let settle_ms = CCS_SETTLE_MS_LIVE.load(Ordering::Relaxed);
                    if t_ms <= CCS_LATE_WINDOW_MS {
                        serial_println!(
                            "xHCI: !! ccs-margin LATE port={} t_ms={} settle_ms={} short_by_ms={} \
                             (missed the initial CCS scan; recovered via CSC) result=CCSMARGIN-LATE",
                            port_id, t_ms, settle_ms, t_ms.saturating_sub(settle_ms));
                    }
                }
                // CONNECT edge (CCS=1). Three cases to tell apart (M1 / XENUM-1):
                //   (a) the reset artifact — the USB reset WE issue for `enumerating_port` asserts
                //       CSC while CCS stays 1 throughout. Never disturbed the connection, so no
                //       CCS=0 edge preceded it (`enum_saw_disconnect` false). SWALLOW — re-queuing
                //       it would re-reset the port we are mid-resetting and loop forever.
                //   (b) a genuine re-plug DURING this port's own enumeration — the device left
                //       (a CCS=0 edge set `enum_saw_disconnect`) and came back. Can't reset it now
                //       (enum in flight), so DEFER: re-queue once the in-flight enum settles.
                //   (c) a genuine hot-plug on an idle port — queue and kick.
                // The disconnect teardown below (CCS=0 branch) disposes an unplugged device's slot,
                // so a re-plug of an already-enumerated device reaches (c) with `has_slot` false
                // instead of being dropped as a "reset side-effect" (the metal rMBP failure).
                let mid_enum = port_id == self.enumerating_port;
                let has_slot = self.slots.iter().enumerate()
                    .any(|(i, s)| i != 0 && s.active && s.port_id == port_id);
                if mid_enum {
                    if self.enum_saw_disconnect {
                        // (b) genuine reconnect mid-enumeration — defer, do not swallow.
                        self.enum_saw_disconnect = false;
                        if !self.requeue_after_settle.contains(&port_id) {
                            self.requeue_after_settle.push(port_id);
                        }
                        serial_println!("xHCI: [Port {}] reconnect during enumeration; deferring re-queue until it settles.", port_id);
                    } else {
                        // (a) reset side-effect — CCS never dropped; swallow (loop guard).
                        serial_println!("xHCI: [Port {}] CSC during enumeration (reset side-effect, CCS stable); not re-queuing.", port_id);
                    }
                } else if has_slot {
                    // A CSC on a still-present, already-enumerated device with no disconnect edge:
                    // a spurious connect-change (bounce that never dropped CCS). Re-enumerating a
                    // live device would disrupt it; leave it be.
                    serial_println!("xHCI: [Port {}] connect-change on an active device (no disconnect); not re-queuing.", port_id);
                } else {
                    // (c) genuine hot-plug on an idle port.
                    serial_println!("xHCI: [Port {}] device connected (hot-plug); queuing for enumeration.", port_id);
                    if !self.ports_to_enumerate.contains(&port_id) {
                        self.ports_to_enumerate.push(port_id);
                    }
                    if !self.enum_active {
                        self.start_next_port();
                    }
                }
            } else {
                // DISCONNECT edge (CCS=0). Tear down any slot bound to this port so a later re-plug
                // enumerates as a fresh connect (case (c) above) instead of being dropped. If the
                // disconnect is on the port we are actively enumerating, don't fight the in-flight
                // FSM (recover_enumeration owns that slot) — just arm the deferred re-queue so the
                // device isn't lost if it comes back before the enum gives up.
                if port_id == self.enumerating_port {
                    self.enum_saw_disconnect = true;
                    serial_println!("xHCI: [Port {}] device left during its own enumeration; will re-queue after settle.", port_id);
                } else {
                    let disposed = self.dispose_disconnected_slots(port_id);
                    serial_println!("xHCI: [Port {}] device disconnected ({} slot(s) torn down).", port_id, disposed);
                }
            }
        }
    }

    /// M1 (XENUM-1): a device on `port` reported a genuine DISCONNECT (CSC with CCS=0). Tear down
    /// every slot bound to that port so a subsequent hot re-plug is seen as a fresh connect
    /// (`has_slot` honest) rather than dropped as a "reset side-effect". Before this a disconnect
    /// left the slot active, so re-plugging an already-enumerated device (metal rMBP: unplug+replug
    /// a mouse) hit the has_slot guard and never re-enumerated — the only workaround was a
    /// power-cycle with the device pre-plugged. Mirrors the recovery disposal (storage/FTDI/HID
    /// bindings cleared, DISABLE_SLOT queued) but, because the device is physically gone, disposes
    /// even a "ready" storage slot (recovery's paranoia guard protects a live device across a
    /// transient stall; a disconnect is not transient). Returns the number of slots torn down.
    fn dispose_disconnected_slots(&mut self, port: u8) -> usize {
        let mut n = 0usize;
        for i in 1..self.slots.len() {
            if !self.slots[i].active || self.slots[i].port_id != port {
                continue;
            }
            if self.configuring_slot == i as u8 {
                self.configuring_slot = 0;
            }
            if self.storage_slot == i as u8 {
                self.storage_slot = 0;
                self.storage_pending_bringup = false;
                self.storage_note = "storage device disconnected";
            }
            // USB-UNPLUG: the xHCI-level teardown above only clears the CONTROLLER's binding. The
            // block registry this device published into on attach (`block::publish_usb_geometry`,
            // called from the SCSI bring-up) is a separate global, and until this call it kept the
            // dead disk forever — so the installer's per-frame disk list went on offering an
            // unplugged stick as an install target. Retract by slot id, unconditionally for every
            // disposed slot rather than only when `storage_slot` still points here: the registry
            // itself decides whether this slot was the storage device (a non-storage slot simply
            // doesn't match), which keeps the retraction correct even if some earlier path already
            // zeroed `storage_slot`. A replug enumerates as a NEW slot and republishes through the
            // normal attach path, so the entry is fresh rather than duplicated.
            crate::drivers::block::unpublish_usb_geometry(i as u8);
            // BOT-RESCUE: a slot that leaves takes its escalation state with it. Without this a
            // surrendered slot id, once recycled by the controller for the NEXT device, would
            // refuse that innocent device's transfers up front — the surrender must bind to the
            // disk that earned it, not to a number.
            self.bot_rescue_clear(i as u8);
            if self.ftdi_configuring_slot == i as u8 {
                self.ftdi_configuring_slot = 0;
            }
            if self.ftdi_slot == i as u8 {
                self.ftdi_slot = 0;
                self.ftdi_pending_bringup = false;
                self.ftdi_pending = None;
                ftdi::set_live(false);
            }
            self.hid_setproto_pending.retain(|s| *s != i as u8);
            self.hid_halt_pending.retain(|(s, _)| *s != i as u8);
            self.hubs_pending.retain(|s| *s != i as u8);
            self.hub_changes_pending.retain(|(hs, _)| *hs != i as u8); // XENUM-2
            self.pending_ports.retain(|p| *p != port);
            self.slots[i].reset_soft_state();
            if !self.slots_to_disable.iter().any(|(s, _)| *s == i as u8) {
                self.slots_to_disable.push((i as u8, 0));
            }
            serial_println!("xHCI: [Port {}] slot {} torn down on disconnect; queued for DISABLE_SLOT.", port, i);
            n += 1;
        }
        n
    }

    /// Begin enumerating the next queued connected port. Called at boot and again each
    /// time a device finishes its setup, so at most one port is mid-enumeration.
    fn start_next_port(&mut self) {
        // BPACE: close the ledger entry for the port the FSM is LEAVING. `start_next_port` is the
        // single funnel through which the root enumeration FSM releases a port — reached from every
        // configure-complete branch (storage, FTDI, HID) and from `recover_enumeration`'s give-up
        // path — so one stamp here covers all of them, where stamping the individual
        // Configure-Endpoint branches would have missed HID (whose enumeration continues through
        // SET_CONFIGURATION) and every failure. `-done` therefore means "the FSM left this port",
        // success or surrender; the outcome is on the neighbouring xHCI lines, the TIME is here.
        if self.enumerating_port != 0 {
            crate::bootpace::record_port(self.enumerating_port, true);
        }
        // M1 (XENUM-1): fold in any port that asked to be re-enumerated once the in-flight
        // enumeration settled (a genuine re-plug that arrived mid-enumeration). Draining here —
        // the single point where the FSM goes idle — keeps the one-port-at-a-time invariant.
        if !self.requeue_after_settle.is_empty() {
            let deferred = core::mem::take(&mut self.requeue_after_settle);
            for port in deferred {
                // If the in-flight enumeration that was flapping actually SUCCEEDED (the device
                // returned fast enough to bind a slot), the port now has an active slot. Re-
                // enumerating it as-is would allocate a SECOND slot + device context for the one
                // device (rings mem::forget'd) — a leak. Dispose the stale slot first, exactly as
                // the CCS=0 hot-plug path relies on dispose to make `has_slot` false, so the
                // re-enumeration lands on a single clean slot.
                let has_slot = self.slots.iter().enumerate()
                    .any(|(i, s)| i != 0 && s.active && s.port_id == port);
                if has_slot {
                    let disposed = self.dispose_disconnected_slots(port);
                    serial_println!("xHCI: [Port {}] deferred re-plug: disposed {} stale slot(s) before re-enumeration.", port, disposed);
                }
                if !self.ports_to_enumerate.contains(&port) {
                    serial_println!("xHCI: [Port {}] re-queuing deferred hot re-plug for enumeration.", port);
                    self.ports_to_enumerate.push(port);
                }
            }
        }
        while let Some(port) = self.ports_to_enumerate.pop() {
            // BOOTPACE M4: consume this port's initial-boot-scan mark, if it has one. Consumed on
            // POP rather than on use, so the `continue` below (port no longer connected) also
            // clears it: a port that has to come back through CSC is a genuine attach and must pay
            // the full debounce.
            let boot_scan = match self.boot_scan_ports.iter().position(|&p| p == port) {
                Some(i) => {
                    self.boot_scan_ports.remove(i);
                    true
                }
                None => false,
            };
            let portsc = self.read_portsc(port);
            if (portsc & 1) == 0 {
                // Not connected — but a USB3 link stuck in an error state reads CCS=0 too
                // (Compliance Mode is CCS=0 by definition, 4.19.1.2), and CAS=1 means a
                // cold-attached device is waiting for the WARM reset only we can issue.
                // Kick the link and move on; the resulting CSC re-queues the port.
                let pls = (portsc >> 5) & 0xF;
                let cas = (portsc & (1 << 24)) != 0;
                if self.port_major(port) == 3 && (cas || pls == 6 || pls == 10) {
                    serial_println!(
                        "xHCI: Port {} disconnected but link stuck (CAS={} PLS={}); warm-resetting.",
                        port, cas as u8, pls);
                    self.write_portsc(port, (1 << 9) | (1u32 << 31));
                } else {
                    serial_println!("xHCI: Port {} no longer connected; skipping.", port);
                }
                continue;
            }
            self.enum_active = true;
            self.enumerating_port = port;
            self.enum_saw_disconnect = false; // M1: fresh disconnect tracking per enumeration
            self.enum_resets = 1;
            serial_println!("xHCI: === Enumerating Port {} (PORTSC={:#x}) ===", port, portsc);
            // BPACE: this port's enumeration begins. Paired with the `-done` stamp at the top of
            // this function, `d=` across the pair is the per-port enumeration cost — the number that
            // says whether the boot's USB time is one slow device or the debounce paid N times.
            crate::bootpace::record_port(port, false);
            // Debounce BEFORE the first reset (USB 2.0 TATTDB: 100 ms of stable connection
            // after attach). The metal rMBP bench captured a hot-plugged High-Speed SD reader
            // that, reset immediately on the connect event, trained at Full-Speed (failed HS
            // chirp) and then failed every ADDRESS_DEVICE with USB Transaction Error (code 4)
            // — resetting a device whose attach hasn't electrically settled is the classic
            // cause. service_enum issues the reset once the gate expires. (BOOTPACE M4: ports
            // queued by the INITIAL boot CCS scan are exempt — see the `boot_scan` branch below.)
            //
            // The reset itself is always issued whether or not PED is already set. A device
            // the firmware enumerated (our USB stick / SD reader IS the UEFI boot device) —
            // and every SuperSpeed device, whose link auto-trains to PED=1 — keeps its old
            // USB address across our controller reset (HCRST resets the controller, not the
            // device). ADDRESS_DEVICE issues SET_ADDRESS to the Default address, so the
            // device must be in Default state, which a USB reset restores. The Port Reset
            // Change event then drives the rest.
            //
            // BOOTPACE M4: the comment above used to end "boot-scan devices (long since stable)
            // just pay the same 100 ms, which is harmless". It is not free — it is 100 ms per
            // boot-scan port on the critical path to the first console and the first block read —
            // and it is not needed for them either. TATTDB (USB 2.0 §7.1.7.3) asks for 100 ms of
            // STABLE ATTACH before the reset; a port queued by the initial CCS scan was attached
            // before the machine was powered, was seen connected after `start()` powered it, and
            // was held across the pre-scan settle. Its debounce has already been served by
            // wall-clock that is not ours to charge twice, so it goes straight to the reset.
            //
            // HOT-PLUG PORTS KEEP THE FULL 100 ms. That path is metal-proven, not theoretical: a
            // hot-plugged High-Speed SD reader reset immediately on the connect event trained at
            // Full-Speed (failed HS chirp) and then failed every ADDRESS_DEVICE with code 4. The
            // 50 ms TRSTRCY reset-settle is untouched on both paths.
            self.enum_cmd_phys = 0;
            if boot_scan {
                serial_println!(
                    "xHCI: [enum port {}] initial boot scan — attach already stable through the settle; skipping the 100 ms connect debounce.",
                    port);
                self.issue_enum_reset(port);
            } else {
                self.set_enum_stage("debounce");
            }
            return;
        }
        self.enum_active = false;
        self.enumerating_port = 0;
        self.enum_cmd_phys = 0;
        self.enum_resets = 0;
        self.set_enum_stage("idle");
        serial_println!("xHCI: Port enumeration queue drained.");
    }

    /// Unwedge and advance the enumeration FSM after the current port's enumeration FAILED —
    /// a command completed with an error, an EP0 transfer errored/STALLed, or the watchdog
    /// expired with no completion at all. Before this existed, ANY of those silently deadlocked
    /// the whole queue (`enum_active=true queued=N` forever — the exact state photographed on
    /// the rMBP when the SuperSpeed SD reader stalls). Records the stall for `usbinfo`, cleans
    /// every piece of in-flight bookkeeping so late completions can't alias into the FSM,
    /// disposes any half-built slot, then either RETRIES the port with a fresh reset (bounded)
    /// or gives up and starts the next queued port.
    ///
    /// Safe from both event context and the main loop: it issues no synchronous commands
    /// (slot disposal is deferred to `service_slot_disposal`) and only writes PORTSC.
    fn recover_enumeration(&mut self, why: &'static str, code: u8) {
        let port = self.enumerating_port;
        if port == 0 {
            return;
        }
        let portsc = self.read_portsc(port);
        serial_println!(
            "xHCI: !!! ENUM RECOVERY: port {} failed at '{}' ({}, code {}) PORTSC={:#010x} !!!",
            port, self.enum_stage, why, code, portsc);
        self.last_stall = Some(EnumStall { port, stage: self.enum_stage, why, code, portsc });
        self.stall_count += 1;

        // Metal diagnostic (rMBP bench 2026-07-08): ADDRESS_DEVICE failing with USB
        // Transaction Error while a USB2 port reads Full-Speed usually means a High-Speed
        // device whose HS chirp failed during reset — name the pattern so the serial capture
        // says what happened instead of just "code 4".
        if code == 4 && self.port_major(port) == 2 && ((portsc >> 10) & 0xF) == 1 {
            serial_println!(
                "xHCI: [recovery] hint: port {} trained at Full-Speed; if this device is High-Speed \
                 the HS chirp failed — the paced retry re-resets it.", port);
        }

        // Clean the in-flight bookkeeping.
        self.pending_ports.retain(|p| *p != port);
        self.enum_cmd_phys = 0;

        // Dispose any slot allocated for this port. Mid-enumeration it is by definition not
        // fully configured — but never touch a published, ready storage slot (paranoia guard).
        for i in 1..self.slots.len() {
            if !self.slots[i].active || self.slots[i].port_id != port {
                continue;
            }
            if self.storage_slot == i as u8 && self.storage_note == "ready" {
                continue;
            }
            if self.configuring_slot == i as u8 {
                self.configuring_slot = 0;
            }
            if self.storage_slot == i as u8 {
                self.storage_slot = 0;
                self.storage_pending_bringup = false;
                self.storage_note = "storage slot disposed after an enumeration stall";
            }
            // U2.5: mirror the storage clears for the FTDI console fields. Without this, a disposed
            // slot id that later gets REUSED (hot-plug) would still match the stale
            // `ftdi_configuring_slot`/`ftdi_slot` — the Configure-Endpoint completion dispatch checks
            // `ftdi_configuring_slot == slot_id` BEFORE the HID branch, so a reused slot would be
            // misrouted into the FTDI console path and its real HID/MSC setup skipped. Guarded on the
            // disposed slot `i`, so a healthy console on another slot is never disturbed.
            if self.ftdi_configuring_slot == i as u8 {
                self.ftdi_configuring_slot = 0;
            }
            if self.ftdi_slot == i as u8 {
                self.ftdi_slot = 0;
                self.ftdi_pending_bringup = false;
                self.ftdi_pending = None;
                ftdi::set_live(false); // the console's slot is torn down — stop the drain
            }
            self.hid_setproto_pending.retain(|s| *s != i as u8);
            self.hid_halt_pending.retain(|(s, _)| *s != i as u8);
            self.hubs_pending.retain(|s| *s != i as u8);
            self.hub_changes_pending.retain(|(hs, _)| *hs != i as u8); // XENUM-2
            self.slots[i].reset_soft_state();
            if !self.slots_to_disable.iter().any(|(s, _)| *s == i as u8) {
                self.slots_to_disable.push((i as u8, 0));
            }
            serial_println!("xHCI: [recovery] slot {} (port {}) queued for DISABLE_SLOT.", i, port);
        }

        // Retry with a fresh reset (bounded), or give up and advance the queue. Retry when the
        // device is still connected, OR when the USB3 link is in a state only a warm reset can
        // recover (Compliance reads CCS=0). The retry reset itself is PACED: Linux spaces
        // recovery attempts 100-200 ms apart because a device still finishing its own reset
        // recovery fails an immediate retry for the same transient reason; service_enum()
        // issues the actual reset once the "retry-wait" gate expires.
        let pls = (portsc >> 5) & 0xF;
        let cas = (portsc & (1 << 24)) != 0;
        let link_recoverable = self.port_major(port) == 3 && (cas || pls == 6 || pls == 10);
        if ((portsc & 1) != 0 || link_recoverable) && self.enum_resets < 3 {
            self.enum_resets += 1;
            serial_println!(
                "xHCI: [recovery] retrying port {} (reset {} of 3) after a settle.",
                port, self.enum_resets);
            self.enum_cmd_phys = 0;
            self.set_enum_stage("retry-wait");
        } else {
            serial_println!("xHCI: [recovery] giving up on port {}; advancing the queue.", port);
            // The final verdict, photographable even after the boot log scrolls: dump the
            // topology summary (with the last-stall record) to serial on every give-up.
            for line in self.port_slot_summary() {
                serial_println!("xHCI: {}", line);
            }
            self.enumerating_port = 0;
            self.start_next_port();
        }
    }

    /// Reset `enumerating_port` (again) on behalf of the enumeration FSM: pick hot vs WARM
    /// reset, rewind the stage tracking, and make sure the completion can actually reach us.
    ///
    /// Warm reset (WPR, bit 31) is USB3-only. The first attempt mirrors Linux (hot reset
    /// first — a hot reset already returns the device to Default state, USB3 7.5.12) UNLESS
    /// the link is in a state only a warm reset can leave: CAS=1 (cold attach), SS.Inactive,
    /// or Compliance Mode. Retries escalate to warm on USB3 ports.
    fn issue_enum_reset(&mut self, port: u8) {
        let portsc = self.read_portsc(port);
        // Never overlap resets: a reset is still in progress (PR reads 1 for hot AND warm,
        // and WPR always reads 0). Restarting one is undefined (Table 5-27 note: reset
        // protocols "are not designed to be interrupted"); a WPR write now would likely be
        // swallowed, silently losing the retry. Rewind to await-reset and let the polling
        // backstop / watchdog pick up the completion of the reset already running.
        if (portsc & (1 << 4)) != 0 {
            serial_println!("xHCI: [enum port {}] reset already in progress; waiting for it.", port);
            self.enum_cmd_phys = 0;
            self.set_enum_stage("await-reset");
            return;
        }
        // Clear any latched change bits FIRST: with any bit set, PSCEG never re-arms and the
        // PRC/WRC completion of the reset we are about to issue would never generate an event.
        let changes = portsc & PORT_CHANGE_BITS;
        if changes != 0 {
            self.clear_port_change(port, changes);
        }

        let usb3 = self.port_major(port) == 3;
        let pls = (portsc >> 5) & 0xF;
        let cas = (portsc & (1 << 24)) != 0;
        let warm = usb3 && (cas || pls == 6 || pls == 10 || self.enum_resets >= 2);
        self.enum_cmd_phys = 0;
        self.set_enum_stage("await-reset");
        if warm {
            serial_println!(
                "xHCI: [enum port {}] issuing WARM reset (CAS={} PLS={} attempt {}).",
                port, cas as u8, pls, self.enum_resets);
            self.write_portsc(port, (1 << 9) | (1u32 << 31));
        } else {
            serial_println!("xHCI: [enum port {}] issuing hot reset (attempt {}).", port, self.enum_resets);
            self.write_portsc(port, (1 << 9) | (1 << 4));
        }
    }

    /// Main-loop hook: DISABLE_SLOT any slots torn down by enumeration recovery (synchronous —
    /// must run in the safe polled context, like storage/hub bring-up). The DCBAA entry is
    /// cleared ONLY after a successful disable: the controller dereferences it while the slot
    /// is enabled (4.5.1), and zeroing it under a live slot risks a Host System Error on this
    /// silicon. Failures are retried a bounded number of times, then the slot is leaked
    /// inside the controller — harmless (MaxSlots is 32+ and recovery is rare).
    pub fn service_slot_disposal(&mut self) {
        if self.slots_to_disable.is_empty() {
            return;
        }
        let work = core::mem::take(&mut self.slots_to_disable);
        for (slot, attempts) in work {
            // The slot id may have been re-issued to a LIVE device since this entry was
            // queued (a timed-out DISABLE_SLOT can still have executed in hardware, freeing
            // the id for the retry enumeration). Never disable a slot that is active again;
            // if it truly needs tearing down, recovery will re-queue it.
            if self.slots[slot as usize].active {
                serial_println!("xHCI: DISABLE_SLOT slot {} skipped (id re-allocated and live).", slot);
                continue;
            }
            let trb = Trb {
                parameter: 0,
                status: 0,
                control: (10 << 10) | ((slot as u32) << 24), // TRB type 10 = Disable Slot
            };
            match self.run_command_sync(trb) {
                Ok((code, _)) => {
                    serial_println!("xHCI: DISABLE_SLOT slot {} -> code {}.", slot, code);
                    if code == 1 {
                        unsafe {
                            if !self.dcbaap.is_null() {
                                *self.dcbaap.add(slot as usize) = 0;
                                // XHCI-COHERENCE: clean the cleared DCBAA entry so the controller
                                // sees the slot released, not a stale output-context pointer. No-op x86.
                                dma_coherency::clean(
                                    self.dcbaap.add(slot as usize) as usize,
                                    core::mem::size_of::<u64>(),
                                );
                            }
                        }
                    }
                }
                Err(()) => {
                    if attempts + 1 < 3 {
                        self.slots_to_disable.push((slot, attempts + 1));
                    } else {
                        serial_println!(
                            "xHCI: DISABLE_SLOT slot {} failed {} times; leaking the slot.",
                            slot, attempts + 1);
                    }
                }
            }
        }
    }

    /// Main-loop hook: drive the timed stages of the root enumeration FSM and watchdog the
    /// rest. Every stage transition stamps `enum_stage_set_at`; this uses that clock to
    ///   - advance "reset-settle" (TRSTRCY: >=50 ms device recovery after a reset before we
    ///     request a slot) and "retry-wait" (>=200 ms pacing between recovery attempts),
    ///   - POLL PORTSC at "await-reset": Port Status Change event delivery has no guarantee
    ///     (a latched change bit gates PSCEG, 4.19.2), so a reset completion whose event was
    ///     lost is picked up here directly instead of stalling out,
    ///   - declare a stage STUCK past its deadline and recover (before this existed, one
    ///     device that never answered deadlocked every port queued behind it, invisibly).
    /// Command stages get ~1 s (a healthy step takes microseconds; Linux allows 5 s but has
    /// users to wait for); EP0 descriptor stages get ~2 s (control transfers may legally be
    /// slow). A wedged command is ABORTED (CRCR.CA handshake) before recovery so the command
    /// ring unblocks.
    pub fn service_enum(&mut self) {
        if !self.enum_active || self.enumerating_port == 0 {
            return;
        }
        let port = self.enumerating_port;
        let age = crate::arch::now_cycles().wrapping_sub(self.enum_stage_set_at);
        let per_ms = (hw_wait_budget() / 2000).max(1); // hw_wait_budget ≈ 2000 ms of cycles

        match self.enum_stage {
            "reset-settle" => {
                if age >= hw_wait_budget() {
                    // Backstop: nothing should sit here (enable_slot either advances the
                    // stage or recovers on failure) — but NO stage may age unwatched.
                    self.recover_enumeration("watchdog-timeout", 0);
                } else if age >= 50 * per_ms {
                    serial_println!("xHCI: [enum port {}] settle done; requesting slot.", port);
                    self.enable_slot(port);
                }
            }
            "debounce" => {
                // Connect debounce (USB 2.0 TATTDB, 100 ms) before the first reset — see
                // start_next_port. Always advances, so no separate watchdog is needed.
                // BOOTPACE M4: only HOT-PLUG ports reach this stage now; initial-boot-scan ports go
                // straight to `issue_enum_reset` because their attach was already stable across the
                // pre-scan settle. This 100 ms is the metal-proven SD-reader FS-chirp fix and stays
                // exactly as it was for every port that genuinely just arrived.
                if age >= 100 * per_ms {
                    self.issue_enum_reset(port);
                }
            }
            "retry-wait" => {
                // Recovery pacing, ESCALATING per attempt (200/400/600 ms): the bench's
                // hot-plugged SD reader failed the same way at a fixed 200 ms spacing —
                // a device still settling from its own failed handshake needs longer, and
                // extra wait on an already-failed port costs nothing.
                if age >= 200 * (self.enum_resets as u64).max(1) * per_ms {
                    self.issue_enum_reset(port);
                }
            }
            "await-reset" => {
                // Polling backstop for a lost/suppressed Port Status Change event.
                let portsc = self.read_portsc(port);
                if portsc & ((1 << 21) | (1 << 19)) != 0 {
                    serial_println!(
                        "xHCI: [enum port {}] reset change latched but no event was delivered; polling fallback.",
                        port);
                    self.handle_port_status(port);
                } else if age >= hw_wait_budget() / 2 {
                    self.recover_enumeration("watchdog-timeout", 0);
                }
            }
            "enable-slot" | "address-device" | "configure-eps" => {
                if age >= hw_wait_budget() / 2 {
                    serial_println!(
                        "xHCI: WATCHDOG: port {} stuck at '{}' (cmd={:#x}).",
                        port, self.enum_stage, self.enum_cmd_phys);
                    // XHCI-COHERENCE (was JB3 boot-8, tegra-only; now general aarch64): on a
                    // non-coherent bus the SMMU/RC passes our stream fault-free yet no event appears —
                    // distinguish "the controller's DMA write never reached DRAM" from "it landed but
                    // the CPU's cached line is stale". `has_event()` now invalidates the dequeue TRB's
                    // line before its read on ALL aarch64 targets (the unified `dma_coherency` seam
                    // that replaced `has_event_after_invalidate`), so this watchdog line simply
                    // re-checks post-invalidate. Behaviour on tegra is identical to the old hack; the
                    // Pi 4 (VL805) now gets the same forensic line.
                    #[cfg(target_arch = "aarch64")]
                    {
                        // PIUSB-9 witness (hypotheses b/d): read the command ring's controller-side
                        // state AT the stall. The doorbell was rung when the command was pushed; by now
                        // a healthy controller has fetched the TRB and posted a completion. CRCR.CRR
                        // (bit3) is the discriminator:
                        //   * CRR=1  -> the controller ACCEPTED the doorbell and the ring is RUNNING; it
                        //              fetched (or is fetching) the ENABLE_SLOT TRB but never posted the
                        //              completion event => a DMA/event-posting fault (hypothesis b: the
                        //              completion write never reached DRAM / event ring), NOT a dead ring.
                        //   * CRR=0  -> the controller NEVER started fetching: either the doorbell write
                        //              was not observed, or the controller is running dead firmware that
                        //              accepts MMIO but processes nothing (hypothesis d). CS/CA would only
                        //              be set by an abort handshake we have not issued yet, so both 0 here.
                        // enum_cmd_phys is the pushed TRB's PA (heap, <4 GiB); print it beside CRCR so the
                        // log ties the stuck command to the ring the controller is (or isn't) fetching.
                        let (crcr, usbcmd, usbsts) = unsafe {
                            (
                                read_reg64((self.op_base + 0x18) as *const u64),
                                core::ptr::read_volatile(self.op_base as *const u32),
                                core::ptr::read_volatile((self.op_base + 0x04) as *const u32),
                            )
                        };
                        serial_println!(
                            "xHCI: [aarch64] enable-slot stall witness: cmd_trb={:#x} CRCR={:#018x}(CRR={} CS={} CA={} RCS={}) USBCMD={:#x}(RS={}) USBSTS={:#x}(HCH={} HSE={} CNR={} HCE={}) => {}",
                            self.enum_cmd_phys, crcr, (crcr >> 3) & 1, (crcr >> 1) & 1, (crcr >> 2) & 1, crcr & 1,
                            usbcmd, usbcmd & 1,
                            usbsts, usbsts & 1, (usbsts >> 2) & 1, (usbsts >> 11) & 1, (usbsts >> 12) & 1,
                            if (crcr >> 3) & 1 == 1 { "CRR=1 ring RUNNING — fetched, no completion posted (DMA/event fault, hyp b)" }
                            else { "CRR=0 ring NEVER STARTED — doorbell not observed or dead fw (hyp d)" }
                        );
                        let landed = {
                            EVENT_RING
                                .lock()
                                .as_ref()
                                .map(|r| r.has_event())
                                .unwrap_or(false)
                        };
                        serial_println!(
                            "xHCI: [aarch64] event ring after dc-civac: {}",
                            if landed {
                                "EVENT PRESENT — writes LAND, CPU snoop broken (coherency)"
                            } else {
                                "still empty — writes never reach DRAM"
                            }
                        );
                    }
                    // Unwedge the command ring first: the xHC executes commands in order, so
                    // the hung command blocks everything (including slot disposal) until it
                    // is aborted. The abort pump may itself deliver the failure completion
                    // and run recovery; only recover here if the FSM didn't move.
                    let advanced = self.abort_enum_command();
                    if !advanced {
                        self.recover_enumeration("watchdog-timeout", 0);
                    }
                }
            }
            "dev-desc" | "cfg-desc" | "set-config" => {
                if age >= hw_wait_budget() {
                    self.recover_enumeration("watchdog-timeout", 0);
                }
            }
            // JB10 (Tegra): learn a Full-Speed device's MPS0 in place then read the full
            // descriptor — done synchronously here in ONE pass (sync_control/run_command_sync are
            // safe in this main-loop context) and it transitions the stage on the way out (to
            // dev-desc via request_device_descriptor, or via recover_enumeration on failure), so it
            // never lingers unwatched. begin_device_descriptor only sets this stage on tegra.
            #[cfg(feature = "tegra")]
            "fs-mps-learn" => {
                match self.slots.iter().position(|s| s.active && !s.is_downstream && s.port_id == port) {
                    Some(slot_id) => self.fs_learn_mps0(slot_id as u8, port),
                    None => self.recover_enumeration("fs-mps-no-slot", 0),
                }
            }
            // Invariant: with enum_active set, EVERY stage has a deadline. An unknown stage
            // here would be a bug — don't let it become an unwatchable parked state.
            _ => {
                if age >= hw_wait_budget() {
                    self.recover_enumeration("watchdog-timeout", 0);
                }
            }
        }
    }

    /// Abort the enumeration FSM's wedged in-flight command — the Linux-mirroring CRCR.CA
    /// handshake (xHCI 4.6.1.2). Safe ONLY from the main-loop context (it pumps the event
    /// ring). Returns true if the FSM advanced during the pump (a completion arrived after
    /// all — success or failure — and was dispatched normally), in which case the caller
    /// must NOT run its own recovery on top.
    ///
    /// Order matters, and each step exists because the naive version corrupts the ring:
    ///  1. Compose the CA write as ONE 64-bit value carrying a VALID ring pointer. CRCR's
    ///     pointer field always reads 0 (5.4.5), so a read-modify-write arms a null dequeue
    ///     pointer if the ring happens to stop as the write lands — the pre-2021 Linux abort
    ///     bug (ff0e50d3564f). We point at the aborted TRB itself, with its own cycle bit as
    ///     RCS: harmless if ignored (CRR=1), correct if accepted (we no-op that TRB below).
    ///  2. Poll CRR -> 0 (bounded), pumping events: the Command Aborted completion (code 25,
    ///     param = the aborted TRB) flows through the normal failure dispatch -> recovery.
    ///  3. Overwrite the aborted TRB in place with a Command No-Op (type 23), because the
    ///     stopped ring's dequeue pointer still references it and a doorbell restart would
    ///     RE-EXECUTE the very command that wedged (Linux trb_to_noop does exactly this).
    ///  4. Only then restart the ring (doorbell 0). `cmd_ring_stopped` blocks every other
    ///     path from pushing/doorbelling mid-abort.
    fn abort_enum_command(&mut self) -> bool {
        let aborted = self.enum_cmd_phys;
        if aborted == 0 {
            return false;
        }
        let stage_stamp = self.enum_stage_set_at;
        let crcr_ptr = (self.op_base + 0x18) as *mut u64;
        let crr_set = unsafe { (read_reg64(crcr_ptr) >> 3) & 1 != 0 };
        if crr_set {
            self.cmd_ring_stopped = true;
            let rcs = COMMAND_RING.lock().as_ref()
                .map(|r| r.trb_cycle(aborted))
                .unwrap_or(1) as u64;
            unsafe {
                write_reg64(crcr_ptr, aborted | rcs | (1 << 2));
            }
            serial_println!("xHCI: command abort issued (CRCR.CA, aborted TRB {:#x}).", aborted);
            let start = crate::arch::now_cycles();
            let budget = hw_wait_budget(); // ~2 s; Linux allows 5 s, but this is boot-critical
            loop {
                while self.drain_event_ring_once() {}
                let crr = unsafe { (read_reg64(crcr_ptr) >> 3) & 1 };
                if crr == 0 {
                    break;
                }
                if crate::arch::now_cycles().wrapping_sub(start) >= budget {
                    // The controller never stopped the ring: it is effectively dead for
                    // commands. Leave cmd_ring_stopped set so nothing pushes/doorbells into
                    // it, and let recovery give up on ports as their watchdogs expire.
                    serial_println!("xHCI: !!! command abort TIMED OUT (CRR stuck); command ring is dead. !!!");
                    return self.enum_stage_set_at != stage_stamp;
                }
                core::hint::spin_loop();
            }
        }
        // Ring stopped. Drain whatever completions the stop produced (Command Aborted for our
        // TRB, Command Ring Stopped), then defuse the aborted TRB and restart.
        while self.drain_event_ring_once() {}
        if let Some(ring) = COMMAND_RING.lock().as_mut() {
            if !ring.replace_with_noop(aborted) {
                serial_println!("xHCI: WARNING: aborted TRB {:#x} not found in the command ring.", aborted);
            }
        }
        self.cmd_ring_stopped = false;
        self.ring_doorbell(0, 0);
        serial_println!("xHCI: command ring restarted after abort.");
        self.enum_stage_set_at != stage_stamp
    }

    pub fn diagnose_command_ring(&self, original_ptr: u64) {
        unsafe {
            // 1. READ CRCR (Command Ring Control Register)
            // Offset 0x18 from OpBase
            let crcr_reg = (self.op_base + 0x18) as *const u64;
            let crcr_raw = read_reg64(crcr_reg);

            // Mask bits 0-5 to get the pointer (address is 64-byte aligned, so low 6 bits are flags)
            let crcr_ptr = crcr_raw & !0x3F;

            serial_println!("xHCI DEBUG: CRCR State Analysis");
            serial_println!("   Started At: {:#x}", original_ptr);
            serial_println!("   Current:    {:#x}", crcr_ptr);
            serial_println!("   Raw CRCR:   {:#x}", crcr_raw);

            if crcr_ptr == original_ptr {
                serial_println!("   CONCLUSION: STALLED. Hardware never fetched the command.");
                serial_println!("   POSSIBLE CAUSES: Doorbell missed, Cycle Bit mismatch, or Bad Address.");
            } else {
                serial_println!("   CONCLUSION: EXECUTED. Hardware moved past the command.");
                serial_println!("   ISSUE: Event Ring lost the receipt.");
            }
        }
    }

    pub fn check_vitals(&mut self) {
        unsafe {
            // 1. CHECK USBSTS (USB Status Register)
            // Offset 0x04 from Operational Base
            let usbsts_ptr = (self.op_base + 0x04) as *const u32;
            let usbsts = core::ptr::read_volatile(usbsts_ptr);

            serial_println!("xHCI DEBUG: USBSTS = {:#x}", usbsts);
            if (usbsts & (1 << 12)) != 0 { serial_println!("   CRITICAL: HCE (Host Controller Error) SET!"); }
            if (usbsts & (1 << 2)) != 0 { serial_println!("   CRITICAL: HSE (Host System Error) SET!"); }

            // 2. CHECK DOORBELL ACCESSIBILITY (The "Cliff" Test)
            // We try to READ the Doorbell register.
            // Even though it's Write-Only, reading it should NOT crash if mapped.
            // If this causes a Page Fault, we know the mapping is too small.
            let db_ptr = (self.base_addr + 0x2000) as *mut u32; // DBOFF is assumed 0x2000 for this test
            serial_println!("xHCI DEBUG: Testing Doorbell Memory Access at {:#p}...", db_ptr);

            let _probe = core::ptr::read_volatile(db_ptr);
            serial_println!("xHCI DEBUG: Doorbell Memory is Accessible. (Value: {:#x})", _probe);

            // 3. CHECK COMMAND WRAPPER
            // Ensure we are writing 32-bits, not 64-bits.
            // Doorbell registers are strictly 32-bit.
            core::ptr::write_volatile(db_ptr, 0);
            serial_println!("xHCI DEBUG: Doorbell 0 (Target 0) manually written.");
        }
    }

    pub fn run(&mut self) {
        unsafe {
            // 1. READ MAX SLOTS (HCSPARAMS1 is Offset 0x04 from CAPABILITY BASE)
            let hcsparams1_ptr = (self.base_addr + 0x04) as *const u32;
            let hcsparams1 = core::ptr::read_volatile(hcsparams1_ptr);
            let max_slots = hcsparams1 & 0xFF; // Bits 0-7

            serial_println!("xHCI: Hardware supports {} Device Slots.", max_slots);

            // 2. WRITE CONFIG REGISTER (Offset 0x38 from OPERATIONAL BASE)
            // Bits 0-7: MaxSlotsEn
            let config_ptr = (self.op_base + 0x38) as *mut u32;
            core::ptr::write_volatile(config_ptr, max_slots);

            serial_println!("xHCI: CONFIG register set to {}.", max_slots);

            // 3. RUN
            let usbcmd_ptr = self.op_base as *mut u32;
            let usbsts_ptr = (self.op_base + 0x04) as *const u32;

            serial_println!("xHCI: Starting Engine (INTERRUPTS DISABLED)...");
            let cmd = core::ptr::read_volatile(usbcmd_ptr);
            // UNA-19-POLLING: Clear Bit 2 (INTE) to disable interrupts (Polling Mode)
            // Set Bit 0 (Run)
            core::ptr::write_volatile(usbcmd_ptr, (cmd & !(1 << 2)) | 1);

            // POLL: Wait for HCHalted (Bit 0 in Status) to CLEAR.
            // This confirms the hardware is executing.
            let _ = wait_until(
                || (core::ptr::read_volatile(usbsts_ptr) & 1) == 0,
                hw_wait_budget(), "USBSTS.HCH=0 (run)");
            serial_println!("xHCI: ENGINE RUNNING (HCHalted cleared).");
        }
    }

    pub fn enable_slot(&mut self, port_id: u8) {
        serial_println!("xHCI: Sending ENABLE_SLOT command for Port {}...", port_id);

        // TRB Type 9 = Enable Slot
        // Control: (Type 9 << 10)
        // Cycle Bit is handled by the Ring.
        let trb = Trb {
            parameter: 0,
            status: 0,
            control: (9 << 10),
        };

        match self.send_command(trb) {
            // ENABLE_SLOT is issued only by the root enumeration FSM (the hub path uses the
            // sync-command primitive), so track it unconditionally — there is no slot id yet
            // to route through track_enum_cmd. pending_ports is pushed ONLY on a successful
            // send: this is called every service_enum tick from "reset-settle", so a
            // persistent send failure (dead/stopped command ring) would otherwise push one
            // stale entry per main-loop iteration forever.
            Ok(phys) => {
                self.pending_ports.push(port_id);
                self.enum_cmd_phys = phys;
                self.set_enum_stage("enable-slot");
            }
            Err(e) => {
                // A refused/failed send means the command ring is unusable — treat it as an
                // enumeration stall NOW (bounded retry, then give-up) rather than leaving
                // the FSM parked in a stage the watchdog can't see out of.
                serial_println!("xHCI: Failed to send Enable Slot command: {}", e);
                self.recover_enumeration("command-send-failed", 0);
                return;
            }
        }

        // Metal diagnostic (usbdebug only): a healthy controller consumes ENABLE_SLOT in
        // microseconds and writes a completion TRB to our event ring. Wait briefly (bounded by the
        // wall-clock deadline so it can't hang) to see whether the controller responded AT ALL,
        // then snapshot health. This turns the silent first-ENABLE_SLOT freeze on the real rMBP
        // into a diagnosable screen: posted=false + clean USBSTS => command/doorbell never reached
        // the controller; HCE/HSE set => it faulted on the command ring; posted=true => the
        // completion is there and the stall is in our drain.
        #[cfg(feature = "usbdebug")]
        {
            let start = crate::arch::now_cycles();
            let budget = crate::arch::hw_wait_budget() / 4;
            let mut posted = false;
            let mut errored = false;
            loop {
                let has = { EVENT_RING.lock().as_ref().map(|r| r.has_event()).unwrap_or(false) };
                if has {
                    posted = true;
                    break;
                }
                let usbsts = unsafe { core::ptr::read_volatile((self.op_base + 0x04) as *const u32) };
                if (usbsts & ((1 << 2) | (1 << 12))) != 0 {
                    errored = true;
                    break;
                }
                if crate::arch::now_cycles().wrapping_sub(start) >= budget {
                    break;
                }
                core::hint::spin_loop();
            }
            serial_println!(
                "xHCI: after ENABLE_SLOT — completion posted to event ring={} controller-error={}",
                posted, errored
            );
            self.dump_hc_health("post ENABLE_SLOT");
        }
    }
    pub fn address_device(&mut self, slot_id: u8, port_id: u8) {
        unsafe {
            serial_println!("xHCI: Addressing Device (Slot {}, Port {})...", slot_id, port_id);

            // 0. Allocate Contexts and Ring
            let input_layout = core::alloc::Layout::from_size_align(core::mem::size_of::<InputContext>(), 64).unwrap();
            let output_layout = core::alloc::Layout::from_size_align(core::mem::size_of::<DeviceContext>(), 64).unwrap();
            
            let input_ctx_virt = alloc::alloc::alloc_zeroed(input_layout) as *mut InputContext;
            let output_ctx_virt = alloc::alloc::alloc_zeroed(output_layout) as *mut DeviceContext;
            let ep0_ring = ring::TransferRing::new(16);
            let ep0_ring_phys = ep0_ring.get_ptr();

            let output_ctx_phys = output_ctx_virt as u64;
            let input_ctx_phys = input_ctx_virt as u64;

            // XHCI-COHERENCE: zeroed-handoff boundary — the controller DMA-WRITES the output (device)
            // context during ADDRESS_DEVICE; clean+invalidate so the zeros reach DRAM and the CPU's
            // later read-back observes the controller's data, not a stale zero line. No-op x86.
            dma_coherency::clean_inval(output_ctx_virt as usize, core::mem::size_of::<DeviceContext>());

            // Store them in slot
            let slot = &mut self.slots[slot_id as usize];
            slot.input_context = input_ctx_virt;
            slot.output_context = output_ctx_virt;
            slot.ep0_ring = Some(ep0_ring);
            slot.port_id = port_id;
            slot.active = true;

            // 1. LINK DCBAAP
            // Point the Slot ID entry to the Output Context
            let dcbaap_ptr = self.dcbaap;
            *dcbaap_ptr.add(slot_id as usize) = output_ctx_phys;
            // XHCI-COHERENCE: producer boundary — the controller reads DCBAA[slot] to locate the
            // output context during ADDRESS_DEVICE; clean the 8-byte entry to DRAM. No-op x86.
            dma_coherency::clean(dcbaap_ptr.add(slot_id as usize) as usize, core::mem::size_of::<u64>());
            serial_println!("xHCI: DCBAAP[{}] linked to {:#x}", slot_id, output_ctx_phys);

            // 2. FILL INPUT CONTEXT (MANUAL OFFSET CALCULATION)
            let base_ptr = input_ctx_virt as *mut u32;

            // Clear Input Context (33 contexts × CTX_WORDS × 4 bytes — stride follows HCCPARAMS1.CSZ)
            core::ptr::write_bytes(base_ptr as *mut u8, 0, core::mem::size_of::<InputContext>());

            // 3a. INPUT CONTROL CONTEXT (Offset 0x00)
            base_ptr.add(1).write_volatile(3); // Enable Slot (Bit 0) and EP0 (Bit 1)

            // 3b. SLOT CONTEXT (Offset 0x20 -> Index 8 in u32).
            // Read the port's speed (PORTSC bits 13:10 = Port Speed ID: 1=FS 2=LS 3=HS 4=SS),
            // program it into the slot context, and pick the matching EP0 Max Packet Size. Real
            // xHCI ENFORCES the EP0 MPS: a Low-Speed keyboard (MPS0=8) with the old hardcoded MPS=64
            // truncates every control read at the first 8-byte packet (garbage vid/pid, empty config
            // descriptor walk, no keyboard) — QEMU is lenient about this, which is why 64 "worked"
            // only in emulation. Mirrors the downstream (hub) path.
            let speed = (self.read_portsc(port_id) >> 10) & 0xF;
            let mps0: u32 = match speed {
                2 => 8,   // Low Speed  (bMaxPacketSize0 always 8)
                3 => 64,  // High Speed (always 64)
                4 => 512, // SuperSpeed (always 512)
                5 => 512, // SuperSpeedPlus (Tegra234 XUSB reports Gen2 root devices as PSI 5;
                          // MPS0 is 512 like SS — without this the slot context carried MPS0=8
                          // and the XUSB FW rejected ADDRESS_DEVICE with code 17, JB9 bench)
                // Full Speed / unknown: FS bMaxPacketSize0 is 8/16/32/64 and unknowable before
                // the first descriptor read. 8 is the safe first guess (a 64-MPS device answers
                // an 18-byte read with one oversized packet -> Babble, code 3); on that exact
                // failure the recovery path below flips this port to 64 and the enum FSM's
                // built-in retry re-addresses with the corrected context.
                _ => {
                    if self.fs_ep0_mps64[(port_id as usize) & 31] { 64 } else { 8 }
                }
            };
            serial_println!("xHCI: Port {} speed {} -> EP0 MPS {}", port_id, speed, mps0);
            let slot_ctx_ptr = base_ptr.add(CTX_WORDS);
            slot_ctx_ptr.add(0).write_volatile((1 << 27) | ((speed & 0xF) << 20)); // Context Entries=1 + Speed
            slot_ctx_ptr.add(1).write_volatile((port_id as u32) << 16); // Root Hub Port Number

            // 3c. ENDPOINT 0 CONTEXT (Offset 0x40 -> Index 16 in u32)
            let ep0_ctx_ptr = base_ptr.add(2 * CTX_WORDS);
            ep0_ctx_ptr.add(1).write_volatile((4 << 3) | (3 << 1) | (mps0 << 16)); // EP Type = 4, CErr = 3, MPS
            ep0_ctx_ptr.add(2).write_volatile((ep0_ring_phys as u32) | 1); // Bit 0 must match Cycle Bit (1)
            ep0_ctx_ptr.add(3).write_volatile((ep0_ring_phys >> 32) as u32);
            ep0_ctx_ptr.add(4).write_volatile(8); // Average TRB Length = 8

            serial_println!("xHCI: Input Context Initialized (Manual Offsets). Phys={:#x}", input_ctx_phys);

            // 4. SEND ADDRESS DEVICE COMMAND
            let trb = Trb {
                parameter: input_ctx_phys,
                status: 0,
                control: (11 << 10) | ((slot_id as u32) << 24),
            };

            match self.send_command(trb) {
                // Root-FSM-only (the hub path addresses via run_command_sync).
                Ok(phys) => self.track_enum_cmd(phys, "address-device"),
                Err(e) => {
                    serial_println!("xHCI: Failed to send Address Device command: {}", e);
                    self.recover_enumeration("command-send-failed", 0);
                }
            }
            // NOTE: do NOT set `configuring_slot` here. That field marks an in-flight
            // Configure-Endpoint command; setting it on Address Device made the Address
            // Device completion be misdispatched as "endpoints configured", which jumped
            // straight to SCSI read (skipping device-descriptor + endpoint setup) and
            // panicked on an unallocated data_buffer. The Address Device completion now
            // correctly falls through to request_device_descriptor().
        }
    }

    /// Build the bulk-IN/OUT input context (rings, DMA buffers, slot fields) for a
    /// Configure-Endpoint command and return the input context's physical address. Shared by
    /// the async root path (`configure_endpoints`) and the synchronous hub-downstream path
    /// (`configure_bulk_endpoints_sync`).
    fn build_bulk_input_ctx(&mut self, slot_id: u8, in_addr: u8, in_mps: u16, out_addr: u8, out_mps: u16) -> u64 {
        unsafe {
            // DCI = endpoint_number * 2 + (1 for IN, 0 for OUT).
            let in_dci = ((in_addr & 0x0F) * 2) + 1;
            let out_dci = (out_addr & 0x0F) * 2;
            serial_println!("xHCI: Configuring Bulk Endpoints for Slot {} (IN {:#x} dci{} mps{}, OUT {:#x} dci{} mps{})...",
                slot_id, in_addr, in_dci, in_mps, out_addr, out_dci, out_mps);

            // 1. GET POINTERS
            let slot = &mut self.slots[slot_id as usize];
            let input_ctx_virt = slot.input_context;
            let output_ctx_virt = slot.output_context;
            let base_ptr = input_ctx_virt as *mut u32;
            // XHCI-COHERENCE: consumer boundary — the slot context copied out of the output context
            // below was DMA-written by the controller (ADDRESS_DEVICE); invalidate so the copy reads
            // fresh, not a stale cached line. No-op x86.
            dma_coherency::inval(output_ctx_virt as usize, core::mem::size_of::<DeviceContext>());

            // ONSET-2 (M3 knob 1): `BOT_RING_TRBS` is 16 unless `botring64` is compiled in, in which
            // case it is 64. The ONLY two rings the knob touches.
            let bulk_in_ring = ring::TransferRing::new(BOT_RING_TRBS);
            let bulk_in_phys = bulk_in_ring.get_ptr();
            slot.bulk_in_ring = Some(bulk_in_ring);

            let bulk_out_ring = ring::TransferRing::new(BOT_RING_TRBS);
            let bulk_out_phys = bulk_out_ring.get_ptr();
            slot.bulk_out_ring = Some(bulk_out_ring);

            // Dedicated DMA buffers for Bulk-Only Transport (CBW / data / CSW).
            let cbw_layout = core::alloc::Layout::from_size_align(64, 64).unwrap();
            slot.cbw_buffer = Some(alloc::alloc::alloc_zeroed(cbw_layout));
            let csw_layout = core::alloc::Layout::from_size_align(64, 64).unwrap();
            slot.csw_buffer = Some(alloc::alloc::alloc_zeroed(csw_layout));
            // MULTIBLK: the data-stage staging buffer is STORAGE_DATA_BYTES (32 KiB), 64 KiB-ALIGNED.
            // The alignment is not a cache-line choice — it is what keeps a single Normal TRB from
            // ever crossing a 64 KiB boundary (xHCI 1.2 §4.11.7.1), so the audited one-TRB data stage
            // stays valid at every size up to the buffer. See STORAGE_DATA_BYTES for the full note.
            let data_layout =
                core::alloc::Layout::from_size_align(STORAGE_DATA_BYTES, STORAGE_DATA_ALIGN).unwrap();
            slot.scsi_data_buffer = Some(alloc::alloc::alloc_zeroed(data_layout));
            slot.bulk_in_ep = in_addr;
            slot.bulk_out_ep = out_addr;

            // 2. CLEAR INPUT CONTEXT (Safety first)
            core::ptr::write_bytes(base_ptr as *mut u8, 0, core::mem::size_of::<InputContext>());

            // 3. INPUT CONTROL CONTEXT (Offset 0x00): A0 (slot context) + both bulk DCIs.
            base_ptr.add(1).write_volatile(1u32 | (1 << in_dci) | (1 << out_dci));

            // 4. SLOT CONTEXT (Offset 0x20 -> Index 8)
            let slot_ctx_ptr = base_ptr.add(CTX_WORDS);
            // Copy from OUTPUT_CONTEXT
            for i in 0..8 {
                let val = core::ptr::read_volatile((output_ctx_virt as *const u32).add(i));
                slot_ctx_ptr.add(i).write_volatile(val);
            }
            // Update Context Entries (Bits 27:31) to the highest DCI in use.
            let max_dci = in_dci.max(out_dci) as u32;
            let old_dw0 = slot_ctx_ptr.add(0).read_volatile();
            let new_dw0 = (old_dw0 & !(0x1F << 27)) | (max_dci << 27);
            slot_ctx_ptr.add(0).write_volatile(new_dw0);

            // 5. BULK IN endpoint context. The DCI-th endpoint context lives at u32
            //    index 16 + (DCI - 1) * 8 in the input context.
            let ep_in_ptr = base_ptr.add((1 + in_dci as usize) * CTX_WORDS);
            ep_in_ptr.add(1).write_volatile((6 << 3) | (3 << 1) | ((in_mps as u32) << 16)); // EP Type 6 (Bulk IN), CErr 3
            ep_in_ptr.add(2).write_volatile((bulk_in_phys as u32) | 1);
            ep_in_ptr.add(3).write_volatile((bulk_in_phys >> 32) as u32);
            ep_in_ptr.add(4).write_volatile(in_mps as u32);

            // 6. BULK OUT endpoint context.
            let ep_out_ptr = base_ptr.add((1 + out_dci as usize) * CTX_WORDS);
            ep_out_ptr.add(1).write_volatile((2 << 3) | (3 << 1) | ((out_mps as u32) << 16)); // EP Type 2 (Bulk OUT), CErr 3
            ep_out_ptr.add(2).write_volatile((bulk_out_phys as u32) | 1);
            ep_out_ptr.add(3).write_volatile((bulk_out_phys >> 32) as u32);
            ep_out_ptr.add(4).write_volatile(out_mps as u32);

            serial_println!("xHCI: Input Context Configured for Bulk Transport.");
            input_ctx_virt as u64
        }
    }

    pub fn configure_endpoints(&mut self, slot_id: u8, in_addr: u8, in_mps: u16, out_addr: u8, out_mps: u16) {
        let input_ctx_phys = self.build_bulk_input_ctx(slot_id, in_addr, in_mps, out_addr, out_mps);
        let trb = Trb {
            parameter: input_ctx_phys,
            status: 0,
            control: (12 << 10) | ((slot_id as u32) << 24),
        };
        match self.send_command(trb) {
            // Root-FSM-only (the hub-downstream path configures via the sync variant below).
            Ok(phys) => self.track_enum_cmd(phys, "configure-eps"),
            Err(e) => {
                serial_println!("xHCI: Failed to send Configure Endpoint command: {}", e);
                self.recover_enumeration("command-send-failed", 0);
            }
        }
    }

    /// Synchronous Configure-Endpoint for a hub-downstream device's bulk pair. Safe ONLY from
    /// the main-loop context (`run_command_sync` pumps the event ring). Deliberately does NOT
    /// touch `configuring_slot`/`track_enum_cmd`: the async completion dispatch belongs to the
    /// root FSM, and a downstream completion routed there would advance the root port queue
    /// mid-enumeration (the exact aliasing the `is_downstream` flag exists to prevent).
    fn configure_bulk_endpoints_sync(&mut self, slot_id: u8, in_addr: u8, in_mps: u16, out_addr: u8, out_mps: u16) -> bool {
        let input_ctx_phys = self.build_bulk_input_ctx(slot_id, in_addr, in_mps, out_addr, out_mps);
        let trb = Trb {
            parameter: input_ctx_phys,
            status: 0,
            control: (12 << 10) | ((slot_id as u32) << 24),
        };
        match self.run_command_sync(trb) {
            Ok((1, _)) => true,
            Ok((c, _)) => {
                serial_println!("xHCI: downstream Configure-Endpoint code {} (slot {})", c, slot_id);
                false
            }
            Err(_) => {
                serial_println!("xHCI: downstream Configure-Endpoint timed out (slot {})", slot_id);
                false
            }
        }
    }
    /// Build a 31-byte CBW into `cbw_buf` for a Bulk-Only Transport command; returns the tag.
    fn build_cbw(&mut self, cbw_buf: *mut u8, data_len: u32, dir: Direction, cdb: &[u8]) -> u32 {
        unsafe {
            let tag = self.bot_tag;
            self.bot_tag = self.bot_tag.wrapping_add(1);
            core::ptr::write_bytes(cbw_buf, 0, 31);
            // dCBWSignature = "USBC" (0x43425355), little-endian on the wire.
            *cbw_buf.add(0) = 0x55; *cbw_buf.add(1) = 0x53; *cbw_buf.add(2) = 0x42; *cbw_buf.add(3) = 0x43;
            // dCBWTag
            *cbw_buf.add(4) = tag as u8;
            *cbw_buf.add(5) = (tag >> 8) as u8;
            *cbw_buf.add(6) = (tag >> 16) as u8;
            *cbw_buf.add(7) = (tag >> 24) as u8;
            // dCBWDataTransferLength
            *cbw_buf.add(8) = data_len as u8;
            *cbw_buf.add(9) = (data_len >> 8) as u8;
            *cbw_buf.add(10) = (data_len >> 16) as u8;
            *cbw_buf.add(11) = (data_len >> 24) as u8;
            // bmCBWFlags: 0x80 = device->host (IN), else 0x00
            *cbw_buf.add(12) = if dir == Direction::In { 0x80 } else { 0x00 };
            *cbw_buf.add(13) = 0; // bCBWLUN
            *cbw_buf.add(14) = cdb.len() as u8; // bCBWCBLength
            for (i, b) in cdb.iter().enumerate().take(16) {
                *cbw_buf.add(15 + i) = *b;
            }
            tag
        }
    }

    /// Execute a synchronous Bulk-Only Transport transaction, with ONE bounded Reset Recovery +
    /// retry on failure (USB Mass Storage Class Bulk-Only Transport 1.0 §5.3.3 "Reset Recovery",
    /// §5.3.4 "Stall Handling", §6.6.1 "CSW Not Valid"). MUST be called from a non-event context
    /// (controller lock held, event ring free) such as the main loop or a shell command — never from
    /// inside handle_event_trb.
    ///
    /// The retry re-runs the WHOLE transaction (`bot_transfer_once` rebuilds the CBW with a FRESH
    /// dCBWTag and re-sends the identical CDB against the identical DMA buffer), which is what makes
    /// it safe for the FAT layer:
    ///   * READ(10) is trivially idempotent.
    ///   * WRITE(10) is retried at the CBW boundary, so a retry writes the SAME sector with the SAME
    ///     bytes from the SAME `scsi_data_buffer` the caller staged before the first attempt. The
    ///     FAT layer's single-sector read-modify-writes stage the merged sector into that buffer and
    ///     then call `storage_write10`; nothing between the two attempts re-reads or re-derives the
    ///     content, so both attempts are byte-identical writes to one LBA. Repeating an identical
    ///     whole-sector write is idempotent by construction — the failure mode a naive retry would
    ///     introduce (a partially applied write followed by a retry recomputed from the now-changed
    ///     media) cannot arise, because the retry never recomputes anything.
    /// Recovery is skipped for `NoDevice` (there is nothing to reset) and, being bounded to exactly
    /// one attempt, cannot loop: `recover_bot_full` itself uses only EP0 control transfers and command-ring
    /// commands, never `bot_transfer`.
    ///
    /// BOT-PHASE fix 1 — **THE SINGLE CHOKEPOINT.** This function is a thin wrapper whose only job
    /// is that *no error exit from a BOT transaction returns with a dirty ring*. See
    /// `bot_clean_rings` for the mechanism and `bot_transfer_body` for the transaction itself.
    pub fn bot_transfer(&mut self, slot_id: u8, cdb: &[u8], data_phys: u64, data_len: u32, dir: Direction)
        -> Result<BotResult, BotError>
    {
        let out = self.bot_transfer_body(slot_id, cdb, data_phys, data_len, dir);
        if let Err(cause) = out {
            // `NoDevice` is raised before anything is built or queued (no rings, no endpoints, or a
            // surrendered slot), and `BadRequest` never reaches this function at all — in neither
            // case is there a ring to clean. Every OTHER error, from every path in the body, lands
            // here exactly once.
            if !matches!(cause, BotError::NoDevice | BotError::BadRequest) {
                self.bot_clean_rings(slot_id, cause);
            }
        }
        out
    }

    /// BOT-PHASE fix 1: leave BOTH bulk rings, and the event ring, in a state the next transaction
    /// can be born onto — and prove it on the wire.
    ///
    /// **The mechanism this closes.** A BOT transaction pushes up to three TRBs (CBW, data, CSW).
    /// Every error exit from the body used to return with whatever it had already pushed still on
    /// the rings and the controller's TR Dequeue Pointer parked on them. Nothing retired them and
    /// nothing repointed the controller, so the *next* transaction's doorbell re-executed them:
    /// a stale CBW and, on the write path, a stale payload, delivered into a device whose own BOT
    /// phase machine was still mid-transfer. The two machines then run one phase apart — the host's
    /// data is read as a command and its command as data — which is how a Command Block Wrapper
    /// ends up written into a FAT directory sector, the medium forensics that opened this arc.
    ///
    /// **The shared-ring aggravator.** The CBW and an OUT data stage ride the SAME bulk-OUT ring.
    /// An abandoned WRITE therefore strands *both* — a 31-byte command wrapper AND up to 32 KiB of
    /// file payload, in that order, ahead of the next doorbell. That is why this cleans both rings
    /// unconditionally rather than only the pipe the failed stage was waiting on: on the write path
    /// the compounding case is the common one, and naming one pipe would leave the other loaded.
    ///
    /// **The tool.** `resync_bulk_ep` — Stop/Reset Endpoint (whichever the EP State admits), drain
    /// the event ring, then Set TR Dequeue Pointer at the ring's live enqueue slot. Pointing the
    /// controller at the enqueue slot discards exactly the stranded TRBs and nothing else (see
    /// `TransferRing::enqueue_ptr_dcs`). It already existed and is already correct; the defect was
    /// never that the tool was wrong, only that the error paths did not call it.
    ///
    /// **Why this also covers the pre-BOT-RESCUE shape.** Before BOT-RESCUE the terminal error exit
    /// was a bare `return Err(...)` with no cleanup of any kind — boots A and B in the capture ran
    /// exactly that code. Because the chokepoint wraps the WHOLE body, that shape and every shape
    /// since (the below-`N_CONSEC` early return, the terminal escalate return, a `RingFull`
    /// refusal, a stalled status stage) are all covered by one construction rather than by an
    /// enumeration of exits that the next arc could forget to extend.
    fn bot_clean_rings(&mut self, slot_id: u8, cause: BotError) {
        let (in_ep, out_ep) = {
            let s = &self.slots[slot_id as usize];
            (s.bulk_in_ep, s.bulk_out_ep)
        };
        if slot_id == 0 || in_ep == 0 || out_ep == 0 {
            return; // no bulk pipes on this slot — nothing was ever pushed
        }
        // Nothing to clean, and nothing that could consume a stale TRB, in two cases — and both
        // must SKIP rather than fail, or `undrained=` would count them and stop being an assertion:
        //   * the slot has no output context: the device is gone (rung (b) power-cycled the port and
        //     `dispose_disconnected_slots` retired the slot), so there is no endpoint to stop, no
        //     controller position to move, and no ring the hardware can still reach;
        //   * the slot is SURRENDERED: `bot_transfer` refuses every later transfer to it with
        //     `NoDevice` at its first line, so there is provably no next doorbell to replay into.
        // In both cases a `resync_bulk_ep` would fail on the missing context and be recorded as an
        // undrained strand, which is exactly backwards.
        if self.slots[slot_id as usize].output_context.is_null() || self.bot_surrendered_slot == slot_id {
            serial_println!(
                ":: BOT: clean slot={} cause={:?} skipped={} — no reachable ring; no further transfer to this slot ::",
                slot_id, cause,
                if self.bot_surrendered_slot == slot_id { "surrendered" } else { "no-output-context" });
            return;
        }
        let in_dci = ((in_ep & 0x0F) * 2) + 1;
        let out_dci = (out_ep & 0x0F) * 2;

        // Any half-armed pending stage must not be matched against an event raised by the
        // stop/reset commands below.
        self.bot_pending = None;

        // ONSET-2 (M1b): the `when=pre` scan is no longer taken here. It is taken inside
        // `resync_bulk_ep` below, between each pipe's Stop/Reset Endpoint and its Set TR Dequeue
        // Pointer — the only window in which the endpoint context's TR Dequeue Pointer is BOTH
        // architecturally defined (the endpoint is out of Running) and still parked on the strand.
        // Taken here it ran after `recover_bot_full` had already repointed both pipes, so `gap=0
        // live=0` was true by construction and §15.8 item 2 could never fire. `abandoned_in=` /
        // `abandoned_out=` are incremented from that scan, so they still get exactly one count per
        // pipe per failure — from the authoritative reading instead of a post-repoint one.
        let in_ok = self.resync_bulk_ep(slot_id, in_dci, true, cause);
        let out_ok = self.resync_bulk_ep(slot_id, out_dci, false, cause);
        // Drain anything the resync itself produced, so a stopped TD's event cannot be mistaken for
        // the NEXT transaction's completion. `resync_bulk_ep` drains once between its stop and its
        // set-deq; this is the drain after the set-deq.
        while self.drain_event_ring_once() {}

        // POST scan. By here both endpoints are Stopped, so the Output Endpoint Context's TR
        // Dequeue Pointer field is architecturally DEFINED (xHCI 1.2 §4.8.3) — unlike the pre-scan,
        // which under a Running endpoint reads a frozen birth value on Intel silicon (GUARD-STATE).
        // That is why the assertion lives on this reading: `live=0` on both pipes here is the fix's
        // own regression witness, and it is read from a field that means what it says.
        let (in_live2, out_live2) = self.bot_strand_witness(slot_id, in_dci, out_dci, cause, "post");
        if in_live2 > 0 || !in_ok { BOT_TD_UNDRAINED.fetch_add(1, Ordering::Relaxed); }
        if out_live2 > 0 || !out_ok { BOT_TD_UNDRAINED.fetch_add(1, Ordering::Relaxed); }
        serial_println!(
            ":: BOT: clean slot={} cause={:?} in_resync={} out_resync={} in_live={} out_live={} undrained={} ::",
            slot_id, cause, if in_ok { "ok" } else { "fail" }, if out_ok { "ok" } else { "fail" },
            in_live2, out_live2, BOT_TD_UNDRAINED.load(Ordering::Relaxed));
    }

    /// BOT-PHASE: the `:: BOT: strand ::` line — per-ring enqueue index, cycle colour, the
    /// controller's context dequeue pointer, and the count of valid-cycle TRBs between the two.
    /// Returns `(in_live, out_live)` so the caller can count and assert on them.
    ///
    /// `epstate` is on the line because it is the line's own reading key: the `ctxdeq` field is only
    /// architecturally defined for a NON-Running endpoint (GUARD-STATE / xHCI 1.2 §4.8.3), so a
    /// `live=` count taken from `epstate=1` is advisory and one taken from `epstate=2/3/4` is
    /// authoritative. `ctxdeq_valid=` states which, rather than leaving a reader to know it.
    fn bot_strand_witness(&self, slot_id: u8, in_dci: u8, out_dci: u8, cause: BotError, when: &str)
        -> (usize, usize)
    {
        let in_live = self.bot_strand_pipe(slot_id, in_dci, true, cause, when);
        let out_live = self.bot_strand_pipe(slot_id, out_dci, false, cause, when);
        (in_live, out_live)
    }

    /// One pipe's `:: BOT: strand ::` line. Returns the count of live (valid-cycle) TRBs between the
    /// controller's dequeue pointer and our enqueue pointer — **0 unless the reading is trusted**,
    /// i.e. unless the endpoint is Halted(2)/Stopped(3)/Error(4), where the Output Endpoint Context's
    /// TR Dequeue Pointer field is architecturally defined (xHCI 1.2 §4.8.3). The raw scan is still
    /// printed under a Running endpoint, tagged `ctxdeq_valid=no-ep-running`, because a tagged
    /// reading is evidence about the instrument even when it is not evidence about the ring.
    ///
    /// **ONSET-2 (M1b): where the `pre` scan is taken from, and why it moved.** Until this arc the
    /// only `when=pre` scan ran inside `bot_clean_rings`, i.e. at the chokepoint on the way OUT of
    /// `bot_transfer` — after `recover_bot_full` had already run `resync_bulk_ep` on both pipes and
    /// issued their Set TR Dequeue Pointers. The controller had therefore been repointed onto our
    /// enqueue before the "pre" scan ever looked, so `gap=0 live=0` was true BY CONSTRUCTION and
    /// §15.8 item 2 ("a `when=pre` line with `live>0` and `ctxdeq_valid=yes`") could never fire from
    /// there. Two metal boots in `rmbp-gr8` show the ordering directly (F: set-deq at 3752/3754, the
    /// pre scan at 3758/3759; G: 4192/4194 then 4198/4199).
    ///
    /// The scan is now taken inside `resync_bulk_ep`, in the window between that pipe's Stop/Reset
    /// Endpoint and its Set TR Dequeue Pointer. That window is the ONLY place in the driver where
    /// both halves of the reading are true at once: the endpoint has been forced out of Running so
    /// the controller has written its real dequeue position back into the context (the reading is
    /// authoritative), and nothing has yet moved that position (the strand, if any, is still
    /// between the two pointers). `bot_clean_rings` keeps its `when=post` scan unchanged — that is
    /// `undrained=`'s reading and this arc does not touch it.
    ///
    /// The `abandoned_in=`/`abandoned_out=` counters are incremented from here, on the `pre` scan
    /// only, so they now count from the authoritative reading rather than from a post-repoint one.
    fn bot_strand_pipe(&self, slot_id: u8, dci: u8, is_in: bool, cause: BotError, when: &str) -> usize {
        let state = self.ep_state_of(slot_id, dci);
        let deq = self.ep_ctx_deq(slot_id, dci);
        let slot = &self.slots[slot_id as usize];
        let ring = if is_in { slot.bulk_in_ring.as_ref() } else { slot.bulk_out_ring.as_ref() };
        let (enq, cyc, ntrb, scan) = match ring {
            Some(r) => (r.enqueue_index(), if r.cycle_bit() { 1 } else { 0 }, r.num_trbs(),
                        r.strand_scan(deq)),
            None => (0, 0, 0, None),
        };
        let (gap, l) = scan.unwrap_or((0, 0));
        let trusted = matches!(state, 2 | 3 | 4);
        serial_println!(
            ":: BOT: strand when={} slot={} cause={:?} pipe={} dci={} epstate={} enq={} cycle={} ntrb={} ctxdeq={:#x} dcs={} ctxdeq_valid={} gap={} live={} gen={} ::",
            when, slot_id, cause, if is_in { "in" } else { "out" }, dci, state,
            enq, cyc, ntrb, deq, deq & 1,
            if trusted { "yes" } else { "no-ep-running" },
            gap, l, BOT_STAGE_GEN.load(Ordering::Relaxed));
        if !trusted {
            return 0;
        }
        if when == "pre" && l > 0 {
            if is_in {
                BOT_TD_ABANDONED_IN.fetch_add(1, Ordering::Relaxed);
            } else {
                BOT_TD_ABANDONED_OUT.fetch_add(1, Ordering::Relaxed);
            }
        }
        l
    }

    fn bot_transfer_body(&mut self, slot_id: u8, cdb: &[u8], data_phys: u64, data_len: u32, dir: Direction)
        -> Result<BotResult, BotError>
    {
        // BOT-RESCUE (c): a surrendered slot never sees another transfer. This is the one gate that
        // makes the guarantee absolute — every path into the driver's storage I/O comes through
        // here, so a disk the ladder gave up on cannot be revived into another ~6 s stall by a
        // caller that missed the retraction. Cleared on disposal / re-enumeration.
        if slot_id != 0 && self.bot_surrendered_slot == slot_id {
            return Err(BotError::NoDevice);
        }
        let first = self.bot_transfer_once(slot_id, cdb, data_phys, data_len, dir);
        let cause = match first {
            // PH-2: a `Failed` CSW is a completed transaction the DEVICE rejected — CHECK
            // CONDITION, not a transport fault. Reset Recovery is the wrong tool (nothing is
            // desynchronised); the right one is REQUEST SENSE, which is also what CLEARS the
            // condition. Handled before, and independently of, `recover_bot_full`.
            // BOT-RESCUE: a device that answers at all is not a device that is wedged, so this and
            // the plain-Ok arm below both END any escalation streak.
            Ok(r) if r.status == CswStatus::Failed => {
                self.bot_rescue_clear(slot_id);
                return self.bot_check_condition(slot_id, cdb, data_phys, data_len, dir, r);
            }
            Ok(r) => {
                self.bot_rescue_clear(slot_id);
                // GUARD-STATE: the once-per-boot deqprobe. Here and nowhere else — a transaction
                // that just succeeded on its FIRST attempt, so both rings are idle, nothing is in
                // flight, and this is not a recovery or sense path. Self-latching and a no-op after
                // the first call.
                if !BOT_SENSE_ACTIVE.load(Ordering::Relaxed) {
                    self.bot_deqprobe(slot_id);
                }
                return Ok(r);
            }
            Err(BotError::NoDevice) => return Err(BotError::NoDevice),
            Err(e) => e,
        };
        // BOT-RESCUE M3 witness 6: the failed stage's own pending record, taken out of
        // `run_bot_stage` instead of dropped, handed to recovery so its evidence line can carry the
        // truth about which pipe the stranded TRB sat on.
        let failed = self.bot_failed.take();
        if !self.recover_bot_full(slot_id, cause, failed) {
            // Recovery itself did not complete. Pre-arc this was the terminal Err; it now counts as
            // one failed cycle and enters the escalation ladder (which, below `BOT_RESCUE_N_CONSEC`,
            // returns exactly the same `Err(cause)` after a back-off).
            return self.bot_rescue_escalate(slot_id, cdb, data_phys, data_len, dir, cause);
        }
        let again = self.bot_transfer_once(slot_id, cdb, data_phys, data_len, dir);
        match &again {
            Ok(r) => {
                BOT_RETRY_OK.fetch_add(1, Ordering::Relaxed);
                serial_println!(
                    ":: BOT: retry result=pass status={:?} residue={} recoveries={} retry_ok={} retry_fail={} ::",
                    r.status, r.residue, BOT_RECOVER_COUNT.load(Ordering::Relaxed),
                    BOT_RETRY_OK.load(Ordering::Relaxed), BOT_RETRY_FAIL.load(Ordering::Relaxed));
            }
            Err(e) => {
                BOT_RETRY_FAIL.fetch_add(1, Ordering::Relaxed);
                serial_println!(
                    ":: BOT: retry result=fail err={:?} recoveries={} retry_ok={} retry_fail={} ::",
                    e, BOT_RECOVER_COUNT.load(Ordering::Relaxed),
                    BOT_RETRY_OK.load(Ordering::Relaxed), BOT_RETRY_FAIL.load(Ordering::Relaxed));
            }
        }
        // BOT-RESCUE: the retry settled it (whatever the CSW says) — end the streak and return the
        // result verbatim, exactly as the pre-arc code did. Otherwise escalate.
        if again.is_ok() {
            self.bot_rescue_clear(slot_id);
            return again;
        }
        self.bot_rescue_escalate(slot_id, cdb, data_phys, data_len, dir, cause)
    }

    /// PH-2: handle a runtime `Failed` CSW (SCSI CHECK CONDITION) with ONE sense fetch and, when
    /// the sense says the condition is transient, ONE retry of the original command.
    ///
    /// Bounded by construction, in this order:
    ///   * `BOT_SENSE_ACTIVE` latches for the whole handler. REQUEST SENSE and the retry both go
    ///     back through `bot_transfer`, so without the latch a device answering `Failed` to its own
    ///     sense command would recurse; with it, any nested `Failed` propagates exactly as it did
    ///     before this arc. One sense, one retry, all-or-nothing.
    ///   * The retry is gated on a `now_cycles`/`hw_wait_budget()` wall-clock deadline taken at
    ///     entry — if the sense fetch alone consumed the budget (a marginal device on metal), the
    ///     failure propagates instead of spending more of the caller's time.
    /// Only UNIT ATTENTION (key 0x6 — media/reset state change, the classic "retry and it works")
    /// and NOT READY (key 0x2 — becoming ready) earn the retry; every other key is a real rejection
    /// (ILLEGAL REQUEST, MEDIUM ERROR, DATA PROTECT …) that a retry would only repeat.
    ///
    /// The retry re-runs the whole transaction through `bot_transfer`, which is what makes it safe
    /// for the FAT layer for exactly the reason documented on `bot_transfer`: nothing between the
    /// attempts re-derives the payload, so a retried WRITE(10) is a byte-identical write of the
    /// same sector. That invariant is why the sense fetch below saves and restores the bytes it
    /// lands in — REQUEST SENSE shares the single per-slot staging buffer with the caller's data.
    fn bot_check_condition(&mut self, slot_id: u8, cdb: &[u8], data_phys: u64, data_len: u32,
        dir: Direction, failed: BotResult) -> Result<BotResult, BotError>
    {
        if BOT_SENSE_ACTIVE.swap(true, Ordering::Relaxed) {
            return Ok(failed);
        }
        let out = self.check_condition_inner(slot_id, cdb, data_phys, data_len, dir, failed);
        BOT_SENSE_ACTIVE.store(false, Ordering::Relaxed);
        out
    }

    /// The body of `bot_check_condition`, split out so the re-entrancy latch is released on every
    /// exit path without an early `return` being able to leak it.
    fn check_condition_inner(&mut self, slot_id: u8, cdb: &[u8], data_phys: u64, data_len: u32,
        dir: Direction, failed: BotResult) -> Result<BotResult, BotError>
    {
        let started = crate::arch::now_cycles();
        let sense_phys = match self.storage_data_phys(slot_id) {
            Ok(p) => p,
            Err(_) => return Ok(failed),
        };
        // REQUEST SENSE DMAs into the per-slot staging buffer, which on a WRITE(10) still holds the
        // caller's payload. Save the 18 bytes it will overwrite and put them back before the retry.
        let mut saved = [0u8; 18];
        unsafe { core::ptr::copy_nonoverlapping(sense_phys as *const u8, saved.as_mut_ptr(), 18); }

        BOT_SENSE_COUNT.fetch_add(1, Ordering::Relaxed);
        let sense_result = self.scsi_request_sense(slot_id);

        let sense = unsafe {
            dma_coherency::inval(sense_phys as usize, 18);
            let d = core::slice::from_raw_parts(sense_phys as *const u8, 18);
            // SPC-4 fixed-format sense: byte 2 bits 3:0 = sense key, byte 12 = ASC, byte 13 = ASCQ.
            (d[2] & 0x0F, d[12], d[13])
        };
        // Restore the caller's bytes before anything can re-send them.
        unsafe { core::ptr::copy_nonoverlapping(saved.as_ptr(), sense_phys as *mut u8, 18); }
        dma_coherency::clean(sense_phys as usize, 18);

        if let Err(e) = sense_result {
            serial_println!(":: BOT: sense result=fail err={:?} ::", e);
            return Ok(failed);
        }

        #[allow(unused_mut)]
        let (mut key, mut asc, mut ascq) = sense;
        // Test-only: the synthetic Failed CSW left the device perfectly healthy, so its real sense
        // is NO SENSE. Rewrite it to UNIT ATTENTION (0x6 / 0x28 "not ready to ready change") so the
        // retry leg is exercised as well as the fetch leg.
        #[cfg(feature = "botfaultinject")]
        if BOT_FAULT_CC_ACTIVE.swap(false, Ordering::Relaxed) {
            key = 0x6; asc = 0x28; ascq = 0x00;
            serial_println!(":: BOT: fault-inject synthetic sense UNIT ATTENTION (once) ::");
        }
        serial_println!(":: BOT: sense key={:#x} asc={:#x} ascq={:#x} ::", key, asc, ascq);

        if key != 0x6 && key != 0x2 {
            serial_println!(":: BOT: sense-retry result=skip key={:#x} ::", key);
            return Ok(failed);
        }
        if crate::arch::now_cycles().wrapping_sub(started) >= hw_wait_budget() {
            serial_println!(":: BOT: sense-retry result=skip reason=budget key={:#x} ::", key);
            return Ok(failed);
        }

        // The latch is still held, so this attempt cannot re-enter the handler however it fails;
        // going through `bot_transfer` keeps `recover_bot_full` in charge of transport faults.
        let again = self.bot_transfer(slot_id, cdb, data_phys, data_len, dir);
        match &again {
            Ok(r) if r.status == CswStatus::Passed => {
                BOT_SENSE_RETRY_OK.fetch_add(1, Ordering::Relaxed);
                serial_println!(
                    ":: BOT: sense-retry result=pass residue={} sense_n={} retry_ok={} retry_fail={} ::",
                    r.residue, BOT_SENSE_COUNT.load(Ordering::Relaxed),
                    BOT_SENSE_RETRY_OK.load(Ordering::Relaxed), BOT_SENSE_RETRY_FAIL.load(Ordering::Relaxed));
            }
            Ok(r) => {
                BOT_SENSE_RETRY_FAIL.fetch_add(1, Ordering::Relaxed);
                serial_println!(
                    ":: BOT: sense-retry result=fail status={:?} sense_n={} retry_ok={} retry_fail={} ::",
                    r.status, BOT_SENSE_COUNT.load(Ordering::Relaxed),
                    BOT_SENSE_RETRY_OK.load(Ordering::Relaxed), BOT_SENSE_RETRY_FAIL.load(Ordering::Relaxed));
            }
            Err(e) => {
                BOT_SENSE_RETRY_FAIL.fetch_add(1, Ordering::Relaxed);
                serial_println!(
                    ":: BOT: sense-retry result=fail err={:?} sense_n={} retry_ok={} retry_fail={} ::",
                    e, BOT_SENSE_COUNT.load(Ordering::Relaxed),
                    BOT_SENSE_RETRY_OK.load(Ordering::Relaxed), BOT_SENSE_RETRY_FAIL.load(Ordering::Relaxed));
            }
        }
        again
    }

    /// BOTEV: read one endpoint's EP State field from the OUTPUT device context (xHCI 1.2 §6.2.3,
    /// Endpoint Context dword 0 bits 2:0; 0=Disabled 1=Running 2=Halted 3=Stopped 4=Error).
    /// Returns `0xFF` when the slot has no output context (nothing to read). One bounded volatile
    /// read, no command, no wait — safe to call before and after every recovery stage.
    fn ep_state_of(&self, slot_id: u8, dci: u8) -> u8 {
        let oc = self.slots[slot_id as usize].output_context;
        if oc.is_null() {
            return 0xFF;
        }
        (unsafe { core::ptr::read_volatile((oc as *const u32).add(dci as usize * CTX_WORDS)) } & 0x7) as u8
    }

    /// BOTEV: the controller's own TR Dequeue Pointer for one endpoint, out of the OUTPUT device
    /// context (xHCI 1.2 §6.2.3, Endpoint Context dwords 2:3 — bit 0 is the Dequeue Cycle State,
    /// kept here because a witness wants the raw field). `0` when the slot has no output context.
    /// Two bounded volatile reads; this is how a boot capture shows whether Set TR Dequeue actually
    /// moved the controller, rather than merely returning success.
    fn ep_ctx_deq(&self, slot_id: u8, dci: u8) -> u64 {
        let oc = self.slots[slot_id as usize].output_context;
        if oc.is_null() {
            return 0;
        }
        let base = unsafe { (oc as *const u32).add(dci as usize * CTX_WORDS) };
        let lo = unsafe { core::ptr::read_volatile(base.add(2)) } as u64;
        let hi = unsafe { core::ptr::read_volatile(base.add(3)) } as u64;
        lo | (hi << 32)
    }

    /// BOT-RESCUE M2: refuse a bulk stage that would lap the controller on its ring.
    ///
    /// Reads the controller's own TR Dequeue Pointer for `dci` out of the OUTPUT device context and
    /// asks the ring whether one more `push` would overrun it (`TransferRing::would_lap`, which
    /// carries the spec citation and the margin argument). Two bounded volatile reads and pure
    /// arithmetic — no command, no wait, no MMIO.
    ///
    /// Healthy path: a BOT transaction awaits each stage's completion before queuing the next, so at
    /// most one TRB is ever outstanding on a 16-TRB ring and this returns `Ok` unconditionally. The
    /// refusal is reachable only when the controller has stopped consuming the ring — the state the
    /// metal failure this arc addresses parks the endpoint in — and there, failing the transfer
    /// immediately is strictly better than corrupting the ring and then waiting the full budget for a
    /// completion that cannot come.
    ///
    /// GUARD-STATE: **the comparison is only meaningful when the endpoint is NOT Running.** The
    /// Output Endpoint Context's TR Dequeue Pointer field is not a live position register. xHCI 1.0
    /// §4.8.3 and §6.2.3 define it as written back by the controller when the endpoint transitions
    /// Running -> Stopped/Halted (and otherwise set by Configure Endpoint / Set TR Dequeue Pointer);
    /// while the endpoint is Running the field is architecturally undefined, and real Intel silicon
    /// (Panther Point, xHCI 1.0) leaves it frozen at the birth value from the last of those writes.
    /// QEMU refreshes it live, which is why no gate could surface this. Comparing our live enqueue
    /// against a frozen birth value manufactures a false `RingFull` on a perfectly healthy
    /// mid-traffic device and self-inflicts the whole rescue ladder on it — observed on metal. So:
    /// read the EP State FIRST and refuse only from Halted(2), Stopped(3) or Error(4), the states in
    /// which the field is defined to hold the controller's real consumer position.
    ///
    /// This restores the pre-M2 behaviour for Running endpoints, which is correct: a healthy BOT
    /// transaction serialises its stages and so holds at most 3 of 16 TRBs, and the lap hazard the
    /// guard exists for only materialises across recovery retries — which run against Stopped
    /// endpoints, exactly where the guard still applies unchanged.
    /// ONSET-2 (M2 witness 2): ring a bulk doorbell for the BOT path, and RECORD that we did.
    ///
    /// The counting lives here rather than in `ring_doorbell` because that function is shared with
    /// enumeration, HID and the FTDI console: these counters must mean "doorbells the BOT transfer
    /// path wrote", not "doorbells anything wrote", or the delta the timeout line prints stops being
    /// about this transfer. One relaxed add and one relaxed store on a path that is already doing an
    /// MMIO write; nothing is decided from either, and the doorbell itself is byte-unchanged.
    ///
    /// ONSET-3 adds one more relaxed add on the same reads: `wrap_db`, the count of BOT doorbells
    /// whose target ring's MOST RECENT push crossed the Link TRB. That is the exact population every
    /// gr9 onset belongs to (see `BOT_WRAP_DB`), and it is read here rather than at the push sites
    /// because a wrapped push that never reaches its doorbell is a stranded TD, not a wrapped
    /// doorbell — keeping the two counts at different points is what lets them disagree.
    fn bot_doorbell(&mut self, slot_id: u8, dci: u8, is_in: bool) {
        let (idx, wrapped) = {
            let s = &self.slots[slot_id as usize];
            let r = if is_in { s.bulk_in_ring.as_ref() } else { s.bulk_out_ring.as_ref() };
            match r {
                Some(r) => (r.enqueue_index() as u32, r.wrapped_on_last_push()),
                None => (u32::MAX, false),
            }
        };
        if wrapped {
            BOT_WRAP_DB.fetch_add(1, Ordering::Relaxed);
        }
        if is_in {
            BOT_DB_IN.fetch_add(1, Ordering::Relaxed);
            BOT_DB_IN_IDX.store(idx, Ordering::Relaxed);
        } else {
            BOT_DB_OUT.fetch_add(1, Ordering::Relaxed);
            BOT_DB_OUT_IDX.store(idx, Ordering::Relaxed);
        }
        self.ring_doorbell(slot_id, dci as u32);
    }

    fn bot_ring_guard(&self, slot_id: u8, dci: u8, is_in: bool) -> Result<(), BotError> {
        // GUARD-STATE: never refuse against a Running(1) endpoint — nor a Disabled(0) or absent
        // (0xFF) one, where there is no consumer position to compare against either.
        let epstate = self.ep_state_of(slot_id, dci);
        match epstate {
            2 | 3 | 4 => {}
            _ => return Ok(()),
        }
        let deq = self.ep_ctx_deq(slot_id, dci);
        if deq == 0 {
            return Ok(()); // no output context / unreadable consumer position — never refuse
        }
        let slot = &self.slots[slot_id as usize];
        let ring = if is_in { slot.bulk_in_ring.as_ref() } else { slot.bulk_out_ring.as_ref() };
        let Some(r) = ring else { return Ok(()) };
        if !r.would_lap(deq) {
            return Ok(());
        }
        // ONSET-2 (M2 witness 5): `epstate=` and `ctxdeq_valid=` are now ON the line. §14.7 item 8's
        // sub-claim — "any `ring refuse` now carries epstate 2, 3 or 4 BY CONSTRUCTION, so it is a
        // real finding rather than an artefact" — was unverifiable on every capture ever taken,
        // because the line printed slot/dci/dir/enq/cycle/ntrb/ctxdeq/dcs and nothing else. The
        // construction argument is sound in the source (the match above returns `Ok` for every other
        // state before `ctxdeq` is even read), but a claim whose witness is absent from the log is an
        // inference, not a finding. Both values are already in hand here; printing them costs a
        // format argument each and makes the sub-claim checkable for the first time.
        //
        // HEALTHY-BUT-IDLE READING: the line does not appear at all. When it does appear, `epstate`
        // must read 2, 3 or 4 and `ctxdeq_valid` must read `yes`; anything else on this line would
        // mean the guard fired from a state in which the field it read is architecturally undefined,
        // which is the GUARD-STATE defect returning.
        serial_println!(
            ":: BOT: ring refuse slot={} dci={} dir={} epstate={} ctxdeq_valid={} enq={} cycle={} ntrb={} ctxdeq={:#x} dcs={} — enqueue would lap the controller (xHCI 1.2 §4.9.1); stage failed instead of overrunning the ring ::",
            slot_id, dci, if is_in { "in" } else { "out" },
            epstate, if matches!(epstate, 2 | 3 | 4) { "yes" } else { "no-ep-running" },
            r.enqueue_index(), if r.cycle_bit() { 1 } else { 0 }, r.num_trbs(),
            deq, deq & 1);
        Err(BotError::RingFull)
    }

    /// GUARD-STATE witness: the once-per-boot **deqprobe** — the experiment that turns "the TR
    /// Dequeue Pointer field is stale under a Running endpoint" from an inference about one metal
    /// capture into a fact every capture records, on whatever silicon it ran.
    ///
    /// Implementation choice: this is the ACTIVE form of the probe (rather than piggy-backing on the
    /// recovery ladder's own Stop Endpoint), because the piggy-back only ever prints during a real
    /// recovery — which never happens in QEMU and, after the guard fix, should never happen on a
    /// healthy metal boot either. The whole point is that the line appears on EVERY boot, so the
    /// platform difference is a recorded fact and not a thing to be re-derived.
    ///
    /// What it does, on the bulk IN endpoint, exactly once, and only from a healthy idle state:
    ///   1. read EP State and the context TR Dequeue Pointer while the endpoint is RUNNING;
    ///   2. **Stop Endpoint** (xHCI 1.2 §4.6.9) — the transition that obliges the controller to write
    ///      its real dequeue position back into the context (§4.8.3);
    ///   3. read both fields again, now from Stopped;
    ///   4. **Set TR Dequeue Pointer** back to the exact value just read (a semantic no-op: it puts
    ///      the controller where it already is, reserved bits 3:1 cleared, Dequeue Cycle State kept)
    ///      and ring the doorbell to return the endpoint to Running.
    ///
    /// Safety: it is called only from the plain-success return of `bot_transfer`, i.e. after a whole
    /// CBW -> data -> CSW transaction retired, with both rings idle, no TD in flight, no sense
    /// handler active and no escalation streak open. It never runs during recovery, and the latch is
    /// taken BEFORE any command so a fault inside it cannot repeat. If Stop Endpoint fails the
    /// endpoint is untouched and the probe stops there — it never leaves an endpoint parked.
    ///
    /// Reading the line: `running_ctxdeq == stopped_ctxdeq` while `enq` has moved is the frozen
    /// birth value (Intel Panther Point). `running_ctxdeq` tracking `enq` is a live field (QEMU).
    fn bot_deqprobe(&mut self, slot_id: u8) {
        if slot_id == 0 || BOT_DEQPROBE_DONE.swap(true, Ordering::Relaxed) {
            return;
        }
        let (in_addr, enq) = {
            let s = &self.slots[slot_id as usize];
            match (s.bulk_in_ep, s.bulk_in_ring.as_ref()) {
                (0, _) | (_, None) => return,
                (a, Some(r)) => (a, r.enqueue_index()),
            }
        };
        let dci = ((in_addr & 0x0F) * 2) + 1;
        let running_state = self.ep_state_of(slot_id, dci);
        if running_state != 1 {
            // Not Running: there is no "before" half of the experiment to take. Leave the latch set —
            // the probe is a one-shot by design, and a slot that is not Running right after a
            // successful transaction is itself outside what this witness can speak to.
            serial_println!(
                ":: BOT: deqprobe slot={} dci={} skipped epstate={} why=not-running ::",
                slot_id, dci, running_state);
            return;
        }
        let running_deq = self.ep_ctx_deq(slot_id, dci);

        let ctx = ((dci as u32) << 16) | ((slot_id as u32) << 24);
        // Stop Endpoint (TRB type 15).
        let (stop_ok, stop_cc, stop_why) =
            self.recover_cmd(Trb { parameter: 0, status: 0, control: (15 << 10) | ctx });
        let stopped_state = self.ep_state_of(slot_id, dci);
        let stopped_deq = self.ep_ctx_deq(slot_id, dci);

        let mut restore_ok = false;
        if stop_ok {
            // Set TR Dequeue Pointer (TRB type 16) back to exactly where the controller says it is.
            let want = stopped_deq & !0xEu64; // keep the address and the Dequeue Cycle State, drop RsvdZ
            let (ok, _, _) =
                self.recover_cmd(Trb { parameter: want, status: 0, control: (16 << 10) | ctx });
            restore_ok = ok;
            // Doorbell the endpoint: Stopped -> Running. Rung whether or not set-deq succeeded, so a
            // refused no-op cannot be what leaves the endpoint parked.
            self.ring_doorbell(slot_id, dci as u32);
        }
        let after_state = self.ep_state_of(slot_id, dci);
        serial_println!(
            ":: BOT: deqprobe slot={} dci={} enq={} running_epstate={} running_ctxdeq={:#x} -> stopped_epstate={} stopped_ctxdeq={:#x} stop_ok={} stop_cc={} stop_why={} restore_ok={} epstate_after={} verdict={} ::",
            slot_id, dci, enq, running_state, running_deq, stopped_state, stopped_deq,
            if stop_ok { "yes" } else { "no" }, stop_cc, stop_why,
            if restore_ok { "yes" } else { "no" }, after_state,
            if !stop_ok {
                "inconclusive (stop-ep failed)"
            } else if running_deq == stopped_deq {
                "ctxdeq-live (field unchanged across the stop; either already current or not written back)"
            } else {
                "ctxdeq-stale-under-running (the running read was a birth value; only the stopped read is a position)"
            });
    }

    /// BOTEV: run ONE recovery-stage xHCI command and render its outcome for a witness:
    /// `(ok, completion_code, why)`. A bare `Result` cannot distinguish the three ways a stage
    /// fails, and the metal capture that motivated this arc reported only `fail`:
    ///   * `why="ok"` — completion code 1 (Success).
    ///   * `why="cc-error"` — the command completed, but with an error code (`cc` carries it; 19 =
    ///     Context State Error, i.e. the command was illegal from the endpoint's actual state).
    ///   * `why="nocompletion"` — no Command Completion Event arrived inside the wall-clock budget
    ///     (`cc` is meaningless, reported 0): the command ring is not being consumed.
    ///   * `why="cmdring-stopped"` — the command ring is parked by an abort in progress, so
    ///     `run_command_sync` refuses before pushing anything.
    /// No retry, no extra wait: exactly the one bounded `run_command_sync` the caller already made.
    fn recover_cmd(&mut self, trb: Trb) -> (bool, u8, &'static str) {
        if self.cmd_ring_stopped {
            return (false, 0, "cmdring-stopped");
        }
        match self.run_command_sync(trb) {
            Ok((1, _)) => (true, 1, "ok"),
            Ok((cc, _)) => (false, cc, "cc-error"),
            Err(()) => (false, 0, "nocompletion"),
        }
    }

    /// Bring ONE bulk endpoint back to a usable, resynchronised state after a failed BOT stage.
    ///
    /// Reads the endpoint's current EP State from the OUTPUT device context (xHCI 1.2 §6.2.3,
    /// Endpoint Context dword 0 bits 2:0; 0=Disabled 1=Running 2=Halted 3=Stopped 4=Error), because
    /// both commands below are legal only from particular states and issuing them blind returns
    /// Context State Error (completion code 19):
    ///   * Halted (or Error) -> **Reset Endpoint** (§4.6.8) transitions it to Stopped.
    ///   * Running -> **Stop Endpoint** (§4.6.9) transitions it to Stopped. A plain timeout (the
    ///     metal failure mode this arc targets) leaves the endpoint Running with a TD still in
    ///     flight, so this arm — not the Reset arm — is the one a timeout takes.
    ///   * Already Stopped -> neither command is needed.
    /// Then **Set TR Dequeue Pointer** (§4.6.10, legal from Stopped/Error) moves the controller's
    /// dequeue pointer to the driver's enqueue pointer, discarding the stranded TRBs of the failed
    /// transaction and restoring the invariant that controller-dequeue == driver-enqueue on an idle
    /// ring. Every step is a single bounded `run_command_sync`; there is no loop.
    ///
    /// BOTEV: every stage is witnessed with its completion code AND the EP State before/after, so a
    /// capture distinguishes "command ring dead" (`why=nocompletion`) from "command refused"
    /// (`why=cc-error cc=19`) from "the state-aware arm chose wrong" (the `epstate` transition did
    /// not happen). The command SEQUENCE is untouched by this instrumentation.
    ///
    /// ONSET-2 (M1b): this function now also emits the `:: BOT: strand when=pre ::` line for its
    /// pipe, in the window between the Stop/Reset Endpoint above and the Set TR Dequeue Pointer
    /// below. See `bot_strand_pipe` for why that is the only window where the reading is worth
    /// taking. Read-only; the command sequence is unchanged.
    fn resync_bulk_ep(&mut self, slot_id: u8, dci: u8, is_in: bool, cause: BotError) -> bool {
        if self.slots[slot_id as usize].output_context.is_null() {
            serial_println!(
                ":: BOT: resync stage=read-state dci={} dir={} ok=no why=no-output-context ::",
                dci, if is_in { "in" } else { "out" });
            return false;
        }
        let dir = if is_in { "in" } else { "out" };
        let ep_state = self.ep_state_of(slot_id, dci) as u32;
        let ctx = ((dci as u32) << 16) | ((slot_id as u32) << 24);
        // ONSET-2 (M2 witness 3): baselines for the stopped-event delta, taken HERE — before the
        // Stop/Reset Endpoint below — because a counter read against its own boot-long total is not
        // a rate (the instrument-baseline law). The delta is printed after the drain.
        let (ev26_0, ev27_0, evany_0) = (
            BOT_EV_STOPPED.load(Ordering::Relaxed),
            BOT_EV_STOPPED_LI.load(Ordering::Relaxed),
            BOT_EV_ANY.load(Ordering::Relaxed));
        // ONSET-3: same baseline discipline for the Stopped-event PAYLOAD latch. Its delta is what
        // says the (dci, trb, residue) triple printed below belongs to THIS Stop Endpoint rather
        // than to some earlier recovery — the fields are last-writer-wins and persist for the boot.
        let stopev_n0 = BOT_STOPEV_N.load(Ordering::Relaxed);
        match ep_state {
            2 | 4 => {
                // Reset Endpoint (TRB type 14). TSP left 0: the device-side toggle was already
                // reset by CLEAR_FEATURE(ENDPOINT_HALT) above, so the controller must reset its own.
                let (ok, cc, why) = self.recover_cmd(Trb { parameter: 0, status: 0, control: (14 << 10) | ctx });
                let after = self.ep_state_of(slot_id, dci);
                serial_println!(
                    ":: BOT: resync stage=reset-ep dci={} dir={} ok={} cc={} why={} epstate={}->{} ::",
                    dci, dir, if ok { "yes" } else { "no" }, cc, why, ep_state, after);
                if !ok {
                    if cc == 19 {
                        // BOTEV (evidence only, no behaviour change): Context State Error means the
                        // controller's EP State was NOT the Halted/Error we read from the output
                        // context — i.e. Reset Endpoint was illegal at the moment it was issued
                        // (xHCI 1.2 §4.6.8: legal only from Halted/Error). Either the context read is
                        // stale or the state changed under us. The fix arc follows the evidence boot.
                        serial_println!(
                            ":: BOT: resync note dci={} dir={} illegal-reset-on-state read={} now={} — Context State Error on Reset Endpoint ::",
                            dci, dir, ep_state, after);
                    }
                    serial_println!("xHCI: BOT recover: Reset Endpoint failed (slot {} dci {})", slot_id, dci);
                    return false;
                }
            }
            1 => {
                // Stop Endpoint (TRB type 15).
                let (ok, cc, why) = self.recover_cmd(Trb { parameter: 0, status: 0, control: (15 << 10) | ctx });
                let after = self.ep_state_of(slot_id, dci);
                serial_println!(
                    ":: BOT: resync stage=stop-ep dci={} dir={} ok={} cc={} why={} epstate={}->{} ::",
                    dci, dir, if ok { "yes" } else { "no" }, cc, why, ep_state, after);
                if !ok {
                    if cc == 19 {
                        // BOTEV (evidence only): Stop Endpoint is legal only from Running
                        // (xHCI 1.2 §4.6.9); a Context State Error says the endpoint was not
                        // Running when the command executed, so the state-aware arm chose on a
                        // stale read.
                        serial_println!(
                            ":: BOT: resync note dci={} dir={} illegal-stop-on-state read={} now={} — Context State Error on Stop Endpoint ::",
                            dci, dir, ep_state, after);
                    }
                    serial_println!("xHCI: BOT recover: Stop Endpoint failed (slot {} dci {})", slot_id, dci);
                    return false;
                }
            }
            3 => {
                serial_println!(
                    ":: BOT: resync stage=skip dci={} dir={} ok=yes cc=0 why=already-stopped epstate={}->{} ::",
                    dci, dir, ep_state, ep_state);
            }
            _ => {
                serial_println!(
                    ":: BOT: resync stage=read-state dci={} dir={} ok=no why=ep-unusable epstate={}->{} ::",
                    dci, dir, ep_state, ep_state);
                serial_println!("xHCI: BOT recover: endpoint unusable (slot {} dci {} state {})", slot_id, dci, ep_state);
                return false;
            }
        }
        // Drain any Transfer Events the stop/reset produced (a stopped TD posts one) so they cannot
        // be mistaken for the retry's completion.
        while self.drain_event_ring_once() {}

        // ONSET-2 (M1b): THE authoritative pre-cleanup reading, and the only place it can be taken.
        // The endpoint is out of Running by here (the arms above stopped or reset it, or found it
        // already Stopped), so the controller has written its real TR Dequeue Pointer back into the
        // output context — and the Set TR Dequeue Pointer that would move it is still below. A
        // `live>0` here with `ctxdeq_valid=yes` is §15.8 item 2's first direct observation of a
        // stranded TRB.
        self.bot_strand_pipe(slot_id, dci, is_in, cause, "pre");

        // ONSET-2 (M2 witness 3): what the Stop/Reset Endpoint above actually posted.
        //
        // READING KEY — and the honest limit, stated because a witness read wrong is worse than
        // none. `ev_stopped`/`ev_stopped_li` counts the Transfer Events xHCI 1.2 §4.6.9 obliges the
        // controller to post for a TD it was IN THE MIDDLE OF when the endpoint was stopped:
        //   * non-zero -> the controller HAD fetched the TD. A timeout on that TD is then the DEVICE
        //     failing to move the data (NAKing, or wedged mid-transfer), not the controller failing
        //     to fetch it — and no host-side ring surgery can help.
        //   * zero, WITH a TD known outstanding (which is exactly the case at a BOT timeout: the
        //     pump was waiting on a specific TRB when it gave up) -> the controller never fetched
        //     the work. Host/endpoint fault; ring surgery is the right family of fix.
        // Zero is ALSO what a healthy idle endpoint reads, because there is nothing to interrupt. So
        // this field discriminates only in the presence of a known-outstanding TD; anywhere else it
        // is silent, not reassuring. `ev_any` is its denominator over the same window.
        //
        // ONSET-3 CORRECTION TO THE KEY ABOVE — `ev_stopped_li` (cc=27) does NOT belong in the
        // "non-zero -> the controller HAD fetched the TD" reading. gr9 boot 4 posts cc=27 on the IN
        // pipe of a recovery whose own `strand` line two lines up reads `gap=0 live=0`, with the CSW
        // not yet pushed at all: an endpoint that was Running but IDLE. §6.4.5 defines 27 as "the
        // TRB Transfer Length field is invalid", which is exactly what a controller reports when it
        // has no computable residual — including at an un-produced slot. Only **cc=26** carries the
        // in-progress reading. Read `ev_stopped=` alone for that; never the sum.
        //
        // ONSET-3 ADDS THE PAYLOAD, which is the field that actually decides the open question:
        //   * `stopev_res=` is the RESIDUE of the interrupted TD — bytes that had NOT moved. For the
        //     gr9 shape (a 512-byte OUT data stage) `stopev_res=512` says the device accepted ZERO
        //     bytes and never entered the data phase, which points at the CBW->DATA handoff and the
        //     two-TDs-under-one-doorbell straddle; `stopev_res=` anything less says it entered the
        //     data phase and stalled part-way, which points at the device or the transfer and
        //     retires the straddle.
        //   * `stopev_trb=` is the interrupted TD's TRB address. Compare it with `wait=` on the
        //     TIMEOUT-TRB line: equal means the controller was stopped on the very TRB the pump had
        //     given up on, which is what makes the residue answer the pump's question and not some
        //     other TD's.
        //   * `stopev_fresh=` is the only thing that licenses reading the other three at all. The
        //     latch is boot-lived and last-writer-wins, so `no` means these values belong to an
        //     EARLIER recovery and say nothing about this one.
        // HEALTHY-BUT-IDLE: `stopev_n=0 stopev_fresh=no stopev_dci=255 stopev_trb=0x0
        // stopev_res=none`. The sentinels are spelled, not numeric, because a residue of 0 is a real
        // and meaningful reading — "the device took every byte" — and must never be confusable with
        // "no event was ever latched".
        let (stopev_n, stopev_fresh) = {
            let n = BOT_STOPEV_N.load(Ordering::Relaxed);
            (n, n != stopev_n0)
        };
        serial_println!(
            ":: BOT: resync stopev dci={} dir={} epstate_read={} ev_stopped={} ev_stopped_li={} ev_any={} stopev_n={} stopev_fresh={} stopev_dci={} stopev_trb={:#x} stopev_res={} — Transfer Events posted by THIS pipe's Stop/Reset Endpoint (xHCI 1.2 §4.6.9) ::",
            dci, dir, ep_state,
            BOT_EV_STOPPED.load(Ordering::Relaxed).wrapping_sub(ev26_0),
            BOT_EV_STOPPED_LI.load(Ordering::Relaxed).wrapping_sub(ev27_0),
            BOT_EV_ANY.load(Ordering::Relaxed).wrapping_sub(evany_0),
            stopev_n,
            if stopev_fresh { "yes" } else { "no" },
            BOT_STOPEV_DCI.load(Ordering::Relaxed),
            BOT_STOPEV_TRB.load(Ordering::Relaxed),
            // Spelled sentinel: `none` when nothing has ever been latched, so a genuine residue of 0
            // reads as `stopev_res=0` and cannot be mistaken for an unarmed instrument.
            ResidueField(if stopev_n == 0 { None } else { Some(BOT_STOPEV_RES.load(Ordering::Relaxed)) }));

        let deq = {
            let slot = &self.slots[slot_id as usize];
            let ring = if is_in { slot.bulk_in_ring.as_ref() } else { slot.bulk_out_ring.as_ref() };
            match ring {
                Some(r) => { let (phys, dcs) = r.dequeue_reset_target(); phys | (dcs as u64) }
                None => {
                    serial_println!(
                        ":: BOT: resync stage=set-deq dci={} dir={} ok=no why=no-ring ::", dci, dir);
                    return false;
                }
            }
        };
        let before_state = self.ep_state_of(slot_id, dci);
        let before_deq = self.ep_ctx_deq(slot_id, dci);
        // Set TR Dequeue Pointer (TRB type 16); Stream ID 0 for a non-streaming bulk endpoint.
        let (ok, cc, why) = self.recover_cmd(Trb { parameter: deq, status: 0, control: (16 << 10) | ctx });
        let after_state = self.ep_state_of(slot_id, dci);
        let after_deq = self.ep_ctx_deq(slot_id, dci);
        serial_println!(
            ":: BOT: resync stage=set-deq dci={} dir={} ok={} cc={} why={} epstate={}->{} want={:#x} ctxdeq={:#x}->{:#x} ::",
            dci, dir, if ok { "yes" } else { "no" }, cc, why,
            before_state, after_state, deq, before_deq, after_deq);
        if !ok {
            serial_println!("xHCI: BOT recover: Set TR Dequeue failed (slot {} dci {} deq {:#x})", slot_id, dci, deq);
            return false;
        }
        true
    }

    /// The single-attempt Bulk-Only Transport transaction: CBW -> (optional data) -> CSW.
    /// `bot_transfer` wraps this with Reset Recovery + one bounded retry.
    fn bot_transfer_once(&mut self, slot_id: u8, cdb: &[u8], data_phys: u64, data_len: u32, dir: Direction)
        -> Result<BotResult, BotError>
    {
        let (cbw_phys, csw_phys, in_addr, out_addr) = {
            let slot = &self.slots[slot_id as usize];
            let cbw = match slot.cbw_buffer { Some(p) => p as u64, None => return Err(BotError::NoDevice) };
            let csw = match slot.csw_buffer { Some(p) => p as u64, None => return Err(BotError::NoDevice) };
            (cbw, csw, slot.bulk_in_ep, slot.bulk_out_ep)
        };
        if in_addr == 0 || out_addr == 0 { return Err(BotError::NoDevice); }
        let in_dci = ((in_addr & 0x0F) * 2) + 1;
        let out_dci = (out_addr & 0x0F) * 2;

        // PIUSB-38: latched when the data stage halts (STALL/Babble). It steers the status stage
        // into Reset Recovery: on a data-phase stall we still collect the CSW (resync), and if the
        // CSW itself fails we escalate to a full Bulk-Only Mass Storage Reset.
        let mut data_stalled = false;
        // BOT-PHASE fix 3: bytes the data stage actually moved, from its Transfer Event residue.
        // Cross-checked against the device's own `dCSWDataResidue` claim at CSW validation.
        let mut data_moved: u32 = 0;
        let tag = self.build_cbw(cbw_phys as *mut u8, data_len, dir, cdb);
        // ONSET-2 (M2 witness 7): name the transaction, so a TIMEOUT line identifies its own victim.
        // §15.2's code -> capture -> medium join had to be reconstructed from a wrecked filesystem
        // because `dCBWTag`, the CDB opcode and the LBA were printed only by `csw_bytes` on a CSW
        // rejection and by `BLK: io-cause` after the fact. Three relaxed stores.
        BOT_LAST_TAG.store(tag, Ordering::Relaxed);
        BOT_LAST_CDB0.store(*cdb.first().unwrap_or(&0) as u32, Ordering::Relaxed);
        // READ(10) / WRITE(10) carry a big-endian 32-bit LBA at CDB bytes 2..5 (SBC-3 §5.10/§5.32).
        // Any other opcode has no LBA in that position, so record 0 rather than a decoded lie.
        BOT_LAST_LBA.store(
            if cdb.len() >= 6 && matches!(cdb[0], 0x28 | 0x2A) {
                ((cdb[2] as u32) << 24) | ((cdb[3] as u32) << 16) | ((cdb[4] as u32) << 8) | cdb[5] as u32
            } else { 0 },
            Ordering::Relaxed);
        unsafe { core::ptr::write_bytes(csw_phys as *mut u8, 0, 13); }
        // XHCI-COHERENCE: the CBW is CPU-written and DMA-read by the controller (bulk OUT) — clean it
        // to DRAM before its doorbell. The CSW was just zeroed and the controller will DMA-write it —
        // clean+invalidate the zeroed handoff so the later read observes the controller's status.
        dma_coherency::clean(cbw_phys as usize, 31);
        dma_coherency::clean_inval(csw_phys as usize, 13);

        // BOT phases are SERIALIZED: CBW -> [DATA] -> CSW, each transfer completing before
        // the next is queued — mirroring the Linux usb-storage bulk transport. We must NOT
        // pipeline the CSW TRB behind an async data stage: QEMU's usb-storage is a single-
        // packet device, so a CSW IN token arriving while the DATA transfer is still async
        // is never serviced and the transfer hangs with no completion event.
        //
        // BOT-CBW 2026-07-30: "the CBW is fire-and-forget (the device consumes it before it can
        // respond)" — which is what this comment used to say — was an assumption the driver never
        // tested, and metal convicted it (§17). ALL THREE stages now carry IOC (1<<5) and all three
        // are awaited in order. Nothing is ever queued behind an un-retired TD on the same endpoint.

        // BOT-PHASE fix 2 — THE RING GUARD RUNS BEFORE ANYTHING IS PUSHED.
        //
        // The guard used to be checked per-stage, immediately before that stage's own push. That is
        // one stage too late: the CBW was pushed first, and a later `Err(RingFull)` from the data or
        // status guard returned through `?` with the CBW SITTING ON THE OUT RING, un-rung and
        // unretired — precisely the stranded-TRB condition this arc exists to end, manufactured by
        // the guard that was supposed to prevent it. Every ring this transaction will touch is
        // checked here, up front, so a refusal leaves BOTH rings byte-untouched and no stage can be
        // stranded by the admission decision for a later one.
        //
        // Healthy path: `bot_ring_guard` returns `Ok` immediately for a Running endpoint
        // (GUARD-STATE), so on a healthy device this is the same three no-op calls it was before,
        // merely made in a different order. Nothing is pushed and no doorbell rings in between, so
        // there is no observable difference; only the RingFull refusal moves earlier.
        let data_out = matches!(dir, Direction::Out);
        self.bot_ring_guard(slot_id, out_dci, false)?;                      // CBW (and an OUT data stage)
        if data_len > 0 && !data_out {
            self.bot_ring_guard(slot_id, in_dci, true)?;                    // IN data stage
        }
        self.bot_ring_guard(slot_id, in_dci, true)?;                        // CSW

        // 1) CBW on bulk OUT (Normal TRB, 31 bytes).
        // BOT-PHASE: the push result is no longer discarded. `TransferRing::push` returns a
        // `Result`, and `.ok()` threw it away — a failed push would then have left the transaction
        // waiting on whatever address the DEFAULT produced, which for the stages below was
        // `ring_base + 0`: an address that is a real TRB slot and recurs, i.e. another aliasing
        // vector for the matching in `handle_event_trb`. `push` cannot fail today (it always
        // returns `Ok`), so this is byte-identical in behaviour; it is here so that if it ever can,
        // the transaction fails honestly instead of waiting on a fabricated address.
        //
        // BOT-CBW: the CBW carries IOC (1<<5) and is pumped to completion before anything else is
        // built, which is what §14.4 has always claimed the code does and what §17's A/B proved it
        // must. There is no build that can turn this off.
        let (cbw_trb_phys, cbw_idx, cbw_wrapped) = {
            let ring = self.slots[slot_id as usize].bulk_out_ring.as_mut().unwrap();
            let base = ring.get_ptr();
            let idx = ring.push(Trb { parameter: cbw_phys, status: 31, control: (1 << 10) | (1 << 5) })
                .map_err(|_| BotError::RingFull)?;
            (base + (idx as u64) * 16, idx, ring.wrapped_on_last_push())
        };
        {
            // The CBW is a first-class awaited stage: shape record, its own doorbell, its own pump.
            // `stage=cbw` on a TIMEOUT-SHAPE line is a reading no pre-2026-07-30 capture was able to
            // produce — "the device never even took the command".
            BOT_LAST_STAGE.store(3, Ordering::Relaxed);
            BOT_LAST_DIR.store(2, Ordering::Relaxed);
            BOT_LAST_LEN.store(31, Ordering::Relaxed);
            BOT_LAST_TRB_IDX.store(cbw_idx as u32, Ordering::Relaxed);
            BOT_LAST_WRAP.store(cbw_wrapped, Ordering::Relaxed);
            self.bot_doorbell(slot_id, out_dci, false);
            let (cbw_code, _) = self.run_bot_stage(slot_id, in_dci, out_dci, cbw_trb_phys)?;
            if cbw_code != 1 && cbw_code != 13 {
                serial_println!("xHCI: BOT CBW stage error, completion code {}", cbw_code);
                return Err(BotError::TransferError(cbw_code));
            }
        }

        // 2) Data stage (IN or OUT), if any. IOC + ISP (1<<2) so both full and short-packet
        //    completions post an event; wait for it to retire BEFORE queuing the CSW.
        if data_len > 0 {
            let (data_dci, data_trb_phys, data_trb_idx) = {
                let ring = if data_out {
                    self.slots[slot_id as usize].bulk_out_ring.as_mut().unwrap()
                } else {
                    self.slots[slot_id as usize].bulk_in_ring.as_mut().unwrap()
                };
                let base = ring.get_ptr();
                // XHCI-COHERENCE: evict the data buffer to DRAM BEFORE the doorbell — for BOTH
                // directions. OUT: the buffer is CPU-written and DMA-read, so the clean pushes the
                // current bytes to DRAM. IN (PIUSB-34): the buffer is freshly `alloc_zeroed` (8 dirty
                // zero lines) and reused across SCSI reads — a short prior read (READ CAPACITY = 8 B,
                // INQUIRY = 36 B) only ever touches line 0, leaving lines 1..7 dirty-zero in cache. On
                // the non-coherent Pi 4 PCIe path the controller DMA-writes the block straight to DRAM;
                // a natural write-back of those stale dirty lines in the window around the DMA clobbers
                // the just-written DRAM with zeros (Passed/residue=0/data=00). Cleaning here leaves no
                // dirty line to lose, so the controller's DMA survives; the post-transfer invalidate
                // below then drops the clean lines and the CPU parses fresh DRAM. This mirrors every
                // other IN-arming site (interrupt-IN reports, control-IN, descriptor reads). No-op x86.
                dma_coherency::clean(data_phys as usize, data_len as usize);
                // BOT-PHASE: `.unwrap_or(0)` here would have silently made the pump wait on
                // `ring_base + 0` — a real, recurring TRB address — after a failed push. Propagate.
                let idx = ring.push(Trb { parameter: data_phys, status: data_len,
                    control: (1 << 10) | (1 << 5) | (1 << 2) }).map_err(|_| BotError::RingFull)?;
                // MULTIBLK: recording the wrap here is what lets a TIMEOUT line answer "did the lost
                // completion sit on a wrapped ring?" — a question §12.2 could only argue about from
                // the ring arithmetic (16 TRBs, 3 pushed per transaction, so ~every 5th transaction
                // wraps) and never observe directly. It is the arc's surviving hard correlation.
                //
                // ONSET-3: the test was `idx == 0`, on the reasoning that `push` returns index 0
                // exactly when it crossed the Link. That is true of every push but the FIRST on a
                // virgin ring, which also lands at index 0 having crossed nothing and toggled
                // nothing. Ask the ring directly.
                let wrapped = ring.wrapped_on_last_push();
                BOT_LAST_WRAP.store(wrapped, Ordering::Relaxed);
                if wrapped { BOT_TX_WRAPPED.fetch_add(1, Ordering::Relaxed); }
                (if data_out { out_dci } else { in_dci }, base + (idx as u64) * 16, idx as u32)
            };
            // MULTIBLK: shape of the TD the pump is about to wait on. Sizes now span 512 B .. 32 KiB,
            // so a wedge that prefers one size or one direction becomes visible instead of invisible.
            BOT_LAST_STAGE.store(1, Ordering::Relaxed);
            BOT_LAST_DIR.store(if data_out { 2 } else { 1 }, Ordering::Relaxed);
            BOT_LAST_LEN.store(data_len, Ordering::Relaxed);
            BOT_LAST_TRB_IDX.store(data_trb_idx, Ordering::Relaxed);
            if data_len > 512 {
                BOT_TX_MULTI.fetch_add(1, Ordering::Relaxed);
            } else {
                BOT_TX_SINGLE.fetch_add(1, Ordering::Relaxed);
            }
            BOT_TX_MAXLEN.fetch_max(data_len as u64, Ordering::Relaxed);
            let sectors = (data_len as u64) / 512;
            if data_out {
                BOT_TX_WR_SECTORS.fetch_add(sectors, Ordering::Relaxed);
            } else {
                BOT_TX_RD_SECTORS.fetch_add(sectors, Ordering::Relaxed);
            }

            // BOT-CBW: the CBW has already rung its own doorbell and RETIRED, so this one announces
            // the data TD alone — an OUT data stage is the only TD outstanding on the OUT ring, and
            // that is precisely the straddle §17 convicted. For an IN data stage this OUT doorbell
            // now has nothing to fetch; it is kept because ringing a drained ring is architecturally
            // legal and a no-op, and because the knob-ON boot that produced §17's n=1108/timeouts=0
            // rang it. Changing it would make this build something metal has not run.
            self.bot_doorbell(slot_id, out_dci, false);
            if data_dci != out_dci { self.bot_doorbell(slot_id, data_dci, true); }

            let (code, residue) = self.run_bot_stage(slot_id, in_dci, out_dci, data_trb_phys)?;
            // BOT-PHASE fix 3 — SHORT-TRANSFER HONESTY.
            //
            // The Transfer Event's TRB Transfer Length field is the RESIDUE: the bytes of this TD
            // that did NOT move (xHCI 1.2 §6.4.2.1). Until this arc `run_bot_stage` returned only
            // the completion code, so `cc=13 SHORT PACKET` — which is exactly the code that says
            // "fewer bytes than the TD asked for" — was accepted as success and the CSW was queued
            // straight behind it. `moved` is what actually crossed the wire.
            let moved = data_len.saturating_sub(residue);
            data_moved = moved;
            if moved != data_len {
                if data_out {
                    BOT_SHORT_DATA_OUT.fetch_add(1, Ordering::Relaxed);
                } else {
                    BOT_SHORT_DATA_IN.fetch_add(1, Ordering::Relaxed);
                }
                // The witness the audit asked for by name. It prints on every shortfall in either
                // direction, so the OUT case (a fault, below) and the IN case (legitimate SCSI, see
                // the reasoning on the fault gate) are both on the record with the same grammar.
                serial_println!(
                    ":: BOT: dtl_vs_moved slot={} dir={} dtl={} moved={} residue={} cc={} verdict={} ::",
                    slot_id, if data_out { "out" } else { "in" }, data_len, moved, residue, code,
                    if data_out { "phase-fault" } else { "short-in-allowed" });
            }
            // OUT: the device stopped ACCEPTING bytes. USB MSC BOT 1.0 §6.7.3 case 9 (Ho > Do) —
            // the device wants less than the host is sending, and the host must run Reset Recovery.
            // It is NOT in its status phase, so queueing the CSW behind this is precisely the step
            // that slides the two phase machines apart, and the next transaction's CBW then lands
            // where a CSW was expected. Feed the recovery path instead.
            //
            // IN is deliberately NOT a fault, and the asymmetry is the spec's, not a softening:
            // §6.7.2 case 4 (Hi > Di) has the device legitimately returning fewer bytes than the
            // allocation length asked for — REQUEST SENSE (18), INQUIRY (36) and READ CAPACITY (8)
            // are all commands whose CDB names a MAXIMUM — and the device is in its status phase
            // afterwards, with `dCSWDataResidue` carrying the shortfall. Failing those would break
            // bring-up on conforming devices. The IN shortfall is instead CROSS-CHECKED against the
            // device's own residue claim at CSW validation below, which is the check that has
            // teeth: host and device disagreeing about how much moved IS a phase fault.
            if data_out && moved != data_len {
                serial_println!(
                    "xHCI: BOT OUT data stage moved {} of {} bytes — phase fault (BOT 1.0 §6.7.3 case 9)",
                    moved, data_len);
                return Err(BotError::TransferError(if code == 1 { 13 } else { code }));
            }
            if code != 1 && code != 13 {
                serial_println!("xHCI: BOT data stage error, completion code {}", code);
                if code == 4 || code == 6 {
                    // PIUSB-38 / USB MSC BOT §6.7.2 (Reset Recovery, data-phase stall): the data
                    // endpoint halted (STALL/Babble). Clear the halt on this bulk pipe, then STILL
                    // proceed to the status stage below to collect the CSW — the device stalled the
                    // DATA phase, not the command, so it is in its status phase and returns a Failed
                    // CSW; reading it resynchronises both BOT state machines so the NEXT command
                    // starts clean. An unrecovered data-phase stall that skipped the CSW was the P47
                    // wedge (every later READ/SENSE/TUR on the slot then timed out). If the status
                    // stage ALSO fails, we escalate to a full Bulk-Only reset below.
                    self.recover_bulk_stall(slot_id, !data_out);
                    data_stalled = true;
                } else {
                    return Err(BotError::TransferError(code));
                }
            }
            // XHCI-COHERENCE: consumer boundary — an IN data stage's buffer was DMA-written by the
            // controller; invalidate it here (ONE chokepoint for every SCSI IN reader: INQUIRY,
            // READ CAPACITY, block reads) so callers parse fresh DRAM. No-op x86. Skip after a
            // data-phase stall: the buffer holds no valid transfer, and the CSW below carries the
            // real (Failed) verdict.
            if !data_out && !data_stalled {
                dma_coherency::inval(data_phys as usize, data_len as usize);
            }
        }
        // BOT-CBW: there is no `else` arm any more. The no-data-stage path used to ring the OUT
        // doorbell here to fetch+send the CBW; the CBW now has its own doorbell and has already
        // retired by this point, so this ring would have been empty.

        // Test-only: deterministic synthetic failure at exactly this point (see BOT_FAULT_FIRED).
        // The data stage above really landed, so returning here leaves the device parked in its CSW
        // phase with a stale CSW pending — the strongest available stand-in for the metal timeout.
        #[cfg(feature = "botfaultinject")]
        if dir == Direction::Out && data_len > 0 && !BOT_FAULT_FIRED.swap(true, Ordering::Relaxed) {
            serial_println!(":: BOT: fault-inject synthetic CSW-stage failure (slot {}, once) ::", slot_id);
            return Err(BotError::Timeout);
        }

        // 3) CSW on bulk IN (13 bytes, IOC). The data stage (if any) has fully retired, so
        //    usb-storage is in its CSW state and services this token immediately.
        // BOT-PHASE fix 2: this ring's headroom was checked up front, with the others, BEFORE the
        // CBW was pushed — a refusal here would have stranded it.
        let csw_trb_phys = {
            let ring = self.slots[slot_id as usize].bulk_in_ring.as_mut().unwrap();
            let base = ring.get_ptr();
            let idx = ring.push(Trb { parameter: csw_phys, status: 13, control: (1 << 10) | (1 << 5) })
                .map_err(|_| BotError::RingFull)?;
            // MULTIBLK: same shape record as the data stage. The CSW is ALWAYS 13 bytes IN, so a
            // TIMEOUT reporting `stage=csw` rules transfer size OUT as the discriminator — which is
            // itself a finding, and one a log with only one transfer shape could never make.
            BOT_LAST_STAGE.store(2, Ordering::Relaxed);
            BOT_LAST_DIR.store(1, Ordering::Relaxed);
            BOT_LAST_LEN.store(13, Ordering::Relaxed);
            BOT_LAST_TRB_IDX.store(idx as u32, Ordering::Relaxed);
            // ONSET-3: real Link-crossing predicate, not `idx == 0` — see the data stage above.
            BOT_LAST_WRAP.store(ring.wrapped_on_last_push(), Ordering::Relaxed);
            base + (idx as u64) * 16
        };
        self.bot_doorbell(slot_id, in_dci, true);

        // PIUSB-38: if the status stage cannot even complete (times out) after a data-phase stall,
        // the pipe is wedged. Surfacing the error is now enough to get it un-wedged: EVERY `Err` this
        // function returns is caught by `bot_transfer`, which runs full Bulk-Only Reset Recovery and
        // then spends its single retry — so the next command is never born onto a dead pipe. (The
        // only `Err` that skips recovery is `NoDevice`, raised at the top before anything is queued,
        // where there is nothing to reset. `run_bot_stage` can only fail with `Timeout`.)
        let (code, _csw_stage_residue) = self.run_bot_stage(slot_id, in_dci, out_dci, csw_trb_phys)?;
        if code != 1 && code != 13 {
            serial_println!("xHCI: BOT transfer error, completion code {}", code);
            if code == 4 || code == 6 {
                // PIUSB-38 / USB MSC BOT §6.7.3 (status-phase stall): the CSW rides the bulk IN pipe;
                // a halt here — or a status-phase halt after the data phase already stalled — leaves
                // the IN endpoint dead. Clear THIS endpoint's halt so the pipe is not left halted even
                // if recovery later fails; the class-level escalation (device BOT reset + both halts +
                // ring resync) is `bot_transfer`'s, on the `Err` below.
                self.recover_bulk_stall(slot_id, true);
                return Err(BotError::Stall);
            }
            return Err(BotError::TransferError(code));
        }

        // 4) Validate the CSW.
        unsafe {
            // XHCI-COHERENCE: consumer boundary — the CSW was DMA-written; invalidate before reading.
            dma_coherency::inval(csw_phys as usize, 13);
            let csw = core::slice::from_raw_parts(csw_phys as *const u8, 13);
            let sig = (csw[0] as u32) | ((csw[1] as u32) << 8) | ((csw[2] as u32) << 16) | ((csw[3] as u32) << 24);
            let csw_tag = (csw[4] as u32) | ((csw[5] as u32) << 8) | ((csw[6] as u32) << 16) | ((csw[7] as u32) << 24);
            let residue = (csw[8] as u32) | ((csw[9] as u32) << 8) | ((csw[10] as u32) << 16) | ((csw[11] as u32) << 24);
            let bstatus = csw[12];

            // BOT-PHASE witness: the raw 13 CSW bytes, printed on EVERY rejection below. The
            // 2026-07-29 capture recorded a single tag of `0xACABAAA9` with nothing to read it
            // against, and the two candidate explanations — a TORN READ of a partially DMA-written
            // CSW, versus an OVERLAY of some other payload onto the CSW buffer — are distinguished
            // by the bytes AROUND the tag, which were never printed. A valid `USBS` signature with
            // a wrong tag is a stale-but-well-formed CSW (phase slip); high-entropy bytes across
            // all 13 are an overlay; a mixture of expected and zero bytes is a torn read.
            let hexdump = |what: &str| {
                serial_println!(
                    ":: BOT: csw_bytes slot={} why={} tag_want={:#010x} b={:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
                    slot_id, what, tag,
                    csw[0], csw[1], csw[2], csw[3], csw[4], csw[5], csw[6],
                    csw[7], csw[8], csw[9], csw[10], csw[11], csw[12]);
            };

            if sig != 0x53425355 {
                BOT_BAD_SIG.fetch_add(1, Ordering::Relaxed);
                serial_println!("xHCI: BOT bad CSW signature {:#x} (boot total {})",
                    sig, BOT_BAD_SIG.load(Ordering::Relaxed));
                hexdump("bad-sig");
                // PIUSB-38: a garbage CSW (after a data-phase stall, the resync attempt did not land a
                // valid status) means the pipe is out of phase — `bot_transfer` runs full Reset
                // Recovery on this Err.
                return Err(BotError::BadCswSignature);
            }
            if csw_tag != tag {
                BOT_TAG_MISMATCH.fetch_add(1, Ordering::Relaxed);
                serial_println!("xHCI: BOT CSW tag mismatch (got {:#x}, want {:#x}; boot total {})",
                    csw_tag, tag, BOT_TAG_MISMATCH.load(Ordering::Relaxed));
                hexdump("tag-mismatch");
                return Err(BotError::TagMismatch);
            }
            // BOT-PHASE fix 3 (Pi-seat addition): VALIDATE `dCSWDataResidue`. It was decoded and
            // handed to the caller but never checked against anything, so a transaction that moved
            // ZERO bytes and came back `bStatus=0` with a full residue was reported to the FAT layer
            // as a clean success — a silent short write, or a read whose buffer keeps whatever was
            // in it. The device's residue is its own claim about how many bytes did not move; the
            // Transfer Event residue is the CONTROLLER's. Two independent witnesses of one quantity:
            // if they disagree, one of the two state machines is a phase out, and that is exactly
            // the condition this arc refuses to call success.
            if data_len > 0 && !data_stalled {
                let device_moved = data_len.saturating_sub(residue.min(data_len));
                if residue > data_len || device_moved != data_moved {
                    serial_println!(
                        ":: BOT: residue_disagree slot={} dir={} dtl={} host_moved={} dev_residue={} dev_moved={} bstatus={} ::",
                        slot_id, if data_out { "out" } else { "in" }, data_len,
                        data_moved, residue, device_moved, bstatus);
                    serial_println!(
                        "xHCI: BOT CSW residue disagrees with the transfer event (dtl {}, host moved {}, device says {} moved) — phase fault",
                        data_len, data_moved, device_moved);
                    hexdump("residue-disagree");
                    return Err(BotError::TransferError(13));
                }
            }
            #[allow(unused_mut)]
            let mut status = match bstatus {
                0 => CswStatus::Passed, 1 => CswStatus::Failed,
                2 => CswStatus::PhaseError, _ => CswStatus::Unknown,
            };
            // Test-only: deterministic synthetic CHECK CONDITION (see BOT_FAULT_CC_FIRED).
            #[cfg(feature = "botfaultinject")]
            if dir == Direction::In && data_len >= 512 && status == CswStatus::Passed
                && !BOT_FAULT_CC_FIRED.swap(true, Ordering::Relaxed)
            {
                serial_println!(":: BOT: fault-inject synthetic Failed CSW (slot {}, once) ::", slot_id);
                status = CswStatus::Failed;
                BOT_FAULT_CC_ACTIVE.store(true, Ordering::Relaxed);
            }
            Ok(BotResult { status, residue })
        }
    }

    // ==================== PIUSB-36: read-wedge experiment matrix ====================

    /// PIUSB-36 step 5: READ(10) LBA0 with the IN data stage split across TWO chained TRBs
    /// (256 B + 256 B, chain bit on the first, IOC on the second) into `data_phys`. Everything
    /// else mirrors `bot_transfer`'s IN path exactly (same clean-before-doorbell + post-invalidate
    /// coherency, same serialized CBW -> DATA -> CSW). A TD-SHAPE variant: if a single-TRB 512 B
    /// TD reads zeros but a two-TRB TD reads data (or vice-versa), the discriminator is TD shape,
    /// not transfer length. Read-only, aarch64-only.
    #[cfg(target_arch = "aarch64")]
    fn piusb36_read10_two_trb(&mut self, slot_id: u8, data_phys: u64) -> Result<BotResult, BotError> {
        let (cbw_phys, csw_phys, in_addr, out_addr) = {
            let slot = &self.slots[slot_id as usize];
            let cbw = match slot.cbw_buffer { Some(p) => p as u64, None => return Err(BotError::NoDevice) };
            let csw = match slot.csw_buffer { Some(p) => p as u64, None => return Err(BotError::NoDevice) };
            (cbw, csw, slot.bulk_in_ep, slot.bulk_out_ep)
        };
        if in_addr == 0 || out_addr == 0 { return Err(BotError::NoDevice); }
        let in_dci = ((in_addr & 0x0F) * 2) + 1;
        let out_dci = (out_addr & 0x0F) * 2;

        let cdb = [0x28u8, 0, 0, 0, 0, 0, 0, 0, 1, 0]; // READ(10) LBA0, 1 block
        let tag = self.build_cbw(cbw_phys as *mut u8, 512, Direction::In, &cdb);
        unsafe { core::ptr::write_bytes(csw_phys as *mut u8, 0, 13); }
        dma_coherency::clean(cbw_phys as usize, 31);
        dma_coherency::clean_inval(csw_phys as usize, 13);

        // 1) CBW on bulk OUT.
        self.slots[slot_id as usize].bulk_out_ring.as_mut().unwrap()
            .push(Trb { parameter: cbw_phys, status: 31, control: 1 << 10 }).ok();

        // 2) Two chained IN data TRBs (256 B + 256 B). Clean the whole 512 B buffer to DRAM first,
        //    exactly like the single-TRB IN path. The completion event (IOC) rides the SECOND TRB;
        //    the first carries the CHAIN bit (1<<4) and no IOC. Wait on the second TRB's phys.
        dma_coherency::clean(data_phys as usize, 512);
        let data_trb_phys = {
            let ring = self.slots[slot_id as usize].bulk_in_ring.as_mut().unwrap();
            let base = ring.get_ptr();
            // TRB 1: 256 B, CHAIN, no IOC.
            ring.push(Trb { parameter: data_phys, status: 256, control: (1 << 10) | (1 << 4) }).ok();
            // TRB 2: 256 B, IOC (1<<5) + ISP (1<<2).
            let idx = ring.push(Trb { parameter: data_phys + 256, status: 256,
                control: (1 << 10) | (1 << 5) | (1 << 2) }).unwrap_or(0);
            base + (idx as u64) * 16
        };
        self.ring_doorbell(slot_id, out_dci as u32);
        self.ring_doorbell(slot_id, in_dci as u32);
        // BOT-PHASE: `run_bot_stage` now also returns the Transfer Event residue. This PIUSB-36
        // experiment path is the aarch64 twin of `bot_transfer_once` and carries the same
        // short-transfer hole fix 3 closes there; it is aarch64-lane and out of this arc's scope, so
        // the residue is bound and named rather than acted on. See §15 for the handoff.
        let (code, _residue) = self.run_bot_stage(slot_id, in_dci, out_dci, data_trb_phys)?;
        if code != 1 && code != 13 {
            if code == 4 || code == 6 { self.recover_bulk_stall(slot_id, true); return Err(BotError::Stall); }
            return Err(BotError::TransferError(code));
        }
        dma_coherency::inval(data_phys as usize, 512);

        // 3) CSW on bulk IN.
        let csw_trb_phys = {
            let ring = self.slots[slot_id as usize].bulk_in_ring.as_mut().unwrap();
            let base = ring.get_ptr();
            let idx = ring.push(Trb { parameter: csw_phys, status: 13, control: (1 << 10) | (1 << 5) }).unwrap_or(0);
            base + (idx as u64) * 16
        };
        self.ring_doorbell(slot_id, in_dci as u32);
        let (code, _csw_residue) = self.run_bot_stage(slot_id, in_dci, out_dci, csw_trb_phys)?;
        if code != 1 && code != 13 {
            if code == 4 || code == 6 { self.recover_bulk_stall(slot_id, true); return Err(BotError::Stall); }
            return Err(BotError::TransferError(code));
        }
        unsafe {
            dma_coherency::inval(csw_phys as usize, 13);
            let csw = core::slice::from_raw_parts(csw_phys as *const u8, 13);
            let csw_tag = (csw[4] as u32) | ((csw[5] as u32) << 8) | ((csw[6] as u32) << 16) | ((csw[7] as u32) << 24);
            let residue = (csw[8] as u32) | ((csw[9] as u32) << 8) | ((csw[10] as u32) << 16) | ((csw[11] as u32) << 24);
            let status = match csw[12] { 0 => CswStatus::Passed, 1 => CswStatus::Failed, 2 => CswStatus::PhaseError, _ => CswStatus::Unknown };
            if csw_tag != tag { return Err(BotError::TagMismatch); }
            Ok(BotResult { status, residue })
        }
    }

    /// PIUSB-36 witness printer: dump the first 16 bytes at `buf_phys` plus a discriminating
    /// verdict. With a `pattern` byte the verdict separates the three outcomes that matter:
    /// PATTERN-SURVIVED (the device never DMA-wrote), ZEROS (something wrote zeros over the
    /// pattern), DATA (real bytes landed). Read-only.
    #[cfg(target_arch = "aarch64")]
    fn piusb36_report(label: &str, buf_phys: u64, status: CswStatus, residue: u32, pattern: Option<u8>) {
        let d = unsafe { core::slice::from_raw_parts(buf_phys as *const u8, 16) };
        let all_zero = d.iter().all(|&b| b == 0);
        let verdict = match pattern {
            Some(p) if d.iter().all(|&b| b == p) => "PATTERN-SURVIVED(device-never-wrote)",
            _ if all_zero => "ZEROS(dma-wrote-zeros-or-nothing)",
            _ => "DATA(real-bytes-landed)",
        };
        serial_println!(
            ":: PIUSB: [piusb36] {} buf={:#x} CSW={:?} residue={} verdict={} — {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            label, buf_phys, status, residue, verdict,
            d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7],
            d[8], d[9], d[10], d[11], d[12], d[13], d[14], d[15]);
    }

    /// Busy-wait `ms` milliseconds off the free-running generic-timer counter (CNTVCT/CNTFRQ). Used
    /// only by the PIUSB-36 posted-write-visibility step — never on any hot path.
    #[cfg(target_arch = "aarch64")]
    fn piusb36_delay_ms(ms: u64) {
        let freq: u64;
        unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack, preserves_flags)); }
        if freq == 0 { return; }
        let budget = (freq / 1000).saturating_mul(ms);
        let start = crate::arch::now_cycles();
        while crate::arch::now_cycles().wrapping_sub(start) < budget { core::hint::spin_loop(); }
    }

    /// PIUSB-36: one-boot decisive experiment matrix for the Pi-only 512-B-read-returns-zeros wedge.
    /// READ CAPACITY (8 B) and INQUIRY-adjacent control transfers DMA correctly to the SAME heap pool,
    /// yet READ(10) 512 B returns Passed/residue=0/all-zero on Pi metal (P45), while the identical
    /// code shape read REAL data in the early pre-SMP IRQ-masked phase (P38) and QEMU virt/x86 are
    /// fine. This runs six read-only experiments in a single boot, each witnessed with first-16-bytes
    /// + a pattern verdict, to corner the variable: buffer region, DMA-lands-zeros-vs-never-lands,
    /// TD shape, mid-size transfer, and posted-write visibility. Bounded (~10 ms + transfers).
    #[cfg(target_arch = "aarch64")]
    fn piusb36_matrix(&mut self) {
        let slot = self.storage_slot;
        if slot == 0 { serial_println!(":: PIUSB: [piusb36] no storage slot — matrix skipped ::"); return; }
        let databuf = match self.slots[slot as usize].scsi_data_buffer {
            Some(p) => p as u64,
            None => { serial_println!(":: PIUSB: [piusb36] no scsi_data_buffer — matrix skipped ::"); return; }
        };
        let read10_lba0 = [0x28u8, 0, 0, 0, 0, 0, 0, 0, 1, 0];

        serial_println!(":: PIUSB: [piusb36] === experiment matrix (read-only, one boot) === ::");

        // --- Step 1: baseline READ(10) LBA0 into the CURRENT scsi_data_buffer (expect zeros on
        //     metal). Establishes the wedge is live this boot before the discriminating variants. ---
        match self.bot_transfer(slot, &read10_lba0, databuf, 512, Direction::In) {
            Ok(r) => Self::piusb36_report("step1-baseline-scsibuf", databuf, r.status, r.residue, None),
            Err(e) => serial_println!(":: PIUSB: [piusb36] step1 baseline ERR {:?} ::", e),
        }

        // --- Step 2: FRESH alloc_zeroed buffer PRE-FILLED with 0xA5, then READ(10) LBA0 into it.
        //     PATTERN-SURVIVED => the device never wrote (DMA never landed); ZEROS => something wrote
        //     zeros OVER the pattern (DMA landed zeros, or a stale write-back clobbered it); DATA =>
        //     the block landed. This is the critical 'never-lands vs lands-zeros' discriminator. ---
        {
            let layout = core::alloc::Layout::from_size_align(512, 64).unwrap();
            let fresh = unsafe { alloc::alloc::alloc_zeroed(layout) };
            if fresh.is_null() {
                serial_println!(":: PIUSB: [piusb36] step2 alloc failed ::");
            } else {
                unsafe { core::ptr::write_bytes(fresh, 0xA5, 512); }
                match self.bot_transfer(slot, &read10_lba0, fresh as u64, 512, Direction::In) {
                    Ok(r) => Self::piusb36_report("step2-fresh-A5-heap", fresh as u64, r.status, r.residue, Some(0xA5)),
                    Err(e) => serial_println!(":: PIUSB: [piusb36] step2 fresh-A5 ERR {:?} ::", e),
                }
                unsafe { alloc::alloc::dealloc(fresh, layout); }
            }
        }

        // --- Step 3: STATIC low buffer (kernel-image .bss, phys typically <4 MiB — a wholly
        //     different region from the 32 MiB heap). Pre-filled 0xA5, same READ(10) LBA0. Tests
        //     whether the wedge is region-specific (RC inbound-window / cache-color) vs universal. ---
        {
            let sbuf = core::ptr::addr_of_mut!(PIUSB36_STATIC_BUF) as *mut u8;
            unsafe { core::ptr::write_bytes(sbuf, 0xA5, 512); }
            match self.bot_transfer(slot, &read10_lba0, sbuf as u64, 512, Direction::In) {
                Ok(r) => Self::piusb36_report("step3-static-A5-low", sbuf as u64, r.status, r.residue, Some(0xA5)),
                Err(e) => serial_println!(":: PIUSB: [piusb36] step3 static ERR {:?} ::", e),
            }
        }

        // --- Step 4: INQUIRY (36 B) into scsi_data_buffer — a MID-SIZE control point between the
        //     8 B READ CAPACITY that works and the 512 B READ(10) that fails. If 36 B lands data,
        //     the failure threshold sits above 36 B (points at a length/burst boundary). ---
        {
            let inquiry = [0x12u8, 0, 0, 0, 36, 0, 0, 0, 0, 0];
            unsafe { core::ptr::write_bytes(databuf as *mut u8, 0xA5, 36); }
            match self.bot_transfer(slot, &inquiry, databuf, 36, Direction::In) {
                Ok(r) => Self::piusb36_report("step4-inquiry36-scsibuf", databuf, r.status, r.residue, Some(0xA5)),
                Err(e) => serial_println!(":: PIUSB: [piusb36] step4 inquiry ERR {:?} ::", e),
            }
        }

        // --- Step 5: READ(10) LBA0 as TWO chained TRBs (256 + 256) into scsi_data_buffer — a
        //     TD-shape variant of the same 512 B transfer. Discriminates TD shape from length. ---
        match self.piusb36_read10_two_trb(slot, databuf) {
            Ok(r) => Self::piusb36_report("step5-two-trb-scsibuf", databuf, r.status, r.residue, None),
            Err(e) => serial_println!(":: PIUSB: [piusb36] step5 two-TRB ERR {:?} ::", e),
        }

        // --- Step 6: POSTED-WRITE VISIBILITY. Re-read LBA0 (single TRB, 1 block): bot_transfer
        //     invalidates + we snapshot immediately (A). Then wait 1 ms, invalidate the SAME buffer
        //     AGAIN, and re-read (B) — WITHOUT re-issuing the transfer. If A is zeros but B is data,
        //     the controller's PCIe-posted DMA write was not yet globally visible in DRAM when the
        //     transfer event fired and we read it (the small-read race window closes for 512 B). If
        //     A == B, no posted-write timing race — the wedge is not a visibility latency. ---
        match self.bot_transfer(slot, &read10_lba0, databuf, 512, Direction::In) {
            Ok(r) => {
                let a: [u8; 16] = unsafe { core::ptr::read(databuf as *const [u8; 16]) };
                Self::piusb36_delay_ms(1);
                dma_coherency::inval(databuf as usize, 512);
                let b: [u8; 16] = unsafe { core::ptr::read(databuf as *const [u8; 16]) };
                let a_zero = a.iter().all(|&x| x == 0);
                let b_zero = b.iter().all(|&x| x == 0);
                let verdict = if a_zero && !b_zero { "POSTED-WRITE-RACE-CONFIRMED(A-zeros,B-data-after-1ms)" }
                    else if a_zero && b_zero { "no-race(both-zero-after-1ms-delay)" }
                    else { "immediate-read-already-had-data(no-race-hit)" };
                serial_println!(
                    ":: PIUSB: [piusb36] step6-posted-write buf={:#x} CSW={:?} residue={} verdict={} | A(immediate)={:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} | B(+1ms+inval)={:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
                    databuf, r.status, r.residue, verdict,
                    a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7],
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]);
            }
            Err(e) => serial_println!(":: PIUSB: [piusb36] step6 posted-write ERR {:?} ::", e),
        }

        serial_println!(":: PIUSB: [piusb36] === matrix complete === ::");
    }

    // ==================== PIUSB-37: chase the command itself ====================

    /// PIUSB-37 helper: dump the first 16 bytes at `phys` with a zeros/pattern/data verdict.
    #[cfg(target_arch = "aarch64")]
    fn piusb37_dump16(label: &str, phys: u64, status: CswStatus, residue: u32) {
        let d = unsafe { core::slice::from_raw_parts(phys as *const u8, 16) };
        let verdict = if d.iter().all(|&b| b == 0) { "ZEROS" } else { "DATA(real-bytes-landed)" };
        serial_println!(
            ":: PIUSB: [piusb37] {} buf={:#x} CSW={:?} residue={} verdict={} — {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            label, phys, status, residue, verdict,
            d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7],
            d[8], d[9], d[10], d[11], d[12], d[13], d[14], d[15]);
    }

    /// PIUSB-37: chase the READ(10)-returns-zeros wedge into the SCSI command itself. With the
    /// cache theory (§5a), the DMA-address theory (§5b), and — pending P46 — buffer/region/TD-shape/
    /// posted-write visibility (§5c) all pointing away from our transport, the residual suspect is
    /// the command or the device's response to it: a wrong LUN, a byte-swapped/zero transfer length,
    /// or a pending UNIT ATTENTION (post power-on/reset) under which some bridges return zero-filled
    /// data + GOOD status until the sense is cleared (which would explain why the early P38 path —
    /// a different reset/clear sequence — read real sectors while the deferred path reads zeros).
    /// Four read-only steps, each witnessed. aarch64-only; never compiled on x86.
    #[cfg(target_arch = "aarch64")]
    fn piusb37_matrix(&mut self) {
        let slot = self.storage_slot;
        if slot == 0 { serial_println!(":: PIUSB: [piusb37] no storage slot — matrix skipped ::"); return; }
        let (databuf, cbw_phys) = {
            let s = &self.slots[slot as usize];
            match (s.scsi_data_buffer, s.cbw_buffer) {
                (Some(d), Some(c)) => (d as u64, c as u64),
                _ => { serial_println!(":: PIUSB: [piusb37] no data/cbw buffer — matrix skipped ::"); return; }
            }
        };
        let read10_lba0 = [0x28u8, 0, 0, 0, 0, 0, 0, 0, 1, 0];

        serial_println!(":: PIUSB: [piusb37] === chase-the-command matrix (read-only, one boot) === ::");

        // --- Step 1: CBW AUDIT. Build the exact 31-byte CBW that bot_transfer hands to the
        //     controller (build_cbw writes the on-the-wire little-endian layout, so a post-build
        //     dump IS what the VL805 DMA-reads) for READ(10) LBA0 and, as a reference, INQUIRY.
        //     Decode + spec-check each field: dCBWSignature must be "USBC" (55 53 42 43),
        //     dCBWDataTransferLength must be 512 for READ(10) / 36 for INQUIRY (little-endian),
        //     bmCBWFlags 0x80 (device->host IN), bCBWLUN 0, bCBWCBLength = CDB len, and the CDB:
        //     READ(10) opcode 0x28 with LBA + transfer-length-in-blocks BIG-endian; a wrong LUN, a
        //     zero blocks field, or a byte-swapped LBA each yields exactly the zeros-with-Passed
        //     signature. This only builds into the CBW buffer — it issues no transfer. ---
        for (label, cdb, want_len) in [
            ("READ10", &read10_lba0[..], 512u32),
            ("INQUIRY", &[0x12u8, 0, 0, 0, 36, 0][..], 36u32),
        ] {
            let _ = self.build_cbw(cbw_phys as *mut u8, want_len, Direction::In, cdb);
            let c = unsafe { core::slice::from_raw_parts(cbw_phys as *const u8, 31) };
            let sig = (c[0] as u32) | ((c[1] as u32) << 8) | ((c[2] as u32) << 16) | ((c[3] as u32) << 24);
            let tag = (c[4] as u32) | ((c[5] as u32) << 8) | ((c[6] as u32) << 16) | ((c[7] as u32) << 24);
            let dxlen = (c[8] as u32) | ((c[9] as u32) << 8) | ((c[10] as u32) << 16) | ((c[11] as u32) << 24);
            let flags = c[12]; let lun = c[13]; let cblen = c[14];
            let sig_ok = sig == 0x43425355;
            let len_ok = dxlen == want_len;
            let flags_ok = flags == 0x80;
            let lun_ok = lun == 0;
            let cblen_ok = cblen as usize == cdb.len();
            serial_println!(
                ":: PIUSB: [piusb37] cbw-dump {} sig={:#010x}({}) tag={:#x} dCBWDataTransferLength={}({}) bmFlags={:#04x}({}) bCBWLUN={}({}) bCBWCBLength={}({}) ::",
                label, sig, if sig_ok {"USBC-ok"} else {"BAD"}, tag,
                dxlen, if len_ok {"ok"} else {"MISMATCH"},
                flags, if flags_ok {"IN-ok"} else {"BAD"},
                lun, if lun_ok {"ok"} else {"NONZERO!"},
                cblen, if cblen_ok {"ok"} else {"MISMATCH"});
            serial_println!(
                ":: PIUSB: [piusb37] cbw-dump {} CDB= {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
                label,
                c[15], c[16], c[17], c[18], c[19], c[20], c[21], c[22],
                c[23], c[24], c[25], c[26], c[27], c[28], c[29], c[30]);
            if label == "READ10" {
                // Opcode 0x28; LBA = c[17..21] BE; transfer length in blocks = c[22..24] BE.
                let opcode = c[15];
                let lba = ((c[17] as u32) << 24) | ((c[18] as u32) << 16) | ((c[19] as u32) << 8) | (c[20] as u32);
                let blocks = ((c[22] as u16) << 8) | (c[23] as u16);
                serial_println!(
                    ":: PIUSB: [piusb37] cbw-dump READ10 decode: opcode={:#04x}({}) LBA(BE)={} blocks(BE)={} — {} ::",
                    opcode, if opcode == 0x28 {"ok"} else {"BAD"}, lba, blocks,
                    if opcode == 0x28 && lba == 0 && blocks == 1 { "CDB well-formed — command is NOT the fault" }
                    else { "CDB MALFORMED — this alone would return zeros+Passed" });
            }
        }

        // --- Step 2: command-set + known-nonzero-LBA matrix. READ(10) of mid-disk FAT-area LBAs
        //     (8192, 16384) — if LBA0 is a zero-filled reserved sector but these read data, the wedge
        //     is not universal. Then LBA0 via READ(12) (0xA8) and READ(16) (0x88): some bridges
        //     mishandle one command-set variant while another works. All read-only, first-16 dumped. ---
        for &lba in &[8192u32, 16384u32] {
            let cdb = [0x28u8, 0,
                (lba >> 24) as u8, (lba >> 16) as u8, (lba >> 8) as u8, lba as u8,
                0, 0, 1, 0];
            unsafe { core::ptr::write_bytes(databuf as *mut u8, 0xA5, 512); }
            match self.bot_transfer(slot, &cdb, databuf, 512, Direction::In) {
                Ok(r) => {
                    let d = unsafe { core::slice::from_raw_parts(databuf as *const u8, 16) };
                    let verdict = if d.iter().all(|&b| b == 0) { "ZEROS" } else { "DATA(real-bytes-landed)" };
                    serial_println!(
                        ":: PIUSB: [piusb37] read10-lba{} buf={:#x} CSW={:?} residue={} verdict={} — {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
                        lba, databuf, r.status, r.residue, verdict,
                        d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7],
                        d[8], d[9], d[10], d[11], d[12], d[13], d[14], d[15]);
                }
                Err(e) => serial_println!(":: PIUSB: [piusb37] read10-lba{} ERR {:?} ::", lba, e),
            }
        }
        // READ(12) LBA0 (opcode 0xA8): LBA BE in bytes 2..6, transfer length BE (blocks) in 6..10.
        {
            let cdb = [0xA8u8, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0];
            unsafe { core::ptr::write_bytes(databuf as *mut u8, 0xA5, 512); }
            match self.bot_transfer(slot, &cdb, databuf, 512, Direction::In) {
                Ok(r) => Self::piusb37_dump16("read12-lba0", databuf, r.status, r.residue),
                Err(e) => serial_println!(":: PIUSB: [piusb37] read12-lba0 ERR {:?} ::", e),
            }
        }
        // READ(16) LBA0 (opcode 0x88): LBA BE in bytes 2..10, transfer length BE (blocks) in 10..14.
        {
            let cdb = [0x88u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0];
            unsafe { core::ptr::write_bytes(databuf as *mut u8, 0xA5, 512); }
            match self.bot_transfer(slot, &cdb, databuf, 512, Direction::In) {
                Ok(r) => Self::piusb37_dump16("read16-lba0", databuf, r.status, r.residue),
                Err(e) => serial_println!(":: PIUSB: [piusb37] read16-lba0 ERR {:?} ::", e),
            }
        }

        // --- Step 3: REQUEST SENSE immediately after a zeros-read. STRONG CANDIDATE: some bridges
        //     return zero-filled data + GOOD status while a UNIT ATTENTION (power-on/reset, sense
        //     key 0x06, ASC 0x29) is pending — the sense stays latched until read. Issue a READ(10)
        //     LBA0 (the zeros-read), then REQUEST SENSE (0x03, 18 B) and decode the sense buffer:
        //     response code, sense key, ASC/ASCQ. A non-zero sense key here is the smoking gun. ---
        {
            unsafe { core::ptr::write_bytes(databuf as *mut u8, 0, 512); }
            let read_res = self.bot_transfer(slot, &read10_lba0, databuf, 512, Direction::In);
            match read_res {
                Ok(r) => Self::piusb37_dump16("presense-read10-lba0", databuf, r.status, r.residue),
                Err(e) => serial_println!(":: PIUSB: [piusb37] presense-read10 ERR {:?} ::", e),
            }
            let sense_cdb = [0x03u8, 0, 0, 0, 18, 0];
            unsafe { core::ptr::write_bytes(databuf as *mut u8, 0, 18); }
            match self.bot_transfer(slot, &sense_cdb, databuf, 18, Direction::In) {
                Ok(r) => {
                    let s = unsafe { core::slice::from_raw_parts(databuf as *const u8, 18) };
                    let resp = s[0] & 0x7f;
                    let key = s[2] & 0x0f;
                    let asc = s[12]; let ascq = s[13];
                    let name = match key {
                        0x00 => "NO SENSE", 0x02 => "NOT READY", 0x03 => "MEDIUM ERROR",
                        0x04 => "HARDWARE ERROR", 0x05 => "ILLEGAL REQUEST",
                        0x06 => "UNIT ATTENTION", 0x0b => "ABORTED COMMAND", _ => "other",
                    };
                    serial_println!(
                        ":: PIUSB: [piusb37] REQUEST-SENSE CSW={:?} residue={} response={:#04x} key={:#x}({}) ASC={:#04x} ASCQ={:#04x} — {} ::",
                        r.status, r.residue, resp, key, name, asc, ascq,
                        if key == 0x06 { "UNIT ATTENTION PENDING — strong candidate for zeros-then-good" }
                        else if key == 0x00 { "no pending sense — UA theory does NOT explain the zeros" }
                        else { "pending non-UA sense condition" });
                    serial_println!(
                        ":: PIUSB: [piusb37] REQUEST-SENSE raw= {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
                        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8],
                        s[9], s[10], s[11], s[12], s[13], s[14], s[15], s[16], s[17]);
                }
                Err(e) => serial_println!(":: PIUSB: [piusb37] REQUEST-SENSE ERR {:?} ::", e),
            }
        }

        // --- Step 4: TEST UNIT READY drain + retry READ(10). Standard bring-up practice: issue
        //     TUR / REQUEST SENSE until the unit reports ready (clearing any latched UA), THEN
        //     re-read LBA0. If the zeros become data after the sense-clear, THAT is the fix: drain
        //     TUR/sense before the first read. If they stay zeros, the UA theory is refuted and the
        //     residual is a genuine device/transport response to the READ command itself. ---
        {
            let mut ready = false;
            for attempt in 0..8 {
                match self.scsi_test_unit_ready(slot) {
                    Ok(CswStatus::Passed) => {
                        serial_println!(":: PIUSB: [piusb37] TUR attempt {} => Passed (ready) ::", attempt);
                        ready = true; break;
                    }
                    Ok(st) => {
                        serial_println!(":: PIUSB: [piusb37] TUR attempt {} => {:?}; clearing sense ::", attempt, st);
                        let sense_cdb = [0x03u8, 0, 0, 0, 18, 0];
                        let _ = self.bot_transfer(slot, &sense_cdb, databuf, 18, Direction::In);
                    }
                    Err(e) => serial_println!(":: PIUSB: [piusb37] TUR attempt {} ERR {:?} ::", attempt, e),
                }
            }
            unsafe { core::ptr::write_bytes(databuf as *mut u8, 0, 512); }
            match self.bot_transfer(slot, &read10_lba0, databuf, 512, Direction::In) {
                Ok(r) => {
                    Self::piusb37_dump16("postready-read10-lba0", databuf, r.status, r.residue);
                    let d = unsafe { core::slice::from_raw_parts(databuf as *const u8, 16) };
                    let still_zeros = d.iter().all(|&b| b == 0);
                    serial_println!(
                        ":: PIUSB: [piusb37] post-TUR verdict: ready={} data={} — {} ::",
                        ready, if still_zeros {"ZEROS"} else {"REAL"},
                        if !still_zeros { "SENSE-CLEAR IS THE FIX: drain TUR/REQUEST-SENSE before first read" }
                        else { "still zeros after ready — UA/sense theory REFUTED; residual is READ-command response" });
                }
                Err(e) => serial_println!(":: PIUSB: [piusb37] postready-read10 ERR {:?} ::", e),
            }
        }

        serial_println!(":: PIUSB: [piusb37] === chase-the-command matrix complete === ::");
    }

    // ==================== PIUSB-38: stall recovery + low-LBA-zeros bisect ====================

    /// PIUSB-38: prove BOT Reset Recovery on the storage pipe, then run the low-LBA-zeros bisect.
    /// Three read-only phases in one boot (each witnessed `:: PIUSB: [piusb38] ... ::`):
    ///
    ///   * **Phase 1 — induced-stall recovery.** Issue an UNSUPPORTED command (READ(16), opcode
    ///     0x88) which the bench VL805 stick STALLs (completion code 4). `bot_transfer_once` clears
    ///     the halt and STILL collects the CSW to resync; if the status phase also fails, the error
    ///     reaches `bot_transfer`, which escalates to a full Bulk-Only Mass Storage Reset and retries
    ///     once. We then prove the pipe is ALIVE: TEST UNIT READY and REQUEST SENSE must COMPLETE
    ///     (not Timeout) afterwards. Pre-P48 the stall left the pipe wedged and every later command
    ///     timed out (the P47 wall).
    ///   * **Phase 2 — explicit full reset.** Call `recover_bot_full` directly and re-prove TUR, so
    ///     the class-level reset path is exercised end to end even when Phase 1's inline recovery
    ///     already sufficed.
    ///   * **Phase 3 — low-LBA-zeros bisect.** Read a ladder LBA 0,1,2,4,8,…,8192 (same READ(10)
    ///     CDB shape, only the LBA field differs), each with a zeros/data verdict + residue, to find
    ///     the zeros→data boundary; then diff the LBA0 vs LBA8192 buffers (first differing byte).
    ///     Because only the LBA field changes, any zeros-vs-data split is region-specific, not a
    ///     command-shape fault (null-hypothesis-our-code: a buffer/cache/aliasing effect on the low
    ///     region, not the SCSI command).
    ///
    /// Read-only (no WRITE(10)); bounded (≈20 transfers + a couple of resets). aarch64-only; never
    /// compiled on x86. Inert in QEMU raspi4b (no VL805 → no storage slot); virt exercises every
    /// step (READ(16) may be supported there — the recovery path stays correct either way).
    #[cfg(target_arch = "aarch64")]
    fn piusb38_matrix(&mut self) {
        let slot = self.storage_slot;
        if slot == 0 { serial_println!(":: PIUSB: [piusb38] no storage slot — matrix skipped ::"); return; }
        let databuf = match self.slots[slot as usize].scsi_data_buffer {
            Some(p) => p as u64,
            None => { serial_println!(":: PIUSB: [piusb38] no scsi_data_buffer — matrix skipped ::"); return; }
        };

        serial_println!(":: PIUSB: [piusb38] === stall-recovery + low-LBA bisect (read-only, one boot) === ::");

        // --- Phase 1: induce a stall, then prove the pipe recovered. ---
        // READ(16), opcode 0x88, LBA0, 1 block — unsupported by many bulk bridges (STALL, code 4).
        let read16_lba0 = [0x88u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0];
        unsafe { core::ptr::write_bytes(databuf as *mut u8, 0xA5, 512); }
        match self.bot_transfer(slot, &read16_lba0, databuf, 512, Direction::In) {
            Ok(r) => serial_println!(
                ":: PIUSB: [piusb38] induced-read16 CSW={:?} residue={} (no stall — device accepted READ16) ::",
                r.status, r.residue),
            Err(BotError::Stall) => serial_println!(
                ":: PIUSB: [piusb38] induced-read16 STALL — inline BOT reset-recovery ran ::"),
            Err(e) => serial_println!(":: PIUSB: [piusb38] induced-read16 ERR {:?} (recovery ran) ::", e),
        }
        // The pipe must be ALIVE now: TUR + REQUEST SENSE must COMPLETE (not Timeout).
        let tur1 = self.scsi_test_unit_ready(slot);
        let sense_cdb = [0x03u8, 0, 0, 0, 18, 0];
        unsafe { core::ptr::write_bytes(databuf as *mut u8, 0, 18); }
        let sense1 = self.bot_transfer(slot, &sense_cdb, databuf, 18, Direction::In);
        let recovered = !matches!(tur1, Err(BotError::Timeout))
            && !matches!(sense1, Err(BotError::Timeout));
        serial_println!(
            ":: PIUSB: [piusb38] post-stall TUR={:?} REQUEST-SENSE={:?} — {} ::",
            tur1, sense1.as_ref().map(|r| r.status),
            if recovered { "PIPE RECOVERED (TUR+SENSE completed — stall no longer wedges the pipe)" }
            else { "PIPE STILL WEDGED (a command timed out after the stall)" });

        // --- Phase 2: exercise the full Bulk-Only Reset Recovery path explicitly, then re-prove. ---
        // `Stall` is the honest cause here: Phase 1 induced one, and it is what the witness records.
        // BOT-RESCUE M3 witness 6: no failed stage to hand over — this call is a deliberate
        // exercise of the recovery path, not the aftermath of one. `None` is the honest record.
        let full_ok = self.recover_bot_full(slot, BotError::Stall, None);
        serial_println!(":: PIUSB: [piusb38] explicit recover_bot_full -> {} ::",
            if full_ok { "ok" } else { "incomplete" });
        let tur2 = self.scsi_test_unit_ready(slot);
        serial_println!(
            ":: PIUSB: [piusb38] post-full-reset TUR={:?} — {} ::",
            tur2,
            if matches!(tur2, Err(BotError::Timeout)) { "pipe dead after full reset" }
            else { "pipe alive after full Bulk-Only Reset Recovery" });

        // --- Phase 3: low-LBA-zeros bisect. Read ladder; find the zeros→data boundary. ---
        let ladder: [u32; 15] = [0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];
        let mut first_data_lba: Option<u32> = None;
        let mut last_zero_lba: Option<u32> = None;
        let mut lba0_first16 = [0u8; 16];
        let mut lba8192_first16 = [0u8; 16];
        for &lba in &ladder {
            let cdb = [0x28u8, 0,
                (lba >> 24) as u8, (lba >> 16) as u8, (lba >> 8) as u8, lba as u8,
                0, 0, 1, 0]; // READ(10), 1 block
            unsafe { core::ptr::write_bytes(databuf as *mut u8, 0xA5, 512); }
            match self.bot_transfer(slot, &cdb, databuf, 512, Direction::In) {
                Ok(r) => {
                    let d = unsafe { core::slice::from_raw_parts(databuf as *const u8, 16) };
                    let all_zero = d.iter().all(|&b| b == 0);
                    let all_a5 = d.iter().all(|&b| b == 0xA5);
                    let verdict = if all_a5 { "PATTERN-SURVIVED(device-never-wrote)" }
                        else if all_zero { "ZEROS" } else { "DATA(real-bytes-landed)" };
                    if lba == 0 { lba0_first16.copy_from_slice(d); }
                    if lba == 8192 { lba8192_first16.copy_from_slice(d); }
                    if !all_zero && !all_a5 && first_data_lba.is_none() { first_data_lba = Some(lba); }
                    if all_zero { last_zero_lba = Some(lba); }
                    serial_println!(
                        ":: PIUSB: [piusb38] ladder-lba{} CSW={:?} residue={} verdict={} — {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
                        lba, r.status, r.residue, verdict,
                        d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7],
                        d[8], d[9], d[10], d[11], d[12], d[13], d[14], d[15]);
                }
                Err(e) => serial_println!(":: PIUSB: [piusb38] ladder-lba{} ERR {:?} ::", lba, e),
            }
        }
        // Boundary verdict.
        match (last_zero_lba, first_data_lba) {
            (Some(z), Some(d)) => serial_println!(
                ":: PIUSB: [piusb38] bisect boundary: last-zeros=LBA{} first-data=LBA{} — zeros→data split IS region-specific (same CDB shape) ::", z, d),
            (Some(z), None) => serial_println!(
                ":: PIUSB: [piusb38] bisect boundary: ALL ladder LBAs zeros (last=LBA{}) — no data landed on any low LBA ::", z),
            (None, Some(d)) => serial_println!(
                ":: PIUSB: [piusb38] bisect boundary: data from the first LBA (LBA{}) — no low-LBA zeros this boot ::", d),
            (None, None) => serial_println!(
                ":: PIUSB: [piusb38] bisect boundary: no zeros and no data (pattern survived / errors) — see per-LBA lines ::"),
        }
        // Diff LBA0 vs LBA8192 (same READ(10) shape, only the LBA field differs).
        let first_diff = (0..16).find(|&i| lba0_first16[i] != lba8192_first16[i]);
        match first_diff {
            Some(i) => serial_println!(
                ":: PIUSB: [piusb38] lba0-vs-lba8192 diff: first differ at byte {} (lba0={:#04x} lba8192={:#04x}) — identical command path, divergent data ⇒ region/buffer effect not command-shape ::",
                i, lba0_first16[i], lba8192_first16[i]),
            None => serial_println!(
                ":: PIUSB: [piusb38] lba0-vs-lba8192 diff: first-16 IDENTICAL (both {}) ::",
                if lba0_first16.iter().all(|&b| b == 0) { "zeros" } else { "equal-nonzero" }),
        }

        serial_println!(":: PIUSB: [piusb38] === stall-recovery + low-LBA bisect complete === ::");
    }

    /// USB-WRITE-2: recover a halted bulk endpoint after a STALL (completion code 4) or Babble
    /// (6) so one faulted transfer cannot dead-line every later BOT command on the slot. This is
    /// the standard USB BOT clear-stall sequence, host + device side:
    ///   1. **Reset Endpoint** (xHCI command TRB type 14): moves the endpoint context from the
    ///      Halted state to Stopped and clears the host-side data-toggle/sequence.
    ///   2. **Set TR Dequeue Pointer** (command TRB type 16): repoints the transfer ring's dequeue
    ///      pointer PAST the faulted TRB (the current enqueue slot + live cycle), so restarting the
    ///      endpoint does not re-fetch the command that stalled.
    ///   3. **CLEAR_FEATURE(ENDPOINT_HALT)** (EP0 control, bmRequestType 0x02, bRequest 0x01,
    ///      wValue 0 = ENDPOINT_HALT, wIndex = endpoint address): clears the DEVICE-side halt so it
    ///      resumes accepting transactions on that pipe.
    /// `ep_in` selects the bulk IN (CSW / READ data) vs bulk OUT (WRITE data) endpoint. Best-effort:
    /// each step logs but the sequence proceeds — a device that NAKs one step should not block the
    /// others. Runs in the same safe synchronous polled context as the BOT pump itself.
    fn recover_bulk_stall(&mut self, slot_id: u8, ep_in: bool) {
        let ep_addr = {
            let slot = &self.slots[slot_id as usize];
            if ep_in { slot.bulk_in_ep } else { slot.bulk_out_ep }
        };
        if ep_addr == 0 { return; }
        let dci = ((ep_addr as u32) & 0x0F) * 2 + if ep_in { 1 } else { 0 };
        serial_println!("xHCI: [usbw] bulk STALL recovery slot {} ep {:#04x} (dci {})", slot_id, ep_addr, dci);

        // 1+2) Host-side: Reset Endpoint (Halted -> Stopped) + Set TR Dequeue past the faulted TRB.
        //      `resync_bulk_ep` is the single host-side implementation, shared with the class-level
        //      `recover_bot_full`; it reads the EP State first, so the Halted endpoint a STALL
        //      leaves behind takes the Reset-Endpoint arm exactly as this path always did.
        self.resync_bulk_ep(slot_id, dci as u8, ep_in, BotError::Stall);

        // 3) Device-side CLEAR_FEATURE(ENDPOINT_HALT) on EP0. wIndex carries the full endpoint
        //    address (with the direction bit for an IN endpoint).
        match self.sync_control(slot_id, 0x02, 0x01, 0x0000, ep_addr as u16, 0, 0, false) {
            Ok(1) => {}
            other => serial_println!("xHCI: [usbw] CLEAR_FEATURE(HALT) unexpected {:?}", other),
        }
    }

    /// FULL BOT **Reset Recovery** (USB Mass Storage Bulk-Only Transport 1.0 §5.3.3/§5.3.4) — the
    /// class-level escalation the spec prescribes when clearing one endpoint's halt does not un-wedge
    /// the pipe (the P47 wall: after an unrecovered stall on the storage slot, every later READ /
    /// REQUEST-SENSE / TEST-UNIT-READY on that pipe timed out — the bulk pipe halted and never
    /// recovered, while HID kept flowing, so the interrupter was NOT globally wedged; the transfer
    /// path of the storage slot alone was dead), and equally the recovery a plain marginal TIMEOUT
    /// needs: a stage that never completed leaves the device parked mid-BOT with the host's stranded
    /// TRBs still queued, so every later transaction sees a tag mismatch.
    ///
    /// This is the ONE class-level recovery on the BOT path. It is the escalation `bot_transfer_once`
    /// used to reach for inline (PIUSB-38) and the verified precondition `bot_transfer` requires
    /// before it spends its single retry — hence the `bool`: it returns true only if EVERY step
    /// succeeded, so a half-recovered device is never driven further.
    ///
    /// Steps, in the spec's order:
    ///   1. **Bulk-Only Mass Storage Reset** (BOT 1.0 §3.1): class request `bmRequestType 0x21`
    ///      (host->device, class, interface), `bRequest 0xFF`, `wValue 0`,
    ///      `wIndex = bInterfaceNumber` of the mass-storage interface, `wLength 0`. This returns the
    ///      DEVICE's Bulk-Only state machine to the "ready for CBW" state — the step that ends the
    ///      desynchronisation. Per §3.1 it does NOT clear the endpoint halts, which is why step 2
    ///      exists.
    ///   2. **CLEAR_FEATURE(ENDPOINT_HALT)** on both bulk endpoints (USB 2.0 §9.4.1, feature
    ///      selector ENDPOINT_HALT = 0; BOT 1.0 §5.3.3 steps 2 and 3): `bmRequestType 0x02`
    ///      (host->device, standard, endpoint), `bRequest 0x01`, `wValue 0`, `wIndex = endpoint
    ///      ADDRESS` (direction bit included). This also resets the endpoint's data toggle /
    ///      sequence number device-side.
    ///   3. **xHCI hygiene** (xHCI 1.2 §4.6.8 Reset Endpoint, §4.6.9 Stop Endpoint, §4.6.10 Set TR
    ///      Dequeue Pointer) per bulk endpoint — see `resync_bulk_ep`. The USB-level reset above is
    ///      invisible to the host controller: its endpoint contexts are still Halted or Running, and
    ///      its dequeue pointers still sit on the stranded TRBs of the failed transaction. Without
    ///      this a retry would either be refused (a Halted EP ignores the doorbell) or would replay
    ///      the stranded CBW/data/CSW at a device that has just been reset.
    ///
    /// Every step is one bounded `sync_control` / `run_command_sync`; there is no loop, and this
    /// never calls `bot_transfer`, so recursion is impossible. Runs in the same safe synchronous
    /// polled context as the BOT pump itself.
    ///
    /// BOT-RESCUE M3 witness 6: `failed` is the pending record of the stage that actually failed,
    /// handed in by the caller. It used to be read out of `self.bot_pending` here — but
    /// `run_bot_stage` had already TAKEN that record on its way to reporting the error, so the read
    /// was always `None` and the `recover evidence` line printed `pipe=none wait_trb=0x0
    /// stage_done=no stage_cc=0` on every capture ever taken. That was a structural lie about the
    /// driver's own state, not a finding about the device. Callers with no record to hand (the
    /// PIUSB-38 aarch64 matrix, which calls recovery directly rather than after a failed stage)
    /// pass `None` and get the honest `pipe=none`.
    fn recover_bot_full(&mut self, slot_id: u8, cause: BotError, failed: Option<BotPending>) -> bool {
        let (in_ep, out_ep, intf) = {
            let s = &self.slots[slot_id as usize];
            (s.bulk_in_ep, s.bulk_out_ep, s.storage_intf)
        };
        if in_ep == 0 || out_ep == 0 { return false; }
        let in_dci = ((in_ep & 0x0F) * 2) + 1;
        let out_dci = (out_ep & 0x0F) * 2;
        BOT_RECOVER_COUNT.fetch_add(1, Ordering::Relaxed);
        serial_println!(
            "xHCI: [usbw] FULL BOT reset-recovery slot {} (intf {}, bulk in {:#04x}/out {:#04x})",
            slot_id, intf, in_ep, out_ep);
        serial_println!(
            ":: BOT: recover begin cause={:?} slot={} ep={:#x}/{:#x} iface={} n={} ::",
            cause, slot_id, in_ep, out_ep, intf, BOT_RECOVER_COUNT.load(Ordering::Relaxed));

        // Any half-armed pending stage from the failed transaction must not be matched against an
        // event raised during recovery's own control transfers.
        self.bot_pending = None;

        // BOTEV: EP State of both bulk endpoints as recovery FINDS them (xHCI 1.2 §6.2.3:
        // 0=Disabled 1=Running 2=Halted 3=Stopped 4=Error). A plain timeout should leave them
        // Running; a stall should leave the faulted one Halted. Anything else means the driver's
        // model of the pipe is wrong before a single recovery command is issued.
        let (in_s0, out_s0) = (self.ep_state_of(slot_id, in_dci), self.ep_state_of(slot_id, out_dci));
        serial_println!(
            ":: BOT: recover entry epin={} epout={} indci={} outdci={} cmdring={} ::",
            in_s0, out_s0, in_dci, out_dci,
            if self.cmd_ring_stopped { "stopped" } else { "running" });

        // 1) Bulk-Only Mass Storage Reset (class, targets the MSC interface).
        let reset_res = self.sync_control(slot_id, 0x21, 0xFF, 0x0000, intf as u16, 0, 0, false);
        let reset_ok = match reset_res {
            Ok(1) => { serial_println!("xHCI: [usbw] Bulk-Only Mass Storage Reset OK (slot {})", slot_id); true }
            other => { serial_println!("xHCI: [usbw] Bulk-Only Mass Storage Reset unexpected {:?}", other); false }
        };
        {
            // BOTEV: the class reset rides EP0, so its failure has three shapes — an error
            // completion code, a control transfer that never completed, or a missing EP0 ring.
            let (cc, why) = match reset_res {
                Ok(1) => (1u8, "ok"),
                Ok(c) => (c, "cc-error"),
                Err(()) => (0, "nocompletion"),
            };
            serial_println!(
                ":: BOT: recover stage=msc-reset iface={} ok={} cc={} why={} epin={}->{} epout={}->{} ::",
                intf, if reset_ok { "yes" } else { "no" }, cc, why,
                in_s0, self.ep_state_of(slot_id, in_dci),
                out_s0, self.ep_state_of(slot_id, out_dci));
        }

        // 2) CLEAR_FEATURE(ENDPOINT_HALT) on both bulk endpoints — IN first, then OUT (§5.3.3).
        let mut halts_ok = true;
        for (ep_addr, dci) in [(in_ep, in_dci), (out_ep, out_dci)] {
            let before = self.ep_state_of(slot_id, dci);
            let r = self.sync_control(slot_id, 0x02, 0x01, 0x0000, ep_addr as u16, 0, 0, false);
            match r {
                Ok(1) => {}
                other => {
                    serial_println!("xHCI: [usbw] CLEAR_FEATURE(HALT) ep {:#04x} unexpected {:?}", ep_addr, other);
                    halts_ok = false;
                }
            }
            let (cc, why) = match r {
                Ok(1) => (1u8, "ok"),
                Ok(c) => (c, "cc-error"),
                Err(()) => (0, "nocompletion"),
            };
            serial_println!(
                ":: BOT: recover stage=clear-halt ep={:#x} dci={} ok={} cc={} why={} epstate={}->{} ::",
                ep_addr, dci, if matches!(r, Ok(1)) { "yes" } else { "no" }, cc, why,
                before, self.ep_state_of(slot_id, dci));
        }

        // 3) xHCI-side endpoint + ring resynchronisation (state-aware: Reset vs Stop Endpoint).
        let in_ring = self.resync_bulk_ep(slot_id, in_dci, true, cause);
        let out_ring = self.resync_bulk_ep(slot_id, out_dci, false, cause);
        let ring_ok = in_ring && out_ring;

        serial_println!(
            ":: BOT: recover done reset={} halts={} ring={} ::",
            if reset_ok { "ok" } else { "fail" },
            if halts_ok { "cleared" } else { "fail" },
            if ring_ok { "resync" } else { "fail" });

        let ok = reset_ok && halts_ok && ring_ok;
        if !ok {
            // BOTEV: ONE follow-up line carrying the failed transaction's own state — which pipe its
            // stranded TRB sat on, whether its stage ever reported done and with what completion
            // code, and the CSW buffer as the controller left it (the CSW is DMA-written, so on a
            // timeout it is normally still the pre-transfer zero fill; a VALID signature here means
            // the status actually landed and only the event went missing — a completely different
            // fault). Reads only; the CSW buffer is invalidated first because the controller may
            // have written it behind our cache (no-op on x86).
            let (ring_name, wait_trb, stage_done, stage_cc) = match failed {
                Some(p) => {
                    let slot = &self.slots[slot_id as usize];
                    let name = if slot.bulk_in_ring.as_ref().is_some_and(|r| r.contains(p.wait_trb_phys)) {
                        "in"
                    } else if slot.bulk_out_ring.as_ref().is_some_and(|r| r.contains(p.wait_trb_phys)) {
                        "out"
                    } else {
                        "unknown"
                    };
                    (name, p.wait_trb_phys, p.done, p.completion_code)
                }
                None => ("none", 0, false, 0),
            };
            let (sig, ctag, residue, bstatus) = match self.slots[slot_id as usize].csw_buffer {
                Some(p) => unsafe {
                    dma_coherency::inval(p as usize, 13);
                    let c = core::slice::from_raw_parts(p as *const u8, 13);
                    let rd = |o: usize| (c[o] as u32) | ((c[o + 1] as u32) << 8)
                        | ((c[o + 2] as u32) << 16) | ((c[o + 3] as u32) << 24);
                    (rd(0), rd(4), rd(8), c[12])
                },
                None => (0, 0, 0, 0xFF),
            };
            serial_println!(
                ":: BOT: recover evidence cause={:?} pipe={} wait_trb={:#x} stage_done={} stage_cc={} csw_sig={:#x} csw_tag={:#x} residue={} csw_status={} epin={} epout={} ::",
                cause, ring_name, wait_trb, if stage_done { "yes" } else { "no" }, stage_cc,
                sig, ctag, residue, bstatus,
                self.ep_state_of(slot_id, in_dci), self.ep_state_of(slot_id, out_dci));
        }
        if ok {
            BOT_RECOVER_OK.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }

    // ==================== BOT-RESCUE: escalation, back-off, surrender ====================

    /// Rate of `crate::arch::now_cycles()` in ticks per millisecond.
    ///
    /// Deliberately NOT derived from `hw_wait_budget()`. That budget is a policy number ("how long
    /// may a wedged handshake burn before we call it dead"), and the settles below are SPEC numbers
    /// (VBUS de-energise, bPwrOn2PwrGood, attach debounce, a flash controller's internal stall
    /// window). Tying one to the other would silently rescale every USB timing constant the day the
    /// timeout policy changed. Each arch answers from its own timebase, with an honest fallback:
    ///   * x86_64 — the calibrated invariant TSC (`apic::tsc_hz`), or a 2 GHz guess before
    ///     calibration has run. A wrong guess makes a settle longer or shorter, never unsound.
    ///   * aarch64 — CNTFRQ_EL0, the generic timer's declared rate (~54 MHz Pi 4, ~62.5 MHz virt),
    ///     or 54 MHz if the register reads zero.
    fn cycles_per_ms() -> u64 {
        #[cfg(target_arch = "x86_64")]
        {
            let hz = crate::arch::apic::tsc_hz();
            if hz != 0 { (hz / 1000).max(1) } else { 2_000_000 }
        }
        #[cfg(target_arch = "aarch64")]
        {
            let freq: u64;
            unsafe {
                core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq,
                    options(nomem, nostack, preserves_flags));
            }
            if freq != 0 { (freq / 1000).max(1) } else { 54_000 }
        }
    }

    /// BOOTPACE M3 — the polled pumps' pre-`hlt()` spin window, in `now_cycles` units. ~200 µs.
    ///
    /// A SPEC-scale number, and deliberately NOT derived from `hw_wait_budget()`, for the same
    /// reason `cycles_per_ms` is not: that budget is a policy ("how long may a wedged handshake burn
    /// before we call it dead"), and rescaling this window the day the timeout policy changed would
    /// be a silent behaviour change in the hot path. 200 µs is chosen against the hardware instead:
    /// a healthy xHC posts a bulk or control completion in single-digit to low-tens of
    /// microseconds, so the window covers a completion with an order of magnitude to spare while
    /// remaining far below the 1 ms the alternative costs.
    fn spin_window() -> u64 {
        (Self::cycles_per_ms() / 5).max(1)
    }

    /// Busy-poll the event ring for at most `window` counter ticks, returning `true` the moment an
    /// event is drained (so the caller re-checks its own pending record) and `false` if the window
    /// expired with the ring still empty.
    ///
    /// BOOTPACE M3 — why this exists. All three synchronous pumps below waited by calling
    /// `crate::hlt()`, which on x86/Pi/aarch64-virt sleeps until the next INTERRUPT. With xHCI
    /// interrupts not enabled (`IRQ_COUNT=0` on every boot), the only thing that ever wakes it is
    /// the 1 kHz APIC timer — so EVERY awaited stage cost at least one full tick regardless of how
    /// fast the controller actually answered, and §17.4 recorded that quantisation as a permanent
    /// cost of the polled design. It is not permanent; it is only the cost of sleeping FIRST.
    /// Spinning for a spec-scale window before sleeping collects the completion in the microseconds
    /// it actually takes, and the hlt path stays exactly as it was for everything slower.
    ///
    /// This changes no budget, no timeout, no wall-clock deadline and no interrupt state. Past the
    /// window, behaviour is byte-identical to before: one `hlt()` per pass, the same deadline
    /// arithmetic, the same timeout lines.
    fn spin_for_event(&mut self, window: u64) -> bool {
        let start = crate::arch::now_cycles();
        while crate::arch::now_cycles().wrapping_sub(start) < window {
            if self.drain_event_ring_once() {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Busy-settle `ms` milliseconds off the free-running counter, draining the event ring as it
    /// goes. Draining matters: a late completion for the transaction that just timed out, or a Port
    /// Status Change raised by the power cycle in escalation (b), must be consumed here rather than
    /// left to be mistaken for the next stage's completion. `bot_pending` is already `None` on every
    /// path that reaches here (`run_bot_stage` took it), so nothing in flight can be claimed by a
    /// stale record. Bounded by construction; no allocation, no lock.
    fn settle_ms(&mut self, ms: u64) {
        let budget = Self::cycles_per_ms().saturating_mul(ms);
        let start = crate::arch::now_cycles();
        while crate::arch::now_cycles().wrapping_sub(start) < budget {
            if self.drain_event_ring_once() {
                continue;
            }
            core::hint::spin_loop();
        }
    }

    /// BOT-RESCUE escalation (a), **rewritten by ONSET-2 (M1a)**: rebase both bulk rings onto a
    /// known-zero producer state and repoint the controller at their bases — using only commands
    /// that are legal from the state the slot is actually in.
    ///
    /// ## Why the old rung (Reset Device -> Configure Endpoint) was retired
    ///
    /// It could not succeed, and it left the slot strictly WORSE than it found it. Metal capture
    /// `rmbp-gr8/ttyUSB1.log` line 3806 is the whole story on one line:
    /// `resetdev_cc=1 resetdev_why=ok cfgep_cc=19 cfgep_why=cc-error … epin=1->0 epout=1->0`.
    ///
    ///   * **Reset Device succeeded** (`cc=1`) and, per xHCI 1.2 §4.6.11, transitioned the Slot to
    ///     the **Default** state, disabled every endpoint except the Default Control Endpoint, and
    ///     set the Output Slot Context's USB Device Address field to 0. The driver's own
    ///     before/after field records the endpoint half of that on the same line:
    ///     `epin=1->0 epout=1->0`. Two **Running** endpoints became **Disabled** ones.
    ///   * **Configure Endpoint is legal only against a Slot in Addressed or Configured**
    ///     (xHCI 1.2 §4.6.6); against Default it is required to return **Context State Error**. So
    ///     `cfgep_cc=19` was never a finding about the device — it is the architecturally guaranteed
    ///     answer to an illegal command. All seven `cfgep_cc=19` readings in the capture, across two
    ///     builds, are preceded by exactly this pair.
    ///   * Nothing inside the rung could then recover: Set TR Dequeue Pointer is legal from Stopped
    ///     or Error (§4.6.10) and the endpoints were **Disabled**, so BOT-PHASE fix 6's
    ///     `stage=repoint` returned `cc=19` on both pipes with `epstate=0` (capture lines
    ///     3804/3805), and the escalation retry that followed died on `completion code 12` —
    ///     Endpoint Not Enabled. The rung's only exit was the port cycle it was trying to avoid.
    ///
    /// ## Why no re-address can be inserted, and §14.1 is right
    ///
    /// The obvious patch — an Address Device between Reset Device and Configure Endpoint — does not
    /// exist in a form that works from here:
    ///
    ///   * **Address Device with BSR=1** (Block Set Address Request) issues no `SET_ADDRESS` on the
    ///     wire, but per xHCI 1.2 §4.6.5 it leaves the Slot in the **Default** state with USB Device
    ///     Address 0. That is precisely where Reset Device already left it, so §4.6.6's precondition
    ///     is still unmet and Configure Endpoint still returns `cc=19`. BSR=1 exists for the
    ///     enumeration sequence — set up the contexts, read the device descriptor at address 0,
    ///     Evaluate Context for MPS0, then Address Device with BSR=0 — every step of which presumes
    ///     a device that HAS just been port-reset.
    ///   * **Address Device with BSR=0** does reach Addressed, but only by issuing `SET_ADDRESS` on
    ///     the wire to device address **0**. A USB device answers address 0 only while it is in the
    ///     Default state, which it enters on a port reset and nowhere else (USB 2.0 §9.1.1, §9.4.6).
    ///     This rung deliberately does not port-reset, so the device still holds the address it was
    ///     given at enumeration and cannot answer; the command would burn a full control timeout and
    ///     fail. §14.1's argument against re-addressing here is therefore **correct**, and the
    ///     proposal to insert an Address Device is not.
    ///
    /// Both halves are one fact from two directions: **the xHC's slot state cannot be returned to
    /// Addressed without a port reset**, and the port reset is rung (b)'s, where re-enumeration
    /// rebuilds both sides from scratch. A rung that can only ever fail, and that disables two
    /// working endpoints on its way to failing, is worse than no rung. So Reset Device and its
    /// Configure Endpoint are gone from the ladder.
    ///
    /// ## What the rung does instead
    ///
    /// The disorder rung (a) actually exists to repair is **ring** disorder — driver and controller
    /// disagreeing about position or cycle colour — not slot-state disorder. That is repairable with
    /// commands legal from where the endpoints already are:
    ///
    ///   1. **Stop Endpoint** on each bulk DCI from Running, or **Reset Endpoint** from Halted/Error
    ///      (§4.6.9 / §4.6.8) — `resync_bulk_ep`'s state-aware arm, reused verbatim so its strand
    ///      witness (M1b) and its event drain come with it.
    ///   2. **`TransferRing::reset`** on each ring: every slot zeroed, enqueue index 0, cycle bit
    ///      back to the initial Consumer Cycle State of 1. The endpoints are Stopped by then, so
    ///      `reset`'s documented safety precondition ("the caller must have stopped the endpoint
    ///      first") is finally honoured — the old rung zeroed the rings while they could still be
    ///      Running.
    ///   3. **Set TR Dequeue Pointer** at each ring's base with DCS=1 (§4.6.10, legal from Stopped
    ///      and from Error) — BOT-PHASE fix 6's repoint, intent intact, moved to the only placement
    ///      where it is reachable. It programs exactly what the failed Configure Endpoint would have.
    ///
    /// That is strictly stronger than the ordinary recovery rung, which repoints at the ring's LIVE
    /// enqueue slot with its live colour on a ring still carrying its history. After this rung both
    /// sides are at index 0 with cycle 1 and no stale TRB remains anywhere on either ring —
    /// including no stale Link TRB, which is the one piece of ring state the ordinary resync cannot
    /// clear.
    ///
    /// **The required property holds by construction: this rung cannot leave the slot worse than it
    /// found it.** It issues no command that can disable an endpoint, change the Slot State or clear
    /// the USB Device Address. Every command it issues is legal from the state it just read, and if
    /// any fails the endpoint is left Stopped (or untouched) and the ladder proceeds to the port
    /// cycle exactly as before.
    ///
    /// Returns true only if BOTH pipes came through — a half-rebased slot is never driven further.
    fn rescue_ring_rebase(&mut self, slot_id: u8) -> bool {
        BOT_RESCUE_RESET_DEVICE.fetch_add(1, Ordering::Relaxed);
        let (in_ep, out_ep) = {
            let s = &self.slots[slot_id as usize];
            (s.bulk_in_ep, s.bulk_out_ep)
        };
        if in_ep == 0 || out_ep == 0 || self.slots[slot_id as usize].output_context.is_null() {
            serial_println!(
                ":: BOT: rescue stage=ring-rebase slot={} ok=no why=no-bulk-context ::", slot_id);
            return false;
        }
        let in_dci = ((in_ep & 0x0F) * 2) + 1;
        let out_dci = (out_ep & 0x0F) * 2;
        let (in_s0, out_s0) = (self.ep_state_of(slot_id, in_dci), self.ep_state_of(slot_id, out_dci));

        // 1) Both endpoints out of Running and into Stopped, state-aware, with the authoritative
        //    `when=pre` strand reading taken between each stop and its set-deq (M1b). Whatever this
        //    leaves the dequeue pointing at is overridden by step 3; the call is here for the stop,
        //    the drain and the witness.
        let in_stop = self.resync_bulk_ep(slot_id, in_dci, true, BotError::Timeout);
        let out_stop = self.resync_bulk_ep(slot_id, out_dci, false, BotError::Timeout);

        // 2) Rings back to their birth state — but ONLY for a pipe whose endpoint is provably
        //    stopped. Zeroing a ring the controller may still be walking is the disagreement this
        //    rung exists to end, not a way to end it.
        {
            let s = &mut self.slots[slot_id as usize];
            if in_stop { if let Some(r) = s.bulk_in_ring.as_mut() { r.reset(); } }
            if out_stop { if let Some(r) = s.bulk_out_ring.as_mut() { r.reset(); } }
        }

        // 3) Repoint the controller at each reset ring's base with DCS=1.
        let mut repoint_ok = [false; 2];
        let mut repoint_cc = [0u8; 2];
        for (i, (dci, is_in, stopped)) in
            [(in_dci, true, in_stop), (out_dci, false, out_stop)].into_iter().enumerate()
        {
            if !stopped {
                continue;
            }
            let base_dcs = {
                let s = &self.slots[slot_id as usize];
                let r = if is_in { s.bulk_in_ring.as_ref() } else { s.bulk_out_ring.as_ref() };
                match r { Some(r) => r.get_ptr() | 1, None => 0 }
            };
            if base_dcs == 0 {
                continue;
            }
            let ctx = ((dci as u32) << 16) | ((slot_id as u32) << 24);
            let (sd_ok, sd_cc, sd_why) =
                self.recover_cmd(Trb { parameter: base_dcs, status: 0, control: (16 << 10) | ctx });
            repoint_ok[i] = sd_ok;
            repoint_cc[i] = sd_cc;
            serial_println!(
                ":: BOT: rescue stage=repoint slot={} dci={} dir={} ok={} cc={} why={} want={:#x} ctxdeq={:#x} epstate={} — ring rebased to base|DCS=1 (Set TR Dequeue Pointer, xHCI 1.2 §4.6.10, legal from Stopped/Error) ::",
                slot_id, dci, if is_in { "in" } else { "out" },
                if sd_ok { "yes" } else { "no" }, sd_cc, sd_why, base_dcs,
                self.ep_ctx_deq(slot_id, dci), self.ep_state_of(slot_id, dci));
        }
        while self.drain_event_ring_once() {}

        let ok = in_stop && out_stop && repoint_ok[0] && repoint_ok[1];
        // The rung's one summary line. `retired=` names what ONSET-2 removed and why, so a reader
        // comparing this capture against any pre-ONSET-2 one — where the same rung printed
        // `stage=reset-device … resetdev_cc=1 cfgep_cc=19` — can see at a glance which build it is
        // and that the `cc=19` question is closed rather than merely unrecorded.
        serial_println!(
            ":: BOT: rescue stage=ring-rebase slot={} ok={} retired=reset-device+configure-endpoint why_retired=xhci-1.2-4.6.11-leaves-slot-Default-and-4.6.6-then-guarantees-cc19 stop_in={} stop_out={} repoint_in_cc={} repoint_out_cc={} indci={} outdci={} epin={}->{} epout={}->{} n={} ::",
            slot_id, if ok { "yes" } else { "no" },
            if in_stop { "ok" } else { "fail" }, if out_stop { "ok" } else { "fail" },
            repoint_cc[0], repoint_cc[1], in_dci, out_dci,
            in_s0, self.ep_state_of(slot_id, in_dci),
            out_s0, self.ep_state_of(slot_id, out_dci),
            BOT_RESCUE_RESET_DEVICE.load(Ordering::Relaxed));
        ok
    }

    /// BOT-RESCUE escalation (b): power-cycle the device's ROOT PORT (PORTSC Port Power, bit 9) and
    /// let the enumeration machinery re-attach whatever comes back.
    ///
    /// This is the only rung that can revive a device whose own logic — not just its BOT state
    /// machine — has hung: removing VBUS is the one action a host can take that a wedged device
    /// firmware cannot ignore. Dwell times are USB spec-scale (`BOT_RESCUE_PORT_OFF_MS`,
    /// `BOT_RESCUE_PORT_ON_MS`), not timeout-derived.
    ///
    /// Re-enumeration is DELEGATED, not open-coded: removing and restoring power raises Connect
    /// Status Change on the port, and the settle below drains the event ring, so the driver's own
    /// (tested, single-threaded, one-port-at-a-time) `handle_port_status` path sees the disconnect
    /// and the reconnect and re-enumerates onto a FRESH slot — retracting this slot's block-registry
    /// entry through `dispose_disconnected_slots` on the way. Open-coding a synchronous re-address
    /// here would race that path for the same port with no way to win.
    ///
    /// PORTSC hygiene: every write goes through a read-modify-write that masks off PED (bit 1,
    /// write-1-to-DISABLE), PR (bit 4, write-1-to-RESET) and all RW1C change bits — the same
    /// discipline `clear_port_change` documents. Nothing here weakens a protection: PP is the
    /// port's own power switch, and the port is returned to powered before the function exits on
    /// every path.
    ///
    /// Returns true if the port reports a device connected (CCS, bit 0) after the cycle.
    fn rescue_port_cycle(&mut self, slot_id: u8) -> bool {
        BOT_RESCUE_PORT_CYCLE.fetch_add(1, Ordering::Relaxed);
        let port = self.slots[slot_id as usize].port_id;
        let downstream = self.slots[slot_id as usize].is_downstream;
        if port == 0 || port > self.max_ports || downstream {
            // A hub-downstream device's power is the HUB's to switch, via a class request on the
            // hub's slot — a different pipe, and one this rung has no business reaching for while
            // the device it would have to ask through may itself be the sick one.
            serial_println!(
                ":: BOT: rescue stage=port-cycle slot={} port={} ok=no why={} ::",
                slot_id, port, if downstream { "downstream-port-not-root" } else { "no-root-port" });
            return false;
        }
        // Preserve everything except the write-1-to-act bits; then drop PP.
        let keep = |v: u32| v & !(PORT_CHANGE_BITS | (1 << 1) | (1 << 4));
        let before = self.read_portsc(port);
        self.write_portsc(port, keep(before) & !(1 << 9));
        self.settle_ms(BOT_RESCUE_PORT_OFF_MS);
        let off = self.read_portsc(port);
        // Restore power.
        let cur = self.read_portsc(port);
        self.write_portsc(port, keep(cur) | (1 << 9));
        self.settle_ms(BOT_RESCUE_PORT_ON_MS);
        let after = self.read_portsc(port);
        // The connect/disconnect edges the cycle raised have been dispatched by the settles' drain;
        // acknowledge any change bits still latched so the port is left clean.
        self.clear_port_change(port, PORT_CHANGE_BITS);
        let ccs = after & 1;
        serial_println!(
            ":: BOT: rescue stage=port-cycle slot={} port={} ok={} off_ms={} on_ms={} portsc={:#x}->{:#x}->{:#x} pp_off={} ccs={} ped={} pls={} n={} ::",
            slot_id, port, if ccs != 0 { "yes" } else { "no" },
            BOT_RESCUE_PORT_OFF_MS, BOT_RESCUE_PORT_ON_MS,
            before, off, after, (off >> 9) & 1, ccs, (after >> 1) & 1, (after >> 5) & 0xF,
            BOT_RESCUE_PORT_CYCLE.load(Ordering::Relaxed));
        ccs != 0
    }

    /// BOT-RESCUE M3 witnesses 1, 2 and 5 — the three lines that make a BOT stage timeout
    /// self-diagnosing instead of merely reported. Pure reads (endpoint contexts, ring fields, DRAM
    /// behind an invalidate); no command, no wait, no state change.
    ///
    /// **Reading key.**
    ///
    /// * `TIMEOUT-PIPES` (witness 1) — for BOTH bulk DCIs: the endpoint's EP State, the
    ///   controller's own TR Dequeue Pointer with its Dequeue Cycle State, and OUR enqueue index and
    ///   cycle bit. This is THE discriminator the 2026-07-29 capture lacked.
    ///
    ///   GUARD-STATE — read `epstate` BEFORE `ctxdeq`, always. The TR Dequeue Pointer field is only
    ///   written back by the controller on a Running -> Stopped/Halted transition (xHCI 1.0 §4.8.3,
    ///   §6.2.3). Under `epstate=1` (Running) it is the BIRTH value from Configure Endpoint or the
    ///   last Set TR Dequeue and carries no information about progress — the line tags it
    ///   `(stale: EP running)` for exactly that reason, and no verdict may be drawn from it. Only a
    ///   Stopped(3)/Halted(2) read is a live position, and only then does the following apply: if
    ///   `ctxdeq` has NOT reached the TRB we were waiting on, the controller never fetched the work —
    ///   a host/endpoint fault; if `ctxdeq` HAS passed it and `dcs` agrees with our `cycle`, the
    ///   controller fetched everything and issued it, the DEVICE is silent, and no amount of
    ///   host-side ring surgery can help. The audit reached that second conclusion by hand, off
    ///   `set-deq` being a provable no-op; this prints it. See `bot_deqprobe` for the per-boot proof
    ///   of the stale-field behaviour on the running platform.
    /// * `foreign=` on the same line (witness 4) — Transfer Events for OTHER slots during this
    ///   wait. Non-zero proves the event ring and interrupter stayed alive for other traffic, so a
    ///   missing completion is this slot's problem, not a global wedge. Carried here rather than on
    ///   the `pump budget=` line so that line stays byte-comparable with every pre-arc capture.
    /// * `TIMEOUT-TRB` (witness 2) — the awaited TRB's four raw dwords as they read back FROM DRAM,
    ///   plus its stored cycle bit against the ring's live one. A TRB whose cycle bit does not match
    ///   the colour the controller expects is one the controller is entitled to ignore forever;
    ///   dwords that do not match what we believe we wrote mean the write never landed (a coherency
    ///   or aliasing fault), which is a different arc entirely.
    /// * `TIMEOUT-CSW` (witness 5) — the CSW buffer's signature, tag, residue and status. The CSW is
    ///   DMA-written, so on a genuine timeout it is still the pre-transfer zero fill. A VALID
    ///   signature (0x53425355) here means the status actually landed and only the completion EVENT
    ///   went missing — a completely different fault class, and the direct test of §12.3's lost-CSW
    ///   hypothesis.
    fn bot_timeout_witness(&self, p: &BotPending, foreign: u64, evts: u64, db_in_d: u64, db_out_d: u64) {
        let slot_id = p.slot_id;
        // Ring state for one direction: (enqueue index, cycle bit, ring size), or the 0xFF sentinel
        // when the slot has no such ring.
        let ring_of = |is_in: bool| -> (usize, u32, usize) {
            let s = &self.slots[slot_id as usize];
            let r = if is_in { s.bulk_in_ring.as_ref() } else { s.bulk_out_ring.as_ref() };
            match r {
                Some(r) => (r.enqueue_index(), if r.cycle_bit() { 1 } else { 0 }, r.num_trbs()),
                None => (0, 0xFF, 0),
            }
        };
        let (in_enq, in_cyc, in_n) = ring_of(true);
        let (out_enq, out_cyc, out_n) = ring_of(false);
        let in_deq = self.ep_ctx_deq(slot_id, p.in_dci);
        let out_deq = self.ep_ctx_deq(slot_id, p.out_dci);
        let in_state = self.ep_state_of(slot_id, p.in_dci);
        let out_state = self.ep_state_of(slot_id, p.out_dci);
        // GUARD-STATE: the TR Dequeue Pointer field is only written back on Running -> Stopped/Halted
        // (xHCI 1.0 §4.8.3, §6.2.3). Under a Running endpoint it is the birth value from Configure
        // Endpoint / the last Set TR Dequeue, not a position — tag it so this line can never again be
        // read as "the controller is behind our enqueue" when it says nothing of the kind.
        let stale = |st: u8| if st == 1 { " (stale: EP running)" } else { "" };
        // ONSET-2 (M2 witnesses 2 and 6) append four fields to this line; `foreign=` is KEPT,
        // unchanged and in place, so every capture ever taken stays diffable against this one.
        //
        //   `evts=`   — every event-ring TRB consumed during THIS wait, of any type. `foreign=` is
        //               structurally pinned at 0 on this platform (the pump is a synchronous spin
        //               that submits no other traffic, so no other slot can have a TRB outstanding
        //               to complete) and therefore supports no verdict whatever it reads. `evts`
        //               can be non-zero, which is what makes a zero reading of it mean something:
        //               `evts=0` says nothing at all came off the event ring across the whole
        //               budget; `evts>0` says the ring was being consumed throughout and only OUR
        //               completion never arrived.
        //   `db_in_d=` / `db_out_d=` — doorbells the BOT path wrote on each pipe DURING this wait,
        //               against a baseline snapshotted at pump entry. Before this arc there was no
        //               line in any capture saying a doorbell had been written at all, so "written
        //               and did not take" and "never written" were indistinguishable — and that is
        //               the ranked hypothesis's own discriminator. A delta of 0 on the pipe the
        //               stage is waiting on means no doorbell was written for it.
        //   `db_in_idx=` / `db_out_idx=` — the ring enqueue index at the last doorbell on each pipe,
        //               to be read against `trb_idx=` on the TIMEOUT-SHAPE line.
        serial_println!(
            ":: BOT: timeout pipes slot={} in_dci={} in_epstate={} in_ctxdeq={:#x}{} in_dcs={} in_enq={} in_cycle={} in_ntrb={} out_dci={} out_epstate={} out_ctxdeq={:#x}{} out_dcs={} out_enq={} out_cycle={} out_ntrb={} foreign={} evts={} db_in_d={} db_out_d={} db_in={} db_out={} db_in_idx={} db_out_idx={} result=TIMEOUT-PIPES ::",
            slot_id,
            p.in_dci, in_state, in_deq & !0xFu64, stale(in_state), in_deq & 1, in_enq, in_cyc, in_n,
            p.out_dci, out_state, out_deq & !0xFu64, stale(out_state), out_deq & 1, out_enq, out_cyc, out_n,
            foreign, evts, db_in_d, db_out_d,
            BOT_DB_IN.load(Ordering::Relaxed), BOT_DB_OUT.load(Ordering::Relaxed),
            BOT_DB_IN_IDX.load(Ordering::Relaxed), BOT_DB_OUT_IDX.load(Ordering::Relaxed));

        // Witness 2: the awaited TRB, read back from DRAM. Rings are identity-mapped, so the
        // physical address the endpoint context and the event dispatch use is also the CPU's.
        let (pipe, ring_cycle) = {
            let s = &self.slots[slot_id as usize];
            if s.bulk_in_ring.as_ref().is_some_and(|r| r.contains(p.wait_trb_phys)) {
                ("in", in_cyc)
            } else if s.bulk_out_ring.as_ref().is_some_and(|r| r.contains(p.wait_trb_phys)) {
                ("out", out_cyc)
            } else {
                ("unknown", 0xFF)
            }
        };
        if p.wait_trb_phys != 0 {
            let (dw0, dw1, dw2, dw3) = unsafe {
                // The controller may have written behind our cache (Event Data / partial retire);
                // invalidate so this reads DRAM, not a stale line. No-op on x86.
                dma_coherency::inval(p.wait_trb_phys as usize, 16);
                let w = p.wait_trb_phys as *const u32;
                (core::ptr::read_volatile(w), core::ptr::read_volatile(w.add(1)),
                 core::ptr::read_volatile(w.add(2)), core::ptr::read_volatile(w.add(3)))
            };
            serial_println!(
                ":: BOT: timeout trb wait={:#x} pipe={} dw0={:#010x} dw1={:#010x} dw2={:#010x} dw3={:#010x} trb_cycle={} ring_cycle={} trb_type={} result=TIMEOUT-TRB ::",
                p.wait_trb_phys, pipe, dw0, dw1, dw2, dw3, dw3 & 1, ring_cycle, (dw3 >> 10) & 0x3F);
        }

        // ONSET-2 (M2 witness 4): LINK TRB FORENSICS, on a wrapped stage only.
        //
        // All three genuine onsets in `rmbp-gr8` are `stage=data dir=out len=512 trb_idx=0
        // wrapped=true` — an OUT data stage landing at ring index 0, i.e. immediately behind a
        // freshly written Link TRB. `TransferRing::push` writes that Link TRB LAZILY, when the
        // enqueue index reaches `ntrb-1`, so for the whole of each lap the last slot holds a stale
        // TRB whose cycle is the colour the controller is no longer expecting. Whether the
        // controller stops there, and what colour and target it actually found, has only ever been
        // argued from the source. These two lines put the bytes on the record: the Link slot
        // (`ntrb-1`) and the TRB immediately ahead of it (`ntrb-2`).
        //
        // HEALTHY-BUT-IDLE READING: not printed at all — the block is gated on the timing-out stage
        // having wrapped. When printed, `type=6` with `tc=1` and `target=` equal to the ring base is
        // a correctly formed Link TRB; a `cycle` disagreeing with `ring_cycle` on the line above is
        // the stale-colour condition the hypothesis names.
        if BOT_LAST_WRAP.load(Ordering::Relaxed) {
            let s = &self.slots[slot_id as usize];
            let ring = match pipe {
                "in" => s.bulk_in_ring.as_ref(),
                "out" => s.bulk_out_ring.as_ref(),
                _ => None,
            };
            if let Some(r) = ring {
                let n = r.num_trbs();
                for (what, idx) in [("link", n.wrapping_sub(1)), ("prev", n.wrapping_sub(2))] {
                    if let Some((dw0, dw1, dw2, dw3)) = r.trb_raw(idx) {
                        serial_println!(
                            ":: BOT: timeout link pipe={} slot={} what={} idx={} ntrb={} dw0={:#010x} dw1={:#010x} dw2={:#010x} dw3={:#010x} cycle={} tc={} type={} target={:#x} result=TIMEOUT-LINK ::",
                            pipe, slot_id, what, idx, n, dw0, dw1, dw2, dw3,
                            dw3 & 1, (dw3 >> 1) & 1, (dw3 >> 10) & 0x3F,
                            ((dw1 as u64) << 32) | (dw0 as u64));
                    }
                }
            }
        }

        // Witness 5: the CSW buffer as the controller left it.
        let (sig, tag, residue, status) = match self.slots[slot_id as usize].csw_buffer {
            Some(b) => unsafe {
                dma_coherency::inval(b as usize, 13);
                let c = core::slice::from_raw_parts(b as *const u8, 13);
                let rd = |o: usize| (c[o] as u32) | ((c[o + 1] as u32) << 8)
                    | ((c[o + 2] as u32) << 16) | ((c[o + 3] as u32) << 24);
                (rd(0), rd(4), rd(8), c[12])
            },
            None => (0, 0, 0, 0xFF),
        };
        let valid = sig == 0x53425355;
        serial_println!(
            ":: BOT: timeout csw sig={:#x} tag={:#x} residue={} status={} valid={} — {} result=TIMEOUT-CSW ::",
            sig, tag, residue, status, if valid { "yes" } else { "no" },
            if valid {
                "VALID signature at a timeout: the status phase LANDED and only its completion event went missing (see usb_xhci.md 12.3)"
            } else {
                "no signature: the status phase never landed, so the CSW never reached DRAM — a transport wedge, not a lost event"
            });

        // ONSET-2 (M2 witness 1): the port register census, for the failing device's port AND every
        // other connected port, read against the `why=bringup` baseline taken on this same boot.
        // Last, because it is the widest reading and the one a reader wants after the pipe and TRB
        // verdicts have narrowed the question.
        self.port_link_witness("timeout");
    }

    /// BOT-RESCUE escalation (c): SURRENDER. Mark the disk FAILED, retract it from the block
    /// registry, and stop issuing transfers to the slot.
    ///
    /// Retraction reuses the GR7 unplug machinery verbatim (`block::unpublish_usb_geometry`), which
    /// is the point: the FAT layer, the shell's `df` and the installer's per-frame disk list already
    /// handle a disk that DISAPPEARS — every block entry point re-reads the registry on each call
    /// and fails honestly with `NotReady`, and the installer's captured `BlockDeviceId` refuses to
    /// resolve rather than retargeting. A disk that has stopped answering is, to every consumer
    /// above the driver, indistinguishable from one that was pulled; saying so in the one vocabulary
    /// they already understand is better than inventing a second failure mode for them to learn.
    /// It matches by slot id, so a microSD published with `slot_id: 0` can never be retracted here.
    ///
    /// `bot_surrendered_slot` then refuses every later transfer to this slot up front. That is the
    /// arc's actual guarantee: a sick disk can never again spin the system at ~6 s per attempt
    /// forever. It is cleared when the slot is disposed or re-enumerated, so a physical replug is a
    /// clean slate and needs no operator action beyond the replug.
    fn bot_surrender(&mut self, slot_id: u8, cause: BotError) {
        if self.bot_surrendered_slot == slot_id {
            return; // already surrendered; one verdict line per disk, not one per caller
        }
        BOT_RESCUE_SURRENDER.fetch_add(1, Ordering::Relaxed);
        self.bot_surrendered_slot = slot_id;
        let retracted = crate::drivers::block::unpublish_usb_geometry(slot_id);
        if self.storage_slot == slot_id {
            self.storage_slot = 0;
            self.storage_pending_bringup = false;
            self.storage_note = "storage device FAILED (BOT rescue surrendered)";
        }
        // THE verdict line. One per surrendered disk, naming the fault class, what the ladder spent,
        // and whether the block layer heard about it.
        serial_println!(
            ":: BOT: SURRENDER slot={} cause={:?} streak={} recoveries={} recover_ok={} retry_ok={} retry_fail={} resetdev={} portcycle={} retracted={} — disk marked FAILED and retracted; NO further transfers to this slot until it is replugged ::",
            slot_id, cause, self.bot_fail_streak,
            BOT_RECOVER_COUNT.load(Ordering::Relaxed), BOT_RECOVER_OK.load(Ordering::Relaxed),
            BOT_RETRY_OK.load(Ordering::Relaxed), BOT_RETRY_FAIL.load(Ordering::Relaxed),
            BOT_RESCUE_RESET_DEVICE.load(Ordering::Relaxed),
            BOT_RESCUE_PORT_CYCLE.load(Ordering::Relaxed),
            if retracted { "yes" } else { "no-registry-entry" });
    }

    /// BOT-RESCUE: clear a slot's escalation state — called when a transaction COMPLETES (the
    /// device is answering, so whatever streak it was on is over) and when a slot is disposed or
    /// re-enumerated (a fresh device inherits nothing).
    fn bot_rescue_clear(&mut self, slot_id: u8) {
        self.bot_fail_streak = 0;
        self.bot_rescue_stage = 0;
        if self.bot_surrendered_slot == slot_id {
            self.bot_surrendered_slot = 0;
        }
    }

    /// BOT-RESCUE: ONE retry after an escalation rung, on the SHORTER budget
    /// (`BOT_BUDGET_SCALE_ESCALATION` — see the constant for why the first attempt keeps ~6 s and
    /// this does not). `Some(_)` = the rung settled the question (the transfer completed, whatever
    /// its CSW says); `None` = keep escalating.
    fn bot_rescue_retry(&mut self, slot_id: u8, cdb: &[u8], data_phys: u64, data_len: u32,
        dir: Direction, rung: &'static str) -> Option<Result<BotResult, BotError>>
    {
        self.bot_budget_scale = BOT_BUDGET_SCALE_ESCALATION;
        let r = self.bot_transfer_once(slot_id, cdb, data_phys, data_len, dir);
        self.bot_budget_scale = BOT_BUDGET_SCALE_FIRST;
        self.bot_failed = None;
        match r {
            Ok(res) => {
                serial_println!(
                    ":: BOT: rescue retry rung={} result=pass status={:?} residue={} budget_scale={} ::",
                    rung, res.status, res.residue, BOT_BUDGET_SCALE_ESCALATION);
                self.bot_rescue_clear(slot_id);
                Some(Ok(res))
            }
            Err(e) => {
                serial_println!(
                    ":: BOT: rescue retry rung={} result=fail err={:?} budget_scale={} ::",
                    rung, e, BOT_BUDGET_SCALE_ESCALATION);
                None
            }
        }
    }

    /// BOT-RESCUE: the ladder above the ladder. Entered when the existing class-level Reset Recovery
    /// plus its single retry did NOT rescue the transaction.
    ///
    /// Counts the consecutive failure, pays an exponential back-off, and — once
    /// `BOT_RESCUE_N_CONSEC` consecutive cycles have failed — walks the rungs in order, each at most
    /// once per streak: (a) Reset Device + endpoint re-setup, (b) root-port power cycle, then
    /// (c) surrender. Terminates by construction: `bot_rescue_stage` only ever increases within a
    /// streak, and its top is surrender, which refuses every later transfer to the slot.
    fn bot_rescue_escalate(&mut self, slot_id: u8, cdb: &[u8], data_phys: u64, data_len: u32,
        dir: Direction, cause: BotError) -> Result<BotResult, BotError>
    {
        self.bot_fail_streak = self.bot_fail_streak.saturating_add(1);
        let streak = self.bot_fail_streak;
        // Exponential back-off: a device wedged mid-internal-stall is made worse by being hammered.
        let backoff = (BOT_RESCUE_BACKOFF_MS << (streak - 1).min(3)).min(BOT_RESCUE_BACKOFF_MAX_MS);
        serial_println!(
            ":: BOT: rescue slot={} cause={:?} streak={} n_consec={} stage={} backoff_ms={} ::",
            slot_id, cause, streak, BOT_RESCUE_N_CONSEC, self.bot_rescue_stage, backoff);
        self.settle_ms(backoff);
        if streak < BOT_RESCUE_N_CONSEC {
            // Not yet enough evidence that the fault is permanent: report the failure as the
            // pre-arc code did, and let the caller decide whether to come back.
            return Err(cause);
        }

        // (a) Ring rebase — ONSET-2 (M1a) replaced Reset Device + Configure Endpoint here; see
        //     `rescue_ring_rebase` for the spec argument and the capture that convicted the old rung.
        if self.bot_rescue_stage == 0 {
            self.bot_rescue_stage = 1;
            if self.rescue_ring_rebase(slot_id) {
                if let Some(r) = self.bot_rescue_retry(slot_id, cdb, data_phys, data_len, dir, "ring-rebase") {
                    return r;
                }
            }
            self.settle_ms(backoff);
        }

        // (b) Port power-cycle + (delegated) re-enumeration.
        if self.bot_rescue_stage == 1 {
            self.bot_rescue_stage = 2;
            if self.rescue_port_cycle(slot_id) {
                if let Some(r) = self.bot_rescue_retry(slot_id, cdb, data_phys, data_len, dir, "port-cycle") {
                    return r;
                }
            }
        }

        // (c) Surrender.
        self.bot_surrender(slot_id, cause);
        Err(cause)
    }

    /// Arm the BOT pending state for one stage (waiting on `wait_trb_phys`'s completion
    /// event), pump the event ring until it arrives, and return its completion code. The
    /// caller queues the stage's TRB(s) and rings the doorbell(s) before calling this.
    /// BOT-PHASE: returns `(completion_code, residue)` — the TRB Transfer Length the event reported
    /// as NOT transferred. The residue was always in the event and always discarded; fix 3 carries
    /// it out so a data stage can be checked against its own `dCBWDataTransferLength`.
    fn run_bot_stage(&mut self, slot_id: u8, in_dci: u8, out_dci: u8, wait_trb_phys: u64)
        -> Result<(u8, u32), BotError>
    {
        let generation = BOT_STAGE_GEN.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        self.bot_pending = Some(BotPending {
            slot_id, in_dci, out_dci, wait_trb_phys,
            done: false, completion_code: 0,
            generation, residue: 0, residue_seen: false,
        });
        let pump = self.pump_until_bot_done();
        let pending = self.bot_pending.take();
        // BOT-RESCUE M3 witness 6: park the taken record where the caller can hand it to recovery.
        // The `pump?` below propagates the error and — before this arc — dropped `pending` on the
        // floor with it, which is why `recover evidence` could only ever print `pipe=none`. Stored
        // on the FAILURE path only, and taken (not cloned) by `bot_transfer`, so a stale record can
        // never be attributed to a later transaction.
        if pump.is_err() {
            self.bot_failed = pending;
        }
        pump?;
        let p = pending.ok_or(BotError::Timeout)?;
        // BPACE: the first BOT stage this boot ever completed. One-shot — this function runs
        // thousands of times per boot, and the ledger wants the EDGE ("the mass-storage protocol
        // started moving"), not a per-transaction trace. Placed after `pump?` so a timed-out first
        // stage does not latch it: `bot-first` present means the wire actually carried a completion.
        {
            static BOT_FIRST: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            crate::bootpace::record_once(&BOT_FIRST, "bot-first");
        }
        Ok((p.completion_code, p.residue))
    }

    /// Pump the event ring until the in-flight BOT transaction reports done, or a WALL-CLOCK
    /// budget is exhausted. Unrelated events (HID input, command completions) are dispatched
    /// normally during the wait.
    ///
    /// The budget is a `now_cycles`/`hw_wait_budget` deadline (the idiom the enumeration FSM
    /// already uses), NOT a raw iteration count — so the pump is correct regardless of how long
    /// each `crate::hlt()` yields. On x86 / Pi / aarch64-virt, and on the pre-drop tegra core,
    /// `hlt()` waits for an interrupt (HLT / WFI with a live timer), so each empty pass costs a
    /// tick; but on the tegra post-drop core the timer is disabled (JD2/JD3), `hlt()` busy-spins,
    /// and a fixed iteration budget would then expire in microseconds — long before a real DMA
    /// completion lands. A wall-clock deadline gives the transfer real time to complete in both
    /// regimes. (`now_cycles` reads a free-running counter — rdtsc / CNTVCT — that keeps advancing
    /// even with the timer interrupt off, exactly as the Pi's polled EMMC2 driver relies on.)
    fn pump_until_bot_done(&mut self) -> Result<(), BotError> {
        let start = crate::arch::now_cycles();
        // A BOT data stage can outlast a bare register handshake, so allow a generous multiple of
        // the base handshake budget; only a FAILING transfer (dead DMA / wedged endpoint) ever pays
        // the full wait — the happy path returns the instant the completion event drains.
        // BOT-RESCUE: the multiple is `bot_budget_scale`, which is `BOT_BUDGET_SCALE_FIRST` (3 —
        // the pre-arc constant, so the first attempt's ~6 s is unchanged) at all times except
        // inside an escalation retry, where the caller briefly drops it to
        // `BOT_BUDGET_SCALE_ESCALATION` and restores it immediately after. A healthy device never
        // reaches an escalation retry, so it never sees anything but the historical budget.
        let budget = crate::arch::hw_wait_budget().saturating_mul(self.bot_budget_scale);
        BOT_PUMP_BUDGET.store(budget, Ordering::Relaxed);
        // IVY: snapshot the waiting slot's topology up front, so the witness (and any timeout line)
        // says whether this transfer rode a root port or a hub route — the one fact the 2026-07-17
        // metal delete failure could not be read off the log.
        let (route, depth, slot) = match &self.bot_pending {
            Some(p) => {
                let s = &self.slots[p.slot_id as usize];
                (s.route_string, s.route_depth, p.slot_id)
            }
            None => (0, 0, 0),
        };
        // BOT-RESCUE M3 witness 4: baseline for the foreign-event delta. Snapshotted HERE, at the
        // start of THIS wait, because a counter read against its own boot-long total is not a rate
        // — the instrument-baseline law. The timeout prints `now - this`.
        let foreign_at_entry = BOT_FOREIGN_EVENTS.load(Ordering::Relaxed);
        // ONSET-2 (M2 witnesses 2 and 6): the two baselines the instrument-baseline law requires for
        // the doorbell and event-ring witnesses. Same discipline as `foreign_at_entry` above — the
        // timeout prints `now - this`, never the boot-long total on its own.
        let (db_in_at_entry, db_out_at_entry) =
            (BOT_DB_IN.load(Ordering::Relaxed), BOT_DB_OUT.load(Ordering::Relaxed));
        // Every event-ring TRB this wait consumes, of any type. Local to the wait by construction:
        // it cannot be read against its own pre-run total because it has none.
        let mut evts: u64 = 0;
        // BOOTPACE M3: hoisted out of the loop — the rate does not change mid-wait, and this keeps
        // the per-pass cost of the spin at zero atomic loads.
        let spin_window = Self::spin_window();
        loop {
            match &self.bot_pending {
                Some(p) if p.done => {
                    Self::note_bot_pump(start, budget, route, depth, slot);
                    return Ok(());
                }
                None => {
                    // Nothing was pending: this pump entry did not wait on a transfer at all.
                    // Counted separately (`nowait=`) so it cannot inflate `n=` against route 0.
                    BOT_PUMP_NOWAIT.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
                _ => {}
            }
            if self.drain_event_ring_once() {
                evts = evts.saturating_add(1);
                continue; // processed an event; drain any more immediately
            }
            // BOOTPACE M3 — SPIN, THEN HALT. Busy-poll the ring for ~200 µs before sleeping. A
            // healthy controller answers in microseconds, so nearly every awaited stage now returns
            // from here instead of paying a full 1 kHz APIC tick to `hlt()`. Counted into `evts`
            // exactly like the drain above, so the timeout witness stays honest.
            if self.spin_for_event(spin_window) {
                evts = evts.saturating_add(1);
                continue;
            }
            // Yield to QEMU's main loop so it can run the xHC bottom-half / async block-I/O
            // completion and DMA the event into the ring; a pure spin never exits TCG. On the
            // timerless tegra post-drop core this falls back to a busy spin (arch::hlt), which the
            // wall-clock deadline below still bounds. KEPT, and not replaced by the spin above: the
            // spin cannot be the only wait, or TCG would never make progress — past its window this
            // path is byte-identical to what it was.
            crate::hlt();
            let elapsed = crate::arch::now_cycles().wrapping_sub(start);
            if elapsed >= budget {
                BOT_PUMP_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
                unsafe {
                    let ir0 = XHCI_IR0_BASE.load(Ordering::Acquire);
                    let op = XHCI_OP_BASE.load(Ordering::Acquire);
                    let iman = if ir0 != 0 { core::ptr::read_volatile(ir0 as *const u32) } else { 0 };
                    let usbsts = if op != 0 { core::ptr::read_volatile((op + 0x04) as *const u32) } else { 0 };
                    serial_println!(
                        "xHCI: BOT pump TIMEOUT after {} cycles (IRQ_COUNT={} IMAN={:#x} USBSTS={:#x})",
                        elapsed, XHCI_IRQ_COUNT.load(Ordering::Relaxed), iman, usbsts);
                }
                // IVY: the measurable half of the timeout — how the exhausted budget compares to the
                // largest wait this boot ever needed. `used == budget` with a `peak` far below it says
                // the completion event was LOST (a wedged endpoint), not that the budget was tight;
                // a `peak` sitting just under `budget` says the opposite. Metal reads the verdict off
                // this one line instead of guessing.
                let n = BOT_PUMP_COUNT.load(Ordering::Relaxed);
                let sum = BOT_PUMP_CYCLES.load(Ordering::Relaxed);
                serial_println!(
                    ":: BOT: pump budget={} used={} peak={} sum={} mean={} route={:#x} depth={} slot={} n={} timeouts={} result=TIMEOUT ::",
                    budget, elapsed, BOT_PUMP_PEAK.load(Ordering::Relaxed), sum,
                    if n != 0 { sum / n } else { 0 }, route, depth, slot,
                    n, BOT_PUMP_TIMEOUTS.load(Ordering::Relaxed));
                // MULTIBLK: the SHAPE of the transaction that did not complete, on its own line so
                // the line above stays byte-comparable with every capture taken before this arc.
                // M2 is still unexplained; this is the evidence that can bound it. Read it as:
                //   stage=data + a large len  -> the wedge prefers big TDs (a controller/stick
                //     boundary or burst problem, and the multi-block win would be trading M1 for M2);
                //   stage=data + len=512      -> size is not the discriminator, and the amplification
                //     cut is pure profit;
                //   stage=csw                 -> the DATA phase landed and only the 13-byte status
                //     event went missing, which points at the event ring, not the transfer;
                //   wrapped=true on a timeout, against `wrapped=` in the SUMMARY line's population
                //     rate, is the direct ring-wrap correlation test.
                // ONSET-2 (M2 witness 7): `tag=`, `cdb0=` and `lba=` APPENDED (every pre-existing
                // field keeps its name and position, so this line stays diffable against every
                // capture taken before this arc). The timing-out transaction now names itself:
                // §15.2's code -> capture -> medium join should be readable off the log rather than
                // reconstructed from a wrecked filesystem. `lba=0` on a non-READ(10)/WRITE(10)
                // opcode means "this CDB has no LBA there", not "LBA zero".
                serial_println!(
                    ":: BOT: pump shape stage={} dir={} len={} trb_idx={} wrapped={} single={} multi={} maxlen={} wrapped_tx={} tag={:#010x} cdb0={:#04x} lba={} result=TIMEOUT-SHAPE ::",
                    match BOT_LAST_STAGE.load(Ordering::Relaxed) { 1 => "data", 2 => "csw", 3 => "cbw", _ => "none" },
                    match BOT_LAST_DIR.load(Ordering::Relaxed) { 1 => "in", 2 => "out", _ => "none" },
                    BOT_LAST_LEN.load(Ordering::Relaxed),
                    BOT_LAST_TRB_IDX.load(Ordering::Relaxed),
                    BOT_LAST_WRAP.load(Ordering::Relaxed),
                    BOT_TX_SINGLE.load(Ordering::Relaxed),
                    BOT_TX_MULTI.load(Ordering::Relaxed),
                    BOT_TX_MAXLEN.load(Ordering::Relaxed),
                    BOT_TX_WRAPPED.load(Ordering::Relaxed),
                    BOT_LAST_TAG.load(Ordering::Relaxed),
                    BOT_LAST_CDB0.load(Ordering::Relaxed),
                    BOT_LAST_LBA.load(Ordering::Relaxed));
                // BOT-RESCUE M3 witnesses 1, 2, 4 and 5 — three further lines, so the two lines
                // above stay byte-comparable with every capture taken before this arc. See
                // `bot_timeout_witness` for the reading key.
                if let Some(p) = self.bot_pending {
                    let foreign = BOT_FOREIGN_EVENTS.load(Ordering::Relaxed)
                        .wrapping_sub(foreign_at_entry);
                    let db_in_d = BOT_DB_IN.load(Ordering::Relaxed).wrapping_sub(db_in_at_entry);
                    let db_out_d = BOT_DB_OUT.load(Ordering::Relaxed).wrapping_sub(db_out_at_entry);
                    self.bot_timeout_witness(&p, foreign, evts, db_in_d, db_out_d);
                }
                return Err(BotError::Timeout);
            }
        }
    }

    /// IVY: fold ONE completed BOT pump wait into the headroom counters, and print the
    /// `:: BOT: … result=OK ::` witness when it sets a new high-water mark that at least DOUBLES the
    /// previous one. Doubling (not every peak) keeps the line count logarithmic in the budget — a
    /// handful of lines across a whole boot even though the storage chain runs thousands of BOT
    /// transactions — so the default-quiet boot stays quiet while the LAST such line still reports
    /// the true worst-case wait the run ever needed. Read against `budget`, that is the headroom the
    /// next metal sitting measures instead of guesses at.
    fn note_bot_pump(start: u64, budget: u64, route: u32, depth: u8, slot: u8) {
        let used = crate::arch::now_cycles().wrapping_sub(start);
        BOT_PUMP_COUNT.fetch_add(1, Ordering::Relaxed);
        // ONSET-2 (M2 witness 7): fold this wait into the log2-millisecond histogram.
        //
        // The capture carries only `sum`, `peak` and `n`, so it CANNOT answer whether the pace is
        // uniform, bimodal or bursty — and cannot test the reading that the ~1 ms mean is simply the
        // 1 kHz APIC tick the polled pump sleeps on (`IRQ_COUNT=0` on every boot means `hlt()` is
        // woken by the timer and by nothing else, so per-stage latency is quantised to the tick).
        // Bucket 0 is "under 1 ms"; bucket k>0 holds waits of 2^(k-1)..2^k - 1 ms; the top bucket
        // saturates. Cheap: one integer divide and one relaxed add per awaited stage.
        {
            let ms = used / Self::cycles_per_ms().max(1);
            let b = if ms == 0 { 0usize } else {
                (64 - ms.leading_zeros() as usize).min(BOT_WAIT_BUCKETS.len() - 1)
            };
            BOT_WAIT_BUCKETS[b].fetch_add(1, Ordering::Relaxed);
        }
        // FRWRITE: accumulate EVERY wait, not just the record-setters — `sum/n` is the mean, and the
        // mean is what separates "one slow outlier" from "the whole write path runs at 0.5 s/sector".
        BOT_PUMP_CYCLES.fetch_add(used, Ordering::Relaxed);
        let prev = BOT_PUMP_PEAK.load(Ordering::Relaxed);
        if used <= prev {
            return;
        }
        BOT_PUMP_PEAK.store(used, Ordering::Relaxed);
        // Throttle against the last REPORTED peak, not the last peak: a peak that creeps up in small
        // increments would otherwise never double its immediate predecessor and never print at all.
        let reported = BOT_PUMP_REPORTED.load(Ordering::Relaxed);
        if used >= reported.saturating_mul(2).max(1) {
            BOT_PUMP_REPORTED.store(used, Ordering::Relaxed);
            // BOT-RESCUE M3 witness 3: IMAN and USBSTS on the SUCCESS line too.
            //
            // The instrument-baseline law: the timeout line already prints both registers, but a
            // reading with nothing to compare it against cannot falsify anything — "IMAN=0x3" on a
            // wedged boot is only evidence if we know what IMAN reads on a working one, and until
            // now no capture carried that. Printing them on the (throttled, logarithmically rare)
            // success line means every register reading in the log has a healthy baseline taken by
            // the same instrument on the same boot. Two volatile MMIO reads on a path that already
            // formats a line; nothing is decided from them.
            let (iman, usbsts) = unsafe {
                let ir0 = XHCI_IR0_BASE.load(Ordering::Acquire);
                let op = XHCI_OP_BASE.load(Ordering::Acquire);
                (if ir0 != 0 { core::ptr::read_volatile(ir0 as *const u32) } else { 0 },
                 if op != 0 { core::ptr::read_volatile((op + 0x04) as *const u32) } else { 0 })
            };
            serial_println!(
                ":: BOT: pump budget={} used={} peak={} route={:#x} depth={} slot={} n={} nowait={} timeouts={} IMAN={:#x} USBSTS={:#x} result=OK ::",
                budget, used, used, route, depth, slot,
                BOT_PUMP_COUNT.load(Ordering::Relaxed), BOT_PUMP_NOWAIT.load(Ordering::Relaxed),
                BOT_PUMP_TIMEOUTS.load(Ordering::Relaxed), iman, usbsts);
        }
    }

    /// Physical address of the storage slot's SCSI data buffer.
    fn storage_data_phys(&self, slot: u8) -> Result<u64, BotError> {
        self.slots[slot as usize].scsi_data_buffer.map(|p| p as u64).ok_or(BotError::NoDevice)
    }

    /// SCSI TEST UNIT READY (0x00), no data.
    fn scsi_test_unit_ready(&mut self, slot: u8) -> Result<CswStatus, BotError> {
        let cdb = [0u8; 6];
        Ok(self.bot_transfer(slot, &cdb, 0, 0, Direction::None)?.status)
    }

    /// SCSI REQUEST SENSE (0x03), 18 bytes — used to clear a CHECK CONDITION.
    fn scsi_request_sense(&mut self, slot: u8) -> Result<(), BotError> {
        let data_phys = self.storage_data_phys(slot)?;
        let cdb = [0x03, 0, 0, 0, 18, 0];
        self.bot_transfer(slot, &cdb, data_phys, 18, Direction::In)?;
        Ok(())
    }

    /// SCSI INQUIRY (0x12), 36 bytes. Returns (vendor[8], product[16]).
    fn scsi_inquiry(&mut self, slot: u8) -> Result<([u8; 8], [u8; 16]), BotError> {
        let data_phys = self.storage_data_phys(slot)?;
        let cdb = [0x12, 0, 0, 0, 36, 0];
        self.bot_transfer(slot, &cdb, data_phys, 36, Direction::In)?;
        let mut vendor = [0u8; 8];
        let mut product = [0u8; 16];
        unsafe {
            let d = core::slice::from_raw_parts(data_phys as *const u8, 36);
            vendor.copy_from_slice(&d[8..16]);
            product.copy_from_slice(&d[16..32]);
        }
        Ok((vendor, product))
    }

    /// SCSI READ CAPACITY(10) (0x25), 8 bytes BE. Returns (block_size, last_lba).
    fn scsi_read_capacity10(&mut self, slot: u8) -> Result<(u32, u32), BotError> {
        let data_phys = self.storage_data_phys(slot)?;
        let cdb = [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        self.bot_transfer(slot, &cdb, data_phys, 8, Direction::In)?;
        unsafe {
            let d = core::slice::from_raw_parts(data_phys as *const u8, 8);
            let last_lba = ((d[0] as u32) << 24) | ((d[1] as u32) << 16) | ((d[2] as u32) << 8) | (d[3] as u32);
            let block_size = ((d[4] as u32) << 24) | ((d[5] as u32) << 16) | ((d[6] as u32) << 8) | (d[7] as u32);
            Ok((block_size, last_lba))
        }
    }

    /// SCSI READ(10) (0x28) of `blocks` blocks at `lba` into the storage data buffer.
    ///
    /// MULTIBLK: `blocks` is now a real count rather than a permanent 1. The CDB always encoded it;
    /// what forbade it was the 512-byte staging buffer, which a `blocks > 1` transfer would have
    /// overrun. The bound below is therefore a BUFFER bound, and it is checked here — at the one
    /// place that turns `blocks` into a byte length — rather than trusted from callers.
    fn scsi_read10(&mut self, slot: u8, lba: u32, blocks: u16) -> Result<BotResult, BotError> {
        if blocks == 0 || blocks > STORAGE_MAX_BLOCKS {
            return Err(BotError::BadRequest);
        }
        let data_phys = self.storage_data_phys(slot)?;
        let len = (blocks as u32) * 512;
        let cdb = [0x28, 0,
            (lba >> 24) as u8, (lba >> 16) as u8, (lba >> 8) as u8, lba as u8,
            0, (blocks >> 8) as u8, blocks as u8, 0];
        self.bot_transfer(slot, &cdb, data_phys, len, Direction::In)
    }

    /// SCSI WRITE(10) (0x2A) of `blocks` blocks at `lba` from the storage data buffer.
    /// MULTIBLK: same buffer bound as [`XhciController::scsi_read10`], and for the same reason —
    /// a `blocks` the staging buffer cannot back is a refusal, never a truncated transfer.
    fn scsi_write10(&mut self, slot: u8, lba: u32, blocks: u16) -> Result<BotResult, BotError> {
        if blocks == 0 || blocks > STORAGE_MAX_BLOCKS {
            return Err(BotError::BadRequest);
        }
        let data_phys = self.storage_data_phys(slot)?;
        let len = (blocks as u32) * 512;
        let cdb = [0x2A, 0,
            (lba >> 24) as u8, (lba >> 16) as u8, (lba >> 8) as u8, lba as u8,
            0, (blocks >> 8) as u8, blocks as u8, 0];
        self.bot_transfer(slot, &cdb, data_phys, len, Direction::Out)
    }

    // ---- Public storage API used by the block layer / shell ----

    /// Pointer to the storage slot's data buffer. MULTIBLK: this now addresses
    /// [`STORAGE_DATA_BYTES`] bytes, not one block — the block layer stages up to
    /// [`STORAGE_MAX_BLOCKS`] sectors here for a single READ(10)/WRITE(10).
    pub fn storage_data_ptr(&self) -> Option<*mut u8> {
        if self.storage_slot == 0 { return None; }
        self.slots[self.storage_slot as usize].scsi_data_buffer
    }

    /// READ(10) into the storage data buffer for the cached storage slot.
    pub fn storage_read10(&mut self, lba: u32, blocks: u16) -> Result<BotResult, BotError> {
        let slot = self.storage_slot;
        if slot == 0 { return Err(BotError::NoDevice); }
        self.scsi_read10(slot, lba, blocks)
    }

    /// WRITE(10) from the storage data buffer for the cached storage slot.
    pub fn storage_write10(&mut self, lba: u32, blocks: u16) -> Result<BotResult, BotError> {
        let slot = self.storage_slot;
        if slot == 0 { return Err(BotError::NoDevice); }
        self.scsi_write10(slot, lba, blocks)
    }

    /// Full SCSI bring-up: TEST UNIT READY (with retry) -> INQUIRY -> READ CAPACITY,
    /// then publish geometry to the block-device registry.
    fn bring_up_storage(&mut self) -> Result<(), BotError> {
        let slot = self.storage_slot;
        if slot == 0 { return Err(BotError::NoDevice); }
        // BOT-RESCUE: a freshly enumerated disk inherits no escalation state, even if the
        // controller handed it a slot id a surrendered disk once held.
        self.bot_rescue_clear(slot);

        // Put the device in the USB CONFIGURED state before touching its bulk endpoints. Real USB
        // Mass-Storage requires a SET_CONFIGURATION before its bulk IN/OUT endpoints become active;
        // QEMU's usb-storage tolerates its absence — which is why BOT "worked" in emulation while on
        // real silicon the endpoints stay inactive and every SCSI command fails (device never becomes
        // a block device). The HID and hub paths already SET_CONFIGURATION; storage did not.
        self.storage_note = "SET_CONFIGURATION";
        match self.sync_control(slot, 0x00, 0x09, 1, 0, 0, 0, false) {
            Ok(1) => serial_println!("xHCI: storage SET_CONFIGURATION(1) OK (slot {})", slot),
            other => {
                serial_println!("xHCI: storage SET_CONFIGURATION unexpected {:?} (slot {})", other, slot);
                self.storage_note = "SET_CONFIGURATION failed";
                return Err(BotError::Stall);
            }
        }

        // TEST UNIT READY — USB sticks often report "becoming ready" a few times.
        self.storage_note = "TEST UNIT READY";
        // PH-2: this loop already IS a sense-and-retry loop — it is the one place a `Failed` CSW was
        // handled before this arc. Hold the CHECK CONDITION latch across it so `bot_transfer` keeps
        // propagating `Failed` verbatim here and the loop's own sense/retry cadence is unchanged;
        // the new handler owns the runtime path only.
        BOT_SENSE_ACTIVE.store(true, Ordering::Relaxed);
        for attempt in 0..16 {
            match self.scsi_test_unit_ready(slot) {
                Ok(CswStatus::Passed) => break,
                Ok(_) => { let _ = self.scsi_request_sense(slot); }
                Err(e) => { serial_println!("xHCI: TUR error {:?} (attempt {})", e, attempt); }
            }
        }
        BOT_SENSE_ACTIVE.store(false, Ordering::Relaxed);

        self.storage_note = "INQUIRY";
        let (vendor, product) = self.scsi_inquiry(slot)?;
        self.storage_note = "READ CAPACITY";
        let (block_size, last_lba) = self.scsi_read_capacity10(slot)?;
        let num_blocks = last_lba as u64 + 1;

        let vendor_s = core::str::from_utf8(&vendor).unwrap_or("?").trim_end();
        let product_s = core::str::from_utf8(&product).unwrap_or("?").trim_end();
        serial_println!("xHCI: Disk '{}' '{}' block_size={} num_blocks={} ({} MiB)",
            vendor_s, product_s, block_size, num_blocks,
            (num_blocks * block_size as u64) / (1024 * 1024));

        let dev_info = crate::drivers::block::BlockDeviceInfo {
            slot_id: slot, block_size, num_blocks, vendor, product,
        };
        // PIUSB-28: publish geometry through the backend-aware helper. It ALWAYS records the stick under
        // the dedicated USB handle (so the read-only /fs/usb mount reaches it via `read_block_usb`) and
        // raises the storage-ready edge (re-arming on every hot-plug re-enum, consumed OUTSIDE this
        // controller lock since the FAT mount re-locks the controller). It claims the GLOBAL BLOCK_DEVICE
        // only when USB is the active backend: on the Pi the microSD registered at BSP probe, so a later
        // USB stick must NOT clobber the SD's geometry (PI-FS-2: a 14 MiB card reader bounded fresh unafs
        // mounts → OutOfBounds(63)); on x86 the stick is the boot backend and still claims the global.
        crate::drivers::block::publish_usb_geometry(dev_info);
        // GUI-WITNESS: the USB block device is up (geometry published). One of the "did storage come
        // up?" milestones a silent boot otherwise can't answer on-panel.
        crate::bootlog::record("block:up");
        self.storage_note = "ready";
        // ONSET-2 (M2 witness 1): THE BASELINE. Taken here, at the one moment the whole chain is
        // provably healthy and idle — the device has enumerated, answered TEST UNIT READY, INQUIRY
        // and READ CAPACITY, and no BOT transaction is in flight. Every `portreg why=timeout` line
        // later in the boot is read against this one, on the same boot, by the same instrument.
        // Without it a `pls=0(U0)` at a timeout proves nothing, because nobody would know what this
        // controller reads when everything is fine.
        self.port_link_witness("bringup");
        // ONSET-2 (M3): the knob witness. `strings` proves the TEXT is in the artifact; this proves
        // the compiled-in VALUE at runtime, which is the thing that has bitten this project twice —
        // a knob wired into `arroyo` but not into `builder/` ships disabled while every gate stays
        // green. A boot log that does not say `ring_trbs=64` did not run the experiment.
        // BOT-CBW: `botring64` is still a knob (still default-off, still a diagnostic — the metal
        // evidence never convicted ring length). The third field is no longer a knob at all: it
        // reads `cbw=always-awaited` in every build, and its presence is how a capture is dated
        // against §17. A log still carrying `botcbwioc=` is from before the fix.
        // BOOTPACE M2: `order=console-first` joins it on the same terms — not a knob, a statement of
        // what the build always does (the SCSI bring-up waits for the enumeration queue to drain,
        // and `service_ftdi` precedes `service_storage` in both x86 ladders). It exists so a capture
        // can be dated: a log whose KNOBS line lacks this field predates the reordering, and its
        // storage-chain timings were taken with the console not yet armed.
        serial_println!(
            ":: BOT: knobs ring_trbs={} {} {} {} {} result=KNOBS ::",
            BOT_RING_TRBS, BOT_RING_KNOB_TAG, BOT_CBWIOC_KNOB_TAG, BOT_ORDER_TAG, BOT_PUMP_TAG);
        Ok(())
    }

    /// Main-loop hook: once storage finishes configuring, run the SCSI bring-up (in a
    /// safe, non-event context) and publish the block device. Also does a one-time
    /// sanity read of LBA 0.
    /// Multi-line dump of the live port + slot state for the shell `usbinfo` command — the metal
    /// diagnostic for "which USB devices enumerated, at what speed, and how far". Read-only.
    /// PIUSB-13: read-only enumeration observability for the Pi-side `enumerate()` pump. These
    /// expose the private root-enum FSM state (stage, in-flight port, last stall) and a structured
    /// root-port snapshot so `piusb::enumerate` can emit the `:: PIUSB: [enum] ... ::` milestone
    /// stream without duplicating the FSM. aarch64-gated: the block does not compile on x86, so x86
    /// codegen is byte-identical (nothing there reads this state). All methods are pure reads with
    /// no controller side effects.
    #[cfg(target_arch = "aarch64")]
    pub fn enum_stage_now(&self) -> &'static str { self.enum_stage }
    /// Root port currently mid-enumeration (0 = none).
    #[cfg(target_arch = "aarch64")]
    pub fn enumerating_port_now(&self) -> u8 { self.enumerating_port }
    /// Last recorded enumeration stall: (port, stage, why, completion-code, PORTSC). `None` until one.
    #[cfg(target_arch = "aarch64")]
    pub fn last_stall_now(&self) -> Option<(u8, &'static str, &'static str, u8, u32)> {
        self.last_stall.as_ref().map(|s| (s.port, s.stage, s.why, s.code, s.portsc))
    }
    /// Total enumeration stalls this boot.
    #[cfg(target_arch = "aarch64")]
    pub fn stall_count_now(&self) -> u32 { self.stall_count }
    /// Per-root-port snapshot for the observer: `(port, connected(CCS), xhci_speed_id)`.
    /// Speed id: 1=FS 2=LS 3=HS 4=SS, 0 = none/untrained.
    #[cfg(target_arch = "aarch64")]
    pub fn root_ports_now(&self) -> Vec<(u8, bool, u32)> {
        let mut v = Vec::new();
        for p in 1..=self.max_ports {
            let s = self.read_portsc(p);
            v.push((p, (s & 1) != 0, (s >> 10) & 0xF));
        }
        v
    }

    pub fn port_slot_summary(&self) -> Vec<alloc::string::String> {
        fn speed_name(s: u32) -> &'static str {
            match s { 1 => "FS", 2 => "LS", 3 => "HS", 4 => "SS", 0 => "-", _ => "?" }
        }
        // Port Link State — for an empty USB3 SuperSpeed port this shows why a device isn't seen
        // (RxDetect / Polling / Disabled / Inactive) vs an idle USB2 port.
        fn pls_name(p: u32) -> &'static str {
            match p {
                0 => "U0", 1 => "U1", 2 => "U2", 3 => "U3", 4 => "Disabled", 5 => "RxDetect",
                6 => "Inactive", 7 => "Polling", 8 => "Recovery", 9 => "HotReset",
                10 => "Compliance", 11 => "TestMode", 15 => "Resume", _ => "?",
            }
        }
        let mut out = Vec::new();
        out.push(alloc::format!(
            "xHCI ports={} storage_slot={} enum_active={} queued={} note='{}'",
            self.max_ports, self.storage_slot, self.enum_active,
            self.ports_to_enumerate.len(), self.storage_note));
        if self.enum_active {
            // The stall-localizer: WHICH port is in flight and at WHICH step, and for how long
            // (ms via the calibrated TSC) — a photo of this line replaces a debugger.
            let age_ms = crate::arch::now_cycles()
                .wrapping_sub(self.enum_stage_set_at)
                / (crate::arch::hw_wait_budget() / 2000).max(1);
            out.push(alloc::format!(
                "  enumerating port {}: stage={} for {} ms (resets={} cmd={:#x})",
                self.enumerating_port, self.enum_stage, age_ms, self.enum_resets,
                self.enum_cmd_phys));
        }
        if let Some(st) = &self.last_stall {
            out.push(alloc::format!(
                "  last stall: port {} @ {} ({}, code {}) PORTSC={:#010x}  [{} total]",
                st.port, st.stage, st.why, st.code, st.portsc, self.stall_count));
        }
        // Show ALL ports (not just connected ones), so an empty-but-present SuperSpeed port's link
        // state is visible — the whole point when a USB3 device isn't showing up as connected.
        for p in 1..=self.max_ports {
            let s = self.read_portsc(p);
            let proto = match self.port_major(p) { 3 => "usb3", 2 => "usb2", _ => "usb?" };
            out.push(alloc::format!(
                "  port {} [{}]: {:#010x} CCS={} PED={} PP={} PR={} CAS={} PLS={}({}) sp={}({})",
                p, proto, s, s & 1, (s >> 1) & 1, (s >> 9) & 1, (s >> 4) & 1, (s >> 24) & 1,
                (s >> 5) & 0xF, pls_name((s >> 5) & 0xF),
                (s >> 10) & 0xF, speed_name((s >> 10) & 0xF)));
        }
        for (i, slot) in self.slots.iter().enumerate() {
            if !slot.active { continue; }
            let role = if i as u8 == self.storage_slot { "STORAGE" }
                       else if slot.is_keyboard && slot.is_mouse { "kbd+mouse" }
                       else if slot.is_keyboard { "keyboard" }
                       else if slot.is_mouse { "mouse" }
                       else { "other/unconfigured" };
            out.push(alloc::format!(
                "  slot {}: port {} {} bulk_in={:#x} bulk_out={:#x}",
                i, slot.port_id, role, slot.bulk_in_ep, slot.bulk_out_ep));
        }
        out
    }

    pub fn service_storage(&mut self) {
        if !self.storage_pending_bringup { return; }
        // BOOTPACE M2 — CONSOLE-FIRST. Defer the whole SCSI bring-up until the enumeration queue
        // has drained. The latch is left SET (this is a `return`, not a consume), so the bring-up is
        // not skipped, only postponed to the first main-loop pass on which no port is mid-enumeration.
        //
        // Why: the FTDI console is the only instrument that exists on a metal boot, and it arms at
        // the END of its own port's enumeration. Before this, the storage Configure-Endpoint
        // completion armed `storage_pending_bringup` and the very next `service_storage` ran the
        // multi-second TUR/INQUIRY/READ-CAPACITY chain — plus the FAT mount and the first
        // flight-recorder flush behind it — while the remaining ports, INCLUDING the FTDI's, were
        // still queued. Every one of those seconds happened with no console attached, so the log
        // reached a second host only as a replay out of the 64 KiB capture ring, which drops the
        // oldest lines on overflow. Enumerating all ports back-to-back first costs the tail of
        // enumeration (hundreds of ms; worst case one wedged port's bounded watchdog — every
        // `service_enum` stage has one, so `enum_active` cannot stay true forever) and buys a live
        // wire for the entire storage chain.
        //
        // Nothing downstream needed changing: `fat::probe_once`, `flight_recorder::service` and the
        // fixtures all gate on `block::info()` internally, so they follow the block device's arrival.
        // This changes ORDER only — no protocol timing, no budget, no settle.
        if self.enum_active || !self.ports_to_enumerate.is_empty() { return; }
        self.storage_pending_bringup = false;
        if self.storage_slot == 0 { return; }

        serial_println!("xHCI: === STORAGE BRING-UP (TUR/INQUIRY/READ CAPACITY) ===");
        // BPACE: the SCSI bring-up chain begins. `d=` between this and `stor-ready` is the whole
        // TUR/INQUIRY/READ-CAPACITY negotiation — every one of whose stages is an awaited BOT
        // transaction, i.e. the phase the pump quantisation of §17.4 taxes hardest.
        crate::bootpace::record("stor-bringup");
        match self.bring_up_storage() {
            Ok(()) => serial_println!("xHCI: storage ready."),
            Err(e) => { serial_println!("xHCI: storage bring-up failed: {:?}", e); return; }
        }
        // BPACE: reached only on SUCCESS — a failed bring-up returns above, so a ledger carrying
        // `stor-bringup` without `stor-ready` says the storage chain died rather than ran slowly.
        crate::bootpace::record("stor-ready");

        // PIUSB-35: decisive DMA-address witness. P45 refuted the cache theory (the LBA0 read still
        // returns all-zero with a Passed/residue=0 CSW even with PIUSB-34's clean-before-doorbell +
        // post-invalidate), so the next suspect was the BCM2711 PCIe RC inbound window / DMA address:
        // if the deferred-phase heap sat above the RC's reachable window (or needed a dma-ranges
        // offset we don't apply), the VL805 would DMA the block into nowhere → stale zeros, while
        // short control transfers using low buffers still worked. STATIC AUDIT REFUTES this: the
        // aarch64 heap is placed at phys 0x0200_0000 (32 MiB), 64 MiB long (boot::MEM_REGIONS), RAM is
        // identity-mapped (VA==PA in the low 1 GiB block), and init_heap_raw hands out from that
        // physical region. The rings/DCBAA/event ring, the CBW buffer (DMA-READ by the device) and the
        // CSW buffer (DMA-WRITTEN by the device — it returns Passed) ALL come from the same 32–96 MiB
        // pool as scsi_data_buffer, and the RC inbound window is RAM@0 / 4 GiB / dma-ranges 1:1
        // (offset 0; see piusb::M1 RC_BAR2). A working CSW-write to that pool cannot coexist with an
        // unreachable data-write to the same pool. This witness prints the live physical addresses so
        // P46 confirms on-metal that the data DMA target is in-window and below 3 GiB, retiring the
        // address theory and redirecting to the length/TD-shape (or genuine device-side) discriminator.
        #[cfg(target_arch = "aarch64")]
        {
            let s = &self.slots[self.storage_slot as usize];
            let databuf = s.scsi_data_buffer.map(|p| p as u64).unwrap_or(0);
            let cbw = s.cbw_buffer.map(|p| p as u64).unwrap_or(0);
            let csw = s.csw_buffer.map(|p| p as u64).unwrap_or(0);
            let in_trb = s.bulk_in_ring.as_ref().map(|r| r.get_ptr()).unwrap_or(0);
            // BCM2711 RC inbound window: RAM base 0, 4 GiB, dma-ranges 1:1 (cpu→pci offset 0).
            const RC_INBOUND_BASE: u64 = 0;
            const RC_INBOUND_SIZE: u64 = 0x1_0000_0000; // 4 GiB
            const VL805_DMA_CEILING: u64 = 0xC000_0000; // classic <3 GiB VL805 DMA quirk boundary
            let in_window = databuf >= RC_INBOUND_BASE && databuf < RC_INBOUND_BASE + RC_INBOUND_SIZE;
            let below_3g = databuf < VL805_DMA_CEILING;
            serial_println!(
                ":: PIUSB: [piusb35] databuf phys={:#x} in_trb={:#x} cbw={:#x} csw={:#x} | rc-inbound=[{:#x},{:#x}) offset=0 (1:1) | databuf in_window={} below_3G={} — CBW(DMA-read)+CSW(DMA-write→Passed) share this pool; address theory {} ::",
                databuf, in_trb, cbw, csw,
                RC_INBOUND_BASE, RC_INBOUND_BASE + RC_INBOUND_SIZE,
                in_window, below_3g,
                if in_window && below_3g {
                    "REFUTED on-metal (data DMA target reachable — look at length/TD-shape or device-side)"
                } else {
                    "HOLDS — data buffer is OUT of the reachable inbound window; move BOT buffers to a low DMA pool"
                });
        }

        // Sanity read of LBA 0.
        match self.storage_read10(0, 1) {
            Ok(res) => {
                serial_println!("xHCI: READ(10) LBA0 CSW status={:?} residue={}", res.status, res.residue);
                if let Some(p) = self.storage_data_ptr() {
                    unsafe {
                        let data = core::slice::from_raw_parts(p as *const u8, 512);
                        let sig = core::str::from_utf8(&data[0..21]).unwrap_or("INVALID");
                        serial_println!("xHCI: SECTOR 0 SIGNATURE: {}", sig);
                        if sig == "UNA-OS-DISK-001-ALPHA" {
                            serial_println!("xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<");
                        }
                        serial_println!("xHCI: [IRQ] xHCI interrupts taken so far: {}",
                            XHCI_IRQ_COUNT.load(Ordering::Relaxed));
                        // PIUSB-25: Pi mass-storage enumeration + LBA0 read-proof witness. aarch64-gated
                        // (byte-identical x86 codegen — nothing here changes the BOT/CSW core path; the
                        // `data` slice is the block just read by the READ(10) above, invalidated at the
                        // shared `bot_transfer` IN chokepoint so it is fresh DRAM on Pi silicon). Reports
                        // the slot + geometry, the first 16 bytes hex, and a boot-sector sanity decode
                        // (0x55AA signature, FAT/GPT hints). Read-only — never a write.
                        #[cfg(target_arch = "aarch64")]
                        {
                            // USBW-1: report the USB device's OWN geometry. Reading the global
                            // BLOCK_DEVICE here made this line print the microSD's 500224000 blocks
                            // (244250 MiB) under a "storage enumerated" label on P57, two lines below
                            // the true READ CAPACITY of 29120 blocks (14 MiB) — the mix-up that sent
                            // the write self-test off the end of the reader.
                            let (bs, nb, mib) = match crate::drivers::block::USB_BLOCK_DEVICE.lock().as_ref() {
                                Some(d) => (d.block_size, d.num_blocks,
                                            (d.num_blocks.saturating_mul(d.block_size as u64)) / (1024 * 1024)),
                                None => (0, 0, 0),
                            };
                            serial_println!(
                                ":: PIUSB: [piusb25] storage enumerated: slot {} bulk_in={:#04x} bulk_out={:#04x} block_size={} num_blocks={} ({} MiB) ::",
                                self.storage_slot,
                                self.slots[self.storage_slot as usize].bulk_in_ep,
                                self.slots[self.storage_slot as usize].bulk_out_ep,
                                bs, nb, mib);
                            serial_println!(
                                ":: PIUSB: [piusb25] READ(10) LBA0 CSW={:?} residue={} — first 16 bytes: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
                                res.status, res.residue,
                                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
                                data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15]);
                            let boot_sig = data[510] == 0x55 && data[511] == 0xAA;
                            let gpt_protective = data[0x1C2] == 0xEE;   // protective-MBR partition type
                            let fat16 = data[0x36..0x3B].starts_with(b"FAT");
                            let fat32 = data[0x52..0x57].starts_with(b"FAT");
                            let fs = if gpt_protective { "GPT (protective MBR)" }
                                     else if fat32 { "FAT32 BPB" }
                                     else if fat16 { "FAT12/16 BPB" }
                                     else { "unrecognized/raw" };
                            serial_println!(
                                ":: PIUSB: [piusb25] boot-sector sanity: 0x55AA={} type={} ::",
                                boot_sig, fs);
                        }
                    }
                }
                // PIUSB-34: fix-proof witness. Re-issue READ(10) LBA0 through the same BOT path
                // (which now cleans the IN buffer before the doorbell AND invalidates after) and dump
                // the first 16 bytes of the freshly-DMA'd + post-invalidate DRAM. On P44 this printed
                // zeros; on P45 it must match the real boot sector. aarch64-only, read-only.
                #[cfg(target_arch = "aarch64")]
                if let Ok(re) = self.storage_read10(0, 1) {
                    if let Some(p) = self.storage_data_ptr() {
                        unsafe {
                            let d = core::slice::from_raw_parts(p as *const u8, 16);
                            serial_println!(
                                ":: PIUSB: [piusb34] LBA0 re-read post-invalidate: CSW={:?} residue={} — {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
                                re.status, re.residue,
                                d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7],
                                d[8], d[9], d[10], d[11], d[12], d[13], d[14], d[15]);
                        }
                    }
                }
            }
            Err(e) => serial_println!("xHCI: READ(10) LBA0 failed: {:?}", e),
        }

        // PIUSB-36: one-boot decisive experiment matrix for the Pi-only 512-B-read-returns-zeros
        // wedge (READ CAPACITY 8 B works, READ(10) 512 B returns Passed/residue=0/zeros). Read-only;
        // aarch64 witness only — no-op on x86 (never compiled). Runs after the baseline witnesses so
        // its buffer/TD-shape/posted-write experiments sit on the same enumerated slot.
        #[cfg(target_arch = "aarch64")]
        self.piusb36_matrix();

        // PIUSB-37: chase the READ(10)-returns-zeros wedge into the SCSI command itself — CBW audit,
        // command-set / known-nonzero-LBA matrix, REQUEST SENSE (UNIT ATTENTION candidate), and a
        // TUR-drain-then-retry. Read-only; aarch64 witness only (no-op on x86). Runs after the
        // piusb36 matrix so it sits on the same enumerated slot.
        #[cfg(target_arch = "aarch64")]
        self.piusb37_matrix();

        // PIUSB-38: prove BOT Reset Recovery (induce a stall, then TUR/REQUEST-SENSE must still
        // complete — the P47 wedge fix) and run the low-LBA-zeros bisect (read ladder 0..8192,
        // zeros→data boundary, LBA0-vs-LBA8192 diff). Read-only; aarch64 witness only (no-op on
        // x86). Runs after piusb37 on the same enumerated slot, before the write self-test.
        #[cfg(target_arch = "aarch64")]
        self.piusb38_matrix();

        // USB-WRITE: prove the BOT WRITE(10) OUT path on the enumerated stick with a
        // read-modify-write-restore of one scratch sector (byte-identical afterward).
        self.mission_write_selftest();
    }

    /// USB-WRITE: MISSION write proof — a read-modify-write-restore of a single scratch sector well
    /// past the filesystem area (the last block), leaving the medium BYTE-IDENTICAL. Exercises the
    /// xHCI BOT WRITE(10) OUT data stage end to end (CBW OUT -> DATA OUT -> CSW) via the same
    /// `storage_write10`/`storage_read10` the block layer uses, with a readback assertion after both
    /// the pattern write AND the restore. Emits `[usbw] write lba=<n> ok` only when every step
    /// verifies; ANY divergence emits a `-> FAIL` witness (which the battery's generic FAIL scan
    /// catches) and NO success is claimed — no partial-write lie. Runs in the same safe non-event
    /// context as the LBA0 sanity read; single-sector, bounded to the storage slot's own DMA buffer.
    /// USBW-1: derive the **keep-out ceiling** — the first LBA on the USB medium that is provably
    /// NOT claimed by on-disk structures — by parsing sector 0. The scratch sector must sit at or
    /// above this. Returns `(ceiling, provenance)`; `ceiling == 0` means sector 0 carries no
    /// recognizable container and the medium is raw.
    ///
    /// This exists because "near the end of the medium" is NOT the same as "clear of the
    /// filesystem". The bench card is a **superfloppy**: the FAT16 BPB sits at LBA 0 with partition
    /// offset 0, so the volume's LBA space IS the raw medium's and a whole-card volume runs to the
    /// very last sector. Every top-of-medium candidate lands *inside* the live `/fs/usb` data
    /// region — where `UNAOS.LOG` grows each boot, so tail clusters are not free by construction.
    ///
    /// Cases, fail-closed (an unreadable or ambiguous sector 0 yields the whole medium as keep-out,
    /// which makes the caller skip rather than guess):
    /// - **GPT** (protective MBR type 0xEE): the *backup GPT header lives in the last LBA*, so the
    ///   top of the medium is the worst possible scratch. Whole medium.
    /// - **MBR**: ceiling = the highest `start + size` over the valid primary entries.
    /// - **FAT BPB at LBA 0** (superfloppy): ceiling = `BPB_TotSec16`, else `BPB_TotSec32`.
    /// - **Raw**: ceiling 0.
    fn usbw_keepout_ceiling(&mut self, nb: u64) -> (u64, &'static str) {
        let ptr = match self.storage_data_ptr() { Some(p) => p, None => return (nb, "no-dma-buffer") };
        match self.storage_read10(0, 1) {
            Ok(r) if r.status == CswStatus::Passed => {}
            _ => return (nb, "sector0-unreadable"),
        }
        let mut s0 = [0u8; 512];
        unsafe { core::ptr::copy_nonoverlapping(ptr as *const u8, s0.as_mut_ptr(), 512); }

        let le16 = |o: usize| (s0[o] as u32) | ((s0[o + 1] as u32) << 8);
        let le32 = |o: usize| {
            (s0[o] as u64) | ((s0[o + 1] as u64) << 8) | ((s0[o + 2] as u64) << 16)
                | ((s0[o + 3] as u64) << 24)
        };
        let signed = s0[510] == 0x55 && s0[511] == 0xAA;

        // A FAT BPB is identified by its structural fields, not by a string: 512-byte sectors, a
        // power-of-two cluster size, at least one reserved sector and at least one FAT.
        let bps = le16(11);
        let spc = s0[13] as u32;
        let rsvd = le16(14);
        let nfats = s0[16] as u32;
        let is_bpb = bps == 512 && spc != 0 && (spc & (spc - 1)) == 0 && rsvd != 0
            && nfats >= 1 && nfats <= 2;

        if signed {
            // MBR-shaped table first — a protective 0xEE means GPT.
            let mut max_end = 0u64;
            let mut any = false;
            for i in 0..4 {
                let e = 446 + i * 16;
                let ptype = s0[e + 4];
                if ptype == 0x00 { continue; }
                if ptype == 0xEE { return (nb, "gpt-protective (backup header in last LBA)"); }
                let start = le32(e + 8);
                let size = le32(e + 12);
                if size == 0 { continue; }
                any = true;
                let end = start.saturating_add(size);
                if end > max_end { max_end = end; }
            }
            if any { return (max_end.min(nb), "mbr-partition-table"); }
            if is_bpb {
                let tot = if le16(19) != 0 { le16(19) as u64 } else { le32(32) };
                if tot != 0 { return (tot.min(nb), "fat-bpb superfloppy (BPB_TotSec)"); }
                return (nb, "fat-bpb superfloppy (TotSec unreadable)");
            }
            // Signed, but neither a usable partition table nor a BPB — do not guess.
            return (nb, "signed-but-unrecognized sector 0");
        }
        if is_bpb {
            let tot = if le16(19) != 0 { le16(19) as u64 } else { le32(32) };
            if tot != 0 { return (tot.min(nb), "fat-bpb superfloppy (BPB_TotSec, unsigned)"); }
            return (nb, "fat-bpb superfloppy (TotSec unreadable)");
        }
        (0, "raw (no container in sector 0)")
    }

    fn mission_write_selftest(&mut self) {
        // USBW-1: the scratch LBA MUST come from the USB stick's own READ CAPACITY, never from the
        // global `block::info()`. On the Pi the microSD registers first and (since PIUSB-28) KEEPS the
        // global, so `info()` returns the SD's geometry — P57 read 500224000 blocks (244250 MiB, the
        // eMMC card) while the enumerated 'USB SD Reader' is 29120 blocks (14 MiB). The probe then
        // addressed lba=500223999 over BOT, ~17000x past the reader's last LBA; the device answered
        // correctly with CHECK CONDITION (CSW Failed, residue 512) after halting the data-IN, and the
        // test mislabeled that honest out-of-range rejection as a transport stall. `usb_info()` is the
        // dedicated USB handle published by the same enumeration that owns this BOT pipe. On x86
        // `publish_usb_geometry` writes both handles, so this is byte-identical there.
        let nb = match crate::drivers::block::usb_info() {
            Some(d) => d.num_blocks,
            None => {
                // USBW-1: never skip silently — an unpublished USB handle used to make the whole
                // write proof vanish without a trace, which is indistinguishable from "it ran".
                serial_println!(
                    ":: PIUSB: [usbw] scratch skipped: no USB block geometry published ::");
                return;
            }
        };
        if nb < 2 {
            serial_println!(":: PIUSB: [usbw] scratch skipped: USB medium too small (num_blocks={}) ::", nb);
            return;
        }

        // USBW-1: establish the keep-out ceiling BEFORE choosing a candidate. "Top of the medium" is
        // not a safe scratch location by itself — on a superfloppy the FAT volume's LBA space IS the
        // raw medium's, so a whole-card volume reaches the last sector and every top-of-medium
        // candidate is inside the live data region.
        let (ceiling, prov) = self.usbw_keepout_ceiling(nb);
        let ptr = match self.storage_data_ptr() { Some(p) => p, None => return };

        // USB-WRITE-2 (as amended by USBW-1): pick a scratch sector near the end of the medium but
        // strictly ABOVE the keep-out ceiling, falling BACK progressively when a choice STALLs.
        // Metal reality (P44): some sticks report a READ CAPACITY last-LBA they will then STALL a
        // READ(10) against — the very-last sector is not always addressable. Try last, last-8,
        // last-64 in order, keeping only candidates at/above the ceiling; each pre-read that halts is
        // recovered inside `bot_transfer`, so the next candidate rides a clean pipe. When the
        // container spans the medium there is NO safe sector and the probe skips outright — it does
        // not fall back to writing inside a mounted volume.
        serial_println!(
            ":: PIUSB: [usbw] scratch geometry: USB last_lba={} (num_blocks={}), keep-out ceiling={} [{}] ::",
            nb - 1, nb, ceiling, prov);
        if ceiling >= nb {
            serial_println!(
                ":: PIUSB: [usbw] scratch skipped: on-disk container spans the medium ({}), no sector outside it — refusing to RMW inside a live volume ::",
                prov);
            return;
        }
        let mut lba = (nb - 1) as u32;
        {
            let mut candidates = [0u32; 3];
            let mut ncand = 0usize;
            for off in [1u64, 8, 64] {
                // Strict `>` (lens fold): with nb == off the candidate would be LBA 0 — the boot
                // sector, the one sector everything else treats as sacred. Only reachable on a raw
                // 8/64-sector medium, but the "near the END of the medium" intent is absolute.
                if nb > off && (nb - off) >= ceiling {
                    candidates[ncand] = (nb - off) as u32;
                    ncand += 1;
                }
            }
            if ncand == 0 {
                serial_println!(
                    ":: PIUSB: [usbw] scratch skipped: no candidate at/above the keep-out ceiling {} ::",
                    ceiling);
                return;
            }
            lba = candidates[0];
            let mut chosen: Option<u32> = None;
            for i in 0..ncand {
                let cand = candidates[i];
                match self.storage_read10(cand, 1) {
                    Ok(r) if r.status == CswStatus::Passed => { chosen = Some(cand); break; }
                    other => {
                        serial_println!(
                            ":: PIUSB: [usbw] pre-read lba={} -> {:?}, falling back ::", cand, other);
                    }
                }
            }
            match chosen {
                Some(c) => {
                    if c != lba {
                        serial_println!(":: PIUSB: [usbw] fallback lba={} ::", c);
                    }
                    lba = c;
                }
                None => {
                    // USBW-1: do NOT call this "stalled" — that verdict was wrong on P57. Every
                    // candidate came back as a COMPLETED BOT round trip whose CSW said Failed
                    // (residue 512), i.e. the device rejected the command; stall recovery had already
                    // run and cleared the halt, which is why the next candidate got a CSW at all.
                    serial_println!(
                        ":: PIUSB: [usbw] write lba={} -> FAIL (no readable scratch candidate; see per-candidate CSW above) ::",
                        lba);
                    return;
                }
            }
        }

        // 1) The chosen sector is already in the DMA buffer from its successful pre-read; stash the
        //    ORIGINAL 512 bytes before we perturb it.
        let mut orig = [0u8; 512];
        unsafe { core::ptr::copy_nonoverlapping(ptr as *const u8, orig.as_mut_ptr(), 512); }

        // 2) Stage a distinct pattern into the DMA buffer and WRITE it.
        let mut pat = [0u8; 512];
        for i in 0..512 { pat[i] = (orig[i] ^ 0xA5).wrapping_add(i as u8); }
        unsafe { core::ptr::copy_nonoverlapping(pat.as_ptr(), ptr, 512); }
        match self.storage_write10(lba, 1) {
            Ok(r) if r.status == CswStatus::Passed => {}
            other => {
                serial_println!(":: PIUSB: [usbw] write lba={} -> FAIL (write {:?}) ::", lba, other);
                self.restore_sector(lba, &orig); return;
            }
        }

        // 3) READ back; the medium must now hold the pattern verbatim.
        match self.storage_read10(lba, 1) {
            Ok(r) if r.status == CswStatus::Passed => {}
            other => {
                serial_println!(":: PIUSB: [usbw] write lba={} -> FAIL (verify-read {:?}) ::", lba, other);
                self.restore_sector(lba, &orig); return;
            }
        }
        let mut rb = [0u8; 512];
        unsafe { core::ptr::copy_nonoverlapping(ptr as *const u8, rb.as_mut_ptr(), 512); }
        if rb != pat {
            serial_println!(":: PIUSB: [usbw] write lba={} -> FAIL (readback mismatch) ::", lba);
            self.restore_sector(lba, &orig); return;
        }

        // 4) RESTORE the original and confirm the medium is byte-identical again.
        if !self.restore_sector(lba, &orig) {
            serial_println!(":: PIUSB: [usbw] write lba={} -> FAIL (restore write) ::", lba); return;
        }
        match self.storage_read10(lba, 1) {
            Ok(r) if r.status == CswStatus::Passed => {}
            other => { serial_println!(":: PIUSB: [usbw] write lba={} -> FAIL (restore-verify {:?}) ::", lba, other); return; }
        }
        let mut chk = [0u8; 512];
        unsafe { core::ptr::copy_nonoverlapping(ptr as *const u8, chk.as_mut_ptr(), 512); }
        if chk != orig {
            serial_println!(":: PIUSB: [usbw] write lba={} -> FAIL (not restored) ::", lba); return;
        }

        serial_println!(":: PIUSB: [usbw] write lba={} ok — RMW+readback+restore, medium byte-identical ::", lba);
    }

    /// USB-WRITE: stage `data` into the storage slot's DMA buffer and WRITE(10) it to `lba` (single
    /// sector). Returns true iff the CSW reported Passed. Used to restore the scratch sector so the
    /// medium is left byte-identical after the self-test.
    fn restore_sector(&mut self, lba: u32, data: &[u8; 512]) -> bool {
        let ptr = match self.storage_data_ptr() { Some(p) => p, None => return false };
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), ptr, 512); }
        matches!(self.storage_write10(lba, 1), Ok(r) if r.status == CswStatus::Passed)
    }

    /// Main-loop hook (U2.5): bring up the FTDI console ONCE (SET_CONFIGURATION + the four FTDI
    /// vendor requests, all synchronous EP0 in this safe non-event context), then drain the
    /// boot-capture ring out its bulk-OUT endpoint on EVERY call while the sink is live. Wired into
    /// both main loops beside `service_hid_setproto`.
    pub fn service_ftdi(&mut self) {
        if self.ftdi_pending_bringup {
            self.ftdi_pending_bringup = false;
            let slot = self.ftdi_slot;
            if slot == 0 {
                return;
            }
            serial_println!("xHCI: === FTDI CONSOLE BRING-UP (SET_CONFIG + vendor setup) ===");
            // SET_CONFIGURATION(1) — put the device in the CONFIGURED state so its bulk endpoints go
            // active. bmRequestType 0x00 (host->device | standard | device), bRequest 0x09 — the exact
            // call `bring_up_storage` makes.
            match self.sync_control(slot, 0x00, 0x09, 1, 0, 0, 0, false) {
                Ok(1) => {}
                other => {
                    serial_println!(":: U2.5: FTDI setup failed (SET_CONFIGURATION {:?}) ::", other);
                    // GUI-WITNESS: the console path was reached but bring-up FAILED — the whole point
                    // of the ring is to split "console never armed" from "armed but TX never left."
                    crate::bootlog::record("ftdi:failed");
                    return;
                }
            }
            // FTDI vendor requests: bmRequestType 0x40 (host->device | vendor | device), wIndex 0, no
            // data stage. Order per Linux ftdi_sio: RESET → SET_BAUDRATE → SET_DATA → SET_FLOW_CTRL.
            let steps: [(&str, u8, u16); 4] = [
                ("RESET", ftdi::FTDI_SIO_RESET, 0x0000),
                ("SET_BAUDRATE", ftdi::FTDI_SIO_SET_BAUDRATE, ftdi::FTDI_BAUD_115200),
                ("SET_DATA", ftdi::FTDI_SIO_SET_DATA, ftdi::FTDI_DATA_8N1),
                ("SET_FLOW_CTRL", ftdi::FTDI_SIO_SET_FLOW_CTRL, 0x0000),
            ];
            for (name, b_req, w_value) in steps {
                match self.sync_control(slot, 0x40, b_req, w_value, 0, 0, 0, false) {
                    Ok(1) => {}
                    other => {
                        serial_println!(":: U2.5: FTDI setup failed ({} {:?}) ::", name, other);
                        // GUI-WITNESS: reached the console path, vendor setup FAILED.
                        crate::bootlog::record("ftdi:failed");
                        return;
                    }
                }
            }
            serial_println!(":: U2.5: FTDI console up (0403:6001, 115200 8N1) ::");
            // GUI-WITNESS: the FTDI console reported UP. If the panel shows this but a second host
            // sees no bytes, the silence is post-arm TX, not a bring-up failure. That split is the
            // bench ask this ring exists to answer.
            crate::bootlog::record("ftdi:console-up");
            // BPACE: the moment a second host can see anything at all. Everything stamped BEFORE
            // this reached the wire only via the capture ring's replay; everything after rides live.
            // `ftdi=` on the total line is therefore also the ledger's own observability boundary.
            crate::bootpace::record("ftdi-up");
            ftdi::set_live(true);
        }
        self.drain_ftdi();
    }

    /// Drain the FTDI boot-capture ring out the console's bulk-OUT endpoint, ≤512 B per transfer,
    /// until the ring is empty. Bounded + non-blocking by construction: any timeout / non-success
    /// completion turns the sink OFF permanently and drops all further output — the kernel must never
    /// wedge on console TX.
    fn drain_ftdi(&mut self) {
        if !ftdi::is_live() {
            return;
        }
        let slot = self.ftdi_slot;
        if slot == 0 {
            return;
        }
        let (out_ep, data_phys) = {
            let s = &self.slots[slot as usize];
            let dp = match s.scsi_data_buffer {
                Some(p) => p as u64,
                None => return,
            };
            (s.bulk_out_ep, dp)
        };
        if out_ep == 0 {
            return;
        }
        let out_dci = (out_ep & 0x0F) * 2;

        loop {
            // Stage up to 512 B of the oldest ring bytes into the FTDI slot's DMA buffer (reused as
            // the TX staging buffer — the FTDI slot never runs BOT, so its `scsi_data_buffer` is free).
            let n = unsafe { ftdi::drain_into(data_phys as *mut u8, 512) };
            if n == 0 {
                break; // ring drained
            }
            match self.ftdi_tx_stage(slot, out_dci, data_phys, n as u32) {
                Ok(1) | Ok(13) => self.ftdi_tx_total += n as u64,
                Ok(_) => {
                    self.disable_ftdi_tx("bad completion code");
                    return;
                }
                Err(()) => {
                    self.disable_ftdi_tx("timeout");
                    return;
                }
            }
        }
        // First clean empty of the backlog: announce the mirror is live. The PASS line itself enters
        // the ring and rides the NEXT drain — that is expected, and is the gate's proof the sink stays
        // live end to end.
        if !self.ftdi_pass_logged && self.ftdi_tx_total > 0 {
            self.ftdi_pass_logged = true;
            serial_println!(
                ":: U2.5: FTDI TX mirror -> PASS ({} boot bytes replayed) ::",
                self.ftdi_tx_total
            );
        }
    }

    /// Turn the FTDI TX sink off permanently and log it exactly once.
    fn disable_ftdi_tx(&mut self, reason: &'static str) {
        ftdi::set_live(false);
        if !self.ftdi_disabled_logged {
            self.ftdi_disabled_logged = true;
            serial_println!(":: U2.5: FTDI TX disabled ({}) ::", reason);
        }
    }

    /// Push one Normal TRB (`len` bytes from `data_phys`) onto the FTDI slot's bulk-OUT ring, ring the
    /// OUT doorbell, and pump the event ring until its completion arrives (matched by TRB address).
    /// Returns the completion code. A slimmer twin of `run_bot_stage` (single stage, no CBW/CSW).
    fn ftdi_tx_stage(&mut self, slot_id: u8, out_dci: u8, data_phys: u64, len: u32) -> Result<u8, ()> {
        // XHCI-COHERENCE: the TX staging buffer is CPU-written (drained from the serial ring) and
        // DMA-read by the controller (bulk OUT); clean it to DRAM before its doorbell. No-op x86.
        dma_coherency::clean(data_phys as usize, len as usize);
        let wait_trb_phys = {
            let ring = self.slots[slot_id as usize].bulk_out_ring.as_mut().ok_or(())?;
            let base = ring.get_ptr();
            let idx = ring
                .push(Trb { parameter: data_phys, status: len, control: (1 << 10) | (1 << 5) })
                .map_err(|_| ())?;
            base + (idx as u64) * 16
        };
        self.ftdi_pending = Some(FtdiPending {
            slot_id,
            out_dci,
            wait_trb_phys,
            done: false,
            completion_code: 0,
        });
        self.ring_doorbell(slot_id, out_dci as u32);
        let pump = self.pump_until_ftdi_done();
        let pending = self.ftdi_pending.take();
        pump?;
        Ok(pending.ok_or(())?.completion_code)
    }

    /// Pump the event ring until the in-flight FTDI TX transfer reports done, or a WALL-CLOCK
    /// budget is exhausted. Unrelated events are dispatched normally during the wait. Mirrors
    /// `pump_until_bot_done`, including its budget idiom: a `now_cycles`/`hw_wait_budget()`
    /// deadline, NOT a raw iteration count — so the pump is correct regardless of how long each
    /// `crate::hlt()` yields. Where a timer is live `hlt()` costs a tick per empty pass, but on a
    /// timerless core it busy-spins and a fixed iteration budget would expire in microseconds,
    /// long before a real completion lands. `now_cycles` (rdtsc / CNTVCT) advances either way.
    fn pump_until_ftdi_done(&mut self) -> Result<(), ()> {
        let start = crate::arch::now_cycles();
        let budget = crate::arch::hw_wait_budget();
        loop {
            match &self.ftdi_pending {
                Some(p) if p.done => {
                    Self::note_ftdi_pump(start, budget);
                    return Ok(());
                }
                None => return Ok(()),
                _ => {}
            }
            if self.drain_event_ring_once() {
                continue;
            }
            crate::hlt();
            let elapsed = crate::arch::now_cycles().wrapping_sub(start);
            if elapsed >= budget {
                FTDI_PUMP_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
                serial_println!(
                    ":: FTDI: tx pump budget={} used={} n={} timeouts={} result=TIMEOUT ::",
                    budget, elapsed, FTDI_PUMP_COUNT.load(Ordering::Relaxed),
                    FTDI_PUMP_TIMEOUTS.load(Ordering::Relaxed));
                return Err(());
            }
        }
    }

    /// Fold ONE completed FTDI TX pump wait into the headroom counters, and print the
    /// `:: FTDI: … result=OK ::` witness when it sets a high-water mark that at least DOUBLES the
    /// last reported one — the same log-scale throttle `note_bot_pump` uses, so the console's
    /// serial traffic cannot flood a default-quiet boot while the LAST such line still reports the
    /// true worst-case TX wait the run needed.
    fn note_ftdi_pump(start: u64, budget: u64) {
        let used = crate::arch::now_cycles().wrapping_sub(start);
        FTDI_PUMP_COUNT.fetch_add(1, Ordering::Relaxed);
        if used <= FTDI_PUMP_PEAK.load(Ordering::Relaxed) {
            return;
        }
        FTDI_PUMP_PEAK.store(used, Ordering::Relaxed);
        let reported = FTDI_PUMP_REPORTED.load(Ordering::Relaxed);
        if used >= reported.saturating_mul(2).max(1) {
            FTDI_PUMP_REPORTED.store(used, Ordering::Relaxed);
            serial_println!(
                ":: FTDI: tx pump budget={} used={} n={} timeouts={} result=OK ::",
                budget, used, FTDI_PUMP_COUNT.load(Ordering::Relaxed),
                FTDI_PUMP_TIMEOUTS.load(Ordering::Relaxed));
        }
    }

    /// Synchronous EP0 control transfer: queue Setup/[Data]/Status on the slot's EP0 ring, ring
    /// the doorbell, and pump the event ring until the Status TRB retires (matched by address).
    /// Used for hub-class requests during hub bring-up (a safe, main-loop, non-event context —
    /// like the synchronous BOT pump). Returns the completion code (1 = success).
    fn sync_control(&mut self, slot_id: u8, bm_req: u8, b_req: u8, w_value: u16, w_index: u16,
                    w_length: u16, data_phys: u64, dir_in: bool) -> Result<u8, ()> {
        let setup: u64 = (bm_req as u64)
            | ((b_req as u64) << 8)
            | ((w_value as u64) << 16)
            | ((w_index as u64) << 32)
            | ((w_length as u64) << 48);

        // XHCI-COHERENCE: producer-side eviction for the data buffer. Some callers pre-zero the buffer
        // (e.g. the 8-byte MPS0-learn) or reuse it, leaving dirty CPU lines; clean them to DRAM BEFORE
        // the controller DMAs so a delayed eviction can't later clobber its write (IN) or so it reads
        // current bytes (OUT). The IN buffer is invalidated again after completion, below. No-op x86.
        if w_length > 0 {
            dma_coherency::clean(data_phys as usize, w_length as usize);
        }

        // Setup stage. TRT: 0 = no data, 2 = OUT data, 3 = IN data.
        let trt: u32 = if w_length == 0 { 0 } else if dir_in { 3 } else { 2 };
        self.push_ep0(slot_id, Trb { parameter: setup, status: 8, control: (2 << 10) | (1 << 6) | (trt << 16) });

        // Data stage (if any). XENUM-3 M1: set IOC (bit 5) so the DATA stage posts its OWN transfer
        // event carrying the TRB Transfer Length residual — without it only the Status TRB (IOC)
        // reports, and the actual transferred length is invisible. The extra event is claimed and
        // consumed by the sync EP0 pump (it never reaches the async FSM); the Status event still
        // drives completion. This lets the downstream enumerator reject a short (partial) read.
        let data_trb_phys = if w_length > 0 {
            let dir: u32 = if dir_in { 1 } else { 0 };
            self.push_ep0(slot_id, Trb { parameter: data_phys, status: w_length as u32, control: (3 << 10) | (1 << 5) | (dir << 16) })
        } else {
            0
        };

        // Status stage (IOC). Direction is opposite the data stage; with no data it is IN.
        let status_dir: u32 = if w_length == 0 { 1 } else if dir_in { 0 } else { 1 };
        let status_phys = {
            let ring = self.slots[slot_id as usize].ep0_ring.as_mut().ok_or(())?;
            let base = ring.get_ptr();
            let idx = ring.push(Trb { parameter: 0, status: 0, control: (4 << 10) | (1 << 5) | (status_dir << 16) }).unwrap_or(0);
            base + (idx as u64) * 16
        };

        self.ep0_pending = Some(Ep0Pending {
            slot_id, wait_trb_phys: status_phys, done: false, completion_code: 0,
            data_trb_phys, data_residual: 0, data_seen: false,
        });
        self.ring_doorbell(slot_id, 1);

        let pump = self.pump_until_ep0_done();
        let pending = self.ep0_pending.take();
        pump?;
        // XHCI-COHERENCE: consumer boundary (one chokepoint for every control-IN reader — device /
        // config / hub-status descriptors all land here). The controller DMA-wrote `data_phys`;
        // invalidate the CPU's stale lines so the caller parses fresh DRAM. No-op x86.
        if dir_in && w_length > 0 {
            dma_coherency::inval(data_phys as usize, w_length as usize);
        }
        let p = pending.ok_or(())?;
        // XENUM-3 M1: surface the actual transferred length. If the DATA stage reported a residual,
        // the read was short; with no data stage (zero-length control) the full "length" is 0.
        self.last_control_len = if p.data_seen {
            (w_length as u32).saturating_sub(p.data_residual)
        } else {
            0
        };
        Ok(p.completion_code)
    }

    /// Pump the event ring until the in-flight synchronous EP0 transfer reports done, or a
    /// WALL-CLOCK budget is exhausted. Unrelated events are dispatched normally during the wait.
    ///
    /// IVY: this was a raw 2000-ITERATION budget, the last one left on the storage path — and it is
    /// reached by hub-downstream bring-up (`bring_up_hub` → descriptor fetches → the storage
    /// SET_CONFIGURATION), where every control transfer costs extra hops through the hub's TT. An
    /// iteration budget measures YIELDS, not time: how long 2000 of them last depends entirely on
    /// what `hlt()` does (a timer tick on x86/Pi, a busy spin on a timerless core), so the same
    /// number bounds wildly different real intervals. `pump_until_bot_done` moved to a `now_cycles`
    /// deadline for exactly this reason; EP0 and the command pump now follow, so no wait on the path
    /// to a hub-routed block device is denominated in yields. Still strictly bounded — a free-running
    /// counter deadline, never unbounded.
    fn pump_until_ep0_done(&mut self) -> Result<(), ()> {
        let start = crate::arch::now_cycles();
        let budget = crate::arch::hw_wait_budget();
        // BOOTPACE M3: the ~200 µs pre-hlt spin window, hoisted (see `spin_window`). EP0 matters
        // most of the three: enumeration is dozens of control transfers per device, each of which
        // used to cost a full APIC tick no matter how fast the device answered.
        let spin_window = Self::spin_window();
        loop {
            match &self.ep0_pending {
                Some(p) if p.done => return Ok(()),
                None => return Ok(()),
                _ => {}
            }
            if self.drain_event_ring_once() {
                continue;
            }
            // BOOTPACE M3 — spin, then halt. Same shape as the BOT pump: busy-poll for a spec-scale
            // window, and fall through to the unchanged hlt path if nothing arrives.
            if self.spin_for_event(spin_window) {
                continue;
            }
            crate::hlt(); // yield to QEMU so it can DMA the completion into the event ring
            let elapsed = crate::arch::now_cycles().wrapping_sub(start);
            if elapsed >= budget {
                serial_println!("xHCI: EP0 sync pump TIMEOUT after {} cycles (budget {})", elapsed, budget);
                return Err(());
            }
        }
    }

    /// Synchronous xHCI command (ENABLE_SLOT / ADDRESS_DEVICE / CONFIGURE_ENDPOINT): push the TRB
    /// to the command ring, ring the command doorbell, and pump until the completion arrives
    /// (matched by command-TRB address). Returns (completion_code, slot_id). Used for hub bring-up
    /// so downstream devices are enumerated without threading new states through the async FSM.
    fn run_command_sync(&mut self, trb: Trb) -> Result<(u8, u8), ()> {
        if self.cmd_ring_stopped {
            serial_println!("xHCI: run_command_sync refused: command ring stopped (abort in progress).");
            return Err(());
        }
        Self::clean_cmd_input_ctx(&trb);
        let cmd_phys = {
            let mut g = COMMAND_RING.lock();
            let ring = g.as_mut().ok_or(())?;
            let base = ring.get_ptr();
            let idx = ring.push(trb).map_err(|_| ())?;
            base + (idx as u64) * 16
        };
        self.ring_doorbell(0, 0);
        self.cmd_pending = Some(CmdPending { cmd_trb_phys: cmd_phys, done: false, completion_code: 0, slot_id: 0 });
        let pump = self.pump_until_cmd_done();
        let pending = self.cmd_pending.take();
        pump?;
        let p = pending.ok_or(())?;
        Ok((p.completion_code, p.slot_id))
    }

    /// Pump the event ring until the in-flight synchronous COMMAND completes, or a WALL-CLOCK budget
    /// is exhausted. IVY: converted from a 2000-iteration budget for the same reason as
    /// `pump_until_ep0_done` — `run_command_sync` carries hub bring-up's ENABLE_SLOT /
    /// ADDRESS_DEVICE / CONFIGURE_ENDPOINT, i.e. every command a behind-hub block device needs.
    fn pump_until_cmd_done(&mut self) -> Result<(), ()> {
        let start = crate::arch::now_cycles();
        let budget = crate::arch::hw_wait_budget();
        // BOOTPACE M3: the ~200 µs pre-hlt spin window, hoisted (see `spin_window`).
        let spin_window = Self::spin_window();
        loop {
            match &self.cmd_pending {
                Some(p) if p.done => return Ok(()),
                None => return Ok(()),
                _ => {}
            }
            if self.drain_event_ring_once() {
                continue;
            }
            // BOOTPACE M3 — spin, then halt. The command ring is the fastest of the three (no device
            // round trip at all for ENABLE_SLOT), so this is where the tick tax was most absurd.
            if self.spin_for_event(spin_window) {
                continue;
            }
            crate::hlt();
            let elapsed = crate::arch::now_cycles().wrapping_sub(start);
            if elapsed >= budget {
                serial_println!("xHCI: command sync pump TIMEOUT after {} cycles (budget {})", elapsed, budget);
                return Err(());
            }
        }
    }

    /// Main-loop hook: bring up any hubs discovered during enumeration. Synchronous (runs in the
    /// safe polled context, like storage bring-up). Additive: only runs when a hub was detected;
    /// the root-port path is untouched.
    pub fn service_hubs(&mut self) {
        while let Some(hub_slot) = self.hubs_pending.pop() {
            self.bring_up_hub(hub_slot);
        }
        // XENUM-2: drain any downstream-port changes the hubs' Status Change Endpoints flagged.
        self.service_hub_changes();
    }

    /// Main-loop hook: send SET_PROTOCOL(boot) to the HID interfaces of any freshly-enumerated slot,
    /// so a boot-capable device that powered up in REPORT protocol (report IDs + a device-defined
    /// layout the decoders don't parse — e.g. the Logitech receiver's `[reportID, buttons, dx, dy]`)
    /// switches to the fixed BOOT layout the keyboard/mouse decoders expect. Synchronous, like hub
    /// bring-up (safe polled context); the interrupt reads are already armed and keep delivering —
    /// the device just changes report format once this completes.
    pub fn service_hid_setproto(&mut self) {
        // PIUSB-39 F1: drain any halted HID interrupt-IN endpoints first — they are dead until
        // un-halted, and the recovery is synchronous like everything else in this hook. Hooked
        // here so no caller outside the driver changes.
        self.service_hid_halts();
        while let Some(slot) = self.hid_setproto_pending.pop() {
            // Only BOOT interfaces accept SET_PROTOCOL: proto 1 (keyboard) and proto 2 (relative
            // boot mouse). The absolute-pointer path (proto 0, e.g. usb-tablet / consumer-control)
            // is NOT a boot interface and would STALL the request, so skip it.
            let (kbd, kbd_intf, boot_mouse, mouse_intf, port) = {
                let s = &self.slots[slot as usize];
                (s.is_keyboard, s.keyboard_intf, s.is_mouse && s.mouse_is_relative, s.mouse_intf, s.port_id)
            };
            // Skip a device that unplugged between enumeration and now (root PORTSC.CCS=0): sending
            // to a gone device rings a doorbell for a completion that never arrives and burns the
            // EP0 pump budget, stalling the main loop. (Hub-downstream slots carry port_id 0 -> no
            // root PORTSC to consult -> treat as present.)
            if port != 0 && (self.read_portsc(port) & 1) == 0 {
                serial_println!("xHCI: SET_PROTOCOL(boot) skipped for slot {} (device disconnected).", slot);
                continue;
            }
            // All HID interfaces of one device share a single control endpoint (EP0). A STALL on one
            // SET_PROTOCOL halts EP0 (we do not Reset-Endpoint-recover it), so any following request
            // on the same EP0 would just time out — stop after the first failure. Send the boot mouse
            // FIRST: it is the interface that provably needs the boot layout (a boot keyboard already
            // reports boot-style, so losing its SET_PROTOCOL is harmless).
            if boot_mouse && !self.set_hid_boot_protocol(slot, mouse_intf, "boot-mouse") {
                continue;
            }
            if kbd && self.set_hid_boot_protocol(slot, kbd_intf, "keyboard") {
                // HID-KEYS: SET_IDLE(0) on the keyboard interface. Duration 0 = "report only on
                // change" (USB HID 1.11 §7.2.4): a keyboard that powered up with a nonzero idle
                // rate (periodic resends) stops re-sending an unchanged report, so a held key is
                // one press + one release edge rather than a stream of duplicate reports. Bounded
                // and tolerated — some keyboards NAK/STALL it; we witness either way and move on.
                // Only issued after SET_PROTOCOL succeeded (a STALL there halts EP0, so a following
                // request would just time out).
                self.set_hid_idle(slot, kbd_intf);
            }
        }
    }

    /// HID-KEYS: HID class request SET_IDLE(duration=0, reportID=0) for one interface —
    /// bmRequestType 0x21 (host->device, class, interface recipient), bRequest 0x0A,
    /// wValue 0x0000 (duration high byte 0, report id low byte 0), wIndex = interface, no data.
    /// Best-effort: logs a single `[hidkeys] set-idle ok/nak slot=N` witness and never bails the
    /// caller (a NAK/STALL/timeout here is expected on some devices and is harmless — the decoders
    /// still work, just without idle suppression).
    fn set_hid_idle(&mut self, slot: u8, intf: u8) {
        match self.sync_control(slot, 0x21, 0x0A, 0x0000, intf as u16, 0, 0, false) {
            Ok(1) => serial_println!("[hidkeys] set-idle ok slot={} iface={}", slot, intf),
            Ok(code) => serial_println!("[hidkeys] set-idle nak slot={} iface={} (code {})", slot, intf, code),
            Err(()) => serial_println!("[hidkeys] set-idle nak slot={} iface={} (EP0 timeout)", slot, intf),
        }
    }

    /// HID-LED: push this slot's current keyboard lock-LED bitmap to the device via SET_REPORT —
    /// bmRequestType 0x21 (host->device, class, interface recipient), bRequest 0x09 (SET_REPORT),
    /// wValue 0x0200 (report-type Output (0x02) << 8 | report-id 0), wIndex = interface, one data
    /// byte OUT carrying the LED bitmap (bit0 Num, bit1 Caps, bit2 Scroll). The byte is staged in
    /// this slot's descriptor_buffer (idle at report time — enumeration is long done). Best-effort:
    /// logs a single `[hidled] caps=<0|1> set-report <ok|nak> slot=N` witness and tolerates a
    /// NAK/STALL/timeout (some devices lack a settable Output report; the state is still tracked).
    fn set_hid_leds(&mut self, slot: u8, intf: u8) {
        let leds = self.slots[slot as usize].keyboard_leds;
        let caps = (leds >> 1) & 1;
        let buf = self.slots[slot as usize].descriptor_buffer;
        if buf.is_null() {
            serial_println!("[hidled] caps={} set-report nak slot={} (no buffer)", caps, slot);
            return;
        }
        unsafe { core::ptr::write(buf, leds); }
        let buf_phys = buf as u64;
        match self.sync_control(slot, 0x21, 0x09, 0x0200, intf as u16, 1, buf_phys, false) {
            Ok(1) => serial_println!("[hidled] caps={} set-report ok slot={}", caps, slot),
            Ok(code) => serial_println!("[hidled] caps={} set-report nak slot={} (code {})", caps, slot, code),
            Err(()) => serial_println!("[hidled] caps={} set-report nak slot={} (EP0 timeout)", caps, slot),
        }
    }

    /// HID class request SET_PROTOCOL(boot) for one interface: bmRequestType 0x21 (host->device,
    /// class, interface recipient), bRequest 0x0B, wValue 0 (0 = Boot, 1 = Report), wIndex = the
    /// interface number, no data stage. Returns true on success (completion code 1); a STALL/other
    /// code or an EP0 pump timeout returns false (the caller stops touching this device's EP0).
    fn set_hid_boot_protocol(&mut self, slot: u8, intf: u8, what: &str) -> bool {
        match self.sync_control(slot, 0x21, 0x0B, 0x0000, intf as u16, 0, 0, false) {
            Ok(1) => {
                serial_println!("xHCI: SET_PROTOCOL(boot) OK for {} (slot {}, iface {}).", what, slot, intf);
                true
            }
            Ok(code) => {
                serial_println!(
                    "xHCI: SET_PROTOCOL(boot) for {} (slot {}, iface {}) returned code {} (device may lack boot protocol / EP0 halted).",
                    what, slot, intf, code
                );
                false
            }
            Err(()) => {
                serial_println!("xHCI: SET_PROTOCOL(boot) for {} (slot {}, iface {}) FAILED (EP0 pump timeout).", what, slot, intf);
                false
            }
        }
    }

    fn bring_up_hub(&mut self, hub_slot: u8) {
        if hub_slot == 0 || self.slots[hub_slot as usize].ep0_ring.is_none() {
            return;
        }
        serial_println!("xHCI: === HUB BRING-UP (slot {}) ===", hub_slot);
        let buf = self.slots[hub_slot as usize].descriptor_buffer as u64;

        // 1. SET_CONFIGURATION(1) so the hub's ports become controllable.
        //    bmRequestType 0x00 (H2D, standard, device), bRequest 9 (SET_CONFIGURATION).
        match self.sync_control(hub_slot, 0x00, 0x09, 1, 0, 0, 0, false) {
            Ok(1) => {}
            Ok(c) => serial_println!("xHCI: HUB set-config returned code {}", c),
            Err(_) => { serial_println!("xHCI: HUB set-config timed out"); return; }
        }

        // 2. GET_DESCRIPTOR (HUB) -> downstream port count + characteristics.
        //    bmRequestType 0xA0 (D2H, class, device), wValue = type<<8.
        // M3 (XENUM-1): a SuperSpeed hub answers the SuperSpeed Hub Descriptor (bDescriptorType
        // 0x2A), NOT the USB2 Hub Descriptor (0x29). A SS hub asked for 0x29 returns a malformed /
        // zeroed descriptor -> bNbrPorts reads 0 -> every device behind it is stranded (metal rMBP:
        // a USB3 hub on root port 5 read "0 downstream ports, characteristics 0x0903"). Branch on
        // the hub's trained speed from its slot context Speed field (dword0 bits 23:20; SS IDs >= 4).
        let hub_speed = unsafe {
            let oc = self.slots[hub_slot as usize].output_context;
            if oc.is_null() { 0 } else { (*(oc as *const u32) >> 20) & 0xF }
        };
        let is_ss = hub_speed >= 4;
        let hub_desc_type: u16 = if is_ss { 0x2A } else { 0x29 };
        if self.sync_control(hub_slot, 0xA0, 0x06, hub_desc_type << 8, 0, 64, buf, true).is_err() {
            serial_println!("xHCI: HUB descriptor request timed out (type {:#04x})", hub_desc_type);
            return;
        }
        let (nbr_ports, characteristics) = unsafe {
            let p = buf as *const u8;
            (*p.add(2), (*p.add(3) as u16) | ((*p.add(4) as u16) << 8))
        };
        serial_println!("xHCI: HUB slot {} speed {} ({}) desc-type {:#04x}: {} downstream ports (characteristics {:#06x})",
            hub_slot, hub_speed, if is_ss { "SS" } else { "HS/FS" }, hub_desc_type, nbr_ports, characteristics);
        // A hub reporting 0 ports strands every device behind it — treat as a failed bring-up
        // rather than silently marking a 0-port hub (which the downstream walk would no-op over).
        if nbr_ports == 0 {
            serial_println!(
                "xHCI: HUB slot {} reported 0 downstream ports (desc-type {:#04x}, speed {}); bring-up ABORTED.",
                hub_slot, hub_desc_type, hub_speed);
            return;
        }

        let root_hub_port = self.slots[hub_slot as usize].port_id;
        let ttt = ((characteristics >> 5) & 0x3) as u32; // TT Think Time (wHubCharacteristics bits 5-6)
        // This hub's own Route String + tier depth (0 for a hub sitting on a root port). Children
        // extend it: a device on downstream port P gets `hub_route | (P << (4*hub_depth))` at depth
        // hub_depth+1 — 4 bits per tier, so nibble `hub_depth` carries P (see DeviceSlot.route_*).
        let hub_route = self.slots[hub_slot as usize].route_string;
        let hub_depth = self.slots[hub_slot as usize].route_depth;
        // xHCI Route String is 20 bits = 5 nibbles = at most 5 hub tiers. A hub already at depth 5
        // has no nibble left for its children — stop the descent here rather than aliasing tier 1.
        if hub_depth >= 5 {
            serial_println!(
                "xHCI: HUB slot {} at max USB tier depth ({}); not descending further.",
                hub_slot, hub_depth);
            serial_println!("xHCI: === HUB slot {} bring-up complete ===", hub_slot);
            return;
        }

        // 3. Mark the slot as a hub (Hub bit + Number of Ports + TTT) so the controller will route
        //    transactions through it to downstream devices.
        self.set_hub_slot_context(hub_slot, nbr_ports, ttt);

        // 4. Power on every downstream port (SET_FEATURE PORT_POWER = feature 8), then settle.
        for port in 1..=nbr_ports {
            let _ = self.sync_control(hub_slot, 0x23, 0x03, 8, port as u16, 0, 0, false);
        }
        for _ in 0..200 { if !self.drain_event_ring_once() { crate::hlt(); } }

        // 5. For each connected downstream port: reset it, then enumerate the device behind it.
        for port in 1..=nbr_ports {
            if self.sync_control(hub_slot, 0xA3, 0x00, 0, port as u16, 4, buf, true).is_err() {
                continue;
            }
            let pstatus = unsafe {
                let p = buf as *const u8;
                (*p.add(0) as u32) | ((*p.add(1) as u32) << 8)
                    | ((*p.add(2) as u32) << 16) | ((*p.add(3) as u32) << 24)
            };
            if pstatus & 1 == 0 {
                continue; // nothing connected
            }
            serial_println!("xHCI: HUB slot {} port {}: device connected; enumerating...", hub_slot, port);
            if let Some(mut speed) = self.reset_downstream_port(hub_slot, port, buf, is_ss) {
                // ORIN-USB-FIX (R22 sitting-2): a SuperSpeed hub's wPortStatus does NOT carry
                // the USB2 LS/HS speed bits — bit 9 is PORT_POWER on an SS hub, which
                // reset_downstream_port's USB2 decode misread as Low Speed (status 0x100203 ->
                // "xHCI speed 2"). The slot context then carried speed 2 / MPS0 8 for an SS
                // device and ADDRESS_DEVICE failed with code 4 three times — stranding the
                // Orin boot stick behind its 0bda:0489 hub. Devices on an SS hub port are
                // always SuperSpeed: force xHCI speed 4, exactly as the hot-plug path
                // (service_hub_changes) already does. Boot-walk / hot-plug now agree.
                if is_ss {
                    speed = 4;
                    serial_println!(
                        "xHCI: HUB slot {} port {} is a SuperSpeed port (speed forced to SS).",
                        hub_slot, port);
                }
                // Each route-string nibble is 4 bits. Clamp a hub port > 15 to 15 (as Linux does)
                // so it stays a valid downstream-port nibble instead of aliasing onto 0 (= the hub
                // itself) or a sibling. Hubs with > 15 ports are rare; the target VIA hub has ≤ 4,
                // so for it this is identical to the port number.
                let child_route = hub_route | (((port as u32).min(15)) << (4 * hub_depth));
                let child_depth = hub_depth + 1;
                self.enumerate_downstream(hub_slot, port, root_hub_port, child_route, child_depth, speed);
            }
        }

        // XENUM-2: configure + arm the hub's interrupt-IN Status Change Endpoint so a device
        // plugged into a downstream port AFTER this one-shot boot walk is noticed (its change
        // bitmap raises an interrupt-IN completion, serviced by service_hub_changes). Boot-present
        // devices were already enumerated by the walk above; this covers everything after.
        self.configure_hub_interrupt_ep(hub_slot);

        serial_println!("xHCI: === HUB slot {} bring-up complete ===", hub_slot);
    }

    /// Mark a slot as a USB hub in its slot context (Hub bit, Number of Ports, TT Think Time) via
    /// a Configure-Endpoint command updating only the slot context. Required before the controller
    /// will route to the hub's downstream devices.
    fn set_hub_slot_context(&mut self, hub_slot: u8, nbr_ports: u8, ttt: u32) {
        unsafe {
            let input_ctx_virt = self.slots[hub_slot as usize].input_context;
            let output_ctx_virt = self.slots[hub_slot as usize].output_context;
            let base_ptr = input_ctx_virt as *mut u32;
            core::ptr::write_bytes(base_ptr as *mut u8, 0, core::mem::size_of::<InputContext>());
            base_ptr.add(1).write_volatile(1); // Input Control: A0 (slot context) only

            // XHCI-COHERENCE: consumer boundary — invalidate the controller-written output context
            // before copying its slot context out. No-op x86.
            dma_coherency::inval(output_ctx_virt as usize, core::mem::size_of::<DeviceContext>());
            let slot_ctx = base_ptr.add(CTX_WORDS);
            for i in 0..8 {
                slot_ctx.add(i).write_volatile(core::ptr::read_volatile((output_ctx_virt as *const u32).add(i)));
            }
            // DW0 bit 26 = Hub. DW1 bits 24:31 = Number of Ports. DW2 bits 16:17 = TT Think Time.
            slot_ctx.add(0).write_volatile(slot_ctx.add(0).read_volatile() | (1 << 26));
            slot_ctx.add(1).write_volatile((slot_ctx.add(1).read_volatile() & 0x00FF_FFFF) | ((nbr_ports as u32) << 24));
            slot_ctx.add(2).write_volatile((slot_ctx.add(2).read_volatile() & !(0x3 << 16)) | (ttt << 16));
        }
        let trb = Trb {
            parameter: self.slots[hub_slot as usize].input_context as u64,
            status: 0,
            control: (12 << 10) | ((hub_slot as u32) << 24),
        };
        match self.run_command_sync(trb) {
            Ok((1, _)) => {
                // XENUM-2: record the hub identity so the Status Change Endpoint dispatch and the
                // route-scoped disconnect teardown can recognise this slot and size its bitmap.
                self.slots[hub_slot as usize].is_hub = true;
                self.slots[hub_slot as usize].hub_nbr_ports = nbr_ports;
                serial_println!("xHCI: HUB slot {} marked as hub ({} ports)", hub_slot, nbr_ports);
            }
            Ok((c, _)) => serial_println!("xHCI: HUB slot {} configure-endpoint code {}", hub_slot, c),
            Err(_) => serial_println!("xHCI: HUB slot {} configure-endpoint timed out", hub_slot),
        }
    }

    /// Reset a downstream hub port; return the attached device's xHCI speed code (1=FS, 2=LS, 3=HS)
    /// or None if the port did not enable. Uses hub-class port requests (CLEAR/SET_FEATURE,
    /// GET_STATUS).
    fn reset_downstream_port(&mut self, hub_slot: u8, port: u8, buf: u64, is_ss: bool) -> Option<u32> {
        let _ = self.sync_control(hub_slot, 0x23, 0x01, 16, port as u16, 0, 0, false); // CLEAR C_PORT_CONNECTION
        // ORIN-USB-FIX-2: a SuperSpeed hub downstream port must be reset with a WARM (BH) reset, not
        // a USB2-style hot reset. On the Realtek 0bda:0489 SS hub carrying the Orin boot stick, a hot
        // reset (SET_FEATURE PORT_RESET=4) completes at the link layer — the port reads Enabled/U0
        // (wPortStatus 0x0203, so the R22 forced-SS + MPS0-512 slot context is well-formed) — yet the
        // device never reaches an addressable Default state, so ADDRESS_DEVICE's internal SET_ADDRESS
        // draws no handshake and the command completes with code 4 (USB Transaction Error) on every
        // retry (metal Orin: "downstream ADDRESS_DEVICE code 4 ×3"). A warm reset (SET_FEATURE
        // BH_PORT_RESET=28) re-trains the link AND resets the device to Default (USB 3.2 §10.14.2.5,
        // §10.3.1.9) — this is the same reset the ROOT-port SS path already relies on (issue_enum_reset
        // escalates USB3 links to a warm reset, and the SAME stick enumerates on the x86 root port with
        // it). Completion is signalled by C_BH_PORT_RESET (change bit 5 → word bit 21), NOT C_PORT_RESET.
        // USB2 hub ports keep the hot reset: the same physical hub's USB2 half (keyboard/pointer) uses
        // it and enumerates cleanly, and QEMU's downstream devices are USB2 — this branch leaves the
        // HS/FS/LS path byte-identical.
        let (reset_sel, done_bit) = if is_ss { (28u16, 1u32 << 21) } else { (4u16, 1u32 << 20) };
        let _ = self.sync_control(hub_slot, 0x23, 0x03, reset_sel, port as u16, 0, 0, false); // SET (BH_)PORT_RESET

        let mut pstatus = 0u32;
        for _ in 0..50 {
            for _ in 0..20 { if !self.drain_event_ring_once() { crate::hlt(); } }
            if self.sync_control(hub_slot, 0xA3, 0x00, 0, port as u16, 4, buf, true).is_err() {
                return None;
            }
            pstatus = unsafe {
                let p = buf as *const u8;
                (*p.add(0) as u32) | ((*p.add(1) as u32) << 8)
                    | ((*p.add(2) as u32) << 16) | ((*p.add(3) as u32) << 24)
            };
            // Reset complete when the matching change bit latches: C_BH_PORT_RESET (bit 21) for a warm
            // reset, C_PORT_RESET (bit 20) for a hot one. Some SS hubs assert C_PORT_RESET on a warm
            // reset too, so accept either on the SS path rather than spin past a genuine completion.
            if pstatus & done_bit != 0 || (is_ss && pstatus & (1 << 20) != 0) { break; }
        }
        // Deassert every reset-related change this reset latched so the hub's Status Change Endpoint
        // can quiesce later: C_PORT_RESET always; on SS additionally C_BH_PORT_RESET (asserted by the
        // warm reset) and C_PORT_LINK_STATE (the warm reset drives the link U0→Recovery→U0). Leaving
        // C_PORT_LINK_STATE latched storms the SCE — the same class of defect the hot-plug ack path
        // (service_one_hub_change) already guards, mirrored here for the one-shot boot walk.
        let _ = self.sync_control(hub_slot, 0x23, 0x01, 20, port as u16, 0, 0, false); // CLEAR C_PORT_RESET
        if is_ss {
            let _ = self.sync_control(hub_slot, 0x23, 0x01, 29, port as u16, 0, 0, false); // CLEAR C_BH_PORT_RESET
            let _ = self.sync_control(hub_slot, 0x23, 0x01, 25, port as u16, 0, 0, false); // CLEAR C_PORT_LINK_STATE
        }

        if pstatus & (1 << 1) == 0 {
            serial_println!("xHCI: HUB port {} did not enable after reset (status {:#x})", port, pstatus);
            return None;
        }
        // Hub port status speed decode is USB2-only: bit 9 = Low Speed, bit 10 = High Speed, else Full
        // Speed. On an SS hub these bits carry other meaning (bit 9 = PORT_POWER — the R22 mis-decode
        // that read "xHCI speed 2"); the caller forces SS speed 4, so return 4 here to keep the trace
        // honest rather than emit a bogus USB2 speed.
        let speed = if is_ss {
            4
        } else if pstatus & (1 << 9) != 0 { 2u32 } else if pstatus & (1 << 10) != 0 { 3 } else { 1 };
        serial_println!("xHCI: HUB port {} reset OK (status {:#x}, {} reset, xHCI speed {})",
            port, pstatus, if is_ss { "warm" } else { "hot" }, speed);
        Some(speed)
    }

    /// XENUM-2: length in bytes of a hub's Status Change bitmap: bit 0 = the hub itself, bit N = port
    /// N, so `(nbr_ports + 1 + 7) / 8` bytes. Clamped to the change buffer size and to >= 1.
    fn hub_change_bitmap_len(nbr_ports: u8) -> usize {
        (((nbr_ports as usize) + 1 + 7) / 8).clamp(1, 8)
    }

    /// XENUM-2: parse a hub's configuration descriptor (in `buf`) for its single interrupt-IN Status
    /// Change Endpoint. Returns (ep_addr, mps, interval) or None.
    fn parse_hub_int_ep(buf: u64) -> Option<(u8, u16, u8)> {
        unsafe {
            let p = buf as *const u8;
            let total = (((*p.add(2) as usize) | ((*p.add(3) as usize) << 8))).min(64);
            let mut off = 0usize;
            while off + 2 <= total {
                let len = *p.add(off) as usize;
                let dtype = *p.add(off + 1);
                if len == 0 { break; }
                if dtype == 0x05 && off + 7 <= total {
                    let ep_addr = *p.add(off + 2);
                    let attr = *p.add(off + 3);
                    // Interrupt (attr bits 1:0 == 3) IN (address bit 7 set).
                    if (attr & 0x03) == 0x03 && (ep_addr & 0x80) != 0 {
                        let mps = ((*p.add(off + 4) as u16) | ((*p.add(off + 5) as u16) << 8)) & 0x07FF;
                        return Some((ep_addr, mps, *p.add(off + 6)));
                    }
                }
                off += len;
            }
            None
        }
    }

    /// XENUM-2 (M1): configure the hub's interrupt-IN Status Change Endpoint (one Configure-Endpoint,
    /// mirroring the HID endpoint config), then arm the first change-bitmap read. Reads the hub's
    /// configuration descriptor to find the endpoint. The hub slot context (Hub bit + Number of Ports)
    /// was already programmed by `set_hub_slot_context`; this ADDS the endpoint on top, preserving it.
    fn configure_hub_interrupt_ep(&mut self, hub_slot: u8) {
        if hub_slot == 0 || self.slots[hub_slot as usize].ep0_ring.is_none() {
            return;
        }
        let buf = self.slots[hub_slot as usize].descriptor_buffer as u64;
        // GET configuration descriptor (first 64 bytes) to locate the status-change endpoint.
        if self.sync_control(hub_slot, 0x80, 0x06, 0x0200, 0, 64, buf, true).is_err() {
            serial_println!("xHCI: HUB slot {} config-descriptor (for status-change EP) failed", hub_slot);
            return;
        }
        let (ep_addr, mps, interval) = match Self::parse_hub_int_ep(buf) {
            Some(v) => v,
            None => {
                serial_println!("xHCI: HUB slot {} exposes no interrupt-IN status-change endpoint; hot-plug servicing disabled for it.", hub_slot);
                return;
            }
        };
        let dci = (((ep_addr & 0x0F) * 2) + 1) as u32; // interrupt IN

        let input_ctx_virt;
        unsafe {
            let output_ctx_virt = self.slots[hub_slot as usize].output_context;
            let ring = ring::TransferRing::new(16);
            let phys = ring.get_ptr();
            self.slots[hub_slot as usize].hub_int_ring = Some(ring);
            if self.slots[hub_slot as usize].hub_change_buffer.is_none() {
                let l = core::alloc::Layout::from_size_align(64, 64).unwrap();
                self.slots[hub_slot as usize].hub_change_buffer = Some(alloc::alloc::alloc_zeroed(l));
            }
            input_ctx_virt = self.slots[hub_slot as usize].input_context;
            let base_ptr = input_ctx_virt as *mut u32;
            core::ptr::write_bytes(base_ptr as *mut u8, 0, core::mem::size_of::<InputContext>());

            // XHCI-COHERENCE: consumer boundary — invalidate the controller-written output context
            // before reading its slot context (speed) and copying it out. No-op x86.
            dma_coherency::inval(output_ctx_virt as usize, core::mem::size_of::<DeviceContext>());
            // Interval encoding follows the hub's own speed (from its output slot context).
            let out_dw0 = core::ptr::read_volatile((output_ctx_virt as *const u32).add(0));
            let speed = (out_dw0 >> 20) & 0x0F;
            let enc_interval: u32 = if speed == 3 || speed >= 4 {
                (interval.saturating_sub(1)) as u32
            } else if interval > 0 {
                (31 - (interval as u32).leading_zeros()) + 3
            } else { 0 };

            // Input Control: A0 (slot) + A(dci).
            base_ptr.add(1).write_volatile(1 | (1 << dci));
            // Slot context copied from the (already hub-marked) output context, Context Entries raised
            // to this endpoint's DCI.
            let slot_ctx = base_ptr.add(CTX_WORDS);
            for i in 0..8 {
                slot_ctx.add(i).write_volatile(core::ptr::read_volatile((output_ctx_virt as *const u32).add(i)));
            }
            let old_dw0 = slot_ctx.add(0).read_volatile();
            slot_ctx.add(0).write_volatile((old_dw0 & !(0x1F << 27)) | ((dci as u32) << 27));
            // Endpoint context: Interrupt IN (EP Type 7), CErr 3.
            let ep = base_ptr.add((1 + dci as usize) * CTX_WORDS);
            ep.add(0).write_volatile((enc_interval << 16) | ((mps as u32) << 24));
            ep.add(1).write_volatile((7 << 3) | (3 << 1) | ((mps as u32) << 16));
            ep.add(2).write_volatile((phys as u32) | 1);
            ep.add(3).write_volatile((phys >> 32) as u32);
            ep.add(4).write_volatile(mps as u32);
        }
        let trb = Trb {
            parameter: input_ctx_virt as u64,
            status: 0,
            control: (12 << 10) | ((hub_slot as u32) << 24),
        };
        match self.run_command_sync(trb) {
            Ok((1, _)) => {
                self.slots[hub_slot as usize].hub_int_ep = ep_addr;
                self.slots[hub_slot as usize].hub_int_mps = mps;
                serial_println!(
                    "xHCI: HUB slot {} status-change endpoint configured (ep {:#04x} mps {} dci {}); hot-plug armed.",
                    hub_slot, ep_addr, mps, dci);
                self.queue_hub_change_read(hub_slot);
            }
            Ok((c, _)) => serial_println!("xHCI: HUB slot {} status-change Configure-Endpoint code {}", hub_slot, c),
            Err(_) => serial_println!("xHCI: HUB slot {} status-change Configure-Endpoint timed out", hub_slot),
        }
    }

    /// XENUM-2: (re-)arm the hub's interrupt-IN Status Change Endpoint read. Mirrors
    /// `queue_mouse_read`: push a Normal TRB over the change buffer, record its physical address for
    /// the dup-Success guard, ring the endpoint doorbell.
    fn queue_hub_change_read(&mut self, hub_slot: u8) {
        let (ep, buf, nbr_ports) = {
            let s = &self.slots[hub_slot as usize];
            (s.hub_int_ep, s.hub_change_buffer, s.hub_nbr_ports)
        };
        if ep == 0 { return; }
        let Some(buf_ptr) = buf else { return; };
        let dci = ((ep & 0x0F) * 2 + 1) as u32; // interrupt IN
        let read_len = Self::hub_change_bitmap_len(nbr_ports) as u32;
        // XHCI-COHERENCE: evict stale/dirty lines of the change buffer before arming the
        // interrupt-IN read (controller DMA-writes it; completion path invalidates before reading).
        dma_coherency::clean(buf_ptr as usize, read_len as usize);
        let in_trb = Trb {
            parameter: buf_ptr as u64,
            status: read_len,
            control: (1 << 10) | (1 << 5), // Normal | IOC
        };
        let idx = match self.slots[hub_slot as usize].hub_int_ring.as_mut() {
            Some(r) => match r.push(in_trb) { Ok(i) => i, Err(_) => return },
            None => return,
        };
        let ring_base = self.slots[hub_slot as usize].hub_int_ring.as_ref().unwrap().get_ptr();
        self.slots[hub_slot as usize].hub_int_expect_phys =
            ring_base + (idx as u64 * core::mem::size_of::<Trb>() as u64);
        self.ring_doorbell(hub_slot, dci);
    }

    /// XENUM-2: main-loop hook — drain the hub-port changes the Status Change Endpoint flagged.
    /// Deferred while a root port is mid-enumeration (the one-port-at-a-time invariant: never
    /// interleave a downstream ENABLE_SLOT/ADDRESS_DEVICE into the root FSM); bounded per wake so a
    /// flapping port can't starve the main loop. Left-over changes ride the next pass.
    pub fn service_hub_changes(&mut self) {
        if self.hub_changes_pending.is_empty() {
            return;
        }
        if self.enum_active {
            return; // a root port is enumerating — service_hubs retries once it goes idle
        }
        let work = core::mem::take(&mut self.hub_changes_pending);
        for (i, (hub_slot, port)) in work.into_iter().enumerate() {
            if i >= HUB_CHANGE_BUDGET {
                // Storm safety: re-queue the remainder for the next wake instead of blocking here.
                if !self.hub_changes_pending.iter().any(|&e| e == (hub_slot, port)) {
                    self.hub_changes_pending.push((hub_slot, port));
                }
                continue;
            }
            self.service_one_hub_change(hub_slot, port);
        }
    }

    /// XENUM-2 (M1/M2/M3): service one downstream-port change on `hub_slot` port `port`.
    /// GET_PORT_STATUS (M1, always traced); on a connect reset + enumerate the new device through the
    /// existing downstream machinery (M2); on a disconnect tear down the slot subtree route-scoped to
    /// this hub port (M3). Runs only from the main loop (synchronous control transfers).
    fn service_one_hub_change(&mut self, hub_slot: u8, port: u8) {
        if hub_slot == 0
            || !self.slots[hub_slot as usize].is_hub
            || self.slots[hub_slot as usize].ep0_ring.is_none()
        {
            return;
        }
        let buf = self.slots[hub_slot as usize].descriptor_buffer as u64;
        // M1: GET_PORT_STATUS (class request, bmRequestType 0xA3, wIndex = port), 4 bytes.
        if self.sync_control(hub_slot, 0xA3, 0x00, 0, port as u16, 4, buf, true).is_err() {
            // Intentionally returns WITHOUT clearing change features: the hub keeps the change
            // latched and re-raises it on the status-change endpoint, so this self-heals next wake.
            serial_println!("xHCI: HUB slot {} port {} GET_PORT_STATUS failed", hub_slot, port);
            return;
        }
        let (wstatus, wchange) = unsafe {
            let p = buf as *const u8;
            (
                (*p.add(0) as u16) | ((*p.add(1) as u16) << 8),
                (*p.add(2) as u16) | ((*p.add(3) as u16) << 8),
            )
        };
        let hub_speed = unsafe {
            let oc = self.slots[hub_slot as usize].output_context;
            if oc.is_null() { 0 } else { (*(oc as *const u32) >> 20) & 0xF }
        };
        let is_ss = hub_speed >= 4;
        serial_println!(
            "xHCI: HUB slot {} port {} status: wPortStatus={:#06x} wPortChange={:#06x} ({})",
            hub_slot, port, wstatus, wchange, if is_ss { "SS" } else { "HS/FS" });

        let connected = (wstatus & 0x0001) != 0;
        let c_connection = (wchange & 0x0001) != 0;

        if c_connection && connected {
            // M2: a device appeared on this downstream port. Reset it, learn its speed, then
            // enumerate through the existing downstream path with the route extended for this tier.
            let (hub_route, hub_depth, root_hub_port) = {
                let s = &self.slots[hub_slot as usize];
                (s.route_string, s.route_depth, s.port_id)
            };
            if hub_depth >= 5 {
                serial_println!(
                    "xHCI: HUB slot {} port {} connect ignored (hub at max USB tier depth {}).",
                    hub_slot, port, hub_depth);
            } else {
                serial_println!("xHCI: HUB slot {} port {} connect: resetting + enumerating downstream device.", hub_slot, port);
                // reset_downstream_port issues CLEAR C_PORT_CONNECTION + SET PORT_RESET, awaits
                // C_PORT_RESET (bounded/paced), clears it, and reads the trained speed.
                if let Some(mut speed) = self.reset_downstream_port(hub_slot, port, buf, is_ss) {
                    if is_ss {
                        // SS hub ports are always SuperSpeed (the HS/FS speed bits don't apply);
                        // best-effort per XENUM-2 — the metal HS/FS mouse/keyboard path is exact.
                        speed = 4;
                        serial_println!("xHCI: HUB slot {} port {} is a SuperSpeed port (speed forced to SS).", hub_slot, port);
                    }
                    let child_route = hub_route | (((port as u32).min(15)) << (4 * hub_depth));
                    let child_depth = hub_depth + 1;
                    self.enumerate_downstream(hub_slot, port, root_hub_port, child_route, child_depth, speed);
                } else {
                    serial_println!("xHCI: HUB slot {} port {} did not enable after reset; leaving unconfigured.", hub_slot, port);
                }
            }
        } else if c_connection && !connected {
            // M3: the device on this downstream port left. Tear down its slot subtree.
            self.disconnect_hub_port(hub_slot, port);
        } else {
            serial_println!("xHCI: HUB slot {} port {}: no actionable connection change.", hub_slot, port);
        }

        // Deassert every latched change on this port so the Status Change Endpoint can report the
        // next change (a USB hub keeps the change bitmap bit set — and its interrupt-IN re-firing —
        // while any C_* feature is set). Ack the FULL wPortChange word, not just connection: a
        // non-connection change (metal rMBP: SS C_PORT_LINK_STATE=0x0040 from a card reader with no
        // card) left latched storms the endpoint forever. The ClearPortFeature selector for a change
        // bit is NOT `16 + bit` on SuperSpeed hubs — see hub_port_change_feature_selector. The
        // connect path's reset already cleared C_PORT_CONNECTION/C_PORT_RESET; clearing again is a
        // harmless no-op. Reserved bits (no selector) are skipped. Bounded: max 16 acks (one word).
        let mut acked = 0u16;
        for bit in 0..16u16 {
            if (wchange & (1 << bit)) != 0 {
                if let Some(sel) = hub_port_change_feature_selector(bit, is_ss) {
                    let _ = self.sync_control(hub_slot, 0x23, 0x01, sel, port as u16, 0, 0, false);
                    acked |= 1 << bit;
                }
            }
        }
        if wchange != 0 {
            // Witness: prove the full change word was acknowledged (acked mask == set change bits,
            // reserved bits excepted) so the Status Change Endpoint can quiesce. A residual
            // (wchange & !acked & known-selectable) would be the storm signature.
            serial_println!(
                "xHCI: HUB slot {} port {} acked change bits {:#06x} of wPortChange {:#06x} ({}) — Status Change Endpoint quiesced.",
                hub_slot, port, acked, wchange, if is_ss { "SS" } else { "HS/FS" });
        }
        // Nudge the read if a previous arm failed (normally still armed from the event dispatch).
        if self.slots[hub_slot as usize].hub_int_expect_phys == 0 {
            self.queue_hub_change_read(hub_slot);
        }
    }

    /// XENUM-2 (M3): tear down every slot whose route string places it ON or BELOW `hub_slot`'s
    /// downstream `port` — the route-prefix analogue of `dispose_disconnected_slots`' port-scoping. A
    /// nested hub's whole subtree goes with it. Root-port slots and OTHER hub ports are provably
    /// untouched: the match requires the port's full route-nibble prefix, so only this port's subtree
    /// qualifies. Bindings cleared, slots queued for the deferred DISABLE_SLOT drain.
    fn disconnect_hub_port(&mut self, hub_slot: u8, port: u8) {
        let (hub_route, hub_depth, hub_root_port) = {
            let s = &self.slots[hub_slot as usize];
            (s.route_string, s.route_depth, s.port_id)
        };
        // The route prefix of this downstream port: the hub's own route with `port` placed in the
        // hub's tier nibble. A device on/below the port has depth > hub_depth and shares this prefix
        // across the low (hub_depth+1) nibbles.
        let port_nibble = ((port as u32).min(15)) << (4 * hub_depth);
        let child_prefix = hub_route | port_nibble;
        let prefix_mask: u32 = if hub_depth >= 5 {
            0xFFFFF
        } else {
            (1u32 << (4 * (hub_depth + 1))) - 1
        };

        let mut torn = 0usize;
        for i in 1..self.slots.len() {
            if !self.slots[i].active || !self.slots[i].is_downstream {
                continue;
            }
            // On/below this port: SAME physical tree (root port match — the xHCI route string does
            // NOT encode the root port, so two hubs on different root ports both carry route 0 and
            // their children share route values; every slot in one tree shares the hub chain's root
            // port_id, propagated by address_downstream), deeper than the hub, AND the port's full
            // nibble prefix matches.
            let below = self.slots[i].port_id == hub_root_port
                && self.slots[i].route_depth > hub_depth
                && (self.slots[i].route_string & prefix_mask) == (child_prefix & prefix_mask);
            if !below {
                continue;
            }
            // Scope assertion (traced): a matched slot must be in THIS root port's tree, share the
            // prefix, and never be a root slot.
            debug_assert!(self.slots[i].port_id == hub_root_port && self.slots[i].route_depth > hub_depth);
            if self.storage_slot == i as u8 {
                self.storage_slot = 0;
                self.storage_pending_bringup = false;
                self.storage_note = "hub-downstream storage disconnected";
            }
            // USB-UNPLUG: same retraction as the root-port teardown — a disk pulled from a HUB port
            // must leave the block registry too, or the installer keeps listing it. Slot-id matched,
            // so only the slot that actually published geometry is retracted.
            crate::drivers::block::unpublish_usb_geometry(i as u8);
            if self.configuring_slot == i as u8 { self.configuring_slot = 0; }
            if self.ftdi_configuring_slot == i as u8 { self.ftdi_configuring_slot = 0; }
            if self.ftdi_slot == i as u8 {
                self.ftdi_slot = 0;
                self.ftdi_pending_bringup = false;
                self.ftdi_pending = None;
                ftdi::set_live(false);
            }
            self.hid_setproto_pending.retain(|s| *s != i as u8);
            self.hid_halt_pending.retain(|(s, _)| *s != i as u8);
            self.hubs_pending.retain(|s| *s != i as u8);
            // A nested hub going away takes its own queued port changes with it.
            self.hub_changes_pending.retain(|(hs, _)| *hs != i as u8);
            let (route, depth) = (self.slots[i].route_string, self.slots[i].route_depth);
            self.slots[i].reset_soft_state();
            if !self.slots_to_disable.iter().any(|(s, _)| *s == i as u8) {
                self.slots_to_disable.push((i as u8, 0));
            }
            serial_println!(
                "xHCI: HUB slot {} port {} disconnect: slot {} (route {:#x} tier {}) in subtree; queued for DISABLE_SLOT.",
                hub_slot, port, i, route, depth);
            torn += 1;
        }
        serial_println!(
            "xHCI: HUB slot {} port {} disconnect: {} slot(s) torn down (scope: root-port {} route-prefix {:#x} mask {:#x}, root + sibling ports + other trees untouched).",
            hub_slot, port, torn, hub_root_port, child_prefix & prefix_mask, prefix_mask);
    }

    /// Address a device behind a hub: like `address_device` but the slot context carries a
    /// non-zero Route String (accumulated across the hub tiers, not just tier 1), the chain's Root
    /// Hub Port Number, the device Speed, and — for a Low/Full-Speed device behind a High-Speed hub
    /// — the Transaction Translator fields (TT Hub Slot ID / TT Port Number, DW2). `depth` is the
    /// device's tier depth (stored on the slot so a downstream hub can extend the route for its own
    /// children); `tt_hub_slot`/`tt_port` are 0 for HS/SS devices (no TT). Synchronous; returns true
    /// on success.
    fn address_downstream(&mut self, slot_id: u8, root_hub_port: u8, route_string: u32, depth: u8, speed: u32, tt_hub_slot: u8, tt_port: u8) -> bool {
        unsafe {
            let input_layout = core::alloc::Layout::from_size_align(core::mem::size_of::<InputContext>(), 64).unwrap();
            let output_layout = core::alloc::Layout::from_size_align(core::mem::size_of::<DeviceContext>(), 64).unwrap();
            let input_ctx_virt = alloc::alloc::alloc_zeroed(input_layout) as *mut InputContext;
            let output_ctx_virt = alloc::alloc::alloc_zeroed(output_layout) as *mut DeviceContext;
            let ep0_ring = ring::TransferRing::new(16);
            let ep0_ring_phys = ep0_ring.get_ptr();

            // XHCI-COHERENCE: zeroed-handoff — the controller DMA-writes this output context during
            // ADDRESS_DEVICE; clean+invalidate so its zeros reach DRAM and the CPU's read-back is
            // fresh. No-op x86.
            dma_coherency::clean_inval(output_ctx_virt as usize, core::mem::size_of::<DeviceContext>());

            let slot = &mut self.slots[slot_id as usize];
            slot.input_context = input_ctx_virt;
            slot.output_context = output_ctx_virt;
            slot.ep0_ring = Some(ep0_ring);
            slot.port_id = root_hub_port;
            // Downstream devices must be distinguishable from the ROOT device on the same
            // port: their async completions must not advance the root port queue or trip
            // root-enumeration recovery. port_id can't tell them apart — this flag does.
            slot.is_downstream = true;
            slot.active = true;
            // Remember the accumulated route + tier so this slot, if it turns out to be a hub,
            // can extend the route for its own downstream children (see bring_up_hub).
            slot.route_string = route_string;
            slot.route_depth = depth;

            *self.dcbaap.add(slot_id as usize) = output_ctx_virt as u64;
            // XHCI-COHERENCE: producer boundary — clean the DCBAA entry the controller reads to
            // locate this slot's output context. No-op x86.
            dma_coherency::clean(self.dcbaap.add(slot_id as usize) as usize, core::mem::size_of::<u64>());

            let base_ptr = input_ctx_virt as *mut u32;
            core::ptr::write_bytes(base_ptr as *mut u8, 0, core::mem::size_of::<InputContext>());
            base_ptr.add(1).write_volatile(3); // A0 (slot) + A1 (EP0)

            let slot_ctx = base_ptr.add(CTX_WORDS);
            // DW0: Context Entries = 1 | Route String (bits 19:0) | Speed (bits 23:20).
            slot_ctx.add(0).write_volatile((1 << 27) | (route_string & 0xFFFFF) | ((speed & 0xF) << 20));
            // DW1: Root Hub Port Number (bits 23:16) — the root port the hub chain starts at.
            slot_ctx.add(1).write_volatile((root_hub_port as u32) << 16);

            // DW2: Transaction Translator. A Low/Full-Speed device behind a High-Speed hub must
            // name its TT (xHCI 4.3): TT Hub Slot ID (bits 7:0) = the HS hub's slot, TT Port Number
            // (bits 15:8) = that hub's downstream port. HS/SS devices need no TT (fields stay 0), so
            // this write is confined to speed 1 (FS) / 2 (LS) and leaves the currently-working
            // HS-downstream path (the VIA hub's own halves) byte-unchanged. For the common single-
            // level topology the immediate parent hub IS the TT (passed in by the caller); a LS/FS
            // device more than one hub below a HS hub would need the higher HS hub — not handled.
            if speed == 1 || speed == 2 {
                slot_ctx.add(2).write_volatile((tt_hub_slot as u32) | ((tt_port as u32) << 8));
            }

            // EP0 control context. MPS: 8 for Low Speed; 64 otherwise (QEMU is lenient about the
            // Full-Speed initial 8-byte read). A Full-Speed device behind a HS hub whose real
            // bMaxPacketSize0 is 8/16/32 (not the 64 guessed here) would short-read the full
            // descriptor — XENUM-3 M1 learns the real value from the 8-byte header and XENUM-4
            // applies it in place via Evaluate Context (see enumerate_downstream), so the initial
            // guess here need only be good enough to read that 8-byte header.
            // ORIN-USB-FIX: a SuperSpeed device's MPS0 is fixed at 512 by spec (USB3 9.6.6) —
            // 64 in the slot's EP0 context is a Parameter/Transaction error waiting to happen.
            // The root path already programs 512 for SS (speed >= 4); mirror it here now that
            // the SS-hub walk can actually deliver speed-4 children.
            let mps0: u32 = if speed == 2 { 8 } else if speed >= 4 { 512 } else { 64 };
            let ep0_ctx = base_ptr.add(2 * CTX_WORDS);
            ep0_ctx.add(1).write_volatile((4 << 3) | (3 << 1) | (mps0 << 16));
            ep0_ctx.add(2).write_volatile((ep0_ring_phys as u32) | 1);
            ep0_ctx.add(3).write_volatile((ep0_ring_phys >> 32) as u32);
            ep0_ctx.add(4).write_volatile(8);
        }
        // XENUM-3 M2: bounded, paced ADDRESS_DEVICE retry. The root-port path gives a stalled device
        // 200/400/600 ms of settle across retries; a hub-downstream device got ONE try and stranded
        // on the first non-success (metal rMBP: code 17 = Context State Error behind the VIA hub).
        // Re-issue the SAME input context (contexts already built above) after an escalating settle;
        // no port re-reset between attempts — a Context State Error is a controller-side transient,
        // not a link fault, so a cheap settle-and-retry is the right (and lane-contained) recovery.
        let trb = Trb {
            parameter: self.slots[slot_id as usize].input_context as u64,
            status: 0,
            control: (11 << 10) | ((slot_id as u32) << 24),
        };
        for attempt in 1..=XENUM_ADDR_RETRIES {
            match self.run_command_sync(trb) {
                Ok((1, _)) => {
                    if attempt > 1 {
                        serial_println!("xHCI: downstream ADDRESS_DEVICE code 1 (attempt {} of {})",
                            attempt, XENUM_ADDR_RETRIES);
                    }
                    return true;
                }
                Ok((c, _)) => serial_println!("xHCI: downstream ADDRESS_DEVICE code {} (attempt {} of {})",
                    c, attempt, XENUM_ADDR_RETRIES),
                Err(_) => serial_println!("xHCI: downstream ADDRESS_DEVICE timed out (attempt {} of {})",
                    attempt, XENUM_ADDR_RETRIES),
            }
            if attempt < XENUM_ADDR_RETRIES {
                // Escalating paced settle (~200 ms per attempt at the storage-path drain cadence).
                for _ in 0..(200 * attempt) { if !self.drain_event_ring_once() { crate::hlt(); } }
            }
        }
        serial_println!("xHCI: downstream ADDRESS_DEVICE failed after {} attempts", XENUM_ADDR_RETRIES);
        false
    }

    /// XENUM-3 M2: dispose a hub-downstream slot that was ENABLE_SLOT'd (and possibly ADDRESS'd) but
    /// never brought to a usable device — a failed address, a descriptor that never read valid, or a
    /// mid-enumeration bail. Without this the slot stayed `active=true` with its contexts allocated
    /// forever (a leaked active entry, and a stale DCBAA pointer the controller keeps referencing).
    /// Mirrors the root-port recovery clean-up: clear the soft state and queue the deferred
    /// DISABLE_SLOT (the contexts/rings are leaked, not freed, until the controller lets go — the
    /// same use-after-free-DMA guard the root path documents). Never touch the published storage slot.
    ///
    /// PRECONDITION (review fold): every call site fires PRE-configuration — before any HID/MSC/FTDI
    /// personality was bound to the slot. That is why this clears a NARROWER set than
    /// recover_enumeration (no configuring_slot / hid_setproto_pending / ftdi_* / storage_* clears):
    /// none of those can reference a slot that never got past the descriptor read. A future call site
    /// on a post-configuration path must mirror recover_enumeration's fuller clear set instead.
    fn dispose_downstream_slot(&mut self, slot_id: u8) {
        let i = slot_id as usize;
        if i == 0 || i >= self.slots.len() || !self.slots[i].active {
            return;
        }
        if self.storage_slot == slot_id && self.storage_note == "ready" {
            return; // paranoia: never dispose a ready storage slot
        }
        self.hubs_pending.retain(|s| *s != slot_id);
        self.hub_changes_pending.retain(|(hs, _)| *hs != slot_id);
        self.slots[i].reset_soft_state();
        if !self.slots_to_disable.iter().any(|(s, _)| *s == slot_id) {
            self.slots_to_disable.push((slot_id, 0));
        }
        serial_println!("xHCI: downstream slot {} disposed (unenumerated); queued for DISABLE_SLOT.", slot_id);
    }

    /// XENUM-4: update a hub-downstream slot's EP0 Max Packet Size in place with an Evaluate Context
    /// command (TRB type 13, xHCI 4.6.7) — the standard mechanism for correcting MPS0 on a slot that
    /// is already in the Addressed state, mirroring Linux `xhci_check_maxpacket`. This replaces the
    /// XENUM-3 re-ADDRESS strategy, which real Panther Point silicon refuses (code 19, Context State
    /// Error) on an already-Addressed slot. Only the EP0 context is flagged (Add Context A1, A0
    /// clear per 4.6.7 for an MPS-only update); the EP0 context is copied from the live device
    /// (output) context so the EP Type / CErr / TR Dequeue Pointer that ADDRESS_DEVICE established are
    /// preserved and only MPS0 changes. The output context, EP0 ring, DCBAA pointer and slot state
    /// are all left untouched — no fresh allocations, no DCBAA rewrite, no second ADDRESS_DEVICE.
    /// Synchronous; true on completion code 1.
    fn evaluate_downstream_ep0_mps(&mut self, slot_id: u8, mps0: u32) -> bool {
        unsafe {
            let input_ctx_virt = self.slots[slot_id as usize].input_context;
            let output_ctx_virt = self.slots[slot_id as usize].output_context;
            if input_ctx_virt.is_null() || output_ctx_virt.is_null() {
                serial_println!("xHCI: downstream slot {} Evaluate Context skipped (null context); disposing.", slot_id);
                return false;
            }
            let base_ptr = input_ctx_virt as *mut u32;
            core::ptr::write_bytes(base_ptr as *mut u8, 0, core::mem::size_of::<InputContext>());
            base_ptr.add(1).write_volatile(1 << 1); // Add Context: A1 (EP0 context) only; A0 clear.
            // Copy the live EP0 context from the OUTPUT (device) context, then patch MPS0. The output
            // (Device) context has NO Input Control prefix — slot@0, EP0 (DCI 1)@1*CTX_WORDS; the INPUT
            // context DOES, so its EP0 lands at 2*CTX_WORDS. Copying from the output EP0 preserves the
            // EP Type / CErr / TR Dequeue Pointer ADDRESS_DEVICE established; only MPS0 (DW1 bits 31:16)
            // changes. (Copying from 2*CTX_WORDS of the zeroed input would submit an EP-Type=0 / null-ring
            // context that strict silicon rejects.)
            // XHCI-COHERENCE: consumer boundary — the live EP0 context copied out here was
            // DMA-written by the controller at ADDRESS_DEVICE; invalidate before reading. No-op x86.
            dma_coherency::inval(output_ctx_virt as usize, core::mem::size_of::<DeviceContext>());
            let ep0_out = (output_ctx_virt as *const u32).add(CTX_WORDS);
            let ep0_in = base_ptr.add(2 * CTX_WORDS);
            for i in 0..8 {
                ep0_in.add(i).write_volatile(core::ptr::read_volatile(ep0_out.add(i)));
            }
            let dw1 = ep0_in.add(1).read_volatile();
            ep0_in.add(1).write_volatile((dw1 & 0x0000_FFFF) | (mps0 << 16));
        }
        let trb = Trb {
            parameter: self.slots[slot_id as usize].input_context as u64,
            status: 0,
            control: (13 << 10) | ((slot_id as u32) << 24), // TRB type 13 = Evaluate Context
        };
        match self.run_command_sync(trb) {
            Ok((1, _)) => {
                serial_println!("xHCI: downstream slot {} EP0 MPS updated via Evaluate Context ({}).", slot_id, mps0);
                true
            }
            Ok((c, _)) => {
                serial_println!("xHCI: downstream slot {} Evaluate Context code {}; disposing.", slot_id, c);
                false
            }
            Err(_) => {
                serial_println!("xHCI: downstream slot {} Evaluate Context timed out; disposing.", slot_id);
                false
            }
        }
    }

    /// Enumerate one device behind a hub: ENABLE_SLOT, ADDRESS_DEVICE (with the accumulated route
    /// string + tier depth), read the device descriptor, and dispatch by class: a downstream device
    /// that is itself a hub (class 0x09) is queued into `hubs_pending` so the next `service_hubs`
    /// pass brings it up and descends another tier (this is what makes the walk recurse past tier 1);
    /// a Mass-Storage device (interface class 0x08 — the metal rMBP's hubbed SD reader) gets a
    /// synchronous bulk Configure-Endpoint + the deferred SCSI bring-up (service_storage); an HID
    /// keyboard/mouse hands off to the existing endpoint-configuration path. (rmbp's hub-downstream
    /// MSC + jetson's nested-hub descent, reconciled at the seat coalesce.)
    ///
    /// `route_string`/`depth` are this device's accumulated route (from `bring_up_hub`); `hub_slot`
    /// is the immediate parent hub (its slot is the Transaction Translator for a LS/FS child).
    fn enumerate_downstream(&mut self, hub_slot: u8, port: u8, root_hub_port: u8, route_string: u32, depth: u8, speed: u32) {
        // ENABLE_SLOT.
        let slot_id = match self.run_command_sync(Trb { parameter: 0, status: 0, control: 9 << 10 }) {
            Ok((1, sid)) if sid > 0 => sid,
            other => { serial_println!("xHCI: downstream ENABLE_SLOT failed ({:?})", other); return; }
        };

        // A Low/Full-Speed child names its parent hub as the Transaction Translator (single-level
        // topology); HS/SS children pass 0 (address_downstream leaves DW2 clear for them).
        let (tt_hub_slot, tt_port) = if speed == 1 || speed == 2 { (hub_slot, port) } else { (0, 0) };
        // ADDRESS_DEVICE with the full accumulated route string (tier `depth`). M2: bounded paced
        // retry lives inside address_downstream; a final failure disposes the slot (below) rather
        // than leaking an active entry with a live DCBAA pointer.
        if !self.address_downstream(slot_id, root_hub_port, route_string, depth, speed, tt_hub_slot, tt_port) {
            self.dispose_downstream_slot(slot_id as u8);
            return;
        }
        let buf = self.slots[slot_id as usize].descriptor_buffer as u64;

        // XENUM-3 M1: MPS0-learn for a Full/Low-Speed device behind a High-Speed hub. address_downstream
        // guesses bMaxPacketSize0 = 64 for anything but Low Speed, but a Full-Speed device's real MPS0
        // can be 8/16/32 — so a full 18-byte read short-reads (only the 8-byte header arrives, the rest
        // stays zeroed → the metal "vid=0000 / no HID interrupt endpoint" strand). Mirror the root path's
        // MPS0-learn idiom: read the 8-byte header first, and if the device's real MPS0 differs from what
        // we programmed, re-ADDRESS with the learned value before the full read. Confined to the
        // downstream path; skipped for HS/SS (speed 0/3/4), whose MPS0 is unambiguous.
        if speed == 1 || speed == 2 {
            unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 8); }
            if self.sync_control(slot_id, 0x80, 0x06, 0x0100, 0, 8, buf, true).is_ok()
                && self.last_control_len >= 8
            {
                let real_mps0 = unsafe { *(buf as *const u8).add(7) } as u32;
                let programmed: u32 = if speed == 2 { 8 } else { 64 };
                if (real_mps0 == 8 || real_mps0 == 16 || real_mps0 == 32 || real_mps0 == 64)
                    && real_mps0 != programmed
                {
                    serial_println!(
                        "xHCI: downstream slot {} MPS0 learned {} (programmed {}); Evaluate Context.",
                        slot_id, real_mps0, programmed);
                    // XENUM-4: apply the learned MPS0 in place via an Evaluate Context command
                    // (xHCI 4.6.7), NOT a second ADDRESS_DEVICE. Real Panther Point silicon refuses a
                    // BSR=0 re-address on an already-Addressed slot with completion code 19 (Context
                    // State Error) — deterministically (metal 2026-07-16, §7g verdict). Evaluate
                    // Context keeps the existing output context, EP0 ring, DCBAA pointer and slot
                    // state; only the EP0 Max Packet Size changes. On failure, dispose (no retry
                    // storm — a refused Evaluate Context is a new fact to capture, not to blindly
                    // hammer).
                    if !self.evaluate_downstream_ep0_mps(slot_id, real_mps0) {
                        self.dispose_downstream_slot(slot_id as u8);
                        return;
                    }
                }
            }
        }

        // GET device descriptor (18 bytes), with a bounded retry on an all-zero/short read.
        // XENUM-1 M2 + XENUM-3 M1: a device freshly reset behind a hub sometimes answers the FIRST
        // descriptor read with all zeros (bLength=0, vid=pid=0000) — the documented hub-downstream
        // vid=0000 intermittency (metal rMBP: a mouse behind a working, keyboard-bearing hub
        // enumerated class=0 vid=0000 and got "no HID interrupt endpoint"). A read is BAD when: it
        // errored; the ACTUAL transferred length (last_control_len) is short of 18; the structural
        // header is wrong (bLength<18 || type!=0x01); OR the header is structurally valid but the
        // content is zeroed (vid==0 && pid==0) — the exact metal case that slipped the old
        // structure-only gate. Retry a few times with a paced settle (mirroring the storage path); a
        // descriptor that never reads valid is left UNCONFIGURED and the slot DISPOSED (honest).
        let mut desc_ok = false;
        for attempt in 1..=XENUM_DESC_RETRIES {
            unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 18); } // no stale-descriptor false pass
            if self.sync_control(slot_id, 0x80, 0x06, 0x0100, 0, 18, buf, true).is_err() {
                serial_println!("xHCI: downstream slot {} device-descriptor read failed (attempt {} of {})",
                    slot_id, attempt, XENUM_DESC_RETRIES);
            } else {
                let got = self.last_control_len;
                let (blen, dtype, dvid, dpid) = unsafe {
                    let p = buf as *const u8;
                    (*p.add(0), *p.add(1),
                     (*p.add(8) as u16) | ((*p.add(9) as u16) << 8),
                     (*p.add(10) as u16) | ((*p.add(11) as u16) << 8))
                };
                if got >= 18 && blen >= 18 && dtype == 0x01 && !(dvid == 0 && dpid == 0) {
                    desc_ok = true;
                    break;
                }
                serial_println!(
                    "xHCI: downstream slot {} device-descriptor bad read (got {} of 18, bLength={} type={:#x} vid={:04x} pid={:04x}, attempt {} of {}); retrying.",
                    slot_id, got, blen, dtype, dvid, dpid, attempt, XENUM_DESC_RETRIES);
            }
            // Paced settle before the retry: let the device finish its own reset-recovery.
            for _ in 0..200 { if !self.drain_event_ring_once() { crate::hlt(); } }
        }
        if !desc_ok {
            serial_println!(
                "xHCI: downstream slot {} device-descriptor never read valid after {} attempts; leaving unconfigured.",
                slot_id, XENUM_DESC_RETRIES);
            self.dispose_downstream_slot(slot_id as u8);
            return;
        }
        let (class, vid, pid) = unsafe {
            let p = buf as *const u8;
            (*p.add(4), (*p.add(8) as u16) | ((*p.add(9) as u16) << 8), (*p.add(10) as u16) | ((*p.add(11) as u16) << 8))
        };
        serial_println!("xHCI: HUB downstream slot {} device class={:#x} vid={:04x} pid={:04x} (route {:#x} tier {})",
            slot_id, class, vid, pid, route_string, depth);

        // A hub behind this hub (device-level class 0x09): queue it for its own bring-up so the
        // walk descends another tier. bring_up_hub does SET_CONFIGURATION + the hub descriptor +
        // the downstream port walk; its slot already carries the extended route/depth. Mirrors the
        // root-port HUB DETECTED push. No config-descriptor read here — bring_up_hub SET_CONFIGs.
        if class == 0x09 {
            serial_println!("xHCI: >>> HUB-BEHIND-HUB DETECTED (slot {}, tier {}) <<<", slot_id, depth);
            self.hubs_pending.push(slot_id as u8);
            return;
        }

        // GET configuration descriptor (first 64 bytes) and look for an HID interrupt-IN endpoint.
        if self.sync_control(slot_id, 0x80, 0x06, 0x0200, 0, 64, buf, true).is_err() {
            serial_println!("xHCI: downstream slot {} config-descriptor failed", slot_id);
            return;
        }
        // Mass storage first: the metal rMBP's SD reader sits behind a hub and reports class 0
        // at the device level — the interface descriptor is the only place to detect it. This
        // used to be HID-only, leaving a hubbed MSC device `other/unconfigured` forever (the
        // photographed metal failure). One storage device is supported, mirroring the root path.
        if let Some(((in_addr, in_mps), (out_addr, out_mps), msc_intf)) = self.parse_msc_config(buf) {
            serial_println!(
                "xHCI: >>> HUB DOWNSTREAM MASS STORAGE (slot {}, bulk in {:#x}/{} out {:#x}/{}) <<<",
                slot_id, in_addr, in_mps, out_addr, out_mps);
            if self.storage_slot != 0 {
                serial_println!("xHCI: storage slot {} already active; ignoring the hubbed device.", self.storage_slot);
            } else if self.configure_bulk_endpoints_sync(slot_id, in_addr, in_mps, out_addr, out_mps) {
                // BOT error recovery's Bulk-Only Mass Storage Reset targets this interface.
                self.slots[slot_id as usize].storage_intf = msc_intf; // PIUSB-38 reset-recovery wIndex
                // Defer SET_CONFIGURATION + SCSI bring-up to service_storage (same main-loop
                // context, next hook) — identical hand-off to the root path's async completion.
                self.storage_slot = slot_id;
                self.storage_pending_bringup = true;
                self.storage_note = "hub-downstream endpoints configured; SCSI bring-up pending";
                serial_println!("xHCI: Endpoints Configured (Slot {}). Storage ready.", slot_id);
            }
            return;
        }
        // Arm EVERY HID interrupt-IN interface behind the hub via the SAME shared walk the
        // root-port path uses — keyboard AND mouse, so a composite receiver (e.g. a wireless
        // kbd+mouse dongle: keyboard on iface0, mouse on iface1) that lands behind a hub arms
        // both, not just the first interface. Then configure them together in one
        // Configure-Endpoint (root_fsm = false: this is the hub-downstream FSM).
        if self.record_hid_interfaces(slot_id, buf) {
            self.configure_hid_endpoints(slot_id, false);
        } else {
            serial_println!("xHCI: HUB downstream slot {}: no HID interrupt endpoint", slot_id);
        }
    }

    /// Parse a configuration descriptor (in `buf`) for a Mass-Storage-Class interface
    /// (bInterfaceClass 0x08 — matched at ANY subclass/protocol, like the root path) and collect
    /// its bulk IN/OUT endpoint pair. Returns ((in_addr, in_mps), (out_addr, out_mps)) or None.
    /// Also returns the MSC interface's `bInterfaceNumber` — the `wIndex` of the Bulk-Only Mass
    /// Storage Reset (BOT 1.0 §3.1) that BOT error recovery issues.
    fn parse_msc_config(&self, buf: u64) -> Option<((u8, u16), (u8, u16), u8)> {
        unsafe {
            let p = buf as *const u8;
            let total = ((*p.add(2) as usize) | ((*p.add(3) as usize) << 8)).min(64);
            let mut off = 0usize;
            let mut in_msc = false;
            // PIUSB-38: the MSC bInterfaceNumber (descriptor byte +2) — the Bulk-Only Mass Storage
            // Reset `wIndex`. Captured when the class-0x08 interface is seen so reset recovery
            // targets the right interface for a hub-downstream stick.
            let mut msc_intf: u8 = 0;
            let mut bulk_in: Option<(u8, u16)> = None;
            let mut bulk_out: Option<(u8, u16)> = None;
            while off + 2 <= total {
                let len = *p.add(off) as usize;
                let dtype = *p.add(off + 1);
                if len == 0 { break; }
                if dtype == 0x04 && off + 8 <= total {
                    in_msc = *p.add(off + 5) == 0x08;
                    if in_msc { msc_intf = *p.add(off + 2); }
                } else if dtype == 0x05 && in_msc && off + 6 <= total {
                    let ep_addr = *p.add(off + 2);
                    let attr = *p.add(off + 3);
                    if (attr & 0x3) == 0x02 { // Bulk
                        // wMaxPacketSize bits 10:0 (mask off HS mult bits 12:11).
                        let mps = ((*p.add(off + 4) as u16) | ((*p.add(off + 5) as u16) << 8)) & 0x07FF;
                        if (ep_addr & 0x80) != 0 {
                            if bulk_in.is_none() { bulk_in = Some((ep_addr, mps)); }
                        } else if bulk_out.is_none() {
                            bulk_out = Some((ep_addr, mps));
                        }
                    }
                }
                off += len;
            }
            match (bulk_in, bulk_out) {
                (Some(i), Some(o)) => Some((i, o, msc_intf)),
                _ => None,
            }
        }
    }

    /// Walk a config descriptor (`buf`, 64-byte window) and record EVERY HID interrupt-IN
    /// interface onto `slots[slot_id]` with proto-0/1/2 disambiguation (first keyboard wins;
    /// proto-2 boot mouse is the definitive relative pointer and overrides an earlier ambiguous
    /// proto-0 absolute). Returns true iff >=1 interrupt-IN EP was recorded.
    ///
    /// This is the SINGLE HID-arming walk shared by the root-port config-descriptor event path and
    /// `enumerate_downstream` (hub-downstream). Reading the raw descriptor via `buf as *const u8`
    /// clamped to 64 bytes mirrors `parse_msc_config`; the proto disambiguation is lifted verbatim
    /// from the historical root-port inline walk so both paths arm keyboard AND mouse identically.
    /// It writes only the HID slot fields, so it is safe to call while the caller still holds a
    /// separate raw-pointer view of the same descriptor buffer (both are reads of that memory).
    fn record_hid_interfaces(&mut self, slot_id: u8, buf: u64) -> bool {
        let mut current_intf_protocol: u8 = 0;
        let mut current_intf_number: u8 = 0;
        let mut found_hid = false;
        let mut found_hid_ep = false;
        unsafe {
            let p = buf as *const u8;
            let total = (((*p.add(2) as usize) | ((*p.add(3) as usize) << 8))).min(64);
            let mut off = 0usize;
            while off + 2 <= total {
                let len = *p.add(off) as usize;
                let desc_type = *p.add(off + 1);
                if len == 0 { break; }
                if desc_type == 0x04 && off + 8 <= total { // Interface Descriptor
                    // Interface descriptor: number at +2, class at +5, protocol at +7. HID = 0x03;
                    // proto 1 = boot keyboard, proto 2 = boot mouse, proto 0 = ambiguous pointer.
                    current_intf_number = *p.add(off + 2);
                    let current_intf_class = *p.add(off + 5);
                    current_intf_protocol = *p.add(off + 7);
                    found_hid = current_intf_class == 0x03; // HID class
                } else if desc_type == 0x05 && found_hid && off + 7 <= total { // HID Endpoint
                    let ep_addr = *p.add(off + 2);
                    let ep_attr = *p.add(off + 3);
                    if (ep_attr & 0x03) == 0x03 && (ep_addr & 0x80) != 0 { // Interrupt IN
                        let ep_mps = (*p.add(off + 4) as u16) | ((*p.add(off + 5) as u16) << 8);
                        let ep_interval = *p.add(off + 6);

                        // A composite device can expose several HID interrupt-IN interfaces
                        // (keyboard, mouse, and consumer/system-control). The driver handles ONE
                        // keyboard + ONE pointer, so choose carefully — a proto-0 consumer-control
                        // interface must NOT be mistaken for a pointer and clobber the real mouse:
                        //   proto 1 = boot keyboard (record the first),
                        //   proto 2 = boot mouse: the DEFINITIVE pointer — always wins,
                        //   proto 0 = ambiguous (usb-tablet OR consumer-control): accept as an
                        //             absolute pointer ONLY if no pointer yet, so a later proto-0
                        //             can't overwrite, and a real proto-2 mouse still overrides it.
                        let already_kbd = self.slots[slot_id as usize].is_keyboard;
                        let already_ptr = self.slots[slot_id as usize].is_mouse;
                        if current_intf_protocol == 1 {
                            if !already_kbd {
                                serial_println!("xHCI: >>> KEYBOARD INTERRUPT IN EP FOUND: {:#x}, MPS: {}, Interval: {} <<<", ep_addr, ep_mps, ep_interval);
                                self.slots[slot_id as usize].keyboard_ep = ep_addr;
                                self.slots[slot_id as usize].keyboard_mps = ep_mps;
                                self.slots[slot_id as usize].keyboard_interval = ep_interval;
                                self.slots[slot_id as usize].keyboard_intf = current_intf_number;
                                self.slots[slot_id as usize].is_keyboard = true;
                                found_hid_ep = true;
                            } else {
                                serial_println!("xHCI: (ignoring extra keyboard HID interface, ep {:#x})", ep_addr);
                            }
                        } else if current_intf_protocol == 2 {
                            // Boot mouse: the real pointer. Record it, overriding any earlier
                            // ambiguous proto-0 pointer on the same device.
                            serial_println!("xHCI: >>> POINTER INTERRUPT IN EP FOUND: {:#x}, MPS: {}, Interval: {}, RELATIVE boot-mouse (proto 2) <<<", ep_addr, ep_mps, ep_interval);
                            self.slots[slot_id as usize].mouse_ep = ep_addr;
                            self.slots[slot_id as usize].mouse_mps = ep_mps;
                            self.slots[slot_id as usize].mouse_interval = ep_interval;
                            self.slots[slot_id as usize].mouse_intf = current_intf_number;
                            self.slots[slot_id as usize].is_mouse = true;
                            self.slots[slot_id as usize].mouse_is_relative = true;
                            found_hid_ep = true;
                        } else if !already_ptr {
                            // Protocol 0, no pointer yet: treat as an absolute pointer (usb-tablet).
                            // If this is actually a consumer-control interface, decoding it as
                            // absolute is the known proto-0 limitation — but it can't clobber a
                            // real mouse.
                            serial_println!("xHCI: >>> POINTER INTERRUPT IN EP FOUND: {:#x}, MPS: {}, Interval: {}, ABSOLUTE tablet (proto {}) <<<",
                                ep_addr, ep_mps, ep_interval, current_intf_protocol);
                            self.slots[slot_id as usize].mouse_ep = ep_addr;
                            self.slots[slot_id as usize].mouse_mps = ep_mps;
                            self.slots[slot_id as usize].mouse_interval = ep_interval;
                            self.slots[slot_id as usize].mouse_intf = current_intf_number;
                            self.slots[slot_id as usize].is_mouse = true;
                            self.slots[slot_id as usize].mouse_is_relative = false;
                            found_hid_ep = true;
                        } else {
                            serial_println!("xHCI: (ignoring extra non-pointer HID interface, proto {}, ep {:#x})", current_intf_protocol, ep_addr);
                        }
                        found_hid = false; // one interrupt-IN EP per interface; next iface re-arms
                    }
                }
                off += len;
            }
        }
        found_hid_ep
    }

    pub unsafe fn scan_ports(&mut self) {
        // 1. GET MAX PORTS
        // HCSPARAMS1 is at Capability Base + 0x04
        let hcsparams1_ptr = (self.base_addr + 0x04) as *const u32;
        let hcsparams1 = core::ptr::read_volatile(hcsparams1_ptr);
        let max_ports = (hcsparams1 >> 24) & 0xFF; // Top 8 bits

        serial_println!("xHCI: Scanning {} Ports...", max_ports);

        // 2. ITERATE PORTS
        for i in 0..max_ports {
            let port_id = (i + 1) as u8;
            let port_csc = self.read_portsc(port_id);

            // Check CCS (Current Connect Status) - Bit 0
            if (port_csc & 1) != 0 {
                serial_println!("xHCI: [PORT {}] DEVICE DETECTED! (Status: {:#x})", port_id, port_csc);

                // 3. RESET PORT (The Handshake)
                // Write 1 to PR (Port Reset) - Bit 4
                // We use Read-Modify-Write to preserve other bits (like PP).
                let reset_cmd = port_csc | (1 << 4);
                self.write_portsc(port_id, reset_cmd);

                serial_println!("xHCI: [PORT {}] Reset Signal Sent. Waiting for Enable...", port_id);
            }
        }
    }

    /// Push a TRB onto the slot's EP0 ring and return its physical address (0 on failure) —
    /// so callers can record which Status TRB's completion the FSM should accept.
    fn push_ep0(&mut self, slot_id: u8, trb: Trb) -> u64 {
        unsafe {
            if let Some(ep0_ring) = &mut self.slots[slot_id as usize].ep0_ring {
                let base = ep0_ring.get_ptr();
                match ep0_ring.push(trb) {
                    Ok(idx) => base + (idx as u64) * 16,
                    Err(_) => 0,
                }
            } else {
                serial_println!("xHCI: push_ep0 failed, no ep0_ring for slot {}", slot_id);
                0
            }
        }
    }

    /// Begin the root device-descriptor read after ADDRESS_DEVICE. Normally this is a straight
    /// `request_device_descriptor`, but on Tegra a Full-Speed device (addressed at MPS0=8, real
    /// MPS0 unknown) is first routed through the `fs-mps-learn` stage (JB10): the main loop reads
    /// 8 bytes, patches MPS0 via Evaluate Context, then reads the full descriptor — avoiding the
    /// babble + tear-down that leaves the FS device silent on this firmware. Non-FS / non-tegra
    /// paths are unchanged and byte-identical.
    fn begin_device_descriptor(&mut self, slot_id: u8) {
        #[cfg(feature = "tegra")]
        {
            let port = self.slots[slot_id as usize].port_id;
            let speed = (self.read_portsc(port) >> 10) & 0xF;
            // speed 1 = Full Speed. Skip if a prior babble already learned MPS0=64 for this port
            // (address_device then programmed 64, so the full read won't babble) — go straight
            // through, which also prevents a learn/retry loop.
            if JB10_FS_EVAL_CTX && speed == 1 && !self.fs_ep0_mps64[(port as usize) & 31] {
                serial_println!(
                    "xHCI: [tegra fs-mps] slot {} (FS port {}): learning MPS0 before full descriptor.",
                    slot_id, port);
                self.enum_cmd_phys = 0;
                self.set_enum_stage("fs-mps-learn");
                return;
            }
        }
        self.request_device_descriptor(slot_id);
    }

    /// JB10 (Tegra): learn a Full-Speed device's real MPS0 and patch EP0 in place, then read the
    /// full descriptor — all on `slot_id`, no teardown. Runs from `service_enum` (main-loop
    /// context, where `sync_control`/`run_command_sync` are safe), never from `poll_events`. On any
    /// failure it falls back to the shared babble→recover path (sets `fs_ep0_mps64` so the
    /// re-address uses MPS0=64). `port` is the root port (for the `fs_ep0_mps64` index).
    #[cfg(feature = "tegra")]
    fn fs_learn_mps0(&mut self, slot_id: u8, port: u8) {
        let buf = self.slots[slot_id as usize].descriptor_buffer as u64;
        // Phase 1: read the first 8 descriptor bytes at MPS0=8 — a single packet, no babble.
        if self.sync_control(slot_id, 0x80, 0x06, 0x0100, 0, 8, buf, true).is_err() {
            serial_println!(
                "xHCI: [tegra fs-mps] slot {} 8-byte dev-desc failed; falling back to babble-recover.",
                slot_id);
            self.fs_ep0_mps64[(port as usize) & 31] = true;
            self.recover_enumeration("fs-mps-8byte-failed", 0);
            return;
        }
        let mps0 = unsafe { *(buf as *const u8).add(7) }; // bMaxPacketSize0
        serial_println!("xHCI: [tegra fs-mps] slot {} bMaxPacketSize0 = {}", slot_id, mps0);
        // Phase 2: if it exceeds the guessed 8, patch EP0 MPS0 in place via Evaluate Context.
        // Only the legal FS values are accepted; anything else falls back rather than program junk.
        if mps0 > 8 {
            if mps0 != 16 && mps0 != 32 && mps0 != 64 {
                serial_println!(
                    "xHCI: [tegra fs-mps] slot {} illegal bMaxPacketSize0 {}; falling back.",
                    slot_id, mps0);
                self.fs_ep0_mps64[(port as usize) & 31] = true;
                self.recover_enumeration("fs-mps-illegal", 0);
                return;
            }
            if !self.evaluate_ep0_mps(slot_id, mps0 as u32) {
                self.fs_ep0_mps64[(port as usize) & 31] = true;
                self.recover_enumeration("fs-mps-eval-failed", 0);
                return;
            }
        }
        // Phase 3: full 18-byte descriptor read on the same slot — rejoins the normal FSM
        // (its completion drives the class dispatch exactly as usual).
        self.request_device_descriptor(slot_id);
    }

    /// JB10 (Tegra): patch a slot's EP0 Max Packet Size in place with an Evaluate Context command
    /// (TRB type 13), mirroring Linux `xhci_check_maxpacket`. Only the EP0 context is flagged (Add
    /// Context A1); the EP0 context is copied from the device (output) context and its MPS field
    /// (Slot/EP context DW1 bits 31:16) replaced. Synchronous; true on completion code 1.
    #[cfg(feature = "tegra")]
    fn evaluate_ep0_mps(&mut self, slot_id: u8, mps0: u32) -> bool {
        unsafe {
            let input_ctx_virt = self.slots[slot_id as usize].input_context;
            let output_ctx_virt = self.slots[slot_id as usize].output_context;
            if input_ctx_virt.is_null() || output_ctx_virt.is_null() {
                return false;
            }
            let base_ptr = input_ctx_virt as *mut u32;
            core::ptr::write_bytes(base_ptr as *mut u8, 0, core::mem::size_of::<InputContext>());
            base_ptr.add(1).write_volatile(1 << 1); // Add Context: A1 (EP0 context) only
            // Copy the current EP0 context from the output (device) context, then patch MPS0.
            // Layout differs between the two: the INPUT context has an Input Control Context at
            // offset 0, so its EP0 (DCI 1) is at 2*CTX_WORDS; the OUTPUT (Device) context has NO
            // control prefix — slot@0, EP0 (DCI 1)@1*CTX_WORDS. Reading the output EP0 preserves
            // the live EP Type / CErr / TR Dequeue Pointer that ADDRESS_DEVICE established; only
            // MPS0 changes. (Copying from 2*CTX_WORDS here would grab the zeroed EP1 region and
            // submit an EP-Type=0 / null-ring context the strict Tegra FW rejects with code 17.)
            // XHCI-COHERENCE: consumer boundary — the live EP0 context copied out here was
            // DMA-written by the controller at ADDRESS_DEVICE; invalidate before reading. No-op x86.
            dma_coherency::inval(output_ctx_virt as usize, core::mem::size_of::<DeviceContext>());
            let ep0_out = (output_ctx_virt as *const u32).add(CTX_WORDS);
            let ep0_in = base_ptr.add(2 * CTX_WORDS);
            for i in 0..8 {
                ep0_in.add(i).write_volatile(core::ptr::read_volatile(ep0_out.add(i)));
            }
            let dw1 = ep0_in.add(1).read_volatile();
            ep0_in.add(1).write_volatile((dw1 & 0x0000_FFFF) | (mps0 << 16));
        }
        let trb = Trb {
            parameter: self.slots[slot_id as usize].input_context as u64,
            status: 0,
            control: (13 << 10) | ((slot_id as u32) << 24), // TRB type 13 = Evaluate Context
        };
        match self.run_command_sync(trb) {
            Ok((1, _)) => {
                serial_println!("xHCI: [tegra fs-mps] slot {} EP0 MPS0 -> {} (Evaluate Context OK)", slot_id, mps0);
                true
            }
            Ok((c, _)) => { serial_println!("xHCI: [tegra fs-mps] slot {} Evaluate Context code {}", slot_id, c); false }
            Err(_) => { serial_println!("xHCI: [tegra fs-mps] slot {} Evaluate Context timed out", slot_id); false }
        }
    }

    pub fn request_device_descriptor(&mut self, slot_id: u8) {
        serial_println!("xHCI: Requesting Device Descriptor for Slot {}...", slot_id);
        // Root-FSM-only caller (the hub path reads descriptors via sync_control). An EP0
        // transfer, not a command — clear the command tracking, note the stage.
        self.enum_cmd_phys = 0;
        self.set_enum_stage("dev-desc");

        let desc_phys = self.slots[slot_id as usize].descriptor_buffer as u64;
        if desc_phys == 0 {
            serial_println!("xHCI: CRITICAL ERROR - Descriptor Buffer Phys Addr is 0!");
            return;
        }
        // XHCI-COHERENCE: evict any dirty/stale lines of the (reused) descriptor buffer before the
        // controller DMA-writes the descriptor into it; the async completion parse invalidates before
        // reading. No-op x86.
        dma_coherency::clean(desc_phys as usize, 18);

        // 1. Setup Stage
        // 0x80 06 00 01 00 00 12 00
        // Little Endian u64: 0x0012000001000680
        let setup_trb = Trb {
            parameter: 0x0012000001000680,
            status: 8, // Transfer Length (Always 8 for Setup)
            control: (2 << 10) // Type 2 (Setup Stage)
                   | (1 << 6)  // IDT (Immediate Data)
                   | (3 << 16), // TRT (3 = IN Data Stage)
        };
        self.push_ep0(slot_id, setup_trb);

        // 2. Data Stage
        let data_trb = Trb {
            parameter: desc_phys,
            status: 18, // Length 18 bytes
            control: (3 << 10) // Type 3 (Data Stage)
                   | (1 << 16), // DIR (1 = IN)
        };
        self.push_ep0(slot_id, data_trb);

        // 3. Status Stage
        let status_trb = Trb {
            parameter: 0,
            status: 0,
            control: (4 << 10) // Type 4 (Status Stage)
                   | (1 << 5)  // IOC (Interrupt On Completion)
                   | (0 << 16), // DIR (0 = OUT)
        };
        let status_phys = self.push_ep0(slot_id, status_trb);
        self.slots[slot_id as usize].ep0_expect_phys = status_phys;

        // 4. Ring Doorbell (Slot 1, Target 1 for EP0)
        self.ring_doorbell(slot_id, 1);
    }

    pub fn request_configuration_descriptor(&mut self, slot_id: u8) {
        serial_println!("xHCI: Requesting Configuration Descriptor for Slot {}...", slot_id);
        // Root-FSM-only caller (see request_device_descriptor).
        self.enum_cmd_phys = 0;
        self.set_enum_stage("cfg-desc");

        let desc_phys = self.slots[slot_id as usize].descriptor_buffer as u64;
        if desc_phys == 0 {
            serial_println!("xHCI: CRITICAL ERROR - Descriptor Buffer Phys Addr is 0!");
            return;
        }
        // XHCI-COHERENCE: evict stale lines of the reused descriptor buffer before the controller
        // DMA-writes the config descriptor (parse invalidates before reading). No-op x86.
        dma_coherency::clean(desc_phys as usize, 64);

        // 1. Setup Stage
        // bmRequestType = 0x80 (Device to Host, Standard, Device)
        // bRequest = 0x06 (GET_DESCRIPTOR)
        // wValue = 0x0200 (Descriptor Type = 2 for Configuration, Index = 0)
        // wIndex = 0x0000
        // wLength = 0x0040 (64 bytes)
        // Little Endian u64: 0x0040000002000680
        let setup_trb = Trb {
            parameter: 0x0040000002000680,
            status: 8, // Transfer Length
            control: (2 << 10) | (1 << 6) | (3 << 16), // Type 2 | IDT | TRT (IN)
        };
        self.push_ep0(slot_id, setup_trb);

        // 2. Data Stage
        let data_trb = Trb {
            parameter: desc_phys,
            status: 64, // Length 64 bytes
            control: (3 << 10) | (1 << 16), // Type 3 | DIR (IN)
        };
        self.push_ep0(slot_id, data_trb);

        // 3. Status Stage
        let status_trb = Trb {
            parameter: 0,
            status: 0,
            control: (4 << 10) | (1 << 5) | (0 << 16), // Type 4 | IOC | DIR (OUT)
        };
        let status_phys = self.push_ep0(slot_id, status_trb);
        self.slots[slot_id as usize].ep0_expect_phys = status_phys;

        // 4. Ring Doorbell
        self.ring_doorbell(slot_id, 1);
    }

    /// Configure ALL of a device's HID interrupt-IN endpoints (keyboard and/or pointer) in ONE
    /// Configure-Endpoint command, reading their addresses/MPS/interval from the slot fields recorded
    /// during enumeration. Then the state machine issues one device-level SET_CONFIGURATION and arms a
    /// read on each (see the command/transfer completion handlers).
    ///
    /// Why a single command: a composite HID device — most wireless kbd+mouse dongles, and real
    /// keyboards with a second consumer-control interface — has more than one HID interface. Issuing a
    /// separate Configure-Endpoint per interface fails, because each rebuilds the shared input context
    /// naming only its own endpoint (and sets Context Entries to only its own DCI), so the second
    /// commits over the first and the earlier endpoint goes unconfigured. Mirroring the mass-storage
    /// bulk-IN+OUT path, we add every endpoint to one input context and set Context Entries to the
    /// highest DCI. The keyboard reads into `data_buffer`, the pointer into its own `mouse_data_buffer`,
    /// so two live endpoints never DMA into the same buffer.
    /// `root_fsm` says which caller this is: true from the root enumeration FSM's config-
    /// descriptor parse (track the command, advance the port queue on skip), false from the
    /// hub bring-up path (whose downstream slots must not touch the root FSM's tracking —
    /// note they share the hub's ROOT port number in port_id, so only the call site knows).
    pub fn configure_hid_endpoints(&mut self, slot_id: u8, root_fsm: bool) {
        let (has_kbd, has_mouse, mouse_rel) = {
            let s = &self.slots[slot_id as usize];
            (
                s.is_keyboard && s.keyboard_ep != 0,
                s.is_mouse && s.mouse_ep != 0,
                s.mouse_is_relative,
            )
        };
        if !has_kbd && !has_mouse {
            serial_println!("xHCI: configure_hid_endpoints(slot {}): no HID endpoints; skipping.", slot_id);
            if root_fsm {
                self.start_next_port();
            }
            return;
        }

        let input_ctx_virt;
        let max_dci;
        unsafe {
            let slot = &mut self.slots[slot_id as usize];
            input_ctx_virt = slot.input_context;
            let output_ctx_virt = slot.output_context;
            let base_ptr = input_ctx_virt as *mut u32;

            // XHCI-COHERENCE: consumer boundary — invalidate the controller-written output context
            // before reading its speed and copying the slot context out below. No-op x86.
            dma_coherency::inval(output_ctx_virt as usize, core::mem::size_of::<DeviceContext>());
            // Speed (from the output slot context) governs the interval encoding for both endpoints.
            let out_dw0 = core::ptr::read_volatile((output_ctx_virt as *const u32).add(0));
            let speed = (out_dw0 >> 20) & 0x0F;
            let encode_interval = |interval: u8| -> u32 {
                if speed == 3 || speed >= 4 {
                    (interval.saturating_sub(1)) as u32 // HS / SS: bInterval - 1
                } else if interval > 0 {
                    (31 - (interval as u32).leading_zeros()) + 3 // LS / FS: floor(log2(bInterval)) + 3
                } else {
                    0
                }
            };

            // Clear the whole input context, then add the slot context (A0) + each endpoint.
            core::ptr::write_bytes(base_ptr as *mut u8, 0, core::mem::size_of::<InputContext>());
            let mut add_flags: u32 = 1; // A0 = slot context
            let mut mdci: u32 = 0;

            // Helper writes one Interrupt-IN endpoint context at its DCI.
            // (Inlined per-endpoint below; kept as a closure would need &mut base_ptr aliasing care.)

            if has_kbd {
                let ep_addr = slot.keyboard_ep;
                let mps = slot.keyboard_mps as u32;
                let interval = slot.keyboard_interval;
                let dci = (((ep_addr & 0x0F) * 2) + if (ep_addr & 0x80) != 0 { 1 } else { 0 }) as u32;
                let ring = ring::TransferRing::new(16);
                let phys = ring.get_ptr();
                slot.keyboard_ring = Some(ring);
                if slot.data_buffer.is_none() {
                    let l = core::alloc::Layout::from_size_align(512, 64).unwrap();
                    slot.data_buffer = Some(alloc::alloc::alloc_zeroed(l));
                }
                let ep = base_ptr.add((1 + dci as usize) * CTX_WORDS);
                ep.add(0).write_volatile((encode_interval(interval) << 16) | (mps << 24));
                ep.add(1).write_volatile((7 << 3) | (3 << 1) | (mps << 16)); // EP Type 7 (Interrupt IN), CErr 3
                ep.add(2).write_volatile((phys as u32) | 1);
                ep.add(3).write_volatile((phys >> 32) as u32);
                ep.add(4).write_volatile(mps);
                add_flags |= 1 << dci;
                mdci = mdci.max(dci);
                slot.keyboard_state = 1;
            }

            if has_mouse {
                let ep_addr = slot.mouse_ep;
                let mps = slot.mouse_mps as u32;
                let interval = slot.mouse_interval;
                let dci = (((ep_addr & 0x0F) * 2) + if (ep_addr & 0x80) != 0 { 1 } else { 0 }) as u32;
                let ring = ring::TransferRing::new(16);
                let phys = ring.get_ptr();
                slot.mouse_ring = Some(ring);
                if slot.mouse_data_buffer.is_none() {
                    let l = core::alloc::Layout::from_size_align(512, 64).unwrap();
                    slot.mouse_data_buffer = Some(alloc::alloc::alloc_zeroed(l));
                }
                let ep = base_ptr.add((1 + dci as usize) * CTX_WORDS);
                ep.add(0).write_volatile((encode_interval(interval) << 16) | (mps << 24));
                ep.add(1).write_volatile((7 << 3) | (3 << 1) | (mps << 16)); // EP Type 7 (Interrupt IN), CErr 3
                ep.add(2).write_volatile((phys as u32) | 1);
                ep.add(3).write_volatile((phys >> 32) as u32);
                ep.add(4).write_volatile(mps);
                add_flags |= 1 << dci;
                mdci = mdci.max(dci);
                slot.mouse_state = 1;
            }

            // Input Control Context (A-flags), then the slot context copied from the output context
            // with Context Entries updated to the highest DCI in use.
            base_ptr.add(1).write_volatile(add_flags);
            let slot_ctx_ptr = base_ptr.add(CTX_WORDS);
            for i in 0..8 {
                let val = core::ptr::read_volatile((output_ctx_virt as *const u32).add(i));
                slot_ctx_ptr.add(i).write_volatile(val);
            }
            let old_dw0 = slot_ctx_ptr.add(0).read_volatile();
            slot_ctx_ptr.add(0).write_volatile((old_dw0 & !(0x1F << 27)) | (mdci << 27));
            max_dci = mdci;
        }

        serial_println!(
            "xHCI: Configuring HID Endpoints for Slot {} ({}{}{}) in one Configure-Endpoint (max DCI {}).",
            slot_id,
            if has_kbd { "keyboard" } else { "" },
            if has_kbd && has_mouse { "+" } else { "" },
            if has_mouse { if mouse_rel { "mouse(rel)" } else { "pointer(abs)" } } else { "" },
            max_dci
        );

        let trb = Trb {
            parameter: input_ctx_virt as u64,
            status: 0,
            control: (12 << 10) | ((slot_id as u32) << 24),
        };
        match self.send_command(trb) {
            Ok(phys) => {
                if root_fsm {
                    self.track_enum_cmd(phys, "configure-eps");
                }
            }
            Err(e) => {
                serial_println!("xHCI: Failed to send HID Configure Endpoint command: {}", e);
                if root_fsm {
                    self.recover_enumeration("command-send-failed", 0);
                }
            }
        }
    }

    /// Async SET_CONFIGURATION on EP0. Used for both root and hub-downstream HID devices;
    /// the caller (the command dispatch) sets the enum stage when this is the root FSM's.
    pub fn send_set_configuration(&mut self, slot_id: u8, config_val: u8) {
        unsafe {
            serial_println!("xHCI: Sending SET_CONFIGURATION({}) to Slot {}", config_val, slot_id);
            let setup_trb = Trb {
                parameter: 0x0000000000000900 | ((config_val as u64) << 16), // bmRequestType=0, bRequest=9 (SET_CONFIGURATION), wValue=config_val
                status: 8, // Length 8
                control: (2 << 10) | (0 << 16) | (1 << 6), // Type 2 (Setup Stage), TRT=0 (No Data Stage), IDT=1
            };
            let s_param = setup_trb.parameter;
            let s_status = setup_trb.status;
            let s_ctrl = setup_trb.control;
            xdbg!("xHCI: Setup TRB -> Param: {:#x}, Status: {:#x}, Control: {:#x}", s_param, s_status, s_ctrl);
            self.push_ep0(slot_id, setup_trb);

            let status_trb = Trb {
                parameter: 0,
                status: 0,
                control: (4 << 10) | (1 << 5) | (1 << 16), // Type 4 (Status Stage), IOC=1, DIR=1 (IN)
            };
            let st_param = status_trb.parameter;
            let st_status = status_trb.status;
            let st_ctrl = status_trb.control;
            xdbg!("xHCI: Status TRB -> Param: {:#x}, Status: {:#x}, Control: {:#x}", st_param, st_status, st_ctrl);
            let status_phys = self.push_ep0(slot_id, status_trb);
            self.slots[slot_id as usize].ep0_expect_phys = status_phys;

            self.ring_doorbell(slot_id, 1);
        }
    }

    /// PIUSB-39 witness — one bounded line naming which population moved:
    /// `[piusb39] mouse rearm=<n> discarded=<n> errrearm=<n> (<tag>)`. `tag` is `poll` (a normal
    /// armed read), `guard` (the dup-Success guard discarded a completion and re-armed anyway) or
    /// `halt` (a halted endpoint was un-halted and re-armed). Split counters because the three are
    /// different populations: only `discarded` proves the guard's pipeline-preserving exit fired.
    /// Knob-gated (`usbdebug`) and rate-limited — the pointer is the highest-traffic endpoint and
    /// a self-sustaining error class would otherwise flood the FTDI at report rate.
    #[allow(unused_variables)]
    fn piusb39_witness(tag: &str) {
        #[cfg(feature = "usbdebug")]
        {
            static LAST_MS: AtomicU64 = AtomicU64::new(0);
            let now = crate::arch::ticks();
            // First line always prints (LAST_MS == 0); after that, at most one per 250 ms.
            let last = LAST_MS.load(Ordering::Relaxed);
            if last != 0 && now.wrapping_sub(last) < 250 { return; }
            LAST_MS.store(now.max(1), Ordering::Relaxed);
            serial_println!(
                "[piusb39] mouse rearm={} discarded={} errrearm={} ({})",
                MOUSE_REARM_COUNT.load(Ordering::Relaxed),
                MOUSE_DISCARD_REARM_COUNT.load(Ordering::Relaxed),
                MOUSE_ERROR_REARM_COUNT.load(Ordering::Relaxed),
                tag);
        }
    }

    /// PIUSB-39 F3: rate-limited trace for a HID interrupt-IN error completion. Unconditional
    /// (not knob-gated — a halted pointer is a real fault worth one line on any boot) but capped
    /// at one line per 500 ms across both endpoints, because a non-halting error class repeats at
    /// the endpoint's poll rate and would otherwise saturate the serial link.
    fn hid_error_witness(what: &str, slot_id: u32, code: u32, halting: bool) {
        static LAST_MS: AtomicU64 = AtomicU64::new(0);
        static SUPPRESSED: AtomicU64 = AtomicU64::new(0);
        let now = crate::arch::ticks();
        let last = LAST_MS.load(Ordering::Relaxed);
        if last != 0 && now.wrapping_sub(last) < 500 {
            SUPPRESSED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        LAST_MS.store(now.max(1), Ordering::Relaxed);
        let dropped = SUPPRESSED.swap(0, Ordering::Relaxed);
        serial_println!(
            "xHCI: {} interrupt-IN error (slot {}, code {}); {} [+{} suppressed]",
            what, slot_id, code,
            if halting { "endpoint HALTED, queued for un-halt recovery" } else { "re-arming" },
            dropped);
    }

    /// Main-loop hook (PIUSB-39 F1): un-halt and re-arm any HID interrupt-IN endpoint that took a
    /// halting error completion. A Halted endpoint ignores the doorbell, so the plain re-queue the
    /// event dispatch does for non-halting codes would be a silent no-op here — the pointer would
    /// stay dead, which is the P54b-class hole for a stalling mouse. The sequence is the same pair
    /// the bulk path uses (`resync_bulk_ep`), generalised over any DCI: **Reset
    /// Endpoint** (TRB 14: Halted -> Stopped, clears the host sequence/toggle), **Set TR Dequeue
    /// Pointer** (TRB 16: past the faulted TRB), then the device-side
    /// `CLEAR_FEATURE(ENDPOINT_HALT)`, and finally the read is armed again. Synchronous, so it runs
    /// here in the safe polled context rather than inside the event dispatch that noticed the error.
    pub fn service_hid_halts(&mut self) {
        while let Some((slot, is_mouse)) = self.hid_halt_pending.pop() {
            if (slot as usize) >= self.slots.len() { continue; }
            let (ep_addr, port, deq) = {
                let s = &self.slots[slot as usize];
                let ep = if is_mouse { s.mouse_ep } else { s.keyboard_ep };
                let ring = if is_mouse { s.mouse_ring.as_ref() } else { s.keyboard_ring.as_ref() };
                (ep, s.port_id, ring.map(|r| r.dequeue_reset_target()))
            };
            if ep_addr == 0 { continue; }
            // A device that unplugged between the error and now: recovery would ring a doorbell
            // for a completion that never arrives and burn the EP0 pump budget (same guard as
            // `service_hid_setproto`). Hub-downstream slots carry port_id 0 -> treat as present.
            if port != 0 && (self.read_portsc(port) & 1) == 0 {
                serial_println!("xHCI: HID un-halt skipped for slot {} (device disconnected).", slot);
                continue;
            }
            let dci: u32 = ((ep_addr as u32) & 0x0F) * 2 + if (ep_addr & 0x80) != 0 { 1 } else { 0 };
            serial_println!(
                "xHCI: [piusb39] un-halting {} interrupt-IN slot {} ep {:#04x} (dci {})",
                if is_mouse { "pointer" } else { "keyboard" }, slot, ep_addr, dci);

            // 1) Reset Endpoint: Halted -> Stopped.
            let reset_trb = Trb { parameter: 0, status: 0,
                control: (14 << 10) | (dci << 16) | ((slot as u32) << 24) };
            match self.run_command_sync(reset_trb) {
                Ok((1, _)) => {}
                other => serial_println!("xHCI: [piusb39] Reset Endpoint unexpected {:?}", other),
            }
            // 2) Set TR Dequeue Pointer to the ring's current enqueue slot (past the faulted TRB).
            if let Some((phys, dcs)) = deq {
                let deq_trb = Trb { parameter: phys | (dcs as u64), status: 0,
                    control: (16 << 10) | (dci << 16) | ((slot as u32) << 24) };
                match self.run_command_sync(deq_trb) {
                    Ok((1, _)) => {}
                    other => serial_println!("xHCI: [piusb39] Set TR Dequeue unexpected {:?}", other),
                }
            }
            // 3) Device-side CLEAR_FEATURE(ENDPOINT_HALT); wIndex = full endpoint address.
            match self.sync_control(slot, 0x02, 0x01, 0x0000, ep_addr as u16, 0, 0, false) {
                Ok(1) => {}
                other => serial_println!("xHCI: [piusb39] CLEAR_FEATURE(HALT) unexpected {:?}", other),
            }
            // 4) The host ring dequeue moved; the old expectation is stale. Clear it so the first
            //    completion after recovery is accepted, then arm the read.
            {
                let s = &mut self.slots[slot as usize];
                if is_mouse { s.mouse_expect_phys = 0; s.mouse_prev_phys = 0; }
                else { s.keyboard_expect_phys = 0; s.keyboard_prev_phys = 0; }
            }
            let armable = {
                let s = &self.slots[slot as usize];
                if is_mouse { s.mouse_data_buffer.is_some() && s.mouse_ring.is_some() }
                else { s.data_buffer.is_some() && s.keyboard_ring.is_some() }
            };
            if armable {
                if is_mouse {
                    MOUSE_ERROR_REARM_COUNT.fetch_add(1, Ordering::Relaxed);
                    self.queue_mouse_read(slot);
                    Self::piusb39_witness("halt");
                } else {
                    self.queue_keyboard_read(slot);
                }
            }
        }
    }

    pub fn queue_mouse_read(&mut self, slot_id: u8) {
        unsafe {
            let ep_num = self.slots[slot_id as usize].mouse_ep & 0x0F;
            let dir_in = (self.slots[slot_id as usize].mouse_ep & 0x80) != 0;
            let dci = (ep_num * 2) + if dir_in { 1 } else { 0 };

            let data_phys = self.slots[slot_id as usize].mouse_data_buffer.unwrap() as u64;
            // XHCI-COHERENCE: evict any stale/dirty lines of the report buffer before arming the
            // interrupt-IN read (the controller DMA-writes it; the completion path invalidates before
            // decoding). No-op x86.
            dma_coherency::clean(data_phys as usize, self.slots[slot_id as usize].mouse_mps as usize);

            let in_trb = Trb {
                parameter: data_phys,
                status: self.slots[slot_id as usize].mouse_mps as u32, // Length
                control: (1 << 10) | (1 << 5), // Type 1 | IOC
            };
            let idx = self.slots[slot_id as usize].mouse_ring.as_mut().unwrap().push(in_trb).unwrap();
            // Record the physical address of the Normal TRB we just armed so the transfer
            // dispatch can match a real completion against it and reject a Panther-Point
            // dup-Success for the already-consumed TD (see `mouse_expect_phys`).
            let ring_base = self.slots[slot_id as usize].mouse_ring.as_ref().unwrap().get_ptr();
            // PIUSB-39: remember the TD we are retiring before overwriting the expectation — a
            // genuine Panther-Point dup-Success names THAT address, and only that address may be
            // discarded without a re-arm.
            self.slots[slot_id as usize].mouse_prev_phys =
                self.slots[slot_id as usize].mouse_expect_phys;
            self.slots[slot_id as usize].mouse_expect_phys =
                ring_base + (idx as u64 * core::mem::size_of::<Trb>() as u64);
            let rearms = MOUSE_REARM_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            // PIUSB-39 witness — knob-gated (usbdebug) and BOUNDED (first arm + every 256th), so a
            // metal capture can read the pointer pipeline's liveness without flooding the FTDI.
            if rearms == 1 || rearms % 256 == 0 {
                Self::piusb39_witness("poll");
            }
            self.ring_doorbell(slot_id, dci as u32);
            xdbg!("xHCI: Mouse Read Queued.");
        }
    }

    pub fn queue_keyboard_read(&mut self, slot_id: u8) {
        unsafe {
            let ep_num = self.slots[slot_id as usize].keyboard_ep & 0x0F;
            let dir_in = (self.slots[slot_id as usize].keyboard_ep & 0x80) != 0;
            let dci = (ep_num * 2) + if dir_in { 1 } else { 0 };

            let data_phys = self.slots[slot_id as usize].data_buffer.unwrap() as u64;
            // XHCI-COHERENCE: evict stale/dirty lines of the report buffer before arming the
            // interrupt-IN read (controller DMA-writes it; completion path invalidates before
            // decoding). No-op x86.
            dma_coherency::clean(data_phys as usize, self.slots[slot_id as usize].keyboard_mps as usize);

            let in_trb = Trb {
                parameter: data_phys,
                status: self.slots[slot_id as usize].keyboard_mps as u32,
                control: (1 << 10) | (1 << 5), // Type 1 (Normal) | IOC
            };
            let idx = self.slots[slot_id as usize].keyboard_ring.as_mut().unwrap().push(in_trb).unwrap();
            // Record the physical address of the Normal TRB we just armed so the transfer dispatch
            // can match a real completion against it and reject a Panther-Point dup-Success for the
            // already-consumed TD (see `keyboard_expect_phys`). Mirrors `queue_mouse_read`.
            let ring_base = self.slots[slot_id as usize].keyboard_ring.as_ref().unwrap().get_ptr();
            // PIUSB-39: mirror of `queue_mouse_read` — remember the TD being retired.
            self.slots[slot_id as usize].keyboard_prev_phys =
                self.slots[slot_id as usize].keyboard_expect_phys;
            self.slots[slot_id as usize].keyboard_expect_phys =
                ring_base + (idx as u64 * core::mem::size_of::<Trb>() as u64);
            self.ring_doorbell(slot_id, dci as u32);
            xdbg!("xHCI: Keyboard Read Queued.");
        }
    }
}
