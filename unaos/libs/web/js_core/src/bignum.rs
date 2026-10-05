//! Arbitrary-precision unsigned integers (little-endian u32 limbs) for BigInt, exact decimal conversion of
//! doubles (Number::toString, toFixed, toExponential, toPrecision) and correctly rounded string -> double.

use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BigUint {
    pub limbs: Vec<u32>, // little endian, no trailing zero limbs
}

impl BigUint {
    pub fn zero() -> Self {
        BigUint { limbs: Vec::new() }
    }
    pub fn from_u64(v: u64) -> Self {
        let mut b = BigUint { limbs: vec![v as u32, (v >> 32) as u32] };
        b.trim();
        b
    }
    pub fn from_u128(v: u128) -> Self {
        let mut b = BigUint { limbs: vec![v as u32, (v >> 32) as u32, (v >> 64) as u32, (v >> 96) as u32] };
        b.trim();
        b
    }
    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }
    pub fn trim(&mut self) {
        while let Some(&0) = self.limbs.last() {
            self.limbs.pop();
        }
    }
    pub fn bits(&self) -> u64 {
        match self.limbs.last() {
            None => 0,
            Some(&top) => (self.limbs.len() as u64 - 1) * 32 + (32 - top.leading_zeros() as u64),
        }
    }
    pub fn bit(&self, i: u64) -> bool {
        let l = (i / 32) as usize;
        l < self.limbs.len() && (self.limbs[l] >> (i % 32)) & 1 == 1
    }
    pub fn to_u64(&self) -> Option<u64> {
        match self.limbs.len() {
            0 => Some(0),
            1 => Some(self.limbs[0] as u64),
            2 => Some(self.limbs[0] as u64 | (self.limbs[1] as u64) << 32),
            _ => None,
        }
    }
    /// Low 64 bits (wrapping).
    pub fn low_u64(&self) -> u64 {
        let a = self.limbs.first().copied().unwrap_or(0) as u64;
        let b = self.limbs.get(1).copied().unwrap_or(0) as u64;
        a | b << 32
    }
    pub fn cmp(&self, o: &BigUint) -> Ordering {
        if self.limbs.len() != o.limbs.len() {
            return self.limbs.len().cmp(&o.limbs.len());
        }
        for i in (0..self.limbs.len()).rev() {
            if self.limbs[i] != o.limbs[i] {
                return self.limbs[i].cmp(&o.limbs[i]);
            }
        }
        Ordering::Equal
    }
    pub fn add(&self, o: &BigUint) -> BigUint {
        let n = self.limbs.len().max(o.limbs.len());
        let mut r = Vec::with_capacity(n + 1);
        let mut carry = 0u64;
        for i in 0..n {
            let s = self.limbs.get(i).copied().unwrap_or(0) as u64 + o.limbs.get(i).copied().unwrap_or(0) as u64 + carry;
            r.push(s as u32);
            carry = s >> 32;
        }
        if carry != 0 {
            r.push(carry as u32);
        }
        let mut b = BigUint { limbs: r };
        b.trim();
        b
    }
    /// self - o, requires self >= o.
    pub fn sub(&self, o: &BigUint) -> BigUint {
        let mut r = Vec::with_capacity(self.limbs.len());
        let mut borrow = 0i64;
        for i in 0..self.limbs.len() {
            let mut d = self.limbs[i] as i64 - o.limbs.get(i).copied().unwrap_or(0) as i64 - borrow;
            if d < 0 {
                d += 1 << 32;
                borrow = 1;
            } else {
                borrow = 0;
            }
            r.push(d as u32);
        }
        let mut b = BigUint { limbs: r };
        b.trim();
        b
    }
    pub fn mul(&self, o: &BigUint) -> BigUint {
        if self.is_zero() || o.is_zero() {
            return BigUint::zero();
        }
        if self.limbs.len() >= 48 && o.limbs.len() >= 48 {
            return karatsuba(self, o);
        }
        let mut r = vec![0u32; self.limbs.len() + o.limbs.len()];
        for (i, &a) in self.limbs.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in o.limbs.iter().enumerate() {
                let t = a as u64 * b as u64 + r[i + j] as u64 + carry;
                r[i + j] = t as u32;
                carry = t >> 32;
            }
            let mut k = i + o.limbs.len();
            while carry != 0 {
                let t = r[k] as u64 + carry;
                r[k] = t as u32;
                carry = t >> 32;
                k += 1;
            }
        }
        let mut b = BigUint { limbs: r };
        b.trim();
        b
    }
    pub fn mul_small(&self, m: u32) -> BigUint {
        let mut r = self.clone();
        r.mul_small_add_assign(m, 0);
        r
    }
    pub fn mul_small_add_assign(&mut self, m: u32, add: u32) {
        let mut carry = add as u64;
        for l in self.limbs.iter_mut() {
            let t = *l as u64 * m as u64 + carry;
            *l = t as u32;
            carry = t >> 32;
        }
        if carry != 0 {
            self.limbs.push(carry as u32);
        }
        self.trim();
    }
    /// Divide in place by a small divisor, returning the remainder.
    pub fn div_small_assign(&mut self, d: u32) -> u32 {
        let mut rem = 0u64;
        for l in self.limbs.iter_mut().rev() {
            let cur = rem << 32 | *l as u64;
            *l = (cur / d as u64) as u32;
            rem = cur % d as u64;
        }
        self.trim();
        rem as u32
    }
    pub fn shl(&self, n: u64) -> BigUint {
        if self.is_zero() {
            return BigUint::zero();
        }
        let ls = (n / 32) as usize;
        let bs = (n % 32) as u32;
        let mut r = vec![0u32; ls];
        if bs == 0 {
            r.extend_from_slice(&self.limbs);
        } else {
            let mut carry = 0u32;
            for &l in &self.limbs {
                r.push(l << bs | carry);
                carry = l >> (32 - bs);
            }
            if carry != 0 {
                r.push(carry);
            }
        }
        let mut b = BigUint { limbs: r };
        b.trim();
        b
    }
    pub fn shr(&self, n: u64) -> BigUint {
        let ls = (n / 32) as usize;
        if ls >= self.limbs.len() {
            return BigUint::zero();
        }
        let bs = (n % 32) as u32;
        let src = &self.limbs[ls..];
        let mut r = Vec::with_capacity(src.len());
        if bs == 0 {
            r.extend_from_slice(src);
        } else {
            for i in 0..src.len() {
                let hi = if i + 1 < src.len() { src[i + 1] << (32 - bs) } else { 0 };
                r.push(src[i] >> bs | hi);
            }
        }
        let mut b = BigUint { limbs: r };
        b.trim();
        b
    }
    /// True if any of the low `n` bits are set.
    pub fn low_bits_nonzero(&self, n: u64) -> bool {
        let full = (n / 32) as usize;
        for i in 0..full.min(self.limbs.len()) {
            if self.limbs[i] != 0 {
                return true;
            }
        }
        let rb = n % 32;
        if rb != 0 && full < self.limbs.len() {
            return self.limbs[full] & ((1u32 << rb) - 1) != 0;
        }
        false
    }
    pub fn pow_small(base: u32, mut e: u32) -> BigUint {
        let mut r = BigUint::from_u64(1);
        let mut b = BigUint::from_u64(base as u64);
        while e > 0 {
            if e & 1 == 1 {
                r = r.mul(&b);
            }
            e >>= 1;
            if e > 0 {
                b = b.mul(&b);
            }
        }
        r
    }
    pub fn pow(&self, mut e: u64) -> BigUint {
        let mut r = BigUint::from_u64(1);
        let mut b = self.clone();
        while e > 0 {
            if e & 1 == 1 {
                r = r.mul(&b);
            }
            e >>= 1;
            if e > 0 {
                b = b.mul(&b);
            }
        }
        r
    }
    /// (quotient, remainder). Panics on division by zero (callers check).
    pub fn divrem(&self, d: &BigUint) -> (BigUint, BigUint) {
        assert!(!d.is_zero());
        if self.cmp(d) == Ordering::Less {
            return (BigUint::zero(), self.clone());
        }
        if d.limbs.len() == 1 {
            let mut q = self.clone();
            let r = q.div_small_assign(d.limbs[0]);
            return (q, BigUint::from_u64(r as u64));
        }
        // Knuth algorithm D.
        let s = d.limbs.last().unwrap().leading_zeros();
        let v = d.shl(s as u64);
        let mut u = self.shl(s as u64).limbs;
        u.push(0);
        let n = v.limbs.len();
        let m = u.len() - n - 1;
        let mut q = vec![0u32; m + 1];
        let vt = v.limbs[n - 1] as u64;
        let vt2 = v.limbs[n - 2] as u64;
        for j in (0..=m).rev() {
            let num = (u[j + n] as u64) << 32 | u[j + n - 1] as u64;
            let mut qhat = num / vt;
            let mut rhat = num % vt;
            while qhat >= 1 << 32 || qhat * vt2 > (rhat << 32 | u[j + n - 2] as u64) {
                qhat -= 1;
                rhat += vt;
                if rhat >= 1 << 32 {
                    break;
                }
            }
            let mut borrow = 0i64;
            let mut carry = 0u64;
            for i in 0..n {
                let p = qhat * v.limbs[i] as u64 + carry;
                carry = p >> 32;
                let t = u[i + j] as i64 - borrow - (p & 0xFFFF_FFFF) as i64;
                u[i + j] = t as u32;
                borrow = if t < 0 { 1 } else { 0 };
            }
            let t = u[j + n] as i64 - borrow - carry as i64;
            u[j + n] = t as u32;
            if t < 0 {
                qhat -= 1;
                let mut c = 0u64;
                for i in 0..n {
                    let s2 = u[i + j] as u64 + v.limbs[i] as u64 + c;
                    u[i + j] = s2 as u32;
                    c = s2 >> 32;
                }
                u[j + n] = u[j + n].wrapping_add(c as u32);
            }
            q[j] = qhat as u32;
        }
        let mut qb = BigUint { limbs: q };
        qb.trim();
        u.truncate(n);
        let mut rb = BigUint { limbs: u };
        rb.trim();
        (qb, rb.shr(s as u64))
    }
    pub fn to_string_radix(&self, radix: u32) -> Vec<u8> {
        if self.is_zero() {
            return vec![b'0'];
        }
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        if radix.is_power_of_two() {
            let shift = radix.trailing_zeros() as u64;
            let mut out = Vec::new();
            let bits = self.bits();
            let mut pos = 0;
            while pos < bits {
                let mut v = 0u32;
                for k in 0..shift {
                    if self.bit(pos + k) {
                        v |= 1 << k;
                    }
                }
                out.push(digits[v as usize]);
                pos += shift;
            }
            while out.len() > 1 && *out.last().unwrap() == b'0' {
                out.pop();
            }
            out.reverse();
            return out;
        }
        // Chunked division: largest power of radix that fits a u32.
        let mut chunk = radix;
        let mut per = 1;
        while (chunk as u64) * (radix as u64) <= u32::MAX as u64 {
            chunk *= radix;
            per += 1;
        }
        let mut n = self.clone();
        let mut out = Vec::new();
        while !n.is_zero() {
            let mut r = n.div_small_assign(chunk);
            for _ in 0..per {
                out.push(digits[(r % radix) as usize]);
                r /= radix;
            }
        }
        while out.len() > 1 && *out.last().unwrap() == b'0' {
            out.pop();
        }
        out.reverse();
        out
    }
    /// Parse digits (already validated) in `radix`.
    pub fn from_digits(digits: &[u8], radix: u32) -> BigUint {
        let mut r = BigUint::zero();
        let mut chunk = 1u32;
        let mut acc = 0u32;
        for &d in digits {
            let v = match d {
                b'0'..=b'9' => d - b'0',
                b'a'..=b'z' => d - b'a' + 10,
                b'A'..=b'Z' => d - b'A' + 10,
                _ => continue,
            } as u32;
            if (chunk as u64) * (radix as u64) > u32::MAX as u64 {
                r.mul_small_add_assign(chunk, acc);
                chunk = 1;
                acc = 0;
            }
            chunk *= radix;
            acc = acc * radix + v;
        }
        if chunk > 1 {
            r.mul_small_add_assign(chunk, acc);
        }
        r
    }
    /// Correctly rounded (round-half-even) conversion to f64.
    pub fn to_f64(&self) -> f64 {
        let bits = self.bits();
        if bits == 0 {
            return 0.0;
        }
        if bits <= 64 {
            let v = self.low_u64();
            return round_u64_to_f64(v, false);
        }
        if bits > 1024 {
            return f64::INFINITY;
        }
        let shift = bits - 64;
        let top = self.shr(shift).low_u64();
        let sticky = self.low_bits_nonzero(shift);
        let f = round_u64_to_f64(top, sticky);
        f * pow2(shift as i32)
    }
}

pub fn pow2(e: i32) -> f64 {
    // Exact for the ranges we use (scaling by up to 2^1023 in steps).
    let mut r = 1.0f64;
    let mut e = e;
    while e > 1000 {
        r *= f64::from_bits(((1000 + 1023) as u64) << 52);
        e -= 1000;
    }
    while e < -1000 {
        r *= f64::from_bits(((-1000i32 + 1023) as u64) << 52);
        e += 1000;
    }
    r * f64::from_bits(((e + 1023) as u64) << 52)
}

/// Round a 64-bit integer (plus a sticky bit for discarded lower bits) to the nearest f64.
fn round_u64_to_f64(v: u64, sticky: bool) -> f64 {
    let lz = v.leading_zeros();
    let sig_bits = 64 - lz;
    if sig_bits <= 53 {
        if !sticky {
            return v as f64;
        }
        // Discarded bits below an exact value: they are less than half an ulp of the integer only when
        // the value has room; with up to 53 bits exact, sticky can only matter at the 64-bit window
        // boundary where sig_bits == 64. Here it does not change rounding.
        return v as f64;
    }
    let drop = sig_bits - 53;
    let mant = v >> drop;
    let rem = v & ((1u64 << drop) - 1);
    let half = 1u64 << (drop - 1);
    let round_up = rem > half || (rem == half && (sticky || mant & 1 == 1)) || (rem == half && sticky);
    let mut m = mant;
    let mut e = drop as i32;
    if round_up {
        m += 1;
        if m == 1 << 53 {
            m >>= 1;
            e += 1;
        }
    }
    (m as f64) * pow2(e)
}

fn karatsuba(a: &BigUint, b: &BigUint) -> BigUint {
    let n = a.limbs.len().max(b.limbs.len()) / 2;
    let split = |x: &BigUint| -> (BigUint, BigUint) {
        if x.limbs.len() <= n {
            return (x.clone(), BigUint::zero());
        }
        let mut lo = BigUint { limbs: x.limbs[..n].to_vec() };
        lo.trim();
        let hi = BigUint { limbs: x.limbs[n..].to_vec() };
        (lo, hi)
    };
    let (a0, a1) = split(a);
    let (b0, b1) = split(b);
    let z0 = a0.mul(&b0);
    let z2 = a1.mul(&b1);
    let z1 = a0.add(&a1).mul(&b0.add(&b1)).sub(&z0).sub(&z2);
    z0.add(&z1.shl(32 * n as u64)).add(&z2.shl(64 * n as u64))
}
