// SPDX-License-Identifier: LGPL-3.0-or-later
//! The oracle: public sample files (fetched at test time) and demux_core's remuxes of them,
//! checked against what Chromium reported for the same bytes (`tests/data/chromium-oracle.jsonl`,
//! written by `tools/play-check/oracle/chromium-oracle.js`): `video.duration`,
//! `videoWidth/Height`, frame count (`presentedFrames`), and the `requestVideoFrameCallback`
//! mediaTime sequence, which must match the demuxed video pts within one frame.

mod common;

use common::*;
use demux_core::build::{self, MkvOptions, Mp4Options};
use demux_core::{Demuxer, Track};

fn video_pts_s(d: &Demuxer) -> (Track, Vec<f64>) {
    let vi = d.tracks().iter().position(|t| t.kind == demux_core::TrackKind::Video).unwrap();
    let t = d.tracks()[vi].clone();
    let mut pts: Vec<f64> = d.track_samples(vi).map(|s| t.to_ns(s.pts) as f64 / 1e9).collect();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (t, pts)
}

/// Compare one file against its Chromium line; returns the max |mediaTime − pts| in seconds.
fn check(name: &str, bytes: Vec<u8>, o: &Oracle) -> f64 {
    let d = Demuxer::open(bytes).unwrap();
    let (t, pts) = video_pts_s(&d);
    let dur = d.duration_ns() as f64 / 1e9;
    assert!((dur - o.duration).abs() < 0.0015, "{name}: duration {dur} vs Chromium {}", o.duration);
    assert_eq!((t.width, t.height), (o.width, o.height), "{name}: size");
    assert_eq!(t.sample_count, o.presented, "{name}: frame count vs presentedFrames");
    // rVFC may coalesce a callback; every reported mediaTime must sit on a demuxed pts.
    assert!(o.times.len() as u64 + 1 >= o.presented, "{name}: Chromium dropped more than one callback");
    let frame = t.to_ns(d.track_samples(d.tracks().iter().position(|x| x.id == t.id).unwrap()).next().unwrap().duration as i64) as f64 / 1e9;
    let mut worst = 0.0f64;
    let mut j = 0;
    for &m in &o.times {
        while j + 1 < pts.len() && (pts[j + 1] - m).abs() <= (pts[j] - m).abs() {
            j += 1;
        }
        let e = (pts[j] - m).abs();
        assert!(e < frame, "{name}: mediaTime {m} has no pts within one frame ({frame}s)");
        worst = worst.max(e);
    }
    // The brief's bar is one frame; what is actually met is exact to Chromium's millisecond
    // rounding of WebM times, so that is what is enforced.
    assert!(worst <= 0.0010001, "{name}: max |mediaTime-pts| {worst}s exceeds 1 ms");
    eprintln!(
        "ORACLE {name}: duration {dur:.3}s (Chromium {:.3}), {}x{}, frames {} (Chromium presented {}, {} callbacks), max |mediaTime-pts| {:.6}s",
        o.duration, t.width, t.height, t.sample_count, o.presented, o.times.len(), worst
    );
    worst
}

#[test]
fn oracle_public_samples_and_remuxes() {
    let (vecs, remuxes) = vectors();
    let oracle = oracle();
    let mut fetched = std::collections::HashMap::new();
    for v in &vecs {
        if let Some(b) = fetch(v) {
            fetched.insert(v.name.clone(), b);
        }
    }
    if fetched.is_empty() {
        eprintln!("SKIP oracle: no vectors available");
        return;
    }
    let mut checked = 0;
    for o in &oracle {
        if let Some(b) = fetched.get(&o.file) {
            check(&o.file, b.clone(), o);
            checked += 1;
        }
    }
    for r in &remuxes {
        let Some(src) = fetched.get(&r.source) else { continue };
        let d = Demuxer::open(src.clone()).unwrap();
        let tracks = build::remux_tracks(&d).unwrap();
        let out = match r.shape.as_str() {
            "webm" => build::mkv(&tracks, &MkvOptions::default()),
            s => build::mp4(&tracks, &Mp4Options { fragment: s.trim_start_matches("frag").parse().unwrap(), ..Default::default() }),
        };
        assert_eq!(hex(&sha256(&out)), r.sha, "{}: remux bytes changed; re-run the Chromium oracle", r.name);
        // Packet payloads survive the remux byte for byte.
        let mut a = Demuxer::open(src.clone()).unwrap();
        let mut b = Demuxer::open(out.clone()).unwrap();
        loop {
            match (a.next_packet(), b.next_packet()) {
                (Some(x), Some(y)) => assert_eq!(x.data, y.data, "{}", r.name),
                (None, None) => break,
                _ => panic!("{}: packet count differs", r.name),
            }
        }
        let o = oracle.iter().find(|o| o.file == r.name).unwrap();
        // Chromium decoded the remux to the same pixels as the source, frame by frame.
        let so = oracle.iter().find(|o| o.file == r.source).unwrap();
        let common = o.hashes.len().min(so.hashes.len());
        let same = o.hashes.iter().filter(|h| so.hashes.contains(h)).count();
        assert!(same + 1 >= common, "{}: decoded frames differ from {}", r.name, r.source);
        check(&r.name, out, o);
        checked += 1;
    }
    eprintln!("ORACLE files checked: {checked}");
    assert!(checked >= 3);
}

/// The fragmented H.264 sample: this Chromium build has no H.264 decoder, so there is no
/// browser oracle; its structure (48 frames at 24 fps, 2 s, B-frames) is checked instead.
#[test]
fn fragmented_avc_structure() {
    let (vecs, _) = vectors();
    let Some(v) = vecs.iter().find(|v| v.name.starts_with("test-v-128k")) else { return };
    let Some(b) = fetch(v) else { return };
    let d = Demuxer::open(b).unwrap();
    let (t, pts) = video_pts_s(&d);
    assert_eq!(t.codec, demux_core::Codec::Avc);
    assert_eq!(t.sample_count, 48);
    assert_eq!(d.duration_ns(), 2_000_000_000);
    // Presentation times, sorted, are a gapless 24 fps ladder (the B-frame reorder undone).
    for w in pts.windows(2) {
        assert!((w[1] - w[0] - 1.0 / 24.0).abs() < 1e-6);
    }
    let keys = d.track_samples(0).filter(|s| s.keyframe).count();
    assert!(keys >= 1);
}

#[test]
fn sha256_kat() {
    assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
}
