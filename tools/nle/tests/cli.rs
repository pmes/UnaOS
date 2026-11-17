// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria (CODEX §2 A/V; BP-handlers-14) — the command-line harness of the host tool `tools/nle` (NLECLITESTS,
//! LEDGER SR799): runs the BUILT binary (`CARGO_BIN_EXE_nle`) and pins the surface people type against: the usage, the
//! refusals (every one by name, NLECLIREFUSALSBYNAME SR801), the help roads (stdout, exit 0), a green raw
//! export of SR718's table timeline, the two-pass stderr lines, and every flag arm's word in the usage. Every pinned string
//! is one the tool prints today. Fixtures live under `CARGO_TARGET_TMPDIR` through ONE helper ([`scratch`]), never /tmp.

use std::path::{Path, PathBuf};
use std::process::Command;

use stria::nle::SYNTH_RATE;
use stria::nle_export::{AUDIO_NAMES, VIDEO_NAMES};

/// The first line `usage()` prints (main.rs `usage`), byte-exact.
const USAGE_FIRST: &str = "usage: nle schedule <edl|media> [--fps N] --src REEL=<media> ...";

/// SR718's table timeline (stria tests/common `timeline()`: A[13,23)@0, B[0,10)@10, gap 20..25, A[23,33)@25 on V1 and A1)
/// as CMX 3600 text; reels A (ordinals 0..) and B (1000..), 50 frames at 25 fps each.
const EDL: &str = "TITLE: NLECLI
FCM: NON-DROP FRAME
* UNA FPS 25
* UNA SOURCE A 00:00:02:00
* UNA SOURCE B 00:00:02:00

001  A        V     C        00:00:00:13 00:00:00:23 00:00:00:00 00:00:00:10
002  A        A     C        00:00:00:13 00:00:00:23 00:00:00:00 00:00:00:10
003  B        V     C        00:00:00:00 00:00:00:10 00:00:00:10 00:00:00:20
004  B        A     C        00:00:00:00 00:00:00:10 00:00:00:10 00:00:00:20
005  A        V     C        00:00:00:23 00:00:01:08 00:00:01:00 00:00:01:10
006  A        A     C        00:00:00:23 00:00:01:08 00:00:01:00 00:00:01:10
";

/// The timeline's length in ticks (= exported frames) and the synthetic reel's frame size.
const TICKS: usize = 35;
const W: usize = 64;
const H: usize = 48;

/// The flag arms of `main`'s option loop (`"--x" =>`), measured at 96dd422ec. A removed arm reds
/// [`cli_usage_lists_every_flag`]; a new arm is caught by the source scan there (its usage word must exist).
const ARMS: [&str; 22] = [
    "--fps", "--out", "--video", "--audio", "--aac", "--png", "--qp", "--gop", "--bitrate", "--mkv", "--crf", "--scenecut",
    "--min-keyint", "--two-pass", "--pass", "--stats", "--aq", "--hrd", "--qcomp", "--cplxblur", "--qblur", "--src",
];

/// Run the built `nle` with `args`: (exit code, stdout, stderr).
fn nle(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_nle")).args(args).output().expect("the built nle runs");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// THE scratch helper: `CARGO_TARGET_TMPDIR/nle-cli-<pid>/<name>`, removed then created empty (the per-process subdir keeps
/// two worktrees' runs on one shared target dir apart, SR739's shape).
fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("nle-cli-{}", std::process::id())).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &Path) -> String {
    p.to_str().expect("a UTF-8 scratch path").to_string()
}

/// The fixture: reels A and B written by the binary's own `--make-src`, [`EDL`] beside them. Returns (dir, edl path,
/// the `--src` arguments).
fn fixture(name: &str) -> (PathBuf, String, Vec<String>) {
    let dir = scratch(name);
    let (a, b) = (dir.join("a.mp4"), dir.join("b.mp4"));
    for (p, base) in [(&a, "0"), (&b, "1000")] {
        let (code, _, err) = nle(&["--make-src", &s(p), base, "50", "25"]);
        assert_eq!(code, 0, "--make-src {base}: {err}");
    }
    let edl = dir.join("cut.edl");
    std::fs::write(&edl, EDL).unwrap();
    (dir.clone(), s(&edl), vec!["--src".into(), format!("A={}", s(&a)), "--src".into(), format!("B={}", s(&b))])
}

/// `nle export <edl> <srcs> --out <out> <extra>`.
fn export(edl: &str, srcs: &[String], out: &Path, extra: &[&str]) -> (i32, String, String) {
    let o = s(out);
    let mut args: Vec<&str> = vec!["export", edl];
    args.extend(srcs.iter().map(String::as_str));
    args.extend(["--out", o.as_str()]);
    args.extend_from_slice(extra);
    nle(&args)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// What `nle` with no verb prints on stderr: the usage.
fn usage_text() -> String {
    nle(&[]).2
}

/// `w` appears in `text` as a whole word (not inside a longer flag: `--pass` is not found in `--pass2`).
fn has_word(text: &str, w: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '-';
    text.match_indices(w).any(|(i, _)| {
        let before = text[..i].chars().next_back().is_none_or(|c| !word(c));
        let after = text[i + w.len()..].chars().next().is_none_or(|c| !word(c));
        before && after
    })
}

#[test]
fn cli_no_verb_prints_usage() {
    let roads: [&[&str]; 3] = [&[], &["bogus"], &["export"]]; // NLECLIREFUSALSBYNAME (SR801): the help roads moved to cli_help_exits_zero_on_stdout
    for road in roads {
        let (code, out, err) = nle(road);
        assert_eq!(code, 2, "{road:?}: usage exits 2");
        assert_eq!(out, "", "{road:?}: nothing on stdout");
        let lines: Vec<&str> = err.lines().collect();
        assert_eq!(lines.len(), 6, "{road:?}: six usage lines: {err}"); // NLECLIPASS2VALUE (SR821): + `nle completions`
        assert_eq!(lines[0], USAGE_FIRST, "{road:?}");
        for (l, head) in lines[1..].iter().zip(["       nle render ", "       nle export ", "       nle --make-src ", "       nle size ", "       nle completions "]) {
            assert!(l.starts_with(head), "{road:?}: {l:?} starts {head:?}");
        }
    }
}

#[test]
fn cli_export_refuses_by_name() {
    let (dir, edl, srcs) = fixture("refuse");
    let stats = s(&dir.join("pass.stats"));
    let st = stats.as_str();
    // Refused BY NAME: `nle: <msg>` on stderr, exit 1 (NLECLIREFUSALSBYNAME SR801: every refusal; the table words from stria's).
    let video_want = format!("nle: --video bogus: one of {}", VIDEO_NAMES.join("|"));
    let audio_want = format!("nle: --audio bogus: one of {}|aac:KBPS", AUDIO_NAMES.join("|"));
    let named: [(&[&str], &str); 24] = [
        (&["--qblur", "17"], "nle: --qblur 17: not a number within 0..=16"),
        (&["--qblur", "x"], "nle: --qblur x: not a number within 0..=16"),
        (&["--qcomp", "2"], "nle: --qcomp 2: not a fraction within 0..=1"),
        (&["--bitrate", "100", "--crf", "20"], "nle: --bitrate and --crf are exclusive"),
        (&["--two-pass"], "nle: --two-pass needs --bitrate"),
        (&["--two-pass", "--pass", "1"], "nle: --two-pass and --pass are exclusive"),
        (&["--pass", "3", "--stats", st, "--bitrate", "40", "--video", "h264"], "nle: --pass takes 1 or 2"),
        (&["--pass", "1", "--stats", st], "nle: --pass needs --bitrate"),
        (&["--pass", "1", "--stats", st, "--bitrate", "40"], "nle: --pass needs --video h264"),
        (&["--stats", st], "nle: --stats needs --pass 1|2"),
        (&["--pass", "1", "--bitrate", "40", "--video", "h264"], "nle: --pass needs --stats <file>"),
        (&["--video", "bogus"], video_want.as_str()),
        (&["--audio", "bogus"], audio_want.as_str()),
        (&["--qp", "x"], "nle: --qp x: not a whole number within 0..=255"),
        (&["--min-keyint", "x"], "nle: --min-keyint x: not a whole number within 0..=4294967295"),
        (&["--cplxblur", "x"], "nle: --cplxblur x: not a whole number within 0..=4294967295"),
        (&["--bogus"], "nle: --bogus: unknown flag (nle export --help lists them)"),
        (&["--src", "AX"], "nle: --src AX: REEL=<media> expected"),
        (&["--pass", "x"], "nle: --pass x: not 1 or 2"),
        (&["--out"], "nle: --out: a value is required"),
        (&["--qpstep"], "nle: --qpstep: a value is required"), // NLECLIREFUSALSREST (SR814): SR773's arm through `value`
        (&["--qpstep", "x"], "nle: --qpstep x: not an integer within 0..=51"),
        (&["--pass2"], "nle: --pass2: a value is required"), // NLECLIPASS2VALUE (SR821): SR790's arm through `value`
        (&["--pass2", "qpstep=52"], "nle: --pass2 qpstep 52: not an integer within 0..=51"), // NLECLIPASS2VALUE (SR821): pass2_cli_of's own text by name
    ];
    for (i, (extra, want)) in named.iter().enumerate() {
        let out = dir.join(format!("named{i}"));
        let (code, stdout, err) = export(&edl, &srcs, &out, extra);
        assert_eq!((code, err.as_str(), stdout.as_str()), (1, format!("{want}\n").as_str(), ""), "{extra:?}");
        assert!(!out.exists(), "{extra:?}: refused before the output dir is made");
    }
    // A reel the EDL cites with no --src: refused by name.
    let (code, _, err) = export(&edl, &srcs[..2], &dir.join("nosrc"), &[]);
    assert_eq!((code, err.as_str()), (1, "nle: no --src for reel B\n"));
}

#[test]
fn cli_export_raw_writes_the_frames() {
    let (dir, edl, srcs) = fixture("raw");
    let out = dir.join("out");
    let (code, stdout, err) = export(&edl, &srcs, &out, &["--video", "raw", "--fps", "25"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(err, "", "the raw arm prints nothing on stderr");
    let rgba = std::fs::read(out.join("video.rgba")).unwrap();
    assert_eq!(rgba.len(), TICKS * W * H * 4);
    let json = std::fs::read_to_string(out.join("video.json")).unwrap();
    assert_eq!(json, "{\"format\":\"rgba\",\"fourcc\":\"raw \",\"w\":64,\"h\":48,\"fps_num\":25,\"fps_den\":1,\"frames\":35}\n");
    let pcm_frames = TICKS * (SYNTH_RATE / 25) as usize;
    let wav = std::fs::read(out.join("audio.wav")).unwrap();
    assert_eq!(wav.len(), 44 + pcm_frames * 2, "mono PCM16 at SYNTH_RATE");
    assert_eq!((&wav[0..4], &wav[8..12]), (&b"RIFF"[..], &b"WAVE"[..]));
    let bytes = rgba.len() + json.len() + wav.len();
    let want = format!(
        "{{\"codec\":\"raw\",\"frames\":{TICKS},\"files\":3,\"bytes\":{bytes},\"fnv\":\"{:016x}\",\"pcm_frames\":{pcm_frames}}}\n",
        fnv1a(&rgba)
    );
    assert_eq!(stdout, want, "the one JSON line; its fnv is video.rgba's");
}

#[test]
fn cli_export_two_pass_plan_words() {
    let (dir, edl, srcs) = fixture("twopass");
    let out = dir.join("out");
    let (code, _, err) = export(&edl, &srcs, &out, &["--video", "h264", "--audio", "none", "--bitrate", "40", "--two-pass", "--qblur", "2"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.lines().any(|l| l.starts_with("nle: two-pass pass1 pictures=35 bytes=")), "the first pass's line: {err}");
    assert!(std::fs::metadata(out.join("video.mp4")).unwrap().len() > 0, "video.mp4 written");
    assert!(!out.join("pass1").exists(), "the first pass's dir is removed");
    if has_word(&usage_text(), "--pass2") {
        // H264ENCTWOPASSCLITABLE (SR790): the planned pass's four knobs on stderr.
        assert!(err.lines().any(|l| l == "nle: two-pass plan qcomp=0.6 cplxblur=20 qblur=2 qpstep=4"), "the plan line: {err}");
    } else {
        assert!(!err.contains("nle: two-pass plan"), "{err}");
        eprintln!("cli_export_two_pass_plan_words: SR790 (--pass2) not on this tree: plan line skipped by name");
    }
}

#[test]
fn cli_usage_lists_every_flag() {
    let usage = usage_text();
    // The arms in main.rs's option loop: every `"--x" =>` literal.
    let src = include_str!("../src/main.rs");
    let mut scanned: Vec<&str> = Vec::new();
    for (i, _) in src.match_indices("\"--") {
        let rest = &src[i + 1..];
        let Some(end) = rest.find('"') else { continue };
        if rest[end..].starts_with("\" =>") && !scanned.contains(&&rest[..end]) {
            scanned.push(&rest[..end]);
        }
    }
    for a in ARMS {
        assert!(scanned.contains(&a), "{a}: an arm of main's option loop");
    }
    for a in &scanned {
        assert!(has_word(&usage, a), "{a}: an arm with no word in the usage");
    }
    let flags: Vec<&str> = src.split_once("const FLAGS: ").and_then(|(_, r)| r.split_once("];")).map(|(b, _)| b.split('"').skip(1).step_by(2).collect()).unwrap_or_default(); let (mut f, mut sc) = (flags.clone(), scanned.clone()); f.sort(); sc.sort(); assert_eq!(f, sc, "NLECLIDIDYOUMEAN (SR816): main.rs FLAGS is the option loop's arms"); // NLECLIDIDYOUMEAN (SR816): the did-you-mean list cannot drift from the arms
    for w in ["--make-src", "nle schedule", "nle render", "nle export", "nle size"] {
        assert!(usage.contains(w), "{w} in the usage");
    }
    let line3 = usage.lines().nth(2).unwrap();
    assert!(line3.contains(&format!("[--video {}]", VIDEO_NAMES.join("|"))), "the video table's words: {line3}");
    assert!(line3.contains(&format!("[--audio {}|aac:KBPS]", AUDIO_NAMES.join("|"))), "the audio table's words: {line3}");
}

// ---- NLECLIREFUSALSBYNAME (LEDGER SR801): NLE-CLI-HELP, the usage asked for (named tail block) ----
#[test]
fn cli_help_exits_zero_on_stdout() {
    let usage = usage_text();
    assert_eq!(usage.lines().next(), Some(USAGE_FIRST), "the stderr usage of `nle`");
    let roads: [&[&str]; 3] = [&["help"], &["--help"], &["export", "--help"]];
    for road in roads {
        let (code, out, err) = nle(road);
        assert_eq!(code, 0, "{road:?}: a person asked for it, exit 0");
        assert_eq!(err, "", "{road:?}: nothing on stderr");
        assert_eq!(out.lines().count(), 6, "{road:?}: six usage lines: {out}"); // NLECLIPASS2VALUE (SR821): + `nle completions`
        assert_eq!(out.lines().next(), Some(USAGE_FIRST), "{road:?}");
        assert_eq!(out, usage, "{road:?}: stdout carries the same usage `nle` prints on stderr");
    }
}

// ---- NLECLIREFUSALSREST (LEDGER SR814): the positional numbers and the missing --out by name (named tail block) ----
#[test]
fn cli_positional_refusals() {
    let (dir, edl, srcs) = fixture("positional");
    // `--make-src`'s numbers: a bad one is refused by name, exit 1, and nothing is written.
    let made = dir.join("made.mp4");
    let m = s(&made);
    let bad_nums: [(&[&str], &str); 2] = [
        (&["--make-src", &m, "x", "50", "25"], "nle: --make-src x: not a whole number within 0..=4294967295"),
        (&["--make-src", &m, "0", "y", "25"], "nle: --make-src y: not a whole number within 0..=4294967295"),
    ];
    for (args, want) in bad_nums {
        let (code, out, err) = nle(args);
        assert_eq!((code, err.as_str(), out.as_str()), (1, format!("{want}\n").as_str(), ""), "{args:?}");
        assert!(!made.exists(), "{args:?}: refused before the file is written");
    }
    // `size --fps`: refused by name before the EDL is read or written.
    let before = std::fs::read(&edl).unwrap();
    let (code, out, err) = nle(&["size", &edl, "A", "fill", "--fps", "x"]);
    assert_eq!((code, err.as_str(), out.as_str()), (1, "nle: --fps x: not a whole number within 0..=4294967295\n", ""));
    assert_eq!(std::fs::read(&edl).unwrap(), before, "the EDL is untouched");
    // The missing `--out` of render / export: named by verb with the usage's placeholder.
    for (verb, want) in [("render", "nle: render: --out <stem> is required"), ("export", "nle: export: --out <dir> is required")] {
        let mut args: Vec<&str> = vec![verb, edl.as_str()];
        args.extend(srcs.iter().map(String::as_str));
        let (code, out, err) = nle(&args);
        assert_eq!((code, err.as_str(), out.as_str()), (1, format!("{want}\n").as_str(), ""), "{verb}");
    }
}

// ---- NLECLIDIDYOUMEAN (LEDGER SR816): an unknown flag or verb names the nearest within 2 (named tail block) ----
#[test]
fn cli_unknown_word_did_you_mean() {
    let (dir, edl, srcs) = fixture("didyoumean");
    let near: [(&[&str], &str); 3] = [
        (&["--qbulr", "2"], "nle: --qbulr: unknown flag; did you mean --qblur?"),
        (&["--qp5"], "nle: --qp5: unknown flag; did you mean --qp?"),
        (&["--zzzzzz"], "nle: --zzzzzz: unknown flag (nle export --help lists them)"), // none within 2: SR801's text unchanged
    ];
    for (i, (extra, want)) in near.iter().enumerate() {
        let out = dir.join(format!("near{i}"));
        let (code, stdout, err) = export(&edl, &srcs, &out, extra);
        assert_eq!((code, err.as_str(), stdout.as_str()), (1, format!("{want}\n").as_str(), ""), "{extra:?}");
        assert!(!out.exists(), "{extra:?}: refused before the output dir is made");
    }
    // An unknown verb near one: the hint, then the usage, exit 2 — before the EDL is read.
    let out = dir.join("verb");
    let o = s(&out);
    let mut full: Vec<&str> = vec!["exprot", edl.as_str()];
    full.extend(srcs.iter().map(String::as_str));
    full.extend(["--out", o.as_str()]);
    let roads: [&[&str]; 2] = [&["exprot"], &full];
    for road in roads {
        let (code, stdout, err) = nle(road);
        assert_eq!((code, stdout.as_str()), (2, ""), "{road:?}");
        let lines: Vec<&str> = err.lines().collect();
        assert_eq!(lines.len(), 7, "{road:?}: the hint and six usage lines: {err}"); // NLECLIPASS2VALUE (SR821): + `nle completions`
        assert_eq!(lines[0], "nle: exprot: unknown verb; did you mean export?", "{road:?}");
        assert_eq!(lines[1], USAGE_FIRST, "{road:?}");
        assert!(!out.exists(), "{road:?}: nothing written");
    }
}

// ---- NLECLIPASS2VALUE (LEDGER SR821): `nle completions bash|zsh|fish` from main.rs FLAGS / VERBS (named tail block) ----
/// The words of main.rs `const <name>: [...]` (FLAGS / VERBS), read from the source as SR816's FLAGS pin reads them.
fn const_words(name: &str) -> Vec<&'static str> {
    let src: &'static str = include_str!("../src/main.rs");
    src.split_once(&format!("const {name}: ")).and_then(|(_, r)| r.split_once("];")).map(|(b, _)| b.split('"').skip(1).step_by(2).collect()).unwrap_or_default()
}

#[test]
fn cli_completions_scripts() {
    let (flags, verbs) = (const_words("FLAGS"), const_words("VERBS"));
    assert!(!flags.is_empty() && !verbs.is_empty(), "FLAGS / VERBS read from main.rs");
    assert!(verbs.contains(&"completions"), "the verb is in VERBS: {verbs:?}");
    let words: Vec<&str> = verbs.iter().chain(flags.iter()).copied().collect();
    let script = |shell: &str| {
        let (code, out, err) = nle(&["completions", shell]);
        assert_eq!((code, err.as_str()), (0, ""), "completions {shell}");
        out
    };
    // bash: the `_nle` fn naming every word, then ONE `complete -F` line (NLECLICOMPLETIONSVALUES SR847).
    let bash = script("bash");
    assert!(bash.starts_with("_nle() {\n"), "{bash}"); /* SR847 */
    assert!(bash.ends_with("\ncomplete -o default -F _nle nle\n"), "{bash}"); /* SR847 */
    // zsh: `#compdef nle` first, every word in the script.
    let zsh = script("zsh");
    assert_eq!(zsh.lines().next(), Some("#compdef nle"), "{zsh}");
    for (shell, text) in [("bash", &bash), ("zsh", &zsh)] {
        for w in &words {
            assert!(has_word(text, w), "{shell}: {w} in {text}");
        }
    }
    // fish: one line per word, a `--name` word as `-l name`, a verb as `-a` at the first word.
    let fish = script("fish");
    assert_eq!(fish.lines().count(), words.len() + 1, "{fish}"); /* SR847 */ // + the size policy line
    for w in &words {
        let line = match w.strip_prefix("--") {
            Some(l) => format!("complete -c nle -l {l}"),
            None => format!("complete -c nle -n __fish_use_subcommand -a {w}"),
        };
        assert!(fish.lines().any(|x| x == line || x.starts_with(&format!("{line} -xa ")) || x == format!("{line} -x")), "fish: {line:?} in {fish}"); /* SR847 */ /* SR1005 */
    }
    // Refusals by SR801's grammar, exit 1, nothing on stdout.
    let refused: [(&[&str], &str); 2] = [
        (&["completions", "tcsh"], "nle: completions tcsh: one of bash|zsh|fish\n"),
        (&["completions"], "nle: completions: a value is required\n"),
    ];
    for (args, want) in refused {
        let (code, out, err) = nle(args);
        assert_eq!((code, err.as_str(), out.as_str()), (1, want, ""), "{args:?}");
    }
    // A misspelt verb: SR816's hint names it, then the usage, exit 2.
    let (code, out, err) = nle(&["completoins", "bash"]);
    assert_eq!((code, out.as_str()), (2, ""));
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines.len(), 7, "the hint and six usage lines: {err}");
    assert_eq!(lines[0], "nle: completoins: unknown verb; did you mean completions?");
    assert_eq!(lines[1], USAGE_FIRST);
    assert_eq!(lines[6], "       nle completions bash|zsh|fish");
}

// ---- NLECLICOMPLETIONSVALUES (LEDGER SR847): the words a flag's value takes in every completion script (named tail block) ----
/// NLECLICOMPLETIONSVALUES (SR847): the value table built from the OWNERS' tables (stria's `VIDEO_NAMES` / `AUDIO_NAMES` +
/// `aac:` / `PASS2_KEYS` as `k=`), never from literals: a new word in a table must reach every script.
fn completion_values() -> Vec<(&'static str, String)> {
    let audio: Vec<String> = AUDIO_NAMES.iter().map(|w| w.to_string()).chain(["aac:".to_string()]).collect();
    vec![
        ("--video", VIDEO_NAMES.join(" ")),
        ("--audio", audio.join(" ")),
        ("--pass2", stria::nle_export::PASS2_KEYS.iter().map(|k| format!("{k}=")).collect::<Vec<_>>().join(" ")),
    ]
}

#[test]
fn cli_completions_values() {
    let flags = const_words("FLAGS");
    let table = completion_values();
    let size = timeline_core::SizePolicy::ALL.map(timeline_core::SizePolicy::name).join(" ");
    let script = |shell: &str| {
        let (code, out, err) = nle(&["completions", shell]);
        assert_eq!((code, err.as_str()), (0, ""), "completions {shell}");
        out
    };
    let (bash, zsh, fish) = (script("bash"), script("zsh"), script("fish"));
    for (f, words) in &table {
        assert!(flags.contains(f), "{f}: a valued flag is in main.rs FLAGS {flags:?}");
        let l = f.trim_start_matches('-');
        assert!(bash.contains(&format!("        {f}) w=\"{words}\" ;;\n")), "bash: {f} -> {words} in {bash}");
        assert!(zsh.contains(&format!(" '*{f}:{l}:({words})'")), "zsh: {f} -> {words} in {zsh}");
        let fl = format!("complete -c nle -l {l} -xa \"{words}\"");
        assert!(fish.lines().any(|x| x == fl), "fish: {fl:?} in {fish}");
    }
    // Exactly the table's flags carry values (no word list the owners' tables do not hold).
    assert_eq!(bash.matches(") w=\"").count() - bash.matches(") w=\"\" ;;").count(), table.len(), "{bash}"); /* SR1005 */
    assert_eq!(zsh.matches(":(").count(), table.len() + 1, "{zsh}"); // + SR821's `1:verb:(..)`
    assert_eq!(fish.lines().filter(|x| x.contains(" -xa ")).count(), table.len() + 1, "{fish}");
    // The size policies, timeline_core's `SizePolicy::ALL`, at `nle size <edl> <reel> _` in each shell.
    assert!(bash.contains(&format!("[[ ${{COMP_WORDS[1]}} == size && $COMP_CWORD == 4 ]] && w=\"{size}\"\n")), "{bash}");
    assert!(zsh.contains(&format!(" '4:policy:{{if [[ $words[2] == size ]]; then compadd -- {size}; else _files; fi}}'")), "{zsh}");
    assert!(fish.contains(&format!("__fish_seen_subcommand_from size; and test (count (commandline -opc)) -eq 4\" -xa \"{size}\"\n")), "{fish}");
    // Each script parses in its own shell (`-n`: read, not run); zsh / fish skipped by name when the binary is absent.
    let dir = scratch("completions");
    for (shell, text) in [("bash", &bash), ("zsh", &zsh), ("fish", &fish)] {
        let p = dir.join(format!("nle.{shell}"));
        std::fs::write(&p, text).unwrap();
        match Command::new(shell).arg("-n").arg(&p).output() {
            Ok(o) => assert!(o.status.success(), "{shell} -n: {}\n{text}", String::from_utf8_lossy(&o.stderr)),
            Err(e) if shell != "bash" => eprintln!("cli_completions_values: {shell} -n skipped: {shell} absent ({e})"),
            Err(e) => panic!("bash -n: {e}"),
        }
    }
}

// ---- NLECLICOMPLETIONSFREE (LEDGER SR1005): FLAG_ARG pinned to the arms' reads, and what each shell offers after a flag (named tail block) ----
/// NLECLICOMPLETIONSFREE (SR1005): main.rs `const FLAG_ARG: [Arg; N] = [..]`'s kinds, read from the source (`Arg::` stripped).
fn flag_arg_src() -> Vec<&'static str> {
    let src: &'static str = include_str!("../src/main.rs");
    src.split_once("const FLAG_ARG: ")
        .and_then(|(_, r)| r.split_once("= ["))
        .and_then(|(_, r)| r.split_once("];"))
        .map(|(b, _)| b.split(',').map(str::trim).filter(|t| !t.is_empty()).map(|t| t.trim_start_matches("Arg::")).collect())
        .unwrap_or_default()
}

/// NLECLICOMPLETIONSFREE (SR1005): each [`const_words`]`("FLAGS")` word with what its arm reads — the arm's body is the span
/// from its `"--x" =>` to the next FLAGS word's (the last to `_ => die(unknown_flag`); SR816's pin holds FLAGS == the arms,
/// so this rides SR799's scan rather than walking the source again. `Opt` a peek (`it.clone().next()`), `Required` a
/// `value(&mut it, "--x")` / `num(&mut it, "--x")`, else `Bare`.
fn arm_kinds() -> Vec<(&'static str, &'static str)> {
    let src: &'static str = include_str!("../src/main.rs");
    let flags = const_words("FLAGS");
    let at: Vec<usize> = flags.iter().map(|f| src.find(&format!("\"{f}\" =>")).unwrap_or_else(|| panic!("{f}: an arm of the option loop"))).collect();
    let end = src[at[at.len() - 1]..].find("_ => die(unknown_flag").map(|e| at[at.len() - 1] + e).expect("the loop's unknown-flag arm");
    let mut kinds = Vec::new();
    for (i, f) in flags.iter().enumerate() {
        let stop = at.get(i + 1).copied().unwrap_or(end);
        assert!(at[i] < stop, "{f}: FLAGS is in arm order");
        let body = &src[at[i]..stop];
        let kind = if body.contains("it.clone().next()") {
            "Opt"
        } else if body.contains(&format!("value(&mut it, \"{f}\")")) || body.contains(&format!("num(&mut it, \"{f}\")")) {
            "Required"
        } else {
            "Bare"
        };
        kinds.push((*f, kind));
    }
    kinds
}

#[test]
fn cli_completions_free() {
    let kinds = arm_kinds();
    let arg = flag_arg_src();
    assert_eq!(arg.len(), kinds.len(), "FLAG_ARG {arg:?} parallel to FLAGS");
    let valued: Vec<&str> = completion_values().iter().map(|(f, _)| *f).collect();
    // FLAG_ARG is exactly what each arm reads: a required value is Words iff SR847's owners' table holds it, else Free.
    for ((f, kind), a) in kinds.iter().zip(&arg) {
        let want = match *kind {
            "Required" if valued.contains(f) => "Words",
            "Required" => "Free",
            k => k,
        };
        assert_eq!(*a, want, "{f}: FLAG_ARG vs its arm ({kind})");
    }
    let of = |k: &str| kinds.iter().zip(&arg).filter(|(_, a)| **a == k).map(|((f, _), _)| *f).collect::<Vec<&str>>();
    let (free, opt, bare) = (of("Free"), of("Opt"), of("Bare"));
    assert_eq!(
        free,
        ["--fps", "--out", "--qp", "--gop", "--bitrate", "--crf", "--min-keyint", "--pass", "--stats", "--qcomp", "--cplxblur", "--qblur", "--qpstep", "--src"],
        "the free-value flags"
    );
    assert_eq!(opt, ["--scenecut", "--aq"], "the optional-K flags");
    let script = |shell: &str| {
        let (code, out, err) = nle(&["completions", shell]);
        assert_eq!((code, err.as_str()), (0, ""), "completions {shell}");
        out
    };
    let (bash, zsh, fish) = (script("bash"), script("zsh"), script("fish"));
    // bash: ONE empty arm naming every Free flag; no arm for an Opt or Bare flag (they fall through to the verb / flag words).
    assert_eq!(bash.matches(") w=\"\" ;;").count(), 1, "{bash}");
    assert!(bash.contains(&format!("        {}) w=\"\" ;;\n", free.join("|"))), "bash: the Free arm in {bash}");
    for f in opt.iter().chain(&bare) {
        assert!(!bash.contains(&format!("{f})")) && !bash.contains(&format!("{f}|")), "bash: no arm for {f} in {bash}");
    }
    // zsh: Free `:<name>: ` (message, no matches), Opt `::<name>: ` (optional), Bare a plain spec.
    for f in &free {
        let l = f.trim_start_matches('-');
        assert!(zsh.contains(&format!(" '*{f}:{l}: '")), "zsh: {f} free in {zsh}");
    }
    for f in &opt {
        let l = f.trim_start_matches('-');
        assert!(zsh.contains(&format!(" '*{f}::{l}: '")), "zsh: {f} optional in {zsh}");
    }
    for f in &bare {
        assert!(zsh.contains(&format!(" '*{f}' ")), "zsh: {f} bare in {zsh}");
    }
    // fish: Free `-x` (a value required, no files, no words); Opt and Bare a plain `-l` line.
    for f in &free {
        let line = format!("complete -c nle -l {} -x", f.trim_start_matches('-'));
        assert!(fish.lines().any(|x| x == line), "fish: {line:?} in {fish}");
    }
    for f in opt.iter().chain(&bare) {
        let line = format!("complete -c nle -l {}", f.trim_start_matches('-'));
        assert!(fish.lines().any(|x| x == line), "fish: {line:?} in {fish}");
    }
}

// ---- NLECLIUSAGEVERBSPIN (LEDGER SR1032): the usage's table words and VERBS pinned to main's dispatch (named tail block) ----
/// NLECLIUSAGEVERBSPIN (SR1032): the words of `line`'s `[<flag> a|b|c]` group, split on `|`; empty when the group is absent.
fn usage_group<'a>(line: &'a str, flag: &str) -> Vec<&'a str> {
    let open = format!("[{flag} ");
    line.split_once(open.as_str()).and_then(|(_, r)| r.split_once(']')).map(|(g, _)| g.split('|').collect()).unwrap_or_default()
}

/// NLECLIUSAGEVERBSPIN (SR1032, NLE5D-CLI-USAGE-TEST): `nle help`'s `--video` / `--audio` groups are exactly stria's tables (the
/// owners' consts, imported: main.rs holds no copy for SR821's `const_words` to read), so a table change reaches the usage.
#[test]
fn cli_usage_names_the_tables() {
    let (code, out, err) = nle(&["help"]);
    assert_eq!((code, err.as_str()), (0, ""), "nle help");
    let audio: Vec<&str> = AUDIO_NAMES.iter().copied().chain(["aac:KBPS"]).collect();
    for (flag, want) in [("--video", VIDEO_NAMES.to_vec()), ("--audio", audio)] {
        let open = format!("[{flag} ");
        let lines: Vec<&str> = out.lines().filter(|l| l.contains(&open)).collect();
        assert_eq!(lines.len(), 1, "{flag}: one usage line carries the group: {out}");
        assert_eq!(lines[0].matches(&open).count(), 1, "{flag}: the group once: {}", lines[0]);
        let group = usage_group(lines[0], flag);
        assert_eq!(group, want, "{flag}: the usage's words are the table's, in order, no extra and none missing");
        let mut d = group.clone();
        d.sort();
        d.dedup();
        assert_eq!(d.len(), group.len(), "{flag}: no word twice: {group:?}");
    }
    assert_eq!(out, usage_text(), "the help road and the stderr usage are one text");
}

/// NLECLIUSAGEVERBSPIN (SR1032, NLE-CLI-VERBS-PIN): the first words `main` dispatches on, read from main.rs in its three shapes —
/// every `args.first().map(String::as_str) == Some("<w>")`, the first slot of each slice pattern in `help_asked`'s `matches!`,
/// and every arm word of `match verb.as_str() {` (8-space indent, up to its `_ => usage(),`). Sorted, deduped.
fn dispatch_verbs() -> Vec<&'static str> {
    let src: &'static str = include_str!("../src/main.rs");
    let unq = |t: &'static str| t.trim().trim_matches('"');
    let mut verbs: Vec<&'static str> = Vec::new();
    let first = "args.first().map(String::as_str) == Some(\"";
    for (i, _) in src.match_indices(first) {
        let rest = &src[i + first.len()..];
        if let Some(e) = rest.find('"') {
            verbs.push(&rest[..e]);
        }
    }
    let help = src.split_once("fn help_asked(").and_then(|(_, r)| r.split_once("matches!(")).and_then(|(_, r)| r.split_once(")\n")).map(|(b, _)| b).expect("help_asked's matches!");
    for pat in help.split('[').skip(1) {
        let slot = pat.split(',').next().unwrap_or("");
        verbs.extend(slot.split('|').map(unq));
    }
    let arms = src.split_once("match verb.as_str() {").and_then(|(_, r)| r.split_once("_ => usage(),")).map(|(b, _)| b).expect("main's `match verb.as_str()`");
    for line in arms.lines() {
        if line.starts_with("        \"") {
            if let Some((words, _)) = line.split_once("=>") {
                verbs.extend(words.split('|').map(unq));
            }
        }
    }
    verbs.sort();
    verbs.dedup();
    verbs
}

/// NLECLIUSAGEVERBSPIN (SR1032, NLE-CLI-VERBS-PIN): main.rs `VERBS` == the dispatch's first words, both ways (a new verb without
/// its VERBS word reds, so SR821's completion scripts, built from VERBS, cannot miss one); each verb in the bash script; each
/// bare `nle <verb>` pinned as measured at 976d8ffbf.
#[test]
fn cli_verbs_pinned_to_dispatch() {
    let scanned = dispatch_verbs();
    for w in ["schedule", "render", "export", "size", "--make-src", "completions", "help", "--help"] {
        assert!(scanned.contains(&w), "{w}: the scan finds the measured verb: {scanned:?}");
    }
    let verbs = const_words("VERBS");
    assert!(!verbs.is_empty(), "VERBS read from main.rs");
    let mut sorted = verbs.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), verbs.len(), "no word twice in VERBS: {verbs:?}");
    assert_eq!(sorted, scanned, "main.rs VERBS is main's dispatch words");
    let (code, bash, err) = nle(&["completions", "bash"]);
    assert_eq!((code, err.as_str()), (0, ""), "completions bash");
    let usage = usage_text();
    for v in &verbs {
        assert!(has_word(&bash, v), "{v}: in the bash completion script");
        let (code, out, err) = nle(&[*v]);
        let want: (i32, &str, &str) = match *v {
            "schedule" | "render" | "export" | "size" | "--make-src" => (2, "", usage.as_str()),
            "completions" => (1, "", "nle: completions: a value is required\n"),
            "help" | "--help" => (0, usage.as_str(), ""),
            _ => panic!("{v}: a VERBS word with no bare-call row here: measure `nle {v}` and add it"),
        };
        assert_eq!((code, out.as_str(), err.as_str()), want, "nle {v}");
    }
}
