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
    let flags: Vec<&str> = src.split_once("const FLAGS: ").and_then(|(_, r)| r.split_once("];")).map(|(b, _)| b.split('"').skip(1).step_by(2).collect()).unwrap_or_default(); let (mut f, mut sc) = (flags.clone(), scanned.clone()); f.sort(); sc.sort(); sc.retain(|w| !const_words("COMPLETIONS_FLAGS").contains(w)); /* SR1011 */ assert_eq!(f, sc, "NLECLIDIDYOUMEAN (SR816): main.rs FLAGS is the option loop's arms"); // NLECLIDIDYOUMEAN (SR816): the did-you-mean list cannot drift from the arms
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
    let roads: [&[&str]; 2] = [&["help"], &["--help"]]; // NLECLIHELPVERB (SR1041): `export --help` is the export section now (cli_help_verb_sections)
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
    assert_eq!(fish.lines().count(), words.len() + 5, "{fish}"); /* SR847 */ /* SR1019 */ /* SR1025 */ /* SR1033 */ // + the size policy line + the --install / --uninstall dir lines + the shells line + --force
    for w in &words {
        let line = match w.strip_prefix("--") {
            Some(l) => format!("complete -c nle{} -l {l}", fish_n(w)), /* SR1033 */
            None => format!("complete -c nle -n __fish_use_subcommand -a {w}"),
        };
        assert!(fish.lines().any(|x| x == line || x.starts_with(&format!("{line} -xa ")) || x == format!("{line} -x") || x == format!("{line} -r -F")), "fish: {line:?} in {fish}"); /* SR847 */ /* SR1005 */ /* SR1019 */
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
    assert_eq!(lines[6], "       nle completions bash|zsh|fish [--install [<dir>] | --uninstall [<dir>]] [--force]"); /* SR1011 */ /* SR1025 */
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
        ("--pass", const_words("PASS_WORDS").join(" ")), /* SR1019 */ // main.rs PASS_WORDS, two_pass_of's own table
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
        let fl = format!("complete -c nle{} -l {l} -xa \"{words}\"", fish_n(f)); /* SR1033 */
        assert!(fish.lines().any(|x| x == fl), "fish: {fl:?} in {fish}");
    }
    // Exactly the table's flags carry values (no word list the owners' tables do not hold).
    assert_eq!(bash.matches(") w=\"").count() - bash.matches(") w=\"\" ;;").count(), table.len(), "{bash}"); /* SR1005 */
    assert_eq!(zsh.matches(":(").count(), table.len() + 1, "{zsh}"); // + SR821's `1:verb:(..)`
    assert_eq!(fish.lines().filter(|x| x.contains(" -xa ")).count(), table.len() + 2, "{fish}"); /* SR1033 */ // + the size line + the shells line
    // The size policies, timeline_core's `SizePolicy::ALL`, at `nle size <edl> <reel> _` in each shell.
    assert!(bash.contains(&format!("[[ ${{COMP_WORDS[1]}} == size && $COMP_CWORD == 4 ]] && w=\"{size}\"\n")), "{bash}");
    assert!(zsh.contains(&format!(" '4:policy:{{if [[ $words[2] == size ]]; then compadd -- {size}; else if [[ $words[CURRENT-1] == (--install|--uninstall) ]]; then _files -/; else _files; fi; fi}}'")), "{zsh}"); /* SR1019 */ /* SR1025 */
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
            "Required" if const_words("PATH_FLAGS").contains(f) => "Path", /* SR1019 */
            "Required" => "Free",
            k => k,
        };
        assert_eq!(*a, want, "{f}: FLAG_ARG vs its arm ({kind})");
    }
    let of = |k: &str| kinds.iter().zip(&arg).filter(|(_, a)| **a == k).map(|((f, _), _)| *f).collect::<Vec<&str>>();
    let (free, opt, bare) = (of("Free"), of("Opt"), of("Bare"));
    assert_eq!(
        free,
        ["--fps", "--qp", "--gop", "--bitrate", "--crf", "--min-keyint", "--qcomp", "--cplxblur", "--qblur", "--qpstep"], /* SR1019 */
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
    assert!(bash.contains(&format!("        {}) compopt +o default; w=\"\" ;;\n", free.join("|"))), "bash: the Free arm in {bash}"); /* SR1019 */
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
        let line = format!("complete -c nle{} -l {} -x", fish_n(f), f.trim_start_matches('-')); /* SR1033 */
        assert!(fish.lines().any(|x| x == line), "fish: {line:?} in {fish}");
    }
    for f in opt.iter().chain(&bare) {
        let line = format!("complete -c nle{} -l {}", fish_n(f), f.trim_start_matches('-')); /* SR1033 */
        assert!(fish.lines().any(|x| x == line), "fish: {line:?} in {fish}");
    }
}

// ---- NLECLICOMPLETIONSINSTALL (LEDGER SR1011): `nle completions <shell> --install [<dir>] [--force]` (named tail block) ----
/// NLECLICOMPLETIONSINSTALL (SR1011): run the built `nle` with HOME / XDG_DATA_HOME / XDG_CONFIG_HOME REMOVED from its env and
/// only the scratch values given set (the real HOME is never read or written), from `CARGO_TARGET_TMPDIR` (a relative
/// path resolves there, never in the tree): (exit code, stdout, stderr).
fn nle_env(args: &[&str], home: Option<&Path>, xdg: &[(&str, &Path)]) -> (i32, String, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_nle"));
    c.args(args).current_dir(env!("CARGO_TARGET_TMPDIR")).env_remove("HOME").env_remove("XDG_DATA_HOME").env_remove("XDG_CONFIG_HOME");
    if let Some(h) = home {
        c.env("HOME", h);
    }
    for (k, v) in xdg {
        c.env(k, v);
    }
    let out = c.output().expect("the built nle runs");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// NLECLICOMPLETIONSINSTALL (SR1011): `nle completions <shell>`'s stdout, the bytes `--install` must write.
fn completion_script(shell: &str) -> String {
    let (code, out, err) = nle(&["completions", shell]);
    assert_eq!((code, err.as_str()), (0, ""), "completions {shell}");
    out
}

/// NLECLICOMPLETIONSINSTALL (SR1011): the zsh fpath line for the file at `path` (empty for another shell).
fn fpath_line(shell: &str, path: &Path) -> String {
    if shell != "zsh" {
        return String::new();
    }
    let d = path.parent().unwrap().display().to_string();
    format!("zsh: add {d} to fpath before compinit, e.g. fpath=({d} $fpath) in ~/.zshrc\n")
}

#[test]
fn cli_completions_install_paths() {
    let dir = scratch("install-paths");
    let home = dir.join("home");
    let want = [("bash", ".local/share/bash-completion/completions/nle"), ("zsh", ".local/share/zsh/site-functions/_nle"), ("fish", ".config/fish/completions/nle.fish")];
    for (shell, rel) in want {
        let path = home.join(rel);
        let script = completion_script(shell);
        let args = ["completions", shell, "--install"];
        let (code, out, err) = nle_env(&args, Some(&home), &[]);
        assert_eq!((code, err.as_str()), (0, ""), "{shell}");
        assert_eq!(out, format!("installed {}\n{}", path.display(), fpath_line(shell, &path)), "{shell}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), script, "{shell}: the file is `nle completions {shell}`'s stdout");
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        // Idempotent: the same bytes are found, not rewritten.
        let (code, out, err) = nle_env(&args, Some(&home), &[]);
        assert_eq!((code, err.as_str()), (0, ""), "{shell} again");
        assert_eq!(out, format!("installed {} (unchanged)\n{}", path.display(), fpath_line(shell, &path)), "{shell} again");
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime, "{shell}: unchanged keeps the mtime");
    }
    // XDG_DATA_HOME / XDG_CONFIG_HOME (absolute) win over HOME.
    let (data, config) = (dir.join("data"), dir.join("config"));
    for (shell, base, rel) in [("bash", &data, "bash-completion/completions/nle"), ("zsh", &data, "zsh/site-functions/_nle"), ("fish", &config, "fish/completions/nle.fish")] {
        let path = base.join(rel);
        let (code, out, err) = nle_env(&["completions", shell, "--install"], Some(&home), &[("XDG_DATA_HOME", data.as_path()), ("XDG_CONFIG_HOME", config.as_path())]);
        assert_eq!((code, err.as_str()), (0, ""), "{shell} xdg");
        assert_eq!(out, format!("installed {}\n{}", path.display(), fpath_line(shell, &path)), "{shell} xdg");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), completion_script(shell), "{shell} xdg");
    }
    // A relative XDG value is ignored (the XDG spec): HOME's path, already installed.
    let (code, out, err) = nle_env(&["completions", "bash", "--install"], Some(&home), &[("XDG_DATA_HOME", Path::new("rel-data"))]);
    assert_eq!((code, err.as_str()), (0, ""), "relative xdg");
    assert_eq!(out, format!("installed {} (unchanged)\n", home.join(want[0].1).display()), "relative xdg");
}

#[test]
fn cli_completions_install_force() {
    let dir = scratch("install-force");
    let home = dir.join("home");
    let path = home.join(".config/fish/completions/nle.fish");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "# mine\n").unwrap();
    // A differing file (a hand-edited script) is refused without --force, untouched.
    let (code, out, err) = nle_env(&["completions", "fish", "--install"], Some(&home), &[]);
    assert_eq!((code, out.as_str(), err), (1, "", format!("nle: exists {} (differs; --force to overwrite)\n", path.display())));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n", "refused: the file is untouched");
    // --force replaces it (`--install --force`: the flag is not taken as a dir).
    let (code, out, err) = nle_env(&["completions", "fish", "--install", "--force"], Some(&home), &[]);
    assert_eq!((code, err.as_str(), out), (0, "", format!("installed {}\n", path.display())));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), completion_script("fish"), "--force: replaced by the script");
}

#[test]
fn cli_completions_install_dir() {
    let dir = scratch("install-dir");
    let target = dir.join("a").join("b");
    let t = s(&target);
    // An explicit dir: created, HOME not needed.
    for (shell, file) in [("bash", "nle"), ("fish", "nle.fish")] {
        let (code, out, err) = nle_env(&["completions", shell, "--install", &t], None, &[]);
        assert_eq!((code, err.as_str(), out), (0, "", format!("installed {}\n", target.join(file).display())), "{shell}");
        assert_eq!(std::fs::read_to_string(target.join(file)).unwrap(), completion_script(shell), "{shell}");
    }
}

#[test]
fn cli_completions_install_refusals() {
    let dir = scratch("install-refuse");
    let home = dir.join("home");
    let refused: [(&[&str], &str); 4] = [
        (&["completions", "nosuch", "--install"], "nle: completions nosuch: one of bash|zsh|fish\n"),
        (&["completions", "bash", "--instal"], "nle: completions bash --instal: unknown flag; did you mean --install?\n"),
        (&["completions", "bash", "--bogus"], "nle: completions bash --bogus: unknown flag (--install [<dir>], --uninstall [<dir>] or --force)\n"), /* SR1025 */
        (&["completions", "bash", "--force"], "nle: completions bash: --force needs --install\n"),
    ];
    for (args, want) in refused {
        let (code, out, err) = nle_env(args, Some(&home), &[]);
        assert_eq!((code, out.as_str(), err.as_str()), (1, "", want), "{args:?}");
    }
    let (code, out, err) = nle_env(&["completions", "zsh", "--install"], None, &[]);
    assert_eq!((code, out.as_str(), err.as_str()), (1, "", "nle: completions zsh --install: HOME is not set (give --install <dir>)\n"));
    assert!(!home.exists(), "refused: nothing written under HOME");
}

#[test]
fn cli_completions_flags_pin() {
    // COMPLETIONS_FLAGS is the completions verb's own arms: each an arm of main.rs, none in FLAGS (the export loop's words;
    // SR816's pin subtracts these by name), each a word of the usage's completions line.
    let src = include_str!("../src/main.rs");
    let (own, flags) = (const_words("COMPLETIONS_FLAGS"), const_words("FLAGS"));
    assert_eq!(own, ["--install", "--force", "--uninstall"], "main.rs COMPLETIONS_FLAGS"); /* SR1025 */
    let usage = usage_text();
    let line = usage.lines().find(|l| l.contains("nle completions ")).expect("the completions usage line");
    for w in &own {
        assert!(src.contains(&format!("\"{w}\" =>")), "{w}: an arm in main.rs");
        assert!(!flags.contains(w), "{w}: not in FLAGS {flags:?}");
        assert!(has_word(line, w), "{w}: in the usage line {line:?}");
    }
}

// ---- NLECLICOMPLETIONSPASSWORDSFILES (LEDGER SR1019): --pass's words from PASS_WORDS; Path / Dir completion in every shell (named tail block) ----
/// NLECLICOMPLETIONSPASSWORDSFILES (SR1019): `nle completions <shell>`'s stdout.
fn script_of(shell: &str) -> String {
    let (code, out, err) = nle(&["completions", shell]);
    assert_eq!((code, err.as_str()), (0, ""), "completions {shell}");
    out
}

#[test]
fn cli_completions_pass_words() {
    let words = const_words("PASS_WORDS");
    assert_eq!(words, ["1", "2"], "main.rs PASS_WORDS");
    let w = words.join(" ");
    let (bash, zsh, fish) = (script_of("bash"), script_of("zsh"), script_of("fish"));
    assert!(bash.contains(&format!("        --pass) w=\"{w}\" ;;\n")), "bash: --pass words in {bash}");
    assert!(zsh.contains(&format!(" '*--pass:pass:({w})'")), "zsh: --pass words in {zsh}");
    let fl = format!("complete -c nle{} -l pass -xa \"{w}\"", fish_n("--pass")); /* SR1033 */
    assert!(fish.lines().any(|x| x == fl), "fish: {fl:?} in {fish}");
    // The refusals name the same words (two_pass_of's and what("--pass")'s texts are built from PASS_WORDS).
    let (dir, edl, srcs) = fixture("pass-words");
    let st = s(&dir.join("pass.stats"));
    let or = words.join(" or ");
    let refused: [(Vec<&str>, String); 2] = [
        (vec!["--pass", "x"], format!("nle: --pass x: not {or}")),
        (vec!["--pass", "3", "--stats", st.as_str(), "--bitrate", "40", "--video", "h264"], format!("nle: --pass takes {or}")),
    ];
    for (i, (extra, want)) in refused.iter().enumerate() {
        let (code, out, err) = export(&edl, &srcs, &dir.join(format!("refused{i}")), extra);
        assert_eq!((code, err, out.as_str()), (1, format!("{want}\n"), ""), "{extra:?}");
    }
    // Each word is a pass two_pass_of takes: past the word check, refused only for the missing --bitrate.
    for p in &words {
        let (code, out, err) = export(&edl, &srcs, &dir.join(format!("accepted{p}")), &["--pass", *p, "--stats", st.as_str()]);
        assert_eq!((code, err.as_str(), out.as_str()), (1, "nle: --pass needs --bitrate\n", ""), "--pass {p}");
    }
}

#[test]
fn cli_completions_files() {
    // PATH_FLAGS is exactly FLAG_ARG's Path words, each a required value by SR1005's arm scan.
    let paths = const_words("PATH_FLAGS");
    assert_eq!(paths, ["--out", "--stats", "--src"], "main.rs PATH_FLAGS");
    let tagged: Vec<&str> = arm_kinds().iter().zip(flag_arg_src()).filter(|(_, a)| *a == "Path").map(|((f, k), _)| {
        assert_eq!(*k, "Required", "{f}: a Path flag reads a required value");
        *f
    }).collect();
    assert_eq!(tagged, paths, "FLAG_ARG's Path words == PATH_FLAGS");
    let (bash, zsh, fish) = (script_of("bash"), script_of("zsh"), script_of("fish"));
    // bash: the numeric Free arm turns readline's filenames off; the Path arm keeps them (-o default); --install directories.
    let free = bash.lines().find(|l| l.ends_with(") compopt +o default; w=\"\" ;;")).expect("bash: the Free arm");
    assert!(free.contains("--fps|") && !paths.iter().any(|p| free.contains(&format!("{p}|")) || free.contains(&format!("{p})"))), "bash: Free arm {free:?}");
    let path_arm = format!("        {}) w=\"\" ;;\n", paths.join("|"));
    assert!(bash.contains(&path_arm), "bash: {path_arm:?} in {bash}");
    assert!(bash.contains("        --install|--uninstall) compopt -o dirnames +o default; w=\"\" ;;\n"), "bash: the --install arm in {bash}"); /* SR1025 */
    assert!(bash.contains("    [[ $COMP_CWORD == 2 && ${COMP_WORDS[1]} != completions && $cur != -* ]] && w=\"\"\n"), "bash: position 2 in {bash}");
    assert!(bash.contains("    [[ $prev == = || $cur == = ]] && w=\"\"\n"), "bash: after REEL= in {bash}");
    assert!(bash.ends_with("\ncomplete -o default -F _nle nle\n"), "{bash}");
    // zsh: the path after the last `=` (else the word) on each Path flag; directories after --install; files at position 2+.
    for p in &paths {
        let l = p.trim_start_matches('-');
        let spec = format!(" '*{p}:{l}:{{compset -P \"*=\"; _files}}'");
        assert!(zsh.contains(&spec), "zsh: {spec:?} in {zsh}");
    }
    let file = "if [[ $words[CURRENT-1] == (--install|--uninstall) ]]; then _files -/; else _files; fi"; /* SR1025 */
    assert!(zsh.contains(&format!(" '*:file:{{{file}}}'\n")), "zsh: the rest spec in {zsh}");
    assert!(zsh.contains(&format!("else {file}; fi}}'")), "zsh: the size spec's else in {zsh}");
    // fish: a Path flag's value is files; --install's directories under the completions verb only.
    for p in &paths {
        let line = format!("complete -c nle{} -l {} -r -F", fish_n(p), p.trim_start_matches('-')); /* SR1033 */
        assert!(fish.lines().any(|x| x == line), "fish: {line:?} in {fish}");
    }
    let install = "complete -c nle -n \"__fish_seen_subcommand_from completions\" -l install -x -a \"(__fish_complete_directories)\"";
    assert_eq!(fish.lines().filter(|x| *x == install).count(), 1, "fish: {install:?} in {fish}");
    // bash parses the script (`-n`: read, not run).
    let dir = scratch("completions-files");
    let p = dir.join("nle.bash");
    std::fs::write(&p, &bash).unwrap();
    let o = Command::new("bash").arg("-n").arg(&p).output().expect("bash runs");
    assert!(o.status.success(), "bash -n: {}\n{bash}", String::from_utf8_lossy(&o.stderr));
}

// ---- NLECLICOMPLETIONSUNINSTALL (LEDGER SR1025): `nle completions <shell> --uninstall [<dir>] [--force]` (named tail block) ----
#[test]
fn cli_completions_uninstall() {
    let dir = scratch("uninstall");
    let home = dir.join("home");
    let want = [("bash", ".local/share/bash-completion/completions/nle"), ("zsh", ".local/share/zsh/site-functions/_nle"), ("fish", ".config/fish/completions/nle.fish")];
    for (shell, rel) in want {
        let path = home.join(rel);
        let (code, _, err) = nle_env(&["completions", shell, "--install"], Some(&home), &[]);
        assert_eq!((code, err.as_str()), (0, ""), "{shell} install");
        assert!(path.exists(), "{shell}: installed at {}", path.display());
        // The SAME path --install wrote: removed, then absent (idempotent, exit 0 both).
        let (code, out, err) = nle_env(&["completions", shell, "--uninstall"], Some(&home), &[]);
        assert_eq!((code, err.as_str(), out), (0, "", format!("removed {}\n", path.display())), "{shell} uninstall");
        assert!(!path.exists(), "{shell}: the file is gone");
        let (code, out, err) = nle_env(&["completions", shell, "--uninstall"], Some(&home), &[]);
        assert_eq!((code, err.as_str(), out), (0, "", format!("removed {} (absent)\n", path.display())), "{shell} again");
    }
    // An explicit dir: --uninstall <dir> removes what --install <dir> wrote, HOME not needed.
    let target = dir.join("a").join("b");
    let t = s(&target);
    let (code, _, err) = nle_env(&["completions", "bash", "--install", &t], None, &[]);
    assert_eq!((code, err.as_str()), (0, ""), "install <dir>");
    let (code, out, err) = nle_env(&["completions", "bash", "--uninstall", &t], None, &[]);
    assert_eq!((code, err.as_str(), out), (0, "", format!("removed {}\n", target.join("nle").display())), "uninstall <dir>");
    assert!(!target.join("nle").exists() && target.exists(), "the file is gone, the dir kept");
    // A file that is not ours (a hand-written _nle) is refused by name and left intact; --force removes it.
    let path = home.join(want[1].1);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "#compdef nle\n# mine\n").unwrap();
    let (code, out, err) = nle_env(&["completions", "zsh", "--uninstall"], Some(&home), &[]);
    assert_eq!((code, out.as_str(), err), (1, "", format!("nle: {}: not nle's completion script (use --force)\n", path.display())));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "#compdef nle\n# mine\n", "refused: the file is intact");
    let (code, out, err) = nle_env(&["completions", "zsh", "--uninstall", "--force"], Some(&home), &[]);
    assert_eq!((code, err.as_str(), out), (0, "", format!("removed {}\n", path.display())), "--force");
    assert!(!path.exists(), "--force: removed");
    // --install with --uninstall is refused by name, nothing written.
    let other = dir.join("other");
    let (code, out, err) = nle_env(&["completions", "fish", "--install", "--uninstall"], Some(&other), &[]);
    assert_eq!((code, out.as_str(), err.as_str()), (1, "", "nle: completions fish: --install and --uninstall together (one at a time)\n"));
    assert!(!other.exists(), "refused: nothing written");
}

#[test]
fn cli_completions_uninstall_words() {
    // --uninstall [<dir>] completes like --install in every shell (COMPLETIONS_ARG's Dir: SR1019's generators, no new road).
    let (bash, zsh, fish) = (script_of("bash"), script_of("zsh"), script_of("fish"));
    assert!(bash.contains("        --install|--uninstall) compopt -o dirnames +o default; w=\"\" ;;\n"), "bash: {bash}");
    assert!(zsh.contains("if [[ $words[CURRENT-1] == (--install|--uninstall) ]]; then _files -/;"), "zsh: {zsh}");
    let line = "complete -c nle -n \"__fish_seen_subcommand_from completions\" -l uninstall -x -a \"(__fish_complete_directories)\"";
    assert_eq!(fish.lines().filter(|x| *x == line).count(), 1, "fish: {line:?} in {fish}");
}

// ---- NLECLICOMPLETIONSVERBFLAGS (LEDGER SR1033): each verb's own flag words in every completion script (named tail block) ----
/// NLECLICOMPLETIONSVERBFLAGS (SR1033): main.rs `const VERB_FLAGS`'s rows read from the source — a `&["--x", ..]` list by its
/// words, a `&NAME` list through [`const_words`] (FLAGS / COMPLETIONS_FLAGS).
fn verb_flags() -> Vec<(&'static str, Vec<&'static str>)> {
    let src: &'static str = include_str!("../src/main.rs");
    let body = src.split_once("const VERB_FLAGS: ").and_then(|(_, r)| r.split_once("= [")).and_then(|(_, r)| r.split_once("\n];")).map(|(b, _)| b).expect("main.rs VERB_FLAGS");
    body.split("(\"")
        .skip(1)
        .map(|e| {
            let (verb, rest) = e.split_once('"').expect("a verb word");
            let rest = rest.trim_start_matches(", ").trim_end().trim_end_matches(',').trim_end_matches(')');
            let words = match rest.strip_prefix("&[") {
                Some(l) => l.split('"').skip(1).step_by(2).collect(),
                None => const_words(rest.trim_start_matches('&')),
            };
            (verb, words)
        })
        .collect()
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): fish's ` -n "__fish_seen_subcommand_from <verbs>"` token of `flag`'s `-l` line —
/// the [`verb_flags`] verbs that take it, in VERBS order.
fn fish_n(flag: &str) -> String {
    let verbs: Vec<&str> = verb_flags().into_iter().filter(|(_, f)| f.contains(&flag)).map(|(v, _)| v).collect();
    format!(" -n \"__fish_seen_subcommand_from {}\"", verbs.join(" "))
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): `s` with every string literal's contents removed (the quotes kept), so a word inside
/// a printed text (`"out={} .."`) is not read as the variable.
fn code_only(s: &str) -> String {
    let (mut out, mut in_str, mut esc) = (String::new(), false, false);
    for c in s.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
                out.push(c);
            }
            continue;
        }
        if c == '"' {
            in_str = true;
        }
        out.push(c);
    }
    out
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): `var` appears in `code` as a whole identifier (`out` is not found in `out_t`).
fn reads_var(code: &str, var: &str) -> bool {
    let id = |c: char| c.is_ascii_alphanumeric() || c == '_';
    code.match_indices(var).any(|(i, _)| code[..i].chars().next_back().is_none_or(|c| !id(c)) && code[i + var.len()..].chars().next().is_none_or(|c| !id(c)))
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): the `"--x"` literals of `text` in order, each once, whose opening quote follows
/// `before` and whose closing quote is followed by `after` (`== "--x"`: before `== `; an arm `"--x" =>`: after ` =>`).
fn literals(text: &'static str, before: &str, after: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for (i, _) in text.match_indices("\"--") {
        let Some(end) = text[i + 1..].find('"') else { continue };
        let lit = &text[i + 1..i + 1 + end];
        if text[..i].ends_with(before) && text[i + 2 + end..].starts_with(after) && !out.contains(&lit) {
            out.push(lit);
        }
    }
    out
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): what each verb READS, measured from main.rs. The option loop (schedule / render /
/// export: ONE loop) gives each FLAGS arm's target variable (the first identifier after `=>`; a block arm's `X.push(`); a
/// verb takes the flags whose variable the shared `load(` line or that verb's `match verb` arm reads. `size` is
/// `size_verb`'s `== "--x"` literals, `--make-src` its block's `"--x"` literals but its own name, `completions`
/// `completions_out`'s `"--x" =>` arms (in COMPLETIONS_FLAGS order), `help` / `--help` none.
fn measured_verb_flags() -> Vec<(&'static str, Vec<&'static str>)> {
    let src: &'static str = include_str!("../src/main.rs");
    let flags = const_words("FLAGS");
    let span = |from: &str, to: &str| -> &'static str {
        let a = src.find(from).unwrap_or_else(|| panic!("main.rs: {from:?}"));
        let b = src[a..].find(to).unwrap_or_else(|| panic!("main.rs: {to:?} after {from:?}"));
        &src[a..a + b]
    };
    let ident_end = |s: &str| s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(s.len());
    // The option loop's arms and the variable each sets.
    let lp = span("let mut it = args[2..].iter();", "_ => die(unknown_flag(a))");
    let mut at: Vec<(usize, &str)> = flags.iter().map(|f| (lp.find(&format!("\"{f}\" =>")).unwrap_or_else(|| panic!("{f}: an arm of the option loop")), *f)).collect();
    at.sort();
    let var_of = |f: &str| -> &'static str {
        let i = at.iter().position(|(_, g)| *g == f).unwrap();
        let start = at[i].0 + f.len() + 5;
        let body = lp[start..at.get(i + 1).map_or(lp.len(), |n| n.0)].trim_start();
        if body.starts_with('{') {
            let head = &body[..body.find(".push(").unwrap_or_else(|| panic!("{f}: a block arm pushes"))];
            &head[head.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(0, |j| j + 1)..]
        } else {
            &body[..ident_end(body)]
        }
    };
    let common = code_only(span("let (tl, sources) = load(", "\n"));
    let arms = span("match verb.as_str() {", "_ => usage(),");
    let arm = |v: &str| -> String {
        let a = arms.find(&format!("\"{v}\" =>")).unwrap_or_else(|| panic!("{v}: an arm of match verb"));
        let next = ["\"schedule\" =>", "\"render\" =>", "\"export\" =>"].iter().filter_map(|p| arms[a + 1..].find(*p).map(|j| a + 1 + j)).min().unwrap_or(arms.len());
        code_only(&arms[a..next])
    };
    let mut out: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    for v in ["schedule", "render", "export"] {
        let code = arm(v);
        out.push((v, flags.iter().copied().filter(|f| { let x = var_of(*f); reads_var(&common, x) || reads_var(&code, x) }).collect()));
    }
    out.push(("size", literals(span("fn size_verb(", "\n}\n"), "== ", "")));
    out.push(("--make-src", literals(span("Some(\"--make-src\") {", "return;"), "", "").into_iter().filter(|w| *w != "--make-src").collect()));
    let own = literals(span("fn completions_out(", "_ => die("), "", " =>");
    let cf: Vec<&'static str> = const_words("COMPLETIONS_FLAGS").into_iter().filter(|w| own.contains(w)).collect();
    assert_eq!(cf.len(), own.len(), "completions_out's arms {own:?} are COMPLETIONS_FLAGS words");
    out.push(("completions", cf));
    out.push(("help", Vec::new()));
    out.push(("--help", Vec::new()));
    out
}

#[test]
fn cli_completions_verb_flags_scan() {
    // VERB_FLAGS: one row per VERBS word, in order; each row exactly what that verb reads (both directions).
    let table = verb_flags();
    assert_eq!(table.iter().map(|(v, _)| *v).collect::<Vec<_>>(), const_words("VERBS"), "main.rs VERB_FLAGS in VERBS order");
    let measured = measured_verb_flags();
    for (v, flags) in &table {
        let want = &measured.iter().find(|(m, _)| m == v).unwrap_or_else(|| panic!("{v}: a verb the scan measures")).1;
        assert_eq!(flags, want, "{v}: VERB_FLAGS vs what main.rs reads for it");
    }
    // The GOAL's cases by name: export takes FLAGS; completions its own words, no export flag; size only --fps.
    let of = |v: &str| table.iter().find(|(w, _)| *w == v).map(|(_, f)| f.clone()).unwrap();
    assert_eq!(of("export"), const_words("FLAGS"));
    assert_eq!(of("completions"), ["--install", "--force", "--uninstall"]);
    assert_eq!(of("size"), ["--fps"]);
    assert_eq!(of("schedule"), ["--fps", "--src"]);
    assert_eq!(of("render"), ["--fps", "--out", "--src"]);
    // Every FLAGS word is some verb's.
    for f in const_words("FLAGS") {
        assert!(table.iter().any(|(_, w)| w.contains(&f)), "{f}: in a VERB_FLAGS row");
    }
}

/// NLECLICOMPLETIONSVERBFLAGS (SR1033): bash's `_nle` run on `words` (COMP_WORDS; COMP_CWORD the last), the script sourced
/// from `script`: COMPREPLY, one word each.
fn bash_complete(script: &Path, words: &[&str]) -> Vec<String> {
    let o = Command::new("bash")
        .arg("-c")
        .arg("source \"$1\"; shift; COMP_WORDS=(\"$@\"); COMP_CWORD=$((${#COMP_WORDS[@]} - 1)); _nle; printf '%s\\n' \"${COMPREPLY[@]}\"")
        .arg("nle-complete")
        .arg(script)
        .args(words)
        .output()
        .expect("bash runs");
    assert!(o.status.success(), "{words:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

#[test]
fn cli_completions_verb_flags_bash() {
    let bash = script_of("bash");
    assert!(bash.contains("    local f=\"\"\n    case ${COMP_WORDS[1]} in\n"), "bash: the verb case in {bash}");
    assert!(bash.contains("    (( COMP_CWORD > 1 )) && w=$f\n"), "bash: the verb's words past word 1 in {bash}");
    let p = scratch("verb-flags-bash").join("nle.bash");
    std::fs::write(&p, &bash).unwrap();
    let o = Command::new("bash").arg("-n").arg(&p).output().expect("bash runs");
    assert!(o.status.success(), "bash -n: {}\n{bash}", String::from_utf8_lossy(&o.stderr));
    let table = verb_flags();
    let of = |v: &str| table.iter().find(|(w, _)| *w == v).map(|(_, f)| f.iter().map(|x| x.to_string()).collect::<Vec<String>>()).unwrap();
    // `nle <verb> <edl> --<TAB>`: exactly that verb's flags (no `--qp` after completions, no `--video` after size).
    let got = bash_complete(&p, &["nle", "completions", "bash", "--"]);
    assert_eq!(got, of("completions"), "nle completions bash --<TAB>");
    assert!(!got.iter().any(|w| w == "--qp"), "{got:?}");
    assert_eq!(bash_complete(&p, &["nle", "export", "x.edl", "--"]), of("export"), "nle export x.edl --<TAB>");
    assert_eq!(bash_complete(&p, &["nle", "size", "x.edl", "--"]), ["--fps"], "nle size x.edl --<TAB>");
    assert_eq!(bash_complete(&p, &["nle", "schedule", "x.edl", "--"]), of("schedule"), "nle schedule x.edl --<TAB>");
    assert_eq!(bash_complete(&p, &["nle", "render", "x.edl", "--"]), of("render"), "nle render x.edl --<TAB>");
    // `nle completions <TAB>`: the shells.
    let shells = const_words("SHELLS");
    assert_eq!(shells, ["bash", "zsh", "fish"], "main.rs SHELLS");
    assert_eq!(bash_complete(&p, &["nle", "completions", ""]), shells, "nle completions <TAB>");
    assert_eq!(bash_complete(&p, &["nle", "completions", "z"]), ["zsh"], "nle completions z<TAB>");
}

#[test]
fn cli_completions_verb_flags_text() {
    let (zsh, fish) = (script_of("zsh"), script_of("fish"));
    let shells = const_words("SHELLS").join(" ");
    // zsh: ONE `case $words[2]`, each verb's `flags=(` arm naming exactly its specs; `_arguments` reads `$flags`.
    assert!(zsh.starts_with("#compdef nle\nlocal -a flags\ncase $words[2] in\n"), "{zsh}");
    assert!(zsh.contains("esac\n_arguments '1:verb:("), "{zsh}");
    assert!(zsh.contains("' $flags '2:shell:"), "{zsh}");
    for (v, flags) in verb_flags() {
        let head = format!("    {v}) flags=(");
        let line = zsh.lines().find(|l| l.starts_with(&head)).unwrap_or_else(|| panic!("zsh: {head:?} in {zsh}"));
        assert!(line.ends_with(" ) ;;"), "zsh: {line:?}");
        let named: Vec<&str> = line.match_indices(" '*--").map(|(i, _)| { let w = &line[i + 3..]; &w[..w.find([':', '\'']).unwrap()] }).collect();
        assert_eq!(named, flags, "zsh: {v}'s specs in {line:?}");
    }
    let comp = zsh.lines().find(|l| l.starts_with("    completions) flags=(")).unwrap();
    assert!(!comp.contains("'*--qp"), "zsh: no export flag after completions: {comp:?}");
    assert!(zsh.contains(&format!(" '2:shell:{{if [[ $words[2] == completions ]]; then compadd -- {shells}; else ")), "zsh: the shells in {zsh}");
    // fish: every `-l` line is under the verbs that take it; the shells line; --force under completions.
    for f in const_words("FLAGS") {
        let line = format!("complete -c nle{} -l {}", fish_n(f), f.trim_start_matches('-'));
        assert_eq!(fish.lines().filter(|x| *x == line || x.starts_with(&format!("{line} "))).count(), 1, "fish: {line:?} in {fish}");
    }
    for l in fish.lines().filter(|l| l.contains(" -l ")) {
        assert!(l.starts_with("complete -c nle -n \"__fish_seen_subcommand_from "), "fish: an unconditioned flag line {l:?}");
    }
    let sh = format!("complete -c nle -n \"__fish_seen_subcommand_from completions; and test (count (commandline -opc)) -eq 2\" -xa \"{shells}\"");
    assert!(fish.lines().any(|x| x == sh), "fish: {sh:?} in {fish}");
    assert!(fish.lines().any(|x| x == "complete -c nle -n \"__fish_seen_subcommand_from completions\" -l force"), "fish: --force in {fish}");
    // The dispatch refusal names SHELLS, bytes unchanged.
    let (code, out, err) = nle(&["completions", "tcsh"]);
    assert_eq!((code, out.as_str(), err), (1, "", format!("nle: completions tcsh: one of {}\n", const_words("SHELLS").join("|"))));
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

// ---- NLECLIHELPVERB (LEDGER SR1041): `nle <verb> --help` prints that verb's section; the full usage frozen (named tail block) ----
/// NLECLIHELPVERB (SR1041): the full usage as main.rs `usage_to` printed it BEFORE this arc (frozen at 5995ad1cf from its six
/// writelns, pinned once here): `{video}` / `{audio}` are stria's tables joined `|` (they may grow; the rest may not move).
const USAGE_FROZEN: [&str; 6] = [
    "usage: nle schedule <edl|media> [--fps N] --src REEL=<media> ...\n",
    "       nle render <edl|media> [--fps N] --src REEL=<media> ... --out <stem>\n",
    "       nle export <edl|media> [--fps N] --src REEL=<media> ... --out <dir> [--video {video}] [--qp N] [--gop N] [--bitrate KBPS | --crf N] [--scenecut [K]] [--min-keyint N] [--aq [K]] [--two-pass | --pass 1|2 --stats <file>] [--qcomp F] [--cplxblur N] [--qblur F] [--qpstep N] [--pass2 k=v,...] [--hrd] [--audio {audio}|aac:KBPS] [--aac] [--mkv] [--png]\n",
    "       nle --make-src <out.mp4> <base> [frames] [fps]\n",
    "       nle size <edl> <reel> <letterbox|fill|stretch|adopt> [--fps N]\n",
    "       nle completions bash|zsh|fish [--install [<dir>] | --uninstall [<dir>]] [--force]\n",
];

/// NLECLIHELPVERB (SR1041): [`USAGE_FROZEN`] with stria's tables filled, one text.
fn usage_frozen() -> String {
    USAGE_FROZEN.concat().replace("{video}", &VIDEO_NAMES.join("|")).replace("{audio}", &AUDIO_NAMES.join("|"))
}

/// NLECLIHELPVERB (SR1041): `nle help` / `nle --help` (stdout, 0) and bare `nle` (stderr, 2) are the frozen usage byte for byte.
#[test]
fn cli_help_full_usage_frozen() {
    let want = usage_frozen();
    for road in [&["help"][..], &["--help"][..]] {
        let (code, out, err) = nle(road);
        assert_eq!((code, out.as_str(), err.as_str()), (0, want.as_str(), ""), "nle {road:?}");
    }
    assert_eq!(usage_text(), want, "nle (no verb): the stderr usage");
}

/// NLECLIHELPVERB (SR1041): `nle <verb> --help` for each verb with a usage line: exit 0 on stdout, line 0 `usage: ` + that
/// verb's line of the full usage, then exactly SR1033's `VERB_FLAGS` row in order, one `  <flag>[ <word>]` per line, the
/// word cut from the verb's own line (FLAG_ARG `Bare` <=> no word; the table flags' words are the owners' tables).
#[test]
fn cli_help_verb_sections() {
    let full = usage_frozen();
    let table = verb_flags();
    let flags = const_words("FLAGS");
    let kinds = flag_arg_src();
    assert_eq!(flags.len(), kinds.len(), "FLAG_ARG parallel to FLAGS");
    let audio: Vec<&str> = AUDIO_NAMES.iter().copied().chain(["aac:KBPS"]).collect();
    let mut seen = Vec::new();
    for v in ["schedule", "render", "export", "size", "--make-src", "completions"] {
        let (code, out, err) = nle(&[v, "--help"]);
        assert_eq!((code, err.as_str()), (0, ""), "nle {v} --help");
        let lines: Vec<&str> = out.lines().filter(|l| !l.starts_with("example: ")).collect(); /* SR1049 */ // NLECLIHELPUNKNOWNEXAMPLES (SR1049): the trailing example line is cli_help_examples_end_sections'
        let line = full.lines().map(|l| l.trim_start_matches("usage: ").trim_start()).find(|l| l.starts_with(&format!("nle {v} "))).unwrap_or_else(|| panic!("{v}: a line of the full usage"));
        assert_eq!(lines[0], format!("usage: {line}"), "nle {v} --help: the verb's own usage line");
        let want = &table.iter().find(|(w, _)| *w == v).unwrap_or_else(|| panic!("{v}: a VERB_FLAGS row")).1;
        let got: Vec<&str> = lines[1..].iter().map(|l| { assert!(l.starts_with("  --"), "{v}: {l:?}"); l[2..].split(' ').next().unwrap() }).collect();
        assert_eq!(&got, want, "nle {v} --help: VERB_FLAGS' row in order");
        for l in &lines[1..] {
            let (f, word) = l[2..].split_once(' ').unwrap_or((&l[2..], ""));
            if !word.is_empty() {
                assert!(line.contains(&format!("{f} {word}")), "{v}: {f} {word:?} cut from {line:?}");
            }
            if let Some(k) = flags.iter().position(|x| *x == f).map(|i| kinds[i]) {
                assert_eq!(k == "Bare", word.is_empty(), "{v}: {f} is {k}, word {word:?}");
            }
            let joined = |t: &[&str]| t.join("|");
            match f {
                "--video" => assert_eq!(word, joined(&VIDEO_NAMES[..]), "--video's word"),
                "--audio" => assert_eq!(word, joined(&audio), "--audio's word"),
                "--pass" => assert_eq!(word, joined(&const_words("PASS_WORDS")), "--pass's word"),
                _ => {}
            }
            seen.push(format!("{v} {}", &l[2..]));
        }
    }
    for want in ["size --fps N", "export --fps N", "export --out <dir>", "render --out <stem>", "export --scenecut [K]", "export --stats <file>", "export --src REEL=<media>", "export --pass2 k=v,...", "export --two-pass", "completions --install [<dir>]", "completions --force", "completions --uninstall [<dir>]"] {
        assert!(seen.iter().any(|x| x == want), "{want:?} in {seen:?}");
    }
    let section = |v: &str| nle(&[v, "--help"]).1.lines().filter(|l| !l.starts_with("example: ")).map(|l| format!("{l}\n")).collect::<String>(); /* SR1049 */ // NLECLIHELPUNKNOWNEXAMPLES (SR1049): minus the example line
    assert_eq!(section("size").lines().count(), 2, "size: the line + --fps");
    assert_eq!(section("completions").lines().count(), 4, "completions: the line + its three flags");
    assert_eq!(section("export").lines().count(), 1 + flags.len(), "export: the line + every FLAGS word");
    let ms = section("--make-src");
    assert_eq!(ms, format!("usage: {}\n", USAGE_FROZEN[3].trim_start().trim_end_matches('\n')), "--make-src: the line alone");
    assert!(!Path::new(env!("CARGO_MANIFEST_DIR")).join("--help").exists(), "--make-src --help writes no file");
}

/// NLECLIHELPVERB (SR1041), flipped by NLECLIHELPUNKNOWNEXAMPLES (SR1049, NLE-CLI-HELP-UNKNOWN-VERB): an unknown verb with
/// `--help` is the usage on stderr, exit 2 (main.rs `verb_hint`), never `load` reading `--help` as the EDL path (exit 1
/// before SR1049); a word near a verb keeps SR816's hint first.
#[test]
fn cli_help_unknown_verb_as_today() {
    let usage = usage_frozen(); /* SR1049 */
    let (code, out, err) = nle(&["bogus", "--help"]);
    assert_eq!((code, out.as_str(), err.as_str()), (2, "", usage.as_str()), "nle bogus --help"); /* SR1049 */
    let (code, out, err) = nle(&["expot", "--help"]);
    assert_eq!((code, out.as_str(), err), (2, "", format!("nle: expot: unknown verb; did you mean export?\n{usage}")), "nle expot --help"); /* SR1049 */
    assert!(!Path::new(env!("CARGO_MANIFEST_DIR")).join("--help").exists(), "no file named --help"); /* SR1049 */
}

// ---- NLECLIHELPUNKNOWNEXAMPLES (LEDGER SR1049): each verb's `--help` ends with its example; every example runs (named tail block) ----
/// NLECLIHELPUNKNOWNEXAMPLES (SR1049): main.rs `USAGE_EXAMPLES` as (verb, line) pairs, read from the source.
fn examples() -> Vec<(&'static str, &'static str)> {
    let w = const_words("USAGE_EXAMPLES");
    assert!(!w.is_empty() && w.len() % 2 == 0, "USAGE_EXAMPLES read from main.rs: {w:?}");
    w.chunks(2).map(|p| (p[0], p[1])).collect()
}

/// NLECLIHELPUNKNOWNEXAMPLES (SR1049): USAGE_EXAMPLES is keyed like USAGE_LINES (same verbs, same order); `nle <verb> --help`
/// (stdout, 0) ends with exactly one `example: <line>`, the line opens `nle <verb> `; the full usage carries none.
#[test]
fn cli_help_examples_end_sections() {
    let ex = examples();
    let keys: Vec<&str> = const_words("USAGE_LINES").chunks(2).map(|p| p[0]).collect();
    assert_eq!(ex.iter().map(|(v, _)| *v).collect::<Vec<_>>(), keys, "USAGE_EXAMPLES keys == USAGE_LINES keys, in order");
    for (v, e) in &ex {
        let (code, out, err) = nle(&[*v, "--help"]);
        assert_eq!((code, err.as_str()), (0, ""), "nle {v} --help");
        assert_eq!(out.lines().last(), Some(format!("example: {e}").as_str()), "nle {v} --help: the example last");
        assert_eq!(out.lines().filter(|l| l.starts_with("example: ")).count(), 1, "nle {v} --help: one example");
        assert!(e.starts_with(&format!("nle {v} ")), "{v}: the example is the verb's: {e}");
    }
    let (code, out, _) = nle(&["help"]);
    assert_eq!((code, out.as_str()), (0, usage_frozen().as_str()), "nle help: the frozen usage");
    assert!(!out.contains("example: "), "nle help: no example");
}

/// NLECLIHELPUNKNOWNEXAMPLES (SR1049): every example RUNS — its words (the leading `nle` dropped) in a fresh [`fixture`] dir as
/// cwd and HOME (XDG unset): exit 0, no usage / unknown-word refusal on stderr, and the verb's artefact.
#[test]
fn cli_help_examples_run() {
    for (v, e) in examples() {
        let name = format!("example-{}", v.trim_start_matches('-'));
        let (dir, _, _) = fixture(&name);
        let words: Vec<&str> = e.split(' ').collect();
        assert_eq!(words[0], "nle", "{v}: {e}");
        let edl_before = std::fs::read(dir.join("cut.edl")).unwrap();
        let o = Command::new(env!("CARGO_BIN_EXE_nle")).args(&words[1..]).current_dir(&dir).env("HOME", &dir).env_remove("XDG_DATA_HOME").env_remove("XDG_CONFIG_HOME").output().expect("the built nle runs");
        let (code, out, err) = (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned());
        assert_eq!(code, 0, "{e}: {err}");
        assert!(!err.lines().any(|l| l.starts_with("usage:") || l.contains("unknown flag") || l.contains("unknown verb")), "{e}: refused: {err}");
        match v {
            "schedule" => assert!(out.lines().any(|l| l.starts_with("out=")), "{e}: the steps: {out}"),
            "render" => assert!(std::fs::metadata(dir.join("frame.rgba")).unwrap().len() > 0, "{e}: frame.rgba"),
            "export" => assert!(std::fs::metadata(dir.join("out").join("video.mp4")).unwrap().len() > 0, "{e}: out/video.mp4"),
            "--make-src" => assert!(std::fs::metadata(dir.join("a.mp4")).unwrap().len() > 0, "{e}: a.mp4"),
            "size" => {
                assert!(out.starts_with("nle: size A letterbox -> "), "{e}: {out}");
                assert_ne!(std::fs::read(dir.join("cut.edl")).unwrap(), edl_before, "{e}: cut.edl rewritten");
            }
            "completions" => {
                let p = dir.join(".local/share/bash-completion/completions/nle");
                assert_eq!(std::fs::read_to_string(&p).unwrap_or_default(), completion_script("bash"), "{e}: {}", p.display());
            }
            _ => panic!("{v}: an example with no artefact row here: measure it and add one"),
        }
    }
}
