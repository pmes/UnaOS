// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Inverse transforms and reconstruction (spec §8.7.1 / §8.7.2): the 4/8/16/32-point inverse DCT,
//! the 4/8/16-point inverse ADST, the 4x4 inverse Walsh-Hadamard (lossless), the 2-D row-then-column
//! process with the final Round2 by 4 / 5 / 6 / 6, and the clipped add into the prediction.
//!
//! The butterflies use the spec's 14-bit constants (cos64(k) = round(16384 cos(k pi / 64)), sinpi(k, 9))
//! and Round2(x, 14) after every multiply, in 64-bit intermediates so 10- and 12-bit streams cannot
//! overflow; the sequence of operations is the one the reference decoder (libvpx inv_txfm.c) uses,
//! which the spec's process reproduces bit for bit.

use crate::consts::*;

/// cospi_k_64 for k = 0..31 (cospi_0_64 = 16384).
const C: [i64; 32] = [
    16384, 16364, 16305, 16207, 16069, 15893, 15679, 15426, 15137, 14811, 14449, 14053, 13623, 13160, 12665, 12140, 11585, 11003,
    10394, 9760, 9102, 8423, 7723, 7005, 6270, 5520, 4756, 3981, 3196, 2404, 1606, 804,
];
const SINPI_1_9: i64 = 5283;
const SINPI_2_9: i64 = 9929;
const SINPI_3_9: i64 = 13377;
const SINPI_4_9: i64 = 15212;

#[inline(always)]
fn rs(x: i64) -> i64 {
    (x + (1 << 13)) >> 14
}

fn idct4(i: &[i64], o: &mut [i64]) {
    let s0 = rs((i[0] + i[2]) * C[16]);
    let s1 = rs((i[0] - i[2]) * C[16]);
    let s2 = rs(i[1] * C[24] - i[3] * C[8]);
    let s3 = rs(i[1] * C[8] + i[3] * C[24]);
    o[0] = s0 + s3;
    o[1] = s1 + s2;
    o[2] = s1 - s2;
    o[3] = s0 - s3;
}

fn iadst4(i: &[i64], o: &mut [i64]) {
    let (x0, x1, x2, x3) = (i[0], i[1], i[2], i[3]);
    if x0 | x1 | x2 | x3 == 0 {
        o[..4].fill(0);
        return;
    }
    let mut s0 = SINPI_1_9 * x0;
    let mut s1 = SINPI_2_9 * x0;
    let mut s2 = SINPI_3_9 * x1;
    let mut s3 = SINPI_4_9 * x2;
    let s4 = SINPI_1_9 * x2;
    let s5 = SINPI_2_9 * x3;
    let s6 = SINPI_4_9 * x3;
    let s7 = x0 - x2 + x3;
    s0 = s0 + s3 + s5;
    s1 = s1 - s4 - s6;
    s3 = s2;
    s2 = SINPI_3_9 * s7;
    o[0] = rs(s0 + s3);
    o[1] = rs(s1 + s3);
    o[2] = rs(s2);
    o[3] = rs(s0 + s1 - s3);
}

fn idct8(i: &[i64], o: &mut [i64]) {
    let mut s1 = [0i64; 8];
    let mut s2 = [0i64; 8];
    s1[0] = i[0];
    s1[2] = i[4];
    s1[1] = i[2];
    s1[3] = i[6];
    s1[4] = rs(i[1] * C[28] - i[7] * C[4]);
    s1[7] = rs(i[1] * C[4] + i[7] * C[28]);
    s1[5] = rs(i[5] * C[12] - i[3] * C[20]);
    s1[6] = rs(i[5] * C[20] + i[3] * C[12]);
    s2[0] = rs((s1[0] + s1[2]) * C[16]);
    s2[1] = rs((s1[0] - s1[2]) * C[16]);
    s2[2] = rs(s1[1] * C[24] - s1[3] * C[8]);
    s2[3] = rs(s1[1] * C[8] + s1[3] * C[24]);
    s2[4] = s1[4] + s1[5];
    s2[5] = s1[4] - s1[5];
    s2[6] = -s1[6] + s1[7];
    s2[7] = s1[6] + s1[7];
    s1[0] = s2[0] + s2[3];
    s1[1] = s2[1] + s2[2];
    s1[2] = s2[1] - s2[2];
    s1[3] = s2[0] - s2[3];
    s1[4] = s2[4];
    s1[5] = rs((s2[6] - s2[5]) * C[16]);
    s1[6] = rs((s2[5] + s2[6]) * C[16]);
    s1[7] = s2[7];
    for k in 0..4 {
        o[k] = s1[k] + s1[7 - k];
        o[7 - k] = s1[k] - s1[7 - k];
    }
}

fn iadst8(i: &[i64], o: &mut [i64]) {
    let mut x0 = i[7];
    let mut x1 = i[0];
    let mut x2 = i[5];
    let mut x3 = i[2];
    let mut x4 = i[3];
    let mut x5 = i[4];
    let mut x6 = i[1];
    let mut x7 = i[6];
    if x0 | x1 | x2 | x3 | x4 | x5 | x6 | x7 == 0 {
        o[..8].fill(0);
        return;
    }
    // stage 1
    let s0 = C[2] * x0 + C[30] * x1;
    let s1 = C[30] * x0 - C[2] * x1;
    let s2 = C[10] * x2 + C[22] * x3;
    let s3 = C[22] * x2 - C[10] * x3;
    let s4 = C[18] * x4 + C[14] * x5;
    let s5 = C[14] * x4 - C[18] * x5;
    let s6 = C[26] * x6 + C[6] * x7;
    let s7 = C[6] * x6 - C[26] * x7;
    x0 = rs(s0 + s4);
    x1 = rs(s1 + s5);
    x2 = rs(s2 + s6);
    x3 = rs(s3 + s7);
    x4 = rs(s0 - s4);
    x5 = rs(s1 - s5);
    x6 = rs(s2 - s6);
    x7 = rs(s3 - s7);
    // stage 2
    let (s0, s1, s2, s3) = (x0, x1, x2, x3);
    let s4 = C[8] * x4 + C[24] * x5;
    let s5 = C[24] * x4 - C[8] * x5;
    let s6 = -C[24] * x6 + C[8] * x7;
    let s7 = C[8] * x6 + C[24] * x7;
    x0 = s0 + s2;
    x1 = s1 + s3;
    x2 = s0 - s2;
    x3 = s1 - s3;
    x4 = rs(s4 + s6);
    x5 = rs(s5 + s7);
    x6 = rs(s4 - s6);
    x7 = rs(s5 - s7);
    // stage 3
    let s2 = C[16] * (x2 + x3);
    let s3 = C[16] * (x2 - x3);
    let s6 = C[16] * (x6 + x7);
    let s7 = C[16] * (x6 - x7);
    x2 = rs(s2);
    x3 = rs(s3);
    x6 = rs(s6);
    x7 = rs(s7);
    o[0] = x0;
    o[1] = -x4;
    o[2] = x6;
    o[3] = -x2;
    o[4] = x3;
    o[5] = -x7;
    o[6] = x5;
    o[7] = -x1;
}

fn idct16(i: &[i64], o: &mut [i64]) {
    let mut s1 = [0i64; 16];
    let mut s2 = [0i64; 16];
    const ORDER: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];
    for k in 0..16 {
        s1[k] = i[ORDER[k]];
    }
    // stage 2
    s2[..8].copy_from_slice(&s1[..8]);
    s2[8] = rs(s1[8] * C[30] - s1[15] * C[2]);
    s2[15] = rs(s1[8] * C[2] + s1[15] * C[30]);
    s2[9] = rs(s1[9] * C[14] - s1[14] * C[18]);
    s2[14] = rs(s1[9] * C[18] + s1[14] * C[14]);
    s2[10] = rs(s1[10] * C[22] - s1[13] * C[10]);
    s2[13] = rs(s1[10] * C[10] + s1[13] * C[22]);
    s2[11] = rs(s1[11] * C[6] - s1[12] * C[26]);
    s2[12] = rs(s1[11] * C[26] + s1[12] * C[6]);
    // stage 3
    s1[..4].copy_from_slice(&s2[..4]);
    s1[4] = rs(s2[4] * C[28] - s2[7] * C[4]);
    s1[7] = rs(s2[4] * C[4] + s2[7] * C[28]);
    s1[5] = rs(s2[5] * C[12] - s2[6] * C[20]);
    s1[6] = rs(s2[5] * C[20] + s2[6] * C[12]);
    s1[8] = s2[8] + s2[9];
    s1[9] = s2[8] - s2[9];
    s1[10] = -s2[10] + s2[11];
    s1[11] = s2[10] + s2[11];
    s1[12] = s2[12] + s2[13];
    s1[13] = s2[12] - s2[13];
    s1[14] = -s2[14] + s2[15];
    s1[15] = s2[14] + s2[15];
    // stage 4
    s2[0] = rs((s1[0] + s1[1]) * C[16]);
    s2[1] = rs((s1[0] - s1[1]) * C[16]);
    s2[2] = rs(s1[2] * C[24] - s1[3] * C[8]);
    s2[3] = rs(s1[2] * C[8] + s1[3] * C[24]);
    s2[4] = s1[4] + s1[5];
    s2[5] = s1[4] - s1[5];
    s2[6] = -s1[6] + s1[7];
    s2[7] = s1[6] + s1[7];
    s2[8] = s1[8];
    s2[15] = s1[15];
    s2[9] = rs(-s1[9] * C[8] + s1[14] * C[24]);
    s2[14] = rs(s1[9] * C[24] + s1[14] * C[8]);
    s2[10] = rs(-s1[10] * C[24] - s1[13] * C[8]);
    s2[13] = rs(-s1[10] * C[8] + s1[13] * C[24]);
    s2[11] = s1[11];
    s2[12] = s1[12];
    // stage 5
    s1[0] = s2[0] + s2[3];
    s1[1] = s2[1] + s2[2];
    s1[2] = s2[1] - s2[2];
    s1[3] = s2[0] - s2[3];
    s1[4] = s2[4];
    s1[5] = rs((s2[6] - s2[5]) * C[16]);
    s1[6] = rs((s2[5] + s2[6]) * C[16]);
    s1[7] = s2[7];
    s1[8] = s2[8] + s2[11];
    s1[9] = s2[9] + s2[10];
    s1[10] = s2[9] - s2[10];
    s1[11] = s2[8] - s2[11];
    s1[12] = -s2[12] + s2[15];
    s1[13] = -s2[13] + s2[14];
    s1[14] = s2[13] + s2[14];
    s1[15] = s2[12] + s2[15];
    // stage 6
    for k in 0..4 {
        s2[k] = s1[k] + s1[7 - k];
        s2[7 - k] = s1[k] - s1[7 - k];
    }
    s2[8] = s1[8];
    s2[9] = s1[9];
    s2[10] = rs((-s1[10] + s1[13]) * C[16]);
    s2[13] = rs((s1[10] + s1[13]) * C[16]);
    s2[11] = rs((-s1[11] + s1[12]) * C[16]);
    s2[12] = rs((s1[11] + s1[12]) * C[16]);
    s2[14] = s1[14];
    s2[15] = s1[15];
    // stage 7
    for k in 0..8 {
        o[k] = s2[k] + s2[15 - k];
        o[15 - k] = s2[k] - s2[15 - k];
    }
}

fn iadst16(i: &[i64], o: &mut [i64]) {
    let mut x = [i[15], i[0], i[13], i[2], i[11], i[4], i[9], i[6], i[7], i[8], i[5], i[10], i[3], i[12], i[1], i[14]];
    if x.iter().all(|&v| v == 0) {
        o[..16].fill(0);
        return;
    }
    let mut s = [0i64; 16];
    // stage 1
    s[0] = x[0] * C[1] + x[1] * C[31];
    s[1] = x[0] * C[31] - x[1] * C[1];
    s[2] = x[2] * C[5] + x[3] * C[27];
    s[3] = x[2] * C[27] - x[3] * C[5];
    s[4] = x[4] * C[9] + x[5] * C[23];
    s[5] = x[4] * C[23] - x[5] * C[9];
    s[6] = x[6] * C[13] + x[7] * C[19];
    s[7] = x[6] * C[19] - x[7] * C[13];
    s[8] = x[8] * C[17] + x[9] * C[15];
    s[9] = x[8] * C[15] - x[9] * C[17];
    s[10] = x[10] * C[21] + x[11] * C[11];
    s[11] = x[10] * C[11] - x[11] * C[21];
    s[12] = x[12] * C[25] + x[13] * C[7];
    s[13] = x[12] * C[7] - x[13] * C[25];
    s[14] = x[14] * C[29] + x[15] * C[3];
    s[15] = x[14] * C[3] - x[15] * C[29];
    for k in 0..8 {
        x[k] = rs(s[k] + s[k + 8]);
        x[k + 8] = rs(s[k] - s[k + 8]);
    }
    // stage 2
    s[..8].copy_from_slice(&x[..8]);
    s[8] = x[8] * C[4] + x[9] * C[28];
    s[9] = x[8] * C[28] - x[9] * C[4];
    s[10] = x[10] * C[20] + x[11] * C[12];
    s[11] = x[10] * C[12] - x[11] * C[20];
    s[12] = -x[12] * C[28] + x[13] * C[4];
    s[13] = x[12] * C[4] + x[13] * C[28];
    s[14] = -x[14] * C[12] + x[15] * C[20];
    s[15] = x[14] * C[20] + x[15] * C[12];
    for k in 0..4 {
        x[k] = s[k] + s[k + 4];
        x[k + 4] = s[k] - s[k + 4];
        x[k + 8] = rs(s[k + 8] + s[k + 12]);
        x[k + 12] = rs(s[k + 8] - s[k + 12]);
    }
    // stage 3
    s[..4].copy_from_slice(&x[..4]);
    s[4] = x[4] * C[8] + x[5] * C[24];
    s[5] = x[4] * C[24] - x[5] * C[8];
    s[6] = -x[6] * C[24] + x[7] * C[8];
    s[7] = x[6] * C[8] + x[7] * C[24];
    s[8..12].copy_from_slice(&x[8..12]);
    s[12] = x[12] * C[8] + x[13] * C[24];
    s[13] = x[12] * C[24] - x[13] * C[8];
    s[14] = -x[14] * C[24] + x[15] * C[8];
    s[15] = x[14] * C[8] + x[15] * C[24];
    x[0] = s[0] + s[2];
    x[1] = s[1] + s[3];
    x[2] = s[0] - s[2];
    x[3] = s[1] - s[3];
    x[4] = rs(s[4] + s[6]);
    x[5] = rs(s[5] + s[7]);
    x[6] = rs(s[4] - s[6]);
    x[7] = rs(s[5] - s[7]);
    x[8] = s[8] + s[10];
    x[9] = s[9] + s[11];
    x[10] = s[8] - s[10];
    x[11] = s[9] - s[11];
    x[12] = rs(s[12] + s[14]);
    x[13] = rs(s[13] + s[15]);
    x[14] = rs(s[12] - s[14]);
    x[15] = rs(s[13] - s[15]);
    // stage 4
    let s2 = -C[16] * (x[2] + x[3]);
    let s3 = C[16] * (x[2] - x[3]);
    let s6 = C[16] * (x[6] + x[7]);
    let s7 = C[16] * (-x[6] + x[7]);
    let s10 = C[16] * (x[10] + x[11]);
    let s11 = C[16] * (-x[10] + x[11]);
    let s14 = -C[16] * (x[14] + x[15]);
    let s15 = C[16] * (x[14] - x[15]);
    x[2] = rs(s2);
    x[3] = rs(s3);
    x[6] = rs(s6);
    x[7] = rs(s7);
    x[10] = rs(s10);
    x[11] = rs(s11);
    x[14] = rs(s14);
    x[15] = rs(s15);
    o[0] = x[0];
    o[1] = -x[8];
    o[2] = x[12];
    o[3] = -x[4];
    o[4] = x[6];
    o[5] = x[14];
    o[6] = x[10];
    o[7] = x[2];
    o[8] = x[3];
    o[9] = x[11];
    o[10] = x[15];
    o[11] = x[7];
    o[12] = x[5];
    o[13] = -x[13];
    o[14] = x[9];
    o[15] = -x[1];
}

/// One rotation pair of the 32-point DCT: (a*cx - b*cy, a*cy + b*cx) rounded.
#[inline(always)]
fn rot(a: i64, b: i64, cx: i64, cy: i64) -> (i64, i64) {
    (rs(a * cx - b * cy), rs(a * cy + b * cx))
}

fn idct32(i: &[i64], o: &mut [i64]) {
    let mut s1 = [0i64; 32];
    let mut s2 = [0i64; 32];
    // stage 1
    const EVEN: [usize; 16] = [0, 16, 8, 24, 4, 20, 12, 28, 2, 18, 10, 26, 6, 22, 14, 30];
    for k in 0..16 {
        s1[k] = i[EVEN[k]];
    }
    (s1[16], s1[31]) = rot(i[1], i[31], C[31], C[1]);
    (s1[17], s1[30]) = rot(i[17], i[15], C[15], C[17]);
    (s1[18], s1[29]) = rot(i[9], i[23], C[23], C[9]);
    (s1[19], s1[28]) = rot(i[25], i[7], C[7], C[25]);
    (s1[20], s1[27]) = rot(i[5], i[27], C[27], C[5]);
    (s1[21], s1[26]) = rot(i[21], i[11], C[11], C[21]);
    (s1[22], s1[25]) = rot(i[13], i[19], C[19], C[13]);
    (s1[23], s1[24]) = rot(i[29], i[3], C[3], C[29]);
    // stage 2
    s2[..8].copy_from_slice(&s1[..8]);
    (s2[8], s2[15]) = rot(s1[8], s1[15], C[30], C[2]);
    (s2[9], s2[14]) = rot(s1[9], s1[14], C[14], C[18]);
    (s2[10], s2[13]) = rot(s1[10], s1[13], C[22], C[10]);
    (s2[11], s2[12]) = rot(s1[11], s1[12], C[6], C[26]);
    for k in (16..32).step_by(4) {
        s2[k] = s1[k] + s1[k + 1];
        s2[k + 1] = s1[k] - s1[k + 1];
        s2[k + 2] = -s1[k + 2] + s1[k + 3];
        s2[k + 3] = s1[k + 2] + s1[k + 3];
    }
    // stage 3
    s1[..4].copy_from_slice(&s2[..4]);
    (s1[4], s1[7]) = rot(s2[4], s2[7], C[28], C[4]);
    (s1[5], s1[6]) = rot(s2[5], s2[6], C[12], C[20]);
    for k in (8..16).step_by(4) {
        s1[k] = s2[k] + s2[k + 1];
        s1[k + 1] = s2[k] - s2[k + 1];
        s1[k + 2] = -s2[k + 2] + s2[k + 3];
        s1[k + 3] = s2[k + 2] + s2[k + 3];
    }
    s1[16] = s2[16];
    s1[31] = s2[31];
    s1[17] = rs(-s2[17] * C[4] + s2[30] * C[28]);
    s1[30] = rs(s2[17] * C[28] + s2[30] * C[4]);
    s1[18] = rs(-s2[18] * C[28] - s2[29] * C[4]);
    s1[29] = rs(-s2[18] * C[4] + s2[29] * C[28]);
    s1[19] = s2[19];
    s1[20] = s2[20];
    s1[21] = rs(-s2[21] * C[20] + s2[26] * C[12]);
    s1[26] = rs(s2[21] * C[12] + s2[26] * C[20]);
    s1[22] = rs(-s2[22] * C[12] - s2[25] * C[20]);
    s1[25] = rs(-s2[22] * C[20] + s2[25] * C[12]);
    s1[23] = s2[23];
    s1[24] = s2[24];
    s1[27] = s2[27];
    s1[28] = s2[28];
    // stage 4
    s2[0] = rs((s1[0] + s1[1]) * C[16]);
    s2[1] = rs((s1[0] - s1[1]) * C[16]);
    s2[2] = rs(s1[2] * C[24] - s1[3] * C[8]);
    s2[3] = rs(s1[2] * C[8] + s1[3] * C[24]);
    s2[4] = s1[4] + s1[5];
    s2[5] = s1[4] - s1[5];
    s2[6] = -s1[6] + s1[7];
    s2[7] = s1[6] + s1[7];
    s2[8] = s1[8];
    s2[15] = s1[15];
    s2[9] = rs(-s1[9] * C[8] + s1[14] * C[24]);
    s2[14] = rs(s1[9] * C[24] + s1[14] * C[8]);
    s2[10] = rs(-s1[10] * C[24] - s1[13] * C[8]);
    s2[13] = rs(-s1[10] * C[8] + s1[13] * C[24]);
    s2[11] = s1[11];
    s2[12] = s1[12];
    for k in [16usize, 24] {
        s2[k] = s1[k] + s1[k + 3];
        s2[k + 1] = s1[k + 1] + s1[k + 2];
        s2[k + 2] = s1[k + 1] - s1[k + 2];
        s2[k + 3] = s1[k] - s1[k + 3];
        s2[k + 4] = -s1[k + 4] + s1[k + 7];
        s2[k + 5] = -s1[k + 5] + s1[k + 6];
        s2[k + 6] = s1[k + 5] + s1[k + 6];
        s2[k + 7] = s1[k + 4] + s1[k + 7];
    }
    // stage 5
    s1[0] = s2[0] + s2[3];
    s1[1] = s2[1] + s2[2];
    s1[2] = s2[1] - s2[2];
    s1[3] = s2[0] - s2[3];
    s1[4] = s2[4];
    s1[5] = rs((s2[6] - s2[5]) * C[16]);
    s1[6] = rs((s2[5] + s2[6]) * C[16]);
    s1[7] = s2[7];
    s1[8] = s2[8] + s2[11];
    s1[9] = s2[9] + s2[10];
    s1[10] = s2[9] - s2[10];
    s1[11] = s2[8] - s2[11];
    s1[12] = -s2[12] + s2[15];
    s1[13] = -s2[13] + s2[14];
    s1[14] = s2[13] + s2[14];
    s1[15] = s2[12] + s2[15];
    s1[16] = s2[16];
    s1[17] = s2[17];
    s1[18] = rs(-s2[18] * C[8] + s2[29] * C[24]);
    s1[29] = rs(s2[18] * C[24] + s2[29] * C[8]);
    s1[19] = rs(-s2[19] * C[8] + s2[28] * C[24]);
    s1[28] = rs(s2[19] * C[24] + s2[28] * C[8]);
    s1[20] = rs(-s2[20] * C[24] - s2[27] * C[8]);
    s1[27] = rs(-s2[20] * C[8] + s2[27] * C[24]);
    s1[21] = rs(-s2[21] * C[24] - s2[26] * C[8]);
    s1[26] = rs(-s2[21] * C[8] + s2[26] * C[24]);
    s1[22] = s2[22];
    s1[23] = s2[23];
    s1[24] = s2[24];
    s1[25] = s2[25];
    s1[30] = s2[30];
    s1[31] = s2[31];
    // stage 6
    for k in 0..4 {
        s2[k] = s1[k] + s1[7 - k];
        s2[7 - k] = s1[k] - s1[7 - k];
    }
    s2[8] = s1[8];
    s2[9] = s1[9];
    s2[10] = rs((-s1[10] + s1[13]) * C[16]);
    s2[13] = rs((s1[10] + s1[13]) * C[16]);
    s2[11] = rs((-s1[11] + s1[12]) * C[16]);
    s2[12] = rs((s1[11] + s1[12]) * C[16]);
    s2[14] = s1[14];
    s2[15] = s1[15];
    for k in 0..4 {
        s2[16 + k] = s1[16 + k] + s1[23 - k];
        s2[23 - k] = s1[16 + k] - s1[23 - k];
        s2[24 + k] = -s1[24 + k] + s1[31 - k];
        s2[31 - k] = s1[24 + k] + s1[31 - k];
    }
    // stage 7
    for k in 0..8 {
        s1[k] = s2[k] + s2[15 - k];
        s1[15 - k] = s2[k] - s2[15 - k];
    }
    s1[16..20].copy_from_slice(&s2[16..20]);
    for k in 0..4 {
        s1[20 + k] = rs((-s2[20 + k] + s2[27 - k]) * C[16]);
        s1[27 - k] = rs((s2[20 + k] + s2[27 - k]) * C[16]);
    }
    s1[28..32].copy_from_slice(&s2[28..32]);
    // final stage
    for k in 0..16 {
        o[k] = s1[k] + s1[31 - k];
        o[31 - k] = s1[k] - s1[31 - k];
    }
}

/// The lossless 4x4 inverse WHT (§8.7.1.10), added into `dst`.
fn iwht4x4_add(coefs: &[i32], dst: &mut [u16], off: usize, stride: usize, max: i32) {
    let mut out = [0i64; 16];
    for i in 0..4 {
        let ip = &coefs[i * 4..];
        let mut a1 = (ip[0] >> 2) as i64;
        let mut c1 = (ip[1] >> 2) as i64;
        let mut d1 = (ip[2] >> 2) as i64;
        let mut b1 = (ip[3] >> 2) as i64;
        a1 += c1;
        d1 -= b1;
        let e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= b1;
        d1 += c1;
        out[i * 4] = a1;
        out[i * 4 + 1] = b1;
        out[i * 4 + 2] = c1;
        out[i * 4 + 3] = d1;
    }
    for i in 0..4 {
        let mut a1 = out[i];
        let mut c1 = out[4 + i];
        let mut d1 = out[8 + i];
        let mut b1 = out[12 + i];
        a1 += c1;
        d1 -= b1;
        let e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= b1;
        d1 += c1;
        for (r, v) in [a1, b1, c1, d1].into_iter().enumerate() {
            let p = &mut dst[off + r * stride + i];
            *p = (*p as i64 + v).clamp(0, max as i64) as u16;
        }
    }
}

/// Reconstruct one transform block: inverse transform `coefs` (row-major, `4 << tx_size` square)
/// and add the residual into `dst[off..]` (row stride `stride`), clipping to the bit depth.
pub fn inverse_transform_add(coefs: &[i32], tx_size: u8, tx_type: u8, lossless: bool, dst: &mut [u16], off: usize, stride: usize, bit_depth: u8) {
    let max = (1i32 << bit_depth) - 1;
    if lossless {
        iwht4x4_add(coefs, dst, off, stride, max);
        return;
    }
    let n = 4usize << tx_size;
    type Tx = fn(&[i64], &mut [i64]);
    let (row_tx, col_tx): (Tx, Tx) = match tx_size {
        TX_4X4 => match tx_type {
            ADST_DCT => (idct4, iadst4),
            DCT_ADST => (iadst4, idct4),
            ADST_ADST => (iadst4, iadst4),
            _ => (idct4, idct4),
        },
        TX_8X8 => match tx_type {
            ADST_DCT => (idct8, iadst8),
            DCT_ADST => (iadst8, idct8),
            ADST_ADST => (iadst8, iadst8),
            _ => (idct8, idct8),
        },
        TX_16X16 => match tx_type {
            ADST_DCT => (idct16, iadst16),
            DCT_ADST => (iadst16, idct16),
            ADST_ADST => (iadst16, iadst16),
            _ => (idct16, idct16),
        },
        _ => (idct32, idct32),
    };
    let shift = match tx_size {
        TX_4X4 => 4,
        TX_8X8 => 5,
        _ => 6,
    };
    let mut mid = [0i64; 1024];
    let mut tin = [0i64; 32];
    let mut tout = [0i64; 32];
    for r in 0..n {
        let row = &coefs[r * n..r * n + n];
        if row.iter().all(|&v| v == 0) {
            continue; // the transform of a zero row is zero (mid is zero-initialised)
        }
        for c in 0..n {
            tin[c] = row[c] as i64;
        }
        row_tx(&tin[..n], &mut tout[..n]);
        mid[r * n..r * n + n].copy_from_slice(&tout[..n]);
    }
    let round = 1i64 << (shift - 1);
    for c in 0..n {
        for r in 0..n {
            tin[r] = mid[r * n + c];
        }
        col_tx(&tin[..n], &mut tout[..n]);
        for r in 0..n {
            let p = &mut dst[off + r * stride + c];
            *p = (*p as i64 + ((tout[r] + round) >> shift)).clamp(0, max as i64) as u16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A DC-only block adds the same value everywhere: Round2(Round2(dc*c16,14)*c16,14) then the
    /// size's final shift (the reference decoder's *_1_add fast path computes exactly this).
    #[test]
    fn dc_only_is_flat() {
        for tx in 0..4u8 {
            let n = 4usize << tx;
            let mut coefs = vec![0i32; n * n];
            coefs[0] = 1000;
            let mut dst = vec![128u16; n * n];
            inverse_transform_add(&coefs, tx, DCT_DCT, false, &mut dst, 0, n, 8);
            let shift = [4, 5, 6, 6][tx as usize];
            let a = rs(rs(1000 * C[16]) * C[16]);
            let want = (128 + ((a + (1 << (shift - 1))) >> shift)) as u16;
            assert!(dst.iter().all(|&p| p == want), "tx {tx}: {:?} want {want}", &dst[..4]);
        }
    }

    #[test]
    fn zero_block_is_identity() {
        let coefs = vec![0i32; 1024];
        let mut dst = vec![77u16; 1024];
        for tx in 0..4u8 {
            for t in 0..4u8 {
                inverse_transform_add(&coefs, tx, t, false, &mut dst, 0, 32, 8);
            }
        }
        assert!(dst.iter().all(|&p| p == 77));
    }

    /// Lossless DC: the WHT of a lone DC spreads it as 4ths (a quarter, with the rounding of >> 1).
    #[test]
    fn wht_dc() {
        let mut coefs = [0i32; 16];
        coefs[0] = 64 << 2;
        let mut dst = [100u16; 16];
        inverse_transform_add(&coefs, TX_4X4, DCT_DCT, true, &mut dst, 0, 4, 8);
        assert_eq!(dst.iter().map(|&v| v as u32).sum::<u32>(), 100 * 16 + 64);
    }
}
