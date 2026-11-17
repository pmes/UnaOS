// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE1 (SR194) M3: the schedule and the render agree frame for frame and sample for sample, over two synthetic
//! sources through the real demuxer and decoders (utp1 test pattern + PCM16 in MP4).

use gneiss_pal::dsp::video::TestPattern;
use stria::nle::{RenderSink, SYNTH_RATE, Sources, render, synth_sample, synth_source};
use timeline_core::schedule::schedule;
use timeline_core::{Clip, Kind, SourceId, Timeline, edl};
mod common; use common::*; // NLE6-UTP1 (LEDGER SR729): tests/common, the ONE fixture / log / feed helper set the four binaries share

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

fn agree(tl: &Timeline) {
    let src = sources(tl); // NLE6-UTP1 (SR729): tests/common's NLE1 reels
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
    let want = vec![Clip { group: 1, ..Clip::new(SourceId(0), 25, 35, 0) }, Clip { group: 2, ..Clip::new(SourceId(0), 5, 15, 10) }]; // STRIALINKAUTO (SR911): each V1 / A1 pair linked, one group per pair
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
    let (two, _) = Sources::from_media("AX", &editmap_file(0x2_0000), 25).unwrap().unwrap(); let fast = vec![Clip::new(SourceId(0), 25, 45, 0).at_rate(0x2_0000), Clip::new(SourceId(0), 5, 25, 10).at_rate(0x2_0000)]; assert_eq!((&two.tracks[0].clips, &two.tracks[1].clips), (&fast, &fast)); // STRIACLIPSPEED (SR829): a 2x edit list now LAYS 2x clips (was EditError::Rate)
    let edl_text = edl::write(&timeline(), "EDITMAP").unwrap();
    assert!(Sources::from_media("AX", edl_text.as_bytes(), 25).unwrap().is_none());
    // A file without an edit list is the whole source.
    let (tl, src) = Sources::from_media("AX", &synth_source(0, 50, 25), 25).unwrap().unwrap();
    assert_eq!(tl.tracks[0].clips, vec![Clip { group: 1, ..Clip::new(SourceId(0), 0, 50, 0) }]); // STRIALINKAUTO (SR911): linked to its A1 clip
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

/// The run's log lines, captured per thread: tests/common's `log_lines` (NLE6-UTP1 SR729; tests/media.rs's HEVCPAL-SWAP shape).
use common::log_lines as nlesizeswitch_lines;

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
    let dir = common::fresh_dir("nle", "nlesizeswitch"); // STRIA-TEST-TMPDIR-REST (SR739): a per-process subdir, created empty
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
    let dir = common::fresh_dir("nle", "nlesizepolicy"); // STRIA-TEST-TMPDIR-REST (SR739): a per-process subdir, created empty
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
    let root = common::scratch_dir("nle", "adoptpng"); // STRIA-TEST-TMPDIR-REST (SR739): a per-process subdir, removed, not created

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
    let root = common::fresh_dir("nle", "adoptpng-refused"); // STRIA-TEST-TMPDIR-REST (SR739): a per-process subdir, created empty
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

// ── NLEHOSTSAVE (LEDGER SR677): the host face saves through timeline_core's ONE writer (handlers.md §5 NLEPROBE-HDR) ──────
// `save` (verb, button, Ctrl+s / Meta+s) writes `edl::write(timeline, <file stem>)` to the path the face was opened from;
// `dirty()` is the timeline differing from the one opened / saved; no path is a refusal by name.

fn nlehostsave_dir(name: &str) -> std::path::PathBuf {
    common::fresh_dir("nlehostsave", name) // STRIA-TEST-TMPDIR (SR729): a per-process subdir
}

#[test]
fn nlehostsave_round_trip_through_the_one_writer() {
    let (tl0, src) = synth_fixture();
    let path = nlehostsave_dir("round").join("cut.edl");
    std::fs::write(&path, edl::write(&tl0, "cut").unwrap()).unwrap();
    let opened = edl::parse(&std::fs::read_to_string(&path).unwrap(), 25).unwrap();
    assert_eq!(opened, tl0, "the fixture round-trips before any edit");
    let mut face = Face::new(EditorState::new(opened), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(path.as_path()));
    assert_eq!(face.doc_path(), Some(path.as_path()));
    assert!(!face.dirty(), "clean at open");
    face.state.select_at(0, 12).unwrap();
    face.state.seek(15);
    assert!(face.verb("split"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl0.split(0, 15).unwrap());
    assert!(face.dirty(), "dirty after split");
    assert!(face.verb("save"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(!face.dirty(), "clean after save");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("TITLE: cut\n"), "titled by the file stem: {text}");
    assert_eq!(text, edl::write(&face.state.timeline, "cut").unwrap());
    assert_eq!(edl::parse(&text, 25).unwrap(), face.state.timeline);
    assert!(face.verb("undo"));
    assert!(face.dirty(), "an undo past the save is dirty");
    assert!(face.verb("redo"));
    assert!(!face.dirty(), "back at the saved timeline reads clean");
    println!("nlehostsave: saved {} bytes to {}", text.len(), path.display());
}

#[test]
fn nlehostsave_refuses_without_a_document_path() {
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    assert!(VERBS.contains(&"save"));
    assert!(face.doc_path().is_none());
    assert!(face.verb("save"), "a verb the face takes");
    assert_eq!(face.last_error.take().as_deref(), Some("save: no document path"));
    assert_eq!(face.state.timeline, tl0);
    assert_eq!(face.save(), Err("no document path".to_string()));
}

#[test]
fn nlehostsave_ctrl_s_and_meta_s_save() {
    let (tl0, src) = synth_fixture();
    let dir = nlehostsave_dir("keys");
    for chord in ["Ctrl+s", "Meta+s"] {
        let path = dir.join(format!("{}.edl", chord.replace('+', "-")));
        let (_, src2) = synth_fixture();
        let mut face = Face::new(EditorState::new(tl0.clone()), src2, (320, 160), PixelOrder::Rgba, false).with_doc(Some(path.as_path()));
        face.state.select_at(0, 12).unwrap();
        face.state.seek(15);
        assert!(face.verb("split"));
        assert!(face.dirty());
        assert!(face.on(&bandy::SMessage::BrowserKey(chord.to_string())), "{chord}");
        assert!(face.last_error.is_none(), "{chord}: {:?}", face.last_error);
        assert!(!face.dirty(), "{chord} saved");
        assert_eq!(edl::parse(&std::fs::read_to_string(&path).unwrap(), 25).unwrap(), face.state.timeline);
    }
    drop(src);
}

/// NLEXFADEPOWER (SR678): an audio-only crossfade (no video transition) A[0,20)@0 -> B[0,20)@20 over 8 ticks. Around its
/// midpoint (samples `24 PER +- 96`) the rendered power under EqualPower is twice the Linear one (the weights sin(pi/4)
/// against 1/2, both sources at their own level), within 1%; the per-tick equal-power runs CONTINUE their decoders (the
/// same seeks as the linear span); and the explicit linear crossfade renders sample for sample as the one a video
/// dissolve implies (NLE3b).
#[test]
fn nlexfadepower_equal_power_doubles_the_midpoint_power() {
    use timeline_core::{Curve, TransitionKind};
    let base = |video: bool| {
        let mut tl = Timeline::new(25);
        let (a, b) = (tl.add_source("A", 50), tl.add_source("B", 50));
        if video {
            tl.add_track(Kind::Video);
        }
        let ti = tl.add_track(Kind::Audio);
        tl.tracks[ti].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20)];
        if video {
            tl.tracks[0].clips = tl.tracks[ti].clips.clone();
        }
        tl
    };
    let run = |tl: &Timeline| {
        let mut src = Sources::new(tl);
        for (i, b) in BASE.iter().enumerate() {
            src.add(SourceId(i as u32), synth_source(*b, 50, 25)).unwrap();
        }
        let mut sink = Collect::default();
        let r = render(tl, 0..40, &src, &mut sink).unwrap();
        assert_eq!(sink.pcm.len(), 40 * PER);
        (r.seeks, sink.pcm.iter().map(|x| (*x * 32768.0).round() as i64).collect::<Vec<i64>>())
    };
    let (eq_seeks, eq) = run(&base(false).add_audio_xfade(0, 20, 8, Curve::EqualPower).unwrap());
    let (lin_seeks, lin) = run(&base(false).add_audio_xfade(0, 20, 8, Curve::Linear).unwrap());
    let mid = 24 * PER;
    let power = |s: &[i64]| s[mid - 96..mid + 96].iter().map(|&x| (x * x) as f64).sum::<f64>();
    let ratio = power(&eq) / power(&lin);
    assert!((ratio - 2.0).abs() <= 0.02, "equal-power / linear midpoint power {ratio}");
    // A, A's handle (the under layer's own seek), B: the per-tick runs reopen nothing
    assert_eq!((eq_seeks, lin_seeks), (3, 3));
    // outside the span the two curves are the same cut
    assert_eq!((&eq[..20 * PER], &eq[28 * PER..]), (&lin[..20 * PER], &lin[28 * PER..]));
    // the explicit linear crossfade is the derived one's sound
    let (_, derived) = run(&base(true).add_transition(0, 20, TransitionKind::Dissolve, 8).unwrap());
    assert_eq!(derived, lin);
    println!("nlexfadepower: midpoint power eq/lin={ratio:.4} seeks={eq_seeks}");
}

// ── NLERAWEXPORT (LEDGER SR689): the raw planar exports on SR394's two-size reel (handlers.md §5 NLEPROBE-HDR's RAW-EXPORT) ──
// `video.json` states ONE size, so under Adopt both raw planar forms refuse the second size by the converter crate's own
// name, as the H.264 export does; under Letterbox the y4m stream's header states the reel's first size at 25 fps.

#[test]
fn nlerawexport_adopt_reel_refused_raw_planar() {
    use stria::nle_export::{AudioOut, ExportError, I420Raw, VideoEncoder, Y4m, export};
    use timeline_core::SizePolicy;
    let _ = nlesizeswitch_lines("");
    let Some(file) = nlesizeswitch_reel() else { return };
    let (tl, src) = Sources::from_media("two", &file, 25).unwrap().expect("an MP4");
    let root = common::scratch_dir("nle", "nlerawexport"); // STRIA-TEST-TMPDIR-REST (SR739): a per-process subdir, removed, not created

    let adopt = tl.set_size_policy(SourceId(0), SizePolicy::Adopt).unwrap();
    let encs: [(&str, Box<dyn VideoEncoder>); 2] = [("i420", Box::new(I420Raw::default())), ("y4m", Box::new(Y4m::new(25, 1)))];
    for (name, enc) in encs {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let e = export(&adopt, 0..4, &src, &dir, enc, AudioOut::None).unwrap_err();
        assert!(matches!(e, ExportError::H264(h264enc_core::Error::FrameSize)), "{name}: {e}");
        assert!(e.to_string().contains("frame does not match"), "{name}: {e}");
    }

    let letterbox = tl.set_size_policy(SourceId(0), SizePolicy::Letterbox).unwrap();
    let dir = root.join("letterbox");
    std::fs::create_dir_all(&dir).unwrap();
    let enc = Y4m::new(letterbox.tb.den as u32, letterbox.tb.num as u32);
    let r = export(&letterbox, 0..4, &src, &dir, Box::new(enc), AudioOut::None).unwrap();
    assert_eq!(r.frames, 4);
    let y4m = std::fs::read(dir.join("video.y4m")).unwrap();
    let head: &[u8] = b"YUV4MPEG2 W768 H512 F25:1 Ip A1:1 C420jpeg\n";
    assert!(y4m.starts_with(head), "{:?}", String::from_utf8_lossy(&y4m[..y4m.len().min(64)]));
    let per = 768 * 512 * 3 / 2;
    assert_eq!(y4m.len(), head.len() + 4 * (6 + per));
    for i in 0..4 {
        let at = head.len() + i * (6 + per);
        assert_eq!(&y4m[at..at + 6], b"FRAME\n", "frame {i}: the FRAME marker");
    }
    println!("nlerawexport: adopt refused i420 + y4m (frame does not match); letterbox y4m 768x512 F25:1 x4 in {}", root.display());
}

// ---- NLE3ETRIMSTRETCH (LEDGER SR692): the face's drag-trim on a transitioned cut stretches the transition (named tail block) ----

#[test]
fn nle3etrimstretch_face_drag_trim_shrinks_the_dissolve() {
    use bandy::SMessage;
    use timeline_core::TransitionKind;
    let (tl, src) = synth_fixture();
    let tl0 = tl.add_transition(0, 10, TransitionKind::Dissolve, 4).unwrap();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(0, 1).unwrap();
    // B[0,10)@10 on V1: its out handle 156..160 on row 0 (y 80..120) at zoom 8; drag it 7 ticks back (x 158 -> 102)
    let btn = |down, x| SMessage::BrowserButton { down, x, y: 100.0, mods: 0, surface: 0 };
    face.on(&btn(true, 158.0));
    face.on(&SMessage::BrowserMouseMove(102.0, 100.0, 0));
    face.on(&btn(false, 102.0));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    // B is 3 ticks, shorter than the 4-tick dissolve: the dissolve shrinks to 3 (was refused, Handle), A1's crossfade with it
    let tl = &face.state.timeline;
    assert_eq!(*tl, tl0.trim_out(0, 1, 3).unwrap());
    assert_eq!((tl.tracks[0].clips[1].len(), tl.tracks[0].transitions[0].at, tl.tracks[0].transitions[0].dur), (3, 10, 3));
    assert_eq!(tl.audio_crossfade(1, 1).map(|x| (x.at, x.dur)), Some((10, 3)));
}

// ---- NLEFACEEXPORT (LEDGER SR694): the editor face's export verb, its progress seam, busy, Esc and the band (named tail block) ----

fn nlefaceexport_dir(name: &str) -> std::path::PathBuf {
    common::scratch_dir("nlefaceexport", name) // STRIA-TEST-TMPDIR (SR729): a per-process subdir, not created
}

fn nlefaceexport_face(tl: Timeline, src: Sources) -> Face {
    Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
}

/// Wait (bounded) until the paused job's reading is `want`.
fn nlefaceexport_until(face: &Face, want: (u64, u64)) {
    let t0 = std::time::Instant::now();
    while face.export_progress() != Some(want) {
        assert!(t0.elapsed() < std::time::Duration::from_secs(60), "reading {:?}, want {want:?}", face.export_progress());
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn nlefaceexport_png_completes_with_its_message() {
    let dir = nlefaceexport_dir("png");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png", dir.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.exporting());
    assert!(face.wait_export());
    assert!(!face.exporting() && face.export_progress().is_none());
    let line = face.last_error.take().expect("the message line");
    assert!(line.starts_with("export: 35 frames, ") && nle5dexportmessageduration_unwalled(&line).ends_with(&format!(" bytes, png + wav -> {}", dir.display())), "{line}"); // NLE5DEXPORTMESSAGEAUDIO (SR756): the words moved the pin
    for n in 0..35 {
        assert!(dir.join(format!("{n:06}.png")).is_file(), "frame {n}");
    }
    assert!(!dir.join("000035.png").exists());
    println!("nlefaceexport: {line}");
}

#[test]
fn nlefaceexport_progress_is_monotone_and_the_seam_is_byte_identical() {
    use stria::nle_export::{AudioOut, Container, PngSequence, export_as, export_as_with};
    let (tl, src) = synth_fixture();
    let (a, b) = (nlefaceexport_dir("seam-plain"), nlefaceexport_dir("seam-ticked"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let plain = export_as(&tl, 0..tl.end(), &src, &a, Box::new(PngSequence), AudioOut::Wav, Container::Mp4).unwrap();
    let mut seen = Vec::new();
    let ticked = export_as_with(&tl, 0..tl.end(), &src, &b, Box::new(PngSequence), AudioOut::Wav, Container::Mp4, &mut |n, t| {
        seen.push((n, t));
        Ok(())
    })
    .unwrap();
    assert_eq!(plain, ticked, "the progress seam changes no byte");
    assert_eq!(seen, (1..=35).map(|n| (n, 35)).collect::<Vec<_>>(), "one reading per frame, total the range");

    let dir = nlefaceexport_dir("poll");
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png", dir.display())));
    let (mut last, mut polls) = ((0u64, 0u64), 0u32);
    while face.exporting() {
        if let Some(p) = face.export_progress() {
            assert!(p.0 >= last.0 && p.1 >= last.1 && p.0 <= 35 && (p.1 == 0 || p.1 == 35), "{p:?} after {last:?}");
            last = p;
            polls += 1;
        }
        face.export_tick();
        std::thread::yield_now();
    }
    assert!(face.last_error.take().is_some_and(|l| l.starts_with("export: 35 frames, ")));
    println!("nlefaceexport: seam {} frames fnv {:016x} == export_as; {polls} polls monotone to {last:?}", ticked.frames, ticked.fnv);
}

#[test]
fn nlefaceexport_busy_then_esc_cancels_with_the_partial_files_named() {
    let (dir, other) = (nlefaceexport_dir("busy"), nlefaceexport_dir("busy-other"));
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    assert!(face.verb(&format!("export {} png", other.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (1 waiting)")); // STRIAEXPORTQUEUE (SR881): a running export queues the request; Esc below drops it
    assert!(!other.exists(), "a busy refusal makes nothing");
    assert!(face.on(&bandy::SMessage::BrowserKey("Escape".into())), "Esc cancels the running export");
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the cancel line");
    assert_eq!(line, format!("export: cancelled at 3/35 frames; partial files left in {}", dir.display()));
    for n in 0..3 {
        assert!(dir.join(format!("{n:06}.png")).is_file(), "partial frame {n}");
    }
    assert!(!dir.join("000003.png").exists(), "nothing encoded after the cancel");
    assert!(!face.on(&bandy::SMessage::BrowserKey("Escape".into())), "Esc with nothing running takes nothing");
    println!("nlefaceexport: {line}");
}

#[test]
fn nlefaceexport_refusals_are_named() {
    let root = nlefaceexport_dir("refused");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("a-file");
    std::fs::write(&file, b"x").unwrap();
    let bad = file.join("sub");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png", bad.display())));
    let line = face.last_error.take().unwrap();
    assert!(line.starts_with(&format!("export: {}: ", bad.display())), "{line}");
    assert!(!face.exporting());
    assert!(face.verb("export"));
    assert_eq!(face.last_error.take().as_deref(), Some("export: no directory (no document path)"));
    assert!(face.verb(&format!("export {} gif", root.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: unknown format gif (h264|png|i420|y4m|raw)"));
    assert!(!face.exporting());
    println!("nlefaceexport: refused {line}");
}

#[test]
fn nlefaceexport_band_paints_only_while_running() {
    let dir = nlefaceexport_dir("band");
    let (mut tl, src) = synth_fixture();
    for t in &mut tl.tracks {
        t.clips.truncate(2); // A[13,23)@0, B[0,10)@10: 20 ticks, so frame 10 is half
    }
    let mut face = nlefaceexport_face(tl, src);
    let idle = face.paint();
    let l = face.layout();
    face.export_paused_at(&dir, "png", 10).unwrap();
    nlefaceexport_until(&face, (10, 20));
    let shown = face.paint();
    let band = (l.tracks_y - stria::nle_face::EXPORT_BAND_H)..l.tracks_y;
    let mut lit = 0;
    for y in 0..l.h {
        for x in 0..l.w {
            let i = ((y * l.w + x) * 4) as usize;
            if band.contains(&y) && x < l.w / 2 {
                assert_eq!(&shown[i..i + 4], &timeline_core::view::GUIDE, "band pixel ({x},{y})");
                lit += 1;
            } else {
                assert_eq!(&shown[i..i + 4], &idle[i..i + 4], "pixel ({x},{y}) outside the band");
            }
        }
    }
    assert_eq!(lit, l.w / 2 * stria::nle_face::EXPORT_BAND_H);
    assert!(face.cancel_export() && face.wait_export());
    assert!(face.last_error.take().is_some_and(|e| e.starts_with("export: cancelled at 10/20 frames")));
    assert_eq!(stria::nle_face::fnv1a(&face.paint()), stria::nle_face::fnv1a(&idle), "no band once the export is reported");
    println!("nlefaceexport: band {lit} px at 10/20 rows {band:?}");
}

// ---- NLE5DEXPORTSTOP (LEDGER SR705): a sink's stop query ends `render` at that step, so a cancel ends the decode (named tail block) ----

/// Counts what `render` hands it; `stop` turns true once `after` frames have arrived (`None` = never: the default).
struct Nle5dStopSink {
    after: Option<usize>,
    frames: Vec<i64>,
    pcm: Vec<i64>,
}
impl RenderSink for Nle5dStopSink {
    fn frame(&mut self, out_t: i64, _w: u32, _h: u32, _rgba: &[u8]) {
        self.frames.push(out_t);
    }
    fn pcm(&mut self, out_t: i64, _rate: u32, _ch: u16, _s: &[f32]) {
        self.pcm.push(out_t);
    }
    fn stop(&self) -> bool {
        self.after.is_some_and(|n| self.frames.len() >= n)
    }
}

#[test]
fn nle5dexportstop_render_ends_at_the_step_the_sink_stops() {
    let (tl, src) = synth_fixture();
    let steps = schedule(&tl, 0..tl.end(), &src).unwrap();
    let mut all = Nle5dStopSink { after: None, frames: Vec::new(), pcm: Vec::new() };
    let full = render(&tl, 0..tl.end(), &src, &mut all).unwrap();
    assert_eq!((all.frames.len(), full.frames, full.stopped), (35, 35, None), "the default sink renders the whole range");

    // The step after the 3rd video step is where the stop is read true (steps interleave video and audio).
    let third = steps.iter().enumerate().filter(|(_, s)| s.kind == Kind::Video).nth(2).map(|(i, _)| i).unwrap();
    let at = steps[third + 1].out_t;
    let audio_before = steps[..=third].iter().filter(|s| s.kind == Kind::Audio).count();
    let mut sink = Nle5dStopSink { after: Some(3), frames: Vec::new(), pcm: Vec::new() };
    let r = render(&tl, 0..tl.end(), &src, &mut sink).unwrap();
    assert_eq!(sink.frames, all.frames[..3].to_vec(), "exactly three frame calls, the first three");
    assert_eq!(r.frames, 3);
    assert_eq!(r.stopped, Some(at), "the report names the step it stopped at");
    assert_eq!(sink.pcm.len(), audio_before, "no audio step past the stop");
    assert!(r.frames < full.frames && r.pcm_frames <= full.pcm_frames);
    println!("nle5dexportstop: render stopped at out_t={at} after 3/35 frames ({} audio steps)", sink.pcm.len());
}

#[test]
fn nle5dexportstop_cancelled_export_stops_the_render() {
    use stria::nle_export::{AudioOut, Container, ExportError, PngSequence, export_as_with};
    // The door: progress refuses at frame 3 -> Ticked latches it -> render stops at the next step.
    // (The 35-tick synth fixture decodes in milliseconds, so the 2 s bound is a guard; the step count above is the pin.)
    let (tl, src) = synth_fixture();
    let dir = nlefaceexport_dir("stop-door");
    std::fs::create_dir_all(&dir).unwrap();
    let mut calls = 0u64;
    let t0 = std::time::Instant::now();
    let r = export_as_with(&tl, 0..tl.end(), &src, &dir, Box::new(PngSequence), AudioOut::Wav, Container::Mp4, &mut |n, _| {
        calls += 1;
        if n == 3 { Err(ExportError::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"))) } else { Ok(()) }
    });
    let ms = t0.elapsed().as_millis();
    assert!(matches!(&r, Err(ExportError::Io(e)) if e.kind() == std::io::ErrorKind::Interrupted), "{r:?}");
    assert_eq!(calls, 3);
    for n in 0..3 {
        assert!(dir.join(format!("{n:06}.png")).is_file(), "partial frame {n}");
    }
    assert!(!dir.join("000003.png").exists());
    assert!(ms < 2000, "the cancelled export returned in {ms} ms");

    // The face: Esc's cancel at 3/35, joined within the bound, SR694's line unchanged.
    let fdir = nlefaceexport_dir("stop-face");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.export_paused_at(&fdir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    let t1 = std::time::Instant::now();
    assert!(face.cancel_export() && face.wait_export());
    let joined = t1.elapsed().as_millis();
    assert!(joined < 2000, "joined in {joined} ms");
    assert_eq!(face.last_error.take(), Some(format!("export: cancelled at 3/35 frames; partial files left in {}", fdir.display())));
    println!("nle5dexportstop: export cancelled at 3 frames, joined in {joined} ms (door {ms} ms)");
}

// ---- STRIAINFOREELSTRIP (LEDGER SR747): the face's message in a row of its own (named tail block) ----

/// Rule: a labelled face paints its message inside the Layout's message row (`with_message`: 14 px cut from the preview band's
/// foot, the ruler and tracks unmoved), the row is the view's BG with no message, and the preview rows above it carry no label
/// ink (equal selected and deselected); the face's pointer geometry (`Face::layout`) carries no row.
#[test]
fn striainforeelstrip_face_message_has_a_row_of_its_own() {
    use stria::nle_face::{EditorState, Face, Layout, MESSAGE_H, PixelOrder};
    use timeline_core::view::BG;
    let (tl, src) = stria::nle_face::synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 144), PixelOrder::Rgba, true);
    let l: Layout = face.layout();
    assert_eq!((l.preview_h, l.message_h), (48, 0), "the pointer geometry has no row");
    let m = l.with_message(true);
    assert_eq!((m.preview_h, m.message_y, m.message_h, m.ruler_y, m.tracks_y), (34, 34, MESSAGE_H, l.ruler_y, l.tracks_y));
    let band = |px: &[u8], y0: u32, y1: u32| -> Vec<[u8; 4]> {
        let mut out = Vec::new();
        for y in y0..y1 {
            for x in 0..64u32 {
                let i = ((y * m.w + x) * 4) as usize;
                out.push([px[i], px[i + 1], px[i + 2], px[i + 3]]);
            }
        }
        out
    };
    let bare = face.paint();
    assert!(band(&bare, m.message_y, m.message_y + m.message_h).iter().all(|&p| p == BG), "no message: the row is BG");
    face.state.select(0, 0).unwrap();
    let sel = face.paint();
    assert!(band(&sel, m.message_y, m.message_y + m.message_h).iter().any(|&p| p != BG), "the reel's line inks the row");
    assert_eq!(band(&sel, 0, m.message_y), band(&bare, 0, m.message_y), "no label ink in the preview rows above the row");
    println!("striainforeelstrip: face row={}+{} ruler_y={} preview_h={}", m.message_y, m.message_h, m.ruler_y, m.preview_h);
}

// ---- NLE5DEXPORTRANGE (LEDGER SR717): the face's in / out marks, the range `export` takes, their ruler marks (named tail block) ----

/// The SR694 face over NLE1's fixture with the marks set at `a` and `b` through the verbs (the playhead back at 0).
fn nle5dexportrange_face(a: i64, b: i64) -> Face {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.seek(a);
    assert!(face.verb("in"));
    face.state.seek(b);
    assert!(face.verb("out"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    face.state.seek(0);
    face
}

#[test]
fn nle5dexportrange_marked_png_writes_the_range() {
    use stria::nle_export::{AudioOut, Container, PngSequence, export_as};
    let (dir, by_hand) = (nlefaceexport_dir("range"), nlefaceexport_dir("range-by-hand"));
    let mut face = nle5dexportrange_face(5, 15);
    assert_eq!((face.marks(), face.export_range()), ((Some(5), Some(15)), Some(5..15)));
    assert!(face.verb(&format!("export {} png", dir.display())));
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the message line");
    assert!(line.starts_with("export: 10 frames ticks 5..15, ") && nle5dexportmessageduration_unwalled(&line).ends_with(&format!(" bytes, png + wav -> {}", dir.display())), "{line}"); // NLE5DEXPORTMESSAGEAUDIO (SR756)
    for n in 0..10 {
        assert!(dir.join(format!("{n:06}.png")).is_file(), "frame {n}");
    }
    assert!(!dir.join("000010.png").exists());
    let (tl, src) = synth_fixture();
    std::fs::create_dir_all(&by_hand).unwrap();
    export_as(&tl, 5..15, &src, &by_hand, Box::new(PngSequence), AudioOut::Wav, Container::Mp4).unwrap();
    for n in [0, 9] {
        let f = format!("{n:06}.png");
        assert_eq!(std::fs::read(dir.join(&f)).unwrap(), std::fs::read(by_hand.join(&f)).unwrap(), "{f} is export_as(5..15)'s");
    }
    println!("nle5dexportrange: {line}");
}

#[test]
fn nle5dexportrange_marks_refuse_by_name_and_clear() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.seek(15);
    assert!(face.verb("out"));
    face.state.seek(20);
    assert!(face.verb("in"));
    assert_eq!(face.last_error.take().as_deref(), Some("in: tick 20 is not before the out mark 15"));
    assert_eq!(face.marks(), (None, Some(15)), "a refused in leaves the marks");
    face.state.seek(5);
    assert!(face.verb("in"));
    assert_eq!((face.marks(), face.last_error.take()), ((Some(5), Some(15)), None));
    face.state.seek(3);
    assert!(face.verb("out"));
    assert_eq!(face.last_error.take().as_deref(), Some("out: tick 3 is not after the in mark 5"));
    assert_eq!(face.marks(), (Some(5), Some(15)));
    face.state.seek(5);
    assert!(face.verb("in"));
    assert_eq!((face.marks(), face.export_range()), ((None, Some(15)), None), "in again at its tick clears it");
    face.state.seek(15);
    assert!(face.verb("out"));
    assert_eq!(face.marks(), (None, None), "out again at its tick clears it");
    assert!(face.last_error.is_none());
    println!("nle5dexportrange: refused in 20 past out 15, out 3 before in 5; cleared both");
}

#[test]
fn nle5dexportrange_all_forces_the_whole_timeline() {
    let dir = nlefaceexport_dir("range-all");
    let mut face = nle5dexportrange_face(5, 15);
    assert!(face.verb(&format!("export --all {} png", dir.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the message line");
    assert!(line.starts_with("export: 35 frames, ") && !line.contains(" ticks "), "{line}");
    assert!(dir.join("000034.png").is_file() && !dir.join("000035.png").exists());
    assert_eq!(face.marks(), (Some(5), Some(15)), "--all leaves the marks");
    println!("nle5dexportrange: --all {line}");
}

#[test]
fn nle5dexportrange_marks_paint_two_ruler_columns() {
    let (tl, src) = synth_fixture();
    let mut idle = nlefaceexport_face(tl, src);
    let mut face = nle5dexportrange_face(5, 15);
    let (a, b) = (idle.paint(), face.paint());
    let l = face.layout();
    let cols = [l.x_of(5) as u32, l.x_of(15) as u32];
    let mut lit = 0;
    for y in 0..l.h {
        for x in 0..l.w {
            let i = ((y * l.w + x) * 4) as usize;
            if (l.ruler_y..l.tracks_y).contains(&y) && cols.contains(&x) {
                assert_eq!(&b[i..i + 4], &timeline_core::view::GUIDE, "mark pixel ({x},{y})");
                lit += 1;
            } else {
                assert_eq!(&b[i..i + 4], &a[i..i + 4], "pixel ({x},{y}) outside the marks");
            }
        }
    }
    assert_eq!(lit, 2 * stria::nle_face::RULER_H);
    println!("nle5dexportrange: marks {lit} px at columns {cols:?} rows {}..{}", l.ruler_y, l.tracks_y);
}

#[test]
fn nle5dexportrange_shortened_timeline_clamps_or_refuses() {
    let dir = nlefaceexport_dir("range-clamp");
    let mut face = nle5dexportrange_face(5, 15);
    for t in &mut face.state.timeline.tracks {
        t.clips.truncate(1); // A[13,23)@0: the timeline now ends at 10
    }
    assert_eq!(face.export_range(), Some(5..10));
    assert!(face.verb(&format!("export {} png", dir.display())));
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the message line");
    assert!(line.starts_with("export: 5 frames ticks 5..10, "), "{line}");

    let past = nlefaceexport_dir("range-past");
    let mut face = nle5dexportrange_face(12, 15);
    for t in &mut face.state.timeline.tracks {
        t.clips.truncate(1);
    }
    assert!(face.verb(&format!("export {} png", past.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: the marks 12..15 are past the timeline's end 10"));
    assert!(!face.exporting() && !past.exists(), "a refused range makes nothing");
    println!("nle5dexportrange: clamped {line}");
}

// ---- NLE5DEXPORTAUDIOOPTIONS (LEDGER SR722): the face's export takes an audio word through the one audio table (named tail block) ----

#[test]
fn nle5dexportaudiooptions_png_none_writes_no_audio() {
    let (wav, none) = (nlefaceexport_dir("audio-default"), nlefaceexport_dir("audio-none"));
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png", wav.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    assert!(face.last_error.take().is_some_and(|l| l.starts_with("export: 35 frames, ")));
    assert!(wav.join("audio.wav").is_file(), "the default audio stays wav");
    assert!(face.verb(&format!("export {} png none", none.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the message line");
    assert!(line.starts_with("export: 35 frames, ") && nle5dexportmessageduration_unwalled(&line).ends_with(&format!(" bytes, png, no audio -> {}", none.display())), "{line}"); // NLE5DEXPORTMESSAGEAUDIO (SR756)
    for n in [0, 34] {
        let f = format!("{n:06}.png");
        assert_eq!(std::fs::read(none.join(&f)).unwrap(), std::fs::read(wav.join(&f)).unwrap(), "{f}: the frames do not move");
    }
    assert!(!none.join("000035.png").exists());
    assert!(!none.join("audio.wav").exists() && !none.join("audio.mp4").exists(), "none writes no audio file");
    println!("nle5dexportaudiooptions: none {line}");
}

#[test]
fn nle5dexportaudiooptions_png_aac_writes_audio_mp4() {
    use gneiss_pal::dsp::demux::{Codec, Demuxer, TrackKind};
    let dir = nlefaceexport_dir("audio-aac");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png aac", dir.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the message line");
    assert!(line.starts_with("export: 35 frames, "), "{line}");
    assert!(dir.join("000034.png").is_file() && !dir.join("audio.wav").exists(), "{line}");
    let d = Demuxer::open(std::fs::read(dir.join("audio.mp4")).unwrap()).unwrap();
    assert_eq!(d.tracks().len(), 1);
    let t = &d.tracks()[0];
    assert_eq!((t.kind, t.codec.clone()), (TrackKind::Audio, Codec::Aac), "one AAC-LC track");
    println!("nle5dexportaudiooptions: aac {line}");
}

#[test]
fn nle5dexportaudiooptions_unknown_audio_refused_by_name() {
    let root = nlefaceexport_dir("audio-refused");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png flac", root.display())));
    assert!(face.last_error.take().is_some_and(|e| e.starts_with("export: unknown audio flac; the words are export "))); // NLE5DEXPORTMESSAGEAUDIO (SR756)
    assert!(face.verb(&format!("export {} png aac:x", root.display())));
    assert!(face.last_error.take().is_some_and(|e| e.starts_with("export: aac rate \"x\": aac:KBPS takes a whole number of kb/s above 0; the words are export "))); // NLE5DEXPORTMESSAGEAUDIO (SR756); STRIAEXPORTAACBITRATE (SR887): a bad rate refused by name
    assert!(!face.exporting() && !root.exists(), "a refused audio word makes nothing");
    println!("nle5dexportaudiooptions: refused flac and aac:x by name");
}

// ---- NLE5DRANGESAVE (LEDGER SR723): the marks live on the Timeline: saved with the document, reopened, undone (named tail block) ----

#[test]
fn nle5drangesave_marks_save_and_reopen_through_the_document() {
    let (tl0, src) = synth_fixture();
    let path = nlehostsave_dir("marks").join("cut.edl");
    std::fs::write(&path, edl::write(&tl0, "cut").unwrap()).unwrap();
    let opened = edl::parse(&std::fs::read_to_string(&path).unwrap(), 25).unwrap();
    assert_eq!(opened.marks, (None, None));
    let mut face = Face::new(EditorState::new(opened), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(path.as_path()));
    face.state.seek(5);
    assert!(face.verb("in"));
    assert!(face.dirty(), "a mark dirties the document");
    face.state.seek(15);
    assert!(face.verb("out"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline.marks, (Some(5), Some(15)));
    assert!(face.verb("save"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(!face.dirty(), "clean after save");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\n* UNA MARK 5 15\n\n"), "the header's last line, before the first event: {text}");
    assert_eq!(text.matches("* UNA MARK").count(), 1);
    let (_, src2) = synth_fixture();
    let reopened = Face::new(EditorState::open(&text, 25).unwrap(), src2, (320, 160), PixelOrder::Rgba, false).with_doc(Some(path.as_path()));
    assert_eq!((reopened.marks(), reopened.export_range()), ((Some(5), Some(15)), Some(5..15)));
    assert_eq!(reopened.state.timeline, face.state.timeline);
    assert!(!reopened.dirty());
    println!("nle5drangesave: saved {} bytes, reopened marks {:?}", text.len(), reopened.marks());
}

#[test]
fn nle5drangesave_a_mark_is_one_undoable_commit() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.seek(5);
    assert!(face.verb("in"));
    assert_eq!((face.marks(), face.state.undo_depth()), ((Some(5), None), 1));
    face.state.seek(15);
    assert!(face.verb("out"));
    assert_eq!((face.marks(), face.state.undo_depth()), ((Some(5), Some(15)), 2));
    assert!(face.verb("undo"));
    assert_eq!(face.marks(), (Some(5), None), "undo clears the out mark");
    assert!(face.verb("undo"));
    assert_eq!(face.marks(), (None, None), "undo clears the in mark");
    assert_eq!(face.state.timeline, tl);
    assert!(face.verb("redo"));
    assert_eq!(face.marks(), (Some(5), None));
    // the editor's words (THE dispatch the kernel face calls), through the face's fall-through
    face.state.seek(15);
    assert!(face.verb("mark out"));
    assert_eq!((face.marks(), face.last_error.take()), ((Some(5), Some(15)), None));
    face.state.seek(2);
    assert!(face.verb("mark out"));
    assert_eq!(face.last_error.take().as_deref(), Some("mark out: the in mark is not before the out mark"));
    assert_eq!(face.marks(), (Some(5), Some(15)), "a refused mark leaves the marks");
    assert!(face.verb("mark clear"));
    assert_eq!((face.marks(), face.export_range()), ((None, None), None));
    println!("nle5drangesave: in / out / undo / redo / mark out / refused / clear");
}

// ---- NLESTANDINAUDIOEDITOR (LEDGER SR713): the editor's audio reel opens through media.rs's AudioSwap (named tail block) ----
/// SR703's fixture (tests/media.rs `nlestandinaudio_unknown_audio_codec_opens_on_the_silent_stand_in`, tests/common's `Fixture`, NLE6-UTP1 SR729):
/// a 64x48 test-pattern video beside PCM16 (every sample 1000) re-labelled `ac-3` — its rate and channels read, no decoder.
fn nlestandinaudioeditor_ac3_reel() -> Vec<u8> {
    Fixture::pattern(64, 48, 25, 50, 10).pcm16(SYNTH_RATE, 1, &vec![1000i16; 2 * SYNTH_RATE as usize]).relabel_audio(*b"ac-3").mp4()
}

/// Rule: a timeline citing a reel whose audio no decoder takes renders (no `RenderError::Decode`): that reel's run is the
/// silent stand-in (zeros where the source holds 1000), the other reel's samples pass bit for bit, the report counts ONE
/// stand-in reel, and media.rs's one `swapped to stand-in` line is the only audio-decode line. A normal timeline counts 0.
#[test]
fn nlestandinaudioeditor_an_ac3_reel_renders_on_the_silent_stand_in() {
    let _ = nlesizeswitch_lines("");
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    for t in 0..2 {
        tl.tracks[t].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10)];
    }
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    src.add(b, nlestandinaudioeditor_ac3_reel()).unwrap();
    let mut sink = Collect::default();
    let r = render(&tl, 0..20, &src, &mut sink).expect("an ac-3 reel renders on the silent stand-in");
    assert_eq!((r.frames, r.stand_in_audio), (20, 1), "{r:?}");
    assert_eq!(sink.pcm.len(), 20 * PER);
    for j in 0..10 * PER {
        assert_eq!(sink.pcm[j], q(synth_sample(0, j as u64) as i32), "reel A sample {j}");
    }
    assert!(sink.pcm[10 * PER..].iter().all(|&x| x == 0.0), "the ac-3 reel's run is silence");
    let lines = nlesizeswitch_lines("[STRIA] :: audio decode:");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("[STRIA] :: audio decode: swapped to stand-in: ") && lines[0].contains("ac-3"), "{}", lines[0]);
    let (normal, _) = mix(&[0], vec![vec![Clip::new(SourceId(0), 0, 10, 0)]], 10, 0);
    assert_eq!(normal.stand_in_audio, 0);
    println!("nlestandinaudioeditor: {} | stand_in_audio={} frames={} pcm={}", lines[0], r.stand_in_audio, r.frames, sink.pcm.len());
}

// ---- NLE6UTP1 (LEDGER SR729): tests/common's fixtures return the bytes the copies built (named tail block) ----
/// NLE6-UTP1 (SR729): FNV-1a of SR713's ac-3 reel (64x48 pattern + PCM16 of 1000s relabelled `ac-3`), pinned on its copy before
/// it moved onto tests/common's `Fixture` (the reel is now that one expression).
#[test]
fn nle6utp1_nle_fixtures_pin() {
    let got = stria::nle_face::fnv1a(&nlestandinaudioeditor_ac3_reel());
    assert_eq!(got, NLE6UTP1_NLE_PIN, "{got:#018x}");
}

/// The fnv [`nle6utp1_nle_fixtures_pin`] reads, measured on the copy before SR729.
const NLE6UTP1_NLE_PIN: u64 = 0xfe29_54c3_c765_08b4;

// ---- NLE5DRANGEKEYS (LEDGER SR735): Shift+I / O / X bind the editor's mark words through the face's KEYS (named tail block) ----

/// A key as GTK sends it and as stria-view's `--do key:<k>` token builds it (`BrowserKey` + `BrowserText` for one char).
fn nle5drangekeys_press(face: &mut Face, k: &str) -> bool {
    let a = face.on(&bandy::SMessage::BrowserKey(k.to_string()));
    let b = face.on(&bandy::SMessage::BrowserText(k.to_string()));
    a || b
}

/// Shift+I at 5 and Shift+O at 15 set the export marks through the editor's ONE `mark in` / `mark out` words (one
/// commit each); an out key before the in mark is refused by the editor's name and leaves the marks; Shift+X (AppKit:
/// the text alone) clears them in one commit, undone by Ctrl+z; a one-char key as `BrowserKey` alone is not taken.
#[test]
fn nle5drangekeys_shift_i_o_x_drive_the_editor_mark_words() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.seek(5);
    assert!(nle5drangekeys_press(&mut face, "I"));
    assert_eq!((face.marks(), face.state.undo_depth()), ((Some(5), None), 1));
    face.state.seek(15);
    assert!(nle5drangekeys_press(&mut face, "O"));
    assert_eq!((face.marks(), face.state.undo_depth(), face.last_error.take()), ((Some(5), Some(15)), 2, None));
    face.state.seek(2);
    assert!(nle5drangekeys_press(&mut face, "O"));
    assert_eq!(face.last_error.take().as_deref(), Some("mark out: the in mark is not before the out mark"));
    assert_eq!((face.marks(), face.state.undo_depth()), ((Some(5), Some(15)), 2), "a refused mark leaves the marks");
    assert!(face.on(&bandy::SMessage::BrowserText("X".into())));
    assert_eq!((face.marks(), face.state.undo_depth(), face.export_range()), ((None, None), 3, None));
    assert!(face.on(&bandy::SMessage::BrowserKey("Ctrl+z".into())));
    assert_eq!(face.marks(), (Some(5), Some(15)), "undo restores the cleared marks");
    assert!(!face.on(&bandy::SMessage::BrowserKey("I".into())), "a one-char key is read from the text only");
    assert!(!face.on(&bandy::SMessage::BrowserKey("Shift+I".into())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("nle5drangekeys: I / O / refused / X / undo");
}

/// The lower-case `i` / `o` stay SR208's clip trims: the timeline is the one `EditorState::verb("mark-in" / "mark-out")`
/// makes at the same ticks, and the export marks are untouched.
#[test]
fn nle5drangekeys_lower_i_o_still_trim_the_clip() {
    let (tl, src) = synth_fixture();
    let mut want = EditorState::new(tl.clone());
    want.select_at(0, 12).unwrap();
    want.seek(13);
    assert_eq!(want.verb("mark-in"), Some(Ok(())));
    want.seek(18);
    assert_eq!(want.verb("mark-out"), Some(Ok(())));
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select_at(0, 12).unwrap();
    face.state.seek(13);
    assert!(nle5drangekeys_press(&mut face, "i"));
    face.state.seek(18);
    assert!(nle5drangekeys_press(&mut face, "o"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_ne!(face.state.timeline, tl, "the trims moved the clip");
    assert_eq!((&face.state.timeline.tracks, face.marks()), (&want.timeline.tracks, (None, None)));
    println!("nle5drangekeys: i / o still trim");
}

// ---- STRIATESTTMPDIRREST (LEDGER SR739): nle.rs's scratch dirs are tests/common's per-pid dirs (named tail block) ----
/// STRIA-TEST-TMPDIR (SR729, SR739): `scratch_dir(g, n)` is `<target tmpdir>/<g>-<pid>/<n>`, removed and absent; `fresh_dir`
/// is the same dir created empty; and no test binary but tests/common reads the target tmpdir itself, so the next inline
/// fixed-path dir (the shared-target race SR729 fixed) reds here.
#[test]
fn nle_tests_scratch_under_the_pid_dir() {
    let d = common::scratch_dir("x", "y");
    let pid = format!("x-{}", std::process::id());
    assert!(d.ends_with(std::path::Path::new(&pid).join("y")), "{}", d.display());
    assert!(!d.exists(), "{}", d.display());
    assert!(d.parent().and_then(|p| p.parent()).is_some_and(|t| t.is_dir()), "{}", d.display());
    let f = common::fresh_dir("x", "z");
    assert_eq!(f.parent(), d.parent());
    assert!(f.is_dir() && std::fs::read_dir(&f).unwrap().next().is_none(), "{}", f.display());
    let token = concat!("CARGO_TARGET", "_TMPDIR");
    let bins = [("nle.rs", include_str!("nle.rs")), ("nle_export.rs", include_str!("nle_export.rs")), ("media.rs", include_str!("media.rs")), ("audio.rs", include_str!("audio.rs"))];
    let inline: usize = bins.iter().map(|(_, s)| s.matches(token).count()).sum();
    for (name, src) in bins {
        assert_eq!(src.matches(token).count(), 0, "{name} reads the target tmpdir inline: use common::scratch_dir / fresh_dir");
    }
    assert!(include_str!("common/mod.rs").contains(token), "tests/common is the one reader");
    println!("striatesttmpdirrest: scratch {} fresh {} inline reads {inline}", d.display(), f.display());
}

// ---- NLE3ESLIPFACE (LEDGER SR737): the face's one-frame slip / slide words, their keys (named tail block) ----

/// The SR694 face over NLE1's fixture (V1: A[13,23)@0 B[0,10)@10 A[23,33)@25) with B selected, and its twin editor.
fn nle3eslipface_pair() -> (Face, EditorState, Timeline) {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select_at(0, 12).unwrap();
    let mut twin = EditorState::new(tl.clone());
    twin.select_at(0, 12).unwrap();
    (face, twin, tl)
}

/// Rule: `slip+` / `slip-` / `slide-` are SR704's `slip 1` / `slip -1` / `slide -1` (one frame = one tick at the
/// timeline's `1/fps`) through THE dispatch: the face's timeline equals a twin editor's after the numbered word; a slide
/// carries the butted neighbour's out edge; undo restores the timeline.
#[test]
fn nle3eslipface_words_are_one_frame_of_the_editor_verbs() {
    let (mut face, mut twin, tl) = nle3eslipface_pair();
    let (a, b) = (SourceId(0), SourceId(1));
    assert!(face.verb("slip+"));
    assert_eq!(twin.verb("slip 1"), Some(Ok(())));
    assert_eq!(face.state.timeline, twin.timeline);
    assert_eq!(face.state.timeline.tracks[0].clips[1], Clip::new(b, 1, 11, 10), "the source window moved one frame");
    assert!(face.verb("slip-"));
    assert_eq!(face.state.timeline, tl);
    assert!(face.verb("slide-"));
    assert_eq!(twin.verb("slip -1"), Some(Ok(())));
    assert_eq!(twin.verb("slide -1"), Some(Ok(())));
    assert_eq!(face.state.timeline, twin.timeline);
    assert_eq!(&face.state.timeline.tracks[0].clips[..2], &[Clip::new(a, 13, 22, 0), Clip::new(b, 0, 10, 9)], "the neighbour follows");
    assert!(face.verb("slide+"));
    assert_eq!(face.state.timeline, tl);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, twin.timeline, "undo takes back the slide+");
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("nle3eslipface: slip+ = slip 1, slide- = slide -1, undo restores");
}

/// The four text keys `,` `.` `<` `>` send the words (the same timelines); a one-char key as `BrowserKey` alone is not taken.
#[test]
fn nle3eslipface_keys_send_the_words() {
    let (mut face, mut twin, tl) = nle3eslipface_pair();
    assert!(!nle3eslipface_press_text_only(&mut face, ","), "a one-char BrowserKey alone is not taken");
    assert!(nle5drangekeys_press(&mut face, "."));
    assert_eq!(twin.verb("slip 1"), Some(Ok(())));
    assert_eq!(face.state.timeline, twin.timeline);
    assert!(nle5drangekeys_press(&mut face, ","));
    assert_eq!(face.state.timeline, tl);
    assert!(nle5drangekeys_press(&mut face, "<"));
    assert_eq!(twin.verb("slip -1"), Some(Ok(())));
    assert_eq!(twin.verb("slide -1"), Some(Ok(())));
    assert_eq!(face.state.timeline, twin.timeline);
    assert!(nle5drangekeys_press(&mut face, ">"));
    assert_eq!(face.state.timeline, tl);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("nle3eslipface: , . < > send slip- slip+ slide- slide+");
}

/// A one-char key as `BrowserKey` alone (the face reads one-char keys from `BrowserText` only).
fn nle3eslipface_press_text_only(face: &mut Face, k: &str) -> bool {
    face.on(&bandy::SMessage::BrowserKey(k.to_string()))
}

/// Rule: with nothing selected the words are taken and refused by the editor's name on the message line
/// (`EditError::NoClip`'s text through `Face::verb`'s `{v}: {e}` road); the timeline does not move.
#[test]
fn nle3eslipface_no_selection_names_the_refusal() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    for v in ["slip-", "slip+", "slide-", "slide+"] {
        assert!(VERBS.contains(&v), "{v}");
        assert!(face.verb(v), "{v}");
        assert_eq!(face.last_error.take(), Some(format!("{v}: {}", EditError::NoClip)));
    }
    assert_eq!(face.state.timeline, tl);
    println!("nle3eslipface: no selection -> slip+: {}", EditError::NoClip);
}

// ---- STRIAINFOREEL (LEDGER SR743): an editor reel's person's line (named tail block) ----

/// The Player's own line for `bytes` (SR734's `summary_line` of the MediaInfo `Player::open` fills).
fn striainforeel_player_line(bytes: Vec<u8>) -> String {
    let p = stria::media::Player::open(bytes, std::sync::Arc::new(gneiss_pal::dsp::avsync::ManualTime::new()), None, 60).unwrap();
    stria::media::summary_line(p.info())
}

/// Rule: a synth reel's `Sources::summary` is the Player's `summary_line` of the same bytes (the two roads agree): the utp1
/// codec, its size, no profile (no configuration record), the measured `~` rate; an unknown id is `None`.
#[test]
fn striainforeel_synth_reel_summary_is_the_players_line() {
    let bytes = synth_source(0, 50, 25);
    let tl = Timeline::new(25);
    let mut src = Sources::new(&tl);
    src.add(SourceId(0), bytes.clone()).unwrap();
    let s = src.summary(SourceId(0)).expect("a synth reel has a line");
    assert!(s.starts_with("utp1 64x48 ~") && s.ends_with(" kb/s"), "{s}");
    assert_eq!(s, striainforeel_player_line(bytes));
    assert_eq!(src.summary(SourceId(7)), None);
    println!("striainforeel: synth reel summary=\"{s}\"");
}

/// Rule: an H.264 reel (SR728's CBR 400 + HRD export) says codec, size, the SPS's profile tokens and the btrt avg with the HRD
/// rate, exactly as the Player's line of the same bytes.
#[test]
fn striainforeel_h264_reel_summary_is_the_players_line() {
    use stria::nle_export::{AudioOut, H264Encoder, export};
    let (tl, src) = stria::nle_face::synth_fixture();
    let dir = common::fresh_dir("striainforeel", "h264"); // SR739: tests/common's per-pid scratch
    export(&tl, 0..35, &src, &dir, Box::new(H264Encoder::new(26, 10).with_bitrate(400, 25, 1).with_hrd(true)), AudioOut::None).unwrap();
    let mp4 = std::fs::read(dir.join("video.mp4")).unwrap();
    let avg = gneiss_pal::dsp::demux::Demuxer::open(mp4.clone()).unwrap().tracks()[0].btrt.expect("an H.264 export carries a btrt").2 as u64;
    let mut reels = Sources::new(&Timeline::new(25));
    reels.add(SourceId(0), mp4.clone()).unwrap();
    let s = reels.summary(SourceId(0)).unwrap();
    assert_eq!(s, format!("avc1 64x48 profile=Baseline level=1.2 {} kb/s (HRD 400)", (avg + 500) / 1000));
    assert_eq!(s, striainforeel_player_line(mp4));
    println!("striainforeel: h264 reel summary=\"{s}\"");
}

/// Rule: a labelled face's message line is the selected clip's `<reel>: <summary>`, painted at the preview band's foot (ink in
/// rows `preview_h - MESSAGE_H .. preview_h` left of the centred frame, where the deselected paint at the same playhead has
/// none of it); an error replaces it; nothing selected and no error: no message.
#[test]
fn striainforeel_face_message_is_the_selected_reel_and_an_error_wins() {
    use stria::nle_face::{EditorState, Face, Layout, MESSAGE_H, PixelOrder};
    let (tl, src) = stria::nle_face::synth_fixture();
    let line = src.summary(SourceId(0)).unwrap();
    let mut face = Face::new(EditorState::new(tl), src, (320, 144), PixelOrder::Rgba, true);
    let l: Layout = face.layout();
    let strip = |px: &[u8]| -> Vec<u8> {
        let mut out = Vec::new();
        for y in l.preview_h - MESSAGE_H..l.preview_h {
            let row = (y * l.w) as usize * 4;
            out.extend_from_slice(&px[row..row + 64 * 4]);
        }
        out
    };
    assert_eq!(face.message_text(), None);
    let bare = strip(&face.paint());
    face.state.select(0, 0).unwrap();
    assert_eq!(face.message_text(), Some(format!("A: {line}")));
    let summary = strip(&face.paint());
    assert_ne!(summary, bare, "the summary inks the message rows");
    face.last_error = Some("split: no such clip".into());
    assert_eq!(face.message_text().as_deref(), Some("split: no such clip"));
    let error = strip(&face.paint());
    assert_ne!(error, summary, "the error replaces the summary");
    face.last_error = None;
    face.state.deselect();
    assert_eq!(strip(&face.paint()), bare);
    println!("striainforeel: face message=\"A: {line}\" preview_h={} rows={}", l.preview_h, MESSAGE_H);
}

// ---- NLE5DKEYSHELP (LEDGER SR744): the keys help overlay — its rows, its toggle, the dismiss not applied, its paint (named tail block) ----
use stria::nle_face::{HELP, Hit, KEYS, help_phrase, help_rows};

fn nle5dkeyshelp_face(w: u32, h: u32, labels: bool) -> Face {
    let (tl, src) = synth_fixture();
    Face::new(EditorState::new(tl), src, (w, h), PixelOrder::Rgba, labels)
}

/// The rows: one per KEYS action in KEYS order, every action has a non-empty phrase in HELP, and every KEYS row's key is
/// in its action's row (`Space` for " "; an AppKit private-use alias only where the row names another key).
#[test]
fn nle5dkeyshelp_rows_cover_every_key() {
    let mut acts: Vec<&str> = Vec::new();
    for &(_, a) in KEYS {
        if !acts.contains(&a) {
            acts.push(a);
        }
    }
    let rows = help_rows();
    assert_eq!(rows.len(), acts.len());
    for (i, &a) in acts.iter().enumerate() {
        assert!(HELP.iter().any(|(x, _)| *x == a), "no phrase for {a}");
        assert_eq!(rows[i].1, help_phrase(a));
        assert!(!rows[i].1.is_empty(), "{a}");
    }
    for &(k, a) in KEYS {
        let i = acts.iter().position(|x| *x == a).unwrap();
        let first = k.chars().next().unwrap();
        if k.chars().count() == 1 && ('\u{f700}'..='\u{f8ff}').contains(&first) {
            assert!(!rows[i].0.is_empty(), "{a}: an alias with no named key");
            continue;
        }
        let lab = if k == " " { "Space" } else { k };
        assert!(rows[i].0.split(" / ").any(|x| x == lab), "{lab:?} not in {:?}", rows[i].0);
    }
    assert!(KEYS.contains(&("?", "help")));
    println!("nle5dkeyshelp: rows={} keys={}", rows.len(), KEYS.len());
}

/// `?` (text) shows the help; a following `End` only dismisses it (no seek, the playhead still); `?` while shown hides it;
/// a bare modifier and GTK's `Shift+question` are not taken and leave it shown; Escape dismisses; `help` is the one toggle.
#[test]
fn nle5dkeyshelp_keys_toggle_and_the_dismiss_is_not_applied() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    let ph = f.state.playhead;
    assert!(!f.help_shown());
    assert!(!f.on(&bandy::SMessage::BrowserKey("?".into())), "a one-char BrowserKey alone is not taken");
    assert!(f.on(&bandy::SMessage::BrowserText("?".into())));
    assert!(f.help_shown());
    assert!(f.on(&bandy::SMessage::BrowserKey("End".into())));
    assert_eq!((f.help_shown(), f.playing, f.rate, f.state.playhead), (false, false, 1, ph));
    assert!(f.on(&bandy::SMessage::BrowserText("?".into())));
    assert!(!f.on(&bandy::SMessage::BrowserKey("Shift+Shift_L".into())));
    assert!(!f.on(&bandy::SMessage::BrowserKey("Shift+question".into())));
    assert!(f.help_shown(), "an unbound key leaves the help shown");
    assert!(f.on(&bandy::SMessage::BrowserText("?".into())));
    assert!(!f.help_shown(), "? while shown hides it");
    assert!(f.verb("help") && f.help_shown());
    assert!(f.on(&bandy::SMessage::BrowserKey("Escape".into())));
    assert!(!f.help_shown());
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    println!("nle5dkeyshelp: ? shows, j dismisses unapplied, Shift_L kept, Escape dismisses");
}

/// While shown, a press on a clip is taken and ignored (no drag), its release dismisses the help, the click after it is not
/// applied (no seek, no selection).
#[test]
fn nle5dkeyshelp_click_dismisses_unapplied() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    let (x, y) = f.layout().point(0, 30);
    assert!(matches!(f.layout().hit(&f.state.timeline, x, y), Hit::Clip(0, 2, _)));
    assert!(f.verb("help"));
    let (ph, sel) = (f.state.playhead, f.state.selection);
    assert!(!f.on(&bandy::SMessage::BrowserButton { down: true, x, y, mods: 0, surface: 0 }));
    assert!(f.dragging().is_none());
    assert!(f.on(&bandy::SMessage::BrowserButton { down: false, x, y, mods: 0, surface: 0 }));
    assert!(!f.help_shown());
    f.on(&bandy::SMessage::BrowserClick(x, y, 0));
    assert_eq!((f.state.playhead, f.state.selection), (ph, sel));
    println!("nle5dkeyshelp: a click dismisses, playhead {ph} selection {sel:?} kept");
}

/// A labelled face paints the help over the timeline band (`ruler_y..h`) and nowhere else; a labels-off face paints
/// byte-identical with the help on (no labels, no overlay).
#[test]
fn nle5dkeyshelp_paints_over_the_timeline_band_only_with_labels() {
    let mut f = nle5dkeyshelp_face(640, 480, true);
    let off = f.paint();
    assert!(f.verb("help"));
    let on = f.paint();
    let cut = f.layout().ruler_y as usize * 640 * 4;
    assert!(cut > 0 && cut < on.len());
    assert_eq!(on[..cut], off[..cut], "the preview band is untouched");
    assert_ne!(on[cut..], off[cut..], "the timeline band shows the help");
    let mut plain = nle5dkeyshelp_face(640, 480, false);
    let a = plain.paint();
    assert!(plain.verb("help"));
    assert_eq!(plain.paint(), a, "labels off: no overlay");
    println!("nle5dkeyshelp: band fnv {:016x} -> {:016x}", stria::nle_face::fnv1a(&off[cut..]), stria::nle_face::fnv1a(&on[cut..]));
}
// ---- NLE5DKEYSHELPBUTTON (LEDGER SR765): the keys help word on the transport — `help` the last VERBS word (named tail block) ----

/// The button's message: `Input { target: SURFACE, text: "help" }`, what stria-view's `?` transport button fires.
fn nle5dkeyshelpbutton_press(f: &mut Face) -> bool {
    f.on(&bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: "help".into() })
}

/// `help` is a transport word, the LAST in VERBS (the button row's order); the button's message toggles the help through
/// `Face::verb`'s one arm: shown, then hidden; the keys help's own row for it (`?`, its phrase) is the KEYS-driven one.
#[test]
fn nle5dkeyshelpbutton_help_is_the_last_verb_and_the_button_toggles() {
    assert_eq!(VERBS.last(), Some(&"help"));
    assert!(VERBS.contains(&"help"));
    let mut f = nle5dkeyshelp_face(640, 480, false);
    assert!(!f.help_shown());
    assert!(nle5dkeyshelpbutton_press(&mut f));
    assert!(f.help_shown());
    assert!(nle5dkeyshelpbutton_press(&mut f));
    assert!(!f.help_shown());
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    let rows = help_rows();
    let row = rows.iter().find(|(_, p)| *p == help_phrase("help")).expect("the help row");
    assert_eq!(row.1, "show / hide this help");
    assert!(row.0.split(" / ").any(|k| k == "?"), "{:?}", row.0);
    println!("nle5dkeyshelpbutton: VERBS[{}] = help; button shows then hides; row {:?}", VERBS.len() - 1, row);
}

/// A labelled face paints differently while the button shows the help and byte-identical after the second press; a
/// labels-off face is byte-identical across the toggle (the help paints through the Labels hook).
#[test]
fn nle5dkeyshelpbutton_paint_toggles_back_byte_identical() {
    let mut f = nle5dkeyshelp_face(640, 480, true);
    let before = f.paint();
    assert!(nle5dkeyshelpbutton_press(&mut f));
    let shown = f.paint();
    assert_ne!(shown, before, "the button shows the help");
    assert!(nle5dkeyshelpbutton_press(&mut f));
    assert_eq!(f.paint(), before, "the second press restores the frame");
    let mut plain = nle5dkeyshelp_face(640, 480, false);
    let a = plain.paint();
    assert!(nle5dkeyshelpbutton_press(&mut plain));
    assert_eq!(plain.paint(), a, "labels off: no overlay");
    assert!(nle5dkeyshelpbutton_press(&mut plain));
    assert_eq!(plain.paint(), a);
    println!("nle5dkeyshelpbutton: fnv {:016x} -> {:016x} -> back", stria::nle_face::fnv1a(&before), stria::nle_face::fnv1a(&shown));
}

// ---- STRIAINFOREELAUDIO (LEDGER SR761): an editor reel's line carries its audio part, equal to the Player's (named tail block) ----

/// Rule: the synth reel's `Sources::summary` is the Player's `summary_line` of the same bytes INCLUDING the audio part: SR743's
/// video half, ` | `, the mono PCM16 48 kHz track's exact `pcm 768 kb/s` (the decoder registry's name, `track_audio_bitrate`).
#[test]
fn striainforeelaudio_synth_reel_line_is_the_players_with_the_pcm_part() {
    let bytes = synth_source(0, 50, 25);
    let mut src = Sources::new(&Timeline::new(25));
    src.add(SourceId(0), bytes.clone()).unwrap();
    let s = src.summary(SourceId(0)).unwrap();
    assert!(s.starts_with("utp1 64x48 ~") && s.ends_with(" kb/s | pcm 768 kb/s"), "{s}");
    assert_eq!(s, striainforeel_player_line(bytes));
    println!("striainforeelaudio: synth reel summary=\"{s}\"");
}

/// Rule: a video-only reel (tests/common's utp1 pattern, no audio track) has no audio part and is still the Player's line.
#[test]
fn striainforeelaudio_video_only_reel_has_no_audio_part() {
    let bytes = pattern_mp4(64, 48, 25, 50, 10);
    let mut src = Sources::new(&Timeline::new(25));
    src.add(SourceId(0), bytes.clone()).unwrap();
    let s = src.summary(SourceId(0)).unwrap();
    assert!(s.starts_with("utp1 64x48 ") && !s.contains(" | "), "{s}");
    assert_eq!(s, striainforeel_player_line(bytes));
}

/// Rule: the face's reel line (the message line's idle text, stria-view's `"reel"`) carries the audio part.
#[test]
fn striainforeelaudio_face_reel_line_carries_the_audio_part() {
    use stria::nle_face::{EditorState, Face, PixelOrder};
    let (tl, src) = stria::nle_face::synth_fixture();
    let line = src.summary(SourceId(0)).unwrap();
    let mut face = Face::new(EditorState::new(tl), src, (320, 144), PixelOrder::Rgba, false);
    face.state.select(0, 0).unwrap();
    let r = face.reel_line().unwrap();
    assert_eq!(r, format!("A: {line}"));
    assert!(r.ends_with(" | pcm 768 kb/s"), "{r}");
    println!("striainforeelaudio: face reel_line=\"{r}\"");
}

// ---- NLE6UTP1SYNTH (LEDGER SR749): the host's synthetic reel is demux_core's (named tail block) ----
/// NLE6UTP1SYNTH (SR749): `synth_source` is demux_core's `build::synth_reel` (the reel both rings build): the same bytes
/// by both roads, pinned by FNV-1a 64 (read on the pre-move `synth_source`; demux_core's `synth_reel_kats` pins the same u64).
#[test]
fn nle6utp1synth_host_reel_is_demux_cores() {
    let host = stria::nle_face::fnv1a(&synth_source(1000, 30, 25));
    let core = stria::nle_face::fnv1a(&gneiss_pal::dsp::demux::build::synth_reel(1000, 30, 25, &Default::default()));
    assert_eq!(host, 0xbabb_167e_432a_5e6b, "the pre-move synth_source bytes");
    assert_eq!(host, core, "both roads build one reel");
    println!("nle6utp1synth: synth_source(1000,30,25) fnv {host:#018x} == demux_core build::synth_reel");
}

// ---- NLE3ESLIPSTEP (LEDGER SR752): Ctrl / Cmd chords step SLIP_STEP frames through SR737's words (named tail block) ----
use stria::nle_face::SLIP_STEP;

/// A chord as GTK sends it: `BrowserKey` alone (no `BrowserText` under Ctrl / Meta).
fn nle3eslipstep_chord(face: &mut Face, k: &str) -> bool {
    face.on(&bandy::SMessage::BrowserKey(k.to_string()))
}

/// Rule: `Ctrl+period` is SR704's `slip 10` through THE dispatch (the face's timeline equals a twin editor's), its Meta
/// twin the same, `Ctrl+comma` the step back, and the word `slip+10` the key's own road.
#[test]
fn nle3eslipstep_ctrl_period_slips_ten_frames() {
    let (mut face, mut twin, tl) = nle3eslipface_pair();
    let b = SourceId(1);
    assert!(nle3eslipstep_chord(&mut face, "Ctrl+period"));
    assert_eq!(twin.verb("slip 10"), Some(Ok(())));
    assert_eq!(face.state.timeline, twin.timeline);
    assert_eq!(face.state.timeline.tracks[0].clips[1], Clip::new(b, SLIP_STEP, 10 + SLIP_STEP, 10), "the source window moved SLIP_STEP frames");
    assert!(nle3eslipstep_chord(&mut face, "Meta+period"));
    assert_eq!(twin.verb("slip 10"), Some(Ok(())));
    assert_eq!(face.state.timeline, twin.timeline);
    assert!(nle3eslipstep_chord(&mut face, "Ctrl+comma"));
    assert!(nle3eslipstep_chord(&mut face, "Meta+comma"));
    assert_eq!(face.state.timeline, tl);
    assert!(face.verb("slip+10"));
    assert_eq!(face.state.timeline.tracks[0].clips[1], Clip::new(b, SLIP_STEP, 10 + SLIP_STEP, 10), "the word is the key's road");
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("nle3eslipstep: Ctrl+period = slip {SLIP_STEP}, Ctrl+comma = slip -{SLIP_STEP}");
}

/// Rule: `Ctrl+Shift+less` (GTK's name for Ctrl+<) is `slide -10` with both butted neighbours following; `Ctrl+Shift+greater`
/// takes it back; the Meta twin is the same. Fixture: V1 A[0,20)@0 B[0,10)@20 A[23,33)@30 (both of B's cuts butted).
#[test]
fn nle3eslipstep_ctrl_less_slides_ten_with_neighbours() {
    let (mut tl, src) = synth_fixture();
    let (a, b) = (SourceId(0), SourceId(1));
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 10, 20), Clip::new(a, 23, 33, 30)];
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select_at(0, 22).unwrap();
    let mut twin = EditorState::new(tl.clone());
    twin.select_at(0, 22).unwrap();
    assert!(nle3eslipstep_chord(&mut face, "Ctrl+Shift+less"));
    assert_eq!(twin.verb("slide -10"), Some(Ok(())));
    assert_eq!(face.state.timeline, twin.timeline);
    assert_eq!(face.state.timeline.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 13, 33, 20)], "both neighbours follow");
    assert!(nle3eslipstep_chord(&mut face, "Ctrl+Shift+greater"));
    assert_eq!(face.state.timeline, tl);
    assert!(nle3eslipstep_chord(&mut face, "Shift+Meta+less"));
    assert_eq!(face.state.timeline, twin.timeline);
    assert!(nle3eslipstep_chord(&mut face, "Shift+Meta+greater"));
    assert_eq!(face.state.timeline, tl);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("nle3eslipstep: Ctrl+Shift+less = slide -{SLIP_STEP}, the neighbours follow");
}

/// Rule: a coarse slip past the source's end is refused by the editor's name (`OutOfSource` through `Face::verb`'s
/// `{v}: {e}` road) and the timeline does not move. B[0,10) over a 50-tick source: four steps reach [40,50), the fifth is out.
#[test]
fn nle3eslipstep_slip_past_the_source_is_refused_by_name() {
    let (mut face, _twin, _tl) = nle3eslipface_pair();
    let b = SourceId(1);
    for _ in 0..4 {
        assert!(nle3eslipstep_chord(&mut face, "Ctrl+period"));
    }
    assert_eq!(face.state.timeline.tracks[0].clips[1], Clip::new(b, 40, 50, 10));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let before = face.state.timeline.clone();
    assert!(nle3eslipstep_chord(&mut face, "Ctrl+period"), "the key is taken");
    assert_eq!(face.last_error.take(), Some(format!("slip+10: {}", EditError::OutOfSource)));
    assert_eq!(face.state.timeline, before);
    println!("nle3eslipstep: slip+10 at [40,50) -> {}", EditError::OutOfSource);
}

/// The eight chords are KEYS rows (no VERBS word, no button); the help names both chords of each word with a phrase; a
/// count other than SLIP_STEP is not a face word; SR737's one-frame `.` still slips exactly one frame.
#[test]
fn nle3eslipstep_help_and_the_one_frame_keys() {
    let chords = [
        ("Ctrl+comma", "Meta+comma", "slip-10"),
        ("Ctrl+period", "Meta+period", "slip+10"),
        ("Ctrl+Shift+less", "Shift+Meta+less", "slide-10"),
        ("Ctrl+Shift+greater", "Shift+Meta+greater", "slide+10"),
    ];
    let rows = help_rows();
    for (c, m, w) in chords {
        assert!(KEYS.contains(&(c, w)) && KEYS.contains(&(m, w)), "{w}");
        assert!(!VERBS.contains(&w), "{w}: a key's word, no button");
        let row = rows.iter().find(|r| r.0.split(" / ").any(|k| k == c)).expect("a help row");
        assert!(row.0.split(" / ").any(|k| k == m), "{:?}", row.0);
        assert_eq!(row.1, help_phrase(w));
        assert!(!row.1.is_empty() && row.1 != w, "{w}: a phrase, not the word");
    }
    let (mut face, _twin, tl) = nle3eslipface_pair();
    for v in ["slip+7", "slip+10x", "slide-010x", "slip10"] {
        assert!(!face.verb(v), "{v} is not a face word");
    }
    assert_eq!(face.state.timeline, tl);
    assert!(nle5drangekeys_press(&mut face, "."));
    assert_eq!(face.state.timeline.tracks[0].clips[1], Clip::new(SourceId(1), 1, 11, 10), "one frame, unchanged");
    println!("nle3eslipstep: help rows={} keys={}", rows.len(), KEYS.len());
}

// ---- NLE5DEXPORTMESSAGEAUDIO (LEDGER SR756): the result line and the refusal name the video and audio words (named tail block) ----

#[test]
fn nle5dexportmessageaudio_aac_rate_is_named_on_the_line() {
    let dir = nlefaceexport_dir("message-aac96");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png aac:96", dir.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the message line");
    assert!(line.starts_with("export: 35 frames, ") && nle5dexportmessageduration_unwalled(&line).ends_with(&format!(" bytes, png + aac:96 -> {}", dir.display())), "{line}");
    assert!(dir.join("audio.mp4").is_file() && !dir.join("audio.wav").exists(), "{line}");
    println!("nle5dexportmessageaudio: {line}");
}

#[test]
fn nle5dexportmessageaudio_unknown_audio_names_both_tables() {
    use stria::nle_export::{AUDIO_NAMES, VIDEO_NAMES};
    let root = nlefaceexport_dir("message-refused");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png flac", root.display())));
    let e = face.last_error.take().expect("the refusal");
    assert!(e.starts_with("export: unknown audio flac; the words are export [--all] [<dir>] ["), "{e}");
    for w in VIDEO_NAMES.iter().chain(AUDIO_NAMES.iter()) {
        assert!(e.contains(w), "{w} missing from {e}");
    }
    assert!(e.contains("aac[:KBPS]"), "{e}");
    assert!(!face.exporting() && !root.exists(), "a refused audio word makes nothing");
    assert!(face.verb(&format!("export {} png wav extra", root.display())));
    let u = face.last_error.take().expect("the usage refusal");
    assert_eq!(u, format!("export: usage: {}", &e["export: unknown audio flac; the words are ".len()..]), "one usage for both refusals");
    println!("nle5dexportmessageaudio: {e}");
}

#[test]
fn nle5dexportmessageaudio_running_job_names_its_audio() {
    let dir = nlefaceexport_dir("message-running");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert_eq!(face.export_audio(), None);
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    assert_eq!(face.export_audio().as_deref(), Some("wav"));
    assert!(face.cancel_export());
    assert!(face.wait_export());
    assert_eq!(face.export_audio(), None);
    assert!(face.last_error.take().is_some_and(|l| l.starts_with("export: cancelled at 3/35 frames")));
    println!("nle5dexportmessageaudio: the paused job named wav, then none");
}

// ---- NLE3ESLIPDRAG (LEDGER SR760): a Ctrl / Cmd drag slips on the clip body, slides on its trim handle (named tail block) ----
use stria::nle_face::DragKind;

/// A press / release as the surface sends it, carrying modifier bits (bandy's `MOD_*`).
fn nle3eslipdrag_btn(down: bool, x: f64, y: f64, mods: u8) -> bandy::SMessage {
    bandy::SMessage::BrowserButton { down, x, y, mods, surface: 0 }
}

/// Ctrl-press `(x, y)`, move 40 px by `dx`, release (Ctrl or Meta by `mods`).
fn nle3eslipdrag_drag(face: &mut Face, x: f64, y: f64, dx: f64, mods: u8) {
    face.on(&nle3eslipdrag_btn(true, x, y, mods));
    face.on(&bandy::SMessage::BrowserMouseMove(x + dx, y, 0));
    face.on(&nle3eslipdrag_btn(false, x + dx, y, mods));
}

/// Rule: a Ctrl-press on B's body (SR737's pair: V1 A[13,23)@0 B[0,10)@10 A[23,33)@25, 8 px a tick) dragged 40 px is a
/// Slip of 5 ticks: during the drag the preview (`Drag::shown`) holds B's window at [5,15) while the committed timeline is
/// unchanged; the release equals a twin editor's `slip 5` (THE op, one commit); undo restores; the Meta (Cmd) press is the same.
#[test]
fn nle3eslipdrag_ctrl_drag_on_the_body_slips() {
    let (mut face, mut twin, tl) = nle3eslipface_pair();
    let b = SourceId(1);
    let l = face.layout();
    let (x, y) = l.point(0, 15);
    assert_eq!(l.tick_at(x + 40.0) - l.tick_at(x), 5, "40 px is 5 ticks at the face's zoom");
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_CONTROL));
    assert_eq!(face.dragging().map(|d| (d.kind, d.idx)), Some((DragKind::Slip, 1)));
    face.on(&bandy::SMessage::BrowserMouseMove(x + 40.0, y, 0));
    let shown = face.dragging().and_then(|d| d.shown(&face.state)).map(|s| s.timeline.tracks[0].clips[1]);
    assert_eq!(shown, Some(Clip::new(b, 5, 15, 10)), "the preview shows the slipped source window");
    assert_eq!(face.state.timeline, tl, "nothing committed during the drag");
    face.on(&nle3eslipdrag_btn(false, x + 40.0, y, bandy::MOD_CONTROL));
    assert_eq!(twin.verb("slip 5"), Some(Ok(())));
    assert_eq!(face.state.timeline, twin.timeline);
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    nle3eslipdrag_drag(&mut face, x, y, 40.0, bandy::MOD_SUPER);
    assert_eq!(face.state.timeline, twin.timeline, "Cmd is Ctrl's twin");
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("nle3eslipdrag: Ctrl-drag 40 px on B's body = slip 5");
}

/// Rule: a Ctrl-press on the selected B's in handle dragged 40 px is a Slide of 5 ticks (A's out edge follows), snapped
/// as Move snaps: B's out edge lands on A@25 (the guide) — a twin's `slide 5`.
#[test]
fn nle3eslipdrag_ctrl_drag_on_the_handle_slides() {
    let (mut face, mut twin, tl) = nle3eslipface_pair();
    let (a, b) = (SourceId(0), SourceId(1));
    let l = face.layout();
    let (x, y) = (l.x_of(10) as f64 + 1.0, l.point(0, 15).1);
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_CONTROL));
    assert_eq!(face.dragging().map(|d| (d.kind, d.idx)), Some((DragKind::Slide, 1)));
    face.on(&bandy::SMessage::BrowserMouseMove(x + 40.0, y, 0));
    assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((5, Some(25))));
    face.on(&nle3eslipdrag_btn(false, x + 40.0, y, bandy::MOD_CONTROL));
    assert_eq!(twin.verb("slide 5"), Some(Ok(())));
    assert_eq!(face.state.timeline, twin.timeline);
    assert_eq!(&face.state.timeline.tracks[0].clips[..2], &[Clip::new(a, 13, 28, 0), Clip::new(b, 0, 10, 15)], "the neighbour follows");
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("nle3eslipdrag: Ctrl-drag 40 px on B's in handle = slide 5");
}

/// Rule: a slip past the source (B[0,10) dragged 40 px left: src_in -5) is refused by name on the message line through
/// the drag's `{e}` road; the preview is None during it, and the timeline and undo depth do not move.
#[test]
fn nle3eslipdrag_slip_past_the_source_is_refused_by_name() {
    let (mut face, _twin, tl) = nle3eslipface_pair();
    let (x, y) = face.layout().point(0, 15);
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_CONTROL));
    face.on(&bandy::SMessage::BrowserMouseMove(x - 40.0, y, 0));
    assert_eq!(face.dragging().map(|d| (d.kind, d.delta)), Some((DragKind::Slip, -5)));
    assert!(face.dragging().and_then(|d| d.shown(&face.state)).is_none(), "a refused slip previews nothing");
    face.on(&nle3eslipdrag_btn(false, x - 40.0, y, bandy::MOD_CONTROL));
    assert_eq!(face.last_error.take(), Some(format!("drag: {}", EditError::OutOfSource)));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl, 0));
    println!("nle3eslipdrag: slip -5 at [0,10) -> {}", EditError::OutOfSource);
}

/// Rule: without Ctrl / Cmd the presses are today's: the body a Move (Alt alone the unsnapped Move, SR306), the handle a TrimIn.
#[test]
fn nle3eslipdrag_plain_presses_keep_move_and_trim() {
    let (mut face, _twin, _tl) = nle3eslipface_pair();
    let l = face.layout();
    let (x, y) = l.point(0, 15);
    face.on(&nle3eslipdrag_btn(true, x, y, 0));
    assert_eq!(face.dragging().map(|d| (d.kind, d.snap)), Some((DragKind::Move, true)));
    face.on(&bandy::SMessage::BrowserKey("Escape".into()));
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_ALT));
    assert_eq!(face.dragging().map(|d| (d.kind, d.snap)), Some((DragKind::Move, false)));
    face.on(&bandy::SMessage::BrowserKey("Escape".into()));
    face.on(&nle3eslipdrag_btn(true, l.x_of(10) as f64 + 1.0, y, 0));
    assert_eq!(face.dragging().map(|d| d.kind), Some(DragKind::TrimIn));
    println!("nle3eslipdrag: plain body = Move, Alt body = unsnapped Move, plain handle = TrimIn");
}

// ---- STRIAINFOREELHOVER (LEDGER SR762): the message names the clip under the pointer (named tail block) ----

/// The labelled (or not) synth face with B (V1 clip 1) selected, and the face's pointer geometry.
fn striainforeelhover_face(labels: bool) -> (stria::nle_face::Face, stria::nle_face::Layout) {
    use stria::nle_face::{EditorState, Face, PixelOrder};
    let (tl, src) = stria::nle_face::synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 144), PixelOrder::Rgba, labels);
    face.state.select(0, 1).unwrap();
    let l = face.layout();
    (face, l)
}

/// A bare pointer move (no button), as the surface sends it and stria-view's `move:<x>:<y>` builds it.
fn striainforeelhover_move(face: &mut stria::nle_face::Face, x: f64, y: f64) -> bool {
    face.on(&bandy::SMessage::BrowserMouseMove(x, y, 0))
}

/// Rule: the pointer resting over an unselected clip makes the message (and the witness's reel line) that clip's
/// `<reel>: <summary>`; the ruler, an empty track spot and the selected clip itself give the selected clip's line back; an
/// error still wins; the message row's paint follows.
#[test]
fn striainforeelhover_message_names_the_clip_under_the_pointer() {
    use stria::nle_face::Hit;
    let (mut face, l) = striainforeelhover_face(true);
    let (a, b) = (format!("A: {}", striainforeelhover_summary(&face, 0)), format!("B: {}", striainforeelhover_summary(&face, 1)));
    let row_y = l.tracks_y as f64 + 10.0;
    let at = |t: i64| l.x_of(t) as f64 + 0.5;
    let tl = face.state.timeline.clone();
    assert!(matches!(l.hit(&tl, at(5), row_y), Hit::Clip(0, 0, _)));
    assert!(matches!(l.hit(&tl, at(5), l.ruler_y as f64 + 1.0), Hit::Ruler(_)));
    assert!(matches!(l.hit(&tl, at(15), row_y), Hit::Clip(0, 1, _)));
    assert!(matches!(l.hit(&tl, at(22), row_y), Hit::Track(0, _)));
    assert_eq!(face.message_text().as_deref(), Some(b.as_str()));
    let selected = face.paint();
    assert!(!striainforeelhover_move(&mut face, at(5), row_y), "entering A asks no repaint"); assert!(striainforeelhoverdelay_dwell(&mut face), "the crossing tick repaints"); // STRIAINFOREELHOVERDELAY (SR789): A's line after HOVER_DWELL ticks
    assert_eq!(face.hovered_clip(), Some((0, 0)));
    assert_eq!(face.message_text().as_deref(), Some(a.as_str()));
    assert_eq!(face.reel_line().as_deref(), Some(a.as_str()), "the witness's reel follows the hover");
    assert_ne!(face.paint(), selected, "the message row shows A's line");
    assert!(!striainforeelhover_move(&mut face, at(6), row_y), "still over A: no repaint");
    face.last_error = Some("split: no such clip".into());
    assert_eq!(face.message_text().as_deref(), Some("split: no such clip"), "an error wins over the hover");
    face.last_error = None;
    assert!(striainforeelhover_move(&mut face, at(5), l.ruler_y as f64 + 1.0));
    assert_eq!((face.hovered_clip(), face.message_text()), (None, Some(b.clone())), "the ruler");
    assert_eq!(face.paint(), selected, "back to the selected clip's paint");
    striainforeelhover_move(&mut face, at(5), row_y); striainforeelhoverdelay_dwell(&mut face); // STRIAINFOREELHOVERDELAY (SR789): dwell on A so leaving it repaints
    assert!(striainforeelhover_move(&mut face, at(15), row_y));
    assert_eq!((face.hovered_clip(), face.message_text()), (None, Some(b.clone())), "the selected clip itself");
    striainforeelhover_move(&mut face, at(5), row_y); striainforeelhoverdelay_dwell(&mut face); // STRIAINFOREELHOVERDELAY (SR789): dwell on A so leaving it repaints
    assert!(striainforeelhover_move(&mut face, at(22), row_y));
    assert_eq!((face.hovered_clip(), face.message_text()), (None, Some(b.clone())), "an empty track spot");
    println!("striainforeelhover: hover={a} selected={b}");
}

/// Rule: while a drag is live the message keeps the selected clip's line (no flicker as the drag crosses other clips).
#[test]
fn striainforeelhover_a_live_drag_keeps_the_selected_line() {
    let (mut face, l) = striainforeelhover_face(true);
    let b = format!("B: {}", striainforeelhover_summary(&face, 1));
    let (row_y, at) = (l.tracks_y as f64 + 10.0, |t: i64| l.x_of(t) as f64 + 0.5);
    face.on(&bandy::SMessage::BrowserButton { down: true, x: at(15), y: row_y, mods: 0, surface: 0 });
    assert!(face.dragging().is_some(), "a press on B's body begins a drag");
    striainforeelhover_move(&mut face, at(5), row_y);
    assert!(face.dragging().is_some());
    assert_eq!((face.hovered_clip(), face.message_text()), (None, Some(b)));
}

/// Rule: a labels-off face paints no message, so a move over another clip asks no repaint and the paint is byte-identical.
#[test]
fn striainforeelhover_labels_off_is_unchanged() {
    let (mut face, l) = striainforeelhover_face(false);
    let before = face.paint();
    assert!(!striainforeelhover_move(&mut face, l.x_of(5) as f64 + 0.5, l.tracks_y as f64 + 10.0));
    assert_eq!(face.hovered_clip(), Some((0, 0)));
    assert_eq!(face.paint(), before);
}

/// The reel summary of V1 clip `i`'s source, as `Sources::summary` gives it (the face's own sources are private: a fresh
/// synth fixture opens the same bytes).
fn striainforeelhover_summary(face: &stria::nle_face::Face, i: usize) -> String {
    let (_, src) = stria::nle_face::synth_fixture();
    src.summary(face.state.timeline.tracks[0].clips[i].src).unwrap()
}

// ---- NLE5DEXPORTMESSAGEDURATION (LEDGER SR775): the export's wall time on the result line (named tail block) ----

/// The wall-time grammar of `stria::nle_face::wall_words`: `D.D s` (under 10 s), `S s` (10..59), `M min SS s` (1..59
/// minutes), `H h MM min` (an hour or more).
fn nle5dexportmessageduration_grammar(t: &str) -> bool {
    let two = |x: &str| x.len() == 2 && x.parse::<u32>().is_ok_and(|n| n < 60);
    let num = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    match nle5dexportmessagerate_unrated(t).unwrap_or("?").split(' ').collect::<Vec<_>>().as_slice() { // NLE5DEXPORTMESSAGERATE (SR785): an optional ` (<r> fps)` tail
        [x, "s"] => match x.split_once('.') {
            Some((a, b)) => num(a) && a.parse::<u32>().is_ok_and(|n| n < 10) && b.len() == 1 && num(b),
            None => num(*x) && x.parse::<u32>().is_ok_and(|n| (10..60).contains(&n)),
        },
        [m, "min", ss, "s"] => num(*m) && m.parse::<u32>().is_ok_and(|n| (1..60).contains(&n)) && two(*ss),
        [h, "h", mm, "min"] => num(*h) && h.parse::<u32>().is_ok_and(|n| n >= 1) && two(*mm),
        _ => false,
    }
}

/// A finished export's result line without its ` in <t>` (the last ` in ` before ` -> <dir>`), the time checked against
/// the grammar; panics when the line carries none (every finished line does).
fn nle5dexportmessageduration_unwalled(line: &str) -> String {
    let arrow = line.rfind(" -> ").unwrap_or_else(|| panic!("no ` -> <dir>` in {line}"));
    let head = &line[..arrow];
    let i = head.rfind(" in ").unwrap_or_else(|| panic!("no ` in <t>` in {line}"));
    let t = &head[i + 4..];
    assert!(nle5dexportmessageduration_grammar(t), "{t:?} is not a wall time in {line}");
    format!("{}{}", &head[..i], &line[arrow..])
}

#[test]
fn nle5dexportmessageduration_wall_words_kat() {
    use stria::nle_face::wall_words;
    use std::time::Duration;
    let ms = Duration::from_millis;
    for (d, want) in [
        (ms(0), "0.0 s"),
        (ms(800), "0.8 s"),
        (ms(9_999), "9.9 s"),
        (ms(10_000), "10 s"),
        (ms(12_400), "12 s"),
        (ms(59_900), "59 s"),
        (ms(60_000), "1 min 00 s"),
        (ms(65_000), "1 min 05 s"),
        (ms(3_599_900), "59 min 59 s"),
        (ms(3_600_000), "1 h 00 min"),
        (ms(3_720_000), "1 h 02 min"),
        (ms(90_000_000), "25 h 00 min"),
    ] {
        assert_eq!(wall_words(d), want, "{d:?}");
        assert!(nle5dexportmessageduration_grammar(want), "{want}");
    }
    for bad in ["10.0 s", "60 s", "1 min 5 s", "0 min 05 s", "0 h 02 min", "12s", "1 h 2 min"] {
        assert!(!nle5dexportmessageduration_grammar(bad), "{bad}");
    }
    println!("nle5dexportmessageduration: wall_words 12 KAT rows, 7 refused shapes");
}

#[test]
fn nle5dexportmessageduration_finished_line_names_its_time() {
    let dir = nlefaceexport_dir("duration-png");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png", dir.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.clone().expect("the message line");
    assert!(line.starts_with("export: 35 frames, ") && line.contains(" bytes, png + wav in "), "{line}");
    assert!(line.ends_with(&format!(" -> {}", dir.display())), "{line}");
    assert!(nle5dexportmessageduration_unwalled(&line).ends_with(&format!(" bytes, png + wav -> {}", dir.display())), "{line}");
    let standing = face.paint();
    face.last_error = None;
    assert_eq!(face.paint(), standing, "labels off: the message paints nothing");
    println!("nle5dexportmessageduration: {line}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nle5dexportmessageduration_refusals_carry_no_time() {
    let has_time = |l: &str| l.match_indices(" in ").any(|(i, _)| l[i + 4..].starts_with(|c: char| c.is_ascii_digit()));
    let dir = nlefaceexport_dir("duration-refused");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png wav extra", dir.display())));
    let usage = face.last_error.take().expect("the usage refusal");
    assert!(usage.starts_with("export: usage: ") && !has_time(&usage), "{usage}");
    assert!(face.verb(&format!("export {} gif", dir.display())));
    let fmt = face.last_error.take().expect("the format refusal");
    assert!(fmt.starts_with("export: unknown format gif") && !has_time(&fmt), "{fmt}");
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    assert!(face.cancel_export());
    assert!(face.wait_export());
    let cancel = face.last_error.take().expect("the cancel line");
    assert_eq!(cancel, format!("export: cancelled at 3/35 frames; partial files left in {}", dir.display()));
    assert!(!has_time(&cancel), "{cancel}");
    println!("nle5dexportmessageduration: usage / unknown format / cancel carry no wall time");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- NLE5DEXPORTMESSAGERATE (LEDGER SR785): the export's frames per second on the result line (named tail block) ----

/// The rate grammar of `stria::nle_face::rate_words`: `N fps` (a whole number, 100 or more) or `A.D fps` (A under 100,
/// one decimal digit), no leading zeros.
fn nle5dexportmessagerate_grammar(r: &str) -> bool {
    let num = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()) && (x == "0" || !x.starts_with('0'));
    let Some(n) = nle5dexportmessageraterealtime_unmultiplied(r).strip_suffix(" fps") else { return false }; // NLE5DEXPORTMESSAGERATEREALTIME (SR794): an optional `, <m>x` tail
    match n.split_once('.') {
        Some((a, d)) => num(a) && a.parse::<u32>().is_ok_and(|v| v < 100) && d.len() == 1 && num(d),
        None => num(n) && n.parse::<u64>().is_ok_and(|v| v >= 100),
    }
}

/// A wall time with its optional ` (<rate>)` tail split off: no ` (` gives `t`; `<wall> (<rate>)` with a rate in the
/// grammar gives `<wall>`; anything else `None`.
fn nle5dexportmessagerate_unrated(t: &str) -> Option<&str> {
    match t.split_once(" (") {
        None => Some(t),
        Some((w, r)) => r.strip_suffix(')').filter(|r| nle5dexportmessagerate_grammar(r)).map(|_| w),
    }
}

#[test]
fn nle5dexportmessagerate_rate_words_kat() {
    use stria::nle_face::rate_words;
    use std::time::Duration;
    let (ms, us) = (Duration::from_millis, Duration::from_micros);
    let rows = [
        (35, ms(800), "43.8 fps"),
        (1000, ms(2_000), "500 fps"),
        (3, ms(1_000), "3.0 fps"),
        (120, ms(1_000), "120 fps"),
        (999, ms(10_000), "99.9 fps"),
        (1999, ms(20_000), "100 fps"),
        (1, ms(3_000), "0.3 fps"),
        (1, ms(1), "1000 fps"),
    ];
    for (f, d, want) in rows {
        assert_eq!(rate_words(f, d, (0, 1)).as_deref(), Some(want), "{f} frames in {d:?}"); // NLE5DEXPORTMESSAGERATEREALTIME (SR794): no rate, no multiple
        assert!(nle5dexportmessagerate_grammar(want), "{want}");
    }
    let nones = [(0, ms(800)), (10, ms(0)), (10, us(999))];
    for (f, d) in nones {
        assert_eq!(rate_words(f, d, (0, 1)), None, "{f} frames in {d:?}"); // NLE5DEXPORTMESSAGERATEREALTIME (SR794)
    }
    let bad = ["044 fps", "99 fps", "100.0 fps", "3 fps", "3.25 fps", "43.8fps"];
    for b in bad {
        assert!(!nle5dexportmessagerate_grammar(b), "{b}");
    }
    assert!(nle5dexportmessageduration_grammar("0.8 s (43.8 fps)") && !nle5dexportmessageduration_grammar("0.8 s (99 fps)"));
    println!("nle5dexportmessagerate: rate_words {} KAT rows, {} None, {} refused shapes", rows.len(), nones.len(), bad.len());
}

#[test]
fn nle5dexportmessagerate_finished_line_names_its_rate() {
    let dir = nlefaceexport_dir("rate-png");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png", dir.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.clone().expect("the message line");
    assert!(line.starts_with("export: 35 frames, ") && line.contains(" bytes, png + wav in "), "{line}");
    let arrow = line.rfind(" -> ").expect("the dir");
    let head = &line[..arrow];
    let t = &head[head.rfind(" in ").expect("the time") + 4..];
    let (wall, rate) = t.split_once(" (").unwrap_or_else(|| panic!("no ` (<r> fps)` in {line}"));
    assert!(rate.strip_suffix(')').is_some_and(nle5dexportmessagerate_grammar), "{rate:?} is not a rate in {line}");
    assert!(nle5dexportmessageduration_grammar(wall), "{wall:?} in {line}");
    assert!(nle5dexportmessageduration_unwalled(&line).ends_with(&format!(" bytes, png + wav -> {}", dir.display())), "{line}");
    let standing = face.paint();
    face.last_error = None;
    assert_eq!(face.paint(), standing, "labels off: the message paints nothing");
    println!("nle5dexportmessagerate: {line}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nle5dexportmessagerate_refusals_carry_no_rate() {
    let dir = nlefaceexport_dir("rate-refused");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png wav extra", dir.display())));
    let usage = face.last_error.take().expect("the usage refusal");
    assert!(usage.starts_with("export: usage: ") && !usage.contains(" fps)"), "{usage}");
    assert!(face.verb(&format!("export {} gif", dir.display())));
    let fmt = face.last_error.take().expect("the format refusal");
    assert!(fmt.starts_with("export: unknown format gif") && !fmt.contains(" fps)"), "{fmt}");
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    assert!(face.cancel_export());
    assert!(face.wait_export());
    let cancel = face.last_error.take().expect("the cancel line");
    assert_eq!(cancel, format!("export: cancelled at 3/35 frames; partial files left in {}", dir.display()));
    assert!(!cancel.contains(" fps)"), "{cancel}");
    println!("nle5dexportmessagerate: usage / unknown format / cancel carry no rate");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- STRIAINFOREELHOVERDELAY (LEDGER SR789): a dwell of HOVER_DWELL face ticks before the hover line replaces the selected one (named tail block) ----

/// `HOVER_DWELL` face ticks; true when every tick but the last asked no repaint and the last (the crossing) did.
fn striainforeelhoverdelay_dwell(face: &mut stria::nle_face::Face) -> bool {
    let ticks: Vec<bool> = (0..stria::nle_face::HOVER_DWELL).map(|_| face.tick()).collect();
    ticks.last() == Some(&true) && ticks[..ticks.len() - 1].iter().all(|t| !t)
}

/// Rule: the hover line shows only after the pointer has stood `HOVER_DWELL` face ticks on the same clip: until then the
/// selected clip's line stands and no tick repaints; the crossing tick repaints once; a move off a dwelt clip gives the
/// selected line back at once and repaints; an error still wins over a dwelt hover.
#[test]
fn striainforeelhoverdelay_the_hover_line_waits_for_the_dwell() {
    use stria::nle_face::HOVER_DWELL;
    let (mut face, l) = striainforeelhover_face(true);
    let (a, b) = (format!("A: {}", striainforeelhover_summary(&face, 0)), format!("B: {}", striainforeelhover_summary(&face, 1)));
    let (row_y, at) = (l.tracks_y as f64 + 10.0, |t: i64| l.x_of(t) as f64 + 0.5);
    assert!(!striainforeelhover_move(&mut face, at(5), row_y), "entering A asks no repaint");
    assert_eq!(face.hovered_clip(), Some((0, 0)));
    assert!(face.hover_pending(), "the clock runs while the dwell counts");
    for n in 1..HOVER_DWELL {
        assert!(!face.tick(), "tick {n}: no repaint");
        assert_eq!(face.message_text().as_deref(), Some(b.as_str()), "tick {n}: still the selected line");
    }
    assert!(face.tick(), "the crossing tick repaints");
    assert_eq!((face.hover_dwelt(), face.message_text()), (Some((0, 0)), Some(a.clone())));
    assert!(!face.hover_pending(), "dwelt: the clock may stop");
    assert!(!face.tick(), "a further tick asks nothing");
    assert_eq!(face.reel_line().as_deref(), Some(a.as_str()));
    face.last_error = Some("split: no such clip".into());
    assert_eq!(face.message_text().as_deref(), Some("split: no such clip"), "an error wins over a dwelt hover");
    face.last_error = None;
    assert!(striainforeelhover_move(&mut face, at(5), l.ruler_y as f64 + 1.0), "a dwelt hover ends: repaint");
    assert_eq!(face.message_text().as_deref(), Some(b.as_str()), "the ruler: the selected line at once");
    println!("striainforeelhoverdelay: dwell={HOVER_DWELL} ticks (320 ms at 25 fps) hover={a} selected={b}");
}

/// Rule: a change of the hovered clip resets the count: A -> B (the selected clip: no hover) -> A within the dwell never
/// shows A, and the count restarts at the second entry.
#[test]
fn striainforeelhoverdelay_a_change_resets_the_count() {
    use stria::nle_face::HOVER_DWELL;
    let (mut face, l) = striainforeelhover_face(true);
    let b = format!("B: {}", striainforeelhover_summary(&face, 1));
    let (row_y, at) = (l.tracks_y as f64 + 10.0, |t: i64| l.x_of(t) as f64 + 0.5);
    for x in [at(5), at(15), at(5)] {
        assert!(!striainforeelhover_move(&mut face, x, row_y), "no dwelt hover ends: no repaint");
        for _ in 1..HOVER_DWELL {
            assert!(!face.tick());
            assert_eq!(face.message_text().as_deref(), Some(b.as_str()));
        }
    }
    assert!(face.tick(), "HOVER_DWELL ticks after the second entry");
    assert_eq!(face.hover_dwelt(), Some((0, 0)));
}

/// Rule: a labels-off face asks no repaint on any tick and paints byte-identically, while the witness's reel line still
/// follows the dwell.
#[test]
fn striainforeelhoverdelay_labels_off_is_unchanged() {
    use stria::nle_face::HOVER_DWELL;
    let (mut face, l) = striainforeelhover_face(false);
    let before = face.paint();
    let b = format!("B: {}", striainforeelhover_summary(&face, 1));
    assert!(!striainforeelhover_move(&mut face, l.x_of(5) as f64 + 0.5, l.tracks_y as f64 + 10.0));
    assert!(!face.hover_pending(), "labels off: the clock stays off");
    for _ in 1..HOVER_DWELL {
        assert!(!face.tick());
    }
    assert_eq!(face.reel_line().as_deref(), Some(b.as_str()));
    assert!(!face.tick(), "labels off: the crossing asks no repaint");
    assert_eq!(face.hover_dwelt(), Some((0, 0)));
    assert_eq!(face.reel_line().map(|s| s.starts_with("A: ")), Some(true), "the witness follows the dwell");
    assert_eq!(face.paint(), before);
}

/// Rule: a change under a resting pointer with no move is seen at the next tick: selecting the dwelt clip A leaves no
/// hover (the line is A's as the selection), and selecting B again restarts the count rather than reusing a stale one.
#[test]
fn striainforeelhoverdelay_a_select_under_the_pointer_leaves_no_stale_count() {
    let (mut face, l) = striainforeelhover_face(true);
    let (a, b) = (format!("A: {}", striainforeelhover_summary(&face, 0)), format!("B: {}", striainforeelhover_summary(&face, 1)));
    striainforeelhover_move(&mut face, l.x_of(5) as f64 + 0.5, l.tracks_y as f64 + 10.0);
    assert!(striainforeelhoverdelay_dwell(&mut face));
    face.state.select(0, 0).unwrap();
    assert_eq!((face.hovered_clip(), face.message_text()), (None, Some(a)), "A selected: no hover, A's line");
    face.tick();
    face.state.select(0, 1).unwrap();
    assert_eq!(face.hover_dwelt(), None, "the count was cleared at the tick that saw no hover");
    assert_eq!(face.message_text().as_deref(), Some(b.as_str()));
    assert!(striainforeelhoverdelay_dwell(&mut face), "the dwell counts again from 1");
}

// ---- NLE5DEXPORTMESSAGERATEREALTIME (LEDGER SR794): the export's speed as a multiple of real time (named tail block) ----

/// The multiple grammar of `stria::nle_face::realtime_words`: `Nx` (a whole number, 10 or more), `A.Dx` (A 1..9, one
/// decimal digit) or `0.DDx` (two decimal digits), no leading zeros.
fn nle5dexportmessageraterealtime_grammar(m: &str) -> bool {
    let num = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()) && (x == "0" || !x.starts_with('0'));
    let digits = |x: &str| x.bytes().all(|b| b.is_ascii_digit());
    let Some(n) = m.strip_suffix('x') else { return false };
    match n.split_once('.') {
        None => num(n) && n.parse::<u64>().is_ok_and(|v| v >= 10),
        Some((a, d)) if d.len() == 1 => num(a) && a.parse::<u32>().is_ok_and(|v| (1..10).contains(&v)) && digits(d),
        Some((a, d)) => a == "0" && d.len() == 2 && digits(d),
    }
}

/// A rate with its optional `, <m>x` tail split off when the tail is in the grammar; else the rate as given.
fn nle5dexportmessageraterealtime_unmultiplied(r: &str) -> &str {
    match r.split_once(", ") {
        Some((h, m)) if nle5dexportmessageraterealtime_grammar(m) => h,
        _ => r,
    }
}

#[test]
fn nle5dexportmessageraterealtime_kat() {
    use stria::nle_face::{rate_words, realtime_words};
    use std::time::Duration;
    let (ms, us) = (Duration::from_millis, Duration::from_micros);
    let rows = [
        (35, ms(800), (25, 1), "1.8x"),
        (1000, ms(2_000), (25, 1), "20x"),
        (10, ms(1_000), (25, 1), "0.40x"),
        (30, ms(1_001), (30_000, 1_001), "1.0x"),
        (19, ms(2_000), (1, 1), "9.5x"),
        (999, ms(100_000), (1, 1), "10x"),
    ];
    for (f, d, fps, want) in rows {
        assert_eq!(realtime_words(f, d, fps).as_deref(), Some(want), "{f} frames in {d:?} at {fps:?}");
        assert!(nle5dexportmessageraterealtime_grammar(want), "{want}");
    }
    let nones = [(0, ms(800), (25, 1)), (35, ms(800), (25, 0)), (35, ms(800), (0, 1)), (35, us(999), (25, 1))];
    for (f, d, fps) in nones {
        assert_eq!(realtime_words(f, d, fps), None, "{f} frames in {d:?} at {fps:?}");
    }
    assert_eq!(rate_words(35, ms(800), (25, 1)).as_deref(), Some("43.8 fps, 1.8x"));
    assert_eq!(rate_words(35, ms(800), (25, 0)).as_deref(), Some("43.8 fps"), "no rate: the fps alone");
    let bad = ["01.8x", "1.80x", "10.0x", "9x", "0.4x", "1.8 x"];
    for b in bad {
        assert!(!nle5dexportmessageraterealtime_grammar(b), "{b}");
    }
    assert!(nle5dexportmessagerate_grammar("43.8 fps, 1.8x") && !nle5dexportmessagerate_grammar("43.8 fps, 1.80x"));
    assert!(nle5dexportmessageduration_grammar("0.8 s (43.8 fps, 1.8x)") && !nle5dexportmessageduration_grammar("0.8 s (43.8 fps, 9x)"));
    println!("nle5dexportmessageraterealtime: realtime_words {} KAT rows, {} None, {} refused shapes", rows.len(), nones.len(), bad.len());
}

#[test]
fn nle5dexportmessageraterealtime_finished_line_names_its_multiple() {
    let dir = nlefaceexport_dir("realtime-png");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png", dir.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.clone().expect("the message line");
    assert!(line.starts_with("export: 35 frames, ") && line.contains(" bytes, png + wav in "), "{line}");
    let arrow = line.rfind(" -> ").expect("the dir");
    let head = &line[..arrow];
    let t = &head[head.rfind(" in ").expect("the time") + 4..];
    let (wall, paren) = t.split_once(" (").unwrap_or_else(|| panic!("no ` (<r> fps, <m>x)` in {line}"));
    let (rate, m) = paren.strip_suffix(')').and_then(|p| p.split_once(", ")).unwrap_or_else(|| panic!("no `, <m>x` in {line}"));
    assert!(nle5dexportmessagerate_grammar(rate) && rate.ends_with(" fps"), "{rate:?} is not a rate in {line}");
    assert!(nle5dexportmessageraterealtime_grammar(m), "{m:?} is not a multiple in {line}");
    assert!(nle5dexportmessageduration_grammar(wall), "{wall:?} in {line}");
    assert!(nle5dexportmessageduration_unwalled(&line).ends_with(&format!(" bytes, png + wav -> {}", dir.display())), "{line}");
    let standing = face.paint();
    face.last_error = None;
    assert_eq!(face.paint(), standing, "labels off: the message paints nothing");
    println!("nle5dexportmessageraterealtime: {line}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nle5dexportmessageraterealtime_refusals_carry_none() {
    let dir = nlefaceexport_dir("realtime-refused");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} png wav extra", dir.display())));
    let usage = face.last_error.take().expect("the usage refusal");
    assert!(usage.starts_with("export: usage: ") && !usage.contains("x)"), "{usage}");
    assert!(face.verb(&format!("export {} gif", dir.display())));
    let fmt = face.last_error.take().expect("the format refusal");
    assert!(fmt.starts_with("export: unknown format gif") && !fmt.contains("x)"), "{fmt}");
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    assert!(face.cancel_export());
    assert!(face.wait_export());
    let cancel = face.last_error.take().expect("the cancel line");
    assert_eq!(cancel, format!("export: cancelled at 3/35 frames; partial files left in {}", dir.display()));
    println!("nle5dexportmessageraterealtime: usage / unknown format / cancel carry no multiple");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- NLE5DEXPORTMESSAGEETA (LEDGER SR796): a running export's remaining time from the rate so far (named tail block) ----

#[test]
fn nle5dexportmessageeta_eta_words_kat() {
    use stria::nle_face::{eta_secs, eta_words};
    use std::time::Duration;
    let (s, ms, us) = (Duration::from_secs, Duration::from_millis, Duration::from_micros);
    let rows = [
        (10, 35, s(2), "~5 s"),
        (1, 100, s(1), "~1:39"),
        (1, 10_000, s(1), "~2:46:39"),
        (35, 35, s(2), "~0 s"),
        (1, 2, ms(400), "~0 s"),
        (1, 2, ms(600), "~1 s"),
        (1, 2, ms(59_600), "~1:00"),
        (1, 2, ms(3_599_500), "~1:00:00"),
    ];
    for (d, t, e, want) in rows {
        assert_eq!(eta_words(d, t, e).as_deref(), Some(want), "eta_words({d}, {t}, {e:?})");
    }
    let nones = [(0, 35, s(2)), (10, 35, Duration::ZERO), (10, 35, us(999))];
    for (d, t, e) in nones {
        assert_eq!(eta_words(d, t, e), None, "eta_words({d}, {t}, {e:?}): no estimate yet");
    }
    assert_eq!(eta_secs(ms(1_500)), 2);
    assert_eq!(eta_secs(ms(1_499)), 1);
    println!("nle5dexportmessageeta: eta_words {} KAT rows, {} None, eta_secs half up", rows.len(), nones.len());
}

#[test]
fn nle5dexportmessageeta_export_eta_runs_only_while_exporting() {
    use stria::nle_face::eta_words;
    use std::time::Duration;
    let dir = nlefaceexport_dir("eta-running");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert_eq!(face.export_eta(), None, "no export: no eta");
    let t0 = std::time::Instant::now();
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    std::thread::sleep(Duration::from_millis(2));
    let e1 = face.export_eta().expect("an eta while the paused job runs");
    assert!(e1 >= Duration::from_millis(2) * 32 / 3, "{e1:?}: at least 32 frames x 2 ms / 3");
    assert!(e1 <= t0.elapsed() * 32 / 3, "{e1:?}: at most the rate since before the job started");
    assert!(eta_words(3, 35, t0.elapsed()).is_some_and(|w| w.starts_with('~')));
    std::thread::sleep(Duration::from_millis(2));
    let e2 = face.export_eta().expect("still running");
    assert!(e2 >= e1, "{e2:?} < {e1:?}: paused, done is fixed and the elapsed grows");
    assert!(face.cancel_export());
    assert!(face.wait_export());
    assert_eq!(face.export_eta(), None, "finished: no eta");
    let cancel = face.last_error.take().expect("the cancel line");
    assert_eq!(cancel, format!("export: cancelled at 3/35 frames; partial files left in {}", dir.display()));
    println!("nle5dexportmessageeta: paused at 3/35 the eta read {e1:?} then {e2:?}, none after");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nle5dexportmessageeta_labels_off_paint_is_unmoved() {
    use std::time::Duration;
    let dir = nlefaceexport_dir("eta-paint");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    std::thread::sleep(Duration::from_millis(2));
    let (e1, a) = (face.export_eta(), face.paint());
    std::thread::sleep(Duration::from_millis(5));
    let (e2, b) = (face.export_eta(), face.paint());
    assert!(e1.is_some() && e2 > e1, "{e1:?} -> {e2:?}: the eta moved");
    assert_eq!(a, b, "labels off: the eta paints nothing; the band is the fraction only");
    assert!(face.cancel_export());
    assert!(face.wait_export());
    println!("nle5dexportmessageeta: labels-off paint byte-identical across eta {e1:?} -> {e2:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- NLE5DEXPORTMESSAGEETABAND (LEDGER SR803): the running export's line on the message line (named tail block) ----

#[test]
fn nle5dexportmessageetaband_export_line_kat() {
    use stria::nle_face::export_line;
    let rows = [
        (3, 35, Some("~12 s"), "export: 3 / 35 ~12 s"),
        (0, 35, None, "export: 0 / 35"),
        (35, 35, Some("~0 s"), "export: 35 / 35 ~0 s"),
        (0, 0, None, "export: starting"),
    ];
    for (d, t, e, want) in rows {
        assert_eq!(export_line(d, t, e.map(String::from)), want, "export_line({d}, {t}, {e:?})");
    }
    println!("nle5dexportmessageetaband: export_line {} KAT rows", rows.len());
}

/// The eta part of a running line: `<n> s`, `m:ss` or `h:mm:ss` (SR796's `eta_words` grades without the tilde).
fn nle5dexportmessageetaband_eta_shaped(tail: &str) -> bool {
    match tail.strip_suffix(" s") {
        Some(n) => !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()),
        None => !tail.is_empty() && tail.contains(':') && tail.bytes().all(|b| b.is_ascii_digit() || b == b':'),
    }
}

#[test]
fn nle5dexportmessageetaband_paused_face_reads_the_export_line() {
    use std::time::Duration;
    let dir = nlefaceexport_dir("etaband-line");
    let (tl, src) = synth_fixture();
    let reel = format!("A: {}", src.summary(SourceId(0)).unwrap());
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, true);
    let l = face.layout().with_message(true); // fold fix: the paint layout carries the message row (STRIAINFOREELSTRIP SR747)
    assert!(l.message_h > 0, "labels on: the message row exists");
    let strip = |px: &[u8]| -> Vec<u8> {
        let mut out = Vec::new();
        for y in l.message_y..l.message_y + l.message_h {
            let row = (y * l.w) as usize * 4;
            out.extend_from_slice(&px[row..row + 64 * 4]);
        }
        out
    };
    face.state.select(0, 0).unwrap();
    assert_eq!(face.message_text().as_deref(), Some(reel.as_str()));
    let before = strip(&face.paint());
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    std::thread::sleep(Duration::from_millis(2));
    let line = face.message_text().expect("the running export's line");
    let tail = line.strip_prefix("export: 3 / 35 ~").unwrap_or_else(|| panic!("{line}: the export wins over the selected reel"));
    assert!(nle5dexportmessageetaband_eta_shaped(tail), "{line}: the eta part");
    assert_ne!(strip(&face.paint()), before, "the export line inks the message row");
    face.last_error = Some("split: no such clip".into());
    assert_eq!(face.message_text().as_deref(), Some("split: no such clip"), "an error wins over the export");
    assert_eq!(face.last_error.take().as_deref(), Some("split: no such clip"));
    assert!(face.message_text().is_some_and(|m| m.starts_with("export: 3 / 35 ~")), "the export line again");
    assert!(face.cancel_export());
    assert!(face.wait_export());
    let cancel = format!("export: cancelled at 3/35 frames; partial files left in {}", dir.display());
    assert_eq!(face.message_text(), Some(cancel.clone()), "the finished line (SR694's cancel) wins after the job");
    assert_eq!(face.last_error.take(), Some(cancel));
    assert_eq!(face.message_text().as_deref(), Some(reel.as_str()), "after the cancel line: SR743's reel line");
    let _ = std::fs::remove_dir_all(&dir);

    // Labels off: the message row paints nothing, so the paused paint is the idle paint outside the band rows.
    let odir = nlefaceexport_dir("etaband-off");
    let (tl, src) = synth_fixture();
    let mut off = nlefaceexport_face(tl, src);
    let lo = off.layout();
    let idle = off.paint();
    off.export_paused_at(&odir, "png", 3).unwrap();
    nlefaceexport_until(&off, (3, 35));
    std::thread::sleep(Duration::from_millis(2));
    assert!(off.message_text().is_some_and(|m| m.starts_with("export: 3 / 35 ~")));
    let shown = off.paint();
    let band = (lo.tracks_y - stria::nle_face::EXPORT_BAND_H)..lo.tracks_y;
    for y in (0..lo.h).filter(|y| !band.contains(y)) {
        let r = (y * lo.w * 4) as usize..((y + 1) * lo.w * 4) as usize;
        assert_eq!(&shown[r.clone()], &idle[r], "labels off: row {y} unmoved by the export line");
    }
    assert!(off.cancel_export() && off.wait_export());
    let _ = std::fs::remove_dir_all(&odir);
    println!("nle5dexportmessageetaband: paused line \"{line}\"; error wins; cancel then the reel line; labels off unmoved");
}

#[test]
fn nle5dexportmessageetaband_tick_repaints_when_the_eta_word_moves() {
    use std::time::Duration;
    let dir = nlefaceexport_dir("etaband-tick");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    std::thread::sleep(Duration::from_millis(2));
    face.export_tick();
    let w = face.message_text().expect("the running line");
    let t0 = std::time::Instant::now();
    while face.message_text().as_ref() == Some(&w) {
        assert!(t0.elapsed() < Duration::from_secs(10), "the paused eta never moved past {w}");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(face.export_progress(), Some((3, 35)), "done never moved");
    assert!(face.export_tick(), "the eta word moved with done fixed: the tick repaints");
    let moved = face.message_text();
    assert!(face.cancel_export());
    assert!(face.wait_export());
    let _ = face.last_error.take();
    let _ = std::fs::remove_dir_all(&dir);
    println!("nle5dexportmessageetaband: tick repaints {w} -> {moved:?} at 3/35");
}

// ---- NLE5DEXPORTMESSAGEETASMOOTH (LEDGER SR808): a time-based EMA of the export rate for the eta (named tail block) ----

#[test]
fn nle5dexportmessageetasmooth_eta_of_rate_kat() {
    use stria::nle_face::eta_of_rate;
    use std::time::Duration;
    let e = eta_of_rate(32, 10.0, Duration::ZERO).expect("32 frames at 10 fps");
    assert_eq!(e.as_millis(), 3200, "{e:?}: 32 / 10 = 3.2 s");
    assert_eq!(eta_of_rate(0, 10.0, Duration::ZERO), Some(Duration::ZERO), "nothing left: no time left");
    for r in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(eta_of_rate(32, r, Duration::ZERO), None, "rate {r}: no estimate");
    }
    println!("nle5dexportmessageetasmooth: eta_of_rate 32 @ 10 fps = {e:?}; rate 0 / negative / NaN / inf -> None");
}

#[test]
fn nle5dexportmessageetasmooth_ema_step_kat() {
    use stria::nle_face::{ETA_TAU, ema_step};
    let one = ema_step(0.0, 1.0, ETA_TAU, ETA_TAU);
    assert!((one - (1.0 - (-1.0f64).exp())).abs() < 1e-12, "{one}: dt = tau moves 1 - 1/e of the step");
    assert_eq!(ema_step(4.0, 9.0, 0.0, ETA_TAU), 4.0, "dt 0 keeps prev");
    let step = ema_step(1.0, 10.0, 3.0 * ETA_TAU, ETA_TAU);
    assert!(step > 9.5, "{step}: 1 -> 10 fps after 3 x tau sits above 9.5");
    let thrice = ema_step(ema_step(ema_step(1.0, 10.0, ETA_TAU, ETA_TAU), 10.0, ETA_TAU, ETA_TAU), 10.0, ETA_TAU, ETA_TAU);
    assert!((thrice - step).abs() < 1e-12, "{thrice} vs {step}: three tau steps are one 3 x tau step (the time rule)");
    let mut sixty = 1.0;
    for _ in 0..60 {
        sixty = ema_step(sixty, 10.0, 1.0 / 60.0, ETA_TAU);
    }
    let mut twentyfive = 1.0;
    for _ in 0..25 {
        twentyfive = ema_step(twentyfive, 10.0, 1.0 / 25.0, ETA_TAU);
    }
    assert!((sixty - twentyfive).abs() < 1e-9, "{sixty} vs {twentyfive}: one second at 60 Hz or 25 Hz smooths the same");
    println!("nle5dexportmessageetasmooth: ema_step tau {one:.6}, 3 tau {step:.6}, 1 s at 60 / 25 Hz {sixty:.9}");
}

#[test]
fn nle5dexportmessageetasmooth_scripted_slow_start() {
    use stria::nle_face::{ema_sample, eta_of_rate, eta_words, eta_words_of};
    use std::time::{Duration, Instant};
    let ms = Duration::from_millis;
    let t0 = Instant::now() + Duration::from_secs(10);
    let started = t0 - Duration::from_secs(2);
    let total = 1000u64;
    let mut st = (0.0, (started, 0u64));
    st = ema_sample(st.0, st.1, 0, t0);
    assert_eq!(st, (0.0, (started, 0)), "no move: unchanged");
    st = ema_sample(st.0, st.1, 1, t0);
    assert_eq!(st, (0.0, (t0, 1)), "the first move (the 2 s warm-up) only records");
    st = ema_sample(st.0, st.1, 2, t0 + ms(1000));
    assert!((st.0 - 1.0).abs() < 1e-9, "{}: the second move seeds 1 fps", st.0);
    let mut now = t0 + ms(1000);
    let mut done = 2;
    for _ in 0..30 {
        now += ms(100);
        done += 10;
        st = ema_sample(st.0, st.1, done, now);
    }
    assert_eq!(st.1, (now, 302));
    let same = ema_sample(st.0, st.1, 302, now + ms(100));
    assert_eq!(same, st, "a tick with done fixed changes nothing: the paused job keeps its rate");
    let smooth = eta_of_rate(total - done, st.0, Duration::ZERO).expect("an EMA eta");
    let elapsed = now - started;
    let mean = elapsed * (total - done) as u32 / done as u32;
    assert!(smooth < mean, "{smooth:?} >= {mean:?}: after the fast run the EMA eta sits under the mean's");
    assert!(st.0 > 50.0 && st.0 < 100.0, "{}: between the mean and the true 100 fps", st.0);
    assert_eq!(eta_words(10, 35, Duration::from_secs(2)).as_deref(), Some(eta_words_of(Duration::from_secs(5)).as_str()), "one grammar");
    println!("nle5dexportmessageetasmooth: 302 / 1000 after a 2 s warm-up: EMA {:.2} fps eta {smooth:?} vs mean eta {mean:?}", st.0);
}

// ---- NLE5DEXPORTMESSAGEETASTALL (LEDGER SR811): a stalled export's eta grows by the overdue time (named tail block) ----

#[test]
fn nle5dexportmessageetastall_eta_of_rate_kat() {
    use stria::nle_face::eta_of_rate;
    use std::time::Duration;
    let ms = Duration::from_millis;
    let near = |d: Option<Duration>, want: Duration| d.is_some_and(|d| d.abs_diff(want) < ms(1));
    assert_eq!(eta_of_rate(32, 10.0, Duration::ZERO).map(|d| d.as_millis()), Some(3200), "no stall: 32 / 10 = 3.2 s");
    assert_eq!(eta_of_rate(32, 10.0, ms(50)).map(|d| d.as_millis()), Some(3200), "50 ms is under one 100 ms frame: no overdue");
    let late = eta_of_rate(32, 10.0, ms(2100));
    assert!(near(late, ms(5200)), "{late:?}: 3.2 s + (2.1 s - 0.1 s) overdue");
    let left0 = eta_of_rate(0, 10.0, ms(5000));
    assert!(near(left0, ms(4900)), "{left0:?}: nothing left but stalled 5 s: the overdue alone (the export is not done)");
    for r in [0.0, f64::NAN] {
        assert_eq!(eta_of_rate(32, r, ms(5000)), None, "rate {r}: no estimate, stalled or not");
    }
    let base = eta_of_rate(32, 10.0, Duration::ZERO).unwrap();
    let (a, b, c) = (eta_of_rate(32, 10.0, ms(3000)).unwrap(), eta_of_rate(32, 10.0, ms(4000)).unwrap(), eta_of_rate(32, 10.0, ms(5000)).unwrap());
    assert!(base <= a && a < b && b < c, "{base:?} {a:?} {b:?} {c:?}: the eta grows with the stall and never drops under the unstalled one");
    println!("nle5dexportmessageetastall: 32 @ 10 fps: 0 / 50 ms -> 3.2 s, 2.1 s -> {late:?}; 0 left stalled 5 s -> {left0:?}");
}

#[test]
fn nle5dexportmessageetastall_scripted_stall() {
    use stria::nle_face::{ema_sample, eta_of_rate, eta_words_of};
    use std::time::{Duration, Instant};
    let ms = Duration::from_millis;
    // SR808's slow-start script by hand: a 2 s warm-up, the second move seeds 1 fps, then 30 x (100 ms, +10 frames).
    let t0 = Instant::now() + Duration::from_secs(10);
    let started = t0 - Duration::from_secs(2);
    let total = 1000u64;
    let mut st = (0.0, (started, 0u64));
    st = ema_sample(st.0, st.1, 1, t0);
    st = ema_sample(st.0, st.1, 2, t0 + ms(1000));
    let mut now = t0 + ms(1000);
    let mut done = 2;
    for _ in 0..30 {
        now += ms(100);
        done += 10;
        st = ema_sample(st.0, st.1, done, now);
    }
    assert!(st.0 > 50.0 && st.0 < 100.0, "{}: SR808's EMA after the fast run", st.0);
    // The stall: 5 s with done fixed leaves the rate and the last move alone.
    let at = now + Duration::from_secs(5);
    let stalled = ema_sample(st.0, st.1, done, at);
    assert_eq!(stalled, st, "a stall changes neither the rate nor the last move");
    let since = at - stalled.1.0;
    assert_eq!(since, Duration::from_secs(5));
    let fresh = eta_of_rate(total - done, st.0, Duration::ZERO).expect("the EMA eta");
    let late = eta_of_rate(total - done, st.0, since).expect("the stalled eta");
    // The arc's "- 0.1 s" assumed 10 fps; the frame interval is 1 / rate at the script's EMA (50..100 fps).
    let want = fresh.as_secs_f64() + 5.0 - 1.0 / st.0;
    assert!((late.as_secs_f64() - want).abs() < 1e-3, "{late:?} vs {want}: the EMA eta + 5 s - 1 / rate ({:.4} s, one frame at the EMA rate)", 1.0 / st.0);
    assert_ne!(eta_words_of(late), eta_words_of(fresh), "the stalled word moved");
    assert!(late > fresh, "{late:?} <= {fresh:?}");
    println!("nle5dexportmessageetastall: EMA {:.2} fps: eta {fresh:?} -> {late:?} after a 5 s stall ({} -> {})", st.0, eta_words_of(fresh), eta_words_of(late));
}

// ---- NLE5DEXPORTMESSAGEETASTALLWORD (LEDGER SR817): a stalled export's line names the stall (named tail block) ----

#[test]
fn nle5dexportmessageetastallword_stall_words_kat() {
    use stria::nle_face::{eta_words_of, stall_words, STALL_AFTER};
    use std::time::Duration;
    let ms = Duration::from_millis;
    assert_eq!(STALL_AFTER, Duration::from_secs(5));
    let rows: [(Duration, f64, Option<&str>); 10] = [
        (ms(4000), 10.0, None),
        (ms(5000), 10.0, None),
        (ms(5500), 10.0, Some("stalled 6 s")),
        (ms(5400), 10.0, Some("stalled 5 s")),
        (ms(7000), 0.0, Some("stalled 7 s")),
        (ms(4900), 0.0, None),
        (ms(500), 0.05, None),
        (ms(100_000), 0.05, None),
        (ms(250_000), 0.05, Some("stalled 4:10")),
        (ms(3_725_000), f64::NAN, Some("stalled 1:02:05")),
    ];
    for (d, r, want) in rows {
        let got = stall_words(d, r);
        assert_eq!(got.as_deref(), want, "stall_words({d:?}, {r})");
        if let Some(w) = got {
            assert_eq!(&w["stalled ".len()..], &eta_words_of(d)[1..], "{w}: SR796's one grammar without the tilde");
        }
    }
    println!("nle5dexportmessageetastallword: stall_words {} KAT rows (strict over max(5 s, 10 / rate); flat 5 s without a rate)", rows.len());
}

#[test]
fn nle5dexportmessageetastallword_scripted_stall() {
    use stria::nle_face::{ema_sample, eta_of_rate, eta_words_of, export_line, stall_words};
    use std::time::{Duration, Instant};
    let ms = Duration::from_millis;
    // SR811's script: SR808's 2 s warm-up, the second move seeds 1 fps, then 30 x (100 ms, +10 frames) to 302 / 1000.
    let t0 = Instant::now() + Duration::from_secs(10);
    let started = t0 - Duration::from_secs(2);
    let total = 1000u64;
    let mut st = (0.0, (started, 0u64));
    st = ema_sample(st.0, st.1, 1, t0);
    st = ema_sample(st.0, st.1, 2, t0 + ms(1000));
    let mut now = t0 + ms(1000);
    let mut done = 2;
    for _ in 0..30 {
        now += ms(100);
        done += 10;
        st = ema_sample(st.0, st.1, done, now);
    }
    assert_eq!(done, 302);
    // The line as `job_line` composes it: the stall word wins over the eta word when Some.
    let line = |done: u64, ema: f64, since: Duration| export_line(done, total, stall_words(since, ema).or_else(|| eta_of_rate(total - done, ema, since).map(eta_words_of)));
    assert_eq!(line(done, st.0, Duration::from_secs(7)), "export: 302 / 1000 stalled 7 s", "a 7 s stall names itself");
    let four = line(done, st.0, Duration::from_secs(4));
    let tail = four.strip_prefix("export: 302 / 1000 ~").unwrap_or_else(|| panic!("{four}: a 4 s stall keeps the eta word"));
    assert!(nle5dexportmessageetaband_eta_shaped(tail), "{four}: the eta part");
    // The next move: the last move is now, so the eta word returns.
    let at = now + ms(7100);
    let moved = ema_sample(st.0, st.1, done + 10, at);
    assert_eq!(moved.1, (at, 312), "the move records its instant");
    assert_eq!(stall_words(at - moved.1.0, moved.0), None, "no time since the move: no stall");
    let back = line(312, moved.0, at - moved.1.0);
    assert!(back.strip_prefix("export: 312 / 1000 ~").is_some_and(nle5dexportmessageetaband_eta_shaped), "{back}: the eta word returns");
    println!("nle5dexportmessageetastallword: EMA {:.2} fps: 7 s -> \"export: 302 / 1000 stalled 7 s\", 4 s -> \"{four}\", next move -> \"{back}\"", st.0);
}

#[test]
fn nle5dexportmessageetastallword_paused_face_names_the_stall() {
    use std::time::Duration;
    let dir = nlefaceexport_dir("etastallword-face");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    // One tick records the first move (0 -> 3) as the serve clock would; the EMA still has no rate, so the mean road's flat
    // 5 s decides, counted from that move (~5 s of wall clock).
    face.export_tick();
    let t0 = std::time::Instant::now();
    let line = loop {
        let m = face.message_text().expect("the running export's line");
        if m.starts_with("export: 3 / 35 stalled ") {
            break m;
        }
        assert!(m.starts_with("export: 3 / 35 ~"), "{m}: before the stall the eta word");
        assert!(t0.elapsed() < Duration::from_secs(20), "the paused job never read stalled: {m}");
        std::thread::sleep(Duration::from_millis(10));
    };
    let n = line.strip_prefix("export: 3 / 35 stalled ").and_then(|t| t.strip_suffix(" s")).and_then(|n| n.parse::<u64>().ok());
    assert!(n.is_some_and(|n| n >= 5), "{line}: the stall in whole seconds, 5 or more");
    assert!(face.export_eta().is_some(), "the witness's eta keeps its number while stalled");
    face.export_tick();
    assert_eq!(face.export_progress(), Some((3, 35)), "done never moved");
    assert!(face.message_text().is_some_and(|m| m.starts_with("export: 3 / 35 stalled ")), "a tick with done fixed keeps the stall word");
    assert!(face.cancel_export());
    assert!(face.wait_export());
    let cancel = format!("export: cancelled at 3/35 frames; partial files left in {}", dir.display());
    assert_eq!(face.last_error.take(), Some(cancel), "the cancel line unchanged");
    let _ = std::fs::remove_dir_all(&dir);
    println!("nle5dexportmessageetastallword: paused face reads \"{line}\"");
}

// ---- NLE5DEXPORTMESSAGEETATOTAL (LEDGER SR820): the total seeded before frame 1 (named tail block) ----
#[test]
fn nle5dexportmessageetatotal_face_reads_the_total_before_frame_one() {
    use stria::nle_face::export_line;
    assert_eq!(export_line(0, 35, None), "export: 0 / 35", "a seeded total before frame 1");
    assert_eq!(export_line(0, 0, None), "export: starting", "the zero-frame grammar kept");
    let dir = nlefaceexport_dir("etatotal-face");
    let (tl, src) = synth_fixture();
    let want = stria::nle_export::export_total(&tl, &(0..tl.end()));
    assert_eq!(want, 35, "the fixture's whole range");
    let mut face = nlefaceexport_face(tl, src);
    face.export_paused_at(&dir, "png", 3).unwrap();
    // The seed is stored before `start_export` returns: the total is the rule's whatever `done` the thread has reached.
    let p0 = face.export_progress().expect("the running job");
    let line = face.message_text().expect("the running export's line");
    let eta = face.export_eta();
    let p1 = face.export_progress().expect("the running job");
    assert_eq!((p0.1, p1.1), (35, 35), "total seeded at the start");
    assert!(p0.0 <= p1.0 && p1.0 <= 3, "{p0:?} then {p1:?}");
    assert_ne!(line, "export: starting", "never the zero-frame word for a 35-frame export");
    let d: u64 = line.strip_prefix("export: ").and_then(|t| t.split(" / ").next()).and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("{line}: the done count"));
    assert!(line.starts_with(&format!("export: {d} / 35")) && d <= 3, "{line}");
    if d == 0 {
        assert_eq!(line, "export: 0 / 35", "before frame 1: the real count, no eta");
    }
    if p1.0 == 0 {
        assert_eq!(eta, None, "SR796: no eta while done is 0");
    }
    nlefaceexport_until(&face, (3, 35));
    assert_eq!(face.export_progress(), Some((3, 35)), "the thread's readings confirm the seed: total never changed");
    assert!(face.cancel_export());
    assert!(face.wait_export());
    let cancel = format!("export: cancelled at 3/35 frames; partial files left in {}", dir.display());
    assert_eq!(face.last_error.take(), Some(cancel), "the cancel line unchanged");
    let _ = std::fs::remove_dir_all(&dir);
    println!("nle5dexportmessageetatotal: right after the call {p0:?} \"{line}\"; at (3, 35) the same total");
}

// ---- NLE5DEXPORTMESSAGEETASTALLWITNESS (LEDGER SR825): the stall rule's predicate for the witness; the finalise named (named tail block) ----
#[test]
fn nle5dexportmessageetastallwitness_stall_since_kat() {
    use stria::nle_face::{eta_words_of, stall_since, stall_words};
    use std::time::Duration;
    let ms = Duration::from_millis;
    // SR817's ten rows, the stalled ones as the time itself.
    let rows: [(Duration, f64, Option<Duration>); 10] = [
        (ms(4000), 10.0, None),
        (ms(5000), 10.0, None),
        (ms(5500), 10.0, Some(ms(5500))),
        (ms(5400), 10.0, Some(ms(5400))),
        (ms(7000), 0.0, Some(ms(7000))),
        (ms(4900), 0.0, None),
        (ms(500), 0.05, None),
        (ms(100_000), 0.05, None),
        (ms(250_000), 0.05, Some(ms(250_000))),
        (ms(3_725_000), f64::NAN, Some(ms(3_725_000))),
    ];
    for (d, r, want) in rows {
        assert_eq!(stall_since(d, r), want, "stall_since({d:?}, {r})");
        let word = stall_since(d, r).map(|s| format!("stalled {}", eta_words_of(s).trim_start_matches('~')));
        assert_eq!(stall_words(d, r), word, "stall_words({d:?}, {r}) reads stall_since: ONE rule");
    }
    println!("nle5dexportmessageetastallwitness: stall_since {} KAT rows, stall_words over it", rows.len());
}

#[test]
fn nle5dexportmessageetastallwitness_finishing_words_kat() {
    use stria::nle_face::{export_line, finishing_words};
    let rows: [(u64, u64, Option<&str>); 4] = [(35, 35, Some("finishing")), (34, 35, None), (0, 35, None), (0, 0, None)];
    for (d, t, want) in rows {
        assert_eq!(finishing_words(d, t).as_deref(), want, "finishing_words({d}, {t})");
    }
    assert_eq!(export_line(35, 35, Some("finishing".to_string())), "export: 35 / 35 finishing");
    assert_eq!(export_line(0, 0, finishing_words(0, 0)), "export: starting", "an export of no frames keeps its word");
    println!("nle5dexportmessageetastallwitness: finishing_words {} KAT rows", rows.len());
}

#[test]
fn nle5dexportmessageetastallwitness_finalise_reads_finishing_never_stalled() {
    use std::time::{Duration, Instant};
    let dir = nlefaceexport_dir("etastallwitness-finishing");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    // The hold after the last frame: done == total and the thread not returned, the finalise window (sink.finish runs next).
    face.export_paused_at(&dir, "png", 35).unwrap();
    nlefaceexport_until(&face, (35, 35));
    face.export_tick();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(6000) {
        assert_eq!(face.message_text().as_deref(), Some("export: 35 / 35 finishing"), "the finalise is named, no eta, no stall");
        assert_eq!(face.export_stall(), None, "the witness reads no stall while finishing");
        face.export_tick();
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(face.exporting(), "the job held in its finalise window");
    assert!(face.cancel_export());
    assert!(face.wait_export());
    let cancel = format!("export: cancelled at 35/35 frames; partial files left in {}", dir.display());
    assert_eq!(face.last_error.take(), Some(cancel), "the cancel line unchanged");
    let _ = std::fs::remove_dir_all(&dir);
    println!("nle5dexportmessageetastallwitness: done == total for 6 s reads \"export: 35 / 35 finishing\"");
}

#[test]
fn nle5dexportmessageetastallwitness_paused_face_export_stall() {
    use std::time::{Duration, Instant};
    let dir = nlefaceexport_dir("etastallwitness-stall");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.export_paused_at(&dir, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    // One tick records the first move (0 -> 3); the EMA has no rate, so the flat 5 s counts from that move (SR817's shape).
    face.export_tick();
    assert_eq!(face.export_stall(), None, "fresh: no stall");
    let t0 = Instant::now();
    let stall = loop {
        if let Some(s) = face.export_stall() {
            break s;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "the paused job never read a stall");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(stall > Duration::from_secs(5), "{stall:?}: strictly over the flat 5 s");
    let line = face.message_text().expect("the running export's line");
    assert!(line.starts_with("export: 3 / 35 stalled "), "{line}: the word and the witness read ONE rule");
    assert!(face.export_stall().is_some_and(|s| s >= stall), "the stall only grows while done is fixed");
    assert!(face.cancel_export());
    assert!(face.wait_export());
    let cancel = format!("export: cancelled at 3/35 frames; partial files left in {}", dir.display());
    assert_eq!(face.last_error.take(), Some(cancel), "the cancel line unchanged");
    assert_eq!(face.export_stall(), None, "no job: no stall");
    let _ = std::fs::remove_dir_all(&dir);
    println!("nle5dexportmessageetastallwitness: paused face export_stall {stall:?}, line \"{line}\"");
}

// ---- STRIAEXPORTFRAME (LEDGER SR830): the playhead frame as a PNG — preview road, pixel_core's writer, the face's `frame` verb (named tail block) ----

/// `frame_path`: beside the document, `<stem>-<tc with : as ->.png`; no document, the cwd-relative `untitled-<tc>.png`.
#[test]
fn striaexportframe_frame_path_kats() {
    use stria::nle_face::frame_path;
    use std::path::{Path, PathBuf};
    assert_eq!(frame_path(Some(Path::new("/x/reel.edl")), "00:00:01:12"), PathBuf::from("/x/reel-00-00-01-12.png"));
    assert_eq!(frame_path(None, "00:00:01:12"), PathBuf::from("untitled-00-00-01-12.png"));
    assert!(frame_path(None, "00:00:01:12").is_relative());
    assert_eq!(frame_path(Some(Path::new("/x/reel.edl")), &edl::timecode(37, 25)), PathBuf::from("/x/reel-00-00-01-12.png"), "edl's own timecode");
    println!("striaexportframe: /x/reel.edl at 00:00:01:12 -> /x/reel-00-00-01-12.png");
}

/// `save_frame` at tick 7 of NLE1's fixture writes a PNG whose header and decoded size are the frame's own (64x48) and whose
/// decoded RGBA is `preview(.., 7)`'s, byte for byte (lossless).
#[test]
fn striaexportframe_save_frame_is_the_preview_lossless() {
    use gneiss_pal::dsp::image::{decode_png, facts_of};
    use stria::nle_face::{preview, save_frame};
    let (tl, src) = synth_fixture();
    let dir = fresh_dir("nle", "striaexportframe-save");
    let path = dir.join("seven.png");
    assert_eq!(save_frame(&tl, &src, 7, &path).unwrap(), (64, 48));
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"\x89PNG"), "a PNG signature");
    let f = facts_of(&bytes).expect("a PNG header");
    assert_eq!((f.width, f.height), (64, 48));
    let img = decode_png(&bytes).unwrap();
    let want = preview(&tl, &src, 7).unwrap().expect("a frame at 7");
    assert_eq!((img.width, img.height), (want.width, want.height));
    assert!(img.rgba == want.rgba, "the saved frame is the preview's RGBA");
    println!("striaexportframe: tick 7 -> {} bytes, 64x48, lossless", bytes.len());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The face's `frame` verb: beside the document at the playhead's timecode, the message `frame: <name>`, `frame_saved` the
/// path; `frame <path>` writes that path; an unwritable path is `frame: <path>: <io>` with no file; no document is refused.
#[test]
fn striaexportframe_face_verb_writes_and_names_it() {
    let dir = fresh_dir("nle", "striaexportframe-face");
    let doc = dir.join("reel.edl");
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(&doc));
    assert_eq!(face.frame_saved(), None);
    face.state.seek(12);
    assert!(face.verb("frame"));
    let png = dir.join("reel-00-00-00-12.png");
    assert!(png.exists(), "{}", png.display());
    assert_eq!(face.message_text().as_deref(), Some("frame: reel-00-00-00-12.png"));
    assert_eq!(face.frame_message(), Some("frame: reel-00-00-00-12.png"));
    assert_eq!(face.frame_saved(), Some(png.as_path()));
    let named = dir.join("named frame.png");
    assert!(face.verb(&format!("frame {}", named.display())));
    assert!(named.exists());
    assert_eq!(face.last_error.take().as_deref(), Some("frame: named frame.png"));
    assert!(face.verb("frame /nonexistent-striaexportframe/x.png"));
    let e = face.last_error.take().unwrap_or_default();
    assert!(e.starts_with("frame: /nonexistent-striaexportframe/x.png: "), "{e}");
    assert!(!std::path::Path::new("/nonexistent-striaexportframe/x.png").exists());
    assert_eq!(face.frame_saved(), None, "a refused frame clears the saved path");
    assert!(face.verb("frames"), "not the frame word");
    let (tl, src) = synth_fixture();
    let mut bare = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    assert!(bare.verb("frame"));
    assert_eq!(bare.last_error.take().as_deref(), Some("frame: no file (no document path)"));
    assert_eq!(bare.frame_saved(), None);
    println!("striaexportframe: face frame -> {}; refused: {e}", png.display());
    let _ = std::fs::remove_dir_all(&dir);
}

/// `frame` is a transport word, before `help` (still the last).
#[test]
fn striaexportframe_frame_is_a_verb_before_help() {
    assert_eq!(VERBS.len(), 27);
    assert!(VERBS.contains(&"frame"));
    assert_eq!(VERBS.last(), Some(&"help"));
}

/// The 100 ms rule's measurement: one `save_frame` of a 1280x720 utp1 frame (render + PNG encode + write), timed and printed
/// (not asserted); the saved size is the reel's.
#[test]
fn striaexportframe_hd_frame_save_time() {
    use gneiss_pal::dsp::image::facts_of;
    use stria::nle_face::save_frame;
    let reel = build::mp4(&[build::test_pattern_track(1, 1280, 720, 25, 10, 10)], &build::Mp4Options::default());
    let mut tl = Timeline::new(25);
    let id = tl.add_source("HD", 10);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(id, 0, 10, 0)];
    let mut src = Sources::new(&tl);
    src.add(id, reel).expect("the HD reel opens");
    let dir = fresh_dir("nle", "striaexportframe-hd");
    let path = dir.join("hd.png");
    let t0 = std::time::Instant::now();
    let size = save_frame(&tl, &src, 3, &path).unwrap();
    let took = t0.elapsed();
    assert_eq!(size, (1280, 720));
    let bytes = std::fs::read(&path).unwrap();
    let f = facts_of(&bytes).expect("a PNG header");
    assert_eq!((f.width, f.height), (1280, 720));
    println!("striaexportframe: 1280x720 save_frame took {:.1} ms ({} bytes)", took.as_secs_f64() * 1000.0, bytes.len());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- STRIAWAVEFORM (LEDGER SR827): the peak scan rides the audio road; the face scans once (named tail block) ----

/// STRIAWAVEFORM (SR827): `scan_peaks` over `synth_source(0, 50, 25)` (2 s of mono PCM16 at 48 kHz) gives 200 buckets at
/// 100 / s, non-flat, equal (within 1 per bucket) to timeline_core's `peaks_of` over the PCM `render` mixes for one 0 dB
/// clip covering the whole source (the mix passes one unfaded layer through bit for bit): the same decoder road. An
/// unknown source is refused. The test line carries the scan's measured time (NLE-WAVEFORM-SCAN-THREAD owed).
#[test]
fn striawaveform_scan_peaks_reads_the_audio_road() {
    use timeline_core::view::{PEAKS_PER_SEC, peaks_of};
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let ti = tl.add_track(Kind::Audio);
    tl.tracks[ti].clips = vec![Clip::new(a, 0, 50, 0)];
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    let t0 = std::time::Instant::now();
    let p = stria::nle::scan_peaks(&src, a, PEAKS_PER_SEC).unwrap().expect("the synth reel has audio");
    let took = t0.elapsed();
    assert_eq!((p.per_sec, p.buckets.len()), (100, 200));
    let max = p.buckets.iter().copied().max().unwrap();
    assert!(max > 0 && p.buckets.iter().any(|&b| b != max), "a non-flat table: {:?}", &p.buckets[..10]);
    let mut sink = Collect::default();
    render(&tl, 0..50, &src, &mut sink).unwrap();
    assert_eq!(sink.pcm.len(), 2 * SYNTH_RATE as usize);
    let mixed = peaks_of(SYNTH_RATE, 1, &sink.pcm, PEAKS_PER_SEC);
    assert_eq!(mixed.buckets.len(), p.buckets.len());
    assert!(mixed.buckets.iter().zip(&p.buckets).all(|(m, s)| (*m as i32 - *s as i32).abs() <= 1), "the scan and the mix agree");
    assert!(stria::nle::scan_peaks(&src, SourceId(9), PEAKS_PER_SEC).is_err(), "an unknown source is refused");
    println!("striawaveform: scan buckets={} per_sec={} max={max} scan_ms={:.3} (2-s synth source, on the calling thread)", p.buckets.len(), p.per_sec, took.as_secs_f64() * 1e3);
}

/// STRIAWAVEFORM (SR827): the synth fixture's face scans each audio source once (reels A and B, both on A1) across three
/// paints; reel A's table is 200 buckets; the A1 row's first clip body shows more than one colour (the peaks over the body).
#[test]
fn striawaveform_face_scans_once_across_three_paints() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    assert_eq!(face.wave_scans(), 0, "Face::new scans nothing");
    let first = face.paint();
    assert_eq!(face.wave_scans(), 2, "the first paint scans A and B");
    let _ = face.paint();
    let third = face.paint();
    assert_eq!(face.wave_scans(), 2, "three paints, two scans");
    assert_eq!(first, third, "the tables never change the picture between paints");
    let a = face.state.timeline.tracks[1].clips[0];
    assert_eq!(face.wave_peaks(a.src).map(|p| p.buckets.len()), Some(200));
    let l = face.layout();
    let (y0, y1) = (l.row_y(1) + 2, l.row_y(1) + stria::nle_face::ROW_H as i64 - 2);
    let (x0, x1) = (l.x_of(a.at) + 1, l.x_of(a.end()) - 1);
    let mut seen: Vec<[u8; 4]> = Vec::new();
    for y in y0..y1 {
        for x in x0.max(1)..x1.min(l.w as i64) {
            let i = (y as usize * l.w as usize + x as usize) * 4;
            let c = [third[i], third[i + 1], third[i + 2], third[i + 3]];
            if !seen.contains(&c) {
                seen.push(c);
            }
        }
    }
    assert!(seen.len() > 1, "the audio body shows its peaks");
    println!("striawaveform: face scans={} after 3 paints body_colours={}", face.wave_scans(), seen.len());
}

// ---- STRIATITLECLIP (LEDGER SR828): a title card is a generated source — its still, its render, the face's word (named tail block) ----

/// The frames a render hands out, at their own size.
#[derive(Default)]
struct StriaTitleFrames(Vec<(i64, u32, u32, Vec<u8>)>);

impl RenderSink for StriaTitleFrames {
    fn frame(&mut self, out_t: i64, w: u32, h: u32, rgba: &[u8]) {
        self.0.push((out_t, w, h, rgba.to_vec()));
    }
    fn pcm(&mut self, _out_t: i64, _rate: u32, _ch: u16, _s: &[f32]) {}
}

/// No `sans` face on this machine: the glyph checks cannot run (said loudly, text_host's own skip rule).
fn striatitleclip_no_font() -> bool {
    let none = text_host::line::TextStyle::new(&["sans"], 16.0).primary().is_none();
    if none {
        println!("SKIP: striatitleclip: no `sans` face installed, the title's glyph checks did not run");
    }
    none
}

/// A 25 fps timeline: optional synth reel A (64x48, 50 ticks) at A[0,10)@0, then title T1 `text` (`dur` ticks) at 10 (or at
/// 0 without A), all on V1.
fn striatitleclip_timeline(with_a: bool, text: &str, dur: i64) -> (Timeline, Sources) {
    let mut tl = Timeline::new(25);
    let a = with_a.then(|| tl.add_source("A", 50));
    let t = tl.add_source("T1", dur);
    tl.sources[t.0 as usize].title = Some(text.to_string());
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = match a {
        Some(a) => vec![Clip::new(a, 0, 10, 0), Clip::new(t, 0, dur, 10)],
        None => vec![Clip::new(t, 0, dur, 0)],
    };
    tl.validate().unwrap();
    let mut src = Sources::new(&tl);
    if let Some(a) = a {
        src.add(a, synth_source(0, 50, 25)).unwrap();
    }
    (tl, src)
}

#[test]
fn striatitleclip_still_is_white_text_centred_on_black() {
    if striatitleclip_no_font() {
        return;
    }
    let s = stria::nle::title_still("Hello", 320, 160);
    assert_eq!((s.width, s.height, s.rgba.len()), (320, 160, 320 * 160 * 4));
    let px = |x: usize, y: usize| &s.rgba[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4];
    assert_eq!(px(0, 0), &[0, 0, 0, 255], "corner black");
    assert_eq!(px(319, 159), &[0, 0, 0, 255], "corner black");
    let lit = (0..320).filter(|&x| px(x, 80)[0] > 128).count();
    assert!(lit > 0, "the centre row carries ink");
    assert!(s.rgba.chunks(4).all(|p| p[3] == 255), "opaque");
    assert!(s.rgba.chunks(4).all(|p| p[0] == p[1] && p[1] == p[2]), "white on black only");
    println!("striatitleclip: Hello at 320x160: {lit} lit px on the centre row");
}

#[test]
fn striatitleclip_title_only_renders_at_title_size_from_the_cache() {
    let (tl, src) = striatitleclip_timeline(false, "Hello", 2);
    let mut a = StriaTitleFrames::default();
    let r = render(&tl, 0..2, &src, &mut a).unwrap();
    assert_eq!(r.frames, 2);
    let (w, h) = stria::nle::TITLE_SIZE;
    assert_eq!((w, h), (1280, 720));
    let still = stria::nle::title_still("Hello", w, h);
    for (i, f) in a.0.iter().enumerate() {
        assert_eq!((f.0, f.1, f.2), (i as i64, w, h));
        assert!(f.3 == still.rgba, "frame {i} is the still");
    }
    let mut b = StriaTitleFrames::default();
    render(&tl, 0..2, &src, &mut b).unwrap();
    assert!(a.0 == b.0, "the same text renders the same bytes");
    let (x, y) = (src.title_still_cached("Hello", w, h), src.title_still_cached("Hello", w, h));
    assert!(std::sync::Arc::ptr_eq(&x, &y), "the second ask is the cached still");
    assert!(!std::sync::Arc::ptr_eq(&x, &src.title_still_cached("Hello", 64, 48)), "another size is another still");
    println!("striatitleclip: title-only render {} frames at {w}x{h}, cached", r.frames);
}

#[test]
fn striatitleclip_title_beside_a_reel_takes_its_size() {
    let (tl, src) = striatitleclip_timeline(true, "Hello", 20);
    let mut f = StriaTitleFrames::default();
    render(&tl, 8..14, &src, &mut f).unwrap();
    assert_eq!(f.0.len(), 6);
    assert!(f.0.iter().all(|x| (x.1, x.2) == (64, 48)), "every frame at reel A's 64x48");
    let still = stria::nle::title_still("Hello", 64, 48);
    assert!(f.0[2..].iter().all(|x| x.3 == still.rgba), "ticks 10.. show the title");
    let mut only = StriaTitleFrames::default();
    render(&tl, 12..13, &src, &mut only).unwrap();
    assert_eq!((only.0[0].1, only.0[0].2), (64, 48), "a range holding only the title still sizes by reel A");
    println!("striatitleclip: title beside reel A renders at 64x48");
}

#[test]
fn striatitleclip_dissolve_onto_a_title_mixes() {
    let (tl, src) = striatitleclip_timeline(true, "Hello", 20);
    let tl = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
    let mut f = StriaTitleFrames::default();
    render(&tl, 10..15, &src, &mut f).unwrap();
    assert_eq!(f.0.len(), 5);
    let mid = &f.0[2].3;
    let title = &f.0[4].3;
    let mut alone = tl.clone();
    alone.tracks[0].transitions.clear();
    alone.tracks[0].clips = vec![Clip::new(SourceId(0), 0, 20, 0)];
    let mut a = StriaTitleFrames::default();
    render(&alone, 12..13, &src, &mut a).unwrap();
    assert!(mid != title, "the mid frame is refused theby title alonename");
    assert!(*mid != a.0[0].3, "the mid frame is not reel A alone");
    println!("striatitleclip: dissolve A -> T1 at 10 for 4: mid frame differs from both ends");
}

#[test]
fn striatitleclip_face_title_word_adds_a_title_at_the_playhead() {
    use stria::nle_face::{EditorState, Face, PixelOrder, VERBS, synth_fixture};
    assert!(VERBS.contains(&"title"));
    assert_eq!(VERBS.last(), Some(&"help"), "help stays last");
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    face.state.seek(12); // STRIAINSERTMARKS (SR905)
    assert!(face.verb("title Hello"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let t1 = face.state.timeline.source_by_reel("T1").expect("T1").clone();
    assert_eq!((t1.duration, t1.title.as_deref()), (125, Some("Hello")));
    let at = face.state.timeline.clip_at(0, 12).expect("a clip at the playhead");
    let c = face.state.timeline.tracks[0].clips[at];
    assert_eq!((c.src, c.at, c.src_in, c.src_out), (t1.id, 12, 0, 125));
    assert_eq!(face.state.selection, Some((0, at)));
    assert!(face.verb("title"));
    assert_eq!(face.state.timeline.source_by_reel("T2").and_then(|s| s.title.clone()).as_deref(), Some("Title"));
    assert!(face.state.undo());
    assert!(face.state.timeline.source_by_reel("T2").is_none(), "undo removes the source");
    assert!(face.state.timeline.source_by_reel("T1").is_some());
    let text = edl::write(&face.state.timeline, "T").unwrap();
    assert!(text.contains("* UNA SOURCE T1 00:00:05:00\n* UNA TITLE T1 \"Hello\"\n"), "{text}");
    assert_eq!(edl::parse(&text, 25).unwrap(), face.state.timeline, "the document reopens with its title");
    println!("striatitleclip: `title Hello` -> T1 125 ticks at 12; bare `title` -> T2 \"Title\"; undo drops T2");
}

// ---- STRIASNAPDRAG (LEDGER SR836): tick 0 and the in / out marks are snap targets on the face's drag (named tail block) ----

/// Rule: over NLE1's fixture with the in mark at 22 (the `in` verb), a press on V's A@25 body at `x_of(28) + 7` moved
/// 21 px left puts A's in edge 3 px right of the mark (raw -2 ticks): it snaps (delta -3, the guide at 22, painted as a
/// GUIDE column through the row), and the release commits A flush on the mark with the guide gone; undone, the same drag
/// pressed with Alt keeps the raw -2 and no guide.
#[test]
fn striasnapdrag_face_drag_snaps_to_the_in_mark_and_alt_does_not() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.seek(22);
    assert!(face.verb("in"));
    face.state.seek(0);
    assert_eq!((face.marks.take(), face.note()), (None, (Some(22), None));
    let tl0 = face.state.timeline.clone();
    let l = face.layout();
    let y = l.point(0, 28).1;
    let (x, to) = (l.x_of(28) as f64 + 7.0, l.x_of(28) as f64 - 14.0);
    let at = |px: &[u8], x: i64| {
        let i = (y as usize * l.w as usize + x as usize) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    face.on(&nle3eslipdrag_btn(true, x, y, 0));
    face.on(&bandy::SMessage::BrowserMouseMove(to, y, 0)));
    assert_eq!(l.tick_at(to) - l.tick_at(x), -2, "the raw delta");
    assert_eq!(face.draggingnote().map(|d| (d.kind, d.delta, d.guide)), Some((DragKind::Move, -3, Some(22))));
    let px = face.paint();
    assert_eq!(at(&px, l.x_of(22)), timeline_core::view::GUIDE, "the guide through the row");
    face.on(&nle3eslipdrag_btn(false, to, y, 0));
    assert!(face.dragging().is_none()); // STRIAXFADECURVEBUTTON (SR925): the message row reads the crossfade
    assert_eq!(face.state.timeline, tl0.move_clip(0, 2, 0, 22).unwrap(), "flush on the mark");
    assert_ne!(at(&face.paint(), l.x_of(22)), timeline_core::view::GUIDE, "the guide is gone after the release");
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl0);
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_ALT));
    face.on(&bandy::SMessage::BrowserMouseMove(to, y, 0));
    assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((-2, None)), "Alt: no snap");
    face.on(&nle3eslipdrag_btn(false, to, y, 0));
    assert_eq!(face.state.timeline, tl0.move_clip(0, 2, 0, 23).unwrap());
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("striasnapdrag: A@25 dragged 21 px left snaps to the in mark 22; Alt lands at 23");
}

// ---- STRIACLIPGAINFACE (LEDGER SR838): the face speaks the NLE4 gain / fades (named tail block) ----
// `gain-` / `gain+` / `gain [<dB>]` / `fade-in` / `fade-out` on the selected clip through timeline_core's ONE verb dispatch
// and the one commit; the message line reads the outcome; a refusal names its `EditError` and drops the edit; the EDL's
// existing `* UNA GAIN` / `* UNA FADE` writer carries the face-edited clip (pinned unchanged).

/// synth_fixture with the audio track's clips replaced by ONE 20-tick clip (A 13..33 at 0), selected.
fn striaclipgainface_face() -> stria::nle_face::Face {
    let (mut tl, src) = stria::nle_face::synth_fixture();
    let a = tl.source_by_reel("A").unwrap().id;
    tl.tracks[1].clips = vec![timeline_core::Clip::new(a, 13, 33, 0)];
    let mut face = stria::nle_face::Face::new(stria::nle_face::EditorState::new(tl), src, (320, 160), stria::nle_face::PixelOrder::Rgba, false);
    face.state.select(1, 0).unwrap();
    face
}

#[test]
fn striaclipgainface_gain_steps_read_on_the_message_line_and_undo() {
    let mut face = striaclipgainface_face();
    for _ in 0..3 {
        assert!(face.verb("gain+"));
    }
    assert_eq!(face.state.selected().unwrap().gain_cdb, 300);
    assert_eq!(face.last_error.take().as_deref(), Some("gain +3.0 dB"));
    assert_eq!(face.state.undo_depth(), 3);
    assert!(face.verb("undo"));
    assert_eq!(face.state.selected().unwrap().gain_cdb, 200);
    assert!(face.verb("undo") && face.verb("undo"));
    assert_eq!(face.state.selected().unwrap().gain_cdb, 0, "undo restores unity");
    assert!(face.verb("gain-"));
    assert_eq!(face.last_error.take().as_deref(), Some("gain -1.0 dB"));
    assert!(face.verb("gain"));
    assert_eq!(face.last_error.take().as_deref(), Some("gain 0.0 dB"));
    assert_eq!(stria::nle_face::db_words(-350), "-3.5 dB");
}

#[test]
fn striaclipgainface_gain_absolute_and_out_of_range_refused_by_name() {
    let mut face = striaclipgainface_face();
    assert!(face.verb("gain -3"));
    assert_eq!(face.last_error.take().as_deref(), Some("gain -3.0 dB"));
    let (tl, depth) = (face.state.timeline.clone(), face.state.undo_depth());
    assert!(face.verb("gain -200"));
    assert_eq!(face.last_error.take(), Some(format!("gain: {}", timeline_core::EditError::Gain)));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl, depth), "the edit dropped");
    assert_eq!(face.state.selected().unwrap().gain_cdb, -300);
    face.state.deselect();
    assert!(face.verb("gain+"));
    assert_eq!(face.last_error.take(), Some(format!("gain+: {}", timeline_core::EditError::NoClip)));
}

#[test]
fn striaclipgainface_fades_longer_than_the_clip_refused_and_cleared() {
    let mut face = striaclipgainface_face();
    assert!(face.verb("fade-in 10"));
    assert_eq!(face.last_error.take().as_deref(), Some("fade-in 10, fade-out 0"));
    assert!(face.verb("fade-out 15"));
    assert_eq!(face.last_error.take(), Some(format!("fade-out: {}", timeline_core::EditError::Fade)));
    let c = face.state.selected().unwrap();
    assert_eq!((c.fade_in, c.fade_out), (10, 0), "the clip keeps its fade-in");
    assert!(face.verb("fade-in 00:00:00:05"));
    assert_eq!(face.state.selected().unwrap().fade_in, 5);
    assert!(face.verb("fade-in 0"));
    assert_eq!(face.state.selected().unwrap().fade_in, 0, "fade-in 0 clears");
}

#[test]
fn striaclipgainface_face_edit_round_trips_the_edl_and_every_word_is_a_button() {
    let mut face = striaclipgainface_face();
    for v in ["gain+", "gain+", "gain+", "fade-in 10"] {
        assert!(face.verb(v));
    }
    let text = timeline_core::edl::write(&face.state.timeline, "GAIN").unwrap();
    assert!(text.contains("* UNA GAIN 300\n"), "{text}");
    assert!(text.contains("* UNA FADE 00:00:00:10 00:00:00:00\n"), "{text}");
    assert_eq!(timeline_core::edl::parse(&text, 25).unwrap(), face.state.timeline);
    for w in ["gain-", "gain+", "gain", "fade-in", "fade-out"] {
        assert!(stria::nle_face::VERBS.contains(&w), "{w}");
        assert!(face.verb(w), "{w}");
    }
    assert_eq!(stria::nle_face::VERBS.last(), Some(&"help"));
}

// ---- STRIATRACKMUTESOLO (LEDGER SR837): per-track mute / solo through the render, the export and the face (named tail block) ----

/// Reels A (base 0) and B (base 1000), 50 ticks each; one 20-tick clip per listed track (`(kind, reel, muted, solo)`).
fn striatrackmutesolo_tl(tracks: &[(Kind, u32, bool, bool)]) -> Timeline {
    let mut tl = Timeline::new(25);
    tl.add_source("A", 50);
    tl.add_source("B", 50);
    for &(k, reel, m, s) in tracks {
        let t = tl.add_track(k);
        tl.tracks[t].clips = vec![Clip::new(SourceId(reel), 0, 20, 0)];
        tl = tl.set_track_flags(t, m, s).unwrap();
    }
    tl
}

fn striatrackmutesolo_render(tl: &Timeline) -> Collect {
    let mut sink = Collect::default();
    render(tl, 0..20, &sources(tl), &mut sink).unwrap();
    sink
}

/// M3: A1 (reel A) muted under A2 (reel B) renders reel B's track alone, byte for byte; A1 soloed renders reel A's alone;
/// both muted decodes nothing (the schedule's runs carry no layers; the render takes its audio format from a scheduled
/// source, so it writes no PCM: today's all-gap behaviour).
#[test]
fn striatrackmutesolo_muted_audio_track_renders_the_other_alone() {
    use Kind::Audio as Au;
    let one_b = striatrackmutesolo_render(&striatrackmutesolo_tl(&[(Au, 1, false, false)])).pcm;
    let one_a = striatrackmutesolo_render(&striatrackmutesolo_tl(&[(Au, 0, false, false)])).pcm;
    assert_eq!(one_b.len(), 20 * PER);
    let muted = striatrackmutesolo_render(&striatrackmutesolo_tl(&[(Au, 0, true, false), (Au, 1, false, false)])).pcm;
    assert_eq!(muted, one_b, "A1 muted: A2 alone");
    let solo = striatrackmutesolo_render(&striatrackmutesolo_tl(&[(Au, 0, false, true), (Au, 1, false, false)])).pcm;
    assert_eq!(solo, one_a, "A1 solo: A1 alone");
    let both = striatrackmutesolo_tl(&[(Au, 0, true, false), (Au, 1, true, false)]);
    let mut sink = Pcm::default();
    let r = render(&both, 0..20, &sources(&both), &mut sink).unwrap();
    assert_eq!((r.seeks, r.pcm_frames, sink.pcm.len()), (0, 0, 0), "no muted layer is decoded");
    println!("striatrackmutesolo audio: muted == one-track ({} samples), solo == one-track, all muted pcm {}", muted.len(), sink.pcm.len());
}

/// M3: V2 (reel B) muted over V1 (reel A) shows V1's frames exactly; a muted V1 under V2's gap is black there; every video
/// track muted schedules black steps but the render refuses `NoSize` (no scheduled source sizes the frame, the all-gap rule);
/// the muted audio track's
/// meter feed (`track_pcm`) is all zero while the live one carries its source.
#[test]
fn striatrackmutesolo_muted_video_track_shows_the_lower() {
    use Kind::{Audio as Au, Video as Vi};
    let low = striatrackmutesolo_render(&striatrackmutesolo_tl(&[(Vi, 0, false, false)]));
    let top = striatrackmutesolo_render(&striatrackmutesolo_tl(&[(Vi, 0, false, false), (Vi, 1, false, false)]));
    let muted = striatrackmutesolo_render(&striatrackmutesolo_tl(&[(Vi, 0, false, false), (Vi, 1, true, false)]));
    assert_eq!(muted.frames.len(), 20);
    assert_eq!(muted.frames, low.frames, "V2 muted: the lower track shows");
    assert_ne!(top.frames, low.frames, "unmuted V2 covers V1");
    assert_eq!(TestPattern::read_counter(&muted.frames[5].1, 64, 48), Some(5));
    let mut gap = striatrackmutesolo_tl(&[(Vi, 0, true, false), (Vi, 1, false, false)]);
    gap.tracks[1].clips = vec![Clip::new(SourceId(1), 0, 10, 0)];
    let shown = striatrackmutesolo_render(&gap);
    assert_eq!(TestPattern::read_counter(&shown.frames[5].1, 64, 48), Some(1005), "V2's clip");
    assert!(shown.frames[10..].iter().all(|(_, f)| f.chunks(4).all(|p| p == [0, 0, 0, 255])), "V1 muted under V2's gap: black");
    let all = striatrackmutesolo_tl(&[(Vi, 0, true, false)]);
    let err = render(&all, 0..20, &sources(&all), &mut Collect::default()).err();
    assert!(matches!(err, Some(stria::nle::RenderError::NoSize)), "every video track muted: the all-gap rule ({err:?})");

    #[derive(Default)]
    struct Taps(std::collections::BTreeMap<usize, Vec<f32>>);
    impl RenderSink for Taps {
        fn frame(&mut self, _: i64, _: u32, _: u32, _: &[u8]) {}
        fn pcm(&mut self, _: i64, _: u32, _: u16, _: &[f32]) {}
        fn taps_tracks(&self) -> bool {
            true
        }
        fn track_pcm(&mut self, _: i64, track: usize, _: u32, _: u16, s: &[f32]) {
            self.0.entry(track).or_default().extend_from_slice(s);
        }
    }
    let tl = striatrackmutesolo_tl(&[(Au, 0, true, false), (Au, 1, false, false)]);
    let mut taps = Taps::default();
    render(&tl, 0..20, &sources(&tl), &mut taps).unwrap();
    assert!(taps.0[&0].iter().all(|&x| x == 0.0), "the muted track meters silence");
    assert!(taps.0[&1].iter().any(|&x| x != 0.0));
    println!("striatrackmutesolo video: V2 muted == V1 alone (20 frames), V1 muted black, muted meter silent");
}

/// M3: the export honours the same set: V2 + A2 muted exports the same RGBA stream (fnv) and the same audio.wav bytes as
/// the V1 + A1 timeline.
#[test]
fn striatrackmutesolo_export_of_the_muted_fixture_is_the_single_track_export() {
    use Kind::{Audio as Au, Video as Vi};
    use stria::nle_export::{AudioOut, RawSequence, export};
    let one = striatrackmutesolo_tl(&[(Vi, 0, false, false), (Au, 0, false, false)]);
    let muted = striatrackmutesolo_tl(&[(Vi, 0, false, false), (Vi, 1, true, false), (Au, 0, false, false), (Au, 1, true, false)]);
    let run = |tl: &Timeline, name: &str| {
        let dir = fresh_dir("striatrackmutesolo", name);
        let r = export(tl, 0..20, &sources(tl), &dir, Box::new(RawSequence), AudioOut::Wav).unwrap();
        let wav = std::fs::read(dir.join("audio.wav")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (r, wav)
    };
    let (r1, w1) = run(&one, "one");
    let (r2, w2) = run(&muted, "muted");
    assert_eq!((r2.frames, r2.pcm_frames, r2.fnv), (r1.frames, r1.pcm_frames, r1.fnv));
    assert_eq!(w2, w1, "audio.wav byte-equal");
    println!("striatrackmutesolo export: frames {} pcm {} fnv {:016x} wav {} bytes, muted == single", r2.frames, r2.pcm_frames, r2.fnv, w2.len());
}

/// M3: the face's `mute` toggles the selected clip's track through ONE commit (undo restores it, the document is dirty,
/// the EDL carries `* UNA MUTE A` and reads back equal); `solo` likewise; with nothing selected the word is refused by name.
#[test]
fn striatrackmutesolo_face_mute_then_undo() {
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    assert!(VERBS.contains(&"mute") && VERBS.contains(&"solo"));
    assert!(face.verb("mute"));
    assert_eq!(face.last_error.take(), Some(format!("mute: {}", timeline_core::EditError::NoTrack)));
    assert_eq!(face.state.timeline, tl0);
    face.state.select(1, 0).unwrap();
    assert!(face.verb("mute"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.state.timeline.tracks[1].muted && !face.state.timeline.tracks[0].muted);
    assert_eq!((face.state.undo_depth(), face.dirty(), face.state.selection), (1, true, Some((1, 0))));
    let text = edl::write(&face.state.timeline, "MUTE").unwrap();
    assert!(text.contains("\n* UNA MUTE A\n\n"), "{text}");
    assert_eq!(edl::parse(&text, 25).unwrap(), face.state.timeline);
    assert!(face.verb("solo"));
    assert!(face.state.timeline.tracks[1].solo);
    assert!(face.verb("undo") && face.verb("undo"));
    assert_eq!((&face.state.timeline, face.dirty()), (&tl0, false));
    println!("striatrackmutesolo face: no selection -> mute: {}; mute + solo then two undos restore", timeline_core::EditError::NoTrack);
}

// ---- STRIATHUMBNAILS (LEDGER SR839): a video clip's head is its src_in frame, decoded once per key (named tail block) ----

/// STRIATHUMBNAILS (SR839): the head still the face should hold for `(src, src_in)`: `preview` of a one-clip timeline at tick 0,
/// scaled by `view::head_still` (the face's own road, re-walked here).
fn striathumbnails_expected(tl: &Timeline, src: &Sources, id: SourceId, src_in: i64) -> timeline_core::view::Still {
    let mut one = tl.clone();
    one.tracks.clear();
    let v = one.add_track(Kind::Video);
    one.tracks[v].clips.push(Clip::new(id, src_in, src_in + 1, 0));
    let full = stria::nle_face::preview(&one, src, 0).unwrap().expect("a frame");
    timeline_core::view::head_still(&full).expect("a head")
}

/// STRIATHUMBNAILS (SR839): the box `(x0, y0)`, `w` x `h` of an RGBA canvas `w_canvas` wide, as bytes.
fn striathumbnails_box(buf: &[u8], w_canvas: u32, x0: i64, y0: i64, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..h as i64 {
        let i = ((y0 + y) as usize * w_canvas as usize + x0 as usize) * 4;
        out.extend_from_slice(&buf[i..i + w as usize * 4]);
    }
    out
}

/// STRIATHUMBNAILS (SR839): the synth fixture's face: V1 clip 0 (reel A, src_in 13) shows its src_in frame at the body's left
/// (the box == the cached head == the preview road's frame scaled), the box's dominant colour is the source frame's, the colour
/// bar continues to the right; three paints decode the three video heads once (the audio clips none). The line carries the
/// measured decode time per head (on the face's thread).
#[test]
fn striathumbnails_face_head_is_the_src_in_frame_decoded_once() {
    use timeline_core::view::{HEAD_H, clip_colour};
    let (tl, src) = synth_fixture();
    let expect = striathumbnails_expected(&tl, &src, SourceId(0), 13);
    // the decode cost of one head: the face's road (`preview` of a one-clip timeline + `head_still`), timed over the 3 heads
    let t0 = std::time::Instant::now();
    for (id, at) in [(SourceId(0), 13), (SourceId(1), 0), (SourceId(0), 23)] {
        let _ = striathumbnails_expected(&tl, &src, id, at);
    }
    let per_head = t0.elapsed() / 3;
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    assert_eq!(face.thumb_decodes(), 0, "Face::new decodes nothing");
    let t0 = std::time::Instant::now();
    let first = face.paint();
    let took = t0.elapsed(); // the whole first paint: the preview band, SR827's two peak scans and the three heads
    assert_eq!(face.thumb_decodes(), 3, "the first paint decodes the three video heads");
    assert!(!face.thumbs_pending());
    let head = face.thumb(SourceId(0), 13).expect("A@13 decoded").clone();
    assert_eq!((head.width, head.height), (48, HEAD_H));
    assert_eq!(head, expect, "the head is the preview road's src_in frame");
    let c = face.state.timeline.tracks[0].clips[0];
    let l = face.layout();
    let (bx, by) = (l.x_of(c.at) + 1, l.row_y(0) + 2);
    let got = striathumbnails_box(&first, l.w, bx, by, head.width, head.height);
    assert_eq!(got, head.rgba, "the head box shows the cached still 1:1");
    // the box's dominant colour is the frame's (the bars)
    let dominant = |px: &[u8]| {
        let mut n: Vec<([u8; 4], usize)> = Vec::new();
        for p in px.chunks(4) {
            let p = [p[0], p[1], p[2], p[3]];
            match n.iter_mut().find(|e| e.0 == p) {
                Some(e) => e.1 += 1,
                None => n.push((p, 1)),
            }
        }
        n.into_iter().max_by_key(|e| e.1).map(|e| e.0)
    };
    assert_eq!(dominant(&got), dominant(&expect.rgba));
    let bar = clip_colour(c.src.0, Kind::Video);
    let i = ((by + 10) as usize * l.w as usize + (bx + head.width as i64) as usize) * 4;
    assert_eq!(&first[i..i + 4], &bar, "the colour bar continues right of the box");
    let _ = face.paint();
    let third = face.paint();
    assert_eq!(face.thumb_decodes(), 3, "three paints, three decodes");
    assert_eq!(first, third, "the heads never change the picture between paints");
    println!("striathumbnails: head={}x{} box_match=1 decodes after 3 paints={} thumb_us={} per head first_paint_us={} (synth 64x48, face thread)", head.width, head.height, face.thumb_decodes(), per_head.as_micros(), took.as_micros());
}

/// STRIATHUMBNAILS (SR839): trimming V1 clip 0's in by 5 frames changes its key (A@13 -> A@18): the next paint decodes the new
/// head once and the box shows it.
#[test]
fn striathumbnails_trim_in_changes_the_head() {
    let (tl, src) = synth_fixture();
    let expect = striathumbnails_expected(&tl, &src, SourceId(0), 18);
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    let before = face.paint();
    let l = face.layout();
    let c = face.state.timeline.tracks[0].clips[0];
    let old_box = striathumbnails_box(&before, l.w, l.x_of(c.at) + 1, l.row_y(0) + 2, 48, 36);
    face.state.select(0, 0).unwrap();
    face.state.trim(stria::nle_face::Edge::In, 5).unwrap();
    face.state.deselect(); // the selection outline and trim handles paint over a body (the painter's order): off for the box read
    let c = face.state.timeline.tracks[0].clips[0];
    assert_eq!(c.src_in, 18);
    let after = face.paint();
    assert_eq!(face.thumb_decodes(), 4, "one more decode: the new key");
    assert_eq!(face.thumb(SourceId(0), 18), Some(&expect));
    let l = face.layout();
    // the trimmed body (5 ticks at zoom 8: 38 px) is narrower than the 48 px head: the box is the head's centre 38 columns
    let bw = (l.x_of(c.end()) - 1 - (l.x_of(c.at) + 1)) as u32;
    assert_eq!(bw, 38);
    let new_box = striathumbnails_box(&after, l.w, l.x_of(c.at) + 1, l.row_y(0) + 2, bw, 36);
    let c0 = ((48 - bw) / 2) as usize;
    let crop: Vec<u8> = expect.rgba.chunks(48 * 4).flat_map(|row| row[c0 * 4..(c0 + bw as usize) * 4].to_vec()).collect();
    assert_eq!(new_box, crop, "the box shows the frame at the new src_in, clipped to the body");
    assert_ne!(new_box, old_box[..].chunks(48 * 4).flat_map(|row| row[c0 * 4..(c0 + bw as usize) * 4].to_vec()).collect::<Vec<u8>>(), "the head changed with src_in");
    println!("striathumbnails: trim in +5 -> head A@{} decodes={}", c.src_in, face.thumb_decodes());
}

/// STRIATHUMBNAILS (SR839): ten 5-frame video clips (zoom 4, all on the canvas): the first paint decodes THUMBS_PER_PAINT (4),
/// the second 8, the third all 10; `thumbs_pending` (what keeps `serve`'s clock running on an idle face) is true until then.
#[test]
fn striathumbnails_ten_clips_fill_over_three_paints() {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let v = tl.add_track(Kind::Video);
    tl.tracks[v].clips = (0..10).map(|i| Clip::new(a, i * 5, i * 5 + 5, i * 5)).collect();
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    let mut st = EditorState::new(tl);
    st.zoom = 4;
    let mut face = Face::new(st, src, (320, 160), PixelOrder::Rgba, false);
    let mut seen = Vec::new();
    let mut pending = Vec::new();
    for _ in 0..3 {
        let _ = face.paint();
        seen.push(face.thumb_decodes());
        pending.push(face.thumbs_pending());
    }
    assert_eq!(seen, vec![4, 8, 10], "THUMBS_PER_PAINT per paint");
    assert_eq!(pending, vec![true, true, false], "pending until every visible head is decoded");
    assert_eq!(stria::nle_face::THUMBS_PER_PAINT, 4);
    let _ = face.paint();
    assert_eq!(face.thumb_decodes(), 10, "a fourth paint decodes nothing");
    println!("striathumbnails: ten clips decodes={}/{}/{} pending={pending:?}", seen[0], seen[1], seen[2]);
}

// ---- STRIACLIPSPEED (LEDGER SR829): per-clip speed through the schedule and the render (named tail block) ----

/// V1 = A[0,20)@0 at `rate` over the NLE1 reel A (utp1 counters 0..49): the counter each output tick shows, the schedule's
/// video seeks, the render report.
fn striaclipspeed_video(rate: u32, ticks: i64) -> (Vec<Option<u32>>, usize, stria::nle::RenderReport) {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0).at_rate(rate)];
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    let seeks = schedule(&tl, 0..ticks, &src).unwrap().iter().filter(|s| s.seek.is_some()).count();
    let mut sink = Collect::default();
    let r = render(&tl, 0..ticks, &src, &mut sink).unwrap();
    (sink.frames.iter().map(|f| TestPattern::read_counter(&f.1, 64, 48)).collect(), seeks, r)
}

#[test]
fn striaclipspeed_a_2x_clip_shows_every_other_frame_and_a_half_speed_clip_holds() {
    let (fast, seeks, r) = striaclipspeed_video(0x2_0000, 10);
    assert_eq!(fast, (0..10).map(|k| Some(2 * k)).collect::<Vec<_>>());
    assert_eq!((seeks, r.seeks, r.frames), (1, 1, 10), "one run, decoded and dropped forward");
    let (slow, seeks, r) = striaclipspeed_video(0x8000, 40);
    assert_eq!(slow, (0..40).map(|k| Some(k / 2)).collect::<Vec<_>>());
    assert_eq!((seeks, r.seeks), (1, 1), "one run, each frame held twice");
    println!("striaclipspeed: 2x over 20 -> 10 ticks, frames 0,2,..,18, seeks=1 runup={}; 0.5x -> 40 ticks, each frame twice, seeks=1", r.runup_frames);
}

/// The zero crossings (sign changes, 0 counted positive) of `pcm` past its first and last `skip` samples.
fn striaclipspeed_crossings(pcm: &[f32], skip: usize) -> usize {
    let body = &pcm[skip..pcm.len() - skip];
    body.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count()
}

/// A1 = T[0,25 * rate)@0 at `rate` over a 2 s mono 1 kHz tone (PCM16 at SYNTH_RATE): 25 output ticks of its render.
fn striaclipspeed_tone(rate: u32) -> Vec<f32> {
    let n = 2 * SYNTH_RATE as usize;
    let tone: Vec<i16> = (0..n).map(|i| (16_000.0 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / SYNTH_RATE as f64 + 0.3).sin()) as i16).collect();
    let file = build::mp4(&[build::pcm16_track(2, SYNTH_RATE, 1, &tone, 960)], &build::Mp4Options::default());
    let mut tl = Timeline::new(25);
    let t = tl.add_source("T", 50);
    let a = tl.add_track(Kind::Audio);
    let span = (25 * rate as i64) >> 16;
    tl.tracks[a].clips = vec![Clip::new(t, 0, span, 0).at_rate(rate)];
    let mut src = Sources::new(&tl);
    src.add(t, file).unwrap();
    let mut sink = Pcm::default();
    render(&tl, 0..25, &src, &mut sink).unwrap();
    assert_eq!((sink.rate, sink.pcm.len()), (SYNTH_RATE, 25 * PER));
    sink.pcm
}

#[test]
fn striaclipspeed_a_1khz_tone_at_2x_renders_at_2khz() {
    let (one, two) = (striaclipspeed_tone(0x1_0000), striaclipspeed_tone(0x2_0000));
    let (z1, z2) = (striaclipspeed_crossings(&one, 480), striaclipspeed_crossings(&two, 480));
    assert!((1900..2000).contains(&z1), "1x: {z1} crossings of a 1 kHz tone over 0.98 s");
    assert!((z2 as i64 - 2 * z1 as i64).abs() <= 2, "1x {z1} crossings, 2x {z2}");
    println!("striaclipspeed: tone 1x z={z1} 2x z={z2} (pitch follows speed through the one Sinc)");
}

#[test]
fn striaclipspeed_face_speed_word_halves_the_drawn_width_and_undo_restores() {
    assert_eq!(VERBS.iter().position(|v| *v == "speed").map(|i| VERBS[i + 1]), Some("title"), "speed sits right before title");
    assert_eq!(VERBS.last(), Some(&"help"), "help stays last");
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("speed"));
    assert_eq!(face.last_error.take(), Some(format!("speed: {}", EditError::NoClip)), "nothing selected");
    face.state.select(0, 0).unwrap();
    let width = |f: &Face| {
        let (l, c) = (f.layout(), f.state.selected().unwrap());
        l.x_of(c.end()) - l.x_of(c.at)
    };
    let w1 = width(&face);
    assert!(face.verb("speed 2"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let c = face.state.selected().unwrap();
    assert_eq!((c.rate_q16, c.at, c.len()), (0x2_0000, 0, 5));
    assert_eq!(width(&face) * 2, w1, "half the drawn width");
    assert!(face.state.undo());
    assert_eq!(face.state.timeline, tl, "undo restores");
    let mut seen = Vec::new();
    for _ in 0..3 {
        assert!(face.verb("speed"));
        seen.push(face.state.selected().unwrap().rate_q16);
    }
    assert_eq!(seen, vec![0x2_0000, 0x4_0000, 0x1_0000], "bare speed cycles 1x -> 2x -> 4x -> 1x");
    assert!(face.verb("speed 0"));
    assert_eq!(face.last_error.take(), Some(format!("speed 0: {}", EditError::Rate)));
    assert!(face.verb("speed 0.5"));
    assert_eq!(face.last_error.take(), Some(format!("speed 0.5: {}", EditError::Overlap)), "0.5x runs into B at 10");
    assert!(!face.verb("speed fast"), "an unreadable speed is not a face word");
    println!("striaclipspeed: `speed 2` -> width {w1} -> {}; undo restores; cycle {seen:x?}", w1 / 2);
}

/// The seat's pin: on the NLE1 synth fixture (every clip 1x) the schedule's video steps and audio runs are exactly the
/// pre-arc identity: each source tick is `src_in + (t - at)` and each seek falls where the old `src_t + 1` / `src_t + len`
/// continuity says (the kernel Player plays exactly this).
#[test]
fn striaclipspeed_a_1x_timeline_schedules_as_before() {
    let tl = timeline();
    let src = sources(&tl);
    assert!(tl.tracks.iter().flat_map(|t| t.clips.iter()).all(|c| c.rate_q16 == 0x1_0000));
    let steps = schedule(&tl, 0..35, &src).unwrap();
    let mut v_next: Option<(SourceId, i64)> = None;
    let mut vseeks = 0;
    for s in steps.iter().filter(|s| s.kind == Kind::Video) {
        match (s.track, s.src) {
            (Some(ti), Some(id)) => {
                let c = tl.tracks[ti].clips[tl.clip_at(ti, s.out_t).unwrap()];
                let want = c.src_in + (s.out_t - c.at);
                assert_eq!(s.src_t, want, "video tick {}", s.out_t);
                assert_eq!(s.seek.is_some(), v_next != Some((id, want)), "video seek at {}", s.out_t);
                vseeks += s.seek.is_some() as usize;
                v_next = Some((id, want + 1));
            }
            _ => v_next = None,
        }
    }
    let runs = timeline_core::schedule::audio_runs(&tl, 0..35, &src).unwrap();
    let mut next: Vec<Option<(SourceId, i64)>> = vec![None; tl.tracks.len()];
    let mut aseeks = 0;
    for r in &runs {
        for (ti, n) in next.iter_mut().enumerate() {
            if !r.layers.iter().any(|l| l.track == ti) {
                *n = None;
            }
        }
        for l in &r.layers {
            let c = tl.tracks[l.track].clips[tl.clip_at(l.track, r.out_t).unwrap()];
            let want = c.src_in + (r.out_t - c.at);
            assert_eq!(l.src_t, want, "audio run at {}", r.out_t);
            assert_eq!(l.seek.is_some(), next[l.track] != Some((l.src, want)), "audio seek at {}", r.out_t);
            aseeks += l.seek.is_some() as usize;
            next[l.track] = Some((l.src, want + r.len));
        }
    }
    assert_eq!((vseeks, aseeks), (3, 3));
    println!("striaclipspeed: 1x fixture schedule = the old identity: {} steps, {} runs, seeks v={vseeks} a={aseeks}", steps.len(), runs.len());
}

// ---- STRIAMARKERS (LEDGER SR835): named markers from the face — the words, the message, undo, the m key (named tail block) ----

fn striamarkers_face() -> Face {
    let (tl, src) = synth_fixture();
    Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
}

/// `mark` drops M1 at the playhead (ONE commit, the face dirty) with its message; `mark` again is refused by name and
/// changes nothing; `mark Intro` names one; `undo` removes it and `redo` restores it; `unmark` and unmark-of-nothing word
/// their messages; `mark in` stays SR723's export mark (no marker made).
#[test]
fn striamarkers_face_words_messages_and_undo() {
    let mut f = striamarkers_face();
    f.state.seek(12);
    assert!(f.verb("mark"));
    assert_eq!(f.last_error.take().as_deref(), Some("marked M1 at 00:00:00:12"));
    assert_eq!(f.state.timeline.markers, vec![timeline_core::Marker { at: 12, name: "M1".into(), colour: 0, len: 0 }]);
    assert_eq!(f.state.undo_depth(), 1);
    assert!(f.dirty(), "a marker dirties the document");
    assert!(f.verb("mark"));
    assert_eq!(f.last_error.take().as_deref(), Some("mark: M1 is already at 00:00:00:12"));
    assert_eq!((f.state.timeline.markers.len(), f.state.undo_depth()), (1, 1));
    f.state.seek(30);
    assert!(f.verb("mark Intro \"one\""));
    assert_eq!(f.last_error.take().as_deref(), Some("marked Intro \"one\" at 00:00:01:05"));
    assert_eq!(f.state.timeline.marker_at(30).map(|m| m.name.as_str()), Some("Intro \"one\""));
    assert!(f.verb("undo"));
    assert!(f.state.timeline.marker_at(30).is_none(), "undo removes the marker");
    assert!(f.verb("redo"));
    assert!(f.state.timeline.marker_at(30).is_some(), "redo restores it");
    assert!(f.verb("unmark"));
    assert_eq!(f.last_error.take().as_deref(), Some("unmarked Intro \"one\" at 00:00:01:05"));
    assert!(f.verb("unmark"));
    assert_eq!(f.last_error.take().as_deref(), Some("unmark: no marker at 00:00:01:05"));
    f.state.seek(5);
    assert!(f.verb("mark in"));
    assert_eq!((f.state.timeline.marks.0, f.state.timeline.markers.len()), (Some(5), 1), "mark in is the export mark");
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    let edl = edl::write(&f.state.timeline, "T").unwrap();
    assert!(edl.contains("* UNA MARK 5 -\n* UNA MARKER 00:00:00:12 \"M1\"\n"), "{edl}");
    println!("striamarkers: M1 at 12, Intro at 30 undone / redone / removed; EDL keeps `* UNA MARKER 00:00:00:12 \"M1\"`");
}

/// The `m` key (text) marks through the same word; the keys help shows its row; `M` stays unbound.
#[test]
fn striamarkers_m_key_and_help_row() {
    let mut f = striamarkers_face();
    assert!(f.on(&bandy::SMessage::BrowserText("m".into())));
    assert_eq!(f.last_error.take().as_deref(), Some("marked M1 at 00:00:00:00"));
    assert_eq!(f.state.timeline.markers.len(), 1);
    assert!(!f.on(&bandy::SMessage::BrowserText("M".into())));
    assert!(KEYS.contains(&("m", "mark")) && VERBS.contains(&"mark") && VERBS.contains(&"unmark"));
    assert_eq!(VERBS.last(), Some(&"help"));
    assert!(help_rows().iter().any(|(k, p)| k == "m" && *p == help_phrase("mark") && *p == "drop a marker at the playhead"));
    assert!(HELP.iter().any(|(a, _)| *a == "mark"));
    println!("striamarkers: m marks M1; help row m -> {}", help_phrase("mark"));
}

// ---- STRIARIPPLETRIM (LEDGER SR850): a Shift drag on the trim handle ripples (named tail block) ----

/// The synth face with V1 made three butted clips (A[13,23)@0 B[0,10)@10 A[38,48)@20 on 50-tick sources; A1 as the
/// fixture lays it), V1 clip `sel` selected, and the timeline it opened with.
fn striarippletrim_face(sel: usize) -> (Face, Timeline) {
    let (mut tl, src) = synth_fixture();
    tl.tracks[0].clips[2] = Clip::new(SourceId(0), 38, 48, 20);
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, sel).unwrap();
    (face, tl)
}

/// Rule: a Shift-press on V1 clip 0's out handle dragged 3 ticks left ripples: V1's clips 1 and 2 each move -3, A1 does
/// not move (the ripple is ONE track's), ONE undo step restores; the face equals a twin editor's `rtrim out -3`.
#[test]
fn striarippletrim_shift_drag_on_the_out_handle_ripples() {
    let (mut face, tl) = striarippletrim_face(0);
    let l = face.layout();
    let (x, y) = (l.x_of(10) as f64 - 2.0, l.point(0, 5).1);
    let dx = -3.0 * l.zoom as f64;
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_SHIFT));
    assert_eq!(face.dragging().map(|d| (d.kind, d.idx, d.ripple)), Some((DragKind::TrimOut, 0, true)));
    face.on(&bandy::SMessage::BrowserMouseMove(x + dx, y, 0));
    assert_eq!(face.dragging().map(|d| d.delta), Some(-3));
    face.on(&nle3eslipdrag_btn(false, x + dx, y, bandy::MOD_SHIFT));
    let mut twin = EditorState::new(tl.clone());
    twin.select(0, 0).unwrap();
    assert_eq!(twin.verb("rtrim out -3"), Some(Ok(())));
    assert_eq!(face.state.timeline, twin.timeline);
    let ats: Vec<i64> = face.state.timeline.tracks[0].clips.iter().map(|c| c.at).collect();
    assert_eq!(ats, vec![0, 7, 17], "V1's later clips follow");
    assert_eq!(face.state.timeline.tracks[1], tl.tracks[1], "A1 does not move");
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    println!("striarippletrim: Shift-drag -3 on V1 clip 0's out handle = rtrim out -3");
}

/// Rule: the same drag without Shift is the plain trim: V1's clips 1 and 2 stay (a gap opens).
#[test]
fn striarippletrim_plain_drag_leaves_the_gap() {
    let (mut face, tl) = striarippletrim_face(0);
    let l = face.layout();
    let (x, y) = (l.x_of(10) as f64 - 2.0, l.point(0, 5).1);
    nle3eslipdrag_drag(&mut face, x, y, -3.0 * l.zoom as f64, 0);
    assert_eq!(face.state.timeline, tl.trim_out(0, 0, 20).unwrap(), "A[13,23) out 3 earlier");
    let ats: Vec<i64> = face.state.timeline.tracks[0].clips.iter().map(|c| c.at).collect();
    assert_eq!(ats, vec![0, 10, 20], "the neighbours stay");
    println!("striarippletrim: the plain drag trims and leaves the gap");
}

/// Rule: a ripple past the source (V1 clip 2's out 48 dragged 3 ticks on a 50-tick source) is refused by name on the
/// message line through the drag's `{e}` road; the timeline and undo depth do not move.
#[test]
fn striarippletrim_refusal_reads_on_the_message_row() {
    let (mut face, tl) = striarippletrim_face(2);
    let l = face.layout();
    let (x, y) = (l.x_of(30) as f64 - 2.0, l.point(0, 25).1);
    nle3eslipdrag_drag(&mut face, x, y, 3.0 * l.zoom as f64, bandy::MOD_SHIFT | bandy::MOD_ALT);
    assert_eq!(face.last_error.take(), Some(format!("drag: {}", EditError::OutOfSource)));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl, 0));
    println!("striarippletrim: ripple +3 past A's end -> {}", EditError::OutOfSource);
}

/// Rule: Shift off the selected clip's handle keeps its meaning: a Shift-press on another clip's body adds it to the set
/// and begins the additive Move.
#[test]
fn striarippletrim_shift_elsewhere_still_adds() {
    let (mut face, _tl) = striarippletrim_face(0);
    let (x, y) = face.layout().point(0, 15);
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_SHIFT));
    assert_eq!(face.dragging().map(|d| (d.kind, d.additive, d.ripple)), Some((DragKind::Move, true, false)));
    assert_eq!((face.state.selection, face.state.set.clone()), (Some((0, 1)), vec![(0, 0), (0, 1)]));
    println!("striarippletrim: Shift on another clip's body still adds it");
}

// ---- STRIAROLLEDIT (LEDGER SR851): the roll words on the face (named tail block) ----

/// Rule: with V1 clip 0 selected (synth_fixture: A[13,23)@0 B[0,10)@10 A[23,33)@25), three `roll+` words roll the A|B cut
/// 3 ticks later (A [13,26)@0, B [3,10)@13), the third clip and A1 stay, ONE undo step per word; `roll-` steps back;
/// the two words sit in VERBS immediately before `help`.
#[test]
fn striarolledit_face_roll_words() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 0).unwrap();
    for _ in 0..3 {
        assert!(face.verb("roll+"));
    }
    assert_eq!(face.state.timeline, tl.roll(0, 0, 3).unwrap());
    assert_eq!(face.state.timeline.tracks[0].clips[..2], [Clip::new(SourceId(0), 13, 26, 0), Clip::new(SourceId(1), 3, 10, 13)]);
    assert_eq!(face.state.timeline.tracks[0].clips[2], tl.tracks[0].clips[2], "the third clip stays");
    assert_eq!(face.state.timeline.tracks[1], tl.tracks[1], "A1 does not move");
    assert_eq!(face.state.undo_depth(), 3);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl.roll(0, 0, 2).unwrap());
    assert!(face.verb("roll-"));
    assert_eq!(face.state.timeline, tl.roll(0, 0, 1).unwrap());
    let n = VERBS.len();
    assert_eq!(VERBS[n - 3..], ["roll-", "roll+", "help"]);
    println!("striarolledit: roll+ x3 on V1 clip 0 = Timeline::roll(0, 0, 3), one undo per word");
}

/// Rule: with nothing selected, or with a selected clip whose next clip is not butted (V1 clip 1 ends at 20, the next
/// starts at 25), the word is refused by name on the message row; the timeline and undo depth do not move.
#[test]
fn striarolledit_refusals_read_on_the_message_row() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("roll+"));
    assert_eq!(face.last_error.take(), Some(format!("roll+: {}", EditError::NoClip)));
    face.state.select(0, 1).unwrap();
    assert!(face.verb("roll+"));
    assert_eq!(face.last_error.take(), Some(format!("roll+: {}", EditError::NoClip)));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl, 0));
    println!("striarolledit: no selection / no butted next clip -> roll+: {}", EditError::NoClip);
}

// ---- STRIAINSERTOVERWRITE (LEDGER SR852): insert / overwrite from the face through timeline_core's dispatch (named tail block) ----

/// The two words are transport words just before `help` (still the last); with B selected and the playhead mid-B the
/// face's `insert` is `place` Insert of B 0..10 at the playhead on V1 (A1 not rippled), `overwrite` the Overwrite; the
/// message row reads the laid clip's reel line.
#[test]
fn striainsertoverwrite_face_words_are_place_at_the_playhead() {
    assert_eq!(VERBS[VERBS.len() - 8..], ["insert", "overwrite", "insert-marked", "overwrite-marked", "insert-all", "insert-marked-all", "target", "help"]); // STRIAINSERTMARKS (SR905): the two marked words before help; STRIAINSERTMARKSKEYSNEXT (SR985)
    for (word, mode) in [("insert", timeline_core::Mode::Insert), ("overwrite", timeline_core::Mode::Overwrite)] {
        let (tl, src) = synth_fixture();
        let mut face = nlefaceexport_face(tl.clone(), src);
        face.state.select(0, 1).unwrap();
        face.state.seek(15);
        assert!(face.verb(word), "{word}");
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        assert_eq!(face.state.timeline, tl.place(0, Clip::new(SourceId(1), 0, 10, 15), mode).unwrap(), "{word}");
        assert_eq!(face.state.timeline.tracks[1], tl.tracks[1], "{word}: only the target track moves");
        assert_eq!(face.state.selected(), Some(Clip::new(SourceId(1), 0, 10, 15)));
        assert_eq!(face.state.playhead, 25);
        assert_eq!(face.message_text(), Some(format!("{word}: B 00:00:00:10 at 00:00:00:15")), "the laid line"); // STRIAINSERTMARKSKEYS (SR924)
        assert!(face.verb("undo"));
        assert_eq!(face.state.timeline, tl, "{word}: one undo");
    }
    println!("striainsertoverwrite: insert / overwrite at 15 lay B 0..10 on V1; VERBS ends insert, overwrite, help");
}

/// No selection: the bare words are refused by the editor's name on the message line; the argument form by timecode lays
/// the named reel's range at the playhead on the reel's first track.
#[test]
fn striainsertoverwrite_face_refusal_and_timecode_form() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    for v in ["insert", "overwrite"] {
        assert!(face.verb(v), "{v}");
        assert_eq!(face.last_error.take(), Some(format!("{v}: {}", EditError::NoClip)));
    }
    assert!(face.verb("insert Z 0 5"));
    assert_eq!(face.last_error.take(), Some(format!("insert Z 0 5: {}", EditError::NoSource)));
    assert_eq!(face.state.timeline, tl);
    face.state.seek(10);
    assert!(face.verb("insert A 00:00:00:40 45") && face.last_error.take() == Some(format!("insert A 00:00:00:40 45: {}", EditError::InsertForm)), "a frame field past the rate is refused by name"); // STRIAINSERTMARKS (SR905)
    assert!(face.verb("insert A 00:00:01:15 00:00:01:20"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let want = tl.place(0, Clip::new(SourceId(0), 40, 45, 10), timeline_core::Mode::Insert).unwrap();
    assert_eq!(face.state.timeline, want);
    println!("striainsertoverwrite: no selection -> insert: {}; insert A 00:00:01:15 00:00:01:20 lays A 40..45 at 10 on V1", EditError::NoClip);
}

// ---- STRIAZOOMTOFIT (LEDGER SR859): `zoom-fit` / `zoom-sel` on the face (named tail block) ----

/// Over the synth fixture (35 ticks) at 320 px: `zoom-fit` is zoom 9 at origin 0 (the end inside the window), `zoom-sel`
/// fits B (10..20) at 32 from 10, then the set {A@0, B} at 16 from 0; never an undo step, never dirty; `undo` after a fit
/// leaves the zoom, `zoom-` halves it; a 20 px window takes zoom 1 at 0; nothing selected is refused by name.
#[test]
fn striazoomtofit_face_fits_the_window() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(VERBS.contains(&"zoom-fit") && VERBS.contains(&"zoom-sel"));
    let clean = |f: &Face| (f.state.undo_depth(), f.dirty(), f.state.timeline == tl);
    assert!(face.verb("zoom-sel"));
    assert_eq!(face.last_error.take(), Some(format!("zoom-sel: {}", EditError::NoClip)));
    assert_eq!(face.state.zoom, 8, "a refusal leaves the zoom");
    assert!(face.verb("zoom-fit"));
    let l = face.layout();
    assert_eq!((face.state.zoom, l.t0), (9, 0));
    assert!(l.x_of(tl.end()) <= l.w as i64, "the end inside the window");
    assert_eq!(clean(&face), (0, false, true));
    face.state.select(0, 1).unwrap();
    assert!(face.verb("zoom-sel"));
    let l = face.layout();
    assert_eq!((face.state.zoom, l.t0), (32, 10));
    assert!(l.x_of(20) <= l.w as i64);
    face.state.select_add(0, 0).unwrap();
    assert!(face.verb("zoom-sel"));
    assert_eq!((face.state.zoom, face.layout().t0), (16, 0));
    assert_eq!(clean(&face), (0, false, true), "a view change, not an edit");
    assert!(face.verb("zoom-fit"));
    assert!(face.verb("undo"));
    assert_eq!(face.state.zoom, 9, "undo is not a view step");
    assert!(face.verb("zoom-"));
    assert_eq!(face.state.zoom, 4);
    face.canvas = (20, 160);
    assert!(face.verb("zoom-fit"));
    assert_eq!((face.state.zoom, face.layout().t0), (1, 0));
    assert_eq!(face.last_error, None);
    assert_eq!(clean(&face), (0, false, true));
    println!("striazoomtofit: 320 px fit 9 @0, sel B 32 @10, set 16 @0; 20 px fit 1 @0; undo 0, clean");
}

// ---- STRIAMULTISELECT (LEDGER SR853): gain over the whole selection (named tail block) ----
// A click and a shift-click build NLE2c's set across V1 and A1; `gain+` / `gain <dB>` step / set every clip of it through
// timeline_core's `EditorState::gain_set` as ONE commit, so one undo restores the whole set.

#[test]
fn striamultiselect_shift_click_two_clips_gain_steps_both_one_undo() {
    let (tl, src) = stria::nle_face::synth_fixture();
    let mut face = stria::nle_face::Face::new(stria::nle_face::EditorState::new(tl), src, (320, 160), stria::nle_face::PixelOrder::Rgba, false);
    let tl0 = face.state.timeline.clone();
    let (x, y) = face.layout().point(0, 15); // V1's B[0,10)@10
    face.on(&nle3eslipdrag_btn(true, x, y, 0));
    face.on(&nle3eslipdrag_btn(false, x, y, 0));
    let (x, y) = face.layout().point(1, 30); // A1's A[23,33)@25
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_SHIFT));
    face.on(&nle3eslipdrag_btn(false, x, y, bandy::MOD_SHIFT));
    assert_eq!((face.state.selection, face.state.set.clone()), (Some((1, 2)), vec![(0, 1), (1, 2)]));
    assert!(face.verb("gain+"));
    let g = |f: &stria::nle_face::Face| f.state.timeline.tracks.iter().map(|t| t.clips.iter().map(|c| c.gain_cdb).collect::<Vec<_>>()).collect::<Vec<_>>();
    assert_eq!(g(&face), vec![vec![0, 100, 0], vec![0, 0, 100]], "both selected clips stepped, the rest unity");
    assert_eq!(face.last_error.take().as_deref(), Some("gain +1.0 dB"));
    assert_eq!(face.state.undo_depth(), 1, "ONE commit for the set");
    assert!(face.verb("gain -2"));
    assert_eq!(g(&face), vec![vec![0, -200, 0], vec![0, 0, -200]]);
    assert!(face.verb("undo"));
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl0, "one undo per word restores the whole set");
}

// ---- STRIATRANSITIONLENGTH (LEDGER SR870): the face reaches `dur <n>` through timeline_core's verb (named tail block) ----

/// M3: typed `dur 6` (the surface's Input) with no transition selected is refused by name; selected, the dissolve is 6
/// ticks in ONE undo step and the thumbnail's key carries it; `[` then steps the new length (the face's existing road).
#[test]
fn striatransitionlength_face_dur_input() {
    use bandy::SMessage;
    use stria::nle_face::SURFACE;
    let (tl, src) = synth_fixture();
    let tl0 = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    let input = |t: &str| SMessage::Input { target: SURFACE.into(), text: t.into() };
    assert!(face.on(&input("dur 6")));
    assert_eq!(face.last_error.take(), Some(format!("dur 6: {}", timeline_core::EditError::Transition)));
    assert_eq!(face.state.timeline, tl0);
    face.state.select_transition(0, 0).unwrap();
    assert!(face.on(&input("dur 6")));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl0.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 6).unwrap());
    assert_eq!(face.state.undo_depth(), 1);
    assert_eq!(face.transition_key().map(|k| k.2), Some(6));
    assert!(face.verb("["));
    assert_eq!(face.state.selected_transition().map(|x| x.dur), Some(5));
}

// ---- STRIAAUDIOCROSSFADE (LEDGER SR860): the face's `xfade` word (named tail block) ----

/// `xfade` is a transport word just before `help`; on the selected audio clip it puts THE equal-power record through ONE
/// commit (the fixture's cut at 10: half a second clamped to the incoming's 10 ticks); undo takes it back.
#[test]
fn striaaudiocrossfade_face_word_is_one_undo_step() {
    use timeline_core::Curve;
    assert!(VERBS.contains(&"xfade")); // STRIAXFADECURVEBUTTON (SR925): siblings add words before help, so no position index
    assert_eq!(VERBS.last(), Some(&"help"));
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(1, 1).unwrap();
    assert!(face.verb("xfade"));
    assert_eq!(face.note.take().as_deref(), Some("xfade: 10 equal")); // STRIAXFADECURVEBUTTON (SR925): the message row reads the crossfade
    assert_eq!(face.state.timeline, tl.add_audio_xfade(1, 10, 10, Curve::EqualPower).unwrap());
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    println!("striaaudiocrossfade: VERBS has xfade, help last of {}; (1,1) -> 10 ticks EqualPower at 10; undo", VERBS.len());
}

/// A refused `xfade` reads the editor's name on the message line (`{v}: {e}`) and moves nothing.
#[test]
fn striaaudiocrossfade_face_refusal_on_the_message_line() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("xfade"));
    assert_eq!(face.last_error.take().as_deref(), Some("xfade: no clip selected (select the incoming audio clip)")); // STRIAXFADECURVE (SR904): the audio wording
    face.state.select(0, 1).unwrap();
    assert!(face.verb("xfade"));
    assert_eq!(face.last_error.take().as_deref(), Some("xfade: the selected clip is not on an audio track")); // STRIAXFADECURVE (SR904): the audio wording
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl, 0));
    println!("striaaudiocrossfade: no selection and a video clip each refused on the message line in the audio wording"); // STRIAXFADECURVE (SR904)
}

// ---- STRIAXFADECURVE (LEDGER SR904): the face's crossfade curve words and the audio refusal text (named tail block) ----

/// `xfade linear` on the selected audio clip puts a LINEAR record of the default length through ONE commit, `xfade equal`
/// swaps the curve keeping the length, `xfade 6 linear` replaces it; refusals read the audio wording and move nothing.
#[test]
fn striaxfadecurve_face_curve_word_and_refusal_text() {
    use timeline_core::Curve;
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(1, 1).unwrap();
    assert!(face.verb("xfade linear"));
    assert_eq!(face.last_error, None);
    assert_eq!(face.state.timeline, tl.add_audio_xfade(1, 10, 10, Curve::Linear).unwrap());
    assert!(face.verb("xfade equal"));
    assert_eq!(face.state.timeline, tl.add_audio_xfade(1, 10, 10, Curve::EqualPower).unwrap());
    assert!(face.verb("xfade 6 linear"));
    assert_eq!(face.state.timeline, tl.add_audio_xfade(1, 10, 6, Curve::Linear).unwrap());
    assert_eq!(face.state.undo_depth(), 3);
    let (before, depth) = (face.state.timeline.clone(), face.state.undo_depth());
    for (v, want) in [
        ("xfade sine", "xfade sine: usage: xfade [<ticks>] [linear|equal], ticks at least 1"),
        ("xfade 99 equal", "xfade 99 equal: the crossfade is longer than the incoming clip or the outgoing clip's handle"),
    ] {
        assert!(face.verb(v));
        assert_eq!(face.last_error.take().as_deref(), Some(want), "{v}");
    }
    face.state.select(1, 2).unwrap();
    assert!(face.verb("xfade linear"));
    assert_eq!(face.last_error.take().as_deref(), Some("xfade linear: no clip ends where the selected clip starts"));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&before, depth));
    println!("striaxfadecurve: linear 10 -> equal 10 -> linear 6 (3 undo steps); sine / 99 / no butting clip refused in words");
}

// ---- STRIATIMECODEDISPLAY (LEDGER SR872): the playhead readout on the message row's left half — `tc`, a view word (named tail block) ----

/// The message row of a 320x144 labelled face split at the middle: (left half, right half) pixels of `px`, and the rows
/// outside the message row.
fn striatimecodedisplay_halves(px: &[u8], m: stria::nle_face::Layout) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (mut left, mut right, mut rest) = (Vec::new(), Vec::new(), Vec::new());
    for y in 0..m.h {
        let row = &px[(y * m.w * 4) as usize..((y + 1) * m.w * 4) as usize];
        if y >= m.message_y && y < m.message_y + m.message_h {
            left.extend_from_slice(&row[..(m.w / 2 * 4) as usize]);
            right.extend_from_slice(&row[(m.w / 2 * 4) as usize..]);
        } else {
            rest.extend_from_slice(row);
        }
    }
    (left, right, rest)
}

fn striatimecodedisplay_face(labels: bool) -> Face {
    let (tl, src) = synth_fixture();
    Face::new(EditorState::new(tl), src, (320, 144), PixelOrder::Rgba, labels)
}

/// `tc` is a transport word right before `help`; four presses cycle off -> timecode -> frames -> seconds -> off; it pushes no
/// undo step, leaves the document clean, the timeline, selection and playhead untouched and no message.
#[test]
fn striatimecodedisplay_tc_cycles_as_a_view_word() {
    use stria::nle_face::TcUnit;
    assert_eq!(&VERBS[VERBS.len() - 2..], &["tc", "help"]);
    let mut face = striatimecodedisplay_face(false);
    face.state.seek(12);
    let (tl, undo, sel) = (face.state.timeline.clone(), face.state.undo_depth(), face.state.selection);
    assert_eq!((face.tc_unit(), face.tc_readout()), (None, None), "off by default");
    let mut seen = Vec::new();
    for _ in 0..4 {
        assert!(face.verb("tcShift+"));
        seen.push((face.tc_unit(), face.tc_readout()));
    }
    assert_eq!(seen, vec![
        (Some(TcUnit::Timecode), Some("00:00:00:12".to_string())),
        (Some(TcUnit::Frames), Some("12".to_string())),
        (Some(TcUnit::Seconds), Some("0.48 s".to_string())),
        (None, None),
    ]);
    assert_eq!((face.state.undo_depth(), face.dirty(), face.state.selection, face.state.playhead), (undo, false, sel, 12));
    assert_eq!(face.state.timeline, tl);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("striatimecodedisplay: tc cycle {seen:?} undo={undo} dirty=false");
}

/// Labels on: off paints are today's; on with no message the readout inks the row's left half only (the right half and every
/// other row as off); with the reel line it moves to the right half; back off is byte-identical; labels off: nothing paints.
#[test]
fn striatimecodedisplay_readout_left_half_and_off_is_byte_identical() {
    use timeline_core::view::BG;
    let bg = |v: &[u8]| v.chunks(4).all(|p| p == BG);
    let mut face = striatimecodedisplay_face(true);
    let m = face.layout().with_message(true);
    assert!(m.message_h > 0);
    let off = face.paint();
    let (l0, r0, rest0) = striatimecodedisplay_halves(&off, m);
    assert!(bg(&l0) && bg(&r0), "off, no message: the row is BG");
    assert!(face.verb("tc"));
    let on = face.paint();
    let (l1, r1, rest1) = striatimecodedisplay_halves(&on, m);
    assert!(!bg(&l1), "the readout inks the left half");
    assert!(bg(&r1), "the right half stays BG");
    assert_eq!(rest1, rest0, "nothing outside the message row moves");
    face.state.select(0, 0).unwrap();
    let sel_on = face.paint();
    let (l2, r2, _) = striatimecodedisplay_halves(&sel_on, m);
    assert_eq!(l2, l1, "the readout's half is the readout's alone");
    assert!(!bg(&r2), "the reel line moves to the right half");
    for _ in 0..3 {
        assert!(face.verb("tc"));
    }
    assert!(face.tc_unit().is_none());
    let sel_off = face.paint();
    let (l3, _, _) = striatimecodedisplay_halves(&sel_off, m);
    assert_ne!(l3, l1, "off, the reel line is back at x 4");
    face.state.deselect();
    assert_eq!(face.paint(), off, "off again: byte-identical");
    let mut plain = striatimecodedisplay_face(false);
    let a = plain.paint();
    assert!(plain.verb("tc"));
    assert_eq!(plain.paint(), a, "labels off: no row, no readout");
    println!("striatimecodedisplay: row {}+{} split x {}", m.message_y, m.message_h, m.w / 2);
}

/// The readout follows the playhead: a seek changes the text and the left half's ink.
#[test]
fn striatimecodedisplay_readout_follows_the_playhead() {
    let mut face = striatimecodedisplay_face(true);
    let m = face.layout().with_message(true);
    assert!(face.verb("tc"));
    let (a, ta) = (face.paint(), face.tc_readout());
    face.state.seek(30);
    let (b, tb) = (face.paint(), face.tc_readout());
    assert_eq!((ta.as_deref(), tb.as_deref()), (Some("00:00:00:00"), Some("00:00:01:05")));
    assert_ne!(striatimecodedisplay_halves(&a, m).0, striatimecodedisplay_halves(&b, m).0);
    println!("striatimecodedisplay: playhead 0 -> 30: {ta:?} -> {tb:?}");
}

// ---- STRIACOPYPASTE (LEDGER SR854): copy / cut / paste through the face's words (named tail block) ----

/// The three words are VERBS before `help`; through `Face::verb` (its fall-through onto timeline_core's ONE dispatch)
/// copy + paste lays the selected clip at the playhead on its track as ONE undo step, and cut lifts it (the hole stays)
/// with a paste back into the hole restoring the timeline.
#[test]
fn striacopypaste_face_copy_cut_paste() {
    assert!(["copy", "cut", "paste"].iter().all(|w| VERBS.contains(w)) && VERBS.last() == Some(&"help"));
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("copy"));
    assert!(!face.dirty(), "copy is no edit");
    face.state.seek(35);
    assert!(face.verb("paste"));
    assert_eq!(face.state.timeline.tracks[0].clips[3], Clip::new(SourceId(1), 0, 10, 35));
    assert_eq!((face.state.undo_depth(), face.dirty(), face.last_error.clone()), (1, true, None));
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("cut"));
    assert_eq!(face.state.timeline, tl.lift(0, 1).unwrap());
    face.state.seek(10);
    assert!(face.verb("paste"));
    assert_eq!(face.state.timeline, tl, "pasted back into its hole");
    println!("striacopypaste: copy / paste at 35, cut / paste back at 10; undo depth {}", face.state.undo_depth());
}

/// Each refusal lands on the message line by the editor's name (`{v}: {e}`); the timeline does not move.
#[test]
fn striacopypaste_face_refusals_name_the_error() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("paste"));
    assert_eq!(face.last_error.take(), Some(format!("paste: {}", EditError::NoClip)));
    assert!(face.verb("copy"));
    assert_eq!(face.last_error.take(), Some(format!("copy: {}", EditError::NoClip)));
    face.state.select(0, 1).unwrap();
    assert!(face.verb("copy"));
    face.state.seek(12);
    assert!(face.verb("paste"));
    assert_eq!(face.last_error.take(), Some(format!("paste: {}", EditError::Overlap)));
    face.state.seek(35);
    face.state.select(1, 0).unwrap();
    assert!(face.verb("paste"));
    assert_eq!(face.last_error.take(), Some(format!("paste: {}", EditError::Kind)));
    assert_eq!(face.state.timeline, tl);
    println!("striacopypaste: paste over a clip -> paste: {}", EditError::Overlap);
}

// ---- STRIAGAPCLOSE (LEDGER SR857): close-gaps / gap <n> on the host face through the one dispatch (named tail block) ----

/// The synth fixture's V1 (A@0, B@10, A@25: a gap 20..25): `close-gaps` (the Close Gaps button's word) closes it through
/// `Face::verb`'s fall-through into timeline_core's `EditorState::verb` (ONE commit; undo restores), `gap 5` opens 5 ticks at
/// the playhead; `close-gaps` sits right before `help` in VERBS; with nothing selected the refusal is named on the message line.
#[test]
fn striagapclose_face_words_ride_the_one_dispatch() {
    let i = VERBS.iter().position(|&v| v == "close-gaps").expect("close-gaps in VERBS");
    assert_eq!(VERBS[i + 1], "help");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("close-gaps"));
    assert_eq!(face.last_error.take(), Some(format!("close-gaps: {}", EditError::NoClip)));
    assert_eq!(face.state.timeline, tl);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("close-gaps"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl.close_gaps(0).unwrap());
    assert_eq!(face.state.timeline.tracks[0].clips[2].at, 20);
    assert_eq!(face.state.timeline.tracks[1], tl.tracks[1]);
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    face.state.seek(10);
    assert!(face.verb("gap 5"));
    assert_eq!(face.state.timeline, tl.insert_gap(0, 10, 5).unwrap());
    assert_eq!(face.state.timeline.tracks[0].clips[1].at, 15);
    println!("striagapclose: VERBS[{i}] = close-gaps; V1 after gap 5 at 10: {:?}", face.state.timeline.tracks[0].clips);
}

// ---- STRIANUDGE (LEDGER SR855): Shift+Left / Shift+Right nudge the selection SLIP_STEP frames; the message reads the new `at` (named tail block) ----

/// Shift+Right moves the selected clip SLIP_STEP frames as ONE commit (timeline_core's `nudge 10` = `move_many`), the
/// message line reads its new `at`; Shift+Left moves it back; an overlap is refused by name with the state kept; with
/// nothing selected the face says so.
#[test]
fn strianudge_shift_arrows_step_ten_through_one_commit() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    let tl0 = f.state.timeline.clone();
    f.state.select(0, 2).unwrap();
    assert!(f.on(&bandy::SMessage::BrowserKey("Shift+Right".into())));
    assert_eq!(f.state.timeline, tl0.move_many(&[(0, 2)], SLIP_STEP).unwrap());
    assert_eq!((f.state.undo_depth(), f.state.selection), (1, Some((0, 2))));
    assert_eq!(f.message_text().as_deref(), Some("nudge: at 35 (00:00:01:10)"));
    assert!(f.on(&bandy::SMessage::BrowserKey("Shift+Left".into())));
    assert_eq!((&f.state.timeline, f.state.undo_depth()), (&tl0, 2));
    assert_eq!(f.last_error.take().as_deref(), Some("nudge: at 25 (00:00:01:00)"));
    f.state.select(0, 1).unwrap();
    assert!(f.on(&bandy::SMessage::BrowserKey("Shift+Right".into())));
    assert_eq!(f.last_error.take().as_deref(), Some("nudge: clips overlap"));
    assert_eq!((&f.state.timeline, f.state.undo_depth()), (&tl0, 2));
    f.state.deselect();
    assert!(f.on(&bandy::SMessage::BrowserKey("Shift+Left".into())));
    assert_eq!(f.last_error.take().as_deref(), Some("nudge: no such clip"));
    assert_eq!(f.state.timeline, tl0);
    println!("strianudge: Shift+Right 25 -> 35, Shift+Left back, overlap and no-clip refused, undo depth 2");
}

/// The two KEYS rows (no VERBS word, no button) have their help phrases; Right still moves one frame and its message
/// reads the new `at`; a set of two says its count; `,` is still SR737's slip; other counts are not face words.
#[test]
fn strianudge_keys_help_and_the_collisions() {
    let rows = help_rows();
    for (k, w, p) in [("Shift+Left", "nudge-10", "move the selection 10 frames earlier"), ("Shift+Right", "nudge+10", "move the selection 10 frames later")] {
        assert!(KEYS.contains(&(k, w)), "{k}");
        assert!(!VERBS.contains(&w), "{w}: a key's word, no button");
        assert_eq!(help_phrase(w), p);
        let row = rows.iter().find(|r| r.0.split(" / ").any(|x| x == k)).expect("a help row");
        assert_eq!(row.1, p);
    }
    assert!(KEYS.contains(&(",", "slip-")) && KEYS.contains(&(".", "slip+")), "SR737's keys kept");
    let mut f = nle5dkeyshelp_face(640, 480, false);
    let tl0 = f.state.timeline.clone();
    f.state.select(0, 2).unwrap();
    assert!(f.on(&bandy::SMessage::BrowserKey("Right".into())));
    assert_eq!(f.state.timeline, tl0.move_many(&[(0, 2)], 1).unwrap());
    assert_eq!(f.last_error.take().as_deref(), Some("nudge: at 26 (00:00:01:01)"));
    assert!(f.verb("undo"));
    f.state.select(0, 1).unwrap();
    f.state.select_add(0, 2).unwrap();
    assert!(f.on(&bandy::SMessage::BrowserText("\u{f703}".into())));
    assert_eq!(f.state.timeline, tl0.move_many(&[(0, 1), (0, 2)], 1).unwrap());
    assert_eq!(f.last_error.take().as_deref(), Some("nudge: 2 clips, at 26 (00:00:01:01)"));
    let tl1 = f.state.timeline.clone();
    for v in ["nudge+7", "nudge10", "nudge-10x"] {
        assert!(!f.verb(v), "{v} is not a face word");
    }
    assert_eq!(f.state.timeline, tl1);
    println!("strianudge: help rows={} keys={}; Right one frame, a set of two says its count", rows.len(), KEYS.len());
}

// ---- STRIAEXPORTPRESETS (LEDGER SR871): the export presets through the face's verb and the one table (named tail block) ----

#[test]
fn striaexportpresets_table_is_the_two_tables_words() {
    use stria::nle_export::{PRESETS, VIDEO_NAMES, audio_named, preset_named, preset_words};
    for p in &PRESETS {
        assert!(VIDEO_NAMES.contains(&p.video), "{}: {} is a video word", p.name, p.video);
        assert!(audio_named(p.audio).is_some(), "{}: {} is an audio word", p.name, p.audio);
        assert_eq!(preset_named(p.name), Some(p), "{}: by name", p.name);
        let o = p.opts();
        assert_eq!((o.qp, o.crf, o.scenecut, o.aq, o.bitrate, o.gop), (p.qp, p.crf, p.scenecut, p.aq, None, None), "{}", p.name);
    }
    assert_eq!(preset_words(), "web|archive|proxy");
    assert_eq!(preset_named("gif"), None);
}

#[test]
fn striaexportpresets_archive_is_png_wav() {
    let (pre, words) = (nlefaceexport_dir("preset-archive"), nlefaceexport_dir("preset-archive-words"));
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} --preset archive", pre.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the result line");
    assert!(line.starts_with("export: 35 frames, ") && line.contains(" png + wav in "), "{line}");
    assert!(face.verb(&format!("export {} png wav", words.display())));
    assert!(face.wait_export());
    assert!(face.last_error.take().is_some_and(|l| l.starts_with("export: 35 frames, ")));
    for f in ["000000.png".to_string(), "000034.png".to_string(), "audio.wav".to_string()] {
        assert_eq!(std::fs::read(pre.join(&f)).unwrap(), std::fs::read(words.join(&f)).unwrap(), "{f}: the preset is its words");
    }
    println!("striaexportpresets: archive {line}");
}

#[test]
fn striaexportpresets_proxy_is_the_tables_encoder() {
    use stria::nle_export::{Container, audio_named, export_as_with, preset_named, video_named_with};
    let (pre, direct) = (nlefaceexport_dir("preset-proxy"), nlefaceexport_dir("preset-proxy-direct"));
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} --preset proxy", pre.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the result line");
    assert!(line.starts_with("export: 35 frames, ") && line.contains(" h264 + aac:64 in "), "{line}");
    let p = preset_named("proxy").unwrap();
    let (tl, src) = synth_fixture();
    std::fs::create_dir_all(&direct).unwrap();
    let enc = stria::nle_export::scaled(video_named_with(p.video, tl.tb, p.opts()).unwrap(), p.scale); // STRIAEXPORTPRESETSNEXT (SR989): the table's scale too
    export_as_with(&tl, 0..tl.end(), &src, &direct, enc, audio_named(p.audio).unwrap(), Container::Mp4, &mut |_, _| Ok(())).unwrap();
    assert_eq!(std::fs::read(pre.join("movie.mp4")).unwrap(), std::fs::read(direct.join("movie.mp4")).unwrap(), "the face's preset is the table's encoder, byte-exact");
    println!("striaexportpresets: proxy {line}");
}

#[test]
fn striaexportpresets_refused_by_name() {
    let root = nlefaceexport_dir("preset-refused");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    let d = root.display().to_string();
    for (v, want) in [
        (format!("export {d} --preset"), "export: --preset needs one of web|archive|proxy".to_string()),
        (format!("export {d} --preset gif"), "export: unknown preset gif (web|archive|proxy)".to_string()),
        (format!("export {d} png --preset web"), "export: --preset web takes only [<dir>]".to_string()),
    ] {
        assert!(face.verb(&v), "{v}");
        assert_eq!(face.last_error.take(), Some(want), "{v}");
        assert!(!face.exporting() && !root.exists(), "{v}: a refused preset makes nothing");
    }
    assert!(face.verb(&format!("export {d} png wav extra")));
    assert!(face.last_error.take().is_some_and(|u| u.starts_with("export: usage: export [--all] [<dir>] [") && u.ends_with(" [--preset web|archive|proxy]")), "the usage names the presets");
}

// ---- STRIASLIPVIEW (LEDGER SR858): the slip drag previews the clip's new first and last frame (named tail block) ----
use stria::nle_face::{Role, Still, preview, slip_split};

/// The left and right cells of a side-by-side pair.
fn striaslipview_halves(s: &Still) -> (Vec<u8>, Vec<u8>) {
    let (w, row) = (s.width as usize / 2 * 4, s.width as usize * 4);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for y in 0..s.height as usize {
        l.extend_from_slice(&s.rgba[y * row..y * row + w]);
        r.extend_from_slice(&s.rgba[y * row + w..(y + 1) * row]);
    }
    (l, r)
}

/// The frames a twin editor's timeline shows at `t0` and `t1` (the synth fixture's reels).
fn striaslipview_frames(tl: &Timeline, t0: i64, t1: i64) -> (Still, Still) {
    let (_, src) = synth_fixture();
    (preview(tl, &src, t0).unwrap().expect("a frame"), preview(tl, &src, t1).unwrap().expect("a frame"))
}

/// Ctrl-press `(x, y)` and move by `dx` (the drag stays live).
fn striaslipview_press_move(face: &mut Face, x: f64, y: f64, dx: f64, mods: u8) {
    face.on(&nle3eslipdrag_btn(true, x, y, mods));
    face.on(&bandy::SMessage::BrowserMouseMove(x + dx, y, 0));
}

/// Rule: a Ctrl-drag of 40 px on B (SR737's pair, B[0,10)@10: slip 5) shows, on the next paint, B's NEW first and last
/// frame side by side: the left cell is a twin's frame at tick 10 after `slip 5`, the right its frame at 19, the left
/// differs from the unslipped frame at 10, and the playhead's frame is not re-rendered while the pair shows.
#[test]
fn striaslipview_slip_drag_shows_the_new_first_and_last_frame() {
    let (mut face, mut twin, tl) = nle3eslipface_pair();
    face.state.seek(12);
    face.paint();
    let before = face.still().cloned().expect("the playhead frame at 12");
    let (x, y) = face.layout().point(0, 15);
    striaslipview_press_move(&mut face, x, y, 40.0, bandy::MOD_CONTROL);
    face.paint();
    assert_eq!(twin.verb("slip 5"), Some(Ok(())));
    let (first, last) = striaslipview_frames(&twin.timeline, 10, 19);
    let pair = face.slip_view().cloned().expect("a pair during the slip");
    assert_eq!((pair.width, pair.height), (first.width * 2, first.height));
    let (l, r) = striaslipview_halves(&pair);
    assert_eq!((l, r), (first.rgba.clone(), last.rgba.clone()), "left = new first frame, right = new last frame");
    let (old_first, _) = striaslipview_frames(&tl, 10, 19);
    assert_ne!(first.rgba, old_first.rgba, "the slip moved the first frame");
    assert_eq!(face.still(), Some(&before), "the playhead frame is not re-rendered while the pair shows");
    assert_eq!(face.state.timeline, tl, "nothing committed during the drag");
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("striaslipview: slip 5 on B[0,10)@10 -> pair {}x{} (frames at 10 and 19)", pair.width, pair.height);
}

/// Rule: the release commits ONE slip (undo depth 1, SR760's road unchanged) and the next paint shows no pair; Esc
/// mid-drag commits nothing and clears the pair.
#[test]
fn striaslipview_release_and_escape_clear_the_pair() {
    let (mut face, mut twin, tl) = nle3eslipface_pair();
    let (x, y) = face.layout().point(0, 15);
    striaslipview_press_move(&mut face, x, y, 40.0, bandy::MOD_CONTROL);
    face.paint();
    assert!(face.slip_view().is_some());
    face.on(&nle3eslipdrag_btn(false, x + 40.0, y, bandy::MOD_CONTROL));
    assert_eq!(twin.verb("slip 5"), Some(Ok(())));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&twin.timeline, 1));
    face.paint();
    assert!(face.slip_view().is_none(), "released: no pair");
    assert!(face.verb("undo"));
    striaslipview_press_move(&mut face, x, y, 40.0, bandy::MOD_CONTROL);
    face.paint();
    assert!(face.slip_view().is_some());
    face.on(&bandy::SMessage::BrowserKey("Escape".into()));
    face.paint();
    assert!(face.slip_view().is_none(), "Esc: no pair");
    assert_eq!(face.state.timeline, tl);
    println!("striaslipview: release = one slip, then no pair; Esc = no pair");
}

/// Rule: only a valid video Slip shows the pair: a plain Move, a plain TrimIn, a refused slip (-5 at
/// B[0,10): `Drag::shown` None) and a slip on the audio track paint no pair, and the refused slip's paint falls back to
/// today's playhead frame.
#[test]
fn striaslipview_other_drags_show_no_pair() {
    let (mut face, _twin, _tl) = nle3eslipface_pair();
    let l = face.layout();
    let (x, y) = l.point(0, 15);
    let esc = bandy::SMessage::BrowserKey("Escape".into());
    for (px, py, dx, mods, what) in [
        (x, y, 40.0, 0, "Move"),
        (l.x_of(10) as f64 + 1.0, y, 16.0, 0, "TrimIn"),
        (l.point(1, 15).0, l.point(1, 15).1, 40.0, bandy::MOD_CONTROL, "audio Slip"),
    ] {
        face.state.select_at(0, 12).unwrap();
        striaslipview_press_move(&mut face, px, py, dx, mods);
        assert!(face.dragging().is_some(), "{what}: a live drag");
        face.paint();
        assert!(face.slip_view().is_none(), "{what}: no pair");
        face.on(&esc);
    }
    face.state.select_at(0, 12).unwrap();
    face.state.seek(12);
    striaslipview_press_move(&mut face, x, y, -40.0, bandy::MOD_CONTROL);
    face.paint();
    assert!(face.slip_view().is_none(), "a refused slip shows no pair");
    let (_, src) = synth_fixture();
    assert_eq!(face.still(), preview(&face.state.timeline, &src, 12).unwrap().as_ref(), "today's playhead frame");
    println!("striaslipview: Move / TrimIn / audio slip / refused slip -> no pair");
}

/// Rule: the throttle: a second slip delta painted at once keeps the first pair and sets `slip_due`; the serve clock's
/// `slip_tick` turns true once the window (max(SLIP_VIEW_MS, the pair's own time)) passes (polled, bounded), and the
/// repaint then shows the pair of the new delta (a twin's `slip 6`).
#[test]
fn striaslipview_throttle_keeps_the_pair_until_the_window_passes() {
    let (mut face, mut twin, _tl) = nle3eslipface_pair();
    let (x, y) = face.layout().point(0, 15);
    striaslipview_press_move(&mut face, x, y, 40.0, bandy::MOD_CONTROL);
    face.paint();
    let p5 = face.slip_view().cloned().expect("the slip 5 pair");
    assert!(!face.slip_due());
    face.on(&bandy::SMessage::BrowserMouseMove(x + 48.0, y, 0));
    assert_eq!(face.dragging().map(|d| d.delta), Some(6));
    face.paint();
    assert_eq!(face.slip_view(), Some(&p5), "inside the window the pair stays");
    assert!(face.slip_due(), "the stale pair is due");
    let t0 = std::time::Instant::now();
    while !face.slip_tick() && t0.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(face.slip_tick(), "the window passed within 5 s");
    face.paint();
    assert_eq!(twin.verb("slip 6"), Some(Ok(())));
    let (first, last) = striaslipview_frames(&twin.timeline, 10, 19);
    assert_eq!(striaslipview_halves(face.slip_view().expect("the slip 6 pair")), (first.rgba, last.rgba));
    assert!(!face.slip_due() && !face.slip_tick());
    println!("striaslipview: second delta kept the pair, due; after {:?} the clock repaint shows slip 6", t0.elapsed());
}

/// Rule: on a split face the tracks Surface renders no pair (its paint carries no still); the preview Surface does.
#[test]
fn striaslipview_split_face_pair_on_the_preview_surface_only() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).on_surfaces(1, 2);
    face.state.select_at(0, 12).unwrap();
    let (x, y) = face.layout().point(0, 15);
    face.on(&bandy::SMessage::BrowserButton { down: true, x, y, mods: bandy::MOD_CONTROL, surface: 2 });
    face.on(&bandy::SMessage::BrowserMouseMove(x + 40.0, y, 2));
    assert_eq!(face.dragging().map(|d| d.kind), Some(DragKind::Slip));
    face.paint_role(Role::Tracks);
    assert!(face.slip_view().is_none(), "the tracks Surface renders no pair");
    face.paint_role(Role::Preview);
    assert!(face.slip_view().is_some(), "the preview Surface does");
    println!("striaslipview: split face: tracks none, preview the pair");
}

/// Rule: `slip_split` KATs: two frames side by side at the first's size; a missing side leaves its cell FIELD; both
/// missing is None; a frame of another size is letterboxed into its cell.
#[test]
fn striaslipview_split_kats() {
    let px = |w: u32, h: u32, v: u8| Still { width: w, height: h, rgba: vec![v; (w * h * 4) as usize] };
    let field = timeline_core::view::FIELD;
    let s = slip_split(Some(&px(2, 2, 9)), Some(&px(2, 2, 5))).unwrap();
    assert_eq!((s.width, s.height), (4, 2));
    assert_eq!(striaslipview_halves(&s), (vec![9; 16], vec![5; 16]));
    let s = slip_split(None, Some(&px(2, 1, 5))).unwrap();
    assert_eq!(striaslipview_halves(&s), (field.repeat(2), vec![5; 8]));
    assert_eq!(slip_split(None, None), None);
    let s = slip_split(Some(&px(4, 2, 9)), Some(&px(2, 2, 5))).unwrap();
    assert_eq!((s.width, s.height), (8, 2));
    let (_, r) = striaslipview_halves(&s);
    assert_eq!((&r[0..4], &r[4..12], &r[12..16]), (&field[..], &[5u8; 8][..], &field[..]), "letterboxed: a 2x2 in a 4x2 cell");
    println!("striaslipview: slip_split KATs");
}

// ---- STRIATRACKADDREMOVE (LEDGER SR856): the V+ / A+ / Track - words on the face, the message line, the save (named tail block) ----

/// The face's words are timeline_core's: `track+ audio` adds an empty A2 (one undo step, dirty), `save` writes
/// `* UNA TRACKS 1 2` and the reopened document holds the empty track; bare `track-` removes the selected clip's track
/// WITH its clips and the message line names it (`track-: removed V (3 clips)`); one undo brings all of it back; with
/// nothing selected the word is refused by name.
#[test]
fn striatrackaddremove_face_buttons_add_and_remove() {
    let (tl0, src) = synth_fixture();
    let path = common::fresh_dir("striatrackaddremove", "face").join("cut.edl");
    std::fs::write(&path, edl::write(&tl0, "cut").unwrap()).unwrap();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(path.as_path()));
    for w in ["track+ video", "track+ audio", "track-"] {
        assert!(VERBS.contains(&w), "{w}");
    }
    assert!(face.verb("track+ audio"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!((face.state.timeline.tracks.len(), face.state.undo_depth(), face.dirty()), (3, 1, true));
    assert!(face.verb("save"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("* UNA TRACKS 1 2\n"), "{text}");
    assert_eq!(edl::parse(&text, 25).unwrap(), face.state.timeline, "the empty A2 survives the save");
    face.state.select(0, 1).unwrap();
    assert!(face.verb("track-"));
    assert_eq!(face.last_error.take().as_deref(), Some("track-: removed V (3 clips)"));
    assert_eq!((face.state.timeline.tracks.len(), face.state.selection), (2, None));
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl0.insert_track(Kind::Audio).unwrap(), "one undo restores the track and its clips");
    face.state.deselect();
    assert!(face.verb("track-"));
    assert_eq!(face.last_error.take(), Some(format!("track-: {}", EditError::NoTrack)));
    assert!(face.verb("track- A2"));
    assert_eq!((face.last_error.take(), face.note.take().as_deref()), (None, Some("track-: removed A2 (0 clips)")));
    assert_eq!(face.state.timeline, tl0);
    println!("striatrackaddremove: A+ -> 3 tracks, saved with * UNA TRACKS 1 2; track- -> removed V (3 clips); undo; A2 removed");
}

// ---- STRIATRACKGAIN (LEDGER SR864): a track's gain through the render's mix and the face's word (named tail block) ----

/// Two audio tracks over reels S0 (base 0) and S1 (base 1000), 50 ticks each: A1 S0[0,20)@0 at clip gain `c1`, A2
/// S1[0,20)@0 at clip gain `c2`, track gains `t1` / `t2`; rendered 0..20.
fn striatrackgain_pcm(c1: i32, c2: i32, t1: i32, t2: i32) -> Vec<f32> {
    let mut tl = Timeline::new(25);
    let (a, b) = (tl.add_source("S0", 50), tl.add_source("S1", 50));
    for (src, c) in [(a, c1), (b, c2)] {
        let t = tl.add_track(Kind::Audio);
        tl.tracks[t].clips = vec![Clip::new(src, 0, 20, 0).gained(c)];
    }
    let tl = tl.set_track_gain(0, t1).unwrap().set_track_gain(1, t2).unwrap();
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    src.add(b, synth_source(1000, 50, 25)).unwrap();
    let mut sink = Pcm::default();
    render(&tl, 0..20, &src, &mut sink).unwrap();
    sink.pcm
}

/// M3 (the mix follows with no mix edit): a unity clip on a -6.02 dB track renders sample for sample like a -6.02 dB
/// clip on a unity track (the Layer carries the sum); the gains multiply (clip -3 dB on a track -3.02 dB = -6.02 dB);
/// A2's track gain leaves A1's term alone; a track at 0 dB renders the bytes it rendered before.
#[test]
fn striatrackgain_render_mix_follows_the_track() {
    let plain = striatrackgain_pcm(0, 0, 0, 0);
    assert_eq!(plain.len(), 20 * PER);
    assert_eq!(striatrackgain_pcm(0, 0, -602, 0), striatrackgain_pcm(-602, 0, 0, 0), "track gain == clip gain");
    assert_eq!(striatrackgain_pcm(-300, 0, -302, 0), striatrackgain_pcm(-602, 0, 0, 0), "they add in dB");
    assert_eq!(striatrackgain_pcm(0, 0, 0, -1200), striatrackgain_pcm(0, -1200, 0, 0), "A2's alone");
    assert_ne!(striatrackgain_pcm(0, 0, 0, -1200), plain);
    let half = striatrackgain_pcm(0, -14_400, -602, -14_400);
    for (j, got) in half.iter().enumerate() {
        let v = synth_sample(0, j as u64) as f32;
        assert!((got * 32768.0 - v / 2.0).abs() <= 1.0 + v / 1000.0 + 1.0, "sample {j}: {} vs {}", got * 32768.0, v / 2.0);
    }
    println!("striatrackgain render: -6.02 dB track == -6.02 dB clip over {} samples", plain.len());
}

/// M3: the face's word through `Face::verb` (timeline_core's dispatch) sets the selected clip's track in one undo step and
/// the saved EDL carries it; with nothing selected the refusal is named on the message line and the timeline stays.
#[test]
fn striatrackgain_face_word_and_refusal() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(VERBS.contains(&"track-gain"));
    assert!(face.verb("track-gain -6"));
    assert_eq!(face.last_error.take(), Some(format!("track-gain -6: {}", EditError::NoTrack)));
    assert_eq!(face.state.timeline, tl);
    face.state.select(1, 0).unwrap();
    assert!(face.verb("track-gain -6"));
    assert_eq!(face.last_error.take().as_deref(), Some("track-gain: A1 -6.0 dB")); // STRIATRACKGAINSTEP (SR901): the success line
    assert_eq!((face.state.timeline.tracks[1].gain_cdb, face.state.undo_depth()), (-600, 1));
    let text = edl::write(&face.state.timeline, "T").unwrap();
    assert!(text.contains("* UNA TGAIN A -600\n"), "{text}");
    assert_eq!(edl::parse(&text, 25).unwrap(), face.state.timeline);
    assert!(face.verb("track-gain x"));
    assert_eq!(face.last_error.take(), Some(format!("track-gain x: {}", EditError::Gain)));
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    println!("striatrackgain face: track-gain -6 -> A1 -600 cdB, saved as * UNA TGAIN A -600; no selection -> {}", EditError::NoTrack);
}


// ---- STRIACROP (LEDGER SR867): the render's crop pass (named tail block) ----

/// STRIACROP (SR867): the preview (THE frame road: `render` of one tick) of a cropped clip is timeline_core's ONE
/// `crop_frame` of the uncropped frame, placed by the reel's policy (Letterbox: FIELD bars at the left edge); a clip with no
/// crop renders the same bytes as before the crop existed; `undo` restores those bytes exactly.
#[test]
fn striacrop_render_crops_the_frame() {
    use stria::nle_face::preview;
    let (tl, src) = synth_fixture();
    let plain = preview(&tl, &src, 12).unwrap().expect("a frame at tick 12");
    let none = tl.set_crop(0, 1, timeline_core::Crop::NONE).unwrap();
    assert_eq!(none, tl, "Crop::NONE is no edit");
    let crop = timeline_core::Crop { l: 250, t: 0, r: 250, b: 0 };
    let mut face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("crop 250 0 250 0"));
    assert_eq!(face.last_error.take(), None);
    assert_eq!(face.state.undo_depth(), 1);
    let (_, src) = synth_fixture();
    let cut = preview(&face.state.timeline, &src, 12).unwrap().expect("a cropped frame");
    assert_eq!((cut.width, cut.height), (plain.width, plain.height));
    let (w, h) = (plain.width as usize, plain.height as usize);
    let want = timeline_core::crop_frame(plain.rgba.as_chunks::<4>().0, w, h, crop, SizePolicy::Letterbox, timeline_core::view::FIELD).into_flattened();
    assert_eq!(cut.rgba, want, "the one crop pass over the frame the step held");
    assert_ne!(cut.rgba, plain.rgba);
    assert_eq!(&cut.rgba[..4], &timeline_core::view::FIELD, "the letterbox bar at the left edge");
    let other = preview(&face.state.timeline, &src, 3).unwrap().expect("tick 3: clip 0, no crop");
    let (_, src2) = synth_fixture();
    assert_eq!(other.rgba, preview(&tl, &src2, 3).unwrap().unwrap().rgba, "an uncropped clip: the same bytes");
    assert!(face.verb("undo"));
    let back = preview(&face.state.timeline, &src2, 12).unwrap().unwrap();
    assert_eq!(back.rgba, plain.rgba, "undo restores the exact bytes");
    println!("striacrop: tick 12 {w}x{h} cropped 250/0/250/0 -> letterboxed, undo restores");
}

/// STRIACROP (SR867): a crop keeping no picture is refused by name on the face's message row, no undo step.
#[test]
fn striacrop_face_refusal_on_the_message_row() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("crop 100 0 100 0"));
    assert_eq!(face.last_error.take(), Some(format!("crop 100 0 100 0: {}", EditError::NoClip)));
    face.state.select(0, 0).unwrap();
    assert!(face.verb("crop 900 0 200 0"));
    assert_eq!(face.last_error.take(), Some(format!("crop 900 0 200 0: {}", EditError::Crop)));
    face.state.select(1, 0).unwrap();
    assert!(face.verb("crop 100 0 100 0"));
    assert_eq!(face.last_error.take(), Some(format!("crop 100 0 100 0: {}", EditError::Kind)));
    assert_eq!(face.state.undo_depth(), 0);
}

// ---- STRIAOPACITY (LEDGER SR868): a non-opaque clip over the track below, rendered through the one mix (named tail block) ----

/// V1 A[0,30)@0 (utp1 counter 0), V2 B[0,10)@10 (counter 1000) at 50%: an overlay frame is timeline_core's ONE
/// `vmix::mix_step` (Dissolve at 50/100: the top at alpha 128 over the lower) of the two sources' own frames, byte for
/// byte; outside the overlay V1 shows alone; opaque, the overlay is V2's own frame.
#[test]
fn striaopacity_half_clip_renders_over_the_track_below() {
    use timeline_core::schedule::Frac;
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 30, 0)];
    tl.tracks[1].clips = vec![Clip::new(b, 0, 10, 10)];
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    src.add(b, synth_source(1000, 50, 25)).unwrap();
    let tl = tl.set_opacity(&[(1, 0)], 50).unwrap();
    let mut sink = Collect::default();
    let r = render(&tl, 0..30, &src, &mut sink).unwrap();
    assert_eq!(sink.frames.len(), 30);
    for t in [10usize, 15, 19] {
        let mut model = TestPattern::render(64, 48, t as u32);
        timeline_core::vmix::mix_step(timeline_core::TransitionKind::Dissolve, Frac { num: 50, den: 100 }, &mut model, &TestPattern::render(64, 48, 1000 + t as u32 - 10), 64, 48);
        assert_eq!(sink.frames[t].1, model, "tick {t}");
    }
    let at = |t: usize| TestPattern::read_counter(&sink.frames[t].1, 64, 48);
    assert_eq!((at(9), at(20), at(29)), (Some(9), Some(20), Some(29)));
    let opaque = tl.set_opacity(&[(1, 0)], timeline_core::OPAQUE).unwrap();
    let mut sink2 = Collect::default();
    render(&opaque, 10..11, &src, &mut sink2).unwrap();
    assert_eq!(sink2.frames[0].1, TestPattern::render(64, 48, 1000), "opaque: V2's own frame");
    println!("striaopacity: frames={} seeks={} runup={}", r.frames, r.seeks, r.runup_frames);
}

/// The face's typed `opacity 50` reaches timeline_core's ONE verb dispatch: one undo step; a bad word is named.
#[test]
fn striaopacity_face_word_is_one_undo_step() {
    use stria::nle_face::{EditorState, Face, PixelOrder, synth_fixture};
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("opacity 50"));
    assert_eq!(face.last_error.take(), None);
    assert_eq!(face.state.timeline, tl0.set_opacity(&[(0, 1)], 50).unwrap());
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("opacity 150"));
    assert_eq!(face.last_error.take().as_deref(), Some("opacity 150: opacity is a percent 0..=100, and video only"));
    assert_eq!(face.state.undo_depth(), 1);
}

// ---- STRIACLIPLINK (LEDGER SR863): the face's `link` / `unlink` words (named tail block) ----
#[test]
fn striacliplink_face_link_word() {
    let (tl, src) = synth_fixture();
    let tl0 = tl.clone();
    let mut face = nlefaceexport_face(tl, src);
    assert!(VERBS.contains(&"link") && VERBS.contains(&"unlink"));
    assert_eq!(VERBS.last(), Some(&"help"), "help stays last");
    assert!(face.verb("link"));
    assert_eq!(face.last_error.take().as_deref(), Some("link: no such clip"), "nothing selected, on the message row");
    face.state.select(0, 2).unwrap();
    face.state.select_add(1, 2).unwrap();
    assert!(face.verb("link"));
    assert_eq!(face.last_error.take(), None);
    assert_eq!(face.state.timeline, tl0.set_group(&[(0, 2), (1, 2)], 1).unwrap());
    face.state.deselect();
    face.state.select(1, 2).unwrap();
    assert_eq!(face.state.set, vec![(0, 2), (1, 2)], "one member selects the pair");
    assert!(face.verb("nudge+"));
    assert_eq!((face.state.timeline.tracks[0].clips[2].at, face.state.timeline.tracks[1].clips[2].at), (26, 26), "picture and sound moved together");
    assert!(face.verb("unlink"));
    assert_eq!(face.state.timeline, tl0.move_many(&[(0, 2), (1, 2)], 1).unwrap());
    assert_eq!(face.state.undo_depth(), 3, "link, nudge, unlink: one step each");
    println!("striacliplink: VERBS[{}] link, unlink; pair nudged to 26", VERBS.len() - 3);
}

// ---- STRIALOUDNESSREADOUT (LEDGER SR866): the export range's integrated loudness on the message line (named tail block) ----

/// The PCM `render` sends, collected whole (the test's own sink: the road the measure takes must be this one).
#[derive(Default)]
struct StriaLoudnessPcm {
    pcm: Vec<f32>,
    fmt: Option<(u32, u16)>,
}

impl stria::nle::RenderSink for StriaLoudnessPcm {
    fn frame(&mut self, _out_t: i64, _width: u32, _height: u32, _rgba: &[u8]) {}
    fn pcm(&mut self, _out_t: i64, rate: u32, channels: u16, interleaved: &[f32]) {
        self.fmt = Some((rate, channels));
        self.pcm.extend_from_slice(interleaved);
    }
}

/// Rule: `nle::loudness` is BS.1770's pure fn over exactly the PCM `render` mixes for the range; the face's `loudness`
/// word puts that number on the message line off its thread, with no undo step and the document clean.
#[test]
fn strialoudnessreadout_face_line_is_the_render_measure() {
    let (tl, src) = synth_fixture();
    let mut sink = StriaLoudnessPcm::default();
    stria::nle::render(&tl, 0..tl.end(), &src, &mut sink).unwrap();
    let (rate, ch) = sink.fmt.expect("the fixture has audio");
    let want = gneiss_pal::dsp::audio::meter::integrated_lufs(&sink.pcm, ch as usize, rate);
    let got = stria::nle::loudness(&tl, 0..tl.end(), &src).unwrap();
    assert_eq!(got, Some(want));
    let mut face = nlefaceexport_face(tl.clone(), src);
    let undo = face.state.undo_depth();
    assert!(VERBS.contains(&"loudness"));
    assert!(face.verb("loudness"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.loudness_pending());
    assert!(face.wait_loudness());
    let line = stria::nle_face::loudness_reading_line(&stria::nle::loudness_reading(&tl, 0..tl.end(), &synth_fixture().1, None), &None); // STRIALOUDNESSNEXT (SR995): the line carries the range and true peak
    assert_eq!(face.last_error.take(), Some(line.clone()));
    assert!(!face.loudness_pending());
    assert_eq!((face.state.undo_depth(), face.dirty(), &face.state.timeline), (undo, false, &tl));
    println!("strialoudnessreadout: whole timeline -> {line}");
}

/// Rule: with both marks set the measure takes `in..out` only and names its ticks; marks past the end are refused by
/// `range_check`'s name (no thread); a second word while one runs cancels it (STRIALOUDNESSCANCEL SR903, its own test).
#[test]
fn strialoudnessreadout_marked_range_and_refusals() {
    let (tl, src) = synth_fixture();
    let (tl2, src2) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.set_marks((Some(5), Some(30))).unwrap();
    let want = stria::nle::loudness_reading(&tl2, 5..30, &src2, None); // STRIALOUDNESSNEXT (SR995): all three readings
    assert!(face.verb("loudness"));
    assert!(face.wait_loudness());
    let line = face.last_error.take().expect("the line");
    assert_eq!(line, stria::nle_face::loudness_reading_line(&want, &Some(5..30))); // STRIALOUDNESSNEXT (SR995)
    assert!(line.ends_with(" ticks 5..30"), "{line}");
    let end = face.state.timeline.end();
    face.state.set_marks((Some(end + 5), Some(end + 9))).unwrap();
    assert!(face.verb("loudness"));
    assert!(!face.loudness_pending(), "a refused range starts no thread");
    assert_eq!(face.last_error.take(), Some(format!("loudness: the marks {}..{} are past the timeline's end {end}", end + 5, end + 9)));
    println!("strialoudnessreadout: {line}");
}

/// Rule: a range that sends no PCM (a video-only timeline) reads `no audio`; silence reads the gate's floor by name.
#[test]
fn strialoudnessreadout_no_audio_and_the_floor() {
    let (mut tl, src) = synth_fixture();
    tl.tracks.retain(|t| t.kind == timeline_core::Kind::Video);
    assert_eq!(stria::nle::loudness(&tl, 0..tl.end(), &src).unwrap(), None);
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb("loudness"));
    assert!(face.wait_loudness());
    assert_eq!(face.last_error.take().as_deref(), Some("loudness: no audio in the range"));
    let floor = stria::nle_face::loudness_line(&Ok(Some(gneiss_pal::dsp::audio::meter::LUFS_FLOOR)), &None);
    assert_eq!(floor, "loudness: -70.0 LUFS integrated (nothing above the -70 LUFS gate)");
    println!("strialoudnessreadout: video-only -> no audio; {floor}");
}

// ---- STRIACOLORLIFT (LEDGER SR869): the clip grade on the render's RGBA, its golden and its word (named tail block) ----

/// Every video frame of `tl` over 0..35 (NLE1's reels).
fn striacolorlift_frames(tl: &Timeline) -> Vec<Vec<u8>> {
    let mut sink = Collect::default();
    render(tl, 0..35, &sources(tl), &mut sink).unwrap();
    sink.frames.into_iter().map(|f| f.1).collect()
}

/// An ungraded timeline renders with no pass: NLE1's EDL text + its frame fnv is nle_export.rs's pinned
/// `NLE6UTP1_NLE_EXPORT_PIN` (0xe149_8435_238b_ff8e), re-computed here byte for byte.
#[test]
fn striacolorlift_identity_render_is_the_pinned_golden() {
    let tl = timeline();
    let mut bytes = edl::write(&tl, "NLE1").unwrap().into_bytes();
    bytes.extend(stria::nle_face::fnv1a(&striacolorlift_frames(&tl).concat()).to_le_bytes());
    let got = stria::nle_face::fnv1a(&bytes);
    assert_eq!(got, 0xe149_8435_238b_ff8e, "{got:#018x}");
}

/// B's two clips (ticks 10..20) graded: their frames are `Grade::apply` of the ungraded frames, every other frame is
/// byte-identical.
#[test]
fn striacolorlift_render_grades_the_clip() {
    let tl = timeline();
    let g = timeline_core::Grade { brightness: 50, contrast: 200, saturation: -1000 };
    let graded = tl.set_grade(0, 1, g).unwrap().set_grade(0, 2, g).unwrap();
    let (plain, got) = (striacolorlift_frames(&tl), striacolorlift_frames(&graded));
    let mut changed = 0;
    for (t, (p, f)) in plain.iter().zip(&got).enumerate() {
        if (10..20).contains(&t) {
            let mut want = p.clone();
            g.apply(&mut want);
            assert_eq!(f, &want, "tick {t}");
            assert!(f.chunks(4).all(|q| q[0] == q[1] && q[1] == q[2]), "tick {t} grey");
            changed += (f != p) as usize;
        } else {
            assert_eq!(f, p, "tick {t} untouched");
        }
    }
    assert_eq!(changed, 10);
    println!("striacolorlift: 10 graded frames == Grade::apply of the plain render, 25 byte-identical");
}

/// A W001 wipe on the cut at 10 with every video clip graded alike: a wipe copies per pixel, so each frame is the ungraded
/// frame through `Grade::apply` only when the transition's OUTGOING frame was graded too (nle.rs `vmix_frame`'s line).
#[test]
fn striacolorlift_wipe_grades_both_sides() {
    use timeline_core::{TransitionKind, WipeDir};
    let tl = timeline().add_transition(0, 10, TransitionKind::Wipe(WipeDir::LeftToRight), 4).unwrap();
    let g = timeline_core::Grade { saturation: -1000, ..timeline_core::Grade::IDENTITY };
    let mut graded = tl.clone();
    for i in 0..graded.tracks[0].clips.len() {
        graded = graded.set_grade(0, i, g).unwrap();
    }
    let (plain, got) = (striacolorlift_frames(&tl), striacolorlift_frames(&graded));
    for (t, (p, f)) in plain.iter().zip(&got).enumerate() {
        let mut want = p.clone();
        g.apply(&mut want);
        assert_eq!(f, &want, "tick {t}");
    }
    assert_ne!(plain[11], got[11], "the wipe's frame is graded");
}

/// The face speaks `color b c s` through THE dispatch: one undo step, the preview grey; a value out of range lands on the
/// message row by EditError::Grade's name and changes nothing; undo gives the exact preview bytes back.
#[test]
fn striacolorlift_face_word_grades_the_preview() {
    let (tl, src) = synth_fixture();
    let (_, src2) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    let before = stria::nle_face::preview(&face.state.timeline, &src2, 12).unwrap().unwrap().rgba;
    let input = |text: &str| bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: text.into() };
    face.state.select(0, 1).unwrap();
    assert!(face.on(&input("color 0 0 -1000")));
    assert_eq!(face.state.undo_depth(), 1);
    let grey = stria::nle_face::preview(&face.state.timeline, &src2, 12).unwrap().unwrap().rgba;
    assert_ne!(grey, before);
    assert!(grey.chunks(4).all(|q| q[0] == q[1] && q[1] == q[2]));
    assert!(face.on(&input("color 5000 0 0")));
    assert_eq!(face.last_error.take(), Some(format!("color 5000 0 0: {}", timeline_core::EditError::Grade)));
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.on(&input("undo")));
    assert_eq!(stria::nle_face::preview(&face.state.timeline, &src2, 12).unwrap().unwrap().rgba, before);
}

// ---- STRIAPAN (LEDGER SR861): per-clip pan — a mono render ignores it, the face's word sets it (named tail block) ----

/// A mono project has one bus and no sides: two tracks panned hard left / hard right render byte-identical to centred.
#[test]
fn striapan_mono_render_ignores_pan() {
    let (a, b) = (SourceId(0), SourceId(1));
    let tracks = |pa: i8, pb: i8| vec![vec![Clip { pan_cq8: pa, ..Clip::new(a, 0, 20, 0) }], vec![Clip { pan_cq8: pb, ..Clip::new(b, 5, 15, 5) }]];
    let (_, centre) = mix(&[0, 1000], tracks(0, 0), 20, SYNTH_RATE);
    for (pa, pb) in [(-128, 127), (127, -127), (64, -1)] {
        let (_, panned) = mix(&[0, 1000], tracks(pa, pb), 20, SYNTH_RATE);
        assert_eq!(panned.pcm, centre.pcm, "pan ({pa}, {pb})");
    }
    println!("striapan: mono render of {} samples unchanged under pan", centre.pcm.len());
}

/// The face's `pan <x>` word reaches timeline_core's verb: the selected audio clip's pan, one undo step, saved as `* UNA PAN`.
#[test]
fn striapan_face_word_sets_the_pan() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(1, 1).unwrap();
    let d0 = face.state.undo_depth();
    assert!(face.verb("pan 0.25"));
    assert_eq!(face.state.timeline.tracks[1].clips[1].pan_cq8, 32);
    assert_eq!(face.state.undo_depth(), d0 + 1);
    assert_eq!(face.last_error, None);
    assert!(edl::write(&face.state.timeline, "T").unwrap().contains("* UNA PAN 32\n"));
    assert!(!face.verb("pan 3"), "a malformed pan word is not a verb");
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline.tracks[1].clips[1].pan_cq8, 0);
    assert!(VERBS.contains(&"pan"), "the button: NLE-PAN-FACE BUILT by SR897");
}

// ---- STRIAPANFACE (LEDGER SR897): the face's pan words and their message-row readout (named tail block) ----

/// `pan+` / `pan-` / `pan` (VERBS, just before `help`) step / centre the selected audio clip through timeline_core's verb and
/// the message row reads the pan: `pan: R 25`, `pan: L 50`, `pan: centre`; nothing selected reads the refusal by name.
#[test]
fn striapanface_face_words_read_the_pan() {
    assert_eq!(&VERBS[VERBS.len() - 4..], &["pan-", "pan", "pan+", "help"]);
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("pan+"));
    assert_eq!(face.last_error.take(), Some(format!("pan: {}", EditError::NoClip)));
    face.state.select(1, 1).unwrap();
    let d0 = face.state.undo_depth();
    for _ in 0..2 {
        assert!(face.verb("pan+"));
    }
    assert_eq!(face.note.take().as_deref(), Some("pan: R 25")); // STRIAPANNEXT (SR994): the reading is the face note
    assert_eq!((face.state.timeline.tracks[1].clips[1].pan_cq8, face.state.undo_depth()), (32, d0 + 2));
    for _ in 0..6 {
        assert!(face.verb("pan-"));
    }
    assert_eq!(face.note.take().as_deref(), Some("pan: L 50")); // STRIAPANNEXT (SR994): the reading is the face note
    assert_eq!(face.state.timeline.tracks[1].clips[1].pan_cq8, -64);
    assert!(face.verb("pan"));
    assert_eq!(face.note.take().as_deref(), Some("pan: centre")); // STRIAPANNEXT (SR994): the reading is the face note
    assert_eq!(face.state.timeline.tracks[1].clips[1].pan_cq8, 0);
    assert!(face.verb("pan 0.25"));
    assert_eq!(face.last_error, None, "`pan <x>` stays silent");
    assert_eq!((stria::nle_face::pan_line(127), stria::nle_face::pan_line(-128), stria::nle_face::pan_line(1)), ("pan: R 100".to_string(), "pan: L 100".to_string(), "pan: R 1".to_string()));
    println!("striapanface: pan+ x2 -> R 25, pan- x6 -> L 50, pan -> centre");
}

// ---- STRIAHELPSEARCH (LEDGER SR874): `help <word>` filters the keys help rows; bare `help` unchanged (named tail block) ----

/// `help slip` shows the help with only the rows whose keys or phrase contain "slip" (ASCII case-insensitive), fewer than the
/// whole table; `help SLIP` the same rows; `help Space` finds the play row by its key.
#[test]
fn striahelpsearch_filters_by_substring() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    assert!(f.verb("help slip"));
    assert!(f.help_shown());
    assert_eq!(f.help_query(), Some("slip"));
    let shown = f.help_rows_shown();
    let want: Vec<_> = help_rows().into_iter().filter(|(k, p)| k.to_ascii_lowercase().contains("slip") || p.contains("slip")).collect();
    assert!(!shown.is_empty() && shown.len() < help_rows().len(), "{} of {}", shown.len(), help_rows().len());
    assert_eq!(shown, want);
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    assert!(f.verb("help SLIP"));
    assert_eq!(f.help_rows_shown(), want);
    assert!(f.verb("help Space"));
    assert!(f.help_rows_shown().iter().any(|(k, p)| *p == help_phrase("play") && k.split(" / ").any(|x| x == "Space")));
    println!("striahelpsearch: help slip -> {} of {} rows", want.len(), help_rows().len());
}

/// A query no row matches is refused by name on the message line; the help is left as it was (hidden here; shown and
/// filtered when it was).
#[test]
fn striahelpsearch_no_match_refused_by_name() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    assert!(f.verb("help zzz"));
    assert_eq!(f.last_error.take().as_deref(), Some("help: no key matches \"zzz\""));
    assert_eq!((f.help_shown(), f.help_query()), (false, None));
    assert!(f.verb("help slip"));
    assert!(f.verb("help zzz"));
    assert_eq!(f.last_error.take().as_deref(), Some("help: no key matches \"zzz\""));
    assert_eq!((f.help_shown(), f.help_query()), (true, Some("slip")));
    println!("striahelpsearch: help zzz refused, the help left as it was");
}

/// View-only: no undo step, the document not dirtied, the timeline and playhead untouched.
#[test]
fn striahelpsearch_view_only() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    let (tl, ph, depth) = (f.state.timeline.clone(), f.state.playhead, f.state.undo_depth());
    assert!(f.verb("help slip"));
    assert!(f.verb("help"));
    assert_eq!((&f.state.timeline, f.state.playhead, f.state.undo_depth(), f.dirty()), (&tl, ph, depth, false));
    println!("striahelpsearch: undo depth {depth} kept, clean");
}

/// Bare `help` after a filtered help hides it and clears the query; a blank query is the bare word.
#[test]
fn striahelpsearch_bare_help_toggles_off_a_filtered_help() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    assert!(f.verb("help slip"));
    assert!(f.verb("help"));
    assert_eq!((f.help_shown(), f.help_query()), (false, None));
    assert!(f.verb("help   "));
    assert_eq!((f.help_shown(), f.help_query()), (true, None));
    assert_eq!(f.help_rows_shown(), help_rows());
    println!("striahelpsearch: bare help clears the query");
}

/// PIN: bare `help` paints exactly as today — after `help slip`, Escape, `help` the labelled frame equals a fresh face's bare
/// help; the filtered frame differs only below the ruler; labels off stays byte-identical; `help` is still the last of 26.
#[test]
fn striahelpsearch_bare_help_unchanged() {
    assert_eq!((VERBS.len(), VERBS.last()), (26, Some(&"help")));
    let mut fresh = nle5dkeyshelp_face(640, 480, true);
    assert!(fresh.verb("help"));
    let bare = fresh.paint();
    let mut f = nle5dkeyshelp_face(640, 480, true);
    assert!(f.verb("help slip"));
    let filtered = f.paint();
    let cut = f.layout().ruler_y as usize * 640 * 4;
    assert_eq!(filtered[..cut], bare[..cut], "the preview band is untouched");
    assert_ne!(filtered[cut..], bare[cut..], "the filtered help paints fewer rows");
    assert!(f.on(&bandy::SMessage::BrowserKey("Escape".into())));
    assert!(!f.help_shown());
    assert!(f.verb("help"));
    assert_eq!(f.paint(), bare, "bare help after a search paints the whole table");
    let mut plain = nle5dkeyshelp_face(640, 480, false);
    let a = plain.paint();
    assert!(plain.verb("help slip"));
    assert_eq!(plain.paint(), a, "labels off: no overlay");
    println!("striahelpsearch: bare fnv {:016x}, filtered {:016x}", stria::nle_face::fnv1a(&bare[cut..]), stria::nle_face::fnv1a(&filtered[cut..]));
}

// ---- STRIASOURCEINFO (LEDGER SR875): the `info` word's line stands INFO_SECS on the face clock (named tail block) ----
/// The fixture face (25 fps, labels off) with reel B's clip on V1 selected.
fn striasourceinfo_face() -> Face {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select(0, 1).unwrap();
    face
}

/// Rule: `info` stands the selected clip's reel line (`info: <reel>: <summary>`) for INFO_SECS x fps face ticks, over a
/// dwelt hover on another clip; it pushes no undo step and dirties nothing; the tick it ends on asks a repaint.
#[test]
fn striasourceinfo_word_stands_five_seconds() {
    let mut face = striasourceinfo_face();
    let (undo, tl) = (face.state.undo_depth(), face.state.timeline.clone());
    assert!(face.verb("info"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let line = face.message_text().expect("the info line");
    assert_eq!(line, format!("info: {}", face.reel_line().expect("B's reel line")));
    assert!(line.starts_with("info: B: utp1 64x48 ~") && line.ends_with(" kb/s"), "{line}");
    assert_eq!((face.state.undo_depth(), face.dirty(), face.state.timeline == tl), (undo, false, true));
    assert!(face.info_pending());
    let (x, y) = face.layout().point(0, 5);
    face.on(&bandy::SMessage::BrowserMouseMove(x, y, 0));
    let n = stria::nle_face::INFO_SECS * 25;
    for k in 1..n {
        assert!(!face.tick(), "tick {k}: no repaint while it stands (labels off)");
    }
    assert!(face.hover_dwelt().is_some(), "the hover on A has dwelt");
    assert_eq!(face.message_text().as_deref(), Some(line.as_str()), "standing over the dwelt hover");
    assert!(face.tick(), "the ending tick repaints");
    assert!(!face.info_pending());
    assert!(face.message_text().is_some_and(|m| m.starts_with("A: ")), "{:?}", face.message_text());
    println!("striasourceinfo: \"{line}\" for {n} ticks, then the hover line");
}

/// Rule: refused by name on the message line: nothing selected `info: no such clip`; a reel with no summary (B not
/// opened) `info: B: no media summary`; neither stands a line.
#[test]
fn striasourceinfo_refusals_by_name() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("info"));
    assert_eq!(face.last_error.take(), Some(format!("info: {}", EditError::NoClip)));
    assert!(!face.info_pending());
    let mut only_a = Sources::new(&tl);
    only_a.add(SourceId(0), synth_source(0, 50, 25)).unwrap();
    let mut face = nlefaceexport_face(tl, only_a);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("info"));
    assert_eq!(face.last_error.take().as_deref(), Some("info: B: no media summary"));
    assert!(!face.info_pending());
    println!("striasourceinfo: info: {} / info: B: no media summary", EditError::NoClip);
}

/// Rule: another selection ends the line at once (the message never names a clip no longer selected); the next tick
/// clears it and repaints.
#[test]
fn striasourceinfo_selection_ends_it() {
    let mut face = striasourceinfo_face();
    assert!(face.verb("info"));
    face.state.select(0, 0).unwrap();
    assert_eq!(face.info_text(), None);
    assert!(face.message_text().is_some_and(|m| m.starts_with("A: ")), "{:?}", face.message_text());
    assert!(face.tick());
    assert!(!face.info_pending());
}

/// Rule: a standing line reads over a running export's line; when it ends the export's line is back.
#[test]
fn striasourceinfo_over_the_export_line() {
    let dir = nlefaceexport_dir("striasourceinfo");
    let mut face = striasourceinfo_face();
    face.export_paused_at(&dir, "png", 3).unwrap();
    assert!(face.message_text().is_some_and(|m| m.starts_with("export: ")));
    assert!(face.verb("info"));
    assert!(face.message_text().is_some_and(|m| m.starts_with("info: B: ")), "{:?}", face.message_text());
    for _ in 0..stria::nle_face::INFO_SECS * 25 {
        face.tick();
    }
    assert!(face.message_text().is_some_and(|m| m.starts_with("export: ")), "{:?}", face.message_text());
    assert!(face.cancel_export());
    assert!(face.wait_export());
    let _ = face.last_error.take();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- STRIATRACKREORDER (LEDGER SR877): track-up / track-down on the transport through the one commit (named tail block) ----

/// NLE1's fixture with a second video track V2 (index 2) holding B[0, 10)@0, the selected clip.
fn striatrackreorder_face() -> (Face, Timeline) {
    let (mut tl, src) = synth_fixture();
    let v2 = tl.add_track(Kind::Video);
    tl.tracks[v2].clips = vec![Clip::new(SourceId(1), 0, 10, 0)];
    tl.validate().unwrap();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select_at(v2, 5).unwrap();
    (face, tl)
}

/// The two words are transport words, before `help` (still the last).
#[test]
fn striatrackreorder_words_are_verbs_before_help() {
    let n = VERBS.len();
    assert_eq!(&VERBS[n - 3..], &["track-up", "track-down", "help"]);
}

/// Rule: `track-up` on V2's clip is `Timeline::swap_track` by hand (V2 swaps with V1, A1 skipped) as ONE undo step; the
/// selection follows its clip; the document is dirty; undo restores it and reads clean; `track-down` swaps it back.
#[test]
fn striatrackreorder_track_up_is_one_commit_and_the_selection_follows() {
    let (mut face, tl) = striatrackreorder_face();
    let clip = face.state.selected().unwrap();
    assert!(face.verb("track-up"));
    assert_eq!(face.last_error.take().as_deref(), Some("track-up: V2 above V1")); // STRIATRACKORDERKEYS (SR922): the move is named on the message row
    assert_eq!(face.state.timeline, tl.swap_track(2, true).unwrap());
    assert_eq!(face.state.selection, Some((0, 0)), "the selection moved with the track");
    assert_eq!(face.state.set, vec![(0, 0)]);
    assert_eq!(face.state.selected(), Some(clip));
    assert_eq!(face.state.undo_depth(), 1, "ONE undo step");
    assert!(face.dirty());
    assert!(face.verb("track-down"));
    assert_eq!(face.state.timeline, tl, "down swaps it back");
    assert_eq!(face.state.selection, Some((2, 0)));
    assert_eq!(face.state.undo_depth(), 2);
    assert!(face.verb("undo"));
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    assert!(!face.dirty(), "undo back to the opened timeline reads clean");
    println!("striatrackreorder: track-up V2 -> V1, selection {:?} follows, one undo step", Some((0, 0)));
}

/// Refusals by name on the message line: the top video track has no video track above; nothing selected; the timeline
/// does not move and no undo step is pushed.
#[test]
fn striatrackreorder_refusals_name_the_reason() {
    let (mut face, tl) = striatrackreorder_face();
    face.state.select_at(0, 5).unwrap();
    assert!(face.verb("track-up"));
    assert_eq!(face.last_error.take(), Some(format!("track-up: {}", EditError::NoTrack)));
    face.state.select_at(1, 5).unwrap();
    assert!(face.verb("track-down"));
    assert_eq!(face.last_error.take(), Some(format!("track-down: {}", EditError::NoTrack)), "A1 is the only audio track");
    face.state.deselect();
    assert!(face.verb("track-down"));
    assert_eq!(face.last_error.take(), Some(format!("track-down: {}", EditError::NoClip)));
    assert_eq!(face.state.timeline, tl);
    assert_eq!(face.state.undo_depth(), 0);
}

// ---- STRIAEXPORTQUEUE (LEDGER SR881): queued export requests run in order from export_tick (named tail block) ----

/// A paused whole-timeline png job into `a` (holding after frame 3 of 35), the face that runs it.
fn striaexportqueue_paused(a: &std::path::Path) -> Face {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.export_paused_at(a, "png", 3).unwrap();
    nlefaceexport_until(&face, (3, 35));
    face
}

#[test]
fn striaexportqueue_runs_in_order_with_their_own_ranges() {
    let (a, b, c) = (nlefaceexport_dir("queue-a"), nlefaceexport_dir("queue-b"), nlefaceexport_dir("queue-c"));
    let mut face = striaexportqueue_paused(&a);
    face.state.seek(5);
    assert!(face.verb("in"));
    face.state.seek(15);
    assert!(face.verb("out"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.verb(&format!("export {} png", b.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (1 waiting)"));
    assert!(face.verb(&format!("export --all {} png", c.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (2 waiting)"));
    assert_eq!(face.export_queued(), 2);
    assert!(!b.exists() && !c.exists(), "a queued request makes nothing until it starts");
    assert!(face.export_skip(), "the running job is cancelled, the queue kept");
    assert_eq!(face.wait_exports(), 3);
    assert!(!face.exporting() && face.export_queued() == 0);
    let line = face.last_error.take().expect("the last job's line");
    assert!(line.starts_with("export: 35 frames, ") && line.ends_with(&format!(" -> {}", c.display())), "{line}");
    assert!(a.join("000002.png").is_file() && !a.join("000003.png").exists(), "A cancelled after frame 3");
    assert!(b.join("000009.png").is_file() && !b.join("000010.png").exists(), "B took its marked range 5..15");
    assert!(c.join("000034.png").is_file(), "C took the whole timeline (--all)");
    assert!(!face.export_skip(), "nothing runs: nothing to skip");
    for d in [&a, &b, &c] {
        let _ = std::fs::remove_dir_all(d);
    }
    println!("striaexportqueue: three jobs in order, last line \"{line}\"");
}

#[test]
fn striaexportqueue_starts_next_from_export_tick() {
    let (a, b) = (nlefaceexport_dir("tick-a"), nlefaceexport_dir("tick-b"));
    let mut face = striaexportqueue_paused(&a);
    assert!(face.verb(&format!("export {} png", b.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (1 waiting)"));
    assert!(face.export_skip());
    let t0 = std::time::Instant::now();
    while face.exporting() {
        assert!(t0.elapsed() < std::time::Duration::from_secs(120), "the queue never drained");
        face.export_tick();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(face.export_queued(), 0);
    let line = face.last_error.take().expect("B's line");
    assert!(line.starts_with("export: 35 frames, ") && line.ends_with(&format!(" -> {}", b.display())), "{line}");
    assert!(b.join("000034.png").is_file(), "B ran to the end, started by export_tick");
    for d in [&a, &b] {
        let _ = std::fs::remove_dir_all(d);
    }
    println!("striaexportqueue: export_tick started the queued job: \"{line}\"");
}

#[test]
fn striaexportqueue_refuses_unknown_format_and_empty_range_at_once() {
    let (a, x, p) = (nlefaceexport_dir("refuse-a"), nlefaceexport_dir("refuse-x"), nlefaceexport_dir("refuse-p"));
    let mut face = striaexportqueue_paused(&a);
    assert!(face.verb(&format!("export {} bogus", x.display())));
    let line = face.last_error.take().expect("the format refusal");
    assert!(line.starts_with("export: unknown format bogus ("), "{line}");
    assert_eq!(face.export_queued(), 0, "a refused request is not queued");
    face.state.seek(12);
    assert!(face.verb("in"));
    face.state.seek(15);
    assert!(face.verb("out"));
    for t in &mut face.state.timeline.tracks {
        t.clips.truncate(1); // A[13,23)@0: the timeline now ends at 10, the marks past it
    }
    face.last_error = None;
    assert!(face.verb(&format!("export {} png", p.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: the marks 12..15 are past the timeline's end 10"));
    assert_eq!(face.export_queued(), 0);
    assert!(!x.exists() && !p.exists(), "a refusal makes nothing");
    assert!(face.cancel_export() && face.wait_export());
    let _ = std::fs::remove_dir_all(&a);
    println!("striaexportqueue: format and range refused at queue time");
}

#[test]
fn striaexportqueue_esc_drops_the_queue() {
    let (a, b, c) = (nlefaceexport_dir("esc-a"), nlefaceexport_dir("esc-b"), nlefaceexport_dir("esc-c"));
    let mut face = striaexportqueue_paused(&a);
    assert!(face.verb(&format!("export {} png", b.display())));
    assert!(face.verb(&format!("export {} png", c.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (2 waiting)"));
    assert!(face.on(&bandy::SMessage::BrowserKey("Escape".into())), "Esc cancels the running export");
    assert_eq!(face.export_queued(), 0, "Esc drops the waiting requests");
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the cancel line");
    assert_eq!(line, format!("export: cancelled at 3/35 frames; partial files left in {}", a.display()));
    assert!(!face.exporting() && !face.export_tick(), "nothing starts after Esc");
    assert!(!b.exists() && !c.exists(), "the dropped requests made nothing");
    let _ = std::fs::remove_dir_all(&a);
    println!("striaexportqueue: Esc dropped 2 queued");
}

#[test]
fn striaexportqueue_full_queue_is_busy() {
    let a = nlefaceexport_dir("full-a");
    let mut face = striaexportqueue_paused(&a);
    for n in 0..stria::nle_face::EXPORT_QUEUE_MAX {
        assert!(face.verb(&format!("export {} png", nlefaceexport_dir(&format!("full-{n}")).display())));
        assert_eq!(face.last_error.take(), Some(format!("export: queued ({} waiting)", n + 1)));
    }
    let over = nlefaceexport_dir("full-over");
    assert!(face.verb(&format!("export {} png", over.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: busy (3/35)"));
    assert_eq!(face.export_queued(), stria::nle_face::EXPORT_QUEUE_MAX);
    assert!(face.cancel_export() && face.wait_export());
    assert!(!over.exists() && !nlefaceexport_dir("full-0").exists(), "nothing queued ran");
    let _ = std::fs::remove_dir_all(&a);
    println!("striaexportqueue: {} queued, one more busy", stria::nle_face::EXPORT_QUEUE_MAX);
}

// ---- STRIAEXPORTAACBITRATE (LEDGER SR887): the face's `aac:KBPS` export word, its bad rate refused by name (named tail block) ----

#[test]
fn striaexportaacbitrate_face_h264_aac96_movie() {
    use gneiss_pal::dsp::demux::{Codec, Demuxer, TrackKind};
    let dir = nlefaceexport_dir("aacbitrate-h264");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb(&format!("export {} h264 aac:96", dir.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the message line");
    assert!(line.starts_with("export: 35 frames, ") && nle5dexportmessageduration_unwalled(&line).ends_with(&format!(" bytes, h264 + aac:96 -> {}", dir.display())), "{line}");
    let d = Demuxer::open(std::fs::read(dir.join("movie.mp4")).unwrap()).unwrap();
    let a = d.tracks().iter().find(|t| t.kind == TrackKind::Audio).expect("an audio track");
    assert_eq!(a.codec, Codec::Aac, "AAC-LC");
    assert_eq!(a.btrt.map(|(_, _, avg)| avg), Some(96_000), "the word's 96 kb/s is the track's avgBitrate");
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaexportaacbitrate: {line}");
}

#[test]
fn striaexportaacbitrate_bad_rate_refused_by_name() {
    let root = nlefaceexport_dir("aacbitrate-refused");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    for (w, k) in [("aac:0", "0"), ("aac:x", "x"), ("aac:", "")] {
        for v in [format!("export {} png {w}", root.display()), format!("export {} {w}", root.display())] {
            assert!(face.verb(&v));
            let e = face.last_error.take().expect("the refusal");
            assert!(e.starts_with(&format!("export: aac rate \"{k}\": aac:KBPS takes a whole number of kb/s above 0; the words are export ")), "{v}: {e}");
            assert!(e.contains("aac[:KBPS]"), "{e}");
            assert!(!face.exporting() && !root.exists(), "{v}: a refused rate makes nothing");
        }
    }
    assert!(face.verb(&format!("export {} png flac", root.display())));
    assert!(face.last_error.take().is_some_and(|e| e.starts_with("export: unknown audio flac; the words are export ")), "another word keeps its refusal");
    println!("striaexportaacbitrate: aac:0 aac:x aac: refused by name in the 2- and 3-word forms");
}

#[test]
fn striaexportaacbitrate_usage_names_the_form() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb("export a b c d"));
    let u = face.last_error.take().expect("the usage refusal");
    assert!(u.starts_with("export: usage: export [--all] [<dir>] [") && u.contains("aac[:KBPS]"), "{u}");
    assert!(!face.exporting());
    println!("striaexportaacbitrate: {u}");
}

// ---- STRIALOUDNESSCANCEL (LEDGER SR903): Esc / a second `loudness` cancels the measure; its progress on the message line (named tail block) ----

/// Rule: a watch whose flag is set before the start stops `render` at its first step (`RenderSink::stop`, before any
/// decode): no PCM reached the meter, so the measure is `None` and nothing was counted.
#[test]
fn strialoudnesscancel_stop_before_start_measures_nothing() {
    let (tl, src) = synth_fixture();
    let range = 0..tl.end();
    let w = stria::nle::LoudnessWatch::new(&range);
    w.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let got = stria::nle::loudness_watched(&tl, range, &src, Some(w.clone())).unwrap();
    assert_eq!(got, None);
    assert_eq!(w.done.load(std::sync::atomic::Ordering::Relaxed), 0);
    println!("strialoudnesscancel: preset stop -> {got:?}, done 0");
}

/// Rule: an unset watch changes nothing about the measure (the same LUFS as `nle::loudness`) and its count ends inside
/// the range: more than nothing, never past the range's ticks.
#[test]
fn strialoudnesscancel_progress_counts_the_range() {
    let (tl, src) = synth_fixture();
    let range = 0..tl.end();
    let total = (range.end - range.start) as u64;
    let w = stria::nle::LoudnessWatch::new(&range);
    let got = stria::nle::loudness_watched(&tl, range.clone(), &src, Some(w.clone())).unwrap();
    assert_eq!(got, stria::nle::loudness(&tl, range, &src).unwrap());
    let done = w.done.load(std::sync::atomic::Ordering::Relaxed);
    assert!(done > 0 && done <= total, "done {done} of {total}");
    println!("strialoudnesscancel: done {done}/{total} -> {got:?}");
}

/// Rule: a second `loudness` word and Esc (the `cancel` key action) each cancel the running measure; its line is
/// `loudness: cancelled` (the job's own flag, as the export's), no undo step, the document clean, nothing left pending.
#[test]
fn strialoudnesscancel_second_word_and_escape_cancel() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    let undo = face.state.undo_depth();
    assert!(face.verb("loudness"));
    assert!(face.verb("loudness"));
    assert!(face.wait_loudness());
    assert_eq!(face.last_error.take().as_deref(), Some("loudness: cancelled"));
    assert!(!face.loudness_pending());
    assert!(face.verb("loudness"));
    assert!(face.on(&bandy::SMessage::BrowserKey("Escape".into())), "Esc cancels the running measure");
    assert!(face.wait_loudness());
    assert_eq!(face.last_error.take().as_deref(), Some("loudness: cancelled"));
    assert!(!face.loudness_pending());
    assert!(!face.cancel_loudness(), "nothing to cancel");
    assert_eq!((face.state.undo_depth(), face.dirty(), &face.state.timeline), (undo, false, &tl));
    println!("strialoudnesscancel: second word and Esc -> loudness: cancelled");
}

/// Rule: the running line is `loudness: measuring <done>/<total>`, done clamped to the total; a face tick while the
/// measure runs puts that line up (or, when the thread already ended, the finished line).
#[test]
fn strialoudnesscancel_progress_line() {
    assert_eq!(stria::nle_face::loudness_progress_line(3, 10), "loudness: measuring 3/10");
    assert_eq!(stria::nle_face::loudness_progress_line(12, 10), "loudness: measuring 10/10");
    let (tl, src) = synth_fixture();
    let (tl2, src2) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb("loudness"));
    let finished = stria::nle_face::loudness_reading_line(&stria::nle::loudness_reading(&tl2, 0..tl2.end(), &src2, None), &None); // STRIALOUDNESSNEXT (SR995)
    if face.loudness_tick() {
        let line = face.last_error.take().expect("a line");
        assert!(line.starts_with("loudness: measuring ") || line == finished, "{line}");
    }
    if face.wait_loudness() {
        assert_eq!(face.last_error.take(), Some(finished.clone()));
    }
    assert!(!face.loudness_pending());
    println!("strialoudnesscancel: progress line, then {finished}");
}

// ---- STRIAROLLKEYS (LEDGER SR893): the roll keys — ( / ) one frame, Ctrl / Cmd + Shift + ( / ) ten (named tail block) ----

/// Rule: `)` / `(` (text keys, like [ / ]) are SR851's `roll+` / `roll-` on the cut at the selected clip's out edge, ONE
/// undo step per key; a one-char key as `BrowserKey` alone is not taken.
#[test]
fn striarollkeys_paren_keys_roll_one_frame() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 0).unwrap();
    assert!(!face.on(&bandy::SMessage::BrowserKey(")".into())), "a one-char BrowserKey alone is not taken");
    assert_eq!(face.state.timeline, tl);
    for _ in 0..3 {
        assert!(face.on(&bandy::SMessage::BrowserText(")".into())));
    }
    assert_eq!(face.state.timeline, tl.roll(0, 0, 3).unwrap());
    assert!(face.on(&bandy::SMessage::BrowserText("(".into())));
    assert_eq!(face.state.timeline, tl.roll(0, 0, 2).unwrap());
    assert_eq!(face.state.undo_depth(), 4);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("striarollkeys: ) x3 then ( = Timeline::roll(0, 0, 2), four undo steps");
}

/// Rule: `Ctrl+Shift+parenright` rolls the cut SLIP_STEP frames later through slipslide_word's `roll+10` -> `roll 10`
/// (A [13,23)@0 | B [0,20)@10 -> A [13,33)@0 | B [10,20)@20), `Shift+Meta+parenleft` ten back; a roll the ops refuse is
/// taken and read on the message row as `roll+10: <err>` with the timeline kept.
#[test]
fn striarollkeys_chords_roll_ten_frames() {
    let (mut tl, src) = synth_fixture();
    tl.tracks[0].clips.truncate(2);
    tl.tracks[0].clips[1] = Clip::new(SourceId(1), 0, 20, 10);
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 0).unwrap();
    assert!(face.on(&bandy::SMessage::BrowserKey("Ctrl+Shift+parenright".into())));
    assert_eq!(face.state.timeline, tl.roll(0, 0, 10).unwrap());
    assert_eq!(face.state.timeline.tracks[0].clips[..], [Clip::new(SourceId(0), 13, 33, 0), Clip::new(SourceId(1), 10, 20, 20)]);
    assert!(face.on(&bandy::SMessage::BrowserKey("Meta+Shift+parenleft".into())));
    assert_eq!(face.state.timeline, tl);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let (plain, src) = synth_fixture();
    let mut face = nlefaceexport_face(plain.clone(), src);
    face.state.select(0, 0).unwrap();
    let err = plain.roll(0, 0, 10).unwrap_err();
    assert!(face.on(&bandy::SMessage::BrowserKey("Ctrl+Shift+parenright".into())), "the key is taken");
    assert_eq!(face.last_error.take(), Some(format!("roll+10: {err}")));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&plain, 0));
    println!("striarollkeys: Ctrl+Shift+parenright = roll 10; on synth_fixture -> roll+10: {err}");
}

/// Rule: the four roll actions are KEYS rows with HELP phrases (not the bare word); the help rows name `(`, `)` and both
/// chords of each ten-frame word; the ten-frame words are keys only (not VERBS); other counts are not face words.
#[test]
fn striarollkeys_help_rows_and_not_verbs() {
    let rows = help_rows();
    for (keys, w) in [(&["("][..], "roll-"), (&[")"][..], "roll+"), (&["Ctrl+Shift+parenleft", "Shift+Meta+parenleft"][..], "roll-10"), (&["Ctrl+Shift+parenright", "Shift+Meta+parenright"][..], "roll+10")] {
        let row = rows.iter().find(|r| r.1 == help_phrase(w)).expect("a help row");
        assert!(HELP.iter().any(|(x, _)| *x == w), "no phrase for {w}");
        assert!(!row.1.is_empty() && row.1 != w, "{w}: a phrase, not the word");
        for k in keys {
            assert!(KEYS.contains(&(*k, w)), "{k} -> {w}");
            assert!(row.0.split(" / ").any(|x| x == *k), "{k} not in {:?}", row.0);
        }
    }
    assert!(!VERBS.contains(&"roll-10") && !VERBS.contains(&"roll+10"), "keys only, no button");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 0).unwrap();
    for v in ["roll+7", "roll10", "roll+10x"] {
        assert!(!face.verb(v), "{v} is not a face word");
    }
    assert_eq!(face.state.timeline, tl);
    println!("striarollkeys: help rows={} keys={}", rows.len(), KEYS.len());
}

// ---- STRIAOPACITYFACE (LEDGER SR898): the opacity buttons step the clip and the message names the percent (named tail block) ----

/// The transport's `opacity-` five times takes the selected clip to 50 % as five undo steps and the message line reads
/// `opacity: 50 %`; `opacity+` reads `opacity: 60 %`; with nothing selected the refusal is named on the message line.
#[test]
fn striaopacityface_face_buttons_step_and_name_the_percent() {
    use stria::nle_face::{EditorState, Face, PixelOrder, VERBS, synth_fixture};
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("opacity-"));
    assert_eq!(face.last_error.take(), Some(format!("opacity: {}", EditError::NoClip)), "nothing selected");
    assert_eq!(face.state.timeline, tl0);
    face.state.select(0, 1).unwrap();
    for _ in 0..5 {
        assert!(VERBS.contains(&"opacity-"));
        assert!(face.verb("opacity-"));
    }
    assert_eq!((face.last_error.take(), face.message_text().as_deref()), (None, Some("opacity: 50 %"))); // STRIAOPACITYNEXT (SR993): the percent is a note
    assert_eq!(face.state.undo_depth(), 5);
    assert_eq!(face.state.timeline, tl0.set_opacity(&[(0, 1)], 50).unwrap());
    assert!(face.verb("opacity+"));
    assert_eq!((face.last_error.take(), face.message_text().as_deref()), (None, Some("opacity: 60 %"))); // STRIAOPACITYNEXT (SR993): the percent is a note
    assert!(face.on(&bandy::SMessage::BrowserSelectAll)); assert_eq!(face.message_text(), face.reel_line(), "the next taken input clears the note: the idle line again"); // STRIAOPACITYNEXT (SR993)
    println!("striaopacityface: opacity- x5 -> 50 %, opacity+ -> 60 %, undo depth {}", face.state.undo_depth());
}

// ---- STRIAMARKERRENAME (LEDGER SR895): `mark <name>` on a taken tick renames, `unmark all` clears (named tail block) ----

/// `mark Intro` on M1's tick renames it (ONE commit, `undo` brings M1 back); the same name again is refused by name with no
/// undo step; bare `mark` there stays refused; the EDL carries the new name.
#[test]
fn striamarkerrename_named_mark_on_a_taken_tick_renames() {
    let mut f = striamarkers_face();
    f.state.seek(12);
    assert!(f.verb("mark"));
    assert_eq!(f.last_error.take().as_deref(), Some("marked M1 at 00:00:00:12"));
    assert!(f.verb("mark Intro"));
    assert_eq!(f.last_error.take().as_deref(), Some("renamed M1 to Intro at 00:00:00:12"));
    assert_eq!(f.state.timeline.markers, vec![timeline_core::Marker { at: 12, name: "Intro".into() }]);
    assert_eq!(f.state.undo_depth(), 2);
    let edl = edl::write(&f.state.timeline, "T").unwrap();
    assert!(edl.contains("* UNA MARKER 00:00:00:12 \"Intro\"\n"), "{edl}");
    assert!(f.verb("mark Intro"));
    assert_eq!(f.last_error.take().as_deref(), Some("mark: Intro is already at 00:00:00:12"));
    assert_eq!(f.state.undo_depth(), 2, "the same name is no edit");
    assert!(f.verb("mark"));
    assert_eq!(f.last_error.take().as_deref(), Some("mark: Intro is already at 00:00:00:12"), "bare mark stays refused");
    assert_eq!(f.state.undo_depth(), 2);
    assert!(f.verb("undo"));
    assert_eq!(f.state.timeline.marker_at(12).map(|m| m.name.as_str()), Some("M1"), "undo brings M1 back");
    println!("striamarkerrename: M1 -> Intro at 12 in one commit; same name and bare mark refused");
}

/// `unmark all` removes every marker in ONE commit (one `undo` brings both back); with none it is refused by name and
/// pushes no undo step.
#[test]
fn striamarkerrename_unmark_all_in_one_commit() {
    let mut f = striamarkers_face();
    f.state.seek(12);
    assert!(f.verb("mark"));
    f.state.seek(30);
    assert!(f.verb("mark A"));
    f.last_error = None;
    assert_eq!(f.state.undo_depth(), 2);
    assert!(f.verb("unmark all"));
    assert_eq!(f.last_error.take().as_deref(), Some("unmarked 2 markers"));
    assert!(f.state.timeline.markers.is_empty());
    assert_eq!(f.state.undo_depth(), 3);
    assert!(f.verb("undo"));
    assert_eq!(f.state.timeline.markers.len(), 2, "one undo brings both back");
    assert!(f.verb("redo"));
    assert!(f.state.timeline.markers.is_empty());
    let depth = f.state.undo_depth();
    assert!(f.verb("unmark all"));
    assert_eq!(f.last_error.take().as_deref(), Some("unmark all: no markers"));
    assert_eq!(f.state.undo_depth(), depth, "no undo step");
    println!("striamarkerrename: unmark all -> 2 markers removed in one commit; none -> refused");
}

// ---- STRIAPASTEINSERT (LEDGER SR896): paste-insert / paste-over through the face's words (named tail block) ----

/// The two words are VERBS before `help`; through `Face::verb` a copied clip lands at the playhead rippling its track
/// (`paste-insert`) or carving what is there (`paste-over`), ONE undo step each.
#[test]
fn striapasteinsert_face_insert_and_over() {
    assert_eq!(VERBS[VERBS.len() - 3..], ["paste-insert", "paste-over", "help"]);
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    let (a, b) = (SourceId(0), SourceId(1));
    face.state.select(0, 1).unwrap();
    assert!(face.verb("copy"));
    face.state.seek(12);
    assert!(face.verb("paste-insert"));
    assert_eq!(face.state.timeline.tracks[0].clips, vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 2, 10), Clip::new(b, 0, 10, 12), Clip::new(b, 2, 10, 22), Clip::new(a, 23, 33, 35)]);
    assert_eq!((face.state.undo_depth(), face.dirty(), face.last_error.clone()), (1, true, None));
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    face.state.seek(12);
    assert!(face.verb("paste-over"));
    assert_eq!(face.state.timeline.tracks[0].clips, vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 2, 10), Clip::new(b, 0, 10, 12), Clip::new(a, 23, 33, 25)]);
    assert_eq!((face.state.undo_depth(), face.last_error.clone()), (1, None));
    println!("striapasteinsert: paste-over at 12 carves V1 to {:?}", face.state.timeline.tracks[0].clips.iter().map(|c| c.at).collect::<Vec<_>>());
}

/// Each refusal lands on the message line by the editor's name; the timeline does not move.
#[test]
fn striapasteinsert_face_refusals_name_the_error() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("paste-insert"));
    assert_eq!(face.last_error.take(), Some(format!("paste-insert: {}", EditError::NoClip)));
    face.state.select(0, 1).unwrap();
    assert!(face.verb("copy"));
    face.state.seek(35);
    face.state.select(1, 0).unwrap();
    assert!(face.verb("paste-over"));
    assert_eq!(face.last_error.take(), Some(format!("paste-over: {}", EditError::Kind)));
    assert_eq!(face.state.timeline, tl);
}

// ---- STRIASNAPTOGGLE (LEDGER SR892): the `snap` word flips the face's drag snap default; Alt inverts it (named tail block) ----

/// Rule: `snap` is a VERBS word just before `help`; it is a view word: it flips the default (`Face::snap_on`), names
/// the new state on the message line (`snap: off` / `snap: on`), pushes no undo step and leaves the document clean.
#[test]
fn striasnaptoggle_word_is_view_only_and_names_itself() {
    assert_eq!(&VERBS[VERBS.len() - 2..], &["snap", "help"]);
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.snap_on());
    assert!(face.verb("snap"));
    assert_eq!(face.note.take().as_deref(), Some("snap: off"));
    assert!(!face.snap_on());
    assert_eq!(face.state.undo_depth(), 0, "no undo step");
    assert!(!face.dirty(), "not dirty");
    assert_eq!(face.state.timeline, tl);
    assert!(face.verb("snap"));
    assert_eq!(face.note.take().as_deref(), Some("snap: on"));
    assert!(face.snap_on());
    assert_eq!(face.state.undo_depth(), 0);
    println!("striasnaptoggle: snap -> off, snap -> on; undo depth 0, clean");
}

/// Rule: over SR836's gesture (in mark 22, V's A@25 body pressed at `x_of(28) + 7` and moved 21 px left: raw -2 ticks),
/// with the snap default off a plain drag keeps the raw -2 and no guide and commits at 23; undone, the same drag with
/// Alt snaps to the mark (delta -3, the guide at 22) and commits flush at 22; with the default back on a plain drag
/// snaps again.
#[test]
fn striasnaptoggle_off_drags_raw_and_alt_snaps() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.seek(22);
    assert!(face.verb("in"));
    face.state.seek(0);
    let tl0 = face.state.timeline.clone();
    let l = face.layout();
    let y = l.point(0, 28).1;
    let (x, to) = (l.x_of(28) as f64 + 7.0, l.x_of(28) as f64 - 14.0);
    assert!(face.verb("snap"));
    assert_eq!(face.note.take().as_deref(), Some("snap: off"));
    face.on(&nle3eslipdrag_btn(true, x, y, 0));
    face.on(&bandy::SMessage::BrowserMouseMove(to, y, 0));
    assert_eq!(face.dragging().map(|d| (d.kind, d.delta, d.guide)), Some((DragKind::Move, -2, None)), "snap off: raw");
    face.on(&nle3eslipdrag_btn(false, to, y, 0));
    assert_eq!(face.state.timeline, tl0.move_clip(0, 2, 0, 23).unwrap());
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl0);
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_ALT));
    face.on(&bandy::SMessage::BrowserMouseMove(to, y, 0));
    assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((-3, Some(22))), "snap off + Alt: snaps");
    face.on(&nle3eslipdrag_btn(false, to, y, 0));
    assert_eq!(face.state.timeline, tl0.move_clip(0, 2, 0, 22).unwrap(), "flush on the mark");
    assert!(face.verb("undo"));
    assert!(face.verb("snap"));
    assert_eq!(face.note.take().as_deref(), Some("snap: on"));
    face.on(&nle3eslipdrag_btn(true, x, y, 0));
    face.on(&bandy::SMessage::BrowserMouseMove(to, y, 0));
    assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((-3, Some(22))), "snap on: SR836's rule");
    face.on(&nle3eslipdrag_btn(false, to, y, 0));
    assert_eq!(face.state.timeline, tl0.move_clip(0, 2, 0, 22).unwrap());
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("striasnaptoggle: off -> lands at 23; off + Alt -> snaps to 22; on -> snaps to 22");
}

// ---- STRIARECENTFILES (LEDGER SR884): the recent documents row's fold (named tail block) ----

/// STRIARECENTFILES M3: an opened document goes first, a repeat moves to the front, the row keeps at most five.
#[test]
fn striarecentfiles_front_dedupe_cap() {
    use std::path::Path;
    use stria::nle_face::{RECENT_MAX, recent_docs, remember_doc};
    assert_eq!(RECENT_MAX, 5);
    let mut row = remember_doc(Path::new("/p/a.edl"), None).expect("a plain path is recorded");
    assert_eq!(row, "/p/a.edl");
    for p in ["/p/b.edl", "/p/c.edl"] {
        row = remember_doc(Path::new(p), Some(&row)).unwrap();
    }
    assert_eq!(recent_docs(Some(&row)), ["/p/c.edl", "/p/b.edl", "/p/a.edl"], "newest first");
    row = remember_doc(Path::new("/p/a.edl"), Some(&row)).unwrap();
    assert_eq!(recent_docs(Some(&row)), ["/p/a.edl", "/p/c.edl", "/p/b.edl"], "a repeat moved to the front, not doubled");
    for p in ["/p/d.edl", "/p/e.edl", "/p/f.edl", "/p/g.edl"] {
        row = remember_doc(Path::new(p), Some(&row)).unwrap();
    }
    assert_eq!(recent_docs(Some(&row)), ["/p/g.edl", "/p/f.edl", "/p/e.edl", "/p/d.edl", "/p/a.edl"], "capped at five");
    assert_eq!(row.lines().count(), 5);
}

/// STRIARECENTFILES M3: an unset or blank row reads empty, blank lines are dropped, an over-long stored row reads five,
/// and a path holding a line break is refused (`None`: the row is one path per line).
#[test]
fn striarecentfiles_reads_and_refusals() {
    use std::path::Path;
    use stria::nle_face::{recent_docs, remember_doc};
    assert!(recent_docs(None).is_empty());
    assert!(recent_docs(Some("")).is_empty());
    assert_eq!(recent_docs(Some("\n/p/a.edl\n\n/p/b.edl\n")), ["/p/a.edl", "/p/b.edl"]);
    assert_eq!(recent_docs(Some("1\n2\n3\n4\n5\n6\n7")).len(), 5);
    assert_eq!(remember_doc(Path::new("/p/bad\nname.edl"), Some("/p/a.edl")), None);
    assert_eq!(remember_doc(Path::new(""), Some("/p/a.edl")), None);
    assert_eq!(remember_doc(Path::new("/p/x.edl"), Some("1\n2\n3\n4\n5\n6\n7")).unwrap(), "/p/x.edl\n1\n2\n3\n4");
}

// ---- STRIAIMPORTWAV (LEDGER SR889): a .wav reel (named tail block) ----

/// STRIAIMPORTWAV (SR889): interleaved sample `i` of the test WAVs (every value names its index).
fn striaimportwav_sample(i: usize) -> i16 {
    (((i * 7) % 60_000) as i32 - 30_000) as i16
}

/// STRIAIMPORTWAV (SR889): a PCM16 WAV of `frames` frames x `ch` at `rate` (the export's own header writer).
fn striaimportwav_pcm16(rate: u32, ch: u16, frames: usize) -> Vec<u8> {
    let mut w = stria::nle_export::wav_header(rate, ch, frames as u64).to_vec();
    for i in 0..frames * ch as usize {
        w.extend_from_slice(&striaimportwav_sample(i).to_le_bytes());
    }
    w
}

/// STRIAIMPORTWAV (SR889): a float32 (tag 3) WAV of `samples`, mono at `rate`.
fn striaimportwav_float32(rate: u32, samples: &[f32]) -> Vec<u8> {
    let data = samples.len() as u32 * 4;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&3u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&32u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

/// STRIAIMPORTWAV (SR889): every PCM run, any channel count.
#[derive(Default)]
struct StriaImportWavSink {
    rate: u32,
    ch: u16,
    pcm: Vec<f32>,
}
impl RenderSink for StriaImportWavSink {
    fn frame(&mut self, _: i64, _: u32, _: u32, _: &[u8]) {}
    fn pcm(&mut self, _out_t: i64, rate: u32, ch: u16, s: &[f32]) {
        (self.rate, self.ch) = (rate, ch);
        self.pcm.extend_from_slice(s);
    }
}

/// STRIAIMPORTWAV (SR889): one audio track holding `clip` of a 50-tick reel `bytes` at 25 fps, rendered over `ticks`.
fn striaimportwav_render(bytes: Vec<u8>, clip: (i64, i64), ticks: i64) -> (Sources, StriaImportWavSink) {
    let mut tl = Timeline::new(25);
    let w = tl.add_source("W", 50);
    let t = tl.add_track(Kind::Audio);
    tl.tracks[t].clips = vec![Clip::new(w, clip.0, clip.1, 0)];
    let mut src = Sources::new(&tl);
    src.add(w, bytes).expect("a PCM16 / float32 WAV is a reel");
    let mut sink = StriaImportWavSink::default();
    render(&tl, 0..ticks, &src, &mut sink).expect("the WAV reel renders");
    (src, sink)
}

#[test]
fn striaimportwav_pcm16_reel_renders_sample_exact() {
    // 2 s of stereo at 48 kHz = 50 ticks at 25 fps (1920 frames a tick); the clip starts 10 ticks in (a seek mid-file).
    let (src, s) = striaimportwav_render(striaimportwav_pcm16(48_000, 2, 96_000), (10, 30), 20);
    assert_eq!(timeline_core::SourceIndex::duration(&src, SourceId(0)), Some(50));
    assert_eq!((s.rate, s.ch), (48_000, 2));
    assert_eq!(s.pcm.len(), 20 * 1920 * 2);
    let skip = 10 * 1920 * 2;
    for (j, got) in s.pcm.iter().enumerate() {
        assert_eq!(*got, q(striaimportwav_sample(skip + j) as i32), "sample {j}: one layer at 0 dB is bit for bit");
    }
    println!("striaimportwav: pcm16 stereo reel {} samples exact from tick 10", s.pcm.len());
}

#[test]
fn striaimportwav_float32_reel_renders() {
    let vals: Vec<f32> = (0..96_000).map(|i| q(striaimportwav_sample(i) as i32)).collect();
    let (_, s) = striaimportwav_render(striaimportwav_float32(48_000, &vals), (0, 50), 50);
    assert_eq!((s.rate, s.ch), (48_000, 1));
    assert_eq!(s.pcm.len(), vals.len());
    for (j, got) in s.pcm.iter().enumerate() {
        assert_eq!(*got, vals[j], "sample {j}: k/32768 floats mix to the same integers");
    }
}

#[test]
fn striaimportwav_refusals_by_name() {
    let refusal = |bytes: Vec<u8>| {
        let tl = Timeline::new(25);
        let mut src = Sources::new(&tl);
        src.add(SourceId(0), bytes).expect_err("refused").to_string()
    };
    let good = striaimportwav_pcm16(48_000, 1, 4800);
    // 12-bit: bits 12.
    let mut eight = good.clone();
    eight[32..34].copy_from_slice(&1u16.to_le_bytes());
    eight[34..36].copy_from_slice(&12u16.to_le_bytes());
    let e = refusal(eight);
    assert!(e.contains("wav: sample layout (8/16/24/32-bit integer or 32/64-bit float)"), "{e}");
    // A compressed tag (2 = MS ADPCM).
    let mut adpcm = good.clone();
    adpcm[20..22].copy_from_slice(&2u16.to_le_bytes());
    let e = refusal(adpcm);
    assert!(e.contains("wav: compressed format tag"), "{e}");
    // A data chunk that says more than the file holds.
    let mut short = stria::nle_export::wav_header(48_000, 1, 4800).to_vec();
    short.extend_from_slice(&good[44..44 + 2 * 2400]);
    let e = refusal(short);
    assert!(e.contains("wav: data chunk runs past the end of the file"), "{e}");
    // No fmt chunk: RIFF/WAVE straight into data.
    let mut nofmt = Vec::new();
    nofmt.extend_from_slice(b"RIFF");
    nofmt.extend_from_slice(&(4 + 8 + 4u32).to_le_bytes());
    nofmt.extend_from_slice(b"WAVEdata");
    nofmt.extend_from_slice(&4u32.to_le_bytes());
    nofmt.extend_from_slice(&[0, 0, 0, 0]);
    let e = refusal(nofmt);
    assert!(e.contains("wav: no fmt chunk"), "{e}");
    // The Player's road is kept: probe and Demuxer::open stay WAV-blind; only open_wav opens it.
    assert_eq!(gneiss_pal::dsp::demux::probe(&good), None);
    assert!(gneiss_pal::dsp::demux::Demuxer::open(good.clone()).is_err());
    let d = gneiss_pal::dsp::demux::Demuxer::open_wav(good).expect("open_wav");
    assert_eq!(d.format(), gneiss_pal::dsp::demux::Format::Wav);
    assert_eq!(d.tracks()[0].codec, gneiss_pal::dsp::demux::Codec::Pcm { bits: 16, float: false, big_endian: false });
}

#[test]
fn striaimportwav_channel_mismatch_is_refused() {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let w = tl.add_source("W", 50);
    let (t1, t2) = (tl.add_track(Kind::Audio), tl.add_track(Kind::Audio));
    tl.tracks[t1].clips = vec![Clip::new(a, 0, 10, 0)];
    tl.tracks[t2].clips = vec![Clip::new(w, 0, 10, 0)];
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    src.add(w, striaimportwav_pcm16(SYNTH_RATE, 2, 2 * SYNTH_RATE as usize)).unwrap();
    let mut sink = StriaImportWavSink::default();
    assert!(matches!(render(&tl, 0..10, &src, &mut sink), Err(stria::nle::RenderError::Format { .. })));
}

#[test]
fn striaimportwav_a_wav_is_its_own_timeline() {
    let wav = striaimportwav_pcm16(48_000, 2, 96_000);
    let (tl, src) = Sources::from_media("W", &wav, 25).unwrap().expect("a WAV is media");
    assert_eq!(tl.tracks.len(), 1);
    assert_eq!(tl.tracks[0].kind, Kind::Audio);
    assert_eq!(tl.tracks[0].clips.len(), 1);
    assert_eq!((tl.tracks[0].clips[0].src_in, tl.tracks[0].clips[0].src_out, tl.tracks[0].clips[0].at), (0, 50, 0));
    let mut sink = StriaImportWavSink::default();
    render(&tl, 0..50, &src, &mut sink).unwrap();
    assert_eq!(sink.pcm.len(), 96_000 * 2);
    assert!(sink.pcm.iter().enumerate().all(|(j, g)| *g == q(striaimportwav_sample(j) as i32)));
}

// ---- STRIAGRADEFACE (LEDGER SR899): the grade step words on the face and the message row's grade (named tail block) ----

/// The six grade words are transport VERBS right before `help` (still the last); with nothing selected each is taken by
/// `Face::verb` and refused on the message line by EditError::NoClip's name, the timeline unchanged.
#[test]
fn striagradeface_verbs_before_help() {
    let words = ["bright-", "bright+", "contrast-", "contrast+", "sat-", "sat+"];
    assert_eq!(VERBS.last(), Some(&"help"));
    assert_eq!(&VERBS[VERBS.len() - 7..VERBS.len() - 1], &words[..]);
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    for v in words {
        assert!(face.verb(v), "{v}");
        assert_eq!(face.last_error.take(), Some(format!("{v}: {}", EditError::NoClip)));
    }
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl, 0));
}

/// The message row's idle line is the reel line; stepping B's grade appends its three values (one undo step per word),
/// a step past the range is refused by EditError::Grade's name, and undo gives the plain line back.
#[test]
fn striagradeface_message_row_reads_the_grade() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 1).unwrap();
    let plain = face.message_text().expect("B's reel line");
    assert_eq!(face.reel_line().as_deref(), Some(plain.as_str()));
    for v in ["bright+", "contrast-", "sat+"] {
        assert!(face.verb(v), "{v}");
        assert!(face.last_error.is_none(), "{v}: {:?}", face.last_error);
    }
    assert_eq!(face.state.undo_depth(), 3);
    assert_eq!(face.message_text(), Some(format!("{plain}  bright 50 contrast -50 sat 50")));
    for _ in 0..19 {
        assert!(face.verb("sat+"));
    }
    assert_eq!(face.state.timeline.tracks[0].clips[1].grade.saturation, 1000);
    let depth = face.state.undo_depth();
    assert!(face.verb("sat+"));
    assert_eq!(face.last_error.take(), Some(format!("sat+: {}", EditError::Grade)));
    assert_eq!(face.state.undo_depth(), depth);
    for _ in 0..depth {
        assert!(face.verb("undo"));
    }
    assert_eq!(face.state.timeline, tl);
    assert_eq!(face.message_text(), Some(plain.clone()));
    println!("striagradeface: message row {plain:?} + \"  bright 50 contrast -50 sat 50\"");
}


// ---- STRIANORMALIZE (LEDGER SR878): the normalize word over the clip's exact peak (named tail block) ----

/// STRIANORMALIZE (SR878): the synth fixture with source A 13..33 on the audio track (track 1), selected.
fn strianormalize_face() -> (stria::nle_face::Face, timeline_core::SourceId) {
    let (mut tl, src) = stria::nle_face::synth_fixture();
    let a = tl.source_by_reel("A").unwrap().id;
    tl.tracks[1].clips = vec![timeline_core::Clip::new(a, 13, 33, 0)];
    let mut face = stria::nle_face::Face::new(stria::nle_face::EditorState::new(tl), src, (320, 160), stria::nle_face::PixelOrder::Rgba, false);
    face.state.select(1, 0).unwrap();
    (face, a)
}

/// STRIANORMALIZE (SR878): the exact peak of synth source A (base 0, PCM16 at `SYNTH_RATE`) over ticks `t0..t1` at 25 fps,
/// from `synth_sample` itself (full scale 32768, the PCM decoder's).
fn strianormalize_want_peak(t0: u64, t1: u64) -> f32 {
    let r = stria::nle::SYNTH_RATE as u64;
    (t0 * r / 25..t1 * r / 25).map(|i| (stria::nle::synth_sample(0, i) as f32 / 32768.0).abs()).fold(0.0f32, f32::max)
}

/// Rule: `normalize` sets the selected audio clip's gain to floor(-100 - 2000 log10 peak) over the clip's exact source
/// range, ONE undo step, its line on the message row; a second `normalize` commits nothing; `undo` restores unity.
#[test]
fn strianormalize_sets_gain_from_the_exact_peak_in_one_undo_step() {
    let (mut face, _) = strianormalize_face();
    let peak = strianormalize_want_peak(13, 33);
    let want = stria::nle_face::normalize_gain_cdb(peak, -100).unwrap();
    assert_eq!(want, (-100.0 - 2000.0 * (peak as f64).log10()).floor() as i32);
    let depth = face.state.undo_depth();
    assert!(stria::nle_face::VERBS.contains(&"normalize"));
    assert!(face.verb("normalize"));
    assert_eq!(face.state.selected().unwrap().gain_cdb, want);
    assert_eq!(face.state.undo_depth(), depth + 1, "one undo step");
    let peak_cdb = (2000.0 * (peak as f64).log10()).round() as i32;
    let line = format!("normalize: peak {} -> gain {}", stria::nle_face::db_words(peak_cdb), stria::nle_face::db_words(want));
    assert_eq!(face.last_error.take(), Some(line.clone()));
    assert!(face.verb("normalize"));
    assert_eq!(face.state.undo_depth(), depth + 1, "the same gain commits nothing");
    assert!(face.verb("undo"));
    assert_eq!(face.state.selected().unwrap().gain_cdb, 0);
    println!("strianormalize: peak {peak} -> gain {want} cdB, line \"{line}\"");
}

/// Rule: an explicit target in dBFS (`cdb_of`'s parse); 0 dBFS is allowed and the result never above it.
#[test]
fn strianormalize_explicit_targets() {
    let (mut face, _) = strianormalize_face();
    let peak = strianormalize_want_peak(13, 33);
    assert!(face.verb("normalize -3"));
    assert_eq!(face.state.selected().unwrap().gain_cdb, stria::nle_face::normalize_gain_cdb(peak, -300).unwrap());
    assert!(face.verb("normalize 0"));
    let g = face.state.selected().unwrap().gain_cdb;
    assert_eq!(g, stria::nle_face::normalize_gain_cdb(peak, 0).unwrap());
    assert!((peak as f64).log10() * 2000.0 + g as f64 <= 0.0, "never above the target");
}

/// Rule: every refusal is named on the message line and leaves the timeline.
#[test]
fn strianormalize_refusals_by_name() {
    let (mut face, _) = strianormalize_face();
    let tl = face.state.timeline.clone();
    for (w, why) in [
        ("normalize x", "x is not a dBFS level".to_string()),
        ("normalize -1 2", "2 is not a dBFS level".to_string()),
        ("normalize 1", "target above 0 dBFS".to_string()),
        ("normalize -200", timeline_core::EditError::Gain.to_string()),
    ] {
        assert!(face.verb(w), "{w}");
        assert_eq!(face.last_error.take(), Some(format!("normalize: {why}")), "{w}");
        assert_eq!(face.state.timeline, tl, "{w}");
    }
    face.state.select(0, 0).unwrap();
    assert!(face.verb("normalize"));
    assert_eq!(face.last_error.take(), Some("normalize: not an audio clip".to_string()));
    face.state.selection = None;
    assert!(face.verb("normalize"));
    assert_eq!(face.last_error.take(), Some(format!("normalize: {}", timeline_core::EditError::NoClip)));
    assert_eq!(face.state.timeline, tl);
}

/// Rule: the gain the word sets rides the EDL's `* UNA GAIN` line and parses back.
#[test]
fn strianormalize_edl_round_trip() {
    let (mut face, _) = strianormalize_face();
    assert!(face.verb("normalize"));
    let g = face.state.selected().unwrap().gain_cdb;
    let text = timeline_core::edl::write(&face.state.timeline, "NORM").unwrap();
    assert!(text.contains(&format!("* UNA GAIN {g}\n")), "{text}");
    assert_eq!(timeline_core::edl::parse(&text, 25).unwrap(), face.state.timeline);
}

/// Rule: `peak_of` is the exact peak on `scan_peaks`' road (within one u8 step of SR827's table over the whole source);
/// `normalize_gain_cdb` floors and refuses silence; `normalize` sits before `help` in VERBS.
#[test]
fn strianormalize_peak_of_reads_the_audio_road() {
    let (tl, src) = stria::nle_face::synth_fixture();
    let a = tl.source_by_reel("A").unwrap().id;
    let got = stria::nle::peak_of(&src, a, 0..tl.tb.to_ns(50)).unwrap().unwrap();
    assert!((got - strianormalize_want_peak(0, 50)).abs() < 1e-6, "{got}");
    let part = stria::nle::peak_of(&src, a, tl.tb.to_ns(13)..tl.tb.to_ns(33)).unwrap().unwrap();
    assert!((part - strianormalize_want_peak(13, 33)).abs() < 1e-6, "{part}");
    let table = stria::nle::scan_peaks(&src, a, timeline_core::view::PEAKS_PER_SEC).unwrap().unwrap();
    let top = table.buckets.iter().copied().max().unwrap() as f32;
    assert!((got * 255.0 - top).abs() <= 0.5 + 1e-3, "{got} vs {top}");
    assert_eq!(stria::nle_face::normalize_gain_cdb(1.0, -100), Some(-100));
    assert_eq!(stria::nle_face::normalize_gain_cdb(0.5, 0), Some(602), "floor(602.06)");
    assert_eq!(stria::nle_face::normalize_gain_cdb(0.0, -100), None);
    assert_eq!(stria::nle_face::normalize_gain_cdb(f32::NAN, -100), None);
    let n = stria::nle_face::VERBS.len();
    assert_eq!(&stria::nle_face::VERBS[n - 2..], &["normalize", "help"]);
    println!("strianormalize: peak_of whole={got} part={part} table max={top}");
}

// ---- STRIAAUTOSAVE (LEDGER SR883): <doc>.autosave.edl beside the document; recover at open (named tail block) ----
// The autosave is `save`'s text written to `<stem>.autosave.edl` (tmp + rename) on the face's clock after
// `AUTOSAVE_EDITS` edits or `AUTOSAVE_SECS` of ticks; `save` and a return to clean remove it; a newer one is offered at
// open and applied only by the word `recover`, as one undo step.

use stria::nle_face::{AUTOSAVE_EDITS, AUTOSAVE_SECS, autosave_path};

/// The synth fixture saved as `<dir>/cut.edl` and opened from it: the face, the document path, the opened timeline.
fn striaautosave_face(name: &str) -> (Face, std::path::PathBuf, Timeline) {
    let (tl0, src) = synth_fixture();
    let doc = common::fresh_dir("striaautosave", name).join("cut.edl");
    std::fs::write(&doc, edl::write(&tl0, "cut").unwrap()).unwrap();
    let face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(doc.as_path()));
    (face, doc, tl0)
}

/// One split of track 0 at tick `t` through the face's `split` word (one commit).
fn striaautosave_split(face: &mut Face, t: i64) {
    face.state.select_at(0, t).unwrap();
    face.state.seek(t);
    assert!(face.verb("split"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
}

/// The `.tmp` sibling the atomic write goes through.
fn striaautosave_tmp(doc: &std::path::Path) -> std::path::PathBuf {
    let mut t = autosave_path(doc).into_os_string();
    t.push(".tmp");
    std::path::PathBuf::from(t)
}

#[test]
fn striaautosave_every_n_edits_writes_the_sibling() {
    let (mut face, doc, _) = striaautosave_face("edits");
    let auto = autosave_path(&doc);
    assert_eq!(auto, doc.with_file_name("cut.autosave.edl"));
    let doc_bytes = std::fs::read(&doc).unwrap();
    assert!(face.last_error.is_none(), "no autosave: no offer");
    assert!(!face.autosave_pending(), "clean at open");
    for t in 1..AUTOSAVE_EDITS as i64 {
        striaautosave_split(&mut face, t);
        assert!(face.autosave_pending(), "dirty and unwritten");
        assert!(!face.autosave_tick());
        assert!(!auto.exists(), "{t} edits: not yet");
    }
    striaautosave_split(&mut face, AUTOSAVE_EDITS as i64);
    assert_eq!(face.state.undo_depth(), AUTOSAVE_EDITS);
    assert!(!face.autosave_tick(), "a written autosave paints nothing");
    let text = std::fs::read_to_string(&auto).unwrap();
    assert_eq!(text, edl::write(&face.state.timeline, "cut").unwrap(), "save's text, titled by the document's stem");
    assert_eq!(edl::parse(&text, 25).unwrap(), face.state.timeline);
    assert_eq!(std::fs::read(&doc).unwrap(), doc_bytes, "the document is never written");
    assert!(!striaautosave_tmp(&doc).exists(), "the tmp was renamed");
    assert_eq!(face.autosaved(), Some(&face.state.timeline));
    assert!(!face.autosave_pending(), "written: the clock may rest");
    assert!(face.dirty(), "an autosave does not clean the document");
    println!("striaautosave: {} edits wrote {} bytes to {}", AUTOSAVE_EDITS, text.len(), auto.display());
}

#[test]
fn striaautosave_sixty_seconds_of_ticks_writes() {
    let (mut face, doc, _) = striaautosave_face("clock");
    let auto = autosave_path(&doc);
    striaautosave_split(&mut face, 15);
    let due = AUTOSAVE_SECS * 25;
    for _ in 1..due {
        assert!(!face.autosave_tick());
    }
    assert!(!auto.exists(), "{} ticks: not yet", due - 1);
    assert!(face.autosave_pending());
    assert!(!face.autosave_tick());
    assert!(auto.exists(), "tick {due} (60 s at 25 fps) writes");
    assert_eq!(edl::parse(&std::fs::read_to_string(&auto).unwrap(), 25).unwrap(), face.state.timeline);
    println!("striaautosave: one edit written after {due} ticks");
}

#[test]
fn striaautosave_save_and_clean_remove_it() {
    let (mut face, doc, tl0) = striaautosave_face("remove");
    let auto = autosave_path(&doc);
    for t in 1..=AUTOSAVE_EDITS as i64 {
        striaautosave_split(&mut face, t);
    }
    face.autosave_tick();
    assert!(auto.exists());
    assert!(face.verb("save"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(!auto.exists(), "a saved document removes its autosave");
    assert_eq!(face.autosaved(), None);
    assert!(!face.autosave_pending());

    let (mut face, doc, _) = striaautosave_face("undo-clean");
    let auto = autosave_path(&doc);
    for t in 1..=AUTOSAVE_EDITS as i64 {
        striaautosave_split(&mut face, t);
    }
    face.autosave_tick();
    assert!(auto.exists());
    for _ in 0..AUTOSAVE_EDITS {
        assert!(face.verb("undo"));
    }
    assert_eq!(face.state.timeline, tl0);
    assert!(!face.dirty());
    assert!(face.autosave_pending(), "clean with an autosave written: owed its removal");
    assert!(!face.autosave_tick());
    assert!(!auto.exists(), "back at the document: the autosave is removed");
    assert!(!face.autosave_pending());
    println!("striaautosave: save and undo-to-clean remove {}", auto.display());
}

#[test]
fn striaautosave_no_document_never_autosaves() {
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false);
    for t in 1..=AUTOSAVE_EDITS as i64 {
        striaautosave_split(&mut face, t);
    }
    assert!(face.dirty());
    assert!(!face.autosave_pending(), "no path: no autosave");
    assert!(!face.autosave_tick());
    assert!(VERBS.contains(&"recover"));
    let before = face.state.timeline.clone();
    assert!(face.verb("recover"), "a verb the face takes");
    assert_eq!(face.last_error.take().as_deref(), Some("recover: no document path"));
    assert_eq!(face.state.timeline, before);
}

#[test]
fn striaautosave_reopen_offers_and_recover_applies_one_step() {
    let (tl0, src) = synth_fixture();
    let doc = common::fresh_dir("striaautosave", "reopen").join("cut.edl");
    let auto = autosave_path(&doc);
    std::fs::write(&doc, edl::write(&tl0, "cut").unwrap()).unwrap();
    let tl1 = tl0.split(0, 15).unwrap();
    std::fs::write(&auto, edl::write(&tl1, "cut").unwrap()).unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    std::fs::File::options().write(true).open(&doc).unwrap().set_modified(past).unwrap();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(doc.as_path()));
    let offer = "autosave: cut.autosave.edl is newer than cut.edl; the word recover loads it";
    assert_eq!(face.last_error.as_deref(), Some(offer), "one message-row line at open");
    assert_eq!(face.message_text().as_deref(), Some(offer));
    face.last_error = None;
    assert_eq!(face.state.timeline, tl0, "nothing is applied without the word");
    assert!(!face.dirty());
    assert_eq!(face.state.undo_depth(), 0);

    assert!(face.verb("recover"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl1);
    assert_eq!(face.state.undo_depth(), 1, "one undo step");
    assert!(face.dirty(), "dirty against the document");
    assert_eq!(face.autosaved(), Some(&tl1));
    assert!(auto.exists(), "the file stays until save");
    assert!(!face.autosave_pending(), "the file already holds the timeline");

    assert!(face.verb("recover"));
    assert_eq!(face.last_error.take().as_deref(), Some("recover: the autosave matches the timeline"));
    assert_eq!(face.state.undo_depth(), 1, "a refused recover pushes nothing");

    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl0, "undo returns to the document as opened");
    assert!(face.verb("redo"));
    assert_eq!(face.state.timeline, tl1);

    assert!(face.verb("save"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(edl::parse(&std::fs::read_to_string(&doc).unwrap(), 25).unwrap(), tl1, "the EDL round trip");
    assert!(!auto.exists(), "saved: the autosave is removed");
    assert!(face.verb("recover"));
    assert_eq!(face.last_error.take(), Some(format!("recover: no autosave ({})", auto.display())));
    println!("striaautosave: offered \"{offer}\", recovered in one step, saved");
}

#[test]
fn striaautosave_older_autosave_is_not_offered() {
    let (tl0, src) = synth_fixture();
    let doc = common::fresh_dir("striaautosave", "older").join("cut.edl");
    let auto = autosave_path(&doc);
    std::fs::write(&auto, edl::write(&tl0.split(0, 15).unwrap(), "cut").unwrap()).unwrap();
    std::fs::write(&doc, edl::write(&tl0, "cut").unwrap()).unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    std::fs::File::options().write(true).open(&auto).unwrap().set_modified(past).unwrap();
    let face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(doc.as_path()));
    assert!(face.last_error.is_none(), "an autosave older than the document is not offered: {:?}", face.last_error);
    assert_eq!(face.state.timeline, tl0);
}

// ---- STRIAUNDOHISTORY (LEDGER SR876): the face's `history` word on the message line (named tail block) ----

/// `history` is the transport word before `help`; it puts the last undo entries, newest first, on the message line
/// (`last_error`) and commits nothing (undo depth, timeline and `dirty` unchanged); a face word names its step with the
/// face's word (`slip+`, not the `slip 1` it calls), and so do the face's own commit roads (a selected transition's `]`).
#[test]
fn striaundohistory_face_word() {
    assert_eq!(VERBS[VERBS.len() - 2], "history");
    assert_eq!(VERBS.last(), Some(&"help"));
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("history"));
    assert_eq!(face.last_error.take().as_deref(), Some("history: nothing to undo"));
    face.state.select(0, 1).unwrap();
    face.state.seek(15);
    assert!(face.verb("split"));
    assert!(face.verb("delete"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let (tl1, depth, dirty) = (face.state.timeline.clone(), face.state.undo_depth(), face.dirty());
    assert!(face.verb("history"));
    assert_eq!(face.last_error.take().as_deref(), Some("history: 2 delete | 1 split"));
    assert_eq!((face.state.timeline == tl1, face.state.undo_depth(), face.dirty()), (true, depth, dirty));
    face.state.select(0, 0).unwrap();
    assert!(face.verb("slip+"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.verb("history"));
    assert_eq!(face.last_error.take().as_deref(), Some("history: 3 slip+ | 2 delete | 1 split"));

    let (tl, src) = synth_fixture();
    let tl = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select_transition(0, 0).unwrap();
    assert!(face.verb("]"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.verb("history"));
    let line = face.last_error.take();
    assert_eq!(line.as_deref(), Some("history: 1 ]"));
    println!("striaundohistory: {line:?}");
}

// ---- STRIAKEYBOARDMAP (LEDGER SR886): the key overlay — remaps, refusals, the help rows, view-only (named tail block) ----
use stria::nle_face::{KEYMAP_UNBIND, Keymap, KeymapError, help_rows_with, parse_keymap};

fn striakeyboardmap_face(keys: &str) -> Face {
    let (tl, src) = synth_fixture();
    Face::new(EditorState::new(tl), src, (640, 480), PixelOrder::Rgba, false).with_keys(keys)
}

fn striakeyboardmap_text(f: &mut Face, t: &str) -> bool {
    f.on(&bandy::SMessage::BrowserText(t.into()))
}

/// `p play` binds p and leaves Space (not named) bound; `Space none` frees Space; `s split` binds a VERBS word with no static
/// key and edits exactly as the word does.
#[test]
fn striakeyboardmap_remap_binds_and_frees() {
    let mut f = striakeyboardmap_face("p play");
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    assert!(striakeyboardmap_text(&mut f, "p"));
    assert!(f.playing, "p plays");
    assert!(striakeyboardmap_text(&mut f, " "));
    assert!(!f.playing, "Space still plays / pauses");
    let mut f = striakeyboardmap_face(&format!("Space {KEYMAP_UNBIND}"));
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    assert!(!striakeyboardmap_text(&mut f, " "), "Space is unbound");
    assert!(!f.playing);
    let mut f = striakeyboardmap_face("s split");
    let mut twin = striakeyboardmap_face("");
    f.state.playhead = 5;
    twin.state.playhead = 5;
    assert!(twin.verb("split"));
    assert!(striakeyboardmap_text(&mut f, "s"), "s is taken");
    assert_eq!(f.state.timeline, twin.state.timeline, "s edits as the word split does");
    assert_eq!(f.last_error, twin.last_error);
    println!("striakeyboardmap: p play, Space none, s split");
}

/// The overlay wins over KEYS: `j stop` makes j stop the transport instead of shuttling; KEYS itself is unchanged.
#[test]
fn striakeyboardmap_overlay_wins() {
    let mut f = striakeyboardmap_face("j stop");
    assert!(striakeyboardmap_text(&mut f, " "));
    assert!(f.playing);
    assert!(striakeyboardmap_text(&mut f, "j"));
    assert!(!f.playing, "j stops");
    assert_eq!(f.rate, 1);
    assert!(KEYS.contains(&("j", "shuttle-")), "the static table is never mutated");
    let mut plain = striakeyboardmap_face("");
    assert!(striakeyboardmap_text(&mut plain, "j"));
    assert!(plain.playing && plain.rate < 0, "without the map j shuttles back");
}

/// Each refusal by name and text; a refused map is not applied (the static table stays, the reason on the message row).
#[test]
fn striakeyboardmap_refusals() {
    assert_eq!(parse_keymap("p spilt"), Err(KeymapError::UnknownAction { line: 1, word: "spilt".into() }));
    assert_eq!(parse_keymap("p play\np stop"), Err(KeymapError::DuplicateKey { line: 2, key: "p".into() }));
    assert_eq!(parse_keymap("# mine\np"), Err(KeymapError::NoAction { line: 2, key: "p".into() }));
    assert_eq!(parse_keymap("? play"), Err(KeymapError::HelpUnbound));
    assert_eq!(parse_keymap("? none"), Err(KeymapError::HelpUnbound));
    assert_eq!(parse_keymap("Space play;Space stop"), Err(KeymapError::DuplicateKey { line: 2, key: "Space".into() }));
    assert_eq!(KeymapError::UnknownAction { line: 3, word: "spilt".into() }.to_string(), "keys: line 3: unknown action \"spilt\"");
    assert_eq!(KeymapError::DuplicateKey { line: 5, key: "p".into() }.to_string(), "keys: line 5: \"p\" is bound twice");
    assert_eq!(KeymapError::NoAction { line: 2, key: "p".into() }.to_string(), "keys: line 2: \"p\" names no action");
    assert_eq!(KeymapError::HelpUnbound.to_string(), "keys: no key shows the help");
    assert_eq!(parse_keymap(""), Ok(Keymap::default()));
    assert_eq!(parse_keymap("# only a comment\n\n"), Ok(Keymap::default()));
    let mut f = striakeyboardmap_face("p play\n? play");
    assert_eq!(f.last_error.as_deref(), Some("keys: no key shows the help"));
    assert!(f.keymap().is_empty(), "a refused map is not applied in part");
    f.last_error = None;
    assert!(!striakeyboardmap_text(&mut f, "p"), "p stays unbound");
    assert!(striakeyboardmap_text(&mut f, "?"));
    assert!(f.help_shown(), "? still shows the help");
}

/// The help rows show the effective bindings: a remapped key joins its action's row and leaves its old one.
#[test]
fn striakeyboardmap_help_rows_show_the_remap() {
    assert_eq!(help_rows_with(&Keymap::default()), help_rows());
    let row = |rows: &[(String, &'static str)], act: &'static str| rows.iter().find(|r| r.1 == help_phrase(act)).map(|r| r.0.clone());
    let rows = help_rows_with(&parse_keymap("p play").unwrap());
    assert_eq!(row(&rows, "play").as_deref(), Some("Space / p"));
    let rows = help_rows_with(&parse_keymap("j stop").unwrap());
    assert_eq!(row(&rows, "stop").as_deref(), Some("k / j"));
    assert_eq!(row(&rows, "shuttle-"), None, "j was shuttle-'s only key");
    let map = parse_keymap("? none; h help").unwrap();
    let rows = help_rows_with(&map);
    assert_eq!(row(&rows, "help").as_deref(), Some("h"));
    let mut f = striakeyboardmap_face("? none; h help");
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    assert!(!striakeyboardmap_text(&mut f, "?"));
    assert!(striakeyboardmap_text(&mut f, "h"));
    assert!(f.help_shown());
    println!("striakeyboardmap: help rows play={:?}", row(&help_rows_with(&parse_keymap("p play").unwrap()), "play"));
}

/// A key map is view-only: taking it pushes no undo step and leaves the document clean.
#[test]
fn striakeyboardmap_view_only() {
    let (tl, src) = synth_fixture();
    let before = tl.clone();
    let mut f = Face::new(EditorState::new(tl), src, (640, 480), PixelOrder::Rgba, false).with_keys("p play\nj stop");
    assert!(!f.dirty());
    assert_eq!(f.state.timeline, before);
    assert!(striakeyboardmap_text(&mut f, "p") && striakeyboardmap_text(&mut f, "j"));
    assert!(!f.dirty());
    assert_eq!(f.state.timeline, before);
    assert!(f.verb("undo"));
    assert_eq!(f.state.timeline, before, "no undo step to take back");
}

/// While the help shows, a remapped key only dismisses it (Stria's rule, through the overlay).
#[test]
fn striakeyboardmap_dismiss_through_overlay() {
    let mut f = striakeyboardmap_face("p play");
    assert!(f.verb("help"));
    assert!(f.help_shown());
    assert!(striakeyboardmap_text(&mut f, "p"));
    assert!(!f.help_shown(), "p dismissed the help");
    assert!(!f.playing, "and was not applied");
}

// ---- STRIAFREEZEFRAME (LEDGER SR880): `freeze` holds the playhead's frame through the one frame road (named tail block) ----

/// synth_fixture (V1 A[13,23)@0 | B@10 | A@25, 25 fps): `freeze` at tick 5 lays a 50-tick hold of source tick 18; the
/// preview (`nle_face::preview`, THE frame road) shows tick 5's old frame on every hold tick and after it; one undo.
#[test]
fn striafreezeframe_face_holds_the_playhead_frame() {
    use stria::nle_face::{EditorState, Face, PixelOrder, VERBS, preview, synth_fixture};
    assert_eq!(VERBS.iter().position(|v| *v == "freeze").map(|i| VERBS[i + 1]), Some("help"), "freeze sits right before help");
    let (tl, src) = synth_fixture();
    let (_, frames) = synth_fixture();
    let before = preview(&tl, &frames, 5).unwrap().expect("a frame at 5");
    assert_ne!(Some(&before), preview(&tl, &frames, 6).unwrap().as_ref(), "the fixture moves: a hold is visible");
    let mut face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
    face.state.seek(5);
    assert!(face.verb("freeze"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let fr = face.state.timeline.clone();
    assert_eq!(fr.end(), tl.end() + 50, "a 2 s hold at 25 fps");
    let h = fr.tracks[0].clips[1];
    assert_eq!((h.src_in, h.src_out, h.at, h.len()), (18, 19, 5, 50));
    assert_eq!(fr.tracks[1], tl.tracks[1], "the audio track does not ripple (NLE-FREEZE-SYNC owed)");
    for t in [5, 6, 30, 54, 55] {
        assert_eq!(preview(&fr, &frames, t).unwrap().as_ref(), Some(&before), "tick {t} shows the frozen frame");
    }
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.state.undo());
    assert_eq!(face.state.timeline, tl, "one undo step");
    let text = edl::write(&fr, "FREEZE").unwrap();
    assert_eq!(edl::parse(&text, 25).unwrap(), fr, "the hold reopens through its M2 line");
    println!("striafreezeframe: freeze at 5 -> hold of A:18 over 5..55, preview equal on 5 / 30 / 54 / 55; one undo");
}

// ---- STRIACROPFACE (LEDGER SR900): the crop words and the badge on the face (named tail block) ----

/// STRIACROPFACE (SR900): `crop` is a transport word just before `help`; `crop-l+` / `crop-r+` reach timeline_core's ONE
/// dispatch through `Face::verb` (one undo step each); a refused step is named on the message row; the painted timeline of a
/// cropped clip carries the CROP_BADGE glyph, and after `crop` clears it the canvas is the uncropped one, byte for byte.
#[test]
fn striacropface_face_words() {
    let (tl, src) = synth_fixture();
    assert_eq!(VERBS[VERBS.len() - 2], "crop");
    assert_eq!(VERBS.last(), Some(&"help"));
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(0, 1).unwrap();
    let before = stria::nle_face::paint(&face.state, 320, 160, PixelOrder::Rgba, None, None);
    assert!(face.verb("crop-l+"));
    assert!(face.verb("crop-r+"));
    assert_eq!(face.last_error.take(), None);
    let s = timeline_core::editor::CROP_STEP;
    assert_eq!(face.state.selected().map(|c| c.crop), Some(timeline_core::Crop { l: s, t: 0, r: s, b: 0 }));
    assert_eq!(face.state.undo_depth(), 2);
    let cut = stria::nle_face::paint(&face.state, 320, 160, PixelOrder::Rgba, None, None);
    assert!(cut.as_chunks::<4>().0.contains(&timeline_core::view::CROP_BADGE), "the badge on the cropped clip");
    assert!(face.verb("crop"));
    assert_eq!((face.last_error.take(), face.state.undo_depth()), (None, 3));
    assert_eq!(stria::nle_face::paint(&face.state, 320, 160, PixelOrder::Rgba, None, None), before, "cleared: no badge");
    assert!(face.verb("crop 500 0 490 0"));
    assert!(face.verb("crop-l+"));
    assert_eq!(face.last_error.take(), Some(format!("crop-l+: {}", EditError::Crop)));
    assert_eq!(face.state.undo_depth(), 4);
    println!("striacropface: crop-l+ crop-r+ -> {s}/0/{s}/0 badge painted; crop clears; crop-l+ past the frame refused");
}

// ---- STRIAIMPORTIMAGESEQUENCE (LEDGER SR888): a PNG sequence as a video reel (named tail block) ----
// A `--src REEL=<dir>/frame_%04d.png` reel is one PNG file per timeline tick (file number `first + tick`), opened by
// `Sources::add_sequence`, read and decoded one file per render tick; a hole is refused by name at its tick. The frames
// sink is SR828's `StriaTitleFrames` (above).

/// A `w` x `h` opaque frame of one colour.
fn striaimportimagesequence_px(w: u32, h: u32, c: [u8; 3]) -> Vec<u8> {
    [c[0], c[1], c[2], 255].repeat(w as usize * h as usize)
}

/// Writes `frame_<n:04>.png` for each `(n, colour)` into a fresh dir; returns the dir's `frame_%04d.png` pattern.
fn striaimportimagesequence_dir(name: &str, w: u32, h: u32, frames: &[(u64, [u8; 3])]) -> String {
    let d = common::fresh_dir("striaimportimagesequence", name);
    for (n, c) in frames {
        let png = gneiss_pal::dsp::image::encode_rgba(w, h, &striaimportimagesequence_px(w, h, *c)).unwrap();
        std::fs::write(d.join(format!("frame_{n:04}.png")), png).unwrap();
    }
    format!("{}/frame_%04d.png", d.display())
}

/// 25 fps, one video track cutting source `SEQ` ticks `0..len` at 0.
fn striaimportimagesequence_tl(len: i64) -> (Timeline, SourceId) {
    let mut tl = Timeline::new(25);
    let s = tl.add_source("SEQ", len);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(s, 0, len, 0)];
    (tl, s)
}

#[test]
fn striaimportimagesequence_pattern_parses() {
    use stria::nle::seq_pattern;
    assert_eq!(seq_pattern("shots/frame_%04d.png"), Some(("shots/frame_", 4, ".png")));
    assert_eq!(seq_pattern("f%d.PNG"), Some(("f", 0, ".PNG")));
    assert_eq!(seq_pattern("a_%06d.jpg"), Some(("a_", 6, ".jpg")), "a pattern: its format is add_sequence's refusal");
    for p in ["clip.mp4", "100%.mp4", "f_%s.png", "f_%4d.png", "f_%04d_%04d.png", "f_%04x.png"] {
        assert_eq!(seq_pattern(p), None, "{p}");
    }
}

#[test]
fn striaimportimagesequence_renders_each_file_per_tick() {
    let cols = [[200, 10, 10], [10, 200, 10], [10, 10, 200]];
    let pat = striaimportimagesequence_dir("ticks", 16, 8, &[(7, cols[0]), (8, cols[1]), (9, cols[2])]);
    let (tl, s) = striaimportimagesequence_tl(3);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    assert_eq!(timeline_core::SourceIndex::duration(&src, s), Some(3), "one tick per file number 7..=9");
    let mut sink = StriaTitleFrames::default();
    let r = render(&tl, 0..3, &src, &mut sink).unwrap();
    assert_eq!((r.width, r.height), (16, 8), "the first file's header sizes the frame");
    assert_eq!(sink.0.len(), 3);
    for (i, f) in sink.0.iter().enumerate() {
        assert_eq!((f.0, f.1, f.2), (i as i64, 16, 8));
        assert!(f.3 == striaimportimagesequence_px(16, 8, cols[i]), "tick {i}: file {}", 7 + i);
    }
    println!("striaimportimagesequence: frame_0007..0009.png -> ticks 0..3 at 16x8");
}

#[test]
fn striaimportimagesequence_hole_refused_by_name() {
    let pat = striaimportimagesequence_dir("hole", 4, 4, &[(7, [1, 2, 3]), (9, [4, 5, 6])]);
    let (tl, s) = striaimportimagesequence_tl(3);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    let mut sink = StriaTitleFrames::default();
    let e = render(&tl, 0..3, &src, &mut sink).unwrap_err().to_string();
    assert!(e.contains("image sequence reel=0 frame 8 missing") && e.contains("frame_0008.png"), "{e}");
    assert_eq!(sink.0.len(), 1, "file 7 drawn before the hole");
    assert!(render(&tl, 2..3, &src, &mut StriaTitleFrames::default()).is_ok(), "a range past the hole renders");
    println!("striaimportimagesequence: hole -> {e}");
}

#[test]
fn striaimportimagesequence_refusals_by_name() {
    let pat = striaimportimagesequence_dir("refuse", 4, 4, &[(1, [9, 9, 9])]);
    let (tl, s) = striaimportimagesequence_tl(1);
    let mut src = Sources::new(&tl);
    let none = pat.replace("frame_%04d", "shot_%04d");
    let jpg = pat.replace("%04d.png", "%04d.tif"); // STRIASEQMORE (SR978): .jpg is a sequence format now; .tif is not
    for (p, why) in [(none.as_str(), "no file matches"), (jpg.as_str(), "only .png"), ("clip.mp4", "no %d or %0Nd")] {
        let e = src.add_sequence(s, p).unwrap_err().to_string();
        assert!(e.contains(&format!("image sequence {p}: ")) && e.contains(why), "{e}");
    }
    let dir = std::path::Path::new(&pat).parent().unwrap().to_path_buf();
    std::fs::write(dir.join("junk_0001.png"), b"not a png").unwrap();
    let junk = format!("{}/junk_%04d.png", dir.display());
    let e = src.add_sequence(s, &junk).unwrap_err().to_string();
    assert!(e.contains("junk_0001.png: not a PNG header"), "{e}");
    assert!(src.add_sequence(s, &pat).is_ok(), "the good pattern still opens");
}

#[test]
fn striaimportimagesequence_round_trip_from_png_export() {
    use stria::nle_export::{AudioOut, Container, export_png};
    let (tl, src) = stria::nle_face::synth_fixture();
    let dir = common::fresh_dir("striaimportimagesequence", "roundtrip").join("out");
    let (r, _) = export_png(&tl, 0..12, &src, &dir, AudioOut::None, Container::Mp4).unwrap();
    assert_eq!(r.frames, 12);
    let mut want = StriaTitleFrames::default();
    render(&tl, 0..12, &src, &mut want).unwrap();
    let (seq_tl, s) = striaimportimagesequence_tl(12);
    let mut seq = Sources::new(&seq_tl);
    seq.add_sequence(s, &format!("{}/%06d.png", dir.display())).unwrap();
    let mut got = StriaTitleFrames::default();
    render(&seq_tl, 0..12, &seq, &mut got).unwrap();
    assert_eq!(got.0.len(), 12);
    for (g, w) in got.0.iter().zip(&want.0) {
        assert!(g == w, "tick {}: the exported file reads back as it rendered", g.0);
    }
    println!("striaimportimagesequence: export 0..12 -> %06d.png -> 12 frames byte-equal");
}

#[test]
fn striaimportimagesequence_cuts_with_a_movie_reel() {
    let (tl0, src0) = stria::nle_face::synth_fixture();
    let mut a_alone = StriaTitleFrames::default();
    render(&tl0, 0..2, &src0, &mut a_alone).unwrap(); // the fixture's A[13,23)@0
    let (w, h) = (a_alone.0[0].1, a_alone.0[0].2);
    let (c0, c1) = ([250, 0, 250], [0, 250, 250]);
    let pat = striaimportimagesequence_dir("cut", w, h, &[(0, c0), (1, c1)]);
    let mut tl = Timeline::new(25);
    let sq = tl.add_source("SEQ", 2);
    let a = tl.add_source("A", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(sq, 0, 2, 0), Clip::new(a, 13, 15, 2), Clip::new(sq, 1, 2, 4)];
    let mut src = Sources::new(&tl);
    src.add_sequence(sq, &pat).unwrap();
    src.add(a, synth_source(0, 50, 25)).unwrap();
    let mut got = StriaTitleFrames::default();
    render(&tl, 0..5, &src, &mut got).unwrap();
    assert_eq!(got.0.len(), 5);
    assert!(got.0[0].3 == striaimportimagesequence_px(w, h, c0) && got.0[1].3 == striaimportimagesequence_px(w, h, c1), "the sequence's two files");
    assert!(got.0[2].3 == a_alone.0[0].3 && got.0[3].3 == a_alone.0[1].3, "the movie step reopens by its seek after the sequence");
    assert!(got.0[4].3 == striaimportimagesequence_px(w, h, c1), "back on the sequence: file 1");
}

#[test]
fn striaimportimagesequence_readd_forgets_sequence() {
    let pat = striaimportimagesequence_dir("readd", 4, 4, &[(0, [1, 1, 1])]);
    let (tl, s) = striaimportimagesequence_tl(1);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    assert_eq!(timeline_core::SourceIndex::duration(&src, s), Some(1));
    src.add(s, synth_source(0, 50, 25)).unwrap();
    assert_eq!(timeline_core::SourceIndex::duration(&src, s), Some(50), "the movie, not the sequence");
    src.add_sequence(s, &pat).unwrap();
    assert_eq!(timeline_core::SourceIndex::duration(&src, s), Some(1), "the sequence again");
}

// ---- STRIAPROXYMEDIA (LEDGER SR882): a source's preview proxy — the preview reads it, the export the original (named tail block) ----

/// The preview (its sink asks for the proxy) shows an attached proxy's frame; the export's render (its sink never asks)
/// reads the original: its fnv is the un-proxied export's.
#[test]
fn striaproxymedia_preview_reads_the_proxy_export_the_original() {
    use stria::nle_export::{AudioOut, export, video_named};
    use stria::nle_face::preview;
    let (tl, src) = synth_fixture();
    let (a, b) = (tl.sources[0].id, tl.sources[1].id);
    let mut alt = Sources::new(&tl);
    alt.add(a, synth_source(5000, 50, 25)).unwrap();
    alt.add(b, synth_source(1000, 50, 25)).unwrap();
    let dir = common::scratch_dir("striaproxymedia", "export");
    std::fs::create_dir_all(&dir).unwrap();
    let before = export(&tl, 0..tl.end(), &src, &dir, video_named("png", tl.tb).unwrap(), AudioOut::None).unwrap();
    let orig = preview(&tl, &src, 3).unwrap().expect("A's frame at 3");
    assert!(!src.has_proxy(a));
    src.set_proxy(a, synth_source(5000, 50, 25)).unwrap();
    assert!(src.has_proxy(a) && !src.has_proxy(b));
    let shown = preview(&tl, &src, 3).unwrap().expect("the proxy's frame at 3");
    let want = preview(&tl, &alt, 3).unwrap().expect("the 5000 reel's frame at 3");
    assert_eq!((shown.width, shown.height), (64, 48));
    assert!(shown.rgba == want.rgba && shown.rgba != orig.rgba, "the preview reads the proxy");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let after = export(&tl, 0..tl.end(), &src, &dir, video_named("png", tl.tb).unwrap(), AudioOut::None).unwrap();
    assert_eq!((after.fnv, after.frames), (before.fnv, before.frames), "the export reads the original");
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxymedia: preview at 3 = the proxy's frame; export fnv {:016x} unchanged", after.fnv);
}

/// `Sources::set_proxy` refuses by name: a proxy of another length (`Stale`), an unknown source and bytes that do not open
/// (`Open`); nothing is attached.
#[test]
fn striaproxymedia_set_proxy_refusals() {
    use stria::nle::{ProxyError, RenderError};
    let (tl, src) = synth_fixture();
    let a = tl.sources[0].id;
    let e = src.set_proxy(a, synth_source(5000, 30, 25)).unwrap_err();
    assert!(matches!(e, ProxyError::Stale { proxy: 30, source: 50 }), "{e}");
    assert_eq!(e.to_string(), "stale (proxy 30 ticks, source 50)");
    assert!(matches!(src.set_proxy(SourceId(9), synth_source(5000, 50, 25)), Err(ProxyError::Open(RenderError::NoSource(_)))));
    assert!(matches!(src.set_proxy(a, vec![0u8; 64]), Err(ProxyError::Open(_))));
    assert!(!src.has_proxy(a));
    println!("striaproxymedia refusals: {e}");
}

/// The verb over reels written beside each other: `proxy` builds `A.proxy.mp4` (half size, every frame) off the face's
/// thread and attaches it; again reuses it; a stamp that no longer matches the source is stale and rebuilt. View-only.
#[test]
fn striaproxymedia_verb_builds_reuses_and_rebuilds() {
    let dir = common::scratch_dir("striaproxymedia", "verb");
    std::fs::create_dir_all(&dir).unwrap();
    let (pa, pb) = (dir.join("A.mp4"), dir.join("B.mp4"));
    std::fs::write(&pa, synth_source(0, 50, 25)).unwrap();
    std::fs::write(&pb, synth_source(1000, 50, 25)).unwrap();
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false).with_reel_paths(vec![("A".into(), pa.clone()), ("B".into(), pb.clone())]);
    face.state.select(0, 0).unwrap();
    let (proxy, stamp, work) = stria::nle_face::proxy_paths(&pa);
    assert_eq!(proxy, dir.join("A.proxy.mp4"));
    assert!(face.verb("proxy"));
    assert_eq!(face.last_error.take(), Some(format!("proxy: A: building (new) -> {}", proxy.display())));
    assert!(face.proxying());
    assert!(face.verb("proxy") && face.last_error.take().is_some_and(|l| l.starts_with("proxy: busy (")), "one job at a time");
    assert!(face.wait_proxy());
    assert!(!face.proxying());
    assert_eq!(face.last_error.take(), Some(format!("proxy: A: 50 frames -> {}", proxy.display())));
    assert!(proxy.is_file() && !work.exists() && stria::nle_face::proxy_fresh(&pa));
    let d = gneiss_pal::dsp::demux::Demuxer::open(std::fs::read(&proxy).unwrap()).unwrap();
    let v = d.tracks().iter().position(|t| t.kind == gneiss_pal::dsp::demux::TrackKind::Video).expect("a video track");
    assert_eq!((d.tracks()[v].width, d.tracks()[v].height, d.track_samples(v).count()), (32, 24, 50));
    assert!(face.verb("proxy"));
    assert_eq!(face.last_error.take(), Some(format!("proxy: A: reused {}", proxy.display())));
    std::fs::write(&stamp, "0 0\n").unwrap();
    assert!(!stria::nle_face::proxy_fresh(&pa));
    assert!(face.verb("proxy"));
    assert_eq!(face.last_error.take(), Some(format!("proxy: A: building (stale) -> {}", proxy.display())));
    assert!(face.wait_proxy());
    assert_eq!(face.last_error.take(), Some(format!("proxy: A: 50 frames -> {}", proxy.display())));
    assert!(stria::nle_face::proxy_fresh(&pa));
    assert_eq!(face.state.timeline, tl, "view-only: the timeline is untouched");
    assert!(!face.dirty(), "view-only: the document stays clean");
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxymedia verb: built 32x24 x 50, reused, rebuilt when stale");
}

/// Refusals by name on the message line, and `proxy` is the VERBS word right before `help`.
#[test]
fn striaproxymedia_verb_refusals() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("proxy"));
    assert_eq!(face.last_error.take().as_deref(), Some("proxy: no clip selected"));
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy"));
    assert_eq!(face.last_error.take().as_deref(), Some("proxy: A: no source path"));
    assert!(!face.proxying() && !face.dirty());
    assert!(VERBS.contains(&"proxy")); // STRIAPROXYBUTTONS (SR930): words now sit between `proxy` and `help`
    assert_eq!(VERBS.last(), Some(&"help"));
    println!("striaproxymedia refusals: no clip selected; A: no source path; VERBS[{}] = proxy", VERBS.len() - 2);
}

// ---- STRIAPOSITIONSCALE (LEDGER SR879): a placed clip over the track below, rendered through the one placement (named tail block) ----

/// V1 A[0,30)@0 (utp1 counter 0), V2 B[0,10)@10 (counter 1000) placed at `pip` (the bottom-right quarter): an overlay
/// frame is timeline_core's ONE `xform_frame` of the two sources' own frames (opaque: V2 placed straight into the rect),
/// byte for byte; outside the rect V1's frame shows unchanged; outside the overlay V1 shows alone.
#[test]
fn striapositionscale_pip_renders_over_the_track_below() {
    use timeline_core::schedule::Frac;
    use timeline_core::{Crop, SizePolicy, Xform};
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 30, 0)];
    tl.tracks[1].clips = vec![Clip::new(b, 0, 10, 10)];
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    src.add(b, synth_source(1000, 50, 25)).unwrap();
    let tl = tl.set_xform(1, 0, Xform::PIP).unwrap();
    let mut sink = Collect::default();
    let r = render(&tl, 0..30, &src, &mut sink).unwrap();
    assert_eq!(sink.frames.len(), 30);
    let rect = Xform::PIP.rect(64, 48);
    for t in [10usize, 15, 19] {
        let (top, under) = (TestPattern::render(64, 48, 1000 + t as u32 - 10), TestPattern::render(64, 48, t as u32));
        let model = timeline_core::xform_frame(top.as_chunks::<4>().0, under.as_chunks::<4>().0, 64, 48, Crop::NONE, SizePolicy::Letterbox, Xform::PIP, Frac { num: 100, den: 100 }).into_flattened();
        assert_eq!(sink.frames[t].1, model, "tick {t}");
        assert_eq!(sink.frames[t].1[..rect.y * 64 * 4], under[..rect.y * 64 * 4], "tick {t}: above the rect is V1's own frame");
        assert_ne!(sink.frames[t].1, under, "tick {t}: the rect holds V2");
    }
    let at = |t: usize| TestPattern::read_counter(&sink.frames[t].1, 64, 48);
    assert_eq!((at(9), at(20), at(29)), (Some(9), Some(20), Some(29)));
    let full = tl.set_xform(1, 0, Xform::NONE).unwrap();
    let mut sink2 = Collect::default();
    render(&full, 10..11, &src, &mut sink2).unwrap();
    assert_eq!(sink2.frames[0].1, TestPattern::render(64, 48, 1000), "unplaced: V2's own frame");
    println!("striapositionscale: frames={} seeks={} runup={} rect={}x{}@{},{}", r.frames, r.seeks, r.runup_frames, rect.w, rect.h, rect.x, rect.y);
}

/// The face's `pip` (a VERBS word, its button) reaches timeline_core's ONE verb dispatch: one undo step; a refusal is
/// named on the message row with no undo step.
#[test]
fn striapositionscale_face_pip_is_one_undo_step() {
    use stria::nle_face::{EditorState, Face, PixelOrder, VERBS, synth_fixture};
    assert!(VERBS.contains(&"pip"), "pip is a VERBS word"); // STRIAXFORMPRESETS (SR938): `scale-` / `scale+` now stand between pip and help
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("pip"));
    assert_eq!(face.last_error.take(), Some(format!("pip: {}", EditError::NoClip)));
    face.state.select(0, 1).unwrap();
    assert!(face.verb("pip"));
    assert_eq!(face.last_error.take(), None);
    assert_eq!(face.state.timeline, tl0.set_xform(0, 1, timeline_core::Xform::PIP).unwrap());
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("scale 600"));
    assert_eq!(face.last_error.take(), Some(format!("scale 600: {}", EditError::Xform)));
    face.state.select(1, 0).unwrap();
    assert!(face.verb("pip"));
    assert_eq!(face.last_error.take(), Some(format!("pip: {}", EditError::Kind)));
    assert_eq!(face.state.undo_depth(), 1);
}

// ---- STRIATCSET (LEDGER SR908): `tc <unit>` sets SR872's readout directly; `t` keys `tc` (named tail block) ----

/// `tc timecode|frames|seconds|off` sets the unit exactly, from off and from a set unit; bare `tc` still cycles from the set
/// unit; a view word: no undo step, not dirty, the timeline and playhead untouched, no message.
#[test]
fn striatcset_tc_sets_each_unit_as_a_view_word() {
    use stria::nle_face::TcUnit;
    let mut face = striatimecodedisplay_face(false);
    face.state.seek(12);
    let (tl, undo) = (face.state.timeline.clone(), face.state.undo_depth());
    let mut seen = Vec::new();
    for v in ["tc frames", "tc seconds", "tc timecode", "tc off", "tc seconds", "tc frames"] {
        assert!(face.verb(v), "{v}");
        seen.push(face.tc_unit());
    }
    assert_eq!(seen, vec![Some(TcUnit::Frames), Some(TcUnit::Seconds), Some(TcUnit::Timecode), None, Some(TcUnit::Seconds), Some(TcUnit::Frames)]);
    assert_eq!(face.tc_readout().as_deref(), Some("12"));
    assert!(face.verb("tc"));
    assert_eq!(face.tc_unit(), Some(TcUnit::Seconds), "bare tc cycles on from the set unit");
    assert_eq!((face.state.undo_depth(), face.dirty(), face.state.playhead), (undo, false, 12));
    assert_eq!(face.state.timeline, tl);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("striatcset: tc <unit> {seen:?} undo={undo} dirty=false");
}

/// An unknown unit or an extra argument keeps the unit and names `tc` in the message; `tcx` is not a face word.
#[test]
fn striatcset_tc_refuses_an_unknown_unit() {
    use stria::nle_face::TcUnit;
    let mut face = striatimecodedisplay_face(false);
    assert!(face.verb("tc seconds"));
    for v in ["tc fps", "tc frames x"] {
        assert!(face.verb(v), "{v}");
        assert_eq!(face.tc_unit(), Some(TcUnit::Seconds), "{v}");
        let e = face.last_error.take().expect("a refusal");
        assert!(e.starts_with("tc: unknown unit"), "{e}");
    }
    assert!(!face.verb("tcx"));
    assert_eq!((face.tc_unit(), face.state.undo_depth(), face.dirty()), (Some(TcUnit::Seconds), 0, false));
    println!("striatcset: tc fps / tc frames x refused, tcx not a word");
}

/// `t` (text) cycles the readout through `tc`; KEYS has the row, the help names `t` with a phrase; while the help shows, `t`
/// only dismisses it (the unit unchanged).
#[test]
fn striatcset_t_key_cycles_and_help_names_it() {
    use stria::nle_face::TcUnit;
    assert!(KEYS.contains(&("t", "tc")));
    let rows = help_rows();
    let row = rows.iter().find(|r| r.0.split(" / ").any(|k| k == "t")).expect("a help row for t");
    assert_eq!(row.1, help_phrase("tc"));
    assert!(row.1 != "tc" && HELP.iter().any(|(a, _)| *a == "tc"));
    let mut face = striatimecodedisplay_face(false);
    assert!(nle5drangekeys_press(&mut face, "t"));
    assert_eq!(face.tc_unit(), Some(TcUnit::Timecode));
    assert!(nle5drangekeys_press(&mut face, "t"));
    assert_eq!(face.tc_unit(), Some(TcUnit::Frames));
    assert!(face.verb("help") && face.help_shown());
    assert!(nle5drangekeys_press(&mut face, "t"));
    assert_eq!((face.help_shown(), face.tc_unit()), (false, Some(TcUnit::Frames)), "dismissed, not applied");
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("striatcset: t -> timecode -> frames; help row {row:?}");
}

/// Labels on: `tc seconds` paints the readout, `tc off` paints byte-identical to the untouched face.
#[test]
fn striatcset_set_off_paint_is_byte_identical() {
    let mut face = striatimecodedisplay_face(true);
    let off = face.paint();
    assert!(face.verb("tc seconds"));
    let on = face.paint();
    assert_ne!(on, off, "the readout shows");
    assert!(face.verb("tc off"));
    assert_eq!(face.paint(), off, "off: byte-identical");
    println!("striatcset: tc seconds inks, tc off fnv {:016x}", stria::nle_face::fnv1a(&off));
}

// ---- STRIAFADESET (LEDGER SR906): fades over the whole selection (named tail block) ----
// A click and a shift-click build NLE2c's set across V1 and A1; `fade-in <n>` sets every clip of it through timeline_core's
// `EditorState::fades_set` as ONE commit (the message line reads the primary's fades), so one undo restores the whole set.

#[test]
fn striafadeset_shift_click_two_clips_fade_in_both_one_undo() {
    let (tl, src) = stria::nle_face::synth_fixture();
    let mut face = stria::nle_face::Face::new(stria::nle_face::EditorState::new(tl), src, (320, 160), stria::nle_face::PixelOrder::Rgba, false);
    let tl0 = face.state.timeline.clone();
    let (x, y) = face.layout().point(0, 15); // V1's B[0,10)@10
    face.on(&nle3eslipdrag_btn(true, x, y, 0));
    face.on(&nle3eslipdrag_btn(false, x, y, 0));
    let (x, y) = face.layout().point(1, 30); // A1's A[23,33)@25
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_SHIFT));
    face.on(&nle3eslipdrag_btn(false, x, y, bandy::MOD_SHIFT));
    assert_eq!((face.state.selection, face.state.set.clone()), (Some((1, 2)), vec![(0, 1), (1, 2)]));
    assert!(face.verb("fade-in 2"));
    let f = |f: &stria::nle_face::Face| f.state.timeline.tracks.iter().map(|t| t.clips.iter().map(|c| (c.fade_in, c.fade_out)).collect::<Vec<_>>()).collect::<Vec<_>>();
    assert_eq!(f(&face), vec![vec![(0, 0), (2, 0), (0, 0)], vec![(0, 0), (0, 0), (2, 0)]], "both selected clips faded, the rest none");
    assert_eq!(face.last_error.take().as_deref(), Some("fade-in 2, fade-out 0"));
    assert_eq!(face.state.undo_depth(), 1, "ONE commit for the set");
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl0, "one undo restores the whole set");
}

// ---- STRIAMARKERJUMP (LEDGER SR894): marker- / marker+ / cut- / cut+ from the face — view words, no undo step (named tail block) ----

/// The four jump words are transport words before `help`; through `Face::verb` (which falls through to THE editor
/// dispatch) `cut+` steps the clip edges and `marker-` returns to a marker, with no message, no undo step and the
/// document's dirtiness unchanged.
#[test]
fn striamarkerjump_face_words_move_only_the_playhead() {
    let (tl, src) = synth_fixture();
    let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    let help = VERBS.iter().position(|v| *v == "help").unwrap();
    for v in ["marker-", "marker+", "cut-", "cut+"] {
        let i = VERBS.iter().position(|x| *x == v).unwrap_or_else(|| panic!("{v} is a VERBS word"));
        assert!(i < help, "{v} before help");
    }
    assert!(f.verb("cut+"));
    assert_eq!((f.state.playhead, f.state.undo_depth()), (10, 0));
    assert!(!f.dirty(), "a jump does not dirty the document");
    assert_eq!(f.last_error.take().as_deref(), Some("cut+: 00:00:00:10")); // STRIAMARKERJUMPKEYS (SR923): the message row names where the jump landed
    f.state.seek(12);
    assert!(f.verb("mark"));
    let _ = f.last_error.take();
    let tl0 = f.state.timeline.clone();
    assert!(f.verb("cut+"));
    assert_eq!(f.state.playhead, 20);
    assert!(f.verb("marker-"));
    assert_eq!(f.state.playhead, 12);
    assert!(f.verb("marker-"));
    assert_eq!(f.state.playhead, 12, "no marker before 12: the playhead stays");
    assert_eq!(f.last_error.take().as_deref(), Some("marker-: M1 at 00:00:00:12")); // STRIAMARKERJUMPKEYS (SR923): the still jump sets nothing; the earlier move's message remains
    assert_eq!(f.state.timeline, tl0);
    assert_eq!(f.state.undo_depth(), 1, "only the mark is an undo step");
    println!("striamarkerjump: cut+ 0 -> 10, 12 -> 20; marker- 20 -> 12");
}

// ---- STRIAINSERTMARKS (LEDGER SR905): insert / overwrite into the in / out marks from the face (named tail block) ----

/// The face's `in` at 5 and `out` at 9 mark the record range; with B selected `insert-marked` lays B 0..4 at 5 on V1
/// through `place` Insert (A1 not rippled), `overwrite-marked` the Overwrite; the message row reads the laid clip's reel.
#[test]
fn striainsertmarks_face_words_fill_the_marked_range() {
    for (word, mode) in [("insert-marked", timeline_core::Mode::Insert), ("overwrite-marked", timeline_core::Mode::Overwrite)] {
        let (tl, src) = synth_fixture();
        let mut face = nlefaceexport_face(tl.clone(), src);
        face.state.select(0, 1).unwrap();
        face.state.seek(5);
        assert!(face.verb("in"));
        face.state.seek(9);
        assert!(face.verb("out"));
        let marked = tl.set_marks((Some(5), Some(9))).unwrap();
        assert_eq!(face.state.timeline, marked, "{word}: the marks are the timeline's");
        assert!(face.verb(word), "{word}");
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        assert_eq!(face.state.timeline, marked.place(0, Clip::new(SourceId(1), 0, 4, 5), mode).unwrap(), "{word}");
        assert_eq!(face.state.timeline.tracks[1], tl.tracks[1], "{word}: only the target track moves");
        assert_eq!((face.state.selected(), face.state.playhead), (Some(Clip::new(SourceId(1), 0, 4, 5)), 9));
        assert_eq!(face.message_text(), Some(format!("{word}: B 00:00:00:04 at 00:00:00:05")), "the laid line"); // STRIAINSERTMARKSKEYS (SR924)
        assert!(face.verb("undo"));
        assert_eq!(face.state.timeline, marked, "{word}: one undo");
    }
    println!("striainsertmarks: in 5, out 9, B selected -> insert-marked / overwrite-marked lay B 0..4 at 5 on V1");
}

/// No marks: the marked words are refused by the editor's name on the message line, the timeline untouched; words after
/// a marked word are the malformed form.
#[test]
fn striainsertmarks_face_refusals_by_name() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 1).unwrap();
    for v in ["insert-marked", "overwrite-marked"] {
        assert!(face.verb(v), "{v}");
        assert_eq!(face.last_error.take(), Some(format!("{v}: {}", EditError::NoMarks)));
    }
    assert!(face.verb("insert-marked now"));
    assert_eq!(face.last_error.take(), Some(format!("insert-marked now: {}", EditError::InsertForm)));
    assert_eq!(face.state.timeline, tl);
    println!("striainsertmarks: no marks -> insert-marked: {}", EditError::NoMarks);
}

// ---- STRIAZOOMFITKEYS (LEDGER SR907): `\` / `|` fit keys, their help rows, the refit on a resize (named tail block) ----

/// The window's resize, as the surface sends it.
fn striazoomfitkeys_resize(f: &mut Face, w: u32) -> bool {
    f.on(&bandy::SMessage::BrowserResize(w, 160, 0))
}

/// `\` (text) fits the timeline, `|` (text) the selection, through the VERBS words; a lone `BrowserKey("\\")` is not
/// taken; the help names both keys with their phrases (Shift+Z stays redo).
#[test]
fn striazoomfitkeys_keys_fit_and_the_help_names_them() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(KEYS.contains(&("\\", "zoom-fit")) && KEYS.contains(&("|", "zoom-sel")) && KEYS.contains(&("Z", "redo")));
    assert!(!face.on(&bandy::SMessage::BrowserKey("\\".into())), "a one-char BrowserKey alone is not taken");
    assert_eq!(face.state.zoom, 8);
    assert!(face.on(&bandy::SMessage::BrowserText("\\".into())));
    assert_eq!((face.state.zoom, face.layout().t0), (9, 0));
    face.state.select(0, 1).unwrap();
    assert!(face.on(&bandy::SMessage::BrowserText("|".into())));
    assert_eq!((face.state.zoom, face.layout().t0), (32, 10));
    assert_eq!(face.last_error, None);
    assert_eq!((face.state.undo_depth(), face.dirty(), face.state.timeline == tl), (0, false, true));
    let rows = help_rows();
    for (k, w) in [("\\", "zoom-fit"), ("|", "zoom-sel")] {
        let row = rows.iter().find(|r| r.0.split(" / ").any(|x| x == k)).expect("a help row");
        assert_eq!(row.1, help_phrase(w));
        assert!(HELP.iter().any(|(a, _)| *a == w) && row.1 != w, "{w}: a phrase, not the word");
    }
    println!("striazoomfitkeys: \\ fit 9 @0, | sel 32 @10; help rows {}", rows.len());
}

/// A resize after a fit fits again at the new width (the timeline, then the selection); after `zoom+` or a wheel zoom a
/// resize leaves the zoom; a refit the emptied set refuses reports once, the next resize is silent; never an undo step.
#[test]
fn striazoomfitkeys_resize_refits_only_after_a_fit() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    let clean = |f: &Face| (f.state.undo_depth(), f.dirty(), f.state.timeline == tl);
    assert!(face.verb("zoom-fit"));
    assert!(striazoomfitkeys_resize(&mut face, 640));
    assert_eq!((face.state.zoom, face.layout().t0), (18, 0), "640 / 35");
    assert!(striazoomfitkeys_resize(&mut face, 480));
    assert_eq!(face.state.zoom, 13, "the refit holds across resizes");
    assert!(face.verb("zoom+"));
    let z = face.state.zoom;
    assert!(striazoomfitkeys_resize(&mut face, 320));
    assert_eq!(face.state.zoom, z, "a zoom word ended the fit");
    assert!(face.verb("zoom-fit"));
    assert_eq!(face.state.zoom, 9);
    assert!(face.zoom_at(1, 0.0));
    let z = face.state.zoom;
    assert!(striazoomfitkeys_resize(&mut face, 640));
    assert_eq!(face.state.zoom, z, "a wheel zoom ended the fit");
    face.state.select(0, 1).unwrap();
    assert!(face.verb("zoom-sel"));
    assert_eq!((face.state.zoom, face.layout().t0), (64, 10));
    assert!(striazoomfitkeys_resize(&mut face, 320));
    assert_eq!((face.state.zoom, face.layout().t0), (32, 10), "the selection refit");
    assert_eq!(face.last_error, None);
    face.state.deselect();
    assert!(striazoomfitkeys_resize(&mut face, 640));
    assert_eq!(face.last_error.take(), Some(format!("zoom-sel: {}", EditError::NoClip)));
    assert_eq!(face.state.zoom, 32, "a refused refit leaves the zoom");
    assert!(striazoomfitkeys_resize(&mut face, 480));
    assert_eq!(face.last_error, None, "reported once");
    assert_eq!(clean(&face), (0, false, true));
    println!("striazoomfitkeys: fit 640->18 480->13; zoom+ / wheel end it; sel 64 -> 32; refusal once");
}

// ---- STRIATRACKGAINSTEP (LEDGER SR901): the track gain step words and their message line on the face (named tail block) ----

/// M3: `track-gain-` / `track-gain+` are transport words before help; each steps the selected clip's track one dB in one
/// undo step and puts `track-gain: A1 <db_words>` on the message line; with nothing selected the refusal names the word.
#[test]
fn striatrackgainstep_face_steps_and_line() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert_eq!(&VERBS[VERBS.len() - 3..], &["track-gain-", "track-gain+", "help"]);
    assert!(face.verb("track-gain-"));
    assert_eq!(face.last_error.take(), Some(format!("track-gain-: {}", EditError::NoTrack)));
    face.state.select(1, 0).unwrap();
    assert!(face.verb("track-gain-"));
    assert_eq!(face.last_error.take().as_deref(), Some("track-gain: A1 -1.0 dB"));
    assert!(face.verb("track-gain+"));
    assert!(face.verb("track-gain+"));
    assert_eq!(face.last_error.take().as_deref(), Some("track-gain: A1 +1.0 dB"));
    assert_eq!((face.state.timeline.tracks[1].gain_cdb, face.state.undo_depth()), (100, 3));
    assert!(face.verb("track-gain+ 2"));
    assert_eq!(face.last_error.take(), Some(format!("track-gain+ 2: {}", EditError::Gain)));
    assert_eq!(stria::nle_face::db_words(-350), "-3.5 dB");
    println!("striatrackgainstep face: track-gain- -> A1 -1.0 dB, two track-gain+ -> +1.0 dB; no selection -> {}", EditError::NoTrack);
}

// ---- STRIALINKAUTO (LEDGER SR911): an opened media file's V1 / A1 pair is linked; the `unlink-all` word (named tail block) ----
#[test]
fn strialinkauto_from_media_links_v1_a1() {
    let (tl, _) = Sources::from_media("AX", &synth_source(0, 50, 25), 25).unwrap().unwrap();
    assert_eq!((tl.tracks[0].kind, tl.tracks[1].kind), (Kind::Video, Kind::Audio));
    assert_eq!((tl.tracks[0].clips[0].group, tl.tracks[1].clips[0].group), (1, 1), "the picture and its sound share one group");
    let mut st = EditorState::new(tl);
    st.select(0, 0).unwrap();
    assert_eq!(st.set, vec![(0, 0), (1, 0)], "selecting the picture takes its sound");
    st.seek(20);
    assert_eq!(st.verb("split"), Some(Ok(())));
    let groups = |t: &Timeline| t.tracks.iter().map(|tr| tr.clips.iter().map(|c| c.group).collect::<Vec<_>>()).collect::<Vec<_>>();
    assert_eq!(groups(&st.timeline), vec![vec![1, 2], vec![1, 2]], "both tracks cut: heads keep the id, tails share a fresh one");
    let text = edl::write(&st.timeline, "AUTO").unwrap();
    assert!(text.contains("* UNA GROUP 1\n") && text.contains("* UNA GROUP 2\n"), "{text}");
    let back = edl::parse(&text, 25).unwrap();
    let mut got: Vec<u32> = back.tracks.iter().flat_map(|tr| tr.clips.iter().map(|c| c.group)).collect();
    got.sort_unstable();
    assert_eq!(got, vec![1, 1, 2, 2], "the links round-trip through the EDL");
    println!("strialinkauto: opened V1 / A1 linked, split into groups {:?}", groups(&st.timeline));
}

#[test]
fn strialinkauto_face_unlink_all_word() {
    let (tl, src) = synth_fixture();
    let tl0 = tl.clone();
    let mut face = nlefaceexport_face(tl.link_av(), src);
    assert_eq!(&VERBS[VERBS.len() - 3..], ["unlink-all", "link-av", "help"], "unlink-all, then link-av, just before help"); // STRIALINKAUTOFUZZ (SR928): `link-av` before help
    assert!(face.verb("unlink-all"));
    assert_eq!(face.last_error.take(), None);
    assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (tl0, 1), "every pair parted, one step");
    println!("strialinkauto: VERBS[{}] unlink-all", VERBS.len() - 2);
}

// ---- STRIAGAPSBLACK (LEDGER SR909): close-gaps names the black fades it removed; the bare `gap` word (named tail block) ----

/// The synth fixture's V1 with a 3-tick to-black at 20 (B's end, the 20..25 gap): `close-gaps` closes the gap, removes the
/// fade in the same commit and names both on the message row; one undo brings the fade back. The bare `gap`
/// word opens one second (25 ticks) at the playhead; `gap` sits right before `close-gaps` in VERBS.
#[test]
fn striagapsblack_face_names_the_removed_fades() {
    let i = VERBS.iter().position(|&v| v == "gap").expect("gap in VERBS");
    assert_eq!(VERBS[i + 1], "close-gaps");
    let (tl, src) = synth_fixture();
    let tl = tl.add_transition_to_black(0, 20, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("close-gaps"));
    assert_eq!(face.note.take().as_deref(), Some("close-gaps: closed 1 gap, removed 1 fade")); // STRIAGAPSTHREE (SR9028): the success line is a face note, not an error
    assert_eq!(face.state.timeline, tl.close_gaps(0).unwrap());
    assert!(face.state.timeline.tracks[0].transitions.is_empty());
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    face.state.seek(10);
    assert!(face.verb("gap"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl.insert_gap(0, 10, 25).unwrap());
    println!("striagapsblack: VERBS[{i}] = gap; V1 after gap at 10: {:?}", face.state.timeline.tracks[0].clips);
}

// ---- STRIALINKTRIM (LEDGER SR910): the face's slip button carries the link mates (named tail block) ----
#[test]
fn strialinktrim_face_slip_word() {
    let (tl, src) = synth_fixture();
    let tl0 = tl.clone();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select(0, 2).unwrap();
    face.state.select_add(1, 2).unwrap();
    assert!(face.verb("link"));
    face.state.deselect();
    face.state.select(0, 2).unwrap();
    assert!(face.verb("slip+"));
    assert_eq!(face.last_error.take(), None);
    let want = tl0.set_group(&[(0, 2), (1, 2)], 1).unwrap().slip(0, 2, 1).unwrap().slip(1, 2, 1).unwrap();
    assert_eq!(face.state.timeline, want, "picture and sound slipped together");
    assert_eq!(face.state.undo_depth(), 2, "link, slip: one step each");
    println!("strialinktrim: linked pair slipped to src_in {}", face.state.timeline.tracks[1].clips[2].src_in);
}

// ---- STRIAIMPORTWAVWIDE (LEDGER SR920): wide and streamed WAV reels (named tail block) ----

/// STRIAIMPORTWAVWIDE (SR920): a RIFF/WAVE file with a 16-byte `fmt ` of `tag` at `bits`; the `data` size field is
/// `size` when given (0 / 0xFFFFFFFF = streamed), else the payload's length.
fn striaimportwavwide_wav(tag: u16, bits: u16, ch: u16, rate: u32, size: Option<u32>, payload: &[u8]) -> Vec<u8> {
    let align = ch * (bits / 8);
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + payload.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&tag.to_le_bytes());
    w.extend_from_slice(&ch.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * align as u32).to_le_bytes());
    w.extend_from_slice(&align.to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&size.unwrap_or(payload.len() as u32).to_le_bytes());
    w.extend_from_slice(payload);
    w
}

/// STRIAIMPORTWAVWIDE (SR920): `n` mono samples `striaimportwav_sample(i)` = v laid out at `bits` (`float`): 8-bit
/// unsigned (v>>8)+128, 16-bit v, 24-bit v<<8, 32-bit integer v<<16, float 64 v/32768.
fn striaimportwavwide_payload(bits: u16, float: bool, n: usize) -> Vec<u8> {
    let mut p = Vec::new();
    for i in 0..n {
        let v = striaimportwav_sample(i) as i32;
        match (float, bits) {
            (true, _) => p.extend_from_slice(&(v as f64 / 32768.0).to_le_bytes()),
            (false, 8) => p.push(((v >> 8) + 128) as u8),
            (false, 16) => p.extend_from_slice(&(v as i16).to_le_bytes()),
            (false, 24) => p.extend_from_slice(&(v << 8).to_le_bytes()[..3]),
            (false, _) => p.extend_from_slice(&(v << 16).to_le_bytes()),
        }
    }
    p
}

#[test]
fn striaimportwavwide_widths_render_exact() {
    // Mono 48 kHz, 2 s = 50 ticks at 25 fps; the clip starts 10 ticks in (a seek mid-file).
    for (tag, bits, float) in [(1u16, 8u16, false), (1, 24, false), (1, 32, false), (3, 64, true)] {
        let wav = striaimportwavwide_wav(tag, bits, 1, 48_000, None, &striaimportwavwide_payload(bits, float, 96_000));
        let d = gneiss_pal::dsp::demux::Demuxer::open_wav(wav.clone()).expect("open_wav");
        assert_eq!(d.tracks()[0].codec, gneiss_pal::dsp::demux::Codec::Pcm { bits, float, big_endian: false });
        let (src, s) = striaimportwav_render(wav, (10, 30), 20);
        assert_eq!(timeline_core::SourceIndex::duration(&src, SourceId(0)), Some(50));
        assert_eq!((s.rate, s.ch), (48_000, 1));
        assert_eq!(s.pcm.len(), 20 * 1920);
        let skip = 10 * 1920;
        for (j, got) in s.pcm.iter().enumerate() {
            let v = striaimportwav_sample(skip + j) as i32;
            let want = if bits == 8 { q((v >> 8) << 8) } else { q(v) };
            assert_eq!(*got, want, "{bits}-bit float={float} sample {j}");
        }
    }
}

#[test]
fn striaimportwavwide_streamed_data_runs_to_eof() {
    for size in [0u32, 0xFFFF_FFFF] {
        let mut payload = striaimportwavwide_payload(16, false, 96_000);
        payload.push(0x5A); // a stray byte: a partial frame, dropped
        let wav = striaimportwavwide_wav(1, 16, 1, 48_000, Some(size), &payload);
        let (src, s) = striaimportwav_render(wav, (0, 50), 50);
        assert_eq!(timeline_core::SourceIndex::duration(&src, SourceId(0)), Some(50), "size field {size:#x}");
        assert_eq!(s.pcm.len(), 96_000);
        for (j, got) in s.pcm.iter().enumerate() {
            assert_eq!(*got, q(striaimportwav_sample(j) as i32), "size field {size:#x} sample {j}");
        }
    }
}

#[test]
fn striaimportwavwide_refusals_by_name() {
    let refusal = |bytes: Vec<u8>| {
        let tl = Timeline::new(25);
        let mut src = Sources::new(&tl);
        src.add(SourceId(0), bytes).expect_err("refused").to_string()
    };
    let half = striaimportwavwide_payload(16, false, 2400);
    // A non-streamed size past the end of the file is still refused.
    let e = refusal(striaimportwavwide_wav(1, 16, 1, 48_000, Some(9600), &half));
    assert!(e.contains("wav: data chunk runs past the end of the file"), "{e}");
    // Float at 16 bits and integer at 12 bits are not layouts the PCM decoder reads.
    let e = refusal(striaimportwavwide_wav(3, 16, 1, 48_000, None, &half));
    assert!(e.contains("wav: sample layout (8/16/24/32-bit integer or 32/64-bit float)"), "{e}");
    let e = refusal(striaimportwavwide_wav(1, 12, 1, 48_000, None, &half));
    assert!(e.contains("wav: sample layout (8/16/24/32-bit integer or 32/64-bit float)"), "{e}");
    // A streamed header with no sample after it.
    let e = refusal(striaimportwavwide_wav(1, 16, 1, 48_000, Some(0), &[]));
    assert!(e.contains("wav: no samples"), "{e}");
}

// ---- STRIAEXPORTQUEUESKIP (LEDGER SR914): the queue's skip / clear words and the queued-timeline snapshot (named tail block) ----

/// Drain the running export and the queue through `export_tick` alone (the serve clock's road), bounded.
fn striaexportqueueskip_drain(face: &mut Face) {
    let t0 = std::time::Instant::now();
    while face.exporting() {
        assert!(t0.elapsed() < std::time::Duration::from_secs(120), "the queue never drained");
        face.export_tick();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn striaexportqueueskip_skip_word_starts_the_next() {
    let (a, b) = (nlefaceexport_dir("skip-a"), nlefaceexport_dir("skip-b"));
    let mut face = striaexportqueue_paused(&a);
    assert!(face.verb(&format!("export {} png", b.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (1 waiting)"));
    assert!(face.verb("export-skip"));
    assert_eq!(face.last_error.take().as_deref(), Some("export: skipping (1 waiting)"));
    striaexportqueueskip_drain(&mut face);
    assert_eq!(face.export_queued(), 0);
    let line = face.last_error.take().expect("B's line");
    assert!(line.starts_with("export: 35 frames, ") && line.ends_with(&format!(" -> {}", b.display())), "{line}");
    assert!(a.join("000002.png").is_file() && !a.join("000003.png").exists(), "A cancelled after frame 3");
    assert!(b.join("000034.png").is_file(), "B ran to the end after the skip");
    for d in [&a, &b] {
        let _ = std::fs::remove_dir_all(d);
    }
    println!("striaexportqueueskip: export-skip started the queued job: \"{line}\"");
}

#[test]
fn striaexportqueueskip_skip_with_nothing_running() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("export-skip"));
    assert_eq!(face.last_error.take().as_deref(), Some("export-skip: no export running"));
    assert!(face.verb("export-clear"));
    assert_eq!(face.last_error.take().as_deref(), Some("export: dropped 0 waiting"));
    assert_eq!(face.state.timeline, tl);
    assert_eq!(face.state.undo_depth(), 0, "view words push no undo step");
    assert!(!face.dirty(), "view words leave the document clean");
    assert!(!face.exporting());
    println!("striaexportqueueskip: nothing running -> export-skip: no export running");
}

#[test]
fn striaexportqueueskip_clear_keeps_the_running_job() {
    let (a, b, c) = (nlefaceexport_dir("clear-a"), nlefaceexport_dir("clear-b"), nlefaceexport_dir("clear-c"));
    let mut face = striaexportqueue_paused(&a);
    assert!(face.verb(&format!("export {} png", b.display())));
    assert!(face.verb(&format!("export {} png", c.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (2 waiting)"));
    assert!(face.verb("export-clear"));
    assert_eq!(face.last_error.take().as_deref(), Some("export: dropped 2 waiting"));
    assert_eq!(face.export_queued(), 0);
    assert!(face.exporting(), "the running job is left running");
    assert_eq!(face.export_progress(), Some((3, 35)));
    assert!(face.export_skip());
    assert_eq!(face.wait_exports(), 1);
    assert!(!b.exists() && !c.exists(), "the dropped requests made nothing");
    let _ = std::fs::remove_dir_all(&a);
    println!("striaexportqueueskip: export-clear dropped 2, the running job kept");
}

#[test]
fn striaexportqueueskip_queued_job_exports_the_timeline_as_queued() {
    let (a, b) = (nlefaceexport_dir("snap-a"), nlefaceexport_dir("snap-b"));
    let mut face = striaexportqueue_paused(&a);
    assert!(face.verb(&format!("export --all {} png", b.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (1 waiting)"));
    for t in &mut face.state.timeline.tracks {
        t.clips.truncate(1); // A[13,23)@0: the live timeline now ends at 10
    }
    let live = face.state.timeline.clone();
    assert_eq!(live.end(), 10);
    assert!(face.export_skip());
    assert_eq!(face.wait_exports(), 2);
    let line = face.last_error.take().expect("B's line");
    assert!(line.starts_with("export: 35 frames, ") && line.ends_with(&format!(" -> {}", b.display())), "{line}");
    assert!(b.join("000034.png").is_file(), "B exported the timeline as it stood when queued (35 frames)");
    assert_eq!(face.state.timeline, live, "the live timeline is back after the start");
    for d in [&a, &b] {
        let _ = std::fs::remove_dir_all(d);
    }
    println!("striaexportqueueskip: queued at 35 frames, live cut to 10, exported 35: \"{line}\"");
}

#[test]
fn striaexportqueueskip_words_are_verbs_before_help() {
    for w in ["export-skip", "export-clear"] {
        assert!(VERBS.contains(&w), "{w}");
    }
    assert_eq!(VERBS.last(), Some(&"help"));
    assert!(VERBS.contains(&"export-skip") && VERBS.contains(&"export-clear") && VERBS.last() == Some(&"help")); // STRIAEXPORTQUEUELIST (SR932): relaxed (siblings add words before help)
    println!("striaexportqueueskip: VERBS ends {:?}", &VERBS[VERBS.len() - 3..]);
}

// ---- STRIAKEYMAPSTORE (LEDGER SR917): the key map's Principia row — names, precedence, the stored map into the face (named tail block) ----
use stria::nle_face::{KEYS_KEY, KEYS_NS, keys_text};

/// The row is `stria.keys` (prefs_core SCHEMA's declared key).
#[test]
fn striakeymapstore_row_names() {
    assert_eq!((KEYS_NS, KEYS_KEY), ("stria", "keys"));
}

/// The flag wins and the store is not asked; `--keys ""` wins over a stored map; an unset row is the built-in table.
#[test]
fn striakeymapstore_flag_wins_over_store() {
    assert_eq!(keys_text(Some("p play"), || panic!("the store is not asked when --keys is given")), "p play");
    assert_eq!(keys_text(Some(""), || panic!("the store is not asked when --keys is given")), "");
    assert_eq!(keys_text(None, || Some("p play".to_string())), "p play");
    assert_eq!(keys_text(None, || None), "");
}

/// An unset row gives the face the empty map and no message; a stored map binds through `Face::with_keys`; a refused stored
/// map keeps the built-in table and says why.
#[test]
fn striakeymapstore_stored_map_reaches_the_face() {
    let f = striakeyboardmap_face(&keys_text(None, || None));
    assert!(f.keymap().is_empty());
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    let mut f = striakeyboardmap_face(&keys_text(None, || Some("p play;? help".to_string())));
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    assert!(!f.playing);
    assert!(striakeyboardmap_text(&mut f, "p"));
    assert!(f.playing, "p plays through the stored map");
    let f = striakeyboardmap_face(&keys_text(None, || Some("p spilt".to_string())));
    assert!(f.keymap().is_empty());
    assert_eq!(f.last_error.as_deref(), Some("keys: line 1: unknown action \"spilt\""));
    println!("striakeymapstore witness: stored \"p play;? help\" + p -> playing");
}

// ---- STRIAPASTEKEYS (LEDGER SR921): the clipboard chords through KEYS (named tail block) ----

/// The eight chord rows: each name bound once, each action a VERBS word with a HELP phrase that is not the word, and the
/// help row of each action lists its Ctrl and Meta chords.
#[test]
fn striapastekeys_rows_and_help() {
    let chords = [("Ctrl+c", "Meta+c", "copy"), ("Ctrl+x", "Meta+x", "cut"), ("Ctrl+v", "Meta+v", "paste"), ("Ctrl+Shift+V", "Meta+Shift+V", "paste-insert")];
    let rows = help_rows();
    for (c, m, w) in chords {
        assert!(KEYS.contains(&(c, w)) && KEYS.contains(&(m, w)), "{w}");
        assert_eq!(KEYS.iter().filter(|(k, _)| *k == c || *k == m).count(), 2, "{w}: a chord bound twice");
        assert!(VERBS.contains(&w), "{w}");
        assert!(HELP.iter().any(|(a, p)| *a == w && *p != w), "{w}: a phrase, not the word");
        let row = rows.iter().find(|r| r.1 == help_phrase(w)).expect("a help row");
        assert!(row.0.split(" / ").any(|k| k == c) && row.0.split(" / ").any(|k| k == m), "{:?}", row.0);
    }
    println!("striapastekeys: help rows={} keys={}", rows.len(), KEYS.len());
}

/// What the surfaces send: `BrowserCopy` copies (no edit), `BrowserPaste` (its text ignored) pastes at the playhead as
/// ONE undo step, `Ctrl+x` lifts, `Ctrl+Shift+V` pastes rippling the track (SR896's numbers).
#[test]
fn striapastekeys_surface_messages_reach_the_words() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    let (a, b) = (SourceId(0), SourceId(1));
    face.state.select(0, 1).unwrap();
    assert!(face.on(&bandy::SMessage::BrowserCopy));
    assert_eq!((face.dirty(), face.state.clipboard.len()), (false, 1));
    face.state.seek(35);
    assert!(face.on(&bandy::SMessage::BrowserPaste("anything".into())));
    assert_eq!(face.state.timeline.tracks[0].clips[3], Clip::new(b, 0, 10, 35));
    assert_eq!((face.state.undo_depth(), face.last_error.clone()), (1, None));
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    face.state.select(0, 1).unwrap();
    assert!(face.on(&bandy::SMessage::BrowserKey("Ctrl+x".into())));
    assert_eq!(face.state.timeline, tl.lift(0, 1).unwrap());
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    face.state.select(0, 1).unwrap();
    assert!(face.on(&bandy::SMessage::BrowserCopy));
    face.state.seek(12);
    assert!(face.on(&bandy::SMessage::BrowserKey("Ctrl+Shift+V".into())));
    assert_eq!(face.state.timeline.tracks[0].clips, vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 2, 10), Clip::new(b, 0, 10, 12), Clip::new(b, 2, 10, 22), Clip::new(a, 23, 33, 35)]);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("striapastekeys: copy / paste at 35, cut, paste-insert at 12 through the surface messages");
}

/// While the help shows, `BrowserCopy` / `BrowserPaste` only dismiss it (nothing copied, nothing pasted); with an empty
/// clipboard `BrowserPaste` lands `paste: <NoClip>` on the message line and the timeline stays.
#[test]
fn striapastekeys_help_dismiss_and_refusal() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("help") && face.help_shown());
    assert!(face.on(&bandy::SMessage::BrowserCopy));
    assert!(!face.help_shown());
    assert!(face.state.clipboard.is_empty(), "the dismiss is not applied");
    assert!(face.verb("help"));
    assert!(face.on(&bandy::SMessage::BrowserPaste(String::new())));
    assert!(!face.help_shown());
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.on(&bandy::SMessage::BrowserPaste(String::new())));
    assert_eq!(face.last_error.take(), Some(format!("paste: {}", EditError::NoClip)));
    assert_eq!(face.state.timeline, tl);
}

// ---- STRIAUNDOHISTORYDRAG (LEDGER SR912): a drag's step named by its kind, `history <n>` (named tail block) ----

/// A drag's undo entry carries its kind's word (`Drag::finish` arms it through `arm_word`): a plain 40 px drag of B is
/// `drag move`, a Ctrl drag `drag slip`; undo / redo move the names with the snapshots; a transition's right-edge drag
/// is `drag transition out`, and a refused one adds no entry.
#[test]
fn striaundohistorydrag_drag_names_its_step() {
    let (mut face, _twin, _tl) = nle3eslipface_pair();
    let l = face.layout();
    let (x, y) = l.point(0, 15);
    nle3eslipdrag_drag(&mut face, x, y, 40.0, 0);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("history"));
    assert_eq!(face.last_error.take().as_deref(), Some("history: 1 drag move"));
    let l = face.layout();
    let at = face.state.selected().map(|c| c.at).unwrap();
    let (x, y) = l.point(0, at + 2);
    nle3eslipdrag_drag(&mut face, x, y, 40.0, bandy::MOD_CONTROL);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.verb("history"));
    assert_eq!(face.last_error.take().as_deref(), Some("history: 2 drag slip | 1 drag move"));
    assert!(face.verb("undo"));
    assert!(face.verb("history"));
    assert_eq!(face.last_error.take().as_deref(), Some("history: 1 drag move"));
    assert!(face.verb("redo"));
    assert!(face.verb("history"));
    assert_eq!(face.last_error.take().as_deref(), Some("history: 2 drag slip | 1 drag move"));

    let (tl, src) = synth_fixture();
    let tl = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
    let mut face = nlefaceexport_face(tl, src);
    let tr = face.state.timeline.tracks[0].transitions[0];
    let l = face.layout();
    let (_, y0, x1, y1) = l.transition_rect(0, &tr);
    let (x, y) = (x1 as f64 - 1.0, (y0 + y1) as f64 / 2.0);
    let tick = face.state.zoom as f64;
    nle3eslipdrag_drag(&mut face, x, y, 1000.0 * tick, bandy::MOD_ALT);
    if face.last_error.take().is_some() {
        assert_eq!(face.state.undo_depth(), 0, "a refused edge drag adds no entry");
    } else {
        assert!(face.verb("undo"), "the long edge drag committed: take it back");
    }
    nle3eslipdrag_drag(&mut face, x, y, tick, bandy::MOD_ALT);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline.tracks[0].transitions[0].dur, 5);
    assert!(face.verb("history"));
    let line = face.last_error.take();
    assert_eq!(line.as_deref(), Some("history: 1 drag transition out"));
    println!("striaundohistorydrag: {line:?}");
}

/// `history <n>` lists the top n entries (bounded by the stack's cap, `UNDO_MAX`); a count of 0, above the cap or not a
/// number is refused by name on the message line; none of them commits, dirties or moves the undo stack.
#[test]
fn striaundohistorydrag_history_count() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select(0, 1).unwrap();
    face.state.seek(15);
    assert!(face.verb("split"));
    assert!(face.verb("delete"));
    face.state.select(0, 0).unwrap();
    assert!(face.verb("slip+"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let (tl1, depth, dirty) = (face.state.timeline.clone(), face.state.undo_depth(), face.dirty());
    assert_eq!(depth, 3);
    for (v, want) in [
        ("history 2", "history: 3 slip+ | 2 delete"),
        ("history 1", "history: 3 slip+"),
        ("history 99", "history: 3 slip+ | 2 delete | 1 split"),
        ("history 0", "history: \"0\" is not a count 1..=256"),
        ("history 257", "history: \"257\" is not a count 1..=256"),
        ("history x", "history: \"x\" is not a count 1..=256"),
    ] {
        assert!(face.verb(v), "{v}");
        assert_eq!(face.last_error.take().as_deref(), Some(want), "{v}");
        assert_eq!((face.state.timeline == tl1, face.state.undo_depth(), face.dirty()), (true, depth, dirty), "{v}");
    }
}

// ---- STRIAAUTOSAVECAP (LEDGER SR913): the count past UNDO_MAX, the witness field, a declined autosave kept (named tail block) ----
// `EditorState::commits` moves at every commit / undo / redo and never falls back, so the face's edit count keeps seeing
// edits after the undo stack is full; an autosave this face neither wrote nor recovered is renamed to
// `<stem>.autosave.1.edl` before the first write or a save would replace / remove it.

use stria::nle_face::{UNDO_MAX, autosave_keep_path};

/// A newer foreign autosave (holding `tl0` split at 15) beside a document of `tl0`; the face opened from it (offered,
/// not recovered): the face, the document path, the foreign bytes.
fn striaautosavecap_foreign(name: &str) -> (Face, std::path::PathBuf, String) {
    let (tl0, src) = synth_fixture();
    let doc = common::fresh_dir("striaautosavecap", name).join("cut.edl");
    std::fs::write(&doc, edl::write(&tl0, "cut").unwrap()).unwrap();
    let old = edl::write(&tl0.split(0, 15).unwrap(), "cut").unwrap();
    std::fs::write(autosave_path(&doc), &old).unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    std::fs::File::options().write(true).open(&doc).unwrap().set_modified(past).unwrap();
    let mut face = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(doc.as_path()));
    assert!(face.last_error.take().is_some_and(|l| l.starts_with("autosave: cut.autosave.edl is newer")), "offered at open");
    (face, doc, old)
}

#[test]
fn striaautosavecap_commits_counts_every_change() {
    let (tl0, _) = synth_fixture();
    let mut st = EditorState::new(tl0.clone());
    assert_eq!(st.commits(), 0);
    assert!(st.split_at_playhead().is_err(), "nothing selected");
    assert_eq!(st.commits(), 0, "a refused edit counts nothing");
    st.select_at(0, 15).unwrap();
    st.seek(15);
    st.split_at_playhead().unwrap();
    assert_eq!(st.commits(), 1);
    assert!(st.undo());
    assert_eq!((st.commits(), st.undo_depth()), (2, 0), "undo is a change; the depth falls, the count does not");
    assert!(st.redo());
    assert_eq!(st.commits(), 3);
    assert!(st.undo() && !st.undo());
    assert_eq!(st.commits(), 4, "an empty undo counts nothing");
    assert!(!st.restore(tl0));
    assert_eq!(st.commits(), 4, "a restore of the same timeline counts nothing");
}

#[test]
fn striaautosavecap_writes_past_undo_max() {
    let (mut face, doc, tl0) = striaautosave_face("cap");
    let auto = autosave_path(&doc);
    let (a, b) = (tl0.split(0, 15).unwrap(), tl0.split(0, 5).unwrap());
    let flip = |face: &mut Face, n: usize| {
        for _ in 0..n {
            let next = if face.state.timeline == a { b.clone() } else { a.clone() };
            assert!(face.state.restore(next));
        }
    };
    flip(&mut face, UNDO_MAX + 44);
    assert_eq!(face.state.undo_depth(), UNDO_MAX);
    assert!(!face.autosave_tick());
    assert_eq!(face.autosaved(), Some(&face.state.timeline));
    flip(&mut face, AUTOSAVE_EDITS - 1);
    assert_eq!(face.state.undo_depth(), UNDO_MAX, "the depth no longer moves");
    assert!(!face.autosave_tick());
    assert_ne!(face.autosaved(), Some(&face.state.timeline), "7 edits and one tick: not yet");
    flip(&mut face, 2);
    assert!(!face.autosave_tick());
    assert_eq!(face.autosaved(), Some(&face.state.timeline), "the count saw the edits at the cap");
    assert_eq!(std::fs::read_to_string(&auto).unwrap(), edl::write(&face.state.timeline, "cut").unwrap());
    assert_eq!(face.state.undo_depth(), UNDO_MAX);
    println!("striaautosavecap: {} commits at undo depth {}; the autosave follows", face.state.commits(), UNDO_MAX);
}

#[test]
fn striaautosavecap_declined_autosave_kept_on_first_write() {
    let (mut face, doc, old) = striaautosavecap_foreign("write");
    let (auto, keep) = (autosave_path(&doc), autosave_keep_path(&doc));
    assert_eq!(keep, doc.with_file_name("cut.autosave.1.edl"));
    for t in 1..=AUTOSAVE_EDITS as i64 {
        striaautosave_split(&mut face, t);
    }
    assert!(face.autosave_tick(), "the keep note repaints"); assert_eq!(face.last_error.take().as_deref(), Some("autosave: kept the older autosave as cut.autosave.1.edl")); // STRIAAUTOSAVEKEEPREAD (SR933): the keep is named once
    assert_eq!(std::fs::read_to_string(&keep).unwrap(), old, "the declined autosave is kept");
    assert_eq!(std::fs::read_to_string(&auto).unwrap(), edl::write(&face.state.timeline, "cut").unwrap());
    std::fs::write(&keep, "unchanged").unwrap();
    striaautosave_split(&mut face, 30);
    for _ in 0..AUTOSAVE_SECS * 25 {
        face.autosave_tick();
    }
    assert_eq!(std::fs::read_to_string(&keep).unwrap(), "unchanged", "this session's own autosave is never kept");
}

#[test]
fn striaautosavecap_declined_autosave_kept_on_save() {
    let (mut face, doc, old) = striaautosavecap_foreign("save");
    striaautosave_split(&mut face, 15);
    assert!(face.verb("save"));
    assert_eq!(face.last_error.take().as_deref(), Some("autosave: kept the older autosave as cut.autosave.1.edl"), "save refused nothing; the keep is named"); // STRIAAUTOSAVEKEEPREAD (SR933)
    assert!(!autosave_path(&doc).exists(), "saved: no autosave");
    assert_eq!(std::fs::read_to_string(autosave_keep_path(&doc)).unwrap(), old, "the declined autosave is kept");
}

#[test]
fn striaautosavecap_recovered_is_not_kept() {
    let (mut face, doc, _) = striaautosavecap_foreign("recover");
    assert!(face.verb("recover"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.verb("save"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(!autosave_path(&doc).exists());
    assert!(!autosave_keep_path(&doc).exists(), "a recovered autosave is this session's: removed, not kept");
}

// ---- STRIATRACKORDERKEYS (LEDGER SR922): Alt+Up / Alt+Down, the move named on the message row, the crossfade refusal (named tail block) ----

/// GTK's chord as the window sends it: a named `BrowserKey` (no `BrowserText` while Alt is held).
fn striatrackorderkeys_key(face: &mut Face, k: &str) -> bool {
    face.on(&bandy::SMessage::BrowserKey(k.into()))
}

/// Rule: `Alt+Up` on V2's clip is the `track-up` word (one commit, the selection follows) and the message row names the
/// move with the names from before it; `Alt+Down` swaps it back and names that move.
#[test]
fn striatrackorderkeys_alt_keys_move_and_name_the_move() {
    let (mut face, tl) = striatrackreorder_face();
    assert!(KEYS.contains(&("Alt+Up", "track-up")) && KEYS.contains(&("Alt+Down", "track-down")));
    assert!(striatrackorderkeys_key(&mut face, "Alt+Up"));
    assert_eq!(face.state.timeline, tl.swap_track(2, true).unwrap());
    assert_eq!(face.state.selection, Some((0, 0)));
    assert_eq!(face.state.undo_depth(), 1, "ONE undo step");
    assert_eq!(face.message_text().as_deref(), Some("track-up: V2 above V1"));
    assert_eq!(face.last_error.take().as_deref(), Some("track-up: V2 above V1"));
    assert!(striatrackorderkeys_key(&mut face, "Alt+Down"));
    assert_eq!(face.state.timeline, tl);
    assert_eq!(face.last_error.take().as_deref(), Some("track-down: V1 below V2"));
    assert_eq!(face.state.undo_depth(), 2);
    println!("striatrackorderkeys: Alt+Up -> \"track-up: V2 above V1\", Alt+Down -> \"track-down: V1 below V2\"");
}

/// The help shows each chord in its word's row with its phrase (not the bare word).
#[test]
fn striatrackorderkeys_help_rows_name_the_chords() {
    let rows = help_rows();
    for (k, w) in [("Alt+Up", "track-up"), ("Alt+Down", "track-down")] {
        let row = rows.iter().find(|r| r.0.split(" / ").any(|x| x == k)).expect("a help row");
        assert_eq!(row.1, help_phrase(w));
        assert!(HELP.iter().any(|(a, _)| *a == w) && row.1 != w, "{w}: a phrase");
    }
    assert_eq!(rows.last().map(|r| r.1), Some(help_phrase("help")), "help stays the last row");
}

/// Rule: V1 and V2 both carry a dissolve on the cut at 10 whose clips pair with A1's cut, with different durations; the
/// audio crossfade takes the first video track's, so swapping them is refused by name on the message row, unchanged.
#[test]
fn striatrackorderkeys_reorder_that_retimes_a_crossfade_is_refused() {
    let (mut tl, src) = synth_fixture();
    let v2 = tl.add_track(Kind::Video);
    tl.tracks[v2].clips = tl.tracks[0].clips.clone();
    tl = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 2).unwrap();
    tl = tl.add_transition(v2, 10, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(tl.audio_crossfade(1, 1).map(|x| x.dur), Some(2), "the first video track's dissolve");
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select_at(v2, 5).unwrap();
    assert!(striatrackorderkeys_key(&mut face, "Alt+Up"));
    assert_eq!(face.last_error.take(), Some(format!("track-up: {}", EditError::Crossfade)));
    assert_eq!(face.state.timeline, tl);
    assert_eq!(face.state.undo_depth(), 0);
}


// ---- STRIAPROXYOFF (LEDGER SR915): proxy off / all / cancel / auto-attach (named tail block) ----

/// Reels A and B written into a fresh scratch dir (the bytes `synth_fixture` opened), and a face over them with their paths.
fn striaproxyoff_face(case: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf, Face, Timeline) {
    let dir = common::scratch_dir("striaproxyoff", case);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (pa, pb) = (dir.join("A.mp4"), dir.join("B.mp4"));
    std::fs::write(&pa, synth_source(0, 50, 25)).unwrap();
    std::fs::write(&pb, synth_source(1000, 50, 25)).unwrap();
    let (tl, src) = synth_fixture();
    let face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false).with_reel_paths(vec![("A".into(), pa.clone()), ("B".into(), pb.clone())]);
    (dir, pa, pb, face, tl)
}

/// `proxy off` detaches the selected clip's source's proxy (the files stay); again: nothing attached. View-only.
#[test]
fn striaproxyoff_off_detaches() {
    let (dir, pa, _, mut face, tl) = striaproxyoff_face("off");
    let a = tl.sources[0].id;
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy") && face.wait_proxy());
    assert!(face.has_proxy(a));
    assert!(face.verb("proxy off"));
    assert_eq!(face.last_error.take().as_deref(), Some("proxy: A: off (the preview reads the original)"));
    assert!(!face.has_proxy(a) && stria::nle_face::proxy_paths(&pa).0.is_file(), "detached; the file stays");
    assert!(face.verb("proxy off"));
    assert_eq!(face.last_error.take().as_deref(), Some("proxy: A: no proxy attached"));
    assert_eq!(face.state.timeline, tl, "view-only");
    assert!(!face.dirty());
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxyoff off: A detached, then no proxy attached");
}

/// `proxy all` builds every reel with a path, one job at a time in source order (finish chains the next); again: both reused.
#[test]
fn striaproxyoff_all_builds_in_order_then_reuses() {
    let (dir, pa, pb, mut face, tl) = striaproxyoff_face("all");
    let (a, b) = (tl.sources[0].id, tl.sources[1].id);
    let (xa, xb) = (stria::nle_face::proxy_paths(&pa).0, stria::nle_face::proxy_paths(&pb).0);
    assert!(face.verb("proxy all"));
    assert_eq!(face.last_error.take(), Some(format!("proxy: all: 2 reels; A: building (new) -> {}", xa.display())));
    assert!(face.wait_proxy());
    assert_eq!(face.last_error.take(), Some(format!("proxy: A: 50 frames -> {}; B: building (new) -> {}", xa.display(), xb.display())));
    assert!(face.proxying(), "B's job runs after A's");
    assert!(face.wait_proxy());
    assert_eq!(face.last_error.take(), Some(format!("proxy: B: 50 frames -> {}", xb.display())));
    assert!(!face.proxying() && face.has_proxy(a) && face.has_proxy(b));
    assert!(stria::nle_face::proxy_fresh(&pa) && stria::nle_face::proxy_fresh(&pb));
    assert!(face.verb("proxy all"));
    assert_eq!(face.last_error.take(), Some(format!("proxy: all: 2 reels; A: reused {}; B: reused {}", xa.display(), xb.display())));
    assert!(!face.proxying() && !face.dirty());
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxyoff all: A then B built, then both reused");
}

/// Esc cancels a running proxy job (held after frame 3): the message names it, no proxy file or work dir is left, nothing
/// attaches; under `proxy all` the queue is dropped.
#[test]
fn striaproxyoff_esc_cancels_the_job() {
    let (dir, pa, pb, mut face, tl) = striaproxyoff_face("cancel");
    let a = tl.sources[0].id;
    let (xa, _, work) = stria::nle_face::proxy_paths(&pa);
    face.hold_proxy_at(Some(3));
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy") && face.proxying());
    assert!(face.on(&bandy::SMessage::BrowserKey("Escape".into())), "Esc cancels the running proxy job");
    assert!(face.wait_proxy());
    let line = face.last_error.take().unwrap_or_default();
    assert!(line.starts_with("proxy: A: cancelled at 3/"), "{line}");
    assert!(!xa.exists() && !work.exists() && !face.has_proxy(a));
    assert!(!face.on(&bandy::SMessage::BrowserKey("Escape".into())), "Esc with nothing running takes nothing");
    assert!(face.verb("proxy all") && face.proxying());
    assert!(face.cancel_proxy());
    assert!(face.wait_proxy());
    assert!(face.last_error.take().is_some_and(|l| l.starts_with("proxy: A: cancelled at 3/")), "the queue is dropped: B never starts");
    assert!(!face.proxying() && !stria::nle_face::proxy_paths(&pb).0.exists());
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxyoff cancel: {line}");
}

/// A FRESH proxy beside a reel attaches when a face opens with the reel's path; a stale one is left alone (not attached,
/// the file untouched); no proxy on disk = no message.
#[test]
fn striaproxyoff_fresh_proxy_attaches_at_open() {
    let (dir, pa, pb, mut face, tl) = striaproxyoff_face("auto");
    let a = tl.sources[0].id;
    assert_eq!(face.last_error.take(), None, "nothing on disk: no message");
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy") && face.wait_proxy());
    let paths = vec![("A".into(), pa.clone()), ("B".into(), pb.clone())];
    let (t2, s2) = synth_fixture();
    let mut fresh = Face::new(EditorState::new(t2), s2, (320, 160), PixelOrder::Rgba, false).with_reel_paths(paths.clone());
    assert!(fresh.has_proxy(a));
    assert_eq!(fresh.last_error.take().as_deref(), Some("proxy: attached A"));
    let (xa, stamp, _) = stria::nle_face::proxy_paths(&pa);
    let before = std::fs::read(&xa).unwrap();
    std::fs::write(&stamp, "0 0\n").unwrap();
    let (t3, s3) = synth_fixture();
    let mut stale = Face::new(EditorState::new(t3), s3, (320, 160), PixelOrder::Rgba, false).with_reel_paths(paths);
    assert!(!stale.has_proxy(a) && !stale.proxying());
    assert_eq!(stale.last_error.take(), None);
    assert_eq!(std::fs::read(&xa).unwrap(), before, "a stale proxy is left alone");
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxyoff auto: fresh A attached at open; stale left alone");
}

/// `proxy <other>` prints the usage; `proxy off` / `proxy all` without a selection / a path refuse by name. No VERBS word added.
#[test]
fn striaproxyoff_usage_and_refusals() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("proxy foo"));
    assert_eq!(face.last_error.take().as_deref(), Some("proxy: usage: proxy [off|all|purge [all]]")); // STRIAPROXYMORE (SR979): purge all joins the usage; STRIAPROXYBUTTONS (SR930): purge joins the usage
    assert!(face.verb("proxy off"));
    assert_eq!(face.last_error.take().as_deref(), Some("proxy: no clip selected"));
    assert!(face.verb("proxy all"));
    assert_eq!(face.last_error.take().as_deref(), Some("proxy: all: no reel has a source path"));
    assert!(!face.proxying() && !face.dirty());
    assert!(!VERBS.contains(&"proxy off") && !VERBS.contains(&"proxy all"));
    println!("striaproxyoff usage: proxy [off|all]");
}

// ---- STRIAFREEZELEN (LEDGER SR916): the hold's length and `freeze-all` through the face (named tail block) ----

/// synth_fixture (V1 = A1: A[13,23)@0 | B@10 | A@25, 25 fps): `freeze-all 1` at tick 5 lays a 25-tick hold on V1 and opens the
/// same gap on A1, so every clip after the hold sits where its A1 twin sits; one undo restores both tracks.
#[test]
fn striafreezelen_face_freeze_all_keeps_sync() {
    use stria::nle_face::{EditorState, Face, PixelOrder, VERBS, synth_fixture};
    assert_eq!(VERBS.iter().position(|v| *v == "freeze-all").map(|i| (VERBS[i - 1], VERBS[i + 1])), Some(("freeze", "help")), "freeze-all sits between freeze and help");
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
    face.state.seek(5);
    assert!(face.verb("freeze-all 1"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let fr = face.state.timeline.clone();
    assert_eq!(fr.end(), tl.end() + 25, "a 1 s hold at 25 fps");
    let h = fr.tracks[0].clips[1];
    assert_eq!((h.src_in, h.src_out, h.at, h.len()), (18, 19, 5, 25));
    let mut v1 = fr.tracks[0].clips.clone();
    v1.remove(1);
    assert_eq!(v1, fr.tracks[1].clips, "V1 without the hold is A1: picture and sound stay in sync");
    assert_eq!(fr.tracks[1].clips.iter().map(|c| c.at).collect::<Vec<_>>(), vec![0, 30, 35, 50], "A1 split at 5 and rippled by 25");
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.state.undo());
    assert_eq!(face.state.timeline, tl, "one undo step");
    println!("striafreezelen: freeze-all 1 at 5 -> hold 5..30 on V1, A1 gapped 5..30, one undo");
}

/// `freeze <len>`: decimal seconds round half up to a tick, a timecode reads as the EDL's, an inexact hold is refused by name
/// on the message line and pushes no undo step.
#[test]
fn striafreezelen_face_lengths() {
    use stria::nle_face::{EditorState, Face, PixelOrder, synth_fixture};
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
    face.state.seek(5);
    for (word, ticks) in [("freeze 0.5", 13), ("freeze 00:00:01:05", 30), ("freeze", 50)] {
        assert!(face.verb(word));
        assert!(face.last_error.is_none(), "{word}: {:?}", face.last_error);
        assert_eq!((face.state.timeline.end(), face.state.timeline.tracks[0].clips[1].len()), (tl.end() + ticks, ticks), "{word}");
        assert_eq!(face.state.timeline.tracks[1], tl.tracks[1], "{word}: plain freeze leaves A1");
        assert!(face.state.undo());
    }
    assert!(face.verb("freeze 20"));
    assert!(face.last_error.as_deref().is_some_and(|e| e.contains("no exact one-frame hold")), "{:?}", face.last_error);
    assert_eq!((face.state.undo_depth(), &face.state.timeline), (0, &tl), "a refusal leaves the state");
    println!("striafreezelen: freeze 0.5 -> 13, 00:00:01:05 -> 30, bare -> 50; freeze 20 (500 ticks) refused Hold");
}

// ---- STRIAMARKERJUMPKEYS (LEDGER SR923): Up / Down and ; / ' jump; the message row names where the jump landed (named tail block) ----

fn striamarkerjumpkeys_face() -> Face {
    let (tl, src) = synth_fixture();
    Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
}

/// Down / Up (`BrowserKey`, and AppKit's U+F701 / U+F700 as text) step the clip edges; `'` / `;` (text) step the markers;
/// every jump action has a help phrase and a help row naming its keys; `M` stays unbound.
#[test]
fn striamarkerjumpkeys_keys_jump_and_help_rows() {
    let mut f = striamarkerjumpkeys_face();
    assert!(f.on(&bandy::SMessage::BrowserKey("Down".into())));
    assert_eq!(f.state.playhead, 10);
    assert!(f.on(&bandy::SMessage::BrowserKey("Up".into())));
    assert_eq!(f.state.playhead, 0);
    assert!(f.on(&bandy::SMessage::BrowserText("\u{f701}".into())));
    assert_eq!(f.state.playhead, 10, "AppKit's Down is Down");
    assert!(f.on(&bandy::SMessage::BrowserText("\u{f700}".into())));
    assert_eq!(f.state.playhead, 0, "AppKit's Up is Up");
    assert!(!f.on(&bandy::SMessage::BrowserKey(";".into())), "a one-char BrowserKey alone is not taken");
    f.state.timeline = f.state.timeline.mark(5, "A").unwrap().mark(30, "B").unwrap();
    assert!(f.on(&bandy::SMessage::BrowserText("'".into())));
    assert_eq!(f.state.playhead, 5);
    assert!(f.on(&bandy::SMessage::BrowserText("'".into())));
    assert_eq!(f.state.playhead, 30);
    assert!(f.on(&bandy::SMessage::BrowserText(";".into())));
    assert_eq!(f.state.playhead, 5);
    assert_eq!(f.state.undo_depth(), 0, "a jump key is not an undo step");
    assert!(!f.on(&bandy::SMessage::BrowserText("M".into())), "M stays unbound");
    let rows = help_rows();
    for (keys, act) in [(["Up", "\u{f700}"], "cut-"), (["Down", "\u{f701}"], "cut+"), ([";", ";"], "marker-"), (["'", "'"], "marker+")] {
        for k in keys {
            assert!(KEYS.contains(&(k, act)), "{k:?} -> {act}");
        }
        assert!(VERBS.contains(&act), "{act} is a VERBS word");
        let p = help_phrase(act);
        assert!(!p.is_empty() && p != act, "{act}: a phrase, not the word");
        let row = rows.iter().find(|r| r.0.split(" / ").any(|x| x == keys[0])).expect("a help row");
        assert_eq!(row.1, p);
    }
    println!("striamarkerjumpkeys: Down 0 -> 10, Up -> 0, ' 0 -> 5 -> 30, ; -> 5; help rows={}", rows.len());
}

/// The message row names where a jump landed (`marker+: Intro at 00:00:01:05`, `cut+: 00:00:00:10`); a jump with nowhere to
/// go says nothing; no jump is an undo step or dirties the document.
#[test]
fn striamarkerjumpkeys_message_names_where_it_landed() {
    let mut f = striamarkerjumpkeys_face();
    f.state.seek(30);
    assert!(f.verb("mark Intro"));
    assert_eq!(f.last_error.take().as_deref(), Some("marked Intro at 00:00:01:05"));
    let depth = f.state.undo_depth();
    f.state.seek(0);
    assert!(f.verb("marker+"));
    assert_eq!(f.last_error.take().as_deref(), Some("marker+: Intro at 00:00:01:05"));
    f.state.seek(0);
    assert!(f.verb("cut+"));
    assert_eq!(f.last_error.take().as_deref(), Some("cut+: 00:00:00:10"));
    f.state.seek(0);
    assert!(f.verb("cut-"));
    assert_eq!(f.state.playhead, 0);
    assert_eq!(f.last_error.take(), None, "nothing when the playhead did not move");
    assert!(f.on(&bandy::SMessage::BrowserKey("Down".into())));
    assert_eq!(f.last_error.take().as_deref(), Some("cut+: 00:00:00:10"), "the key words the same message");
    assert_eq!(f.state.undo_depth(), depth, "a jump is not an undo step");
    println!("striamarkerjumpkeys: marker+ -> Intro at 00:00:01:05, cut+ -> 00:00:00:10, still cut- -> no message");
}

// ---- STRIAINSERTMARKSKEYS (LEDGER SR924): keys for the insert words and the laid line on the message row (named tail block) ----

/// `w` / `d` / `W` / `D` are KEYS rows for the four insert words, each a VERBS word with a HELP phrase of its own; the help
/// shows each key on its word's row.
#[test]
fn striainsertmarkskeys_keys_rows_and_phrases() {
    let rows = help_rows();
    for (k, w) in [("w", "insert"), ("d", "overwrite"), ("W", "insert-marked"), ("D", "overwrite-marked")] {
        assert!(KEYS.contains(&(k, w)), "{k} -> {w}");
        assert!(VERBS.contains(&w), "{w}: the face word");
        assert!(HELP.iter().any(|(a, _)| *a == w), "{w}: a phrase");
        assert_ne!(help_phrase(w), w, "{w}: a phrase, not the word");
        let row = rows.iter().find(|r| r.1 == help_phrase(w)).expect("a help row");
        assert!(row.0.split(" / ").any(|x| x == k), "{k} not in {:?}", row.0);
    }
    println!("striainsertmarkskeys: KEYS w d W D -> insert overwrite insert-marked overwrite-marked; help rows={}", rows.len());
}

/// In 5, out 9, B selected: the text key `W` lays B 0..4 at 5 on V1 through the one dispatch (one undo step) and the
/// message row names it `insert-marked: B 00:00:00:04 at 00:00:00:05` (no error); the next bound key (`k`) clears it back
/// to the reel line. `D` with the marks cleared is refused by the editor's name and leaves no laid line.
#[test]
fn striainsertmarkskeys_shift_w_fills_the_marks_and_names_it() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 1).unwrap();
    face.state.seek(5);
    assert!(face.verb("in"));
    face.state.seek(9);
    assert!(face.verb("out"));
    let marked = tl.set_marks((Some(5), Some(9))).unwrap();
    assert!(nle5drangekeys_press(&mut face, "W"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, marked.place(0, Clip::new(SourceId(1), 0, 4, 5), timeline_core::Mode::Insert).unwrap());
    assert_eq!(face.message_text().as_deref(), Some("insert-marked: B 00:00:00:04 at 00:00:00:05"));
    assert!(nle5drangekeys_press(&mut face, "k"));
    assert!(face.message_text().is_some_and(|m| m.starts_with("B: ")), "{:?}", face.message_text());
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, marked, "one undo");
    assert!(face.verb("mark clear"));
    let before = face.state.timeline.clone();
    assert!(nle5drangekeys_press(&mut face, "D"));
    assert_eq!(face.last_error.take(), Some(format!("overwrite-marked: {}", EditError::NoMarks)));
    assert_eq!(face.state.timeline, before);
    assert!(!face.message_text().is_some_and(|m| m.starts_with("overwrite-marked")), "{:?}", face.message_text());
    println!("striainsertmarkskeys: in 5 out 9, W -> insert-marked: B 00:00:00:04 at 00:00:00:05; D unmarked -> {}", EditError::NoMarks);
}

/// B selected, playhead 15: `w` inserts and `d` overwrites B 0..10 there, each naming `<word>: B 00:00:00:10 at
/// 00:00:00:15`; GTK's `BrowserKey("w")` alone (the twin of the text key) is not taken and changes nothing.
#[test]
fn striainsertmarkskeys_w_d_at_the_playhead() {
    for (k, word, mode) in [("w", "insert", timeline_core::Mode::Insert), ("d", "overwrite", timeline_core::Mode::Overwrite)] {
        let (tl, src) = synth_fixture();
        let mut face = nlefaceexport_face(tl.clone(), src);
        face.state.select(0, 1).unwrap();
        face.state.seek(15);
        assert!(!face.on(&bandy::SMessage::BrowserKey(k.to_string())), "{k}: a one-char BrowserKey is not taken");
        assert_eq!(face.state.timeline, tl);
        assert!(nle5drangekeys_press(&mut face, k), "{k}");
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        assert_eq!(face.state.timeline, tl.place(0, Clip::new(SourceId(1), 0, 10, 15), mode).unwrap(), "{word}");
        assert_eq!(face.message_text(), Some(format!("{word}: B 00:00:00:10 at 00:00:00:15")));
    }
    println!("striainsertmarkskeys: w / d at 15 -> insert / overwrite: B 00:00:00:10 at 00:00:00:15");
}

// ---- STRIAXFADECURVEBUTTON (LEDGER SR925): the crossfade curve word, its keys, button phrase and the message row (named tail block) ----

/// `xfade-curve` is a transport word just before `help`; `c` puts the crossfade (row `xfade: 10 equal`), `C` swaps its curve
/// (row `xfade: 10 linear`), each one undo step; the help names both keys; a refusal reads its words and moves nothing.
#[test]
fn striaxfadecurvebutton_face_keys_button_and_message() {
    use timeline_core::Curve;
    assert!(VERBS.contains(&"xfade-curve"));
    assert_eq!(VERBS.last(), Some(&"help"));
    assert!(KEYS.contains(&("c", "xfade")) && KEYS.contains(&("C", "xfade-curve")));
    let rows = help_rows();
    for (k, w) in [("c", "xfade"), ("C", "xfade-curve")] {
        let row = rows.iter().find(|r| r.0.split(" / ").any(|x| x == k)).expect("a help row");
        assert_eq!(row.1, help_phrase(w));
        assert!(!row.1.is_empty() && row.1 != w, "{w}: a phrase, not the word");
    }
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(1, 1).unwrap();
    assert!(nle5drangekeys_press(&mut face, "c"));
    assert_eq!(face.note.take().as_deref(), Some("xfade: 10 equal"));
    assert!(nle5drangekeys_press(&mut face, "C"));
    assert_eq!(face.note.take().as_deref(), Some("xfade: 10 linear"));
    assert_eq!(face.state.timeline, tl.add_audio_xfade(1, 10, 10, Curve::Linear).unwrap());
    assert!(face.verb("xfade-curve"));
    assert_eq!(face.note.take().as_deref(), Some("xfade: 10 equal"));
    assert_eq!(face.state.undo_depth(), 3);
    assert!(face.verb("xfade"));
    assert_eq!(face.note.take().as_deref(), Some("xfade: off"));
    let (before, depth) = (face.state.timeline.clone(), face.state.undo_depth());
    assert!(face.verb("xfade-curve"));
    assert_eq!(face.last_error.take().as_deref(), Some("xfade-curve: no crossfade on the selected clip's head cut (xfade puts one)"));
    face.state.select(0, 1).unwrap();
    assert!(face.verb("xfade-curve"));
    assert_eq!(face.last_error.take().as_deref(), Some("xfade-curve: the selected clip is not on an audio track"));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&before, depth));
    println!("striaxfadecurvebutton: c -> 10 equal, C -> 10 linear, xfade-curve -> 10 equal, xfade -> off; refusals in words");
}

// ---- STRIATRACKGAINKEYS (LEDGER SR926): text keys for the audio words, their help rows (named tail block) ----
// `v` / `V` gain- / gain+, `:` / `"` track-gain- / track-gain+, `Q` mute, `S` solo: KEYS rows naming VERBS words, so a
// press is `Face::verb` — the buttons' one commit and message line; HELP carries a phrase for each.

/// The six keys and their words (the KEYS rows this arc adds).
const STRIATRACKGAINKEYS: [(&str, &str); 6] = [("v", "gain-"), ("V", "gain+"), (":", "track-gain-"), ("\"", "track-gain+"), ("Q", "mute"), ("S", "solo")];

/// With A1's first clip selected each key edits in one undo step and reads the buttons' line; Ctrl+z undoes the solo;
/// with nothing selected the keys are refused by the words' names; a one-char key as `BrowserKey` alone is not taken.
#[test]
fn striatrackgainkeys_keys_drive_the_audio_words() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select(1, 0).unwrap();
    assert!(nle5drangekeys_press(&mut face, "v"));
    assert_eq!((face.last_error.take().as_deref(), face.state.selected().unwrap().gain_cdb, face.state.undo_depth()), (Some("gain -1.0 dB"), -100, 1));
    assert!(nle5drangekeys_press(&mut face, "V"));
    assert_eq!((face.last_error.take().as_deref(), face.state.selected().unwrap().gain_cdb, face.state.undo_depth()), (Some("gain 0.0 dB"), 0, 2));
    assert!(nle5drangekeys_press(&mut face, ":"));
    assert_eq!((face.last_error.take().as_deref(), face.state.timeline.tracks[1].gain_cdb, face.state.undo_depth()), (Some("track-gain: A1 -1.0 dB"), -100, 3));
    assert!(nle5drangekeys_press(&mut face, "\""));
    assert_eq!((face.last_error.take().as_deref(), face.state.timeline.tracks[1].gain_cdb, face.state.undo_depth()), (Some("track-gain: A1 0.0 dB"), 0, 4));
    assert!(nle5drangekeys_press(&mut face, "Q"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!((face.state.timeline.tracks[1].muted, face.state.timeline.tracks[0].muted, face.state.undo_depth()), (true, false, 5));
    assert!(nle5drangekeys_press(&mut face, "S"));
    assert_eq!((face.state.timeline.tracks[1].solo, face.state.undo_depth()), (true, 6));
    assert!(face.on(&bandy::SMessage::BrowserKey("Ctrl+z".into())));
    assert_eq!((face.state.timeline.tracks[1].solo, face.state.timeline.tracks[1].muted), (false, true), "undo takes the solo back");
    assert!(!face.on(&bandy::SMessage::BrowserKey("v".into())), "a one-char key is read from the text only");
    face.state.deselect();
    assert!(nle5drangekeys_press(&mut face, "v"));
    assert_eq!(face.last_error.take(), Some(format!("gain-: {}", EditError::NoClip)));
    assert!(nle5drangekeys_press(&mut face, "Q"));
    assert_eq!(face.last_error.take(), Some(format!("mute: {}", EditError::NoTrack)));
    println!("striatrackgainkeys: v / V / : / \" / Q / S each one commit; Ctrl+z undoes S; no selection refused");
}

/// Each pair is a KEYS row and its key is bound once; its help row names the key with a HELP phrase (not the bare
/// word); no key is one the kernel's EDIT_KEYS binds (its bytes as a literal) or SR835's `m` / SR923's `;` / `'`.
#[test]
fn striatrackgainkeys_help_rows_and_no_collision() {
    let kernel = ["s", "x", "u", "z", "Z", "[", "]", "{", "}", "i", "o", "+", "=", "-", "0", "f"];
    let rows = help_rows();
    for (k, w) in STRIATRACKGAINKEYS {
        assert!(KEYS.contains(&(k, w)), "{k} -> {w}");
        assert_eq!(KEYS.iter().filter(|(n, _)| *n == k).count(), 1, "{k} bound once");
        assert!(VERBS.contains(&w), "{w}: a VERBS word (its button)");
        assert!(HELP.iter().any(|(a, _)| *a == w), "no phrase for {w}");
        let row = rows.iter().find(|r| r.0.split(" / ").any(|x| x == k)).expect("a help row");
        assert_eq!(row.1, help_phrase(w));
        assert!(!row.1.is_empty() && row.1 != w, "{w}: a phrase, not the word");
        assert!(!kernel.contains(&k) && !["m", ";", "'"].contains(&k), "{k} collides");
    }
    println!("striatrackgainkeys: help rows={} keys={}", rows.len(), KEYS.len());
}

// ---- STRIALINKSLIDE (LEDGER SR927): the face's slide button carries the link mates (named tail block) ----
#[test]
fn strialinkslide_face_slide_word() {
    let (tl, src) = synth_fixture();
    let tl0 = tl.clone();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select(0, 2).unwrap();
    face.state.select_add(1, 2).unwrap();
    assert!(face.verb("link"));
    face.state.deselect();
    face.state.select(0, 2).unwrap();
    assert!(face.verb("slide-"));
    assert_eq!(face.last_error.take(), None);
    let want = tl0.set_group(&[(0, 2), (1, 2)], 1).unwrap().slide(0, 2, -1).unwrap().slide(1, 2, -1).unwrap();
    assert_eq!(face.state.timeline, want, "picture and sound slid together");
    assert_eq!((face.state.timeline.tracks[0].clips[2].at, face.state.timeline.tracks[1].clips[2].at), (24, 24));
    assert_eq!(face.state.undo_depth(), 2, "link, slide: one step each");
    println!("strialinkslide: linked pair slid to {}", face.state.timeline.tracks[1].clips[2].at);
}

// ---- STRIALINKAUTOFUZZ (LEDGER SR928): the `link-av` word on the face and on a document opened from an EDL (named tail block) ----
#[test]
fn strialinkautofuzz_face_link_av_word() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert_eq!(&VERBS[VERBS.len() - 2..], ["link-av", "help"], "link-av just before help");
    assert!(face.verb("link-av"));
    assert_eq!(face.last_error.take(), None);
    assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (tl.link_av(), 1), "every pair linked, one step");
    println!("strialinkautofuzz: VERBS[{}] link-av", VERBS.len() - 2);
}

#[test]
fn strialinkautofuzz_edl_document_links_by_word() {
    let (tl, _) = synth_fixture();
    let text = edl::write(&tl, "FUZZ").unwrap();
    let back = edl::parse(&text, 25).unwrap();
    assert!(back.tracks.iter().all(|tr| tr.clips.iter().all(|c| c.group == 0)), "an EDL opens with no auto-link");
    let mut st = EditorState::new(back.clone());
    assert_eq!(st.verb("link-av"), Some(Ok(())));
    assert_ne!(st.timeline, back, "the word linked the V1 / A1 pairs");
    assert_eq!((st.timeline.clone(), st.undo_depth()), (back.link_av(), 1), "one commit");
}
// ---- STRIAGAPSALLTRACKS (LEDGER SR929): `close-gaps all` on the host face names gaps and fades over every track (named tail block) ----

/// The synth fixture (V1 and A1 both A@0|B@10|A@25: a gap 20..25 on each) with a 3-tick to-black at 20 on V1: `close-gaps all`
/// with nothing selected closes both gaps, removes the fade (its audio fade to silence goes with it) in ONE commit and names
/// them; a timeline whose A1 would pair onto a zero handle is refused whole and the message names `A1`.
#[test]
fn striagapsalltracks_face_closes_every_track() {
    let (tl, src) = synth_fixture();
    let (a, b) = (tl.tracks[0].clips[0].src, tl.tracks[0].clips[1].src);
    let tl = tl.add_transition_to_black(0, 20, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("close-gaps all"));
    assert_eq!(face.note.take().as_deref(), Some("close-gaps all: closed 2 gaps, removed 1 fade")); // STRIAGAPSTHREE (SR9028): the success line is a face note, not an error
    assert_eq!(face.state.timeline, tl.close_gaps_all().unwrap());
    assert_eq!(face.state.undo_depth(), 1);
    let mut k = tl.clone();
    k.tracks[0].transitions.clear();
    k.tracks[0].clips = vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10)];
    k.tracks[1].clips = vec![Clip::new(a, 40, 50, 0), Clip::new(b, 0, 10, 15)];
    let k = k.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let (_, src) = synth_fixture();
    let mut face = nlefaceexport_face(k.clone(), src);
    assert!(face.verb("close-gaps all"));
    assert_eq!(face.last_error.take(), Some(format!("close-gaps all: A1: {}", EditError::Handle)));
    assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (k, 0));
    println!("striagapsalltracks: V1 {:?} A1 {:?}", face.state.timeline.tracks[0].clips, face.state.timeline.tracks[1].clips);
}

// ---- STRIAPROXYBUTTONS (LEDGER SR930): the proxy's button words, purge and the attached reels (named tail block) ----

/// `proxy-off` / `proxy-all` (the buttons' words) are `proxy off` / `proxy all`; `proxy_reels` follows the attachments.
#[test]
fn striaproxybuttons_words_alias_the_arguments() {
    let (dir, pa, pb, mut face, tl) = striaproxyoff_face("buttons");
    let a = tl.sources[0].id;
    let (xa, xb) = (stria::nle_face::proxy_paths(&pa).0, stria::nle_face::proxy_paths(&pb).0);
    assert_eq!(&VERBS[VERBS.len() - 3..], &["proxy-off", "proxy-all", "help"]);
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy") && face.wait_proxy());
    assert_eq!(face.proxy_reels(), vec!["A".to_string()]);
    assert!(face.verb("proxy-off"));
    assert_eq!(face.last_error.take().as_deref(), Some("proxy: A: off (the preview reads the original)"));
    assert!(!face.has_proxy(a) && face.proxy_reels().is_empty());
    assert!(face.verb("proxy-all"));
    assert_eq!(face.last_error.take(), Some(format!("proxy: all: 2 reels; A: reused {}; B: building (new) -> {}", xa.display(), xb.display())));
    assert!(face.wait_proxy());
    assert_eq!(face.proxy_reels(), vec!["A".to_string(), "B".to_string()]);
    assert!(!face.dirty());
    assert_eq!(face.state.timeline, tl, "view-only");
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxybuttons words: proxy-off detached A; proxy-all reused A, built B");
}

/// `proxy purge` detaches and removes the proxy file and stamp; again: nothing attached, no files. View-only.
#[test]
fn striaproxybuttons_purge_removes_files() {
    let (dir, pa, _, mut face, tl) = striaproxyoff_face("purge");
    let a = tl.sources[0].id;
    let (xa, stamp, work) = stria::nle_face::proxy_paths(&pa);
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy") && face.wait_proxy());
    assert!(face.has_proxy(a) && xa.is_file() && stamp.is_file());
    assert!(face.verb("proxy purge"));
    assert_eq!(face.last_error.take(), Some(format!("proxy: A: off (the preview reads the original); removed {}, {}", xa.display(), stamp.display())));
    assert!(!face.has_proxy(a) && !xa.exists() && !stamp.exists() && !work.exists());
    assert!(pa.is_file(), "the source stays");
    assert!(face.verb("proxy purge"));
    assert_eq!(face.last_error.take().as_deref(), Some("proxy: A: no proxy attached; no proxy files"));
    assert_eq!(face.state.timeline, tl, "view-only");
    assert!(!face.dirty());
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxybuttons purge: A detached, proxy and stamp removed");
}

/// `proxy purge` is refused by name while that source's own job runs (its files untouched); without a path it detaches only.
#[test]
fn striaproxybuttons_purge_refusals() {
    let (dir, pa, _, mut face, _) = striaproxyoff_face("purgebusy");
    let work = stria::nle_face::proxy_paths(&pa).2;
    face.hold_proxy_at(Some(3));
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy") && face.proxying());
    assert!(face.verb("proxy purge"));
    let line = face.last_error.take().unwrap_or_default();
    assert!(line.starts_with("proxy: A: busy (") && line.ends_with("): Esc cancels the build"), "{line}");
    assert!(work.is_dir() && face.proxying(), "the job and its work dir untouched");
    assert!(face.cancel_proxy() && face.wait_proxy());
    let _ = face.last_error.take();
    let (tl, src) = synth_fixture();
    let mut bare = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    assert!(bare.verb("proxy purge"));
    assert_eq!(bare.last_error.take().as_deref(), Some("proxy: no clip selected"));
    bare.state.select(0, 0).unwrap();
    assert!(bare.verb("proxy purge"));
    assert_eq!(bare.last_error.take().as_deref(), Some("proxy: A: no proxy attached; no source path"));
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxybuttons purge refusals: {line}");
}

// ---- STRIAFREEZEMARKS (LEDGER SR931): the export marks ride `freeze-all` through the face (named tail block) ----

/// synth_fixture (end 35, 25 fps), marks 2..20: `freeze-all 1` at tick 5 moves the out mark by the 25-tick hold, so the export
/// range still ends on the same picture (2..45); one undo restores 2..20; plain `freeze 1` leaves the marks.
#[test]
fn striafreezemarks_face_export_range_follows() {
    use stria::nle_face::{EditorState, Face, PixelOrder, synth_fixture};
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    assert_eq!(face.state.set_marks((Some(2), Some(20))), Ok(()));
    face.state.seek(5);
    assert!(face.verb("freeze-all 1"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!((face.marks(), face.export_range()), ((Some(2), Some(45)), Some(2..45)), "the out mark rides the hold");
    assert_eq!(face.state.undo_depth(), 2, "the marks and the hold: the hold is one step");
    assert!(face.state.undo());
    assert_eq!(face.marks(), (Some(2), Some(20)), "one undo restores the marks");
    assert!(face.verb("freeze 1"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!((face.marks(), face.export_range()), ((Some(2), Some(20)), Some(2..20)), "plain freeze leaves the marks");
    println!("striafreezemarks: marks 2..20, freeze-all 1 at 5 -> 2..45, undo -> 2..20, freeze 1 -> 2..20");
}

// ---- STRIAEXPORTQUEUELIST (LEDGER SR932): the export queue read back, and `E` keys the skip (named tail block) ----

fn striaexportqueuelist_name(d: &std::path::Path) -> String {
    d.file_name().expect("a named scratch dir").to_string_lossy().into_owned()
}

#[test]
fn striaexportqueuelist_lists_the_waiting() {
    let (a, b, c) = (nlefaceexport_dir("list-a"), nlefaceexport_dir("list-b"), nlefaceexport_dir("list-c"));
    let mut face = striaexportqueue_paused(&a);
    assert!(face.verb(&format!("export {} png", b.display())));
    assert!(face.verb(&format!("export {} h264", c.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (2 waiting)"));
    let tl = face.state.timeline.clone();
    assert!(face.verb("export-list"));
    let want = format!("export: 2 waiting: {} png, {} h264", striaexportqueuelist_name(&b), striaexportqueuelist_name(&c));
    assert_eq!(face.last_error.take(), Some(want));
    assert_eq!(face.export_queued(), 2, "the list leaves the queue");
    assert!(face.exporting(), "the list leaves the running job");
    assert_eq!(face.state.timeline, tl);
    assert_eq!(face.state.undo_depth(), 0, "a view word pushes no undo step");
    assert!(!face.dirty());
    assert!(face.cancel_export() && face.wait_export());
    assert!(!b.exists() && !c.exists());
    let _ = std::fs::remove_dir_all(&a);
    println!("striaexportqueuelist: two waiting listed");
}

#[test]
fn striaexportqueuelist_nothing_waiting() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("export-list"));
    assert_eq!(face.last_error.take().as_deref(), Some("export: nothing waiting"));
    assert_eq!(face.state.timeline, tl);
    assert_eq!(face.state.undo_depth(), 0);
    assert!(!face.dirty() && !face.exporting());
    println!("striaexportqueuelist: export: nothing waiting");
}

#[test]
fn striaexportqueuelist_is_bounded() {
    let a = nlefaceexport_dir("listmax-a");
    let mut face = striaexportqueue_paused(&a);
    let dirs: Vec<std::path::PathBuf> = (0..5).map(|n| nlefaceexport_dir(&format!("listmax-{n}"))).collect();
    for d in &dirs {
        assert!(face.verb(&format!("export {} png", d.display())));
    }
    assert_eq!(face.export_queued(), 5);
    assert!(face.verb("export-list"));
    let n: Vec<String> = dirs.iter().map(|d| striaexportqueuelist_name(d)).collect();
    assert_eq!(stria::nle_face::EXPORT_LIST_SHOWN, 3);
    assert_eq!(face.last_error.take(), Some(format!("export: 5 waiting: {} png, {} png, {} png, +2 more", n[0], n[1], n[2])));
    assert!(face.cancel_export() && face.wait_export());
    let _ = std::fs::remove_dir_all(&a);
    println!("striaexportqueuelist: five waiting, three named, +2 more");
}

#[test]
fn striaexportqueuelist_key_skips() {
    let rows = help_rows();
    let row = rows.iter().find(|r| r.0.split(" / ").any(|k| k == "E")).expect("the E help row");
    assert_eq!(row.1, help_phrase("export-skip"));
    assert!(KEYS.contains(&("E", "export-skip")));
    let (a, b) = (nlefaceexport_dir("listkey-a"), nlefaceexport_dir("listkey-b"));
    let mut face = striaexportqueue_paused(&a);
    assert!(face.verb(&format!("export {} png", b.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (1 waiting)"));
    assert!(face.verb("help"));
    assert!(face.on(&bandy::SMessage::BrowserText("E".into())));
    assert!(!face.help_shown(), "E while the help shows only dismisses it");
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.export_progress(), Some((3, 35)), "no skip while dismissing");
    assert!(face.on(&bandy::SMessage::BrowserText("E".into())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: skipping (1 waiting)"));
    striaexportqueueskip_drain(&mut face);
    let line = face.last_error.take().expect("B's line");
    assert!(line.starts_with("export: 35 frames, ") && line.ends_with(&format!(" -> {}", b.display())), "{line}");
    assert!(b.join("000034.png").is_file(), "B ran after the E skip");
    for d in [&a, &b] {
        let _ = std::fs::remove_dir_all(d);
    }
    println!("striaexportqueuelist: E skipped A, B ran: \"{line}\"");
}

#[test]
fn striaexportqueuelist_word_is_before_help() {
    assert_eq!(&VERBS[VERBS.len() - 2..], &["export-list", "help"]);
    println!("striaexportqueuelist: VERBS ends {:?}", &VERBS[VERBS.len() - 2..]);
}

// ---- STRIAAUTOSAVEKEEPREAD (LEDGER SR933): recover 1 reads the kept autosave; the keep named (named tail block) ----
// `recover 1` applies `<stem>.autosave.1.edl` through SR883's one recover road (`Face::recover_from`): one commit, the
// same refusals by name; the face does not own `<stem>.autosave.edl` after it. SR913's keep is named once on the
// message row, and never over a message that already stands.

#[test]
fn striaautosavekeepread_recover_1_applies_the_kept_file() {
    let (mut face, doc, old) = striaautosavecap_foreign("keepread");
    let keep = autosave_keep_path(&doc);
    for t in 1..=AUTOSAVE_EDITS as i64 {
        striaautosave_split(&mut face, t);
    }
    assert!(face.autosave_tick(), "the keep note repaints");
    assert_eq!(face.last_error.take().as_deref(), Some("autosave: kept the older autosave as cut.autosave.1.edl"));
    assert_eq!(std::fs::read_to_string(&keep).unwrap(), old);
    let kept = edl::parse(&old, 25).unwrap();
    let depth = face.state.undo_depth();
    assert!(face.verb("recover 1"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, kept, "the kept autosave is applied");
    assert_eq!(face.state.undo_depth(), depth + 1, "one undo step");
    assert!(face.dirty());
    assert_eq!(face.autosaved(), None, "the face does not own <stem>.autosave.edl after recover 1");
    assert!(face.autosave_pending(), "the clock will write the recovered timeline");
    assert!(keep.is_file(), "the kept file stays");
    assert!(face.verb("recover 1"));
    assert_eq!(face.last_error.take().as_deref(), Some("recover: the autosave matches the timeline"));
    assert_eq!(face.state.undo_depth(), depth + 1, "a refused recover pushes nothing");
    assert!(face.verb("undo"));
    assert_ne!(face.state.timeline, kept, "undo returns to the edits before recover 1");
    println!("striaautosavekeepread: recover 1 applied {} in one step", keep.display());
}

#[test]
fn striaautosavekeepread_refusals_by_name() {
    let (tl0, src) = synth_fixture();
    let mut bare = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false);
    assert!(bare.verb("recover 1"));
    assert_eq!(bare.last_error.take().as_deref(), Some("recover: no document path"));
    let (mut face, doc, tl0) = striaautosave_face("keepread-refuse");
    assert!(face.verb("recover 1"));
    assert_eq!(face.last_error.take(), Some(format!("recover: no autosave ({})", autosave_keep_path(&doc).display())));
    for w in ["recover 2", "recover x"] {
        assert!(face.verb(w), "{w} is the recover word");
        assert_eq!(face.last_error.take().as_deref(), Some("recover: one older autosave is kept: recover 1"), "{w}");
    }
    assert!(face.verb("recover"));
    assert_eq!(face.last_error.take(), Some(format!("recover: no autosave ({})", autosave_path(&doc).display())), "bare recover unchanged");
    assert_eq!(face.state.undo_depth(), 0);
    assert_eq!(face.state.timeline, tl0);
}

#[test]
fn striaautosavekeepread_keep_note_never_hides_an_error() {
    let (mut face, doc, old) = striaautosavecap_foreign("keepread-quiet");
    striaautosave_split(&mut face, 15);
    face.last_error = Some("an earlier message".into());
    assert!(face.verb("save"));
    assert_eq!(face.last_error.take().as_deref(), Some("an earlier message"), "the keep note never replaces a standing message");
    assert_eq!(std::fs::read_to_string(autosave_keep_path(&doc)).unwrap(), old, "kept all the same");
}

// ---- STRIAUNDOJUMP (LEDGER SR934): `undo <n>` / `redo <n>` name their landing; `h` keys the history (named tail block) ----

/// SR912's three named commits (split, delete, slip+) on the export face: (the face, the post-split timeline, the
/// post-slip+ timeline).
fn striaundojump_face() -> (Face, Timeline, Timeline) {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select(0, 1).unwrap();
    face.state.seek(15);
    assert!(face.verb("split"));
    let split = face.state.timeline.clone();
    assert!(face.verb("delete"));
    face.state.select(0, 0).unwrap();
    assert!(face.verb("slip+"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.undo_depth(), 3);
    let top = face.state.timeline.clone();
    (face, split, top)
}

/// `undo 2` steps two entries and names the oldest it undid; each step stays its own entry so `redo 2` returns exactly
/// and names the newest it redid; `undo 3` lands back on the opened timeline (clean).
#[test]
fn striaundojump_steps_and_returns() {
    let (mut face, split, top) = striaundojump_face();
    assert!(face.verb("undo 2"));
    assert_eq!(face.last_error.take().as_deref(), Some("undo 2: back to before \"delete\""));
    assert_eq!((face.state.undo_depth(), face.state.redo_depth()), (1, 2));
    assert_eq!(face.state.timeline, split);
    assert!(face.verb("history"));
    assert_eq!(face.last_error.take().as_deref(), Some("history: 1 split"));
    assert!(face.verb("redo 2"));
    assert_eq!(face.last_error.take().as_deref(), Some("redo 2: forward to after \"slip+\""));
    assert_eq!((face.state.undo_depth(), face.state.redo_depth()), (3, 0));
    assert_eq!(face.state.timeline, top);
    assert!(face.verb("undo 3"));
    let line = face.last_error.take();
    assert_eq!(line.as_deref(), Some("undo 3: back to before \"split\""));
    assert_eq!((face.state.undo_depth(), face.state.redo_depth()), (0, 3));
    assert!(!face.dirty(), "undo 3 lands on the opened timeline");
    println!("striaundojump: {line:?}");
}

/// A count of 0, past the depth or not a number is refused by name; an empty stack says so; nothing moves. Plain `undo`
/// still puts nothing on the message line.
#[test]
fn striaundojump_refusals() {
    let (mut face, _split, top) = striaundojump_face();
    let dirty = face.dirty();
    for (v, want) in [
        ("undo 4", "undo: \"4\" is not a count 1..=3"),
        ("undo 0", "undo: \"0\" is not a count 1..=3"),
        ("undo x", "undo: \"x\" is not a count 1..=3"),
        ("redo 1", "redo: nothing to redo"),
    ] {
        assert!(face.verb(v), "{v}");
        assert_eq!(face.last_error.take().as_deref(), Some(want), "{v}");
        assert_eq!((face.state.timeline == top, face.state.undo_depth(), face.state.redo_depth(), face.dirty()), (true, 3, 0, dirty), "{v}");
    }
    assert!(face.verb("undo"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.verb("redo 2"));
    assert_eq!(face.last_error.take().as_deref(), Some("redo: \"2\" is not a count 1..=1"));
}

/// `h` (text) is the history key: a KEYS row onto SR876's `history` word with its HELP phrase; it lists the steps and
/// commits nothing. A one-character BrowserKey alone is not taken.
#[test]
fn striaundojump_history_key() {
    assert!(KEYS.contains(&("h", "history")));
    assert_eq!(help_phrase("history"), "list the last undo steps");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select(0, 1).unwrap();
    face.state.seek(15);
    assert!(face.verb("split"));
    let (tl1, dirty) = (face.state.timeline.clone(), face.dirty());
    assert!(!face.on(&bandy::SMessage::BrowserKey("h".into())));
    assert!(face.on(&bandy::SMessage::BrowserText("h".into())));
    assert_eq!(face.last_error.take().as_deref(), Some("history: 1 split"));
    assert_eq!((face.state.timeline == tl1, face.state.undo_depth(), face.dirty()), (true, 1, dirty));
}

// ---- STRIAPERFDRAGSHOWN (LEDGER SR936): the drag preview kept across paints (named tail block) ----

/// Rule: a live drag replays its preview (`Drag::shown`) once per pointer position: three paints after one move are one
/// replay, a second move is the second, and after the release a paint replays nothing.
#[test]
fn striaperfdragshown_paints_replay_once_until_the_pointer_moves() {
    let (mut face, _twin, _tl) = nle3eslipface_pair();
    let (x, y) = face.layout().point(0, 15);
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_CONTROL));
    face.on(&bandy::SMessage::BrowserMouseMove(x + 40.0, y, 0));
    assert_eq!(face.drag_replays(), 0, "no paint yet");
    for _ in 0..3 {
        let _ = face.paint();
    }
    assert_eq!(face.drag_replays(), 1, "three paints, one replay");
    face.on(&bandy::SMessage::BrowserMouseMove(x + 48.0, y, 0));
    let _ = face.paint();
    assert_eq!(face.drag_replays(), 2, "the pointer moved");
    face.on(&nle3eslipdrag_btn(false, x + 48.0, y, bandy::MOD_CONTROL));
    assert!(face.dragging().is_none());
    let _ = face.paint();
    assert_eq!(face.drag_replays(), 2, "no drag, no replay");
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("striaperfdragshown: 3 paints after one move = 1 replay, 2 moves = 2");
}

/// Rule: the kept preview paints the same bytes as a fresh replay: a miss, a hit, and a paint after the reset agree.
#[test]
fn striaperfdragshown_cached_paint_is_byte_identical() {
    let (mut face, _twin, _tl) = nle3eslipface_pair();
    let (x, y) = face.layout().point(0, 15);
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_CONTROL));
    face.on(&bandy::SMessage::BrowserMouseMove(x + 40.0, y, 0));
    assert!(face.dragging().and_then(|d| d.shown(&face.state)).is_some(), "the slip previews");
    let p1 = face.paint();
    let p2 = face.paint();
    face.drag_shown_reset();
    let p3 = face.paint();
    assert_eq!(face.drag_replays(), 2, "the miss and the replay after the reset");
    assert!(p1 == p2, "the hit paints the miss's bytes");
    assert!(p2 == p3, "the hit paints a fresh replay's bytes");
    println!("striaperfdragshown: {} bytes, miss == hit == fresh", p1.len());
}

/// Rule: a split face's repaint (Preview then Tracks) pays ONE replay of the live drag, not one per Surface.
#[test]
fn striaperfdragshown_split_pays_one_replay_per_repaint() {
    let (tl, src) = synth_fixture();
    let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).on_surfaces(0, 1);
    f.state.select(0, 1).unwrap();
    f.on(&bandy::SMessage::BrowserButton { down: true, x: 81.0, y: 36.0, mods: 0, surface: 1 });
    f.on(&bandy::SMessage::BrowserMouseMove(105.0, 36.0, 1));
    assert!(f.dragging().is_some(), "the press on the tracks Surface began a drag");
    assert_eq!(f.frames().len(), 2);
    assert_eq!(f.drag_replays(), 1, "two Surfaces, one replay");
    let _ = f.frames();
    assert_eq!(f.drag_replays(), 1, "a repaint with the pointer still: none");
    println!("striaperfdragshown: split repaint x2 = 1 replay");
}

// ---- STRIAWAVVALIDBITS (LEDGER SR937): EXTENSIBLE valid bits below the container and RF64 / BW64 reels (named tail block) ----

/// STRIAWAVVALIDBITS (SR937): a mono 48 kHz RIFF/WAVE file whose 40-byte `fmt ` is WAVE_FORMAT_EXTENSIBLE at container
/// `bits` with `valid` valid bits, SubFormat KSDATAFORMAT_SUBTYPE_PCM.
fn striawavvalidbits_ext(bits: u16, valid: u16, payload: &[u8]) -> Vec<u8> {
    let align = bits / 8;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(4 + 8 + 40 + 8 + payload.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&40u32.to_le_bytes());
    w.extend_from_slice(&0xFFFEu16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&48_000u32.to_le_bytes());
    w.extend_from_slice(&(48_000 * align as u32).to_le_bytes());
    w.extend_from_slice(&align.to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(&22u16.to_le_bytes());
    w.extend_from_slice(&valid.to_le_bytes());
    w.extend_from_slice(&4u32.to_le_bytes()); // front centre
    w.extend_from_slice(&[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    w.extend_from_slice(payload);
    w
}

/// STRIAWAVVALIDBITS (SR937): a mono 48 kHz PCM16 `magic` (RF64 / BW64) file: a `ds64` chunk first whose dataSize is
/// `ds64_size` (None: no ds64 chunk at all), the `data` size field 0xFFFFFFFF, then `trailer` after the samples.
fn striawavvalidbits_rf64(magic: &[u8; 4], ds64_size: Option<u64>, payload: &[u8], trailer: &[u8]) -> Vec<u8> {
    let mut w = Vec::new();
    w.extend_from_slice(magic);
    w.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    w.extend_from_slice(b"WAVE");
    if let Some(n) = ds64_size {
        w.extend_from_slice(b"ds64");
        w.extend_from_slice(&28u32.to_le_bytes());
        w.extend_from_slice(&(4 + 36 + 24 + 8 + payload.len() as u64 + trailer.len() as u64).to_le_bytes());
        w.extend_from_slice(&n.to_le_bytes());
        w.extend_from_slice(&(n / 2).to_le_bytes());
        w.extend_from_slice(&0u32.to_le_bytes());
    }
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&48_000u32.to_le_bytes());
    w.extend_from_slice(&96_000u32.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    w.extend_from_slice(payload);
    w.extend_from_slice(trailer);
    w
}

#[test]
fn striawavvalidbits_extensible_below_container_renders() {
    // 20 valid bits in a 24-bit container and 24 in 32, left-justified: decoded at the container width.
    for (bits, valid) in [(24u16, 20u16), (32, 24)] {
        let wav = striawavvalidbits_ext(bits, valid, &striaimportwavwide_payload(bits, false, 96_000));
        let d = gneiss_pal::dsp::demux::Demuxer::open_wav(wav.clone()).expect("open_wav");
        assert_eq!(d.tracks()[0].codec, gneiss_pal::dsp::demux::Codec::Pcm { bits, float: false, big_endian: false });
        let (src, s) = striaimportwav_render(wav, (10, 30), 20);
        assert_eq!(timeline_core::SourceIndex::duration(&src, SourceId(0)), Some(50));
        assert_eq!((s.rate, s.ch), (48_000, 1));
        assert_eq!(s.pcm.len(), 20 * 1920);
        let skip = 10 * 1920;
        for (j, got) in s.pcm.iter().enumerate() {
            assert_eq!(*got, q(striaimportwav_sample(skip + j) as i32), "{valid} in {bits} sample {j}");
        }
    }
}

#[test]
fn striawavvalidbits_rf64_takes_the_ds64_size() {
    let payload = striaimportwavwide_payload(16, false, 96_000);
    let trailer = b"LIST\x04\x00\x00\x00INFO"; // a chunk after the samples: never read as audio
    for magic in [b"RF64", b"BW64"] {
        let wav = striawavvalidbits_rf64(magic, Some(payload.len() as u64), &payload, trailer);
        assert!(gneiss_pal::dsp::demux::wav::is_wav(&wav));
        // The Player's road is kept: probe and Demuxer::open stay WAV-blind.
        assert_eq!(gneiss_pal::dsp::demux::probe(&wav), None);
        assert!(gneiss_pal::dsp::demux::Demuxer::open(wav.clone()).is_err());
        let d = gneiss_pal::dsp::demux::Demuxer::open_wav(wav.clone()).expect("open_wav");
        assert_eq!(d.samples().iter().map(|s| s.duration).sum::<u64>(), 96_000, "{magic:?}");
        let (tl, _) = Sources::from_media("W", &wav, 25).unwrap().expect("an RF64 is media");
        assert_eq!(tl.tracks.len(), 1);
        assert_eq!((tl.tracks[0].clips[0].src_in, tl.tracks[0].clips[0].src_out), (0, 50));
        let (src, s) = striaimportwav_render(wav, (0, 50), 50);
        assert_eq!(timeline_core::SourceIndex::duration(&src, SourceId(0)), Some(50));
        assert_eq!(s.pcm.len(), 96_000);
        for (j, got) in s.pcm.iter().enumerate() {
            assert_eq!(*got, q(striaimportwav_sample(j) as i32), "{magic:?} sample {j}");
        }
    }
}

#[test]
fn striawavvalidbits_refusals_by_name() {
    let refusal = |bytes: Vec<u8>| {
        let tl = Timeline::new(25);
        let mut src = Sources::new(&tl);
        src.add(SourceId(0), bytes).expect_err("refused").to_string()
    };
    // More valid bits than the container holds.
    let e = refusal(striawavvalidbits_ext(24, 28, &striaimportwavwide_payload(24, false, 2400)));
    assert!(e.contains("wav: sample layout (8/16/24/32-bit integer or 32/64-bit float)"), "{e}");
    let payload = striaimportwavwide_payload(16, false, 2400);
    // An RF64 whose data size field defers to a ds64 chunk it does not carry.
    let e = refusal(striawavvalidbits_rf64(b"RF64", None, &payload, &[]));
    assert!(e.contains("wav: RF64 without a ds64 chunk"), "{e}");
    // A ds64 data size past the end of the file.
    let e = refusal(striawavvalidbits_rf64(b"BW64", Some(2 * payload.len() as u64), &payload, &[]));
    assert!(e.contains("wav: data chunk runs past the end of the file"), "{e}");
}

// ---- STRIAXFORMPRESETS (LEDGER SR938): the face's corner presets, scale steps and the placement line (named tail block) ----

/// `pip tr` and `scale-` / `scale+` (VERBS words just before `help`) reach timeline_core's ONE verb dispatch, one undo step
/// each; the message row then reads the placement (`pip: x 725 y 25 scale 250`) with no error, until the next message the
/// face takes; a step off the frame is refused by name on the message row with no undo step.
#[test]
fn striaxformpresets_face_presets_and_placement_line() {
    use stria::nle_face::{EditorState, Face, PixelOrder, SURFACE, VERBS, synth_fixture};
    assert_eq!(&VERBS[VERBS.len() - 3..], &["scale-", "scale+", "help"]);
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("scale+"));
    assert_eq!(face.last_error.take(), Some(format!("scale+: {}", EditError::NoClip)));
    face.state.select(0, 1).unwrap();
    assert!(face.verb("pip tr"));
    assert_eq!(face.last_error.take(), None);
    assert_eq!(face.message_text().as_deref(), Some("pip: x 725 y 25 scale 250"));
    assert_eq!(face.state.timeline, tl0.set_xform(0, 1, timeline_core::Xform { x: 725, y: 25, s: 250 }).unwrap());
    assert!(face.verb("scale-"));
    assert_eq!(face.message_text().as_deref(), Some("pip: x 750 y 50 scale 200"));
    assert_eq!(face.state.undo_depth(), 2);
    face.on(&bandy::SMessage::Input { target: SURFACE.into(), text: "zoom+".into() });
    assert_ne!(face.message_text().as_deref(), Some("pip: x 750 y 50 scale 200"), "the next message clears the line");
    assert!(face.verb("pip off"));
    assert_eq!(face.message_text().as_deref(), Some("pip: x 0 y 0 scale 1000"));
    assert!(face.verb("scale+"));
    assert_eq!(face.last_error.take(), Some(format!("scale+: {}", EditError::Xform)));
    assert_eq!(face.state.undo_depth(), 3);
}

// ---- STRIASEQINFO (LEDGER SR940): a sequence reel's line and its kept frame (named tail block) ----
// `Sources::summary` names a sequence reel `seq <first>..=<last> <w>x<h>` (the face's reel line, stria-view's `"reel"`), and a
// sequence reel keeps its last decoded frame (one per reel, keyed by file number) so a repaint of the same tick reads no file.

#[test]
fn striaseqinfo_summary_names_the_sequence() {
    let pat = striaimportimagesequence_dir("seqinfo-summary", 16, 8, &[(7, [1, 2, 3]), (8, [4, 5, 6]), (9, [7, 8, 9])]);
    let (tl, s) = striaimportimagesequence_tl(3);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    assert_eq!(src.summary(s).as_deref(), Some("seq 7..=9 16x8"));
    assert_eq!(src.summary(SourceId(7)), None, "an unknown id says nothing");
    println!("striaseqinfo: summary=\"seq 7..=9 16x8\"");
}

#[test]
fn striaseqinfo_face_reel_line_names_it() {
    use stria::nle_face::{EditorState, Face, PixelOrder};
    let pat = striaimportimagesequence_dir("seqinfo-face", 16, 8, &[(7, [1, 2, 3]), (8, [4, 5, 6]), (9, [7, 8, 9])]);
    let (tl, s) = striaimportimagesequence_tl(3);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    let mut face = Face::new(EditorState::new(tl), src, (320, 144), PixelOrder::Rgba, false);
    assert_eq!(face.reel_line(), None, "nothing selected");
    face.state.select(0, 0).unwrap();
    assert_eq!(face.reel_line().as_deref(), Some("SEQ: seq 7..=9 16x8"));
    println!("striaseqinfo: face reel_line=\"SEQ: seq 7..=9 16x8\"");
}

#[test]
fn striaseqinfo_repaint_reads_no_file() {
    let cols = [[200, 10, 10], [10, 200, 10], [10, 10, 200]];
    let pat = striaimportimagesequence_dir("seqinfo-repaint", 4, 4, &[(7, cols[0]), (8, cols[1]), (9, cols[2])]);
    let (tl, s) = striaimportimagesequence_tl(3);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    let mut first = StriaTitleFrames::default();
    render(&tl, 1..2, &src, &mut first).unwrap();
    assert!(first.0[0].3 == striaimportimagesequence_px(4, 4, cols[1]), "tick 1 is file 8");
    use std::os::unix::fs::PermissionsExt; // STRIASEQTHREE (SR9022): a kept frame is served while its file's stamp holds,
    let f8 = pat.replace("%04d", "0008"); // so the no-read leg makes file 8 unreadable (stamp unchanged) rather than removing it
    std::fs::set_permissions(&f8, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&f8).is_err() {
        let mut again = StriaTitleFrames::default();
        render(&tl, 1..2, &src, &mut again).expect("the kept frame: file 8 is not read again");
        assert!(again.0 == first.0, "the repaint is the same frame");
    } else {
        println!("striaseqinfo: SKIP the no-read leg: a 0o000 file reads here (root); the striaseqthree_kept unit tests carry it");
    }
    std::fs::set_permissions(&f8, std::fs::Permissions::from_mode(0o644)).unwrap();
    render(&tl, 2..3, &src, &mut StriaTitleFrames::default()).unwrap();
    std::fs::remove_file(&f8).unwrap();
    let e = render(&tl, 1..2, &src, &mut StriaTitleFrames::default()).unwrap_err().to_string();
    assert!(e.contains("frame 8 missing"), "a kept frame of a file gone from disk is not served: {e}");
    println!("striaseqinfo: repaint of tick 1 with file 8 unreadable -> kept frame; after removing file 8 -> {e}");
}

#[test]
fn striaseqinfo_readd_forgets_kept_frame() {
    let pat = striaimportimagesequence_dir("seqinfo-readd", 4, 4, &[(7, [1, 1, 1])]);
    let (tl, s) = striaimportimagesequence_tl(1);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    render(&tl, 0..1, &src, &mut StriaTitleFrames::default()).unwrap();
    let png = gneiss_pal::dsp::image::encode_rgba(4, 4, &striaimportimagesequence_px(4, 4, [90, 90, 90])).unwrap();
    std::fs::write(pat.replace("%04d", "0007"), png).unwrap();
    src.add_sequence(s, &pat).unwrap();
    let mut got = StriaTitleFrames::default();
    render(&tl, 0..1, &src, &mut got).unwrap();
    assert!(got.0[0].3 == striaimportimagesequence_px(4, 4, [90, 90, 90]), "the re-added reel reads the rewritten file");
}

// ---- STRIAHELPSEARCHLIVE (LEDGER SR941): the help filter typed live, matching the action word (named tail block) ----

fn striahelpsearchlive_type(f: &mut Face, s: &str) {
    for c in s.chars() {
        assert!(f.on(&bandy::SMessage::BrowserText(c.to_string())), "{c:?} taken");
    }
}

fn striahelpsearchlive_backspace(f: &mut Face) -> bool {
    f.on(&bandy::SMessage::BrowserKey("BackSpace".into()))
}

/// While shown, typed characters narrow the help (never applied: `l` does not shuttle, `i` marks nothing); Backspace
/// widens it back to the whole table; Backspace with no query is taken and ignored, the help still shown.
#[test]
fn striahelpsearchlive_typing_narrows_backspace_widens() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    let (ph, tl) = (f.state.playhead, f.state.timeline.clone());
    assert!(f.on(&bandy::SMessage::BrowserText("?".into())));
    assert_eq!((f.help_shown(), f.help_query()), (true, None));
    striahelpsearchlive_type(&mut f, "sli");
    assert_eq!((f.help_shown(), f.help_query()), (true, Some("sli")));
    assert_eq!(f.help_rows_shown(), stria::nle_face::help_matching("sli"));
    assert!(f.help_rows_shown().len() < help_rows().len());
    assert_eq!((f.playing, f.rate, f.state.playhead, &f.state.timeline), (false, 1, ph, &tl));
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    assert!(striahelpsearchlive_backspace(&mut f));
    assert_eq!(f.help_query(), Some("sl"));
    assert!(striahelpsearchlive_backspace(&mut f));
    assert!(striahelpsearchlive_backspace(&mut f));
    assert_eq!((f.help_shown(), f.help_query()), (true, None));
    assert_eq!(f.help_rows_shown(), help_rows());
    assert!(!striahelpsearchlive_backspace(&mut f), "no query: taken, nothing to repaint");
    assert_eq!((f.help_shown(), f.help_query()), (true, None));
    println!("striahelpsearchlive: sli -> {} rows, back to {}", stria::nle_face::help_matching("sli").len(), help_rows().len());
}

/// A character that would leave no row is refused by name on the message line and not added.
#[test]
fn striahelpsearchlive_no_match_keeps_the_filter() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    assert!(f.verb("help"));
    striahelpsearchlive_type(&mut f, "slip");
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    assert!(f.on(&bandy::SMessage::BrowserText("z".into())));
    assert_eq!(f.last_error.take().as_deref(), Some("help: no key matches \"slipz\""));
    assert_eq!((f.help_shown(), f.help_query()), (true, Some("slip")));
    println!("striahelpsearchlive: slipz refused, slip kept");
}

/// Escape and `?` still close a typed help; reopening by `?` shows the whole table.
#[test]
fn striahelpsearchlive_escape_and_question_close() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    assert!(f.verb("help"));
    striahelpsearchlive_type(&mut f, "zo");
    assert!(f.on(&bandy::SMessage::BrowserKey("Escape".into())));
    assert!(!f.help_shown());
    assert!(f.on(&bandy::SMessage::BrowserText("?".into())));
    assert_eq!((f.help_shown(), f.help_query()), (true, None));
    striahelpsearchlive_type(&mut f, "zo");
    assert!(f.on(&bandy::SMessage::BrowserText("?".into())));
    assert!(!f.help_shown());
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    println!("striahelpsearchlive: Escape and ? close");
}

/// The action word matches too: `help nudge` (neither key nor phrase says nudge) shows the two nudge rows; typed `slip-`
/// shows the slip- and slip-10 rows; HELP's phrases are distinct (the inverse the action match reads).
#[test]
fn striahelpsearchlive_matches_the_action_word() {
    for (i, (_, a)) in HELP.iter().enumerate() {
        assert!(HELP[i + 1..].iter().all(|(_, b)| b != a), "phrase {a:?} twice");
    }
    let mut f = nle5dkeyshelp_face(640, 480, false);
    assert!(f.verb("help nudge"));
    let rows = f.help_rows_shown();
    assert_eq!(rows.iter().map(|r| r.1).collect::<Vec<_>>(), vec![help_phrase("nudge-"), help_phrase("nudge+")]);
    assert!(f.verb("help"));
    assert!(!f.help_shown());
    assert!(f.verb("help"));
    striahelpsearchlive_type(&mut f, "slip-");
    assert_eq!(f.help_query(), Some("slip-"));
    assert_eq!(f.help_rows_shown().iter().map(|r| r.1).collect::<Vec<_>>(), vec![help_phrase("slip-"), help_phrase("slip-10")]);
    println!("striahelpsearchlive: nudge -> {} rows, slip- -> 2 rows", rows.len());
}

/// View-only: typing and Backspace push no undo step and leave the document clean.
#[test]
fn striahelpsearchlive_view_only() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    let (tl, ph, depth) = (f.state.timeline.clone(), f.state.playhead, f.state.undo_depth());
    assert!(f.verb("help"));
    striahelpsearchlive_type(&mut f, "del");
    assert!(striahelpsearchlive_backspace(&mut f));
    assert_eq!((&f.state.timeline, f.state.playhead, f.state.undo_depth(), f.dirty()), (&tl, ph, depth, false));
    println!("striahelpsearchlive: undo depth {depth} kept, clean");
}

/// A typed `slip` paints exactly the `help slip` frame on a labelled face; labels off stays byte-identical while typing.
#[test]
fn striahelpsearchlive_paint_matches_the_word_form() {
    let mut word = nle5dkeyshelp_face(640, 480, true);
    assert!(word.verb("help slip"));
    let want = word.paint();
    let mut typed = nle5dkeyshelp_face(640, 480, true);
    assert!(typed.verb("help"));
    striahelpsearchlive_type(&mut typed, "slip");
    assert_eq!(typed.paint(), want);
    let mut plain = nle5dkeyshelp_face(640, 480, false);
    let a = plain.paint();
    assert!(plain.verb("help"));
    striahelpsearchlive_type(&mut plain, "slip");
    assert_eq!(plain.paint(), a, "labels off: no overlay");
    println!("striahelpsearchlive: typed slip fnv {:016x}", stria::nle_face::fnv1a(&want));
}

// ---- STRIASOURCEINFOKEY (LEDGER SR942): Ctrl / Cmd+I, and the info line's seconds in wall time (named tail block) ----
/// Rule: `Ctrl+i` and `Meta+i` (BrowserKey) each stand the selected clip's reel line through the one `info` word: no undo
/// step, not dirty; a bare `i` (text) is still `mark-in` and stands no info line.
#[test]
fn striasourceinfokey_ctrl_i_and_meta_i_stand_the_line() {
    for chord in ["Ctrl+i", "Meta+i"] {
        let mut face = striasourceinfo_face();
        let (undo, tl) = (face.state.undo_depth(), face.state.timeline.clone());
        assert!(face.on(&bandy::SMessage::BrowserKey(chord.to_string())), "{chord}");
        assert!(face.last_error.is_none(), "{chord}: {:?}", face.last_error);
        assert!(face.info_pending(), "{chord}");
        assert!(face.message_text().is_some_and(|m| m.starts_with("info: B: ")), "{chord}: {:?}", face.message_text());
        assert_eq!((face.state.undo_depth(), face.dirty(), face.state.timeline == tl), (undo, false, true), "{chord}");
    }
    let mut face = striasourceinfo_face();
    let _ = face.on(&bandy::SMessage::BrowserText("i".to_string()));
    let _ = face.last_error.take();
    assert!(!face.info_pending(), "a bare i is mark-in, not info");
    println!("striasourceinfokey: Ctrl+i / Meta+i stand the info line; i stays mark-in");
}

/// Rule: the help names the chords: the row whose phrase is `help_phrase("info")` lists `Ctrl+i` and `Meta+i`.
#[test]
fn striasourceinfokey_help_names_the_chords() {
    assert!(KEYS.contains(&("Ctrl+i", "info")) && KEYS.contains(&("Meta+i", "info")));
    let phrase = help_phrase("info");
    assert!(!phrase.is_empty() && phrase != "info", "a phrase, not the word");
    let rows = help_rows();
    let row = rows.iter().find(|r| r.1 == phrase).expect("the info row");
    assert!(row.0.split(" / ").any(|k| k == "Ctrl+i") && row.0.split(" / ").any(|k| k == "Meta+i"), "{:?}", row.0);
    println!("striasourceinfokey: help row {row:?}");
}

/// Rule: the line ends at INFO_SECS of wall time even with face ticks left (the ticks `serve` drops under Skip no longer
/// make it outstay): at +4 s it stands, at +5 s ONE tick ends it and asks a repaint.
#[test]
fn striasourceinfokey_wall_deadline_ends_it() {
    let mut face = striasourceinfo_face();
    assert!(face.verb("info"));
    let t0 = std::time::Instant::now();
    assert!(!face.info_tick_at(t0 + std::time::Duration::from_secs(4)));
    assert!(face.info_pending());
    assert!(face.message_text().is_some_and(|m| m.starts_with("info: B: ")), "{:?}", face.message_text());
    assert!(face.info_tick_at(t0 + std::time::Duration::from_secs(stria::nle_face::INFO_SECS as u64)), "the deadline tick repaints");
    assert!(!face.info_pending());
    assert_eq!(face.info_text(), None);
    println!("striasourceinfokey: standing at +4 s, ended at +{} s after 2 of {} ticks", stria::nle_face::INFO_SECS, stria::nle_face::INFO_SECS * 25);
}

// ---- STRIASEQREPORT (LEDGER SR943): a sequence reel's open line and its own rate (named tail block) ----
// `Sources::open_report` answers `open=seq` for a sequence reel (so tools/nle `schedule` prints its `reel=` line with the
// summary), and `--src REEL=<dir>/frame_%04d.png@<R>` reads the sequence at R frames a second by exact integer arithmetic on the
// timeline's tick (a file held for several ticks, or files skipped); no `@` stays one file per tick.

#[test]
fn striaseqreport_open_report_names_the_sequence() {
    let pat = striaimportimagesequence_dir("seqreport-open", 16, 8, &[(7, [1, 2, 3]), (8, [4, 5, 6]), (9, [7, 8, 9])]);
    let (tl, s) = striaimportimagesequence_tl(3);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    let r = src.open_report(s).expect("a sequence reel reports");
    assert_eq!(r.to_string(), "open=seq");
    assert_eq!((r.seq, r.colour), (true, None));
    assert!(src.open_report(SourceId(7)).is_none(), "an unknown id says nothing");
    println!("striaseqreport: reel=SEQ {r} summary=\"{}\"", src.summary(s).unwrap());
}

#[test]
fn striaseqreport_rate_holds_each_file() {
    let cols = [[200, 10, 10], [10, 200, 10], [10, 10, 200]];
    let pat = striaimportimagesequence_dir("seqreport-hold", 4, 4, &[(7, cols[0]), (8, cols[1]), (9, cols[2])]);
    let (tl, s) = striaimportimagesequence_tl(7);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &format!("{pat}@12")).unwrap();
    assert_eq!(timeline_core::SourceIndex::duration(&src, s), Some(7), "3 files at 12/s on a 25/s timeline: ceil(75/12)");
    assert_eq!(src.summary(s).as_deref(), Some("seq 7..=9 4x4 @12"));
    let mut sink = StriaTitleFrames::default();
    render(&tl, 0..7, &src, &mut sink).unwrap();
    assert_eq!(sink.0.len(), 7);
    for (t, file) in [0usize, 0, 0, 1, 1, 2, 2].into_iter().enumerate() {
        assert!(sink.0[t].3 == striaimportimagesequence_px(4, 4, cols[file]), "tick {t}: file {}", 7 + file);
    }
    let (long, s2) = striaimportimagesequence_tl(8);
    let mut past = Sources::new(&long);
    past.add_sequence(s2, &format!("{pat}@12")).unwrap();
    assert!(render(&long, 7..8, &past, &mut StriaTitleFrames::default()).is_err(), "tick 7 is file 10: past the last");
    println!("striaseqreport: frame_0007..0009.png@12 -> 7 ticks at 25/s, files 7,7,7,8,8,9,9");
}

#[test]
fn striaseqreport_rate_skips_at_a_slower_timeline() {
    let cols: Vec<[u8; 3]> = (0..6u8).map(|i| [i * 40, 10, 10]).collect();
    let frames: Vec<(u64, [u8; 3])> = cols.iter().enumerate().map(|(i, c)| (i as u64, *c)).collect();
    let pat = striaimportimagesequence_dir("seqreport-skip", 4, 4, &frames);
    let (tl, s) = striaimportimagesequence_tl(3);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &format!("{pat}@50")).unwrap();
    assert_eq!(timeline_core::SourceIndex::duration(&src, s), Some(3), "6 files at 50/s on a 25/s timeline");
    let mut sink = StriaTitleFrames::default();
    render(&tl, 0..3, &src, &mut sink).unwrap();
    for (t, file) in [0usize, 2, 4].into_iter().enumerate() {
        assert!(sink.0[t].3 == striaimportimagesequence_px(4, 4, cols[file]), "tick {t}: file {file}");
    }
}

#[test]
fn striaseqreport_rate_refused_by_name() {
    let pat = striaimportimagesequence_dir("seqreport-refuse", 4, 4, &[(1, [9, 9, 9])]);
    let (tl, s) = striaimportimagesequence_tl(1);
    let mut src = Sources::new(&tl);
    for r in ["0", "", "x", "-3", "12.5.1", "+3"] { // STRIASEQMORE (SR978): 12.5 is a rate now
        let p = format!("{pat}@{r}");
        let e = src.add_sequence(s, &p).unwrap_err().to_string();
        assert!(e.contains(&format!("image sequence {p}: rate \"{r}\"")), "{e}");
    }
    assert!(src.add_sequence(s, &pat).is_ok(), "the plain pattern still opens");
}

#[test]
fn striaseqreport_default_is_one_file_per_tick() {
    let pat = striaimportimagesequence_dir("seqreport-default", 16, 8, &[(7, [1, 2, 3]), (8, [4, 5, 6]), (9, [7, 8, 9])]);
    let (tl, s) = striaimportimagesequence_tl(3);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    assert_eq!(timeline_core::SourceIndex::duration(&src, s), Some(3));
    assert_eq!(src.summary(s).as_deref(), Some("seq 7..=9 16x8"));
}

// ---- STRIANUDGEMORE (LEDGER SR944): a free `nudge <d>` rides the face's nudge fn and its message line (named tail block) ----

/// `Face::verb("nudge 5")` moves the selected clip five frames as ONE commit and the message line reads its new `at`, as
/// the KEYS words do; a refusal reads `nudge: <why>` with the state kept; SR855's `nudge+7` is still not a face word.
#[test]
fn strianudgemore_free_count_reads_the_at() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    let tl0 = f.state.timeline.clone();
    f.state.select(0, 2).unwrap();
    assert!(f.verb("nudge 5"));
    assert_eq!(f.state.timeline, tl0.move_many(&[(0, 2)], 5).unwrap());
    assert_eq!(f.state.undo_depth(), 1);
    assert_eq!(f.last_error.take().as_deref(), Some("nudge: at 30 (00:00:01:05)"));
    let tl1 = f.state.timeline.clone();
    assert!(f.verb("nudge -1000"));
    assert_eq!(f.last_error.take().as_deref(), Some("nudge: negative time"));
    assert_eq!((&f.state.timeline, f.state.undo_depth()), (&tl1, 1));
    assert!(!f.verb("nudge+7"), "nudge+7 is not a face word");
    assert_eq!(f.state.timeline, tl1);
    println!("strianudgemore: nudge 5 25 -> 30 in one commit, nudge -1000 refused by name");
}

// ---- STRIAEXPORTPRESETSMORE (LEDGER SR948): the face's picked export preset, the `preset` word and its bare export (named tail block) ----

#[test]
fn striaexportpresetsmore_word_steps_the_table_as_a_note() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(VERBS.contains(&"preset"));
    assert_eq!(VERBS.last(), Some(&"help"));
    assert_eq!(face.export_preset(), None);
    for want in ["web: h264 + aac crf=23 aq=10 scenecut=40", "archive: png + wav", "proxy: h264 + aac:64 crf=32 scale=1/2"] { // STRIAEXPORTPRESETSNEXT (SR989): proxy names its scale
        assert!(face.verb("preset"));
        assert_eq!(face.last_error, None, "a pick is a note, not an error");
        assert_eq!(face.message_text(), Some(format!("preset {want}")));
        assert_eq!(face.export_preset().map(|p| p.words()), Some(want.to_string()));
    }
    assert!(face.verb("preset"));
    assert_eq!((face.export_preset(), face.message_text()), (None, Some("preset none: export's own words".to_string())));
    assert!(face.verb("preset archive"));
    assert_eq!(face.export_preset().map(|p| p.name), Some("archive"));
    assert!(face.verb("preset none"));
    assert_eq!(face.export_preset(), None);
    assert_eq!(face.state.timeline, tl, "a view-only word leaves the document");
    assert!(!face.dirty());
    // The next taken input clears the note.
    assert!(face.verb("preset web"));
    face.on(&bandy::SMessage::BrowserMouseMove(1.0, 1.0, 0));
    assert_eq!(face.message_text().filter(|m| m.starts_with("preset ")), None);
    println!("striaexportpresetsmore: preset steps web, archive, proxy, none");
}

#[test]
fn striaexportpresetsmore_unknown_name_refused_pick_kept() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb("preset proxy"));
    assert!(face.verb("preset gif"));
    assert_eq!(face.last_error.take(), Some("preset: unknown preset gif (web|archive|proxy)".to_string()));
    assert!(face.verb("preset web archive"));
    assert_eq!(face.last_error.take(), Some("preset: usage: preset [none|web|archive|proxy]".to_string()));
    assert_eq!(face.export_preset().map(|p| p.name), Some("proxy"), "a refusal keeps the pick");
}

#[test]
fn striaexportpresetsmore_bare_export_takes_the_pick() {
    let (picked, named) = (nlefaceexport_dir("picked-proxy"), nlefaceexport_dir("picked-proxy-named"));
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb("preset proxy"));
    assert!(face.verb(&format!("export {}", picked.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the result line");
    assert!(line.starts_with("export: 35 frames, ") && line.contains(" h264 + aac:64 in "), "{line}");
    assert!(face.verb(&format!("export {} --preset proxy", named.display())));
    assert!(face.wait_export());
    assert!(face.last_error.take().is_some_and(|l| l.contains(" h264 + aac:64 in ")));
    assert_eq!(std::fs::read(picked.join("movie.mp4")).unwrap(), std::fs::read(named.join("movie.mp4")).unwrap(), "the pick is SR871's --preset, byte-exact");
    println!("striaexportpresetsmore: picked proxy {line}");
}

#[test]
fn striaexportpresetsmore_explicit_words_win() {
    let (over, words) = (nlefaceexport_dir("picked-over"), nlefaceexport_dir("picked-over-words"));
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb("preset web"));
    assert!(face.verb(&format!("export {} png wav", over.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the result line");
    assert!(line.contains(" png + wav in "), "explicit words beat the pick: {line}");
    assert!(face.verb(&format!("export {} --preset archive", words.display())));
    assert!(face.wait_export());
    assert!(face.last_error.take().is_some_and(|l| l.contains(" png + wav in ")), "an explicit --preset beats the pick");
    assert_eq!(face.export_preset().map(|p| p.name), Some("web"), "an export leaves the pick");
}

// ---- STRIASNAPMORE (LEDGER SR952): the `n` key flips the drag snap default; the help names it (named tail block) ----

/// `n` is a KEYS row onto SR892's `snap` word: a one-char BrowserKey alone is not taken; the text key flips the default off
/// (named on the message line), view-only (no undo step, not dirty, the timeline unchanged); again turns it back on.
#[test]
fn striasnapmore_n_key_flips_the_default_view_only() {
    assert!(KEYS.contains(&("n", "snap")));
    let mut f = nle5dkeyshelp_face(640, 480, false);
    let tl = f.state.timeline.clone();
    assert!(f.snap_on());
    assert!(!f.on(&bandy::SMessage::BrowserKey("n".into())), "a one-char BrowserKey alone is not taken");
    assert!(f.snap_on());
    assert!(f.on(&bandy::SMessage::BrowserText("n".into())));
    assert!(!f.snap_on());
    assert_eq!(f.note.take().as_deref(), Some("snap: off"));
    assert_eq!((f.state.undo_depth(), f.dirty()), (0, false));
    assert_eq!(f.state.timeline, tl);
    assert!(f.on(&bandy::SMessage::BrowserText("n".into())));
    assert!(f.snap_on());
    assert_eq!(f.note.take().as_deref(), Some("snap: on"));
    println!("striasnapmore: n -> snap: off, n -> snap: on, undo 0, clean");
}

/// The keys help has a row naming `n` with the snap word's phrase (not the bare word); while the help shows, `n` only
/// dismisses it and the snap default stays on.
#[test]
fn striasnapmore_help_names_the_key_and_n_under_help_only_dismisses() {
    let rows = help_rows();
    let row = rows.iter().find(|r| r.0.split(" / ").any(|k| k == "n")).expect("a help row for n");
    assert_eq!(row.1, help_phrase("snap"));
    assert!(!row.1.is_empty() && row.1 != "snap", "{:?}", row);
    let mut f = nle5dkeyshelp_face(640, 480, false);
    assert!(f.verb("help") && f.help_shown());
    assert!(f.on(&bandy::SMessage::BrowserText("n".into())));
    assert!(!f.help_shown());
    assert!(f.snap_on(), "a key under the help is not applied");
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    println!("striasnapmore: help row {:?}; n under the help only dismisses", row);
}

// ---- STRIATRANSITIONMORE (LEDGER SR956): the `[` / `]` help names both roads; a typed timecode `dur` (named tail block) ----

/// M3: the help rows of `[` and `]` name the clip trim and the selected transition's step; typed `dur 00:00:00:06` on a
/// selected dissolve gives it 6 ticks in one undo step, no error.
#[test]
fn striatransitionmore_bracket_help_names_the_transition() {
    use bandy::SMessage;
    use stria::nle_face::SURFACE;
    let rows = help_rows();
    for k in ["[", "]"] {
        let row = rows.iter().find(|r| r.0.split(" / ").any(|x| x == k)).expect("a help row");
        assert!(row.1.contains("clip start") && row.1.contains("transition"), "{k}: {:?}", row.1);
    }
    let (tl, src) = synth_fixture();
    let tl0 = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select_transition(0, 0).unwrap();
    assert!(face.on(&SMessage::Input { target: SURFACE.into(), text: "dur 00:00:00:06".into() }));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl0.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 6).unwrap());
    assert_eq!(face.state.undo_depth(), 1);
}

// ---- STRIAGRADEMORE (LEDGER SR957): the grade's digit keys, a summary-less reel's note, a size switch's bars (named tail block) ----

/// `2` / `4` / `6` (text keys) step B's grade up through SR899's words (one undo step each), `1` / `3` / `5` step it back;
/// a one-char BrowserKey is not taken; with nothing selected a digit lands on the message row by EditError::NoClip's name;
/// the keys help carries each digit on its word's row with its HELP phrase.
#[test]
fn striagrademore_digit_keys_step_the_grade() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.on(&bandy::SMessage::BrowserText("2".into())));
    assert_eq!(face.last_error.take(), Some(format!("bright+: {}", EditError::NoClip)));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl, 0));
    face.state.select(0, 1).unwrap();
    assert!(!face.on(&bandy::SMessage::BrowserKey("2".into())), "a one-char BrowserKey alone is not taken");
    for k in ["2", "4", "6"] {
        assert!(face.on(&bandy::SMessage::BrowserText(k.into())), "{k}");
        assert!(face.last_error.is_none(), "{k}: {:?}", face.last_error);
    }
    assert_eq!(face.state.timeline.tracks[0].clips[1].grade, timeline_core::Grade { brightness: 50, contrast: 50, saturation: 50 });
    assert_eq!(face.state.undo_depth(), 3);
    for k in ["1", "3", "5"] {
        assert!(face.on(&bandy::SMessage::BrowserText(k.into())), "{k}");
    }
    assert_eq!(face.state.timeline.tracks[0].clips[1].grade, timeline_core::Grade::IDENTITY);
    assert_eq!(face.state.undo_depth(), 6);
    let rows = help_rows();
    for (k, w) in [("1", "bright-"), ("2", "bright+"), ("3", "contrast-"), ("4", "contrast+"), ("5", "sat-"), ("6", "sat+")] {
        assert!(KEYS.contains(&(k, w)), "{k} -> {w}");
        let row = rows.iter().find(|(_, p)| *p == help_phrase(w)).expect("the grade row");
        assert!(row.0.split(" / ").any(|x| x == k), "{k} not in {:?}", row.0);
        assert!(help_phrase(w).ends_with("one step"), "{w}");
    }
    println!("striagrademore: keys 1..6 step the grade, depth 6");
}

/// A reel with nothing to say (B absent from the Sources) shows no line at identity; graded, it reads its grade alone; undo
/// gives None back. `reel_words`' three arms.
#[test]
fn striagrademore_note_without_a_summary() {
    use stria::nle_face::reel_words;
    let (tl, _) = synth_fixture();
    let mut src = Sources::new(&tl);
    src.add(SourceId(0), synth_source(0, 50, 25)).expect("synthetic source opens");
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 1).unwrap();
    assert_eq!(face.reel_line(), None);
    assert!(face.verb("bright+"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.reel_line().as_deref(), Some("B: bright 50 contrast 0 sat 0"));
    assert!(face.verb("undo"));
    assert_eq!(face.reel_line(), None);
    assert_eq!(reel_words("A".into(), Some("s".into()), String::new()).as_deref(), Some("A: s"));
    assert_eq!(reel_words("A".into(), Some("s".into()), "  bright 1 contrast 0 sat 0".into()).as_deref(), Some("A: s  bright 1 contrast 0 sat 0"));
    assert_eq!(reel_words("A".into(), None, "  bright 1 contrast 0 sat 0".into()).as_deref(), Some("A: bright 1 contrast 0 sat 0"));
    assert_eq!(reel_words("A".into(), None, String::new()), None);
}

/// SR384's two-size reel with every clip graded: the unswitched frames are `Grade::apply` of the decoder's conversion, the
/// switched ones are that graded picture letterboxed over FIELD, so the bars stay FIELD (SKIP offline).
#[test]
fn striagrademore_size_switch_bars_stay_black() {
    use gneiss_pal::dsp::demux::Demuxer;
    use gneiss_pal::dsp::video::{chroma_of, decoder_for};
    use timeline_core::view::{Canvas, FIELD, Order, Still};
    let Some(file) = nlesizeswitch_reel() else { return };
    let (tl, src) = Sources::from_media("two", &file, 25).unwrap().expect("an MP4");
    let g = timeline_core::Grade { brightness: 300, ..timeline_core::Grade::IDENTITY };
    let mut graded = tl.clone();
    for i in 0..graded.tracks[0].clips.len() {
        graded = graded.set_grade(0, i, g).unwrap();
    }
    let mut sink = NleSizeSwitchSink::default();
    render(&graded, 0..4, &src, &mut sink).unwrap();
    assert_eq!(sink.frames.len(), 4);
    let colour = src.open_report(SourceId(0)).and_then(|o| o.colour);
    let mut d = Demuxer::open(file.clone()).unwrap();
    let track = d.tracks()[0].clone();
    let mut dec = decoder_for(&track).unwrap();
    let mut decoded = Vec::new();
    while let Some(p) = d.next_packet() {
        decoded.extend(dec.decode(&p).unwrap());
    }
    decoded.extend(dec.flush());
    for (i, f) in decoded.iter().enumerate() {
        let mut native = f.to_rgba_colour(colour.as_ref(), chroma_of(&track));
        g.apply(&mut native);
        let want = if i < 2 {
            native
        } else {
            let mut c = Canvas::new(768, 512, Order::Rgba);
            c.rect(0, 0, 768, 512, FIELD);
            c.blit_fit(&Still { width: 1280, height: 720, rgba: native }, 0, 0, 768, 512);
            c.px
        };
        assert!(sink.frames[i].2 == want, "frame {i}: graded before the fit");
    }
    let px = |i: usize, x: usize, y: usize| sink.frames[i].2[(y * 768 + x) * 4..(y * 768 + x) * 4 + 4].to_vec();
    assert_eq!((px(2, 384, 0), px(2, 384, 511)), (FIELD.to_vec(), FIELD.to_vec()), "the bars are not graded");
    println!("striagrademore: graded size switch, bars FIELD");
}

// ---- STRIAOPACITYMORE (LEDGER SR958): the 8 / 9 keys step the selection's opacity (named tail block) ----

/// `8` (text) on a selected clip reads `opacity: 90 %` as one undo step, `9` reads `opacity: 100 %`; the named key
/// `BrowserKey("8")` is not taken; the keys help has a `8` row and a `9` row with their phrases.
#[test]
fn striaopacitymore_keys_step_the_selection_and_help_names_them() {
    use stria::nle_face::{EditorState, Face, PixelOrder, help_phrase, help_rows, synth_fixture};
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(0, 1).unwrap();
    assert!(face.on(&bandy::SMessage::BrowserText("8".into())));
    assert_eq!((face.last_error.take(), face.message_text().as_deref()), (None, Some("opacity: 90 %"))); // STRIAOPACITYNEXT (SR993): the percent is a note
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.on(&bandy::SMessage::BrowserText("9".into())));
    assert_eq!((face.last_error.take(), face.message_text().as_deref()), (None, Some("opacity: 100 %"))); // STRIAOPACITYNEXT (SR993): the percent is a note
    assert_eq!(face.state.undo_depth(), 2);
    assert!(!face.on(&bandy::SMessage::BrowserKey("8".into())), "a one-character key is text only");
    let rows = help_rows();
    for (k, a) in [("8", "opacity-"), ("9", "opacity+")] {
        assert!(rows.iter().any(|(keys, p)| *p == help_phrase(a) && keys.split(" / ").any(|x| x == k)), "{k} {a}");
        assert_ne!(help_phrase(a), a, "{a} has a phrase");
    }
    println!("striaopacitymore: key 8 -> opacity: 90 %, key 9 -> opacity: 100 %, help rows 8 / 9");
}

// ---- STRIAPANMORE (LEDGER SR959): the Alt pan keys and the pan on the whole selection set (named tail block) ----

/// Alt+Right / Alt+0 / Alt+Left (named keys) step / centre the selected audio clip through SR897's face words and read the
/// pan on the message row; the text form is not taken; the help names the keys; a set takes the pan on its audio clips
/// only, in one undo step; a set with no audio clip is refused `NoClip`.
#[test]
fn striapanmore_alt_keys_pan_and_read() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    let key = |f: &mut Face, k: &str| f.on(&bandy::SMessage::BrowserKey(k.into()));
    face.state.select(1, 1).unwrap();
    assert!(key(&mut face, "Alt+Right") && key(&mut face, "Alt+Right"));
    assert_eq!((face.state.timeline.tracks[1].clips[1].pan_cq8, face.note.take().as_deref()), (32, Some("pan: R 25"))); // STRIAPANNEXT (SR994): the reading is the face note
    assert!(key(&mut face, "Alt+0"));
    assert_eq!((face.state.timeline.tracks[1].clips[1].pan_cq8, face.note.take().as_deref()), (0, Some("pan: centre"))); // STRIAPANNEXT (SR994): the reading is the face note
    assert!(key(&mut face, "Alt+Left"));
    assert_eq!((face.state.timeline.tracks[1].clips[1].pan_cq8, face.note.take().as_deref()), (-16, Some("pan: L 13"))); // STRIAPANNEXT (SR994): the reading is the face note
    assert!(!face.on(&bandy::SMessage::BrowserText("Alt+Left".into())), "a named key is read from BrowserKey only");
    let rows = help_rows();
    for (k, w) in [("Alt+Left", "pan-"), ("Alt+0", "pan"), ("Alt+Right", "pan+")] {
        assert!(KEYS.contains(&(k, w)), "{k}");
        let row = rows.iter().find(|r| r.0.split(" / ").any(|x| x == k)).expect("a help row");
        assert_eq!(row.1, help_phrase(w));
        assert!(row.1 != w, "{w}: a phrase, not the word");
    }
    face.state.select(0, 1).unwrap();
    face.state.select_add(1, 0).unwrap();
    face.state.select_add(1, 2).unwrap();
    let d0 = face.state.undo_depth();
    assert!(key(&mut face, "Alt+Right"));
    let p = |f: &Face, t: usize, i: usize| f.state.timeline.tracks[t].clips[i].pan_cq8;
    assert_eq!((p(&face, 1, 0), p(&face, 1, 2), p(&face, 0, 1), face.state.undo_depth()), (16, 16, 0, d0 + 1), "audio clips only, one step");
    assert_eq!(face.note.take().as_deref(), Some("pan: R 13")); // STRIAPANNEXT (SR994): the reading is the face note
    face.state.select(0, 0).unwrap();
    face.state.select_add(0, 1).unwrap();
    assert!(face.verb("pan+"));
    assert_eq!(face.last_error.take(), Some(format!("pan: {}", EditError::NoClip)), "no audio clip in the set");
    assert_eq!(face.state.undo_depth(), d0 + 1);
    println!("striapanmore: Alt keys -> R 25 / centre / L 13; set of two audio + one video -> 16, 16, 0");
}

// ---- STRIALOUDNESSMORE (LEDGER SR961): the measure decodes no video; its progress on the export band (named tail block) ----

/// A sink that declines every frame and keeps the PCM (the loudness sink's shape, observable).
#[derive(Default)]
struct StriaLoudnessMoreSink {
    asked: Vec<i64>,
    frames: usize,
    pcm: Vec<f32>,
}

impl stria::nle::RenderSink for StriaLoudnessMoreSink {
    fn frame(&mut self, _out_t: i64, _width: u32, _height: u32, _rgba: &[u8]) {
        self.frames += 1;
    }
    fn pcm(&mut self, _out_t: i64, _rate: u32, _channels: u16, interleaved: &[f32]) {
        self.pcm.extend_from_slice(interleaved);
    }
    fn frame_wanted(&mut self, out_t: i64) -> bool {
        self.asked.push(out_t);
        false
    }
}

/// Rule: a sink answering `frame_wanted` false is asked once per video step (in output order) and never handed a frame;
/// the report counts no frames; the PCM is the very PCM a frame-taking render sends.
#[test]
fn strialoudnessmore_unwanted_frames_decode_nothing() {
    let (tl, src) = synth_fixture();
    let range = 0..tl.end();
    let video: Vec<i64> = schedule(&tl, range.clone(), &src).unwrap().iter().filter(|s| s.kind == Kind::Video).map(|s| s.out_t).collect();
    let mut sink = StriaLoudnessMoreSink::default();
    let r = render(&tl, range.clone(), &src, &mut sink).unwrap();
    assert_eq!((sink.frames, r.frames), (0, 0));
    assert_eq!(sink.asked, video);
    let mut full = StriaLoudnessPcm::default();
    render(&tl, range, &src, &mut full).unwrap();
    assert!(!full.pcm.is_empty(), "the fixture has audio");
    assert_eq!(sink.pcm, full.pcm);
    println!("strialoudnessmore: {} video steps asked, 0 decoded, {} PCM samples alike", video.len(), full.pcm.len());
}

/// Rule: the measure (which now declines every frame) still reads BS.1770 over exactly the PCM a full render mixes, and
/// its watched count still moves inside the range.
#[test]
fn strialoudnessmore_measure_unchanged_without_video() {
    let (tl, src) = synth_fixture();
    let range = 0..tl.end();
    let mut full = StriaLoudnessPcm::default();
    render(&tl, range.clone(), &src, &mut full).unwrap();
    let (rate, ch) = full.fmt.expect("the fixture has audio");
    let want = gneiss_pal::dsp::audio::meter::integrated_lufs(&full.pcm, ch as usize, rate);
    assert_eq!(stria::nle::loudness(&tl, range.clone(), &src).unwrap(), Some(want));
    let total = (range.end - range.start) as u64;
    let w = stria::nle::LoudnessWatch::new(&range);
    assert_eq!(stria::nle::loudness_watched(&tl, range, &src, Some(w.clone())).unwrap(), Some(want));
    let done = w.done.load(std::sync::atomic::Ordering::Relaxed);
    assert!(done > 0 && done <= total, "done {done} of {total}");
    println!("strialoudnessmore: {want:.2} LUFS with no video decoded, done {done}/{total}");
}

/// Rule: the export band's fraction falls back to the running measure's: none before the word, a fraction in [0, 1]
/// while it runs, none once reported; the idle paint is byte-identical before and after.
#[test]
fn strialoudnessmore_band_follows_the_measure() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert_eq!(face.loudness_frac(), None);
    let idle = face.paint();
    assert!(face.verb("loudness"));
    let f = face.loudness_frac().expect("a running measure has a fraction");
    assert!((0.0..=1.0).contains(&f), "{f}");
    assert!(face.wait_loudness());
    assert_eq!(face.loudness_frac(), None);
    face.last_error = None;
    assert_eq!(stria::nle_face::fnv1a(&face.paint()), stria::nle_face::fnv1a(&idle), "no band once the measure is reported");
    println!("strialoudnessmore: band fraction {f:.3} while running, none after");
}


// ---- STRIATHUMBNAILSMORE (LEDGER SR949): a wide video clip's filmstrip and tail, decoded on the heads' budget (named tail block) ----

/// STRIATHUMBNAILSMORE (SR949): the synth fixture at zoom 16 on a 640 px canvas (each V1 body 158 px, over two 48 px heads):
/// painted until no tile is pending, clip 0 (A[13,23)@0) shows A@13 at its left, A@16 (48 px / 16 px per tick = 3 ticks in)
/// next to it and A@22 (src_out - 1) right-aligned, each box the preview road's frame scaled; three clips x three stills =
/// nine decodes, and a further paint decodes none.
#[test]
fn striathumbnailsmore_face_filmstrip_and_tail() {
    let (tl, src) = synth_fixture();
    let want = [13i64, 16, 22].map(|t| striathumbnails_expected(&tl, &src, SourceId(0), t));
    let mut st = EditorState::new(tl);
    st.zoom = 16;
    let mut face = Face::new(st, src, (640, 160), PixelOrder::Rgba, false);
    let mut buf = face.paint();
    let mut paints = 1;
    while face.thumbs_pending() && paints < 10 {
        buf = face.paint();
        paints += 1;
    }
    assert!(!face.thumbs_pending(), "every visible tile decoded within {paints} paints");
    assert_eq!(face.thumb_decodes(), 9, "three clips: head, one middle, tail each");
    let c = face.state.timeline.tracks[0].clips[0];
    let l = face.layout();
    let (x0, x1, y0) = (l.x_of(c.at) + 1, l.x_of(c.end()) - 1, l.row_y(0) + 2);
    assert_eq!(x1 - x0, 158);
    for (i, (bx, t)) in [(x0, 13i64), (x0 + 48, 16), (x1 - 48, 22)].into_iter().enumerate() {
        assert_eq!(face.thumb(SourceId(0), t), Some(&want[i]), "A@{t} cached");
        let got = striathumbnails_box(&buf, l.w, bx, y0, want[i].width, want[i].height);
        assert_eq!(got, want[i].rgba, "the box at x {bx} is A@{t}");
    }
    let again = face.paint();
    assert_eq!(face.thumb_decodes(), 9, "a further paint decodes nothing");
    assert_eq!(again, buf, "the strip never changes the picture between paints");
    println!("striathumbnailsmore: filmstrip A@13 / A@16 / A@22 decodes={} paints={paints}", face.thumb_decodes());
}

/// STRIATHUMBNAILSMORE (SR949): heads first: four wide clips (zoom 16, 158 px bodies) on a 640 px canvas: the first paint's
/// THUMBS_PER_PAINT (4) decodes are the four heads and leaves the tiles pending; the next paints fill every middle and tail.
#[test]
fn striathumbnailsmore_heads_before_tiles() {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let v = tl.add_track(Kind::Video);
    tl.tracks[v].clips = (0..5).map(|i| Clip::new(a, i * 10, i * 10 + 10, i * 10)).collect();
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    let mut st = EditorState::new(tl);
    st.zoom = 16;
    let mut face = Face::new(st, src, (640, 160), PixelOrder::Rgba, false);
    let _ = face.paint();
    assert_eq!(face.thumb_decodes(), 4, "THUMBS_PER_PAINT");
    assert!(face.thumbs_pending(), "tiles left");
    for i in 0..4i64 {
        assert!(face.thumb(a, i * 10).is_some(), "head {i} first");
        assert!(face.thumb(a, i * 10 + 3).is_none() && face.thumb(a, i * 10 + 9).is_none(), "no tile of clip {i} before every head");
    }
    let mut paints = 1;
    while face.thumbs_pending() && paints < 10 {
        let _ = face.paint();
        paints += 1;
    }
    assert_eq!(face.thumb_decodes(), 12, "four visible clips x (head, middle, tail); clip 4 is off the canvas");
    for i in 0..4i64 {
        assert!(face.thumb(a, i * 10 + 3).is_some() && face.thumb(a, i * 10 + 9).is_some(), "clip {i}'s middle and tail");
    }
    println!("striathumbnailsmore: heads first, decodes=12 after {paints} paints");
}

// ---- STRIAMULTISELECTMORE (LEDGER SR950): the set toggle and the set dragged to another track (named tail block) ----
// A shift-click on a clip already in NLE2c's set takes it out (timeline_core `Drag::release_still` ->
// `EditorState::select_remove`, no undo step); a set dragged onto another row of its kind lands there as ONE commit
// (`EditorState::move_set_track` -> `Timeline::move_many_track`).

#[test]
fn striamultiselectmore_shift_click_member_toggles_it_out() {
    let (tl, src) = stria::nle_face::synth_fixture();
    let mut face = stria::nle_face::Face::new(stria::nle_face::EditorState::new(tl), src, (320, 160), stria::nle_face::PixelOrder::Rgba, false);
    let tl0 = face.state.timeline.clone();
    let (x, y) = face.layout().point(0, 15); // V1's B[0,10)@10
    face.on(&nle3eslipdrag_btn(true, x, y, 0));
    face.on(&nle3eslipdrag_btn(false, x, y, 0));
    let (xa, ya) = face.layout().point(1, 30); // A1's A[23,33)@25
    face.on(&nle3eslipdrag_btn(true, xa, ya, bandy::MOD_SHIFT));
    face.on(&nle3eslipdrag_btn(false, xa, ya, bandy::MOD_SHIFT));
    assert_eq!((face.state.selection, face.state.set.clone()), (Some((1, 2)), vec![(0, 1), (1, 2)]));
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_SHIFT));
    assert!(face.on(&nle3eslipdrag_btn(false, x, y, bandy::MOD_SHIFT)));
    assert_eq!((face.state.selection, face.state.set.clone()), (Some((1, 2)), vec![(1, 2)]), "V1's B toggled out");
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl0, 0), "view-only");
    face.on(&nle3eslipdrag_btn(true, xa, ya, bandy::MOD_SHIFT));
    face.on(&nle3eslipdrag_btn(false, xa, ya, bandy::MOD_SHIFT));
    assert_eq!((face.state.selection, face.state.set.len()), (None, 0), "the last member toggled out");
}

#[test]
fn striamultiselectmore_set_drag_down_to_another_video_track_one_undo() {
    let (mut tl, src) = stria::nle_face::synth_fixture();
    tl.add_track(Kind::Video);
    let mut face = stria::nle_face::Face::new(stria::nle_face::EditorState::new(tl), src, (320, 200), stria::nle_face::PixelOrder::Rgba, false);
    let tl0 = face.state.timeline.clone();
    let (x, y) = face.layout().point(0, 15); // V1's B[0,10)@10
    face.on(&nle3eslipdrag_btn(true, x, y, 0));
    face.on(&nle3eslipdrag_btn(false, x, y, 0));
    let (xa, ya) = face.layout().point(0, 5); // V1's A[13,23)@0
    face.on(&nle3eslipdrag_btn(true, xa, ya, bandy::MOD_SHIFT));
    face.on(&nle3eslipdrag_btn(false, xa, ya, bandy::MOD_SHIFT));
    assert_eq!(face.state.set, vec![(0, 0), (0, 1)]);
    let (_, y2) = face.layout().point(2, 15); // V2, empty
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_ALT));
    face.on(&bandy::SMessage::BrowserMouseMove(x + 40.0, y2, 0));
    assert_eq!(face.dragging().map(|d| (d.delta, d.to_track)), Some((5, 2)));
    face.on(&nle3eslipdrag_btn(false, x + 40.0, y2, bandy::MOD_ALT));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let want = tl0.move_many_track(&[(0, 0), (0, 1)], 2, 5).unwrap();
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&want, 1), "ONE commit");
    assert_eq!(face.state.timeline.tracks[0].clips.len(), 1, "V1 keeps its third clip");
    assert_eq!(face.state.set, vec![(2, 0), (2, 1)]);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl0);
}

// ---- STRIATRACKSMORE (LEDGER SR955): the `track-` success line is the face's note, not an error (named tail block) ----

/// `track- A2` through `Face::on` names the removal in `note` (no `last_error`: the witness counts that as an error);
/// `message_text` reads it, an error still wins over it, and the next taken input clears it. Undo then puts the A2
/// selection back on its own track (SR955's track map).
#[test]
fn striatracksmore_track_remove_note_cleared_by_next_input() {
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    let input = |w: &str| bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: w.into() };
    assert!(face.on(&input("track+ audio")));
    assert!(face.on(&input("track- A2")));
    let line = "track-: removed A2 (0 clips)";
    assert_eq!((face.last_error.clone(), face.note.as_deref(), face.message_text().as_deref()), (None, Some(line), Some(line)));
    face.last_error = Some("x: y".into());
    assert_eq!(face.message_text().as_deref(), Some("x: y"), "an error wins over the note");
    face.last_error = None;
    assert!(face.on(&input("zoom+")));
    assert_eq!(face.note, None, "the next taken input clears it");
    assert_eq!(face.state.timeline, tl0);
    face.state.select(1, 0).unwrap();
    let a = face.state.selected();
    assert!(face.on(&input("track- V")));
    assert_eq!((face.state.selection, face.note.as_deref()), (Some((0, 0)), Some("track-: removed V (3 clips)")));
    assert!(face.on(&input("undo")));
    assert_eq!((face.state.timeline.clone(), face.state.selection, face.state.selected(), face.note.clone()), (tl0.clone(), Some((1, 0)), a, None));
    println!("striatracksmore: track- A2 -> note {line:?}, cleared by zoom+; track- V then undo -> the A1 selection back on track 1");
}

// ---- STRIAROLLMORE (LEDGER SR954): the roll tool on the face — r / `roll-tool`, the trim-handle drag rolls (named tail block) ----

/// Rule: `r` (text) and the word `roll-tool` toggle the roll tool; the word is view-only (no undo step, the document stays
/// clean); it is a VERBS word right before `help` with a HELP phrase, and `r` alone as `BrowserKey` is not taken.
#[test]
fn striarollmore_r_and_the_word_toggle_the_tool() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(!face.roll_tool());
    assert!(!face.on(&bandy::SMessage::BrowserKey("r".into())), "a one-char BrowserKey alone is not taken");
    assert!(face.on(&bandy::SMessage::BrowserText("r".into())));
    assert!(face.roll_tool());
    assert!(face.verb("roll-tool"));
    assert!(!face.roll_tool());
    assert_eq!((&face.state.timeline, face.state.undo_depth(), face.dirty()), (&tl, 0, false));
    assert_eq!(VERBS[VERBS.len() - 2..], ["roll-tool", "help"]);
    assert!(KEYS.contains(&("r", "roll-tool")));
    assert_ne!(help_phrase("roll-tool"), "roll-tool");
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    println!("striarollmore: r / roll-tool toggle the tool, no undo step, clean");
}

/// Rule: the tool on, an Alt-press (no snap) on V1 clip 0's out handle (synth_fixture: A [13,23)@0 | B [0,10)@10 butted)
/// dragged +3 ticks is ONE roll: the timeline equals `Timeline::roll(0, 0, 3)`, one undo step restores it; the tool off,
/// the same handle dragged -3 is the plain trim (clip 0 ends at 7, clip 1 stays at 10).
#[test]
fn striarollmore_tool_drag_rolls_the_cut() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 0).unwrap();
    assert!(face.verb("roll-tool"));
    let l = face.layout();
    let (x, y) = (l.x_of(10) as f64 - 2.0, l.point(0, 5).1);
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_ALT));
    assert_eq!(face.dragging().map(|d| (d.kind, d.idx, d.roll)), Some((DragKind::TrimOut, 0, true)));
    face.on(&bandy::SMessage::BrowserMouseMove(x + 3.0 * l.zoom as f64, y, 0));
    face.on(&nle3eslipdrag_btn(false, x + 3.0 * l.zoom as f64, y, bandy::MOD_ALT));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl.roll(0, 0, 3).unwrap());
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    assert!(face.verb("roll-tool"));
    face.state.select(0, 0).unwrap();
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_ALT));
    assert_eq!(face.dragging().map(|d| (d.kind, d.roll)), Some((DragKind::TrimOut, false)));
    face.on(&bandy::SMessage::BrowserMouseMove(x - 3.0 * l.zoom as f64, y, 0));
    face.on(&nle3eslipdrag_btn(false, x - 3.0 * l.zoom as f64, y, bandy::MOD_ALT));
    let v1 = &face.state.timeline.tracks[0].clips;
    assert_eq!((v1[0], v1[1].at), (Clip::new(SourceId(0), 13, 20, 0), 10));
    println!("striarollmore: tool on = Timeline::roll(0, 0, 3); tool off = the plain trim");
}

/// Rule: while the tool is on the message line names it (after any error); off, the line is what it was.
#[test]
fn striarollmore_message_names_the_tool() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    let before = face.message_text();
    assert!(face.verb("roll-tool"));
    assert_eq!(face.message_text().as_deref(), Some(stria::nle_face::ROLL_TOOL_NOTE));
    face.last_error = Some("x".into());
    assert_eq!(face.message_text().as_deref(), Some("x"));
    face.last_error = None;
    assert!(face.verb("roll-tool"));
    assert_eq!(face.message_text(), before);
    println!("striarollmore: the message line reads {:?} while the tool is on", stria::nle_face::ROLL_TOOL_NOTE);
}

// ---- STRIATCMORE (LEDGER SR964): the readout reads the time base (named tail block) ----

/// A 30000/1001 timeline's readout is drop-frame at the real rate (frames the tick, seconds at 1001/30000); a 1/25 base
/// reads exactly `tc_text` at 25 as before.
#[test]
fn striatcmore_readout_reads_an_ntsc_time_base() {
    use stria::nle_face::{TcUnit, tc_text};
    let mut face = striatimecodedisplay_face(false);
    face.state.timeline.tb = timeline_core::Timebase { num: 1001, den: 30000 };
    face.state.playhead = 1800;
    assert!(face.verb("tc timecode"));
    assert_eq!(face.tc_readout().as_deref(), Some("00:01:00;02"));
    assert!(face.verb("tc frames"));
    assert_eq!(face.tc_readout().as_deref(), Some("1800"));
    assert!(face.verb("tc seconds"));
    assert_eq!(face.tc_readout().as_deref(), Some("60.06 s"));
    let mut plain = striatimecodedisplay_face(false);
    plain.state.timeline.tb = timeline_core::Timebase { num: 1, den: 25 };
    plain.state.seek(12);
    for (w, u) in [("tc timecode", TcUnit::Timecode), ("tc frames", TcUnit::Frames), ("tc seconds", TcUnit::Seconds)] {
        assert!(plain.verb(w));
        assert_eq!(plain.tc_readout(), Some(tc_text(plain.state.playhead, 25, u)), "{w}");
    }
    assert!(face.last_error.is_none() && plain.last_error.is_none());
    println!("striatcmore: 30000/1001 @ 1800 reads 00:01:00;02 / 1800 / 60.06 s");
}

/// With the readout off, an NTSC timeline paints byte-identical before and after `tc` / `tc off`.
#[test]
fn striatcmore_readout_off_ntsc_paint_is_unchanged() {
    let mut face = striatimecodedisplay_face(true);
    face.state.timeline.tb = timeline_core::Timebase { num: 1001, den: 30000 };
    let off = face.paint();
    assert!(face.verb("tc"));
    assert_ne!(face.paint(), off, "the readout shows");
    assert!(face.verb("tc off"));
    assert_eq!(face.paint(), off, "off: byte-identical");
    println!("striatcmore: ntsc off fnv {:016x}", stria::nle_face::fnv1a(&off));
}

// ---- STRIANORMALIZEMORE (LEDGER SR963): `normalize ... tp` against the true (oversampled) peak (named tail block) ----

/// Rule: `up_peak` through the mix's one Sinc at 4x reads the inter-sample peak a sample peak misses: a 48 kHz sine at
/// fs / 4 and 45 degrees has every sample at +-0.7071 while the band-limited wave reaches 1.0.
#[test]
fn strianormalizemore_up_peak_reads_the_inter_sample_peak() {
    let pcm: Vec<f32> = (0..4800).map(|n| (std::f64::consts::FRAC_PI_2 * n as f64 + std::f64::consts::FRAC_PI_4).sin() as f32).collect();
    let sample = pcm.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    assert!((sample - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{sample}");
    let os = stria::nle::TRUE_PEAK_OS;
    let mut s = gneiss_pal::dsp::audio::resample::Sinc::new(48_000, 48_000 * os, 1);
    let tp = stria::nle::up_peak(&mut s, 1, &pcm, false).max(stria::nle::up_peak(&mut s, 1, &[], true));
    assert!(tp > 0.95 && tp < 1.2, "true peak {tp} vs sample peak {sample}");
    println!("strianormalizemore: fs/4 at 45 deg sample={sample} true={tp}");
}

/// Rule: `peak_over(.., 1)` is SR878's `peak_of` exactly; at `TRUE_PEAK_OS` it is never below it and within 3 dB of it on
/// synth source A (whole and the 13..33 part).
#[test]
fn strianormalizemore_peak_over_extends_peak_of() {
    let (tl, src) = stria::nle_face::synth_fixture();
    let a = tl.source_by_reel("A").unwrap().id;
    for (t0, t1) in [(0i64, 50i64), (13, 33)] {
        let ns = tl.tb.to_ns(t0)..tl.tb.to_ns(t1);
        let p = stria::nle::peak_of(&src, a, ns.clone()).unwrap().unwrap();
        assert_eq!(stria::nle::peak_over(&src, a, ns.clone(), 1).unwrap().unwrap(), p);
        let tp = stria::nle::peak_over(&src, a, ns, stria::nle::TRUE_PEAK_OS).unwrap().unwrap();
        assert!(tp >= p && tp <= p * 1.4126, "{t0}..{t1}: true {tp} vs sample {p}");
        println!("strianormalizemore: {t0}..{t1} sample={p} true={tp}");
    }
}

/// Rule: `normalize tp` / `normalize -3 tp` / `normalize tp -3` set `normalize_gain_cdb(true peak, target)` in ONE undo
/// step with a `true peak` line; the token is filtered before the level, so `normalize tp x` refuses `x` by name.
#[test]
fn strianormalizemore_normalize_tp_sets_gain_from_the_true_peak() {
    for (w, target) in [("normalize tp", -100), ("normalize -3 tp", -300), ("normalize tp -3", -300)] {
        let (mut face, a) = strianormalize_face();
        let c = face.state.selected().unwrap();
        let tb = face.state.timeline.tb;
        let (_, src) = stria::nle_face::synth_fixture();
        let tp = stria::nle::peak_over(&src, a, tb.to_ns(c.src_in)..tb.to_ns(c.src_out), stria::nle::TRUE_PEAK_OS).unwrap().unwrap();
        let want = stria::nle_face::normalize_gain_cdb(tp, target).unwrap();
        let depth = face.state.undo_depth();
        assert!(face.verb(w), "{w}");
        assert_eq!(face.state.selected().unwrap().gain_cdb, want, "{w}");
        assert_eq!(face.state.undo_depth(), depth + 1, "{w}: one undo step");
        let peak_cdb = (2000.0 * (tp as f64).log10()).round() as i32;
        let line = format!("normalize: true peak {} -> gain {}", stria::nle_face::db_words(peak_cdb), stria::nle_face::db_words(want));
        assert_eq!(face.last_error.take(), Some(line), "{w}");
    }
    let (mut face, _) = strianormalize_face();
    assert!(face.verb("normalize tp x"));
    assert_eq!(face.last_error.take(), Some("normalize: x is not a dBFS level".to_string()));
    assert_eq!(face.state.selected().unwrap().gain_cdb, 0, "a refusal leaves the timeline");
}

// ---- STRIASLIPVIEWMORE (LEDGER SR946): the slide drag's cuts, the word slips' hold, the witness size (named tail block) ----

/// A FIELD-filled cell of `w` x `h` (the side `slip_split` leaves when its frame is missing).
fn striaslipviewmore_field(w: u32, h: u32) -> Vec<u8> {
    timeline_core::view::FIELD.repeat((w * h) as usize)
}

/// Rule: a Ctrl Slide on B's in handle (SR737's pair: A[13,23)@0 butted to B[0,10)@10, A@25 NOT butted) shows the frame
/// the slide moves: the left cell is a twin's frame at `10 + d - 1` after `slide d` (A's new last frame), the right cell
/// is FIELD (no butted next clip); nothing commits during the drag, and the release commits one slide.
#[test]
fn striaslipviewmore_slide_drag_shows_the_moving_cut_frames() {
    let (mut face, mut twin, tl) = nle3eslipface_pair();
    let l = face.layout();
    let (x, y) = (l.x_of(10) as f64 + 1.0, l.point(0, 15).1);
    striaslipview_press_move(&mut face, x, y, 16.0, bandy::MOD_CONTROL);
    let d = face.dragging().map(|d| (d.kind, d.delta)).expect("a live drag");
    assert_eq!(d.0, DragKind::Slide);
    assert!(d.1 > 0, "slid later by {}", d.1);
    face.paint();
    assert_eq!(twin.verb(&format!("slide {}", d.1)), Some(Ok(())));
    let (_, src) = synth_fixture();
    let last_a = preview(&twin.timeline, &src, 10 + d.1 - 1).unwrap().expect("A's new last frame");
    let pair = face.slip_view().cloned().expect("a pair during the slide");
    assert_eq!((pair.width, pair.height), (last_a.width * 2, last_a.height));
    let (left, right) = striaslipview_halves(&pair);
    assert_eq!(left, last_a.rgba, "left = the previous clip's new last frame");
    assert_eq!(right, striaslipviewmore_field(last_a.width, last_a.height), "right = FIELD: no butted next clip");
    assert_eq!(face.state.timeline, tl, "nothing committed during the drag");
    face.on(&nle3eslipdrag_btn(false, x + 16.0, y, bandy::MOD_CONTROL));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&twin.timeline, 1));
    face.paint();
    assert!(face.slip_view().is_none(), "released: no pair");
    println!("striaslipviewmore: Ctrl slide {} on B -> left A's last frame at {}, right FIELD", d.1, 10 + d.1 - 1);
}

/// Rule: the word `slide+` on a clip butted on both sides (A@25 moved to 20) holds both cells: the previous clip's new
/// last frame (tick 10) and the next clip's new first frame (tick 21), each a twin's frame after `slide 1`.
#[test]
fn striaslipviewmore_slide_word_both_neighbours() {
    let (mut tl, src) = synth_fixture();
    tl.tracks[0].clips[2].at = 20;
    let mut face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select_at(0, 12).unwrap();
    let mut twin = EditorState::new(tl);
    twin.select_at(0, 12).unwrap();
    assert!(face.verb("slide+"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    face.paint();
    assert_eq!(twin.verb("slide 1"), Some(Ok(())));
    let (prev, next) = striaslipview_frames(&twin.timeline, 10, 21);
    assert_eq!(striaslipview_halves(face.slip_view().expect("the slide's pair")), (prev.rgba, next.rgba));
    assert!(face.slip_held());
    println!("striaslipviewmore: slide+ with both cuts butted -> frames at 10 and 21");
}

/// Rule: the word `slip+` holds the pair of the selected clip (a twin's frames at 10 and 19 after `slip 1`) with ONE undo
/// step, the playhead frame not re-rendered while it shows; a zoom and a hover keep it; `undo` ends it.
#[test]
fn striaslipviewmore_word_slip_holds_its_pair() {
    let (mut face, mut twin, _tl) = nle3eslipface_pair();
    face.state.seek(12);
    face.paint();
    let before = face.still().cloned().expect("the playhead frame at 12");
    assert!(face.verb("slip+"));
    face.paint();
    assert!(face.slip_held());
    assert_eq!(twin.verb("slip 1"), Some(Ok(())));
    let (first, last) = striaslipview_frames(&twin.timeline, 10, 19);
    assert_eq!(striaslipview_halves(face.slip_view().expect("the word's pair")), (first.rgba, last.rgba));
    assert_eq!(face.still(), Some(&before), "the playhead frame is not re-rendered while the pair shows");
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&twin.timeline, 1), "one undo step");
    face.on(&bandy::SMessage::BrowserZoom(1));
    face.paint();
    assert!(face.slip_view().is_some(), "a zoom keeps the pair");
    face.on(&bandy::SMessage::BrowserMouseMove(5.0, 5.0, 0));
    face.paint();
    assert!(face.slip_view().is_some(), "a hover keeps the pair");
    assert!(face.verb("undo"));
    face.paint();
    assert!(face.slip_view().is_none() && !face.slip_held(), "undo ends the hold");
    println!("striaslipviewmore: slip+ holds the pair; zoom / hover keep it; undo ends it");
}

/// Rule: the next action ends a word's hold and its pair: a seek, another selection, `play`, a press.
#[test]
fn striaslipviewmore_next_action_ends_the_hold() {
    let (mut face, _twin, _tl) = nle3eslipface_pair();
    let (x, y) = face.layout().point(0, 15);
    let actions: [(&str, fn(&mut Face, f64, f64)); 4] = [
        ("seek", |f, _, _| f.state.seek(3)),
        ("select", |f, _, _| f.state.select_at(0, 2).unwrap()),
        ("play", |f, _, _| assert!(f.verb("play"))),
        ("press", |f, x, y| {
            f.on(&nle3eslipdrag_btn(true, x, y, 0));
        }),
    ];
    for (what, act) in actions {
        face.state.select_at(0, 12).unwrap();
        assert!(face.verb("slip+"));
        face.paint();
        assert!(face.slip_view().is_some(), "{what}: the word's pair");
        act(&mut face, x, y);
        face.paint();
        assert!(face.slip_view().is_none() && !face.slip_held(), "{what}: the hold ended");
        if face.playing {
            assert!(face.verb("play"));
        }
        if face.dragging().is_some() {
            face.on(&bandy::SMessage::BrowserKey("Escape".into()));
        }
    }
    println!("striaslipviewmore: seek / select / play / press end the hold");
}

/// Rule: a refused word (`slip-` at B's source start: OutOfSource) changes nothing and holds no pair; a word slip on the
/// audio track holds no pair (video only).
#[test]
fn striaslipviewmore_refused_and_audio_words_hold_no_pair() {
    let (mut face, _twin, tl) = nle3eslipface_pair();
    assert!(face.verb("slip-"));
    assert!(face.last_error.take().is_some(), "refused by name");
    face.paint();
    assert!(face.slip_view().is_none() && !face.slip_held(), "a refused word holds nothing");
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl, 0));
    face.state.select_at(1, 12).unwrap();
    assert!(face.verb("slip+"));
    face.paint();
    assert!(face.slip_view().is_none(), "an audio clip shows no pair");
    println!("striaslipviewmore: refused slip- and an audio slip+ -> no pair");
}

// ---- STRIACLIPSPEEDMORE (LEDGER SR951): the rippling speed word on the face (named tail block) ----

/// STRIACLIPSPEEDMORE: on the synth fixture (A 13..23 @0, B @10, A 23..33 @25) `speed 0.5` on the first clip runs into B
/// (`Overlap`), while `rspeed 0.5` (THE editor verb, reached by the face's fall-through) ripples: the clip is 20 ticks, B
/// moves to 20 and the last clip to 35, ONE undo step restores the fixture; the other track is untouched.
#[test]
fn striaclipspeedmore_face_rspeed_ripples_where_speed_overlaps() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(0, 0).unwrap();
    assert!(face.verb("speed 0.5"));
    assert_eq!(face.last_error.take(), Some(format!("speed 0.5: {}", EditError::Overlap)));
    assert!(face.verb("rspeed 0.5"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let ats: Vec<(i64, i64, u32)> = face.state.timeline.tracks[0].clips.iter().map(|c| (c.at, c.end(), c.rate_q16)).collect();
    assert_eq!(ats, vec![(0, 20, 0x8000), (20, 30, 0x1_0000), (35, 45, 0x1_0000)]);
    assert_eq!(face.state.timeline.tracks[1], tl.tracks[1], "the other track stays (NLE-SPEED-RIPPLE-LINKED owed)");
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.state.undo());
    assert_eq!(face.state.timeline, tl, "undo restores");
    println!("striaclipspeedmore: rspeed 0.5 -> {ats:?}; speed 0.5 -> {}", EditError::Overlap);
}

// ---- STRIARIPPLEMORE (LEDGER SR953): the ripple across every track through the face's word road (named tail block) ----

/// Rule: `rtrim-all out -3` on V1 clip 0 (A[13,23)@0) reaches timeline_core's verb through the face: V1 and A1 (whose
/// clip 0 is V1's exact-span twin) ripple together: both clip 0s end at 7, V1's later clips move to 7 / 17, A1's to
/// 7 / 22; ONE undo step restores; no message.
#[test]
fn striaripplemore_rtrim_all_moves_every_track() {
    let (mut face, tl) = striarippletrim_face(0);
    assert!(face.verb("rtrim-all out -3"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl.ripple_all_out(0, 0, 20).unwrap());
    let ats = |k: usize| face.state.timeline.tracks[k].clips.iter().map(|c| (c.at, c.end())).collect::<Vec<_>>();
    assert_eq!(ats(0), vec![(0, 7), (7, 17), (17, 27)], "V1");
    assert_eq!(ats(1), vec![(0, 7), (7, 17), (22, 32)], "A1 follows");
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    println!("striaripplemore: rtrim-all out -3 ripples V1 and A1 together");
}

/// Rule: a clip on another track straddling the edit point cannot stay in sync: V1 clip 2's `rtrim-all out 1` (its end
/// 30 inside A1's 25..35) is refused by name on the message line; the timeline and undo depth do not move.
#[test]
fn striaripplemore_a_straddler_reads_on_the_message_row() {
    let (mut face, tl) = striarippletrim_face(2);
    assert!(face.verb("rtrim-all out 1"));
    assert_eq!(face.last_error.take(), Some(format!("rtrim-all out 1: {}", EditError::Overlap)));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl, 0));
    println!("striaripplemore: rtrim-all out 1 over A1's 25..35 -> {}", EditError::Overlap);
}

// ---- STRIAMARKERSMORE (LEDGER SR966): `Alt+m` and the help row for `unmark all` (named tail block) ----

/// `unmark all` is a VERBS word just before `help`; `Alt+m` (a named key, never text) removes every marker in ONE commit
/// with SR895's message; with none it is refused by name and pushes no undo step; the help shows `Alt+m` with its phrase.
#[test]
fn striamarkersmore_alt_m_unmarks_all_and_help_row() {
    use stria::nle_face::{HELP, KEYS, help_phrase, help_rows};
    assert_eq!(VERBS[VERBS.len() - 2], "unmark all");
    assert_eq!(VERBS.last(), Some(&"help"));
    assert!(KEYS.contains(&("Alt+m", "unmark all")));
    assert!(HELP.iter().any(|(a, _)| *a == "unmark all"));
    let mut f = striamarkers_face();
    f.state.seek(12);
    assert!(f.verb("mark"));
    f.state.seek(30);
    assert!(f.verb("mark A"));
    f.last_error = None;
    let depth = f.state.undo_depth();
    assert!(!f.on(&bandy::SMessage::BrowserText("Alt+m".into())), "a named key, not text");
    assert!(f.on(&bandy::SMessage::BrowserKey("Alt+m".into())));
    assert_eq!(f.last_error.take().as_deref(), Some("unmarked 2 markers"));
    assert!(f.state.timeline.markers.is_empty());
    assert_eq!(f.state.undo_depth(), depth + 1);
    assert!(f.verb("undo"));
    assert_eq!(f.state.timeline.markers.len(), 2, "one undo brings both back");
    f.last_error = None;
    assert!(f.verb("redo"));
    let depth = f.state.undo_depth();
    assert!(f.on(&bandy::SMessage::BrowserKey("Alt+m".into())));
    assert_eq!(f.last_error.take().as_deref(), Some("unmark all: no markers"));
    assert_eq!(f.state.undo_depth(), depth, "no undo step");
    assert!(help_rows().iter().any(|(k, p)| k == "Alt+m" && *p == help_phrase("unmark all") && *p == "remove every marker"));
    println!("striamarkersmore: Alt+m -> unmarked 2 markers; none -> refused; help row Alt+m -> {}", help_phrase("unmark all"));
}

// ---- STRIAAUTOSAVEMORE (LEDGER SR972): the kept autosave offered at open (named tail block) ----
// SR913's kept `<stem>.autosave.1.edl` newer than the document is offered at open on SR883's message row, naming
// `recover 1`; with the autosave also newer, both are offered on one line; an older kept file is not offered, and a
// standing message is never replaced.

/// A document of `tl0` dated 60 s past, with `tl0` split at 15 written as the autosave (`auto`) and / or the kept file
/// (`kept`) now: the timeline, the sources, the document path, the split timeline.
fn striaautosavemore_doc(name: &str, auto: bool, kept: bool) -> (Timeline, Sources, std::path::PathBuf, Timeline) {
    let (tl0, src) = synth_fixture();
    let doc = common::fresh_dir("striaautosavemore", name).join("cut.edl");
    std::fs::write(&doc, edl::write(&tl0, "cut").unwrap()).unwrap();
    let tl1 = tl0.split(0, 15).unwrap();
    if auto {
        std::fs::write(autosave_path(&doc), edl::write(&tl1, "cut").unwrap()).unwrap();
    }
    if kept {
        std::fs::write(autosave_keep_path(&doc), edl::write(&tl1, "cut").unwrap()).unwrap();
    }
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    std::fs::File::options().write(true).open(&doc).unwrap().set_modified(past).unwrap();
    (tl0, src, doc, tl1)
}

#[test]
fn striaautosavemore_kept_newer_is_offered() {
    let (tl0, src, doc, tl1) = striaautosavemore_doc("kept", false, true);
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(doc.as_path()));
    let offer = "autosave: cut.autosave.1.edl is newer than cut.edl; the word recover 1 loads it";
    assert_eq!(face.last_error.take().as_deref(), Some(offer), "the kept file is offered at open");
    assert_eq!(face.state.timeline, tl0, "nothing is applied without the word");
    assert_eq!(face.state.undo_depth(), 0);
    assert!(face.verb("recover 1"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl1, "recover 1 applies the offered file");
    assert_eq!(face.state.undo_depth(), 1, "one undo step");
    assert!(face.dirty());
    println!("striaautosavemore: offered \"{offer}\", recovered in one step");
}

#[test]
fn striaautosavemore_both_offered_on_one_line() {
    let (tl0, src, doc, tl1) = striaautosavemore_doc("both", true, true);
    let mut face = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(doc.as_path()));
    let offer = "autosave: cut.autosave.edl is newer than cut.edl; the word recover loads it; cut.autosave.1.edl too: the word recover 1 loads it";
    assert_eq!(face.last_error.take().as_deref(), Some(offer), "both offered on one message-row line");
    assert!(face.verb("recover"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl1, "bare recover still loads the autosave");
    assert_eq!(face.autosaved(), Some(&tl1));
}

#[test]
fn striaautosavemore_older_kept_not_offered() {
    let (tl0, src, doc, _) = striaautosavemore_doc("older", false, true);
    let older = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
    std::fs::File::options().write(true).open(autosave_keep_path(&doc)).unwrap().set_modified(older).unwrap();
    let face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(doc.as_path()));
    assert!(face.last_error.is_none(), "a kept file older than the document is not offered: {:?}", face.last_error);
    let (tl0, src, doc, _) = striaautosavemore_doc("standing", false, true);
    let mut face = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false);
    face.last_error = Some("an earlier message".into());
    let face = face.with_doc(Some(doc.as_path()));
    assert_eq!(face.last_error.as_deref(), Some("an earlier message"), "the offer never replaces a standing message");
}

// ---- STRIAEXPORTQUEUEMORE (LEDGER SR971): the queue's range / audio on the list, the Esc and running counts (named tail block) ----

#[test]
fn striaexportqueuemore_list_names_range_and_audio() {
    let (a, b, c) = (nlefaceexport_dir("more-a"), nlefaceexport_dir("more-b"), nlefaceexport_dir("more-c"));
    let mut face = striaexportqueue_paused(&a);
    face.state.seek(5);
    assert!(face.verb("in"));
    face.state.seek(15);
    assert!(face.verb("out"));
    assert!(face.verb(&format!("export {} png", b.display())));
    assert!(face.verb(&format!("export --all {} h264 none", c.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (2 waiting)"));
    assert!(face.verb("export-list"));
    let want = format!("export: 2 waiting: {} png ticks 5..15, {} h264 none", striaexportqueuelist_name(&b), striaexportqueuelist_name(&c));
    assert_eq!(face.last_error.take(), Some(want));
    assert_eq!(face.export_queued(), 2, "the list leaves the queue");
    assert!(face.cancel_export() && face.wait_export());
    assert!(!b.exists() && !c.exists());
    let _ = std::fs::remove_dir_all(&a);
    println!("striaexportqueuemore: the list names the range and the audio word");
}

#[test]
fn striaexportqueuemore_esc_names_the_dropped() {
    let (a, b, c) = (nlefaceexport_dir("moreesc-a"), nlefaceexport_dir("moreesc-b"), nlefaceexport_dir("moreesc-c"));
    let mut face = striaexportqueue_paused(&a);
    assert!(face.verb(&format!("export {} png", b.display())));
    assert!(face.verb(&format!("export {} png", c.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (2 waiting)"));
    assert!(face.on(&bandy::SMessage::BrowserKey("Escape".into())), "Esc cancels the running export");
    assert_eq!(face.last_error.take().as_deref(), Some("export: cancelling (dropped 2 waiting)"));
    assert_eq!(face.export_queued(), 0);
    assert!(face.wait_export());
    assert_eq!(face.last_error.take(), Some(format!("export: cancelled at 3/35 frames; partial files left in {}", a.display())));
    let _ = std::fs::remove_dir_all(&a);
    let d = nlefaceexport_dir("moreesc-d");
    let mut face = striaexportqueue_paused(&d);
    assert!(face.on(&bandy::SMessage::BrowserKey("Escape".into())));
    assert!(face.last_error.is_none(), "nothing queued: no dropped line {:?}", face.last_error);
    assert!(face.wait_export());
    assert!(!b.exists() && !c.exists(), "the dropped requests made nothing");
    let _ = std::fs::remove_dir_all(&d);
    println!("striaexportqueuemore: Esc named 2 dropped");
}

#[test]
fn striaexportqueuemore_running_line_counts_the_waiting() {
    let (a, b) = (nlefaceexport_dir("morerun-a"), nlefaceexport_dir("morerun-b"));
    let mut face = striaexportqueue_paused(&a);
    let line = face.message_text().expect("the running line");
    assert!(line.starts_with("export: 3 / 35") && !line.contains("waiting"), "{line}");
    assert!(face.verb(&format!("export {} png", b.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (1 waiting)"));
    let line = face.message_text().expect("the running line");
    assert!(line.starts_with("export: 3 / 35") && line.ends_with(" (1 waiting)"), "{line}");
    assert!(face.verb("export-clear"));
    assert_eq!(face.last_error.take().as_deref(), Some("export: dropped 1 waiting"));
    let line = face.message_text().expect("the running line");
    assert!(!line.contains("waiting"), "{line}");
    assert!(face.cancel_export() && face.wait_export());
    assert!(!b.exists());
    let _ = std::fs::remove_dir_all(&a);
    println!("striaexportqueuemore: the running line counts the waiting");
}


// ---- STRIAZOOMFITMORE (LEDGER SR965): the face's layout is timeline_core's ONE anchor rule (named tail block) ----

/// After a fit the face's layout equals `Layout::of_role(..).anchor(the fit's origin, the playhead)` at every playhead:
/// the face and the kernel Player's view band read the same rule.
#[test]
fn striazoomfitmore_face_layout_is_the_anchor() {
    use stria::nle_face::{Layout, Role};
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 1).unwrap();
    let at = face.state.playhead;
    assert!(face.verb("zoom-sel"));
    assert_eq!(face.last_error, None);
    let (w, h) = face.canvas;
    let start = face.state.timeline.tracks[0].clips[1].at;
    for ph in [at, start + 1, 5, 25, tl.end()] {
        face.state.seek(ph);
        let want = Layout::of_role(Role::Whole, w, h, &face.state).anchor(Some((start, at)), face.state.playhead);
        assert_eq!(face.layout(), want, "playhead {ph}");
    }
    println!("striazoomfitmore: face layout == Layout::anchor at 5 playheads");
}

// ---- STRIAFREEZEMORE (LEDGER SR970): `retitle` retypes the selected title through the face (named tail block) ----

/// synth_fixture: `title Hello` at 12, then `retitle Bye` retypes T1 in one undo step and the document keeps the new text; one
/// undo restores "Hello"; a media clip selected is refused by name on the message line.
#[test]
fn striafreezemore_face_retitle_renders_the_new_text() {
    use stria::nle_face::{EditorState, Face, PixelOrder, synth_fixture};
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    face.state.seek(12);
    assert!(face.verb("title Hello"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let d = face.state.undo_depth();
    assert!(face.verb("retitle Bye"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let text = |f: &Face| f.state.timeline.source_by_reel("T1").and_then(|s| s.title.clone());
    assert_eq!((text(&face).as_deref(), face.state.undo_depth()), (Some("Bye"), d + 1));
    let doc = edl::write(&face.state.timeline, "T").unwrap();
    assert!(doc.contains("* UNA TITLE T1 \"Bye\"\n"), "{doc}");
    assert!(face.state.undo());
    assert_eq!(text(&face).as_deref(), Some("Hello"), "one undo restores the old text");
    face.state.select(0, 0).unwrap();
    assert!(face.verb("retitle X"));
    assert_eq!(face.last_error.take().as_deref(), Some("retitle X: no such source"));
    assert_eq!(text(&face).as_deref(), Some("Hello"));
    println!("striafreezemore: title Hello -> retitle Bye -> undo Hello; a media clip -> no such source");
}

// ---- STRIACROPMORE (LEDGER SR960): the crop chords and the crop drag on the preview picture (named tail block) ----

/// The synth fixture's face (320 x 160, labels off: the preview band 0..64), V1's first clip selected at playhead 0, painted
/// once; the preview picture's rect (the still letterboxed into the band) as `(x, y, w, h)` and its vertical middle.
fn striacropmore_face() -> (Face, (f64, f64, f64, f64), f64) {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(0, 0).unwrap();
    let _ = face.paint();
    let s = face.still().expect("a preview still").clone();
    let l = face.layout();
    let q = SizePolicy::Letterbox.place(s.width as usize, s.height as usize, l.w as usize, l.preview_h as usize).dst;
    let ym = (q.y + q.h / 2) as f64;
    (face, (q.x as f64, q.y as f64, q.w as f64, q.h as f64), ym)
}

/// STRIACROPMORE (SR960): a press on the preview picture's left edge begins a crop drag (no timeline drag); a move of a
/// tenth of the picture is l = 100 while live (nothing committed, the dragged rect painted in CROP_BADGE); the release
/// commits ONCE and undo clears it.
#[test]
fn striacropmore_drag_left_edge_commits_once() {
    let (mut face, (qx, _, qw, _), ym) = striacropmore_face();
    let cropless = face.paint();
    let btn = |down: bool, x: f64, y: f64| bandy::SMessage::BrowserButton { down, x, y, mods: 0, surface: 0 };
    assert!(face.on(&btn(true, qx, ym)));
    assert!(face.dragging().is_none());
    assert_eq!(face.crop_dragging().map(|d| d.sides), Some(1));
    let dx = qw / 10.0;
    assert!(face.on(&bandy::SMessage::BrowserMouseMove(qx + dx, ym, 0)));
    assert_eq!(face.crop_dragging().map(|d| d.crop().l), Some(100));
    let live = face.paint();
    assert_ne!(live, cropless);
    assert!(live.as_chunks::<4>().0.contains(&timeline_core::view::CROP_BADGE), "the dragged rect");
    assert_eq!(face.state.undo_depth(), 0, "nothing committed while live");
    assert!(face.on(&btn(false, qx + dx, ym)));
    face.on(&bandy::SMessage::BrowserClick(qx + dx, ym, 0));
    assert!(face.crop_dragging().is_none());
    assert_eq!(face.last_error.take(), None);
    assert_eq!(face.state.selected().map(|c| c.crop), Some(timeline_core::Crop { l: 100, t: 0, r: 0, b: 0 }));
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("undo"));
    assert_eq!(face.state.selected().map(|c| c.crop), Some(timeline_core::Crop::NONE));
    println!("striacropmore: left edge dragged {dx} px -> l=100, one commit, undo clears");
}

/// A press in the middle of the picture is no crop drag (the old road: nothing); Escape drops a live crop drag uncommitted.
#[test]
fn striacropmore_press_off_the_edge_and_escape() {
    let (mut face, (qx, _, qw, _), ym) = striacropmore_face();
    let btn = |down: bool, x: f64, y: f64| bandy::SMessage::BrowserButton { down, x, y, mods: 0, surface: 0 };
    assert!(!face.on(&btn(true, qx + qw / 2.0, ym)));
    assert!(face.crop_dragging().is_none());
    face.on(&btn(false, qx + qw / 2.0, ym));
    assert!(face.on(&btn(true, qx + qw - 1.0, ym)));
    assert_eq!(face.crop_dragging().map(|d| d.sides), Some(4));
    assert!(face.on(&bandy::SMessage::BrowserMouseMove(qx + qw / 2.0, ym, 0)));
    assert!(face.on(&bandy::SMessage::BrowserKey("Escape".into())));
    assert!(face.crop_dragging().is_none());
    assert_eq!((face.state.selected().map(|c| c.crop), face.state.undo_depth()), (Some(timeline_core::Crop::NONE), 0));
}

/// The split face: the preview Surface's press / move / release are the crop drag's (its other pointer messages still
/// refused), committing once.
#[test]
fn striacropmore_split_preview_surface_drags() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).on_surfaces(0, 1);
    face.state.select(0, 0).unwrap();
    let _ = face.frames();
    let s = face.still().expect("a preview still").clone();
    let l = face.layout_for(stria::nle_face::Role::Preview);
    let q = SizePolicy::Letterbox.place(s.width as usize, s.height as usize, l.w as usize, l.preview_h as usize).dst;
    let (qx, ym, dx) = (q.x as f64, (q.y + q.h / 2) as f64, q.w as f64 / 10.0);
    let btn = |down: bool, x: f64| bandy::SMessage::BrowserButton { down, x, y: ym, mods: 0, surface: 0 };
    assert!(!face.on(&btn(true, qx + q.w as f64 / 2.0)), "off the edge: the preview Surface's press is refused as before");
    assert!(face.on(&btn(true, qx)));
    assert!(face.on(&bandy::SMessage::BrowserMouseMove(qx + dx, ym, 0)));
    assert!(face.on(&btn(false, qx + dx)));
    assert_eq!(face.state.selected().map(|c| c.crop.l), Some(100));
    assert_eq!((face.state.undo_depth(), face.last_error.take()), (1, None));
}

/// The eight crop chords are KEYS rows onto SR900's step words (no VERBS word): Alt+l crops the left side in by CROP_STEP,
/// Alt+Shift+L uncrops it (one undo step each); the help names each chord with its phrase; the chord as text is not taken.
#[test]
fn striacropmore_keys_step_the_crop() {
    let chords = [
        ("Alt+l", "crop-l+"),
        ("Alt+Shift+L", "crop-l-"),
        ("Alt+t", "crop-t+"),
        ("Alt+Shift+T", "crop-t-"),
        ("Alt+r", "crop-r+"),
        ("Alt+Shift+R", "crop-r-"),
        ("Alt+b", "crop-b+"),
        ("Alt+Shift+B", "crop-b-"),
    ];
    let rows = help_rows();
    for (k, w) in chords {
        assert!(KEYS.contains(&(k, w)), "{k}");
        assert!(!VERBS.contains(&w), "{w}: a key's word, no button");
        let row = rows.iter().find(|r| r.0.split(" / ").any(|x| x == k)).expect("a help row");
        assert_eq!(row.1, help_phrase(w));
        assert!(row.1 != w, "{w}: a phrase, not the word");
    }
    let (mut face, _, _) = striacropmore_face();
    let s = timeline_core::editor::CROP_STEP;
    assert!(face.on(&bandy::SMessage::BrowserKey("Alt+l".into())));
    assert!(face.on(&bandy::SMessage::BrowserKey("Alt+b".into())));
    assert_eq!(face.state.selected().map(|c| c.crop), Some(timeline_core::Crop { l: s, t: 0, r: 0, b: s }));
    assert!(face.on(&bandy::SMessage::BrowserKey("Alt+Shift+L".into())));
    assert_eq!(face.state.selected().map(|c| c.crop), Some(timeline_core::Crop { l: 0, t: 0, r: 0, b: s }));
    assert_eq!((face.state.undo_depth(), face.last_error.take()), (3, None));
    assert!(!face.on(&bandy::SMessage::BrowserText("Alt+l".into())));
    println!("striacropmore: eight chords, Alt+l Alt+b Alt+Shift+L -> 0/0/0/{s}");
}

// ---- STRIALINKMORE (LEDGER SR967): the face's `alone` word (named tail block) ----
#[test]
fn strialinkmore_face_alone_word() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert_eq!(&VERBS[VERBS.len() - 2..], ["alone", "help"], "alone before help");
    assert!(face.verb("alone"));
    assert_eq!(face.last_error.take().as_deref(), Some("alone: no such clip"), "nothing selected, on the message row");
    face.state.select(0, 2).unwrap();
    face.state.select_add(1, 2).unwrap();
    assert!(face.verb("link"));
    face.state.select(0, 2).unwrap();
    assert_eq!(face.state.set, vec![(0, 2), (1, 2)]);
    let linked = face.state.timeline.clone();
    assert!(face.verb("alone"));
    assert_eq!(face.last_error.take(), None);
    assert_eq!((face.state.selection, face.state.set.clone()), (Some((0, 2)), vec![(0, 2)]));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&linked, 1), "link only: alone is no edit");
    println!("strialinkmore: VERBS[{}] alone; set {:?}", VERBS.len() - 2, face.state.set);
}

// ---- STRIAUNDOMORE (LEDGER SR973): pick an undo step on the history line (named tail block) ----

/// SR934's three named commits (split, delete, slip+) on the synth face, labels on or off: (the face, the post-split
/// timeline). The history line then reads `history: 3 slip+ | 2 delete | 1 split`.
fn striaundomore_face(labels: bool) -> (stria::nle_face::Face, Timeline) {
    use stria::nle_face::{EditorState, Face, PixelOrder};
    let (tl, src) = stria::nle_face::synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 144), PixelOrder::Rgba, labels);
    face.state.select(0, 1).unwrap();
    face.state.seek(15);
    assert!(face.verb("split"));
    let split = face.state.timeline.clone();
    assert!(face.verb("delete"));
    face.state.select(0, 0).unwrap();
    assert!(face.verb("slip+"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.undo_depth(), 3);
    (face, split)
}

/// The `history` word through `Face::on`, its `last_error` taken as `serve` takes it: the line it showed.
fn striaundomore_history(face: &mut stria::nle_face::Face) -> String {
    assert!(face.on(&bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: "history".into() }));
    let line = face.last_error.take().expect("the history line");
    assert_eq!(line, "history: 3 slip+ | 2 delete | 1 split");
    line
}

/// The point on the message row 2 px into byte `i` of `line` (painted at x 4 with the face's own labels style).
fn striaundomore_at(face: &stria::nle_face::Face, line: &str, i: usize) -> (f64, f64) {
    let l = face.layout().with_message(true);
    assert_eq!(l.message_h, stria::nle_face::MESSAGE_H, "the 320x144 face has a message row");
    let style = text_host::line::TextStyle::new(&["sans"], 11.0);
    let x = 4.0 + style.shape(line).caret_x(line, i) as f64 + 2.0;
    (x, (l.message_y + l.message_h / 2) as f64)
}

fn striaundomore_press(face: &mut stria::nle_face::Face, x: f64, y: f64) -> bool {
    face.on(&bandy::SMessage::BrowserButton { down: true, x, y, mods: 0, surface: 0 })
}

/// Rule: the history line stays on the message row after `last_error` is taken (the note); a move keeps it; a press on
/// the entry `2 delete` runs SR934's `undo 2`, back to before "delete", named on the message line, and the note is gone.
#[test]
fn striaundomore_pick_lands_before_the_entry() {
    let (mut face, split) = striaundomore_face(true);
    let line = striaundomore_history(&mut face);
    assert_eq!(face.message_text().as_deref(), Some(line.as_str()), "the note keeps the line on the row");
    let (x, y) = striaundomore_at(&face, &line, line.find("2 delete").unwrap());
    face.on(&bandy::SMessage::BrowserMouseMove(x, y, 0));
    assert_eq!(face.message_text().as_deref(), Some(line.as_str()), "a move keeps the note");
    assert!(striaundomore_press(&mut face, x, y));
    let landed = face.last_error.take();
    assert_eq!(landed.as_deref(), Some("undo 2: back to before \"delete\""));
    assert_eq!((face.state.undo_depth(), face.state.redo_depth()), (1, 2));
    assert_eq!(face.state.timeline, split);
    assert!(face.message_text().is_none_or(|m| !m.starts_with("history:")), "the pick clears the note");
    println!("striaundomore: pick at x {x:.1} -> {landed:?}");
}

/// Rule: a press on the row off any entry (the `history: ` prefix) is taken and moves nothing; an error wins over the note;
/// an Input word clears it; labels off there is no row, and a press clears the note and runs as before.
#[test]
fn striaundomore_pick_misses_and_clears() {
    let (mut face, _split) = striaundomore_face(true);
    let line = striaundomore_history(&mut face);
    let top = face.state.timeline.clone();
    let (x, y) = striaundomore_at(&face, &line, 2);
    assert!(!striaundomore_press(&mut face, x, y), "on the prefix: taken, nothing to repaint");
    assert_eq!((face.state.undo_depth(), face.state.timeline == top), (3, true));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.message_text().as_deref(), Some(line.as_str()), "the note stands");
    face.last_error = Some("split: no such clip".into());
    assert_eq!(face.message_text().as_deref(), Some("split: no such clip"), "an error wins over the note");
    face.last_error = None;
    assert!(face.on(&bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: "zoom+".into() }));
    face.last_error = None;
    assert!(face.message_text().is_none_or(|m| !m.starts_with("history:")), "an input clears the note");

    let (mut off, _split) = striaundomore_face(false);
    assert!(off.verb("history"));
    off.last_error = None;
    striaundomore_press(&mut off, 40.0, 60.0);
    off.last_error = None;
    assert_eq!(off.state.undo_depth(), 3, "no row: nothing is picked");
    assert!(off.message_text().is_none_or(|m| !m.starts_with("history:")), "the press clears the note");
}

// ---- STRIAGAPSMORE (LEDGER SR969): the Close All button word and `close-gaps sync` on the host face (named tail block) ----

/// `close-all` sits right before `gap` in VERBS and is the argument word `close-gaps all` (one commit, the timeline_core op by
/// hand). On the synth fixture (V1 and A1 both A@0|B@10|A@25) with a to-black at 20 and a from-black at 25 on V1,
/// `close-gaps sync` with nothing selected closes the one span empty on both tracks (20..25), removes both of V1's fades in
/// ONE commit and names them.
#[test]
fn striagapsmore_face_close_all_and_sync() {
    let i = VERBS.iter().position(|&v| v == "close-all").expect("close-all in VERBS");
    assert_eq!(VERBS[i + 1], "gap");
    let (tl, src) = synth_fixture();
    let tl = tl.add_transition_to_black(0, 20, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let tl = tl.add_transition_from_black(0, 25, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("close-all"));
    assert_eq!(face.state.timeline, tl.close_gaps_all().unwrap());
    assert_eq!(face.state.undo_depth(), 1);
    let (_, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert_eq!(tl.sync_gaps(), vec![(20, 25)]);
    assert!(face.verb("close-gaps sync"));
    assert_eq!(face.note.take().as_deref(), Some("close-gaps sync: closed 1 gap, removed 2 fades")); // STRIAGAPSTHREE (SR9028): the success line is a face note, not an error
    assert_eq!(face.state.timeline, tl.close_gaps_sync().unwrap());
    assert_eq!((face.state.timeline.tracks[0].clips[2].at, face.state.timeline.tracks[1].clips[2].at), (20, 20));
    assert_eq!(face.state.undo_depth(), 1);
    println!("striagapsmore: V1 {:?} A1 {:?}", face.state.timeline.tracks[0].clips, face.state.timeline.tracks[1].clips);
}
// ---- STRIAXFORMMORE (LEDGER SR975): the face's nudge words and the placement line (named tail block) ----

/// `pip left` (a timeline_core nudge word, no VERBS change) moves the selected clip's rect one step, one undo step; the
/// message row reads the placement with no error (SR938's note: any `pip` word); a nudge past the edge is refused by name
/// on the message row with no undo step.
#[test]
fn striaxformmore_face_nudge_note() {
    use stria::nle_face::{EditorState, Face, PixelOrder, synth_fixture};
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("pip tl"));
    assert!(face.verb("pip left"));
    assert_eq!(face.last_error.take(), None);
    assert_eq!(face.message_text().as_deref(), Some("pip: x 0 y 25 scale 250"));
    assert_eq!(face.state.undo_depth(), 2);
    assert!(face.verb("pip left"));
    assert_eq!(face.last_error.take(), Some(format!("pip left: {}", timeline_core::EditError::Xform)));
    assert_eq!(face.state.undo_depth(), 2);
}

// ---- STRIAPROXYMORE (LEDGER SR979): purge all, an Adopt reel's proxy, the proxy's audio (named tail block) ----

/// `proxy purge all` detaches every reel and removes each proxy file and stamp; again: nothing to remove. View-only.
#[test]
fn striaproxymore_purge_all_removes_every_reel() {
    let (dir, pa, pb, mut face, tl) = striaproxyoff_face("purgeall");
    let (a, b) = (tl.sources[0].id, tl.sources[1].id);
    let ((xa, sa, wa), (xb, sb, wb)) = (stria::nle_face::proxy_paths(&pa), stria::nle_face::proxy_paths(&pb));
    assert!(face.verb("proxy all") && face.wait_proxy() && face.wait_proxy());
    assert!(face.has_proxy(a) && face.has_proxy(b) && xa.is_file() && xb.is_file());
    let _ = face.last_error.take();
    assert!(face.verb("proxy purge all"));
    assert_eq!(face.last_error.take(), Some(format!("proxy: purge all: A: off, removed {}, {}; B: off, removed {}, {}", xa.display(), sa.display(), xb.display(), sb.display())));
    assert!(!face.has_proxy(a) && !face.has_proxy(b));
    assert!(!xa.exists() && !sa.exists() && !wa.exists() && !xb.exists() && !sb.exists() && !wb.exists());
    assert!(pa.is_file() && pb.is_file(), "the sources stay");
    assert!(face.verb("proxy purge all"));
    assert_eq!(face.last_error.take().as_deref(), Some("proxy: purge all: A: no proxy files; B: no proxy files"));
    assert!(face.proxy_reels().is_empty());
    assert_eq!(face.state.timeline, tl, "view-only");
    assert!(!face.dirty());
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxymore purge all: A and B detached, their proxies and stamps removed");
}

/// `proxy purge all` skips the source whose job runs (its work dir untouched) and purges the rest; `proxy purge-all` is the
/// same word; without paths every reel says so.
#[test]
fn striaproxymore_purge_all_skips_the_running_job() {
    let (dir, pa, pb, mut face, tl) = striaproxyoff_face("purgeallbusy");
    let b = tl.sources[1].id;
    let ((_, _, wa), (xb, sb, _)) = (stria::nle_face::proxy_paths(&pa), stria::nle_face::proxy_paths(&pb));
    face.state.select(0, 1).unwrap();
    assert!(face.verb("proxy") && face.wait_proxy());
    assert!(face.has_proxy(b));
    face.hold_proxy_at(Some(3));
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy") && face.proxying());
    let _ = face.last_error.take();
    assert!(face.verb("proxy purge-all"));
    let line = face.last_error.take().unwrap_or_default();
    assert!(line.starts_with("proxy: purge all: A: busy ("), "{line}");
    assert!(line.ends_with(&format!("): Esc cancels the build; B: off, removed {}, {}", xb.display(), sb.display())), "{line}");
    assert!(wa.is_dir() && face.proxying(), "A's job and its work dir untouched");
    assert!(!face.has_proxy(b) && !xb.exists() && !sb.exists());
    assert!(face.cancel_proxy() && face.wait_proxy());
    let _ = face.last_error.take();
    let (t2, s2) = synth_fixture();
    let mut bare = Face::new(EditorState::new(t2), s2, (320, 160), PixelOrder::Rgba, false);
    for word in ["proxy purge all", "proxy purge-all"] {
        assert!(bare.verb(word));
        assert_eq!(bare.last_error.take().as_deref(), Some("proxy: purge all: A: no source path; B: no source path"), "{word}");
    }
    assert!(!bare.dirty());
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxymore purge all busy: {line}");
}

/// An Adopt reel's half-size proxy previews at the reel's size (the Letterbox up-fit), not the proxy's; an Adopt reel
/// without a proxy previews as before.
#[test]
fn striaproxymore_adopt_reel_proxy_fits_the_reel() {
    use stria::nle_face::preview;
    let (dir, pa, _, mut face, tl) = striaproxyoff_face("adopt");
    let a = tl.sources[0].id;
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy") && face.wait_proxy());
    let bytes = std::fs::read(stria::nle_face::proxy_paths(&pa).0).unwrap();
    let (t2, s2) = synth_fixture();
    let adopt = t2.set_size_policy(a, SizePolicy::Adopt).unwrap();
    let plain = preview(&adopt, &s2, 3).unwrap().expect("A's frame at 3");
    assert_eq!((plain.width, plain.height), (64, 48));
    assert!(plain.rgba == preview(&t2, &s2, 3).unwrap().expect("A's frame at 3").rgba, "no proxy: Adopt unchanged");
    s2.set_proxy(a, bytes).unwrap();
    let shown = preview(&adopt, &s2, 3).unwrap().expect("the proxy's frame at 3");
    let boxed = preview(&t2, &s2, 3).unwrap().expect("the proxy's frame at 3");
    assert_eq!((shown.width, shown.height), (64, 48), "the reel's size, not the proxy's 32x24");
    assert!(shown.rgba == boxed.rgba, "Adopt fits the proxy as Letterbox does");
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxymore adopt: proxied preview {}x{}", shown.width, shown.height);
}

/// A preview-sink render (`proxy() == true`) reads an attached proxy's video but never its audio: the PCM is the original's.
#[test]
fn striaproxymore_proxy_leaves_the_audio_original() {
    #[derive(Default)]
    struct ProxySink {
        frames: Vec<Vec<u8>>,
        pcm: Vec<f32>,
    }
    impl RenderSink for ProxySink {
        fn frame(&mut self, _out_t: i64, _w: u32, _h: u32, rgba: &[u8]) {
            self.frames.push(rgba.to_vec());
        }
        fn pcm(&mut self, _out_t: i64, _rate: u32, _ch: u16, s: &[f32]) {
            self.pcm.extend_from_slice(s);
        }
        fn proxy(&self) -> bool {
            true
        }
    }
    let (tl, src) = synth_fixture();
    let a = tl.sources[0].id;
    let mut before = ProxySink::default();
    render(&tl, 0..tl.end(), &src, &mut before).unwrap();
    src.set_proxy(a, synth_source(5000, 50, 25)).unwrap();
    let mut after = ProxySink::default();
    render(&tl, 0..tl.end(), &src, &mut after).unwrap();
    assert!(after.frames != before.frames, "the video reads the proxy");
    assert!(!before.pcm.is_empty());
    assert!(after.pcm == before.pcm, "the audio stays the original's");
    println!("striaproxymore audio: {} samples unchanged under a proxy", after.pcm.len());
}

// ---- STRIAWAVVALIDBITSNEXT (LEDGER SR982): RF64 / BW64 ds64 table sizes (named tail block) ----

/// STRIAWAVVALIDBITSNEXT (SR982): a mono 48 kHz PCM16 `magic` (RF64 / BW64) file: a `ds64` chunk first carrying `table`
/// (chunkId, chunkSize) entries and dataSize = the payload, `fmt `, a `JUNK` chunk whose size field is 0xFFFFFFFF with
/// `junk_len` bytes, then `data` (size field 0xFFFFFFFF) and the payload.
fn striawavvalidbitsnext_rf64(magic: &[u8; 4], table: &[([u8; 4], u64)], junk_len: usize, payload: &[u8]) -> Vec<u8> {
    let ds64_len = 28 + 12 * table.len() as u32;
    let mut w = Vec::new();
    w.extend_from_slice(magic);
    w.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"ds64");
    w.extend_from_slice(&ds64_len.to_le_bytes());
    w.extend_from_slice(&(4 + 8 + ds64_len as u64 + 24 + 8 + junk_len as u64 + 8 + payload.len() as u64).to_le_bytes());
    w.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    w.extend_from_slice(&(payload.len() as u64 / 2).to_le_bytes());
    w.extend_from_slice(&(table.len() as u32).to_le_bytes());
    for (id, n) in table {
        w.extend_from_slice(id);
        w.extend_from_slice(&n.to_le_bytes());
    }
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&48_000u32.to_le_bytes());
    w.extend_from_slice(&96_000u32.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"JUNK");
    w.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    w.extend(std::iter::repeat(0u8).take(junk_len));
    w.extend_from_slice(b"data");
    w.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    w.extend_from_slice(payload);
    w
}

#[test]
fn striawavvalidbitsnext_rf64_table_sizes_a_chunk() {
    let payload = striaimportwavwide_payload(16, false, 96_000);
    for magic in [b"RF64", b"BW64"] {
        let wav = striawavvalidbitsnext_rf64(magic, &[(*b"JUNK", 6)], 6, &payload);
        let d = gneiss_pal::dsp::demux::Demuxer::open_wav(wav.clone()).expect("open_wav");
        assert_eq!(d.samples().iter().map(|s| s.duration).sum::<u64>(), 96_000, "{magic:?}");
        let (src, s) = striaimportwav_render(wav, (0, 50), 50);
        assert_eq!(timeline_core::SourceIndex::duration(&src, SourceId(0)), Some(50));
        assert_eq!(s.pcm.len(), 96_000);
        for (j, got) in s.pcm.iter().enumerate() {
            assert_eq!(*got, q(striaimportwav_sample(j) as i32), "{magic:?} sample {j}");
        }
    }
}

#[test]
fn striawavvalidbitsnext_rf64_table_refusal_by_name() {
    let refusal = |bytes: Vec<u8>| {
        let tl = Timeline::new(25);
        let mut src = Sources::new(&tl);
        src.add(SourceId(0), bytes).expect_err("refused").to_string()
    };
    let payload = striaimportwavwide_payload(16, false, 2400);
    // No table entry at all, and an entry for another chunk id: the JUNK chunk's 0xFFFFFFFF size is not given.
    let tables: [&[([u8; 4], u64)]; 2] = [&[], &[(*b"LIST", 6)]];
    for table in tables {
        let e = refusal(striawavvalidbitsnext_rf64(b"RF64", table, 6, &payload));
        assert!(e.contains("wav: RF64 chunk size not in the ds64 table"), "{e}");
    }
}

// ---- STRIAHELPMORE (LEDGER SR977): the help query field on the bar and drawn on the band (named tail block) ----

fn striahelpmore_field(f: &mut Face, q: &str) -> bool {
    f.on(&bandy::SMessage::ConsoleInput(q.into()))
}

/// The bar field's Enter is `help <q>`: `slip` shows the filtered help as `help slip` does; `zzz` is refused by name and
/// the help left as it was; a blank Enter is the bare `help` toggle (hides it, clears the query); view-only.
#[test]
fn striahelpmore_field_enter_filters() {
    let mut f = nle5dkeyshelp_face(640, 480, false);
    let (tl, ph, depth) = (f.state.timeline.clone(), f.state.playhead, f.state.undo_depth());
    assert!(striahelpmore_field(&mut f, "slip"));
    assert_eq!((f.help_shown(), f.help_query()), (true, Some("slip")));
    assert_eq!(f.help_rows_shown(), stria::nle_face::help_matching("slip"));
    assert!(f.last_error.is_none(), "{:?}", f.last_error);
    assert!(striahelpmore_field(&mut f, "zzz"));
    assert_eq!(f.last_error.take().as_deref(), Some("help: no key matches \"zzz\""));
    assert_eq!((f.help_shown(), f.help_query()), (true, Some("slip")));
    assert!(striahelpmore_field(&mut f, "  "));
    assert_eq!((f.help_shown(), f.help_query()), (false, None));
    assert!(striahelpmore_field(&mut f, ""));
    assert_eq!((f.help_shown(), f.help_query()), (true, None));
    assert_eq!((&f.state.timeline, f.state.playhead, f.state.undo_depth(), f.dirty()), (&tl, ph, depth, false));
    println!("striahelpmore: field slip -> {} rows, zzz refused, blank toggles", f.help_rows_shown().len());
}

/// While a query is set the band's first line reads `find: <q>_`: `help slip` and `help SLIP` show the same rows and their
/// frames differ (only below the ruler) by that line; the bare help paints as a fresh face's; labels off byte-identical.
#[test]
fn striahelpmore_query_drawn_on_the_band() {
    let mut lower = nle5dkeyshelp_face(640, 480, true);
    assert!(lower.verb("help slip"));
    assert_eq!(lower.help_query_text().as_deref(), Some("find: slip_"));
    let mut upper = nle5dkeyshelp_face(640, 480, true);
    assert!(upper.verb("help SLIP"));
    assert_eq!(upper.help_rows_shown(), lower.help_rows_shown());
    let (a, b) = (lower.paint(), upper.paint());
    let cut = lower.layout().ruler_y as usize * 640 * 4;
    assert_eq!(a[..cut], b[..cut], "the preview band is untouched");
    assert_ne!(a[cut..], b[cut..], "the query line differs");
    let mut fresh = nle5dkeyshelp_face(640, 480, true);
    assert!(fresh.verb("help"));
    assert_eq!(fresh.help_query_text(), None);
    let bare = fresh.paint();
    assert!(lower.verb("help"));
    assert!(lower.verb("help"));
    assert_eq!(lower.paint(), bare, "no query: no line, the whole table as before");
    let mut plain = nle5dkeyshelp_face(640, 480, false);
    let p = plain.paint();
    assert!(plain.verb("help slip"));
    assert_eq!(plain.paint(), p, "labels off: no overlay");
    println!("striahelpmore: find: slip_ fnv {:016x}", stria::nle_face::fnv1a(&a[cut..]));
}

/// The typed query is drawn as typed: `sl` then Backspace reads `find: s_` and paints as `help s`; Backspace to empty
/// draws no line and paints the bare help.
#[test]
fn striahelpmore_typed_query_drawn() {
    let mut f = nle5dkeyshelp_face(640, 480, true);
    assert!(f.verb("help"));
    let bare = f.paint();
    striahelpsearchlive_type(&mut f, "sl");
    assert_eq!(f.help_query_text().as_deref(), Some("find: sl_"));
    assert!(striahelpsearchlive_backspace(&mut f));
    assert_eq!(f.help_query_text().as_deref(), Some("find: s_"));
    let mut word = nle5dkeyshelp_face(640, 480, true);
    assert!(word.verb("help s"));
    assert_eq!(f.paint(), word.paint());
    assert!(striahelpsearchlive_backspace(&mut f));
    assert_eq!((f.help_shown(), f.help_query_text()), (true, None));
    assert_eq!(f.paint(), bare);
    println!("striahelpmore: typed sl, Backspace -> find: s_, Backspace -> bare");
}

// ---- STRIATRACKGAINKEYSNEXT (LEDGER SR983): `mute <ch>` / `solo <ch>` on the face with no selection (named tail block) ----
/// M3: the face takes `solo A1` / `mute V1` with nothing selected (timeline_core's verb, no line), one undo step each, the
/// EDL carries the flags; a channel naming no track is refused by name on the message line, the state kept.
#[test]
fn striatrackgainkeysnext_face_mute_by_channel() {
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("solo A1"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.verb("mute V1"));
    assert_eq!((face.state.timeline.tracks[1].solo, face.state.timeline.tracks[0].muted), (true, true));
    assert_eq!((face.state.undo_depth(), face.dirty(), face.state.selection), (2, true, None));
    let text = edl::write(&face.state.timeline, "CH").unwrap();
    assert!(text.contains("\n* UNA MUTE V\n* UNA SOLO A\n\n"), "{text}");
    let at = face.state.timeline.clone();
    assert!(face.verb("mute A9"));
    assert_eq!(face.last_error.take(), Some(format!("mute A9: {}", timeline_core::EditError::NoTrack)));
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&at, 2));
    assert!(face.verb("undo") && face.verb("undo"));
    assert_eq!((&face.state.timeline, face.dirty()), (&tl0, false));
    println!("striatrackgainkeysnext face: solo A1 + mute V1 with no selection, two undos restore; mute A9 -> {}", timeline_core::EditError::NoTrack);
}

// ---- STRIASEQMORE (LEDGER SR978): sequence formats, fractional rates, the title size (named tail block) ----
/// STRIASEQMORE: a `w` x `h` 24-bit bottom-up BMP (BITMAPINFOHEADER, BI_RGB) of one colour `c`.
fn striaseqmore_bmp(w: u32, h: u32, c: [u8; 3]) -> Vec<u8> {
    let stride = (w as usize * 3).div_ceil(4) * 4;
    let size = 54 + stride * h as usize;
    let mut b = Vec::with_capacity(size);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&(size as u32).to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&((stride * h as usize) as u32).to_le_bytes());
    b.extend_from_slice(&2835u32.to_le_bytes());
    b.extend_from_slice(&2835u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..h {
        let mut row: Vec<u8> = (0..w).flat_map(|_| [c[2], c[1], c[0]]).collect();
        row.resize(stride, 0);
        b.extend_from_slice(&row);
    }
    b
}

#[test]
fn striaseqmore_bmp_sequence_renders_each_file() {
    let d = common::fresh_dir("striaseqmore", "bmp");
    let cols = [[200, 10, 10], [10, 200, 10], [10, 10, 200]];
    for (i, c) in cols.iter().enumerate() {
        std::fs::write(d.join(format!("shot_{:04}.bmp", i + 1)), striaseqmore_bmp(4, 4, *c)).unwrap();
    }
    let pat = format!("{}/shot_%04d.bmp", d.display());
    let (tl, s) = striaimportimagesequence_tl(3);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    assert_eq!(src.summary(s).as_deref(), Some("seq 1..=3 4x4"));
    assert_eq!(timeline_core::SourceIndex::duration(&src, s), Some(3));
    let mut sink = StriaTitleFrames::default();
    render(&tl, 0..3, &src, &mut sink).unwrap();
    assert_eq!(sink.0.len(), 3);
    for (t, c) in cols.iter().enumerate() {
        assert_eq!((sink.0[t].1, sink.0[t].2), (4, 4));
        assert!(sink.0[t].3 == striaimportimagesequence_px(4, 4, *c), "tick {t}: shot_{:04}.bmp", t + 1);
    }
    println!("striaseqmore: shot_0001..0003.bmp -> 3 ticks, each file's pixels");
}

#[test]
fn striaseqmore_format_mismatch_refused_by_name() {
    let pat = striaimportimagesequence_dir("seqmore-mismatch", 4, 4, &[(1, [9, 9, 9])]);
    let dir = std::path::Path::new(&pat).parent().unwrap().to_path_buf();
    std::fs::copy(dir.join("frame_0001.png"), dir.join("fake_0001.bmp")).unwrap();
    let (tl, s) = striaimportimagesequence_tl(1);
    let mut src = Sources::new(&tl);
    let fake = format!("{}/fake_%04d.bmp", dir.display());
    let e = src.add_sequence(s, &fake).unwrap_err().to_string();
    assert!(e.contains("fake_0001.bmp: not a BMP header"), "{e}");
    let tif = format!("{}/frame_%04d.tif", dir.display());
    let e = src.add_sequence(s, &tif).unwrap_err().to_string();
    assert!(e.contains(&format!("image sequence {tif}: only .png")), "{e}");
    assert!(src.add_sequence(s, &pat).is_ok(), "the PNG pattern still opens");
}

#[test]
fn striaseqmore_fractional_rate_maps_ticks() {
    let cols = [[200, 10, 10], [10, 200, 10], [10, 10, 200]];
    let pat = striaimportimagesequence_dir("seqmore-frac", 4, 4, &[(1, cols[0]), (2, cols[1]), (3, cols[2])]);
    for r in ["25/2", "12.5", "50/4"] {
        let (tl, s) = striaimportimagesequence_tl(6);
        let mut src = Sources::new(&tl);
        src.add_sequence(s, &format!("{pat}@{r}")).unwrap();
        assert_eq!(timeline_core::SourceIndex::duration(&src, s), Some(6), "@{r}: 3 files at 12.5/s on a 25/s timeline");
        assert_eq!(src.summary(s).as_deref(), Some("seq 1..=3 4x4 @25/2"), "@{r} reduces to 25/2");
        let mut sink = StriaTitleFrames::default();
        render(&tl, 0..6, &src, &mut sink).unwrap();
        for (t, file) in [0usize, 0, 1, 1, 2, 2].into_iter().enumerate() {
            assert!(sink.0[t].3 == striaimportimagesequence_px(4, 4, cols[file]), "@{r} tick {t}: file {}", file + 1);
        }
    }
    let many: Vec<(u64, [u8; 3])> = (1..=8u64).map(|n| (n, [n as u8 * 20, 0, 0])).collect();
    let pat = striaimportimagesequence_dir("seqmore-ntsc", 4, 4, &many);
    let (tl, s) = striaimportimagesequence_tl(7);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &format!("{pat}@30000/1001")).unwrap();
    assert_eq!(timeline_core::SourceIndex::duration(&src, s), Some(7), "ceil(8 * 25 * 1001 / 30000)");
    assert_eq!(src.summary(s).as_deref(), Some("seq 1..=8 4x4 @30000/1001"));
    let mut sink = StriaTitleFrames::default();
    render(&tl, 0..7, &src, &mut sink).unwrap();
    for (t, file) in [0usize, 1, 2, 3, 4, 5, 7].into_iter().enumerate() {
        assert!(sink.0[t].3 == striaimportimagesequence_px(4, 4, many[file].1), "tick {t}: file {}", file + 1);
    }
    println!("striaseqmore: @25/2 holds each file 2 ticks; @30000/1001 skips file 7 at tick 6");
}

#[test]
fn striaseqmore_fractional_rate_refused_by_name() {
    let pat = striaimportimagesequence_dir("seqmore-refuse", 4, 4, &[(1, [9, 9, 9])]);
    let (tl, s) = striaimportimagesequence_tl(1);
    let mut src = Sources::new(&tl);
    for r in ["0/1", "1/0", "1/", "/2", ".5", "12.", "1.2.3", "x/2", "0.0", "1/2/3", "1.0000000001"] {
        let p = format!("{pat}@{r}");
        let e = src.add_sequence(s, &p).unwrap_err().to_string();
        assert!(e.contains(&format!("image sequence {p}: rate \"{r}\"")), "{e}");
    }
}

#[test]
fn striaseqmore_title_only_range_takes_sequence_size() {
    let pat = striaimportimagesequence_dir("seqmore-title", 16, 8, &[(1, [1, 2, 3]), (2, [4, 5, 6])]);
    let mut tl = Timeline::new(25);
    let q = tl.add_source("SEQ", 2);
    let t = tl.add_source("T1", 2);
    tl.sources[t.0 as usize].title = Some("Hello".to_string());
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(t, 0, 2, 0)];
    let mut src = Sources::new(&tl);
    src.add_sequence(q, &pat).unwrap();
    let mut sink = StriaTitleFrames::default();
    let r = render(&tl, 0..2, &src, &mut sink).unwrap();
    assert_eq!((r.width, r.height), (16, 8), "the sequence reel's size, not TITLE_SIZE");
    assert!(sink.0.iter().all(|f| (f.1, f.2) == (16, 8)));
    println!("striaseqmore: a title-only range beside a 16x8 sequence reel renders at 16x8");
}

// ---- STRIASNAPNEXT (LEDGER SR990): the snap line is a face note held until the next acting input (named tail block) ----

/// Rule: the `snap` word's line is `Face.note`, not `last_error`: `message_text` names it; a pointer move and a zoom keep
/// it; an error standing beside it wins the message row; the next acting input (a typed word) clears it.
#[test]
fn striasnapnext_snap_line_is_a_note_held_until_the_next_input() {
    let snap = || bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: "snap".into() };
    let mut f = nle5dkeyshelp_face(640, 480, false);
    assert_eq!(f.note, None);
    assert!(f.on(&snap()));
    assert!(!f.snap_on());
    assert_eq!((f.note.as_deref(), f.last_error.as_deref()), (Some("snap: off"), None));
    assert_eq!(f.message_text().as_deref(), Some("snap: off"));
    f.on(&bandy::SMessage::BrowserMouseMove(5.0, 5.0, 0));
    f.on(&bandy::SMessage::BrowserZoom(1));
    assert_eq!(f.note.as_deref(), Some("snap: off"), "a move and a zoom keep the note");
    f.last_error = Some("drag: clips overlap".into());
    assert_eq!(f.message_text().as_deref(), Some("drag: clips overlap"), "an error wins the row");
    f.last_error = None;
    assert!(f.on(&snap()));
    assert_eq!(f.note.as_deref(), Some("snap: on"), "the word's own input clears, then names the new state");
    assert!(f.on(&bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: "zoom+".into() }));
    assert_eq!(f.note, None, "the next acting input clears it");
    assert_eq!((f.state.undo_depth(), f.dirty()), (0, false));
    println!("striasnapnext: snap -> note snap: off held over a move and a zoom; snap -> snap: on; zoom+ clears");
}

// ---- STRIAXFADEMORE (LEDGER SR980): the face's note, a success reading that is not an error (named tail block) ----

/// `c` puts the crossfade: the reading is the face's note (last_error stays None) and the message row shows it; a pointer
/// move keeps it, the next typed word clears it; a refusal is still last_error.
#[test]
fn striaxfademore_face_note_is_not_an_error() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select(1, 1).unwrap();
    assert!(nle5drangekeys_press(&mut face, "c"));
    assert_eq!(face.last_error, None);
    assert_eq!(face.note.as_deref(), Some("xfade: 10 equal"));
    assert_eq!(face.message_text().as_deref(), Some("xfade: 10 equal"));
    face.on(&bandy::SMessage::BrowserMouseMove(1.0, 1.0, 0));
    assert_eq!(face.note.as_deref(), Some("xfade: 10 equal"), "a pointer move keeps the note");
    face.on(&bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: "undo".into() });
    assert_eq!(face.note, None, "the next typed word clears it");
    face.state.select(0, 1).unwrap();
    assert!(face.verb("xfade"));
    assert_eq!(face.note, None);
    assert_eq!(face.last_error.take().as_deref(), Some("xfade: the selected clip is not on an audio track"));
    println!("striaxfademore: c -> note \"xfade: 10 equal\" (no error); move keeps, undo clears; refusal on last_error");
}


// ---- STRIATRANSITIONNEXT (LEDGER SR991): the new transition length on the message row (named tail block) ----

/// M3: a length change of the selected dissolve (`dur 00:00:00:06`, `dur 00:05`, `]`, the `[` key) names the new length on
/// the message row (`Face::message_text`, the face `note`); a refusal shows the error, not a note; a pointer move keeps
/// the note, the next command input drops it; with no transition selected there is no note.
#[test]
fn striatransitionnext_length_note_on_message_row() {
    use bandy::SMessage;
    use stria::nle_face::SURFACE;
    let input = |t: &str| SMessage::Input { target: SURFACE.into(), text: t.into() };
    let (tl, src) = synth_fixture();
    let tl0 = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
    let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.on(&input("dur 6")));
    assert_eq!(face.last_error.take(), Some(format!("dur 6: {}", timeline_core::EditError::Transition)));
    assert_eq!(face.note, None, "no transition selected: no note");
    face.state.select_transition(0, 0).unwrap();
    assert!(face.on(&input("dur 00:00:00:06")));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.message_text().as_deref(), Some("transition: 00:00:00:06 (6 ticks)"));
    assert!(face.on(&input("dur 00:05")));
    assert_eq!(face.message_text().as_deref(), Some("transition: 00:00:00:05 (5 ticks)"));
    assert_eq!(face.state.undo_depth(), 2);
    assert!(face.on(&input("]")));
    assert_eq!(face.message_text().as_deref(), Some("transition: 00:00:00:06 (6 ticks)"));
    face.on(&SMessage::BrowserMouseMove(5.0, 5.0, 0));
    assert_eq!(face.note.as_deref(), Some("transition: 00:00:00:06 (6 ticks)"), "a move keeps the note");
    face.on(&SMessage::BrowserKey("[".into()));
    assert!(face.on(&SMessage::BrowserText("[".into())));
    assert_eq!(face.message_text().as_deref(), Some("transition: 00:00:00:05 (5 ticks)"), "the key road");
    assert!(face.on(&input("dur 99")));
    assert_eq!(face.note, None, "a refusal leaves no note");
    assert_eq!(face.message_text(), Some(format!("dur 99: {}", timeline_core::EditError::Handle)));
    face.last_error = None;
    assert!(face.on(&input("dur 7")));
    assert!(face.note.is_some());
    face.on(&input("zoom+"));
    assert_eq!(face.note, None, "the next command input drops the note");
}

// ---- STRIAINSERTMARKSKEYSNEXT (LEDGER SR985): the all-tracks insert words and the target track on the face (named tail block) ----

/// `insert-all` at 15 with B selected lays B 0..10 on V1 and ripples A1 (place's `Mode::InsertAll`, one undo step); the
/// message row names it `insert-all: B 00:00:00:10 at 00:00:00:15`.
#[test]
fn striainsertmarkskeysnext_face_insert_all_ripples_every_track() {
    assert_eq!(VERBS[VERBS.len() - 4..], ["insert-all", "insert-marked-all", "target", "help"]);
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 1).unwrap();
    face.state.seek(15);
    assert!(face.verb("insert-all"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl.place(0, Clip::new(SourceId(1), 0, 10, 15), timeline_core::Mode::InsertAll).unwrap());
    assert_eq!(face.state.timeline.tracks[1].clips[2], Clip::new(SourceId(1), 5, 10, 25), "A1 rippled");
    assert_eq!(face.message_text().as_deref(), Some("insert-all: B 00:00:00:10 at 00:00:00:15"));
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl, "one undo");
    println!("striainsertmarkskeysnext: insert-all at 15 -> V1 B 0..10 at 15, A1 B 5..10 at 25");
}

/// `target V2` notes `target: V2` (no undo step, the document clean) and steers `insert` onto V2; `target A1` with a V1
/// clip refuses `insert` by kind; `target off` notes `target: off`; an unknown label is `target V9: no such track`.
#[test]
fn striainsertmarkskeysnext_face_target_steers_and_notes() {
    let (mut tl, src) = synth_fixture();
    tl.add_track(Kind::Video);
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("target V2"));
    assert_eq!(face.message_text().as_deref(), Some("target: V2"));
    assert_eq!((face.state.undo_depth(), &face.state.timeline), (0, &tl), "the target is not the document");
    face.state.select(0, 1).unwrap();
    face.state.seek(5);
    assert!(face.verb("insert"));
    assert_eq!(face.state.timeline, tl.place(2, Clip::new(SourceId(1), 0, 10, 5), timeline_core::Mode::Insert).unwrap());
    assert_eq!(face.message_text().as_deref(), Some("insert: B 00:00:00:10 at 00:00:00:05"));
    assert!(face.verb("target A1"));
    face.state.select(0, 0).unwrap();
    let before = face.state.timeline.clone();
    assert!(face.verb("insert"));
    assert_eq!(face.last_error.take(), Some("insert: video and audio tracks do not mix".to_string()));
    assert_eq!(face.state.timeline, before);
    assert!(face.verb("target off"));
    assert_eq!(face.message_text().as_deref(), Some("target: off"));
    assert!(face.verb("target V9"));
    assert_eq!(face.last_error.take(), Some("target V9: no such track".to_string()));
    println!("striainsertmarkskeysnext: target V2 -> insert on V2; target A1 -> insert refused by kind");
}

// ---- STRIAEXPORTPRESETSNEXT (LEDGER SR989): a preset's scale through THE one scaler, the pick on the message row (named tail block) ----

/// A PNG's IHDR width and height.
fn striaexportpresetsnext_png_size(path: &std::path::Path) -> (u32, u32) {
    let b = std::fs::read(path).unwrap();
    (u32::from_be_bytes([b[16], b[17], b[18], b[19]]), u32::from_be_bytes([b[20], b[21], b[22], b[23]]))
}

#[test]
fn striaexportpresetsnext_scaled_frames_are_halved() {
    use stria::nle_export::{AudioOut, Container, PngSequence, export_as, scaled, scaled_size};
    let (full, half) = (nlefaceexport_dir("scale-full"), nlefaceexport_dir("scale-half"));
    std::fs::create_dir_all(&full).unwrap();
    std::fs::create_dir_all(&half).unwrap();
    let (tl, src) = synth_fixture();
    let a = export_as(&tl, 0..tl.end(), &src, &full, Box::new(PngSequence), AudioOut::Wav, Container::Mp4).unwrap();
    let b = export_as(&tl, 0..tl.end(), &src, &half, scaled(Box::new(PngSequence), 2), AudioOut::Wav, Container::Mp4).unwrap();
    assert_eq!((a.frames, b.frames), (35, 35));
    let (w, h) = striaexportpresetsnext_png_size(&full.join("000000.png"));
    assert_eq!(striaexportpresetsnext_png_size(&half.join("000000.png")), scaled_size(w, h, 2));
    assert_eq!(striaexportpresetsnext_png_size(&half.join("000034.png")), scaled_size(w, h, 2));
    assert_eq!(scaled_size(w, h, 2), (((w / 2) & !1).max(2), ((h / 2) & !1).max(2)));
    assert_eq!(std::fs::read(full.join("audio.wav")).unwrap(), std::fs::read(half.join("audio.wav")).unwrap(), "the sound is not scaled");
    println!("striaexportpresetsnext: {w}x{h} -> {:?}", scaled_size(w, h, 2));
}

#[test]
fn striaexportpresetsnext_scale_one_is_the_encoder_itself() {
    use stria::nle_export::{AudioOut, Container, H264Opts, export_as, scaled, video_named_with};
    let (a, b) = (nlefaceexport_dir("scale-one-a"), nlefaceexport_dir("scale-one-b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let (tl, src) = synth_fixture();
    export_as(&tl, 0..tl.end(), &src, &a, video_named_with("h264", tl.tb, H264Opts::default()).unwrap(), AudioOut::Wav, Container::Mp4).unwrap();
    export_as(&tl, 0..tl.end(), &src, &b, scaled(video_named_with("h264", tl.tb, H264Opts::default()).unwrap(), 1), AudioOut::Wav, Container::Mp4).unwrap();
    assert_eq!(std::fs::read(a.join("video.mp4")).unwrap(), std::fs::read(b.join("video.mp4")).unwrap(), "a divisor of one is no scale, byte-exact");
}

#[test]
fn striaexportpresetsnext_face_proxy_is_the_scaled_table_encoder() {
    use stria::nle_export::{Container, PRESETS, audio_named, export_as_with, preset_named, scaled, video_named_with};
    assert_eq!(PRESETS.iter().map(|p| (p.name, p.scale)).collect::<Vec<_>>(), [("web", 1), ("archive", 1), ("proxy", 2)]);
    let (picked, direct) = (nlefaceexport_dir("scale-picked"), nlefaceexport_dir("scale-direct"));
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(face.verb("preset proxy"));
    assert_eq!(face.message_text(), Some("preset proxy: h264 + aac:64 crf=32 scale=1/2".to_string()));
    assert!(face.verb(&format!("export {}", picked.display())));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(face.wait_export());
    let line = face.last_error.take().expect("the result line");
    assert!(line.starts_with("export: 35 frames, ") && line.contains(" h264 + aac:64 in "), "{line}");
    let p = preset_named("proxy").unwrap();
    let (tl, src) = synth_fixture();
    std::fs::create_dir_all(&direct).unwrap();
    let enc = scaled(video_named_with(p.video, tl.tb, p.opts()).unwrap(), 2);
    export_as_with(&tl, 0..tl.end(), &src, &direct, enc, audio_named(p.audio).unwrap(), Container::Mp4, &mut |_, _| Ok(())).unwrap();
    assert_eq!(std::fs::read(picked.join("movie.mp4")).unwrap(), std::fs::read(direct.join("movie.mp4")).unwrap(), "the face's proxy is the table's encoder at half size, byte-exact");
    println!("striaexportpresetsnext: proxy {line}");
}

#[test]
fn striaexportpresetsnext_pick_is_the_message_rows_last_fallback() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl.clone(), src);
    let before = face.message_text();
    assert!(face.verb("preset proxy"));
    face.on(&bandy::SMessage::BrowserMouseMove(1.0, 1.0, 0));
    match before {
        None => assert_eq!(face.message_text(), Some("export preset proxy".to_string()), "nothing else said: the pick"),
        Some(b) => assert_eq!(face.message_text(), Some(b), "a reel line wins over the pick"),
    }
    face.last_error = Some("export: busy (1/2)".into());
    assert_eq!(face.message_text().as_deref(), Some("export: busy (1/2)"), "an error wins over the pick");
    face.last_error = None;
    assert!(face.verb("preset none"));
    face.on(&bandy::SMessage::BrowserMouseMove(1.0, 1.0, 0));
    assert_eq!(face.message_text().filter(|m| m.starts_with("export preset ")), None, "no pick, nothing named");
    assert_eq!(face.state.timeline, tl, "a view-only word leaves the document");
    assert!(!face.dirty());
}

// ---- STRIAKMARKERS (LEDGER SR9001): the host KEYS the kernel Player's `m` / `M` must agree with (named tail block) ----

/// The kernel Player binds `m` -> `mark` and `M` -> `unmark` (vplay.rs `EDIT_KEYS`): the host's `m` names the same word,
/// and the host leaves `M` unbound, so no key means two things on the two faces.
#[test]
fn striakmarkers_host_keys_agree_with_the_players_m_and_shift_m() {
    use stria::nle_face::KEYS;
    assert!(KEYS.contains(&("m", "mark")));
    assert!(!KEYS.iter().any(|&(k, _)| k == "M"), "M is unbound on the host");
}

// ---- STRIAOPACITYNEXT (LEDGER SR993): the lower run hands back; the step note is not an error (named tail block) ----

/// SR868's stack (V1 A[0,30)@0 counter 0, V2 B[0,10)@10 counter 1000 at 50 %): after the overlay A's run (kept in the mix
/// slot) continues as the live run, so the render opens TWO runs (A at 0, B at 10), not three, and A's frames after the
/// overlay are its own ticks.
#[test]
fn striaopacitynext_render_hands_back_without_a_reseek() {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 30, 0)];
    tl.tracks[1].clips = vec![Clip::new(b, 0, 10, 10)];
    let mut src = Sources::new(&tl);
    src.add(a, synth_source(0, 50, 25)).unwrap();
    src.add(b, synth_source(1000, 50, 25)).unwrap();
    let tl = tl.set_opacity(&[(1, 0)], 50).unwrap();
    let steps = schedule(&tl, 0..30, &src).unwrap();
    assert_eq!(steps[20].seek, None, "the schedule hands the lower run back");
    let mut sink = Collect::default();
    let r = render(&tl, 0..30, &src, &mut sink).unwrap();
    assert_eq!(sink.frames.len(), 30);
    let at = |t: usize| TestPattern::read_counter(&sink.frames[t].1, 64, 48);
    assert_eq!((at(9), at(20), at(21), at(29)), (Some(9), Some(20), Some(21), Some(29)));
    assert_eq!(r.seeks, 2, "A at 0, B at 10; A continues after the overlay");
    println!("striaopacitynext: frames={} seeks={} runup={}", r.frames, r.seeks, r.runup_frames);
}

/// The opacity step's percent is a face note: `last_error` stays empty and `message_text` reads it; a refusal is still an
/// error; a pointer move keeps the note, a taken select-all clears it.
#[test]
fn striaopacitynext_step_note_is_not_an_error() {
    use stria::nle_face::{EditorState, Face, PixelOrder, synth_fixture};
    let (tl0, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false);
    assert!(face.verb("opacity+"));
    assert!(face.last_error.take().is_some_and(|e| e.starts_with("opacity: ")), "nothing selected: a refusal");
    assert_eq!(face.note, None);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("opacity-"));
    assert_eq!(face.last_error, None);
    assert_eq!(face.message_text().as_deref(), Some("opacity: 90 %"));
    face.on(&bandy::SMessage::BrowserMouseMove(5.0, 5.0, 0));
    assert_eq!(face.note.as_deref(), Some("opacity: 90 %"), "a pointer move keeps the note");
    assert!(face.on(&bandy::SMessage::BrowserSelectAll));
    assert_eq!(face.note, None, "a taken input clears it");
    assert_eq!(face.message_text(), face.reel_line());
    println!("striaopacitynext: opacity- -> note opacity: 90 %, cleared by select-all");
}

// ---- STRIAROLLNEXT (LEDGER SR999): the roll tool's cue on the band, GTK's order for the slide-ten chords (named tail block) ----

/// Rule: the tool on with V1 clip 0 selected (synth_fixture: A [13,23)@0 | B [0,10)@10 butted), the face's paint shows
/// timeline_core's ROLL_CUE straddling the cut at tick 10 on V1's row; the tool off again, the paint is byte-identical to
/// the paint before the tool was turned on.
#[test]
fn striarollnext_tool_paints_the_cue_on_the_cut() {
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select(0, 0).unwrap();
    let off = face.paint();
    assert!(face.verb("roll-tool"));
    let on = face.paint();
    let l = face.layout();
    let (x, y) = (l.x_of(10), l.point(0, 5).1 as i64);
    let at = |px: &[u8], x: i64, y: i64| px[((y * l.w as i64 + x) * 4) as usize..((y * l.w as i64 + x) * 4 + 4) as usize].to_vec();
    assert_eq!(at(&on, x - 2, y), timeline_core::view::ROLL_CUE.to_vec());
    assert_eq!(at(&on, x + 1, y), timeline_core::view::ROLL_CUE.to_vec());
    assert_ne!(at(&off, x - 2, y), timeline_core::view::ROLL_CUE.to_vec());
    assert!(face.verb("roll-tool"));
    assert_eq!(face.paint(), off, "off: byte-identical");
    println!("striarollnext: the roll tool paints ROLL_CUE over the cut at x {x}");
}

/// Rule: SR752's Cmd slide-ten rows take GTK's chord order: `Shift+Meta+less` is `slide -10` (a twin editor's), and
/// `Shift+Meta+greater` takes it back; the `Meta+Shift+` spellings are no KEYS rows.
#[test]
fn striarollnext_cmd_slide_chords_take_gtks_order() {
    let (mut tl, src) = synth_fixture();
    let (a, b) = (SourceId(0), SourceId(1));
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 10, 20), Clip::new(a, 23, 33, 30)];
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select_at(0, 22).unwrap();
    let mut twin = EditorState::new(tl.clone());
    twin.select_at(0, 22).unwrap();
    assert_eq!(twin.verb("slide -10"), Some(Ok(())));
    assert!(nle3eslipstep_chord(&mut face, "Shift+Meta+less"));
    assert_eq!(face.state.timeline, twin.timeline);
    assert!(nle3eslipstep_chord(&mut face, "Shift+Meta+greater"));
    assert_eq!(face.state.timeline, tl);
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert!(!KEYS.iter().any(|k| k.0 == "Meta+Shift+less" || k.0 == "Meta+Shift+greater"));
    println!("striarollnext: Shift+Meta+less / greater slide ten");
}

// ---- STRIAMULTISELECTNEXT (LEDGER SR998): a shift-click on a set member keeps the user's primary (named tail block) ----
#[test]
fn striamultiselectnext_shift_click_member_keeps_primary() {
    let (tl, src) = stria::nle_face::synth_fixture();
    let mut face = stria::nle_face::Face::new(stria::nle_face::EditorState::new(tl), src, (320, 160), stria::nle_face::PixelOrder::Rgba, false);
    let tl0 = face.state.timeline.clone();
    let (x0, y0) = face.layout().point(0, 5); // V1's A@0
    let (x1, y1) = face.layout().point(0, 15); // V1's B@10
    let (x2, y2) = face.layout().point(0, 28); // V1's third clip@25
    face.on(&nle3eslipdrag_btn(true, x0, y0, 0));
    face.on(&nle3eslipdrag_btn(false, x0, y0, 0));
    for (x, y) in [(x1, y1), (x2, y2)] {
        face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_SHIFT));
        face.on(&nle3eslipdrag_btn(false, x, y, bandy::MOD_SHIFT));
    }
    assert_eq!((face.state.selection, face.state.set.clone()), (Some((0, 2)), vec![(0, 0), (0, 1), (0, 2)]));
    face.on(&nle3eslipdrag_btn(true, x1, y1, bandy::MOD_SHIFT));
    assert_eq!(face.state.selection, Some((0, 2)), "the press does not take the primary");
    assert!(face.on(&nle3eslipdrag_btn(false, x1, y1, bandy::MOD_SHIFT)));
    assert_eq!((face.state.selection, face.state.set.clone()), (Some((0, 2)), vec![(0, 0), (0, 2)]), "B out, the primary kept");
    assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl0, 0), "view-only");
}

// ---- STRIATHUMBNAILSNEXT (LEDGER SR997): heads and peak tables decoded off the face's thread (named tail block) ----

/// STRIATHUMBNAILSNEXT (SR997): a face decoding off its thread paints its first frame with no head (the jobs queued, the face
/// pending); once the worker settles it holds the three heads (the preview road's frames) and its next paint is a synchronous
/// face's first paint, byte for byte.
#[test]
fn striathumbnailsnext_heads_decode_off_the_face_thread() {
    let (tl, src) = synth_fixture();
    let expect = striathumbnails_expected(&tl, &src, SourceId(0), 13);
    let (tl2, src2) = synth_fixture();
    let mut sync = Face::new(EditorState::new(tl2), src2, (320, 160), PixelOrder::Rgba, false);
    let want = sync.paint();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    face.decode_off_thread();
    assert!(face.bg_running());
    let first = face.paint();
    assert_eq!(face.thumb_decodes(), 0, "the first paint decodes no head on the face's thread");
    assert!(face.thumb(SourceId(0), 13).is_none(), "the placeholder holds the bar");
    assert!(face.thumbs_pending(), "jobs out: serve's clock keeps running");
    face.bg_settle();
    assert_eq!(face.thumb_decodes(), 3, "the worker decoded the three heads once");
    assert_eq!(face.thumb(SourceId(0), 13), Some(&expect), "the worker's head is the preview road's src_in frame");
    let second = face.paint();
    assert!(!face.thumbs_pending());
    assert_ne!(first, second, "the heads and peaks landed");
    assert_eq!(second, want, "the settled face paints the synchronous face's frame");
    println!("striathumbnailsnext: first paint decodes=0 pending=1, settled decodes={} second==sync", face.thumb_decodes());
}

/// STRIATHUMBNAILSNEXT (SR997): the peak scans go to the worker too: none after the first paint, then the synchronous face's
/// scans and tables once settled.
#[test]
fn striathumbnailsnext_peaks_scan_off_the_face_thread() {
    let (tl2, src2) = synth_fixture();
    let mut sync = Face::new(EditorState::new(tl2), src2, (320, 160), PixelOrder::Rgba, false);
    let _ = sync.paint();
    assert!(sync.wave_scans() > 0);
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    face.decode_off_thread();
    let _ = face.paint();
    assert_eq!(face.wave_scans(), 0, "no scan on the face's thread");
    assert!(face.wave_peaks(SourceId(0)).is_none());
    face.bg_settle();
    assert_eq!(face.wave_scans(), sync.wave_scans());
    for id in [SourceId(0), SourceId(1)] {
        assert_eq!(face.wave_peaks(id), sync.wave_peaks(id), "source {id:?}'s table");
    }
    println!("striathumbnailsnext: settled scans={}", face.wave_scans());
}

/// STRIATHUMBNAILSNEXT (SR997): a face nobody serves starts no worker and decodes inside the paint as before.
#[test]
fn striathumbnailsnext_unserved_face_stays_synchronous() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    assert!(!face.bg_running());
    let _ = face.paint();
    assert_eq!(face.thumb_decodes(), 3);
    assert!(!face.thumbs_pending());
    face.bg_settle();
    assert_eq!(face.thumb_decodes(), 3, "settling a face with no worker is a no-op");
}
// ---- STRIALOUDNESSNEXT (LEDGER SR995): an audio-only measure under an empty video track; range and true peak on the line (named tail block) ----

/// NLE1's fixture with every video track's clips removed: the video track holds only gaps, the audio track its clips.
fn strialoudnessnext_gap_video() -> (Timeline, Sources) {
    let (mut tl, src) = synth_fixture();
    for t in tl.tracks.iter_mut().filter(|t| t.kind == Kind::Video) {
        t.clips.clear();
    }
    (tl, src)
}

/// Rule: under a video track of gaps the measure reads its audio: the LUFS of exactly the PCM a render of the audio
/// tracks alone mixes, the count moving inside the range; a frame-taking sink over the same timeline is still refused
/// `NoSize`.
#[test]
fn strialoudnessnext_empty_video_track_measures_the_audio() {
    let (tl, src) = strialoudnessnext_gap_video();
    let range = 0..tl.end();
    assert!(schedule(&tl, range.clone(), &src).unwrap().iter().any(|s| s.kind == Kind::Video), "the gap track still schedules video steps");
    let mut audio = tl.clone();
    audio.tracks.retain(|t| t.kind == Kind::Audio);
    let mut pcm = StriaLoudnessPcm::default();
    render(&audio, range.clone(), &src, &mut pcm).unwrap();
    let (rate, ch) = pcm.fmt.expect("the fixture has audio");
    let want = gneiss_pal::dsp::audio::meter::integrated_lufs(&pcm.pcm, ch as usize, rate);
    assert_eq!(stria::nle::loudness(&tl, range.clone(), &src).unwrap(), Some(want));
    let w = stria::nle::LoudnessWatch::new(&range);
    let got = stria::nle::loudness_reading(&tl, range.clone(), &src, Some(w.clone())).unwrap().expect("a reading");
    assert_eq!(got.lufs, want);
    let done = w.done.load(std::sync::atomic::Ordering::Relaxed);
    let total = (range.end - range.start) as u64;
    assert!(done > 0 && done <= total, "done {done} of {total}");
    let mut taker = StriaLoudnessPcm::default();
    assert!(matches!(render(&tl, range, &src, &mut taker), Err(stria::nle::RenderError::NoSize)));
    println!("strialoudnessnext: gap video track -> {want:.2} LUFS, done {done}/{total}");
}

/// A sink that declines only the first frame it is asked for, then wants every one.
#[derive(Default)]
struct StriaLoudnessNextLate {
    asked: usize,
    frames: usize,
}

impl stria::nle::RenderSink for StriaLoudnessNextLate {
    fn frame(&mut self, _out_t: i64, _width: u32, _height: u32, _rgba: &[u8]) {
        self.frames += 1;
    }
    fn pcm(&mut self, _out_t: i64, _rate: u32, _channels: u16, _interleaved: &[f32]) {}
    fn frame_wanted(&mut self, _out_t: i64) -> bool {
        self.asked += 1;
        self.asked > 1
    }
}

/// Rule: a sink that declines the first video frame passes the size check, but the first frame it wants with nothing
/// sized is refused `NoSize` at its step: no 0x0 frame is ever handed out.
#[test]
fn strialoudnessnext_late_wanted_frame_still_refused() {
    let (tl, src) = strialoudnessnext_gap_video();
    let mut sink = StriaLoudnessNextLate::default();
    assert!(matches!(render(&tl, 0..tl.end(), &src, &mut sink), Err(stria::nle::RenderError::NoSize)));
    assert_eq!(sink.frames, 0);
    assert!(sink.asked >= 2, "asked {}", sink.asked);
    println!("strialoudnessnext: late wanted frame refused after {} asks", sink.asked);
}

/// Rule: the face's finished line carries the range and the true peak after the LUFS, the ticks note last; under a gap
/// video track it reads the audio (not the NoSize words); the readings agree with audio_core's meter over the PCM.
#[test]
fn strialoudnessnext_face_line_reads_range_and_peak() {
    let (tl, src) = strialoudnessnext_gap_video();
    let (tl2, src2) = strialoudnessnext_gap_video();
    let mut face = nlefaceexport_face(tl.clone(), src);
    assert!(face.verb("loudness"));
    assert!(face.wait_loudness());
    let line = face.last_error.take().expect("the line");
    let want = stria::nle::loudness_reading(&tl2, 0..tl2.end(), &src2, None);
    assert_eq!(line, stria::nle_face::loudness_reading_line(&want, &None));
    let r = want.unwrap().expect("a reading");
    assert!(line.contains(&format!("LUFS integrated, LRA {:.1} LU, true peak {:.1} dBTP", r.range_lu, r.true_peak_db)), "{line}");
    assert!(!line.contains("no video source"), "{line}");
    let mut audio = tl2.clone();
    audio.tracks.retain(|t| t.kind == Kind::Audio);
    let mut pcm = StriaLoudnessPcm::default();
    render(&audio, 0..tl2.end(), &src2, &mut pcm).unwrap();
    let (rate, ch) = pcm.fmt.expect("audio");
    let mut m = gneiss_pal::dsp::audio::meter::Loudness::new(rate, ch as usize);
    m.push(&pcm.pcm);
    assert_eq!((r.range_lu, r.true_peak_db), (m.range_lu(), m.true_peak_db()));
    let marked = stria::nle_face::loudness_reading_line(&Ok(Some(r)), &Some(5..30));
    assert!(marked.ends_with(" ticks 5..30"), "{marked}");
    assert_eq!(stria::nle_face::loudness_reading_line(&Ok(None), &None), "loudness: no audio in the range");
    println!("strialoudnessnext: {line}");
}

// ---- STRIAPANNEXT (LEDGER SR994): the face note and the pan drag's reading (named tail block) ----

/// `pan+` reads its pan on the face note (not `last_error`), and `message_text` shows it; a pointer move keeps it, a named key
/// clears it; a refusal is `last_error` with no note; a button drag on the selected audio clip's pan grip pans it in one undo
/// step and its release reads the pan on the note.
#[test]
fn striapannext_note_reads_after_last_error() {
    use timeline_core::view::{PAN_GRIP_H, pan_half};
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    let input = |f: &mut Face, v: &str| f.on(&bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: v.into() });
    assert!(input(&mut face, "pan+"));
    assert_eq!((face.note.clone(), face.last_error.take()), (None, Some(format!("pan: {}", EditError::NoClip))), "a refusal: no note");
    face.state.select(1, 1).unwrap();
    assert!(input(&mut face, "pan+"));
    assert_eq!((face.note.as_deref(), face.last_error.as_deref()), (Some("pan: R 13"), None));
    assert_eq!(face.message_text().as_deref(), Some("pan: R 13"));
    face.on(&bandy::SMessage::BrowserMouseMove(5.0, 5.0, 0));
    assert_eq!(face.note.as_deref(), Some("pan: R 13"), "a pointer move keeps the note");
    face.on(&bandy::SMessage::BrowserKey("Home".into()));
    assert_eq!(face.note, None, "a named key clears it");

    let l = face.layout();
    let (x0, x1, y0) = (l.x_of(10), l.x_of(20), l.row_y(1));
    let half = pan_half(x0, x1).max(1) as f64;
    let (x, y) = ((x0 + x1) as f64 / 2.0, y0 as f64 + 2.0);
    assert!(y < (y0 + PAN_GRIP_H) as f64);
    let (p0, d0) = (face.state.timeline.tracks[1].clips[1].pan_cq8, face.state.undo_depth());
    face.on(&bandy::SMessage::BrowserButton { down: true, x, y, mods: 0, surface: 0 });
    assert_eq!(face.dragging().map(|d| d.kind), Some(DragKind::Pan));
    face.on(&bandy::SMessage::BrowserMouseMove(x + half, y, 0));
    face.on(&bandy::SMessage::BrowserButton { down: false, x: x + half, y, mods: 0, surface: 0 });
    assert_eq!((p0, face.state.timeline.tracks[1].clips[1].pan_cq8, face.state.undo_depth()), (16, 127, d0 + 1));
    assert_eq!((face.note.as_deref(), face.last_error.as_deref()), (Some("pan: R 100"), None), "the release reads the pan");
    println!("striapannext: note pan: R 13 kept by a move, cleared by Home; grip drag -> R 100");
}

// ---- STRIAMARKERJUMPKEYSNEXT (LEDGER SR984): marker colour / marker range on the face, the note (named tail block) ----

/// `marker colour` / `marker range` sit before `help`; each success is ONE undo step that dirties the document and words
/// the face note (read by `message_text` after `last_error`, cleared by the next named key); refusals are `last_error`;
/// the EDL carries `* UNA MCOLOUR` / `* UNA MRANGE`.
#[test]
fn striamarkerjumpkeysnext_face_colour_range_note_and_refusals() {
    let help = VERBS.iter().position(|v| *v == "help").unwrap();
    for v in ["marker colour", "marker range"] {
        let i = VERBS.iter().position(|x| *x == v).unwrap_or_else(|| panic!("{v} is a VERBS word"));
        assert!(i < help, "{v} before help");
    }
    let mut f = striamarkerjumpkeys_face();
    assert!(f.verb("marker colour"));
    assert_eq!(f.last_error.take().as_deref(), Some("marker colour: no marker at 00:00:00:00"));
    assert!(f.verb("marker range"));
    assert_eq!(f.last_error.take().as_deref(), Some("marker range: no marker at or before 00:00:00:00"));
    assert_eq!((f.state.undo_depth(), f.note.as_deref()), (0, None));
    f.state.seek(12);
    assert!(f.verb("mark"));
    let _ = f.last_error.take();
    let depth = f.state.undo_depth();
    assert!(f.verb("marker colour"));
    assert_eq!(f.note.as_deref(), Some("marker M1 colour red at 00:00:00:12"));
    assert_eq!(f.message_text().as_deref(), Some("marker M1 colour red at 00:00:00:12"));
    assert!(f.verb("marker colour red"));
    assert_eq!(f.last_error.take().as_deref(), Some("marker colour: M1 is already red"));
    assert!(f.verb("marker colour mauve"));
    assert_eq!(f.last_error.take().as_deref(), Some("marker colour: no colour mauve (orange, red, yellow, green, cyan, blue, purple, pink)"));
    assert!(f.verb("marker colour blue"));
    assert_eq!(f.note.as_deref(), Some("marker M1 colour blue at 00:00:00:12"));
    f.state.seek(20);
    assert!(f.verb("marker range"));
    assert_eq!(f.note.as_deref(), Some("marker M1 range 00:00:00:12..00:00:00:20"));
    assert!(f.verb("marker range"));
    assert_eq!(f.last_error.take().as_deref(), Some("marker range: M1 already ends at 00:00:00:20"));
    assert_eq!(f.state.undo_depth(), depth + 3, "colour, colour, range: one step each");
    assert!(f.dirty());
    let m = f.state.timeline.marker_at(12).cloned().unwrap();
    assert_eq!((m.colour, m.len), (5, 8));
    let text = edl::write(&f.state.timeline, "T").unwrap();
    assert!(text.contains("* UNA MARKER 00:00:00:12 \"M1\"\n* UNA MCOLOUR 00:00:00:12 blue\n* UNA MRANGE 00:00:00:12 00:00:00:20\n"), "{text}");
    assert!(f.on(&bandy::SMessage::BrowserKey("Down".into())));
    assert_eq!(f.note, None, "a named key clears the note");
    f.state.seek(12);
    assert!(f.verb("marker range"));
    assert_eq!(f.note.as_deref(), Some("marker M1 range cleared at 00:00:00:12"));
    assert_eq!(f.state.timeline.marker_at(12).map(|m| m.len), Some(0));
    assert!(f.verb("undo"));
    assert_eq!(f.state.timeline.marker_at(12).map(|m| m.len), Some(8));
    println!("striamarkerjumpkeysnext: M1 red -> blue, range 12..20, cleared, undone; EDL {}", text.lines().filter(|l| l.starts_with("* UNA M")).count());
}

// ---- STRIAKTC (LEDGER SR9018): the face's readout is timeline_core's one text (named tail block) ----

/// Each unit's readout is `view::tc_readout` on the face's timeline and playhead (the text the kernel Player's band shows),
/// on a 1/25 and on a 30000/1001 timeline; off, none.
#[test]
fn striaktc_face_readout_is_the_shared_text() {
    use timeline_core::view::{TcUnit, tc_readout};
    let mut face = striatimecodedisplay_face(false);
    face.state.seek(12);
    assert_eq!(face.tc_readout(), None, "off");
    for tb in [timeline_core::Timebase { num: 1, den: 25 }, timeline_core::Timebase { num: 1001, den: 30000 }] {
        face.state.timeline.tb = tb;
        for (w, u) in [("tc timecode", TcUnit::Timecode), ("tc frames", TcUnit::Frames), ("tc seconds", TcUnit::Seconds)] {
            assert!(face.verb(w));
            assert_eq!(face.tc_readout(), Some(tc_readout(&face.state.timeline, face.state.playhead, u)), "{w}");
        }
    }
    assert!(face.verb("tc off"));
    assert_eq!(face.tc_readout(), None);
    assert!(face.last_error.is_none());
    println!("striaktc: face readout == view::tc_readout at {}", face.state.playhead);
}

// ---- STRIASEQTHREE (LEDGER SR9022): sequence cache span and staleness, upright frames (named tail block) ----
// A sequence reel keeps up to eight decoded frames (a scrub back and forth reads no file), each served only while its file's
// `(length, modified)` stamp holds: a rewritten file is read again, a removed one is refused by name. Every decoded frame is
// turned upright by its EXIF orientation, and a JPEG reel's size is taken upright at the add.

#[test]
fn striaseqthree_rewritten_file_is_read_again() {
    let pat = striaimportimagesequence_dir("seqthree-rewrite", 4, 4, &[(7, [10, 10, 10])]);
    let (tl, s) = striaimportimagesequence_tl(1);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    let mut a = StriaTitleFrames::default();
    render(&tl, 0..1, &src, &mut a).unwrap();
    assert!(a.0[0].3 == striaimportimagesequence_px(4, 4, [10, 10, 10]));
    let f = pat.replace("%04d", "0007");
    let png = gneiss_pal::dsp::image::encode_rgba(4, 4, &striaimportimagesequence_px(4, 4, [90, 90, 90])).unwrap();
    std::fs::write(&f, png).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    std::fs::File::options().write(true).open(&f).unwrap().set_modified(later).unwrap();
    let mut b = StriaTitleFrames::default();
    render(&tl, 0..1, &src, &mut b).unwrap();
    assert!(b.0[0].3 == striaimportimagesequence_px(4, 4, [90, 90, 90]), "the rewritten file is read, not the kept frame");
    println!("striaseqthree: file 7 rewritten -> the repaint reads the new pixels");
}

#[test]
fn striaseqthree_removed_file_is_missing() {
    let pat = striaimportimagesequence_dir("seqthree-removed", 4, 4, &[(7, [10, 10, 10])]);
    let (tl, s) = striaimportimagesequence_tl(1);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    render(&tl, 0..1, &src, &mut StriaTitleFrames::default()).unwrap();
    std::fs::remove_file(pat.replace("%04d", "0007")).unwrap();
    let e = render(&tl, 0..1, &src, &mut StriaTitleFrames::default()).unwrap_err().to_string();
    assert!(e.contains("frame 7 missing"), "the kept frame of a removed file is not served: {e}");
    println!("striaseqthree: file 7 removed -> {e}");
}

#[test]
fn striaseqthree_scrub_keeps_several_frames() {
    use std::os::unix::fs::PermissionsExt;
    let cols = [[200, 10, 10], [10, 200, 10], [10, 10, 200]];
    let pat = striaimportimagesequence_dir("seqthree-scrub", 4, 4, &[(7, cols[0]), (8, cols[1]), (9, cols[2])]);
    let (tl, s) = striaimportimagesequence_tl(3);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    render(&tl, 0..3, &src, &mut StriaTitleFrames::default()).unwrap();
    let files: Vec<String> = ["0007", "0008", "0009"].iter().map(|n| pat.replace("%04d", n)).collect();
    for f in &files {
        std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    if std::fs::read(&files[0]).is_err() {
        for (t, c) in [2i64, 0, 1].into_iter().map(|t| (t, cols[t as usize])) {
            let mut got = StriaTitleFrames::default();
            render(&tl, t..t + 1, &src, &mut got).expect("every scrubbed tick is a kept frame: no file is read");
            assert!(got.0[0].3 == striaimportimagesequence_px(4, 4, c), "tick {t} is its own kept file");
        }
        println!("striaseqthree: ticks 2, 0, 1 repainted from the kept frames with every file unreadable");
    } else {
        println!("striaseqthree: SKIP the no-read leg: a 0o000 file reads here (root); the striaseqthree_kept unit tests carry it");
    }
    for f in &files {
        std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}

#[test]
fn striaseqthree_jpeg_exif6_is_upright() {
    let d = common::fresh_dir("striaseqthree", "exif6");
    std::fs::write(d.join("f_0001.jpg"), include_bytes!("../../../tools/eyes/suites/facet/fixtures/photo-o6.jpg")).unwrap();
    let pat = format!("{}/f_%04d.jpg", d.display());
    let (tl, s) = striaimportimagesequence_tl(1);
    let mut src = Sources::new(&tl);
    src.add_sequence(s, &pat).unwrap();
    assert_eq!(src.summary(s).as_deref(), Some("seq 1..=1 32x48"), "a 48x32 stored card with EXIF 6 is 32x48 upright");
    let mut sink = StriaTitleFrames::default();
    let r = render(&tl, 0..1, &src, &mut sink).unwrap();
    assert_eq!((r.width, r.height), (32, 48), "the reel's size is upright");
    assert_eq!((sink.0[0].1, sink.0[0].2), (32, 48));
    println!("striaseqthree: photo-o6.jpg (48x32, EXIF 6) -> reel 32x48, frame 32x48");
}

// ---- STRIATRACKGAINTHREE (LEDGER SR9024): the face note after mute / solo; the track gain line names the channel's track (named tail block) ----
/// M3: `mute` / `solo A1` put their success line on the face NOTE (not last_error), message_text reads it; a hover keeps
/// it, the next taken Input clears it; a refusal stays on last_error and wins over the note.
#[test]
fn striatrackgainthree_face_note_mute() {
    use bandy::SMessage;
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(1, 0).unwrap();
    assert!(face.verb("mute"));
    assert_eq!((face.last_error.as_deref(), face.note.as_deref()), (None, Some("mute: A1 on")));
    assert_eq!(face.message_text().as_deref(), Some("mute: A1 on"));
    assert!(face.verb("solo V1"));
    assert_eq!(face.note.as_deref(), Some("solo: V1 on"));
    assert!(face.verb("mute A1"));
    assert_eq!(face.note.as_deref(), Some("mute: A1 off"));
    face.on(&SMessage::BrowserMouseMove(1.0, 1.0, 0));
    assert_eq!(face.note.as_deref(), Some("mute: A1 off"), "a hover keeps the note");
    face.on(&SMessage::Input { target: stria::nle_face::SURFACE.into(), text: "zoom+".into() });
    assert_eq!(face.note, None, "the next taken input clears it");
    face.note = Some("x".into());
    assert!(face.verb("mute A9"));
    assert_eq!(face.message_text(), Some(format!("mute A9: {}", timeline_core::EditError::NoTrack)), "an error wins over the note");
    assert_eq!(face.state.undo_depth(), 3);
    println!("striatrackgainthree face: mute -> note 'mute: A1 on'; cleared by the next Input; mute A9 on last_error");
}

/// M3: `track-gain A1 -6` with V1's clip selected sets A1 (one undo step) and its line names A1, not the selection's V1.
#[test]
fn striatrackgainthree_face_track_gain_channel() {
    let (tl, src) = synth_fixture();
    let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
    face.state.select(0, 0).unwrap();
    assert!(face.verb("track-gain A1 -6"));
    assert_eq!(face.last_error.take().as_deref(), Some("track-gain: A1 -6.0 dB"));
    assert_eq!((face.state.timeline.tracks[1].gain_cdb, face.state.timeline.tracks[0].gain_cdb, face.state.undo_depth()), (-600, 0, 1));
    assert!(face.verb("track-gain- A1"));
    assert_eq!(face.last_error.take().as_deref(), Some("track-gain: A1 -7.0 dB"));
    assert!(face.verb("track-gain A2 -6"));
    assert_eq!(face.last_error.take(), Some(format!("track-gain A2 -6: {}", timeline_core::EditError::NoTrack)));
    println!("striatrackgainthree face: track-gain A1 -6 with V1 selected -> track-gain: A1 -6.0 dB");
}

// ---- STRIAUNDOTHREE (LEDGER SR9027): redo entries on the noted history line, picked by pointer (named tail block) ----

/// SR973's face after `undo 2` (the post-split timeline shown), then `history` through `Face::on` with `last_error`
/// taken as `serve` takes it: (the face, the post-slip+ timeline, the note on the row).
fn striaundothree_face() -> (stria::nle_face::Face, Timeline, String) {
    let (mut face, split) = striaundomore_face(true);
    let top = face.state.timeline.clone();
    assert!(face.verb("undo 2"));
    face.last_error = None;
    assert_eq!(face.state.timeline, split);
    assert!(face.on(&bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: "history".into() }));
    assert_eq!(face.last_error.take().as_deref(), Some("history: 1 split"), "last_error keeps SR876's line");
    let note = face.message_text().expect("the note on the row");
    (face, top, note)
}

/// Rule: with redo entries the note lists them after the undo line, next first; a press on `r2 slip+` runs SR934's
/// `redo 2`, forward to after "slip+", and the note is gone.
#[test]
fn striaundothree_pick_redo() {
    let (mut face, top, note) = striaundothree_face();
    assert_eq!(note, "history: 1 split || redo: r1 delete | r2 slip+");
    let (x, y) = striaundomore_at(&face, &note, note.find("r2 slip+").unwrap());
    assert!(striaundomore_press(&mut face, x, y));
    let landed = face.last_error.take();
    assert_eq!(landed.as_deref(), Some("redo 2: forward to after \"slip+\""));
    assert_eq!((face.state.undo_depth(), face.state.redo_depth()), (3, 0));
    assert_eq!(face.state.timeline, top);
    assert!(face.message_text().is_none_or(|m| !m.starts_with("history:")), "the pick clears the note");
    println!("striaundothree: pick at x {x:.1} -> {landed:?}");
}

/// Rule: an undo entry on a line that carries redo entries still picks SR973's undo; a press on the redo separator is
/// taken and moves nothing; with nothing to redo the note is SR876's line unchanged.
#[test]
fn striaundothree_redo_part_miss_and_undo_pick() {
    let (mut face, _top, note) = striaundothree_face();
    let shown = face.state.timeline.clone();
    let (x, y) = striaundomore_at(&face, &note, note.find("|| redo").unwrap() + 4);
    assert!(!striaundomore_press(&mut face, x, y), "on the separator: taken, nothing to repaint");
    assert_eq!((face.state.undo_depth(), face.state.redo_depth(), face.state.timeline == shown), (1, 2, true));
    assert_eq!(face.message_text().as_deref(), Some(note.as_str()), "the note stands");
    let (x, y) = striaundomore_at(&face, &note, note.find("1 split").unwrap());
    assert!(striaundomore_press(&mut face, x, y));
    assert_eq!(face.last_error.take().as_deref(), Some("undo 1: back to before \"split\""));
    assert_eq!((face.state.undo_depth(), face.state.redo_depth()), (0, 3));

    let (mut fresh, _split) = striaundomore_face(true);
    assert!(fresh.verb("history"));
    let line = fresh.last_error.take();
    assert_eq!(line.as_deref(), Some("history: 3 slip+ | 2 delete | 1 split"));
    assert_eq!(fresh.message_text(), line, "no redo entries: the note is the history line");
}

// ---- STRIAPROXYTHREE (LEDGER SR9026): a reel changed on disk reloads at the next acting input, its proxy follows (named tail block) ----

/// The `help` button's message (an acting input that edits nothing).
fn striaproxythree_input(text: &str) -> bandy::SMessage {
    bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: text.into() }
}

/// A proxied reel rewritten on disk reloads at the next acting input: the note names it, its stale proxy is detached and
/// rebuilt on SR915's queue, the preview shows the new file; the timeline and the document are untouched.
#[test]
fn striaproxythree_changed_reel_reloads_and_the_proxy_follows() {
    use stria::nle_face::{preview, proxy_fresh, proxy_paths};
    let (dir, pa, _, mut face, tl) = striaproxyoff_face("three-reload");
    let a = tl.sources[0].id;
    let xa = proxy_paths(&pa).0;
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy") && face.wait_proxy() && face.has_proxy(a));
    face.last_error = None;
    assert!(face.on(&striaproxythree_input("zoom+")));
    assert_eq!(face.note, None, "nothing changed on disk: no note");
    let new = synth_source(5000, 50, 25);
    std::fs::write(&pa, &new).unwrap();
    face.on(&striaproxythree_input("zoom-"));
    assert_eq!(face.note, Some(format!("watch: A: reloaded {} bytes, proxy rebuilds", new.len())));
    assert_eq!(face.last_error.take(), Some(format!("proxy: A: building (stale) -> {}", xa.display())));
    assert!(face.proxying() && !face.has_proxy(a), "the old proxy is detached while the new one builds");
    assert!(face.wait_proxy());
    assert_eq!(face.last_error.take(), Some(format!("proxy: A: 50 frames -> {}", xa.display())));
    assert!(face.has_proxy(a) && proxy_fresh(&pa));
    assert!(face.verb("proxy off"));
    face.last_error = None;
    face.state.seek(3);
    let _ = face.paint();
    let mut alt = Sources::new(&tl);
    alt.add(a, new.clone()).unwrap();
    alt.add(tl.sources[1].id, synth_source(1000, 50, 25)).unwrap();
    let want = preview(&tl, &alt, 3).unwrap().expect("the rewritten reel's frame at 3");
    assert!(face.still().is_some_and(|s| s.rgba == want.rgba), "the preview reads the reloaded file");
    face.on(&striaproxythree_input("zoom+"));
    assert_eq!(face.note, None, "the next acting input clears the note and reloads nothing");
    assert!(face.last_error.is_none());
    assert_eq!(face.state.timeline.sources, tl.sources, "the document's sources are untouched");
    assert!(!face.dirty(), "a reload leaves the document clean");
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxythree reload: A reloaded {} bytes, proxy rebuilt, preview = the new file", new.len());
}

/// A reel changed while a job holds the sources waits (the note; the next acting input retries); bytes that do not open are
/// refused by name with the old media kept; a pointer move stats nothing; a face without paths watches nothing.
#[test]
fn striaproxythree_busy_waits_unreadable_and_pointer() {
    let (dir, _, pb, mut face, _) = striaproxyoff_face("three-busy");
    face.hold_proxy_at(Some(3));
    face.state.select(0, 0).unwrap();
    assert!(face.verb("proxy") && face.proxying());
    face.last_error = None;
    let new = synth_source(7000, 50, 25);
    std::fs::write(&pb, &new).unwrap();
    face.on(&striaproxythree_input("zoom+"));
    assert_eq!(face.note.as_deref(), Some("watch: B: changed on disk; reloads after the running job"));
    assert!(face.cancel_proxy() && face.wait_proxy());
    face.last_error = None;
    face.on(&striaproxythree_input("zoom-"));
    assert_eq!(face.note, Some(format!("watch: B: reloaded {} bytes", new.len())));
    assert!(face.last_error.is_none(), "B carried no proxy: nothing rebuilds");
    std::fs::write(&pb, b"not media").unwrap();
    face.on(&bandy::SMessage::BrowserMouseMove(5.0, 5.0, 0));
    assert!(face.last_error.is_none(), "a pointer move stats nothing");
    face.on(&striaproxythree_input("zoom+"));
    let line = face.last_error.take().unwrap_or_default();
    assert!(line.starts_with("watch: B: "), "{line}");
    assert_eq!(face.note, None);
    face.state.seek(12);
    let _ = face.paint();
    assert!(face.still().is_some(), "the old media stays after a refused reload");
    face.on(&striaproxythree_input("zoom-"));
    assert!(face.last_error.is_none(), "a refused stamp is recorded: no repeat");
    assert!(!face.dirty());
    let (t2, s2) = synth_fixture();
    let mut bare = Face::new(EditorState::new(t2), s2, (320, 160), PixelOrder::Rgba, false);
    bare.on(&striaproxythree_input("zoom+"));
    assert!(bare.note.is_none() && bare.last_error.is_none(), "no paths: nothing watched");
    let _ = std::fs::remove_dir_all(&dir);
    println!("striaproxythree busy: waited, reloaded, refused \"{line}\"");
}

// ---- STRIACROPTHREE (LEDGER SR9032): the crop drag moves the whole rect (named tail block) ----

/// STRIACROPTHREE (SR9032): on a clip cropped 100 on every side, a press in the middle of the picture (inside the crop rect,
/// away from the picture's edges) begins a move (all four sides); a tenth of the picture to the right is l = 200, r = 0
/// (t / b kept), the release commits ONCE (undo depth 2 after the crop_set) and undo restores the centred rect.
#[test]
fn striacropthree_drag_moves_whole_rect() {
    let (mut face, (qx, _, qw, _), ym) = striacropmore_face();
    let c100 = timeline_core::Crop { l: 100, t: 100, r: 100, b: 100 };
    face.state.crop_set(|_| c100).unwrap();
    let _ = face.paint();
    let btn = |down: bool, x: f64, y: f64| bandy::SMessage::BrowserButton { down, x, y, mods: 0, surface: 0 };
    let xm = qx + qw / 2.0;
    assert!(face.on(&btn(true, xm, ym)));
    assert!(face.dragging().is_none());
    assert_eq!(face.crop_dragging().map(|d| d.sides), Some(15));
    let dx = qw / 10.0;
    assert!(face.on(&bandy::SMessage::BrowserMouseMove(xm + dx, ym, 0)));
    assert_eq!(face.crop_dragging().map(|d| d.crop()), Some(timeline_core::Crop { l: 200, t: 100, r: 0, b: 100 }));
    assert_eq!(face.state.undo_depth(), 1, "nothing committed while live");
    assert!(face.on(&btn(false, xm + dx, ym)));
    face.on(&bandy::SMessage::BrowserClick(xm + dx, ym, 0));
    assert!(face.crop_dragging().is_none());
    assert_eq!(face.last_error.take(), None);
    assert_eq!(face.state.selected().map(|c| c.crop), Some(timeline_core::Crop { l: 200, t: 100, r: 0, b: 100 }));
    assert_eq!(face.state.undo_depth(), 2);
    assert!(face.verb("undo"));
    assert_eq!(face.state.selected().map(|c| c.crop), Some(c100));
    println!("striacropthree: centre press moved the rect {dx} px -> l=200 r=0, one commit, undo restores");
}

// ---- STRIAXFADETHREE (LEDGER SR9023): the face drags the audio crossfade bracket's edge (named tail block) ----

/// A 4-tick crossfade on A1's cut at 10 (its bracket x 80..112 at zoom 8, in A1's foot band): a press on its right edge
/// zone, dragged +2 ticks with Alt (no snap), released, is ONE commit to 6 ticks with the curve kept and the incoming clip
/// selected; `undo` restores the 4; a press in the bracket's middle is the clip press it always was (no length change).
#[test]
fn striaxfadethree_face_drags_the_xfade_bracket() {
    use bandy::SMessage;
    use timeline_core::view::{Layout, TRANS_H};
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    face.state.select(1, 1).unwrap();
    assert!(face.verb("xfade 4"));
    let _ = face.note.take();
    let (tl0, depth) = (face.state.timeline.clone(), face.state.undo_depth());
    let x = tl0.audio_xfades[0];
    assert_eq!((x.track, x.at, x.dur), (1, 10, 4));
    let l = Layout::of(320, 160, &face.state);
    let (x0, y0, x1, y1) = l.xfade_rect(&x);
    assert_eq!((x0, x1, y1 - y0), (80, 112, TRANS_H));
    let y = (y0 + y1) as f64 / 2.0;
    face.state.deselect();
    let btn = |down, x| SMessage::BrowserButton { down, x, y, mods: bandy::MOD_ALT, surface: 0 };
    face.on(&btn(true, 110.0));
    assert_eq!(face.dragging().map(|d| d.kind), Some(stria::nle_face::DragKind::XfadeOut));
    face.on(&SMessage::BrowserMouseMove(126.0, y, 0));
    face.on(&btn(false, 126.0));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    let after = face.state.timeline.audio_xfades[0];
    assert_eq!((after.track, after.at, after.dur, after.curve), (1, 10, 6, x.curve));
    assert_eq!((face.state.selection, face.state.undo_depth()), (Some((1, 1)), depth + 1));
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl0);
    face.on(&btn(true, 96.0));
    assert_ne!(face.dragging().map(|d| d.kind), Some(stria::nle_face::DragKind::XfadeOut));
    face.on(&btn(false, 96.0));
    assert_eq!(face.state.timeline.audio_xfades, tl0.audio_xfades, "the middle is no edge drag");
    println!("striaxfadethree: the bracket 80..112 right edge +2 ticks -> 6, one undo step; the middle unchanged");
}

// ---- STRIANORMALIZETHREE (LEDGER SR9035): normalize at the mix's rate, its scan off the face's thread (named tail block) ----

/// STRIANORMALIZETHREE (SR9035): a project rate that is not the synth's own.
fn strianormalizethree_rate() -> u32 {
    if stria::nle::SYNTH_RATE == 44_100 { 48_000 } else { 44_100 }
}

/// Rule: `peak_at(.., os, 0)` is SR963's `peak_over` exactly (os 1 and 4), and `peak_mix` with no project rate is too.
#[test]
fn strianormalizethree_peak_at_extends_peak_over() {
    let (tl, src) = stria::nle_face::synth_fixture();
    let a = tl.source_by_reel("A").unwrap().id;
    let ns = tl.tb.to_ns(13)..tl.tb.to_ns(33);
    for os in [1, stria::nle::TRUE_PEAK_OS] {
        let p = stria::nle::peak_over(&src, a, ns.clone(), os).unwrap().unwrap();
        assert_eq!(stria::nle::peak_at(&src, a, ns.clone(), os, 0).unwrap().unwrap(), p, "os {os}");
        assert_eq!(stria::nle::peak_mix(&src, a, ns.clone(), os).unwrap().unwrap(), p, "os {os}");
    }
}

/// Rule: under a project rate `peak_mix` is the larger of the sample peak and the peak of the source through the mix's
/// one `Sinc` at that rate, and within 3 dB of the sample peak (synth source A, whole: the stream starts at sample 0).
#[test]
fn strianormalizethree_peak_mix_reads_the_project_rate() {
    let (tl, mut src) = stria::nle_face::synth_fixture();
    let a = tl.source_by_reel("A").unwrap().id;
    let r2 = strianormalizethree_rate();
    src.set_audio_rate(r2);
    let ns = 0..tl.tb.to_ns(50);
    let sample = stria::nle::peak_of(&src, a, ns.clone()).unwrap().unwrap();
    let r = stria::nle::SYNTH_RATE as u64;
    let pcm: Vec<f32> = (0..50 * r / 25).map(|i| stria::nle::synth_sample(0, i) as f32 / 32768.0).collect();
    let mut s = gneiss_pal::dsp::audio::resample::Sinc::new(stria::nle::SYNTH_RATE, r2, 1);
    let mut want = sample;
    for chunk in pcm.chunks(stria::nle::SYNTH_RATE as usize) {
        want = want.max(stria::nle::up_peak(&mut s, 1, chunk, false));
    }
    want = want.max(stria::nle::up_peak(&mut s, 1, &[], true));
    let got = stria::nle::peak_mix(&src, a, ns, 1).unwrap().unwrap();
    assert!((got - want).abs() < 1e-5, "mix peak {got} vs {want}");
    assert!(got >= sample && got <= sample * 1.4126, "mix {got} vs sample {sample}");
    println!("strianormalizethree: {r2} Hz mix peak {got}, sample peak {sample}");
}

/// Rule: a face over sources at a project rate normalizes against `peak_mix`, in one undo step.
#[test]
fn strianormalizethree_normalize_uses_the_mix_peak() {
    let (mut tl, mut src) = stria::nle_face::synth_fixture();
    let a = tl.source_by_reel("A").unwrap().id;
    tl.tracks[1].clips = vec![timeline_core::Clip::new(a, 13, 33, 0)];
    src.set_audio_rate(strianormalizethree_rate());
    let peak = stria::nle::peak_mix(&src, a, tl.tb.to_ns(13)..tl.tb.to_ns(33), 1).unwrap().unwrap();
    let mut face = stria::nle_face::Face::new(stria::nle_face::EditorState::new(tl), src, (320, 160), stria::nle_face::PixelOrder::Rgba, false);
    face.state.select(1, 0).unwrap();
    let depth = face.state.undo_depth();
    assert!(face.verb("normalize"));
    assert_eq!(face.state.selected().unwrap().gain_cdb, stria::nle_face::normalize_gain_cdb(peak, -100).unwrap());
    assert_eq!(face.state.undo_depth(), depth + 1, "one undo step");
}

/// STRIANORMALIZETHREE (SR9035): tick the scan until it reports (bounded: 2000 x 5 ms).
fn strianormalizethree_wait(face: &mut stria::nle_face::Face) {
    for _ in 0..2000 {
        if face.normalize_tick() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the normalize scan never reported");
}

/// Rule: a served face (`scan.thread`) answers `measuring` at once with the gain unmoved, refuses a second word as
/// `busy`, and its tick sets the synchronous gain with the synchronous line in one undo step.
#[test]
fn strianormalizethree_scan_runs_off_the_face_thread() {
    let (mut sync, _) = strianormalize_face();
    assert!(sync.verb("normalize"));
    let (want_gain, want_line) = (sync.state.selected().unwrap().gain_cdb, sync.last_error.take());
    let (mut face, _) = strianormalize_face();
    face.scan.thread = true;
    let depth = face.state.undo_depth();
    assert!(face.verb("normalize"));
    assert_eq!(face.last_error.take(), Some(format!("normalize: {}", stria::nle_face::NORMALIZE_SCANNING)));
    assert_eq!(face.state.selected().unwrap().gain_cdb, 0, "the gain waits for the scan");
    assert!(face.normalizing());
    assert!(face.verb("normalize"));
    assert_eq!(face.last_error.take(), Some("normalize: busy".to_string()));
    strianormalizethree_wait(&mut face);
    assert!(!face.normalizing());
    assert_eq!(face.state.selected().unwrap().gain_cdb, want_gain);
    assert_eq!(face.last_error.take(), want_line);
    assert_eq!(face.state.undo_depth(), depth + 1, "one undo step");
    assert!(!face.normalize_tick(), "nothing left to report");
}

/// Rule: a scan whose selection moved before its tick sets nothing and says so.
#[test]
fn strianormalizethree_scan_refuses_a_moved_selection() {
    let (mut face, _) = strianormalize_face();
    face.scan.thread = true;
    assert!(face.verb("normalize"));
    face.state.select(0, 0).unwrap();
    let tl = face.state.timeline.clone();
    strianormalizethree_wait(&mut face);
    assert_eq!(face.last_error.take(), Some("normalize: the selection moved".to_string()));
    assert_eq!(face.state.timeline, tl, "the timeline unmoved");
}

// ---- STRIAGAPSTHREE (LEDGER SR9028): the gap notes on the face note, `close-gaps keep`, the Close Sync word (named tail block) ----

/// `close-sync` sits right before `close-all` in VERBS. On the synth fixture with a to-black at 20 and a from-black at 25 on
/// V1: `close-gaps keep` (V1 selected) keeps the faded 20..25 gap open (an equal value: no commit) and names it on the face
/// note; a taken Input clears the note; `close-sync` is `close-gaps sync` (one commit) and its success line is a note, not an
/// error, read by `message_text`.
#[test]
fn striagapsthree_note_keep_and_close_sync() {
    let i = VERBS.iter().position(|&v| v == "close-sync").expect("close-sync in VERBS");
    assert_eq!(VERBS[i + 1], "close-all");
    let (tl, src) = synth_fixture();
    let tl = tl.add_transition_to_black(0, 20, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let tl = tl.add_transition_from_black(0, 25, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let mut face = nlefaceexport_face(tl.clone(), src);
    face.state.select(0, 1).unwrap();
    assert!(face.verb("close-gaps keep"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.note.as_deref(), Some("close-gaps keep: closed 0 gaps, kept 1"));
    assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (tl.close_gaps_keep(0).unwrap(), 0));
    assert_eq!(face.state.timeline, tl);
    assert!(face.on(&bandy::SMessage::Input { target: stria::nle_face::SURFACE.into(), text: "zoom+".into() }));
    assert_eq!(face.note, None, "a taken input clears the note");
    assert!(face.verb("close-sync"));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.message_text().as_deref(), Some("close-gaps sync: closed 1 gap, removed 2 fades"));
    assert_eq!(face.note.take().as_deref(), Some("close-gaps sync: closed 1 gap, removed 2 fades"));
    assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (tl.close_gaps_sync().unwrap(), 1));
    println!("striagapsthree: VERBS[{i}] = close-sync; V1 {:?}", face.state.timeline.tracks[0].clips);
}
// ---- STRIAEXPORTQUEUETHREE (LEDGER SR9042): GTK's Shift+Escape keys export-skip (named tail block) ----

/// GTK's `Shift+Escape` skips the running export through the `export-skip` word: the queue is kept (unlike Esc) and B runs;
/// while the help shows it only dismisses; with nothing running it names that.
#[test]
fn striaexportqueuethree_shift_escape_skips() {
    let (a, b) = (nlefaceexport_dir("three-a"), nlefaceexport_dir("three-b"));
    let mut face = striaexportqueue_paused(&a);
    assert!(face.verb(&format!("export {} png", b.display())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: queued (1 waiting)"));
    assert!(face.verb("help"));
    assert!(face.on(&bandy::SMessage::BrowserKey("Shift+Escape".into())));
    assert!(!face.help_shown(), "Shift+Escape while the help shows only dismisses it");
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.export_progress(), Some((3, 35)), "no skip while dismissing");
    assert!(face.on(&bandy::SMessage::BrowserKey("Shift+Escape".into())));
    assert_eq!(face.last_error.take().as_deref(), Some("export: skipping (1 waiting)"));
    assert_eq!(face.export_queued(), 1, "the skip keeps the queue");
    striaexportqueueskip_drain(&mut face);
    let line = face.last_error.take().expect("B's line");
    assert!(line.starts_with("export: 35 frames, ") && line.ends_with(&format!(" -> {}", b.display())), "{line}");
    assert!(b.join("000034.png").is_file(), "B ran after the Shift+Escape skip");
    assert!(face.on(&bandy::SMessage::BrowserKey("Shift+Escape".into())));
    assert_eq!(face.last_error.take().as_deref(), Some("export-skip: no export running"));
    for d in [&a, &b] {
        let _ = std::fs::remove_dir_all(d);
    }
    println!("striaexportqueuethree: Shift+Escape skipped A, B ran: \"{line}\"");
}

/// The export-skip help row names both keys; Escape stays cancel; the name never arrives as text.
#[test]
fn striaexportqueuethree_help_row_names_both_keys() {
    assert!(KEYS.contains(&("Shift+Escape", "export-skip")));
    assert!(KEYS.contains(&("Escape", "cancel")));
    let rows = help_rows();
    let row = rows.iter().find(|r| r.1 == help_phrase("export-skip")).expect("the export-skip help row");
    assert_eq!(row.0, "E / Shift+Escape");
    let (tl, src) = synth_fixture();
    let mut face = nlefaceexport_face(tl, src);
    assert!(!face.on(&bandy::SMessage::BrowserText("Shift+Escape".into())), "a multi-char name is never text");
    println!("striaexportqueuethree: help row {:?}", row);
}

// ---- STRIALINKAUTOTHREE (LEDGER SR9039): the link words' keys (named tail block) ----
const STRIALINKAUTOTHREE_KEYS: [(&str, &str); 6] = [("y", "link-av"), ("Y", "unlink"), ("Ctrl+l", "link"), ("Meta+l", "link"), ("Ctrl+Shift+L", "unlink-all"), ("Shift+Meta+L", "unlink-all")];

/// The six KEYS rows name the four link words; each word's help row carries its keys and its phrase.
#[test]
fn strialinkautothree_keys_name_the_link_words() {
    let rows = help_rows();
    for (k, w) in STRIALINKAUTOTHREE_KEYS {
        assert!(KEYS.contains(&(k, w)), "{k} -> {w}");
        assert!(VERBS.contains(&w), "{w} is a VERBS word");
        assert!(HELP.iter().any(|(a, _)| *a == w), "no phrase for {w}");
        let row = rows.iter().find(|(_, p)| *p == help_phrase(w)).expect("the word's help row");
        assert!(row.0.split(" / ").any(|x| x == k), "{k:?} not in {:?}", row.0);
    }
    assert_eq!(help_phrase("link-av"), "link each picture to its sound");
    assert_eq!(help_phrase("unlink-all"), "unlink every clip");
    println!("strialinkautothree: {} link keys, help rows={}", STRIALINKAUTOTHREE_KEYS.len(), rows.len());
}

/// The keys drive the words through `Face::verb`: `y` links the V1 / A1 pairs, `Y` parts the selection's pair, Ctrl+l
/// links the selection, Ctrl+Shift+L parts every link; one undo step each. A one-char key as BrowserKey is not taken.
#[test]
fn strialinkautothree_keys_drive_the_link_words() {
    let (tl, src) = synth_fixture();
    let tl0 = tl.clone();
    let mut face = nlefaceexport_face(tl, src);
    assert!(!face.on(&bandy::SMessage::BrowserKey("y".into())), "a one-char BrowserKey alone is not taken");
    assert_eq!(face.state.timeline, tl0);
    assert!(face.on(&bandy::SMessage::BrowserText("y".into())));
    assert_eq!(face.last_error.take(), None);
    assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (tl0.link_av(), 1), "y: every pair linked, one step");
    face.state.select(0, 0).unwrap();
    assert_eq!(face.state.set, vec![(0, 0), (1, 0)], "the picture takes its sound");
    let linked = face.state.timeline.clone();
    assert!(face.on(&bandy::SMessage::BrowserText("Y".into())));
    assert_eq!(face.last_error.take(), None);
    assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (linked.set_group(&[(0, 0), (1, 0)], 0).unwrap(), 2), "Y: the pair parted");
    face.state.deselect();
    face.state.select(0, 0).unwrap();
    face.state.select_add(1, 0).unwrap();
    let before = face.state.timeline.clone();
    assert!(face.on(&bandy::SMessage::BrowserKey("Ctrl+l".into())));
    assert_eq!(face.last_error.take(), None);
    assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (before.set_group(&[(0, 0), (1, 0)], before.next_group()).unwrap(), 3), "Ctrl+l: one fresh group");
    assert!(face.on(&bandy::SMessage::BrowserKey("Ctrl+Shift+L".into())));
    assert_eq!(face.last_error.take(), None);
    assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (tl0.clone(), 4), "Ctrl+Shift+L: every link parted");
    face.state.deselect();
    assert!(face.on(&bandy::SMessage::BrowserKey("Ctrl+l".into())));
    assert_eq!(face.last_error.take().as_deref(), Some("link: no such clip"), "nothing selected, on the message row");
    assert_eq!(face.state.undo_depth(), 4);
    println!("strialinkautothree: y, Y, Ctrl+l, Ctrl+Shift+L: four steps");
}

// ---- STRIARIPPLETHREE (LEDGER SR9034): the ripple-all tool — a ripple drag across every track (named tail block) ----

use stria::nle_face::RIPPLE_ALL_NOTE;

/// Rule: `ripple-all` is a transport word before `help` and the key `A`; it toggles the tool with no undo step and no
/// dirty document; while on the message line names the tool; again turns it off.
#[test]
fn striaripplethree_the_word_toggles_the_tool() {
    let (mut face, tl) = striarippletrim_face(0);
    assert_eq!(VERBS[VERBS.len() - 2], "ripple-all");
    assert!(KEYS.contains(&("A", "ripple-all")));
    assert!(!face.ripple_all());
    assert!(face.verb("ripple-all"));
    assert!(face.ripple_all());
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.message_text().as_deref(), Some(RIPPLE_ALL_NOTE));
    assert_eq!((&face.state.timeline, face.state.undo_depth(), face.dirty()), (&tl, 0, false));
    assert!(nle5drangekeys_press(&mut face, "A"));
    assert!(!face.ripple_all());
    assert_ne!(face.message_text().as_deref(), Some(RIPPLE_ALL_NOTE));
    println!("striaripplethree: ripple-all on -> \"{RIPPLE_ALL_NOTE}\", A -> off");
}

/// Rule: with the tool on, SR850's Shift-drag of V1 clip 0's out handle 3 ticks left is the all-tracks ripple: the face
/// equals `ripple_all_out(0, 0, 20)` (A1's twin trims, A1's later clips follow), ONE undo step restores; with the tool
/// off the same drag ripples V1 only (SR850's).
#[test]
fn striaripplethree_tool_on_shift_drag_ripples_every_track() {
    let (mut face, tl) = striarippletrim_face(0);
    assert!(face.verb("ripple-all"));
    let l = face.layout();
    let (x, y) = (l.x_of(10) as f64 - 2.0, l.point(0, 5).1);
    let dx = -3.0 * l.zoom as f64;
    face.on(&nle3eslipdrag_btn(true, x, y, bandy::MOD_SHIFT | bandy::MOD_ALT));
    assert_eq!(face.dragging().map(|d| (d.kind, d.ripple, d.ripple_all)), Some((DragKind::TrimOut, true, true)));
    face.on(&bandy::SMessage::BrowserMouseMove(x + dx, y, 0));
    face.on(&nle3eslipdrag_btn(false, x + dx, y, bandy::MOD_SHIFT | bandy::MOD_ALT));
    assert!(face.last_error.is_none(), "{:?}", face.last_error);
    assert_eq!(face.state.timeline, tl.ripple_all_out(0, 0, 20).unwrap());
    assert_eq!(face.state.undo_depth(), 1);
    assert!(face.verb("undo"));
    assert_eq!(face.state.timeline, tl);
    let (mut face, tl) = striarippletrim_face(0);
    nle3eslipdrag_drag(&mut face, x, y, dx, bandy::MOD_SHIFT | bandy::MOD_ALT);
    assert_eq!(face.state.timeline, tl.ripple_trim_out(0, 0, 20).unwrap());
    assert_eq!(face.state.timeline.tracks[1], tl.tracks[1], "tool off: A1 does not move");
    println!("striaripplethree: tool on, Shift-drag -3 = ripple_all_out; tool off = ripple_trim_out");
}
