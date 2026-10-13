//! CHARTER: Kernel — wm
//!
//! GLASSKIT (rmbp-ledger B628) — the glass's DRAWING KIT: anti-aliased rounded rectangles, strokes and hairlines,
//! soft drop shadows, dithered gradients, circles and lines, and the control set drawn with them. Every window
//! (Settings, the dock, the menu bar, the window chrome, the login screen) draws with this module instead of bare
//! `fill_rect` boxes with 1-px borders. The contract and the control catalogue: docs/dev/OS/05_USER_EXPERIENCE/GLASSKIT.md.
//!
//! * `no_std`, NO HEAP on any paint path (the fixture alone allocates its offscreen surface).
//! * Surfaces are the glass's `u32` `0x__RRGGBB` rows (stride `w`); the destination's top byte is preserved.
//! * Every op is clipped to the surface and to its [`Surf::clip`] rectangle; a shape may hang off either.
//! * Colour: an [`Ink`] is an `0x00RRGGBB` colour with a straight alpha (`255` = opaque). A bare `u32` converts
//!   as OPAQUE (the theme's roles carry a zero top byte); [`Ink::argb`] reads an `0xAARRGGBB` word's alpha.
//! * Geometry is in device px. The controls' default dimensions are logical (96-ppi) px in the ONE table [`DIMS`],
//!   scaled by `ui::px` at paint time; a dimension is named there once and nowhere else.
//! * Coverage, not supersampling: a corner pixel's coverage is its centre's signed distance to the arc in
//!   1/256 px, clamped to one pixel — the area estimate every vector rasteriser starts from.
//!
//! `tests glasskit` (R80: a verb, never at boot) draws every primitive and control into an offscreen 256x256 surface,
//! checks the pixel invariants and prints the per-op cost on the running machine:
//! `:: GLASSKIT: corner=ok aa=ok stroke=ok hairline=ok shadow=ok gradient=ok dither=ok circle=ok line=ok blend=ok controls=ok -> PASS ::`
//! `:: GLASSKIT: cost clock=<tsc|ms> fill_rrect_ns=<n> stroke_rrect_ns=<n> gradient_ns=<n> circle_ns=<n> line_ns=<n> controls_ns=<n> shadow_win_us=<n> shadow_budget_us=1000 ::`

use crate::video::text::{self, Face};
use crate::video::theme;

// ── geometry ────────────────────────────────────────────────────────────────────────────────────────────────────

/// A rectangle in device px. Signed: a shape may hang off the surface; every op clips.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }
    #[inline]
    pub const fn right(self) -> i32 {
        self.x + self.w
    }
    #[inline]
    pub const fn bottom(self) -> i32 {
        self.y + self.h
    }
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    /// Shrunk by `d` on every side (grown when `d` is negative); never negative in size.
    #[inline]
    pub fn inset(self, d: i32) -> Rect {
        Rect { x: self.x + d, y: self.y + d, w: (self.w - 2 * d).max(0), h: (self.h - 2 * d).max(0) }
    }
    /// Moved by `(dx, dy)`.
    #[inline]
    pub fn offset(self, dx: i32, dy: i32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..self }
    }
    /// The overlap (empty when none).
    pub fn intersect(self, o: Rect) -> Rect {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = (self.right().min(o.right()), self.bottom().min(o.bottom()));
        Rect { x: x0, y: y0, w: (x1 - x0).max(0), h: (y1 - y0).max(0) }
    }
    /// Does the rectangle hold the pixel `(x, y)`?
    #[inline]
    pub const fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// The four corner radii (top-left, top-right, bottom-right, bottom-left), in device px.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Radii {
    pub tl: i32,
    pub tr: i32,
    pub br: i32,
    pub bl: i32,
}

impl Radii {
    pub const NONE: Radii = Radii { tl: 0, tr: 0, br: 0, bl: 0 };
    pub const fn all(r: i32) -> Radii {
        Radii { tl: r, tr: r, br: r, bl: r }
    }
    pub const fn new(tl: i32, tr: i32, br: i32, bl: i32) -> Radii {
        Radii { tl, tr, br, bl }
    }
    /// Each radius held to `0 ..= min(w, h) / 2` of `r`, so the four arcs never overlap.
    pub fn fit(self, r: Rect) -> Radii {
        let m = (r.w.min(r.h) / 2).max(0);
        Radii { tl: self.tl.clamp(0, m), tr: self.tr.clamp(0, m), br: self.br.clamp(0, m), bl: self.bl.clamp(0, m) }
    }
    /// Every radius less `d` (the inner edge of a stroke `d` wide), never negative.
    pub fn shrink(self, d: i32) -> Radii {
        Radii { tl: (self.tl - d).max(0), tr: (self.tr - d).max(0), br: (self.br - d).max(0), bl: (self.bl - d).max(0) }
    }
}

impl From<i32> for Radii {
    fn from(r: i32) -> Radii {
        Radii::all(r)
    }
}

/// A colour and a straight alpha.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ink {
    pub rgb: u32,
    pub a: u32,
}

impl Ink {
    /// Opaque `0x00RRGGBB` (any top byte ignored).
    #[inline]
    pub const fn solid(c: u32) -> Ink {
        Ink { rgb: c & 0x00FF_FFFF, a: 255 }
    }
    /// `c` at alpha `a` (`0 ..= 255`).
    #[inline]
    pub const fn new(c: u32, a: u32) -> Ink {
        Ink { rgb: c & 0x00FF_FFFF, a: if a > 255 { 255 } else { a } }
    }
    /// A straight `0xAARRGGBB` word (the theme's alpha-bearing roles: hairline, shadow, focus ring).
    #[inline]
    pub const fn argb(c: u32) -> Ink {
        Ink { rgb: c & 0x00FF_FFFF, a: c >> 24 }
    }
    /// This ink with its alpha scaled by `k / 255` (a disabled control's fade).
    #[inline]
    pub const fn fade(self, k: u32) -> Ink {
        Ink { rgb: self.rgb, a: mul255(self.a, k) }
    }
}

impl From<u32> for Ink {
    #[inline]
    fn from(c: u32) -> Ink {
        Ink::solid(c)
    }
}

/// A destination: the glass's pixel rows, their stride and the rectangle every op is clipped to.
pub struct Surf<'a> {
    pub px: &'a mut [u32],
    pub w: usize,
    pub h: usize,
    pub clip: Rect,
}

impl<'a> Surf<'a> {
    /// The whole of `px` at stride `w` (rows = `len / w`).
    pub fn new(px: &'a mut [u32], w: usize) -> Surf<'a> {
        let h = if w == 0 { 0 } else { px.len() / w };
        Surf { px, w, h, clip: Rect::new(0, 0, w as i32, h as i32) }
    }
    /// Narrow the clip to `c` (never past the surface).
    pub fn set_clip(&mut self, c: Rect) {
        self.clip = Rect::new(0, 0, self.w as i32, self.h as i32).intersect(c);
    }
    /// Reopen the clip to the whole surface.
    pub fn reset_clip(&mut self) {
        self.clip = Rect::new(0, 0, self.w as i32, self.h as i32);
    }
    /// `r` clipped: `(x0, y0, x1, y1)`, empty when `x0 >= x1 || y0 >= y1`.
    #[inline]
    fn span(&self, r: Rect) -> (i32, i32, i32, i32) {
        let c = self.clip.intersect(r);
        (c.x, c.y, c.right(), c.bottom())
    }
    /// Row `y` (inside the clip, so inside the surface).
    #[inline]
    fn row(&mut self, y: i32) -> &mut [u32] {
        let o = y as usize * self.w;
        &mut self.px[o..o + self.w]
    }
    /// The pixel at `(x, y)` when it is inside the surface (a test probe; painters never read through this).
    pub fn get(&self, x: i32, y: i32) -> Option<u32> {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return None;
        }
        self.px.get(y as usize * self.w + x as usize).copied()
    }
}

// ── the two primitives the rest build on ────────────────────────────────────────────────────────────────────────

/// `a * b / 255`, rounded.
#[inline]
pub const fn mul255(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    (t + (t >> 8)) >> 8
}

/// `dst` moved `a / 255` of the way to `src`, per channel; `dst`'s top byte is kept.
#[inline]
pub fn blend(dst: u32, src: u32, a: u32) -> u32 {
    if a == 0 {
        return dst;
    }
    if a >= 255 {
        return (dst & 0xFF00_0000) | (src & 0x00FF_FFFF);
    }
    let a = a + (a >> 7); // 1..=254 -> 1..=255 of 256: exact at both ends
    let na = 256 - a;
    let rb = (((src & 0x00FF_00FF) * a + (dst & 0x00FF_00FF) * na) >> 8) & 0x00FF_00FF;
    let g = (((src & 0x0000_FF00) * a + (dst & 0x0000_FF00) * na) >> 8) & 0x0000_FF00;
    (dst & 0xFF00_0000) | rb | g
}

/// Porter-Duff OVER of a straight `0xAARRGGBB` source onto an opaque destination pixel.
#[inline]
pub fn over(dst_px: u32, src_px: u32) -> u32 {
    blend(dst_px, src_px, src_px >> 24)
}

/// Floor square root.
pub fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x = 1u64 << (64 - n.leading_zeros()).div_ceil(2);
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// Coverage (`0 ..= 255`) of pixel `(x, y)` by the disc of radius `r` centred on the pixel-grid point `(cx, cy)`
/// — the pixel centre's distance to the arc in 1/256 px, clamped to one pixel.
#[inline]
fn arc_cov(x: i32, y: i32, cx: i32, cy: i32, r: i32) -> u32 {
    let dx = ((x as i64) << 8) + 128 - ((cx as i64) << 8);
    let dy = ((y as i64) << 8) + 128 - ((cy as i64) << 8);
    let d2 = (dx * dx + dy * dy) as u64;
    let rq = (r as i64) << 8;
    let inner = (rq - 128).max(0) as u64;
    if d2 <= inner * inner {
        return 255;
    }
    let outer = (rq + 128) as u64;
    if d2 >= outer * outer {
        return 0;
    }
    let d = isqrt(d2) as i64;
    let c = (rq + 128 - d).clamp(0, 256) as u32;
    (c * 255 + 128) >> 8
}

/// Coverage of pixel `(x, y)` by the rounded rectangle `r` with (fitted) radii `rad`.
#[inline]
fn rrect_cov(r: Rect, rad: Radii, x: i32, y: i32) -> u32 {
    if !r.contains(x, y) {
        return 0;
    }
    let (lx, ty, rx, by) = (x - r.x, y - r.y, r.right() - 1 - x, r.bottom() - 1 - y);
    if lx < rad.tl && ty < rad.tl {
        arc_cov(x, y, r.x + rad.tl, r.y + rad.tl, rad.tl)
    } else if rx < rad.tr && ty < rad.tr {
        arc_cov(x, y, r.right() - rad.tr, r.y + rad.tr, rad.tr)
    } else if rx < rad.br && by < rad.br {
        arc_cov(x, y, r.right() - rad.br, r.bottom() - rad.br, rad.br)
    } else if lx < rad.bl && by < rad.bl {
        arc_cov(x, y, r.x + rad.bl, r.bottom() - rad.bl, rad.bl)
    } else {
        255
    }
}

/// THE rounded-rectangle row walker every filled shape rides: the corner pixels by coverage, the straight span
/// between them without a coverage test. `color(x, y)` names the pixel's colour (a constant, a gradient row).
fn rrect_paint(s: &mut Surf, r: Rect, rad: Radii, alpha: u32, mut color: impl FnMut(i32, i32) -> u32) {
    if r.is_empty() || alpha == 0 {
        return;
    }
    let rad = rad.fit(r);
    let (x0, y0, x1, y1) = s.span(r);
    for y in y0..y1 {
        let (ty, by) = (y - r.y, r.bottom() - 1 - y);
        let lr = if ty < rad.tl { rad.tl } else if by < rad.bl { rad.bl } else { 0 };
        let rr = if ty < rad.tr { rad.tr } else if by < rad.br { rad.br } else { 0 };
        let le = (r.x + lr).clamp(x0, x1);
        let rs = (r.right() - rr).clamp(le, x1);
        let row = s.row(y);
        for x in x0..le {
            let a = mul255(rrect_cov(r, rad, x, y), alpha);
            let p = &mut row[x as usize];
            *p = blend(*p, color(x, y), a);
        }
        if alpha >= 255 {
            for x in le..rs {
                let p = &mut row[x as usize];
                *p = (*p & 0xFF00_0000) | (color(x, y) & 0x00FF_FFFF);
            }
        } else {
            for x in le..rs {
                let p = &mut row[x as usize];
                *p = blend(*p, color(x, y), alpha);
            }
        }
        for x in rs..x1 {
            let a = mul255(rrect_cov(r, rad, x, y), alpha);
            let p = &mut row[x as usize];
            *p = blend(*p, color(x, y), a);
        }
    }
}

// ── (1) filled rounded rectangles ───────────────────────────────────────────────────────────────────────────────

/// An anti-aliased rounded rectangle (`radius`: one `i32` for all four corners, or a [`Radii`]).
pub fn fill_rrect(s: &mut Surf, r: Rect, radius: impl Into<Radii>, ink: impl Into<Ink>) {
    let ink = ink.into();
    rrect_paint(s, r, radius.into(), ink.a, |_, _| ink.rgb);
}

/// [`fill_rrect`] at a straight alpha `alpha` (`0 ..= 255`, multiplied into the ink's own).
pub fn fill_rrect_alpha(s: &mut Surf, r: Rect, radius: impl Into<Radii>, ink: impl Into<Ink>, alpha: u32) {
    let ink = ink.into();
    rrect_paint(s, r, radius.into(), mul255(ink.a, alpha.min(255)), |_, _| ink.rgb);
}

/// A square-cornered fill through the kit (clipped, alpha-aware).
pub fn fill(s: &mut Surf, r: Rect, ink: impl Into<Ink>) {
    let ink = ink.into();
    rrect_paint(s, r, Radii::NONE, ink.a, |_, _| ink.rgb);
}

// ── (2) strokes and hairlines ───────────────────────────────────────────────────────────────────────────────────

/// An anti-aliased stroke `width` px wide drawn INSIDE `r`'s edge (so a stroke and a fill of one rect align).
pub fn stroke_rrect(s: &mut Surf, r: Rect, radius: impl Into<Radii>, width: i32, ink: impl Into<Ink>) {
    let ink = ink.into();
    if r.is_empty() || width <= 0 || ink.a == 0 {
        return;
    }
    let rad = radius.into().fit(r);
    if width * 2 >= r.w.min(r.h) {
        rrect_paint(s, r, rad, ink.a, |_, _| ink.rgb);
        return;
    }
    if rad == Radii::NONE {
        // Square corners: four bands, no coverage test.
        fill(s, Rect::new(r.x, r.y, r.w, width), ink);
        fill(s, Rect::new(r.x, r.bottom() - width, r.w, width), ink);
        fill(s, Rect::new(r.x, r.y + width, width, r.h - 2 * width), ink);
        fill(s, Rect::new(r.right() - width, r.y + width, width, r.h - 2 * width), ink);
        return;
    }
    let inner = r.inset(width);
    let irad = rad.shrink(width).fit(inner);
    let (x0, y0, x1, y1) = s.span(r);
    let cov = |x: i32, y: i32| rrect_cov(r, rad, x, y).saturating_sub(rrect_cov(inner, irad, x, y));
    for y in y0..y1 {
        let (ty, by) = (y - r.y, r.bottom() - 1 - y);
        let row = s.row(y);
        if ty < width || by < width {
            for x in x0..x1 {
                let p = &mut row[x as usize];
                *p = blend(*p, ink.rgb, mul255(cov(x, y), ink.a));
            }
            continue;
        }
        let lr = if ty < rad.tl { rad.tl } else if by < rad.bl { rad.bl } else { 0 };
        let rr = if ty < rad.tr { rad.tr } else if by < rad.br { rad.br } else { 0 };
        let le = (r.x + lr.max(width)).clamp(x0, x1);
        let rs = (r.right() - rr.max(width)).clamp(le, x1);
        for x in (x0..le).chain(rs..x1) {
            let p = &mut row[x as usize];
            *p = blend(*p, ink.rgb, mul255(cov(x, y), ink.a));
        }
    }
}

/// A one-device-pixel horizontal line from `(x, y)`, `len` px long — the fast path of a 1 px stroke.
pub fn hairline_h(s: &mut Surf, x: i32, y: i32, len: i32, ink: impl Into<Ink>) {
    fill(s, Rect::new(x, y, len, 1), ink);
}

/// A one-device-pixel vertical line from `(x, y)`, `len` px long.
pub fn hairline_v(s: &mut Surf, x: i32, y: i32, len: i32, ink: impl Into<Ink>) {
    fill(s, Rect::new(x, y, 1, len), ink);
}

// ── (3) the soft drop shadow ────────────────────────────────────────────────────────────────────────────────────

/// The widest blur [`shadow_rrect`] draws (device px); a wider ask is held to it. Sizes the falloff table.
pub const SHADOW_BLUR_MAX: i32 = 96;
const SHADOW_LUT: usize = (SHADOW_BLUR_MAX * 8 + 1) as usize;

/// A soft drop shadow of the rounded rectangle `r` (uniform `radius`), `blur` px of falloff on each side of its
/// edge, ink `color_alpha` (`0xAARRGGBB`: the alpha is the shadow's peak opacity).
///
/// MECHANISM — an analytic falloff, not a convolution: the shadow's opacity at a pixel is a function of the pixel's
/// signed distance `sd` to the rounded rectangle (Q2, quarter px), `peak * (1 - smoothstep(-blur, +blur, sd))` —
/// the edge of a Gaussian-blurred box to within a few percent, without a buffer or a pass per axis. The falloff is
/// one table per call (≤ 769 bytes on the stack); straight edges cost a subtraction per pixel, corners one integer
/// square root per row and an incremental step per pixel. Half the peak sits on the edge itself.
///
/// CONTRACT — the caster covers its own footprint: pixels inside `r` deeper than `blur` are not touched (they are
/// under the window that casts the shadow, painted after it). An offset shadow is `r.offset(0, dy)` with
/// `dy <= blur`, so that skipped interior is still under the caster.
pub fn shadow_rrect(s: &mut Surf, r: Rect, radius: i32, blur: i32, color_alpha: u32) {
    let peak = color_alpha >> 24;
    let rgb = color_alpha & 0x00FF_FFFF;
    if r.is_empty() || peak == 0 {
        return;
    }
    let blur = blur.clamp(1, SHADOW_BLUR_MAX);
    let rad = radius.clamp(0, r.w.min(r.h) / 2);
    let n = 8 * blur; // the table spans sd = -blur ..= +blur in quarter px
    let half = 4 * blur;
    let mut lut = [0u8; SHADOW_LUT];
    for (i, e) in lut.iter_mut().enumerate().take(n as usize + 1) {
        let t = ((i as u64) << 16) / n as u64; // 0 ..= 1 in Q16
        let ss = (t * t >> 16) * (3 * 65536 - 2 * t) >> 16; // 3t^2 - 2t^3
        *e = ((peak as u64 * (65536 - ss.min(65536)) + 32768) >> 16) as u8;
    }
    // The straight-edge box (the rect less its radius), in quarter px.
    let (bx0, bx1, by0, by1) = (4 * (r.x + rad), 4 * (r.right() - rad), 4 * (r.y + rad), 4 * (r.bottom() - rad));
    let r4 = 4 * rad;
    let outer = r.inset(-blur);
    let deep = r.inset(blur);
    let (x0, y0, x1, y1) = s.span(outer);
    let a_of = |sd: i32| -> u32 {
        let i = sd + half;
        if i <= 0 {
            peak
        } else if i > n {
            0
        } else {
            lut[i as usize] as u32
        }
    };
    for y in y0..y1 {
        let py = 4 * y + 2;
        let qy = (by0 - py).max(py - by1);
        let (sk0, sk1) = if y >= deep.y && y < deep.bottom() { (deep.x.max(x0), deep.right().min(x1)) } else { (x1, x1) };
        let row = s.row(y);
        let mut put = |x: i32, a: u32| {
            if a != 0 {
                let p = &mut row[x as usize];
                *p = blend(*p, rgb, a);
            }
        };
        if qy > 0 {
            // A top or bottom row: the middle span is one opacity, the corners measure a distance.
            let am = a_of(qy - r4);
            let (cl, cr) = ((bx0 >> 2).clamp(x0, x1), (bx1 >> 2).clamp(x0, x1));
            for x in cl..cr {
                if x < sk0 || x >= sk1 {
                    put(x, am);
                }
            }
            let q2 = (qy as i64) * (qy as i64);
            // Left corner, walked right to left from the box edge so the root steps outward.
            let mut d: i64 = qy as i64;
            for x in (x0..cl).rev() {
                let qx = (bx0 - (4 * x + 2)) as i64;
                let t = qx * qx + q2;
                while (d + 1) * (d + 1) <= t {
                    d += 1;
                }
                while d * d > t {
                    d -= 1;
                }
                if x < sk0 || x >= sk1 {
                    put(x, a_of(d as i32 - r4));
                }
            }
            let mut d: i64 = qy as i64;
            for x in cr..x1 {
                let qx = ((4 * x + 2) - bx1) as i64;
                let t = qx * qx + q2;
                while (d + 1) * (d + 1) <= t {
                    d += 1;
                }
                while d * d > t {
                    d -= 1;
                }
                if x < sk0 || x >= sk1 {
                    put(x, a_of(d as i32 - r4));
                }
            }
        } else {
            // A straight row: the distance is the horizontal one (the vertical only inside the caster).
            for x in (x0..sk0.min(x1)).chain(sk1.max(x0).max(sk0.min(x1))..x1) {
                let px = 4 * x + 2;
                let qx = (bx0 - px).max(px - bx1);
                put(x, a_of(qx.max(qy) - r4));
            }
        }
    }
}

// ── (4) gradients, dithered ─────────────────────────────────────────────────────────────────────────────────────

/// The 4x4 ordered-dither matrix (Bayer), as fractions of one 8-bit step in 1/256: `(m * 16 + 8)`.
const BAYER: [u16; 16] = [8, 136, 40, 168, 200, 72, 232, 104, 56, 184, 24, 152, 248, 120, 216, 88];

/// One channel of the gradient at position `t / den`, in 1/256 of a step.
#[inline]
fn ramp(a: u32, b: u32, shift: u32, t: u32, den: u32) -> u32 {
    let (ca, cb) = (((a >> shift) & 0xFF) << 8, ((b >> shift) & 0xFF) << 8);
    if den == 0 {
        return ca;
    }
    if cb >= ca {
        ca + (cb - ca) * t / den
    } else {
        ca - (ca - cb) * t / den
    }
}

/// The dithered pixel of a ramp `(r8, g8, b8)` (each in 1/256 step) at `(x, y)`.
#[inline]
fn dithered(c: (u32, u32, u32), x: i32, y: i32) -> u32 {
    let d = BAYER[(((y & 3) << 2) | (x & 3)) as usize] as u32;
    let ch = |v: u32| ((v + d) >> 8).min(255);
    (ch(c.0) << 16) | (ch(c.1) << 8) | ch(c.2)
}

/// A vertical gradient filling `r`: `top` on its first row, `bottom` on its last, ordered-dithered between (no
/// banding on an 8-bit panel; the end rows are exact).
pub fn gradient_v(s: &mut Surf, r: Rect, top: u32, bottom: u32) {
    gradient_rrect_v(s, r, Radii::NONE, top, bottom);
}

/// A horizontal gradient filling `r`: `left` in its first column, `right` in its last, dithered.
pub fn gradient_h(s: &mut Surf, r: Rect, left: u32, right: u32) {
    if r.is_empty() {
        return;
    }
    let den = (r.w - 1).max(0) as u32;
    rrect_paint(s, r, Radii::NONE, 255, |x, y| {
        let t = (x - r.x) as u32;
        let c = (ramp(left, right, 16, t, den), ramp(left, right, 8, t, den), ramp(left, right, 0, t, den));
        if t == 0 || t == den { (c.0 >> 8 << 16) | (c.1 >> 8 << 8) | (c.2 >> 8) } else { dithered(c, x, y) }
    });
}

/// A rounded rectangle filled with a vertical dithered gradient (a button face, a title band).
pub fn gradient_rrect_v(s: &mut Surf, r: Rect, radius: impl Into<Radii>, top: u32, bottom: u32) {
    if r.is_empty() {
        return;
    }
    let den = (r.h - 1).max(0) as u32;
    let mut cur = (i32::MIN, (0, 0, 0));
    rrect_paint(s, r, radius.into(), 255, |x, y| {
        if cur.0 != y {
            let t = (y - r.y) as u32;
            cur = (y, (ramp(top, bottom, 16, t, den), ramp(top, bottom, 8, t, den), ramp(top, bottom, 0, t, den)));
        }
        let t = (y - r.y) as u32;
        let c = cur.1;
        if t == 0 || t == den { (c.0 >> 8 << 16) | (c.1 >> 8 << 8) | (c.2 >> 8) } else { dithered(c, x, y) }
    });
}

// ── (5) circles and lines — glyph-free icons ────────────────────────────────────────────────────────────────────

/// An anti-aliased disc filling the square `r` (its diameter is `min(w, h)`).
pub fn fill_disc(s: &mut Surf, r: Rect, ink: impl Into<Ink>) {
    let d = r.w.min(r.h);
    fill_rrect(s, Rect::new(r.x + (r.w - d) / 2, r.y + (r.h - d) / 2, d, d), d / 2, ink);
}

/// An anti-aliased disc of radius `radius` centred on the pixel-grid point `(cx, cy)`.
pub fn fill_circle(s: &mut Surf, cx: i32, cy: i32, radius: i32, ink: impl Into<Ink>) {
    fill_rrect(s, Rect::new(cx - radius, cy - radius, 2 * radius, 2 * radius), radius, ink);
}

/// An anti-aliased ring `width` px wide inside the circle of radius `radius` centred on `(cx, cy)`.
pub fn stroke_circle(s: &mut Surf, cx: i32, cy: i32, radius: i32, width: i32, ink: impl Into<Ink>) {
    stroke_rrect(s, Rect::new(cx - radius, cy - radius, 2 * radius, 2 * radius), radius, width, ink);
}

/// Sub-pixel units per device px for [`line_aa`]'s coordinates and width.
pub const SUB: i32 = 16;

/// An anti-aliased line with round caps from `(x0, y0)` to `(x1, y1)`, `width` wide — all in 1/[`SUB`] px
/// (`x * SUB + SUB / 2` is pixel `x`'s centre). The coverage is the pixel centre's distance to the segment.
pub fn line_aa(s: &mut Surf, x0: i32, y0: i32, x1: i32, y1: i32, width: i32, ink: impl Into<Ink>) {
    let ink = ink.into();
    if width <= 0 || ink.a == 0 {
        return;
    }
    let hw = width as i64 * 16 / 2; // half width, in 1/256 px
    let pad = width / 2 / SUB + 2;
    let bb = Rect::new(x0.min(x1) / SUB - pad, y0.min(y1) / SUB - pad, (x0 - x1).abs() / SUB + 2 * pad + 1, (y0 - y1).abs() / SUB + 2 * pad + 1);
    let (ax, ay, bx, by) = (x0 as i64 * 16, y0 as i64 * 16, x1 as i64 * 16, y1 as i64 * 16);
    let (vx, vy) = (bx - ax, by - ay);
    let len2 = vx * vx + vy * vy;
    let (cx0, cy0, cx1, cy1) = s.span(bb);
    for y in cy0..cy1 {
        let py = ((y as i64) << 8) + 128;
        let row = s.row(y);
        for x in cx0..cx1 {
            let px = ((x as i64) << 8) + 128;
            let (wx, wy) = (px - ax, py - ay);
            let t = wx * vx + wy * vy;
            let d2 = if len2 == 0 || t <= 0 {
                wx * wx + wy * wy
            } else if t >= len2 {
                let (ex, ey) = (px - bx, py - by);
                ex * ex + ey * ey
            } else {
                let c = (wx * vy - wy * vx) as i128;
                (c * c / len2 as i128) as i64
            };
            let d = isqrt(d2 as u64) as i64;
            let c = (hw + 128 - d).clamp(0, 256) as u32;
            if c != 0 {
                let p = &mut row[x as usize];
                *p = blend(*p, ink.rgb, mul255((c * 255 + 128) >> 8, ink.a));
            }
        }
    }
}

// ── the dock's proof seam: round a span another painter already filled square ──────────────────────────────────

/// The widest corner [`row_keep`] can hold (device px).
pub const KEEP_MAX: usize = 64;

/// The pixels a square fill is about to cover at the two ends of one row span, kept so [`row_round`] can put the
/// rounded corners back over them. Stack only.
pub struct RowKeep {
    l: [u32; KEEP_MAX],
    r: [u32; KEEP_MAX],
    n: usize,
}

/// Keep the `radius` pixels at each end of `row[x .. x + w]` (a strip painter's row, before the tile is filled).
pub fn row_keep(row: &[u32], x: usize, w: usize, radius: usize) -> RowKeep {
    let n = radius.min(w / 2).min(KEEP_MAX);
    let mut k = RowKeep { l: [0; KEEP_MAX], r: [0; KEEP_MAX], n };
    if x + w > row.len() {
        k.n = 0;
        return k;
    }
    k.l[..n].copy_from_slice(&row[x..x + n]);
    k.r[..n].copy_from_slice(&row[x + w - n..x + w]);
    k
}

/// Row `y` of a `w` x `h` tile at `row[x ..]` was filled square: blend its four corners back to what [`row_keep`]
/// kept, by the coverage of a rounded rectangle of `radius` — the tile is anti-aliased and rounded, and whatever the
/// fill drew inside it (a glyph) is untouched.
pub fn row_round(row: &mut [u32], k: &RowKeep, x: usize, w: usize, h: usize, y: usize, radius: usize) {
    if k.n == 0 || x + w > row.len() || y >= h {
        return;
    }
    let r = Rect::new(0, 0, w as i32, h as i32);
    let rad = Radii::all(radius as i32).fit(r);
    for i in 0..k.n {
        let c = rrect_cov(r, rad, i as i32, y as i32);
        row[x + i] = blend(k.l[i], row[x + i], c);
        let j = w - k.n + i;
        let c = rrect_cov(r, rad, j as i32, y as i32);
        row[x + j] = blend(k.r[i], row[x + j], c);
    }
}

/// An icon tile's corner radius for a tile `side` px tall.
#[inline]
pub fn icon_radius(side: usize) -> usize {
    side * DIMS.icon_radius_pct / 100
}

// ── (7) the control set ─────────────────────────────────────────────────────────────────────────────────────────

/// A control's interaction state. `NORMAL`, `HOVER`, `PRESSED`, `DISABLED`, `FOCUSED`; focus combines with the
/// others (`ControlState::HOVER.focus(true)`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ControlState {
    pub hover: bool,
    pub pressed: bool,
    pub disabled: bool,
    pub focused: bool,
}

impl ControlState {
    pub const NORMAL: ControlState = ControlState { hover: false, pressed: false, disabled: false, focused: false };
    pub const HOVER: ControlState = ControlState { hover: true, ..Self::NORMAL };
    pub const PRESSED: ControlState = ControlState { pressed: true, ..Self::NORMAL };
    pub const DISABLED: ControlState = ControlState { disabled: true, ..Self::NORMAL };
    pub const FOCUSED: ControlState = ControlState { focused: true, ..Self::NORMAL };
    pub const fn focus(self, on: bool) -> ControlState {
        ControlState { focused: on, ..self }
    }
    /// The whole control's opacity (a disabled control is faded, never re-coloured).
    #[inline]
    fn alpha(self) -> u32 {
        if self.disabled { DIMS.disabled_alpha } else { 255 }
    }
    /// A face nudged for hover / press (toward the ink: darker on Light, lighter on Dark).
    fn face(self, c: u32) -> u32 {
        if self.disabled {
            c
        } else if self.pressed {
            blend(c, theme::text_primary(), DIMS.press_tint)
        } else if self.hover {
            blend(c, theme::text_primary(), DIMS.hover_tint)
        } else {
            c
        }
    }
}

/// The controls' default dimensions — logical (96-ppi) px, scaled by `ui::px` when painted, except where a field
/// says device px or percent. NAMED ONCE: GLASSKIT.md's catalogue is this table; a window that wants a different
/// size passes geometry, it never re-spells these.
pub struct Dims {
    /// The layout grid every control's outer size sits on.
    pub grid: usize,
    pub toggle_w: usize,
    pub toggle_h: usize,
    /// The knob's inset from the toggle's track.
    pub toggle_inset: usize,
    pub check: usize,
    pub check_radius: usize,
    /// The check mark's stroke.
    pub check_stroke: usize,
    pub radio: usize,
    pub radio_dot: usize,
    pub slider_track: usize,
    pub slider_knob: usize,
    pub button_h: usize,
    pub button_radius: usize,
    pub button_pad_x: usize,
    pub button_min_w: usize,
    pub seg_h: usize,
    pub seg_radius: usize,
    pub seg_pad_x: usize,
    /// The selected segment's inset inside the segmented control's well.
    pub seg_inset: usize,
    pub field_h: usize,
    pub field_radius: usize,
    pub field_pad_x: usize,
    /// The caret's width.
    pub caret_w: usize,
    /// The focus ring's width, drawn OUTSIDE the control.
    pub focus_ring: usize,
    pub row_h: usize,
    pub row_radius: usize,
    pub row_pad_x: usize,
    pub group_radius: usize,
    pub group_pad: usize,
    /// The gap between a group box's title and its box.
    pub group_title_gap: usize,
    /// A knob's (toggle, slider, radio) soft shadow blur and its drop.
    pub knob_blur: usize,
    pub knob_drop: usize,
    /// A push button's shadow blur and drop.
    pub button_blur: usize,
    pub button_drop: usize,
    /// The hairline's width in DEVICE px (one at every scale — a hairline is the panel's finest line).
    pub hairline_dev: usize,
    /// An icon tile's corner radius, percent of its side.
    pub icon_radius_pct: usize,
    /// A disabled control's opacity (of 255).
    pub disabled_alpha: u32,
    /// Hover / press tint toward the ink (of 255).
    pub hover_tint: u32,
    pub press_tint: u32,
}

/// THE table. Proposed defaults on the 8 px grid; the DESIGN-LANGUAGE survey aligns the values here, once.
pub const DIMS: Dims = Dims {
    grid: 8,
    toggle_w: 32,
    toggle_h: 18,
    toggle_inset: 2,
    check: 16,
    check_radius: 4,
    check_stroke: 2,
    radio: 16,
    radio_dot: 6,
    slider_track: 4,
    slider_knob: 18,
    button_h: 24,
    button_radius: 6,
    button_pad_x: 14,
    button_min_w: 64,
    seg_h: 24,
    seg_radius: 6,
    seg_pad_x: 12,
    seg_inset: 2,
    field_h: 24,
    field_radius: 6,
    field_pad_x: 8,
    caret_w: 1,
    focus_ring: 3,
    row_h: 28,
    row_radius: 6,
    row_pad_x: 10,
    group_radius: 10,
    group_pad: 12,
    group_title_gap: 6,
    knob_blur: 3,
    knob_drop: 1,
    button_blur: 2,
    button_drop: 1,
    hairline_dev: 1,
    icon_radius_pct: 22,
    disabled_alpha: 110,
    hover_tint: 10,
    press_tint: 28,
};

/// A logical length at the panel's scale, in device px.
#[inline]
fn d(n: usize) -> i32 {
    crate::ui::px(n) as i32
}

/// The face every control's label draws with.
const LABEL_FACE: Face = Face::Ui;

/// Draw `s` with its line top at `(x, y)`, clipped to the surface's clip; returns the pen x.
fn label(sf: &mut Surf, x: i32, y: i32, s: &[u8], ink: u32, bold: bool) -> i32 {
    let c = sf.clip;
    if c.is_empty() || x < c.x || y < c.y || x >= c.right() {
        return x;
    }
    text::draw_text(sf.px, sf.w, c.right() as usize, c.bottom() as usize, x as usize, y as usize, s, ink, bold, LABEL_FACE) as i32
}

/// `s`'s width in the label face.
fn label_w(s: &[u8], bold: bool) -> i32 {
    text::advance(s, bold, LABEL_FACE) as i32
}

/// The label's top for a line centred in `r`.
fn label_top(r: Rect) -> i32 {
    r.y + (r.h - LABEL_FACE.cell_h() as i32) / 2
}

/// `ink` faded toward `bg` by the state's alpha (text is drawn opaque; a disabled label is a lighter ink).
fn label_ink(ink: u32, bg: u32, st: ControlState) -> u32 {
    blend(bg, ink, st.alpha())
}

/// The focus ring OUTSIDE `r` (radius `rad` is `r`'s own).
fn focus_ring(s: &mut Surf, r: Rect, rad: i32, st: ControlState) {
    if st.focused && !st.disabled {
        let w = d(DIMS.focus_ring);
        stroke_rrect(s, r.inset(-w), rad + w, w, Ink::argb(theme::focus_ring()));
    }
}

/// A knob: its soft shadow, its face, its hairline edge.
fn knob(s: &mut Surf, r: Rect, st: ControlState) {
    let a = st.alpha();
    let sh = Ink::argb(theme::shadow_soft()).fade(a);
    shadow_rrect(s, r.offset(0, d(DIMS.knob_drop)), r.w / 2, d(DIMS.knob_blur), sh.rgb | (sh.a << 24));
    fill_rrect_alpha(s, r, r.w.min(r.h) / 2, st.face(theme::knob_face()), a);
    stroke_rrect(s, r, r.w.min(r.h) / 2, DIMS.hairline_dev as i32, Ink::argb(theme::control_edge()).fade(a));
}

/// A switch at `(x, y)`: the track (the accent when `on`), the knob at the end it is set to. Returns its rect.
pub fn toggle(s: &mut Surf, x: i32, y: i32, on: bool, st: ControlState) -> Rect {
    toggle_at(s, x, y, if on { motion::ONE } else { 0 }, st)
}

/// MOTIONKIT (B648): [`toggle`] with its knob `pos` of the way along its travel (Q16: 0 the off end,
/// [`motion::ONE`] the on end) — one frame of a slide. The track's colour and its off hairline follow the knob, so
/// the face reads as one thing moving, not a knob over a switched track. At 0 and `ONE` it is [`toggle`] exactly.
pub fn toggle_at(s: &mut Surf, x: i32, y: i32, pos: u32, st: ControlState) -> Rect {
    let r = Rect::new(x, y, d(DIMS.toggle_w), d(DIMS.toggle_h));
    let (rad, a) = (r.h / 2, st.alpha());
    let on = pos.min(motion::ONE) * 255 / motion::ONE;
    let track = st.face(if on == 255 { theme::accent() } else if on == 0 { theme::control_track() } else { blend(theme::control_track(), theme::accent(), on) });
    fill_rrect_alpha(s, r, rad, track, a);
    if on < 255 {
        stroke_rrect(s, r, rad, DIMS.hairline_dev as i32, Ink::argb(theme::hairline()).fade(mul255(a, 255 - on)));
    }
    let i = d(DIMS.toggle_inset);
    let k = r.h - 2 * i;
    let kx = motion::lerp(r.x + i, r.right() - i - k, pos);
    knob(s, Rect::new(kx, r.y + i, k, k), ControlState { hover: false, pressed: false, ..st });
    focus_ring(s, r, rad, st);
    r
}

/// A check box at `(x, y)`; `checked` fills it with the accent and draws the mark as two strokes.
pub fn checkbox(s: &mut Surf, x: i32, y: i32, checked: bool, st: ControlState) -> Rect {
    let b = d(DIMS.check);
    let r = Rect::new(x, y, b, b);
    let (rad, a) = (d(DIMS.check_radius), st.alpha());
    if checked {
        fill_rrect_alpha(s, r, rad, st.face(theme::accent()), a);
        let (q, sw) = (b * SUB, d(DIMS.check_stroke) * SUB);
        let p = |fx: i32, fy: i32| (x * SUB + q * fx / 100, y * SUB + q * fy / 100);
        let ((ax, ay), (mx, my), (ex, ey)) = (p(26, 52), p(43, 70), p(75, 32));
        let ink = Ink::new(theme::ink_on_accent(), a);
        line_aa(s, ax, ay, mx, my, sw, ink);
        line_aa(s, mx, my, ex, ey, sw, ink);
    } else {
        fill_rrect_alpha(s, r, rad, st.face(theme::knob_face()), a);
        stroke_rrect(s, r, rad, DIMS.hairline_dev as i32, Ink::argb(theme::control_edge()).fade(a));
    }
    focus_ring(s, r, rad, st);
    r
}

/// A radio button at `(x, y)`; `selected` fills it with the accent around a light dot.
pub fn radio(s: &mut Surf, x: i32, y: i32, selected: bool, st: ControlState) -> Rect {
    let b = d(DIMS.radio);
    let r = Rect::new(x, y, b, b);
    let a = st.alpha();
    if selected {
        fill_disc(s, r, Ink::new(st.face(theme::accent()), a));
        let dd = d(DIMS.radio_dot);
        fill_disc(s, Rect::new(x + (b - dd) / 2, y + (b - dd) / 2, dd, dd), Ink::new(theme::ink_on_accent(), a));
    } else {
        fill_disc(s, r, Ink::new(st.face(theme::knob_face()), a));
        stroke_rrect(s, r, b / 2, DIMS.hairline_dev as i32, Ink::argb(theme::control_edge()).fade(a));
    }
    focus_ring(s, r, b / 2, st);
    r
}

/// A horizontal slider `w` px wide at `(x, y)`: the track, the accent up to `value / max`, the knob there.
pub fn slider(s: &mut Surf, x: i32, y: i32, w: i32, value: u32, max: u32, st: ControlState) -> Rect {
    let kd = d(DIMS.slider_knob);
    let r = Rect::new(x, y, w.max(kd), kd);
    let th = d(DIMS.slider_track);
    let a = st.alpha();
    let track = Rect::new(r.x, r.y + (kd - th) / 2, r.w, th);
    fill_rrect_alpha(s, track, th / 2, theme::control_track(), a);
    let span = (r.w - kd).max(0) as i64;
    let v = if max == 0 { 0 } else { (span * value.min(max) as i64 / max as i64) as i32 };
    fill_rrect_alpha(s, Rect::new(track.x, track.y, v + kd / 2, th), th / 2, theme::accent(), a);
    let kr = Rect::new(r.x + v, r.y, kd, kd);
    knob(s, kr, st);
    focus_ring(s, kr, kd / 2, st);
    r
}

/// A push button at `(x, y)`, `w` wide (0 = fit the label). `default` is the window's default action: the accent
/// face. Returns its rect.
pub fn push_button(s: &mut Surf, x: i32, y: i32, w: i32, text: &[u8], default: bool, st: ControlState) -> Rect {
    let lw = label_w(text, false);
    let w = if w > 0 { w } else { (lw + 2 * d(DIMS.button_pad_x)).max(d(DIMS.button_min_w)) };
    let r = Rect::new(x, y, w, d(DIMS.button_h));
    let (rad, a) = (d(DIMS.button_radius), st.alpha());
    if !st.disabled {
        shadow_rrect(s, r.offset(0, d(DIMS.button_drop)), rad, d(DIMS.button_blur), theme::shadow_soft());
    }
    let face = st.face(if default { theme::accent() } else { theme::knob_face() });
    fill_rrect_alpha(s, r, rad, face, a);
    if !default {
        stroke_rrect(s, r, rad, DIMS.hairline_dev as i32, Ink::argb(theme::control_edge()).fade(a));
    }
    let ink = if default { theme::ink_on_accent() } else { theme::text_primary() };
    label(s, r.x + (r.w - lw) / 2, label_top(r), text, label_ink(ink, face, st), false);
    focus_ring(s, r, rad, st);
    r
}

/// A segmented control at `(x, y)`: one equal-width segment per label, `sel` raised on a knob-coloured pill.
pub fn segmented(s: &mut Surf, x: i32, y: i32, labels: &[&[u8]], sel: usize, st: ControlState) -> Rect {
    let n = labels.len().max(1) as i32;
    let sw = labels.iter().map(|l| label_w(l, false)).max().unwrap_or(0) + 2 * d(DIMS.seg_pad_x);
    segmented_w(s, x, y, sw * n, labels, sel, st)
}

/// SETTINGSCRAFT (B630): [`segmented`] `w` device px wide, split into equal segments — for a window whose press
/// router already maps `x / (w / n)` to a segment, so the painted segments and the hit segments are one geometry.
pub fn segmented_w(s: &mut Surf, x: i32, y: i32, w: i32, labels: &[&[u8]], sel: usize, st: ControlState) -> Rect {
    let sw = w / labels.len().max(1) as i32;
    segmented_at(s, x, y, w, labels, sel, x + sw * sel.min(labels.len().saturating_sub(1)) as i32, st)
}

/// MOTIONKIT (B648): [`segmented_w`] with its thumb's left edge at `thumb_x` (device px, anywhere along the well)
/// — one frame of a slide. `sel` still names the segment whose label is raised; the hairlines the thumb covers are
/// not drawn. With `thumb_x` at segment `sel`'s left edge it is [`segmented_w`] exactly.
#[allow(clippy::too_many_arguments)]
pub fn segmented_at(s: &mut Surf, x: i32, y: i32, w: i32, labels: &[&[u8]], sel: usize, thumb_x: i32, st: ControlState) -> Rect {
    let n = labels.len().max(1) as i32;
    let sw = w / n;
    let r = Rect::new(x, y, sw * n, d(DIMS.seg_h));
    let (rad, a, i) = (d(DIMS.seg_radius), st.alpha(), d(DIMS.seg_inset));
    let well = theme::control_track();
    fill_rrect_alpha(s, r, rad, well, a);
    let thumb = Rect::new(thumb_x.clamp(r.x, r.right() - sw), r.y, sw, r.h);
    for k in 1..n {
        let hx = r.x + sw * k;
        if hx >= thumb.x && hx <= thumb.right() { continue; }
        hairline_v(s, hx, r.y + 2 * i, r.h - 4 * i, Ink::argb(theme::control_edge()).fade(a));
    }
    let pill = thumb.inset(i);
    if !st.disabled {
        shadow_rrect(s, pill.offset(0, d(DIMS.knob_drop)), rad - i, d(DIMS.button_blur), theme::shadow_soft());
    }
    fill_rrect_alpha(s, pill, rad - i, st.face(theme::knob_face()), a);
    for (k, l) in labels.iter().enumerate() {
        let seg = Rect::new(r.x + sw * k as i32, r.y, sw, r.h);
        let on = k == sel;
        let bg = if on { theme::knob_face() } else { well };
        label(s, seg.x + (sw - label_w(l, on)) / 2, label_top(seg), l, label_ink(theme::text_primary(), bg, st), on);
    }
    focus_ring(s, r, rad, st);
    r
}

/// A text field at `(x, y)`, `w` wide, showing `text`; focused (`st.focused`) it draws the ring and the caret.
pub fn text_field(s: &mut Surf, x: i32, y: i32, w: i32, text: &[u8], st: ControlState) -> Rect {
    let r = Rect::new(x, y, w, d(DIMS.field_h));
    let (rad, a) = (d(DIMS.field_radius), st.alpha());
    let bg = theme::surface_content();
    fill_rrect_alpha(s, r, rad, bg, a);
    stroke_rrect(s, r, rad, DIMS.hairline_dev as i32, Ink::argb(theme::control_edge()).fade(a));
    let inner = r.inset(d(DIMS.field_pad_x) / 2);
    let saved = s.clip;
    s.set_clip(saved.intersect(Rect::new(r.x + d(DIMS.field_pad_x), r.y, r.w - 2 * d(DIMS.field_pad_x), r.h)));
    let pen = label(s, r.x + d(DIMS.field_pad_x), label_top(r), text, label_ink(theme::text_primary(), bg, st), false);
    s.clip = saved;
    if st.focused && !st.disabled {
        let ch = LABEL_FACE.cell_h() as i32;
        let cx = pen.min(inner.right() - 1);
        fill(s, Rect::new(cx, label_top(r), d(DIMS.caret_w), ch), theme::text_primary());
    }
    focus_ring(s, r, rad, st);
    r
}

/// A source-list row at `(x, y)`, `w` wide: `selected` is an accent pill with light ink; hover a faint wash.
pub fn sidebar_row(s: &mut Surf, x: i32, y: i32, w: i32, text: &[u8], selected: bool, st: ControlState) -> Rect {
    let r = Rect::new(x, y, w, d(DIMS.row_h));
    let rad = d(DIMS.row_radius);
    let base = theme::surface_window();
    let (ink, bg) = if selected {
        let f = st.face(theme::accent());
        fill_rrect_alpha(s, r, rad, f, st.alpha());
        (theme::ink_on_accent(), f)
    } else if st.hover || st.pressed {
        fill_rrect(s, r, rad, Ink::argb(theme::hairline()));
        (theme::text_primary(), base)
    } else {
        (theme::text_primary(), base)
    };
    let saved = s.clip;
    s.set_clip(saved.intersect(r.inset(1)));
    label(s, r.x + d(DIMS.row_pad_x), label_top(r), text, label_ink(ink, bg, st), selected);
    s.clip = saved;
    focus_ring(s, r, rad, st);
    r
}

/// An inset group box filling `r`: its `title` above it in the secondary ink, the box one step lighter than the
/// window with a hairline edge. Returns the content rectangle (the box less its padding).
pub fn group_box(s: &mut Surf, r: Rect, title: &[u8]) -> Rect {
    let mut b = r;
    if !title.is_empty() {
        let th = LABEL_FACE.cell_h() as i32 + d(DIMS.group_title_gap);
        label(s, r.x + d(DIMS.group_pad) / 2, r.y, title, theme::ink_secondary(), true);
        b = Rect::new(r.x, r.y + th, r.w, (r.h - th).max(0));
    }
    let rad = d(DIMS.group_radius);
    fill_rrect(s, b, rad, theme::group_fill());
    stroke_rrect(s, b, rad, DIMS.hairline_dev as i32, Ink::argb(theme::hairline()));
    b.inset(d(DIMS.group_pad))
}

// ── (8) `tests glasskit` ────────────────────────────────────────────────────────────────────────────────────────

/// Register `tests glasskit` (from the tests registry's settings block).
pub fn selftest() {
    let (ok, why) = check();
    serial_println!(
        ":: GLASSKIT: corner={} aa={} stroke={} hairline={} shadow={} gradient={} dither={} circle={} line={} blend={} controls={} -> {} ::",
        f(why & 1), f(why & 2), f(why & 4), f(why & 8), f(why & 16), f(why & 32), f(why & 64), f(why & 128), f(why & 256), f(why & 512), f(why & 1024),
        if ok { "PASS" } else { "FAIL" }
    );
    cost();
}

fn f(bad: u32) -> &'static str {
    if bad == 0 { "ok" } else { "FAIL" }
}

/// The fixture's side.
const T: usize = 256;

/// Run every invariant; `(all good, bitmask of the failed ones)`.
fn check() -> (bool, u32) {
    let mut buf = alloc::vec::Vec::new();
    if buf.try_reserve_exact(T * T).is_err() {
        return (false, u32::MAX);
    }
    buf.resize(T * T, 0u32);
    let bg = theme::surface_window();
    let fg = theme::accent();
    let ink = theme::text_primary();
    let mut bad = 0u32;
    let mut s = Surf::new(&mut buf, T);
    let clear = |s: &mut Surf| s.px.fill(bg);
    let rgb = |p: Option<u32>| p.map(|v| v & 0x00FF_FFFF);
    let mid = |p: Option<u32>, a: u32, b: u32| p.is_some_and(|v| (v & 0x00FF_FFFF) != (a & 0x00FF_FFFF) && (v & 0x00FF_FFFF) != (b & 0x00FF_FFFF));

    // 1. corner pixels are the background; the body is the ink; 2. the arc has intermediate values.
    clear(&mut s);
    let r = Rect::new(16, 16, 96, 64);
    fill_rrect(&mut s, r, 16, fg);
    if rgb(s.get(16, 16)) != Some(bg) || rgb(s.get(111, 79)) != Some(bg) || rgb(s.get(64, 48)) != Some(fg) || rgb(s.get(16, 48)) != Some(fg) {
        bad |= 1;
    }
    if !(0..16).any(|i| mid(s.get(16 + i, 16 + i), bg, fg)) || !(0..16).any(|i| mid(s.get(111 - i, 79 - i), bg, fg)) {
        bad |= 2;
    }
    // 3. a stroke: a middle row holds exactly 2 x width inked pixels (each the full ink), the top band is full.
    for wdt in [1, 3] {
        clear(&mut s);
        stroke_rrect(&mut s, r, 12, wdt, ink);
        let row = (r.x..r.right()).filter(|&x| rgb(s.get(x, 48)) != Some(bg)).count();
        let full = (r.x..r.right()).filter(|&x| rgb(s.get(x, 48)) == Some(ink)).count();
        let top = (r.x + 16..r.right() - 16).all(|x| rgb(s.get(x, r.y)) == Some(ink));
        if row != 2 * wdt as usize || full != row || !top || rgb(s.get(64, 48)) != Some(bg) {
            bad |= 4;
        }
    }
    // 4. hairlines: one row / one column, `len` px, nothing beside them.
    clear(&mut s);
    hairline_h(&mut s, 10, 200, 50, ink);
    hairline_v(&mut s, 100, 150, 40, ink);
    let hn = (0..T as i32).filter(|&x| rgb(s.get(x, 200)) == Some(ink)).count();
    let vn = (0..T as i32).filter(|&y| rgb(s.get(100, y)) == Some(ink)).count();
    if hn != 50 || vn != 40 || rgb(s.get(30, 199)) != Some(bg) || rgb(s.get(30, 201)) != Some(bg) {
        bad |= 8;
    }
    // 5. the shadow: darker next to the edge, monotone lighter away from it, the background past the blur.
    clear(&mut s);
    let sr = Rect::new(64, 64, 96, 64);
    let blur = 24;
    shadow_rrect(&mut s, sr, 8, blur, (theme::shadow_soft() & 0x00FF_FFFF) | (160 << 24));
    let g = |p: Option<u32>| p.map(|v| (v >> 8) & 0xFF).unwrap_or(0);
    let mut prev = 0;
    let mut mono = true;
    for x in sr.right() - 4..sr.right() + blur + 2 {
        let v = g(s.get(x, 96));
        if v < prev {
            mono = false;
        }
        prev = v;
    }
    let mut prevy = 0;
    for y in (sr.bottom() - 4..sr.bottom() + blur + 2).map(|y| g(s.get(112, y))) {
        if y < prevy {
            mono = false;
        }
        prevy = y;
    }
    let corner_mono = (0..blur).all(|k| g(s.get(sr.right() + k, sr.bottom() + k)) <= g(s.get(sr.right() + k + 1, sr.bottom() + k + 1)));
    if !mono || !corner_mono || rgb(s.get(sr.right() + blur + 1, 96)) != Some(bg) || g(s.get(sr.right(), 96)) >= (bg >> 8) & 0xFF {
        bad |= 16;
    }
    // 6. a vertical gradient's end rows are exact; 7. the middle is dithered (a row holds two values).
    clear(&mut s);
    let gr = Rect::new(0, 0, T as i32, T as i32);
    gradient_v(&mut s, gr, fg, bg);
    if (0..T as i32).any(|x| rgb(s.get(x, 0)) != Some(fg) || rgb(s.get(x, T as i32 - 1)) != Some(bg)) {
        bad |= 32;
    }
    if !(1..T as i32 - 1).any(|y| (1..8).any(|x| rgb(s.get(x, y)) != rgb(s.get(0, y)))) {
        bad |= 64;
    }
    gradient_h(&mut s, gr, bg, fg);
    if rgb(s.get(0, 77)) != Some(bg) || rgb(s.get(T as i32 - 1, 77)) != Some(fg) {
        bad |= 32;
    }
    // 8. a circle: centre inked, bounding-box corner background, an anti-aliased rim.
    clear(&mut s);
    fill_circle(&mut s, 128, 128, 40, fg);
    if rgb(s.get(128, 128)) != Some(fg) || rgb(s.get(89, 89)) != Some(bg) || !(0..40).any(|i| mid(s.get(128 + i, 128 + i), bg, fg)) {
        bad |= 128;
    }
    stroke_circle(&mut s, 128, 128, 60, 2, ink);
    if rgb(s.get(128, 128)) != Some(fg) || rgb(s.get(128, 68)) != Some(ink) {
        bad |= 128;
    }
    // 9. a line: inked on its axis, background off it, intermediate at its edge.
    clear(&mut s);
    line_aa(&mut s, 20 * SUB + SUB / 2, 20 * SUB + SUB / 2, 220 * SUB + SUB / 2, 120 * SUB + SUB / 2, 3 * SUB, ink);
    if rgb(s.get(120, 70)) != Some(ink) || rgb(s.get(120, 90)) != Some(bg) || !(78..94).any(|y| mid(s.get(150, y), bg, ink)) {
        bad |= 256;
    }
    // 10. the primitives: alpha 0 keeps dst, 255 is src, half is the midpoint, OVER reads the top byte, the
    // destination's top byte survives.
    let half = blend(bg, fg, 128);
    let ch = |v: u32, sh: u32| (v >> sh) & 0xFF;
    let halfway = [16u32, 8, 0].iter().all(|&sh| ch(half, sh).abs_diff((ch(bg, sh) + ch(fg, sh)) / 2) <= 1);
    if blend(bg, fg, 0) != bg
        || blend(bg, fg, 255) != fg
        || !halfway
        || over(bg, fg) != bg
        || over(bg, fg | (255 << 24)) != fg
        || blend(bg | (255 << 24), fg, 255) >> 24 != 255
    {
        bad |= 512;
    }
    // 11. every control paints (each changes pixels) in every state, inside its returned rect's neighbourhood.
    clear(&mut s);
    let sts = [ControlState::NORMAL, ControlState::HOVER, ControlState::PRESSED, ControlState::DISABLED, ControlState::FOCUSED];
    let mut ctl_ok = true;
    for st in sts {
        let mut probe = |s: &mut Surf, f: &dyn Fn(&mut Surf) -> Rect| {
            s.px.fill(bg);
            let r = f(s);
            let n = s.px.iter().filter(|p| (*p & 0x00FF_FFFF) != bg).count();
            if r.is_empty() || n == 0 {
                ctl_ok = false;
            }
        };
        probe(&mut s, &|s| toggle(s, 8, 8, true, st));
        probe(&mut s, &|s| toggle(s, 8, 8, false, st));
        probe(&mut s, &|s| checkbox(s, 8, 8, true, st));
        probe(&mut s, &|s| checkbox(s, 8, 8, false, st));
        probe(&mut s, &|s| radio(s, 8, 8, true, st));
        probe(&mut s, &|s| radio(s, 8, 8, false, st));
        probe(&mut s, &|s| slider(s, 8, 8, 160, 30, 100, st));
        probe(&mut s, &|s| push_button(s, 8, 8, 0, b"Apply", false, st));
        probe(&mut s, &|s| push_button(s, 8, 8, 0, b"OK", true, st));
        probe(&mut s, &|s| segmented(s, 8, 8, &[b"Light", b"Dark", b"Auto"], 1, st));
        probe(&mut s, &|s| text_field(s, 8, 8, 180, b"una", st));
        probe(&mut s, &|s| sidebar_row(s, 8, 8, 180, b"Displays", true, st));
        probe(&mut s, &|s| sidebar_row(s, 8, 8, 180, b"Sound", false, st));
        probe(&mut s, &|s| group_box(s, Rect::new(8, 8, 200, 120), b"Appearance"));
    }
    if !ctl_ok {
        bad |= 1024;
    }
    // The dock seam: a square tile rounded after the fact keeps its interior and restores its corner.
    let mut row = [bg; 64];
    let k = row_keep(&row, 8, 40, 10);
    row[8..48].fill(fg);
    row_round(&mut row, &k, 8, 40, 40, 0, 10);
    if row[8] & 0x00FF_FFFF != bg || row[28] & 0x00FF_FFFF != fg {
        bad |= 1;
    }
    (bad == 0, bad)
}

/// Monotonic nanoseconds and the clock's name (the TSC where it is calibrated, else the millisecond tick).
fn now_ns() -> (u64, &'static str) {
    #[cfg(target_arch = "x86_64")]
    {
        let hz = crate::arch::x86_64::apic::tsc_hz();
        if hz >= 1_000_000 {
            return (crate::arch::now_cycles().saturating_mul(1000) / (hz / 1_000_000), "tsc");
        }
    }
    (crate::arch::ms().saturating_mul(1_000_000), "ms")
}

/// The per-op cost on the running machine: each op repeated, the mean printed.
fn cost() {
    // The window the budget is stated for: 400 x 300 logical at 2x, its shadow at 2x.
    const WIN_W: i32 = 800;
    const WIN_H: i32 = 600;
    const WIN_BLUR: i32 = 32;
    const WIN_RADIUS: i32 = 20;
    const BUDGET_US: u64 = 1000;
    let side = (WIN_W + 4 * WIN_BLUR) as usize;
    let mut buf = alloc::vec::Vec::new();
    if buf.try_reserve_exact(side * side).is_err() {
        serial_println!(":: GLASSKIT: cost no-surface ::");
        return;
    }
    buf.resize(side * side, theme::surface_window());
    let mut s = Surf::new(&mut buf, side);
    let mut time = |n: u64, f: &mut dyn FnMut(&mut Surf)| -> u64 {
        let (t0, _) = now_ns();
        for _ in 0..n {
            f(&mut s);
        }
        now_ns().0.saturating_sub(t0) / n.max(1)
    };
    let r = Rect::new(16, 16, 200, 120);
    let a = time(64, &mut |s| fill_rrect(s, r, 12, theme::accent()));
    let b = time(64, &mut |s| stroke_rrect(s, r, 12, 1, Ink::argb(theme::hairline())));
    let c = time(64, &mut |s| gradient_v(s, r, theme::accent(), theme::surface_window()));
    let e = time(64, &mut |s| fill_circle(s, 100, 100, 20, theme::accent()));
    let l = time(64, &mut |s| line_aa(s, 20 * SUB, 20 * SUB, 200 * SUB, 90 * SUB, 2 * SUB, theme::text_primary()));
    let k = time(16, &mut |s| {
        toggle(s, 8, 8, true, ControlState::NORMAL);
        checkbox(s, 80, 8, true, ControlState::NORMAL);
        slider(s, 8, 60, 200, 40, 100, ControlState::NORMAL);
        push_button(s, 8, 100, 0, b"Apply", true, ControlState::NORMAL);
    });
    let w = Rect::new(2 * WIN_BLUR, 2 * WIN_BLUR, WIN_W, WIN_H);
    let sh = time(8, &mut |s| shadow_rrect(s, w.offset(0, WIN_BLUR / 4), WIN_RADIUS, WIN_BLUR, theme::shadow_soft()));
    let (_, clock) = now_ns();
    serial_println!(
        ":: GLASSKIT: cost clock={} fill_rrect_ns={} stroke_rrect_ns={} gradient_ns={} circle_ns={} line_ns={} controls_ns={} shadow_win_us={} shadow_budget_us={} win={}x{}@2x blur={} ::",
        clock, a, b, c, e, l, k, sh / 1000, BUDGET_US, WIN_W / 2, WIN_H / 2, WIN_BLUR
    );
}

// ── (9) MOTIONKIT (B648): the motion primitive — `draw::motion`, in video/motion.rs ─────────────────────────────
// Declared from this file's tail (not video/mod.rs, the module root every image lexes) so it rides the kit's own
// desktop gate and no line above it moves. The tween, the curves, the glides, the frame gate and `tests motion`.
#[path = "motion.rs"]
pub mod motion;
