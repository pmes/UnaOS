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
//! SNTP-X86 (shared, arch-neutral): the RFC 4330 client-mode SNTP reply parser + request builder.
//!
//! Ported from pi/genet's PI-NET-16 client into a shared, arch-neutral module so the x86 smolnet
//! SNTP client (`crate::smolnet`) and — in a later fold — the pi/genet client render one parser. The
//! parser is the security surface: it reads a 48-byte datagram straight off the wire, so EVERY field
//! is bounds-/sanity-checked before use and NO path can panic. Pure `#![no_std]`, no `alloc`, no I/O.
//!
//! Civil rendering (`YYYY-MM-DDTHH:MM:SSZ`) is NOT duplicated here — it already lives once in
//! `crate::clock::render_iso8601` (moved there by CLOCK-1). This module owns only the wire parse.

/// Seconds between the NTP epoch (1900-01-01) and the Unix epoch (1970-01-01).
pub const NTP_UNIX_DELTA: u64 = 2_208_988_800;
/// Era-1 offset (2^32 - NTP_UNIX_DELTA): added to an NTP seconds value whose high bit is CLEAR, which
/// per RFC 4330 §3 denotes a timestamp in the era beginning 2036-02-07 (the 2036 rollover).
pub const NTP_ERA1_OFFSET: u64 = 2_085_978_496;
/// Sanity band on the resolved Unix time: reject anything before ~2023-11 or after ~2096. A server
/// answering with a wildly-out-of-band timestamp (misconfigured, spoofed, or a stratum-0 KoD that
/// slipped the stratum check) is rejected rather than jamming the clock to a nonsense year.
pub const SANE_MIN_UNIX: u64 = 1_700_000_000; // ~2023-11-14
pub const SANE_MAX_UNIX: u64 = 4_000_000_000; // ~2096-10
/// The SNTP client request first byte: LI=0 (no warning), VN=4, Mode=3 (client). `(0<<6)|(4<<3)|3`.
pub const SNTP_REQ_B0: u8 = 0x23;
/// An SNTP datagram is exactly 48 bytes (no extension / authentication fields here).
pub const SNTP_LEN: usize = 48;
/// The well-known NTP/SNTP UDP port.
pub const NTP_PORT: u16 = 123;

/// Typed outcome of parsing an SNTP reply — every failure mode gets its own arm so the caller can emit
/// a distinct honest witness.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sntp {
    /// A usable time: UTC Unix seconds + the server's stratum.
    Ok { unix_secs: u64, stratum: u8 },
    /// Stratum 0 = Kiss-o'-Death (rate-limit / deny). RFC 4330 §8: back off, do NOT use.
    KissOfDeath,
    /// Structurally invalid, wrong mode/version, LI=alarm(3), zero/insane timestamp — rejected.
    Malformed,
}

/// Convert a 32-bit NTP seconds field to UTC Unix seconds, era-aware (the 2036 rollover): a value with
/// the high bit SET is era 0 (1900-based, 1968..2036); with it clear, era 1 (2036-based).
pub fn ntp_to_unix(ntp_secs: u32) -> u64 {
    let s = ntp_secs as u64;
    if s >= NTP_UNIX_DELTA {
        s - NTP_UNIX_DELTA
    } else {
        s + NTP_ERA1_OFFSET
    }
}

/// Parse an SNTP server reply. HOSTILE-INPUT HARDENED: the length is checked before any field read; the
/// LI/VN/Mode byte is decoded and an alarm LI (3), a version outside 3..=4, or a non-server mode (!=4)
/// is rejected; stratum 0 surfaces as KoD and stratum > 15 is rejected (reserved); the transmit
/// timestamp is read from a bounds-checked window and a zero or out-of-sanity-band time is rejected. No
/// path can panic. `pkt` is the raw UDP payload.
pub fn parse(pkt: &[u8]) -> Sntp {
    if pkt.len() < SNTP_LEN {
        return Sntp::Malformed; // an SNTP datagram is exactly 48 bytes
    }
    let b0 = pkt[0];
    let li = (b0 >> 6) & 0x3;
    let vn = (b0 >> 3) & 0x7;
    let mode = b0 & 0x7;
    if li == 3 {
        return Sntp::Malformed; // LI=3: server clock unsynchronized (alarm) — do not trust its time
    }
    if !(3..=4).contains(&vn) {
        return Sntp::Malformed; // we speak SNTPv4; accept a v3 responder, reject anything else
    }
    if mode != 4 {
        return Sntp::Malformed; // Mode 4 = server. A non-server reply is stale/spoofed
    }
    let stratum = pkt[1];
    if stratum == 0 {
        return Sntp::KissOfDeath; // stratum 0 = KoD packet
    }
    if stratum > 15 {
        return Sntp::Malformed; // 16..=255 reserved / unsynchronized
    }
    // Transmit timestamp: seconds at [40..44], fraction at [44..48] (fraction unused — 1 s resolution
    // is plenty for a civil clock, and avoids float math entirely).
    let secs = u32::from_be_bytes([pkt[40], pkt[41], pkt[42], pkt[43]]);
    if secs == 0 {
        return Sntp::Malformed; // a zero transmit timestamp is not a real time
    }
    let unix = ntp_to_unix(secs);
    if !(SANE_MIN_UNIX..=SANE_MAX_UNIX).contains(&unix) {
        return Sntp::Malformed; // out of the plausible band — refuse to jam the clock to a nonsense year
    }
    Sntp::Ok { unix_secs: unix, stratum }
}

/// Build the 48-byte SNTP client request into `out` (LI=0/VN=4/Mode=3, all other fields zero, per
/// RFC 4330 §5 for a client-only request).
pub fn build_request(out: &mut [u8; SNTP_LEN]) {
    *out = [0u8; SNTP_LEN];
    out[0] = SNTP_REQ_B0;
}

/// Build a well-formed SNTP server reply for `unix_secs` with the given `li`/`vn`/`mode`/`stratum` —
/// the deterministic-gate fixture (feeds canned datagrams through `parse`). Mirrors pi/genet's
/// `build_sntp_reply`. Stack-only, no `alloc`, so it is arch-neutral and usable from any gate.
pub fn build_reply(unix_secs: u64, li: u8, vn: u8, mode: u8, stratum: u8) -> [u8; SNTP_LEN] {
    let mut v = [0u8; SNTP_LEN];
    v[0] = ((li & 0x3) << 6) | ((vn & 0x7) << 3) | (mode & 0x7);
    v[1] = stratum;
    let ntp_secs = (unix_secs + NTP_UNIX_DELTA) as u32;
    v[40..44].copy_from_slice(&ntp_secs.to_be_bytes());
    v
}
