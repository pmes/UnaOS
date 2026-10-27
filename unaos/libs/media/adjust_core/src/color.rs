// SPDX-License-Identifier: LGPL-3.0-or-later
//! Colour adjustments: hue / saturation / lightness, invert, threshold.

use alloc::vec::Vec;

use crate::{Error, check, clamp_u8, div_round};

/// The hue circle: six sectors of 256.
pub const HUE_CIRCLE: i64 = 1536;
const SECTOR: i64 = 256;

/// Hue / saturation / lightness. `hue_deg` -180..=180, `sat_pct` and `light_pct` -100..=100.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HueSat {
    pub hue_deg: i16,
    pub sat_pct: i16,
    pub light_pct: i16,
}

/// `(77 R + 150 G + 29 B) >> 8` — the weights sum to 256, so a grey `v` has luma `v`.
pub fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((77 * r as u32 + 150 * g as u32 + 29 * b as u32) >> 8) as u8
}

/// Rotate the hue by `shift` (in `HUE_CIRCLE` units), keeping max, min (so HSL's L and S).
fn rotate(rgb: [u8; 3], shift: i64) -> [u8; 3] {
    let [r, g, b] = rgb.map(|c| c as i64);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d == 0 {
        return rgb;
    }
    let h = if max == r {
        div_round(SECTOR * (g - b), d)
    } else if max == g {
        2 * SECTOR + div_round(SECTOR * (b - r), d)
    } else {
        4 * SECTOR + div_round(SECTOR * (r - g), d)
    };
    let h = (h + shift).rem_euclid(HUE_CIRCLE);
    let s = h / SECTOR;
    let f = h % SECTOR;
    let up = min + div_round(d * f, SECTOR);
    let down = max - div_round(d * f, SECTOR);
    let out = match s {
        0 => [max, up, min],
        1 => [down, max, min],
        2 => [min, max, up],
        3 => [min, down, max],
        4 => [up, min, max],
        _ => [max, min, down],
    };
    out.map(clamp_u8)
}

/// Hue (HSL rotation), then saturation (about luma), then lightness (toward white / black); alpha copied.
pub fn hue_sat(px: &[u8], w: u32, h: u32, p: HueSat) -> Result<Vec<u8>, Error> {
    check(px, w, h)?;
    if !(-180..=180).contains(&p.hue_deg) || !(-100..=100).contains(&p.sat_pct) || !(-100..=100).contains(&p.light_pct)
    {
        return Err(Error::BadParam);
    }
    let shift = div_round(p.hue_deg as i64 * HUE_CIRCLE, 360);
    let k = 100 + p.sat_pct as i64;
    let lp = p.light_pct as i64;
    let mut out = Vec::with_capacity(px.len());
    for q in px.chunks_exact(4) {
        let mut c = [q[0], q[1], q[2]];
        if p.hue_deg != 0 {
            c = rotate(c, shift);
        }
        if p.sat_pct != 0 {
            let y = luma(c[0], c[1], c[2]) as i64;
            c = c.map(|v| clamp_u8(y + div_round((v as i64 - y) * k, 100)));
        }
        if lp > 0 {
            c = c.map(|v| clamp_u8(v as i64 + div_round((255 - v as i64) * lp, 100)));
        } else if lp < 0 {
            c = c.map(|v| clamp_u8(div_round(v as i64 * (100 + lp), 100)));
        }
        out.extend_from_slice(&[c[0], c[1], c[2], q[3]]);
    }
    Ok(out)
}

/// `255 - c` on R, G, B; alpha copied.
pub fn invert(px: &[u8], w: u32, h: u32) -> Result<Vec<u8>, Error> {
    check(px, w, h)?;
    let mut out = Vec::with_capacity(px.len());
    for q in px.chunks_exact(4) {
        out.extend_from_slice(&[255 - q[0], 255 - q[1], 255 - q[2], q[3]]);
    }
    Ok(out)
}

/// `luma >= level` -> white, else black, on R, G, B; alpha copied.
pub fn threshold(px: &[u8], w: u32, h: u32, level: u8) -> Result<Vec<u8>, Error> {
    check(px, w, h)?;
    let mut out = Vec::with_capacity(px.len());
    for q in px.chunks_exact(4) {
        let v = if luma(q[0], q[1], q[2]) >= level { 255 } else { 0 };
        out.extend_from_slice(&[v, v, v, q[3]]);
    }
    Ok(out)
}
