// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANTOOL (LEDGER SR471): a recording (PRB1 records back to back) round-trips, and every way its bytes can
//! be wrong is refused by name with the record it happened in.

use probe_core::frame::{Frame, PixelFormat, StreamKind};
use probe_core::recording::{self, Record, RecordingError};
use probe_core::replay;
use probe_core::wire::{self, HEADER_LEN, WireError};

const K: [f32; 4] = [421.5, 421.5, 424.0, 240.0];

fn three() -> Vec<Record> {
    vec![
        Record::new(replay::depth_plane(16, 8, 500, 10, 1), "uvc:8086:0b3a", 1000, Some(K)).unwrap(),
        Record::new(replay::depth_plane(16, 8, 600, 5, 2), "uvc:8086:0b3a", 100, Some(K)).unwrap(),
        Record::new(replay::ir_gradient(16, 8, 3), "uvc:8086:0b3a", 0, None).unwrap(),
    ]
}

#[test]
fn three_records_round_trip() {
    let recs = three();
    let bytes = recording::write(&recs).unwrap();
    let rec_len = |r: &Record| HEADER_LEN + r.frame.data.len();
    assert_eq!(bytes.len(), recs.iter().map(rec_len).sum::<usize>());
    let back = recording::read(&bytes).unwrap();
    assert_eq!(back.records, recs);
    assert_eq!(back.depth().count(), 2);
    let r0 = &back.records[0];
    assert_eq!(r0.id(), "uvc:8086:0b3a");
    assert_eq!(r0.depth_scale().unwrap().micrometres(), 1000);
    assert_eq!(back.records[1].depth_scale().unwrap().micrometres(), 100);
    let i = r0.intrinsics().unwrap();
    assert_eq!((i.width, i.height, i.fx, i.fy, i.ppx, i.ppy), (16, 8, 421.5, 421.5, 424.0, 240.0));
    assert!(back.records[2].intrinsics().is_none());
    assert!(!back.records[2].is_depth());
    // write(read(x)) is x.
    assert_eq!(recording::write(&back.records).unwrap(), bytes);
}

#[test]
fn truncation_is_named_with_its_record() {
    let recs = three();
    let bytes = recording::write(&recs).unwrap();
    let start2 = 2 * HEADER_LEN + recs[0].frame.data.len() + recs[1].frame.data.len();
    // Every cut inside the third record (header or data) names record 2 at its offset.
    for cut in start2 + 1..bytes.len() {
        match recording::read(&bytes[..cut]) {
            Err(RecordingError::Truncated { index: 2, offset, have, .. }) => {
                assert_eq!(offset, start2);
                assert_eq!(have as usize, cut - start2);
            }
            other => panic!("cut {cut}: {other:?}"),
        }
    }
    // A cut inside the second header.
    let start1 = HEADER_LEN + recs[0].frame.data.len();
    assert_eq!(
        recording::read(&bytes[..start1 + 40]),
        Err(RecordingError::Truncated { index: 1, offset: start1, need: HEADER_LEN as u64, have: 40 })
    );
    // A cut at a record boundary is a shorter recording, not a refusal.
    assert_eq!(recording::read(&bytes[..start2]).unwrap().records.len(), 2);
    assert_eq!(recording::read(&[]), Err(RecordingError::Empty));
}

#[test]
fn a_bad_header_is_named_with_its_record() {
    let recs = three();
    let mut bytes = recording::write(&recs).unwrap();
    let start2 = 2 * HEADER_LEN + recs[0].frame.data.len() + recs[1].frame.data.len();
    bytes[start2] = b'X';
    assert_eq!(
        recording::read(&bytes),
        Err(RecordingError::Header { index: 2, offset: start2, error: WireError::Magic })
    );
}

#[test]
fn a_longer_header_and_padding_are_walked_by_their_lengths() {
    let f = replay::depth_plane(4, 2, 500, 1, 7);
    // A 96-byte header: the 16 bytes this reader does not know are skipped.
    let h = wire::encode(&f, "uvc:8086:0b3a", 1000, Some(K)).unwrap();
    let mut long = h.to_vec();
    long[4..6].copy_from_slice(&96u16.to_le_bytes());
    long.extend_from_slice(&[0xAA; 16]);
    long.extend_from_slice(&f.data);
    // A padded frame: data length 4 bytes past what the geometry needs.
    let mut padded_data = f.data.clone();
    padded_data.extend_from_slice(&[0, 0, 0, 0]);
    let padded = Frame::new(StreamKind::Depth, PixelFormat::Z16, 4, 2, 8, 0, 8, padded_data).unwrap();
    let tail = recording::write(&[Record::new(padded.clone(), "x", 1000, None).unwrap()]).unwrap();
    long.extend_from_slice(&tail);
    let back = recording::read(&long).unwrap();
    assert_eq!(back.records.len(), 2);
    assert_eq!(back.records[0].header.header_len, 96);
    assert_eq!(back.records[0].frame, f);
    assert_eq!(back.records[1].frame, padded);
}

#[test]
fn an_overlong_id_is_refused_at_record_construction() {
    let f = replay::depth_plane(4, 2, 500, 1, 7);
    assert_eq!(Record::new(f, "an-id-of-seventeen", 1000, None), Err(WireError::Id));
}

// ── SONDE-RECORD-IMU (LEDGER SR491): motion records beside the frames ──────────────────────────────────────────────

use probe_core::imu::{ImuKind, ACCEL_MPS2_PER_LSB};
use probe_core::recording::Motion;
use probe_core::wire::{MotionRecord, EXTRINSIC_LEN, FLAG_EXTRINSIC, FORMAT_MOTION, MOTION_LEN};

const EXTR: [f32; 12] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, -0.00552, 0.0051, 0.01174];

fn accel(seq: u64, extrinsic: Option<[f32; 12]>) -> MotionRecord {
    MotionRecord {
        kind: ImuKind::Accel,
        raw: [12, -1003, 40],
        device_ts: 0x0102_0304_0506_0708,
        scale: ACCEL_MPS2_PER_LSB,
        seq,
        timestamp_us: 1_000 + seq,
        extrinsic,
        id: wire::motion_id("uvc:8086:0b3a").unwrap(),
    }
}

fn gyro(seq: u64) -> MotionRecord {
    MotionRecord { kind: ImuKind::Gyro, raw: [-7, 3, 32767], scale: 0.0, extrinsic: None, ..accel(seq, None) }
}

fn motion_bytes(m: &MotionRecord) -> Vec<u8> {
    let (b, n) = wire::encode_motion(m).unwrap();
    b[..n].to_vec()
}

#[test]
fn motion_records_interleave_and_round_trip() {
    let recs = three();
    let a0 = accel(0, Some(EXTR));
    let g0 = gyro(0);
    let a1 = accel(1, None);
    // frame, accel (with the extrinsic), gyro (unscaled), frame, accel, frame
    let mut bytes = recording::write(&recs[..1]).unwrap();
    bytes.extend_from_slice(&motion_bytes(&a0));
    bytes.extend_from_slice(&motion_bytes(&g0));
    bytes.extend_from_slice(&recording::write(&recs[1..2]).unwrap());
    bytes.extend_from_slice(&motion_bytes(&a1));
    bytes.extend_from_slice(&recording::write(&recs[2..]).unwrap());
    assert_eq!(motion_bytes(&a0).len(), HEADER_LEN + MOTION_LEN + EXTRINSIC_LEN);
    assert_eq!(motion_bytes(&g0).len(), HEADER_LEN + MOTION_LEN);

    let back = recording::read(&bytes).unwrap();
    assert_eq!(back.records, recs);
    assert_eq!(
        back.motion,
        vec![Motion { after: 1, record: a0 }, Motion { after: 1, record: g0 }, Motion { after: 2, record: a1 }]
    );
    assert_eq!(back.extrinsic(), Some(EXTR));
    // A decoded record re-encodes to the same bytes.
    for m in &back.motion {
        let (b, n) = wire::encode_motion(&m.record).unwrap();
        assert_eq!(wire::decode_motion(&b[..n]).unwrap(), m.record);
    }
    // SI units: accel raw x 0.001 g; the unscaled gyro gives raw counts only.
    let s = a0.sample().unwrap();
    assert_eq!(s.kind, ImuKind::Accel);
    assert_eq!(s.timestamp, 0x0102_0304_0506_0708);
    assert!((s.xyz[1] - (-1003.0 * 0.001 * 9.80665)).abs() < 1e-5, "{:?}", s.xyz);
    assert!(g0.sample().is_none());
    assert_eq!(back.motion[1].record.raw, [-7, 3, 32767]);
    assert_eq!(back.motion[0].record.id_str(), "uvc:8086:0b3a");
    // A recording with no motion records has none.
    assert!(recording::read(&recording::write(&recs).unwrap()).unwrap().motion.is_empty());
}

#[test]
fn motion_refusals_are_named() {
    let m = motion_bytes(&accel(0, None));
    // A frame reader refuses a motion header by its format code.
    assert_eq!(wire::decode(&m), Err(WireError::Format));
    assert!(wire::is_motion(&m));
    assert_eq!(m[7], FORMAT_MOTION);
    // Format 4 on a depth stream code.
    let mut bad = m.clone();
    bad[6] = 0;
    assert_eq!(wire::decode_motion(&bad), Err(WireError::Stream));
    // A data length the flag does not explain.
    let mut bad = m.clone();
    bad[40..44].copy_from_slice(&20u32.to_le_bytes());
    bad[16..20].copy_from_slice(&20u32.to_le_bytes());
    assert_eq!(wire::decode_motion_lengths(&bad), Err(WireError::Geometry));
    let mut bad = m.clone();
    bad[44..48].copy_from_slice(&FLAG_EXTRINSIC.to_le_bytes());
    assert_eq!(wire::decode_motion_lengths(&bad), Err(WireError::Geometry));
    // A record shorter than its lengths.
    assert_eq!(wire::decode_motion(&m[..m.len() - 1]), Err(WireError::Short));
    assert_eq!(wire::motion_id("an-id-of-seventeen"), Err(WireError::Id));

    // In a recording: a cut inside the motion payload names that record (index 1: frame 0, motion 1).
    let recs = three();
    let mut bytes = recording::write(&recs[..1]).unwrap();
    let start1 = bytes.len();
    bytes.extend_from_slice(&m);
    for cut in start1 + 1..bytes.len() {
        match recording::read(&bytes[..cut]) {
            Err(RecordingError::Truncated { index: 1, offset, .. }) => assert_eq!(offset, start1),
            other => panic!("cut {cut}: {other:?}"),
        }
    }
    // A bad motion header names its record too.
    bytes[start1 + 6] = 1;
    assert_eq!(
        recording::read(&bytes),
        Err(RecordingError::Header { index: 1, offset: start1, error: WireError::Stream })
    );
}
