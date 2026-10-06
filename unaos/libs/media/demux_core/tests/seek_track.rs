// SPDX-License-Identifier: LGPL-3.0-or-later
//! SEEKTABLE (rmbp-ledger B433): `Demuxer::seek_track` with an AUDIO track as the reference, over TEST.M4A
//! (`$UNAOS_TESTF_DIR` or `unaos/target/testf`). The landing sample is the last whose pts is at or before the
//! target; `audio_core`'s MP4 seek (its own `stsc`/`stco`/`stsz` walk) restarts one unit earlier (its pre-roll) —
//! `audio_core/tests/seek_kat.rs` pins that unit's byte, this test pins both. Absent sample: SKIPPED out loud.
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"))
}

#[test]
fn testf_m4a_seek_track() {
    let Ok(b) = std::fs::read(dir().join("TEST.M4A")) else {
        eprintln!("SKIP TEST.M4A: not staged");
        return;
    };
    let mut d = demux_core::Demuxer::open(b).unwrap();
    let a = d.tracks().iter().position(|t| t.kind == demux_core::TrackKind::Audio).expect("audio track");
    let tb = d.tracks()[a].timebase;
    let samples: Vec<_> = d.track_samples(a).copied().collect();
    // the cross-check: at 200 ms both cores agree — this core lands on the unit whose pre-roll unit is the one
    // audio_core restarts at (byte 997, `audio_core/tests/seek_kat.rs` M4A_PREROLL_BYTE_200MS)
    let p = d.seek_track(a, 200_000_000).unwrap();
    let i = samples.iter().position(|s| tb.to_ns(s.pts) == p).unwrap();
    assert_eq!(samples[i - 1].offset, 997, "audio_core's pre-roll unit");
    for ms in [0i64, 50, 100, 223, 250] {
        let target = ms * 1_000_000;
        let p = d.seek_track(a, target).expect("seek");
        assert!(p <= target || ms == 0, "landed {p} after {target}");
        let pk = d.next_packet().expect("a packet after the seek");
        assert_eq!(tb.to_ns(pk.pts), p);
        let i = samples.iter().position(|s| tb.to_ns(s.pts) == p).unwrap();
        if let Some(n) = samples.get(i + 1) { assert!(tb.to_ns(n.pts) > target, "{ms} ms: not the last unit at or before"); }
        if ms == 223 { assert_eq!(samples[i].offset, 1234); }
        eprintln!("TEST.M4A @ {ms} ms: unit {i} pts_ns={p} byte={} prev_byte={:?}", samples[i].offset, i.checked_sub(1).map(|j| samples[j].offset));
    }
}
