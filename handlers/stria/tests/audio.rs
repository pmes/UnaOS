// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AUDIOTRACK (LEDGER SR45) M1/M2: Stria's audio path end to end, through a real resonance
//! stereo graph pulled block by block as a device would.
//!
//! * an audio-only WebM (a libopus conformance stream, stereo) plays on the audio clock, both
//!   channels sample-for-sample equal to the registry's decode (graph rate = stream rate), the
//!   meters equal to the peak of each 50 ms window of that decode, and the session ends;
//! * a seek FLUSHES the device ring: the first sample out after the jump is the target's (the
//!   stale-sample bug PLAYBACK named; go-red without `StreamFeed::flush`);
//! * pause silences the device and freezes the clock; resume continues on the next sample;
//! * a bare WAV (no container the demuxer knows) is an audio-only session through
//!   `dsp::audio`'s file decoder, stereo kept, exact;
//! * the bus answers an audio-only `MediaPoster`/`PlayMedia` with a 0×0 `MediaOpened`, meter
//!   frames and `MediaEnded`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bandy::{SMessage, Synapse};
use gneiss_pal::dsp::audio_track::{Timing, audio_decoder_for};
use gneiss_pal::dsp::avsync::ManualTime;
use gneiss_pal::dsp::demux::build::{self, MediaTrack, MkvOptions, SampleSpec, TrackSpec};
use gneiss_pal::dsp::demux::{Demuxer, TrackKind};
use gneiss_pal::dsp::video::Frame;
use resonance::{AudioGraph, BLOCK_SIZE, stream_pair_channels};
use stria::media::{AudioOut, FrameSink, LEVEL_WINDOW_NS, Player, Tick};
use stria::media_bus::MediaService;

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

#[derive(Default)]
struct Meters {
    windows: Vec<(i64, [f32; 2])>,
    frames: usize,
}
impl FrameSink for Meters {
    fn present(&mut self, _f: &Frame, _ordinal: u64) {
        self.frames += 1;
    }
    fn levels(&mut self, pts_ns: i64, peaks: &[[f32; 2]]) {
        for (i, p) in peaks.iter().enumerate() {
            self.windows.push((pts_ns + i as i64 * LEVEL_WINDOW_NS, *p));
        }
    }
}

/// A player + a 48 kHz stereo graph standing in for the device.
struct Rig {
    p: Player,
    time: ManualTime,
    graph: AudioGraph,
    owed: usize,
    l: Vec<f32>,
    r: Vec<f32>,
}
impl Rig {
    fn new(file: Vec<u8>, rate: u32) -> Rig {
        let time = ManualTime::new();
        let (src, feed) = stream_pair_channels(rate, 2, rate as usize / 2);
        let mut graph = AudioGraph::new(rate as f64);
        graph.add_node(Box::new(src));
        let p = Player::open(file, Arc::new(time.clone()), Some(Box::new(SharedFeed(Arc::new(Mutex::new(feed))))), 60).unwrap();
        Rig { p, time, graph, owed: 0, l: Vec::new(), r: Vec::new() }
    }
    /// One 60 Hz refresh: tick, advance time, let the device pull `rate/60` frames.
    fn step(&mut self, sink: &mut dyn FrameSink, rate: usize) -> Tick {
        let t = self.p.tick(sink);
        self.time.advance(16_666_667);
        self.owed += rate / 60;
        while self.owed >= BLOCK_SIZE {
            let (l, r) = self.graph.process_stereo();
            self.l.extend(l.iter().map(|&x| x as f32));
            self.r.extend(r.iter().map(|&x| x as f32));
            self.owed -= BLOCK_SIZE;
        }
        t
    }
}

fn opus_webm(name: &str) -> Vec<u8> {
    use audio_core_paths::*;
    let bits = std::fs::read(opus_dir().join(format!("{name}.bit"))).unwrap();
    let (mut pk, mut p, mut last) = (Vec::new(), 0usize, 960u32);
    let mut t = 0i64;
    while p + 8 <= bits.len() {
        let len = u32::from_be_bytes(bits[p..p + 4].try_into().unwrap()) as usize;
        p += 8;
        let d = bits[p..p + len].to_vec();
        p += len;
        if len > 0 {
            let toc = d[0];
            let per = gneiss_pal::dsp::audio::opus::decoder::packet_samples_per_frame(toc, 48_000) as u32;
            let n = gneiss_pal::dsp::audio::opus::decoder::parse_packet(&d).map(|x| x.2.len() as u32).unwrap_or(1);
            last = per * n;
        }
        pk.push(SampleSpec { data: d, dts: t, pts: t, duration: last, keyframe: true });
        t += last as i64;
    }
    let mut head = b"OpusHead".to_vec();
    head.extend_from_slice(&[1, 2]);
    head.extend_from_slice(&312u16.to_le_bytes());
    head.extend_from_slice(&48_000u32.to_le_bytes());
    head.extend_from_slice(&[0, 0, 0]);
    let spec = TrackSpec {
        id: 1,
        kind: TrackKind::Audio,
        fourcc: *b"Opus",
        config_box: Some(*b"dOps"),
        codec_id: "A_OPUS",
        config: head,
        timescale: 48_000,
        width: 0,
        height: 0,
        sample_rate: 48_000,
        channels: 2,
        bit_depth: 16,
        default_duration_ns: 0,
    };
    build::mkv(&[MediaTrack { spec, samples: pk }], &MkvOptions::default())
}

mod audio_core_paths {
    pub fn opus_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../unaos/libs/media/audio_core/tests/data/opus")
    }
}

/// The registry's own decode of the file (the expected PCM), interleaved stereo.
fn reference(file: &[u8]) -> Vec<f32> {
    let mut d = Demuxer::open(file.to_vec()).unwrap();
    let t = d.audio_track().unwrap().clone();
    let mut dec = audio_decoder_for(&t, Timing::of(&d, &t)).unwrap();
    let mut v = Vec::new();
    while let Some(p) = d.next_packet() {
        v.extend(dec.decode(&p).unwrap().samples);
    }
    v
}

#[test]
fn audio_only_opus_webm_plays_stereo_exact_with_meters_and_ends() {
    let file = opus_webm("t06");
    let want = reference(&file);
    let mut rig = Rig::new(file, 48_000);
    let i = rig.p.info().clone();
    assert!(!i.has_video && i.audio_clock && (i.sample_rate, i.channels) == (48_000, 2), "{i:?}");
    assert_eq!(i.audio_decoder, "opus");
    rig.p.play();
    let mut m = Meters::default();
    let mut ended = false;
    for _ in 0..(want.len() / 2 / 800 + 120) {
        if rig.step(&mut m, 48_000) == Tick::Ended {
            ended = true;
            break;
        }
    }
    assert!(ended, "the audio-only session ends when the clock passes the last sample");
    assert_eq!(m.frames, 0);
    // what the device played, from its first sound, is the decode — both channels
    let first = rig.l.iter().zip(&rig.r).position(|(a, b)| *a != 0.0 || *b != 0.0).unwrap();
    let wf = want.chunks_exact(2).position(|f| f[0] != 0.0 || f[1] != 0.0).unwrap();
    assert_eq!(first, wf, "playback starts at the first presented sample");
    let n = want.len() / 2;
    assert!(rig.l.len() >= n);
    let wl: Vec<f32> = want.iter().step_by(2).copied().collect();
    let wr: Vec<f32> = want.iter().skip(1).step_by(2).copied().collect();
    assert!(rig.l[..n] == wl[..], "left channel differs");
    assert!(rig.r[..n] == wr[..], "right channel differs");
    assert!(wl != wr, "the stream is stereo and stays stereo");
    // meters: every 50 ms window, its peak L/R exactly the decode's
    let wins = n.div_ceil(2400);
    assert_eq!(m.windows.len(), wins);
    for (k, (t, p)) in m.windows.iter().enumerate() {
        assert_eq!(*t, k as i64 * LEVEL_WINDOW_NS);
        let s = &want[k * 4800..((k + 1) * 4800).min(want.len())];
        let pl = s.iter().step_by(2).fold(0f32, |a, x| a.max(x.abs()));
        let pr = s.iter().skip(1).step_by(2).fold(0f32, |a, x| a.max(x.abs()));
        assert_eq!(*p, [pl, pr], "window {k}");
    }
}

#[test]
fn seek_flushes_the_ring_so_the_target_sample_plays_next() {
    let file = opus_webm("t06");
    let want = reference(&file);
    let mut rig = Rig::new(file, 48_000);
    rig.p.play();
    let mut m = Meters::default();
    for _ in 0..20 {
        rig.step(&mut m, 48_000);
    }
    // a full ring of pre-seek audio is queued now; jump to 4.0 s
    let before = rig.l.len();
    rig.p.seek(4_000_000_000);
    for _ in 0..30 {
        rig.step(&mut m, 48_000);
    }
    let after = &rig.l[before..];
    let s = after.iter().position(|&x| x != 0.0).unwrap();
    // the first sound after the seek is the target's own sample run (pre-roll decoded, then
    // dropped up to the target), with no stale pre-seek audio before it
    let tgt = 4 * 48_000;
    let wl: Vec<f32> = want.iter().step_by(2).copied().collect();
    // A reset decoder re-converges (CELT's inter-frame energy prediction): the post-seek run
    // is the target's to within the codec's own re-start error, at offset 0 and no other.
    let k = 4800; // 100 ms
    let snr = |o: isize| {
        let (mut sig, mut err) = (0f64, 0f64);
        for i in 0..k {
            let a = after[s + i] as f64;
            let b = wl[(tgt as isize + o + i as isize) as usize] as f64;
            sig += b * b;
            err += (a - b) * (a - b);
        }
        10.0 * (sig / err.max(1e-30)).log10()
    };
    let best = (-480..=480).max_by(|&a, &b| snr(a).partial_cmp(&snr(b)).unwrap()).unwrap();
    eprintln!("post-seek: first sound at +{s}, best offset {best}, snr {:.1} dB (offset 0: {:.1} dB)", snr(best), snr(0));
    assert_eq!(best + s as isize, 0, "the post-seek audio is the target's, sample-aligned");
    assert!(snr(0) > 30.0, "post-seek SNR {:.1} dB", snr(0));
    assert!((rig.p.clock_ns() - 4_500_000_000).abs() < 50_000_000, "clock {} follows the device from the target", rig.p.clock_ns());
}

#[test]
fn pause_silences_the_device_and_resume_continues() {
    let file = opus_webm("t06");
    let want = reference(&file);
    let wl: Vec<f32> = want.iter().step_by(2).copied().collect();
    let mut rig = Rig::new(file, 48_000);
    rig.p.play();
    let mut m = Meters::default();
    for _ in 0..30 {
        rig.step(&mut m, 48_000);
    }
    rig.p.pause();
    let clock = rig.p.clock_ns();
    let at = rig.l.len();
    for _ in 0..30 {
        rig.step(&mut m, 48_000);
    }
    assert!(rig.l[at + 64..].iter().all(|&x| x == 0.0), "paused: silence");
    assert!((rig.p.clock_ns() - clock).abs() <= 20_000_000, "paused: the clock holds");
    rig.p.play();
    for _ in 0..10 {
        rig.step(&mut m, 48_000);
    }
    // everything played is the stream with a silent hole: the non-silent samples, in order,
    // are the decode's prefix — nothing skipped, nothing repeated
    let first = wl.iter().position(|&x| x != 0.0).unwrap();
    let heard: Vec<f32> = rig.l.iter().copied().filter(|&x| x != 0.0).collect();
    let wnz: Vec<f32> = wl[first..].iter().copied().filter(|&x| x != 0.0).collect();
    assert!(heard.len() > 25_000, "{}", heard.len());
    assert!(heard[..] == wnz[..heard.len()], "resume continued on the next sample");
}

fn wav_stereo(rate: u32, frames: usize) -> (Vec<u8>, Vec<i16>) {
    let pcm: Vec<i16> = (0..frames).flat_map(|i| [((i as f64 * 0.031).sin() * 9000.0) as i16, ((i as f64 * 0.017).cos() * 5000.0) as i16]).collect();
    let mut w = Vec::new();
    let data = (pcm.len() * 2) as u32;
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in &pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    (w, pcm)
}

#[test]
fn bare_wav_is_an_audio_only_session_exact_in_stereo() {
    let (wav, pcm) = wav_stereo(44_100, 44_100);
    let mut rig = Rig::new(wav, 44_100);
    let i = rig.p.info().clone();
    assert!(!i.has_video && i.audio_clock && i.channels == 2 && i.duration_ns == 1_000_000_000, "{i:?}");
    rig.p.play();
    let mut m = Meters::default();
    let mut ended = false;
    for _ in 0..200 {
        if rig.step(&mut m, 44_100) == Tick::Ended {
            ended = true;
            break;
        }
    }
    assert!(ended);
    let l: Vec<i32> = rig.l[..44_100].iter().map(|&x| (x * 32768.0) as i32).collect();
    let r: Vec<i32> = rig.r[..44_100].iter().map(|&x| (x * 32768.0) as i32).collect();
    assert_eq!(l, pcm.iter().step_by(2).map(|&x| x as i32).collect::<Vec<_>>());
    assert_eq!(r, pcm.iter().skip(1).step_by(2).map(|&x| x as i32).collect::<Vec<_>>());
    assert_eq!(m.windows.len(), 20);
}

#[test]
fn bus_audio_only_session_opens_without_a_picture_meters_and_ends() {
    let dir = std::env::temp_dir().join(format!("stria-audio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.wav");
    std::fs::write(&path, wav_stereo(48_000, 24_000).0).unwrap();
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
    assert!(wait_for(&|m| matches!(m, SMessage::MediaOpened { .. }), &mut got));
    synapse.fire(SMessage::PlayMedia { url: url.clone(), title: String::new(), mime: "audio/wav".into() });
    assert!(wait_for(&|m| matches!(m, SMessage::MediaEnded { .. }), &mut got));
    drop(svc);
    let _ = std::fs::remove_dir_all(&dir);
    let opened = got.iter().find_map(|m| match m {
        SMessage::MediaOpened { width, height, video, audio, duration_ns, .. } => Some((*width, *height, video.clone(), audio.clone(), *duration_ns)),
        _ => None,
    });
    assert_eq!(opened, Some((0, 0, String::new(), "wav".to_string(), 500_000_000)));
    let meters: Vec<(i64, usize)> = got
        .iter()
        .filter_map(|m| match m {
            SMessage::MediaFrame { pts_ns, width: 0, height: 0, rgba, levels, .. } if rgba.is_empty() => Some((*pts_ns, levels.len())),
            _ => None,
        })
        .collect();
    assert!(!meters.is_empty());
    assert_eq!(meters.iter().map(|m| m.1).sum::<usize>(), 10, "0.5 s = ten 50 ms windows, each sent once");
    assert!(meters.windows(2).all(|w| w[0].0 < w[1].0));
}

#[test]
fn mute_zeroes_the_device_while_the_clock_runs_on() {
    let (wav, _) = wav_stereo(48_000, 48_000);
    let mut rig = Rig::new(wav, 48_000);
    rig.p.set_muted(true);
    rig.p.play();
    let mut m = Meters::default();
    for _ in 0..30 {
        rig.step(&mut m, 48_000);
    }
    assert!(rig.l.iter().chain(&rig.r).all(|&x| x == 0.0), "muted: the device hears zeros");
    assert!((rig.p.clock_ns() - 500_000_000).abs() < 30_000_000, "the clock ran: {}", rig.p.clock_ns());
    assert!(m.windows.iter().any(|w| w.1[0] > 0.1), "the meters still measure the stream");
    rig.p.set_muted(false);
    for _ in 0..40 {
        rig.step(&mut m, 48_000);
    }
    assert!(rig.l.iter().any(|&x| x != 0.0), "unmuted: sound (after the ring's queued zeros)");
}
