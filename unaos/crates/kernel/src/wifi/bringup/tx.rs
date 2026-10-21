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
use super::{r32, settle, w32, D11_SHM_CONTROL, D11_SHM_DATA};
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
/// `main.c:665` — the offset is OR'd with 0x0020 before the select.
const MACFILTER_SELECT: u16 = 0x0020;

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
const TXH_PHY_ANT01AUTO: u16 = 0x00C0;
const TXH_PHY_TXPWR: u16 = 0xFC00;
/// `xmit.h:167` + `xmit.c:167-181` — PHY control word 1 for a CCK rate on an HT-PHY is the 20 MHz bandwidth alone.
const TXH_PHY1_BW_20: u16 = 0x0002;
/// `xmit.h:132` — extra frame types: the fallback encoding is CCK.
const TXH_EFT_FB_CCK: u8 = 0x00;
/// `xmit.c:96-99` + `b43.h:593` — CCK 1 Mbit/s: PLCP rate code 0x0A, bitrate 2 (units of 500 kbit/s).
const PLCP_RATECODE_CCK_1MB: u8 = 0x0A;
const CCK_RATE_1MB: u32 = 2;
/// `xmit.c:688` — the FCS the hardware appends is in the PLCP length.
const FCS_LEN: usize = 4;

// ── The retry limits (WIFIAUTH B788; b43.h:229, :345-346, :608-609; main.c:3972-3985, called at core init :4909) ──
/// `b43.h:229` — the SHM routing enum's third entry, SCRATCH. Scratch is WORD-addressed (`main.c:555-569`:
/// only the SHARED routing shifts the offset).
const SHM_ROUTE_SCRATCH: u32 = 0x0002;
const SHM_SC_SRLIMIT: u32 = 0x0006;
const SHM_SC_LRLIMIT: u32 = 0x0007;
const RETRY_SHORT: u16 = 7;
const RETRY_LONG: u16 = 4;
/// WIFIDHCP (B831): a frame's header and its body each own one `FRAME_STRIDE` region of `buf` (header region at
/// `2i * FRAME_STRIDE`, body region right after it), so the body cap IS the stride: a 342-octet DHCP DISCOVER is
/// 360 octets of air and fits. `buf` holds `BUF_FRAMES` such pairs (8 KiB).
const FRAME_STRIDE: u64 = 512;
pub(in crate::wifi) const FRAME_MAX: usize = FRAME_STRIDE as usize;
const BUF_FRAMES: usize = 8;
const BUF_BYTES: usize = BUF_FRAMES * 2 * FRAME_STRIDE as usize;
/// Frame numbers on the descriptor ring: two slots each, 32 per lap. Frame `n` uses buffer pair `n % BUF_FRAMES`
/// (free again once `fire_frame` returned: it waits for the frame's status) and slots `2n`/`2n+1`; the index wraps
/// to slot 0 after slot 63, which carries DTABLEEND (`fill`; b43 `dma.c` next_slot).
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

/// `main.c:657-677` — `MACFILTER_SELF` <- the station MAC, three halfwords through the data port.
fn macfilter_self(bar0: u64, mac: [u8; 6], w: &mut Writes) {
    unsafe {
        w16(bar0, MACFILTER_CONTROL, MACFILTER_SELF | MACFILTER_SELECT);
        w16(bar0, MACFILTER_DATA, u16::from_le_bytes([mac[0], mac[1]]));
        w16(bar0, MACFILTER_DATA, u16::from_le_bytes([mac[2], mac[3]]));
        w16(bar0, MACFILTER_DATA, u16::from_le_bytes([mac[4], mac[5]]));
    }
    w.macfilter += 4;
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

/// `xmit.c:136-165` — the CCK PLCP header: rate code, service 0x04, length in microseconds
/// (`octets * 16 / bitrate`, bitrate in 500 kbit/s units; 1 Mbit/s divides evenly so no 0x84 case).
fn plcp_cck(out: &mut [u8], octets: usize) {
    let plen = (octets as u32 * 16) / CCK_RATE_1MB;
    out[0] = PLCP_RATECODE_CCK_1MB;
    out[1] = 0x04;
    out[2] = plen as u8;
    out[3] = (plen >> 8) as u8;
    out[4] = 0;
    out[5] = 0;
}

/// `xmit.c:235-567` for a management frame at CCK 1 Mbit/s, no key, no RTS/CTS, no short preamble:
/// phy_rate, the FC copy, the receiver, dur_fb = the frame's own duration (0: `:279-286`), both PLCPs,
/// extra_ft CCK fallback, chan_radio_code = the channel (`:360-364`), phy_ctl CCK + ANT01AUTO
/// (`:367-392`), mac_ctl HWSEQ + STMSDU + LONGFRAME (`:396-421`) + ACK on a unicast addr1 (`:399-400`), phy_ctl1 and
/// its fallback = BW_20 (`:554-558`, `:167-181`), the cookie (`:541-552`). Everything else stays 0 (`:263`).
fn txhdr(frame: &[u8], ch: u16, cookie: u16) -> [u8; TXH_SIZE] {
    let mut h = [0u8; TXH_SIZE];
    let octets = frame.len() + FCS_LEN;
    let unicast = frame[4] & 0x01 == 0;
    let mac_ctl = TXH_MAC_HWSEQ | TXH_MAC_STMSDU | TXH_MAC_LONGFRAME | if unicast { TXH_MAC_ACK } else { 0 };
    h[TXH_MAC_CTL..TXH_MAC_CTL + 4].copy_from_slice(&mac_ctl.to_le_bytes());
    h[TXH_MAC_FRAME_CTL..TXH_MAC_FRAME_CTL + 2].copy_from_slice(&frame[0..2]);
    let phy_ctl = TXH_PHY_ENC_CCK | TXH_PHY_ANT01AUTO;
    h[TXH_PHY_CTL..TXH_PHY_CTL + 2].copy_from_slice(&phy_ctl.to_le_bytes());
    h[TXH_PHY_CTL1..TXH_PHY_CTL1 + 2].copy_from_slice(&TXH_PHY1_BW_20.to_le_bytes());
    h[TXH_PHY_CTL1_FB..TXH_PHY_CTL1_FB + 2].copy_from_slice(&TXH_PHY1_BW_20.to_le_bytes());
    h[TXH_PHY_RATE] = PLCP_RATECODE_CCK_1MB;
    h[TXH_EXTRA_FT] = TXH_EFT_FB_CCK;
    h[TXH_CHAN_RADIO_CODE] = ch as u8;
    h[TXH_TX_RECEIVER..TXH_TX_RECEIVER + 6].copy_from_slice(&frame[4..10]);
    plcp_cck(&mut h[TXH_PLCP_FB..TXH_PLCP_FB + 6], octets);
    h[TXH_DUR_FB..TXH_DUR_FB + 2].copy_from_slice(&frame[2..4]);
    h[TXH_COOKIE_598..TXH_COOKIE_598 + 2].copy_from_slice(&cookie.to_le_bytes());
    plcp_cck(&mut h[TXH_PLCP_598..TXH_PLCP_598 + 6], octets);
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
    macfilter_self(bar0, mac, w);
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
    fire_frame(bar0, p, &probe_request(mac, ch), ch, 0, "probe-req", w)
}

/// Frame `n` (0..FRAMES) of the ring out. Precondition: `prep` done and the MAC ENABLED (S6's
/// `rx::arm`, which also wakes the ucode). The header on slot 2n (FRAMESTART), the frame on slot 2n+1
/// (FRAMEEND | IRQ), `TXINDEX` <- the slot after (`dma.c:1186-1286`, `:213-217`), the cookie
/// `(controller + 1) << 12 | 2n` (`dma.c:1133-1150`); then the TX-status FIFO is drained for up to
/// `TXSTAT_WAIT_MS` and the entry carrying our cookie decoded (`main.c:1334-1360`).
pub(super) fn fire_frame(bar0: u64, p: &Prep, frame: &[u8], ch: u16, n: usize, kind: &str, w: &mut Writes) -> Report {
    let mut r = Report::default();
    r.frame_len = frame.len();
    if n >= FRAMES || frame.len() > FRAME_MAX || frame.len() < 24 {
        r.cookie = 0;
        serial_println!("[wifi10] tx-poke kind={} REFUSED reason=frame-slot(n={} len={}) — nothing written", kind, n, frame.len());
        return r;
    }
    let slot = n * TX_SLOTS_PER_FRAME;
    let cookie = COOKIE_RING1 | slot as u16;
    let hdr = txhdr(frame, ch, cookie);
    r.cookie = cookie;
    let hbuf = p.buf + (n % BUF_FRAMES) as u64 * 2 * FRAME_STRIDE;
    let fbuf = hbuf + FRAME_STRIDE;
    for (i, b) in hdr.iter().enumerate() {
        unsafe { core::ptr::write_volatile((hbuf + i as u64) as *mut u8, *b) };
    }
    for (i, b) in frame.iter().enumerate() {
        unsafe { core::ptr::write_volatile((fbuf + i as u64) as *mut u8, *b) };
    }
    unsafe {
        fill(p.ring, slot, hbuf, TXH_SIZE, DCTL0_FRAMESTART, w);
        fill(p.ring, slot + 1, fbuf, frame.len(), DCTL0_FRAMEEND | DCTL0_IRQ, w);
    }
    r.dptr_before = unsafe { r32(bar0, TXSTATUS) } & TXSTATDPTR;
    // dma.c:1284-1285: wmb, then poke the slot AFTER the frame's last one (slot 0 after slot 63, the table end).
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    let txindex = ((slot + TX_SLOTS_PER_FRAME) % SLOTS) * DESC_BYTES;
    unsafe { w32(bar0, TXINDEX, txindex as u32) };
    w.dma += 1;
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
                let tmp = v0 & 0xFFFF;
                r.status_seen = true;
                r.cookie_match = true;
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
        // WIFIDHCP (B831): a spin, not a sleep — the kept station sends from inside smolnet's STACK hold, which must
        // never park its holder.
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
