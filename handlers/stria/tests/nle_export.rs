// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE5 (SR204): the editor export. Over NLE1's two synthetic sources (utp1 + PCM16 MP4) the exported PNG sequence
//! decodes back to the render's frames (counters equal the schedule's), the raw stream is the render's bytes, the WAV
//! and the PCM16 MP4 carry the render's samples; the WAV header and the f32 -> i16 quantiser are known answers.
//! NLE5B (SR206): the H.264 export's movie decodes, sample by sample, to the encoder's reconstruction.

use std::path::PathBuf;

use gneiss_pal::dsp::demux::{Codec, Demuxer, TrackKind};
use gneiss_pal::dsp::image::decode_png;
use gneiss_pal::dsp::video::{Pixels, TestPattern, decoder_for};
use h264enc_core::yuv::I420;
use h264enc_core::{Config, Encoder, RateControl, RcMode};
use stria::nle::{RenderSink, SYNTH_RATE, Sources, render, synth_sample, synth_source};
use stria::nle_export::{AudioOut, H264Encoder, PngSequence, RawSequence, export, pcm16, wav_header};
use timeline_core::schedule::schedule;
use timeline_core::{Clip, Kind, SourceId, Timeline};

const BASE: [u32; 2] = [0, 1000];
const PER: usize = (SYNTH_RATE / 25) as usize; // samples per tick
const TICKS: i64 = 35; // H264ENCCOLOURHOST (SR377): HOST_VUI, the export's VUI colour, the fresh encoders' too (test tail)

/// NLE1's timeline: A[13,23)@0, B[0,10)@10 split at 15, a gap 20..25, A[23,33)@25 (video and audio alike).
fn timeline() -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    for t in 0..2 {
        tl.tracks[t].clips = vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
    }
    tl.split(0, 15).unwrap().split(1, 15).unwrap()
}

fn sources(tl: &Timeline) -> Sources {
    let mut src = Sources::new(tl);
    for (i, b) in BASE.iter().enumerate() {
        src.add(SourceId(i as u32), synth_source(*b, 50, 25)).unwrap();
    }
    src
}

#[derive(Default)]
struct Collect {
    frames: Vec<Vec<u8>>,
    pcm: Vec<f32>,
    fnv: u64,
}
impl RenderSink for Collect {
    fn frame(&mut self, _out_t: i64, _w: u32, _h: u32, rgba: &[u8]) {
        for &b in rgba {
            self.fnv = (self.fnv ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.frames.push(rgba.to_vec());
    }
    fn pcm(&mut self, _out_t: i64, _rate: u32, _ch: u16, s: &[f32]) {
        self.pcm.extend_from_slice(s);
    }
}

/// The reference: the plain render of the same range.
fn reference(tl: &Timeline, src: &Sources) -> Collect {
    let mut c = Collect { fnv: 0xcbf2_9ce4_8422_2325, ..Collect::default() };
    render(tl, 0..TICKS, src, &mut c).unwrap();
    c
}

fn fresh(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("nle_export").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The schedule's synthetic sample for every output sample.
fn want_pcm(tl: &Timeline, src: &Sources) -> Vec<i16> {
    let mut want = vec![0i16; TICKS as usize * PER];
    for st in schedule(tl, 0..TICKS, src).unwrap().iter().filter(|s| s.kind == Kind::Audio) {
        if let Some(s) = st.src {
            for j in 0..st.len as usize * PER {
                want[st.out_t as usize * PER + j] = synth_sample(BASE[s.0 as usize], st.src_t as u64 * PER as u64 + j as u64);
            }
        }
    }
    want
}

#[test]
fn wav_header_kat() {
    #[rustfmt::skip]
    let want: [u8; 44] = [
        b'R', b'I', b'F', b'F', 0xC4, 0x0F, 0x00, 0x00, b'W', b'A', b'V', b'E',
        b'f', b'm', b't', b' ', 0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x00,
        0x44, 0xAC, 0x00, 0x00, 0x10, 0xB1, 0x02, 0x00, 0x04, 0x00, 0x10, 0x00,
        b'd', b'a', b't', b'a', 0xA0, 0x0F, 0x00, 0x00,
    ];
    assert_eq!(wav_header(44_100, 2, 1000), want);
}

#[test]
fn pcm16_kat() {
    for s in i16::MIN..=i16::MAX {
        assert_eq!(pcm16(s as f32 / 32768.0), s);
    }
    assert_eq!((pcm16(1.5), pcm16(-2.0), pcm16(1.0), pcm16(0.0)), (32767, -32768, 32767, 0));
}

#[test]
fn png_sequence_round_trip() {
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let dir = fresh("png");
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(PngSequence), AudioOut::None).unwrap();
    assert_eq!((r.frames, r.files, r.pcm_frames), (35, 35, TICKS as u64 * PER as u64));
    assert_eq!(r.fnv, refr.fnv);
    let steps = schedule(&tl, 0..TICKS, &src).unwrap();
    let vs: Vec<_> = steps.iter().filter(|s| s.kind == Kind::Video).collect();
    let mut bytes = 0u64;
    for (i, st) in vs.iter().enumerate() {
        let file = std::fs::read(dir.join(format!("{i:06}.png"))).unwrap();
        bytes += file.len() as u64;
        let img = decode_png(&file).unwrap();
        assert_eq!((img.width, img.height), (64, 48));
        assert_eq!(img.rgba, refr.frames[i], "frame {i} differs from the render");
        let got = TestPattern::read_counter(&img.rgba, 64, 48);
        match st.src {
            Some(s) => assert_eq!(got, Some(BASE[s.0 as usize] + st.src_t as u32), "frame {i}"),
            None => assert!(img.rgba.chunks(4).all(|p| p == [0, 0, 0, 255]), "frame {i} not black"),
        }
    }
    assert_eq!(r.bytes, bytes);
    assert!(!dir.join(format!("{:06}.png", vs.len())).exists());
}

#[test]
fn raw_sequence_and_wav() {
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let dir = fresh("raw");
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(RawSequence), AudioOut::Wav).unwrap();
    assert_eq!((r.frames, r.files, r.pcm_frames), (35, 3, 35 * PER as u64));
    assert_eq!(r.fnv, refr.fnv);
    let raw = std::fs::read(dir.join("video.rgba")).unwrap();
    assert_eq!(raw, refr.frames.concat());
    let json = std::fs::read_to_string(dir.join("video.json")).unwrap();
    assert_eq!(json, "{\"format\":\"rgba\",\"fourcc\":\"raw \",\"w\":64,\"h\":48,\"fps_num\":25,\"fps_den\":1,\"frames\":35}\n");
    let wav = std::fs::read(dir.join("audio.wav")).unwrap();
    assert_eq!(wav[..44], wav_header(SYNTH_RATE, 1, 35 * PER as u64));
    let got: Vec<i16> = wav[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    let quant: Vec<i16> = refr.pcm.iter().map(|&x| pcm16(x)).collect();
    assert_eq!(got, quant);
    assert_eq!(got, want_pcm(&tl, &src));
    assert_eq!(r.bytes, (raw.len() + json.len() + wav.len()) as u64);
}

#[test]
fn mp4_audio_round_trip() {
    let tl = timeline();
    let src = sources(&tl);
    let dir = fresh("mp4");
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(RawSequence), AudioOut::Mp4).unwrap();
    assert_eq!((r.files, r.pcm_frames), (3, 35 * PER as u64));
    let mp4 = std::fs::read(dir.join("audio.mp4")).unwrap();
    let d = Demuxer::open(mp4).unwrap();
    assert_eq!(d.tracks().len(), 1);
    let t = &d.tracks()[0];
    assert_eq!((t.kind, t.sample_rate, t.channels), (TrackKind::Audio, SYNTH_RATE, 1));
    assert_eq!(t.codec, Codec::Pcm { bits: 16, float: false, big_endian: false });
    let data: Vec<u8> = d.track_samples(0).flat_map(|s| d.packet_at(s).data).collect();
    let got: Vec<i16> = data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    assert_eq!(got, want_pcm(&tl, &src));
}

/// NLE6EXPORTAAC (rmbp-ledger B960): `AudioOut::Aac` writes `audio.mp4` with one `mp4a` AAC-LC track: the ASC is LC
/// mono at 48 kHz, the edit list skips the encoder's 1024-sample priming, every sample decodes through audio_core's
/// decoder (`gneiss_pal::dsp::audio::aac`), and the decoded sound after the priming tracks the render's samples (the
/// synthetic sawtooth at 128 kb/s: 34.2 dB measured, 67 units, 7325 bytes; the floor 30 dB).
#[test]
fn aac_audio_round_trip() {
    use gneiss_pal::dsp::audio::aac::{AacDecoder, Asc};
    use gneiss_pal::dsp::audio::bits::BitReader;
    const SNR_FLOOR: f64 = 30.0;
    let tl = timeline();
    let src = sources(&tl);
    let dir = fresh("aac");
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(RawSequence), AudioOut::Aac(128)).unwrap();
    assert_eq!((r.files, r.pcm_frames), (3, 35 * PER as u64));
    let mp4 = std::fs::read(dir.join("audio.mp4")).unwrap();
    assert_eq!(r.bytes, std::fs::metadata(dir.join("video.rgba")).unwrap().len() + std::fs::metadata(dir.join("video.json")).unwrap().len() + mp4.len() as u64);
    let d = Demuxer::open(mp4).unwrap();
    assert_eq!(d.tracks().len(), 1);
    let t = &d.tracks()[0];
    assert_eq!((t.kind, t.codec.clone(), t.sample_rate, t.channels), (TrackKind::Audio, Codec::Aac, SYNTH_RATE, 1));
    assert_eq!(t.config, vec![0x11, 0x88]);
    assert_eq!(t.edit.map(|e| e.media_time), Some(1024));
    let asc = Asc::parse(&t.config).unwrap();
    let mut dec = AacDecoder::new(asc.sf_index, asc.layout().unwrap()).unwrap();
    let want = want_pcm(&tl, &src);
    let units: Vec<Vec<u8>> = d.track_samples(0).map(|s| d.packet_at(s).data).collect();
    assert_eq!(units.len(), want.len().div_ceil(1024) + 1);
    let mut got = Vec::new();
    for u in &units {
        dec.decode_block(&mut BitReader::new(u), 0).unwrap();
        got.extend(dec.out[0].iter().map(|&v| v as f64 * 32768.0));
    }
    let (mut s, mut n) = (0f64, 0f64);
    for (i, &w) in want.iter().enumerate() {
        let e = w as f64 - got[i + 1024];
        s += w as f64 * w as f64;
        n += e * e;
    }
    let snr = 10.0 * (s / n.max(1e-9)).log10();
    println!("aac_audio_round_trip: units={} bytes={} snr={snr:.1} dB", units.len(), units.iter().map(|u| u.len()).sum::<usize>());
    assert!(snr >= SNR_FLOOR, "SNR {snr:.1} dB");
}

/// NLE5B: a fresh H264ENC encoder over the render's frames (the same converter and config): its samples, its avcC and
/// its reconstruction per frame.
fn fresh_h264(frames: &[Vec<u8>], gop: u32) -> (Vec<Vec<u8>>, Vec<u8>, Vec<[Vec<u8>; 3]>, Vec<I420>) {
    let mut enc = Encoder::new(Config { width: 64, height: 48, qp: 26, gop, colour: HOST_VUI, ..Default::default() }).unwrap();
    let (mut samples, mut recon, mut input) = (Vec::new(), Vec::new(), Vec::new());
    for f in frames {
        let yuv = I420::from_rgba(64, 48, f).unwrap();
        samples.push(enc.encode(&yuv.as_yuv420()).unwrap().avcc_sample());
        let r = enc.reconstruction();
        recon.push([r[0].to_vec(), r[1].to_vec(), r[2].to_vec()]);
        input.push(yuv);
    }
    (samples, enc.avcc(), recon, input)
}

fn psnr(a: &[&[u8]], b: &[&[u8]]) -> f64 {
    let (mut se, mut n) = (0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.len(), y.len());
        for (&p, &q) in x.iter().zip(y.iter()) {
            se += (p as f64 - q as f64).powi(2);
            n += 1.0;
        }
    }
    if se == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / (se / n)).log10() }
}

#[test]
fn h264_movie_round_trip() {
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let dir = fresh("h264");
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(H264Encoder::new(26, 10)), AudioOut::Mp4).unwrap();
    assert_eq!((r.frames, r.files, r.pcm_frames), (35, 1, 35 * PER as u64));
    assert_eq!(r.fnv, refr.fnv);
    let mp4 = std::fs::read(dir.join("movie.mp4")).unwrap();
    assert_eq!(r.bytes, mp4.len() as u64);
    assert!(!dir.join("audio.mp4").exists() && !dir.join("video.mp4").exists());
    let (want, avcc, recon, input) = fresh_h264(&refr.frames, 10);

    let d = Demuxer::open(mp4).unwrap();
    assert_eq!(d.tracks().len(), 2);
    let vt = &d.tracks()[0];
    assert_eq!((vt.kind, vt.codec.clone(), vt.width, vt.height), (TrackKind::Video, Codec::Avc, 64, 48));
    assert_eq!(vt.config, avcc, "the avcC box is the encoder's record");
    assert_eq!(vt.sample_count, 35);
    let mut dec = decoder_for(vt).unwrap();
    let mut frames = Vec::new();
    for (i, s) in d.track_samples(0).copied().collect::<Vec<_>>().iter().enumerate() {
        let pk = d.packet_at(s);
        assert_eq!(pk.keyframe, i % 10 == 0, "sample {i} sync flag");
        assert_eq!(pk.pts, i as i64, "sample {i} pts at the project fps");
        assert_eq!(pk.data, want[i], "sample {i} is the encoder's access unit");
        frames.extend(dec.decode(&pk).unwrap());
    }
    frames.extend(dec.flush());
    assert_eq!(frames.len(), 35);
    for (i, f) in frames.iter().enumerate() {
        let Pixels::I420 { y, u, v, .. } = &f.pixels else { panic!("frame {i} not I420") };
        assert_eq!((f.width, f.height), (64, 48));
        assert_eq!([y.clone(), u.clone(), v.clone()], recon[i], "frame {i}: decode differs from the reconstruction");
        let db = psnr(&[y, u, v], &[&input[i].y, &input[i].u, &input[i].v]);
        assert!(db >= 35.0, "frame {i}: PSNR {db:.2} dB below 35");
    }

    let at = &d.tracks()[1];
    assert_eq!((at.kind, at.sample_rate, at.channels), (TrackKind::Audio, SYNTH_RATE, 1));
    assert_eq!(at.codec, Codec::Pcm { bits: 16, float: false, big_endian: false });
    let data: Vec<u8> = d.track_samples(1).flat_map(|s| d.packet_at(s).data).collect();
    let got: Vec<i16> = data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    assert_eq!(got.len(), 35 * PER);
    assert_eq!(got, want_pcm(&tl, &src));
}

#[test]
fn h264_video_plus_wav() {
    let tl = timeline();
    let src = sources(&tl);
    let dir = fresh("h264wav");
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(H264Encoder::new(26, 25)), AudioOut::Wav).unwrap();
    assert_eq!((r.frames, r.files), (35, 2));
    let mp4 = std::fs::read(dir.join("video.mp4")).unwrap();
    let wav = std::fs::read(dir.join("audio.wav")).unwrap();
    assert_eq!(r.bytes, (mp4.len() + wav.len()) as u64);
    let d = Demuxer::open(mp4).unwrap();
    assert_eq!(d.tracks().len(), 1);
    assert_eq!((d.tracks()[0].codec.clone(), d.tracks()[0].sample_count), (Codec::Avc, 35));
    let got: Vec<i16> = wav[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    assert_eq!(got, want_pcm(&tl, &src));
}

/// H264ENCRC (SR218): `H264Encoder::with_bitrate` (`nle export --bitrate`) hands h264enc_core a CBR rate control at
/// the timeline's fps: the movie's samples are a fresh encoder's under the same `RateControl`, and every frame decodes.
#[test]
fn h264_bitrate_export_is_the_rate_controlled_encoder() {
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let dir = fresh("h264rate");
    let enc = H264Encoder::new(26, 10).with_bitrate(40, tl.tb.den as u32, tl.tb.num as u32);
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(enc), AudioOut::None).unwrap();
    assert_eq!(r.frames, 35);
    let rate = RateControl { bits_per_second: 40_000, fps_num: 25, fps_den: 1, mode: RcMode::Cbr };
    let mut fresh = Encoder::new(Config { width: 64, height: 48, qp: 26, gop: 10, rate: Some(rate), ..Default::default() }).unwrap();
    let want: Vec<Vec<u8>> = refr.frames.iter().map(|f| fresh.encode(&I420::from_rgba(64, 48, f).unwrap().as_yuv420()).unwrap().avcc_sample()).collect();
    let d = Demuxer::open(std::fs::read(dir.join("video.mp4")).unwrap()).unwrap();
    let mut dec = decoder_for(&d.tracks()[0]).unwrap();
    let mut frames = Vec::new();
    for (i, s) in d.track_samples(0).copied().collect::<Vec<_>>().iter().enumerate() {
        let pk = d.packet_at(s);
        assert_eq!(pk.data, want[i], "sample {i} is the rate-controlled encoder's access unit");
        frames.extend(dec.decode(&pk).unwrap());
    }
    frames.extend(dec.flush());
    assert_eq!(frames.len(), 35);
    let bytes: usize = want.iter().map(|s| s.len()).sum();
    println!("h264 --bitrate 40 export: {bytes} bytes over 35 frames at 25 fps = {:.1} kbit/s", bytes as f64 * 8.0 * 25.0 / 35.0 / 1000.0);
}

/// H264ENCVBRCRF (SR383): `H264Encoder::with_quality` (`nle export --crf`) hands h264enc_core a constant-quality rate control:
/// the movie's samples are a fresh encoder's under the hand-written `RcMode::Crf` literal, and every frame decodes.
#[test]
fn h264_quality_export_is_the_crf_encoder() {
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let dir = fresh("h264crf");
    let enc = H264Encoder::new(26, 10).with_quality(24, tl.tb.den as u32, tl.tb.num as u32);
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(enc), AudioOut::None).unwrap();
    assert_eq!(r.frames, 35);
    let rate = RateControl { bits_per_second: 0, fps_num: 25, fps_den: 1, mode: RcMode::Crf(24) };
    let mut fresh = Encoder::new(Config { width: 64, height: 48, qp: 26, gop: 10, rate: Some(rate), ..Default::default() }).unwrap();
    let want: Vec<Vec<u8>> = refr.frames.iter().map(|f| fresh.encode(&I420::from_rgba(64, 48, f).unwrap().as_yuv420()).unwrap().avcc_sample()).collect();
    let d = Demuxer::open(std::fs::read(dir.join("video.mp4")).unwrap()).unwrap();
    let mut dec = decoder_for(&d.tracks()[0]).unwrap();
    let mut frames = Vec::new();
    for (i, s) in d.track_samples(0).copied().collect::<Vec<_>>().iter().enumerate() {
        let pk = d.packet_at(s);
        assert_eq!(pk.data, want[i], "sample {i} is the crf encoder's access unit");
        frames.extend(dec.decode(&pk).unwrap());
    }
    frames.extend(dec.flush());
    assert_eq!(frames.len(), 35);
    let bytes: usize = want.iter().map(|s| s.len()).sum();
    println!("h264 --crf 24 export: {bytes} bytes over 35 frames");
}

/// H264ENCBFRAMES (SR333): `H264Encoder::with_bframes(2)` exports B pictures. The movie's samples are a fresh encoder's
/// `push` / `flush` units in decoding order; demux_core reads back the stored dts (strictly increasing) and the `ctts`
/// composition offsets (pts = display + 1, dts <= pts); the avcC's VUI gives the Player's reorder depth 1; the Player's
/// decoder (`decoder_for`) fed in decoding order outputs every frame in display order equal to the reconstruction.
#[test]
fn h264_bframes_export_writes_composition_offsets() {
    use gneiss_pal::dsp::video::reorder_depth;
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let dir = fresh("h264bframes");
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(H264Encoder::new(26, 10).with_bframes(2)), AudioOut::Mp4).unwrap();
    assert_eq!(r.frames, 35);
    let mut enc = Encoder::new(Config { width: 64, height: 48, qp: 26, gop: 10, bframes: 2, colour: HOST_VUI, ..Default::default() }).unwrap();
    let (mut want, mut recon) = (Vec::new(), Vec::new());
    for f in &refr.frames {
        want.extend(enc.push(&I420::from_rgba(64, 48, f).unwrap().as_yuv420()).unwrap());
        recon.extend(enc.reconstructions().iter().cloned());
    }
    want.extend(enc.flush().unwrap());
    recon.extend(enc.reconstructions().iter().cloned());
    assert_eq!((want.len(), recon.len()), (35, 35));

    let d = Demuxer::open(std::fs::read(dir.join("movie.mp4")).unwrap()).unwrap();
    let vt = d.tracks()[0].clone();
    assert_eq!((vt.codec.clone(), vt.sample_count), (Codec::Avc, 35));
    assert_eq!(vt.config, enc.avcc());
    assert_eq!(reorder_depth(&vt), Some(1), "the avcC's VUI max_num_reorder_frames");
    let mut dec = decoder_for(&vt).unwrap();
    let (mut frames, mut pts, mut offsets) = (Vec::new(), Vec::new(), Vec::new());
    let mut last_dts = -1;
    for (i, s) in d.track_samples(0).copied().collect::<Vec<_>>().iter().enumerate() {
        let pk = d.packet_at(s);
        assert_eq!(pk.data, want[i].avcc_sample(), "sample {i} is the encoder's access unit");
        assert_eq!((pk.dts, pk.pts), (want[i].dts as i64, want[i].pts as i64), "sample {i} (dts, pts) at the project fps");
        assert!(pk.dts > last_dts && pk.dts <= pk.pts, "sample {i}: dts {} pts {}", pk.dts, pk.pts);
        assert_eq!(pk.keyframe, want[i].keyframe, "sample {i} sync flag");
        last_dts = pk.dts;
        pts.push(pk.pts);
        offsets.push(pk.pts - pk.dts);
        frames.extend(dec.decode(&pk).unwrap());
    }
    frames.extend(dec.flush());
    assert!(offsets.iter().any(|&o| o != offsets[0]), "the ctts carries the reorder");
    pts.sort();
    assert_eq!(pts, (1..=35).collect::<Vec<i64>>(), "every frame presented once, from tick 1");
    assert_eq!(frames.len(), 35);
    let mut order: Vec<usize> = (0..35).collect();
    order.sort_by_key(|&i| want[i].pts);
    for (k, f) in frames.iter().enumerate() {
        let Pixels::I420 { y, u, v, .. } = &f.pixels else { panic!("frame {k} not I420") };
        assert_eq!([y.clone(), u.clone(), v.clone()], recon[order[k]], "frame {k}: decode differs from the reconstruction");
    }
    let bytes: usize = want.iter().map(|a| a.avcc_sample().len()).sum();
    println!("h264 bframes 2 export: {bytes} bytes, ctts offsets {offsets:?}");
}

/// H264ENCPYRAMID (SR349): `H264Encoder::with_bframes(3)` exports h264enc_core's B pyramid. The avcC's VUI gives reorder
/// depth 2, the `ctts` offsets carry the reorder (pts = display + 2, dts <= pts), and the Player's decoder (`decoder_for`)
/// fed in decoding order outputs every frame in display order equal to the reconstruction.
#[test]
fn h264_pyramid_export_reorder_depth_two() {
    use gneiss_pal::dsp::video::reorder_depth;
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let dir = fresh("h264pyramid");
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(H264Encoder::new(26, 10).with_bframes(3)), AudioOut::Mp4).unwrap();
    assert_eq!(r.frames, 35);
    let mut enc = Encoder::new(Config { width: 64, height: 48, qp: 26, gop: 10, bframes: 3, colour: HOST_VUI, ..Default::default() }).unwrap();
    let (mut want, mut recon) = (Vec::new(), Vec::new());
    for f in &refr.frames {
        want.extend(enc.push(&I420::from_rgba(64, 48, f).unwrap().as_yuv420()).unwrap());
        recon.extend(enc.reconstructions().iter().cloned());
    }
    want.extend(enc.flush().unwrap());
    recon.extend(enc.reconstructions().iter().cloned());
    assert_eq!((want.len(), recon.len()), (35, 35));

    let d = Demuxer::open(std::fs::read(dir.join("movie.mp4")).unwrap()).unwrap();
    let vt = d.tracks()[0].clone();
    assert_eq!(vt.config, enc.avcc());
    assert_eq!(reorder_depth(&vt), Some(2), "the avcC's VUI max_num_reorder_frames under the pyramid");
    let mut dec = decoder_for(&vt).unwrap();
    let (mut frames, mut pts, mut offsets) = (Vec::new(), Vec::new(), Vec::new());
    let mut last_dts = -1;
    for (i, s) in d.track_samples(0).copied().collect::<Vec<_>>().iter().enumerate() {
        let pk = d.packet_at(s);
        assert_eq!(pk.data, want[i].avcc_sample(), "sample {i} is the encoder's access unit");
        assert_eq!((pk.dts, pk.pts), (want[i].dts as i64, want[i].pts as i64), "sample {i} (dts, pts)");
        assert!(pk.dts > last_dts && pk.dts <= pk.pts, "sample {i}: dts {} pts {}", pk.dts, pk.pts);
        last_dts = pk.dts;
        pts.push(pk.pts);
        offsets.push(pk.pts - pk.dts);
        frames.extend(dec.decode(&pk).unwrap());
    }
    frames.extend(dec.flush());
    assert!(offsets.iter().any(|&o| o != offsets[0]), "the ctts carries the reorder");
    pts.sort();
    assert_eq!(pts, (2..=36).collect::<Vec<i64>>(), "every frame presented once, from tick 2");
    assert_eq!(frames.len(), 35);
    let mut order: Vec<usize> = (0..35).collect();
    order.sort_by_key(|&i| want[i].pts);
    for (k, f) in frames.iter().enumerate() {
        let Pixels::I420 { y, u, v, .. } = &f.pixels else { panic!("frame {k} not I420") };
        assert_eq!([y.clone(), u.clone(), v.clone()], recon[order[k]], "frame {k}: decode differs from the reconstruction");
    }
    let bytes: usize = want.iter().map(|a| a.avcc_sample().len()).sum();
    println!("h264 pyramid bframes 3 export: {bytes} bytes, ctts offsets {offsets:?}");
}

// ── H264ENCCOLOURHOST (LEDGER SR377): the host export states ONE file colour (handlers.md §5 NLEPROBE-HDR) ──
/// The VUI codes of `nle_export::RGBA_COLOUR`: the fresh encoders above state them as the export does (their avcC pins).
const HOST_VUI: Option<(u8, u8, u8, bool)> = Some((2, 2, 6, false));

/// An H.264 export states its file colour as the avc1's `colr` (`Track::colour` on a plain open) and as the avcC's SPS VUI
/// (the Player's `colour_of`): matrix 6 limited (the RGBA path's BT.601 limited, the kernel export's tag). EXPORTCOLOURPRIM
/// (rmbp-ledger B979): the primaries / transfer are the reels' one pair (`Timeline::reels_colour`, the kernel's rule too):
/// untagged reels 2/2/6, one reel tagged BT.709 (`colr` 1/1/1 limited) 1/1/6, two reels that differ (BT.709 and BT.2020)
/// 2/2/6, since the mix converts no primaries. An audio-only MP4 (no H.264) states none.
#[test]
fn h264enccolourhost_export_states_the_file_colour() {
    use gneiss_pal::dsp::demux::build::Mp4Extras;
    use gneiss_pal::dsp::demux::colour::Colour;
    use gneiss_pal::dsp::video::colour_of;
    use stria::nle::synth_source_with;
    use stria::nle_export::RGBA_COLOUR;
    assert_eq!(RGBA_COLOUR, Colour { primaries: 2, transfer: 2, matrix: 6, full_range: Some(false), mastering: None, light: None });
    let check = |label: &str, mp4: Vec<u8>, want: Colour| {
        let d = Demuxer::open(mp4).unwrap();
        let vt = &d.tracks()[0];
        assert_eq!(vt.codec, Codec::Avc, "{label}");
        assert_eq!(vt.colour, Some(want), "{label}: the colr");
        assert_eq!(colour_of(vt, &[]), Some(want), "{label}: the SPS VUI");
    };
    let tl = timeline();
    let src = sources(&tl);
    let dir = fresh("h264colour");
    export(&tl, 0..TICKS, &src, &dir, Box::new(H264Encoder::new(26, 10)), AudioOut::None).unwrap();
    check("untagged reels", std::fs::read(dir.join("video.mp4")).unwrap(), RGBA_COLOUR);

    let mut one = Timeline::new(25);
    let a = one.add_source("A", 50);
    one.add_track(Kind::Video);
    one.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
    let bt709 = Colour { primaries: 1, transfer: 1, matrix: 1, full_range: Some(false), mastering: None, light: None };
    let mut s1 = Sources::new(&one);
    s1.add(a, synth_source_with(0, 50, 25, &Mp4Extras { colour: vec![(1, bt709)], ..Mp4Extras::default() })).unwrap();
    assert_eq!(s1.open_report(a).and_then(|r| r.colour), Some(bt709), "the reel states BT.709");
    let dir = fresh("h264colour709");
    export(&one, 0..20, &s1, &dir, Box::new(H264Encoder::new(26, 10).with_bframes(2)), AudioOut::None).unwrap();
    check("one BT.709 reel", std::fs::read(dir.join("video.mp4")).unwrap(), Colour { primaries: 1, transfer: 1, ..RGBA_COLOUR });

    let mut two = Timeline::new(25);
    let a = two.add_source("A", 50);
    let b = two.add_source("B", 50);
    two.add_track(Kind::Video);
    two.tracks[0].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10)];
    let bt2020 = Colour { primaries: 9, transfer: 14, matrix: 9, full_range: Some(false), mastering: None, light: None };
    let mut s2 = Sources::new(&two);
    s2.add(a, synth_source_with(0, 50, 25, &Mp4Extras { colour: vec![(1, bt709)], ..Mp4Extras::default() })).unwrap();
    s2.add(b, synth_source_with(0, 50, 25, &Mp4Extras { colour: vec![(1, bt2020)], ..Mp4Extras::default() })).unwrap();
    assert_eq!(s2.open_report(b).and_then(|r| r.colour), Some(bt2020), "the second reel states BT.2020");
    let dir = fresh("h264colourmixed");
    export(&two, 0..20, &s2, &dir, Box::new(H264Encoder::new(26, 10)), AudioOut::None).unwrap();
    check("two differing reels", std::fs::read(dir.join("video.mp4")).unwrap(), RGBA_COLOUR);

    let dir = fresh("rawcolour");
    export(&tl, 0..TICKS, &src, &dir, Box::new(RawSequence), AudioOut::Mp4).unwrap();
    let d = Demuxer::open(std::fs::read(dir.join("audio.mp4")).unwrap()).unwrap();
    assert!(d.tracks().iter().all(|t| t.kind == TrackKind::Audio && t.colour.is_none()), "no video, no colour");
}

/// NLE6EXPORTMKV (rmbp-ledger B966): `export_as(.., Container::Mkv)` writes `movie.mkv` (no `movie.mp4`) through
/// `build::mux`'s Matroska; re-demuxed it is the encoder's stream: Avc with the encoder's avcC as CodecPrivate, 35
/// samples, a keyframe every 10, pts at 40 ms per tick (Matroska stores no dts), each sample the encoder's access unit,
/// and the PCM track the render's samples.
#[test]
fn h264_movie_mkv_round_trip() {
    use stria::nle_export::{Container, export_as};
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let dir = fresh("h264mkv");
    let r = export_as(&tl, 0..TICKS, &src, &dir, Box::new(H264Encoder::new(26, 10)), AudioOut::Mp4, Container::Mkv).unwrap();
    assert_eq!((r.frames, r.files, r.pcm_frames), (35, 1, 35 * PER as u64));
    let mkv = std::fs::read(dir.join("movie.mkv")).unwrap();
    assert_eq!(r.bytes, mkv.len() as u64);
    assert!(!dir.join("movie.mp4").exists());
    let (want, avcc, _, _) = fresh_h264(&refr.frames, 10);

    let d = Demuxer::open(mkv.clone()).unwrap();
    assert_eq!(d.format(), gneiss_pal::dsp::demux::Format::Matroska);
    assert_eq!(d.tracks().len(), 2);
    let vt = &d.tracks()[0];
    assert_eq!((vt.kind, vt.codec.clone(), vt.width, vt.height), (TrackKind::Video, Codec::Avc, 64, 48));
    assert_eq!(vt.config, avcc, "CodecPrivate is the encoder's avcC record");
    let samples: Vec<_> = d.track_samples(0).copied().collect();
    assert_eq!(samples.len(), 35);
    for (i, s) in samples.iter().enumerate() {
        let pk = d.packet_at(s);
        assert_eq!(pk.keyframe, i % 10 == 0, "sample {i} keyframe flag");
        assert_eq!(vt.to_ns(pk.pts), i as i64 * 40_000_000, "sample {i} pts");
        assert_eq!(pk.data, want[i], "sample {i} is the encoder's access unit");
    }
    let at = &d.tracks()[1];
    assert_eq!((at.kind, at.sample_rate, at.channels), (TrackKind::Audio, SYNTH_RATE, 1));
    assert_eq!(at.codec, Codec::Pcm { bits: 16, float: false, big_endian: false });
    let data: Vec<u8> = d.track_samples(1).flat_map(|s| d.packet_at(s).data).collect();
    let got: Vec<i16> = data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    assert_eq!(got, want_pcm(&tl, &src));
    assert!(Demuxer::open_indexed(mkv).unwrap().indexed(), "the Cues are found");
}

// ── NLERAWEXPORT (LEDGER SR689): the raw planar exports, i420 and y4m, over the ONE converter (handlers.md §5 NLE5b's NLE5-I420, NLEPROBE-HDR's RAW-EXPORT) ──

/// The converter's planes of one 64x48 RGBA frame of the synth fixture, Y then Cb then Cr.
fn nlerawexport_planes(rgba: &[u8]) -> Vec<u8> {
    let f = I420::from_rgba(64, 48, rgba).unwrap();
    [f.y, f.u, f.v].concat()
}

#[test]
fn nlerawexport_i420_is_the_converter_planes() {
    use stria::nle_export::I420Raw;
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let dir = fresh("i420");
    let enc = I420Raw::default();
    let size = enc.size();
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(enc), AudioOut::None).unwrap();
    assert_eq!((r.frames, r.files), (35, 2));
    assert_eq!(r.fnv, refr.fnv);
    assert_eq!(size.get(), Some((64, 48)), "the latched size");
    let raw = std::fs::read(dir.join("video.i420")).unwrap();
    assert_eq!(raw.len(), 35 * 64 * 48 * 3 / 2, "frames x w x h x 3 / 2");
    assert_eq!(raw[..64 * 48], I420::from_rgba(64, 48, &refr.frames[0]).unwrap().y[..], "frame 0's Y plane is the converter's");
    let want: Vec<u8> = refr.frames.iter().flat_map(|f| nlerawexport_planes(f)).collect();
    assert!(raw == want, "every frame's Y, Cb, Cr planes are the converter's, bit for bit");
    let json = std::fs::read_to_string(dir.join("video.json")).unwrap();
    assert_eq!(json, "{\"format\":\"i420\",\"fourcc\":\"I420\",\"w\":64,\"h\":48,\"fps_num\":25,\"fps_den\":1,\"frames\":35}\n");
    assert_eq!(r.bytes, (raw.len() + json.len()) as u64);
}

#[test]
fn nlerawexport_y4m_header_frames_planes() {
    use stria::nle_export::{I420Raw, Y4m};
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let dir = fresh("y4m");
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(Y4m::new(tl.tb.den as u32, tl.tb.num as u32)), AudioOut::None).unwrap();
    assert_eq!((r.frames, r.files), (35, 2));
    assert_eq!(r.fnv, refr.fnv);
    let i420_dir = fresh("y4m-i420");
    export(&tl, 0..TICKS, &src, &i420_dir, Box::new(I420Raw::default()), AudioOut::None).unwrap();
    let i420 = std::fs::read(i420_dir.join("video.i420")).unwrap();
    let y4m = std::fs::read(dir.join("video.y4m")).unwrap();
    let head: &[u8] = b"YUV4MPEG2 W64 H48 F25:1 Ip A1:1 C420jpeg\n";
    assert!(y4m.starts_with(head), "{:?}", String::from_utf8_lossy(&y4m[..y4m.len().min(64)]));
    let per = 64 * 48 * 3 / 2;
    assert_eq!(y4m.len(), head.len() + 35 * (6 + per));
    let mut rest = &y4m[head.len()..];
    let mut n = 0;
    while !rest.is_empty() {
        assert!(rest.starts_with(b"FRAME\n"), "frame {n}: the FRAME marker");
        assert!(rest[6..6 + per] == i420[n * per..(n + 1) * per], "frame {n}: the i420 export's planes");
        rest = &rest[6 + per..];
        n += 1;
    }
    assert_eq!(n, 35, "one FRAME per picture");
    let json = std::fs::read_to_string(dir.join("video.json")).unwrap();
    assert_eq!(json, "{\"format\":\"y4m\",\"fourcc\":\"I420\",\"w\":64,\"h\":48,\"fps_num\":25,\"fps_den\":1,\"frames\":35}\n");
}

#[test]
fn nlerawexport_y4m_header_768x512_and_fps_reduced() {
    use stria::nle_export::{VideoEncoder, Y4m};
    let mut enc = Y4m::new(50, 2);
    let black = vec![0u8; 768 * 512 * 4];
    let per = 768 * 512 * 3 / 2;
    let first = enc.encode(768, 512, &black).unwrap();
    assert_eq!(first.len(), 1);
    let head: &[u8] = b"YUV4MPEG2 W768 H512 F25:1 Ip A1:1 C420jpeg\n";
    assert!(first[0].data.starts_with(head), "{:?}", String::from_utf8_lossy(&first[0].data[..64]));
    assert_eq!(&first[0].data[head.len()..head.len() + 6], b"FRAME\n");
    assert_eq!(first[0].data.len(), head.len() + 6 + per);
    let next = enc.encode(768, 512, &black).unwrap();
    assert!(next[0].data.starts_with(b"FRAME\n") && next[0].data.len() == 6 + per, "the header once");
}

#[test]
fn nlerawexport_second_size_refused_by_name() {
    use stria::nle_export::{ExportError, I420Raw, VideoEncoder, Y4m};
    let encs: [Box<dyn VideoEncoder>; 2] = [Box::new(I420Raw::default()), Box::new(Y4m::new(25, 1))];
    for mut enc in encs {
        enc.encode(4, 4, &[0u8; 64]).unwrap();
        let e = enc.encode(6, 6, &[0u8; 144]).unwrap_err();
        assert!(matches!(e, ExportError::H264(h264enc_core::Error::FrameSize)), "{e}");
        assert!(e.to_string().contains("frame does not match"), "{e}");
    }
}

// ---- H264ENCHRDMP4 (LEDGER SR720): the H.264 movie's btrt, read back through demux_core (named tail block) ----
/// ffmpeg movenc's avgBitrate: floor(coded bits x 25 / frames) at the test timeline's 25 fps, over the demuxed samples.
fn h264enchrdmp4_avg(d: &Demuxer) -> (u32, u64) {
    let bytes: u64 = d.track_samples(0).map(|s| d.packet_at(s).data.len() as u64).sum();
    let frames = d.track_samples(0).count() as u64;
    ((bytes * 8 * 25 / frames) as u32, frames)
}

/// H264ENCHRDMP4 (SR720): a CBR 400 kbit/s export with the HRD signalled carries the bucket in the avc1's `btrt`: bufferSizeDB
/// 50000 (the one-second CPB / 8), maxBitrate 400000 (the HRD rate), avgBitrate the floored coded rate, the coded bits within one
/// CpbSize of rate x duration (the HRD's bound: the 1.4 s clip's bucket transient is about half the CPB).
#[test]
fn h264enchrdmp4_cbr_export_carries_the_hrd_btrt() {
    let tl = timeline();
    let src = sources(&tl);
    let dir = fresh("h264enchrdmp4_cbr");
    let enc = H264Encoder::new(26, 10).with_bitrate(400, tl.tb.den as u32, tl.tb.num as u32).with_hrd(true);
    let r = export(&tl, 0..TICKS, &src, &dir, Box::new(enc), AudioOut::None).unwrap();
    assert_eq!(r.frames, 35);
    let d = Demuxer::open(std::fs::read(dir.join("video.mp4")).unwrap()).unwrap();
    let (avg, frames) = h264enchrdmp4_avg(&d);
    assert_eq!(frames, 35);
    let (buffer, max, got_avg) = d.tracks()[0].btrt.expect("a CBR export carries a btrt");
    assert_eq!((buffer, max, got_avg), (50_000, 400_000, avg), "the HRD CPB / 8, the HRD rate, the floored coded rate");
    let coded: u64 = d.track_samples(0).map(|s| d.packet_at(s).data.len() as u64 * 8).sum(); assert!(coded.abs_diff(400_000 * frames / 25) <= 400_000, "coded {coded} bits (avgBitrate {avg}) within one CpbSize of 400000 x {frames} / 25 (the HRD bound; 258360 measured on this 1.4 s clip)");
}

/// H264ENCHRDMP4 (SR720): a constant-QP export has no bucket: the btrt is ffmpeg's CRF form (buffer 0, max = avg); an
/// audio-only `audio.mp4` (no video hint) carries none.
#[test]
fn h264enchrdmp4_cqp_export_carries_avg_only() {
    let tl = timeline();
    let src = sources(&tl);
    let dir = fresh("h264enchrdmp4_cqp");
    export(&tl, 0..TICKS, &src, &dir, Box::new(H264Encoder::new(26, 10)), AudioOut::None).unwrap();
    let d = Demuxer::open(std::fs::read(dir.join("video.mp4")).unwrap()).unwrap();
    let (avg, _) = h264enchrdmp4_avg(&d);
    assert!(avg > 0);
    assert_eq!(d.tracks()[0].btrt, Some((0, avg, avg)));
    let dir = fresh("h264enchrdmp4_audio");
    export(&tl, 0..TICKS, &src, &dir, Box::new(RawSequence), AudioOut::Mp4).unwrap();
    let a = Demuxer::open(std::fs::read(dir.join("audio.mp4")).unwrap()).unwrap();
    assert_eq!(a.tracks()[0].btrt, None, "no video encoder hint, no btrt");
}

// ---- NLE5DCLITABLE (LEDGER SR718): the one encoder table against tools/nle's old match (named tail block) ----
use stria::nle_export::{H264Opts, VIDEO_NAMES, video_named, video_named_with};

/// FNV-1a over everything an export takes from a video encoder: the colour call, every access unit (keyframe, time, data)
/// of every frame and of the flush, then the config (fourcc, config bytes) and the layout.
fn nle5dclitable_fnv(mut e: Box<dyn stria::nle_export::VideoEncoder>, frames: &[Vec<u8>]) -> u64 {
    fn eat(h: &mut u64, bytes: &[u8]) {
        for &b in bytes {
            *h = (*h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let _ = e.colour((2, 2));
    let mut units = Vec::new();
    for f in frames {
        units.extend(e.encode(64, 48, f).unwrap());
    }
    units.extend(e.flush().unwrap());
    for a in &units {
        eat(&mut h, &[a.keyframe as u8]);
        if let Some((d, p)) = a.time {
            eat(&mut h, &d.to_le_bytes());
            eat(&mut h, &p.to_le_bytes());
        }
        eat(&mut h, &a.data);
    }
    let c = e.config();
    eat(&mut h, &c.fourcc);
    eat(&mut h, &c.config);
    eat(&mut h, format!("{:?}", e.layout()).as_bytes());
    h
}

/// A first pass (`nle export --two-pass`'s `pass1_stats` chain: CBR 40 kbit/s at 25 fps) and its per-picture records.
fn nle5dclitable_pass1(frames: &[Vec<u8>]) -> Vec<stria::nle_export::PicStat> {
    let enc = H264Encoder::new(26, 10).with_bitrate(40, 25, 1);
    let stats = enc.stats();
    nle5dclitable_fnv(Box::new(enc), frames);
    let v = stats.borrow().clone();
    v
}

/// One `nle export` h264 option set: (qp, gop, bitrate, crf, scenecut, aq, hrd, two_pass).
type Nle5dSet = (u8, u32, Option<u32>, Option<u8>, Option<u8>, Option<u8>, bool, bool);

/// The option sets the tool accepts (and bitrate + crf, which the tool refuses before the table: the bitrate arm wins).
const NLE5DCLITABLE_SETS: [Nle5dSet; 8] = [
    (26, 10, None, None, None, None, false, false),
    (30, 5, None, None, Some(40), Some(10), false, false),
    (26, 10, Some(40), None, None, None, false, false),
    (26, 10, Some(40), None, None, None, true, false),
    (26, 10, None, Some(28), None, None, false, false),
    (26, 10, None, Some(28), None, None, true, false),
    (26, 10, Some(40), Some(28), None, None, false, false),
    (26, 10, Some(40), None, None, None, false, true),
];

/// tools/nle's encoder match before SR718 (main.rs `export_verb`), written out at the fixture's 25 fps: the oracle.
fn nle5dclitable_chain(s: Nle5dSet, stats: Vec<stria::nle_export::PicStat>) -> H264Encoder {
    let (qp, gop, bitrate, crf, scenecut, aq, hrd, two_pass) = s;
    match (bitrate, crf) {
        (Some(k), _) if two_pass => H264Encoder::new(qp, gop).with_scenecut(scenecut).with_aq(aq).with_two_pass(k, 25, 1, stats),
        (Some(k), _) => H264Encoder::new(qp, gop).with_scenecut(scenecut).with_aq(aq).with_hrd(hrd).with_bitrate(k, 25, 1),
        (None, Some(c)) => H264Encoder::new(qp, gop).with_scenecut(scenecut).with_aq(aq).with_quality(c, 25, 1),
        (None, None) => H264Encoder::new(qp, gop).with_scenecut(scenecut).with_aq(aq),
    }
}

#[test]
fn nle5dclitable_h264_table_is_the_cli_chain() {
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    let stats = nle5dclitable_pass1(&refr.frames);
    for (i, s) in NLE5DCLITABLE_SETS.iter().enumerate() {
        let (qp, gop, bitrate, crf, scenecut, aq, hrd, two_pass) = *s;
        let chain = nle5dclitable_fnv(Box::new(nle5dclitable_chain(*s, stats.clone())), &refr.frames);
        assert_eq!(chain, NLE5DCLITABLE_BEFORE[i], "set {i}: the old match's stream, pinned before SR718");
        let opts = H264Opts { qp, gop: Some(gop), bitrate, crf, scenecut, aq, min_keyint: None, hrd, two_pass_stats: two_pass.then(|| stats.clone()) };
        let table = nle5dclitable_fnv(video_named_with("h264", tl.tb, opts).unwrap(), &refr.frames);
        assert_eq!(table, chain, "set {i}: the table is the CLI's chain");
    }
}

/// The fnvs of [`nle5dclitable_chain`] over [`NLE5DCLITABLE_SETS`], measured on the tree before SR718 (the old match in place).
const NLE5DCLITABLE_BEFORE: [u64; 8] = [
    0xb3c1_26d2_643f_7976,
    0xd4a7_27d4_5b5d_78c5,
    0x050f_27c6_7cc3_2beb,
    0x7265_0fd7_6ad4_126c,
    0x3657_59b8_8c60_a4d0,
    0x3657_59b8_8c60_a4d0,
    0x050f_27c6_7cc3_2beb,
    0x7aca_82b3_ce88_470a,
];

#[test]
fn nle5dclitable_video_named_is_the_default_options() {
    let tl = timeline();
    let src = sources(&tl);
    let refr = reference(&tl, &src);
    for name in VIDEO_NAMES {
        let a = nle5dclitable_fnv(video_named(name, tl.tb).unwrap(), &refr.frames);
        let b = nle5dclitable_fnv(video_named_with(name, tl.tb, H264Opts::default()).unwrap(), &refr.frames);
        assert_eq!(a, b, "{name}");
    }
    let face = nle5dclitable_fnv(video_named("h264", tl.tb).unwrap(), &refr.frames);
    assert_eq!(face, 0x8318_f822_3505_f96b, "the face's h264 (SR694), pinned before SR718");
    assert_eq!(face, nle5dclitable_fnv(Box::new(H264Encoder::new(26, 25)), &refr.frames), "QP 26, a one-second GOP");
    assert!(video_named_with("vp9", tl.tb, H264Opts::default()).is_none());
}

// ---- NLE5DEXPORTAUDIOOPTIONS (LEDGER SR722): the one audio table against tools/nle's old audio match (named tail block) ----
use stria::nle_export::{AUDIO_NAMES, DEFAULT_AAC_KBPS, audio_named};

/// tools/nle's `--audio` match as it stood before SR722 (main.rs `export_verb`, pinned here as SR718 pinned the h264 chain).
fn nle5daudio_cli_arm(word: &str) -> Option<AudioOut> {
    match word {
        "wav" => Some(AudioOut::Wav),
        "mp4" => Some(AudioOut::Mp4),
        "aac" => Some(AudioOut::Aac(128)),
        "none" => Some(AudioOut::None),
        _ => None,
    }
}

#[test]
fn nle5dexportaudiooptions_table_is_the_cli_arm() {
    for w in AUDIO_NAMES {
        assert!(audio_named(w).is_some(), "{w}");
        assert_eq!(audio_named(w), nle5daudio_cli_arm(w), "{w} is the CLI's old arm");
    }
    for w in ["png", "h264", "flac", "pcm"] {
        assert_eq!(audio_named(w), nle5daudio_cli_arm(w), "{w}: refused by both");
    }
    assert_eq!(DEFAULT_AAC_KBPS, 128);
    assert_eq!(audio_named("aac:96"), Some(AudioOut::Aac(96)));
    assert_eq!(audio_named("aac:128"), audio_named("aac"));
    for w in ["flac", "aac:x", "aac:", "aac:0", "wav:96", "mp4:1", "AAC", ""] {
        assert_eq!(audio_named(w), None, "{w:?} refused");
    }
    println!("nle5dexportaudiooptions: table {} words = the CLI arm; aac:96 -> {:?}", AUDIO_NAMES.len(), audio_named("aac:96"));
}
