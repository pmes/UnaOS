// SPDX-License-Identifier: LGPL-3.0-or-later
//! Blurs: box (running sum) and gaussian (sampled integer kernel), both separable with edge clamp. They blur all
//! four channels, alpha too (straight alpha: a transparent pixel's colour bleeds; ADJUSTCORE-PREMUL is owed).

use alloc::vec;
use alloc::vec::Vec;

use crate::levels::exp2_q16;
use crate::{Error, check};

/// The gaussian kernel row sums to exactly `1 << GAUSS_SHIFT`.
pub const GAUSS_SHIFT: u32 = 16;
/// `round(log2(e) * 65536)`.
const LOG2_E_Q16: i64 = 94548;

/// One separable pass: `f(get, n)` filters each row (horizontal) or column (vertical) of `n` samples, per
/// channel, `get(i)` reading sample `i` of that line.
fn pass(src: &[u8], w: usize, h: usize, horizontal: bool, f: &dyn Fn(&dyn Fn(usize) -> u32, usize) -> Vec<u8>) -> Vec<u8> {
    let mut dst = vec![0u8; src.len()];
    let (lines, n) = if horizontal { (h, w) } else { (w, h) };
    for line in 0..lines {
        for ch in 0..4 {
            let at = |i: usize| -> usize {
                let (x, y) = if horizontal { (i, line) } else { (line, i) };
                (y * w + x) * 4 + ch
            };
            let get = |i: usize| src[at(i)] as u32;
            let out = f(&get, n);
            for (i, v) in out.into_iter().enumerate() {
                dst[at(i)] = v;
            }
        }
    }
    dst
}

/// Running-sum box over one line of `n` samples, window `2r + 1`, edge clamp, rounded.
fn box_line(get: &dyn Fn(usize) -> u32, n: usize, r: usize) -> Vec<u8> {
    let win = (2 * r + 1) as u32;
    let clamp = |i: isize| -> usize { i.clamp(0, n as isize - 1) as usize };
    let mut sum: u32 = (-(r as isize)..=r as isize).map(|i| get(clamp(i))).sum();
    let mut out = Vec::with_capacity(n);
    for i in 0..n as isize {
        out.push(((sum + win / 2) / win) as u8);
        sum += get(clamp(i + r as isize + 1));
        sum -= get(clamp(i - r as isize));
    }
    out
}

/// Box blur, separable (horizontal then vertical), edge clamp, all four channels. `radius == 0` is a copy.
pub fn box_blur(px: &[u8], w: u32, h: u32, radius: u16) -> Result<Vec<u8>, Error> {
    check(px, w, h)?;
    if radius == 0 || px.is_empty() {
        return Ok(px.to_vec());
    }
    let (w, h, r) = (w as usize, h as usize, radius as usize);
    let f = |get: &dyn Fn(usize) -> u32, n: usize| box_line(get, n, r);
    let t = pass(px, w, h, true, &f);
    Ok(pass(&t, w, h, false, &f))
}

/// The sampled gaussian row for `sigma_q8` (1.0 = 256): length `2 * ceil(3 sigma) + 1`, summing to exactly
/// `1 << GAUSS_SHIFT` (the rounding residual on the centre tap). `sigma_q8 == 0` gives `[65536]`.
pub fn gaussian_kernel(sigma_q8: u16) -> Vec<u32> {
    let total: i64 = 1 << GAUSS_SHIFT;
    if sigma_q8 == 0 {
        return vec![total as u32];
    }
    let r = (3 * sigma_q8 as usize).div_ceil(256);
    let s2 = 2 * (sigma_q8 as i64) * (sigma_q8 as i64); // 2 sigma^2, in q8^2 units
    // Q16 samples w(i) = exp(-i^2 / (2 sigma^2)) = 2^(-x log2 e), x = i^2 / (2 sigma^2) in Q16.
    let raw: Vec<i64> = (0..=r)
        .map(|i| {
            let x_q16 = ((i * i) as i64) * (1i64 << 32) / s2;
            let y = -(x_q16 * LOG2_E_Q16 >> 16);
            exp2_q16(y) as i64
        })
        .collect();
    let sum: i64 = raw[0] + 2 * raw[1..].iter().sum::<i64>();
    let mut half: Vec<i64> = raw.iter().map(|&v| (v * total + sum / 2) / sum).collect();
    let got = half[0] + 2 * half[1..].iter().sum::<i64>();
    half[0] += total - got;
    let mut k = Vec::with_capacity(2 * r + 1);
    k.extend(half[1..].iter().rev().map(|&v| v as u32));
    k.extend(half.iter().map(|&v| v as u32));
    k
}

/// Convolve one line with `k` (odd length, sums to `1 << GAUSS_SHIFT`), edge clamp, rounded.
fn kernel_line(get: &dyn Fn(usize) -> u32, n: usize, k: &[u32]) -> Vec<u8> {
    let r = (k.len() / 2) as isize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n as isize {
        let mut acc: u64 = 0;
        for (j, &kw) in k.iter().enumerate() {
            let s = (i + j as isize - r).clamp(0, n as isize - 1) as usize;
            acc += kw as u64 * get(s) as u64;
        }
        out.push(((acc + (1 << (GAUSS_SHIFT - 1))) >> GAUSS_SHIFT).min(255) as u8);
    }
    out
}

/// Gaussian blur, separable, edge clamp, all four channels; radius `ceil(3 sigma)`. `sigma_q8 == 0` is a copy.
pub fn gaussian_blur(px: &[u8], w: u32, h: u32, sigma_q8: u16) -> Result<Vec<u8>, Error> {
    check(px, w, h)?;
    if sigma_q8 == 0 || px.is_empty() {
        return Ok(px.to_vec());
    }
    let k = gaussian_kernel(sigma_q8);
    let (w, h) = (w as usize, h as usize);
    let f = |get: &dyn Fn(usize) -> u32, n: usize| kernel_line(get, n, &k);
    let t = pass(px, w, h, true, &f);
    Ok(pass(&t, w, h, false, &f))
}
