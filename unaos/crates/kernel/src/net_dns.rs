// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.
//
//! SOCK-8 (shared, arch-neutral): the DNS A-record query builder + response parser.
//!
//! Ported from pi/genet's PI-NET-14 outbound client (`net14_build_dns_query` / `net14_skip_name` /
//! `net14_parse_a`) into a shared, arch-neutral module so the x86 smolnet resolver (`crate::smolnet`)
//! and — in a later fold — the pi/genet client render ONE parser. The parser is the security surface: it
//! reads a UDP datagram straight off the wire, so EVERY field is bounds-/sanity-checked before use and NO
//! path can panic or loop unbounded. Pure `#![no_std]`, no `alloc`, no I/O.
//!
//! Hardening (carried verbatim from PI-NET-14, with ONE strictly-stricter addition — the total-name cap
//! in `build_query`, see below):
//!  * the header length is checked before any field read;
//!  * the transaction id must match (a mismatch is a stale/spoofed datagram);
//!  * the QR bit must say "response"; a non-zero RCODE surfaces as `ServerErr`;
//!  * the question + answer sections are walked with `skip_name`, which NEVER dereferences a compression
//!    pointer (a pointer is two bytes and terminates the name, so a compression LOOP is immune by
//!    construction — the walk cannot follow it, cannot hang), carries a label hop cap, and rejects the
//!    reserved top-two-length-bit encodings (0x40 / 0x80);
//!  * every fixed RR field is bounds-checked before read and the answer walk is capped at 64 records.

/// DNS RR/Q TYPE + CLASS codes we handle. Only an A record in the IN class with a 4-byte RDATA is a usable
/// answer; everything else (CNAME, AAAA, ...) is skipped.
pub const DNS_TYPE_A: u16 = 1;
pub const DNS_CLASS_IN: u16 = 1;
/// The well-known DNS UDP port.
pub const DNS_PORT: u16 = 53;
/// RFC 1035 §2.3.4 hard limits: a label is 1..=63 octets; the whole encoded name (length octets + labels +
/// the root) is at most 255 octets. `build_query` enforces both — the 255-octet TOTAL cap is the one
/// strictly-stricter addition over PI-NET-14 (which capped only per-label), and never rejects a legal name.
pub const MAX_LABEL: usize = 63;
pub const MAX_NAME: usize = 255;
/// The fixed 12-byte DNS header.
pub const HDR_LEN: usize = 12;
/// Answer-walk cap: a sane reply carries a handful of records; refuse to walk more than this.
pub const MAX_ANSWERS: u16 = 64;

/// Typed outcome of parsing a DNS response — a distinct arm per failure mode so the caller can emit an
/// honest, localised witness.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dns {
    /// The first A/IN record's four address octets.
    Resolved([u8; 4]),
    /// Structurally invalid: short header, wrong transaction id, QR=query, a malformed name (reserved
    /// length bits / a truncated label / a truncated RR), or otherwise unparseable — rejected.
    Malformed,
    /// A well-formed response with RCODE=0 that carried no usable A record (only CNAME/AAAA/etc., or an
    /// empty answer section).
    NoAnswer,
    /// The server answered with a non-zero RCODE (NXDOMAIN=3, SERVFAIL=2, REFUSED=5, ...).
    ServerErr(u8),
}

/// Build a minimal DNS A-record query for `host` into `out`, returning its length. RD=1 standard recursive
/// query, one question, QTYPE A / QCLASS IN. HOSTILE-OUTPUT SAFE: never writes past `out`; rejects (`None`)
/// an empty/over-long (> 63 B) label, a total encoded name over `MAX_NAME` octets (the strictly-stricter
/// RFC 1035 total-name cap), or a buffer too small.
pub fn build_query(out: &mut [u8], txid: u16, host: &str) -> Option<usize> {
    if out.len() < HDR_LEN {
        return None;
    }
    out[0..2].copy_from_slice(&txid.to_be_bytes());
    out[2..4].copy_from_slice(&0x0100u16.to_be_bytes()); // flags: RD=1 (standard recursive query)
    out[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
    out[6..12].copy_from_slice(&[0, 0, 0, 0, 0, 0]); // AN/NS/AR = 0
    let mut w = HDR_LEN;
    // The encoded name length so far (length octets + label bytes), root octet included at the end. Capped
    // at MAX_NAME per RFC 1035 §2.3.4 — the one guard tighter than PI-NET-14.
    let mut name_len = 0usize;
    for label in host.split('.') {
        let lb = label.as_bytes();
        if lb.is_empty() || lb.len() > MAX_LABEL {
            return None; // an empty or > 63-byte label is not a legal DNS label
        }
        name_len += 1 + lb.len();
        if name_len + 1 > MAX_NAME {
            return None; // encoded name (with the trailing root octet) would exceed 255 octets
        }
        if w + 1 + lb.len() > out.len() {
            return None;
        }
        out[w] = lb.len() as u8;
        w += 1;
        out[w..w + lb.len()].copy_from_slice(lb);
        w += lb.len();
    }
    if w + 5 > out.len() {
        return None;
    }
    out[w] = 0; // root label
    w += 1;
    out[w..w + 2].copy_from_slice(&DNS_TYPE_A.to_be_bytes()); // QTYPE = A
    w += 2;
    out[w..w + 2].copy_from_slice(&DNS_CLASS_IN.to_be_bytes()); // QCLASS = IN
    w += 2;
    Some(w)
}

/// Return the offset just past the DNS name encoded at `off` in `pkt`. HOSTILE-INPUT HARDENED: every byte
/// is bounds-checked; a compression pointer (top two bits set) is NEVER followed — it is two bytes and
/// terminates the name, so returning `off + 2` is both correct for skipping AND immune to compression loops
/// by construction; a label walk carries a hop cap; the reserved length bits (0x40 / 0x80) are rejected.
/// Returns `None` on any structural violation.
pub fn skip_name(pkt: &[u8], mut off: usize) -> Option<usize> {
    let mut labels = 0usize;
    loop {
        let b = *pkt.get(off)?;
        match b & 0xc0 {
            0x00 => {
                if b == 0 {
                    return Some(off + 1); // root label — end of name
                }
                off = off.checked_add(1 + b as usize)?;
                if off > pkt.len() {
                    return None;
                }
                labels += 1;
                if labels > 127 {
                    return None; // hop cap — a sane name has far fewer than 128 labels
                }
            }
            0xc0 => {
                pkt.get(off + 1)?; // bounds-check the 2nd pointer byte; we do NOT dereference it
                return Some(off + 2);
            }
            _ => return None, // 0x40 / 0x80 reserved in the top two bits — malformed
        }
    }
}

/// Parse a DNS response and extract the first A record, or a typed failure. HOSTILE-INPUT HARDENED: header
/// length checked, transaction id must match (a mismatch is a stale/spoofed datagram), the QR bit must say
/// "response", a non-zero RCODE surfaces as `ServerErr`, the question + answer sections are walked with
/// `skip_name` (compression-loop-immune) and every fixed field is bounds-checked before read, and the
/// answer walk is capped. No path can panic or loop unbounded. `pkt` is the raw UDP payload; `txid` the
/// value passed to the matching `build_query`.
pub fn parse_a(pkt: &[u8], txid: u16) -> Dns {
    if pkt.len() < HDR_LEN {
        return Dns::Malformed;
    }
    if pkt[0..2] != txid.to_be_bytes() {
        return Dns::Malformed; // not our transaction
    }
    let flags = u16::from_be_bytes([pkt[2], pkt[3]]);
    if flags & 0x8000 == 0 {
        return Dns::Malformed; // QR=0: this is a query, not a response
    }
    let rcode = (flags & 0x000f) as u8;
    if rcode != 0 {
        return Dns::ServerErr(rcode); // NXDOMAIN (3), SERVFAIL (2), REFUSED (5), ...
    }
    let qd = u16::from_be_bytes([pkt[4], pkt[5]]);
    let an = u16::from_be_bytes([pkt[6], pkt[7]]);
    let mut off = HDR_LEN;
    // Skip the question section: each question is a name + QTYPE(2) + QCLASS(2).
    for _ in 0..qd {
        off = match skip_name(pkt, off) {
            Some(o) => o,
            None => return Dns::Malformed,
        };
        off = match off.checked_add(4) {
            Some(o) if o <= pkt.len() => o,
            _ => return Dns::Malformed,
        };
    }
    // Walk the answers (capped) for the first A/IN record with a 4-byte RDATA.
    let mut i = 0u16;
    while i < an && i < MAX_ANSWERS {
        off = match skip_name(pkt, off) {
            Some(o) => o,
            None => return Dns::Malformed,
        };
        // Fixed RR header: TYPE(2) CLASS(2) TTL(4) RDLENGTH(2) = 10 bytes.
        if off + 10 > pkt.len() {
            return Dns::Malformed;
        }
        let rtype = u16::from_be_bytes([pkt[off], pkt[off + 1]]);
        let rclass = u16::from_be_bytes([pkt[off + 2], pkt[off + 3]]);
        let rdlen = u16::from_be_bytes([pkt[off + 8], pkt[off + 9]]) as usize;
        let rdata = off + 10;
        if rdata + rdlen > pkt.len() {
            return Dns::Malformed;
        }
        if rtype == DNS_TYPE_A && rclass == DNS_CLASS_IN && rdlen == 4 {
            return Dns::Resolved([pkt[rdata], pkt[rdata + 1], pkt[rdata + 2], pkt[rdata + 3]]);
        }
        off = rdata + rdlen; // skip CNAME / AAAA / etc. and keep looking
        i += 1;
    }
    Dns::NoAnswer
}

/// Build a well-formed DNS A-record REPLY for a query whose QNAME is `host`, into `out`, returning its
/// length — the deterministic-gate fixture (mirrors pi/genet's `build_a_response`, but stack-only / no
/// `alloc` so it is arch-neutral and usable from any gate). QR=1 RD=1 RA=1 + `rcode`; `ancount` answer
/// records, each a compression pointer to the question name (offset 12, exactly as a real resolver emits)
/// + A/IN, TTL 300, RDATA = `ip`. Returns `None` if `out` is too small or `host` has an illegal label.
pub fn build_a_reply(
    out: &mut [u8],
    txid: u16,
    host: &str,
    ip: [u8; 4],
    rcode: u8,
    ancount: u16,
) -> Option<usize> {
    if out.len() < HDR_LEN {
        return None;
    }
    out[0..2].copy_from_slice(&txid.to_be_bytes());
    out[2..4].copy_from_slice(&(0x8180u16 | rcode as u16).to_be_bytes()); // QR=1 RD=1 RA=1 + rcode
    out[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
    out[6..8].copy_from_slice(&ancount.to_be_bytes()); // ANCOUNT
    out[8..12].copy_from_slice(&[0, 0, 0, 0]); // NS/AR = 0
    let mut w = HDR_LEN;
    // Question: <host> A IN (name starts at offset 12 — the answer's compression pointer targets it).
    for label in host.split('.') {
        let lb = label.as_bytes();
        if lb.is_empty() || lb.len() > MAX_LABEL || w + 1 + lb.len() > out.len() {
            return None;
        }
        out[w] = lb.len() as u8;
        w += 1;
        out[w..w + lb.len()].copy_from_slice(lb);
        w += lb.len();
    }
    if w + 5 > out.len() {
        return None;
    }
    out[w] = 0; // root
    w += 1;
    out[w..w + 2].copy_from_slice(&DNS_TYPE_A.to_be_bytes()); // QTYPE A
    out[w + 2..w + 4].copy_from_slice(&DNS_CLASS_IN.to_be_bytes()); // QCLASS IN
    w += 4;
    for _ in 0..ancount {
        if w + 16 > out.len() {
            return None;
        }
        out[w..w + 2].copy_from_slice(&[0xc0, 0x0c]); // name = pointer to offset 12
        out[w + 2..w + 4].copy_from_slice(&DNS_TYPE_A.to_be_bytes()); // TYPE A
        out[w + 4..w + 6].copy_from_slice(&DNS_CLASS_IN.to_be_bytes()); // CLASS IN
        out[w + 6..w + 10].copy_from_slice(&300u32.to_be_bytes()); // TTL
        out[w + 10..w + 12].copy_from_slice(&4u16.to_be_bytes()); // RDLENGTH
        out[w + 12..w + 16].copy_from_slice(&ip);
        w += 16;
    }
    Some(w)
}
