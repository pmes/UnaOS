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
//! # STRIPFACTOR — the dock is now TENANT #1 of [`super::strip`], and unchanged by being one
//!
//! Peter's 2026-08-11 direction (*"UnaOS is a spatial game-engine OS … we will not always have a
//! menu bar"*) made the kernel's contribution the STRIP MECHANISM rather than any particular strip.
//! Everything below that was general — edge-anchored geometry with floors, the staged row-run
//! painter, the vacated-pixel erase, the damage slot, the cost ledger, the rounded-corner and disc
//! arithmetic, and occlusion citizenship — moved to [`super::strip`] and is shared with
//! [`super::menubar`]. What stayed here is everything a DOCK is and a strip is not: the tile model,
//! the tile arithmetic, the caption budget, the running pip, and the raise-and-unhide press.
//!
//! **Nothing about the dock's behaviour moved with it.** The tile geometry, the damage conditions,
//! the paint order, the colours, and the press routing are the same code reading the same constants;
//! the primitive received the machinery verbatim rather than a reimplementation of it. Two things
//! about the ARTIFACT did change, and are disclosed rather than implied away:
//!
//!  * the not-word4 decline line is emitted by the primitive for every tenant, so it reads
//!    `[strip] decline reason=not-word4` where it read `[dock] decline reason=not-word4`;
//!  * [`super::strip::MAX_STRIP_W`] is 4096, not this module's old 2048, because a FLUSH tenant is
//!    the panel's full width and the bench panels reach 2880. The scratch is 32 KiB of `.bss`, up
//!    from 16. The dock's own `const` proof that its worst-case layout fits is unchanged and still
//!    checked below.
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

use super::{ceramic, strip, theme, wm};

// ---------------------------------------------------------------------------
// Metrics — every one a `theme` name, or derived from one with the derivation
// written out. Nothing here is a bare literal with a look chosen for it.
// ---------------------------------------------------------------------------

/// Padding inside the strip, gap between tiles, and the strip's margin off the panel's bottom edge —
/// all [`theme::GAP`], the kit's one "standard gap between controls", by way of the primitive's
/// [`strip::PAD`] so a strip's margin and a tenant's padding cannot drift apart.
const PAD: usize = strip::PAD;

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
///
/// `pub` since STRIPFACTOR: [`super::menubar`]'s floor is derived from it (the bar must not crowd the
/// dock off a short panel), and a second copy of this arithmetic there is exactly the drift the
/// single-accessor law forbids.
pub const STRIP_H: usize = TILE_H + 2 * PAD;

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

/// The widest strip the primitive will compose — [`strip::MAX_STRIP_W`], shared with every tenant.
///
/// The dock's own worst case is unchanged and is still what the `const` proof below checks: a full
/// table of [`wm::MAX_WINDOWS`] tiles at [`LABEL_MAX`] glyphs is
/// `2*PAD + 12*(2*PAD + 8*CELL) + 11*PAD` = `24 + 12*152 + 132` = 1980 px. STRIPFACTOR raised the
/// shared bound to 4096 for the flush tenants; the dock neither needs nor is affected by the extra
/// width, and it no longer owns the scratch that provides it.
const MAX_STRIP_W: usize = strip::MAX_STRIP_W;

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
        let mut glyphs = LABEL_MAX;
        loop {
            let tile_w = 2 * PAD + glyphs * CELL;
            let w = 2 * PAD + n * tile_w + (n - 1) * PAD;
            // STRIPFACTOR — the anchoring, the margin and BOTH floors are the primitive's
            // `frame_centred`: `ph < STRIP_H + 2*PAD`, `w + 2*PAD > pw` and `w > MAX_STRIP_W` were
            // three separate tests here and are the same three there, in the same order, against the
            // same constants. The step-down loop stays, because auto-sizing the caption to the panel
            // is the DOCK's arithmetic, not any strip's.
            if let Some((x, y, w, h)) = strip::frame_centred(strip::Edge::Bottom, w, STRIP_H, pw, ph)
            {
                return Some(Layout { x, y, w, h, n, tile_w, glyphs });
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
        strip::contains(self.rect(), STRIP_R, px, py)
    }

    /// The strip as a plain rect, for the damage question.
    #[inline]
    pub fn rect(&self) -> (usize, usize, usize, usize) {
        (self.x, self.y, self.w, self.h)
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// STRIPFACTOR — the tenant's registry hook: **the dock's rect on this panel, or `None`.**
///
/// Registered as [`strip::TENANTS`]`[strip::DOCK_SLOT]`, so this is what `wm::erase_clip` reads. It
/// is [`Layout::for_panel`] and nothing else — the dock's one source of tile arithmetic, reached
/// through the tile count [`wm::dock_scan`] reports — which is the same expression the erase clip
/// used to inline. The dock is unconditionally PRESENT (there is no disable for it: it is the
/// console's only way back), so `None` here means only that the panel cannot host it.
pub fn strip_rect(pw: usize, ph: usize) -> Option<strip::Rect> {
    let mut tiles = [wm::DockEntry::empty(); wm::MAX_WINDOWS];
    // A zero rect asks the damage question nothing; only the tile count is wanted here.
    let (n, _) = wm::dock_scan(&mut tiles, (0, 0, 0, 0));
    Layout::for_panel(n, pw, ph).map(|l| l.rect())
}

/// What the dock last put on the panel — signature and rect, in the primitive's [`strip::Slot`].
/// The rect is read by [`compose`] to ask `wm` the damage question BEFORE it knows this pass's
/// layout; a stale rect is safe, because a layout change is also a signature change and repaints
/// regardless.
static SLOT: strip::Slot = strip::Slot::new();

/// The window id whose tile is held down, or `wm::WIN_NONE`. Cleared by the raise that follows.
static PRESSED: AtomicU32 = AtomicU32::new(wm::WIN_NONE);

/// Cost ledger — the primitive's [`strip::Ledger`]. Not `witness`-gated: the metal image is built
/// WITHOUT `witness`, and a performance claim that is absent from the only artifact that matters is
/// not a claim.
static LEDGER: strip::Ledger = strip::Ledger::new();

/// The dock's own vocabulary, appended to the ledger's common terms so the line is unchanged.
static PRESSES_N: AtomicU64 = AtomicU64::new(0);
static RAISES: AtomicU64 = AtomicU64::new(0);
static UNHIDES: AtomicU64 = AtomicU64::new(0);
/// WCK5 — **passes in which a window had painted over the strip.** The repaint this arc is removing.
///
/// `paints` conflates the two damage conditions: a MODEL change (a window opened, closed, was renamed
/// or changed focus — a repaint the dock owes and always will) and a CLOBBER (a window blit published
/// over the strip, so the strip has to put itself back). During a sustained drag the model does not
/// change at all, so before WCK5 every paint of a drag was a clobber and the strip was being redrawn
/// at motion rate — Peter's "it goes away dragging any window", from the panel's own ledger.
///
/// `occclip_dock_px` proves the withholding on a `witness` build; this proves the CONSEQUENCE on the
/// metal image, which is built without `witness` and is the only artifact the symptom was ever seen
/// on. Deliberately counted at the damage question rather than at the paint, so it stays readable
/// when a model change and a clobber coincide.
static CLOBBERS: AtomicU64 = AtomicU64::new(0);

/// The dock's tail on every ledger line: the WCK5 clobber count, presses, and what they did. A macro
/// rather than a function because `format_args!` borrows its arguments and the result cannot outlive
/// the call.
///
/// STRIPFACTOR × WCK5 — `clob=` is dock-specific (WCK5's "a window painted over the strip" counter)
/// and rides the tail rather than the primitive's common terms, because the menu bar has no such
/// counter and a shared field would print a meaningless `clob=0` for it. It moved from between
/// `paints=` and `rate=` (WCK5's inline `serial_println!`) to the tail when the rollup folded onto
/// `strip::Ledger`; no spec pins its position, so the reconciliation is a field reorder the analyzer
/// and the FORBIDs (which match `clob=` anywhere) do not see.
macro_rules! dock_tail {
    () => {
        format_args!(
            "clob={} presses={} raises={} unhides={}",
            CLOBBERS.load(Ordering::Relaxed),
            PRESSES_N.load(Ordering::Relaxed),
            RAISES.load(Ordering::Relaxed),
            UNHIDES.load(Ordering::Relaxed)
        )
    };
}

/// FNV-1a 64 over the tile model — the whole "has anything changed?" test, reduced to one integer.
///
/// Everything the painter reads goes in: the window id, the owner, the caption bytes, the
/// visible/focused bits, the pressed id, and the layout (which folds in the panel geometry and the
/// glyph budget). A field the painter uses and this hash omits is a field whose change would leave a
/// stale strip on the panel, so the two lists are the same list on purpose.
fn signature(e: &[wm::DockEntry], l: &Layout, pressed: u32) -> u64 {
    // STRIPFACTOR — the same FNV-1a 64, from the primitive, over the same fields in the same order.
    let mut h = strip::FNV_BASIS;
    for v in [
        l.x as u64, l.y as u64, l.w as u64, l.h as u64,
        l.n as u64, l.tile_w as u64, l.glyphs as u64, pressed as u64,
    ] {
        h = strip::fnv1a_u64(h, v);
    }
    for r in e {
        for k in 0..4 {
            h = strip::fnv1a(h, ((r.id >> (k * 8)) & 0xFF) as u8);
        }
        h = strip::fnv1a_u64(h, r.owner_asid);
        h = strip::fnv1a(h, r.visible as u8);
        h = strip::fnv1a(h, r.focused as u8);
        h = strip::fnv1a(h, r.title_len as u8);
        for &b in r.title[..r.title_len.min(wm::MAX_TITLE)].iter() {
            h = strip::fnv1a(h, b);
        }
    }
    // A zero signature means "nothing painted"; fold it away so a real model can never collide with
    // the empty state.
    strip::seal(h)
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
    let (n, clobbered) = wm::dock_scan(&mut rows, SLOT.rect());
    // WCK5 — one relaxed add on the pass that was clobbered, and nothing at all on the quiet pass.
    if clobbered {
        CLOBBERS.fetch_add(1, Ordering::Relaxed);
    }
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));
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
    let painted_sig = SLOT.sig();
    LEDGER.pass(crate::arch::now_cycles().saturating_sub(t0));

    // The ledger reaches the METAL image, or it is not a performance claim.
    //
    // `rollup` is not `witness`-gated, but a function with no caller outside `witness` is not in the
    // artifact either — the linker drops it and the claim evaporates exactly where it matters. So the
    // live emitter is HERE, on the path every pass takes, rate-limited by the primitive's `tick`.
    // Cost on a pass that does not print: one relaxed load and a compare.
    LEDGER.tick("dock", dock_tail!());

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
    let old = SLOT.packed();
    let new = strip::pack_rect(layout.map(|l| l.rect()));
    let vacated = if old != 0 && old != new { Some(strip::unpack_rect(old)) } else { None };

    let Some(l) = layout else {
        SLOT.clear();
        return match vacated {
            Some(v) => strip::erase_rect(v),
            None => false,
        };
    };

    let t1 = crate::arch::now_cycles();
    if let Some(v) = vacated {
        // Erase FIRST, then paint: the new strip lands on top of the cleaned area, so the two never
        // race to own an overlapping pixel and the panel never shows a half-erased strip.
        strip::erase_rect(v);
    }
    if !paint(&l, &rows[..n], pressed) {
        return false;
    }
    LEDGER.paint(
        crate::arch::now_cycles().saturating_sub(t1),
        (l.w * l.h) as u64,
    );
    SLOT.store(sig, Some(l.rect()));
    true
}

/// Paint the strip. Returns `false` without touching the panel if it could not (no scratch, or a
/// surface whose layout the row-run path does not cover).
///
/// The one framebuffer writer in this module, and it writes the way the subsystem's law requires:
/// compose a row in cached RAM, copy it out with one `blit`, clean the whole rect once at the end.
fn paint(l: &Layout, rows: &[wm::DockEntry], pressed: u32) -> bool {
    // STRIPFACTOR — the whole body moved to `strip::paint`: the readiness and word4 checks, the
    // bounds check, the scratch `try_lock`, the cursor bracket, the per-row encode memo, the row
    // `blit` and the single `flush_rect`. This function is now the dock's row composer and nothing
    // else, which is exactly the split that lets a second tenant exist.
    strip::paint("dock", l.rect(), |out, j| compose_row(out, l, rows, pressed, j))
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
fn compose_row(out: &mut [u32], l: &Layout, rows: &[wm::DockEntry], pressed: u32, j: usize) {
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
            if strip::corner_cut(i, j, l.w, l.h, STRIP_R) {
                // The pixels the painter cuts out of the corners are filled with the DESKTOP, exactly
                // as `wm::paint_window` fills a window's cut head corners. Same rule, same colour,
                // and `Layout::contains` declines the same pixels so a press there falls through.
                out[i] = wm::DESKTOP_BG;
            } else if strip::edge_ring(i, j, l.w, l.h, STRIP_R) {
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
                if strip::in_disc(i, j, px0, py0, d) {
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
                if !strip::corner_cut(i - bx, j - by, tw, th, TILE_R) {
                    out[i] = tface;
                }
            }
            for i in mid0..mid1 {
                out[i] = tface;
            }
            for i in mid1..hi {
                if !strip::corner_cut(i - bx, j - by, tw, th, TILE_R) {
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

// STRIPFACTOR — `edge_ring` moved to `strip::edge_ring`, which takes the radius as an argument
// rather than closing over this module's `STRIP_R`. Same four probes, same order, same result for
// the dock's radius; a flush tenant passes 0 and pays one compare.

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
    // STRIPFACTOR × WCK5 — the common terms come from the primitive's `Ledger`; the dock's own four
    // (WCK5's clobber count, presses, raises, unhides) are its tail. Every field WCK5's inline line
    // carried survives — `clob=` moved from between `paints=` and `rate=` to the tail, which no spec
    // pins.
    LEDGER.rollup("dock", scope, dock_tail!());
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

    // STRIPFACTOR — the STRIP battery runs as one battery, and the menu bar's fixture is driven from
    // here rather than from its own call site.
    //
    // ⚠ Stated plainly, because it is a compromise and not a design: the natural home is beside
    // `dock::selftest`'s own invocation in `arch/x86_64/syscall.rs`, and that file is owned by a
    // concurrent arc and outside this arc's lane. Driving it here buys the identical preconditions —
    // the same `witness` gate, the same real panel, the same "after every one-shot per-window latch"
    // ordering — at the cost of coupling two fixtures that are not otherwise related.
    //
    // It runs BEFORE this fixture mints its rows, deliberately: the bar's legs are about the registry
    // and the panel's edges, not about the window table, and running first means they cannot be
    // perturbed by three synthetic windows — nor skipped by the `SKIP` return below if the table is
    // full.
    //
    // ⚠ **FOR THE INTEGRATOR — re-seat this in `arch/x86_64/syscall.rs` once that lane frees.** The
    // canonical home is line ~15477 there, immediately after `crate::video::dock::selftest();`, as
    // its own statement: `crate::video::menubar::selftest();`. It belongs beside the dock's call, not
    // nested inside the dock fixture — the two fixtures are unrelated and this nesting is a lane
    // compromise, not a design. When moved, DELETE this call and this comment block; nothing in either
    // fixture depends on the coupling, and `menubar::selftest`'s own one-shot `DONE` latch makes the
    // move idempotent (a double-drive from a botched move runs once, not twice).
    super::menubar::selftest();

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
    let rect_before = SLOT.packed();
    for &i in w.iter() {
        wm::close(i);
    }
    let rect_after = SLOT.packed();
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
