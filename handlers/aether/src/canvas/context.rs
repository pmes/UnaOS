//! The 2D rendering context's engine (HTML §4.12.5.1): the output bitmap (premultiplied RGBA8), the
//! drawing-state stack, the current default path, and the drawing model of §4.12.5.1.15 — render the shape
//! into image A with the fill/stroke style, render the shadow B from A, multiply both by `globalAlpha`,
//! composite B then A onto the bitmap with `globalCompositeOperation` inside the clipping region.

use super::composite::{composite, Op};
use super::geom::{winding, on_edge, Matrix, Path, P};
use super::paint::{premul, Bitmap, Paint, Rgba};
use super::raster::{self, Cap, Coverage, Join, StrokeStyle};
use super::text::{self, Align, Baseline, FontSpec, TextOpts};
use js_core::vm::Obj;
use std::rc::Rc;

/// A fill or stroke style: a colour, or a CanvasGradient / CanvasPattern object the binding resolves.
#[derive(Clone, Debug, PartialEq)]
pub enum Style {
    Color(Rgba),
    Object(Obj),
}

#[derive(Clone, Debug)]
pub struct State {
    pub ctm: Matrix,
    /// Full-canvas clip coverage (0..=255); None = no clip.
    pub clip: Option<Rc<Vec<u8>>>,
    pub fill: Style,
    pub stroke: Style,
    pub alpha: f64,
    pub op: Op,
    pub smoothing: bool,
    pub smoothing_quality: &'static str,
    pub shadow_x: f64,
    pub shadow_y: f64,
    pub shadow_blur: f64,
    pub shadow_color: Rgba,
    pub line_width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter: f64,
    pub dash: Vec<f64>,
    pub dash_offset: f64,
    pub font: FontSpec,
    pub align: Align,
    pub baseline: Baseline,
    /// 0 inherit, 1 ltr, 2 rtl.
    pub direction: u8,
    pub letter_spacing: (f64, String),
    pub word_spacing: (f64, String),
    pub kerning: &'static str,
    pub stretch: u16,
    pub variant_caps: &'static str,
    pub text_rendering: &'static str,
    pub filter: String,
}

impl Default for State {
    fn default() -> Self {
        State {
            ctm: Matrix::IDENTITY,
            clip: None,
            fill: Style::Color([0.0, 0.0, 0.0, 1.0]),
            stroke: Style::Color([0.0, 0.0, 0.0, 1.0]),
            alpha: 1.0,
            op: Op::SourceOver,
            smoothing: true,
            smoothing_quality: "low",
            shadow_x: 0.0,
            shadow_y: 0.0,
            shadow_blur: 0.0,
            shadow_color: [0.0, 0.0, 0.0, 0.0],
            line_width: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter: 10.0,
            dash: Vec::new(),
            dash_offset: 0.0,
            font: FontSpec::default(),
            align: Align::Start,
            baseline: Baseline::Alphabetic,
            direction: 0,
            letter_spacing: (0.0, "0px".into()),
            word_spacing: (0.0, "0px".into()),
            kerning: "auto",
            stretch: 100,
            variant_caps: "normal",
            text_rendering: "auto",
            filter: "none".into(),
        }
    }
}

/// A rendered layer: premultiplied colour over a device-space box.
struct Layer {
    x: i64,
    y: i64,
    w: usize,
    h: usize,
    data: Vec<[f32; 4]>,
}

impl Layer {
    #[inline]
    fn get(&self, x: i64, y: i64) -> [f32; 4] {
        if x < self.x || y < self.y || x >= self.x + self.w as i64 || y >= self.y + self.h as i64 {
            return [0.0; 4];
        }
        self.data[((y - self.y) as usize) * self.w + (x - self.x) as usize]
    }
}

/// The largest bitmap the context allocates (pixels); a bigger canvas has no bitmap (drawing is a no-op).
pub const MAX_PIXELS: usize = 1 << 26;

pub struct Canvas2D {
    pub w: usize,
    pub h: usize,
    /// Premultiplied RGBA8, row-major.
    pub px: Vec<u8>,
    /// `alpha: false` in the context settings: an opaque bitmap (starts opaque black).
    pub opaque: bool,
    pub will_read: bool,
    pub color_space: &'static str,
    pub state: State,
    pub stack: Vec<State>,
    pub path: Path,
    /// Bumped on every pixel change (the renderer's cache key).
    pub generation: u64,
}

impl Canvas2D {
    pub fn new(w: usize, h: usize, opaque: bool) -> Canvas2D {
        let mut c = Canvas2D {
            w: 0,
            h: 0,
            px: Vec::new(),
            opaque,
            will_read: false,
            color_space: "srgb",
            state: State::default(),
            stack: Vec::new(),
            path: Path::new(),
            generation: 0,
        };
        c.resize(w, h);
        c
    }

    /// Set bitmap dimensions: a new transparent (opaque: black) bitmap, and the context reset to its default
    /// state (HTML: "reset the rendering context to its default state" on any width/height set).
    pub fn resize(&mut self, w: usize, h: usize) {
        let (w, h) = if w.saturating_mul(h) > MAX_PIXELS { (0, 0) } else { (w, h) };
        self.w = w;
        self.h = h;
        self.px = vec![0u8; w * h * 4];
        if self.opaque {
            for p in self.px.chunks_exact_mut(4) {
                p[3] = 255;
            }
        }
        self.reset_state();
    }

    pub fn reset_state(&mut self) {
        self.state = State::default();
        self.stack.clear();
        self.path.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// `ctx.reset()`: clear the bitmap and reset everything.
    pub fn reset(&mut self) {
        let (w, h) = (self.w, self.h);
        self.resize(w, h);
    }

    pub fn save(&mut self) {
        if self.stack.len() < 1 << 16 {
            self.stack.push(self.state.clone());
        }
    }

    pub fn restore(&mut self) {
        if let Some(s) = self.stack.pop() {
            self.state = s;
        }
    }

    // ------------------------------------------------------------------------------------- the drawing model

    fn shadows_on(&self) -> bool {
        let s = &self.state;
        s.shadow_color[3] > 0.0 && (s.shadow_blur > 0.0 || s.shadow_x != 0.0 || s.shadow_y != 0.0)
    }

    fn layer_from(&self, cov: &Coverage, paint: &Paint) -> Layer {
        let ga = self.state.alpha as f32;
        let mut data = vec![[0f32; 4]; cov.w * cov.h];
        for yy in 0..cov.h {
            for xx in 0..cov.w {
                let c = cov.data[yy * cov.w + xx];
                if c <= 0.0 {
                    continue;
                }
                let s = paint.sample(cov.x + xx, cov.y + yy);
                let k = c * ga;
                data[yy * cov.w + xx] = [s[0] * k, s[1] * k, s[2] * k, s[3] * k];
            }
        }
        Layer { x: cov.x as i64, y: cov.y as i64, w: cov.w, h: cov.h, data }
    }

    fn shadow_of(&self, a: &Layer) -> Layer {
        let s = &self.state;
        let sigma = s.shadow_blur / 2.0;
        let pad = if sigma > 0.0 { (sigma * 3.0).ceil() as usize + 1 } else { 0 };
        let (w, h) = (a.w + 2 * pad, a.h + 2 * pad);
        let mut alpha = vec![0f32; w * h];
        for yy in 0..a.h {
            for xx in 0..a.w {
                alpha[(yy + pad) * w + xx + pad] = a.data[yy * a.w + xx][3];
            }
        }
        raster::blur(&mut alpha, w, h, sigma);
        let c = premul(s.shadow_color);
        let data = alpha.iter().map(|&k| [c[0] * k, c[1] * k, c[2] * k, c[3] * k]).collect();
        Layer {
            x: a.x - pad as i64 + s.shadow_x.round() as i64,
            y: a.y - pad as i64 + s.shadow_y.round() as i64,
            w,
            h,
            data,
        }
    }

    fn composite_layer(&mut self, l: &Layer) {
        let op = self.state.op;
        let clip = self.state.clip.clone();
        let (cw, ch) = (self.w as i64, self.h as i64);
        let (x0, y0, x1, y1) = if op.unbounded() {
            (0, 0, cw, ch)
        } else {
            (l.x.max(0), l.y.max(0), (l.x + l.w as i64).min(cw), (l.y + l.h as i64).min(ch))
        };
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let opaque = self.opaque;
        for y in y0..y1 {
            for x in x0..x1 {
                let s = l.get(x, y);
                if !op.unbounded() && s[3] <= 0.0 && s[0] <= 0.0 && s[1] <= 0.0 && s[2] <= 0.0 {
                    continue;
                }
                let i = ((y * cw + x) * 4) as usize;
                let k = match &clip {
                    Some(m) => m[(y * cw + x) as usize] as f32 / 255.0,
                    None => 1.0,
                };
                if k <= 0.0 {
                    continue;
                }
                let d = [self.px[i] as f32 / 255.0, self.px[i + 1] as f32 / 255.0, self.px[i + 2] as f32 / 255.0, self.px[i + 3] as f32 / 255.0];
                let mut r = composite(op, s, d);
                if k < 1.0 {
                    for c in 0..4 {
                        r[c] = d[c] + (r[c] - d[c]) * k;
                    }
                }
                if opaque {
                    r[3] = 1.0;
                }
                for c in 0..4 {
                    self.px[i + c] = (r[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
    }

    /// Draw coverage with a paint through the full drawing model.
    pub fn draw(&mut self, cov: Option<Coverage>, paint: &Paint) {
        if self.w == 0 || self.h == 0 {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let cov = cov.unwrap_or_default();
        let a = self.layer_from(&cov, paint);
        if self.shadows_on() && cov.w > 0 {
            let b = self.shadow_of(&a);
            self.composite_layer(&b);
        }
        self.composite_layer(&a);
    }

    // ------------------------------------------------------------------------------------------ shapes

    pub fn fill_polys(&mut self, polys: &[(Vec<P>, bool)], evenodd: bool, paint: &Paint) {
        let cov = raster::fill(polys, evenodd, self.w, self.h);
        self.draw(cov, paint);
    }

    /// Fill a device-space path.
    pub fn fill_path(&mut self, dev: &Path, evenodd: bool, paint: &Paint) {
        let polys = dev.flatten(0.1);
        self.fill_polys(&polys, evenodd, paint);
    }

    fn stroke_style(&self) -> StrokeStyle {
        let s = &self.state;
        StrokeStyle { width: s.line_width, cap: s.cap, join: s.join, miter: s.miter, dash: s.dash.clone(), dash_offset: s.dash_offset }
    }

    /// The stroke outline of a USER-space path, in device space.
    pub fn stroke_outline_user(&self, user: &Path) -> Vec<(Vec<P>, bool)> {
        let ctm = self.state.ctm;
        let tol = 0.1 / ctm.max_scale();
        let polys = user.flatten(tol);
        let out = raster::stroke_polys(&polys, &self.stroke_style(), tol);
        raster::transform_polys(&out, &ctm)
    }

    /// Stroke a device-space path (the context's own path): back to user space through the inverse CTM, so
    /// line width, joins and dashes are in the current user space.
    pub fn stroke_path(&mut self, dev: &Path, paint: &Paint) {
        let Some(inv) = self.state.ctm.invert() else { return };
        let user = dev.transformed(&inv);
        self.stroke_user(&user, paint);
    }

    pub fn stroke_user(&mut self, user: &Path, paint: &Paint) {
        if !self.state.ctm.is_invertible() {
            return;
        }
        let polys = self.stroke_outline_user(user);
        self.fill_polys(&polys, false, paint);
    }

    pub fn rect_path(&self, x: f64, y: f64, w: f64, h: f64) -> Path {
        let mut p = Path::new();
        p.rect(&self.state.ctm, x, y, w, h);
        p
    }

    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64, paint: &Paint) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || w == 0.0 || h == 0.0 {
            return;
        }
        let p = self.rect_path(x, y, w, h);
        self.fill_path(&p, false, paint);
    }

    pub fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64, paint: &Paint) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || (w == 0.0 && h == 0.0) {
            return;
        }
        let mut p = Path::new();
        let id = Matrix::IDENTITY;
        if w == 0.0 || h == 0.0 {
            p.move_to(&id, x, y);
            p.line_to(&id, x + w, y + h);
        } else {
            p.rect(&id, x, y, w, h);
        }
        self.stroke_user(&p, paint);
    }

    /// `clearRect`: transparent black inside the rectangle ∩ clip, ignoring style, alpha, shadows and op.
    pub fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || w == 0.0 || h == 0.0 || self.w == 0 {
            return;
        }
        let p = self.rect_path(x, y, w, h);
        let Some(cov) = raster::fill(&p.flatten(0.1), false, self.w, self.h) else { return };
        self.generation = self.generation.wrapping_add(1);
        let clip = self.state.clip.clone();
        for yy in 0..cov.h {
            for xx in 0..cov.w {
                let (px, py) = (cov.x + xx, cov.y + yy);
                let mut k = cov.data[yy * cov.w + xx];
                if let Some(m) = &clip {
                    k *= m[py * self.w + px] as f32 / 255.0;
                }
                if k <= 0.0 {
                    continue;
                }
                let i = (py * self.w + px) * 4;
                for c in 0..4 {
                    let v = self.px[i + c] as f32 * (1.0 - k);
                    self.px[i + c] = (v + 0.5) as u8;
                }
                if self.opaque {
                    self.px[i + 3] = 255;
                }
            }
        }
    }

    /// Intersect the clipping region with a device-space path.
    pub fn clip(&mut self, dev: &Path, evenodd: bool) {
        let (w, h) = (self.w, self.h);
        let cov = raster::fill(&dev.flatten(0.1), evenodd, w, h);
        let mut mask = vec![0u8; w * h];
        if let Some(c) = cov {
            for yy in 0..c.h {
                for xx in 0..c.w {
                    mask[(c.y + yy) * w + c.x + xx] = (c.data[yy * c.w + xx] * 255.0 + 0.5) as u8;
                }
            }
        }
        if let Some(old) = &self.state.clip {
            for (m, o) in mask.iter_mut().zip(old.iter()) {
                *m = ((*m as u32 * *o as u32 + 127) / 255) as u8;
            }
        }
        self.state.clip = Some(Rc::new(mask));
    }

    pub fn point_in_path(dev: &Path, x: f64, y: f64, evenodd: bool) -> bool {
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        let polys = dev.flatten(0.05);
        if on_edge(&polys, (x, y)) {
            return true;
        }
        let (wn, cr) = winding(&polys, (x, y));
        if evenodd { cr % 2 == 1 } else { wn != 0 }
    }

    pub fn point_in_stroke_user(&self, user: &Path, x: f64, y: f64) -> bool {
        if !x.is_finite() || !y.is_finite() || !self.state.ctm.is_invertible() {
            return false;
        }
        let polys = self.stroke_outline_user(user);
        if on_edge(&polys, (x, y)) {
            return true;
        }
        winding(&polys, (x, y)).0 != 0
    }

    // ------------------------------------------------------------------------------------------ images

    /// `drawImage` with the source rectangle already normalised and clipped to the image.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_image(&mut self, img: Bitmap, sx: f64, sy: f64, sw: f64, sh: f64, dx: f64, dy: f64, dw: f64, dh: f64) {
        if sw == 0.0 || sh == 0.0 || dw == 0.0 || dh == 0.0 {
            return;
        }
        let ctm = self.state.ctm;
        let to_user = Matrix::translate(dx, dy).mul(&Matrix::scale(dw / sw, dh / sh)).mul(&Matrix::translate(-sx, -sy));
        let Some(inv) = ctm.mul(&to_user).invert() else { return };
        let paint = Paint::Image {
            inv,
            img,
            repeat: super::paint::Repeat::None,
            smooth: self.state.smoothing,
            src: (sx, sy, sw, sh),
        };
        let p = self.rect_path(dx, dy, dw, dh);
        self.fill_path(&p, false, &paint);
    }

    /// The bitmap as straight RGBA (the snapshot drawImage/createPattern/getImageData/toDataURL read).
    pub fn snapshot(&self) -> Bitmap {
        Bitmap { w: self.w, h: self.h, rgba: Rc::new(self.straight_rect(0, 0, self.w as i64, self.h as i64)) }
    }

    /// Straight RGBA of a rectangle (outside the bitmap: transparent black).
    pub fn straight_rect(&self, sx: i64, sy: i64, sw: i64, sh: i64) -> Vec<u8> {
        let (sw, sh) = (sw.max(0) as usize, sh.max(0) as usize);
        let mut out = vec![0u8; sw * sh * 4];
        for yy in 0..sh {
            let y = sy + yy as i64;
            if y < 0 || y >= self.h as i64 {
                continue;
            }
            for xx in 0..sw {
                let x = sx + xx as i64;
                if x < 0 || x >= self.w as i64 {
                    continue;
                }
                let i = ((y as usize) * self.w + x as usize) * 4;
                let o = (yy * sw + xx) * 4;
                let a = self.px[i + 3] as u32;
                if a == 0 {
                    continue;
                }
                for c in 0..3 {
                    out[o + c] = if a == 255 { self.px[i + c] } else { ((self.px[i + c] as u32 * 255 + a / 2) / a).min(255) as u8 };
                }
                out[o + 3] = a as u8;
            }
        }
        out
    }

    /// `putImageData`: copy straight RGBA (`iw × ih`) into the bitmap at (dx, dy), restricted to the dirty
    /// rectangle (already clamped to the image), bypassing style, alpha, shadows, op and clip.
    #[allow(clippy::too_many_arguments)]
    pub fn put_image_data(&mut self, data: &[u8], iw: usize, ih: usize, dx: i64, dy: i64, rx: i64, ry: i64, rw: i64, rh: i64) {
        self.generation = self.generation.wrapping_add(1);
        for yy in ry.max(0)..(ry + rh).min(ih as i64) {
            let y = dy + yy;
            if y < 0 || y >= self.h as i64 {
                continue;
            }
            for xx in rx.max(0)..(rx + rw).min(iw as i64) {
                let x = dx + xx;
                if x < 0 || x >= self.w as i64 {
                    continue;
                }
                let s = ((yy as usize) * iw + xx as usize) * 4;
                if s + 4 > data.len() {
                    continue;
                }
                let d = ((y as usize) * self.w + x as usize) * 4;
                let a = if self.opaque { 255 } else { data[s + 3] as u32 };
                for c in 0..3 {
                    self.px[d + c] = ((data[s + c] as u32 * a + 127) / 255) as u8;
                }
                self.px[d + 3] = a as u8;
            }
        }
    }

    // ------------------------------------------------------------------------------------------ text

    pub fn text_opts(&self) -> TextOpts<'_> {
        let s = &self.state;
        TextOpts {
            font: &s.font,
            align: s.align,
            baseline: s.baseline,
            rtl: s.direction == 2,
            letter_spacing: s.letter_spacing.0,
            word_spacing: s.word_spacing.0,
            kerning: s.kerning != "none",
            stretch: Some(s.stretch),
        }
    }

    pub fn draw_text(&mut self, t: &str, x: f64, y: f64, max_width: Option<f64>, paint: &Paint, stroke: bool) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let ctm = self.state.ctm;
        let path = {
            let o = self.text_opts();
            let laid = text::lay_out(t, &o);
            let m = if stroke { Matrix::IDENTITY } else { ctm };
            text::text_path(&laid, &o, x, y, max_width, &m)
        };
        let Some(path) = path else { return };
        if stroke {
            self.stroke_user(&path, paint);
        } else {
            self.fill_path(&path, false, paint);
        }
    }

    pub fn measure_text(&self, t: &str) -> text::Metrics {
        text::measure(t, &self.text_opts())
    }

    // ------------------------------------------------------------------------------------------ export

    /// PNG bytes of the bitmap (pixel_core's encoder), None for a zero-size canvas.
    pub fn to_png(&self) -> Option<Vec<u8>> {
        if self.w == 0 || self.h == 0 {
            return None;
        }
        let rgba = self.straight_rect(0, 0, self.w as i64, self.h as i64);
        pixel_core::png::encode::encode_rgba(self.w as u32, self.h as u32, &rgba).ok()
    }
}
