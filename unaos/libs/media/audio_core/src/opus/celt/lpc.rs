//! The pieces of `celt/celt_lpc.c` and `celt/pitch.c` the packet-loss concealment uses (fixed point):
//! windowed autocorrelation, Levinson-Durbin with the 16-bit fit, FIR/IIR filters, the 2x pitch
//! downsampler and the two-stage pitch search.
use super::fixed::*;
use super::bands::inner_prod;
use alloc::vec;

pub fn celt_lpc(lpc_out: &mut [i16], ac: &[i32], p: usize) {
    let mut lpc = [0i32; 24];
    let mut error = ac[0];
    if ac[0] != 0 {
        for i in 0..p {
            let mut rr = 0i32;
            for j in 0..i { rr = rr.wrapping_add(mult32_32_q31(lpc[j], ac[i - j])); }
            rr = rr.wrapping_add(ac[i + 1] >> 6);
            let r = frac_div32(shl32(rr, 6), error).wrapping_neg();
            lpc[i] = r >> 6;
            for j in 0..(i + 1) >> 1 {
                let tmp1 = lpc[j];
                let tmp2 = lpc[i - 1 - j];
                lpc[j] = tmp1.wrapping_add(mult32_32_q31(r, tmp2));
                lpc[i - 1 - j] = tmp2.wrapping_add(mult32_32_q31(r, tmp1));
            }
            error = error.wrapping_sub(mult32_32_q31(mult32_32_q31(r, r), error));
            if error <= ac[0] >> 10 { break; }
        }
    }
    let mut idx = 0usize;
    let mut iter = 0;
    while iter < 10 {
        let mut maxabs = 0i32;
        for i in 0..p {
            let a = lpc[i].wrapping_abs();
            if a > maxabs { maxabs = a; idx = i; }
        }
        maxabs = pshr32(maxabs, 13);
        if maxabs > 32767 {
            maxabs = maxabs.min(163838);
            let mut chirp_q16 = 65470 - (shl32(maxabs - 32767, 14) / ((maxabs.wrapping_mul(idx as i32 + 1)) >> 2));
            let chirp_minus_one_q16 = chirp_q16 - 65536;
            for i in 0..p - 1 {
                lpc[i] = mult32_32_q16(chirp_q16, lpc[i]);
                chirp_q16 += pshr32(chirp_q16.wrapping_mul(chirp_minus_one_q16), 16);
            }
            lpc[p - 1] = mult32_32_q16(chirp_q16, lpc[p - 1]);
        } else {
            break;
        }
        iter += 1;
    }
    if iter == 10 {
        for v in lpc_out[..p].iter_mut() { *v = 0; }
        lpc_out[0] = 4096;
    } else {
        for i in 0..p { lpc_out[i] = pshr32(lpc[i], 13) as i16; }
    }
}

/// `_celt_autocorr` (returns the shift).
pub fn celt_autocorr(x: &[i16], ac: &mut [i32], window: Option<&[i16]>, overlap: usize, lag: usize, n: usize) -> i32 {
    let fast_n = n - lag;
    let mut xx = vec![0i16; n];
    xx.copy_from_slice(&x[..n]);
    if overlap > 0 {
        let w = window.unwrap();
        for i in 0..overlap {
            xx[i] = mult16_16_q15(x[i] as i32, w[i] as i32) as i16;
            xx[n - i - 1] = mult16_16_q15(x[n - i - 1] as i32, w[i] as i32) as i16;
        }
    }
    let mut ac0 = 1i32 + ((n as i32) << 7);
    if n & 1 != 0 { ac0 = ac0.wrapping_add(mult16_16(xx[0] as i32, xx[0] as i32) >> 9); }
    let mut i = n & 1;
    while i < n {
        ac0 = ac0.wrapping_add(mult16_16(xx[i] as i32, xx[i] as i32) >> 9);
        ac0 = ac0.wrapping_add(mult16_16(xx[i + 1] as i32, xx[i + 1] as i32) >> 9);
        i += 2;
    }
    let mut shift = (celt_ilog2(ac0) - 30 + 10) / 2;
    if shift > 0 {
        for v in xx.iter_mut() { *v = pshr32(*v as i32, shift) as i16; }
    } else {
        shift = 0;
    }
    for k in 0..=lag {
        let mut s = inner_prod(&xx[..fast_n], &xx[k..k + fast_n], fast_n);
        let mut d = 0i32;
        for i in k + fast_n..n { d = mac16_16(d, xx[i] as i32, xx[i - k] as i32); }
        s = s.wrapping_add(d);
        ac[k] = s;
    }
    shift *= 2;
    if shift <= 0 { ac[0] = ac[0].wrapping_add(shl32(1, -shift)); }
    if ac[0] < 268435456 {
        let shift2 = 29 - super::super::range::ilog(ac[0] as u32);
        for v in ac[..=lag].iter_mut() { *v = shl32(*v, shift2); }
        shift -= shift2;
    } else if ac[0] >= 536870912 {
        let mut shift2 = 1;
        if ac[0] >= 1073741824 { shift2 += 1; }
        for v in ac[..=lag].iter_mut() { *v >>= shift2; }
        shift += shift2;
    }
    shift
}

/// `celt_fir`: `x` holds `ord` history samples before index `x0`.
pub fn celt_fir(x: &[i16], x0: usize, num: &[i16], y: &mut [i16], n: usize, ord: usize) {
    for i in 0..n {
        let mut sum = shl32(x[x0 + i] as i32, SIG_SHIFT);
        for j in 0..ord { sum = mac16_16(sum, num[ord - j - 1] as i32, x[x0 + i + j - ord] as i32); }
        y[i] = sround16(sum, SIG_SHIFT) as i16;
    }
}

/// `celt_iir` in place on `buf[off..off+n]`.
pub fn celt_iir(buf: &mut [i32], off: usize, den: &[i16], n: usize, ord: usize, mem: &mut [i16]) {
    let mut y = vec![0i16; n + ord];
    for i in 0..ord { y[i] = mem[ord - i - 1]; }
    for i in 0..n {
        let mut sum = buf[off + i];
        for j in 0..ord { sum = sum.wrapping_sub(mult16_16(den[ord - j - 1] as i32, y[i + j] as i32)); }
        y[i + ord] = sround16(sum, SIG_SHIFT) as i16;
        buf[off + i] = sum;
    }
    for i in 0..ord { mem[i] = buf[off + n - i - 1] as i16; }
}

fn celt_fir5(x: &mut [i16], num: &[i32; 5], n: usize) {
    let mut mem = [0i32; 5];
    for i in 0..n {
        let mut sum = shl32(x[i] as i32, SIG_SHIFT);
        for k in 0..5 { sum = mac16_16(sum, num[k], mem[k]); }
        mem[4] = mem[3]; mem[3] = mem[2]; mem[2] = mem[1]; mem[1] = mem[0];
        mem[0] = x[i] as i32;
        x[i] = round16(sum, SIG_SHIFT) as i16;
    }
}

pub fn pitch_downsample(x: &[&[i32]], x_lp: &mut [i16], len: usize) {
    let c = x.len();
    let mut maxabs = celt_maxabs32(&x[0][..len]);
    if c == 2 { maxabs = maxabs.max(celt_maxabs32(&x[1][..len])); }
    if maxabs < 1 { maxabs = 1; }
    let mut shift = celt_ilog2(maxabs) - 10;
    if shift < 0 { shift = 0; }
    if c == 2 { shift += 1; }
    for ch in 0..c {
        let xs = x[ch];
        for i in 1..len >> 1 {
            let v = (xs[2 * i - 1] >> (shift + 2)).wrapping_add(xs[2 * i + 1] >> (shift + 2)).wrapping_add(xs[2 * i] >> (shift + 1));
            x_lp[i] = if ch == 0 { v as i16 } else { (x_lp[i] as i32 + v) as i16 };
        }
        let v = (xs[1] >> (shift + 2)).wrapping_add(xs[0] >> (shift + 1));
        x_lp[0] = if ch == 0 { v as i16 } else { (x_lp[0] as i32 + v) as i16 };
    }
    let mut ac = [0i32; 5];
    celt_autocorr(x_lp, &mut ac, None, 0, 4, len >> 1);
    ac[0] = ac[0].wrapping_add(ac[0] >> 13);
    for i in 1..=4 { ac[i] = ac[i].wrapping_sub(mult16_32_q15((2 * i * i) as i32, ac[i])); }
    let mut lpc = [0i16; 4];
    celt_lpc(&mut lpc, &ac, 4);
    let mut tmp = Q15ONE;
    for i in 0..4 {
        tmp = mult16_16_q15(29491, tmp);
        lpc[i] = mult16_16_q15(lpc[i] as i32, tmp) as i16;
    }
    let c1 = 26214;
    let lpc2 = [
        (lpc[0] as i32 + 3277) as i16 as i32,
        (lpc[1] as i32 + mult16_16_q15(c1, lpc[0] as i32)) as i16 as i32,
        (lpc[2] as i32 + mult16_16_q15(c1, lpc[1] as i32)) as i16 as i32,
        (lpc[3] as i32 + mult16_16_q15(c1, lpc[2] as i32)) as i16 as i32,
        mult16_16_q15(c1, lpc[3] as i32) as i16 as i32,
    ];
    celt_fir5(x_lp, &lpc2, len >> 1);
}

fn find_best_pitch(xcorr: &[i32], y: &[i16], len: usize, max_pitch: usize, best_pitch: &mut [usize; 2], yshift: i32, maxcorr: i32) {
    let mut syy = 1i32;
    let mut best_num = [-1i32; 2];
    let mut best_den = [0i32; 2];
    let xshift = celt_ilog2(maxcorr) - 14;
    best_pitch[0] = 0;
    best_pitch[1] = 1;
    for j in 0..len { syy = syy.wrapping_add(mult16_16(y[j] as i32, y[j] as i32) >> yshift); }
    for i in 0..max_pitch {
        if xcorr[i] > 0 {
            let xcorr16 = vshr32(xcorr[i], xshift) as i16 as i32;
            let num = mult16_16_q15(xcorr16, xcorr16) as i16 as i32;
            if mult16_32_q15(num, best_den[1]) > mult16_32_q15(best_num[1], syy) {
                if mult16_32_q15(num, best_den[0]) > mult16_32_q15(best_num[0], syy) {
                    best_num[1] = best_num[0];
                    best_den[1] = best_den[0];
                    best_pitch[1] = best_pitch[0];
                    best_num[0] = num;
                    best_den[0] = syy;
                    best_pitch[0] = i;
                } else {
                    best_num[1] = num;
                    best_den[1] = syy;
                    best_pitch[1] = i;
                }
            }
        }
        syy = syy.wrapping_add((mult16_16(y[i + len] as i32, y[i + len] as i32) >> yshift) - (mult16_16(y[i] as i32, y[i] as i32) >> yshift));
        syy = syy.max(1);
    }
}

pub fn pitch_search(x_lp: &[i16], y: &[i16], len: usize, max_pitch: usize) -> i32 {
    let lag = len + max_pitch;
    let mut x_lp4 = vec![0i16; len >> 2];
    let mut y_lp4 = vec![0i16; lag >> 2];
    let mut xcorr = vec![0i32; max_pitch >> 1];
    for j in 0..len >> 2 { x_lp4[j] = x_lp[2 * j]; }
    for j in 0..lag >> 2 { y_lp4[j] = y[2 * j]; }
    let xmax = celt_maxabs16(&x_lp4);
    let ymax = celt_maxabs16(&y_lp4);
    let mut shift = celt_ilog2(1.max(xmax.max(ymax))) - 11;
    if shift > 0 {
        for v in x_lp4.iter_mut() { *v >>= shift; }
        for v in y_lp4.iter_mut() { *v >>= shift; }
        shift *= 2;
    } else {
        shift = 0;
    }
    let mut maxcorr = 1i32;
    for i in 0..max_pitch >> 2 {
        let s = inner_prod(&x_lp4, &y_lp4[i..], len >> 2);
        xcorr[i] = s;
        maxcorr = maxcorr.max(s);
    }
    let mut best = [0usize; 2];
    find_best_pitch(&xcorr, &y_lp4, len >> 2, max_pitch >> 2, &mut best, 0, maxcorr);
    maxcorr = 1;
    for i in 0..max_pitch >> 1 {
        xcorr[i] = 0;
        if (i as i32 - 2 * best[0] as i32).abs() > 2 && (i as i32 - 2 * best[1] as i32).abs() > 2 { continue; }
        let mut sum = 0i32;
        for j in 0..len >> 1 { sum = sum.wrapping_add(mult16_16(x_lp[j] as i32, y[i + j] as i32) >> shift); }
        xcorr[i] = sum.max(-1);
        maxcorr = maxcorr.max(sum);
    }
    find_best_pitch(&xcorr, y, len >> 1, max_pitch >> 1, &mut best, shift + 1, maxcorr);
    let offset;
    if best[0] > 0 && best[0] < (max_pitch >> 1) - 1 {
        let a = xcorr[best[0] - 1];
        let b = xcorr[best[0]];
        let c = xcorr[best[0] + 1];
        if c - a > mult16_32_q15(22938, b - a) { offset = 1; }
        else if a - c > mult16_32_q15(22938, b - c) { offset = -1; }
        else { offset = 0; }
    } else {
        offset = 0;
    }
    2 * best[0] as i32 - offset
}
