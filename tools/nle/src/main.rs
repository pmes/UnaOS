// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `nle` — the non-linear editor's headless face (NLE1, LEDGER SR194); `play-check` is to the Player what this is to
//! `stria::nle`: argument plumbing and files, no editing or rendering logic.
//!
//! ```text
//! nle schedule <edl|media> [--fps N] --src REEL=<media> ...
//! nle render   <edl|media> [--fps N] --src REEL=<media> ... --out <stem>
//! nle export   <edl|media> [--fps N] --src REEL=<media> ... --out <dir> [--video png|raw|h264] [--qp N] [--gop N] [--bitrate KBPS | --crf N] [--audio wav|mp4|aac|none] [--aac] [--mkv] [--png]
//! nle --make-src <out.mp4> <base> [frames] [fps]
//! nle size <edl> <reel> <letterbox|fill|stretch|adopt> [--fps N]
//! ```
//!
//! `size` (NLESIZEPOLICYUI, LEDGER SR402) sets a reel's size policy through the editor's ONE verb dispatch
//! (`timeline_core::editor::EditorState::verb`, `size <reel> <policy>`) and writes the EDL back through the ONE writer
//! (`edl::write`, titled by the file's stem): `nle: size <reel> <policy> -> <bytes> bytes`; a refusal is printed, exit 1.
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
//! `--audio mp4`, else `video.mp4`; `--qp` (16..=40, default 26) and `--gop` (default the timeline's fps); `--bitrate`
//! a CBR target in kbit/s at the timeline's fps (H264ENCRC SR218: `--qp` becomes the starting QP); `--crf` (0..=51,
//! H264ENCVBRCRF SR383) a constant quality instead (h264enc_core's `RcMode::Crf`), exclusive with `--bitrate`. `--audio aac` (or
//! `--aac`; NLE6EXPORTAAC rmbp-ledger B960) writes the MP4's sound as AAC-LC at 128 kb/s instead of PCM16.
//! `--mkv` (NLE6EXPORTMKV, rmbp-ledger B966) names the muxed file `.mkv` (`movie.mkv` / `video.mkv` / `audio.mkv`): the
//! extension is the container (an AAC track's priming edit list is refused under `.mkv`: CodecDelay owed).
//! `--png` (ADOPTPNG, LEDGER SR405) is `--video png`: the PNG sequence (`stria::nle_export::export_png`) read back per
//! frame, `nle: frame <n> <w>x<h> <path>` on stderr per frame and `nle: export png frames=<n> sizes=<k>` last (a reel under
//! Adopt keeps each frame's own size); the stdout JSON line is unchanged.

use std::io::Write;

use stria::nle::{RenderSink, Sources, render, synth_source};
use stria::nle_export::{AudioOut, Container, ExportReport, H264Encoder, PngSequence, RawSequence, VideoEncoder, export_as, export_png};
use timeline_core::schedule::schedule;
use timeline_core::{Kind, Timeline, edl};

fn usage() -> ! {
    eprintln!("usage: nle schedule <edl|media> [--fps N] --src REEL=<media> ...");
    eprintln!("       nle render <edl|media> [--fps N] --src REEL=<media> ... --out <stem>");
    eprintln!("       nle export <edl|media> [--fps N] --src REEL=<media> ... --out <dir> [--video png|raw|h264] [--qp N] [--gop N] [--bitrate KBPS | --crf N] [--audio wav|mp4|aac|none] [--aac] [--mkv] [--png]");
    eprintln!("       nle --make-src <out.mp4> <base> [frames] [fps]");
    eprintln!("       nle size <edl> <reel> <letterbox|fill|stretch|adopt> [--fps N]");
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
    if args.first().map(String::as_str) == Some("size") { size_verb(&args[1..]); return; } // NLESIZEPOLICYUI (SR402): the reel's size policy into the EDL (file tail)
    let (Some(verb), Some(edl_path)) = (args.first(), args.get(1)) else { usage() };
    let (mut fps, mut out, mut srcs) = (25u32, None::<String>, Vec::new());
    let (mut video, mut audio) = ("png".to_string(), "wav".to_string());
    let (mut qp, mut gop, mut bitrate, mut crf) = (26u8, None::<u32>, None::<u32>, None::<u8>);
    let mut container = Container::Mp4;
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--fps" => fps = it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()),
            "--out" => out = Some(it.next().unwrap_or_else(|| usage()).clone()),
            "--video" => video = it.next().unwrap_or_else(|| usage()).clone(),
            "--audio" => audio = it.next().unwrap_or_else(|| usage()).clone(),
            "--aac" => audio = "aac".to_string(),
            "--png" => video = "png".to_string(), // ADOPTPNG (SR405): the PNG sequence with its per-frame manifest (png_export, file tail)
            "--qp" => qp = it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()),
            "--gop" => gop = Some(it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())),
            "--bitrate" => bitrate = Some(it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())),
            "--mkv" => container = Container::Mkv,
            "--crf" => crf = Some(it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())),
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
            if bitrate.is_some() && crf.is_some() {
                die("--bitrate and --crf are exclusive".into());
            }
            let gop = gop.unwrap_or((tl.tb.den / tl.tb.num.max(1)).max(1) as u32);
            export_verb(&tl, range, &sources, &out.unwrap_or_else(|| usage()), &video, &audio, qp, gop, bitrate, crf, container)
        }
        _ => usage(),
    }
}

/// NLE5 (SR204): the `export` verb — plumbing only; the export is `stria::nle_export`'s.
#[allow(clippy::too_many_arguments)]
fn export_verb(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, dir: &str, video: &str, audio: &str, qp: u8, gop: u32, bitrate: Option<u32>, crf: Option<u8>, container: Container) {
    let enc: Box<dyn VideoEncoder> = match video {
        "png" => Box::new(PngSequence),
        "raw" => Box::new(RawSequence),
        "h264" => match (bitrate, crf) {
            (Some(k), _) => Box::new(H264Encoder::new(qp, gop).with_bitrate(k, tl.tb.den as u32, tl.tb.num as u32)),
            (None, Some(c)) => Box::new(H264Encoder::new(qp, gop).with_quality(c, tl.tb.den as u32, tl.tb.num as u32)),
            (None, None) => Box::new(H264Encoder::new(qp, gop)),
        },
        _ => usage(),
    };
    let aud = match audio {
        "wav" => AudioOut::Wav,
        "mp4" => AudioOut::Mp4,
        "aac" => AudioOut::Aac(128),
        "none" => AudioOut::None,
        _ => usage(),
    };
    std::fs::create_dir_all(dir).unwrap_or_else(|e| die(format!("{dir}: {e}")));
    let r = if video == "png" { png_export(tl, range, sources, dir, aud, container) } else { export_as(tl, range, sources, std::path::Path::new(dir), enc, aud, container).unwrap_or_else(|e| die(e.to_string())) }; // ADOPTPNG (SR405): the PNG arm's manifest (file tail)
    println!(
        "{{\"codec\":\"{video}\",\"frames\":{},\"files\":{},\"bytes\":{},\"fnv\":\"{:016x}\",\"pcm_frames\":{}}}",
        r.frames, r.files, r.bytes, r.fnv, r.pcm_frames
    );
}

/// NLESIZEPOLICYUI (SR402): `nle size <edl> <reel> <policy> [--fps N]` — plumbing only: the EDL opened as the editor's
/// state, the `size` verb run through timeline_core's ONE dispatch, the timeline written back by the ONE writer.
fn size_verb(args: &[String]) {
    let (Some(path), Some(reel), Some(word)) = (args.first(), args.get(1), args.get(2)) else { usage() };
    let fps = match &args[3..] {
        [] => 25,
        [f, n] if f == "--fps" => n.parse().unwrap_or_else(|_| usage()),
        _ => usage(),
    };
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let mut st = timeline_core::editor::EditorState::open(&text, fps).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let v = format!("size {reel} {word}");
    match st.verb(&v) {
        Some(Ok(())) => {}
        Some(Err(e)) => die(format!("{v}: {e}")),
        None => usage(),
    }
    let stem = std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("NLE");
    let out = edl::write(&st.timeline, stem).unwrap_or_else(|e| die(format!("{path}: {e}")));
    std::fs::write(path, &out).unwrap_or_else(|e| die(format!("{path}: {e}")));
    println!("nle: {v} -> {} bytes", out.len());
}

/// ADOPTPNG (SR405): the `png` arm of `export` — plumbing only: `stria::nle_export::export_png` writes and reads back the
/// sequence; one stderr line per frame (`nle: frame <n> <w>x<h> <path>`), then `nle: export png frames=<n> sizes=<k>` (k the
/// distinct sizes); the report goes back to `export_verb`, which prints the one JSON line. A refusal is printed, exit 1.
fn png_export(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, dir: &str, aud: AudioOut, container: Container) -> ExportReport {
    let (r, frames) = export_png(tl, range, sources, std::path::Path::new(dir), aud, container).unwrap_or_else(|e| die(e.to_string()));
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    for (n, (path, w, h)) in frames.iter().enumerate() {
        eprintln!("nle: frame {n} {w}x{h} {}", path.display());
        if !sizes.contains(&(*w, *h)) {
            sizes.push((*w, *h));
        }
    }
    eprintln!("nle: export png frames={} sizes={}", frames.len(), sizes.len());
    r
}
