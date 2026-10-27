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

use alloc::vec::Vec;

use crate::profile::{Curve, Lut, LutKind, Model, Pcs, Profile, Refusal, Space};

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
    /// RGB matrix/TRC: the curves, their 8-bit tables, and colorants folded into the output matrix.
    Matrix { trc: [Curve; 3], lin: [Vec<f32>; 3], m: M3 },
    /// GRAY: the curve, whether its output is L* (Lab PCS), and the 8-bit result per grey level.
    Gray { trc: Curve, lab: bool, rgb8: Vec<[u8; 3]> },
    /// An A2B pipeline with its PCS.
    Lut { lut: Lut, pcs: Pcs },
}

/// A source profile to sRGB, ready to run.
#[derive(Clone, Debug)]
pub struct Transform {
    space: Space,
    out: M3,
    stage: Stage,
    enc: Vec<f32>,
}

impl Transform {
    /// The transform from `p` to sRGB.
    pub fn to_srgb(p: &Profile) -> Result<Transform, Refusal> {
        let out = pcs_to_linear_srgb();
        let enc: Vec<f32> = (0..=OETF_STEPS).map(|i| oetf(i as f32 / OETF_STEPS as f32)).collect();
        let stage = match &p.model {
            Model::MatrixTrc { m, trc } => {
                let lin = core::array::from_fn(|c| (0..256).map(|v| trc[c].eval(v as f32 / 255.0)).collect());
                Stage::Matrix { trc: trc.clone(), lin, m: mul(&out, m) }
            }
            Model::GrayTrc(trc) => Stage::Gray { trc: trc.clone(), lab: p.pcs == Pcs::Lab, rgb8: Vec::new() },
            Model::Lut(lut) => Stage::Lut { lut: lut.clone(), pcs: p.pcs },
        };
        let mut t = Transform { space: p.space, out, stage, enc };
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
            Stage::Lut { lut, pcs } => {
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

    /// The OETF through the cached table, to 8 bits.
    fn enc8(&self, c: f32) -> u8 {
        let c = if c.is_nan() { 0.0 } else { c.clamp(0.0, 1.0) };
        let p = c * OETF_STEPS as f32;
        let i = (p as usize).min(OETF_STEPS - 1);
        let f = p - i as f32;
        let v = self.enc[i] + (self.enc[i + 1] - self.enc[i]) * f;
        (v * 255.0 + 0.5) as u8
    }

    /// Convert straight RGBA in place (alpha untouched). A GRAY source reads the red channel; a CMYK source cannot
    /// run over RGBA and leaves the pixels as they are (`apply_icc_to_srgb` refuses it by name first).
    pub fn apply_rgba(&self, rgba: &mut [u8]) {
        match &self.stage {
            Stage::Matrix { lin, m, .. } => {
                for px in rgba.chunks_exact_mut(4) {
                    let l = [lin[0][px[0] as usize], lin[1][px[1] as usize], lin[2][px[2] as usize]];
                    let o = apply(m, l);
                    px[0] = self.enc8(o[0]);
                    px[1] = self.enc8(o[1]);
                    px[2] = self.enc8(o[2]);
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
                let mut memo: Option<([u8; 3], [u8; 3])> = None;
                for px in rgba.chunks_exact_mut(4) {
                    let key = [px[0], px[1], px[2]];
                    let o = match memo {
                        Some((k, o)) if k == key => o,
                        _ => {
                            let lin = apply(&self.out, self.to_pcs(&key.map(|c| c as f32 / 255.0)));
                            let o = lin.map(|c| self.enc8(c));
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
