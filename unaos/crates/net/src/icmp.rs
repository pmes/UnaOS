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

//! Minimal ICMPv4: enough to answer echo requests (ping). An ICMP message is
//! `type(1) | code(1) | checksum(2) | rest-of-header(4) | data`. For echo the
//! rest-of-header is identifier(2) + sequence(2); an echo reply mirrors the request
//! verbatim except `type` becomes 0 and the checksum is recomputed.

pub const ICMP_ECHO_REPLY: u8 = 0;
pub const ICMP_ECHO_REQUEST: u8 = 8;

/// The ICMP message `type` byte, if present.
pub fn message_type(msg: &[u8]) -> Option<u8> {
    msg.first().copied()
}

/// Standard 1's-complement checksum over the whole ICMP message.
fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8; // trailing odd byte in the high position
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Write an ICMP echo *request* (type 8) into `out`: type, code, checksum(2),
/// identifier(2), sequence(2), then `data`. Returns the total length, or `None` if
/// `out` is too small. The outbound counterpart of [`write_echo_reply`] — used by the
/// `ping` client to initiate traffic.
pub fn write_echo_request(out: &mut [u8], ident: u16, seq: u16, data: &[u8]) -> Option<usize> {
    let len = 8 + data.len();
    if out.len() < len {
        return None;
    }
    out[0] = ICMP_ECHO_REQUEST; // type 8
    out[1] = 0; // code
    out[2..4].copy_from_slice(&0u16.to_be_bytes()); // checksum (zero before computing)
    out[4..6].copy_from_slice(&ident.to_be_bytes());
    out[6..8].copy_from_slice(&seq.to_be_bytes());
    out[8..len].copy_from_slice(data);
    let csum = checksum(&out[..len]);
    out[2..4].copy_from_slice(&csum.to_be_bytes());
    Some(len)
}

/// Parse an ICMP echo *reply*: returns `(identifier, sequence)` if `msg` is a type-0
/// echo reply, else `None`. Lets a `ping` client match a reply to a request it sent.
pub fn parse_echo_reply(msg: &[u8]) -> Option<(u16, u16)> {
    if msg.len() < 8 || msg[0] != ICMP_ECHO_REPLY {
        return None;
    }
    let ident = u16::from_be_bytes([msg[4], msg[5]]);
    let seq = u16::from_be_bytes([msg[6], msg[7]]);
    Some((ident, seq))
}

/// If `request` is an ICMP echo request, write the corresponding echo reply into
/// `out` and return its length (same size as the request). Returns `None` if the
/// message is not an echo request or `out` is too small.
pub fn write_echo_reply(out: &mut [u8], request: &[u8]) -> Option<usize> {
    // 8 bytes: type, code, checksum(2), identifier(2), sequence(2).
    if request.len() < 8 || request[0] != ICMP_ECHO_REQUEST {
        return None;
    }
    let len = request.len();
    if out.len() < len {
        return None;
    }
    out[..len].copy_from_slice(request);
    out[0] = ICMP_ECHO_REPLY; // type 8 -> 0
    out[1] = 0; // code
    out[2..4].copy_from_slice(&0u16.to_be_bytes()); // zero checksum before computing
    let csum = checksum(&out[..len]);
    out[2..4].copy_from_slice(&csum.to_be_bytes());
    Some(len)
}
