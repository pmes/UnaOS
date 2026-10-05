//! The fixed-point arithmetic of the Opus reference decoder (`celt/fixed_generic.h`, `celt/mathops.[ch]`,
//! 64-bit-host flavour: `OPUS_FAST_INT64`). Every helper reproduces the C macro's casts exactly — the
//! 16-bit truncation of `MULT16_16`'s operands, the wrap of `ADD16` — because RFC 6716 makes the reference
//! implementation normative and the KAT is bit-exact PCM.
#![allow(dead_code)]

pub const Q15ONE: i32 = 32767;
pub const SIG_SHIFT: i32 = 12;
pub const SIG_SAT: i32 = 300_000_000;
pub const DB_SHIFT: i32 = 10;

#[inline(always)] pub fn mult16_16(a: i32, b: i32) -> i32 { (a as i16 as i32).wrapping_mul(b as i16 as i32) }
#[inline(always)] pub fn mult16_16su(a: i32, b: i32) -> i32 { (a as i16 as i32).wrapping_mul(b as u16 as i32) }
#[inline(always)] pub fn mult16_16_q15(a: i32, b: i32) -> i32 { mult16_16(a, b) >> 15 }
#[inline(always)] pub fn mult16_16_q14(a: i32, b: i32) -> i32 { mult16_16(a, b) >> 14 }
#[inline(always)] pub fn mult16_16_q13(a: i32, b: i32) -> i32 { mult16_16(a, b) >> 13 }
#[inline(always)] pub fn mult16_16_q11(a: i32, b: i32) -> i32 { mult16_16(a, b) >> 11 }
#[inline(always)] pub fn mult16_16_p15(a: i32, b: i32) -> i32 { mult16_16(a, b).wrapping_add(16384) >> 15 }
#[inline(always)] pub fn mult16_16_p14(a: i32, b: i32) -> i32 { mult16_16(a, b).wrapping_add(8192) >> 14 }
#[inline(always)] pub fn mult16_16_p13(a: i32, b: i32) -> i32 { mult16_16(a, b).wrapping_add(4096) >> 13 }
#[inline(always)] pub fn mult16_32_q15(a: i32, b: i32) -> i32 { (((a as i16 as i64) * (b as i64)) >> 15) as i32 }
#[inline(always)] pub fn mult16_32_q16(a: i32, b: i32) -> i32 { (((a as i16 as i64) * (b as i64)) >> 16) as i32 }
#[inline(always)] pub fn mult16_32_p16(a: i32, b: i32) -> i32 { (((a as i16 as i64) * (b as i64) + 32768) >> 16) as i32 }
#[inline(always)] pub fn mult32_32_q31(a: i32, b: i32) -> i32 { (((a as i64) * (b as i64)) >> 31) as i32 }
#[inline(always)] pub fn mult32_32_q16(a: i32, b: i32) -> i32 { (((a as i64) * (b as i64)) >> 16) as i32 }
#[inline(always)] pub fn mac16_16(c: i32, a: i32, b: i32) -> i32 { c.wrapping_add(mult16_16(a, b)) }
#[inline(always)] pub fn mac16_32_q15(c: i32, a: i32, b: i32) -> i32 {
    c.wrapping_add(mult16_16(a, b >> 15).wrapping_add(mult16_16(a, b & 0x7fff) >> 15))
}
#[inline(always)] pub fn mac16_32_q16(c: i32, a: i32, b: i32) -> i32 {
    c.wrapping_add(mult16_16(a, b >> 16).wrapping_add(mult16_16su(a, b & 0xffff) >> 16))
}
/// `ADD16`: the sum truncated to 16 bits.
#[inline(always)] pub fn add16(a: i32, b: i32) -> i32 { (a as i16).wrapping_add(b as i16) as i32 }
/// `SUB16`: operands truncated, the difference is NOT.
#[inline(always)] pub fn sub16(a: i32, b: i32) -> i32 { (a as i16 as i32) - (b as i16 as i32) }
#[inline(always)] pub fn shl16(a: i32, s: i32) -> i32 { ((a as u16) << s) as i16 as i32 }
#[inline(always)] pub fn shl32(a: i32, s: i32) -> i32 { ((a as u32) << s) as i32 }
#[inline(always)] pub fn pshr32(a: i32, s: i32) -> i32 { a.wrapping_add((1i32 << s) >> 1) >> s }
#[inline(always)] pub fn vshr32(a: i32, s: i32) -> i32 { if s > 0 { a >> s } else { shl32(a, -s) } }
#[inline(always)] pub fn saturate(x: i32, a: i32) -> i32 { if x > a { a } else if x < -a { -a } else { x } }
#[inline(always)] pub fn sat16(x: i32) -> i32 { x.clamp(-32768, 32767) }
#[inline(always)] pub fn round16(x: i32, a: i32) -> i32 { pshr32(x, a) as i16 as i32 }
#[inline(always)] pub fn sround16(x: i32, a: i32) -> i32 { saturate(pshr32(x, a), 32767) as i16 as i32 }
#[inline(always)] pub fn extract16(x: i32) -> i32 { x as i16 as i32 }
/// `SIG2WORD16`.
#[inline(always)] pub fn sig2word16(x: i32) -> i16 { pshr32(x, SIG_SHIFT).clamp(-32768, 32767) as i16 }

#[inline(always)] pub fn celt_ilog2(x: i32) -> i32 { 31 - (x as u32).leading_zeros() as i32 }
#[inline(always)] pub fn celt_zlog2(x: i32) -> i32 { if x <= 0 { 0 } else { celt_ilog2(x) } }

pub fn celt_maxabs16(x: &[i16]) -> i32 {
    let mut maxv = 0i32;
    let mut minv = 0i32;
    for &v in x { maxv = maxv.max(v as i32); minv = minv.min(v as i32); }
    maxv.max(-minv)
}
pub fn celt_maxabs32(x: &[i32]) -> i32 {
    let mut maxv = 0i32;
    let mut minv = 0i32;
    for &v in x { maxv = maxv.max(v); minv = minv.min(v); }
    maxv.max(minv.wrapping_neg())
}

/// Base-2 log, Q14 in, Q10 out.
pub fn celt_log2(x: i32) -> i32 {
    const C: [i32; 5] = [-6801 + (1 << (13 - DB_SHIFT)), 15746, -5217, 2545, -1401];
    if x == 0 { return -32767; }
    let i = celt_ilog2(x);
    let n = (vshr32(x, i - 15) - 32768 - 16384) as i16 as i32;
    let frac = add16(C[0], mult16_16_q15(n, add16(C[1], mult16_16_q15(n, add16(C[2], mult16_16_q15(n, add16(C[3], mult16_16_q15(n, C[4]))))))));
    add16(shl16(i - 13, DB_SHIFT), frac >> (14 - DB_SHIFT)) // SHL16(..)+SHR16(..) then stored as val16
}
fn celt_exp2_frac(x: i32) -> i32 {
    let frac = shl16(x, 4);
    add16(16383, mult16_16_q15(frac, add16(22804, mult16_16_q15(frac, add16(14819, mult16_16_q15(10204, frac))))))
}
/// Base-2 exponential, Q10 in, Q16 out.
pub fn celt_exp2(x: i32) -> i32 {
    let x = x as i16 as i32;
    let integer = (x as i16 as i32) >> 10;
    if integer > 14 { return 0x7f000000; }
    if integer < -15 { return 0; }
    let frac = celt_exp2_frac(x - shl16(integer, 10)) as i16 as i32;
    vshr32(frac, -integer - 2)
}
pub fn celt_rcp(x: i32) -> i32 {
    let i = celt_ilog2(x);
    let n = (vshr32(x, i - 15) - 32768) as i16 as i32;
    let mut r = add16(30840, mult16_16_q15(-15420, n));
    r = sub16(r, mult16_16_q15(r, add16(mult16_16_q15(r, n), add16(r, -32768)))) as i16 as i32;
    r = sub16(r, add16(1, mult16_16_q15(r, add16(mult16_16_q15(r, n), add16(r, -32768))))) as i16 as i32;
    vshr32(r, i - 16)
}
#[inline] pub fn celt_div(a: i32, b: i32) -> i32 { mult32_32_q31(a, celt_rcp(b)) }
pub fn frac_div32(a: i32, b: i32) -> i32 {
    let shift = celt_ilog2(b) - 29;
    let a = vshr32(a, shift);
    let b = vshr32(b, shift);
    let rcp = round16(celt_rcp(round16(b, 16)), 3);
    let mut result = mult16_32_q15(rcp, a);
    let rem = pshr32(a, 2).wrapping_sub(mult32_32_q31(result, b));
    result = result.wrapping_add(shl32(mult16_32_q15(rcp, rem), 2));
    if result >= 536870912 { 2147483647 } else if result <= -536870912 { -2147483647 } else { shl32(result, 2) }
}
/// Reciprocal sqrt in [0.25,1): Q16 in, Q14 out.
pub fn celt_rsqrt_norm(x: i32) -> i32 {
    let n = (x - 32768) as i16 as i32;
    let r = add16(23557, mult16_16_q15(n, add16(-13490, mult16_16_q15(n, 6713))));
    let r2 = mult16_16_q15(r, r) as i16 as i32;
    let y = shl16(sub16(add16(mult16_16_q15(r2, n), r2), 16384), 1);
    add16(r, mult16_16_q15(r, mult16_16_q15(y, sub16(mult16_16_q15(y, 12288), 16384))))
}
pub fn celt_sqrt(x: i32) -> i32 {
    const C: [i32; 5] = [23175, 11561, -3011, 1699, -664];
    if x == 0 { return 0; }
    if x >= 1073741824 { return 32767; }
    let k = (celt_ilog2(x) >> 1) - 7;
    let x = vshr32(x, 2 * k);
    let n = (x - 32768) as i16 as i32;
    let rt = add16(C[0], mult16_16_q15(n, add16(C[1], mult16_16_q15(n, add16(C[2], mult16_16_q15(n, add16(C[3], mult16_16_q15(n, C[4]))))))));
    vshr32(rt, 7 - k)
}
fn cos_pi_2(x: i32) -> i32 {
    let x2 = mult16_16_p15(x, x) as i16 as i32;
    add16(1, 32766.min(sub16(32767, x2).wrapping_add(mult16_16_p15(x2, (-7651i32).wrapping_add(mult16_16_p15(x2, 8277i32.wrapping_add(mult16_16_p15(-626, x2))))))))
}
pub fn celt_cos_norm(x: i32) -> i32 {
    let mut x = x & 0x0001ffff;
    if x > (1 << 16) { x = (1 << 17) - x; }
    if x & 0x00007fff != 0 {
        if x < (1 << 15) { cos_pi_2(x as i16 as i32) } else { -cos_pi_2((65536 - x) as i16 as i32) }
    } else if x & 0x0000ffff != 0 { 0 } else if x & 0x0001ffff != 0 { -32767 } else { 32767 }
}
pub fn isqrt32(mut val: u32) -> u32 {
    let mut g = 0u32;
    let mut bshift = (crate::opus::range::ilog(val) - 1) >> 1;
    let mut b = 1u32 << bshift;
    loop {
        let t = ((g << 1) + b) << bshift;
        if t <= val { g += b; val -= t; }
        b >>= 1;
        bshift -= 1;
        if bshift < 0 { break; }
    }
    g
}
fn atan01(x: i32) -> i32 {
    mult16_16_p15(x, 32767i32.wrapping_add(mult16_16_p15(x, (-21i32).wrapping_add(mult16_16_p15(x, (-11943i32).wrapping_add(mult16_16_p15(4936, x)))))))
}
pub fn celt_atan2p(y: i32, x: i32) -> i32 {
    if y < x {
        let mut arg = celt_div(shl32(y, 15), x);
        if arg >= 32767 { arg = 32767; }
        atan01(arg as i16 as i32) >> 1
    } else {
        let mut arg = celt_div(shl32(x, 15), y);
        if arg >= 32767 { arg = 32767; }
        25736 - (atan01(arg as i16 as i32) >> 1)
    }
}
