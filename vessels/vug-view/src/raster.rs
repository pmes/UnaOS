// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The software rasteriser (VUGFACE1, LEDGER SR470): the honest first face, measured — quartzite's gpu_view forwards
//! no input to a vessel and euclase has no depth / lit-mesh pipeline yet (VUG-FACE-GPU owed). One pass over the model's
//! f32 buffers: back-face cull, edge-function fill with a perspective-correct 1/z buffer, flat shading (one
//! camera-relative key light, a hemisphere ambient, silhouette darkening), the picked triangle in the accent; then the
//! ground grid (10 mm lines, 1 mm sub-lines, footprint-weighted and faded with distance, depth-tested against the
//! model); then the axis triad. Every colour is palette_core's (the dark rows).

use crate::model::Model;
use crate::view::View;
use euclase::Vec3;

/// The viewport background: `surface.content`, dark.
pub const BACKGROUND: u32 = palette_core::SURFACE_CONTENT.1;
/// The model's albedo: `text.secondary`, dark (a neutral light grey that reads on the background).
pub const ALBEDO: u32 = palette_core::TEXT_SECONDARY.1;
/// The picked triangle: the default accent (crispy).
pub const ACCENT: u32 = palette_core::ACCENTS[0];
/// Grid ink: `line.strong` for the 10 mm lines, `line.subtle` for the 1 mm sub-lines (dark rows).
pub const GRID_MAJOR: u32 = palette_core::LINE_STRONG.1;
pub const GRID_MINOR: u32 = palette_core::LINE_SUBTLE.1;
/// The triad: X clay, Y moss, Z crispy.
pub const AXIS: [u32; 3] = [palette_core::ACCENTS[4], palette_core::ACCENTS[2], palette_core::ACCENTS[0]];

const MAJOR_MM: f32 = 10.0;
const MINOR_MM: f32 = 1.0;

/// A rendered frame: straight RGBA (top row first) plus the 1/z buffer and the triangle id under each pixel.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// 1 / view depth of the nearest triangle (0 = empty).
    pub inv_z: Vec<f32>,
    /// The triangle drawn at each pixel (`u32::MAX` = none).
    pub id: Vec<u32>,
}

impl Frame {
    /// The RGBA of pixel (x, y).
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// An opaque RGBA from a palette `0x00RRGGBB`.
pub fn rgba(c: u32) -> [u8; 4] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]
}

fn v3(p: &[f32], i: u32) -> Vec3 {
    let k = i as usize * 3;
    Vec3::new(p[k], p[k + 1], p[k + 2])
}

/// The triangle's corners, or `None` for an index out of range.
fn corners(model: &Model, tri: u32) -> Option<[Vec3; 3]> {
    let idx = model.indices();
    let p = model.positions();
    let k = tri as usize * 3;
    let t = idx.get(k..k + 3)?;
    if t.iter().any(|&i| (i as usize + 1) * 3 > p.len()) {
        return None;
    }
    Some([v3(p, t[0]), v3(p, t[1]), v3(p, t[2])])
}

/// The flat shade of a face with unit normal `n` seen along unit `dir` (eye -> face), as RGBA: hemisphere ambient
/// (ground 0.22 to sky 0.38 on n.Z) + a key light from the camera's upper left (0.62 n.L) times the silhouette term
/// 1 - 0.45 (1 - |n.dir|)^2 (grazing faces darken), on `ALBEDO`.
pub fn shade(view: &View, n: Vec3, dir: Vec3) -> [u8; 4] {
    let (r, u, f) = view.camera.basis();
    let light = (r * -0.45 + u * 0.75 - f * 0.5).normalize();
    let hemi = 0.22 + 0.16 * (n.z * 0.5 + 0.5);
    let key = 0.62 * n.dot(light).max(0.0);
    let g = 1.0 - n.dot(dir).abs();
    let sil = 1.0 - 0.45 * g * g;
    let k = (hemi + key) * sil;
    let a = rgba(ALBEDO);
    let c = [0, 1, 2].map(|i| (a[i] as f32 * k).round().clamp(0.0, 255.0) as u8);
    [c[0], c[1], c[2], 255]
}

/// The colour `render` paints triangle `tri` with (its flat shade, or the accent when picked); `None` when the
/// triangle is degenerate or out of range.
pub fn tri_colour(model: &Model, view: &View, tri: u32) -> Option<[u8; 4]> {
    if view.picked == Some(tri) {
        return Some(rgba(ACCENT));
    }
    let [a, b, c] = corners(model, tri)?;
    let n = (b - a).cross(c - a).normalize();
    if n == Vec3::zero() {
        return None;
    }
    let centroid = (a + b + c) / 3.0;
    let dir = (centroid - view.camera.eye).normalize();
    Some(shade(view, n, dir))
}

/// Render the model under the view.
pub fn render(model: &Model, view: &View) -> Frame {
    // VUGFACE2 (SR485): the design's body (or the pull's preview) replaces the opened mesh.
    let shown = view.design.shown();
    let model = shown.as_deref().unwrap_or(model);
    let (w, h) = view.size;
    let n = w as usize * h as usize;
    let bg = rgba(BACKGROUND);
    let mut fr = Frame { width: w, height: h, rgba: bg.repeat(n), inv_z: vec![0.0; n], id: vec![u32::MAX; n] };
    fill(model, view, &mut fr);
    grid(model, view, &mut fr);
    // VUGFACEPHOTO (SR506): the rectified photo under the sketch, dimmed, and the outline's sigma band.
    crate::photo::underlay(view, &mut fr);
    triad(view, &mut fr);
    // VUGFACE2 (SR485): the sketch overlay (profile tint, entities, glyphs, dof) over everything.
    view.overlay(&mut fr);
    fr
}

/// One projected corner: surface x, y and 1 / view depth.
#[derive(Clone, Copy)]
struct Sv {
    x: f32,
    y: f32,
    iz: f32,
}

fn fill(model: &Model, view: &View, fr: &mut Frame) {
    let vm = view.camera.view();
    let eye = view.camera.eye;
    let fp = view.focal_px();
    let (cx, cy) = (fr.width as f32 * 0.5, fr.height as f32 * 0.5);
    let near = model.radius() * 1e-3;
    let pos = model.positions();
    // Every vertex once: view space (euclase's look-at: the camera looks down -Z) to the surface.
    let proj: Vec<Option<Sv>> = (0..pos.len() / 3)
        .map(|i| {
            let p = vm.transform_point3(v3(pos, i as u32));
            let z = -p.z;
            (z > near).then(|| Sv { x: cx + p.x / z * fp, y: cy - p.y / z * fp, iz: 1.0 / z })
        })
        .collect();
    let idx = model.indices();
    for t in 0..idx.len() / 3 {
        let [i0, i1, i2] = [idx[t * 3], idx[t * 3 + 1], idx[t * 3 + 2]];
        let (Some(Some(a)), Some(Some(b)), Some(Some(c))) = (proj.get(i0 as usize), proj.get(i1 as usize), proj.get(i2 as usize)) else {
            continue; // a corner behind the near plane (no clipping: the triangle is dropped) or out of range
        };
        // Back faces off, decided in world space (independent of the surface's y-down handedness).
        let Some([wa, wb, wc]) = corners(model, t as u32) else { continue };
        let nrm = (wb - wa).cross(wc - wa);
        if nrm.dot(wa - eye) >= 0.0 {
            continue;
        }
        let Some(col) = tri_colour(model, view, t as u32) else { continue };
        raster_tri(fr, [*a, *b, *c], col, t as u32);
    }
}

fn edge(a: Sv, b: Sv, x: f32, y: f32) -> f32 {
    (b.x - a.x) * (y - a.y) - (b.y - a.y) * (x - a.x)
}

fn raster_tri(fr: &mut Frame, v: [Sv; 3], col: [u8; 4], id: u32) {
    let [a, mut b, mut c] = v;
    let mut area = edge(a, b, c.x, c.y);
    if area < 0.0 {
        core::mem::swap(&mut b, &mut c);
        area = -area;
    }
    if area <= f32::EPSILON {
        return;
    }
    let (w, h) = (fr.width as i32, fr.height as i32);
    let x0 = (a.x.min(b.x).min(c.x).floor() as i32).max(0);
    let x1 = (a.x.max(b.x).max(c.x).ceil() as i32).min(w - 1);
    let y0 = (a.y.min(b.y).min(c.y).floor() as i32).max(0);
    let y1 = (a.y.max(b.y).max(c.y).ceil() as i32).min(h - 1);
    if x0 > x1 || y0 > y1 {
        return;
    }
    let inv_area = 1.0 / area;
    for y in y0..=y1 {
        let py = y as f32 + 0.5;
        let row = y as usize * fr.width as usize;
        for x in x0..=x1 {
            let px = x as f32 + 0.5;
            let w0 = edge(b, c, px, py);
            let w1 = edge(c, a, px, py);
            let w2 = edge(a, b, px, py);
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let iz = (w0 * a.iz + w1 * b.iz + w2 * c.iz) * inv_area;
            let i = row + x as usize;
            if iz > fr.inv_z[i] {
                fr.inv_z[i] = iz;
                fr.id[i] = id;
                fr.rgba[i * 4..i * 4 + 4].copy_from_slice(&col);
            }
        }
    }
}

/// The coverage of a line family of spacing `s` at coordinate `c` for a pixel footprint `fw` (world units).
fn line_cover(c: f32, s: f32, fw: f32) -> f32 {
    let q = c / s;
    let d = (q - q.round()).abs() * s;
    (1.0 - d / (fw * 0.75).max(1e-6)).clamp(0.0, 1.0)
}

fn blend(px: &mut [u8], ink: [u8; 4], a: f32) {
    for k in 0..3 {
        px[k] = (px[k] as f32 + (ink[k] as f32 - px[k] as f32) * a).round() as u8;
    }
}

/// The ground grid on the plane z = the model's lowest z, under the model's depth.
fn grid(model: &Model, view: &View, fr: &mut Frame) {
    let z0 = model.bounds().0[2];
    let eye = view.camera.eye;
    let (r, u, f) = view.camera.basis();
    let fp = view.focal_px();
    let (cx, cy) = (fr.width as f32 * 0.5, fr.height as f32 * 0.5);
    let (major, minor) = (rgba(GRID_MAJOR), rgba(GRID_MINOR));
    for y in 0..fr.height {
        let b = -(y as f32 + 0.5 - cy) / fp;
        for x in 0..fr.width {
            let a = (x as f32 + 0.5 - cx) / fp;
            let d = f + r * a + u * b; // d . f = 1: the ray parameter is the view depth
            if d.z.abs() < 1e-6 {
                continue;
            }
            let t = (z0 - eye.z) / d.z;
            if t <= 0.0 {
                continue;
            }
            let i = (y * fr.width + x) as usize;
            if 1.0 / t <= fr.inv_z[i] {
                continue; // the model is in front
            }
            let p = eye + d * t;
            // The pixel's footprint on the ground: depth / focal, stretched at grazing angles.
            let slope = (d.z.abs() / d.magnitude()).max(0.08);
            let fw = t / fp / slope;
            let a_minor = (1.0 - fw / (0.5 * MINOR_MM)).clamp(0.0, 1.0);
            let a_major = (1.0 - fw / (0.5 * MAJOR_MM)).clamp(0.0, 1.0);
            let cmaj = line_cover(p.x, MAJOR_MM, fw).max(line_cover(p.y, MAJOR_MM, fw)) * a_major;
            let cmin = line_cover(p.x, MINOR_MM, fw).max(line_cover(p.y, MINOR_MM, fw)) * a_minor;
            let px = &mut fr.rgba[i * 4..i * 4 + 4];
            if cmin > 0.0 {
                blend(px, minor, cmin * 0.8);
            }
            if cmaj > 0.0 {
                blend(px, major, cmaj * 0.9);
            }
        }
    }
}

/// The axis triad in the lower-left corner: the world axes turned by the camera, X / Y / Z lines from one origin.
fn triad(view: &View, fr: &mut Frame) {
    let (r, u, _) = view.camera.basis();
    let len = (fr.width.min(fr.height) as f32 * 0.1).max(8.0);
    let o = (len + 6.0, fr.height as f32 - len - 6.0);
    let axes = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)];
    for (k, ax) in axes.iter().enumerate() {
        let e = (o.0 + ax.dot(r) * len, o.1 - ax.dot(u) * len);
        line(fr, o, e, rgba(AXIS[k]));
    }
}

/// A 1-pixel line (DDA), clipped to the frame (public for the sketch overlay, VUGFACE2 SR485).
pub fn line(fr: &mut Frame, a: (f32, f32), b: (f32, f32), col: [u8; 4]) {
    let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil().max(1.0) as i32;
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let (x, y) = ((a.0 + (b.0 - a.0) * t) as i32, (a.1 + (b.1 - a.1) * t) as i32);
        if x >= 0 && y >= 0 && (x as u32) < fr.width && (y as u32) < fr.height {
            let i = (y as u32 * fr.width + x as u32) as usize * 4;
            fr.rgba[i..i + 4].copy_from_slice(&col);
        }
    }
}

// ---- VUGFACE2 (SR485): the sketch overlay's primitives: the profile tint and the glyph font ----

/// Tint the region the closed `rings` bound (even-odd over all of them, so a hole stays clear): `col` blended at
/// `alpha` into every pixel whose centre is inside, clipped to the frame.
pub fn tint(fr: &mut Frame, rings: &[Vec<(f32, f32)>], col: [u8; 4], alpha: f32) {
    let (w, h) = (fr.width as i32, fr.height as i32);
    let mut ys = rings.iter().flatten().map(|p| p.1);
    let Some(first) = ys.next() else { return };
    let (lo, hi) = ys.fold((first, first), |(a, b), y| (a.min(y), b.max(y)));
    if !(lo.is_finite() && hi.is_finite()) {
        return;
    }
    let y0 = (lo.floor() as i32).max(0);
    let y1 = (hi.ceil() as i32).min(h - 1);
    let mut xs: Vec<f32> = Vec::new();
    for y in y0..=y1 {
        let py = y as f32 + 0.5;
        xs.clear();
        for r in rings {
            let n = r.len();
            for i in 0..n {
                let (a, b) = (r[i], r[(i + 1) % n]);
                if (a.1 <= py) != (b.1 <= py) {
                    xs.push(a.0 + (py - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        for pair in xs.chunks(2) {
            if pair.len() < 2 {
                break;
            }
            let xa = ((pair[0] - 0.5).ceil() as i32).max(0);
            let xb = ((pair[1] - 0.5).floor() as i32).min(w - 1);
            for x in xa..=xb {
                let i = (y * w + x) as usize * 4;
                blend(&mut fr.rgba[i..i + 4], col, alpha);
            }
        }
    }
}

/// A 3x5 bitmap per character (bit 4 the left column): digits, the constraint marks and the dof label's letters.
const FONT: &[(char, [u8; 5])] = &[
    ('0', [7, 5, 5, 5, 7]),
    ('1', [2, 6, 2, 2, 7]),
    ('2', [7, 1, 7, 4, 7]),
    ('3', [7, 1, 7, 1, 7]),
    ('4', [5, 5, 7, 1, 1]),
    ('5', [7, 4, 7, 1, 7]),
    ('6', [7, 4, 7, 5, 7]),
    ('7', [7, 1, 1, 1, 1]),
    ('8', [7, 5, 7, 5, 7]),
    ('9', [7, 5, 7, 1, 7]),
    ('H', [5, 5, 7, 5, 5]),
    ('V', [5, 5, 5, 5, 2]),
    ('=', [0, 7, 0, 7, 0]),
    ('⟂', [2, 2, 2, 2, 7]),
    ('T', [7, 2, 2, 2, 2]),
    ('M', [5, 7, 7, 5, 5]),
    ('o', [0, 2, 5, 2, 0]),
    ('d', [1, 1, 7, 5, 7]),
    ('f', [3, 2, 7, 2, 2]),
    ('!', [2, 2, 2, 0, 2]),
    // VUGFACEDIMENSION (SR494): a dimension's value (the point, the sign, the degree, R / D) and the refusal words.
    ('.', [0, 0, 0, 0, 2]),
    ('-', [0, 0, 7, 0, 0]),
    ('°', [2, 5, 2, 0, 0]),
    ('A', [2, 5, 7, 5, 5]),
    ('C', [7, 4, 4, 4, 7]),
    ('D', [6, 5, 5, 5, 6]),
    ('E', [7, 4, 6, 4, 7]),
    ('F', [7, 4, 6, 4, 4]),
    ('G', [7, 4, 5, 5, 7]),
    ('I', [7, 2, 2, 2, 7]),
    ('L', [4, 4, 4, 4, 7]),
    ('N', [6, 5, 5, 5, 5]),
    ('O', [2, 5, 5, 5, 2]),
    ('R', [6, 5, 6, 5, 5]),
    ('U', [5, 5, 5, 5, 7]),
    // VUGFACEPHOTO (SR506): the rest of the capitals and the statement / refusal punctuation (a proposal's text and
    // a refusal's name are drawn upper-cased).
    // VUGFACETREE (SR513): the tree panel's statements are drawn upper-cased — the rest of the capitals and the
    // statement punctuation (VUGFACEPHOTO adds the same sixteen B..',' with the same bitmaps: the fold keeps ONE copy)
    // and the list brackets, the edit's mark and the plus (this arc's).
    ('B', [6, 5, 6, 5, 6]),
    ('J', [1, 1, 1, 5, 7]),
    ('K', [5, 5, 6, 5, 5]),
    ('P', [6, 5, 6, 4, 4]),
    ('Q', [2, 5, 5, 6, 3]),
    ('S', [3, 4, 2, 1, 6]),
    ('W', [5, 5, 7, 7, 5]),
    ('X', [5, 5, 2, 5, 5]),
    ('Y', [5, 5, 2, 2, 2]),
    ('Z', [7, 1, 2, 4, 7]),
    (':', [0, 2, 0, 2, 0]),
    ('_', [0, 0, 0, 0, 7]),
    ('/', [1, 1, 2, 4, 4]),
    ('(', [1, 2, 2, 2, 1]),
    (')', [4, 2, 2, 2, 4]),
    (',', [0, 0, 0, 2, 4]),
    ('[', [3, 2, 2, 2, 3]),
    (']', [6, 2, 2, 2, 6]),
    ('>', [4, 2, 1, 2, 4]),
    ('+', [0, 2, 7, 2, 0]),
];

/// One character of the bitmap font at (x, y) (its top-left), each bit a 2x2 block; an unknown character (and the
/// space) draws nothing. Returns the advance (8 px).
pub fn glyph(fr: &mut Frame, ch: char, x: f32, y: f32, col: [u8; 4]) -> f32 {
    if let Some((_, rows)) = FONT.iter().find(|(c, _)| *c == ch) {
        let (x0, y0) = (x.round() as i32, y.round() as i32);
        for (r, bits) in rows.iter().enumerate() {
            for c in 0..3 {
                if bits & (4 >> c) == 0 {
                    continue;
                }
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (px, py) = (x0 + c * 2 + dx, y0 + r as i32 * 2 + dy);
                    if px >= 0 && py >= 0 && (px as u32) < fr.width && (py as u32) < fr.height {
                        let i = (py as u32 * fr.width + px as u32) as usize * 4;
                        fr.rgba[i..i + 4].copy_from_slice(&col);
                    }
                }
            }
        }
    }
    8.0
}

/// A string of the bitmap font from (x, y).
pub fn text(fr: &mut Frame, s: &str, x: f32, y: f32, col: [u8; 4]) {
    let mut at = x;
    for ch in s.chars() {
        at += glyph(fr, ch, at, y, col);
    }
}
