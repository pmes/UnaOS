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

//! FONT — the kernel's shared anti-aliased text face.
//!
//! ## Why this module exists (GR27, Peter: "ours looks like a lightweight x-windows variant")
//!
//! Every character the kernel put on glass before this arc came from `font8x8` — a 1-bit 8×8
//! bitmap — replicated by an integer scale and blitted as opaque foreground pixels. That is the
//! literal X11 `fixed` recipe: hard staircase edges, one stroke weight, square tracking, no
//! baseline. This module replaces the *face* and the *blit* in one place so every chrome text
//! path (window captions, menu bar, dock labels, the crystal menu, the panel console) picks up
//! the same rendering.
//!
//! ## The face
//!
//! **Noto Sans Mono**, pre-rasterized to 8-bit ALPHA atlases at build time by the
//! `noto-sans-mono-bitmap` crate (regular + bold, Basic Latin, at the rasters the [`Face`] set
//! below resolves to). The atlases are pure `static` rodata — nothing here allocates, so every
//! function is safe in present context, in panic context, and pre-heap.
//!
//! **License**: the crate is MIT (Philipp Schuster); the Noto Sans Mono glyph outlines it
//! rasterizes are SIL Open Font License 1.1 — permissive, embeddable, explicitly not GPL. The
//! OFL's only obligations attach to redistributing the *font software itself*; bitmap renderings
//! embedded in a program carry no reservation.
//!
//! ## The metrics
//!
//! One glyph cell is [`CELL_W`]×[`CELL_H`] pixels (7×16 at this raster — the advance is the
//! mono font's own, so tracking is finally the typeface's and not `8 * scale`). The raster
//! already contains Noto's real side bearings and line-gap padding, which is what gives text
//! drawn with this module a correct baseline and inter-line rhythm with **zero** per-call
//! metric arithmetic: callers place cells on a grid exactly as they placed `font8x8` cells.
//!
//! ## TWO faces, and why the second one is DERIVED (FONT-METRIC, Peter on the bench, PA43)
//!
//! The first metal boot of the merged face on the 1920x1200 bench Pi returned *"fonts were looking
//! good BUT window title font size is small for the size of the title and menu bars"*. That is not
//! a wiring fault — the captions were already the anti-aliased face — it is a METRIC fault: the
//! chrome strip is [`theme::TITLE_HEIGHT`] = 34 px tall and carried a 16 px raster, ~47% of the
//! band, where the platform this kit quotes sets its caption at ~60% of the bar.
//!
//! So this module now carries two atlases and the second one's size is **computed from the bar
//! metric, never written down**:
//!
//! * [`Face::Body`] — 16 px. Terminal-grade text whose size is set by how much of it must fit:
//!   the panel console, and anything else whose surface is a character grid.
//! * [`Face::Chrome`] — [`chrome_raster`]`(theme::TITLE_HEIGHT)`. Window captions, the menu bar
//!   title and clock, the crystal menu's rows, the dock's tile captions — every glyph that lives
//!   inside a piece of furniture whose height the theme decides.
//!
//! Because the chrome raster is a function of `TITLE_HEIGHT`, raising the bar raises the face with
//! it and no call site holds a pixel size of its own: `wm::TITLE_CELL_W`/`_H` is the one metric,
//! `menubar`, `crystal` and `dock` all resolve their `CELL_W`/`CELL_H` to it, and their layout
//! constants (`ITEM_H`, `MENU_W`, `FLOOR_W`, the dock's strip budget) are already expressions over
//! it. Nothing in this arc had to learn a new number; the numbers it already had simply moved.
//!
//! ## The blit
//!
//! Alpha compositing, two forms, chosen by what the caller can afford to read:
//!
//! * [`draw_row`] — one glyph scanline into a cached-RAM `&mut [u32]` row (the strip painters:
//!   menubar, dock, crystal). The destination is readable RAM, so the blend reads `out[i]` and
//!   is exact over any backdrop, gradients included.
//! * [`draw_glyph_fb`] / [`blend`] — full glyph into a [`FrameBuffer`] against a **known**
//!   background colour, computed instead of read. The console surface and the title strip are
//!   background-clean by construction (band fills / `title_row_color`), and this keeps every
//!   glyph write WRITE-ONLY — no UC/WC read-back of a panel mapping, the GR15 sin.
//!
//! `alpha == 255` returns the ink bit-exactly and `alpha == 0` the background bit-exactly, so
//! any instrument comparing a glyph core against its ink constant still holds.
//!
//! ## What deliberately still uses `font8x8` — the fallback boundary, stated exactly
//!
//! The original wording ("aarch64 fallback") described the tree at the fonts merge, not the tree
//! today, and it was broader than the truth in one direction and narrower in another. The honest
//! boundary is **not architectural at all** — it is *has this surface reached its desktop seam
//! yet*:
//!
//! * **Before the seam — font8x8, and rightly.** [`super::fbcon`]'s console from reset until the
//!   arch's desktop-ready point (x86: the Kepler takeover's `panel_console_resume`; aarch64:
//!   [`super::desktop_firmware::activate`]'s `panel_console_face_arm`), and the panic screen when it lands
//!   before that point. This face costs an allocator-free table lookup and a static atlas — both
//!   available at reset — so the fold is not about capability; it is that the early console's job
//!   is to survive, and re-homing its cell grid mid-boot is a state change a boot that is already
//!   failing should not be asked to absorb. **After** the seam both arches' consoles are
//!   [`Face::Body`], panic included (the panic screen inherits whatever cell the console holds).
//! * **Never had a seam — font8x8, and it is a gap, not a fold.** [`super::pal`]'s `draw_text`
//!   (and therefore `pulsewin`'s labels, `console`, `ui_status`), `video::quarry`'s tree and list
//!   text, and `instgui`'s installer dialogs. Each of these owns a cached-RAM surface and could
//!   take [`draw_row`] tomorrow; none of them is blocked by anything this module does. They are
//!   named here so the boundary is a ledger and not an excuse.
//!
//! `font8x8` therefore stays in-tree as the pre-seam face and as those three surfaces' current
//! face.

use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

use super::framebuffer::FrameBuffer;

/// The BODY raster — the character-grid face. Its size is set by how much text has to fit on a
/// surface, not by any piece of furniture, so it is a constant and stays one: the panel console is
/// the only consumer today and its capacity ledger (`fbcon`'s `PANEL_SCALE` doc) is written against
/// this 16 px cell height.
const SIZE: RasterHeight = RasterHeight::Size16;

/// The CHROME raster — **derived from the bar metric, not chosen.**
///
/// The reference platform sets its title text at very close to three fifths of the title bar's
/// height (a 28 px bar carrying a ~17 px face). This applies that ratio to the bar height it is
/// handed and then snaps DOWN to the nearest raster the atlas is actually built at, so the result
/// is always a size that exists and always fits the band it was derived from.
///
/// The ladder is the set of rasters enabled in `crates/kernel/Cargo.toml`'s
/// `noto-sans-mono-bitmap` feature list, and the two must move together: adding a rung here
/// without the feature is a compile error at the `RasterHeight` variant, which is the failure
/// mode we want. At `TITLE_HEIGHT` = 34 this yields `Size20`; the ladder covers bar heights up to
/// ~40, and a taller bar than that wants `size_32` added to both places.
const fn chrome_raster(bar_h: usize) -> RasterHeight {
    match bar_h * 3 / 5 {
        h if h >= 24 => RasterHeight::Size24,
        h if h >= 20 => RasterHeight::Size20,
        _ => RasterHeight::Size16,
    }
}

/// The chrome atlas's raster, resolved once from the theme's own bar height.
const CHROME_SIZE: RasterHeight = chrome_raster(super::theme::TITLE_HEIGHT);

/// Glyph advance in pixels — the mono font's own, not a cell guess. 7 at `Size16`.
pub const CELL_W: usize = get_raster_width(FontWeight::Regular, SIZE);

/// Glyph cell height in pixels, including Noto's own line-gap padding. 16 at `Size16`.
pub const CELL_H: usize = SIZE.val();

/// Chrome glyph advance in pixels. 9 at `Size20`.
pub const CHROME_CELL_W: usize = get_raster_width(FontWeight::Regular, CHROME_SIZE);

/// Chrome glyph cell height in pixels. 20 at `Size20`.
pub const CHROME_CELL_H: usize = CHROME_SIZE.val();

/// Which atlas a text call draws from. The two are the same face at two rasters; the choice is
/// about what SETS the size — a character grid's capacity ([`Face::Body`]) or a piece of the
/// theme's furniture ([`Face::Chrome`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    /// 16 px — the panel console and any other character-grid surface.
    Body,
    /// [`CHROME_CELL_H`] px — captions, the bar, the crystal menu, dock tiles.
    Chrome,
}

impl Face {
    /// The face's glyph advance, in pixels.
    #[inline]
    pub const fn cell_w(self) -> usize {
        match self {
            Face::Body => CELL_W,
            Face::Chrome => CHROME_CELL_W,
        }
    }

    /// The face's cell height, in pixels.
    #[inline]
    pub const fn cell_h(self) -> usize {
        match self {
            Face::Body => CELL_H,
            Face::Chrome => CHROME_CELL_H,
        }
    }

    /// The face's name, for the boot witness that proves which face a surface actually drew with.
    /// `noto<raster>-aa` — the same shape `fbcon`'s `glyphs-active` banner already prints.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Face::Body => "noto16-aa",
            Face::Chrome => match CHROME_SIZE {
                RasterHeight::Size24 => "noto24-aa",
                RasterHeight::Size20 => "noto20-aa",
                _ => "noto16-aa",
            },
        }
    }
}

/// Printable ASCII: `0x20..=0x7E`. Everything outside renders as the space at index 0, so a
/// hostile byte can only ever paint a blank — the same containment rule every old path had.
const GLYPHS: usize = 0x5F;

/// One atlas: per glyph, `CELL_H` rows of `CELL_W` alpha bytes. Built at COMPILE TIME from the
/// crate's rodata — the table is 95 wide pointers per weight; the pixel data itself lives once
/// in the crate's statics.
const fn table(weight: FontWeight, size: RasterHeight) -> [&'static [&'static [u8]]; GLYPHS] {
    // A raster this table can never hand out: `get_raster` is total over Basic Latin for the
    // weights this module enables, and the const evaluator proves it — a missing glyph is a
    // compile error, not a runtime blank.
    let mut t: [&'static [&'static [u8]]; GLYPHS] = [&[]; GLYPHS];
    let mut i = 0;
    while i < GLYPHS {
        t[i] = match get_raster((0x20 + i as u8) as char, weight, size) {
            Some(r) => r.raster(),
            None => panic!("glyph missing from atlas"),
        };
        i += 1;
    }
    t
}

static REGULAR: [&'static [&'static [u8]]; GLYPHS] = table(FontWeight::Regular, SIZE);
static BOLD: [&'static [&'static [u8]]; GLYPHS] = table(FontWeight::Bold, SIZE);
static CHROME_REGULAR: [&'static [&'static [u8]]; GLYPHS] = table(FontWeight::Regular, CHROME_SIZE);
static CHROME_BOLD: [&'static [&'static [u8]]; GLYPHS] = table(FontWeight::Bold, CHROME_SIZE);

const _: () = {
    // Both weights must share the advance, or mixed-weight layout arithmetic would shear.
    assert!(get_raster_width(FontWeight::Bold, SIZE) == CELL_W);
    assert!(get_raster_width(FontWeight::Bold, CHROME_SIZE) == CHROME_CELL_W);
    assert!(CELL_W >= 1 && CELL_H >= 1);
    // The derivation's own contract: the chrome cell must fit the band it was derived from, and it
    // must not be SMALLER than the body face — that would mean the ladder had no rung for this bar
    // and the chrome silently fell back to terminal metrics, the exact defect PA43 reported.
    assert!(CHROME_CELL_H <= super::theme::TITLE_HEIGHT);
    assert!(CHROME_CELL_H >= CELL_H && CHROME_CELL_W >= CELL_W);
};

/// The full alpha raster for one byte in `face`: `face.cell_h()` rows × `face.cell_w()` columns,
/// row-major.
#[inline]
pub fn glyph(ch: u8, bold: bool, face: Face) -> &'static [&'static [u8]] {
    let i = if (0x20..0x7f).contains(&ch) { (ch - 0x20) as usize } else { 0 };
    match (face, bold) {
        (Face::Body, false) => REGULAR[i],
        (Face::Body, true) => BOLD[i],
        (Face::Chrome, false) => CHROME_REGULAR[i],
        (Face::Chrome, true) => CHROME_BOLD[i],
    }
}

/// Mix `ink` over `bg` at coverage `a` (0 = background, 255 = ink, both BIT-EXACT at the
/// endpoints so pixel-equality instruments keep their witness). Channels are 0x00RRGGBB —
/// the one packing every kernel surface uses.
#[inline]
pub fn blend(bg: u32, ink: u32, a: u8) -> u32 {
    match a {
        0 => bg,
        255 => ink,
        _ => {
            let a = a as i32;
            let ch = |shift: u32| -> u32 {
                let b = ((bg >> shift) & 0xFF) as i32;
                let i = ((ink >> shift) & 0xFF) as i32;
                (b + ((i - b) * a + 127) / 255) as u32
            };
            (ch(16) << 16) | (ch(8) << 8) | ch(0)
        }
    }
}

/// Blend one SCANLINE of an ASCII byte string into a cached-RAM row at `x0` — the strip
/// painters' shape (menubar, dock, crystal all paint row-at-a-time into a scratch row). `sy` is
/// the row within the glyph cell (`0..CELL_H`); rows outside draw nothing. The destination is
/// read, so the text composites correctly over gradients and fills alike — RAM reads only; a
/// strip scratch row is never a panel mapping.
#[inline]
pub fn draw_row(
    out: &mut [u32],
    w: usize,
    s: &[u8],
    x0: usize,
    sy: usize,
    ink: u32,
    bold: bool,
    face: Face,
) {
    if sy >= face.cell_h() {
        return;
    }
    for (c, &b) in s.iter().enumerate() {
        let row = glyph(b, bold, face)[sy];
        let gx = x0 + c * face.cell_w();
        for (rx, &a) in row.iter().enumerate() {
            if a == 0 {
                continue;
            }
            let i = gx + rx;
            if i < w {
                out[i] = blend(out[i], ink, a);
            }
        }
    }
}

/// Blend one glyph into a [`FrameBuffer`] cell at `(cx, cy)` against a KNOWN background —
/// computed, never read, so this is safe (and fast) on write-only mappings. The caller owes the
/// same invariant the 1-bit path relied on: the cell holds `bg` when this runs (fbcon's cells
/// are background-clean by band-fill construction). Only covered pixels are written, exactly as
/// before.
#[inline]
pub fn draw_glyph_fb(
    fb: &FrameBuffer,
    ch: u8,
    cx: usize,
    cy: usize,
    ink: u32,
    bg: u32,
    bold: bool,
    face: Face,
) {
    for (ry, row) in glyph(ch, bold, face).iter().enumerate() {
        for (rx, &a) in row.iter().enumerate() {
            if a != 0 {
                fb.put_pixel(cx + rx, cy + ry, blend(bg, ink, a));
            }
        }
    }
}

/// FONTSURF (SO48) — blend an ASCII byte string into a cached-RAM **SURFACE** at `(x, y)`, the
/// rectangular counterpart of [`draw_row`], and the seam the app surfaces have been missing.
///
/// ## Why this exists, stated as the gap it closes
///
/// The module doc's "never had a seam" list — `pal::draw_text`, `video::quarry`'s panes,
/// `instgui`'s dialogs, and `video::login`'s form — is not a list of surfaces that CHOSE the 1-bit
/// face. It is a list of surfaces whose painter is a rectangle, not a scanline: each owns a
/// `&mut [u32]` of `stride`-pitch cached RAM and writes `px[(y + ry) * stride + x + rx]`. The two
/// blits this module shipped both had the wrong shape for that — [`draw_row`] takes ONE scanline of
/// a strip's scratch row, and [`draw_glyph_fb`] writes a [`FrameBuffer`] against a computed
/// background. So every one of those surfaces kept `font8x8`, replicated each set bit into a
/// `ts`x`ts` block of flat ink, and the boundary stayed a "gap, not a fold" for want of ten lines.
///
/// This is those ten lines. The destination is READ (it is cached RAM by the caller's contract —
/// never a panel mapping, the GR15 sin), so the blend composites correctly over whatever the
/// surface already holds: a selection highlight, a content fill, a bevel.
///
/// ## Contract, matching the helpers it replaces
///
/// `stride` is the surface pitch in `u32`s; `clip_w`/`clip_h` are the surface's own extents. A
/// glyph that does not fit WHOLE inside the clip box ENDS the string — the same all-or-nothing
/// rule `quarry::text`, `instgui::text` and `login::text` each wrote by hand, so no caller changes
/// its truncation behaviour by adopting this. Returns the pen x after the last glyph drawn.
///
/// ## Metrics, and the one thing a caller must re-read
///
/// The advance becomes the FACE's (`face.cell_w()`), not `8 * ts`. A 1-bit 8x8 cell doubled is
/// square — 16 wide for 16 tall — where a 16 px mono face is 7 wide, because that is the width
/// Noto's own side bearings give it. So a converted caller's VERTICAL rhythm is unchanged when it
/// was built on a 16 px cell ([`CELL_H`] is 16), and its HORIZONTAL arithmetic must move from the
/// old cell to [`Face::cell_w`]. Callers that used one `CELL` constant for both axes have to split
/// it; that split is the whole cost of the conversion.
///
/// ## What it costs the image: NOTHING in rodata
///
/// No new atlas. Both rasters and both weights are already `static` above and already reachable
/// from [`glyph`], which `fbcon` calls on every build of either arch — so a surface adopting this
/// face adds its call site and not one byte of glyph data.
#[inline]
pub fn draw_text(
    px: &mut [u32],
    stride: usize,
    clip_w: usize,
    clip_h: usize,
    x: usize,
    y: usize,
    s: &[u8],
    ink: u32,
    bold: bool,
    face: Face,
) -> usize {
    let (cw, ch) = (face.cell_w(), face.cell_h());
    let mut cx = x;
    for &b in s {
        // All-or-nothing, and checked BEFORE the first pixel: a partially drawn glyph at the clip
        // edge is the one artefact the hand-written helpers all took care to avoid.
        if cx + cw > clip_w || y + ch > clip_h {
            break;
        }
        for (ry, row) in glyph(b, bold, face).iter().enumerate() {
            let base = (y + ry) * stride;
            for (rx, &a) in row.iter().enumerate() {
                if a == 0 {
                    continue;
                }
                let i = base + cx + rx;
                // The clip test above already bounds this; the length test is the containment
                // guarantee a caller passing a short slice or a lying stride still gets.
                if i < px.len() {
                    px[i] = blend(px[i], ink, a);
                }
            }
        }
        cx += cw;
    }
    cx
}

/// FONTAA (SO48) — **the COMPILE-TIME half of the anti-aliasing gate, and the reason it is here
/// rather than only in `selftest.rs`.**
///
/// The runtime fixture (`selftest.rs`'s `video.font.aa`) lives in `tste`'s LIVE section, and that
/// section is reached only when an operator types `tste`: it is NOT executed by `./arroyo test`,
/// measured rather than assumed — `LC_ALL=C grep -a -c -F ":: TSTE: suite start"` on a battery
/// capture is 0, and so is the count for `video.geometry`, the live fixture that has sat beside it
/// for arcs. A gate that only a bench operator can fire is not a gate the battery holds, so the one
/// leg that can be settled without a blend runs HERE, in every build of either arch, and goes red
/// in `./arroyo check`.
///
/// WHAT IT ASSERTS. "Blocky" is a statement about COVERAGE, not about size: a 1-bit table has two
/// ink states, so an edge is a step at any magnification, and block-replicating it multiplies the
/// tread without adding information. An 8-bit alpha face is the opposite claim about the same
/// bytes. So the check is: does the shipped atlas carry PARTIAL coverage?
///
/// ⚠ A RULE THIS ARC PAID FOR, PARKED HERE BECAUSE THE TAIL IS WHERE AN APPEND SHIFTS NOTHING (it
/// belongs on [`glyph`], and a doc comment there would move every `panic::Location` below it in this
/// file). **Pass `face` to [`glyph`] as a COMPILE-TIME CONSTANT, or the image pays for both
/// rasters.** `glyph`'s four-arm match is what lets the linker strip the atlas pair a given build
/// never uses: with a constant `face` it folds to two statics, and in a build with no chrome — no
/// menu bar, no captions, no dock — the `Size20` pair is then unreferenced and gone. Hand it a value
/// the optimizer cannot see through and all four arms stay live. The price is measured, not
/// estimated: making ONE fixture's face opaque grew the knob-off loadable image by **87,482 bytes on
/// aarch64 and 121,652 on x86** (`./arroyo knoboff wc f164b6fd`, 2026-09-13), against the `Size20`
/// regular+bold pair's 98,040 B priced from the crate's own geometry (95 glyphs x 20 rows x 9 px,
/// plus the row-slice tables). Every call site in the tree passes a `const` today — `crystal.rs:178`,
/// `menubar.rs:167`, `dock.rs:344`, `winmenu.rs:657`, `fbcon.rs:168` — and this note exists so the
/// next one does too.
///
/// THE CONTROL. `has_zero` is required for the same reason every gate here carries one — a scan
/// that found a single class of byte would report `has_mid` from a buffer it never really read, and
/// would report it in the same words a true pass uses. Requiring both classes means the scan
/// demonstrably distinguished them. Deliberately NOT required: a fully-opaque byte. Whether any
/// glyph in a given raster reaches 255 is a fact about Noto's hinting at that size, not about
/// whether the face is anti-aliased, and asserting it would make this gate go red on a font update
/// for a reason that is not the one it is about.
const _: () = {
    const fn coverage(size: RasterHeight) -> (bool, bool) {
        let t = table(FontWeight::Regular, size);
        let (mut has_zero, mut has_mid) = (false, false);
        let mut i = 0;
        while i < GLYPHS {
            let rows = t[i];
            let mut y = 0;
            while y < rows.len() {
                let row = rows[y];
                let mut x = 0;
                while x < row.len() {
                    let a = row[x];
                    if a == 0 {
                        has_zero = true;
                    } else if a != 255 {
                        has_mid = true;
                    }
                    x += 1;
                }
                y += 1;
            }
            i += 1;
        }
        (has_zero, has_mid)
    }

    let (body_zero, body_mid) = coverage(SIZE);
    assert!(body_zero, "FONTAA control: the body atlas scan saw no empty pixel — it read nothing");
    assert!(body_mid, "FONTAA: the body atlas has no partial coverage — this is a 1-bit face");

    let (chrome_zero, chrome_mid) = coverage(CHROME_SIZE);
    assert!(chrome_zero, "FONTAA control: the chrome atlas scan saw no empty pixel — it read nothing");
    assert!(chrome_mid, "FONTAA: the chrome atlas has no partial coverage — this is a 1-bit face");
};
