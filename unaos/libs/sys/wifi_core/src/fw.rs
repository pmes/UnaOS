// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The firmware container (`bcm4331.md` §S4 "Container header — W3, ANSWERED ON METAL").
//!
//! Every file carries one 8-byte header: `type` u8, `ver` u8, two reserved bytes, `be32 size`.
//! * `type` 0x75 — the microcode: `size` is the payload BYTE count and the payload is whole be32 words
//!   (measured: `ucode29_mimo.fw` 39760 bytes, size 39752).
//! * `type` 0x69 — both initvals files: `size` is the payload RECORD count; a record is `be16 offset`
//!   (bit 15 flags a 32-bit value) then a `be16` or `be32` value (measured: 477 and 35 records, each
//!   consuming its payload exactly).
//!
//! What a record MEANS to the core is `bcm4331.md` §S4-W5 fact 6 (`[SPEC-V3 InitialValues]`): one
//! per-register MMIO write into the 802.11 core's window, 16 or 32 bits wide, applied AFTER the
//! microcode reports Ready (`[SPEC-V3 ChipInit]` step 8). No Linux driver source was read.

use alloc::vec::Vec;

/// `type` byte of the microcode word-stream. `[METAL]` rmbp1-boot1.
pub const TYPE_UCODE: u8 = 0x75;
/// `type` byte of an initvals record-stream. `[METAL]` rmbp1-boot1.
pub const TYPE_INITVALS: u8 = 0x69;
/// Bit 15 of a record's offset: the value is 32 bits wide. `[METAL]` (the exact-consume walk).
pub const REC_WIDE: u16 = 0x8000;
/// The register-offset bits of a record.
pub const REC_OFFSET_MASK: u16 = 0x7FFF;

/// Why a container was refused. Every variant names the rule that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FwError {
    /// Fewer than the 8 header bytes.
    Short,
    /// The `type` byte is not the one this role needs.
    WrongType(u8),
    /// The microcode's declared byte count does not equal the payload length.
    SizeMismatch { declared: u32, payload: usize },
    /// The microcode payload is not whole 32-bit words, or is empty.
    NotWords,
    /// The record walk ran off the end of the payload (a trailing fragment).
    TruncatedRecord { at: usize },
    /// The walk consumed the payload exactly but found a different record count than declared.
    CountMismatch { declared: u32, walked: u32 },
}

impl FwError {
    /// A short token for a witness line.
    pub fn token(&self) -> &'static str {
        match self {
            FwError::Short => "shorter-than-header",
            FwError::WrongType(_) => "wrong-type",
            FwError::SizeMismatch { .. } => "size-mismatch",
            FwError::NotWords => "not-whole-words",
            FwError::TruncatedRecord { .. } => "truncated-record",
            FwError::CountMismatch { .. } => "count-mismatch",
        }
    }
}

/// The 8-byte header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: u8,
    pub ver: u8,
    pub declared: u32,
}

pub fn header(data: &[u8]) -> Result<Header, FwError> {
    if data.len() < 8 {
        return Err(FwError::Short);
    }
    Ok(Header {
        kind: data[0],
        ver: data[1],
        declared: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
    })
}

/// The microcode payload, verified: type 0x75, declared == payload bytes, whole non-empty words.
pub fn ucode_payload(data: &[u8]) -> Result<&[u8], FwError> {
    let h = header(data)?;
    if h.kind != TYPE_UCODE {
        return Err(FwError::WrongType(h.kind));
    }
    let payload = &data[8..];
    if h.declared as usize != payload.len() {
        return Err(FwError::SizeMismatch { declared: h.declared, payload: payload.len() });
    }
    if payload.is_empty() || payload.len() % 4 != 0 {
        return Err(FwError::NotWords);
    }
    Ok(payload)
}

/// The be32 words of a verified microcode payload, in stream order.
pub fn words(payload: &[u8]) -> impl Iterator<Item = u32> + '_ {
    payload.chunks_exact(4).map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
}

/// One initvals record: a register offset in the core window and the value to write there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub offset: u16,
    pub value: u32,
    /// 32-bit write (`true`) or 16-bit write (`false`).
    pub wide: bool,
}

/// Walk an initvals record-stream: type 0x69, a clean walk whose count equals the declared count.
pub fn records(data: &[u8]) -> Result<Vec<Record>, FwError> {
    let h = header(data)?;
    if h.kind != TYPE_INITVALS {
        return Err(FwError::WrongType(h.kind));
    }
    let p = &data[8..];
    let mut out = Vec::new();
    let mut off = 0usize;
    while off < p.len() {
        if off + 2 > p.len() {
            return Err(FwError::TruncatedRecord { at: off });
        }
        let raw = u16::from_be_bytes([p[off], p[off + 1]]);
        let wide = raw & REC_WIDE != 0;
        let need = if wide { 6 } else { 4 };
        if off + need > p.len() {
            return Err(FwError::TruncatedRecord { at: off });
        }
        let value = if wide {
            u32::from_be_bytes([p[off + 2], p[off + 3], p[off + 4], p[off + 5]])
        } else {
            u16::from_be_bytes([p[off + 2], p[off + 3]]) as u32
        };
        out.push(Record { offset: raw & REC_OFFSET_MASK, value, wide });
        off += need;
    }
    if out.len() as u32 != h.declared {
        return Err(FwError::CountMismatch { declared: h.declared, walked: out.len() as u32 });
    }
    Ok(out)
}

/// The microcode's self-published build date (`bcm4331.md` §S4a renders the shared word this way):
/// year `2000 + ((d >> 12) & 0xF)`, month `(d >> 8) & 0xF`, day `d & 0xFF`.
pub fn ucode_date(d: u16) -> (u16, u8, u8) {
    (2000 + ((d >> 12) & 0xF), ((d >> 8) & 0xF) as u8, (d & 0xFF) as u8)
}

/// The build time, same section: hour `(t >> 11) & 0x1F`, minute `(t >> 5) & 0x3F`, second `t & 0x1F`.
pub fn ucode_time(t: u16) -> (u8, u8, u8) {
    (((t >> 11) & 0x1F) as u8, ((t >> 5) & 0x3F) as u8, (t & 0x1F) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn ucode_rules() {
        let mut f = vec![0x75, 0x01, 0, 0, 0, 0, 0, 8];
        f.extend_from_slice(&[0x03, 0x00, 0x10, 0x4e, 0, 0, 0, 0]);
        let p = ucode_payload(&f).unwrap();
        let w: Vec<u32> = words(p).collect();
        assert_eq!(w, vec![0x0300_104e, 0]);
        f[7] = 12;
        assert!(matches!(ucode_payload(&f), Err(FwError::SizeMismatch { .. })));
        f[0] = 0x69;
        assert_eq!(ucode_payload(&f), Err(FwError::WrongType(0x69)));
        assert_eq!(ucode_payload(&[1, 2]), Err(FwError::Short));
    }

    #[test]
    fn record_walk() {
        // two records: 16-bit at 0x0120 = 0x0404, 32-bit at 0x0400 = 0xdeadbeef
        let f = vec![0x69, 1, 0, 0, 0, 0, 0, 2, 0x01, 0x20, 0x04, 0x04, 0x84, 0x00, 0xde, 0xad, 0xbe, 0xef];
        let r = records(&f).unwrap();
        assert_eq!(r[0], Record { offset: 0x120, value: 0x404, wide: false });
        assert_eq!(r[1], Record { offset: 0x400, value: 0xdead_beef, wide: true });
        let mut bad = f.clone();
        bad[7] = 3;
        assert!(matches!(records(&bad), Err(FwError::CountMismatch { declared: 3, walked: 2 })));
        let trunc = &f[..f.len() - 1];
        assert!(matches!(records(trunc), Err(FwError::TruncatedRecord { .. })));
    }

    #[test]
    fn metal_handshake_decodes() {
        // f20-boots.log: shm[0x4]=0xb217 shm[0x6]=0x09e7
        assert_eq!(ucode_date(0xb217), (2011, 2, 23));
        assert_eq!(ucode_time(0x09e7), (1, 15, 7));
    }
}
