// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE1 (SR194) M3: the schedule and the render agree frame for frame and sample for sample, over two synthetic
//! sources through the real demuxer and decoders (utp1 test pattern + PCM16 in MP4).

use gneiss_pal::dsp::video::TestPattern;
use stria::nle::{RenderSink, SYNTH_RATE, Sources, render, synth_sample, synth_source};
use timeline_core::schedule::schedule;
use timeline_core::{Clip, Kind, SourceId, Timeline, edl};

#[derive(Default)]
struct Collect {
    frames: Vec<(i64, Vec<u8>)>,
    pcm: Vec<f32>,
}
impl RenderSink for Collect {
    fn frame(&mut self, out_t: i64, w: u32, h: u32, rgba: &[u8]) {
        assert_eq!((w, h), (64, 48));
        self.frames.push((out_t, rgba.to_vec()));
    }
    fn pcm(&mut self, _out_t: i64, rate: u32, ch: u16, s: &[f32]) {
        assert_eq!((rate, ch), (SYNTH_RATE, 1));
        self.pcm.extend_from_slice(s);
    }
}

const BASE: [u32; 2] = [0, 1000];
const PER: usize = (SYNTH_RATE / 25) as usize; // samples per tick

/// V1 and A1: A[13,23)@0 (mid-GOP in point), B[0,10)@10 split at 15, a gap 20..25, A[23,33)@25.
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

fn agree(tl: &Timeline) {
    let mut src = Sources::new(tl);
    for (i, b) in BASE.iter().enumerate() {
        src.add(SourceId(i as u32), synth_source(*b, 50, 25)).unwrap();
    }
    let steps = schedule(tl, 0..35, &src).unwrap();
    let mut sink = Collect::default();
    let r = render(tl, 0..35, &src, &mut sink).unwrap();

    // Video: every output tick's frame is the frame its step names.
    let vs: Vec<_> = steps.iter().filter(|s| s.kind == Kind::Video).collect();
    assert_eq!((vs.len(), sink.frames.len()), (35, 35));
    let mut ok = 0;
    for (st, (out_t, rgba)) in vs.iter().zip(&sink.frames) {
        assert_eq!(st.out_t, *out_t);
        let got = TestPattern::read_counter(rgba, 64, 48);
        match st.src {
            Some(s) => assert_eq!(got, Some(BASE[s.0 as usize] + st.src_t as u32), "tick {out_t}"),
            None => assert!(rgba.chunks(4).all(|p| p == [0, 0, 0, 255]), "tick {out_t} not black"),
        }
        ok += 1;
    }
    // Anchors that do not lean on the schedule.
    let at = |t: usize| TestPattern::read_counter(&sink.frames[t].1, 64, 48);
    assert_eq!((at(0), at(9), at(10), at(15), at(19), at(20), at(25), at(34)), (Some(13), Some(22), Some(1000), Some(1005), Some(1009), None, Some(23), Some(32)));

    // Audio: every output sample is the source sample its step names.
    assert_eq!(sink.pcm.len(), 35 * PER);
    for st in steps.iter().filter(|s| s.kind == Kind::Audio) {
        for j in 0..st.len as usize * PER {
            let got = sink.pcm[st.out_t as usize * PER + j];
            let want = match st.src {
                Some(s) => synth_sample(BASE[s.0 as usize], st.src_t as u64 * PER as u64 + j as u64) as f32 / 32768.0,
                None => 0.0,
            };
            assert_eq!(got, want, "audio tick {} sample {j}", st.out_t);
        }
    }
    assert_eq!(sink.pcm[0], synth_sample(0, 13 * PER as u64) as f32 / 32768.0);
    assert_eq!(sink.pcm[10 * PER], synth_sample(1000, 0) as f32 / 32768.0);

    // Three cuts per kind seek (A at 13, B at 0, A at 23; the split continues); the run-up is 13-10 and 23-20.
    assert_eq!(r.frames, 35);
    assert_eq!(r.pcm_frames, 35 * PER as u64);
    assert_eq!(r.seeks, 6);
    assert_eq!(r.runup_frames, 6);
    assert_eq!((r.width, r.height, r.rate, r.channels), (64, 48, SYNTH_RATE, 1));
    println!("nle: frames={} seeks={} runup={} agree={ok}/{}", r.frames, r.seeks, r.runup_frames, vs.len());
}

#[test]
fn schedule_and_render_agree() {
    agree(&timeline());
}

#[test]
fn the_edl_text_renders_the_same() {
    let tl = timeline();
    let text = edl::write(&tl, "NLE1").unwrap();
    let back = edl::parse(&text, 25).unwrap();
    assert_eq!(back, tl);
    agree(&back);
}

#[test]
fn a_clip_past_the_media_is_refused() {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 80); // the timeline claims 80 ticks; the medium holds 50
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 40, 60, 0)];
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    let mut sink = Collect::default();
    assert!(matches!(render(&tl, 0..20, &src, &mut sink), Err(stria::nle::RenderError::Schedule(_))));
}

/// NLE3 (SR202): V1 A[0,20)@0, B[0,20)@20 (9-tick dissolve), C[5,25)@40 (9-tick W001 wipe), over utp1 sources whose
/// counters start at 0 / 1000 / 2000. The mid-dissolve frame is pixel_core's model (`blend_nonpremult`) of the two
/// counters' frames, byte for byte, and within its floor bound of their lerp; the mid-wipe frame's left half is the
/// incoming's, its right half the outgoing's.
#[test]
fn transitions_blend() {
    use timeline_core::{TransitionKind, WipeDir};
    let mut tl = Timeline::new(25);
    let ids: Vec<SourceId> = ["A", "B", "C"].iter().map(|r| tl.add_source(r, 50)).collect();
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(ids[0], 0, 20, 0), Clip::new(ids[1], 0, 20, 20), Clip::new(ids[2], 5, 25, 40)];
    let tl = tl
        .add_transition(0, 20, TransitionKind::Dissolve, 9)
        .unwrap()
        .add_transition(0, 40, TransitionKind::Wipe(WipeDir::LeftToRight), 9)
        .unwrap();
    let mut src = Sources::new(&tl);
    for (i, id) in ids.iter().enumerate() {
        src.add(*id, synth_source(i as u32 * 1000, 50, 25)).unwrap();
    }
    let mut sink = Collect::default();
    let r = render(&tl, 0..60, &src, &mut sink).unwrap();
    assert_eq!(sink.frames.len(), 60);
    let at = |t: usize| TestPattern::read_counter(&sink.frames[t].1, 64, 48);
    assert_eq!((at(0), at(19), at(29), at(39), at(49), at(59)), (Some(0), Some(19), Some(1009), Some(1019), Some(2014), Some(2024)));
    // mid-dissolve (tick 24, t = 5/10 -> alpha round(255 t) = 128): outgoing A at 24, incoming B at 4.
    // The model: the incoming at alpha 128 over the opaque outgoing, byte for byte.
    let (a, b) = (TestPattern::render(64, 48, 24), TestPattern::render(64, 48, 1004));
    let got = &sink.frames[24].1;
    let model: Vec<u8> = a
        .chunks_exact(4)
        .zip(b.chunks_exact(4))
        .flat_map(|(o, p)| gneiss_pal::dsp::image::blend_nonpremult([p[0], p[1], p[2], 128], [o[0], o[1], o[2], o[3]]))
        .collect();
    assert_eq!(got, &model);
    // Its bound against the lerp: for an opaque dst, dfa = 255 * (256 - 128) >> 8 = 127 (not 127.5) and each channel
    // is floor((s * 128 + d * 127) * floor(2^24 / 255) >> 24), so the weights are 128/255 and 127/255 and both floors
    // round down: the worst case s = 0, d = 255 gives 126 against 127.5. Every byte is within 1.5 of (s + d) / 2.
    for i in 0..got.len() {
        let want = (a[i] as f64 + b[i] as f64) / 2.0;
        assert!((got[i] as f64 - want).abs() <= 1.5, "byte {i}: got {} want {want}", got[i]);
    }
    assert_ne!(got, &a);
    // mid-wipe (tick 44, t = 5/10): incoming C at 9 on the left 32 columns, outgoing B at 24 on the right
    let (inc, out) = (TestPattern::render(64, 48, 2009), TestPattern::render(64, 48, 1024));
    let got = &sink.frames[44].1;
    for y in 0..48 {
        let row = y * 64 * 4;
        assert_eq!(&got[row..row + 32 * 4], &inc[row..row + 32 * 4], "row {y} left");
        assert_eq!(&got[row + 32 * 4..row + 64 * 4], &out[row + 32 * 4..row + 64 * 4], "row {y} right");
    }
    // three opens (A, B, C at their keyframe 0); the outgoing runs continue; C's run-up is 5 frames
    assert_eq!((r.frames, r.seeks, r.runup_frames), (60, 3, 5));
    println!("nle3: frames={} seeks={} runup={}", r.frames, r.seeks, r.runup_frames);
}

// ---- NLE4 (SR203): the audio mix — every audio track summed with its gain and fade, resampled to the project rate.

#[derive(Default)]
struct Pcm {
    rate: u32,
    pcm: Vec<f32>,
}
impl RenderSink for Pcm {
    fn frame(&mut self, _: i64, _: u32, _: u32, _: &[u8]) {}
    fn pcm(&mut self, _out_t: i64, rate: u32, ch: u16, s: &[f32]) {
        assert_eq!(ch, 1);
        self.rate = rate;
        self.pcm.extend_from_slice(s);
    }
}

/// Audio tracks only, one clip list per track, over synthetic sources of bases `bases` (50 ticks at 25 fps each).
fn mix(bases: &[u32], tracks: Vec<Vec<Clip>>, ticks: i64, rate: u32) -> (stria::nle::RenderReport, Pcm) {
    let mut tl = Timeline::new(25);
    for (i, _) in bases.iter().enumerate() {
        tl.add_source(&format!("S{i}"), 50);
    }
    for clips in tracks {
        let t = tl.add_track(Kind::Audio);
        tl.tracks[t].clips = clips;
    }
    let mut src = Sources::new(&tl);
    for (i, b) in bases.iter().enumerate() {
        src.add(SourceId(i as u32), synth_source(*b, 50, 25)).unwrap();
    }
    src.set_audio_rate(rate);
    let mut sink = Pcm::default();
    let r = render(&tl, 0..ticks, &src, &mut sink).unwrap();
    (r, sink)
}

fn q(v: i32) -> f32 {
    v as f32 / 32768.0
}

#[test]
fn two_tracks_sum_sample_for_sample() {
    let (a, b) = (SourceId(0), SourceId(1));
    let (r, s) = mix(&[0, 1000], vec![vec![Clip::new(a, 0, 20, 0)], vec![Clip::new(b, 0, 20, 0)]], 20, 0);
    assert_eq!(s.pcm.len(), 20 * PER);
    for (j, got) in s.pcm.iter().enumerate() {
        let want = synth_sample(0, j as u64) as i32 + synth_sample(1000, j as u64) as i32;
        assert_eq!(*got, q(want), "sample {j}");
    }
    assert_eq!((r.seeks, r.rate), (2, SYNTH_RATE));
}

#[test]
fn the_sum_clamps_to_sixteen_bits_once() {
    let (a, b) = (SourceId(0), SourceId(1));
    let (_, s) = mix(&[1000, 2000], vec![vec![Clip::new(a, 0, 10, 0)], vec![Clip::new(b, 0, 10, 0)]], 10, 0);
    for (j, got) in s.pcm.iter().enumerate() {
        let want = (synth_sample(1000, j as u64) as i32 + synth_sample(2000, j as u64) as i32).min(32767);
        assert_eq!(*got, q(want), "sample {j}");
    }
    assert_eq!(s.pcm[5000], q(32767));
}

#[test]
fn minus_six_db_halves() {
    let (_, s) = mix(&[0], vec![vec![Clip::new(SourceId(0), 0, 10, 0).gained(-602)]], 10, 0);
    for (j, got) in s.pcm.iter().enumerate() {
        let v = synth_sample(0, j as u64) as f32;
        let got = got * 32768.0;
        assert!((got - v / 2.0).abs() <= 1.0 + v / 1000.0, "sample {j}: {got} vs {}", v / 2.0);
    }
}

#[test]
fn a_fade_in_starts_silent_and_is_half_way_at_its_middle() {
    let (_, s) = mix(&[0], vec![vec![Clip::new(SourceId(0), 0, 20, 0).faded(10, 0)]], 20, 0);
    assert_eq!(s.pcm[0], 0.0);
    // tick 5 of a 10-tick fade: sample 9600 at half gain
    assert_eq!(s.pcm[5 * PER], q(synth_sample(0, 5 * PER as u64) as i32 / 2));
    // past the knee: full
    assert_eq!(s.pcm[12 * PER], q(synth_sample(0, 12 * PER as u64) as i32));
}

#[test]
fn a_48k_source_on_a_44k1_project_is_resampled() {
    let (r, s) = mix(&[0], vec![vec![Clip::new(SourceId(0), 0, 25, 0)]], 25, 44_100);
    assert_eq!((r.rate, s.rate), (44_100, 44_100));
    assert!((s.pcm.len() as i64 - 44_100).abs() <= 1, "{} samples", s.pcm.len());
    // NLE4B (SR222): the windowed sinc's tolerance. Output sample 441 sits exactly on source sample 480 (within one LSB),
    // and EVERY interior sample is the source ramp (`i mod 10000`) at `k * 48000 / 44100` within one LSB, skipping the
    // first 32 and the window's reach (24 source samples) around each wrap of the ramp, where any filter rings.
    assert!((s.pcm[441] * 32768.0 - synth_sample(0, 480) as f32).abs() <= 1.0, "{}", s.pcm[441] * 32768.0);
    let mut checked = 0;
    for (k, &v) in s.pcm.iter().enumerate().skip(32) {
        let x = k as f64 * 48_000.0 / 44_100.0;
        let ramp = x % 10_000.0;
        if !(24.0..10_000.0 - 24.0).contains(&ramp) {
            continue;
        }
        assert!((v as f64 * 32768.0 - ramp).abs() <= 1.0, "sample {k}: {} vs {ramp}", v as f64 * 32768.0);
        checked += 1;
    }
    assert!(checked > 43_000, "{checked}");
    assert_eq!(r.pcm_frames, s.pcm.len() as u64);
}

#[test]
fn only_a_channel_mismatch_is_refused() {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let st = tl.add_source("ST", 50);
    let (t1, t2) = (tl.add_track(Kind::Audio), tl.add_track(Kind::Audio));
    tl.tracks[t1].clips = vec![Clip::new(a, 0, 10, 0)];
    tl.tracks[t2].clips = vec![Clip::new(st, 0, 10, 0)];
    let stereo: Vec<i16> = (0..2 * 2 * SYNTH_RATE as usize).map(|i| i as i16).collect();
    let track = gneiss_pal::dsp::demux::build::pcm16_track(2, SYNTH_RATE, 2, &stereo, 960);
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    src.add(st, gneiss_pal::dsp::demux::build::mp4(&[track], &gneiss_pal::dsp::demux::build::Mp4Options::default())).unwrap();
    let mut sink = Collect::default();
    assert!(matches!(render(&tl, 0..10, &src, &mut sink), Err(stria::nle::RenderError::Format { .. })));
}

// ---- NLE3b / NLE3c (SR209): the audio crossfade under a dissolve; the W041 / W101 / W201 wipes; a fade from black.

/// NLE3b: V1 and A1 both A[0,20)@0, B[0,20)@20 with a 9-tick dissolve on V1. Inside 20..29 every output sample is the
/// Q16 model of `mix_into` (the outgoing A at its handle x the falling envelope plus the incoming B x the rising one,
/// each floored), and against the exact lerp within `(-2 - d, +d]`, `d = |xa - xb| / 65536`: the two floors give
/// `(-2, 0]` and the envelope's truncation (`eB = floor(ONE j / n)`, `eA = ONE - eB`) adds `(xa - xb) delta / 65536`,
/// `delta` in `[0, 1)`, whose sign follows `xa - xb`. Outside the span: the plain cut.
#[test]
fn nle3b_audio_crossfades_under_a_dissolve() {
    use timeline_core::TransitionKind;
    let mut tl = Timeline::new(25);
    let (a, b) = (tl.add_source("A", 50), tl.add_source("B", 50));
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    for t in 0..2 {
        tl.tracks[t].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20)];
    }
    let tl = tl.add_transition(0, 20, TransitionKind::Dissolve, 9).unwrap();
    let mut src = Sources::new(&tl);
    for (i, base) in BASE.iter().enumerate() {
        src.add(SourceId(i as u32), synth_source(*base, 50, 25)).unwrap();
    }
    let mut sink = Collect::default();
    let r = render(&tl, 0..40, &src, &mut sink).unwrap();
    assert_eq!(sink.pcm.len(), 40 * PER);
    let one: i64 = 1 << 16;
    let n = (9 * PER) as i64;
    let mut maxdev = 0f64;
    for (i, got) in sink.pcm.iter().enumerate() {
        let got = (*got * 32768.0).round() as i64;
        let want = if i < 20 * PER {
            synth_sample(0, i as u64) as i64
        } else if i >= 29 * PER {
            synth_sample(1000, (i - 20 * PER) as u64) as i64
        } else {
            let j = (i - 20 * PER) as i64;
            let (xa, xb) = (synth_sample(0, (i) as u64) as i64, synth_sample(1000, j as u64) as i64);
            let (ea, eb) = (one + (0 - one) * j / n, one * j / n);
            let model = ((xa * one * ea) >> 32) + ((xb * one * eb) >> 32);
            let lerp = xa as f64 + (xb - xa) as f64 * j as f64 / n as f64;
            let dev = got as f64 - lerp;
            let d = (xa - xb).abs() as f64 / 65536.0;
            assert!(dev > -2.0 - d && dev <= d, "sample {i}: {got} vs lerp {lerp} (d {d})");
            maxdev = maxdev.max(dev.abs());
            model
        };
        assert_eq!(got, want, "sample {i}");
    }
    // video: A and B open; audio: A, A's handle (the under layer's own seek), B
    assert_eq!(r.seeks, 5);
    println!("nle3b: xfade samples={n} maxdev={maxdev:.3} seeks={}", r.seeks);
}

/// NLE3c: each new wipe at t = 1/2 (tick 4 of a 7-tick wipe: weight 4/8) shows the incoming inside its shape and
/// the outgoing outside; a 5-tick fade from black's mid tick (weight 3/6) is the incoming blended over black.
#[test]
fn nle3c_new_wipes_and_a_fade_from_black() {
    use timeline_core::{TransitionKind, WipeDir};
    let render_one = |tl: &Timeline, t: i64| {
        let mut src = Sources::new(tl);
        for (i, base) in BASE.iter().enumerate() {
            src.add(SourceId(i as u32), synth_source(*base, 50, 25)).unwrap();
        }
        let mut sink = Collect::default();
        render(tl, t..t + 1, &src, &mut sink).unwrap();
        sink.frames.remove(0).1
    };
    let mut cut = Timeline::new(25);
    let (a, b) = (cut.add_source("A", 50), cut.add_source("B", 50));
    cut.add_track(Kind::Video);
    cut.tracks[0].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20)];
    // tick 23: outgoing A at 23, incoming B at 3
    let (out, inc) = (TestPattern::render(64, 48, 23), TestPattern::render(64, 48, 1003));
    let px = |f: &[u8], x: usize, y: usize| f[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4].to_vec();
    // (wipe, points that are the incoming's, points that are the outgoing's)
    let cases = [
        (WipeDir::Diagonal, vec![(0, 0), (30, 20)], vec![(63, 47), (40, 30)]),
        (WipeDir::Box, vec![(32, 24), (17, 13)], vec![(0, 0), (15, 24), (32, 11)]),
        (WipeDir::Clock, vec![(40, 10), (40, 40)], vec![(20, 10), (20, 40)]),
    ];
    let mut wipes = 0;
    for (d, ins, outs) in cases {
        let tl = cut.add_transition(0, 20, TransitionKind::Wipe(d), 7).unwrap();
        let f = render_one(&tl, 23);
        for (x, y) in ins {
            assert_eq!(px(&f, x, y), px(&inc, x, y), "{d:?} ({x},{y}) incoming");
        }
        for (x, y) in outs {
            assert_eq!(px(&f, x, y), px(&out, x, y), "{d:?} ({x},{y}) outgoing");
        }
        // and every pixel is one source's or the other's
        assert!((0..64 * 48).all(|i| f[i * 4..i * 4 + 4] == inc[i * 4..i * 4 + 4] || f[i * 4..i * 4 + 4] == out[i * 4..i * 4 + 4]));
        wipes += 1;
    }
    // from black: a gap 0..10, B[0,20)@10, a 5-tick dissolve from black; tick 12 = weight 3/6 = alpha 128
    let mut fb = Timeline::new(25);
    fb.add_source("A", 50);
    let bb = fb.add_source("B", 50);
    fb.add_track(Kind::Video);
    fb.tracks[0].clips = vec![Clip::new(bb, 0, 20, 10)];
    let fb = fb.add_transition_from_black(0, 10, TransitionKind::Dissolve, 5).unwrap();
    let f = render_one(&fb, 12);
    let inc = TestPattern::render(64, 48, 1002);
    let mut black_dev = 0f64;
    for i in 0..f.len() {
        let want = if i % 4 == 3 { 255.0 } else { inc[i] as f64 / 2.0 };
        let dev = (f[i] as f64 - want).abs();
        assert!(dev <= 1.5, "byte {i}: got {} want {want}", f[i]);
        black_dev = black_dev.max(dev);
    }
    println!("nle3c: wipes={wipes} black_dev={black_dev:.2}");
}

// ----- DEMUXWIRE (LEDGER SR213): the editor's and the Player's opens call DEMUXSEEK's reads -----

use std::sync::Arc;

use gneiss_pal::dsp::avsync::ManualTime;
use gneiss_pal::dsp::demux::build::{self, MediaTrack, MkvOptions, SampleSpec, TrackSpec};
use gneiss_pal::dsp::demux::{DecodeTime, TrackKind};
use gneiss_pal::dsp::video::reorder_depth;
use stria::media::Player;
use timeline_core::SourceIndex;

/// An AVCDecoderConfigurationRecord: a Main-profile SPS (4x3 macroblocks, level 3.0) whose VUI bitstream_restriction says
/// max_num_reorder_frames 2 / max_dec_frame_buffering 3 (demux_core's seek_index `sps_rbsp(Some((2, 3)), 0)`), and a
/// stand-in PPS.
const AVCC_REORDER2: [u8; 27] = [
    0x01, 0x4D, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x0C, 0x67, 0x4D, 0x00, 0x1E, 0xED, 0x88, 0xF4, 0x03, 0xC2, 0x21, 0x16, 0x48,
    0x01, 0x00, 0x04, 0x68, 0xCE, 0x38, 0x80,
];

/// Decode order I P B B P B B I P B B; presentation order in frame units (NLEPROBE's / DEMUXSEEK's pattern).
const PRES: [i64; 11] = [0, 3, 1, 2, 6, 4, 5, 7, 10, 8, 9];
const MS: i64 = 1_000_000;

/// An AVC track at 25 fps, 1 ms ticks (pts = (pres + 1) x 40), keyframes at decode index 0 and 7.
fn avc_bframes() -> MediaTrack {
    let spec = TrackSpec {
        id: 1,
        kind: TrackKind::Video,
        fourcc: *b"avc1",
        config_box: Some(*b"avcC"),
        codec_id: "V_MPEG4/ISO/AVC",
        config: AVCC_REORDER2.to_vec(),
        timescale: 1000,
        width: 64,
        height: 48,
        sample_rate: 0,
        channels: 0,
        bit_depth: 0,
        default_duration_ns: 0,
    };
    let samples = PRES
        .iter()
        .enumerate()
        .map(|(i, &p)| SampleSpec { data: vec![i as u8; 5 + i], dts: i as i64 * 40, pts: (p + 1) * 40, duration: 40, keyframe: i == 0 || i == 7 })
        .collect();
    MediaTrack { spec, samples }
}

#[test]
fn demuxwire_editor_open_indexes_and_derives() {
    let tl = Timeline::new(25);
    let mkv = |cues: bool| {
        build::mkv(&[avc_bframes()], &MkvOptions { webm: false, cues, cluster_blocks: 7, duration: true, ..Default::default() })
    };
    let mut src = Sources::new(&tl);
    src.add(SourceId(0), mkv(true)).unwrap();
    src.add(SourceId(1), mkv(false)).unwrap();
    src.add(SourceId(2), synth_source(0, 50, 25)).unwrap();
    let r = src.open_report(SourceId(0)).unwrap();
    assert_eq!(r.to_string(), "open=indexed dts=derived:2 edits=0 applied=first-only", "{r:?}");
    assert_eq!(src.open_report(SourceId(1)).unwrap().to_string(), "open=full dts=derived:2 edits=0 applied=first-only");
    let mp4 = src.open_report(SourceId(2)).unwrap();
    assert!(mp4.to_string().starts_with("open=full dts=stored edits="), "{mp4}");
    // keyframe-before: source time 0 is the first pts (40 ms); the second keyframe is presented at 320 ms. At 300 ms the
    // indexed reel reads the second Cluster's span (load_at) and answers what the whole-file reel answers.
    for id in [SourceId(0), SourceId(1)] {
        assert_eq!(src.keyframe_ns(id, Kind::Video, 300 * MS), Some(280 * MS), "{id:?}");
        assert_eq!(src.keyframe_ns(id, Kind::Video, 100 * MS), Some(0), "{id:?}");
    }
    println!("demuxwire: editor {r} key300={}ms", src.keyframe_ns(SourceId(0), Kind::Video, 300 * MS).unwrap() / MS);
}

#[test]
fn demuxwire_player_plays_a_bframe_mkv_in_decode_order() {
    let pcm = vec![0i16; 24_000];
    let file = build::mkv(&[avc_bframes(), build::pcm16_track(2, 48_000, 1, &pcm, 960)], &MkvOptions { webm: false, ..Default::default() });
    let p = Player::open(file, Arc::new(ManualTime::new()), None, 60).unwrap();
    assert_eq!(reorder_depth(p.video_track().unwrap()), Some(2), "the avcC's VUI reorder depth");
    let s = p.survey().unwrap();
    let v = s.streams.iter().find(|x| x.kind == TrackKind::Video).unwrap();
    let a = s.streams.iter().find(|x| x.kind == TrackKind::Audio).unwrap();
    assert_eq!(v.decode_time, DecodeTime::Derived(2));
    assert_eq!(a.decode_time, DecodeTime::Presentation, "audio is not reordered");
    // dts = the sorted pts two frames back: every frame decodes no later than it is presented.
    assert_eq!(v.composition_offsets, [80, 160, 40, 40, 160, 40, 40, 80, 160, 40, 40].map(|o| o * MS).to_vec());
    println!("demuxwire: player dts=derived:2");
}

// ---------------------------------------------------------------------------------------------------
// EDITMAP (LEDGER SR223): an edit-listed MP4 is its own timeline (`Sources::from_media`), and the Player maps the same list.
// ---------------------------------------------------------------------------------------------------

use stria::nle::synth_source_with;
use timeline_core::EditError;

/// A 50-frame 25 fps synth source (utp1 counters 0..49 + PCM16) whose video and audio edit lists both play 400 ms of the
/// media from 1.0 s, then 400 ms from 0.2 s (movie timescale 1000; video media 25000 ticks/s, audio 48000), at `rate`.
fn editmap_file(rate: i32) -> Vec<u8> {
    let x = build::Mp4Extras {
        edit_lists: vec![(1, vec![(400, 25_000, rate), (400, 5_000, rate)]), (2, vec![(400, 48_000, rate), (400, 9_600, rate)])],
        ..Default::default()
    };
    synth_source_with(0, 50, 25, &x)
}

#[test]
fn editmap_the_editor_plays_the_whole_edit_list() {
    let (tl, src) = Sources::from_media("AX", &editmap_file(0x1_0000), 25).unwrap().expect("an MP4 is media");
    let want = vec![Clip::new(SourceId(0), 25, 35, 0), Clip::new(SourceId(0), 5, 15, 10)];
    assert_eq!((tl.tracks[0].kind, &tl.tracks[0].clips), (Kind::Video, &want));
    assert_eq!((tl.tracks[1].kind, &tl.tracks[1].clips), (Kind::Audio, &want));
    assert_eq!((tl.sources[0].reel.as_str(), tl.sources[0].duration), ("AX", 50));
    let r = src.open_report(SourceId(0)).unwrap();
    assert_eq!(r.to_string(), "open=full dts=stored edits=2 applied=2");

    // The schedule seeks at each segment boundary only: 1.0 s from the 800 ms keyframe, 0.2 s from the 0 ms one.
    let steps = schedule(&tl, 0..20, &src).unwrap();
    let seeks: Vec<_> = steps.iter().filter(|s| s.kind == Kind::Video && s.seek.is_some()).map(|s| (s.out_t, s.src_t, s.seek.unwrap())).collect();
    assert_eq!(seeks, vec![(0, 25, 800 * MS), (10, 5, 0)]);

    // The render shows the segments' frames and plays their samples.
    let mut sink = Collect::default();
    let rr = render(&tl, 0..20, &src, &mut sink).unwrap();
    let at = |t: usize| TestPattern::read_counter(&sink.frames[t].1, 64, 48);
    assert_eq!((at(0), at(9), at(10), at(19)), (Some(25), Some(34), Some(5), Some(14)));
    assert_eq!(sink.pcm.len(), 20 * PER);
    assert_eq!(sink.pcm[0], synth_sample(0, 25 * PER as u64) as f32 / 32768.0);
    assert_eq!(sink.pcm[10 * PER], synth_sample(0, 5 * PER as u64) as f32 / 32768.0);
    assert_eq!(sink.pcm[20 * PER - 1], synth_sample(0, 15 * PER as u64 - 1) as f32 / 32768.0);
    println!("editmap: reel=AX {r} frames={} seeks={}", rr.frames, rr.seeks);
}

#[test]
fn editmap_a_rate_edit_is_refused_and_an_edl_is_not_media() {
    assert!(matches!(Sources::from_media("AX", &editmap_file(0x2_0000), 25), Err(stria::nle::RenderError::Edit(EditError::Rate))));
    let edl_text = edl::write(&timeline(), "EDITMAP").unwrap();
    assert!(Sources::from_media("AX", edl_text.as_bytes(), 25).unwrap().is_none());
    // A file without an edit list is the whole source.
    let (tl, src) = Sources::from_media("AX", &synth_source(0, 50, 25), 25).unwrap().unwrap();
    assert_eq!(tl.tracks[0].clips, vec![Clip::new(SourceId(0), 0, 50, 0)]);
    assert_eq!(src.open_report(SourceId(0)).unwrap().to_string(), "open=full dts=stored edits=0 applied=1");
}

#[test]
fn editmap_the_player_reports_the_same_segment_boundaries() {
    let file = editmap_file(0x1_0000);
    let p = Player::open(file.clone(), Arc::new(ManualTime::new()), None, 60).unwrap();
    let got: Vec<_> = p.edit_segments().iter().map(|c| (c.at, c.src_in, c.src_out)).collect();
    assert_eq!(got, vec![(0, 1000 * MS, 1400 * MS), (400 * MS, 200 * MS, 600 * MS)]);
    // The editor's clips at 40 ms ticks are the same boundaries.
    let (tl, _) = Sources::from_media("AX", &file, 25).unwrap().unwrap();
    let ed: Vec<_> = tl.tracks[0].clips.iter().map(|c| (c.at * 40 * MS, c.src_in * 40 * MS, c.src_out * 40 * MS)).collect();
    assert_eq!(got, ed);
    // A file without an edit list maps nothing.
    assert!(Player::open(synth_source(0, 50, 25), Arc::new(ManualTime::new()), None, 60).unwrap().edit_segments().is_empty());
    println!("editmap: player segments={}", got.len());
}
