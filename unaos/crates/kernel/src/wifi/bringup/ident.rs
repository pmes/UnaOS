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
    *ID.lock() = Some(id);
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
