//! CELT shape decoding, RFC 6716 §4.3.4–§4.3.6 (`celt/bands.c`, `celt/vq.c`, `celt/cwrs.c`): PVQ codeword
//! decoding, the spreading rotation, recursive band splitting with the theta angle, stereo (mid/side,
//! intensity, dual), time/frequency changes (Haar, Hadamard interleave), folding, anti-collapse and the
//! denormalisation back to MDCT coefficients.
//!
//! All the C pointers into the band buffer, the folding buffer and the scratch are offsets into ONE
//! working vector `w` here, so the aliasing the reference relies on is preserved exactly.
use super::super::range::{ilog, RangeDecoder, BITRES};
use super::energy::E_MEANS;
use super::fixed::*;
use super::rate::*;
use super::tables::*;
use alloc::vec;
use alloc::vec::Vec;

pub const SPREAD_NONE: i32 = 0;
pub const SPREAD_AGGRESSIVE: i32 = 3;
const NORM_SCALING: i32 = 16384;

#[inline] pub fn lcg_rand(seed: u32) -> u32 { seed.wrapping_mul(1664525).wrapping_add(1013904223) }
#[inline] fn frac_mul16(a: i32, b: i32) -> i32 { (16384 + (a as i16 as i32) * (b as i16 as i32)) >> 15 }

fn bitexact_cos(x: i32) -> i32 {
    let tmp = (4096 + x * x) >> 13;
    let x2 = tmp as i16 as i32;
    let x2 = ((32767 - x2) + frac_mul16(x2, -7651 + frac_mul16(x2, 8277 + frac_mul16(-626, x2)))) as i16 as i32;
    1 + x2
}
fn bitexact_log2tan(isin: i32, icos: i32) -> i32 {
    let lc = ilog(icos as u32);
    let ls = ilog(isin as u32);
    let icos = icos << (15 - lc);
    let isin = isin << (15 - ls);
    (ls - lc) * (1 << 11) + frac_mul16(isin, frac_mul16(isin, -2597) + 7932) - frac_mul16(icos, frac_mul16(icos, -2597) + 7932)
}

// ---- cwrs.c (the small-footprint row recurrence; same integers as the table form) ----
fn unext(u: &mut [u32], len: usize, mut ui0: u32) {
    let mut j = 1;
    loop {
        let ui1 = u[j].wrapping_add(u[j - 1]).wrapping_add(ui0);
        u[j - 1] = ui0;
        ui0 = ui1;
        j += 1;
        if j >= len { break; }
    }
    u[j - 1] = ui0;
}
fn uprev(u: &mut [u32], n: usize, mut ui0: u32) {
    let mut j = 1;
    loop {
        let ui1 = u[j].wrapping_sub(u[j - 1]).wrapping_sub(ui0);
        u[j - 1] = ui0;
        ui0 = ui1;
        j += 1;
        if j >= n { break; }
    }
    u[j - 1] = ui0;
}
fn ncwrs_urow(n: usize, k: usize, u: &mut [u32]) -> u32 {
    let len = k + 2;
    u[0] = 0;
    u[1] = 1;
    for kk in 2..len { u[kk] = ((kk as u32) << 1) - 1; }
    for _ in 2..n { unext(&mut u[1..], k + 1, 1); }
    u[k].wrapping_add(u[k + 1])
}
fn cwrsi(n: usize, mut k: usize, mut i: u32, y: &mut [i32], u: &mut [u32]) -> i32 {
    let mut yy = 0i32;
    for j in 0..n {
        let mut p = u[k + 1];
        let s: i32 = -((i >= p) as i32);
        i = i.wrapping_sub(p & s as u32);
        let mut yj = k as i32;
        p = u[k];
        while p > i { k -= 1; p = u[k]; }
        i -= p;
        yj -= k as i32;
        let val = ((yj + s) ^ s) as i16 as i32;
        y[j] = val;
        yy = mac16_16(yy, val, val);
        uprev(u, k + 2, 0);
    }
    yy
}
fn decode_pulses(y: &mut [i32], n: usize, k: usize, dec: &mut RangeDecoder) -> i32 {
    let mut u = vec![0u32; k + 2];
    let v = ncwrs_urow(n, k, &mut u);
    let i = dec.dec_uint(v);
    cwrsi(n, k, i, y, &mut u)
}

// ---- vq.c ----
fn exp_rotation1(x: &mut [i16], len: usize, stride: usize, c: i32, s: i32) {
    let ms = -s;
    if len > stride {
        for i in 0..len - stride {
            let x1 = x[i] as i32;
            let x2 = x[i + stride] as i32;
            x[i + stride] = pshr32(mac16_16(mult16_16(c, x2), s, x1), 15) as i16;
            x[i] = pshr32(mac16_16(mult16_16(c, x1), ms, x2), 15) as i16;
        }
    }
    let start = len as isize - 2 * stride as isize - 1;
    let mut i = start;
    while i >= 0 {
        let iu = i as usize;
        let x1 = x[iu] as i32;
        let x2 = x[iu + stride] as i32;
        x[iu + stride] = pshr32(mac16_16(mult16_16(c, x2), s, x1), 15) as i16;
        x[iu] = pshr32(mac16_16(mult16_16(c, x1), ms, x2), 15) as i16;
        i -= 1;
    }
}
fn exp_rotation(x: &mut [i16], len: usize, dir: i32, stride: usize, k: i32, spread: i32) {
    const SPREAD_FACTOR: [i32; 3] = [15, 10, 5];
    if 2 * k >= len as i32 || spread == SPREAD_NONE { return; }
    let factor = SPREAD_FACTOR[(spread - 1) as usize];
    let gain = celt_div(mult16_16(Q15ONE, len as i32), len as i32 + factor * k) as i16 as i32;
    let theta = (mult16_16_q15(gain, gain) >> 1) as i16 as i32;
    let c = celt_cos_norm(theta);
    let s = celt_cos_norm(sub16(Q15ONE, theta));
    let mut stride2 = 0usize;
    if len >= 8 * stride {
        stride2 = 1;
        while (stride2 * stride2 + stride2) * stride + (stride >> 2) < len { stride2 += 1; }
    }
    let l = len / stride;
    for i in 0..stride {
        let seg = &mut x[i * l..i * l + l];
        if dir < 0 {
            if stride2 != 0 { exp_rotation1(seg, l, stride2, s, c); }
            exp_rotation1(seg, l, 1, c, s);
        } else {
            exp_rotation1(seg, l, 1, c, -s);
            if stride2 != 0 { exp_rotation1(seg, l, stride2, s, -c); }
        }
    }
}
fn normalise_residual(iy: &[i32], x: &mut [i16], n: usize, ryy: i32, gain: i32) {
    let k = celt_ilog2(ryy) >> 1;
    let t = vshr32(ryy, 2 * (k - 7));
    let g = mult16_16_p15(celt_rsqrt_norm(t), gain);
    for i in 0..n { x[i] = pshr32(mult16_16(g, iy[i]), k + 1) as i16; }
}
fn extract_collapse_mask(iy: &[i32], n: usize, b: usize) -> u32 {
    if b <= 1 { return 1; }
    let n0 = n / b;
    let mut mask = 0u32;
    for i in 0..b {
        let mut tmp = 0i32;
        for j in 0..n0 { tmp |= iy[i * n0 + j]; }
        mask |= ((tmp != 0) as u32) << i;
    }
    mask
}
fn alg_unquant(x: &mut [i16], n: usize, k: i32, spread: i32, b: usize, dec: &mut RangeDecoder, gain: i32) -> u32 {
    let mut iy = vec![0i32; n];
    let ryy = decode_pulses(&mut iy, n, k as usize, dec);
    normalise_residual(&iy, x, n, ryy, gain);
    exp_rotation(x, n, -1, b, k, spread);
    extract_collapse_mask(&iy, n, b)
}
pub fn inner_prod(x: &[i16], y: &[i16], n: usize) -> i32 {
    let mut s = 0i32;
    for i in 0..n { s = mac16_16(s, x[i] as i32, y[i] as i32); }
    s
}
pub fn renormalise_vector(x: &mut [i16], n: usize, gain: i32) {
    let e = 1i32.wrapping_add(inner_prod(x, x, n));
    let k = celt_ilog2(e) >> 1;
    let t = vshr32(e, 2 * (k - 7));
    let g = mult16_16_p15(celt_rsqrt_norm(t), gain);
    for i in 0..n { x[i] = pshr32(mult16_16(g, x[i] as i32), k + 1) as i16; }
}

// ---- bands.c ----
#[allow(clippy::too_many_arguments)]
pub fn denormalise_bands(x: &[i16], freq: &mut [i32], band_log_e: &[i16], start: usize, end: usize, m: i32, downsample: i32, silence: bool) {
    let n = (m * SHORT_MDCT_SIZE as i32) as usize;
    let mut bound = (m * eb(end)) as usize;
    if downsample != 1 { bound = bound.min(n / downsample as usize); }
    let (mut start, mut end) = (start, end);
    if silence { bound = 0; start = 0; end = 0; }
    let mut f = 0usize;
    let mut xi = (m * eb(start)) as usize;
    for _ in 0..(m * eb(start)) as usize { freq[f] = 0; f += 1; }
    for i in start..end {
        let mut j = m * eb(i);
        let band_end = m * eb(i + 1);
        let lg = sat16((band_log_e[i] as i32).wrapping_add(shl32(E_MEANS[i] as i32, 6)));
        let mut shift = 16 - (lg >> DB_SHIFT);
        let g;
        if shift > 31 {
            shift = 0;
            g = 0;
        } else {
            g = {
                let fr = shl16(lg & ((1 << DB_SHIFT) - 1), 4);
                add16(16383, mult16_16_q15(fr, add16(22804, mult16_16_q15(fr, add16(14819, mult16_16_q15(10204, fr))))))
            };
        }
        if shift < 0 {
            let (g, shift) = if shift <= -2 { (16384, -2) } else { (g, shift) };
            loop {
                freq[f] = shl32(mult16_16(x[xi] as i32, g), -shift);
                f += 1; xi += 1; j += 1;
                if j >= band_end { break; }
            }
        } else {
            loop {
                freq[f] = mult16_16(x[xi] as i32, g) >> shift;
                f += 1; xi += 1; j += 1;
                if j >= band_end { break; }
            }
        }
    }
    for v in freq[bound..n].iter_mut() { *v = 0; }
}

#[allow(clippy::too_many_arguments)]
pub fn anti_collapse(x_: &mut [i16], collapse_masks: &[u8], lm: i32, c: usize, size: usize, start: usize, end: usize,
    log_e: &[i16], prev1: &[i16], prev2: &[i16], pulses: &[i32], mut seed: u32) {
    for i in start..end {
        let n0 = eb(i + 1) - eb(i);
        let depth = (((1 + pulses[i]) as u32 / n0 as u32) >> lm) as i32;
        let thresh32 = celt_exp2(-shl16(depth, 10 - BITRES)) >> 1;
        let thresh = mult16_32_q15(16384, thresh32.min(32767));
        let shift;
        let sqrt_1;
        {
            let t = n0 << lm;
            shift = celt_ilog2(t) >> 1;
            let t = shl32(t, (7 - shift) << 1);
            sqrt_1 = celt_rsqrt_norm(t);
        }
        for ch in 0..c {
            let mut p1 = prev1[ch * NB_EBANDS + i] as i32;
            let mut p2 = prev2[ch * NB_EBANDS + i] as i32;
            if c == 1 {
                p1 = p1.max(prev1[NB_EBANDS + i] as i32);
                p2 = p2.max(prev2[NB_EBANDS + i] as i32);
            }
            let ediff = (log_e[ch * NB_EBANDS + i] as i32 - p1.min(p2)).max(0);
            let mut r: i32;
            if ediff < 16384 {
                let r32 = celt_exp2(-(ediff as i16 as i32)) >> 1;
                r = (2 * 16383.min(r32)) as i16 as i32;
            } else {
                r = 0;
            }
            if lm == 3 { r = mult16_16_q14(23170, 23169.min(r)) as i16 as i32; }
            r = (thresh.min(r) >> 1) as i16 as i32;
            r = (mult16_16_q15(sqrt_1, r) >> shift) as i16 as i32;
            let xo = ch * size + (eb(i) << lm) as usize;
            let mut renorm = false;
            for k in 0..(1usize << lm) {
                if collapse_masks[i * c + ch] as u32 & (1 << k) == 0 {
                    for j in 0..n0 as usize {
                        seed = lcg_rand(seed);
                        x_[xo + (j << lm) + k] = (if seed & 0x8000 != 0 { r } else { -r }) as i16;
                    }
                    renorm = true;
                }
            }
            if renorm {
                let len = (n0 << lm) as usize;
                renormalise_vector(&mut x_[xo..xo + len], len, Q15ONE);
            }
        }
    }
}

fn stereo_merge(w: &mut [i16], x: usize, y: usize, mid: i32, n: usize) {
    let mut xp = 0i32;
    let mut side = 0i32;
    for i in 0..n {
        xp = mac16_16(xp, w[y + i] as i32, w[x + i] as i32);
        side = mac16_16(side, w[y + i] as i32, w[y + i] as i32);
    }
    xp = mult16_32_q15(mid, xp);
    let mid2 = mid >> 1;
    let el = mult16_16(mid2, mid2).wrapping_add(side).wrapping_sub(2 * xp);
    let er = mult16_16(mid2, mid2).wrapping_add(side).wrapping_add(2 * xp);
    if er < 161061 || el < 161061 {
        for i in 0..n { w[y + i] = w[x + i]; }
        return;
    }
    let mut kl = celt_ilog2(el) >> 1;
    let mut kr = celt_ilog2(er) >> 1;
    let lgain = celt_rsqrt_norm(vshr32(el, (kl - 7) << 1));
    let rgain = celt_rsqrt_norm(vshr32(er, (kr - 7) << 1));
    if kl < 7 { kl = 7; }
    if kr < 7 { kr = 7; }
    for j in 0..n {
        let l = mult16_16_p15(mid, w[x + j] as i32) as i16 as i32;
        let r = w[y + j] as i32;
        w[x + j] = pshr32(mult16_16(lgain, sub16(l, r)), kl + 1) as i16;
        w[y + j] = pshr32(mult16_16(rgain, add16(l, r)), kr + 1) as i16;
    }
}

static ORDERY_TABLE: [usize; 30] = [1, 0, 3, 0, 2, 1, 7, 0, 4, 3, 6, 1, 5, 2, 15, 0, 8, 7, 12, 3, 11, 4, 14, 1, 9, 6, 13, 2, 10, 5];

fn deinterleave_hadamard(x: &mut [i16], n0: usize, stride: usize, hadamard: bool) {
    let n = n0 * stride;
    let mut tmp = vec![0i16; n];
    if hadamard {
        let ordery = &ORDERY_TABLE[stride - 2..];
        for i in 0..stride { for j in 0..n0 { tmp[ordery[i] * n0 + j] = x[j * stride + i]; } }
    } else {
        for i in 0..stride { for j in 0..n0 { tmp[i * n0 + j] = x[j * stride + i]; } }
    }
    x[..n].copy_from_slice(&tmp);
}
fn interleave_hadamard(x: &mut [i16], n0: usize, stride: usize, hadamard: bool) {
    let n = n0 * stride;
    let mut tmp = vec![0i16; n];
    if hadamard {
        let ordery = &ORDERY_TABLE[stride - 2..];
        for i in 0..stride { for j in 0..n0 { tmp[j * stride + i] = x[ordery[i] * n0 + j]; } }
    } else {
        for i in 0..stride { for j in 0..n0 { tmp[j * stride + i] = x[i * n0 + j]; } }
    }
    x[..n].copy_from_slice(&tmp);
}
pub fn haar1(x: &mut [i16], n0: usize, stride: usize) {
    let n0 = n0 >> 1;
    for i in 0..stride {
        for j in 0..n0 {
            let tmp1 = mult16_16(23170, x[stride * 2 * j + i] as i32);
            let tmp2 = mult16_16(23170, x[stride * (2 * j + 1) + i] as i32);
            x[stride * 2 * j + i] = pshr32(tmp1.wrapping_add(tmp2), 15) as i16;
            x[stride * (2 * j + 1) + i] = pshr32(tmp1.wrapping_sub(tmp2), 15) as i16;
        }
    }
}

fn compute_qn(n: i32, b: i32, offset: i32, pulse_cap: i32, stereo: bool) -> i32 {
    const EXP2_TABLE8: [i32; 8] = [16384, 17866, 19483, 21247, 23170, 25267, 27554, 30048];
    let mut n2 = 2 * n - 1;
    if stereo && n == 2 { n2 -= 1; }
    let mut qb = (b + n2 * offset) / n2;
    qb = qb.min(b - pulse_cap - (4 << BITRES));
    qb = qb.min(8 << BITRES);
    if qb < (1 << BITRES >> 1) { 1 } else {
        let qn = EXP2_TABLE8[(qb & 7) as usize] >> (14 - (qb >> BITRES));
        ((qn + 1) >> 1) << 1
    }
}

struct Ctx {
    i: usize,
    intensity: i32,
    spread: i32,
    tf_change: i32,
    remaining_bits: i32,
    seed: u32,
    disable_inv: bool,
}
struct Split { inv: bool, imid: i32, iside: i32, delta: i32, itheta: i32, qalloc: i32 }

#[allow(clippy::too_many_arguments)]
fn compute_theta(ctx: &mut Ctx, ec: &mut RangeDecoder, n: i32, b: &mut i32, bb: i32, b0: i32, lm: i32, stereo: bool, fill: &mut i32) -> Split {
    let i = ctx.i;
    let pulse_cap = LOGN400[i] as i32 + lm * (1 << BITRES);
    let offset = (pulse_cap >> 1) - if stereo && n == 2 { QTHETA_OFFSET_TWOPHASE } else { QTHETA_OFFSET };
    let mut qn = compute_qn(n, *b, offset, pulse_cap, stereo);
    if stereo && i as i32 >= ctx.intensity { qn = 1; }
    let tell = ec.tell_frac() as i32;
    let mut itheta = 0i32;
    let mut inv = false;
    if qn != 1 {
        if stereo && n > 2 {
            let p0 = 3;
            let x0 = qn / 2;
            let ft = p0 * (x0 + 1) + x0;
            let fs = ec.decode(ft as u32) as i32;
            let x = if fs < (x0 + 1) * p0 { fs / p0 } else { x0 + 1 + (fs - (x0 + 1) * p0) };
            let fl = if x <= x0 { p0 * x } else { (x - 1 - x0) + (x0 + 1) * p0 };
            let fh = if x <= x0 { p0 * (x + 1) } else { (x - x0) + (x0 + 1) * p0 };
            ec.update(fl as u32, fh as u32, ft as u32);
            itheta = x;
        } else if b0 > 1 || stereo {
            itheta = ec.dec_uint((qn + 1) as u32) as i32;
        } else {
            let ft = ((qn >> 1) + 1) * ((qn >> 1) + 1);
            let fm = ec.decode(ft as u32) as i32;
            let (fs, fl);
            if fm < ((qn >> 1) * ((qn >> 1) + 1) >> 1) {
                itheta = ((isqrt32(8 * fm as u32 + 1) - 1) >> 1) as i32;
                fs = itheta + 1;
                fl = (itheta * (itheta + 1)) >> 1;
            } else {
                itheta = ((2 * (qn + 1)) as u32 - isqrt32(8 * (ft - fm - 1) as u32 + 1)) as i32 >> 1;
                fs = qn + 1 - itheta;
                fl = ft - (((qn + 1 - itheta) * (qn + 2 - itheta)) >> 1);
            }
            ec.update(fl as u32, (fl + fs) as u32, ft as u32);
        }
        itheta = ((itheta * 16384) as u32 / qn as u32) as i32;
    } else if stereo {
        if *b > 2 << BITRES && ctx.remaining_bits > 2 << BITRES {
            inv = ec.bit_logp(2);
        } else {
            inv = false;
        }
        if ctx.disable_inv { inv = false; }
        itheta = 0;
    }
    let qalloc = ec.tell_frac() as i32 - tell;
    *b -= qalloc;
    let (imid, iside, delta);
    if itheta == 0 {
        imid = 32767; iside = 0;
        *fill &= (1 << bb) - 1;
        delta = -16384;
    } else if itheta == 16384 {
        imid = 0; iside = 32767;
        *fill &= ((1 << bb) - 1) << bb;
        delta = 16384;
    } else {
        imid = bitexact_cos(itheta);
        iside = bitexact_cos(16384 - itheta);
        delta = frac_mul16((n - 1) << 7, bitexact_log2tan(iside, imid));
    }
    Split { inv, imid, iside, delta, itheta, qalloc }
}

fn quant_band_n1(ctx: &mut Ctx, ec: &mut RangeDecoder, w: &mut [i16], x: usize, y: Option<usize>, lowband_out: Option<usize>) -> u32 {
    let mut cur = x;
    let stereo = y.is_some();
    for c in 0..1 + stereo as usize {
        let mut sign = 0;
        if ctx.remaining_bits >= 1 << BITRES {
            sign = ec.bits(1);
            ctx.remaining_bits -= 1 << BITRES;
        }
        w[cur] = (if sign != 0 { -NORM_SCALING } else { NORM_SCALING }) as i16;
        if c == 0 { if let Some(yy) = y { cur = yy; } }
    }
    if let Some(lo) = lowband_out { w[lo] = w[x] >> 4; }
    1
}

#[allow(clippy::too_many_arguments)]
fn quant_partition(ctx: &mut Ctx, ec: &mut RangeDecoder, w: &mut [i16], x: usize, n: i32, b: i32, bb: i32, lowband: Option<usize>, lm: i32, gain: i32, fill: i32) -> u32 {
    let i = ctx.i;
    let cache = cache_slice(i, lm);
    let b0 = bb;
    let mut cm: u32;
    if lm != -1 && b > cache[cache[0] as usize] as i32 + 12 && n > 2 {
        let n = n >> 1;
        let y = x + n as usize;
        let lm = lm - 1;
        let mut fill = fill;
        if bb == 1 { fill = (fill & 1) | (fill << 1); }
        let bb = (bb + 1) >> 1;
        let mut b = b;
        let sc = compute_theta(ctx, ec, n, &mut b, bb, b0, lm, false, &mut fill);
        let (mid, side) = (sc.imid, sc.iside);
        let mut delta = sc.delta;
        let itheta = sc.itheta;
        if b0 > 1 && (itheta & 0x3fff) != 0 {
            if itheta > 8192 { delta -= delta >> (4 - lm); } else { delta = 0.min(delta + ((n << BITRES) >> (5 - lm))); }
        }
        let mut mbits = 0.max(b.min((b - delta) / 2));
        let mut sbits = b - mbits;
        ctx.remaining_bits -= sc.qalloc;
        let next_lowband2 = lowband.map(|l| l + n as usize);
        let mut rebalance = ctx.remaining_bits;
        if mbits >= sbits {
            cm = quant_partition(ctx, ec, w, x, n, mbits, bb, lowband, lm, mult16_16_p15(gain, mid), fill);
            rebalance = mbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && itheta != 0 { sbits += rebalance - (3 << BITRES); }
            cm |= quant_partition(ctx, ec, w, y, n, sbits, bb, next_lowband2, lm, mult16_16_p15(gain, side), fill >> bb) << (b0 >> 1);
        } else {
            cm = quant_partition(ctx, ec, w, y, n, sbits, bb, next_lowband2, lm, mult16_16_p15(gain, side), fill >> bb) << (b0 >> 1);
            rebalance = sbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && itheta != 16384 { mbits += rebalance - (3 << BITRES); }
            cm |= quant_partition(ctx, ec, w, x, n, mbits, bb, lowband, lm, mult16_16_p15(gain, mid), fill);
        }
    } else {
        let mut q = bits2pulses(i, lm, b);
        let mut curr_bits = pulses2bits(i, lm, q);
        ctx.remaining_bits -= curr_bits;
        while ctx.remaining_bits < 0 && q > 0 {
            ctx.remaining_bits += curr_bits;
            q -= 1;
            curr_bits = pulses2bits(i, lm, q);
            ctx.remaining_bits -= curr_bits;
        }
        let nn = n as usize;
        if q != 0 {
            let k = get_pulses(q);
            cm = alg_unquant(&mut w[x..x + nn], nn, k, ctx.spread, bb as usize, ec, gain);
        } else {
            let cm_mask = ((1u64 << bb) - 1) as u32;
            let fill = fill as u32 & cm_mask;
            cm = 0;
            if fill == 0 {
                for v in w[x..x + nn].iter_mut() { *v = 0; }
            } else {
                match lowband {
                    None => {
                        for j in 0..nn {
                            ctx.seed = lcg_rand(ctx.seed);
                            w[x + j] = ((ctx.seed as i32) >> 20) as i16;
                        }
                        cm = cm_mask;
                    }
                    Some(lb) => {
                        for j in 0..nn {
                            ctx.seed = lcg_rand(ctx.seed);
                            let tmp = if ctx.seed & 0x8000 != 0 { 4 } else { -4 };
                            w[x + j] = (w[lb + j] as i32 + tmp) as i16;
                        }
                        cm = fill;
                    }
                }
                renormalise_vector(&mut w[x..x + nn], nn, gain);
            }
        }
    }
    cm
}

#[allow(clippy::too_many_arguments)]
fn quant_band(ctx: &mut Ctx, ec: &mut RangeDecoder, w: &mut [i16], x: usize, n: i32, b: i32, bb: i32, lowband: Option<usize>, lm: i32,
    lowband_out: Option<usize>, gain: i32, lowband_scratch: Option<usize>, fill: i32) -> u32 {
    let n0 = n;
    let mut n_b = n;
    let mut b0 = bb;
    let mut bb = bb;
    let mut time_divide = 0;
    let mut recombine = 0;
    let long_blocks = b0 == 1;
    let mut tf_change = ctx.tf_change;
    let mut fill = fill;
    let mut lowband = lowband;
    n_b = (n_b as u32 / bb as u32) as i32;
    if n == 1 { return quant_band_n1(ctx, ec, w, x, None, lowband_out); }
    if tf_change > 0 { recombine = tf_change; }
    if let (Some(ls), Some(lb)) = (lowband_scratch, lowband) {
        if recombine != 0 || ((n_b & 1) == 0 && tf_change < 0) || b0 > 1 {
            w.copy_within(lb..lb + n as usize, ls);
            lowband = Some(ls);
        }
    }
    const BIT_INTERLEAVE_TABLE: [i32; 16] = [0, 1, 1, 1, 2, 3, 3, 3, 2, 3, 3, 3, 2, 3, 3, 3];
    for k in 0..recombine {
        if let Some(lb) = lowband { haar1(&mut w[lb..], (n >> k) as usize, 1 << k); }
        fill = BIT_INTERLEAVE_TABLE[(fill & 0xF) as usize] | (BIT_INTERLEAVE_TABLE[(fill >> 4) as usize] << 2);
    }
    bb >>= recombine;
    n_b <<= recombine;
    while (n_b & 1) == 0 && tf_change < 0 {
        if let Some(lb) = lowband { haar1(&mut w[lb..], n_b as usize, bb as usize); }
        fill |= fill << bb;
        bb <<= 1;
        n_b >>= 1;
        time_divide += 1;
        tf_change += 1;
    }
    b0 = bb;
    let n_b0 = n_b;
    if b0 > 1 {
        if let Some(lb) = lowband { deinterleave_hadamard(&mut w[lb..], (n_b >> recombine) as usize, (b0 << recombine) as usize, long_blocks); }
    }
    let mut cm = quant_partition(ctx, ec, w, x, n, b, bb, lowband, lm, gain, fill);
    if b0 > 1 { interleave_hadamard(&mut w[x..], (n_b >> recombine) as usize, (b0 << recombine) as usize, long_blocks); }
    n_b = n_b0;
    bb = b0;
    for _ in 0..time_divide {
        bb >>= 1;
        n_b <<= 1;
        cm |= cm >> bb;
        haar1(&mut w[x..], n_b as usize, bb as usize);
    }
    const BIT_DEINTERLEAVE_TABLE: [u32; 16] = [0x00, 0x03, 0x0C, 0x0F, 0x30, 0x33, 0x3C, 0x3F, 0xC0, 0xC3, 0xCC, 0xCF, 0xF0, 0xF3, 0xFC, 0xFF];
    for k in 0..recombine {
        cm = BIT_DEINTERLEAVE_TABLE[cm as usize];
        haar1(&mut w[x..], (n0 >> k) as usize, 1 << k);
    }
    bb <<= recombine;
    if let Some(lo) = lowband_out {
        let nn = celt_sqrt(shl32(n0, 22));
        for j in 0..n0 as usize { w[lo + j] = mult16_16_q15(nn, w[x + j] as i32) as i16; }
    }
    cm &= ((1u64 << bb) - 1) as u32;
    cm
}

#[allow(clippy::too_many_arguments)]
fn quant_band_stereo(ctx: &mut Ctx, ec: &mut RangeDecoder, w: &mut [i16], x: usize, y: usize, n: i32, b: i32, bb: i32, lowband: Option<usize>, lm: i32,
    lowband_out: Option<usize>, lowband_scratch: Option<usize>, fill: i32) -> u32 {
    if n == 1 { return quant_band_n1(ctx, ec, w, x, Some(y), lowband_out); }
    let orig_fill = fill;
    let mut fill = fill;
    let mut b = b;
    let sc = compute_theta(ctx, ec, n, &mut b, bb, bb, lm, true, &mut fill);
    let (mid, side, delta, itheta, inv) = (sc.imid, sc.iside, sc.delta, sc.itheta, sc.inv);
    let mut cm: u32;
    if n == 2 {
        let mut mbits = b;
        let mut sbits = 0;
        if itheta != 0 && itheta != 16384 { sbits = 1 << BITRES; }
        mbits -= sbits;
        let c = itheta > 8192;
        ctx.remaining_bits -= sc.qalloc + sbits;
        let (x2, y2) = if c { (y, x) } else { (x, y) };
        let mut sign = 0i32;
        if sbits != 0 { sign = ec.bits(1) as i32; }
        sign = 1 - 2 * sign;
        cm = quant_band(ctx, ec, w, x2, n, mbits, bb, lowband, lm, lowband_out, Q15ONE, lowband_scratch, orig_fill);
        w[y2] = (-sign * w[x2 + 1] as i32) as i16;
        w[y2 + 1] = (sign * w[x2] as i32) as i16;
        w[x] = mult16_16_q15(mid, w[x] as i32) as i16;
        w[x + 1] = mult16_16_q15(mid, w[x + 1] as i32) as i16;
        w[y] = mult16_16_q15(side, w[y] as i32) as i16;
        w[y + 1] = mult16_16_q15(side, w[y + 1] as i32) as i16;
        let tmp = w[x] as i32;
        w[x] = sub16(tmp, w[y] as i32) as i16;
        w[y] = add16(tmp, w[y] as i32) as i16;
        let tmp = w[x + 1] as i32;
        w[x + 1] = sub16(tmp, w[y + 1] as i32) as i16;
        w[y + 1] = add16(tmp, w[y + 1] as i32) as i16;
    } else {
        let mut mbits = 0.max(b.min((b - delta) / 2));
        let mut sbits = b - mbits;
        ctx.remaining_bits -= sc.qalloc;
        let mut rebalance = ctx.remaining_bits;
        if mbits >= sbits {
            cm = quant_band(ctx, ec, w, x, n, mbits, bb, lowband, lm, lowband_out, Q15ONE, lowband_scratch, fill);
            rebalance = mbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && itheta != 0 { sbits += rebalance - (3 << BITRES); }
            cm |= quant_band(ctx, ec, w, y, n, sbits, bb, None, lm, None, side, None, fill >> bb);
        } else {
            cm = quant_band(ctx, ec, w, y, n, sbits, bb, None, lm, None, side, None, fill >> bb);
            rebalance = sbits - (rebalance - ctx.remaining_bits);
            if rebalance > 3 << BITRES && itheta != 16384 { mbits += rebalance - (3 << BITRES); }
            cm |= quant_band(ctx, ec, w, x, n, mbits, bb, lowband, lm, lowband_out, Q15ONE, lowband_scratch, fill);
        }
    }
    if n != 2 { stereo_merge(w, x, y, mid, n as usize); }
    if inv { for j in 0..n as usize { w[y + j] = (-(w[y + j] as i32)) as i16; } }
    cm
}

/// `quant_all_bands` (decode side). `x_` holds C*N normalised coefficients (Y at `n` when stereo).
#[allow(clippy::too_many_arguments)]
pub fn quant_all_bands(start: usize, end: usize, x_: &mut [i16], c: usize, collapse_masks: &mut [u8], pulses: &[i32], short_blocks: bool,
    spread: i32, mut dual_stereo: i32, intensity: i32, tf_res: &[i32], total_bits: i32, mut balance: i32, ec: &mut RangeDecoder, lm: i32,
    coded_bands: i32, seed: &mut u32, disable_inv: bool) {
    let m = 1i32 << lm;
    let n_total = (m * SHORT_MDCT_SIZE as i32) as usize;
    let bb = if short_blocks { m } else { 1 };
    let norm_offset = (m * eb(start)) as usize;
    let norm_len = (m * eb(NB_EBANDS - 1)) as usize - norm_offset;
    // w = [X (C*N) | norm (norm_len) | norm2 (norm_len)]
    let mut w: Vec<i16> = vec![0; c * n_total + 2 * norm_len];
    w[..c * n_total].copy_from_slice(&x_[..c * n_total]);
    let norm = c * n_total;
    let norm2 = norm + norm_len;
    let mut lowband_scratch: Option<usize> = Some((m * eb(NB_EBANDS - 1)) as usize);
    let mut lowband_offset = 0usize;
    let mut update_lowband = true;
    let mut ctx = Ctx { i: 0, intensity, spread, tf_change: 0, remaining_bits: 0, seed: *seed, disable_inv };
    let _ = bb;
    for i in start..end {
        ctx.i = i;
        let last = i == end - 1;
        let x = (m * eb(i)) as usize;
        let y = if c == 2 { Some(n_total + (m * eb(i)) as usize) } else { None };
        let n = m * eb(i + 1) - m * eb(i);
        let tell = ec.tell_frac() as i32;
        if i != start { balance -= tell; }
        let remaining_bits = total_bits - tell - 1;
        ctx.remaining_bits = remaining_bits;
        let b = if i as i32 <= coded_bands - 1 {
            let curr_balance = balance / 3.min(coded_bands - i as i32);
            0.max(16383.min((remaining_bits + 1).min(pulses[i] + curr_balance)))
        } else { 0 };
        if (m * eb(i) - n >= m * eb(start) || i == start + 1) && (update_lowband || lowband_offset == 0) { lowband_offset = i; }
        if i == start + 1 {
            let n1 = (m * (eb(start + 1) - eb(start))) as usize;
            let n2 = (m * (eb(start + 2) - eb(start + 1))) as usize;
            if n2 > n1 {
                w.copy_within(norm + 2 * n1 - n2..norm + n1, norm + n1);
                if dual_stereo != 0 { w.copy_within(norm2 + 2 * n1 - n2..norm2 + n1, norm2 + n1); }
            }
        }
        let tf_change = tf_res[i];
        ctx.tf_change = tf_change;
        if last { lowband_scratch = None; }
        let mut effective_lowband: i32 = -1;
        let (mut x_cm, mut y_cm): (u32, u32);
        if lowband_offset != 0 && (spread != SPREAD_AGGRESSIVE || bb > 1 || tf_change < 0) {
            effective_lowband = 0.max(m * eb(lowband_offset) - norm_offset as i32 - n);
            let mut fold_start = lowband_offset;
            loop { fold_start -= 1; if m * eb(fold_start) <= effective_lowband + norm_offset as i32 { break; } }
            let mut fold_end = lowband_offset - 1;
            loop { fold_end += 1; if !(fold_end < i && m * eb(fold_end) < effective_lowband + norm_offset as i32 + n) { break; } }
            x_cm = 0; y_cm = 0;
            let mut fi = fold_start;
            loop {
                x_cm |= collapse_masks[fi * c] as u32;
                y_cm |= collapse_masks[fi * c + c - 1] as u32;
                fi += 1;
                if fi >= fold_end { break; }
            }
        } else {
            x_cm = ((1u64 << bb) - 1) as u32;
            y_cm = x_cm;
        }
        if dual_stereo != 0 && i as i32 == intensity {
            dual_stereo = 0;
            for j in 0..(m * eb(i)) as usize - norm_offset {
                w[norm + j] = ((w[norm + j] as i32 + w[norm2 + j] as i32) >> 1) as i16;
            }
        }
        let lb = |base: usize| if effective_lowband != -1 { Some(base + effective_lowband as usize) } else { None };
        let lo = |base: usize| if last { None } else { Some(base + (m * eb(i)) as usize - norm_offset) };
        if dual_stereo != 0 {
            x_cm = quant_band(&mut ctx, ec, &mut w, x, n, b / 2, bb, lb(norm), lm, lo(norm), Q15ONE, lowband_scratch, x_cm as i32);
            y_cm = quant_band(&mut ctx, ec, &mut w, y.unwrap(), n, b / 2, bb, lb(norm2), lm, lo(norm2), Q15ONE, lowband_scratch, y_cm as i32);
        } else {
            if let Some(yy) = y {
                x_cm = quant_band_stereo(&mut ctx, ec, &mut w, x, yy, n, b, bb, lb(norm), lm, lo(norm), lowband_scratch, (x_cm | y_cm) as i32);
            } else {
                x_cm = quant_band(&mut ctx, ec, &mut w, x, n, b, bb, lb(norm), lm, lo(norm), Q15ONE, lowband_scratch, (x_cm | y_cm) as i32);
            }
            y_cm = x_cm;
        }
        collapse_masks[i * c] = x_cm as u8;
        collapse_masks[i * c + c - 1] = y_cm as u8;
        balance += pulses[i] + tell;
        update_lowband = b > (n << BITRES);
    }
    *seed = ctx.seed;
    x_[..c * n_total].copy_from_slice(&w[..c * n_total]);
}
