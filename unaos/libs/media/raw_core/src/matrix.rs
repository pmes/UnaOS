// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The colour matrix (RAWCOLOR, rmbp-ledger B724): camera RGB -> linear sRGB (D65 white), applied after the white
//! balance and before the gamma, in Q12 integer ([`Q`]).
//!
//! The file's or the table's matrix maps XYZ -> camera (DNG 1.6 §6.3 `ColorMatrix2`, tag 50722, nine SRATIONALs,
//! row-major, one row per camera channel; dcraw's `adobe_coeff` rows are the same matrix times 10000). The develop
//! wants the other way, so — dcraw's `cam_xyz_coeff`:
//!
//! 1. `cam_rgb = cam_xyz * xyz_rgb` (`xyz_rgb` = sRGB primaries, D65, IEC 61966-2-1 — dcraw's table);
//! 2. each row of `cam_rgb` is divided by its sum, so sRGB white lands on camera (1, 1, 1) — the white-balanced
//!    neutral;
//! 3. `rgb_cam = cam_rgb^-1` (dcraw's `pseudoinverse`, which for three colours is the inverse).
//!
//! Each row of `rgb_cam` therefore sums to one: a white-balanced grey stays grey. In Q12 the rounding is folded into
//! the diagonal so every row sums to EXACTLY [`Q`] — grey is grey to the bit.
//!
//! Sources, in this order: the file's `ColorMatrix2` (`matrix=dng`), else — for a Sony body — the built-in rows
//! ([`SONY`], dcraw 9.28's `adobe_coeff`, first prefix match wins as in dcraw: `ILCE-7M3` takes the `ILCE-7` row)
//! (`matrix=table`), else none (`matrix=none`: the camera's native RGB, as B711 rendered it).

/// One in Q12.
pub const Q: i32 = 4096;

/// Where the matrix came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixSource {
    /// The file's DNG `ColorMatrix2` (tag 50722).
    Dng,
    /// The built-in table ([`SONY`]).
    Table,
}

impl MatrixSource {
    /// The wire word: `dng`, `table` or `none`.
    pub fn name(s: Option<MatrixSource>) -> &'static str {
        match s {
            Some(MatrixSource::Dng) => "dng",
            Some(MatrixSource::Table) => "table",
            None => "none",
        }
    }
}

/// sRGB (D65) -> XYZ, dcraw's `xyz_rgb`.
pub const XYZ_RGB: [[f64; 3]; 3] = [[0.412453, 0.357580, 0.180423], [0.212671, 0.715160, 0.072169], [0.019334, 0.119193, 0.950227]];

/// dcraw 9.28 `adobe_coeff` rows for Sony bodies (the model string after "Sony "; XYZ -> camera times 10000, the
/// table's black/maximum columns are not carried). dcraw's order is kept: the first prefix that matches wins.
pub const SONY: &[(&str, [i16; 9])] = &[
    ("DSC-RX100M", [6596, -2079, -562, -4782, 13016, 1933, -970, 1581, 5181]),
    ("DSC-RX100", [8651, -2754, -1057, -3464, 12207, 1373, -568, 1398, 4434]),
    ("DSC-RX10", [6679, -1825, -745, -5047, 13256, 1953, -1580, 2422, 5183]),
    ("DSC-RX1RM2", [6629, -1900, -483, -4618, 12349, 2550, -622, 1381, 6514]),
    ("DSC-RX1", [6344, -1612, -462, -4863, 12477, 2681, -865, 1786, 6899]),
    ("ILCA-68", [6435, -1903, -536, -4722, 12449, 2550, -663, 1363, 6517]),
    ("ILCA-77M2", [5991, -1732, -443, -4100, 11989, 2381, -704, 1467, 5992]),
    ("ILCA-99M2", [6660, -1918, -471, -4613, 12398, 2485, -649, 1433, 6447]),
    ("ILCE-6300", [5973, -1695, -419, -3826, 11797, 2293, -639, 1398, 5789]),
    ("ILCE-6500", [5973, -1695, -419, -3826, 11797, 2293, -639, 1398, 5789]),
    ("ILCE-7M2", [5271, -712, -347, -6153, 13653, 2763, -1601, 2366, 7242]),
    ("ILCE-7S", [5838, -1430, -246, -3497, 11477, 2297, -748, 1885, 5778]),
    ("ILCE-7RM2", [6629, -1900, -483, -4618, 12349, 2550, -622, 1381, 6514]),
    ("ILCE-7R", [4913, -541, -202, -6130, 13513, 2906, -1564, 2151, 7183]),
    ("ILCE-7", [5271, -712, -347, -6153, 13653, 2763, -1601, 2366, 7242]),
    ("ILCE", [5991, -1456, -455, -4764, 12135, 2980, -707, 1425, 6701]),
    ("NEX-5N", [5991, -1456, -455, -4764, 12135, 2980, -707, 1425, 6701]),
    ("NEX-5R", [6129, -1545, -418, -4930, 12490, 2743, -977, 1693, 6615]),
    ("NEX-5T", [6129, -1545, -418, -4930, 12490, 2743, -977, 1693, 6615]),
    ("NEX-3N", [6129, -1545, -418, -4930, 12490, 2743, -977, 1693, 6615]),
    ("NEX-3", [6549, -1550, -436, -4880, 12435, 2753, -854, 1868, 6976]),
    ("NEX-5", [6549, -1550, -436, -4880, 12435, 2753, -854, 1868, 6976]),
    ("NEX-6", [6129, -1545, -418, -4930, 12490, 2743, -977, 1693, 6615]),
    ("NEX-7", [5491, -1192, -363, -4951, 12342, 2948, -911, 1722, 7192]),
    ("NEX", [5991, -1456, -455, -4764, 12135, 2980, -707, 1425, 6701]),
];

/// The built-in row for `make`/`model` (Sony only; the model may or may not repeat the make).
pub fn table(make: Option<&str>, model: Option<&str>) -> Option<[i16; 9]> {
    if !make.map(|m| m.trim().eq_ignore_ascii_case("SONY")).unwrap_or(false) {
        return None;
    }
    let m = model?.trim();
    let m = if m.len() >= 5 && m[..5].eq_ignore_ascii_case("SONY ") { m[5..].trim_start() } else { m };
    SONY.iter().find(|(p, _)| m.len() >= p.len() && m.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())).map(|r| r.1)
}

fn round(x: f64) -> i64 {
    if x >= 0.0 { (x + 0.5) as i64 } else { (x - 0.5) as i64 }
}

fn abs(x: f64) -> f64 {
    if x < 0.0 { -x } else { x }
}

/// The 3x3 inverse by cofactors; `None` when singular.
fn inverse(a: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let c = |r0: usize, r1: usize, c0: usize, c1: usize| a[r0][c0] * a[r1][c1] - a[r0][c1] * a[r1][c0];
    let det = a[0][0] * c(1, 2, 1, 2) - a[0][1] * c(1, 2, 0, 2) + a[0][2] * c(1, 2, 0, 1);
    if abs(det) < 1e-9 {
        return None;
    }
    let adj = [
        [c(1, 2, 1, 2), -c(0, 2, 1, 2), c(0, 1, 1, 2)],
        [-c(1, 2, 0, 2), c(0, 2, 0, 2), -c(0, 1, 0, 2)],
        [c(1, 2, 0, 1), -c(0, 2, 0, 1), c(0, 1, 0, 1)],
    ];
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = adj[i][j] / det;
        }
    }
    Some(o)
}

/// `rgb_cam` in Q12 from an XYZ -> camera matrix (row-major, one row per camera channel R, G, B). `None` when a row
/// of `cam_rgb` sums to zero, the matrix is singular, or an entry lies beyond +-16.
pub fn from_xyz_cam(cam_xyz: &[f64; 9]) -> Option<[[i32; 3]; 3]> {
    let mut cam_rgb = [[0.0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            cam_rgb[i][j] = (0..3).map(|k| cam_xyz[i * 3 + k] * XYZ_RGB[k][j]).sum();
        }
        let num: f64 = cam_rgb[i].iter().sum();
        if abs(num) < 1e-6 {
            return None;
        }
        for v in cam_rgb[i].iter_mut() {
            *v /= num;
        }
    }
    let inv = inverse(&cam_rgb)?;
    let mut q = [[0i32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let v = round(inv[i][j] * Q as f64);
            if !(-16 * Q as i64..=16 * Q as i64).contains(&v) {
                return None;
            }
            q[i][j] = v as i32;
        }
        // Every row sums to exactly Q: the rounding goes to the diagonal.
        let s: i32 = q[i].iter().sum();
        q[i][i] += Q - s;
    }
    Some(q)
}

/// A table row (times 10000) as the matrix.
pub fn from_table(row: &[i16; 9]) -> Option<[[i32; 3]; 3]> {
    let mut m = [0.0f64; 9];
    for (o, &v) in m.iter_mut().zip(row) {
        *o = v as f64 / 10000.0;
    }
    from_xyz_cam(&m)
}

/// A DNG `ColorMatrix2`'s nine (numerator, denominator) pairs as the matrix.
pub fn from_rationals(r: &[(i32, i32); 9]) -> Option<[[i32; 3]; 3]> {
    let mut m = [0.0f64; 9];
    for (o, &(n, d)) in m.iter_mut().zip(r) {
        if d == 0 {
            return None;
        }
        *o = n as f64 / d as f64;
    }
    from_xyz_cam(&m)
}

/// `rgb = m * cam` on Q16 linear values (0..=65535 is black..white), clamped to `0..=65535`.
#[inline]
pub fn apply(m: &[[i32; 3]; 3], cam: [u32; 3]) -> [u32; 3] {
    let mut o = [0u32; 3];
    for i in 0..3 {
        let s: i64 = (0..3).map(|j| m[i][j] as i64 * cam[j] as i64).sum();
        o[i] = (s >> 12).clamp(0, 65535) as u32;
    }
    o
}
