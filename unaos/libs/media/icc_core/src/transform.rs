// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A source profile to sRGB (PDFICC, LEDGER SR358).
//!
//! source -> PCS (XYZ D50, or Lab D50 turned into XYZ) -> ONE output matrix `inv(M_sRGB_D65) . bradford(D50, D65)` ->
//! the sRGB OETF (IEC 61966-2-1). Matrix/TRC and GRAY profiles take an 8-bit cached path: per-channel linearisation
//! tables of 256 entries built once, the profile's matrix folded into the output matrix (one 3x3 per pixel), and a
//! 4096-step interpolated OETF table; a GRAY profile becomes one 256-entry RGB table. LUT profiles evaluate their
//! pipeline per pixel (the last sample is memoised). Lab PCS values decode by the tag type's encoding: lut16 the v2
//! legacy one (L* 100 = 0xFF00), lut8 and lutAToB the v4 one (L* 100 = 0xFFFF).
//! KERNEL-ICC-INT (LEDGER SR376): `apply_rgba` runs a matrix/TRC source in integers (`Q14Run`, DisplayTransform's
//! shape: u16 decode tables, a Q14 3x3 in i64, the ONE `encode_table` of sRGB); `apply_rgba_f32` is the f32 reference.
//! KERNEL-ICC-INT-LUT / -BUILD / PNG16-ICC (LEDGER SR380): an RGB lut8 / lut16 / mAB source runs in integers too
//! (`LutRun`, the CLUT through `clut3`, `Clut::eval`'s trilinear rule), both runs read ONE 4097-entry decode table shape
//! at 8 bits (x*257) and 16 bits (`Transform::apply_rgba16`), and the f32 tables are built only by `apply_rgba_f32`.

use alloc::vec::Vec;

use crate::profile::{Clut, Curve, Lut, LutKind, Model, Pcs, Profile, Refusal, Space};

/// The PCS illuminant D50 (ICC.1 §7.2.16, s15Fixed16-rounded) and D65 (sRGB's white).
pub const D50: [f32; 3] = [0.9642, 1.0, 0.8249];
pub const D65: [f32; 3] = [0.95047, 1.0, 1.08883];

/// XYZ (D65) to linear sRGB (IEC 61966-2-1).
const XYZ_TO_SRGB: [[f32; 3]; 3] = [[3.2404542, -1.5371385, -0.4985314], [-0.9692660, 1.8760108, 0.0415560], [0.0556434, -0.2040259, 1.0572252]];

/// The Bradford cone response matrix.
const BRADFORD: [[f32; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

type M3 = [[f32; 3]; 3];

pub(crate) fn mul(a: &M3, b: &M3) -> M3 {
    core::array::from_fn(|i| core::array::from_fn(|j| a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j]))
}

pub(crate) fn apply(m: &M3, v: [f32; 3]) -> [f32; 3] {
    core::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

pub(crate) fn inverse(m: &M3) -> M3 {
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let det = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    let adj = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    adj.map(|r| r.map(|v| v / det))
}

/// The Bradford chromatic adaptation from white `src` to white `dst` (both XYZ, Y = 1).
pub fn bradford(src: [f32; 3], dst: [f32; 3]) -> [[f32; 3]; 3] {
    let (s, d) = (apply(&BRADFORD, src), apply(&BRADFORD, dst));
    let scale = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    mul(&inverse(&BRADFORD), &mul(&scale, &BRADFORD))
}

/// PCS XYZ D50 to linear sRGB: `inv(M_sRGB_D65) . bradford(D50, D65)`.
pub fn pcs_to_linear_srgb() -> [[f32; 3]; 3] {
    mul(&XYZ_TO_SRGB, &bradford(D50, D65))
}

/// The sRGB OETF (linear 0..=1 to encoded 0..=1).
pub fn oetf(c: f32) -> f32 {
    let c = if c.is_nan() { 0.0 } else { c.clamp(0.0, 1.0) };
    if c <= 0.0031308 { 12.92 * c } else { 1.055 * libm::powf(c, 1.0 / 2.4) - 0.055 }
}

/// Lab D50 to XYZ D50.
fn lab_to_xyz(l: f32, a: f32, b: f32) -> [f32; 3] {
    let fy = (l + 16.0) / 116.0;
    let (fx, fz) = (fy + a / 500.0, fy - b / 200.0);
    let inv = |t: f32| if t > 6.0 / 29.0 { t * t * t } else { 3.0 * (6.0 / 29.0) * (6.0 / 29.0) * (t - 4.0 / 29.0) };
    [D50[0] * inv(fx), D50[1] * inv(fy), D50[2] * inv(fz)]
}

const OETF_STEPS: usize = 4096;

#[derive(Clone, Debug)]
enum Stage {
    /// RGB matrix/TRC: the curves, the colorants folded into the output matrix, and the integer run (SR380: the f32
    /// 8-bit tables are `apply_rgba_f32`'s own, built per call).
    Matrix { trc: [Curve; 3], m: M3, q: Q14Run },
    /// GRAY: the curve, whether its output is L* (Lab PCS), and the 8-bit result per grey level.
    Gray { trc: Curve, lab: bool, rgb8: Vec<[u8; 3]> },
    /// An A2B pipeline with its PCS, and its integer run for an RGB source (KERNEL-ICC-INT-LUT, SR380).
    Lut { lut: Lut, pcs: Pcs, run: Option<LutRun> },
}

/// A source profile to sRGB, ready to run.
#[derive(Clone, Debug)]
pub struct Transform {
    space: Space,
    out: M3,
    stage: Stage,
    /// The f32 curve / OETF evaluations the build ran (KERNEL-ICC-BUILD's witness, `build_evals`).
    evals: u32,
}

impl Transform {
    /// The transform from `p` to sRGB.
    pub fn to_srgb(p: &Profile) -> Result<Transform, Refusal> {
        let out = pcs_to_linear_srgb();
        let stage = match &p.model {
            Model::MatrixTrc { m, trc } => {
                let fm = mul(&out, m);
                Stage::Matrix { trc: trc.clone(), m: fm, q: Q14Run::new(trc, &fm) }
            }
            Model::GrayTrc(trc) => Stage::Gray { trc: trc.clone(), lab: p.pcs == Pcs::Lab, rgb8: Vec::new() },
            Model::Lut(lut) => {
                let rgb = p.space == Space::Rgb && lut.clut.as_ref().is_none_or(|c| c.grid.len() == 3 && c.outs == 3);
                Stage::Lut { lut: lut.clone(), pcs: p.pcs, run: if rgb { Some(LutRun::new(lut, p.pcs, &out)) } else { None } }
            }
        };
        let evals = match &stage {
            Stage::Matrix { q, .. } => q.evals(),
            Stage::Gray { .. } => 256 * 4,
            Stage::Lut { run, .. } => run.as_ref().map_or(0, |r| r.evals()),
        };
        let mut t = Transform { space: p.space, out, stage, evals };
        if let Stage::Gray { .. } = t.stage {
            let table: Vec<[u8; 3]> = (0..256).map(|v| t.eval(&[v as f32 / 255.0]).map(|c| (c * 255.0 + 0.5) as u8)).collect();
            if let Stage::Gray { rgb8, .. } = &mut t.stage {
                *rgb8 = table;
            }
        }
        Ok(t)
    }

    /// Parse `icc` and build its transform.
    pub fn from_icc(icc: &[u8]) -> Result<Transform, Refusal> {
        Transform::to_srgb(&Profile::parse(icc)?)
    }

    /// The rule [`crate::apply_icc_to_srgb`] runs, for a caller converting several RGBA buffers with one profile (an
    /// animation's frames): `Ok(None)` when the profile is sRGB (leave the pixels), the transform for a GRAY or RGB
    /// profile, and a CMYK profile refused by name (the decoder already turned CMYK samples into RGB).
    pub fn for_rgba(icc: &[u8]) -> Result<Option<Transform>, Refusal> {
        let p = Profile::parse(icc)?;
        if p.is_srgb() {
            return Ok(None);
        }
        if p.space == Space::Cmyk {
            return Err(Refusal::Unsupported("CMYK profile over RGB samples"));
        }
        Transform::to_srgb(&p).map(Some)
    }

    /// The source's channels per sample (1 GRAY, 3 RGB, 4 CMYK).
    pub fn channels(&self) -> usize {
        self.space.channels()
    }

    /// The source's data colour space.
    pub fn space(&self) -> Space {
        self.space
    }

    /// XYZ D50 of one sample (each 0..=1).
    fn to_pcs(&self, input: &[f32]) -> [f32; 3] {
        let x = |i: usize| input.get(i).copied().unwrap_or(0.0);
        match &self.stage {
            Stage::Matrix { .. } => [0.0; 3],
            Stage::Gray { trc, lab, .. } => {
                let y = trc.eval(x(0));
                if *lab { lab_to_xyz(y * 100.0, 0.0, 0.0) } else { [D50[0] * y, D50[1] * y, D50[2] * y] }
            }
            Stage::Lut { lut, pcs, .. } => {
                let v = lut.eval(input);
                match pcs {
                    Pcs::Xyz => v.map(|c| c * 65535.0 / 32768.0),
                    Pcs::Lab => {
                        let (l, a, b) = match lut.kind {
                            LutKind::Lut16 => (v[0] * 65535.0 / 65280.0 * 100.0, v[1] * 65535.0 / 256.0 - 128.0, v[2] * 65535.0 / 256.0 - 128.0),
                            _ => (v[0] * 100.0, v[1] * 255.0 - 128.0, v[2] * 255.0 - 128.0),
                        };
                        lab_to_xyz(l, a, b)
                    }
                }
            }
        }
    }

    /// Encoded sRGB (each 0..=1) of one sample (each 0..=1, `channels()` of them).
    pub fn eval(&self, input: &[f32]) -> [f32; 3] {
        let lin = match &self.stage {
            Stage::Matrix { trc, m, .. } => {
                let x = |i: usize| input.get(i).copied().unwrap_or(0.0);
                apply(m, [trc[0].eval(x(0)), trc[1].eval(x(1)), trc[2].eval(x(2))])
            }
            _ => apply(&self.out, self.to_pcs(input)),
        };
        lin.map(oetf)
    }

    /// Convert straight RGBA in place (alpha untouched). A GRAY source reads the red channel; a CMYK source cannot
    /// run over RGBA and leaves the pixels as they are (`apply_icc_to_srgb` refuses it by name first). A matrix/TRC
    /// source runs in integers (KERNEL-ICC-INT, SR376: within 1 of [`Transform::apply_rgba_f32`]), and so does an RGB
    /// LUT source (KERNEL-ICC-INT-LUT, SR380); a GRAY source reads its 8-bit table as `apply_rgba_f32` does.
    pub fn apply_rgba(&self, rgba: &mut [u8]) {
        match &self.stage {
            Stage::Matrix { q, .. } => run8(rgba, &q.enc, |x| q.lin(x)),
            Stage::Lut { lut, run: Some(r), .. } => run8(rgba, &r.enc, |x| r.lin(lut.clut.as_ref(), x)),
            _ => self.apply_rgba_f32(rgba),
        }
    }

    /// The f32 run (SR358's): the reference the integer runs are held within 1 of. Its f32 tables (the 8-bit
    /// linearisation, the 4097-step OETF) are built here, per call, never by `to_srgb` (KERNEL-ICC-BUILD, SR380).
    pub fn apply_rgba_f32(&self, rgba: &mut [u8]) {
        match &self.stage {
            Stage::Matrix { trc, m, .. } => {
                let enc = oetf_table();
                let lin: [Vec<f32>; 3] = core::array::from_fn(|c| (0..256).map(|v| trc[c].eval(v as f32 / 255.0)).collect());
                for px in rgba.chunks_exact_mut(4) {
                    let l = [lin[0][px[0] as usize], lin[1][px[1] as usize], lin[2][px[2] as usize]];
                    let o = apply(m, l);
                    px[0] = enc8(&enc, o[0]);
                    px[1] = enc8(&enc, o[1]);
                    px[2] = enc8(&enc, o[2]);
                }
            }
            Stage::Gray { rgb8, .. } => {
                for px in rgba.chunks_exact_mut(4) {
                    let o = rgb8[px[0] as usize];
                    px[..3].copy_from_slice(&o);
                }
            }
            Stage::Lut { .. } => {
                if self.space != Space::Rgb {
                    return;
                }
                let enc = oetf_table();
                let mut memo: Option<([u8; 3], [u8; 3])> = None;
                for px in rgba.chunks_exact_mut(4) {
                    let key = [px[0], px[1], px[2]];
                    let o = match memo {
                        Some((k, o)) if k == key => o,
                        _ => {
                            let lin = apply(&self.out, self.to_pcs(&key.map(|c| c as f32 / 255.0)));
                            let o = lin.map(|c| enc8(&enc, c));
                            memo = Some((key, o));
                            o
                        }
                    };
                    px[..3].copy_from_slice(&o);
                }
            }
        }
    }
}

// ── KERNEL-ICC-INT (LEDGER SR376): the integer run, ONE Q14 arithmetic shared with `display::DisplayTransform` ──
//
// A matrix/TRC source in DisplayTransform's shape (SR372): per-channel u16 decode tables (the source TRC x 65535,
// evaluated once at build: any monotone `Curve`; SR380: 4097 entries per distinct curve, read at 8 and 16 bits), the folded 3x3 (colorants . Bradford D50 to D65 . XYZ to linear
// sRGB) in Q14 accumulated in i64 and clamped, and sRGB's inverse TRC through the 65536-entry `encode_table` both sides
// build with. No float per pixel (the kernel is soft-float).

/// The integer run of a matrix/TRC source to sRGB.
#[derive(Clone, Debug)]
pub(crate) struct Q14Run {
    /// The source TRC as 4097-entry decode tables (linear x 65535), one per DISTINCT curve (SR380, `dec_pool`), and the
    /// table each channel reads.
    dec: Vec<Vec<u16>>,
    pick: [u8; 3],
    /// The folded source-to-linear-sRGB matrix in Q14.
    m: [[i32; 3]; 3],
    /// sRGB's inverse TRC over a 16-bit linear input, to 8 and to 16 bits.
    enc: Encode,
}

impl Q14Run {
    fn new(trc: &[Curve; 3], m: &M3) -> Q14Run {
        let (dec, p) = dec_pool(&[&trc[0], &trc[1], &trc[2]]);
        Q14Run { dec, pick: [p[0], p[1], p[2]], m: q14_matrix(m), enc: Encode::srgb() }
    }

    /// The f32 evaluations the build ran.
    fn evals(&self) -> u32 {
        (self.dec.len() * DEC_LEN + 255) as u32
    }

    /// One 16-bit source triple to a 16-bit linear sRGB index per channel (both depths' one body).
    #[inline]
    fn lin(&self, x: [u16; 3]) -> [usize; 3] {
        let l: [i64; 3] = core::array::from_fn(|c| dec16(&self.dec[self.pick[c] as usize], x[c]) as i64);
        core::array::from_fn(|i| q14_dot(&self.m[i], &l))
    }
}

/// A 3x3 in Q14 (rounded).
pub(crate) fn q14_matrix(m: &M3) -> [[i32; 3]; 3] {
    m.map(|r| r.map(|v| libm::roundf(v * 16384.0) as i32))
}

/// One Q14 row times a 16-bit linear triple, rounded and clamped to a 16-bit linear index.
#[inline]
pub(crate) fn q14_dot(r: &[i32; 3], l: &[i64; 3]) -> usize {
    q14_affine(r, l, 0)
}

/// [`q14_dot`] with a Q14 offset `c` added before the rounding and the clamp (an mAB matrix's offsets, SR380).
#[inline]
pub(crate) fn q14_affine(r: &[i32; 3], l: &[i64; 3], c: i64) -> usize {
    let v = (r[0] as i64 * l[0] + r[1] as i64 * l[1] + r[2] as i64 * l[2] + c + 8192) >> 14;
    v.clamp(0, 65535) as usize
}

/// The inverse of `trc` to 8 bits over a 16-bit linear input: entry `v` is the code nearest (in code space) to
/// linear `v / 65535` (moved from display.rs, SR372; the ONE builder both runs use; SR380: read from
/// [`encode_thresholds`], byte-identical).
pub(crate) fn encode_table(trc: &Curve) -> Vec<u8> {
    code_table(&encode_thresholds(trc))
}

// ── KERNEL-ICC-INT-LUT + KERNEL-ICC-BUILD + KERNEL-PNG16-ICC (LEDGER SR380): the LUT run, one table shape, 16 bits ──
//
// Both integer runs read 4097-entry u16 decode tables (entry i = the curve at min(16i, 65535) / 65535; a 16-bit input
// indexes by its top 12 bits and interpolates its low 4; an 8-bit input enters as x*257), one table per DISTINCT curve
// (display.rs's `pick` rule). An RGB lut8 / lut16 / mAB source runs its A2B pipeline in integers (`LutRun`): the CLUT
// through `clut3`, the trilinear rule `Clut::eval` uses (an integer tetrahedral would sit up to 9 codes from the f32
// reference on a 9-point grid, measured), the mAB matrix through `q14_affine`, a Lab PCS through f⁻¹ = t³ in Q16, and
// the PCS-to-sRGB 3x3 through `q14_dot`. Out through ONE `Encode`: `encode_table` to 8 bits, or the 16-bit inverse TRC
// interpolated between its half-code thresholds (no extra powf). The f32 run is the reference (`apply_rgba_f32`).

/// The f32 OETF table `apply_rgba_f32` interpolates (4097 steps; built per call, KERNEL-ICC-BUILD).
fn oetf_table() -> Vec<f32> {
    (0..=OETF_STEPS).map(|i| oetf(i as f32 / OETF_STEPS as f32)).collect()
}

/// The OETF through `enc` (an [`oetf_table`]), to 8 bits.
fn enc8(enc: &[f32], c: f32) -> u8 {
    let c = if c.is_nan() { 0.0 } else { c.clamp(0.0, 1.0) };
    let p = c * OETF_STEPS as f32;
    let i = (p as usize).min(OETF_STEPS - 1);
    let f = p - i as f32;
    let v = enc[i] + (enc[i + 1] - enc[i]) * f;
    (v * 255.0 + 0.5) as u8
}

/// The 16-bit linear thresholds of `trc`'s inverse: entry k is the first `v` whose code is above k (the running max of
/// `trc((k + 0.5) / 255) * 65535`, rounded up — for an integer `v`, `v >= ceil(t)` is `v as f32 >= t`).
pub(crate) fn encode_thresholds(trc: &Curve) -> [u32; 255] {
    let mut th = [0u32; 255];
    let mut run = 0f32;
    for (k, t) in th.iter_mut().enumerate() {
        run = run.max(trc.eval((k as f32 + 0.5) / 255.0) * 65535.0);
        *t = libm::ceilf(run) as u32;
    }
    th
}

/// The 65536-entry code table of `thr`.
fn code_table(thr: &[u32; 255]) -> Vec<u8> {
    let mut out = Vec::with_capacity(65536);
    let mut k = 0usize;
    for v in 0..65536u32 {
        while k < 255 && v >= thr[k] {
            k += 1;
        }
        out.push(k as u8);
    }
    out
}

/// sRGB's inverse TRC over a 16-bit linear input: [`encode_table`] for 8 bits, and its thresholds for 16.
#[derive(Clone, Debug)]
pub(crate) struct Encode {
    enc: Vec<u8>,
    thr: [u32; 255],
}

impl Encode {
    fn srgb() -> Encode {
        let thr = encode_thresholds(&crate::profile::SRGB_TRC);
        Encode { enc: code_table(&thr), thr }
    }

    /// The 8-bit code of linear `v`.
    #[inline]
    fn code8(&self, v: usize) -> u8 {
        self.enc[v]
    }

    /// The 16-bit code of linear `v`: linear between the half-code thresholds around its 8-bit code k (`thr[k-1]` is
    /// code k - 0.5, `thr[k]` is k + 0.5; the ends 0 and 65536 are codes 0 and 255), times 257. It rounds to `code8(v)`.
    #[inline]
    fn code16(&self, v: usize) -> u16 {
        let k = self.enc[v] as i64;
        let v = v as i64;
        let (lo, lo2) = if k == 0 { (0, 0) } else { (self.thr[k as usize - 1] as i64, 2 * k - 1) };
        let (hi, hi2) = if k == 255 { (65536, 510) } else { (self.thr[k as usize] as i64, 2 * k + 1) };
        let span = (hi - lo).max(1);
        let c2 = lo2 * span + (hi2 - lo2) * (v - lo);
        ((c2 * 257 + span) / (2 * span)).clamp(0, 65535) as u16
    }
}

/// Entries per decode table: 4096 steps of 16 over the 16-bit input, plus the end.
pub(crate) const DEC_LEN: usize = 4097;

/// One decode table: the curve at `min(16 i, 65535) / 65535`, x 65535.
fn dec_table(c: &Curve) -> Vec<u16> {
    (0..DEC_LEN).map(|i| (c.eval((i * 16).min(65535) as f32 / 65535.0) * 65535.0 + 0.5) as u16).collect()
}

/// One decode table per distinct curve of `curves`, and the table each curve reads.
pub(crate) fn dec_pool(curves: &[&Curve]) -> (Vec<Vec<u16>>, Vec<u8>) {
    let mut pool: Vec<Vec<u16>> = Vec::new();
    let mut pick: Vec<u8> = Vec::with_capacity(curves.len());
    for c in 0..curves.len() {
        match (0..c).find(|&j| curves[j] == curves[c]) {
            Some(j) => pick.push(pick[j]),
            None => {
                pick.push(pool.len() as u8);
                pool.push(dec_table(curves[c]));
            }
        }
    }
    (pool, pick)
}

/// The ONE decode lookup: a 16-bit input through a [`DEC_LEN`] table (top 12 bits the entry, low 4 the interpolation).
#[inline]
pub(crate) fn dec16(t: &[u16], x: u16) -> u16 {
    let (i, f) = ((x >> 4) as usize, (x & 15) as i32);
    let (a, b) = (t[i] as i32, t[i + 1] as i32);
    (a + (((b - a) * f + 8) >> 4)) as u16
}

/// The ONE integer CLUT interpolation: a 3-input, 3-output `c` at 16-bit `x`, trilinear as [`Clut::eval`] (per-input
/// grids, the first input slowest), the fractions in Q16, as seven successive lerps in i64.
pub(crate) fn clut3(c: &Clut, x: [u16; 3]) -> [u16; 3] {
    let g = [c.grid[0] as i64, c.grid[1] as i64, c.grid[2] as i64];
    let s2 = c.outs as i64;
    let s1 = s2 * g[2];
    let s0 = s1 * g[1];
    let mut lo = [0i64; 3];
    let mut fr = [0i64; 3];
    for d in 0..3 {
        let pos = x[d] as i64 * (g[d] - 1);
        let i = (pos / 65535).min(g[d] - 2);
        lo[d] = i;
        fr[d] = (pos - i * 65535) * 65536 / 65535;
    }
    let at = |i0: i64, i1: i64, i2: i64, k: usize| c.data[(i0 * s0 + i1 * s1 + i2 * s2) as usize + k] as i64;
    let lerp = |a: i64, b: i64, f: i64| a + (((b - a) * f + 32768) >> 16);
    core::array::from_fn(|k| {
        let (a, b, z) = (lo[0], lo[1], lo[2]);
        let e = |i0: i64, i1: i64| lerp(at(i0, i1, z, k), at(i0, i1, z + 1, k), fr[2]);
        let f0 = lerp(e(a, b), e(a, b + 1), fr[1]);
        let f1 = lerp(e(a + 1, b), e(a + 1, b + 1), fr[1]);
        lerp(f0, f1, fr[0]).clamp(0, 65535) as u16
    })
}

/// A Lab PCS triple (16-bit, in the tag's encoding: lut16's legacy one when `legacy`) to f⁻¹ of fx, fy, fz in Q16
/// (CIE: t³ above 6/29, the linear segment below), so X, Y, Z = D50 x these.
fn lab_finv_q16(v: [i64; 3], legacy: bool) -> [i64; 3] {
    const Q: i64 = 65536;
    let (l, a, b) = if legacy {
        (v[0] * 100 * Q / 65280, v[1] * 256 - 128 * Q, v[2] * 256 - 128 * Q)
    } else {
        (v[0] * 100 * Q / 65535, v[1] * 255 * Q / 65535 - 128 * Q, v[2] * 255 * Q / 65535 - 128 * Q)
    };
    let fy = (l + 16 * Q) / 116;
    let (fx, fz) = (fy + a / 500, fy - b / 200);
    let inv = |t: i64| if t * 29 > 6 * Q { t * t / Q * t / Q } else { 108 * (29 * t - 4 * Q) / (841 * 29) };
    [inv(fx), inv(fy), inv(fz)]
}

/// The integer run of an RGB A2B pipeline to sRGB (KERNEL-ICC-INT-LUT): the a / m / b curves as decode tables, the CLUT
/// borrowed from the stage's `Lut`, the mAB matrix in Q14 with its offsets, the PCS 3x3 in Q14.
#[derive(Clone, Debug)]
pub(crate) struct LutRun {
    dec: Vec<Vec<u16>>,
    /// The table each curve reads: a 0..3, m 3..6, b 6..9 (a missing curve is the identity).
    pick: [u8; 9],
    /// The mAB matrix in Q14 and its offsets (x 65535, in Q14).
    mat: Option<([[i32; 3]; 3], [i64; 3])>,
    /// PCS to linear sRGB in Q14: XYZ's 65535/32768 folded in, or Lab's D50 (over f⁻¹ in Q16).
    pcs: [[i32; 3]; 3],
    /// None for an XYZ PCS; Some(legacy) for Lab (lut16's legacy encoding).
    lab: Option<bool>,
    enc: Encode,
}

impl LutRun {
    fn new(lut: &Lut, pcs: Pcs, out: &M3) -> LutRun {
        let id = Curve::Identity;
        let curves: [&Curve; 9] = core::array::from_fn(|k| {
            let v = match k / 3 {
                0 => &lut.a,
                1 => &lut.m,
                _ => &lut.b,
            };
            v.get(k % 3).unwrap_or(&id)
        });
        let (dec, p) = dec_pool(&curves);
        let mat = lut.matrix.map(|m| {
            let rows = [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]];
            (q14_matrix(&rows), core::array::from_fn(|r| libm::round(m[9 + r] as f64 * 65535.0 * 16384.0) as i64))
        });
        let (rows, lab): (M3, Option<bool>) = match pcs {
            Pcs::Xyz => (out.map(|r| r.map(|v| v * (65535.0 / 32768.0))), None),
            Pcs::Lab => (core::array::from_fn(|i| core::array::from_fn(|j| out[i][j] * D50[j] * (65535.0 / 65536.0))), Some(lut.kind == LutKind::Lut16)),
        };
        LutRun { dec, pick: core::array::from_fn(|k| p[k]), mat, pcs: q14_matrix(&rows), lab, enc: Encode::srgb() }
    }

    /// The f32 evaluations the build ran.
    fn evals(&self) -> u32 {
        (self.dec.len() * DEC_LEN + 255) as u32
    }

    /// One 16-bit RGB triple to a 16-bit linear sRGB index per channel (both depths' one body).
    #[inline]
    fn lin(&self, clut: Option<&Clut>, x: [u16; 3]) -> [usize; 3] {
        let d = |k: usize, v: u16| dec16(&self.dec[self.pick[k] as usize], v);
        let a = [d(0, x[0]), d(1, x[1]), d(2, x[2])];
        let p = match clut {
            Some(c) => clut3(c, a),
            None => a,
        };
        let mut p = [d(3, p[0]), d(4, p[1]), d(5, p[2])];
        if let Some((m, off)) = &self.mat {
            let l = p.map(|v| v as i64);
            p = core::array::from_fn(|r| q14_affine(&m[r], &l, off[r]) as u16);
        }
        let p = [d(6, p[0]), d(7, p[1]), d(8, p[2])].map(|v| v as i64);
        let l = match self.lab {
            None => p,
            Some(legacy) => lab_finv_q16(p, legacy),
        };
        core::array::from_fn(|i| q14_dot(&self.pcs[i], &l))
    }
}

/// The ONE 8-bit pixel loop of the integer runs: straight RGBA in place (alpha untouched, a trailing partial pixel
/// passes), each sample widened to 16 bits (x*257), the last pixel memoised.
fn run8(rgba: &mut [u8], enc: &Encode, f: impl Fn([u16; 3]) -> [usize; 3]) {
    let mut memo: Option<([u8; 3], [u8; 3])> = None;
    for px in rgba.chunks_exact_mut(4) {
        let key = [px[0], px[1], px[2]];
        let o = match memo {
            Some((k, o)) if k == key => o,
            _ => {
                let o = f(key.map(|c| c as u16 * 257)).map(|v| enc.code8(v));
                memo = Some((key, o));
                o
            }
        };
        px[..3].copy_from_slice(&o);
    }
}

/// The ONE 16-bit pixel loop: [`run8`]'s shape over RGBA16.
fn run16(rgba: &mut [u16], enc: &Encode, f: impl Fn([u16; 3]) -> [usize; 3]) {
    let mut memo: Option<([u16; 3], [u16; 3])> = None;
    for px in rgba.chunks_exact_mut(4) {
        let key = [px[0], px[1], px[2]];
        let o = match memo {
            Some((k, o)) if k == key => o,
            _ => {
                let o = f(key).map(|v| enc.code16(v));
                memo = Some((key, o));
                o
            }
        };
        px[..3].copy_from_slice(&o);
    }
}

impl Transform {
    /// Convert straight RGBA16 in place (alpha untouched; KERNEL-PNG16-ICC, SR380): a matrix/TRC or RGB LUT source
    /// through the same integer run as [`Transform::apply_rgba`] on its full 16-bit samples, out at 16 bits; a GRAY
    /// source reads its 8-bit table at the red sample's top byte, widened (KERNEL-PNG16-GRAY owed); a non-RGB LUT source
    /// leaves the pixels.
    pub fn apply_rgba16(&self, rgba: &mut [u16]) {
        match &self.stage {
            Stage::Matrix { q, .. } => run16(rgba, &q.enc, |x| q.lin(x)),
            Stage::Lut { lut, run: Some(r), .. } => run16(rgba, &r.enc, |x| r.lin(lut.clut.as_ref(), x)),
            Stage::Gray { rgb8, .. } => {
                for px in rgba.chunks_exact_mut(4) {
                    let o = rgb8[(px[0] >> 8) as usize];
                    for i in 0..3 {
                        px[i] = o[i] as u16 * 257;
                    }
                }
            }
            Stage::Lut { run: None, .. } => {}
        }
    }

    /// The f32 curve / OETF evaluations [`Transform::to_srgb`] ran (KERNEL-ICC-BUILD, SR380): a decode table per
    /// distinct curve (4097 each) and the 255 encode thresholds for the integer runs, 256 x 4 for GRAY. SR376 built
    /// 5888 for a one-TRC matrix source (the f32 tables beside its integer run).
    pub fn build_evals(&self) -> u32 {
        self.evals
    }
}
