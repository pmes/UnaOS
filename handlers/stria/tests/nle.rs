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

/// NLE3cBLACK (SR323): a 5-tick dissolve TO black after A[0,20)@0: tick 22 (weight 3/6, alpha 128) is black blended
/// over A@22 by `blend_nonpremult` byte for byte, and the outgoing decodes on past its out point (one open in 0..25).
#[test]
fn nle3cblack_a_dissolve_to_black() {
    use timeline_core::TransitionKind;
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
    let tl = tl.add_transition_to_black(0, 20, TransitionKind::Dissolve, 5).unwrap();
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(BASE[0], 50, 25)).unwrap();
    let mut sink = Collect::default();
    render(&tl, 22..23, &src, &mut sink).unwrap();
    let f = sink.frames.remove(0).1;
    let out = TestPattern::render(64, 48, 22);
    for (i, (g, o)) in f.chunks_exact(4).zip(out.chunks_exact(4)).enumerate() {
        let want = gneiss_pal::dsp::image::blend_nonpremult([0, 0, 0, 128], [o[0], o[1], o[2], o[3]]);
        assert_eq!(g, &want[..], "pixel {i}");
    }
    let mut all = Collect::default();
    let r = render(&tl, 0..25, &src, &mut all).unwrap();
    assert_eq!((r.frames, r.seeks), (25, 1));
    println!("nle3cblack: to-black frames={} seeks={}", r.frames, r.seeks);
}

/// NLE3cBLACK: each new SMPTE wipe at t = 1/2 (tick 23 of a 7-tick wipe from A to B) shows the incoming inside its
/// shape and the outgoing outside, every pixel one source's or the other's.
#[test]
fn nle3cblack_new_wipe_codes() {
    use timeline_core::{TransitionKind, WipeDir};
    let mut cut = Timeline::new(25);
    let (a, b) = (cut.add_source("A", 50), cut.add_source("B", 50));
    cut.add_track(Kind::Video);
    cut.tracks[0].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20)];
    let (out, inc) = (TestPattern::render(64, 48, 23), TestPattern::render(64, 48, 1003));
    let px = |f: &[u8], x: usize, y: usize| f[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4].to_vec();
    let cases: [(WipeDir, Vec<(usize, usize)>, Vec<(usize, usize)>); 10] = [
        (WipeDir::CornerTopLeft, vec![(0, 0), (31, 23)], vec![(32, 0), (0, 24), (63, 47)]),
        (WipeDir::CornerTopRight, vec![(63, 0), (32, 23)], vec![(31, 0), (63, 24)]),
        (WipeDir::CornerBottomRight, vec![(63, 47), (32, 24)], vec![(31, 47), (63, 23)]),
        (WipeDir::CornerBottomLeft, vec![(0, 47), (31, 24)], vec![(32, 47), (0, 23)]),
        (WipeDir::BarnDoorVertical, vec![(16, 0), (47, 47), (32, 24)], vec![(15, 24), (48, 24)]),
        (WipeDir::BarnDoorHorizontal, vec![(0, 12), (63, 35)], vec![(32, 11), (32, 36)]),
        (WipeDir::DiagonalTopRight, vec![(63, 0), (40, 20)], vec![(0, 47), (20, 40)]),
        (WipeDir::ClockThree, vec![(10, 40), (50, 40)], vec![(10, 10), (50, 10)]),
        (WipeDir::ClockSix, vec![(10, 10), (10, 40)], vec![(50, 10), (50, 40)]),
        (WipeDir::ClockNine, vec![(10, 10), (50, 10)], vec![(10, 40), (50, 40)]),
    ];
    let mut src = Sources::new(&cut);
    for (i, base) in BASE.iter().enumerate() {
        src.add(SourceId(i as u32), synth_source(*base, 50, 25)).unwrap();
    }
    let mut wipes = 0;
    for (d, ins, outs) in cases {
        let tl = cut.add_transition(0, 20, TransitionKind::Wipe(d), 7).unwrap();
        let mut sink = Collect::default();
        render(&tl, 23..24, &src, &mut sink).unwrap();
        let f = sink.frames.remove(0).1;
        for (x, y) in ins {
            assert_eq!(px(&f, x, y), px(&inc, x, y), "{d:?} ({x},{y}) incoming");
        }
        for (x, y) in outs {
            assert_eq!(px(&f, x, y), px(&out, x, y), "{d:?} ({x},{y}) outgoing");
        }
        assert!((0..64 * 48).all(|i| f[i * 4..i * 4 + 4] == inc[i * 4..i * 4 + 4] || f[i * 4..i * 4 + 4] == out[i * 4..i * 4 + 4]));
        wipes += 1;
    }
    println!("nle3cblack: wipes={wipes}");
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

// ----- NLE3E (LEDGER SR225): a transition follows its cut through an insert and renders at the new tick -----

/// V1 A[0,20)@0, B[0,20)@20 with a 9-tick dissolve; a 10-tick C inserted at 0 moves the cut (and the dissolve) to
/// 30, and tick 34 is transitions_blend's mid-dissolve frame (B at 4 at alpha 128 over A at 24) byte for byte.
#[test]
fn nle3e_a_transition_follows_an_insert_and_renders_there() {
    use timeline_core::{Mode, TransitionKind};
    let mut tl = Timeline::new(25);
    let ids: Vec<SourceId> = ["A", "B", "C"].iter().map(|r| tl.add_source(r, 50)).collect();
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(ids[0], 0, 20, 0), Clip::new(ids[1], 0, 20, 20)];
    let tl = tl.add_transition(0, 20, TransitionKind::Dissolve, 9).unwrap();
    let tl = tl.place(0, Clip::new(ids[2], 5, 15, 0), Mode::Insert).unwrap();
    assert_eq!(tl.tracks[0].transitions.iter().map(|x| x.at).collect::<Vec<_>>(), vec![30]);
    let mut src = Sources::new(&tl);
    for (i, id) in ids.iter().enumerate() {
        src.add(*id, synth_source(i as u32 * 1000, 50, 25)).unwrap();
    }
    let mut sink = Collect::default();
    render(&tl, 34..35, &src, &mut sink).unwrap();
    let (a, b) = (TestPattern::render(64, 48, 24), TestPattern::render(64, 48, 1004));
    let model: Vec<u8> = a
        .chunks_exact(4)
        .zip(b.chunks_exact(4))
        .flat_map(|(o, p)| gneiss_pal::dsp::image::blend_nonpremult([p[0], p[1], p[2], 128], [o[0], o[1], o[2], o[3]]))
        .collect();
    assert_eq!((sink.frames.len(), sink.frames[0].0), (1, 34));
    assert_eq!(sink.frames[0].1, model);
    println!("nle3e: moved_at=30 frame34=blend");
}

// ---------------------------------------------------------------------------------------------------
// NLEPROBEHDRWIRE (LEDGER SR305): the colour reaches the editor's open (`OpenReport`, what `nle schedule` prints) and the
// Player (`Player::colour`) through `dsp::video::colour_of` passed to `fill_colour` beside `reorder_depth`.
// ---------------------------------------------------------------------------------------------------

use gneiss_pal::dsp::demux::colour::{Colour, Light, Mastering};
use gneiss_pal::dsp::video::colour_of;

/// [`AVCC_REORDER2`]'s SPS with a VUI video_signal_type: video_format 5, limited range, colour_description 1/1/1 (BT.709),
/// then the same bitstream_restriction (reorder 2, dpb 3): demux_core's seek_index
/// `sps_rbsp_vui(Some((2, 3)), 0, Some((1, 1, 1, false)))` in `avcc(..)`'s record (no emulation-prevention byte needed).
const AVCC_VUI709_REORDER2: [u8; 31] = [
    0x01, 0x4D, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x10, 0x67, 0x4D, 0x00, 0x1E, 0xED, 0x88, 0xF4, 0xD4, 0x04, 0x04, 0x04, 0x1E,
    0x11, 0x08, 0xB2, 0x40, 0x01, 0x00, 0x04, 0x68, 0xCE, 0x38, 0x80,
];

/// BT.2020 PQ, limited range, ST 2086 mastering (1000 / 0.005 cd/m²) and content light levels 1000 / 400 (NLEPROBEHDR's).
fn bt2020_pq() -> Colour {
    Colour {
        primaries: 9,
        transfer: 16,
        matrix: 9,
        full_range: Some(false),
        mastering: Some(Mastering { primaries: [[35400, 14600], [8500, 39850], [6550, 2300]], white: [15635, 16450], max_lum: 10_000_000, min_lum: 50 }),
        light: Some(Light { max_cll: 1000, max_fall: 400 }),
    }
}

fn avc_vui709() -> MediaTrack {
    let mut t = avc_bframes();
    t.spec.config = AVCC_VUI709_REORDER2.to_vec();
    t
}

#[test]
fn nleprobehdrwire_editor_and_player_report_a_bt2020_pq_mp4() {
    let x = build::Mp4Extras { colour: vec![(1, bt2020_pq())], ..Default::default() };
    let file = synth_source_with(0, 50, 25, &x);
    let tl = Timeline::new(25);
    let mut src = Sources::new(&tl);
    src.add(SourceId(0), file.clone()).unwrap();
    src.add(SourceId(1), synth_source(0, 50, 25)).unwrap();
    let r = src.open_report(SourceId(0)).unwrap();
    assert_eq!(r.colour, Some(bt2020_pq()));
    // What `nle schedule` prints after `reel=<r> ` (tools/nle: `println!("reel={} {r}", ..)`).
    assert_eq!(r.to_string(), "open=full dts=stored edits=0 applied=first-only colour=9/16/9/limited mdcv clli");
    // A reel that states no colour prints the open line it printed before the wire.
    let plain = src.open_report(SourceId(1)).unwrap();
    assert_eq!((plain.colour, plain.to_string().contains("colour=")), (None, false), "{plain}");

    let p = Player::open(file, Arc::new(ManualTime::new()), None, 60).unwrap();
    assert_eq!(p.colour(), Some(&bt2020_pq()));
    let q = Player::open(synth_source(0, 50, 25), Arc::new(ManualTime::new()), None, 60).unwrap();
    assert_eq!(q.colour(), None);
    println!("nleprobehdrwire: reel=AX {r}");
}

#[test]
fn nleprobehdrwire_a_no_colr_h264_reel_falls_back_to_the_vui() {
    let bt709 = Colour { primaries: 1, transfer: 1, matrix: 1, full_range: Some(false), mastering: None, light: None };
    let mkv = build::mkv(&[avc_vui709()], &MkvOptions { webm: false, ..Default::default() });
    let mp4 = build::mp4(&[avc_vui709()], &build::Mp4Options::default());
    // The container wins: an MP4 whose colr says BT.2020 PQ keeps it over the SPS's BT.709.
    let pq_only = Colour { mastering: None, light: None, ..bt2020_pq() };
    let x = build::Mp4Extras { colour: vec![(1, pq_only)], ..Default::default() };
    let tagged = build::mp4_with(&[avc_vui709()], &build::Mp4Options::default(), &x);

    let tl = Timeline::new(25);
    let mut src = Sources::new(&tl);
    src.add(SourceId(0), mkv.clone()).unwrap();
    src.add(SourceId(1), mp4.clone()).unwrap();
    src.add(SourceId(2), tagged.clone()).unwrap();
    let r = src.open_report(SourceId(0)).unwrap();
    assert_eq!(r.to_string(), "open=full dts=derived:2 edits=0 applied=first-only colour=1/1/1/limited", "{r:?}");
    let m = src.open_report(SourceId(1)).unwrap();
    assert!(m.to_string().ends_with(" colour=1/1/1/limited"), "{m}");
    assert_eq!(src.open_report(SourceId(2)).unwrap().colour, Some(pq_only));

    for (file, want) in [(mkv, bt709), (mp4, bt709), (tagged, pq_only)] {
        let p = Player::open(file, Arc::new(ManualTime::new()), None, 60).unwrap();
        assert_eq!(p.colour(), Some(&want));
    }
    // The dispatch itself reads the SPS VUI; the reorder depth beside it is unchanged by the colour description.
    let t = avc_vui709();
    let track = gneiss_pal::dsp::demux::Demuxer::open(build::mkv(&[t], &MkvOptions { webm: false, ..Default::default() })).unwrap().tracks()[0].clone();
    assert_eq!(colour_of(&track, &[]), Some(bt709));
    assert_eq!(reorder_depth(&track), Some(2));
    println!("nleprobehdrwire: vui {r}");
}

// COLOURONE (LEDGER SR328): the editor's render converts a reel's frames by the reel's colour. NLEPROBEHDR's synth reel is
// a test-pattern (RGBA) stream no matrix can show in, so the same BT.2020 PQ `colr` goes on an hvc1 reel of the HEIC
// fixture's PCM picture (64x48, a non-neutral chroma gradient).

fn colourone_render(file: Vec<u8>) -> Vec<u8> {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("P", 2);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 2, 0)];
    let mut src = Sources::new(&tl);
    src.add(SourceId(0), file).unwrap();
    let mut sink = Collect::default();
    render(&tl, 0..2, &src, &mut sink).unwrap();
    assert_eq!(sink.frames.len(), 2);
    sink.frames.swap_remove(0).1
}

#[test]
fn colourone_editor_renders_a_bt2020_reel_with_the_2020_matrix() {
    use gneiss_pal::dsp::image::heic_kat::{access_unit, hvc1_track, pcm_hvcc, pcm_slice};
    let track = || hvc1_track(pcm_hvcc(), &access_unit(&pcm_slice()), 64, 48, 2);
    let x = build::Mp4Extras { colour: vec![(1, bt2020_pq())], ..Default::default() };
    let tagged = colourone_render(build::mp4_with(&[track()], &build::Mp4Options::default(), &x));
    let plain = colourone_render(build::mp4(&[track()], &build::Mp4Options::default()));
    let at = |rgba: &[u8], x: usize, y: usize| rgba[(y * 64 + x) * 4..][..4].to_vec();
    // (10, 10): Y 100 Cb 60 Cr 185 — BT.2020 limited (298, 430, 48, 167, 548) at >> 8 gives (194, 73, 0); BT.601 (189, 78, 0).
    // (20, 6): Y 158 Cb 73 Cr 184 — BT.2020 (255, 139, 48); BT.601 (255, 141, 54).
    assert_eq!((at(&tagged, 10, 10), at(&plain, 10, 10)), (vec![194, 73, 0, 255], vec![189, 78, 0, 255]));
    assert_eq!((at(&tagged, 20, 6), at(&plain, 20, 6)), (vec![255, 139, 48, 255], vec![255, 141, 54, 255]));
    println!("colourone: editor bt2020 px=194,73,0 bt601=189,78,0");
}

// NLE-STANDIN-OPEN (LEDGER SR353): a reel no decoder fits opens in the editor as the Player plays it — the labelled stand-in
// through `stria::decode_swap::open` — instead of the render failing on `RenderError::Decode`, and the timeline stays editable.

/// Rule: a `V_MPEG4/ISO/ASP` reel (Codec::Other, no core decodes it) opens with ` decode=stand-in` in its open report BEFORE any render,
/// renders every tick as the stand-in's labelled counter of that source frame, and a trim renders the trimmed frame.
#[test]
fn nle_standin_open_a_reel_no_decoder_fits_is_edited_on_stand_in_frames() {
    // Matroska states the frame size for any video codec (MP4 reads it only for the sample entries it maps), and
    // `V_MPEG4/ISO/ASP` (MPEG-4 Part 2) is mapped by no demux_core arm (Codec::Other) and decoded by no core.
    let mut t = build::test_pattern_track(1, 64, 48, 25, 10, 5);
    t.spec.codec_id = "V_MPEG4/ISO/ASP";
    let file = build::mkv(&[t], &MkvOptions::default());
    let (tl, src) = Sources::from_media("S", &file, 25).unwrap().unwrap();
    let report = src.open_report(SourceId(0)).unwrap().to_string();
    assert!(report.ends_with(" decode=stand-in"), "{report}");
    let mut sink = Collect::default();
    let r = render(&tl, 0..10, &src, &mut sink).unwrap();
    assert_eq!(r.frames, 10);
    assert_eq!(sink.frames.iter().map(|f| f.0).collect::<Vec<_>>(), (0..10).collect::<Vec<i64>>());
    for (i, (_, rgba)) in sink.frames.iter().enumerate() {
        assert_eq!(rgba, &TestPattern::render(64, 48, i as u32), "tick {i} is not the stand-in's frame {i}");
    }
    let trimmed = tl.trim_in(0, 0, 3).unwrap();
    let mut sink = Collect::default();
    render(&trimmed, 3..5, &src, &mut sink).unwrap();
    assert_eq!(sink.frames[0], (3, TestPattern::render(64, 48, 3)));
    println!("nle-standin-open: {report} frames={}", r.frames);
}

/// SWAPBACKNLECACHE (LEDGER SR388): a reel's runs reuse ONE held decoder (each run's close parks it in the reel's `Media`, the
/// next open resets it, the Player's seek). Rule: ticks rendered one at a time out of order (a preview scrub: each a run opened
/// on the held, reset decoder) draw exactly the whole render's frames at those ticks.
#[test]
fn swapbacknlecache_scrubbed_runs_draw_the_whole_render() {
    let tl = timeline();
    let mut src = Sources::new(&tl);
    for (i, b) in BASE.iter().enumerate() {
        src.add(SourceId(i as u32), synth_source(*b, 50, 25)).unwrap();
    }
    let mut whole = Collect::default();
    render(&tl, 0..35, &src, &mut whole).unwrap();
    for t in [30i64, 5, 17, 12, 30, 5, 0, 34] {
        let mut one = Collect::default();
        render(&tl, t..t + 1, &src, &mut one).unwrap();
        assert_eq!(one.frames, [whole.frames[t as usize].clone()], "tick {t}");
    }
    println!("swapbacknlecache: scrub 8 ticks = whole");
}

// NLESIZESWITCH (LEDGER SR384): the editor's run across a mid-stream size switch. SR379's two-size AV1 reel (kodim23's item,
// 768x512, for samples 0-1, kids_720p's, 1280x720, a new coded video sequence, for 2-3; the track declares 768x512) was
// refused `source 0 frame 1280x720 is not the project size`; now the run letterboxes each switched frame into the reel's
// first size through the editor's ONE fit (`timeline_core::view::Canvas::blit_fit`), says so once, and the host export
// encodes every frame at that size. The pictures are av1_core's sha256-pinned public vectors (SKIP offline).
#[path = "../../../unaos/libs/media/av1_core/tests/common/mod.rs"]
mod av1_vectors;

/// The run's log lines, captured per thread (tests/media.rs's HEVCPAL-SWAP shape; this binary had no logger).
struct NleSizeSwitchLog;
static NLESIZESWITCH_LINES: std::sync::Mutex<Vec<(std::thread::ThreadId, String)>> = std::sync::Mutex::new(Vec::new());
impl log::Log for NleSizeSwitchLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        NLESIZESWITCH_LINES.lock().unwrap().push((std::thread::current().id(), r.args().to_string()));
    }
    fn flush(&self) {}
}
fn nlesizeswitch_lines(containing: &str) -> Vec<String> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        log::set_logger(&NleSizeSwitchLog).expect("no other logger in this test binary");
        log::set_max_level(log::LevelFilter::Info);
    });
    let me = std::thread::current().id();
    NLESIZESWITCH_LINES.lock().unwrap().iter().filter(|(t, l)| *t == me && l.contains(containing)).map(|(_, l)| l.clone()).collect()
}

/// SR379's fixture (tests/media.rs `av1inbandsize_reel`): one av01 track declared 768x512, 4 key samples of 40 ms.
fn nlesizeswitch_reel() -> Option<Vec<u8>> {
    use gneiss_pal::dsp::demux::build::{self, Mp4Options};
    let small = av1_vectors::avif_item(&av1_vectors::fetch("kodim23_yuv420_8bpc")?).unwrap();
    let big = av1_vectors::avif_item(&av1_vectors::fetch("kids_720p")?).unwrap();
    assert_eq!((small.ispe, big.ispe), (Some((768, 512)), Some((1280, 720))));
    let mut spec = build::TrackSpec::test_pattern(1, 768, 512, 25);
    spec.fourcc = *b"av01";
    spec.config_box = Some(*b"av1C");
    spec.codec_id = "V_AV1";
    spec.config = small.av1c.clone().unwrap()[..4].to_vec();
    spec.timescale = 1000;
    let samples = (0..4).map(|i| build::SampleSpec { data: if i < 2 { small.data.clone() } else { big.data.clone() }, dts: i * 40, pts: i * 40, duration: 40, keyframe: true }).collect();
    Some(build::mp4_with(&[build::MediaTrack { spec, samples }], &Mp4Options::default(), &build::Mp4Extras::default()))
}

#[derive(Default)]
struct NleSizeSwitchSink {
    frames: Vec<(u32, u32, Vec<u8>)>,
}
impl RenderSink for NleSizeSwitchSink {
    fn frame(&mut self, _out_t: i64, w: u32, h: u32, rgba: &[u8]) {
        self.frames.push((w, h, rgba.to_vec()));
    }
    fn pcm(&mut self, _out_t: i64, _rate: u32, _ch: u16, _s: &[f32]) {}
}

#[test]
fn nlesizeswitch_two_size_reel_runs_and_exports_at_the_first_size() {
    use gneiss_pal::dsp::demux::Demuxer;
    use gneiss_pal::dsp::video::{chroma_of, decoder_for};
    use timeline_core::view::{Canvas, FIELD, Order, Still};
    let _ = nlesizeswitch_lines("");
    let Some(file) = nlesizeswitch_reel() else { return };
    let (tl, src) = Sources::from_media("two", &file, 25).unwrap().expect("an MP4");

    // The run: every frame at the reel's first size.
    let mut sink = NleSizeSwitchSink::default();
    let r = render(&tl, 0..4, &src, &mut sink).unwrap();
    assert_eq!((r.width, r.height, r.frames), (768, 512, 4));
    assert!(sink.frames.iter().all(|(w, h, px)| (*w, *h, px.len()) == (768, 512, 768 * 512 * 4)), "one size reaches the sink");

    // The expected pictures: the decoder's own frames converted by the reel's colour, the switched ones fitted by Canvas::blit_fit.
    let colour = src.open_report(SourceId(0)).and_then(|o| o.colour);
    let mut d = Demuxer::open(file.clone()).unwrap();
    let track = d.tracks()[0].clone();
    let mut dec = decoder_for(&track).unwrap();
    let mut decoded = Vec::new();
    while let Some(p) = d.next_packet() {
        decoded.extend(dec.decode(&p).unwrap());
    }
    decoded.extend(dec.flush());
    assert_eq!(decoded.iter().map(|f| (f.width, f.height)).collect::<Vec<_>>(), vec![(768, 512), (768, 512), (1280, 720), (1280, 720)]);
    for (i, f) in decoded.iter().enumerate() {
        let native = f.to_rgba_colour(colour.as_ref(), chroma_of(&track));
        let want = if i < 2 {
            native
        } else {
            let mut c = Canvas::new(768, 512, Order::Rgba);
            c.rect(0, 0, 768, 512, FIELD);
            c.blit_fit(&Still { width: 1280, height: 720, rgba: native }, 0, 0, 768, 512);
            c.px
        };
        assert!(sink.frames[i].2 == want, "frame {i}: the run's picture is the reel's conversion (fitted when switched)");
    }
    // 1280x720 into 768x512 keeps its aspect: 768x432 at y 40, black bars above and below.
    let px = |i: usize, x: usize, y: usize| sink.frames[i].2[(y * 768 + x) * 4..(y * 768 + x) * 4 + 4].to_vec();
    assert_eq!((px(2, 384, 0), px(2, 384, 39), px(2, 384, 472), px(2, 384, 511)), (FIELD.to_vec(), FIELD.to_vec(), FIELD.to_vec(), FIELD.to_vec()));
    let lines = nlesizeswitch_lines("nle size switch");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("reel=0 1280x720->768x512 scaled"), "{}", lines[0]);

    // The host export: the encoder sees the first size only.
    use stria::nle_export::{AudioOut, H264Encoder, export};
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("nlesizeswitch");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let rep = export(&tl, 0..4, &src, &dir, Box::new(H264Encoder::new(26, 10)), AudioOut::None).unwrap();
    assert_eq!(rep.frames, 4);
    let out = Demuxer::open(std::fs::read(dir.join("video.mp4")).unwrap()).unwrap();
    let vt = out.tracks()[0].clone();
    assert_eq!((vt.width, vt.height, vt.sample_count), (768, 512, 4));
    let mut dec = decoder_for(&vt).unwrap();
    let mut got = Vec::new();
    for s in out.track_samples(0).copied().collect::<Vec<_>>() {
        got.extend(dec.decode(&out.packet_at(&s)).unwrap());
    }
    got.extend(dec.flush());
    assert_eq!(got.iter().map(|f| (f.width, f.height)).collect::<Vec<_>>(), vec![(768, 512); 4]);
    let lines = nlesizeswitch_lines("nle size switch");
    assert_eq!(lines.len(), 2, "the export's own run says it once more: {lines:?}");
    println!("nlesizeswitch: {} ; run 768x512 x4, export avc1 768x512 x4", lines[0]);
}

// ── NLESIZEPOLICY (LEDGER SR394): SR384's two-size reel under each size policy (handlers.md §5 NLEPROBE-HDR's NLE-SIZE-POLICY) ──
// The reel's policy is an EDL property (`* UNA SIZE`); Letterbox is SR384's run above, unchanged. Fill and Stretch fit the
// 1280x720 frames into the reel's 768x512 through `Canvas::blit_policy`, pinned here against a hand nearest loop over the
// decoder's own conversion; Adopt hands them on at 1280x720 and the H.264 export refuses by the encoder's own name.

/// The decoder's own frames of the SR379 reel, converted by its colour (as `src_rgba` does).
fn nlesizepolicy_native(file: &[u8], src: &Sources) -> Vec<(u32, u32, Vec<u8>)> {
    use gneiss_pal::dsp::demux::Demuxer;
    use gneiss_pal::dsp::video::{chroma_of, decoder_for};
    let colour = src.open_report(SourceId(0)).and_then(|o| o.colour);
    let mut d = Demuxer::open(file.to_vec()).unwrap();
    let track = d.tracks()[0].clone();
    let mut dec = decoder_for(&track).unwrap();
    let mut out = Vec::new();
    while let Some(p) = d.next_packet() {
        out.extend(dec.decode(&p).unwrap());
    }
    out.extend(dec.flush());
    out.iter().map(|f| (f.width, f.height, f.to_rgba_colour(colour.as_ref(), chroma_of(&track)))).collect()
}

/// A hand nearest sample: the 768x512 picture whose pixel (x, y) is the 1280x720 `native`'s
/// (`sx0 + x * sw / 768`, `y * 720 / 512`) — the source rect `sw` wide from column `sx0`.
fn nlesizepolicy_hand(native: &[u8], sx0: usize, sw: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(768 * 512 * 4);
    for y in 0..512 {
        for x in 0..768 {
            let si = ((y * 720 / 512) * 1280 + sx0 + x * sw / 768) * 4;
            out.extend_from_slice(&native[si..si + 4]);
        }
    }
    out
}

#[test]
fn nlesizepolicy_two_size_reel_under_each_policy() {
    use timeline_core::SizePolicy;
    let _ = nlesizeswitch_lines("");
    let Some(file) = nlesizeswitch_reel() else { return };
    let (tl, src) = Sources::from_media("two", &file, 25).unwrap().expect("an MP4");
    let native = nlesizepolicy_native(&file, &src);
    assert_eq!(native.iter().map(|f| (f.0, f.1)).collect::<Vec<_>>(), vec![(768, 512), (768, 512), (1280, 720), (1280, 720)]);
    let px = |p: &[u8], w: usize, x: usize, y: usize| p[(y * w + x) * 4..(y * w + x) * 4 + 4].to_vec();

    for (fit, sx0, sw) in [(SizePolicy::Fill, 100, 1080), (SizePolicy::Stretch, 0, 1280)] {
        // The policy rides the EDL: written, read back, and the run takes the read-back timeline.
        let set = tl.set_size_policy(SourceId(0), fit).unwrap();
        let text = edl::write(&set, "TWO").unwrap();
        assert!(text.contains(&format!("* UNA SIZE {} {}\n", set.sources[0].reel, fit.name())), "{text}");
        let back = edl::parse(&text, 25).unwrap();
        assert_eq!(back.sources[0].size_policy, fit);
        let mut sink = NleSizeSwitchSink::default();
        let r = render(&back, 0..4, &src, &mut sink).unwrap();
        assert_eq!((r.width, r.height, r.frames), (768, 512, 4));
        assert!(sink.frames.iter().all(|(w, h, p)| (*w, *h, p.len()) == (768, 512, 768 * 512 * 4)), "{fit:?}: one size reaches the sink");
        assert!(sink.frames[0].2 == native[0].2 && sink.frames[1].2 == native[1].2, "{fit:?}: first-size frames pass as decoded");
        for i in 2..4 {
            assert!(sink.frames[i].2 == nlesizepolicy_hand(&native[i].2, sx0, sw), "{fit:?} frame {i}: the hand nearest sample of the placed rect");
        }
        // Named pins: the corners map to the placed source rect's corners (no bars: every pixel is the picture's).
        let (f, n) = (&sink.frames[2].2, &native[2].2);
        assert_eq!(px(f, 768, 0, 0), px(n, 1280, sx0, 0));
        assert_eq!(px(f, 768, 767, 511), px(n, 1280, sx0 + 767 * sw / 768, 718));
        assert_eq!(px(f, 768, 384, 256), px(n, 1280, sx0 + 384 * sw / 768, 360));
        let lines = nlesizeswitch_lines(&format!("fit={}", fit.name()));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains(&format!("nle size switch reel=0 1280x720->768x512 scaled fit={}", fit.name())), "{}", lines[0]);
    }
    let fill = tl.set_size_policy(SourceId(0), SizePolicy::Fill).unwrap();
    let mut a = NleSizeSwitchSink::default();
    render(&fill, 0..4, &src, &mut a).unwrap();
    let st = tl.set_size_policy(SourceId(0), SizePolicy::Stretch).unwrap();
    let mut b = NleSizeSwitchSink::default();
    render(&st, 0..4, &src, &mut b).unwrap();
    assert!(a.frames[2].2 != b.frames[2].2, "Fill crops, Stretch squeezes: two pictures");

    // Adopt: the run hands the switched frames on at their own size, said once; the H.264 export refuses by the encoder's name.
    let adopt = tl.set_size_policy(SourceId(0), SizePolicy::Adopt).unwrap();
    let mut sink = NleSizeSwitchSink::default();
    let r = render(&adopt, 0..4, &src, &mut sink).unwrap();
    assert_eq!((r.width, r.height, r.frames), (768, 512, 4), "the report keeps the project's size");
    assert_eq!(sink.frames.iter().map(|f| (f.0, f.1)).collect::<Vec<_>>(), vec![(768, 512), (768, 512), (1280, 720), (1280, 720)]);
    for (i, f) in sink.frames.iter().enumerate() {
        assert!(f.2 == native[i].2, "adopt frame {i}: the decoder's own picture");
    }
    let lines = nlesizeswitch_lines("adopted");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("nle size switch reel=0 1280x720 adopted"), "{}", lines[0]);
    use stria::nle_export::{AudioOut, ExportError, H264Encoder, export};
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("nlesizepolicy");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let e = export(&adopt, 0..4, &src, &dir, Box::new(H264Encoder::new(26, 10)), AudioOut::None).unwrap_err();
    assert!(matches!(e, ExportError::H264(h264enc_core::Error::FrameSize)), "{e}");
    println!("nlesizepolicy: fill/stretch 768x512 x4 pinned, adopt 1280x720 frames 2-3, export refused: {e}");
}

// ── NLESIZEPOLICYUI (LEDGER SR402): the reel's size policy chosen from the editor (handlers.md §5 NLEPROBE-HDR) ──────────
// The `size` words of timeline_core's ONE verb dispatch (`EditorState::verb`, both faces), the EDL's `* UNA SIZE` round
// trip through the ONE writer, the refusals by name, and the host face's Size button (a 4-way cycle) and label token.

use stria::nle_face::{EditorState, Face, PixelOrder, VERBS, synth_fixture};
use timeline_core::SizePolicy;

/// The labels the ONE painter draws (`timeline_core::view::Text`), recorded.
struct NlesizepolicyuiLabels(Vec<String>);
impl timeline_core::view::Text for NlesizepolicyuiLabels {
    fn text(&mut self, _: &mut timeline_core::view::Canvas, s: &str, _: f32, _: f32, _: (i32, i32)) {
        self.0.push(s.to_string());
    }
}

fn nlesizepolicyui_labels(st: &EditorState) -> Vec<String> {
    let mut rec = NlesizepolicyuiLabels(Vec::new());
    timeline_core::view::paint(st, 320, 160, timeline_core::view::Order::Rgba, Some(&mut rec), None);
    rec.0
}

#[test]
fn nlesizepolicyui_size_verb_sets_the_reels_policy_and_the_edl_round_trips_it() {
    let (tl0, _) = synth_fixture();
    let b = tl0.source_by_reel("B").unwrap().id;
    let mut st = EditorState::new(tl0.clone());
    assert_eq!(st.verb("size B fill"), Some(Ok(())));
    assert_eq!(st.timeline, tl0.set_size_policy(b, SizePolicy::Fill).unwrap());
    assert_eq!(st.undo_depth(), 1);
    assert_eq!(st.verb("size B fill"), Some(Ok(())), "the same policy again");
    assert_eq!(st.undo_depth(), 1, "commits nothing");
    let text = edl::write(&st.timeline, "SIZE").unwrap();
    assert!(text.contains("* UNA SIZE B fill\n"), "{text}");
    assert!(!text.contains("* UNA SIZE A"), "a Letterbox reel writes no line: {text}");
    let back = edl::parse(&text, 25).unwrap();
    assert_eq!(back.source_by_reel("B").unwrap().size_policy, SizePolicy::Fill);
    assert_eq!(back.source_by_reel("A").unwrap().size_policy, SizePolicy::Letterbox);
    assert_eq!(st.verb("size A ADOPT"), Some(Ok(())), "the policy word ignores ASCII case");
    assert_eq!(st.timeline.source_by_reel("A").unwrap().size_policy, SizePolicy::Adopt);
    assert_eq!(st.verb("undo"), Some(Ok(())));
    assert_eq!(st.verb("undo"), Some(Ok(())));
    assert_eq!(st.timeline, tl0, "undo restores Letterbox");
    assert_eq!(edl::write(&st.timeline, "SIZE").unwrap(), edl::write(&tl0, "SIZE").unwrap());
}

#[test]
fn nlesizepolicyui_refuses_an_unknown_reel_and_an_unknown_policy_by_name() {
    let (tl0, _) = synth_fixture();
    let mut st = EditorState::new(tl0.clone());
    assert_eq!(st.verb("size Q fill"), Some(Err(EditError::NoSource)));
    assert_eq!(st.verb("size Q huge"), Some(Err(EditError::NoSource)));
    assert_eq!(st.verb("size A huge"), Some(Err(EditError::Policy)));
    assert_eq!(st.verb("size A"), Some(Err(EditError::Policy)));
    assert_eq!(st.verb("size A fill extra"), Some(Err(EditError::Policy)));
    assert_eq!(st.verb("size"), Some(Err(EditError::NoClip)), "a bare size cycles the selected clip's reel");
    assert_eq!(st.verb("sizes"), None, "not a size verb");
    assert_eq!(st.timeline, tl0);
    assert_eq!(st.undo_depth(), 0);
    assert_eq!(EditError::Policy.to_string(), "no such size policy (letterbox, fill, stretch, adopt)");
    assert_eq!(EditError::NoSource.to_string(), "no such source");
    assert_eq!(SizePolicy::ALL.map(|p| p.next()), [SizePolicy::Fill, SizePolicy::Stretch, SizePolicy::Adopt, SizePolicy::Letterbox]);
}

#[test]
fn nlesizepolicyui_face_size_button_cycles_through_the_verb() {
    assert!(VERBS.contains(&"size"));
    let (tl0, src) = synth_fixture();
    let b = tl0.source_by_reel("B").unwrap().id;
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("size"));
    assert_eq!(face.last_error.take().as_deref(), Some("size: no such clip"));
    assert_eq!(face.state.timeline, tl0);
    face.state.select(0, 1).unwrap();
    let labels = nlesizepolicyui_labels(&face.state);
    assert!(labels.iter().any(|s| s == "B") && !labels.iter().any(|s| s.starts_with("B ")), "{labels:?}");
    let mut want = tl0.clone();
    for p in [SizePolicy::Fill, SizePolicy::Stretch, SizePolicy::Adopt, SizePolicy::Letterbox] {
        assert!(face.verb("size"));
        assert_eq!(face.last_error.take(), None);
        want = want.set_size_policy(b, p).unwrap();
        assert_eq!(face.state.timeline, want, "{p:?}");
        let labels = nlesizepolicyui_labels(&face.state);
        let token = if p == SizePolicy::Letterbox { "B".to_string() } else { format!("B {}", p.name()) };
        assert!(labels.contains(&token), "{p:?}: {labels:?}");
        assert!(labels.iter().any(|s| s == "A"), "reel A's label untouched: {labels:?}");
    }
    assert_eq!(face.state.undo_depth(), 4);
    assert!(face.verb("size B huge"));
    assert_eq!(face.last_error.take().as_deref(), Some("size B huge: no such size policy (letterbox, fill, stretch, adopt)"));
}

// ── ADOPTPNG (LEDGER SR405): the PNG sequence honours Adopt (handlers.md §5 NLEPROBE-HDR's ADOPT-PNG) ─────────────────────
// SR394's two-size reel through `nle_export::export_png`: under Adopt the files are frames of two sizes, read back by
// pixel_core's header reader and, independently, by the PNG decoder (lossless: the decoder's own picture); under Letterbox,
// Fill and Stretch one size. A directory that cannot be made is refused naming it.

#[test]
fn adoptpng_two_size_reel_exports_png_frames_at_their_own_size() {
    use gneiss_pal::dsp::image::{decode_png, facts_of};
    use stria::nle_export::{AudioOut, Container, export_png};
    let _ = nlesizeswitch_lines("");
    let Some(file) = nlesizeswitch_reel() else { return };
    let (tl, src) = Sources::from_media("two", &file, 25).unwrap().expect("an MP4");
    let native = nlesizepolicy_native(&file, &src);
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("adoptpng");
    let _ = std::fs::remove_dir_all(&root);

    let adopt = tl.set_size_policy(SourceId(0), SizePolicy::Adopt).unwrap();
    let dir = root.join("adopt");
    let (r, frames) = export_png(&adopt, 0..4, &src, &dir, AudioOut::None, Container::Mp4).unwrap();
    assert_eq!((r.frames, r.files), (4, 4));
    assert_eq!(frames.iter().map(|f| (f.1, f.2)).collect::<Vec<_>>(), vec![(768, 512), (768, 512), (1280, 720), (1280, 720)]);
    for (i, (path, w, h)) in frames.iter().enumerate() {
        assert_eq!(*path, dir.join(format!("{i:06}.png")));
        let bytes = std::fs::read(path).unwrap();
        let f = facts_of(&bytes).expect("a PNG header");
        assert_eq!((f.width, f.height), (*w, *h), "frame {i}: the header");
        let img = decode_png(&bytes).unwrap();
        assert_eq!((img.width, img.height), (*w, *h), "frame {i}: the decoded picture");
        assert!(img.rgba == native[i].2, "adopt frame {i}: the decoder's own picture, lossless");
    }
    let mut sizes: Vec<(u32, u32)> = frames.iter().map(|f| (f.1, f.2)).collect();
    sizes.dedup();
    assert_eq!(sizes.len(), 2, "two sizes under Adopt");
    let lines = nlesizeswitch_lines("adopted");
    assert_eq!(lines.len(), 1, "{lines:?}");

    for p in [SizePolicy::Letterbox, SizePolicy::Fill, SizePolicy::Stretch] {
        let set = tl.set_size_policy(SourceId(0), p).unwrap();
        let dir = root.join(p.name());
        let (r, frames) = export_png(&set, 0..4, &src, &dir, AudioOut::None, Container::Mp4).unwrap();
        assert_eq!(r.frames, 4, "{p:?}");
        assert!(frames.iter().all(|f| (f.1, f.2) == (768, 512)), "{p:?}: one size");
        for (path, _, _) in &frames {
            let img = decode_png(&std::fs::read(path).unwrap()).unwrap();
            assert_eq!((img.width, img.height), (768, 512), "{p:?}: {}", path.display());
        }
    }

    // Kept for the tool's run: the reel and its Adopt EDL (`nle export <edl> --src <reel>=two.mp4 --out <dir> --png`).
    std::fs::write(root.join("two.mp4"), &file).unwrap();
    std::fs::write(root.join("two.edl"), edl::write(&adopt, "TWO").unwrap()).unwrap();
    println!("adoptpng: adopt 768x512 x2 + 1280x720 x2 (sizes=2), letterbox/fill/stretch 768x512 x4; reel={} kept in {}", adopt.sources[0].reel, root.display());
}

#[test]
fn adoptpng_refused_dir_by_name() {
    use stria::nle_export::{AudioOut, Container, ExportError, export_png};
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 4);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 4, 0)];
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 4, 25)).unwrap();
    // A path under a regular file: no directory can be made there, root or not (a mode bit would not stop root).
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("adoptpng-refused");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("plain");
    std::fs::write(&file, b"not a directory").unwrap();
    let dir = file.join("frames");
    let e = export_png(&tl, 0..4, &src, &dir, AudioOut::None, Container::Mp4).unwrap_err();
    assert!(matches!(e, ExportError::Io(_)), "{e}");
    let text = e.to_string();
    assert!(text.starts_with(&format!("io: {}: ", dir.display())), "the refusal names the directory: {text}");
    assert_eq!(std::fs::read(&file).unwrap(), b"not a directory", "nothing written");
    println!("adoptpng: refused {text}");
}
