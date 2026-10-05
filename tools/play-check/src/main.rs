// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `play-check` — PLAYBACK's headless face (LEDGER SR26).
//!
//! ```text
//! play-check <file> [--frame N --out f.png] [--hz 60] [--oracle chromium-oracle.jsonl]
//!            [--i420-oracle <dir>]
//! play-check --make-utp <out.mp4|out.webm> [frames] [fps]
//! ```
//!
//! Plays `<file>` through Stria's [`Player`] on a manual clock at a `--hz` display (no device,
//! no window, wall master), recording every presented frame. Prints one JSON line: the codecs,
//! the decoder (and whether it is real or the labelled test-pattern stand-in), presented /
//! dropped / repeated counts, the presented pts list, the worst |pts − clock| at presentation,
//! and, when frame N was asked for, its pts, its FNV-1a RGBA hash and the test-pattern counter
//! read back from it. With `--oracle` it finds the file's Chromium line (by file name) and
//! checks that every Chromium `mediaTime` lies within one frame of a presented pts, and the
//! frame counts agree; exit status 1 on any mismatch. With `--i420-oracle <dir>` (VP8CORE, SR40)
//! every presented I420 frame is compared byte for byte with the planes Chromium's own decoder
//! produced for the same pts (`<dir>/<file name>.<pts µs>.i420`, written by
//! `unaos/libs/media/vp8_core/oracle/video-frames.cjs`): frames compared, exact, worst max diff.

mod png;

use std::sync::Arc;

use gneiss_pal::dsp::avsync::ManualTime;
use gneiss_pal::dsp::video::{Frame, TestPattern};
use stria::media::{FrameSink, Player, Tick};

struct Capture {
    want: Option<u64>,
    shown: Vec<(u64, i64, i64)>,
    clock: i64,
    frame: Option<Frame>,
    /// `--i420-oracle`: (dir, file name, frames compared, exact, missing, worst max abs diff).
    i420: Option<(String, String, u64, u64, u64, u8)>,
}
impl FrameSink for Capture {
    fn present(&mut self, f: &Frame, ordinal: u64) {
        self.shown.push((ordinal, f.pts_ns, self.clock));
        if self.want == Some(ordinal) {
            self.frame = Some(f.clone());
        }
        if let (Some(o), gneiss_pal::dsp::video::Pixels::I420 { y, u, v, y_stride, uv_stride }) = (self.i420.as_mut(), &f.pixels) {
            let (w, h) = (f.width as usize, f.height as usize);
            let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
            let mut ours = Vec::with_capacity(w * h + 2 * cw * ch);
            for r in 0..h {
                ours.extend_from_slice(&y[r * y_stride..r * y_stride + w]);
            }
            for p in [u, v] {
                for r in 0..ch {
                    ours.extend_from_slice(&p[r * uv_stride..r * uv_stride + cw]);
                }
            }
            let path = format!("{}/{}.{}.i420", o.0, o.1, (f.pts_ns + 500) / 1000);
            match std::fs::read(&path) {
                Ok(theirs) if theirs.len() == ours.len() => {
                    let d = ours.iter().zip(&theirs).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
                    o.2 += 1;
                    o.3 += (d == 0) as u64;
                    o.5 = o.5.max(d);
                }
                _ => o.4 += 1,
            }
        }
    }
}

fn arg(args: &[String], k: &str) -> Option<String> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1).cloned())
}

/// The Chromium oracle line for `name`: (mediaTime list, presentedFrames).
fn oracle_line(path: &str, name: &str) -> Option<(Vec<f64>, u64)> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().find(|l| l.contains(&format!("\"file\":\"{name}\"")))?;
    let t0 = line.find("\"times\":[")? + 9;
    let t1 = t0 + line[t0..].find(']')?;
    let times = line[t0..t1].split(',').filter(|s| !s.is_empty()).map(|s| s.parse().unwrap_or(f64::NAN)).collect();
    let p0 = line.find("\"presented\":")? + 12;
    let p1 = p0 + line[p0..].find(|c: char| !c.is_ascii_digit()).unwrap_or(line.len() - p0);
    Some((times, line[p0..p1].parse().ok()?))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // `play-check --make-utp <out.mp4|out.webm> [frames] [fps]`: write a test-pattern stream
    // (320x240, keyframe every 10) with demux_core's writer, for the counter oracle.
    if args.get(1).map(String::as_str) == Some("--make-utp") {
        use gneiss_pal::dsp::demux::build;
        let out = args.get(2).expect("--make-utp <out>");
        let n: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);
        let fps: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(30);
        let t = build::test_pattern_track(1, 320, 240, fps, n, 10);
        let bytes = if out.ends_with(".mp4") { build::mp4(&[t], &build::Mp4Options::default()) } else { build::mkv(&[t], &build::MkvOptions::default()) };
        std::fs::write(out, bytes).expect("write");
        return;
    }
    let Some(file) = args.get(1).filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: play-check <file> [--frame N --out f.png] [--hz 60] [--oracle chromium-oracle.jsonl]");
        std::process::exit(2);
    };
    let hz: u32 = arg(&args, "--hz").and_then(|s| s.parse().ok()).unwrap_or(60);
    let want: Option<u64> = arg(&args, "--frame").and_then(|s| s.parse().ok());
    let bytes = std::fs::read(file).unwrap_or_else(|e| {
        eprintln!("play-check: {file}: {e}");
        std::process::exit(2)
    });
    let time = ManualTime::new();
    let mut p = Player::open(bytes, Arc::new(time.clone()), None, hz).unwrap_or_else(|e| {
        eprintln!("play-check: {file}: {e}");
        std::process::exit(2)
    });
    let period = 1_000_000_000u64 / hz as u64;
    let name = std::path::Path::new(file).file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let i420 = arg(&args, "--i420-oracle").map(|d| (d, name, 0, 0, 0, 0u8));
    let mut cap = Capture { want, shown: Vec::new(), clock: 0, frame: None, i420 };
    p.play();
    let max_ticks = (p.info().duration_ns / period + hz as u64 * 2) as usize;
    for _ in 0..max_ticks {
        cap.clock = p.clock_ns();
        if p.tick(&mut cap) == Tick::Ended {
            break;
        }
        time.advance(period);
    }
    let st = p.stats();
    let dr = p.drift();
    let info = p.info().clone();
    let pts: Vec<String> = cap.shown.iter().map(|s| format!("{:.6}", s.1 as f64 / 1e9)).collect();
    let mut ok = true;
    let mut extra = String::new();
    if let Some(n) = want {
        match &cap.frame {
            Some(f) => {
                let rgba = f.to_rgba();
                let counter = TestPattern::read_counter(&rgba, f.width, f.height);
                extra += &format!(",\"frame\":{n},\"frame_pts\":{:.6},\"frame_size\":[{},{}],\"frame_fnv\":\"{:x}\",\"counter\":{}", f.pts_ns as f64 / 1e9, f.width, f.height, png::fnv1a(&rgba), counter.map(|c| c.to_string()).unwrap_or("null".into()));
                if counter.is_some() && counter != Some(n as u32) {
                    ok = false;
                }
                if let Some(out) = arg(&args, "--out") {
                    std::fs::write(&out, png::encode_rgba(f.width, f.height, &rgba)).unwrap_or_else(|e| {
                        eprintln!("play-check: {out}: {e}");
                        std::process::exit(2)
                    });
                }
            }
            None => {
                extra += &format!(",\"frame\":{n},\"frame_error\":\"not presented (dropped or past the end)\"");
                ok = false;
            }
        }
    }
    if let Some((_, _, compared, exact, missing, worst)) = &cap.i420 {
        let all = *compared > 0 && compared == exact && *missing == 0;
        ok &= all;
        extra += &format!(",\"i420_oracle\":{{\"compared\":{compared},\"exact\":{exact},\"missing\":{missing},\"max_abs_diff\":{worst}}}");
    }
    if let Some(o) = arg(&args, "--oracle") {
        let name = std::path::Path::new(file).file_name().and_then(|s| s.to_str()).unwrap_or("");
        match oracle_line(&o, name) {
            Some((times, presented)) => {
                // One frame = the track's mean frame duration.
                let vt = p.video_track();
                let frame_ns = if vt.sample_count > 0 { vt.duration_ns / vt.sample_count } else { 33_333_333 } as f64;
                let ours: Vec<f64> = cap.shown.iter().map(|s| s.1 as f64).collect();
                let worst = times
                    .iter()
                    .map(|&t| ours.iter().map(|&q| (q - t * 1e9).abs()).fold(f64::INFINITY, f64::min))
                    .fold(0.0, f64::max);
                let within = worst <= frame_ns;
                let count_ok = cap.shown.len() as u64 == presented;
                ok &= within && count_ok;
                extra += &format!(",\"oracle\":{{\"chromium_presented\":{presented},\"chromium_callbacks\":{},\"max_mediatime_vs_pts_ms\":{:.3},\"one_frame_ms\":{:.3},\"within_one_frame\":{within},\"count_match\":{count_ok}}}", times.len(), worst / 1e6, frame_ns / 1e6);
            }
            None => {
                extra += ",\"oracle\":\"no line for this file\"";
                ok = false;
            }
        }
    }
    println!(
        "{{\"file\":\"{}\",\"video\":\"{}\",\"audio\":\"{}\",\"decoder\":\"{}\",\"real_video\":{},\"audio_note\":\"{}\",\"size\":[{},{}],\"duration\":{:.6},\"hz\":{hz},\"presented\":{},\"dropped\":{},\"repeated\":{},\"av_max_ms\":{:.3},\"pts\":[{}]{extra},\"ok\":{ok}}}",
        file,
        info.video,
        info.audio,
        info.video_decoder,
        info.real_video,
        info.audio_note.replace('"', "'"),
        info.width,
        info.height,
        info.duration_ns as f64 / 1e9,
        st.presented,
        st.dropped,
        st.repeated,
        dr.av_max_ns as f64 / 1e6,
        pts.join(",")
    );
    std::process::exit(if ok { 0 } else { 1 });
}
