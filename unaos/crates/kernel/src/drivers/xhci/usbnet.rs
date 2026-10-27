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
use crate::sync::Mutex;

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

static RXQ: MaskedRing = MaskedRing(Mutex::new(FrameRing::new()));
static TXQ: MaskedRing = MaskedRing(Mutex::new(FrameRing::new()));

/// USBNETKILLSFTDI (rmbp-ledger B486, flight 26 boot 1): the frame rings are taken by three contexts — the
/// main loop's controller pass (`deliver`/`next_tx`), the preemptible `net-tick` task (`pump_until` ->
/// `rx_ready` -> `drive`) and a ring-3 socket syscall that runs MASKED (`[net] poll enter op=sendto … masked=1`).
/// The wire's last lines were `[lock] spin-wait name=usbnet.rs:209 at=usbnet.rs:560 waiter=20@cpu2 on=untracked`
/// beside `held-too-long name=smolnet.rs:480 … by=20@cpu2`: the masked syscall spun on RXQ while the holder —
/// `net-tick`, on the same cpu2 — had been preempted inside its hold, holding the xHCI LOAN around it (`drive`).
/// A masked spin on a preemptible holder is WEDGE-8's F3 deadlock: cpu2 never ran `net-tick` again, the loan
/// never came back, and the main loop's FTDI drain (which needs the loan) stopped for good — the wire died while
/// the glass, off the xHCI, lived. Every hold of either ring is now a masked micro-hold (a memcpy of one frame),
/// so no holder can be preempted and no masked waiter can outlive it. The guard drops the lock BEFORE the mask.
struct MaskedRing(Mutex<FrameRing>);
struct MaskedGuard<'a> {
    g: crate::sync::MutexGuard<'a, FrameRing>,
    _m: crate::arch::IrqMask,
}
impl MaskedRing {
    #[track_caller]
    #[inline]
    fn lock(&self) -> MaskedGuard<'_> {
        let m = crate::arch::IrqMask::new();
        MaskedGuard { g: self.0.lock(), _m: m }
    }
}
impl core::ops::Deref for MaskedGuard<'_> {
    type Target = FrameRing;
    fn deref(&self) -> &FrameRing {
        &self.g
    }
}
impl core::ops::DerefMut for MaskedGuard<'_> {
    fn deref_mut(&mut self) -> &mut FrameRing {
        &mut self.g
    }
}

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
/// USBNET9: `slot` is the link's — taken as a candidate, configured or up (the hub witness names it `net`).
pub fn is_link_slot(slot: u8) -> bool {
    SLOT.load(Ordering::Relaxed) == slot && STATE.load(Ordering::Relaxed) != ST_ABSENT
}
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
    retry_reset(); // PUMPRETRY (B537): a new candidate starts at try 0
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
    if STATE.load(Ordering::Relaxed) != ST_CONFIGURED || crate::arch::ms() < RETRY_AT_MS.load(Ordering::Relaxed) || !crate::boot::services_gate("usbnet") || usbnet_bar_holds() {
        return None; // FIRSTLOGIN (B636): and while the desktop's bar is unbuilt (flight 34 boot 1: this bring-up's 333 ms sat inside the first login's bar_ms=341). PUMPRETRY (B537): a refused try waits out its back-off on later passes (the pump runs on). INSTALLBARE (R86): the class bring-up (the register walk, rx arm, link poll) waits for the Desktop's services; enumeration already ran
    }
    crate::boot::note_start("usbnet");
    Some(SLOT.load(Ordering::Relaxed))
}
pub fn cfg_value() -> u8 { CFG_VALUE.load(Ordering::Relaxed) } #[cfg(all(target_arch = "x86_64", feature = "wc"))] fn usbnet_bar_holds() -> bool { crate::video::desktopbuild::starter_holds("usbnet") } #[cfg(not(all(target_arch = "x86_64", feature = "wc")))] fn usbnet_bar_holds() -> bool { false } // FIRSTLOGIN (B636): the class bring-up waits (bounded) for the bar's first paint, KVMLAT2's HDA hold made one call. LINE-NEUTRAL append, code first
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
            try_line(slot, "-", "armed", 0);
            STATE.store(ST_UP, Ordering::Relaxed);
            serial_println!(
                ":: USBNET: up kind={} slot={} cfg={} ctrl={} data={} alt={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} filter={} -> armed ::",
                kind_name(), slot, CFG_VALUE.load(Ordering::Relaxed), CTRL_IFACE.load(Ordering::Relaxed),
                DATA_IFACE.load(Ordering::Relaxed), DATA_ALT.load(Ordering::Relaxed),
                m[0], m[1], m[2], m[3], m[4], m[5], if filter_ok { "ok" } else { "refused" }
            );
            #[cfg(all(target_arch = "aarch64", feature = "net6"))]
            crate::net_phy::net6::register_nic(&NET6_OPS);
        }
        Some(why) => {
            if retry_after_refusal(slot, why) {
                return; // PUMPRETRY (B537): still ST_CONFIGURED; `bringup_pending` re-offers the slot once the back-off has run out
            }
            STATE.store(ST_FAILED, Ordering::Relaxed);
            serial_println!(":: USBNET: bring-up kind={} slot={} refused at {} retries={} -> declined ::", kind_name(), slot, why, BRINGUP_RETRIES);
            serial_println!(":: USBNET: bus=xhci slot={} mac=00:00:00:00:00:00 link=down speed=0 usb=? rx=0 tx=0 refused={} -> declined ::", slot, why);
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
    retry_reset(); // PUMPRETRY (B537): a pull during a back-off forgets the pending retry
    reset_arm(); U9_CARRY.store(0, Ordering::Relaxed); // USBNET9 M2: a carried burst dies with the slot
    ud_reset(); // USBNETDHCP (B538): the client, its lease and the ping die with the slot
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
    serial_println!("[usbnet] detach slot={} was_up={} rings=emptied loan=pass -> clean", slot, was_up as u8); // USBNETKILLSFTDI (B486): the teardown ran to its end inside the controller pass; a stalled glass after a pull shows as this line missing
}

// ── PUMPRETRY (rmbp-ledger B537): a refused bring-up is RETRIED, and no wait holds the pump ────────
//
// Flight 31: the dongle's first plug (high-speed, `in_mps=512`) stopped answering EP0 at the CLK_SELECT
// write; `pump_until_ep0_done` waited its whole `hw_wait_budget()` (2 s on x86, `budget 5387711410` at
// 2.69 GHz) INSIDE the usb-pump's pass, PUMPWEDGE fired (`age_ms=2160 bound_ms=2000`), and `set_up(.., Some)`
// stored ST_FAILED — terminal until a re-plug. Now:
//   * PASS CAP. `service_usbnet` opens a `PassScope`: a deadline `hw_wait_budget()*3/5` (1.2 s x86) from the
//     pass start. Every EP0 / command wait inside it is `sync_budget(budget)` = min(budget/4, time left),
//     so one dead transfer costs <= 500 ms and the whole pass (spins included: 10+200+100 ms) ends well
//     inside PUMPWEDGE's 2 s. A transfer past the deadline fails at once and the try is refused.
//   * RETRY. A refused try (`set_up(.., Some(why))`) leaves the link ST_CONFIGURED and arms a back-off
//     (250 / 500 / 1000 ms for retries 1 / 2 / 3); `bringup_pending` re-offers the slot on the first pass
//     after it — the wait is spread over passes, never spun. Try 0 is the first bring-up; after retry 3
//     the device is declined (final). Any refusal is retried, not only a timeout: bounded, and the
//     flight-31 refusal was a timeout followed by a readback on a ring still holding the dead TD.
//   * EP0 RESET. A retry after a try that timed out first takes EP0 out of its state (Stop Endpoint when
//     Running, Reset Endpoint when Halted/Error) and moves its dequeue to our enqueue (Set TR Dequeue),
//     abandoning the stranded TD — through the driver's ONE EP0 recovery, `ep0_recover` (mod.rs file tail, PORTRESET B837).
// Wire: `[usbnet] bring-up try=<n> of 3 slot= after_ms= ep0_reset=..` at a retry's start, and one
// `[usbnet] bring-up try=<n> of 3 kind= slot= ms= ep0_timeouts= at= -> armed | retry backoff_ms= | declined(final)`
// per try.
pub const BRINGUP_RETRIES: u8 = 3;
const BACKOFF_MS: [u64; BRINGUP_RETRIES as usize] = [250, 500, 1000];
static TRY: AtomicU8 = AtomicU8::new(0);
static RETRY_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_BACKOFF_MS: AtomicU64 = AtomicU64::new(0);
static TRY_T0_MS: AtomicU64 = AtomicU64::new(0);
static EP0_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static TRY_EP0_BASE: AtomicU32 = AtomicU32::new(0);
static PREV_TRY_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
/// `now_cycles()` deadline of the open `PassScope`; 0 = no scope (every wait keeps its full budget).
static PASS_DEADLINE: AtomicU64 = AtomicU64::new(0);

fn retry_reset() {
    TRY.store(0, Ordering::Relaxed);
    RETRY_AT_MS.store(0, Ordering::Relaxed);
    LAST_BACKOFF_MS.store(0, Ordering::Relaxed);
    PREV_TRY_TIMEOUTS.store(0, Ordering::Relaxed);
}

/// `pump_until_ep0_done` timed out (mod.rs, same-line append on its TIMEOUT line).
pub fn note_ep0_timeout() {
    EP0_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
}

/// The wall-clock budget (cycles) an EP0 or command wait may spend: unchanged outside the link's pass;
/// inside it, a quarter of the base budget and never past the pass deadline (at least one cycle, so a
/// wait past the deadline fails on its first check instead of spinning).
pub fn sync_budget(budget: u64) -> u64 {
    let dl = PASS_DEADLINE.load(Ordering::Relaxed);
    if dl == 0 {
        return budget;
    }
    (budget / 4).min(dl.saturating_sub(crate::arch::now_cycles())).max(1)
}

/// The link's service pass: opened at the top of `service_usbnet`, closed when it returns (Drop).
pub struct PassScope(());
impl PassScope {
    pub fn enter() -> Self {
        let allowance = crate::arch::hw_wait_budget() / 5 * 3;
        PASS_DEADLINE.store(crate::arch::now_cycles().wrapping_add(allowance).max(1), Ordering::Relaxed);
        PassScope(())
    }
}
impl Drop for PassScope {
    fn drop(&mut self) {
        PASS_DEADLINE.store(0, Ordering::Relaxed);
    }
}

/// A try starts: returns (try number, whether the previous try timed out on EP0 and the ring wants a reset).
fn begin_try() -> (u8, bool) {
    TRY_T0_MS.store(crate::arch::ms(), Ordering::Relaxed);
    TRY_EP0_BASE.store(EP0_TIMEOUTS.load(Ordering::Relaxed), Ordering::Relaxed);
    let n = TRY.load(Ordering::Relaxed);
    (n, n > 0 && PREV_TRY_TIMEOUTS.load(Ordering::Relaxed) > 0)
}

fn try_timeouts() -> u32 {
    EP0_TIMEOUTS.load(Ordering::Relaxed).wrapping_sub(TRY_EP0_BASE.load(Ordering::Relaxed))
}

/// The one per-try verdict line.
fn try_line(slot: u8, at: &str, verdict: &str, backoff_ms: u64) {
    let ms = crate::arch::ms().saturating_sub(TRY_T0_MS.load(Ordering::Relaxed));
    if backoff_ms != 0 {
        serial_println!(
            "[usbnet] bring-up try={} of {} kind={} slot={} ms={} ep0_timeouts={} at={} -> {} backoff_ms={}",
            TRY.load(Ordering::Relaxed), BRINGUP_RETRIES, kind_name(), slot, ms, try_timeouts(), at, verdict, backoff_ms
        );
    } else {
        serial_println!(
            "[usbnet] bring-up try={} of {} kind={} slot={} ms={} ep0_timeouts={} at={} -> {}",
            TRY.load(Ordering::Relaxed), BRINGUP_RETRIES, kind_name(), slot, ms, try_timeouts(), at, verdict
        );
    }
}

/// A try was refused at `why`: arm the next retry (true) or, after the last, let the caller decline (false).
fn retry_after_refusal(slot: u8, why: &'static str) -> bool {
    let n = TRY.load(Ordering::Relaxed);
    if n >= BRINGUP_RETRIES {
        try_line(slot, why, "declined(final)", 0);
        return false;
    }
    let backoff = BACKOFF_MS[n as usize];
    try_line(slot, why, "retry", backoff);
    PREV_TRY_TIMEOUTS.store(try_timeouts(), Ordering::Relaxed);
    LAST_BACKOFF_MS.store(backoff, Ordering::Relaxed);
    RETRY_AT_MS.store(crate::arch::ms().saturating_add(backoff), Ordering::Relaxed);
    TRY.store(n + 1, Ordering::Relaxed);
    true
}

impl super::XhciController {
    /// One bring-up try, called from `service_usbnet` once `bringup_pending` offers the slot. A retry
    /// after a timed-out try resets EP0 first; the try's verdict is printed by `set_up`.
    pub(super) fn usbnet_bringup_try(&mut self, slot: u8) {
        let (n, reset) = begin_try();
        if n > 0 {
            let after = LAST_BACKOFF_MS.load(Ordering::Relaxed);
            if reset {
                let (st, stop_cc, deq_cc) = self.ep0_recover(slot);
                serial_println!(
                    "[usbnet] bring-up try={} of {} slot={} after_ms={} ep0_reset=state:{} stop_cc={} deq_cc={} now={}",
                    n, BRINGUP_RETRIES, slot, after, st, stop_cc, deq_cc, self.ep_state_of(slot, 1)
                );
            } else {
                serial_println!("[usbnet] bring-up try={} of {} slot={} after_ms={} ep0_reset=skip", n, BRINGUP_RETRIES, slot, after);
            }
        }
        self.usbnet_bringup(slot);
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
        U8_TX_RESID.store(transfer_len, Ordering::Relaxed); // USBNET8 M7: the OUT TD's residual, for `tx done`
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
    u8_pass_tick(); // USBNET8 M8: every data pass asks here; the interval between asks is the pass time
    ud_tick(); // USBNETDHCP (B538): the link's DHCP client and gateway ping, paced off the data pass (under the controller loan)
    if !DONE.load(Ordering::Relaxed) {
        return None;
    }
    DONE.store(false, Ordering::Relaxed);
    ARMED.store(false, Ordering::Relaxed);
    Some((CODE.load(Ordering::Relaxed), RESIDUE.load(Ordering::Relaxed)))
}
pub fn note_error(code: u8) {
    U9_CARRY.store(0, Ordering::Relaxed); // USBNET9 M2: an error completion ends any carried burst
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
    let queued = RXQ.lock().push(frame);
    ud_tap(frame, queued); // USBNETDHCP (B538): every frame classified on the wire, the DHCP reply / post-lease ARP + ICMP taken here, whether or not the 7-frame ring had room
    if queued {
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
    let n = TXQ.lock().pop(out);
    if let Some(k) = n { u9_tx_seen(&out[..k.min(out.len())]); } // USBNET9: a solicited frame opens the answer-owed clock; a DHCP client frame goes on the wire
    n
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
    let _mask = crate::arch::IrqMask::new(); // USBNETKILLSFTDI (B486): the stack-side loan is a MASKED hold — `net-tick` preempted inside it kept the loan (and the FTDI's drain) from the main loop for good on flight 26
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
    /// AB 0x0008 | AMALL 0x0002 — Linux `ax88179_reset`'s value exactly. DROPCRCERR does not read back (flight 23:
    /// 0x03aa/0x02aa); Linux never reads it back either.
    pub const RX_CTL_RUN: u16 = 0x03aa;
    /// USBNET8 M7: what Linux writes once the interface is open and on every `ax88179_link_reset` (`rxctl` from
    /// `ax88179_set_multicast`): START 0x0080 | AB 0x0008 | IPE 0x0200, plus AMALL 0x0002 (Linux sets AM + the hash filter
    /// for its IPv6/mDNS groups; this driver programs no hash, so all-multicast).
    /// USBNETRX (B545): no RX_CTL bit gates DIRECTED frames (DA == NODE_ID). Linux `ax88179_set_multicast` starts from
    /// `AX_RX_CTL_START | AX_RX_CTL_AB | AX_RX_CTL_IPE` and, with an empty multicast list, adds nothing — commented "just
    /// broadcast and directed"; `AX_RX_CTL_AP` 0x0020 (in `ax88179_reset`'s value, dropped here exactly as Linux drops it on
    /// open), `AX_RX_CTL_AM` 0x0010 (hash) and `AX_RX_CTL_AMALL` 0x0002 are multicast selectors, `AX_RX_CTL_PRO` 0x0001
    /// promiscuous; asix.h names the START bit `AX_RX_CTL_SO`. Unicast to us rides START + the NODE_ID match — the link-up
    /// ARP probe (`ud_probe_tick`) is what proves it on the wire.
    pub const RX_CTL_OPEN: u16 = 0x028a;
    pub const RX_CTL_STOP: u16 = 0x0000;
    /// USBNET8 M7: Linux `ax88179_reset`'s provisional medium, before any link: RECEIVE_EN 0x0100 | TXFLOW_CTRLEN 0x0020 |
    /// RXFLOW_CTRLEN 0x0010 | FULL_DUPLEX 0x0002 | GIGAMODE 0x0001 = 0x0133 (was 0x013f: + EN_125MHZ 0x0008 + 0x0004, a bit
    /// mainline defines no name for; flight 23 read back 0x0133 — neither stuck).
    pub const MEDIUM_RUN: u16 = 0x0133;
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
    /// MEDIUM_STATUS_MODE bits (Linux AX_MEDIUM_*): GIGAMODE 0x0001, FULL_DUPLEX 0x0002, EN_125MHZ 0x0008, RXFLOW_CTRLEN
    /// 0x0010, TXFLOW_CTRLEN 0x0020, RECEIVE_EN 0x0100, PS (100M) 0x0200. Bit 2 (0x0004) has no mainline name; USBNET8 drops
    /// it: `ax88179_link_reset` starts from RECEIVE_EN | TXFLOW | RXFLOW and adds the PHYSR-derived bits (1000 full = 0x013b).
    pub const MEDIUM_BASE: u16 = 0x0100 | 0x0020 | 0x0010;
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
            RX_OK.fetch_add(1, Ordering::Relaxed); rxr_frame(len);
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
fn net6_rx_ready() -> bool { rx_ready(false) } // ARMRESOLVE (B560) at the fold: the ARMNET (B346) poll-gate half NET6_OPS lacked — no aarch64 image with usbnet+net6 compiled without it
#[cfg(all(target_arch = "aarch64", feature = "net6"))]
static NET6_OPS: crate::net_phy::net6::NicOps = crate::net_phy::net6::NicOps {
    rx: raw_rx,
    tx: raw_tx,
    mac: net6_mac,
    link_up: is_up,
    rx_ready: net6_rx_ready,
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
        LINK.store(true, Ordering::Relaxed); if ARMED.load(Ordering::Relaxed) { NF_ARM_AT.store(now_ms.max(1), Ordering::Relaxed); } // USBNET8: a TD armed before the PHY had link has waited on nothing; its stall clock starts at link-up (flight 23's first kick was the bring-up TD at link-up)
        serial_println!("[usbnet] link up speed={}M duplex={} physr={:#06x} usb_plsr={:#04x} medium={:#06x}", mbps, if p & PHYSR_FULL != 0 { "full" } else { "half" }, p, plsr, m); ud_link_up(now_ms); // DHCPREPLUG (B632): a link-up restarts the link's DHCP client (fresh xid, back-off 4 s, the DISCOVER inside 1 s)
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
        0x0806 => { RX_ARP.fetch_add(1, Ordering::Relaxed); u8_arp_seen(f); } // USBNET8: the txprobe's answer
        0x86dd => { RX_V6.fetch_add(1, Ordering::Relaxed); }
        0x0800 => {
            RX_V4.fetch_add(1, Ordering::Relaxed); u8_v4_seen(f); // USBNET8: a LAN peer for the txprobe
            let ihl = ((f.get(14).copied().unwrap_or(0) & 0x0f) as usize) * 4;
            if f.len() >= 14 + ihl + 4 && f[23] == 17 && u16::from_be_bytes([f[14 + ihl], f[15 + ihl]]) == 67 { RX_DHCP.fetch_add(1, Ordering::Relaxed); u9_rx_dhcp(f, ihl); }
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
/// USBNET8 M8: frames are tapped, not consumed — they go on to the stack.
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
    // USBNET8 M8: the fixture TAPS the ring (counts frames as they are pushed, reads the newest ethertype) and lets the stack
    // pop them — it used to pop up to 4 frames itself for 5 s, so a DHCP offer arriving in that window went to the fixture.
    let mut frames = 0u32;
    let mut first: Option<u16> = None;
    let mut first_at = 0u64;
    let f0 = RX_FRAMES.load(Ordering::Relaxed);
    while crate::arch::ms().saturating_sub(t0) < 5000 {
        main_pass();
        nf_stack_poll();
        let seen = (RX_FRAMES.load(Ordering::Relaxed) - f0) as u32;
        if seen > frames {
            if frames == 0 { first_at = crate::arch::ms(); first = Some(NF_LAST_ETYPE.load(Ordering::Relaxed)); } // USBNET7: when the first frame of the run arrived
            frames = seen;
            if frames >= 4 { break; }
        }
        for _ in 0..256 { core::hint::spin_loop(); }
    }
    let dropped = RX_CHIP_DROP.load(Ordering::Relaxed) + RX_CRC.load(Ordering::Relaxed) - d0;
    let et = match first { Some(e) => alloc::format!("{:#06x}", e), None => alloc::string::String::from("none") };
    let (v, why) = if frames > 0 { ("PASS", "") } else if dropped > 0 { ("FAIL", " reason=chip-drop-see-rx_ctl") } else { ("FAIL", " reason=no-frame-5s") };
    serial_println!(
        "[usbnet] census arp={} ipv4={} ipv6={} dhcp_replies={} rx_pad={} rx_ok={} leased={}",
        RX_ARP.load(Ordering::Relaxed), RX_V4.load(Ordering::Relaxed), RX_V6.load(Ordering::Relaxed), RX_DHCP.load(Ordering::Relaxed),
        RX_PAD_HDR.load(Ordering::Relaxed), RX_OK.load(Ordering::Relaxed), leased() as u8
    );
    ud_report(); // USBNETDHCP (B538): the frame classes and the client's state
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
    // USBNET8: `first_frame_ms` is the LINK's (first arm -> first frame of the boot): a quiet LAN during the fixture's
    // window is not an RX fault (flight 23: rx_ok=59 on the ring, the window saw none). The window's own is `window_ms`.
    let boot_ms = u8_first_frame_ms();
    let (kn, rn, idle) = u8_needed();
    let pass = (matches!(ms, Some(m) if m <= 3000) || boot_ms.is_some()) && kn == 0 && rn == 0;
    let et = match first { Some(e) => alloc::format!("{:#06x}", e), None => alloc::format!("{:#06x}", FIRST_ETYPE.load(Ordering::Relaxed)) };
    let wms = match ms { Some(m) => alloc::format!("{}", m), None => alloc::string::String::from("none") };
    let fms = match boot_ms { Some(m) => alloc::format!("{}", m), None => alloc::string::String::from("none") };
    serial_println!(":: USBNET7: rx_ok={} first_frame_ms={} kicks={} resets={} idle_rungs={} ethertype={} window_ms={} -> {} ::", RX_OK.load(Ordering::Relaxed), fms, kn, rn, idle, et, wms, if pass { "PASS" } else { "FAIL" });
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
        u8_tx_done(code); // USBNET8 M7: `[usbnet] tx done` for the first frames and every non-success
        return Some(code);
    }
    let age_us = crate::arch::now_cycles().wrapping_sub(TX_T0.load(Ordering::Relaxed)) / cycles_per_us();
    if age_us >= TX_STUCK_MS * 1000 {
        if TX_STUCK.fetch_add(1, Ordering::Relaxed) == 0 {
            serial_println!("[usbnet] tx stuck dci={} trb={:#x} age_ms={} — abandoned, TX continues", TX_DCI.load(Ordering::Relaxed), TX_TRB.load(Ordering::Relaxed), age_us / 1000);
        }
        tx_reset();
        u8_tx_done(0); // USBNET8 M7: an abandoned TD is a non-success on the same line
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
    U9_OWED_AT.store(0, Ordering::Relaxed); // USBNET9: any completion proves the ring alive — nothing is owed now
    if NF_STALLED.swap(false, Ordering::Relaxed) {
        NF_RESUMED.store(true, Ordering::Relaxed);
        let (after, needed) = u8_answered(now); // USBNET8: a completion within U8_NEEDED_MS of the rung = the rung was needed
        if needed && U8_RUNG_KIND.load(Ordering::Relaxed) == 2 { U9_BACKOFF_MS.store(U9_BACKOFF_MIN_MS, Ordering::Relaxed); } // USBNET9: a reset that was needed earns the short back-off back
        let gap = U8_PASS_GAP_MAX.swap(0, Ordering::Relaxed); // the longest data-pass interval since the rung: over 50 ms, `needed` can undercount
        let pass = if after > U8_NEEDED_MS && gap > U8_NEEDED_MS { alloc::format!(" pass_ms={}", gap) } else { alloc::string::String::new() };
        serial_println!("[usbnet] rx resumed len={} kicks={} resets={} after_ms={} needed={}{}", n, NF_KICKS.load(Ordering::Relaxed), NF_RESETS.load(Ordering::Relaxed), after, needed as u8, pass);
    }
    let i = NF_N.fetch_add(1, Ordering::Relaxed) as usize;
    if i < NF_LOG {
        NF_LENS[i].store(n as u32, Ordering::Relaxed);
        NF_MS[i].store(now, Ordering::Relaxed);
    }
}
/// M3: the data pass asks this when the IN TD is still outstanding. 0 = nothing; 1 = rung one, ring the IN doorbell
/// again; 2 = rung two, Stop Endpoint + Set TR Dequeue Pointer on the IN endpoint, then re-arm (`usbnet_rx_reset`).
/// USBNET9 (B385): rung one keeps its cadence (a doorbell on a Running endpoint is harmless, xHCI 1.2 §4.7, and it is
/// USBNET8's `needed=` measurement): `NF_KICK_MS` after the arm, and again `NF_KICK_MS` after an answer became owed.
/// Rung two fires ONLY when a frame is expected — an answer is owed (`u9_owed_ms`: a solicited TX issued after the last
/// RX completion, at least `U9_OWED_MS` ago), a kick fired after the owe began and drew nothing for `NF_KICK_MS`, and
/// the back-off since the last reset has passed. On a quiet link nothing is owed: no reset, ever (flight 24/25: a reset
/// every ~4 s on an idle LAN, `needed=0` on nearly every resume).
pub fn rx_stall_action() -> u8 {
    if !ARMED.load(Ordering::Relaxed) || DONE.load(Ordering::Relaxed) || (kind() == KIND_AX88179 && !link_up()) {
        return 0;
    }
    let now = crate::arch::ms();
    let arm_at = NF_ARM_AT.load(Ordering::Relaxed);
    let kick_at = NF_KICK_AT.load(Ordering::Relaxed);
    let owed_at = U9_OWED_AT.load(Ordering::Relaxed);
    let owed_ms = if owed_at == 0 { 0 } else { now.saturating_sub(owed_at) };
    let first = kick_at <= arm_at;
    if first || (owed_at != 0 && kick_at < owed_at) {
        let since = if first { now.saturating_sub(arm_at) } else { owed_ms };
        if since < NF_KICK_MS { return 0; }
        NF_KICK_AT.store(now.max(arm_at + 1).max(owed_at), Ordering::Relaxed);
        NF_STALLED.store(true, Ordering::Relaxed);
        let k = NF_KICKS.fetch_add(1, Ordering::Relaxed) + 1; u8_rung(1, now); // USBNET8: when, and which rung
        if k <= 2 || k.is_power_of_two() {
            serial_println!("[usbnet] rx kick n={} pending_ms={} owed_ms={} xfers={} rx_ok={} resets={}", k, now.saturating_sub(arm_at), owed_ms, RX_XFERS.load(Ordering::Relaxed), RX_OK.load(Ordering::Relaxed), NF_RESETS.load(Ordering::Relaxed));
        }
        return 1;
    }
    if now.saturating_sub(kick_at) < NF_KICK_MS { return 0; }
    if owed_at == 0 || owed_ms < U9_OWED_MS {
        u9_idle(now, owed_at != 0);
        return 0;
    }
    let last = U9_RESET_AT.load(Ordering::Relaxed);
    let backoff = U9_BACKOFF_MS.load(Ordering::Relaxed).max(U9_BACKOFF_MIN_MS);
    if last != 0 && now.saturating_sub(last) < backoff { return 0; }
    U9_RESET_AT.store(now.max(1), Ordering::Relaxed);
    U9_BACKOFF_MS.store(rxr_backoff((backoff * 2).min(U9_BACKOFF_MAX_MS)), Ordering::Relaxed); // USBNETRX M2: held at the floor while no IN completion ever arrived
    U9_RESET_OWED_MS.store(owed_ms, Ordering::Relaxed);
    2
}
/// Rung two's outcome, from the controller (`usbnet_rx_reset`), after the Stop and the dequeue move. USBNET9 M2: a reset
/// loses nothing it can keep — a real completion (cc 1 Success / 13 Short Packet) that raced the Stop stays DONE and the
/// next data pass delivers it; a Stopped TD (cc 26) whose bytes are a whole number of max packets is CARRIED: the next
/// arm posts its TD at the offset after those bytes, so the AX88179 burst completes contiguously and is delivered whole.
/// `[usbnet] rx reset n= stop_cc= deq_cc= in_state= … owed_ms= kept=` on the first two and every power of two.
pub fn note_rx_reset(stop_cc: u8, deq_cc: u8, in_state: u8) {
    let r = NF_RESETS.fetch_add(1, Ordering::Relaxed) + 1; u8_rung(2, crate::arch::ms()); // USBNET8: when, and which rung
    let kept = u9_keep();
    if r <= 2 || r.is_power_of_two() {
        serial_println!("[usbnet] rx reset n={} stop_cc={} deq_cc={} in_state={} kicks={} xfers={} rx_ok={} owed_ms={} backoff_ms={} kept={}", r, stop_cc, deq_cc, in_state, NF_KICKS.load(Ordering::Relaxed), RX_XFERS.load(Ordering::Relaxed), RX_OK.load(Ordering::Relaxed), U9_RESET_OWED_MS.load(Ordering::Relaxed), U9_BACKOFF_MS.load(Ordering::Relaxed), kept);
    }
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
    let probe = u8_txprobe_send(t0); // USBNET8 M5: does our TX reach the LAN at all? (an ARP probe for a peer we have heard)
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
        ":: USBNET7: rx_ok={} frames={} first_frame_ms={} ethertype={} dhcp={} kicks={} resets={} idle_rungs={} -> {} ::",
        RX_OK.load(Ordering::Relaxed), frames, fms, et, if offer { "offer" } else { "none" }, u8_needed().0, u8_needed().1, u8_needed().2, if pass { "PASS" } else { "FAIL" }
    );
    u8_txprobe_report(probe);
    if !pass { netframe_rxlog(); }
}
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn nf_stack_poll() { let _ = crate::smolnet::usbnet7_poll(); }
#[cfg(not(all(feature = "smolnet", target_arch = "x86_64")))]
fn nf_stack_poll() { let mut b = [0u8; FRAME_CAP]; while raw_rx(&mut b).is_some() {} }

// ── USBNET8 (rmbp-ledger B381) ──────────────────────────────────────────────────────────────────────
// Flight 23 read: 43 of 52 bulk-IN completions arrived with no rung before them; the NETFRAME ladder fired on every 2 s of
// silence (82 kicks, 74 resets in six minutes). These make the next flight decide instead of the seat guessing: the arm's
// own TRB on the wire (M2), and whether each rung was NEEDED — a completion within `U8_NEEDED_MS` of the rung means the
// data was already there and the controller had not been told (a missed doorbell); later means the wire was quiet (M3).
static U8_ARMS: AtomicU64 = AtomicU64::new(0);
/// One bulk-IN arm: `[usbnet] rx arm trb=<idx> cycle=<c> buf=<pa> len=<n> cross64k=<0|1>` at arms 1, 2 and every power of
/// two. `cross64k` is xHCI 1.2 §4.11.7.1's test on the ONE Normal TRB the arm posts (the buffer is 64 KiB aligned: 0).
pub fn note_arm_trb(trb_phys: u64, ring_base: u64, cycle: bool, buf: u64, len: usize) {
    let n = U8_ARMS.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 { U8_FIRST_ARM_AT.store(crate::arch::ms().max(1), Ordering::Relaxed); }
    if n <= 2 || n.is_power_of_two() {
        let cross = len != 0 && (buf >> 16) != ((buf + len as u64 - 1) >> 16);
        serial_println!("[usbnet] rx arm trb={} cycle={} buf={:#x} len={} cross64k={} n={}",
            trb_phys.wrapping_sub(ring_base) / 16, cycle as u8, buf, len, cross as u8, n);
    }
}
/// A completion within this long of a ladder rung means the data was already waiting: the rung was NEEDED.
const U8_NEEDED_MS: u64 = 50;
static U8_FIRST_ARM_AT: AtomicU64 = AtomicU64::new(0);
static U8_RUNG_AT: AtomicU64 = AtomicU64::new(0);
static U8_RUNG_KIND: AtomicU8 = AtomicU8::new(0);
static U8_NEEDED_KICKS: AtomicU64 = AtomicU64::new(0);
static U8_NEEDED_RESETS: AtomicU64 = AtomicU64::new(0);
/// A ladder rung fired (1 = doorbell kick, 2 = Stop Endpoint + Set TR Dequeue).
fn u8_rung(kind: u8, now: u64) {
    U8_PASS_GAP_MAX.store(0, Ordering::Relaxed);
    U8_RUNG_AT.store(now.max(1), Ordering::Relaxed);
    U8_RUNG_KIND.store(kind, Ordering::Relaxed);
}
/// The first completion after a stall: ms since the last rung, and whether that rung was needed (counted per kind).
fn u8_answered(now: u64) -> (u64, bool) {
    let at = U8_RUNG_AT.load(Ordering::Relaxed);
    if at == 0 { return (0, false); }
    let after = now.saturating_sub(at);
    let needed = after <= U8_NEEDED_MS;
    if needed {
        if U8_RUNG_KIND.load(Ordering::Relaxed) == 2 { U8_NEEDED_RESETS.fetch_add(1, Ordering::Relaxed); } else { U8_NEEDED_KICKS.fetch_add(1, Ordering::Relaxed); }
    }
    (after, needed)
}
/// (needed kicks, needed resets, rungs that fired on a quiet wire).
fn u8_needed() -> (u64, u64, u64) {
    let (k, r) = (U8_NEEDED_KICKS.load(Ordering::Relaxed), U8_NEEDED_RESETS.load(Ordering::Relaxed));
    let all = NF_KICKS.load(Ordering::Relaxed) + NF_RESETS.load(Ordering::Relaxed);
    (k, r, all.saturating_sub(k + r))
}
/// The link's first frame, in ms after the first bulk-IN arm (None before one).
fn u8_first_frame_ms() -> Option<u64> {
    let (a, f) = (U8_FIRST_ARM_AT.load(Ordering::Relaxed), FIRST_FRAME_AT.load(Ordering::Relaxed));
    if a == 0 || f == 0 { None } else { Some(f.saturating_sub(a)) }
}

// ── USBNET8 M5: the TX question ─────────────────────────────────────────────────────────────────────
// Flight 23: 33 IPv4 frames reached the ring, not one from UDP port 67, across six minutes of DISCOVERs whose OUT TDs all
// completed (tx=58). Nothing we ever sent has drawn an answer, so nothing says our frames reach the LAN. Under
// `tests usbnet7` only (R80): one ARP probe (RFC 5227: sender IP 0.0.0.0, so no peer's cache is touched) for the newest
// IPv4 source the census heard, and whether that peer answers our MAC.
static U8_PEER_V4: AtomicU32 = AtomicU32::new(0);
static U8_PROBE_IP: AtomicU32 = AtomicU32::new(0);
static U8_PROBE_AT: AtomicU64 = AtomicU64::new(0);
static U8_PROBE_REPLY_AT: AtomicU64 = AtomicU64::new(0);
/// An IPv4 frame on the ring: remember its source (not 0.0.0.0, not a multicast/broadcast source, not our own MAC's).
fn u8_v4_seen(f: &[u8]) {
    if f.len() < 34 || f[6..12] == mac() { return; }
    let ip = u32::from_be_bytes([f[26], f[27], f[28], f[29]]);
    if ip != 0 && f[26] < 224 && ip != u32::MAX { U8_PEER_V4.store(ip, Ordering::Relaxed); }
}
/// An ARP frame on the ring: a reply (op 2) from the probed address to our MAC answers the probe.
fn u8_arp_seen(f: &[u8]) {
    let want = U8_PROBE_IP.load(Ordering::Relaxed);
    if want == 0 || f.len() < 42 || u16::from_be_bytes([f[20], f[21]]) != 2 { return; }
    if u32::from_be_bytes([f[28], f[29], f[30], f[31]]) == want && f[32..38] == mac() {
        let _ = U8_PROBE_REPLY_AT.compare_exchange(0, crate::arch::ms().max(1), Ordering::Relaxed, Ordering::Relaxed);
    }
}
/// Queue the probe (the data pass issues it). Returns the target, 0 when no IPv4 peer has been heard yet.
fn u8_txprobe_send(now: u64) -> u32 {
    let ip = U8_PEER_V4.load(Ordering::Relaxed);
    U8_PROBE_REPLY_AT.store(0, Ordering::Relaxed);
    U8_PROBE_IP.store(ip, Ordering::Relaxed);
    if ip == 0 { return 0; }
    let fr = u8_probe_frame(mac(), ip);
    U8_PROBE_AT.store(now.max(1), Ordering::Relaxed);
    raw_tx(&fr);
    ip
}
/// The RFC 5227 ARP probe for `ip` from MAC `m` (sender IP 0.0.0.0): the owner's reply is addressed to `m` —
/// a frame UNICAST to our station address. Shared by the USBNET7 fixture and USBNETRX's link-up probe.
fn u8_probe_frame(m: [u8; 6], ip: u32) -> [u8; 60] {
    let mut fr = [0u8; 60]; // 42 bytes of ARP, zero-padded to the Ethernet minimum
    fr[0..6].copy_from_slice(&[0xff; 6]);
    fr[6..12].copy_from_slice(&m);
    fr[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
    fr[14..22].copy_from_slice(&[0x00, 0x01, 0x08, 0x00, 6, 4, 0x00, 0x01]); // Ethernet, IPv4, request
    fr[22..28].copy_from_slice(&m); // sender MAC; sender IP 28..32 stays 0.0.0.0 (a probe)
    fr[38..42].copy_from_slice(&ip.to_be_bytes()); // target IP; target MAC 32..38 stays zero
    fr
}
/// `[usbnet] txprobe arp who_has=<ip> sent=<0|1> reply_ms=<n|none> tx=<completions>`.
fn u8_txprobe_report(ip: u32) {
    let o = ip.to_be_bytes();
    let (at, rep) = (U8_PROBE_AT.load(Ordering::Relaxed), U8_PROBE_REPLY_AT.load(Ordering::Relaxed));
    let rms = if ip != 0 && rep != 0 { alloc::format!("{}", rep.saturating_sub(at)) } else { alloc::string::String::from("none") };
    serial_println!("[usbnet] txprobe arp who_has={}.{}.{}.{} sent={} reply_ms={} tx={}", o[0], o[1], o[2], o[3], (ip != 0) as u8, rms, TX_FRAMES.load(Ordering::Relaxed));
    U8_PROBE_IP.store(0, Ordering::Relaxed);
}

// ── USBNET8 M7: TX on the wire ──────────────────────────────────────────────────────────────────────
static U8_TX_RESID: AtomicU32 = AtomicU32::new(0);
static U8_TX_LEN: AtomicU32 = AtomicU32::new(0);
static U8_TX_N: AtomicU64 = AtomicU64::new(0);
/// The data pass issued an OUT TD of `n` bytes (header + frame + any pad byte).
pub fn u8_tx_len(n: u32) { U8_TX_LEN.store(n, Ordering::Relaxed); U8_TX_RESID.store(0, Ordering::Relaxed); }
/// `[usbnet] tx done n=<k> len=<bytes> cc=<code> residual=<bytes>` for the first three TDs and every non-success (cc not
/// 1 Success / 13 Short Packet; 0 = abandoned by the stuck valve). Success with residual 0 = the controller moved every
/// byte to the dongle; whether the dongle put it on the LAN is the txprobe's question.
fn u8_tx_done(code: u8) {
    let k = U8_TX_N.fetch_add(1, Ordering::Relaxed) + 1;
    if k <= 3 || (code != 1 && code != 13) {
        serial_println!("[usbnet] tx done n={} len={} cc={} residual={}", k, U8_TX_LEN.load(Ordering::Relaxed), code, U8_TX_RESID.load(Ordering::Relaxed));
    }
}
/// The link-up register sequence's outcome (`usbnet_ax_link_reset`, Linux `ax88179_link_reset` order).
pub fn u8_note_link_reset(rxctl_rb: u16, fifo: Option<u32>, tries: u32, ms: u64, medium: u16, ok: bool) {
    let f = match fifo { Some(v) => alloc::format!("{:#010x}", v), None => alloc::string::String::from("refused") };
    serial_println!("[usbnet] link reset order=linux rxctl={:#06x}/{:#06x} txfifo={} tries={} ms={} medium={:#06x} ok={}",
        ax::RX_CTL_OPEN, rxctl_rb, f, tries, ms, medium, ok as u8);
}
static U8_PASS_LAST: AtomicU64 = AtomicU64::new(0);
static U8_PASS_GAP_MAX: AtomicU64 = AtomicU64::new(0);
/// One data pass (from `take_done`): the longest interval between two passes since the last rung, in ms.
fn u8_pass_tick() {
    let now = crate::arch::ms();
    let last = U8_PASS_LAST.swap(now, Ordering::Relaxed);
    if last != 0 { U8_PASS_GAP_MAX.fetch_max(now.saturating_sub(last), Ordering::Relaxed); }
}

// ── USBNET9 (rmbp-ledger B385) ──────────────────────────────────────────────────────────────────────
// Flights 24/25: the NETFRAME ladder reset the IN endpoint every ~4 s on a quiet LAN (`needed=0` on nearly every
// resume), and each reset discarded whatever the Stop caught. M1: rung two needs an ANSWER OWED — the AX88179 has no
// RX-FIFO-level register in Linux `ax88179_178a.c` (R83: none invented), so "a frame is expected" is a solicited TX
// (ARP request, DHCP to 67, DNS, NTP, ICMP echo, TCP SYN) issued after the last RX completion. M2: what a reset catches
// is kept (`u9_keep`). M3: the DHCP client's frames on the wire, checksums verified here, and the first server reply.
/// An answer owed this long (with a kick that drew nothing) is a stall worth rung two.
const U9_OWED_MS: u64 = 2_000;
/// Rung two's back-off: from this, doubling per reset that was not needed, to the cap.
const U9_BACKOFF_MIN_MS: u64 = 8_000;
const U9_BACKOFF_MAX_MS: u64 = 256_000;
/// `[usbnet] rx idle …` at most this often.
const U9_IDLE_EVERY_MS: u64 = 60_000;
static U9_OWED_AT: AtomicU64 = AtomicU64::new(0);
static U9_RESET_AT: AtomicU64 = AtomicU64::new(0);
static U9_BACKOFF_MS: AtomicU64 = AtomicU64::new(U9_BACKOFF_MIN_MS);
static U9_RESET_OWED_MS: AtomicU64 = AtomicU64::new(0);
static U9_IDLE_AT: AtomicU64 = AtomicU64::new(0);
static U9_SOLICITED: AtomicU64 = AtomicU64::new(0);
/// The byte count a Stopped TD left in the RX buffer; the next arm continues after it (M2).
static U9_CARRY: AtomicU32 = AtomicU32::new(0);
/// The TD length the current arm posted (rx_len minus the carry it continues).
static U9_TD_LEN: AtomicU32 = AtomicU32::new(0);
static U9_KEPT: AtomicU64 = AtomicU64::new(0);
static U9_DHCP_TX: AtomicU64 = AtomicU64::new(0);
static U9_DHCP_RX: AtomicBool = AtomicBool::new(false);

/// M1: the quiet-link line, once a minute at most: the IN TD is pending, nothing is owed (or not yet long enough), no reset.
fn u9_idle(now: u64, owed: bool) {
    let last_rx = NF_RX_LAST.load(Ordering::Relaxed).max(NF_ARM_AT.load(Ordering::Relaxed));
    let quiet = now.saturating_sub(last_rx);
    if quiet < 10_000 { return; }
    let at = U9_IDLE_AT.load(Ordering::Relaxed);
    if at != 0 && now.saturating_sub(at) < U9_IDLE_EVERY_MS { return; }
    U9_IDLE_AT.store(now.max(1), Ordering::Relaxed);
    serial_println!("[usbnet] rx idle quiet_s={} pending=1 owed={} solicited={} kicks={} resets={}",
        quiet / 1000, owed as u8, U9_SOLICITED.load(Ordering::Relaxed), NF_KICKS.load(Ordering::Relaxed), NF_RESETS.load(Ordering::Relaxed));
}

/// M2: the controller's IN TD for the next arm — (byte offset into the RX buffer, length). After a carried Stop the TD
/// continues the burst at the offset where the stopped one ended.
pub fn rx_td() -> (usize, usize) {
    let rxl = rx_len();
    let c = (U9_CARRY.load(Ordering::Relaxed) as usize).min(rxl);
    let len = rxl - c;
    U9_TD_LEN.store(len as u32, Ordering::Relaxed);
    (c, len)
}
/// M2: the bytes a completion leaves in the RX buffer from its start: the carry plus what this TD moved. Clears the carry.
pub fn rx_take_len(residue: u32) -> usize {
    let c = U9_CARRY.swap(0, Ordering::Relaxed) as usize;
    let td = match U9_TD_LEN.load(Ordering::Relaxed) as usize { 0 => rx_len() - c.min(rx_len()), v => v };
    (c + td.saturating_sub(residue as usize)).min(rx_len())
}
/// M2: after rung two's commands drained the Stopped transfer event. Returns what was kept, for the reset line.
fn u9_keep() -> alloc::string::String {
    if !DONE.load(Ordering::Relaxed) {
        reset_arm(); // the TD was never touched (or the event did not come): nothing to keep
        return alloc::string::String::from("none");
    }
    let code = CODE.load(Ordering::Relaxed);
    let residue = RESIDUE.load(Ordering::Relaxed) as usize;
    if code == 1 || code == 13 {
        U9_KEPT.fetch_add(1, Ordering::Relaxed);
        return alloc::string::String::from("complete"); // DONE stays: the next data pass delivers it like any completion
    }
    let carry = U9_CARRY.load(Ordering::Relaxed) as usize;
    let td = U9_TD_LEN.load(Ordering::Relaxed) as usize;
    let moved = td.saturating_sub(residue);
    let mps = (IN_MPS.load(Ordering::Relaxed) as usize).max(1);
    reset_arm();
    if code == 26 && moved > 0 && moved < td && moved % mps == 0 {
        U9_CARRY.store((carry + moved) as u32, Ordering::Relaxed);
        U9_KEPT.fetch_add(1, Ordering::Relaxed);
        return alloc::format!("carry:{}", carry + moved);
    }
    U9_CARRY.store(0, Ordering::Relaxed);
    if moved > 0 || carry > 0 { alloc::format!("lost:{}:cc{}", carry + moved, code) } else { alloc::string::String::from("none") }
}

/// M1 + M3: every frame the data pass issues. Solicited frames open the answer-owed clock; DHCP client frames are dumped.
fn u9_tx_seen(f: &[u8]) {
    if f.len() < 14 { return; }
    let et = u16::from_be_bytes([f[12], f[13]]);
    let solicited = match et {
        0x0806 => f.len() >= 22 && u16::from_be_bytes([f[20], f[21]]) == 1, // ARP request
        0x0800 if f.len() >= 34 => {
            let ihl = ((f[14] & 0x0f) as usize) * 4;
            let l4 = 14 + ihl;
            match f[23] {
                1 => f.len() > l4 && f[l4] == 8, // ICMP echo request
                17 if f.len() >= l4 + 8 => {
                    let dport = u16::from_be_bytes([f[l4 + 2], f[l4 + 3]]);
                    if dport == 67 { u9_dhcp_dump(f, ihl); }
                    dport == 67 || dport == 53 || dport == 123
                }
                6 if f.len() >= l4 + 14 => f[l4 + 13] & 0x12 == 0x02, // TCP SYN (not SYN-ACK)
                _ => false,
            }
        }
        _ => false,
    };
    if solicited {
        U9_SOLICITED.fetch_add(1, Ordering::Relaxed);
        let _ = U9_OWED_AT.compare_exchange(0, crate::arch::ms().max(1), Ordering::Relaxed, Ordering::Relaxed);
    }
}
/// RFC 1071 one's-complement sum, folded.
fn u9_sum(mut acc: u32, b: &[u8]) -> u32 {
    let mut i = 0;
    while i + 1 < b.len() { acc += u16::from_be_bytes([b[i], b[i + 1]]) as u32; i += 2; }
    if i < b.len() { acc += (b[i] as u32) << 8; }
    while acc > 0xffff { acc = (acc & 0xffff) + (acc >> 16); }
    acc
}
/// The DHCP option 53 (message type) of a BOOTP payload, and its options run (after the magic cookie).
fn u9_dhcp_type(bootp: &[u8]) -> (u8, &[u8]) {
    if bootp.len() < 240 || bootp[236..240] != [0x63, 0x82, 0x53, 0x63] { return (0, &[]); }
    let opts = &bootp[240..];
    let mut i = 0;
    let mut ty = 0u8;
    while i < opts.len() {
        match opts[i] {
            0 => { i += 1; continue; }
            255 => { i += 1; break; }
            code => {
                if i + 1 >= opts.len() { break; }
                let l = opts[i + 1] as usize;
                if code == 53 && l >= 1 && i + 2 < opts.len() { ty = opts[i + 2]; }
                i += 2 + l;
            }
        }
    }
    (ty, &opts[..i.min(opts.len())])
}
/// M3: `[usbnet] tx dhcp <discover|request|type=N> xid= len= ipsum= udpsum= flags= bytes=<first 64 of the frame> opts=<options>`,
/// the first two and every power of two — the next flight lays it beside RFC 2131 §4.1 / §4.3.1.
fn u9_dhcp_dump(f: &[u8], ihl: usize) {
    let n = U9_DHCP_TX.fetch_add(1, Ordering::Relaxed) + 1;
    if !(n <= 2 || n.is_power_of_two()) { return; }
    let l4 = 14 + ihl;
    let ipsum = if u9_sum(0, &f[14..l4]) == 0xffff { "ok" } else { "bad" };
    let ulen = u16::from_be_bytes([f[l4 + 4], f[l4 + 5]]) as usize;
    let udpsum = if u16::from_be_bytes([f[l4 + 6], f[l4 + 7]]) == 0 {
        "none"
    } else if l4 + ulen <= f.len() {
        let mut acc = u9_sum(0, &f[26..34]); // source + destination address
        acc += 17 + ulen as u32;
        if u9_sum(acc, &f[l4..l4 + ulen]) == 0xffff { "ok" } else { "bad" }
    } else {
        "short"
    };
    let bootp = &f[(l4 + 8).min(f.len())..];
    let (ty, opts) = u9_dhcp_type(bootp);
    let name = match ty { 1 => "discover", 3 => "request", _ => "other" };
    let xid = if bootp.len() >= 8 { u32::from_be_bytes([bootp[4], bootp[5], bootp[6], bootp[7]]) } else { 0 };
    let flags = if bootp.len() >= 12 { u16::from_be_bytes([bootp[10], bootp[11]]) } else { 0 };
    serial_println!("[usbnet] tx dhcp {} n={} type={} xid={:08x} len={} ipsum={} udpsum={} flags={:04x} bytes={} opts={}",
        name, n, ty, xid, f.len(), ipsum, udpsum, flags, HexRun(&f[..f.len().min(64)]), HexRun(&opts[..opts.len().min(96)]));
}
/// M3: the first DHCP server reply that reaches the ring: `[usbnet] rx dhcp from=<ip> type=<2 offer|5 ack|6 nak> xid= len=`.
fn u9_rx_dhcp(f: &[u8], ihl: usize) {
    if U9_DHCP_RX.swap(true, Ordering::Relaxed) || f.len() < 34 { return; }
    let bootp = &f[(14 + ihl + 8).min(f.len())..];
    let (ty, _) = u9_dhcp_type(bootp);
    let xid = if bootp.len() >= 8 { u32::from_be_bytes([bootp[4], bootp[5], bootp[6], bootp[7]]) } else { 0 };
    serial_println!("[usbnet] rx dhcp from={}.{}.{}.{} type={} xid={:08x} len={}", f[26], f[27], f[28], f[29], ty, xid, f.len());
}

// ── USBNETKILLSFTDI (rmbp-ledger B486) — TAIL-APPENDED ──────────────────────────────────────────────
// Flight 26 boot 1: the FTDI went silent at the dongle's eighth RX arm (`rx arm trb=7 … n=8`) + the first DHCP
// discover, and never spoke again. The FTDI's heartbeat is its completed bulk-OUT transfers (`drain_ftdi`'s
// success arm calls `ftdi_beat`); the first beat after the burst depth says the console survived the burst.

/// The RX-arm depth at which flight 26's wire died.
const FTDI_BURST_ARMS: u64 = 8;
/// FTDI bulk-OUT completions this boot (every success, any link state).
static FTDI_BEATS: AtomicU64 = AtomicU64::new(0);
static FTDI_ALIVE_SAID: AtomicBool = AtomicBool::new(false);

/// One FTDI bulk-OUT completed (the controller's drain, under the loan). Prints `[ftdi] alive after usbnet arm
/// rx=<n>` ONCE: the first beat after the link's RX arms passed the burst depth.
pub fn ftdi_beat() {
    FTDI_BEATS.fetch_add(1, Ordering::Relaxed);
    let arms = RX_ARMS.load(Ordering::Relaxed);
    if arms >= FTDI_BURST_ARMS && !FTDI_ALIVE_SAID.swap(true, Ordering::Relaxed) {
        serial_println!("[ftdi] alive after usbnet arm rx={} beats={}", arms, FTDI_BEATS.load(Ordering::Relaxed));
    }
}

/// `tests usbnetftdi`: with the dongle up, drive the link for 5 s from BOTH sides the flight-26 deadlock needed —
/// the controller pass (`main_pass`) and a MASKED stack-side drive (`rx_ready(true)` under an `IrqMask`, the
/// shape of a ring-3 socket syscall) — and read the FTDI's heartbeat over the same window.
/// `:: USBNETFTDI: ftdi_alive=<0|1> rx_armed=<0|1> events=<n> beats=<n> drives=<n> -> PASS|FAIL ::`;
/// no dongle -> `SKIP reason=no-dongle`, no FTDI console -> `SKIP reason=no-ftdi`.
pub fn usbnetftdi_selftest() {
    if !is_up() {
        serial_println!(":: USBNETFTDI: ftdi_alive=0 rx_armed=0 events=0 reason=no-dongle -> SKIP ::");
        return;
    }
    if !super::ftdi::is_live() {
        serial_println!(":: USBNETFTDI: ftdi_alive=0 rx_armed=0 events=0 reason=no-ftdi -> SKIP ::");
        return;
    }
    let (b0, x0, d0, a0) = (FTDI_BEATS.load(Ordering::Relaxed), RX_XFERS.load(Ordering::Relaxed), DRIVES.load(Ordering::Relaxed), RX_ARMS.load(Ordering::Relaxed));
    let t0 = crate::arch::ms();
    let mut tick = 0u64;
    while crate::arch::ms().saturating_sub(t0) < 5000 && is_up() {
        main_pass();
        {
            let _m = crate::arch::IrqMask::new();
            let _ = rx_ready(true);
        }
        let s = crate::arch::ms().saturating_sub(t0) / 1000;
        if s > tick {
            tick = s;
            serial_println!("[usbnetftdi] heartbeat s={} beats={}", s, FTDI_BEATS.load(Ordering::Relaxed) - b0); // the line itself is an FTDI bulk-OUT: it is the beat it counts
        }
        for _ in 0..256 { core::hint::spin_loop(); }
    }
    let beats = FTDI_BEATS.load(Ordering::Relaxed) - b0;
    let events = RX_XFERS.load(Ordering::Relaxed) - x0;
    let rx_armed = armed() || RX_ARMS.load(Ordering::Relaxed) > a0;
    let ok = beats > 0 && rx_armed && is_up();
    serial_println!(
        ":: USBNETFTDI: ftdi_alive={} rx_armed={} events={} beats={} drives={} -> {} ::",
        (beats > 0) as u8, rx_armed as u8, events, beats, DRIVES.load(Ordering::Relaxed) - d0, if ok { "PASS" } else { "FAIL" }
    );
}

// ── USBNETRX (rmbp-ledger B502) ─────────────────────────────────────────────────────────────────────
// Flight 27: the first High-Speed attach (`mps=512`, PORTSC speed 3) armed the bulk-IN ring, rang it, kicked it and
// reset it, and no IN transfer ever completed (`xfers=0`) while the OUT pipe of the same slot completed every DISCOVER.
// The wire could not say whether the controller FETCHED the armed TRB (its TR Dequeue past it: the completion event was
// lost) or never did (dequeue == the armed TRB: the device NAKed every IN). These lines are that read (xHCI 1.2 §6.2.3
// for the endpoint state, §4.6.9/§4.6.10 for Stop and Set TR Dequeue), and the bound on the back-off that hid it.
static RXR_DOORBELLS: AtomicU64 = AtomicU64::new(0);
static RXR_LINES: AtomicU64 = AtomicU64::new(0);
static RXR_AFTER_RESET: AtomicBool = AtomicBool::new(false);
static RXR_TD: AtomicU8 = AtomicU8::new(0); // 0 unread, 1 untouched (controller dequeue == armed TRB), 2 consumed
static RXR_DEAD_SAID: AtomicBool = AtomicBool::new(false);
static RXR_FRAME_SAID: AtomicBool = AtomicBool::new(false);
/// xHCI 1.2 §6.2.3 Table 6-8, Endpoint Context EP State.
pub fn ep_state_name(s: u8) -> &'static str {
    match s { 0 => "disabled", 1 => "running", 2 => "halted", 3 => "stopped", 4 => "error", _ => "unknown" }
}
fn rxr_usb() -> &'static str {
    match IN_MPS.load(Ordering::Relaxed) { 1024 => "ss", 512 => "hs", 0 => "none", _ => "fs" }
}
fn rxr_td_name(t: u8) -> &'static str {
    match t { 1 => "untouched", 2 => "consumed", _ => "unread" }
}
/// Index of `phys` in a ring of `n` TRBs at `base`, or `out` when the controller's pointer is not in the ring.
fn rxr_idx(phys: u64, base: u64, n: usize) -> alloc::string::String {
    if phys >= base && phys < base + (n as u64) * 16 { alloc::format!("{}", (phys - base) / 16) } else { alloc::string::String::from("out") }
}
/// The first delivered frame of the boot: `[usbnet] rx frame n=1 len=<n>` (the witness the arc asks for, once).
fn rxr_frame(len: usize) {
    if !RXR_FRAME_SAID.swap(true, Ordering::Relaxed) {
        serial_println!("[usbnet] rx frame n=1 len={} usb={} resets={}", len, rxr_usb(), NF_RESETS.load(Ordering::Relaxed));
    }
}
/// While no IN completion has ever arrived, rung two's back-off stays at its floor: zero frames is a FAIL already said,
/// not a reason to wait 32, 64, 128 s for the next look.
fn rxr_backoff(doubled: u64) -> u64 {
    if RX_XFERS.load(Ordering::Relaxed) == 0 { U9_BACKOFF_MIN_MS } else { doubled }
}
impl super::XhciController {
    /// USBNETRX M1: after each arm's doorbell. `[usbnet] rx arm trbs=<idx> cycle=<the TRB's own C bit> pcs=<the ring's
    /// producer cycle> ep_state=<§6.2.3 name> hw_deq=<the endpoint context's TR Dequeue, as an index> doorbell=rung
    /// usb=<hs|ss>` — arms 1..8, the first arm after each reset, and every power of two.
    pub(super) fn usbnet_arm_read(&self, slot: u8, dci: u8, trb_phys: u64) {
        RXR_DOORBELLS.fetch_add(1, Ordering::Relaxed);
        let n = RXR_LINES.fetch_add(1, Ordering::Relaxed) + 1;
        let after = RXR_AFTER_RESET.swap(false, Ordering::Relaxed);
        if !(n <= 8 || after || n.is_power_of_two()) { return; }
        let r = match self.slots[slot as usize].bulk_in_ring.as_ref() { Some(r) => r, None => return };
        let base = r.get_ptr();
        let c = unsafe { core::ptr::read_volatile((trb_phys + 12) as *const u32) } & 1;
        let hw = self.ep_ctx_deq(slot, dci);
        serial_println!("[usbnet] rx arm trbs={} cycle={} pcs={} ep_state={} hw_deq={} dcs={} doorbell=rung usb={} n={}",
            trb_phys.wrapping_sub(base) / 16, c, r.cycle_bit() as u8, ep_state_name(self.ep_state_of(slot, dci)),
            rxr_idx(hw & !0xF, base, r.num_trbs()), hw & 1, rxr_usb(), n);
    }
    /// USBNETRX M1 + M2: rung two, after the Stop Endpoint and BEFORE the Set TR Dequeue moves the pointer — the one moment
    /// the controller's own dequeue says whether it fetched the armed TD. `[usbnet] rx reset read armed=<idx> hw_deq=<idx>
    /// td=<untouched|consumed> stopped_ev=<ccN|none> ep_state=<name>`; on the second fruitless reset with no IN completion
    /// ever, once: `[usbnet] rx dead resets=<n> xfers=0 td=<…> usb=<…> -> FAIL reason=<device-silent|event-lost>`.
    pub(super) fn usbnet_reset_read(&self, slot: u8, dci: u8) {
        let armed = TRB_PHYS.load(Ordering::Relaxed);
        let hw = self.ep_ctx_deq(slot, dci) & !0xF;
        let td = if armed == 0 { 0 } else if hw == armed { 1 } else { 2 };
        RXR_TD.store(td, Ordering::Relaxed);
        RXR_AFTER_RESET.store(true, Ordering::Relaxed);
        let ev = if DONE.load(Ordering::Relaxed) { alloc::format!("cc{}", CODE.load(Ordering::Relaxed)) } else { alloc::string::String::from("none") };
        let r = NF_RESETS.load(Ordering::Relaxed) + 1;
        if r <= 8 || r.is_power_of_two() {
            let (ai, hi) = match self.slots[slot as usize].bulk_in_ring.as_ref() {
                Some(ring) => (rxr_idx(armed, ring.get_ptr(), ring.num_trbs()), rxr_idx(hw, ring.get_ptr(), ring.num_trbs())),
                None => (alloc::string::String::from("none"), alloc::string::String::from("none")),
            };
            serial_println!("[usbnet] rx reset read n={} armed={} hw_deq={} td={} stopped_ev={} ep_state={} usb={}",
                r, ai, hi, rxr_td_name(td), ev, ep_state_name(self.ep_state_of(slot, dci)), rxr_usb());
        }
        if r >= 2 && RX_XFERS.load(Ordering::Relaxed) == 0 && !RXR_DEAD_SAID.swap(true, Ordering::Relaxed) {
            serial_println!("[usbnet] rx dead resets={} xfers=0 td={} usb={} -> FAIL reason={}", r, rxr_td_name(td), rxr_usb(),
                match td { 1 => "device-silent", 2 => "event-lost", _ => "unread" });
        }
    }
}
/// `tests usbnetrx` (B502): drive the link up to 5 s or until a frame arrives, then read the IN endpoint.
/// `:: USBNETRX: armed=<0|1> doorbell=<0|1> ep_state=<name> frames=<n> td=<…> usb=<hs|ss> -> PASS|FAIL ::`;
/// no dongle -> `SKIP reason=no-dongle`. PASS = armed, rung, Running, and at least one frame delivered this attach.
pub fn usbnetrx_selftest() {
    if !is_up() {
        serial_println!(":: USBNETRX: armed=0 doorbell=0 ep_state=none frames=0 reason=no-dongle -> SKIP ::");
        return;
    }
    let (f0, t0) = (RX_OK.load(Ordering::Relaxed), crate::arch::ms());
    while crate::arch::ms().saturating_sub(t0) < 5000 && is_up() && RX_OK.load(Ordering::Relaxed) == f0 {
        main_pass();
        for _ in 0..256 { core::hint::spin_loop(); }
    }
    let st = match super::claim() { Ok(x) => x.usbnet_ring_state().map(|s| s.3).unwrap_or(0xFF), Err(_) => 0xFF };
    let armed = (armed() || RX_ARMS.load(Ordering::Relaxed) > 0) as u8;
    let rung = (RXR_DOORBELLS.load(Ordering::Relaxed) > 0) as u8;
    let frames = RX_OK.load(Ordering::Relaxed);
    let ok = armed == 1 && rung == 1 && st == 1 && frames > 0;
    serial_println!(":: USBNETRX: armed={} doorbell={} ep_state={} frames={} window={} td={} usb={} -> {} ::",
        armed, rung, ep_state_name(st), frames, frames - f0, rxr_td_name(RXR_TD.load(Ordering::Relaxed)), rxr_usb(), if ok { "PASS" } else { "FAIL" });
}

// ── USBNETDHCP (rmbp-ledger B538) — TAIL-APPENDED ───────────────────────────────────────────────────
// Flight 31: smoltcp's DISCOVER went out three times (262-octet BOOTP, broadcast flag clear), 54 frames came
// back, no reply from port 67 ever reached the ring (no `[usbnet] rx dhcp` line) and nothing said what the 54
// were. The tap below (called from `deliver` for every frame, before the ring's verdict matters) classifies
// each frame on the wire and runs the link's own RFC 2131 client (`net::dhcp`, 300-octet BOOTP, broadcast
// flag set — so the OFFER/ACK come back broadcast, the class the RX filter is already proven to pass), then
// answers ARP for the lease and pings the gateway once. Everything here runs under the controller loan
// (`take_done` and `deliver` are both inside `usbnet_data_pass`), so the client mutex never contends; frames
// go straight onto TXQ (never `raw_tx`: its `drive` would re-take the loan we hold) and the same pass issues them.

use net::dhcp::{DhcpClient, Handled};

/// Frames printed one line each before the counts take over.
const UD_SHOW: u64 = 32;
/// Link seen -> the first DISCOVER (the PHY's first frames settle).
const UD_SETTLE_MS: u64 = 1_000;
/// DISCOVER retransmit back-off (RFC 2131 §4.1: 4, 8, 16, 32 s …), capped.
const UD_RETX_MIN_MS: u64 = 4_000;
const UD_RETX_MAX_MS: u64 = 32_000;
/// A REQUEST unanswered this long restarts discovery.
const UD_REQ_MS: u64 = 4_000;
/// Gateway ARP / echo: per-try wait and tries per stage.
const UD_PING_MS: u64 = 2_000;
const UD_PING_TRIES: u32 = 3;
/// ICMP identifier of the gateway echo ("UD"), distinct from smolnet's 0x554E.
const UD_PING_IDENT: u16 = 0x5544;
const UD_PING_DATA: &[u8] = b"unaos-usbnet";

const UD_IDLE: u8 = 0;
const UD_DISCOVERING: u8 = 1;
const UD_REQUESTING: u8 = 2;
const UD_BOUND: u8 = 3;
/// Ping stages: none, ARP sent, echo sent, done (answered or given up).
const UP_NONE: u8 = 0;
const UP_ARP: u8 = 1;
const UP_ECHO: u8 = 2;
const UP_DONE: u8 = 3;

static UD_CLIENT: Mutex<Option<DhcpClient>> = Mutex::new(None);
static UD_STATE: AtomicU8 = AtomicU8::new(UD_IDLE);
/// When the client next acts (0 = the link not yet seen up).
static UD_NEXT: AtomicU64 = AtomicU64::new(0);
static UD_RETX: AtomicU64 = AtomicU64::new(0);
static UD_DISCOVERS: AtomicU64 = AtomicU64::new(0);
static UD_OFFERS: AtomicU64 = AtomicU64::new(0);
static UD_ACKS: AtomicU64 = AtomicU64::new(0);
static UD_NAKS: AtomicU64 = AtomicU64::new(0);
static UD_IGNORED: AtomicU64 = AtomicU64::new(0);
/// The lease, big-endian octets packed (0 = none).
static UD_IP: AtomicU32 = AtomicU32::new(0);
static UD_MASK: AtomicU32 = AtomicU32::new(0);
static UD_GW: AtomicU32 = AtomicU32::new(0);
static UD_DNS: AtomicU32 = AtomicU32::new(0);
static UD_LEASE_S: AtomicU32 = AtomicU32::new(0);
/// A bound lease the stack (smolnet) has not applied yet.
static UD_PENDING: AtomicBool = AtomicBool::new(false);
static UD_PSTATE: AtomicU8 = AtomicU8::new(UP_NONE);
static UD_PAT: AtomicU64 = AtomicU64::new(0);
static UD_PTRIES: AtomicU32 = AtomicU32::new(0);
static UD_GWMAC: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];
static UD_ARPS: AtomicU64 = AtomicU64::new(0);
// Frame classes (every frame the data pass delivered, ring room or not).
static UD_RX_N: AtomicU64 = AtomicU64::new(0);
static UD_C_ARP: AtomicU64 = AtomicU64::new(0);
static UD_C_V4: AtomicU64 = AtomicU64::new(0);
static UD_C_V6: AtomicU64 = AtomicU64::new(0);
static UD_C_OTHER: AtomicU64 = AtomicU64::new(0);
static UD_C_BCAST: AtomicU64 = AtomicU64::new(0);
static UD_C_MCAST: AtomicU64 = AtomicU64::new(0);
static UD_C_OURS: AtomicU64 = AtomicU64::new(0);
static UD_C_FOREIGN: AtomicU64 = AtomicU64::new(0);
static UD_C_UDP67: AtomicU64 = AtomicU64::new(0);
static UD_C_QFULL: AtomicU64 = AtomicU64::new(0);

/// Dotted-quad display.
struct UdIp([u8; 4]);
impl core::fmt::Display for UdIp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}
fn ud_ip(a: &AtomicU32) -> [u8; 4] { a.load(Ordering::Relaxed).to_be_bytes() }
fn ud_be16(b: &[u8], i: usize) -> u16 { u16::from_be_bytes([b[i], b[i + 1]]) }
fn ud_state_name(s: u8) -> &'static str {
    match s { UD_DISCOVERING => "discovering", UD_REQUESTING => "requesting", UD_BOUND => "bound", _ => "idle" }
}
/// One frame onto TXQ (a masked micro-hold, like every other hold of the ring).
fn ud_push(frame: &[u8]) -> bool {
    let ok = TXQ.lock().push(frame);
    if !ok { TX_DROP.fetch_add(1, Ordering::Relaxed); }
    ok
}

/// The slot went away: the client, the lease and the ping with it (the counts stay, per boot).
fn ud_reset() {
    { let _m = crate::arch::IrqMask::new(); *UD_CLIENT.lock() = None; }
    UD_STATE.store(UD_IDLE, Ordering::Relaxed);
    UD_NEXT.store(0, Ordering::Relaxed);
    UD_RETX.store(0, Ordering::Relaxed);
    for a in [&UD_IP, &UD_MASK, &UD_GW, &UD_DNS, &UD_LEASE_S] { a.store(0, Ordering::Relaxed); }
    UD_PENDING.store(false, Ordering::Relaxed);
    UD_PSTATE.store(UP_NONE, Ordering::Relaxed);
    UD_PTRIES.store(0, Ordering::Relaxed);
    ud_probe_reset(); // USBNETRX (B545)
    UD_ACQ_T0.store(0, Ordering::Relaxed); UD_XID_NEXT.store(0, Ordering::Relaxed); UD_BIG.store(UB_NONE, Ordering::Relaxed); // DHCPREPLUG (B632)
}

/// Every data pass: with the PHY up, the client's next step when it is due; once bound, the ping's.
fn ud_tick() {
    if !is_up() || (kind() == KIND_AX88179 && !link_up()) { return; }
    let now = crate::arch::ms().max(1);
    ud_probe_tick(now); // USBNETRX (B545): the link-up ARP probe whose reply is unicast to our MAC
    ud_big_tick(now); // DHCPREPLUG (B632): after the probe's unicast answer, the same who-has padded to the DISCOVER's 342 bytes
    let state = UD_STATE.load(Ordering::Relaxed);
    if state == UD_BOUND { ud_ping_tick(now); return; }
    let next = UD_NEXT.load(Ordering::Relaxed);
    if next == 0 { UD_NEXT.store(now + UD_SETTLE_MS, Ordering::Relaxed); return; }
    if now < next { return; }
    if state == UD_REQUESTING {
        serial_println!("[dhcp] request unanswered ms={} -> discover", UD_REQ_MS);
    }
    let retx = UD_RETX.load(Ordering::Relaxed).max(UD_RETX_MIN_MS).min(UD_RETX_MAX_MS);
    UD_RETX.store((retx * 2).min(UD_RETX_MAX_MS), Ordering::Relaxed);
    UD_NEXT.store(now + retx, Ordering::Relaxed);
    ud_discover(now, retx);
}

/// A padded broadcast DISCOVER under a fresh xid onto TXQ. `[dhcp] discover n= xid= len= retx_ms=` the first
/// two, every power of two, and the first after a link-up restart (the u9 dump prints its bytes when the pass issues
/// it: `flags=8000 len=342`). DHCPREPLUG (B632): `retx_ms` is the interval this DISCOVER waits before the next — the
/// B538 line printed half the stored next value, so at the 32 s cap it read 16000 (flight 34); `secs` counts from
/// this acquisition's first DISCOVER (RFC 2131 §4.4.1); the full option set (61, 12, 55) via `discover_rfc`; the
/// first DISCOVER of the boot is decoded once as `[dhcp] tx-audit`.
fn ud_discover(now: u64, retx: u64) {
    let m = mac();
    let pre = UD_XID_NEXT.swap(0, Ordering::Relaxed);
    let xid = if pre != 0 { pre } else { (crate::arch::now_cycles() as u32) ^ u32::from_be_bytes([m[2], m[3], m[4], m[5]]) };
    let _ = UD_ACQ_T0.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
    let secs = (now.saturating_sub(UD_ACQ_T0.load(Ordering::Relaxed)) / 1000).min(0xffff) as u16;
    let mut out = [0u8; 400];
    let n = {
        let _m = crate::arch::IrqMask::new();
        let mut g = UD_CLIENT.lock();
        let c = g.get_or_insert_with(|| DhcpClient::new(m, xid));
        c.discover_rfc(xid, secs, &mut out)
    };
    let Some(n) = n else { return };
    UD_STATE.store(UD_DISCOVERING, Ordering::Relaxed);
    UD_T_DISC.store(now, Ordering::Relaxed);
    UD_LAST_RETX.store(retx, Ordering::Relaxed);
    let k = UD_DISCOVERS.fetch_add(1, Ordering::Relaxed) + 1;
    let pushed = ud_push(&out[..n]);
    let restart = UD_RESTART_SHOW.swap(false, Ordering::Relaxed);
    if k <= 2 || k.is_power_of_two() || !pushed || restart {
        let ra = UD_RESTART_AT.load(Ordering::Relaxed);
        serial_println!("[dhcp] discover n={} xid={:08x} len={} secs={} client=net retx_ms={} q={}{}", k, xid, n, secs,
            retx, if pushed { "ok" } else { "full" }, if restart { alloc::format!(" after_link_up_ms={}", now.saturating_sub(ra)) } else { alloc::string::String::new() });
        ud_filter_line("discover");
    }
    if !UD_AUDITED.swap(true, Ordering::Relaxed) { ud_tx_audit(&out[..n]); }
}

/// The counts line: every frame class since boot.
fn ud_counts_line(n: u64) {
    serial_println!(
        "[usbnet] rx counts n={} arp={} v4={} v6={} other={} bcast={} mcast={} ours={} foreign={} udp67={} qfull={}",
        n, UD_C_ARP.load(Ordering::Relaxed), UD_C_V4.load(Ordering::Relaxed), UD_C_V6.load(Ordering::Relaxed),
        UD_C_OTHER.load(Ordering::Relaxed), UD_C_BCAST.load(Ordering::Relaxed), UD_C_MCAST.load(Ordering::Relaxed),
        UD_C_OURS.load(Ordering::Relaxed), UD_C_FOREIGN.load(Ordering::Relaxed), UD_C_UDP67.load(Ordering::Relaxed),
        UD_C_QFULL.load(Ordering::Relaxed)
    );
}

/// `tests usbnet`: the classes and the client.
fn ud_report() {
    ud_counts_line(UD_RX_N.load(Ordering::Relaxed));
    let ping = match UD_PSTATE.load(Ordering::Relaxed) { UP_ARP => "arp", UP_ECHO => "echo", UP_DONE => "done", _ => "none" };
    serial_println!(
        "[usbnet] dhcp state={} discovers={} offers={} acks={} naks={} ignored={} ip={} gw={} ping={} arp_replies={}",
        ud_state_name(UD_STATE.load(Ordering::Relaxed)), UD_DISCOVERS.load(Ordering::Relaxed), UD_OFFERS.load(Ordering::Relaxed),
        UD_ACKS.load(Ordering::Relaxed), UD_NAKS.load(Ordering::Relaxed), UD_IGNORED.load(Ordering::Relaxed),
        UdIp(ud_ip(&UD_IP)), UdIp(ud_ip(&UD_GW)), ping, UD_ARPS.load(Ordering::Relaxed)
    );
}

/// Every delivered frame: classified (one line each for the first `UD_SHOW`, then the counts at each power of
/// two), then a DHCP reply goes to the client and, once bound, ARP and ICMP to the responders.
fn ud_tap(f: &[u8], queued: bool) {
    if f.len() < 14 { return; }
    let et = ud_be16(f, 12);
    let m = mac();
    let d = &f[0..6];
    let dst = if d.iter().all(|&b| b == 0xff) {
        UD_C_BCAST.fetch_add(1, Ordering::Relaxed); "bcast"
    } else if d[0] & 1 != 0 {
        UD_C_MCAST.fetch_add(1, Ordering::Relaxed); "mcast"
    } else if d == &m[..] {
        UD_C_OURS.fetch_add(1, Ordering::Relaxed); "ours"
    } else {
        UD_C_FOREIGN.fetch_add(1, Ordering::Relaxed); "other"
    };
    if !queued { UD_C_QFULL.fetch_add(1, Ordering::Relaxed); }
    let mut proto: Option<u8> = None;
    let mut arp_op: Option<u16> = None;
    let mut ports: Option<(u16, u16)> = None;
    let mut l4 = 0usize;
    match et {
        0x0806 => {
            UD_C_ARP.fetch_add(1, Ordering::Relaxed);
            if f.len() >= 22 { arp_op = Some(ud_be16(f, 20)); }
        }
        0x0800 => {
            UD_C_V4.fetch_add(1, Ordering::Relaxed);
            if f.len() >= 34 {
                l4 = 14 + ((f[14] & 0x0f) as usize) * 4;
                proto = Some(f[23]);
                if (f[23] == 17 || f[23] == 6) && f.len() >= l4 + 4 { ports = Some((ud_be16(f, l4), ud_be16(f, l4 + 2))); }
            }
        }
        0x86dd => {
            UD_C_V6.fetch_add(1, Ordering::Relaxed);
            if f.len() >= 54 {
                proto = Some(f[20]);
                if (f[20] == 17 || f[20] == 6) && f.len() >= 58 { ports = Some((ud_be16(f, 54), ud_be16(f, 56))); }
            }
        }
        _ => { UD_C_OTHER.fetch_add(1, Ordering::Relaxed); }
    }
    let n = UD_RX_N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= UD_SHOW || (dst == "ours" && UD_C_OURS.load(Ordering::Relaxed) <= UD_OURS_SHOW) { // USBNETRX: the first unicast frames to our MAC print past the 32
        let p = match (arp_op, proto) {
            (Some(op), _) => alloc::format!("arp{}", op),
            (None, Some(p)) => alloc::format!("{}", p),
            _ => alloc::string::String::from("-"),
        };
        let ps = match ports { Some((s, t)) => alloc::format!("{}->{}", s, t), None => alloc::string::String::from("-") };
        serial_println!("[usbnet] rx ethertype={:#06x} proto={} ports={} len={} dst={} q={}", et, p, ps, f.len(), dst, if queued { "ok" } else { "full" });
    } else if n.is_power_of_two() {
        ud_counts_line(n);
    }
    if let (0x0800, Some(17), Some((sp, dp))) = (et, proto, ports) {
        if sp == 67 || sp == 68 || dp == 67 || dp == 68 { ud_heard(f, dst); } // DHCPREPLUG (B632): every UDP 67/68 frame we hear, anyone's
    }
    if et == 0x0800 {
        if let Some((sp, dp)) = ports {
            if proto == Some(17) && sp == 67 {
                UD_C_UDP67.fetch_add(1, Ordering::Relaxed);
                if dp == 68 { ud_dhcp_rx(f); }
            }
        } else if proto == Some(1) && f.len() >= l4 + 8 {
            ud_icmp_rx(f, l4);
        }
    } else if et == 0x0806 && f.len() >= 42 {
        ud_probe_rx(f, arp_op.unwrap_or(0), dst); // USBNETRX (B545): the probe's answer, with the class it arrived as
        ud_arp_rx(f, arp_op.unwrap_or(0));
        ud_big_rx(f, arp_op.unwrap_or(0), dst); // DHCPREPLUG (B632)
    }
}

/// A DHCP server reply (UDP 67 -> 68) into the client.
fn ud_dhcp_rx(f: &[u8]) {
    let Some(l) = net::dhcp::parse_lease(f) else { return };
    let mut out = [0u8; 400];
    let (h, ours) = {
        let _m = crate::arch::IrqMask::new();
        let mut g = UD_CLIENT.lock();
        let now = crate::arch::ms().max(1);
        let t0 = UD_ACQ_T0.load(Ordering::Relaxed);
        let secs = if t0 == 0 { 0 } else { (now.saturating_sub(t0) / 1000).min(0xffff) as u16 };
        match g.as_mut() { Some(c) => (c.handle_rfc(&l, secs, &mut out), c.xid()), None => (Handled::Ignored, 0) } // DHCPREPLUG (B632): the REQUEST carries 61/12/55 and secs as the DISCOVER did
    };
    let now = crate::arch::ms().max(1);
    let r = &l.reply;
    match h {
        Handled::Request(n) => {
            UD_OFFERS.fetch_add(1, Ordering::Relaxed);
            UD_STATE.store(UD_REQUESTING, Ordering::Relaxed);
            UD_NEXT.store(now + UD_REQ_MS, Ordering::Relaxed);
            UD_T_OFFER.store(now, Ordering::Relaxed); // DHCPREPLUG (B632)
            let pushed = ud_push(&out[..n]);
            serial_println!("[dhcp] offer ip={} server={} -> request{}", UdIp(r.your_ip), UdIp(r.server_ip), if pushed { "" } else { " q=full" });
        }
        Handled::Bound => {
            UD_ACKS.fetch_add(1, Ordering::Relaxed);
            UD_IP.store(u32::from_be_bytes(r.your_ip), Ordering::Relaxed);
            UD_MASK.store(u32::from_be_bytes(l.mask), Ordering::Relaxed);
            UD_GW.store(u32::from_be_bytes(l.router), Ordering::Relaxed);
            UD_DNS.store(u32::from_be_bytes(l.dns), Ordering::Relaxed);
            UD_LEASE_S.store(l.lease_s, Ordering::Relaxed);
            UD_STATE.store(UD_BOUND, Ordering::Relaxed);
            UD_PENDING.store(true, Ordering::Relaxed);
            UD_DORA_MS.store(now.saturating_sub(UD_T_DISC.load(Ordering::Relaxed)), Ordering::Relaxed); UD_ACQ_T0.store(0, Ordering::Relaxed); // DHCPREPLUG (B632): DISCOVER -> ACK of the transaction that bound
            serial_println!(":: USBNET: dhcp=bound ip={} mask={} gw={} dns={} lease={} ::",
                UdIp(r.your_ip), UdIp(l.mask), UdIp(l.router), UdIp(l.dns), l.lease_s);
            ud_ping_start(now);
        }
        Handled::Nak => {
            UD_NAKS.fetch_add(1, Ordering::Relaxed);
            UD_STATE.store(UD_IDLE, Ordering::Relaxed);
            UD_RETX.store(0, Ordering::Relaxed);
            UD_NEXT.store(now, Ordering::Relaxed);
            serial_println!("[dhcp] nak server={} -> discover", UdIp(r.server_ip));
        }
        Handled::Ignored => {
            let k = UD_IGNORED.fetch_add(1, Ordering::Relaxed);
            if k < 4 {
                serial_println!("[dhcp] reply ignored type={} xid={:08x} ours={:08x} state={}", r.msg_type, r.xid, ours, ud_state_name(UD_STATE.load(Ordering::Relaxed)));
            }
        }
    }
}

/// After the ACK: ARP the gateway (the echo follows its reply), or say there is no router to ping.
fn ud_ping_start(now: u64) {
    UD_PTRIES.store(0, Ordering::Relaxed);
    if UD_GW.load(Ordering::Relaxed) == 0 {
        UD_PSTATE.store(UP_DONE, Ordering::Relaxed);
        serial_println!("[net] ping gw=0.0.0.0 rtt_ms=none reason=no-router");
        return;
    }
    ud_arp_gw(now);
}
/// Who-has the gateway, from the lease, broadcast, padded to 60.
fn ud_arp_gw(now: u64) {
    let mut buf = [0u8; 60];
    let m = mac();
    if net::arp::build_request(&mut buf[14..42], m, ud_ip(&UD_IP), ud_ip(&UD_GW)).is_none()
        || net::ethernet::write_header(&mut buf[0..14], [0xff; 6], m, 0x0806).is_none()
    {
        return;
    }
    UD_PSTATE.store(UP_ARP, Ordering::Relaxed);
    UD_PAT.store(now, Ordering::Relaxed);
    UD_PTRIES.fetch_add(1, Ordering::Relaxed);
    let _ = ud_push(&buf);
}
/// One echo request to the gateway's MAC (seq 1), padded to 60.
fn ud_echo(now: u64) {
    let mut buf = [0u8; 80];
    let m = mac();
    let mut gm = [0u8; 6];
    for i in 0..6 { gm[i] = UD_GWMAC[i].load(Ordering::Relaxed); }
    let Some(n) = net::icmp::write_echo_request(&mut buf[34..], UD_PING_IDENT, 1, UD_PING_DATA) else { return };
    if net::ipv4::write_header(&mut buf[14..34], ud_ip(&UD_IP), ud_ip(&UD_GW), net::ipv4::PROTO_ICMP, n).is_none()
        || net::ethernet::write_header(&mut buf[0..14], gm, m, 0x0800).is_none()
    {
        return;
    }
    UD_PSTATE.store(UP_ECHO, Ordering::Relaxed);
    UD_PAT.store(now, Ordering::Relaxed);
    UD_PTRIES.fetch_add(1, Ordering::Relaxed);
    let _ = ud_push(&buf[..(34 + n).max(60)]);
}
/// Bound, every pass: retry the stage that has waited `UD_PING_MS`, or give up after `UD_PING_TRIES`.
fn ud_ping_tick(now: u64) {
    let st = UD_PSTATE.load(Ordering::Relaxed);
    if st != UP_ARP && st != UP_ECHO { return; }
    if now.saturating_sub(UD_PAT.load(Ordering::Relaxed)) < UD_PING_MS { return; }
    let tries = UD_PTRIES.load(Ordering::Relaxed);
    if tries >= UD_PING_TRIES {
        UD_PSTATE.store(UP_DONE, Ordering::Relaxed);
        serial_println!("[net] ping gw={} rtt_ms=none reason={} tries={}", UdIp(ud_ip(&UD_GW)), if st == UP_ARP { "no-arp" } else { "no-reply" }, tries);
        return;
    }
    if st == UP_ARP { ud_arp_gw(now) } else { ud_echo(now) }
}
/// ARP while bound: answer a request for our address; take the gateway's reply and send the echo.
fn ud_arp_rx(f: &[u8], op: u16) {
    if UD_STATE.load(Ordering::Relaxed) != UD_BOUND { return; }
    let a = &f[14..];
    let ours = ud_ip(&UD_IP);
    let m = mac();
    if op == 1 {
        let sm = net::arp::ArpStateMachine::new(ours, m);
        if let Some((reply, dmac)) = sm.process_packet(a) {
            let mut buf = [0u8; 60];
            if net::ethernet::write_frame(&mut buf, dmac, m, 0x0806, &reply).is_some() {
                let pushed = ud_push(&buf);
                let k = UD_ARPS.fetch_add(1, Ordering::Relaxed) + 1;
                if k <= 4 || k.is_power_of_two() {
                    serial_println!("[net] arp reply to={} for={} n={}{}", UdIp([a[14], a[15], a[16], a[17]]), UdIp(ours), k, if pushed { "" } else { " q=full" });
                }
            }
        }
    } else if op == 2 && UD_PSTATE.load(Ordering::Relaxed) == UP_ARP && a[14..18] == ud_ip(&UD_GW) {
        for i in 0..6 { UD_GWMAC[i].store(a[8 + i], Ordering::Relaxed); }
        serial_println!("[net] arp gw={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", UdIp(ud_ip(&UD_GW)), a[8], a[9], a[10], a[11], a[12], a[13]);
        UD_PTRIES.store(0, Ordering::Relaxed);
        ud_echo(crate::arch::ms().max(1));
    }
}
/// ICMP while the echo is out: the gateway's reply to our ident/seq times the ping.
fn ud_icmp_rx(f: &[u8], l4: usize) {
    if UD_PSTATE.load(Ordering::Relaxed) != UP_ECHO { return; }
    if f[26..30] != ud_ip(&UD_GW) || f[30..34] != ud_ip(&UD_IP) { return; }
    if let Some((id, seq)) = net::icmp::parse_echo_reply(&f[l4..]) {
        if id == UD_PING_IDENT && seq == 1 {
            let rtt = crate::arch::ms().saturating_sub(UD_PAT.load(Ordering::Relaxed));
            UD_PSTATE.store(UP_DONE, Ordering::Relaxed);
            serial_println!("[net] ping gw={} rtt_ms={}", UdIp(ud_ip(&UD_GW)), rtt);
        }
    }
}

/// The bound lease the stack has not applied yet: (address, mask, router, DNS). smolnet's `usb_lease_tick`.
pub fn lease_pending() -> Option<([u8; 4], [u8; 4], [u8; 4], [u8; 4])> {
    if !UD_PENDING.load(Ordering::Relaxed) || UD_STATE.load(Ordering::Relaxed) != UD_BOUND { return None; }
    Some((ud_ip(&UD_IP), ud_ip(&UD_MASK), ud_ip(&UD_GW), ud_ip(&UD_DNS)))
}
/// The stack took (or declined) the lease.
pub fn lease_applied() { UD_PENDING.store(false, Ordering::Relaxed); }

// ── USBNETRX (rmbp-ledger B545) — TAIL-APPENDED ─────────────────────────────────────────────────────
// Flight 32: 64 frames classified, `ours=0 udp67=0` — every frame mcast/bcast. Nothing in that session SOLICITED a
// unicast to our MAC: smoltcp's flag-clear DISCOVERs drew no reply of any class (no udp67 even broadcast), and the
// USBNET7 ARP probe (whose reply is unicast to us) runs only inside `tests usbnet7`, which was not typed. So "zero
// unicast" did not separate "the RX filter drops directed frames" from "our TX never reached the LAN". This probe runs
// on every link-up by itself: who-has a peer we have HEARD (`U8_PEER_V4`, an IPv4 source off the ring), sender IP
// 0.0.0.0, sender MAC ours (RFC 5227) — the owner replies to our MAC. Three tries, 2 s apart, onto TXQ (we are inside
// the data pass under the loan: never `raw_tx`).
//   `[usbnet] txprobe link who_has=<ip> try=<k> reply_ms=<n> dst=ours -> unicast-rx ok`   TX reached the LAN AND a
//                                                                                      directed frame passed RX_CTL
//   `[usbnet] txprobe link who_has=<ip> tries=3 reply_ms=none -> no-reply`                 TX or directed RX is dead
//   (`dst=bcast` on the ok line = the peer broadcast its reply: TX proven, unicast RX still unproven.)
/// Unicast frames to our MAC printed one line each, past `UD_SHOW`.
const UD_OURS_SHOW: u64 = 8;
const UD_PROBE_WAIT_MS: u64 = 2_000;
const UD_PROBE_TRIES: u32 = 3;
static UD_PROBE_IP: AtomicU32 = AtomicU32::new(0);
static UD_PROBE_AT: AtomicU64 = AtomicU64::new(0);
static UD_PROBE_N: AtomicU32 = AtomicU32::new(0);
/// The probe has a verdict this boot (answered, or out of tries).
static UD_PROBE_DONE: AtomicBool = AtomicBool::new(false);

/// Every data pass with the PHY up: the next probe try when due, the no-reply verdict after the last.
fn ud_probe_tick(now: u64) {
    if UD_PROBE_DONE.load(Ordering::Relaxed) { return; }
    let at = UD_PROBE_AT.load(Ordering::Relaxed);
    if at != 0 && now.saturating_sub(at) < UD_PROBE_WAIT_MS { return; }
    let k = UD_PROBE_N.load(Ordering::Relaxed);
    if k >= UD_PROBE_TRIES {
        UD_PROBE_DONE.store(true, Ordering::Relaxed);
        let o = UD_PROBE_IP.load(Ordering::Relaxed).to_be_bytes();
        serial_println!("[usbnet] txprobe link who_has={}.{}.{}.{} tries={} reply_ms=none ours={} -> no-reply", o[0], o[1], o[2], o[3], k, UD_C_OURS.load(Ordering::Relaxed));
        return;
    }
    let ip = U8_PEER_V4.load(Ordering::Relaxed);
    if ip == 0 { return; } // no IPv4 peer heard yet: nobody to ask
    if !ud_push(&u8_probe_frame(mac(), ip)) { return; } // TXQ full: next pass
    UD_PROBE_IP.store(ip, Ordering::Relaxed);
    UD_PROBE_AT.store(now, Ordering::Relaxed);
    UD_PROBE_N.store(k + 1, Ordering::Relaxed);
}

/// An ARP reply (op 2) from the probed address to our MAC: the verdict, naming the class the reply came as.
fn ud_probe_rx(f: &[u8], op: u16, dst: &str) {
    let want = UD_PROBE_IP.load(Ordering::Relaxed);
    if op != 2 || want == 0 || UD_PROBE_DONE.load(Ordering::Relaxed) { return; }
    if u32::from_be_bytes([f[28], f[29], f[30], f[31]]) != want || f[32..38] != mac() { return; }
    UD_PROBE_DONE.store(true, Ordering::Relaxed);
    let o = want.to_be_bytes();
    let rms = crate::arch::ms().saturating_sub(UD_PROBE_AT.load(Ordering::Relaxed));
    serial_println!("[usbnet] txprobe link who_has={}.{}.{}.{} try={} reply_ms={} dst={} -> {}", o[0], o[1], o[2], o[3],
        UD_PROBE_N.load(Ordering::Relaxed), rms, dst, if dst == "ours" { "unicast-rx ok" } else { "tx ok, reply not unicast" });
}

/// The slot went away: the next link-up probes again.
fn ud_probe_reset() {
    UD_PROBE_IP.store(0, Ordering::Relaxed);
    UD_PROBE_AT.store(0, Ordering::Relaxed);
    UD_PROBE_N.store(0, Ordering::Relaxed);
    UD_PROBE_DONE.store(false, Ordering::Relaxed);
}

// ── DHCPREPLUG (rmbp-ledger B632) — TAIL-APPENDED ───────────────────────────────────────────────────
// Flight 34: the LAN answered our unicast-solicited ARP (`txprobe … dst=ours -> unicast-rx ok`) and never a DISCOVER
// (16 of them, 20 minutes, `udp67=0`). This section reads that silence from our side: (1) a link-up restarts the
// client — fresh xid, back-off back to 4 s, the DISCOVER `UD_RESTART_MS` after the PHY reports link
// (`[dhcp] link-up -> restart xid= backoff=4000`), called from `link_seen`'s up edge and from `tests dhcp`;
// (2a) every UDP 67/68 frame the tap sees, anyone's, one `[dhcp] rx` row each; (2b) the first DISCOVER of the boot
// decoded field by field against RFC 2131 §4.4.1 / RFC 2132 (`[dhcp] tx-audit`, `net::dhcp::audit_request`);
// (2c) the RX filter as the part read it back and the frame classes since the link came up (`[dhcp] rx-filter`);
// (2e) the probe's who-has re-sent padded to the DISCOVER's 342 bytes — the 60-byte probe proves TX of a minimum
// frame, this tests TX of a frame the DISCOVER's size (`[dhcp] tx-size probe`).
// `tests dhcp` (QEMU user-net answers DHCP): the lease, then a simulated link-down/up restarts and re-leases.

/// Link-up -> the restart's DISCOVER (inside the brief's 1 s; the AX medium word is written after `link_seen` returns).
const UD_RESTART_MS: u64 = 250;
/// A DISCOVER -> ACK slower than this fails `tests dhcp`.
const UD_DORA_MAX_MS: u64 = 2_000;
static UD_XID_NEXT: AtomicU32 = AtomicU32::new(0);
/// This acquisition's first DISCOVER (ms; 0 = none in flight) — the `secs` field counts from it.
static UD_ACQ_T0: AtomicU64 = AtomicU64::new(0);
static UD_T_DISC: AtomicU64 = AtomicU64::new(0);
static UD_T_OFFER: AtomicU64 = AtomicU64::new(0);
static UD_DORA_MS: AtomicU64 = AtomicU64::new(0);
static UD_LAST_RETX: AtomicU64 = AtomicU64::new(0);
static UD_RESTART_AT: AtomicU64 = AtomicU64::new(0);
static UD_RESTARTS: AtomicU64 = AtomicU64::new(0);
static UD_RESTART_SHOW: AtomicBool = AtomicBool::new(false);
static UD_AUDITED: AtomicBool = AtomicBool::new(false);
static UD_HEARD: AtomicU64 = AtomicU64::new(0);
/// Frame classes at the last link-up (the `rx-filter` line prints what arrived since).
static UD_LU_BCAST: AtomicU64 = AtomicU64::new(0);
static UD_LU_MCAST: AtomicU64 = AtomicU64::new(0);
static UD_LU_OURS: AtomicU64 = AtomicU64::new(0);
static UD_LU_FOREIGN: AtomicU64 = AtomicU64::new(0);
static UD_LU_HEARD: AtomicU64 = AtomicU64::new(0);
/// The 342-byte who-has: none, due (the 60-byte one was answered), sent, verdict printed.
const UB_NONE: u8 = 0;
const UB_DUE: u8 = 1;
const UB_SENT: u8 = 2;
const UB_DONE: u8 = 3;
const UB_LEN: usize = 342;
static UD_BIG: AtomicU8 = AtomicU8::new(UB_NONE);
static UD_BIG_AT: AtomicU64 = AtomicU64::new(0);

struct UdMac([u8; 6]);
impl core::fmt::Display for UdMac {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let m = self.0;
        write!(f, "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
    }
}
fn ud_list(v: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for (i, b) in v.iter().enumerate() {
        if i > 0 { s.push(','); }
        s.push_str(&alloc::format!("{}", b));
    }
    if s.is_empty() { s.push('-'); }
    s
}

/// The PHY reported link (or `tests dhcp` simulated a down/up): restart the client. The lease atomics stay until
/// the new ACK replaces them; the client object stays (its xid is replaced at the DISCOVER, so a late OFFER for the
/// old transaction is ignored); the link probes run again.
fn ud_link_up(now: u64) {
    let now = now.max(1);
    let m = mac();
    let xid = ((crate::arch::now_cycles() as u32) ^ u32::from_be_bytes([m[2], m[3], m[4], m[5]])).max(1);
    let old = { let _m = crate::arch::IrqMask::new(); UD_CLIENT.lock().as_ref().map(|c| c.xid()).unwrap_or(0) };
    let was = UD_STATE.load(Ordering::Relaxed);
    UD_XID_NEXT.store(xid, Ordering::Relaxed);
    UD_STATE.store(UD_IDLE, Ordering::Relaxed);
    UD_RETX.store(0, Ordering::Relaxed);
    UD_ACQ_T0.store(0, Ordering::Relaxed);
    UD_NEXT.store(now + UD_RESTART_MS, Ordering::Relaxed);
    UD_PSTATE.store(UP_NONE, Ordering::Relaxed);
    UD_PTRIES.store(0, Ordering::Relaxed);
    ud_probe_reset();
    UD_BIG.store(UB_NONE, Ordering::Relaxed);
    UD_RESTART_AT.store(now, Ordering::Relaxed);
    UD_RESTART_SHOW.store(true, Ordering::Relaxed);
    let n = UD_RESTARTS.fetch_add(1, Ordering::Relaxed) + 1;
    UD_LU_BCAST.store(UD_C_BCAST.load(Ordering::Relaxed), Ordering::Relaxed);
    UD_LU_MCAST.store(UD_C_MCAST.load(Ordering::Relaxed), Ordering::Relaxed);
    UD_LU_OURS.store(UD_C_OURS.load(Ordering::Relaxed), Ordering::Relaxed);
    UD_LU_FOREIGN.store(UD_C_FOREIGN.load(Ordering::Relaxed), Ordering::Relaxed);
    UD_LU_HEARD.store(UD_HEARD.load(Ordering::Relaxed), Ordering::Relaxed);
    serial_println!("[dhcp] link-up -> restart xid={:08x} backoff={} discover_in_ms={} was={} old_xid={:08x} restarts={}",
        xid, UD_RETX_MIN_MS, UD_RESTART_MS, ud_state_name(was), old, n);
    ud_filter_line("link-up");
}

/// `[dhcp] rx-filter`: the RX filter as programmed and read back, and what arrived by class (boot and since link-up).
fn ud_filter_line(at: &str) {
    let rx = RX_CTL_RB.load(Ordering::Relaxed);
    let (b, m, o, f) = (UD_C_BCAST.load(Ordering::Relaxed), UD_C_MCAST.load(Ordering::Relaxed), UD_C_OURS.load(Ordering::Relaxed), UD_C_FOREIGN.load(Ordering::Relaxed));
    let h = UD_HEARD.load(Ordering::Relaxed);
    serial_println!(
        "[dhcp] rx-filter at={} kind={} rxctl={:#06x} start={} ab={} pro={} amall={} am={} ecm_filter={} boot: bcast={} mcast={} ucast={} foreign={} dhcp_rx={} link: bcast={} mcast={} ucast={} foreign={} dhcp_rx={}",
        at, kind_name(), rx, (rx >> 7) & 1, (rx >> 3) & 1, rx & 1, (rx >> 1) & 1, (rx >> 4) & 1,
        if kind() == KIND_ECM { if FILTER_OK.load(Ordering::Relaxed) { "ok" } else { "refused" } } else { "n/a" },
        b, m, o, f, h,
        b.saturating_sub(UD_LU_BCAST.load(Ordering::Relaxed)), m.saturating_sub(UD_LU_MCAST.load(Ordering::Relaxed)),
        o.saturating_sub(UD_LU_OURS.load(Ordering::Relaxed)), f.saturating_sub(UD_LU_FOREIGN.load(Ordering::Relaxed)),
        h.saturating_sub(UD_LU_HEARD.load(Ordering::Relaxed))
    );
}

/// `[dhcp] tx-audit`: our DISCOVER, every field as it sits in the buffer handed to TXQ, and the first RFC check
/// that fails (`fail=none` = every check passed).
fn ud_tx_audit(f: &[u8]) {
    let Some(a) = net::dhcp::audit_request(f) else {
        serial_println!("[dhcp] tx-audit len={} -> short", f.len());
        return;
    };
    serial_println!(
        "[dhcp] tx-audit eth dst={} src={} type={:#06x} ; ip ver_ihl={:#04x} tos={:#04x} len={} id={} frag={:#06x} ttl={} proto={} csum={:#06x} csum_ok={} src={} dst={} ; udp {}->{} len={} csum={:#06x} udpsum={}",
        UdMac(a.eth_dst), UdMac(a.eth_src), a.ethertype, a.ip_ver_ihl, a.ip_tos, a.ip_len, a.ip_id, a.ip_frag, a.ip_ttl,
        a.ip_proto, a.ip_csum, a.ip_csum_ok as u8, UdIp(a.ip_src), UdIp(a.ip_dst), a.udp_sport, a.udp_dport, a.udp_len,
        a.udp_csum, match a.udp_csum_verdict { 0 => "none", 1 => "ok", _ => "BAD" }
    );
    let host = core::str::from_utf8(&a.host[..a.host_len]).unwrap_or("?");
    serial_println!(
        "[dhcp] tx-audit bootp op={} htype={} hlen={} hops={} xid={:08x} secs={} flags={:#06x} ciaddr={} chaddr={} chaddr_is_src={} cookie={} bootp_len={} ; opts={} type={} client_id={} host={} params={} end={} -> rfc2131 fail={}",
        a.op, a.htype, a.hlen, a.hops, a.xid, a.secs, a.flags, UdIp(a.ciaddr), UdMac(a.chaddr), (a.chaddr == a.eth_src) as u8,
        if a.cookie_ok { "ok" } else { "BAD" }, a.bootp_len, ud_list(&a.opts[..a.n_opts.min(16)]), net::dhcp::msg_name(a.msg_type),
        if a.client_id_ok { "ok" } else { "absent" }, if a.host_len == 0 { "-" } else { host }, ud_list(&a.params[..a.n_params]),
        if a.end_ok { "ok" } else { "BAD" }, a.fail
    );
}

/// `[dhcp] rx`: one row per UDP 67/68 frame the tap sees, whoever it is for.
fn ud_heard(f: &[u8], dst: &str) {
    let Some(h) = net::dhcp::hear(f) else { return };
    let k = UD_HEARD.fetch_add(1, Ordering::Relaxed) + 1;
    let ours = { let _m = crate::arch::IrqMask::new(); UD_CLIENT.lock().as_ref().map(|c| c.xid() == h.xid && h.xid != 0).unwrap_or(false) };
    serial_println!(
        "[dhcp] rx n={} {}:{} -> {}:{} from={} dst={} op={} type={} xid={:08x} ours={} flags={:#06x} yiaddr={} chaddr={} chaddr_is_us={} server={} lease_s={} bootp_len={} udpsum={}",
        k, UdIp(h.ip_src), h.sport, UdIp(h.ip_dst), h.dport, UdMac(h.eth_src), dst, h.op, net::dhcp::msg_name(h.msg_type), h.xid,
        ours as u8, h.flags, UdIp(h.yiaddr), UdMac(h.chaddr), (h.chaddr == mac()) as u8, UdIp(h.server_id), h.lease_s, h.bootp_len,
        match h.udp_csum_verdict { 0 => "none", 1 => "ok", _ => "BAD" }
    );
}

/// After the 60-byte probe's answer: the same who-has padded to `UB_LEN`; its verdict after `UD_PROBE_WAIT_MS`.
fn ud_big_tick(now: u64) {
    match UD_BIG.load(Ordering::Relaxed) {
        UB_DUE => {
            let ip = UD_PROBE_IP.load(Ordering::Relaxed);
            let mut fr = [0u8; UB_LEN];
            fr[..60].copy_from_slice(&u8_probe_frame(mac(), ip));
            if !ud_push(&fr) { return; }
            UD_BIG_AT.store(now, Ordering::Relaxed);
            UD_BIG.store(UB_SENT, Ordering::Relaxed);
        }
        UB_SENT if now.saturating_sub(UD_BIG_AT.load(Ordering::Relaxed)) >= UD_PROBE_WAIT_MS => {
            UD_BIG.store(UB_DONE, Ordering::Relaxed);
            serial_println!("[dhcp] tx-size probe len={} who_has={} reply_ms=none -> large-tx lost (the 60-byte probe was answered)",
                UB_LEN, UdIp(UD_PROBE_IP.load(Ordering::Relaxed).to_be_bytes()));
        }
        _ => {}
    }
}
/// An ARP reply from the probed address to our MAC: the 60-byte probe's answer arms the large one; the large one's
/// answer is its verdict.
fn ud_big_rx(f: &[u8], op: u16, dst: &str) {
    let want = UD_PROBE_IP.load(Ordering::Relaxed);
    if op != 2 || want == 0 || u32::from_be_bytes([f[28], f[29], f[30], f[31]]) != want || f[32..38] != mac() { return; }
    match UD_BIG.load(Ordering::Relaxed) {
        UB_NONE => UD_BIG.store(UB_DUE, Ordering::Relaxed),
        UB_SENT => {
            UD_BIG.store(UB_DONE, Ordering::Relaxed);
            serial_println!("[dhcp] tx-size probe len={} who_has={} reply_ms={} dst={} -> large-tx ok", UB_LEN, UdIp(want.to_be_bytes()),
                crate::arch::ms().saturating_sub(UD_BIG_AT.load(Ordering::Relaxed)), dst);
        }
        _ => {}
    }
}

/// Pump the data pass until `done` or `ms` elapse; true when `done` held.
fn ud_pump(ms: u64, done: impl Fn() -> bool) -> bool {
    let t0 = crate::arch::ms();
    while crate::arch::ms().saturating_sub(t0) < ms && is_up() && !done() {
        main_pass();
        for _ in 0..256 { core::hint::spin_loop(); }
    }
    done()
}

/// `tests dhcprestart`: the verb — a simulated link-down/up (the client restarts exactly as at a PHY link-up).
pub fn dhcp_restart_verb() {
    if !is_up() {
        serial_println!("[dhcp] sim link-down/up -> SKIP reason=no-dongle");
        return;
    }
    serial_println!("[dhcp] sim link-down/up state={}", ud_state_name(UD_STATE.load(Ordering::Relaxed)));
    ud_link_up(crate::arch::ms());
}

/// `tests dhcp` (QEMU user-net answers DHCP): (1) the lease — the boot's, or wait up to 10 s for it; (2) a
/// simulated link-down/up: the restart's DISCOVER inside 1 s with back-off 4000 under a new xid, then DISCOVER ->
/// ACK inside 2 s and the same lease again.
/// `:: DHCP: lease ip= mask= gw= dns= lease_s= dora_ms= ; restart xid=<old>-><new> backoff= discover_ms= dora_ms= ip= -> PASS ::`
pub fn dhcp_selftest() {
    if !is_up() {
        serial_println!(":: DHCP: lease ip=0.0.0.0 reason=no-dongle -> SKIP ::");
        return;
    }
    let bound = || UD_STATE.load(Ordering::Relaxed) == UD_BOUND;
    let b1 = ud_pump(10_000, bound);
    let ip1 = ud_ip(&UD_IP);
    let dora1 = UD_DORA_MS.load(Ordering::Relaxed);
    serial_println!("[dhcp] fixture lease bound={} ip={} mask={} gw={} dns={} lease_s={} dora_ms={}", b1 as u8, UdIp(ip1),
        UdIp(ud_ip(&UD_MASK)), UdIp(ud_ip(&UD_GW)), UdIp(ud_ip(&UD_DNS)), UD_LEASE_S.load(Ordering::Relaxed), dora1);
    let old = { let _m = crate::arch::IrqMask::new(); UD_CLIENT.lock().as_ref().map(|c| c.xid()).unwrap_or(0) };
    let d0 = UD_DISCOVERS.load(Ordering::Relaxed);
    dhcp_restart_verb();
    let at = UD_RESTART_AT.load(Ordering::Relaxed);
    let sent = ud_pump(1_500, || UD_DISCOVERS.load(Ordering::Relaxed) > d0);
    let disc_ms = if sent { UD_T_DISC.load(Ordering::Relaxed).saturating_sub(at) } else { u64::MAX };
    let new = { let _m = crate::arch::IrqMask::new(); UD_CLIENT.lock().as_ref().map(|c| c.xid()).unwrap_or(0) };
    let backoff = UD_LAST_RETX.load(Ordering::Relaxed);
    let b2 = sent && ud_pump(UD_DORA_MAX_MS + 1_000, bound);
    let dora2 = UD_DORA_MS.load(Ordering::Relaxed);
    let ip2 = ud_ip(&UD_IP);
    let ok = b1 && dora1 <= UD_DORA_MAX_MS && sent && disc_ms <= 1_000 && backoff == UD_RETX_MIN_MS && new != old
        && b2 && dora2 <= UD_DORA_MAX_MS && ip2 != [0; 4];
    serial_println!(
        ":: DHCP: lease ip={} mask={} gw={} dns={} lease_s={} dora_ms={} ; restart xid={:08x}->{:08x} backoff={} discover_ms={} dora_ms={} ip={} -> {} ::",
        UdIp(ip1), UdIp(ud_ip(&UD_MASK)), UdIp(ud_ip(&UD_GW)), UdIp(ud_ip(&UD_DNS)), UD_LEASE_S.load(Ordering::Relaxed), dora1,
        old, new, backoff, if sent { alloc::format!("{}", disc_ms) } else { alloc::string::String::from("none") },
        if b2 { alloc::format!("{}", dora2) } else { alloc::string::String::from("none") }, UdIp(ip2), if ok { "PASS" } else { "FAIL" }
    );
}

// HELDMAP (rmbp-ledger B838, docs/dev/blueprint/power.md BP-power-7) — FILE-TAIL append under the `pub mod suspend` gate.
/// `usbnet_ax_poll` on a held slot: the PHY read was REFUSED (`Ep0Err::Held`), not answered, so the link word is left as it
/// was and only the next poll is scheduled, at the cadence that word implies (`link_seen`'s 2000 / 250 ms) — otherwise
/// `poll_due` would stay true and the refusal would repeat on every pump pass.
#[cfg(all(target_arch = "x86_64", feature = "lidsleep", feature = "wc"))]
pub fn link_poll_held(now_ms: u64) {
    POLL_NEXT.store(now_ms + if LINK.load(Ordering::Relaxed) { 2000 } else { 250 }, Ordering::Relaxed);
}
