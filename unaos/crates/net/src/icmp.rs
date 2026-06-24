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
