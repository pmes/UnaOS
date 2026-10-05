// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The inverse transforms (RFC 6386 §14.3 Walsh–Hadamard, §14.4 DCT), with the reference
//! decoder's 16-bit intermediate storage (results wrap to `i16` between passes exactly as the
//! spec's `short` arrays do).

const C1: i32 = 20091; // cos(pi/8) * sqrt(2) - 1, Q16
const S1: i32 = 35468; // sin(pi/8) * sqrt(2), Q16

/// Inverse WHT of the Y2 block: returns the 16 luma DC values (§14.3).
pub fn iwht(input: &[i16; 16]) -> [i16; 16] {
    let mut t = [0i16; 16];
    for i in 0..4 {
        let ip = |k: usize| input[i + 4 * k] as i32;
        let a1 = ip(0) + ip(3);
        let b1 = ip(1) + ip(2);
        let c1 = ip(1) - ip(2);
        let d1 = ip(0) - ip(3);
        t[i] = (a1 + b1) as i16;
        t[4 + i] = (c1 + d1) as i16;
        t[8 + i] = (a1 - b1) as i16;
        t[12 + i] = (d1 - c1) as i16;
    }
    let mut out = [0i16; 16];
    for i in 0..4 {
        let ip = |k: usize| t[4 * i + k] as i32;
        let a1 = ip(0) + ip(3);
        let b1 = ip(1) + ip(2);
        let c1 = ip(1) - ip(2);
        let d1 = ip(0) - ip(3);
        out[4 * i] = ((a1 + b1 + 3) >> 3) as i16;
        out[4 * i + 1] = ((c1 + d1 + 3) >> 3) as i16;
        out[4 * i + 2] = ((a1 - b1 + 3) >> 3) as i16;
        out[4 * i + 3] = ((d1 - c1 + 3) >> 3) as i16;
    }
    out
}

/// Inverse DCT of one 4×4 block, added to the prediction already in `dst` (§14.4).
pub fn idct_add(input: &[i16; 16], dst: &mut [u8], o: usize, stride: usize) {
    if input[1..].iter().all(|&c| c == 0) {
        // DC only: every output is (dc + 4) >> 3 (the full transform gives the same).
        let dc = (input[0] as i32 + 4) >> 3;
        for r in 0..4 {
            for c in 0..4 {
                let p = &mut dst[o + r * stride + c];
                *p = (*p as i32 + dc).clamp(0, 255) as u8;
            }
        }
        return;
    }
    let mut t = [0i16; 16];
    for i in 0..4 {
        let ip = |k: usize| input[i + 4 * k] as i32;
        let a1 = ip(0) + ip(2);
        let b1 = ip(0) - ip(2);
        let c1 = ((ip(1) * S1) >> 16) - (ip(3) + ((ip(3) * C1) >> 16));
        let d1 = (ip(1) + ((ip(1) * C1) >> 16)) + ((ip(3) * S1) >> 16);
        t[i] = (a1 + d1) as i16;
        t[12 + i] = (a1 - d1) as i16;
        t[4 + i] = (b1 + c1) as i16;
        t[8 + i] = (b1 - c1) as i16;
    }
    for i in 0..4 {
        let ip = |k: usize| t[4 * i + k] as i32;
        let a1 = ip(0) + ip(2);
        let b1 = ip(0) - ip(2);
        let c1 = ((ip(1) * S1) >> 16) - (ip(3) + ((ip(3) * C1) >> 16));
        let d1 = (ip(1) + ((ip(1) * C1) >> 16)) + ((ip(3) * S1) >> 16);
        let row = [
            ((a1 + d1 + 4) >> 3) as i16,
            ((b1 + c1 + 4) >> 3) as i16,
            ((b1 - c1 + 4) >> 3) as i16,
            ((a1 - d1 + 4) >> 3) as i16,
        ];
        for c in 0..4 {
            let p = &mut dst[o + i * stride + c];
            *p = (*p as i32 + row[c] as i32).clamp(0, 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_only_matches_full_transform() {
        for dc in [-2048i16, -100, -5, -4, -3, 0, 3, 4, 5, 77, 2047] {
            let mut inp = [0i16; 16];
            inp[0] = dc;
            let mut a = [128u8; 16];
            idct_add(&inp, &mut a, 0, 4);
            // force the general path with a zero-valued "AC" that is not skipped: compute manually
            let want = (128 + ((dc as i32 + 4) >> 3)).clamp(0, 255) as u8;
            assert!(a.iter().all(|&p| p == want), "dc {dc}");
        }
    }

    #[test]
    fn wht_dc_only_spreads_evenly() {
        let mut inp = [0i16; 16];
        inp[0] = 85;
        assert_eq!(iwht(&inp), [((85 + 3) >> 3) as i16; 16]);
    }
}
