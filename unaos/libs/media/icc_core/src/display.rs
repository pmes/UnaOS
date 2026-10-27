// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Facet (CODEX §2 Images), seam shared-core — the DISPLAY side of the ONE ICC engine: sRGB to the panel.
//! sRGB to a matrix/TRC display (KERNEL-DISPLAY-ICC, LEDGER SR372).
//!
//! Every pixel UnaOS composes is sRGB (Facet converts a tagged image at open, SR358 / SR367); the display step is the
//! LAST transform, from sRGB to the panel's own primaries and tone curve. The panel is described either by its EDID
//! (VESA E-EDID 1.4 §3.6-§3.7: the gamma byte 0x17, the sRGB-default bit of 0x18, the 10-bit chromaticities at
//! 0x19-0x22; [`edid_colour`]) or by a matrix/TRC ICC display profile ([`DisplayTransform::from_profile`]; a LUT-based
//! or GRAY display profile is refused by name).
//!
//! THE CONSTRUCTION (relative colorimetric, the ICC v4 way). Each side's RGB-to-XYZ matrix is built from its xy
//! primaries and white (white Y = 1) and adapted to the PCS white with [`crate::bradford`]: `M_D50 = bradford(W, D50) .
//! NPM` ([`colorants`]) — the panel's EDID white is its adopted white, adapted exactly as a profile maker's `chad`
//! does, so sRGB white lands on the panel's full white. sRGB's own side uses the SAME construction from sRGB's xy, so
//! an exact-sRGB panel gives the identity. `lin_panel = inv(M_panel_D50) . M_sRGB_D50 . eotf(sRGB)`, then the inverse
//! of the panel's TRC. A profile's `MatrixTrc.m` IS its `M_panel_D50`, so `from_profile` and `from_edid` of the same
//! primaries are one transform.
//!
//! THE RUN (integer, the kernel compositor's hot path has no float): the sRGB EOTF as a 256-entry u16 table (linear x
//! 65535), the 3x3 in Q14 accumulated in i64 and clamped, and the inverse TRC as ONE 65536-entry u8 table per distinct
//! curve, built from the 255 code-space thresholds `trc((k + 0.5) / 255)` — round-to-nearest in the panel's code space
//! for any monotone curve (`curv` gamma, `curv` table or `para`; a non-monotone table is held monotone by a running
//! max), so no per-type closed form is needed. Identity ([`DisplayTransform::is_identity`]): the EDID's sRGB-default
//! bit, or the matrix within 0.01 of I and every TRC within 1/255 of sRGB's at 17 points (`Profile::is_srgb`'s
//! tolerances) — an sRGB panel is a no-op.
use alloc::vec::Vec;

use crate::profile::{Curve, Model, Pcs, Profile, Refusal, SRGB_TRC, Space};
use crate::transform::{D50, apply, bradford, inverse, mul};

type M3 = [[f32; 3]; 3];

/// The EDID 1.x fixed header (the first eight bytes of a base block).
const EDID_HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// sRGB's primaries and white (IEC 61966-2-1): r, g, b, w as (x, y).
pub const SRGB_XY: [[f32; 2]; 4] = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06], [0.3127, 0.3290]];

/// A panel's colour characteristics as its EDID base block states them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdidColour {
    /// r, g, b, w as (x, y), each the 10-bit EDID value / 1024.
    pub chroma: [[f32; 2]; 4],
    /// The display transfer characteristic: (byte 0x17 + 100) / 100.
    pub gamma: f32,
    /// Byte 0x18 bit 2: sRGB is the panel's default colour space.
    pub srgb: bool,
}

/// Decode the colour bytes of an EDID base block (at least 128 bytes, the fixed header first; the checksum is the
/// block publisher's gate). A gamma byte of 0xFF (the curve lives in an extension block) is refused by name.
pub fn edid_colour(block: &[u8]) -> Result<EdidColour, Refusal> {
    if block.len() < 128 || block[0..8] != EDID_HEADER {
        return Err(Refusal::Malformed("EDID base block"));
    }
    let g = block[0x17];
    if g == 0xFF {
        return Err(Refusal::Unsupported("EDID gamma in an extension block"));
    }
    let lo = [block[0x19], block[0x1A]];
    // i: Rx Ry Gx Gy (low bits in 0x19, high to low) then Bx By Wx Wy (0x1A); the high eight bits at 0x1B + i.
    let v = |i: usize| {
        let l = (lo[i / 4] >> (6 - 2 * (i % 4))) & 3;
        (((block[0x1B + i] as u16) << 2) | l as u16) as f32 / 1024.0
    };
    Ok(EdidColour {
        chroma: [[v(0), v(1)], [v(2), v(3)], [v(4), v(5)], [v(6), v(7)]],
        gamma: (g as f32 + 100.0) / 100.0,
        srgb: block[0x18] & 0x04 != 0,
    })
}

fn finite(m: &M3) -> bool {
    m.iter().all(|r| r.iter().all(|v| v.is_finite()))
}

fn xy_to_xyz(c: [f32; 2]) -> Result<[f32; 3], Refusal> {
    if !(c[1] > 0.0) {
        return Err(Refusal::Unsupported("EDID chromaticity degenerate"));
    }
    Ok([c[0] / c[1], 1.0, (1.0 - c[0] - c[1]) / c[1]])
}

/// The D50-adapted RGB-to-XYZ matrix of primaries + white `chroma` (r, g, b, w as xy): `m[i][c]` is colorant c's XYZ
/// component i, the layout of `Model::MatrixTrc.m` (an ICC display profile's rXYZ / gXYZ / bXYZ).
pub fn colorants(chroma: &[[f32; 2]; 4]) -> Result<[[f32; 3]; 3], Refusal> {
    let p = [xy_to_xyz(chroma[0])?, xy_to_xyz(chroma[1])?, xy_to_xyz(chroma[2])?];
    let w = xy_to_xyz(chroma[3])?;
    let m: M3 = core::array::from_fn(|i| core::array::from_fn(|c| p[c][i]));
    let mi = inverse(&m);
    if !finite(&mi) {
        return Err(Refusal::Unsupported("EDID chromaticity degenerate"));
    }
    let s = apply(&mi, w);
    let npm: M3 = core::array::from_fn(|i| core::array::from_fn(|c| m[i][c] * s[c]));
    Ok(mul(&bradford(w, D50), &npm))
}

/// The inverse of `trc` to 8 bits over a 16-bit linear input: entry `v` is the panel code nearest (in code space) to
/// linear `v / 65535`.
fn encode_table(trc: &Curve) -> Vec<u8> {
    let mut th = [0f32; 255];
    let mut run = 0f32;
    for (k, t) in th.iter_mut().enumerate() {
        run = run.max(trc.eval((k as f32 + 0.5) / 255.0) * 65535.0);
        *t = run;
    }
    let mut out = Vec::with_capacity(65536);
    let mut k = 0usize;
    for v in 0..65536u32 {
        while k < 255 && v as f32 >= th[k] {
            k += 1;
        }
        out.push(k as u8);
    }
    out
}

/// sRGB to one panel, ready to run over 8-bit pixels.
#[derive(Clone, Debug)]
pub struct DisplayTransform {
    identity: bool,
    /// The sRGB EOTF, linear x 65535.
    dec: [u16; 256],
    /// `inv(M_panel_D50) . M_sRGB_D50` in Q14.
    m: [[i32; 3]; 3],
    /// One inverse-TRC table per distinct curve, and the table each channel reads.
    enc: Vec<Vec<u8>>,
    pick: [u8; 3],
}

impl DisplayTransform {
    /// The no-op (an sRGB panel).
    fn identity() -> DisplayTransform {
        DisplayTransform { identity: true, dec: [0; 256], m: [[0; 3]; 3], enc: Vec::new(), pick: [0; 3] }
    }

    fn build(panel: &M3, trc: [&Curve; 3]) -> Result<DisplayTransform, Refusal> {
        let pi = inverse(panel);
        if !finite(&pi) {
            return Err(Refusal::Unsupported("display matrix singular"));
        }
        let t = mul(&pi, &colorants(&SRGB_XY)?);
        let near_i = (0..3).all(|i| (0..3).all(|j| libm::fabsf(t[i][j] - if i == j { 1.0 } else { 0.0 }) <= 0.01));
        let srgb_trc = trc.iter().all(|c| {
            (0..=16).all(|k| libm::fabsf(c.eval(k as f32 / 16.0) - SRGB_TRC.eval(k as f32 / 16.0)) <= 1.0 / 255.0)
        });
        if near_i && srgb_trc {
            return Ok(DisplayTransform::identity());
        }
        let dec = core::array::from_fn(|k| (SRGB_TRC.eval(k as f32 / 255.0) * 65535.0 + 0.5) as u16);
        let m = t.map(|r| r.map(|v| libm::roundf(v * 16384.0) as i32));
        let mut enc: Vec<Vec<u8>> = Vec::new();
        let mut pick = [0u8; 3];
        for c in 0..3 {
            match (0..c).find(|&j| trc[j] == trc[c]) {
                Some(j) => pick[c] = pick[j],
                None => {
                    pick[c] = enc.len() as u8;
                    enc.push(encode_table(trc[c]));
                }
            }
        }
        Ok(DisplayTransform { identity: false, dec, m, enc, pick })
    }

    /// A panel of primaries + white `chroma` (r, g, b, w as xy) and a pure power `gamma` (> 0).
    pub fn from_edid(chroma: [[f32; 2]; 4], gamma: f32) -> Result<DisplayTransform, Refusal> {
        if !(gamma.is_finite() && gamma > 0.0) {
            return Err(Refusal::Malformed("display gamma"));
        }
        let trc = Curve::Gamma(gamma);
        DisplayTransform::build(&colorants(&chroma)?, [&trc, &trc, &trc])
    }

    /// The panel an EDID describes: the sRGB-default bit is the identity (EDID 1.4 then fixes the chromaticities to
    /// sRGB's), else [`DisplayTransform::from_edid`].
    pub fn from_edid_colour(e: &EdidColour) -> Result<DisplayTransform, Refusal> {
        if e.srgb {
            return Ok(DisplayTransform::identity());
        }
        DisplayTransform::from_edid(e.chroma, e.gamma)
    }

    /// The panel a matrix/TRC display profile describes (RGB over an XYZ PCS); LUT-based and GRAY profiles are refused
    /// by name (DISPLAY-ICC-LUT owed).
    pub fn from_profile(p: &Profile) -> Result<DisplayTransform, Refusal> {
        match &p.model {
            Model::MatrixTrc { m, trc } if p.space == Space::Rgb && p.pcs == Pcs::Xyz => DisplayTransform::build(m, [&trc[0], &trc[1], &trc[2]]),
            Model::Lut(_) => Err(Refusal::Unsupported("LUT-based display profile")),
            Model::GrayTrc(_) => Err(Refusal::Unsupported("GRAY display profile")),
            _ => Err(Refusal::Unsupported("display profile not RGB over XYZ")),
        }
    }

    /// Is this the no-op?
    pub fn is_identity(&self) -> bool {
        self.identity
    }

    #[inline]
    fn px(&self, s: [u8; 3]) -> [u8; 3] {
        let l = [self.dec[s[0] as usize] as i64, self.dec[s[1] as usize] as i64, self.dec[s[2] as usize] as i64];
        core::array::from_fn(|i| {
            let r = &self.m[i];
            let v = (r[0] as i64 * l[0] + r[1] as i64 * l[1] + r[2] as i64 * l[2] + 8192) >> 14;
            self.enc[self.pick[i] as usize][v.clamp(0, 65535) as usize]
        })
    }

    /// One sRGB colour on this panel.
    #[inline]
    pub fn rgb(&self, s: [u8; 3]) -> [u8; 3] {
        if self.identity { s } else { self.px(s) }
    }

    /// Convert straight RGBA in place (the fourth byte untouched; a trailing partial pixel passes).
    pub fn apply(&self, rgba: &mut [u8]) {
        if self.identity {
            return;
        }
        for p in rgba.chunks_exact_mut(4) {
            let o = self.px([p[0], p[1], p[2]]);
            p[..3].copy_from_slice(&o);
        }
    }

    /// Convert BGRA in place (a little-endian `0x00RRGGBB` word's bytes).
    pub fn apply_bgra(&self, bgra: &mut [u8]) {
        if self.identity {
            return;
        }
        for p in bgra.chunks_exact_mut(4) {
            let o = self.px([p[2], p[1], p[0]]);
            p[0] = o[2];
            p[1] = o[1];
            p[2] = o[0];
        }
    }
}

/// Test helpers: an EDID base block carrying given colour bytes.
pub mod kat {
    /// A 128-byte EDID 1.4 base block: the header, version 1.4, `gamma_byte` at 0x17, the sRGB-default bit of 0x18
    /// when `srgb`, the 10-bit chromaticities `chroma_q10` (r, g, b, w as (x, y) x 1024) at 0x19-0x22, the checksum.
    pub fn edid(chroma_q10: [[u16; 2]; 4], gamma_byte: u8, srgb: bool) -> [u8; 128] {
        let mut b = [0u8; 128];
        b[0..8].copy_from_slice(&super::EDID_HEADER);
        b[0x12] = 1;
        b[0x13] = 4;
        b[0x17] = gamma_byte;
        b[0x18] = 0x0A | if srgb { 0x04 } else { 0 };
        for i in 0..8 {
            let v = chroma_q10[i / 2][i % 2] & 0x3FF;
            b[0x1B + i] = (v >> 2) as u8;
            b[0x19 + i / 4] |= ((v & 3) as u8) << (6 - 2 * (i % 4));
        }
        let sum = b[..127].iter().fold(0u8, |a, x| a.wrapping_add(*x));
        b[127] = 0u8.wrapping_sub(sum);
        b
    }
}
