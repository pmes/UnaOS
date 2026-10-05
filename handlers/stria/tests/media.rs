// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PLAYBACK M3/M4: Stria's video track end to end on synthetic streams whose every frame and
//! sample is known — open → demux → decode → schedule → RGBA sink, audio through a real
//! resonance graph (pulled block by block as a device would), and the bus verbs.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bandy::{SMessage, Synapse};
use gneiss_pal::dsp::avsync::ManualTime;
use gneiss_pal::dsp::demux::build::{self, MkvOptions, Mp4Options};
use gneiss_pal::dsp::video::{Frame, TestPattern};
use resonance::{AudioGraph, BLOCK_SIZE, stream_pair};
use stria::media::{AudioOut, FrameSink, Player, Tick};
use stria::media_bus::MediaService;

#[derive(Default)]
struct Rec {
    shown: Vec<(u64, i64, u32, i64)>, // ordinal, pts, counter read back, clock at present
}
struct RecSink<'a> {
    rec: &'a mut Rec,
    clock: i64,
}
impl FrameSink for RecSink<'_> {
    fn present(&mut self, f: &Frame, ordinal: u64) {
        let n = TestPattern::read_counter(&f.to_rgba(), f.width, f.height).expect("counter legible");
        self.rec.shown.push((ordinal, f.pts_ns, n, self.clock));
    }
}

/// A resonance StreamFeed shared with the test's "device" loop.
struct SharedFeed(Arc<Mutex<resonance::StreamFeed>>);
impl AudioOut for SharedFeed {
    fn push(&mut self, mono: &[f32]) -> usize {
        self.0.lock().unwrap().push(mono)
    }
    fn consumed(&self) -> u64 {
        self.0.lock().unwrap().consumed()
    }
}

fn tone(n: usize) -> Vec<i16> {
    (0..n).map(|i| ((i as f64 * 0.05).sin() * 12000.0) as i16).collect()
}

#[test]
fn utp1_with_pcm_audio_plays_through_resonance_on_the_audio_clock() {
    // 2 s: 25 fps test pattern + 48 kHz mono PCM, in both containers.
    let video = build::test_pattern_track(1, 320, 240, 25, 50, 25);
    let pcm = tone(96_000);
    let audio = build::pcm16_track(2, 48_000, 1, &pcm, 960);
    for file in [build::mp4(&[video.clone(), audio.clone()], &Mp4Options::default()), build::mkv(&[video.clone(), audio.clone()], &MkvOptions::default())] {
        let time = ManualTime::new();
        let (src, feed) = stream_pair(48_000, 24_000);
        let feed = Arc::new(Mutex::new(feed));
        let mut graph = AudioGraph::new(48_000.0);
        graph.add_node(Box::new(src));
        let mut p = Player::open(file, Arc::new(time.clone()), Some(Box::new(SharedFeed(feed.clone()))), 60).unwrap();
        assert!(p.info().audio_clock, "{:?}", p.info());
        assert!(p.info().real_video);
        p.play();
        let mut rec = Rec::default();
        let mut out = Vec::new();
        // 60 Hz display; the "device" pulls 800 samples (12.5 blocks of 64) per refresh.
        let mut owed = 0usize;
        let mut ended = false;
        for _ in 0..200 {
            let clock = p.clock_ns();
            let mut sink = RecSink { rec: &mut rec, clock };
            if p.tick(&mut sink) == Tick::Ended {
                ended = true;
                break;
            }
            time.advance(16_666_667);
            owed += 800;
            while owed >= BLOCK_SIZE {
                out.extend(graph.process().iter().map(|&x| x as f32));
                owed -= BLOCK_SIZE;
            }
        }
        assert!(ended);
        // Every frame shown once, in order, its counter == its ordinal == pts / 40 ms.
        assert_eq!(rec.shown.len(), 50);
        for (i, &(ord, pts, n, clock)) in rec.shown.iter().enumerate() {
            assert_eq!((ord, n as u64, pts), (i as u64, i as u64, i as i64 * 40_000_000));
            // Presented within half a display period of the audio clock.
            assert!((pts - clock).abs() <= 8_333_334, "frame {i}: pts {pts} clock {clock}");
        }
        // Audio: what resonance played is the source, sample for sample (same rate, mono).
        let first = out.iter().position(|&x| x != 0.0).unwrap();
        let played: Vec<i32> = out[first..].iter().map(|&x| (x * 32768.0).round() as i32).take(90_000).collect();
        let want: Vec<i32> = pcm[first..first + played.len()].iter().map(|&x| x as i32).collect();
        assert_eq!(played, want);
        let r = p.drift();
        assert!(r.av_max_ns <= 8_333_334, "{r:?}");
    }
}

#[test]
fn seek_lands_on_the_frame_and_ordinal() {
    let file = build::mp4(&[build::test_pattern_track(1, 160, 120, 25, 100, 10)], &Mp4Options::default());
    let time = ManualTime::new();
    let mut p = Player::open(file, Arc::new(time.clone()), None, 60).unwrap();
    assert!(!p.info().audio_clock);
    p.seek(1_000_000_000);
    p.play();
    let mut rec = Rec::default();
    for _ in 0..3 {
        let clock = p.clock_ns();
        p.tick(&mut RecSink { rec: &mut rec, clock });
        time.advance(16_666_667);
    }
    assert_eq!(rec.shown[0].0, 25);
    assert_eq!(rec.shown[0].2, 25);
    assert_eq!(rec.shown[0].1, 1_000_000_000);
}

/// A seek INSIDE a frame's interval shows that frame (HTML: the frame whose interval holds
/// the position), paused, with its own ordinal — not the next frame later. AETHERVIDEO.
#[test]
fn seek_inside_a_frame_shows_the_covering_frame() {
    // 25 fps = 40 ms frames; 1.030 s is inside frame 25 [1.000, 1.040).
    let file = build::mkv(&[build::test_pattern_track(1, 160, 120, 25, 100, 10)], &MkvOptions::default());
    let time = ManualTime::new();
    let mut p = Player::open(file, Arc::new(time.clone()), None, 60).unwrap();
    for (target, frame) in [(1_030_000_000i64, 25u32), (1_000_000_000, 25), (39_000_000, 0), (3_999_000_000, 99), (9_000_000_000, 99)] {
        p.seek(target);
        let mut rec = Rec::default();
        for _ in 0..3 {
            let clock = p.clock_ns();
            p.tick(&mut RecSink { rec: &mut rec, clock });
            time.advance(16_666_667);
        }
        assert_eq!(rec.shown.len(), 1, "paused: exactly one frame after the seek to {target}");
        assert_eq!((rec.shown[0].0, rec.shown[0].2), (frame as u64, frame), "seek {target}");
        assert_eq!(rec.shown[0].1, frame as i64 * 40_000_000);
    }
}

#[test]
fn poster_is_frame_zero_while_paused() {
    let file = build::mkv(&[build::test_pattern_track(1, 160, 120, 30, 30, 30)], &MkvOptions::default());
    let time = ManualTime::new();
    let mut p = Player::open(file, Arc::new(time.clone()), None, 60).unwrap();
    let mut rec = Rec::default();
    for _ in 0..10 {
        let clock = p.clock_ns();
        p.tick(&mut RecSink { rec: &mut rec, clock });
        time.advance(16_666_667);
    }
    assert_eq!(rec.shown.len(), 1);
    assert_eq!(rec.shown[0].2, 0);
}

#[test]
fn codec_without_decoder_plays_the_labelled_stand_in() {
    let mut t = build::test_pattern_track(1, 320, 240, 10, 10, 10);
    t.spec.fourcc = *b"vp09";
    t.spec.config_box = Some(*b"vpcC");
    t.spec.config = vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let file = build::mp4(&[t], &Mp4Options::default());
    let time = ManualTime::new();
    let mut p = Player::open(file, Arc::new(time.clone()), None, 60).unwrap();
    assert!(!p.info().real_video);
    assert_eq!(p.info().video_decoder, "test-pattern (stand-in)");
    p.play();
    let mut rec = Rec::default();
    while rec.shown.len() < 10 {
        let clock = p.clock_ns();
        if p.tick(&mut RecSink { rec: &mut rec, clock }) == Tick::Ended {
            break;
        }
        time.advance(16_666_667);
    }
    assert_eq!(rec.shown.iter().map(|s| s.2).collect::<Vec<_>>(), (0..10).collect::<Vec<u32>>());
}

#[test]
fn bus_verbs_poster_play_end() {
    let dir = std::env::temp_dir().join(format!("stria-media-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tp.webm");
    std::fs::write(&path, build::mkv(&[build::test_pattern_track(1, 160, 120, 25, 20, 10)], &MkvOptions::default())).unwrap();
    let url = format!("file://{}", path.display());
    let synapse = Synapse::new();
    let mut rx = synapse.subscribe();
    let svc = MediaService::spawn(synapse.clone(), 120, false);
    let mut got = Vec::new();
    let mut wait_for = |pred: &dyn Fn(&SMessage) -> bool, got: &mut Vec<SMessage>| {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(20) {
            match rx.try_recv() {
                Ok(m) => {
                    let hit = pred(&m);
                    got.push(m);
                    if hit {
                        return true;
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        false
    };
    synapse.fire(SMessage::MediaPoster { url: url.clone() });
    assert!(wait_for(&|m| matches!(m, SMessage::MediaFrame { .. }), &mut got));
    synapse.fire(SMessage::PlayMedia { url: url.clone(), title: String::new(), mime: "video/webm".into() });
    assert!(wait_for(&|m| matches!(m, SMessage::MediaEnded { .. }), &mut got));
    synapse.fire(SMessage::PlayMedia { url: "file:///nonexistent.webm".into(), title: String::new(), mime: String::new() });
    assert!(wait_for(&|m| matches!(m, SMessage::MediaError { .. }), &mut got));
    drop(svc);
    let _ = std::fs::remove_dir_all(&dir);

    let opened = got.iter().find_map(|m| match m {
        SMessage::MediaOpened { width, height, video, real_video, audio_clock, duration_ns, .. } => Some((*width, *height, video.clone(), *real_video, *audio_clock, *duration_ns)),
        _ => None,
    });
    assert_eq!(opened, Some((160, 120, "V_UNAOS/TESTPATTERN".to_string(), true, false, 800_000_000)));
    let frames: Vec<(i64, u32)> = got
        .iter()
        .filter_map(|m| match m {
            SMessage::MediaFrame { pts_ns, width, height, rgba, .. } => Some((*pts_ns, TestPattern::read_counter(rgba, *width, *height).unwrap())),
            _ => None,
        })
        .collect();
    // Poster = frame 0; then playback presents in order through frame 19 (drops allowed under
    // host load, never reordering); every frame's counter matches its pts.
    assert_eq!(frames[0], (0, 0));
    assert!(frames.windows(2).all(|w| w[0].0 <= w[1].0));
    assert!(frames.iter().all(|&(pts, n)| pts == n as i64 * 40_000_000));
    assert_eq!(frames.last().unwrap().1, 19);
    let ended = got.iter().find_map(|m| match m {
        SMessage::MediaEnded { presented, dropped, .. } => Some((*presented, *dropped)),
        _ => None,
    });
    let (pr, dr) = ended.unwrap();
    assert_eq!(pr + dr, 20, "every frame either presented or dropped");
}
