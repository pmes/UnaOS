// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WIFI6 (rmbp-ledger B439) — rung S2r, the board's SPROM identity, read on the FLOWN wifi2 path.
//!
//! CHARTER: Kernel — driver. A child of `bringup`: it reads through the window wifi2 ALREADY points
//! at ChipCommon (f25 08:23:22Z `WROTE cfg:0x80 … new=0x18000000 … took=1`, then `cc-raw chipid=…
//! MATCH`) — no selector of its own, no register written, no new knob. The shadow is cc+0x800,
//! 220 words, inside that 4 KiB window ([ONE-SOURCE: bcm4331.md §S2r]); the decode is the shared core
//! `wifi_core::sprom` (one implementation, host-tested). The identity line is printed ONCE, at the
//! end of `bringup_once`, so it carries the PHY_VER read (core-pre) and the wifi4 gate's V4 radio
//! read beside the SPROM's own fields: agreement of `board=` with the PCI ssid device is the
//! offset's confirmation; the radio id is NOT a SPROM field in the rev-8 layout as cited, so
//! `radio=` is the gate's read printed for the comparison the seat makes, never decoded from SPROM.

use super::r16;
use alloc::string::String;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use spin::Mutex;
use wifi_core::sprom::{self, Identity, Verdict};

/// cc+0x800. [ONE-SOURCE: bcm4331.md §S2r] — the same value `drivers/bcma.rs` `CC_SPROM` carries.
const CC_SPROM: u64 = 0x0800;

static ID: Mutex<Option<Identity>> = Mutex::new(None);
/// The core-pre PHY_VER word (0 = not read).
static PHY: AtomicU16 = AtomicU16::new(0);
/// The wifi4 gate's V4-order radio-id word (0 = wifi4 did not read it this boot).
static RADIO: AtomicU32 = AtomicU32::new(0);

/// Read + dump the shadow while cfg:0x80 is on ChipCommon. `ssid_dev` is the PCI subsystem device.
pub(super) fn sprom_read(bar0: u64, ssid_dev: u16) {
    let mut words = alloc::vec::Vec::with_capacity(sprom::WORDS);
    for i in 0..sprom::WORDS {
        words.push(unsafe { r16(bar0, CC_SPROM + (i as u64) * 2) });
    }
    for (r, row) in words.chunks(16).enumerate() {
        let mut s = String::new();
        for v in row {
            let _ = write!(s, " {:04x}", v);
        }
        serial_println!("[wifi6] sprom-dump +{:#05x}{}", r * 32, s);
    }
    let id = sprom::decode(&words, ssid_dev);
    super::super::status::set_sprom(match id.verdict {
        Verdict::Plausible => 1,
        Verdict::Suspect => 2,
        Verdict::Blocked => 3,
        Verdict::Short => 4,
    });
    *ID.lock() = Some(id); #[cfg(feature = "wifi10")] { *PWR.lock() = Some(pwr_decode(&words)); } // WIFI11 (B653): the per-core power blocks, same read
}

pub(super) fn note_phy(raw: u16) {
    PHY.store(raw, Ordering::Relaxed);
}

#[cfg(feature = "wifi4")]
pub(super) fn note_radio(id_v4: u32) {
    RADIO.store(id_v4, Ordering::Relaxed);
}

/// The ONE identity line (end of `bringup_once`).
pub(super) fn identity_line() {
    let Some(id) = *ID.lock() else {
        serial_println!("[wifi6] sprom UNREAD reason=chipcommon-not-reached — wifi2 refused before cfg:0x80 sat on ChipCommon");
        return;
    };
    let phy = PHY.load(Ordering::Relaxed);
    let radio = RADIO.load(Ordering::Relaxed);
    let m = id.mac;
    let radio_s = if radio == 0 {
        String::from("unread(wifi4-not-run-this-boot)")
    } else {
        // [SPEC-V3 RadioID] + [SPEC-V4 802.11/Radio/RadioID]: rev=31:28 ver=27:12 mfg=11:0 (wifi4's own decode).
        alloc::format!("{:#06x}/{} mfg={:#05x}", (radio >> 12) & 0xFFFF, radio >> 28, radio & 0xFFF)
    };
    serial_println!(
        "[wifi6] sprom rev={} board={:#06x} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} phy={}/{} radio={} -> {} (rev-ok={} board-vs-ssid={} mac-ok={} board-rev={:#06x} boardflags={:#010x} ant-2g={:#04x} ant-5g={:#04x}; radio is the wifi4 V4 gate read, NOT a SPROM field; CRC not computed)",
        id.rev, id.board_type, m[0], m[1], m[2], m[3], m[4], m[5],
        (phy >> 8) & 0xF, phy & 0xF, radio_s, id.verdict.token(),
        id.rev_ok as u8, if id.board_matches_ssid { "MATCH" } else { "MISMATCH" }, id.mac_ok as u8,
        id.board_rev, id.boardflags, id.ant_2g, id.ant_5g
    );
    if id.verdict == Verdict::Blocked {
        serial_println!(
            "[wifi6] sprom BLOCKED — the shadow reads uniform; bcm4331.md §S2 names the write that would unblock it (the ChipCommon PA-line mux control) and OTP's read command (OTPP) — both past the read-only ceiling, NOT made"
        );
    }
}

/// WIFI10 (B646): the station MAC S7 transmits from — the SPROM `il0macaddr` this boot read, only when
/// S2r judged it plausible (unicast, not all-zero, not all-ones: `wifi_core::sprom::decode`).
#[cfg(feature = "wifi10")]
pub(super) fn mac() -> Option<[u8; 6]> {
    let id = (*ID.lock())?;
    if id.mac_ok { Some(id.mac) } else { None }
}

// ── WIFI11 (B653): the SPROM's per-core 2.4 GHz power info, for HT-PHY TX power control ───────────
/// One core's 2.4 GHz power block as `drivers/bcma/sprom.c` reads it for SPROM revisions 8..11
/// (`bcma_sprom_valid` accepts 8..=11, `sprom.c:167-173`; every one is extracted by
/// `bcma_sprom_extract_r8`, `sprom.c:643`): byte +0x00 = maxpwr_2g (bits 7:0) and itssi_2g (bits
/// 15:8) (`ssb_regs.h:542-545`, `sprom.c:287-290`), +0x02/+0x04/+0x06 = pa_2g[0..2] (`ssb_regs.h:546-548`,
/// `sprom.c:292-294`). A fourth coefficient exists in Linux's struct and is never read for this layout.
#[cfg(feature = "wifi10")]
#[derive(Clone, Copy)]
pub(super) struct CorePwr {
    pub itssi_2g: u8,
    pub maxpwr_2g: u8,
    pub pa_2g: [u16; 3],
}

/// The three HT-PHY cores' power-info block base offsets (bytes): `ssb_regs.h:537-539`
/// (`SSB_SROM8_PWR_INFO_CORE0..2`); the fourth block (0x0120) belongs to no HT-PHY core
/// (`phy_ht.c:632-637` reads cores 0..2). The last byte read, 0x100 + 0x07, is inside the 440-byte shadow.
#[cfg(feature = "wifi10")]
pub(super) const PWR_INFO_CORE: [usize; 3] = [0x00C0, 0x00E0, 0x0100];

#[cfg(feature = "wifi10")]
static PWR: Mutex<Option<[CorePwr; 3]>> = Mutex::new(None);

/// Decode the three blocks from the shadow words (word index = byte offset / 2, `sprom.c:183`).
#[cfg(feature = "wifi10")]
fn pwr_decode(words: &[u16]) -> [CorePwr; 3] {
    let w = |b: usize| words.get(b / 2).copied().unwrap_or(0xFFFF);
    let mut out = [CorePwr { itssi_2g: 0, maxpwr_2g: 0, pa_2g: [0; 3] }; 3];
    for (c, base) in PWR_INFO_CORE.iter().enumerate() {
        let mi = w(*base);
        out[c] = CorePwr {
            maxpwr_2g: (mi & 0x00FF) as u8,
            itssi_2g: (mi >> 8) as u8,
            pa_2g: [w(*base + 0x02), w(*base + 0x04), w(*base + 0x06)],
        };
    }
    out
}

/// A block an unprogrammed SPROM would carry: the three PA words uniform 0xFFFF or 0x0000, or a max
/// power of 0 or 0xFF. b43 would drive power control from such a block; this tree refuses (the wire
/// says `sprom=blank`) and the frame leaves at WIFI10's fixed index instead.
#[cfg(feature = "wifi10")]
pub(super) fn pwr_blank(p: &CorePwr) -> bool {
    let pa = p.pa_2g;
    pa == [0xFFFF; 3] || pa == [0; 3] || p.maxpwr_2g == 0 || p.maxpwr_2g == 0xFF
}

/// The power info this boot read, with the SPROM revision, only when S2r judged the shadow PLAUSIBLE
/// (`status::sprom_token() == "ok"`, the gate `mac()` shares). `Err` names the verdict otherwise.
#[cfg(feature = "wifi10")]
pub(super) fn pwr_2g() -> Result<(u8, [CorePwr; 3]), &'static str> {
    let Some(id) = *ID.lock() else { return Err("unread") };
    if id.verdict != Verdict::Plausible {
        return Err(id.verdict.token());
    }
    match *PWR.lock() {
        Some(p) => Ok((id.rev, p)),
        None => Err("unread"),
    }
}

/// Print the decoded blocks by name, one line: `[wifi11] sprom rev=… core0@0xc0 maxpwr_2g=… …`.
#[cfg(feature = "wifi10")]
pub(super) fn pwr_line(rev: u8, p: &[CorePwr; 3]) {
    let mut s = String::new();
    for (c, cp) in p.iter().enumerate() {
        let _ = write!(
            s, " core{}@{:#05x} maxpwr_2g={}({}dBm/4) itssi_2g={} pa_2g=[{:#06x},{:#06x},{:#06x}]{}",
            c, PWR_INFO_CORE[c], cp.maxpwr_2g, cp.maxpwr_2g, cp.itssi_2g, cp.pa_2g[0], cp.pa_2g[1], cp.pa_2g[2],
            if pwr_blank(cp) { "(blank)" } else { "" }
        );
    }
    serial_println!(
        "[wifi11] sprom rev={} layout=rev8..11(bcma extract_r8 for every accepted revision){} — sprom.c:167-173, :221-232, :285-294; ssb_regs.h:537-548",
        rev, s
    );
}
