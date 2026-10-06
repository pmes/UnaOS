// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — kernel-by-ruling
//!
//! PANICSCREEN (rmbp-ledger B406, MACPARITY row 37; R95 "the midnight red screen of death"). The kernel's own
//! glass at its own death — no handler can run here, so the kernel is the only owner there can be.
//!
//! * ONE PLAIN SCREEN. `fbcon::panic_screen` (every fatal path's first glass act) calls [`seal`] and then
//!   [`draw`]: the panel's dark neutral (`PANEL_BG`), `UnaOS stopped.` and `It will restart in 10 s.` in large
//!   type, the one-line reason and its file:line in small type below. SEALED, `fbcon::_print` paints nothing
//!   more — the panic text and the other cores' rollups go to the serial wire only — and the panicking core's
//!   lines are CAPTURED ([`capture`]) for the log.
//! * THE LOG. [`finish`] (in place of `hlt_loop` on every fatal path) writes the captured text and the last
//!   64 KiB of the flight recorder to `/var/log/panic-<n>.txt` on a native UnaFS root, with the marker
//!   `/var/log/panic.last`, and says on the wire why when it cannot (`[panic] log not written reason=<why>`):
//!   only from an UNMASKED panic (the block pump waits through `hlt`), only with the heap lock free.
//! * THE RESTART. A 10 s countdown (TSC) on the glass, then POWER's reset port (`acpi_power::reboot`).
//!   `UNAOS_PANIC_HOLD=1` (`panic_hold`) holds instead, for the bench capture.
//! * THE NEXT BOOT. [`next_boot_service`] (the storage pass, after login) reads the marker once:
//!   `[panic] previous boot stopped: <reason> log=<path>` and a DIALOG with `Show log`.
//! * `tests panicscreen` ([`test`]) draws the screen for 2 s WITHOUT panicking and restores the desktop.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use super::framebuffer::FrameBuffer;

/// Bench hold (`UNAOS_PANIC_HOLD=1`): no countdown, no restart.
pub const HOLD: bool = cfg!(feature = "panic_hold");
/// The countdown, seconds.
pub const COUNTDOWN_S: u32 = 10;
/// The log's directory and marker.
pub const DIR: &str = "/var/log";
pub const MARKER: &str = "/var/log/panic.last";
/// The log's extension (plain text).
const LOG_EXT: &str = "txt";
/// The DIALOG's title (its `Show log` answer is routed back here by title).
pub const DLG_TITLE: &[u8] = b"Last session";

const BG: u32 = super::PANEL_BG;
const INK: u32 = super::theme::PANIC_INK;
const INK_SMALL: u32 = super::theme::PANIC_INK_SMALL;
const TEXT_CAP: usize = 16 * 1024;
const TAIL_CAP: usize = 64 * 1024;
const FILE_CAP: usize = TEXT_CAP + TAIL_CAP + 1024;
const LINE_CAP: usize = 160;

static SEALED: AtomicBool = AtomicBool::new(false);
static PANIC_CPU: AtomicU32 = AtomicU32::new(u32::MAX);
/// Fatal-path entries this boot (a second one is a panic inside the panic path).
static ENTRIES: AtomicU32 = AtomicU32::new(0);
/// The panic came from an unmasked context (the log may be written).
static UNMASKED: AtomicBool = AtomicBool::new(false);
static FINISHING: AtomicBool = AtomicBool::new(false);

/// A fixed byte buffer written only by the panicking core (or the shell's test, never at once).
struct Buf<const N: usize> {
    b: core::cell::UnsafeCell<[u8; N]>,
    n: AtomicUsize,
}
unsafe impl<const N: usize> Sync for Buf<N> {}
impl<const N: usize> Buf<N> {
    const fn new() -> Self {
        Buf { b: core::cell::UnsafeCell::new([0; N]), n: AtomicUsize::new(0) }
    }
    fn clear(&self) {
        self.n.store(0, Ordering::Relaxed);
    }
    fn push(&self, s: &[u8]) {
        let n = self.n.load(Ordering::Relaxed);
        let k = s.len().min(N - n);
        // SAFETY: one writer by construction (the panicking core); `n` bounds the copy.
        unsafe { (&mut *self.b.get())[n..n + k].copy_from_slice(&s[..k]) };
        self.n.store(n + k, Ordering::Relaxed);
    }
    fn get(&self) -> &[u8] {
        // SAFETY: the prefix `..n` is written before `n` moves past it.
        unsafe { &(&*self.b.get())[..self.n.load(Ordering::Relaxed)] }
    }
}
struct W<'a, const N: usize>(&'a Buf<N>);
impl<const N: usize> Write for W<'_, N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.push(s.as_bytes());
        Ok(())
    }
}

static REASON: Buf<LINE_CAP> = Buf::new();
static AT: Buf<LINE_CAP> = Buf::new();
static TEXT: Buf<TEXT_CAP> = Buf::new();
static FILE: Buf<FILE_CAP> = Buf::new();
static PANEL: spin::Mutex<Option<FrameBuffer>> = spin::Mutex::new(None);

/// GS-FREE core identity (the initial APIC id from CPUID): the #MC and #DB arms keep their GS-free contract,
/// and a fault in the syscall-entry window may still hold the user's GS.
fn this_cpu() -> u32 {
    // SAFETY: CPUID leaf 1 exists on every x86_64 CPU.
    unsafe { core::arch::x86_64::__cpuid(1).ebx >> 24 }
}

/// The glass is sealed: `fbcon::_print` paints nothing more.
#[inline]
pub fn sealed() -> bool {
    SEALED.load(Ordering::Relaxed)
}

fn first_line(dst: &Buf<LINE_CAP>, args: fmt::Arguments) {
    dst.clear();
    let _ = W(dst).write_fmt(args);
    let s = dst.get();
    let cut = s.iter().position(|&b| b == b'\n').unwrap_or(s.len());
    dst.n.store(cut, Ordering::Relaxed);
}

/// The `#[panic_handler]`'s first statement: the reason (the message's first line) and its file:line.
pub fn note_panic(info: &core::panic::PanicInfo) {
    if ENTRIES.fetch_add(1, Ordering::AcqRel) == 0 {
        UNMASKED.store(!crate::arch::irqs_masked(), Ordering::Relaxed);
        first_line(&REASON, format_args!("{}", info.message()));
        match info.location() {
            Some(l) => first_line(&AT, format_args!("{}:{}", l.file(), l.line())),
            None => first_line(&AT, format_args!("-")),
        }
    }
}

/// A fatal CPU fault's first statement (interrupt gates: always masked, so the log says `reason=masked`).
pub fn note_fault(what: &str, rip: u64, cr2: u64) {
    if ENTRIES.fetch_add(1, Ordering::AcqRel) == 0 {
        UNMASKED.store(false, Ordering::Relaxed);
        first_line(&REASON, format_args!("{}", what));
        if cr2 != 0 {
            first_line(&AT, format_args!("rip={:#x} addr={:#x}", rip, cr2));
        } else {
            first_line(&AT, format_args!("rip={:#x}", rip));
        }
    }
}

/// The #DF's reason: a kernel stack that ran out (CR2 inside the current task's slab or just under it) or a
/// plain double fault. The #DF then `panic!`s, which keeps this reason (only the first note counts).
pub fn note_double_fault(rip: u64, cr2: u64) {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    let ovf = crate::arch::sched::current_slab(cpu)
        .map(|(base, len)| cr2 >= base.saturating_sub(4096) && cr2 < base + len as u64)
        .unwrap_or(false);
    note_fault(if ovf { "double fault: a kernel stack overflowed its guard" } else { "double fault" }, rip, cr2);
}

/// `fbcon::panic_screen`'s first statement: detach the console from the glass.
pub fn seal() {
    if !SEALED.swap(true, Ordering::AcqRel) {
        PANIC_CPU.store(this_cpu(), Ordering::Relaxed);
    }
}

/// `fbcon::_print` while sealed: the panicking core's lines go to the log buffer (never the glass).
pub fn capture(args: fmt::Arguments) {
    if PANIC_CPU.load(Ordering::Relaxed) == this_cpu() && !FINISHING.load(Ordering::Relaxed) {
        let _ = W(&TEXT).write_fmt(args);
    }
}

/// `fbcon::panic_screen` after its fill: the plain screen, from the panel handle fbcon already holds.
pub fn draw_panic(fb: &FrameBuffer) {
    if let Some(mut p) = PANEL.try_lock() {
        *p = Some(*fb);
    }
    draw(fb, REASON.get(), AT.get(), if HOLD { None } else { Some(COUNTDOWN_S) });
}

// ── the painter (bitmap atlas only: no font cache, no lock, no heap) ─────────────────────────────────

fn glyphs(fb: &FrameBuffer, s: &[u8], x: usize, y: usize, k: usize, ink: u32, bold: bool) {
    use super::font;
    let cw = font::CELL_W * k;
    for (i, &ch) in s.iter().enumerate() {
        let cx = x + i * cw;
        if cx + cw > fb.width() {
            break;
        }
        for (ry, row) in font::glyph(ch, bold, font::Face::Body).iter().enumerate() {
            for (rx, &a) in row.iter().enumerate() {
                if a == 0 {
                    continue;
                }
                let c = font::blend(BG, ink, a);
                for dy in 0..k {
                    for dx in 0..k {
                        fb.put_pixel(cx + rx * k + dx, y + ry * k + dy, c);
                    }
                }
            }
        }
    }
}

fn centred(fb: &FrameBuffer, s: &[u8], y: usize, k: usize, ink: u32, bold: bool) {
    let w = s.len() * super::font::CELL_W * k;
    let x = fb.width().saturating_sub(w) / 2;
    glyphs(fb, s, x, y, k, ink, bold);
}

/// The countdown line: `It will restart in <s> s.` (`None`: the bench hold's words).
fn count_line(out: &mut [u8; 64], secs: Option<u32>) -> usize {
    let b: &[u8] = b"It will restart in ";
    let mut n = b.len();
    out[..n].copy_from_slice(b);
    match secs {
        Some(s) => {
            if s >= 10 {
                out[n] = b'0' + (s / 10 % 10) as u8;
                n += 1;
            }
            out[n] = b'0' + (s % 10) as u8;
            n += 1;
            out[n..n + 4].copy_from_slice(b" s. ");
            n + 4
        }
        None => {
            let h: &[u8] = b"Held for the bench: press the power button.";
            out[..h.len()].copy_from_slice(h);
            h.len()
        }
    }
}

fn scales(fb: &FrameBuffer) -> (usize, usize) {
    let big = (fb.width() / 640).clamp(2, 5);
    let small = (fb.width() / 1600).clamp(1, 2);
    (big, small)
}

fn line_y(fb: &FrameBuffer, row: usize) -> usize {
    let (big, small) = scales(fb);
    let ch = super::font::CELL_H;
    let top = fb.height() * 2 / 5;
    match row {
        0 => top,
        1 => top + ch * big * 3 / 2,
        2 => top + ch * big * 3 + ch * small,
        _ => top + ch * big * 3 + ch * small * 5 / 2,
    }
}

/// The whole screen: background, headline, countdown, reason and where.
pub fn draw(fb: &FrameBuffer, reason: &[u8], at: &[u8], secs: Option<u32>) {
    if !fb.is_ready() {
        return;
    }
    let (big, small) = scales(fb);
    fb.fill_screen(BG);
    centred(fb, b"UnaOS stopped.", line_y(fb, 0), big, INK, true);
    let mut l = [0u8; 64];
    let n = count_line(&mut l, secs);
    centred(fb, &l[..n], line_y(fb, 1), big, INK, false);
    centred(fb, reason, line_y(fb, 2), small, INK_SMALL, false);
    centred(fb, at, line_y(fb, 3), small, INK_SMALL, false);
    fb.flush_all();
}

/// Repaint the countdown line alone (the rest of the screen holds still).
fn draw_count(secs: u32) {
    let Some(g) = PANEL.try_lock() else { return };
    let Some(fb) = *g else { return };
    let (big, _) = scales(&fb);
    let y = line_y(&fb, 1);
    let h = super::font::CELL_H * big;
    fb.fill_rect(0, y, fb.width(), h, BG);
    let mut l = [0u8; 64];
    let n = count_line(&mut l, Some(secs));
    centred(&fb, &l[..n], y, big, INK, false);
    fb.flush_rect(0, y, fb.width(), h);
}

// ── the terminal: log, countdown, restart ─────────────────────────────────────────────────────────

fn s(b: &[u8]) -> &str {
    core::str::from_utf8(b).unwrap_or("?")
}

/// Every fatal path's last statement (in place of `hlt_loop`). Never returns.
pub fn finish() -> ! {
    let nested = ENTRIES.load(Ordering::Acquire) > 1 || FINISHING.swap(true, Ordering::AcqRel);
    if !sealed() {
        seal(); // a path that never reached `panic_screen` (none today): the words still go to the wire
    }
    if PANEL.try_lock().map(|p| p.is_none()).unwrap_or(false) {
        // `panic_screen` lost FBCON's lock (the dying core held it): the panel handle itself, if free.
        if let Some(fb) = super::WRITER.try_lock().map(|f| *f) {
            draw_panic(&fb);
        }
    }
    serial_println!(
        "[panic] screen=plain reason={} at={} hold={} nested={}",
        s(REASON.get()),
        s(AT.get()),
        HOLD as u8,
        nested as u8
    );
    if nested {
        serial_println!("[panic] log not written reason=nested (a fault inside the panic path)");
    } else {
        write_log();
    }
    if HOLD {
        serial_println!("[panic] held (UNAOS_PANIC_HOLD) — the machine waits for the power button");
        crate::hlt_loop();
    }
    serial_println!("[panic] restart in {} s", COUNTDOWN_S);
    let hz = crate::arch::apic::tsc_hz();
    for left in (1..=COUNTDOWN_S).rev() {
        draw_count(left);
        wait_one_second(hz);
    }
    serial_println!("[panic] restart via=acpi-reset");
    let _ = crate::serial_ring::power_drain("panic");
    crate::arch::acpi_power::reboot()
}

fn wait_one_second(hz: u64) {
    if hz == 0 {
        for _ in 0..400_000_000u64 {
            core::hint::spin_loop();
        }
        return;
    }
    let t0 = crate::arch::now_cycles();
    while crate::arch::now_cycles().wrapping_sub(t0) < hz {
        core::hint::spin_loop();
    }
}

/// The heap lock free within 50 ms (a panic inside the allocator holds it for good).
fn heap_free(hz: u64) -> bool {
    let t0 = crate::arch::now_cycles();
    let budget = if hz == 0 { 50_000_000 } else { hz / 20 };
    while crate::allocator::heap_busy() {
        if crate::arch::now_cycles().wrapping_sub(t0) > budget {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

fn write_log() {
    if !UNMASKED.load(Ordering::Relaxed) {
        serial_println!("[panic] log not written reason=masked (a fault gate: the block pump needs interrupts)");
        return;
    }
    if !heap_free(crate::arch::apic::tsc_hz()) {
        serial_println!("[panic] log not written reason=heap-held");
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    if !root_native(&mt) {
        serial_println!("[panic] log not written reason=root-not-unafs");
        return;
    }
    ensure_dir(&mt, "/var");
    ensure_dir(&mt, DIR);
    let n = boot_number(&mt, true);
    let mut path = [0u8; 48];
    let pl = log_path(&mut path, n);
    let p = s(&path[..pl]);
    FILE.clear();
    let _ = write!(
        W(&FILE),
        "# UnaOS stopped: boot {} reason={} at={} ms={}\n\n## the panic text\n",
        n,
        s(REASON.get()),
        s(AT.get()),
        crate::arch::ms()
    );
    FILE.push(TEXT.get());
    FILE.push(b"\n## the flight recorder (last 64 KiB)\n");
    let at = FILE.n.load(Ordering::Relaxed);
    // SAFETY: the panicking core is FILE's only writer; the slice ends at the buffer's capacity.
    let room = unsafe { &mut (&mut *FILE.b.get())[at..(at + TAIL_CAP).min(FILE_CAP)] };
    let got = crate::flight_recorder::panic_tail(room);
    FILE.n.store(at + got, Ordering::Relaxed);
    let ok = write_all(&mt, p, FILE.get());
    if !ok {
        serial_println!("[panic] log not written reason=write path={}", p);
        return;
    }
    let mut m = [0u8; 256];
    let ml = {
        let mut w = Slice { b: &mut m, n: 0 };
        let _ = write!(w, "{} {} {}\n", n, p, s(REASON.get()));
        w.n
    };
    let marked = write_all(&mt, MARKER, &m[..ml]);
    serial_println!("[panic] log written path={} bytes={} marker={}", p, FILE.get().len(), marked as u8);
}

struct Slice<'a> {
    b: &'a mut [u8],
    n: usize,
}
impl Write for Slice<'_> {
    fn write_str(&mut self, t: &str) -> fmt::Result {
        let k = t.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&t.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}

fn log_path(out: &mut [u8; 48], n: u64) -> usize {
    let mut w = Slice { b: out, n: 0 };
    let _ = write!(w, "{}/panic-{}.{}", DIR, n, LOG_EXT);
    w.n
}

// ── VFS helpers (the kernel principal; bootwit's shape, which is `selfdiag`-gated) ──────────────────

fn root_native(mt: &crate::fs::vfs::MountTable) -> bool {
    matches!(mt.volume_name("/"), Ok(v) if v == "native")
}

fn ensure_dir(mt: &crate::fs::vfs::MountTable, d: &str) {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    if mt.stat(d).is_err() {
        let _ = mt.create(d, NodeKind::Dir, KERNEL_PRINCIPAL);
    }
}

fn read_small(mt: &crate::fs::vfs::MountTable, p: &str) -> Option<alloc::vec::Vec<u8>> {
    let st = mt.stat(p).ok()?;
    if st.size == 0 || st.size > 4096 {
        return None;
    }
    mt.read(p, 0, st.size as usize).ok()
}

fn write_all(mt: &crate::fs::vfs::MountTable, p: &str, b: &[u8]) -> bool {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    let _ = mt.unlink(p, KERNEL_PRINCIPAL);
    if mt.create(p, NodeKind::File, KERNEL_PRINCIPAL).is_err() {
        return false;
    }
    let mut off = 0usize;
    while off < b.len() {
        match mt.write(p, off as u64, &b[off..], KERNEL_PRINCIPAL) {
            Ok(0) | Err(_) => return false,
            Ok(w) => off += w,
        }
    }
    true
}

fn parse_dec(b: &[u8]) -> Option<u64> {
    let mut v: u64 = 0;
    let mut any = false;
    for &c in b {
        if c.is_ascii_digit() {
            v = v.checked_mul(10)?.checked_add((c - b'0') as u64)?;
            any = true;
        } else if any {
            break;
        } else if c != b' ' {
            return None;
        }
    }
    any.then_some(v)
}

/// This boot's number: SELFDIAG's when it assigned one, else `/var/log/boot.last` + 1 (claimed — written back
/// — when `claim`, so the next boot takes the number after it).
fn boot_number(mt: &crate::fs::vfs::MountTable, claim: bool) -> u64 {
    #[cfg(feature = "selfdiag")]
    {
        let n = crate::bootwit::state().1;
        if n != 0 {
            return n;
        }
    }
    let last = "/var/log/boot.last";
    let n = read_small(mt, last).and_then(|b| parse_dec(&b)).unwrap_or(0) + 1;
    if claim {
        let mut d = [0u8; 24];
        let mut w = Slice { b: &mut d, n: 0 };
        let _ = write!(w, "{}\n", n);
        let k = w.n;
        let _ = write_all(mt, last, &d[..k]);
    }
    n
}

// ── the next boot: the line and the dialog ────────────────────────────────────────────────────────

static NEXT_DONE: AtomicBool = AtomicBool::new(false);
static SHOW_PATH: spin::Mutex<[u8; 48]> = spin::Mutex::new([0; 48]);
static SHOW_LEN: AtomicUsize = AtomicUsize::new(0);

/// The storage pass (`login::notice_service`), once a session is up: read the marker once.
#[cfg(feature = "login")]
pub fn next_boot_service() {
    if NEXT_DONE.load(Ordering::Relaxed) {
        return;
    }
    let mut who = [0u8; 32];
    if crate::arch::syscall::session_name(&mut who).is_none() {
        return;
    }
    NEXT_DONE.store(true, Ordering::Relaxed);
    let mt = crate::shell::vfs_mount_table();
    if !root_native(&mt) {
        return;
    }
    let Some(m) = read_small(&mt, MARKER) else { return };
    let line = &m[..m.iter().position(|&b| b == b'\n').unwrap_or(m.len())];
    let mut parts = line.splitn(3, |&b| b == b' ');
    let _n = parts.next();
    let path = parts.next().unwrap_or(b"-");
    let reason = parts.next().unwrap_or(b"-");
    serial_println!("[panic] previous boot stopped: {} log={}", s(reason), s(path));
    if let Some(mut g) = SHOW_PATH.try_lock() {
        let k = path.len().min(48);
        g[..k].copy_from_slice(&path[..k]);
        SHOW_LEN.store(k, Ordering::Relaxed);
    }
    let mut info = [0u8; 200];
    let il = {
        let mut w = Slice { b: &mut info, n: 0 };
        let _ = write!(w, "{}\nThe log is {}.", s(reason), s(path));
        w.n
    };
    let d = super::dialog::Dlg::new(
        super::dialog::Icon::Caution,
        DLG_TITLE,
        b"The last session stopped unexpectedly.",
        &info[..il],
        &[b"OK", b"Show log"],
    );
    let posted = super::dialog::post(d); let _ = super::notify::post_quiet(b"system", b"Previous session stopped", &info[..il], b"Show log", super::notify::ACT_SHOW_LOG, b""); // NOTIFY (B418): the dialog is on the glass; the Center keeps the record
    let _ = mt.unlink(MARKER, crate::fs::vfs::KERNEL_PRINCIPAL); // said once
    serial_println!("[panic] notice posted={} marker=retired", posted as u8);
}

/// The DIALOG's `Show log` (its default button): open the log in the file viewer.
pub fn show_log() {
    let g = SHOW_PATH.lock();
    let p = s(&g[..SHOW_LEN.load(Ordering::Relaxed)]);
    match super::fileview::open(p) {
        Ok(_) => serial_println!("[panic] show log path={} -> opened", p),
        Err(e) => serial_println!("[panic] show log path={} -> {}", p, e),
    }
}

// ── `tests panicscreen` ──────────────────────────────────────────────────────────────────────────

/// Register `tests panicscreen` (the storage pass's first call).
pub fn register() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::Relaxed) {
        crate::tests::register("panicscreen", test);
    }
}

/// Draw the panic screen for 2 s WITHOUT panicking (the panel owner word holds the compositor and the
/// cursor off), then hand the panel back and repaint the desktop the way the idle wake does.
pub fn test() {
    use super::PanelOwner;
    let owner0 = super::panel_owner();
    let Some(fb) = super::panel_snapshot().filter(|f| f.is_ready()) else {
        serial_println!(":: PANICSCREEN: drawn=0 restored=0 reason=no-panel -> FAIL ::");
        return;
    };
    super::publish_panel_owner(PanelOwner::Panic, "panicscreen::test");
    draw(&fb, b"tests panicscreen (no panic: the screen alone)", b"video/panicscreen.rs", if HOLD { None } else { Some(COUNTDOWN_S) });
    let drawn = true;
    let t0 = crate::arch::ms();
    while crate::arch::ms().wrapping_sub(t0) < 2000 {
        crate::hlt();
    }
    super::publish_panel_owner(owner0, "panicscreen::test");
    fb.fill_screen(super::wm::DESKTOP_BG);
    fb.flush_all();
    let _ = super::wm::damage_intersecting(0, 0, fb.width(), fb.height());
    super::wm::composite();
    let restored = super::panel_refuse_term() != Some("owner-word-panic") && super::panel_owner() == owner0;
    let mt = crate::shell::vfs_mount_table();
    let native = root_native(&mt);
    let n = if native { boot_number(&mt, false) } else { 0 };
    let mut path = [0u8; 48];
    let pl = log_path(&mut path, n);
    let writable = native && {
        ensure_dir(&mt, "/var");
        ensure_dir(&mt, DIR);
        let probe = "/var/log/panic.probe";
        let ok = write_all(&mt, probe, b"probe\n");
        let _ = mt.unlink(probe, crate::fs::vfs::KERNEL_PRINCIPAL);
        ok
    };
    let pass = drawn && restored;
    serial_println!(
        ":: PANICSCREEN: drawn={} restored={} log_path={} writable={} hold={} sealed={} -> {} ::",
        drawn as u8,
        restored as u8,
        s(&path[..pl]),
        writable as u8,
        HOLD as u8,
        sealed() as u8,
        if pass { "PASS" } else { "FAIL" }
    );
}
