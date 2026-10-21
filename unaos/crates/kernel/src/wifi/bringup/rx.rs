// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (the d11's MAC and its 64-bit RX DMA engine; no CODEX §2 handler owns a radio register).
//!
//! WIFI9 (rmbp-ledger B625, R110) — rung S6, the RX chain, RECEIVE-ONLY: the MAC suspended around the
//! PHY re-init (`htphyinit.rs`), then put in station mode with beacon-promiscuous receive, ONE 64-bit
//! RX DMA ring of 64 descriptors x 2 KiB in host memory, the MAC enabled, and every frame the ring
//! delivers in 5 s counted; beacons are decoded for their SSID IE and nothing else.
//!
//! **Provenance (R110, R95 §2).** Register offsets, bits and sequences from Linux b43 (`main.c`, `dma.c`,
//! `dma.h`, `b43.h`, `xmit.h`, `xmit.c`) and two values from Linux bcma (`include/linux/bcma/bcma.h:481-484`
//! the DMA translation, `drivers/bcma/core.c:140-149` which one a PCI host takes), each cited beside its
//! use and in `docs/dev/evidence/wifi9/CITATIONS.md`. No text copied, no function transcribed.
//!
//! **Receive-only, by construction.** No TX DMA ring is programmed (TX engines untouched), no template
//! RAM is written, MACCTL.AP is cleared (`main.c:3111`), and the probe-response max time is set to 1 us
//! so the microcode can never answer a probe (`main.c:4914-4918`). Residual, said on the wire: a running
//! d11 microcode may ACK a frame addressed to the MAC address in its match registers; this rung writes
//! no address (b43's `b43_upload_card_macaddress`, `main.c:4956`, is NOT run).
//!
//! **The index rule, measured not assumed.** b43 writes RXINDEX = ring length at setup (`dma.c:728-729`)
//! and, after consuming up to the hardware's slot, RXINDEX = that slot (`dma.c:1699-1700`) — and relies
//! on the RX-descriptor-underflow interrupt to back it off by one (`dma.c:1665-1682`: "Decrement RX
//! index by one will let the device to see all slots as free again"). This rung has no interrupt, so
//! it applies that back-off on every pass: RXINDEX = the slot before the hardware's.

use super::{
    r16, r32, settle, w32, BAR0_WRAP_OFF, D11_GEN_IRQ_REASON, D11_MACCTL, D11_SHM_CONTROL, D11_SHM_DATA,
    D11_SHM_DATA_UNALIGNED, D11_TSF_HIGH, D11_TSF_LOW, SHM_ROUTE_SHARED, WRAP_IOST,
};
use crate::arch::pci::{read_config_16, write_config_32};
use alloc::string::String;
use alloc::vec::Vec;

// ── MACCTL bits, b43.h:451-475 ──────────────────────────────────────────────────────────────────
const MACCTL_ENABLED: u32 = 0x0000_0001; // :451
const MACCTL_INFRA: u32 = 0x0002_0000; // :460
const MACCTL_AP: u32 = 0x0004_0000; // :461
const MACCTL_BEACPROMISC: u32 = 0x0010_0000; // :463
const MACCTL_KEEP_BADPLCP: u32 = 0x0020_0000; // :464
const MACCTL_KEEP_CTL: u32 = 0x0040_0000; // :466
const MACCTL_KEEP_BAD: u32 = 0x0080_0000; // :467
const MACCTL_PROMISC: u32 = 0x0100_0000; // :468
const MACCTL_HWPS: u32 = 0x0200_0000; // :469
const MACCTL_AWAKE: u32 = 0x0400_0000; // :470
const MACCTL_DISCPMQ: u32 = 0x4000_0000; // :474
/// `b43.h:532`.
const IRQ_MAC_SUSPENDED: u32 = 0x0000_0001;

// ── SHM shared offsets, b43.h ───────────────────────────────────────────────────────────────────
const SHM_SH_UCODESTAT: u16 = 0x0040; // :323
const SHM_SH_UCODESTAT_SLEEP: u16 = 4; // :328
const SHM_SH_PHYVER: u16 = 0x0050; // :244
const SHM_SH_PHYTYPE: u16 = 0x0052; // :245
const SHM_SH_PRMAXTIME: u16 = 0x0074; // :311
const SHM_SH_CHAN: u16 = 0x00A0; // :255
/// `b43.h:425` — `B43_PHYTYPE_HT`.
const PHYTYPE_HT: u16 = 0x07;

// ── The 64-bit DMA engine, controller 0 (b43.h:69, dma.h:113-155) ───────────────────────────────
const DMA64_BASE0: u64 = 0x0200;
const RXCTL: u64 = DMA64_BASE0 + 0x20;
const RXINDEX: u64 = DMA64_BASE0 + 0x24;
const RXRINGLO: u64 = DMA64_BASE0 + 0x28;
const RXRINGHI: u64 = DMA64_BASE0 + 0x2C;
const RXSTATUS: u64 = DMA64_BASE0 + 0x30;
const RXERROR: u64 = DMA64_BASE0 + 0x34;
const RXENABLE: u32 = 0x0000_0001; // dma.h:114
const RXFROFF_SHIFT: u32 = 1; // dma.h:116
const RXPARITYDISABLE: u32 = 0x0000_0800; // dma.h:118
const RXADDREXT_MASK: u32 = 0x0003_0000; // dma.h:119
const RXADDREXT_SHIFT: u32 = 16; // dma.h:120
const RXSTATDPTR: u32 = 0x0000_1FFF; // dma.h:125
const RXSTAT: u32 = 0xF000_0000; // dma.h:126
const DCTL0_DTABLEEND: u32 = 0x1000_0000; // dma.h:149
const DCTL1_BYTECNT: u32 = 0x0000_1FFF; // dma.h:153
const DCTL1_ADDREXT_MASK: u32 = 0x0003_0000; // dma.h:154
const DCTL1_ADDREXT_SHIFT: u32 = 16; // dma.h:155
const DESC_BYTES: usize = 16; // dma.h:143-148
/// `dma.h:166` + `dma.c:409-417` — 8 KiB ring memory, 8 KiB aligned (the BCM4331 copies address bit
/// 0x1000 into RXSTATUS's pointer otherwise).
const RINGMEM: usize = 8192;
/// `dma.h:168` — the FW598 RX frame offset (the uploaded ucode is rev 666 >= 598, `main.c:2699-2700`).
const FRAMEOFFSET: usize = 38;
/// The brief's ring: ONE ring, 64 descriptors of 2 KiB.
const SLOTS: usize = 64;
const BUFSIZE: usize = 2048;
/// `bcma.h:481-484` + `bcma/core.c:140-149` (PCI host, IOST.DMA64 set) — translation in the HIGH word
/// for a 64-bit engine (`dma.c:1033-1047`, `:37-68`).
const DMA_TRANSLATION_MASK: u32 = 0xC000_0000;
const DMA_TRANSLATION_DMA64_CMT: u32 = 0x8000_0000;
/// `bcma_regs.h:37` (also this tree's drivers/bcma.rs:244).
const IOST_DMA64: u32 = 0x0000_1000;

// ── The RX header (xmit.h:239-297, FW598 format) and the frame (xmit.c:664-688) ─────────────────
const RXH_MAC_STATUS: usize = 16; // frame_len(2) pad(2) phy_status0..3 (8) phy_status4/5 (4)
const RXH_CHANNEL: usize = 22; // mac_time(2) after mac_status(4)
const RX_MAC_FCSERR: u32 = 0x0000_0001; // xmit.h:341
const RX_MAC_PADDING: u32 = 0x0000_0004; // xmit.h:339
const RX_CHAN_ID: u16 = 0x07F8; // xmit.h:346
const RX_CHAN_ID_SHIFT: u16 = 3; // xmit.h:347
const PLCP_HDR6: usize = 6; // xmit.h:19-20
const FCS_LEN: usize = 4; // xmit.c:688
/// PCI Local Bus spec: Command register bit 2, Bus Master Enable.
const PCI_CMD_BME: u16 = 0x0004;

const WATCH_US: u64 = 5_000_000;
const MAX_SSIDS: usize = 8;

/// Every device write this module makes, by class.
#[derive(Default, Clone, Copy)]
pub(super) struct Writes {
    pub macctl: u32,
    pub shm: u32,
    pub dma: u32,
    pub irq_ack: u32,
    pub pci_cmd: u32,
    /// WIFI10 (B646): MACFILTER_CONTROL/DATA halfwords (`main.c:657-677`).
    pub macfilter: u32,
    /// WIFI10 (B646): TX descriptor words written into host ring memory (`dma.c:177-211`).
    pub tx_desc: u32,
}

/// # Safety
/// As `super::w32`: the window is mapped and on the d11 core (checked live by `live::handle`).
pub(super) unsafe fn w16(base: u64, off: u64, val: u16) {
    core::ptr::write_volatile((base + off) as *mut u16, val);
}

/// `main.c:479-489` + `:555-569` — a shared-memory halfword write (unaligned offsets through
/// SHM_DATA_UNALIGNED with the word index of `offset >> 2`).
pub(super) fn shm_w16(bar0: u64, off: u16, val: u16, w: &mut Writes) {
    unsafe {
        w32(bar0, D11_SHM_CONTROL, (SHM_ROUTE_SHARED << 16) | ((off as u32) >> 2));
        if off & 3 != 0 { w16(bar0, D11_SHM_DATA_UNALIGNED, val) } else { w16(bar0, D11_SHM_DATA, val) }
    }
    w.shm += 2;
}

pub(super) fn shm_r16(bar0: u64, off: u16, w: &mut Writes) -> u16 {
    unsafe {
        w32(bar0, D11_SHM_CONTROL, (SHM_ROUTE_SHARED << 16) | ((off as u32) >> 2));
        w.shm += 1;
        if off & 3 != 0 { r16(bar0, D11_SHM_DATA_UNALIGNED) } else { r16(bar0, D11_SHM_DATA) }
    }
}

fn macctl_w(bar0: u64, v: u32, w: &mut Writes) {
    unsafe { w32(bar0, D11_MACCTL, v) };
    w.macctl += 1;
}

/// `main.c:1145-1200` (b43 forces awake-on, hwps-off: `:1175-1177`): MACCTL.AWAKE on, HWPS off, then
/// wait (100 x 10 us) for the microcode to leave SLEEP. Returns the last UCODESTAT read.
fn awake(bar0: u64, w: &mut Writes) -> u16 {
    let v = unsafe { r32(bar0, D11_MACCTL) };
    macctl_w(bar0, (v & !MACCTL_HWPS) | MACCTL_AWAKE, w);
    let _ = unsafe { r32(bar0, D11_MACCTL) };
    let mut st = 0;
    for _ in 0..100 {
        st = shm_r16(bar0, SHM_SH_UCODESTAT, w);
        if st != SHM_SH_UCODESTAT_SLEEP {
            break;
        }
        let _ = settle(10);
    }
    st
}

/// `main.c:2987-3017` — suspend the MAC: awake, MACCTL.ENABLED off, then poll GEN_IRQ_REASON for
/// MAC_SUSPENDED (35 x 10 us, then 40 x 1 ms). Printed; `true` = the bit was seen.
pub(super) fn suspend(bar0: u64, w: &mut Writes, tag: &str) -> bool {
    let pre = unsafe { r32(bar0, D11_MACCTL) };
    let st = awake(bar0, w);
    let v = unsafe { r32(bar0, D11_MACCTL) };
    macctl_w(bar0, v & !MACCTL_ENABLED, w);
    let _ = unsafe { r32(bar0, D11_MACCTL) };
    let mut seen = false;
    let mut polls = 0u32;
    for i in 0..75u32 {
        polls += 1;
        if unsafe { r32(bar0, D11_GEN_IRQ_REASON) } & IRQ_MAC_SUSPENDED != 0 {
            seen = true;
            break;
        }
        let _ = settle(if i < 35 { 10 } else { 1000 });
    }
    let post = unsafe { r32(bar0, D11_MACCTL) };
    serial_println!(
        "[{}] mac-suspend macctl pre={:#010x} post={:#010x} ucodestat={} suspended={} polls={} — main.c:2987-3017",
        tag, pre, post, st, seen as u8, polls
    );
    seen
}

/// `phy_common.c:425-432` — the channel cookie, written before the PHY tunes (2.4 GHz, 20 MHz: the
/// channel number alone). Returns the pre-image.
pub(super) fn chan_cookie(bar0: u64, ch: u16, w: &mut Writes) -> u16 {
    let pre = shm_r16(bar0, SHM_SH_CHAN, w);
    shm_w16(bar0, SHM_SH_CHAN, ch, w);
    pre
}

/// What one boot's receive window saw.
#[derive(Default)]
pub(super) struct Seen {
    pub frames: u32,
    pub beacons: u32,
    pub bytes: u64,
    pub fcs_err: u32,
    pub oversize: u32,
    pub empty: u32,
    pub other_chan: u32,
    pub ssids: Vec<String>,
    /// WIFI10 (B646): probe responses decoded (subtype 5), of which addressed to `me`, and their BSSIDs.
    pub probe_resp: u32,
    pub resp_to_us: u32,
    pub bssids: Vec<String>,
    /// The station MAC a probe response must name in addr1 to count as `resp_to_us` (zeros = none).
    pub me: [u8; 6], #[cfg(feature = "wifi10")] pub join: super::join::Heard, #[cfg(feature = "wifi10")] pub data: Vec<Vec<u8>>,
}

/// An SSID as printable text: bytes outside 0x20..0x7e and the list's own `,[]` become `?`; empty =
/// `<hidden>`.
fn ssid_text(b: &[u8]) -> String {
    if b.is_empty() || b.iter().all(|c| *c == 0) {
        return String::from("<hidden>");
    }
    b.iter().map(|&c| if (0x20..0x7f).contains(&c) && c != b',' && c != b'[' && c != b']' { c as char } else { '?' }).collect()
}

/// Copy `n` bytes out of a DMA buffer the device wrote (volatile: the compiler must not cache it).
fn copy_out(p: u64, n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        v.push(unsafe { core::ptr::read_volatile((p + i as u64) as *const u8) });
    }
    v
}

/// `dma.c:177-211` — one 64-bit RX descriptor: ctl0 (table end on the last slot), ctl1 (byte count +
/// address extension), the buffer address with the translation in the high word.
unsafe fn fill(ring: u64, slot: usize, buf: u64, w: &mut Writes) {
    let lo = buf as u32;
    let hi_raw = (buf >> 32) as u32;
    let hi = (hi_raw & !DMA_TRANSLATION_MASK) | DMA_TRANSLATION_DMA64_CMT;
    let ext = (hi_raw & DMA_TRANSLATION_MASK) >> 30;
    let ctl0 = if slot == SLOTS - 1 { DCTL0_DTABLEEND } else { 0 };
    let ctl1 = (BUFSIZE as u32 & DCTL1_BYTECNT) | ((ext << DCTL1_ADDREXT_SHIFT) & DCTL1_ADDREXT_MASK);
    let d = ring + (slot * DESC_BYTES) as u64;
    core::ptr::write_volatile(d as *mut u32, ctl0);
    core::ptr::write_volatile((d + 4) as *mut u32, ctl1);
    core::ptr::write_volatile((d + 8) as *mut u32, lo);
    core::ptr::write_volatile((d + 12) as *mut u32, hi);
    // `dma.c:577-590` — poison: frame_len 0, and the first 8 frame bytes 0xFF.
    core::ptr::write_volatile(buf as *mut u16, 0);
    for i in 0..(PLCP_HDR6 + 2) {
        core::ptr::write_volatile((buf + (FRAMEOFFSET + i) as u64) as *mut u8, 0xFF);
    }
    let _ = w;
}

unsafe fn tsf(bar0: u64) -> u64 {
    let lo = r32(bar0, D11_TSF_LOW);
    let hi = r32(bar0, D11_TSF_HIGH);
    ((hi as u64) << 32) | lo as u64
}

/// `dma.c:439-475` — RXCTL <- 0 and wait (10 x 1 ms) for the engine state to read DISABLED.
fn rx_reset(bar0: u64, w: &mut Writes) -> bool {
    unsafe { w32(bar0, RXCTL, 0) };
    w.dma += 1;
    for _ in 0..10 {
        if unsafe { r32(bar0, RXSTATUS) } & RXSTAT == 0 {
            return true;
        }
        crate::arch::sched::sleep_ms(1);
    }
    false
}

/// One delivered slot (`dma.c:1579-1663` + `xmit.c:635-690`): count it, decode a beacon's SSID.
fn take(buf: u64, s: &mut Seen, ch: u16, raw_rows: &mut u32, tag: &str) -> usize {
    let mut len = unsafe { core::ptr::read_volatile(buf as *const u16) } as usize;
    if len == 0 {
        for _ in 0..5 {
            let _ = settle(2);
            len = unsafe { core::ptr::read_volatile(buf as *const u16) } as usize;
            if len != 0 { break; }
        }
        if len == 0 {
            s.empty += 1;
            return 1;
        }
    }
    if len + FRAMEOFFSET > BUFSIZE {
        // `dma.c:1617-1642`: a frame spilt over several buffers is dropped, slot by slot.
        s.oversize += 1;
        let mut spans = 0usize;
        let mut t = len as isize;
        loop {
            spans += 1;
            t -= BUFSIZE as isize;
            if t <= 0 { break; }
        }
        return spans;
    }
    let hdr = copy_out(buf, 24);
    let macstat = u32::from_le_bytes([hdr[RXH_MAC_STATUS], hdr[RXH_MAC_STATUS + 1], hdr[RXH_MAC_STATUS + 2], hdr[RXH_MAC_STATUS + 3]]);
    let chanstat = u16::from_le_bytes([hdr[RXH_CHANNEL], hdr[RXH_CHANNEL + 1]]);
    let chan = (chanstat & RX_CHAN_ID) >> RX_CHAN_ID_SHIFT;
    s.frames += 1;
    s.bytes += len as u64;
    if chan != ch { s.other_chan += 1; }
    if *raw_rows < 3 {
        // bcm4331.md §S6: the raw RX header beside the decode (a garbage SSID is S5 feedback).
        *raw_rows += 1;
        let mut hx = String::new();
        for b in hdr.iter() { hx.push_str(&alloc::format!("{:02x}", b)); }
        serial_println!("[{}] rx-raw n={} len={} macstat={:#010x} chanstat={:#06x} ch={} hdr={}", tag, raw_rows, len, macstat, chanstat, chan, hx);
    }
    if macstat & RX_MAC_FCSERR != 0 {
        s.fcs_err += 1;
        return 1;
    }
    let pad = if macstat & RX_MAC_PADDING != 0 { 2 } else { 0 };
    let start = pad + PLCP_HDR6;
    if len < start + 24 + FCS_LEN {
        return 1;
    }
    let frame = copy_out(buf + (FRAMEOFFSET + start) as u64, len - start - FCS_LEN); #[cfg(feature = "wifi10")] super::join::hear(&frame, &mut s.join); #[cfg(feature = "wifi10")] super::super::data::keep(&frame, &mut s.data);
    // FC byte 0: type bits 3:2 = 0 (management), subtype bits 7:4 = 8 (beacon) — IEEE 802.11 §9.2.4.1.
    if (frame[0] >> 2) & 0x3 == 0 && (frame[0] >> 4) == 0x8 {
        if let Some(b) = wifi_core::ieee80211::parse_beacon(&frame) {
            s.beacons += 1;
            let t = ssid_text(&b.ssid);
            if s.ssids.len() < MAX_SSIDS && !s.ssids.iter().any(|x| *x == t) {
                s.ssids.push(t);
            }
        }
    } else if (frame[0] >> 2) & 0x3 == 0 && (frame[0] >> 4) == 0x5 {
        // WIFI10 (B646): subtype 5, a probe response — the same fixed fields + IEs as a beacon
        // (IEEE 802.11 §9.3.3.10); `parse_beacon` accepts both. addr1 (bytes 4..10) names the station it answers.
        if let Some(b) = wifi_core::ieee80211::parse_beacon(&frame) {
            s.probe_resp += 1;
            if frame[4..10] == s.me {
                s.resp_to_us += 1;
            }
            let t = ssid_text(&b.ssid);
            if s.ssids.len() < MAX_SSIDS && !s.ssids.iter().any(|x| *x == t) {
                s.ssids.push(t);
            }
            let bs = alloc::format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", b.bssid[0], b.bssid[1], b.bssid[2], b.bssid[3], b.bssid[4], b.bssid[5]);
            if s.bssids.len() < MAX_SSIDS && !s.bssids.iter().any(|x| *x == bs) {
                s.bssids.push(bs);
            }
        }
    }
    1
}

/// The armed RX engine and its poll state. S6 (`run`) and S7's response window (WIFI10, `bringup/tx.rs`
/// via `bringup::wifi10`) share the three halves below.
pub(super) struct Rx {
    ring: u64,
    bufs: u64,
    slot: usize,
    moves: u32,
    raw_rows: u32,
    tsf_delta: u64,
    tag: &'static str,
}

/// S6's setup half. Precondition: the MAC is suspended (`suspend`), the PHY re-initialised and tuned.
/// The engine check, PCI bus mastering, the ring and its buffers, station opmode, the SHM words, the
/// controller, the MAC enabled. `Err` carries the refusal text the caller prints (nothing programmed).
pub(super) fn arm(bar0: u64, bus: u8, dev: u8, func: u8, w: &mut Writes, tag: &'static str) -> Result<Rx, String> {
    // ── The engine type (dma.c:800-811): the wrapper's IOST.DMA64. ──
    let iost = unsafe { r32(bar0, BAR0_WRAP_OFF + WRAP_IOST) };
    if iost & IOST_DMA64 == 0 {
        return Err(alloc::format!("engine-not-dma64 iost={:#010x} — dma.c:800-811; nothing programmed", iost));
    }
    // ── Bus mastering: the d11 DMAs into host memory (PCI Command bit 2). ──
    let cmd = unsafe { read_config_16(bus, dev, func, 0x04) };
    if cmd & PCI_CMD_BME == 0 {
        unsafe { write_config_32(bus, dev, func, 0x04, (cmd | PCI_CMD_BME) as u32) };
        w.pci_cmd += 1;
    }
    let cmd_post = unsafe { read_config_16(bus, dev, func, 0x04) };
    // ── Ring + buffers: host memory, VA == PA on this kernel's x86 identity map (ahci.rs's `bus_addr`). ──
    let ring_l = core::alloc::Layout::from_size_align(RINGMEM, RINGMEM).ok();
    let bufs_l = core::alloc::Layout::from_size_align(SLOTS * BUFSIZE, 4096).ok();
    let (Some(rl), Some(bl)) = (ring_l, bufs_l) else {
        return Err(String::from("layout — nothing programmed"));
    };
    // Deliberately never freed: the engine owns this memory until RXCTL reads DISABLED, and a failed
    // reset below would leave it owning it for the rest of the boot.
    let ring = unsafe { alloc::alloc::alloc_zeroed(rl) } as u64;
    let bufs = unsafe { alloc::alloc::alloc_zeroed(bl) } as u64;
    if ring == 0 || bufs == 0 {
        return Err(String::from("no-memory — nothing programmed"));
    }
    let reset_ok = rx_reset(bar0, w);
    for i in 0..SLOTS {
        unsafe { fill(ring, i, bufs + (i * BUFSIZE) as u64, w) };
    }
    // ── Opmode (main.c:3103-3160): station, not AP; BEACPROMISC (FIF_BCN_PRBRESP_PROMISC, :3131-3132)
    // so beacons of every BSS come up; PROMISC (b43.h:468) for the frame count; KEEP_BAD (FIF_FCSFAIL,
    // :3127-3128) so a PHY that decodes nothing clean shows as fcs_err, not as silence (bcm4331.md §S6). ──
    let ctl0 = unsafe { r32(bar0, D11_MACCTL) };
    let mut ctl = ctl0 & !(MACCTL_AP | MACCTL_KEEP_CTL | MACCTL_KEEP_BADPLCP | MACCTL_KEEP_BAD | MACCTL_PROMISC | MACCTL_BEACPROMISC);
    ctl |= MACCTL_INFRA | MACCTL_BEACPROMISC | MACCTL_PROMISC | MACCTL_KEEP_BAD | MACCTL_DISCPMQ;
    macctl_w(bar0, ctl, w);
    // ── SHM (main.c:4914-4918, :4931-4933): no probe responses ever; the PHY type and revision. ──
    shm_w16(bar0, SHM_SH_PRMAXTIME, 1, w);
    shm_w16(bar0, SHM_SH_PHYTYPE, PHYTYPE_HT, w);
    let phyver = unsafe { r16(bar0, super::D11_PHY_VER) } & 0xF;
    shm_w16(bar0, SHM_SH_PHYVER, phyver, w);
    // ── The ring (dma.c:709-729, 64-bit arm; parity disabled on BCMA, dma.c:1076-1081). ──
    let lo = ring as u32;
    let hi_raw = (ring >> 32) as u32;
    let hi = (hi_raw & !DMA_TRANSLATION_MASK) | DMA_TRANSLATION_DMA64_CMT;
    let ext = (hi_raw & DMA_TRANSLATION_MASK) >> 30;
    let rxctl = ((FRAMEOFFSET as u32) << RXFROFF_SHIFT) | RXENABLE | ((ext << RXADDREXT_SHIFT) & RXADDREXT_MASK) | RXPARITYDISABLE;
    unsafe {
        w32(bar0, RXCTL, rxctl);
        w32(bar0, RXRINGLO, lo);
        w32(bar0, RXRINGHI, hi);
        w32(bar0, RXINDEX, (SLOTS * DESC_BYTES) as u32);
    }
    w.dma += 4;
    let st0 = unsafe { r32(bar0, RXSTATUS) };
    // ── MAC enable (main.c:2958-2983): ENABLED, ack MAC_SUSPENDED, flush, awake. ──
    let v = unsafe { r32(bar0, D11_MACCTL) };
    macctl_w(bar0, v | MACCTL_ENABLED, w);
    unsafe { w32(bar0, D11_GEN_IRQ_REASON, IRQ_MAC_SUSPENDED) };
    w.irq_ack += 1;
    let _ = unsafe { (r32(bar0, D11_MACCTL), r32(bar0, D11_GEN_IRQ_REASON)) };
    let _ = awake(bar0, w);
    let mac_on = unsafe { r32(bar0, D11_MACCTL) };
    serial_println!(
        "[{}] rx-setup ring={:#x} bufs={:#x} slots={} buf={} fo={} rxctl={:#010x} ringhi={:#010x} rxstatus={:#010x} reset={} pci_cmd={:#06x}->{:#06x} macctl={:#010x}->{:#010x} — dma.c:672-750, main.c:3103-3160, :2958-2983",
        tag, ring, bufs, SLOTS, BUFSIZE, FRAMEOFFSET, rxctl, hi, st0, reset_ok as u8, cmd, cmd_post, ctl0, mac_on
    );
    Ok(Rx { ring, bufs, slot: 0, moves: 0, raw_rows: 0, tsf_delta: 0, tag })
}

/// The window: `window_us` by TSF (TSC cap one second longer), polled every 5 ms (a sleep inside the
/// shell task). Every delivered slot is counted into `s` and re-armed.
pub(super) fn poll(bar0: u64, rx: &mut Rx, s: &mut Seen, ch: u16, window_us: u64, w: &mut Writes) {
    let hz = crate::bootpace::origin_hz();
    let cap = if hz >= 1000 { hz * (window_us / 1_000_000 + 1) } else { 0 };
    let c0 = crate::arch::now_cycles();
    let t0 = unsafe { tsf(bar0) };
    let mut t1 = t0;
    while t1.wrapping_sub(t0) < window_us && crate::arch::now_cycles().wrapping_sub(c0) < cap {
        drain(bar0, rx, s, ch, w);
        crate::arch::sched::sleep_ms(5);
        t1 = unsafe { tsf(bar0) };
    }
    rx.tsf_delta += t1.wrapping_sub(t0);
}

/// S6's stop half: the MAC suspended, the engine reset (dma.c:439-475); the memory is leaked on
/// purpose (see `arm`). `(mac_suspended, engine_disabled)`.
pub(super) fn finish(bar0: u64, rx: &Rx, w: &mut Writes) -> (bool, bool) {
    let st1 = unsafe { r32(bar0, RXSTATUS) };
    let err1 = unsafe { r32(bar0, RXERROR) };
    let mac_off = suspend(bar0, w, rx.tag);
    let stopped = rx_reset(bar0, w);
    serial_println!(
        "[{}] rx-window tsf-delta={}us slots-consumed={} rxstatus={:#010x} rxerror={:#010x} stop: mac-suspended={} engine-disabled={} — b43 dma.c:1684-1702 polled",
        rx.tag, rx.tsf_delta, rx.moves, st1, err1, mac_off as u8, stopped as u8
    );
    (mac_off, stopped)
}

/// S6. Precondition: the MAC is suspended (`suspend`), the PHY re-initialised and tuned to `ch`.
pub(super) fn run(bar0: u64, bus: u8, dev: u8, func: u8, ch: u16, w: &mut Writes) -> Seen {
    let mut s = Seen::default();
    let mut rx = match arm(bar0, bus, dev, func, w, "wifi9") {
        Ok(r) => r,
        Err(why) => {
            serial_println!(":: wifi9: rx REFUSED reason={} ::", why);
            return s;
        }
    };
    poll(bar0, &mut rx, &mut s, ch, WATCH_US, w);
    let _ = finish(bar0, &rx, w);
    s
}

/// One pass over the ring, no sleep (`poll`'s body, and WIFIDHCP's kept station from inside smolnet's poll, B831):
/// every slot the hardware filled since the last pass is counted into `s` and re-armed, then RXINDEX is backed off.
pub(super) fn drain(bar0: u64, rx: &mut Rx, s: &mut Seen, ch: u16, w: &mut Writes) {
    let cur = ((unsafe { r32(bar0, RXSTATUS) } & RXSTATDPTR) as usize / DESC_BYTES) % SLOTS; // dma.c:231-239
    while rx.slot != cur {
        rx.moves += 1;
        let n = take(rx.bufs + (rx.slot * BUFSIZE) as u64, s, ch, &mut rx.raw_rows, rx.tag);
        for _ in 0..n {
            unsafe { fill(rx.ring, rx.slot, rx.bufs + (rx.slot * BUFSIZE) as u64, w) };
            rx.slot = (rx.slot + 1) % SLOTS;
            if rx.slot == cur { break; }
        }
    }
    // dma.c:1699-1700 with the dma.c:1679-1681 back-off: RXINDEX = the slot before the hardware's.
    unsafe { w32(bar0, RXINDEX, (((cur + SLOTS - 1) % SLOTS) * DESC_BYTES) as u32) };
    w.dma += 1;
}
