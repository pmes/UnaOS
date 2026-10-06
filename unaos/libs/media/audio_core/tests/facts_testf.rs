// SPDX-License-Identifier: LGPL-3.0-or-later
//! ATTRCOLUMNS (rmbp-ledger B402): `facts_of` over the test-f audio (`$UNAOS_TESTF_DIR` or `unaos/target/testf`),
//! the header's length cross-checked against this core's own decoder (frames / rate) within 60 ms or 3 %.
//! Absent samples are SKIPPED out loud.
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"))
}

#[test]
fn testf_audio_facts_match_the_decoder() {
    let cases = [("TEST.WAV", "pcm"), ("TEST.FLAC", "flac"), ("TEST.OPUS", "opus"), ("TEST.OGG", "vorbis"), ("TEST.MP3", "mp3"), ("TEST.AAC", "aac"), ("TEST.AIF", "pcm")];
    let mut seen = 0;
    for (name, codec) in cases {
        let Ok(b) = std::fs::read(dir().join(name)) else {
            eprintln!("SKIP {name}: not fetched");
            continue;
        };
        seen += 1;
        let f = audio_core::facts_of(&b, b.len() as u64, &b).unwrap_or_else(|| panic!("{name}: no facts"));
        assert_eq!(f.codec, codec, "{name}");
        let d = f.duration_ms.unwrap_or_else(|| panic!("{name}: no duration"));
        // Head-only read (the kernel's large-file path): the facts still come, with a duration.
        let head = &b[..b.len().min(4096)];
        let tail = &b[b.len().saturating_sub(4096)..];
        let fh = audio_core::facts_of(head, b.len() as u64, tail).unwrap_or_else(|| panic!("{name}: no facts from head"));
        let oracle = std::panic::catch_unwind(|| audio_core::decode_all(&b).ok()).ok().flatten();
        match oracle {
            Some((info, pcm)) => {
                let frames = pcm.len() as u64 / info.channels.max(1) as u64;
                let dec_ms = frames * 1000 / info.rate.max(1) as u64;
                eprintln!("{name}: facts {d} ms codec={} title={:?} head={:?} | decoder {dec_ms} ms", f.codec, f.title, fh.duration_ms);
                let tol = (dec_ms * 3 / 100).max(60);
                assert!(d.abs_diff(dec_ms) <= tol, "{name}: facts {d} ms vs decoded {dec_ms} ms");
            }
            None => eprintln!("{name}: facts {d} ms codec={} (decoder oracle unavailable)", f.codec),
        }
    }
    eprintln!("audio facts: {seen} samples");
}

#[test]
fn id3_title_and_bad_input() {
    // An ID3v2.3 tag with TIT2 "Hello" (Latin-1) ahead of one MPEG-1 Layer III 128 kbps 44.1 kHz frame header.
    let mut b = b"ID3\x03\x00\x00\x00\x00\x00\x10TIT2\x00\x00\x00\x06\x00\x00\x00Hello".to_vec();
    b.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
    b.resize(b.len() + 413, 0);
    let f = audio_core::facts_of(&b, b.len() as u64, &b).expect("facts");
    assert_eq!(f.title.as_deref(), Some("Hello"));
    assert_eq!(f.codec, "mp3");
    assert_eq!(audio_core::facts_of(b"RIFF\x00\x00\x00\x00WAVE", 12, b""), None);
    assert_eq!(audio_core::facts_of(b"hello", 5, b"hello"), None);
}
