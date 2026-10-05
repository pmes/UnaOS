//! The glyph cache: rendered coverage bitmaps keyed by (font id, glyph, size, subpixel x, subpixel y).
//! Sizes are keyed in 1/64 px; origins are quantized to [`SUBPIXEL_STEPS`] positions per pixel on each axis
//! (Skia's horizontal-text choice is 4 x positions, 1 y position; both axes are kept so vertical subpixel
//! placement costs nothing extra). When the cache reaches its entry cap it is cleared wholesale.

use crate::fmath::{floor, round};
use crate::raster::{rasterize_glyph_mode, GlyphBitmap, RenderMode};
use crate::Font;
use alloc::collections::BTreeMap;

pub const SUBPIXEL_STEPS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey {
    pub font: u32,
    pub glyph: u16,
    pub size_64: u32,
    pub sub_x: u8,
    pub sub_y: u8,
}

/// Split a pixel coordinate into (integer pixel, subpixel step in 0..SUBPIXEL_STEPS).
pub fn split_position(x: f32) -> (i32, u8) {
    let q = round(x * SUBPIXEL_STEPS as f32) as i64;
    let s = SUBPIXEL_STEPS as i64;
    (q.div_euclid(s) as i32, q.rem_euclid(s) as u8)
}

pub struct GlyphCache {
    map: BTreeMap<CacheKey, Option<GlyphBitmap>>,
    cap: usize,
    /// Coverage mode for every glyph this cache renders (change it only on an empty cache).
    pub mode: RenderMode,
    pub hits: u64,
    pub misses: u64,
}

impl GlyphCache {
    /// A cache rendering in `mode`.
    pub fn with_mode(cap: usize, mode: RenderMode) -> Self {
        let mut c = Self::new(cap);
        c.mode = mode;
        c
    }

    pub fn new(cap: usize) -> Self {
        GlyphCache { map: BTreeMap::new(), cap: cap.max(1), mode: RenderMode::Exact, hits: 0, misses: 0 }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// The bitmap for `glyph` of font `font_id` at `size` px with subpixel steps (`sub_x`, `sub_y`).
    /// `None` for glyphs without an outline.
    pub fn get(&mut self, font_id: u32, font: &Font, glyph: u16, size: f32, sub_x: u8, sub_y: u8) -> Option<&GlyphBitmap> {
        let key = CacheKey {
            font: font_id,
            glyph,
            size_64: floor(size * 64.0 + 0.5) as u32,
            sub_x: sub_x % SUBPIXEL_STEPS as u8,
            sub_y: sub_y % SUBPIXEL_STEPS as u8,
        };
        if self.map.contains_key(&key) {
            self.hits += 1;
        } else {
            self.misses += 1;
            if self.map.len() >= self.cap {
                self.map.clear();
            }
            let step = 1.0 / SUBPIXEL_STEPS as f32;
            let size_q = key.size_64 as f32 / 64.0;
            let bm = rasterize_glyph_mode(font, glyph, size_q, key.sub_x as f32 * step, key.sub_y as f32 * step, self.mode);
            self.map.insert(key, bm);
        }
        self.map.get(&key).and_then(|b| b.as_ref())
    }
}
