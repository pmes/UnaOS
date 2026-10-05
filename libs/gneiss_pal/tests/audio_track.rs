// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AUDIOTRACK (LEDGER SR45) known answers for `dsp::audio_track`'s compressed registry.
//!
//! Each codec's packets are carried through demux_core's own MP4 and Matroska writers, demuxed
//! back, and decoded by the registry behind its gapless trimmer; the answer is the decoder
//! AUDIOCODEC already proved:
//!
//! * **Opus** — the twelve libopus conformance streams (`audio_core/tests/data/opus`): untrimmed,
//!   the PCM MD5 equals libopus 1.5.2's (`expected.txt`), in both containers; trimmed, the output
//!   is that PCM minus the 312-sample pre-skip, whether the pre-skip is stated by the Matroska
//!   `OpusHead`, an MP4 edit list, or (no edit list) the MP4 `dOps` alone.
//! * **Vorbis** — the ten libvorbis streams (`data/vorbis/*.ogg`) re-muxed into WebM with a
//!   Xiph-laced CodecPrivate: sample-for-sample equal to `audio_core`'s Ogg decode (proven
//!   135 dB against libvorbis), the WebM side longer only by the Ogg end trim it has no way to
//!   carry.
//! * **AAC** — the four MP4 wrappings (`data/aac/m*.m4a`: edit list, `iTunSMPB` with the moov
//!   last, fragmented, plain) through `Timing::of`: bit-identical to `audio_core`'s MP4 file decode
//!   (proven against faad2), equal frame counts — the gapless start and end both honoured.
//! * **MP3** — Chromium's `sfx.mp3` split into frames (Xing frame dropped), muxed into MP4 with
//!   an edit list of the LAME delay + 529: equal to the file decode's LAME-trimmed output.
//! * **FLAC** — Chromium's `sfx.flac` split into frames, muxed with `dfLa` / `fLaC`: exact.

use std::path::{Path, PathBuf};
use std::process::Command;

use audio_core::md5::Md5;
use gneiss_pal::dsp::audio_track::{AudioTrackDecoder, Timing, audio_decoder_for};
use gneiss_pal::dsp::demux::build::{self, MediaTrack, MkvOptions, Mp4Options, SampleSpec, TrackSpec};
use gneiss_pal::dsp::demux::{Demuxer, TrackKind};

fn core_data(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../unaos/libs/media/audio_core/tests/data").join(sub)
}

/// Fetch a listed vector into `target/audiotrack-vectors` (sha256-checked); None offline.
fn vector(name: &str) -> Option<Vec<u8>> {
    let list = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/audiotrack-vectors.txt")).unwrap();
    let line = list.lines().find(|l| l.split_whitespace().next() == Some(name)).expect("listed");
    let mut it = line.split_whitespace().skip(1);
    let (sha, url) = (it.next().unwrap(), it.next().unwrap());
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/audiotrack-vectors");
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join(name);
    if !p.exists() {
        let tmp = p.with_extension("part");
        let ok = Command::new("curl").args(["-sSfL", "-m", "120", "-o"]).arg(&tmp).arg(url).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            eprintln!("SKIP (offline?) {url}");
            return None;
        }
        std::fs::rename(&tmp, &p).ok()?;
    }
    let o = Command::new("sha256sum").arg(&p).output().ok()?;
    let got = String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap_or("").to_string();
    assert_eq!(got, sha, "sha256 of {name}");
    std::fs::read(&p).ok()
}

fn audio_spec(fourcc: &[u8; 4], config_box: Option<[u8; 4]>, codec_id: &'static str, config: Vec<u8>, rate: u32, channels: u16) -> TrackSpec {
    TrackSpec {
        id: 1,
        kind: TrackKind::Audio,
        fourcc: *fourcc,
        config_box,
        codec_id,
        config,
        timescale: rate,
        width: 0,
        height: 0,
        sample_rate: rate,
        channels,
        bit_depth: 16,
        default_duration_ns: 0,
    }
}

/// Packets with their durations (ticks = samples) laid end to end from 0.
fn samples(pkts: &[(Vec<u8>, u32)]) -> Vec<SampleSpec> {
    let mut t = 0i64;
    pkts.iter()
        .map(|(d, dur)| {
            let s = SampleSpec { data: d.clone(), dts: t, pts: t, duration: *dur, keyframe: true };
            t += *dur as i64;
            s
        })
        .collect()
}

/// Demux `file` and decode its audio track through the registry; (rate, channels, samples).
fn decode(file: Vec<u8>, timing: Option<Timing>) -> (u32, u16, Vec<f32>) {
    let mut d = Demuxer::open(file).unwrap();
    let tr = d.audio_track().unwrap().clone();
    let t = timing.unwrap_or_else(|| Timing::of(&d, &tr));
    let mut dec: Box<dyn AudioTrackDecoder> = audio_decoder_for(&tr, t).unwrap();
    let (mut rate, mut ch, mut out) = (0, 0, Vec::new());
    let mut next_pts: Option<i64> = None;
    while let Some(p) = d.next_packet() {
        let b = dec.decode(&p).unwrap();
        if b.samples.is_empty() {
            continue;
        }
        // blocks are contiguous on the presentation timeline (within 1 µs of rounding)
        if let Some(n) = next_pts {
            assert!((b.pts_ns - n).abs() <= 1_000, "gap: block at {} expected {}", b.pts_ns, n);
        }
        next_pts = Some(b.pts_ns + (b.frames() as i64 * 1_000_000_000 / b.sample_rate as i64));
        rate = b.sample_rate;
        ch = b.channels;
        out.extend(b.samples);
    }
    (rate, ch, out)
}

fn md5_i16(x: &[f32]) -> String {
    let bytes: Vec<u8> = x.iter().flat_map(|&s| ((s * 32768.0) as i16).to_le_bytes()).collect();
    let mut m = Md5::new();
    m.update(&bytes);
    m.finish().iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------------------------------------- Opus

const PRE_SKIP: u16 = 312;

fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut h = b"OpusHead".to_vec();
    h.push(1);
    h.push(2);
    h.extend_from_slice(&pre_skip.to_le_bytes());
    h.extend_from_slice(&48_000u32.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes());
    h.push(0);
    h
}
fn dops(pre_skip: u16) -> Vec<u8> {
    let mut h = vec![0u8, 2];
    h.extend_from_slice(&pre_skip.to_be_bytes());
    h.extend_from_slice(&48_000u32.to_be_bytes());
    h.extend_from_slice(&0i16.to_be_bytes());
    h.push(0);
    h
}

fn opus_packets(bits: &[u8]) -> Vec<(Vec<u8>, u32)> {
    use audio_core::opus::decoder::{packet_samples_per_frame, parse_packet};
    let (mut out, mut p, mut last) = (Vec::new(), 0usize, 960u32);
    while p + 8 <= bits.len() {
        let len = u32::from_be_bytes(bits[p..p + 4].try_into().unwrap()) as usize;
        p += 8;
        let pk = bits[p..p + len].to_vec();
        p += len;
        if len > 0 {
            if let Ok((toc, _, sizes)) = parse_packet(&pk) {
                last = packet_samples_per_frame(toc, 48_000) as u32 * sizes.len() as u32;
            }
        }
        out.push((pk, last));
    }
    out
}

#[test]
fn opus_through_both_containers_is_libopus_exact_and_pre_skip_trimmed() {
    let expected = std::fs::read_to_string(core_data("opus/expected.txt")).unwrap();
    let mut n = 0;
    for line in expected.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (name, md5_ref) = (f[0], f[1]);
        let pk = opus_packets(&std::fs::read(core_data(&format!("opus/{name}.bit"))).unwrap());
        let s = samples(&pk);
        let mkv = build::mkv(&[MediaTrack { spec: audio_spec(b"Opus", Some(*b"dOps"), "A_OPUS", opus_head(PRE_SKIP), 48_000, 2), samples: s.clone() }], &MkvOptions::default());
        let mp4_spec = audio_spec(b"Opus", Some(*b"dOps"), "A_OPUS", dops(PRE_SKIP), 48_000, 2);
        let mp4 = build::mp4(&[MediaTrack { spec: mp4_spec.clone(), samples: s.clone() }], &Mp4Options::default());
        let mp4_elst = build::mp4(&[MediaTrack { spec: mp4_spec, samples: s }], &Mp4Options { edits: vec![(1, 0, PRE_SKIP as i64)], ..Default::default() });
        let (_, _, full) = decode(mkv.clone(), Some(Timing::default()));
        assert_eq!(md5_i16(&full), md5_ref, "{name}: untrimmed Matroska decode != libopus");
        let (_, _, full4) = decode(mp4.clone(), Some(Timing::default()));
        assert_eq!(md5_i16(&full4), md5_ref, "{name}: untrimmed MP4 decode != libopus");
        let want = &full[PRE_SKIP as usize * 2..];
        for (label, file) in [("mkv OpusHead", mkv), ("mp4 dOps", mp4), ("mp4 elst", mp4_elst)] {
            let (rate, ch, got) = decode(file, None);
            assert_eq!((rate, ch), (48_000, 2));
            assert_eq!(got.len(), want.len(), "{name} {label}: frames");
            assert!(got == want, "{name} {label}: trimmed PCM differs");
        }
        n += 1;
    }
    assert_eq!(n, 12);
}

// ----------------------------------------------------------------------------------- Vorbis

fn ogg_packets(file: &[u8]) -> Vec<Vec<u8>> {
    let mut r = audio_core::ogg::OggReader::new(audio_core::ByteStream::new(Box::new(audio_core::VecReader::new(file.to_vec()))));
    let mut v = Vec::new();
    while let Ok(Some(p)) = r.next_packet() {
        v.push(p.data);
    }
    v
}

fn xiph_lace(h: &[Vec<u8>]) -> Vec<u8> {
    let mut c = vec![2u8];
    for x in &h[..2] {
        let mut n = x.len();
        while n >= 255 {
            c.push(255);
            n -= 255;
        }
        c.push(n as u8);
    }
    for x in h {
        c.extend_from_slice(x);
    }
    c
}

#[test]
fn vorbis_in_webm_equals_the_ogg_decode() {
    let mut n = 0;
    for i in 1..=10 {
        let ogg = std::fs::read(core_data(&format!("vorbis/v{i:02}.ogg"))).unwrap();
        let pk = ogg_packets(&ogg);
        let (ident, setup) = (&pk[0], &pk[2]);
        let (ch, rate, _) = audio_core::vorbis::Setup::parse_ident(ident).unwrap();
        // packet durations per FFmpeg/WebM convention: what each packet's decode completes
        let mut probe = audio_core::vorbis::VorbisDecoder::new(audio_core::vorbis::Setup::parse(ident, setup).unwrap());
        let mut planes = Vec::new();
        let audio: Vec<(Vec<u8>, u32)> = pk[3..].iter().map(|p| (p.clone(), probe.decode(p, &mut planes).unwrap_or(0) as u32)).collect();
        let spec = audio_spec(b"vorb", None, "A_VORBIS", xiph_lace(&pk[..3]), rate, ch as u16);
        let webm = build::mkv(&[MediaTrack { spec, samples: samples(&audio) }], &MkvOptions::default());
        let (_, got_ch, got) = decode(webm, None);
        let (info, want) = audio_core::decode_all(&ogg).unwrap();
        assert_eq!(got_ch, info.channels);
        assert!(got.len() >= want.len() && got.len() - want.len() < 8192 * ch, "v{i:02}: {} vs {}", got.len(), want.len());
        assert!(got[..want.len()] == want[..], "v{i:02}: samples differ from the Ogg decode");
        n += 1;
    }
    assert_eq!(n, 10);
}

// -------------------------------------------------------------------------------------- AAC

#[test]
fn aac_mp4_gapless_equals_the_file_decode() {
    for name in ["m01.m4a", "m04.m4a", "m07.m4a", "m10.m4a"] {
        let file = std::fs::read(core_data(&format!("aac/{name}"))).unwrap();
        let (info, want) = audio_core::decode_all(&file).unwrap();
        let (rate, ch, got) = decode(file, None);
        assert_eq!((rate, ch), (info.rate, info.channels), "{name}");
        assert_eq!(got.len(), want.len(), "{name}: frame count (gapless start and end)");
        assert!(got == want, "{name}: samples differ");
    }
}

// -------------------------------------------------------------------------------------- MP3

#[test]
fn mp3_in_mp4_with_the_lame_edit_equals_the_file_decode() {
    let Some(mp3) = vector("sfx.mp3") else { return };
    let mut p = 0usize;
    if &mp3[..3] == b"ID3" {
        p = 10 + (((mp3[6] as usize) << 21) | ((mp3[7] as usize) << 14) | ((mp3[8] as usize) << 7) | mp3[9] as usize);
    }
    let mut frames: Vec<(Vec<u8>, u32)> = Vec::new();
    let mut skip = 0i64;
    let (mut rate, mut ch) = (0, 0);
    while p + 4 <= mp3.len() {
        let Some(h) = audio_core::mp3::Header::parse(u32::from_be_bytes(mp3[p..p + 4].try_into().unwrap())) else { break };
        let len = h.frame_len();
        if len == 0 || p + len > mp3.len() {
            break;
        }
        let f = &mp3[p..p + len];
        let off = 4 + if h.crc { 2 } else { 0 } + h.side_len();
        if frames.is_empty() && skip == 0 && (&f[off..off + 4] == b"Xing" || &f[off..off + 4] == b"Info") {
            // the LAME tag: delay (12 bits) at +21 after the Xing fields (flags 0xF: 4+4+100+4)
            let flags = u32::from_be_bytes(f[off + 4..off + 8].try_into().unwrap());
            let q = off + 8 + [(1, 4), (2, 4), (4, 100), (8, 4)].iter().filter(|(b, _)| flags & b != 0).map(|(_, n)| n).sum::<usize>();
            let d = &f[q + 21..q + 24];
            skip = (((d[0] as i64) << 4) | (d[1] as i64 >> 4)) + 529;
        } else {
            rate = h.rate();
            ch = h.channels() as u16;
            frames.push((f.to_vec(), h.samples() as u32));
        }
        p += len;
    }
    assert!(skip > 529 && !frames.is_empty());
    let spec = audio_spec(b".mp3", None, "A_MPEG/L3", Vec::new(), rate, ch);
    let mp4 = build::mp4(&[MediaTrack { spec: spec.clone(), samples: samples(&frames) }], &Mp4Options { edits: vec![(1, 0, skip)], ..Default::default() });
    let mkv = build::mkv(&[MediaTrack { spec, samples: samples(&frames) }], &MkvOptions::default());
    let (_, want) = audio_core::decode_all(&mp3).unwrap();
    let (r, c, got) = decode(mp4, None);
    assert_eq!((r, c), (rate, ch));
    // the file path also trims the LAME end padding, which an edit list without a duration cannot
    assert!(got.len() >= want.len() && got.len() - want.len() <= 1152 * 2 * ch as usize, "{} vs {}", got.len(), want.len());
    assert!(got[..want.len()] == want[..], "MP3 in MP4 differs from the file decode");
    // Matroska carries no edit: the untrimmed stream, the file decode at offset `skip`
    let (_, _, raw) = decode(mkv, None);
    let s = skip as usize * ch as usize;
    assert!(raw[s..s + want.len()] == want[..], "MP3 in Matroska differs from the file decode at the LAME offset");
}

// ------------------------------------------------------------------------------------- FLAC

#[test]
fn flac_in_mp4_and_matroska_is_exact() {
    let Some(flac) = vector("sfx.flac") else { return };
    assert_eq!(&flac[..4], b"fLaC");
    let mut p = 4usize;
    let mut streaminfo = Vec::new();
    loop {
        let last = flac[p] & 0x80 != 0;
        let len = ((flac[p + 1] as usize) << 16) | ((flac[p + 2] as usize) << 8) | flac[p + 3] as usize;
        if flac[p] & 0x7F == 0 {
            streaminfo = flac[p..p + 4 + len].to_vec();
            streaminfo[0] |= 0x80; // the only block we carry: mark it last
        }
        p += 4 + len;
        if last {
            break;
        }
    }
    let si = audio_core::flac::StreamInfo::parse(&streaminfo[4..]).unwrap();
    let mut fd = audio_core::flac::FrameDecoder::new(si);
    let mut pcm = audio_core::Pcm::default();
    let mut frames = Vec::new();
    while p < flac.len() {
        let Ok(used) = fd.decode(&flac[p..], &mut pcm) else { break };
        frames.push((flac[p..p + used].to_vec(), pcm.frames as u32));
        p += used;
    }
    let mut dfla = vec![0u8; 4];
    dfla.extend_from_slice(&streaminfo);
    let mut cp = b"fLaC".to_vec();
    cp.extend_from_slice(&streaminfo);
    let mp4 = build::mp4(&[MediaTrack { spec: audio_spec(b"fLaC", Some(*b"dfLa"), "A_FLAC", dfla, si.rate, si.channels as u16), samples: samples(&frames) }], &Mp4Options::default());
    let mkv = build::mkv(&[MediaTrack { spec: audio_spec(b"fLaC", Some(*b"dfLa"), "A_FLAC", cp, si.rate, si.channels as u16), samples: samples(&frames) }], &MkvOptions::default());
    let (_, want) = audio_core::decode_all(&flac).unwrap();
    for (label, file) in [("mp4", mp4), ("mkv", mkv)] {
        let (r, c, got) = decode(file, None);
        assert_eq!((r, c as u32), (si.rate, si.channels));
        assert_eq!(got.len(), want.len(), "{label}");
        assert!(got == want, "FLAC in {label} not exact");
    }
}
