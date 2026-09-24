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

//! USBNET (LEDGER SO56) — a USB Ethernet LINK on the xHCI bus, and its first front-end, the USB CDC
//! Ethernet Control Model (ECM) class driver.
//!
//! WHY IT EXISTS. rmbp-ledger B8 asked for a wired NIC on the rMBP and named the wrong part: the only
//! class-0x02 PCI device on that board is the Broadcom Wi-Fi (`drivers/bcma.rs`'s arc), and the board
//! has no wired port at all. What every board CAN carry is a USB Ethernet dongle — Peter's two are
//! identical ASIX AX88179B — so the honest "wired networking" job is ONE driver on the shared xHCI
//! path that serves the rMBP, the Pi and the Orin alike. This module is the link half of that job,
//! written so a front-end is a small set of vendor calls (see §front-ends).
//!
//! WHAT IT IS, in the shape `ftdi.rs` established: the xHCI controller owns the bulk endpoints and
//! the transfer rings; THIS module owns the device's identity, its bring-up state, two frame rings
//! (RX: device → stack, TX: stack → device) and the one-outstanding bulk-IN arm/claim protocol
//! (`ftdi::ftdirx`'s exact shape, because the same event-ring dispatch claims both). The controller
//! calls in from three places: the descriptor walk (candidate detection), the Configure-Endpoint
//! completion (`configured`), and the per-pass service (`Controller::service_usbnet`, file-tail
//! `impl` in mod.rs), which does the class bring-up once and then moves frames both ways.
//!
//! HOW IT REACHES THE STACK. x86: the smoltcp `Device` in `smolnet.rs` reaches its NIC through
//! `e1000::raw_rx` / `raw_tx` / `hw_addr`; each of those falls back to THIS module when no e1000 is
//! present (same-line, cfg-gated), so every smolnet verb and fixture — DHCP, ping, arp, dns, the
//! ring-3 socket family — runs over the dongle with no stack edit. aarch64: `NicOps` registered into
//! `net_phy::net6` the moment the link is up, exactly as virtio-net and the RTL8168 register.
//!
//! CDC-ECM, the parts used (USB CDC 1.2 + the ECM subclass specification; class codes and requests are
//! the specification's, nothing here is vendor knowledge):
//!   * Communications Interface: bInterfaceClass 0x02, bInterfaceSubClass 0x06 (ECM). It carries the
//!     class-specific functional descriptors (bDescriptorType 0x24 CS_INTERFACE); the Ethernet
//!     Networking Functional Descriptor (bDescriptorSubtype 0x0F) names `iMACAddress`, the STRING
//!     descriptor index of the station address as 12 UTF-16LE hex digits, and `wMaxSegmentSize`.
//!   * Data Interface: bInterfaceClass 0x0A, alternate setting 0 with NO endpoints, alternate
//!     setting 1 with the bulk IN/OUT pair — so the bring-up must SET_INTERFACE(alt 1) or the pipes
//!     stay closed. One Ethernet frame per bulk transfer, no framing header; a frame whose length is
//!     a multiple of the OUT max packet size is terminated by a zero-length packet (the OUT stage
//!     below chains a 0-length TRB onto such a frame so the TD ends with a short packet).
//!   * SetEthernetPacketFilter: class request bRequest 0x43, bmRequestType 0x21, wValue = filter bits
//!     (0x01 PROMISCUOUS, 0x02 ALL_MULTICAST, 0x04 DIRECTED, 0x08 BROADCAST), wIndex = the
//!     Communications interface. Optional: a device that STALLs it still receives directed + broadcast.
//!   * A device may present the RNDIS configuration FIRST (QEMU's `usb-net` does: bNumConfigurations
//!     2, RNDIS as configuration index 0, ECM as index 1). The walk therefore records what it saw, and
//!     when configuration 0 held no ECM pair and the device declares another configuration, the
//!     controller re-requests configuration index 1 with the FULL length (`usbnet_request_config`).
//!
//! §front-ends. The AX88179 (Peter's dongles) is NOT ECM: it is a vendor-specific interface with its
//! own register set behind vendor control requests and a per-transfer RX header. Its front-end is the
//! owed rung (QUEUE.md USBNET row): the same rings, the same arm/claim, the same `NicOps`, plus
//! ~six vendor calls at bring-up and a header strip on RX. Nothing in this file is ECM-only except
//! `bringup`'s request list and `is_candidate`'s class match, both of which are named as such.
//!
//! WIRE. One-shot: `:: USBNET: ecm candidate slot=N cfg=V ctrl=I data=J alt=A imac=S mss=M ::` at the
//! walk, `:: USBNET: up slot=N cfg=V ctrl=I data=J alt=A mac=xx:xx:xx:xx:xx:xx filter=ok|refused
//! -> PASS ::` at bring-up (`-> FAIL` names the request that refused), and a log-scale rollup
//! `:: USBNET: rx=N tx=N rx_drop=N tx_drop=N errors=N ::`. Byte identity: this file is a
//! `#[cfg(feature = "usbnet")]` module and every in-file site in mod.rs / e1000.rs is a same-line
//! cfg-gated append or a `#[inline(always)] false` helper, so the knob-off image is unchanged.

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

/// One Ethernet frame with room for a VLAN tag — `net_phy::FRAME_CAP`'s value, restated here so this
/// file compiles on a build that has no smoltcp seam at all.
pub const FRAME_CAP: usize = 1536;
/// Frames each ring holds. Eight is a burst of ARP + DHCP + the first pings; the rings are polled
/// every device-service pass (milliseconds), so a deeper ring would only hide a stalled pass.
const RING: usize = 8;
/// Bulk-IN transfer size. Larger than any frame the device may send (1514 + a possible VLAN tag),
/// so every frame completes as ONE short packet (completion code 13) and never spans two TRBs.
pub const RX_CHUNK: usize = 2048;
/// Where the RX and TX bounce buffers live inside the slot's `scsi_data_buffer` (32 KiB, allocated
/// by `configure_endpoints` for every bulk device, never used by BOT on a slot that runs no SCSI).
/// RX at +0, TX at +4096: disjoint, each 2 KiB, both 64-byte aligned.
pub const RX_BUF_OFFSET: usize = 0;
pub const TX_BUF_OFFSET: usize = 4096;

// ── The link's identity and state ────────────────────────────────────────────────────────────────
const ST_ABSENT: u8 = 0;
const ST_CANDIDATE: u8 = 1; // the walk saw an ECM control+data pair; endpoints not yet configured
const ST_CONFIGURED: u8 = 2; // Configure-Endpoint completed; class bring-up pending
const ST_UP: u8 = 3; // bring-up done; frames move
const ST_FAILED: u8 = 4; // bring-up refused; stays down, logged once

static STATE: AtomicU8 = AtomicU8::new(ST_ABSENT);
static SLOT: AtomicU8 = AtomicU8::new(0);
static NUM_CONFIGS: AtomicU8 = AtomicU8::new(0);
static CFG_INDEX: AtomicU8 = AtomicU8::new(0); // which configuration index the current walk describes
static CFG_VALUE: AtomicU8 = AtomicU8::new(0); // bConfigurationValue of the configuration holding ECM
static CTRL_SEEN: AtomicBool = AtomicBool::new(false); // an ECM Communications interface in this walk
static CTRL_IFACE: AtomicU8 = AtomicU8::new(0);
static DATA_IFACE: AtomicU8 = AtomicU8::new(0);
static DATA_ALT: AtomicU8 = AtomicU8::new(0);
static IMAC_IDX: AtomicU8 = AtomicU8::new(0);
static MSS: AtomicU16 = AtomicU16::new(0);
static RNDIS_SEEN: AtomicBool = AtomicBool::new(false);
static MAC: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];
static FILTER_OK: AtomicBool = AtomicBool::new(false);
static IN_MPS: AtomicU16 = AtomicU16::new(0);
static OUT_MPS: AtomicU16 = AtomicU16::new(0);

// ── The one-outstanding bulk-IN protocol (ftdirx's shape) ──────────────────────────────────────
static ARMED: AtomicBool = AtomicBool::new(false);
static TRB_PHYS: AtomicU64 = AtomicU64::new(0);
static DCI: AtomicU8 = AtomicU8::new(0);
static DONE: AtomicBool = AtomicBool::new(false);
static CODE: AtomicU8 = AtomicU8::new(0);
static RESIDUE: AtomicU32 = AtomicU32::new(0);

// ── Counters ─────────────────────────────────────────────────────────────────────────────────────
static RX_FRAMES: AtomicU64 = AtomicU64::new(0);
static TX_FRAMES: AtomicU64 = AtomicU64::new(0);
static RX_DROP: AtomicU64 = AtomicU64::new(0);
static TX_DROP: AtomicU64 = AtomicU64::new(0);
static ERRORS: AtomicU64 = AtomicU64::new(0);
static REPORTED: AtomicU64 = AtomicU64::new(0);

/// A fixed ring of frames. `w == r` empty; one slot is sacrificed so full is `w - r == RING - 1`.
pub struct FrameRing {
    buf: [[u8; FRAME_CAP]; RING],
    len: [u16; RING],
    r: usize,
    w: usize,
}

impl FrameRing {
    const fn new() -> Self {
        FrameRing { buf: [[0; FRAME_CAP]; RING], len: [0; RING], r: 0, w: 0 }
    }
    fn push(&mut self, frame: &[u8]) -> bool {
        if frame.is_empty() || frame.len() > FRAME_CAP {
            return false;
        }
        let next = (self.w + 1) % RING;
        if next == self.r {
            return false;
        }
        self.buf[self.w][..frame.len()].copy_from_slice(frame);
        self.len[self.w] = frame.len() as u16;
        self.w = next;
        true
    }
    fn pop(&mut self, out: &mut [u8]) -> Option<usize> {
        if self.r == self.w {
            return None;
        }
        let n = (self.len[self.r] as usize).min(out.len());
        out[..n].copy_from_slice(&self.buf[self.r][..n]);
        self.r = (self.r + 1) % RING;
        Some(n)
    }
    fn is_empty(&self) -> bool {
        self.r == self.w
    }
}

static RXQ: Mutex<FrameRing> = Mutex::new(FrameRing::new());
static TXQ: Mutex<FrameRing> = Mutex::new(FrameRing::new());

// ── Descriptor-walk hooks (called from the controller's enumeration, same-line, cfg-gated) ───────

/// The device descriptor arrived for `slot`. Remembers `bNumConfigurations` (byte 17) so a walk that
/// finds no ECM in configuration 0 knows whether there is a configuration 1 to ask for. Only a device
/// that is not already the link is considered (one dongle per boot in this rung, like the FTDI).
pub fn note_device(slot: u8, dev_class: u8, num_configs: u8) {
    if STATE.load(Ordering::Relaxed) >= ST_CONFIGURED {
        return;
    }
    let _ = dev_class;
    SLOT.store(slot, Ordering::Relaxed);
    NUM_CONFIGS.store(num_configs, Ordering::Relaxed);
    CFG_INDEX.store(0, Ordering::Relaxed);
    CTRL_SEEN.store(false, Ordering::Relaxed);
    RNDIS_SEEN.store(false, Ordering::Relaxed);
    STATE.store(ST_ABSENT, Ordering::Relaxed);
}

/// `true` when a DEVICE-level class should have its configuration descriptor requested for this
/// module's sake: Communications devices (0x02) report their class at the device level and the stock
/// enumerator only walks class 0x00 (composite). Called on the same line as that test.
#[inline]
pub fn device_class_wants_walk(dev_class: u8) -> bool {
    dev_class == 0x02
}

/// The configuration descriptor header arrived: `bConfigurationValue` is byte 5. Resets the
/// per-walk ECM flags — a second walk (configuration index 1) must not inherit the first's.
pub fn note_config_header(slot: u8, cfg_value: u8) {
    if SLOT.load(Ordering::Relaxed) != slot || STATE.load(Ordering::Relaxed) >= ST_CONFIGURED {
        return;
    }
    CFG_VALUE.store(cfg_value, Ordering::Relaxed);
    CTRL_SEEN.store(false, Ordering::Relaxed);
    RNDIS_SEEN.store(false, Ordering::Relaxed);
}

/// An interface descriptor. Returns `true` when this is the ECM DATA interface (class 0x0A) of a
/// configuration whose ECM Communications interface (0x02/0x06) was already seen — the caller then
/// collects the bulk endpoints that follow it, exactly as it does for storage and the FTDI.
pub fn note_interface(slot: u8, iface: u8, alt: u8, class: u8, sub: u8, proto: u8) -> bool {
    if SLOT.load(Ordering::Relaxed) != slot || STATE.load(Ordering::Relaxed) >= ST_CONFIGURED {
        return false;
    }
    if class == 0x02 && sub == 0x06 {
        CTRL_SEEN.store(true, Ordering::Relaxed);
        CTRL_IFACE.store(iface, Ordering::Relaxed);
        return false;
    }
    // RNDIS control interface: 0x02/0x02/0xFF (CDC ACM-shaped) or 0xE0/0x01/0x03 (Wireless, RNDIS).
    if (class == 0x02 && sub == 0x02 && proto == 0xFF) || (class == 0xE0 && sub == 0x01 && proto == 0x03) {
        RNDIS_SEEN.store(true, Ordering::Relaxed);
        return false;
    }
    if class == 0x0A && CTRL_SEEN.load(Ordering::Relaxed) {
        DATA_IFACE.store(iface, Ordering::Relaxed);
        DATA_ALT.store(alt, Ordering::Relaxed);
        STATE.store(ST_CANDIDATE, Ordering::Relaxed);
        return true;
    }
    false
}

/// A class-specific interface descriptor (bDescriptorType 0x24). The Ethernet Networking Functional
/// Descriptor (subtype 0x0F) carries `iMACAddress` at byte 3 and `wMaxSegmentSize` at bytes 6..8.
pub fn note_cs(slot: u8, d: &[u8]) {
    if SLOT.load(Ordering::Relaxed) != slot || d.len() < 8 || d[2] != 0x0F {
        return;
    }
    IMAC_IDX.store(d[3], Ordering::Relaxed);
    MSS.store((d[6] as u16) | ((d[7] as u16) << 8), Ordering::Relaxed);
}

/// The walk of configuration index `CFG_INDEX` ended for `slot`. `true` when it produced an ECM
/// candidate (the caller configures its bulk pair); `false` otherwise, and then `other_config`
/// says whether a second configuration is worth asking for.
pub fn walk_is_candidate(slot: u8) -> bool {
    SLOT.load(Ordering::Relaxed) == slot && STATE.load(Ordering::Relaxed) == ST_CANDIDATE
}

/// After a walk with no ECM: the configuration index to request next, if the device declares one
/// this module has not walked yet. RNDIS-first devices land here once.
pub fn other_config(slot: u8) -> Option<u8> {
    if SLOT.load(Ordering::Relaxed) != slot || STATE.load(Ordering::Relaxed) != ST_ABSENT {
        return None;
    }
    let idx = CFG_INDEX.load(Ordering::Relaxed);
    if idx == 0 && NUM_CONFIGS.load(Ordering::Relaxed) > 1 {
        CFG_INDEX.store(1, Ordering::Relaxed);
        serial_println!(
            ":: USBNET: configuration 0 holds no ECM pair (rndis_seen={}) — requesting configuration index 1 of {} ::",
            RNDIS_SEEN.load(Ordering::Relaxed) as u8, NUM_CONFIGS.load(Ordering::Relaxed)
        );
        return Some(1);
    }
    None
}

/// The controller took the candidate: record the bulk pair and announce it. Called right before
/// Configure-Endpoint is issued.
pub fn taken(slot: u8, in_mps: u16, out_mps: u16) {
    IN_MPS.store(in_mps, Ordering::Relaxed);
    OUT_MPS.store(out_mps, Ordering::Relaxed);
    serial_println!(
        ":: USBNET: ecm candidate slot={} cfg={} ctrl={} data={} alt={} imac={} mss={} in_mps={} out_mps={} ::",
        slot, CFG_VALUE.load(Ordering::Relaxed), CTRL_IFACE.load(Ordering::Relaxed),
        DATA_IFACE.load(Ordering::Relaxed), DATA_ALT.load(Ordering::Relaxed), IMAC_IDX.load(Ordering::Relaxed),
        MSS.load(Ordering::Relaxed), in_mps, out_mps
    );
}

/// Configure-Endpoint completed for the candidate slot: class bring-up is now pending.
pub fn configured(slot: u8) {
    if SLOT.load(Ordering::Relaxed) == slot && STATE.load(Ordering::Relaxed) == ST_CANDIDATE {
        STATE.store(ST_CONFIGURED, Ordering::Relaxed);
    }
}

/// Bring-up state for the controller's service pass.
pub fn bringup_pending() -> Option<u8> {
    if STATE.load(Ordering::Relaxed) == ST_CONFIGURED { Some(SLOT.load(Ordering::Relaxed)) } else { None }
}
pub fn cfg_value() -> u8 { CFG_VALUE.load(Ordering::Relaxed) }
pub fn ctrl_iface() -> u8 { CTRL_IFACE.load(Ordering::Relaxed) }
pub fn data_iface() -> u8 { DATA_IFACE.load(Ordering::Relaxed) }
pub fn data_alt() -> u8 { DATA_ALT.load(Ordering::Relaxed) }
pub fn imac_index() -> u8 { IMAC_IDX.load(Ordering::Relaxed) }
pub fn out_mps() -> u16 { OUT_MPS.load(Ordering::Relaxed) }

/// The station address, parsed from the STRING descriptor `iMACAddress` names: `bLength, 0x03,`
/// then 12 UTF-16LE hex digits. Returns `false` (and leaves MAC zero) on any other shape.
pub fn set_mac_from_string_descriptor(d: &[u8]) -> bool {
    if d.len() < 26 || d[1] != 0x03 || d[0] < 26 {
        return false;
    }
    let mut mac = [0u8; 6];
    for i in 0..6 {
        let hi = hex(d[2 + i * 4]);
        let lo = hex(d[4 + i * 4]);
        let (Some(h), Some(l)) = (hi, lo) else { return false };
        mac[i] = (h << 4) | l;
    }
    for i in 0..6 {
        MAC[i].store(mac[i], Ordering::Relaxed);
    }
    true
}
fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Bring-up finished. `failed_at` names the request that refused, or `None` on success.
pub fn set_up(slot: u8, filter_ok: bool, failed_at: Option<&'static str>) {
    FILTER_OK.store(filter_ok, Ordering::Relaxed);
    let m = mac();
    match failed_at {
        None => {
            STATE.store(ST_UP, Ordering::Relaxed);
            serial_println!(
                ":: USBNET: up slot={} cfg={} ctrl={} data={} alt={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} filter={} -> PASS ::",
                slot, CFG_VALUE.load(Ordering::Relaxed), CTRL_IFACE.load(Ordering::Relaxed),
                DATA_IFACE.load(Ordering::Relaxed), DATA_ALT.load(Ordering::Relaxed),
                m[0], m[1], m[2], m[3], m[4], m[5], if filter_ok { "ok" } else { "refused" }
            );
            #[cfg(all(target_arch = "aarch64", feature = "net6"))]
            crate::net_phy::net6::register_nic(&NET6_OPS);
        }
        Some(why) => {
            STATE.store(ST_FAILED, Ordering::Relaxed);
            serial_println!(":: USBNET: bring-up slot={} refused at {} -> FAIL ::", slot, why);
        }
    }
}

/// The slot went away (disconnect or controller reset): everything back to absent, rings emptied.
pub fn disconnect(slot: u8) {
    if SLOT.load(Ordering::Relaxed) != slot || STATE.load(Ordering::Relaxed) == ST_ABSENT {
        return;
    }
    let was_up = STATE.load(Ordering::Relaxed) == ST_UP;
    STATE.store(ST_ABSENT, Ordering::Relaxed);
    SLOT.store(0, Ordering::Relaxed);
    reset_arm();
    for m in MAC.iter() {
        m.store(0, Ordering::Relaxed);
    }
    {
        let mut q = RXQ.lock();
        q.r = 0;
        q.w = 0;
    }
    {
        let mut q = TXQ.lock();
        q.r = 0;
        q.w = 0;
    }
    if was_up {
        serial_println!(":: USBNET: link down slot={} (disconnect) — rx={} tx={} ::", slot,
            RX_FRAMES.load(Ordering::Relaxed), TX_FRAMES.load(Ordering::Relaxed));
    }
}

// ── The bulk-IN arm/claim protocol ───────────────────────────────────────────────────────────────
#[inline]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}
pub fn arm(dci: u8, trb_phys: u64) {
    DCI.store(dci, Ordering::Relaxed);
    TRB_PHYS.store(trb_phys, Ordering::Relaxed);
    DONE.store(false, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
}
fn reset_arm() {
    ARMED.store(false, Ordering::Relaxed);
    DONE.store(false, Ordering::Relaxed);
    TRB_PHYS.store(0, Ordering::Relaxed);
    DCI.store(0, Ordering::Relaxed);
}
/// Event-ring hook: claims the bulk-IN completion of the armed TRB (or any error on that endpoint).
pub fn claim(slot_id: u8, endpoint_id: u8, param: u64, code: u8, transfer_len: u32) -> bool {
    if !ARMED.load(Ordering::Relaxed)
        || SLOT.load(Ordering::Relaxed) != slot_id
        || DCI.load(Ordering::Relaxed) != endpoint_id
    {
        return false;
    }
    let is_error = code != 1 && code != 13;
    if param != TRB_PHYS.load(Ordering::Relaxed) && !is_error {
        return false;
    }
    if !DONE.swap(true, Ordering::Relaxed) {
        CODE.store(code, Ordering::Relaxed);
        RESIDUE.store(transfer_len, Ordering::Relaxed);
    }
    true
}
pub fn take_done() -> Option<(u8, u32)> {
    if !DONE.load(Ordering::Relaxed) {
        return None;
    }
    DONE.store(false, Ordering::Relaxed);
    ARMED.store(false, Ordering::Relaxed);
    Some((CODE.load(Ordering::Relaxed), RESIDUE.load(Ordering::Relaxed)))
}
pub fn note_error(code: u8) {
    let n = ERRORS.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        serial_println!(":: USBNET: IN completion code={} errors={} — re-arming ::", code, n);
    }
}

// ── Frames ───────────────────────────────────────────────────────────────────────────────────────
/// A received frame from the controller's service pass into the RX ring (drops when full: the
/// stack polls every pass, so a full ring means the stack is not being polled, not a fast link).
pub fn deliver(frame: &[u8]) {
    if frame.is_empty() {
        return; // the ZLP that terminates an MPS-multiple frame completes a whole TRB with 0 bytes
    }
    if RXQ.lock().push(frame) {
        RX_FRAMES.fetch_add(1, Ordering::Relaxed);
    } else {
        RX_DROP.fetch_add(1, Ordering::Relaxed);
    }
    rollup();
}
/// The controller's service pass takes the next frame to send, if any.
pub fn next_tx(out: &mut [u8]) -> Option<usize> {
    TXQ.lock().pop(out)
}
pub fn tx_pending() -> bool {
    !TXQ.lock().is_empty()
}
pub fn note_tx_done() {
    TX_FRAMES.fetch_add(1, Ordering::Relaxed);
    rollup();
}

// ── The stack-facing accessors (the e1000 fallbacks on x86, the NicOps on aarch64) ───────────────
pub fn is_up() -> bool {
    STATE.load(Ordering::Relaxed) == ST_UP
}
/// The link's slot while it is up, else 0 — the controller asks rather than keeping a second copy.
pub fn slot() -> u8 {
    if is_up() { SLOT.load(Ordering::Relaxed) } else { 0 }
}
pub fn mac() -> [u8; 6] {
    let mut m = [0u8; 6];
    for i in 0..6 {
        m[i] = MAC[i].load(Ordering::Relaxed);
    }
    m
}
/// Pop one frame for the stack. `None` when the link is down or the ring is empty.
///
/// SYNCHRONOUS DRIVE, and why (measured on the first QEMU run): smolnet's pumps — `dhcp_acquire`,
/// the ping/ARP `pump`, the ring-3 fixtures — spin on `raw_rx` for hundreds of thousands of polls
/// with the STACK lock held, on the assumption the e1000 makes: the NIC's RX ring is READABLE from
/// the caller's context. A USB link's frames only move on the xHCI device-service pass, which runs
/// on the main loop and could not run while a pump spun; the run showed `SOCK-5 … no offer`,
/// `SOCK-1 … 0/4` and `tx_drop=45322` beside a link that was up and moving frames. So when the RX
/// ring is empty this accessor takes the controller LOAN itself (a try-claim: `Busy` when the main
/// loop holds it, and then the pass is about to run anyway) and moves frames both ways before
/// answering. The loan is the controller's one mutual exclusion, so this is the same discipline the
/// main loop follows; no lock of this module is held across the claim.
pub fn raw_rx(out: &mut [u8]) -> Option<usize> {
    if !is_up() {
        return None;
    }
    if let Some(n) = RXQ.lock().pop(out) {
        return Some(n);
    }
    drive();
    RXQ.lock().pop(out)
}
/// Queue one frame and, when the controller loan is free, send it now (see `raw_rx`). Dropped
/// (counted) when the link is down or the ring is full — smoltcp retransmits, the count is on the
/// rollup line.
pub fn raw_tx(frame: &[u8]) {
    if !is_up() || !TXQ.lock().push(frame) {
        TX_DROP.fetch_add(1, Ordering::Relaxed);
        return;
    }
    drive();
}
/// One controller pass for this link, from the stack's context, when the loan is free.
fn drive() {
    if let Ok(mut x) = crate::drivers::xhci::claim() {
        x.poll_events();
        x.service_usbnet();
    }
}
/// The static "our IP" the hand-rolled ARP/ICMP pump in smolnet needs before DHCP; the smoltcp
/// interface itself takes its address from the lease. QEMU's user network hands guests 10.0.2.15
/// first; a real network overrides it through DHCP on the interface, never through this value.
pub const STATIC_IP: [u8; 4] = [10, 0, 2, 15];
/// The e1000-shaped triple: (MAC, static IP, link up). `None` until the link is up.
pub fn hw_addr() -> Option<([u8; 6], [u8; 4], bool)> {
    if is_up() { Some((mac(), STATIC_IP, true)) } else { None }
}

fn rollup() {
    let total = RX_FRAMES.load(Ordering::Relaxed) + TX_FRAMES.load(Ordering::Relaxed);
    let reported = REPORTED.load(Ordering::Relaxed);
    if total < reported.saturating_mul(2).max(1) {
        return;
    }
    REPORTED.store(total, Ordering::Relaxed);
    serial_println!(
        ":: USBNET: rx={} tx={} rx_drop={} tx_drop={} errors={} ::",
        RX_FRAMES.load(Ordering::Relaxed), TX_FRAMES.load(Ordering::Relaxed),
        RX_DROP.load(Ordering::Relaxed), TX_DROP.load(Ordering::Relaxed), ERRORS.load(Ordering::Relaxed)
    );
}

#[cfg(all(target_arch = "aarch64", feature = "net6"))]
fn net6_mac() -> Option<[u8; 6]> {
    if is_up() { Some(mac()) } else { None }
}
#[cfg(all(target_arch = "aarch64", feature = "net6"))]
static NET6_OPS: crate::net_phy::net6::NicOps = crate::net_phy::net6::NicOps {
    rx: raw_rx,
    tx: raw_tx,
    mac: net6_mac,
    link_up: is_up,
    name: "usbnet-ecm",
};
