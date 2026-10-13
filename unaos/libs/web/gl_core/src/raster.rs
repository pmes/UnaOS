//! Drawing (ES 2.0 §2.6–§2.12, Chapter 3, §4.1–§4.3): drawArrays / drawElements, clear, readPixels and the
//! drawing buffer's presentation.
//!
//! A draw: validate (WebGL's range checking, framebuffer completeness, sampler-unit and stencil-face
//! agreement), take the render target's storage out of its objects (so writing and sampling never alias),
//! shade every referenced vertex once, assemble primitives (§2.6.1), clip in clip space (§2.13: triangles by
//! Sutherland–Hodgman against the six planes, lines parametrically, points by their vertex), divide, map to
//! the viewport (§2.12.1), rasterize (§3.3 points, §3.4 lines, §3.5 polygons: 8 sub-pixel bits, integer edge
//! functions, pixel centres, the top-left rule, perspective-correct varyings by eq. 3.9), run the fragment
//! shader (with helper invocations for derivatives), and apply the per-fragment operations of §4.1 in order:
//! scissor, stencil, depth, blending, the colour mask, quantization to the target's bits. Dithering is off.

use crate::consts::*;
use crate::context::{Context, DrawStats, State, StencilFace, POINT_SIZE_MAX};
use crate::glsl::codegen::StageCode;
use crate::link::Linked;
use crate::object::Attach;
use crate::texture::{self, Texture};
use crate::vm::{self, Lod, Mode, Outcome, Sampler};
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

/// A colour buffer's storage format (every format is held as RGBA8 and quantized on write).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fmt {
    Rgba8,
    Rgb8,
    Rgba4,
    Rgb565,
    Rgb5a1,
}

pub fn fmt_of(format: u32, ty: u32) -> Fmt {
    match (format, ty) {
        (RGB, UNSIGNED_BYTE) => Fmt::Rgb8,
        (_, UNSIGNED_SHORT_4_4_4_4) => Fmt::Rgba4,
        (_, UNSIGNED_SHORT_5_6_5) => Fmt::Rgb565,
        (_, UNSIGNED_SHORT_5_5_5_1) => Fmt::Rgb5a1,
        _ => Fmt::Rgba8,
    }
}

fn rb_fmt(f: u32) -> Fmt {
    match f {
        RGBA4 => Fmt::Rgba4,
        RGB565 => Fmt::Rgb565,
        _ => Fmt::Rgb5a1,
    }
}

/// ES 2.0 §2.1.2 fixed-point conversion: round(c · (2^b − 1)), expanded back to 8 bits by bit replication.
pub fn quantize(c: [f32; 4], f: Fmt) -> [u8; 4] {
    let q = |v: f32, bits: u32| -> u8 {
        let max = ((1u32 << bits) - 1) as f32;
        let n = (v.clamp(0.0, 1.0) * max + 0.5) as u32;
        let e = (n << (8 - bits)) | (n >> (2 * bits).saturating_sub(8).min(bits));
        if bits == 8 { n as u8 } else { (e & 0xFF) as u8 }
    };
    match f {
        Fmt::Rgba8 => [q(c[0], 8), q(c[1], 8), q(c[2], 8), q(c[3], 8)],
        Fmt::Rgb8 => [q(c[0], 8), q(c[1], 8), q(c[2], 8), 255],
        Fmt::Rgba4 => [q(c[0], 4), q(c[1], 4), q(c[2], 4), q(c[3], 4)],
        Fmt::Rgb565 => [q(c[0], 5), q(c[1], 6), q(c[2], 5), 255],
        Fmt::Rgb5a1 => [q(c[0], 5), q(c[1], 5), q(c[2], 5), if c[3] >= 0.5 { 255 } else { 0 }],
    }
}

/// Where a render target's planes came from (to put them back).
#[derive(Clone, Copy, Debug)]
enum Origin {
    Default,
    Fbo([Attach; 4]),
}

pub(crate) struct Target {
    pub w: u32,
    pub h: u32,
    pub color: Vec<u8>,
    pub fmt: Fmt,
    pub has_color: bool,
    pub depth: Vec<u32>,
    /// 0 none, 16 or 24.
    pub depth_bits: u8,
    pub stencil: Vec<u8>,
    pub has_stencil: bool,
    origin: Origin,
}

/// The draw's view of the texture units: complete textures only (an incomplete one samples (0, 0, 0, 1)).
pub(crate) struct TexEnv<'a> {
    pub units: Vec<(Option<&'a Texture>, Option<&'a Texture>)>,
}

impl<'a> TexEnv<'a> {
    pub fn new(ctx: &'a Context) -> TexEnv<'a> {
        let pick = |n: u32| ctx.textures.get(n).filter(|t| t.complete());
        TexEnv { units: ctx.units.iter().map(|(a, b)| (pick(*a), pick(*b))).collect() }
    }
}

impl Sampler for TexEnv<'_> {
    fn sample(&self, unit: usize, cube: bool, coord: [f32; 3], lod: Lod) -> [f32; 4] {
        let Some(u) = self.units.get(unit) else { return [0.0, 0.0, 0.0, 1.0] };
        match if cube { u.1 } else { u.0 } {
            Some(t) => texture::sample(t, cube, coord, lod),
            None => [0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// Load a stage's constants and uniforms (and gl_DepthRange) into its register file.
fn preload(code: &StageCode, map: &[(u32, u16)], store: &[[f32; 4]], st: &State) -> Vec<[f32; 4]> {
    let mut regs = vec![[0.0f32; 4]; code.nregs.max(1)];
    for (r, v) in &code.consts {
        regs[*r as usize] = *v;
    }
    for (row, r) in map {
        regs[*r as usize] = store[*row as usize];
    }
    if let Some(d) = code.builtins.depth_range {
        let (n, f) = (st.depth_range[0], st.depth_range[1]);
        regs[d as usize] = [n; 4];
        regs[d as usize + 1] = [f; 4];
        regs[d as usize + 2] = [f - n; 4];
    }
    regs
}

#[derive(Clone, Default)]
struct FragIn {
    x: i32,
    y: i32,
    z: f32,
    invw: f32,
    front: bool,
    pc: [f32; 2],
    vars: Vec<[f32; 4]>,
}

/// A rasterized primitive's interpolation (window coordinates, y up).
enum Setup {
    Tri { p: [[f64; 2]; 3], z: [f64; 3], iw: [f64; 3], area: f64, base: [usize; 3], front: bool, offset: f64 },
    Line { p: [[f64; 2]; 2], z: [f64; 2], iw: [f64; 2], base: [usize; 2], xmajor: bool },
    Point { c: [f64; 2], z: f64, iw: f64, base: usize, size: f64 },
}

/// Vertex-stage outputs.
struct VOut {
    clip: [f32; 4],
    psize: f32,
}

struct Pipe<'a> {
    st: &'a State,
    l: &'a Linked,
    env: &'a TexEnv<'a>,
    regs: Vec<[f32; 4]>,
    t: &'a mut Target,
    stats: DrawStats,
    quad: bool,
    nv: usize,
    /// Vertex varyings, nv per vertex (flat), and window-space data per clipped vertex appended as needed.
    vars: Vec<[f32; 4]>,
    scissor: [i32; 4],
    log_x: Vec<[f32; 4]>,
    log_y: Vec<[f32; 4]>,
    fin: [FragIn; 3],
}

fn cmp(func: u32, a: u32, b: u32) -> bool {
    match func {
        NEVER => false,
        LESS => a < b,
        EQUAL => a == b,
        LEQUAL => a <= b,
        GREATER => a > b,
        NOTEQUAL => a != b,
        GEQUAL => a >= b,
        _ => true,
    }
}

fn stencil_op(op: u32, old: u8, reference: u8) -> u8 {
    match op {
        ZERO => 0,
        REPLACE => reference,
        INCR => old.saturating_add(1),
        DECR => old.saturating_sub(1),
        INVERT => !old,
        INCR_WRAP => old.wrapping_add(1),
        DECR_WRAP => old.wrapping_sub(1),
        _ => old,
    }
}

/// ES 2.0 Table 4.1/4.2: a blend factor (rgb factor in lanes 0..3, alpha factor in lane 3 when `alpha`).
fn factor(f: u32, s: &[f32; 4], d: &[f32; 4], c: &[f32; 4]) -> [f32; 4] {
    match f {
        ZERO => [0.0; 4],
        ONE => [1.0; 4],
        SRC_COLOR => *s,
        ONE_MINUS_SRC_COLOR => s.map(|v| 1.0 - v),
        DST_COLOR => *d,
        ONE_MINUS_DST_COLOR => d.map(|v| 1.0 - v),
        SRC_ALPHA => [s[3]; 4],
        ONE_MINUS_SRC_ALPHA => [1.0 - s[3]; 4],
        DST_ALPHA => [d[3]; 4],
        ONE_MINUS_DST_ALPHA => [1.0 - d[3]; 4],
        CONSTANT_COLOR => *c,
        ONE_MINUS_CONSTANT_COLOR => c.map(|v| 1.0 - v),
        CONSTANT_ALPHA => [c[3]; 4],
        ONE_MINUS_CONSTANT_ALPHA => [1.0 - c[3]; 4],
        SRC_ALPHA_SATURATE => {
            let f = s[3].min(1.0 - d[3]);
            [f, f, f, 1.0]
        }
        _ => [0.0; 4],
    }
}

/// ES 2.0 §4.1.6 blending of one fragment over the destination (all values in [0, 1]).
pub fn blend(st: &State, s: [f32; 4], d: [f32; 4]) -> [f32; 4] {
    let c = &st.blend_color;
    let sr = factor(st.blend_src[0], &s, &d, c);
    let sa = factor(st.blend_src[1], &s, &d, c);
    let dr = factor(st.blend_dst[0], &s, &d, c);
    let da = factor(st.blend_dst[1], &s, &d, c);
    let mut o = [0.0f32; 4];
    for i in 0..4 {
        let (fs, fd, eq) = if i < 3 { (sr[i], dr[i], st.blend_eq[0]) } else { (sa[3], da[3], st.blend_eq[1]) };
        let v = match eq {
            FUNC_SUBTRACT => s[i] * fs - d[i] * fd,
            FUNC_REVERSE_SUBTRACT => d[i] * fd - s[i] * fs,
            _ => s[i] * fs + d[i] * fd,
        };
        o[i] = v.clamp(0.0, 1.0);
    }
    o
}

/// Depth at a buffer's precision (24- or 16-bit fixed point, ES 2.0 §2.12.1).
fn depth_q(z: f32, bits: u8) -> u32 {
    let max = ((1u64 << bits) - 1) as f64;
    ((z.clamp(0.0, 1.0) as f64) * max + 0.5) as u32
}

impl Pipe<'_> {
    fn interp(&self, s: &Setup, px: f64, py: f64, out: &mut FragIn) {
        let nv = self.nv;
        out.vars.resize(nv, [0.0; 4]);
        match s {
            Setup::Tri { p, z, iw, area, base, front, offset } => {
                let e = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0]);
                let b0 = e(p[1], p[2]) / area;
                let b1 = e(p[2], p[0]) / area;
                let b2 = 1.0 - b0 - b1;
                let f = [b0 * iw[0], b1 * iw[1], b2 * iw[2]];
                let sum = f[0] + f[1] + f[2];
                out.z = ((b0 * z[0] + b1 * z[1] + b2 * z[2]) + offset) as f32;
                out.invw = sum as f32;
                out.front = *front;
                out.pc = [0.0; 2];
                let k = if sum != 0.0 { 1.0 / sum } else { 0.0 };
                for j in 0..nv {
                    let (a, b, c) = (self.vars[base[0] + j], self.vars[base[1] + j], self.vars[base[2] + j]);
                    for l in 0..4 {
                        out.vars[j][l] = ((f[0] * a[l] as f64 + f[1] * b[l] as f64 + f[2] * c[l] as f64) * k) as f32;
                    }
                }
            }
            Setup::Line { p, z, iw, base, xmajor } => {
                let (d, o) = if *xmajor { (p[1][0] - p[0][0], px - p[0][0]) } else { (p[1][1] - p[0][1], py - p[0][1]) };
                let t = if d != 0.0 { o / d } else { 0.0 };
                let f = [(1.0 - t) * iw[0], t * iw[1]];
                let sum = f[0] + f[1];
                out.z = ((1.0 - t) * z[0] + t * z[1]) as f32;
                out.invw = sum as f32;
                out.front = true;
                out.pc = [0.0; 2];
                let k = if sum != 0.0 { 1.0 / sum } else { 0.0 };
                for j in 0..nv {
                    let (a, b) = (self.vars[base[0] + j], self.vars[base[1] + j]);
                    for l in 0..4 {
                        out.vars[j][l] = ((f[0] * a[l] as f64 + f[1] * b[l] as f64) * k) as f32;
                    }
                }
            }
            Setup::Point { c, z, iw, base, size } => {
                out.z = *z as f32;
                out.invw = *iw as f32;
                out.front = true;
                // ES 2.0 §3.3: gl_PointCoord's origin is the upper left of the point.
                out.pc = [(0.5 + (px - c[0]) / size) as f32, (0.5 - (py - c[1]) / size) as f32];
                for j in 0..nv {
                    out.vars[j] = self.vars[base + j];
                }
            }
        }
    }

    fn load_fs(&mut self, k: usize) {
        let l = self.l;
        let fs = &l.fs;
        let inp = &self.fin[k];
        for (j, (_, r)) in l.varyings.iter().enumerate() {
            self.regs[*r as usize] = inp.vars[j];
        }
        if let Some(r) = fs.builtins.frag_coord {
            self.regs[r as usize] = [inp.x as f32 + 0.5, inp.y as f32 + 0.5, inp.z, inp.invw];
        }
        if let Some(r) = fs.builtins.front_facing {
            self.regs[r as usize] = [inp.front as u8 as f32; 4];
        }
        if let Some(r) = fs.builtins.point_coord {
            self.regs[r as usize] = [inp.pc[0], inp.pc[1], 0.0, 1.0];
        }
    }

    /// Shade and write the fragment at pixel (x, y).
    fn fragment(&mut self, s: &Setup, x: i32, y: i32) {
        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
        let mut f0 = core::mem::take(&mut self.fin[0]);
        self.interp(s, px, py, &mut f0);
        f0.x = x;
        f0.y = y;
        self.fin[0] = f0;
        let l = self.l;
        let code = &l.fs.ops;
        let outcome = if self.quad {
            // Helper invocations at the right and upper neighbours (the 2×2 quad's differences).
            for (k, (dx, dy)) in [(1usize, (1i32, 0i32)), (2, (0, 1))] {
                let mut f = core::mem::take(&mut self.fin[k]);
                self.interp(s, px + dx as f64, py + dy as f64, &mut f);
                f.x = x + dx;
                f.y = y + dy;
                self.fin[k] = f;
                self.load_fs(k);
                let mut log = core::mem::take(if k == 1 { &mut self.log_x } else { &mut self.log_y });
                log.clear();
                let (_, n) = vm::run(code, &mut self.regs, self.env, Mode::Record(&mut log), true);
                self.stats.fragment_ops += n;
                self.stats.helper_invocations += 1;
                if k == 1 {
                    self.log_x = log;
                } else {
                    self.log_y = log;
                }
            }
            self.load_fs(0);
            let (o, n) = vm::run(code, &mut self.regs, self.env, Mode::Quad { x: &self.log_x, y: &self.log_y }, true);
            self.stats.fragment_ops += n;
            o
        } else {
            self.load_fs(0);
            let (o, n) = vm::run(code, &mut self.regs, self.env, Mode::Plain, true);
            self.stats.fragment_ops += n;
            o
        };
        self.stats.fragments += 1;
        if outcome == Outcome::Discard {
            return;
        }
        let l = self.l;
        let b = &l.fs.builtins;
        let color = match b.frag_color.or(b.frag_data) {
            Some(r) => self.regs[r as usize],
            None => [0.0; 4],
        };
        let (z, front) = (self.fin[0].z, self.fin[0].front);
        self.ops(x, y, z, front, color);
    }

    /// ES 2.0 §4.1: stencil, depth, blending, mask, write.
    fn ops(&mut self, x: i32, y: i32, z: f32, front: bool, color: [f32; 4]) {
        let st = self.st;
        let t = &mut *self.t;
        let i = y as usize * t.w as usize + x as usize;
        let face: &StencilFace = if front { &st.front } else { &st.back };
        let stencil_on = st.stencil_test && t.has_stencil;
        let reference = face.reference.clamp(0, 255) as u8;
        let wm = (face.write_mask & 0xFF) as u8;
        let apply = |t: &mut Target, op: u32| {
            let old = t.stencil[i];
            let nv = stencil_op(op, old, reference);
            t.stencil[i] = (old & !wm) | (nv & wm);
        };
        if stencil_on {
            let m = face.value_mask & 0xFF;
            if !cmp(face.func, reference as u32 & m, t.stencil[i] as u32 & m) {
                apply(t, face.fail);
                return;
            }
        }
        if st.depth_test && t.depth_bits > 0 {
            let zq = depth_q(z, t.depth_bits);
            if !cmp(st.depth_func, zq, t.depth[i]) {
                if stencil_on {
                    apply(t, face.zfail);
                }
                return;
            }
            if st.depth_mask {
                t.depth[i] = zq;
            }
        }
        if stencil_on {
            apply(t, face.zpass);
        }
        if !t.has_color {
            return;
        }
        let px = &mut t.color[i * 4..i * 4 + 4];
        let mut c = color.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        if st.blend {
            let mut d = [px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0, px[3] as f32 / 255.0];
            if matches!(t.fmt, Fmt::Rgb8 | Fmt::Rgb565) {
                d[3] = 1.0;
            }
            c = blend(st, c, d);
        }
        let q = quantize(c, t.fmt);
        for k in 0..4 {
            if st.color_mask[k] {
                px[k] = q[k];
            }
        }
    }

    fn inside_scissor(&self, x: i32, y: i32) -> bool {
        let s = self.scissor;
        x >= s[0] && y >= s[1] && x < s[2] && y < s[3]
    }

    // ---------------------------------------------------------------------------------- triangles

    /// A triangle in window coordinates (vertex records: x, y, z, 1/w and the varyings base index).
    fn triangle(&mut self, v: [([f64; 4], usize); 3]) {
        let st = self.st;
        let p = [[v[0].0[0], v[0].0[1]], [v[1].0[0], v[1].0[1]], [v[2].0[0], v[2].0[1]]];
        let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
        if area == 0.0 || !area.is_finite() {
            return;
        }
        // ES 2.0 §3.5.1: CCW (positive area, y up) is front when FRONT_FACE is CCW.
        let front = (area > 0.0) == (st.front_face == CCW);
        if st.cull && (st.cull_mode == FRONT_AND_BACK || (st.cull_mode == FRONT) == front) {
            return;
        }
        // Orient counter-clockwise for the edge functions.
        let ord = if area > 0.0 { [0, 1, 2] } else { [0, 2, 1] };
        let pv = [v[ord[0]], v[ord[1]], v[ord[2]]];
        let pp = [[pv[0].0[0], pv[0].0[1]], [pv[1].0[0], pv[1].0[1]], [pv[2].0[0], pv[2].0[1]]];
        let z = [pv[0].0[2], pv[1].0[2], pv[2].0[2]];
        let a2 = area.abs();
        // §3.5.5 polygon offset: m·factor + r·units, m = max |∂z/∂x|, |∂z/∂y|.
        let mut offset = 0.0;
        if st.polygon_offset_fill {
            let dzdx = ((z[1] - z[0]) * (pp[2][1] - pp[0][1]) - (z[2] - z[0]) * (pp[1][1] - pp[0][1])) / a2;
            let dzdy = ((z[2] - z[0]) * (pp[1][0] - pp[0][0]) - (z[1] - z[0]) * (pp[2][0] - pp[0][0])) / a2;
            let m = dzdx.abs().max(dzdy.abs());
            let r = if self.t.depth_bits == 16 { 1.0 / 65536.0 } else { 1.0 / 16_777_216.0 };
            offset = m * st.po_factor as f64 + r * st.po_units as f64;
        }
        let setup = Setup::Tri {
            p: pp,
            z,
            iw: [pv[0].0[3], pv[1].0[3], pv[2].0[3]],
            area: a2,
            base: [pv[0].1, pv[1].1, pv[2].1],
            front,
            offset,
        };
        // Fixed point, 8 sub-pixel bits.
        let fx = |c: f64| (c * 256.0).round() as i64;
        let q: [[i64; 2]; 3] = [[fx(pp[0][0]), fx(pp[0][1])], [fx(pp[1][0]), fx(pp[1][1])], [fx(pp[2][0]), fx(pp[2][1])]];
        let x0 = (q.iter().map(|a| a[0]).min().unwrap() >> 8).max(self.scissor[0] as i64);
        let y0 = (q.iter().map(|a| a[1]).min().unwrap() >> 8).max(self.scissor[1] as i64);
        let x1 = ((q.iter().map(|a| a[0]).max().unwrap() >> 8) + 1).min(self.scissor[2] as i64);
        let y1 = ((q.iter().map(|a| a[1]).max().unwrap() >> 8) + 1).min(self.scissor[3] as i64);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        // Edge k runs from q[k] to q[k+1]; the pixel is inside when every edge function is ≥ 0, with ties kept
        // only on top and left edges (ES 2.0 §3.5.1's "exactly one of two adjacent primitives").
        let edges: [([i64; 2], [i64; 2]); 3] = [(q[1], q[2]), (q[2], q[0]), (q[0], q[1])];
        let bias: [i64; 3] = edges.map(|(a, b)| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            if dy < 0 || (dy == 0 && dx < 0) { 0 } else { 1 }
        });
        if q.iter().all(|a| a == &q[0]) {
            return;
        }
        for y in y0..y1 {
            let pyq = y * 256 + 128;
            for x in x0..x1 {
                let pxq = x * 256 + 128;
                let mut inside = true;
                for (k, (a, b)) in edges.iter().enumerate() {
                    let e = (b[0] - a[0]) * (pyq - a[1]) - (b[1] - a[1]) * (pxq - a[0]);
                    if e - bias[k] < 0 {
                        inside = false;
                        break;
                    }
                }
                if inside {
                    self.fragment(&setup, x as i32, y as i32);
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------- lines

    fn line(&mut self, a: ([f64; 4], usize), b: ([f64; 4], usize)) {
        let (x0, y0, x1, y1) = (a.0[0], a.0[1], b.0[0], b.0[1]);
        let xmajor = (x1 - x0).abs() >= (y1 - y0).abs();
        let setup = Setup::Line { p: [[x0, y0], [x1, y1]], z: [a.0[2], b.0[2]], iw: [a.0[3], b.0[3]], base: [a.1, b.1], xmajor };
        // ES 2.0 §3.4.1 (the diamond-exit rule approximated by major-axis stepping): one fragment per column
        // (x-major) between the start pixel and the end pixel, the end pixel excluded.
        let (s, e, s2, e2) = if xmajor { (x0, x1, y0, y1) } else { (y0, y1, x0, x1) };
        let (c0, c1) = (s.floor() as i64, e.floor() as i64);
        if c0 == c1 {
            return;
        }
        let step = if c1 > c0 { 1 } else { -1 };
        let mut c = c0;
        while c != c1 {
            let pc = c as f64 + 0.5;
            let t = ((pc - s) / (e - s)).clamp(0.0, 1.0);
            let other = (s2 + t * (e2 - s2)).floor() as i64;
            let (x, y) = if xmajor { (c, other) } else { (other, c) };
            if x >= 0 && y >= 0 && self.inside_scissor(x as i32, y as i32) {
                self.fragment(&setup, x as i32, y as i32);
            }
            c += step;
        }
    }

    // ---------------------------------------------------------------------------------- points

    fn point(&mut self, v: ([f64; 4], usize), size: f32) {
        let size = (size.clamp(1.0, POINT_SIZE_MAX)) as f64;
        let (cx, cy) = (v.0[0], v.0[1]);
        let setup = Setup::Point { c: [cx, cy], z: v.0[2], iw: v.0[3], base: v.1, size };
        let h = size / 2.0;
        let xs = (cx - h - 0.5).ceil() as i64;
        let xe = (cx + h - 0.5).ceil() as i64;
        let ys = (cy - h - 0.5).ceil() as i64;
        let ye = (cy + h - 0.5).ceil() as i64;
        for y in ys.max(self.scissor[1] as i64)..ye.min(self.scissor[3] as i64) {
            for x in xs.max(self.scissor[0] as i64)..xe.min(self.scissor[2] as i64) {
                self.fragment(&setup, x as i32, y as i32);
            }
        }
    }
}

/// A clip-space vertex with its varyings (for clipping).
#[derive(Clone)]
struct CV {
    p: [f64; 4],
    v: Vec<[f32; 4]>,
}

fn lerp_cv(a: &CV, b: &CV, t: f64) -> CV {
    let mut p = [0.0; 4];
    for i in 0..4 {
        p[i] = a.p[i] + (b.p[i] - a.p[i]) * t;
    }
    let v = a.v.iter().zip(b.v.iter()).map(|(x, y)| {
        let mut o = [0.0f32; 4];
        for i in 0..4 {
            o[i] = x[i] + (y[i] - x[i]) * t as f32;
        }
        o
    });
    CV { p, v: v.collect() }
}

/// The six clip planes as distances (≥ 0 inside), ES 2.0 §2.13.
fn plane(p: &[f64; 4], k: usize) -> f64 {
    match k {
        0 => p[3] + p[0],
        1 => p[3] - p[0],
        2 => p[3] + p[1],
        3 => p[3] - p[1],
        4 => p[3] + p[2],
        _ => p[3] - p[2],
    }
}

fn clip_poly(mut poly: Vec<CV>) -> Vec<CV> {
    for k in 0..6 {
        if poly.is_empty() {
            break;
        }
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let a = &poly[i];
            let b = &poly[(i + 1) % poly.len()];
            let (da, db) = (plane(&a.p, k), plane(&b.p, k));
            if da >= 0.0 {
                out.push(a.clone());
            }
            if (da >= 0.0) != (db >= 0.0) {
                out.push(lerp_cv(a, b, da / (da - db)));
            }
        }
        poly = out;
    }
    poly
}

impl Context {
    /// preserveDrawingBuffer:false — the presented drawing buffer reads as cleared (colour 0, depth 1,
    /// stencil 0) before anything touches it again (WebGL 1.0 "The Drawing Buffer").
    pub(crate) fn fresh_drawing_buffer(&mut self) {
        if self.framebuffer == 0 && self.db.clear_pending {
            self.db.clear_pending = false;
            self.db.color.iter_mut().for_each(|b| *b = 0);
            self.db.depth.iter_mut().for_each(|d| *d = 0x00FF_FFFF);
            self.db.stencil.iter_mut().for_each(|s| *s = 0);
            self.db.generation += 1;
        }
    }

    pub(crate) fn take_target(&mut self) -> Target {
        if self.framebuffer == 0 {
            let db = &mut self.db;
            return Target {
                w: db.w,
                h: db.h,
                color: core::mem::take(&mut db.color),
                fmt: if self.attrs.alpha { Fmt::Rgba8 } else { Fmt::Rgb8 },
                has_color: true,
                depth: core::mem::take(&mut db.depth),
                depth_bits: if self.attrs.depth { 24 } else { 0 },
                stencil: core::mem::take(&mut db.stencil),
                has_stencil: self.attrs.stencil,
                origin: Origin::Default,
            };
        }
        let f = self.framebuffers.get(self.framebuffer).unwrap();
        let pts = f.points();
        let mut t = Target {
            w: 0,
            h: 0,
            color: Vec::new(),
            fmt: Fmt::Rgba8,
            has_color: false,
            depth: Vec::new(),
            depth_bits: 0,
            stencil: Vec::new(),
            has_stencil: false,
            origin: Origin::Fbo(pts),
        };
        match pts[0] {
            Attach::Rb(r) => {
                let r = self.renderbuffers.get_mut(r).unwrap();
                t.color = core::mem::take(&mut r.color);
                t.fmt = rb_fmt(r.format);
                (t.w, t.h, t.has_color) = (r.w, r.h, true);
            }
            Attach::Tex(n, target, level) => {
                let tex = self.textures.get_mut(n).unwrap();
                let face = texture::face_index(target).unwrap_or(0);
                if let Some(l) = tex.faces.get_mut(face).and_then(|f| f.get_mut(level as usize)).and_then(|l| l.as_mut()) {
                    t.color = core::mem::take(&mut l.data);
                    t.fmt = fmt_of(l.format, l.ty);
                    (t.w, t.h, t.has_color) = (l.w, l.h, true);
                }
            }
            Attach::None => {}
        }
        if let Attach::Rb(r) = pts[1] {
            let r = self.renderbuffers.get_mut(r).unwrap();
            t.depth = core::mem::take(&mut r.depth);
            t.depth_bits = 16;
            (t.w, t.h) = (r.w, r.h);
        }
        if let Attach::Rb(r) = pts[2] {
            let r = self.renderbuffers.get_mut(r).unwrap();
            t.stencil = core::mem::take(&mut r.stencil);
            t.has_stencil = true;
            (t.w, t.h) = (r.w, r.h);
        }
        if let Attach::Rb(r) = pts[3] {
            let r = self.renderbuffers.get_mut(r).unwrap();
            t.depth = core::mem::take(&mut r.depth);
            t.stencil = core::mem::take(&mut r.stencil);
            t.depth_bits = 24;
            t.has_stencil = true;
            (t.w, t.h) = (r.w, r.h);
        }
        t
    }

    pub(crate) fn put_target(&mut self, t: Target) {
        match t.origin {
            Origin::Default => {
                self.db.color = t.color;
                self.db.depth = t.depth;
                self.db.stencil = t.stencil;
                self.db.generation += 1;
            }
            Origin::Fbo(pts) => {
                match pts[0] {
                    Attach::Rb(r) => self.renderbuffers.get_mut(r).unwrap().color = t.color,
                    Attach::Tex(n, target, level) => {
                        let tex = self.textures.get_mut(n).unwrap();
                        let face = texture::face_index(target).unwrap_or(0);
                        if let Some(l) = tex.faces.get_mut(face).and_then(|f| f.get_mut(level as usize)).and_then(|l| l.as_mut()) {
                            l.data = t.color;
                        }
                    }
                    Attach::None => {}
                }
                if let Attach::Rb(r) = pts[1] {
                    self.renderbuffers.get_mut(r).unwrap().depth = t.depth;
                } else if let Attach::Rb(r) = pts[3] {
                    let r = self.renderbuffers.get_mut(r).unwrap();
                    r.depth = t.depth;
                    r.stencil = t.stencil;
                    return;
                }
                if let Attach::Rb(r) = pts[2] {
                    self.renderbuffers.get_mut(r).unwrap().stencil = t.stencil;
                }
            }
        }
    }

    /// The scissor-and-target clip box [x0, y0, x1, y1).
    fn clip_box(&self, w: u32, h: u32) -> [i32; 4] {
        let mut b = [0, 0, w as i32, h as i32];
        if self.st.scissor_test {
            let s = self.st.scissor;
            b[0] = b[0].max(s[0]);
            b[1] = b[1].max(s[1]);
            b[2] = b[2].min(s[0].saturating_add(s[2]));
            b[3] = b[3].min(s[1].saturating_add(s[3]));
        }
        b
    }

    /// ES 2.0 §4.2.3 Clear: honours the scissor box and the colour/depth/stencil write masks.
    pub fn clear(&mut self, mask: u32) {
        if mask & !(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT | STENCIL_BUFFER_BIT) != 0 {
            return self.err(INVALID_VALUE);
        }
        if self.framebuffer_status() != FRAMEBUFFER_COMPLETE {
            return self.err(INVALID_FRAMEBUFFER_OPERATION);
        }
        self.fresh_drawing_buffer();
        let mut t = self.take_target();
        let b = self.clip_box(t.w, t.h);
        let st = &self.st;
        let q = quantize(st.clear_color, t.fmt);
        let dq = depth_q(st.clear_depth, t.depth_bits.max(16));
        let wm = (st.front.write_mask & 0xFF) as u8;
        let sv = (st.clear_stencil & 0xFF) as u8;
        for y in b[1].max(0)..b[3] {
            for x in b[0].max(0)..b[2] {
                let i = y as usize * t.w as usize + x as usize;
                if mask & COLOR_BUFFER_BIT != 0 && t.has_color {
                    for k in 0..4 {
                        if st.color_mask[k] {
                            t.color[i * 4 + k] = q[k];
                        }
                    }
                }
                if mask & DEPTH_BUFFER_BIT != 0 && t.depth_bits > 0 && st.depth_mask {
                    t.depth[i] = dq;
                }
                if mask & STENCIL_BUFFER_BIT != 0 && t.has_stencil {
                    t.stencil[i] = (t.stencil[i] & !wm) | (sv & wm);
                }
            }
        }
        self.put_target(t);
    }

    /// ES 2.0 §4.3.1 ReadPixels (WebGL 1.0: RGBA/UNSIGNED_BYTE always; pixels outside the framebuffer leave
    /// the destination untouched). Rows are returned bottom-up, each padded to PACK_ALIGNMENT.
    #[allow(clippy::too_many_arguments)]
    pub fn read_pixels(&mut self, x: i32, y: i32, w: i32, h: i32, format: u32, ty: u32, out: &mut [u8]) {
        if !matches!(format, ALPHA | RGB | RGBA) || !matches!(ty, UNSIGNED_BYTE | UNSIGNED_SHORT_5_6_5 | UNSIGNED_SHORT_4_4_4_4 | UNSIGNED_SHORT_5_5_5_1) {
            return self.err(INVALID_ENUM);
        }
        if format != RGBA || ty != UNSIGNED_BYTE {
            return self.err(INVALID_OPERATION);
        }
        if w < 0 || h < 0 {
            return self.err(INVALID_VALUE);
        }
        if self.framebuffer_status() != FRAMEBUFFER_COMPLETE {
            return self.err(INVALID_FRAMEBUFFER_OPERATION);
        }
        let align = self.st.pack_alignment as usize;
        let row = (w as usize * 4).div_ceil(align) * align;
        let need = if h == 0 || w == 0 { 0 } else { row * (h as usize - 1) + w as usize * 4 };
        if out.len() < need {
            return self.err(INVALID_OPERATION);
        }
        self.fresh_drawing_buffer();
        let t = self.take_target();
        if !t.has_color {
            self.put_target(t);
            return self.err(INVALID_OPERATION);
        }
        for r in 0..h {
            let sy = y as i64 + r as i64;
            if sy < 0 || sy >= t.h as i64 {
                continue;
            }
            for c in 0..w {
                let sx = x as i64 + c as i64;
                if sx < 0 || sx >= t.w as i64 {
                    continue;
                }
                let si = (sy as usize * t.w as usize + sx as usize) * 4;
                let di = r as usize * row + c as usize * 4;
                out[di..di + 4].copy_from_slice(&t.color[si..si + 4]);
                if matches!(t.fmt, Fmt::Rgb8 | Fmt::Rgb565) {
                    out[di + 3] = 255;
                }
            }
        }
        // put_target bumps the default buffer's generation; a read is not a change.
        let g = self.db.generation;
        self.put_target(t);
        self.db.generation = g;
    }

    /// The drawing buffer for the compositor: top-down RGBA8, PREMULTIPLIED (the canvas bitmap's form) —
    /// `premultipliedAlpha: true` values are taken as premultiplied (colour clamped to alpha), `false` ones
    /// premultiplied here, `alpha: false` opaque (WebGL 1.0 "Premultiplied Alpha, Canvas APIs and texImage2D").
    /// `presented` marks it composited: with preserveDrawingBuffer:false it reads as cleared afterwards.
    pub fn present_premultiplied(&mut self, presented: bool) -> (u32, u32, Vec<u8>) {
        let (w, h) = (self.db.w as usize, self.db.h as usize);
        let mut out = vec![0u8; w * h * 4];
        if !self.db.clear_pending {
            for y in 0..h {
                let src = &self.db.color[(h - 1 - y) * w * 4..(h - y) * w * 4];
                let dst = &mut out[y * w * 4..(y + 1) * w * 4];
                for x in 0..w {
                    let p = &src[x * 4..x * 4 + 4];
                    let o = &mut dst[x * 4..x * 4 + 4];
                    if !self.attrs.alpha {
                        o.copy_from_slice(&[p[0], p[1], p[2], 255]);
                    } else if self.attrs.premultiplied_alpha {
                        o.copy_from_slice(&[p[0].min(p[3]), p[1].min(p[3]), p[2].min(p[3]), p[3]]);
                    } else {
                        let a = p[3] as u32;
                        o.copy_from_slice(&[((p[0] as u32 * a + 127) / 255) as u8, ((p[1] as u32 * a + 127) / 255) as u8, ((p[2] as u32 * a + 127) / 255) as u8, p[3]]);
                    }
                }
            }
        }
        if presented && !self.attrs.preserve_drawing_buffer {
            self.db.clear_pending = true;
        }
        (w as u32, h as u32, out)
    }

    // ------------------------------------------------------------------------------------------ draws

    fn valid_mode(mode: u32) -> bool {
        mode <= TRIANGLE_FAN
    }

    /// The state checks every draw shares (after its own argument checks): program, framebuffer, samplers,
    /// stencil faces (WebGL 1.0 "Stencil Separate Mask and Reference Value"), feedback loops.
    fn draw_state_ok(&mut self) -> bool {
        let cur = self.current_program;
        if cur == 0 || !self.programs.get(cur).is_some_and(|p| p.linked) {
            self.err(INVALID_OPERATION);
            return false;
        }
        if self.framebuffer_status() != FRAMEBUFFER_COMPLETE {
            self.err(INVALID_FRAMEBUFFER_OPERATION);
            return false;
        }
        let Ok(units) = self.sampler_units(cur) else {
            self.err(INVALID_OPERATION);
            return false;
        };
        let (f, b) = (&self.st.front, &self.st.back);
        if (f.write_mask & 0xFF) != (b.write_mask & 0xFF) || (f.value_mask & 0xFF) != (b.value_mask & 0xFF) || f.reference.clamp(0, 255) != b.reference.clamp(0, 255) {
            self.err(INVALID_OPERATION);
            return false;
        }
        // A texture image that is both sampled and attached to the draw framebuffer is a feedback loop.
        if let Some(fb) = self.framebuffers.get(self.framebuffer) {
            if let Attach::Tex(n, _, _) = fb.color {
                if units.iter().any(|(u, cube)| {
                    let (a, c) = self.units[*u];
                    (if *cube { c } else { a }) == n
                }) {
                    self.err(INVALID_OPERATION);
                    return false;
                }
            }
        }
        true
    }

    /// WebGL's range check: every enabled array an active attribute reads must hold `max_index`.
    fn attribs_in_range(&mut self, l: &Linked, max_index: Option<u32>) -> bool {
        for a in &l.attribs {
            for c in 0..a.ty.cols as u32 {
                let at = self.attribs[(a.location + c) as usize];
                if !at.enabled {
                    continue;
                }
                let Some(buf) = self.buffers.get(at.buffer) else {
                    self.err(INVALID_OPERATION);
                    return false;
                };
                if let Some(mx) = max_index {
                    let tsize = match at.ty {
                        BYTE | UNSIGNED_BYTE => 1u64,
                        SHORT | UNSIGNED_SHORT => 2,
                        _ => 4,
                    };
                    let stride = if at.stride == 0 { tsize * at.size as u64 } else { at.stride as u64 };
                    let end = at.offset as u64 + mx as u64 * stride + tsize * at.size as u64;
                    if end > buf.data.len() as u64 {
                        self.err(INVALID_OPERATION);
                        return false;
                    }
                }
            }
        }
        true
    }

    /// ES 2.0 §2.8 DrawArrays.
    pub fn draw_arrays(&mut self, mode: u32, first: i32, count: i32) {
        if !Self::valid_mode(mode) {
            return self.err(INVALID_ENUM);
        }
        if first < 0 || count < 0 {
            return self.err(INVALID_VALUE);
        }
        if !self.draw_state_ok() {
            return;
        }
        let l = self.programs.get(self.current_program).unwrap().linked_data.clone().unwrap();
        let max = if count == 0 { None } else { Some((first as u64 + count as u64 - 1).min(u32::MAX as u64) as u32) };
        if !self.attribs_in_range(&l, max) || count == 0 {
            return;
        }
        let idx: Vec<u32> = (first as u32..first as u32 + count as u32).collect();
        self.draw(mode, &idx, &l);
    }

    /// ES 2.0 §2.8 DrawElements (WebGL: offset a multiple of the type size, indices in range;
    /// UNSIGNED_INT with OES_element_index_uint).
    pub fn draw_elements(&mut self, mode: u32, count: i32, ty: u32, offset: i64) {
        if !Self::valid_mode(mode) {
            return self.err(INVALID_ENUM);
        }
        let size = match ty {
            UNSIGNED_BYTE => 1,
            UNSIGNED_SHORT => 2,
            UNSIGNED_INT if self.ext.element_index_uint => 4,
            _ => return self.err(INVALID_ENUM),
        };
        if count < 0 || offset < 0 {
            return self.err(INVALID_VALUE);
        }
        if offset % size as i64 != 0 {
            return self.err(INVALID_OPERATION);
        }
        let Some(eb) = self.buffers.get(self.element_buffer) else { return self.err(INVALID_OPERATION) };
        let end = offset as u64 + count as u64 * size as u64;
        if end > eb.data.len() as u64 {
            return self.err(INVALID_OPERATION);
        }
        let data = &eb.data[offset as usize..end as usize];
        let idx: Vec<u32> = match size {
            1 => data.to_vec().into_iter().map(|b| b as u32).collect(),
            2 => data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).collect(),
            _ => data.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
        };
        if !self.draw_state_ok() {
            return;
        }
        let l = self.programs.get(self.current_program).unwrap().linked_data.clone().unwrap();
        let max = idx.iter().copied().max();
        if !self.attribs_in_range(&l, max) || count == 0 {
            return;
        }
        self.draw(mode, &idx, &l);
    }

    /// Fetch one attribute element (ES 2.0 §2.8; normalized signed values by (2c + 1) / (2^b − 1), Table 2.7).
    fn fetch(&self, loc: usize, index: u32) -> [f32; 4] {
        let a = &self.attribs[loc];
        if !a.enabled {
            return a.current;
        }
        let Some(buf) = self.buffers.get(a.buffer) else { return [0.0, 0.0, 0.0, 1.0] };
        let tsize = match a.ty {
            BYTE | UNSIGNED_BYTE => 1usize,
            SHORT | UNSIGNED_SHORT => 2,
            _ => 4,
        };
        let stride = if a.stride == 0 { tsize * a.size as usize } else { a.stride as usize };
        let base = a.offset as usize + index as usize * stride;
        let mut out = [0.0, 0.0, 0.0, 1.0];
        let d = &buf.data;
        for c in 0..a.size as usize {
            let o = base + c * tsize;
            if o + tsize > d.len() {
                break;
            }
            out[c] = match a.ty {
                BYTE => {
                    let v = d[o] as i8 as f32;
                    if a.normalized { (2.0 * v + 1.0) / 255.0 } else { v }
                }
                UNSIGNED_BYTE => {
                    let v = d[o] as f32;
                    if a.normalized { v / 255.0 } else { v }
                }
                SHORT => {
                    let v = i16::from_le_bytes([d[o], d[o + 1]]) as f32;
                    if a.normalized { (2.0 * v + 1.0) / 65535.0 } else { v }
                }
                UNSIGNED_SHORT => {
                    let v = u16::from_le_bytes([d[o], d[o + 1]]) as f32;
                    if a.normalized { v / 65535.0 } else { v }
                }
                _ => f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]),
            };
        }
        out
    }

    fn draw(&mut self, mode: u32, idx: &[u32], l: &Linked) {
        self.fresh_drawing_buffer();
        let store = self.programs.get(self.current_program).unwrap().store.clone();
        let mut target = self.take_target();
        let stats = {
            let ctx: &Context = self;
            let st = &ctx.st;
            let env = TexEnv::new(ctx);
            // Vertex stage: each referenced index shaded once.
            let mut vregs = preload(&l.vs, &l.vs_map, &store, st);
            let nv = l.varyings.len();
            let mut slot_of: BTreeMap<u32, usize> = BTreeMap::new();
            let mut outs: Vec<VOut> = Vec::new();
            let mut vary: Vec<[f32; 4]> = Vec::new();
            let mut stats = DrawStats::default();
            let mut order = Vec::with_capacity(idx.len());
            for &i in idx {
                let s = match slot_of.get(&i) {
                    Some(s) => *s,
                    None => {
                        for a in &l.attribs {
                            for c in 0..a.ty.cols as u32 {
                                vregs[a.reg as usize + c as usize] = ctx.fetch((a.location + c) as usize, i);
                            }
                        }
                        let (_, n) = vm::run(&l.vs.ops, &mut vregs, &env, Mode::Plain, false);
                        stats.vertices += 1;
                        stats.vertex_ops += n;
                        let b = &l.vs.builtins;
                        let clip = b.position.map(|r| vregs[r as usize]).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                        let psize = b.point_size.map(|r| vregs[r as usize][0]).unwrap_or(1.0);
                        for (vr, _) in &l.varyings {
                            vary.push(vr.map(|r| vregs[r as usize]).unwrap_or([0.0; 4]));
                        }
                        outs.push(VOut { clip, psize });
                        let s = outs.len() - 1;
                        slot_of.insert(i, s);
                        s
                    }
                };
                order.push(s);
            }
            // Primitive assembly (§2.6.1).
            let n = order.len();
            let mut tris: Vec<[usize; 3]> = Vec::new();
            let mut lines: Vec<[usize; 2]> = Vec::new();
            let mut points: Vec<usize> = Vec::new();
            match mode {
                POINTS => points.extend(order.iter().copied()),
                LINES => {
                    for k in 0..n / 2 {
                        lines.push([order[2 * k], order[2 * k + 1]]);
                    }
                }
                LINE_STRIP | LINE_LOOP => {
                    for k in 1..n {
                        lines.push([order[k - 1], order[k]]);
                    }
                    if mode == LINE_LOOP && n > 2 {
                        lines.push([order[n - 1], order[0]]);
                    }
                }
                TRIANGLES => {
                    for k in 0..n / 3 {
                        tris.push([order[3 * k], order[3 * k + 1], order[3 * k + 2]]);
                    }
                }
                TRIANGLE_STRIP => {
                    for k in 0..n.saturating_sub(2) {
                        tris.push(if k % 2 == 0 { [order[k], order[k + 1], order[k + 2]] } else { [order[k + 1], order[k], order[k + 2]] });
                    }
                }
                _ => {
                    for k in 1..n.saturating_sub(1) {
                        tris.push([order[0], order[k], order[k + 1]]);
                    }
                }
            }
            let vp = st.viewport;
            let (n_, f_) = (st.depth_range[0] as f64, st.depth_range[1] as f64);
            let to_window = |p: &[f64; 4]| -> [f64; 4] {
                let iw = 1.0 / p[3];
                let (x, y, z) = (p[0] * iw, p[1] * iw, p[2] * iw);
                [
                    (x + 1.0) * vp[2] as f64 / 2.0 + vp[0] as f64,
                    (y + 1.0) * vp[3] as f64 / 2.0 + vp[1] as f64,
                    z * (f_ - n_) / 2.0 + (n_ + f_) / 2.0,
                    iw,
                ]
            };
            let fs_regs = preload(&l.fs, &l.fs_map, &store, st);
            let quad = l.fs.derivatives || (l.fs.implicit_lod && env.units.iter().any(|(a, b)| (*a).or(*b).is_some_and(|t| texture::uses_mipmaps(t.min_filter))));
            let scissor = ctx.clip_box(target.w, target.h);
            let mut pipe = Pipe {
                st,
                l,
                env: &env,
                regs: fs_regs,
                t: &mut target,
                stats,
                quad,
                nv,
                vars: vary,
                scissor,
                log_x: Vec::new(),
                log_y: Vec::new(),
                fin: [FragIn::default(), FragIn::default(), FragIn::default()],
            };
            let cv = |pipe: &Pipe, s: usize| -> CV {
                let c = outs[s].clip;
                CV { p: [c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64], v: pipe.vars[s * nv..s * nv + nv].to_vec() }
            };
            let inside = |p: &[f64; 4]| (0..6).all(|k| plane(p, k) >= 0.0);
            for t in tris {
                let vs3 = [cv(&pipe, t[0]), cv(&pipe, t[1]), cv(&pipe, t[2])];
                let poly = if vs3.iter().all(|v| inside(&v.p)) { vs3.to_vec() } else { clip_poly(vs3.to_vec()) };
                if poly.len() < 3 {
                    continue;
                }
                // Append the clipped vertices' varyings so the setup can index them.
                let mut recs = Vec::with_capacity(poly.len());
                for v in &poly {
                    if v.p[3] <= 0.0 {
                        recs.clear();
                        break;
                    }
                    let base = pipe.vars.len();
                    pipe.vars.extend_from_slice(&v.v);
                    recs.push((to_window(&v.p), base));
                }
                for k in 1..recs.len().saturating_sub(1) {
                    pipe.triangle([recs[0], recs[k], recs[k + 1]]);
                }
            }
            for ln in lines {
                let (mut a, mut b) = (cv(&pipe, ln[0]), cv(&pipe, ln[1]));
                // Liang–Barsky against the six planes.
                let (mut t0, mut t1) = (0.0f64, 1.0f64);
                let mut keep = true;
                for k in 0..6 {
                    let (da, db) = (plane(&a.p, k), plane(&b.p, k));
                    if da < 0.0 && db < 0.0 {
                        keep = false;
                        break;
                    }
                    if da < 0.0 {
                        t0 = t0.max(da / (da - db));
                    } else if db < 0.0 {
                        t1 = t1.min(da / (da - db));
                    }
                }
                if !keep || t0 > t1 {
                    continue;
                }
                let (na, nb) = (lerp_cv(&a, &b, t0), lerp_cv(&a, &b, t1));
                a = na;
                b = nb;
                if a.p[3] <= 0.0 || b.p[3] <= 0.0 {
                    continue;
                }
                let ba = pipe.vars.len();
                pipe.vars.extend_from_slice(&a.v);
                let bb = pipe.vars.len();
                pipe.vars.extend_from_slice(&b.v);
                pipe.line((to_window(&a.p), ba), (to_window(&b.p), bb));
            }
            for p in points {
                let c = outs[p].clip;
                let pc = [c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64];
                if !inside(&pc) || pc[3] <= 0.0 {
                    continue;
                }
                let size = outs[p].psize;
                pipe.point((to_window(&pc), p * nv), size);
            }
            pipe.stats
        };
        self.put_target(target);
        self.stats = stats;
    }
}
