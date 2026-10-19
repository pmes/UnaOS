//! CHARTER: Kernel — wm
//!
//! A11YFILTERS (rmbp-ledger B705; BP-bar-22, research C §10: colour-vision filters must be system-wide, not per app)
//! — the colour filter every writer to the scanout applies AT THE COPY. Settings > Accessibility sets it
//! (`system.accessibility.cvd`, `system.accessibility.invert`; `settings::a11y_load` / `a11y_set` call [`set`]).
//!
//! WHY AT THE COPY AND NOT AFTER IT. The x86 compositor keeps no composed frame in RAM: it is damage-driven, and
//! about six paths write the write-combined BAR1 scanout directly (KCOMP's census,
//! `docs/dev/evidence/rmbp-1005/KCOMP.md` §M1: the window present, the occluded spans, the cursor sprite, the strips,
//! the erase/wallpaper fill, `Screen::flush`). A post-pass over the scanout would re-filter every pixel the pass did
//! NOT repaint (invert twice is the identity; a CVD matrix twice compounds) and would read BAR1 uncached (~1.7 us a
//! probe). So the rule is BP-video-25: every writer to the scanout filters at the copy, from a source that is never
//! filtered. The window present takes [`super::Op::CopyFilter`] (the CPU blitter runs it; the copy engines refuse
//! it, and `run_on` falls back), the row writers call [`each`] / [`in_place`], the painters of solid colours call
//! [`color`]. The splash and fbcon stay out (pre-desktop; the keys load at login).
//!
//! THE MATRICES. Machado, Oliveira & Fernandes, "A Physiologically-based Model for Simulation of Color Vision
//! Deficiency", IEEE TVCG 15(6):1291-1298, 2009 — the severity-1.0 protanopia / deuteranopia / tritanopia matrices
//! ([`MACHADO`], the published decimals), applied to the panel's sRGB words as the paper's own sample code does, in
//! Q12 integers ([`Q12`]: each published coefficient x 4096 rounded, one entry per row nudged by 1 so each row sums
//! to exactly 4096 — white stays white and black stays black). Grayscale is Rec. 709 luma (0.2126, 0.7152, 0.0722).
//! Invert is 255 - x per channel, after the matrix. Integer only (the compositor has no float on its hot path); the
//! decimals are read only by `tests a11yfilters`' reference.
//!
//! THE COST. [`each`] and [`in_place`] time the transform they run (two cycle reads per chunk) into [`TCYC`];
//! `present_band` counts each filtered window present; the Settings service pass prints
//! `[a11y] filter=<mode> pass_us=<n> presents=<n> motion=<full|reduced>` (transform us per window present) every
//! ten seconds while a filter is on and on every change. Off, every call site costs one relaxed load.
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering::Relaxed};

use unaos_boot_info::PixelFormat;

/// The `system.accessibility.cvd` spellings, in mode order (0 = no matrix).
pub const NAMES: [&str; 5] = ["none", "protan", "deutan", "tritan", "grayscale"];
pub const MODES: usize = NAMES.len();

/// Machado 2009, severity 1.0, row-major (R', G', B' from R, G, B), the published decimals; then Rec. 709 luma.
pub const MACHADO: [[f64; 9]; 4] = [
    [0.152286, 1.052583, -0.204868, 0.114503, 0.786281, 0.099216, -0.003882, -0.048116, 1.051998],
    [0.367322, 0.860646, -0.227968, 0.280085, 0.672501, 0.047413, -0.011820, 0.042940, 0.968881],
    [1.255528, -0.076749, -0.178779, -0.078411, 0.930809, 0.147602, 0.004733, 0.691367, 0.303900],
    [0.2126, 0.7152, 0.0722, 0.2126, 0.7152, 0.0722, 0.2126, 0.7152, 0.0722],
];

/// [`MACHADO`] in Q12 (x 4096, rounded; deutan B'b and tritan R'b / G'g one lower so every row sums to 4096), mode-indexed.
pub const Q12: [[i32; 9]; MODES] = [
    [4096, 0, 0, 0, 4096, 0, 0, 0, 4096],
    [624, 4311, -839, 469, 3221, 406, -16, -197, 4309],
    [1505, 3525, -934, 1147, 2755, 194, -48, 176, 3968],
    [5143, -314, -733, -321, 3812, 605, 19, 2832, 1245],
    [871, 2929, 296, 871, 2929, 296, 871, 2929, 296],
];
const _: () = {
    let mut m = 0;
    while m < MODES {
        let mut r = 0;
        while r < 3 {
            assert!(Q12[m][3 * r] + Q12[m][3 * r + 1] + Q12[m][3 * r + 2] == 4096);
            r += 1;
        }
        m += 1;
    }
};

static MODE: AtomicU8 = AtomicU8::new(0);
static INVERT: AtomicBool = AtomicBool::new(false);
/// `MODE != 0 || INVERT`: the one load every call site pays while the filter is off.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Transform cycles and pixels since the last [`take_stats`], and filtered window presents.
static TCYC: AtomicU64 = AtomicU64::new(0);
static TPX: AtomicU64 = AtomicU64::new(0);
static PRESENTS: AtomicU64 = AtomicU64::new(0);

/// Pixels a row writer filters per stack chunk (1 KiB).
const CHUNK: usize = 256;

/// Apply a mode (an index of [`NAMES`], held to the table) and the invert switch.
pub fn set(mode: usize, invert: bool) {
    let m = mode.min(MODES - 1);
    MODE.store(m as u8, Relaxed);
    INVERT.store(invert, Relaxed);
    ACTIVE.store(m != 0 || invert, Relaxed);
}

#[inline]
pub fn mode() -> usize {
    MODE.load(Relaxed) as usize
}

#[inline]
pub fn invert() -> bool {
    INVERT.load(Relaxed)
}

/// Is any filter on?
#[inline]
pub fn active() -> bool {
    ACTIVE.load(Relaxed)
}

/// The mode a stored spelling names.
pub fn mode_of(name: &str) -> Option<usize> {
    NAMES.iter().position(|n| *n == name)
}

/// The wire's `filter=` value: the mode, `+invert` when inverted (`invert` alone over no matrix).
pub fn label(mode: usize, invert: bool) -> alloc::string::String {
    match (mode.min(MODES - 1), invert) {
        (0, true) => alloc::string::String::from("invert"),
        (m, true) => alloc::format!("{}+invert", NAMES[m]),
        (m, false) => alloc::string::String::from(NAMES[m]),
    }
}

/// One logical `0x00RRGGBB` colour under `(mode, invert)`; the top byte passes through. Pure, integer.
#[inline]
pub fn apply(c: u32, mode: usize, inv: bool) -> u32 {
    let (r, g, b) = (((c >> 16) & 255) as i32, ((c >> 8) & 255) as i32, (c & 255) as i32);
    let o = if mode == 0 {
        c & 0x00FF_FFFF
    } else {
        let m = &Q12[mode.min(MODES - 1)];
        let ch = |k: usize| ((m[k] * r + m[k + 1] * g + m[k + 2] * b + 2048) >> 12).clamp(0, 255) as u32;
        (ch(0) << 16) | (ch(3) << 8) | ch(6)
    };
    let o = if inv { o ^ 0x00FF_FFFF } else { o };
    o | (c & 0xFF00_0000)
}

/// A solid colour on its way to the scanout, under the applied filter (the identity while off).
#[inline]
pub fn color(c: u32) -> u32 {
    if !active() {
        return c;
    }
    apply(c, mode(), invert())
}

/// The independent reference `tests a11yfilters` reads back against: [`MACHADO`]'s decimals in f64, rounded to the
/// nearest, clamped; then invert. Never on a paint path.
pub fn reference(c: u32, mode: usize, inv: bool) -> u32 {
    let (r, g, b) = (((c >> 16) & 255) as f64, ((c >> 8) & 255) as f64, (c & 255) as f64);
    let ch = |m: &[f64; 9], k: usize| {
        let v = m[k] * r + m[k + 1] * g + m[k + 2] * b + 0.5;
        if v <= 0.0 { 0u32 } else if v >= 255.0 { 255 } else { v as u32 }
    };
    let o = if mode == 0 {
        c & 0x00FF_FFFF
    } else {
        let m = &MACHADO[mode.min(MODES - 1) - 1];
        (ch(m, 0) << 16) | (ch(m, 3) << 8) | ch(m, 6)
    };
    if inv { o ^ 0x00FF_FFFF } else { o }
}

/// Swap the R and B bytes of a little-endian pixel word (an `Rgb` surface's word <-> the logical colour).
#[inline]
fn swap_rb(x: u32) -> u32 {
    (x & 0xFF00_FF00) | ((x >> 16) & 255) | ((x & 255) << 16)
}

/// Filter 4-byte pixels in place under `(mode, invert)` for a surface of `fmt` (`Bgr`'s LE word IS the logical
/// colour; `Rgb`'s is R/B swapped). Other formats and a trailing partial pixel pass unchanged. Pure; no clock.
pub fn transform(fmt: PixelFormat, mode: usize, inv: bool, buf: &mut [u8]) {
    let rgb = match fmt {
        PixelFormat::Rgb => true,
        PixelFormat::Bgr => false,
        _ => return,
    };
    let n = buf.len() / 4;
    if mode == 0 {
        if !inv {
            return;
        }
        // Invert alone is order-blind: XOR the three colour bytes.
        for i in 0..n {
            buf[4 * i] ^= 255;
            buf[4 * i + 1] ^= 255;
            buf[4 * i + 2] ^= 255;
        }
        return;
    }
    for i in 0..n {
        let p = &mut buf[4 * i..4 * i + 4];
        let w = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
        let c = if rgb { swap_rb(w) } else { w };
        let o = apply(c, mode, inv);
        p.copy_from_slice(&(if rgb { swap_rb(o) } else { o }).to_le_bytes());
    }
}

/// Filter a RAM row in place for the scanout of `fmt` / `bpp` (the erase fill's staged row), timed into the cost
/// line. No-op while off or for a non-4-byte surface.
#[inline]
pub fn in_place(fmt: PixelFormat, bpp: usize, buf: &mut [u8]) {
    if !active() || bpp != 4 {
        return;
    }
    let t0 = crate::arch::now_cycles();
    transform(fmt, mode(), invert(), buf);
    TCYC.fetch_add(crate::arch::now_cycles().saturating_sub(t0), Relaxed);
    TPX.fetch_add((buf.len() / 4) as u64, Relaxed);
}

/// Hand `src` (an unfiltered RAM row) to `sink(byte_offset_in_src, bytes)`: whole and untouched while off; else
/// filtered through a 1 KiB stack chunk, chunk by chunk (the source is never written). Every row writer to the
/// scanout calls this around its existing `blit`.
#[inline]
pub fn each(fmt: PixelFormat, bpp: usize, src: &[u8], mut sink: impl FnMut(usize, &[u8])) {
    if !active() || bpp != 4 {
        sink(0, src);
        return;
    }
    let (m, inv) = (mode(), invert());
    let mut buf = [0u8; CHUNK * 4];
    let mut i = 0;
    while i < src.len() {
        let n = (src.len() - i).min(CHUNK * 4);
        buf[..n].copy_from_slice(&src[i..i + n]);
        let t0 = crate::arch::now_cycles();
        transform(fmt, m, inv, &mut buf[..n]);
        TCYC.fetch_add(crate::arch::now_cycles().saturating_sub(t0), Relaxed);
        TPX.fetch_add((n / 4) as u64, Relaxed);
        sink(i, &buf[..n]);
        i += n;
    }
}

/// One filtered window present (`blitter::present_band`).
#[inline]
pub fn note_present() {
    PRESENTS.fetch_add(1, Relaxed);
}

/// Drain the cost counters: `(window presents, pixels filtered, transform us)` since the last drain.
pub fn take_stats() -> (u64, u64, u64) {
    let (p, px, cyc) = (PRESENTS.swap(0, Relaxed), TPX.swap(0, Relaxed), TCYC.swap(0, Relaxed));
    (p, px, super::cycles_to_us(cyc))
}

/// The counters without draining them.
pub fn peek_stats() -> (u64, u64, u64) {
    (PRESENTS.load(Relaxed), TPX.load(Relaxed), super::cycles_to_us(TCYC.load(Relaxed)))
}
