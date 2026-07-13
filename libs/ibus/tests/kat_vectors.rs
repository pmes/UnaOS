// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.

//! Known-answer tests that FREEZE the FlySky i-BUS servo wire format.
//!
//! The format contract (32-byte frame, `0x20 0x40` header, 14× LE-u16 channels
//! in µs, `0xFFFF − Σ` checksum) is pinned here three independent ways:
//!
//! 1. [`GOLDEN_ALL_NEUTRAL`] — a fully byte-literal frame, checksum bytes
//!    written out by hand, so the layout is nailed to concrete bytes.
//! 2. [`kat_frame`] — a *local* re-statement of the checksum formula (does not
//!    call the crate's private checksum), used to synthesise the corruption /
//!    resync / range / command vectors.
//! 3. [`ibus::encode_servo_frame`] — the crate's own encoder, cross-checked
//!    against both of the above.
//!
//! ⚠ Upgrading synthetic → silicon-derived: drop real FGr8B captures into
//! [`CAPTURED_FRAMES`] and [`kat_captured_frames_decode`] exercises them. No
//! other change is needed.

use ibus::{
    encode_servo_frame, Frame, Parser, CHAN_MAX_US, CHAN_MIN_US, CMD_SERVO, FRAME_LEN, HEADER_LEN,
    NUM_CHANNELS,
};

/// Local, independent re-statement of the i-BUS checksum. Deliberately NOT the
/// crate's internal routine, so a KAT that uses it is a genuine cross-check.
fn kat_checksum(first30: &[u8]) -> u16 {
    assert_eq!(first30.len(), FRAME_LEN - 2);
    let mut sum: u16 = 0;
    for &b in first30 {
        sum = sum.wrapping_add(b as u16);
    }
    0xFFFFu16.wrapping_sub(sum)
}

/// Synthesise a 32-byte frame with an arbitrary command byte and channel set,
/// computing the checksum locally. `cmd == 0x40` yields a servo frame.
fn kat_frame(cmd: u8, channels: &[u16; NUM_CHANNELS]) -> [u8; FRAME_LEN] {
    let mut buf = [0u8; FRAME_LEN];
    buf[0] = HEADER_LEN;
    buf[1] = cmd;
    for (i, &c) in channels.iter().enumerate() {
        let off = 2 + i * 2;
        buf[off..off + 2].copy_from_slice(&c.to_le_bytes());
    }
    let ck = kat_checksum(&buf[..FRAME_LEN - 2]).to_le_bytes();
    buf[FRAME_LEN - 2] = ck[0];
    buf[FRAME_LEN - 1] = ck[1];
    buf
}

/// Push every byte, returning the single frame expected (asserts exactly one).
fn push_one(bytes: &[u8]) -> (Parser, Option<Frame>) {
    let mut p = Parser::new();
    let mut out = None;
    for &b in bytes {
        if let Some(f) = p.push(b) {
            assert!(out.is_none(), "more than one frame emitted");
            out = Some(f);
        }
    }
    (p, out)
}

// ---------------------------------------------------------------------------
// 1. The byte-literal golden frame: all 14 channels at neutral (1500 µs).
//    1500 = 0x05DC → LE [0xDC, 0x05]. Σ bytes[0..30] = 0x20 + 0x40 + 14·0xE1
//    = 3246; checksum = 0xFFFF − 3246 = 0xF351 → LE [0x51, 0xF3].
// ---------------------------------------------------------------------------
#[rustfmt::skip]
const GOLDEN_ALL_NEUTRAL: [u8; FRAME_LEN] = [
    0x20, 0x40,                                                     // header + servo cmd
    0xDC, 0x05, 0xDC, 0x05, 0xDC, 0x05, 0xDC, 0x05, 0xDC, 0x05,     // ch 0..5
    0xDC, 0x05, 0xDC, 0x05, 0xDC, 0x05, 0xDC, 0x05, 0xDC, 0x05,     // ch 5..10
    0xDC, 0x05, 0xDC, 0x05, 0xDC, 0x05, 0xDC, 0x05,                 // ch 10..14
    0x51, 0xF3,                                                     // checksum (LE)
];

/// Real captured FGr8B frames go here (byte-literal). Empty until Peter benches.
const CAPTURED_FRAMES: &[[u8; FRAME_LEN]] = &[];

#[test]
fn kat_golden_literal_decodes_all_neutral() {
    let (p, frame) = push_one(&GOLDEN_ALL_NEUTRAL);
    let frame = frame.expect("golden frame must decode");
    assert_eq!(frame.channels(), &[1500u16; NUM_CHANNELS]);
    assert!(frame.in_range());
    assert_eq!(p.counters().frames_ok, 1);
    assert_eq!(p.counters().frames_bad, 0);
    assert_eq!(p.counters().frames_skipped, 0);
}

#[test]
fn kat_encoder_and_local_helper_agree_with_golden() {
    // All three routes to the wire format must produce identical bytes.
    let neutral = [1500u16; NUM_CHANNELS];
    assert_eq!(encode_servo_frame(&neutral), GOLDEN_ALL_NEUTRAL);
    assert_eq!(kat_frame(CMD_SERVO, &neutral), GOLDEN_ALL_NEUTRAL);
    // And the literal checksum bytes match the formula.
    assert_eq!(
        kat_checksum(&GOLDEN_ALL_NEUTRAL[..FRAME_LEN - 2]).to_le_bytes(),
        [GOLDEN_ALL_NEUTRAL[FRAME_LEN - 2], GOLDEN_ALL_NEUTRAL[FRAME_LEN - 1]],
    );
}

#[test]
fn kat_edge_values_min_neutral_max() {
    // 1000 / 1500 / 2000 across the first three channels, rest neutral.
    let mut ch = [1500u16; NUM_CHANNELS];
    ch[0] = 1000;
    ch[1] = 1500;
    ch[2] = 2000;
    let (_, frame) = push_one(&kat_frame(CMD_SERVO, &ch));
    let frame = frame.expect("edge-value frame decodes");
    assert_eq!(frame.channel(0), Some(1000));
    assert_eq!(frame.channel(1), Some(1500));
    assert_eq!(frame.channel(2), Some(2000));
    assert_eq!(frame.channel(NUM_CHANNELS), None);
    assert!(frame.in_range());
}

#[test]
fn kat_back_to_back_frames_both_decode() {
    let a = kat_frame(CMD_SERVO, &[1100u16; NUM_CHANNELS]);
    let b = kat_frame(CMD_SERVO, &[1900u16; NUM_CHANNELS]);
    let mut stream = [0u8; FRAME_LEN * 2];
    stream[..FRAME_LEN].copy_from_slice(&a);
    stream[FRAME_LEN..].copy_from_slice(&b);

    let mut p = Parser::new();
    let mut seen = 0usize;
    p.feed(&stream, |f| {
        match seen {
            0 => assert_eq!(f.channels(), &[1100u16; NUM_CHANNELS]),
            1 => assert_eq!(f.channels(), &[1900u16; NUM_CHANNELS]),
            _ => panic!("too many frames"),
        }
        seen += 1;
    });
    assert_eq!(seen, 2);
    assert_eq!(p.counters().frames_ok, 2);
}

#[test]
fn kat_checksum_corrupt_rejected_then_resync() {
    let mut bad = kat_frame(CMD_SERVO, &[1500u16; NUM_CHANNELS]);
    bad[10] ^= 0xFF; // flip a data byte → checksum mismatch
    let good = kat_frame(CMD_SERVO, &[1234u16; NUM_CHANNELS]);

    let mut stream = [0u8; FRAME_LEN * 2];
    stream[..FRAME_LEN].copy_from_slice(&bad);
    stream[FRAME_LEN..].copy_from_slice(&good);

    let mut p = Parser::new();
    let mut frames = 0usize;
    p.feed(&stream, |f| {
        assert_eq!(f.channels(), &[1234u16; NUM_CHANNELS]);
        frames += 1;
    });
    assert_eq!(frames, 1, "only the clean frame survives");
    assert_eq!(p.counters().frames_ok, 1);
    assert!(p.counters().frames_bad >= 1, "corrupt frame counted bad");
}

#[test]
fn kat_truncated_then_clean_recovers() {
    // 20 bytes of a frame, then the transmitter re-frames cleanly.
    let partial = kat_frame(CMD_SERVO, &[1500u16; NUM_CHANNELS]);
    let clean = kat_frame(CMD_SERVO, &[1600u16; NUM_CHANNELS]);

    let mut p = Parser::new();
    let mut frames = 0usize;
    p.feed(&partial[..20], |_| frames += 1);
    assert_eq!(frames, 0, "truncated input yields nothing");
    p.feed(&clean, |f| {
        assert_eq!(f.channels(), &[1600u16; NUM_CHANNELS]);
        frames += 1;
    });
    assert_eq!(frames, 1, "clean frame recovers after truncation");
    assert_eq!(p.counters().frames_ok, 1);
}

#[test]
fn kat_misaligned_junk_prefix_with_stray_header_resyncs() {
    // Junk containing a stray 0x20 (a false anchor) then a real frame.
    let junk = [0x00u8, 0x20, 0xFF, 0x20, 0x11, 0x20, 0x99];
    let real = kat_frame(CMD_SERVO, &[1450u16; NUM_CHANNELS]);

    let mut p = Parser::new();
    let mut frames = 0usize;
    p.feed(&junk, |_| frames += 1);
    p.feed(&real, |f| {
        assert_eq!(f.channels(), &[1450u16; NUM_CHANNELS]);
        frames += 1;
    });
    assert_eq!(frames, 1, "real frame decodes despite misaligned prefix");
    assert_eq!(p.counters().frames_ok, 1);
}

#[test]
fn kat_interleaved_garbage_between_frames() {
    let a = kat_frame(CMD_SERVO, &[1000u16; NUM_CHANNELS]);
    let b = kat_frame(CMD_SERVO, &[2000u16; NUM_CHANNELS]);
    let garbage = [0xAAu8, 0x20, 0x00, 0xBB, 0x20, 0x40, 0xCC];

    let mut p = Parser::new();
    let mut seen = 0usize;
    for chunk in [&a[..], &garbage[..], &b[..]] {
        p.feed(chunk, |f| {
            match seen {
                0 => assert_eq!(f.channels(), &[1000u16; NUM_CHANNELS]),
                1 => assert_eq!(f.channels(), &[2000u16; NUM_CHANNELS]),
                _ => panic!("too many frames"),
            }
            seen += 1;
        });
    }
    assert_eq!(seen, 2, "both real frames survive the interleaved garbage");
}

#[test]
fn kat_non_servo_command_skipped_and_counted() {
    // A checksum-VALID frame with a non-servo command (e.g. telemetry 0x04).
    let telem = kat_frame(0x04, &[1500u16; NUM_CHANNELS]);
    let servo = kat_frame(CMD_SERVO, &[1550u16; NUM_CHANNELS]);

    let mut stream = [0u8; FRAME_LEN * 2];
    stream[..FRAME_LEN].copy_from_slice(&telem);
    stream[FRAME_LEN..].copy_from_slice(&servo);

    let (p, _) = {
        let mut p = Parser::new();
        let mut frames = 0usize;
        let mut last = None;
        p.feed(&stream, |f| {
            last = Some(f);
            frames += 1;
        });
        assert_eq!(frames, 1, "only the servo frame is emitted");
        assert_eq!(last.unwrap().channels(), &[1550u16; NUM_CHANNELS]);
        (p, ())
    };
    assert_eq!(p.counters().frames_ok, 1);
    assert_eq!(p.counters().frames_skipped, 1, "telemetry frame skipped+counted");
    assert_eq!(p.counters().frames_bad, 0, "a skipped frame is NOT an error");
}

#[test]
fn kat_invalid_range_decoded_but_flagged() {
    // Checksum is valid, but channel 0 sits below the sane band → flagged.
    let mut ch = [1500u16; NUM_CHANNELS];
    ch[0] = 500; // < CHAN_MIN_US
    let (p, frame) = push_one(&kat_frame(CMD_SERVO, &ch));
    let frame = frame.expect("frame with a valid checksum still decodes");
    assert_eq!(frame.channel(0), Some(500));
    assert!(!frame.in_range(), "out-of-band channel flags the frame");
    // A range-invalid but checksum-valid frame is a good frame to the codec.
    assert_eq!(p.counters().frames_ok, 1);
    assert_eq!(p.counters().frames_bad, 0);

    // And a channel just above the band is likewise flagged.
    let mut hi = [1500u16; NUM_CHANNELS];
    hi[3] = CHAN_MAX_US + 1;
    let (_, hf) = push_one(&kat_frame(CMD_SERVO, &hi));
    assert!(!hf.unwrap().in_range());

    // The band edges themselves are in range.
    let mut edge = [1500u16; NUM_CHANNELS];
    edge[0] = CHAN_MIN_US;
    edge[1] = CHAN_MAX_US;
    let (_, ef) = push_one(&kat_frame(CMD_SERVO, &edge));
    assert!(ef.unwrap().in_range());
}

#[test]
fn kat_long_junk_never_panics_and_stays_bounded() {
    // A large, adversarial junk stream (many stray 0x20/0x40) must not panic and
    // must still deliver the trailing clean frame. The fixed 32-byte window is
    // the buffering bound; we assert recovery, which is only possible if resync
    // never wedges.
    let mut p = Parser::new();
    let mut frames = 0usize;
    for i in 0u32..5000 {
        let b = match i % 4 {
            0 => 0x20,
            1 => 0x40,
            2 => (i & 0xFF) as u8,
            _ => 0xFF,
        };
        if p.push(b).is_some() {
            frames += 1;
        }
    }
    let clean = kat_frame(CMD_SERVO, &[1500u16; NUM_CHANNELS]);
    p.feed(&clean, |f| {
        assert!(f.in_range());
        frames += 1;
    });
    assert!(frames >= 1, "the trailing clean frame must decode");
}

#[test]
fn kat_captured_frames_decode() {
    // No-op until real FGr8B captures are appended to CAPTURED_FRAMES.
    for raw in CAPTURED_FRAMES {
        let (_, f) = push_one(raw);
        let f = f.expect("captured frame must be a valid servo frame");
        assert!(f.in_range(), "captured channels should be within the servo band");
    }
}
