// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. PDF2 (SR313): the content interpreter (ISO 32000-1 §8) painting through svg_core's
//! rasterizer; text goes to a [`TextSink`] (PDF3 draws glyphs).
//!
//! A graphics-state machine over [`crate::content::ops`]: the CTM and the q/Q stack, line width / cap / join / miter /
//! dash, fill and stroke colour ([`colour`], tints through [`function`]), clipping (W n / W* n intersect a device mask),
//! paths (m l c v y h re) and the painting operators (f F f* B B* b b* S s n), constant alpha and the ExtGState
//! ([`state`]), form XObjects (recursive to [`MAX_FORM_DEPTH`], their /Matrix and /BBox clip), image XObjects and
//! inline images ([`image`]), shadings and shading patterns ([`shading`]). Every fill and stroke is svg_core's: its
//! path filler (`raster::fill_polys`, nonzero / even-odd), its stroker (`stroke::stroke_polys`), its shaders
//! (`paint::Shader`), its clip `Mask` and its one paint loop (`raster::paint_coverage`). This module draws NOTHING
//! for text: each string goes to the [`TextSink`], which returns the advance, an outline to paint under the text
//! rendering mode, and Type 3 glyph procedures to run; the recorder [`TextLog`] returns none.

pub mod colour;
pub mod function;
pub mod image;
pub mod shading;
pub mod state;

use alloc::rc::Rc;
use alloc::vec::Vec;

use svg_core::geom::{Path, Transform};
use svg_core::paint::{Shader, premul};
use svg_core::raster::{Coverage, FLATTEN_TOL, FillRule, Mask, div255, fill_polys, paint_coverage};
use svg_core::stroke::stroke_polys;

pub use svg_core::raster::Pixmap;

use self::colour::{Colour, ColourSpace};
use self::image::Img;
use self::state::{GState, TextMatrices, TextState, refuse};
use crate::content::{Op, ops};
use crate::doc::{Document, Page};
use crate::object::{Dict, Object, Stream};
use crate::{Error, MAX_DEPTH};

/// The deepest form XObject / Type 3 glyph nesting drawn.
pub const MAX_FORM_DEPTH: usize = 16;

/// A rendered page.
pub struct Rendered {
    /// Opaque RGBA (white paper first, so premultiplied and straight agree).
    pub pixmap: Pixmap,
    /// Features met and not drawn as the file asks, by name, each once (e.g. "JPXDecode", "soft mask").
    pub refused: Vec<&'static str>,
}

/// One string shown by Tj / TJ / ' / ": everything a font engine needs to place its glyphs.
pub struct TextRun<'a> {
    /// The text matrix and the CTM at the start of the string.
    pub tm: Transform,
    pub ctm: Transform,
    /// The /Font resource name and its dictionary (empty when absent).
    pub font_name: &'a [u8],
    pub font: &'a Dict,
    pub size: f64,
    pub char_spacing: f64,
    pub word_spacing: f64,
    /// Tz / 100.
    pub h_scale: f64,
    pub rise: f64,
    pub render_mode: u8,
    /// The string's bytes, undecoded.
    pub bytes: &'a [u8],
}

/// A Type 3 glyph to draw: its content stream, its resources (empty: the caller's), and glyph space -> text space.
#[derive(Clone, Debug, Default)]
pub struct Type3Glyph {
    pub proc_: Vec<u8>,
    pub resources: Dict,
    pub m: [f64; 6],
}

/// What a [`TextSink`] returns for one string.
#[derive(Clone, Debug, Default)]
pub struct Glyphs {
    /// The pen's displacement in text space (Tfs, Tc, Tw, Th applied): Tm moves by it.
    pub advance: f64,
    /// The string's glyph outlines in TEXT space (each glyph at its pen x, Tfs / Th / rise applied); the renderer
    /// applies Tm x CTM and paints it under the rendering mode (fill, stroke, clip).
    pub outline: Path,
    /// Type 3 glyphs (empty for every other font): each run through this interpreter under m x Tm x CTM.
    pub type3: Vec<Type3Glyph>,
}

/// The font seam: PDF3 implements it; the renderer calls it once per string.
pub trait TextSink {
    fn text(&mut self, run: &TextRun) -> Glyphs;
}

/// One recorded string.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRecord {
    pub tm: Transform,
    pub ctm: Transform,
    pub font_name: Vec<u8>,
    pub size: f64,
    pub bytes: Vec<u8>,
}

/// A [`TextSink`] that records every string and draws nothing.
#[derive(Clone, Debug, Default)]
pub struct TextLog {
    pub runs: Vec<TextRecord>,
}

impl TextSink for TextLog {
    fn text(&mut self, run: &TextRun) -> Glyphs {
        self.runs.push(TextRecord { tm: run.tm, ctm: run.ctm, font_name: run.font_name.to_vec(), size: run.size, bytes: run.bytes.to_vec() });
        Glyphs::default()
    }
}

impl Page {
    /// Render the page's CropBox at `dpi` (72 = one pixel per point), /Rotate applied clockwise.
    pub fn render(&self, doc: &Document, dpi: f64, sink: &mut dyn TextSink) -> Result<Rendered, Error> {
        let s = dpi / 72.0;
        if !(s > 0.0 && s.is_finite()) {
            return Err(Error::Unsupported("render dpi"));
        }
        let [x0, y0, x1, y1] = self.crop_box;
        let (w0, h0) = ((x1 - x0) * s, (y1 - y0) * s);
        let px = |v: f64| svg_core::fmath::ceil(v - 1e-6).max(1.0);
        let (pw, ph) = (px(w0), px(h0));
        if pw > svg_core::MAX_DIM as f64 || ph > svg_core::MAX_DIM as f64 {
            return Err(Error::TooLarge);
        }
        let flip = Transform::new(s, 0.0, 0.0, -s, -x0 * s, y1 * s);
        let (rot, w, h) = match self.rotate {
            90 => (Transform::new(0.0, 1.0, -1.0, 0.0, h0, 0.0), ph, pw),
            180 => (Transform::new(-1.0, 0.0, 0.0, -1.0, w0, h0), pw, ph),
            270 => (Transform::new(0.0, -1.0, 1.0, 0.0, 0.0, w0), ph, pw),
            _ => (Transform::IDENTITY, pw, ph),
        };
        let (w, h) = (w as usize, h as usize);
        let mut canvas = Pixmap::new(w, h);
        canvas.data.fill(255);
        let mut it = Interp {
            doc,
            canvas,
            gs: GState::new(rot.then_pre(&flip)),
            stack: Vec::new(),
            path: Path::new(),
            cur: (0.0, 0.0),
            start: (0.0, 0.0),
            pending_clip: None,
            text: TextMatrices::default(),
            text_clip: None,
            res: self.resources.clone(),
            refused: Vec::new(),
            sink,
            depth: 0,
        };
        it.run(&ops(&self.contents));
        Ok(Rendered { pixmap: it.canvas, refused: it.refused })
    }
}

/// A rendered page as PNG bytes (pixel_core's encoder).
pub fn png(p: &Pixmap) -> Result<Vec<u8>, &'static str> {
    pixel_core::png::encode::encode_rgba(p.w as u32, p.h as u32, &p.to_straight()).map_err(|_| "png encode")
}

struct Interp<'d, 's> {
    doc: &'d Document,
    canvas: Pixmap,
    gs: GState,
    stack: Vec<GState>,
    /// The current path, in user space.
    path: Path,
    cur: (f64, f64),
    start: (f64, f64),
    /// W / W*: the clip the next painting operator adds.
    pending_clip: Option<FillRule>,
    text: TextMatrices,
    /// Text rendering modes 4-7: glyph outlines (device space) added to the clip at ET.
    text_clip: Option<Path>,
    /// The current content stream's resources.
    res: Dict,
    refused: Vec<&'static str>,
    sink: &'s mut dyn TextSink,
    depth: usize,
}

fn nums(op: &Op) -> Vec<f64> {
    op.operands.iter().filter_map(Object::as_f64).collect()
}

fn rect_path(x: f64, y: f64, w: f64, h: f64) -> Path {
    let mut p = Path::new();
    p.move_to(x, y);
    p.line_to(x + w, y);
    p.line_to(x + w, y + h);
    p.line_to(x, y + h);
    p.close();
    p
}

fn matrix(doc: &Document, o: Option<&Object>) -> Option<Transform> {
    let a = doc.get(o?);
    let v: Vec<f64> = a.as_array()?.iter().filter_map(|x| doc.get(x).as_f64()).collect();
    (v.len() == 6).then(|| Transform::new(v[0], v[1], v[2], v[3], v[4], v[5]))
}

impl Interp<'_, '_> {
    fn run(&mut self, list: &[Op]) {
        for op in list {
            self.op(op);
        }
    }

    fn resource(&self, cat: &[u8], name: &[u8]) -> Option<Object> {
        let table = self.doc.get(self.res.get(cat)?);
        let o = table.as_dict()?.get(name)?.clone();
        Some(self.doc.get(&o))
    }

    fn op(&mut self, op: &Op) {
        let n = nums(op);
        let name = op.operands.first().and_then(Object::as_name).map(<[u8]>::to_vec);
        match op.operator.as_slice() {
            // --- graphics state (§8.4.4)
            b"q" => {
                if self.stack.len() < MAX_DEPTH {
                    self.stack.push(self.gs.clone());
                }
            }
            b"Q" => {
                if let Some(g) = self.stack.pop() {
                    self.gs = g;
                }
            }
            b"cm" if n.len() == 6 => {
                self.gs.ctm = self.gs.ctm.then_pre(&Transform::new(n[0], n[1], n[2], n[3], n[4], n[5]));
            }
            b"w" if n.len() == 1 => self.gs.line_width = n[0].abs(),
            b"J" if n.len() == 1 => self.gs.set_cap(n[0] as i64),
            b"j" if n.len() == 1 => self.gs.set_join(n[0] as i64),
            b"M" if n.len() == 1 => self.gs.miter_limit = n[0],
            b"d" => {
                if let (Some(a), Some(ph)) = (op.operands.first().and_then(Object::as_array), op.operands.get(1).and_then(Object::as_f64)) {
                    self.gs.dash = a.iter().filter_map(Object::as_f64).collect();
                    self.gs.dash_phase = ph;
                }
            }
            b"gs" => {
                if let Some(Object::Dict(d)) = name.and_then(|nm| self.resource(b"ExtGState", &nm)) {
                    self.gs.apply_ext(&d, self.doc, &mut self.refused);
                }
            }
            // --- path construction (§8.5.2)
            b"m" if n.len() == 2 => {
                self.path.move_to(n[0], n[1]);
                self.cur = (n[0], n[1]);
                self.start = self.cur;
            }
            b"l" if n.len() == 2 => {
                self.path.line_to(n[0], n[1]);
                self.cur = (n[0], n[1]);
            }
            b"c" if n.len() == 6 => {
                self.path.cubic_to((n[0], n[1]), (n[2], n[3]), (n[4], n[5]));
                self.cur = (n[4], n[5]);
            }
            b"v" if n.len() == 4 => {
                self.path.cubic_to(self.cur, (n[0], n[1]), (n[2], n[3]));
                self.cur = (n[2], n[3]);
            }
            b"y" if n.len() == 4 => {
                self.path.cubic_to((n[0], n[1]), (n[2], n[3]), (n[2], n[3]));
                self.cur = (n[2], n[3]);
            }
            b"h" => {
                self.path.close();
                self.cur = self.start;
            }
            b"re" if n.len() == 4 => {
                self.path.extend(&rect_path(n[0], n[1], n[2], n[3]));
                self.cur = (n[0], n[1]);
                self.start = self.cur;
            }
            // --- path painting (§8.5.3)
            b"f" | b"F" => self.paint_path(Some(FillRule::NonZero), false, false),
            b"f*" => self.paint_path(Some(FillRule::EvenOdd), false, false),
            b"B" => self.paint_path(Some(FillRule::NonZero), true, false),
            b"B*" => self.paint_path(Some(FillRule::EvenOdd), true, false),
            b"b" => self.paint_path(Some(FillRule::NonZero), true, true),
            b"b*" => self.paint_path(Some(FillRule::EvenOdd), true, true),
            b"S" => self.paint_path(None, true, false),
            b"s" => self.paint_path(None, true, true),
            b"n" => self.paint_path(None, false, false),
            b"W" => self.pending_clip = Some(FillRule::NonZero),
            b"W*" => self.pending_clip = Some(FillRule::EvenOdd),
            // --- colour (§8.6.8)
            b"CS" | b"cs" => {
                if let Some(o) = op.operands.first() {
                    if let Some(sp) = ColourSpace::parse(o, &self.res, self.doc) {
                        *self.colour_mut(op.operator == b"CS") = Colour::of(sp);
                    }
                }
            }
            b"SC" | b"SCN" | b"sc" | b"scn" => {
                let pat = op.operands.last().and_then(Object::as_name).map(<[u8]>::to_vec);
                let c = self.colour_mut(op.operator[0] == b'S');
                if !n.is_empty() {
                    c.comps = n;
                }
                c.pattern = pat;
            }
            b"G" | b"g" if n.len() == 1 => {
                *self.colour_mut(op.operator == b"G") = Colour { space: Rc::new(ColourSpace::Gray), comps: n, pattern: None };
            }
            b"RG" | b"rg" if n.len() == 3 => {
                *self.colour_mut(op.operator == b"RG") = Colour { space: Rc::new(ColourSpace::Rgb), comps: n, pattern: None };
            }
            b"K" | b"k" if n.len() == 4 => {
                *self.colour_mut(op.operator == b"K") = Colour { space: Rc::new(ColourSpace::Cmyk), comps: n, pattern: None };
            }
            // --- shadings, XObjects, inline images
            b"sh" => {
                if let Some(o) = name.and_then(|nm| self.resource(b"Shading", &nm)) {
                    self.shade(&o);
                }
            }
            b"Do" => {
                if let Some(Object::Stream(s)) = name.and_then(|nm| self.resource(b"XObject", &nm)) {
                    match s.dict.name(b"Subtype") {
                        Some(b"Form") => self.form(&s),
                        Some(b"Image") => {
                            if let Some(img) = image::xobject(&s, &self.res, self.doc) {
                                self.draw_image(img);
                            }
                        }
                        _ => {}
                    }
                }
            }
            b"BI" => {
                if let (Some(Object::Dict(d)), Some(Object::String(data))) = (op.operands.first(), op.operands.get(1)) {
                    if let Some(img) = image::inline(d, data, &self.res, self.doc) {
                        self.draw_image(img);
                    }
                }
            }
            // --- text (§9.3, §9.4)
            b"BT" => {
                self.text = TextMatrices::default();
                self.text_clip = None;
            }
            b"ET" => {
                if let Some(p) = self.text_clip.take() {
                    if !p.is_empty() {
                        self.clip_device(&p, FillRule::NonZero);
                    }
                }
            }
            b"Tc" if n.len() == 1 => self.gs.text.char_spacing = n[0],
            b"Tw" if n.len() == 1 => self.gs.text.word_spacing = n[0],
            b"Tz" if n.len() == 1 => self.gs.text.h_scale = n[0] / 100.0,
            b"TL" if n.len() == 1 => self.gs.text.leading = n[0],
            b"Tr" if n.len() == 1 => self.gs.text.render_mode = (n[0].clamp(0.0, 7.0)) as u8,
            b"Ts" if n.len() == 1 => self.gs.text.rise = n[0],
            b"Tf" if n.len() == 1 => {
                if let Some(nm) = name {
                    let font = match self.resource(b"Font", &nm) {
                        Some(Object::Dict(d)) => d,
                        _ => Dict::new(),
                    };
                    let t: &mut TextState = &mut self.gs.text;
                    t.font_name = nm;
                    t.font = font;
                    t.size = n[0];
                }
            }
            b"Td" if n.len() == 2 => self.text.td(n[0], n[1]),
            b"TD" if n.len() == 2 => {
                self.gs.text.leading = -n[1];
                self.text.td(n[0], n[1]);
            }
            b"Tm" if n.len() == 6 => self.text.set(Transform::new(n[0], n[1], n[2], n[3], n[4], n[5])),
            b"T*" => self.text.td(0.0, -self.gs.text.leading),
            b"Tj" => {
                if let Some(Object::String(s)) = op.operands.first() {
                    self.show(s);
                }
            }
            b"'" => {
                self.text.td(0.0, -self.gs.text.leading);
                if let Some(Object::String(s)) = op.operands.first() {
                    self.show(s);
                }
            }
            b"\"" => {
                if let (Some(aw), Some(ac), Some(Object::String(s))) =
                    (op.operands.first().and_then(Object::as_f64), op.operands.get(1).and_then(Object::as_f64), op.operands.get(2))
                {
                    self.gs.text.word_spacing = aw;
                    self.gs.text.char_spacing = ac;
                    self.text.td(0.0, -self.gs.text.leading);
                    self.show(s);
                }
            }
            b"TJ" => {
                if let Some(a) = op.operands.first().and_then(Object::as_array) {
                    for e in a {
                        match e {
                            Object::String(s) => self.show(s),
                            o => {
                                if let Some(v) = o.as_f64() {
                                    let t = &self.gs.text;
                                    self.text.advance(-v / 1000.0 * t.size * t.h_scale);
                                }
                            }
                        }
                    }
                }
            }
            // ri i d0 d1 BX EX BMC BDC EMC MP DP and anything malformed: nothing to draw.
            _ => {}
        }
    }

    fn colour_mut(&mut self, stroke: bool) -> &mut Colour {
        if stroke { &mut self.gs.stroke } else { &mut self.gs.fill }
    }

    /// The shader the fill (or stroke) colour paints with; `None` paints nothing.
    fn shader(&mut self, stroke: bool) -> Option<Shader> {
        let col = if stroke { &self.gs.stroke } else { &self.gs.fill };
        if let Some(p) = col.pattern.clone() {
            let o = self.resource(b"Pattern", &p)?;
            return shading::pattern(&o, self.gs.base, &self.res, self.doc, &mut self.refused);
        }
        if matches!(*col.space, ColourSpace::Pattern(_)) {
            return None;
        }
        let c = col.rgb();
        Some(Shader::Solid(premul([c[0], c[1], c[2], 1.0])))
    }

    /// Composite a coverage region through the clip.
    fn paint(&mut self, mut c: Coverage, sh: &Shader, opacity: f32) {
        if let Some(m) = &self.gs.clip {
            for yy in 0..c.h {
                for xx in 0..c.w {
                    let i = yy * c.w + xx;
                    let k = m.data[(c.y + yy) * m.w + c.x + xx] as u32;
                    c.data[i] = div255(c.data[i] as u32 * k) as u8;
                }
            }
        }
        paint_coverage(&mut self.canvas, &c, sh, opacity);
    }

    fn fill_device(&mut self, device: &Path, rule: FillRule, sh: &Shader, opacity: f32) {
        let polys = device.flatten(FLATTEN_TOL);
        if let Some(c) = fill_polys(&polys, rule, self.canvas.w, self.canvas.h, true) {
            self.paint(c, sh, opacity);
        }
    }

    fn stroke_user(&mut self, path: &Path, ts: &Transform) {
        let Some(sh) = self.shader(true) else { return };
        let st = self.gs.stroke_style(ts);
        let polys = stroke_polys(path, &st, ts);
        if let Some(c) = fill_polys(&polys, FillRule::NonZero, self.canvas.w, self.canvas.h, true) {
            self.paint(c, &sh, self.gs.stroke_alpha);
        }
    }

    /// The painting operators: fill with `fill`, then stroke; then the pending clip; then the path ends.
    fn paint_path(&mut self, fill: Option<FillRule>, stroke: bool, close: bool) {
        if close {
            self.path.close();
            self.cur = self.start;
        }
        let path = core::mem::take(&mut self.path);
        let ctm = self.gs.ctm;
        if let Some(rule) = fill {
            if let Some(sh) = self.shader(false) {
                self.fill_device(&path.transform(&ctm), rule, &sh, self.gs.fill_alpha);
            }
        }
        if stroke {
            self.stroke_user(&path, &ctm);
        }
        if let Some(rule) = self.pending_clip.take() {
            self.clip_device(&path.transform(&ctm), rule);
        }
    }

    /// Intersect the clip with a device-space path.
    fn clip_device(&mut self, device: &Path, rule: FillRule) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        let mut mask = Mask::new(w, h, 0);
        if let Some(c) = fill_polys(&device.flatten(FLATTEN_TOL), rule, w, h, true) {
            mask.union_coverage(&c);
        }
        if let Some(old) = &self.gs.clip {
            mask.multiply(old);
        }
        self.gs.clip = Some(Rc::new(mask));
    }

    /// `sh`: the shading over the current clip (its /BBox when it has one), in shading space = the CTM.
    fn shade(&mut self, o: &Object) {
        let ctm = self.gs.ctm;
        let Some(p) = shading::shading(o, ctm, &self.res, self.doc, &mut self.refused) else { return };
        let area = match p.bbox {
            Some(b) => rect_path(b[0], b[1], b[2] - b[0], b[3] - b[1]).transform(&ctm),
            None => rect_path(0.0, 0.0, self.canvas.w as f64, self.canvas.h as f64),
        };
        self.fill_device(&area, FillRule::NonZero, &p.shader, self.gs.fill_alpha);
    }

    /// An image fills the unit square of user space (§8.3.4): image row 0 at the top (y = 1).
    fn draw_image(&mut self, img: Img) {
        let ctm = self.gs.ctm;
        let polys = rect_path(0.0, 0.0, 1.0, 1.0).transform(&ctm).flatten(FLATTEN_TOL);
        let Some(mut c) = fill_polys(&polys, FillRule::NonZero, self.canvas.w, self.canvas.h, true) else { return };
        let to_image = |pix: &Pixmap| ctm.then_pre(&Transform::new(1.0 / pix.w as f64, 0.0, 0.0, -1.0 / pix.h as f64, 0.0, 1.0)).invert();
        match img {
            Img::Refused(name) => {
                refuse(&mut self.refused, name);
                self.paint(c, &Shader::Solid(shading::REFUSED_GREY), self.gs.fill_alpha);
            }
            Img::Pix { pix, smooth } => {
                let Some(inv) = to_image(&pix) else { return };
                let sh = Shader::Image { inv, pix, smooth, repeat: false, cubic: None };
                self.paint(c, &sh, self.gs.fill_alpha);
            }
            Img::Stencil { pix, smooth } => {
                let Some(inv) = to_image(&pix) else { return };
                let Some(fill) = self.shader(false) else { return };
                let mask = Shader::Image { inv, pix, smooth, repeat: false, cubic: None };
                for yy in 0..c.h {
                    for xx in 0..c.w {
                        let i = yy * c.w + xx;
                        if c.data[i] == 0 {
                            continue;
                        }
                        let a = mask.at((c.x + xx) as f64 + 0.5, (c.y + yy) as f64 + 0.5)[3];
                        c.data[i] = (c.data[i] as f32 * a + 0.5) as u8;
                    }
                }
                self.paint(c, &fill, self.gs.fill_alpha);
            }
        }
    }

    /// A form XObject (§8.10): its /Matrix, its /BBox clip, its own resources.
    fn form(&mut self, s: &Stream) {
        let Ok(dec) = self.doc.decode(s) else { return };
        let d = &s.dict;
        let m = matrix(self.doc, d.get(b"Matrix")).unwrap_or(Transform::IDENTITY);
        let bbox: Option<[f64; 4]> = self.doc.get(d.get(b"BBox").unwrap_or(&Object::Null)).as_array().and_then(|a| {
            let v: Vec<f64> = a.iter().filter_map(|x| self.doc.get(x).as_f64()).collect();
            (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
        });
        let res = match d.get(b"Resources").map(|o| self.doc.get(o)) {
            Some(Object::Dict(r)) => r,
            _ => self.res.clone(),
        };
        self.nested(m, bbox, &dec.data, res);
    }

    /// Run a content stream (a form or a Type 3 glyph) under `m` x CTM, isolated: the graphics state, the path, the
    /// text matrices and the resources come back as they were.
    fn nested(&mut self, m: Transform, bbox: Option<[f64; 4]>, data: &[u8], res: Dict) {
        if self.depth >= MAX_FORM_DEPTH {
            return;
        }
        let saved_gs = self.gs.clone();
        let saved_stack = self.stack.len();
        let saved_path = core::mem::take(&mut self.path);
        let saved_clip = self.pending_clip.take();
        let saved_text = self.text;
        let saved_text_clip = self.text_clip.take();
        self.gs.ctm = self.gs.ctm.then_pre(&m);
        if let Some(b) = bbox {
            let p = rect_path(b[0], b[1], b[2] - b[0], b[3] - b[1]).transform(&self.gs.ctm);
            self.clip_device(&p, FillRule::NonZero);
        }
        self.gs.base = self.gs.ctm;
        let saved_res = core::mem::replace(&mut self.res, res);
        self.depth += 1;
        self.run(&ops(data));
        self.depth -= 1;
        self.res = saved_res;
        self.stack.truncate(saved_stack);
        self.gs = saved_gs;
        self.path = saved_path;
        self.pending_clip = saved_clip;
        self.text = saved_text;
        self.text_clip = saved_text_clip;
    }

    /// Show one string through the sink: paint what it returns, then advance Tm.
    fn show(&mut self, bytes: &[u8]) {
        let g = {
            let t = &self.gs.text;
            let run = TextRun {
                tm: self.text.tm,
                ctm: self.gs.ctm,
                font_name: &t.font_name,
                font: &t.font,
                size: t.size,
                char_spacing: t.char_spacing,
                word_spacing: t.word_spacing,
                h_scale: t.h_scale,
                rise: t.rise,
                render_mode: t.render_mode,
                bytes,
            };
            self.sink.text(&run)
        };
        let mode = self.gs.text.render_mode;
        if !g.outline.is_empty() {
            let ts = self.gs.ctm.then_pre(&self.text.tm);
            if matches!(mode, 0 | 2 | 4 | 6) {
                if let Some(sh) = self.shader(false) {
                    self.fill_device(&g.outline.transform(&ts), FillRule::NonZero, &sh, self.gs.fill_alpha);
                }
            }
            if matches!(mode, 1 | 2 | 5 | 6) {
                self.stroke_user(&g.outline, &ts);
            }
            if mode >= 4 {
                self.text_clip.get_or_insert_with(Path::new).extend(&g.outline.transform(&ts));
            }
        }
        if mode != 3 && mode != 7 {
            for t3 in &g.type3 {
                let m = Transform::new(t3.m[0], t3.m[1], t3.m[2], t3.m[3], t3.m[4], t3.m[5]);
                let res = if t3.resources.is_empty() { self.res.clone() } else { t3.resources.clone() };
                self.nested(self.text.tm.then_pre(&m), None, &t3.proc_, res);
            }
        }
        self.text.advance(g.advance);
    }
}
