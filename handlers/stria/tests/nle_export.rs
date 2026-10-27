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
const TICKS: i64 = 35;

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

/// NLE5B: a fresh H264ENC encoder over the render's frames (the same converter and config): its samples, its avcC and
/// its reconstruction per frame.
fn fresh_h264(frames: &[Vec<u8>], gop: u32) -> (Vec<Vec<u8>>, Vec<u8>, Vec<[Vec<u8>; 3]>, Vec<I420>) {
    let mut enc = Encoder::new(Config { width: 64, height: 48, qp: 26, gop, ..Default::default() }).unwrap();
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
