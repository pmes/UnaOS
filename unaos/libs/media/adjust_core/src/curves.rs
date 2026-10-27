// SPDX-License-Identifier: LGPL-3.0-or-later
//! Curves: a master table and optional per-channel tables, built from control points by piecewise-linear
//! interpolation.

use alloc::vec::Vec;

use crate::{Error, check, clamp_u8, div_round, map_rgb};

/// Curves. Per channel `out = master[chan[c]]` (channel first, then master); `None` = identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Curves {
    pub master: [u8; 256],
    pub r: Option<[u8; 256]>,
    pub g: Option<[u8; 256]>,
    pub b: Option<[u8; 256]>,
}

const IDENT: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = i as u8;
        i += 1;
    }
    t
};

impl Curves {
    /// The identity table.
    pub fn identity() -> [u8; 256] {
        IDENT
    }

    /// A table from control points: sorted by x (a repeated x keeps the last given), `(0,0)` / `(255,255)` added
    /// when no point has x = 0 / x = 255, linear between neighbours (rounded half away from zero).
    pub fn from_points(points: &[(u8, u8)]) -> [u8; 256] {
        // y per x, last given wins.
        let mut at: [Option<u8>; 256] = [None; 256];
        for &(x, y) in points {
            at[x as usize] = Some(y);
        }
        if at[0].is_none() {
            at[0] = Some(0);
        }
        if at[255].is_none() {
            at[255] = Some(255);
        }
        let mut lut = [0u8; 256];
        let mut x0 = 0usize;
        let mut y0 = at[0].unwrap_or(0) as i64;
        lut[0] = y0 as u8;
        for x1 in 1..256usize {
            let Some(y1) = at[x1] else { continue };
            let y1 = y1 as i64;
            let dx = (x1 - x0) as i64;
            for x in x0 + 1..=x1 {
                lut[x] = clamp_u8(y0 + div_round((y1 - y0) * (x - x0) as i64, dx));
            }
            x0 = x1;
            y0 = y1;
        }
        lut
    }
}

/// Curves on R, G, B (alpha copied).
pub fn curves(px: &[u8], w: u32, h: u32, c: &Curves) -> Result<Vec<u8>, Error> {
    check(px, w, h)?;
    let mut tabs = [[0u8; 256]; 3];
    for (tab, ch) in tabs.iter_mut().zip([&c.r, &c.g, &c.b]) {
        let chan = ch.as_ref().unwrap_or(&IDENT);
        for (v, slot) in tab.iter_mut().enumerate() {
            *slot = c.master[chan[v] as usize];
        }
    }
    Ok(map_rgb(px, [&tabs[0], &tabs[1], &tabs[2]]))
}
