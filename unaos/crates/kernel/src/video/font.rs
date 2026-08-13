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
//! **Noto Sans Mono**, pre-rasterized to an 8-bit ALPHA atlas at build time by the
//! `noto-sans-mono-bitmap` crate (regular + bold, 16 px raster, Basic Latin). The atlas is pure
//! `static` rodata — nothing here allocates, so every function is safe in present context, in
//! panic context, and pre-heap.
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
//! ## What deliberately still uses `font8x8`
//!
//! The pre-takeover boot console (aarch64 whole-boot, x86 before the Kepler seam), `pal`'s
//! legacy GUI strip, and `instgui`'s dialogs. The first is a deliberate fold: the early console
//! must work with nothing but the raw panel and is not a taste surface; the others are follow-up
//! rungs. `font8x8` therefore stays in-tree as the fallback face.

use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

use super::framebuffer::FrameBuffer;

/// The raster size the atlas is built at. ONE size, deliberately: every kernel text surface
/// today draws a 16 px cell (`wm::TITLE_CELL_H`, the console's `PANEL_SCALE`d cell), so one
/// atlas serves all of them and a future size change is this constant plus the feature list in
/// `Cargo.toml`.
const SIZE: RasterHeight = RasterHeight::Size16;

/// Glyph advance in pixels — the mono font's own, not a cell guess. 7 at `Size16`.
pub const CELL_W: usize = get_raster_width(FontWeight::Regular, SIZE);

/// Glyph cell height in pixels, including Noto's own line-gap padding. 16 at `Size16`.
pub const CELL_H: usize = SIZE.val();

/// Printable ASCII: `0x20..=0x7E`. Everything outside renders as the space at index 0, so a
/// hostile byte can only ever paint a blank — the same containment rule every old path had.
const GLYPHS: usize = 0x5F;

/// One atlas: per glyph, `CELL_H` rows of `CELL_W` alpha bytes. Built at COMPILE TIME from the
/// crate's rodata — the table is 95 wide pointers per weight; the pixel data itself lives once
/// in the crate's statics.
const fn table(weight: FontWeight) -> [&'static [&'static [u8]]; GLYPHS] {
    // A raster this table can never hand out: `get_raster` is total over Basic Latin for the
    // weights this module enables, and the const evaluator proves it — a missing glyph is a
    // compile error, not a runtime blank.
    let mut t: [&'static [&'static [u8]]; GLYPHS] = [&[]; GLYPHS];
    let mut i = 0;
    while i < GLYPHS {
        t[i] = match get_raster((0x20 + i as u8) as char, weight, SIZE) {
            Some(r) => r.raster(),
            None => panic!("glyph missing from atlas"),
        };
        i += 1;
    }
    t
}

static REGULAR: [&'static [&'static [u8]]; GLYPHS] = table(FontWeight::Regular);
static BOLD: [&'static [&'static [u8]]; GLYPHS] = table(FontWeight::Bold);

const _: () = {
    // Both weights must share the advance, or mixed-weight layout arithmetic would shear.
    assert!(get_raster_width(FontWeight::Bold, SIZE) == CELL_W);
    assert!(CELL_W >= 1 && CELL_H >= 1);
};

/// The full alpha raster for one byte: `CELL_H` rows × `CELL_W` columns, row-major.
#[inline]
pub fn glyph(ch: u8, bold: bool) -> &'static [&'static [u8]] {
    let i = if (0x20..0x7f).contains(&ch) { (ch - 0x20) as usize } else { 0 };
    if bold { BOLD[i] } else { REGULAR[i] }
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
pub fn draw_row(out: &mut [u32], w: usize, s: &[u8], x0: usize, sy: usize, ink: u32, bold: bool) {
    if sy >= CELL_H {
        return;
    }
    for (c, &b) in s.iter().enumerate() {
        let row = glyph(b, bold)[sy];
        let gx = x0 + c * CELL_W;
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
pub fn draw_glyph_fb(fb: &FrameBuffer, ch: u8, cx: usize, cy: usize, ink: u32, bg: u32, bold: bool) {
    for (ry, row) in glyph(ch, bold).iter().enumerate() {
        for (rx, &a) in row.iter().enumerate() {
            if a != 0 {
                fb.put_pixel(cx + rx, cy + ry, blend(bg, ink, a));
            }
        }
    }
}
