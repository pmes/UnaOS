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

/// Does this native FLAC carry a SEEKTABLE block with real points?
fn has_seektable(b: &[u8]) -> bool {
    let mut p = 4;
    while p + 4 <= b.len() {
        let (t, last, len) = (b[p] & 0x7F, b[p] & 0x80 != 0, u32::from_be_bytes([0, b[p + 1], b[p + 2], b[p + 3]]) as usize);
        if t == 3 && b[p + 4..p + 4 + len].chunks_exact(18).any(|c| c[..8] != [0xFF; 8]) { return true; }
        if last { break; }
        p += 4 + len;
    }
    false
}

#[test]
fn testf_flac() {
    let Some(b) = testf("TEST.FLAC") else { return };
    check("TEST.FLAC", &b, "flac", 0, true, &[0, 1, 37, 100, 150, 250, 289, 10_000]);
}

#[test]
fn flac_subset_seek() {
    let mut seen = (0, 0);
    for v in common::vectors("flac") {
        let Some(b) = common::fetch(&v) else { continue };
        if &b[..4] != b"fLaC" { continue; }
        let (rate, ch, pcm) = full(&b);
        let dur = (pcm.len() / ch) as u64 * 1000 / rate as u64;
        let t = has_seektable(&b);
        seen.0 += 1;
        seen.1 += t as u32;
        let name = v.path.file_name().unwrap().to_string_lossy().into_owned();
        check(&name, &b, "flac", 0, true, &[dur / 7, dur / 2, dur * 5 / 6, dur.saturating_sub(1)]);
    }
    eprintln!("flac subset: {} files seeked, {} with a SEEKTABLE block", seen.0, seen.1);
}

/// TEST.MP3 (a Xing + LAME VBR file) through its TOC; then the same frames with the Xing frame replaced by a VBRI
/// frame whose TOC is built from the real frame lengths (the exact path); then with no tag at all (the CBR estimate).
#[test]
fn testf_mp3() {
    let Some(b) = testf("TEST.MP3") else { return };
    let targets = [0, 40, 100, 160, 250];
    check("TEST.MP3", &b, "xing", 1152, false, &targets);
    // the frames after the Xing frame: walk the headers
    let id3 = 10 + (((b[6] as usize) << 21) | ((b[7] as usize) << 14) | ((b[8] as usize) << 7) | b[9] as usize);
    let flen = |o: usize| -> usize {
        let h = u32::from_be_bytes(b[o..o + 4].try_into().unwrap());
        let br = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320][((h >> 12) & 15) as usize] * 1000;
        let sr = [44100, 48000, 32000][((h >> 10) & 3) as usize];
        144 * br / sr + ((h >> 9) & 1) as usize
    };
    let xing_len = flen(id3);
    let mut lens = Vec::new();
    let mut o = id3 + xing_len;
    while o + 4 <= b.len() && b[o] == 0xFF { lens.push(flen(o)); o += flen(o); }
    let audio = &b[id3 + xing_len..o];
    // a VBRI frame: same header as the Xing frame, VBRI 32 bytes in, one TOC entry per 2 frames (16-bit, scale 1)
    let mut v = b[id3..id3 + xing_len].to_vec();
    v[4..].fill(0);
    let entries: Vec<u16> = lens.chunks(2).map(|c| c.iter().sum::<usize>() as u16).collect();
    let mut tag = b"VBRI".to_vec();
    tag.extend_from_slice(&1u16.to_be_bytes());
    tag.extend_from_slice(&0u16.to_be_bytes());
    tag.extend_from_slice(&75u16.to_be_bytes());
    tag.extend_from_slice(&(audio.len() as u32 + xing_len as u32).to_be_bytes());
    tag.extend_from_slice(&(lens.len() as u32).to_be_bytes());
    tag.extend_from_slice(&(entries.len() as u16).to_be_bytes());
    tag.extend_from_slice(&1u16.to_be_bytes());
    tag.extend_from_slice(&2u16.to_be_bytes());
    tag.extend_from_slice(&2u16.to_be_bytes());
    for e in &entries { tag.extend_from_slice(&e.to_be_bytes()); }
    assert!(36 + tag.len() <= v.len());
    v[36..36 + tag.len()].copy_from_slice(&tag);
    let mut vb = v.clone();
    vb.extend_from_slice(audio);
    // without the LAME gapless trim the decode starts at the first frame: compare against that file's own decode
    check("TEST.MP3+VBRI", &vb, "vbri", 1152, false, &targets);
    check("TEST.MP3 (no tag)", audio, "cbr", 1152, false, &targets);
}

/// TEST.M4A (AAC-LC in MP4, an edit list's priming) through its sample table.
#[test]
fn testf_m4a() {
    let Some(b) = testf("TEST.M4A") else { return };
    check("TEST.M4A", &b, "mp4", 1024, false, &[0, 10, 23, 100, 150, 250]);
    let mut d = Decoder::open_bytes(b.clone()).unwrap();
    let p = d.seek(200).unwrap().unwrap();
    // the pre-roll unit's byte — demux_core's `seek_track` (its stts times) lands on the unit after it
    // (`unaos/libs/media/demux_core/tests/seek_track.rs` pins the same two offsets)
    eprintln!("TEST.M4A @ 200 ms: pre-roll byte={} sample={}", p.byte, p.sample);
    assert_eq!(p.byte, M4A_PREROLL_BYTE_200MS);
}
const M4A_PREROLL_BYTE_200MS: u64 = 997;

/// SEEKTABLE2 (rmbp-ledger B469): TEST.OGG (Vorbis) and TEST.OPUS (Opus) through the page-granule bisection,
/// TEST.AAC (ADTS) through the header-walk index. Vorbis (one-block pre-roll) and ADTS (one-AU pre-roll) are
/// PCM-identical with the full decode from the target on; Opus (80 ms pre-roll, RFC 7845 §4.6) lands on the
/// target sample with the PCM within the float noise of the converged decoder.
#[test]
fn testf_ogg_adts() {
    if let Some(b) = testf("TEST.OGG") { check("TEST.OGG", &b, "ogg", 0, true, &[0, 1, 10, 100, 150, 250, 280, 10_000]); }
    if let Some(b) = testf("TEST.OPUS") { check_opus("TEST.OPUS", &b, &[0, 1, 10, 79, 81, 100, 150, 250, 10_000]); }
    if let Some(b) = testf("TEST.AAC") { check("TEST.AAC", &b, "adts", 0, true, &[0, 1, 23, 24, 100, 150, 250, 10_000]); }
}

/// The packets of an Ogg file (headers included), through the core's own page reader.
fn ogg_packets(b: &[u8]) -> Vec<Vec<u8>> {
    use audio_core::io::{ByteStream, VecReader};
    let mut r = audio_core::ogg::OggReader::new(ByteStream::new(Box::new(VecReader::new(b.to_vec()))));
    let mut v = Vec::new();
    while let Some(p) = r.next_packet().unwrap() { v.push(p.data); }
    v
}

/// An Ogg stream of `headers` (each ending its page) then `audio` packets (each with the granule at its end), laid
/// into pages of at most `body` bytes — packets span pages (continued flag) and pages hold several packets, so a
/// page's granule is that of the last packet completing on it, −1 when none does (RFC 3533 §6).
fn paginate(headers: &[Vec<u8>], audio: &[(Vec<u8>, u64)], body: usize) -> Vec<u8> {
    use audio_core::crc::crc32_ogg;
    let mut out = Vec::new();
    let mut seq = 0u32;
    let mut emit = |flags: u8, granule: u64, lacing: &[u8], data: &[u8], out: &mut Vec<u8>| {
        let mut h = b"OggS".to_vec();
        h.push(0);
        h.push(flags);
        h.extend_from_slice(&granule.to_le_bytes());
        h.extend_from_slice(&0x5eeu32.to_le_bytes());
        h.extend_from_slice(&seq.to_le_bytes());
        h.extend_from_slice(&[0; 4]);
        h.push(lacing.len() as u8);
        h.extend_from_slice(lacing);
        h.extend_from_slice(data);
        let crc = crc32_ogg(&h);
        h[22..26].copy_from_slice(&crc.to_le_bytes());
        out.extend(h);
        seq += 1;
    };
    let lace = |n: usize| { let mut l = vec![255u8; n / 255]; l.push((n % 255) as u8); l };
    for (i, h) in headers.iter().enumerate() { emit(if i == 0 { 2 } else { 0 }, 0, &lace(h.len()), h, &mut out); }
    // the audio as one segment stream; each packet's segments, then cut into pages
    let segs: Vec<(u8, usize, Option<u64>)> = audio.iter().enumerate().flat_map(|(_, (p, g))| {
        let l = lace(p.len());
        let n = l.len();
        l.into_iter().enumerate().map(move |(k, s)| (s, 0, if k + 1 == n { Some(*g) } else { None })).collect::<Vec<_>>()
    }).collect();
    let data: Vec<u8> = audio.iter().flat_map(|(p, _)| p.iter().copied()).collect();
    let (mut at, mut i, mut continued) = (0usize, 0usize, false);
    while i < segs.len() {
        let (mut lacing, mut size, mut gran) = (Vec::new(), 0usize, u64::MAX);
        while i < segs.len() && lacing.len() < 255 && size + segs[i].0 as usize <= body.max(255) {
            lacing.push(segs[i].0);
            size += segs[i].0 as usize;
            if let Some(g) = segs[i].2 { gran = g; }
            i += 1;
        }
        let last = i == segs.len();
        emit(if continued { 1 } else { 0 } | if last { 4 } else { 0 }, gran, &lacing, &data[at..at + size], &mut out);
        at += size;
        continued = *lacing.last().unwrap() == 255;
    }
    out
}

/// SEEKTABLE2: long multi-page streams, so the bisection and the page resume really run — TEST.OGG's and TEST.OPUS's
/// packets looped 24 times and re-paged (packets spanning pages, several per page), TEST.AAC's frames looped 24
/// times; plus Chromium's own multi-page Ogg files (vectors.txt: bear-opus, opus-trimming-test, 9ch Vorbis, bear-flac).
#[test]
fn ogg_adts_long_streams() {
    if let Some(b) = testf("TEST.OGG") {
        let pk = ogg_packets(&b);
        let audio: Vec<Vec<u8>> = (0..24).flat_map(|_| pk[3..].iter().cloned()).collect();
        let setup = audio_core::vorbis::Setup::parse(&pk[0], &pk[2]).unwrap();
        let mut dec = audio_core::vorbis::VorbisDecoder::new(setup);
        let (mut out, mut g) = (Vec::new(), 0u64);
        let audio: Vec<(Vec<u8>, u64)> = audio.into_iter().map(|p| { g += dec.decode(&p, &mut out).unwrap() as u64; (p, g) }).collect();
        let f = paginate(&pk[..3], &audio, 700);
        let dur = g * 1000 / 44_100;
        eprintln!("long vorbis: {} bytes, {} ms", f.len(), dur);
        check("TEST.OGG x24", &f, "ogg", 0, true, &[0, 5, dur / 7, dur / 3, dur / 2, dur * 5 / 6, dur - 1, dur + 1000]);
    }
    if let Some(b) = testf("TEST.OPUS") {
        let pk = ogg_packets(&b);
        let head = audio_core::opus::OpusHead::parse(&pk[0]).unwrap();
        let mut g = head.pre_skip as u64;
        let audio: Vec<(Vec<u8>, u64)> = (0..24).flat_map(|_| pk[2..].iter().cloned()).map(|p| {
            let (toc, _, sizes) = audio_core::opus::decoder::parse_packet(&p).unwrap();
            g += sizes.len() as u64 * audio_core::opus::decoder::packet_samples_per_frame(toc, 48_000) as u64;
            (p, g)
        }).collect();
        let f = paginate(&pk[..2], &audio, 500);
        let dur = (g - head.pre_skip as u64) / 48;
        eprintln!("long opus: {} bytes, {} ms", f.len(), dur);
        check_opus("TEST.OPUS x24", &f, &[0, 5, 79, 81, dur / 7, dur / 3, dur / 2, dur * 5 / 6, dur - 30, dur + 1000]);
    }
    if let Some(b) = testf("TEST.AAC") {
        let f: Vec<u8> = (0..24).flat_map(|_| b.iter().copied()).collect();
        let (rate, ch, pcm) = full(&f);
        let dur = (pcm.len() / ch) as u64 * 1000 / rate as u64;
        check("TEST.AAC x24", &f, "adts", 0, true, &[0, 5, dur / 7, dur / 3, dur / 2, dur * 5 / 6, dur - 1, dur + 1000]);
    }
    for v in common::vectors("lossy").into_iter().chain(common::vectors("lossless")) {
        let name = v.path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".ogg") || name.contains("sfx") { continue; }
        let Some(b) = common::fetch(&v) else { continue };
        let (rate, ch, pcm) = full(&b);
        let dur = (pcm.len() / ch) as u64 * 1000 / rate as u64;
        let t = [0, dur / 7, dur / 2, dur * 5 / 6, dur.saturating_sub(1)];
        if ogg_packets(&b)[0].starts_with(b"OpusHead") { check_opus(&name, &b, &t) } else { check(&name, &b, "ogg", 0, true, &t) }
    }
}

/// Ogg Opus: the landing is exact (the page granule counts samples) and the output is the full decode's from the
/// target on, sample for sample in length; the fresh decoder's PCM converges over the pre-roll (CELT's inter-frame
/// energy prediction and SILK's LPC/LTP state decay geometrically: < 5e-2 at the landing on these streams, < 2e-3 from
/// 250 ms after it — under -54 dBFS — and on the CELT streams bit-identical with the full decode soon after).
fn check_opus(name: &str, b: &[u8], targets: &[u64]) {
    let (_, ch, reference) = full(b);
    let total = (reference.len() / ch) as u64;
    for &ms in targets {
        let target = (ms * 48).min(total);
        let (p, got) = after_seek(b, ms);
        let p = p.unwrap_or_else(|| panic!("{name}: no seek table at {ms} ms"));
        assert_eq!((p.table, p.exact, p.landed), ("ogg", true, target), "{name} @ {ms} ms");
        let want = &reference[target as usize * ch..];
        assert_eq!(got.len(), want.len(), "{name} @ {ms} ms: length after seek");
        let err = got.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        let settled = (250 * 48 * ch).min(got.len());
        let tail = got[settled..].iter().zip(&want[settled..]).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        eprintln!("{name} @ {ms} ms: table=ogg exact=1 landed_ms={} byte={} preroll_ms={} err={err:.1e} err_after_250ms={tail:.1e}", p.landed / 48, p.byte, (target - p.sample) / 48);
        assert!(err < 5e-2, "{name} @ {ms} ms: PCM after seek is not the full decode's (err {err})");
        assert!(tail < 2e-3, "{name} @ {ms} ms: not converged 250 ms after the landing (err {tail})");
    }
}
