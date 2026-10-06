// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Linear light -> sRGB 8-bit (IEC 61966-2-1). The raw mosaic is linear; pixel_core's surface is sRGB-encoded.
//! The EOTF is lux's `color.rs` (`((s + 0.055) / 1.055)^2.4`, linear below 0.04045); the encoder inverts it by
//! THRESHOLDS — code `c` is the largest whose lower decision point `EOTF((c - 0.5) / 255)` the value reaches —
//! so the 8-bit result is exact without a `powf` in `no_std`: `x^2.4 = x^2 * (x^2)^(1/5)`, the fifth root by
//! Newton's method.

/// `a^(1/5)` for `a` in `[0, 1]` (Newton on `y^5 = a`).
fn fifth_root(a: f64) -> f64 {
    if a <= 0.0 {
        return 0.0;
    }
    let mut y = 1.0f64;
    for _ in 0..64 {
        let y4 = y * y * y * y;
        let n = (4.0 * y + a / y4) / 5.0;
        if n - y < 1e-15 && y - n < 1e-15 {
            return n;
        }
        y = n;
    }
    y
}

/// sRGB EOTF: encoded `s` in `[0, 1]` -> linear.
pub fn eotf(s: f64) -> f64 {
    if s <= 0.04045 {
        s / 12.92
    } else {
        let x = (s + 0.055) / 1.055;
        let x2 = x * x;
        x2 * fifth_root(x2)
    }
}

/// The 255 decision points: `t[c - 1]` = the linear value at which code `c` begins.
pub fn thresholds() -> [f64; 255] {
    let mut t = [0.0f64; 255];
    for (i, e) in t.iter_mut().enumerate() {
        *e = eotf((i as f64 + 0.5) / 255.0);
    }
    t
}

/// Encode one linear value in `[0, 1]` (clamped) to an sRGB byte.
pub fn encode(lin: f64, t: &[f64; 255]) -> u8 {
    t.partition_point(|&x| x <= lin) as u8
}
