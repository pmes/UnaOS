// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WIFI6 (rmbp-ledger B439) — the BCM4331 SPROM shadow's identity fields, decoded from words the
//! kernel read (rung S2r, `bcm4331.md` §S2r / §7). Pure: a `&[u16]` in, an [`Identity`] out.
//!
//! CHARTER: Kernel — shared-core. One decoder for the layout; the kernel's `src/wifi/` reads the 220
//! words through the ChipCommon window wifi2 already opens, and the host tests below pin the decode.
//!
//! SOURCES: every offset is `[ONE-SOURCE: bcm4331.md §S2r]` — the tree's own transcription of the
//! rev-8 layout (table "What S2 reads"). It is a READ-path layout: nothing here addresses a write.
//! Its corroboration is on metal and is part of the verdict: `board_type` must equal the PCI
//! subsystem device id (f25 S0: `subsys=0x106b:0x00ef … MATCH`), the revision must sit in 8..=11, and
//! the MAC must be a non-degenerate unicast address. The CRC-8 is NOT computed (§S2: no in-tree
//! transcription of its table; a wrong one would call a good SPROM bad).

/// Shadow length in 16-bit words. [ONE-SOURCE: §S2r]
pub const WORDS: usize = 220;
/// Byte offsets inside the shadow. [ONE-SOURCE: §S2r]
pub const SPID: usize = 0x04;
pub const BOARDREV: usize = 0x82;
pub const BFL_LO: usize = 0x84;
pub const BFL_HI: usize = 0x86;
pub const IL0MAC: usize = 0x8C;
pub const ANTAVAIL: usize = 0x9C;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every corroboration term holds.
    Plausible,
    /// Readable, but at least one term fails; the line names which.
    Suspect,
    /// Uniformly 0xFFFF or 0x0000 (§S2: the PA-line mux, or no SPROM).
    Blocked,
    /// Fewer than [`WORDS`] words were read.
    Short,
}

impl Verdict {
    pub fn token(self) -> &'static str {
        match self {
            Verdict::Plausible => "PLAUSIBLE",
            Verdict::Suspect => "SUSPECT",
            Verdict::Blocked => "BLOCKED",
            Verdict::Short => "SHORT",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub rev: u8,
    pub board_type: u16,
    pub board_rev: u16,
    pub boardflags: u32,
    pub mac: [u8; 6],
    pub ant_2g: u8,
    pub ant_5g: u8,
    pub rev_ok: bool,
    pub board_matches_ssid: bool,
    pub mac_ok: bool,
    pub verdict: Verdict,
}

fn w(words: &[u16], byte_off: usize) -> u16 {
    words.get(byte_off / 2).copied().unwrap_or(0xFFFF)
}

/// Decode the shadow. `ssid_device` is the PCI subsystem device id read from config space.
pub fn decode(words: &[u16], ssid_device: u16) -> Identity {
    let rev = (w(words, (WORDS - 1) * 2) & 0xFF) as u8;
    let m = [w(words, IL0MAC), w(words, IL0MAC + 2), w(words, IL0MAC + 4)];
    // Each MAC word is stored big-endian (§S2r: "3 BE words").
    let mac = [
        (m[0] >> 8) as u8, m[0] as u8, (m[1] >> 8) as u8, m[1] as u8, (m[2] >> 8) as u8, m[2] as u8,
    ];
    let ant = w(words, ANTAVAIL);
    let board_type = w(words, SPID);
    let full = words.len() >= WORDS;
    let all_ff = words.iter().take(WORDS).all(|v| *v == 0xFFFF);
    let all_00 = words.iter().take(WORDS).all(|v| *v == 0x0000);
    let rev_ok = (8..=11).contains(&rev);
    let board_matches_ssid = board_type == ssid_device;
    let mac_ok = mac != [0; 6] && mac != [0xFF; 6] && (mac[0] & 1) == 0;
    let verdict = if !full {
        Verdict::Short
    } else if all_ff || all_00 {
        Verdict::Blocked
    } else if rev_ok && board_matches_ssid && mac_ok {
        Verdict::Plausible
    } else {
        Verdict::Suspect
    };
    Identity {
        rev,
        board_type,
        board_rev: w(words, BOARDREV),
        boardflags: ((w(words, BFL_HI) as u32) << 16) | w(words, BFL_LO) as u32,
        mac,
        ant_2g: ant as u8,
        ant_5g: (ant >> 8) as u8,
        rev_ok,
        board_matches_ssid,
        mac_ok,
        verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn shadow() -> alloc::vec::Vec<u16> {
        let mut s = vec![0x1234u16; WORDS];
        s[SPID / 2] = 0x00ef;
        s[IL0MAC / 2] = 0x0026;
        s[IL0MAC / 2 + 1] = 0x4ab1;
        s[IL0MAC / 2 + 2] = 0x2233;
        s[ANTAVAIL / 2] = 0x0703;
        s[WORDS - 1] = 0x5a08;
        s
    }

    #[test]
    fn plausible_rev8_board() {
        let id = decode(&shadow(), 0x00ef);
        assert_eq!(id.verdict, Verdict::Plausible);
        assert_eq!(id.mac, [0x00, 0x26, 0x4a, 0xb1, 0x22, 0x33]);
        assert_eq!((id.rev, id.ant_2g, id.ant_5g), (8, 3, 7));
    }

    #[test]
    fn ssid_disagreement_is_suspect() {
        assert_eq!(decode(&shadow(), 0x00f0).verdict, Verdict::Suspect);
    }

    #[test]
    fn multicast_mac_is_suspect() {
        let mut s = shadow();
        s[IL0MAC / 2] = 0x0126;
        assert!(!decode(&s, 0x00ef).mac_ok);
    }

    #[test]
    fn uniform_shadow_is_blocked_short_is_short() {
        assert_eq!(decode(&vec![0xFFFF; WORDS], 0x00ef).verdict, Verdict::Blocked);
        assert_eq!(decode(&vec![0; WORDS], 0x00ef).verdict, Verdict::Blocked);
        assert_eq!(decode(&shadow()[..100], 0x00ef).verdict, Verdict::Short);
    }
}
