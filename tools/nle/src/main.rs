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
//! nle completions bash|zsh|fish
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
use stria::nle_export::{AudioOut, Container, ExportReport, H264Encoder, H264Opts, I420Raw, VIDEO_NAMES, VideoEncoder, Y4m, export_as, export_png, Pass2Cli, pass2_cli_of, pass2_words, preset_named, preset_words, qblur_of, qcomp_of, qpstep_of, video_named_with};
use timeline_core::schedule::schedule;
use timeline_core::{Kind, Timeline, edl};

fn usage() -> ! {
    usage_to(&mut std::io::stderr(), 2) // NLECLIREFUSALSBYNAME (SR801): the five lines in `usage_to` (file tail): stderr, exit 2
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
    for s in tl.sources.iter().filter(|s| s.title.is_none()) { // STRIATITLECLIP (SR828): a title source has no file
        let Some((_, path)) = srcs.iter().find(|(r, _)| *r == s.reel) else { die(format!("no --src for reel {}", s.reel)) };
        if stria::nle::seq_pattern(path).is_some() { sources.add_sequence(s.id, path).unwrap_or_else(|e| die(format!("{path}: {e}"))); continue; } let bytes = std::fs::read(path).unwrap_or_else(|e| die(format!("{path}: {e}"))); // STRIAIMPORTIMAGESEQUENCE (SR888): a `%04d` path is a PNG sequence reel
        sources.add(s.id, bytes).unwrap_or_else(|e| die(format!("{path}: {e}")));
    }
    (tl, sources)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--make-src") {
        let out = args.get(1).unwrap_or_else(|| usage());
        let num = |i: usize, d: u32| args.get(i).map(|s| s.parse().unwrap_or_else(|_| bad("--make-src", s, &what("--make-src")))).unwrap_or(d); // NLECLIREFUSALSREST (SR814): a bad number by name
        let (base, frames, fps) = (num(2, 0), num(3, 50), num(4, 25));
        std::fs::write(out, synth_source(base, frames, fps)).unwrap_or_else(|e| die(format!("{out}: {e}")));
        return;
    }
    if args.first().map(String::as_str) == Some("size") { size_verb(&args[1..]); return; } // NLESIZEPOLICYUI (SR402): the reel's size policy into the EDL (file tail)
    if args.first().map(String::as_str) == Some("completions") { let sh = value(&mut args[1..].iter(), "completions"); print!("{}", completions(&sh).unwrap_or_else(|| bad("completions", &sh, "one of bash|zsh|fish"))); return; } if args.first().map(String::as_str) == Some("frame") { frame_verb(&args[1..]); return; } // STRIAEXPORTFRAME (SR830): the frame at a tick as a PNG (file tail)
    if help_asked(&args) { usage_to(&mut std::io::stdout(), 0) } verb_hint(&args); let (Some(verb), Some(edl_path)) = (args.first(), args.get(1)) else { usage() }; // NLECLIREFUSALSBYNAME (SR801): `nle help` / `--help` / `<verb> --help` print the usage on stdout, exit 0 (file tail); NLECLIPASS2VALUE (SR821): `nle completions <shell>` prints the script on stdout, exit 0 (file tail)
    let (mut fps, mut out, mut srcs) = (25u32, None::<String>, Vec::new());
    let (mut video, mut audio) = ("png".to_string(), "wav".to_string());
    let (mut qp, mut gop, mut bitrate, mut crf) = (26u8, None::<u32>, None::<u32>, None::<u8>);
    let mut scenecut = None::<u8>; let mut aq = None::<u8>; let mut two_pass = TwoPass::Off; let (mut pass, mut stats) = (None::<u8>, None::<String>); let mut hrd = false; let mut min_keyint = None::<u32>; let mut p2 = Pass2Cli::default(); let mut preset_crf = false; let mut scale = 1u8; // STRIAEXPORTPRESETSNEXT (SR989): `--scale N`, the frame's divisor (a preset sets it); H264ENCSCENECUT (SR680): `--scenecut [K]`, h264enc_core's `Config::scenecut`; STRIAEXPORTPRESETSMORE (SR948): `preset_crf` = the crf came from a `--preset`
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
            "--bitrate" => { bitrate = Some(num(&mut it, "--bitrate")); if preset_crf { crf = None } } // STRIAEXPORTPRESETSMORE (SR948): a `--bitrate` after a crf preset drops the preset's crf (the last flag written wins)
            "--mkv" => container = Container::Mkv,
            "--crf" => { crf = Some(num(&mut it, "--crf")); preset_crf = false } // STRIAEXPORTPRESETSMORE (SR948): an operator's own crf stays exclusive with --bitrate
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
            "--preset" => { let s = value(&mut it, "--preset"); let p = preset_named(&s).unwrap_or_else(|| bad("--preset", &s, &format!("one of {}", preset_words()))); (video, audio, qp, crf, bitrate, scenecut, aq) = (p.video.into(), p.audio.into(), p.qp, p.crf, None, p.scenecut, p.aq); preset_crf = p.crf.is_some(); scale = p.scale; } // STRIAEXPORTPRESETS (SR871): THE preset table assigns the knobs below it; the last flag written wins; STRIAEXPORTPRESETSMORE (SR948): it records whether the crf is the preset's
            "--scale" => scale = num(&mut it, "--scale"), // STRIAEXPORTPRESETSNEXT (SR989): the export frame's width / height divisor through stria's `scaled` (1 = the timeline's size)
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
                let track = s.track.and_then(|t| tl.track_name(t)).unwrap_or_else(|| "-".into()); // STRIATRACKGAINTHREE (SR9024): timeline_core's ONE name rule
                let src = s.src.and_then(|id| tl.source(id)).map(|x| x.reel.as_str()).unwrap_or("-");
                let seek = s.seek.map(|k| k.to_string()).unwrap_or_else(|| "-".into());
                println!("out={} kind={kind} track={track} src={src} src_t={} len={} seek_ns={seek}{}", s.out_t, s.src_t, s.len, timeline_core::schedule::stack_line(&tl, &s).map(|x| format!(" {x}")).unwrap_or_default()); // STRIASTACKTOOL (SR9093): a stack step's layers
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
            export_verb(&tl, range, &sources, &out.unwrap_or_else(|| die(format!("export: {}", what("export --out")))), &video, &audio, qp, gop, bitrate, crf, scenecut, min_keyint, aq, two_pass, hrd, p2, container, scale) // STRIAEXPORTPRESETSNEXT (SR989): the divisor
        }
        _ => usage(),
    }
}

/// NLE5 (SR204): the `export` verb — plumbing only; the export is `stria::nle_export`'s.
#[allow(clippy::too_many_arguments)]
fn export_verb(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, dir: &str, video: &str, audio: &str, qp: u8, gop: u32, bitrate: Option<u32>, crf: Option<u8>, scenecut: Option<u8>, min_keyint: Option<u32>, aq: Option<u8>, two_pass: TwoPass, hrd: bool, p2: Pass2Cli, container: Container, scale: u8) { // STRIAEXPORTPRESETSNEXT (SR989): `scale` wraps the encoder
    let two_pass_stats = match bitrate { Some(k) if two_pass.plans() && video == "h264" => Some(pass2_stats(&two_pass, tl, range.clone(), sources, dir, qp, gop, k, scenecut, aq, container)), _ => None }; if two_pass_stats.is_some() { eprintln!("nle: two-pass plan {}", pass2_words(&p2)); } // NLE5DCLITABLE (SR718): the planned pass of --two-pass / --pass 2 (H264ENCTWOPASSFILE SR719: `pass2_stats`, file tail), then the ONE table
    let mut kept = None; let enc: Box<dyn VideoEncoder> = match (video, bitrate, &two_pass) { ("h264", Some(k), TwoPass::Pass1(_)) => { let e = H264Encoder::new(qp, gop).with_scenecut(scenecut).with_min_keyint(min_keyint).with_aq(aq).with_hrd(hrd).with_bitrate(k, tl.tb.den as u32, tl.tb.num as u32); kept = Some(e.stats()); Box::new(e) } _ => video_named_with(video, tl.tb, p2.into_opts(H264Opts { qp, gop: Some(gop), bitrate, crf, scenecut, aq, min_keyint, hrd, two_pass_stats, ..H264Opts::default() })).unwrap_or_else(|| bad("--video", video, &format!("one of {}", VIDEO_NAMES.join("|")))) }; let enc = stria::nle_export::scaled(enc, scale); // STRIAEXPORTPRESETSNEXT (SR989): 1 hands the encoder back itself. H264ENCTWOPASSFILE (SR719): `--pass 1` keeps the records' handle, which the table cannot hand out (owed H264ENC-TWOPASS-FILE-TABLE)
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
    let pass1 = std::path::Path::new(dir).join("pass1");
    std::fs::create_dir_all(&pass1).unwrap_or_else(|e| die(format!("{}: {e}", pass1.display())));
    let enc = H264Encoder::new(qp, gop).with_scenecut(scenecut).with_aq(aq).with_bitrate(kbps, tl.tb.den as u32, tl.tb.num as u32);
    let stats = enc.stats();
    export_as(tl, range, sources, &pass1, Box::new(enc), AudioOut::None, container).unwrap_or_else(|e| die(e.to_string()));
    std::fs::remove_dir_all(&pass1).unwrap_or_else(|e| die(format!("{}: {e}", pass1.display())));
    let v = stats.borrow().clone();
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
        (_, Some(_), Some(_)) => die("--pass takes 1 or 2".into()),
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
        "--pass" => "not 1 or 2".to_string(),
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
/// `nle <schedule|render|export> --help`.
fn help_asked(args: &[String]) -> bool {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    matches!(a.as_slice(), ["help" | "--help", ..] | ["schedule" | "render" | "export", "--help", ..])
}

/// NLECLIREFUSALSBYNAME (SR801): THE usage text, to `w`, then exit `code` (`usage()`: stderr, 2; the help road: stdout, 0).
fn usage_to(w: &mut dyn Write, code: i32) -> ! {
    let _ = writeln!(w, "usage: nle schedule <edl|media> [--fps N] --src REEL=<media> ...");
    let _ = writeln!(w, "       nle render <edl|media> [--fps N] --src REEL=<media> ... --out <stem>");
    let _ = writeln!(w, "       nle export <edl|media> [--fps N] --src REEL=<media> ... --out <dir> [--video {}] [--qp N] [--gop N] [--bitrate KBPS | --crf N] [--scenecut [K]] [--min-keyint N] [--aq [K]] [--two-pass | --pass 1|2 --stats <file>] [--qcomp F] [--cplxblur N] [--qblur F] [--qpstep N] [--pass2 k=v,...] [--hrd] [--audio {}|aac:KBPS] [--aac] [--mkv] [--png] [--preset {}] [--scale N]", VIDEO_NAMES.join("|"), stria::nle_export::AUDIO_NAMES.join("|"), preset_words()); // STRIAEXPORTPRESETS (SR871): the preset words
    let _ = writeln!(w, "       nle --make-src <out.mp4> <base> [frames] [fps]");
    let _ = writeln!(w, "       nle size <edl> <reel> <letterbox|fill|stretch|adopt> [--fps N]");
    let _ = writeln!(w, "       nle completions bash|zsh|fish");
    let _ = w.flush();
    std::process::exit(code)
}

// ---- NLECLIDIDYOUMEAN (LEDGER SR816): the nearest arm / verb of an unknown word (named tail block) ----
/// NLECLIDIDYOUMEAN (SR816): THE list of the option loop's arm words, in arm order (a tie names the first). SR799's
/// `cli_usage_lists_every_flag` pins it equal to the arms its source scan finds, so a new arm without its word here reds.
const FLAGS: [&str; 26] = [
    "--fps", "--out", "--video", "--audio", "--aac", "--png", "--qp", "--gop", "--bitrate", "--mkv", "--crf", "--scenecut",
    "--min-keyint", "--two-pass", "--pass", "--stats", "--aq", "--hrd", "--qcomp", "--cplxblur", "--qblur", "--qpstep", "--pass2", "--preset", "--scale", "--src",
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
}

// ---- NLECLIPASS2VALUE (LEDGER SR821): the shell completion scripts from FLAGS / VERBS (named tail block) ----
/// NLECLIPASS2VALUE (SR821): `nle completions <shell>`'s script, its words [`VERBS`] then [`FLAGS`] (a new arm's word is in
/// every script with no second list); `None` for a shell other than bash / zsh / fish. The grammar is each shell's own
/// (recalled): bash `complete -o default -W` (filenames when no word matches), zsh `#compdef` + `_arguments` (`*` = the
/// option may repeat, `--src` does), fish `complete -c nle` (`-a` a verb at the first word, `-l` a `--name` word).
fn completions(shell: &str) -> Option<String> {
    let words = || VERBS.iter().chain(FLAGS.iter()).copied();
    Some(match shell {
        "bash" => format!("complete -o default -W \"{}\" nle\n", words().collect::<Vec<_>>().join(" ")),
        "zsh" => {
            let mut s = format!("#compdef nle\n_arguments '1:verb:({})'", VERBS.join(" "));
            for f in FLAGS {
                s.push_str(&format!(" '*{f}'"));
            }
            s.push_str(" '*:file:_files'\n");
            s
        }
        "fish" => words()
            .map(|w| match w.strip_prefix("--") {
                Some(l) => format!("complete -c nle -l {l}\n"),
                None => format!("complete -c nle -n __fish_use_subcommand -a {w}\n"),
            })
            .collect(),
        _ => return None,
    })
}

// ---- STRIAEXPORTFRAME (LEDGER SR830): `nle frame`, the frame at a tick as a PNG through the face's one save (named tail block) ----
/// `nle frame <edl|media> <tick> [<out.png>] [--fps N] [--src REEL=<media> ...]`: the frame tick `<tick>` shows, saved by
/// `stria::nle_face::save_frame` (the editor face's `frame` verb's own fn: the preview road and pixel_core's PNG writer) to
/// `<out.png>`, else `stria::nle_face::frame_path` beside the EDL (`<stem>-<HH-MM-SS-FF>.png`). Prints
/// `nle: frame <tick> <w>x<h> -> <path>`; a refusal is `nle: frame: <why>`, exit 1. Not in the usage text (owed
/// NLE-EXPORT-FRAME-USAGE); a tick only (owed NLE-EXPORT-FRAME-TC).
fn frame_verb(args: &[String]) {
    let (Some(path), Some(at)) = (args.first(), args.get(1)) else { usage() };
    let t: i64 = at.parse().unwrap_or_else(|_| bad("frame", at, "not a whole tick"));
    let mut rest = args[2..].iter().peekable();
    let out = rest.next_if(|a| !a.starts_with("--")).cloned();
    let (mut fps, mut srcs) = (25u32, Vec::new());
    let flags: Vec<String> = rest.cloned().collect();
    let mut it = flags.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--fps" => fps = num(&mut it, "--fps"),
            "--src" => {
                let v = value(&mut it, "--src");
                let Some((r, p)) = v.split_once('=') else { usage() };
                srcs.push((r.to_string(), p.to_string()));
            }
            _ => usage(),
        }
    }
    let (tl, sources) = load(path, fps, &srcs);
    let out = out.map(std::path::PathBuf::from).unwrap_or_else(|| {
        let tc = tl.fps().map_or_else(|| t.to_string(), |f| edl::timecode(t, f));
        stria::nle_face::frame_path(Some(std::path::Path::new(path)), &tc)
    });
    match stria::nle_face::save_frame(&tl, &sources, t, &out) {
        Ok((w, h)) => println!("nle: frame {t} {w}x{h} -> {}", out.display()),
        Err(e) => die(format!("frame: {e}")),
    }
}
