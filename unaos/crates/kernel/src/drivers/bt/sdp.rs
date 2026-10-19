//! CHARTER: Comscan — driver (Bluetooth; SDP client queries ride BTHID's SDP channel and its continuation loop, B339; this file is the A2DP sink query and its answer)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A2DP (rmbp-ledger B739) — SDP discovery of the A2DP sink service on a paired device.
//!
//! The SDP CLIENT exists in the BTHID host (`drivers/ehci/bthid.rs`: `sdp_request` sends a
//! ServiceSearchAttributeRequest on the SDP channel, `on_sdp` gathers the AttributeLists across continuation states,
//! at most 8 rounds, then hands the whole byte run to a parser). For an audio link the request's two data elements
//! come from [`a2dp_params`] and the answer goes to [`a2dp_sink`] instead of the HID descriptor walk.
//!
//! The query: ServiceSearchPattern = { UUID16 0x110B AudioSink }, AttributeIDList = { range 0x0004..0x0009 }
//! (ProtocolDescriptorList 0x0004 .. BluetoothProfileDescriptorList 0x0009). The answer read: the uint16 that
//! follows UUID 0x0100 (L2CAP) is the PSM, the one after UUID 0x0019 (AVDTP) its version, the one after UUID
//! 0x110D (AdvancedAudioDistribution) the A2DP version. A data element is walked by its header (type << 3 | size
//! index; index 5/6/7 = an 8/16/32-bit length follows); a sequence is entered, not skipped, so the walk is flat.

use alloc::vec::Vec;

pub const UUID_L2CAP: u16 = 0x0100;
pub const UUID_AVDTP: u16 = 0x0019;
pub const UUID_AUDIO_SINK: u16 = 0x110B;
pub const UUID_A2DP: u16 = 0x110D;

/// The ServiceSearchAttributeRequest parameters before the continuation state: the search pattern, the maximum
/// attribute byte count (0x0280, as the HID query), the attribute ID range.
pub fn a2dp_params(p: &mut Vec<u8>) {
    p.extend_from_slice(&[0x35, 0x03, 0x19, (UUID_AUDIO_SINK >> 8) as u8, UUID_AUDIO_SINK as u8]); // DES { UUID16 AudioSink }
    p.extend_from_slice(&0x0280u16.to_be_bytes());
    p.extend_from_slice(&[0x35, 0x05, 0x0A, 0x00, 0x04, 0x00, 0x09]); // DES { uint32 range 0x0004..0x0009 }
}

/// What the sink record says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SinkRecord {
    /// The L2CAP PSM of AVDTP (0x0019 when the record names none).
    pub psm: u16,
    /// AVDTP version, e.g. 0x0103 = 1.3 (0 = not stated).
    pub avdtp: u16,
    /// A2DP version (0 = not stated).
    pub a2dp: u16,
}

/// One data element: (type, value offset, value length); a sequence's "value" is entered by the walk.
fn element(b: &[u8], i: usize) -> Option<(u8, usize, usize)> {
    let h = *b.get(i)?;
    let (ty, sz) = (h >> 3, h & 7);
    let (off, len) = match sz {
        0 if ty == 0 => (i + 1, 0),
        0 => (i + 1, 1),
        1 => (i + 1, 2),
        2 => (i + 1, 4),
        3 => (i + 1, 8),
        4 => (i + 1, 16),
        5 => (i + 2, *b.get(i + 1)? as usize),
        6 => (i + 3, u16::from_be_bytes([*b.get(i + 1)?, *b.get(i + 2)?]) as usize),
        _ => (i + 5, u32::from_be_bytes([*b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?, *b.get(i + 4)?]) as usize),
    };
    if off + len > b.len() {
        return None;
    }
    Some((ty, off, len))
}

/// The A2DP sink record in an SDP AttributeLists byte run, or None when the run names no AVDTP (no sink record).
pub fn a2dp_sink(b: &[u8]) -> Option<SinkRecord> {
    let mut r = SinkRecord { psm: 0x0019, avdtp: 0, a2dp: 0 };
    let mut seen_avdtp = false;
    let mut last_uuid: Option<u16> = None;
    let mut i = 0usize;
    while i < b.len() {
        let Some((ty, off, len)) = element(b, i) else { break };
        let mut uuid = None;
        match (ty, len) {
            (6 | 7, _) => {
                i = off; // enter the sequence
                continue;
            }
            (3, 2) => uuid = Some(u16::from_be_bytes([b[off], b[off + 1]])),
            (1, 2) => {
                let v = u16::from_be_bytes([b[off], b[off + 1]]);
                match last_uuid {
                    Some(UUID_L2CAP) => r.psm = v,
                    Some(UUID_AVDTP) => r.avdtp = v,
                    Some(UUID_A2DP) => r.a2dp = v,
                    _ => {}
                }
            }
            _ => {}
        }
        if uuid == Some(UUID_AVDTP) {
            seen_avdtp = true;
        }
        last_uuid = uuid;
        i = off + len;
    }
    seen_avdtp.then_some(r)
}

/// A sink record as a headset answers it (one record: L2CAP PSM 0x0019, AVDTP 1.3, A2DP 1.3), for `tests a2dp`.
pub fn fixture_record() -> Vec<u8> {
    alloc::vec![
        0x35, 0x22, // the record's attribute list
        0x09, 0x00, 0x04, 0x35, 0x10, 0x35, 0x06, 0x19, 0x01, 0x00, 0x09, 0x00, 0x19, 0x35, 0x06, 0x19, 0x00, 0x19, 0x09, 0x01, 0x03, // 0x0004
        0x09, 0x00, 0x09, 0x35, 0x08, 0x35, 0x06, 0x19, 0x11, 0x0D, 0x09, 0x01, 0x03, // 0x0009
    ]
}
