// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! White balance as the file states it (RAWVIEW, rmbp-ledger B711): per-channel multipliers, green = [`ONE`].
//!
//! Two sources, in this order:
//!
//! * **AsShotNeutral** (tag 50728, DNG 1.6 §6.3 — three RATIONALs, the neutral in camera RGB): the multiplier of
//!   a channel is the inverse of its neutral, normalised so green is one.
//! * **Sony's SR2 levels**: IFD0's `DNGPrivateData` (tag 50740) points at the SR2Private IFD, whose tags 0x7200 /
//!   0x7201 / 0x7221 name the offset, length and key of the ENCRYPTED SR2SubIFD; decrypted (dcraw's
//!   `sony_decrypt`), that IFD holds `WB_RGGBLevels` (0x7313, read as R G G B) or, on older bodies,
//!   `WB_GRBGLevels` (0x7303, read as G R B G) — the camera's as-shot levels; dcraw's `cam_mul`.
//!
//! Neither present: `None`, and the develop uses unity gains (`wb=none` on the wire). Every offset is checked
//! against the slice; the decrypted block is bounded ([`MAX_SR2`]) and reserved fallibly.

use alloc::vec::Vec;

use crate::tiff::{Entry, Reader};

pub mod tag {
    pub const AS_SHOT_NEUTRAL: u16 = 50728;
    pub const DNG_PRIVATE: u16 = 50740;
    pub const SR2_OFFSET: u16 = 0x7200;
    pub const SR2_LENGTH: u16 = 0x7201;
    pub const SR2_KEY: u16 = 0x7221;
    pub const WB_GRBG: u16 = 0x7303;
    pub const WB_RGGB: u16 = 0x7313;
}

/// Green's multiplier: multipliers are Q10 fixed point.
pub const ONE: u32 = 1024;
/// Largest SR2SubIFD decrypted (real files carry a few KiB).
pub const MAX_SR2: usize = 1 << 20;

/// Where the white balance came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WbSource {
    AsShotNeutral,
    Sony,
}

/// Per-channel multipliers R, G, B in Q10, green = [`ONE`]; each within `ONE/16 ..= ONE*16`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wb {
    pub mul: [u32; 3],
    pub src: WbSource,
}

impl Wb {
    /// From camera levels (larger level = larger gain), green normalised to [`ONE`].
    pub fn from_levels(r: u32, g: u32, b: u32, src: WbSource) -> Option<Wb> {
        if r == 0 || g == 0 || b == 0 {
            return None;
        }
        let q = |c: u32| ((c as u64 * ONE as u64 + g as u64 / 2) / g as u64) as u32;
        Wb::fenced([q(r), ONE, q(b)], src)
    }

    /// From AsShotNeutral's three rationals: the gain of channel `c` is `neutral[G] / neutral[c]`.
    pub fn from_neutral(n: [(u32, u32); 3]) -> Option<Wb> {
        if n.iter().any(|&(num, den)| num == 0 || den == 0) {
            return None;
        }
        let (gn, gd) = (n[1].0 as u64, n[1].1 as u64);
        let q = |(cn, cd): (u32, u32)| {
            let num = gn * cd as u64 * ONE as u64;
            let den = gd * cn as u64;
            ((num + den / 2) / den).min(u32::MAX as u64) as u32
        };
        Wb::fenced([q(n[0]), ONE, q(n[2])], WbSource::AsShotNeutral)
    }

    fn fenced(mul: [u32; 3], src: WbSource) -> Option<Wb> {
        mul.iter().all(|&m| (ONE / 16..=ONE * 16).contains(&m)).then_some(Wb { mul, src })
    }

    /// The wire word: `asshot`, `sony` or `none`.
    pub fn name(wb: Option<&Wb>) -> &'static str {
        match wb.map(|w| w.src) {
            Some(WbSource::AsShotNeutral) => "asshot",
            Some(WbSource::Sony) => "sony",
            None => "none",
        }
    }
}

/// dcraw's `sony_decrypt` over a whole block (it is an XOR stream: the same call encrypts). The pad is a
/// 127-word lagged generator seeded from `key`; each 32-bit word of `buf` is XORed with the next pad word in
/// big-endian byte order (dcraw's `htonl`). A trailing partial word is left as is.
pub fn sony_decrypt(buf: &mut [u8], key: u32) {
    let mut pad = [0u32; 128];
    let mut k = key;
    for p in pad.iter_mut().take(4) {
        k = k.wrapping_mul(48_828_125).wrapping_add(1);
        *p = k;
    }
    pad[3] = pad[3] << 1 | (pad[0] ^ pad[2]) >> 31;
    for p in 4..127 {
        pad[p] = (pad[p - 4] ^ pad[p - 2]) << 1 | (pad[p - 3] ^ pad[p - 1]) >> 31;
    }
    let mut p = 127usize;
    for word in buf.chunks_exact_mut(4) {
        p += 1;
        let v = pad[p & 127] ^ pad[(p + 64) & 127];
        pad[(p - 1) & 127] = v;
        for (b, x) in word.iter_mut().zip(v.to_be_bytes()) {
            *b ^= x;
        }
    }
}

/// A 32-bit pointer or key held by `e`: a SHORT/LONG value, or the first four bytes of a BYTE/UNDEFINED field.
fn word(r: &Reader, e: &Entry) -> Option<u32> {
    match e.typ {
        3 | 4 | 13 => r.uint(e, 0),
        1 | 7 if e.count >= 4 => {
            let (s, _) = r.value_span(e)?;
            r.u32(s)
        }
        _ => None,
    }
}

/// AsShotNeutral's three rationals.
pub fn neutral(r: &Reader, e: &Entry) -> Option<[(u32, u32); 3]> {
    if e.typ != 5 || e.count != 3 {
        return None;
    }
    let (s, _) = r.value_span(e)?;
    let mut n = [(0u32, 0u32); 3];
    for (i, v) in n.iter_mut().enumerate() {
        *v = (r.u32(s + 8 * i)?, r.u32(s + 8 * i + 4)?);
    }
    Some(n)
}

/// The DNGPrivateData entry `e` of IFD0 -> the SR2Private IFD -> the decrypted SR2SubIFD -> the levels.
pub fn sony_sr2(r: &Reader, e: &Entry) -> Option<Wb> {
    let at = word(r, e)? as usize;
    let (ents, _) = r.ifd(at).ok()?;
    let get = |t: u16| ents.iter().find(|x| x.tag == t).and_then(|x| word(r, x));
    let (off, len, key) = (get(tag::SR2_OFFSET)? as usize, get(tag::SR2_LENGTH)? as usize, get(tag::SR2_KEY)?);
    if !(18..=MAX_SR2).contains(&len) {
        return None;
    }
    let src = r.b.get(off..off.checked_add(len)?)?;
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve_exact(len).ok()?;
    buf.extend_from_slice(src);
    sony_decrypt(&mut buf, key);
    sr2_levels(&buf, off, r.le)
}

/// Read the decrypted SR2SubIFD (an IFD at the block's start whose out-of-line offsets are FILE offsets; the
/// block sits at file offset `base`).
fn sr2_levels(b: &[u8], base: usize, le: bool) -> Option<Wb> {
    let r = Reader { b, le };
    let n = r.u16(0)? as usize;
    if n == 0 || n > 512 {
        return None;
    }
    let (mut rggb, mut grbg) = (None, None);
    for i in 0..n {
        let e = 2 + i * 12;
        let (t, typ, cnt) = (r.u16(e)?, r.u16(e + 2)?, r.u32(e + 4)?);
        if !(t == tag::WB_RGGB || t == tag::WB_GRBG) || typ != 3 || cnt < 4 {
            continue;
        }
        // Four SHORTs are eight bytes: always out of line.
        let Some(at) = (r.u32(e + 8)? as usize).checked_sub(base) else { continue };
        let v = [r.u16(at), r.u16(at + 2), r.u16(at + 4), r.u16(at + 6)];
        let Some(v) = (v.iter().all(|x| x.is_some())).then(|| v.map(|x| x.unwrap_or(0) as u32)) else { continue };
        if t == tag::WB_RGGB {
            rggb = Some(v);
        } else {
            grbg = Some(v);
        }
    }
    if let Some([r0, g1, g2, b3]) = rggb {
        return Wb::from_levels(r0, (g1 + g2) / 2, b3, WbSource::Sony);
    }
    let [g0, r1, b2, g3] = grbg?;
    Wb::from_levels(r1, (g0 + g3) / 2, b2, WbSource::Sony)
}
