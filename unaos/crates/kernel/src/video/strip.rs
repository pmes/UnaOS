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

//! STRIP — the furniture-strip PRIMITIVE: an edge-anchored slab that is not a window.
//!
//! # Why this module exists, and what it is NOT
//!
//! Peter's direction, 2026-08-11: **UnaOS is a spatial game-engine OS.** The desktop is one shell
//! running on it, not the OS's identity — *"we will not always have a menu bar"*. So the kernel does
//! not get a menu bar as a feature. It gets the MECHANISM a menu bar is one instance of, and the
//! desktop shell's dock is another, and a game's HUD band or an in-world status overlay is a third.
//!
//! Everything an edge strip needs and a window does not have is here, and exactly once:
//!
//!  * **geometry with floors** — [`frame_centred`] and [`frame_flush`], the two anchorings, each
//!    returning `None` rather than a squeezed rectangle on a panel that cannot host the strip;
//!  * **the front-buffer painter** — [`paint`], the row-run staged copy the WC-H/WC-K/WC-L law
//!    requires of every non-compositor painter, plus [`erase_rect`] for the pixels a strip vacates;
//!  * **the damage slot** — [`Slot`], one `u64` signature and one packed rect, which is the whole of
//!    "did anything change, and what did I last own";
//!  * **occlusion citizenship** — [`TENANTS`] and [`rects`], the registry `wm::erase_clip` walks so
//!    that a strip is a first-class occluder instead of a special case bolted onto the erase;
//!  * **the cost ledger** — [`Ledger`], so every strip's per-pass and per-repaint cost reaches the
//!    metal image on the same terms the dock's already did.
//!
//! What is NOT here is any strip's *content*: tiles, captions, clocks, menus, meters. A tenant owns
//! its own layout arithmetic and its own row composer, and hands this module a closure. That split is
//! deliberate — it is what lets a tenant be deleted without touching the primitive, and the primitive
//! be reused without inheriting a desktop's vocabulary.
//!
//! # The registry, and what ABSENT costs
//!
//! [`TENANTS`] is a `const` array of `STRIP_MAX` entries, each a name, an edge, and one function
//! pointer that answers *"what rectangle do you occupy on a `pw` x `ph` panel right now, if any?"*.
//!
//! A tenant that answers `None` is **absent, and absent is free**:
//!
//!  * it pushes no box into the erase clip, so it consumes no [`wm`]-side capacity at runtime;
//!  * its `compose` returns before reading a pixel (each tenant's own first line);
//!  * it owns no panel rows, so no other painter is clipped against it.
//!
//! The static sizing is the worst case and the runtime occupancy is what the panel pays. `STRIP_MAX`
//! is what `wm::OCC_MAX` reserves; a `const` assertion in `wm` ties the two together so a tenant
//! added here without widening the clip fails the BUILD rather than dropping an occluder on a
//! non-witness image — the exact silent hole the WCK4 review named.
//!
//! # Two panels now, and gated on each
//!
//! `#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature =
//! "desktop_firmware")))]` at the `mod` declaration in [`super`] — the same gate [`super::dock`],
//! [`super::menubar`] and [`super::crystal`] carry. It used to be the x86 term alone; PI-DESK added
//! the second half, and the two are independent.
//!
//! **A knob-off aarch64 build is still BYTE-IDENTICAL with this file present** — measured against the
//! pre-arc `kernel8.img` by sha256, not asserted — because it is not compiled there, `wm`'s compose
//! seam and `erase_clip`'s furniture arm carry the same gate, and the aarch64 erase path keeps the
//! pixel-identity `pi4-regression.spec` pins on `[wc-k]`. Knob ON, this module composes at the tail of
//! `wm::composite_once` on the BCM2711 panel exactly as it does on the x86 one.
//!
//! Nothing here had to become arch-neutral to cross: the geometry, the row-run painter, the damage
//! slot and the registry were always integer arithmetic over `wm` and the materials. The single
//! exception is [`cycles_to_us`], whose input `arch::now_cycles()` is arch-neutral but whose RATE is
//! not — see its own note for why the aarch64 arm reads CNTFRQ_EL0 instead of inheriting x86's
//! uncalibrated-TSC guess.

use super::{theme, wm};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// A rectangle on the panel: `(x, y, w, h)`. The same shape `wm`'s clip boxes use, so a strip's rect
/// crosses into the occlusion machinery with no conversion.
pub type Rect = (usize, usize, usize, usize);

/// Which edge a strip is anchored to.
///
/// Only the two horizontal edges exist, and that is a statement rather than an omission: a strip's
/// whole affordance is that it is a full-width-or-centred BAND, and the row-run painter below is
/// built on a band's rows being contiguous in the framebuffer. A vertical rail is a different object
/// with a different cost model (every row a separate short run) and would be a different primitive.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

/// The margin a centred strip keeps off its edge, and the padding tenants lay out inside themselves —
/// [`theme::GAP`], the kit's one "standard gap between controls".
pub const PAD: usize = theme::GAP;

/// The widest strip this module will compose, in panel pixels — the scratch row's length.
///
/// **Raised from the dock's own 2048 to cover a FLUSH strip**, and the derivation is stated because
/// the number moved: a centred strip is sized by its contents (the dock's worst case is 1980 px, see
/// its `const` block), but a flush strip is the panel's full width, and the panels this kernel drives
/// go up to the bench rMBP's 2880. 4096 covers every panel up to 4K wide; above it [`paint`] declines
/// once, loudly, rather than truncating a strip to a width the operator did not ask for.
///
/// Two scratch rows of `u32` = 32 KiB of `.bss`, up from 16.
pub const MAX_STRIP_W: usize = 4096;

/// The geometry of a strip that is **centred** on its edge with a [`PAD`] margin — the dock's shape.
///
/// `w` and `h` are the strip's own size, computed by the tenant from its contents. `None` when the
/// panel cannot host it: too short for the strip and its two margins, too narrow for the strip and
/// its two margins, or wider than the scratch can compose. A `None` here is the tenant's cue to draw
/// NOTHING — never to squeeze, because a squeezed strip is a strip whose painter and router have
/// stopped agreeing about where its contents are.
pub fn frame_centred(edge: Edge, w: usize, h: usize, pw: usize, ph: usize) -> Option<Rect> {
    if w == 0 || h == 0 || w > MAX_STRIP_W {
        return None;
    }
    if ph < h + 2 * PAD || w + 2 * PAD > pw {
        return None;
    }
    let y = match edge {
        Edge::Top => PAD,
        Edge::Bottom => ph - PAD - h,
    };
    Some(((pw - w) / 2, y, w, h))
}

/// The geometry of a strip that runs **flush** to its edge, corner to corner — a menu bar's shape,
/// and a HUD band's.
///
/// No margin and no centring: the strip owns the full panel width and sits at `y = 0` (top) or
/// `y = ph - h` (bottom). That is the DENSITY case the taste law asks for — one band serving every
/// client, with no cosmetic air around it — and it is why this is a second constructor rather than
/// `frame_centred` with a zero margin: the two differ in what they do with the leftover width, not
/// merely in a number.
///
/// `None` when the panel is shorter than `reserve` (the strip plus whatever the tenant declares it
/// must leave for the rest of the furniture) or wider than the scratch can compose.
pub fn frame_flush(edge: Edge, h: usize, reserve: usize, pw: usize, ph: usize) -> Option<Rect> {
    if h == 0 || pw == 0 || pw > MAX_STRIP_W {
        return None;
    }
    if ph < reserve.max(h) {
        return None;
    }
    let y = match edge {
        Edge::Top => 0,
        Edge::Bottom => ph - h,
    };
    Some((0, y, pw, h))
}

/// Is `(i, j)` outside the rounded corner of a `w` x `h` box with radius `r`?
///
/// All four corners, unlike `wm::corner_outside` (which cuts only the two TOP corners of a window
/// head): a centred strip is a free-floating slab, so it is rounded all the way round. Integer only —
/// `dx*dx + dy*dy > r*r` against the corner-circle centre. A flush strip passes `r == 0` and pays one
/// compare.
#[inline]
pub fn corner_cut(i: usize, j: usize, w: usize, h: usize, r: usize) -> bool {
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

/// The keyline follows the ROUNDED edge, not just the four straight sides: a pixel that is inside the
/// box but whose neighbour one step outward is cut belongs to the outline. One test, no second radius
/// table.
#[inline]
pub fn edge_ring(i: usize, j: usize, w: usize, h: usize, r: usize) -> bool {
    corner_cut(i.wrapping_sub(1).min(w - 1), j, w, h, r)
        || corner_cut((i + 1).min(w - 1), j, w, h, r)
        || corner_cut(i, j.wrapping_sub(1).min(h - 1), w, h, r)
        || corner_cut(i, (j + 1).min(h - 1), w, h, r)
}

/// Is `(i, j)` inside the filled disc of diameter `d` whose top-left is `(bx, by)`? Mirrors
/// `wm::in_circle`'s integer form; the dock's running pip and any tenant's status dot share it.
#[inline]
pub fn in_disc(i: usize, j: usize, bx: usize, by: usize, d: usize) -> bool {
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

/// Does `r` contain the panel point, with its corners cut at radius `rad`?
///
/// A press on a cut corner is a press on whatever is BEHIND the strip, exactly as `wm::hit_test`
/// treats a window's cut head corners — so a tenant that routes presses gets the same answer its
/// painter drew.
#[inline]
pub fn contains(r: Rect, rad: usize, px: usize, py: usize) -> bool {
    let (x, y, w, h) = r;
    px >= x && px < x + w && py >= y && py < y + h && !corner_cut(px - x, py - y, w, h, rad)
}

// ---------------------------------------------------------------------------
// The damage slot
// ---------------------------------------------------------------------------

/// What a strip last put on the panel: one signature and one rectangle.
///
/// The signature is the tenant's whole "has anything changed?" test reduced to an integer, and the
/// rect is what the strip must ERASE if it shrinks or goes away — because `wm::erase` cleans the
/// boxes of WINDOWS and a strip is not one, so no other painter knows its pixels are stale.
///
/// A tenant declares one of these as a `static`. `0` in either field means "nothing painted", which
/// is the state a teardown, a first boot, and a disabled tenant all leave.
pub struct Slot {
    sig: AtomicU64,
    rect: AtomicU64,
}

impl Slot {
    pub const fn new() -> Slot {
        Slot { sig: AtomicU64::new(0), rect: AtomicU64::new(0) }
    }

    /// The signature the strip on the panel was painted from.
    #[inline]
    pub fn sig(&self) -> u64 {
        self.sig.load(Ordering::Acquire)
    }

    /// The rect currently on the panel, unpacked; `(0,0,0,0)` for none.
    #[inline]
    pub fn rect(&self) -> Rect {
        unpack_rect(self.rect.load(Ordering::Acquire))
    }

    /// The rect currently on the panel, still packed — for the `!= 0` and `!=` tests the vacate rule
    /// makes, which want identity rather than fields.
    #[inline]
    pub fn packed(&self) -> u64 {
        self.rect.load(Ordering::Acquire)
    }

    /// Publish what was just painted. Both fields move together, always.
    #[inline]
    pub fn store(&self, sig: u64, r: Option<Rect>) {
        self.sig.store(sig, Ordering::Release);
        self.rect.store(pack_rect(r), Ordering::Release);
    }

    /// Publish "nothing is on the panel".
    #[inline]
    pub fn clear(&self) {
        self.store(0, None);
    }
}

/// Pack a rect into one `u64`, `y<<48 | x<<32 | h<<16 | w`. A zero-area rect packs to `0`, which is
/// the "none" sentinel — so "no strip" and "a strip of no size" are the same state, deliberately.
#[inline]
pub fn pack_rect(r: Option<Rect>) -> u64 {
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

/// The inverse of [`pack_rect`].
#[inline]
pub fn unpack_rect(v: u64) -> Rect {
    (
        ((v >> 32) & 0xFFFF) as usize,
        ((v >> 48) & 0xFFFF) as usize,
        (v & 0xFFFF) as usize,
        ((v >> 16) & 0xFFFF) as usize,
    )
}

/// FNV-1a 64, the hash every tenant's signature is built with. Exposed as a byte-at-a-time step so a
/// tenant folds exactly the fields its painter reads and nothing else — a field the painter uses and
/// the signature omits is a field whose change leaves a stale strip on the panel.
#[inline]
pub fn fnv1a(h: u64, b: u8) -> u64 {
    (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
}

/// The FNV-1a offset basis — a signature's starting value.
pub const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// Fold a `u64`'s eight bytes into a running signature, little end first.
#[inline]
pub fn fnv1a_u64(mut h: u64, v: u64) -> u64 {
    for k in 0..8 {
        h = fnv1a(h, ((v >> (k * 8)) & 0xFF) as u8);
    }
    h
}

/// A signature can never be `0` — that value is reserved for "nothing painted", and a real model
/// colliding with it would leave the strip un-repainted forever.
#[inline]
pub fn seal(h: u64) -> u64 {
    if h == 0 { 1 } else { h }
}

// ---------------------------------------------------------------------------
// The painter — front-buffer discipline, shared
// ---------------------------------------------------------------------------

/// The scratch every strip composes in — CACHED RAM, never the scan-out. One row of logical colours
/// and one of pre-encoded framebuffer words; the encode is hoisted out of the pixel loop through a
/// tiny memo, exactly as `FrameBuffer::encode4` was built for.
///
/// **One scratch for every tenant, not one each.** `try_lock`, never `lock`: a contended pass
/// declines and repaints on the next one rather than spinning inside a composite — `wm::stage_fill`'s
/// own rule for `STAGE`. Two strips wanting to repaint in the same pass therefore serialise, and the
/// loser repaints one pass later, which is the behaviour a shared 32 KiB buffer buys over 32 KiB per
/// tenant.
struct Scratch {
    log: [u32; MAX_STRIP_W],
    raw: [u32; MAX_STRIP_W],
}

static SCRATCH: spin::Mutex<Scratch> = spin::Mutex::new(Scratch {
    log: [0; MAX_STRIP_W],
    raw: [0; MAX_STRIP_W],
});

/// One-shot: no strip can be composed on this surface (not a 4-byte word-aligned layout), said once
/// for the whole primitive rather than once per tenant per pass.
static NOWORD_SAID: AtomicBool = AtomicBool::new(false);

/// **Paint a strip.** `compose_row(out, j)` fills `out[0..w]` with panel row `j` of the strip as
/// logical `0x00RRGGBB` colours; this function does everything else.
///
/// Returns `false` without touching the panel if it could not paint (surface not ready, a layout the
/// row-run path does not cover, a rect off the panel, or a contended scratch).
///
/// The one framebuffer writer in the strip stack, and it writes the way the subsystem's law requires:
/// compose a row in cached RAM, copy it out with one `blit`, clean the whole rect once at the end.
/// Nothing here reads the front buffer and nothing writes it per-pixel.
///
/// **The sprite bracket** is opened here and closed by the caller: [`super::cursor::undraw`] takes
/// the arrow off the panel BEFORE the first byte lands, and a `true` return is the caller's
/// obligation to upgrade the pass's cursor tail to `Repaint`. Without it these rows would overwrite
/// the sprite and the save-under would later restore pre-strip pixels over a freshly painted strip.
pub fn paint(name: &str, r: Rect, mut compose_row: impl FnMut(&mut [u32], usize)) -> bool {
    let (x, y, w, h) = r;
    if w == 0 || h == 0 || w > MAX_STRIP_W {
        return bar_decline(name, DECL_GEOM); // TEARSCOPE — counted, still `false`
    }
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return bar_decline(name, DECL_READY); // TEARSCOPE
    }
    if !fb.word4() {
        // Said ONCE for the whole primitive rather than once per tenant: the condition is a property
        // of the SURFACE, not of the strip that happened to notice it first, so a second tenant
        // repeating it every pass would be noise about the same fact. The name of the tenant that
        // noticed goes on the line so the reader knows which path reached it first.
        if !NOWORD_SAID.swap(true, Ordering::Relaxed) {
            serial_println!(
                "[strip] decline reason=not-word4 first={} — no strip composed on this surface",
                name
            );
        }
        return bar_decline(name, DECL_WORD); // TEARSCOPE
    }
    let info = fb.info();
    if x + w > info.width || y + h > info.height {
        return bar_decline(name, DECL_GEOM); // TEARSCOPE
    }
    let Some(mut s) = SCRATCH.try_lock() else {
        // contended: the next pass repaints (the signature is still unmatched).
        return bar_decline(name, DECL_LOCK); // TEARSCOPE
    };

    // CURSOR — take the arrow off the panel before the first byte lands. `wm::erase`'s bracket, and
    // for its reason: without it these rows would overwrite the sprite and the save-under would later
    // restore pre-strip pixels over a freshly painted strip. The caller restores it (we return
    // `true`, which upgrades the pass's cursor tail to `Repaint`).
    super::cursor::undraw();

    // TEARSCOPE — the PRESENT clock opens here, at the first byte, and closes after `flush_rect`.
    // It brackets exactly what reaches the SCAN-OUT and nothing else: the compose of each row happens
    // inside it because a strip composes row-by-row INTO the panel, which is the whole reason a bar's
    // present can outrun the beam where a window's staged blit cannot. See [`bar_painted`].
    // BEAM (orin 26) — hold clear of the beam BEFORE the present clock opens (the wait is the
    // hold's, not the paint's), publish inside the bracket, release after the clean below.
    let hb = super::beam::hold(y, y + h, info.height, false, true);
    let tp0 = crate::arch::now_cycles();
    let stride_b = info.stride * 4;
    for j in 0..h {
        compose_row(&mut s.log[..w], j);
        // Encode logical colours to this surface's words, with a two-entry memo: a strip row is long
        // runs of very few colours, so the memo hits on nearly every pixel and `encode4`'s match runs
        // a handful of times per row instead of `w` times.
        let (mut mc, mut mr) = (u32::MAX, 0u32);
        for i in 0..w {
            let c = s.log[i];
            if c != mc {
                mc = c;
                mr = fb.encode4(c).unwrap_or(0);
            }
            s.raw[i] = mr;
        }
        let off = (y + j) * stride_b + x * 4;
        // SAFETY: `raw` is a live `[u32; MAX_STRIP_W]` and `w <= MAX_STRIP_W` (checked above); the
        // byte view is `4 * w` bytes of it, correctly aligned and initialised. `blit` bounds-checks
        // the destination itself and is a no-op on an overrun.
        let bytes = unsafe { core::slice::from_raw_parts(s.raw.as_ptr() as *const u8, w * 4) };
        fb.blit(off, bytes);
    }
    fb.flush_rect(x, y, w, h);
    let _ = super::beam::settle(hb);
    // TEARSCOPE — record what this present cost against what the beam spends on the same rows.
    bar_painted(name, r, crate::arch::now_cycles().saturating_sub(tp0), info.height);
    true
}

/// Fill a vacated strip rect with the desktop colour, through the SAME staged row-run path [`paint`]
/// uses. Returns `true` if it painted (so the caller owes the sprite a `Repaint`).
///
/// **A strip owes its own vacated pixels.** `wm::erase` cleans the boxes of WINDOWS; a strip is not a
/// window and no other painter knows its rect, so a strip that shrinks or goes away would leave its
/// old ends standing on the panel until something else happened to paint over them. The rule is the
/// one `wm::close` follows: erase what you vacate, in the same pass, through the same staged path.
///
/// `wm::erase` would be the natural call and is not used: it is private, it takes a slice of boxes,
/// and it opens and closes its own cursor bracket, which re-entering from the tail of a composite
/// pass would nest inside the one this module already holds. One row-blit loop over `DESKTOP_BG` is
/// the smaller thing.
pub fn erase_rect(r: Rect) -> bool {
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
    // BEAM (orin 26) — bracketed like `paint`, UNRECORDED: no witness reads this erase, and a
    // recorded observation nobody takes would be handed to the next `bar_painted` on this core.
    let hb = super::beam::hold(y, y + h, info.height, false, false);
    for j in 0..h {
        // SAFETY: as in `paint` — `raw` is a live `[u32; MAX_STRIP_W]`, `w <= MAX_STRIP_W`, and
        // `blit` bounds-checks its destination.
        let bytes = unsafe { core::slice::from_raw_parts(s.raw.as_ptr() as *const u8, w * 4) };
        fb.blit((y + j) * stride_b + x * 4, bytes);
    }
    fb.flush_rect(x, y, w, h);
    let _ = super::beam::settle(hb);
    true
}

// ---------------------------------------------------------------------------
// The cost ledger
// ---------------------------------------------------------------------------

/// **What a strip cost, and what it drew.** One per tenant, declared as a `static`.
///
/// Deliberately NOT `witness`-gated, on `ceramic::witness`'s and the dock's precedent: the metal
/// image is built WITHOUT `witness`, and a cost claim absent from the only artifact that matters is
/// not a claim.
///
/// `scan_cyc` is what EVERY composite pass pays for the strip existing (the model scan, the hash and
/// the compare). `paint_cyc` is what a repaint costs, and `paints/passes` is the repaint RATE — the
/// number that says whether the strip is damage-driven or is quietly redrawing every frame. A strip
/// that repainted per frame would print `paints == passes`, so the claim is falsifiable from the wire.
pub struct Ledger {
    passes: AtomicU64,
    paints: AtomicU64,
    scan_cyc: AtomicU64,
    paint_cyc: AtomicU64,
    paint_px: AtomicU64,
    rollup_last: AtomicU64,
}

/// How often a live ledger speaks. 5 s, matching `wm`'s own `WCN_ROLLUP_MS` so a capture carries
/// every strip's line at the same cadence as the compositor's and they can be read side by side.
const ROLLUP_PERIOD_US: u64 = 5_000_000;

impl Ledger {
    pub const fn new() -> Ledger {
        Ledger {
            passes: AtomicU64::new(0),
            paints: AtomicU64::new(0),
            scan_cyc: AtomicU64::new(0),
            paint_cyc: AtomicU64::new(0),
            paint_px: AtomicU64::new(0),
            rollup_last: AtomicU64::new(0),
        }
    }

    /// Charge one composite pass, and the cycles its scan half took.
    #[inline]
    pub fn pass(&self, scan_cyc: u64) {
        self.passes.fetch_add(1, Ordering::Relaxed);
        self.scan_cyc.fetch_add(scan_cyc, Ordering::Relaxed);
    }

    /// Charge one repaint: its cycles and its pixels.
    #[inline]
    pub fn paint(&self, cyc: u64, px: u64) {
        self.paints.fetch_add(1, Ordering::Relaxed);
        self.paint_cyc.fetch_add(cyc, Ordering::Relaxed);
        self.paint_px.fetch_add(px, Ordering::Relaxed);
    }

    /// Emit the ledger. `tail` is the tenant's own vocabulary — presses, raises, picks — appended to
    /// the common terms so one line carries both halves and a capture has one line per strip.
    pub fn rollup(&self, name: &str, scope: &str, tail: core::fmt::Arguments<'_>) {
        let passes = self.passes.load(Ordering::Relaxed).max(1);
        let paints = self.paints.load(Ordering::Relaxed);
        let scan = self.scan_cyc.load(Ordering::Relaxed) / passes;
        let paint = self.paint_cyc.load(Ordering::Relaxed) / paints.max(1);
        serial_println!(
            "[{}] {} passes={} paints={} rate={}/1k scan={}cyc/{}us paint={}cyc/{}us px/paint={} {}",
            name,
            scope,
            self.passes.load(Ordering::Relaxed),
            paints,
            (paints * 1000) / passes,
            scan,
            cycles_to_us(scan),
            paint,
            cycles_to_us(paint),
            self.paint_px.load(Ordering::Relaxed) / paints.max(1),
            tail,
        );
    }

    /// Emit the live ledger if this pass is the one that owes it.
    ///
    /// Rate-limited on the free-running counter rather than on a pass count, so the cadence is the
    /// same whether the panel is idle or busy. The `compare_exchange` is what keeps two cores from
    /// printing the same interval twice; a loser simply skips, which is correct — the counters are
    /// cumulative and the next interval reports everything.
    ///
    /// A `rollup_last` of `0` means "never", which makes the FIRST pass print — deliberately: a boot
    /// whose strip never speaks is then distinguishable from a boot whose strip never ran.
    pub fn tick(&self, name: &str, tail: core::fmt::Arguments<'_>) {
        let now = crate::arch::now_cycles();
        let last = self.rollup_last.load(Ordering::Relaxed);
        if last != 0 && cycles_to_us(now.saturating_sub(last)) < ROLLUP_PERIOD_US {
            return;
        }
        if self
            .rollup_last
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        self.rollup(name, "live", tail);
    }
}

/// `rdtsc` ticks to microseconds, at the rate `apic::calibrate` measured against the ACPI PM timer.
///
/// The same arithmetic and the same uncalibrated fallback `wcg::cycles_to_us` uses — restated here
/// rather than called, because `wcg` is `witness`-gated and this ledger deliberately is not (the
/// metal image is built without `witness`). Two consumers of an unknown TSC rate in this kernel, one
/// guess: 1.25 GHz, which is what `arch::HW_WAIT_BUDGET` already assumes.
///
/// PI-DESK — `now_cycles()` is arch-neutral but its RATE is not, and this is the one place in the
/// family that has to know it. On x86 it is the calibrated TSC (`apic::tsc_hz`, `0` until
/// `apic::calibrate` has run, hence the guess). On aarch64 `now_cycles()` is CNTVCT_EL0, whose rate
/// is CNTFRQ_EL0 — 54 MHz on the BCM2711, ~62.5 MHz under QEMU — and it is EXACT and available from
/// the first instruction, so the fallback arm is dead there rather than merely unlikely. Reading the
/// register (via `timer::cntfrq`, the one accessor the arch already publishes) instead of assuming
/// 1.25 GHz is the difference between a `[dock] paint=` in microseconds and one 23x too small.
#[inline]
pub fn cycles_to_us(dt: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    let hz = crate::arch::apic::tsc_hz();
    #[cfg(target_arch = "aarch64")]
    let hz = crate::arch::timer::cntfrq();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    dt.saturating_mul(1_000_000) / hz
}

// ---------------------------------------------------------------------------
// The registry — occlusion citizenship
// ---------------------------------------------------------------------------

/// How many furniture strips the panel can carry at once.
///
/// **This is the number `wm::OCC_MAX` reserves capacity for**, and `wm` asserts the two agree at
/// compile time. Raising it here without widening the clip is a BUILD failure rather than an
/// occluder silently dropped on a non-witness image — which is the failure mode the WCK4 review named
/// and the reason the registry exists at all instead of a hand-written arm per strip.
pub const STRIP_MAX: usize = 2;

/// A registered furniture strip.
///
/// One function pointer, and it is the tenant's OWN single geometry accessor — never a second copy of
/// the strip's arithmetic living in `wm`. `rect(pw, ph)` answers `None` for a tenant that is absent
/// on this panel (too short, nothing to show) or absent by configuration (disabled), and `None` is
/// the whole of "costs nothing": no box in the clip, no capacity consumed, no pixel owned.
pub struct Tenant {
    /// The name that appears on this strip's ledger and witness lines.
    pub name: &'static str,
    /// Which edge it anchors to. Carried for the witness rather than the geometry — the tenant has
    /// already applied it — so a capture says WHERE a strip claimed to be without re-deriving it.
    pub edge: Edge,
    /// The tenant's rect on a `pw` x `ph` panel right now, or `None` when absent.
    pub rect: fn(usize, usize) -> Option<Rect>,
}

/// The dock's slot in [`TENANTS`] and in [`rects`]'s output.
pub const DOCK_SLOT: usize = 0;

/// The menu bar's slot in [`TENANTS`] and in [`rects`]'s output.
///
/// Named rather than written as `1` at each use: a witness asserting membership must index the slot
/// its tenant actually occupies, and a registry reordered without moving the index is a fixture that
/// silently starts asserting about the wrong strip.
pub const MENUBAR_SLOT: usize = 1;

/// **The registry.** Order is the composite order: earlier tenants are painted first, so a later one
/// wins an overlap. Today they do not overlap (opposite edges), and `frame_flush`'s `reserve`
/// argument is what keeps that true as tenants are added.
pub static TENANTS: [Tenant; STRIP_MAX] = [
    Tenant {
        name: "dock",
        edge: Edge::Bottom,
        rect: super::dock::strip_rect,
    },
    Tenant {
        name: "menubar",
        edge: Edge::Top,
        rect: super::menubar::strip_rect,
    },
];

/// Every PRESENT strip's rect on this panel, in registry order, written into `out`.
///
/// Returns how many were present. **The one call `wm::erase_clip` makes** — it replaces the
/// hand-written dock arm that used to be the only furniture the erase knew about, and it is why a
/// second strip is a registry entry rather than a second special case in the occlusion machinery.
///
/// An absent tenant contributes nothing and is not counted, so the runtime occupancy of the clip is
/// the number of strips actually on the glass, not `STRIP_MAX`.
pub fn rects(pw: usize, ph: usize, out: &mut [Option<Rect>; STRIP_MAX]) -> usize {
    let mut n = 0;
    for (k, t) in TENANTS.iter().enumerate() {
        let r = (t.rect)(pw, ph);
        out[k] = r;
        if r.is_some() {
            n += 1;
        }
    }
    n
}

/// The composite seam: every tenant's `compose`, in registry order.
///
/// Returns `true` iff ANY strip painted, which is the caller's obligation to upgrade the pass's
/// cursor tail to `Repaint` — each tenant has already taken the arrow off the panel. The `|` is
/// deliberately not `||`: both tenants must run, because a short-circuit would let the first
/// repainting strip starve the second's damage test for the whole pass.
pub fn compose_all() -> bool {
    let a = super::dock::compose();
    let b = super::menubar::compose();
    // CRYSTAL — the SHARD menu's dropdown is a TRANSIENT surface, not a registered strip tenant (it
    // takes no occlusion slot), so it is not in `TENANTS`/`rects`; but its per-pass compose belongs
    // exactly here, at the furniture tail, so it composites on top of the windows and beside the bar
    // it hangs from. Cheap when closed: two relaxed atomics and a return. The `|` is still not `||`
    // for the same reason — every furniture surface must get its damage test in every pass.
    let c = super::crystal::compose();
    // WINMENU (R21) — the FOCUSED WINDOW's dropdown, on the crystal's exact terms and for the same
    // reason, painted AFTER it so a window menu is the topmost surface on the panel while it is down.
    // Cheap when closed: two relaxed atomics and a return. The `|` is still not `||`.
    let d = super::winmenu::compose();
    // TEARSCOPE — the bar's census speaks from here, at the furniture tail, AFTER every tenant has
    // had its damage test. Rate- and delta-gated (see `bar_rollup_one`), so a pass that changed
    // nothing costs five relaxed loads and five compares.
    bar_rollup_tick();
    a | b | c | d
}

/// **The press seam: every furniture surface's press arm, in COMPOSITE-INVERSE order.** The twin of
/// [`compose_all`], and the one place the furniture layer's routing rule lives.
///
/// Returns `true` iff the press was CONSUMED by furniture, in which case the caller must drop the
/// matching RELEASE (store its DROP sentinel) and return without consulting the window table. `false`
/// means no furniture claimed the point and the window arms get their say.
///
/// # Why this function exists at all (PI-DESK, and the extraction it chose)
///
/// The Pi has a live mouse, so the aarch64 router owed the same two arms x86's
/// `wc_click_route_at` already carried. Two options: copy the arms, or extract them. Copying would
/// have put the ORDERING RULE — the whole content of this seam — in two files that are edited by two
/// different lanes on two different schedules, free to drift, and drifting SILENTLY (the symptom of a
/// stale order is a press landing on the wrong layer, which no gate asserts). So the core is
/// extracted here, arch-neutral, beside `compose_all` — because the order below is not a routing
/// preference, it is the INVERSE of the paint order that function fixes, and the two belong within
/// one screen of each other or they will disagree.
///
/// Both arch routers now call this and neither owns a copy. What stays per-arch is exactly what is
/// per-arch: the edge detection, the press-target latch, and the input rings.
///
/// # The order, and why neither arm can starve the other
///
///  1. **CRYSTAL first**, ahead of the dock and every window arm. An OPEN dropdown is a modal surface
///     composited at the pass tail, on top of everything, so its press must be judged before any
///     layer beneath it. CLOSED, the only points it claims lie in the bar's upper-left corner cell
///     (FITTS-CORNER, `menubar::crystal_corner_abs`) — pixels the bar owns anyway — and it declines
///     every other point, so nothing below it is starved.
///  2. **DOCK second**, still ahead of every window arm, because the dock is composited on top of
///     them: `wm::hit_test` knows nothing of the strip, so a window lying under the dock would
///     otherwise take a press the operator can see landed on a tile. The dock declines every point
///     outside its own strip (`Layout::contains`, the SAME accessor its painter draws from — corners
///     included, which is why a corner hit-tests as desktop), and the strip is auto-sized to its tiles
///     and drawn only when there is at least one, so a bare desktop has no dock to swallow anything.
///
/// There is no point at which two arms both answer "mine". That is a property of the accessors, not a
/// tie-break policy: each arm asks the same rect its own painter drew.
///
/// # The click grammar is NOT relaxed here
///
/// A furniture press is an instruction to the WINDOW SYSTEM, never app input — the same law the close
/// and chrome arms follow — so it is consumed and its release is dropped rather than delivered into
/// whatever holds focus after the raise. A dock press SELECTS (raises, un-hides, hands over the
/// keyboard) and acknowledges on the wire; it does not stop, start or kill anything. Nothing in this
/// seam touches a running program's execution.
///
/// # WINMENU (R21) — a THIRD arm, and it goes FIRST
///
/// The window-menu dropdown is the same kind of modal surface the SHARD menu is, composited after it,
/// so by the rule above it must be judged before it. But the ordering is load-bearing for a second,
/// sharper reason: `wm::MENU_OCC_MAX` reserves capacity for exactly ONE open dropdown. Putting
/// `winmenu` second would let a press land on the crystal's CLOSED corner arm while a window menu was
/// still down — two modal surfaces, one occluder slot. First, it consumes every press while its menu
/// is open, so that cannot happen; and its own CLOSED arm declines every point while the SHARD menu
/// is open, so the crystal keeps its dismiss-outside press. The invariant is therefore a property of
/// this order plus those two declines, and it is what `menubar::open_dropdown_rect` reads.
#[inline]
pub fn press_route(x: i32, y: i32) -> bool {
    super::winmenu::press_at(x, y) || super::crystal::press_at(x, y) || super::dock::press_at(x, y)
}

/// **The KEY seam: every furniture surface's `<Esc>` arm.** The twin of [`press_route`], extracted for
/// its reason.
///
/// Both arch routers asked `crystal::key_escape` by name, ahead of the focus ring, because a modal
/// surface must get Escape before the focus ring can TAB the desktop out from under it. R21 gives the
/// panel a SECOND modal surface, and a second name at each of two call sites in two files edited by
/// two lanes is the drift `press_route`'s own header was written about. So the question moves here and
/// the routers ask one thing.
///
/// Consumes ONLY a bare `Esc` while one of the two menus is open; every other event, and `Esc` with
/// nothing down, falls straight through, so a boot that never opens a menu is byte-alike in behaviour.
#[inline]
pub fn key_escape(ev: crate::pal::Event) -> bool {
    super::crystal::key_escape(ev) || super::winmenu::key_escape(ev)
}

// ---------------------------------------------------------------------------
// Compile-time sanity
// ---------------------------------------------------------------------------

const _: () = {
    // A registry with no room for the tenants declared above is a registry that silently drops one.
    assert!(STRIP_MAX >= 1);
    // Every named slot must be inside the registry, or a witness indexes past its own tenant table.
    assert!(DOCK_SLOT < STRIP_MAX);
    assert!(MENUBAR_SLOT < STRIP_MAX);
    assert!(DOCK_SLOT != MENUBAR_SLOT);
    // The scratch must hold the widest strip any constructor can hand the painter. `frame_flush`
    // returns the panel's full width, so this is the panel bound the painter declines above.
    assert!(MAX_STRIP_W >= 2048);
    // A margin of zero would make `frame_centred` and `frame_flush` the same function.
    assert!(PAD > 0);
};

// ---------------------------------------------------------------------------
// TEARSCOPE — the BAR's own census
// ---------------------------------------------------------------------------
//
// # Why this exists, and what the render9 capture proved was missing
//
// Peter, at the bench on the render9 flight, 2026-09-07: *"there was major tearing on the ends of the
// taskbar when i closed pulse that went away when i reopened"*. The capture
// (`~/unaos-bench/capture/line-acm0/orin.log`, boot at lines 106765..126738) carries 530
// `scope=window` rollups, 363 `scope=live`, 5 `scope=desktop` and ONE `scope=fills`. **There is no
// `bar` scope and there never was.** The only `torn=` counter on the wire for that boot belonged to
// `[wc-h] rollup … scope=window`, which measures a WINDOW's staged present: torn=0 across the
// close (emits 22/23), torn=1 at emits 30-36 ~20 s later. `[wc-k] rollup scope=fills`, which does
// carry a `torn=` over the DESKTOP-erase path, is a one-shot at its fourth sample: it printed at
// line 108593 and the close happened at 111570, ~3000 lines later, with nothing to re-read it.
//
// So the bar had no instrument at all, and this is it.
//
// # What owns the bar's pixels, and why the metrics are the ones they are
//
// [`paint`] is the single front-buffer writer for every furniture surface — dock, menubar, and the
// two transient dropdowns. It composes each row in cached RAM and `blit`s it straight into the
// SCAN-OUT; there is no back buffer and no compositor present in front of it. That is the
// load-bearing difference from a window, and it is why `torn=` here is a stronger claim than the
// `[wc-h]` one rather than a weaker one: a window's pixels are staged and published, a strip's are
// written under the beam. The definition is `wcg::erase_note`'s, unchanged — a present whose
// wall-clock duration exceeds the time the beam spends on the rows it covered
// (`rectscan_us = FRAME_US * h / panel_h`) could have been overtaken.
//
// **`torn=` is nevertheless not the metric that scores Peter's episode, and saying so is the point.**
// A bar is short. The dock on the bench panel is ~64 rows of 1200, so `rectscan_us` is ~889 µs and
// the measured `paint=124us` on the render9 `[dock] live` lines sits comfortably under it. `torn=` on
// a healthy bar reads 0 STRUCTURALLY, and an instrument whose headline number cannot move is the
// defect this arc was sent to remove. It is kept because it is honest, cheap, and the one thing that
// would catch a genuinely slow bar paint — but the census below carries the fields that actually
// distinguish *"repainted correctly"* from *"left stale"* over the span a shrinking strip uncovers.
//
// # The uncovered span, which is what "the ends of the taskbar" names
//
// The dock is `frame_centred(Edge::Bottom, …)` (`super::dock::Layout::for_panel`, via
// [`frame_centred`]) and is AUTO-SIZED TO ITS TILES. Close a window and the tile count changes, so
// the strip narrows — and because it is CENTRED, the pixels it stops owning are at BOTH ENDS.
// `dock`'s own comment names the symptom, written before any instrument could see it: *"a strip that
// shrinks … would leave its old ends standing on the panel until something else happened to paint
// over them"*. [`erase_rect`] is the only thing that repaints them, and it can DECLINE (a contended
// [`SCRATCH`], a surface that is not ready).
//
// [`vacate`] is [`erase_rect`] with that question asked out loud. It computes the UNCOVERED area —
// the part of the old rect the new rect does not cover — so a strip that GROWS (old inside new, the
// common case through a boot) correctly owes nothing, and only a shrink or a move can move the
// counters that matter. `owed=` records whether the CALLER keeps the debt when the erase declines: a
// site that re-publishes its slot regardless has FORGOTTEN the span, and no later pass is coming
// back for it.
//
// # What is deliberately NOT claimed
//
// `flat=` counts an uncovered span that WAS erased, successfully, to flat `wm::DESKTOP_BG` — which is
// the only colour [`erase_rect`] paints — with no backdrop repaint requested.
//
// # CURSORBG — the render11 capture answered the question this paragraph left open
//
// The sentence above used to end: *"whether it is what Peter saw is a question for the next capture,
// not for this comment. `super::crystal` and `super::winmenu` already call their own
// `repaint_vacated` after an erase and are the precedent; `dock` and `menubar` do not."*
//
// The next capture came (`boot-render11-B-full.log`, 2026-09-08) and it reads
// `[strip] rollup tenant=dock scope=bar … scene=yes … uncovered=6 uncovered_px=33696 unerased=0
// forgotten=0 flat=6 flat_px=33696 -> FLAT-VACATE`: six shrinks, every erase successful, 33 696
// panel pixels painted flat desktop colour with nothing told. Peter, the same boot: *"tearing and
// background drawing issue"*. So the two named tenants got the `repaint_vacated` the other two
// already had — [`restore_vacated`], called from [`vacate`] — and `flat=` became the counter for a
// span that could NOT be handed back rather than for one nobody tried to hand back.
//
// **This module still only counts on the paths it counted before.** [`vacate`] returns exactly what
// [`erase_rect`] returned, and every caller uses it exactly as it used the call it replaced; what is
// new is the damage propagation the erase always owed and never made.

/// One 60 Hz frame, in microseconds — the constant `[wc-h]` and `[wc-k]` both print as `frame_us=`.
///
/// Duplicated rather than imported: `super::wcg` is `#[cfg(feature = "witness")]` and this module is
/// not, and the bar's census has to reach the METAL image — which is built WITHOUT `witness` — or it
/// is not a claim about the panel Peter watches. `wcg::FRAME_US` is the original; the two must stay
/// equal, and this is the only copy.
const FRAME_US: u64 = 16_667;

/// Decline reason: the shared [`SCRATCH`] was contended, so this pass painted nothing.
const DECL_LOCK: usize = 0;
/// Decline reason: the surface is not ready.
const DECL_READY: usize = 1;
/// Decline reason: the surface is not the 4-byte word layout the row-run path covers.
const DECL_WORD: usize = 2;
/// Decline reason: a zero, oversized, or off-panel rect.
const DECL_GEOM: usize = 3;

/// How many decline reasons [`paint`] has. One per arm, and the array is indexed by the constants
/// above, so an arm added without a constant fails the BUILD rather than landing in the wrong bucket.
const DECL_KINDS: usize = 4;

/// How many tenants the census tracks: the two registered strips, the two transient dropdowns that
/// also paint through [`paint`], and a catch-all.
const BAR_SLOTS: usize = 5;

/// The census slot names, in slot order. Slot [`BAR_OTHER`] catches a name this table does not know,
/// so a furniture surface added without touching this array is REPORTED under `tenant=other` rather
/// than silently dropped.
const BAR_NAMES: [&str; BAR_SLOTS] = ["dock", "menubar", "crystal", "winmenu", "other"];

/// The catch-all slot. See [`BAR_NAMES`].
const BAR_OTHER: usize = 4;

/// How often a tenant's `scope=bar` rollup may speak. The [`Ledger`]'s own cadence, so a capture
/// carries the census and the cost ledger for the same strip side by side and at the same rate.
const BAR_ROLLUP_US: u64 = ROLLUP_PERIOD_US;

/// One-shot latch: the STALE-ENDS evidence line has been printed for this tenant.
///
/// The rollup is rate-limited to [`BAR_ROLLUP_US`] and Peter's episode is instantaneous, so the COUNT
/// lives on the rollup and the EVIDENCE gets its own line at the moment it happens. That is
/// `[wc-k] rollup scope=starve`'s precedent, adopted for its reason: a verdict is only worth having
/// if the boot can still trip it after the rollup has spoken.
const SAID_STALE: u64 = 1;
/// One-shot latch for the retryable half of the same fault.
const SAID_UNERASED: u64 = 2;
/// One-shot latch for a bar present that outran the beam.
const SAID_TORN: u64 = 4;
/// CURSORBG — one-shot latch for the first uncovered span this tenant handed back to its owners.
/// The positive twin of [`SAID_STALE`], and it is on the wire for the same reason: the rollup is
/// rate-limited, so the moment the mechanism first runs gets its own line.
const SAID_RESTORE: u64 = 8;

/// Per-tenant bar census.
///
/// Every field is a MONOTONE TOTAL, incremented once per event at record time; nothing here is a
/// delta and printing does not reset it. A rollup line is therefore a snapshot, and the reader's rule
/// is `[wc-h]`'s: take the GREATEST `emit=` for each `tenant=`, never sum lines.
struct BarCensus {
    paints: AtomicU64,
    paint_px: AtomicU64,
    maxpaint_us: AtomicU64,
    minpaint_us: AtomicU64,
    scan_us: AtomicU64,
    torn: AtomicU64,
    declines: AtomicU64,
    decl: [AtomicU64; DECL_KINDS],
    vacates: AtomicU64,
    uncovered: AtomicU64,
    uncovered_px: AtomicU64,
    unerased: AtomicU64,
    unerased_px: AtomicU64,
    forgotten: AtomicU64,
    flat: AtomicU64,
    flat_px: AtomicU64,
    /// CURSORBG — uncovered spans handed back to their owners (window damage + a desktop present
    /// request) rather than left standing as flat desktop colour. `restored + flat == uncovered`
    /// over every span whose erase succeeded.
    restored: AtomicU64,
    restored_px: AtomicU64,
    /// BEAM (orin 26) — paints with a beam observation, microseconds the hold spun for them, and
    /// the exposure census `wcg` keeps for windows (see `H_BEAMCROSS`), here for the bars.
    beamobs: AtomicU64,
    beamwait_us: AtomicU64,
    beamcross_ppk: AtomicU64,
    rect: AtomicU64,
    emit: AtomicU64,
    t0: AtomicU64,
    last: AtomicU64,
    lastcensus: AtomicU64,
    said: AtomicU64,
}

impl BarCensus {
    const fn new() -> BarCensus {
        BarCensus {
            paints: AtomicU64::new(0),
            paint_px: AtomicU64::new(0),
            maxpaint_us: AtomicU64::new(0),
            // `u64::MAX` is the "no sample yet" sentinel, printed as `0`. A real present cannot reach
            // it, so the sentinel is unambiguous — `wcg::H_MINPRES`'s convention.
            minpaint_us: AtomicU64::new(u64::MAX),
            scan_us: AtomicU64::new(0),
            torn: AtomicU64::new(0),
            declines: AtomicU64::new(0),
            decl: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            vacates: AtomicU64::new(0),
            uncovered: AtomicU64::new(0),
            uncovered_px: AtomicU64::new(0),
            unerased: AtomicU64::new(0),
            unerased_px: AtomicU64::new(0),
            forgotten: AtomicU64::new(0),
            flat: AtomicU64::new(0),
            flat_px: AtomicU64::new(0),
            restored: AtomicU64::new(0),
            restored_px: AtomicU64::new(0),
            beamobs: AtomicU64::new(0),
            beamwait_us: AtomicU64::new(0),
            beamcross_ppk: AtomicU64::new(0),
            rect: AtomicU64::new(0),
            emit: AtomicU64::new(0),
            t0: AtomicU64::new(0),
            last: AtomicU64::new(0),
            lastcensus: AtomicU64::new(0),
            said: AtomicU64::new(0),
        }
    }

    /// Every event this tenant has had. The quantity the rollup's delta gate is taken over — "nothing
    /// new to say, say nothing", which is what keeps an idle desktop silent.
    fn total(&self) -> u64 {
        self.paints
            .load(Ordering::Relaxed)
            .wrapping_add(self.declines.load(Ordering::Relaxed))
            .wrapping_add(self.vacates.load(Ordering::Relaxed))
    }
}

/// One census per tenant slot. `static` rather than per-tenant declarations for the reason
/// [`TENANTS`] is a registry: a tenant added without a census would be invisible, and the catch-all
/// slot makes that impossible.
static BARS: [BarCensus; BAR_SLOTS] = [
    BarCensus::new(),
    BarCensus::new(),
    BarCensus::new(),
    BarCensus::new(),
    BarCensus::new(),
];

/// Which census slot a tenant name occupies.
///
/// Four short byte-slice comparisons on a `&'static str` literal, on a path that runs only when a
/// strip actually paints, declines or vacates — never per pixel, never per row, and never on the pass
/// that finds its signature unchanged.
fn bar_slot(name: &str) -> usize {
    let mut k = 0;
    while k < BAR_OTHER {
        if BAR_NAMES[k].as_bytes() == name.as_bytes() {
            return k;
        }
        k += 1;
    }
    BAR_OTHER
}

/// Note that this tenant has been seen, so `age_ms=` has an origin.
///
/// `compare_exchange` from `0`, never a plain store: the origin must be the FIRST record, and a later
/// store would silently reset the age and make every subsequent rollup read like a startup burst.
#[inline]
fn bar_seen(c: &BarCensus) {
    if c.t0.load(Ordering::Relaxed) == 0 {
        let _ =
            c.t0.compare_exchange(0, crate::arch::now_cycles(), Ordering::Relaxed, Ordering::Relaxed);
    }
}

/// TEARSCOPE — record a [`paint`] that declined, and return the `false` the caller was already
/// returning.
///
/// **The return value is the whole contract.** This function exists so a decline can be counted
/// without an `if` appearing at four arms of [`paint`], and it changes nothing about what [`paint`]
/// does: every arm that called it returned `false` before this arc and returns `false` now.
fn bar_decline(name: &str, reason: usize) -> bool {
    let c = &BARS[bar_slot(name)];
    bar_seen(c);
    c.declines.fetch_add(1, Ordering::Relaxed);
    c.decl[reason].fetch_add(1, Ordering::Relaxed);
    false
}

/// TEARSCOPE — record a completed [`paint`]: what it covered, what the present cost, and whether that
/// present outran the beam over the rows it wrote.
fn bar_painted(name: &str, r: Rect, cyc: u64, panel_h: usize) {
    let (_, _, w, h) = r;
    let c = &BARS[bar_slot(name)];
    bar_seen(c);
    let present_us = cycles_to_us(cyc);
    // `wcg::erase_note`'s arithmetic, and its deliberate bias toward NOT reporting a tear: `FRAME_US`
    // includes blanking the beam does not spend on visible rows, so the real scan time of these rows
    // is SHORTER than the figure the present is compared against, and a `torn=0` near the threshold
    // is not a proof of safety.
    let rectscan_us = if panel_h == 0 { 0 } else { FRAME_US * h as u64 / panel_h as u64 };
    c.paints.fetch_add(1, Ordering::Relaxed);
    c.paint_px.fetch_add((w as u64) * (h as u64), Ordering::Relaxed);
    c.maxpaint_us.fetch_max(present_us, Ordering::Relaxed);
    c.minpaint_us.fetch_min(present_us, Ordering::Relaxed);
    c.scan_us.store(rectscan_us, Ordering::Relaxed);
    c.rect.store(pack_rect(Some(r)), Ordering::Relaxed);
    // BEAM (orin 26) — the exposure on every paint, and the observation `paint`'s bracket
    // recorded, which decides `torn=` where it exists; the duration predicate only where it does
    // not. The same split `wcg::stage_note` makes, for the same reason (TEAR-DIAG).
    c.beamcross_ppk.fetch_add(super::beam::exposure_ppk(present_us, rectscan_us), Ordering::Relaxed);
    let obs = super::beam::take_last();
    let torn = match obs {
        Some(o) => {
            c.beamobs.fetch_add(1, Ordering::Relaxed);
            c.beamwait_us.fetch_add(o.waited_us as u64, Ordering::Relaxed);
            o.torn
        }
        None => present_us > rectscan_us,
    };
    if torn {
        let n = c.torn.fetch_add(1, Ordering::Relaxed) + 1;
        if c.said.fetch_or(SAID_TORN, Ordering::Relaxed) & SAID_TORN == 0 {
            serial_println!(
                "[strip] paint tenant={} box={}x{} present_us={} rectscan_us={} beam={} torn={} -> AT-RISK",
                name,
                w,
                h,
                present_us,
                rectscan_us,
                match obs { Some(o) => BeamFmt(Some((o.vs, o.ve, o.vt))), None => BeamFmt(None) },
                n
            );
        }
    }
}

/// BEAM — `beam=` for the strip's lines: `vs..ve/vt` observed, `blind` otherwise.
struct BeamFmt(Option<(u32, u32, u32)>);

impl core::fmt::Display for BeamFmt {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some((vs, ve, vt)) => write!(f, "{}..{}/{}", vs, ve, vt),
            None => f.write_str("blind"),
        }
    }
}

/// The area of `old` that `new` does not cover — the span a shrinking or moving strip UNCOVERS, in
/// pixels. `0` for a strip that grew over its own old rect, which owes nothing.
///
/// Both rects are axis-aligned, so the uncovered area is exactly `area(old) - area(old ∩ new)`. A
/// `new` of `None` is a strip going away entirely, which uncovers all of `old`.
fn bar_uncovered(old: Rect, new: Option<Rect>) -> u64 {
    let (ox, oy, ow, oh) = old;
    let area = (ow as u64) * (oh as u64);
    let Some((nx, ny, nw, nh)) = new else {
        return area;
    };
    let x0 = ox.max(nx);
    let y0 = oy.max(ny);
    let x1 = (ox + ow).min(nx + nw);
    let y1 = (oy + oh).min(ny + nh);
    if x1 <= x0 || y1 <= y0 {
        return area;
    }
    area - ((x1 - x0) as u64) * ((y1 - y0) as u64)
}

/// CURSORBG — **hand a just-vacated span back to its OWNERS.** The pair every other `DESKTOP_BG`
/// writer in this subsystem already makes, brought to the two strips that did not make it.
///
/// # What [`erase_rect`] cannot do, and why nobody noticed
///
/// `erase_rect` paints flat [`wm::DESKTOP_BG`] and returns. That is the right colour for the pixels a
/// strip vacated **only if nothing was under the strip**, and on this arch something routinely is:
/// `wm::occ_clip` is structurally `OccClip::none` on aarch64 (PARITY §6.2), so a window blit paints
/// straight over the dock and a window may legitimately lie under the strip's ends. Painting those
/// ends flat stamps a HOLE over that window, and no later pass is coming for it — `damage_intersecting`
/// was never called, so the compositor does not know the row is dirty, and no desktop present was
/// requested, so the backdrop layer does not know either.
///
/// This is not a new mechanism; it is the one three other sites already use, written down once:
///
/// ```text
///   wm.rs        drain_deferred      stage_fill(DESKTOP_BG) -> damage_intersecting + request_present_rect
///   crystal.rs   repaint_vacated     erase_rect             -> damage_intersecting + request_full_present
///   winmenu.rs   repaint_vacated     erase_rect             -> damage_intersecting + request_full_present
/// ```
///
/// `crystal`'s own header states the argument verbatim for its dropdown: *"`strip::erase_rect`'s
/// `DESKTOP_BG` does not restore those windows or the desktop content beneath — it stamps a HOLE over
/// them"*. A dock that shrinks uncovers its ends for exactly the same reason a dismissed dropdown
/// uncovers its box, and this module's own census note named `dock` and `menubar` as the two tenants
/// that had no such call.
///
/// # Rect, not flag
///
/// `request_present_rect` is DRAG-PI M1's narrow twin of `request_full_present`, and `drain_deferred`
/// is the precedent that chose it: the desktop content that needs restoring is the content the erase
/// just covered, which is this box and nothing outside it. The queue MERGES on overflow rather than
/// dropping, so the desktop layer is always asked for a superset of the vacated span and never for
/// less — no span can be lost to queue pressure.
///
/// Returns whether the handback was made. `false` only for a degenerate rect, and it is what keeps
/// `flat=` a live counter rather than a field this arc retired: a span erased with no handback behind
/// it is still reportable, and the rollup still fails it.
fn restore_vacated(r: Rect) -> bool {
    let (x, y, w, h) = r;
    if w == 0 || h == 0 {
        return false;
    }
    // Order is `drain_deferred`'s: the window layer first, then the backdrop. Neither call blocks on
    // a lock this module holds — `SCRATCH` was released by `erase_rect` before it returned, and the
    // two locks these take (`wm::TABLE`, `screen::PRESENT_RECTS`) are the same pair the compositor's
    // own drain takes from inside a masked present.
    wm::damage_intersecting(x, y, w, h);
    super::screen::request_present_rect(x, y, w, h);
    true
}

/// **Erase a rect a strip has VACATED, and say what became of the span it uncovered.**
///
/// A thin accounting wrapper over [`erase_rect`]: it calls it, returns exactly what it returned, and
/// counts. `new` is the rect this tenant is about to own (`None` when the strip is going away
/// entirely), and only the part of `old` that `new` does not cover can be left stale.
///
/// `owed` is the CALLER's debt policy, and it is what separates a latency cost from a defect:
///
///  * `owed = true` — the caller keeps its [`Slot`] until the erase has actually painted, so a
///    decline stays visible and the next pass retries. `super::crystal`'s dismiss arm is the
///    precedent, and its own comment is the argument: clearing the slot first *"turned that decline
///    into a silent loss"*.
///  * `owed = false` — the caller re-publishes its slot regardless of what this returned, so a
///    decline here is a span nothing is coming back for. That is `forgotten=`, and it is the field
///    Peter's episode is scored on.
///
/// **This function does not retry, defer, or hold anything.** Making a declined vacate arrive is a
/// change to the damage discipline and belongs to a different arc; this one measures.
pub fn vacate(name: &str, old: Rect, new: Option<Rect>, owed: bool) -> bool {
    let c = &BARS[bar_slot(name)];
    bar_seen(c);
    c.vacates.fetch_add(1, Ordering::Relaxed);
    let px = bar_uncovered(old, new);
    let erased = erase_rect(old);
    if px == 0 {
        // A grow: the new rect covers every pixel the old one held, so nothing is uncovered and a
        // declined erase costs the panel nothing. Counted as a vacate and excluded from every risk
        // field — which is also why a boot's dock, which grows tile by tile, cannot spend the
        // evidence latches before the operator's first close.
        return erased;
    }
    c.uncovered.fetch_add(1, Ordering::Relaxed);
    c.uncovered_px.fetch_add(px, Ordering::Relaxed);
    let (ox, oy, ow, oh) = old;
    if !erased {
        c.unerased.fetch_add(1, Ordering::Relaxed);
        c.unerased_px.fetch_add(px, Ordering::Relaxed);
        if !owed {
            let n = c.forgotten.fetch_add(1, Ordering::Relaxed) + 1;
            if c.said.fetch_or(SAID_STALE, Ordering::Relaxed) & SAID_STALE == 0 {
                serial_println!(
                    "[strip] vacate tenant={} box={}x{}+{}+{} uncovered_px={} erased=no owed=no forgotten={} -> STALE-ENDS",
                    name, ow, oh, ox, oy, px, n
                );
            }
        } else if c.said.fetch_or(SAID_UNERASED, Ordering::Relaxed) & SAID_UNERASED == 0 {
            serial_println!(
                "[strip] vacate tenant={} box={}x{}+{}+{} uncovered_px={} erased=no owed=yes -> UNERASED",
                name, ow, oh, ox, oy, px
            );
        }
        return erased;
    }
    // Erased — to flat `wm::DESKTOP_BG`, which is the only colour [`erase_rect`] paints.
    //
    // CURSORBG — **and now HANDED BACK, which is what render10's census asked for and render11
    // convicted.** `flat=` used to be the end of this path: the span was painted flat and nothing was
    // told, so `[strip] rollup tenant=dock … flat_px=33696 -> FLAT-VACATE` on the render11 boot
    // described 33 696 panel pixels of desktop colour standing where the layers underneath belong.
    // [`restore_vacated`] is the pair every other `DESKTOP_BG` writer in this subsystem already makes
    // (`wm::drain_deferred`, `crystal::repaint_vacated`, `winmenu::repaint_vacated`); the dock and the
    // menu bar were the two that did not, which is exactly what this module's own census note said.
    //
    // The counters split on the ANSWER rather than on the board: `restored` is a span handed back,
    // `flat` is a span left flat. So `flat=0` is the fixed state on every board, and `flat > 0` means
    // a handback was asked for and could not be made — which is a defect the rollup must still be
    // able to report, not a case this arc made unreachable.
    if restore_vacated(old) {
        c.restored.fetch_add(1, Ordering::Relaxed);
        c.restored_px.fetch_add(px, Ordering::Relaxed);
        if c.said.fetch_or(SAID_RESTORE, Ordering::Relaxed) & SAID_RESTORE == 0 {
            serial_println!(
                "[strip] vacate tenant={} box={}x{}+{}+{} uncovered_px={} erased=yes src={} -> SCENE-RESTORE",
                name, ow, oh, ox, oy, px,
                if super::desktop_scene_owns_backdrop() { "scene" } else { "flat" }
            );
        }
    } else {
        c.flat.fetch_add(1, Ordering::Relaxed);
        c.flat_px.fetch_add(px, Ordering::Relaxed);
    }
    erased
}

/// TEARSCOPE — emit every tenant's `scope=bar` rollup that owes one.
///
/// ### Reachability, and how it is guaranteed rather than assumed
///
/// [`compose_all`] is the sole call site, and it has itself exactly one caller —
/// `wm::composite_pass_half`, the furniture seam, the pass's last layer under the sprite. So this
/// runs on exactly the population it reports on: a strip that is composing is a strip whose census is
/// being read. It needs no timer, no thread and no new call site.
///
/// The corollary matters as much: a board that never composites never prints a line, and on the Orin
/// the render task does not drive `wm::composite` itself. That it nevertheless runs there is not
/// assumed — the render9 capture carries 183 `[dock] live` and 184 `[menubar] live` lines in one
/// boot, and those come from [`Ledger::tick`] on this same seam.
fn bar_rollup_tick() {
    let mut k = 0;
    while k < BAR_SLOTS {
        bar_rollup_one(k);
        k += 1;
    }
}

/// One tenant's `scope=bar` line, if it owes one.
fn bar_rollup_one(k: usize) {
    let c = &BARS[k];
    // Delta gate first — nothing new to say, say nothing. One relaxed load and a compare on the
    // common case, which is an idle desktop arriving here at composite rate. A tenant that has never
    // been seen is silent for the whole boot, which is how an absent strip stays free.
    let total = c.total();
    if total == 0 || total == c.lastcensus.load(Ordering::Relaxed) {
        return;
    }
    // Rate gate, on the free-running counter rather than a pass count, so the cadence is the same
    // whether the panel is idle or busy. `last == 0` is the first line and is not held back.
    let last = c.last.load(Ordering::Relaxed);
    let now = crate::arch::now_cycles();
    if last != 0 && cycles_to_us(now.saturating_sub(last)) < BAR_ROLLUP_US {
        return;
    }
    // Arm: exactly one core proceeds. The loser observed the same `last`, its swap fails, and it
    // returns without printing — which is correct, because the counters are shared and its line would
    // have carried the same census.
    if c.last.compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed).is_err() {
        return;
    }
    c.lastcensus.store(total, Ordering::Relaxed);
    let emit = c.emit.fetch_add(1, Ordering::Relaxed) + 1;
    let t0 = c.t0.load(Ordering::Relaxed);
    let age_ms = if t0 == 0 { 0 } else { cycles_to_us(now.saturating_sub(t0)) / 1000 };
    let (x, y, w, h) = unpack_rect(c.rect.load(Ordering::Relaxed));
    let forgotten = c.forgotten.load(Ordering::Relaxed);
    let unerased = c.unerased.load(Ordering::Relaxed);
    let torn = c.torn.load(Ordering::Relaxed);
    let flat = c.flat.load(Ordering::Relaxed);
    // CURSORBG — the two halves of "what became of every uncovered span whose erase succeeded".
    let restored = c.restored.load(Ordering::Relaxed);
    let uncovered = c.uncovered.load(Ordering::Relaxed);
    let declines = c.declines.load(Ordering::Relaxed);
    // Whether a flat `DESKTOP_BG` slab is VISIBLE at all: on a scene-owned backdrop it stands where
    // scene content belongs, and on a flat one it is the same colour as its surroundings. A field
    // rather than a term folded into the counts, because it is a property of the BOARD and everything
    // else on the line is a property of the bar.
    let scene = super::desktop_scene_owns_backdrop();
    // Precedence, worst and most specific first.
    //
    //  * STALE-ENDS — an uncovered span whose erase declined at a site that then re-published its
    //    slot. Those panel rows hold the departed strip's pixels and NO later pass is coming for
    //    them. It outranks everything below because every other term describes a repaint that
    //    happened — late, or flat, or fast; this one describes a repaint that did not happen and
    //    will not.
    //  * UNERASED — the same decline where the caller kept the debt. Still owed, so still arriving.
    //  * AT-RISK — a bar present outran the beam over its own rows. Below the two above because a
    //    torn frame is replaced by the next one and a stale span is not.
    //  * FLAT-VACATE — every uncovered span WAS erased, to flat desktop colour, on a board whose
    //    backdrop is a SCENE. Reported, never forbidden: it is a colour question, and the capture is
    //    what settles whether it is visible.
    //  * DECLINED — a paint declined. One decline is a fact to report, not a boot to fail.
    let verdict = if forgotten > 0 {
        "STALE-ENDS"
    } else if unerased > 0 {
        "UNERASED"
    } else if torn > 0 {
        "AT-RISK"
    } else if scene && restored < uncovered {
        // CURSORBG — the term MOVED, and the move is the arc. It used to be `flat > 0 && scene`: an
        // uncovered span erased to desktop colour on a scene board, reported and permitted. Now every
        // successful erase either hands its span back (`restored`) or is counted `flat`, and the
        // question the verdict asks is the one that matters — did EVERY uncovered span get an owner?
        // `restored < uncovered` catches a flat leftover, a declined erase, and a forgotten span
        // alike, and it is falsifiable by construction: revert `restore_vacated`'s call and the
        // render11 dock reads `uncovered=6 restored=0 flat=6 -> FLAT-VACATE` again.
        "FLAT-VACATE"
    } else if declines > 0 {
        "DECLINED"
    } else {
        "CLEAN"
    };
    let minp = c.minpaint_us.load(Ordering::Relaxed);
    serial_println!(
        "[strip] rollup tenant={} scope=bar emit={} age_ms={} rect={}x{}+{}+{} scene={} pop=all-paints paints={} paint_px={} torn={} beam={} beamobs={} beamwait_us={} beamcross_ppk={} maxpaint_us={} minpaint_us={} rectscan_us={} declines={} decl_lock={} decl_ready={} decl_word={} decl_geom={} pop=vacates vacates={} uncovered={} uncovered_px={} unerased={} unerased_px={} forgotten={} flat={} flat_px={} restored={} restored_px={} pop=constant frame_us={} -> {}",
        BAR_NAMES[k],
        emit,
        age_ms,
        w,
        h,
        x,
        y,
        if scene { "yes" } else { "no" },
        c.paints.load(Ordering::Relaxed),
        c.paint_px.load(Ordering::Relaxed),
        torn,
        if c.beamobs.load(Ordering::Relaxed) > 0 { "obs" } else { "blind" },
        c.beamobs.load(Ordering::Relaxed),
        c.beamwait_us.load(Ordering::Relaxed),
        c.beamcross_ppk.load(Ordering::Relaxed),
        c.maxpaint_us.load(Ordering::Relaxed),
        if minp == u64::MAX { 0 } else { minp },
        c.scan_us.load(Ordering::Relaxed),
        declines,
        c.decl[DECL_LOCK].load(Ordering::Relaxed),
        c.decl[DECL_READY].load(Ordering::Relaxed),
        c.decl[DECL_WORD].load(Ordering::Relaxed),
        c.decl[DECL_GEOM].load(Ordering::Relaxed),
        c.vacates.load(Ordering::Relaxed),
        c.uncovered.load(Ordering::Relaxed),
        c.uncovered_px.load(Ordering::Relaxed),
        unerased,
        c.unerased_px.load(Ordering::Relaxed),
        forgotten,
        flat,
        c.flat_px.load(Ordering::Relaxed),
        restored,
        c.restored_px.load(Ordering::Relaxed),
        FRAME_US,
        verdict
    );
}

const _: () = {
    // Every decline reason must have a bucket, or a counted decline lands in the wrong one.
    assert!(DECL_LOCK < DECL_KINDS);
    assert!(DECL_READY < DECL_KINDS);
    assert!(DECL_WORD < DECL_KINDS);
    assert!(DECL_GEOM < DECL_KINDS);
    // The catch-all must be the LAST slot: `bar_slot` scans `0..BAR_OTHER` and falls through to it,
    // so a catch-all anywhere else would shadow a real tenant's name.
    assert!(BAR_OTHER == BAR_SLOTS - 1);
    // Both registered tenants must have a census slot ahead of the catch-all, or the two strips this
    // arc exists to watch would report as `tenant=other`.
    assert!(BAR_SLOTS > STRIP_MAX);
};

// =================================================================================================
// STRIPVAC — the fixture for [`restore_vacated`], and what it can and cannot claim
// =================================================================================================
//
// # What it forces
//
// A real strip vacate on the real panel: an old rect, a new one that covers only part of it, and the
// question the arc turns on — did the uncovered span get an OWNER, or was it painted flat and
// forgotten? The census is the oracle, because the census is what the bench reads: `uncovered=1
// restored=1 flat=0` is the fixed state and `uncovered=1 restored=0 flat=1` is the render11 state.
// Revert [`vacate`]'s call to [`restore_vacated`] and this fixture goes RED on its own gate, which is
// the property that makes it a gate rather than a printout.
//
// It runs under the CATCH-ALL census slot (`tenant=other`), never the dock's or the menu bar's, so it
// cannot perturb the two counters an operator reads a boot by — and `bar_slot`'s fall-through is what
// puts it there, so the isolation is a property of the naming rather than of a special case.
//
// # What it deliberately does NOT claim
//
// **It does not prove the handback reached the glass.** `damage_intersecting` marks and
// `request_present_rect` queues; whether a present then runs is the render pump's business and a
// different board's question (`screen::present_owed` and the Orin pump's `dirty` are that half). A
// fixture that asserted panel pixels here would be asserting the QEMU desktop's cadence, not this
// module's contract.
//
// **It does not exercise a CURSOR sweep**, and the reason is structural rather than an omission: on
// the x86 QEMU gate there is no HID pointer, `pal::cursor::visible()` is false for the whole boot,
// `video::cursor` writes zero pixels and `sp.drawn` is never set — so `undraw_locked` returns `None`
// at its first line, `repair` is never handed a rect, and `restore_note` cannot be reached by any
// gesture the gate can make. Synthesising one would mean driving `pal::cursor::set_abs` from a
// fixture, which arms the sprite for the REST of the boot and puts an arrow into every `[wc-c]`
// checksum and `[wc-d]` scan-out verdict downstream — the exact perturbation `cursor`'s own header
// argues the module must never make on the gate. The cursor half's witness is `[cursor] restore
// src=… -> HANDED-BACK` on the metal wire; this fixture gates the strip half, which is the half the
// render11 capture convicted.

/// STRIPVAC — force one vacate with a genuine uncovered span and score the census.
///
/// One-shot, `witness`-gated, driven from [`super::dock::selftest`] (the lane compromise that
/// function already documents for `menubar::selftest`, on the same terms and for the same reason).
#[cfg(feature = "witness")]
pub fn vacate_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() || !fb.word4() {
            serial_println!(":: STRIPVAC: fixture — no word4 surface :: SKIP ::");
            return;
        }
        let i = fb.info();
        (i.width, i.height)
    };
    // The smallest honest gesture: a 16x8 box at the panel's bottom-left shrinking to its left half,
    // so the uncovered span is the right 8x8 = 64 px. Bottom-left is chosen because it is outside
    // every furniture rect on this panel (the dock is CENTRED, the menu bar is the top edge), so the
    // erase cannot land on a live strip's pixels; and the handback this fixture is testing is itself
    // what asks for those 64 px back.
    if pw < 32 || ph < 16 {
        serial_println!(":: STRIPVAC: fixture — panel {}x{} too small :: SKIP ::", pw, ph);
        return;
    }
    let old: Rect = (0, ph - 8, 16, 8);
    let new: Rect = (0, ph - 8, 8, 8);
    let c = &BARS[BAR_OTHER];
    let (u0, r0, f0) = (
        c.uncovered.load(Ordering::Relaxed),
        c.restored.load(Ordering::Relaxed),
        c.flat.load(Ordering::Relaxed),
    );
    let erased = vacate("stripvac", old, Some(new), true);
    // [`erase_rect`] opens the sprite bracket (`cursor::undraw`) and every caller in the compose seam
    // closes it by returning `true` into `compose_all`'s cursor tail. This fixture is not in that
    // seam, so it closes its own: `repaint` draws only while `pal::cursor::visible()`, so it is a
    // no-op on the pointer-less QEMU gate and the correct restore anywhere else.
    super::cursor::repaint();
    let (du, dr, df) = (
        c.uncovered.load(Ordering::Relaxed) - u0,
        c.restored.load(Ordering::Relaxed) - r0,
        c.flat.load(Ordering::Relaxed) - f0,
    );
    // A declined erase is not this fixture's subject and must not be scored as its failure: the span
    // is then still OWED (`owed=true` above), which is a different, already-gated class. Said as SKIP
    // so a contended scratch cannot read as the defect.
    if !erased {
        serial_println!(
            ":: STRIPVAC: erase declined (owed, retried next pass) uncovered={} :: SKIP ::",
            du
        );
        return;
    }
    // The claim, in one line: the span was uncovered, it was handed back, and NOTHING was left flat.
    let pass = du == 1 && dr == 1 && df == 0;
    serial_println!(
        ":: STRIPVAC: box={}x{}+{}+{} -> {}x{}+{}+{} uncovered={} restored={} flat={} owed_px={} scene={} :: {} ::",
        old.2, old.3, old.0, old.1,
        new.2, new.3, new.0, new.1,
        du, dr, df,
        bar_uncovered(old, Some(new)),
        if super::desktop_scene_owns_backdrop() { "yes" } else { "no" },
        if pass { "PASS" } else { "FAIL" }
    );
}
