// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BCM2711 VideoCore "mailbox" property-channel driver — the bare-metal Pi 4's framebuffer.
//
// Booting from the microSD slot (arch/aarch64/boot.rs) there is no UEFI, so there is no GOP to
// hand us a framebuffer. Instead we ask the VideoCore GPU for one directly over its mailbox: a
// pair of FIFO registers at peripheral offset 0xB880 through which the ARM posts a 16-byte-aligned
// "property tag" buffer on channel 8 and the GPU fills in the reply in place. One request here
// sets the display size/depth/pixel-order, allocates the framebuffer, and reads back its base
// address + pitch; the result becomes the BootInfo framebuffer the normal video stack draws on.
//
// Two BCM-specific subtleties, both handled below:
//   * Addresses across the mailbox are *VideoCore bus* addresses, not ARM physical. We post the
//     buffer through the uncached bus alias (`| 0xC000_0000`) so the GPU reads coherent RAM, and
//     mask the returned framebuffer base (`& 0x3FFF_FFFF`) back to an ARM physical address.
//   * The CPU's data cache is not snooped by the GPU, so the request buffer is cleaned to RAM
//     before posting and invalidated after the reply (arch::aarch64::cache). No-ops in QEMU.
//
// The GPU/DMA mutates the `MBOX` static behind the compiler's back, so every reply word is read
// with `read_volatile` *after* the call returns — never through a `&mut` held across the call,
// which the optimiser could satisfy from a stale register.
//
// MBOX-1 adds two properties the transport lacked, both about how it behaves when it goes WRONG:
//
//   * **It fails clean.** A timed-out call used to return `false` and leave the FIFO and the request
//     buffer exactly as the failure found them — so a late firmware reply became the next call's
//     doorbell, and a failure-path buffer read hit our own posted words in the data cache and read
//     them as a reply. Every timeout exit now drains the read FIFO (bounded) and runs the same cache
//     invalidate the success path does, and says on the wire how many stale words it discarded. See
//     `MboxLoan::timeout_clean`. This matters beyond one call: a wedged property channel takes the
//     framebuffer, V3D power/clock, the `NOTIFY_XHCI_RESET` firmware reload and EMMC2's SD base
//     clock down with it.
//   * **It has ONE user at a time, and says so.** `MBOX` was a bare `static mut` resting on a
//     "single-core boot, single-threaded" premise that the call graph no longer supports. It now
//     lives behind a WEDGE-8-shaped claim/loan (`claim` / `MboxLoan`): a mutex held only for a
//     masked O(1) take/put, the transport loaned out for the long CNTPCT poll with nothing held, and
//     an immediate honest `Busy` for contenders. See `claim` for the audit and the reasoning.

use unaos_boot_info::{FrameBufferInfo, PixelFormat};

use super::cache;

// --- MMIO: peripheral base + mailbox registers (Pi 4 low-peripheral mode). ---
// 0xFE00_0000 is the BCM2711 peripheral base; the mailbox sits at +0xB880. These all land in the
// Device-mapped 0xC000_0000–0xFFFF_FFFF window of the boot page table, so reads/writes are
// strongly ordered (no caching) as MMIO requires.
const PERI_BASE: usize = 0xFE00_0000;
const MBOX_BASE: usize = PERI_BASE + 0xB880;
const MBOX_READ: usize = MBOX_BASE + 0x00;
const MBOX_STATUS: usize = MBOX_BASE + 0x18;
const MBOX_WRITE: usize = MBOX_BASE + 0x20;

const MBOX_FULL: u32 = 0x8000_0000; // STATUS: write FIFO full  -> can't post
const MBOX_EMPTY: u32 = 0x4000_0000; // STATUS: read FIFO empty -> nothing to read
const MBOX_RESPONSE: u32 = 0x8000_0000; // request-code reply: success
/// Request-code reply: "error parsing request buffer" — the VPU read the message and found it
/// MALFORMED. It wrote, but it never looked at any tag inside, so this code must never be read as
/// evidence that a tag was acted on (PI-V3D-81, `NoReplySend::buffer_rejected`).
#[cfg(feature = "v3d81")]
const MBOX_ERROR: u32 = 0x8000_0001;

const CH_PROP: u32 = 8; // ARM -> VC property tags

/// Uncached VideoCore bus alias. ARM physical RAM is visible to the GPU at `phys | 0xC000_0000`
/// without going through the VC L2 — pairing it with a CPU cache-clean gives the GPU a coherent
/// view of our request. (QEMU aliases all four GiB windows to RAM, so this also resolves there.)
const BUS_UNCACHED: u32 = 0xC000_0000;
/// Mask to convert a returned VC bus address back to an ARM physical address (strip the alias).
const BUS_TO_PHYS: u32 = 0x3FFF_FFFF;

// --- Property tags (RPi firmware mailbox interface). ---
const TAG_GET_PHYS_WH: u32 = 0x0004_0003; // get current physical (display) width/height
const TAG_SET_PHYS_WH: u32 = 0x0004_8003;
const TAG_SET_VIRT_WH: u32 = 0x0004_8004;
const TAG_SET_VIRT_OFFSET: u32 = 0x0004_8009;
const TAG_SET_DEPTH: u32 = 0x0004_8005;
const TAG_SET_PIXEL_ORDER: u32 = 0x0004_8006;
const TAG_ALLOCATE_FB: u32 = 0x0004_0001;
const TAG_GET_PITCH: u32 = 0x0004_0008;
// WC-E: the READ-side companions of the framebuffer setters. `init_framebuffer` programs the geometry
// and then keeps the values it REQUESTED; the firmware is free to clamp, round or refuse any of them,
// and every one of those divergences is a scan-out defect that looks like garble on the panel and like
// nothing at all in the serial log. These tags ask the firmware what it actually settled on, so the
// wire carries the scan-out ground truth (see `witness_fb_geometry`). Query-only — they set nothing.
const TAG_GET_VIRT_WH: u32 = 0x0004_0004; // get virtual (framebuffer) width/height
const TAG_GET_VIRT_OFFSET: u32 = 0x0004_0009; // get the virtual viewport offset {x, y}
const TAG_GET_DEPTH: u32 = 0x0004_0005; // get bits per pixel
const TAG_GET_PIXEL_ORDER: u32 = 0x0004_0006; // get pixel order (0 = BGR, 1 = RGB)
const TAG_GET_ALPHA_MODE: u32 = 0x0004_0007; // get alpha mode (0 = enabled/0-opaque, 1 = reversed, 2 = ignored)
const TAG_GET_CLOCK_RATE: u32 = 0x0003_0002; // M6g: query a clock's current rate (value buffer {id, rate})
// PI-V3D-1: firmware property tags for powering + clocking the VideoCore VI (V3D) block. These are
// the Pi-4/VC6 path (the legacy VC4 `Enable_QPU`/set-power tags are NOT used here). See
// arch_arm64.md §PI-V3D and the scout research of record. `v3d`-gated so a knob-off kernel8 is
// byte-identical to baseline (these are additive, not called on any default path).
#[cfg(any(feature = "v3d", feature = "piusb"))]
const TAG_SET_DOMAIN_STATE: u32 = 0x0003_8030; // set a power-domain's on/off state (value {domain, state})
const TAG_SET_ENABLE_QPU: u32 = 0x0003_0012; // firmware-side QPU/V3D enable (value {enable}) — the VPU init path the KMS overlay boot runs and a bare-metal boot never receives (PI-V3D-75)
const TAG_NOTIFY_DISPLAY_DONE: u32 = 0x0003_0066; // vc4's probe-time handover: "firmware display driver, stop — the ARM owns the complex now" (PI-V3D-80)
#[cfg(any(feature = "v3d", feature = "piusb"))]
const TAG_SET_CLOCK_STATE: u32 = 0x0003_8001; // enable/disable a clock's GATE (value {clock_id, state})
#[cfg(feature = "v3d")]
const TAG_SET_CLOCK_RATE: u32 = 0x0003_8002; // set a clock's rate (value {clock_id, rate_hz, skip_turbo})
// PIUSB-32: the READ-side companions the diagnostic census uses to witness the power/clock state of the
// candidate firmware domains BEFORE the deferred RC APB read (which P39/P40/P41 metal proved hard-stalls
// the CPU). Query-only tags — they never change firmware state. `{id, state}` value buffers; the reply
// `state` echoes bit0 = on/active, bit1 = "device/clock/domain does not exist". `piusb`-gated (additive).
#[cfg(feature = "piusb")]
const TAG_GET_POWER_STATE: u32 = 0x0002_0001; // get a firmware DEVICE's power state (value {device_id, state})
#[cfg(feature = "piusb")]
const TAG_GET_DOMAIN_STATE: u32 = 0x0003_0030; // get a power-DOMAIN's on/off state (value {domain, state})
// PI-V3D-58: widened to match its only consumer. V3D-55 made `get_clock_state` compile for `v3d` as
// well as `piusb` (so the clock-domain audit cannot depend on `piusb` being enabled in the same build)
// but left this tag `piusb`-only, so a `v3d`-without-`piusb` build failed to compile at E0425. The two
// cfgs must agree or the widening is a no-op that only breaks the build it was meant to enable.
#[cfg(any(feature = "piusb", feature = "v3d"))]
const TAG_GET_CLOCK_STATE: u32 = 0x0003_0001; // get a clock's GATE state (value {clock_id, state})
// PI-USB-1: notify the VideoCore firmware to reset (and reload the SPI-EEPROM firmware into) the VIA
// VL805 xHCI behind the BCM2711 PCIe RC. The RPi bootloader normally loads the VL805 firmware at power-on;
// this tag re-issues that reset for an OS bringing the controller up itself. Value buffer = one u32, the
// VL805's PCI device address `(bus<<20)|(dev<<15)|(fn<<12)` (bus 1, dev 0, fn 0 => 0x0010_0000).
// `piusb`-gated so a knob-off kernel8 is byte-identical to baseline (additive, not on any default path).
#[cfg(feature = "piusb")]
const TAG_NOTIFY_XHCI_RESET: u32 = 0x0003_0058;
const TAG_END: u32 = 0x0000_0000;

// PI-V3D-1 identifiers for the two calls above (RPi firmware mailbox interface).
#[cfg(feature = "v3d")]
pub const POWER_DOMAIN_V3D: u32 = 10; // firmware power-domain index for the V3D block
#[cfg(feature = "v3d")]
pub const CLOCK_ID_V3D: u32 = 5; // firmware clock id for V3D

const PIXEL_ORDER_BGR: u32 = 0; // firmware: 0 = BGR, 1 = RGB. We request BGR to match the rest of
                                // the stack's default (and the GOP path's observed Bgr); put_pixel
                                // writes B,G,R for PixelFormat::Bgr, which is what the HVS then
                                // scans out. If colours come out swapped on metal, flip both.
const DEPTH_BITS: u32 = 32;
const BYTES_PER_PIXEL: u32 = DEPTH_BITS / 8;

/// Fallback display size if the firmware reports no current mode (it normally does — it sets HDMI
/// to the monitor's preferred mode at boot). 1920×1080 is the safe ubiquitous HDMI mode.
const FALLBACK_W: u32 = 1920;
const FALLBACK_H: u32 = 1080;

/// WC-D — the compile-time PANEL OVERRIDE (`UNAOS_FBW` / `UNAOS_FBH`), 0 = off (query the firmware).
/// See [`init_framebuffer`] for why a size knob is a correctness tool and not a convenience: the window
/// compositor's upscale factor is a function of the panel, so a 640x480 QEMU gate cannot reach the blit
/// path a 1920x1200 bench panel runs. Default-quiet: unset => the const is 0 => not one instruction of
/// behaviour changes.
pub const FORCE_W: u32 = parse_u32(option_env!("UNAOS_FBW"));
pub const FORCE_H: u32 = parse_u32(option_env!("UNAOS_FBH"));

/// Decimal `u32` parse usable in a `const` initialiser (no `str::parse` in const context). Any
/// non-digit or absent value yields 0, which is the "override off" value — fail-closed to the
/// firmware-queried mode rather than to an invented resolution.
const fn parse_u32(s: Option<&str>) -> u32 {
    let b = match s {
        Some(s) => s.as_bytes(),
        None => return 0,
    };
    let mut v: u32 = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] < b'0' || b[i] > b'9' {
            return 0;
        }
        v = v * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    v
}

/// The property message buffer. The framebuffer message is 35 words; 48 words (192 bytes) rounds
/// the allocation up to whole 64-byte cache lines so the clean/invalidate over it can't touch a
/// neighbouring static's lines. **64-byte aligned** for the same reason — `DC IVAC/CIVAC` operate on
/// whole lines, so a sub-line-aligned buffer would let a maintenance op clobber adjacent data; a
/// line-aligned, line-padded buffer is self-contained. The mailbox ABI's own 16-byte alignment
/// requirement is subsumed by this.
///
/// MBOX-1: this buffer is reachable ONLY through [`MboxLoan`]. It used to be a bare `static mut` on
/// the premise that mailbox use is confined to single-core boot; see [`claim`] for why that premise
/// is false and what replaced it.
#[repr(C, align(64))]
struct MboxBuf {
    words: [u32; 48],
}
static mut MBOX: MboxBuf = MboxBuf { words: [0; 48] };

/// MBOX-1 — the transport's availability flag, and the ONLY lock in this module. `true` = the
/// transport is on the shelf; `false` = it is loaned to exactly one context. Held for a masked O(1)
/// take/put and nothing else (WEDGE-8 discipline, `drivers/xhci::claim`): the LONG work — a property
/// transaction whose CNTPCT deadline runs to 500 ms by default and 5 s for `NOTIFY_DISPLAY_DONE` —
/// runs with this mutex NOT held, so no masked spinner can ever wait on it for more than a few dozen
/// cycles and no holder of it can be preempted mid-hold.
///
/// The invariant is grep-checkable, the F1/WEDGE-8 idiom: `MBOX_FREE.lock()` appears ONLY in
/// [`claim`] and `MboxLoan::drop`, and `MBOX` itself is named only by [`mbox_phys`] and the
/// [`MboxLoan`] accessors — the statics are private, so the compiler enforces the rest.
static MBOX_FREE: spin::Mutex<bool> = spin::Mutex::new(true);

#[inline]
fn mmio_read(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}
#[inline]
fn mmio_write(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

/// Base address of the `MBOX` static as a `usize` (for cache ranges and the post address). Takes an
/// address only — never dereferences — so it needs no loan.
#[inline]
fn mbox_phys() -> usize {
    &raw const MBOX as usize
}

/// MBOX-1 — the hard cap on any read-FIFO drain. A FIFO that will not empty in this many words is a
/// fault, not a queue, and spinning on one would hang the boot the drain is meant to protect.
const DRAIN_MAX: u32 = 64;

/// MBOX-1 — empty the read FIFO without waiting for anything, returning how many words were
/// discarded. Bounded by construction (see [`DRAIN_MAX`]). Pure MMIO: it touches the FIFO, never the
/// request buffer, which is why it is callable with or without a loan.
fn drain_read_fifo_bounded() -> u32 {
    let mut n = 0;
    while mmio_read(MBOX_STATUS) & MBOX_EMPTY == 0 && n < DRAIN_MAX {
        let _ = mmio_read(MBOX_READ);
        n += 1;
    }
    n
}

/// Why [`claim`] handed back no transport. One variant only: unlike the xHCI controller the mailbox
/// is never "not yet installed" — it is a static buffer plus always-present MMIO, live from the
/// first instruction — so `Busy` is the whole failure space.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MboxClaimError {
    /// Another context holds the transport right now. The claim did NOT wait: waiting is the
    /// caller's decision, and a masked caller must not.
    Busy,
}

/// MBOX-1 — an exclusive loan of the VideoCore property transport: the `MBOX` request buffer and the
/// mailbox FIFO pair, which are one indivisible resource (the buffer is where the reply lands, so a
/// second writer between our post and our read-back corrupts both transactions). Dropping it returns
/// the transport to the shelf — a masked O(1) put, panic-safe by RAII.
///
/// Every accessor that touches `MBOX` is a method here, so "no mailbox access without the loan" is a
/// compile-time property of this module rather than a comment asking readers to be careful.
pub struct MboxLoan(());

impl Drop for MboxLoan {
    fn drop(&mut self) {
        // MBOX-1: masked micro-hold. Local order is the discipline in miniature — `guard` drops
        // before `_mask` restores, so we never run unmasked while holding the lock.
        let _mask = crate::arch::IrqMask::new();
        let mut guard = MBOX_FREE.lock();
        *guard = true;
    }
}

/// MBOX-1 — claim exclusive use of the property transport. O(1), never waits.
///
/// ### The premise this replaces
/// `MBOX` was a bare `static mut`, justified in a dozen doc comments by "single-user: boot-time,
/// single-threaded, the framebuffer call is long done". That is no longer true, and the audit says
/// where it stopped being true:
///   * `emmc2::probe()` (`main.rs`) runs its `GET_CLOCK_RATE` on the BSP **after**
///     `sched::start_aps` has put the secondaries to work — the two calls are adjacent lines.
///   * `piusb::bringup_task` → `enumerate` → `notify_before_reset` → [`notify_xhci_reset`] runs
///     later still, on the BSP, *past the GUI/input/render task spawn*, with AP tasks live.
/// Neither is the single-threaded boot window the comments describe. No current caller runs masked
/// and no current caller runs on an AP, so nothing has been observed to corrupt yet — but the
/// premise the safety rested on is gone, and a torn property transaction is not a defect that
/// announces itself: it hands back a plausible reply word from the wrong tag.
///
/// ### Why claim/loan and not a masked guard
/// WEDGE-7 (`video::wm::table`) masks across the critical section, which is affordable there because
/// every TABLE hold is a bounded row scan. A property transaction is not: it polls a CNTPCT deadline
/// of 500 ms by default and 5 s for `NOTIFY_DISPLAY_DONE`, against a 12 ms quantum. Masking a core
/// for half a second — let alone five — is the bug in another coat, and holding a plain mutex across
/// it is the WEDGE-8 F3 death (a preempted holder that a masked spinner on the same core can never
/// let run again). So WEDGE-8's shape applies: the mutex guards a masked O(1) take/put, the
/// TRANSPORT is loaned out for the long poll with no lock held, and contenders get an immediate
/// honest [`MboxClaimError::Busy`] instead of a wait.
fn claim() -> Result<MboxLoan, MboxClaimError> {
    let _mask = crate::arch::IrqMask::new();
    let mut guard = MBOX_FREE.lock();
    if *guard {
        *guard = false;
        Ok(MboxLoan(()))
    } else {
        Err(MboxClaimError::Busy)
    }
}

/// MBOX-1 — the ONE caller policy that is not "fail loud": retry the claim, unmasked and bounded,
/// for up to `budget_ms`. Used only by [`get_clock_rate`], whose refusal has a real cost — the EMMC2
/// probe falls back to *assuming* 100 MHz for the SD base clock, which programs a wrong divider
/// rather than merely losing a diagnostic. Every other entry point fails loud instead, because for
/// them a `Busy` costs a log line and nothing else.
///
/// Refuses to wait at all when IRQs are masked (`arch::irqs_masked`, the WEDGE-8 rule): a masked
/// waiter cannot be preempted OR take a timer IRQ, so spinning there is exactly the deadlock the
/// claim/loan model exists to prevent. No caller is masked today; the check is what keeps that from
/// silently changing.
fn claim_bounded(budget_ms: u64) -> Result<MboxLoan, MboxClaimError> {
    let first = claim();
    if first.is_ok() || crate::arch::irqs_masked() {
        return first;
    }
    let deadline = super::timer::cntpct() + cntfrq() / 1000 * budget_ms;
    loop {
        if let Ok(l) = claim() {
            return Ok(l);
        }
        if super::timer::cntpct() >= deadline {
            return Err(MboxClaimError::Busy);
        }
        core::hint::spin_loop();
    }
}

/// MBOX-1 — the refusal witness. One line, naming the operation that was turned away, so a `None`
/// caused by contention is never mistaken on the wire for a `None` caused by a dead VideoCore. Same
/// `:: MAILBOX: ... ::` grammar as the timeout witnesses.
#[cold]
fn witness_busy(op: &str) {
    serial_println!(":: MAILBOX: BUSY — {} refused, transport loaned to another context ::", op);
}

/// MBOX-1 — claim or bail with this entry point's documented failure value, emitting the refusal
/// witness on the way out. Keeps the Busy policy to one line per entry point instead of five.
macro_rules! claim_or {
    ($op:expr, $bail:expr) => {
        match claim() {
            Ok(loan) => loan,
            Err(_) => {
                witness_busy($op);
                return $bail;
            }
        }
    };
}

/// What `init_framebuffer` resolved: an ARM-physical framebuffer the video stack can draw on.
pub struct FbAlloc {
    pub base: u64,
    pub size: usize,
    pub info: FrameBufferInfo,
}

/// Generic-timer frequency (Hz), read from CNTFRQ_EL0. Available with no init at this single-core
/// boot point; used only to size the mailbox timeout below.
#[inline]
fn cntfrq() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// Post `MBOX.words[0..len]` to the property channel and wait for the reply in place. Returns
/// whether the GPU reported success. `len` is the number of valid u32 words (word 0 = total byte
/// size). Handles the cache maintenance and the bus-address conversion.
///
/// Every wait is bounded by a wall-clock deadline off the free-running CNTPCT: a real VideoCore
/// always replies in microseconds, but if it never does (an unsupported/malformed tag the firmware
/// silently drops, an HDMI/firmware quirk) an unbounded spin here would hang boot forever — and the
/// serial-only fallback in main.rs (`framebuffer_addr == 0`) would be unreachable. On timeout we
/// return false so init_framebuffer yields None and the kernel degrades to the serial console,
/// mirroring the x86 xHCI wall-clock-deadline discipline. (QEMU always replies, so this never
/// fires there.)
impl MboxLoan {
    /// Read reply word `idx` straight from RAM. Use only after [`Self::call`]: the GPU wrote the
    /// reply and the call invalidated our cached copy, so this volatile load re-fetches the fresh
    /// value. MBOX-1: on a call that FAILED the invalidate still ran (see [`Self::timeout_clean`]),
    /// so this reads the GPU's memory rather than a cache hit on our own request words.
    #[inline]
    fn reply(&self, idx: usize) -> u32 {
        unsafe { core::ptr::read_volatile(&raw const MBOX.words[idx]) }
    }

    /// Write request word `idx`. Volatile so the store is committed to memory before [`Self::call`]'s
    /// cache-clean pushes it to the GPU (no reliance on the optimiser ordering plain stores).
    #[inline]
    fn request(&mut self, idx: usize, val: u32) {
        unsafe { core::ptr::write_volatile(&raw mut MBOX.words[idx], val) }
    }

    #[inline]
    fn call(&mut self, len: usize) -> bool {
        // ~500 ms budget — generous vs the microseconds a real reply takes.
        self.call_budget(len, 500)
    }

    /// MBOX-1 — the timeout exit, and the only one. Returns `false` for the caller, but never before
    /// putting the transport back in a state the NEXT call can trust:
    ///
    ///   * **Drain the read FIFO** (bounded, [`DRAIN_MAX`]). A firmware reply that arrives after our
    ///     deadline is still coming; left in the FIFO it becomes the *next* call's doorbell, and that
    ///     call then reads a buffer belonging to a tag it never sent. One late reply silently
    ///     mis-attributes every transaction after it.
    ///   * **Invalidate the buffer**, exactly as the success path does. Without this our own posted
    ///     request words stay in the data cache, so a caller that reads reply slots on the failure
    ///     path — [`notify_xhci_reset`] does, unconditionally — gets a cache hit on what it wrote and
    ///     reads it as a reply. PIUSB-12 relies on the `echo` slot reading back 0 *as proof the
    ///     invalidate happened*; on the old timeout path it read back the posted `dev_addr` and
    ///     inverted that witness.
    ///
    /// Why this matters beyond one call: a wedged property channel takes the framebuffer, V3D
    /// power/clock, the `NOTIFY_XHCI_RESET` firmware reload and EMMC2's SD base clock down with it.
    /// The transport is allowed to fail — it is not allowed to fail DIRTY.
    #[cold]
    fn timeout_clean(&mut self, len: usize, what: &str) -> bool {
        let drained = drain_read_fifo_bounded();
        cache::clean_invalidate_range(mbox_phys(), len * 4);
        serial_println!(
            ":: MAILBOX: timeout {} — drained {} stale FIFO word(s), buffer invalidated ::",
            what, drained
        );
        false
    }

/// PI-V3D-80: same call, caller-chosen reply budget, for a caller that has reason to think its tag
/// takes longer than the default 500 ms. The kernel's mailbox wait is unbounded; ours stays
/// bounded, just wider where the work is real.
///
/// ⚠ PI-V3D-81 correction (v3d.md §46.5). This entry point was added on the reading that
/// `NOTIFY_DISPLAY_DONE` merely replies SLOWLY — that the firmware stops its whole display driver
/// before answering. That reading is wrong, and no budget can repair it: P92 timed out at 500 ms,
/// P93 timed out at 5 s, and the same P93 capture shows `NOTIFY_XHCI_RESET` timing out *while the
/// VL805 firmware load it requests demonstrably works*. On this firmware a NOTIFY-class tag is
/// acted on WITHOUT ever ringing the read FIFO, so a timeout here is a statement about the
/// DOORBELL and not about the tag. Widening the budget only makes that statement more slowly. To
/// read such a tag, post it with [`post_noreply`] and read its EFFECT.
    fn call_budget(&mut self, len: usize, budget_ms: u64) -> bool {
        let buf_phys = mbox_phys() as u32;

        // Clean our request out to RAM so the GPU (which doesn't snoop our cache) sees it.
        cache::clean_range(mbox_phys(), len * 4);

        // CNTPCT is monotonic and won't wrap within any boot window, so a plain `>=` compare is sound.
        let deadline = super::timer::cntpct() + cntfrq() / 1000 * budget_ms;
        let timed_out = || super::timer::cntpct() >= deadline;

        // Post: VC bus address of the buffer in the high 28 bits, channel in the low 4. Match Circle's
        // BUS_ADDRESS exactly — strip any alias bits then OR the uncached alias — so a buffer that
        // happened to sit above 1 GiB still resolves (it can't here; the kernel is in low RAM). The
        // 64-byte alignment guarantees the low 4 bits are clear for the channel.
        let msg = ((buf_phys & BUS_TO_PHYS) | BUS_UNCACHED) | CH_PROP;
        while mmio_read(MBOX_STATUS) & MBOX_FULL != 0 {
            if timed_out() {
                return self.timeout_clean(len, "waiting for write FIFO");
            }
            core::hint::spin_loop();
        }
        mmio_write(MBOX_WRITE, msg);

        // Spin for *our* channel's reply (the property channel is the only one we use, but the FIFO is
        // shared, so match the channel and discard anything else).
        loop {
            while mmio_read(MBOX_STATUS) & MBOX_EMPTY != 0 {
                if timed_out() {
                    return self.timeout_clean(len, "waiting for reply");
                }
                core::hint::spin_loop();
            }
            let resp = mmio_read(MBOX_READ);
            if resp & 0xF == CH_PROP {
                break;
            }
            if timed_out() {
                return self.timeout_clean(len, "(only other-channel replies)");
            }
        }

        // The GPU wrote its reply straight to RAM; drop our stale cached copy before reading it back.
        // Clean+invalidate (not bare invalidate) is the always-safe choice — if any line were still
        // dirty it's written back rather than discarded over the GPU's reply.
        cache::clean_invalidate_range(mbox_phys(), len * 4);
        self.reply(1) == MBOX_RESPONSE
    }
}

/// Ask the firmware for the current physical (display) resolution — the mode it auto-set from the
/// monitor at boot. Returns `None` if the call fails or the size is implausible, so the caller
/// falls back to a fixed mode.
fn query_display_size() -> Option<(u32, u32)> {
    let mut m = claim_or!("GET_PHYS_WH (display mode)", None);
    m.request(0, 8 * 4); // total size (8 words used)
    m.request(1, 0); // request
    m.request(2, TAG_GET_PHYS_WH);
    m.request(3, 8); // value buffer size
    m.request(4, 0); // request code
    m.request(5, 0); // width  (reply)
    m.request(6, 0); // height (reply)
    m.request(7, TAG_END);
    if !m.call(8) {
        return None;
    }
    let (width, height) = (m.reply(5), m.reply(6));
    // Reject 0 (no mode) and absurd sizes; the back buffer must fit the 48 MiB heap.
    if (640..=3840).contains(&width) && (480..=2160).contains(&height) {
        Some((width, height))
    } else {
        None
    }
}

/// M6g: ask the VideoCore firmware for a clock's current rate in Hz (RPi firmware property interface,
/// tag `GET_CLOCK_RATE`; value buffer `{clock_id, rate}`). Returns `None` on a mailbox failure or a
/// zero rate (clock not present / not running). Used ONLY by the EMMC2 probe on the BSP to resolve the
/// SDHCI base clock when the controller's own `CAPABILITIES` base-clock field reads zero.
///
/// MBOX-1 — the one BOUNDED-RETRY caller. The old doc here claimed the `MBOX` static was single-user
/// because "the probe runs single-threaded on the BSP"; `emmc2::probe()` is in fact the line after
/// `sched::start_aps`, so the secondaries are already running when this fires. It retries the claim
/// for 600 ms (one default transaction budget plus slack) rather than failing loud, because its
/// caller's fallback is not a lost diagnostic but a WRONG SD divider — `base_clock` assumes 100 MHz
/// when this returns `None`. Every other entry point in this module fails loud instead.
pub fn get_clock_rate(clock_id: u32) -> Option<u32> {
    let mut m = match claim_bounded(600) {
        Ok(loan) => loan,
        Err(_) => {
            witness_busy("GET_CLOCK_RATE (emmc2 base clock, after a 600ms bounded retry)");
            return None;
        }
    };
    m.request(0, 8 * 4); // total size (8 words used)
    m.request(1, 0); // request
    m.request(2, TAG_GET_CLOCK_RATE);
    m.request(3, 8); // value buffer size (2 words: clock_id, rate)
    m.request(4, 0); // request code
    m.request(5, clock_id); // clock id (reply preserves it)
    m.request(6, 0); // rate (reply)
    m.request(7, TAG_END);
    if !m.call(8) {
        return None;
    }
    let rate = m.reply(6);
    if rate == 0 { None } else { Some(rate) }
}

/// PI-V3D-75: ask the FIRMWARE to enable the QPU/V3D (`SET_ENABLE_QPU`, `0x00030012`). This is the
/// VPU-side V3D init path: on a KMS-overlay piOS boot the firmware runs its own V3D bring-up (the
/// mid-bin dump's `RPIVID_ASB_V3D_M_CTRL=0x4040` is its signature), while a bare-metal boot gets the
/// parked state (`0x7`, bridges stopped, bits 6/14 never set). Returns the firmware's reply word
/// (observed 0 on success per the historic mailbox interface), or `None` on a mailbox failure.
#[cfg(feature = "v3d")]
pub fn set_enable_qpu(enable: u32) -> Option<u32> {
    let mut m = claim_or!("SET_ENABLE_QPU", None);
    m.request(0, 7 * 4); // total size (7 words used)
    m.request(1, 0); // request
    m.request(2, TAG_SET_ENABLE_QPU);
    m.request(3, 4); // value buffer size (1 word: enable)
    m.request(4, 0); // request code
    m.request(5, enable);
    m.request(6, TAG_END);
    if !m.call(7) {
        return None;
    }
    Some(m.reply(5))
}

/// PI-V3D-80: `NOTIFY_DISPLAY_DONE` — the ONE mailbox act vc4's probe performs that no UnaOS boot
/// ever has: a zero-length property telling the firmware display driver to stop, ceding the
/// display/GPU complex to the ARM. The V3D-80 hypothesis: the VPU holds V3D thread-start to
/// itself until the ARM claims ownership this way. ⚠ Side effect on a firmware-fb system: the
/// firmware display driver stops — scanout of the mailbox-allocated framebuffer may die (black
/// panel; serial unaffected).
///
/// ⚠ PI-V3D-81 (§46.5): this function cannot report whether the handover HAPPENED. It returns
/// `Some(())` only when a DOORBELL arrives, and this firmware honours NOTIFY-class tags without
/// ringing — so `None` means "no doorbell", never "no handover". Kept so the historic P92/P93 sends
/// stay reproducible; [`notify_display_done_noreply`] is the reading path.
#[cfg(feature = "v3d")]
pub fn notify_display_done() -> Option<()> {
    let mut m = claim_or!("NOTIFY_DISPLAY_DONE", None);
    m.request(0, 6 * 4); // total size (6 words used)
    m.request(1, 0); // request
    m.request(2, TAG_NOTIFY_DISPLAY_DONE);
    m.request(3, 0); // value buffer size (zero-length, like vc4's NULL/0 call)
    m.request(4, 0); // request code
    m.request(5, TAG_END);
    // 5 s budget, kept only for reproducibility of P93. It was chosen on the belief that the
    // firmware stops its whole display driver before replying; §46.5 refuted that — P93 timed out
    // at 5 s exactly as P92 did at 500 ms, because there is no reply to wait for. A false return
    // below is therefore "no doorbell", not "no handover".
    if !m.call_budget(6, 5000) {
        return None;
    }
    Some(())
}

// ── PI-V3D-81: the REPLY-LESS property post, and reading a tag by its EFFECT. ──────────────────
//
// P92/P93 (v3d.md §46.5) established the protocol fact this block exists to act on: on this
// firmware (hash 3484b5dd…, piOS's own) a NOTIFY-class tag is acted on WITHOUT a doorbell coming
// back on the read FIFO. `mbox_call` cannot express that. It returns only when a doorbell arrives,
// so for such a tag it reports failure no matter how perfectly the VPU honoured the request — and
// every V3D-80 verdict read through it was therefore a statement about the DOORBELL, not about the
// tag. Raising the budget (P92 500 ms → P93 5 s) could not fix that; it only made the same
// statement more slowly.
//
// A reply-less post keeps the request half of the protocol (buffer cleaned out to RAM, message
// posted on channel 8) and drops the reply half. In place of waiting it WATCHES, for a bounded and
// stated settle, through two channels that are independent of each other:
//
//   * the DOORBELL, if one ever comes — not required, but counted and timed. Its absence is the
//     protocol claim under test; its presence refutes that claim on the spot for that tag.
//   * the BUFFER, which the VPU writes by DMA and which nothing about the doorbell gates. The
//     property protocol has the firmware stamp word 1 with `0x8000_0000` and the tag's own
//     request word with bit 31 plus the response length. If those words are set, the message was
//     parsed and the tag was HANDLED, whether or not anyone rang. This is the channel P92/P93
//     never read — the buffer was posted and then abandoned when the doorbell wait timed out.
//
// The buffer channel only means something while the CPU can see VPU writes at all, so callers must
// pair it with a positive control on an UNRELATED tag (`v3d.rs` [v3d81] uses `GET_CLOCK_RATE`): a
// control that answers proves both the transport and our cache invalidate are alive at that instant,
// which is what makes an untouched buffer attributable to the firmware's silence rather than to our
// instrument's blindness. Without it, "nothing moved" is uninterpretable — the exact trap §46.5
// records against panel-survival as a reading.
//
// One hazard is inherent and is instrumented rather than hidden: a tag that replies LATER than our
// settle leaves a doorbell in the FIFO that the next ordinary `mbox_call` would consume as its own
// reply, reading a buffer that the newer request has already overwritten. `post_noreply` therefore
// drains and COUNTS the FIFO before posting (`stale_pre`) and again across the settle, and
// [`drain_property_fifo`] lets a caller re-drain before each follow-up query — RETURNING the count,
// because a drain that swallows a late doorbell silently destroys the only evidence that one ever
// arrived. Every caller of it must print what it discarded: whichever drain runs first is where the
// late reply shows up, and `stale_pre` will read 0 on a leg whose caller already swept the FIFO.

/// PI-V3D-81 — everything a reply-less post left behind. Read the fields in this order: `posted`
/// (did the message even reach the write FIFO — nothing below means anything if not), `doorbells`
/// (did the reply half of the protocol happen after all), then the buffer words (did the VPU write
/// a response without ringing).
#[cfg(feature = "v3d81")]
pub struct NoReplySend {
    /// The message reached the write FIFO.
    pub posted: bool,
    /// Words drained from the read FIFO (any channel) BEFORE posting. Expected 0; non-zero means an
    /// earlier reply-less post was answered late and this reading is standing on a dirty FIFO.
    pub stale_pre: u32,
    /// Property-channel doorbells that arrived during the settle. Greater than zero REFUTES
    /// reply-lessness for this tag at this budget — the tag does reply, and §46.5's protocol
    /// reading does not cover it.
    pub doorbells: u32,
    /// Microseconds from post to the FIRST doorbell. Meaningless unless `doorbells > 0`.
    pub first_doorbell_us: u64,
    /// Doorbells seen on other channels, drained and discarded. Expected 0 (we own the FIFO at boot).
    pub other_ch: u32,
    /// The settle actually spent, measured off CNTPCT rather than assumed from the request.
    pub settle_us: u64,
    /// Buffer word 1 after the settle: `0x8000_0000` = the VPU processed the message,
    /// `0x8000_0001` = it rejected the buffer as malformed, `0` = it never wrote (what we posted).
    pub overall: u32,
    /// The tag's own request/response word (buffer word 4). Bit 31 set = THIS TAG was handled, with
    /// its response length in bits [30:0]. The discriminating word — see `notify_xhci_reset`'s
    /// PIUSB-12 note, which named it for the doorbell-bearing case and is what suggested reading it
    /// for the doorbell-less one.
    pub tag_code: u32,
    /// Buffer word 5, printed raw because its meaning is per-tag: the first VALUE word for a valued
    /// tag (`SET_ENABLE_QPU`'s enable echo) and the END marker for a zero-length one
    /// (`NOTIFY_DISPLAY_DONE`, where it must read 0 both before and after).
    pub word5: u32,
    /// ARM-physical address of the message buffer, so a raw capture can be correlated.
    pub buf_pa: usize,
}

#[cfg(feature = "v3d81")]
impl NoReplySend {
    /// The VPU parsed our message and REJECTED it (`overall = 0x8000_0001`, "error parsing request
    /// buffer"). It wrote — so the transport and our invalidate were both alive — but it never
    /// reached the tag, so this state must never read as "acted". Low-probability (these messages
    /// are byte-for-byte the working doorbell-waiting senders' messages) and split out anyway,
    /// because it is precisely the state in which the buffer channel says the opposite of what it
    /// means: the one write that proves the tag was NOT looked at.
    pub fn buffer_rejected(&self) -> bool {
        self.overall == MBOX_ERROR
    }

    /// Did the VPU write a RESPONSE into our buffer? True = the message was parsed and the tag
    /// acted on, doorbell or no doorbell. This is the "acted" half of the acted-vs-ignored
    /// discrimination, and it costs no panel and no attended observer. A rejected buffer is
    /// excluded — see [`Self::buffer_rejected`].
    ///
    /// Only the TRUE reading is control-free. A stamped `0x8000_0000` cannot come from our own
    /// posted words, so it stands on its own; FALSE can equally mean our invalidate went blind,
    /// which is what the caller's positive control is there to rule out.
    pub fn buffer_written(&self) -> bool {
        !self.buffer_rejected() && (self.overall != 0 || self.tag_code & 0x8000_0000 != 0)
    }
}

/// PI-V3D-81 — empty the read FIFO without waiting for anything, returning how many words were
/// discarded. Bounded by construction: a FIFO that will not empty is a fault, not a queue, and
/// spinning on one here would hang the boot the instrument is meant to describe.
///
/// The count is not optional bookkeeping. A doorbell that arrives after a leg's settle is the only
/// trace a late-replying tag leaves, and this function is what destroys it — so a caller that drops
/// the return has silently deleted the evidence for the one hazard the reply-less path admits to.
/// Print it.
///
/// MBOX-1: now a thin re-export of [`drain_read_fifo_bounded`], which the timeout path of
/// [`MboxLoan::call_budget`] shares — one drain primitive, one bound, so the reply-less instrument
/// and the ordinary transport cannot disagree about what "drained" means. Behaviour is unchanged
/// (same `DRAIN_MAX` = 64 cap). Deliberately loan-free: it touches only the FIFO, and the reply-less
/// legs call it between transactions.
#[cfg(feature = "v3d81")]
pub fn drain_property_fifo() -> u32 {
    drain_read_fifo_bounded()
}

/// PI-V3D-81 — post `MBOX.words[0..len]` on the property channel and DO NOT wait for a reply.
/// Settles `settle_ms` (bounded, measured), watching the read FIFO without requiring it, then
/// re-reads the request buffer out of RAM so the caller can see whether the VPU wrote a response
/// into it. Never blocks on the VideoCore: the only wait is the bounded write-FIFO-full wait and
/// the settle itself, both off CNTPCT.
#[cfg(feature = "v3d81")]
impl MboxLoan {
fn post_noreply(&mut self, len: usize, settle_ms: u64) -> NoReplySend {
    let frq = cntfrq();
    let elapsed_us = |from: u64| (super::timer::cntpct().wrapping_sub(from)).saturating_mul(1_000_000) / frq.max(1);

    // Anything already in the FIFO belongs to an earlier transaction; take it out of the way and
    // report it, so a reading taken over somebody else's late reply is never mistaken for a clean one.
    let stale_pre = drain_read_fifo_bounded();

    // Clean our request out to RAM — the GPU does not snoop our cache. Identical to `mbox_call`;
    // the divergence is entirely on the reply side.
    cache::clean_range(mbox_phys(), len * 4);

    let msg = ((mbox_phys() as u32 & BUS_TO_PHYS) | BUS_UNCACHED) | CH_PROP;
    let t0 = super::timer::cntpct();
    let post_deadline = t0 + frq / 1000 * 50; // 50 ms just to get INTO the write FIFO
    let mut posted = false;
    loop {
        if mmio_read(MBOX_STATUS) & MBOX_FULL == 0 {
            mmio_write(MBOX_WRITE, msg);
            posted = true;
            break;
        }
        if super::timer::cntpct() >= post_deadline {
            break;
        }
        core::hint::spin_loop();
    }

    // The settle. Not a wait for a reply — a stated interval during which the firmware may act, and
    // during which we notice a doorbell if one comes without needing it to. Skipped outright when
    // the post never landed: there is nothing for the firmware to act ON, so the whole interval (up
    // to 10 s) would buy a leg that already carries no reading nothing but a slower boot. The
    // measured `settle_us` then reads ~0, which is the honest record of what was actually waited.
    let t_post = super::timer::cntpct();
    let settle_end = if posted { t_post + frq / 1000 * settle_ms } else { t_post };
    let (mut doorbells, mut other_ch, mut first_doorbell_us) = (0u32, 0u32, 0u64);
    while super::timer::cntpct() < settle_end {
        if mmio_read(MBOX_STATUS) & MBOX_EMPTY == 0 {
            let w = mmio_read(MBOX_READ);
            if w & 0xF == CH_PROP {
                if doorbells == 0 {
                    first_doorbell_us = elapsed_us(t_post);
                }
                doorbells += 1;
            } else {
                other_ch += 1;
            }
        }
        core::hint::spin_loop();
    }
    let settle_us = elapsed_us(t_post);

    // Drop our stale cached copy so the words we read are the VPU's, not ours. Clean+invalidate for
    // the same reason `mbox_call` uses it: a still-dirty line is written back rather than discarded
    // over a reply. If these words come back as we posted them, the firmware wrote nothing — and the
    // caller's positive control is what turns that into a fact about the firmware.
    cache::clean_invalidate_range(mbox_phys(), len * 4);
    NoReplySend {
        posted,
        stale_pre,
        doorbells,
        first_doorbell_us,
        other_ch,
        settle_us,
        overall: self.reply(1),
        tag_code: self.reply(4),
        word5: self.reply(5),
        buf_pa: mbox_phys(),
    }
}
}

/// MBOX-1 — what a reply-less leg reports when the transport was already loaned: `posted = false`,
/// every reading zero. `posted` is the first field the doc tells a reader to check, and it is false
/// here for the honest reason — the message never reached the write FIFO.
#[cfg(feature = "v3d81")]
fn noreply_busy(op: &str) -> NoReplySend {
    witness_busy(op);
    NoReplySend {
        posted: false,
        stale_pre: 0,
        doorbells: 0,
        first_doorbell_us: 0,
        other_ch: 0,
        settle_us: 0,
        overall: 0,
        tag_code: 0,
        word5: 0,
        buf_pa: mbox_phys(),
    }
}

/// PI-V3D-81 — `SET_ENABLE_QPU` posted reply-less. [v3d75a]/P86 read this tag's silence as "tag
/// unhandled by this firmware" and closed the route; §46.5 reopened it, because that reading came
/// from `mbox_call` and a NOTIFY-class silence means only "no doorbell". Same message bytes as
/// [`set_enable_qpu`] — only the reply half differs, so a difference in outcome between the two is
/// a difference in how we LISTENED, not in what we asked.
#[cfg(feature = "v3d81_qpu")]
pub fn enable_qpu_noreply(enable: u32, settle_ms: u64) -> NoReplySend {
    let mut m = match claim() {
        Ok(loan) => loan,
        Err(_) => return noreply_busy("SET_ENABLE_QPU (reply-less)"),
    };
    m.request(0, 7 * 4); // total size (7 words used)
    m.request(1, 0); // request
    m.request(2, TAG_SET_ENABLE_QPU);
    m.request(3, 4); // value buffer size (1 word: enable)
    m.request(4, 0); // request code — the firmware stamps bit31|len here if it handles the tag
    m.request(5, enable);
    m.request(6, TAG_END);
    m.post_noreply(7, settle_ms)
}

/// PI-V3D-81 — `NOTIFY_DISPLAY_DONE` posted reply-less: the send P92/P93 could only ever mis-read.
/// Same zero-length message as [`notify_display_done`]; the difference is that this one does not
/// pretend the absence of a doorbell is the absence of an act. ⚠ If the tag IS acted on, the
/// firmware display driver stops and scanout of the mailbox-allocated framebuffer dies (black
/// panel; serial unaffected) — that is the hypothesis, not a malfunction.
#[cfg(feature = "v3d81_display")]
pub fn notify_display_done_noreply(settle_ms: u64) -> NoReplySend {
    let mut m = match claim() {
        Ok(loan) => loan,
        Err(_) => return noreply_busy("NOTIFY_DISPLAY_DONE (reply-less)"),
    };
    m.request(0, 6 * 4); // total size (6 words used)
    m.request(1, 0); // request
    m.request(2, TAG_NOTIFY_DISPLAY_DONE);
    m.request(3, 0); // value buffer size (zero-length, like vc4's NULL/0 call)
    m.request(4, 0); // request code — the firmware stamps bit31|len here if it handles the tag
    m.request(5, TAG_END);
    m.post_noreply(6, settle_ms)
}

/// PI-V3D-81 — the RAW display-mode query. [`query_display_size`] folds an implausible answer (0x0
/// included) into `None` alongside a transport failure, which is right for choosing a framebuffer
/// mode and fatal here: "the firmware reports no display mode any more" is precisely the reading
/// that would show a stopped display driver, and it must not be indistinguishable from a dead
/// mailbox. Same split, same reason, as `get_clock_rate_raw` vs `get_clock_rate`. `None` means
/// ONLY that the transaction failed; `Some((0, 0))` is a successful query reporting no mode.
#[cfg(feature = "v3d81")]
pub fn query_display_size_raw() -> Option<(u32, u32)> {
    let mut m = claim_or!("GET_PHYS_WH (raw display mode)", None);
    m.request(0, 8 * 4); // total size (8 words used)
    m.request(1, 0); // request
    m.request(2, TAG_GET_PHYS_WH);
    m.request(3, 8); // value buffer size
    m.request(4, 0); // request code
    m.request(5, 0); // width  (reply)
    m.request(6, 0); // height (reply)
    m.request(7, TAG_END);
    if !m.call(8) {
        return None; // transport failure — NOT a "no mode" answer
    }
    Some((m.reply(5), m.reply(6)))
}

/// PI-V3D-1: turn a firmware power domain on (`state = 1`) or off (`0`). Returns the state the
/// firmware reports back (it echoes the achieved state in the reply's `state` word), or `None` on a
/// mailbox failure. Used to power the V3D block (`POWER_DOMAIN_V3D`) before touching its registers —
/// with the domain off, V3D MMIO reads garbage. MBOX-1: claims the transport for the transaction and
/// fails loud on contention (a refused domain set is a diagnostic loss, not a wrong divider).
#[cfg(any(feature = "v3d", feature = "piusb"))]
pub fn set_power_domain(domain: u32, state: u32) -> Option<u32> {
    let mut m = claim_or!("SET_DOMAIN_STATE", None);
    m.request(0, 8 * 4); // total size (8 words used)
    m.request(1, 0); // request
    m.request(2, TAG_SET_DOMAIN_STATE);
    m.request(3, 8); // value buffer size (2 words: domain, state)
    m.request(4, 0); // request code
    m.request(5, domain);
    m.request(6, state); // reply preserves/echoes the achieved state
    m.request(7, TAG_END);
    if !m.call(8) {
        return None;
    }
    Some(m.reply(6))
}

/// PI-V3D-2: enable or disable a firmware-managed clock's GATE (RPi firmware property tag
/// `SET_CLOCK_STATE`, `0x00038001`). Distinct from `set_clock_rate`, which programs the *frequency*
/// but does NOT touch the enable gate — the firmware treats rate and state independently, so a clock
/// can have a rate set while still gated OFF. In that state the block it feeds is powered-but-unclocked
/// and its registers read open-bus poison (`0xdeadbeef`): exactly the PI-V3D-1 metal false-pass, where
/// power + rate both ACKed yet the V3D never decoded. Reply `state`: bit0 = clock active, bit1 = clock
/// not present. Returns `Some(true)` if the firmware reports the clock present AND active,
/// `Some(false)` if present-but-off, `None` on a mailbox failure or an absent clock.
#[cfg(any(feature = "v3d", feature = "piusb"))]
pub fn set_clock_state(clock_id: u32, on: bool) -> Option<bool> {
    let mut m = claim_or!("SET_CLOCK_STATE", None);
    m.request(0, 8 * 4); // total size (8 words used)
    m.request(1, 0); // request
    m.request(2, TAG_SET_CLOCK_STATE);
    m.request(3, 8); // value buffer size (2 words: clock_id, state)
    m.request(4, 0); // request code
    m.request(5, clock_id);
    m.request(6, if on { 1 } else { 0 }); // reply: achieved state (bit0 active, bit1 not-present)
    m.request(7, TAG_END);
    if !m.call(8) {
        return None;
    }
    let state = m.reply(6);
    if state & 0x2 != 0 {
        return None; // firmware reports the clock not present
    }
    Some(state & 0x1 != 0)
}

/// PIUSB-32: the read-only power/clock STATE witnesses. Each returns the raw firmware `state` reply word
/// (bit0 = on/active, bit1 = "does-not-exist") so the census can print the whole word, or `None` on a
/// mailbox transport failure. Query-only: none of these three tags mutates firmware state — they are the
/// diagnostic companions to `set_power_domain` / `set_clock_state`. MBOX-1: each claims the transport
/// for its transaction and fails loud on contention — a census reading taken over somebody else's
/// buffer would be worse than no reading.
#[cfg(feature = "piusb")]
pub fn get_power_state(device_id: u32) -> Option<u32> {
    let mut m = claim_or!("GET_POWER_STATE", None);
    m.request(0, 8 * 4);
    m.request(1, 0);
    m.request(2, TAG_GET_POWER_STATE);
    m.request(3, 8); // value buffer (2 words: device_id, state)
    m.request(4, 0);
    m.request(5, device_id);
    m.request(6, 0); // state (reply)
    m.request(7, TAG_END);
    if !m.call(8) {
        return None;
    }
    Some(m.reply(6))
}

/// PIUSB-32: read-only power-DOMAIN state (the same domain-id namespace `set_power_domain` writes —
/// e.g. `POWER_DOMAIN_V3D`). Returns the firmware `state` word (bit0 on, bit1 does-not-exist).
#[cfg(feature = "piusb")]
pub fn get_domain_state(domain: u32) -> Option<u32> {
    let mut m = claim_or!("GET_DOMAIN_STATE", None);
    m.request(0, 8 * 4);
    m.request(1, 0);
    m.request(2, TAG_GET_DOMAIN_STATE);
    m.request(3, 8); // value buffer (2 words: domain, state)
    m.request(4, 0);
    m.request(5, domain);
    m.request(6, 0); // state (reply)
    m.request(7, TAG_END);
    if !m.call(8) {
        return None;
    }
    Some(m.reply(6))
}

/// PIUSB-32: read-only CLOCK gate state (the clock-id namespace `set_clock_state` writes — e.g.
/// `CLOCK_ID_V3D`). Returns the firmware `state` word (bit0 active, bit1 does-not-exist).
/// PI-V3D-55: also compiled for the `v3d` feature — the clock-domain audit reads the V3D gate state
/// directly, and must not depend on `piusb` happening to be enabled in the same build.
#[cfg(any(feature = "piusb", feature = "v3d"))]
pub fn get_clock_state(clock_id: u32) -> Option<u32> {
    let mut m = claim_or!("GET_CLOCK_STATE", None);
    m.request(0, 8 * 4);
    m.request(1, 0);
    m.request(2, TAG_GET_CLOCK_STATE);
    m.request(3, 8); // value buffer (2 words: clock_id, state)
    m.request(4, 0);
    m.request(5, clock_id);
    m.request(6, 0); // state (reply)
    m.request(7, TAG_END);
    if !m.call(8) {
        return None;
    }
    Some(m.reply(6))
}

/// PI-V3D-55: the RAW `GET_CLOCK_RATE` query — transport failure and a genuine **0 Hz grant** are
/// DIFFERENT facts and this is the only caller that must tell them apart. `get_clock_rate` above
/// deliberately collapses both into `None` (its EMMC2 caller wants "usable rate or nothing"), which
/// would make the single most diagnostic V3D reading — the firmware granting the V3D clock 0 Hz —
/// indistinguishable from a dead mailbox, and would render the 0 Hz verdict arm unreachable.
///
/// Returns `None` **only** when `mbox_call` itself fails (no reply / bad response code); `Some(0)` is a
/// SUCCESSFUL transaction reporting a real zero rate. Additive: `get_clock_rate`'s existing contract
/// and its callers are untouched.
#[cfg(feature = "v3d")]
pub fn get_clock_rate_raw(clock_id: u32) -> Option<u32> {
    let mut m = claim_or!("GET_CLOCK_RATE (raw)", None);
    m.request(0, 8 * 4); // total size (8 words used)
    m.request(1, 0); // request
    m.request(2, TAG_GET_CLOCK_RATE);
    m.request(3, 8); // value buffer size (2 words: clock_id, rate)
    m.request(4, 0); // request code
    m.request(5, clock_id); // clock id (reply preserves it)
    m.request(6, 0); // rate (reply)
    m.request(7, TAG_END);
    if !m.call(8) {
        return None; // transport failure — NOT a 0 Hz grant
    }
    Some(m.reply(6)) // a successful transaction, rate verbatim (0 included)
}

/// PI-V3D-1: set a firmware-managed clock's rate in Hz. Returns the rate the firmware actually
/// programmed (it may clamp to the clock's min/max), or `None` on a mailbox failure. Trap closed by
/// the caller: a V3D domain that is powered but whose clock was never set reads garbage registers —
/// always power THEN clock, in that order. `skip_turbo = 0` lets the firmware raise other clocks as
/// needed.
#[cfg(feature = "v3d")]
pub fn set_clock_rate(clock_id: u32, rate_hz: u32) -> Option<u32> {
    let mut m = claim_or!("SET_CLOCK_RATE", None);
    m.request(0, 9 * 4); // total size (9 words used)
    m.request(1, 0); // request
    m.request(2, TAG_SET_CLOCK_RATE);
    m.request(3, 12); // value buffer size (3 words: clock_id, rate, skip_turbo)
    m.request(4, 0); // request code
    m.request(5, clock_id);
    m.request(6, rate_hz); // reply: the rate actually set
    m.request(7, 0); // skip_turbo = 0
    m.request(8, TAG_END);
    if !m.call(9) {
        return None;
    }
    let set = m.reply(6);
    if set == 0 { None } else { Some(set) }
}

/// PI-USB-1: ask the VideoCore firmware to reset the VL805 xHCI (RPi firmware property tag
/// `NOTIFY_XHCI_RESET`, `0x00030058`). `dev_addr` is the VL805's PCI device address
/// `(bus<<20)|(dev<<15)|(fn<<12)` — `0x0010_0000` for bus 1, dev 0, fn 0. The firmware re-runs the
/// VL805 reset/firmware-load it does at cold boot; a bringing-up OS issues it after enumerating the
/// controller and before attaching the xHCI driver. Returns whether the mailbox reported success.
/// Single-user `MBOX` like the other calls (boot-time, single-threaded, framebuffer call long done).
///
/// PIUSB-12: the full response witness for one NOTIFY. The RPi property protocol writes THREE
/// distinct status words the caller must separate to localise a no-op:
///   * `overall` = buffer word 1: the whole-message code. `0x8000_0000` = processed OK,
///     `0x8000_0001` = "error parsing request buffer" (malformed message). `ok` is `overall ==
///     0x8000_0000`. This alone does NOT prove any individual tag was honoured.
///   * `tag_code` = the tag's own request/response word (our buffer word 4). On REQUEST it is 0;
///     on a honoured RESPONSE the firmware sets bit 31 and puts the returned byte-length in bits
///     [30:0] (so a 4-byte response reads `0x8000_0004`). If the firmware does NOT recognise /
///     act on the tag, it leaves bit 31 CLEAR here. **This is the discriminating word** — it, not
///     the value echo, says whether the VideoCore actually ran the VL805 reset/load handler.
///   * `echo` = the value-buffer word (our buffer word 5). For NOTIFY_XHCI_RESET the firmware does
///     NOT echo `dev_addr` back — it is a command tag, and on metal (boot-P21) it returned 0 even
///     with the mailbox reporting success. Because we POST `dev_addr` (non-zero) into this slot, a
///     read-back of 0 also PROVES cache invalidation worked (a stale cached read would return our
///     own posted `dev_addr`, not 0), so `echo` is a cache-coherency witness, not a load witness.
/// `buf_pa` is the ARM-physical address of the message buffer (for correlating the raw dump).
#[cfg(feature = "piusb")]
pub struct NotifyResp {
    pub ok: bool,
    pub overall: u32,
    pub tag_code: u32,
    pub echo: u32,
    pub buf_pa: usize,
}

/// PI-USB-1 / PIUSB-12: ask the firmware to reset+reload the VL805 xHCI (tag `NOTIFY_XHCI_RESET`,
/// `0x00030058`), `dev_addr = (bus<<20)|(dev<<15)|(fn<<12)` = `0x0010_0000` for bus1/dev0/fn0.
/// Returns the full [`NotifyResp`] so the caller can print the per-word witness that discriminates
/// "tag honoured, VL805 reset ran" (tag_code bit31 set) from "tag silently dropped" (bit31 clear).
/// Linux (`rpi_reset_reset`) checks only the overall return code; we witness every word because the
/// CNR-never-clears wall means we must know WHICH layer is failing.
///
/// MBOX-1 — this is the call the dirty-timeout defect hurt most, and the reason the fix is not
/// cosmetic. It reads its three witness words on EVERY path, `ok` or not. Before MBOX-1 a timed-out
/// call left the buffer's cache lines holding the words we posted, so the failure-path read of
/// `echo` returned our own `dev_addr` — and PIUSB-12 above documents a zero there as the PROOF that
/// cache invalidation worked. The witness inverted itself exactly when it was most needed. The
/// timeout path now invalidates (see [`MboxLoan::timeout_clean`]), so these words are the GPU's
/// memory on both paths.
///
/// On contention the transport is not claimed at all: `ok = false` with every word zero, preceded by
/// the `BUSY` witness line. Fail loud — `notify_before_reset` already prints the full word set, so a
/// refusal is visible rather than inferred.
#[cfg(feature = "piusb")]
pub fn notify_xhci_reset(dev_addr: u32) -> NotifyResp {
    let mut m = match claim() {
        Ok(loan) => loan,
        Err(_) => {
            witness_busy("NOTIFY_XHCI_RESET");
            return NotifyResp { ok: false, overall: 0, tag_code: 0, echo: 0, buf_pa: mbox_phys() };
        }
    };
    m.request(0, 7 * 4); // total size (7 words used)
    m.request(1, 0); // request
    m.request(2, TAG_NOTIFY_XHCI_RESET);
    m.request(3, 4); // value buffer size (1 word: the PCI device address)
    m.request(4, 0); // request code (firmware overwrites with 0x8000_0000|len if honoured)
    m.request(5, dev_addr);
    m.request(6, TAG_END);
    let ok = m.call(7);
    NotifyResp {
        ok,
        overall: m.reply(1),
        tag_code: m.reply(4),
        echo: m.reply(5),
        buf_pa: mbox_phys(),
    }
}

/// WC-E — the SCAN-OUT GROUND TRUTH witness. One line, once, at framebuffer bring-up, carrying every
/// geometry field the display pipe actually scans with, read back FROM THE FIRMWARE rather than echoed
/// from what we asked for.
///
/// ### Why this exists
/// `init_framebuffer` sends five setters and keeps the values it REQUESTED (`width`/`height`/
/// `DEPTH_BITS`/`PIXEL_ORDER_BGR`/offset 0,0), reading back only base, size and pitch. The firmware may
/// clamp a size to the panel's mode, round a pitch up for alignment, refuse a depth, or ignore a pixel
/// order — and every one of those leaves our `FrameBufferInfo` describing a framebuffer that is not the
/// one the HVS is scanning. The whole class shows up on the panel as garble (a pitch or virtual-width
/// divergence shifts every row's phase) or as wrong colours (an order divergence), and shows up in the
/// serial log as absolutely nothing: the compositor's own read-back witness (`wm::verify_window`) reads
/// through the SAME `info.stride` it wrote through, so it agrees with itself no matter what the hardware
/// is doing. A witness that asks OUR numbers can never falsify our numbers. This one asks the firmware's.
///
/// ### What a reader can conclude
/// `req=` is what we asked for; every other field is the firmware's answer. Agreement retires the whole
/// scan-out-geometry suspect surface for that boot — pitch, depth, order, virtual size, viewport offset,
/// base alignment — and is the precondition for blaming anything downstream. A divergence names itself.
/// `pitch` is repeated from the allocation reply so one line carries the complete picture, and
/// `pitch == virt_w * bpp` is the specific identity a row-phase garble breaks.
///
/// Query-only tags: this changes no firmware state and is safe to run after the allocation. Failure to
/// call is non-fatal — it prints a FAIL line and returns, because a diagnostic must never be able to
/// take down the boot it is diagnosing.
fn witness_fb_geometry(base: u64, size: u32, pitch: u32, req_w: u32, req_h: u32) {
    // MBOX-1: the loan covers ONE transaction — fill, call, read back — and is dropped before
    // `query_pitch` below, which is a transaction of its own and claims for itself.
    let mut m = match claim() {
        Ok(loan) => loan,
        Err(_) => {
            witness_busy("fb-geometry query");
            serial_println!("[wc-e] fb-geometry query FAILED — scan-out ground truth unavailable");
            return;
        }
    };
    let mut i = 0usize;
    macro_rules! put {
        ($val:expr) => {{
            m.request(i, $val);
            i += 1;
        }};
    }
    put!(0); // total size, patched below
    put!(0); // request

    put!(TAG_GET_PHYS_WH);
    put!(8);
    put!(0);
    let phys_idx = i;
    put!(0);
    put!(0);

    put!(TAG_GET_VIRT_WH);
    put!(8);
    put!(0);
    let virt_idx = i;
    put!(0);
    put!(0);

    put!(TAG_GET_VIRT_OFFSET);
    put!(8);
    put!(0);
    let off_idx = i;
    put!(0);
    put!(0);

    put!(TAG_GET_DEPTH);
    put!(4);
    put!(0);
    let depth_idx = i;
    put!(0);

    put!(TAG_GET_PIXEL_ORDER);
    put!(4);
    put!(0);
    let order_idx = i;
    put!(0);

    put!(TAG_GET_ALPHA_MODE);
    put!(4);
    put!(0);
    let alpha_idx = i;
    put!(0);

    put!(TAG_END);

    let total = i;
    m.request(0, (total * 4) as u32);
    if !m.call(total) {
        serial_println!("[wc-e] fb-geometry query FAILED — scan-out ground truth unavailable");
        return;
    }

    let (phys_w, phys_h) = (m.reply(phys_idx), m.reply(phys_idx + 1));
    let (virt_w, virt_h) = (m.reply(virt_idx), m.reply(virt_idx + 1));
    let (off_x, off_y) = (m.reply(off_idx), m.reply(off_idx + 1));
    let depth = m.reply(depth_idx);
    let order = m.reply(order_idx);
    let alpha = m.reply(alpha_idx);
    // MBOX-1: every reply word is now in a local — release the transport before `query_pitch`, which
    // is a separate transaction and claims for itself. Holding across it would self-deny.
    drop(m);
    let bpp = (depth / 8).max(1);
    // The two identities a scan-out defect breaks. `row_ok`: the pitch the HVS steps by must be exactly
    // one virtual row of pixels — anything else shears every row against the one above it. `fit_ok`: the
    // allocation must hold the whole visible image, or the bottom rows scan out of somebody else's memory.
    let row_ok = pitch == virt_w * bpp;
    let fit_ok = (size as u64) >= (pitch as u64) * (virt_h as u64);
    serial_println!(
        "[wc-e] fb-geometry req={}x{}@{}bpp order={} :: firmware phys={}x{} virt={}x{} offset=({},{}) depth={} order={} alpha={} pitch={}B base={:#x} size={} align={} row_ok={} fit_ok={}",
        req_w, req_h, DEPTH_BITS, PIXEL_ORDER_BGR,
        phys_w, phys_h, virt_w, virt_h, off_x, off_y, depth, order, alpha,
        pitch, base, size, base & 0xFFF,
        row_ok, fit_ok
    );

    // WC-F — record the firmware's answers, and add the one field WC-E never asked for: a pitch read
    // back in a TRANSACTION OF ITS OWN. Every pitch on the line above descends from the allocation
    // reply; a firmware that reported one pitch while programming the HVS with another would be
    // self-consistent there and wrong on the panel. This second `GET_PITCH` is the cheapest possible
    // independent read of the same number. Recording (rather than only printing) is what lets the
    // compositor-side witness compare the LIVE `FrameBuffer` — the object the blit path actually
    // addresses through — against the display pipe, which nothing in WC-D/WC-E ever did.
    #[cfg(feature = "witness")]
    {
        let get_pitch = query_pitch();
        // SAFETY: `SCANOUT` is written exactly once, here, during single-core boot (framebuffer
        // bring-up in `build_boot_info`, before SMP), and is read-only afterwards. Unlike `MBOX`
        // — which MBOX-1 moved behind a claim/loan because its single-user premise was false —
        // this static genuinely has one writer at one point in the boot.
        unsafe {
            core::ptr::write_volatile(
                &raw mut SCANOUT,
                ScanoutTruth {
                    valid: true,
                    alloc_base: base,
                    alloc_size: size,
                    alloc_pitch: pitch,
                    get_pitch,
                    phys_w, phys_h, virt_w, virt_h, off_x, off_y,
                    depth, order, alpha,
                },
            );
        }
    }
}

/// WC-F — the firmware's scan-out geometry, captured once at framebuffer bring-up.
///
/// Why a record and not just a log line: WC-E prints these numbers, and a human comparing them to the
/// compositor's numbers by eye is not a gate. The `FrameBuffer` the blit path addresses through is
/// built from the allocation reply and then lives on its own — its `base`, `len` and `stride` could
/// each drift from what the HVS scans (a remap, a clamp, a rounding) with nothing in WC-D or WC-E able
/// to notice, because both of those read through the very numbers under suspicion. Holding the
/// firmware's answers lets `video::wcf` put the two side by side on one line.
#[cfg(feature = "witness")]
#[derive(Clone, Copy)]
pub struct ScanoutTruth {
    /// False when the framebuffer never came up (or its geometry query failed): consumers must skip.
    pub valid: bool,
    /// ARM-physical base from the allocation reply (bus alias already masked off).
    pub alloc_base: u64,
    pub alloc_size: u32,
    /// Pitch as reported alongside the allocation.
    pub alloc_pitch: u32,
    /// Pitch re-read in a separate, query-only transaction. Must equal `alloc_pitch`.
    pub get_pitch: u32,
    pub phys_w: u32,
    pub phys_h: u32,
    pub virt_w: u32,
    pub virt_h: u32,
    pub off_x: u32,
    pub off_y: u32,
    pub depth: u32,
    pub order: u32,
    pub alpha: u32,
}

#[cfg(feature = "witness")]
static mut SCANOUT: ScanoutTruth = ScanoutTruth {
    valid: false,
    alloc_base: 0,
    alloc_size: 0,
    alloc_pitch: 0,
    get_pitch: 0,
    phys_w: 0,
    phys_h: 0,
    virt_w: 0,
    virt_h: 0,
    off_x: 0,
    off_y: 0,
    depth: 0,
    order: 0,
    alpha: 0,
};

/// WC-F — the recorded scan-out truth. `valid == false` means "never captured"; callers must not
/// invent a verdict from the zeroes.
#[cfg(feature = "witness")]
pub fn scanout_truth() -> ScanoutTruth {
    // SAFETY: written once during single-core boot, read-only afterwards.
    unsafe { core::ptr::read_volatile(&raw const SCANOUT) }
}

/// WC-F — pitch, asked on its own. Returns 0 if the query fails, which reads as a mismatch rather
/// than as agreement: a diagnostic must fail loud, never silently agree with the thing it checks.
#[cfg(feature = "witness")]
fn query_pitch() -> u32 {
    let mut m = claim_or!("GET_PITCH (independent read)", 0);
    m.request(0, 7 * 4);
    m.request(1, 0);
    m.request(2, TAG_GET_PITCH);
    m.request(3, 4);
    m.request(4, 0);
    m.request(5, 0);
    m.request(6, TAG_END);
    if !m.call(7) {
        return 0;
    }
    m.reply(5)
}

/// Bring up the VideoCore framebuffer: pick a resolution (the firmware's current mode if sane,
/// else 1920×1080), then set size/depth/pixel-order, allocate the buffer, and read back its base
/// and pitch. Returns the ARM-physical framebuffer for BootInfo, or `None` on any failure (the
/// caller then boots serial-only).
pub fn init_framebuffer() -> Option<FbAlloc> {
    let (width, height) = match (FORCE_W, FORCE_H) {
        // WC-D: a compile-time PANEL OVERRIDE, so the QEMU gate can run the compositor at the bench
        // panel's geometry. QEMU raspi4b's mode is 640x480; the bench Pi drives 1920x1200. The window
        // compositor's placement rule (`video::wm::place`) derives each window's integer upscale FROM
        // the panel size, so those two panels do not exercise the same code: a 128x128 window lands at
        // scale 1 on 640x480 and scale 4 on 1920x1200. Every scaled-blit defect is therefore invisible
        // to a 640x480 gate BY CONSTRUCTION. Forcing the geometry is what makes the gate able to see
        // what the bench sees. Off by default (0,0 => query the firmware, unchanged behaviour).
        (w, h) if w != 0 && h != 0 => (w, h),
        _ => query_display_size().unwrap_or((FALLBACK_W, FALLBACK_H)),
    };

    // A single message carrying every framebuffer tag. Layout per tag: id, value-buffer-size,
    // request-code(0), value words… A local macro (not a closure) appends words so we can still
    // read `i` between tags to capture the reply slots.
    //
    // MBOX-1: `query_display_size` above already ran and released its own loan; this claim covers
    // the allocation transaction only, and is released before `witness_fb_geometry` and the V3D
    // bring-up at the tail — both of which re-enter this module and would otherwise self-deny.
    let mut m = claim_or!("framebuffer allocate", None);
    let mut i = 0usize;
    macro_rules! put {
        ($val:expr) => {{
            m.request(i, $val);
            i += 1;
        }};
    }
    put!(0); // [0] total size, patched below
    put!(0); // [1] request

    put!(TAG_SET_PHYS_WH);
    put!(8);
    put!(0);
    put!(width);
    put!(height);

    put!(TAG_SET_VIRT_WH);
    put!(8);
    put!(0);
    put!(width);
    put!(height);

    put!(TAG_SET_VIRT_OFFSET);
    put!(8);
    put!(0);
    put!(0); // x
    put!(0); // y

    put!(TAG_SET_DEPTH);
    put!(4);
    put!(0);
    put!(DEPTH_BITS);

    put!(TAG_SET_PIXEL_ORDER);
    put!(4);
    put!(0);
    put!(PIXEL_ORDER_BGR);

    put!(TAG_ALLOCATE_FB);
    put!(8);
    put!(0);
    let alloc_base_idx = i; // reply: base then size land here
    put!(4096); // alignment (reply overwrites with base)
    put!(0); //          (reply overwrites with size)

    put!(TAG_GET_PITCH);
    put!(4);
    put!(0);
    let pitch_idx = i;
    put!(0); // reply: pitch (bytes per line)

    put!(TAG_END);

    let total = i;
    m.request(0, (total * 4) as u32);

    if !m.call(total) {
        serial_println!(":: MAILBOX: framebuffer allocate FAILED ::");
        return None;
    }

    let fb_bus = m.reply(alloc_base_idx);
    let fb_size = m.reply(alloc_base_idx + 1);
    let pitch = m.reply(pitch_idx);
    // MBOX-1: allocation transaction complete and every reply word copied out — hand the transport
    // back before anything below re-enters this module.
    drop(m);
    if fb_bus == 0 || fb_size == 0 || pitch == 0 {
        serial_println!(
            ":: MAILBOX: bad framebuffer reply (base={:#x} size={} pitch={}) ::",
            fb_bus, fb_size, pitch
        );
        return None;
    }

    // The reply base is a VC bus address; mask the alias to get the ARM physical address.
    let base = (fb_bus & BUS_TO_PHYS) as u64;
    let stride_px = (pitch / BYTES_PER_PIXEL) as usize; // pitch is in bytes; stride in pixels
    let info = FrameBufferInfo {
        width: width as usize,
        height: height as usize,
        stride: stride_px,
        bytes_per_pixel: BYTES_PER_PIXEL as usize,
        pixel_format: PixelFormat::Bgr,
    };
    serial_println!(
        ":: MAILBOX: framebuffer {}x{} pitch={}B stride={}px base={:#x} size={} ::",
        width, height, pitch, stride_px, base, fb_size
    );
    // WC-E: and immediately ask the firmware what it ACTUALLY set, so the line above (which reports what
    // we requested) can be checked against the display pipe instead of trusted. Runs on every boot.
    witness_fb_geometry(base, fb_size, pitch, width, height);

    // PI-V3D-1: with the framebuffer resolved, bring up the V3D (VideoCore VI) GPU — probe → MMU →
    // clear job — and blit the CPU-verified clear into THIS framebuffer as a visible witness. This is
    // the byte-identity-preserving call site: the trigger lives here (in the VideoCore mailbox driver,
    // at the tail of the last function) rather than in `main.rs`, because inserting a gated block into
    // the middle of `kernel_main` shifts the embedded panic-location line numbers of the aarch64 code
    // below it (a positional artifact that breaks the knob-off byte-identity gate); a gated call at the
    // end of this file shifts nothing. Single-threaded here (boot, pre-SMP, mailbox idle); the probe
    // degrades gracefully when V3D is absent (QEMU). `v3d`-gated: knob-off this call + the whole v3d
    // module vanish and `kernel8` is byte-identical to baseline. See arch_arm64.md §PI-V3D.
    #[cfg(feature = "v3d")]
    super::v3d::bringup(Some(super::v3d::FbTarget {
        base,
        size: fb_size as usize,
        width: info.width,
        height: info.height,
        stride_px: info.stride,
        bytes_per_pixel: info.bytes_per_pixel,
    }));

    Some(FbAlloc { base, size: fb_size as usize, info })
}
