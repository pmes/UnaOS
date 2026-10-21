//! CHARTER: Comscan — driver (Bluetooth; AVCTP and the AVRCP target ride BTHID's link and L2CAP channels, B339; the transport it drives is the Player's media-key door, the volume the one master model's through `audio::bt_master`)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AVRCP (rmbp-ledger B774) — AVCTP on PSM 0x0017 and the AVRCP 1.4 TARGET a headset's buttons talk to.
//!
//! THE LAYERS. The link, its security and the L2CAP channel are the BTHID host's (`drivers/ehci/bthid.rs`); an inbound
//! channel on PSM 0x0017 is accepted by [`super::l2cap::inbound_served`] and its PDUs reach [`Bt::avrcp_rx`] from
//! `on_acl`. This module is a child of `bthid` (declared at its tail beside `l2cap`, `sdp`, `avdtp`): the same `BT`
//! lock, nothing stepped of its own. The SDP record that names it is [`super::sdp::records`] (0x110C, features 0x0003).
//!
//! * AVCTP: one 3-byte header — transaction label (4 bits), packet type (single only; a fragmented packet is dropped),
//!   C/R, IPID — then the profile ID 0x110E. A response echoes the command's label; another PID is answered with the
//!   header alone, IPID set ([`header`], [`frame`]).
//! * AVRCP TARGET, AV/C panel subunit ([`target`], pure): PASS THROUGH play / pause / stop / forward / backward /
//!   volume up / volume down (acted on the press, the release acked); UNIT INFO and SUBUNIT INFO; the vendor-dependent
//!   PDUs GetCapabilities (company ID, events {volume changed}), SetAbsoluteVolume and RegisterNotification
//!   (volume changed: INTERIM now, one CHANGED on the kept label at the next AVRCP volume change, [`VOL_NOTE`]).
//!   Everything else is refused: NOT IMPLEMENTED (other opcodes and pass-through ops), REJECTED (other vendor PDUs,
//!   other events). Browsing (PSM 0x001B), the controller role and metadata are not built.
//! * THE EFFECTS ([`apply`]): play/pause/next/prev go to the Player's media-key door (`video::player::media_key`, the
//!   F7/F8/F9 router; atomics only, so safe under `BT`) — PLAY flips only a stream that is not playing, PAUSE and STOP
//!   only one that is (`hda::play::busy`/`paused`, read); the volume goes to the one master model through
//!   `audio::bt_master` (0..=127 onto the 0..=16 level).
//!
//! Locks: `BT`, then [`VOL_NOTE`] alone (never with `avdtp`'s `A2` or `ACP_DUE`); `audio::bt_master` reaches
//! `hda::vol`'s ring lock, which never takes `BT`. Wire: `[avrcp] cmd=<op> from=<addr> -> <play-pause|next|prev|none|
//! volume=<n>|refused=<why>>`; `tests a2dp`: `avrcp=<ok|FAIL>` at the END of the `:: A2DP: … ::` verdict ([`legs`]).
//! Compiled only under the `a2dp` knob.

use super::{fmt_addr, Bt};
use alloc::vec::Vec;

/// AVCTP's PSM (the control channel; browsing's 0x001B is not served).
pub use super::l2cap::PSM_AVCTP;

/// The AV Remote Control profile ID an AVCTP frame names.
const PID_AVRCP: u16 = 0x110E;
/// The Bluetooth SIG company ID AVRCP's vendor-dependent PDUs carry.
const BT_SIG: [u8; 3] = [0x00, 0x19, 0x58];
/// AV/C subunit: panel (type 9), ID 0.
const PANEL: u8 = 0x48;

// AV/C command types and response codes.
const CONTROL: u8 = 0x0;
const STATUS: u8 = 0x1;
const NOTIFY: u8 = 0x3;
const NOT_IMPLEMENTED: u8 = 0x8;
const ACCEPTED: u8 = 0x9;
const REJECTED: u8 = 0xA;
const STABLE: u8 = 0xC;
const CHANGED: u8 = 0xD;
const INTERIM: u8 = 0xF;

// AV/C opcodes.
const VENDOR: u8 = 0x00;
const UNIT_INFO: u8 = 0x30;
const SUBUNIT_INFO: u8 = 0x31;
const PASS_THROUGH: u8 = 0x7C;

// Pass-through operation IDs.
const OP_VOL_UP: u8 = 0x41;
const OP_VOL_DOWN: u8 = 0x42;
const OP_PLAY: u8 = 0x44;
const OP_STOP: u8 = 0x45;
const OP_PAUSE: u8 = 0x46;
const OP_FORWARD: u8 = 0x4B;
const OP_BACKWARD: u8 = 0x4C;

// AVRCP PDU IDs and the one event served.
const GET_CAPS: u8 = 0x10;
const REGISTER_NOTE: u8 = 0x31;
const SET_ABS_VOL: u8 = 0x50;
const EV_VOLUME: u8 = 0x0D;

// AVRCP status codes (REJECTED's one parameter byte).
const E_INVALID_CMD: u8 = 0x00;
const E_INVALID_PARAM: u8 = 0x01;

/// RegisterNotification(volume changed) a controller left open: (its address, the transaction label of the INTERIM).
static VOL_NOTE: crate::sync::Mutex<Option<([u8; 6], u8)>> = crate::sync::Mutex::new(None);

/// An AVCTP single-packet header: (label, is_response, PID), or None for a fragment or a short packet.
pub fn header(b: &[u8]) -> Option<(u8, bool, u16)> {
    let h = *b.first()?;
    if b.len() < 3 || (h >> 2) & 0x3 != 0 {
        return None;
    }
    Some((h >> 4, h & 0x2 != 0, u16::from_be_bytes([b[1], b[2]])))
}

/// A single-packet AVCTP response on `label` carrying the AV/C frame `avc` (empty with `ipid`: the PID was not ours).
pub fn frame(label: u8, ipid: bool, pid: u16, avc: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(3 + avc.len());
    v.push((label << 4) | 0x2 | ipid as u8);
    v.extend_from_slice(&pid.to_be_bytes());
    v.extend_from_slice(avc);
    v
}

/// What an answered command asks the host to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    None,
    /// The Player's play/pause key.
    PlayPause,
    Next,
    Prev,
    /// Set the master to this absolute volume (0..=127).
    Volume(u8),
    /// One master level up (true) or down.
    VolStep(bool),
}

/// The target's answer to one AVCTP PDU.
pub struct Reply {
    pub rsp: Vec<u8>,
    pub op: &'static str,
    pub act: Act,
    /// A RegisterNotification(volume changed) answered INTERIM on this label.
    pub note: Option<u8>,
    /// Why it was refused (the wire's `refused=`).
    pub refused: Option<&'static str>,
}

/// A vendor-dependent AV/C frame: header, company ID, PDU ID, packet type (single), the parameter length and `params`.
fn vendor(rsp: u8, pdu: u8, params: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(10 + params.len());
    v.extend_from_slice(&[rsp, PANEL, VENDOR]);
    v.extend_from_slice(&BT_SIG);
    v.extend_from_slice(&[pdu, 0x00]);
    v.extend_from_slice(&(params.len() as u16).to_be_bytes());
    v.extend_from_slice(params);
    v
}

/// The CHANGED that closes a kept RegisterNotification(volume changed) on `label`, at `abs`.
pub fn changed(label: u8, abs: u8) -> Vec<u8> {
    frame(label, false, PID_AVRCP, &vendor(CHANGED, REGISTER_NOTE, &[EV_VOLUME, abs & 0x7F]))
}

/// The AVRCP target's answer to `pdu` (one AVCTP PDU) while the Player is `playing` and the master is `abs` (0..=127).
/// None for a PDU nothing answers: a fragment, a response, a header too short.
pub fn target(pdu: &[u8], playing: bool, abs: u8) -> Option<Reply> {
    let (label, rsp, pid) = header(pdu)?;
    if rsp {
        return None;
    }
    let mut r = Reply { rsp: Vec::new(), op: "unknown", act: Act::None, note: None, refused: None };
    if pid != PID_AVRCP {
        r.rsp = frame(label, true, pid, &[]);
        r.refused = Some("pid");
        return Some(r);
    }
    let avc = &pdu[3..];
    if avc.len() < 3 {
        r.refused = Some("short");
        return Some(r);
    }
    let (ctype, opcode) = (avc[0] & 0x0F, avc[2]);
    let avc_rsp: Vec<u8> = match opcode {
        PASS_THROUGH if avc.len() >= 5 && ctype == CONTROL => {
            let (released, op) = (avc[3] & 0x80 != 0, avc[3] & 0x7F);
            let (name, act) = match op {
                OP_PLAY => ("play", if playing { Act::None } else { Act::PlayPause }),
                OP_PAUSE => ("pause", if playing { Act::PlayPause } else { Act::None }),
                OP_STOP => ("stop", if playing { Act::PlayPause } else { Act::None }),
                OP_FORWARD => ("next", Act::Next),
                OP_BACKWARD => ("prev", Act::Prev),
                OP_VOL_UP => ("vol-up", Act::VolStep(true)),
                OP_VOL_DOWN => ("vol-down", Act::VolStep(false)),
                _ => ("pass-through", Act::None),
            };
            r.op = name;
            let known = name != "pass-through";
            if known && !released {
                r.act = act;
            }
            if !known {
                r.refused = Some("not-implemented");
            }
            let mut v = avc.to_vec();
            v[0] = if known { ACCEPTED } else { NOT_IMPLEMENTED };
            v
        }
        UNIT_INFO if ctype == STATUS => {
            r.op = "unit-info";
            alloc::vec![STABLE, 0xFF, UNIT_INFO, 0x07, PANEL, BT_SIG[0], BT_SIG[1], BT_SIG[2]]
        }
        SUBUNIT_INFO if ctype == STATUS => {
            r.op = "subunit-info";
            alloc::vec![STABLE, 0xFF, SUBUNIT_INFO, 0x07, PANEL, 0xFF, 0xFF, 0xFF]
        }
        VENDOR if avc.len() >= 10 && avc[3..6] == BT_SIG => vendor_pdu(avc, ctype, abs, label, &mut r),
        _ => {
            r.refused = Some("not-implemented");
            let mut v = avc.to_vec();
            v[0] = NOT_IMPLEMENTED;
            v
        }
    };
    r.rsp = frame(label, false, PID_AVRCP, &avc_rsp);
    Some(r)
}

/// A vendor-dependent AVRCP PDU (`avc` is the whole AV/C frame, company ID checked).
fn vendor_pdu(avc: &[u8], ctype: u8, abs: u8, label: u8, r: &mut Reply) -> Vec<u8> {
    let (pdu, len) = (avc[6], u16::from_be_bytes([avc[8], avc[9]]) as usize);
    let p = avc.get(10..10 + len).unwrap_or(&[]);
    let reject = |r: &mut Reply, code: u8, why: &'static str| {
        r.refused = Some(why);
        vendor(REJECTED, pdu, &[code])
    };
    if avc[7] != 0 || p.len() != len {
        r.op = "vendor";
        return reject(r, E_INVALID_CMD, "fragmented");
    }
    match pdu {
        GET_CAPS if ctype == STATUS && !p.is_empty() => {
            r.op = "get-caps";
            match p[0] {
                0x02 => vendor(STABLE, pdu, &[0x02, 0x01, BT_SIG[0], BT_SIG[1], BT_SIG[2]]),
                0x03 => vendor(STABLE, pdu, &[0x03, 0x01, EV_VOLUME]),
                _ => reject(r, E_INVALID_PARAM, "capability"),
            }
        }
        SET_ABS_VOL if ctype == CONTROL && p.len() == 1 => {
            r.op = "set-volume";
            let v = p[0] & 0x7F;
            r.act = Act::Volume(v);
            vendor(ACCEPTED, pdu, &[v])
        }
        REGISTER_NOTE if ctype == NOTIFY && p.len() == 5 => {
            r.op = "register-volume";
            if p[0] != EV_VOLUME {
                return reject(r, E_INVALID_PARAM, "event");
            }
            r.note = Some(label);
            vendor(INTERIM, pdu, &[EV_VOLUME, abs & 0x7F])
        }
        _ => {
            r.op = "vendor";
            reject(r, E_INVALID_CMD, "pdu")
        }
    }
}

/// The master level (0..=16) an absolute volume (0..=127) lands on, and back; every level round-trips.
pub fn level_of(abs: u8) -> u8 {
    (((abs.min(127) as u32) * 16 + 63) / 127) as u8
}

pub fn abs_of(level: u8) -> u8 {
    (((level.min(16) as u32) * 127 + 8) / 16) as u8
}

/// The Player has a stream that is running and not paused (`hda::play`'s atomics).
fn playing() -> bool {
    #[cfg(feature = "hda-tone")]
    return crate::drivers::hda::play::busy() && !crate::drivers::hda::play::paused();
    #[cfg(not(feature = "hda-tone"))]
    false
}

/// The master as an absolute volume (0..=127); 0 on an image with no audio seam (no master to read).
fn master() -> u8 {
    #[cfg(any(feature = "uvc", feature = "hda-tone"))]
    return crate::audio::bt_master(None);
    #[cfg(not(any(feature = "uvc", feature = "hda-tone")))]
    0
}

/// Set the master to `abs` (0..=127) through the one master fn; the absolute volume now in force.
fn set_master(abs: u8) -> u8 {
    #[cfg(any(feature = "uvc", feature = "hda-tone"))]
    return crate::audio::bt_master(Some(abs));
    #[cfg(not(any(feature = "uvc", feature = "hda-tone")))]
    {
        let _ = abs;
        0
    }
}

/// The Player's media key (0 prev, 1 play/pause, 2 next): the latch its pass reads (an image with no Player: nothing).
fn media(k: u8) {
    #[cfg(feature = "wc")]
    crate::video::player::media_key(k);
    let _ = k;
}

/// Carry out `act`: the Player's media key or the master. The wire's tail, and the new absolute volume when it moved.
fn apply(act: Act) -> (alloc::string::String, Option<u8>) {
    use alloc::string::ToString;
    match act {
        Act::None => ("none".to_string(), None),
        Act::PlayPause => {
            media(1);
            ("play-pause".to_string(), None)
        }
        Act::Next => {
            media(2);
            ("next".to_string(), None)
        }
        Act::Prev => {
            media(0);
            ("prev".to_string(), None)
        }
        Act::Volume(a) => {
            let v = set_master(a);
            (alloc::format!("volume={}", v), Some(v))
        }
        Act::VolStep(up) => {
            let l = level_of(master());
            let v = set_master(abs_of(if up { (l + 1).min(16) } else { l.saturating_sub(1) }));
            (alloc::format!("volume={}", v), Some(v))
        }
    }
}

impl Bt {
    /// A PDU on an AVCTP channel: the target's answer on that channel, its effect, and the CHANGED a kept
    /// RegisterNotification is owed when the volume moved.
    pub(super) fn avrcp_rx(&mut self, li: usize, ci: usize, body: &[u8]) {
        let (addr, h, rcid) = (self.links[li].addr, self.links[li].handle, self.links[li].chans[ci].rcid);
        let Some(r) = target(body, playing(), master()) else { return };
        if !r.rsp.is_empty() {
            self.l2_send(h, rcid, &r.rsp);
        }
        if let Some(why) = r.refused {
            serial_println!("[avrcp] cmd={} from={} -> refused={}", r.op, fmt_addr(&addr), why);
            return;
        }
        let (tail, moved) = apply(r.act);
        serial_println!("[avrcp] cmd={} from={} -> {}", r.op, fmt_addr(&addr), tail);
        if let Some(label) = r.note {
            *VOL_NOTE.lock() = Some((addr, label));
        } else if let Some(v) = moved {
            let kept = VOL_NOTE.lock().take();
            if let Some((a, label)) = kept {
                if a == addr {
                    self.l2_send(h, rcid, &changed(label, v));
                }
            }
        }
    }
}

/// `tests a2dp` `avrcp` leg — a synthetic controller: PLAY (press then release), PAUSE while playing, an unknown
/// pass-through op, a foreign PID, UNIT INFO, GetCapabilities(events), RegisterNotification(volume), SetAbsoluteVolume
/// 50 %, each answered byte for byte; then the effects for real: the Player's play/pause key latched, the master at 8/16.
pub fn legs() -> Result<(), &'static str> {
    let is = |pdu: &[u8], playing: bool, want: &[u8], act: Act, leg: &'static str| -> Result<Reply, &'static str> {
        let r = target(pdu, playing, 95).ok_or(leg)?;
        if r.rsp != want || r.act != act {
            return Err(leg);
        }
        Ok(r)
    };
    is(&[0x10, 0x11, 0x0E, 0x00, 0x48, 0x7C, 0x44, 0x00], false, &[0x12, 0x11, 0x0E, 0x09, 0x48, 0x7C, 0x44, 0x00], Act::PlayPause, "play")?;
    is(&[0x20, 0x11, 0x0E, 0x00, 0x48, 0x7C, 0xC4, 0x00], false, &[0x22, 0x11, 0x0E, 0x09, 0x48, 0x7C, 0xC4, 0x00], Act::None, "play-release")?;
    is(&[0x30, 0x11, 0x0E, 0x00, 0x48, 0x7C, 0x46, 0x00], true, &[0x32, 0x11, 0x0E, 0x09, 0x48, 0x7C, 0x46, 0x00], Act::PlayPause, "pause")?;
    is(&[0x30, 0x11, 0x0E, 0x00, 0x48, 0x7C, 0x46, 0x00], false, &[0x32, 0x11, 0x0E, 0x09, 0x48, 0x7C, 0x46, 0x00], Act::None, "pause-idle")?;
    is(&[0x40, 0x11, 0x0E, 0x00, 0x48, 0x7C, 0x40, 0x00], false, &[0x42, 0x11, 0x0E, 0x08, 0x48, 0x7C, 0x40, 0x00], Act::None, "not-implemented")?;
    is(&[0x50, 0x11, 0x0F, 0x00, 0x48, 0x7C, 0x44, 0x00], false, &[0x53, 0x11, 0x0F], Act::None, "ipid")?;
    is(&[0x60, 0x11, 0x0E, 0x01, 0xFF, 0x30, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], false, &[0x62, 0x11, 0x0E, 0x0C, 0xFF, 0x30, 0x07, 0x48, 0x00, 0x19, 0x58], Act::None, "unit-info")?;
    is(&[0x70, 0x11, 0x0E, 0x01, 0x48, 0x00, 0x00, 0x19, 0x58, 0x10, 0x00, 0x00, 0x01, 0x03], false, &[0x72, 0x11, 0x0E, 0x0C, 0x48, 0x00, 0x00, 0x19, 0x58, 0x10, 0x00, 0x00, 0x03, 0x03, 0x01, 0x0D], Act::None, "get-caps")?;
    let n = is(&[0x80, 0x11, 0x0E, 0x03, 0x48, 0x00, 0x00, 0x19, 0x58, 0x31, 0x00, 0x00, 0x05, 0x0D, 0x00, 0x00, 0x00, 0x00], false, &[0x82, 0x11, 0x0E, 0x0F, 0x48, 0x00, 0x00, 0x19, 0x58, 0x31, 0x00, 0x00, 0x02, 0x0D, 0x5F], Act::None, "register-volume")?;
    if n.note != Some(8) || changed(8, 0x40) != [0x82, 0x11, 0x0E, 0x0D, 0x48, 0x00, 0x00, 0x19, 0x58, 0x31, 0x00, 0x00, 0x02, 0x0D, 0x40] {
        return Err("changed");
    }
    is(&[0x90, 0x11, 0x0E, 0x00, 0x48, 0x00, 0x00, 0x19, 0x58, 0x50, 0x00, 0x00, 0x01, 0x40], false, &[0x92, 0x11, 0x0E, 0x09, 0x48, 0x00, 0x00, 0x19, 0x58, 0x50, 0x00, 0x00, 0x01, 0x40], Act::Volume(0x40), "set-volume")?;
    is(&[0xA0, 0x11, 0x0E, 0x01, 0x48, 0x00, 0x00, 0x19, 0x58, 0x20, 0x00, 0x00, 0x00], false, &[0xA2, 0x11, 0x0E, 0x0A, 0x48, 0x00, 0x00, 0x19, 0x58, 0x20, 0x00, 0x00, 0x01, 0x00], Act::None, "vendor-rejected")?;
    if target(&[0x14, 0x11, 0x0E, 0x00], false, 0).is_some() || (0..=16).any(|l| level_of(abs_of(l)) != l) || level_of(0x40) != 8 {
        return Err("fragment-or-map");
    }
    // The effects, for real: the Player's play/pause key (the latch its pass reads), and the master at 50 %.
    #[cfg(feature = "wc")]
    {
        let _ = crate::video::player::media_take();
        apply(Act::PlayPause);
        if crate::video::player::media_take() != 2 {
            return Err("transport");
        }
    }
    #[cfg(any(feature = "uvc", feature = "hda-tone"))]
    {
        let before = master();
        let (_, v) = apply(Act::Volume(0x40));
        let lv = level_of(master());
        apply(Act::Volume(before));
        serial_println!("[avrcp] fixture set-volume abs=64 -> master={}/16 (restored to abs={})", lv, before);
        if v != Some(abs_of(8)) || lv != 8 {
            return Err("master");
        }
    }
    Ok(())
}
