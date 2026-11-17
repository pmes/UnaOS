// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `nle` — the non-linear editor's headless face (NLE1, LEDGER SR194); `play-check` is to the Player what this is to
//! `stria::nle`: argument plumbing and files, no editing or rendering logic.
//!
//! ```text
//! nle schedule <edl|media> [--fps N] --src REEL=<media> ...
//! nle render   <edl|media> [--fps N] --src REEL=<media> ... --out <stem>
//! nle export   <edl|media> [--fps N] --src REEL=<media> ... --out <dir> [--video png|raw|h264|i420|y4m] [--qp N] [--gop N] [--bitrate KBPS | --crf N] [--scenecut [K]] [--min-keyint N] [--aq [K]] [--two-pass | --pass 1|2 --stats <file>] [--qcomp F] [--cplxblur N] [--qblur F] [--qpstep N] [--pass2 k=v,...] [--hrd] [--audio wav|aac[:KBPS]|mp4|none] [--aac] [--mkv] [--png]
//! nle --make-src <out.mp4> <base> [frames] [fps]
//! nle size <edl> <reel> <letterbox|fill|stretch|adopt> [--fps N]
//! nle completions bash|zsh|fish [--install [<dir>] | --uninstall [<dir>]] [--force]
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
//! H264ENCVBRCRF SR383) a constant quality instead (h264enc_core's `RcMode::Crf`), exclusive with `--bitrate`. `--scenecut [K]`
//! (H264ENCSCENECUT SR680) codes an IDR at a detected scene cut, K the threshold in percent (default 40); `--aq [K]` (H264ENCAQ SR693) adaptive quantisation per macroblock, K the strength in tenths (default 10, x264's 1.0); `--two-pass` (H264ENCTWOPASS SR698, with `--bitrate`) exports twice: a CBR first pass into `<dir>/pass1/` (removed after; `pass1_stats`, file tail), then the H.264 export planned from its per-picture records. `--pass 1 --stats <f>` / `--pass 2 --stats <f>` (H264ENCTWOPASSFILE SR719, with `--bitrate` and `--video h264`, x264's two-process form) split the two: pass 1 is the CBR export, its records written to `<f>` in h264enc_core's ONE stats text (`rc::write_stats`; `nle: two-pass pass1 pictures=<n> bytes=<b> stats=<f>`), pass 2 reads `<f>` (`rc::read_stats`, refused by name, and on an fps other than the timeline's; another picture count is rescaled to the range's, `rc::rescale_stats`, H264ENCTWOPASSRESCALE SR727, `nle: two-pass pass2 stats rescaled <m> -> <n>`) and plans the export from it (`nle: two-pass pass2 stats=<f> pictures=<n>`); `two_pass_of` / `pass2_stats` / `write_pass1`, file tail; `--qcomp F` / `--cplxblur N` (H264ENCTWOPASSQCOMPCLI SR753) set x264's qcompress (0..=1, refused by name outside) and complexity blur of the planned pass (`--two-pass` / `--pass 2`, `stria::nle_export::pass2_of`), unread otherwise; `--qblur F` (H264ENCTWOPASSQBLURCLI SR766) sets x264's QP blur of the planned pass (default 0.5, 0 = none; 0..=16, refused by name outside, `stria::nle_export::qblur_of`), read as `--qcomp` is; `--qpstep N` (H264ENCTWOPASSQPSTEPCLI SR773) sets x264's qpstep of the planned pass (default 4, 0 = no clamp; 0..=51, refused by name outside, `stria::nle_export::qpstep_of`), read as `--qcomp` is; `--pass2 k=v,...` (H264ENCTWOPASSCLITABLE SR790) sets any of the four in one word (`qcomp=0.6,cplxblur=20,qblur=0.5,qpstep=4`; an unknown key or a bad value refused by name, `stria::nle_export::pass2_cli_of`), the last flag written winning in argv order, and a planned export prints `nle: two-pass plan qcomp=<F> cplxblur=<N> qblur=<F> qpstep=<N>` on stderr (`pass2_words`); `--hrd` (H264ENCHRD SR699, with `--bitrate`) signals the CBR bucket as the NAL HRD (VUI nal_hrd_parameters, buffering-period / pic-timing SEI, filler). `--audio aac` (or
//! `--aac`; NLE6EXPORTAAC rmbp-ledger B960) writes the MP4's sound as AAC-LC at 128 kb/s instead of PCM16 (`--audio aac:KBPS` another rate; the words are `nle_export::audio_named`'s, NLE5DEXPORTAUDIOOPTIONS SR722).
//! `--mkv` (NLE6EXPORTMKV, rmbp-ledger B966) names the muxed file `.mkv` (`movie.mkv` / `video.mkv` / `audio.mkv`): the
//! extension is the container (an AAC track's priming edit list is refused under `.mkv`: CodecDelay owed).
//! `--png` (ADOPTPNG, LEDGER SR405) is `--video png`: the PNG sequence (`stria::nle_export::export_png`) read back per
//! frame, `nle: frame <n> <w>x<h> <path>` on stderr per frame and `nle: export png frames=<n> sizes=<k>` last (a reel under
//! Adopt keeps each frame's own size); the stdout JSON line is unchanged.
//! `--video i420|y4m` (NLERAWEXPORT, LEDGER SR689) writes `video.i420` (raw planar 4:2:0) or `video.y4m` (YUV4MPEG2) + `video.json` over h264enc_core's ONE converter (`planar_export`, file tail): `nle: export <fmt> frames=<n> <w>x<h>` on stderr; a second size is refused (`frame does not match`).

use std::io::Write;

use stria::nle::{RenderSink, Sources, render, synth_source};
use stria::nle_export::{AudioOut, Container, ExportReport, H264Encoder, H264Opts, I420Raw, VIDEO_NAMES, VideoEncoder, Y4m, export_as, export_png, Pass2Cli, pass2_cli_of, pass2_words, qblur_of, qcomp_of, qpstep_of, video_named_with};
use timeline_core::schedule::schedule;
use timeline_core::{Kind, Timeline, edl};

fn usage() -> ! {
    usage_to(&mut std::io::stderr(), 2, None) // NLECLIREFUSALSBYNAME (SR801): the five lines in `usage_to` (file tail): stderr, exit 2
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
    let args: Vec<String> = std::env::args().skip(1).collect(); if help_asked(&args) { usage_to(&mut std::io::stdout(), 0, Some(args[0].as_str()).filter(|v| !matches!(*v, "help" | "--help"))) }
    if args.first().map(String::as_str) == Some("--make-src") {
        let out = args.get(1).unwrap_or_else(|| usage());
        let num = |i: usize, d: u32| args.get(i).map(|s| s.parse().unwrap_or_else(|_| bad("--make-src", s, &what("--make-src")))).unwrap_or(d); // NLECLIREFUSALSREST (SR814): a bad number by name
        let (base, frames, fps) = (num(2, 0), num(3, 50), num(4, 25));
        std::fs::write(out, synth_source(base, frames, fps)).unwrap_or_else(|e| die(format!("{out}: {e}")));
        return;
    }
    if args.first().map(String::as_str) == Some("size") { size_verb(&args[1..]); return; } // NLESIZEPOLICYUI (SR402): the reel's size policy into the EDL (file tail)
    if args.first().map(String::as_str) == Some("completions") { let sh = value(&mut args[1..].iter(), "completions"); completions_out(&sh, &completions(&sh).unwrap_or_else(|| bad("completions", &sh, &format!("one of {}", SHELLS.join("|")))), &args[2..]); return; } verb_hint(&args); let (Some(verb), Some(edl_path)) = (args.first(), args.get(1)) else { usage() }; // NLECLIREFUSALSBYNAME (SR801): `nle help` / `--help` / `<verb> --help` print the usage on stdout, exit 0 (file tail); NLECLIPASS2VALUE (SR821): `nle completions <shell>` prints the script on stdout, exit 0 (file tail); NLECLICOMPLETIONSINSTALL (SR1011): `--install [<dir>] [--force]` writes it into the shell's user completion dir (file tail)
    let (mut fps, mut out, mut srcs) = (25u32, None::<String>, Vec::new());
    let (mut video, mut audio) = ("png".to_string(), "wav".to_string());
    let (mut qp, mut gop, mut bitrate, mut crf) = (26u8, None::<u32>, None::<u32>, None::<u8>);
    let mut scenecut = None::<u8>; let mut aq = None::<u8>; let mut two_pass = TwoPass::Off; let (mut pass, mut stats) = (None::<u8>, None::<String>); let mut hrd = false; let mut min_keyint = None::<u32>; let mut p2 = Pass2Cli::default(); // H264ENCSCENECUT (SR680): `--scenecut [K]`, h264enc_core's `Config::scenecut`
    let mut container = Container::Mp4;
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--fps" => fps = num(&mut it, "--fps"),
            "--out" => out = Some(value(&mut it, "--out")),
            "--video" => video = value(&mut it, "--video"),
            "--audio" => audio = value(&mut it, "--audio"),
            "--aac" => audio = "aac".to_string(),
            "--png" => video = "png".to_string(), // ADOPTPNG (SR405): the PNG sequence with its per-frame manifest (png_export, file tail)
            "--qp" => qp = num(&mut it, "--qp"),
            "--gop" => gop = Some(num(&mut it, "--gop")),
            "--bitrate" => bitrate = Some(num(&mut it, "--bitrate")),
            "--mkv" => container = Container::Mkv,
            "--crf" => crf = Some(num(&mut it, "--crf")),
            "--scenecut" => scenecut = Some(match it.clone().next().and_then(|s| s.parse().ok()) { Some(k) => { it.next(); k } None => 40 }),
            "--min-keyint" => min_keyint = Some(num(&mut it, "--min-keyint")), // H264ENCSCENECUTKEYINTMIN (SR757): x264's --min-keyint, h264enc_core's `Config::min_keyint`
            "--two-pass" => two_pass = TwoPass::InProcess, // H264ENCTWOPASS (SR698): two exports, the second planned from the first
            "--pass" => pass = Some(num(&mut it, "--pass")), // H264ENCTWOPASSFILE (SR719): x264's --pass 1|2
            "--stats" => stats = Some(value(&mut it, "--stats")), // H264ENCTWOPASSFILE (SR719): the first pass's records on disk
            "--aq" => aq = Some(match it.clone().next().and_then(|s| s.parse().ok()) { Some(k) => { it.next(); k } None => 10 }), // H264ENCAQ (SR693): h264enc_core's `Config::aq`
            "--hrd" => hrd = true, // H264ENCHRD (SR699): h264enc_core's `Config::hrd`
            "--qcomp" => p2.qcomp = Some({ let s = value(&mut it, "--qcomp"); qcomp_of(&s).unwrap_or_else(|| bad("--qcomp", &s, "not a fraction within 0..=1")) }), // H264ENCTWOPASSQCOMPCLI (SR753): x264's qcompress of the planned pass
            "--cplxblur" => p2.cplxblur = Some(num(&mut it, "--cplxblur")), // H264ENCTWOPASSQCOMPCLI (SR753): x264's complexity blur of the planned pass
            "--qblur" => p2.qblur_q8 = Some({ let s = value(&mut it, "--qblur"); qblur_of(&s).unwrap_or_else(|| bad("--qblur", &s, "not a number within 0..=16")) }), // H264ENCTWOPASSQBLURCLI (SR766): x264's QP blur of the planned pass (Q8)
            "--qpstep" => p2.qpstep = Some({ let s = value(&mut it, "--qpstep"); qpstep_of(&s).unwrap_or_else(|| bad("--qpstep", &s, "not an integer within 0..=51")) }), // H264ENCTWOPASSQPSTEPCLI (SR773): x264's qpstep of the planned pass
            "--pass2" => p2 = pass2_cli_of(&value(&mut it, "--pass2")).unwrap_or_else(|e| die(format!("--pass2 {e}"))).over(p2), // H264ENCTWOPASSCLITABLE (SR790): any of the four plan knobs in one word, the last flag written winning; NLECLIPASS2VALUE (SR821): a missing value through `value`
            "--src" => {
                let v = value(&mut it, "--src");
                let Some((r, p)) = v.split_once('=') else { bad("--src", &v, "REEL=<media> expected") };
                srcs.push((r.to_string(), p.to_string()));
            }
            _ => die(unknown_flag(a)), // NLECLIDIDYOUMEAN (SR816): the nearest arm within 2 by name, else SR801's text (file tail)
        }
    }
    let (tl, sources) = load(edl_path, fps, &srcs);
    let range = 0..tl.end();
    match verb.as_str() {
        "schedule" => {
            let steps = schedule(&tl, range, &sources).unwrap_or_else(|e| die(e.to_string()));
            for s in &tl.sources {
                if let Some(r) = sources.open_report(s.id) {
                    println!("reel={} {r}{}", s.reel, sources.summary(s.id).map(|x| format!(" summary=\"{x}\"")).unwrap_or_default());
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
            let stem = out.unwrap_or_else(|| die(format!("render: {}", what("render --out")))); // NLECLIREFUSALSREST (SR814): the missing --out by name
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
            let two_pass = two_pass_of(two_pass, pass, stats, bitrate.is_some(), &video); // H264ENCTWOPASS (SR698) / H264ENCTWOPASSFILE (SR719): the mode and its refusals (file tail)
            let gop = gop.unwrap_or((tl.tb.den / tl.tb.num.max(1)).max(1) as u32);
            export_verb(&tl, range, &sources, &out.unwrap_or_else(|| die(format!("export: {}", what("export --out")))), &video, &audio, qp, gop, bitrate, crf, scenecut, min_keyint, aq, two_pass, hrd, p2, container)
        }
        _ => usage(),
    }
}

/// NLE5 (SR204): the `export` verb — plumbing only; the export is `stria::nle_export`'s.
#[allow(clippy::too_many_arguments)]
fn export_verb(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, dir: &str, video: &str, audio: &str, qp: u8, gop: u32, bitrate: Option<u32>, crf: Option<u8>, scenecut: Option<u8>, min_keyint: Option<u32>, aq: Option<u8>, two_pass: TwoPass, hrd: bool, p2: Pass2Cli, container: Container) {
    let two_pass_stats = match bitrate { Some(k) if two_pass.plans() && video == "h264" => Some(pass2_stats(&two_pass, tl, range.clone(), sources, dir, qp, gop, k, scenecut, aq, container)), _ => None }; if two_pass_stats.is_some() { eprintln!("nle: two-pass plan {}", pass2_words(&p2)); } // NLE5DCLITABLE (SR718): the planned pass of --two-pass / --pass 2 (H264ENCTWOPASSFILE SR719: `pass2_stats`, file tail), then the ONE table
    let mut kept = None; let enc: Box<dyn VideoEncoder> = match (video, bitrate, &two_pass) { ("h264", Some(k), TwoPass::Pass1(_)) => { let e = H264Encoder::new(qp, gop).with_scenecut(scenecut).with_min_keyint(min_keyint).with_aq(aq).with_hrd(hrd).with_bitrate(k, tl.tb.den as u32, tl.tb.num as u32); kept = Some(e.stats()); Box::new(e) } _ => video_named_with(video, tl.tb, p2.into_opts(H264Opts { qp, gop: Some(gop), bitrate, crf, scenecut, aq, min_keyint, hrd, two_pass_stats, ..H264Opts::default() })).unwrap_or_else(|| bad("--video", video, &format!("one of {}", VIDEO_NAMES.join("|")))) }; // H264ENCTWOPASSFILE (SR719): `--pass 1` keeps the records' handle, which the table cannot hand out (owed H264ENC-TWOPASS-FILE-TABLE)
    let aud = stria::nle_export::audio_named(audio).unwrap_or_else(|| bad("--audio", audio, &format!("one of {}|aac:KBPS", stria::nle_export::AUDIO_NAMES.join("|")))); // NLE5DEXPORTAUDIOOPTIONS (SR722): THE audio table
    std::fs::create_dir_all(dir).unwrap_or_else(|e| die(format!("{dir}: {e}")));
    let r = if video == "png" { png_export(tl, range, sources, dir, aud, container) } else if matches!(video, "i420" | "y4m") { planar_export(tl, range, sources, dir, video, aud, container) } else { export_as(tl, range, sources, std::path::Path::new(dir), enc, aud, container).unwrap_or_else(|e| die(e.to_string())) }; // ADOPTPNG (SR405): the PNG arm's manifest (file tail)
    if r.stand_in_audio > 0 { eprintln!("nle: audio stand-in reels={}", r.stand_in_audio); } // NLESTANDINAUDIOEDITOR (SR713): reels whose audio rendered as the silent stand-in
    if let (TwoPass::Pass1(f), Some(s)) = (&two_pass, &kept) { write_pass1(f, &s.borrow(), tl); } // H264ENCTWOPASSFILE (SR719): pass 1's records to `--stats` (file tail)
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
        [f, n] if f == "--fps" => n.parse().unwrap_or_else(|_| bad("--fps", n, &what("--fps"))), // NLECLIREFUSALSREST (SR814): a bad number by name
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

// ---- NLERAWEXPORT (LEDGER SR689): the raw planar arms of `export` (named tail block) ----
/// NLERAWEXPORT (SR689): the `i420` / `y4m` arms of `export` — plumbing only: `stria::nle_export::I420Raw` / `Y4m` through
/// `export_as`, then `nle: export <fmt> frames=<n> <w>x<h>` on stderr (the size the encoder latched); the report goes back to
/// `export_verb`, which prints the one JSON line. A refusal (a second size: `frame does not match`) is printed, exit 1.
fn planar_export(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, dir: &str, video: &str, aud: AudioOut, container: Container) -> ExportReport {
    let (enc, size): (Box<dyn VideoEncoder>, _) = if video == "y4m" {
        let e = Y4m::new(tl.tb.den as u32, tl.tb.num as u32);
        let size = e.size();
        (Box::new(e), size)
    } else {
        let e = I420Raw::default();
        let size = e.size();
        (Box::new(e), size)
    };
    let r = export_as(tl, range, sources, std::path::Path::new(dir), enc, aud, container).unwrap_or_else(|e| die(e.to_string()));
    let (w, h) = size.get().unwrap_or((0, 0));
    eprintln!("nle: export {video} frames={} {w}x{h}", r.frames);
    r
}

// ---- H264ENCTWOPASS (LEDGER SR698): the first pass of `export --two-pass` (named tail block) ----
/// H264ENCTWOPASS (SR698): the first pass — plumbing only: the same timeline exported once more through `stria::nle_export::export_as`
/// as a CBR H.264 stream at `kbps` into `<dir>/pass1/` (no audio), its per-picture records read through `H264Encoder::stats`, the
/// directory removed; `nle: two-pass pass1 pictures=<n> bytes=<b>` on stderr. A refusal is printed, exit 1.
#[allow(clippy::too_many_arguments)]
fn pass1_stats(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, dir: &str, qp: u8, gop: u32, kbps: u32, scenecut: Option<u8>, aq: Option<u8>, container: Container) -> Vec<stria::nle_export::PicStat> {
    let v = stria::nle_export::pass1_stats(tl, range, sources, std::path::Path::new(dir), qp, gop, kbps, scenecut, aq, container).unwrap_or_else(|e| die(e)); // H264ENCTWOPASSQCOMPFACE (SR1076): the one first pass, moved to the lib (the editor face runs it too)
    eprintln!("nle: two-pass pass1 pictures={} bytes={}", v.len(), v.iter().map(|p| p.bits).sum::<u64>() / 8);
    v
}

// ---- H264ENCTWOPASSFILE (LEDGER SR719): export --pass 1|2 --stats <file> (named tail block) ----
/// H264ENCTWOPASSFILE (SR719): where the two-pass export's first-pass records come from. `InProcess` is `--two-pass` (SR698,
/// `pass1_stats`); `Pass1(f)` is the CBR export with its records written to `f`; `Pass2(f)` the export planned from `f`.
enum TwoPass {
    Off,
    InProcess,
    Pass1(String),
    Pass2(String),
}

impl TwoPass {
    /// The export is planned from a first pass (the `with_two_pass` arm).
    fn plans(&self) -> bool {
        matches!(self, TwoPass::InProcess | TwoPass::Pass2(_))
    }
}

/// H264ENCTWOPASSFILE (SR719): the mode from `--two-pass`, `--pass N` and `--stats <f>` — plumbing only; each refusal by name, exit 1.
fn two_pass_of(two_pass: TwoPass, pass: Option<u8>, stats: Option<String>, bitrate: bool, video: &str) -> TwoPass {
    let mode = match (two_pass, pass, stats) {
        (TwoPass::InProcess, Some(_), _) => die("--two-pass and --pass are exclusive".into()),
        (TwoPass::InProcess, None, Some(_)) => die("--stats needs --pass 1|2".into()),
        (m, None, None) => m,
        (_, None, Some(_)) => die("--stats needs --pass 1|2".into()),
        (_, Some(_), None) => die("--pass needs --stats <file>".into()),
        (_, Some(1), Some(f)) => TwoPass::Pass1(f),
        (_, Some(2), Some(f)) => TwoPass::Pass2(f),
        (_, Some(_), Some(_)) => die(format!("--pass takes {}", PASS_WORDS.join(" or "))),
    };
    match mode {
        TwoPass::InProcess if !bitrate => die("--two-pass needs --bitrate".into()),
        TwoPass::Pass1(_) | TwoPass::Pass2(_) if !bitrate => die("--pass needs --bitrate".into()),
        TwoPass::Pass1(_) | TwoPass::Pass2(_) if video != "h264" => die("--pass needs --video h264".into()),
        m => m,
    }
}

/// H264ENCTWOPASSFILE (SR719): the first-pass records the planned export takes — `--two-pass`: `pass1_stats` unchanged; `--pass 2`:
/// `<f>` read through `rc::read_stats`, refused when its fps or picture count is not the timeline's (the count is the range in
/// ticks, the export's frames), then `nle: two-pass pass2 stats=<f> pictures=<n>` on stderr. A refusal is printed, exit 1.
#[allow(clippy::too_many_arguments)]
fn pass2_stats(two_pass: &TwoPass, tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, dir: &str, qp: u8, gop: u32, kbps: u32, scenecut: Option<u8>, aq: Option<u8>, container: Container) -> Vec<stria::nle_export::PicStat> {
    let TwoPass::Pass2(f) = two_pass else { return pass1_stats(tl, range, sources, dir, qp, gop, kbps, scenecut, aq, container) };
    let text = std::fs::read_to_string(f).unwrap_or_else(|e| die(format!("{f}: {e}")));
    let mut s = stria::nle_export::read_stats(&text).unwrap_or_else(|e| die(format!("{f}: {e}")));
    let fps = (tl.tb.den as u32, tl.tb.num as u32);
    if s.fps != fps {
        die(format!("{f}: fps {}/{}, the timeline {}/{}", s.fps.0, s.fps.1, fps.0, fps.1));
    }
    let frames = (range.end - range.start).max(0) as usize;
    if s.pics.len() != frames { eprintln!("nle: two-pass pass2 stats rescaled {} -> {frames}", s.pics.len()); s.pics = stria::nle_export::rescale_stats(&s.pics, frames); } // H264ENCTWOPASSRESCALE (SR727): another picture count plans from the curve resampled to the range's (rc::rescale_stats), not refused
    eprintln!("nle: two-pass pass2 stats={f} pictures={}", s.pics.len());
    s.pics
}

/// H264ENCTWOPASSFILE (SR719): pass 1's records (the CBR export's `H264Encoder::stats`) written to `f` in h264enc_core's ONE stats
/// text (`rc::write_stats`) at the timeline's fps; `nle: two-pass pass1 pictures=<n> bytes=<b> stats=<f>` on stderr.
fn write_pass1(f: &str, pics: &[stria::nle_export::PicStat], tl: &Timeline) {
    let mut text = String::new();
    stria::nle_export::write_stats(pics, (tl.tb.den as u32, tl.tb.num as u32), &mut text).unwrap_or_else(|e| die(format!("{f}: {e}")));
    std::fs::write(f, &text).unwrap_or_else(|e| die(format!("{f}: {e}")));
    eprintln!("nle: two-pass pass1 pictures={} bytes={} stats={f}", pics.len(), pics.iter().map(|p| p.bits).sum::<u64>() / 8);
}

// ---- NLECLIREFUSALSBYNAME (LEDGER SR801): the option loop's refusals by name and the help road (named tail block) ----
/// NLECLIREFUSALSBYNAME (SR801): THE grammar of a bad value — `nle: <flag> <s>: <what>` through [`die`], exit 1.
fn bad(flag: &str, s: &str, what: &str) -> ! {
    die(format!("{flag} {s}: {what}"))
}

/// NLECLIREFUSALSBYNAME (SR801): the value after `flag`; none left is `nle: <flag>: a value is required`, exit 1.
fn value(it: &mut std::slice::Iter<'_, String>, flag: &str) -> String {
    it.next().cloned().unwrap_or_else(|| die(format!("{flag}: a value is required")))
}

/// NLECLIREFUSALSBYNAME (SR801): THE table of what a numeric arm takes — the range its PARSE takes (the encoder stays the
/// authority on a narrower bound: h264enc_core's QP 16..=40, CRF <= 51), `--pass` two_pass_of's own 1 or 2; `<verb> --out` the
/// missing `--out` of render / export (NLECLIREFUSALSREST SR814: `nle: <verb>: --out <stem|dir> is required`).
fn what(flag: &str) -> String {
    match flag {
        "--qp" | "--crf" => format!("not a whole number within 0..={}", u8::MAX),
        "--pass" => format!("not {}", PASS_WORDS.join(" or ")),
        "render --out" => "--out <stem> is required".to_string(), // NLECLIREFUSALSREST (SR814): the missing flag, by verb
        "export --out" => "--out <dir> is required".to_string(),
        _ => format!("not a whole number within 0..={}", u32::MAX),
    }
}

/// NLECLIREFUSALSBYNAME (SR801): `flag`'s value parsed as `T`: a missing value by [`value`], an unparsed one by
/// [`bad`]`(flag, s, what(flag))`.
fn num<T: std::str::FromStr>(it: &mut std::slice::Iter<'_, String>, flag: &str) -> T {
    let s = value(it, flag);
    s.parse().unwrap_or_else(|_| bad(flag, &s, &what(flag)))
}

/// NLECLIREFUSALSBYNAME (SR801, NLE-CLI-HELP): a person asked for the usage — `nle help`, `nle --help`, or
/// `nle <schedule|render|export|size|--make-src|completions> --help`.
fn help_asked(args: &[String]) -> bool {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    matches!(a.as_slice(), ["help" | "--help", ..] | ["schedule" | "render" | "export" | "size" | "--make-src" | "completions", "--help", ..])
}

/// NLECLIREFUSALSBYNAME (SR801): THE usage text, to `w`, then exit `code` (`usage()`: stderr, 2; the help road: stdout, 0).
fn usage_to(w: &mut dyn Write, code: i32, verb: Option<&str>) -> ! {
    match verb { Some(v) => verb_usage(w, v), None => for (i, (_, line)) in USAGE_LINES.iter().enumerate() { let _ = writeln!(w, "{}{}", if i == 0 { "usage: " } else { "       " }, usage_fill(line)); } } // NLECLIHELPVERB (SR1041): Some(verb) its own section (file tail); None the six lines of USAGE_LINES, bytes unchanged
    let _ = w.flush();
    std::process::exit(code)
}

// ---- NLECLIDIDYOUMEAN (LEDGER SR816): the nearest arm / verb of an unknown word (named tail block) ----
/// NLECLIDIDYOUMEAN (SR816): THE list of the option loop's arm words, in arm order (a tie names the first). SR799's
/// `cli_usage_lists_every_flag` pins it equal to the arms its source scan finds, so a new arm without its word here reds.
const FLAGS: [&str; 24] = [
    "--fps", "--out", "--video", "--audio", "--aac", "--png", "--qp", "--gop", "--bitrate", "--mkv", "--crf", "--scenecut",
    "--min-keyint", "--two-pass", "--pass", "--stats", "--aq", "--hrd", "--qcomp", "--cplxblur", "--qblur", "--qpstep", "--pass2", "--src",
];

/// NLECLIDIDYOUMEAN (SR816): the words `main` reads as its first argument, in the usage's order (NLECLIPASS2VALUE SR821: `completions`).
const VERBS: [&str; 8] = ["schedule", "render", "export", "size", "--make-src", "completions", "help", "--help"];

/// NLECLIDIDYOUMEAN (SR816): the farthest a word may be from a candidate and still be named (git's autocorrect suggests
/// within a bound too; ours is plain distance, ONE candidate).
const NEAR: usize = 2;

/// NLECLIDIDYOUMEAN (SR816): the optimal-string-alignment Damerau-Levenshtein distance over chars — an insertion, a
/// deletion, a substitution or a swap of two neighbours each cost 1 (`--qbulr` -> `--qblur` is 1). Three rolling rows:
/// the swap reads the row two back.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut back: Vec<usize> = vec![0; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let sub = usize::from(a[i - 1] != b[j - 1]);
            let mut d = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + sub);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(back[j - 2] + 1);
            }
            cur[j] = d;
        }
        std::mem::swap(&mut back, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// NLECLIDIDYOUMEAN (SR816): THE nearest of `words` to `word` — the smallest [`edit_distance`] within [`NEAR`], the first
/// in `words` on a tie; none within [`NEAR`] is `None`.
fn nearest(word: &str, words: &[&'static str]) -> Option<&'static str> {
    let mut best: Option<(usize, &'static str)> = None;
    for &w in words {
        let d = edit_distance(word, w);
        if d <= NEAR && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, w));
        }
    }
    best.map(|(_, w)| w)
}

/// NLECLIDIDYOUMEAN (SR816): the option loop's unknown-flag refusal — `<a>: unknown flag; did you mean <f>?` when an arm
/// is within [`NEAR`], else SR801's text byte-identical.
fn unknown_flag(a: &str) -> String {
    match nearest(a, &FLAGS) {
        Some(f) => format!("{a}: unknown flag; did you mean {f}?"),
        None => format!("{a}: unknown flag (nle export --help lists them)"),
    }
}

/// NLECLIDIDYOUMEAN (SR816): a first word that is no verb but is within [`NEAR`] of one — `nle: <w>: unknown verb; did you
/// mean <v>?` on stderr, then the usage (exit 2), before the option loop and the EDL are read. Otherwise nothing: the
/// road is today's.
fn verb_hint(args: &[String]) {
    let Some(w) = args.first() else { return };
    if VERBS.contains(&w.as_str()) {
        return;
    }
    if let Some(v) = nearest(w, &VERBS) {
        eprintln!("nle: {w}: unknown verb; did you mean {v}?");
        usage()
    }
    if args.get(1).map(String::as_str) == Some("--help") { usage() } // NLECLIHELPUNKNOWNEXAMPLES (SR1049): a far unknown word + `--help` is the usage (stderr, exit 2), never `load("--help")`
}

// ---- NLECLIPASS2VALUE (LEDGER SR821): the shell completion scripts from FLAGS / VERBS (named tail block) ----
/// NLECLIPASS2VALUE (SR821): `nle completions <shell>`'s script, its words [`VERBS`] then [`FLAGS`] (a new arm's word is in
/// every script with no second list); `None` for a shell other than bash / zsh / fish. The grammar is each shell's own
/// (recalled): bash `complete -o default -W` (filenames when no word matches), zsh `#compdef` + `_arguments` (`*` = the
/// option may repeat, `--src` does), fish `complete -c nle` (`-a` a verb at the first word, `-l` a `--name` word).
fn completions(shell: &str) -> Option<String> {
    let words = || VERBS.iter().chain(FLAGS.iter()).copied();
    Some(match shell {
        "bash" => bash_script(&words().collect::<Vec<_>>().join(" ")),
        "zsh" => {
            let mut s = format!("#compdef nle\n{}_arguments '1:verb:({})' $flags", zsh_verbs(), VERBS.join(" ")); // NLECLICOMPLETIONSVERBFLAGS (SR1033): each verb's flag specs in `$flags` (zsh_verbs, file tail)
            s.push_str(&format!("{}{} '*:file:{{{}}}'\n", zsh_shells(), zsh_size(), zsh_file()));
            s
        }
        "fish" => words()
            .map(|w| match w.strip_prefix("--") {
                Some(l) => format!("complete -c nle{} -l {l}{}\n", fish_when(w), fish_value(w)),
                None => format!("complete -c nle -n __fish_use_subcommand -a {w}\n"),
            })
            .chain([fish_size(), fish_dirs(), fish_completions()]).collect(),
        _ => return None,
    })
}

// ---- NLECLICOMPLETIONSVALUES (LEDGER SR847): the words a flag's value takes, from the parser's own tables (named tail block) ----
/// NLECLICOMPLETIONSVALUES (SR847): THE value table of `nle completions` — each valued flag with the words its parse takes,
/// read from the parser's own tables (stria's `VIDEO_NAMES` / `AUDIO_NAMES` + `aac:` for `aac:KBPS` / `PASS2_KEYS` as `k=`),
/// so a new codec / audio / pass2 word reaches every script with no second list.
fn flag_values() -> [(&'static str, Vec<String>); 4] {
    let own = |t: &[&str]| t.iter().map(|w| w.to_string()).collect::<Vec<String>>();
    let mut audio = own(&stria::nle_export::AUDIO_NAMES[..]);
    audio.push("aac:".to_string());
    [
        ("--video", own(&VIDEO_NAMES[..])),
        ("--audio", audio),
        ("--pass2", stria::nle_export::PASS2_KEYS.iter().map(|k| format!("{k}=")).collect()),
        ("--pass", own(&PASS_WORDS[..])), // NLECLICOMPLETIONSPASSWORDSFILES (SR1019): two_pass_of's words
    ]
}

/// NLECLICOMPLETIONSVALUES (SR847): `nle size <edl> <reel> <policy>`'s words, timeline_core's `SizePolicy::ALL` by `name`.
fn size_words() -> String {
    timeline_core::SizePolicy::ALL.map(timeline_core::SizePolicy::name).join(" ")
}

/// NLECLICOMPLETIONSVALUES (SR847): a flag's value words from [`flag_values`] joined by spaces; `None` for a flag with no table.
fn values_of(flag: &str) -> Option<String> {
    flag_values().into_iter().find(|(f, _)| *f == flag).map(|(_, v)| v.join(" "))
}

/// NLECLICOMPLETIONSVALUES (SR847): bash's script — ONE `_nle` fn (a `-W` list cannot see the previous word): the verbs and
/// flags by default, a valued flag's words after it, the size policies at `nle size <edl> <reel> _`; filenames still when
/// nothing matches (`-o default`).
fn bash_script(words: &str) -> String {
    let mut s = format!("_nle() {{\n    local cur=${{COMP_WORDS[COMP_CWORD]}} prev=${{COMP_WORDS[COMP_CWORD-1]}} w=\"{words}\"\n{}    case $prev in\n", bash_verbs());
    for (f, v) in flag_values() {
        s.push_str(&format!("        {f}) w=\"{}\" ;;\n", v.join(" ")));
    }
    s.push_str(&bash_free()); s.push_str(&bash_paths()); s.push_str(&format!("    esac\n    [[ ${{COMP_WORDS[1]}} == size && $COMP_CWORD == 4 ]] && w=\"{}\"\n", size_words()));
    s.push_str(&bash_after()); s.push_str(&bash_shells()); s.push_str("    COMPREPLY=($(compgen -W \"$w\" -- \"$cur\"))\n}\ncomplete -o default -F _nle nle\n");
    s
}

/// NLECLICOMPLETIONSVALUES (SR847): zsh's `_arguments` argument of a valued flag, `:<name>:(<words>)`; empty for any other.
fn zsh_value(flag: &str) -> String {
    values_of(flag).map(|v| format!(":{}:({v})", flag.trim_start_matches('-'))).unwrap_or_else(|| zsh_arg(flag))
}

/// NLECLICOMPLETIONSVALUES (SR847): zsh's fourth positional — the size policies after `nle size <edl> <reel>`, files otherwise.
fn zsh_size() -> String {
    format!(" '4:policy:{{if [[ $words[2] == size ]]; then compadd -- {}; else {}; fi}}'", size_words(), zsh_file())
}

/// NLECLICOMPLETIONSVALUES (SR847): fish's ` -xa "<words>"` on a valued `-l` line (a value required, these words); empty for any other.
fn fish_value(word: &str) -> String {
    values_of(word).map(|v| format!(" -xa \"{v}\"")).unwrap_or_else(|| fish_arg(word))
}

/// NLECLICOMPLETIONSVALUES (SR847): fish's size line — the policies as the fourth word after `nle size`.
fn fish_size() -> String {
    format!("complete -c nle -n \"__fish_seen_subcommand_from size; and test (count (commandline -opc)) -eq 4\" -xa \"{}\"\n", size_words())
}

// ---- NLECLICOMPLETIONSFREE (LEDGER SR1005): what a flag word takes after it, parallel to FLAGS (named tail block) ----
/// NLECLICOMPLETIONSFREE (SR1005): what the option loop reads after a flag word — `Bare` nothing, `Free` a required value with
/// no word table (`value` / `num`), `Opt` an optional K (`--scenecut [K]` / `--aq [K]`: a peeked word taken only when it
/// parses), `Words` a required value from [`flag_values`] (SR847).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arg {
    Bare,
    Free,
    Opt,
    Words,
    /// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): a required value that is a path ([`PATH_FLAGS`]): files in every shell.
    Path,
    /// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): a directory (`completions --install [<dir>]`, [`COMPLETIONS_ARG`]).
    Dir,
}

/// NLECLICOMPLETIONSFREE (SR1005): THE kind of each [`FLAGS`] word, in FLAGS order (FLAGS stays the one word list). tests/cli.rs
/// `cli_completions_free` pins it to each arm's own read (`value(&mut it, ..)` / `num(&mut it, ..)` / `it.clone().next()`).
const FLAG_ARG: [Arg; 24] = [
    Arg::Free, Arg::Path, Arg::Words, Arg::Words, Arg::Bare, Arg::Bare, Arg::Free, Arg::Free, Arg::Free, Arg::Bare, Arg::Free, Arg::Opt,
    Arg::Free, Arg::Bare, Arg::Words, Arg::Path, Arg::Opt, Arg::Bare, Arg::Free, Arg::Free, Arg::Free, Arg::Free, Arg::Words, Arg::Path,
];

/// NLECLICOMPLETIONSFREE (SR1005): `flag`'s [`Arg`] by its place in [`FLAGS`]; `Bare` for a word not in it.
fn arg_of(flag: &str) -> Arg {
    FLAGS.iter().position(|f| *f == flag).map_or(Arg::Bare, |i| FLAG_ARG[i])
}

/// NLECLICOMPLETIONSFREE (SR1005): bash's `case $prev` arm of every `Free` flag — an empty word list (`-o default` still
/// gives readline's filenames); empty when no flag is `Free`.
fn bash_free() -> String {
    let free: Vec<&str> = FLAGS.iter().copied().filter(|f| arg_of(f) == Arg::Free).collect();
    if free.is_empty() { String::new() } else { format!("        {}) compopt +o default; w=\"\" ;;\n", free.join("|")) } // NLECLICOMPLETIONSPASSWORDSFILES (SR1019): a numeric value offers no filenames
}

/// NLECLICOMPLETIONSFREE (SR1005): zsh's argument of a flag with no word table — `Free` `:<name>: ` (a space action: the
/// message, no matches), `Opt` `::<name>: ` (an optional argument); empty for a `Bare` flag.
fn zsh_arg(flag: &str) -> String {
    let name = flag.trim_start_matches('-');
    match arg_of(flag) {
        Arg::Free => format!(":{name}: "),
        Arg::Opt => format!("::{name}: "),
        Arg::Path => format!(":{name}:{{compset -P \"*=\"; _files}}"), // NLECLICOMPLETIONSPASSWORDSFILES (SR1019): the path after the last `=` (`--src REEL=<media>`), else the word
        Arg::Bare | Arg::Words | Arg::Dir => String::new(),
    }
}

/// NLECLICOMPLETIONSFREE (SR1005): fish's ` -x` on a `Free` flag's `-l` line (a value required, no files, no `-a` words);
/// empty otherwise (fish cannot say an optional value for a separate word, so `Opt` is a plain `-l`).
fn fish_arg(word: &str) -> String {
    (match arg_of(word) { Arg::Free => " -x", Arg::Path => " -r -F", _ => "" }).to_string() // NLECLICOMPLETIONSPASSWORDSFILES (SR1019): a Path flag's value is files
}

// ---- NLECLICOMPLETIONSINSTALL (LEDGER SR1011): nle completions <shell> --install [<dir>] [--force] (named tail block) ----
/// NLECLICOMPLETIONSINSTALL (SR1011): the `completions` verb's own arm words (not the export loop's: `FLAGS` feeds
/// `unknown_flag` and every script's word list, so these stay out of it); SR816's pin in tests/cli.rs subtracts them by name.
const COMPLETIONS_FLAGS: [&str; 3] = ["--install", "--force", "--uninstall"]; // NLECLICOMPLETIONSUNINSTALL (SR1025): --uninstall

/// NLECLICOMPLETIONSINSTALL (SR1011): what follows `nle completions <shell>` — nothing prints `script` on stdout (SR821's road,
/// byte-identical); `--install [<dir>]` writes it through [`install_completions`] and prints `installed <path>` (or
/// `installed <path> (unchanged)`), zsh a second line naming the fpath step; `--force` overwrites a differing file. An
/// unknown word is refused by name ([`nearest`] over [`COMPLETIONS_FLAGS`]), every refusal through [`die`], exit 1.
fn completions_out(shell: &str, script: &str, rest: &[String]) {
    let (mut install, mut force, mut uninstall) = (None::<Option<std::path::PathBuf>>, false, None::<Option<std::path::PathBuf>>); // NLECLICOMPLETIONSUNINSTALL (SR1025): uninstall
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--install" => install = Some(match it.clone().next() { Some(d) if !d.starts_with("--") => { it.next(); Some(std::path::PathBuf::from(d)) } _ => None }),
            "--uninstall" => uninstall = Some(match it.clone().next() { Some(d) if !d.starts_with("--") => { it.next(); Some(std::path::PathBuf::from(d)) } _ => None }), // NLECLICOMPLETIONSUNINSTALL (SR1025)
            "--force" => force = true,
            _ => die(match nearest(a, &COMPLETIONS_FLAGS) {
                Some(f) => format!("completions {shell} {a}: unknown flag; did you mean {f}?"),
                None => format!("completions {shell} {a}: unknown flag (--install [<dir>], --uninstall [<dir>] or --force)"), // NLECLICOMPLETIONSUNINSTALL (SR1025)
            }),
        }
    }
    // NLECLICOMPLETIONSUNINSTALL (SR1025): `--uninstall [<dir>]` removes what `--install [<dir>]` writes (before the
    // `--force needs --install` refusal: `--uninstall --force` removes a file that is not ours).
    if let Some(dir) = uninstall {
        if install.is_some() {
            die(format!("completions {shell}: --install and --uninstall together (one at a time)"));
        }
        let (path, absent) = uninstall_completions(shell, dir, force, script).unwrap_or_else(|e| die(e));
        println!("removed {}{}", path.display(), if absent { " (absent)" } else { "" });
        return;
    }
    let Some(dir) = install else {
        if force {
            die(format!("completions {shell}: --force needs --install"));
        }
        print!("{script}");
        return;
    };
    let done = install_completions(shell, script, dir, force).unwrap_or_else(|e| die(e));
    println!("installed {}{}", done.path.display(), if done.unchanged { " (unchanged)" } else { "" });
    if shell == "zsh" {
        let d = done.dir.display();
        println!("zsh: add {d} to fpath before compinit, e.g. fpath=({d} $fpath) in ~/.zshrc");
    }
}

/// NLECLICOMPLETIONSINSTALL (SR1011): where [`install_completions`] wrote (or found) the script.
struct Installed {
    path: std::path::PathBuf,
    dir: std::path::PathBuf,
    unchanged: bool,
}

/// NLECLICOMPLETIONSINSTALL (SR1011): `script` written as [`completion_file`] in `dir` (else [`completion_dir`]): the same
/// bytes already there is `unchanged` and NOT written (the mtime kept); other bytes are refused without `force`
/// (`exists <path> (differs; --force to overwrite)`: a hand-edited script is not clobbered); else the dir is created and
/// the file written.
fn install_completions(shell: &str, script: &str, dir: Option<std::path::PathBuf>, force: bool) -> Result<Installed, String> {
    let (dir, path, found) = completions_target(shell, dir, script)?; // NLECLICOMPLETIONSUNINSTALL (SR1025): the path + compare lifted
    match found {
        Some(true) => return Ok(Installed { path, dir, unchanged: true }),
        Some(false) if !force => return Err(format!("exists {} (differs; --force to overwrite)", path.display())),
        _ => {}
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(&path, script).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Installed { path, dir, unchanged: false })
}

/// NLECLICOMPLETIONSINSTALL (SR1011): the shell's user completion directory — bash-completion v2's
/// `${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions`, zsh `${XDG_DATA_HOME:-$HOME/.local/share}/zsh/site-functions`
/// (zsh has no standard user dir: the fpath line names it), fish `${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions`. An
/// XDG variable unset, empty or relative is ignored (the XDG spec); HOME unset or relative is refused by name.
fn completion_dir(shell: &str) -> Result<std::path::PathBuf, String> {
    let abs = |v: &str| std::env::var_os(v).map(std::path::PathBuf::from).filter(|p| p.is_absolute());
    let base = |xdg: &str, under_home: &str| {
        abs(xdg).or_else(|| abs("HOME").map(|h| h.join(under_home))).ok_or_else(|| format!("completions {shell} --install: HOME is not set (give --install <dir>)"))
    };
    Ok(match shell {
        "fish" => base("XDG_CONFIG_HOME", ".config")?.join("fish").join("completions"),
        "zsh" => base("XDG_DATA_HOME", ".local/share")?.join("zsh").join("site-functions"),
        _ => base("XDG_DATA_HOME", ".local/share")?.join("bash-completion").join("completions"),
    })
}

/// NLECLICOMPLETIONSINSTALL (SR1011): the script's file name in [`completion_dir`] — each shell's own convention (bash the
/// command's name, zsh `_<command>` for `#compdef`, fish `<command>.fish`).
fn completion_file(shell: &str) -> &'static str {
    match shell {
        "zsh" => "_nle",
        "fish" => "nle.fish",
        _ => "nle",
    }
}

// ---- NLECLICOMPLETIONSPASSWORDSFILES (LEDGER SR1019): --pass's words from ONE table; Path / Dir completion kinds (named tail block) ----
/// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): THE words `--pass` takes (`two_pass_of`'s `Some(1)` / `Some(2)` arms): read by
/// `two_pass_of`'s refusal, `what("--pass")` and [`flag_values`], so the completion words and the refusal cannot drift.
const PASS_WORDS: [&str; 2] = ["1", "2"];

/// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): the [`FLAGS`] words whose value is a path (`--out <stem|dir>`, `--stats <file>`,
/// `--src REEL=<media>`); [`FLAG_ARG`] tags them `Path` and stays the table the generators read; tests/cli.rs
/// `cli_completions_files` pins this list equal to FLAG_ARG's Path words, each a required value by the arm scan.
#[allow(dead_code)] // read from the source by tests/cli.rs (the named witness of FLAG_ARG's Path tags)
const PATH_FLAGS: [&str; 3] = ["--out", "--stats", "--src"];

/// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): the kind of each [`COMPLETIONS_FLAGS`] word, in its order (FLAG_ARG's shape):
/// `--install [<dir>]` a directory, `--force` nothing.
const COMPLETIONS_ARG: [Arg; 3] = [Arg::Dir, Arg::Bare, Arg::Dir]; // NLECLICOMPLETIONSUNINSTALL (SR1025): --uninstall [<dir>]

/// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): the completions verb's words that take a directory ([`COMPLETIONS_ARG`] `Dir`).
fn dir_words() -> Vec<&'static str> {
    COMPLETIONS_FLAGS.iter().zip(COMPLETIONS_ARG).filter(|(_, a)| *a == Arg::Dir).map(|(f, _)| *f).collect()
}

/// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): bash's `case $prev` arms of the `Path` flags (an empty list: `-o default`
/// gives readline's filenames) and the `Dir` words (`compopt -o dirnames +o default`: directories only).
fn bash_paths() -> String {
    let path: Vec<&str> = FLAGS.iter().copied().filter(|f| arg_of(f) == Arg::Path).collect();
    let mut s = String::new();
    if !path.is_empty() {
        s.push_str(&format!("        {}) w=\"\" ;;\n", path.join("|")));
    }
    let dirs = dir_words();
    if !dirs.is_empty() {
        s.push_str(&format!("        {}) compopt -o dirnames +o default; w=\"\" ;;\n", dirs.join("|")));
    }
    s
}

/// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): bash's lines after `esac` — the positional `<edl|media>` (and `size <edl>`,
/// `--make-src <out>`) at word 2 offers files, not the verb / flag words; after a `=` (`--src REEL=`: COMP_WORDS splits
/// at it, and readline's default word breaks there too) no words, so `-o default` completes the path.
fn bash_after() -> String {
    "    [[ $COMP_CWORD == 2 && ${COMP_WORDS[1]} != completions && $cur != -* ]] && w=\"\"\n    [[ $prev == = || $cur == = ]] && w=\"\"\n".to_string()
}

/// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): zsh's file action of the rest spec and the size spec's else — directories
/// after a `Dir` word (`--install`), files otherwise (whether or not `_arguments` counts `--install` as a positional).
fn zsh_file() -> String {
    format!("if [[ $words[CURRENT-1] == ({}) ]]; then _files -/; else _files; fi", dir_words().join("|"))
}

/// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): fish's line per `Dir` word — the option declared under the completions verb
/// only (fish completes an option's argument only for a declared option), its value directories.
fn fish_dirs() -> String {
    dir_words()
        .iter()
        .map(|w| format!("complete -c nle -n \"__fish_seen_subcommand_from completions\" -l {} -x -a \"(__fish_complete_directories)\"\n", w.trim_start_matches('-')))
        .collect()
}

// ---- NLECLICOMPLETIONSUNINSTALL (LEDGER SR1025): nle completions <shell> --uninstall [<dir>] [--force] (named tail block) ----
/// NLECLICOMPLETIONSUNINSTALL (SR1025): THE target of `--install` and `--uninstall` — the dir (`dir`, else [`completion_dir`]),
/// the file ([`completion_file`] in it) and what is there now, by ONE read: `None` nothing, `Some(true)` exactly `script`'s
/// bytes (ours), `Some(false)` other bytes (a hand-written script); a read error other than NotFound is `<path>: <error>`.
/// Lifted verbatim from SR1011's `install_completions` head, so the two roads cannot name different files.
fn completions_target(shell: &str, dir: Option<std::path::PathBuf>, script: &str) -> Result<(std::path::PathBuf, std::path::PathBuf, Option<bool>), String> {
    let dir = match dir {
        Some(d) => d,
        None => completion_dir(shell)?,
    };
    let path = dir.join(completion_file(shell));
    let found = match std::fs::read(&path) {
        Ok(b) => Some(b == script.as_bytes()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    Ok((dir, path, found))
}

/// NLECLICOMPLETIONSUNINSTALL (SR1025): remove the file `--install [<dir>]` would write ([`completions_target`]): nothing
/// there is `(path, true)` (absent: idempotent); bytes that are not `script`'s are refused without `force`
/// (`<path>: not nle's completion script (use --force)`: a hand-written `_nle` is never deleted silently); else the file
/// is removed, `(path, false)`. The dir is left in place (it is the shell's, not nle's).
fn uninstall_completions(shell: &str, dir: Option<std::path::PathBuf>, force: bool, script: &str) -> Result<(std::path::PathBuf, bool), String> {
    let (_, path, found) = completions_target(shell, dir, script)?;
    match found {
        None => Ok((path, true)),
        Some(false) if !force => Err(format!("{}: not nle's completion script (use --force)", path.display())),
        Some(_) => {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok((path, false))
        }
    }
}

// ---- NLECLICOMPLETIONSVERBFLAGS (LEDGER SR1033): each verb's own flag words in every completion script (named tail block) ----
/// NLECLICOMPLETIONSVERBFLAGS (SR1033): THE shells `nle completions <shell>` writes a script for ([`completions`]' three
/// arms): main's dispatch refusal names them and every script offers them at `nle completions _`.
const SHELLS: [&str; 3] = ["bash", "zsh", "fish"];

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): THE flag words each [`VERBS`] word takes, in VERBS order. schedule / render / export
/// share ONE option loop, so each lists the FLAGS whose loop variable it READS (`load`'s `fps` / `srcs` for all three,
/// render's `out`, export every one); `size` its one `--fps` literal; `--make-src` / `help` / `--help` none; `completions`
/// its own [`COMPLETIONS_FLAGS`]. tests/cli.rs `cli_completions_verb_flags_scan` pins it to those reads both ways.
const VERB_FLAGS: [(&str, &[&str]); 8] = [
    ("schedule", &["--fps", "--src"]),
    ("render", &["--fps", "--out", "--src"]),
    ("export", &FLAGS),
    ("size", &["--fps"]),
    ("--make-src", &[]),
    ("completions", &COMPLETIONS_FLAGS),
    ("help", &[]),
    ("--help", &[]),
];

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): the verbs whose [`VERB_FLAGS`] list holds `flag`, in VERBS order.
fn verbs_of(flag: &str) -> Vec<&'static str> {
    VERB_FLAGS.iter().filter(|(_, f)| f.contains(&flag)).map(|(v, _)| *v).collect()
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): bash's verb case before `case $prev` — `f` the verb's flag words (a name of its own,
/// so SR847 / SR1005's `) w="` arms stay the only ones), taken as `w` past the verb's own word (word 1 keeps the verbs).
fn bash_verbs() -> String {
    let mut s = String::from("    local f=\"\"\n    case ${COMP_WORDS[1]} in\n");
    for (v, flags) in VERB_FLAGS {
        s.push_str(&format!("        {v}) f=\"{}\" ;;\n", flags.join(" ")));
    }
    s.push_str("    esac\n    (( COMP_CWORD > 1 )) && w=$f\n");
    s
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): bash's [`SHELLS`] at `nle completions _` (SR847's size-line shape).
fn bash_shells() -> String {
    format!("    [[ ${{COMP_WORDS[1]}} == completions && $COMP_CWORD == 2 ]] && w=\"{}\"\n", SHELLS.join(" "))
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): zsh's `flags` array per verb (`case $words[2]`), each spec SR847 / SR1005 / SR1019's
/// [`zsh_value`]; `_arguments` reads `$flags` (empty before a verb and for a verb with none).
fn zsh_verbs() -> String {
    let mut s = String::from("local -a flags\ncase $words[2] in\n");
    for (v, flags) in VERB_FLAGS {
        let specs: String = flags.iter().map(|f| format!(" '*{f}{}'", zsh_value(f))).collect();
        s.push_str(&format!("    {v}) flags=({specs} ) ;;\n"));
    }
    s.push_str("esac\n");
    s
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): zsh's second positional — [`SHELLS`] after `nle completions`, else SR1019's file
/// action ([`zsh_size`]'s shape).
fn zsh_shells() -> String {
    format!(" '2:shell:{{if [[ $words[2] == completions ]]; then compadd -- {}; else {}; fi}}'", SHELLS.join(" "), zsh_file())
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): fish's condition on a flag's `-l` line — the verbs that take it ([`fish_dirs`]' shape).
fn fish_when(flag: &str) -> String {
    format!(" -n \"__fish_seen_subcommand_from {}\"", verbs_of(flag).join(" "))
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): fish's [`SHELLS`] line at `nle completions _` ([`fish_size`]'s count test) and a line
/// per non-`Dir` [`COMPLETIONS_FLAGS`] word (`--force`; the `Dir` words are [`fish_dirs`]' lines).
fn fish_completions() -> String {
    let mut s = format!("complete -c nle -n \"__fish_seen_subcommand_from completions; and test (count (commandline -opc)) -eq 2\" -xa \"{}\"\n", SHELLS.join(" "));
    for (w, a) in COMPLETIONS_FLAGS.iter().zip(COMPLETIONS_ARG) {
        if a != Arg::Dir {
            s.push_str(&format!("complete -c nle{} -l {}\n", fish_when(w), w.trim_start_matches('-')));
        }
    }
    s
}

// ---- NLECLIHELPVERB (LEDGER SR1041): per-verb `--help` from ONE usage table and VERB_FLAGS (named tail block) ----
/// NLECLIHELPVERB (SR1041): THE usage lines, one per verb shape in the usage's order, without the `usage: ` / indent prefix
/// ([`usage_to`] adds it). `{video}` / `{audio}` are stria's tables, filled by [`usage_fill`]. The full usage and each
/// verb's `--help` section ([`verb_usage`]) print from this one table.
const USAGE_LINES: [(&str, &str); 6] = [
    ("schedule", "nle schedule <edl|media> [--fps N] --src REEL=<media> ..."),
    ("render", "nle render <edl|media> [--fps N] --src REEL=<media> ... --out <stem>"),
    ("export", "nle export <edl|media> [--fps N] --src REEL=<media> ... --out <dir> [--video {video}] [--qp N] [--gop N] [--bitrate KBPS | --crf N] [--scenecut [K]] [--min-keyint N] [--aq [K]] [--two-pass | --pass 1|2 --stats <file>] [--qcomp F] [--cplxblur N] [--qblur F] [--qpstep N] [--pass2 k=v,...] [--hrd] [--audio {audio}|aac:KBPS] [--aac] [--mkv] [--png]"),
    ("--make-src", "nle --make-src <out.mp4> <base> [frames] [fps]"),
    ("size", "nle size <edl> <reel> <letterbox|fill|stretch|adopt> [--fps N]"),
    ("completions", "nle completions bash|zsh|fish [--install [<dir>] | --uninstall [<dir>]] [--force]"),
];

/// NLECLIHELPVERB (SR1041): `line` with `{video}` / `{audio}` replaced by stria's `VIDEO_NAMES` / `AUDIO_NAMES` joined `|`.
fn usage_fill(line: &str) -> String {
    line.replace("{video}", &VIDEO_NAMES.join("|")).replace("{audio}", &stria::nle_export::AUDIO_NAMES.join("|"))
}

/// NLECLIHELPVERB (SR1041): the value word after the whole-word `flag` in a usage `line` — up to the first space or the first
/// unmatched `]` at bracket depth 0 (`--scenecut [K]` -> `[K]`, `--pass2 k=v,...]` -> `k=v,...`); empty when the flag is
/// absent or followed by `]`, `|` or another flag (`--two-pass |`, `[--aac]`, `[--force]`).
fn flag_word<'a>(line: &'a str, flag: &str) -> &'a str {
    let whole = |i: usize| (i == 0 || line[..i].ends_with([' ', '['])) && (line[i + flag.len()..].is_empty() || line[i + flag.len()..].starts_with([' ', ']']));
    let Some(i) = line.match_indices(flag).map(|(i, _)| i).find(|&i| whole(i)) else { return "" };
    let rest = line[i + flag.len()..].strip_prefix(' ').unwrap_or("");
    let mut depth = 0usize;
    let end = rest
        .char_indices()
        .find(|&(_, c)| match c {
            '[' => {
                depth += 1;
                false
            }
            ']' if depth == 0 => true,
            ']' => {
                depth -= 1;
                false
            }
            ' ' => depth == 0,
            _ => false,
        })
        .map_or(rest.len(), |(j, _)| j);
    let word = &rest[..end];
    if word.is_empty() || word.starts_with(['|', '-']) || word.starts_with("[-") { "" } else { word }
}

/// NLECLIHELPVERB (SR1041): `nle <verb> --help` — the verb's [`USAGE_LINES`] line as `usage: <line>`, then one line per
/// [`VERB_FLAGS`] word in its order, `  <flag>` and its value word cut from that same line ([`flag_word`]) when it has one.
fn verb_usage(w: &mut dyn Write, verb: &str) {
    let line = USAGE_LINES.iter().find(|(v, _)| *v == verb).map(|(_, l)| usage_fill(l)).unwrap_or_default();
    let _ = writeln!(w, "usage: {line}");
    let flags: &[&str] = VERB_FLAGS.iter().find(|(v, _)| *v == verb).map_or(&[][..], |(_, f)| *f);
    for f in flags {
        let word = flag_word(&line, f);
        let _ = writeln!(w, "  {f}{}", if word.is_empty() { String::new() } else { format!(" {word}") });
    }
    if let Some((_, e)) = USAGE_EXAMPLES.iter().find(|(v, _)| *v == verb) { let _ = writeln!(w, "example: {e}"); } // NLECLIHELPUNKNOWNEXAMPLES (SR1049): the verb's example last (file tail)
}

// ---- NLECLIHELPUNKNOWNEXAMPLES (LEDGER SR1049): one example command line per verb, printed by `nle <verb> --help` (named tail block) ----
/// NLECLIHELPUNKNOWNEXAMPLES (SR1049): THE example per [`USAGE_LINES`] verb, same keys and order; [`verb_usage`] prints it
/// last as `example: <line>` (the full usage carries none). The paths are relative: tests/cli.rs `cli_help_examples_run`
/// runs every line in a fixture dir holding `cut.edl` (reels A and B), `a.mp4` and `b.mp4`, so an example cannot drift
/// from the parser.
const USAGE_EXAMPLES: [(&str, &str); 6] = [
    ("schedule", "nle schedule cut.edl --src A=a.mp4 --src B=b.mp4"),
    ("render", "nle render cut.edl --src A=a.mp4 --src B=b.mp4 --out frame"),
    ("export", "nle export cut.edl --src A=a.mp4 --src B=b.mp4 --out out --video h264 --bitrate 2500"),
    ("--make-src", "nle --make-src a.mp4 0 50 25"),
    ("size", "nle size cut.edl A letterbox"),
    ("completions", "nle completions bash --install"),
];
