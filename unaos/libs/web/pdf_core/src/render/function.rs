// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF functions (ISO 32000-1 §7.10): type 0 (sampled, multilinear interpolation), type 2 (exponential), type 3
//! (stitching) and type 4 (the PostScript calculator, §7.10.5: a tiny stack machine over numbers and booleans), plus
//! an array of 1-output functions read as one. Domain and Range clip every input and output. Bounded: nesting by
//! [`MAX_NEST`], sampled inputs by [`MAX_INPUTS`], the calculator by [`MAX_STACK`] and [`MAX_STEPS`].

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use svg_core::fmath::{atan2, ceil, cos, floor, ln, powf, sin, sqrt};

use crate::doc::Document;
use crate::lexer::{Lexer, Token};
use crate::object::{Dict, Object};

/// The deepest stitching / array nesting followed.
pub const MAX_NEST: usize = 8;
/// The most inputs a sampled function may take.
pub const MAX_INPUTS: usize = 8;
/// The calculator's operand stack depth (§7.10.5 / Annex C: 100).
pub const MAX_STACK: usize = 100;
/// The most calculator operators one evaluation may run.
pub const MAX_STEPS: usize = 10_000;

/// A parsed function.
#[derive(Clone, Debug)]
pub enum Function {
    Sampled { domain: Vec<f64>, range: Vec<f64>, size: Vec<usize>, bps: u32, encode: Vec<f64>, decode: Vec<f64>, samples: Vec<u8> },
    Exponential { domain: Vec<f64>, range: Vec<f64>, c0: Vec<f64>, c1: Vec<f64>, n: f64 },
    Stitching { domain: Vec<f64>, range: Vec<f64>, functions: Vec<Function>, bounds: Vec<f64>, encode: Vec<f64> },
    Calculator { domain: Vec<f64>, range: Vec<f64>, program: Vec<Ps> },
    /// n 1-output functions whose outputs are concatenated (shadings, tint transforms).
    Array(Vec<Function>),
}

/// One calculator instruction (blocks are nested, `if` / `ifelse` hold theirs).
#[derive(Clone, Debug, PartialEq)]
pub enum Ps {
    Int(i64),
    Real(f64),
    Bool(bool),
    Op(Box<[u8]>),
    If(Vec<Ps>),
    IfElse(Vec<Ps>, Vec<Ps>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Val {
    I(i64),
    R(f64),
    B(bool),
}

impl Val {
    fn f(self) -> Option<f64> {
        match self {
            Val::I(i) => Some(i as f64),
            Val::R(r) => Some(r),
            Val::B(_) => None,
        }
    }
}

fn nums(doc: &Document, o: Option<&Object>) -> Option<Vec<f64>> {
    let a = doc.get(o?);
    let a = a.as_array()?;
    a.iter().map(|x| doc.get(x).as_f64()).collect()
}

fn interp(x: f64, x0: f64, x1: f64, y0: f64, y1: f64) -> f64 {
    if x1 == x0 { y0 } else { y0 + (x - x0) * (y1 - y0) / (x1 - x0) }
}

fn clip(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

impl Function {
    /// Parse a function object (a dictionary or a stream; or an array of them).
    pub fn parse(o: &Object, doc: &Document) -> Option<Function> {
        Self::parse_depth(o, doc, 0)
    }

    fn parse_depth(o: &Object, doc: &Document, depth: usize) -> Option<Function> {
        if depth > MAX_NEST {
            return None;
        }
        let o = doc.get(o);
        if let Object::Array(a) = &o {
            let fs: Option<Vec<Function>> = a.iter().map(|f| Self::parse_depth(f, doc, depth + 1)).collect();
            return fs.map(Function::Array);
        }
        let (dict, data): (Dict, Option<Vec<u8>>) = match &o {
            Object::Dict(d) => (d.clone(), None),
            Object::Stream(s) => (s.dict.clone(), Some(doc.decode(s).ok()?.data)),
            _ => return None,
        };
        let ty = doc.get(dict.get(b"FunctionType")?).as_int()?;
        let domain = nums(doc, dict.get(b"Domain")).filter(|d| d.len() >= 2 && d.len() % 2 == 0)?;
        let range = nums(doc, dict.get(b"Range")).filter(|r| r.len() % 2 == 0).unwrap_or_default();
        match ty {
            0 => {
                let m = domain.len() / 2;
                if m > MAX_INPUTS || range.is_empty() {
                    return None;
                }
                let size: Vec<usize> = nums(doc, dict.get(b"Size"))?.iter().map(|&v| v.max(1.0) as usize).collect();
                if size.len() != m {
                    return None;
                }
                let bps = doc.get(dict.get(b"BitsPerSample")?).as_int()? as u32;
                if !matches!(bps, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32) {
                    return None;
                }
                let total = size.iter().try_fold(range.len() / 2, |acc, &s| acc.checked_mul(s))?;
                if total.checked_mul(bps as usize)? / 8 > crate::MAX_DECODED {
                    return None;
                }
                let encode = nums(doc, dict.get(b"Encode"))
                    .filter(|e| e.len() == 2 * m)
                    .unwrap_or_else(|| size.iter().flat_map(|&s| [0.0, (s - 1) as f64]).collect());
                let decode = nums(doc, dict.get(b"Decode")).filter(|d| d.len() == range.len()).unwrap_or_else(|| range.clone());
                Some(Function::Sampled { domain, range, size, bps, encode, decode, samples: data? })
            }
            2 => {
                let c0 = nums(doc, dict.get(b"C0")).unwrap_or_else(|| vec![0.0]);
                let c1 = nums(doc, dict.get(b"C1")).unwrap_or_else(|| vec![1.0]);
                if c0.len() != c1.len() {
                    return None;
                }
                let n = doc.get(dict.get(b"N")?).as_f64()?;
                Some(Function::Exponential { domain, range, c0, c1, n })
            }
            3 => {
                let fo = doc.get(dict.get(b"Functions")?);
                let functions: Vec<Function> =
                    fo.as_array()?.iter().map(|f| Self::parse_depth(f, doc, depth + 1)).collect::<Option<_>>()?;
                let k = functions.len();
                let bounds = nums(doc, dict.get(b"Bounds")).unwrap_or_default();
                let encode = nums(doc, dict.get(b"Encode"))?;
                if k == 0 || bounds.len() != k - 1 || encode.len() != 2 * k {
                    return None;
                }
                Some(Function::Stitching { domain, range, functions, bounds, encode })
            }
            4 => {
                let program = compile(&data?)?;
                Some(Function::Calculator { domain, range, program })
            }
            _ => None,
        }
    }

    /// Evaluate. A malformed evaluation (a calculator error) yields the clipped zeros of its range.
    pub fn eval(&self, input: &[f64]) -> Vec<f64> {
        match self {
            Function::Array(fs) => fs.iter().flat_map(|f| f.eval(input)).collect(),
            Function::Sampled { domain, range, size, bps, encode, decode, samples } => {
                let m = domain.len() / 2;
                let n = range.len() / 2;
                let maxv = if *bps >= 32 { u32::MAX as f64 } else { ((1u64 << bps) - 1) as f64 };
                // Each input in sample coordinates: e_i in [0, size_i - 1].
                let mut e = vec![0.0; m];
                for i in 0..m {
                    let x = clip(input.get(i).copied().unwrap_or(0.0), domain[2 * i], domain[2 * i + 1]);
                    let v = interp(x, domain[2 * i], domain[2 * i + 1], encode[2 * i], encode[2 * i + 1]);
                    e[i] = clip(v, 0.0, (size[i] - 1) as f64);
                }
                let read = |idx: usize| -> f64 {
                    let bit = idx * *bps as usize;
                    let mut v: u64 = 0;
                    for b in 0..*bps as usize {
                        let p = bit + b;
                        let byte = samples.get(p / 8).copied().unwrap_or(0);
                        v = (v << 1) | ((byte >> (7 - p % 8)) & 1) as u64;
                    }
                    v as f64
                };
                let mut out = vec![0.0; n];
                // Multilinear: every corner of the cell around e, weighted.
                for corner in 0..(1usize << m) {
                    let mut w = 1.0;
                    let mut idx = 0usize;
                    let mut stride = 1usize;
                    for i in 0..m {
                        let lo = floor(e[i]);
                        let f = e[i] - lo;
                        let hi_bit = corner >> i & 1 == 1;
                        let c = if hi_bit { (lo as usize + 1).min(size[i] - 1) } else { lo as usize };
                        w *= if hi_bit { f } else { 1.0 - f };
                        idx += c * stride;
                        stride *= size[i];
                    }
                    if w == 0.0 {
                        continue;
                    }
                    for (j, o) in out.iter_mut().enumerate() {
                        *o += w * read(idx * n + j);
                    }
                }
                (0..n).map(|j| clip(interp(out[j], 0.0, maxv, decode[2 * j], decode[2 * j + 1]), range[2 * j], range[2 * j + 1])).collect()
            }
            Function::Exponential { domain, range, c0, c1, n } => {
                let x = clip(input.first().copied().unwrap_or(0.0), domain[0], domain[1]);
                let p = if *n == 1.0 { x } else { powf(x, *n) };
                let out: Vec<f64> = c0.iter().zip(c1).map(|(a, b)| a + p * (b - a)).collect();
                clip_range(out, range)
            }
            Function::Stitching { domain, range, functions, bounds, encode } => {
                let x = clip(input.first().copied().unwrap_or(0.0), domain[0], domain[1]);
                let k = functions.len();
                let i = bounds.iter().position(|&b| x < b).unwrap_or(k - 1);
                let lo = if i == 0 { domain[0] } else { bounds[i - 1] };
                let hi = if i == k - 1 { domain[1] } else { bounds[i] };
                let t = interp(x, lo, hi, encode[2 * i], encode[2 * i + 1]);
                clip_range(functions[i].eval(&[t]), range)
            }
            Function::Calculator { domain, range, program } => {
                let mut st: Vec<Val> = Vec::new();
                for (i, &x) in input.iter().enumerate().take(domain.len() / 2) {
                    st.push(Val::R(clip(x, domain[2 * i], domain[2 * i + 1])));
                }
                let n = range.len() / 2;
                let mut steps = 0;
                if run(program, &mut st, &mut steps).is_none() {
                    return clip_range(vec![0.0; n], range);
                }
                let vals: Vec<f64> = st.iter().map(|v| v.f().unwrap_or(0.0)).collect();
                let start = vals.len().saturating_sub(n);
                let mut out: Vec<f64> = vals[start..].to_vec();
                while out.len() < n {
                    out.insert(0, 0.0);
                }
                clip_range(out, range)
            }
        }
    }
}

fn clip_range(mut v: Vec<f64>, range: &[f64]) -> Vec<f64> {
    for (j, x) in v.iter_mut().enumerate() {
        if 2 * j + 1 < range.len() {
            *x = clip(*x, range[2 * j], range[2 * j + 1]);
        }
    }
    v
}

/// Compile a calculator program `{ ... }` into nested instructions.
pub fn compile(src: &[u8]) -> Option<Vec<Ps>> {
    let mut lx = Lexer::new(src);
    // A stack of open blocks; the outermost `{` opens the program.
    let mut blocks: Vec<Vec<Ps>> = Vec::new();
    // Blocks just closed and not yet consumed by `if` / `ifelse`.
    let mut pending: Vec<Vec<Ps>> = Vec::new();
    while let Some(t) = lx.next() {
        match t {
            Token::Keyword(b"{") => {
                blocks.push(Vec::new());
                if blocks.len() > MAX_NEST * 4 {
                    return None;
                }
            }
            Token::Keyword(b"}") => {
                let b = blocks.pop()?;
                if blocks.is_empty() {
                    return Some(b);
                }
                pending.push(b);
            }
            tok => {
                let cur = blocks.last_mut()?;
                match tok {
                    Token::Int(i) => cur.push(Ps::Int(i)),
                    Token::Real(r) => cur.push(Ps::Real(r)),
                    Token::Keyword(b"true") => cur.push(Ps::Bool(true)),
                    Token::Keyword(b"false") => cur.push(Ps::Bool(false)),
                    Token::Keyword(b"if") => {
                        let a = pending.pop()?;
                        cur.push(Ps::If(a));
                    }
                    Token::Keyword(b"ifelse") => {
                        let b = pending.pop()?;
                        let a = pending.pop()?;
                        cur.push(Ps::IfElse(a, b));
                    }
                    Token::Keyword(k) => cur.push(Ps::Op(k.to_vec().into_boxed_slice())),
                    _ => return None,
                }
            }
        }
    }
    None
}

fn run(prog: &[Ps], st: &mut Vec<Val>, steps: &mut usize) -> Option<()> {
    for ins in prog {
        *steps += 1;
        if *steps > MAX_STEPS || st.len() > MAX_STACK {
            return None;
        }
        match ins {
            Ps::Int(i) => st.push(Val::I(*i)),
            Ps::Real(r) => st.push(Val::R(*r)),
            Ps::Bool(b) => st.push(Val::B(*b)),
            Ps::If(a) => {
                if let Val::B(c) = st.pop()? {
                    if c {
                        run(a, st, steps)?;
                    }
                } else {
                    return None;
                }
            }
            Ps::IfElse(a, b) => {
                let Val::B(c) = st.pop()? else { return None };
                run(if c { a } else { b }, st, steps)?;
            }
            Ps::Op(op) => op_exec(op, st)?,
        }
    }
    Some(())
}

fn op_exec(op: &[u8], st: &mut Vec<Val>) -> Option<()> {
    let num1 = |st: &mut Vec<Val>| st.pop()?.f();
    match op {
        b"pop" => {
            st.pop()?;
        }
        b"dup" => {
            let v = *st.last()?;
            st.push(v);
        }
        b"exch" => {
            let n = st.len();
            if n < 2 {
                return None;
            }
            st.swap(n - 1, n - 2);
        }
        b"copy" => {
            let Val::I(k) = st.pop()? else { return None };
            if k < 0 || k as usize > st.len() {
                return None;
            }
            let from = st.len() - k as usize;
            for i in from..st.len() {
                st.push(st[i]);
            }
        }
        b"index" => {
            let Val::I(k) = st.pop()? else { return None };
            if k < 0 || k as usize >= st.len() {
                return None;
            }
            let v = st[st.len() - 1 - k as usize];
            st.push(v);
        }
        b"roll" => {
            let Val::I(j) = st.pop()? else { return None };
            let Val::I(n) = st.pop()? else { return None };
            if n < 0 || n as usize > st.len() {
                return None;
            }
            let n = n as usize;
            if n > 0 {
                let from = st.len() - n;
                let s = &mut st[from..];
                let j = j.rem_euclid(n as i64) as usize;
                s.rotate_right(j);
            }
        }
        b"abs" | b"neg" | b"ceiling" | b"floor" | b"round" | b"truncate" => {
            let v = st.pop()?;
            let out = match (op, v) {
                (b"abs", Val::I(i)) => Val::I(i.checked_abs()?),
                (b"neg", Val::I(i)) => Val::I(i.checked_neg()?),
                (_, Val::I(i)) => Val::I(i),
                (b"abs", Val::R(r)) => Val::R(r.abs()),
                (b"neg", Val::R(r)) => Val::R(-r),
                (b"ceiling", Val::R(r)) => Val::R(ceil(r)),
                (b"floor", Val::R(r)) => Val::R(floor(r)),
                // PostScript `round`: halves go up.
                (b"round", Val::R(r)) => Val::R(floor(r + 0.5)),
                (_, Val::R(r)) => Val::R(if r < 0.0 { ceil(r) } else { floor(r) }),
                _ => return None,
            };
            st.push(out);
        }
        b"add" | b"sub" | b"mul" => {
            let b = st.pop()?;
            let a = st.pop()?;
            let out = match (a, b) {
                (Val::I(x), Val::I(y)) => {
                    let r = match op {
                        b"add" => x.checked_add(y),
                        b"sub" => x.checked_sub(y),
                        _ => x.checked_mul(y),
                    };
                    match r {
                        Some(r) => Val::I(r),
                        None => Val::R(arith(op, x as f64, y as f64)),
                    }
                }
                (a, b) => Val::R(arith(op, a.f()?, b.f()?)),
            };
            st.push(out);
        }
        b"div" => {
            let b = num1(st)?;
            let a = num1(st)?;
            if b == 0.0 {
                return None;
            }
            st.push(Val::R(a / b));
        }
        b"idiv" | b"mod" => {
            let Val::I(b) = st.pop()? else { return None };
            let Val::I(a) = st.pop()? else { return None };
            if b == 0 {
                return None;
            }
            st.push(Val::I(if op == b"idiv" { a.checked_div(b)? } else { a.checked_rem(b)? }));
        }
        b"sqrt" => {
            let a = num1(st)?;
            if a < 0.0 {
                return None;
            }
            st.push(Val::R(sqrt(a)));
        }
        b"sin" | b"cos" => {
            let a = num1(st)? * core::f64::consts::PI / 180.0;
            st.push(Val::R(if op == b"sin" { sin(a) } else { cos(a) }));
        }
        b"atan" => {
            let den = num1(st)?;
            let num = num1(st)?;
            let mut d = atan2(num, den) * 180.0 / core::f64::consts::PI;
            if d < 0.0 {
                d += 360.0;
            }
            st.push(Val::R(d));
        }
        b"exp" => {
            let e = num1(st)?;
            let b = num1(st)?;
            st.push(Val::R(powf(b, e)));
        }
        b"ln" | b"log" => {
            let a = num1(st)?;
            if a <= 0.0 {
                return None;
            }
            st.push(Val::R(if op == b"ln" { ln(a) } else { ln(a) / core::f64::consts::LN_10 }));
        }
        b"cvi" => {
            let a = num1(st)?;
            st.push(Val::I(if a < 0.0 { ceil(a) } else { floor(a) } as i64));
        }
        b"cvr" => {
            let a = num1(st)?;
            st.push(Val::R(a));
        }
        b"eq" | b"ne" | b"gt" | b"ge" | b"lt" | b"le" => {
            let b = st.pop()?;
            let a = st.pop()?;
            let r = match (a, b) {
                (Val::B(x), Val::B(y)) => match op {
                    b"eq" => x == y,
                    b"ne" => x != y,
                    _ => return None,
                },
                (a, b) => {
                    let (x, y) = (a.f()?, b.f()?);
                    match op {
                        b"eq" => x == y,
                        b"ne" => x != y,
                        b"gt" => x > y,
                        b"ge" => x >= y,
                        b"lt" => x < y,
                        _ => x <= y,
                    }
                }
            };
            st.push(Val::B(r));
        }
        b"and" | b"or" | b"xor" => {
            let b = st.pop()?;
            let a = st.pop()?;
            let out = match (a, b) {
                (Val::B(x), Val::B(y)) => Val::B(match op {
                    b"and" => x & y,
                    b"or" => x | y,
                    _ => x ^ y,
                }),
                (Val::I(x), Val::I(y)) => Val::I(match op {
                    b"and" => x & y,
                    b"or" => x | y,
                    _ => x ^ y,
                }),
                _ => return None,
            };
            st.push(out);
        }
        b"not" => {
            let out = match st.pop()? {
                Val::B(x) => Val::B(!x),
                Val::I(x) => Val::I(!x),
                Val::R(_) => return None,
            };
            st.push(out);
        }
        b"bitshift" => {
            let Val::I(s) = st.pop()? else { return None };
            let Val::I(a) = st.pop()? else { return None };
            let v = if s >= 0 { a.checked_shl(s as u32).unwrap_or(0) } else { a.checked_shr((-s) as u32).unwrap_or(0) };
            st.push(Val::I(v));
        }
        b"true" => st.push(Val::B(true)),
        b"false" => st.push(Val::B(false)),
        _ => return None,
    }
    Some(())
}

fn arith(op: &[u8], a: f64, b: f64) -> f64 {
    match op {
        b"add" => a + b,
        b"sub" => a - b,
        _ => a * b,
    }
}
