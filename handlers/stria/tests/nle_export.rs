// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE5 (SR204): the editor export. Over NLE1's two synthetic sources (utp1 + PCM16 MP4) the exported PNG sequence
//! decodes back to the render's frames (counters equal the schedule's), the raw stream is the render's bytes, the WAV
//! and the PCM16 MP4 carry the render's samples; the WAV header and the f32 -> i16 quantiser are known answers.

use std::path::PathBuf;

use gneiss_pal::dsp::demux::{Codec, Demuxer, TrackKind};
use gneiss_pal::dsp::image::decode_png;
use gneiss_pal::dsp::video::TestPattern;
use stria::nle::{RenderSink, SYNTH_RATE, Sources, render, synth_sample, synth_source};
use stria::nle_export::{AudioOut, PngSequence, RawSequence, export, pcm16, wav_header};
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
