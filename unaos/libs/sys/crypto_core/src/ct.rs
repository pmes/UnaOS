// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Constant-time helpers and zeroization — ours, no `subtle`, no `zeroize` crate.
//!
//! A secret-dependent decision is carried as a [`Choice`] (0 or 1 in a `u8`) and turned into an
//! all-ones/all-zeros mask; nothing here branches on it. `core::hint::black_box` is the optimisation
//! barrier that keeps LLVM from recognising the mask arithmetic as a boolean and re-introducing a branch
//! (the same barrier `subtle` uses on stable Rust). It is a best-effort barrier, as it is everywhere in
//! Rust: the guarantee is in the source shape, checked by reading the generated code on the targets we
//! ship (x86_64, AArch64), not by the language.

use core::hint::black_box;

/// A secret boolean: 0 or 1, never branched on.
#[derive(Clone, Copy, Debug)]
pub struct Choice(u8);

impl Choice {
    /// From 0/1. Constant-time. Any other value is a caller bug (masked to its low bit).
    #[inline]
    pub fn from_u8(b: u8) -> Self {
        Choice(black_box(b & 1))
    }
    /// 0 or 1.
    #[inline]
    pub fn unwrap_u8(self) -> u8 {
        self.0
    }
    /// Declassify: turn the choice into a `bool` the caller may branch on. Only for results that are
    /// PUBLIC by protocol (did the tag verify, is the point valid).
    #[inline]
    pub fn into_bool(self) -> bool {
        black_box(self.0) == 1
    }
    /// 0xFF..FF when set, 0 otherwise.
    #[inline]
    pub fn mask_u64(self) -> u64 {
        (black_box(self.0) as u64).wrapping_neg()
    }
    /// 0xFFFFFFFF when set, 0 otherwise.
    #[inline]
    pub fn mask_u32(self) -> u32 {
        (black_box(self.0) as u32).wrapping_neg()
    }
    /// 0xFF when set, 0 otherwise.
    #[inline]
    pub fn mask_u8(self) -> u8 {
        black_box(self.0).wrapping_neg()
    }
}

impl core::ops::BitAnd for Choice {
    type Output = Choice;
    fn bitand(self, rhs: Choice) -> Choice {
        Choice(self.0 & rhs.0)
    }
}
impl core::ops::BitOr for Choice {
    type Output = Choice;
    fn bitor(self, rhs: Choice) -> Choice {
        Choice(self.0 | rhs.0)
    }
}
impl core::ops::Not for Choice {
    type Output = Choice;
    fn not(self) -> Choice {
        Choice(self.0 ^ 1)
    }
}

/// 1 when `x == 0`. Constant-time (no branch, no comparison instruction on the secret).
#[inline]
pub fn is_zero_u64(x: u64) -> Choice {
    let x = black_box(x);
    // (x | -x) has its top bit set iff x != 0.
    Choice::from_u8((((x | x.wrapping_neg()) >> 63) as u8) ^ 1)
}

/// 1 when `a == b`. Constant-time.
#[inline]
pub fn eq_u64(a: u64, b: u64) -> Choice {
    is_zero_u64(a ^ b)
}

/// 1 when `a < b` (unsigned). Constant-time: the borrow of `a - b`.
#[inline]
pub fn lt_u64(a: u64, b: u64) -> Choice {
    let (_, borrow) = black_box(a).overflowing_sub(black_box(b));
    Choice::from_u8(borrow as u8)
}

/// Byte-slice equality. Constant-time in the CONTENTS; the lengths are public (unequal lengths return
/// `false` at once — a length is never the secret, a MAC's length is fixed by its algorithm).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    ct_eq_choice(a, b).into_bool()
}

/// [`ct_eq`] as a [`Choice`], for callers that keep combining before declassifying.
pub fn ct_eq_choice(a: &[u8], b: &[u8]) -> Choice {
    if a.len() != b.len() {
        return Choice::from_u8(0);
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    is_zero_u64(acc as u64)
}

/// `if choice { a } else { b }` on a `u64`. Constant-time.
#[inline]
pub fn ct_select_u64(choice: Choice, a: u64, b: u64) -> u64 {
    let m = choice.mask_u64();
    (a & m) | (b & !m)
}

/// `if choice { a } else { b }` on a `u32`. Constant-time.
#[inline]
pub fn ct_select_u32(choice: Choice, a: u32, b: u32) -> u32 {
    let m = choice.mask_u32();
    (a & m) | (b & !m)
}

/// `if choice { a } else { b }` on a `u8`. Constant-time.
#[inline]
pub fn ct_select_u8(choice: Choice, a: u8, b: u8) -> u8 {
    let m = choice.mask_u8();
    (a & m) | (b & !m)
}

/// `out = if choice { a } else { b }`, byte-wise over equal-length slices. Constant-time in the contents
/// and in `choice`; panics if the lengths differ (lengths are public).
pub fn ct_select(choice: Choice, a: &[u8], b: &[u8], out: &mut [u8]) {
    assert!(a.len() == b.len() && b.len() == out.len());
    let m = choice.mask_u8();
    for i in 0..out.len() {
        out[i] = (a[i] & m) | (b[i] & !m);
    }
}

/// `if choice { dst = src }`. Constant-time.
pub fn ct_assign(choice: Choice, dst: &mut [u8], src: &[u8]) {
    assert!(dst.len() == src.len());
    let m = choice.mask_u8();
    for i in 0..dst.len() {
        dst[i] ^= (dst[i] ^ src[i]) & m;
    }
}

/// `if choice { swap(a, b) }` on `u64` limbs. Constant-time.
#[inline]
pub fn ct_swap_u64(choice: Choice, a: &mut [u64], b: &mut [u64]) {
    let m = choice.mask_u64();
    for i in 0..a.len() {
        let t = (a[i] ^ b[i]) & m;
        a[i] ^= t;
        b[i] ^= t;
    }
}

/// Erase memory in a way the optimiser may not remove: a volatile write per element, then a compiler
/// fence so later code cannot be reordered above the wipe. Implemented by every key-bearing type's `Drop`.
pub trait Zeroize {
    /// Overwrite with zeros.
    fn zeroize(&mut self);
}

#[allow(unsafe_code)]
fn volatile_zero<T: Copy + Default>(s: &mut [T]) {
    for x in s.iter_mut() {
        // SAFETY: `x` is a valid, aligned, exclusive reference for the duration of the write.
        unsafe { core::ptr::write_volatile(x as *mut T, T::default()) };
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

impl Zeroize for [u8] {
    fn zeroize(&mut self) {
        volatile_zero(self)
    }
}
impl Zeroize for [u32] {
    fn zeroize(&mut self) {
        volatile_zero(self)
    }
}
impl Zeroize for [u64] {
    fn zeroize(&mut self) {
        volatile_zero(self)
    }
}
impl<const N: usize> Zeroize for [u8; N] {
    fn zeroize(&mut self) {
        volatile_zero(&mut self[..])
    }
}
impl<const N: usize> Zeroize for [u32; N] {
    fn zeroize(&mut self) {
        volatile_zero(&mut self[..])
    }
}
impl<const N: usize> Zeroize for [u64; N] {
    fn zeroize(&mut self) {
        volatile_zero(&mut self[..])
    }
}
#[cfg(feature = "alloc")]
impl Zeroize for alloc::vec::Vec<u8> {
    fn zeroize(&mut self) {
        volatile_zero(&mut self[..])
    }
}

/// A fixed-size secret byte array that zeroizes itself on drop. `Deref`s to the bytes.
#[derive(Clone)]
pub struct SecretBytes<const N: usize>(pub [u8; N]);

impl<const N: usize> Drop for SecretBytes<N> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
impl<const N: usize> core::ops::Deref for SecretBytes<N> {
    type Target = [u8; N];
    fn deref(&self) -> &[u8; N] {
        &self.0
    }
}
impl<const N: usize> core::fmt::Debug for SecretBytes<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SecretBytes<{}>(..)", N)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eq_select() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert_eq!(ct_select_u64(Choice::from_u8(1), 5, 7), 5);
        assert_eq!(ct_select_u64(Choice::from_u8(0), 5, 7), 7);
        assert_eq!(is_zero_u64(0).unwrap_u8(), 1);
        assert_eq!(is_zero_u64(1 << 63).unwrap_u8(), 0);
        assert_eq!(lt_u64(3, 4).unwrap_u8(), 1);
        assert_eq!(lt_u64(4, 4).unwrap_u8(), 0);
        let mut a = [1u64, 2];
        let mut b = [3u64, 4];
        ct_swap_u64(Choice::from_u8(1), &mut a, &mut b);
        assert_eq!((a, b), ([3, 4], [1, 2]));
        let mut k = [9u8; 8];
        k.zeroize();
        assert_eq!(k, [0; 8]);
    }
}
