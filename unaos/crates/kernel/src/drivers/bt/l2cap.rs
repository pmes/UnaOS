//! CHARTER: Comscan — driver (Bluetooth; the radio is the EHCI controller's; the link, its L2CAP channels and the ACL pipes are BTHID's, B339; this file adds the decisions A2DP needed)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A2DP (rmbp-ledger B739) — L2CAP, the part A2DP had to add.
//!
//! L2CAP's connection-oriented channels ALREADY exist, in the BTHID host (`drivers/ehci/bthid.rs`): the signalling
//! channel (`Bt::sig`, `on_sig_cmd`: Connection Request/Response, Configure Request/Response with the MTU option and
//! the Basic-mode refusal, Disconnection, Echo, Information), the channel table (`Chan`, `open_chan`, `accept_chan`,
//! `maybe_open`), PDU reassembly from ACL fragments (`on_acl`) and one PDU per ACL packet on transmit (`step`). So no
//! second L2CAP is written here. What A2DP needed from it, and what this file decides:
//! * an inbound channel on AVDTP's PSM 0x0019 is accepted beside HID's two ([`inbound_served`]);
//! * AVDTP opens TWO channels on the same PSM — the first is signalling, the one opened after OPEN carries the
//!   stream ([`avdtp_role`]) — where the HID host allowed one channel per PSM;
//! * a media packet must fit ONE ACL packet the transmit path writes ([`payload_fit`]): BTHID's `step` refuses a PDU
//!   whose ACL packet exceeds 256 bytes or the controller's ACL buffer, so the media payload is bounded by the
//!   smaller of the peer's MTU, the controller's buffer and 248 bytes. Outbound fragmentation is not built (owed).

/// AVDTP's PSM (signalling and media both).
pub const PSM_AVDTP: u16 = 0x0019;

/// The largest ACL packet BTHID's transmit path writes (4-byte ACL header included).
pub const ACL_PKT_MAX: usize = 256;

/// An inbound Connection Request on `psm` is answered (HID control, HID interrupt, AVDTP); any other is refused
/// with "PSM not supported" — this host runs no SDP server.
pub fn inbound_served(psm: u16) -> bool {
    matches!(psm, 0x0011 | 0x0013 | PSM_AVDTP)
}

/// The largest L2CAP payload one ACL packet carries to a peer whose MTU is `peer_mtu`, on a controller whose ACL
/// buffer is `acl_len` bytes: min(peer MTU, acl_len - 4, 256 - 8).
pub fn payload_fit(acl_len: u16, peer_mtu: u16) -> usize {
    (peer_mtu as usize).min((acl_len as usize).saturating_sub(4)).min(ACL_PKT_MAX - 8)
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
