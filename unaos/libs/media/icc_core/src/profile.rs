// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ICC.1:2010 (v2 and v4) profile parsing (PDFICC, LEDGER SR358).
//!
//! The 128-byte header (§7.2: size, version, class, data colour space, PCS, rendering intent), the tag table (§7.3,
//! every tag bounds-checked inside the profile) and the tag types a source profile needs to reach the PCS:
//! `XYZ ` (§10.31), `curv` (§10.6: identity, u8Fixed8 gamma, u16 table), `para` (§10.18, function types 0-4),
//! `mft1` lut8 / `mft2` lut16 (§10.10 / §10.11: input tables, CLUT, output tables; the matrix applies only to XYZ
//! input, which a GRAY / RGB / CMYK source never is) and `mAB ` lutAToB (§10.12: A curves, CLUT, M curves, 3x4
//! matrix, B curves). The model is an A2B tag when one is present (the skcms order; the header intent picks A2B0 /
//! A2B1 / A2B2), else matrix/TRC (rXYZ gXYZ bXYZ + rTRC gTRC bTRC) for RGB and kTRC for GRAY.
//!
//! Refused by name: named-colour, device-link and abstract profiles, multiProcessElements (`mpet`, `D2Bx`), ICC v5
//! (iccMAX), data colour spaces other than GRAY / RGB / CMYK, and a lutBToA where an A2B tag belongs (output-side
//! mBA / B2A is owed).

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

/// A profile's data colour space (§7.2.6), the ones a source profile is read for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Gray,
    Rgb,
    Cmyk,
}

impl Space {
    /// Channels per sample.
    pub fn channels(self) -> usize {
        match self {
            Space::Gray => 1,
            Space::Rgb => 3,
            Space::Cmyk => 4,
        }
    }
}

/// The profile connection space (§7.2.7), D50.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pcs {
    Xyz,
    Lab,
}

/// Why a profile is not used. Every variant names the feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Class `nmcl`.
    NamedColour,
    /// Class `link`.
    DeviceLink,
    /// Class `abst`.
    Abstract,
    /// An `mpet` tag type, or a `D2Bx` tag.
    MultiProcessElements,
    /// The bytes break the format.
    Malformed(&'static str),
    /// A valid profile using something this engine does not convert.
    Unsupported(&'static str),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NamedColour => f.write_str("named-colour profile"),
            Refusal::DeviceLink => f.write_str("device-link profile"),
            Refusal::Abstract => f.write_str("abstract profile"),
            Refusal::MultiProcessElements => f.write_str("multiProcessElements"),
            Refusal::Malformed(why) => write!(f, "malformed ICC profile: {why}"),
            Refusal::Unsupported(why) => write!(f, "unsupported ICC profile: {why}"),
        }
    }
}

/// One tone curve, input and output 0..=1.
#[derive(Clone, Debug, PartialEq)]
pub enum Curve {
    Identity,
    /// `curv` with one entry: `x^g`.
    Gamma(f32),
    /// `curv` with n >= 2 entries, linearly interpolated.
    Table(Vec<u16>),
    /// `para`: the function type (0-4) and its parameters in the order g, a, b, c, d, e, f (unused ones 0).
    Para(u8, [f32; 7]),
}

impl Curve {
    /// The curve at `x` (clamped to 0..=1 on the way in; the result is clamped too).
    pub fn eval(&self, x: f32) -> f32 {
        let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        let pw = |b: f32, g: f32| if b <= 0.0 { 0.0 } else { libm::powf(b, g) };
        let y = match self {
            Curve::Identity => x,
            Curve::Gamma(g) => pw(x, *g),
            Curve::Table(t) => {
                let n = t.len();
                let p = x * (n - 1) as f32;
                let i = (p as usize).min(n - 2);
                let f = p - i as f32;
                (t[i] as f32 + (t[i + 1] as f32 - t[i] as f32) * f) / 65535.0
            }
            Curve::Para(ty, p) => {
                let [g, a, b, c, d, e, f] = *p;
                match *ty {
                    0 => pw(x, g),
                    1 => {
                        if a != 0.0 && x >= -b / a { pw(a * x + b, g) } else { 0.0 }
                    }
                    2 => {
                        if a != 0.0 && x >= -b / a { pw(a * x + b, g) + c } else { c }
                    }
                    3 => {
                        if x >= d { pw(a * x + b, g) } else { c * x }
                    }
                    _ => {
                        if x >= d { pw(a * x + b, g) + e } else { c * x + f }
                    }
                }
            }
        };
        if y.is_nan() { 0.0 } else { y.clamp(0.0, 1.0) }
    }
}

/// A colour lookup table: `ins` inputs on a grid of `grid[d]` points each (the first input varies slowest), `outs`
/// outputs per node, every value scaled to 0..=65535.
#[derive(Clone, Debug, PartialEq)]
pub struct Clut {
    pub grid: Vec<u8>,
    pub outs: usize,
    pub data: Vec<u16>,
}

impl Clut {
    /// Multilinear interpolation (trilinear for 3 inputs, quadrilinear for 4) of `input` (0..=1 each) into `out`.
    pub fn eval(&self, input: &[f32], out: &mut [f32]) {
        let ins = self.grid.len();
        let mut lo = [0usize; 8];
        let mut hi = [0usize; 8];
        let mut fr = [0f32; 8];
        let mut stride = [0usize; 8];
        let mut s = self.outs;
        for d in (0..ins).rev() {
            stride[d] = s;
            s *= self.grid[d] as usize;
        }
        for d in 0..ins {
            let g = self.grid[d] as usize;
            let x = input.get(d).copied().unwrap_or(0.0);
            let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
            let p = x * (g - 1) as f32;
            let i = (p as usize).min(g.saturating_sub(2));
            lo[d] = i;
            hi[d] = (i + 1).min(g - 1);
            fr[d] = (p - i as f32).clamp(0.0, 1.0);
        }
        for o in out.iter_mut() {
            *o = 0.0;
        }
        for corner in 0..(1usize << ins) {
            let mut w = 1.0f32;
            let mut off = 0usize;
            for d in 0..ins {
                if corner >> d & 1 == 1 {
                    w *= fr[d];
                    off += hi[d] * stride[d];
                } else {
                    w *= 1.0 - fr[d];
                    off += lo[d] * stride[d];
                }
            }
            if w == 0.0 {
                continue;
            }
            for (k, o) in out.iter_mut().enumerate().take(self.outs) {
                *o += w * self.data[off + k] as f32 / 65535.0;
            }
        }
    }
}

/// Which lut type the A2B tag was: it fixes the Lab PCS encoding (§10.10-10.12; v2 legacy 16-bit for lut16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LutKind {
    Lut8,
    Lut16,
    AToB,
}

/// An A2B pipeline: `a` curves -> `clut` -> `m` curves -> `matrix` (3x3 + offsets) -> `b` curves. lut8 / lut16 fill
/// `a` (input tables), `clut` and `b` (output tables) only.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut {
    pub kind: LutKind,
    pub a: Vec<Curve>,
    pub clut: Option<Clut>,
    pub m: Vec<Curve>,
    pub matrix: Option<[f32; 12]>,
    pub b: Vec<Curve>,
}

impl Lut {
    /// The PCS values (0..=1 each, still in the tag's encoding) of one sample.
    pub fn eval(&self, input: &[f32]) -> [f32; 3] {
        let mut v = [0f32; 15];
        let n = input.len().min(15);
        for i in 0..n {
            v[i] = match self.a.get(i) {
                Some(c) => c.eval(input[i]),
                None => input[i],
            };
        }
        let mut p = [0f32; 3];
        match &self.clut {
            Some(c) => c.eval(&v[..n], &mut p),
            None => p.copy_from_slice(&v[..3]),
        }
        for (i, x) in p.iter_mut().enumerate() {
            if let Some(c) = self.m.get(i) {
                *x = c.eval(*x);
            }
        }
        if let Some(m) = &self.matrix {
            let q = p;
            for (r, x) in p.iter_mut().enumerate() {
                *x = (m[r * 3] * q[0] + m[r * 3 + 1] * q[1] + m[r * 3 + 2] * q[2] + m[9 + r]).clamp(0.0, 1.0);
            }
        }
        for (i, x) in p.iter_mut().enumerate() {
            if let Some(c) = self.b.get(i) {
                *x = c.eval(*x);
            }
        }
        p
    }
}

/// How a profile reaches the PCS.
#[derive(Clone, Debug, PartialEq)]
pub enum Model {
    /// RGB: `m[i][c]` is colorant c's XYZ component i (c = r, g, b), after each channel's TRC.
    MatrixTrc { m: [[f32; 3]; 3], trc: [Curve; 3] },
    /// GRAY: kTRC to Y (XYZ PCS) or L*/100 (Lab PCS).
    GrayTrc(Curve),
    /// An A2B tag.
    Lut(Lut),
}

/// A parsed source profile.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    /// Header bytes 8..12 (major, minor/bugfix, 0, 0).
    pub version: u32,
    pub class: [u8; 4],
    pub space: Space,
    pub pcs: Pcs,
    /// 0 perceptual, 1 relative colorimetric, 2 saturation, 3 absolute colorimetric.
    pub intent: u32,
    pub model: Model,
}

/// sRGB's TRC as a `para` type 3 (IEC 61966-2-1).
pub const SRGB_TRC: Curve = Curve::Para(3, [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045, 0.0, 0.0]);

/// The built-in sRGB profile: the D50-adapted sRGB colorants (the ICC's own sRGB profile values) and its TRC.
pub const SRGB: Profile = Profile {
    version: 0x0430_0000,
    class: *b"mntr",
    space: Space::Rgb,
    pcs: Pcs::Xyz,
    intent: 0,
    model: Model::MatrixTrc {
        m: [[0.4361, 0.3851, 0.1431], [0.2225, 0.7169, 0.0606], [0.0139, 0.0971, 0.7141]],
        trc: [SRGB_TRC, SRGB_TRC, SRGB_TRC],
    },
};

fn be16(b: &[u8], at: usize) -> Result<u16, Refusal> {
    b.get(at..at + 2).map(|s| u16::from_be_bytes([s[0], s[1]])).ok_or(Refusal::Malformed("tag data truncated"))
}

fn be32(b: &[u8], at: usize) -> Result<u32, Refusal> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]])).ok_or(Refusal::Malformed("tag data truncated"))
}

fn s15(b: &[u8], at: usize) -> Result<f32, Refusal> {
    Ok(be32(b, at)? as i32 as f32 / 65536.0)
}

/// The most tags read (real profiles carry 10-30).
const MAX_TAGS: usize = 100;
/// The most CLUT values accepted (a 33^4 x 3 CMYK table is 3.6 M).
const MAX_CLUT: usize = 1 << 24;

impl Profile {
    /// Parse a profile (the bytes an iCCP / APP2 / ICCP chunk, a JP2 colr box or a PDF /ICCBased stream carries).
    pub fn parse(bytes: &[u8]) -> Result<Profile, Refusal> {
        if bytes.len() < 132 {
            return Err(Refusal::Malformed("shorter than the header and tag count"));
        }
        let size = be32(bytes, 0)? as usize;
        if size < 132 || size > bytes.len() {
            return Err(Refusal::Malformed("profile size"));
        }
        let b = &bytes[..size];
        let version = be32(b, 8)?;
        match b[8] {
            2 | 4 => {}
            5 => return Err(Refusal::Unsupported("ICC v5 (iccMAX)")),
            _ => return Err(Refusal::Unsupported("ICC version")),
        }
        let class: [u8; 4] = [b[12], b[13], b[14], b[15]];
        match &class {
            b"nmcl" => return Err(Refusal::NamedColour),
            b"link" => return Err(Refusal::DeviceLink),
            b"abst" => return Err(Refusal::Abstract),
            b"mntr" | b"scnr" | b"spac" | b"prtr" => {}
            _ => return Err(Refusal::Unsupported("profile class")),
        }
        let space = match &b[16..20] {
            b"GRAY" => Space::Gray,
            b"RGB " => Space::Rgb,
            b"CMYK" => Space::Cmyk,
            _ => return Err(Refusal::Unsupported("data colour space")),
        };
        let pcs = match &b[20..24] {
            b"XYZ " => Pcs::Xyz,
            b"Lab " => Pcs::Lab,
            _ => return Err(Refusal::Malformed("PCS")),
        };
        let intent = be32(b, 64)?;
        let intent = if intent > 3 { 0 } else { intent };
        let count = be32(b, 128)? as usize;
        if count > MAX_TAGS || 132 + 12 * count > size {
            return Err(Refusal::Malformed("tag count"));
        }
        let mut tags: Vec<([u8; 4], &[u8])> = Vec::with_capacity(count);
        for i in 0..count {
            let e = 132 + 12 * i;
            let off = be32(b, e + 4)? as usize;
            let len = be32(b, e + 8)? as usize;
            let end = off.checked_add(len).ok_or(Refusal::Malformed("tag outside the profile"))?;
            if end > size || len < 8 {
                return Err(Refusal::Malformed("tag outside the profile"));
            }
            tags.push(([b[e], b[e + 1], b[e + 2], b[e + 3]], &b[off..end]));
        }
        let find = |sig: &[u8; 4]| tags.iter().find(|t| &t.0 == sig).map(|t| t.1);
        if find(b"D2B0").is_some() || find(b"D2B1").is_some() || find(b"D2B2").is_some() {
            return Err(Refusal::MultiProcessElements);
        }
        let order: &[&[u8; 4]] = match intent {
            0 => &[b"A2B0"],
            2 => &[b"A2B2", b"A2B0"],
            _ => &[b"A2B1", b"A2B0"],
        };
        let a2b = order.iter().find_map(|s| find(s));
        let model = if let Some(t) = a2b {
            Model::Lut(parse_lut(t, space.channels())?)
        } else {
            match space {
                Space::Rgb => {
                    if pcs != Pcs::Xyz {
                        return Err(Refusal::Unsupported("matrix/TRC over a Lab PCS"));
                    }
                    let need = |s: &[u8; 4]| find(s).ok_or(Refusal::Malformed("matrix/TRC tag missing"));
                    let (r, g, bl) = (xyz(need(b"rXYZ")?)?, xyz(need(b"gXYZ")?)?, xyz(need(b"bXYZ")?)?);
                    let m = [[r[0], g[0], bl[0]], [r[1], g[1], bl[1]], [r[2], g[2], bl[2]]];
                    let trc = [curve(need(b"rTRC")?)?.0, curve(need(b"gTRC")?)?.0, curve(need(b"bTRC")?)?.0];
                    Model::MatrixTrc { m, trc }
                }
                Space::Gray => Model::GrayTrc(curve(find(b"kTRC").ok_or(Refusal::Malformed("kTRC missing"))?)?.0),
                Space::Cmyk => return Err(Refusal::Unsupported("CMYK profile without an A2B tag")),
            }
        };
        Ok(Profile { version, class, space, pcs, intent, model })
    }

    /// sRGB, by content rather than by name (the skcms / Chromium short-circuit): an RGB matrix/TRC profile over an XYZ
    /// PCS whose colorants are each within 0.01 of [`SRGB`]'s and whose TRCs are within 1/255 of sRGB's at 17 points.
    pub fn is_srgb(&self) -> bool {
        let (Model::MatrixTrc { m, trc }, Model::MatrixTrc { m: sm, .. }) = (&self.model, &SRGB.model) else {
            return false;
        };
        if self.space != Space::Rgb || self.pcs != Pcs::Xyz {
            return false;
        }
        let close = |a: f32, b: f32, t: f32| libm::fabsf(a - b) <= t;
        for i in 0..3 {
            for c in 0..3 {
                if !close(m[i][c], sm[i][c], 0.01) {
                    return false;
                }
            }
        }
        trc.iter().all(|t| (0..=16).all(|k| close(t.eval(k as f32 / 16.0), SRGB_TRC.eval(k as f32 / 16.0), 1.0 / 255.0)))
    }
}

fn xyz(t: &[u8]) -> Result<[f32; 3], Refusal> {
    if &t[..4] != b"XYZ " || t.len() < 20 {
        return Err(Refusal::Malformed("XYZ tag"));
    }
    Ok([s15(t, 8)?, s15(t, 12)?, s15(t, 16)?])
}

/// A `curv` or `para` at the start of `t`: the curve and the bytes it occupies (unpadded).
fn curve(t: &[u8]) -> Result<(Curve, usize), Refusal> {
    match t.get(..4) {
        Some(b"curv") => {
            let n = be32(t, 8)? as usize;
            if n > (t.len().saturating_sub(12)) / 2 {
                return Err(Refusal::Malformed("curv count"));
            }
            let c = match n {
                0 => Curve::Identity,
                1 => Curve::Gamma(be16(t, 12)? as f32 / 256.0),
                _ => Curve::Table((0..n).map(|i| be16(t, 12 + 2 * i)).collect::<Result<Vec<_>, _>>()?),
            };
            Ok((c, 12 + 2 * n))
        }
        Some(b"para") => {
            let ty = be16(t, 8)?;
            let n = match ty {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return Err(Refusal::Malformed("para function type")),
            };
            let mut p = [0f32; 7];
            for (i, v) in p.iter_mut().enumerate().take(n) {
                *v = s15(t, 12 + 4 * i)?;
            }
            Ok((Curve::Para(ty as u8, p), 12 + 4 * n))
        }
        _ => Err(Refusal::Malformed("curve type")),
    }
}

/// `n` curves in sequence from `at`, each starting on a 4-byte boundary (§10.12).
fn curves(t: &[u8], mut at: usize, n: usize) -> Result<Vec<Curve>, Refusal> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let (c, len) = curve(t.get(at..).ok_or(Refusal::Malformed("mAB curve offset"))?)?;
        out.push(c);
        at += (len + 3) & !3;
    }
    Ok(out)
}

fn clut_len(grid: &[u8], outs: usize) -> Result<usize, Refusal> {
    let mut n = outs;
    for &g in grid {
        if g < 2 {
            return Err(Refusal::Malformed("CLUT grid"));
        }
        n = n.checked_mul(g as usize).filter(|&n| n <= MAX_CLUT).ok_or(Refusal::Malformed("CLUT too large"))?;
    }
    Ok(n)
}

/// An A2B tag for a source with `ins` channels into the 3-component PCS.
fn parse_lut(t: &[u8], ins: usize) -> Result<Lut, Refusal> {
    match t.get(..4) {
        Some(sig @ (b"mft1" | b"mft2")) => {
            let lut8 = sig == b"mft1";
            let (i, o, g) = (*t.get(8).unwrap_or(&0) as usize, *t.get(9).unwrap_or(&0) as usize, *t.get(10).unwrap_or(&0));
            if i != ins || o != 3 {
                return Err(Refusal::Malformed("lut channel counts"));
            }
            // lut8: 256-entry tables from byte 48; lut16: the input and output table sizes at 48 / 50, tables from 52.
            let (n, no, mut at, e) =
                if lut8 { (256usize, 256usize, 48usize, 1usize) } else { (be16(t, 48)? as usize, be16(t, 50)? as usize, 52usize, 2usize) };
            if !(2..=4096).contains(&n) || !(2..=4096).contains(&no) {
                return Err(Refusal::Malformed("lut table size"));
            }
            let grid = vec![g; i];
            let cl = clut_len(&grid, o)?;
            if t.len() < at + e * (i * n + cl + o * no) {
                return Err(Refusal::Malformed("lut data truncated"));
            }
            let mut read = |count: usize| -> Vec<u16> {
                let v = (0..count).map(|k| if lut8 { t[at + k] as u16 * 257 } else { u16::from_be_bytes([t[at + 2 * k], t[at + 2 * k + 1]]) }).collect();
                at += e * count;
                v
            };
            let a: Vec<Curve> = (0..i).map(|_| Curve::Table(read(n))).collect();
            let data = read(cl);
            let b: Vec<Curve> = (0..o).map(|_| Curve::Table(read(no))).collect();
            Ok(Lut { kind: if lut8 { LutKind::Lut8 } else { LutKind::Lut16 }, a, clut: Some(Clut { grid, outs: o, data }), m: Vec::new(), matrix: None, b })
        }
        Some(b"mAB ") => {
            let (i, o) = (*t.get(8).unwrap_or(&0) as usize, *t.get(9).unwrap_or(&0) as usize);
            if i != ins || o != 3 {
                return Err(Refusal::Malformed("mAB channel counts"));
            }
            let (ob, om, omc, oc, oa) = (be32(t, 12)? as usize, be32(t, 16)? as usize, be32(t, 20)? as usize, be32(t, 24)? as usize, be32(t, 28)? as usize);
            if ob == 0 {
                return Err(Refusal::Malformed("mAB without B curves"));
            }
            let b = curves(t, ob, o)?;
            let matrix = if om != 0 {
                let mut m = [0f32; 12];
                for (k, v) in m.iter_mut().enumerate() {
                    *v = s15(t, om + 4 * k)?;
                }
                Some(m)
            } else {
                None
            };
            let m = if omc != 0 { curves(t, omc, o)? } else { Vec::new() };
            let clut = if oc != 0 {
                let grid: Vec<u8> = t.get(oc..oc + i).ok_or(Refusal::Malformed("mAB CLUT offset"))?.to_vec();
                let prec = *t.get(oc + 16).ok_or(Refusal::Malformed("mAB CLUT offset"))? as usize;
                if prec != 1 && prec != 2 {
                    return Err(Refusal::Malformed("mAB CLUT precision"));
                }
                let cl = clut_len(&grid, o)?;
                let d = oc + 20;
                if t.len() < d + prec * cl {
                    return Err(Refusal::Malformed("mAB CLUT truncated"));
                }
                let data = (0..cl).map(|k| if prec == 1 { t[d + k] as u16 * 257 } else { u16::from_be_bytes([t[d + 2 * k], t[d + 2 * k + 1]]) }).collect();
                Some(Clut { grid, outs: o, data })
            } else {
                if i != 3 {
                    return Err(Refusal::Malformed("mAB without a CLUT needs 3 inputs"));
                }
                None
            };
            let a = if oa != 0 { curves(t, oa, i)? } else { Vec::new() };
            Ok(Lut { kind: LutKind::AToB, a, clut, m, matrix, b })
        }
        Some(b"mpet") => Err(Refusal::MultiProcessElements),
        Some(b"mBA ") => Err(Refusal::Unsupported("lutBToA in an A2B tag")),
        _ => Err(Refusal::Unsupported("A2B tag type")),
    }
}

/// Profile WRITERS for known-answer tests: the one builder icc_core's, pixel_core's, pdf_core's and Facet's suites
/// share (gneiss_pal's `avif_kat` shape), so no suite hand-types a profile.
pub mod kat {
    use alloc::vec::Vec;

    fn s15(v: f32) -> [u8; 4] {
        (libm::roundf(v * 65536.0) as i32).to_be_bytes()
    }

    /// An `XYZ ` tag.
    pub fn xyz_tag(v: [f32; 3]) -> Vec<u8> {
        let mut t = b"XYZ \0\0\0\0".to_vec();
        for c in v {
            t.extend(s15(c));
        }
        t
    }

    /// A one-entry `curv` (u8Fixed8 gamma).
    pub fn curv_gamma(g: f32) -> Vec<u8> {
        let mut t = b"curv\0\0\0\0".to_vec();
        t.extend(1u32.to_be_bytes());
        t.extend((libm::roundf(g * 256.0) as u16).to_be_bytes());
        t
    }

    /// A `curv` table.
    pub fn curv_table(v: &[u16]) -> Vec<u8> {
        let mut t = b"curv\0\0\0\0".to_vec();
        t.extend((v.len() as u32).to_be_bytes());
        for x in v {
            t.extend(x.to_be_bytes());
        }
        t
    }

    /// A `para` of function type `ty` with its parameters (g, a, b, c, d, e, f as the type uses them).
    pub fn para(ty: u16, p: &[f32]) -> Vec<u8> {
        let mut t = b"para\0\0\0\0".to_vec();
        t.extend(ty.to_be_bytes());
        t.extend([0, 0]);
        for &x in p {
            t.extend(s15(x));
        }
        t
    }

    /// A whole profile: header (version major `major`, class, space, PCS, intent 0, D50 illuminant, `acsp`) and the
    /// tags, each tag's data on a 4-byte boundary.
    pub fn profile(major: u8, class: &[u8; 4], space: &[u8; 4], pcs: &[u8; 4], tags: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut h = alloc::vec![0u8; 128];
        h[8] = major;
        h[9] = if major == 4 { 0x30 } else { 0x10 };
        h[12..16].copy_from_slice(class);
        h[16..20].copy_from_slice(space);
        h[20..24].copy_from_slice(pcs);
        h[36..40].copy_from_slice(b"acsp");
        h[68..72].copy_from_slice(&s15(0.9642));
        h[72..76].copy_from_slice(&s15(1.0));
        h[76..80].copy_from_slice(&s15(0.8249));
        h.extend((tags.len() as u32).to_be_bytes());
        let mut off = 132 + 12 * tags.len();
        let mut data = Vec::new();
        for (sig, d) in tags {
            h.extend(sig);
            h.extend((off as u32).to_be_bytes());
            h.extend((d.len() as u32).to_be_bytes());
            data.extend(d);
            while data.len() % 4 != 0 {
                data.push(0);
            }
            off = 132 + 12 * tags.len() + data.len();
        }
        h.extend(data);
        let n = h.len() as u32;
        h[0..4].copy_from_slice(&n.to_be_bytes());
        h
    }

    /// A v2 RGB display profile: colorants r, g, b (each XYZ D50) and one TRC tag shared by the three channels.
    pub fn matrix_trc_v2(colorants: [[f32; 3]; 3], trc: Vec<u8>) -> Vec<u8> {
        profile(
            2,
            b"mntr",
            b"RGB ",
            b"XYZ ",
            &[
                (*b"wtpt", xyz_tag([0.9642, 1.0, 0.8249])),
                (*b"rXYZ", xyz_tag(colorants[0])),
                (*b"gXYZ", xyz_tag(colorants[1])),
                (*b"bXYZ", xyz_tag(colorants[2])),
                (*b"rTRC", trc.clone()),
                (*b"gTRC", trc.clone()),
                (*b"bTRC", trc),
            ],
        )
    }

    /// Adobe RGB (1998) as AdobeRGB1998.icc states it: its D50 colorants and the curv gamma 563/256.
    pub fn adobe_rgb_v2() -> Vec<u8> {
        matrix_trc_v2(
            [[0.60974, 0.31111, 0.01947], [0.20528, 0.62567, 0.06087], [0.14919, 0.06322, 0.74457]],
            curv_gamma(563.0 / 256.0),
        )
    }

    /// sRGB as a v2 profile: the D50 colorants and a 1024-entry curv table of the sRGB curve.
    pub fn srgb_v2() -> Vec<u8> {
        let t: Vec<u16> = (0..1024).map(|i| libm::roundf(super::SRGB_TRC.eval(i as f32 / 1023.0) * 65535.0) as u16).collect();
        matrix_trc_v2([[0.4361, 0.2225, 0.0139], [0.3851, 0.7169, 0.0971], [0.1431, 0.0606, 0.7141]], curv_table(&t))
    }

    /// A v2 GRAY display profile with a gamma kTRC.
    pub fn gray_v2(gamma: f32) -> Vec<u8> {
        profile(2, b"mntr", b"GRAY", b"XYZ ", &[(*b"wtpt", xyz_tag([0.9642, 1.0, 0.8249])), (*b"kTRC", curv_gamma(gamma))])
    }

    /// The Lab D50 of an sRGB colour (0..=1 each), for building test CLUTs.
    pub fn srgb_to_lab(rgb: [f32; 3]) -> [f32; 3] {
        let super::Model::MatrixTrc { m, .. } = &super::SRGB.model else { unreachable!() };
        let lin = rgb.map(|c| super::SRGB_TRC.eval(c));
        let x: [f32; 3] = core::array::from_fn(|i| m[i][0] * lin[0] + m[i][1] * lin[1] + m[i][2] * lin[2]);
        let f = |t: f32| if t > 216.0 / 24389.0 { libm::cbrtf(t) } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
        let (fx, fy, fz) = (f(x[0] / 0.9642), f(x[1]), f(x[2] / 0.8249));
        [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
    }

    /// A v2 CMYK output profile with an A2B0 lut16 (Lab PCS, legacy encoding): identity 2-point tables and a 2^4 CLUT
    /// holding the Lab of the naive conversion R = (1-C)(1-K), G = (1-M)(1-K), B = (1-Y)(1-K) at each corner.
    pub fn lut16_cmyk_v2() -> Vec<u8> {
        let mut t = b"mft2\0\0\0\0".to_vec();
        t.extend([4, 3, 2, 0]);
        for k in [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            t.extend(s15(k));
        }
        t.extend(2u16.to_be_bytes());
        t.extend(2u16.to_be_bytes());
        for _ in 0..4 {
            t.extend([0u8, 0, 0xFF, 0xFF]);
        }
        for corner in 0..16u32 {
            // The first input (C) varies slowest.
            let bit = |d: u32| ((corner >> (3 - d)) & 1) as f32;
            let k = 1.0 - bit(3);
            let lab = srgb_to_lab([(1.0 - bit(0)) * k, (1.0 - bit(1)) * k, (1.0 - bit(2)) * k]);
            let enc = [lab[0] / 100.0 * 65280.0, (lab[1] + 128.0) * 256.0, (lab[2] + 128.0) * 256.0];
            for v in enc {
                t.extend((libm::roundf(v.clamp(0.0, 65535.0)) as u16).to_be_bytes());
            }
        }
        for _ in 0..3 {
            t.extend([0u8, 0, 0xFF, 0xFF]);
        }
        profile(2, b"prtr", b"CMYK", b"Lab ", &[(*b"wtpt", xyz_tag([0.9642, 1.0, 0.8249])), (*b"A2B0", t)])
    }

    /// A named-colour profile (class `nmcl`), header only.
    pub fn named_colour() -> Vec<u8> {
        profile(2, b"nmcl", b"RGB ", b"Lab ", &[])
    }

    /// A device-link profile (class `link`), header only.
    pub fn device_link() -> Vec<u8> {
        profile(4, b"link", b"RGB ", b"RGB ", &[])
    }

    /// An abstract profile (class `abst`), header only.
    pub fn abstract_profile() -> Vec<u8> {
        profile(4, b"abst", b"Lab ", b"Lab ", &[])
    }

    /// A v4 RGB profile whose A2B0 is a multiProcessElements (`mpet`) tag.
    pub fn mpet_rgb() -> Vec<u8> {
        let mut t = b"mpet\0\0\0\0".to_vec();
        t.extend([0, 3, 0, 3, 0, 0, 0, 0]);
        profile(4, b"mntr", b"RGB ", b"XYZ ", &[(*b"A2B0", t)])
    }
}
