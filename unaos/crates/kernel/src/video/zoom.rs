//! CHARTER: Kernel — wm (ZOOM rmbp-ledger B713; BP-bar-22 and research C §10: the full-screen magnifier, the panel composed unfiltered in RAM and presented through a lens that follows the pointer or the focus, with no app support)
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

//! ZOOM (rmbp-ledger B713) — the screen magnifier.
//!
//! # The RAM frame, named
//!
//! The x86 compositor has NO full-frame staging buffer: `wm::stage_window` composes each window into a per-core
//! cached-RAM BAND (`stage_for_core`, at most `MAX_STAGE_BYTES`) and copies the band to the scan-out, and the other
//! nine writers of KCOMP's census (`docs/dev/evidence/rmbp-1005/KCOMP.md` §M1: the strips, the erase drain, the
//! cursor's save-under, the console, …) write the panel directly. What every one of them shares is ONE handle: the
//! [`super::WRITER`] `FrameBuffer` (or `panel_snapshot`, its copy). So the unfiltered frame in RAM is THIS module's
//! [`Lens::shadow`], and engaging the lens RETARGETS `WRITER` at it: the same geometry, the same pixel format, a
//! tight pitch, cached heap. Every writer keeps composing the unmagnified desktop exactly as it did (into RAM now,
//! where a read is a cached load, not a 1.7 us BAR1 probe), and ONE presenter owns the glass: [`present`] copies a
//! `(W/z) x (H/z)` window of the RAM frame to the real scan-out, scaled by `z`.
//!
//! # The present
//!
//! Integer and allocation-free per pixel: one destination row is built in [`Lens::row`] (allocated with the frame,
//! once) and written to the scan-out with `FrameBuffer::blit` (one streaming row store into the write-combined
//! aperture). NEAREST while the pointer moves (a source row is built once and stored `z` times); once the pointer has
//! been still for [`SETTLE_MS`] the presents are BILINEAR (four taps, integer weights `(z - f)` / `f`, per byte, so
//! the format is not decoded); an aligned destination pixel (`dx % z == 0 && dy % z == 0`) is its source pixel
//! exactly under both. The window is centred on the pointer (`pal::cursor::pos`) or, with `follow=focus`, on the
//! focused window's rect (`wm::wl_focused` + `wm::info`, the row A11YTREE flags FOCUSED), clamped at the panel's
//! edges. It is re-presented on every composite pass (`wm::composite_once`'s one call, [`on_damage`]) and on every
//! pointer move (the cursor's `RepaintTail::finish`, [`on_pointer`]); a masked caller or a present already running
//! only OWES one ([`OWED`]), cashed by the next unmasked caller or the service pass ([`service`]), which also sweeps
//! every [`SWEEP_MS`] so a writer outside the composite pass is never stranded behind the lens.
//!
//! # The cursor
//!
//! The sprite is drawn by `video::cursor` into `WRITER` like every other writer — into the RAM frame at the
//! pointer's LOGICAL position — and reaches the glass through the lens, i.e. at its magnified position
//! `(p - origin) * z`, magnified with the frame. Hit testing stays in logical panel coordinates, which is what the
//! sprite shows.
//!
//! # Memory and the swap's two hazards
//!
//! The RAM frame is `W x H x 4` bytes (9.2 MB at 1920x1200, 20.7 MB at 2880x1800), allocated fallibly on the first
//! engage and KEPT for the boot: a painter that copied the `WRITER` handle before a disengage may still be writing
//! through it, so freeing it would be a use-after-free (a geometry change forgets the old one for the same reason).
//! A painter that copied the REAL handle before an engage writes the glass once and is overdrawn by the next present;
//! the engage seeds the RAM frame from the glass and then damages every window and the desktop, so anything that
//! raced the seed is re-composed into RAM. A disengage copies the RAM frame back unscaled under the `WRITER` hold, so
//! the glass is exact the instant the handle is the panel's again.
//!
//! WIRE: `[zoom] z=<n> follow=<p|f> origin=<x>,<y> present_us=<n>` on each change (rate-limited to 1/s, from the
//! service pass); `[zoom] engage refused z=<n> why=<why>` (printed by [`engage`] itself, ZOOM2 B829); `tests zoom`:
//! `:: ZOOM: z=2 px_ok=3 present_us=<n> -> PASS ::` (+ `panel= smooth_us= engage_us= budget_us= under= lost= why=`).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering::*};

use unaos_boot_info::FrameBufferInfo;

use super::FrameBuffer;

// PINCHZOOM (B735): the trackpad's gesture recogniser lives in the input tree (`input/gestures.rs`) and is declared
// here, beside its first route, so it compiles exactly where the lens does (x86 `wc`) — `keymap.rs`'s idiom for
// `input/layouts.rs`.
#[path = "../input/gestures.rs"]
pub mod gestures;

/// The largest factor (the schema's `accessibility.zoom` range is `0..=4`; 1 reads as off).
pub const Z_MAX: u8 = 4;
/// The present's budget at 1920x1200, z=2 (the arc's bar; reported, never enforced).
pub const BUDGET_US: u64 = 4_000;
/// The pointer still for this long: presents go bilinear.
const SETTLE_MS: u64 = 120;
/// The `[zoom]` line at most once a second.
const LINE_MS: u64 = 1_000;
/// The service pass re-presents at least this often while the lens is live (writers outside the composite pass).
const SWEEP_MS: u64 = 250;
/// No focus centre (follow=focus falls back to the pointer).
const NONE: u64 = u64::MAX;

/// The factor asked for (Settings, the store, the chord): 0 off, 2, 3, 4.
static WANT: AtomicU8 = AtomicU8::new(0);
/// What the lens follows: 0 the pointer, 1 the focused window.
static FOLLOW: AtomicU8 = AtomicU8::new(0);
/// The factor live on the glass (0 = `WRITER` is the panel's own handle).
static LIVE: AtomicU8 = AtomicU8::new(0);
/// A chord changed the factor: the Settings pass stores it (no bus write on the input band).
static PERSIST: AtomicBool = AtomicBool::new(false);
/// A present is owed (a masked caller, a busy lens, a paced move, a change).
static OWED: AtomicBool = AtomicBool::new(false);
/// The focused window's centre, packed `x << 32 | y` ([`NONE`] = no focus).
static FOCUS_C: AtomicU64 = AtomicU64::new(NONE);
/// PINCHZOOM (B735): a panel point the lens centres on (packed `x << 32 | y`, [`NONE`] = none) until the pointer next
/// moves or a chord / Settings sets the factor ([`set_factor_at`]).
static AT: AtomicU64 = AtomicU64::new(NONE);
/// The last pointer move (ms) — nearest while moving, bilinear once settled.
static MOVE_MS: AtomicU64 = AtomicU64::new(0);
/// The last present's end (ms and cycles), its cost, its kind, its origin.
static PRESENT_MS: AtomicU64 = AtomicU64::new(0);
static PRESENT_END_CYC: AtomicU64 = AtomicU64::new(0);
static PRESENT_US: AtomicU64 = AtomicU64::new(0);
static SMOOTH_LAST: AtomicBool = AtomicBool::new(false);
static ORIGIN: AtomicU64 = AtomicU64::new(0);
/// The `[zoom]` line's rate limit and the state it last printed.
static LINE_AT: AtomicU64 = AtomicU64::new(0);
static LINE_KEY: AtomicU64 = AtomicU64::new(0);
/// The last engage's cost (the seed copy from the glass), and panels re-attached under a live lens.
static ENGAGE_US: AtomicU64 = AtomicU64::new(0);
static PANEL_LOST: AtomicU64 = AtomicU64::new(0);

/// The lens: the RAM frame `WRITER` points at while live, the real panel handle, the one destination row.
struct Lens {
    shadow: Vec<u32>,
    sfb: FrameBuffer,
    real: FrameBuffer,
    row: Vec<u32>,
    w: usize,
    h: usize,
    rstride: usize,
}

static LENS: crate::sync::Mutex<Option<Lens>> = crate::sync::Mutex::new(None);

/// A stored or asked factor as the lens takes it: 0 and 1 are off, past [`Z_MAX`] is [`Z_MAX`]. Pure.
pub fn normalize(z: i64) -> u8 {
    match z {
        i64::MIN..=1 => 0,
        2 => 2,
        3 => 3,
        _ => Z_MAX,
    }
}

/// One chord step from `z`: `+` 0 -> 2 -> 3 -> 4, `-` 4 -> 3 -> 2 -> 0, `0` clears. Pure.
pub fn stepped(z: u8, step: i8) -> u8 {
    match step {
        0 => 0,
        s if s > 0 => match z { 0 | 1 => 2, 2 => 3, _ => Z_MAX },
        _ => match z { 0..=2 => 0, 3 => 2, _ => 3 },
    }
}

/// Settings (the login's load, a PrefChanged, a press): the factor and what it follows. Applied by [`service`].
pub fn set(z: u8, follow: u8) {
    AT.store(NONE, Relaxed);
    WANT.store(normalize(z as i64), Relaxed);
    FOLLOW.store(follow.min(1), Relaxed);
    OWED.store(true, Release);
}

/// The key chord (`keymap::zoom_edge`, from the HID decoders): `Win+=` / `Win+-` step, `Win+0` clears. Atomics only —
/// the engage, the store write and the line are the service pass's.
pub fn chord(step: i8) {
    let z = stepped(WANT.load(Relaxed), step);
    AT.store(NONE, Relaxed);
    WANT.store(z, Relaxed);
    PERSIST.store(true, Release);
    OWED.store(true, Release);
}

/// PINCHZOOM (B735): the pinch's route (`gestures::feed`, the trackpad decoder's one call) and `tests pinchzoom` —
/// the factor `z` (normalised as Settings' is) centred on the panel point `at` until the pointer next moves (`None`:
/// the pointer, or the focus with follow=focus). Atomics only, the chord's regime: the engage, the present and the
/// `[zoom]` line are [`service`]'s; `keep` = Settings stores the factor as it stores a chord's (the fixture keeps
/// nothing).
pub fn set_factor_at(z: u8, at: Option<(u32, u32)>, keep: bool) {
    AT.store(at.map_or(NONE, |(x, y)| ((x as u64) << 32) | y as u64), Relaxed);
    WANT.store(normalize(z as i64), Relaxed);
    if keep {
        PERSIST.store(true, Release);
    }
    OWED.store(true, Release);
}

/// `(factor asked, follow)`.
pub fn want() -> (u8, u8) {
    (WANT.load(Relaxed), FOLLOW.load(Relaxed))
}

/// A chord's change not yet stored: Settings' pass writes `accessibility.zoom` from it.
pub fn take_persist() -> Option<(u8, u8)> {
    PERSIST.swap(false, AcqRel).then(want)
}

/// The cursor moved (one call in `cursor::RepaintTail::finish`). One relaxed load when the lens is off.
#[inline]
pub fn on_pointer() {
    if LIVE.load(Relaxed) == 0 {
        return;
    }
    if AT.load(Relaxed) != NONE {
        AT.store(NONE, Relaxed);
    }
    MOVE_MS.store(crate::arch::ms(), Relaxed);
    kick(false);
}

/// A composite pass put pixels in the RAM frame (one call in `wm::composite_once`). One relaxed load when off.
#[inline]
pub fn on_damage() {
    if LIVE.load(Relaxed) == 0 {
        return;
    }
    kick(true);
}

/// Present now, or owe it: a masked caller owes (the present is milliseconds of row stores); a pointer move inside
/// the previous present's budget owes (the HID band never queues presents behind each other).
fn kick(damage: bool) {
    if crate::arch::irqs_masked() {
        OWED.store(true, Release);
        return;
    }
    if !damage {
        let hz = crate::arch::apic::tsc_hz().max(1);
        let gap = crate::arch::now_cycles().saturating_sub(PRESENT_END_CYC.load(Relaxed));
        if gap.saturating_mul(1_000_000) / hz < BUDGET_US {
            OWED.store(true, Release);
            return;
        }
    }
    let _ = present(None);
}

/// Bilinear once the pointer has settled.
fn smooth_now() -> bool {
    crate::arch::ms().saturating_sub(MOVE_MS.load(Relaxed)) >= SETTLE_MS
}

/// Run the present (`smooth` forced, else by the settle clock), re-running while a caller owed one during it (at most
/// three rounds). Returns the last round's cost in us; `None` = nothing presented (busy, off, the panel re-attached).
fn present(smooth: Option<bool>) -> Option<u64> {
    let Some(mut g) = LENS.try_lock() else {
        OWED.store(true, Release);
        return None;
    };
    let mut last = None;
    for _ in 0..3 {
        OWED.store(false, Release);
        let l = g.as_mut()?;
        last = present_locked(l, smooth.unwrap_or_else(smooth_now));
        if last.is_none() || !OWED.load(Acquire) {
            break;
        }
    }
    last
}

/// The lens's centre: the focused rect's (follow=focus, when one is focused), else the pointer's. Clamped to the panel.
fn centre(w: usize, h: usize) -> (usize, usize) {
    let (f, at) = (FOCUS_C.load(Relaxed), AT.load(Relaxed));
    let (x, y) = if at != NONE {
        ((at >> 32) as usize, (at & 0xFFFF_FFFF) as usize)
    } else if FOLLOW.load(Relaxed) == 1 && f != NONE {
        ((f >> 32) as usize, (f & 0xFFFF_FFFF) as usize)
    } else {
        let (x, y) = crate::pal::cursor::pos(w as i32, h as i32);
        (x.max(0) as usize, y.max(0) as usize)
    };
    (x.min(w - 1), y.min(h - 1))
}

/// The window's top-left for centre `c` on an axis of `n` px at factor `z`: `ceil(n / z)` wide, clamped. Pure.
fn origin_of(c: usize, n: usize, z: usize) -> usize {
    let s = n.div_ceil(z);
    c.saturating_sub(s / 2).min(n - s)
}

/// One byte-wise bilinear tap: `p00 p10` (row `sy`), `p01 p11` (row `sy + 1`), weights `(z - fx, fx)` x `(z - fy, fy)`.
#[inline]
fn bilin(p00: u32, p10: u32, p01: u32, p11: u32, fx: u32, fy: u32, z: u32) -> u32 {
    let (gx, gy, d) = (z - fx, z - fy, z * z);
    let mut out = 0u32;
    let mut sh = 0;
    while sh < 32 {
        let c = |p: u32| (p >> sh) & 0xFF;
        let v = ((c(p00) * gx + c(p10) * fx) * gy + (c(p01) * gx + c(p11) * fx) * fy) / d;
        out |= v << sh;
        sh += 8;
    }
    out
}

/// THE PRESENT: the `ceil(W/z) x ceil(H/z)` window at the centre, scaled into the real scan-out. Returns its cost.
fn present_locked(l: &mut Lens, smooth: bool) -> Option<u64> {
    let z = LIVE.load(Acquire) as usize;
    if z < 2 {
        return None;
    }
    // The panel still ours? A re-attach (a re-init of WRITER) under a live lens ends it; the service re-engages.
    let cur = super::panel_snapshot()?;
    if cur.base_addr() != l.sfb.base_addr() {
        LIVE.store(0, Release);
        PANEL_LOST.fetch_add(1, Relaxed);
        return None;
    }
    let (w, h) = (l.w, l.h);
    let (cx, cy) = centre(w, h);
    let (ox, oy) = (origin_of(cx, w, z), origin_of(cy, h, z));
    ORIGIN.store(((ox as u64) << 32) | oy as u64, Relaxed);
    let t0 = crate::arch::now_cycles();
    let src = &l.shadow;
    let rb = w * 4;
    let mut built = usize::MAX;
    for dy in 0..h {
        let (sy, fy) = (oy + dy / z, dy % z);
        if smooth && fy != 0 {
            let (r0, r1) = (sy * w, (sy + 1).min(h - 1) * w);
            let (mut sx, mut fx) = (ox, 0usize);
            for px in l.row.iter_mut() {
                let sx1 = (sx + 1).min(w - 1);
                *px = bilin(src[r0 + sx], src[r0 + sx1], src[r1 + sx], src[r1 + sx1], fx as u32, fy as u32, z as u32);
                fx += 1;
                if fx == z { fx = 0; sx = (sx + 1).min(w - 1); }
            }
            built = usize::MAX;
        } else if built != sy {
            let r0 = sy * w;
            let (mut sx, mut fx) = (ox, 0usize);
            for px in l.row.iter_mut() {
                *px = if smooth && fx != 0 {
                    let sx1 = (sx + 1).min(w - 1);
                    bilin(src[r0 + sx], src[r0 + sx1], 0, 0, fx as u32, 0, z as u32)
                } else {
                    src[r0 + sx]
                };
                fx += 1;
                if fx == z { fx = 0; sx = (sx + 1).min(w - 1); }
            }
            built = sy;
        }
        // SAFETY: `row` holds exactly `w` initialised u32s; the slice is its bytes. `blit` bounds the store.
        let bytes = unsafe { core::slice::from_raw_parts(l.row.as_ptr() as *const u8, rb) };
        l.real.blit(dy * l.rstride * 4, bytes);
    }
    let end = crate::arch::now_cycles();
    let us = super::blitter::cycles_to_us(end.saturating_sub(t0));
    PRESENT_US.store(us, Relaxed);
    PRESENT_END_CYC.store(end, Relaxed);
    PRESENT_MS.store(crate::arch::ms(), Relaxed);
    SMOOTH_LAST.store(smooth, Relaxed);
    Some(us)
}

/// Engage at `z` ([`engage_at`]); a refusal is named on the wire here, once, never a silent `Err` (ZOOM2 B829).
fn engage(z: u8) -> Result<u64, &'static str> {
    let r = engage_at(z);
    if let Err(why) = r {
        serial_println!("[zoom] engage refused z={} why={}", z, why);
    }
    r
}

/// Engage at `z`: the RAM frame (allocated once, kept), seeded from the glass, `WRITER` retargeted at it, every
/// window and the desktop damaged so whatever raced the seed is re-composed into RAM. Task context only.
fn engage_at(z: u8) -> Result<u64, &'static str> {
    let mut g = LENS.lock();
    let real = *super::WRITER.lock();
    if g.as_ref().is_some_and(|l| l.sfb.base_addr() == real.base_addr()) {
        LIVE.store(z, Release); // WRITER is the RAM frame already: only the factor moves
        return Ok(0);
    }
    if !real.is_ready() {
        return Err("no-panel");
    }
    let info = real.info();
    if info.bytes_per_pixel != 4 {
        return Err("bpp");
    }
    let (w, h) = (info.width, info.height);
    if w == 0 || h == 0 || info.stride < w {
        return Err("geometry");
    }
    let fits = matches!(g.as_ref(), Some(l) if l.w == w && l.h == h && l.real.base_addr() == real.base_addr());
    if !fits {
        if let Some(old) = g.take() {
            core::mem::forget(old); // a stale copy of its handle may still be painting (module doc)
        }
        let mut shadow: Vec<u32> = Vec::new();
        let mut row: Vec<u32> = Vec::new();
        if shadow.try_reserve_exact(w * h).is_err() || row.try_reserve_exact(w).is_err() {
            return Err("heap");
        }
        shadow.resize(w * h, 0);
        row.resize(w, 0);
        let mut sfb = FrameBuffer::new();
        sfb.init(shadow.as_mut_ptr() as usize, w * h * 4, FrameBufferInfo { width: w, height: h, stride: w, bytes_per_pixel: 4, pixel_format: info.pixel_format });
        *g = Some(Lens { shadow, sfb, real, row, w, h, rstride: info.stride });
    }
    let l = g.as_mut().ok_or("lens")?;
    let t0 = crate::arch::now_cycles();
    // crawl A 7: seeded from RAM, never read back from BAR1 (uncached, ~1.7 us a probe: seconds at 1920x1200) — the
    // frame starts as the filtered desktop colour; the theme epoch bump below re-seals every strip and the damage-all
    // re-composes every window into it (the lock's wipe idiom, `lock::wipe`).
    l.sfb.fill_screen(super::blitter::a11yfilter::color(super::wm::DESKTOP_BG));
    {
        let mut wg = super::WRITER.lock();
        if wg.base_addr() != real.base_addr() {
            return Err("panel-moved");
        }
        *wg = l.sfb;
    }
    LIVE.store(z, Release);
    let us = super::blitter::cycles_to_us(crate::arch::now_cycles().saturating_sub(t0));
    drop(g); super::theme::bump_epoch(); // crawl A 7: every strip re-seals into the RAM frame
    let _ = super::wm::damage_intersecting(0, 0, w, h);
    super::screen::request_full_present();
    ENGAGE_US.store(us, Relaxed);
    OWED.store(true, Release);
    Ok(us)
}

/// Disengage: the RAM frame copied back unscaled and `WRITER` handed back, under the `WRITER` hold. Task context.
fn disengage() {
    let mut g = LENS.lock();
    LIVE.store(0, Release);
    let Some(l) = g.as_mut() else { return };
    let mut wg = super::WRITER.lock();
    if wg.base_addr() == l.sfb.base_addr() {
        for y in 0..l.h {
            // SAFETY: row `y` of the RAM frame, `w` initialised u32s.
            let bytes = unsafe { core::slice::from_raw_parts(l.shadow.as_ptr().add(y * l.w) as *const u8, l.w * 4) };
            l.real.blit(y * l.rstride * 4, bytes);
        }
        *wg = l.real;
    }
}

/// The focused rect's centre, refreshed off the hot path (the table lock is a task-context acquire).
fn refresh_focus() {
    let c = super::wm::wl_focused()
        .and_then(|(id, _)| super::wm::info(id))
        .map_or(NONE, |i| (((i.x + i.w * i.scale / 2) as u64) << 32) | (i.y + i.h * i.scale / 2) as u64);
    FOCUS_C.store(c, Relaxed);
}

/// The service pass (Settings' `a11y_service`, task context): reconcile the asked factor with the glass, cash an owed
/// or settled present, sweep, and say what changed (at most once a second).
pub fn service() {
    if FOLLOW.load(Relaxed) == 1 {
        refresh_focus();
    }
    let (want, live) = (WANT.load(Relaxed), LIVE.load(Acquire));
    if want != live {
        if want == 0 {
            disengage();
        } else if live == 0 {
            if engage(want).is_err() {
                WANT.store(0, Relaxed); // the refusal's line is engage's own
            }
        } else {
            LIVE.store(want, Release);
        }
        OWED.store(true, Release);
    }
    if LIVE.load(Acquire) != 0 {
        let now = crate::arch::ms();
        let settle = !SMOOTH_LAST.load(Relaxed) && smooth_now();
        if OWED.load(Acquire) || settle || now.saturating_sub(PRESENT_MS.load(Relaxed)) >= SWEEP_MS {
            let _ = present(None);
        }
    }
    line();
}

/// `[zoom] z= follow= origin= present_us=` when the state moved, at most once a second.
fn line() {
    let now = crate::arch::ms();
    if now.saturating_sub(LINE_AT.load(Relaxed)) < LINE_MS {
        return;
    }
    let (z, f) = (LIVE.load(Relaxed) as u64, FOLLOW.load(Relaxed) as u64);
    let o = if z == 0 { 0 } else { ORIGIN.load(Relaxed) };
    let key = (z << 60) ^ (f << 56) ^ o;
    if LINE_KEY.swap(key, Relaxed) == key {
        return;
    }
    LINE_AT.store(now, Relaxed);
    serial_println!("[zoom] z={} follow={} origin={},{} present_us={}", z, if f == 1 { "f" } else { "p" }, o >> 32, o & 0xFFFF_FFFF, PRESENT_US.load(Relaxed));
}

/// `tests zoom`: z=2 engaged, the pointer moved to three corners, each nearest present timed and one scan-out pixel
/// read back against the RAM frame's source pixel (the pointer's own: `(p - origin) * z` on the glass, `p` in RAM);
/// one bilinear present timed; every latch, the pointer and the glass restored. Nothing is stored.
pub fn selftest() {
    let Some(fb) = super::panel_snapshot().filter(|f| f.is_ready()) else {
        serial_println!(":: ZOOM: -> SKIP :: reason=no-panel");
        return;
    };
    let live0 = LIVE.load(Acquire);
    let (want0, follow0) = want();
    let info = fb.info();
    let (w, h) = (info.width, info.height);
    let (px0, py0) = crate::pal::cursor::pos(w as i32, h as i32);
    let real = if live0 == 0 { fb } else { match LENS.lock().as_ref() { Some(l) => l.real, None => fb } };
    FOLLOW.store(0, Relaxed);
    WANT.store(2, Relaxed);
    let engaged = if live0 == 0 { engage(2) } else { LIVE.store(2, Release); Ok(0) };
    if let Err(why) = engaged {
        WANT.store(want0, Relaxed);
        FOLLOW.store(follow0, Relaxed);
        serial_println!(":: ZOOM: -> SKIP :: reason={}", why);
        return;
    }
    let abs = |v: usize, n: usize| ((v * 32767) / (n - 1).max(1)) as i32;
    let mut px_ok = 0u32;
    let mut worst = 0u64;
    let mut why = "-";
    for (cx, cy) in [(0usize, 0usize), (w - 1, 0), (0, h - 1)] {
        match probe(cx, cy, w, h, real.info().stride) {
            Ok(us) => {
                worst = worst.max(us);
                px_ok += 1;
            }
            Err((us, miss)) => {
                worst = worst.max(us);
                if why == "-" {
                    why = miss;
                }
            }
        }
    }
    let smooth_us = LENS.lock().as_mut().and_then(|l| present_locked(l, true)).unwrap_or(0);
    let o = ORIGIN.load(Relaxed);
    serial_println!("[zoom] z=2 follow=p origin={},{} present_us={}", o >> 32, o & 0xFFFF_FFFF, worst);
    crate::pal::cursor::set_abs(abs(px0.max(0) as usize, w), abs(py0.max(0) as usize, h), w as i32, h as i32);
    WANT.store(want0, Relaxed);
    FOLLOW.store(follow0, Relaxed);
    if live0 == 0 {
        disengage();
    } else {
        LIVE.store(live0, Release);
        let _ = present(None);
    }
    let pass = px_ok == 3;
    serial_println!(
        ":: ZOOM: z=2 px_ok={} present_us={} -> {} :: panel={}x{} smooth_us={} engage_us={} budget_us={} under={} lost={} why={}",
        px_ok, worst, if pass { "PASS" } else { "FAIL" }, w, h, smooth_us, ENGAGE_US.load(Relaxed), BUDGET_US, (worst <= BUDGET_US) as u8, PANEL_LOST.load(Relaxed), why
    );
}

/// ZOOM2 (B829): one fixture present + its read-back under ONE waiting `LENS` hold. The pointer is moved to the corner
/// `(cx, cy)` OUTSIDE the hold (the cursor's `on_pointer` kick try-locks the lens), then the hold is TAKEN — the
/// waiting acquire `engage` / `disengage` take from task context — so a present running on another core (a
/// composite pass's `on_damage`) is waited out, never lost to, and no other present moves `ORIGIN` or the glass
/// between `present_locked` and the two probe reads: the RAM frame's pixel under the pointer (`p`) and the scan-out's
/// at `(p - origin) * 2`. `Ok(us)` on a match; `Err((us, why))` names the miss (`no-lens`, `no-present`, `edge`,
/// `pixel`).
fn probe(cx: usize, cy: usize, w: usize, h: usize, stride: usize) -> Result<u64, (u64, &'static str)> {
    let abs = |v: usize, n: usize| ((v * 32767) / (n - 1).max(1)) as i32;
    crate::pal::cursor::set_abs(abs(cx, w), abs(cy, h), w as i32, h as i32);
    let mut g = LENS.lock();
    let Some(l) = g.as_mut() else { return Err((0, "no-lens")) };
    let Some(us) = present_locked(l, false) else { return Err((0, "no-present")) };
    let (x, y) = crate::pal::cursor::pos(w as i32, h as i32);
    let (x, y) = (x.max(0) as usize, y.max(0) as usize);
    let o = ORIGIN.load(Relaxed);
    let (ox, oy) = ((o >> 32) as usize, (o & 0xFFFF_FFFF) as usize);
    let (dx, dy) = ((x - ox.min(x)) * 2, (y - oy.min(y)) * 2);
    if x >= l.w || y >= l.h || dx >= l.w || dy >= l.h || l.shadow.len() < l.w * l.h {
        return Err((us, "edge"));
    }
    let want_px = l.shadow[y * l.w + x];
    // SAFETY: `(dx, dy)` is inside the scan-out (bounded above by its geometry, `stride` its pitch); 4-byte pixels.
    let got_px = unsafe { core::ptr::read_volatile((l.real.base_addr() + (dy * stride + dx) * 4) as *const u32) };
    if want_px == got_px { Ok(us) } else { Err((us, "pixel")) }
}

// crawl A 8 (BP-video-26) — TAIL-APPENDED. The render service's panel `Screen` copied the REAL `WRITER` handle once at
// start, so while the lens was live `Screen::flush` wrote the scan-out unmagnified, around `present`. Each flush now
// asks this for its front: the RAM frame while live, the real panel otherwise — only for a `Screen` whose front IS the
// lens's panel (or its frame) and only when the geometry is the frame's (the frame's tight pitch must equal the panel's
// pitch, or the back store's offsets would not address it; then the flush keeps its own handle, as before). A flush
// into the frame owes a present (the service pass's `OWED`). Try-locks only, in the LENS -> nothing order: a lens
// mid-engage keeps the handle the flush already had.
pub fn screen_front(front: FrameBuffer) -> FrameBuffer {
    let Some(g) = LENS.try_lock() else { return front };
    let Some(l) = g.as_ref() else { return front };
    if front.base_addr() != l.real.base_addr() && front.base_addr() != l.sfb.base_addr() {
        return front;
    }
    let (fi, si) = (front.info(), l.sfb.info());
    if fi.width != si.width || fi.height != si.height || fi.stride != si.stride || fi.bytes_per_pixel != si.bytes_per_pixel {
        return front;
    }
    if LIVE.load(Acquire) != 0 {
        OWED.store(true, Release);
        l.sfb
    } else {
        l.real
    }
}
