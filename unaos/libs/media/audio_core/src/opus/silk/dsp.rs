//! SILK signal processing shared by the decoder (`silk/NLSF_*.c`, `NLSF2A.c`, `LPC_fit.c`, `bwexpander*.c`,
//! `LPC_inv_pred_gain.c`, `lin2log.c`, `log2lin.c`, `sum_sqr_shift.c`, `LPC_analysis_filter.c`, `sort.c`).
use super::macros::*;
use super::tables::*;

pub fn nlsf_unpack(ec_ix: &mut [i16], pred_q8: &mut [u8], cb: &NlsfCb, cb1_index: usize) {
    let mut sel = cb1_index * cb.order / 2;
    let mut i = 0;
    while i < cb.order {
        let entry = cb.ec_sel[sel] as i32;
        sel += 1;
        ec_ix[i] = smulbb((entry >> 1) & 7, 9) as i16;
        pred_q8[i] = cb.pred_q8[i + (entry & 1) as usize * (cb.order - 1)];
        ec_ix[i + 1] = smulbb((entry >> 5) & 7, 9) as i16;
        pred_q8[i + 1] = cb.pred_q8[i + ((entry >> 4) & 1) as usize * (cb.order - 1) + 1];
        i += 2;
    }
}

fn insertion_sort_increasing_all_values_i16(a: &mut [i16], l: usize) {
    for i in 1..l {
        let value = a[i];
        let mut j = i as isize - 1;
        while j >= 0 && value < a[j as usize] {
            a[j as usize + 1] = a[j as usize];
            j -= 1;
        }
        a[(j + 1) as usize] = value;
    }
}

pub fn nlsf_stabilize(nlsf: &mut [i16], ndelta_min: &[i16], l: usize) {
    let mut loops = 0;
    while loops < 20 {
        let mut min_diff = nlsf[0] as i32 - ndelta_min[0] as i32;
        let mut ii = 0usize;
        for i in 1..l {
            let diff = nlsf[i] as i32 - (nlsf[i - 1] as i32 + ndelta_min[i] as i32);
            if diff < min_diff { min_diff = diff; ii = i; }
        }
        let diff = (1 << 15) - (nlsf[l - 1] as i32 + ndelta_min[l] as i32);
        if diff < min_diff { min_diff = diff; ii = l; }
        if min_diff >= 0 { return; }
        if ii == 0 {
            nlsf[0] = ndelta_min[0];
        } else if ii == l {
            nlsf[l - 1] = ((1 << 15) - ndelta_min[l] as i32) as i16;
        } else {
            let mut min_center = 0i32;
            for k in 0..ii { min_center += ndelta_min[k] as i32; }
            min_center += (ndelta_min[ii] as i32) >> 1;
            let mut max_center = 1i32 << 15;
            let mut k = l;
            while k > ii { max_center -= ndelta_min[k] as i32; k -= 1; }
            max_center -= (ndelta_min[ii] as i32) >> 1;
            let center = limit(rshift_round(nlsf[ii - 1] as i32 + nlsf[ii] as i32, 1), min_center, max_center) as i16 as i32;
            nlsf[ii - 1] = (center - ((ndelta_min[ii] as i32) >> 1)) as i16;
            nlsf[ii] = (nlsf[ii - 1] as i32 + ndelta_min[ii] as i32) as i16;
        }
        loops += 1;
    }
    insertion_sort_increasing_all_values_i16(nlsf, l);
    nlsf[0] = (nlsf[0] as i32).max(ndelta_min[0] as i32) as i16;
    for i in 1..l {
        nlsf[i] = (nlsf[i] as i32).max(sat16(nlsf[i - 1] as i32 + ndelta_min[i] as i32)) as i16;
    }
    nlsf[l - 1] = (nlsf[l - 1] as i32).min((1 << 15) - ndelta_min[l] as i32) as i16;
    for i in (0..l - 1).rev() {
        nlsf[i] = (nlsf[i] as i32).min(nlsf[i + 1] as i32 - ndelta_min[i + 1] as i32) as i16;
    }
}

pub fn nlsf_decode(nlsf_q15: &mut [i16], indices: &[i8], cb: &NlsfCb) {
    let mut pred_q8 = [0u8; 16];
    let mut ec_ix = [0i16; 16];
    let mut res_q10 = [0i16; 16];
    nlsf_unpack(&mut ec_ix, &mut pred_q8, cb, indices[0] as usize);
    // residual dequant
    let order = cb.order;
    let mut out_q10 = 0i32;
    for i in (0..order).rev() {
        let pred_q10 = smulbb(out_q10, pred_q8[i] as i16 as i32) >> 8;
        out_q10 = lshift(indices[1 + i] as i32, 10);
        if out_q10 > 0 { out_q10 -= 102; } else if out_q10 < 0 { out_q10 += 102; }
        out_q10 = smlawb(pred_q10, out_q10, cb.quant_step_size_q16);
        res_q10[i] = out_q10 as i16;
    }
    let base = indices[0] as usize * order;
    for i in 0..order {
        let tmp = add_lshift32(lshift(res_q10[i] as i32, 14) / cb.cb1_wght_q9[base + i] as i32, cb.cb1_nlsf_q8[base + i] as i16 as i32, 7);
        nlsf_q15[i] = limit(tmp, 0, 32767) as i16;
    }
    nlsf_stabilize(nlsf_q15, cb.delta_min_q15, order);
}

pub fn bwexpander(ar: &mut [i16], d: usize, chirp_q16: i32) {
    let mut chirp = chirp_q16;
    let cm1 = chirp - 65536;
    for i in 0..d - 1 {
        ar[i] = rshift_round(mul(chirp, ar[i] as i32), 16) as i16;
        chirp += rshift_round(mul(chirp, cm1), 16);
    }
    ar[d - 1] = rshift_round(mul(chirp, ar[d - 1] as i32), 16) as i16;
}

pub fn bwexpander_32(ar: &mut [i32], d: usize, chirp_q16: i32) {
    let mut chirp = chirp_q16;
    let cm1 = chirp - 65536;
    for i in 0..d - 1 {
        ar[i] = smulww(chirp, ar[i]);
        chirp += rshift_round(mul(chirp, cm1), 16);
    }
    ar[d - 1] = smulww(chirp, ar[d - 1]);
}

pub fn lpc_fit(a_qout: &mut [i16], a_qin: &mut [i32], qout: i32, qin: i32, d: usize) {
    let mut idx = 0usize;
    let mut i = 0;
    while i < 10 {
        let mut maxabs = 0i32;
        for k in 0..d {
            let a = abs(a_qin[k]);
            if a > maxabs { maxabs = a; idx = k; }
        }
        maxabs = rshift_round(maxabs, qin - qout);
        if maxabs > 32767 {
            maxabs = maxabs.min(163838);
            let chirp_q16 = fix_const(0.999, 16) - lshift(maxabs - 32767, 14) / (mul(maxabs, idx as i32 + 1) >> 2);
            bwexpander_32(a_qin, d, chirp_q16);
        } else {
            break;
        }
        i += 1;
    }
    if i == 10 {
        for k in 0..d {
            a_qout[k] = sat16(rshift_round(a_qin[k], qin - qout)) as i16;
            a_qin[k] = lshift(a_qout[k] as i32, qin - qout);
        }
    } else {
        for k in 0..d { a_qout[k] = rshift_round(a_qin[k], qin - qout) as i16; }
    }
}

const A_LIMIT: i32 = 16773022; // SILK_FIX_CONST(0.99975, 24)
const INV_GAIN_MIN_Q30: i32 = 107374; // SILK_FIX_CONST(1.0f / 1e4f, 30)
#[inline] fn mul32_frac_q(a: i32, b: i32, q: i32) -> i32 { rshift_round64(smull(a, b), q) as i32 }

fn lpc_inverse_pred_gain_qa(a: &mut [i32; 16], order: usize) -> i32 {
    let mut inv_gain_q30 = 1i32 << 30;
    let mut k = order - 1;
    while k > 0 {
        if a[k] > A_LIMIT || a[k] < -A_LIMIT { return 0; }
        let rc_q31 = lshift(a[k], 31 - 24).wrapping_neg();
        let rc_mult1_q30 = (1i32 << 30).wrapping_sub(smmul(rc_q31, rc_q31));
        inv_gain_q30 = lshift(smmul(inv_gain_q30, rc_mult1_q30), 2);
        if inv_gain_q30 < INV_GAIN_MIN_Q30 { return 0; }
        let mult2q = 32 - clz32(abs(rc_mult1_q30));
        let rc_mult2 = inverse32_var_q(rc_mult1_q30, mult2q + 30);
        for n in 0..(k + 1) >> 1 {
            let tmp1 = a[n];
            let tmp2 = a[k - n - 1];
            let t64 = rshift_round64(smull(sub_sat32(tmp1, mul32_frac_q(tmp2, rc_q31, 31)), rc_mult2), mult2q);
            if t64 > i32::MAX as i64 || t64 < i32::MIN as i64 { return 0; }
            a[n] = t64 as i32;
            let t64 = rshift_round64(smull(sub_sat32(tmp2, mul32_frac_q(tmp1, rc_q31, 31)), rc_mult2), mult2q);
            if t64 > i32::MAX as i64 || t64 < i32::MIN as i64 { return 0; }
            a[k - n - 1] = t64 as i32;
        }
        k -= 1;
    }
    if a[0] > A_LIMIT || a[0] < -A_LIMIT { return 0; }
    let rc_q31 = lshift(a[0], 31 - 24).wrapping_neg();
    let rc_mult1_q30 = (1i32 << 30).wrapping_sub(smmul(rc_q31, rc_q31));
    inv_gain_q30 = lshift(smmul(inv_gain_q30, rc_mult1_q30), 2);
    if inv_gain_q30 < INV_GAIN_MIN_Q30 { return 0; }
    inv_gain_q30
}

pub fn lpc_inverse_pred_gain(a_q12: &[i16], order: usize) -> i32 {
    let mut atmp = [0i32; 16];
    let mut dc_resp = 0i32;
    for k in 0..order {
        dc_resp += a_q12[k] as i32;
        atmp[k] = lshift(a_q12[k] as i32, 24 - 12);
    }
    if dc_resp >= 4096 { return 0; }
    lpc_inverse_pred_gain_qa(&mut atmp, order)
}

fn nlsf2a_find_poly(out: &mut [i32], c_lsf: &[i32], off: usize, dd: usize) {
    out[0] = lshift(1, 16);
    out[1] = -c_lsf[off];
    for k in 1..dd {
        let ftmp = c_lsf[off + 2 * k];
        out[k + 1] = lshift(out[k - 1], 1).wrapping_sub(rshift_round64(smull(ftmp, out[k]), 16) as i32);
        let mut n = k;
        while n > 1 {
            out[n] = out[n].wrapping_add(out[n - 2].wrapping_sub(rshift_round64(smull(ftmp, out[n - 1]), 16) as i32));
            n -= 1;
        }
        out[1] = out[1].wrapping_sub(ftmp);
    }
}

pub fn nlsf2a(a_q12: &mut [i16], nlsf: &[i16], d: usize) {
    const ORDERING16: [usize; 16] = [0, 15, 8, 7, 4, 11, 12, 3, 2, 13, 10, 5, 6, 9, 14, 1];
    const ORDERING10: [usize; 10] = [0, 9, 6, 3, 4, 5, 8, 1, 2, 7];
    let ordering: &[usize] = if d == 16 { &ORDERING16 } else { &ORDERING10 };
    let mut cos_lsf_qa = [0i32; 16];
    for k in 0..d {
        let f_int = (nlsf[k] as i32) >> (15 - 7);
        let f_frac = nlsf[k] as i32 - lshift(f_int, 15 - 7);
        let cos_val = LSF_COS_TAB_FIX_Q12[f_int as usize] as i32;
        let delta = LSF_COS_TAB_FIX_Q12[f_int as usize + 1] as i32 - cos_val;
        cos_lsf_qa[ordering[k]] = rshift_round(lshift(cos_val, 8) + mul(delta, f_frac), 20 - 16);
    }
    let dd = d >> 1;
    let mut p = [0i32; 9];
    let mut q = [0i32; 9];
    nlsf2a_find_poly(&mut p, &cos_lsf_qa, 0, dd);
    nlsf2a_find_poly(&mut q, &cos_lsf_qa, 1, dd);
    let mut a32_qa1 = [0i32; 16];
    for k in 0..dd {
        let ptmp = p[k + 1].wrapping_add(p[k]);
        let qtmp = q[k + 1].wrapping_sub(q[k]);
        a32_qa1[k] = qtmp.wrapping_neg().wrapping_sub(ptmp);
        a32_qa1[d - k - 1] = qtmp.wrapping_sub(ptmp);
    }
    lpc_fit(a_q12, &mut a32_qa1, 12, 17, d);
    let mut i = 0;
    while lpc_inverse_pred_gain(a_q12, d) == 0 && i < 16 {
        bwexpander_32(&mut a32_qa1, d, 65536 - lshift(2, i));
        for k in 0..d { a_q12[k] = rshift_round(a32_qa1[k], 17 - 12) as i16; }
        i += 1;
    }
}

pub fn lin2log(in_lin: i32) -> i32 {
    let (lz, frac_q7) = clz_frac(in_lin);
    add_lshift32(smlawb(frac_q7, mul(frac_q7, 128 - frac_q7), 179), 31 - lz, 7)
}

pub fn log2lin(in_log_q7: i32) -> i32 {
    if in_log_q7 < 0 { return 0; }
    if in_log_q7 >= 3967 { return i32::MAX; }
    let mut out = lshift(1, in_log_q7 >> 7);
    let frac_q7 = in_log_q7 & 0x7F;
    if in_log_q7 < 2048 {
        out = add_rshift32(out, mul(out, smlawb(frac_q7, smulbb(frac_q7, 128 - frac_q7), -174)), 7);
    } else {
        out = mla(out, out >> 7, smlawb(frac_q7, smulbb(frac_q7, 128 - frac_q7), -174));
    }
    out
}

pub fn sum_sqr_shift(x: &[i16], len: usize) -> (i32, i32) {
    let mut shft = 31 - clz32(len as i32);
    let mut nrg = len as i32;
    let pass = |shft: i32, init: i32| -> i32 {
        let mut nrg = init;
        let mut i = 0usize;
        while i + 1 < len {
            let mut t = smulbb(x[i] as i32, x[i] as i32) as u32;
            t = t.wrapping_add((x[i + 1] as i32 * x[i + 1] as i32) as u32);
            nrg = (nrg as u32).wrapping_add(t >> shft) as i32;
            i += 2;
        }
        if i < len {
            let t = smulbb(x[i] as i32, x[i] as i32) as u32;
            nrg = (nrg as u32).wrapping_add(t >> shft) as i32;
        }
        nrg
    };
    nrg = pass(shft, nrg);
    shft = 0.max(shft + 3 - clz32(nrg));
    nrg = pass(shft, 0);
    (nrg, shft)
}

/// `silk_LPC_analysis_filter`: out[d..len] from in (both indexed from their own origins).
pub fn lpc_analysis_filter(out: &mut [i16], input: &[i16], b: &[i16], len: usize, d: usize) {
    for ix in d..len {
        let ip = ix - 1;
        let mut o = smulbb(input[ip] as i32, b[0] as i32);
        for j in 1..d { o = o.wrapping_add((input[ip - j] as i32) * (b[j] as i32)); }
        o = lshift(input[ip + 1] as i32, 12).wrapping_sub(o);
        out[ix] = sat16(rshift_round(o, 12)) as i16;
    }
    for v in out[..d].iter_mut() { *v = 0; }
}
