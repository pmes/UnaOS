// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `usbnet_core` — the receive framing of the USB Ethernet parts UnaOS drives (NETFRAME, rmbp-ledger B368).
//!
//! CHARTER: Kernel — shared-core. The kernel's `drivers/xhci/usbnet.rs` splits every bulk-IN completion with
//! THIS code and the host runs the known-answer tests on the same code (`cargo test -p usbnet_core`), so the
//! parse the metal runs is the parse the tests pin.
//!
//! * [`ax88179`] — ASIX AX88179/AX88179A/B (VID 0b95, PID 1790): N packets then a TRAILING header block.
//! * [`rtl8153`] — Realtek RTL8152/8153 vendor mode (VID 0bda): a LEADING 24-byte descriptor per packet.
//!
//! Both splitters are pure: they take the completion's bytes and call back once per packet with a
//! [`Pkt`] verdict naming an `(offset, length)` view into the same buffer. No allocation, no state.
//!
//! SOURCES: the parts' datasheet framing facts (data only), recorded in
//! `docs/dev/evidence/rmbp-1005/NETFRAME.md`, and the tree's own metal captures (flight 19's alignment dummy
//! header `0x80008000`, flight 22's one-packet completions).
#![no_std]
#![forbid(unsafe_code)]

/// Smallest Ethernet frame header (dst, src, ethertype) — a "frame" shorter than this is not one.
pub const ETH_HDR: usize = 14;

/// One packet's verdict from a splitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pkt {
    /// A good frame at `buf[off..off + len]` (padding and FCS already removed).
    Frame { off: usize, len: usize },
    /// The AX88179 alignment dummy header (length 0) — skipped, never a drop.
    Pad,
    /// The chip flagged this packet (`crc`: FCS error; else the drop bit). `hdr` is the raw header word.
    Flagged { hdr: u32, crc: bool, off: usize, len: usize },
}

/// Why a whole completion was refused (no further packet of it is delivered).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Under the 4-byte trailer: a ZLP or a runt.
    Empty,
    /// The trailer's header block does not fit the completion, or `pkt_cnt` is 0.
    Geometry,
    /// A packet's length runs past the packet area or is shorter than a frame header.
    PktLen,
}

/// What a split did: packets seen (of every kind) and, if it stopped early, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Split {
    pub packets: usize,
    pub refused: Option<Refusal>,
}

#[inline]
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[inline]
fn align8(n: usize) -> usize {
    (n + 7) & !7
}

pub mod ax88179 {
    //! AX88179 bulk-IN framing.
    //!
    //! The LAST 4 bytes of the transfer are the receive header: `pkt_cnt` in bits 0..15, `hdr_off` in bits
    //! 16..31. At `hdr_off` sit `pkt_cnt` little-endian u32 packet headers. Packet `i`'s length is bits
    //! 16..28 of its header (13 bits); bit 29 is the CRC error, bit 31 the drop error. Packets are laid out
    //! from offset 0, each occupying its length rounded up to 8 bytes. With RX_CTL.IPE set the chip puts
    //! 2 alignment bytes ahead of every frame, counted in the length. A header of length 0 is the alignment
    //! DUMMY that follows each real header on this part (flight 19: `0x80008000`) and carries no packet.
    use super::{Pkt, Refusal, Split, ETH_HDR, align8, le32};

    pub const HDR_CRC_ERR: u32 = 1 << 29;
    pub const HDR_DROP_ERR: u32 = 1 << 31;
    /// RX_CTL.IPE: the 2 bytes ahead of every frame.
    pub const IPE_PAD: usize = 2;

    /// The trailer of a completion `buf`: `(rx_hdr, pkt_cnt, hdr_off)`, or `None` under 4 bytes.
    pub fn trailer(buf: &[u8]) -> Option<(u32, usize, usize)> {
        let n = buf.len();
        if n < 4 {
            return None;
        }
        let h = le32(buf, n - 4);
        Some((h, (h & 0xffff) as usize, (h >> 16) as usize))
    }

    /// Length (bits 16..28) of one packet header.
    pub fn pkt_len(hdr: u32) -> usize {
        ((hdr >> 16) & 0x1fff) as usize
    }

    /// Split one completion. `pad` is the per-frame alignment (`IPE_PAD` when RX_CTL.IPE is set, else 0).
    pub fn split(buf: &[u8], pad: usize, mut f: impl FnMut(Pkt)) -> Split {
        let n = buf.len();
        let Some((_, pkt_cnt, hdr_off)) = trailer(buf) else {
            return Split { packets: 0, refused: Some(Refusal::Empty) };
        };
        if pkt_cnt == 0 || hdr_off.checked_add(pkt_cnt * 4).is_none_or(|e| e > n) || hdr_off + 4 > n {
            return Split { packets: 0, refused: Some(Refusal::Geometry) };
        }
        let mut off = 0usize;
        for i in 0..pkt_cnt {
            let hdr = le32(buf, hdr_off + i * 4);
            let len = pkt_len(hdr);
            if len == 0 {
                f(Pkt::Pad);
                continue;
            }
            if hdr & (HDR_DROP_ERR | HDR_CRC_ERR) != 0 && off + len <= hdr_off {
                f(Pkt::Flagged { hdr, crc: hdr & HDR_DROP_ERR == 0, off, len });
                off += align8(len);
                continue;
            }
            if len < pad + ETH_HDR || off + len > hdr_off {
                return Split { packets: i, refused: Some(Refusal::PktLen) };
            }
            f(Pkt::Frame { off: off + pad, len: len - pad });
            off += align8(len);
        }
        Split { packets: pkt_cnt, refused: None }
    }
}

pub mod rtl8153 {
    //! RTL8152/RTL8153 vendor-mode bulk-IN framing.
    //!
    //! Each packet is a 24-byte receive descriptor (six little-endian u32, `opts1`..`opts6`) followed by
    //! the frame. `opts1` bits 0..14 are the packet length INCLUDING the 4-byte FCS. The next descriptor
    //! starts at the 8-byte-aligned offset after the frame. A descriptor whose length is 0, or one that
    //! would run past the completion, ends the split (the remaining bytes are the chip's tail padding).
    use super::{Pkt, Refusal, Split, ETH_HDR, align8, le32};

    pub const DESC_LEN: usize = 24;
    pub const FCS: usize = 4;
    pub const LEN_MASK: u32 = 0x7fff;

    /// Split one completion.
    pub fn split(buf: &[u8], mut f: impl FnMut(Pkt)) -> Split {
        let n = buf.len();
        if n < DESC_LEN {
            return Split { packets: 0, refused: Some(Refusal::Empty) };
        }
        let mut off = 0usize;
        let mut packets = 0usize;
        while off + DESC_LEN <= n {
            let len = (le32(buf, off) & LEN_MASK) as usize;
            if len == 0 {
                break;
            }
            let start = off + DESC_LEN;
            if len < ETH_HDR + FCS || start + len > n {
                return Split { packets, refused: Some(Refusal::PktLen) };
            }
            f(Pkt::Frame { off: start, len: len - FCS });
            packets += 1;
            off = align8(start + len);
        }
        Split { packets, refused: None }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn eth(et: u16, body: usize) -> Vec<u8> {
        let mut f = std::vec![0xffu8; 6];
        f.extend_from_slice(&[0x9c, 0x69, 0xd3, 0x28, 0x6e, 0xf4]);
        f.extend_from_slice(&et.to_be_bytes());
        f.extend((0..body).map(|i| i as u8));
        f
    }

    /// Build an AX88179 completion: packets (with `pad` alignment bytes ahead), a dummy header after each
    /// real one when `dummies`, then the header block and the trailer.
    fn ax_build(frames: &[(&[u8], u32)], pad: usize, dummies: bool) -> Vec<u8> {
        let mut b = Vec::new();
        let mut hdrs = Vec::new();
        for (fr, flags) in frames {
            let len = fr.len() + pad;
            b.extend(std::iter::repeat_n(0u8, pad));
            b.extend_from_slice(fr);
            while b.len() % 8 != 0 {
                b.push(0);
            }
            hdrs.push(((len as u32) << 16) | flags);
            if dummies {
                hdrs.push(0x8000_8000);
            }
        }
        let hdr_off = b.len();
        for h in &hdrs {
            b.extend_from_slice(&h.to_le_bytes());
        }
        let tr = ((hdr_off as u32) << 16) | hdrs.len() as u32;
        b.extend_from_slice(&tr.to_le_bytes());
        b
    }

    fn ax_collect(buf: &[u8]) -> (Split, Vec<Pkt>) {
        let mut v = Vec::new();
        let s = ax88179::split(buf, ax88179::IPE_PAD, |p| v.push(p));
        (s, v)
    }

    #[test]
    fn ax_flight22_shape_one_frame_one_dummy() {
        // Flight 22: every completion = one real packet + one alignment dummy (rx_pad == rx_ok == 35).
        let f = eth(0x0800, 60);
        let b = ax_build(&[(&f, 0)], 2, true);
        let (s, v) = ax_collect(&b);
        assert_eq!(s, Split { packets: 2, refused: None });
        assert_eq!(v.len(), 2);
        match v[0] {
            Pkt::Frame { off, len } => {
                assert_eq!(&b[off..off + len], &f[..]);
                assert_eq!(u16::from_be_bytes([b[off + 12], b[off + 13]]), 0x0800);
            }
            p => panic!("{p:?}"),
        }
        assert_eq!(v[1], Pkt::Pad);
    }

    #[test]
    fn ax_multi_frame_8_byte_stride() {
        let a = eth(0x0806, 28);
        let c = eth(0x86dd, 77);
        let d = eth(0x0800, 1486);
        let b = ax_build(&[(&a, 0), (&c, 0), (&d, 0)], 2, false);
        let (s, v) = ax_collect(&b);
        assert_eq!(s.refused, None);
        let got: Vec<&[u8]> = v.iter().map(|p| match *p { Pkt::Frame { off, len } => &b[off..off + len], _ => panic!() }).collect();
        assert_eq!(got, [&a[..], &c[..], &d[..]]);
    }

    #[test]
    fn ax_flags_skip_one_packet_only() {
        let a = eth(0x0800, 40);
        let c = eth(0x0800, 41);
        let b = ax_build(&[(&a, ax88179::HDR_CRC_ERR), (&c, 0)], 2, true);
        let (s, v) = ax_collect(&b);
        assert_eq!(s.refused, None);
        assert!(matches!(v[0], Pkt::Flagged { crc: true, .. }));
        assert_eq!(v[1], Pkt::Pad);
        match v[2] { Pkt::Frame { off, len } => assert_eq!(&b[off..off + len], &c[..]), p => panic!("{p:?}") }
        let b2 = ax_build(&[(&a, ax88179::HDR_DROP_ERR)], 2, false);
        assert!(matches!(ax_collect(&b2).1[0], Pkt::Flagged { crc: false, .. }));
    }

    #[test]
    fn ax_no_ipe_pad() {
        let a = eth(0x0800, 50);
        let b = ax_build(&[(&a, 0)], 0, false);
        let mut v = Vec::new();
        ax88179::split(&b, 0, |p| v.push(p));
        match v[0] { Pkt::Frame { off, len } => assert_eq!(&b[off..off + len], &a[..]), p => panic!("{p:?}") }
    }

    #[test]
    fn ax_refusals() {
        assert_eq!(ax_collect(&[]).0.refused, Some(Refusal::Empty));
        assert_eq!(ax_collect(&[1, 2, 3]).0.refused, Some(Refusal::Empty));
        // pkt_cnt 0
        assert_eq!(ax_collect(&[0, 0, 0, 0]).0.refused, Some(Refusal::Geometry));
        // hdr_off past the end
        let tr = (4000u32 << 16) | 1;
        assert_eq!(ax_collect(&tr.to_le_bytes()).0.refused, Some(Refusal::Geometry));
        // a length running into the header block
        let mut b = std::vec![0u8; 16];
        b.extend_from_slice(&((200u32) << 16).to_le_bytes());
        b.extend_from_slice(&((16u32 << 16) | 1).to_le_bytes());
        assert_eq!(ax_collect(&b).0.refused, Some(Refusal::PktLen));
        // a 20 KiB buffer read as RTL framing would be garbage; read as AX it is exact — the framing is per chip.
    }

    #[test]
    fn ax_trailer_fields() {
        let b = ax_build(&[(&eth(0x0800, 46), 0)], 2, true);
        let (h, cnt, off) = ax88179::trailer(&b).unwrap();
        assert_eq!(cnt, 2);
        assert_eq!(off, 64);
        assert_eq!(h, (64 << 16) | 2);
        assert_eq!(ax88179::pkt_len(0x8000_8000), 0);
        assert_eq!(ax88179::pkt_len(0x0042_0000), 0x42);
    }

    fn rtl_build(frames: &[&[u8]], tail: usize) -> Vec<u8> {
        let mut b = Vec::new();
        for fr in frames {
            let len = (fr.len() + rtl8153::FCS) as u32;
            b.extend_from_slice(&(len | (1 << 24)).to_le_bytes()); // opts1: length + an unrelated flag bit
            b.extend(std::iter::repeat_n(0u8, rtl8153::DESC_LEN - 4));
            b.extend_from_slice(fr);
            b.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]); // FCS
            while b.len() % 8 != 0 {
                b.push(0);
            }
        }
        b.extend(std::iter::repeat_n(0u8, tail));
        b
    }

    #[test]
    fn rtl_multi_frame_strips_desc_and_fcs() {
        let a = eth(0x0800, 46);
        let c = eth(0x86dd, 101);
        let b = rtl_build(&[&a, &c], 0);
        let mut v = Vec::new();
        let s = rtl8153::split(&b, |p| v.push(p));
        assert_eq!(s, Split { packets: 2, refused: None });
        let got: Vec<&[u8]> = v.iter().map(|p| match *p { Pkt::Frame { off, len } => &b[off..off + len], _ => panic!() }).collect();
        assert_eq!(got, [&a[..], &c[..]]);
    }

    #[test]
    fn rtl_zero_tail_and_runts() {
        let a = eth(0x0806, 28);
        let b = rtl_build(&[&a], 48);
        let mut n = 0;
        assert_eq!(rtl8153::split(&b, |_| n += 1), Split { packets: 1, refused: None });
        assert_eq!(n, 1);
        assert_eq!(rtl8153::split(&[0u8; 8], |_| {}).refused, Some(Refusal::Empty));
        let mut over = std::vec![0u8; 32];
        over[..4].copy_from_slice(&1500u32.to_le_bytes());
        assert_eq!(rtl8153::split(&over, |_| {}).refused, Some(Refusal::PktLen));
    }

    #[test]
    fn framings_are_not_interchangeable() {
        // An AX completion read as RTL: the first dword is frame bytes (dst MAC ff..), not a descriptor.
        let b = ax_build(&[(&eth(0x0800, 60), 0)], 2, true);
        let mut rtl = Vec::new();
        let _ = rtl8153::split(&b, |p| rtl.push(p));
        let mut ax = Vec::new();
        let _ = ax88179::split(&b, 2, |p| ax.push(p));
        assert_ne!(rtl, ax);
    }
}
