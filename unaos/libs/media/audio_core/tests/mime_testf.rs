// SPDX-License-Identifier: LGPL-3.0-or-later
//! OPENERS (rmbp-ledger B379): `mime_of` over the builder's test-f audio samples (fetched into
//! `unaos/target/testf/` or `$UNAOS_TESTF_DIR`, never committed), and TEST.M4A — the file Quarry could not open on
//! flight 23 — DECODES through the MP4 demuxer (`stsz`/`stco` sample tables) and the AAC-LC core, the path `play`
//! takes. Absent samples are SKIPPED out loud.
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"))
}

#[test]
fn testf_audio_typed() {
    let cases = [
        ("TEST.WAV", Some(("audio/wav", true))),
        ("TEST.FLAC", Some(("audio/flac", true))),
        ("TEST.OPUS", Some(("audio/ogg", true))),
        ("TEST.OGG", Some(("audio/ogg", true))),
        ("TEST.MP3", Some(("audio/mpeg", true))),
        ("TEST.AAC", Some(("audio/aac", false))),
        ("TEST.AIF", Some(("audio/aiff", true))),
        ("TEST.M4A", None), // ISO-BMFF: demux_core decides audio/mp4
    ];
    let mut seen = 0;
    for (name, want) in cases {
        let Ok(b) = std::fs::read(dir().join(name)) else {
            eprintln!("SKIP {name}: not fetched (unaos/target/testf)");
            continue;
        };
        seen += 1;
        assert_eq!(audio_core::mime_of(&b), want, "{name}");
    }
    eprintln!("mime_testf: {seen}/8 samples present");
}

#[test]
fn testf_m4a_decodes_aac() {
    let Ok(b) = std::fs::read(dir().join("TEST.M4A")) else {
        eprintln!("SKIP TEST.M4A: not fetched (unaos/target/testf)");
        return;
    };
    let (info, pcm) = audio_core::decode_all(&b).expect("TEST.M4A decodes");
    assert_eq!(info.format, audio_core::Format::Mp4);
    assert_eq!(info.codec, audio_core::Codec::Aac);
    let frames = pcm.len() / info.channels as usize;
    assert!(frames > info.rate as usize / 10, "TEST.M4A: {frames} frames at {} Hz", info.rate);
    assert!(pcm.iter().any(|s| s.abs() > 1e-3), "TEST.M4A decodes to silence");
    eprintln!("TEST.M4A: rate={} ch={} frames={frames}", info.rate, info.channels);
}

#[test]
fn text_is_not_audio() {
    assert_eq!(audio_core::mime_of(b"hello, plain text\n"), None);
}
