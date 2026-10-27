// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria (A/V) — seam `shared-core` (hevc_core's SEI reader: the decoded picture hash, nothing else).
//!
//! HEVCCONF (LEDGER SR321): the decoded picture hash SEI message (7.3.5 sei_message, D.2.19 decoded_picture_hash,
//! payloadType 132 in a SUFFIX_SEI_NUT) and its check (D.3.19) against the decoded sample arrays after the in-loop
//! filters, the full pic_width x pic_height picture (not the conformance-window crop). The CRC and checksum variants are
//! computed here; MD5 needs a digest this zero-dependency crate does not carry and is refused BY NAME
//! ([`HashCheck::Md5Unchecked`], owed HEVCCONF-MD5 through crypto_core). SEI is non-normative: a malformed message never
//! fails a decode (the decoder drops it).

use crate::picture::Plane;
use crate::{Error, Result};
use PictureHash::{Checksum, Crc, Md5};

/// The decoded_picture_hash of one picture, per colour component (cIdx 0, 1, 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PictureHash {
    /// hash_type 0: picture_md5.
    Md5([[u8; 16]; 3]),
    /// hash_type 1: picture_crc.
    Crc([u16; 3]),
    /// hash_type 2: picture_checksum.
    Checksum([u32; 3]),
}

/// The verdict of [`check`] for one picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashCheck {
    /// Every component matches.
    Match,
    /// The first component (cIdx) whose hash differs.
    Mismatch(usize),
    /// An MD5 hash: refused by name (HEVCCONF-MD5), not compared.
    Md5Unchecked,
}

/// The decoded_picture_hash (payloadType 132) of a SEI RBSP (7.3.2.4 sei_rbsp: sei_message()s until the trailing bits),
/// `None` when the RBSP carries none or a reserved hash_type. Three components (hevc_core decodes 4:2:0 only).
pub fn picture_hash(rbsp: &[u8]) -> Result<Option<PictureHash>> {
    let mut i = 0;
    // more_rbsp_data(): stop at the rbsp_stop_one_bit byte followed only by zeros.
    while i < rbsp.len() && !(rbsp[i] == 0x80 && rbsp[i + 1..].iter().all(|&b| b == 0)) {
        let ff = |i: &mut usize| -> Result<usize> {
            let mut v = 0;
            loop {
                let b = *rbsp.get(*i).ok_or(Error::Truncated)?;
                *i += 1;
                v += b as usize;
                if b != 0xFF {
                    return Ok(v);
                }
            }
        };
        let payload_type = ff(&mut i)?;
        let payload_size = ff(&mut i)?;
        let payload = rbsp.get(i..i + payload_size).ok_or(Error::Truncated)?;
        i += payload_size;
        if payload_type == 132 {
            return decoded_picture_hash(payload);
        }
    }
    Ok(None)
}

/// D.2.19 decoded_picture_hash( payloadSize ).
fn decoded_picture_hash(p: &[u8]) -> Result<Option<PictureHash>> {
    let (&hash_type, rest) = p.split_first().ok_or(Error::Truncated)?;
    let n = match hash_type {
        0 => 16,
        1 => 2,
        2 => 4,
        _ => return Ok(None),
    };
    if rest.len() < 3 * n {
        return Err(Error::Truncated);
    }
    let c = |k: usize| &rest[k * n..(k + 1) * n];
    Ok(Some(match hash_type {
        0 => Md5([0, 1, 2].map(|k| c(k).try_into().unwrap())),
        1 => Crc([0, 1, 2].map(|k| u16::from_be_bytes(c(k).try_into().unwrap()))),
        _ => Checksum([0, 1, 2].map(|k| u32::from_be_bytes(c(k).try_into().unwrap()))),
    }))
}

/// D.3.19 pictureData: one byte per sample at bitDepth 8, two (low byte first) above.
fn for_each_byte(p: &Plane, bit_depth: u32, mut f: impl FnMut(u8)) {
    for &v in &p.data {
        f(v as u8);
        if bit_depth > 8 {
            f((v >> 8) as u8);
        }
    }
}

/// D.3.19 picture_crc of one component: the bitwise CRC (polynomial 0x1021, initial 0xFFFF) of pictureData followed
/// by two zero bytes (the CRC-16/AUG-CCITT form: "123456789" gives 0xE5CC).
pub fn plane_crc(p: &Plane, bit_depth: u32) -> u16 {
    fn feed(crc: &mut u16, byte: u8) {
        for k in 0..8 {
            let msb = (*crc >> 15) & 1;
            let bit = ((byte >> (7 - k)) & 1) as u16;
            *crc = ((*crc << 1) | bit) ^ (msb * 0x1021);
        }
    }
    let mut crc = 0xFFFFu16;
    for_each_byte(p, bit_depth, |b| feed(&mut crc, b));
    feed(&mut crc, 0);
    feed(&mut crc, 0);
    crc
}

/// D.3.19 picture_checksum of one component: the sum of every sample byte XORed with
/// xorMask = (x & 0xFF) ^ (y & 0xFF) ^ (x >> 8) ^ (y >> 8), modulo 2^32.
pub fn plane_checksum(p: &Plane, bit_depth: u32) -> u32 {
    let mut sum = 0u32;
    for y in 0..p.h {
        for x in 0..p.w {
            let mask = ((x & 0xFF) ^ (y & 0xFF) ^ (x >> 8) ^ (y >> 8)) as u32;
            let v = p.data[y * p.w + x] as u32;
            sum = sum.wrapping_add((v & 0xFF) ^ mask);
            if bit_depth > 8 {
                sum = sum.wrapping_add((v >> 8) ^ mask);
            }
        }
    }
    sum
}

/// Check a picture's decoded sample arrays (after deblocking and SAO) against its decoded_picture_hash. HEVCREXT
/// (SR334): D.3.19's bitDepth is per component — BitDepthY for plane 0, BitDepthC (`bit_depth_c`) for planes 1 and 2.
pub fn check(planes: &[Plane; 3], bit_depth: u32, bit_depth_c: u32, h: &PictureHash) -> HashCheck {
    for c in 0..3 {
        let bd = if c == 0 { bit_depth } else { bit_depth_c };
        let ok = match h {
            Md5(_) => return HashCheck::Md5Unchecked,
            Crc(want) => plane_crc(&planes[c], bd) == want[c],
            Checksum(want) => plane_checksum(&planes[c], bd) == want[c],
        };
        if !ok {
            return HashCheck::Mismatch(c);
        }
    }
    HashCheck::Match
}
