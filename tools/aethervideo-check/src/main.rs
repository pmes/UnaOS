// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AETHERVIDEO (LEDGER SR39) oracle face.
//!
//! ```text
//! aethervideo-check make <dir> [frames] [fps] [w] [h]   test-pattern streams + raw frames
//! aethervideo-check mux <chunks.bin> <out.webm> <w> <h> <fps>   VP9 chunks → WebM (demux_core)
//! aethervideo-check render <page.html> <out.png> --frame N --fps F [--width W --height H]
//! aethervideo-check compare <aether.png> <chromium.png> <x,y,w,h> [--counter N] [--min-psnr dB]
//! ```
//!
//! `make` writes `pattern-utp.webm` (Stria's real `utp1` test-pattern codec, every frame a
//! keyframe) and `frames.rgba` (the same frames, raw RGBA) for `oracle/encode.cjs`, which has
//! Chromium's WebCodecs VP9 encoder turn them into `chunks.bin` at quantizer 0 (VP9 lossless);
//! `mux` writes those chunks into `pattern-vp9.webm` with demux_core's own Matroska writer.
//!
//! `render` is Aether headless with Stria's media service on a real bandy bus: load the page,
//! fire the engine's media requests, feed Stria's replies back, wait for every `<video>`'s
//! poster, seek each to the middle of frame N, wait for that frame, render, write the PNG, and
//! print one JSON line per element (box, url, pts, the counter read back from the box).
//!
//! `compare` scores a box of two PNGs: PSNR (RGB), max channel difference, % of pixels within 8,
//! and, with `--counter N`, reads the test-pattern counter out of both boxes (the box must show
//! the frame at 1:1).

use std::path::Path;
use std::time::{Duration, Instant};

use aether::AetherEngine;
use bandy::{SMessage, Synapse};
use gneiss_pal::dsp::demux::build::{self, MediaTrack, MkvOptions, SampleSpec, TrackSpec};
use gneiss_pal::dsp::demux::TrackKind;
use gneiss_pal::dsp::video::TestPattern;

fn arg<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn die(msg: String) -> ! {
    eprintln!("aethervideo-check: {msg}");
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("make") => make(&args),
        Some("mux") => mux(&args),
        Some("render") => render(&args),
        Some("compare") => {
            let ok = compare(&args);
            std::process::exit(if ok { 0 } else { 1 });
        }
        _ => die("usage: make | mux | render | compare (see the module docs)".into()),
    }
}

fn num(args: &[String], i: usize, def: u32) -> u32 {
    args.get(i).and_then(|s| s.parse().ok()).unwrap_or(def)
}

fn make(args: &[String]) {
    let dir = Path::new(args.get(2).unwrap_or_else(|| die("make <dir>".into())));
    let (n, fps, w, h) = (num(args, 3, 10), num(args, 4, 10), num(args, 5, 320), num(args, 6, 240));
    std::fs::create_dir_all(dir).unwrap_or_else(|e| die(format!("{}: {e}", dir.display())));
    let t = build::test_pattern_track(1, w as u16, h as u16, fps, n, 1);
    std::fs::write(dir.join("pattern-utp.webm"), build::mkv(&[t], &MkvOptions::default())).unwrap();
    let mut raw = Vec::with_capacity((w * h * 4 * n) as usize);
    for i in 0..n {
        raw.extend_from_slice(&TestPattern::render(w, h, i));
    }
    std::fs::write(dir.join("frames.rgba"), raw).unwrap();
    std::fs::write(dir.join("meta.json"), format!("{{\"frames\":{n},\"fps\":{fps},\"width\":{w},\"height\":{h}}}\n")).unwrap();
}

/// `chunks.bin`: repeated `[u32 LE length][u8 keyframe][payload]` in presentation order.
fn mux(args: &[String]) {
    let input = args.get(2).unwrap_or_else(|| die("mux <chunks.bin> <out.webm> <w> <h> <fps>".into()));
    let out = args.get(3).unwrap_or_else(|| die("mux needs <out.webm>".into()));
    let (w, h, fps) = (num(args, 4, 320), num(args, 5, 240), num(args, 6, 10));
    let bytes = std::fs::read(input).unwrap_or_else(|e| die(format!("{input}: {e}")));
    let mut samples = Vec::new();
    let mut o = 0usize;
    while o + 5 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
        let key = bytes[o + 4] != 0;
        let data = bytes.get(o + 5..o + 5 + len).unwrap_or_else(|| die("truncated chunks.bin".into())).to_vec();
        o += 5 + len;
        let i = samples.len() as i64;
        samples.push(SampleSpec { data, dts: i * 1000, pts: i * 1000, duration: 1000, keyframe: key });
    }
    let spec = TrackSpec {
        id: 1,
        kind: TrackKind::Video,
        fourcc: *b"vp09",
        config_box: None,
        codec_id: "V_VP9",
        config: Vec::new(),
        timescale: fps * 1000,
        width: w as u16,
        height: h as u16,
        sample_rate: 0,
        channels: 0,
        bit_depth: 0,
        default_duration_ns: 1_000_000_000 / fps as u64,
    };
    std::fs::write(out, build::mkv(&[MediaTrack { spec, samples }], &MkvOptions::default())).unwrap();
}

/// The shell's half of the bus, synchronously: fire the engine's queued requests, then feed
/// Stria's replies to the engine until `done()` or the deadline.
fn pump(engine: &mut AetherEngine, syn: &Synapse, rx: &mut tokio::sync::broadcast::Receiver<SMessage>, deadline: Duration, done: impl Fn(&AetherEngine) -> bool) -> bool {
    let end = Instant::now() + deadline;
    loop {
        for req in engine.take_media_requests() {
            syn.fire(req);
        }
        loop {
            match rx.try_recv() {
                Ok(m) => {
                    engine.on_media_message(&m);
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        if done(engine) {
            return true;
        }
        if Instant::now() > end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn render(args: &[String]) {
    let page = args.get(2).unwrap_or_else(|| die("render <page.html> <out.png> --frame N --fps F".into()));
    let out = args.get(3).unwrap_or_else(|| die("render needs <out.png>".into()));
    let frame: u64 = arg(args, "--frame").and_then(|s| s.parse().ok()).unwrap_or(0);
    let fps: u64 = arg(args, "--fps").and_then(|s| s.parse().ok()).unwrap_or(10);
    let width: u32 = arg(args, "--width").and_then(|s| s.parse().ok()).unwrap_or(640);
    let height: u32 = arg(args, "--height").and_then(|s| s.parse().ok()).unwrap_or(400);
    let path = std::fs::canonicalize(page).unwrap_or_else(|e| die(format!("{page}: {e}")));
    let html = std::fs::read_to_string(&path).unwrap();

    let syn = Synapse::new();
    let mut rx = syn.subscribe();
    // Stria's real media service: headless (no audio device), 60 Hz cadence.
    let _stria = stria::media_bus::MediaService::spawn(syn.clone(), 60, false);

    let mut engine = AetherEngine::new();
    engine.handle_event(aether::api::events::Event::Resize(width, height));
    engine.load_html(&format!("file://{}", path.display()), &html, true);
    let n = engine.media_elements().len();
    // Every video's poster (MediaOpened + first MediaFrame), or an error.
    let ready = |e: &AetherEngine| {
        e.media_elements().iter().all(|m| m.kind != aether::media::Kind::Video || m.frame.is_some() || matches!(m.state, aether::media::State::Error(_)))
    };
    if !pump(&mut engine, &syn, &mut rx, Duration::from_secs(10), ready) {
        die("timed out waiting for posters".into());
    }
    // Seek every video to the middle of frame N (the same target the Chromium script sets).
    let frame_ns = 1_000_000_000 / fps;
    let target = frame * frame_ns + frame_ns / 2;
    for i in 0..n {
        if engine.media_elements()[i].kind == aether::media::Kind::Video {
            engine.media_seek(i, target);
        }
    }
    let want_pts = (frame * frame_ns) as i64;
    let landed = |e: &AetherEngine| {
        e.media_elements().iter().all(|m| m.kind != aether::media::Kind::Video || matches!(m.state, aether::media::State::Error(_)) || (m.pts_ns - want_pts).abs() < 2_000_000)
    };
    if !pump(&mut engine, &syn, &mut rx, Duration::from_secs(10), landed) {
        die("timed out waiting for the seek".into());
    }
    engine.damage_rects.push((0, 0, width, height));
    engine.render_frame();
    let mut rgba = engine.surface().to_vec();
    for p in rgba.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let img = image::RgbaImage::from_raw(width, height, rgba).unwrap();
    img.save(out).unwrap_or_else(|e| die(format!("{out}: {e}")));
    for m in engine.media_elements() {
        let b = engine.box_of(&m.node).unwrap_or_default();
        let counter = match (m.natural, b) {
            (Some((nw, nh)), (x, y, w, h)) if w.round() as u32 == nw && h.round() as u32 == nh => {
                let crop = image::imageops::crop_imm(&img, x.round() as u32, y.round() as u32, nw, nh).to_image();
                TestPattern::read_counter(crop.as_raw(), nw, nh).map(|c| c.to_string()).unwrap_or_else(|| "null".into())
            }
            _ => "null".into(),
        };
        println!(
            "{{\"url\":\"{}\",\"box\":[{},{},{},{}],\"pts_ns\":{},\"real_video\":{},\"state\":\"{:?}\",\"counter\":{}}}",
            m.url, b.0, b.1, b.2, b.3, m.pts_ns, m.real_video, m.state, counter
        );
    }
}

fn load(p: &str) -> image::RgbaImage {
    image::open(p).unwrap_or_else(|e| die(format!("{p}: {e}"))).to_rgba8()
}

fn compare(args: &[String]) -> bool {
    let a = load(args.get(2).unwrap_or_else(|| die("compare <a.png> <b.png> <x,y,w,h>".into())));
    let b = load(args.get(3).unwrap_or_else(|| die("compare needs <b.png>".into())));
    let r: Vec<u32> = args.get(4).unwrap_or_else(|| die("compare needs <x,y,w,h>".into())).split(',').map(|v| v.trim().parse().unwrap_or_else(|_| die(format!("bad box {v}")))).collect();
    let (x, y, w, h) = (r[0], r[1], r[2], r[3]);
    let s = score(&a, &b, x, y, w, h);
    if let Some(out) = arg(args, "--diff") {
        // Per-pixel max channel difference ×4, grey; the box only.
        let mut d = image::RgbaImage::new(w, h);
        for yy in 0..h {
            for xx in 0..w {
                let (pa, pb) = (a.get_pixel(x + xx, y + yy).0, b.get_pixel(x + xx, y + yy).0);
                let m = (0..3).map(|k| pa[k].abs_diff(pb[k])).max().unwrap_or(0).saturating_mul(4);
                d.put_pixel(xx, yy, image::Rgba([m, m, m, 255]));
            }
        }
        d.save(out).unwrap_or_else(|e| die(format!("{out}: {e}")));
    }
    let mut ok = true;
    if let Some(min) = arg(args, "--min-psnr").and_then(|v| v.parse::<f64>().ok()) {
        ok &= s.psnr >= min;
    }
    let mut counters = String::new();
    if let Some(n) = arg(args, "--counter").and_then(|v| v.parse::<u32>().ok()) {
        let ca = TestPattern::read_counter(image::imageops::crop_imm(&a, x, y, w, h).to_image().as_raw(), w, h);
        let cb = TestPattern::read_counter(image::imageops::crop_imm(&b, x, y, w, h).to_image().as_raw(), w, h);
        ok &= ca == Some(n) && cb == Some(n);
        counters = format!(",\"counter_a\":{},\"counter_b\":{}", ca.map_or("null".into(), |c| c.to_string()), cb.map_or("null".into(), |c| c.to_string()));
    }
    println!(
        "{{\"box\":[{x},{y},{w},{h}],\"psnr_db\":{:.2},\"max_diff\":{},\"within8_pct\":{:.3},\"exact_pct\":{:.3}{counters},\"ok\":{ok}}}",
        s.psnr, s.max, s.within8, s.exact
    );
    ok
}

struct Score {
    psnr: f64,
    max: u8,
    within8: f64,
    exact: f64,
}

fn score(a: &image::RgbaImage, b: &image::RgbaImage, x: u32, y: u32, w: u32, h: u32) -> Score {
    let (mut se, mut max, mut within, mut exact, mut n) = (0f64, 0u8, 0u64, 0u64, 0u64);
    for yy in y..y + h {
        for xx in x..x + w {
            let (pa, pb) = (a.get_pixel(xx, yy).0, b.get_pixel(xx, yy).0);
            let mut m = 0u8;
            for k in 0..3 {
                let d = pa[k].abs_diff(pb[k]);
                se += (d as f64) * (d as f64);
                m = m.max(d);
            }
            max = max.max(m);
            within += (m <= 8) as u64;
            exact += (m == 0) as u64;
            n += 1;
        }
    }
    let mse = se / (n.max(1) * 3) as f64;
    let psnr = if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
    Score { psnr, max, within8: 100.0 * within as f64 / n.max(1) as f64, exact: 100.0 * exact as f64 / n.max(1) as f64 }
}
