// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `play-check` — PLAYBACK's headless face (LEDGER SR26).
//!
//! ```text
//! play-check <file> [--frame N --out f.png] [--hz 60] [--oracle chromium-oracle.jsonl]
//!            [--i420-oracle <dir>] [--md5]
//!            [--audio-out played.f32 [--audio-ref chromium.f32 --audio-floor exact|snr:<dB>|lsb:<n>
//!             [--ref-frames-max]]]
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
//! PLAYCHECKHI (SR352): an I420Hi frame (`bit_depth` 9..16) is compared as yuv420p10le / yuv420p12le bytes (each sample
//! two bytes, low first: `hevc_core::le_bytes`, HEVCCONF's ONE layout) and reported apart as `i420_hi` with its depth;
//! `--md5` lists each presented frame's md5 (crypto_core) of its yuv420p / yuv420p10le / yuv420p12le bytes.
//!
//! AUDIOTRACK (SR45): with `--audio-out` the player gets a headless DEVICE — an [`AudioOut`]
//! that consumes the stereo stream at exactly its sample rate on the same manual clock — so the
//! audio track decodes and is the master clock, as on a real host; every sample the device
//! played is written as `u32 channels, u32 frames, u32 rate, planar f32` (the layout
//! `audio_core/oracle/chromium-oracle.cjs` dumps Chromium's `decodeAudioData` in), for the
//! sample-for-sample oracle. Audio-only files (no video track) play the same way.
//!
//! `--audio-ref` compares what the device played with Chromium's decode of the same file, per
//! channel (a mono reference is held against both played channels — the player duplicates
//! mono), at the floor named: `exact` (bit-identical), `snr:<dB>`, or `lsb:<n>` (every sample
//! within n 16-bit LSBs, or SNR ≥ 60 dB for quiet clips). `decodeAudioData` has no timeline, so
//! the device's first `audio_start_ns` of lead-in silence (an audio track that starts after the
//! video does) is skipped before comparing; the best-offset search must then land on 0. Frame counts must be equal, except with
//! `--ref-frames-max`, where ours must not exceed Chromium's (Chromium returns every decoded
//! Vorbis sample in WebM; the stream's own end trim is not applied there).

use std::sync::Arc;

use gneiss_pal::dsp::avsync::{ManualTime, TimeSource};
use gneiss_pal::dsp::image::encode_rgba;
use gneiss_pal::dsp::video::{Chroma, Frame, Pixels, TestPattern, chroma_of};
use stria::media::{AudioOut, FrameSink, Player, Tick};

/// A headless audio device: takes stereo frames into a half-second ring and consumes them at
/// `rate` frames per second of the manual clock while playing; records what it played.
struct Device {
    time: ManualTime,
    rate: u64,
    cap: u64,
    pushed: Vec<f32>,
    /// Consumption is `base + (now − base_t)·rate` (while playing), capped at what was pushed.
    base: u64,
    base_t: u64,
    playing: bool,
    /// Frames dropped by flushes (pushed, never played), and where they were cut out.
    cuts: Vec<(usize, usize)>,
}
impl Device {
    fn pushed_frames(&self) -> u64 {
        self.pushed.len() as u64 / 2
    }
    fn now_consumed(&self) -> u64 {
        let mut c = self.base;
        if self.playing {
            c += (self.time.now_ns().saturating_sub(self.base_t) as u128 * self.rate as u128 / 1_000_000_000) as u64;
        }
        c.min(self.pushed_frames())
    }
    fn rebase(&mut self) {
        self.base = self.now_consumed();
        self.base_t = self.time.now_ns();
    }
}
struct SharedDevice(std::sync::Arc<std::sync::Mutex<Device>>);
impl AudioOut for SharedDevice {
    fn push(&mut self, stereo: &[f32]) -> usize {
        let mut d = self.0.lock().unwrap();
        d.rebase();
        let room = d.cap.saturating_sub(d.pushed_frames() - d.base) as usize * 2;
        let n = (stereo.len() / 2 * 2).min(room);
        d.pushed.extend_from_slice(&stereo[..n]);
        n
    }
    fn consumed(&self) -> u64 {
        let d = self.0.lock().unwrap();
        d.now_consumed()
    }
    fn flush(&mut self) {
        let mut d = self.0.lock().unwrap();
        d.rebase();
        let played = d.base as usize * 2;
        let len = d.pushed.len();
        d.cuts.push((played, len - played));
        d.pushed.truncate(played);
    }
    fn set_paused(&mut self, paused: bool) {
        let mut d = self.0.lock().unwrap();
        d.rebase();
        d.playing = !paused;
    }
}

struct Capture {
    want: Option<u64>,
    shown: Vec<(u64, i64, i64)>,
    clock: i64,
    frame: Option<Frame>,
    /// `--i420-oracle` (VP8CORE SR40; I420Hi frames PLAYCHECKHI SR352).
    i420: Option<I420Oracle>,
    /// `--md5` (PLAYCHECKHI SR352): per presented frame, the md5 of its yuv bytes (None: no YUV planes).
    md5: Option<Vec<Option<String>>>,
}
impl FrameSink for Capture {
    fn present(&mut self, f: &Frame, ordinal: u64) {
        self.shown.push((ordinal, f.pts_ns, self.clock));
        if self.want == Some(ordinal) {
            self.frame = Some(f.clone());
        }
        self.compare_yuv(f);
    }
}

/// A planar f32 dump: (channels, rate, planes).
fn read_dump(path: &str) -> Option<(usize, u32, Vec<Vec<f32>>)> {
    let b = std::fs::read(path).ok()?;
    let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let (ch, frames, rate) = (u(0), u(4), u(8) as u32);
    if b.len() < 12 + ch * frames * 4 {
        return None;
    }
    let planes = (0..ch).map(|c| b[12 + c * frames * 4..12 + (c + 1) * frames * 4].chunks_exact(4).map(|x| f32::from_le_bytes(x.try_into().unwrap())).collect()).collect();
    Some((ch, rate, planes))
}

struct AudioCmp {
    frames_ours: usize,
    frames_ref: usize,
    snr_db: f64,
    max_abs: f64,
    mismatches: usize,
    /// The shift (−2048..=2048 frames) that maximises SNR: 0 when the timelines agree.
    best_offset: i64,
}

fn compare_audio(ours: &[Vec<f32>], refr: &[Vec<f32>]) -> AudioCmp {
    let fo = ours[0].len();
    let fr = refr[0].len();
    let pair = |c: usize| (&ours[c], &refr[c.min(refr.len() - 1)]);
    let score = |off: i64| -> (f64, f64, usize) {
        let (mut sig, mut err, mut maxd, mut mis) = (0f64, 0f64, 0f64, 0usize);
        for c in 0..ours.len() {
            let (o, r) = pair(c);
            for i in 0..fr {
                let j = i as i64 + off;
                if j < 0 || j as usize >= fo {
                    continue;
                }
                let a = o[j as usize] as f64;
                let b = r[i] as f64;
                if a != b {
                    mis += 1;
                }
                sig += b * b;
                err += (a - b) * (a - b);
                maxd = maxd.max((a - b).abs());
            }
        }
        (if err == 0.0 { f64::INFINITY } else { 10.0 * (sig / err).log10() }, maxd, mis)
    };
    let (snr, maxd, mis) = score(0);
    let mut best = (0i64, snr);
    if snr.is_finite() && snr < 40.0 {
        for off in (-2048..=2048).step_by(1) {
            let (s, _, _) = score(off);
            if s > best.1 {
                best = (off, s);
            }
        }
    }
    AudioCmp { frames_ours: fo, frames_ref: fr, snr_db: snr, max_abs: maxd, mismatches: mis, best_offset: best.0 }
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
    let audio_out = arg(&args, "--audio-out");
    let device = audio_out.as_ref().map(|_| {
        let rate = Player::probe_audio_rate(&bytes).max(1) as u64;
        std::sync::Arc::new(std::sync::Mutex::new(Device { time: time.clone(), rate, cap: rate / 2, pushed: Vec::new(), base: 0, base_t: 0, playing: false, cuts: Vec::new() }))
    });
    let aout: Option<Box<dyn AudioOut>> = device.as_ref().map(|d| Box::new(SharedDevice(d.clone())) as Box<dyn AudioOut>);
    let mut p = Player::open(bytes, Arc::new(time.clone()), aout, hz).unwrap_or_else(|e| {
        eprintln!("play-check: {file}: {e}");
        std::process::exit(2)
    });
    let period = 1_000_000_000u64 / hz as u64;
    let name = std::path::Path::new(file).file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let i420 = arg(&args, "--i420-oracle").map(|d| I420Oracle::new(d, name));
    let md5 = args.iter().any(|a| a == "--md5").then(Vec::new);
    let mut cap = Capture { want, shown: Vec::new(), clock: 0, frame: None, i420, md5 };
    p.play();
    let max_ticks = (p.info().duration_ns / period + hz as u64 * 3) as usize;
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
                let rgba = f.to_rgba_colour(p.colour(), p.video_track().map_or(Chroma::Nearest, chroma_of)); // AV1INBANDCOLOUR (SR366) PLAYCHECK-COLOUR: BusSink's conversion (`Player::colour`, `chroma_of`)
                let counter = TestPattern::read_counter(&rgba, f.width, f.height);
                extra += &format!(",\"frame\":{n},\"frame_pts\":{:.6},\"frame_size\":[{},{}],\"frame_fnv\":\"{:x}\",\"counter\":{}", f.pts_ns as f64 / 1e9, f.width, f.height, fnv1a(&rgba), counter.map(|c| c.to_string()).unwrap_or("null".into()));
                if counter.is_some() && counter != Some(n as u32) {
                    ok = false;
                }
                if let Some(out) = arg(&args, "--out") {
                    let png = encode_rgba(f.width, f.height, &rgba).unwrap_or_else(|e| {
                        eprintln!("play-check: {out}: png {e:?}");
                        std::process::exit(2)
                    });
                    std::fs::write(&out, png).unwrap_or_else(|e| {
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
    if let Some(o) = &cap.i420 {
        ok &= o.ok();
        extra += &o.json();
    }
    if let Some(m) = &cap.md5 {
        extra += &format!(",\"md5\":[{}]", m.iter().map(|h| h.as_ref().map(|h| format!("\"{h}\"")).unwrap_or_else(|| "null".into())).collect::<Vec<String>>().join(","));
    }
    if let Some(o) = arg(&args, "--oracle") {
        let name = std::path::Path::new(file).file_name().and_then(|s| s.to_str()).unwrap_or("");
        match oracle_line(&o, name) {
            Some((times, presented)) => {
                // One frame = the track's mean frame duration.
                let frame_ns = match p.video_track() {
                    Some(vt) if vt.sample_count > 0 => vt.duration_ns / vt.sample_count,
                    _ => 33_333_333,
                } as f64;
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
    if let (Some(out), Some(dev)) = (&audio_out, &device) {
        let d = dev.lock().unwrap();
        let played = d.now_consumed() as usize;
        let frames = &d.pushed[..played * 2];
        let mut b = Vec::with_capacity(12 + frames.len() * 4);
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&(played as u32).to_le_bytes());
        b.extend_from_slice(&(d.rate as u32).to_le_bytes());
        for c in 0..2 {
            for f in frames.chunks_exact(2) {
                b.extend_from_slice(&f[c].to_le_bytes());
            }
        }
        std::fs::write(out, b).unwrap_or_else(|e| {
            eprintln!("play-check: {out}: {e}");
            std::process::exit(2)
        });
        extra += &format!(",\"audio_played\":{played},\"audio_flushed\":{}", d.cuts.iter().map(|c| c.1 / 2).sum::<usize>());
        if let Some(rp) = arg(&args, "--audio-ref") {
            match read_dump(&rp) {
                Some((_, rrate, refr)) => {
                    let lead = (info.audio_start_ns.max(0) as u128 * d.rate as u128 / 1_000_000_000) as usize;
                    let ours: Vec<Vec<f32>> = (0..2).map(|c| frames.chunks_exact(2).skip(lead).map(|f| f[c]).collect()).collect();
                    let c = compare_audio(&ours, &refr);
                    let floor = arg(&args, "--audio-floor").unwrap_or_else(|| "exact".into());
                    let level_ok = if floor == "exact" {
                        c.mismatches == 0
                    } else if let Some(db) = floor.strip_prefix("snr:") {
                        c.snr_db >= db.parse::<f64>().unwrap_or(f64::INFINITY)
                    } else if let Some(n) = floor.strip_prefix("lsb:") {
                        c.max_abs <= n.parse::<f64>().unwrap_or(0.0) / 32768.0 || c.snr_db >= 60.0
                    } else {
                        false
                    };
                    let frames_ok = if args.iter().any(|a| a == "--ref-frames-max") { c.frames_ours <= c.frames_ref } else { c.frames_ours == c.frames_ref };
                    let rate_ok = rrate as u64 == d.rate;
                    ok &= level_ok && frames_ok && rate_ok && c.best_offset == 0;
                    extra += &format!(
                        ",\"audio_oracle\":{{\"floor\":\"{floor}\",\"lead_frames\":{lead},\"ref_channels\":{},\"frames_ours\":{},\"frames_ref\":{},\"snr_db\":{},\"max_abs\":{:.3e},\"max_lsb16\":{:.2},\"mismatches\":{},\"best_offset\":{},\"ok\":{}}}",
                        refr.len(),
                        c.frames_ours,
                        c.frames_ref,
                        if c.snr_db.is_finite() { format!("{:.2}", c.snr_db) } else { "\"inf\"".into() },
                        c.max_abs,
                        c.max_abs * 32768.0,
                        c.mismatches,
                        c.best_offset,
                        level_ok && frames_ok && rate_ok && c.best_offset == 0
                    );
                }
                None => {
                    extra += ",\"audio_oracle\":\"unreadable reference\"";
                    ok = false;
                }
            }
        }
    }
    extra += &profile_json(&info.video_profile); // HEVCLEVELLINE (SR368): "profile" / "tier" / "level"
    println!(
        "{{\"file\":\"{}\",\"video\":\"{}\",\"audio\":\"{}\",\"decoder\":\"{}\",\"audio_decoder\":\"{}\",\"sample_rate\":{},\"channels\":{},\"audio_clock\":{},\"real_video\":{},\"audio_note\":\"{}\",\"size\":[{},{}],\"duration\":{:.6},\"hz\":{hz},\"presented\":{},\"dropped\":{},\"repeated\":{},\"av_max_ms\":{:.3},\"pts\":[{}]{extra},\"ok\":{ok}}}",
        file,
        info.video,
        info.audio,
        info.video_decoder,
        info.audio_decoder,
        info.sample_rate,
        info.channels,
        info.audio_clock,
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

/// FNV-1a over bytes (the same hash the Chromium oracles print).
pub fn fnv1a(d: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for &b in d {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// PLAYCHECKHI (LEDGER SR352): the `--i420-oracle` counters — 8-bit (`Pixels::I420`) frames as VP8CORE (SR40) counted
/// them, I420Hi frames (yuv420p10le / 12le oracle bytes) apart, so an 8-bit run reports byte for byte as before.
struct I420Oracle {
    dir: String,
    name: String,
    compared: u64,
    exact: u64,
    missing: u64,
    worst: u16,
    /// I420Hi frames: (bit depth, compared, exact, missing, worst max abs sample diff).
    hi: (u8, u64, u64, u64, u16),
}

impl I420Oracle {
    fn new(dir: String, name: String) -> I420Oracle {
        I420Oracle { dir, name, compared: 0, exact: 0, missing: 0, worst: 0, hi: (0, 0, 0, 0, 0) }
    }

    /// VP8CORE's rule over both depths: at least one frame compared, every compared frame exact, none missing.
    fn ok(&self) -> bool {
        let (_, hc, he, hm, _) = self.hi;
        self.compared + hc > 0 && self.compared == self.exact && hc == he && self.missing + hm == 0
    }

    /// The report fragment: `i420_oracle` as before, then `i420_hi` only when an I420Hi frame was presented.
    fn json(&self) -> String {
        let mut s = format!(",\"i420_oracle\":{{\"compared\":{},\"exact\":{},\"missing\":{},\"max_abs_diff\":{}}}", self.compared, self.exact, self.missing, self.worst);
        let (d, hc, he, hm, hw) = self.hi;
        if hc + hm > 0 {
            s += &format!(",\"i420_hi\":{{\"depth\":{d},\"compared\":{hc},\"exact\":{he},\"missing\":{hm},\"max_abs_diff\":{hw}}}");
        }
        s
    }
}

/// PLAYCHECKHI (SR352): a presented frame's planes packed as the oracles and `--md5` take them, with its depth: yuv420p
/// at 8 bits (the crop VP8CORE SR40 wrote inline), above 8 bits the u16 samples cropped at their strides and laid out by
/// `hevc_core::le_bytes` (yuv420p10le / 12le, HEVCCONF's ONE layout). An I420Hi plane too short for its rows yields no
/// bytes (the oracle counts the frame missing); a frame without YUV planes yields None. 4:2:2 / 4:4:4 never arrive here:
/// gneiss_pal's HEVC adapter refuses them by name (COLOURONE-422).
fn yuv_layout(f: &Frame) -> Option<(Vec<u8>, u8)> {
    let (w, h) = (f.width as usize, f.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    match &f.pixels {
        Pixels::I420 { y, u, v, y_stride, uv_stride } => {
            let mut ours = Vec::with_capacity(w * h + 2 * cw * ch);
            for r in 0..h {
                ours.extend_from_slice(&y[r * y_stride..r * y_stride + w]);
            }
            for p in [u, v] {
                for r in 0..ch {
                    ours.extend_from_slice(&p[r * uv_stride..r * uv_stride + cw]);
                }
            }
            Some((ours, 8))
        }
        Pixels::I420Hi { y, u, v, y_stride, uv_stride, bit_depth } => {
            let crop = |p: &[u16], stride: usize, pw: usize, ph: usize| -> Option<Vec<u16>> {
                (0..ph).map(|r| p.get(r * stride..r * stride + pw)).collect::<Option<Vec<&[u16]>>>().map(|rows| rows.concat())
            };
            let bytes = match (crop(y.as_slice(), *y_stride, w, h), crop(u.as_slice(), *uv_stride, cw, ch), crop(v.as_slice(), *uv_stride, cw, ch)) {
                (Some(py), Some(pu), Some(pv)) => hevc_core::le_bytes([py.as_slice(), pu.as_slice(), pv.as_slice()]),
                _ => Vec::new(),
            };
            Some((bytes, *bit_depth))
        }
        _ => None,
    }
}

/// PLAYCHECKHI (SR352): the largest |ours − theirs| sample difference — per byte at 8 bits, per two-byte little-endian
/// sample above; None when the lengths differ (the oracle's frame is missing, or of another size or depth).
fn max_diff(ours: &[u8], theirs: &[u8], depth: u8) -> Option<u16> {
    if ours.len() != theirs.len() {
        return None;
    }
    Some(if depth <= 8 {
        ours.iter().zip(theirs).map(|(a, b)| a.abs_diff(*b) as u16).max().unwrap_or(0)
    } else {
        let s = |c: &[u8]| u16::from_le_bytes([c[0], c[1]]);
        ours.chunks_exact(2).zip(theirs.chunks_exact(2)).map(|(a, b)| s(a).abs_diff(s(b))).max().unwrap_or(0)
    })
}

/// PLAYCHECKHI (SR352): lowercase hex of crypto_core's MD5 (the digest md5sum and the conformance lists print).
fn md5_hex(data: &[u8]) -> String {
    crypto_core::md5::md5(data).iter().map(|b| format!("{b:02x}")).collect()
}

impl Capture {
    /// PLAYCHECKHI (SR352): one presented frame's `--md5` entry and its `--i420-oracle` compare against
    /// `<dir>/<file name>.<pts µs>.i420` (yuv420p bytes at 8 bits, yuv420p10le / 12le above).
    fn compare_yuv(&mut self, f: &Frame) {
        if self.i420.is_none() && self.md5.is_none() {
            return;
        }
        let lay = yuv_layout(f);
        if let Some(m) = self.md5.as_mut() {
            m.push(lay.as_ref().filter(|(b, d)| *d <= 8 || !b.is_empty()).map(|(b, _)| md5_hex(b)));
        }
        let (Some(o), Some((ours, depth))) = (self.i420.as_mut(), lay) else {
            return;
        };
        let path = format!("{}/{}.{}.i420", o.dir, o.name, (f.pts_ns + 500) / 1000);
        let d = if depth > 8 && ours.is_empty() { None } else { std::fs::read(&path).ok().and_then(|theirs| max_diff(&ours, &theirs, depth)) };
        if depth <= 8 {
            match d {
                Some(d) => {
                    o.compared += 1;
                    o.exact += (d == 0) as u64;
                    o.worst = o.worst.max(d);
                }
                None => o.missing += 1,
            }
        } else {
            o.hi.0 = depth;
            match d {
                Some(d) => {
                    o.hi.1 += 1;
                    o.hi.2 += (d == 0) as u64;
                    o.hi.4 = o.hi.4.max(d);
                }
                None => o.hi.3 += 1,
            }
        }
    }
}

/// PLAYCHECKHI (SR352): hevc_core's hand-assembled-stream kit (HEVCMAIN10's KAT pictures), by path as HEVCCONF takes
/// h264_core's (`tests/conformance.rs` `mod jvt`).
#[cfg(test)]
#[path = "../../../unaos/libs/media/hevc_core/tests/common/mod.rs"]
mod hevc_kit;

#[cfg(test)]
mod tests {
    use super::*;
    use gneiss_pal::dsp::image::decode_png;

    /// PNGONE (LEDGER SR228): `--out` encodes through pixel_core's `encode_rgba` (gneiss_pal's re-export); the
    /// file decodes back to the exact RGBA. `frame_fnv` hashes the pixels, so the Chromium png-oracle's number is
    /// unchanged by the writer; the PNG bytes' hash is pinned for the new writer.
    #[test]
    fn png_round_trips_through_pixel_core() {
        assert_eq!(fnv1a(b""), 0x811c_9dc5);
        assert_eq!(fnv1a(b"a"), 0xe40c_292c);
        let (w, h) = (37u32, 23u32);
        let grad: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| [(x * 7) as u8, (y * 11) as u8, (x ^ y) as u8, (x + y * 3) as u8])).collect();
        assert_eq!(fnv1a(&grad), 0xa1fd_494e);
        // SR228: the retired stored-deflate writer's bytes were red 73 B fnv 34c82d69, gradient 3495 B fnv 9736d2bf
        // (pixel_core: red 70 B, gradient 468 B).
        for (w, h, rgba, want) in [(1u32, 1u32, vec![255u8, 0, 0, 255], 0x2093_7f3a), (w, h, grad, 0xe989_86dc)] {
            let png = encode_rgba(w, h, &rgba).unwrap();
            assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
            let img = decode_png(&png).unwrap();
            assert_eq!((img.width, img.height), (w, h));
            assert_eq!(img.rgba, rgba);
            assert_eq!(fnv1a(&png), want, "{w}x{h}: {} B", png.len());
        }
    }

    /// PLAYCHECKHI (SR352): a flat 10-bit field through the I420Hi path against an inline yuv420p10le oracle.
    #[test]
    fn i420hi_flat_ten_bit_field_against_inline_oracle() {
        let f = Frame { width: 4, height: 2, pts_ns: 0, pixels: Pixels::I420Hi { y: vec![600; 8], u: vec![512; 2], v: vec![512; 2], y_stride: 4, uv_stride: 2, bit_depth: 10 } };
        let mut oracle: Vec<u8> = [0x58u8, 0x02].repeat(8);
        oracle.extend([0x00u8, 0x02].repeat(4));
        let (ours, depth) = yuv_layout(&f).unwrap();
        assert_eq!((ours.as_slice(), depth), (oracle.as_slice(), 10));
        assert_eq!(max_diff(&ours, &oracle, 10), Some(0));
        let mut off = oracle.clone();
        off[6] = 0x59; // Y sample 3: 601
        assert_eq!(max_diff(&ours, &off, 10), Some(1));
        assert_eq!(max_diff(&ours, &oracle[..oracle.len() - 2], 10), None);
        assert_eq!(md5_hex(&ours), "4a25c6d2b52679da6bb5a2c244bd87ad");
        // A plane too short for its rows: no bytes, so the frame counts missing.
        let short = Frame { pixels: Pixels::I420Hi { y: vec![600; 7], u: vec![512; 2], v: vec![512; 2], y_stride: 4, uv_stride: 2, bit_depth: 10 }, ..f };
        assert_eq!(yuv_layout(&short), Some((Vec::new(), 10)));
        let mut o = I420Oracle::new(String::new(), String::new());
        o.hi = (10, 1, 1, 0, 0);
        assert!(o.ok());
        assert_eq!(o.json(), ",\"i420_oracle\":{\"compared\":0,\"exact\":0,\"missing\":0,\"max_abs_diff\":0},\"i420_hi\":{\"depth\":10,\"compared\":1,\"exact\":1,\"missing\":0,\"max_abs_diff\":0}");
        o.hi.3 = 1;
        assert!(!o.ok());
    }

    /// PLAYCHECKHI (SR352): HEVCMAIN10's KAT picture (tests/main10.rs `main10_all_dc_is_flat_512`, assembled with the
    /// same kit calls as its `dc32(&cfg32(10))`), as gneiss_pal's `to_frame` hands it on (y_stride w, uv_stride w / 2):
    /// the md5 `--md5` prints is the md5 of the bytes HEVCCONF hashes (`hevc_core::yuv_bytes`) — ONE layout.
    #[test]
    fn main10_kat_picture_md5_equals_hevcconf_layout() {
        use crate::hevc_kit::{CabacEnc, PpsCfg, SpsCfg, idr_slice_header, nal, pps, sps, vps};
        let cfg = SpsCfg { width: 32, height: 32, log2_ctb: 5, bit_depth: 10, ..Default::default() };
        let mut s = vps(false);
        s.extend(sps(&cfg));
        s.extend(pps(&PpsCfg { init_qp: 26, ..Default::default() }));
        let mut e = CabacEnc::new(idr_slice_header(false, false, false, false), 0, 26);
        e.split_cu(0, false);
        e.dc_cu(1);
        s.extend(nal(19, &e.finish().out));
        let mut d = hevc_core::Decoder::new();
        d.decode_annexb(&s).expect("decode");
        d.flush();
        let f = d.take_frames().remove(0);
        let [y, u, v] = f.planes16.clone().expect("Main 10 keeps planes16");
        let w = f.width as usize;
        let ours = Frame { width: f.width, height: f.height, pts_ns: 0, pixels: Pixels::I420Hi { y, u, v, y_stride: w, uv_stride: w / 2, bit_depth: f.bit_depth } };
        let (bytes, depth) = yuv_layout(&ours).unwrap();
        assert_eq!(depth, 10);
        assert_eq!(bytes, hevc_core::yuv_bytes(&f));
        assert_eq!(md5_hex(&bytes), "ca021302dd2c310dc2b2c9a2ff98fefc");
    }

    /// PLAYCHECKHI (SR352): the 8-bit path is VP8CORE's (SR40) — the same strided crop, the same report fragment and rule.
    #[test]
    fn eight_bit_layout_and_report_unchanged() {
        let y: Vec<u8> = (1..=12).collect();
        let f = Frame { width: 3, height: 3, pts_ns: 0, pixels: Pixels::I420 { y, u: vec![20, 21, 99, 22, 23, 99], v: vec![30, 31, 99, 32, 33, 99], y_stride: 4, uv_stride: 3 } };
        let want = vec![1u8, 2, 3, 5, 6, 7, 9, 10, 11, 20, 21, 22, 23, 30, 31, 32, 33];
        assert_eq!(yuv_layout(&f), Some((want.clone(), 8)));
        assert_eq!(md5_hex(&want), "330511d9a5adb60b4e5469bf7aa40652");
        assert_eq!(md5_hex(&[1, 2, 3, 4, 5, 6, 7, 8, 9]), "8596c1af55b14b7b320112944fcb8536");
        let mut off = want.clone();
        off[4] += 3;
        assert_eq!((max_diff(&want, &want, 8), max_diff(&want, &off, 8), max_diff(&want, &want[1..], 8)), (Some(0), Some(3), None));
        assert_eq!(yuv_layout(&Frame { width: 1, height: 1, pts_ns: 0, pixels: Pixels::Rgba(vec![0; 4]) }), None);
        let mut o = I420Oracle::new(String::new(), String::new());
        assert!(!o.ok());
        (o.compared, o.exact) = (3, 3);
        assert!(o.ok());
        assert_eq!(o.json(), ",\"i420_oracle\":{\"compared\":3,\"exact\":3,\"missing\":0,\"max_abs_diff\":0}");
    }

    /// AV1INBANDCOLOUR (SR366) PLAYCHECK-COLOUR: `frame_fnv`, the counter read and `--out` take BusSink's conversion
    /// (`Frame::to_rgba_colour` by `Player::colour` and `chroma_of`), no longer `to_rgba` (BT.601 limited nearest for every
    /// stream). An untagged SD frame is byte-identical to before; an untagged HD frame takes BT.709; an AV1 frame (bilinear)
    /// moves: its hash is pinned here.
    #[test]
    fn playcheck_colour_converts_as_the_bus_sink() {
        use gneiss_pal::dsp::demux::colour::Colour;
        let frame = |w: u32, h: u32| {
            let (cw, ch) = (w.div_ceil(2) as usize, h.div_ceil(2) as usize);
            let y: Vec<u8> = (0..w as usize * h as usize).map(|i| (16 + (i * 7) % 220) as u8).collect();
            let u: Vec<u8> = (0..cw * ch).map(|i| (16 + (i * 13) % 224) as u8).collect();
            let v: Vec<u8> = (0..cw * ch).map(|i| (240 - (i * 5) % 224) as u8).collect();
            Frame { width: w, height: h, pts_ns: 0, pixels: Pixels::I420 { y, u, v, y_stride: w as usize, uv_stride: cw } }
        };
        let sd = frame(320, 240);
        assert_eq!(sd.to_rgba_colour(None, Chroma::Nearest), sd.to_rgba(), "untagged SD: the hash is unchanged");
        let hd = frame(1280, 720);
        assert_ne!(hd.to_rgba_colour(None, Chroma::Nearest), hd.to_rgba(), "untagged HD converts BT.709 (the size default)");
        let bt709 = Colour { primaries: 1, transfer: 1, matrix: 1, full_range: Some(false), mastering: None, light: None };
        let av1 = hd.to_rgba_colour(Some(&bt709), Chroma::Bilinear);
        assert_eq!(fnv1a(&hd.to_rgba()), 0x0754_30e0, "the retired to_rgba hash of the HD frame");
        assert_eq!(fnv1a(&av1), 0x72e1_61fd, "an AV1 BT.709 limited bilinear frame_fnv");
    }
}

/// HEVCLEVELLINE (LEDGER SR368): the Player's profile tokens (`MediaInfo::video_profile`, `profile=.. tier=.. level=..`)
/// as JSON fields, one per `key=value` token; "" (no tokens) adds none.
fn profile_json(tokens: &str) -> String {
    tokens.split_whitespace().filter_map(|t| t.split_once('=')).map(|(k, v)| format!(",\"{k}\":\"{v}\"")).collect()
}

#[cfg(test)]
mod profile_tests {
    #[test]
    fn profile_json_kat() {
        assert_eq!(super::profile_json("profile=Main tier=main level=2"), ",\"profile\":\"Main\",\"tier\":\"main\",\"level\":\"2\"");
        assert_eq!(super::profile_json("profile=High level=4.1"), ",\"profile\":\"High\",\"level\":\"4.1\"");
        assert_eq!(super::profile_json(""), "");
    }
}
