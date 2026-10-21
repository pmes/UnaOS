// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WIFI6 (rmbp-ledger B439) — rung S2r, the board's SPROM identity, read on the FLOWN wifi2 path.
//!
//! CHARTER: Kernel — driver. A child of `bringup`: it reads through the window wifi2 ALREADY points
//! at ChipCommon (f25 08:23:22Z `WROTE cfg:0x80 … new=0x18000000 … took=1`, then `cc-raw chipid=…
//! MATCH`) — no selector of its own, no new knob. The shadow is cc+0x800 (or the OTP general-use
//! region above it), 220 words, inside that 4 KiB window ([ONE-SOURCE: bcm4331.md §S2r]); the decode is the shared core
//! `wifi_core::sprom` (one implementation, host-tested). The identity line is printed ONCE, at the
//! end of `bringup_once`, so it carries the PHY_VER read (core-pre) and the wifi4 gate's V4 radio
//! read beside the SPROM's own fields: agreement of `board=` with the PCI ssid device is the
//! offset's confirmation; the radio id is NOT a SPROM field in the rev-8 layout as cited, so
//! `radio=` is the gate's read printed for the comparison the seat makes, never decoded from SPROM.
//!
//! WIFIMAC (rmbp-ledger B669): flight 36 read the shadow 220 words of 0x0000 on all three boots
//! (`f36-boot2.log` 00:13:49 `[wifi6] sprom-dump`), so no offset or byte order could have yielded a
//! MAC: the read's PRECONDITION was missing. Linux's `bcma_sprom_get` (drivers/bcma/sprom.c) picks the
//! source first (`bcma_sprom_ext_available`: CAP.SPROM + SROM_CONTROL.PRESENT on a ChipCommon rev >= 31;
//! else `bcma_sprom_onchip_available`/`_offset`: CHIPSTAT OTP-present, CAP.OTPS, OTPS.GU_PROG_HW, OTPL's
//! general-use offset) and then, for the 4331, calls `bcma_chipco_bcm4331_ext_pa_lines_ctl(cc, false)`
//! BEFORE the read — CHIPCTL's EXTPA_EN / EXTPA_EN2 / EXTPA_ON_GPIO2_5, the last of which puts the
//! A-band PA line on `sprom_dout` on package 9 (this part: `cc-decode … pkg[23:20]=9`). This module now
//! does the same: the source registers read and printed (`[wifi6] sprom-src`), the three PA bits cleared
//! in CHIPCTL, the 220 words read, and CHIPCTL's PRE-IMAGE written back (reversible, BP-net-8; Linux
//! re-derives the bits instead — the pre-image is the stricter undo). Those two ChipCommon writes are
//! the only writes this file makes; they are counted on wifi2's `end` line (`wrote-cc-chipctl=`).

use super::{r16, r32, w32};
use alloc::string::String;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use crate::sync::Mutex;
use wifi_core::sprom::{self, Identity, Verdict};

/// cc+0x800. [ONE-SOURCE: bcm4331.md §S2r] — the same value `drivers/bcma.rs` `CC_SPROM` carries.
const CC_SPROM: u64 = 0x0800;
/// The ChipCommon window is one 4 KiB page: an OTP region whose 220 words would cross it is not read.
const CC_WINDOW: u64 = 0x1000;

// WIFIMAC (B669): the ChipCommon words Linux's source choice reads, `bcma_driver_chipcommon.h` names
// ([ONE-SOURCE: bcm4331.md §S2r "WIFIMAC"]; the same offsets `drivers/bcma.rs` S0/L0 read).
const CC_CAP: u64 = 0x0004; // BCMA_CC_CAP
const CC_OTPS: u64 = 0x0010; // BCMA_CC_OTPS
const CC_OTPL: u64 = 0x001C; // BCMA_CC_OTPL
const CC_CHIPCTL: u64 = 0x0028; // BCMA_CC_CHIPCTL — the one register written (cleared, then its pre-image)
const CC_CHIPSTAT: u64 = 0x002C; // BCMA_CC_CHIPSTAT
const CC_SROM_CONTROL: u64 = 0x0190; // BCMA_CC_SROM_CONTROL
const CAP_SPROM: u32 = 0x4000_0000; // BCMA_CC_CAP_SPROM
const CAP_OTPS: u32 = 0x0038_0000; // BCMA_CC_CAP_OTPS (size field; 0 = no OTP)
const SROM_CONTROL_PRESENT: u32 = 0x0000_0001; // BCMA_CC_SROM_CONTROL_PRESENT
const CHIPST_4331_SPROM_PRESENT: u32 = 0x0000_0002; // BCMA_CC_CHIPST_4331_SPROM_PRESENT (printed; rev < 31 authority)
const CHIPST_4331_OTP_PRESENT: u32 = 0x0000_0004; // BCMA_CC_CHIPST_4331_OTP_PRESENT
const OTPS_GU_PROG_HW: u32 = 0x0000_0100; // BCMA_CC_OTPS_GU_PROG_HW — the HW (SPROM-image) region is programmed
const OTPL_GURGN_OFFSET: u32 = 0x0000_0FFF; // BCMA_CC_OTPL_GURGN_OFFSET — a BIT offset (Linux: `>> 3` to bytes)
/// `bcma_chipco_bcm4331_ext_pa_lines_ctl(cc, false)` clears exactly these three CHIPCTL bits.
const CHIPCTL_4331_EXTPA_EN: u32 = 1 << 4;
const CHIPCTL_4331_EXTPA_ON_GPIO2_5: u32 = 1 << 7; // "aband extpa will be at gpio2/5 and sprom_dout"
const CHIPCTL_4331_EXTPA_EN2: u32 = 1 << 12;
const CHIPCTL_4331_PA_LINES: u32 = CHIPCTL_4331_EXTPA_EN | CHIPCTL_4331_EXTPA_ON_GPIO2_5 | CHIPCTL_4331_EXTPA_EN2;

/// The OUIs a station MAC must carry to be `plausible=1` (and so to be transmitted from by WIFI10).
/// NON-EXHAUSTIVE on purpose: Apple holds hundreds of OUIs; these are Broadcom's and the Apple
/// prefixes of the 2008-2013 Mac era (the first 26 are `drivers/bcma.rs` `oui_known`'s table, the rest
/// added for the 2011-2012 AirPort parts), transcribed from the IEEE registry as remembered — NOT
/// checked against a registry file in this tree. A genuine MAC whose prefix is missing reads
/// `plausible=0 oui=unlisted` with its raw bytes on the line and WIFI10 declines (the safe direction:
/// nothing transmits from an address this table cannot place); add the prefix from the capture.
const ACCEPTED_OUIS: &[([u8; 3], &str)] = &[
    ([0x00, 0x10, 0x18], "broadcom"), ([0x00, 0x1B, 0xE9], "broadcom"), ([0x18, 0xC0, 0x86], "broadcom"),
    ([0x00, 0x1B, 0x63], "apple"), ([0x00, 0x1E, 0xC2], "apple"), ([0x00, 0x1F, 0xF3], "apple"),
    ([0x00, 0x21, 0xE9], "apple"), ([0x00, 0x22, 0x41], "apple"), ([0x00, 0x23, 0x12], "apple"),
    ([0x00, 0x23, 0x6C], "apple"), ([0x00, 0x23, 0xDF], "apple"), ([0x00, 0x24, 0x36], "apple"),
    ([0x00, 0x25, 0x00], "apple"), ([0x00, 0x25, 0x4B], "apple"), ([0x00, 0x25, 0xBC], "apple"),
    ([0x00, 0x26, 0x08], "apple"), ([0x00, 0x26, 0x4A], "apple"), ([0x00, 0x26, 0xB0], "apple"),
    ([0x00, 0x26, 0xBB], "apple"), ([0x00, 0x3E, 0xE1], "apple"), ([0x04, 0x0C, 0xCE], "apple"),
    ([0x04, 0x1E, 0x64], "apple"), ([0x04, 0x54, 0x53], "apple"), ([0x04, 0xF1, 0x3E], "apple"),
    ([0x08, 0x00, 0x07], "apple"), ([0x0C, 0x3E, 0x9F], "apple"),
    ([0x00, 0x03, 0x93], "apple"), ([0x00, 0x0A, 0x95], "apple"), ([0x00, 0x0D, 0x93], "apple"),
    ([0x00, 0x11, 0x24], "apple"), ([0x00, 0x14, 0x51], "apple"), ([0x00, 0x16, 0xCB], "apple"),
    ([0x00, 0x17, 0xF2], "apple"), ([0x00, 0x19, 0xE3], "apple"), ([0x00, 0x1C, 0xB3], "apple"),
    ([0x00, 0x1D, 0x4F], "apple"), ([0x00, 0x1E, 0x52], "apple"), ([0x00, 0x1F, 0x5B], "apple"),
    ([0x00, 0x23, 0x32], "apple"), ([0x00, 0x30, 0x65], "apple"), ([0x00, 0x50, 0xE4], "apple"),
    ([0x10, 0x40, 0xF3], "apple"), ([0x10, 0x93, 0xE9], "apple"), ([0x10, 0x9A, 0xDD], "apple"),
    ([0x14, 0x10, 0x9F], "apple"), ([0x1C, 0xAB, 0xA7], "apple"), ([0x20, 0xC9, 0xD0], "apple"),
    ([0x28, 0x37, 0x37], "apple"), ([0x28, 0xCF, 0xDA], "apple"), ([0x28, 0xCF, 0xE9], "apple"),
    ([0x34, 0x15, 0x9E], "apple"), ([0x3C, 0x07, 0x54], "apple"), ([0x40, 0x6C, 0x8F], "apple"),
    ([0x44, 0x2A, 0x60], "apple"), ([0x58, 0x55, 0xCA], "apple"), ([0x60, 0x33, 0x4B], "apple"),
    ([0x60, 0xC5, 0x47], "apple"), ([0x60, 0xFB, 0x42], "apple"), ([0x64, 0xB9, 0xE8], "apple"),
    ([0x68, 0xA8, 0x6D], "apple"), ([0x70, 0x56, 0x81], "apple"), ([0x78, 0x31, 0xC1], "apple"),
    ([0x7C, 0x6D, 0x62], "apple"), ([0x7C, 0xC3, 0xA1], "apple"), ([0x7C, 0xD1, 0xC3], "apple"),
    ([0x80, 0x49, 0x71], "apple"), ([0x84, 0x38, 0x35], "apple"), ([0x88, 0x53, 0x95], "apple"),
    ([0x8C, 0x2D, 0xAA], "apple"), ([0x90, 0x27, 0xE4], "apple"), ([0x98, 0xFE, 0x94], "apple"),
    ([0xA4, 0x67, 0x06], "apple"), ([0xA8, 0x86, 0xDD], "apple"), ([0xB8, 0x8D, 0x12], "apple"),
    ([0xB8, 0xE8, 0x56], "apple"), ([0xB8, 0xF6, 0xB1], "apple"), ([0xC8, 0x2A, 0x14], "apple"),
    ([0xC8, 0xBC, 0xC8], "apple"), ([0xD4, 0x9A, 0x20], "apple"), ([0xD8, 0x30, 0x62], "apple"),
    ([0xE0, 0xF8, 0x47], "apple"), ([0xE4, 0xCE, 0x8F], "apple"), ([0xF0, 0xB4, 0x79], "apple"),
    ([0xF8, 0x1E, 0xDF], "apple"),
];

fn oui_vendor(m: &[u8; 6]) -> Option<&'static str> {
    ACCEPTED_OUIS.iter().find(|(o, _)| o[..] == m[..3]).map(|(_, v)| *v)
}

/// The station-MAC plausibility the arc names: not all-zero, not all-ff, unicast (bit 0 of byte 0
/// clear) — `wifi_core::sprom::decode`'s `mac_ok` — AND an OUI in [`ACCEPTED_OUIS`].
fn mac_plausible(id: &Identity) -> bool {
    id.mac_ok && oui_vendor(&id.mac).is_some()
}

/// Where the 220 words came from: the external SPROM (cc+0x800), the OTP's HW region (cc+0x800 +
/// OTPL's offset), or neither advertised (cc+0x800 read anyway, so the dump still says what is there).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Src {
    Sprom,
    Otp,
    Unadvertised,
}

impl Src {
    fn token(self) -> &'static str {
        match self {
            Src::Sprom => "sprom",
            Src::Otp => "otp",
            Src::Unadvertised => "none(neither SROM_CONTROL.PRESENT nor OTP GU_PROG_HW; cc+0x800 read anyway)",
        }
    }
}

static SRC: Mutex<Option<Src>> = Mutex::new(None);

static ID: Mutex<Option<Identity>> = Mutex::new(None);
/// The core-pre PHY_VER word (0 = not read).
static PHY: AtomicU16 = AtomicU16::new(0);
/// The wifi4 gate's V4-order radio-id word (0 = wifi4 did not read it this boot).
static RADIO: AtomicU32 = AtomicU32::new(0);

/// Read + dump the shadow while cfg:0x80 is on ChipCommon. `ssid_dev` is the PCI subsystem device.
/// Returns the number of ChipCommon CHIPCTL writes made (0 when the PA bits were already clear, else 2:
/// the clear and the pre-image restore) for wifi2's `end` census.
pub(super) fn sprom_read(bar0: u64, ssid_dev: u16) -> u32 {
    // 1. The source, as `bcma_sprom_get` chooses it. The chip id matched 0x13924331 before this call,
    //    and this part's ChipCommon is rev 37 (f36 `erom-core[0] … rev=37`), so the rev >= 31 rule is
    //    the one that applies: CAP.SPROM && SROM_CONTROL.PRESENT. CHIPSTAT's SPROM bit is printed beside it.
    let (cap, srom_ctl, chipst, otps, otpl) = unsafe {
        (r32(bar0, CC_CAP), r32(bar0, CC_SROM_CONTROL), r32(bar0, CC_CHIPSTAT), r32(bar0, CC_OTPS), r32(bar0, CC_OTPL))
    };
    let ext = cap & CAP_SPROM != 0 && srom_ctl & SROM_CONTROL_PRESENT != 0;
    let onchip = chipst & CHIPST_4331_OTP_PRESENT != 0 && cap & CAP_OTPS != 0;
    let otp_hw = otps & OTPS_GU_PROG_HW != 0;
    let otp_off = ((otpl & OTPL_GURGN_OFFSET) >> 3) as u64;
    let otp_fits = CC_SPROM + otp_off + (sprom::WORDS as u64) * 2 <= CC_WINDOW;
    let (src, base) = if ext {
        (Src::Sprom, CC_SPROM)
    } else if onchip && otp_hw && otp_fits {
        (Src::Otp, CC_SPROM + otp_off)
    } else {
        (Src::Unadvertised, CC_SPROM)
    };
    // 2. The 4331's PA-line mux off (Linux: ext_pa_lines_ctl(cc, false), made for both sources).
    let ctl_pre = unsafe { r32(bar0, CC_CHIPCTL) };
    let ctl_clear = ctl_pre & !CHIPCTL_4331_PA_LINES;
    let mut writes = 0u32;
    let mut ctl_mid = ctl_pre;
    if ctl_clear != ctl_pre {
        unsafe { w32(bar0, CC_CHIPCTL, ctl_clear) };
        writes += 1;
        ctl_mid = unsafe { r32(bar0, CC_CHIPCTL) };
    }
    // 3. The read.
    let mut words = alloc::vec::Vec::with_capacity(sprom::WORDS);
    for i in 0..sprom::WORDS {
        words.push(unsafe { r16(bar0, base + (i as u64) * 2) });
    }
    // 4. CHIPCTL's pre-image back (the undo; Linux re-derives the bits, the pre-image is exact).
    if writes > 0 {
        unsafe { w32(bar0, CC_CHIPCTL, ctl_pre) };
        writes += 1;
    }
    let ctl_post = unsafe { r32(bar0, CC_CHIPCTL) };
    *SRC.lock() = Some(src);
    serial_println!(
        "[wifi6] sprom-src cap={:#010x} cap-sprom={} srom-ctl={:#010x} present={} chipst={:#010x} chipst-sprom={} chipst-otp={} cap-otps={} otps={:#010x} gu-prog-hw={} otpl={:#010x} otp-off={:#05x} -> src={} base=cc+{:#05x} chipctl pre={:#010x} pa-lines={:#06x} wrote-clear={:#010x} readback={:#010x} restored={:#010x} {} writes={} — bcma sprom.c bcma_sprom_get: ext_available, onchip_available/_offset, bcm4331_ext_pa_lines_ctl(false) before the read",
        cap, (cap & CAP_SPROM != 0) as u8, srom_ctl, (srom_ctl & SROM_CONTROL_PRESENT) as u8, chipst,
        (chipst & CHIPST_4331_SPROM_PRESENT != 0) as u8, (chipst & CHIPST_4331_OTP_PRESENT != 0) as u8,
        (cap & CAP_OTPS) >> 19, otps, otp_hw as u8, otpl, otp_off, src.token(), base,
        ctl_pre, ctl_pre & CHIPCTL_4331_PA_LINES, ctl_clear, ctl_mid, ctl_post,
        if ctl_post == ctl_pre { "MATCH" } else { "MISMATCH" }, writes
    );
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
    writes
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
    let src = (*SRC.lock()).map(Src::token).unwrap_or("unread");
    serial_println!(
        "[wifi6] sprom mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} src={} rev={} plausible={} oui={} board={:#06x} phy={}/{} radio={} -> {} (rev-ok={} board-vs-ssid={} mac-ok={} board-rev={:#06x} boardflags={:#010x} ant-2g={:#04x} ant-5g={:#04x}; il0macaddr @+0x8c (SSB_SPROM8_IL0MAC) 3 BE words; radio is the wifi4 V4 gate read, NOT a SPROM field; CRC not computed)",
        m[0], m[1], m[2], m[3], m[4], m[5], src, id.rev, mac_plausible(&id) as u8,
        oui_vendor(&m).unwrap_or("unlisted"), id.board_type,
        (phy >> 8) & 0xF, phy & 0xF, radio_s, id.verdict.token(),
        id.rev_ok as u8, if id.board_matches_ssid { "MATCH" } else { "MISMATCH" }, id.mac_ok as u8,
        id.board_rev, id.boardflags, id.ant_2g, id.ant_5g
    );
    if id.verdict == Verdict::Blocked {
        serial_println!(
            "[wifi6] sprom BLOCKED — the shadow reads uniform WITH the PA-line mux cleared (see `[wifi6] sprom-src`): read its src/present/gu-prog-hw fields — no SPROM attached and no OTP HW region is the board's truth, not this read's; OTP contents through OTPP stay unmade"
        );
    }
}

/// WIFI10 (B646): the station MAC S7 transmits from — the SPROM `il0macaddr` this boot read, only when
/// S2r judged it plausible (unicast, not all-zero, not all-ones: `wifi_core::sprom::decode`; and, since
/// WIFIMAC B669, an OUI in [`ACCEPTED_OUIS`] — the identity line's `plausible=1`).
#[cfg(feature = "wifi10")]
pub(super) fn mac() -> Option<[u8; 6]> {
    let id = (*ID.lock())?;
    if mac_plausible(&id) { Some(id.mac) } else { None }
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
