//! The fixed-point KISS FFT (`celt/kiss_fft.c`, radices 2/3/4/5) and the inverse MDCT
//! (`celt/mdct.c::clt_mdct_backward`) with its TDAC window mirror, exactly as the reference computes them.
use super::fixed::*;
use super::tables::{FftState, FFT_STATES, FFT_TWIDDLES, MDCT_TWIDDLES960};

#[inline(always)] fn s_mul(a: i32, b: i32) -> i32 { mult16_32_q15(b, a) }
#[inline(always)] fn tw(i: usize) -> (i32, i32) { (FFT_TWIDDLES[2 * i] as i32, FFT_TWIDDLES[2 * i + 1] as i32) }
#[inline(always)] fn cmul(a: (i32, i32), b: (i32, i32)) -> (i32, i32) {
    (s_mul(a.0, b.0).wrapping_sub(s_mul(a.1, b.1)), s_mul(a.0, b.1).wrapping_add(s_mul(a.1, b.0)))
}
#[inline(always)] fn get(f: &[i32], k: usize) -> (i32, i32) { (f[2 * k], f[2 * k + 1]) }
#[inline(always)] fn set(f: &mut [i32], k: usize, v: (i32, i32)) { f[2 * k] = v.0; f[2 * k + 1] = v.1; }
#[inline(always)] fn cadd(a: (i32, i32), b: (i32, i32)) -> (i32, i32) { (a.0.wrapping_add(b.0), a.1.wrapping_add(b.1)) }
#[inline(always)] fn csub(a: (i32, i32), b: (i32, i32)) -> (i32, i32) { (a.0.wrapping_sub(b.0), a.1.wrapping_sub(b.1)) }

fn bfly2(f: &mut [i32], base: usize, n: usize) {
    let twc = 23170; // QCONST16(0.7071067812, 15)
    let mut fo = base;
    for _ in 0..n {
        let t = get(f, fo + 4);
        set(f, fo + 4, csub(get(f, fo), t));
        set(f, fo, cadd(get(f, fo), t));
        let a = get(f, fo + 5);
        let t = (s_mul(a.0.wrapping_add(a.1), twc), s_mul(a.1.wrapping_sub(a.0), twc));
        set(f, fo + 5, csub(get(f, fo + 1), t));
        set(f, fo + 1, cadd(get(f, fo + 1), t));
        let a = get(f, fo + 6);
        let t = (a.1, a.0.wrapping_neg());
        set(f, fo + 6, csub(get(f, fo + 2), t));
        set(f, fo + 2, cadd(get(f, fo + 2), t));
        let a = get(f, fo + 7);
        let t = (s_mul(a.1.wrapping_sub(a.0), twc), s_mul(a.1.wrapping_add(a.0).wrapping_neg(), twc));
        set(f, fo + 7, csub(get(f, fo + 3), t));
        set(f, fo + 3, cadd(get(f, fo + 3), t));
        fo += 8;
    }
}

fn bfly4(f: &mut [i32], base: usize, fstride: usize, m: usize, n: usize, mm: usize) {
    if m == 1 {
        let mut fo = base;
        for _ in 0..n {
            let f0 = get(f, fo); let f1 = get(f, fo + 1); let f2 = get(f, fo + 2); let f3 = get(f, fo + 3);
            let s0 = csub(f0, f2);
            let f0 = cadd(f0, f2);
            let s1 = cadd(f1, f3);
            set(f, fo + 2, csub(f0, s1));
            set(f, fo, cadd(f0, s1));
            let s1 = csub(f1, f3);
            set(f, fo + 1, (s0.0.wrapping_add(s1.1), s0.1.wrapping_sub(s1.0)));
            set(f, fo + 3, (s0.0.wrapping_sub(s1.1), s0.1.wrapping_add(s1.0)));
            fo += 4;
        }
    } else {
        let (m2, m3) = (2 * m, 3 * m);
        for i in 0..n {
            let mut fo = base + i * mm;
            let (mut t1, mut t2, mut t3) = (0usize, 0usize, 0usize);
            for _ in 0..m {
                let s0 = cmul(get(f, fo + m), tw(t1));
                let s1 = cmul(get(f, fo + m2), tw(t2));
                let s2 = cmul(get(f, fo + m3), tw(t3));
                let f0 = get(f, fo);
                let s5 = csub(f0, s1);
                let f0 = cadd(f0, s1);
                let s3 = cadd(s0, s2);
                let s4 = csub(s0, s2);
                set(f, fo + m2, csub(f0, s3));
                t1 += fstride; t2 += fstride * 2; t3 += fstride * 3;
                set(f, fo, cadd(f0, s3));
                set(f, fo + m, (s5.0.wrapping_add(s4.1), s5.1.wrapping_sub(s4.0)));
                set(f, fo + m3, (s5.0.wrapping_sub(s4.1), s5.1.wrapping_add(s4.0)));
                fo += 1;
            }
        }
    }
}

fn bfly3(f: &mut [i32], base: usize, fstride: usize, m: usize, n: usize, mm: usize) {
    let m2 = 2 * m;
    let epi3i = -28378;
    for i in 0..n {
        let mut fo = base + i * mm;
        let (mut t1, mut t2) = (0usize, 0usize);
        for _ in 0..m {
            let s1 = cmul(get(f, fo + m), tw(t1));
            let s2 = cmul(get(f, fo + m2), tw(t2));
            let s3 = cadd(s1, s2);
            let mut s0 = csub(s1, s2);
            t1 += fstride; t2 += fstride * 2;
            let f0 = get(f, fo);
            let mut fm = (f0.0.wrapping_sub(s3.0 >> 1), f0.1.wrapping_sub(s3.1 >> 1));
            s0 = (s_mul(s0.0, epi3i), s_mul(s0.1, epi3i));
            set(f, fo, cadd(f0, s3));
            set(f, fo + m2, (fm.0.wrapping_add(s0.1), fm.1.wrapping_sub(s0.0)));
            fm = (fm.0.wrapping_sub(s0.1), fm.1.wrapping_add(s0.0));
            set(f, fo + m, fm);
            fo += 1;
        }
    }
}

fn bfly5(f: &mut [i32], base: usize, fstride: usize, m: usize, n: usize, mm: usize) {
    let ya = (10126, -31164);
    let yb = (-26510, -19261);
    for i in 0..n {
        let fo = base + i * mm;
        for u in 0..m {
            let (p0, p1, p2, p3, p4) = (fo + u, fo + m + u, fo + 2 * m + u, fo + 3 * m + u, fo + 4 * m + u);
            let s0 = get(f, p0);
            let s1 = cmul(get(f, p1), tw(u * fstride));
            let s2 = cmul(get(f, p2), tw(2 * u * fstride));
            let s3 = cmul(get(f, p3), tw(3 * u * fstride));
            let s4 = cmul(get(f, p4), tw(4 * u * fstride));
            let s7 = cadd(s1, s4);
            let s10 = csub(s1, s4);
            let s8 = cadd(s2, s3);
            let s9 = csub(s2, s3);
            set(f, p0, (s0.0.wrapping_add(s7.0.wrapping_add(s8.0)), s0.1.wrapping_add(s7.1.wrapping_add(s8.1))));
            let s5 = (s0.0.wrapping_add(s_mul(s7.0, ya.0).wrapping_add(s_mul(s8.0, yb.0))), s0.1.wrapping_add(s_mul(s7.1, ya.0).wrapping_add(s_mul(s8.1, yb.0))));
            let s6 = (s_mul(s10.1, ya.1).wrapping_add(s_mul(s9.1, yb.1)), s_mul(s10.0, ya.1).wrapping_add(s_mul(s9.0, yb.1)).wrapping_neg());
            set(f, p1, csub(s5, s6));
            set(f, p4, cadd(s5, s6));
            let s11 = (s0.0.wrapping_add(s_mul(s7.0, yb.0).wrapping_add(s_mul(s8.0, ya.0))), s0.1.wrapping_add(s_mul(s7.1, yb.0).wrapping_add(s_mul(s8.1, ya.0))));
            let s12 = (s_mul(s9.1, ya.1).wrapping_sub(s_mul(s10.1, yb.1)), s_mul(s10.0, yb.1).wrapping_sub(s_mul(s9.0, ya.1)));
            set(f, p2, cadd(s11, s12));
            set(f, p3, csub(s11, s12));
        }
    }
}

/// `opus_fft_impl` on interleaved (re, im) `i32` pairs starting at `base` (in complex units of `f`).
pub fn fft_impl(st: &FftState, f: &mut [i32]) {
    let shift = if st.shift > 0 { st.shift as usize } else { 0 };
    let mut fstride = [0usize; 9];
    fstride[0] = 1;
    let mut l = 0usize;
    loop {
        let p = st.factors[2 * l] as usize;
        let m = st.factors[2 * l + 1] as usize;
        fstride[l + 1] = fstride[l] * p;
        l += 1;
        if m == 1 { break; }
    }
    let mut m = st.factors[2 * l - 1] as usize;
    for i in (0..l).rev() {
        let m2 = if i != 0 { st.factors[2 * i - 1] as usize } else { 1 };
        match st.factors[2 * i] {
            2 => bfly2(f, 0, fstride[i]),
            4 => bfly4(f, 0, fstride[i] << shift, m, fstride[i], m2),
            3 => bfly3(f, 0, fstride[i] << shift, m, fstride[i], m2),
            5 => bfly5(f, 0, fstride[i] << shift, m, fstride[i], m2),
            _ => {}
        }
        m = m2;
    }
}

/// `clt_mdct_backward`: `input` is read with `stride` starting at `in_off`; `out` receives N/2+overlap/2
/// samples (the first `overlap` of which are TDAC-mixed with what `out` already holds).
pub fn mdct_backward(input: &[i32], in_off: usize, out: &mut [i32], window: &[i16], overlap: usize, shift: usize, stride: usize) {
    let mut n = 1920usize;
    let mut trig_off = 0usize;
    for _ in 0..shift { n >>= 1; trig_off += n; }
    let trig = &MDCT_TWIDDLES960[trig_off..];
    let n2 = n >> 1;
    let n4 = n >> 2;
    let st = &FFT_STATES[shift];
    let ho = overlap >> 1;
    {
        let bitrev = st.bitrev;
        let mut xp1 = in_off;
        let mut xp2 = in_off + stride * (n2 - 1);
        for i in 0..n4 {
            let rev = bitrev[i] as usize;
            let (a1, a2) = (input[xp1], input[xp2]);
            let yr = s_mul(a2, trig[i] as i32).wrapping_add(s_mul(a1, trig[n4 + i] as i32));
            let yi = s_mul(a1, trig[i] as i32).wrapping_sub(s_mul(a2, trig[n4 + i] as i32));
            out[ho + 2 * rev + 1] = yr;
            out[ho + 2 * rev] = yi;
            xp1 += 2 * stride;
            xp2 = xp2.wrapping_sub(2 * stride);
        }
    }
    fft_impl(st, &mut out[ho..ho + n2]);
    {
        let mut yp0 = ho;
        let mut yp1 = ho + n2 - 2;
        for i in 0..(n4 + 1) >> 1 {
            let (re, im) = (out[yp0 + 1], out[yp0]);
            let (t0, t1) = (trig[i] as i32, trig[n4 + i] as i32);
            let yr = s_mul(re, t0).wrapping_add(s_mul(im, t1));
            let yi = s_mul(re, t1).wrapping_sub(s_mul(im, t0));
            let (re, im) = (out[yp1 + 1], out[yp1]);
            out[yp0] = yr;
            out[yp1 + 1] = yi;
            let (t0, t1) = (trig[n4 - i - 1] as i32, trig[n2 - i - 1] as i32);
            let yr = s_mul(re, t0).wrapping_add(s_mul(im, t1));
            let yi = s_mul(re, t1).wrapping_sub(s_mul(im, t0));
            out[yp1] = yr;
            out[yp0 + 1] = yi;
            yp0 += 2;
            yp1 = yp1.wrapping_sub(2);
        }
    }
    {
        for i in 0..overlap / 2 {
            let xp1 = overlap - 1 - i;
            let yp1 = i;
            let (x1, x2) = (out[xp1], out[yp1]);
            let wp1 = window[i] as i32;
            let wp2 = window[overlap - 1 - i] as i32;
            out[yp1] = mult16_32_q15(wp2, x2).wrapping_sub(mult16_32_q15(wp1, x1));
            out[xp1] = mult16_32_q15(wp1, x2).wrapping_add(mult16_32_q15(wp2, x1));
        }
    }
}
