// SPDX-License-Identifier: LGPL-3.0-or-later
//! OPENERS (rmbp-ledger B379): `mime_of` over the builder's test-f containers (fetched into `unaos/target/testf/`
//! or `$UNAOS_TESTF_DIR`, never committed) — whole files AND their first 512 bytes (the kernel's sniff window) —
//! and TEST.M4A demuxes: one AAC audio track whose `stsz`/`stco` samples all lie inside the file. Absent samples
//! are SKIPPED out loud.
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"))
}

#[test]
fn testf_containers_typed() {
    for (name, want) in [("TEST.M4A", "audio/mp4"), ("TEST.MP4", "video/mp4"), ("TEST.WEBM", "video/webm")] {
        let Ok(b) = std::fs::read(dir().join(name)) else {
            eprintln!("SKIP {name}: not fetched (unaos/target/testf)");
            continue;
        };
        assert_eq!(demux_core::mime::mime_of(&b), Some(want), "{name} whole");
        assert_eq!(demux_core::mime::mime_of(&b[..512.min(b.len())]), Some(want), "{name} head");
    }
}

#[test]
fn testf_m4a_demuxes() {
    let Ok(b) = std::fs::read(dir().join("TEST.M4A")) else {
        eprintln!("SKIP TEST.M4A: not fetched (unaos/target/testf)");
        return;
    };
    let len = b.len() as u64;
    let d = demux_core::Demuxer::open(b).expect("TEST.M4A demuxes");
    assert!(d.video_track().is_none());
    let a = d.audio_track().expect("one audio track");
    assert_eq!(a.codec, demux_core::Codec::Aac);
    assert!(a.sample_count > 0);
    assert!(d.samples().iter().all(|s| s.offset + s.size as u64 <= len));
    eprintln!("TEST.M4A: aac samples={}", a.sample_count);
}

#[test]
fn brands_and_handlers() {
    use demux_core::mime::{iso_brand_kind, iso_mime};
    use demux_core::TrackKind;
    assert_eq!(iso_brand_kind(b"M4A \0\0\x02\0isomiso2"), Some(TrackKind::Audio));
    assert_eq!(iso_brand_kind(b"isom\0\0\x02\0iso2av01mp41"), Some(TrackKind::Video));
    assert_eq!(iso_brand_kind(b"iso5\0\0\x02\0iso6mp41"), None);
    assert_eq!(iso_mime(Some(b"iso5\0\0\x02\0iso6mp41"), None), "video/mp4");
    assert_eq!(demux_core::mime::mime_of(b"plain text, not a container"), None);
}
