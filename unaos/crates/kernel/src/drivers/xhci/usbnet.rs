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
/// USBNET5 M1: the AX88179 bulk-IN buffer is `1024*(QCTRL[3]+2)` (Linux `rx_urb_size`): SS 20 KiB, HS 24 KiB,
/// FS 26 KiB. The largest is 26624 B; the slot's 32 KiB `scsi_data_buffer` (64 KiB aligned, so one TRB never
/// crosses a 64 KiB boundary) holds RX at +0 and TX at +28672. ECM keeps `RX_CHUNK`.
pub const RX_CHUNK_MAX: usize = 26624;
static RX_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(RX_CHUNK);
/// The bulk-IN transfer size currently posted (ECM 2048; AX88179 per link speed after bring-up).
pub fn rx_len() -> usize { RX_LEN.load(Ordering::Relaxed) }
/// Set the AX bulk-IN size from the QCTRL tuple written (Linux `1024*(q[3]+2)`), clamped to `RX_CHUNK_MAX`.
pub fn set_rx_len_from_qctrl(q: &[u8; 5]) -> usize {
    let n = (1024 * (q[3] as usize + 2)).min(RX_CHUNK_MAX);
    RX_LEN.store(n, Ordering::Relaxed);
    n
}
/// Where the RX and TX bounce buffers live inside the slot's `scsi_data_buffer` (32 KiB, allocated
/// by `configure_endpoints` for every bulk device, never used by BOT on a slot that runs no SCSI).
/// RX at +0 (up to RX_CHUNK_MAX), TX at +28672: disjoint, both 64-byte aligned (USBNET5 moved TX up for the 20-26 KiB AX88179 RX buffer).
pub const RX_BUF_OFFSET: usize = 0;
pub const TX_BUF_OFFSET: usize = 28672;

// ── The link's identity and state ────────────────────────────────────────────────────────────────
const ST_ABSENT: u8 = 0;
const ST_CANDIDATE: u8 = 1; // the walk saw an ECM control+data pair; endpoints not yet configured
const ST_CONFIGURED: u8 = 2; // Configure-Endpoint completed; class bring-up pending
const ST_UP: u8 = 3; // bring-up done; frames move
const ST_FAILED: u8 = 4; // bring-up refused; stays down, logged once

/// Which front-end the candidate is. ECM is the class driver; AX88179 is the vendor front-end for
/// Peter's dongles (`0b95:1790` AX88179B, `0b95:178a` AX88179/178A): a vendor-specific interface
/// (class 0xFF) with the same bulk pair, register access over vendor control requests
/// (bRequest 0x01 = MAC/register space, 0x02 = PHY; bmRequestType 0x40 write / 0xC0 read; wValue =
/// register, wIndex = byte count), a packet TRAILER on every bulk-IN transfer and an 8-byte header on
/// every bulk-OUT frame. The register map and the framing are the part's, as ASIX documents them and
/// as the Linux `ax88179_178a` driver (GPL, this tree's own licence) uses them — see `ax::*`. BUILT
/// FROM THAT MAP, NOT YET FLOWN: QEMU has no model of the part, so every constant below is confirmed
/// on the bench with the dongle (the owed rung of SO56), and the wire says `kind=ax88179` so a
/// reading is never mistaken for ECM's.
pub const KIND_ECM: u8 = 1;
pub const KIND_AX88179: u8 = 2;
static KIND: AtomicU8 = AtomicU8::new(0);
static VID: AtomicU16 = AtomicU16::new(0);
static PID: AtomicU16 = AtomicU16::new(0);
pub fn kind() -> u8 { KIND.load(Ordering::Relaxed) }
pub fn kind_name() -> &'static str {
    match KIND.load(Ordering::Relaxed) { KIND_ECM => "ecm", KIND_AX88179 => "ax88179", _ => "none" }
}
fn is_ax_part(vid: u16, pid: u16) -> bool {
    vid == 0x0b95 && (pid == 0x1790 || pid == 0x178a)
}
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
    /// NETCLOCK: frames queued.
    fn len(&self) -> usize {
        (self.w + RING - self.r) % RING
    }
}

static RXQ: Mutex<FrameRing> = Mutex::new(FrameRing::new());
static TXQ: Mutex<FrameRing> = Mutex::new(FrameRing::new());

// ── Descriptor-walk hooks (called from the controller's enumeration, same-line, cfg-gated) ───────

/// The device descriptor arrived for `slot`. Remembers `bNumConfigurations` (byte 17) so a walk that
/// finds no ECM in configuration 0 knows whether there is a configuration 1 to ask for. Only a device
/// that is not already the link is considered (one dongle per boot in this rung, like the FTDI).
pub fn note_device(slot: u8, dev_class: u8, num_configs: u8, vid: u16, pid: u16) {
    if STATE.load(Ordering::Relaxed) >= ST_CONFIGURED {
        return;
    }
    let _ = dev_class;
    SLOT.store(slot, Ordering::Relaxed);
    VID.store(vid, Ordering::Relaxed);
    PID.store(pid, Ordering::Relaxed);
    KIND.store(0, Ordering::Relaxed);
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
    dev_class == 0x02 || (dev_class == 0xFF && is_ax_part(VID.load(Ordering::Relaxed), PID.load(Ordering::Relaxed)))
}

/// The configuration descriptor header arrived: `bConfigurationValue` is byte 5. Resets the
/// per-walk ECM flags — a second walk (configuration index 1) must not inherit the first's.
pub fn note_config_header(slot: u8, cfg_value: u8) {
    if SLOT.load(Ordering::Relaxed) != slot || STATE.load(Ordering::Relaxed) >= ST_CONFIGURED {
        return;
    }
    CFG_VALUE.store(cfg_value, Ordering::Relaxed);
    CTRL_SEEN.store(false, Ordering::Relaxed);
    FIRST_IN.store(0, Ordering::Relaxed); FIRST_OUT.store(0, Ordering::Relaxed); // USBNET7: the first-bulk memory is per walk
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
    if class == 0xFF && is_ax_part(VID.load(Ordering::Relaxed), PID.load(Ordering::Relaxed)) {
        // The AX88179's one interface: vendor-specific, bulk IN + bulk OUT + interrupt IN, alt 0.
        KIND.store(KIND_AX88179, Ordering::Relaxed);
        CTRL_IFACE.store(iface, Ordering::Relaxed);
        DATA_IFACE.store(iface, Ordering::Relaxed);
        DATA_ALT.store(alt, Ordering::Relaxed);
        STATE.store(ST_CANDIDATE, Ordering::Relaxed);
        return true;
    }
    // RNDIS control interface: 0x02/0x02/0xFF (CDC ACM-shaped) or 0xE0/0x01/0x03 (Wireless, RNDIS).
    if (class == 0x02 && sub == 0x02 && proto == 0xFF) || (class == 0xE0 && sub == 0x01 && proto == 0x03) {
        RNDIS_SEEN.store(true, Ordering::Relaxed);
        return false;
    }
    if class == 0x0A && CTRL_SEEN.load(Ordering::Relaxed) {
        KIND.store(KIND_ECM, Ordering::Relaxed);
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
        ":: USBNET: candidate kind={} slot={} vidpid={:04x}:{:04x} cfg={} ctrl={} data={} alt={} imac={} mss={} in_mps={} out_mps={} ::",
        kind_name(), slot, VID.load(Ordering::Relaxed), PID.load(Ordering::Relaxed), CFG_VALUE.load(Ordering::Relaxed), CTRL_IFACE.load(Ordering::Relaxed),
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
                ":: USBNET: up kind={} slot={} cfg={} ctrl={} data={} alt={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} filter={} -> PASS ::",
                kind_name(), slot, CFG_VALUE.load(Ordering::Relaxed), CTRL_IFACE.load(Ordering::Relaxed),
                DATA_IFACE.load(Ordering::Relaxed), DATA_ALT.load(Ordering::Relaxed),
                m[0], m[1], m[2], m[3], m[4], m[5], if filter_ok { "ok" } else { "refused" }
            );
            #[cfg(all(target_arch = "aarch64", feature = "net6"))]
            crate::net_phy::net6::register_nic(&NET6_OPS);
        }
        Some(why) => {
            STATE.store(ST_FAILED, Ordering::Relaxed);
            serial_println!(":: USBNET: bring-up kind={} slot={} refused at {} -> FAIL ::", kind_name(), slot, why);
            serial_println!(":: USBNET: bus=xhci slot={} mac=00:00:00:00:00:00 link=down speed=0 usb=? rx=0 tx=0 refused={} -> FAIL ::", slot, why);
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
    tx_reset(); // NETCLOCK: an in-flight OUT TD dies with the slot
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
    ARMED.store(true, Ordering::Relaxed); NF_ARM_AT.store(crate::arch::ms().max(1), Ordering::Relaxed); // NETFRAME M3: the stall clock starts at the arm
}
fn reset_arm() {
    ARMED.store(false, Ordering::Relaxed);
    DONE.store(false, Ordering::Relaxed);
    TRB_PHYS.store(0, Ordering::Relaxed);
    DCI.store(0, Ordering::Relaxed);
}
/// Event-ring hook: claims the bulk-IN completion of the armed TRB (or any error on that endpoint).
pub fn claim(slot_id: u8, endpoint_id: u8, param: u64, code: u8, transfer_len: u32) -> bool {
    if tx_claim(slot_id, endpoint_id, param, code) {
        return true; // NETCLOCK M3: the in-flight bulk-OUT TD (same event-ring line, no new dispatch arm)
    }
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
        note_ethertype(frame);
        let _ = FIRST_FRAME_AT.compare_exchange(0, crate::arch::ms().max(1), Ordering::Relaxed, Ordering::Relaxed); // USBNET7
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
    TX_FRAMES.fetch_add(1, Ordering::Relaxed); NF_TX_LAST.store(crate::arch::ms().max(1), Ordering::Relaxed); // NETFRAME M2
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
/// NETCLOCK M3: queue one frame on the bounded TX ring and, when the controller loan is free, run one
/// data pass, which ISSUES it (stage + TRB + doorbell) and returns — the completion is reaped by a later
/// pass. Nothing waits on the wire. The stack checks `tx_room` first (back-pressure), so a drop here is
/// only the receive-path reply token racing a full ring — counted on the rollup line.
pub fn raw_tx(frame: &[u8]) {
    if !is_up() || !TXQ.lock().push(frame) {
        TX_DROP.fetch_add(1, Ordering::Relaxed);
        return;
    }
    drive();
}
/// One controller pass for this link, from the stack's context, when the loan is free. NETCLOCK: the
/// DATA pass only (RX reap/arm, TX reap/issue — no PHY poll, no bring-up, which stay on the main loop's
/// full `service_usbnet`), so the loan is held for the event drain and one enqueue; the hold is measured.
fn drive() {
    if let Ok(mut x) = crate::drivers::xhci::claim() {
        let t0 = crate::arch::now_cycles();
        x.poll_events();
        x.usbnet_data_pass();
        let held = crate::arch::now_cycles().wrapping_sub(t0);
        drop(x);
        LOAN_MAX_CYC.fetch_max(held, Ordering::Relaxed);
        DRIVES.fetch_add(1, Ordering::Relaxed);
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

/// AX88179: the station address straight from the NODE_ID register read (6 bytes).
pub fn set_mac_bytes(m: &[u8]) -> bool {
    if m.len() < 6 || m.iter().take(6).all(|&b| b == 0) {
        return false;
    }
    for i in 0..6 {
        MAC[i].store(m[i], Ordering::Relaxed);
    }
    true
}

/// The AX88179's register map and framing, as the part documents them (see the `KIND` note).
pub mod ax {
    pub const REQ_MAC: u8 = 0x01; // register space: wValue = reg, wIndex = byte count
    pub const REQ_PHY: u8 = 0x02; // PHY: wValue = phy id, wIndex = mii reg, 2 bytes
    pub const PHY_ID: u16 = 0x03;
    pub const REG_PHYSICAL_LINK_STATUS: u16 = 0x02; // 8-bit: 0x04 SS, 0x02 HS, 0x01 FS
    pub const REG_RX_CTL: u16 = 0x0b; // 16-bit
    pub const REG_NODE_ID: u16 = 0x10; // 6 bytes
    pub const REG_MEDIUM_STATUS_MODE: u16 = 0x22; // 16-bit
    pub const REG_MONITOR_MODE: u16 = 0x24; // 8-bit
    pub const REG_PHYPWR_RSTCTL: u16 = 0x26; // 16-bit
    pub const REG_RX_BULKIN_QCTRL: u16 = 0x2e; // 5 bytes
    pub const REG_CLK_SELECT: u16 = 0x33; // 8-bit
    pub const REG_RXCOE_CTL: u16 = 0x34; // 8-bit
    pub const REG_TXCOE_CTL: u16 = 0x35; // 8-bit
    pub const REG_PAUSE_WATERLVL_HIGH: u16 = 0x54; // 8-bit
    pub const REG_PAUSE_WATERLVL_LOW: u16 = 0x55; // 8-bit
    pub const PHYPWR_IPRL: u16 = 0x0020;
    pub const CLK_ACS_BCS: u8 = 0x03;
    /// DROPCRCERR 0x0100 | IPE 0x0200 (2-byte IP alignment pad on RX) | START 0x0080 | AP 0x0020 |
    /// AB 0x0008 | AMALL 0x0002.
    pub const RX_CTL_RUN: u16 = 0x03aa;
    pub const RX_CTL_STOP: u16 = 0x0000;
    /// RECEIVE_EN 0x0100 | TXFLOW 0x0020 | RXFLOW 0x0010 | EN_125MHZ 0x0008 | ALWAYS_ONE 0x0004 |
    /// FULL_DUPLEX 0x0002 | GIGAMODE 0x0001.
    pub const MEDIUM_RUN: u16 = 0x013f;
    pub const BMCR_ANEG_RESTART: u16 = 0x1200;
    /// Bulk-IN queue control by USB speed (the part's aggregation timer/size tuple).
    pub const BULKIN_QCTRL_SS: [u8; 5] = [0x07, 0x4f, 0x00, 0x12, 0xff];
    pub const BULKIN_QCTRL_HS: [u8; 5] = [0x07, 0x20, 0x03, 0x16, 0xff];
    /// Linux `ax88179_bulkin_size[3]`: full-speed USB.
    pub const BULKIN_QCTRL_FS: [u8; 5] = [0x07, 0xcc, 0x4c, 0x18, 0x08];
    /// TX header: two little-endian u32 — [0] = frame length, [1] = flags; bit 31|15 (0x80008000)
    /// asks the part to pad when the transfer would otherwise end exactly on a max packet.
    pub const TX_HDR_LEN: usize = 8;
    pub const TX_PAD_FLAG: u32 = 0x8000_8000;
    /// Per-packet header bits inside the RX trailer.
    pub const RXHDR_CRC_ERR: u32 = 1 << 29;
    pub const RXHDR_DROP_ERR: u32 = 1 << 31;
    pub const RX_PAD: usize = 2; // the IPE alignment pad ahead of every frame
    // ── PHY (MII) registers over REQ_PHY (bmRequestType 0xC0 read / 0x40 write, wValue = PHY_ID, wIndex = reg, 2 bytes) ──
    pub const MII_BMCR: u16 = 0x00; // Basic Mode Control: ANENABLE 0x1000 | ANRESTART 0x0200
    pub const MII_ADVERTISE: u16 = 0x04; // ADVERTISE_ALL 0x01e0 | CSMA 0x0001 | PAUSE_CAP 0x0400 (Linux ax88179_reset)
    pub const ADVERTISE_ALL_PAUSE: u16 = 0x05e1;
    pub const MII_CTRL1000: u16 = 0x09; // ADVERTISE_1000FULL 0x0200
    pub const ADVERTISE_1000FULL: u16 = 0x0200;
    pub const GMII_PHY_PHYSR: u16 = 0x11; // the part's PHY specific status (Linux GMII_PHY_PHYSR)
    pub const PHYSR_LINK: u16 = 0x0400; // real-time link
    pub const PHYSR_SMASK: u16 = 0xc000; // speed: 0x8000 gigabit, 0x4000 100M, 0 10M
    pub const PHYSR_GIGA: u16 = 0x8000;
    pub const PHYSR_100: u16 = 0x4000;
    pub const PHYSR_FULL: u16 = 0x2000; // duplex
    /// MEDIUM_STATUS_MODE bits (Linux AX_MEDIUM_*): GIGAMODE 0x0001, FULL_DUPLEX 0x0002, ALWAYS_ONE 0x0004,
    /// EN_125MHZ 0x0008, RXFLOW 0x0010, TXFLOW 0x0020, PS (100M) 0x0200, RECEIVE_EN 0x0100.
    pub const MEDIUM_BASE: u16 = 0x0100 | 0x0020 | 0x0010 | 0x0004;
    pub const MEDIUM_GIGA: u16 = 0x0001 | 0x0008;
    pub const MEDIUM_PS: u16 = 0x0200;
    pub const MEDIUM_125: u16 = 0x0008;
    pub const MEDIUM_FULL: u16 = 0x0002;
}

/// AX88179 receive: one bulk-IN transfer carries N frames and a trailer. The last 4 bytes are the
/// transfer header (`pkt_cnt` low 16, `hdr_off` high 16); at `hdr_off` sit `pkt_cnt` little-endian
/// u32 packet headers, each with the packet length in bits 16..29 (2 pad bytes + the frame); packets
/// start at 0 and each occupies `(len + 7) & !7` bytes. Bad geometry is counted and the transfer
/// dropped whole; a per-packet CRC/DROP flag drops that packet only.
pub fn deliver_ax(buf: &[u8]) {
    let n = buf.len();
    if n < 4 {
        return; // ZLP / nothing
    }
    netframe_rx0(buf); // NETFRAME M2: the first non-empty completion's bytes, once, whatever the parse says
    let (rx_hdr, pkt_cnt, hdr_off) = usbnet_core::ax88179::trailer(buf).unwrap_or((0, 0, 0));
    let first_hdr = if hdr_off + 4 <= n { u32::from_le_bytes([buf[hdr_off], buf[hdr_off + 1], buf[hdr_off + 2], buf[hdr_off + 3]]) } else { 0 };
    // NETFRAME M3: the split is the shared core's (`usbnet_core::ax88179`, host KATs); verdicts and counters unchanged.
    let split = usbnet_core::ax88179::split(buf, ax::RX_PAD, |p| match p {
        usbnet_core::Pkt::Pad => { RX_PAD_HDR.fetch_add(1, Ordering::Relaxed); }
        usbnet_core::Pkt::Flagged { hdr, crc, off, len } => {
            if crc { RX_CRC.fetch_add(1, Ordering::Relaxed); } else { RX_CHIP_DROP.fetch_add(1, Ordering::Relaxed); }
            RX_DROP.fetch_add(1, Ordering::Relaxed);
            flagged_once(hdr, &buf[off..(off + len).min(off + 16)]);
        }
        usbnet_core::Pkt::Frame { off, len } => {
            RX_OK.fetch_add(1, Ordering::Relaxed);
            deliver(&buf[off..off + len]);
        }
    });
    if let Some(r) = split.refused {
        RX_SHORT.fetch_add(1, Ordering::Relaxed);
        ERRORS.fetch_add(1, Ordering::Relaxed);
        RX_DROP.fetch_add(1, Ordering::Relaxed);
        let why = match r { usbnet_core::Refusal::PktLen => "pkt-len-bad", _ => "short-geometry" };
        let ph = if split.packets < pkt_cnt && hdr_off + split.packets * 4 + 4 <= n { let h = hdr_off + split.packets * 4; u32::from_le_bytes([buf[h], buf[h + 1], buf[h + 2], buf[h + 3]]) } else { first_hdr };
        rx_raw_once(n, rx_hdr, pkt_cnt, hdr_off, ph, why);
    }
}

static RX_OK: AtomicU64 = AtomicU64::new(0);
static RX_CRC: AtomicU64 = AtomicU64::new(0);
static RX_DROP_ERR: AtomicU64 = AtomicU64::new(0);
static RX_SHORT: AtomicU64 = AtomicU64::new(0);
static RX_CHIP_DROP: AtomicU64 = AtomicU64::new(0);
static MEDIUM_RB: AtomicU16 = AtomicU16::new(0);
static RX_DUMPED: AtomicU8 = AtomicU8::new(0);
/// Last MEDIUM_STATUS_MODE read back from the part (`re=` is its RECEIVE_EN bit).
pub fn note_medium_readback(m: u16) { MEDIUM_RB.store(m, Ordering::Relaxed); }
/// USBNET5: the first chip-dropped transfer's head and trailer bytes, once, so the payload under a DROP header is readable.
#[allow(dead_code)] // USBNET6: the dummy header is no longer a drop; kept for the next geometry read
fn rx_dump_once(buf: &[u8], hdr_off: usize) {
    if RX_DUMPED.swap(1, Ordering::Relaxed) != 0 { return; }
    let n = buf.len();
    serial_println!("[usbnet] rx dump head={:02x?}", &buf[..n.min(32)]);
    let t = hdr_off.min(n);
    serial_println!("[usbnet] rx dump trailer@{}={:02x?}", t, &buf[t..n.min(t + 16)]);
}
static RX_RAW_SEEN: AtomicU64 = AtomicU64::new(0);
/// M1: print the first dropped transfer's shape once, so a geometry mismatch is named on the bench.
fn rx_raw_once(len: usize, rx_hdr: u32, pkt_cnt: usize, hdr_off: usize, first_pkt_hdr: u32, reason: &str) {
    if RX_RAW_SEEN.fetch_add(1, Ordering::Relaxed) != 0 {
        return;
    }
    serial_println!(
        "[usbnet] rx raw len={} rx_hdr={:#010x} pkt_cnt={} hdr_off={} first_pkt_hdr={:#010x} reason={}",
        len, rx_hdr, pkt_cnt, hdr_off, first_pkt_hdr, reason
    );
}

fn rollup() {
    if !CENSUS.load(Ordering::Relaxed) { return; } // USBNET6 (R80): the doubling RX/TX census runs only under `tests usbnet`
    let total = RX_FRAMES.load(Ordering::Relaxed) + TX_FRAMES.load(Ordering::Relaxed);
    let reported = REPORTED.load(Ordering::Relaxed);
    if total < reported.saturating_mul(2).max(1) {
        return;
    }
    REPORTED.store(total, Ordering::Relaxed); if !crate::census::on(crate::census::USBNET) { return; } // QUIETBOOT (R80): a census, OFF until `census start`.
    serial_println!(
        ":: USBNET: rx={} tx={} rx_drop={} tx_drop={} errors={} rx_ok={} rx_crc={} rx_drop_err={} rx_short={} rx_chip_drop={} rx_xfers={} rx_zlp={} rx_arms={} polls={} tx_q={} tx_stuck={} ::",
        RX_FRAMES.load(Ordering::Relaxed), TX_FRAMES.load(Ordering::Relaxed),
        RX_DROP.load(Ordering::Relaxed), TX_DROP.load(Ordering::Relaxed), ERRORS.load(Ordering::Relaxed),
        RX_OK.load(Ordering::Relaxed), RX_CRC.load(Ordering::Relaxed), RX_DROP_ERR.load(Ordering::Relaxed), RX_SHORT.load(Ordering::Relaxed), RX_CHIP_DROP.load(Ordering::Relaxed), RX_XFERS.load(Ordering::Relaxed), RX_ZLP.load(Ordering::Relaxed), RX_ARMS.load(Ordering::Relaxed),
        stack_polls(), tx_q(), TX_STUCK.load(Ordering::Relaxed) // NETCLOCK (B335)
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

/// AX88179 register access behind a transport, so the xHCI (`XhciAx`, xhci/mod.rs tail) and the EHCI
/// (`EhciAx`, ehci/mod.rs tail) front-ends drive the SAME register logic. Vendor requests only:
/// `reg_read`/`reg_write` are bRequest 0x01 (bmRequestType 0xC0/0x40, wValue = reg, wIndex = len).
pub mod ax_xport {
    use super::ax::*;
    pub trait AxTransport {
        fn reg_read(&mut self, reg: u16, out: &mut [u8]) -> bool;
        fn reg_write(&mut self, reg: u16, data: &[u8]) -> bool;
        fn wait_ms(&mut self, ms: u64);
    }
    /// Power/reset, clock select, then the station address from NODE_ID — the front of the xHCI
    /// bring-up (`usbnet_bringup_ax`), in the same order and with the same settles. Err names the step.
    pub fn identity<T: AxTransport>(t: &mut T) -> Result<[u8; 6], &'static str> {
        if !t.reg_write(REG_PHYPWR_RSTCTL, &0u16.to_le_bytes()) { return Err("PHYPWR_RSTCTL=0"); }
        t.wait_ms(10);
        if !t.reg_write(REG_PHYPWR_RSTCTL, &PHYPWR_IPRL.to_le_bytes()) { return Err("PHYPWR_RSTCTL=IPRL"); }
        t.wait_ms(200);
        if !t.reg_write(REG_CLK_SELECT, &[CLK_ACS_BCS]) { return Err("CLK_SELECT"); }
        t.wait_ms(100);
        let mut mac = [0u8; 6];
        if !t.reg_read(REG_NODE_ID, &mut mac) { return Err("NODE_ID"); }
        Ok(mac)
    }
    /// PHYSICAL_LINK_STATUS: (link up, raw byte). Bits 0x04 SS / 0x02 HS / 0x01 FS are the negotiated USB speed.
    pub fn link<T: AxTransport>(t: &mut T) -> Option<(bool, u8)> {
        let mut b = [0u8; 1];
        if !t.reg_read(REG_PHYSICAL_LINK_STATUS, &mut b) { return None; }
        Some((b[0] & 0x07 != 0, b[0]))
    }
}

// ── USBNET3: the AX88179 link poll + the `bus=xhci` witness ─────────────────────────────────────
// The link is polled from the controller's service pass (`Controller::usbnet_ax_poll`), never blocked on:
// autonegotiation takes seconds, the main loop must keep painting. `poll_due` gates the cadence; `link_seen`
// records what the PHY said and returns the MEDIUM_STATUS_MODE word to write when the link comes up.
static LINK: AtomicBool = AtomicBool::new(false);
static SPEED_MBPS: AtomicU16 = AtomicU16::new(0);
static USB_SPD: AtomicU8 = AtomicU8::new(0); // PHYSICAL_LINK_STATUS raw byte
static POLL_T0: AtomicU64 = AtomicU64::new(0);
static POLL_NEXT: AtomicU64 = AtomicU64::new(0);
static WIT_N: AtomicU8 = AtomicU8::new(0);
static WIT_NEXT: AtomicU64 = AtomicU64::new(0);
const LINK_WAIT_MS: u64 = 15_000;

/// Bring-up finished for the AX front-end: start the poll clock.
pub fn poll_start(now_ms: u64, plsr: u8) {
    USB_SPD.store(plsr, Ordering::Relaxed);
    POLL_T0.store(now_ms, Ordering::Relaxed);
    POLL_NEXT.store(now_ms + 250, Ordering::Relaxed);
    WIT_N.store(0, Ordering::Relaxed);
    LINK.store(false, Ordering::Relaxed);
}
/// `true` when the service pass should read the PHY now (250 ms while waiting for link, 2 s after).
pub fn poll_due(now_ms: u64) -> bool {
    POLL_T0.load(Ordering::Relaxed) != 0 && now_ms >= POLL_NEXT.load(Ordering::Relaxed)
}
/// Record one PHYSR reading (`None` = the read failed). Returns the MEDIUM_STATUS_MODE word to write when
/// the link has just come up (Linux `ax88179_link_reset`: speed bits from PHYSR, EN_125MHZ when USB is HS/SS).
pub fn link_seen(now_ms: u64, physr: Option<u16>, plsr: u8) -> Option<u16> {
    use ax::*;
    let was = LINK.load(Ordering::Relaxed);
    let up = physr.map(|p| p & PHYSR_LINK != 0).unwrap_or(false);
    POLL_NEXT.store(now_ms + if up { 2000 } else { 250 }, Ordering::Relaxed);
    USB_SPD.store(plsr, Ordering::Relaxed);
    let mut medium = None;
    if up && !was {
        let p = physr.unwrap_or(0);
        let mut m = MEDIUM_BASE;
        let mbps = match p & PHYSR_SMASK { PHYSR_GIGA => { m |= MEDIUM_GIGA; 1000 } PHYSR_100 => { m |= MEDIUM_PS; if plsr & 0x06 != 0 { m |= MEDIUM_125; } 100 } _ => 10 };
        if p & PHYSR_FULL != 0 { m |= MEDIUM_FULL; }
        SPEED_MBPS.store(mbps, Ordering::Relaxed);
        LINK.store(true, Ordering::Relaxed);
        serial_println!("[usbnet] link up speed={}M duplex={} physr={:#06x} usb_plsr={:#04x} medium={:#06x}", mbps, if p & PHYSR_FULL != 0 { "full" } else { "half" }, p, plsr, m);
        medium = Some(m);
    } else if !up && was {
        LINK.store(false, Ordering::Relaxed);
        SPEED_MBPS.store(0, Ordering::Relaxed);
        serial_println!("[usbnet] link down (PHYSR={:?})", physr);
    }
    medium
}
fn usb_name() -> &'static str {
    let b = USB_SPD.load(Ordering::Relaxed);
    if b & 0x04 != 0 { "ss" } else if b & 0x02 != 0 { "hs" } else if b & 0x01 != 0 { "fs" } else { "?" }
}
/// The one-line reading: `:: USBNET: bus=xhci slot= mac= link= speed= usb= rx= tx= -> PASS ::`. PASS = the MAC
/// was read and the link was polled (link=down is a valid reading — no cable); rx/tx are the evidence.
pub fn witness(bus: &str, slot: u8) {
    let m = mac();
    let ok = m.iter().any(|&b| b != 0);
    serial_println!(
        ":: USBNET: bus={} slot={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link={} speed={} usb={} rx={} tx={} rx_drop={} tx_drop={} errors={} rx_ok={} rx_crc={} rx_drop_err={} rx_short={} rx_chip_drop={} rx_xfers={} rx_zlp={} rx_arms={} buf={} re={} -> {} ::",
        bus, slot, m[0], m[1], m[2], m[3], m[4], m[5],
        if LINK.load(Ordering::Relaxed) { "up" } else { "down" }, SPEED_MBPS.load(Ordering::Relaxed), usb_name(),
        RX_FRAMES.load(Ordering::Relaxed), TX_FRAMES.load(Ordering::Relaxed),
        RX_DROP.load(Ordering::Relaxed), TX_DROP.load(Ordering::Relaxed), ERRORS.load(Ordering::Relaxed),
        RX_OK.load(Ordering::Relaxed), RX_CRC.load(Ordering::Relaxed), RX_DROP_ERR.load(Ordering::Relaxed), RX_SHORT.load(Ordering::Relaxed),
        RX_CHIP_DROP.load(Ordering::Relaxed), RX_XFERS.load(Ordering::Relaxed), RX_ZLP.load(Ordering::Relaxed), RX_ARMS.load(Ordering::Relaxed), rx_len(), (MEDIUM_RB.load(Ordering::Relaxed) >> 8) & 1,
        if ok { "PASS" } else { "FAIL" }
    );
}
/// Emit the witness at link resolution (up, or `LINK_WAIT_MS` without link), then at +20 s and +60 s so rx/tx
/// carry the reading. Called from the service pass after each poll.
pub fn witness_tick(now_ms: u64, slot: u8) {
    let t0 = POLL_T0.load(Ordering::Relaxed);
    if t0 == 0 { return; }
    let n = WIT_N.load(Ordering::Relaxed);
    let due = match n {
        0 => LINK.load(Ordering::Relaxed) || now_ms.saturating_sub(t0) >= LINK_WAIT_MS,
        1 | 2 => now_ms >= WIT_NEXT.load(Ordering::Relaxed),
        _ => false,
    };
    if !due { return; }
    if n == 0 { verdict(); } else if CENSUS.load(Ordering::Relaxed) { witness("xhci", slot); } // USBNET6 (R80): the driver's verdict at link resolution; the census repeats only under `tests usbnet`
    WIT_N.store(n + 1, Ordering::Relaxed);
    WIT_NEXT.store(now_ms + if n == 0 { 20_000 } else { 40_000 }, Ordering::Relaxed);
}

// ── USBNET6 ─────────────────────────────────────────────────────────────────────────────────────────
// The alignment-dummy count, the RX_CTL readback, the first three chip-flagged headers, the per-ethertype
// census the `tests usbnet` fixture reads, and the bring-up verdict line.
static RX_PAD_HDR: AtomicU64 = AtomicU64::new(0);
static RX_CTL_RB: AtomicU16 = AtomicU16::new(0);
static FLAGGED: AtomicU8 = AtomicU8::new(0);
static CENSUS: AtomicBool = AtomicBool::new(false);
static FIRST_ETYPE: AtomicU16 = AtomicU16::new(0);
static RX_ARP: AtomicU64 = AtomicU64::new(0);
static RX_V4: AtomicU64 = AtomicU64::new(0);
static RX_V6: AtomicU64 = AtomicU64::new(0);
static RX_DHCP: AtomicU64 = AtomicU64::new(0);

/// 0x-prefixed hex of a register value: little-endian number for 1-2 bytes, bytes in written order for longer ones.
pub struct HexLe<'a>(pub &'a [u8]);
impl core::fmt::Display for HexLe<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0.len() {
            0 => f.write_str("-"),
            1 => write!(f, "{:#04x}", self.0[0]),
            2 => write!(f, "{:#06x}", u16::from_le_bytes([self.0[0], self.0[1]])),
            _ => { f.write_str("0x")?; for b in self.0 { write!(f, "{:02x}", b)?; } Ok(()) }
        }
    }
}
/// The RX_CTL value the part reads back (bring-up `reg RX_CTL=` and the link-up `regs` dump).
pub fn note_rx_ctl_readback(v: u16) { RX_CTL_RB.store(v, Ordering::Relaxed); }
/// PHY link (PHYSR LINK) — distinct from `is_up()`, which is "bring-up finished".
pub fn link_up() -> bool { LINK.load(Ordering::Relaxed) }
/// The first three chip-flagged (DROP_ERR / CRC_ERR with a real length) packet headers of the boot, raw.
fn flagged_once(pkt_hdr: u32, head: &[u8]) {
    let k = FLAGGED.fetch_add(1, Ordering::Relaxed);
    if k >= 3 { return; }
    serial_println!("[usbnet] rx flagged #{} hdr={:#010x} drop={} crc={} len={} head={:02x?}", k + 1, pkt_hdr, (pkt_hdr >> 31) & 1, (pkt_hdr >> 29) & 1, (pkt_hdr >> 16) & 0x1fff, head);
}
/// Per-ethertype counts of frames that reached the RX ring (ARP / IPv4 / IPv6, and IPv4 UDP from port 67 = a DHCP server reply).
fn note_ethertype(f: &[u8]) {
    if f.len() < 14 { return; }
    let et = u16::from_be_bytes([f[12], f[13]]);
    let _ = FIRST_ETYPE.compare_exchange(0, et, Ordering::Relaxed, Ordering::Relaxed); NF_LAST_ETYPE.store(et, Ordering::Relaxed); // NETFRAME: the newest frame's ethertype, for `tests usbnet7`
    match et {
        0x0806 => { RX_ARP.fetch_add(1, Ordering::Relaxed); }
        0x86dd => { RX_V6.fetch_add(1, Ordering::Relaxed); }
        0x0800 => {
            RX_V4.fetch_add(1, Ordering::Relaxed);
            let ihl = ((f.get(14).copied().unwrap_or(0) & 0x0f) as usize) * 4;
            if f.len() >= 14 + ihl + 4 && f[23] == 17 && u16::from_be_bytes([f[14 + ihl], f[15 + ihl]]) == 67 { RX_DHCP.fetch_add(1, Ordering::Relaxed); }
        }
        _ => {}
    }
}
/// The bring-up verdict, once at link resolution (R80: the driver deciding, not a census).
fn verdict() {
    let m = mac();
    serial_println!(
        "[usbnet] link={} speed={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} rx_ctl={:#06x} rx_ok={} rx_drop={} rx_pad={}",
        if LINK.load(Ordering::Relaxed) { "up" } else { "down" }, SPEED_MBPS.load(Ordering::Relaxed),
        m[0], m[1], m[2], m[3], m[4], m[5], RX_CTL_RB.load(Ordering::Relaxed),
        RX_OK.load(Ordering::Relaxed), RX_DROP.load(Ordering::Relaxed), RX_PAD_HDR.load(Ordering::Relaxed)
    );
}

/// USBNET6 M4 — `tests usbnet`: with a link, pull frames off the dongle for up to 5 s and PASS on the first one
/// (its ethertype printed); no dongle / no link → SKIP; nothing received while the chip flagged packets → FAIL
/// naming the RX_CTL readback. The census (rollup + the full `:: USBNET: bus=` line) is switched on for the run.
/// Frames pulled here are consumed by the fixture, not the stack (5 s at most).
pub fn selftest() {
    CENSUS.store(true, Ordering::Relaxed);
    let chip = match kind() { KIND_AX88179 => "ax88179", KIND_ECM => "ecm", _ => "none" };
    let mfb = rx_len();
    let rxctl = RX_CTL_RB.load(Ordering::Relaxed);
    let skip = if !is_up() { Some("no-dongle") } else if kind() == KIND_AX88179 && !link_up() { Some("no-link") } else { None };
    if let Some(r) = skip {
        serial_println!(":: USBNET6: chip={} rx_ctl={:#06x} mfb=qctrl-buf{} frames=0 dropped=0 first_ethertype=none reason={} -> SKIP ::", chip, rxctl, mfb, r);
        serial_println!(":: USBNET7: rx_ok=0 first_frame_ms=none ethertype=none reason={} -> SKIP ::", r);
        return;
    }
    witness("xhci", slot());
    let d0 = RX_CHIP_DROP.load(Ordering::Relaxed) + RX_CRC.load(Ordering::Relaxed);
    let t0 = crate::arch::ms();
    let mut buf = [0u8; FRAME_CAP];
    let mut frames = 0u32;
    let mut first: Option<u16> = None;
    let mut first_at = 0u64;
    while crate::arch::ms().saturating_sub(t0) < 5000 {
        if let Some(n) = raw_rx(&mut buf) {
            if frames == 0 { first_at = crate::arch::ms(); } // USBNET7: when the first frame of the run arrived
            frames += 1;
            if first.is_none() && n >= 14 { first = Some(u16::from_be_bytes([buf[12], buf[13]])); }
            if frames >= 4 { break; }
        } else {
            core::hint::spin_loop();
        }
    }
    let dropped = RX_CHIP_DROP.load(Ordering::Relaxed) + RX_CRC.load(Ordering::Relaxed) - d0;
    let et = match first { Some(e) => alloc::format!("{:#06x}", e), None => alloc::string::String::from("none") };
    let (v, why) = if frames > 0 { ("PASS", "") } else if dropped > 0 { ("FAIL", " reason=chip-drop-see-rx_ctl") } else { ("FAIL", " reason=no-frame-5s") };
    serial_println!(
        "[usbnet] census arp={} ipv4={} ipv6={} dhcp_replies={} rx_pad={} rx_ok={} leased={}",
        RX_ARP.load(Ordering::Relaxed), RX_V4.load(Ordering::Relaxed), RX_V6.load(Ordering::Relaxed), RX_DHCP.load(Ordering::Relaxed),
        RX_PAD_HDR.load(Ordering::Relaxed), RX_OK.load(Ordering::Relaxed), leased() as u8
    );
    serial_println!(":: USBNET6: chip={} rx_ctl={:#06x} mfb=qctrl-buf{} frames={} dropped={} first_ethertype={}{} -> {} ::", chip, rxctl, mfb, frames, dropped, et, why, v);
    usbnet7_verdict(t0, first_at, first);
}
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn leased() -> bool { crate::smolnet::leased() }
#[cfg(not(all(feature = "smolnet", target_arch = "x86_64")))]
fn leased() -> bool { false }

// ── USBNET7 (B328) ──────────────────────────────────────────────────────────────────────────────────
// Boot 20 received NOTHING after USBNET6 and the wire could not say whether the armed bulk-IN TD ever completed: a completion
// under 4 bytes went nowhere, the arm itself printed nothing. These count every IN completion, every empty one, every arm, and
// keep the FIRST bulk endpoint of each direction from the descriptor walk (the walk keeps the last: OUT 0x05 instead of 0x03).
static RX_XFERS: AtomicU64 = AtomicU64::new(0);
static RX_ZLP: AtomicU64 = AtomicU64::new(0);
static RX_ARMS: AtomicU64 = AtomicU64::new(0);
static FIRST_FRAME_AT: AtomicU64 = AtomicU64::new(0);
/// The first bulk IN / OUT endpoint of the current walk: `addr << 16 | mps`, 0 = none yet.
static FIRST_IN: AtomicU32 = AtomicU32::new(0);
static FIRST_OUT: AtomicU32 = AtomicU32::new(0);
/// The RX_BULKIN_QCTRL tuple written at bring-up, rewritten at link-up.
static QCTRL: [AtomicU8; 5] = [const { AtomicU8::new(0) }; 5];

/// Descriptor walk: one bulk endpoint of the link's slot. First of each direction wins.
pub fn note_bulk_ep(slot: u8, addr: u8, mps: u16) {
    if SLOT.load(Ordering::Relaxed) != slot || STATE.load(Ordering::Relaxed) >= ST_CONFIGURED {
        return;
    }
    let cell = if addr & 0x80 != 0 { &FIRST_IN } else { &FIRST_OUT };
    let _ = cell.compare_exchange(0, ((addr as u32) << 16) | mps as u32, Ordering::Relaxed, Ordering::Relaxed);
}
/// The bulk pair the link configures: for the AX88179 the FIRST of each direction (Linux `usbnet_get_endpoints`: IN 0x82,
/// OUT 0x03 — the part's descriptor also lists a bulk OUT 0x05, which the walk's last-wins picked); ECM keeps the walk's pair.
pub fn first_bulk(slot: u8, walk_in: Option<(u8, u16)>, walk_out: Option<(u8, u16)>) -> (Option<(u8, u16)>, Option<(u8, u16)>) {
    if SLOT.load(Ordering::Relaxed) != slot || kind() != KIND_AX88179 {
        return (walk_in, walk_out);
    }
    let dec = |v: u32| if v == 0 { None } else { Some(((v >> 16) as u8, (v & 0xffff) as u16)) };
    (dec(FIRST_IN.load(Ordering::Relaxed)).or(walk_in), dec(FIRST_OUT.load(Ordering::Relaxed)).or(walk_out))
}
pub fn set_qctrl(q: &[u8; 5]) {
    for i in 0..5 { QCTRL[i].store(q[i], Ordering::Relaxed); }
}
pub fn qctrl() -> [u8; 5] {
    let mut q = [0u8; 5];
    for i in 0..5 { q[i] = QCTRL[i].load(Ordering::Relaxed); }
    q
}
/// One bulk-IN completion of `n` bytes (success / short packet).
pub fn note_xfer(n: usize) {
    RX_XFERS.fetch_add(1, Ordering::Relaxed);
    if n < 4 { RX_ZLP.fetch_add(1, Ordering::Relaxed); }
    netframe_note_xfer(n); // NETFRAME M2: the completion log (length + ms) and the last completion's time
}
/// One bulk-IN TD posted. The first prints `[usbnet] rx_arm n=<queued> ep=<addr> mps=<n>` once (bring-up, not per frame).
pub fn note_arm(queued: u32, ep: u8) {
    if RX_ARMS.fetch_add(1, Ordering::Relaxed) == 0 {
        serial_println!("[usbnet] rx_arm n={} ep={:#04x} mps={} buf={}", queued, ep, IN_MPS.load(Ordering::Relaxed), rx_len());
    }
}
/// `tests usbnet`, USBNET7: did ANY frame arrive within 3 s of the fixture's start?
/// `:: USBNET7: rx_ok=<n> first_frame_ms=<n> ethertype=<0x....> -> PASS|FAIL ::`; on FAIL the bulk-IN ring once.
fn usbnet7_verdict(t0: u64, first_at: u64, first: Option<u16>) {
    let ms = if first_at != 0 { Some(first_at.saturating_sub(t0)) } else { None };
    let pass = matches!(ms, Some(m) if m <= 3000);
    let et = match first { Some(e) => alloc::format!("{:#06x}", e), None => alloc::string::String::from("none") };
    let fms = match ms { Some(m) => alloc::format!("{}", m), None => alloc::string::String::from("none") };
    serial_println!(":: USBNET7: rx_ok={} first_frame_ms={} ethertype={} -> {} ::", RX_OK.load(Ordering::Relaxed), fms, et, if pass { "PASS" } else { "FAIL" });
    if pass { return; }
    let st = match crate::drivers::xhci::claim() { Ok(x) => x.usbnet_ring_state(), Err(_) => None };
    let pending = (ARMED.load(Ordering::Relaxed) && !DONE.load(Ordering::Relaxed)) as u8;
    match st {
        Some((deq, enq, cyc, eps)) => serial_println!(
            "[usbnet] ring deq={:#x} enq={} cycle={} pending={} ep_state={} trb={:#x} xfers={} zlp={} arms={} first_frame_at={}",
            deq, enq, cyc as u8, pending, eps, TRB_PHYS.load(Ordering::Relaxed), RX_XFERS.load(Ordering::Relaxed), RX_ZLP.load(Ordering::Relaxed),
            RX_ARMS.load(Ordering::Relaxed), FIRST_FRAME_AT.load(Ordering::Relaxed)
        ),
        None => serial_println!(
            "[usbnet] ring deq=? enq=? cycle=? pending={} ep_state=? trb={:#x} xfers={} zlp={} arms={} first_frame_at={}",
            pending, TRB_PHYS.load(Ordering::Relaxed), RX_XFERS.load(Ordering::Relaxed), RX_ZLP.load(Ordering::Relaxed),
            RX_ARMS.load(Ordering::Relaxed), FIRST_FRAME_AT.load(Ordering::Relaxed)
        ),
    }
}

// ── NETCLOCK (B335) ─────────────────────────────────────────────────────────────────────────────────
// Boot 20: every frame was one synchronous bulk-OUT TD awaited with the xHCI loan held (`usbnet_tx_stage` on
// `pump_until_ftdi_done`, worst wait 56.7 ms), and the stack polled ~1M times a second, each poll a controller
// pass. Now: TX is asynchronous — ONE OUT TD in flight, issued by the data pass, its completion claimed by
// `claim` (the IN claim's line) and reaped by the next pass; the stack's poll gate asks `rx_ready`, which
// drives the controller at most every `DRIVE_PACE_US`.
static TX_ARMED: AtomicBool = AtomicBool::new(false);
static TX_DONE: AtomicBool = AtomicBool::new(false);
static TX_CODE: AtomicU8 = AtomicU8::new(0);
static TX_DCI: AtomicU8 = AtomicU8::new(0);
static TX_TRB: AtomicU64 = AtomicU64::new(0);
static TX_T0: AtomicU64 = AtomicU64::new(0);
static TX_STUCK: AtomicU64 = AtomicU64::new(0);
static LOAN_MAX_CYC: AtomicU64 = AtomicU64::new(0);
static DRIVES: AtomicU64 = AtomicU64::new(0);
static LAST_DRIVE: AtomicU64 = AtomicU64::new(0);
/// The stack-side controller pass runs at most this often from a pump's poll gate.
const DRIVE_PACE_US: u64 = 250;
/// An issued OUT TD with no completion after this long is abandoned (counted `tx_stuck`) so TX cannot wedge.
const TX_STUCK_MS: u64 = 1_000;

fn cycles_per_us() -> u64 {
    (crate::arch::hw_wait_budget() / 2_000_000).max(1)
}
/// Event-ring hook: the completion of the in-flight OUT TD (matched by TRB address), or any error on its endpoint.
fn tx_claim(slot_id: u8, endpoint_id: u8, param: u64, code: u8) -> bool {
    if !TX_ARMED.load(Ordering::Relaxed) || SLOT.load(Ordering::Relaxed) != slot_id || TX_DCI.load(Ordering::Relaxed) != endpoint_id {
        return false;
    }
    let is_error = code != 1 && code != 13;
    if param != TX_TRB.load(Ordering::Relaxed) && !is_error {
        return false;
    }
    if !TX_DONE.swap(true, Ordering::Relaxed) {
        TX_CODE.store(code, Ordering::Relaxed);
    }
    true
}
fn tx_reset() {
    TX_ARMED.store(false, Ordering::Relaxed);
    TX_DONE.store(false, Ordering::Relaxed);
    TX_TRB.store(0, Ordering::Relaxed);
    TX_DCI.store(0, Ordering::Relaxed);
}
/// The data pass: is an OUT TD in flight (issued, not yet reaped)?
pub fn tx_inflight() -> bool {
    TX_ARMED.load(Ordering::Relaxed)
}
/// The data pass issued one OUT TD (its last TRB, the one with IOC, at `trb_phys`).
pub fn tx_arm(dci: u8, trb_phys: u64) {
    TX_DCI.store(dci, Ordering::Relaxed);
    TX_TRB.store(trb_phys, Ordering::Relaxed);
    TX_DONE.store(false, Ordering::Relaxed);
    TX_T0.store(crate::arch::now_cycles(), Ordering::Relaxed);
    TX_ARMED.store(true, Ordering::Relaxed);
}
/// The data pass reaps: `Some(code)` once the in-flight TD completed (or was abandoned: code 0), freeing the slot.
pub fn tx_take_done() -> Option<u8> {
    if !TX_ARMED.load(Ordering::Relaxed) {
        return None;
    }
    if TX_DONE.load(Ordering::Relaxed) {
        let code = TX_CODE.load(Ordering::Relaxed);
        tx_reset();
        return Some(code);
    }
    let age_us = crate::arch::now_cycles().wrapping_sub(TX_T0.load(Ordering::Relaxed)) / cycles_per_us();
    if age_us >= TX_STUCK_MS * 1000 {
        if TX_STUCK.fetch_add(1, Ordering::Relaxed) == 0 {
            serial_println!("[usbnet] tx stuck dci={} trb={:#x} age_ms={} — abandoned, TX continues", TX_DCI.load(Ordering::Relaxed), TX_TRB.load(Ordering::Relaxed), age_us / 1000);
        }
        tx_reset();
        return Some(0);
    }
    None
}
/// Back-pressure for the stack: room on the TX ring right now.
pub fn tx_room() -> bool {
    is_up() && TXQ.lock().len() < RING - 1
}
/// Frames waiting on the TX ring (the census `tx_q=`).
pub fn tx_q() -> usize {
    TXQ.lock().len()
}
/// The stack's poll gate: a frame waiting on the RX ring? With `drive` (a pump — the main loop's pass is not
/// running) one paced data pass first, which also reaps/issues TX.
pub fn rx_ready(drive_now: bool) -> bool {
    if !is_up() {
        return false;
    }
    if !RXQ.lock().is_empty() {
        return true;
    }
    if drive_now {
        let now = crate::arch::now_cycles();
        let last = LAST_DRIVE.load(Ordering::Relaxed);
        if now.wrapping_sub(last) >= DRIVE_PACE_US * cycles_per_us() {
            LAST_DRIVE.store(now, Ordering::Relaxed);
            drive();
            return !RXQ.lock().is_empty();
        }
    }
    false
}
/// `tests netclock`: the longest stack-side loan hold since the last reset, in µs, and the pass count.
pub fn loan_held_max_us() -> u64 {
    LOAN_MAX_CYC.load(Ordering::Relaxed) / cycles_per_us()
}
pub fn loan_reset() {
    LOAN_MAX_CYC.store(0, Ordering::Relaxed);
}
pub fn drives() -> u64 {
    DRIVES.load(Ordering::Relaxed)
}
/// `tests netclock`: frames sent (completed) and good frames received so far.
pub fn tx_frames() -> u64 {
    TX_FRAMES.load(Ordering::Relaxed)
}
pub fn rx_ok() -> u64 {
    RX_OK.load(Ordering::Relaxed).max(RX_FRAMES.load(Ordering::Relaxed))
}
/// `tests netclock`: one main-loop-shaped controller pass (`poll_events` + `service_ftdi`, which drains the
/// FTDI console and runs the full `service_usbnet`: PHY poll, RX, TX), for a fixture that holds the main loop
/// while it measures. Not counted in the stack-side loan hold.
pub fn main_pass() {
    if let Ok(mut x) = crate::drivers::xhci::claim() {
        x.poll_events();
        x.service_ftdi();
    }
}
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn stack_polls() -> u64 { crate::net_tick::polls() }
#[cfg(not(all(feature = "smolnet", target_arch = "x86_64")))]
fn stack_polls() -> u64 { 0 }

// ── NETFRAME (rmbp-ledger B368) ─────────────────────────────────────────────────────────────────────
// Flight 22: `rx_ok=35 rx_pad=35` with `census ipv4=25 ipv6=10` — every completion parsed to one real frame plus the
// alignment dummy, so the framing was right; then RX STOPPED (`xfers=35 arms=36 pending=1 ep_state=1`, no error event).
// M2 makes the next flight show the framing AND the stall on the wire: the first completion's bytes once, a log of
// the first completions (length + ms), the last RX / TX completion times, and on a stall the dongle's port link
// state. M3: a TD pending past `NF_KICK_MS` with the PHY up gets its IN doorbell rung again (`kicks=`) — a
// recovery candidate for a doorbell the controller missed, harmless on a Running endpoint (xHCI 1.2 §4.7) —
// and `tests usbnet7`, which drives the main pass and the stack itself and asks for a frame, an IPv4 frame and a
// DHCP server reply.
const NF_LOG: usize = 16;
/// A bulk-IN TD with no completion for this long (PHY link up) is kicked; again every `NF_KICK_MS`.
const NF_KICK_MS: u64 = 2_000;
/// `tests usbnet7` window.
const NF_WINDOW_MS: u64 = 10_000;
static NF_RX0: AtomicBool = AtomicBool::new(false);
static NF_LENS: [AtomicU32; NF_LOG] = [const { AtomicU32::new(0) }; NF_LOG];
static NF_MS: [AtomicU64; NF_LOG] = [const { AtomicU64::new(0) }; NF_LOG];
static NF_N: AtomicU64 = AtomicU64::new(0);
static NF_RX_LAST: AtomicU64 = AtomicU64::new(0);
static NF_TX_LAST: AtomicU64 = AtomicU64::new(0);
static NF_ARM_AT: AtomicU64 = AtomicU64::new(0);
static NF_KICK_AT: AtomicU64 = AtomicU64::new(0);
static NF_KICKS: AtomicU64 = AtomicU64::new(0);
static NF_LAST_ETYPE: AtomicU16 = AtomicU16::new(0);

/// M2: `[usbnet] rx0 len=<n> hdr=<trailer> pkt_cnt=<n> hdr_off=<n> bytes=<first 48> tail=<last 16>`, once per boot.
fn netframe_rx0(buf: &[u8]) {
    if NF_RX0.swap(true, Ordering::Relaxed) { return; }
    let n = buf.len();
    let (h, cnt, off) = usbnet_core::ax88179::trailer(buf).unwrap_or((0, 0, 0));
    serial_println!(
        "[usbnet] rx0 len={} hdr={:#010x} pkt_cnt={} hdr_off={} bytes={} tail={}",
        n, h, cnt, off, HexRun(&buf[..n.min(48)]), HexRun(&buf[n.saturating_sub(16)..])
    );
}
/// Bytes as one unbroken lowercase hex run (no `0x`, no separators — not a sha-length word in practice, and greppable).
struct HexRun<'a>(&'a [u8]);
impl core::fmt::Display for HexRun<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for b in self.0 { write!(f, "{:02x}", b)?; }
        Ok(())
    }
}
fn netframe_note_xfer(n: usize) {
    let now = crate::arch::ms().max(1);
    NF_RX_LAST.store(now, Ordering::Relaxed);
    if NF_STALLED.swap(false, Ordering::Relaxed) {
        NF_RESUMED.store(true, Ordering::Relaxed);
        serial_println!("[usbnet] rx resumed len={} kicks={} resets={}", n, NF_KICKS.load(Ordering::Relaxed), NF_RESETS.load(Ordering::Relaxed));
    }
    let i = NF_N.fetch_add(1, Ordering::Relaxed) as usize;
    if i < NF_LOG {
        NF_LENS[i].store(n as u32, Ordering::Relaxed);
        NF_MS[i].store(now, Ordering::Relaxed);
    }
}
/// M3: the data pass asks this when the IN TD is still outstanding. 0 = nothing; 1 = rung one, ring the IN doorbell
/// again (the TD has waited `NF_KICK_MS` since its arm); 2 = rung two, the kick drew no completion within a further
/// `NF_KICK_MS`: Stop Endpoint + Set TR Dequeue Pointer on the IN endpoint, then re-arm (`usbnet_rx_reset`).
pub fn rx_stall_action() -> u8 {
    if !ARMED.load(Ordering::Relaxed) || DONE.load(Ordering::Relaxed) || (kind() == KIND_AX88179 && !link_up()) {
        return 0;
    }
    let now = crate::arch::ms();
    let arm_at = NF_ARM_AT.load(Ordering::Relaxed);
    let kick_at = NF_KICK_AT.load(Ordering::Relaxed);
    if kick_at <= arm_at {
        if now.saturating_sub(arm_at) < NF_KICK_MS { return 0; }
        NF_KICK_AT.store(now.max(arm_at + 1), Ordering::Relaxed);
        NF_STALLED.store(true, Ordering::Relaxed);
        let k = NF_KICKS.fetch_add(1, Ordering::Relaxed) + 1;
        if k <= 2 || k.is_power_of_two() {
            serial_println!("[usbnet] rx kick n={} pending_ms={} xfers={} rx_ok={} resets={}", k, now.saturating_sub(arm_at), RX_XFERS.load(Ordering::Relaxed), RX_OK.load(Ordering::Relaxed), NF_RESETS.load(Ordering::Relaxed));
        }
        return 1;
    }
    if now.saturating_sub(kick_at) < NF_KICK_MS { return 0; }
    2
}
/// Rung two's outcome, from the controller (`usbnet_rx_reset`): the stranded TD is abandoned and the next data pass
/// posts a fresh one. `[usbnet] rx reset n= stop_cc= deq_cc= in_state=` on the first two and every power of two.
pub fn note_rx_reset(stop_cc: u8, deq_cc: u8, in_state: u8) {
    let r = NF_RESETS.fetch_add(1, Ordering::Relaxed) + 1;
    if r <= 2 || r.is_power_of_two() {
        serial_println!("[usbnet] rx reset n={} stop_cc={} deq_cc={} in_state={} kicks={} xfers={} rx_ok={}", r, stop_cc, deq_cc, in_state, NF_KICKS.load(Ordering::Relaxed), RX_XFERS.load(Ordering::Relaxed), RX_OK.load(Ordering::Relaxed));
    }
    reset_arm(); // a Stopped transfer event the commands drained may have set DONE (code 26): not a frame, not an error
}
/// True once after RX completes again following a kick or a reset (the DHCP link tries re-arm on it).
pub fn take_rx_resumed() -> bool { NF_RESUMED.swap(false, Ordering::Relaxed) }
static NF_RESETS: AtomicU64 = AtomicU64::new(0);
static NF_STALLED: AtomicBool = AtomicBool::new(false);
static NF_RESUMED: AtomicBool = AtomicBool::new(false);
/// M2: `[usbnet] rxlog n=<completions> lens=[…] ms=[…] last_ms=<n> tx_last_ms=<n> now_ms=<n> kicks=<n>`.
fn netframe_rxlog() {
    let n = NF_N.load(Ordering::Relaxed);
    let k = (n as usize).min(NF_LOG);
    let mut lens = alloc::string::String::new();
    let mut ms = alloc::string::String::new();
    for i in 0..k {
        if i > 0 { lens.push(','); ms.push(','); }
        let _ = core::fmt::Write::write_fmt(&mut lens, format_args!("{}", NF_LENS[i].load(Ordering::Relaxed)));
        let _ = core::fmt::Write::write_fmt(&mut ms, format_args!("{}", NF_MS[i].load(Ordering::Relaxed)));
    }
    serial_println!(
        "[usbnet] rxlog n={} lens=[{}] ms=[{}] last_ms={} tx_last_ms={} now_ms={} kicks={} resets={} tx_stuck={}",
        n, lens, ms, NF_RX_LAST.load(Ordering::Relaxed), NF_TX_LAST.load(Ordering::Relaxed), crate::arch::ms(),
        NF_KICKS.load(Ordering::Relaxed), NF_RESETS.load(Ordering::Relaxed), TX_STUCK.load(Ordering::Relaxed)
    );
    match crate::drivers::xhci::claim() {
        Ok(x) => match x.usbnet_stall_probe() {
            Some((portsc, in_st, out_st)) => serial_println!(
                "[usbnet] stall portsc={:#010x} ccs={} ped={} pls={} speed={} in_state={} out_state={} pending={}",
                portsc, portsc & 1, (portsc >> 1) & 1, (portsc >> 5) & 0xf, (portsc >> 10) & 0xf, in_st, out_st,
                (ARMED.load(Ordering::Relaxed) && !DONE.load(Ordering::Relaxed)) as u8
            ),
            None => serial_println!("[usbnet] stall portsc=? (no slot)"),
        },
        Err(_) => serial_println!("[usbnet] stall portsc=? (controller busy)"),
    }
}

/// M3 — `tests usbnet7`: up to 10 s of the main loop's two net calls run by the fixture (the shell holds the main
/// loop): the xHCI pass (`main_pass`: events, FTDI, the full `service_usbnet`) and the stack's poll, which keeps the
/// DHCP client retransmitting. Frames go to the STACK, not the fixture. PASS = a frame arrived, an IPv4 frame among
/// them, and a DHCP server reply (UDP from port 67) or a lease.
/// `:: USBNET7: rx_ok=<n> frames=<n> first_frame_ms=<n> ethertype=0x0800 dhcp=offer -> PASS ::`; otherwise FAIL with
/// the `rxlog` + `stall` lines. No dongle / no PHY link -> SKIP.
pub fn usbnet7_selftest() {
    CENSUS.store(true, Ordering::Relaxed);
    let skip = if !is_up() { Some("no-dongle") } else if kind() == KIND_AX88179 && !link_up() { Some("no-link") } else { None };
    if let Some(r) = skip {
        serial_println!(":: USBNET7: rx_ok={} frames=0 first_frame_ms=none ethertype=none dhcp=none -> SKIP reason={} ::", RX_OK.load(Ordering::Relaxed), r);
        return;
    }
    let (f0, v40, d0) = (RX_FRAMES.load(Ordering::Relaxed), RX_V4.load(Ordering::Relaxed), RX_DHCP.load(Ordering::Relaxed));
    let t0 = crate::arch::ms();
    let mut first_ms: Option<u64> = None;
    let mut first_et = 0u16;
    while crate::arch::ms().saturating_sub(t0) < NF_WINDOW_MS {
        main_pass();
        nf_stack_poll();
        if first_ms.is_none() && RX_FRAMES.load(Ordering::Relaxed) != f0 {
            first_ms = Some(crate::arch::ms().saturating_sub(t0));
            first_et = NF_LAST_ETYPE.load(Ordering::Relaxed);
        }
        if RX_V4.load(Ordering::Relaxed) != v40 && (RX_DHCP.load(Ordering::Relaxed) != d0 || leased()) { break; }
        for _ in 0..256 { core::hint::spin_loop(); }
    }
    let frames = RX_FRAMES.load(Ordering::Relaxed) - f0;
    let v4 = RX_V4.load(Ordering::Relaxed) - v40;
    let offer = RX_DHCP.load(Ordering::Relaxed) != d0 || leased();
    let et = if v4 > 0 { alloc::string::String::from("0x0800") } else if frames > 0 { alloc::format!("{:#06x}", first_et) } else { alloc::string::String::from("none") };
    let fms = match first_ms { Some(m) => alloc::format!("{}", m), None => alloc::string::String::from("none") };
    let pass = frames > 0 && v4 > 0 && offer;
    serial_println!(
        ":: USBNET7: rx_ok={} frames={} first_frame_ms={} ethertype={} dhcp={} -> {} ::",
        RX_OK.load(Ordering::Relaxed), frames, fms, et, if offer { "offer" } else { "none" }, if pass { "PASS" } else { "FAIL" }
    );
    if !pass { netframe_rxlog(); }
}
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn nf_stack_poll() { let _ = crate::smolnet::usbnet7_poll(); }
#[cfg(not(all(feature = "smolnet", target_arch = "x86_64")))]
fn nf_stack_poll() { let mut b = [0u8; FRAME_CAP]; while raw_rx(&mut b).is_some() {} }
