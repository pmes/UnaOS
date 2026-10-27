// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (the d11's 64-bit TX DMA engine, its TX-status FIFO and the MAC address
//! filter; no CODEX §2 handler owns a radio register).
//!
//! WIFI10 (rmbp-ledger B646, R110) — rung S7's FIRST HALF, one frame out of the chip: a probe request
//! (management, broadcast DA and wildcard BSSID, wildcard SSID, the four basic CCK rates, DS channel 11)
//! at 1 Mbit/s CCK through ONE 64-bit TX DMA ring on controller 1 (b43's non-QoS AC_BE ring,
//! `dma.c:1089`, `:1330`), as a two-descriptor frame — the 118-byte FW598 TX header (`xmit.h:25-65`,
//! size `xmit.h:192-196`) on the first slot, the 802.11 frame on the second (`dma.c:1186-1286`) — and
//! its verdict read from the microcode's TX-status FIFO (`XMITSTAT_0/1`, `main.c:1334-1360`). The
//! probe RESPONSES are then heard by S6's receiver (`rx.rs`: `arm` / `poll` / `finish`), the answer
//! counted by addr1 == our SPROM MAC.
//!
//! **Provenance (R110, R95 §2).** Register offsets, bits, layouts and sequences from Linux b43
//! (`dma.c`, `dma.h`, `xmit.c`, `xmit.h`, `main.c`, `b43.h`), each cited beside its use and in
//! `docs/dev/evidence/wifi10/WIFI10.md`. The frame itself is IEEE Std 802.11 (`[PUBLIC]`). No text
//! copied, no function transcribed.
//!
//! **What this rung does NOT build, said on the wire.** HT-PHY TX power control (`phy_ht.c:589-640`,
//! `b43_phy_ht_tx_power_ctl_setup`) needs the SPROM's per-core PA coefficients, whose rev-8 SPROM
//! offsets live in `drivers/bcma/sprom.c` — outside the arc's source list — so the frame leaves at the
//! fixed power index the init tables left in `TXPCTL_CMD_C1..C3` (read and printed by
//! `htphyinit::tx_prep`), with power control OFF exactly as b43 holds it between `:999` and `:1004`.
//! The idle-TSSI tone (`:1001`) and the TSSI setup (`:1003`) serve only that power control and are
//! not run. No template RAM is written (templates are the ucode's own beacons / probe responses,
//! `main.c:1708-1726`; PRMAXTIME stays 1 so it answers no probe). No RTS/CTS, no encryption, no QoS.
//!
//! **Residual, accepted by design (S7 is the transmit rung).** `MACFILTER_SELF` now carries the station
//! MAC (`main.c:705-709`), so the running microcode ACKs unicast frames addressed to it — the probe
//! responses this rung asks for. That is the first time this tree lets the d11 emit anything, and the
//! rung's end line counts every write class that made it possible.

use super::rx::{shm_r16, shm_w16, w16, Writes};
use super::{r32, settle, w32, D11_MACCTL, D11_SHM_CONTROL, D11_SHM_DATA, MACCTL_BE};
use alloc::string::String;
use alloc::vec::Vec;

// ── The 64-bit DMA engine, controller 1 (b43.h:70; dma.h:85-112) ────────────────────────────────
/// `b43.h:70` — `B43_MMIO_DMA64_BASE1`; `dma.c:1089` sets it up as `tx_ring_AC_BE`, `dma.c:1330` picks
/// that ring for every frame when QoS is off.
const DMA64_BASE1: u64 = 0x0240;
const TXCTL: u64 = DMA64_BASE1 + 0x00; // dma.h:85
const TXINDEX: u64 = DMA64_BASE1 + 0x04; // dma.h:93
const TXRINGLO: u64 = DMA64_BASE1 + 0x08; // dma.h:94
const TXRINGHI: u64 = DMA64_BASE1 + 0x0C; // dma.h:95
const TXSTATUS: u64 = DMA64_BASE1 + 0x10; // dma.h:96
const TXERROR: u64 = DMA64_BASE1 + 0x14; // dma.h:104
const TXENABLE: u32 = 0x0000_0001; // dma.h:86
const TXPARITYDISABLE: u32 = 0x0000_0800; // dma.h:90
const TXADDREXT_MASK: u32 = 0x0003_0000; // dma.h:91
const TXADDREXT_SHIFT: u32 = 16; // dma.h:92
const TXSTATDPTR: u32 = 0x0000_1FFF; // dma.h:97
const TXSTAT: u32 = 0xF000_0000; // dma.h:98
const TXSTAT_DISABLED: u32 = 0x0000_0000; // dma.h:99
const TXSTAT_ACTIVE: u32 = 0x1000_0000; // dma.h:100
const TXSTAT_IDLEWAIT: u32 = 0x2000_0000; // dma.h:101
const TXSTAT_STOPPED: u32 = 0x3000_0000; // dma.h:102
const TXSTAT_SUSP: u32 = 0x4000_0000; // dma.h:103
const TXERR: u32 = 0xF000_0000; // dma.h:106
/// `dma.h:149-152` — descriptor control 0.
const DCTL0_DTABLEEND: u32 = 0x1000_0000;
const DCTL0_IRQ: u32 = 0x2000_0000;
const DCTL0_FRAMEEND: u32 = 0x4000_0000;
const DCTL0_FRAMESTART: u32 = 0x8000_0000;
/// `dma.h:153-155` — descriptor control 1.
const DCTL1_BYTECNT: u32 = 0x0000_1FFF;
const DCTL1_ADDREXT_MASK: u32 = 0x0003_0000;
const DCTL1_ADDREXT_SHIFT: u32 = 16;
const DESC_BYTES: usize = 16; // dma.h:143-148
/// `dma.h:166` + `dma.c:407-428` — 8 KiB ring memory, 8 KiB aligned (the same rule S6's RX ring obeys).
const RINGMEM: usize = 8192;
/// The brief's ring: ONE ring; 64 slots (b43's 256, `dma.h:172`, is a tuning knob, not a hardware fact).
const SLOTS: usize = 64;
/// `dma.c:35` — two slots per frame: the header, then the body.
const TX_SLOTS_PER_FRAME: usize = 2;
/// `bcma.h:481-484` + `bcma/core.c:140-149` (PCI host, IOST.DMA64 set) — translation in the HIGH word
/// (`dma.c:1033-1047`, `:37-68`); the value S6 proved on this engine's RX side.
const DMA_TRANSLATION_MASK: u32 = 0xC000_0000;
const DMA_TRANSLATION_DMA64_CMT: u32 = 0x8000_0000;
/// `dma.c:1133-1150` — the cookie: `(controller index + 1) << 12 | slot`; controller 1 => 0x2000.
const COOKIE_RING1: u16 = 0x2000;

// ── The TX-status FIFO (b43.h:53-54; decode main.c:1341-1356) ───────────────────────────────────
const XMITSTAT_0: u64 = 0x0170;
const XMITSTAT_1: u64 = 0x0174;
/// `xmit.h:226-235` — `supp_reason`.
const SUPP_NAMES: [&str; 8] = ["none", "pmq", "flush", "prev-frag", "chan-mismatch", "lifetime", "underflow", "ab-nack"];

// ── The MAC address filter (b43.h:106-107, :399; main.c:657-677) ────────────────────────────────
const MACFILTER_CONTROL: u64 = 0x0420;
const MACFILTER_DATA: u64 = 0x0422;
const MACFILTER_SELF: u16 = 0x0000;
/// `b43.h:400` — the BSSID match entry (STATIONRATE B855: the kept station's receive filter, `rx::station_filter`).
pub(super) const MACFILTER_BSSID: u16 = 0x0003;
/// `main.c:665` — the offset is OR'd with 0x0020 before the select.
const MACFILTER_SELECT: u16 = 0x0020;

// ── Template RAM (STATIONTEMPLATE, rmbp-ledger B862; b43.h:45-46; main.c:465-477, :679-703) ─────────────────────
const RAM_CONTROL: u64 = 0x0130;
const RAM_DATA: u64 = 0x0134;
/// `main.c:701` — the station MAC then the BSSID, 12 octets as three words from template RAM offset 0x20.
const TEMPLATE_MAC_BSSID: u32 = 0x0020;

// ── SHM words the ucode's own TX reads (b43.h; main.c:3162-3216) ────────────────────────────────
const SHM_SH_ACKCTSPHYCTL: u16 = 0x0022; // b43.h:306
const SHM_SH_BEACPHYCTL: u16 = 0x0054; // b43.h:302
const SHM_SH_PRPHYCTL: u16 = 0x0188; // b43.h:312
/// `main.c:3162-3175` — the rate memory: `shared[base + (ratecode & 0xF) * 2 + 0x20] <- shared[base + …]`.
const SHM_RATE_OFDM_BASE: u16 = 0x0480;
const SHM_RATE_CCK_BASE: u16 = 0x04C0;
const SHM_RATE_COPY_OFF: u16 = 0x0020;
/// `xmit.c:96-131` — the PLCP rate codes b43 keys the rate memory on (CCK 1/2/5.5/11, OFDM 6..54).
const CCK_RATECODES: [u8; 4] = [0x0A, 0x14, 0x37, 0x6E];
const OFDM_RATECODES: [u8; 8] = [0x0B, 0x0F, 0x0A, 0x0E, 0x09, 0x0D, 0x08, 0x0C];
/// STATIONRATE (B855): the rates the two tables above code, in 500 kbit/s units (`b43.h:593-604`, IEEE §9.4.2.3).
const CCK_RATES: [u8; 4] = [2, 4, 11, 22];
const OFDM_RATES: [u8; 8] = [12, 18, 24, 36, 48, 72, 96, 108];
/// `xmit.c:22-35` + `xmit.h:177-187` — PHY control word 1's coding rate | modulation per OFDM rate (6..54).
const OFDM_PHY1: [u16; 8] = [0x0000, 0x0200, 0x0800, 0x0A00, 0x1000, 0x1200, 0x1900, 0x1A00];

// ── The FW598 TX header (xmit.h:25-65; 112 + 6 bytes, xmit.h:192-196) ───────────────────────────
const TXH_SIZE: usize = 118;
const TXH_MAC_CTL: usize = 0; // __le32 mac_ctl
const TXH_MAC_FRAME_CTL: usize = 4; // __le16, a copy of the frame's FC
const TXH_PHY_CTL: usize = 8; // __le16
const TXH_PHY_CTL1: usize = 10; // __le16
const TXH_PHY_CTL1_FB: usize = 12; // __le16
const TXH_PHY_RATE: usize = 18; // u8
const TXH_EXTRA_FT: usize = 20; // u8
const TXH_CHAN_RADIO_CODE: usize = 21; // u8
const TXH_TX_RECEIVER: usize = 38; // [u8; 6]
const TXH_PLCP_FB: usize = 54; // plcp_hdr6
const TXH_DUR_FB: usize = 60; // __le16
const TXH_COOKIE_598: usize = 76; // format_598.cookie
const TXH_PLCP_598: usize = 112; // format_598.plcp
/// `xmit.h:124-127` — MAC control: hardware sequence number, start of MSDU, long-frame retry limit.
const TXH_MAC_HWSEQ: u32 = 0x0000_0010;
const TXH_MAC_STMSDU: u32 = 0x0000_0008;
const TXH_MAC_LONGFRAME: u32 = 0x0000_0002;
/// `xmit.h:128` — immediate ACK expected; b43 sets it on every frame mac80211 does not mark NO_ACK
/// (`xmit.c:399-400`), i.e. every unicast frame (WIFIAUTH B788: the auth and association requests).
const TXH_MAC_ACK: u32 = 0x0000_0001;
/// `xmit.h:149`, `:157`, `:160` — PHY control: CCK encoding, antenna 0/1 auto, the TX-power field.
const TXH_PHY_ENC_CCK: u16 = 0x0000;
/// `xmit.h:150` — OFDM encoding (STATIONRATE B855).
const TXH_PHY_ENC_OFDM: u16 = 0x0001;
/// `xmit.h:153` — short preamble for the primary rate (b43 `xmit.c:374-375`; STATIONSHORTPRE B862). The fallback's own bit
/// (`B43_TXH_MAC_FB_SHORTPRMBL`, `xmit.h:103`) stays clear: the fallback is CCK 1, long preamble only.
const TXH_PHY_SHORTPRMBL: u16 = 0x0010;
const TXH_PHY_ANT01AUTO: u16 = 0x00C0;
const TXH_PHY_TXPWR: u16 = 0xFC00;
/// `xmit.h:167` + `xmit.c:167-181` — PHY control word 1 for a CCK rate on an HT-PHY is the 20 MHz bandwidth alone.
const TXH_PHY1_BW_20: u16 = 0x0002;
/// `xmit.h:132` — extra frame types: the fallback encoding is CCK.
const TXH_EFT_FB_CCK: u8 = 0x00;
// `xmit.c:96-99` + `b43.h:593` — CCK 1 Mbit/s (units of 500 kbit/s): every rung's frame and every frame's fallback.
pub(super) use wifi_core::ieee80211::RATE_1M;
/// `xmit.c:688` — the FCS the hardware appends is in the PLCP length.
const FCS_LEN: usize = 4;
/// IEEE Std 802.11-2020 aSIFSTime for DSSS / HR-DSSS / ERP (2.4 GHz), microseconds (STATIONDURATION B862).
const SIFS_US: u16 = 10;

// ── The retry limits (WIFIAUTH B788; b43.h:229, :345-346, :608-609; main.c:3972-3985, called at core init :4909) ──
/// `b43.h:229` — the SHM routing enum's third entry, SCRATCH. Scratch is WORD-addressed (`main.c:555-569`:
/// only the SHARED routing shifts the offset).
const SHM_ROUTE_SCRATCH: u32 = 0x0002;
const SHM_SC_SRLIMIT: u32 = 0x0006;
const SHM_SC_LRLIMIT: u32 = 0x0007;
const RETRY_SHORT: u16 = 7;
const RETRY_LONG: u16 = 4;
/// STATIONMTU (B843; was WIFIDHCP B831's 512-octet pair): a frame's body owns one `BODY_STRIDE` region of `buf` (body `k`
/// at `k * BODY_STRIDE`, 2 KiB aligned) and its TX header one `HDR_STRIDE` region after all the bodies (header `k` at
/// `BUF_FRAMES * BODY_STRIDE + k * HDR_STRIDE`, >= `TXH_SIZE`), so the body cap IS the body stride: a 1514-octet Ethernet
/// frame is 1532 octets of air and fits (the descriptor byte count is 13 bits, `DCTL1_BYTECNT`). `buf` holds `BUF_FRAMES`
/// bodies and headers (17 KiB).
const BODY_STRIDE: u64 = 2048;
const HDR_STRIDE: u64 = 128;
pub(in crate::wifi) const FRAME_MAX: usize = BODY_STRIDE as usize;
const BUF_FRAMES: usize = 8;
const BUF_BYTES: usize = BUF_FRAMES * (BODY_STRIDE + HDR_STRIDE) as usize;
/// Frame numbers on the descriptor ring: two slots each, 32 per lap. Frame `n` uses buffer pair `n % BUF_FRAMES`
/// (free again once its status was read: `fire_frame` waits for it; the kept station's `Queue` keeps at most
/// `BUF_FRAMES` frames in flight, and consecutive frame numbers mod 32 map to distinct pairs) and slots `2n`/`2n+1`;
/// the index wraps to slot 0 after slot 63, which carries DTABLEEND (`fill`; b43 `dma.c` next_slot).
pub(in crate::wifi) const FRAMES: usize = SLOTS / TX_SLOTS_PER_FRAME;

/// How long the TX-status FIFO is polled after the poke (1 ms steps).
const TXSTAT_WAIT_MS: u32 = 200;

/// The programmed ring: host memory the engine owns (leaked on purpose, as S6's).
pub(super) struct Prep {
    ring: u64,
    buf: u64,
    pub rate_mem: u32,
    pub reset_ok: bool,
    pub txstatus0: u32,
}

/// One frame's status as the microcode reported it (`main.c:1341-1356`).
#[derive(Default, Clone, Copy)]
pub(super) struct Report {
    pub status_seen: bool,
    pub cookie: u16,
    pub cookie_match: bool,
    pub frame_count: u8,
    pub rts_count: u8,
    pub supp: u8,
    pub acked: bool,
    pub intermediate: bool,
    pub for_ampdu: bool,
    pub phy_stat: u8,
    pub seq: u16,
    pub v0: u32,
    pub v1: u32,
    /// Entries read that were not ours.
    pub others: u32,
    pub dptr_before: u32,
    pub dptr_after: u32,
    pub txstatus: u32,
    pub txerror: u32,
    pub waited_ms: u32,
    pub frame_len: usize,
}

impl Report {
    /// SENT ⇔ the ucode reported OUR cookie, transmitted it at least once and suppressed nothing.
    pub fn sent(&self) -> bool {
        self.status_seen && self.cookie_match && self.frame_count > 0 && self.supp == 0
    }
    /// STATIONRATE (B855): SENT, ACKED, and within the long-retry tries the primary rate owns — past them the ucode sent at
    /// the fallback rate (`xmit.c:884`: `frame_count > retry_limit`; the limit is `RETRY_LONG`, `TXH_MAC_LONGFRAME` set).
    pub fn acked_at_rate(&self) -> bool {
        self.sent() && self.acked && (self.frame_count as u16) <= RETRY_LONG
    }
    pub fn reason(&self) -> String {
        if !self.status_seen {
            alloc::format!("no-txstatus(dptr {}->{} txstatus={:#010x} txerror={:#010x})", self.dptr_before, self.dptr_after, self.txstatus, self.txerror)
        } else if !self.cookie_match {
            alloc::format!("foreign-cookie={:#06x}", self.cookie)
        } else if self.frame_count == 0 {
            alloc::format!("frame_count=0 supp={}", SUPP_NAMES[(self.supp & 7) as usize])
        } else if self.supp != 0 {
            alloc::format!("supp={}", SUPP_NAMES[(self.supp & 7) as usize])
        } else {
            String::from("none")
        }
    }
}

fn txstat_name(v: u32) -> &'static str {
    match v & TXSTAT {
        TXSTAT_DISABLED => "disabled",
        TXSTAT_ACTIVE => "active",
        TXSTAT_IDLEWAIT => "idle-wait",
        TXSTAT_STOPPED => "stopped",
        TXSTAT_SUSP => "suspended",
        _ => "?",
    }
}

/// `dma.c:478-530` — the TX engine reset: wait for a quiescent state (10 x 1 ms), TXCTL <- 0, wait
/// DISABLED (10 x 1 ms), 1 ms more. `true` = DISABLED was read.
fn tx_reset(bar0: u64, w: &mut Writes) -> bool {
    for _ in 0..10 {
        let st = unsafe { r32(bar0, TXSTATUS) } & TXSTAT;
        if st == TXSTAT_DISABLED || st == TXSTAT_IDLEWAIT || st == TXSTAT_STOPPED {
            break;
        }
        crate::arch::sched::sleep_ms(1);
    }
    unsafe { w32(bar0, TXCTL, 0) };
    w.dma += 1;
    let mut ok = false;
    for _ in 0..10 {
        if unsafe { r32(bar0, TXSTATUS) } & TXSTAT == TXSTAT_DISABLED {
            ok = true;
            break;
        }
        crate::arch::sched::sleep_ms(1);
    }
    crate::arch::sched::sleep_ms(1);
    ok
}

/// `main.c:655-677` — filter entry `entry` (`MACFILTER_SELF`, or `MACFILTER_BSSID` since STATIONRATE B855) <- `mac`, three
/// halfwords through the data port.
pub(super) fn macfilter(bar0: u64, entry: u16, mac: [u8; 6], w: &mut Writes) {
    unsafe {
        w16(bar0, MACFILTER_CONTROL, entry | MACFILTER_SELECT);
        w16(bar0, MACFILTER_DATA, u16::from_le_bytes([mac[0], mac[1]]));
        w16(bar0, MACFILTER_DATA, u16::from_le_bytes([mac[2], mac[3]]));
        w16(bar0, MACFILTER_DATA, u16::from_le_bytes([mac[4], mac[5]]));
    }
    w.macfilter += 4;
}

/// STATIONTEMPLATE (B862) — the template half of `main.c:679-703` (`b43_write_mac_bssid_templates`; its filter half is
/// `macfilter`, the one filter writer, already run by `rx::station_filter`): the station MAC then the BSSID, 12 octets as
/// three little-endian words, into template RAM at 0x20/0x24/0x28 through RAM_CONTROL / RAM_DATA (`main.c:465-477`), each
/// word byte-swapped when MACCTL says big-endian. Written live, as b43 writes it from bss_info_changed (`main.c:4139-4140`,
/// before its suspend). Not counted in `Writes`: `join::keep` runs after the pass's census line. Returns the words written.
pub(super) fn templates(bar0: u64, mac: [u8; 6], bssid: [u8; 6]) -> u32 {
    let mut b = [0u8; 12];
    b[..6].copy_from_slice(&mac);
    b[6..].copy_from_slice(&bssid);
    let be = unsafe { r32(bar0, D11_MACCTL) } & MACCTL_BE != 0;
    let mut n = 0u32;
    for i in 0..3usize {
        let v = u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        unsafe {
            w32(bar0, RAM_CONTROL, TEMPLATE_MAC_BSSID + 4 * i as u32);
            w32(bar0, RAM_DATA, if be { v.swap_bytes() } else { v });
        }
        n += 1;
    }
    n
}

/// `main.c:3162-3199` — the rate memory for a G/N/HT PHY: 8 OFDM + 4 CCK entries copied to +0x20.
fn rate_memory_init(bar0: u64, w: &mut Writes) -> u32 {
    let mut n = 0u32;
    for rc in OFDM_RATECODES.iter() {
        let off = SHM_RATE_OFDM_BASE + ((*rc as u16) & 0x000F) * 2;
        let v = shm_r16(bar0, off, w);
        shm_w16(bar0, off + SHM_RATE_COPY_OFF, v, w);
        n += 1;
    }
    for rc in CCK_RATECODES.iter() {
        let off = SHM_RATE_CCK_BASE + ((*rc as u16) & 0x000F) * 2;
        let v = shm_r16(bar0, off, w);
        shm_w16(bar0, off + SHM_RATE_COPY_OFF, v, w);
        n += 1;
    }
    n
}

/// `main.c:3972-3985` — the ucode's short and long retry limits into SHM scratch (b43's defaults 7/4,
/// `b43.h:608-609`). The first unicast frames this tree sends; without a written limit the counter reads
/// whatever the upload left.
fn retry_limits(bar0: u64, w: &mut Writes) {
    unsafe {
        w32(bar0, D11_SHM_CONTROL, (SHM_ROUTE_SCRATCH << 16) | SHM_SC_SRLIMIT);
        w16(bar0, D11_SHM_DATA, RETRY_SHORT);
        w32(bar0, D11_SHM_CONTROL, (SHM_ROUTE_SCRATCH << 16) | SHM_SC_LRLIMIT);
        w16(bar0, D11_SHM_DATA, RETRY_LONG);
    }
    w.shm += 4;
}

/// `main.c:3206-3216` — the PHY TX control words the ucode uses for its own frames (beacon, ACK/CTS,
/// probe response): CCK, antenna auto, full TX-power field.
fn phytxctl_defaults(bar0: u64, w: &mut Writes) {
    let ctl = TXH_PHY_ENC_CCK | TXH_PHY_ANT01AUTO | TXH_PHY_TXPWR;
    shm_w16(bar0, SHM_SH_BEACPHYCTL, ctl, w);
    shm_w16(bar0, SHM_SH_ACKCTSPHYCTL, ctl, w);
    shm_w16(bar0, SHM_SH_PRPHYCTL, ctl, w);
}

/// `dma.c:177-211` — one 64-bit TX descriptor: ctl0 flags, ctl1 byte count + address extension, the
/// buffer address with the translation in the high word.
unsafe fn fill(ring: u64, slot: usize, buf: u64, len: usize, ctl0_flags: u32, w: &mut Writes) {
    let lo = buf as u32;
    let hi_raw = (buf >> 32) as u32;
    let hi = (hi_raw & !DMA_TRANSLATION_MASK) | DMA_TRANSLATION_DMA64_CMT;
    let ext = (hi_raw & DMA_TRANSLATION_MASK) >> 30;
    let ctl0 = ctl0_flags | if slot == SLOTS - 1 { DCTL0_DTABLEEND } else { 0 };
    let ctl1 = (len as u32 & DCTL1_BYTECNT) | ((ext << DCTL1_ADDREXT_SHIFT) & DCTL1_ADDREXT_MASK);
    let d = ring + (slot * DESC_BYTES) as u64;
    core::ptr::write_volatile(d as *mut u32, ctl0);
    core::ptr::write_volatile((d + 4) as *mut u32, ctl1);
    core::ptr::write_volatile((d + 8) as *mut u32, lo);
    core::ptr::write_volatile((d + 12) as *mut u32, hi);
    w.tx_desc += 4;
}

/// IEEE Std 802.11 §9.3.3.9 — a probe request: FC type 0 subtype 4, duration 0, DA broadcast, SA the
/// station, BSSID wildcard, sequence 0 (the ucode assigns it, `TXH_MAC_HWSEQ`); SSID IE wildcard
/// (§9.4.2.2), Supported Rates 1/2/5.5/11 basic (§9.4.2.3), DS Parameter Set = `ch` (§9.4.2.4).
pub(super) fn probe_request(sa: [u8; 6], ch: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(40);
    f.push(0x40); // (subtype 4 << 4) | (type 0 << 2)
    f.push(0x00); // no flags
    f.extend_from_slice(&[0, 0]); // duration
    f.extend_from_slice(&[0xFF; 6]); // addr1 DA
    f.extend_from_slice(&sa); // addr2 SA
    f.extend_from_slice(&[0xFF; 6]); // addr3 BSSID
    f.extend_from_slice(&[0, 0]); // sequence control
    f.extend_from_slice(&[0, 0]); // SSID: wildcard
    f.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8B, 0x96]); // Supported Rates
    f.extend_from_slice(&[3, 1, ch as u8]); // DS Parameter Set
    f
}

/// The PLCP rate code of `rate` (500 kbit/s units) and whether it is OFDM (`xmit.c:96-131`); `None` = not a legacy rate.
fn ratecode(rate: u8) -> Option<(u8, bool)> {
    if let Some(i) = CCK_RATES.iter().position(|r| *r == rate) {
        return Some((CCK_RATECODES[i], false));
    }
    OFDM_RATES.iter().position(|r| *r == rate).map(|i| (OFDM_RATECODES[i], true))
}

/// `xmit.c:167-189` — PHY control word 1: a CCK rate on an HT-PHY is the 20 MHz bandwidth alone; an OFDM rate adds its
/// coding rate and modulation (`xmit.c:22-35`), SISO.
fn phy_ctl1(rate: u8) -> u16 {
    match OFDM_RATES.iter().position(|r| *r == rate) {
        Some(i) => TXH_PHY1_BW_20 | OFDM_PHY1[i],
        None => TXH_PHY1_BW_20,
    }
}

/// `xmit.c:136-165` — the PLCP header for `octets` at `rate`: OFDM = rate code | length << 5 (le32); CCK = rate code,
/// service 0x04 (0x84 when CCK 11's rounded-up length needs the length-extension bit), length in microseconds
/// (`octets * 16 / rate`, rounded up). A rate outside the legacy set is coded as CCK 1.
fn plcp(out: &mut [u8], octets: usize, rate: u8) {
    let (rc, ofdm) = ratecode(rate).unwrap_or((CCK_RATECODES[0], false));
    let rate = if ratecode(rate).is_some() { rate as u32 } else { RATE_1M as u32 };
    out[..6].fill(0);
    let o = octets as u32;
    if ofdm {
        out[0..4].copy_from_slice(&((rc as u32) | (o << 5)).to_le_bytes());
    } else {
        let mut plen = o * 16 / rate;
        out[1] = 0x04;
        if o * 16 % rate > 0 {
            plen += 1;
            if rate == wifi_core::ieee80211::RATE_11M as u32 && o * 8 % 11 < 4 {
                out[1] = 0x84;
            }
        }
        out[0] = rc;
        out[2] = plen as u8;
        out[3] = (plen >> 8) as u8;
    }
}

/// `xmit.c:235-567` for a frame at `rate` (STATIONRATE B855: the rungs pass `RATE_1M`; the kept station its chosen rate),
/// no key, no RTS/CTS, no short preamble, the fallback ALWAYS CCK 1 (`plcp_fb`, `phy_ctl1_fb`, extra_ft FB_CCK: `:354-361`):
/// phy_rate (`:271-274`), phy_ctl's encoding (`:370-371`), the FC copy, the receiver, dur_fb = the frame's own duration (0: `:279-286`), both PLCPs,
/// extra_ft CCK fallback, chan_radio_code = the channel (`:360-364`), phy_ctl CCK + ANT01AUTO
/// (`:367-392`), mac_ctl HWSEQ + STMSDU + LONGFRAME (`:396-421`) + ACK on a unicast addr1 (`:399-400`), phy_ctl1 and
/// its fallback = BW_20 (`:554-558`, `:167-181`), the cookie (`:541-552`). Everything else stays 0 (`:263`).
/// STATIONAIR (B862): the rate arrives as ONE `TxRate`; phy_ctl carries SHORTPRMBL (`xmit.c:374-375`) only for a short
/// CCK 2/5.5/11 rate (`short_ok`); dur_fb = `duration`'s NAV re-taken at the CCK 1 long fallback (SIFS + ACK = 314 us) when
/// a Duration was computed and the rate is not 1 Mbit/s, else the computed Duration, else the frame's own field (the rungs'
/// management frames: byte-identical). THE ONE DURATION WRITER is `post`: it writes `duration(frame, r)` into the frame's
/// DMA body copy at octets 2-3; the codec's output (`wifi_core::ieee80211`) is never re-encoded and no other site (no
/// codec, no Link) writes a Duration. This divergence from b43 is deliberate: b43 `xmit.c:287` takes the fallback duration
/// from `ieee80211_generic_frame_duration(fragment_len, fbrate)`, the whole fragment's airtime; we take the ACK's.
fn txhdr(frame: &[u8], ch: u16, cookie: u16, r: TxRate) -> [u8; TXH_SIZE] {
    let rate = r.rate;
    let mut h = [0u8; TXH_SIZE];
    let octets = frame.len() + FCS_LEN;
    let unicast = frame[4] & 0x01 == 0;
    let mac_ctl = TXH_MAC_HWSEQ | TXH_MAC_STMSDU | TXH_MAC_LONGFRAME | if unicast { TXH_MAC_ACK } else { 0 };
    h[TXH_MAC_CTL..TXH_MAC_CTL + 4].copy_from_slice(&mac_ctl.to_le_bytes());
    h[TXH_MAC_FRAME_CTL..TXH_MAC_FRAME_CTL + 2].copy_from_slice(&frame[0..2]);
    let (rc, ofdm) = ratecode(rate).unwrap_or((CCK_RATECODES[0], false));
    let phy_ctl = (if ofdm { TXH_PHY_ENC_OFDM } else { TXH_PHY_ENC_CCK }) | TXH_PHY_ANT01AUTO | if short_ok(r) { TXH_PHY_SHORTPRMBL } else { 0 };
    h[TXH_PHY_CTL..TXH_PHY_CTL + 2].copy_from_slice(&phy_ctl.to_le_bytes());
    h[TXH_PHY_CTL1..TXH_PHY_CTL1 + 2].copy_from_slice(&phy_ctl1(if ratecode(rate).is_some() { rate } else { RATE_1M }).to_le_bytes());
    h[TXH_PHY_CTL1_FB..TXH_PHY_CTL1_FB + 2].copy_from_slice(&phy_ctl1(RATE_1M).to_le_bytes());
    h[TXH_PHY_RATE] = rc;
    h[TXH_EXTRA_FT] = TXH_EFT_FB_CCK;
    h[TXH_CHAN_RADIO_CODE] = ch as u8;
    h[TXH_TX_RECEIVER..TXH_TX_RECEIVER + 6].copy_from_slice(&frame[4..10]);
    plcp(&mut h[TXH_PLCP_FB..TXH_PLCP_FB + 6], octets, RATE_1M);
    let dur_fb = match duration(frame, r) {
        Some(d) if d != 0 && rate != RATE_1M => nav_us(TxRate::long(RATE_1M)).to_le_bytes(),
        Some(d) => d.to_le_bytes(),
        None => [frame[2], frame[3]],
    };
    h[TXH_DUR_FB..TXH_DUR_FB + 2].copy_from_slice(&dur_fb);
    h[TXH_COOKIE_598..TXH_COOKIE_598 + 2].copy_from_slice(&cookie.to_le_bytes());
    plcp(&mut h[TXH_PLCP_598..TXH_PLCP_598 + 6], octets, rate);
    h
}

/// S7 setup, with the MAC still suspended: the ucode's rate memory and PHY-TX-control words, the station
/// MAC into the filter, the TX engine reset and its ring programmed (`dma.c:681-694`). Nothing is
/// transmitted here; the engine has no descriptor to read until `fire` pokes `TXINDEX`.
pub(super) fn prep(bar0: u64, mac: [u8; 6], w: &mut Writes) -> Result<Prep, String> {
    let ring_l = core::alloc::Layout::from_size_align(RINGMEM, RINGMEM).ok();
    let buf_l = core::alloc::Layout::from_size_align(BUF_BYTES, 4096).ok();
    let (Some(rl), Some(bl)) = (ring_l, buf_l) else {
        return Err(String::from("layout — nothing programmed"));
    };
    // Deliberately never freed: the engine owns this memory until TXCTL reads DISABLED, and a failed
    // reset would leave it owning it for the rest of the boot.
    let ring = unsafe { alloc::alloc::alloc_zeroed(rl) } as u64;
    let buf = unsafe { alloc::alloc::alloc_zeroed(bl) } as u64;
    if ring == 0 || buf == 0 {
        return Err(String::from("no-memory — nothing programmed"));
    }
    let rate_mem = rate_memory_init(bar0, w);
    phytxctl_defaults(bar0, w);
    retry_limits(bar0, w);
    macfilter(bar0, MACFILTER_SELF, mac, w);
    let reset_ok = tx_reset(bar0, w);
    // dma.c:681-694 — TXCTL (enable, address extension, parity off on BCMA: dma.c:1076-1081), RINGLO, RINGHI.
    let lo = ring as u32;
    let hi_raw = (ring >> 32) as u32;
    let hi = (hi_raw & !DMA_TRANSLATION_MASK) | DMA_TRANSLATION_DMA64_CMT;
    let ext = (hi_raw & DMA_TRANSLATION_MASK) >> 30;
    let txctl = TXENABLE | ((ext << TXADDREXT_SHIFT) & TXADDREXT_MASK) | TXPARITYDISABLE;
    unsafe {
        w32(bar0, TXCTL, txctl);
        w32(bar0, TXRINGLO, lo);
        w32(bar0, TXRINGHI, hi);
    }
    w.dma += 3;
    let txstatus0 = unsafe { r32(bar0, TXSTATUS) };
    serial_println!(
        "[wifi10] tx-setup ctl=1 ring={:#x} buf={:#x} slots={} txctl={:#010x} ringhi={:#010x} txstatus={:#010x}({}) reset={} rate_mem={} phytxctl=3 retry={}/{} macfilter_self={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} — dma.c:478-530, :681-694; main.c:3162-3216, :657-677",
        ring, buf, SLOTS, txctl, hi, txstatus0, txstat_name(txstatus0), reset_ok as u8, rate_mem, RETRY_SHORT, RETRY_LONG,
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
    Ok(Prep { ring, buf, rate_mem, reset_ok, txstatus0 })
}

/// S7's probe request: frame 0 of the boot's ring (`fire_frame`).
pub(super) fn fire(bar0: u64, p: &Prep, mac: [u8; 6], ch: u16, w: &mut Writes) -> Report {
    fire_frame(bar0, p, &probe_request(mac, ch), ch, 0, "probe-req", RATE_1M, w)
}

/// Frame `n` (0..FRAMES) of the ring out. Precondition: `prep` done and the MAC ENABLED (S6's
/// `rx::arm`, which also wakes the ucode). The header on slot 2n (FRAMESTART), the frame on slot 2n+1
/// (FRAMEEND | IRQ), `TXINDEX` <- the slot after (`dma.c:1186-1286`, `:213-217`), the cookie
/// `(controller + 1) << 12 | 2n` (`dma.c:1133-1150`); then the TX-status FIFO is drained for up to
/// `TXSTAT_WAIT_MS` and the entry carrying our cookie decoded (`main.c:1334-1360`).
pub(super) fn fire_frame(bar0: u64, p: &Prep, frame: &[u8], ch: u16, n: usize, kind: &str, rate: u8, w: &mut Writes) -> Report {
    let mut r = Report::default();
    r.frame_len = frame.len();
    if n >= FRAMES || frame.len() > FRAME_MAX || frame.len() < 24 {
        r.cookie = 0;
        serial_println!("[wifi10] tx-poke kind={} REFUSED reason=frame-slot(n={} len={}) — nothing written", kind, n, frame.len());
        return r;
    }
    let (cookie, txindex, dptr, hdr) = post(bar0, p, frame, ch, n, TxRate::long(rate), w);
    r.cookie = cookie;
    r.dptr_before = dptr;
    let mut hx = String::new();
    for b in hdr.iter().take(24) {
        hx.push_str(&alloc::format!("{:02x}", b));
    }
    serial_println!(
        "[wifi10] tx-poke kind={} len={} plcp_octets={} cookie={:#06x} txindex={} hdr[0..24]={} — xmit.c:235-567, dma.c:1186-1286",
        kind, frame.len(), frame.len() + FCS_LEN, cookie, txindex, hx
    );
    // The status FIFO (main.c:1341-1356): bit 0 of XMITSTAT_0 = an entry is there; reading both words consumes it.
    let mut waited = 0u32;
    while waited < TXSTAT_WAIT_MS {
        let v0 = unsafe { r32(bar0, XMITSTAT_0) };
        if v0 & 0x0000_0001 != 0 {
            let v1 = unsafe { r32(bar0, XMITSTAT_1) };
            let ck = (v0 >> 16) as u16;
            if !r.status_seen && ck == cookie {
                r.status_seen = true;
                r.cookie_match = true;
                decode(v0, v1, &mut r);
                if r.intermediate {
                    // main.c:823 — not final; keep reading for the final one.
                    r.status_seen = false;
                    continue;
                }
                break;
            } else {
                r.others += 1;
                if !r.status_seen {
                    r.cookie = ck;
                    r.v0 = v0;
                    r.v1 = v1;
                }
            }
            continue;
        }
        // WIFIDHCP (B831): a spin, not a sleep (it ran under smolnet's STACK hold); since STATIONTX (B843) only the pass's
        // rungs wait here — the kept station posts through `queue` and reaps through `reap`.
        let _ = settle(1000);
        waited += 1;
    }
    r.waited_ms = waited;
    r.txstatus = unsafe { r32(bar0, TXSTATUS) };
    r.txerror = unsafe { r32(bar0, TXERROR) };
    r.dptr_after = r.txstatus & TXSTATDPTR;
    let _ = settle(1);
    serial_println!(
        "[wifi10] tx-status seen={} cookie={:#06x} match={} frame_count={} rts_count={} supp={} acked={} intermediate={} ampdu={} phy_stat={:#04x} seq={} v0={:#010x} v1={:#010x} others={} dptr={}->{} txstatus={:#010x}({}) txerror={:#010x}(err={}) waited={}ms — main.c:1334-1360, xmit.h:212-235",
        r.status_seen as u8, r.cookie, r.cookie_match as u8, r.frame_count, r.rts_count, SUPP_NAMES[(r.supp & 7) as usize],
        r.acked as u8, r.intermediate as u8, r.for_ampdu as u8, r.phy_stat, r.seq, r.v0, r.v1, r.others, r.dptr_before, r.dptr_after,
        r.txstatus, txstat_name(r.txstatus), r.txerror, (r.txerror & TXERR) >> 28, r.waited_ms
    );
    r
}

/// S7's stop: the TX engine reset (`dma.c:478-530`) and the ring registers cleared (`dma.c:753-760`).
/// The MAC is suspended by S6's `rx::finish` beside it. `true` = DISABLED was read.
pub(super) fn stop(bar0: u64, w: &mut Writes) -> bool {
    let ok = tx_reset(bar0, w);
    unsafe {
        w32(bar0, TXRINGLO, 0);
        w32(bar0, TXRINGHI, 0);
    }
    w.dma += 2;
    ok
}

// ── STATIONMTU / STATIONTX (rmbp-ledger B843): the one posting half and the one status decode, and the kept station's
// asynchronous ring over them ─────────────────────────────────────────────────────────────────────────────────────

/// The posting half of `fire_frame` (and of the kept station's `queue`): the header into pair `n % BUF_FRAMES`'s header
/// region, the frame into its body region, slot 2n (FRAMESTART) and 2n+1 (FRAMEEND | IRQ) filled, `TXINDEX` <- the slot
/// after (`dma.c:1186-1286`, `:213-217`). Precondition (the callers' guards): `n < FRAMES`, 24 <= len <= `FRAME_MAX`.
/// Returns (the cookie `(controller + 1) << 12 | 2n`, the TXINDEX written, the engine's pointer before the poke, the header).
fn post(bar0: u64, p: &Prep, frame: &[u8], ch: u16, n: usize, rate: TxRate, w: &mut Writes) -> (u16, usize, u32, [u8; TXH_SIZE]) {
    let slot = n * TX_SLOTS_PER_FRAME;
    let cookie = COOKIE_RING1 | slot as u16;
    let hdr = txhdr(frame, ch, cookie, rate);
    let k = (n % BUF_FRAMES) as u64;
    let fbuf = p.buf + k * BODY_STRIDE;
    let hbuf = p.buf + BUF_FRAMES as u64 * BODY_STRIDE + k * HDR_STRIDE;
    for (i, b) in hdr.iter().enumerate() {
        unsafe { core::ptr::write_volatile((hbuf + i as u64) as *mut u8, *b) };
    }
    for (i, b) in frame.iter().enumerate() {
        unsafe { core::ptr::write_volatile((fbuf + i as u64) as *mut u8, *b) };
    }
    // STATIONDURATION (B862): the ONE Duration writer — the NAV into the body copy's octets 2-3 (see `txhdr`).
    if let Some(d) = duration(frame, rate) {
        for (i, b) in d.to_le_bytes().iter().enumerate() {
            unsafe { core::ptr::write_volatile((fbuf + 2 + i as u64) as *mut u8, *b) };
        }
    }
    unsafe {
        fill(p.ring, slot, hbuf, TXH_SIZE, DCTL0_FRAMESTART, w);
        fill(p.ring, slot + 1, fbuf, frame.len(), DCTL0_FRAMEEND | DCTL0_IRQ, w);
    }
    let dptr = unsafe { r32(bar0, TXSTATUS) } & TXSTATDPTR;
    // dma.c:1284-1285: wmb, then poke the slot AFTER the frame's last one (slot 0 after slot 63, the table end).
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    let txindex = ((slot + TX_SLOTS_PER_FRAME) % SLOTS) * DESC_BYTES;
    unsafe { w32(bar0, TXINDEX, txindex as u32) };
    w.dma += 1;
    (cookie, txindex, dptr, hdr)
}

/// `main.c:1341-1356` — one TX-status entry's fields into `r` (the caller matched the cookie, `v0 >> 16`).
fn decode(v0: u32, v1: u32, r: &mut Report) {
    let tmp = v0 & 0xFFFF;
    r.frame_count = ((tmp & 0xF000) >> 12) as u8;
    r.rts_count = ((tmp & 0x0F00) >> 8) as u8;
    r.supp = ((tmp & 0x001C) >> 2) as u8;
    r.acked = tmp & 0x0002 != 0;
    r.intermediate = tmp & 0x0040 != 0;
    r.for_ampdu = tmp & 0x0020 != 0;
    r.seq = (v1 & 0xFFFF) as u16;
    r.phy_stat = ((v1 & 0x00FF_0000) >> 16) as u8;
    r.v0 = v0;
    r.v1 = v1;
}

/// Frames the kept station may have posted and not yet reaped: every buffer pair, so a pair is never rewritten while
/// the engine may still read it.
const INFLIGHT: usize = BUF_FRAMES;
/// Per-frame station lines (witness knob only): the first frames, then only a non-success (usbnet's `tx done` shape).
const SAY_FIRST: u32 = 8;

/// STATIONTX (B843) — the kept station's TX ring, usbnet's asynchronous shape: a frame is posted and returns at once;
/// its status is reaped on a later pass (`reap`); `room` is the stack's back-pressure. Frames in flight carry
/// consecutive ring numbers from `first`.
#[derive(Default)]
pub(super) struct Queue {
    first: usize,
    n: usize,
    t0_ms: [u64; BUF_FRAMES],
    pub posted: u32,
    pub sent: u32,
    pub acked: u32,
    pub failed: u32,
    pub stuck: u32,
    pub full: u32,
}

impl Queue {
    /// Room for one more frame right now.
    pub fn room(&self) -> bool {
        self.n < INFLIGHT
    }
    /// Frames posted and not yet reaped.
    pub fn in_flight(&self) -> usize {
        self.n
    }
    fn pop(&mut self) {
        self.first = (self.first + 1) % FRAMES;
        self.n -= 1;
    }
}

fn cookie_of(n: usize) -> u16 {
    COOKIE_RING1 | ((n % FRAMES) * TX_SLOTS_PER_FRAME) as u16
}

/// Post frame `n` on the ring for the kept station and return — no wait (`post`). `false` = refused (no room, counted
/// `full`; or outside the ring's frame / length bounds), nothing written.
pub(super) fn queue(bar0: u64, p: &Prep, q: &mut Queue, frame: &[u8], ch: u16, n: usize, now_ms: u64, rate: TxRate, w: &mut Writes) -> bool {
    if !q.room() {
        q.full += 1;
        return false;
    }
    if n >= FRAMES || frame.len() > FRAME_MAX || frame.len() < 24 {
        return false;
    }
    if q.n == 0 {
        q.first = n;
    }
    q.t0_ms[n % BUF_FRAMES] = now_ms;
    let (cookie, _, _, _) = post(bar0, p, frame, ch, n, rate, w);
    q.n += 1;
    q.posted += 1;
    if cfg!(feature = "witness") && q.posted <= SAY_FIRST {
        serial_println!("[wifi10] station tx n={} len={} cookie={:#06x} q={}", n, frame.len(), cookie, q.n);
    }
    true
}

/// Reap the TX-status FIFO without waiting (`main.c:1334-1360`): each final entry naming an in-flight frame pops it and
/// every earlier frame still unstatused (counted `failed`), the frame counted `sent` (and `acked`) or `failed`; then the
/// oldest frame older than `TXSTAT_WAIT_MS` is abandoned (`stuck`), so back-pressure never wedges.
pub(super) fn reap(bar0: u64, q: &mut Queue, now_ms: u64) {
    for _ in 0..(2 * FRAMES) {
        let v0 = unsafe { r32(bar0, XMITSTAT_0) };
        if v0 & 0x0000_0001 == 0 {
            break;
        }
        let v1 = unsafe { r32(bar0, XMITSTAT_1) };
        let mut r = Report::default();
        decode(v0, v1, &mut r);
        let ck = (v0 >> 16) as u16;
        if r.intermediate || q.n == 0 {
            continue;
        }
        let Some(k) = (0..q.n).find(|&i| cookie_of(q.first + i) == ck) else { continue };
        for _ in 0..k {
            q.pop();
            q.failed += 1;
        }
        q.pop();
        let ok = r.frame_count > 0 && r.supp == 0;
        if ok {
            q.sent += 1;
            if r.acked {
                q.acked += 1;
            }
        } else {
            q.failed += 1;
        }
        if cfg!(feature = "witness") && (q.sent + q.failed <= SAY_FIRST || !ok) {
            serial_println!(
                "[wifi10] station tx-status cookie={:#06x} frame_count={} acked={} supp={}",
                ck, r.frame_count, r.acked as u8, SUPP_NAMES[(r.supp & 7) as usize]
            );
        }
    }
    if q.n > 0 && now_ms.saturating_sub(q.t0_ms[q.first % BUF_FRAMES]) >= TXSTAT_WAIT_MS as u64 {
        q.pop();
        q.stuck += 1;
    }
}

// ── STATIONRATE (rmbp-ledger B855): the TX header's rate fields, known-answer ────────────────────────────────────

/// `tests wifi` (through `join::station_kat`): [plcp-cck1, plcp-cck11, plcp-ofdm24, txhdr-rate], hand-computed from
/// `xmit.c:136-189`. 64 octets = a 60-octet frame + FCS; 1536 = the 1532-octet full frame + FCS.
pub(super) fn rate_kat() -> [bool; 4] {
    let mut o = [0u8; 6];
    plcp(&mut o, 64, RATE_1M);
    let cck1 = o == [0x0A, 0x04, 0x00, 0x02, 0, 0];
    // CCK 11: 1536 * 16 / 22 = 1117 r 2 -> 1118 = 0x045E, 1536 * 8 % 11 = 1 < 4 -> 0x84; 64 -> 47, 512 % 11 = 6 -> 0x04.
    plcp(&mut o, 1536, wifi_core::ieee80211::RATE_11M);
    let mut cck11 = o == [0x6E, 0x84, 0x5E, 0x04, 0, 0];
    plcp(&mut o, 64, wifi_core::ieee80211::RATE_11M);
    cck11 &= o == [0x6E, 0x04, 0x2F, 0x00, 0, 0];
    // OFDM 24: rate code 0x9 | 1536 << 5 = 0xC009.
    plcp(&mut o, 1536, wifi_core::ieee80211::RATE_24M);
    let ofdm24 = o == [0x09, 0xC0, 0x00, 0x00, 0, 0];
    // The header: a 60-octet unicast ToDS frame at OFDM 24 (phy_rate 0x09, phy_ctl 0x00C1, phy_ctl1 0x1002, fallback CCK 1:
    // phy_ctl1_fb 0x0002, plcp_fb 0A 04 00 02, extra_ft 0), at CCK 11 (0x6E, 0x00C0, 0x0002) and at CCK 1 (today's bytes).
    let mut f = alloc::vec![0u8; 60];
    f[0] = 0x08;
    f[1] = 0x01;
    f[4..10].copy_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    let h = txhdr(&f, 11, COOKIE_RING1, TxRate::long(wifi_core::ieee80211::RATE_24M));
    let g = txhdr(&f, 11, COOKIE_RING1, TxRate::long(wifi_core::ieee80211::RATE_11M));
    let c = txhdr(&f, 11, COOKIE_RING1, TxRate::long(RATE_1M));
    let hdr = h[TXH_PHY_RATE] == 0x09 && h[TXH_PHY_CTL..TXH_PHY_CTL + 2] == [0xC1, 0x00]
        && h[TXH_PHY_CTL1..TXH_PHY_CTL1 + 2] == [0x02, 0x10] && h[TXH_PHY_CTL1_FB..TXH_PHY_CTL1_FB + 2] == [0x02, 0x00]
        && h[TXH_EXTRA_FT] == TXH_EFT_FB_CCK && h[TXH_PLCP_FB..TXH_PLCP_FB + 6] == [0x0A, 0x04, 0x00, 0x02, 0, 0]
        && h[TXH_PLCP_598..TXH_PLCP_598 + 6] == [0x09, 0x08, 0, 0, 0, 0]
        && g[TXH_PHY_RATE] == 0x6E && g[TXH_PHY_CTL..TXH_PHY_CTL + 2] == [0xC0, 0x00] && g[TXH_PHY_CTL1..TXH_PHY_CTL1 + 2] == [0x02, 0x00]
        && c[TXH_PHY_RATE] == 0x0A && c[TXH_PHY_CTL..TXH_PHY_CTL + 2] == [0xC0, 0x00] && c[TXH_PLCP_598..TXH_PLCP_598 + 6] == [0x0A, 0x04, 0x00, 0x02, 0, 0];
    [cck1, cck11, ofdm24, hdr]
}

// ── STATIONAIR (rmbp-ledger B862): the station's air — one TxRate, the NAV, the short preamble ────────────────────

/// The rate a frame leaves at (500 kbit/s units), the rate the AP answers it with (`assoc::ack_rate`, the control response
/// rate) and whether the primary rate uses the short preamble (`assoc::station_short`). The rungs pass `long(rate)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct TxRate {
    pub rate: u8,
    pub ack: u8,
    pub short: bool,
}

impl TxRate {
    /// `rate`, answered at `rate`, long preamble.
    pub(super) const fn long(rate: u8) -> TxRate {
        TxRate { rate, ack: rate, short: false }
    }
}

/// The short-preamble bit applies: asked for, and the rate is CCK 2/5.5/11 (never CCK 1, never OFDM, never an unknown rate).
fn short_ok(r: TxRate) -> bool {
    r.short && matches!(ratecode(r.rate), Some((_, false))) && r.rate != RATE_1M
}

/// The ACK's airtime (14 octets, 112 bits) at `ack` (500 kbit/s units): CCK = the PLCP preamble + header (192 us long, 96 us
/// short; CCK 1 is always long) + ceil(112 / Mbit/s) = ceil(224 / ack); OFDM (ERP) = 16 us preamble + 4 us SIGNAL + 4 us per
/// symbol over 16 + 112 + 6 bits at 2 * ack data bits per symbol + the 6 us signal extension. An unknown rate counts as CCK 1.
fn ack_us(ack: u8, short: bool) -> u16 {
    match ratecode(ack) {
        Some((_, true)) => {
            let ndbps = 2 * ack as u16;
            16 + 4 + 4 * ((134 + ndbps - 1) / ndbps) + 6
        }
        Some((_, false)) => {
            let pre = if short && ack != RATE_1M { 96 } else { 192 };
            pre + (224 + ack as u16 - 1) / ack as u16
        }
        None => 192 + 112,
    }
}

/// The NAV an individually addressed frame carries: SIFS + the ACK at `r.ack` with `r`'s preamble.
pub(super) fn nav_us(r: TxRate) -> u16 {
    SIFS_US + ack_us(r.ack, r.short)
}

/// IEEE Std 802.11-2020 §9.2.5 (Duration/ID) for a non-fragmented frame: `None` unless a Data-type frame (FC type 2) of at
/// least 24 octets whose Duration is 0 (the field is then left as the codec built it); 0 for a group addr1; else `nav_us`.
pub(super) fn duration(frame: &[u8], r: TxRate) -> Option<u16> {
    if frame.len() < 24 || (frame[0] >> 2) & 0x3 != 0x2 || frame[2] != 0 || frame[3] != 0 {
        return None;
    }
    if frame[4] & 0x01 != 0 {
        return Some(0);
    }
    Some(nav_us(r))
}

/// `tests wifi` (through `join::station_kat`): [duration, shortpre], hand-computed. A 60-octet unicast ToDS data frame:
/// CCK 11 long 10+192+11 = 213, short 10+96+11 = 117, answered at CCK 2 10+192+56 = 258, CCK 5.5 10+192+21 = 223, CCK 1
/// 10+192+112 = 314, OFDM 24 10+16+4+8+6 = 44, OFDM 6 10+16+4+24+6 = 60; a group addr1 0; a management frame and a frame
/// already carrying a Duration untouched; dur_fb 314 (0x013A) at CCK 11 and at CCK 1. phy_ctl: CCK 11 short 0x00D0, CCK 1
/// asked short 0x00C0, OFDM 24 asked short 0x00C1; mac_ctl's fallback-preamble bits (top three) stay clear.
pub(super) fn air_kat() -> [bool; 2] {
    use wifi_core::ieee80211::{RATE_11M, RATE_24M};
    let mut f = alloc::vec![0u8; 60];
    f[0] = 0x08;
    f[1] = 0x01;
    f[4..10].copy_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    let t = |rate: u8, ack: u8, short: bool| TxRate { rate, ack, short };
    let mut grp = f.clone();
    grp[4..10].copy_from_slice(&[0xFF; 6]);
    let mut mgmt = f.clone();
    mgmt[0] = 0x00;
    let mut set = f.clone();
    set[2] = 0x3A;
    set[3] = 0x01;
    let h11 = txhdr(&f, 11, COOKIE_RING1, TxRate::long(RATE_11M));
    let h1 = txhdr(&f, 11, COOKIE_RING1, TxRate::long(RATE_1M));
    let dur = duration(&f, TxRate::long(RATE_11M)) == Some(213)
        && duration(&f, t(RATE_11M, RATE_11M, true)) == Some(117)
        && duration(&f, t(RATE_11M, 4, false)) == Some(258)
        && duration(&f, TxRate::long(11)) == Some(223)
        && duration(&f, TxRate::long(RATE_1M)) == Some(314)
        && duration(&f, TxRate::long(RATE_24M)) == Some(44)
        && duration(&f, TxRate::long(12)) == Some(60)
        && duration(&grp, TxRate::long(RATE_11M)) == Some(0)
        && duration(&mgmt, TxRate::long(RATE_11M)).is_none()
        && duration(&set, TxRate::long(RATE_11M)).is_none()
        && h11[TXH_DUR_FB..TXH_DUR_FB + 2] == [0x3A, 0x01] && h1[TXH_DUR_FB..TXH_DUR_FB + 2] == [0x3A, 0x01];
    let s11 = txhdr(&f, 11, COOKIE_RING1, t(RATE_11M, RATE_11M, true));
    let s1 = txhdr(&f, 11, COOKIE_RING1, t(RATE_1M, RATE_1M, true));
    let s24 = txhdr(&f, 11, COOKIE_RING1, t(RATE_24M, RATE_24M, true));
    let short = s11[TXH_PHY_CTL..TXH_PHY_CTL + 2] == [0xD0, 0x00] && s11[TXH_MAC_CTL + 3] & 0xE0 == 0
        && s11[TXH_PLCP_FB..TXH_PLCP_FB + 6] == h11[TXH_PLCP_FB..TXH_PLCP_FB + 6]
        && s1[TXH_PHY_CTL..TXH_PHY_CTL + 2] == [0xC0, 0x00] && s24[TXH_PHY_CTL..TXH_PHY_CTL + 2] == [0xC1, 0x00];
    [dur, short]
}
