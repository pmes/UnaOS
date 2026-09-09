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
        return false;
    }
    let fb = *super::WRITER.lock();
    if !fb.is_ready() {
        return false;
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
        return false;
    }
    let info = fb.info();
    if x + w > info.width || y + h > info.height {
        return false;
    }
    let Some(mut s) = SCRATCH.try_lock() else {
        return false; // contended: the next pass repaints (the signature is still unmatched).
    };

    // CURSOR — take the arrow off the panel before the first byte lands. `wm::erase`'s bracket, and
    // for its reason: without it these rows would overwrite the sprite and the save-under would later
    // restore pre-strip pixels over a freshly painted strip. The caller restores it (we return
    // `true`, which upgrades the pass's cursor tail to `Repaint`).
    super::cursor::undraw();

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
    for j in 0..h {
        // SAFETY: as in `paint` — `raw` is a live `[u32; MAX_STRIP_W]`, `w <= MAX_STRIP_W`, and
        // `blit` bounds-checks its destination.
        let bytes = unsafe { core::slice::from_raw_parts(s.raw.as_ptr() as *const u8, w * 4) };
        fb.blit((y + j) * stride_b + x * 4, bytes);
    }
    fb.flush_rect(x, y, w, h);
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
