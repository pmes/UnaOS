//! CHARTER: Comscan — driver (Bluetooth; the radio is the EHCI controller's; the link, its L2CAP channels and the ACL pipes are BTHID's, B339; this file adds the decisions A2DP needed)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A2DP (rmbp-ledger B739) — L2CAP, the part A2DP had to add.
//!
//! L2CAP's connection-oriented channels ALREADY exist, in the BTHID host (`drivers/ehci/bthid.rs`): the signalling
//! channel (`Bt::sig`, `on_sig_cmd`: Connection Request/Response, Configure Request/Response with the MTU option and
//! the Basic-mode refusal, Disconnection, Echo, Information), the channel table (`Chan`, `open_chan`, `accept_chan`,
//! `maybe_open`), PDU reassembly from ACL fragments (`on_acl`) and the ACL transmit (`step`). So no
//! second L2CAP is written here. What A2DP needed from it, and what this file decides:
//! * an inbound channel on AVDTP's PSM 0x0019 is accepted beside HID's two, and one on SDP's PSM 0x0001 for the SDP
//!   server (SDPSERVER B769, [`super::sdp::serve`]) ([`inbound_served`]);
//! * AVDTP opens TWO channels on the same PSM — the first is signalling, the one opened after OPEN carries the
//!   stream ([`avdtp_role`]) — where the HID host allowed one channel per PSM;
//! * a PDU longer than one ACL packet goes out as FRAGMENTS (A2DPKNOB, rmbp-ledger B750; [`fragment`]): the first
//!   with Packet_Boundary 0b10, every continuation with 0b01 — the mirror of `on_acl`'s reassembly — queued back to
//!   back, all or none ([`Bt::l2_send_frag`]); a PDU that fits one packet is queued exactly as before;
//! * a media packet is bounded by the peer's MTU and TWO full ACL packets ([`payload_fit`]): 500 bytes on the BCM
//!   radio, so bitpool 53 and four frames a packet in two fragments.
//!
//! Compiled only under the `a2dp` knob (A2DPKNOB B750; it implies `btc`): a btc-only image carries none of it.

use super::{Bt, PSM_SDP};
use alloc::vec::Vec;

/// AVDTP's PSM (signalling and media both).
pub const PSM_AVDTP: u16 = 0x0019;

/// The largest ACL packet BTHID's transmit path writes (4-byte ACL header included).
pub const ACL_PKT_MAX: usize = 256;

/// An inbound Connection Request on `psm` is answered (HID control, HID interrupt, AVDTP, SDP — a device asking
/// about US is answered by the SDP server, SDPSERVER B769); any other is refused with "PSM not supported".
pub fn inbound_served(psm: u16) -> bool {
    matches!(psm, 0x0011 | 0x0013 | PSM_AVDTP | PSM_SDP)
}

/// The most one ACL packet carries on BTHID's transmit path: the controller's ACL buffer, and the 256-byte packet
/// bound (`step`'s `data_buf`) less the 4-byte ACL header.
pub fn acl_data_max(acl_len: u16) -> usize {
    (acl_len as usize).min(ACL_PKT_MAX - 4)
}

/// The largest L2CAP payload of one AVDTP PDU to a peer whose MTU is `peer_mtu`, on a controller whose ACL buffer is
/// `acl_len` bytes: min(peer MTU, two full ACL packets less the 4-byte basic header) — 500 on a 1021-byte buffer.
pub fn payload_fit(acl_len: u16, peer_mtu: u16) -> usize {
    (peer_mtu as usize).min((2 * acl_data_max(acl_len)).saturating_sub(4))
}

/// A queued ACL payload that CONTINUES an L2CAP PDU carries this bit in its handle word (bit 12, where the ACL
/// header's Packet_Boundary flag sits); [`acl_hdr`] turns it into PB = 0b01.
pub const CONT: u16 = 0x1000;

/// The ACL header's first word for a queued handle word: PB = 0b01 for a continuation fragment, else 0b10 (the first
/// packet of a PDU, automatically flushable); broadcast flag 0b00.
pub fn acl_hdr(h: u16) -> u16 {
    (h & 0x0FFF) | if h & CONT != 0 { 0b01 << 12 } else { 0b10 << 12 }
}

/// An L2CAP PDU of `pdu_len` bytes (basic header included) is longer than one ACL packet on a radio that is up.
pub fn needs_frag(pdu_len: usize, acl_len: u16) -> bool {
    acl_len > 0 && pdu_len > acl_data_max(acl_len)
}

/// The ACL packets an L2CAP PDU of `pdu_len` bytes (basic header included) takes.
pub fn frag_count(pdu_len: usize, acl_len: u16) -> usize {
    pdu_len.div_ceil(acl_data_max(acl_len).max(1)).max(1)
}

/// Outbound fragmentation: the basic header (length, `cid`) + `body` as the queued ACL payloads that carry it —
/// the first under `handle`, each continuation under `handle | CONT` — each at most [`acl_data_max`] bytes.
pub fn fragment(handle: u16, cid: u16, body: &[u8], acl_len: u16) -> Vec<(u16, Vec<u8>)> {
    let mut pdu = Vec::with_capacity(4 + body.len());
    pdu.extend_from_slice(&(body.len() as u16).to_le_bytes());
    pdu.extend_from_slice(&cid.to_le_bytes());
    pdu.extend_from_slice(body);
    let m = acl_data_max(acl_len).max(1);
    pdu.chunks(m).enumerate().map(|(i, c)| ((handle & 0x0FFF) | if i > 0 { CONT } else { 0 }, c.to_vec())).collect()
}

/// The transmit queue's bound, counted in ACL packets (BTHID's `l2_send` drops at 8 queued PDUs).
const ACLTX_MAX: usize = 8;

impl Bt {
    /// `l2_send` for a PDU longer than one ACL packet: its fragments queued back to back at the tail, all or none,
    /// so the FIFO transmit keeps them contiguous when credits run out mid-PDU.
    pub(super) fn l2_send_frag(&mut self, handle: u16, cid: u16, body: &[u8]) {
        let f = fragment(handle, cid, body, self.acl_len);
        if self.acltx.len() + f.len() > ACLTX_MAX {
            serial_println!(":: bthid: l2cap tx queue full — PDU of {} bytes ({} fragments) on cid={:#06x} dropped ::", 4 + body.len(), f.len(), cid);
            return;
        }
        self.acltx.extend(f);
    }
}

/// What an AVDTP channel is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AvdtpRole {
    Signalling,
    Media,
}

/// The first AVDTP channel between two devices is signalling; one opened while signalling is open is the media
/// transport (the stream that OPEN made).
pub fn avdtp_role(signalling_open: bool) -> AvdtpRole {
    if signalling_open { AvdtpRole::Media } else { AvdtpRole::Signalling }
}
