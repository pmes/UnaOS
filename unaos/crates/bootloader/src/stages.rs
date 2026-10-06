// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (B490 LOADERSTALL: the loader's own lines; the record rides the boot-info seam)
//!
//! LOADERSTALL (rmbp-ledger B490) — the x86 UEFI loader says its own phase. Flight 26 sat on "starting"
//! for minutes with nothing on the wire, and nothing the loader did was visible anywhere: its only output
//! was ConOut, which Apple's firmware does not put on the glass. So:
//!
//! * **Panel** — one line per stage at the bottom of the GOP framebuffer, drawn with the font8x8 cell
//!   (scaled to the panel): `loader wire=<efi-serial|com1|none> tsc=<n>MHz`, then `firmware ok <ms>ms`,
//!   `gop <w>x<h> <ms>ms`, `volumes <n> <ms>ms`, `kernel <bytes> <ms>ms`, `elf <ms>ms`, `discover <ms>ms`,
//!   `jumping <ms>ms`. The lines stay until the kernel's splash covers them, so a stall with these lines
//!   and no splash is the loader's; a stall on the splash is the kernel's.
//! * **Wire** — the same lines on the UART the loader can drive: the firmware's EFI SerialIo when it
//!   publishes one, else the chipset 16550 at COM1 when its scratch register answers, else none (the
//!   FTDI is USB and the kernel's). Which one is said on the panel's first line.
//! * **Timeouts** — a 1 s firmware timer event (TPL_CALLBACK) watches the running stage against its
//!   budget; past it, it prints `loader: TIMEOUT stage=<s> ms=<n> last=<status>` on panel + UART, then
//!   again every 5 s while the stage still runs. A firmware call that never returns cannot be given up
//!   from inside it; the watchdog names it. The kernel read is chunked (1 MiB; `last` = bytes so far) and
//!   a failed chunk is retried three times before the loader gives up.
//! * **Hand-over** — `unaos_boot_info::LoaderStages` (raw TSC per stage; the kernel converts with its own
//!   measured rate and prints `:: LOADER:`; `tests loader` reads it back).
//!
//! x86_64 only (`mod stages` is `cfg`-gated), so the aarch64 `bootloader.efi` does not move.

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use uefi::boot::{self, EventType, OpenProtocolAttributes, OpenProtocolParams, SearchType, TimerTrigger, Tpl};
use uefi::proto::console::serial::Serial;
use uefi::proto::media::file::RegularFile;
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::{Event, Status};
use unaos_boot_info::{FrameBufferInfo, LoaderStages, LOADER_STAGES_MAGIC, LOADER_STAGE_MAX, LOADER_STAGE_NAMES, LOADER_UART_NAMES};

pub const FIRMWARE: u32 = 0;
pub const GOP: u32 = 1;
pub const VOLUMES: u32 = 2;
pub const KERNEL: u32 = 3;
pub const ELF: u32 = 4;
pub const DISCOVER: u32 = 5;
pub const JUMP: u32 = 6;

/// Per-stage budget (ms). The healthy bench numbers (flight 26): kernel read 2191 ms, the rest tens.
const BUDGET_MS: [u64; 7] = [5_000, 5_000, 5_000, 15_000, 5_000, 5_000, 3_000];
/// After the first TIMEOUT line, one more every this many ms while the stage still runs.
const REPEAT_MS: u64 = 5_000;
/// The kernel read's chunk.
const CHUNK: usize = 1 << 20;
/// Panel rows: header + seven stages + the TIMEOUT row.
const ROWS: u32 = 9;
const TIMEOUT_ROW: u32 = 8;

/// The record. Written by the main path only; the watchdog reads atomics, never this.
static mut REC: LoaderStages = LoaderStages::EMPTY;

static CUR: AtomicU32 = AtomicU32::new(u32::MAX);
static T0: AtomicU64 = AtomicU64::new(0);
static LAST: AtomicU32 = AtomicU32::new(0);
static HZ: AtomicU64 = AtomicU64::new(0);
static TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static WARNED: AtomicU32 = AtomicU32::new(0);
static UART: AtomicU32 = AtomicU32::new(0);
static SERIAL_IF: AtomicU64 = AtomicU64::new(0);
static UART_BUSY: AtomicBool = AtomicBool::new(false);
static EVENT: AtomicU64 = AtomicU64::new(0);
static ENTRY: AtomicU64 = AtomicU64::new(0);
static FB_ADDR: AtomicU64 = AtomicU64::new(0);
static FB_W: AtomicU64 = AtomicU64::new(0);
static FB_H: AtomicU64 = AtomicU64::new(0);
static FB_STRIDE: AtomicU64 = AtomicU64::new(0);

fn rdtsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn ms(cy: u64) -> u64 {
    let hz = HZ.load(Ordering::Relaxed);
    if hz < 1000 { 0 } else { cy / (hz / 1000) }
}

/// A fixed line buffer (no allocation: the jump-stage line is formatted with boot services gone).
struct Line {
    buf: [u8; 128],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Line { buf: [0; 128], len: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len < self.buf.len() {
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

// ---- the UART ---------------------------------------------------------------------------------

const COM1: u16 = 0x3F8;

unsafe fn outb(port: u16, v: u8) {
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags)) };
}

unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags)) };
    v
}

/// The firmware's SerialIo, else COM1 when its scratch register answers, else none.
fn find_uart() -> u32 {
    if let Ok(handles) = boot::locate_handle_buffer(SearchType::from_proto::<Serial>()) {
        if let Some(&h) = handles.first() {
            // Non-exclusive: the firmware's console splitter may hold this handle BY_DRIVER.
            let opened = unsafe {
                boot::open_protocol::<Serial>(
                    OpenProtocolParams { handle: h, agent: boot::image_handle(), controller: None },
                    OpenProtocolAttributes::GetProtocol,
                )
            };
            if let Ok(mut sp) = opened {
                let p: *mut Serial = &mut *sp;
                // GET_PROTOCOL needs no close; keep the interface for the loader's lifetime.
                core::mem::forget(sp);
                SERIAL_IF.store(p as u64, Ordering::Relaxed);
                return 1;
            }
        }
    }
    unsafe {
        outb(COM1 + 7, 0x5A);
        let present = inb(COM1 + 7) == 0x5A && inb(COM1 + 5) != 0xFF;
        if present {
            outb(COM1 + 1, 0x00); // no interrupts
            outb(COM1 + 3, 0x80); // DLAB
            outb(COM1, 0x01); // 115200
            outb(COM1 + 1, 0x00);
            outb(COM1 + 3, 0x03); // 8N1
            outb(COM1 + 2, 0xC7); // FIFO on, cleared
            outb(COM1 + 4, 0x03); // DTR + RTS
            return 2;
        }
    }
    0
}

/// Write one line to the UART. Skipped (not interleaved) when the other context is mid-line.
fn uart_line(s: &str) {
    if UART_BUSY.swap(true, Ordering::Acquire) {
        return;
    }
    match UART.load(Ordering::Relaxed) {
        1 => {
            let p = SERIAL_IF.load(Ordering::Relaxed) as *mut Serial;
            if !p.is_null() && boot_services_live() {
                let sp = unsafe { &mut *p };
                let _ = sp.write(s.as_bytes());
                let _ = sp.write(b"\r\n");
            }
        }
        2 => {
            for &b in s.as_bytes().iter().chain(b"\r\n".iter()) {
                unsafe {
                    let mut spin = 0u32;
                    while inb(COM1 + 5) & 0x20 == 0 && spin < 100_000 {
                        spin += 1;
                    }
                    outb(COM1, b);
                }
            }
        }
        _ => {}
    }
    UART_BUSY.store(false, Ordering::Release);
}

static BS_GONE: AtomicBool = AtomicBool::new(false);
fn boot_services_live() -> bool {
    !BS_GONE.load(Ordering::Relaxed)
}

// ---- the panel --------------------------------------------------------------------------------

fn glyph(c: char) -> [u8; 8] {
    let u = c as usize;
    if u < 128 {
        font8x8::legacy::BASIC_LEGACY[u]
    } else if (0xA0..0x100).contains(&u) {
        font8x8::legacy::LATIN_LEGACY[u - 0xA0]
    } else {
        [0; 8]
    }
}

/// Paint `text` on panel row `row` (white on black; format-independent).
fn panel_row(row: u32, text: &str) {
    let base = FB_ADDR.load(Ordering::Relaxed);
    let (w, h, stride) = (
        FB_W.load(Ordering::Relaxed) as usize,
        FB_H.load(Ordering::Relaxed) as usize,
        FB_STRIDE.load(Ordering::Relaxed) as usize,
    );
    if base == 0 || w < 320 || h < 240 || stride < w {
        return;
    }
    let scale = core::cmp::max(1, w / 960);
    let cell = 8 * scale;
    let line_h = cell + 2 * scale;
    let x0 = 2 * cell;
    let top = h.saturating_sub((ROWS as usize + 1) * line_h);
    let y0 = top + row as usize * line_h;
    if y0 + line_h > h {
        return;
    }
    let px = base as *mut u32;
    let right = w - x0;
    for y in y0..y0 + line_h {
        for x in x0..right {
            unsafe { px.add(y * stride + x).write_volatile(0) };
        }
    }
    let mut x = x0;
    for c in text.chars() {
        if x + cell > right {
            break;
        }
        let g = glyph(c);
        for (gy, bits) in g.iter().enumerate() {
            for gx in 0..8 {
                if bits & (1 << gx) != 0 {
                    for sy in 0..scale {
                        let yy = y0 + scale + gy * scale + sy;
                        let row_p = yy * stride + x + gx * scale;
                        for sx in 0..scale {
                            unsafe { px.add(row_p + sx).write_volatile(0x00FF_FFFF) };
                        }
                    }
                }
            }
        }
        x += cell;
    }
}

fn say(row: u32, text: &str) {
    panel_row(row, text);
    uart_line(text);
}

// ---- the stages -------------------------------------------------------------------------------

/// Closes the watchdog on every return to the firmware (its notify function lives in this image).
pub struct Watchdog;

impl Drop for Watchdog {
    fn drop(&mut self) {
        disarm();
    }
}

fn disarm() {
    let e = EVENT.swap(0, Ordering::AcqRel);
    if e != 0 {
        if let Some(ev) = unsafe { Event::from_ptr(e as *mut core::ffi::c_void) } {
            let _ = boot::close_event(ev);
        }
    }
}

unsafe extern "efiapi" fn watchdog(_e: Event, _ctx: Option<core::ptr::NonNull<core::ffi::c_void>>) {
    let cur = CUR.load(Ordering::Relaxed);
    if cur as usize >= BUDGET_MS.len() || HZ.load(Ordering::Relaxed) < 1000 {
        return;
    }
    let el = ms(rdtsc().wrapping_sub(T0.load(Ordering::Relaxed)));
    let w = WARNED.load(Ordering::Relaxed) as u64;
    if el < BUDGET_MS[cur as usize] + w * REPEAT_MS {
        return;
    }
    if w == 0 {
        TIMEOUTS.fetch_add(1, Ordering::Relaxed);
    }
    WARNED.store(w as u32 + 1, Ordering::Relaxed);
    let mut l = Line::new();
    let _ = write!(
        l,
        "loader: TIMEOUT stage={} ms={} last={} budget={}",
        LOADER_STAGE_NAMES[cur as usize], el, LAST.load(Ordering::Relaxed), BUDGET_MS[cur as usize]
    );
    say(TIMEOUT_ROW, l.as_str());
}

fn rec() -> &'static mut LoaderStages {
    unsafe { &mut *core::ptr::addr_of_mut!(REC) }
}

fn stage_line(id: u32, el: u64) -> Line {
    let r = rec();
    let mut l = Line::new();
    let name = LOADER_STAGE_NAMES[id as usize];
    let _ = match id {
        FIRMWARE => write!(l, "firmware ok {}ms", el),
        GOP => write!(l, "gop {}x{} {}ms", FB_W.load(Ordering::Relaxed), FB_H.load(Ordering::Relaxed), el),
        VOLUMES => write!(l, "volumes {} {}ms", r.volumes, el),
        KERNEL => write!(l, "kernel {} {}ms retries={}", r.kernel_bytes, el, r.retries),
        JUMP => write!(l, "jumping {}ms", el),
        _ => write!(l, "{} {}ms", name, el),
    };
    l
}

/// Record the end of stage `id` with its last status, say its line, start the next stage.
pub fn end(id: u32, last: u32) {
    let now = rdtsc();
    let r = rec();
    let i = r.count as usize;
    if i < LOADER_STAGE_MAX {
        r.id[i] = id as u8;
        r.end_tsc[i] = now;
        r.last_status[i] = last;
        r.count += 1;
    }
    let el = ms(now.wrapping_sub(T0.load(Ordering::Relaxed)));
    let l = stage_line(id, el);
    say(1 + id, l.as_str());
    T0.store(now, Ordering::Relaxed);
    LAST.store(0, Ordering::Relaxed);
    WARNED.store(0, Ordering::Relaxed);
    CUR.store(id + 1, Ordering::Relaxed);
}

/// Stage `firmware` ends here (loader entry → console up): calibrate, find the UART, arm the watchdog.
pub fn start(tsc_entry: u64) -> Watchdog {
    let a = rdtsc();
    boot::stall(core::time::Duration::from_millis(10));
    let hz = rdtsc().wrapping_sub(a).saturating_mul(100);
    HZ.store(hz, Ordering::Relaxed);
    let uart = find_uart();
    UART.store(uart, Ordering::Relaxed);
    let r = rec();
    r.magic = LOADER_STAGES_MAGIC;
    r.uart = uart;
    r.loader_hz = hz;
    T0.store(tsc_entry, Ordering::Relaxed);
    ENTRY.store(tsc_entry, Ordering::Relaxed);
    let mut l = Line::new();
    let _ = write!(l, "loader wire={} tsc={}MHz", LOADER_UART_NAMES[uart as usize], hz / 1_000_000);
    uart_line(l.as_str());
    end(FIRMWARE, 0);
    let ev = unsafe { boot::create_event(EventType::TIMER | EventType::NOTIFY_SIGNAL, Tpl::CALLBACK, Some(watchdog), None) };
    match ev {
        Ok(ev) => {
            let _ = boot::set_timer(&ev, TimerTrigger::Periodic(core::time::Duration::from_secs(1)));
            EVENT.store(ev.as_ptr() as u64, Ordering::Release);
        }
        Err(e) => {
            let mut l = Line::new();
            let _ = write!(l, "loader: watchdog unavailable ({:?}) - stages are timed, not watched", e.status());
            uart_line(l.as_str());
        }
    }
    Watchdog
}

/// Stage `gop` ends: the panel is known; replay the header and the stages so far onto it.
pub fn gop(addr: u64, size: usize, info: FrameBufferInfo) {
    if addr != 0 && size != 0 && info.stride >= info.width && info.stride * info.height * 4 <= size {
        FB_W.store(info.width as u64, Ordering::Relaxed);
        FB_H.store(info.height as u64, Ordering::Relaxed);
        FB_STRIDE.store(info.stride as u64, Ordering::Relaxed);
        FB_ADDR.store(addr, Ordering::Relaxed);
    }
    let mut l = Line::new();
    let _ = write!(l, "loader wire={} tsc={}MHz", LOADER_UART_NAMES[UART.load(Ordering::Relaxed) as usize], HZ.load(Ordering::Relaxed) / 1_000_000);
    panel_row(0, l.as_str());
    let r = rec();
    if r.count >= 1 {
        let el = ms(r.end_tsc[0].wrapping_sub(ENTRY.load(Ordering::Relaxed)));
        panel_row(1, stage_line(FIRMWARE, el).as_str());
    }
    end(GOP, 0);
}

/// Stage `volumes` ends (`kernel.elf` open on our own volume): count what the firmware published.
pub fn volumes() {
    let n = boot::locate_handle_buffer(SearchType::from_proto::<SimpleFileSystem>()).map(|b| b.len()).unwrap_or(0);
    rec().volumes = n as u32;
    end(VOLUMES, 0);
}

/// Read `kernel.elf` in 1 MiB chunks (`last` = bytes so far for the watchdog); a failed chunk is retried
/// three times from its own offset before the error is returned. Same contract as `RegularFile::read`.
pub fn read_kernel(f: &mut RegularFile, buf: &mut [u8]) -> uefi::Result<usize, usize> {
    let mut off = 0usize;
    let mut tries = 0u32;
    while off < buf.len() {
        let n = core::cmp::min(CHUNK, buf.len() - off);
        match f.read(&mut buf[off..off + n]) {
            Ok(0) => break,
            Ok(k) => {
                off += k;
                tries = 0;
                LAST.store(off as u32, Ordering::Relaxed);
            }
            Err(e) => {
                tries += 1;
                rec().retries += 1;
                let mut l = Line::new();
                let _ = write!(l, "loader: kernel read at {} failed ({:?}) retry {}/3", off, e.status(), tries);
                say(TIMEOUT_ROW, l.as_str());
                if tries > 3 || f.set_position(off as u64).is_err() {
                    return Err(uefi::Error::new(e.status(), off));
                }
            }
        }
    }
    rec().kernel_bytes = off as u64;
    Ok(off)
}

/// The last line before `exit_boot_services`: `jumping`, and the watchdog closed.
pub fn jumping() {
    disarm();
    let el = ms(rdtsc().wrapping_sub(T0.load(Ordering::Relaxed)));
    let l = stage_line(JUMP, el);
    say(1 + JUMP, l.as_str());
    BS_GONE.store(true, Ordering::Relaxed);
}

/// After `exit_boot_services`: the record with the jump stage closed at `tsc_jump` (no allocation).
pub fn close(tsc_jump: u64) -> LoaderStages {
    let r = rec();
    let i = r.count as usize;
    if i < LOADER_STAGE_MAX {
        r.id[i] = JUMP as u8;
        r.end_tsc[i] = tsc_jump;
        r.last_status[i] = Status::SUCCESS.0 as u32;
        r.count += 1;
    }
    r.timeouts = TIMEOUTS.load(Ordering::Relaxed);
    *r
}
