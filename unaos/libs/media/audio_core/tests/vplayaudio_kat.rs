// SPDX-License-Identifier: LGPL-3.0-or-later
//! VPLAYAUDIO (rmbp-ledger B475): a video's sound through `audio_core::container::open_demuxed` over the picture
//! job's `Demuxer::share` (the kernel's `vplay::shared_audio` chain on the host: one parse, the file's bytes shared).
//! MP4 (AAC, MP3 beside a video track) decodes and seeks identically through that door and through audio_core alone;
//! Matroska/WebM Vorbis (TEST.WEBM) seeks bit-exact from its packet index (`table=matroska`); Opus in WebM honours
//! `DiscardPadding` (TEST.OPUS's packets remuxed: the PCM is the Ogg decode's, end trim included) and seeks exactly.
//! The test-f samples come from `$UNAOS_TESTF_DIR` or `unaos/target/testf` (absent: SKIPPED out loud).
use audio_core::{AudioDecoder, Decoder};
use demux_core::Demuxer;
use std::path::PathBuf;

fn testf(n: &str) -> Option<Vec<u8>> {
    let d = std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"));
    let r = std::fs::read(d.join(n)).ok();
    if r.is_none() { eprintln!("SKIP {n}: not staged"); }
    r
}

fn drain(d: &mut Decoder) -> Vec<f32> {
    let ch = d.info().channels as usize;
    let mut buf = vec![0f32; 4096 * ch];
    let mut out = Vec::new();
    loop {
        let k = d.next(&mut buf).expect("decode");
        if k == 0 { break; }
        out.extend_from_slice(&buf[..k * ch]);
    }
    out
}

/// The kernel's path: the picture job's Demuxer stays open; the sound takes a share of it.
fn via_share(f: &[u8]) -> (Demuxer, Decoder) {
    let pic = Demuxer::open(f.to_vec()).expect("demux");
    let src = audio_core::container::open_demuxed(pic.share()).expect("open_demuxed");
    (pic, Decoder::from_source(src))
}

fn desc(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![tag, body.len() as u8];
    v.extend_from_slice(body);
    v
}

fn esds(oti: u8, dsi: &[u8]) -> Vec<u8> {
    let mut dcd = vec![oti, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    if !dsi.is_empty() { dcd.extend_from_slice(&desc(5, dsi)); }
    let mut es = vec![0, 1, 0];
    es.extend_from_slice(&desc(4, &dcd));
    es.extend_from_slice(&desc(6, &[2]));
    let mut v = vec![0, 0, 0, 0];
    v.extend_from_slice(&desc(3, &es));
    v
}

fn audio_track(fourcc: [u8; 4], oti: u8, dsi: &[u8], ts: u32, rate: u32, ch: u16, units: Vec<(Vec<u8>, u32)>) -> demux_core::build::MediaTrack {
    use demux_core::build::{MediaTrack, SampleSpec, TrackSpec};
    let mut dts = 0i64;
    let samples = units.into_iter().map(|(data, dur)| { let s = SampleSpec { data, dts, pts: dts, duration: dur, keyframe: true }; dts += dur as i64; s }).collect();
    let spec = TrackSpec {
        id: 2, kind: demux_core::TrackKind::Audio, fourcc, config_box: (&fourcc == b"mp4a").then_some(*b"esds"), codec_id: "A_AAC",
        config: if &fourcc == b"mp4a" { esds(oti, dsi) } else { Vec::new() }, timescale: ts, width: 0, height: 0,
        sample_rate: rate, channels: ch, bit_depth: 16, default_duration_ns: 0,
    };
    MediaTrack { spec, samples }
}

fn with_video(audio: demux_core::build::MediaTrack, edit: Option<i64>) -> Vec<u8> {
    let video = demux_core::build::test_pattern_track(1, 64, 48, 30, 12, 6);
    let opts = demux_core::build::Mp4Options { movie_timescale: 1000, chunk: 4, edits: edit.map(|m| vec![(audio.spec.id, 0, m)]).unwrap_or_default(), co64: false, fragment: 0 };
    demux_core::build::mp4(&[video, audio], &opts)
}

/// The same file through both doors: the full decode and four seeks are identical.
fn same_both_doors(name: &str, f: &[u8], seeks: &[u64]) {
    let alone = drain(&mut Decoder::open_bytes(f.to_vec()).unwrap());
    let (_pic, mut d) = via_share(f);
    let shared = drain(&mut d);
    assert!(!alone.is_empty() && alone == shared, "{name}: PCM through vplay's door differs from audio_core alone");
    for &ms in seeks {
        let mut a = Decoder::open_bytes(f.to_vec()).unwrap();
        let (_pic, mut b) = via_share(f);
        let (pa, pb) = (a.seek(ms).unwrap(), b.seek(ms).unwrap());
        assert_eq!(pa, pb, "{name} @ {ms} ms: seek point");
        assert!(drain(&mut a) == drain(&mut b), "{name} @ {ms} ms: PCM after seek");
        let p = pb.expect("a seek table");
        eprintln!("VPLAYAUDIO {name} @ {ms} ms: table={} exact={} landed={} byte={}", p.table, p.exact as u8, p.landed, p.byte);
    }
    eprintln!("VPLAYAUDIO {name}: {} samples identical through open_demuxed(share) and audio_core alone", shared.len());
}

#[test]
fn mp4_aac_mp3_beside_video() {
    if let Some(b) = testf("TEST.M4A") {
        let d = Demuxer::open(b).unwrap();
        let ai = d.tracks().iter().position(|t| t.kind == demux_core::TrackKind::Audio).unwrap();
        let t = d.tracks()[ai].clone();
        let units: Vec<_> = d.track_samples(ai).map(|s| (d.packet_at(s).data, s.duration as u32)).collect();
        let f = with_video(audio_track(*b"mp4a", 0x40, &t.config, t.timebase.den as u32, t.sample_rate, t.channels, units), t.edit.map(|e| e.media_time));
        same_both_doors("aac+video", &f, &[0, 1, 100, 10_000]);
    }
    if let Some(b) = testf("TEST.MP3") {
        let mut p = if b.len() >= 10 && &b[..3] == b"ID3" { 10 + ((b[6] as usize & 127) << 21 | (b[7] as usize & 127) << 14 | (b[8] as usize & 127) << 7 | (b[9] as usize & 127)) } else { 0 };
        let (mut frames, mut rate, mut ch) = (Vec::new(), 0, 0);
        while p + 4 <= b.len() {
            let Some(h) = audio_core::mp3::Header::parse(u32::from_be_bytes(b[p..p + 4].try_into().unwrap())) else { p += 1; continue };
            let n = h.frame_len();
            if n == 0 || p + n > b.len() { break; }
            rate = h.rate();
            ch = h.channels() as u16;
            frames.push((b[p..p + n].to_vec(), h.samples() as u32));
            p += n;
        }
        let f = with_video(audio_track(*b".mp3", 0, &[], rate, rate, ch, frames), None);
        same_both_doors("mp3+video", &f, &[0, 1, 100]);
    }
}

/// Seek `f` to each target through vplay's door: exact, `table=matroska`, and the PCM after it is the full decode's
/// from the target on — bit-exact (`tol` = 0: Vorbis) or converged (Opus: the fresh decoder's RFC 7845 §4.6 80 ms
/// pre-roll; the packet index lands closer to the target than an Ogg page does, so the landing's bound is looser than
/// SEEKTABLE2's page KAT — TEST.OPUS (SILK, mono) at 100 ms: 1.3e-1 at the landing, 0 from 250 ms after it).
fn seeks(name: &str, f: &[u8], reference: &[f32], ch: usize, rate: u64, targets: &[u64], tol: f32) {
    let total = (reference.len() / ch) as u64;
    for &ms in targets {
        let target = (ms * rate / 1000).min(total);
        let (_pic, mut d) = via_share(f);
        let p = d.seek(ms).unwrap().unwrap_or_else(|| panic!("{name}: no seek table at {ms} ms"));
        assert_eq!((p.table, p.exact, p.landed), ("matroska", true, target), "{name} @ {ms} ms");
        let got = drain(&mut d);
        let want = &reference[target as usize * ch..];
        assert_eq!(got.len(), want.len(), "{name} @ {ms} ms: length after seek");
        let err = got.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        let settled = ((250 * rate / 1000) as usize * ch).min(got.len());
        let tail = got[settled..].iter().zip(&want[settled..]).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        eprintln!("VPLAYAUDIO {name} @ {ms} ms: table=matroska exact=1 landed={} byte={} preroll={} err={err:.1e} err_after_250ms={tail:.1e}", p.landed, p.byte, target - p.sample);
        if tol == 0.0 { assert!(got == want, "{name} @ {ms} ms: PCM after seek is not the full decode's"); }
        else { assert!(err < tol && tail < 2e-3, "{name} @ {ms} ms: err {err} tail {tail}"); }
    }
}

#[test]
fn webm_vorbis_index_seek() {
    let Some(b) = testf("TEST.WEBM") else { return };
    let (_pic, mut d) = via_share(&b);
    let i = d.info();
    let ch = i.channels as usize;
    let full = drain(&mut d);
    assert_eq!(i.frames, Some((full.len() / ch) as u64), "the packet index counts the decode");
    // the retired kernel adapter's decode (packets straight into VorbisDecoder) is the reference
    let mut dm = Demuxer::open(b.clone()).unwrap();
    let at = dm.audio_track().unwrap().clone();
    let h = audio_core::container::xiph_split(&at.config).unwrap();
    let mut dec = audio_core::vorbis::VorbisDecoder::new(audio_core::vorbis::Setup::parse(h[0], h[2]).unwrap());
    let (mut out, mut want) = (Vec::new(), Vec::new());
    while let Some(p) = dm.next_packet() {
        if p.track != at.id { continue; }
        let n = dec.decode(&p.data, &mut out).unwrap_or(0);
        for k in 0..n { for c in 0..ch { want.push(out[c][k]); } }
    }
    assert!(full == want, "TEST.WEBM: open_demuxed's Vorbis is the packet decode's");
    let dur = (full.len() / ch) as u64 * 1000 / i.rate as u64;
    eprintln!("VPLAYAUDIO TEST.WEBM vorbis rate={} ch={ch} frames={} ms={dur}", i.rate, full.len() / ch);
    seeks("TEST.WEBM", &b, &full, ch, i.rate as u64, &[0, 1, 10, 100, 500, dur / 3, dur / 2, dur.saturating_sub(30), dur + 1000], 0.0);
}

#[test]
fn webm_opus_discard_padding_and_seek() {
    use demux_core::build::{MediaTrack, MkvOptions, SampleSpec, TrackSpec};
    let Some(b) = testf("TEST.OPUS") else { return };
    let (_, want) = audio_core::decode_all(&b).unwrap();
    let mut r = audio_core::ogg::OggReader::new(audio_core::io::ByteStream::new(Box::new(audio_core::io::VecReader::new(b.clone()))));
    let mut pk = Vec::new();
    while let Some(p) = r.next_packet().unwrap() { pk.push(p.data); }
    let head = audio_core::opus::OpusHead::parse(&pk[0]).unwrap();
    let ch = head.channels;
    let mut t = 0i64;
    let samples: Vec<SampleSpec> = pk[2..].iter().map(|p| {
        let (toc, _, sizes) = audio_core::opus::decoder::parse_packet(p).unwrap();
        let d = sizes.len() as i64 * audio_core::opus::decoder::packet_samples_per_frame(toc, 48_000) as i64;
        let s = SampleSpec { data: p.clone(), dts: t, pts: t, duration: d as u32, keyframe: true };
        t += d;
        s
    }).collect();
    // the Ogg end trim (the last granule) as the Matroska DiscardPadding of the last block
    let pad = t as u64 - head.pre_skip as u64 - (want.len() / ch) as u64;
    let spec = TrackSpec {
        id: 1, kind: demux_core::TrackKind::Audio, fourcc: *b"Opus", config_box: None, codec_id: "A_OPUS", config: pk[0].clone(),
        timescale: 48_000, width: 0, height: 0, sample_rate: 48_000, channels: ch as u16, bit_depth: 0, default_duration_ns: 0,
    };
    let webm = demux_core::build::mkv(&[MediaTrack { spec, samples }], &MkvOptions { discard_last_ns: pad * 1_000_000_000 / 48_000, ..Default::default() });
    let dm = Demuxer::open(webm.clone()).unwrap();
    let last = dm.samples().last().unwrap().discard_ns;
    assert!(pad == 0 || last > 0, "the writer's DiscardPadding reads back");
    let (_pic, mut d) = via_share(&webm);
    assert_eq!(d.info().frames, Some((want.len() / ch) as u64), "the index's length is the Ogg's");
    let got = drain(&mut d);
    assert!(got == want, "Opus in WebM with DiscardPadding decodes to TEST.OPUS's PCM, end trim included ({} vs {})", got.len(), want.len());
    eprintln!("VPLAYAUDIO TEST.OPUS in WebM: packets={} ch={ch} pad={pad} discard_ns={last} samples={} identical to the Ogg decode", pk.len() - 2, got.len() / ch);
    let dur = (want.len() / ch) as u64 / 48;
    seeks("TEST.OPUS in WebM", &webm, &want, ch, 48_000, &[0, 1, 10, 79, 81, 100, dur / 2, dur.saturating_sub(30), dur + 1000], 2e-1);
}
