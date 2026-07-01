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

use ring::TransferRing;
use self::trb::Trb;
use self::event::{EventRing, ErstEntry, ErstTable};
use self::context::{InputContext, DeviceContext};
use spin::Mutex;
use alloc::vec::Vec;

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
const HID_SCANCODE_TO_ASCII: [(u8, u8); 104] = [
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
    (0, 0), // 0x4F: Right Arrow
    (0, 0), // 0x50: Left Arrow
    (0, 0), // 0x51: Down Arrow
    (0, 0), // 0x52: Up Arrow
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

        // Reset Controller
        let cmd = core::ptr::read_volatile(usbcmd_ptr);
        core::ptr::write_volatile(usbcmd_ptr, cmd | 2);

        let _ = wait_until(
            || (core::ptr::read_volatile(usbcmd_ptr) & 2) == 0,
            hw_wait_budget(), "USBCMD.HCRST=0 (reset)");

        // Wait for Controller Not Ready (CNR) to clear
        let _ = wait_until(
            || (core::ptr::read_volatile(usbsts_ptr) & (1 << 11)) == 0,
            hw_wait_budget(), "USBSTS.CNR=0");
        serial_println!("xHCI: Controller Reset Complete.");
    }

    serial_println!("[XHCI] CONTROLLER RESET.");
}

pub static XHCI_CONTROLLER: spin::Mutex<Option<XhciController>> = spin::Mutex::new(None);

pub static COMMAND_RING: Mutex<Option<TransferRing>> = Mutex::new(None);
pub static EVENT_RING: Mutex<Option<EventRing>> = Mutex::new(None);

pub static mut ERST_TABLE: ErstTable = ErstTable { entries: [ErstEntry { ring_address: 0, size: 0, _rsvd: 0, _rsvd2: 0 }] };

// Store Physical Address of the Event Ring for Runtime ERDP updates
static mut EVENT_RING_PHYS_BASE: u64 = 0;

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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
    core::ptr::write_volatile(doorbell_addr as *mut u32, target);
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
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

pub struct DeviceSlot {
    pub active: bool,
    pub port_id: u8,
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
    pub mouse_state: u8,
    pub mouse_ring: Option<TransferRing>,

    pub is_keyboard: bool,
    pub keyboard_ep: u8,
    pub keyboard_mps: u16,
    pub keyboard_interval: u8,
    pub keyboard_state: u8,
    pub keyboard_ring: Option<TransferRing>,

    pub descriptor_buffer: *mut u8,

    // Dedicated DMA buffers for Bulk-Only Transport (mass storage). Kept separate from
    // descriptor_buffer / data_buffer so a CBW can't clobber descriptors or HID reports.
    pub cbw_buffer: Option<*mut u8>,       // 31-byte Command Block Wrapper
    pub csw_buffer: Option<*mut u8>,       // 13-byte Command Status Wrapper
    pub scsi_data_buffer: Option<*mut u8>, // data-stage buffer (>= one block)
    pub bulk_in_ep: u8,                    // bulk IN endpoint address (e.g. 0x81)
    pub bulk_out_ep: u8,                   // bulk OUT endpoint address (e.g. 0x02)
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
            mouse_state: 0,
            mouse_ring: None,
            is_keyboard: false,
            keyboard_ep: 0,
            keyboard_mps: 0,
            keyboard_interval: 0,
            keyboard_state: 0,
            keyboard_ring: None,
            descriptor_buffer: desc_buffer,
            cbw_buffer: None,
            csw_buffer: None,
            scsi_data_buffer: None,
            bulk_in_ep: 0,
            bulk_out_ep: 0,
        }
    }
}

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
    /// Monotonic CBW tag.
    pub bot_tag: u32,
    /// In-flight BOT transaction, populated by the event handler.
    bot_pending: Option<BotPending>,

    /// In-flight synchronous EP0 control transfer (hub bring-up). See `Ep0Pending`.
    ep0_pending: Option<Ep0Pending>,
    /// In-flight synchronous command (hub bring-up). See `CmdPending`.
    cmd_pending: Option<CmdPending>,
    /// Slots of hubs detected during enumeration, awaiting (main-loop) bring-up.
    hubs_pending: Vec<u8>,
    /// True while a port is mid-enumeration. Enumeration is serialized (one port at a time);
    /// this lets a hot-plug Connect Status Change event know whether to kick `start_next_port`
    /// immediately or just queue the port (the in-flight device's completion will drain it).
    enum_active: bool,
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
            bot_tag: 1,
            bot_pending: None,
            ep0_pending: None,
            cmd_pending: None,
            hubs_pending: Vec::new(),
            enum_active: false,
        }
    }



    pub fn send_noop_command(&mut self) -> Result<usize, &'static str> {
        COMMAND_RING.lock().as_mut().unwrap().push_noop()
    }

    pub fn send_command(&mut self, trb: Trb) -> Result<usize, &'static str> {
        let res = COMMAND_RING.lock().as_mut().unwrap().push(trb);
        if res.is_ok() {
            // Ring the Doorbell for the Host Controller (Slot 0)
            // Target 0 = Command Ring
            self.ring_doorbell(0, 0);
        }
        res
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

    /// Safely clear one or more PORTSC change bits (all RW1C). PORTSC has dangerous
    /// write-1 semantics: bit 1 (PED) is write-1-to-DISABLE and bit 4 (PR) is
    /// write-1-to-RESET. A naive `read | change_bit` write-back can therefore disable
    /// or reset the port if those bits read back as 1. This masks off PED, PR, and all
    /// RW1C change bits, then writes 1 only to the requested change bit(s).
    fn clear_port_change(&self, port_id: u8, change_bits: u32) {
        const ALL_CHANGE: u32 =
            (1 << 17) | (1 << 18) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);
        let portsc = self.read_portsc(port_id);
        let preserved = portsc & !(ALL_CHANGE | (1 << 1) | (1 << 4));
        self.write_portsc(port_id, preserved | (change_bits & ALL_CHANGE));
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
            // Bit 3 (EHB) is write-1-to-clear.
            core::ptr::write_volatile((ir0_base + 0x18) as *mut u64, new_dequeue_ptr | 8);
            xdbg!("xHCI: ERDP Advanced to {:#x}", new_dequeue_ptr);
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

                        // Completion Code 1 = Success
                        if completion_code == 1 {
                            serial_println!("xHCI: >>> COMMAND SUCCESS <<<");
                            if slot_id > 0 {
                                serial_println!("xHCI: SLOT ID ALLOCATED: {}", slot_id);

                                // UNA-18-ADDRESS: If we have a pending port ID, this is likely the result of Enable Slot.
                                // Proceed to Address Device.
                                if let Some(port_to_map) = self.pending_ports.pop() {
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
                                    // Storage setup is done; move on to the next connected port.
                                    self.start_next_port();
                                }
                                else if self.slots[slot_id as usize].mouse_state == 1
                                    || self.slots[slot_id as usize].keyboard_state == 1 {
                                    // The single Configure-Endpoint that programmed this device's HID
                                    // interface(s) completed. Advance whichever endpoints it covered,
                                    // then issue ONE device-level SET_CONFIGURATION for the whole device.
                                    serial_println!("xHCI: HID Endpoints Configured (Slot {}). Proceeding to Set Configuration...", slot_id);
                                    if self.slots[slot_id as usize].keyboard_state == 1 {
                                        self.slots[slot_id as usize].keyboard_state = 2;
                                    }
                                    if self.slots[slot_id as usize].mouse_state == 1 {
                                        self.slots[slot_id as usize].mouse_state = 2;
                                    }
                                    self.send_set_configuration(slot_id as u8, 1);
                                }
                                else {
                                    // UNA-19-IDENTITY: If pending_ports is empty, we assume Address Device just finished.
                                    serial_println!("xHCI: >>> SLOT {} ENABLED & ADDRESSED <<<", slot_id);
                                    self.request_device_descriptor(slot_id as u8);
                                }
                            }
                        } else {
                            serial_println!("xHCI: >>> COMMAND FAILED (Code {}) <<<", completion_code);
                            // UNA-19-HALT: Stop on Code 5
                            if completion_code == 5 {
                                serial_println!("xHCI: CRITICAL FAILURE: TRB ERROR (CODE 5).");
                            }
                        }
                    },
                    34 => { // PORT STATUS CHANGE EVENT
                        let port_id = ((param >> 24) & 0xFF) as u8;
                        serial_println!("xHCI: [Event] Port Status Change. Port={}", port_id);

                        let port_sc = self.read_portsc(port_id);

                        // PRC (Port Reset Change, bit 21): a reset we initiated has
                        // completed. Acknowledge it and, if the port is now enabled,
                        // request a device slot.
                        if (port_sc & (1 << 21)) != 0 {
                            serial_println!("xHCI: [Port {}] Reset Complete. Clearing PRC...", port_id);
                            self.clear_port_change(port_id, 1 << 21);

                            // Bit 1: PED (Port Enabled).
                            if (port_sc & (1 << 1)) != 0 {
                                serial_println!("xHCI: [Port {}] is ENABLED. Requesting Slot...", port_id);
                                self.enable_slot(port_id);
                            }
                        }

                        // CSC (Connect Status Change, bit 17): acknowledge, then drive HOT-PLUG.
                        // At boot, connected ports are caught by the initial scan; this handles a
                        // device plugged in AFTER boot (the common case on real hardware, where QEMU
                        // never exercised it). We queue the port and let the serialized
                        // start_next_port() reset/enumerate it — only kicking it here if no
                        // enumeration is already in flight (else the in-flight device's completion
                        // drains the queue), so the one-at-a-time invariant holds.
                        if (port_sc & (1 << 17)) != 0 {
                            self.clear_port_change(port_id, 1 << 17);
                            // Bit 0: CCS (Current Connect Status) — 1 = a device is now attached.
                            if (port_sc & 1) != 0 {
                                serial_println!("xHCI: [Port {}] device connected (hot-plug); queuing for enumeration.", port_id);
                                if !self.ports_to_enumerate.contains(&port_id) {
                                    self.ports_to_enumerate.push(port_id);
                                }
                                if !self.enum_active {
                                    self.start_next_port();
                                }
                            } else {
                                serial_println!("xHCI: [Port {}] device disconnected.", port_id);
                            }
                        }
                    },
                    32 => { // TRANSFER EVENT
                        let transfer_len = status & 0xFFFFFF;
                        let completion_code = (status >> 24) & 0xFF;
                        let slot_id = (control >> 24) & 0xFF; // Slot ID is in Control Bits 31:24
                        let endpoint_id = (control >> 16) & 0x1F; // Endpoint ID in Control Bits 16:20

                        xdbg!("xHCI DEBUG: [Transfer Event] Slot={}, EP={}, Code={}, Len={}",
                            slot_id, endpoint_id, completion_code, transfer_len);

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
                                }
                            }
                        }

                        // UNA-19-REVEAL: If success or short packet, check buffer
                        if completion_code == 1 || completion_code == 13 {
                            // UNA-21-DEBUG: Force Transition based on Endpoint ID
                            // EP1 = Control (Device Descriptor)
                            // EP3 = Bulk IN (SCSI Read)

                            if endpoint_id == 1 && slot_id > 0 { // EP0 (Control) -> Device Descriptor
                                if self.slots[slot_id as usize].mouse_state == 2
                                    || self.slots[slot_id as usize].keyboard_state == 2 {
                                    // One device-level SET_CONFIGURATION covered every HID interface; arm
                                    // a read on each endpoint that was configured (keyboard into
                                    // data_buffer, pointer into mouse_data_buffer — separate buffers so a
                                    // composite device's two endpoints don't race), then advance ports.
                                    serial_println!("xHCI: >>> HID SET_CONFIGURATION COMPLETE <<<");
                                    if self.slots[slot_id as usize].keyboard_state == 2 {
                                        self.slots[slot_id as usize].keyboard_state = 3;
                                        self.queue_keyboard_read(slot_id as u8);
                                    }
                                    if self.slots[slot_id as usize].mouse_state == 2 {
                                        self.slots[slot_id as usize].mouse_state = 3;
                                        self.queue_mouse_read(slot_id as u8);
                                    }
                                    self.start_next_port();
                                } else {
                                    serial_println!("xHCI: >>> INTERCEPTED DESCRIPTOR EVENT (Slot 1 EP 1) <<<");
                                    unsafe {
                                        let desc_buf = self.slots[slot_id as usize].descriptor_buffer;
                                        let desc_data = core::slice::from_raw_parts(desc_buf, 256);
                                        let vid = (desc_data[8] as u16) | ((desc_data[9] as u16) << 8);
                                    let pid = (desc_data[10] as u16) | ((desc_data[11] as u16) << 8);

                                    serial_println!(">>> SYSTEM ALERT: NEW HARDWARE DETECTED <<<");
                                    serial_println!(">>> [CONTACT ESTABLISHED] SLOT {}", slot_id);
                                    serial_println!(">>> VENDOR ID : [{:04x}]", vid);
                                    serial_println!(">>> PRODUCT ID: [{:04x}]", pid);

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
                                        
                                        // Track current interface state while parsing
                                        let mut current_intf_class: u8 = 0;
                                        let mut current_intf_protocol: u8 = 0;
                                        let mut found_hid = false;
                                        // COLLECT every HID interrupt-IN endpoint (recording it into the
                                        // slot), then configure them all together after the walk via one
                                        // Configure-Endpoint (see `configure_hid_endpoints`). A composite
                                        // device — most wireless kbd+mouse dongles, and real keyboards with
                                        // a second consumer-control interface — has more than one HID
                                        // interface; the earlier "first interface only" workaround left the
                                        // pointer (interface 1) unconfigured, so the mouse never worked.
                                        let mut found_hid_ep = false;
                                        // Mass-storage tracking: collect the bulk IN/OUT
                                        // endpoints during the walk, configure once after.
                                        let mut is_mass_storage = false;
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

                                                found_hid = current_intf_class == 0x03; // HID class

                                                if current_intf_class == 0x08 {
                                                    // Mass Storage interface (SCSI Bulk-Only, 0x08/0x06/0x50).
                                                    // This device reports class 0 at the device level, so the
                                                    // interface descriptor is the only place to detect it. We
                                                    // collect its bulk endpoints below and configure after the walk.
                                                    serial_println!("xHCI: >>> MASS STORAGE INTERFACE DETECTED (Class 0x08) <<<");
                                                    is_mass_storage = true;
                                                }
                                            } else if desc_type == 0x05 && found_hid { // HID Endpoint
                                                if offset + 6 >= 256 { break; }
                                                let ep_addr = desc_data[offset + 2];
                                                let ep_attr = desc_data[offset + 3];
                                                if (ep_attr & 0x03) == 0x03 && (ep_addr & 0x80) != 0 { // Interrupt IN
                                                    let ep_mps = (desc_data[offset + 4] as u16) | ((desc_data[offset + 5] as u16) << 8);
                                                    let ep_interval = desc_data[offset + 6];

                                                    // A composite device can expose several HID interrupt-IN
                                                    // interfaces (keyboard, mouse, and consumer/system-control).
                                                    // The driver handles ONE keyboard + ONE pointer, so choose
                                                    // carefully — a proto-0 consumer-control interface must NOT be
                                                    // mistaken for a pointer and clobber the real mouse:
                                                    //   proto 1 = boot keyboard (record the first),
                                                    //   proto 2 = boot mouse: the DEFINITIVE pointer — always wins,
                                                    //   proto 0 = ambiguous (usb-tablet OR consumer-control): accept
                                                    //             as an absolute pointer ONLY if no pointer yet, so a
                                                    //             later proto-0 can't overwrite, and a real proto-2
                                                    //             mouse (below) still overrides it.
                                                    let already_kbd = self.slots[slot_id as usize].is_keyboard;
                                                    let already_ptr = self.slots[slot_id as usize].is_mouse;
                                                    if current_intf_protocol == 1 {
                                                        if !already_kbd {
                                                            serial_println!("xHCI: >>> KEYBOARD INTERRUPT IN EP FOUND: {:#x}, MPS: {}, Interval: {} <<<", ep_addr, ep_mps, ep_interval);
                                                            self.slots[slot_id as usize].keyboard_ep = ep_addr;
                                                            self.slots[slot_id as usize].keyboard_mps = ep_mps;
                                                            self.slots[slot_id as usize].keyboard_interval = ep_interval;
                                                            self.slots[slot_id as usize].is_keyboard = true;
                                                            found_hid_ep = true;
                                                        } else {
                                                            serial_println!("xHCI: (ignoring extra keyboard HID interface, ep {:#x})", ep_addr);
                                                        }
                                                    } else if current_intf_protocol == 2 {
                                                        // Boot mouse: the real pointer. Record it, overriding any
                                                        // earlier ambiguous proto-0 pointer on the same device.
                                                        serial_println!("xHCI: >>> POINTER INTERRUPT IN EP FOUND: {:#x}, MPS: {}, Interval: {}, RELATIVE boot-mouse (proto 2) <<<", ep_addr, ep_mps, ep_interval);
                                                        self.slots[slot_id as usize].mouse_ep = ep_addr;
                                                        self.slots[slot_id as usize].mouse_mps = ep_mps;
                                                        self.slots[slot_id as usize].mouse_interval = ep_interval;
                                                        self.slots[slot_id as usize].is_mouse = true;
                                                        self.slots[slot_id as usize].mouse_is_relative = true;
                                                        found_hid_ep = true;
                                                    } else if !already_ptr {
                                                        // Protocol 0, no pointer yet: treat as an absolute pointer
                                                        // (usb-tablet). If this is actually a consumer-control
                                                        // interface, decoding it as absolute is the known proto-0
                                                        // limitation — but at least it can't clobber a real mouse.
                                                        serial_println!("xHCI: >>> POINTER INTERRUPT IN EP FOUND: {:#x}, MPS: {}, Interval: {}, ABSOLUTE tablet (proto {}) <<<",
                                                            ep_addr, ep_mps, ep_interval, current_intf_protocol);
                                                        self.slots[slot_id as usize].mouse_ep = ep_addr;
                                                        self.slots[slot_id as usize].mouse_mps = ep_mps;
                                                        self.slots[slot_id as usize].mouse_interval = ep_interval;
                                                        self.slots[slot_id as usize].is_mouse = true;
                                                        self.slots[slot_id as usize].mouse_is_relative = false;
                                                        found_hid_ep = true;
                                                    } else {
                                                        serial_println!("xHCI: (ignoring extra non-pointer HID interface, proto {}, ep {:#x})", current_intf_protocol, ep_addr);
                                                    }
                                                    found_hid = false; // one interrupt-IN EP per interface; next iface re-arms
                                                }
                                            } else if desc_type == 0x05 && is_mass_storage { // Bulk Endpoint
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

                                        // Configure ALL collected HID interrupt-IN endpoints (keyboard
                                        // and/or pointer) in one Configure-Endpoint command.
                                        if found_hid_ep {
                                            self.configure_hid_endpoints(slot_id as u8);
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
                                        }
                                    }
                                }
                                }
                            } else if endpoint_id > 1 && slot_id > 0 { // Non-EP0 Transfer Event
                                // Bulk-Only Transport routing: if a BOT transaction is in
                                // flight on this slot's bulk endpoints, hand the completion to
                                // the synchronous pump. The CSW is matched by its TRB address so
                                // it is never confused with a data-stage event; any error
                                // completion also finishes the transaction (for stall handling).
                                if let Some(p) = self.bot_pending {
                                    if p.slot_id == slot_id as u8
                                        && (endpoint_id as u8 == p.in_dci || endpoint_id as u8 == p.out_dci)
                                    {
                                        let is_match = param == p.wait_trb_phys;
                                        let is_error = completion_code != 1 && completion_code != 13;
                                        if is_match || is_error {
                                            if let Some(bp) = self.bot_pending.as_mut() {
                                                bp.completion_code = completion_code as u8;
                                                bp.done = true;
                                            }
                                        }
                                        return; // consumed by the BOT pump
                                    }
                                }
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
                                    
                                    if mouse_dci == Some(endpoint_id as u8) {
                                        // --- POINTER (mouse / tablet) --- reads into its own buffer.
                                        if let Some(data_buf_ptr) = slot.mouse_data_buffer {
                                            let data_data = core::slice::from_raw_parts(data_buf_ptr, 512);
                                            let _buttons = data_data[0];
                                            // Metal diagnostic (parallels the keyboard dump): show the raw
                                            // pointer report so a serial-less boot can confirm pointer
                                            // transfers are arriving and see whether the layout decodes.
                                            // Skip idle all-zero reports (a composite dongle polls its
                                            // endpoints continuously) so only real movement/clicks print.
                                            #[cfg(feature = "usbdebug")]
                                            if data_data[0] != 0 || data_data[1] != 0 || data_data[2] != 0 || data_data[3] != 0 {
                                                serial_println!(
                                                    "USB-DEBUG: ptr report ({}) {:02x} {:02x} {:02x} {:02x}",
                                                    if slot.mouse_is_relative { "rel" } else { "abs" },
                                                    data_data[0], data_data[1], data_data[2], data_data[3]
                                                );
                                            }

                                            if slot.mouse_is_relative {
                                                // HID BOOT mouse: byte0 = buttons, byte1 = dx:i8, byte2 = dy:i8
                                                // (byte3 = wheel, ignored). Signed relative deltas — sign-extend
                                                // i8 -> i32 and emit only on actual motion.
                                                let dx = data_data[1] as i8 as i32;
                                                let dy = data_data[2] as i8 as i32;
                                                if dx != 0 || dy != 0 {
                                                    crate::pal::push_event(crate::pal::Event::Mouse { x: dx, y: dy });
                                                }
                                            } else {
                                                // usb-tablet / absolute pointer: byte1-2 = X, byte3-4 = Y (0..32767).
                                                let x = (data_data[1] as u16) | ((data_data[2] as u16) << 8);
                                                let y = (data_data[3] as u16) | ((data_data[4] as u16) << 8);
                                                if x != 0 || y != 0 {
                                                    crate::pal::push_event(crate::pal::Event::MouseAbsolute { x: x as i32, y: y as i32 });
                                                }
                                            }

                                            self.queue_mouse_read(slot_id as u8);
                                        }
                                    } else if keyboard_dci == Some(endpoint_id as u8) {
                                        // --- KEYBOARD ---
                                        if let Some(data_buf_ptr) = slot.data_buffer {
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
                                            
                                            for i in 2..8 {
                                                let keycode = report[i];
                                                if keycode == 0 { continue; } // No key
                                                if keycode == 1 { continue; } // ErrorRollOver
                                                
                                                if (keycode as usize) < HID_SCANCODE_TO_ASCII.len() {
                                                    let (unshifted, shifted) = HID_SCANCODE_TO_ASCII[keycode as usize];
                                                    let ascii = if shift { shifted } else { unshifted };
                                                    if ascii != 0 {
                                                        serial_println!("xHCI: KEY: '{}' (scancode {:#x})", ascii as char, keycode);
                                                        crate::pal::push_event(crate::pal::Event::Key(ascii));
                                                    }
                                                }
                                            }
                                            
                                            self.queue_keyboard_read(slot_id as u8);
                                        }
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
        unsafe {
            // 1. Allocate and set DCBAAP
            let dcbaap_size = (self.max_slots as usize + 1) * 8;
            let layout = core::alloc::Layout::from_size_align(dcbaap_size, 64).unwrap();
            let dcbaap_ptr = alloc::alloc::alloc_zeroed(layout) as *mut u64;
            self.dcbaap = dcbaap_ptr;

            let dcbaap_reg = (self.op_base + 0x30) as *mut u64;
            core::ptr::write_volatile(dcbaap_reg, dcbaap_ptr as u64);
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
                let pagesize = core::ptr::read_volatile((self.op_base + 0x08) as *const u32) & 0xFFFF;
                let page_bytes = 1usize << (pagesize.trailing_zeros() + 12);

                // The Scratchpad Buffer Array: one u64 physical pointer per buffer, page-aligned.
                let arr_layout =
                    core::alloc::Layout::from_size_align(max_scratchpad * 8, page_bytes).unwrap();
                let arr = alloc::alloc::alloc_zeroed(arr_layout) as *mut u64;
                for i in 0..max_scratchpad {
                    let buf_layout =
                        core::alloc::Layout::from_size_align(page_bytes, page_bytes).unwrap();
                    let buf = alloc::alloc::alloc_zeroed(buf_layout);
                    // Identity-mapped heap: the allocation's virtual address IS its bus/phys address.
                    *arr.add(i) = buf as u64;
                }
                *dcbaap_ptr.add(0) = arr as u64;
                serial_println!(
                    "xHCI: scratchpad: {} buffer(s) x {} bytes; DCBAA[0]={:#x}",
                    max_scratchpad, page_bytes, arr as u64
                );
            } else {
                serial_println!("xHCI: scratchpad: controller requests 0 buffers (none needed).");
            }

            // 2. Set Command Ring Control Register (CRCR)
            // OpBase + 0x18.
            // MUST set Bit 0 (RCS - Ring Cycle State) to 1 to match our initial Ring state.
            let crcr_reg = (self.op_base + 0x18) as *mut u64;
            let crcr_value = ring_phys_addr | 1;
            core::ptr::write_volatile(crcr_reg, crcr_value);
            serial_println!("xHCI: CRCR set to {:#x}", crcr_value);
        }
    }

    // Call this AFTER init_pointers but BEFORE run
    pub fn init_interrupter(&mut self, event_ring_phys: u64, erst_table_phys: u64) {
        unsafe {
            // SAVE THIS for later use in the interrupt/event loop (ERDP updates)
            EVENT_RING_PHYS_BASE = event_ring_phys;

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
            EVENT_RING_PHYS_BASE = event_ring_phys;

            // 3. Write ERSTSZ (Segment Table Size) - Offset 0x08
            // Value = 1 (We have 1 segment)
            let erstsz_ptr = (ir0_base + 0x08) as *mut u32;
            core::ptr::write_volatile(erstsz_ptr, 1);

            // 4. Write ERSTBA (Segment Table Base Address) - Offset 0x10
            let erstba_ptr = (ir0_base + 0x10) as *mut u64;
            core::ptr::write_volatile(erstba_ptr, erst_table_phys);

            // 5. Write ERDP (Event Ring Dequeue Pointer) - Offset 0x18
            // Initialize to the start of the ring.
            // PRESERVE BIT 3 (EHB - Event Handler Busy)? No, clear it initially.
            let erdp_ptr = (ir0_base + 0x18) as *mut u64;
            core::ptr::write_volatile(erdp_ptr, event_ring_phys); // Pointer to the RING, not the table

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
            let usbcmd_ptr = self.op_base as *mut u32;
            let cmd = core::ptr::read_volatile(usbcmd_ptr);
            core::ptr::write_volatile(usbcmd_ptr, cmd | 0b101);

            // Wait until USBSTS.HCH (Halted) is 0
            let usbsts_ptr = (self.op_base + 0x04) as *const u32;
            let _ = wait_until(
                || (core::ptr::read_volatile(usbsts_ptr) & 1) == 0,
                hw_wait_budget(), "USBSTS.HCH=0 (run)");
            serial_println!("xHCI: Controller Started!");

            // Power on all ports. Use the REAL MaxPorts (HCSPARAMS1 bits 24:31),
            // captured as self.max_ports. The previous code read bits 0:7, which is
            // MaxSlots (64 here) — powering 64 nonexistent ports.
            let max_ports = self.max_ports;
            serial_println!("xHCI: Max Ports = {}", max_ports);

            for i in 1..=max_ports {
                let port_offset = 0x400 + (i as usize - 1) * 0x10;
                let portsc_ptr = (self.op_base + port_offset) as *mut u32;
                let status = core::ptr::read_volatile(portsc_ptr);

                // Bit 9: PP (Port Power)
                if (status & (1 << 9)) == 0 {
                    serial_println!("xHCI: Powering on Port {}", i);
                    core::ptr::write_volatile(portsc_ptr, status | (1 << 9));
                } else {
                    serial_println!("xHCI: Port {} already powered. Status: {:#x}", i, status);
                }
            }

            // Collect every connected port and enumerate them ONE AT A TIME. Push in
            // reverse so the queue pops in ascending port order.
            self.ports_to_enumerate.clear();
            for i in (1..=max_ports).rev() {
                let port_offset = 0x400 + (i as usize - 1) * 0x10;
                let portsc_ptr = (self.op_base + port_offset) as *const u32;
                let status = core::ptr::read_volatile(portsc_ptr);

                // Bit 0: CCS (Current Connect Status)
                if (status & 1) != 0 {
                    serial_println!("xHCI: Port {} connected (Status: {:#x}); queued for enumeration.", i, status);
                    self.ports_to_enumerate.push(i);
                }
            }
        }

        // Kick off enumeration of the first connected port (outside the unsafe block).
        self.start_next_port();
    }

    /// Begin enumerating the next queued connected port. Called at boot and again each
    /// time a device finishes its setup, so at most one port is mid-enumeration.
    fn start_next_port(&mut self) {
        while let Some(port) = self.ports_to_enumerate.pop() {
            let portsc = self.read_portsc(port);
            if (portsc & 1) == 0 {
                serial_println!("xHCI: Port {} no longer connected; skipping.", port);
                continue;
            }
            self.enum_active = true;
            serial_println!("xHCI: === Enumerating Port {} (PORTSC={:#x}) ===", port, portsc);
            // Bit 1: PED (Port Enabled/Disabled).
            if (portsc & 2) != 0 {
                // Already enabled (typical for SuperSpeed): request a slot directly.
                serial_println!("xHCI: Port {} already enabled; requesting slot.", port);
                self.enable_slot(port);
            } else {
                // Needs a USB reset first; the Port Reset Change event drives enable_slot.
                serial_println!("xHCI: Port {} requires reset before enable.", port);
                self.handle_port_change(port);
            }
            return;
        }
        self.enum_active = false;
        serial_println!("xHCI: Port enumeration queue drained.");
    }

    fn handle_port_change(&self, port_id: u8) {
        unsafe {
            // 1. Get the Port Register Set
            // PORTSC is at op_base + 0x400 + (port_id - 1) * 0x10
            // Note: port_id is 1-based from the Event TRB.
            let port_offset = 0x400 + (port_id as usize - 1) * 0x10;
            let portsc_ptr = (self.op_base + port_offset) as *mut u32;

            let mut status = core::ptr::read_volatile(portsc_ptr);
            serial_println!("xHCI: Port {} Status: {:#x}", port_id, status);

            // PHASE 1: ACKNOWLEDGE (Clear CSC if set)
            // Bit 17: CSC (Connect Status Change). RW1C (Read/Write 1 to Clear).
            if (status & (1 << 17)) != 0 {
                serial_println!("xHCI: Clearing CSC on Port {}", port_id);
                // Clear CSC (Bit 17) by writing 1 to it.
                // Preserve other R/W bits, but ensure PR (Bit 4) is 0 to avoid unintended reset.
                let clear_csc = (status & !(1 << 4)) | (1 << 17);
                core::ptr::write_volatile(portsc_ptr, clear_csc);

                // Re-read status after clear
                status = core::ptr::read_volatile(portsc_ptr);
            }

            // 2. Check for Connection (Bit 0: CCS - Current Connect Status)
            if (status & 1) != 0 {
                // Only reset if enabled bit (Bit 1: PED) is 0 (not yet enabled)
                // AND we are not already resetting (Bit 4: PR)
                if (status & 2) == 0 && (status & (1 << 4)) == 0 {
                    serial_println!("xHCI: Device Connected on Port {}. Resetting...", port_id);
                    // 3. Initiate Port Reset (Bit 4: PR)
                    core::ptr::write_volatile(portsc_ptr, status | (1 << 4));
                }
            }
        }
    }

    pub fn diagnose_command_ring(&self, original_ptr: u64) {
        unsafe {
            // 1. READ CRCR (Command Ring Control Register)
            // Offset 0x18 from OpBase
            let crcr_reg = (self.op_base + 0x18) as *const u64;
            let crcr_raw = core::ptr::read_volatile(crcr_reg);

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
        self.pending_ports.push(port_id);

        // TRB Type 9 = Enable Slot
        // Control: (Type 9 << 10)
        // Cycle Bit is handled by the Ring.
        let trb = Trb {
            parameter: 0,
            status: 0,
            control: (9 << 10),
        };

        if let Err(e) = self.send_command(trb) {
            serial_println!("xHCI: Failed to send Enable Slot command: {}", e);
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
            serial_println!("xHCI: DCBAAP[{}] linked to {:#x}", slot_id, output_ctx_phys);

            // 2. FILL INPUT CONTEXT (MANUAL OFFSET CALCULATION)
            let base_ptr = input_ctx_virt as *mut u32;

            // Clear Input Context (33 * 32 = 1056 bytes)
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 1056);

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
                _ => 8,   // Full Speed / unknown: 8 is always valid for FS control transfers (a
                          // larger FS MPS0 would be refined from bMaxPacketSize0 — a follow-up).
            };
            serial_println!("xHCI: Port {} speed {} -> EP0 MPS {}", port_id, speed, mps0);
            let slot_ctx_ptr = base_ptr.add(8);
            slot_ctx_ptr.add(0).write_volatile((1 << 27) | ((speed & 0xF) << 20)); // Context Entries=1 + Speed
            slot_ctx_ptr.add(1).write_volatile((port_id as u32) << 16); // Root Hub Port Number

            // 3c. ENDPOINT 0 CONTEXT (Offset 0x40 -> Index 16 in u32)
            let ep0_ctx_ptr = base_ptr.add(16);
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

            if let Err(e) = self.send_command(trb) {
                serial_println!("xHCI: Failed to send Address Device command: {}", e);
            }
            // NOTE: do NOT set `configuring_slot` here. That field marks an in-flight
            // Configure-Endpoint command; setting it on Address Device made the Address
            // Device completion be misdispatched as "endpoints configured", which jumped
            // straight to SCSI read (skipping device-descriptor + endpoint setup) and
            // panicked on an unallocated data_buffer. The Address Device completion now
            // correctly falls through to request_device_descriptor().
        }
    }

    pub fn configure_endpoints(&mut self, slot_id: u8, in_addr: u8, in_mps: u16, out_addr: u8, out_mps: u16) {
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
            
            let bulk_in_ring = ring::TransferRing::new(16);
            let bulk_in_phys = bulk_in_ring.get_ptr();
            slot.bulk_in_ring = Some(bulk_in_ring);

            let bulk_out_ring = ring::TransferRing::new(16);
            let bulk_out_phys = bulk_out_ring.get_ptr();
            slot.bulk_out_ring = Some(bulk_out_ring);

            // Dedicated DMA buffers for Bulk-Only Transport (CBW / data / CSW).
            let cbw_layout = core::alloc::Layout::from_size_align(64, 64).unwrap();
            slot.cbw_buffer = Some(alloc::alloc::alloc_zeroed(cbw_layout));
            let csw_layout = core::alloc::Layout::from_size_align(64, 64).unwrap();
            slot.csw_buffer = Some(alloc::alloc::alloc_zeroed(csw_layout));
            let data_layout = core::alloc::Layout::from_size_align(512, 64).unwrap();
            slot.scsi_data_buffer = Some(alloc::alloc::alloc_zeroed(data_layout));
            slot.bulk_in_ep = in_addr;
            slot.bulk_out_ep = out_addr;

            // 2. CLEAR INPUT CONTEXT (Safety first)
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 1056);

            // 3. INPUT CONTROL CONTEXT (Offset 0x00): A0 (slot context) + both bulk DCIs.
            base_ptr.add(1).write_volatile(1u32 | (1 << in_dci) | (1 << out_dci));

            // 4. SLOT CONTEXT (Offset 0x20 -> Index 8)
            let slot_ctx_ptr = base_ptr.add(8);
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
            let ep_in_ptr = base_ptr.add(16 + ((in_dci as usize - 1) * 8));
            ep_in_ptr.add(1).write_volatile((6 << 3) | (3 << 1) | ((in_mps as u32) << 16)); // EP Type 6 (Bulk IN), CErr 3
            ep_in_ptr.add(2).write_volatile((bulk_in_phys as u32) | 1);
            ep_in_ptr.add(3).write_volatile((bulk_in_phys >> 32) as u32);
            ep_in_ptr.add(4).write_volatile(in_mps as u32);

            // 6. BULK OUT endpoint context.
            let ep_out_ptr = base_ptr.add(16 + ((out_dci as usize - 1) * 8));
            ep_out_ptr.add(1).write_volatile((2 << 3) | (3 << 1) | ((out_mps as u32) << 16)); // EP Type 2 (Bulk OUT), CErr 3
            ep_out_ptr.add(2).write_volatile((bulk_out_phys as u32) | 1);
            ep_out_ptr.add(3).write_volatile((bulk_out_phys >> 32) as u32);
            ep_out_ptr.add(4).write_volatile(out_mps as u32);

            serial_println!("xHCI: Input Context Configured for Bulk Transport.");

            // 7. SEND CONFIGURE ENDPOINT COMMAND
            let trb = Trb {
                parameter: input_ctx_virt as u64,
                status: 0,
                control: (12 << 10) | ((slot_id as u32) << 24),
            };

            if let Err(e) = self.send_command(trb) {
                serial_println!("xHCI: Failed to send Configure Endpoint command: {}", e);
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

    /// Execute a synchronous Bulk-Only Transport transaction: CBW -> (optional data) -> CSW.
    /// MUST be called from a non-event context (controller lock held, event ring free) such
    /// as the main loop or a shell command — never from inside handle_event_trb.
    pub fn bot_transfer(&mut self, slot_id: u8, cdb: &[u8], data_phys: u64, data_len: u32, dir: Direction)
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

        let tag = self.build_cbw(cbw_phys as *mut u8, data_len, dir, cdb);
        unsafe { core::ptr::write_bytes(csw_phys as *mut u8, 0, 13); }

        // BOT phases are SERIALIZED: CBW -> [DATA] -> CSW, each transfer completing before
        // the next is queued — mirroring the Linux usb-storage bulk transport. We must NOT
        // pipeline the CSW TRB behind an async data stage: QEMU's usb-storage is a single-
        // packet device, so a CSW IN token arriving while the DATA transfer is still async
        // is never serviced and the transfer hangs with no completion event. The CBW is
        // fire-and-forget (the device consumes it before it can respond); the DATA and CSW
        // stages each carry IOC (1<<5) so their completion posts a Transfer Event (-> MSI ->
        // the pump wakes). We await the DATA stage, then the CSW — never the CBW directly.

        // 1) CBW on bulk OUT (Normal TRB, 31 bytes).
        self.slots[slot_id as usize].bulk_out_ring.as_mut().unwrap()
            .push(Trb { parameter: cbw_phys, status: 31, control: 1 << 10 }).ok();

        // 2) Data stage (IN or OUT), if any. IOC + ISP (1<<2) so both full and short-packet
        //    completions post an event; wait for it to retire BEFORE queuing the CSW.
        if data_len > 0 {
            let data_out = matches!(dir, Direction::Out);
            let (data_dci, data_trb_phys) = {
                let ring = if data_out {
                    self.slots[slot_id as usize].bulk_out_ring.as_mut().unwrap()
                } else {
                    self.slots[slot_id as usize].bulk_in_ring.as_mut().unwrap()
                };
                let base = ring.get_ptr();
                let idx = ring.push(Trb { parameter: data_phys, status: data_len,
                    control: (1 << 10) | (1 << 5) | (1 << 2) }).unwrap_or(0);
                (if data_out { out_dci } else { in_dci }, base + (idx as u64) * 16)
            };

            // Ring OUT to fetch+send the CBW; for an IN data stage also ring the IN ring.
            // (An OUT data stage rides the same OUT ring as the CBW, in order.)
            self.ring_doorbell(slot_id, out_dci as u32);
            if data_dci != out_dci { self.ring_doorbell(slot_id, data_dci as u32); }

            let code = self.run_bot_stage(slot_id, in_dci, out_dci, data_trb_phys)?;
            if code != 1 && code != 13 {
                serial_println!("xHCI: BOT data stage error, completion code {}", code);
                return if code == 4 || code == 6 { Err(BotError::Stall) }
                       else { Err(BotError::TransferError(code)) };
            }
        } else {
            // No data stage: fetch+send the CBW now; the CSW is queued next.
            self.ring_doorbell(slot_id, out_dci as u32);
        }

        // 3) CSW on bulk IN (13 bytes, IOC). The data stage (if any) has fully retired, so
        //    usb-storage is in its CSW state and services this token immediately.
        let csw_trb_phys = {
            let ring = self.slots[slot_id as usize].bulk_in_ring.as_mut().unwrap();
            let base = ring.get_ptr();
            let idx = ring.push(Trb { parameter: csw_phys, status: 13, control: (1 << 10) | (1 << 5) }).unwrap_or(0);
            base + (idx as u64) * 16
        };
        self.ring_doorbell(slot_id, in_dci as u32);

        let code = self.run_bot_stage(slot_id, in_dci, out_dci, csw_trb_phys)?;
        if code != 1 && code != 13 {
            serial_println!("xHCI: BOT transfer error, completion code {}", code);
            return if code == 4 || code == 6 { Err(BotError::Stall) }
                   else { Err(BotError::TransferError(code)) };
        }

        // 4) Validate the CSW.
        unsafe {
            let csw = core::slice::from_raw_parts(csw_phys as *const u8, 13);
            let sig = (csw[0] as u32) | ((csw[1] as u32) << 8) | ((csw[2] as u32) << 16) | ((csw[3] as u32) << 24);
            let csw_tag = (csw[4] as u32) | ((csw[5] as u32) << 8) | ((csw[6] as u32) << 16) | ((csw[7] as u32) << 24);
            let residue = (csw[8] as u32) | ((csw[9] as u32) << 8) | ((csw[10] as u32) << 16) | ((csw[11] as u32) << 24);
            let bstatus = csw[12];

            if sig != 0x53425355 {
                serial_println!("xHCI: BOT bad CSW signature {:#x}", sig);
                return Err(BotError::BadCswSignature);
            }
            if csw_tag != tag {
                serial_println!("xHCI: BOT CSW tag mismatch (got {:#x}, want {:#x})", csw_tag, tag);
                return Err(BotError::TagMismatch);
            }
            let status = match bstatus {
                0 => CswStatus::Passed, 1 => CswStatus::Failed,
                2 => CswStatus::PhaseError, _ => CswStatus::Unknown,
            };
            Ok(BotResult { status, residue })
        }
    }

    /// Arm the BOT pending state for one stage (waiting on `wait_trb_phys`'s completion
    /// event), pump the event ring until it arrives, and return its completion code. The
    /// caller queues the stage's TRB(s) and rings the doorbell(s) before calling this.
    fn run_bot_stage(&mut self, slot_id: u8, in_dci: u8, out_dci: u8, wait_trb_phys: u64)
        -> Result<u8, BotError>
    {
        self.bot_pending = Some(BotPending {
            slot_id, in_dci, out_dci, wait_trb_phys,
            done: false, completion_code: 0,
        });
        let pump = self.pump_until_bot_done(2000);
        let pending = self.bot_pending.take();
        pump?;
        let p = pending.ok_or(BotError::Timeout)?;
        Ok(p.completion_code)
    }

    /// Pump the event ring until the in-flight BOT transaction reports done, or the
    /// iteration budget is exhausted. Unrelated events (HID input, command completions)
    /// are dispatched normally during the wait.
    fn pump_until_bot_done(&mut self, max_iters: u64) -> Result<(), BotError> {
        let mut iters: u64 = 0;
        loop {
            match &self.bot_pending {
                Some(p) if p.done => return Ok(()),
                None => return Ok(()),
                _ => {}
            }
            if self.drain_event_ring_once() {
                continue; // processed an event; drain any more immediately
            }
            // Yield to QEMU's main loop so it can run the xHC bottom-half / async block-I/O
            // completion and DMA the event into the ring; a pure spin never exits TCG.
            crate::hlt();
            iters += 1;
            if iters >= max_iters {
                unsafe {
                    let ir0 = XHCI_IR0_BASE.load(Ordering::Acquire);
                    let op = XHCI_OP_BASE.load(Ordering::Acquire);
                    let iman = if ir0 != 0 { core::ptr::read_volatile(ir0 as *const u32) } else { 0 };
                    let usbsts = if op != 0 { core::ptr::read_volatile((op + 0x04) as *const u32) } else { 0 };
                    serial_println!(
                        "xHCI: BOT pump TIMEOUT after {} yields (IRQ_COUNT={} IMAN={:#x} USBSTS={:#x})",
                        iters, XHCI_IRQ_COUNT.load(Ordering::Relaxed), iman, usbsts);
                }
                return Err(BotError::Timeout);
            }
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
    fn scsi_read10(&mut self, slot: u8, lba: u32, blocks: u16) -> Result<BotResult, BotError> {
        let data_phys = self.storage_data_phys(slot)?;
        let len = (blocks as u32) * 512;
        let cdb = [0x28, 0,
            (lba >> 24) as u8, (lba >> 16) as u8, (lba >> 8) as u8, lba as u8,
            0, (blocks >> 8) as u8, blocks as u8, 0];
        self.bot_transfer(slot, &cdb, data_phys, len, Direction::In)
    }

    /// SCSI WRITE(10) (0x2A) of `blocks` blocks at `lba` from the storage data buffer.
    fn scsi_write10(&mut self, slot: u8, lba: u32, blocks: u16) -> Result<BotResult, BotError> {
        let data_phys = self.storage_data_phys(slot)?;
        let len = (blocks as u32) * 512;
        let cdb = [0x2A, 0,
            (lba >> 24) as u8, (lba >> 16) as u8, (lba >> 8) as u8, lba as u8,
            0, (blocks >> 8) as u8, blocks as u8, 0];
        self.bot_transfer(slot, &cdb, data_phys, len, Direction::Out)
    }

    // ---- Public storage API used by the block layer / shell ----

    /// Pointer to the storage slot's data buffer (one block).
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

        // TEST UNIT READY — USB sticks often report "becoming ready" a few times.
        for attempt in 0..16 {
            match self.scsi_test_unit_ready(slot) {
                Ok(CswStatus::Passed) => break,
                Ok(_) => { let _ = self.scsi_request_sense(slot); }
                Err(e) => { serial_println!("xHCI: TUR error {:?} (attempt {})", e, attempt); }
            }
        }

        let (vendor, product) = self.scsi_inquiry(slot)?;
        let (block_size, last_lba) = self.scsi_read_capacity10(slot)?;
        let num_blocks = last_lba as u64 + 1;

        let vendor_s = core::str::from_utf8(&vendor).unwrap_or("?").trim_end();
        let product_s = core::str::from_utf8(&product).unwrap_or("?").trim_end();
        serial_println!("xHCI: Disk '{}' '{}' block_size={} num_blocks={} ({} MiB)",
            vendor_s, product_s, block_size, num_blocks,
            (num_blocks * block_size as u64) / (1024 * 1024));

        *crate::drivers::block::BLOCK_DEVICE.lock() = Some(crate::drivers::block::BlockDeviceInfo {
            slot_id: slot, block_size, num_blocks, vendor, product,
        });
        Ok(())
    }

    /// Main-loop hook: once storage finishes configuring, run the SCSI bring-up (in a
    /// safe, non-event context) and publish the block device. Also does a one-time
    /// sanity read of LBA 0.
    pub fn service_storage(&mut self) {
        if !self.storage_pending_bringup { return; }
        self.storage_pending_bringup = false;
        if self.storage_slot == 0 { return; }

        serial_println!("xHCI: === STORAGE BRING-UP (TUR/INQUIRY/READ CAPACITY) ===");
        match self.bring_up_storage() {
            Ok(()) => serial_println!("xHCI: storage ready."),
            Err(e) => { serial_println!("xHCI: storage bring-up failed: {:?}", e); return; }
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
                    }
                }
            }
            Err(e) => serial_println!("xHCI: READ(10) LBA0 failed: {:?}", e),
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

        // Setup stage. TRT: 0 = no data, 2 = OUT data, 3 = IN data.
        let trt: u32 = if w_length == 0 { 0 } else if dir_in { 3 } else { 2 };
        self.push_ep0(slot_id, Trb { parameter: setup, status: 8, control: (2 << 10) | (1 << 6) | (trt << 16) });

        // Data stage (if any).
        if w_length > 0 {
            let dir: u32 = if dir_in { 1 } else { 0 };
            self.push_ep0(slot_id, Trb { parameter: data_phys, status: w_length as u32, control: (3 << 10) | (dir << 16) });
        }

        // Status stage (IOC). Direction is opposite the data stage; with no data it is IN.
        let status_dir: u32 = if w_length == 0 { 1 } else if dir_in { 0 } else { 1 };
        let status_phys = {
            let ring = self.slots[slot_id as usize].ep0_ring.as_mut().ok_or(())?;
            let base = ring.get_ptr();
            let idx = ring.push(Trb { parameter: 0, status: 0, control: (4 << 10) | (1 << 5) | (status_dir << 16) }).unwrap_or(0);
            base + (idx as u64) * 16
        };

        self.ep0_pending = Some(Ep0Pending { slot_id, wait_trb_phys: status_phys, done: false, completion_code: 0 });
        self.ring_doorbell(slot_id, 1);

        let pump = self.pump_until_ep0_done(2000);
        let pending = self.ep0_pending.take();
        pump?;
        Ok(pending.ok_or(())?.completion_code)
    }

    /// Pump the event ring until the in-flight synchronous EP0 transfer reports done (or the
    /// iteration budget is exhausted). Unrelated events are dispatched normally during the wait.
    fn pump_until_ep0_done(&mut self, max_iters: u64) -> Result<(), ()> {
        let mut iters: u64 = 0;
        loop {
            match &self.ep0_pending {
                Some(p) if p.done => return Ok(()),
                None => return Ok(()),
                _ => {}
            }
            if self.drain_event_ring_once() {
                continue;
            }
            crate::hlt(); // yield to QEMU so it can DMA the completion into the event ring
            iters += 1;
            if iters >= max_iters {
                serial_println!("xHCI: EP0 sync pump TIMEOUT after {} yields", iters);
                return Err(());
            }
        }
    }

    /// Synchronous xHCI command (ENABLE_SLOT / ADDRESS_DEVICE / CONFIGURE_ENDPOINT): push the TRB
    /// to the command ring, ring the command doorbell, and pump until the completion arrives
    /// (matched by command-TRB address). Returns (completion_code, slot_id). Used for hub bring-up
    /// so downstream devices are enumerated without threading new states through the async FSM.
    fn run_command_sync(&mut self, trb: Trb) -> Result<(u8, u8), ()> {
        let cmd_phys = {
            let mut g = COMMAND_RING.lock();
            let ring = g.as_mut().ok_or(())?;
            let base = ring.get_ptr();
            let idx = ring.push(trb).map_err(|_| ())?;
            base + (idx as u64) * 16
        };
        self.ring_doorbell(0, 0);
        self.cmd_pending = Some(CmdPending { cmd_trb_phys: cmd_phys, done: false, completion_code: 0, slot_id: 0 });
        let pump = self.pump_until_cmd_done(2000);
        let pending = self.cmd_pending.take();
        pump?;
        let p = pending.ok_or(())?;
        Ok((p.completion_code, p.slot_id))
    }

    fn pump_until_cmd_done(&mut self, max_iters: u64) -> Result<(), ()> {
        let mut iters: u64 = 0;
        loop {
            match &self.cmd_pending {
                Some(p) if p.done => return Ok(()),
                None => return Ok(()),
                _ => {}
            }
            if self.drain_event_ring_once() {
                continue;
            }
            crate::hlt();
            iters += 1;
            if iters >= max_iters {
                serial_println!("xHCI: command sync pump TIMEOUT after {} yields", iters);
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

        // 2. GET_DESCRIPTOR (HUB, type 0x29) -> downstream port count + characteristics.
        //    bmRequestType 0xA0 (D2H, class, device), wValue = type<<8.
        if self.sync_control(hub_slot, 0xA0, 0x06, (0x29u16) << 8, 0, 64, buf, true).is_err() {
            serial_println!("xHCI: HUB descriptor request timed out");
            return;
        }
        let (nbr_ports, characteristics) = unsafe {
            let p = buf as *const u8;
            (*p.add(2), (*p.add(3) as u16) | ((*p.add(4) as u16) << 8))
        };
        serial_println!("xHCI: HUB slot {} has {} downstream ports (characteristics {:#06x})",
            hub_slot, nbr_ports, characteristics);

        let root_hub_port = self.slots[hub_slot as usize].port_id;
        let ttt = ((characteristics >> 5) & 0x3) as u32; // TT Think Time (wHubCharacteristics bits 5-6)

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
            if let Some(speed) = self.reset_downstream_port(hub_slot, port, buf) {
                self.enumerate_downstream(hub_slot, port, root_hub_port, speed);
            }
        }
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
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 1056);
            base_ptr.add(1).write_volatile(1); // Input Control: A0 (slot context) only

            let slot_ctx = base_ptr.add(8);
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
            Ok((1, _)) => serial_println!("xHCI: HUB slot {} marked as hub ({} ports)", hub_slot, nbr_ports),
            Ok((c, _)) => serial_println!("xHCI: HUB slot {} configure-endpoint code {}", hub_slot, c),
            Err(_) => serial_println!("xHCI: HUB slot {} configure-endpoint timed out", hub_slot),
        }
    }

    /// Reset a downstream hub port; return the attached device's xHCI speed code (1=FS, 2=LS, 3=HS)
    /// or None if the port did not enable. Uses hub-class port requests (CLEAR/SET_FEATURE,
    /// GET_STATUS).
    fn reset_downstream_port(&mut self, hub_slot: u8, port: u8, buf: u64) -> Option<u32> {
        let _ = self.sync_control(hub_slot, 0x23, 0x01, 16, port as u16, 0, 0, false); // CLEAR C_PORT_CONNECTION
        let _ = self.sync_control(hub_slot, 0x23, 0x03, 4, port as u16, 0, 0, false); // SET PORT_RESET

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
            if pstatus & (1 << 20) != 0 { break; } // C_PORT_RESET set
        }
        let _ = self.sync_control(hub_slot, 0x23, 0x01, 20, port as u16, 0, 0, false); // CLEAR C_PORT_RESET

        if pstatus & (1 << 1) == 0 {
            serial_println!("xHCI: HUB port {} did not enable after reset (status {:#x})", port, pstatus);
            return None;
        }
        // Hub port status: bit 9 = Low Speed, bit 10 = High Speed; otherwise Full Speed.
        let speed = if pstatus & (1 << 9) != 0 { 2u32 } else if pstatus & (1 << 10) != 0 { 3 } else { 1 };
        serial_println!("xHCI: HUB port {} reset OK (status {:#x}, xHCI speed {})", port, pstatus, speed);
        Some(speed)
    }

    /// Address a device behind a hub: like `address_device` but the slot context carries a
    /// non-zero Route String (the downstream port number, tier 1), the chain's Root Hub Port
    /// Number, and the device Speed. Synchronous. Returns true on success.
    fn address_downstream(&mut self, slot_id: u8, root_hub_port: u8, route_string: u32, speed: u32) -> bool {
        unsafe {
            let input_layout = core::alloc::Layout::from_size_align(core::mem::size_of::<InputContext>(), 64).unwrap();
            let output_layout = core::alloc::Layout::from_size_align(core::mem::size_of::<DeviceContext>(), 64).unwrap();
            let input_ctx_virt = alloc::alloc::alloc_zeroed(input_layout) as *mut InputContext;
            let output_ctx_virt = alloc::alloc::alloc_zeroed(output_layout) as *mut DeviceContext;
            let ep0_ring = ring::TransferRing::new(16);
            let ep0_ring_phys = ep0_ring.get_ptr();

            let slot = &mut self.slots[slot_id as usize];
            slot.input_context = input_ctx_virt;
            slot.output_context = output_ctx_virt;
            slot.ep0_ring = Some(ep0_ring);
            slot.port_id = root_hub_port;
            slot.active = true;

            *self.dcbaap.add(slot_id as usize) = output_ctx_virt as u64;

            let base_ptr = input_ctx_virt as *mut u32;
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 1056);
            base_ptr.add(1).write_volatile(3); // A0 (slot) + A1 (EP0)

            let slot_ctx = base_ptr.add(8);
            // DW0: Context Entries = 1 | Route String (bits 19:0) | Speed (bits 23:20).
            slot_ctx.add(0).write_volatile((1 << 27) | (route_string & 0xFFFFF) | ((speed & 0xF) << 20));
            // DW1: Root Hub Port Number (bits 23:16) — the root port the hub chain starts at.
            slot_ctx.add(1).write_volatile((root_hub_port as u32) << 16);

            // EP0 control context. MPS: 8 for Low Speed; 64 otherwise (QEMU is lenient about the
            // Full-Speed initial 8-byte read).
            let mps0: u32 = if speed == 2 { 8 } else { 64 };
            let ep0_ctx = base_ptr.add(16);
            ep0_ctx.add(1).write_volatile((4 << 3) | (3 << 1) | (mps0 << 16));
            ep0_ctx.add(2).write_volatile((ep0_ring_phys as u32) | 1);
            ep0_ctx.add(3).write_volatile((ep0_ring_phys >> 32) as u32);
            ep0_ctx.add(4).write_volatile(8);
        }
        let trb = Trb {
            parameter: self.slots[slot_id as usize].input_context as u64,
            status: 0,
            control: (11 << 10) | ((slot_id as u32) << 24),
        };
        match self.run_command_sync(trb) {
            Ok((1, _)) => true,
            Ok((c, _)) => { serial_println!("xHCI: downstream ADDRESS_DEVICE code {}", c); false }
            Err(_) => { serial_println!("xHCI: downstream ADDRESS_DEVICE timed out"); false }
        }
    }

    /// Enumerate one device behind a hub: ENABLE_SLOT, ADDRESS_DEVICE (with route string), read the
    /// device + configuration descriptors, and — for an HID keyboard/mouse — hand off to the
    /// existing endpoint-configuration path (which finishes set-config + arms the interrupt read
    /// via the normal async completion). Storage / non-HID behind a hub is not yet handled.
    fn enumerate_downstream(&mut self, _hub_slot: u8, port: u8, root_hub_port: u8, speed: u32) {
        // ENABLE_SLOT.
        let slot_id = match self.run_command_sync(Trb { parameter: 0, status: 0, control: 9 << 10 }) {
            Ok((1, sid)) if sid > 0 => sid,
            other => { serial_println!("xHCI: downstream ENABLE_SLOT failed ({:?})", other); return; }
        };

        // ADDRESS_DEVICE with route string = downstream port number (tier 1).
        if !self.address_downstream(slot_id, root_hub_port, port as u32, speed) {
            return;
        }
        let buf = self.slots[slot_id as usize].descriptor_buffer as u64;

        // GET device descriptor (18 bytes).
        if self.sync_control(slot_id, 0x80, 0x06, 0x0100, 0, 18, buf, true).is_err() {
            serial_println!("xHCI: downstream slot {} device-descriptor failed", slot_id);
            return;
        }
        let (class, vid, pid) = unsafe {
            let p = buf as *const u8;
            (*p.add(4), (*p.add(8) as u16) | ((*p.add(9) as u16) << 8), (*p.add(10) as u16) | ((*p.add(11) as u16) << 8))
        };
        serial_println!("xHCI: HUB downstream slot {} device class={:#x} vid={:04x} pid={:04x}", slot_id, class, vid, pid);

        // GET configuration descriptor (first 64 bytes) and look for an HID interrupt-IN endpoint.
        if self.sync_control(slot_id, 0x80, 0x06, 0x0200, 0, 64, buf, true).is_err() {
            serial_println!("xHCI: downstream slot {} config-descriptor failed", slot_id);
            return;
        }
        match self.parse_hid_config(buf) {
            Some((is_kbd, is_rel, ep_addr, mps, interval)) => {
                serial_println!("xHCI: HUB downstream slot {} is {} (ep {:#x} mps {})",
                    slot_id,
                    if is_kbd { "KEYBOARD" } else if is_rel { "MOUSE (relative)" } else { "TABLET (absolute)" },
                    ep_addr, mps);
                if is_kbd {
                    self.slots[slot_id as usize].is_keyboard = true;
                    self.slots[slot_id as usize].keyboard_ep = ep_addr;
                    self.slots[slot_id as usize].keyboard_mps = mps;
                    self.slots[slot_id as usize].keyboard_interval = interval;
                } else {
                    self.slots[slot_id as usize].is_mouse = true;
                    self.slots[slot_id as usize].mouse_is_relative = is_rel;
                    self.slots[slot_id as usize].mouse_ep = ep_addr;
                    self.slots[slot_id as usize].mouse_mps = mps;
                    self.slots[slot_id as usize].mouse_interval = interval;
                }
                // Configure the (single, behind-a-hub) HID endpoint via the shared path.
                // parse_hid_config surfaces one interface, so this arms exactly that endpoint;
                // a composite device behind a hub keeps the single-interface limitation for now.
                self.configure_hid_endpoints(slot_id);
            }
            None => serial_println!("xHCI: HUB downstream slot {}: no HID interrupt endpoint", slot_id),
        }
    }

    /// Parse a configuration descriptor (in `buf`) for the first HID interrupt-IN endpoint.
    /// Returns (is_keyboard, is_relative_mouse, ep_addr, max_packet_size, interval) or None.
    /// `is_relative_mouse` = bInterfaceProtocol == 2 (HID boot mouse; relative deltas) vs an
    /// absolute pointer (protocol 0). Only meaningful when `is_keyboard` is false.
    fn parse_hid_config(&self, buf: u64) -> Option<(bool, bool, u8, u16, u8)> {
        unsafe {
            let p = buf as *const u8;
            let total = (((*p.add(2) as usize) | ((*p.add(3) as usize) << 8))).min(64);
            let mut off = 0usize;
            let mut in_hid = false;
            let mut is_kbd = false;
            let mut is_rel = false;
            while off + 2 <= total {
                let len = *p.add(off) as usize;
                let dtype = *p.add(off + 1);
                if len == 0 { break; }
                if dtype == 0x04 && off + 8 <= total {
                    // Interface descriptor: class at +5, protocol at +7. HID = 0x03;
                    // proto 1 = boot keyboard, proto 2 = boot mouse (relative).
                    in_hid = *p.add(off + 5) == 0x03;
                    is_kbd = *p.add(off + 7) == 1;
                    is_rel = *p.add(off + 7) == 2;
                } else if dtype == 0x05 && in_hid && off + 7 <= total {
                    // Endpoint descriptor: address +2, attributes +3, MPS +4..6, interval +6.
                    let ep_addr = *p.add(off + 2);
                    let attr = *p.add(off + 3);
                    if (ep_addr & 0x80) != 0 && (attr & 0x3) == 3 {
                        let mps = ((*p.add(off + 4) as u16) | ((*p.add(off + 5) as u16) << 8)) & 0x7FF;
                        return Some((is_kbd, is_rel, ep_addr, mps, *p.add(off + 6)));
                    }
                }
                off += len;
            }
            None
        }
    }

    pub fn send_scsi_read(&mut self, slot_id: u8) {
        unsafe {
            serial_println!("xHCI: UNA-21 Initiating SCSI Read (Sector 0)...");

            let (desc_phys, data_phys, cbw_ptr) = {
                let slot = &self.slots[slot_id as usize];
                let Some(data_buf) = slot.data_buffer else {
                    serial_println!("xHCI: send_scsi_read: slot {} has no data_buffer (endpoints not configured); aborting read", slot_id);
                    return;
                };
                (slot.descriptor_buffer as u64, data_buf as u64, slot.descriptor_buffer)
            };

            core::ptr::write_bytes(cbw_ptr, 0, 64);

            *cbw_ptr.add(0) = 0x55; *cbw_ptr.add(1) = 0x53; *cbw_ptr.add(2) = 0x42; *cbw_ptr.add(3) = 0x43;
            *cbw_ptr.add(4) = 0xEF; *cbw_ptr.add(5) = 0xBE; *cbw_ptr.add(6) = 0xAD; *cbw_ptr.add(7) = 0xDE;
            *cbw_ptr.add(8) = 0x00; *cbw_ptr.add(9) = 0x02; *cbw_ptr.add(10) = 0x00; *cbw_ptr.add(11) = 0x00;
            *cbw_ptr.add(12) = 0x80; *cbw_ptr.add(13) = 0x00; *cbw_ptr.add(14) = 10;
            *cbw_ptr.add(15) = 0x28; *cbw_ptr.add(23) = 1;

            let out_trb = Trb {
                parameter: desc_phys,
                status: 31,
                control: (1 << 10) | (1 << 5),
            };
            self.slots[slot_id as usize].bulk_out_ring.as_mut().unwrap().push(out_trb).unwrap();
            self.ring_doorbell(slot_id, 4);

            let in_trb = Trb {
                parameter: data_phys,
                status: 512,
                control: (1 << 10) | (1 << 5) | (1 << 2),
            };
            self.slots[slot_id as usize].bulk_in_ring.as_mut().unwrap().push(in_trb).unwrap();
            self.ring_doorbell(slot_id, 3);

            serial_println!("xHCI: SCSI Command Dispatched.");
        }
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

    fn push_ep0(&mut self, slot_id: u8, trb: Trb) {
        unsafe {
            if let Some(ep0_ring) = &mut self.slots[slot_id as usize].ep0_ring {
                ep0_ring.push(trb).unwrap();
            } else {
                serial_println!("xHCI: push_ep0 failed, no ep0_ring for slot {}", slot_id);
            }
        }
    }

    pub fn request_device_descriptor(&mut self, slot_id: u8) {
        serial_println!("xHCI: Requesting Device Descriptor for Slot {}...", slot_id);

        let desc_phys = self.slots[slot_id as usize].descriptor_buffer as u64;
        if desc_phys == 0 {
            serial_println!("xHCI: CRITICAL ERROR - Descriptor Buffer Phys Addr is 0!");
            return;
        }

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
        self.push_ep0(slot_id, status_trb);

        // 4. Ring Doorbell (Slot 1, Target 1 for EP0)
        self.ring_doorbell(slot_id, 1);
    }

    pub fn request_configuration_descriptor(&mut self, slot_id: u8) {
        serial_println!("xHCI: Requesting Configuration Descriptor for Slot {}...", slot_id);

        let desc_phys = self.slots[slot_id as usize].descriptor_buffer as u64;
        if desc_phys == 0 {
            serial_println!("xHCI: CRITICAL ERROR - Descriptor Buffer Phys Addr is 0!");
            return;
        }

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
        self.push_ep0(slot_id, status_trb);

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
    pub fn configure_hid_endpoints(&mut self, slot_id: u8) {
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
            self.start_next_port();
            return;
        }

        let input_ctx_virt;
        let max_dci;
        unsafe {
            let slot = &mut self.slots[slot_id as usize];
            input_ctx_virt = slot.input_context;
            let output_ctx_virt = slot.output_context;
            let base_ptr = input_ctx_virt as *mut u32;

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
            core::ptr::write_bytes(base_ptr as *mut u8, 0, 1056);
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
                let ep = base_ptr.add(16 + ((dci as usize - 1) * 8));
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
                let ep = base_ptr.add(16 + ((dci as usize - 1) * 8));
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
            let slot_ctx_ptr = base_ptr.add(8);
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
        if let Err(e) = self.send_command(trb) {
            serial_println!("xHCI: Failed to send HID Configure Endpoint command: {}", e);
        } else {
            self.ring_doorbell(0, 0); // Ring the command-ring doorbell.
        }
    }

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
            self.push_ep0(slot_id, status_trb);

            self.ring_doorbell(slot_id, 1);
        }
    }

    pub fn queue_mouse_read(&mut self, slot_id: u8) {
        unsafe {
            let ep_num = self.slots[slot_id as usize].mouse_ep & 0x0F;
            let dir_in = (self.slots[slot_id as usize].mouse_ep & 0x80) != 0;
            let dci = (ep_num * 2) + if dir_in { 1 } else { 0 };

            let data_phys = self.slots[slot_id as usize].mouse_data_buffer.unwrap() as u64;

            let in_trb = Trb {
                parameter: data_phys,
                status: self.slots[slot_id as usize].mouse_mps as u32, // Length
                control: (1 << 10) | (1 << 5), // Type 1 | IOC
            };
            self.slots[slot_id as usize].mouse_ring.as_mut().unwrap().push(in_trb).unwrap();
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

            let in_trb = Trb {
                parameter: data_phys,
                status: self.slots[slot_id as usize].keyboard_mps as u32,
                control: (1 << 10) | (1 << 5), // Type 1 (Normal) | IOC
            };
            self.slots[slot_id as usize].keyboard_ring.as_mut().unwrap().push(in_trb).unwrap();
            self.ring_doorbell(slot_id, dci as u32);
            xdbg!("xHCI: Keyboard Read Queued.");
        }
    }
}
