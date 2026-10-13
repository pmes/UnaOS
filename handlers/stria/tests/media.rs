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
use resonance::{AudioGraph, BLOCK_SIZE, stream_pair_channels};
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
    fn push(&mut self, stereo: &[f32]) -> usize {
        self.0.lock().unwrap().push(stereo)
    }
    fn consumed(&self) -> u64 {
        self.0.lock().unwrap().consumed()
    }
    fn flush(&mut self) {
        self.0.lock().unwrap().flush()
    }
    fn set_paused(&mut self, paused: bool) {
        self.0.lock().unwrap().set_paused(paused)
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
        let (src, feed) = stream_pair_channels(48_000, 2, 24_000);
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
                let (l, r) = graph.process_stereo();
                assert_eq!(l, r, "a mono source plays on both channels");
                out.extend(l.iter().map(|&x| x as f32));
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
        // Audio: what resonance played is the source, sample for sample (same rate, mono
        // duplicated to stereo).
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
    // 25 fps = 40 ms frames; 1.030 s is inside frame 25 [1.000, 1.040). (A target within the
    // scheduler's early window — half a display period — of the next frame presents that one.)
    let file = build::mkv(&[build::test_pattern_track(1, 160, 120, 25, 100, 10)], &MkvOptions::default());
    let time = ManualTime::new();
    let mut p = Player::open(file, Arc::new(time.clone()), None, 60).unwrap();
    for (target, frame) in [(1_030_000_000i64, 25u32), (1_000_000_000, 25), (25_000_000, 0), (3_999_000_000, 99), (9_000_000_000, 99)] {
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
    t.spec.fourcc = *b"avc1";
    t.spec.config_box = Some(*b"avcC");
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

// ------------------------------------------------------------------------------------------------
// AETHERMEDIA2 (SR134): the MediaSourceFeed (MSE segments instead of a file), playbackRate.
// ------------------------------------------------------------------------------------------------

use gneiss_pal::dsp::demux::fragment::{MediaSourceModel, Op};
use stria::media_source::MediaSourceFeed;

/// A fragmented MP4 cut at its box boundaries: the initialization segment (`ftyp` + `moov`) and
/// one media segment per `moof` + `mdat`.
fn fmp4_segments(file: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut p = 0;
    let mut init = Vec::new();
    let mut media: Vec<Vec<u8>> = Vec::new();
    while p + 8 <= file.len() {
        let size = u32::from_be_bytes([file[p], file[p + 1], file[p + 2], file[p + 3]]) as usize;
        let kind = &file[p + 4..p + 8];
        let bx = &file[p..p + size];
        match kind {
            b"ftyp" | b"moov" => init.extend_from_slice(bx),
            b"moof" => media.push(bx.to_vec()),
            b"mdat" => media.last_mut().expect("moof before mdat").extend_from_slice(bx),
            _ => {}
        }
        p += size;
    }
    (init, media)
}

fn record(op: Op) -> Vec<u8> {
    let mut b = Vec::new();
    op.encode(&mut b);
    b
}

/// Ticks a live player for `n` display periods at 60 Hz; returns the frames shown.
fn run(p: &mut Player, time: &ManualTime, rec: &mut Rec, n: usize) -> bool {
    for _ in 0..n {
        let clock = p.clock_ns();
        if p.tick(&mut RecSink { rec: &mut *rec, clock }) == Tick::Ended {
            return true;
        }
        time.advance(16_666_667);
    }
    false
}

/// Rule (MSE + HTML): appended fragmented-MP4 segments play through the feed; at the buffered end
/// the clock holds (`waiting`) instead of ending; the next append resumes after the last frame
/// decoded; endOfStream ends after the last frame. The records travel in their journal form.
#[test]
fn media_source_feed_plays_appended_fmp4_waits_at_the_buffered_end_and_ends_at_end_of_stream() {
    // 2 s at 25 fps, a keyframe per second, one fragment per second
    let file = build::mp4(&[build::test_pattern_track(1, 160, 120, 25, 50, 25)], &Mp4Options { fragment: 25, ..Default::default() });
    let (init, media) = fmp4_segments(&file);
    assert_eq!(media.len(), 2);
    let mut feed = MediaSourceFeed::new();
    let mut journal = record(Op::AddSourceBuffer { mime: "video/mp4; codecs=\"utp1\"".into() });
    journal.extend(record(Op::Append { sb: 0, bytes: init.clone() }));
    // a record cut in two still applies whole
    let cut = journal.len() - 7;
    assert_eq!(feed.push_records(&journal[..cut]), 1);
    assert_eq!(feed.push_records(&journal[cut..]), 1);
    assert!(feed.ready(), "init segment on every SourceBuffer");
    feed.push_records(&record(Op::Append { sb: 0, bytes: media[0].clone() }));
    assert_eq!(feed.model.buffered(), vec![(0, 1_000_000_000)]);

    let time = ManualTime::new();
    let mut p = Player::open_live(feed.demuxer().unwrap(), Arc::new(time.clone()), None, 60).unwrap();
    p.refresh_live(feed.demuxer().unwrap(), feed.duration_ns(), feed.ended());
    p.play();
    let mut rec = Rec::default();
    assert!(!run(&mut p, &time, &mut rec, 90), "no end before endOfStream");
    assert!(p.waiting(), "held at the buffered end");
    assert_eq!(rec.shown.len(), 25);
    let held = p.clock_ns();
    assert!(held <= 1_000_000_000 + 16_666_667, "clock held at {held}");
    run(&mut p, &time, &mut rec, 10);
    assert_eq!(p.clock_ns(), held, "the clock does not move while waiting");

    feed.push_records(&record(Op::Append { sb: 0, bytes: media[1].clone() }));
    assert!(feed.take_changed());
    assert_eq!(feed.model.buffered(), vec![(0, 2_000_000_000)]);
    p.refresh_live(feed.demuxer().unwrap(), feed.duration_ns(), feed.ended());
    run(&mut p, &time, &mut rec, 5);
    assert!(!p.waiting());
    feed.push_records(&record(Op::EndOfStream { error: 0 }));
    p.refresh_live(feed.demuxer().unwrap(), feed.duration_ns(), feed.ended());
    assert!(run(&mut p, &time, &mut rec, 120), "ended after endOfStream");
    let counters: Vec<u32> = rec.shown.iter().map(|s| s.2).collect();
    assert_eq!(counters, (0..50).collect::<Vec<u32>>(), "every frame once, in order, across the stall");
    assert_eq!(feed.duration_ns(), 2_000_000_000);
}

/// Rule (MSE WebM byte stream): a WebM appended in arbitrary chunks (init segment and clusters cut
/// mid-element) yields the same coded frames as the file demuxer; an unknown-size Cluster completes
/// at the next Cluster, the last one at endOfStream.
#[test]
fn webm_appended_in_chunks_parses_into_the_file_frames() {
    for unknown in [false, true] {
        let opts = MkvOptions { cluster_blocks: 10, unknown_size_cluster: unknown, unknown_size_segment: unknown, ..Default::default() };
        let file = build::mkv(&[build::test_pattern_track(1, 160, 120, 25, 50, 10)], &opts);
        let mut m = MediaSourceModel::new();
        m.apply(&Op::AddSourceBuffer { mime: "video/webm".into() });
        for c in file.chunks(333) {
            let a = m.apply(&Op::Append { sb: 0, bytes: c.to_vec() });
            assert_eq!(a.error, None, "unknown={unknown}");
        }
        let before = m.buffer(0).unwrap().tracks[0].frames.len();
        m.apply(&Op::EndOfStream { error: 0 });
        let frames = &m.buffer(0).unwrap().tracks[0].frames;
        assert_eq!(frames.len(), 50, "unknown={unknown}");
        if unknown {
            assert_eq!(before, 40, "the last unknown-size Cluster completes at endOfStream");
        }
        assert!(frames.iter().enumerate().all(|(i, f)| f.pts_ns == i as i64 * 40_000_000));
        assert_eq!(m.buffered(), vec![(0, 2_000_000_000)]);
    }
}

/// Rule (MSE §3.5.8 / §3.5.9): `timestampOffset` moves a segment; "sequence" mode lays a re-appended
/// segment after the last one; `remove()` runs to the next random access point; a media segment
/// before any initialization segment is an append error.
#[test]
fn coded_frame_processing_offset_sequence_and_remove() {
    let file = build::mp4(&[build::test_pattern_track(1, 160, 120, 25, 50, 25)], &Mp4Options { fragment: 25, ..Default::default() });
    let (init, media) = fmp4_segments(&file);
    let sec = 1_000_000_000i64;

    let mut m = MediaSourceModel::new();
    m.apply(&Op::AddSourceBuffer { mime: "video/mp4".into() });
    assert!(m.apply(&Op::Append { sb: 0, bytes: media[0].clone() }).error.is_some());
    m.apply(&Op::Abort { sb: 0 });
    m.apply(&Op::Append { sb: 0, bytes: init.clone() });
    m.apply(&Op::Append { sb: 0, bytes: media[0].clone() });
    m.apply(&Op::TimestampOffset { sb: 0, ns: 5 * sec });
    m.apply(&Op::Append { sb: 0, bytes: media[0].clone() });
    assert_eq!(m.buffered(), vec![(0, sec), (5 * sec, 6 * sec)]);
    // remove [0.5 s, 0.6 s): it runs to the next random access point (5 s), so [0.5, 1) goes and the
    // GOP's head stays
    m.apply(&Op::Remove { sb: 0, start_ns: sec / 2, end_ns: sec / 2 + sec / 10 });
    assert_eq!(m.buffered(), vec![(0, sec / 2), (5 * sec, 6 * sec)]);

    let mut q = MediaSourceModel::new();
    q.apply(&Op::AddSourceBuffer { mime: "video/mp4".into() });
    q.apply(&Op::Mode { sb: 0, sequence: true });
    q.apply(&Op::Append { sb: 0, bytes: init });
    for _ in 0..3 {
        q.apply(&Op::Append { sb: 0, bytes: media[1].clone() });
    }
    assert_eq!(q.buffered(), vec![(0, 3 * sec)], "sequence mode: back to back from 0");
    assert_eq!(q.duration_ns, Some(3 * sec));
}

/// Rule (playbackRate 0.25..4): the master clock runs at the rate, so frame selection follows it —
/// at 2x, one second of wall time presents two seconds of frames; at 0.5x, half a second.
#[test]
fn playback_rate_scales_the_clock_and_the_frames() {
    for (rate, want_ns) in [(2.0, 2_000_000_000i64), (0.5, 500_000_000)] {
        let file = build::mp4(&[build::test_pattern_track(1, 160, 120, 25, 100, 25)], &Mp4Options::default());
        let time = ManualTime::new();
        let mut p = Player::open(file, Arc::new(time.clone()), None, 60).unwrap();
        assert!(p.set_rate(rate));
        p.play();
        let mut rec = Rec::default();
        run(&mut p, &time, &mut rec, 60);
        let c = p.clock_ns();
        assert!((c - want_ns).abs() <= 2 * 16_666_667 * 4, "rate {rate}: clock {c}");
        let last = rec.shown.last().unwrap();
        assert!((last.1 - c).abs() <= 80_000_000, "rate {rate}: the frame on glass follows the rated clock");
    }
    let file = build::mp4(&[build::test_pattern_track(1, 160, 120, 25, 10, 10)], &Mp4Options::default());
    let mut p = Player::open(file, Arc::new(ManualTime::new()), None, 60).unwrap();
    assert!(!p.set_rate(8.0), "outside 0.25..4 is clamped");
    assert_eq!(p.rate(), 4.0);
}

/// Rule (WSOLA keeps the pitch): a 441 Hz tone stretched at 2x lasts half as long and still crosses
/// zero 882 times a second; at 0.5x it lasts twice as long, same frequency.
#[test]
fn wsola_time_stretch_keeps_the_pitch() {
    use gneiss_pal::dsp::audio::stretch::Wsola;
    let sr = 48_000usize;
    let tone: Vec<f32> = (0..sr * 2).flat_map(|i| {
        let x = (2.0 * std::f64::consts::PI * 441.0 * i as f64 / sr as f64).sin() as f32 * 0.5;
        [x, x]
    }).collect();
    for rate in [2.0, 0.5] {
        let mut w = Wsola::new(sr as u32, 2);
        w.set_rate(rate);
        let mut out = Vec::new();
        for c in tone.chunks(4096) {
            w.push(c);
            w.pull(&mut out);
        }
        let frames = out.len() / 2;
        let want = (sr as f64 * 2.0 / rate) as usize;
        // the tail still inside the analysis window (a frame plus the search) is not out yet
        assert!((frames as f64 - want as f64).abs() < sr as f64 * 0.1, "rate {rate}: {frames} frames, want ~{want}");
        // zero crossings of the left channel over the middle half of the output, per second
        let m = frames / 2;
        let mid: Vec<f32> = out.iter().step_by(2).skip(frames / 4).take(m).copied().collect();
        let zc = mid.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        let per_sec = zc as f64 * sr as f64 / m as f64;
        assert!((per_sec - 882.0).abs() <= 15.0, "rate {rate}: {per_sec} zero crossings per second (pitch moved)");
    }
}
