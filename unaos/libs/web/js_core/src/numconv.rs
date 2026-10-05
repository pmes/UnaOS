//! Exact conversions between doubles and decimal text (ECMA-262 §6.1.6.1.20 Number::toString, §21.1.3
//! toFixed / toExponential / toPrecision, §7.1.4.1 StringToNumber). Every conversion is exact: decimal
//! input is correctly rounded (round-half-even) through big integers, and output digits come from the
//! Burger–Dybvig free-format algorithm on exact big-integer arithmetic.

use crate::bignum::{pow2, BigUint};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Round `m * 2^e` (plus a sticky bit for discarded lower bits) to the nearest double (ties to even).
pub fn round_to_f64(m: &BigUint, sticky: bool, e: i64) -> f64 {
    let bits = m.bits() as i64;
    if bits == 0 {
        return 0.0;
    }
    let top = bits - 1 + e; // binary exponent of the leading bit
    if top > 1023 {
        return f64::INFINITY;
    }
    let p: i64 = if top >= -1022 { 53 } else { 53 - (-1022 - top) };
    if p < 0 {
        return 0.0;
    }
    let drop = bits - p;
    let (mut mant, mut x): (u64, i64);
    if drop <= 0 {
        mant = m.low_u64() << (-drop) as u32;
        x = e + drop;
    } else {
        let q = m.shr(drop as u64);
        mant = q.low_u64();
        x = e + drop;
        let half_bit = m.bit((drop - 1) as u64);
        let below = m.low_bits_nonzero((drop - 1) as u64) || sticky;
        if half_bit && (below || mant & 1 == 1) {
            mant += 1;
            if p > 0 && mant == 1u64 << p {
                mant >>= 1;
                x += 1;
            } else if p == 0 && mant == 1 {
                // rounded up to the smallest subnormal
            }
        }
    }
    if mant == 0 {
        return 0.0;
    }
    if x + (64 - mant.leading_zeros() as i64) - 1 > 1023 {
        return f64::INFINITY;
    }
    let f = mant as f64; // exact: mant <= 2^53
    if x < -900 {
        f * pow2((x + 600) as i32) * pow2(-600)
    } else {
        f * pow2(x as i32)
    }
}

const POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19,
    1e20, 1e21, 1e22,
];

/// Correctly rounded value of `digits × 10^exp10` (digits are ASCII '0'..'9').
pub fn decimal_to_f64(digits: &[u8], exp10: i64) -> f64 {
    let mut d = digits;
    while let [b'0', rest @ ..] = d {
        d = rest;
    }
    let mut exp10 = exp10;
    while let [rest @ .., b'0'] = d {
        d = rest;
        exp10 += 1;
    }
    if d.is_empty() {
        return 0.0;
    }
    let nd = d.len() as i64;
    if nd + exp10 > 310 {
        return f64::INFINITY;
    }
    if nd + exp10 < -326 {
        return 0.0;
    }
    if nd <= 15 && (-22..=22).contains(&exp10) {
        let mut m: u64 = 0;
        for &c in d {
            m = m * 10 + (c - b'0') as u64;
        }
        let f = m as f64;
        return if exp10 >= 0 { f * POW10[exp10 as usize] } else { f / POW10[(-exp10) as usize] };
    }
    let n = BigUint::from_digits(d, 10);
    if exp10 >= 0 {
        let v = n.mul(&BigUint::pow_small(10, exp10 as u32));
        return round_to_f64(&v, false, 0);
    }
    let den = BigUint::pow_small(10, (-exp10) as u32);
    let s = (66 + den.bits() as i64 - n.bits() as i64).max(0);
    let (q, r) = n.shl(s as u64).divrem(&den);
    round_to_f64(&q, !r.is_zero(), -s)
}

/// Decompose a finite positive double into (f, e) with v = f × 2^e.
fn decompose(v: f64) -> (u64, i32) {
    let bits = v.to_bits();
    let exp = ((bits >> 52) & 0x7FF) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    if exp == 0 {
        (frac, -1074)
    } else {
        (frac | (1u64 << 52), exp - 1075)
    }
}

/// Shortest round-tripping digits of a finite positive double: (digits, k) with v ≈ 0.d1d2…dn × 10^k.
pub fn shortest_digits(v: f64) -> (Vec<u8>, i32) {
    let (f, e) = decompose(v);
    let even = f & 1 == 0;
    let (mut r, mut s, mut mp, mut mm);
    let fb = BigUint::from_u64(f);
    if e >= 0 {
        let be = BigUint::from_u64(1).shl(e as u64);
        if f != 1u64 << 52 {
            r = fb.shl(e as u64 + 1);
            s = BigUint::from_u64(2);
            mp = be.clone();
            mm = be;
        } else {
            r = fb.shl(e as u64 + 2);
            s = BigUint::from_u64(4);
            mp = be.shl(1);
            mm = be;
        }
    } else if e == -1074 || f != 1u64 << 52 {
        r = fb.shl(1);
        s = BigUint::from_u64(1).shl((1 - e) as u64);
        mp = BigUint::from_u64(1);
        mm = BigUint::from_u64(1);
    } else {
        r = fb.shl(2);
        s = BigUint::from_u64(1).shl((2 - e) as u64);
        mp = BigUint::from_u64(2);
        mm = BigUint::from_u64(1);
    }
    // Estimate k = ceil(log10(v)).
    let mut k = libm_log10_ceil(v);
    if k >= 0 {
        s = s.mul(&BigUint::pow_small(10, k as u32));
    } else {
        let p = BigUint::pow_small(10, (-k) as u32);
        r = r.mul(&p);
        mp = mp.mul(&p);
        mm = mm.mul(&p);
    }
    // Fix up the estimate in both directions.
    loop {
        let hi = r.add(&mp);
        let c = hi.cmp(&s);
        if c == core::cmp::Ordering::Greater || (even && c == core::cmp::Ordering::Equal) {
            s = s.mul_small(10);
            k += 1;
        } else {
            break;
        }
    }
    loop {
        let hi = r.add(&mp).mul_small(10);
        let c = hi.cmp(&s);
        if c == core::cmp::Ordering::Less || (!even && c == core::cmp::Ordering::Equal) {
            r = r.mul_small(10);
            mp = mp.mul_small(10);
            mm = mm.mul_small(10);
            k -= 1;
        } else {
            break;
        }
    }
    let mut digits = Vec::new();
    loop {
        r = r.mul_small(10);
        mp = mp.mul_small(10);
        mm = mm.mul_small(10);
        let (q, rem) = r.divrem(&s);
        let d = q.low_u64() as u8;
        r = rem;
        let c1 = r.cmp(&mm);
        let tc1 = c1 == core::cmp::Ordering::Less || (even && c1 == core::cmp::Ordering::Equal);
        let c2 = r.add(&mp).cmp(&s);
        let tc2 = c2 == core::cmp::Ordering::Greater || (even && c2 == core::cmp::Ordering::Equal);
        if !tc1 && !tc2 {
            digits.push(d);
            continue;
        }
        let last = if tc1 && !tc2 {
            d
        } else if !tc1 && tc2 {
            d + 1
        } else {
            let c = r.shl(1).cmp(&s);
            match c {
                core::cmp::Ordering::Less => d,
                core::cmp::Ordering::Greater => d + 1,
                core::cmp::Ordering::Equal => {
                    if d % 2 == 0 {
                        d
                    } else {
                        d + 1
                    }
                }
            }
        };
        digits.push(last);
        break;
    }
    // A carry into a 10 cannot happen with correct boundaries, but normalise defensively.
    let mut i = digits.len();
    while i > 0 && digits[i - 1] == 10 {
        digits[i - 1] = 0;
        if i == 1 {
            digits.insert(0, 1);
            k += 1;
            break;
        }
        digits[i - 2] += 1;
        i -= 1;
    }
    while digits.len() > 1 && *digits.last().unwrap() == 0 {
        digits.pop();
    }
    (digits.iter().map(|d| d + b'0').collect(), k)
}

fn libm_log10_ceil(v: f64) -> i32 {
    // log10(v) ≈ (exponent + log2(mantissa)) * log10(2); only an estimate, fixed up exactly by the caller.
    let (f, e) = decompose(v);
    let bits = 64 - f.leading_zeros() as i32;
    let l2 = (e + bits - 1) as f64; // floor(log2 v)
    let est = l2 * 0.301_029_995_663_981_2;
    let c = est as i32;
    if est > c as f64 {
        c + 1
    } else {
        c
    }
}

/// Number::toString(x) for radix 10 (§6.1.6.1.20).
pub fn f64_to_js_string(v: f64) -> String {
    if v.is_nan() {
        return String::from("NaN");
    }
    if v == 0.0 {
        return String::from("0");
    }
    if v.is_infinite() {
        return String::from(if v > 0.0 { "Infinity" } else { "-Infinity" });
    }
    let mut out = String::new();
    let v = if v < 0.0 {
        out.push('-');
        -v
    } else {
        v
    };
    if v < 9007199254740992.0 && (v as u64) as f64 == v {
        let mut n = v as u64;
        let mut buf = [0u8; 20];
        let mut i = buf.len();
        while n > 0 {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        out.push_str(core::str::from_utf8(&buf[i..]).unwrap());
        return out;
    }
    let (digits, n) = shortest_digits(v);
    format_js(&mut out, &digits, n);
    out
}

fn format_js(out: &mut String, digits: &[u8], n: i32) {
    let k = digits.len() as i32;
    let ds = core::str::from_utf8(digits).unwrap();
    if k <= n && n <= 21 {
        out.push_str(ds);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&ds[..n as usize]);
        out.push('.');
        out.push_str(&ds[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(ds);
    } else {
        out.push_str(&ds[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&ds[1..]);
        }
        out.push('e');
        let e = n - 1;
        out.push(if e >= 0 { '+' } else { '-' });
        push_int(out, e.unsigned_abs() as u64);
    }
}

pub fn push_int(out: &mut String, mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    if n == 0 {
        out.push('0');
        return;
    }
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    out.push_str(core::str::from_utf8(&buf[i..]).unwrap());
}

/// Exact value of a finite non-negative double as (numerator, denominator-power-of-two).
fn exact(v: f64) -> (BigUint, i32) {
    let (f, e) = decompose(v);
    (BigUint::from_u64(f), e)
}

/// The integer n minimising |n / 10^frac - v| (ties: larger n), as decimal digits. v >= 0, finite.
pub fn round_scaled(v: f64, frac: i32) -> BigUint {
    let (f, e) = exact(v);
    // n = round(f * 2^e * 10^frac), half up
    let (num, den_shift) = if frac >= 0 {
        (f.mul(&BigUint::pow_small(10, frac as u32)), e)
    } else {
        (f, e)
    };
    if frac < 0 {
        // divide by 10^-frac as well
        let den = BigUint::pow_small(10, (-frac) as u32);
        let (num2, den2) = if den_shift >= 0 { (num.shl(den_shift as u64), den) } else { (num, den.shl((-den_shift) as u64)) };
        let (q, r) = num2.divrem(&den2);
        return if r.shl(1).cmp(&den2) != core::cmp::Ordering::Less { q.add(&BigUint::from_u64(1)) } else { q };
    }
    if den_shift >= 0 {
        return num.shl(den_shift as u64);
    }
    let sh = (-den_shift) as u64;
    let q = num.shr(sh);
    if sh > 0 && num.bit(sh - 1) {
        q.add(&BigUint::from_u64(1))
    } else {
        q
    }
}

/// Number.prototype.toFixed digits for 0 <= v < 1e21.
pub fn to_fixed(v: f64, frac: usize) -> String {
    let n = round_scaled(v, frac as i32);
    let mut m: Vec<u8> = if n.is_zero() { vec![b'0'] } else { n.to_string_radix(10) };
    let mut out = String::new();
    if frac != 0 {
        if m.len() <= frac {
            let mut z = vec![b'0'; frac + 1 - m.len()];
            z.extend_from_slice(&m);
            m = z;
        }
        let k = m.len();
        out.push_str(core::str::from_utf8(&m[..k - frac]).unwrap());
        out.push('.');
        out.push_str(core::str::from_utf8(&m[k - frac..]).unwrap());
    } else {
        out.push_str(core::str::from_utf8(&m).unwrap());
    }
    out
}

/// Digits for toExponential / toPrecision: `p` significant digits of v (v > 0), returning (digits, e) with
/// v ≈ d.ddd × 10^e. Ties pick the larger n.
pub fn precision_digits(v: f64, p: usize) -> (Vec<u8>, i32) {
    // e = floor(log10 v), fixed up exactly.
    let (sd, k) = shortest_digits(v);
    let _ = sd;
    let mut e = k - 1;
    loop {
        let n = round_scaled(v, p as i32 - 1 - e);
        let digits = n.to_string_radix(10);
        if digits.len() > p {
            e += 1;
            continue;
        }
        if digits.len() < p {
            e -= 1;
            continue;
        }
        // The shortest form may have rounded up across a power of ten (1e-21 is really 9.99…e-22): prefer the
        // finer exponent whenever it still yields exactly p digits.
        let n2 = round_scaled(v, p as i32 - e);
        let d2 = n2.to_string_radix(10);
        if d2.len() == p {
            return (d2, e - 1);
        }
        return (digits, e);
    }
}

// ------------------------------------------------------------------------------------------------ parsing

/// Parse a StrDecimalLiteral prefix (used by both StringToNumber and parseFloat). Returns (value, bytes used).
/// `s` is ASCII bytes. Accepts optional sign, "Infinity", digits with optional fraction and exponent.
pub fn parse_decimal_prefix(s: &[u8]) -> Option<(f64, usize)> {
    let mut i = 0;
    let mut neg = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    if s[i..].starts_with(b"Infinity") {
        let v = if neg { f64::NEG_INFINITY } else { f64::INFINITY };
        return Some((v, i + 8));
    }
    let mut digits: Vec<u8> = Vec::new();
    let mut exp: i64 = 0;
    let mut any = false;
    while i < s.len() && s[i].is_ascii_digit() {
        digits.push(s[i]);
        any = true;
        i += 1;
    }
    if i < s.len() && s[i] == b'.' {
        let save = i;
        i += 1;
        let mut frac_any = false;
        while i < s.len() && s[i].is_ascii_digit() {
            digits.push(s[i]);
            exp -= 1;
            frac_any = true;
            i += 1;
        }
        if !any && !frac_any {
            i = save;
        }
        any = any || frac_any;
    }
    if !any {
        return None;
    }
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        let mut j = i + 1;
        let mut eneg = false;
        if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
            eneg = s[j] == b'-';
            j += 1;
        }
        if j < s.len() && s[j].is_ascii_digit() {
            let mut ev: i64 = 0;
            while j < s.len() && s[j].is_ascii_digit() {
                if ev < 100_000_000 {
                    ev = ev * 10 + (s[j] - b'0') as i64;
                }
                j += 1;
            }
            exp += if eneg { -ev } else { ev };
            i = j;
        }
    }
    let v = decimal_to_f64(&digits, exp);
    Some((if neg { -v } else { v }, i))
}

/// Parse an integer in `radix` from ASCII digits (all valid), exactly rounded.
pub fn parse_radix_int(digits: &[u8], radix: u32) -> f64 {
    if radix == 10 {
        return decimal_to_f64(digits, 0);
    }
    let n = BigUint::from_digits(digits, radix);
    n.to_f64()
}

pub fn digit_val(c: u32) -> Option<u32> {
    match c {
        0x30..=0x39 => Some(c - 0x30),
        0x61..=0x7A => Some(c - 0x61 + 10),
        0x41..=0x5A => Some(c - 0x41 + 10),
        _ => None,
    }
}

/// Number.prototype.toString(radix) for radix != 10 (shortest digits that uniquely identify the value,
/// following the delta method used by engines: digits are produced until the remaining fraction is
/// within half an ulp).
pub fn f64_to_radix_string(v: f64, radix: u32) -> String {
    if v.is_nan() {
        return String::from("NaN");
    }
    if v.is_infinite() {
        return String::from(if v > 0.0 { "Infinity" } else { "-Infinity" });
    }
    if v == 0.0 {
        return String::from("0");
    }
    let chars = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let neg = v < 0.0;
    let value = v.abs();
    let mut integer = libm_floor(value);
    let mut fraction = value - integer;
    let next = f64::from_bits(value.to_bits() + 1);
    let mut delta = 0.5 * (next - value);
    let prev0 = f64::from_bits(0);
    let _ = prev0;
    if !(delta > 0.0) {
        delta = f64::from_bits(1);
    }
    let mut frac_digits: Vec<u8> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= radix as f64;
            delta *= radix as f64;
            let digit = fraction as u32;
            frac_digits.push(digit as u8);
            fraction -= digit as f64;
            if fraction > 0.5 || (fraction == 0.5 && (digit & 1) == 1) {
                if fraction + delta > 1.0 {
                    // round up, propagating carries
                    loop {
                        match frac_digits.pop() {
                            None => {
                                integer += 1.0;
                                break;
                            }
                            Some(d) => {
                                if (d as u32) + 1 < radix {
                                    frac_digits.push(d + 1);
                                    break;
                                }
                            }
                        }
                    }
                    break;
                }
            }
            if fraction < delta {
                break;
            }
        }
    }
    // Integer part: exact via bignum.
    let (f, e) = decompose(integer.max(0.0));
    let int_digits: Vec<u8> = if integer == 0.0 {
        vec![b'0']
    } else {
        let n = if e >= 0 { BigUint::from_u64(f).shl(e as u64) } else { BigUint::from_u64(f).shr((-e) as u64) };
        n.to_string_radix(radix)
    };
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push_str(core::str::from_utf8(&int_digits).unwrap());
    if !frac_digits.is_empty() {
        out.push('.');
        for d in frac_digits {
            out.push(chars[d as usize] as char);
        }
    }
    out
}

pub fn libm_floor(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    if x.abs() >= 4503599627370496.0 {
        return x;
    }
    let t = x as i64 as f64;
    if t > x {
        t - 1.0
    } else {
        t
    }
}
