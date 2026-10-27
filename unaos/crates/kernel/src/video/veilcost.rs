//! CHARTER: Kernel — wm (VEILCOST B696, BP-bar-23: the VEIL's paint cost against a solid fill of the same rect, measured on the running compositor)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VEILCOST (rmbp-ledger B696) — BP-bar-23 "beauty (VEIL, shimmer) costs no more than a solid fill", the owed line.
//!
//! ONE SEAM: `wm::composite_once` calls [`frame`] once per pass, after `cash_tail` and `k2p::end` (wm.rs, a
//! same-line fold). `frame` costs one clock read and one load until a sample is due ([`SAMPLE_MS`]); a due sample
//! paints the MENU BAR's rect (panel width x the bar's height at (0, 0)) through the painter the bar composes with,
//! `wm::desktop::bar_row` over the live wallpaper's VEIL, into one scratch row, then fills the same rows with the
//! bar's face (`surface.window`) — the solid fill the bar would be without the VEIL. Both are timed with the TSC
//! (`arch::now_cycles`, `blitter::cycles_to_us`'s rate, kept in ns) and land in two 256-entry rings: p50/p99 over the
//! last 256 sampled frames. The fold repoints `bar_row` at STAGEVERB's `bar_row_cached` (c71c19dd), the cache the
//! live bar reads; the measurement is then the cost the live frame pays.
//!
//! WIRE: `[veil] cost p50_us=<n> p99_us=<n> solid_us=<n> ratio=<x.xx>` on the first pass after `[desktop] built`
//! and every 60 s while the desktop is up; printed only with IRQs on (the k2p `Flush` rule), else on a later pass.
//! `p50_us`/`p99_us` are the VEIL rect's, `solid_us` the fill's p99, `ratio` = VEIL p99 / fill p99. DRAINVEIL (B868): the
//! line ends ` drain_p99_us=<n>` — the drain-only present's whole call (`wm::drain_present_once`, [`drain`], file tail).
//! The fixture (`tests veilcost`, tests.rs tail) reads [`sample`] and [`stats`].

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::{AcqRel, Relaxed}};

/// The ring: the last 256 sampled frames.
pub const RING: usize = 256;
/// A sample at most this often (256 samples span ~128 s; the line is every 60 s).
pub const SAMPLE_MS: u64 = 500;
/// The line's period while the desktop is up.
pub const LINE_MS: u64 = 60_000;

#[allow(clippy::declare_interior_mutable_const)]
const Z: AtomicU32 = AtomicU32::new(0);
static VEIL_NS: [AtomicU32; RING] = [Z; RING];
static SOLID_NS: [AtomicU32; RING] = [Z; RING];
/// Samples ever recorded (the ring index is `n % RING`).
static N: AtomicUsize = AtomicUsize::new(0);
static LAST_SAMPLE: AtomicU64 = AtomicU64::new(0);
static LAST_LINE: AtomicU64 = AtomicU64::new(0);
/// The `[desktop] built` line owed (set on the built edge, cleared when printed); and the desktop's last state.
static LINE_OWED: AtomicBool = AtomicBool::new(false);
static WAS_BUILT: AtomicBool = AtomicBool::new(false);
/// The scratch row (panel width), grown once; `try_lock` only — a pass on another core skips its sample.
static ROW: crate::sync::Mutex<alloc::vec::Vec<u32>> = crate::sync::Mutex::new(alloc::vec::Vec::new());

/// TSC cycles to ns (the compositor's clock: `blitter::cycles_to_us` at 1000x).
#[inline]
fn ns(dt: u64) -> u32 {
    crate::video::blitter::cycles_to_us(dt.saturating_mul(1000)).min(u32::MAX as u64) as u32
}

/// One measurement: the bar's rect through the VEIL painter and as a solid fill. `(veil_ns, solid_ns)`; `None`
/// when no picture is installed (the flat colour: there is no VEIL to cost) or the scratch row is busy.
pub fn measure() -> Option<(u32, u32)> {
    let back = crate::video::wm::desktop::live_back()?;
    let (pw, ph) = (back.w, back.h);
    let (bx, by, bw, bar_h) = crate::video::menubar::geometry(pw, ph).unwrap_or((0, 0, pw, crate::ui::px(28).min(ph / 8).max(1))); // crawl A 5: the LIVE bar's rect (BAR_H = TITLE_HEIGHT), so the VeilKey is the compose's and the sample times the cached row
    if pw == 0 {
        return None;
    }
    let mut g = ROW.try_lock()?;
    if g.len() < pw {
        let more = pw - g.len();
        if g.try_reserve_exact(more).is_err() {
            return None;
        }
        g.resize(pw, 0);
    }
    let out = &mut g[..pw];
    let t0 = crate::arch::now_cycles();
    for j in 0..bar_h {
        crate::video::wm::desktop::bar_row_live(out, bx, by, bw.min(pw), bar_h, j, Some(&back)); // the fold: STAGEVERB's cached row, the live path
        core::hint::black_box(&mut *out);
    }
    let t1 = crate::arch::now_cycles();
    let face = crate::video::theme::surface_window();
    for _ in 0..bar_h {
        out.fill(face);
        core::hint::black_box(&mut *out);
    }
    let t2 = crate::arch::now_cycles();
    Some((ns(t1.saturating_sub(t0)), ns(t2.saturating_sub(t1))))
}

/// Measure once and record it in the rings. Returns the sample.
pub fn sample() -> Option<(u32, u32)> {
    let (v, s) = measure()?;
    let i = N.fetch_add(1, AcqRel) % RING;
    VEIL_NS[i].store(v, Relaxed);
    SOLID_NS[i].store(s, Relaxed);
    Some((v, s))
}

/// The rings' read: `(veil p50 us, veil p99 us, solid p99 us, ratio x100, samples in the ring)`.
pub fn stats() -> (u32, u32, u32, u32, usize) {
    let k = N.load(Relaxed).min(RING);
    let (mut v, mut s) = ([0u32; RING], [0u32; RING]);
    for i in 0..k {
        v[i] = VEIL_NS[i].load(Relaxed);
        s[i] = SOLID_NS[i].load(Relaxed);
    }
    let (v50, v99) = crate::video::wm::k2p::p50_p99(&mut v[..k]);
    let (_, s99) = crate::video::wm::k2p::p50_p99(&mut s[..k]);
    let ratio = if s99 == 0 { 0 } else { (v99 as u64 * 100 / s99 as u64).min(u32::MAX as u64) as u32 };
    (v50 / 1000, v99 / 1000, s99 / 1000, ratio, k)
}

/// Print the `[veil] cost` line from the rings.
pub fn line() {
    let (p50, p99, solid, r, _) = stats();
    serial_println!("[veil] cost p50_us={} p99_us={} solid_us={} ratio={}.{:02} drain_p99_us={}", p50, p99, solid, r / 100, r % 100, drain_p99_us());
}

/// **The seam** — `wm::composite_once`, once per pass after the present. Samples at most every [`SAMPLE_MS`];
/// owes the line on the `[desktop] built` edge and every [`LINE_MS`] after; prints it only with IRQs on.
pub fn frame() {
    let now = crate::arch::ms();
    let built = crate::video::desktopbuild::built();
    if built != WAS_BUILT.swap(built, Relaxed) && built {
        LINE_OWED.store(true, Relaxed);
        LAST_SAMPLE.store(0, Relaxed); // the built line carries at least this pass's sample
    }
    let last = LAST_SAMPLE.load(Relaxed);
    if (last == 0 || now.wrapping_sub(last) >= SAMPLE_MS) && LAST_SAMPLE.compare_exchange(last, now.max(1), AcqRel, Relaxed).is_ok() {
        let _ = sample();
    }
    if !built {
        return;
    }
    let ll = LAST_LINE.load(Relaxed);
    let due = LINE_OWED.load(Relaxed) || (ll != 0 && now.wrapping_sub(ll) >= LINE_MS);
    if due && N.load(Relaxed) > 0 && x86_64::instructions::interrupts::are_enabled() && LAST_LINE.compare_exchange(ll, now.max(1), AcqRel, Relaxed).is_ok() {
        LINE_OWED.store(false, Relaxed);
        line();
    }
}

// DRAINVEIL (rmbp-ledger B868; BP-video-11) — TAIL-APPENDED. The drain-only present (`wm::drain_present_once`, the
// shimmer's idle seat, SHIMMERPRESENT B840) is a frame the glass shows that never reaches [`frame`] (which runs once per
// WHOLE pass). Its own cost is recorded here, tagged drain: one ring of the same shape as the VEIL's, timed from the
// drain present's own open (`dm_t0`, DRAINMETER's clock) to its tail, under the caller's `COMP_GATE`. It records only
// that cost — no VEIL paint sample runs inside the drain's hold, so the drain present stays the cost it measures.

static DRAIN_NS: [AtomicU32; RING] = [Z; RING];
/// Drain presents ever recorded (the ring index is `k % RING`).
static DRAIN_K: AtomicUsize = AtomicUsize::new(0);

/// `wm::drain_present_once`'s tail: one drain present ran, opened at cycle `t0`. Two relaxed stores and a clock read.
pub fn drain(t0: u64) {
    let i = DRAIN_K.fetch_add(1, Relaxed) % RING;
    DRAIN_NS[i].store(ns(crate::arch::now_cycles().saturating_sub(t0)), Relaxed);
}

/// The drain ring's p99 in us (0 = no drain present yet).
pub fn drain_p99_us() -> u32 {
    let k = DRAIN_K.load(Relaxed).min(RING);
    let mut d = [0u32; RING];
    for (i, x) in d.iter_mut().take(k).enumerate() {
        *x = DRAIN_NS[i].load(Relaxed);
    }
    crate::video::wm::k2p::p50_p99(&mut d[..k]).1 / 1000
}
