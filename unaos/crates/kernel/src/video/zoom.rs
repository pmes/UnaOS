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
//! only OWES one ([`OWED`]), cashed by the next unmasked caller or the service pass ([`service`]). ZOOMFURN (B848):
//! there is no timer sweep; a writer outside the composite pass names its rows ([`note_drawn`]) or owes one whole
//! present (an unnoted dirty pass; `note_unnoted` is deleted by ZOOMFLUSH2 B868).
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
//! # Only what moved (ZOOMCOST, rmbp-ledger B835)
//!
//! A composite pass names the source rows it wrote: wm reads its own damage set at its own snapshot points
//! (`zoom_note_pass` over the pass's rows/dirty/bands through `damaged_box`, `zoom_note_boxes` over the erase drain's
//! boxes) into [`note_rows`] ([`PASS_ROWS`]), and [`on_damage`] commits them to [`OWED_ROWS`] after the pass's pixels.
//! While the origin, the factor and nearest/bilinear are what the glass already shows ([`Lens::glass`]),
//! [`present_locked`] writes only the destination rows whose source rows are owed (`sy = oy + dy/z`, and `sy + 1`
//! while bilinear with `fy != 0`); a moved origin is a WHOLE present (a scroll of the glass would read the uncached
//! scan-out, BP-video-25).
//!
//! # Every writer names its rows (ZOOMFURN, rmbp-ledger B848)
//!
//! The writers outside wm's damage set name the rows they ALREADY wrote, straight into [`OWED_ROWS`], through
//! [`note_drawn`]: the strips, the HUD and the dropdowns at `strip::paint` / `strip::erase_rect` (after their pixels),
//! the cursor at `cursor::flush_box` / `flush_rect` (its post-write clean), the panel `Screen` after each damage rect's
//! copy. So a pass's `Owed.dirty` (a strip painted, a sprite repair) is already rows: it owes a whole present only when
//! no noted write came since the last pass ([`DRAWN`]). The pointer path is KEYED ([`KEYED`]): `present_locked`'s own
//! glass compare makes a moved origin whole, an unmoved pointer presents the sprite's noted rows. THE SWEEP RULE: a
//! whole present is owed only by a moved glass key, an engage or factor change, or an UNNOTED write (a dirty pass with
//! no note — `note_unnoted`, the door dimidle/lidsleep/lock/winsnap once called, is deleted by ZOOMFLUSH2 B868), each counted
//! ([`SWEEP_WHOLE`], `sweep_whole=`) and paid ONCE by the next present. No timer: with every writer noted a boot never
//! sweeps. ZOOMFLUSH (B859): the post-write clean IS the note — `FrameBuffer::flush_*` of the RAM frame names its rows ([`flushed`]).
//! ZOOMFLUSH2 (B868): it is the ONLY note — strip.rs's and wm's own notes are retired (`double_notes` counts every non-flush
//! source; `pass_notes` the callerless [`note_rows`] door, both gated 0 by `tests zoom`).
//!
//! WIRE: `[zoom] z=<n> follow=<p|f> origin=<x>,<y> present_us=<n> rows=<w>/<h> skipped=<0|1>` on each change
//! (rate-limited to 1/s, from the service pass); `[zoom] engage refused z=<n> why=<why>` (printed by [`engage`]
//! itself, ZOOM2 B829); `tests zoom`: `:: ZOOM: z=2 px_ok=3 present_us=<n> -> PASS ::` (+ `panel= smooth_us=
//! engage_us= budget_us= under= lost= why= present_full_us= present_band_us= rows=<w>/<h> skipped= furn_rows=<ok|bad>
//! furn=<flush rows>/<dest rows> sweep_whole=<fixture delta> sweep_boot=<n> flush_noted=<n> double_notes=0 pass_notes=0`); ZOOMFURN (B848): `[zoom] rows=<n>/<h>
//! src=<strip|hud|dropdown|cursor|screen|flush> noted=strip:<a>,hud:<b>,dropdown:<c>,cursor:<d>,screen:<e>,flush:<f> sweep_whole=<n>`
//! (the service pass, at most every 5 s, only when a tally moved).

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
/// ZOOMFURN (B848): the `[zoom] rows=` furniture line at most this often.
const FURN_MS: u64 = 5_000;
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
/// The last present's end (cycles), its cost, its kind, its origin.
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
/// ZOOMCOST (B835): one bit a source row of the RAM frame, `ROW_WORDS * 64` rows (a taller panel presents whole).
const ROW_WORDS: usize = 64;
/// The source rows the running composite pass wrote ([`note_rows`]), committed by [`on_damage`] after its pixels.
static PASS_ROWS: [AtomicU64; ROW_WORDS] = [const { AtomicU64::new(0) }; ROW_WORDS];
/// The source rows owed to the glass; taken at [`present_locked`]'s head.
static OWED_ROWS: [AtomicU64; ROW_WORDS] = [const { AtomicU64::new(0) }; ROW_WORDS];
/// The last present's written destination rows, and whether it wrote none (the wire's `rows=` / `skipped=`).
static PRESENT_ROWS: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicBool = AtomicBool::new(false);
/// ZOOMFURN (B848): the noting sources ([`note_drawn`]'s `src`), the wire's `src=` names.
pub const SRC_STRIP: u8 = 0;
pub const SRC_HUD: u8 = 1;
pub const SRC_DROPDOWN: u8 = 2;
pub const SRC_CURSOR: u8 = 3;
pub const SRC_SCREEN: u8 = 4; pub const SRC_FLUSH: u8 = 5; // ZOOMFLUSH (B859): `FrameBuffer::flush_*` of the RAM frame (`flushed`, file tail)
const SRC_N: usize = 6;
const SRC_NAMES: [&str; SRC_N] = ["strip", "hud", "dropdown", "cursor", "screen", "flush"];
/// A noted write since the last [`on_damage`] took it (the pass's `Owed.dirty` is then rows, not a whole owe).
static DRAWN: AtomicBool = AtomicBool::new(false);
/// Source rows noted per source since boot, and the last noter.
static NOTED: [AtomicU64; SRC_N] = [const { AtomicU64::new(0) }; SRC_N];
static LAST_SRC: AtomicU8 = AtomicU8::new(0);
/// A present is owed whose extent the glass key decides (a pointer move; `present_locked` goes whole on a moved key).
static KEYED: AtomicBool = AtomicBool::new(false);
/// UNNOTED writes paid whole since boot (`sweep_whole=`): a dirty pass with no note.
static SWEEP_WHOLE: AtomicU64 = AtomicU64::new(0);
/// ZOOMFLUSH2 (B868): source rows named through [`note_rows`] since boot (`pass_notes=`, gated 0: the door has no caller).
static PASS_NOTED: AtomicU64 = AtomicU64::new(0);
/// The furniture line's rate limit and the tally sum it last printed.
static FURN_AT: AtomicU64 = AtomicU64::new(0);
static FURN_KEY: AtomicU64 = AtomicU64::new(0);

/// The lens: the RAM frame `WRITER` points at while live, the real panel handle, the one destination row.
struct Lens {
    shadow: Vec<u32>,
    sfb: FrameBuffer,
    real: FrameBuffer,
    row: Vec<u32>,
    w: usize,
    h: usize,
    rstride: usize,
    /// ZOOMCOST (B835): what the glass shows, `(ox, oy, z, smooth)`; `None` = unknown (engage, disengage).
    glass: Option<(usize, usize, usize, bool)>,
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
    KEYED.store(true, Release); // ZOOMFURN (B848): keyed, not whole — a moved origin is whole by the glass compare
    kick(false);
}

/// A composite pass put pixels in the RAM frame (one call in `wm::composite_once`). One relaxed load when off.
/// ZOOMCOST (B835): the pass's noted rows are committed to [`OWED_ROWS`] here, after its pixels. ZOOMFURN (B848):
/// `dirty` (the pass's `Owed.dirty`: a strip painted, a sprite repair) is already rows when a noted write came since
/// the last call ([`DRAWN`], swapped here on every call, whatever the caller: the composite pass or a drain-only
/// present); with none it is an UNNOTED write: one whole owe, counted ([`SWEEP_WHOLE`]).
#[inline]
pub fn on_damage(dirty: bool) {
    if LIVE.load(Relaxed) == 0 {
        return;
    }
    for (p, o) in PASS_ROWS.iter().zip(OWED_ROWS.iter()) {
        let w = p.swap(0, AcqRel);
        if w != 0 {
            o.fetch_or(w, Release);
        }
    }
    if !DRAWN.swap(false, AcqRel) && dirty {
        OWED.store(true, Release);
        SWEEP_WHOLE.fetch_add(1, Relaxed);
    }
    kick(true);
}

/// ZOOMCOST (B835): a composite pass wrote source rows `y..y+h` of the RAM frame. ZOOMFLUSH2 (B868): no caller — wm's
/// `zoom_note_pass` / `zoom_note_boxes` are retired (the pass's own `flush_rect`s note through [`flushed_rows`]); a call
/// is tallied ([`PASS_NOTED`], `pass_notes=`) so `tests zoom` sees a writer that names rows here instead of cleaning.
/// One relaxed load when the lens is off.
#[inline]
pub fn note_rows(y: usize, h: usize) {
    if LIVE.load(Relaxed) == 0 {
        return;
    }
    set_rows(&PASS_ROWS, y, h);
    PASS_NOTED.fetch_add(h as u64, Relaxed);
}

/// ZOOMFURN (B848): a writer outside wm's damage set ALREADY wrote source rows `y..y+h` of the RAM frame (`src` one of
/// the `SRC_*`): straight into [`OWED_ROWS`] (the next present writes their destination rows), [`DRAWN`] raised for
/// the pass's dirty rule, the source tallied. One relaxed load when the lens is off.
#[inline]
pub fn note_drawn(y: usize, h: usize, src: u8) {
    if LIVE.load(Relaxed) == 0 || h == 0 {
        return;
    }
    set_rows(&OWED_ROWS, y, h);
    DRAWN.store(true, Release);
    let k = (src as usize).min(SRC_N - 1);
    NOTED[k].fetch_add(h as u64, Relaxed);
    LAST_SRC.store(k as u8, Relaxed);
}

/// Set rows `y..y+h` (clipped to the bitset) in `set`.
fn set_rows(set: &[AtomicU64; ROW_WORDS], y: usize, h: usize) {
    let end = y.saturating_add(h).min(ROW_WORDS * 64);
    let mut r = y;
    while r < end {
        let b = r % 64;
        let n = (64 - b).min(end - r);
        let mask = if n == 64 { u64::MAX } else { ((1u64 << n) - 1) << b };
        set[r / 64].fetch_or(mask, Relaxed);
        r += n;
    }
}

/// Any source row owed to the glass.
fn rows_owed() -> bool {
    OWED_ROWS.iter().any(|w| w.load(Acquire) != 0)
}

/// Present now, or owe it: a masked caller owes (the present is milliseconds of row stores); a pointer move inside
/// the previous present's budget owes (the HID band never queues presents behind each other). ZOOMCOST (B835): a
/// damage kick's owe is its rows, already in [`OWED_ROWS`]. ZOOMFURN (B848): the pointer path's owe is [`KEYED`]
/// (stored by [`on_pointer`]), and its present is not `full`: `present_locked`'s glass compare decides.
fn kick(damage: bool) {
    if crate::arch::irqs_masked() {
        return;
    }
    if !damage {
        let hz = crate::arch::apic::tsc_hz().max(1);
        let gap = crate::arch::now_cycles().saturating_sub(PRESENT_END_CYC.load(Relaxed));
        if gap.saturating_mul(1_000_000) / hz < BUDGET_US {
            return;
        }
    }
    let _ = present(None, false);
}

/// Bilinear once the pointer has settled.
fn smooth_now() -> bool {
    crate::arch::ms().saturating_sub(MOVE_MS.load(Relaxed)) >= SETTLE_MS
}

/// Run the present (`smooth` forced, else by the settle clock; `full` = a whole present asked). ZOOMOWE (B835): the
/// whole owe is swapped at each round's head; another round runs only for rows committed during the round or a
/// whole owe this call has not paid (at most three rounds), so an owe raised after this call paid one cashes once,
/// at the next caller or the service pass, never extending this hold. A lost try-lock stores `OWED` only for a whole
/// owe (a damage owe is its rows, which the holder's loop sees). Returns the last round's cost in us; `None` =
/// nothing presented (busy, off, the panel re-attached).
fn present(smooth: Option<bool>, full: bool) -> Option<u64> {
    let Some(mut g) = LENS.try_lock() else {
        if full {
            OWED.store(true, Release);
        }
        return None;
    };
    let mut last = None;
    let mut paid = false;
    for _ in 0..3 {
        let whole = OWED.swap(false, AcqRel) || (full && !paid);
        paid |= whole;
        KEYED.store(false, Release); // ZOOMFURN (B848): this round reads the key; a move after it re-rounds
        let l = g.as_mut()?;
        last = present_locked(l, smooth.unwrap_or_else(smooth_now), whole);
        if last.is_none() || !(rows_owed() || KEYED.load(Acquire) || (!paid && OWED.load(Acquire))) {
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
/// ZOOMCOST (B835): whole when `full`, when the glass shows another `(ox, oy, z, smooth)` or the panel is taller than
/// the bitset; otherwise only the destination rows whose source rows are owed. No row written: `Some(0)`, skipped.
fn present_locked(l: &mut Lens, smooth: bool, full: bool) -> Option<u64> {
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
    let key = (ox, oy, z, smooth);
    let whole = full || l.glass != Some(key) || h > ROW_WORDS * 64;
    let mut owed = [0u64; ROW_WORDS];
    for (o, word) in owed.iter_mut().zip(OWED_ROWS.iter()) {
        *o = word.swap(0, AcqRel);
    }
    let bit = |r: usize| ((owed[r / 64] >> (r % 64)) & 1) != 0;
    let t0 = crate::arch::now_cycles();
    let src = &l.shadow;
    let rb = w * 4;
    let mut built = usize::MAX;
    let mut written = 0usize;
    for dy in 0..h {
        let (sy, fy) = (oy + dy / z, dy % z);
        if !whole && !bit(sy) && !(smooth && fy != 0 && bit((sy + 1).min(h - 1))) {
            continue;
        }
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
        written += 1;
    }
    l.glass = Some(key);
    PRESENT_ROWS.store(written as u64, Relaxed);
    SKIPPED.store(written == 0, Relaxed);
    SMOOTH_LAST.store(smooth, Relaxed);
    if written == 0 {
        PRESENT_US.store(0, Relaxed);
        return Some(0);
    }
    let end = crate::arch::now_cycles();
    let us = super::blitter::cycles_to_us(end.saturating_sub(t0));
    PRESENT_US.store(us, Relaxed);
    PRESENT_END_CYC.store(end, Relaxed);
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
    if let Some(l) = g.as_mut().filter(|l| l.sfb.base_addr() == real.base_addr()) {
        l.glass = None;
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
        *g = Some(Lens { shadow, sfb, real, row, w, h, rstride: info.stride, glass: None });
    }
    let l = g.as_mut().ok_or("lens")?;
    l.glass = None;
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
        *wg = l.sfb; publish_frame(l); // ZOOMFLUSH (B859): the frame's base and the real panel published for `flushed` and `panic_glass` (file tail)
    }
    LIVE.store(z, Release);
    let us = super::blitter::cycles_to_us(crate::arch::now_cycles().saturating_sub(t0));
    drop(g); super::wm::desktop::shadows_covered(super::theme::epoch().wrapping_add(1)); super::theme::bump_epoch(); // crawl A 7: every strip re-seals into the RAM frame. LENSCOVER (SHADOWDAMAGE rmbp-ledger B865): the epoch this bump makes is covered by the damage-all + full present below, so the shadow re-lay only sets STALE (`desktop::shadows_relay`); the one cover fold, ZOOMFLUSH2 (B868) holds the note path. LINE-NEUTRAL fold, code first.
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
    l.glass = None;
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

/// The service pass (Settings' `a11y_service`, task context): reconcile the asked factor with the glass, cash an owed,
/// keyed or settled present, and say what changed (at most once a second). ZOOMFURN (B848): no timer sweep.
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
        let settle = !SMOOTH_LAST.load(Relaxed) && smooth_now();
        if OWED.load(Acquire) || KEYED.load(Acquire) || rows_owed() || settle {
            let _ = present(None, false);
        }
    }
    line();
    furn_line();
}

/// ZOOMFURN (B848): `[zoom] rows=<n>/<h> src= noted=strip:,hud:,dropdown:,cursor:,screen: sweep_whole=` when a tally
/// moved, at most every [`FURN_MS`] (`rows` = the last present's destination rows, `src` = the last noter).
fn furn_line() {
    let now = crate::arch::ms();
    if now.saturating_sub(FURN_AT.load(Relaxed)) < FURN_MS {
        return;
    }
    let n: [u64; SRC_N] = core::array::from_fn(|k| NOTED[k].load(Relaxed));
    let sw = SWEEP_WHOLE.load(Relaxed);
    let key = n.iter().fold(sw, |a, &v| a.wrapping_mul(31).wrapping_add(v));
    if FURN_KEY.swap(key, Relaxed) == key {
        return;
    }
    FURN_AT.store(now, Relaxed);
    let ph = LENS.try_lock().and_then(|g| g.as_ref().map(|l| l.h)).unwrap_or(0);
    serial_println!(
        "[zoom] rows={}/{} src={} noted=strip:{},hud:{},dropdown:{},cursor:{},screen:{},flush:{} sweep_whole={}",
        PRESENT_ROWS.load(Relaxed), ph, SRC_NAMES[(LAST_SRC.load(Relaxed) as usize).min(SRC_N - 1)], n[0], n[1], n[2], n[3], n[4], n[5], sw
    );
}

/// `[zoom] z= follow= origin= present_us= rows=<w>/<h> skipped=` when the state moved, at most once a second.
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
    let ph = LENS.try_lock().and_then(|g| g.as_ref().map(|l| l.h)).unwrap_or(0);
    serial_println!(
        "[zoom] z={} follow={} origin={},{} present_us={} rows={}/{} skipped={}",
        z, if f == 1 { "f" } else { "p" }, o >> 32, o & 0xFFFF_FFFF, PRESENT_US.load(Relaxed), PRESENT_ROWS.load(Relaxed), ph, SKIPPED.load(Relaxed) as u8
    );
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
    let sweep0 = SWEEP_WHOLE.load(Relaxed);
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
    let smooth_us = LENS.lock().as_mut().and_then(|l| present_locked(l, true, true)).unwrap_or(0);
    let (full_us, band_us, band_rows, skipped) = band_probe();
    let (furn_cur, furn_rows, double_notes, furn_why) = furn_probe();
    let o = ORIGIN.load(Relaxed);
    serial_println!("[zoom] z=2 follow=p origin={},{} present_us={}", o >> 32, o & 0xFFFF_FFFF, worst);
    crate::pal::cursor::set_abs(abs(px0.max(0) as usize, w), abs(py0.max(0) as usize, h), w as i32, h as i32);
    WANT.store(want0, Relaxed);
    FOLLOW.store(follow0, Relaxed);
    if live0 == 0 {
        disengage();
    } else {
        LIVE.store(live0, Release);
        let _ = present(None, true);
    }
    let band_ok = band_us <= BUDGET_US && band_rows > 0 && (band_rows as usize) < h;
    if px_ok == 3 && !band_ok {
        why = "band";
    }
    let sweep_boot = SWEEP_WHOLE.load(Relaxed);
    let sweep_whole = sweep_boot.saturating_sub(sweep0);
    let furn_ok = furn_why == "-";
    let pass_notes = PASS_NOTED.load(Relaxed);
    if px_ok == 3 && band_ok && why == "-" {
        if !furn_ok {
            why = furn_why;
        } else if sweep_whole != 0 {
            why = "sweep";
        } else if pass_notes != 0 {
            why = "pass";
        }
    }
    let pass = px_ok == 3 && band_ok && furn_ok && sweep_whole == 0 && pass_notes == 0;
    serial_println!(
        ":: ZOOM: z=2 px_ok={} present_us={} -> {} :: panel={}x{} smooth_us={} engage_us={} budget_us={} under={} lost={} why={} present_full_us={} present_band_us={} rows={}/{} skipped={} furn_rows={} furn={}/{} sweep_whole={} sweep_boot={} flush_noted={} double_notes={} pass_notes={}",
        px_ok, worst, if pass { "PASS" } else { "FAIL" }, w, h, smooth_us, ENGAGE_US.load(Relaxed), BUDGET_US, (worst <= BUDGET_US) as u8, PANEL_LOST.load(Relaxed), why,
        full_us, band_us, band_rows, h, skipped as u8, if furn_ok { "ok" } else { "bad" }, furn_cur, furn_rows, sweep_whole, sweep_boot, furn_cur, double_notes, pass_notes
    );
}

/// ZOOMCOST (B835): `tests zoom` M3 under ONE waiting `LENS` hold — a whole present, a 16-source-row band owed at the
/// lens centre (nearest, so `16 * z` destination rows), then a present with nothing owed (skipped; reported, not
/// gated). Returns `(present_full_us, present_band_us, band_rows, skipped)`.
fn band_probe() -> (u64, u64, u64, bool) {
    let mut g = LENS.lock();
    let Some(l) = g.as_mut() else { return (0, 0, 0, false) };
    let full_us = present_locked(l, false, true).unwrap_or(0);
    let z = (LIVE.load(Acquire) as usize).max(2);
    let oy = (ORIGIN.load(Relaxed) & 0xFFFF_FFFF) as usize;
    set_rows(&OWED_ROWS, (oy + l.h.div_ceil(z) / 2).saturating_sub(8), 16);
    let band = present_locked(l, false, false);
    let band_rows = if band.is_some() { PRESENT_ROWS.load(Relaxed) } else { 0 };
    let _ = present_locked(l, false, false);
    (full_us, band.unwrap_or(0), band_rows, SKIPPED.load(Relaxed))
}

/// ZOOMFURN (B848): `tests zoom` M3 under ONE waiting `LENS` hold — a whole nearest present (the glass known), a REAL
/// cursor write (`cursor::repaint`: its `flush_box`/`flush_rect` flushes note [`SRC_FLUSH`]; its `on_pointer` kick loses the
/// try-lock to this hold), then [`on_damage`]`(true)` (the dirty rule under test: a noted write came, so no whole
/// owe), then a present that must write rows, not the panel: `0 < rows < h`. Returns `(cursor rows noted, destination
/// rows written, why)`; why `-` = ok, `furn` = the cursor noted nothing, `furn-whole` = the noted dirty owed whole,
/// `furn-rows` = the present wrote none or every row. ZOOMFLUSH (B859): the cursor's rows are its flush's (`SRC_FLUSH`), and
/// (ZOOMFLUSH2 B868) a note from any other source during the write is a double (`double`, returned third).
fn furn_probe() -> (u64, u64, u64, &'static str) {
    let mut g = LENS.lock();
    let Some(l) = g.as_mut() else { return (0, 0, 0, "furn") };
    let _ = present_locked(l, false, true);
    let c0 = NOTED[SRC_FLUSH as usize].load(Relaxed); let d0 = noted_others(); // ZOOMFLUSH2 (B868): the cursor write is noted by its flush; a note from ANY other source is a double
    DRAWN.store(false, Release);
    super::cursor::repaint();
    let cur = NOTED[SRC_FLUSH as usize].load(Relaxed).saturating_sub(c0); let double = noted_others().saturating_sub(d0);
    let whole0 = OWED.swap(false, AcqRel);
    on_damage(true);
    let whole = OWED.load(Acquire);
    if whole0 {
        OWED.store(true, Release); // an owe raised elsewhere during the probe is kept for the next present
    }
    let rows = match present_locked(l, false, false) {
        Some(_) => PRESENT_ROWS.load(Relaxed),
        None => 0,
    };
    let why = if cur == 0 {
        "furn"
    } else if whole {
        "furn-whole"
    } else if rows == 0 || rows as usize >= l.h {
        "furn-rows"
    } else if double != 0 {
        "double"
    } else {
        "-"
    };
    (cur, rows, double, why)
}

/// ZOOMFLUSH2 (B868): source rows noted since boot by every source but `flush` — the one note door is the flush, so any
/// of these moving is a double note (`double_notes=`, [`furn_probe`]).
fn noted_others() -> u64 {
    NOTED.iter().enumerate().filter(|&(k, _)| k != SRC_FLUSH as usize).map(|(_, n)| n.load(Relaxed)).sum()
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
    let Some(us) = present_locked(l, false, true) else { return Err((0, "no-present")) };
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
// pitch, or the back store's offsets would not address it; then the flush keeps its own handle, as before). ZOOMFURN
// (B848): a flush into the frame owes no whole present here — the Screen names each damage rect's rows after its copy
// (`note_drawn`, `SRC_SCREEN`). Try-locks only, in the LENS -> nothing order: a lens mid-engage keeps the handle the
// flush already had.
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
    if LIVE.load(Acquire) != 0 { l.sfb } else { l.real }
}

// ZOOMFLUSH + ZOOMPANIC (rmbp-ledger B859; BP-video-26) — TAIL-APPENDED. Every writer's post-write clean is
// `FrameBuffer::flush_range` / `flush_rect` / `flush_all`; when its base is the lens's RAM frame (published here by the
// engage, under the LENS and WRITER holds) the flush names the rows it cleaned through [`note_drawn`] (`SRC_FLUSH`): one
// predicate, one call, so a writer outside the furniture's noted set is noted, and `on_damage`'s dirty rule owes whole
// only for a pass that flushed nothing. ZOOMPANIC: the panic screen paints the lens's REAL panel and stops the lens
// ([`panic_glass`]), lock-free — never `disengage`, whose LENS / WRITER waits the dying core may hold, and whose copy-back
// the panic fill overpaints at once.

/// The RAM frame's base and pitch (bytes); the real panel's base, length (bytes) and stride (pixels). 0 = never engaged.
static FRAME_BASE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static FRAME_PITCH: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static REAL_BASE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static REAL_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static REAL_STRIDE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// `engage_at`, as `WRITER` is retargeted (LENS and WRITER held): publish the frame and the real panel. The frame's base
/// is stored last (Release), and `LIVE` after it, so a flush that sees `LIVE` and the base sees the pitch too.
fn publish_frame(l: &Lens) {
    REAL_BASE.store(l.real.base_addr(), Relaxed);
    REAL_LEN.store(l.real.len(), Relaxed);
    REAL_STRIDE.store(l.rstride, Relaxed);
    FRAME_PITCH.store(l.w * 4, Relaxed);
    FRAME_BASE.store(l.sfb.base_addr(), Release);
}

/// `FrameBuffer::flush_range` (and `flush_all`): bytes `off..off+len` of the surface at `base` were cleaned after a write.
/// The RAM frame's: their rows noted (`SRC_FLUSH`). One relaxed load when the lens is off.
#[inline]
pub fn flushed(base: usize, off: usize, len: usize) {
    if LIVE.load(Relaxed) == 0 || len == 0 || base != FRAME_BASE.load(Acquire) {
        return;
    }
    let p = FRAME_PITCH.load(Relaxed).max(1);
    let y = off / p;
    note_drawn(y, (off + len).div_ceil(p) - y, SRC_FLUSH);
}

/// `FrameBuffer::flush_rect`: rows `y..y+h` of the surface at `base` were cleaned after a write. The RAM frame's: noted
/// (`SRC_FLUSH`). One relaxed load when the lens is off.
#[inline]
pub fn flushed_rows(base: usize, y: usize, h: usize) {
    if LIVE.load(Relaxed) == 0 || base != FRAME_BASE.load(Acquire) {
        return;
    }
    note_drawn(y, h, SRC_FLUSH);
}

/// ZOOMPANIC (B859): `panicscreen::draw_panic`'s surface. While the lens is live and `fb` is its RAM frame or its real
/// panel: the lens is stopped (`LIVE = 0`: no core presents the frame over the panic screen, every note is inert) and the
/// REAL panel is returned, rebuilt from the published atomics (no lock). Otherwise `fb` unchanged. `WRITER` stays on the
/// RAM frame, so a writer after the panic paints memory nobody shows.
pub fn panic_glass(fb: FrameBuffer) -> FrameBuffer {
    let (frame, real) = (FRAME_BASE.load(Acquire), REAL_BASE.load(Relaxed));
    if LIVE.load(Acquire) == 0 || frame == 0 || real == 0 || (fb.base_addr() != frame && fb.base_addr() != real) {
        return fb;
    }
    LIVE.store(0, Release);
    let fi = fb.info();
    let mut out = FrameBuffer::new();
    out.init(real, REAL_LEN.load(Relaxed), FrameBufferInfo { width: fi.width, height: fi.height, stride: REAL_STRIDE.load(Relaxed), bytes_per_pixel: 4, pixel_format: fi.pixel_format });
    out
}
