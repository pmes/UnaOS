//! Box decoration painting with rounded corners and border styles
//! (css-backgrounds-3 §4 border-style, §5 border-radius).
//!
//! The box is a rounded rectangle (per-corner circular radii, reduced
//! proportionally when adjacent radii overflow a side, §5.5). Coverage is
//! the signed distance to that shape, so corners are anti-aliased. The
//! background fills the border box (`background-clip: border-box`); the
//! border is the outer shape minus the padding-edge shape, whose radii
//! are the outer radii less the adjoining border widths (§5.2). Each
//! border pixel belongs to the side whose width-normalised distance is
//! smallest (the corner joins split on the diagonal through the widths).
//! dashed: dashes of 2w with gaps of w; dotted: w-square dots every 2w;
//! double: two strokes of w/3 with a w/3 gap — Chromium's pattern sizes.

/// One side: (width px, colour, style code 0 solid / 1 dashed / 2 dotted / 3 double).
pub(crate) type Side = Option<(f32, (u8, u8, u8), u8)>;

/// Signed distance from (px, py) to a rounded rect (negative inside).
pub(crate) fn sdf(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: [f32; 4]) -> f32 {
    let cx = (x0 + x1) / 2.0;
    let cy = (y0 + y1) / 2.0;
    let hw = (x1 - x0) / 2.0;
    let hh = (y1 - y0) / 2.0;
    // Corner radius for this quadrant: [tl, tr, br, bl].
    let rad = match (px >= cx, py >= cy) {
        (false, false) => r[0],
        (true, false) => r[1],
        (true, true) => r[2],
        (false, true) => r[3],
    }
    .min(hw.max(0.0))
    .min(hh.max(0.0));
    let qx = (px - cx).abs() - (hw - rad);
    let qy = (py - cy).abs() - (hh - rad);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - rad
}

fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// Resolves percentage radii (negative fractions) against the box and
/// scales all radii down when adjacent ones overflow a side (§5.5).
pub(crate) fn used_radii(r: [f32; 4], w: f32, h: f32) -> [f32; 4] {
    let mut r = r.map(|v| if v < 0.0 { -v * w.min(h) } else { v });
    let mut f = 1.0f32;
    for (a, b, len) in [(r[0], r[1], w), (r[1], r[2], h), (r[2], r[3], w), (r[3], r[0], h)] {
        if a + b > len && a + b > 0.0 {
            f = f.min(len / (a + b));
        }
    }
    if f < 1.0 {
        for v in r.iter_mut() {
            *v *= f;
        }
    }
    r
}

/// True when this side's style paints the pixel at `along` (distance along
/// the side from its start corner) and `across` (distance into the border
/// from the outer edge). `len` is the side's outer length: dashes and dots
/// are spread so the side starts AND ends on a mark (the corners join
/// solid), the gap stretched to fit — Chromium's distribution.
fn style_on(style: u8, w: f32, along: f32, across: f32, len: f32) -> bool {
    let fit = |mark: f32, gap: f32| -> bool {
        let n = ((len + gap) / (mark + gap)).round().max(1.0);
        let period = (len + gap) / n;
        along.rem_euclid(period) < period - gap
    };
    match style {
        1 => fit(2.0 * w, w),
        2 => {
            let n = ((len + w) / (2.0 * w)).round().max(1.0);
            let period = (len + w) / n;
            along.rem_euclid(period) < w
        }
        3 => {
            let t = (w / 3.0).max(1.0);
            across < t || across >= w - t
        }
        _ => true,
    }
}

/// Paints background and border of one box. Coordinates are screen floats
/// (the box may extend off-screen). `blend(x, y, colour, alpha)` writes a
/// pixel; it applies the damage and clip tests.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint(
    x0: f32,
    y0: f32,
    w: f32,
    h: f32,
    radii: [f32; 4],
    sides: [Side; 4],
    background: Option<(u8, u8, u8)>,
    screen_w: u32,
    screen_h: u32,
    blend: &mut dyn FnMut(u32, u32, (u8, u8, u8), f32),
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (x1, y1) = (x0 + w, y0 + h);
    let r = used_radii(radii, w, h);
    let bw = |i: usize| sides[i].map(|s| s.0).unwrap_or(0.0);
    let (bt, br, bb, bl) = (bw(0), bw(1), bw(2), bw(3));
    let (ix0, iy0, ix1, iy1) = (x0 + bl, y0 + bt, x1 - br, y1 - bb);
    let ir = [
        (r[0] - bl.max(bt)).max(0.0),
        (r[1] - br.max(bt)).max(0.0),
        (r[2] - br.max(bb)).max(0.0),
        (r[3] - bl.max(bb)).max(0.0),
    ];
    let has_inner = ix1 > ix0 && iy1 > iy0;
    let sx0 = x0.floor().max(0.0) as u32;
    let sy0 = y0.floor().max(0.0) as u32;
    let sx1 = (x1.ceil().max(0.0) as u32).min(screen_w);
    let sy1 = (y1.ceil().max(0.0) as u32).min(screen_h);
    for py in sy0..sy1 {
        for px in sx0..sx1 {
            let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
            let outer = coverage(sdf(fx, fy, x0, y0, x1, y1, r));
            if outer <= 0.0 {
                continue;
            }
            let inner = if has_inner { coverage(sdf(fx, fy, ix0, iy0, ix1, iy1, ir)) } else { 0.0 };
            if let Some(bg) = background {
                blend(px, py, bg, outer);
            }
            let ring = (outer - inner).max(0.0);
            if ring <= 0.0 {
                continue;
            }
            // Which side owns this pixel: smallest width-normalised distance.
            let d = [(fy - y0, bt), (x1 - fx, br), (y1 - fy, bb), (fx - x0, bl)];
            let mut best = None;
            let mut best_v = f32::MAX;
            for (i, &(dist, bw)) in d.iter().enumerate() {
                if bw <= 0.0 {
                    continue;
                }
                let v = dist / bw;
                if v < best_v {
                    best_v = v;
                    best = Some(i);
                }
            }
            let Some(i) = best else { continue };
            let Some((sw, color, style)) = sides[i] else { continue };
            let (along, across, len) = match i {
                0 => (px as f32 - x0, fy - y0, w),
                1 => (py as f32 - y0, x1 - fx, h),
                2 => (px as f32 - x0, y1 - fy, w),
                _ => (py as f32 - y0, fx - x0, h),
            };
            if style_on(style, sw.max(1.0), along, across, len) {
                blend(px, py, color, ring);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5.5: radii that overflow a side scale down together; % radii
    /// resolve against the box.
    #[test]
    fn used_radii_kat() {
        assert_eq!(used_radii([10.0; 4], 100.0, 50.0), [10.0; 4]);
        assert_eq!(used_radii([50.0; 4], 100.0, 50.0), [25.0; 4]);
        assert_eq!(used_radii([-0.5; 4], 40.0, 40.0), [20.0; 4]);
    }

    /// Chromium's mark distribution: a side starts and ends on a mark.
    #[test]
    fn dash_and_dot_fit_kat() {
        // 3px dotted over 45px: n = round(48/6) = 8, period 6.
        assert!(style_on(2, 3.0, 0.5, 1.0, 45.0));
        assert!(!style_on(2, 3.0, 3.5, 1.0, 45.0));
        assert!(style_on(2, 3.0, 44.5, 1.0, 45.0), "ends on a dot");
        // 3px dashed: dash 6, gap 3.
        assert!(style_on(1, 3.0, 5.5, 1.0, 744.0));
        assert!(!style_on(1, 3.0, 7.0, 1.0, 744.0));
        // double 6px: two 2px strokes.
        assert!(style_on(3, 6.0, 0.0, 1.0, 10.0) && !style_on(3, 6.0, 0.0, 3.0, 10.0) && style_on(3, 6.0, 0.0, 5.0, 10.0));
    }

    /// The painted ring: corner pixel outside a 10px radius is untouched,
    /// the straight edge is the border colour, the inside the background.
    #[test]
    fn rounded_ring_paint_kat() {
        let mut px = std::collections::HashMap::new();
        let side = Some((2.0, (255, 0, 0), 0));
        paint(0.0, 0.0, 40.0, 40.0, [10.0; 4], [side; 4], Some((0, 0, 255)), 40, 40, &mut |x, y, c, a| {
            px.insert((x, y), (c, a));
        });
        assert!(!px.contains_key(&(0, 0)), "corner outside the radius");
        assert_eq!(px[&(20, 0)], ((255, 0, 0), 1.0), "top edge stroke");
        assert_eq!(px[&(20, 20)], ((0, 0, 255), 1.0), "background inside");
    }
}
