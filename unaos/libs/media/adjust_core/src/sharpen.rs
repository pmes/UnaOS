// SPDX-License-Identifier: LGPL-3.0-or-later
//! Unsharp mask: original + amount x (original - gaussian), where |original - gaussian| >= threshold.

use alloc::vec::Vec;

use crate::blur::gaussian_blur;
use crate::{Error, check, clamp_u8, div_round};

/// Unsharp parameters. `amount_pct`: 100 = add the full difference; `sigma_q8`: 1.0 = 256.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unsharp {
    pub amount_pct: u16,
    pub sigma_q8: u16,
    pub threshold: u8,
}

/// Unsharp mask on R, G, B; alpha is the original's.
pub fn unsharp(px: &[u8], w: u32, h: u32, p: Unsharp) -> Result<Vec<u8>, Error> {
    check(px, w, h)?;
    if p.amount_pct == 0 {
        return Ok(px.to_vec());
    }
    let blurred = gaussian_blur(px, w, h, p.sigma_q8)?;
    let mut out = Vec::with_capacity(px.len());
    for (o, b) in px.chunks_exact(4).zip(blurred.chunks_exact(4)) {
        for c in 0..3 {
            let diff = o[c] as i64 - b[c] as i64;
            out.push(if diff.unsigned_abs() >= p.threshold as u64 {
                clamp_u8(o[c] as i64 + div_round(diff * p.amount_pct as i64, 100))
            } else {
                o[c]
            });
        }
        out.push(o[3]);
    }
    Ok(out)
}
