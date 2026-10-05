//! Glyph rasterization the way Chromium draws text on Linux (AETHERFONT M3), on font_core's rasterizer:
//!
//! - coverage from `RenderMode::SkiaAaa` (exact area with Skia analytic-AA's quarter-row edge snap; no
//!   hinting — Chromium's Linux default for web content is `--font-render-hinting` off in the oracle and
//!   slight hinting is not applied to subpixel-positioned text);
//! - glyph origins at quarter-pixel x phases (Skia's subpixel positioning for horizontal text, rounded to
//!   the nearest quarter), whole-pixel baselines;
//! - Skia's A8 mask pre-blend ([`preblend`]): `SkTMaskGamma_build_correcting_lut` with contrast 0.2 and
//!   gamma 1.2 (Chromium Linux's SK_GAMMA_CONTRAST / SK_GAMMA_EXPONENT), indexed by the ink's luminance in
//!   3 bits, as `SkScalerContext` does for A8 masks;
//! - synthetic bold as Skia's FreeType port does it: `FT_Outline_Embolden` with strength ppem/24
//!   ([`embolden`], FreeType's algorithm), advances unchanged; synthetic oblique as Blink sets it, a skew
//!   of −1/4 (x += y/4 in font units).

use super::Face;
use font_core::path::{OutlineSink, Path, PathCmd};
use font_core::raster::{Rasterizer, RenderMode, Scaled};
use std::collections::HashMap;
use std::rc::Rc;

/// A glyph's raw coverage (before the pre-blend), placed relative to its pen origin.
#[derive(Debug, Clone)]
pub struct GlyphMask {
    pub left: i32,
    pub top: i32,
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// Quarter-pixel x positioning: the whole pixel and the phase 0..=3 (nearest quarter, Skia's rounding).
pub fn split_x(x: f32) -> (i32, u8) {
    let q = (x * 4.0).round() as i64;
    ((q.div_euclid(4)) as i32, q.rem_euclid(4) as u8)
}

/// One contour as FreeType sees it: points with on/off-curve tags (off = quadratic or cubic control).
struct Contour {
    pts: Vec<(f32, f32)>,
    /// 0 on-curve, 1 quadratic control, 2 cubic control.
    tags: Vec<u8>,
}

fn contours_of(p: &Path) -> Vec<Contour> {
    let mut out: Vec<Contour> = Vec::new();
    let mut cur: Option<Contour> = None;
    let mut flush = |c: &mut Option<Contour>, out: &mut Vec<Contour>| {
        if let Some(mut k) = c.take() {
            // a closing point equal to the start is the same FreeType point
            if k.pts.len() > 1 && k.pts.last() == k.pts.first() && k.tags.last() == Some(&0) {
                k.pts.pop();
                k.tags.pop();
            }
            if !k.pts.is_empty() {
                out.push(k);
            }
        }
    };
    for c in &p.cmds {
        match *c {
            PathCmd::MoveTo(x, y) => {
                flush(&mut cur, &mut out);
                cur = Some(Contour { pts: vec![(x, y)], tags: vec![0] });
            }
            PathCmd::LineTo(x, y) => {
                if let Some(k) = cur.as_mut() {
                    k.pts.push((x, y));
                    k.tags.push(0);
                }
            }
            PathCmd::QuadTo(a, b, x, y) => {
                if let Some(k) = cur.as_mut() {
                    k.pts.extend([(a, b), (x, y)]);
                    k.tags.extend([1, 0]);
                }
            }
            PathCmd::CubicTo(a, b, c2, d, x, y) => {
                if let Some(k) = cur.as_mut() {
                    k.pts.extend([(a, b), (c2, d), (x, y)]);
                    k.tags.extend([2, 2, 0]);
                }
            }
            PathCmd::Close => flush(&mut cur, &mut out),
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn path_of(cs: &[Contour]) -> Path {
    let mut p = Path::new();
    for c in cs {
        let n = c.pts.len();
        if n == 0 {
            continue;
        }
        // start at an on-curve point
        let s = (0..n).find(|&i| c.tags[i] == 0).unwrap_or(0);
        let at = |k: usize| c.pts[(s + k) % n];
        let tag = |k: usize| c.tags[(s + k) % n];
        p.move_to(at(0).0, at(0).1);
        let mut k = 1;
        while k <= n {
            match tag(k % n) {
                1 => {
                    let (a, e) = (at(k), at(k + 1));
                    p.quad_to(a.0, a.1, e.0, e.1);
                    k += 2;
                }
                2 => {
                    let (a, b, e) = (at(k), at(k + 1), at(k + 2));
                    p.curve_to(a.0, a.1, b.0, b.1, e.0, e.1);
                    k += 3;
                }
                _ => {
                    let e = at(k);
                    p.line_to(e.0, e.1);
                    k += 1;
                }
            }
        }
        p.close();
    }
    p
}

/// FreeType's `FT_Outline_EmboldenXY(outline, s, s)` on a font_core path (font units): every point moves
/// along the bisector of its two edges so each edge shifts outward by s/2, then the outline is translated
/// by (s/2, s/2) — the glyph grows by `s` to the right and up, its lower-left edges stay.
pub fn embolden(p: &Path, strength: f32) -> Path {
    let mut cs = contours_of(p);
    // FT_Outline_Get_Orientation: the sign of Σ (y_i − y_{i−1})(x_i + x_{i−1}) over every contour.
    let mut area = 0.0f64;
    for c in &cs {
        let n = c.pts.len();
        for i in 0..n {
            let (a, b) = (c.pts[(i + n - 1) % n], c.pts[i]);
            area += (b.1 - a.1) as f64 * (b.0 + a.0) as f64;
        }
    }
    let truetype = area < 0.0; // clockwise (fill right)
    let (xs, ys) = (strength / 2.0, strength / 2.0);
    for c in cs.iter_mut() {
        let n = c.pts.len();
        if n < 2 {
            continue;
        }
        let orig = c.pts.clone();
        for i in 0..n {
            let p0 = orig[i];
            // nearest distinct neighbours
            let prev = (1..n).map(|k| orig[(i + n - k) % n]).find(|q| *q != p0);
            let next = (1..n).map(|k| orig[(i + k) % n]).find(|q| *q != p0);
            let (Some(pv), Some(nx)) = (prev, next) else { continue };
            let (ix, iy) = (p0.0 - pv.0, p0.1 - pv.1);
            let (ox, oy) = (nx.0 - p0.0, nx.1 - p0.1);
            let (l_in, l_out) = ((ix * ix + iy * iy).sqrt(), (ox * ox + oy * oy).sqrt());
            let (ix, iy, ox, oy) = (ix / l_in, iy / l_in, ox / l_out, oy / l_out);
            let mut d = ix * ox + iy * oy;
            let (mut sx, mut sy) = (0.0f32, 0.0f32);
            if d > -0.9375 {
                d += 1.0;
                sx = iy + oy;
                sy = ix + ox;
                if truetype {
                    sx = -sx;
                } else {
                    sy = -sy;
                }
                let mut q = ox * iy - oy * ix;
                if truetype {
                    q = -q;
                }
                let l = l_in.min(l_out);
                sx = if xs * q <= l * d { sx * xs / d } else { sx * l / q };
                sy = if ys * q <= l * d { sy * ys / d } else { sy * l / q };
            }
            c.pts[i] = (p0.0 + xs + sx, p0.1 + ys + sy);
        }
    }
    path_of(&cs)
}

/// Skew x by `k`·y (font units; y up): Blink's synthetic oblique is k = 1/4.
fn skew(p: &Path, k: f32) -> Path {
    let mut out = Path::new();
    let t = |x: f32, y: f32| (x + k * y, y);
    for c in &p.cmds {
        match *c {
            PathCmd::MoveTo(x, y) => {
                let a = t(x, y);
                out.move_to(a.0, a.1)
            }
            PathCmd::LineTo(x, y) => {
                let a = t(x, y);
                out.line_to(a.0, a.1)
            }
            PathCmd::QuadTo(a, b, x, y) => {
                let (c1, e) = (t(a, b), t(x, y));
                out.quad_to(c1.0, c1.1, e.0, e.1)
            }
            PathCmd::CubicTo(a, b, c2, d, x, y) => {
                let (p1, p2, e) = (t(a, b), t(c2, d), t(x, y));
                out.curve_to(p1.0, p1.1, p2.0, p2.1, e.0, e.1)
            }
            PathCmd::Close => out.close(),
        }
    }
    out
}

/// Rasterizes one glyph of `face` at `size` px with its origin at x phase `sub` quarters.
pub fn rasterize(face: &Face, gid: u16, size: f32, sub: u8) -> Option<GlyphMask> {
    let mut path = face.font.glyph_path(gid)?;
    if path.is_empty() || !(size > 0.0 && size <= 4096.0) {
        return None;
    }
    let upem = face.font.units_per_em.max(1) as f32;
    if face.synth_bold {
        path = embolden(&path, upem / 24.0);
    }
    if face.synth_oblique {
        path = skew(&path, 0.25);
    }
    let scale = size / upem;
    let sub_x = sub as f32 / 4.0;
    let x0 = (sub_x + path.x_min * scale).floor() as i32 - 1;
    let x1 = (sub_x + path.x_max * scale).ceil() as i32 + 1;
    let y0 = (-path.y_max * scale).floor() as i32 - 1;
    let y1 = (-path.y_min * scale).ceil() as i32 + 1;
    let (w, h) = ((x1 - x0).max(1) as usize, (y1 - y0).max(1) as usize);
    if w * h > 1 << 22 {
        return None;
    }
    let mut r = Rasterizer::new(w, h);
    r.mode = RenderMode::SkiaAaa;
    {
        let mut s = Scaled { r: &mut r, scale, ox: sub_x - x0 as f32, oy: -(y0 as f32) };
        path.replay(&mut s);
    }
    let data = r.finish();
    if data.iter().all(|&v| v == 0) {
        return None;
    }
    Some(GlyphMask { left: x0, top: y0, w: w as u32, h: h as u32, data })
}

type Key = (u32, u16, u32, u8);

thread_local! {
    static GLYPHS: std::cell::RefCell<HashMap<Key, Option<Rc<GlyphMask>>>> = std::cell::RefCell::new(HashMap::new());
}

const GLYPH_CAP: usize = 32_768;

/// The cached coverage of a glyph (size quantized to 1/64 px).
pub fn glyph(face: &Face, gid: u16, size: f32, sub: u8) -> Option<Rc<GlyphMask>> {
    let size_64 = (size * 64.0).round() as u32;
    let key = (face.id, gid, size_64, sub & 3);
    if let Some(g) = GLYPHS.with(|c| c.borrow().get(&key).cloned()) {
        return g;
    }
    let g = rasterize(face, gid, size_64 as f32 / 64.0, sub & 3).map(Rc::new);
    GLYPHS.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= GLYPH_CAP {
            c.clear();
        }
        c.insert(key, g.clone());
    });
    g
}

/// SkColorSpaceLuminance::computeLuminance with gamma 1.2: the ink's perceptual luminance 0..=255.
pub fn luminance(ink: (u8, u8, u8)) -> u8 {
    let g = 1.2f32;
    let lin = |v: u8| (v as f32 / 255.0).powf(g);
    let l = lin(ink.0) * 0.2126 + lin(ink.1) * 0.7152 + lin(ink.2) * 0.0722;
    (l.powf(1.0 / g) * 255.0).round().clamp(0.0, 255.0) as u8
}

/// `SkTMaskGamma_build_correcting_lut(table, srcI, contrast 0.2, gamma 1.2, gamma 1.2)`.
pub fn correcting_lut(src_i: u8) -> [u8; 256] {
    let (contrast, g) = (0.2f32, 1.2f32);
    let to_luma = |x: f32| x.powf(g);
    let from_luma = |x: f32| x.powf(1.0 / g);
    let src = src_i as f32 / 255.0;
    let lin_src = to_luma(src);
    let dst = 1.0 - src;
    let lin_dst = to_luma(dst);
    let adj = contrast * lin_dst;
    let apply = |a: f32| a + (1.0 - a) * adj * a;
    let mut t = [0u8; 256];
    for (i, v) in t.iter_mut().enumerate() {
        let raw = i as f32 / 255.0;
        let srca = apply(raw);
        *v = if (src - dst).abs() < 1.0 / 256.0 {
            (255.0 * srca).round() as u8
        } else {
            let lin_out = lin_src * srca + (1.0 - srca) * lin_dst;
            let out = from_luma(lin_out);
            let r = (out - dst) / (src - dst);
            (255.0 * r).round().clamp(0.0, 255.0) as u8
        };
    }
    t
}

/// The A8 pre-blend for an ink colour: its luminance quantized to 3 bits (SkTMaskGamma<3,3,3>, the G table
/// for A8 masks), expanded back to 8 bits, through [`correcting_lut`].
pub fn preblend(ink: (u8, u8, u8)) -> Rc<[u8; 256]> {
    thread_local! {
        static LUTS: std::cell::RefCell<[Option<Rc<[u8; 256]>>; 8]> = const { std::cell::RefCell::new([const { None }; 8]) };
    }
    let bucket = (luminance(ink) >> 5) as usize;
    LUTS.with(|l| {
        let mut l = l.borrow_mut();
        l[bucket]
            .get_or_insert_with(|| {
                let b = bucket as u32;
                let src = ((b << 5) | (b << 2) | (b >> 1)) as u8;
                Rc::new(correcting_lut(src))
            })
            .clone()
    })
}

/// Draws one glyph with its pen origin at (`x`, `baseline`): `put(px, py, alpha)` for every covered pixel,
/// alpha already through the ink's pre-blend.
pub fn draw_glyph(face: &Face, gid: u16, size: f32, x: f32, baseline: f32, lut: &[u8; 256], put: &mut dyn FnMut(i32, i32, u8)) {
    let (ix, sub) = split_x(x);
    let Some(g) = glyph(face, gid, size, sub) else { return };
    let iy = baseline.round() as i32;
    for row in 0..g.h as i32 {
        let base = (row as u32 * g.w) as usize;
        for col in 0..g.w as i32 {
            let a = lut[g.data[base + col as usize] as usize];
            if a != 0 {
                put(ix + g.left + col, iy + g.top + row, a);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preblend_matches_fontcore_dark_table() {
        // black ink: luminance bucket 0, the table FONTCORE fitted to Chromium (contrast 0.2, gamma 1.2)
        let t = preblend((0, 0, 0));
        let want = &font_core::ui::PREBLEND_DARK;
        let worst = (0..256).map(|i| (t[i] as i32 - want[i] as i32).abs()).max().unwrap();
        assert!(worst <= 1, "worst {worst}");
        assert_eq!(luminance((255, 255, 255)), 255);
        assert_eq!(luminance((0, 0, 0)), 0);
        // white ink: no contrast boost, gamma only: a^(1/1.2)
        let w = correcting_lut(255);
        assert_eq!(w[0], 0);
        assert_eq!(w[255], 255);
        assert_eq!(w[128], ((128.0f32 / 255.0).powf(1.0 / 1.2) * 255.0).round() as u8);
    }

    #[test]
    fn quarter_pixel_split() {
        assert_eq!(split_x(10.0), (10, 0));
        assert_eq!(split_x(10.13), (10, 1));
        assert_eq!(split_x(10.9), (11, 0));
        assert_eq!(split_x(-0.3), (-1, 3));
    }

    /// Embolden on a CCW square grows it by the strength to the right and up (FreeType's behaviour), and on
    /// a CW square the same; oblique shears by y/4.
    #[test]
    fn embolden_and_skew_kat() {
        let sq = |cw: bool| {
            let mut p = Path::new();
            let pts: [(f32, f32); 4] = if cw { [(0.0, 0.0), (0.0, 100.0), (100.0, 100.0), (100.0, 0.0)] } else { [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)] };
            p.move_to(pts[0].0, pts[0].1);
            for q in &pts[1..] {
                p.line_to(q.0, q.1);
            }
            p.close();
            p
        };
        for cw in [false, true] {
            let e = embolden(&sq(cw), 10.0);
            assert!((e.x_min - 0.0).abs() < 1e-3 && (e.y_min - 0.0).abs() < 1e-3, "{cw}: {:?}", (e.x_min, e.y_min));
            assert!((e.x_max - 110.0).abs() < 1e-3 && (e.y_max - 110.0).abs() < 1e-3, "{cw}: {:?}", (e.x_max, e.y_max));
        }
        let s = skew(&sq(false), 0.25);
        assert!((s.x_max - 125.0).abs() < 1e-3);
    }

    #[test]
    fn synthetic_bold_inks_more() {
        let Some(f) = crate::fonts::face(&crate::fonts::FontSel::new(crate::fonts::SANS, 400, false)) else { return };
        let gid = f.font.glyph_index('l');
        let plain = rasterize(f, gid, 32.0, 0).unwrap();
        let bold = Face { id: u32::MAX, font: f.font, synth_bold: true, synth_oblique: false, family: f.family.clone(), style: f.style };
        let b = rasterize(&bold, gid, 32.0, 0).unwrap();
        let ink = |m: &GlyphMask| m.data.iter().map(|&v| v as u32).sum::<u32>() as f32 / 255.0;
        // the stem widens by ppem/24 = 1.33 px over its height
        let extra = ink(&b) - ink(&plain);
        let stem_h = (f.font.glyph_path(gid).unwrap().y_max * 32.0 / f.font.units_per_em as f32) as f32;
        assert!((extra / stem_h - 32.0 / 24.0).abs() < 0.35, "extra {extra} over {stem_h}");
    }
}
