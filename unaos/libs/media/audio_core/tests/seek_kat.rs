// SPDX-License-Identifier: LGPL-3.0-or-later
//! SEEKTABLE (rmbp-ledger B433): `Decoder::seek` over the test-f samples (`$UNAOS_TESTF_DIR` or `unaos/target/testf`,
//! staged by the builder from `unaos/builder/testf.list`) and the FLAC subset vectors. The proof: the PCM after a
//! seek is the full decode's PCM from the landed sample on — bit-exact for the lossless formats, within the
//! codec's pre-roll tolerance for the lossy ones — and every seek lands within one frame of its target.
//! Absent samples are SKIPPED out loud.
mod common;
use audio_core::{AudioDecoder, Decoder, SeekPoint};
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"))
}

fn full(b: &[u8]) -> (u32, usize, Vec<f32>) {
    let (info, pcm) = audio_core::decode_all(b).expect("full decode");
    (info.rate, info.channels as usize, pcm)
}

fn after_seek(b: &[u8], ms: u64) -> (Option<SeekPoint>, Vec<f32>) {
    let mut d = Decoder::open_bytes(b.to_vec()).expect("open");
    let ch = d.info().channels as usize;
    let p = d.seek(ms).expect("seek");
    let mut out = Vec::new();
    let mut buf = vec![0f32; 4096 * ch];
    loop {
        let n = d.next(&mut buf).expect("decode after seek");
        if n == 0 { break; }
        out.extend_from_slice(&buf[..n * ch]);
    }
    (p, out)
}

/// The sample offset in `full` where `got` (skipping its first `settle` frames) matches best, searched within
/// `±span` frames of `near`; returns (offset, max abs error over the compared window).
fn align(full: &[f32], got: &[f32], ch: usize, near: u64, span: u64, settle: usize) -> (u64, f32) {
    let win = 2048.min(got.len() / ch - settle.min(got.len() / ch));
    let mut best = (near, f32::MAX);
    let lo = near.saturating_sub(span);
    for k in lo..=near + span {
        let s = (k as usize + settle) * ch;
        if s + win * ch > full.len() { break; }
        let g = &got[settle * ch..(settle + win) * ch];
        let e = full[s..s + win * ch].iter().zip(g).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        if e < best.1 { best = (k, e); }
    }
    best
}

/// One file: seek to each of `targets` (ms); check the table, the landing and the PCM.
fn check(name: &str, b: &[u8], table: &str, frame: u64, lossless: bool, targets: &[u64]) {
    let (rate, ch, reference) = full(b);
    let total = (reference.len() / ch) as u64;
    for &ms in targets {
        let target = (ms * rate as u64 / 1000).min(total);
        let (p, got) = after_seek(b, ms);
        let p = p.unwrap_or_else(|| panic!("{name}: no seek table at {ms} ms"));
        assert_eq!(p.table, table, "{name} @ {ms} ms");
        assert!(p.landed.abs_diff(target) <= frame, "{name} @ {ms} ms: landed {} vs target {target} (frame {frame})", p.landed);
        if lossless {
            assert!(p.exact, "{name}: a lossless table seek is exact");
            assert_eq!(p.landed, target, "{name} @ {ms} ms");
            let want = &reference[(target as usize * ch).min(reference.len())..];
            assert_eq!(got.len(), want.len(), "{name} @ {ms} ms: length after seek");
            assert!(got == want, "{name} @ {ms} ms: PCM after seek differs from the full decode");
        } else if got.len() / ch > 64 {
            // lossy: where the output really is in the full decode (the pre-roll primes the decoder, so the PCM
            // matches from the first sample within the codec's float noise)
            let (k, err) = align(&reference, &got, ch, p.landed, frame + 64, 0);
            eprintln!("{name} @ {ms} ms: table={} exact={} landed={} target={target} actual={k} err={err:.2e} byte={}", p.table, p.exact, p.landed, p.byte);
            assert!(err < 1e-3, "{name} @ {ms} ms: PCM after seek does not match the full decode (err {err})");
            assert!(k.abs_diff(target) <= frame, "{name} @ {ms} ms: actual landing {k} vs target {target}");
            if p.exact { assert_eq!(k, target, "{name} @ {ms} ms: an exact seek lands on the target"); }
        }
        eprintln!("{name} @ {ms} ms: table={} exact={} landed_ms={} byte={}", p.table, p.exact as u8, p.landed_ms(rate), p.byte);
    }
}

fn testf(name: &str) -> Option<Vec<u8>> {
    match std::fs::read(dir().join(name)) {
        Ok(b) => Some(b),
        Err(_) => { eprintln!("SKIP {name}: not staged (unaos/builder/testf.list)"); None }
    }
}

#[test]
fn testf_wav_aiff_pcm() {
    if let Some(b) = testf("TEST.WAV") { check("TEST.WAV", &b, "pcm", 0, true, &[0, 50, 150, 280, 100_000]); }
    if let Some(b) = testf("TEST.AIF") { check("TEST.AIF", &b, "pcm", 0, true, &[0, 333, 1000, 2000]); }
    let s = common::synth::signal(48_000, 2, 24, 7);
    let w = common::synth::wav_int(48_000, 2, 3, 24, false, &s);
    check("synth.wav", &w, "pcm", 0, true, &[0, 1, 500, 999]);
}
