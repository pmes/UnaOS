//! FONTHINT (LEDGER SR62): hinting, the way Chromium on Linux asks FreeType for it.
//!
//! The host's fontconfig gives every face `hintslight` (10-hinting-slight.conf, autohint false). Skia maps that to
//! `SkFontHinting::kSlight`, which loads glyphs with `FT_LOAD_TARGET_LIGHT`. FreeType 2.13 then routes:
//!
//! - **TrueType** faces to its **auto-hinter** in light mode (the TrueType driver does not declare
//!   `FT_MODULE_DRIVER_HINTS_LIGHTLY`, so `FT_LOAD_TARGET_LIGHT` selects the autofitter, not the bytecode
//!   interpreter — measured on this host: light == force-autohint ≠ no-autohint for DejaVu and Liberation, and
//!   Chromium's raster matches the autohinted glyph, mean |Δ| 2.2 levels against 4.8 for the v40 interpreter);
//! - **CFF** faces to the **Adobe CFF engine** (which hints lightly natively).
//!
//! [`autofit`] is the light auto-hinter: FreeType's latin writing system (blue zones from the script's reference
//! characters, the x-height scale adjustment, segments → edges → stems, blue-zone snapping, strong and weak point
//! interpolation), vertical only, plus its CJK writing system (both axes) for CJK, Indic and every glyph no script
//! claims. [`cff_hint`] is the Adobe CFF engine's hint model. [`Hinting`] is the mode the rasterizer and the glyph
//! caches key on.

pub mod autofit;
mod autofit_tables;
pub mod cff_hint;
pub mod coverage;
pub mod fixed;

use crate::path::{OutlineSink, Path};
use alloc::vec::Vec;

/// How glyph outlines are grid-fitted before rasterization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hinting {
    /// Unhinted outlines (what Chromium draws for web fonts).
    #[default]
    None,
    /// fontconfig `hintslight` as Chromium applies it to installed faces: FreeType `FT_LOAD_TARGET_LIGHT` —
    /// the light auto-hinter for TrueType outlines, vertical snapping only.
    Slight,
}

/// FreeType outline point tags.
pub const TAG_ON: u8 = 1;
pub const TAG_CONIC: u8 = 0;
pub const TAG_CUBIC: u8 = 2;

/// A glyph outline as FreeType holds it: points (font units, or 26.6 after hinting; y up), per-point tags
/// ([`TAG_ON`], [`TAG_CONIC`], [`TAG_CUBIC`]) and contour end indices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outline {
    pub points: Vec<(i64, i64)>,
    pub tags: Vec<u8>,
    pub ends: Vec<usize>,
}

impl Outline {
    /// The outline as a path in pixels (points / 64), the way `FT_Outline_Decompose` walks it: consecutive
    /// conic controls imply an on-curve midpoint, a contour may start on a control point.
    pub fn to_path_26_6(&self) -> Path {
        let mut p = Path::new();
        let f = |v: i64| v as f32 / 64.0;
        let mut start = 0usize;
        for &end in &self.ends {
            if end < start || end >= self.points.len() {
                break;
            }
            let pts = &self.points[start..=end];
            let tags = &self.tags[start..=end];
            decompose(pts, tags, &mut p, &f);
            start = end + 1;
        }
        p
    }
}

fn decompose(pts: &[(i64, i64)], tags: &[u8], p: &mut Path, f: &dyn Fn(i64) -> f32) {
    let n = pts.len();
    if n == 0 {
        return;
    }
    let pt = |i: usize| (f(pts[i % n].0), f(pts[i % n].1));
    let mid = |a: (f32, f32), b: (f32, f32)| ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5);
    // FT_Outline_Decompose: choose the start point.
    let (start, mut i, limit) = if tags[0] == TAG_ON {
        (pt(0), 1usize, n)
    } else if tags[n - 1] == TAG_ON {
        (pt(n - 1), 0usize, n - 1)
    } else if tags[0] == TAG_CONIC {
        (mid(pt(0), pt(n - 1)), 0usize, n)
    } else {
        return; // a contour starting with a cubic control is invalid
    };
    p.move_to(start.0, start.1);
    while i < limit {
        match tags[i] {
            TAG_ON => {
                let q = pt(i);
                p.line_to(q.0, q.1);
                i += 1;
            }
            TAG_CONIC => {
                let mut c = pt(i);
                i += 1;
                loop {
                    if i >= limit {
                        p.quad_to(c.0, c.1, start.0, start.1);
                        p.close();
                        return;
                    }
                    let q = pt(i);
                    if tags[i] == TAG_ON {
                        p.quad_to(c.0, c.1, q.0, q.1);
                        i += 1;
                        break;
                    }
                    if tags[i] != TAG_CONIC {
                        return;
                    }
                    let m = mid(c, q);
                    p.quad_to(c.0, c.1, m.0, m.1);
                    c = q;
                    i += 1;
                }
            }
            _ => {
                // two cubic controls then an end point (which may be the start)
                if i + 1 >= limit + 1 || tags.get(i + 1) != Some(&TAG_CUBIC) {
                    return;
                }
                let (c1, c2) = (pt(i), pt(i + 1));
                i += 2;
                if i < limit {
                    let q = pt(i);
                    p.curve_to(c1.0, c1.1, c2.0, c2.1, q.0, q.1);
                    i += 1;
                } else {
                    p.curve_to(c1.0, c1.1, c2.0, c2.1, start.0, start.1);
                    p.close();
                    return;
                }
            }
        }
    }
    p.line_to(start.0, start.1);
    p.close();
}
