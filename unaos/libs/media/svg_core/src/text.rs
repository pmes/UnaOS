//! `<text>` / `<tspan>` (SVG 1.1 §10, SVG 2 §11): character collection with `xml:space` white-space handling
//! (§10.15), per-character `x`/`y`/`dx`/`dy`/`rotate` lists assigned innermost-first (§10.5), text chunks
//! started by absolute positions and aligned by `text-anchor` (§10.9.1), `letter-spacing`/`word-spacing`,
//! `baseline-shift` and `dominant-baseline`, `text-decoration`, font selection and per-character fallback
//! through [`crate::fonts::FontSet`], shaping (kerning, ligatures) and glyph outlines through font_core (SR48).
//! Glyphs are drawn as paths with the fill/stroke machinery, so gradients, patterns and strokes apply to text.

use crate::fonts::FontSet;
use crate::geom::{Path, Rect, Transform};
use crate::raster::{Mask, Pixmap, fill_coverage};
use crate::render::{Ctx, Renderer};
use crate::style::{self, Anchor, Axis, Style, parse_length_list};
use crate::xml::Kind;
use alloc::string::String;
use alloc::vec::Vec;
use font_core::{OutlineSink, ShapeOptions};

#[derive(Clone)]
struct Ch {
    c: char,
    style: usize,
    x: Option<f64>,
    y: Option<f64>,
    dx: f64,
    dy: f64,
    rotate: f64,
    decor: u8,
    decor_style: usize,
}

struct Frame {
    xs: Vec<f64>,
    ys: Vec<f64>,
    dxs: Vec<f64>,
    dys: Vec<f64>,
    rots: Vec<f64>,
    count: usize,
}

struct Collector<'r, 'a> {
    r: &'r Renderer<'a>,
    ctx: &'r Ctx,
    styles: Vec<Style>,
    chars: Vec<Ch>,
    frames: Vec<Frame>,
}

impl Collector<'_, '_> {
    fn lens(&self, node: usize, a: &str, axis: Axis, st: &Style) -> Vec<f64> {
        self.r.doc.nodes[node]
            .attr(a)
            .and_then(parse_length_list)
            .map(|v| v.into_iter().map(|l| self.r.len(l, axis, self.ctx, st)).collect())
            .unwrap_or_default()
    }

    fn push_char(&mut self, c: char, style: usize, decor: u8, decor_style: usize) {
        let (mut x, mut y, mut dx, mut dy, mut rot) = (None, None, None, None, None);
        for f in self.frames.iter().rev() {
            let k = f.count;
            if x.is_none() && k < f.xs.len() {
                x = Some(f.xs[k]);
            }
            if y.is_none() && k < f.ys.len() {
                y = Some(f.ys[k]);
            }
            if dx.is_none() && k < f.dxs.len() {
                dx = Some(f.dxs[k]);
            }
            if dy.is_none() && k < f.dys.len() {
                dy = Some(f.dys[k]);
            }
            if rot.is_none() && !f.rots.is_empty() {
                rot = Some(f.rots[k.min(f.rots.len() - 1)]);
            }
        }
        for f in self.frames.iter_mut() {
            f.count += 1;
        }
        self.chars.push(Ch { c, style, x, y, dx: dx.unwrap_or(0.0), dy: dy.unwrap_or(0.0), rotate: rot.unwrap_or(0.0), decor, decor_style });
    }

    fn walk(&mut self, node: usize, parent_style: &Style, decor: u8, decor_style: usize, preserve_parent: bool) {
        let n = &self.r.doc.nodes[node];
        let st = Style::compute(parent_style, &self.r.props[node]);
        if st.display_none {
            return;
        }
        let preserve = st.preserve_space || (preserve_parent && style::get(&self.r.props[node], "xml:space").is_none());
        let sidx = self.styles.len();
        self.styles.push(st.clone());
        let (decor, decor_style) = if st.decoration != 0 { (st.decoration | decor, sidx) } else { (decor, decor_style) };
        let frame = Frame {
            xs: self.lens(node, "x", Axis::X, &st),
            ys: self.lens(node, "y", Axis::Y, &st),
            dxs: self.lens(node, "dx", Axis::X, &st),
            dys: self.lens(node, "dy", Axis::Y, &st),
            rots: n.attr("rotate").and_then(crate::geom::number_list).unwrap_or_default(),
            count: 0,
        };
        self.frames.push(frame);
        let kids = n.children.clone();
        for k in kids {
            match &self.r.doc.nodes[k].kind {
                Kind::Text(t) => {
                    let t = t.clone();
                    for c in t.chars() {
                        let c = match c {
                            '\n' | '\r' if !preserve => continue,
                            '\n' | '\r' | '\t' => ' ',
                            c => c,
                        };
                        if c == ' ' && !preserve {
                            // Collapse runs, drop leading spaces.
                            match self.chars.last() {
                                None => continue,
                                Some(l) if l.c == ' ' => continue,
                                _ => {}
                            }
                        }
                        self.push_char(c, sidx, decor, decor_style);
                    }
                }
                Kind::Element { .. } => {
                    let kn = &self.r.doc.nodes[k];
                    if kn.is_svg("tspan") || kn.is_svg("a") {
                        self.walk(k, &st, decor, decor_style, preserve);
                    }
                }
                _ => {}
            }
        }
        self.frames.pop();
    }
}

/// One drawable run: glyph outlines in user space and the style that paints them.
pub struct Run {
    pub style: usize,
    pub path: Path,
    pub decorations: Vec<(u8, usize, Path)>,
}

pub struct Layout {
    pub styles: Vec<Style>,
    pub runs: Vec<Run>,
    pub bbox: Option<Rect>,
}

struct Sink {
    t: Transform,
    path: Path,
}

impl OutlineSink for Sink {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.t.apply(x as f64, y as f64);
        self.path.move_to(p.0, p.1);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.t.apply(x as f64, y as f64);
        self.path.line_to(p.0, p.1);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let a = self.t.apply(x1 as f64, y1 as f64);
        let p = self.t.apply(x as f64, y as f64);
        self.path.quad_to(a, p);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let a = self.t.apply(x1 as f64, y1 as f64);
        let b = self.t.apply(x2 as f64, y2 as f64);
        let p = self.t.apply(x as f64, y as f64);
        self.path.cubic_to(a, b, p);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

struct Placed {
    ch: usize,
    face: usize,
    glyph: u16,
    x: f64,
    y: f64,
    advance: f64,
    chunk: usize,
}

fn baseline_offset(st: &Style, font: &font_core::Font) -> f64 {
    let s = st.font_size / font.units_per_em as f64;
    let (asc, desc, _) = font.line_metrics();
    let (asc, desc) = (asc as f64 * s, desc as f64 * s);
    let xh = font.os2.and_then(|o| o.x_height).map(|v| v as f64 * s).unwrap_or(asc * 0.5);
    let mut y = match st.dominant_baseline.as_deref() {
        Some("middle") => xh / 2.0,
        Some("central") => (asc + desc) / 2.0,
        Some("hanging") => asc * 0.8,
        Some("mathematical") => asc * 0.5,
        Some("text-before-edge") | Some("text-top") | Some("before-edge") => asc,
        Some("text-after-edge") | Some("text-bottom") | Some("after-edge") | Some("ideographic") => desc,
        _ => 0.0,
    };
    match st.baseline_shift.as_deref() {
        Some("sub") => y += st.font_size / 5.0 + 1.0,
        Some("super") => y -= st.font_size / 3.0 + 1.0,
        Some("baseline") | None => {}
        Some(v) => {
            if let Some(l) = style::parse_length(v) {
                let d = if l.unit == style::Unit::Percent { l.v / 100.0 * st.font_size } else { style::resolve(l, Axis::X, 0.0, 0.0, st.font_size) };
                y -= d;
            }
        }
    }
    y
}

pub fn layout(r: &Renderer, node: usize, ctx: &Ctx, parent: &Style) -> Option<Layout> {
    let fonts: &FontSet = r.opts.fonts?;
    let mut col = Collector { r, ctx, styles: Vec::new(), chars: Vec::new(), frames: Vec::new() };
    col.walk(node, parent, 0, 0, false);
    // Trailing space (default white-space handling).
    let root_preserve = col.styles.first().map(|s| s.preserve_space).unwrap_or(false);
    if !root_preserve {
        while col.chars.last().map(|c| c.c == ' ' && !col.styles[c.style].preserve_space).unwrap_or(false) {
            col.chars.pop();
        }
    }
    let styles = col.styles;
    let chars = col.chars;
    if chars.is_empty() {
        return Some(Layout { styles, runs: Vec::new(), bbox: None });
    }
    // Font face per character (with fallback).
    let mut faces = Vec::with_capacity(chars.len());
    let mut primary_cache: Vec<Option<usize>> = alloc::vec![None; styles.len()];
    for ch in &chars {
        let st = &styles[ch.style];
        let p = match primary_cache[ch.style] {
            Some(p) => Some(p),
            None => {
                let p = fonts.select(&st.font_family, st.font_weight, st.italic);
                primary_cache[ch.style] = p;
                p
            }
        };
        let Some(p) = p else { return None };
        let has = fonts.faces[p].font().map(|f| f.glyph_index(ch.c) != 0).unwrap_or(false);
        let f = if has || ch.c == ' ' || ch.c.is_control() { p } else { fonts.fallback_for(ch.c, st.font_weight, st.italic, p).unwrap_or(p) };
        faces.push(f);
    }
    // Runs: same style + face, no explicit positioning except on the first character.
    let mut placed: Vec<Placed> = Vec::new();
    let (mut px, mut py) = (0.0f64, 0.0f64);
    let mut chunk = 0usize;
    let mut chunk_starts: Vec<(usize, f64)> = Vec::new(); // (first placed index, start x)
    let mut i = 0;
    while i < chars.len() {
        let mut j = i + 1;
        while j < chars.len()
            && chars[j].style == chars[i].style
            && faces[j] == faces[i]
            && chars[j].x.is_none()
            && chars[j].y.is_none()
            && chars[j].dx == 0.0
            && chars[j].dy == 0.0
            && chars[j].rotate == chars[i].rotate
        {
            j += 1;
        }
        let first = &chars[i];
        if first.x.is_some() || first.y.is_some() || i == 0 {
            if let Some(x) = first.x {
                px = x;
            }
            if let Some(y) = first.y {
                py = y;
            }
            if i != 0 {
                chunk += 1;
            }
            chunk_starts.push((placed.len(), px));
        }
        px += first.dx;
        py += first.dy;
        let st = &styles[first.style];
        let face = &fonts.faces[faces[i]];
        let Some(font) = face.font() else {
            i = j;
            continue;
        };
        let scale = st.font_size / font.units_per_em as f64;
        let text: String = chars[i..j].iter().map(|c| c.c).collect();
        let byte_to_char: Vec<usize> = {
            let mut v = alloc::vec![0usize; text.len() + 1];
            for (ci, (b, _)) in text.char_indices().enumerate() {
                v[b] = ci;
            }
            v
        };
        let opts = ShapeOptions { kerning: st.kerning, ligatures: st.letter_spacing == 0.0 };
        let glyphs = font_core::shape(&font, &text, &opts);
        let bo = baseline_offset(st, &font);
        for (gi, g) in glyphs.iter().enumerate() {
            let ci = i + byte_to_char[g.cluster.min(text.len())];
            // Letter spacing once per cluster (after its last glyph).
            let cluster_end = glyphs.get(gi + 1).map(|n| n.cluster != g.cluster).unwrap_or(true);
            let mut adv = g.x_advance as f64 * scale;
            if cluster_end {
                adv += st.letter_spacing;
                if chars[ci].c == ' ' {
                    adv += st.word_spacing;
                }
            }
            placed.push(Placed { ch: ci, face: faces[i], glyph: g.glyph, x: px + g.x_offset as f64 * scale, y: py - g.y_offset as f64 * scale + bo, advance: adv, chunk });
            px += adv;
        }
        i = j;
    }
    // text-anchor per chunk.
    let nchunks = chunk + 1;
    let mut chunk_end = alloc::vec![0.0f64; nchunks];
    let mut chunk_begin = alloc::vec![f64::INFINITY; nchunks];
    let mut chunk_anchor = alloc::vec![Anchor::Start; nchunks];
    let mut seen = alloc::vec![false; nchunks];
    for p in &placed {
        let c = p.chunk;
        if !seen[c] {
            seen[c] = true;
            chunk_anchor[c] = styles[chars[p.ch].style].anchor;
            chunk_begin[c] = p.x;
        }
        chunk_begin[c] = chunk_begin[c].min(p.x);
        chunk_end[c] = chunk_end[c].max(p.x + p.advance);
    }
    let _ = chunk_starts;
    for p in placed.iter_mut() {
        let c = p.chunk;
        let w = chunk_end[c] - chunk_begin[c];
        match chunk_anchor[c] {
            Anchor::Middle => p.x -= w / 2.0,
            Anchor::End => p.x -= w,
            Anchor::Start => {}
        }
    }
    // Outlines into runs (one per style), decorations, and the cell bbox.
    let mut runs: Vec<Run> = Vec::new();
    let mut bounds = crate::geom::Bounds::new();
    let mut any = false;
    for p in &placed {
        let ch = &chars[p.ch];
        let st = &styles[ch.style];
        let face = &fonts.faces[p.face];
        let Some(font) = face.font() else { continue };
        let s = st.font_size / font.units_per_em as f64;
        let rot = Transform::rotate(ch.rotate);
        let gt = Transform::translate(p.x, p.y).mul(&rot).mul(&Transform::scale(s, -s));
        let (asc, desc, _) = font.line_metrics();
        let cell = [(0.0, asc as f64), (p.advance / s, asc as f64), (p.advance / s, desc as f64), (0.0, desc as f64)];
        for c in cell {
            bounds.add(gt.apply(c.0, c.1));
            any = true;
        }
        let mut sink = Sink { t: gt, path: Path::new() };
        font.outline(p.glyph, &mut sink);
        let run = match runs.last_mut() {
            Some(r) if r.style == ch.style => r,
            _ => {
                runs.push(Run { style: ch.style, path: Path::new(), decorations: Vec::new() });
                runs.last_mut().unwrap()
            }
        };
        run.path.extend(&sink.path);
        if ch.decor != 0 {
            let dst = &styles[ch.decor_style];
            let ds = dst.font_size / font.units_per_em as f64;
            let ut = font.post.map(|p| p.underline_thickness as f64).filter(|&v| v > 0.0).unwrap_or(font.units_per_em as f64 / 14.0) * ds;
            let up = font.post.map(|p| p.underline_position as f64).unwrap_or(-(font.units_per_em as f64) / 10.0) * ds;
            let lines: [(u8, f64); 3] = [
                (style::DECOR_UNDERLINE, -up),
                (style::DECOR_OVERLINE, -(asc as f64) * ds),
                (style::DECOR_LINE_THROUGH, -(font.os2.and_then(|o| o.x_height).unwrap_or(asc / 2) as f64) * ds / 2.0),
            ];
            for (bit, off) in lines {
                if ch.decor & bit == 0 {
                    continue;
                }
                let base = Transform::translate(p.x, p.y).mul(&rot);
                let mut d = Path::new();
                let (y0, y1) = (off - ut / 2.0, off + ut / 2.0);
                let pts = [(0.0, y0), (p.advance, y0), (p.advance, y1), (0.0, y1)];
                let q: Vec<(f64, f64)> = pts.iter().map(|&(x, y)| base.apply(x, y)).collect();
                d.move_to(q[0].0, q[0].1);
                d.line_to(q[1].0, q[1].1);
                d.line_to(q[2].0, q[2].1);
                d.line_to(q[3].0, q[3].1);
                d.close();
                run.decorations.push((bit, ch.decor_style, d));
            }
        }
    }
    Some(Layout { styles, runs, bbox: if any { bounds.rect() } else { None } })
}

pub fn render_text(r: &mut Renderer, node: usize, ctx: &Ctx, parent: &Style, canvas: &mut Pixmap) {
    let Some(lay) = layout(r, node, ctx, parent) else { return };
    let bbox = lay.bbox;
    for run in &lay.runs {
        let rs = &lay.styles[run.style];
        if !rs.visible {
            continue;
        }
        // Underline and overline under the text, line-through over it (CSS Text Decoration 3 §3).
        for (bit, ds, d) in &run.decorations {
            if *bit != style::DECOR_LINE_THROUGH {
                let dst = lay.styles[*ds].clone();
                r.fill_path(d, &dst, bbox, ctx, canvas);
                r.stroke_path(d, &dst, bbox, ctx, canvas);
            }
        }
        let rs = rs.clone();
        if rs.stroke_first {
            r.stroke_path(&run.path, &rs, bbox, ctx, canvas);
            r.fill_path(&run.path, &rs, bbox, ctx, canvas);
        } else {
            r.fill_path(&run.path, &rs, bbox, ctx, canvas);
            r.stroke_path(&run.path, &rs, bbox, ctx, canvas);
        }
        for (bit, ds, d) in &run.decorations {
            if *bit == style::DECOR_LINE_THROUGH {
                let dst = lay.styles[*ds].clone();
                r.fill_path(d, &dst, bbox, ctx, canvas);
                r.stroke_path(d, &dst, bbox, ctx, canvas);
            }
        }
    }
}

pub fn text_bbox(r: &mut Renderer, node: usize, ctx: &Ctx, _st: &Style, t: &Transform) -> Option<Rect> {
    let lay = layout(r, node, ctx, &ctx.style)?;
    lay.bbox.map(|b| b.transform(t))
}

pub fn clip_text(r: &mut Renderer, node: usize, ctx: &Ctx, mask: &mut Mask) {
    let Some(lay) = layout(r, node, ctx, &ctx.style) else { return };
    for run in &lay.runs {
        let rs = &lay.styles[run.style];
        if !rs.visible {
            continue;
        }
        if let Some(c) = fill_coverage(&run.path.transform(&ctx.ts), rs.clip_rule, r.w, r.h, rs.aa) {
            mask.union_coverage(&c);
        }
    }
}
