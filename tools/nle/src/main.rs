// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `nle` — the non-linear editor's headless face (NLE1, LEDGER SR194); `play-check` is to the Player what this is to
//! `stria::nle`: argument plumbing and files, no editing or rendering logic.
//!
//! ```text
//! nle schedule <edl|media> [--fps N] --src REEL=<media> ...
//! nle render   <edl|media> [--fps N] --src REEL=<media> ... --out <stem>
//! nle export   <edl|media> [--fps N] --src REEL=<media> ... --out <dir> [--video png|raw|h264] [--qp N] [--gop N] [--audio wav|mp4|none]
//! nle --make-src <out.mp4> <base> [frames] [fps]
//! ```
//!
//! `schedule` prints one line per reel (`reel=<r> open=<full|indexed> dts=<stored|derived:n|presentation> edits=<n>`,
//! `stria::nle::Sources::open_report`; DEMUXWIRE SR213; ` applied=<n|first-only>` EDITMAP SR223), then one line per step
//! (`timeline_core::schedule`). EDITMAP (SR223): a MEDIA file in the `<edl>` place is its own timeline — its edit list
//! over the one reel `AX` (`stria::nle::Sources::from_media`; `--src` is not read).
//! `render` writes `<stem>.rgba` (every output frame, `w*h*4` bytes each, in order) and `<stem>.f32`
//! (interleaved little-endian f32 PCM), then one JSON line: frames, size,
//! PCM frames, rate, channels, seeks, run-up frames and the FNV-1a 64 of the RGBA stream. `--fps` is the frame rate when
//! the EDL carries no `* UNA FPS` line (default 25). `--make-src` writes `stria::nle::synth_source` (utp1 + PCM16).
//! `export` (NLE5, SR204) drives `stria::nle_export::export` into `<dir>` (created): `%06d.png` per frame or
//! `video.rgba` + `video.json`, and `audio.wav` or `audio.mp4`; one JSON line: codec, frames, files, bytes, the RGBA
//! fnv (`render`'s) and PCM frames. `--video h264` (NLE5B, SR206) writes `movie.mp4` (H.264 + PCM16) under
//! `--audio mp4`, else `video.mp4`; `--qp` (16..=40, default 26) and `--gop` (default the timeline's fps).

use std::io::Write;

use stria::nle::{RenderSink, Sources, render, synth_source};
use stria::nle_export::{AudioOut, H264Encoder, PngSequence, RawSequence, VideoEncoder, export};
use timeline_core::schedule::schedule;
use timeline_core::{Kind, Timeline, edl};

fn usage() -> ! {
    eprintln!("usage: nle schedule <edl|media> [--fps N] --src REEL=<media> ...");
    eprintln!("       nle render <edl|media> [--fps N] --src REEL=<media> ... --out <stem>");
    eprintln!("       nle export <edl|media> [--fps N] --src REEL=<media> ... --out <dir> [--video png|raw|h264] [--qp N] [--gop N] [--audio wav|mp4|none]");
    eprintln!("       nle --make-src <out.mp4> <base> [frames] [fps]");
    std::process::exit(2)
}

fn die(msg: String) -> ! {
    eprintln!("nle: {msg}");
    std::process::exit(1)
}

struct Files {
    rgba: std::io::BufWriter<std::fs::File>,
    pcm: std::io::BufWriter<std::fs::File>,
    fnv: u64,
    err: Option<std::io::Error>,
}

impl RenderSink for Files {
    fn frame(&mut self, _out_t: i64, _w: u32, _h: u32, rgba: &[u8]) {
        for &b in rgba {
            self.fnv = (self.fnv ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        if let Err(e) = self.rgba.write_all(rgba) {
            self.err.get_or_insert(e);
        }
    }
    fn pcm(&mut self, _out_t: i64, _rate: u32, _ch: u16, s: &[f32]) {
        let bytes: Vec<u8> = s.iter().flat_map(|x| x.to_le_bytes()).collect();
        if let Err(e) = self.pcm.write_all(&bytes) {
            self.err.get_or_insert(e);
        }
    }
}

fn load(edl_path: &str, fps: u32, srcs: &[(String, String)]) -> (Timeline, Sources) {
    let bytes = std::fs::read(edl_path).unwrap_or_else(|e| die(format!("{edl_path}: {e}")));
    // EDITMAP (SR223): a media file is its own timeline (its edit list over one reel); anything else is an EDL.
    if let Some(opened) = Sources::from_media("AX", &bytes, fps).unwrap_or_else(|e| die(format!("{edl_path}: {e}"))) {
        return opened;
    }
    let text = String::from_utf8(bytes).unwrap_or_else(|e| die(format!("{edl_path}: {e}")));
    let tl = edl::parse(&text, fps).unwrap_or_else(|e| die(format!("{edl_path}: {e}")));
    let mut sources = Sources::new(&tl);
    for s in &tl.sources {
        let Some((_, path)) = srcs.iter().find(|(r, _)| *r == s.reel) else { die(format!("no --src for reel {}", s.reel)) };
        let bytes = std::fs::read(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
        sources.add(s.id, bytes).unwrap_or_else(|e| die(format!("{path}: {e}")));
    }
    (tl, sources)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--make-src") {
        let out = args.get(1).unwrap_or_else(|| usage());
        let num = |i: usize, d: u32| args.get(i).map(|s| s.parse().unwrap_or_else(|_| usage())).unwrap_or(d);
        let (base, frames, fps) = (num(2, 0), num(3, 50), num(4, 25));
        std::fs::write(out, synth_source(base, frames, fps)).unwrap_or_else(|e| die(format!("{out}: {e}")));
        return;
    }
    let (Some(verb), Some(edl_path)) = (args.first(), args.get(1)) else { usage() };
    let (mut fps, mut out, mut srcs) = (25u32, None::<String>, Vec::new());
    let (mut video, mut audio) = ("png".to_string(), "wav".to_string());
    let (mut qp, mut gop) = (26u8, None::<u32>);
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--fps" => fps = it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()),
            "--out" => out = Some(it.next().unwrap_or_else(|| usage()).clone()),
            "--video" => video = it.next().unwrap_or_else(|| usage()).clone(),
            "--audio" => audio = it.next().unwrap_or_else(|| usage()).clone(),
            "--qp" => qp = it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()),
            "--gop" => gop = Some(it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())),
            "--src" => {
                let v = it.next().unwrap_or_else(|| usage());
                let Some((r, p)) = v.split_once('=') else { usage() };
                srcs.push((r.to_string(), p.to_string()));
            }
            _ => usage(),
        }
    }
    let (tl, sources) = load(edl_path, fps, &srcs);
    let range = 0..tl.end();
    match verb.as_str() {
        "schedule" => {
            let steps = schedule(&tl, range, &sources).unwrap_or_else(|e| die(e.to_string()));
            for s in &tl.sources {
                if let Some(r) = sources.open_report(s.id) {
                    println!("reel={} {r}", s.reel);
                }
            }
            for s in steps {
                let kind = if s.kind == Kind::Video { "V" } else { "A" };
                let track = s.track.and_then(|t| tl.track_number(t)).map(|n| format!("{kind}{n}")).unwrap_or_else(|| "-".into());
                let src = s.src.and_then(|id| tl.source(id)).map(|x| x.reel.as_str()).unwrap_or("-");
                let seek = s.seek.map(|k| k.to_string()).unwrap_or_else(|| "-".into());
                println!("out={} kind={kind} track={track} src={src} src_t={} len={} seek_ns={seek}", s.out_t, s.src_t, s.len);
            }
        }
        "render" => {
            let stem = out.unwrap_or_else(|| usage());
            let open = |ext: &str| {
                let p = format!("{stem}.{ext}");
                std::io::BufWriter::new(std::fs::File::create(&p).unwrap_or_else(|e| die(format!("{p}: {e}"))))
            };
            let mut files = Files { rgba: open("rgba"), pcm: open("f32"), fnv: 0xcbf2_9ce4_8422_2325, err: None };
            let r = render(&tl, range, &sources, &mut files).unwrap_or_else(|e| die(e.to_string()));
            if let Some(e) = files.err.take() {
                die(format!("{stem}: {e}"));
            }
            files.rgba.flush().and_then(|_| files.pcm.flush()).unwrap_or_else(|e| die(format!("{stem}: {e}")));
            println!(
                "{{\"frames\":{},\"w\":{},\"h\":{},\"pcm_frames\":{},\"rate\":{},\"channels\":{},\"seeks\":{},\"runup\":{},\"fnv\":\"{:016x}\"}}",
                r.frames, r.width, r.height, r.pcm_frames, r.rate, r.channels, r.seeks, r.runup_frames, files.fnv
            );
        }
        "export" => {
            let gop = gop.unwrap_or((tl.tb.den / tl.tb.num.max(1)).max(1) as u32);
            export_verb(&tl, range, &sources, &out.unwrap_or_else(|| usage()), &video, &audio, qp, gop)
        }
        _ => usage(),
    }
}

/// NLE5 (SR204): the `export` verb — plumbing only; the export is `stria::nle_export`'s.
#[allow(clippy::too_many_arguments)]
fn export_verb(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, dir: &str, video: &str, audio: &str, qp: u8, gop: u32) {
    let enc: Box<dyn VideoEncoder> = match video {
        "png" => Box::new(PngSequence),
        "raw" => Box::new(RawSequence),
        "h264" => Box::new(H264Encoder::new(qp, gop)),
        _ => usage(),
    };
    let aud = match audio {
        "wav" => AudioOut::Wav,
        "mp4" => AudioOut::Mp4,
        "none" => AudioOut::None,
        _ => usage(),
    };
    std::fs::create_dir_all(dir).unwrap_or_else(|e| die(format!("{dir}: {e}")));
    let r = export(tl, range, sources, std::path::Path::new(dir), enc, aud).unwrap_or_else(|e| die(e.to_string()));
    println!(
        "{{\"codec\":\"{video}\",\"frames\":{},\"files\":{},\"bytes\":{},\"fnv\":\"{:016x}\",\"pcm_frames\":{}}}",
        r.frames, r.files, r.bytes, r.fnv, r.pcm_frames
    );
}
