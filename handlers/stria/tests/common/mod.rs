// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria (CODEX §2 A/V), host test hygiene — the ONE helper module the four integration binaries (audio.rs,
//! media.rs, nle.rs, nle_export.rs) share through `mod common;` (NLE6-UTP1, LEDGER SR729). Every helper here was a copy in
//! two or three of them; each returns the bytes its copies built (the `nle6utp1_*` pins). Cargo builds no binary from
//! `tests/common/mod.rs` (the directory form), and each binary uses a subset, hence the dead-code allowance.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gneiss_pal::dsp::demux::build::{self, MediaTrack, MkvOptions, Mp4Options};
use stria::media::AudioOut;
use stria::nle::{SYNTH_RATE, Sources, synth_source};
use stria::nle_face::synth_fixture;
use timeline_core::{SourceId, Timeline};

/// A resonance StreamFeed shared with the test's "device" loop (from tests/media.rs; tests/audio.rs held the same copy).
pub struct SharedFeed(pub Arc<Mutex<resonance::StreamFeed>>);
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

// HEVCPAL-SWAP (LEDGER SR304)'s log capture, per thread (a binary's tests run in parallel and log through the one global
// logger; a test counts only the lines its own thread wrote). From tests/nle.rs's `nlesizeswitch_lines` (it installed on
// read); tests/media.rs's `capture_logs` / `my_log_lines` were the same copy with the install split out.
struct LogCapture;
static CAPTURED: Mutex<Vec<(std::thread::ThreadId, String)>> = Mutex::new(Vec::new());
impl log::Log for LogCapture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        CAPTURED.lock().unwrap().push((std::thread::current().id(), r.args().to_string()));
    }
    fn flush(&self) {}
}

/// Install the capturing logger once for this binary (Info and up).
pub fn capture_logs() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        log::set_logger(&LogCapture).expect("no other logger in this test binary");
        log::set_max_level(log::LevelFilter::Info);
    });
}

/// The lines this thread logged that contain `containing` (installs the logger first: `log_lines("")` arms it).
pub fn log_lines(containing: &str) -> Vec<String> {
    capture_logs();
    let me = std::thread::current().id();
    CAPTURED.lock().unwrap().iter().filter(|(t, l)| *t == me && l.contains(containing)).map(|(_, l)| l.clone()).collect()
}

/// A synthetic movie over demux_core's `build` primitives: the utp1 test pattern as track 1, audio tracks after it.
pub struct Fixture {
    pub tracks: Vec<MediaTrack>,
}
impl Fixture {
    /// `frames` utp1 frames, `w`x`h` at `fps`, a keyframe every `gop` (`build::test_pattern_track(1, ..)`).
    pub fn pattern(w: u16, h: u16, fps: u32, frames: u32, gop: u32) -> Fixture {
        Fixture { tracks: vec![build::test_pattern_track(1, w, h, fps, frames, gop)] }
    }
    /// Add PCM16 audio as track 2, 960 frames per packet (`build::pcm16_track(2, ..)`).
    pub fn pcm16(mut self, rate: u32, ch: u16, pcm: &[i16]) -> Fixture {
        self.tracks.push(build::pcm16_track(2, rate, ch, pcm, 960));
        self
    }
    /// Relabel the last track's sample entry (`ac-3`: its rate and channels read, no decoder takes it).
    pub fn relabel_audio(mut self, fourcc: [u8; 4]) -> Fixture {
        self.tracks.last_mut().expect("a track to relabel").spec.fourcc = fourcc;
        self
    }
    pub fn mp4(&self) -> Vec<u8> {
        build::mp4(&self.tracks, &Mp4Options::default())
    }
    pub fn mkv(&self) -> Vec<u8> {
        build::mkv(&self.tracks, &MkvOptions::default())
    }
}

/// A pattern-only MP4 (`Fixture::pattern(..).mp4()`).
pub fn pattern_mp4(w: u16, h: u16, fps: u32, frames: u32, gop: u32) -> Vec<u8> {
    Fixture::pattern(w, h, fps, frames, gop).mp4()
}

/// NLE1's reel bases (reel A ordinals 0.., reel B 1000..), as `synth_fixture` builds them.
pub const BASE: [u32; 2] = [0, 1000];
pub const PER: usize = (SYNTH_RATE / 25) as usize; // samples per tick

/// NLE1's timeline: A[13,23)@0, B[0,10)@10 split at 15, a gap 20..25, A[23,33)@25 (video and audio alike) —
/// `stria::nle_face::synth_fixture`'s clips, split at 15 on both tracks.
pub fn timeline() -> Timeline {
    synth_fixture().0.split(0, 15).unwrap().split(1, 15).unwrap()
}

/// NLE1's two reels opened against `tl` (from tests/nle_export.rs; `Sources::new` takes the caller's time base).
pub fn sources(tl: &Timeline) -> Sources {
    let mut src = Sources::new(tl);
    for (i, b) in BASE.iter().enumerate() {
        src.add(SourceId(i as u32), synth_source(*b, 50, 25)).unwrap();
    }
    src
}

/// STRIA-TEST-TMPDIR (SR729): `CARGO_TARGET_TMPDIR/<group>-<pid>/<name>`, removed and not created. The per-process subdir
/// keeps two worktrees' runs on one shared target dir out of each other's fixtures.
pub fn scratch_dir(group: &str, name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{group}-{}", std::process::id())).join(name);
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// [`scratch_dir`], created empty.
pub fn fresh_dir(group: &str, name: &str) -> PathBuf {
    let d = scratch_dir(group, name);
    std::fs::create_dir_all(&d).unwrap();
    d
}
