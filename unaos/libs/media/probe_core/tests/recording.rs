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
