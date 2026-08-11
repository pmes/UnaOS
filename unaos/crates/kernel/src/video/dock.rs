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

//! DOCK — the bottom strip, and the guarantee that **every window has a way back**.
//!
//! # Why it exists
//!
//! Peter's ruling, white board Q10, 2026-08-09, verbatim:
//!
//! > *"i guess mac has had the dock forever so we should have a doc and all macos like experience.
//! > remember we are trying to make mac users comfortable with unaos/crispy we are pretending to be
//! > a normal OS. crispy is meant to be an amalgamation of macos over the years"*
//!
//! and the standing performance priority from the same board: *"just make the os high performance
//! if looks a little off we will change it"*.
//!
//! # What it is, and what it deliberately is NOT
//!
//! It is a **window switcher**. One tile per live window — *including the windows that are not on
//! the panel* — with a press that raises and un-hides the window it names. That is the load-bearing
//! job: [`super::wm`] expresses "minimised" as a POSITION (a row whose `z` is below
//! [`super::wm::shell_z`] does not composite at all, see `wm::above_shell`), and until this module
//! there was no gesture that could bring such a row back except `<TAB>`. A window the operator can
//! send away and cannot call back is a window they have lost.
//!
//! It is **NOT an app launcher and carries no app grid.** `docs/dev/OS/ARCHITECTURE.md` is explicit
//! that UnaOS avoids fixed-feature apps, and the standing instruction is not to build them. There is
//! exactly one launch path in this kernel (the shell's program source / `bg`), and this module does
//! not add a second one. If launching ever belongs on the dock it arrives through that path.
//!
//! # x86 only, and gated
//!
//! `#[cfg(all(target_arch = "x86_64", feature = "wc"))]` at the `mod` declaration in
//! [`super`]. aarch64 is byte-identical with this file present: it is not compiled there. The
//! composite seam in `wm::composite_once` and the press seam in `arch::x86_64::syscall` carry the
//! same gate, so a knob-off x86 build has neither.
//!
//! # Materials and metrics — every value is a Crispy role or a Crispy metric
//!
//! **No new theme role was needed and none was invented.** The strip is the same object as the
//! window chrome and is machined from the same material:
//!
//! | element | colour | metric |
//! |---|---|---|
//! | strip face | [`theme::CHROME_FACE`] under [`ceramic::shade`] | height `2*GAP + BUTTON_HEIGHT` |
//! | strip keyline | [`theme::FRAME_LINE`] | 1 px, radius [`theme::CORNER_RADIUS`] |
//! | strip top bevel | [`theme::BEVEL_LIGHT`] | [`theme::BEVEL`] px |
//! | tile face | [`theme::BUTTON_FACE`] / [`theme::BUTTON_FACE_PRESSED`] under [`ceramic::shade_gain`] at [`ceramic::CONTROL_GAIN_Q16`] | [`theme::BUTTON_HEIGHT`] x auto, radius [`theme::WIDGET_RADIUS`] |
//! | caption ink | [`theme::TITLE_TEXT_ACTIVE`] / [`theme::TITLE_TEXT_INACTIVE`] | [`super::wm::TITLE_CELL`] |
//! | running indicator | [`theme::ACCENT`] (on the panel) / [`theme::SCROLL_THUMB`] (minimised) | `theme::GAP / 2` |
//! | padding, gaps, bottom margin | — | [`theme::GAP`] |
//!
//! The two DERIVED numbers, both stated here rather than buried: the indicator's diameter is
//! `theme::GAP / 2` (half the padding band the pip sits in — the dot is a status pip, not a
//! control, and must not read as one; see [`IND_D`] for why KNURL moved it off `CONTROL_BOX`);
//! and the tile takes the material at [`ceramic::CONTROL_GAIN_Q16`] rather than full gain, for
//! ceramic's own stated reason — a tile is a small saturated object and the grain competes with it.
//! Neither is dressed up as a kit citation. `kits/crispy/` is not in this repo and nothing here
//! pretends to have read it.
//!
//! # Performance — damage-driven, and measured
//!
//! **The dock does not repaint per frame.** [`compose`] runs at the tail of every composite pass and
//! repaints only when one of two things is true:
//!
//!  1. the tile model CHANGED — a different window set, a different caption, a different
//!     visible/focused/pressed state. Reduced to one `u64` FNV-1a signature, so the test is an
//!     integer compare against the last painted state, not a redraw;
//!  2. the pass PAINTED OVER the strip — a damaged, visible window whose outer box intersects the
//!     dock rect. That question is answered inside `wm`'s own table scan
//!     ([`super::wm::dock_scan`]), so it costs no second lock and no second walk.
//!
//! A quiet desktop therefore pays exactly: one `wm::dock_scan` (a bounded `MAX_WINDOWS` row scan,
//! the same shape as `focus_ring`), one signature hash over at most 12 short rows, one compare, and
//! a return. No framebuffer read, no framebuffer write, no allocation.
//!
//! The cost of both halves is COUNTED, in cycles, and put on the wire by [`rollup`] — `scan_cyc` is
//! what every pass pays, `paint_cyc` what a repaint costs — so "what did the dock cost per
//! composite" is a number in the capture rather than an estimate.
//!
//! # Front-buffer discipline (WC-H / WC-K / WC-L)
//!
//! The standing law in this subsystem is that nothing writes the live scan-out per-pixel: a painter
//! composes in CACHED RAM and copies out as contiguous row runs. The dock honours it. Each panel row
//! of the strip is composed into a cached scratch row and copied out with one `FrameBuffer::blit`
//! (a row of the strip is contiguous), and the whole strip is cleaned once with `flush_rect`. That
//! is `wm::stage_fill`'s shape at one-row granularity; the scratch is 2 x 6.4 KiB of `.bss` rather
//! than a whole-strip buffer, which is what keeps it affordable.
//!
//! The sprite is bracketed the way every other non-compositor painter in this subsystem brackets it
//! ([`super::wm::erase`]'s rule): [`compose`] takes the arrow off the panel with `cursor::undraw()`
//! BEFORE the first byte lands, and reports `true` so its caller upgrades the pass's cursor tail to
//! `Repaint`. A dock repaint therefore can never leave the sprite's save-under holding dock pixels.
//!
//! # Hit-testing follows drawing BY CONSTRUCTION
//!
//! There is exactly ONE tile-geometry accessor, [`Layout`], and both the painter ([`paint`]) and the
//! click router ([`press_at`]) obtain their rectangles from it. There is no second copy of the tile
//! arithmetic to drift — the law crispywire established after `controls` and `paint_window` disagreed
//! by one `GAP`. [`selftest`] asserts the two agree by driving a synthetic press at a tile centre
//! that [`Layout`] itself computed and checking WHICH window came back.

use super::{ceramic, theme, wm};

// ---------------------------------------------------------------------------
// Metrics — every one a `theme` name, or derived from one with the derivation
// written out. Nothing here is a bare literal with a look chosen for it.
// ---------------------------------------------------------------------------

/// Padding inside the strip, gap between tiles, and the strip's margin off the panel's bottom edge —
/// all [`theme::GAP`], the kit's one "standard gap between controls".
const PAD: usize = theme::GAP;

/// A tile's height — [`theme::BUTTON_HEIGHT`]. A dock tile IS a button by the kit's own taxonomy: a
/// raised control with a label that does something when pressed.
const TILE_H: usize = theme::BUTTON_HEIGHT;

/// A tile's corner radius — [`theme::WIDGET_RADIUS`], the kit's radius "for widgets (buttons and
/// other raised controls)".
const TILE_R: usize = theme::WIDGET_RADIUS;

/// The strip's corner radius — [`theme::CORNER_RADIUS`], the same radius the window head is cut with,
/// so the dock reads as the same fabrication as the chrome.
const STRIP_R: usize = theme::CORNER_RADIUS;

/// The strip's height: a tile with the standard gap above and below it.
const STRIP_H: usize = TILE_H + 2 * PAD;

/// The running indicator's diameter, px.
///
/// DERIVED, and stated as derived: `theme::GAP / 2` — half the padding band the pip sits in.
///
/// ⚠ **RE-DERIVED, same VALUE (6 px), by KNURL.** It was `theme::CONTROL_BOX / 2`, on the argument
/// that *"a pip the size of a control READS as a control, and a dot the operator tries to click is
/// worse than no dot"*. Peter's size ruling then took `CONTROL_BOX` from 12 to 24, which broke that
/// derivation in both directions at once: arithmetically it violated the `IND_D < PAD` assertion
/// below (12 is not less than `GAP` = 12, a BUILD failure), and semantically it produced a pip of
/// exactly the OLD control's diameter — i.e. the one outcome the halving existed to prevent.
///
/// So the pip is now derived from the band that actually bounds it, `PAD` = `theme::GAP`, which is
/// the constraint the assertion states and is independent of how large a control disc becomes. The
/// rendered pip is unchanged at 6 px; only its provenance moved. This is the sole line of the dock
/// module the size ruling touched, and it was touched because the alternative was a red tree.
const IND_D: usize = theme::GAP / 2;

/// The glyph cell the caption is drawn at — [`wm::TITLE_CELL`], i.e. the kit's `text_px` resolved to
/// the nearest integer scale of the bitmap font, exactly as the window caption resolves it. One
/// definition, so a kit text-size change moves the window caption and the dock caption together.
const CELL: usize = wm::TITLE_CELL;

/// The longest caption a tile will ever show, in glyphs. Bounded by [`wm::MAX_TITLE`]; capped at 8
/// because a dock is a row of many small things and a tile wide enough for a whole 16-byte title
/// would let four windows fill a 1920 panel.
const LABEL_MAX: usize = 8;

/// The widest strip this module will compose, in panel pixels — the scratch row's length.
///
/// Sized from the layout it must be able to hold rather than picked: a full table of
/// [`wm::MAX_WINDOWS`] tiles at [`LABEL_MAX`] glyphs is
/// `2*PAD + 12*(2*PAD + 8*CELL) + 11*PAD` = `24 + 12*152 + 132` = 1980 px, so 2048 covers every
/// layout this module can produce on any panel. Two scratch rows of `u32` = 16 KiB of `.bss`.
const MAX_STRIP_W: usize = 2048;

/// The layout cannot ask for a strip the scratch cannot hold. A `const` proof rather than a runtime
/// clamp, so a future `LABEL_MAX` or `MAX_WINDOWS` raise fails the BUILD.
const _: () = {
    assert!(2 * PAD + wm::MAX_WINDOWS * (2 * PAD + LABEL_MAX * CELL) + (wm::MAX_WINDOWS - 1) * PAD
        <= MAX_STRIP_W);
    // The caption must fit inside the tile it is centred in, or there is nothing to draw.
    assert!(CELL <= TILE_H);
    // The indicator must fit in the padding band below the tile.
    assert!(IND_D < PAD);
    // Both of a tile's corners must fit within its own height — the kit asserts this for buttons and
    // a tile IS a button; restated here because the tile is the object being cut.
    assert!(2 * TILE_R <= TILE_H);
    assert!(2 * STRIP_R <= STRIP_H);
    assert!(LABEL_MAX <= wm::MAX_TITLE);
};

// ---------------------------------------------------------------------------
// Layout — THE ONE geometry accessor. Painter and router both read it.
// ---------------------------------------------------------------------------

/// The dock's geometry for a given tile count and panel.
///
/// **The single source of tile arithmetic.** [`paint`] draws from it and [`press_at`] routes from it;
/// neither computes a rectangle of its own. Copy this arithmetic into a second place and the painter
/// and the router can disagree, which is exactly the defect crispywire convicted in `wm` (`controls`
/// and `paint_window` kept separate copies and differed by one `GAP`, so the threshold admitted
/// strips the painter then gave a zero-glyph budget).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The strip's outer box on the panel.
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    /// Tiles across.
    pub n: usize,
    /// One tile's width.
    pub tile_w: usize,
    /// Caption budget in glyphs, chosen so the strip fits the panel.
    pub glyphs: usize,
}

impl Layout {
    /// The layout for `n` tiles on a `pw` x `ph` panel, or `None` when there is nothing to draw
    /// (no tiles) or nowhere to draw it (a panel too narrow for even one-glyph tiles, or too short
    /// for the strip and its margin).
    ///
    /// The caption budget is chosen by trying [`LABEL_MAX`] glyphs and stepping down until the whole
    /// strip fits between two [`PAD`] margins — auto-sizing to the CONTENTS, with the panel as the
    /// only cap. Deterministic, integer, at most eight iterations.
    pub fn for_panel(n: usize, pw: usize, ph: usize) -> Option<Layout> {
        if n == 0 || n > wm::MAX_WINDOWS {
            return None;
        }
        if ph < STRIP_H + 2 * PAD {
            return None;
        }
        let mut glyphs = LABEL_MAX;
        loop {
            let tile_w = 2 * PAD + glyphs * CELL;
            let w = 2 * PAD + n * tile_w + (n - 1) * PAD;
            if w + 2 * PAD <= pw && w <= MAX_STRIP_W {
                return Some(Layout {
                    x: (pw - w) / 2,
                    y: ph - PAD - STRIP_H,
                    w,
                    h: STRIP_H,
                    n,
                    tile_w,
                    glyphs,
                });
            }
            if glyphs == 1 {
                return None; // even one glyph per tile will not fit: draw no dock at all.
            }
            glyphs -= 1;
        }
    }

    /// Tile `i`'s box on the panel, or `None` for an index past the tile count.
    #[inline]
    pub fn tile(&self, i: usize) -> Option<(usize, usize, usize, usize)> {
        if i >= self.n {
            return None;
        }
        Some((
            self.x + PAD + i * (self.tile_w + PAD),
            self.y + PAD,
            self.tile_w,
            TILE_H,
        ))
    }

    /// The tile index containing the panel point, or `None`. The ROUTER's whole geometry question,
    /// answered from the same fields the painter draws from.
    ///
    /// A point inside the STRIP but between two tiles answers `None` for the index while the caller's
    /// `contains` still answers `true` — the press is consumed by the dock (it landed on the dock)
    /// and raises nothing, which is what a press on the dock's own background should do.
    #[inline]
    pub fn tile_at(&self, px: usize, py: usize) -> Option<usize> {
        for i in 0..self.n {
            let (tx, ty, tw, th) = self.tile(i)?;
            if px >= tx && px < tx + tw && py >= ty && py < ty + th {
                return Some(i);
            }
        }
        None
    }

    /// Does the strip contain this panel point? Rounded corners included: a press on a cut corner is
    /// a press on whatever is behind the dock, exactly as `wm::hit_test` treats a window's cut head
    /// corners.
    #[inline]
    pub fn contains(&self, px: usize, py: usize) -> bool {
        px >= self.x
            && px < self.x + self.w
            && py >= self.y
            && py < self.y + self.h
            && !corner_cut(px - self.x, py - self.y, self.w, self.h, STRIP_R)
    }

    /// The strip as a plain rect, for the damage question.
    #[inline]
    pub fn rect(&self) -> (usize, usize, usize, usize) {
        (self.x, self.y, self.w, self.h)
    }
}

/// Is `(i, j)` outside the rounded corner of a `w` x `h` box with radius `r`?
///
/// All four corners, unlike `wm::corner_outside` (which cuts only the two TOP corners of a window
/// head): the dock is a free-floating slab, so it is rounded all the way round. Integer only —
/// `dx*dx + dy*dy > r*r` against the corner-circle centre.
#[inline]
fn corner_cut(i: usize, j: usize, w: usize, h: usize, r: usize) -> bool {
    if r == 0 || w < 2 * r || h < 2 * r {
        return false;
    }
    let (cx, cy) = if i < r {
        (r, if j < r { r } else if j >= h - r { h - r - 1 } else { return false })
    } else if i >= w - r {
        (w - r - 1, if j < r { r } else if j >= h - r { h - r - 1 } else { return false })
    } else {
        return false;
    };
    let dx = if i > cx { i - cx } else { cx - i };
    let dy = if j > cy { j - cy } else { cy - j };
    dx * dx + dy * dy > r * r
}

/// Is `(i, j)` inside the filled disc of diameter `d` whose top-left is `(bx, by)`? The indicator
/// pip's only shape test; mirrors `wm::in_circle`'s integer form.
#[inline]
fn in_disc(i: usize, j: usize, bx: usize, by: usize, d: usize) -> bool {
    if d == 0 || i < bx || j < by || i >= bx + d || j >= by + d {
        return false;
    }
    let (u, v) = (2 * (i - bx) + 1, 2 * (j - by) + 1);
    let (du, dv) = (
        if u > d { u - d } else { d - u },
        if v > d { v - d } else { d - v },
    );
    du * du + dv * dv <= d * d
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// The signature of the tile model the strip on the panel was painted from. `0` means "nothing is
/// painted", which is the state a teardown or a first boot leaves.
static PAINTED_SIG: AtomicU64 = AtomicU64::new(0);

/// The strip rect currently on the panel, packed `y<<48 | x<<32 | h<<16 | w`, or `0` for none.
/// Read by [`compose`] to ask `wm` the damage question BEFORE it knows this pass's layout — a
/// stale rect is safe, because a layout change is also a signature change and repaints regardless.
static PAINTED_RECT: AtomicU64 = AtomicU64::new(0);

/// The window id whose tile is held down, or `wm::WIN_NONE`. Cleared by the raise that follows.
static PRESSED: AtomicU32 = AtomicU32::new(wm::WIN_NONE);

/// One-shot: the strip cannot be composed on this surface (not a 4-byte word-aligned layout), said
/// once rather than on every pass.
static NOWORD_SAID: AtomicBool = AtomicBool::new(false);

/// Cost ledger. Not `witness`-gated: the metal image is built WITHOUT `witness`, and a performance
/// claim that is absent from the only artifact that matters is not a claim. Four relaxed adds per
/// pass; the rollup line is bounded and rate-limited by its own call site.
static PASSES: AtomicU64 = AtomicU64::new(0);
static PAINTS: AtomicU64 = AtomicU64::new(0);
static SCAN_CYC: AtomicU64 = AtomicU64::new(0);
static PAINT_CYC: AtomicU64 = AtomicU64::new(0);
static PAINT_PX: AtomicU64 = AtomicU64::new(0);
static PRESSES_N: AtomicU64 = AtomicU64::new(0);
static RAISES: AtomicU64 = AtomicU64::new(0);
static UNHIDES: AtomicU64 = AtomicU64::new(0);

/// The scratch the strip is composed in — CACHED RAM, never the scan-out. One row of logical colours
/// and one of pre-encoded framebuffer words; the encode is hoisted out of the pixel loop through a
/// tiny memo, exactly as `FrameBuffer::encode4` was built for.
///
/// `try_lock`, never `lock`: a contended pass declines and repaints on the next one rather than
/// spinning inside a composite. That is `wm::stage_fill`'s own rule for `STAGE`.
struct Scratch {
    log: [u32; MAX_STRIP_W],
    raw: [u32; MAX_STRIP_W],
}

static SCRATCH: spin::Mutex<Scratch> = spin::Mutex::new(Scratch {
    log: [0; MAX_STRIP_W],
    raw: [0; MAX_STRIP_W],
});

#[inline]
fn pack_rect(r: Option<(usize, usize, usize, usize)>) -> u64 {
    match r {
        Some((x, y, w, h)) if w != 0 && h != 0 => {
            ((y as u64 & 0xFFFF) << 48)
                | ((x as u64 & 0xFFFF) << 32)
                | ((h as u64 & 0xFFFF) << 16)
                | (w as u64 & 0xFFFF)
        }
        _ => 0,
    }
}

#[inline]
fn unpack_rect(v: u64) -> (usize, usize, usize, usize) {
    (
        ((v >> 32) & 0xFFFF) as usize,
        ((v >> 48) & 0xFFFF) as usize,
        (v & 0xFFFF) as usize,
        ((v >> 16) & 0xFFFF) as usize,
    )
}

/// FNV-1a 64 over the tile model — the whole "has anything changed?" test, reduced to one integer.
///
/// Everything the painter reads goes in: the window id, the owner, the caption bytes, the
/// visible/focused bits, the pressed id, and the layout (which folds in the panel geometry and the
/// glyph budget). A field the painter uses and this hash omits is a field whose change would leave a
/// stale strip on the panel, so the two lists are the same list on purpose.
fn signature(e: &[wm::DockEntry], l: &Layout, pressed: u32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut byte = |b: u8, h: &mut u64| {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x0000_0100_0000_01B3);
    };
    for v in [
        l.x as u64, l.y as u64, l.w as u64, l.h as u64,
        l.n as u64, l.tile_w as u64, l.glyphs as u64, pressed as u64,
    ] {
        for k in 0..8 {
            byte(((v >> (k * 8)) & 0xFF) as u8, &mut h);
        }
    }
    for r in e {
        for k in 0..4 {
            byte(((r.id >> (k * 8)) & 0xFF) as u8, &mut h);
        }
        for k in 0..8 {
            byte(((r.owner_asid >> (k * 8)) & 0xFF) as u8, &mut h);
        }
        byte(r.visible as u8, &mut h);
        byte(r.focused as u8, &mut h);
        byte(r.title_len as u8, &mut h);
        for &b in r.title[..r.title_len.min(wm::MAX_TITLE)].iter() {
            byte(b, &mut h);
        }
    }
    // A zero signature means "nothing painted"; fold it away so a real model can never collide with
    // the empty state.
    if h == 0 { 1 } else { h }
}

// ---------------------------------------------------------------------------
// The composite seam
// ---------------------------------------------------------------------------

/// **The dock's whole per-composite cost.** Called from `wm::composite_once` at the tail of every
/// pass, AFTER the window loop and BEFORE the cursor tail.
///
/// Returns `true` iff it painted, in which case the caller owes the sprite a `Repaint` tail — this
/// function has already taken the arrow off the panel.
///
/// The quiet path is one `wm::dock_scan`, one hash and a compare. See the module header.
pub fn compose() -> bool {
    let t0 = crate::arch::now_cycles();
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    // Ask `wm` for the tile model AND the damage question in ONE table scan: "were any of the
    // windows that intersect the strip I last painted damaged in the pass that just ran?"
    let (n, clobbered) = wm::dock_scan(&mut rows, unpack_rect(PAINTED_RECT.load(Ordering::Acquire)));
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            PASSES.fetch_add(1, Ordering::Relaxed);
            SCAN_CYC.fetch_add(
                crate::arch::now_cycles().saturating_sub(t0),
                Ordering::Relaxed,
            );
            return false;
        }
        (fb.width(), fb.height())
    };
    let layout = Layout::for_panel(n, pw, ph);
    let pressed = PRESSED.load(Ordering::Acquire);
    let sig = match layout {
        Some(l) => signature(&rows[..n], &l, pressed),
        None => 0,
    };
    let painted_sig = PAINTED_SIG.load(Ordering::Acquire);
    PASSES.fetch_add(1, Ordering::Relaxed);
    SCAN_CYC.fetch_add(
        crate::arch::now_cycles().saturating_sub(t0),
        Ordering::Relaxed,
    );

    // The ledger reaches the METAL image, or it is not a performance claim.
    //
    // `rollup` is not `witness`-gated, but a function with no caller outside `witness` is not in the
    // artifact either — the linker drops it and the claim evaporates exactly where it matters. So the
    // live emitter is HERE, on the path every pass takes, rate-limited to one line per
    // [`ROLLUP_PERIOD_US`]. Cost on a pass that does not print: one relaxed load and a compare.
    rollup_tick();

    // The two damage conditions, and nothing else. Note the ordering: a signature that MATCHES and a
    // pass that did not touch the strip is the common case and returns here having read no pixel.
    if sig == painted_sig && !clobbered {
        return false;
    }
    // THE STRIP OWES ITS OWN VACATED PIXELS. `wm::erase` cleans the boxes of WINDOWS; the dock is not
    // a window and no other painter knows its rect, so a strip that shrinks (a window closed, so the
    // tiles are fewer and the strip is narrower) or goes away entirely (the last window closed) would
    // leave its old ends standing on the panel until something else happened to paint over them. The
    // rule is the one `wm::close` follows: erase what you vacate, in the same pass, through the same
    // staged path.
    let old = PAINTED_RECT.load(Ordering::Acquire);
    let new = pack_rect(layout.map(|l| l.rect()));
    let vacated = if old != 0 && old != new { Some(unpack_rect(old)) } else { None };

    let Some(l) = layout else {
        if let Some(v) = vacated {
            let painted = erase_rect(v);
            PAINTED_SIG.store(0, Ordering::Release);
            PAINTED_RECT.store(0, Ordering::Release);
            return painted;
        }
        PAINTED_SIG.store(0, Ordering::Release);
        PAINTED_RECT.store(0, Ordering::Release);
        return false;
    };

    let t1 = crate::arch::now_cycles();
    if let Some(v) = vacated {
        // Erase FIRST, then paint: the new strip lands on top of the cleaned area, so the two never
        // race to own an overlapping pixel and the panel never shows a half-erased strip.
        erase_rect(v);
    }
    if !paint(&l, &rows[..n], pressed) {
        return false;
    }
    PAINTS.fetch_add(1, Ordering::Relaxed);
    PAINT_CYC.fetch_add(
        crate::arch::now_cycles().saturating_sub(t1),
        Ordering::Relaxed,
    );
    PAINT_PX.fetch_add((l.w * l.h) as u64, Ordering::Relaxed);
    PAINTED_SIG.store(sig, Ordering::Release);
    PAINTED_RECT.store(pack_rect(Some(l.rect())), Ordering::Release);
    true
}

/// Paint the strip. Returns `false` without touching the panel if it could not (no scratch, or a
/// surface whose layout the row-run path does not cover).
///
/// The one framebuffer writer in this module, and it writes the way the subsystem's law requires:
/// compose a row in cached RAM, copy it out with one `blit`, clean the whole rect once at the end.
fn paint(l: &Layout, rows: &[wm::DockEntry], pressed: u32) -> bool {
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return false;
    }
    if !fb.word4() {
        if !NOWORD_SAID.swap(true, Ordering::Relaxed) {
            serial_println!("[dock] decline reason=not-word4 — strip not composed on this surface");
        }
        return false;
    }
    let info = fb.info();
    if l.x + l.w > info.width || l.y + l.h > info.height || l.w > MAX_STRIP_W {
        return false;
    }
    let Some(mut s) = SCRATCH.try_lock() else {
        return false; // contended: the next pass repaints (the signature is still unmatched).
    };

    // CURSOR — take the arrow off the panel before the first byte lands. `wm::erase`'s bracket, and
    // for its reason: without it these rows would overwrite the sprite and the save-under would later
    // restore pre-dock pixels over a freshly painted strip. The caller restores it (we return `true`,
    // which upgrades the pass's cursor tail to `Repaint`).
    super::cursor::undraw();

    let stride_b = info.stride * 4;
    for j in 0..l.h {
        compose_row(&mut s.log, l, rows, pressed, j);
        // Encode logical colours to this surface's words, with a two-entry memo: a dock row is long
        // runs of very few colours, so the memo hits on nearly every pixel and `encode4`'s match runs
        // a handful of times per row instead of `w` times.
        let (mut mc, mut mr) = (u32::MAX, 0u32);
        for i in 0..l.w {
            let c = s.log[i];
            if c != mc {
                mc = c;
                mr = fb.encode4(c).unwrap_or(0);
            }
            s.raw[i] = mr;
        }
        let off = (l.y + j) * stride_b + l.x * 4;
        // SAFETY: `raw` is a live `[u32; MAX_STRIP_W]` and `l.w <= MAX_STRIP_W` (checked above); the
        // byte view is `4 * l.w` bytes of it, correctly aligned and initialised. `blit` bounds-checks
        // the destination itself and is a no-op on an overrun.
        let bytes = unsafe {
            core::slice::from_raw_parts(s.raw.as_ptr() as *const u8, l.w * 4)
        };
        fb.blit(off, bytes);
    }
    fb.flush_rect(l.x, l.y, l.w, l.h);
    true
}

/// Fill a vacated strip rect with the desktop colour, through the SAME staged row-run path
/// [`paint`] uses. Returns `true` if it painted (so the caller owes the sprite a `Repaint`).
///
/// `wm::erase` does exactly this job for a window's vacated box and would be the natural call — it is
/// private, it takes a slice of boxes, and it opens and closes its own cursor bracket. Re-entering it
/// from the tail of a composite pass would nest that bracket inside the one this module already holds.
/// One row-blit loop over `DESKTOP_BG` is the smaller thing.
fn erase_rect(r: (usize, usize, usize, usize)) -> bool {
    let (x, y, w, h) = r;
    if w == 0 || h == 0 || w > MAX_STRIP_W {
        return false;
    }
    let fb = *super::WRITER.lock();
    if !fb.is_ready() || !fb.word4() {
        return false;
    }
    let info = fb.info();
    if x >= info.width || y >= info.height {
        return false;
    }
    let (w, h) = (w.min(info.width - x), h.min(info.height - y));
    let Some(mut s) = SCRATCH.try_lock() else {
        return false;
    };
    super::cursor::undraw();
    let raw = fb.encode4(wm::DESKTOP_BG).unwrap_or(0);
    for i in 0..w {
        s.raw[i] = raw;
    }
    let stride_b = info.stride * 4;
    for j in 0..h {
        // SAFETY: as in `paint` — `raw` is a live `[u32; MAX_STRIP_W]`, `w <= MAX_STRIP_W`, and
        // `blit` bounds-checks its destination.
        let bytes = unsafe { core::slice::from_raw_parts(s.raw.as_ptr() as *const u8, w * 4) };
        fb.blit((y + j) * stride_b + x * 4, bytes);
    }
    fb.flush_rect(x, y, w, h);
    true
}

/// Compose panel row `j` of the strip into `out[0..l.w]` as logical `0x00RRGGBB` colours.
///
/// Two halves, in this order: a field pass (strip face, keyline, bevel, tile faces, the indicator
/// pips, the cut corners), then the caption glyphs overlaid by index. The glyphs are an overlay
/// rather than a per-pixel test so the inner loop stays a handful of integer compares instead of a
/// scan over every tile's caption at every pixel.
///
/// **The curvature is paid for only where there IS curvature.** The first cut ran `corner_cut` — and
/// through [`edge_ring`], four more of them — at EVERY pixel of the strip, which measured at ~95
/// cycles a pixel and made the dock the most expensive thing in the pass by an order of magnitude. A
/// rounded rectangle is straight everywhere except inside `r` of a corner, so the row is laid down as
/// flat spans first and the two `STRIP_R`-wide end bands are patched per-pixel, and only on the
/// `2*STRIP_R` rows that have a corner in them at all. Same pixels, same shape; the shape test runs
/// `4 * (2*STRIP_R)^2` times per repaint instead of `5 * w * h`.
fn compose_row(out: &mut [u32; MAX_STRIP_W], l: &Layout, rows: &[wm::DockEntry], pressed: u32, j: usize) {
    // The strip's material is anchored to the STRIP, not to the panel: index ceramic by the row's
    // offset inside the box, exactly as the window chrome indexes it by the row's offset inside the
    // window. The grain then belongs to the object.
    let face = ceramic::shade(theme::CHROME_FACE, j);
    let line = ceramic::shade(theme::FRAME_LINE, j);
    // The row's interior colour: the top bevel hairline for the first `BEVEL` rows under the keyline,
    // the chrome face everywhere else.
    let fill = if j >= 1 && j < 1 + theme::BEVEL { theme::BEVEL_LIGHT } else { face };
    if j == 0 || j + 1 == l.h {
        for i in 0..l.w {
            out[i] = line;
        }
    } else {
        out[0] = line;
        out[l.w - 1] = line;
        for i in 1..l.w - 1 {
            out[i] = fill;
        }
    }
    // The corner bands — the only pixels whose membership is in question.
    if j < STRIP_R || j + STRIP_R >= l.h {
        for i in (0..STRIP_R).chain(l.w - STRIP_R..l.w) {
            if corner_cut(i, j, l.w, l.h, STRIP_R) {
                // The pixels the painter cuts out of the corners are filled with the DESKTOP, exactly
                // as `wm::paint_window` fills a window's cut head corners. Same rule, same colour,
                // and `Layout::contains` declines the same pixels so a press there falls through.
                out[i] = wm::DESKTOP_BG;
            } else if edge_ring(i, j, l.w, l.h) {
                out[i] = line;
            }
        }
    }
    // Tiles.
    for (t, r) in rows.iter().enumerate().take(l.n) {
        let Some((tx, ty, tw, th)) = l.tile(t) else { continue };
        let (bx, by) = (tx - l.x, ty - l.y);
        // The indicator band: the PAD below the tile, inside the strip.
        if j >= by + th && j < by + th + PAD {
            let d = IND_D;
            let px0 = bx + tw / 2 - d / 2;
            let py0 = by + th + (PAD - d) / 2;
            let ink = if r.visible { theme::ACCENT } else { theme::SCROLL_THUMB };
            for i in px0..(px0 + d).min(l.w) {
                if in_disc(i, j, px0, py0, d) {
                    out[i] = ink;
                }
            }
            continue;
        }
        if j < by || j >= by + th {
            continue;
        }
        // The tile face — the material at the CONTROL gain (see the module header).
        let base = if r.id == pressed {
            theme::BUTTON_FACE_PRESSED
        } else {
            theme::BUTTON_FACE
        };
        let tface = ceramic::shade_gain(base, j, ceramic::CONTROL_GAIN_Q16);
        // Same span-then-patch shape as the strip: the tile's straight middle is a flat run, and the
        // two `TILE_R` end bands are tested per-pixel only on the rows that actually have a corner.
        // A cut tile corner shows the STRIP's face — the tile is a slab lying ON the strip, so what a
        // cut corner reveals is what is behind it, not the desktop.
        let (lo, hi) = (bx.min(l.w), (bx + tw).min(l.w));
        let corner_row = (j - by) < TILE_R || (j - by) + TILE_R >= th;
        if !corner_row {
            for i in lo..hi {
                out[i] = tface;
            }
        } else {
            let mid0 = (bx + TILE_R).min(hi);
            let mid1 = (bx + tw - TILE_R).max(mid0).min(hi);
            for i in lo..mid0 {
                if !corner_cut(i - bx, j - by, tw, th, TILE_R) {
                    out[i] = tface;
                }
            }
            for i in mid0..mid1 {
                out[i] = tface;
            }
            for i in mid1..hi {
                if !corner_cut(i - bx, j - by, tw, th, TILE_R) {
                    out[i] = tface;
                }
            }
        }
        // The caption, overlaid. Vertically centred in the tile, left-padded by one `PAD`, and
        // truncated to the layout's glyph budget — the budget the layout SIZED the tile from, so the
        // text can never overrun the box it is in.
        let ty0 = by + (th - CELL) / 2;
        if j < ty0 || j >= ty0 + CELL {
            continue;
        }
        let scale = wm::TITLE_SCALE.max(1);
        let sy = (j - ty0) / scale;
        if sy >= 8 {
            continue;
        }
        let ink = if r.focused {
            theme::TITLE_TEXT_ACTIVE
        } else {
            theme::TITLE_TEXT_INACTIVE
        };
        let cols = l.glyphs.min(r.title_len);
        for c in 0..cols {
            let b = r.title[c];
            let ch = if (0x20..0x7f).contains(&b) { b } else { b' ' };
            let bits = font8x8::legacy::BASIC_LEGACY[ch as usize][sy];
            if bits == 0 {
                continue;
            }
            let gx = bx + PAD + c * CELL;
            for rx in 0..8usize {
                if bits & (1 << rx) == 0 {
                    continue;
                }
                for sx in 0..scale {
                    let i = gx + rx * scale + sx;
                    if i < l.w {
                        out[i] = ink;
                    }
                }
            }
        }
    }
}

/// The keyline follows the ROUNDED edge, not just the four straight sides: a pixel that is inside the
/// box but whose neighbour one step outward is cut belongs to the outline. One test, no second
/// radius table.
#[inline]
fn edge_ring(i: usize, j: usize, w: usize, h: usize) -> bool {
    corner_cut(i.wrapping_sub(1).min(w - 1), j, w, h, STRIP_R)
        || corner_cut((i + 1).min(w - 1), j, w, h, STRIP_R)
        || corner_cut(i, j.wrapping_sub(1).min(h - 1), w, h, STRIP_R)
        || corner_cut(i, (j + 1).min(h - 1), w, h, STRIP_R)
}

// ---------------------------------------------------------------------------
// The press seam
// ---------------------------------------------------------------------------

/// **Route a press at a panel point.** Returns `true` iff the DOCK consumed it.
///
/// Called from the head of `wc_click_route_at`'s press edge, ahead of every window arm, because the
/// dock is composited ON TOP of the window layer: a point the dock covers is a point the operator can
/// see the dock at, and `wm::hit_test` — which knows nothing of this strip — would otherwise hand it
/// to the window underneath. That is the fall-through this ordering forbids.
///
/// On a TILE it does exactly what the router's own raise arm does, through the same two primitives
/// and in the same order (`user_input_set_active`, then `wm::focus_changed`) — no second focus
/// mechanism is invented here. `focus_changed` is what makes this a RESTORE: it takes a fresh `z` off
/// the same monotonic allocator for every window the owner has, which lifts a row that was sitting
/// below `SHELL_Z` back over the shell, and it publishes the owner's UNHIDE to the syscall layer,
/// which is the wake edge a parked vug needs. One gesture, both halves.
///
/// A kernel-owned row (the panel console) hands the KEYBOARD to the shell instead of to a program
/// with no input ring — the rule the router's furniture arm already states.
pub fn press_at(x: i32, y: i32) -> bool {
    if x < 0 || y < 0 {
        return false;
    }
    let (px, py) = (x as usize, y as usize);
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            return false;
        }
        (fb.width(), fb.height())
    };
    let Some(l) = Layout::for_panel(n, pw, ph) else {
        return false;
    };
    if !l.contains(px, py) {
        return false;
    }
    PRESSES_N.fetch_add(1, Ordering::Relaxed);
    let Some(t) = l.tile_at(px, py) else {
        serial_println!("[dock] press at ({},{}) -> strip tiles={} raised=none", x, y, n);
        return true; // the dock's own background: consumed, raises nothing.
    };
    let r = rows[t];
    let was_hidden = !r.visible;
    PRESSED.store(r.id, Ordering::Release);
    if crate::video::wm::is_kernel_owner(r.owner_asid) {
        crate::arch::x86_64::syscall::user_input_set_active(0);
    } else {
        crate::arch::x86_64::syscall::user_input_set_active(r.owner_asid);
    }
    wm::focus_changed(r.owner_asid);
    PRESSED.store(wm::WIN_NONE, Ordering::Release);
    RAISES.fetch_add(1, Ordering::Relaxed);
    if was_hidden {
        UNHIDES.fetch_add(1, Ordering::Relaxed);
    }
    let now_visible = wm::info(r.id).map(|i| i.z > wm::shell_z()).unwrap_or(false);
    serial_println!(
        "[dock] press at ({},{}) tile={}/{} win={} owner={:#x} was_hidden={} -> raised={} unhid={}",
        x, y, t, n, r.id, r.owner_asid, was_hidden, now_visible,
        was_hidden && now_visible
    );
    true
}

// ---------------------------------------------------------------------------
// Witness
// ---------------------------------------------------------------------------

/// **What the dock cost, and what it drew.** One bounded line; the caller decides how often.
///
/// Deliberately NOT `witness`-gated, on `ceramic::witness`'s precedent: the metal image is built
/// without `witness`, and a cost claim absent from that artifact is not a claim.
///
/// `scan_cyc` is what EVERY composite pass pays for the dock existing (the model scan, the hash and
/// the compare). `paint_cyc` is what a repaint costs, and `paints/passes` is the repaint RATE — the
/// number that says whether the strip is damage-driven or is quietly redrawing every frame. A dock
/// that repainted per frame would show `paints == passes`, so the claim is falsifiable from the wire.
pub fn rollup(scope: &str) {
    let passes = PASSES.load(Ordering::Relaxed).max(1);
    let paints = PAINTS.load(Ordering::Relaxed);
    let scan = SCAN_CYC.load(Ordering::Relaxed) / passes;
    let paint = PAINT_CYC.load(Ordering::Relaxed) / paints.max(1);
    serial_println!(
        "[dock] {} passes={} paints={} rate={}/1k scan={}cyc/{}us paint={}cyc/{}us px/paint={} presses={} raises={} unhides={}",
        scope,
        PASSES.load(Ordering::Relaxed),
        paints,
        (paints * 1000) / passes,
        scan,
        cycles_to_us(scan),
        paint,
        cycles_to_us(paint),
        PAINT_PX.load(Ordering::Relaxed) / paints.max(1),
        PRESSES_N.load(Ordering::Relaxed),
        RAISES.load(Ordering::Relaxed),
        UNHIDES.load(Ordering::Relaxed),
    );
}

/// How often the live ledger speaks. 5 s, matching `wm`'s own `WCN_ROLLUP_MS` so a capture carries
/// the dock's line at the same cadence as the compositor's and the two can be read side by side.
const ROLLUP_PERIOD_US: u64 = 5_000_000;

/// The cycle count at the last live rollup. `0` means "never", which makes the FIRST pass print —
/// deliberately: a boot whose dock never speaks is then distinguishable from a boot whose dock never
/// ran, and the first line also states the ledger is alive before any window has moved.
static ROLLUP_LAST: AtomicU64 = AtomicU64::new(0);

/// Emit the live ledger if this pass is the one that owes it.
///
/// Rate-limited on the free-running counter rather than on a pass count, so the cadence is the same
/// whether the panel is idle or busy. The `compare_exchange` is what keeps two cores from printing
/// the same interval twice; a loser simply skips, which is correct — the counters are cumulative and
/// the next interval reports everything.
fn rollup_tick() {
    let now = crate::arch::now_cycles();
    let last = ROLLUP_LAST.load(Ordering::Relaxed);
    if last != 0 && cycles_to_us(now.saturating_sub(last)) < ROLLUP_PERIOD_US {
        return;
    }
    if ROLLUP_LAST
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    rollup("live");
}

/// `rdtsc` ticks to microseconds, at the rate `apic::calibrate` measured against the ACPI PM timer.
///
/// The same arithmetic and the same uncalibrated fallback `wcg::cycles_to_us` uses — restated here
/// rather than called, because `wcg` is `witness`-gated and this ledger deliberately is not (the
/// metal image is built without `witness`). Two consumers of an unknown TSC rate in this kernel, one
/// guess: 1.25 GHz, which is what `arch::HW_WAIT_BUDGET` already assumes.
#[inline]
fn cycles_to_us(dt: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    dt.saturating_mul(1_000_000) / hz
}

/// DOCK fixture — **a minimised window is restorable, and the tile that restores it is the tile the
/// painter drew.**
///
/// Five legs, each able to FAIL on its own:
///
/// ### CONSOLEWIN — the restored window is KERNEL FURNITURE, and that is what this fixture is for
/// The third row carries a reserved kernel owner rather than an ordinary ASID. It is the same row
/// legs 1-4 already minimised and brought back, so the shape of the fixture is unchanged; what
/// changes is that the claim now covers the one window whose reversibility has no other route.
/// `<TAB>` cannot restore a parked console — x86's `focus_ring_apps` filters the reserved band out
/// of the focus rotation — so the dock IS the console's way back, and `wm::minimise`'s standing
/// precondition ("a control that hides a window with no way back is worse than an inert one") rests
/// on this leg for every kernel row. `wm::dock_scan` has included kernel-owned rows since the module
/// landed; what did not exist was a leg that would notice if that stopped being true.
///
/// It also means the row must be parked EXPLICITLY (see the `wm::minimise` call below): furniture is
/// exempt from the shell raise, so `focus_changed(0)` no longer puts it under, and the fixture uses
/// the gesture the operator's minimise disc actually calls.
///
/// 1. **the model** — three windows are minted (three distinct owners), `focus_changed(0)` pushes
///    every row below the shell, two raises bring two of them back, and the third is minimised. The
///    dock must report exactly THREE tiles, of which exactly ONE is not visible. A dock that
///    enumerated only the windows on the panel would report two here, which is the whole defect this
///    module exists to prevent — a minimised window with no way back.
/// 2. **geometry agreement** — the press point is TILE `k`'s centre as [`Layout`] computes it, and
///    `Layout::tile_at` must answer `k` for it. Painter and router share the accessor, so this leg
///    fails only if the accessor is internally inconsistent.
/// 3. **the restore** — a synthetic press at the HIDDEN window's tile centre must be CONSUMED, and
///    the window must come back: `z > shell_z()` where it was `<` before. This is the load-bearing
///    claim.
/// 4. **specificity** — the press must have raised THAT window and not merely raised something. The
///    other hidden-then-raised rows are checked by identity, so an off-by-one in `tile_at` (the
///    classic painter/router drift) fails here rather than passing by luck.
/// 5. **the miss** — a press one pixel ABOVE the strip must NOT be consumed. Without this leg the
///    module could consume the whole panel and still pass legs 1-4; with it, "the dock does not
///    swallow the desktop" is checked rather than asserted.
///
/// Self-cleaning: the three rows are closed and the focus state is restored. Runs on the real panel,
/// so it belongs after every one-shot per-window latch — the same ordering rule
/// `wm::hittest_selftest` states at its own call site.
#[cfg(feature = "witness")]
pub fn selftest() {
    use core::sync::atomic::AtomicBool as OnceBool;
    static DONE: OnceBool = OnceBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    /// Three 8x8 ARGB8888 surfaces in rodata — read-only, because the compositor only reads.
    static SURF: [[u32; 64]; 3] = [[0x0020_40FF; 64], [0x0040_FF20; 64], [0x00FF_4020; 64]];
    /// CONSOLEWIN — the THIRD owner is kernel FURNITURE, and it is the row leg 3 restores.
    ///
    /// It was an ordinary ASID (`0xD0C3`). The change is what makes this fixture the x86 witness for
    /// the arc's reopen claim, and it costs nothing: `w[2]` is already the row the legs minimise and
    /// bring back, so the only thing that moves is WHICH owner is being proved restorable.
    ///
    /// Why it has to be this row and not an extra fourth one: the console's route back is the only
    /// one in the system that `<TAB>` cannot serve — x86's `focus_ring_apps` filters the reserved
    /// band out of the focus rotation, so a parked kernel row is not in the ring at all. The dock is
    /// therefore not a convenience for it, it is the whole of its reversibility, and
    /// `wm::minimise`'s precondition ("a control that hides a window with no way back is worse than
    /// an inert one") rests on THIS leg for furniture. `dock_scan` includes kernel-owned rows by
    /// design and has since the module landed; what was missing was a leg that would notice if that
    /// stopped being true.
    ///
    /// Deliberately NOT `KERNEL_OWNER_CONSOLE`: the real console row carries that owner on a live
    /// x86 boot, and a fixture sharing it would make `owner_hidden` and every owner-scoped raise
    /// ambiguous between the operator's console and a synthetic 8x8 square. Any value in the
    /// reserved band satisfies `is_kernel_owner`, which is the arm under test.
    const OWNER_FURNITURE: u64 = wm::KERNEL_OWNER_BASE + 0x50;
    const OWNERS: [u64; 3] = [0xD0C1, 0xD0C2, OWNER_FURNITURE];
    const NAMES: [&[u8]; 3] = [b"dockA", b"dockB", b"dockC"];

    let saved_focus = crate::arch::x86_64::syscall::user_input_active();
    let mut w = [wm::WIN_NONE; 3];
    for k in 0..3 {
        w[k] = wm::create(
            OWNERS[k],
            SURF[k].as_ptr() as usize,
            core::mem::size_of_val(&SURF[k]),
            8,
            8,
            // STRIDE IS IN BYTES, not pixels — `create_inner`'s extent contract is
            // `w * 4 <= stride` and `h * stride <= surf_len`. 8 px * 4 B = 32 B a row, 8 rows = the
            // whole 256-byte surface.
            32,
            NAMES[k],
        );
    }
    if w.iter().any(|&i| i == wm::WIN_NONE) {
        serial_println!(
            ":: DOCK: fixture — table full, wins={:?} :: SKIP ::",
            w
        );
        for &i in w.iter() {
            if i != wm::WIN_NONE {
                wm::close(i);
            }
        }
        return;
    }

    // Every row below the shell, then bring TWO of the three back: w[2] stays minimised.
    wm::focus_changed(0);
    wm::focus_changed(OWNERS[0]);
    wm::focus_changed(OWNERS[1]);
    // CONSOLEWIN — **and w[2] is parked EXPLICITLY, because the shell raise no longer parks it.**
    //
    // `w[2]` is kernel furniture now, and furniture is exempt from the incidental hide: a shell raise
    // sweeping past it leaves it composited, which is the whole of the CLOSEISO fix and is asserted
    // by `wm::closeiso_selftest` leg 1. So the `focus_changed(0)` above — which used to be what put
    // this row under — does nothing to it, and legs 1 and 3 would be reading a VISIBLE row and
    // proving nothing.
    //
    // The fix is not to weaken the exemption, it is to use the gesture the arc actually wired: an
    // operator pressing this window's own minimise disc. `wm::minimise` is what that disc calls, and
    // it is a DELIBERATE park, which `wm::above_shell` honours for furniture where it ignores the
    // shell raise. The outcome token is asserted rather than discarded — `parked` means the row went
    // down AND its owner is hidden, so a `declined` or an `already` from a future guard that started
    // refusing kernel rows again fails here loudly instead of leaving legs 1-4 to fail obscurely.
    let park = wm::minimise(w[2]);
    let park_ok = park == "parked";

    // Leg 1 — the model. Ours are the three rows we just made; the live console/desktop rows are in
    // the table too, so the assertions are made about OUR ids rather than about the total.
    let mut rows = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    let (n, _) = wm::dock_scan(&mut rows, (0, 0, 0, 0));
    let mine: [Option<usize>; 3] = [
        rows[..n].iter().position(|r| r.id == w[0]),
        rows[..n].iter().position(|r| r.id == w[1]),
        rows[..n].iter().position(|r| r.id == w[2]),
    ];
    let model_ok = mine.iter().all(|m| m.is_some())
        && mine[0].map(|i| rows[i].visible) == Some(true)
        && mine[1].map(|i| rows[i].visible) == Some(true)
        // THE leg: the minimised window is IN the model, and is marked as off the panel.
        && mine[2].map(|i| rows[i].visible) == Some(false);

    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        (fb.width(), fb.height())
    };
    let layout = Layout::for_panel(n, pw, ph);

    // Legs 2-5.
    let (geom_ok, restore_ok, specific_ok, miss_ok, probe) = match (layout, mine[2]) {
        (Some(l), Some(k)) => {
            let (tx, ty, tw, th) = l.tile(k).unwrap_or((0, 0, 0, 0));
            let (cx, cy) = (tx + tw / 2, ty + th / 2);
            // Leg 2 — the accessor agrees with itself: the centre of tile k IS in tile k.
            let geom = l.tile_at(cx, cy) == Some(k) && l.contains(cx, cy);
            // The window is below the shell before the press, or leg 3 proves nothing.
            let below_before = wm::info(w[2]).map(|i| i.z < wm::shell_z()).unwrap_or(false);
            // Leg 3 — the synthetic press.
            let consumed = press_at(cx as i32, cy as i32);
            let back = wm::info(w[2]).map(|i| i.z > wm::shell_z()).unwrap_or(false);
            let restore = consumed && below_before && back;
            // Leg 4 — it raised THAT window: w[2] is now the topmost of the three.
            let z = |id| wm::info(id).map(|i| i.z).unwrap_or(0);
            let specific = z(w[2]) > z(w[0]) && z(w[2]) > z(w[1]);
            // Leg 5 — one pixel above the strip is NOT the dock's.
            let miss = !press_at(l.x as i32 + 1, l.y as i32 - 1);
            (geom, restore, specific, miss, Some((cx, cy)))
        }
        _ => (false, false, false, false, None),
    };

    // Leg 6 — THE STRIP OWES ITS VACATED PIXELS. Closing three windows makes the dock narrower (or
    // removes it), and the rect it reports as painted must FOLLOW: a strip that shrank without
    // erasing what it vacated would still be claiming — and still be showing — the wide rect. This is
    // the defect the leg was written for, found by review rather than by the panel.
    let rect_before = PAINTED_RECT.load(Ordering::Acquire);
    for &i in w.iter() {
        wm::close(i);
    }
    let rect_after = PAINTED_RECT.load(Ordering::Acquire);
    let vacate_ok = rect_before == 0 || rect_after != rect_before;
    crate::arch::x86_64::syscall::user_input_set_active(saved_focus);
    wm::focus_changed(saved_focus);

    let ok = model_ok && geom_ok && restore_ok && specific_ok && miss_ok && vacate_ok && park_ok;
    let (px, py) = probe.unwrap_or((0, 0));
    let (lx, lw, lg) = layout.map(|l| (l.x, l.w, l.glyphs)).unwrap_or((0, 0, 0));
    if ok {
        serial_println!(
            ":: DOCK: strip tiles={} at x={} w={} glyphs={}, probe=({},{}) model={} geom={} restore={} specific={} miss={} vacate={} furniture park={}/{} :: PASS ::",
            n, lx, lw, lg, px, py, model_ok, geom_ok, restore_ok, specific_ok, miss_ok, vacate_ok,
            park, park_ok
        );
    } else {
        serial_println!(
            ":: DOCK: strip tiles={} at x={} w={} glyphs={}, probe=({},{}) model={} geom={} restore={} specific={} miss={} vacate={} furniture park={}/{} :: FAIL ::",
            n, lx, lw, lg, px, py, model_ok, geom_ok, restore_ok, specific_ok, miss_ok, vacate_ok,
            park, park_ok
        );
    }
    rollup("selftest");
}
