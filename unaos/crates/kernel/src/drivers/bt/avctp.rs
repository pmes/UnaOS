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
//!   other events). Browsing (PSM 0x001B) and metadata are not built.
//! * AVRCP CONTROLLER (AVRCPCT, rmbp-ledger B780; [`Ct`], pure): on the A2DP headset's AVCTP channel (opened by either
//!   side; ours after [`super::avdtp::ACP_WAIT_MS`] once the stream is up and the headset opened none, [`ct_arm`]) the CT
//!   sends RegisterNotification(volume changed) and SetAbsoluteVolume, on its own labels beside the TG's; the
//!   headset's CHANGED moves the master (then re-registers), a refused SetAbsoluteVolume stops the pushes.
//! * THE WATCH ([`Watch::step`], once per BTHID pass from `avdtp::tick`): the master read each pass; when it moved, a
//!   kept TG RegisterNotification gets its CHANGED (every writer: F-keys, the slider, Settings, AVRCP) and the CT pushes
//!   SetAbsoluteVolume unless the headset itself set that level. While the headset has accepted one, it carries the
//!   level and the mixer plays BT at unity ([`ct_volume`]; mute stays the mixer's too).
//! * MUTE ON THE WIRE (AVRCPMUTE, rmbp-ledger B790): AVRCP 1.6 §6.13.2 has no mute, so a muted master is
//!   SetAbsoluteVolume(0) through the same one-in-flight path ([`Watch::step`]'s wire value) and the level again on
//!   unmute; the level is the master's own (kept across the mute). A headset CHANGED while muted moves that kept level
//!   and sends nothing ([`Watch::heard`]); `ct_volume` stays true while muted.
//! * THE EFFECTS ([`apply`]): play/pause/next/prev go to the Player's media-key door (`video::player::media_key`, the
//!   F7/F8/F9 router; atomics only, so safe under `BT`) — PLAY flips only a stream that is not playing, PAUSE and STOP
//!   only one that is (`hda::play::busy`/`paused`, read); the volume goes to the one master model through
//!   `audio::bt_master` (0..=127 onto the 0..=16 level).
//!
//! Locks: `BT`, then [`VOL_NOTE`] alone or [`WATCH`] alone (never both, never with `avdtp`'s `A2` or `ACP_DUE`); `audio::bt_master` reaches
//! `hda::vol`'s ring lock, which never takes `BT`. Wire: `[avrcp] cmd=<op> from=<addr> -> <play-pause|next|prev|none|
//! volume=<n>|refused=<why>>` · `[avrcp] ct cmd=<set-volume|register> to=<addr> -> <sent[ volume=<n>[ muted=1]]|interim volume=<n>|
//! changed volume=<n>|accepted volume=<n>|rejected|not-implemented>` · `[avrcp] ct open to=<addr> …` · `[avrcp] tg changed
//! to=<addr> -> volume=<n>`; `tests a2dp`: `avrcp=<ok|FAIL>`, `avrcp_ct=<ok|FAIL>` then `avrcp_mute=<ok|FAIL>` at the END of
//! the `:: A2DP: … ::` verdict ([`legs`], [`legs_ct`], [`legs_mute`]).
//! Compiled only under the `a2dp` knob.

use super::avdtp::ACP_WAIT_MS;
use super::{fmt_addr, Bt, LinkSt};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

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

/// The master is muted (AVRCPMUTE B790: the wire then carries 0, the level stays the master's); false with no audio seam.
fn muted() -> bool {
    #[cfg(any(feature = "uvc", feature = "hda-tone"))]
    return crate::video::status::volume().1;
    #[cfg(not(any(feature = "uvc", feature = "hda-tone")))]
    false
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
    /// A PDU on an AVCTP channel: the target's answer on that channel and its effect (a response goes to the CT,
    /// [`Bt::ct_rx`]; the CHANGED a kept RegisterNotification is owed goes from the watch, [`Bt::avrcp_watch`]).
    pub(super) fn avrcp_rx(&mut self, li: usize, ci: usize, body: &[u8]) {
        let (addr, h, rcid) = (self.links[li].addr, self.links[li].handle, self.links[li].chans[ci].rcid);
        let Some(r) = target(body, playing(), master()) else { return self.ct_rx(li, ci, body) };
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
            ct_heard(addr, v); // the headset set this level: the CT pushes nothing back; the watch sends the CHANGED
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

// ── AVRCPCT (rmbp-ledger B780): the CONTROLLER role and the one watch on the master ─────────────────────────────

/// A SetAbsoluteVolume with no response this long is given up (the next move sends again).
const CT_RSP_MS: u64 = 1000;

/// The AVRCP controller toward the A2DP headset (the stream's device, [`ct_arm`]).
#[derive(Clone, Copy)]
pub struct Ct {
    addr: [u8; 6],
    /// When the stream came up with no AVCTP channel: this host opens one after [`ACP_WAIT_MS`] (ACP_DUE's shape).
    due: Option<u64>,
    /// The open AVCTP channel the CT runs on (0: none).
    rcid: u16,
    /// The CT's next transaction label (0..=15; the headset's labels on its commands are the TG's, [`VOL_NOTE`]).
    next: u8,
    /// The RegisterNotification(volume changed) awaiting its CHANGED: its label.
    reg: Option<u8>,
    /// The SetAbsoluteVolume awaiting its response: (label, sent at).
    set: Option<(u8, u64)>,
    /// The absolute volume the headset was last given (or set itself): the watch pushes only a different one.
    pushed: Option<u8>,
    /// The headset has not refused SetAbsoluteVolume.
    abs_ok: bool,
    /// The headset accepted one: it carries the level ([`ct_volume`]).
    carries: bool,
}

/// What the headset answered the CT.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CtEv {
    Interim(u8),
    Changed(u8),
    Accepted(u8),
    Refused(&'static str),
}

impl Ct {
    pub fn new(addr: [u8; 6], now: u64) -> Ct {
        Ct { addr, due: Some(now), rcid: 0, next: 0, reg: None, set: None, pushed: None, abs_ok: true, carries: false }
    }

    fn label(&mut self) -> u8 {
        let l = self.next;
        self.next = (l + 1) & 0x0F;
        l
    }

    /// The outbound open's wait ran out (once).
    pub fn open_due(&mut self, now: u64) -> bool {
        match self.due {
            Some(t) if now.saturating_sub(t) > ACP_WAIT_MS => {
                self.due = None;
                true
            }
            _ => false,
        }
    }

    /// A RegisterNotification(volume changed) on a fresh label.
    pub fn register(&mut self) -> Vec<u8> {
        let l = self.label();
        self.reg = Some(l);
        ct_register(l)
    }

    /// A response PDU from the headset on one of the CT's open labels: (the command it answers, what it said).
    pub fn on_rsp(&mut self, pdu: &[u8]) -> Option<(&'static str, CtEv)> {
        let (label, rsp, pid) = header(pdu)?;
        let avc = pdu.get(3..)?;
        if !rsp || pid != PID_AVRCP || avc.len() < 7 || avc[2] != VENDOR || avc[3..6] != BT_SIG {
            return None;
        }
        let (code, id, p) = (avc[0] & 0x0F, avc[6], avc.get(10..).unwrap_or(&[]));
        let vol = (p.len() >= 2 && p[0] == EV_VOLUME).then(|| p[1] & 0x7F);
        if self.reg == Some(label) && id == REGISTER_NOTE {
            let ev = match (code, vol) {
                (INTERIM, Some(v)) => return Some(("register", CtEv::Interim(v))),
                (CHANGED, Some(v)) => CtEv::Changed(v),
                (REJECTED, _) => CtEv::Refused("rejected"),
                (NOT_IMPLEMENTED, _) => CtEv::Refused("not-implemented"),
                _ => return None,
            };
            self.reg = None;
            return Some(("register", ev));
        }
        if self.set.is_some_and(|(l, _)| l == label) && id == SET_ABS_VOL {
            self.set = None;
            let ev = match code {
                ACCEPTED if !p.is_empty() => CtEv::Accepted(p[0] & 0x7F),
                REJECTED => CtEv::Refused("rejected"),
                NOT_IMPLEMENTED => CtEv::Refused("not-implemented"),
                _ => CtEv::Refused("unexpected"),
            };
            match ev {
                CtEv::Accepted(_) => {
                    // CRAWLE (LEDGER SR173): the headset carries the level now. `pushed` stays the value `Watch::step`
                    // SENT: an ACCEPTED carries the volume the headset actually set (AVRCP 1.6 §6.13.2), which a headset
                    // with coarser steps rounds — storing that would make `step` push the master again, forever.
                    self.carries = true;
                }
                _ => {
                    self.abs_ok = false;
                    self.carries = false;
                }
            }
            return Some(("set-volume", ev));
        }
        None
    }
}

/// The CT's RegisterNotification(volume changed) command on `label` (playback interval 0).
pub fn ct_register(label: u8) -> Vec<u8> {
    command(label, &vendor(NOTIFY, REGISTER_NOTE, &[EV_VOLUME, 0, 0, 0, 0]))
}

/// The CT's SetAbsoluteVolume command on `label`.
pub fn ct_set_volume(label: u8, abs: u8) -> Vec<u8> {
    command(label, &vendor(CONTROL, SET_ABS_VOL, &[abs & 0x7F]))
}

/// A single-packet AVCTP COMMAND (C/R = 0) on `label` carrying `avc` ([`frame`] is the response's).
fn command(label: u8, avc: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(3 + avc.len());
    v.push(label << 4);
    v.extend_from_slice(&PID_AVRCP.to_be_bytes());
    v.extend_from_slice(avc);
    v
}

/// One PDU the watch sends: to whom, which (`changed` the TG's, `register` / `set-volume` the CT's), the volume.
pub struct Push {
    pub to: [u8; 6],
    pub cmd: &'static str,
    pub pdu: Vec<u8>,
    pub abs: u8,
    /// A set-volume carrying the master's mute (`abs` is then 0, AVRCPMUTE B790).
    pub muted: bool,
}

/// The watch's state: the master at the last pass, and the CT.
pub struct Watch {
    seen: Option<u8>,
    ct: Option<Ct>,
}

impl Watch {
    /// One pass, pure: the master is `abs`, `note` the TG's kept RegisterNotification, `chan` the open AVCTP channel
    /// to the CT's headset. A moved master closes the note with CHANGED; a new channel starts the CT (register); the
    /// CT pushes SetAbsoluteVolume when the master differs from what the headset has and none is in flight. A `muted`
    /// master is 0 on the wire (AVRCPMUTE B790); the TG's CHANGED stays on the level.
    pub fn step(&mut self, abs: u8, muted: bool, now: u64, note: &mut Option<([u8; 6], u8)>, chan: Option<u16>) -> Vec<Push> {
        let mut out = Vec::new();
        let wire = if muted { 0 } else { abs };
        let moved = self.seen.is_some_and(|s| s != abs);
        self.seen = Some(abs);
        if moved {
            if let Some((to, label)) = note.take() {
                out.push(Push { to, cmd: "changed", pdu: changed(label, abs), abs, muted: false });
            }
        }
        let Some(ct) = self.ct.as_mut() else { return out };
        let chan = chan.unwrap_or(0);
        if ct.rcid != chan {
            *ct = Ct { rcid: chan, due: None, ..Ct::new(ct.addr, now) };
            if chan != 0 {
                out.push(Push { to: ct.addr, cmd: "register", pdu: ct.register(), abs, muted: false });
            }
        }
        if ct.rcid == 0 || !ct.abs_ok {
            return out;
        }
        if ct.set.is_some_and(|(_, t)| now.saturating_sub(t) > CT_RSP_MS) {
            ct.set = None;
        }
        if ct.set.is_none() && ct.pushed != Some(wire) {
            let l = ct.label();
            ct.set = Some((l, now));
            ct.pushed = Some(wire);
            out.push(Push { to: ct.addr, cmd: "set-volume", pdu: ct_set_volume(l, wire), abs: wire, muted });
        }
        out
    }

    /// The headset carries the level now.
    fn carries(&self) -> bool {
        self.ct.as_ref().is_some_and(|c| c.carries && c.abs_ok && c.rcid != 0)
    }

    /// The headset at `addr` set the master to `abs` itself, pure: nothing is pushed back. While `muted` the headset was
    /// told 0 and that stays what it has (AVRCPMUTE B790): the new level is the master's, kept for the unmute.
    pub fn heard(&mut self, addr: [u8; 6], abs: u8, muted: bool) {
        if let Some(c) = self.ct.as_mut().filter(|c| c.addr == addr) {
            if !muted {
                c.pushed = Some(abs);
            }
        }
    }
}

/// The watch and the CT. Lock order: `BT`, then this alone (never with [`VOL_NOTE`], `A2` or `ACP_DUE`).
static WATCH: crate::sync::Mutex<Watch> = crate::sync::Mutex::new(Watch { seen: None, ct: None });
/// [`Watch::carries`], published for the mixer (an atomic: the mixer never takes a Bluetooth lock).
static CARRIES: AtomicBool = AtomicBool::new(false);

/// The A2DP stream to `addr` is up (`avdtp`'s Out::Streaming): the CT is bound to that headset; with no AVCTP channel
/// yet, the outbound open's wait starts.
pub(super) fn ct_arm(addr: [u8; 6], now: u64) {
    let mut w = WATCH.lock();
    match w.ct.as_mut() {
        Some(c) if c.addr == addr => {
            if c.rcid == 0 {
                c.due = Some(now);
            }
        }
        _ => w.ct = Some(Ct::new(addr, now)),
    }
}

/// The headset at `addr` set the master itself (the TG's SetAbsoluteVolume / volume keys): nothing is pushed back.
fn ct_heard(addr: [u8; 6], abs: u8) {
    WATCH.lock().heard(addr, abs, muted());
}

/// The headset has accepted SetAbsoluteVolume on a live CT: it carries the level, so the mixer plays BT at unity
/// (BP-audio-14: the master is applied once).
pub fn ct_volume() -> bool {
    CARRIES.load(Ordering::Acquire)
}

impl Bt {
    /// THE WATCH, once per BTHID pass (`avdtp::tick`): the CT's outbound AVCTP open when due, then [`Watch::step`]'s
    /// PDUs sent on their links' AVCTP channels.
    pub(super) fn avrcp_watch(&mut self, now: u64) {
        let ct_addr = WATCH.lock().ct.map(|c| c.addr);
        let li = ct_addr.and_then(|a| self.link_addr(&a));
        let chan = li.and_then(|li| self.links[li].open_rcid(PSM_AVCTP));
        if let (Some(li), None) = (li, chan) {
            let due = WATCH.lock().ct.as_mut().is_some_and(|c| c.open_due(now));
            if due && self.links[li].st == LinkSt::Up && self.links[li].chan_psm(PSM_AVCTP).is_none() {
                serial_println!("[avrcp] ct open to={} (the peer opened no AVCTP within {} ms)", fmt_addr(&self.links[li].addr), ACP_WAIT_MS);
                self.open_chan(li, PSM_AVCTP, now);
            }
        }
        let mut note = VOL_NOTE.lock().take();
        let sends = {
            let mut w = WATCH.lock();
            let s = w.step(master(), muted(), now, &mut note, chan);
            CARRIES.store(w.carries(), Ordering::Release);
            s
        };
        if let Some(n) = note {
            VOL_NOTE.lock().get_or_insert(n);
        }
        for s in sends {
            self.avrcp_send(&s);
        }
    }

    /// One watch PDU on the AVCTP channel of the link to `s.to`, and its wire line.
    fn avrcp_send(&mut self, s: &Push) {
        let Some(li) = self.link_addr(&s.to) else { return };
        let Some(rcid) = self.links[li].open_rcid(PSM_AVCTP) else { return };
        let h = self.links[li].handle;
        self.l2_send(h, rcid, &s.pdu);
        match s.cmd {
            "changed" => serial_println!("[avrcp] tg changed to={} -> volume={}", fmt_addr(&s.to), s.abs),
            "register" => serial_println!("[avrcp] ct cmd=register to={} -> sent", fmt_addr(&s.to)),
            cmd => serial_println!("[avrcp] ct cmd={} to={} -> sent volume={}{}", cmd, fmt_addr(&s.to), s.abs, if s.muted { " muted=1" } else { "" }),
        }
    }

    /// A RESPONSE on an AVCTP channel: the CT's. The headset's CHANGED moves the master (nothing pushed back) and the
    /// CT registers again on the same channel.
    fn ct_rx(&mut self, li: usize, ci: usize, body: &[u8]) {
        let (addr, h, rcid) = (self.links[li].addr, self.links[li].handle, self.links[li].chans[ci].rcid);
        let ev = {
            let mut w = WATCH.lock();
            let ev = w.ct.as_mut().filter(|c| c.addr == addr).and_then(|c| c.on_rsp(body));
            CARRIES.store(w.carries(), Ordering::Release);
            ev
        };
        let Some((cmd, ev)) = ev else { return };
        let tail = match ev {
            CtEv::Interim(v) => alloc::format!("interim volume={}", v),
            CtEv::Changed(v) => alloc::format!("changed volume={}", v),
            CtEv::Accepted(v) => alloc::format!("accepted volume={}", v),
            CtEv::Refused(why) => alloc::string::String::from(why),
        };
        serial_println!("[avrcp] ct cmd={} to={} -> {}", cmd, fmt_addr(&addr), tail);
        if let CtEv::Changed(v) = ev {
            ct_heard(addr, set_master(v));
            let again = WATCH.lock().ct.as_mut().filter(|c| c.addr == addr).map(|c| c.register());
            if let Some(pdu) = again {
                self.l2_send(h, rcid, &pdu);
                serial_println!("[avrcp] ct cmd=register to={} -> sent", fmt_addr(&addr));
            }
        }
    }
}

/// `tests a2dp` `avrcp_ct` leg — a synthetic headset: the outbound open's wait; on the channel, RegisterNotification
/// then SetAbsoluteVolume out byte for byte; INTERIM and ACCEPTED parsed (the headset carries the level); the master
/// moves -> the TG's kept note gets CHANGED on its label and SetAbsoluteVolume goes out; the headset's CHANGED is
/// followed; a REJECTED stops the pushes; a stray label is ignored.
pub fn legs_ct() -> Result<(), &'static str> {
    const A: [u8; 6] = [1, 2, 3, 4, 5, 6];
    let mut w = Watch { seen: None, ct: Some(Ct::new(A, 0)) };
    let mut note = None;
    if !w.step(64, false, 0, &mut note, None).is_empty() {
        return Err("no-channel");
    }
    let c = w.ct.as_mut().ok_or("ct")?;
    if c.open_due(ACP_WAIT_MS) || !c.open_due(ACP_WAIT_MS + 1) || c.open_due(ACP_WAIT_MS + 2) {
        return Err("open-due");
    }
    let out = w.step(64, false, 1700, &mut note, Some(0x0041));
    let want_reg: &[u8] = &[0x00, 0x11, 0x0E, 0x03, 0x48, 0x00, 0x00, 0x19, 0x58, 0x31, 0x00, 0x00, 0x05, 0x0D, 0x00, 0x00, 0x00, 0x00];
    let want_set: &[u8] = &[0x10, 0x11, 0x0E, 0x00, 0x48, 0x00, 0x00, 0x19, 0x58, 0x50, 0x00, 0x00, 0x01, 0x40];
    if out.len() != 2 || out[0].cmd != "register" || out[0].pdu != want_reg || out[1].cmd != "set-volume" || out[1].pdu != want_set {
        return Err("register-and-set");
    }
    let c = w.ct.as_mut().ok_or("ct")?;
    if c.on_rsp(&[0x02, 0x11, 0x0E, 0x0F, 0x48, 0x00, 0x00, 0x19, 0x58, 0x31, 0x00, 0x00, 0x02, 0x0D, 0x30]) != Some(("register", CtEv::Interim(0x30))) {
        return Err("interim");
    }
    if c.on_rsp(&[0x12, 0x11, 0x0E, 0x09, 0x48, 0x00, 0x00, 0x19, 0x58, 0x50, 0x00, 0x00, 0x01, 0x40]) != Some(("set-volume", CtEv::Accepted(0x40))) || !w.carries() {
        return Err("accepted");
    }
    if !w.step(64, false, 1800, &mut note, Some(0x0041)).is_empty() {
        return Err("still");
    }
    note = Some((A, 9));
    let out = w.step(abs_of(9), false, 1900, &mut note, Some(0x0041));
    if out.len() != 2 || out[0].cmd != "changed" || out[0].pdu != changed(9, abs_of(9)) || out[1].pdu != ct_set_volume(2, abs_of(9)) || note.is_some() {
        return Err("move");
    }
    let c = w.ct.as_mut().ok_or("ct")?;
    if c.on_rsp(&[0x72, 0x11, 0x0E, 0x09, 0x48, 0x00, 0x00, 0x19, 0x58, 0x50, 0x00, 0x00, 0x01, 0x40]).is_some() {
        return Err("stray-label");
    }
    if c.on_rsp(&[0x02, 0x11, 0x0E, 0x0D, 0x48, 0x00, 0x00, 0x19, 0x58, 0x31, 0x00, 0x00, 0x02, 0x0D, 0x50]) != Some(("register", CtEv::Changed(0x50))) || c.reg.is_some() {
        return Err("changed");
    }
    if c.on_rsp(&[0x22, 0x11, 0x0E, 0x0A, 0x48, 0x00, 0x00, 0x19, 0x58, 0x50, 0x00, 0x00, 0x01, 0x00]) != Some(("set-volume", CtEv::Refused("rejected"))) || w.carries() {
        return Err("rejected");
    }
    if !w.step(80, false, 2000, &mut note, Some(0x0041)).is_empty() {
        return Err("refused-pushes");
    }
    // CRAWLE (LEDGER SR173): a headset that ROUNDS — ACCEPTED 0x48 for a sent 0x47 — is answered once, not pushed again.
    let mut w = Watch { seen: None, ct: Some(Ct::new(A, 0)) };
    let mut note = None;
    let out = w.step(0x47, 1700, &mut note, Some(0x0041));
    if out.len() != 2 || out[1].pdu != ct_set_volume(1, 0x47) {
        return Err("quantized-set");
    }
    let c = w.ct.as_mut().ok_or("ct")?;
    if c.on_rsp(&[0x12, 0x11, 0x0E, 0x09, 0x48, 0x00, 0x00, 0x19, 0x58, 0x50, 0x00, 0x00, 0x01, 0x48]) != Some(("set-volume", CtEv::Accepted(0x48))) || !w.carries() {
        return Err("quantized-accepted");
    }
    if !w.step(0x47, 1800, &mut note, Some(0x0041)).is_empty() {
        return Err("quantized");
    }
    Ok(())
}

// ── AVRCPMUTE (rmbp-ledger B790): the master's mute on the wire ─────────────────────────────────────────────────────

/// `tests a2dp` `avrcp_mute` leg — a synthetic headset that carries the level: mute -> SetAbsoluteVolume(0) sent (and
/// accepted, the headset still carries); the headset's CHANGED while muted -> the level remembered, nothing sent; unmute
/// -> the remembered level sent; a mute while a set is in flight waits for it (the one-in-flight path), then 0.
pub fn legs_mute() -> Result<(), &'static str> {
    const A: [u8; 6] = [1, 2, 3, 4, 5, 6];
    const CH: Option<u16> = Some(0x0041);
    let mut w = Watch { seen: None, ct: Some(Ct::new(A, 0)) };
    let mut note = None;
    let out = w.step(64, false, 0, &mut note, CH);
    if out.len() != 2 || out[1].cmd != "set-volume" || out[1].pdu != ct_set_volume(1, 64) || out[1].muted {
        return Err("setup");
    }
    let c = w.ct.as_mut().ok_or("ct")?;
    if c.on_rsp(&[0x12, 0x11, 0x0E, 0x09, 0x48, 0x00, 0x00, 0x19, 0x58, 0x50, 0x00, 0x00, 0x01, 0x40]) != Some(("set-volume", CtEv::Accepted(0x40))) {
        return Err("setup-accepted");
    }
    let out = w.step(64, true, 100, &mut note, CH);
    if out.len() != 1 || out[0].cmd != "set-volume" || out[0].pdu != ct_set_volume(2, 0) || out[0].abs != 0 || !out[0].muted {
        return Err("mute-sent");
    }
    let c = w.ct.as_mut().ok_or("ct")?;
    if c.on_rsp(&[0x22, 0x11, 0x0E, 0x09, 0x48, 0x00, 0x00, 0x19, 0x58, 0x50, 0x00, 0x00, 0x01, 0x00]) != Some(("set-volume", CtEv::Accepted(0))) || !w.carries() {
        return Err("mute-accepted");
    }
    w.heard(A, 80, true);
    if !w.step(80, true, 200, &mut note, CH).is_empty() || !w.step(80, true, 300, &mut note, CH).is_empty() {
        return Err("changed-while-muted");
    }
    let out = w.step(80, false, 400, &mut note, CH);
    if out.len() != 1 || out[0].pdu != ct_set_volume(3, 80) || out[0].abs != 80 || out[0].muted {
        return Err("unmute-sent");
    }
    if !w.step(80, true, 500, &mut note, CH).is_empty() {
        return Err("in-flight");
    }
    let out = w.step(80, true, 500 + CT_RSP_MS + 1, &mut note, CH);
    if out.len() != 1 || out[0].pdu != ct_set_volume(4, 0) || !out[0].muted {
        return Err("mute-after-flight");
    }
    Ok(())
}
