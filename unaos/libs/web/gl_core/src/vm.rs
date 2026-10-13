//! The shader bytecode and its interpreter: a flat list of [`Op`]s over a flat register file of `[f32; 4]`
//! registers (one file per shader stage per draw). Not a tree walker: the code generator lowers the typed
//! GLSL tree once per compile and every invocation runs the list.
//!
//! An op reads its sources through a swizzle ([`Src`]: register + 2 bits per lane), computes four lanes and
//! writes the lanes its [`Dst`] mask selects. A scalar operand is a broadcast swizzle; a write to `v.zy` is a
//! Mov whose swizzle routes the value's lanes 0,1 to lanes 2,1. Ints and bools live in f32 lanes (ints are
//! exact to 2^24, bools are 0 / 1). Builtins are computed in f64 ([`crate::fmath`]) and rounded once.
//!
//! Functions have static registers (GLSL ES §6.1 forbids recursion, so no frame is ever live twice): Call
//! pushes a return address, Ret pops it, and a Ret with an empty stack ends the invocation.

use crate::fmath as m;
use crate::glsl::ast::TexKind;
use alloc::vec::Vec;

/// The identity swizzle `xyzw`.
pub const IDENT: u8 = 0b11_10_01_00;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Src {
    pub r: u16,
    pub sw: u8,
}

impl Src {
    pub fn reg(r: u16) -> Src {
        Src { r, sw: IDENT }
    }
    pub fn lane(&self, i: usize) -> u8 {
        (self.sw >> (2 * i)) & 3
    }
    /// The same register with lane `i` of this swizzle broadcast to all four lanes.
    pub fn bcast(&self, i: usize) -> Src {
        let l = self.lane(i);
        Src { r: self.r, sw: l | (l << 2) | (l << 4) | (l << 6) }
    }
}

pub fn swizzle(sel: [u8; 4]) -> u8 {
    sel[0] | (sel[1] << 2) | (sel[2] << 4) | (sel[3] << 6)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dst {
    pub r: u16,
    pub m: u8,
}

impl Dst {
    pub fn all(r: u16) -> Dst {
        Dst { r, m: 0xF }
    }
    pub fn lane(r: u16, i: u8) -> Dst {
        Dst { r, m: 1 << i }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Un {
    Neg,
    Not,
    ToBool,
    Abs,
    Sign,
    Floor,
    Ceil,
    Fract,
    Trunc,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Exp,
    Log,
    Exp2,
    Log2,
    Sqrt,
    InvSqrt,
    Radians,
    Degrees,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bin {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    And,
    Or,
    Min,
    Max,
    Mod,
    Step,
    Pow,
    Atan2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tri {
    /// a·b + c
    Mad,
    Clamp,
    Mix,
    Smoothstep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deriv {
    Dx,
    Dy,
    Fwidth,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Mov(Dst, Src),
    Un(Un, Dst, Src),
    Bin(Bin, Dst, Src, Src),
    Tri(Tri, Dst, Src, Src, Src),
    /// dot of the first n lanes, broadcast to the masked lanes.
    Dot(u8, Dst, Src, Src),
    Len(u8, Dst, Src),
    Dist(u8, Dst, Src, Src),
    Norm(u8, Dst, Src),
    Cross(Dst, Src, Src),
    FaceFwd(u8, Dst, Src, Src, Src),
    Reflect(u8, Dst, Src, Src),
    Refract(u8, Dst, Src, Src, Src),
    Any(u8, Dst, Src),
    All(u8, Dst, Src),
    /// 1 when the first n lanes of a and b are all equal (aggregate ==).
    EqAll(u8, Dst, Src, Src),
    /// Copy n registers from `base + clamp(idx) * stride` to `d..d+n` (dynamic array / matrix indexing).
    LdIdx { d: u16, base: u16, idx: Src, stride: u16, len: u16, n: u16 },
    /// Copy n registers from `s..s+n` to `base + clamp(idx) * stride`.
    StIdx { base: u16, idx: Src, stride: u16, len: u16, s: u16, n: u16 },
    /// d = v[idx] (dynamic vector component), broadcast.
    ExtDyn(Dst, Src, Src),
    /// d[idx] = val.x
    InsDyn { d: u16, idx: Src, val: Src },
    Jmp(u32),
    /// Jump when lane x of the source is 0.
    Jz(Src, u32),
    Jnz(Src, u32),
    Call(u32),
    Ret,
    Discard,
    /// A texture lookup; `extra` = the bias (implicit LOD) or the explicit lod (`lod`).
    Tex { kind: TexKind, d: Dst, coord: Src, sampler: Src, extra: Option<Src>, lod: bool },
    Deriv(Deriv, Dst, Src),
}

/// How a lookup's level of detail is chosen (ES 2.0 §3.7.7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lod {
    /// No derivatives available: λ = bias (vertex shaders, and fragments without helper invocations).
    Bias(f32),
    Explicit(f32),
    /// The coordinate's differences to the right and upper neighbours, plus the bias.
    Grad { dx: [f32; 3], dy: [f32; 3], bias: f32 },
}

/// The texture units a run can sample (the draw call's snapshot).
pub trait Sampler {
    fn sample(&self, unit: usize, cube: bool, coord: [f32; 3], lod: Lod) -> [f32; 4];
}

/// A sampler with nothing bound (every lookup returns (0, 0, 0, 1)).
pub struct NoTextures;

impl Sampler for NoTextures {
    fn sample(&self, _: usize, _: bool, _: [f32; 3], _: Lod) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// Quad mode for fragment derivatives: Record logs every derivative-bearing operand in execution order;
/// Quad reads the right (`x`) and upper (`y`) neighbours' logs for the k-th such operand.
pub enum Mode<'a> {
    Plain,
    Record(&'a mut Vec<[f32; 4]>),
    Quad { x: &'a [[f32; 4]], y: &'a [[f32; 4]] },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Discard,
    /// The op budget ran out (an unbounded loop): the invocation's outputs are whatever they hold.
    Budget,
}

/// Per-invocation op budget.
pub const BUDGET: u64 = 1 << 20;

#[inline]
fn rd(regs: &[[f32; 4]], s: Src) -> [f32; 4] {
    let v = regs[s.r as usize];
    [v[(s.sw & 3) as usize], v[((s.sw >> 2) & 3) as usize], v[((s.sw >> 4) & 3) as usize], v[((s.sw >> 6) & 3) as usize]]
}

#[inline]
fn wr(regs: &mut [[f32; 4]], d: Dst, v: [f32; 4]) {
    let r = &mut regs[d.r as usize];
    for (i, x) in v.iter().enumerate() {
        if d.m & (1 << i) != 0 {
            r[i] = *x;
        }
    }
}

#[inline]
fn map1(a: [f32; 4], f: impl Fn(f64) -> f64) -> [f32; 4] {
    [f(a[0] as f64) as f32, f(a[1] as f64) as f32, f(a[2] as f64) as f32, f(a[3] as f64) as f32]
}

#[inline]
fn map2(a: [f32; 4], b: [f32; 4], f: impl Fn(f32, f32) -> f32) -> [f32; 4] {
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), f(a[3], b[3])]
}

fn dot(n: u8, a: [f32; 4], b: [f32; 4]) -> f32 {
    let mut s = 0.0f64;
    for i in 0..n as usize {
        s += a[i] as f64 * b[i] as f64;
    }
    s as f32
}

fn b(x: bool) -> f32 {
    x as u8 as f32
}

fn un(f: Un, a: [f32; 4]) -> [f32; 4] {
    match f {
        Un::Neg => a.map(|x| -x),
        Un::Not => a.map(|x| b(x == 0.0)),
        Un::ToBool => a.map(|x| b(x != 0.0)),
        Un::Abs => a.map(|x| x.abs()),
        Un::Sign => a.map(|x| if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 }),
        Un::Floor => map1(a, m::floor),
        Un::Ceil => map1(a, m::ceil),
        Un::Fract => map1(a, m::fract),
        Un::Trunc => map1(a, m::trunc),
        Un::Sin => map1(a, m::sin),
        Un::Cos => map1(a, m::cos),
        Un::Tan => map1(a, m::tan),
        Un::Asin => map1(a, m::asin),
        Un::Acos => map1(a, m::acos),
        Un::Atan => map1(a, m::atan),
        Un::Exp => map1(a, m::exp),
        Un::Log => map1(a, m::ln),
        Un::Exp2 => map1(a, m::exp2),
        Un::Log2 => map1(a, m::log2),
        Un::Sqrt => map1(a, m::sqrt),
        Un::InvSqrt => map1(a, |x| 1.0 / m::sqrt(x)),
        Un::Radians => map1(a, |x| x * (core::f64::consts::PI / 180.0)),
        Un::Degrees => map1(a, |x| x * (180.0 / core::f64::consts::PI)),
    }
}

fn bin(f: Bin, x: [f32; 4], y: [f32; 4]) -> [f32; 4] {
    match f {
        Bin::Add => map2(x, y, |a, c| a + c),
        Bin::Sub => map2(x, y, |a, c| a - c),
        Bin::Mul => map2(x, y, |a, c| a * c),
        Bin::Div => map2(x, y, |a, c| a / c),
        Bin::Lt => map2(x, y, |a, c| b(a < c)),
        Bin::Gt => map2(x, y, |a, c| b(a > c)),
        Bin::Le => map2(x, y, |a, c| b(a <= c)),
        Bin::Ge => map2(x, y, |a, c| b(a >= c)),
        Bin::Eq => map2(x, y, |a, c| b(a == c)),
        Bin::Ne => map2(x, y, |a, c| b(a != c)),
        Bin::And => map2(x, y, |a, c| b(a != 0.0 && c != 0.0)),
        Bin::Or => map2(x, y, |a, c| b(a != 0.0 || c != 0.0)),
        Bin::Min => map2(x, y, |a, c| if c < a { c } else { a }),
        Bin::Max => map2(x, y, |a, c| if c > a { c } else { a }),
        // GLSL ES §8.3: mod(x, y) = x − y·floor(x/y).
        Bin::Mod => map2(x, y, |a, c| (a as f64 - c as f64 * m::floor(a as f64 / c as f64)) as f32),
        // step(edge, x): 0 when x < edge.
        Bin::Step => map2(x, y, |e, v| b(v >= e)),
        Bin::Pow => map2(x, y, |a, c| m::pow(a as f64, c as f64) as f32),
        Bin::Atan2 => map2(x, y, |a, c| m::atan2(a as f64, c as f64) as f32),
    }
}

fn tri(f: Tri, x: [f32; 4], y: [f32; 4], z: [f32; 4]) -> [f32; 4] {
    let mut o = [0.0; 4];
    for i in 0..4 {
        let (a, c, e) = (x[i], y[i], z[i]);
        o[i] = match f {
            Tri::Mad => a * c + e,
            Tri::Clamp => {
                let v = if a < c { c } else { a };
                if v > e { e } else { v }
            }
            Tri::Mix => a * (1.0 - e) + c * e,
            Tri::Smoothstep => {
                let t = ((e - a) / (c - a)).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            }
        };
    }
    o
}

/// Run `code` on `regs` from pc 0. Returns the outcome and the number of ops executed.
pub fn run(code: &[Op], regs: &mut [[f32; 4]], tex: &dyn Sampler, mut mode: Mode, fragment: bool) -> (Outcome, u64) {
    let mut pc = 0usize;
    let mut stack: Vec<usize> = Vec::new();
    let mut ops = 0u64;
    let mut k = 0usize; // derivative-bearing op counter (quad mode)
    while pc < code.len() {
        ops += 1;
        if ops > BUDGET {
            return (Outcome::Budget, ops);
        }
        let op = code[pc];
        pc += 1;
        match op {
            Op::Mov(d, s) => {
                let v = rd(regs, s);
                wr(regs, d, v);
            }
            Op::Un(f, d, s) => {
                let v = un(f, rd(regs, s));
                wr(regs, d, v);
            }
            Op::Bin(f, d, a, c) => {
                let v = bin(f, rd(regs, a), rd(regs, c));
                wr(regs, d, v);
            }
            Op::Tri(f, d, a, c, e) => {
                let v = tri(f, rd(regs, a), rd(regs, c), rd(regs, e));
                wr(regs, d, v);
            }
            Op::Dot(n, d, a, c) => {
                let v = dot(n, rd(regs, a), rd(regs, c));
                wr(regs, d, [v; 4]);
            }
            Op::Len(n, d, a) => {
                let x = rd(regs, a);
                let v = m::sqrt(dot(n, x, x) as f64) as f32;
                wr(regs, d, [v; 4]);
            }
            Op::Dist(n, d, a, c) => {
                let x = map2(rd(regs, a), rd(regs, c), |p, q| p - q);
                let v = m::sqrt(dot(n, x, x) as f64) as f32;
                wr(regs, d, [v; 4]);
            }
            Op::Norm(n, d, a) => {
                let x = rd(regs, a);
                let l = m::sqrt(dot(n, x, x) as f64) as f32;
                wr(regs, d, x.map(|v| v / l));
            }
            Op::Cross(d, a, c) => {
                let (x, y) = (rd(regs, a), rd(regs, c));
                wr(regs, d, [x[1] * y[2] - y[1] * x[2], x[2] * y[0] - y[2] * x[0], x[0] * y[1] - y[0] * x[1], 0.0]);
            }
            Op::FaceFwd(n, d, nn, i, nref) => {
                let (nv, iv, rv) = (rd(regs, nn), rd(regs, i), rd(regs, nref));
                let v = if dot(n, rv, iv) < 0.0 { nv } else { nv.map(|x| -x) };
                wr(regs, d, v);
            }
            Op::Reflect(n, d, i, nn) => {
                let (iv, nv) = (rd(regs, i), rd(regs, nn));
                let k2 = 2.0 * dot(n, nv, iv);
                wr(regs, d, map2(iv, nv, |p, q| p - k2 * q));
            }
            Op::Refract(n, d, i, nn, eta) => {
                let (iv, nv, e) = (rd(regs, i), rd(regs, nn), rd(regs, eta)[0]);
                let ni = dot(n, nv, iv);
                let kk = 1.0 - e * e * (1.0 - ni * ni);
                let v = if kk < 0.0 {
                    [0.0; 4]
                } else {
                    let s = e * ni + m::sqrt(kk as f64) as f32;
                    map2(iv, nv, |p, q| e * p - s * q)
                };
                wr(regs, d, v);
            }
            Op::Any(n, d, a) => {
                let x = rd(regs, a);
                wr(regs, d, [b(x[..n as usize].iter().any(|v| *v != 0.0)); 4]);
            }
            Op::All(n, d, a) => {
                let x = rd(regs, a);
                wr(regs, d, [b(x[..n as usize].iter().all(|v| *v != 0.0)); 4]);
            }
            Op::EqAll(n, d, a, c) => {
                let (x, y) = (rd(regs, a), rd(regs, c));
                wr(regs, d, [b((0..n as usize).all(|i| x[i] == y[i])); 4]);
            }
            Op::LdIdx { d, base, idx, stride, len, n } => {
                let i = (rd(regs, idx)[0] as i64).clamp(0, len.max(1) as i64 - 1) as usize;
                let from = base as usize + i * stride as usize;
                for j in 0..n as usize {
                    regs[d as usize + j] = regs[from + j];
                }
            }
            Op::StIdx { base, idx, stride, len, s, n } => {
                let i = (rd(regs, idx)[0] as i64).clamp(0, len.max(1) as i64 - 1) as usize;
                let to = base as usize + i * stride as usize;
                for j in 0..n as usize {
                    regs[to + j] = regs[s as usize + j];
                }
            }
            Op::ExtDyn(d, v, idx) => {
                let i = (rd(regs, idx)[0] as i64).clamp(0, 3) as usize;
                let x = rd(regs, v)[i];
                wr(regs, d, [x; 4]);
            }
            Op::InsDyn { d, idx, val } => {
                let i = (rd(regs, idx)[0] as i64).clamp(0, 3) as usize;
                regs[d as usize][i] = rd(regs, val)[0];
            }
            Op::Jmp(t) => pc = t as usize,
            Op::Jz(c, t) => {
                if rd(regs, c)[0] == 0.0 {
                    pc = t as usize;
                }
            }
            Op::Jnz(c, t) => {
                if rd(regs, c)[0] != 0.0 {
                    pc = t as usize;
                }
            }
            Op::Call(t) => {
                stack.push(pc);
                pc = t as usize;
            }
            Op::Ret => match stack.pop() {
                Some(r) => pc = r,
                None => return (Outcome::Done, ops),
            },
            Op::Discard => return (Outcome::Discard, ops),
            Op::Tex { kind, d, coord, sampler, extra, lod } => {
                let c = rd(regs, coord);
                let unit = rd(regs, sampler)[0].max(0.0) as usize;
                // Projective forms divide by the last component (§8.7).
                let (st, cube) = match kind {
                    TexKind::T2D => ([c[0], c[1], 0.0], false),
                    TexKind::T2DProj3 => ([c[0] / c[2], c[1] / c[2], 0.0], false),
                    TexKind::T2DProj4 => ([c[0] / c[3], c[1] / c[3], 0.0], false),
                    TexKind::Cube => ([c[0], c[1], c[2]], true),
                };
                let e = extra.map(|s| rd(regs, s)[0]).unwrap_or(0.0);
                let l = if lod {
                    Lod::Explicit(e)
                } else if !fragment {
                    // Vertex lookups without an explicit lod use level 0 (λ = 0, §8.7).
                    Lod::Bias(0.0)
                } else {
                    match &mut mode {
                        Mode::Plain => Lod::Bias(e),
                        Mode::Record(log) => {
                            log.push([st[0], st[1], st[2], 0.0]);
                            Lod::Bias(e)
                        }
                        Mode::Quad { x, y } => {
                            let g = |l: &[[f32; 4]]| -> [f32; 3] {
                                match l.get(k) {
                                    Some(n) => [n[0] - st[0], n[1] - st[1], n[2] - st[2]],
                                    None => [0.0; 3],
                                }
                            };
                            let (dx, dy) = (g(x), g(y));
                            k += 1;
                            Lod::Grad { dx, dy, bias: e }
                        }
                    }
                };
                let v = tex.sample(unit, cube, st, l);
                wr(regs, d, v);
            }
            Op::Deriv(f, d, a) => {
                let v = rd(regs, a);
                let out = match &mut mode {
                    Mode::Plain => [0.0; 4],
                    Mode::Record(log) => {
                        log.push(v);
                        [0.0; 4]
                    }
                    Mode::Quad { x, y } => {
                        let nx = x.get(k).copied().unwrap_or(v);
                        let ny = y.get(k).copied().unwrap_or(v);
                        k += 1;
                        let dx = map2(nx, v, |p, q| p - q);
                        let dy = map2(ny, v, |p, q| p - q);
                        match f {
                            Deriv::Dx => dx,
                            Deriv::Dy => dy,
                            Deriv::Fwidth => map2(dx, dy, |p, q| p.abs() + q.abs()),
                        }
                    }
                };
                wr(regs, d, out);
            }
        }
    }
    (Outcome::Done, ops)
}
