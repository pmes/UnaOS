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
// with `read_volatile` *after* `mbox_call` returns — never through a `&mut` held across the call,
// which the optimiser could satisfy from a stale register.

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
const TAG_GET_CLOCK_RATE: u32 = 0x0003_0002; // M6g: query a clock's current rate (value buffer {id, rate})
// PI-V3D-1: firmware property tags for powering + clocking the VideoCore VI (V3D) block. These are
// the Pi-4/VC6 path (the legacy VC4 `Enable_QPU`/set-power tags are NOT used here). See
// arch_arm64.md §PI-V3D and the scout research of record. `v3d`-gated so a knob-off kernel8 is
// byte-identical to baseline (these are additive, not called on any default path).
#[cfg(any(feature = "v3d", feature = "piusb"))]
const TAG_SET_DOMAIN_STATE: u32 = 0x0003_8030; // set a power-domain's on/off state (value {domain, state})
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

/// The property message buffer. One static buffer, used only during single-core boot
/// (build_boot_info, before SMP/interrupts), so no locking is needed. The framebuffer message is
/// 35 words; 48 words (192 bytes) rounds the allocation up to whole 64-byte cache lines so the
/// clean/invalidate over it can't touch a neighbouring static's lines. **64-byte aligned** for the
/// same reason — `DC IVAC/CIVAC` operate on whole lines, so a sub-line-aligned buffer would let a
/// maintenance op clobber adjacent data; a line-aligned, line-padded buffer is self-contained. The
/// mailbox ABI's own 16-byte alignment requirement is subsumed by this.
#[repr(C, align(64))]
struct MboxBuf {
    words: [u32; 48],
}
static mut MBOX: MboxBuf = MboxBuf { words: [0; 48] };

#[inline]
fn mmio_read(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}
#[inline]
fn mmio_write(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

/// Base address of the `MBOX` static as a `usize` (for cache ranges and the post address).
#[inline]
fn mbox_phys() -> usize {
    &raw const MBOX as usize
}
/// Read reply word `idx` straight from RAM. Use only after `mbox_call`: the GPU wrote the reply
/// and `mbox_call` invalidated our cached copy, so this volatile load re-fetches the fresh value.
#[inline]
fn reply(idx: usize) -> u32 {
    unsafe { core::ptr::read_volatile(&raw const MBOX.words[idx]) }
}
/// Write request word `idx`. Volatile so the store is committed to memory before `mbox_call`'s
/// cache-clean pushes it to the GPU (no reliance on the optimiser ordering plain stores).
#[inline]
fn request(idx: usize, val: u32) {
    unsafe { core::ptr::write_volatile(&raw mut MBOX.words[idx], val) }
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
fn mbox_call(len: usize) -> bool {
    let buf_phys = mbox_phys() as u32;

    // Clean our request out to RAM so the GPU (which doesn't snoop our cache) sees it.
    cache::clean_range(mbox_phys(), len * 4);

    // ~500 ms budget — generous vs the microseconds a real reply takes. CNTPCT is monotonic and
    // won't wrap within any boot window, so a plain `>=` compare is sound.
    let deadline = super::timer::cntpct() + cntfrq() / 2;
    let timed_out = || super::timer::cntpct() >= deadline;

    // Post: VC bus address of the buffer in the high 28 bits, channel in the low 4. Match Circle's
    // BUS_ADDRESS exactly — strip any alias bits then OR the uncached alias — so a buffer that
    // happened to sit above 1 GiB still resolves (it can't here; the kernel is in low RAM). The
    // 64-byte alignment guarantees the low 4 bits are clear for the channel.
    let msg = ((buf_phys & BUS_TO_PHYS) | BUS_UNCACHED) | CH_PROP;
    while mmio_read(MBOX_STATUS) & MBOX_FULL != 0 {
        if timed_out() {
            serial_println!(":: MAILBOX: timeout waiting for write FIFO ::");
            return false;
        }
        core::hint::spin_loop();
    }
    mmio_write(MBOX_WRITE, msg);

    // Spin for *our* channel's reply (the property channel is the only one we use, but the FIFO is
    // shared, so match the channel and discard anything else).
    loop {
        while mmio_read(MBOX_STATUS) & MBOX_EMPTY != 0 {
            if timed_out() {
                serial_println!(":: MAILBOX: timeout waiting for reply ::");
                return false;
            }
            core::hint::spin_loop();
        }
        let resp = mmio_read(MBOX_READ);
        if resp & 0xF == CH_PROP {
            break;
        }
        if timed_out() {
            serial_println!(":: MAILBOX: timeout (only other-channel replies) ::");
            return false;
        }
    }

    // The GPU wrote its reply straight to RAM; drop our stale cached copy before reading it back.
    // Clean+invalidate (not bare invalidate) is the always-safe choice — if any line were still
    // dirty it's written back rather than discarded over the GPU's reply.
    cache::clean_invalidate_range(mbox_phys(), len * 4);
    reply(1) == MBOX_RESPONSE
}

/// Ask the firmware for the current physical (display) resolution — the mode it auto-set from the
/// monitor at boot. Returns `None` if the call fails or the size is implausible, so the caller
/// falls back to a fixed mode.
fn query_display_size() -> Option<(u32, u32)> {
    request(0, 8 * 4); // total size (8 words used)
    request(1, 0); // request
    request(2, TAG_GET_PHYS_WH);
    request(3, 8); // value buffer size
    request(4, 0); // request code
    request(5, 0); // width  (reply)
    request(6, 0); // height (reply)
    request(7, TAG_END);
    if !mbox_call(8) {
        return None;
    }
    let (width, height) = (reply(5), reply(6));
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
/// SDHCI base clock when the controller's own `CAPABILITIES` base-clock field reads zero. The `MBOX`
/// static is single-user: the boot framebuffer call is long done by SMP/probe time, and the probe runs
/// single-threaded on the BSP (before the loader task is spawned), so no lock is needed — same as the
/// framebuffer calls above.
pub fn get_clock_rate(clock_id: u32) -> Option<u32> {
    request(0, 8 * 4); // total size (8 words used)
    request(1, 0); // request
    request(2, TAG_GET_CLOCK_RATE);
    request(3, 8); // value buffer size (2 words: clock_id, rate)
    request(4, 0); // request code
    request(5, clock_id); // clock id (reply preserves it)
    request(6, 0); // rate (reply)
    request(7, TAG_END);
    if !mbox_call(8) {
        return None;
    }
    let rate = reply(6);
    if rate == 0 { None } else { Some(rate) }
}

/// PI-V3D-1: turn a firmware power domain on (`state = 1`) or off (`0`). Returns the state the
/// firmware reports back (it echoes the achieved state in the reply's `state` word), or `None` on a
/// mailbox failure. Used to power the V3D block (`POWER_DOMAIN_V3D`) before touching its registers —
/// with the domain off, V3D MMIO reads garbage. Single-user `MBOX` like the other calls: the V3D
/// bring-up runs single-threaded on the BSP after the boot framebuffer call is long done.
#[cfg(any(feature = "v3d", feature = "piusb"))]
pub fn set_power_domain(domain: u32, state: u32) -> Option<u32> {
    request(0, 8 * 4); // total size (8 words used)
    request(1, 0); // request
    request(2, TAG_SET_DOMAIN_STATE);
    request(3, 8); // value buffer size (2 words: domain, state)
    request(4, 0); // request code
    request(5, domain);
    request(6, state); // reply preserves/echoes the achieved state
    request(7, TAG_END);
    if !mbox_call(8) {
        return None;
    }
    Some(reply(6))
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
    request(0, 8 * 4); // total size (8 words used)
    request(1, 0); // request
    request(2, TAG_SET_CLOCK_STATE);
    request(3, 8); // value buffer size (2 words: clock_id, state)
    request(4, 0); // request code
    request(5, clock_id);
    request(6, if on { 1 } else { 0 }); // reply: achieved state (bit0 active, bit1 not-present)
    request(7, TAG_END);
    if !mbox_call(8) {
        return None;
    }
    let state = reply(6);
    if state & 0x2 != 0 {
        return None; // firmware reports the clock not present
    }
    Some(state & 0x1 != 0)
}

/// PIUSB-32: the read-only power/clock STATE witnesses. Each returns the raw firmware `state` reply word
/// (bit0 = on/active, bit1 = "does-not-exist") so the census can print the whole word, or `None` on a
/// mailbox transport failure. Query-only: none of these three tags mutates firmware state — they are the
/// diagnostic companions to `set_power_domain` / `set_clock_state`. Single-user `MBOX` like every other
/// call here (the census runs single-threaded on the BSP, boot framebuffer call long done).
#[cfg(feature = "piusb")]
pub fn get_power_state(device_id: u32) -> Option<u32> {
    request(0, 8 * 4);
    request(1, 0);
    request(2, TAG_GET_POWER_STATE);
    request(3, 8); // value buffer (2 words: device_id, state)
    request(4, 0);
    request(5, device_id);
    request(6, 0); // state (reply)
    request(7, TAG_END);
    if !mbox_call(8) {
        return None;
    }
    Some(reply(6))
}

/// PIUSB-32: read-only power-DOMAIN state (the same domain-id namespace `set_power_domain` writes —
/// e.g. `POWER_DOMAIN_V3D`). Returns the firmware `state` word (bit0 on, bit1 does-not-exist).
#[cfg(feature = "piusb")]
pub fn get_domain_state(domain: u32) -> Option<u32> {
    request(0, 8 * 4);
    request(1, 0);
    request(2, TAG_GET_DOMAIN_STATE);
    request(3, 8); // value buffer (2 words: domain, state)
    request(4, 0);
    request(5, domain);
    request(6, 0); // state (reply)
    request(7, TAG_END);
    if !mbox_call(8) {
        return None;
    }
    Some(reply(6))
}

/// PIUSB-32: read-only CLOCK gate state (the clock-id namespace `set_clock_state` writes — e.g.
/// `CLOCK_ID_V3D`). Returns the firmware `state` word (bit0 active, bit1 does-not-exist).
/// PI-V3D-55: also compiled for the `v3d` feature — the clock-domain audit reads the V3D gate state
/// directly, and must not depend on `piusb` happening to be enabled in the same build.
#[cfg(any(feature = "piusb", feature = "v3d"))]
pub fn get_clock_state(clock_id: u32) -> Option<u32> {
    request(0, 8 * 4);
    request(1, 0);
    request(2, TAG_GET_CLOCK_STATE);
    request(3, 8); // value buffer (2 words: clock_id, state)
    request(4, 0);
    request(5, clock_id);
    request(6, 0); // state (reply)
    request(7, TAG_END);
    if !mbox_call(8) {
        return None;
    }
    Some(reply(6))
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
    request(0, 8 * 4); // total size (8 words used)
    request(1, 0); // request
    request(2, TAG_GET_CLOCK_RATE);
    request(3, 8); // value buffer size (2 words: clock_id, rate)
    request(4, 0); // request code
    request(5, clock_id); // clock id (reply preserves it)
    request(6, 0); // rate (reply)
    request(7, TAG_END);
    if !mbox_call(8) {
        return None; // transport failure — NOT a 0 Hz grant
    }
    Some(reply(6)) // a successful transaction, rate verbatim (0 included)
}

/// PI-V3D-1: set a firmware-managed clock's rate in Hz. Returns the rate the firmware actually
/// programmed (it may clamp to the clock's min/max), or `None` on a mailbox failure. Trap closed by
/// the caller: a V3D domain that is powered but whose clock was never set reads garbage registers —
/// always power THEN clock, in that order. `skip_turbo = 0` lets the firmware raise other clocks as
/// needed.
#[cfg(feature = "v3d")]
pub fn set_clock_rate(clock_id: u32, rate_hz: u32) -> Option<u32> {
    request(0, 9 * 4); // total size (9 words used)
    request(1, 0); // request
    request(2, TAG_SET_CLOCK_RATE);
    request(3, 12); // value buffer size (3 words: clock_id, rate, skip_turbo)
    request(4, 0); // request code
    request(5, clock_id);
    request(6, rate_hz); // reply: the rate actually set
    request(7, 0); // skip_turbo = 0
    request(8, TAG_END);
    if !mbox_call(9) {
        return None;
    }
    let set = reply(6);
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
#[cfg(feature = "piusb")]
pub fn notify_xhci_reset(dev_addr: u32) -> NotifyResp {
    request(0, 7 * 4); // total size (7 words used)
    request(1, 0); // request
    request(2, TAG_NOTIFY_XHCI_RESET);
    request(3, 4); // value buffer size (1 word: the PCI device address)
    request(4, 0); // request code (firmware overwrites with 0x8000_0000|len if honoured)
    request(5, dev_addr);
    request(6, TAG_END);
    let ok = mbox_call(7);
    NotifyResp { ok, overall: reply(1), tag_code: reply(4), echo: reply(5), buf_pa: mbox_phys() }
}

/// Bring up the VideoCore framebuffer: pick a resolution (the firmware's current mode if sane,
/// else 1920×1080), then set size/depth/pixel-order, allocate the buffer, and read back its base
/// and pitch. Returns the ARM-physical framebuffer for BootInfo, or `None` on any failure (the
/// caller then boots serial-only).
pub fn init_framebuffer() -> Option<FbAlloc> {
    let (width, height) = query_display_size().unwrap_or((FALLBACK_W, FALLBACK_H));

    // A single message carrying every framebuffer tag. Layout per tag: id, value-buffer-size,
    // request-code(0), value words… A local macro (not a closure) appends words so we can still
    // read `i` between tags to capture the reply slots.
    let mut i = 0usize;
    macro_rules! put {
        ($val:expr) => {{
            request(i, $val);
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
    request(0, (total * 4) as u32);

    if !mbox_call(total) {
        serial_println!(":: MAILBOX: framebuffer allocate FAILED ::");
        return None;
    }

    let fb_bus = reply(alloc_base_idx);
    let fb_size = reply(alloc_base_idx + 1);
    let pitch = reply(pitch_idx);
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
