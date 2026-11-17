// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `stria-view` — Stria's editor vessel (NLE2, LEDGER SR201): a window over `stria::nle_face`.
//!
//! Like every vessel this binary is wiring and lifecycle only (facet-view's shape). It opens an EDL (or NLE1's
//! synthetic fixture), starts a Synapse, serves `stria::nle_face::serve` on it and summons ONE Quartzite tetra tree —
//! the editor surface over a transport row — through `Backend::new_tetra_vessel`, which Quartzite's `eval_tetra`
//! evaluates on AppKit (macOS) and GTK (`--features gtk`; no Qt). The headless witness drives the very same `Face` with
//! the same messages and prints one JSON line:
//!
//! ```text
//! stria-view <edl> [--fps N] --src REEL=<media> ...            # the window
//! stria-view --synth                                           # NLE1's two-source fixture
//! stria-view --synth --size 320x160 --do at:12:0,split,delete,undo [--out shot.rgba] [--labels]
//! ```
//! `--do`: comma-separated transport verbs (`stria::nle_face::VERBS`), `at:<tick>:<track>` (a click on that track at
//! that tick) and `ruler:<tick>` (a click on the ruler); STRIAFREEZEMORE (SR970): `freeze:<len>`, `freeze-all:<len>`, `title:<text>`, `retitle:<text>` (the word, a space, everything after the first `:`). NLE2B (SR208), the gestures as a face sends them:
//! `press:<x>:<y>` / `move:<x>:<y>` (surface pixels), `release` (at the last pointer: `BrowserButton` up + `BrowserClick`),
//! `key:<name>` (as GTK sends it: `BrowserKey`, plus `BrowserText` for a one-character key; `key:space` types a space)
//! and `wheel:<n>` (n notches at the last pointer, n > 0 up = zoom in). The JSON line also carries `playing`, `rate`
//! and the selected clip as `clip: "<at>:<src_in>:<src_out>"`. NLE2c (SR217): `shift:<token>` / `alt:<token>` set
//! `MOD_SHIFT` / `MOD_ALT` in the `mods` of every press the token sends (NLE2-MODS SR306, as both faces send them: add to
//! the set / no snapping), `marquee:<x0>:<y0>:<x1>:<y1>` is a press, a move and the release, `key:Ctrl+a` /
//! `key:Meta+a` is the `BrowserSelectAll` both faces send for it, and the JSON line ends with the set's size `set`.
//! NLE3d (SR302): `transition:<track>:<idx>` clicks that transition's widget (selecting it; `delete`, `[` / `]` then edit
//! it), and the JSON line ends with `transition` (the selected one and its thumbnail's size, or `null`).
//! STRIATRANSITIONLENGTH (SR870): `dur:<n>` gives the selected transition exactly n ticks (timeline_core's `dur <n>` word).
//! STRIATRANSITIONMORE (SR956): `dur:HH:MM:SS:FF` gives it a timecode count (the EDL's one parser, at the timeline's fps).
//! NLE4C (SR301): `meters` feeds the face's meters the tick at the playhead (what one played tick feeds); the JSON line
//! then ends `"meters":[{"track":<n>,"peak":<dBFS>,"rms":<dBFS>},...,{"track":"master",...}]`.
//! SURFACEID (SR332): `--two-surfaces` puts a second Surface ([`AUX_SURFACE`]) above the editor's in the tree, so the
//! editor's Surface is number 1 (quartzite's `tetra::SurfaceMap`) and the face is routed to it; every token's pointer
//! messages carry the editor's number, `aux:<token>` the aux Surface's, and the JSON line ends
//! `"presses":[[<surface>,<taken>],...]` (each press, the Surface it carried and whether the face took it).
//! NLE2-SPLIT (SR339): `--split` builds the two-Surface tree ([`split_tree`]: the preview Surface over the transport row,
//! the tracks Surface under it) and the face splits over them (`Face::on_surfaces`); the window runs it, and the witness
//! stamps tokens with the tracks Surface's number (`aux:<token>`: the preview's), its line ending
//! `"presses":[...],"surfaces":2,"fnvs":["<preview>","<tracks>"]` (`w` / `h` / `fnv` are the tracks blit's).
//! NLEHOSTSAVE (SR677): the face keeps the `<edl>` path (`Face::with_doc`); `save` (the Save button, `--do save`) writes it back through `edl::write`, and the JSON line carries `"dirty"` after `transition`.
//! NLEFACEEXPORT (SR694): `export` (the Export button, `--do export`) runs the face's export off its thread; while it runs the JSON line carries `"export":{"done":<n>,"total":<m>}` after the other fields (the default line unchanged).
//! STRIARECENTFILES (SR884): a window's `<edl>` goes first in Principia's `stria.recent` (the last five, `nle_face::remember_doc`); `--do recent` ends the JSON line with `"recent":[...]` read from it (the witness never writes it).

use bandy::{SMessage, Synapse};
use quartzite::tetra::{SurfaceMap, TetraNode};
use stria::nle::Sources;
use stria::nle_face::{self, EditorState, Face, Layout, PixelOrder, PREVIEW_SURFACE, SURFACE, TRACKS_SURFACE};
use timeline_core::{Timeline, edl};

struct Cli {
    edl: Option<String>,
    synth: bool,
    fps: u32,
    srcs: Vec<(String, String)>,
    size: (u32, u32),
    script: Option<String>,
    out: Option<String>,
    labels: bool,
    two: bool,
    split: bool,
    mute: bool,
    keys: Option<String>, // STRIAKEYBOARDMAP (SR886): `--keys MAP`, the key overlay text handed to `Face::with_keys`
    bin: bool, // STRIAPROJECTBIN (SR885): `--bin` puts the source bin Surface under the split tree's tracks (file tail)
}

/// SURFACEID (SR332): the second Surface of `--two-surfaces` (no painter: a press on it must not reach the editor).
const AUX_SURFACE: &str = "stria.nle.aux";

const USAGE: &str = "usage: stria-view <edl> [--fps N] --src REEL=<media|dir/frame_%04d.png> ... | --synth  [--size WxH] [--do SCRIPT] [--out shot.rgba] [--labels] [--two-surfaces | --split [--bin]] [--mute] [--presets] [--keys MAP]  (SCRIPT: verbs, at:T:ROW, ruler:T, press:X:Y, move:X:Y, release, key:NAME, wheel:N, marquee:X0:Y0:X1:Y1, shift:TOKEN, alt:TOKEN, transition:T:I, meters, tick:N, aux:TOKEN, bin:ROW, freeze:LEN, freeze-all:LEN, title:TEXT, retitle:TEXT)";

fn die(msg: String) -> ! {
    eprintln!("stria-view: {msg}");
    std::process::exit(1)
}

fn cli() -> Cli {
    let mut c = Cli { edl: None, synth: false, fps: 25, srcs: Vec::new(), size: (960, 540), script: None, out: None, labels: false, two: false, split: false, mute: false, keys: None, bin: false };
    let mut a = std::env::args().skip(1);
    while let Some(arg) = a.next() {
        match arg.as_str() {
            "--synth" => c.synth = true,
            "--labels" => c.labels = true,
            "--two-surfaces" => c.two = true,
            "--split" => c.split = true,
            "--mute" => c.mute = true, "--paint-ms" => {} // NLEPLAYAUDIO (SR676): the window plays no sound
            "--bin" => c.bin = true, // STRIAPROJECTBIN (SR885): the source bin Surface (with --split)
            "--fps" => c.fps = a.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| die(USAGE.into())),
            "--size" => {
                let v = a.next().unwrap_or_default();
                let (w, h) = v.split_once('x').unwrap_or_else(|| die(USAGE.into()));
                c.size = (w.parse().unwrap_or(960), h.parse().unwrap_or(540));
            }
            "--src" => {
                let v = a.next().unwrap_or_else(|| die(USAGE.into()));
                let (r, p) = v.split_once('=').unwrap_or_else(|| die(USAGE.into()));
                c.srcs.push((r.to_string(), p.to_string()));
            }
            "--presets" => { print!("{}", presets_text()); std::process::exit(0) } // STRIAEXPORTPRESETS (SR871): the export presets, one line each, on stdout (file tail)
            "--do" => c.script = a.next(),
            "--keys" => c.keys = a.next(), // STRIAKEYBOARDMAP (SR886): the key map as text (entries split on newline or `;`)
            "--out" => c.out = a.next(),
            "--help" | "-h" => {
                eprintln!("{USAGE}");
                std::process::exit(0);
            }
            _ if !arg.starts_with('-') => c.edl = Some(arg),
            _ => die(USAGE.into()),
        }
    }
    if (c.edl.is_none() && !c.synth) || (c.two && c.split) || (c.bin && !c.split) { // STRIAPROJECTBIN (SR885): the bin rides the split tree
        die(USAGE.into());
    }
    c
}

/// The timeline and its media: the EDL's reels each from its `--src` (tools/nle's flag), or the synthetic fixture.
fn load(c: &Cli) -> (Timeline, Sources) {
    let Some(path) = c.edl.as_deref() else { return nle_face::synth_fixture() };
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let tl = edl::parse(&text, c.fps).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let mut sources = Sources::new(&tl);
    for s in tl.sources.iter().filter(|s| s.title.is_none()) { // STRIATITLECLIP (SR828): a title source has no file
        let Some((_, p)) = c.srcs.iter().find(|(r, _)| *r == s.reel) else { die(format!("no --src for reel {}", s.reel)) };
        if stria::nle::seq_pattern(p).is_some() { sources.add_sequence(s.id, p).unwrap_or_else(|e| die(format!("{p}: {e}"))); continue; } let bytes = std::fs::read(p).unwrap_or_else(|e| die(format!("{p}: {e}"))); // STRIAIMPORTIMAGESEQUENCE (SR888): a `%04d` path is a PNG sequence reel
        sources.add(s.id, bytes).unwrap_or_else(|e| die(format!("{p}: {e}")));
    }
    (tl, sources)
}

/// THE editor tree, declared once: the editor surface over the transport row. Every button fires
/// `SMessage::Input { target: SURFACE, text: <verb> }`.
fn editor_tree() -> TetraNode {
    editor_tree_for(false)
}

/// SURFACEID (SR332): the editor tree, with `two` a second Surface ([`AUX_SURFACE`]) first, above the editor's.
fn editor_tree_for(two: bool) -> TetraNode {
    let aux = two.then(|| TetraNode::Surface { id: AUX_SURFACE.into() });
    TetraNode::VStack(aux.into_iter().chain([TetraNode::Surface { id: SURFACE.into() }, transport()]).collect())
}

/// A `--do` token as the messages the window would send.
fn token(face: &Face, tok: &str) -> Option<Vec<SMessage>> {
    let num = |s: &str| s.parse::<i64>().ok();
    let px = |s: &str| s.parse::<f64>().ok();
    let l: Layout = face.layout();
    let (lx, ly) = face.pointer().unwrap_or((0.0, 0.0));
    for (prefix, bit) in [("shift:", bandy::MOD_SHIFT), ("alt:", bandy::MOD_ALT)] {
        if let Some(rest) = tok.strip_prefix(prefix) {
            let mut out = token(face, rest)?;
            for m in &mut out {
                if let SMessage::BrowserButton { down: true, mods, .. } = m {
                    *mods |= bit;
                }
            }
            return Some(out);
        }
    }
    if matches!(tok, "key:Ctrl+a" | "key:Meta+a") {
        return Some(vec![SMessage::BrowserSelectAll]);
    } if let Some(m) = striapastekeys_token(tok) { return Some(m); } // STRIAPASTEKEYS (SR921): `key:Ctrl+c` / `key:Ctrl+v` (and Meta) are the BrowserCopy / BrowserPaste the surfaces send (file tail)
    if let Some(name) = tok.strip_prefix("key:") {
        let (key, text) = if name == "space" { (name, " ") } else { (name, name) };
        let mut out = vec![SMessage::BrowserKey(key.to_string())];
        if text.chars().count() == 1 {
            out.push(SMessage::BrowserText(text.to_string()));
        }
        return Some(out);
    }
    let parts: Vec<&str> = tok.split(':').collect();
    let one = match parts.as_slice() {
        ["at", t, row] => {
            let (x, y) = l.point(row.parse().ok()?, num(t)?);
            SMessage::BrowserClick(x, y, 0)
        }
        ["transition", t, i] => {
            let (t, i) = (t.parse::<usize>().ok()?, i.parse::<usize>().ok()?);
            let (x0, y0, x1, y1) = l.transition_rect(t, face.state.timeline.tracks.get(t)?.transitions.get(i)?);
            SMessage::BrowserClick((x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0, 0)
        }
        ["ruler", t] => SMessage::BrowserClick(l.x_of(num(t)?) as f64 + 0.5, l.ruler_y as f64 + 1.0, 0),
        ["press", x, y] => SMessage::BrowserButton { down: true, x: px(x)?, y: px(y)?, mods: 0, surface: 0 },
        ["move", x, y] => SMessage::BrowserMouseMove(px(x)?, px(y)?, 0),
        ["release"] => return Some(vec![SMessage::BrowserButton { down: false, x: lx, y: ly, mods: 0, surface: 0 }, SMessage::BrowserClick(lx, ly, 0)]),
        ["marquee", x0, y0, x1, y1] => {
            let (x0, y0, x1, y1) = (px(x0)?, px(y0)?, px(x1)?, px(y1)?);
            return Some(vec![
                SMessage::BrowserButton { down: true, x: x0, y: y0, mods: 0, surface: 0 },
                SMessage::BrowserMouseMove(x1, y1, 0),
                SMessage::BrowserButton { down: false, x: x1, y: y1, mods: 0, surface: 0 },
                SMessage::BrowserClick(x1, y1, 0),
            ]);
        }
        ["wheel", n] => SMessage::BrowserScroll(0.0, -nle_face::WHEEL_STEP * px(n)?, 0),
        [v] if nle_face::VERBS.contains(v) => SMessage::Input { target: SURFACE.into(), text: v.to_string() },
        [w @ ("kind" | "ease" | "dur" | "gap" | "track+" | "track-" | "pan" | "help" | "opacity" | "tc" | "history" | "close-gaps" | "proxy" | "recover" | "undo" | "redo"), arg] => SMessage::Input { target: SURFACE.into(), text: format!("{w} {arg}") }, ["color", b, c, s] => SMessage::Input { target: SURFACE.into(), text: format!("color {b} {c} {s}") }, // STRIACOLORLIFT (SR869): `color:<b>:<c>:<s>` (file tail); STRIATRANSITIONLENGTH (SR870): `dur:7`; STRIAOPACITYFACE (SR898): `opacity:50`; STRIAAUTOSAVEKEEPREAD (SR933): `recover:1` (file tail); NLETRANSITIONFACE (SR691): `kind:W007`, `ease:in`
        [w @ ("insert" | "overwrite"), reel, a, b] => SMessage::Input { target: SURFACE.into(), text: format!("{w} {reel} {a} {b}") }, // STRIAINSERTOVERWRITE (SR852): `insert:A:13:23` (frames: `:` is the token separator); STRIAGAPCLOSE (SR857): `gap:5`; STRIATRACKADDREMOVE (SR856): `track+:audio`, `track-:V2`
        ["track-gain", db] => SMessage::Input { target: SURFACE.into(), text: format!("track-gain {db}") }, // STRIATRACKGAIN (SR864): `track-gain:-6`
        ["crop", l, t, r, b] => SMessage::Input { target: SURFACE.into(), text: format!("crop {l} {t} {r} {b}") }, // STRIACROP (SR867): `crop:250:0:250:0`, the selected clip's crop
        [w] if w.starts_with("crop-") => SMessage::Input { target: SURFACE.into(), text: w.to_string() }, // STRIACROPFACE (SR900): `crop-l+` ... `crop-b-`, one side of the selection's crop by a step
        ["pip", x, y] => SMessage::Input { target: SURFACE.into(), text: format!("pip {x} {y}") }, ["pip", "off"] => SMessage::Input { target: SURFACE.into(), text: "pip off".into() }, ["scale", n] => SMessage::Input { target: SURFACE.into(), text: format!("scale {n}") }, ["pip", c @ ("tl" | "tr" | "bl" | "br" | "left" | "right" | "up" | "down")] => SMessage::Input { target: SURFACE.into(), text: format!("pip {c}") }, // STRIAXFORMMORE (SR975): `pip:left` ... the nudge words; STRIAXFORMPRESETS (SR938): `pip:tl` ... the corner presets; STRIAPOSITIONSCALE (SR879): `pip:725:725` / `pip:off` / `scale:250`, the selected clip's placement (bare `pip` is a VERBS word); STRIATCSET (SR908): `tc:frames` sets the readout; STRIAUNDOHISTORYDRAG (SR912): `history:<n>`; STRIAGAPSALLTRACKS (SR929): `close-gaps:all`; STRIAUNDOJUMP (SR934): `undo:<n>` / `redo:<n>`
        ["dur", h, m, s, f] => SMessage::Input { target: SURFACE.into(), text: format!("dur {h}:{m}:{s}:{f}") }, // STRIATRANSITIONMORE (SR956): `dur:00:00:00:07`
        ["mark", name] => SMessage::Input { target: SURFACE.into(), text: format!("mark {name}") }, // STRIAMARKERSMORE (SR966): `mark:<name>` names / renames the playhead's marker
        ["help"] => SMessage::Input { target: SURFACE.into(), text: "help".into() }, ["find", q] => SMessage::ConsoleInput(q.to_string()), // NLE5DKEYSHELP (SR744) / NLE5DKEYSHELPBUTTON (SR765): the keys help word and button; STRIAHELPMORE (SR977): `find:<q>` the bar field's Enter
        _ => return None,
    };
    Some(vec![one])
}

/// The script through `Face::on`, token by token; the errors it met.
fn drive(face: &mut Face, script: &str) -> Vec<String> {
    drive_on(face, script, None, &mut Vec::new())
}

/// SURFACEID (SR332): [`drive`] on a two-surface tree: `surfaces` = (the editor's number, the aux Surface's); each
/// token's pointer messages are stamped with the editor's (`aux:<token>`: the aux one's), and each press is recorded in
/// `presses` as (its Surface, whether the face took it).
fn drive_on(face: &mut Face, script: &str, surfaces: Option<(u32, u32)>, presses: &mut Vec<(u32, bool)>) -> Vec<String> {
    let mut errors = Vec::new();
    for tok in script.split(',').filter(|t| !t.is_empty()) {
        let (tok, aux) = match tok.strip_prefix("aux:") { Some(rest) => (rest, true), None => (tok, false) };
        match if tok == "meters" { face.feed_meters(face.state.playhead, 1); Some(Vec::new()) } else { tick_token(face, tok).or_else(|| recent_token(tok)).or_else(|| bin_token(face, tok)).or_else(|| nudge_token(tok)).or_else(|| token(face, tok)).or_else(|| word_token(tok)) } { // STRIAINFOREELHOVERDELAY (SR789): `tick:<n>` steps the face clock n times (file tail); STRIAPROJECTBIN (SR885): `bin:<row>` clicks the bin (file tail); STRIANUDGEMORE (SR944): the nudge words (file tail); STRIAFREEZEMORE (SR970): `freeze:<len>` / `title:<text>` ... (file tail)
            Some(ms) => {
                for mut m in ms {
                    if let (Some((editor, other)), Some(s)) = (surfaces, m.surface_mut()) {
                        *s = if aux { other } else { editor };
                    }
                    if let SMessage::BrowserButton { down: true, surface, .. } = m {
                        presses.push((surface, face.takes(&m)));
                    }
                    face.on(&m);
                }
            }
            None => errors.push(format!("bad token {tok}")),
        }
        if let Some(e) = face.last_error.take() {
            errors.push(e);
        }
    }
    errors
}

/// The headless witness: the script through `Face::on`, then the last frame's facts as one JSON line.
fn witness(c: &Cli, mut face: Face) -> i32 {
    let map = SurfaceMap::of(&tree(c));
    let surfaces = if c.split {
        Some((map.number(TRACKS_SURFACE).unwrap_or(1), map.number(PREVIEW_SURFACE).unwrap_or(0)))
    } else {
        c.two.then(|| (map.number(SURFACE).unwrap_or(0), map.number(AUX_SURFACE).unwrap_or(0)))
    };
    let mut presses = Vec::new();
    let mut errors = drive_on(&mut face, c.script.as_deref().unwrap_or(""), surfaces, &mut presses);
    let t0 = std::time::Instant::now(); let blits = face.frames(); let paint = paint_ms_json(paint_ms_on(), t0.elapsed()); // STRIAPERFPAINTBUDGET (SR890): the wall time of the one repaint `serve` pays (file tail)
    let fnvs = blits_json(c, &blits);
    let Some(SMessage::SurfaceBlit { width, height, pixels, .. }) = blits.into_iter().filter(|m| !matches!(m, SMessage::SurfaceBlit { url, .. } if url == nle_face::BIN_SURFACE)).last() else { return 1 }; // STRIAPROJECTBIN (SR885): w / h / fnv stay the tracks blit's with --bin
    if let Some(e) = face.last_error.take() {
        errors.push(e);
    }
    if let Some(out) = &c.out {
        if let Err(e) = std::fs::write(out, &pixels) {
            eprintln!("stria-view: {out}: {e}");
            return 1;
        }
    }
    let st = &face.state;
    let clips: usize = st.timeline.tracks.iter().map(|t| t.clips.len()).sum();
    let sel = st.selection.map(|(t, i)| format!("\"{t}:{i}\"")).unwrap_or_else(|| "null".into());
    let clip = st.selected().map(|c| format!("\"{}:{}:{}{}\"", c.at, c.src_in, c.src_out, if c.rate_q16 == 0x1_0000 { String::new() } else { format!(":{}", c.rate_q16) })).unwrap_or_else(|| "null".into()); // STRIACLIPSPEED (SR829): `:<rate_q16>` only at a rate other than 1x
    let err = errors.join("; ").replace('"', "'");
    let meters = meters_json(c, &face) + &help_json(&face) + &snap_json(&face) + &tc_json(&face) + &help_search_json(&face) + &info_json(&face) + &clipped_json(&face) + &autosaved_json(&face) + &held_json(&face) + &slip_json(&face); // STRIAAUTOSAVECAP (SR913): `"autosaved":true` while this session's autosave stands (file tail); STRIASLIPVIEWMORE (SR946): `"slip":"WxH"` while a slip / slide pair shows (file tail); NLE5DKEYSHELP (SR744): `"help":true` while the keys help shows (file tail); STRIASNAPMORE (SR952): `"snap":false` while the drag snap default is off (file tail)
    let presses = presses_json(c, &presses); let export = export_json(&face) + &range_json(&face) + &reel_json(&face) + &queued_json(&face) + &bin_json(c, &face) + &recent_json(c) + &frame_json(&face) + &markers_json(&face) + &color_json(&face) + &paint + &proxy_json(&face); // STRIACOLORLIFT (SR869): the selected clip's grade (file tail); STRIAPROXYBUTTONS (SR930): the reels with an attached proxy (file tail); NLEFACEEXPORT (SR694): the running export's reading (file tail); NLE5DEXPORTRANGE (SR717): the marks' range (file tail); STRIAINFOREEL (SR743): the selected reel (file tail); STRIAEXPORTFRAME (SR830): the saved frame (file tail); STRIAMARKERS (SR835): the markers (file tail); STRIAPROJECTBIN (SR885): the bin's rows with --bin (file tail); STRIAEXPORTQUEUEMORE (SR971): the waiting count (file tail)
    println!(
        "{{\"playhead\":{},\"tracks\":{},\"clips\":{},\"selection\":{sel},\"clip\":{clip},\"playing\":{},\"rate\":{},\"undo\":{},\"zoom\":{},\"w\":{width},\"h\":{height},\"fnv\":\"{:016x}\",\"errors\":\"{err}\",\"set\":{},\"transition\":{},\"dirty\":{}{meters}{presses}{fnvs}{export}{}}}",
        st.playhead,
        st.timeline.tracks.len(),
        clips,
        face.playing,
        face.rate,
        st.undo_depth(),
        st.zoom,
        nle_face::fnv1a(&pixels),
        st.set.len(),
        transition_json(&face),
        face.dirty(),
        if c.mute { ",\"audio\":\"none\"" } else { "" } // NLEPLAYAUDIO (SR676): the witness opens no device
    );
    0
}

/// NLE4C: the witness's `"meters"` field, when the script fed them.
fn meters_json(c: &Cli, face: &Face) -> String {
    if !c.script.as_deref().unwrap_or("").split(',').any(|t| t == "meters") {
        return String::new();
    }
    let one = |m: &stria::nle::MeterLevel| {
        let track = m.track.map(|t| t.to_string()).unwrap_or_else(|| "\"master\"".into());
        format!("{{\"track\":{track},\"peak\":{:.2},\"rms\":{:.2}}}", m.peak_db, m.rms_db)
    };
    let all: Vec<String> = face.meters().levels().iter().map(one).collect();
    format!(",\"meters\":[{}]", all.join(","))
}

fn main() {
    let c = cli();
    let (tl, sources) = load(&c);
    let windowed = cfg!(any(target_os = "macos", all(target_os = "linux", feature = "gtk")));
    if c.script.is_some() || c.out.is_some() || c.two || !windowed {
        let face = route(&c, Face::new(EditorState::new(tl), sources, c.size, PixelOrder::Rgba, c.labels).with_doc(c.edl.as_deref().map(std::path::Path::new)).with_keys(c.keys.as_deref().unwrap_or("")).with_reel_paths(c.srcs.iter().map(|(r, p)| (r.clone(), p.into())).collect()));
        std::process::exit(witness(&c, face));
    }

    remember_recent(&c); let title = c.edl.as_deref().and_then(|p| std::path::Path::new(p).file_name()).and_then(|s| s.to_str()).unwrap_or("synthetic").to_string();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let synapse = Synapse::new();
    let mut engine: Option<Box<dyn std::any::Any>> = None; // NLEPLAYAUDIO (SR676): the audio engine, held for the process life
    {
        let rx = synapse.subscribe();
        let face = play_audio(&c, &mut engine, route(&c, Face::new(EditorState::new(tl), sources, c.size, PixelOrder::Rgba, true).with_doc(c.edl.as_deref().map(std::path::Path::new)).with_keys(&window_keys(&c)).with_reel_paths(c.srcs.iter().map(|(r, p)| (r.clone(), p.into())).collect()))); // STRIAKEYMAPSTORE (SR917): `--keys`, else Principia's `stria.keys` (file tail)
        rt.spawn(nle_face::serve(synapse.clone(), rx, face));
    }
    quartzite::Backend::new_tetra_vessel(
        "org.unaos.stria",
        &format!("Stria — {title}"),
        (c.size.0 as f64, c.size.1 as f64 + 40.0),
        tree(&c),
        synapse.clone(),
    )
    .run();
    drop(rt);
    drop(engine);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tree is one editor surface over a transport row, and every button's action is a verb the Face takes.
    #[test]
    fn nle2_tree_buttons_are_face_verbs() {
        let TetraNode::VStack(top) = editor_tree() else { panic!("not a VStack") };
        assert!(matches!(&top[0], TetraNode::Surface { id } if id == SURFACE));
        let TetraNode::HStack(row) = &top[1] else { panic!("no transport row") };
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(row.len(), nle_face::VERBS.len() + 1); // STRIAHELPMORE (SR977): plus the help query field
        for b in row {
            let TetraNode::Button { action, .. } = b else { assert!(matches!(b, TetraNode::TextField { id, .. } if id == nle_face::HELP_FIELD), "not a button"); continue }; // STRIAHELPMORE (SR977): the one non-button is the help field
            let SMessage::Input { target, text } = action else { panic!("not an Input") };
            assert_eq!(target, SURFACE);
            assert!(face.verb(text), "{text}");
        }
    }

    /// NLE2B M3: the witness's gesture tokens are the messages a face sends; the script's result is the ops by hand.
    #[test]
    fn nle2b_witness_gestures() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        // Select B (a click at tick 12 on V1), drag its in handle (x 80..84) right 3 ticks, release.
        let errors = drive(&mut face, "at:12:0,press:81:100,move:105:100,release");
        assert!(errors.is_empty(), "{errors:?}");
        let want = tl0.trim_in(0, 1, 3).unwrap();
        assert_eq!(face.state.timeline, want);
        assert_eq!((face.state.playhead, face.state.undo_depth()), (12, 1));
        let c = face.state.selected().unwrap();
        assert_eq!((c.at, c.src_in, c.src_out), (13, 3, 10));
        // Drag A[23,33)@25 (tick 28: x 228) 4 ticks right, Esc a second drag, L L shuttles at 2x, undo by chord.
        let errors = drive(&mut face, "press:228:100,move:260:100,release,press:260:100,move:300:100,key:Escape,release,key:l,key:l");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.timeline, want.move_clip(0, 2, 0, 29).unwrap());
        assert_eq!((face.playing, face.rate), (true, 2));
        drive(&mut face, "key:k,key:Ctrl+z,key:Home,move:160:100,wheel:1");
        assert_eq!(face.state.timeline, want);
        assert_eq!((face.playing, face.state.playhead, face.state.zoom), (false, 0, 16));
        assert_eq!(face.layout().tick_at(160.0), 20);
        assert_eq!(drive(&mut face, "press:x:1"), vec!["bad token press:x:1".to_string()]);
    }

    /// NLE2c M3: `shift:` sets MOD_SHIFT on its token's press (NLE2-MODS; the press then adds to the set), `marquee:`
    /// is a press, move and release from an empty spot, `key:Ctrl+a` is `BrowserSelectAll`; the results are the ops.
    #[test]
    fn nle2c_witness_set_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:5:0,shift:press:228:100,release,delete");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.timeline, tl0.ripple_delete_many(&[(0, 0), (0, 2)]).unwrap());
        assert_eq!((face.state.undo_depth(), face.state.set.len()), (1, 0));
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "marquee:316:85:1:155").is_empty());
        assert_eq!(face.state.set.len(), 6);
        assert!(drive(&mut face, "press:172:90,release").is_empty());
        assert_eq!(face.state.set.len(), 0, "a click on an empty spot deselects");
        assert!(drive(&mut face, "key:Ctrl+a").is_empty());
        assert_eq!(face.state.set.len(), 6);
        assert_eq!(drive(&mut face, "shift:nope"), vec!["bad token shift:nope".to_string()]);
    }

    /// NLE3d M3: `transition:T:I` clicks the widget (the transition selected), `]` / `key:[` change its length and
    /// `delete` removes it, each the timeline_core op by hand; the JSON value names it and its thumbnail.
    #[test]
    fn nle3d_witness_transition_token() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
        let mut face = Face::new(EditorState::new(tl0.clone()), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(transition_json(&face), "null");
        let errors = drive(&mut face, "transition:0:0,]");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.timeline, tl0.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 5).unwrap());
        face.frame();
        assert_eq!(transition_json(&face), r#"{"track":0,"idx":0,"at":10,"dur":5,"kind":"D","ease":"linear","from_black":false,"still":"64x48"}"#);
        assert!(drive(&mut face, "key:[,key:[,delete").is_empty());
        assert_eq!(face.state.timeline, tl0.remove_transition(0, 10).unwrap());
        assert_eq!((transition_json(&face), face.state.undo_depth()), ("null".to_string(), 4));
        assert_eq!(drive(&mut face, "transition:0:0"), vec!["bad token transition:0:0".to_string()]);
    }

    /// SURFACEID M3: with two Surfaces the editor's is number 1; a gesture on the aux Surface is not taken and leaves the
    /// timeline, the same gesture on the editor's Surface is the NLE2B trim; the default tree is unchanged.
    #[test]
    fn surfaceid_witness_two_surfaces() {
        let one = SurfaceMap::of(&editor_tree());
        assert_eq!((one.number(SURFACE), one.number(AUX_SURFACE)), (Some(0), None));
        let two = SurfaceMap::of(&editor_tree_for(true));
        assert_eq!((two.number(AUX_SURFACE), two.number(SURFACE)), (Some(0), Some(1)));
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).on_surface(1);
        let mut presses = Vec::new();
        let errors = drive_on(&mut face, "at:12:0,aux:press:81:100,aux:move:105:100,aux:release", Some((1, 0)), &mut presses);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl0, 0));
        let errors = drive_on(&mut face, "press:81:100,move:105:100,release", Some((1, 0)), &mut presses);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.timeline, tl0.trim_in(0, 1, 3).unwrap());
        assert_eq!(presses, vec![(0, false), (1, true)]);
        let c = Cli { edl: None, synth: true, fps: 25, srcs: Vec::new(), size: (320, 160), script: None, out: None, labels: false, two: true, split: false, mute: false, keys: None, bin: false };
        assert_eq!(presses_json(&c, &presses), r#","presses":[[0,false],[1,true]]"#);
        assert_eq!(presses_json(&Cli { two: false, ..c }, &presses), "");
    }

    /// NLE4C M3: the `meters` token feeds the tick at the playhead; the face then holds track 1's meter and the master's.
    #[test]
    fn nle4c_witness_meters() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "meters").is_empty());
        let lv = face.meters().levels();
        assert_eq!(lv.iter().map(|m| m.track).collect::<Vec<_>>(), vec![Some(1), None]);
        assert!((lv[1].peak_db + 13.56).abs() < 0.01, "{}", lv[1].peak_db);
    }

    /// NLEHOSTSAVE M3: `save` is the verb's Input; with no document (the synth fixture) it refuses by name and the face
    /// stays dirty after an edit; a path that cannot be written is refused naming the path.
    #[test]
    fn nlehostsave_witness_save_token() {
        let (tl, src) = nle_face::synth_fixture();
        let face0 = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
        assert!(matches!(token(&face0, "save").as_deref(), Some([SMessage::Input { text, .. }]) if text == "save"));
        let mut face = face0;
        assert!(!face.dirty() && face.doc_path().is_none());
        assert_eq!(drive(&mut face, "save"), vec!["save: no document path".to_string()]);
        assert!(drive(&mut face, "at:12:0,split").is_empty());
        assert!(face.dirty());
        assert_eq!(drive(&mut face, "save"), vec!["save: no document path".to_string()]);
        assert!(face.dirty(), "a refused save leaves the face dirty");
        let (_, src) = nle_face::synth_fixture();
        let bad = std::path::Path::new("/nonexistent-nlehostsave/x.edl");
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(bad));
        assert!(drive(&mut face, "at:12:0,split").is_empty());
        let errors = drive(&mut face, "save");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("save: /nonexistent-nlehostsave/x.edl: "), "{errors:?}");
        assert!(face.dirty() && !bad.exists());
    }
}

/// NLE3d (SR302): the witness's `transition` value: the selected transition and its thumbnail's size, or `null`.
fn transition_json(face: &Face) -> String {
    let (Some((t, i)), Some(x)) = (face.state.transition, face.state.selected_transition()) else { return "null".into() };
    let kind = match x.kind {
        timeline_core::TransitionKind::Dissolve => "D".to_string(),
        timeline_core::TransitionKind::Wipe(d) => format!("W{:03}", d.code()),
    };
    let still = face.transition_still().map(|s| format!("\"{}x{}\"", s.width, s.height)).unwrap_or_else(|| "null".into());
    format!(
        "{{\"track\":{t},\"idx\":{i},\"at\":{},\"dur\":{},\"kind\":\"{kind}\",\"ease\":\"{}\",\"from_black\":{},\"still\":{still}}}",
        x.at, x.dur, x.ease.name(), x.from_black
    )
}

/// SURFACEID (SR332): the witness's `"presses"` field, with `--two-surfaces` only (the default line is unchanged).
fn presses_json(c: &Cli, presses: &[(u32, bool)]) -> String {
    if !(c.two || c.split) {
        return String::new();
    }
    let all: Vec<String> = presses.iter().map(|(s, t)| format!("[{s},{t}]")).collect();
    format!(",\"presses\":[{}]", all.join(","))
}

// NLE2-SPLIT (LEDGER SR339): the two-Surface editor tree, the face routed over it, the witness's two fnvs.

/// THE transport row (one button per verb, each firing `SMessage::Input { target: SURFACE, text: <verb> }`), shared by
/// every editor tree.
fn transport() -> TetraNode {
    let button = |verb: &str, label: &str| TetraNode::Button {
        id: verb.into(),
        label: label.into(),
        action: SMessage::Input { target: SURFACE.into(), text: verb.into() },
    };
    TetraNode::HStack(vec![
        button("play", "Play"),
        button("prev", "<"),
        button("next", ">"),
        button("split", "Split"),
        button("delete", "Ripple Delete"),
        button("undo", "Undo"),
        button("redo", "Redo"),
        button("zoom-", "-"),
        button("zoom+", "+"),
        button("[", "In -"),
        button("]", "In +"),
        button("{", "Out -"),
        button("}", "Out +"),
        button("lift", "Lift"),
        button("size", "Size"), // NLESIZEPOLICYUI (SR402): cycles the selected clip's reel's size policy
        button("save", "Save"), // NLEHOSTSAVE (SR677): writes the timeline to the document's path
        button("export", "Export"), // NLEFACEEXPORT (SR694): exports beside the document (`<stem>-export/`, H.264), off the face's thread
        button("frame", "Frame"), // STRIAEXPORTFRAME (SR830): the playhead frame as a PNG beside the document (`<stem>-<tc>.png`)
        button("in", "Range In"), button("out", "Range Out"), // NLE5DEXPORTRANGE (SR717): the export range's marks at the playhead
        button("kind", "Kind"), button("ease", "Ease"), // NLETRANSITIONFACE (SR691): the selected transition's kind / ease cycle
        button("slip-", "Slip -"), button("slip+", "Slip +"), button("slide-", "Slide -"), button("slide+", "Slide +"), button("loudness", "LUFS"), // NLE3ESLIPFACE (SR737): one-frame slip / slide of the selected clip
        TetraNode::TextField { id: nle_face::HELP_FIELD.into(), placeholder: "find a key".into(), action: quartzite::tetra::TextAction::ConsoleInput, secret: false }, button("speed", "Speed"), // STRIACLIPSPEED (SR829): the selected clip 1x -> 2x -> 4x -> 1x (THE `speed` verb)
        button("mark", "Mark"), button("unmark", "Unmark"), // STRIAMARKERS (SR835): a named marker at the playhead (`M<n>`) / the one there removed
        button("title", "Title"), // STRIATITLECLIP (SR828): a 5 s title card at the playhead (THE `title` verb)
        button("gain-", "Gain -"), button("gain+", "Gain +"), button("gain", "0 dB"), button("fade-in", "Fade In"), button("fade-out", "Fade Out"), // STRIACLIPGAINFACE (SR838): the selected clip's gain (1 dB a press, bare `gain` unity) and fades to the playhead
        button("mute", "Mute"), button("solo", "Solo"), // STRIATRACKMUTESOLO (SR837): mute / solo the selected clip's track (help stays the last button)
        button("roll-", "Roll -"), button("roll+", "Roll +"), button("roll-tool", "Roll tool"), // STRIAROLLEDIT (SR851): one-frame roll of the cut at the selected clip's out edge; STRIAROLLMORE (SR954): the roll tool on / off
        button("insert", "Insert"), button("overwrite", "Overwrite"), // STRIAINSERTOVERWRITE (SR852): the selected clip's source range at the playhead, rippling / over its track
        button("zoom-fit", "Fit"), button("zoom-sel", "Fit Sel"), // STRIAZOOMTOFIT (SR859): fit the timeline / the selection in the window
        button("xfade", "Xfade"), button("tc", "TC"), // STRIATIMECODEDISPLAY (SR872): the playhead readout off / timecode / frames / seconds
        button("copy", "Copy"), button("cut", "Cut"), button("paste", "Paste"), // STRIACOPYPASTE (SR854): the selection onto the clipboard / lifted onto it / laid at the playhead on the selected track
        button("close-all", "Close All"), // STRIAGAPSMORE (SR969): closes every gap on every track (`close-gaps all`, one undo step)
        button("gap", "Gap"), // STRIAGAPSBLACK (SR909): opens one second of gap at the playhead on the selected clip's track (one undo step)
        button("close-gaps", "Close Gaps"), // STRIAGAPCLOSE (SR857): closes every gap on the selected clip's track (one undo step)
        button("track+ video", "V+"), button("track+ audio", "A+"), button("track-", "Track -"), // STRIATRACKADDREMOVE (SR856): add a video / audio track, remove the selected clip's track
        button("track-gain-", "Track -"), button("track-gain+", "Track +"), button("track-gain", "Track 0 dB"), // STRIATRACKGAINSTEP (SR901): one dB a press on the selected clip's track; STRIATRACKGAIN (SR864): the selected clip's track back to unity (`track-gain:<dB>` on `--do` sets it)
        button("link", "Link"), button("unlink", "Unlink"), button("unlink-all", "Unlink all"), button("link-av", "Link A/V"), button("alone", "Alone"), // STRIACLIPLINK (SR863): link / unlink the selected clips' group; STRIALINKMORE (SR967): `alone` keeps the primary without its mates
        button("info", "Info"), // STRIASOURCEINFO (SR875): the selected clip's reel line, standing five seconds on the message row
        button("track-up", "Track Up"), button("track-down", "Track Down"), // STRIATRACKREORDER (SR877): the selected clip's track swaps with its same-kind neighbour
        button("meters-reset", "Reset Meters"), button("opacity-", "Opacity -"), button("opacity+", "Opacity +"), // STRIAOPACITYFACE (SR898): the selected clips' opacity 10 % down / up (THE `opacity-` / `opacity+` verbs)
        button("paste-insert", "Paste Insert"), button("paste-over", "Paste Over"), // STRIAPASTEINSERT (SR896): the clipboard laid at the playhead rippling the track / carving what is there
        button("snap", "Snap"), // STRIASNAPTOGGLE (SR892): the drag snap default on / off (THE `snap` word)
        button("bright-", "Bright -"), button("bright+", "Bright +"), button("contrast-", "Contrast -"), button("contrast+", "Contrast +"), button("sat-", "Sat -"), button("sat+", "Sat +"), // STRIAGRADEFACE (SR899): step the selected clip's grade (timeline_core's words)
        button("normalize", "Normalize"), // STRIANORMALIZE (SR878): the selected audio clip's gain to its exact peak at -1 dBFS
        button("recover", "Recover"), // STRIAAUTOSAVE (SR883): loads the document's newer autosave (`<stem>.autosave.edl`) as one undo step
        button("history", "History"), button("freeze", "Freeze"), // STRIAFREEZEFRAME (SR880): a 2 s hold of the playhead's frame (THE `freeze` verb)
        button("freeze-all", "Freeze all"), // STRIAFREEZELEN (SR916): a 2 s hold that ripples every track (THE `freeze-all` verb)
        button("pan-", "Pan L"), button("pan", "Pan C"), button("pan+", "Pan R"), // STRIAPANFACE (SR897): step / centre the selected clip's pan
        button("crop", "Uncrop"), // STRIACROPFACE (SR900): clears the selection's crop (THE `crop` word)
        button("proxy", "Proxy"), // STRIAPROXYMEDIA (SR882): a half-size H.264 of the selected clip's source, beside it; the preview reads it, an export never
        button("pip", "PiP"), // STRIAPOSITIONSCALE (SR879): the selected clip to the bottom-right quarter (THE `pip` verb)
        button("marker-", "Marker <"), button("marker+", "Marker >"), button("cut-", "Cut <"), button("cut+", "Cut >"), // STRIAMARKERJUMP (SR894): the playhead to the previous / next marker / clip edge (view words)
        button("insert-marked", "Insert Marks"), button("overwrite-marked", "Overwrite Marks"), // STRIAINSERTMARKS (SR905): the selected clip's source into the in / out marks, rippling / over its track
        button("export-skip", "Skip Export"), button("export-clear", "Clear Queue"), // STRIAEXPORTQUEUESKIP (SR914): cancel the running export (the next queued starts) / drop the waiting requests
        button("xfade-curve", "Curve"), button("proxy-off", "Proxy Off"), button("proxy-all", "Proxy All"), // STRIAPROXYBUTTONS (SR930): detach the selected clip's source's proxy / build or reuse one for every reel with a path
        button("export-list", "Export Queue"), // STRIAEXPORTQUEUELIST (SR932): the waiting export requests on the message row
        button("scale-", "Scale -"), button("scale+", "Scale +"), // STRIAXFORMPRESETS (SR938): the selected clip's placement scale stepped about its centre
        button("preset", "Preset"), // STRIAEXPORTPRESETSMORE (SR948): steps the export preset a bare `export` takes (THE `preset` word)
        button("unmark all", "Unmark All"), // STRIAMARKERSMORE (SR966): THE `unmark all` word
        button("help", "?"), // STRIAUNDOHISTORY (SR876): the undo history on the message line; NLE5DKEYSHELPBUTTON (SR765): the keys help overlay on / off (THE `help` verb, like `?`); STRIAAUDIOCROSSFADE (SR860): Xfade puts / takes the selected audio clip's head crossfade; STRIAXFADECURVEBUTTON (SR925): Curve swaps its curve linear / equal; STRIAHELPMORE (SR977): the help query field before it
    ])
}

/// NLE2-SPLIT (SR339): the preview Surface (number 0) over the transport row, the tracks Surface (number 1) under it.
fn split_tree() -> TetraNode {
    TetraNode::VStack(vec![TetraNode::Surface { id: PREVIEW_SURFACE.into() }, transport(), TetraNode::Surface { id: TRACKS_SURFACE.into() }])
}

/// The tree this run declares: `--split`'s, else the editor tree (with `--two-surfaces` its aux Surface).
fn tree(c: &Cli) -> TetraNode {
    if c.split { bin_tree(c, split_tree()) } else if c.two { editor_tree_for(true) } else { editor_tree() }
}

/// The face routed by the tree's `SurfaceMap`: split over the preview and tracks Surfaces, else on the editor's.
fn route(c: &Cli, face: Face) -> Face {
    let map = SurfaceMap::of(&tree(c));
    if c.split {
        bin_route(c, &map, face.on_surfaces(map.number(PREVIEW_SURFACE).unwrap_or(0), map.number(TRACKS_SURFACE).unwrap_or(1))) // STRIAPROJECTBIN (SR885): and the bin's Surface with --bin (file tail)
    } else {
        face.on_surface(map.number(SURFACE).unwrap_or(0))
    }
}

/// The witness's `"surfaces"` / `"fnvs"` fields, with `--split` only (the default line is unchanged).
fn blits_json(c: &Cli, blits: &[SMessage]) -> String {
    if !c.split {
        return String::new();
    }
    let fnvs: Vec<String> = blits
        .iter()
        .filter_map(|m| match m {
            SMessage::SurfaceBlit { pixels, .. } => Some(format!("\"{:016x}\"", nle_face::fnv1a(pixels))),
            _ => None,
        })
        .collect();
    format!(",\"surfaces\":{},\"fnvs\":[{}]", fnvs.len(), fnvs.join(","))
}

#[cfg(test)]
mod split_tests {
    use super::*;

    /// NLE2-SPLIT M3: the split tree numbers the preview 0 and the tracks 1 and carries the one transport row; a gesture
    /// on the preview Surface is not taken, the same on the tracks Surface is the NLE2B trim; the witness's fields.
    #[test]
    fn split_witness_two_surfaces() {
        let m = SurfaceMap::of(&split_tree());
        assert_eq!((m.number(PREVIEW_SURFACE), m.number(TRACKS_SURFACE), m.number(SURFACE)), (Some(0), Some(1), None));
        let TetraNode::VStack(top) = split_tree() else { panic!("not a VStack") };
        let TetraNode::VStack(one) = editor_tree() else { panic!("not a VStack") };
        assert_eq!(format!("{:?}", top[1]), format!("{:?}", one[1]), "the one transport row");
        let c = Cli { edl: None, synth: true, fps: 25, srcs: Vec::new(), size: (320, 160), script: None, out: None, labels: false, two: false, split: true, mute: false, keys: None, bin: false };
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = route(&c, Face::new(EditorState::new(tl), src, c.size, PixelOrder::Rgba, false));
        assert_eq!(face.canvas, (320, 96));
        let mut presses = Vec::new();
        let errors = drive_on(&mut face, "at:12:0,aux:press:81:36,aux:move:105:36,aux:release", Some((1, 0)), &mut presses);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl0, 0));
        let errors = drive_on(&mut face, "press:81:36,move:105:36,release", Some((1, 0)), &mut presses);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.timeline, tl0.trim_in(0, 1, 3).unwrap());
        assert_eq!(presses_json(&c, &presses), r#","presses":[[0,false],[1,true]]"#);
        let j = blits_json(&c, &face.frames());
        assert!(j.starts_with(r#","surfaces":2,"fnvs":[""#) && j.len() == r#","surfaces":2,"fnvs":["","#.len() + 2 * 16 + 3, "{j}");
        assert_eq!(blits_json(&Cli { split: false, ..c }, &face.frames()), "");
    }
}

// ---- NLEPLAYAUDIO (LEDGER SR676): the window's audio device (named tail block) ----

/// The editor's audio rate: the device's and the render's (`Face::with_audio` sets the Sources' rate to it).
const AUDIO_HZ: u32 = 48_000;

/// The window's face with sound: `stria::media::resonance_out` at [`AUDIO_HZ`] installed by `Face::with_audio`, its engine
/// held in `engine` (no resonance dependency here); `--mute` or no device leaves the face silent and says why.
fn play_audio(c: &Cli, engine: &mut Option<Box<dyn std::any::Any>>, face: Face) -> Face {
    if c.mute {
        eprintln!("[STRIA] :: nle audio=none why=mute");
        return face;
    }
    match stria::media::resonance_out(AUDIO_HZ) {
        Some((e, feed)) => {
            *engine = Some(Box::new(e));
            eprintln!("[STRIA] :: nle audio={AUDIO_HZ}");
            face.with_audio(Box::new(feed), AUDIO_HZ)
        }
        None => {
            eprintln!("[STRIA] :: nle audio=none why=no-device");
            face
        }
    }
}

// ---- NLEFACEEXPORT (LEDGER SR694): the witness's export reading (named tail block) ----
/// The witness's `"export"` field, only while an export runs (`Face::export_progress`); the default line is unchanged.
fn export_json(face: &Face) -> String {
    face.export_progress().map(|(d, t)| format!(",\"export\":{{\"done\":{d},\"total\":{t},\"audio\":\"{}\"{}}}", face.export_audio().unwrap_or_default(), face.export_eta().map(|e| format!(",\"eta_secs\":{}", nle_face::eta_secs(e))).unwrap_or_default() + &face.export_stall().map(|s| format!(",\"stalled_secs\":{}", nle_face::eta_secs(s))).unwrap_or_default())).unwrap_or_default() // NLE5DEXPORTMESSAGEAUDIO (SR756): the running job's audio word; NLE5DEXPORTMESSAGEETA (SR796): eta_secs while known; NLE5DEXPORTMESSAGEETASTALLWITNESS (SR825): stalled_secs while stalled
}

// ---- NLE5DEXPORTRANGE (LEDGER SR717): the witness's marked range (named tail block) ----
/// The witness's `"range"` field, only when both marks are set (`Face::marks`); the default line is unchanged.
fn range_json(face: &Face) -> String {
    match face.marks() {
        (Some(a), Some(b)) => format!(",\"range\":[{a},{b}]"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod exportrange_tests {
    use super::*;

    /// NLE5DEXPORTRANGE (SR717): `in` / `out` ride the verb road; the ruler-seek script sets the marks and the witness line
    /// carries them; a face with no marks (or one) adds nothing.
    #[test]
    fn nle5dexportrange_witness_range_field() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(matches!(token(&face, "in").as_deref(), Some([SMessage::Input { text, .. }]) if text == "in"));
        assert!(matches!(token(&face, "out").as_deref(), Some([SMessage::Input { text, .. }]) if text == "out"));
        assert_eq!(range_json(&face), "");
        assert!(drive(&mut face, "ruler:5,in").is_empty());
        assert_eq!((face.marks(), range_json(&face)), ((Some(5), None), String::new()));
        assert!(drive(&mut face, "ruler:15,out").is_empty());
        assert_eq!(range_json(&face), r#","range":[5,15]"#);
    }
}

// ---- STRIAINFOREEL (LEDGER SR743): the witness's selected reel (named tail block) ----
/// The witness's `"reel"` field, only while a clip is selected: the face's message idle text (`Face::reel_line`,
/// `<reel>: <codec> <W>x<H> <profile> <bit rate>`); the default line is unchanged.
fn reel_json(face: &Face) -> String {
    face.reel_line().map(|s| format!(",\"reel\":\"{}\"", s.replace('"', "'"))).unwrap_or_default()
}

#[cfg(test)]
mod striainforeel_tests {
    use super::*;

    /// STRIAINFOREEL M3: `at:12:0` selects reel B's clip on V1 and the witness's field names it; with nothing selected it is
    /// absent (the default line unchanged).
    #[test]
    fn reel_field_only_while_a_clip_is_selected() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(reel_json(&face), "");
        let errors = drive(&mut face, "at:12:0");
        assert!(errors.is_empty(), "{errors:?}");
        let j = reel_json(&face);
        assert!(j.starts_with(",\"reel\":\"B: utp1 64x48 ~") && j.ends_with(" kb/s\""), "{j}");
        println!("striainforeel witness: {j}");
    }
}

// ---- NLETRANSITIONFACE (LEDGER SR691): the Kind / Ease buttons, the `kind:` / `ease:` tokens and the JSON `ease` (named tail block) ----

#[cfg(test)]
mod transitionface_tests {
    use super::*;

    /// M3: `transition:T:I` selects the dissolve, `kind:W007` / `ease:inout` are timeline_core's verbs with their argument,
    /// bare `kind` / `ease` (the buttons' words) cycle; the JSON value names the kind and the ease.
    #[test]
    fn nletransitionface_witness_kind_and_ease() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
        let mut face = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "transition:0:0,kind:W007,ease:inout");
        assert!(errors.is_empty(), "{errors:?}");
        let json = transition_json(&face);
        assert!(json.contains(r#""kind":"W007","ease":"inout""#), "{json}");
        assert!(drive(&mut face, "kind,ease").is_empty());
        let json = transition_json(&face);
        assert!(json.contains(r#""kind":"W021","ease":"linear""#), "{json}");
        assert_eq!(face.state.undo_depth(), 4);
        assert_eq!(drive(&mut face, "kind:W008"), vec![format!("kind W008: {}", timeline_core::EditError::Transition)]);
    }
}

// ---- NLE5DKEYSHELP (LEDGER SR744): the witness's keys help reading (named tail block) ----
/// The witness's `"help"` field, only while the keys help shows (`Face::help_shown`); the default line is unchanged.
fn help_json(face: &Face) -> String {
    if face.help_shown() { ",\"help\":true".into() } else { String::new() }
}

#[cfg(test)]
mod nle5dkeyshelp_tests {
    use super::*;

    fn face() -> Face {
        let (tl, src) = nle_face::synth_fixture();
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
    }

    /// NLE5DKEYSHELP M3: `--do help` shows the help and the line carries `"help":true`; a second `help` hides it (the field
    /// absent); `key:?` shows it; a following `key:End` only dismisses it (no seek: not playing, rate 1).
    #[test]
    fn help_field_while_the_help_shows() {
        let mut f = face();
        assert_eq!(help_json(&f), "");
        let errors = drive(&mut f, "help");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(help_json(&f), ",\"help\":true");
        drive(&mut f, "help");
        assert_eq!(help_json(&f), "");
        let errors = drive(&mut f, "key:?");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(f.help_shown());
        let errors = drive(&mut f, "key:End");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((f.help_shown(), f.playing, f.rate), (false, false, 1));
        println!("nle5dkeyshelp witness: help -> ,\"help\":true; key:? shows; key:j dismisses unapplied");
    }
}

// ---- NLE5DKEYSHELPBUTTON (LEDGER SR765): the `?` transport button — the keys help word on the bar (named tail block) ----

#[cfg(test)]
mod nle5dkeyshelpbutton_tests {
    use super::*;

    /// NLE5DKEYSHELPBUTTON M3: the transport row's LAST button is `help` labelled `?`, firing the face's `help` word; driving
    /// that word (the `--do` road the click takes) carries `"help":true` on the line, a second press clears it.
    #[test]
    fn help_button_is_the_last_and_toggles_the_help() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let Some(TetraNode::Button { id, label, action }) = row.last() else { panic!("no last button") };
        assert_eq!((id.as_str(), label.as_str()), ("help", "?"));
        let SMessage::Input { target, text } = action else { panic!("not an Input") };
        assert_eq!((target.as_str(), text.as_str()), (SURFACE, "help"));
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(help_json(&f), "");
        let errors = drive(&mut f, text);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(help_json(&f), ",\"help\":true");
        let errors = drive(&mut f, text);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(help_json(&f), "");
        println!("nle5dkeyshelpbutton witness: last button {id} {label:?} -> ,\"help\":true, again -> cleared");
    }
}

// ---- NLE5DEXPORTMESSAGEAUDIO (LEDGER SR756): the witness's export object names its audio (named tail block) ----
#[cfg(test)]
mod exportaudio_tests {
    use super::*;

    /// NLE5DEXPORTMESSAGEAUDIO (SR756): while an export runs the `"export"` object carries `"audio"` (the paused seam,
    /// `Face::export_paused_at`, exports `png` with the default `wav`); once it is cancelled and joined the field is gone.
    #[test]
    fn nle5dexportmessageaudio_witness_export_audio_field() {
        let exe = std::env::current_exe().expect("the test binary");
        let dir = exe.parent().expect("its directory").join("nle5dexportmessageaudio-witness");
        let _ = std::fs::remove_dir_all(&dir);
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        face.export_paused_at(&dir, "png", 3).unwrap();
        let t0 = std::time::Instant::now();
        while face.export_progress() != Some((3, 35)) {
            assert!(t0.elapsed() < std::time::Duration::from_secs(60), "reading {:?}", face.export_progress());
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let j = export_json(&face); let (head, eta) = j.split_once(",\"eta_secs\":").expect("eta while it runs"); assert_eq!(head, r#","export":{"done":3,"total":35,"audio":"wav""#); assert!(eta.strip_suffix('}').and_then(|t| t.split(',').next()).is_some_and(|n| n.parse::<u64>().is_ok()), "{j}"); // NLE5DEXPORTMESSAGEETA (SR796): the eta tail is timing-dependent; the head stays byte-exact; NLE5DEXPORTMESSAGEETASTALLWITNESS (SR825): eta_secs read by name (an optional stalled_secs after it)
        assert!(face.cancel_export());
        assert!(face.wait_export());
        assert_eq!(export_json(&face), "");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---- STRIAINFOREELHOVERDELAY (LEDGER SR789): the witness's tick:N token (named tail block) ----
/// `tick:<n>`: [`Face::tick`] n times (the face clock `serve` runs: playback steps and the hover dwell), no messages; any
/// other token is `None` (so `tick:x` stays a bad token).
fn tick_token(face: &mut Face, tok: &str) -> Option<Vec<SMessage>> {
    let n = tok.strip_prefix("tick:")?.parse::<u32>().ok()?;
    for _ in 0..n {
        face.tick(); face.autosave_tick(); // STRIAAUTOSAVECAP (SR913): serve's clock arm also ticks the autosave (file tail)
    }
    Some(Vec::new())
}

#[cfg(test)]
mod striainforeelhoverdelay_tests {
    use super::*;

    /// STRIAINFOREELHOVERDELAY M3: with B selected, `move:` over A then `tick:<HOVER_DWELL - 1>` still carries B's reel; one
    /// more `tick:1` carries A's; `tick:x` is a bad token.
    #[test]
    fn tick_token_carries_the_dwelt_hover() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "at:12:0").is_empty());
        let (x, y) = face.layout().point(0, 5);
        let errors = drive(&mut face, &format!("move:{x}:{y},tick:{}", nle_face::HOVER_DWELL - 1));
        assert!(errors.is_empty(), "{errors:?}");
        assert!(reel_json(&face).starts_with(",\"reel\":\"B: "), "{}", reel_json(&face));
        assert!(drive(&mut face, "tick:1").is_empty());
        assert!(reel_json(&face).starts_with(",\"reel\":\"A: "), "{}", reel_json(&face));
        assert_eq!(drive(&mut face, "tick:x"), vec!["bad token tick:x".to_string()]);
        println!("striainforeelhoverdelay witness: move over A + tick:{} -> A's reel", nle_face::HOVER_DWELL);
    }
}

// ---- NLE5DEXPORTMESSAGEETA (LEDGER SR796): the witness's export object carries the remaining time while known (named tail block) ----
#[cfg(test)]
mod exporteta_tests {
    use super::*;

    /// NLE5DEXPORTMESSAGEETA (SR796): a job paused at frame 3 of 35 (`Face::export_paused_at`) carries `"eta_secs":<n>` after
    /// `"audio"`, the head byte-exact and `n` the face's own reading through `nle_face::eta_secs`; once it is cancelled and
    /// joined the field is gone with the object.
    #[test]
    fn nle5dexportmessageeta_witness_eta_secs_field() {
        let exe = std::env::current_exe().expect("the test binary");
        let dir = exe.parent().expect("its directory").join("nle5dexportmessageeta-witness");
        let _ = std::fs::remove_dir_all(&dir);
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        face.export_paused_at(&dir, "png", 3).unwrap();
        let t0 = std::time::Instant::now();
        while face.export_progress() != Some((3, 35)) {
            assert!(t0.elapsed() < std::time::Duration::from_secs(60), "reading {:?}", face.export_progress());
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        let j = export_json(&face);
        let (head, eta) = j.split_once(",\"eta_secs\":").unwrap_or_else(|| panic!("no eta_secs in {j}"));
        assert_eq!(head, r#","export":{"done":3,"total":35,"audio":"wav""#);
        let n: u64 = eta.strip_suffix('}').and_then(|t| t.split(',').next()).and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("{j}")); // NLE5DEXPORTMESSAGEETASTALLWITNESS (SR825): eta_secs read by name (an optional stalled_secs after it)
        assert!(face.export_eta().is_some_and(|e| nle_face::eta_secs(e) >= n), "{j}: the paused eta only grows");
        assert!(face.cancel_export());
        assert!(face.wait_export());
        assert_eq!(export_json(&face), "");
        println!("nle5dexportmessageeta witness: {j}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- STRIAPAN (LEDGER SR861): the `pan:<x>` token (named tail block) ----
    /// `--do at:12:1,pan:-0.5` selects the audio clip and pans it through the face's `pan <x>` word (timeline_core's verb).
    #[test]
    fn striapan_do_token_pans_the_selected_clip() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:1,pan:-0.5");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.selection, Some((1, 1)));
        assert_eq!(face.state.timeline.tracks[1].clips[1].pan_cq8, -64);
        println!("striapan witness: pan:-0.5 -> pan_cq8 {}", face.state.timeline.tracks[1].clips[1].pan_cq8);
    }
}

// ---- NLE5DEXPORTMESSAGEETASTALLWITNESS (LEDGER SR825): the witness's export object carries the stall seconds while stalled (named tail block) ----
#[cfg(test)]
mod exportstall_tests {
    use super::*;

    /// NLE5DEXPORTMESSAGEETASTALLWITNESS (SR825): a job paused at frame 3 of 35 carries no `"stalled_secs"` while fresh; past
    /// SR817's flat 5 s (the mean road: no tick, so the time counts from the start) it carries `"stalled_secs":<n>`, n >= 5,
    /// after `"eta_secs"` (`Face::export_stall` through `nle_face::eta_secs`); once cancelled and joined the object is gone.
    #[test]
    fn nle5dexportmessageetastallwitness_witness_stalled_secs_field() {
        use std::time::{Duration, Instant};
        let exe = std::env::current_exe().expect("the test binary");
        let dir = exe.parent().expect("its directory").join("nle5dexportmessageetastallwitness-witness");
        let _ = std::fs::remove_dir_all(&dir);
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let start = Instant::now();
        face.export_paused_at(&dir, "png", 3).unwrap();
        while face.export_progress() != Some((3, 35)) {
            assert!(start.elapsed() < Duration::from_secs(60), "reading {:?}", face.export_progress());
            std::thread::sleep(Duration::from_millis(1));
        }
        let fresh = export_json(&face);
        if start.elapsed() < Duration::from_secs(4) {
            assert!(!fresh.contains("stalled_secs"), "{fresh}: no stall before the flat 5 s");
        }
        let j = loop {
            let j = export_json(&face);
            if j.contains(",\"stalled_secs\":") {
                break j;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "the paused job never carried stalled_secs: {j}");
            std::thread::sleep(Duration::from_millis(10));
        };
        let (head, stall) = j.split_once(",\"stalled_secs\":").expect("the stall key");
        assert!(head.starts_with(r#","export":{"done":3,"total":35,"audio":"wav""#) && head.contains(",\"eta_secs\":"), "{j}: the key after eta_secs");
        let n: u64 = stall.strip_suffix('}').and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("{j}"));
        assert!(n >= 5, "{j}: the stall in whole seconds, 5 or more");
        assert!(face.cancel_export());
        assert!(face.wait_export());
        assert_eq!(export_json(&face), "");
        println!("nle5dexportmessageetastallwitness witness: {j}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---- STRIAEXPORTFRAME (LEDGER SR830): the witness's saved frame (named tail block) ----
/// The witness's `"frame"` field: the file the last `frame` verb wrote (`Face::frame_saved`); absent on a fresh face or after
/// a refused `frame` (the default line unchanged).
fn frame_json(face: &Face) -> String {
    face.frame_saved().map(|p| format!(",\"frame\":\"{}\"", p.display().to_string().replace('"', "'"))).unwrap_or_default()
}

#[cfg(test)]
mod striaexportframe_tests {
    use super::*;

    /// STRIAEXPORTFRAME M3: the Frame button sits after Export and fires the face's `frame` word; on a face opened from
    /// `<dir>/reel.edl` it writes `reel-00-00-00-00.png` beside it and the witness carries the path; a fresh face carries
    /// nothing; the `--do frame` road reads `frame: <name>` on the message; no document is refused by name.
    #[test]
    fn striaexportframe_button_writes_and_the_witness_names_it() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let at = row.iter().position(|b| matches!(b, TetraNode::Button { id, .. } if id == "frame")).expect("a frame button");
        assert!(matches!(&row[at - 1], TetraNode::Button { id, .. } if id == "export"), "beside Export");
        let TetraNode::Button { label, action, .. } = &row[at] else { panic!("not a button") };
        assert_eq!(label, "Frame");
        assert!(matches!(action, SMessage::Input { target, text } if target == SURFACE && text == "frame"));
        let exe = std::env::current_exe().expect("the test binary");
        let dir = exe.parent().expect("its directory").join("striaexportframe-witness");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("reel.edl");
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(&doc));
        assert_eq!(frame_json(&face), "");
        assert!(face.on(action));
        let png = dir.join("reel-00-00-00-00.png");
        assert!(png.exists(), "{}", png.display());
        assert_eq!(face.frame_message(), Some("frame: reel-00-00-00-00.png"));
        assert_eq!(frame_json(&face), format!(",\"frame\":\"{}\"", png.display()));
        std::fs::remove_file(&png).unwrap();
        assert_eq!(drive(&mut face, "frame"), vec!["frame: reel-00-00-00-00.png".to_string()]);
        assert!(png.exists());
        assert_eq!(frame_json(&face), format!(",\"frame\":\"{}\"", png.display()), "the key outlives the taken message");
        let (tl, src) = nle_face::synth_fixture();
        let mut bare = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(drive(&mut bare, "frame"), vec!["frame: no file (no document path)".to_string()]);
        assert_eq!(frame_json(&bare), "");
        println!("striaexportframe witness: {}", frame_json(&face));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---- STRIAMARKERS (LEDGER SR835): the witness's markers (named tail block) ----
/// The witness's `"markers"` field: every marker as `{"t":<tick>,"name":"<name>"}` in tick order, only when the timeline
/// holds one (a `"` in a name shown `'`, `reel_json`'s rule); the default line is unchanged.
fn markers_json(face: &Face) -> String {
    let m = &face.state.timeline.markers;
    if m.is_empty() {
        return String::new();
    }
    let items: Vec<String> = m.iter().map(|k| format!("{{\"t\":{},\"name\":\"{}\"}}", k.at, k.name.replace('"', "'"))).collect();
    format!(",\"markers\":[{}]", items.join(","))
}

#[cfg(test)]
mod striamarkers_tests {
    use super::*;

    /// STRIAMARKERS M3: the Mark / Unmark buttons sit before Title (help stays last) and fire the face's words; the
    /// witness carries no `"markers"` on a fresh face, `[{"t":0,"name":"M1"}]` after `mark`, nothing again after `unmark`.
    #[test]
    fn striamarkers_witness_markers_field() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let at = row.iter().position(|b| matches!(b, TetraNode::Button { id, .. } if id == "mark")).expect("a mark button");
        assert!(matches!(&row[at + 1], TetraNode::Button { id, .. } if id == "unmark"));
        assert!(matches!(&row[at + 2], TetraNode::Button { id, .. } if id == "title"));
        assert!(matches!(row.last(), Some(TetraNode::Button { id, .. }) if id == "help"));
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(markers_json(&face), "");
        assert_eq!(drive(&mut face, "mark"), vec!["marked M1 at 00:00:00:00".to_string()]);
        assert_eq!(markers_json(&face), r#","markers":[{"t":0,"name":"M1"}]"#);
        assert_eq!(drive(&mut face, "ruler:12,mark"), vec!["marked M2 at 00:00:00:12".to_string()]);
        assert_eq!(markers_json(&face), r#","markers":[{"t":0,"name":"M1"},{"t":12,"name":"M2"}]"#);
        println!("striamarkers witness: {}", markers_json(&face));
        assert_eq!(drive(&mut face, "unmark,ruler:0,unmark"), vec!["unmarked M2 at 00:00:00:12".to_string(), "unmarked M1 at 00:00:00:00".to_string()]);
        assert_eq!(markers_json(&face), "");
    }
}

// ---- STRIAINSERTOVERWRITE (LEDGER SR852): the insert / overwrite buttons and the `--do` argument token (named tail block) ----

#[cfg(test)]
mod striainsertoverwrite_tests {
    use super::*;
    use timeline_core::{Clip, SourceId};

    fn face() -> Face {
        let (tl, src) = nle_face::synth_fixture();
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
    }

    /// STRIAINSERTOVERWRITE M3: the two buttons sit just before `help`; `--do at:12:0,ruler:15,insert` lays the selected
    /// B 0..10 at 15 on V1 (the witness's `"clip"` reads `15:0:10`), A1 untouched; `insert:A:13:23` is the face's word.
    #[test]
    fn striainsertoverwrite_buttons_and_token() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let ids: Vec<String> = row.iter().map(|b| match b { TetraNode::Button { id, .. } => id.clone(), _ => panic!("not a button") }).collect();
        assert_eq!(ids[ids.len() - 5..], ["insert", "overwrite", "insert-marked", "overwrite-marked", "help"]); // STRIAINSERTMARKS (SR905)
        let mut f = face();
        let a1 = f.state.timeline.tracks[1].clone();
        let errors = drive(&mut f, "at:12:0,ruler:15,insert");
        assert!(errors.is_empty(), "{errors:?}");
        let c = f.state.selected().expect("the laid clip selected");
        assert_eq!(format!("{}:{}:{}", c.at, c.src_in, c.src_out), "15:0:10");
        assert_eq!((c.src, f.state.playhead, f.state.timeline.tracks[0].clips.iter().map(|c| c.end()).max()), (SourceId(1), 25, Some(45)));
        assert_eq!(f.state.timeline.tracks[1], a1, "insert ripples only the target track");
        assert!(matches!(token(&f, "insert:A:13:23").as_deref(), Some([SMessage::Input { text, .. }]) if text == "insert A 13 23"));
        let mut g = face();
        let errors = drive(&mut g, "ruler:5,overwrite:A:40:45");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(g.state.timeline.tracks[0].clips[1], Clip::new(SourceId(0), 40, 45, 5));
        println!("striainsertoverwrite witness: at:12:0,ruler:15,insert -> clip 15:0:10, V1 end 45; overwrite:A:40:45 at 5");
    }
}

// ---- STRIATRANSITIONLENGTH (LEDGER SR870): the `dur:<n>` token (named tail block) ----

#[cfg(test)]
mod striatransitionlength_tests {
    use super::*;

    /// M3: `transition:0:0,dur:7` gives the selected dissolve 7 ticks (the JSON `"dur":7`, one undo step); `dur:99` runs
    /// past the incoming clip's handle and is refused by name, the state left.
    #[test]
    fn striatransitionlength_witness_dur_token() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
        let mut face = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "transition:0:0,dur:7");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(transition_json(&face).contains(r#""at":10,"dur":7,"#), "{}", transition_json(&face));
        assert_eq!(face.state.undo_depth(), 1);
        let before = face.state.clone();
        assert_eq!(drive(&mut face, "dur:99"), vec![format!("dur 99: {}", timeline_core::EditError::Handle)]);
        assert_eq!(face.state, before);
    }
}

// ---- STRIATIMECODEDISPLAY (LEDGER SR872): the TC button and the witness's `"tc"` reading (named tail block) ----
/// The witness's `"tc"` field: the playhead readout's unit while it is on (`Face::tc_unit`); the default line is unchanged.
fn tc_json(face: &Face) -> String {
    face.tc_unit().map(|u| format!(",\"tc\":\"{}\"", u.word())).unwrap_or_default()
}

#[cfg(test)]
mod striatimecodedisplay_tests {
    use super::*;

    /// The second-to-last button is `tc` labelled `TC` firing the face's word; `--do tc` carries `"tc":"timecode"`, the next
    /// presses `frames` / `seconds`, the fourth clears the field (the default line has none).
    #[test]
    fn striatimecodedisplay_button_and_json() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let Some(TetraNode::Button { id, label, action }) = row.iter().rev().nth(1) else { panic!("no button") };
        assert_eq!((id.as_str(), label.as_str()), ("tc", "TC"));
        let SMessage::Input { target, text } = action else { panic!("not an Input") };
        assert_eq!((target.as_str(), text.as_str()), (SURFACE, "tc"));
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(tc_json(&f), "");
        let mut seen = Vec::new();
        for _ in 0..4 {
            let errors = drive(&mut f, text);
            assert!(errors.is_empty(), "{errors:?}");
            seen.push(tc_json(&f));
        }
        assert_eq!(seen, [",\"tc\":\"timecode\"", ",\"tc\":\"frames\"", ",\"tc\":\"seconds\"", ""]);
        assert_eq!((f.state.undo_depth(), f.dirty()), (0, false));
        println!("striatimecodedisplay witness: tc x4 -> {seen:?}");
    }
}

// ---- STRIACOPYPASTE (LEDGER SR854): the copy / cut / paste buttons and `--do` words (named tail block) ----

#[cfg(test)]
mod striacopypaste_tests {
    use super::*;

    /// `--do at:12:0,copy,ruler:35,paste`: B copied off V1, laid at the ruler's tick on V1 (the selected track), one undo
    /// step; the three buttons sit before the help button.
    #[test]
    fn striacopypaste_witness_copy_paste_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,copy,ruler:35,paste");
        assert!(errors.is_empty(), "{errors:?}");
        let v1 = &face.state.timeline.tracks[0].clips;
        assert_eq!(v1.last().map(|c| (c.src_in, c.src_out, c.at)), Some((0, 10, 35)));
        assert_eq!(face.state.timeline.tracks[1], tl0.tracks[1]);
        assert_eq!((face.state.undo_depth(), face.last_error.clone()), (1, None));
        let TetraNode::VStack(top) = editor_tree() else { panic!("not a VStack") };
        let TetraNode::HStack(row) = &top[1] else { panic!("no transport row") };
        let words: Vec<String> = row.iter().filter_map(|b| match b { TetraNode::Button { action: SMessage::Input { text, .. }, .. } => Some(text.clone()), _ => None }).collect();
        assert!(["copy", "cut", "paste"].iter().all(|w| words.iter().any(|x| x == w)) && words.last().map(String::as_str) == Some("help"));
        println!("striacopypaste witness: V1 {:?}", v1.last());
    }
}

// ---- STRIAGAPCLOSE (LEDGER SR857): the witness's close-gaps button word and `gap:<n>` token (named tail block) ----
#[cfg(test)]
mod striagapclose_tests {
    use super::*;

    /// `close-gaps` (the Close Gaps button's word) closes V1's 20..25 gap; `gap:5` opens 5 ticks at the playhead (inside B:
    /// a split), each the timeline_core op by hand; the button is in the transport row before help.
    #[test]
    fn striagapclose_witness_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "at:12:0,close-gaps").is_empty());
        let closed = tl0.close_gaps(0).unwrap();
        assert_eq!(face.state.timeline, closed);
        assert_eq!(face.state.timeline.tracks[0].clips[2].at, 20);
        assert!(drive(&mut face, "gap:5").is_empty());
        assert_eq!(face.state.timeline, closed.insert_gap(0, 12, 5).unwrap());
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let ids: Vec<String> = row.iter().filter_map(|b| if let TetraNode::Button { id, .. } = b { Some(id.clone()) } else { None }).collect();
        assert_eq!(&ids[ids.len() - 2..], ["close-gaps".to_string(), "help".to_string()]);
        println!("striagapclose witness: close-gaps -> {:?}", face.state.timeline.tracks[0].clips);
    }
}

// ---- STRIAEXPORTPRESETS (LEDGER SR871): the witness lists the export presets (named tail block) ----
/// `stria-view --presets`: one line per [`stria::nle_export::PRESETS`] row, its own words ([`stria::nle_export::ExportPreset::words`]),
/// the names the face's `export [<dir>] --preset <name>` takes.
fn presets_text() -> String {
    stria::nle_export::PRESETS.iter().map(|p| p.words() + "\n").collect()
}

#[cfg(test)]
mod striaexportpresets_tests {
    use super::*;

    /// The listing is the table: one line per preset, in table order, each naming its video and audio words.
    #[test]
    fn striaexportpresets_lists_the_table() {
        let t = presets_text();
        assert_eq!(t, "web: h264 + aac crf=23 aq=10 scenecut=40\narchive: png + wav\nproxy: h264 + aac:64 crf=32\n");
        assert_eq!(t.lines().count(), stria::nle_export::PRESETS.len());
        assert!(USAGE.contains("[--presets]"), "{USAGE}");
    }
}

// ---- STRIATRACKADDREMOVE (LEDGER SR856): the V+ / A+ / Track - buttons and the `track+:` / `track-:` tokens (named tail block) ----

#[cfg(test)]
mod striatrackaddremove_tests {
    use super::*;

    /// M3: `track+:audio` adds A2 (the JSON `"tracks"` reads 3), `track-:A2` removes it (2) and names it on the message
    /// line (the witness's `errors`), two undo steps; the three buttons' words are VERBS words and sit before `help`, still the last button.
    #[test]
    fn striatrackaddremove_witness_tokens_and_buttons() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "track+:audio").is_empty());
        assert_eq!(face.state.timeline.tracks.len(), 3);
        assert!(face.dirty());
        assert_eq!((drive(&mut face, "track-:A2"), face.note.as_deref()), (Vec::<String>::new(), Some("track-: removed A2 (0 clips)")), "the note road");
        assert_eq!((face.state.timeline.tracks.len(), face.state.undo_depth()), (2, 2));
        assert_eq!(face.state.timeline, tl);
        assert_eq!(drive(&mut face, "track-:V9"), vec![format!("track- V9: {}", timeline_core::EditError::NoTrack)]);
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let ids: Vec<&str> = row.iter().map(|b| if let TetraNode::Button { id, .. } = b { id.as_str() } else { "" }).collect();
        assert_eq!(ids[ids.len() - 4..], ["track+ video", "track+ audio", "track-", "help"]);
        assert!(["track+ video", "track+ audio", "track-"].iter().all(|w| nle_face::VERBS.contains(w)));
    }
}

// ---- STRIATRACKGAIN (LEDGER SR864): the `track-gain:<dB>` token and the Track 0 dB button (named tail block) ----

#[cfg(test)]
mod striatrackgain_tests {
    use super::*;

    /// M3: `at:12:1,track-gain:-6` selects the A1 clip and sets A1's gain (one undo step, the clip's own gain untouched);
    /// the button's bare word puts it back to unity; with nothing selected the word is refused by name on the message road.
    #[test]
    fn striatrackgain_witness_token_and_button() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        assert!(row.iter().any(|b| matches!(b, TetraNode::Button { id, .. } if id == "track-gain")));
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:1,track-gain:-6");
        assert_eq!(errors, vec!["track-gain: A1 -6.0 dB".to_string()], "STRIATRACKGAINSTEP (SR901): the success line");
        assert_eq!((face.state.timeline.tracks[1].gain_cdb, face.state.timeline.tracks[0].gain_cdb, face.state.undo_depth()), (-600, 0, 1));
        assert!(face.state.timeline.tracks[1].clips.iter().all(|c| c.gain_cdb == 0));
        assert_eq!(drive(&mut face, "track-gain"), vec!["track-gain: A1 0.0 dB".to_string()]); // STRIATRACKGAINSTEP (SR901): the success line
        assert_eq!((face.state.timeline.tracks[1].gain_cdb, face.state.undo_depth()), (0, 2));
        let (tl, src) = nle_face::synth_fixture();
        let mut none = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(drive(&mut none, "track-gain:-6"), vec![format!("track-gain -6: {}", timeline_core::EditError::NoTrack)]);
        println!("striatrackgain witness: track-gain:-6 -> A1 -600 cdB; Track 0 dB -> 0; no selection -> {}", timeline_core::EditError::NoTrack);
    }
}


// ---- STRIACROP (LEDGER SR867): the witness's crop token (named tail block) ----
#[cfg(test)]
mod striacrop_tests {
    use super::*;

    /// STRIACROP (SR867): `crop:L:T:R:B` is timeline_core's `crop L T R B` on the selected clip (one undo step, the clip
    /// carries it); a short token is a bad token; a crop keeping no picture is refused by name on the message row.
    #[test]
    fn striacrop_witness_crop_token() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,crop:250:0:250:0");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.undo_depth(), 1);
        assert_eq!(face.state.selected().map(|c| c.crop), Some(timeline_core::Crop { l: 250, t: 0, r: 250, b: 0 }));
        assert_eq!(drive(&mut face, "crop:1:2"), vec!["bad token crop:1:2".to_string()]);
        assert_eq!(drive(&mut face, "crop:900:0:200:0"), vec![format!("crop 900 0 200 0: {}", timeline_core::EditError::Crop)]);
        assert_eq!(face.state.undo_depth(), 1);
    }
}

// ---- STRIACLIPLINK (LEDGER SR863): the `link` / `unlink` buttons through `--do` (named tail block) ----
#[cfg(test)]
mod striacliplink_tests {
    use super::*;

    /// `--do link` with nothing selected names its refusal; after two clips are selected `link` joins them and a click on
    /// one member selects both; `unlink` parts them; both words are buttons before `help`.
    #[test]
    fn striacliplink_do_link_button() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, "link");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("no such clip"), "{errors:?}");
        f.state.select(0, 0).unwrap();
        f.state.select_add(1, 0).unwrap();
        let errors = drive(&mut f, "link");
        assert!(errors.is_empty(), "{errors:?}");
        f.state.select(0, 0).unwrap();
        assert_eq!(f.state.set, vec![(0, 0), (1, 0)]);
        let errors = drive(&mut f, "unlink");
        assert!(errors.is_empty(), "{errors:?}");
        f.state.select(0, 0).unwrap();
        assert_eq!(f.state.set, vec![(0, 0)]);
        let TetraNode::VStack(top) = editor_tree() else { panic!("not a VStack") };
        let TetraNode::HStack(row) = &top[1] else { panic!("no transport row") };
        let ids: Vec<&str> = row.iter().map(|b| match b { TetraNode::Button { id, .. } => id.as_str(), _ => "" }).collect();
        assert_eq!(&ids[ids.len() - 5..], ["link", "unlink", "unlink-all", "link-av", "help"]); // STRIALINKAUTO (SR911): `unlink-all` before help; STRIALINKAUTOFUZZ (SR928): `link-av` after it
    }
}

// ---- STRIALINKAUTO (LEDGER SR911): the `unlink-all` button through `--do` (named tail block) ----
#[cfg(test)]
mod strialinkauto_tests {
    use super::*;

    /// `--do unlink-all` parts every link of the face's timeline in one step; the button sits just before `help`.
    #[test]
    fn strialinkauto_do_unlink_all_button() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl.link_av()), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, "unlink-all");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((f.state.timeline.clone(), f.state.undo_depth()), (tl, 1));
        let TetraNode::VStack(top) = editor_tree() else { panic!("not a VStack") };
        let TetraNode::HStack(row) = &top[1] else { panic!("no transport row") };
        assert_eq!(row.len(), nle_face::VERBS.len());
    }
}

// ---- STRIALOUDNESSREADOUT (LEDGER SR866): the LUFS transport button — the `loudness` word on the bar (named tail block) ----
#[cfg(test)]
mod strialoudnessreadout_tests {
    use super::*;

    /// STRIALOUDNESSREADOUT M3: the `loudness` button (`LUFS`) sits immediately before `help`; driving its word (the
    /// `--do` road the click takes) starts the measure with no error, and its end puts the face's own line on the message line.
    #[test]
    fn strialoudnessreadout_button_before_help_measures() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let Some(TetraNode::Button { id, label, action }) = row.iter().rev().nth(1) else { panic!("no button before help") };
        assert_eq!((id.as_str(), label.as_str()), ("loudness", "LUFS"));
        let SMessage::Input { target, text } = action else { panic!("not an Input") };
        assert_eq!((target.as_str(), text.as_str()), (SURFACE, "loudness"));
        let (tl, src) = nle_face::synth_fixture();
        let (tl2, src2) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, text);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(f.loudness_pending());
        assert!(f.wait_loudness());
        let want = nle_face::loudness_line(&stria::nle::loudness(&tl2, 0..tl2.end(), &src2), &None);
        assert_eq!(f.last_error.take(), Some(want.clone()));
        println!("strialoudnessreadout witness: button {id} {label:?} -> {want}");
    }
}

// ---- STRIACOLORLIFT (LEDGER SR869): the `color:<b>:<c>:<s>` token and the JSON `color` (named tail block) ----
/// `,"color":[b,c,s]` while the selected clip carries a grade; empty otherwise (the default line unchanged).
fn color_json(face: &Face) -> String {
    match face.state.selected().map(|c| c.grade) {
        Some(g) if g != timeline_core::Grade::IDENTITY => format!(",\"color\":[{},{},{}]", g.brightness, g.contrast, g.saturation),
        _ => String::new(),
    }
}

#[cfg(test)]
mod striacolorlift_tests {
    use super::*;

    /// M3: `at:12:0` selects B, `color:0:0:-1000` is timeline_core's `color` word (one undo step) and the witness field names
    /// the grade; `color:5000:0:0` is refused by EditError::Grade's name; bare `color` clears it and the field goes.
    #[test]
    fn striacolorlift_witness_color_token() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(color_json(&face), "");
        let errors = drive(&mut face, "at:12:0,color:0:0:-1000");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(color_json(&face), r#","color":[0,0,-1000]"#);
        assert_eq!(face.state.undo_depth(), 1);
        assert_eq!(drive(&mut face, "color:5000:0:0"), vec![format!("color 5000 0 0: {}", timeline_core::EditError::Grade)]);
        assert_eq!(face.state.undo_depth(), 1);
        assert!(drive(&mut face, "undo").is_empty());
        assert_eq!(color_json(&face), "");
        println!("striacolorlift witness: color:0:0:-1000 -> \"color\":[0,0,-1000]");
    }
}

// ---- STRIAHELPSEARCH (LEDGER SR874): the witness's `help <word>` reading (named tail block) ----
/// `"help_query"` and the shown row count, only while the keys help shows filtered (`help:<word>`, `Face::help_query`);
/// the bare help's line is unchanged.
fn help_search_json(face: &Face) -> String {
    match face.help_query() {
        Some(q) if face.help_shown() => format!(",\"help_query\":\"{}\",\"help_rows\":{}", q.replace('"', "'"), face.help_rows_shown().len()),
        _ => String::new(),
    }
}

#[cfg(test)]
mod striahelpsearch_tests {
    use super::*;

    /// STRIAHELPSEARCH M3: `--do help:slip` shows the filtered help, the line carries the query and the shown count; a
    /// bare `help` hides it and both fields are gone; `help:zzz` is refused by name.
    #[test]
    fn striahelpsearch_witness_query_field() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(help_search_json(&f), "");
        let errors = drive(&mut f, "help:slip");
        assert!(errors.is_empty(), "{errors:?}");
        let n = nle_face::help_matching("slip").len();
        assert!(n > 0);
        assert_eq!(help_json(&f), ",\"help\":true");
        assert_eq!(help_search_json(&f), format!(",\"help_query\":\"slip\",\"help_rows\":{n}"));
        drive(&mut f, "help");
        assert_eq!((help_json(&f), help_search_json(&f)), (String::new(), String::new()));
        let errors = drive(&mut f, "help:zzz");
        assert_eq!(errors, vec!["help: no key matches \"zzz\"".to_string()]);
        assert!(!f.help_shown());
        println!("striahelpsearch witness: help:slip -> ,\"help_query\":\"slip\",\"help_rows\":{n}");
    }
}

// ---- STRIASOURCEINFO (LEDGER SR875): the witness's standing info line (named tail block) ----
/// The witness's `"info"` field, only while an `info` line stands (`Face::info_text`); the default line is unchanged.
fn info_json(face: &Face) -> String {
    face.info_text().map(|s| format!(",\"info\":\"{}\"", s.replace('"', "'"))).unwrap_or_default()
}

#[cfg(test)]
mod striasourceinfo_tests {
    use super::*;

    /// STRIASOURCEINFO M3: the button before `help` fires the face's `info` word; `at:12:0,info` carries reel B's line on
    /// `"info"`; `tick:<INFO_SECS x 25 - 1>` keeps it, one more tick ends it (the field absent); `info` alone is refused by name.
    #[test]
    fn striasourceinfo_button_and_field() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let Some(TetraNode::Button { id, label, action }) = row.get(row.len() - 2) else { panic!("no button before help") };
        assert_eq!((id.as_str(), label.as_str()), ("info", "Info"));
        let SMessage::Input { target, text } = action else { panic!("not an Input") };
        assert_eq!((target.as_str(), text.as_str()), (SURFACE, "info"));
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(drive(&mut f, "info"), vec![format!("info: {}", timeline_core::EditError::NoClip)]);
        assert_eq!(info_json(&f), "");
        let errors = drive(&mut f, "at:12:0,info");
        assert!(errors.is_empty(), "{errors:?}");
        let j = info_json(&f);
        assert!(j.starts_with(",\"info\":\"info: B: utp1 64x48 ~") && j.ends_with(" kb/s\""), "{j}");
        let n = nle_face::INFO_SECS * 25;
        assert!(drive(&mut f, &format!("tick:{}", n - 1)).is_empty());
        assert_eq!(info_json(&f), j, "still standing one tick before the end");
        assert!(drive(&mut f, "tick:1").is_empty());
        assert_eq!(info_json(&f), "");
        println!("striasourceinfo witness: at:12:0,info -> {j}; tick:{n} -> absent");
    }
}

// ---- STRIAMETERCLIPVERB (LEDGER SR902): the meters-reset button and the witness's "clipped" (named tail block) ----
/// The witness's `"clipped"` field: the latched meters (track index, `"master"`), in latch order, only while any latch
/// stands (`Face::meter_clipped`); the default line is unchanged.
fn clipped_json(face: &Face) -> String {
    let keys = face.meter_clipped();
    if keys.is_empty() {
        return String::new();
    }
    let all: Vec<String> = keys.iter().copied().map(|k| k.map(|t| t.to_string()).unwrap_or_else(|| "\"master\"".into())).collect();
    format!(",\"clipped\":[{}]", all.join(","))
}

#[cfg(test)]
mod striameterclipverb_tests {
    use super::*;

    /// STRIAMETERCLIPVERB M3: no latch, no field; +24 dB on track 1 played two ticks latches track 1 and the master;
    /// `meters-reset` (the button's word, through `--do`) clears the field.
    #[test]
    fn clipped_field_only_while_latched() {
        let (tl, src) = nle_face::synth_fixture();
        let face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(clipped_json(&face), "");
        let (mut tl, src) = nle_face::synth_fixture();
        tl.tracks[1].clips.iter_mut().for_each(|c| *c = c.gained(2400));
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "play,tick:2");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(clipped_json(&face), r#","clipped":[1,"master"]"#);
        let errors = drive(&mut face, "meters-reset");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(clipped_json(&face), "");
    }

    /// STRIAMETERCLIPVERB M3: the Reset Meters button sits immediately before `?`, firing the face's `meters-reset`.
    #[test]
    fn reset_button_sits_before_help() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let Some(TetraNode::Button { id, label, action }) = row.get(row.len() - 2) else { panic!("no button before help") };
        assert_eq!((id.as_str(), label.as_str()), ("meters-reset", "Reset Meters"));
        let SMessage::Input { target, text } = action else { panic!("not an Input") };
        assert_eq!((target.as_str(), text.as_str()), (SURFACE, "meters-reset"));
    }
}

// ---- STRIAPERFPAINTBUDGET (LEDGER SR890): the witness's paint time and the 60-clip budget (named tail block) ----
// `--paint-ms` adds `,"paint_ms":<ms>` at the line's end: the wall time of the witness's ONE `Face::frames` (one
// `paint_role` unsplit, the preview's and the tracks' with `--split`), which is exactly what `serve` pays per repaint. Off by
// default: a wall time differs run to run and the default line stays byte-identical. The flag is read from the arguments
// (no `Cli` field: the tests' two `Cli` literals stay as they are). Owed (handlers.md NLE5b row): NLE-PERF-PREVIEW-CACHE,
// NLE-PERF-PREVIEW-SCALE, NLE-PERF-CANVAS-FILL, NLE-PERF-LABEL-SHAPE, NLE-PERF-ROLE-CULL, NLE-PERF-MESSAGE, NLE-PERF-HELP,
// NLE-PERF-DRAG-SHOWN, NLE-PERF-DAMAGE; NLE-PERF-PIN-REALMEDIA, NLE-PERF-PIN-NUMBER.

/// `--paint-ms` was given (`cli` accepts it and keeps no field for it).
fn paint_ms_on() -> bool {
    std::env::args().skip(1).any(|a| a == "--paint-ms")
}

/// The witness's `"paint_ms"` field: `d` in milliseconds to three decimals when `on`, else nothing (the default line).
fn paint_ms_json(on: bool, d: std::time::Duration) -> String {
    if on { format!(",\"paint_ms\":{:.3}", d.as_secs_f64() * 1000.0) } else { String::new() }
}

#[cfg(test)]
mod striaperfpaintbudget_tests {
    use super::*;
    use std::time::{Duration, Instant};
    use timeline_core::{Clip, Kind};

    /// The ceiling the test asserts (ms, median): generous, a debug build on a shared loaded bench. The 8 ms pin of the
    /// backlog entry is a release-build number the seat sets once it has run this (NLE-PERF-PIN-NUMBER).
    const PAINT_CEILING_MS: f64 = 250.0;

    /// 60 clips on one page at the default zoom: V1 + A1, 30 clips each of 8 ticks back to back (240 ticks = 1920 px at
    /// 8 px per tick), reels A / B alternating, `src_in` (i % 5) * 8 inside 50-frame synth sources (NLE1's, `synth_fixture`'s shape).
    fn sixty_clips() -> (Timeline, Sources) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for t in 0..2 {
            tl.tracks[t].clips = (0..30i64).map(|i| Clip::new(if i % 2 == 0 { a } else { b }, (i % 5) * 8, (i % 5) * 8 + 8, i * 8)).collect();
        }
        let mut src = Sources::new(&tl);
        for (id, base) in [(a, 0u32), (b, 1000)] {
            src.add(id, stria::nle::synth_source(base, 50, 25)).expect("synthetic source opens");
        }
        (tl, src)
    }

    /// One warm `frames()`, then 20 timed: (min, median, max) in ms; every blit is the whole 1920x1200 canvas.
    fn time_frames(face: &mut Face) -> (f64, f64, f64) {
        assert_eq!(face.frames().len(), 1);
        let mut ms: Vec<f64> = (0..20)
            .map(|_| {
                let t0 = Instant::now();
                let blits = face.frames();
                let d = t0.elapsed().as_secs_f64() * 1000.0;
                assert!(matches!(blits.as_slice(), [SMessage::SurfaceBlit { width: 1920, height: 1200, .. }]));
                d
            })
            .collect();
        ms.sort_by(|x, y| x.total_cmp(y));
        (ms[0], ms[ms.len() / 2], ms[ms.len() - 1])
    }

    /// STRIAPERFPAINTBUDGET M3: the field is absent unless asked for, and reads milliseconds to three decimals.
    #[test]
    fn striaperfpaintbudget_field_off_by_default() {
        assert_eq!(paint_ms_json(false, Duration::from_micros(1234)), "");
        assert_eq!(paint_ms_json(true, Duration::from_micros(1234)), r#","paint_ms":1.234"#);
    }

    /// STRIAPERFPAINTBUDGET M3: a 60-clip timeline at 1920x1200 with a clip selected (handles, the reel message line) and the
    /// playhead mid-page (a preview decodes): the paint PRINTED with labels, without, and with the keys help; only the
    /// generous ceiling is asserted (the 64x48 synth reels under-measure the preview: NLE-PERF-PIN-REALMEDIA).
    #[test]
    fn striaperfpaintbudget_sixty_clips_paint() {
        let mut runs = Vec::new();
        for (name, labels, script) in [("labels", true, "at:120:0"), ("nolabels", false, "at:120:0"), ("help", true, "at:120:0,help")] {
            let (tl, src) = sixty_clips();
            let mut face = Face::new(EditorState::new(tl), src, (1920, 1200), PixelOrder::Rgba, labels);
            let errors = drive(&mut face, script);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(face.state.timeline.tracks.iter().map(|t| t.clips.len()).sum::<usize>(), 60);
            assert!(face.state.selected().is_some(), "at:120:0 selects a clip");
            let (min, med, max) = time_frames(&mut face);
            assert!(med < PAINT_CEILING_MS, "{name}: median paint {med:.3} ms over the {} ms ceiling", PAINT_CEILING_MS);
            runs.push(format!("{name} {min:.3}/{med:.3}/{max:.3}"));
        }
        println!("striaperfpaintbudget witness: 60 clips 1920x1200 paint_ms min/median/max {}", runs.join(" "));
    }
}

// ---- STRIAOPACITYFACE (LEDGER SR898): `--do opacity:<n>` and the two opacity buttons (named tail block) ----
#[cfg(test)]
mod striaopacityface_tests {
    use super::*;

    /// `--do at:12:0,opacity:50` sets the clicked clip to 50 % through the typed word with no error; the transport's two
    /// opacity buttons sit right before the help button and fire their VERBS words; `opacity+` then names `opacity: 60 %`.
    #[test]
    fn striaopacityface_do_opacity_and_buttons() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,opacity:50");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.selected().map(|c| c.opacity), Some(50));
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let ids: Vec<String> = row.iter().map(|b| match b { TetraNode::Button { id, .. } => id.clone(), _ => panic!("not a button") }).collect();
        let n = ids.len();
        assert_eq!(&ids[n - 3..], &["opacity-".to_string(), "opacity+".to_string(), "help".to_string()]);
        let errors = drive(&mut face, "opacity+");
        assert_eq!(errors, vec!["opacity: 60 %".to_string()]);
        assert_eq!(face.state.selected().map(|c| c.opacity), Some(60));
        println!("striaopacityface: buttons {:?}; opacity+ -> {errors:?}", &ids[n - 3..]);
    }
}

// ---- STRIAPASTEINSERT (LEDGER SR896): the paste-insert / paste-over buttons and `--do` words (named tail block) ----

#[cfg(test)]
mod striapasteinsert_tests {
    use super::*;

    /// `--do at:12:0,copy,ruler:12,paste-insert`: B copied off V1, laid at tick 12 on V1 rippling the rest; the two
    /// buttons sit before the help button.
    #[test]
    fn striapasteinsert_witness_paste_insert_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,copy,ruler:12,paste-insert");
        assert!(errors.is_empty(), "{errors:?}");
        let v1: Vec<(i64, i64, i64)> = face.state.timeline.tracks[0].clips.iter().map(|c| (c.src_in, c.src_out, c.at)).collect();
        assert!(v1.contains(&(0, 10, 12)) && v1.contains(&(2, 10, 22)), "{v1:?}");
        assert_eq!(face.state.timeline.tracks[1], tl0.tracks[1]);
        assert_eq!((face.state.undo_depth(), face.last_error.clone()), (1, None));
        let TetraNode::VStack(top) = editor_tree() else { panic!("not a VStack") };
        let TetraNode::HStack(row) = &top[1] else { panic!("no transport row") };
        let words: Vec<String> = row.iter().filter_map(|b| match b { TetraNode::Button { action: SMessage::Input { text, .. }, .. } => Some(text.clone()), _ => None }).collect();
        assert_eq!(words[words.len() - 3..], ["paste-insert", "paste-over", "help"]);
        println!("striapasteinsert witness: V1 {v1:?}");
    }
}

// ---- STRIASNAPTOGGLE (LEDGER SR892): the `snap` transport button — the drag snap default on / off (named tail block) ----
#[cfg(test)]
mod striasnaptoggle_tests {
    use super::*;

    /// STRIASNAPTOGGLE M3: the transport row's second-to-last button is `snap` (before `help`), firing the face's `snap`
    /// word; driving it (the `--do` road the click takes) turns the snap default off and names it on the message line
    /// (the witness's errors list), a second press turns it back on.
    #[test]
    fn striasnaptoggle_button_before_help_drives_the_word() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let TetraNode::Button { id, label, action } = &row[row.len() - 2] else { panic!("not a button") };
        assert_eq!((id.as_str(), label.as_str()), ("snap", "Snap"));
        let SMessage::Input { target, text } = action else { panic!("not an Input") };
        assert_eq!((target.as_str(), text.as_str()), (SURFACE, "snap"));
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(f.snap_on());
        assert_eq!(drive(&mut f, text), vec!["snap: off".to_string()]);
        assert!(!f.snap_on());
        assert_eq!(drive(&mut f, text), vec!["snap: on".to_string()]);
        assert!(f.snap_on());
        println!("striasnaptoggle witness: button {id} {label:?} -> snap: off, again -> snap: on");
    }
}

// ---- STRIARECENTFILES (LEDGER SR884): the recent documents through Principia's store (named tail block) ----

/// STRIARECENTFILES (SR884): a window's `<edl>` goes first in Principia's `stria.recent` (canonicalized, else as given),
/// folded by `nle_face::remember_doc` and written through quartzite's host prefs get / set (`chrome_rows::stored_str` /
/// `store_str`: the SAVERECENT SR391 road, one store). `--synth` records nothing.
fn remember_recent(c: &Cli) {
    use quartzite::widgets::chrome_rows::{store_str, stored_str};
    let Some(edl) = c.edl.as_deref() else { return };
    let path = std::fs::canonicalize(edl).unwrap_or_else(|_| std::path::PathBuf::from(edl));
    if let Some(row) = nle_face::remember_doc(&path, stored_str(nle_face::RECENT_NS, nle_face::RECENT_KEY).as_deref()) {
        store_str(nle_face::RECENT_NS, nle_face::RECENT_KEY, row);
    }
}

/// STRIARECENTFILES (SR884): the `recent` token asks the witness line for the recent list; it sends the face nothing.
fn recent_token(tok: &str) -> Option<Vec<SMessage>> {
    (tok == "recent").then(Vec::new)
}

/// STRIARECENTFILES (SR884): the witness's `"recent"` field, only when the script names `recent` (the default line is
/// unchanged and reads no store).
fn recent_json(c: &Cli) -> String {
    if !recent_asked(c.script.as_deref()) {
        return String::new();
    }
    recent_json_of(quartzite::widgets::chrome_rows::stored_str(nle_face::RECENT_NS, nle_face::RECENT_KEY).as_deref())
}

/// STRIARECENTFILES (SR884): the script names the `recent` token.
fn recent_asked(script: Option<&str>) -> bool {
    script.unwrap_or("").split(',').any(|t| t == "recent")
}

/// STRIARECENTFILES (SR884): `,"recent":["<path>",...]` of a stored row, newest first (a `"` read as `'`, as `errors`).
fn recent_json_of(stored: Option<&str>) -> String {
    let all: Vec<String> = nle_face::recent_docs(stored).iter().map(|p| format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "'"))).collect();
    format!(",\"recent\":[{}]", all.join(","))
}

#[cfg(test)]
mod striarecentfiles_tests {
    use super::*;

    /// STRIARECENTFILES M3: `recent` is a witness token that sends the face nothing and raises no error.
    #[test]
    fn striarecentfiles_token_is_quiet() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "recent");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((face.state.playhead, face.state.undo_depth(), face.dirty()), (0, 0, false));
        assert_eq!(recent_token("recent").map(|m| m.len()), Some(0));
        assert!(recent_token("recently").is_none());
    }

    /// STRIARECENTFILES M3: the field's shapes: an unset row is an empty list, newest first, a quote read as `'`.
    #[test]
    fn striarecentfiles_json_shapes() {
        assert_eq!(recent_json_of(None), ",\"recent\":[]");
        assert_eq!(recent_json_of(Some("/p/b.edl\n/p/a.edl")), ",\"recent\":[\"/p/b.edl\",\"/p/a.edl\"]");
        assert_eq!(recent_json_of(Some("/p/say \"hi\".edl")), ",\"recent\":[\"/p/say 'hi'.edl\"]");
    }

    /// STRIARECENTFILES M3: a script without `recent` adds no field (and reads no store).
    #[test]
    fn striarecentfiles_default_line_unchanged() {
        assert!(!recent_asked(None));
        assert!(!recent_asked(Some("play,play,meters")));
        assert!(!recent_asked(Some("recently")));
        assert!(recent_asked(Some("play,recent")));
    }
}

// ---- STRIAGRADEFACE (LEDGER SR899): the grade step buttons (named tail block) ----
#[cfg(test)]
mod striagradeface_tests {
    use super::*;

    /// M3: `at:12:0` selects B; `bright+` and `sat-` are the transport buttons' words (one undo step each) and the
    /// witness's `color` field reads the stepped grade.
    #[test]
    fn striagradeface_witness_buttons() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,bright+,sat-");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(color_json(&face), r#","color":[50,0,-50]"#);
        assert_eq!(face.state.undo_depth(), 2);
        println!("striagradeface witness: bright+,sat- -> \"color\":[50,0,-50]");
    }
}

// ---- STRIAUNDOHISTORY (LEDGER SR876): the History button and the witness's history line (named tail block) ----
#[cfg(test)]
mod striaundohistory_tests {
    use super::*;

    /// The transport row carries `history` right before `help`; `--do ...,history` puts the undo history line on the
    /// message (the witness's `errors`, THE message road) and commits nothing.
    #[test]
    fn striaundohistory_button_and_line() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let ids: Vec<&str> = row.iter().filter_map(|b| match b { TetraNode::Button { id, .. } => Some(id.as_str()), _ => None }).collect();
        assert_eq!(&ids[ids.len() - 2..], &["history", "help"]);
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, "at:12:0,split,history");
        assert_eq!(errors, vec!["history: 1 split".to_string()]);
        assert_eq!(f.state.undo_depth(), 1);
    }
}

// ---- STRIAKEYBOARDMAP (LEDGER SR886): the witness's --keys overlay (named tail block) ----
#[cfg(test)]
mod striakeyboardmap_tests {
    use super::*;

    /// STRIAKEYBOARDMAP M3: the face `--keys "p play"` builds takes `key:p` as play (the overlay first); a refused map
    /// leaves the static table and its reason on the message row.
    #[test]
    fn keys_overlay_through_the_script() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_keys("p play");
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        assert!(!face.playing);
        assert!(drive(&mut face, "key:p").is_empty());
        assert!(face.playing, "p plays through the overlay");
        let (tl, src) = nle_face::synth_fixture();
        let face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_keys("p spilt");
        assert_eq!(face.last_error.as_deref(), Some("keys: line 1: unknown action \"spilt\""));
        assert!(face.keymap().is_empty());
        println!("striakeyboardmap witness: --keys \"p play\" + key:p -> playing");
    }
}

// ---- STRIAPANFACE (LEDGER SR897): the three pan buttons before `?` (named tail block) ----
#[cfg(test)]
mod striapanface_tests {
    use super::*;

    /// The transport row's three buttons just before `?` are `pan-` / `pan` / `pan+`; `--do at:12:1,pan+,pan+` pans the
    /// selected audio clip two eighths right and each press reads the pan on the message row.
    #[test]
    fn striapanface_buttons_pan_the_clip() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let ids: Vec<&str> = row[row.len() - 4..].iter().map(|b| match b { TetraNode::Button { id, .. } => id.as_str(), _ => panic!("not a button") }).collect();
        assert_eq!(ids, ["pan-", "pan", "pan+", "help"]);
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let lines = drive(&mut face, "at:12:1,pan+,pan+");
        assert_eq!(lines, ["pan: R 13", "pan: R 25"]);
        assert_eq!(face.state.timeline.tracks[1].clips[1].pan_cq8, 32);
        println!("striapanface witness: pan+ x2 -> {lines:?}");
    }
}

// ---- STRIACROPFACE (LEDGER SR900): the witness's crop words and the Uncrop button (named tail block) ----
#[cfg(test)]
mod striacropface_tests {
    use super::*;

    /// STRIACROPFACE (SR900): `crop-l+` / `crop-r+` step the selected clip's crop (one undo step each); `crop` clears it (a
    /// third); the transport's button before `?` is `Uncrop`, firing the `crop` word.
    #[test]
    fn striacropface_witness_crop_words() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,crop-l+,crop-r+");
        assert!(errors.is_empty(), "{errors:?}");
        let s = timeline_core::editor::CROP_STEP;
        assert_eq!(face.state.selected().map(|c| c.crop), Some(timeline_core::Crop { l: s, t: 0, r: s, b: 0 }));
        assert_eq!(face.state.undo_depth(), 2);
        assert!(drive(&mut face, "crop").is_empty());
        assert_eq!((face.state.selected().map(|c| c.crop), face.state.undo_depth()), (Some(timeline_core::Crop::NONE), 3));
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let Some(TetraNode::Button { id, label, action }) = row.iter().rev().nth(1) else { panic!("no button before help") };
        assert_eq!((id.as_str(), label.as_str()), ("crop", "Uncrop"));
        let SMessage::Input { target, text } = action else { panic!("not an Input") };
        assert_eq!((target.as_str(), text.as_str()), (SURFACE, "crop"));
    }
}

// ---- STRIAPOSITIONSCALE (LEDGER SR879): the witness's pip / scale tokens and button (named tail block) ----
#[cfg(test)]
mod striapositionscale_tests {
    use super::*;

    /// STRIAPOSITIONSCALE (SR879): `pip` (the VERBS word and its button), `pip:X:Y`, `scale:S` and `pip:off` are
    /// timeline_core's words on the selected clip, one undo step each; a rect off the frame is refused by name on the
    /// message row; a malformed token is a bad token.
    #[test]
    fn striapositionscale_witness_pip_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,pip,scale:100,pip:0:0");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.undo_depth(), 3);
        assert_eq!(face.state.selected().map(|c| c.xform), Some(timeline_core::Xform { x: 0, y: 0, s: 100 }));
        assert_eq!(drive(&mut face, "pip:1:2:3"), vec!["bad token pip:1:2:3".to_string()]);
        assert_eq!(drive(&mut face, "pip:950:0"), vec![format!("pip 950 0: {}", timeline_core::EditError::Xform)]);
        assert!(drive(&mut face, "pip:off").is_empty());
        assert_eq!((face.state.undo_depth(), face.state.selected().map(|c| c.xform)), (4, Some(timeline_core::Xform::NONE)));
    }
}

// ---- STRIAPROJECTBIN (LEDGER SR885): the bin Surface in the split tree, the `bin:<row>` token, the witness's bin rows (named tail block) ----

/// `--bin`: the split tree with the source bin Surface ([`nle_face::BIN_SURFACE`]) last, so it is number 2 and the
/// preview / tracks numbers stay 0 / 1. Without `--bin` the tree is returned unchanged.
fn bin_tree(c: &Cli, t: TetraNode) -> TetraNode {
    match t {
        TetraNode::VStack(mut v) if c.bin => {
            v.push(TetraNode::Surface { id: nle_face::BIN_SURFACE.into() });
            TetraNode::VStack(v)
        }
        t => t,
    }
}

/// `--bin`: the split face with its bin on the tree's bin Surface (`Face::with_bin`).
fn bin_route(c: &Cli, map: &SurfaceMap, face: Face) -> Face {
    if c.bin { face.with_bin(map.number(nle_face::BIN_SURFACE).unwrap_or(2)) } else { face }
}

/// `bin:<row>`: a click on that bin row, stamped with the bin's Surface number and sent through `Face::on` here (so
/// `drive_on`'s stamping does not move it to the tracks Surface). `None` with no bin (a bad token).
fn bin_token(face: &mut Face, tok: &str) -> Option<Vec<SMessage>> {
    let row = tok.strip_prefix("bin:")?.parse::<usize>().ok()?;
    let b = face.bin()?;
    let (x, y) = timeline_core::view::bin_point(row);
    face.on(&SMessage::BrowserClick(x, y, b.n));
    Some(Vec::new())
}

/// The witness's `"bin"` field with `--bin` only: each source's reel, duration as timecode and kind (the default line is
/// unchanged).
fn bin_json(c: &Cli, face: &Face) -> String {
    if !c.bin {
        return String::new();
    }
    let tl = &face.state.timeline;
    let fps = tl.fps().unwrap_or(25);
    let rows: Vec<String> = timeline_core::view::bin_rows(tl)
        .iter()
        .map(|r| format!("{{\"reel\":\"{}\",\"dur\":\"{}\",\"kind\":\"{}\"}}", r.reel.replace('"', "'"), edl::timecode(r.duration, fps), r.kind.name()))
        .collect();
    format!(",\"bin\":[{}]", rows.join(","))
}

#[cfg(test)]
mod striaprojectbin_tests {
    use super::*;
    use timeline_core::{Clip, Mode, SourceId};

    fn bin_cli() -> Cli {
        Cli { edl: None, synth: true, fps: 25, srcs: Vec::new(), size: (320, 160), script: None, out: None, labels: false, two: false, split: true, mute: false, keys: None, bin: true }
    }

    /// `--split --bin` numbers the bin Surface 2 (preview 0, tracks 1); `--split` alone is today's tree; the routed face
    /// carries the bin and its three blits.
    #[test]
    fn striaprojectbin_tree_numbers_the_bin_two() {
        let c = bin_cli();
        let m = SurfaceMap::of(&tree(&c));
        assert_eq!((m.number(PREVIEW_SURFACE), m.number(TRACKS_SURFACE), m.number(nle_face::BIN_SURFACE)), (Some(0), Some(1), Some(2)));
        assert_eq!(format!("{:?}", tree(&Cli { bin: false, ..bin_cli() })), format!("{:?}", split_tree()));
        let (tl, src) = nle_face::synth_fixture();
        let mut face = route(&c, Face::new(EditorState::new(tl), src, c.size, PixelOrder::Rgba, false));
        assert_eq!(face.bin().map(|b| b.n), Some(2));
        let frames = face.frames();
        assert_eq!(frames.len(), 3);
        assert!(blits_json(&c, &frames).starts_with(r#","surfaces":3,"fnvs":[""#));
    }

    /// `ruler:15,bin:1` lays reel B's whole range at 15 on V1 (SR852's `insert B 0 50`, one undo step); the witness's bin
    /// field lists both reels; without a bin `bin:1` is a bad token.
    #[test]
    fn striaprojectbin_token_inserts_and_json_lists_the_sources() {
        let c = bin_cli();
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = route(&c, Face::new(EditorState::new(tl), src, c.size, PixelOrder::Rgba, false));
        assert_eq!(bin_json(&c, &face), r#","bin":[{"reel":"A","dur":"00:00:02:00","kind":"AV"},{"reel":"B","dur":"00:00:02:00","kind":"AV"}]"#);
        assert_eq!(bin_json(&Cli { bin: false, ..bin_cli() }, &face), "");
        let mut presses = Vec::new();
        let errors = drive_on(&mut face, "ruler:15,bin:1", Some((1, 0)), &mut presses);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.timeline, tl0.place(0, Clip::new(SourceId(1), 0, 50, 15), Mode::Insert).unwrap());
        assert_eq!((face.state.undo_depth(), face.state.playhead), (1, 65));
        let (tl, src) = nle_face::synth_fixture();
        let mut plain = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(drive(&mut plain, "bin:1"), vec!["bad token bin:1".to_string()]);
        println!("striaprojectbin witness: --split --bin --do ruler:15,bin:1 -> B 0..50 at 15 on V1, undo 1, playhead 65");
    }
}

// ---- STRIATCSET (LEDGER SR908): the witness's `tc:<unit>` token and the `t` key (named tail block) ----
#[cfg(test)]
mod striatcset_tests {
    use super::*;

    /// `tc:frames` / `tc:off` set the readout (no error; `"tc"` reads frames, then absent); `tc:bogus` is the face's one
    /// error naming `tc`; `key:t` cycles from off to timecode. No undo step, not dirty.
    #[test]
    fn striatcset_do_tc_unit_token() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut f, "tc:frames").is_empty());
        assert_eq!(tc_json(&f), ",\"tc\":\"frames\"");
        assert!(drive(&mut f, "tc:off").is_empty());
        assert_eq!(tc_json(&f), "");
        let errors = drive(&mut f, "tc:bogus");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("tc:"), "{errors:?}");
        assert_eq!(tc_json(&f), "");
        assert!(drive(&mut f, "key:t").is_empty());
        assert_eq!(tc_json(&f), ",\"tc\":\"timecode\"");
        assert_eq!((f.state.undo_depth(), f.dirty()), (0, false));
        println!("striatcset witness: tc:frames, tc:off, tc:bogus refused, key:t -> timecode");
    }
}

// ---- STRIAINSERTMARKS (LEDGER SR905): the insert-marked / overwrite-marked buttons through `--do` (named tail block) ----

#[cfg(test)]
mod striainsertmarks_tests {
    use super::*;
    use timeline_core::SourceId;

    /// STRIAINSERTMARKS M3: the two buttons sit just before `help`; `--do at:12:0,ruler:5,in,ruler:9,out,insert-marked`
    /// lays the selected B 0..4 at 5 on V1 (the witness's `"clip"` reads `5:0:4`), the playhead at the out mark.
    #[test]
    fn striainsertmarks_buttons_and_drive() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let ids: Vec<String> = row.iter().map(|b| match b { TetraNode::Button { id, .. } => id.clone(), _ => panic!("not a button") }).collect();
        assert_eq!(ids[ids.len() - 3..], ["insert-marked", "overwrite-marked", "help"]);
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let a1 = f.state.timeline.tracks[1].clone();
        let errors = drive(&mut f, "at:12:0,ruler:5,in,ruler:9,out,insert-marked");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        let c = f.state.selected().expect("the laid clip selected");
        assert_eq!(format!("{}:{}:{}", c.at, c.src_in, c.src_out), "5:0:4");
        assert_eq!((c.src, f.state.playhead, f.state.timeline.marks), (SourceId(1), 9, (Some(5), Some(9))));
        assert_eq!(f.state.timeline.tracks[1], a1, "insert ripples only the target track");
        println!("striainsertmarks witness: at:12:0,ruler:5,in,ruler:9,out,insert-marked -> clip 5:0:4, playhead 9");
    }
}

// ---- STRIATRACKGAINSTEP (LEDGER SR901): the Track - / Track + buttons and the success line (named tail block) ----

#[cfg(test)]
mod striatrackgainstep_tests {
    use super::*;

    /// M3: both step buttons are on the row (help still last); `at:12:1,track-gain-` steps A1 one dB down in one undo step
    /// and the message road carries the success line; `track-gain+` brings it back to unity.
    #[test]
    fn striatrackgainstep_witness_buttons_and_line() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        for w in ["track-gain-", "track-gain+"] {
            assert!(row.iter().any(|b| matches!(b, TetraNode::Button { id, .. } if id == w)), "{w}");
        }
        assert!(matches!(row.last(), Some(TetraNode::Button { id, .. }) if id == "help"));
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(drive(&mut face, "at:12:1,track-gain-"), vec!["track-gain: A1 -1.0 dB".to_string()]);
        assert_eq!((face.state.timeline.tracks[1].gain_cdb, face.state.undo_depth()), (-100, 1));
        assert_eq!(drive(&mut face, "track-gain+"), vec!["track-gain: A1 0.0 dB".to_string()]);
        assert_eq!((face.state.timeline.tracks[1].gain_cdb, face.state.undo_depth()), (0, 2));
        println!("striatrackgainstep witness: track-gain- -> A1 -1.0 dB; track-gain+ -> 0.0 dB");
    }
}

// ---- STRIAGAPSBLACK (LEDGER SR909): the witness's bare `gap` button word (named tail block) ----
#[cfg(test)]
mod striagapsblack_tests {
    use super::*;

    /// The Gap button sits before Close Gaps (help stays last); its word `gap` (the `--do gap` token) opens one second at the
    /// playhead: 25 ticks at the synth fixture's 25 fps, the timeline_core op by hand.
    #[test]
    fn striagapsblack_gap_button() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let ids: Vec<String> = row.iter().filter_map(|b| if let TetraNode::Button { id, .. } = b { Some(id.clone()) } else { None }).collect();
        assert_eq!(&ids[ids.len() - 3..], ["gap".to_string(), "close-gaps".to_string(), "help".to_string()]);
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "at:12:0,gap").is_empty());
        assert_eq!(face.state.timeline, tl0.insert_gap(0, 12, 25).unwrap());
        println!("striagapsblack witness: gap -> {:?}", face.state.timeline.tracks[0].clips);
    }
}

// ---- STRIAKEYMAPSTORE (LEDGER SR917): the window's key map from Principia's store (named tail block) ----

/// STRIAKEYMAPSTORE (SR917): a window's key map: `--keys` wins, else Principia's `stria.keys` row through quartzite's host
/// prefs get (`chrome_rows::stored_str`, read once here at the open), else "" (the built-in KEYS). The witness never calls it.
fn window_keys(c: &Cli) -> String {
    nle_face::keys_text(c.keys.as_deref(), || quartzite::widgets::chrome_rows::stored_str(nle_face::KEYS_NS, nle_face::KEYS_KEY))
}

// ---- STRIAPASTEKEYS (LEDGER SR921): the clipboard chords as the surfaces send them (named tail block) ----

/// `key:Ctrl+c` / `key:Meta+c` is the `BrowserCopy` and `key:Ctrl+v` / `key:Meta+v` the `BrowserPaste` (an empty text:
/// the face ignores it) both surfaces send for the chord; `None` for any other token (`key:Ctrl+x` and
/// `key:Ctrl+Shift+V` go on as `BrowserKey`).
fn striapastekeys_token(tok: &str) -> Option<Vec<SMessage>> {
    match tok {
        "key:Ctrl+c" | "key:Meta+c" => Some(vec![SMessage::BrowserCopy]),
        "key:Ctrl+v" | "key:Meta+v" => Some(vec![SMessage::BrowserPaste(String::new())]),
        _ => None,
    }
}

#[cfg(test)]
mod striapastekeys_tests {
    use super::*;

    /// `--do at:12:0,key:Ctrl+c,ruler:35,key:Ctrl+v`: B copied off V1 through the copy chord, laid at tick 35 through the
    /// paste chord (one undo step); then `ruler:12,key:Ctrl+Shift+V` pastes rippling V1.
    #[test]
    fn striapastekeys_witness_chord_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        assert!(matches!(striapastekeys_token("key:Meta+c").as_deref(), Some([SMessage::BrowserCopy])));
        assert!(striapastekeys_token("key:Ctrl+x").is_none());
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,key:Ctrl+c,ruler:35,key:Ctrl+v");
        assert!(errors.is_empty(), "{errors:?}");
        let v1 = &face.state.timeline.tracks[0].clips;
        assert_eq!(v1.last().map(|c| (c.src_in, c.src_out, c.at)), Some((0, 10, 35)));
        assert_eq!(face.state.timeline.tracks[1], tl0.tracks[1]);
        assert_eq!((face.state.undo_depth(), face.last_error.clone()), (1, None));
        let mut face = Face::new(EditorState::new(tl0.clone()), nle_face::synth_fixture().1, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,key:Ctrl+c,ruler:12,key:Ctrl+Shift+V");
        assert!(errors.is_empty(), "{errors:?}");
        let v1: Vec<(i64, i64, i64)> = face.state.timeline.tracks[0].clips.iter().map(|c| (c.src_in, c.src_out, c.at)).collect();
        assert!(v1.contains(&(0, 10, 12)) && v1.contains(&(2, 10, 22)), "{v1:?}");
        println!("striapastekeys witness: V1 {v1:?}");
    }
}

// ---- STRIAUNDOHISTORYDRAG (LEDGER SR912): the witness's `history:<n>` token (named tail block) ----
#[cfg(test)]
mod striaundohistorydrag_tests {
    use super::*;

    /// `--do ...,history:<n>` is the face's `history <n>`: the top n undo entries on the message line (the witness's
    /// `errors`), nothing committed.
    #[test]
    fn striaundohistorydrag_count_token() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, "at:12:0,split,history:1");
        assert_eq!(errors, vec!["history: 1 split".to_string()]);
        assert_eq!(f.state.undo_depth(), 1);
        let errors = drive(&mut f, "history:0");
        assert_eq!(errors, vec!["history: \"0\" is not a count 1..=256".to_string()]);
        assert_eq!(f.state.undo_depth(), 1);
    }
}

// ---- STRIAAUTOSAVECAP (LEDGER SR913): the witness's autosave reading (named tail block) ----
/// The witness's `"autosaved"` field, only while an autosave this session wrote (or recovered) stands
/// (`Face::autosaved`); the default line is unchanged.
fn autosaved_json(face: &Face) -> String {
    if face.autosaved().is_some() { ",\"autosaved\":true".into() } else { String::new() }
}

#[cfg(test)]
mod striaautosavecap_tests {
    use super::*;

    /// STRIAAUTOSAVECAP M3: a face opened from a document carries no field; one split then `tick:<60 s at 25 fps>` (the
    /// clock arm's autosave tick) writes `<stem>.autosave.edl` and the field reads true; `save` removes it and the field.
    #[test]
    fn striaautosavecap_witness_autosaved_field() {
        let exe = std::env::current_exe().expect("the test binary");
        let dir = exe.parent().expect("its directory").join("striaautosavecap-witness");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("cut.edl");
        let (tl, src) = nle_face::synth_fixture();
        std::fs::write(&doc, edl::write(&tl, "cut").unwrap()).unwrap();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(doc.as_path()));
        assert_eq!(autosaved_json(&face), "");
        assert!(drive(&mut face, "at:12:0,split").is_empty());
        assert!(face.dirty());
        assert_eq!(autosaved_json(&face), "unlink", "nothing written before the clock");
        let ticks = format!("tick:{}", nle_face::AUTOSAVE_SECS * 25);
        let errors = drive(&mut face, &ticks);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(nle_face::autosave_path(&doc).is_file());
        assert_eq!(autosaved_json(&face), r#","autosaved":true"#);
        assert!(drive(&mut face, "save").is_empty());
        assert!(!nle_face::autosave_path(&doc).exists());
        assert_eq!(autosaved_json(&face), "");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---- STRIATRACKGAINKEYS (LEDGER SR926): the witness's key tokens reach the audio words (named tail block) ----
#[cfg(test)]
mod striatrackgainkeys_tests {
    use super::*;

    /// M3: `at:12:1,key:v` steps the clicked A1 clip's gain one step down (the gain line); `key::` steps A1's track one
    /// dB down (the `key:` prefix is stripped before the `:` split); `key:Q` mutes A1 with no line; three undo steps.
    #[test]
    fn striatrackgainkeys_witness_key_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(drive(&mut face, "at:12:1,key:v"), vec!["gain -1.0 dB".to_string()]);
        assert_eq!(drive(&mut face, "key::"), vec!["track-gain: A1 -1.0 dB".to_string()]);
        assert!(drive(&mut face, "key:Q").is_empty());
        assert_eq!((face.state.timeline.tracks[1].muted, face.state.timeline.tracks[1].gain_cdb, face.state.undo_depth()), (true, -100, 3));
        println!("striatrackgainkeys witness: key:v -> gain -1.0 dB; key:: -> A1 -1.0 dB; key:Q mutes A1");
    }
}

// ---- STRIALINKAUTOFUZZ (LEDGER SR928): the `link-av` button through `--do` (named tail block) ----
#[cfg(test)]
mod strialinkautofuzz_tests {
    use super::*;

    /// `--do link-av` links the face's V1 / A1 pairs in one step; the button sits just before `help`.
    #[test]
    fn strialinkautofuzz_do_link_av_button() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, "link-av");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((f.state.timeline.clone(), f.state.undo_depth()), (tl.link_av(), 1));
        let TetraNode::VStack(top) = editor_tree() else { panic!("not a VStack") };
        let TetraNode::HStack(row) = &top[1] else { panic!("no transport row") };
        assert_eq!(row.len(), nle_face::VERBS.len());
    }
}

// ---- STRIAGAPSALLTRACKS (LEDGER SR929): the witness's `close-gaps:all` token (named tail block) ----
#[cfg(test)]
mod striagapsalltracks_tests {
    use super::*;

    /// `close-gaps:all` (the argument word `close-gaps all`) closes V1's and A1's 20..25 gaps with nothing selected: the
    /// timeline_core op by hand, no message (no fade removed).
    #[test]
    fn striagapsalltracks_witness_token() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "close-gaps:all").is_empty());
        assert_eq!(face.state.timeline, tl0.close_gaps_all().unwrap());
        assert_eq!((face.state.timeline.tracks[0].clips[2].at, face.state.timeline.tracks[1].clips[2].at), (20, 20));
        println!("striagapsalltracks witness: close-gaps:all -> {:?}", face.state.timeline.tracks[1].clips);
    }
}

// ---- STRIAPROXYBUTTONS (LEDGER SR930): the Proxy Off / Proxy All buttons and the witness's attached reels (named tail block) ----
/// The witness's `"proxy"` field, only while some reel has an attached proxy (`Face::proxy_reels`); the default line is
/// unchanged.
fn proxy_json(face: &Face) -> String {
    let reels = face.proxy_reels();
    if reels.is_empty() {
        return String::new();
    }
    let list: Vec<String> = reels.iter().map(|r| format!("\"{}\"", r.replace('"', "'"))).collect();
    format!(",\"proxy\":[{}]", list.join(","))
}

#[cfg(test)]
mod striaproxybuttons_tests {
    use super::*;

    /// STRIAPROXYBUTTONS M3: the two buttons sit right before `?`, each firing its own VERBS word; with nothing selected
    /// the face refuses by name.
    #[test]
    fn striaproxybuttons_buttons_precede_help() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let n = row.len();
        for (b, (want, lab)) in row[n - 3..n - 1].iter().zip([("proxy-off", "Proxy Off"), ("proxy-all", "Proxy All")]) {
            let TetraNode::Button { id, label, action } = b else { panic!("not a button") };
            assert_eq!((id.as_str(), label.as_str()), (want, lab));
            let SMessage::Input { target, text } = action else { panic!("not an Input") };
            assert_eq!((target.as_str(), text.as_str()), (SURFACE, want));
        }
        assert_eq!(&nle_face::VERBS[nle_face::VERBS.len() - 3..], &["proxy-off", "proxy-allalone", "help"]);
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(drive(&mut f, "proxy-off"), vec!["proxy: no clip selected".to_string()]);
        assert_eq!(drive(&mut f, "proxy-all"), vec!["proxy: all: no reel has a source path".to_string()]);
        println!("striaproxybuttons witness: Proxy Off / Proxy All before ?");
    }

    /// STRIAPROXYBUTTONS M3: no proxy attached -> no `"proxy"` field; `proxy:purge` is a token the face takes.
    #[test]
    fn striaproxybuttons_proxy_field_absent_without_proxy() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(proxy_json(&f), "");
        assert_eq!(drive(&mut f, "proxy:purge"), vec!["proxy: no clip selected".to_string()]);
        assert_eq!(proxy_json(&f), "");
        println!("striaproxybuttons witness: no proxy -> no field; proxy:purge -> no clip selected");
    }
}

// ---- STRIAAUTOSAVEKEEPREAD (LEDGER SR933): the witness's `recover:1` token (named tail block) ----
#[cfg(test)]
mod striaautosavekeepread_tests {
    use super::*;

    /// STRIAAUTOSAVEKEEPREAD M3: `recover:1` is the word `recover 1` at the face: with no kept file beside the document it
    /// is refused by name (`recover: no autosave (<stem>.autosave.1.edl)`); `recover:2` is refused as one generation.
    #[test]
    fn striaautosavekeepread_witness_recover_1() {
        let exe = std::env::current_exe().expect("the test binary");
        let dir = exe.parent().expect("its directory").join("striaautosavekeepread-witness");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("cut.edl");
        let (tl, src) = nle_face::synth_fixture();
        std::fs::write(&doc, edl::write(&tl, "cut").unwrap()).unwrap();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_doc(Some(doc.as_path()));
        let keep = nle_face::autosave_keep_path(&doc);
        assert_eq!(drive(&mut face, "recover:1"), vec![format!("recover: no autosave ({})", keep.display())]);
        assert_eq!(drive(&mut face, "recover:2"), vec!["recover: one older autosave is kept: recover 1".to_string()]);
        assert_eq!(face.state.undo_depth(), 0, "a refused recover pushes nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---- STRIAUNDOJUMP (LEDGER SR934): the witness's `undo:<n>` / `redo:<n>` tokens (named tail block) ----
#[cfg(test)]
mod striaundojump_tests {
    use super::*;

    /// `--do ...,undo:<n>` / `redo:<n>` are the face's `undo <n>` / `redo <n>`: n steps, the landing named on the message
    /// line (the witness's `errors`).
    #[test]
    fn striaundojump_count_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, "at:12:0,split,undo:1");
        assert_eq!(errors, vec!["undo 1: back to before \"split\"".to_string()]);
        assert_eq!((f.state.undo_depth(), f.state.redo_depth()), (0, 1));
        let errors = drive(&mut f, "redo:1");
        assert_eq!(errors, vec!["redo 1: forward to after \"split\"".to_string()]);
        assert_eq!((f.state.undo_depth(), f.state.redo_depth()), (1, 0));
    }
}

// ---- STRIAXFORMPRESETS (LEDGER SR938): the witness's corner tokens and scale step buttons (named tail block) ----
#[cfg(test)]
mod striaxformpresets_tests {
    use super::*;

    /// STRIAXFORMPRESETS (SR938): `pip:bl` (a corner preset) and `scale-` / `scale+` (VERBS words, their buttons) are
    /// timeline_core's words on the selected clip, one undo step each, no error; a step off the frame is refused by name;
    /// an unknown corner is a bad token.
    #[test]
    fn striaxformpresets_witness_corner_and_step_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,pip:bl,scale+,scale-,scale-");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.undo_depth(), 4);
        assert_eq!(face.state.selected().map(|c| c.xform), Some(timeline_core::Xform { x: 50, y: 750, s: 200 }));
        assert_eq!(drive(&mut face, "pip:xx"), vec!["bad token pip:xx".to_string()]);
        assert!(drive(&mut face, "pip:off").is_empty());
        assert_eq!(drive(&mut face, "scale+"), vec![format!("scale+: {}", timeline_core::EditError::Xform)]);
        assert_eq!(face.state.undo_depth(), 5);
    }
}

// ---- STRIAHELPSEARCHLIVE (LEDGER SR941): the witness's live help filter (named tail block) ----
#[cfg(test)]
mod striahelpsearchlive_tests {
    use super::*;

    /// STRIAHELPSEARCHLIVE M3: with the help shown, `key:s` `key:l` narrow the help (the line carries `"help_query":"sl"`
    /// and the shown count), `key:BackSpace` widens it to `"s"`; nothing is applied (not playing).
    #[test]
    fn striahelpsearchlive_witness_typed_query() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, "help,key:s,key:l");
        assert!(errors.is_empty(), "{errors:?}");
        let n = nle_face::help_matching("sl").len();
        assert_eq!(help_search_json(&f), format!(",\"help_query\":\"sl\",\"help_rows\":{n}"));
        assert!(!f.playing);
        let errors = drive(&mut f, "key:BackSpace");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(f.help_query(), Some("s"));
        assert!(f.help_shown());
        println!("striahelpsearchlive witness: help,key:s,key:l -> ,\"help_query\":\"sl\",\"help_rows\":{n}");
    }
}

// ---- STRIASOURCEINFOKEY (LEDGER SR942): the witness reads the Ctrl+i chord (named tail block) ----
#[cfg(test)]
mod striasourceinfokey_tests {
    use super::*;

    /// STRIASOURCEINFOKEY M3: `at:12:0,key:Ctrl+i` stands reel B's line on `"info"` (the `key:` token, no new code).
    #[test]
    fn striasourceinfokey_key_field() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, "at:12:0,key:Ctrl+i");
        assert!(errors.is_empty(), "{errors:?}");
        let j = info_json(&f);
        assert!(j.starts_with(",\"info\":\"info: B: utp1 64x48 ~") && j.ends_with(" kb/s\""), "{j}");
        println!("striasourceinfokey witness: at:12:0,key:Ctrl+i -> {j}");
    }
}

// ---- STRIANUDGEMORE (LEDGER SR944): the witness's nudge words (named tail block) ----
/// `nudge:<d>` -> the face word `nudge <d>` (timeline_core's `editor::nudge_words` takes the count: signed, nonzero); a bare
/// `nudge…` word that is a KEYS action (`nudge-`, `nudge+`, `nudge-10`, `nudge+10`) -> itself; each as the surface's
/// `Input`. Any other token is `None` (so `nudge:0`, `nudge:x`, `nudge+7` stay bad tokens).
fn nudge_token(tok: &str) -> Option<Vec<SMessage>> {
    let text = match tok.strip_prefix("nudge:") {
        Some(d) => timeline_core::editor::nudge_words(&format!("nudge {d}")).map(|d| format!("nudge {d}"))?,
        None if tok.starts_with("nudge") && nle_face::KEYS.iter().any(|&(_, a)| a == tok) => tok.to_string(),
        None => return None,
    };
    Some(vec![SMessage::Input { target: SURFACE.into(), text }])
}

#[cfg(test)]
mod strianudgemore_tests {
    use super::*;

    /// STRIANUDGEMORE M3: `--do` takes the nudge words: `nudge+` one frame, `nudge+10` ten, `nudge:-3` a free count, each
    /// one commit with the message line's new `at`; `nudge:0`, `nudge:x` and `nudge+7` are bad tokens.
    #[test]
    fn do_takes_the_nudge_words() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "at:30:0").is_empty());
        assert_eq!(face.state.selection, Some((0, 2)));
        assert_eq!(drive(&mut face, "nudge+"), vec!["nudge: at 26 (00:00:01:01)".to_string()]);
        assert_eq!(drive(&mut face, "nudge+10"), vec!["nudge: at 36 (00:00:01:11)".to_string()]);
        assert_eq!(drive(&mut face, "nudge:-3"), vec!["nudge: at 33 (00:00:01:08)".to_string()]);
        assert_eq!(face.state.undo_depth(), 3);
        for t in ["nudge:0", "nudge:x", "nudge+7"] {
            assert_eq!(drive(&mut face, t), vec![format!("bad token {t}")]);
        }
        assert_eq!(face.state.selected().map(|c| c.at), Some(33));
        println!("stria-view strianudgemore witness: nudge+ / nudge+10 / nudge:-3 -> at 33, three commits");
    }
}

// ---- STRIAEXPORTPRESETSMORE (LEDGER SR948): the `Preset` transport button picks the face's export preset (named tail block) ----

#[cfg(test)]
mod striaexportpresetsmore_tests {
    use super::*;

    /// The button just before `?` is `preset` labelled `Preset`, firing the face's `preset` word; driving it (the `--do`
    /// road the click takes) steps THE table in order and back to none, each step a note on the message line, no error.
    #[test]
    fn striaexportpresetsmore_button_picks_the_preset() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let Some(TetraNode::Button { id, label, action }) = row.iter().rev().nth(1) else { panic!("no button before help") };
        assert_eq!((id.as_str(), label.as_str()), ("preset", "Preset"));
        let SMessage::Input { target, text } = action else { panic!("not an Input") };
        assert_eq!((target.as_str(), text.as_str()), (SURFACE, "preset"));
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(f.export_preset(), None);
        for want in ["web", "archive", "proxy"] {
            let errors = drive(&mut f, text);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(f.export_preset().map(|p| p.name), Some(want));
            assert!(f.message_text().is_some_and(|m| m.starts_with(&format!("preset {want}: "))), "{:?}", f.message_text());
        }
        let errors = drive(&mut f, text);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((f.export_preset(), f.message_text().as_deref()), (None, Some("preset none: export's own words")));
        println!("striaexportpresetsmore witness: button {id} {label:?} steps web, archive, proxy, none");
    }
}

// ---- STRIASNAPMORE (LEDGER SR952): the witness's drag snap default reading (named tail block) ----
/// The witness's `"snap"` field, only while the drag snap default is off (`Face::snap_on`); the default line is unchanged.
fn snap_json(face: &Face) -> String {
    if face.snap_on() { String::new() } else { ",\"snap\":false".into() }
}

#[cfg(test)]
mod striasnapmore_tests {
    use super::*;

    /// STRIASNAPMORE M3: the field is absent at the default; `key:n` turns the snap default off (the message line names
    /// it) and the line carries `"snap":false`; the `snap` word turns it back on and the field is absent again.
    #[test]
    fn striasnapmore_snap_field_only_while_off() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(snap_json(&f), "");
        assert_eq!(drive(&mut f, "key:n"), vec!["snap: off".to_string()]);
        assert_eq!(snap_json(&f), ",\"snap\":false");
        assert_eq!(drive(&mut f, "snap"), vec!["snap: on".to_string()]);
        assert_eq!(snap_json(&f), "");
        println!("striasnapmore witness: key:n -> \"snap\":false, snap -> absent");
    }
}

// ---- STRIATRANSITIONMORE (LEDGER SR956): the `dur:HH:MM:SS:FF` token (named tail block) ----

#[cfg(test)]
mod striatransitionmore_tests {
    use super::*;

    /// M3: `transition:0:0,dur:00:00:00:07` gives the selected dissolve 7 ticks at 25 fps (the JSON `"dur":7`, one undo
    /// step); a frame field past the fps (`dur:00:00:00:25`) is refused by name, the state left.
    #[test]
    fn striatransitionmore_witness_dur_timecode_token() {
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
        let mut face = Face::new(EditorState::new(tl0), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "transition:0:0,dur:00:00:00:07");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(transition_json(&face).contains(r#""at":10,"dur":7,"#), "{}", transition_json(&face));
        assert_eq!(face.state.undo_depth(), 1);
        let before = face.state.clone();
        assert_eq!(drive(&mut face, "dur:00:00:00:25"), vec![format!("dur 00:00:00:25: {}", timeline_core::EditError::Transition)]);
        assert_eq!(face.state, before);
    }
}

// ---- STRIAMETERSMORE (LEDGER SR962): the witness's "held" — the meters' standing peak holds (named tail block) ----
/// The witness's `"held"` field: each standing peak hold (`Face::meter_held`) as `{"track":<index or "master">,
/// "peak":<held dBFS>}`, in hold order, only while any hold stands; the default line is unchanged.
fn held_json(face: &Face) -> String {
    let held = face.meter_held();
    if held.is_empty() {
        return String::new();
    }
    let all: Vec<String> = held
        .iter()
        .map(|&(k, db)| format!("{{\"track\":{},\"peak\":{db:.2}}}", k.map(|t| t.to_string()).unwrap_or_else(|| "\"master\"".into())))
        .collect();
    format!(",\"held\":[{}]", all.join(","))
}

#[cfg(test)]
mod striametersmore_tests {
    use super::*;

    /// STRIAMETERSMORE M3: no hold, no field; +24 dB on track 1 played two ticks holds track 1 and the master;
    /// `meters-reset` (through `--do`) clears the field.
    #[test]
    fn held_field_only_while_held() {
        let (tl, src) = nle_face::synth_fixture();
        let face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(held_json(&face), "");
        let (mut tl, src) = nle_face::synth_fixture();
        tl.tracks[1].clips.iter_mut().for_each(|c| *c = c.gained(2400));
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "play,tick:2");
        assert!(errors.is_empty(), "{errors:?}");
        let j = held_json(&face);
        assert!(j.starts_with(",\"held\":[") && j.contains("{\"track\":1,\"peak\":") && j.contains("{\"track\":\"master\",\"peak\":"), "{j}");
        let errors = drive(&mut face, "meters-reset");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(held_json(&face), "");
    }
}

// ---- STRIASLIPVIEWMORE (LEDGER SR946): the witness reads the slip / slide pair's size (named tail block) ----
/// The witness's `"slip"` field, only while the face shows a slip / slide pair (`Face::slip_view`, after the line's own
/// paint); the default line is unchanged.
fn slip_json(face: &Face) -> String {
    face.slip_view().map(|s| format!(",\"slip\":\"{}x{}\"", s.width, s.height)).unwrap_or_default()
}

#[cfg(test)]
mod striaslipviewmore_tests {
    use super::*;

    /// STRIASLIPVIEWMORE (SR946): `at:12:0,slip+` (select B, slip it one frame) holds the pair, and after the witness's
    /// paint (`Face::frames`) the field reads two synth frames side by side; no script: no field.
    #[test]
    fn striaslipviewmore_witness_slip_field() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        face.frames();
        assert_eq!(slip_json(&face), "");
        assert_eq!(drive(&mut face, "at:12:0,slip+"), Vec::<String>::new());
        face.frames();
        assert_eq!(slip_json(&face), ",\"slip\":\"128x48\"");
        assert!(drive(&mut face, "undo").is_empty());
        face.frames();
        assert_eq!(slip_json(&face), "");
        println!("striaslipviewmore witness: at:12:0,slip+ -> ,\"slip\":\"128x48\"; undo -> cleared");
    }
}

// ---- STRIAMARKERSMORE (LEDGER SR966): the Unmark All button and the `mark:<name>` token (named tail block) ----
// The button fires THE `unmark all` word; `mark:<name>` is the witness's spelling of `mark <name>` (a name holding `:` or
// `,` cannot ride a token). Both land on SR835 / SR895's words through `Face::verb`.

#[cfg(test)]
mod striamarkersmore_tests {
    use super::*;

    /// STRIAMARKERSMORE M3: the Unmark All button sits after Title and before help; `mark:<name>` names a marker and, on a
    /// taken tick, renames it; `unmark all` as a token clears them all; bare `mark` stays the VERBS word.
    #[test]
    fn striamarkersmore_button_and_mark_token() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let at = row.iter().position(|b| matches!(b, TetraNode::Button { id, .. } if id == "unmark all")).expect("an unmark all button");
        assert!(matches!(&row[at - 1], TetraNode::Button { id, .. } if id == "title"));
        assert!(matches!(&row[at + 1], TetraNode::Button { id, .. } if id == "help"));
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(matches!(token(&face, "mark").as_deref(), Some([SMessage::Input { text, .. }]) if text == "mark"));
        assert_eq!(drive(&mut face, "mark:Intro"), vec!["marked Intro at 00:00:00:00".to_string()]);
        assert_eq!(drive(&mut face, "ruler:12,mark:Outro"), vec!["marked Outro at 00:00:00:12".to_string()]);
        assert_eq!(drive(&mut face, "mark:Opening"), vec!["renamed Outro to Opening at 00:00:00:12".to_string()]);
        assert_eq!(markers_json(&face), r#","markers":[{"t":0,"name":"Intro"},{"t":12,"name":"Opening"}]"#);
        println!("striamarkersmore witness: {}", markers_json(&face));
        assert_eq!(drive(&mut face, "unmark all"), vec!["unmarked 2 markers".to_string()]);
        assert_eq!(markers_json(&face), "");
    }
}

// ---- STRIAEXPORTQUEUEMORE (LEDGER SR971): the witness's waiting export count (named tail block) ----
/// The witness's `"queued"` field, only while export requests wait (`Face::export_queued`); the default line is unchanged.
fn queued_json(face: &Face) -> String {
    match face.export_queued() {
        0 => String::new(),
        n => format!(",\"queued\":{n}"),
    }
}

#[cfg(test)]
mod striaexportqueuemore_tests {
    use super::*;

    /// STRIAEXPORTQUEUEMORE (SR971): a paused job with one request queued behind it carries `"queued":1`; `export-clear`
    /// drops it and the field is gone; cancelled and joined, nothing is left.
    #[test]
    fn striaexportqueuemore_witness_queued_field() {
        let exe = std::env::current_exe().expect("the test binary");
        let dir = exe.parent().expect("its directory").join("striaexportqueuemore-witness-a");
        let other = exe.parent().expect("its directory").join("striaexportqueuemore-witness-b");
        let _ = std::fs::remove_dir_all(&dir);
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(queued_json(&face), "");
        face.export_paused_at(&dir, "png", 3).unwrap();
        assert!(face.verb(&format!("export {} png", other.display())));
        assert_eq!(queued_json(&face), r#","queued":1"#);
        assert!(face.verb("export-clear"));
        assert_eq!(queued_json(&face), "");
        assert!(face.cancel_export());
        assert!(face.wait_export());
        assert!(!other.exists(), "the dropped request made nothing");
        let _ = std::fs::remove_dir_all(&dir);
        println!("striaexportqueuemore witness: ,\"queued\":1 then cleared");
    }
}

// ---- STRIAFREEZEMORE (LEDGER SR970): the witness's word:arg tokens (named tail block) ----
/// STRIAFREEZEMORE: `freeze:<len>`, `freeze-all:<len>`, `title:<text>`, `retitle:<text>` as the Input word `"<w> <rest>"`, `rest`
/// everything after the FIRST `:` (so `freeze:00:00:01:00` keeps the timecode's colons); `None` for any other token.
fn word_token(tok: &str) -> Option<Vec<SMessage>> {
    let (w, rest) = tok.split_once(':')?;
    matches!(w, "freeze" | "freeze-all" | "title" | "retitle").then(|| vec![SMessage::Input { target: SURFACE.into(), text: format!("{w} {rest}") }])
}

#[cfg(test)]
mod striafreezemore_tests {
    use super::*;

    /// `freeze:00:00:01:00` lays the same 25-tick hold as `freeze:1` (synth_fixture, 25 fps, playhead 5); `freeze:x` is the
    /// freeze word's own refusal by name, not a bad token.
    #[test]
    fn striafreezemore_freeze_timecode_and_seconds_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "ruler:5").is_empty());
        let ph = face.state.playhead;
        let e0 = face.state.timeline.end();
        let errors = drive(&mut face, "freeze:00:00:01:00");
        assert!(errors.is_empty(), "{errors:?}");
        let tc = face.state.timeline.clone();
        assert_eq!(tc.end(), e0 + 25, "a one-second hold");
        assert!(drive(&mut face, "undo").is_empty());
        face.state.seek(ph);
        assert!(drive(&mut face, "freeze:1").is_empty());
        assert_eq!(face.state.timeline, tc, "the timecode and the seconds lay the same hold");
        let errors = drive(&mut face, "freeze:x");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("freeze x: "), "{errors:?}");
        println!("striafreezemore witness: freeze:00:00:01:00 == freeze:1 (+25 ticks); freeze:x -> {}", errors[0]);
    }

    /// `title:Hello world` lays T1 "Hello world"; `retitle:Bye: now` retypes it (the colon after the first kept).
    #[test]
    fn striafreezemore_title_and_retitle_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let text = |f: &Face| f.state.timeline.source_by_reel("T1").and_then(|s| s.title.clone());
        let errors = drive(&mut face, "ruler:12,title:Hello world");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(text(&face).as_deref(), Some("Hello world"));
        let errors = drive(&mut face, "retitle:Bye: now");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(text(&face).as_deref(), Some("Bye: now"));
        assert_eq!(drive(&mut face, "nope:x"), vec!["bad token nope:x".to_string()]);
        println!("striafreezemore witness: title:Hello world -> T1, retitle:Bye: now -> {:?}", text(&face));
    }
}

// ---- STRIACROPMORE (LEDGER SR960): the witness's crop drag on the preview picture (named tail block) ----
#[cfg(test)]
mod striacropmore_tests {
    use super::*;

    /// STRIACROPMORE (SR960): `press` on the preview picture's left edge, `move` a tenth of the picture right, `release`:
    /// the selected clip's left side is cropped 100 (one undo step), through the witness's existing pointer tokens.
    #[test]
    fn striacropmore_witness_crop_drag() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "at:12:0").is_empty());
        let _ = face.paint();
        let s = face.still().expect("a preview still").clone();
        let l = face.layout();
        let q = timeline_core::SizePolicy::Letterbox.place(s.width as usize, s.height as usize, l.w as usize, l.preview_h as usize).dst;
        let (x, y, dx) = (q.x as f64, (q.y + q.h / 2) as f64, q.w as f64 / 10.0);
        let errors = drive(&mut face, &format!("press:{x}:{y},move:{}:{y},release", x + dx));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.selected().map(|c| c.crop), Some(timeline_core::Crop { l: 100, t: 0, r: 0, b: 0 }));
        assert_eq!(face.state.undo_depth(), 1);
        assert!(face.crop_dragging().is_none());
    }
}

// ---- STRIALINKMORE (LEDGER SR967): the `alone` button through `--do` (named tail block) ----
#[cfg(test)]
mod strialinkmore_tests {
    use super::*;

    /// `--do alone` with nothing selected names its refusal; on a linked pair it keeps the primary alone in the set with
    /// no undo step; the button sits before `help`.
    #[test]
    fn strialinkmore_alone_button() {
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, "alone");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("no such clip"), "{errors:?}");
        f.state.select(0, 0).unwrap();
        f.state.select_add(1, 0).unwrap();
        assert!(drive(&mut f, "link").is_empty());
        f.state.select(0, 0).unwrap();
        assert_eq!(f.state.set, vec![(0, 0), (1, 0)]);
        let errors = drive(&mut f, "alone");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((f.state.selection, f.state.set.clone()), (Some((0, 0)), vec![(0, 0)]));
        assert_eq!(f.state.undo_depth(), 1, "link only: alone is no edit");
        let TetraNode::VStack(top) = editor_tree() else { panic!("not a VStack") };
        let TetraNode::HStack(row) = &top[1] else { panic!("no transport row") };
        let ids: Vec<&str> = row.iter().map(|b| match b { TetraNode::Button { id, .. } => id.as_str(), _ => "" }).collect();
        assert_eq!(&ids[ids.len() - 2..], ["alone", "help"]);
    }
}

// ---- STRIAGAPSMORE (LEDGER SR969): the witness's Close All button and `close-gaps:sync` token (named tail block) ----
#[cfg(test)]
mod striagapsmore_tests {
    use super::*;

    /// The Close All button sits right before Gap (Gap, Close Gaps, help stay last); its word `close-all` is the argument
    /// word `close-gaps all`; `close-gaps:sync` closes the spans empty on every track, the timeline_core op by hand.
    #[test]
    fn striagapsmore_witness_button_and_token() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let ids: Vec<String> = row.iter().filter_map(|b| if let TetraNode::Button { id, .. } = b { Some(id.clone()) } else { None }).collect();
        let i = ids.iter().position(|x| x == "close-all").expect("close-all button");
        assert_eq!(ids[i + 1], "gap");
        let (tl, src) = nle_face::synth_fixture();
        let tl0 = tl.clone();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "close-all").is_empty());
        assert_eq!(face.state.timeline, tl0.close_gaps_all().unwrap());
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(drive(&mut face, "close-gaps:sync").is_empty());
        assert_eq!(face.state.timeline, tl0.close_gaps_sync().unwrap());
        assert_eq!(tl0.sync_gaps(), vec![(20, 25)]);
        println!("striagapsmore witness: close-gaps:sync -> {:?}", face.state.timeline.tracks[0].clips);
    }
}

// ---- STRIAXFORMMORE (LEDGER SR975): the witness's nudge tokens (named tail block) ----
#[cfg(test)]
mod striaxformmore_tests {
    use super::*;

    /// STRIAXFORMMORE (SR975): `pip:down` / `pip:right` / `pip:left` / `pip:up` are timeline_core's nudge words on the
    /// selected clip, one undo step each, no error; a nudge past the edge is refused by name and commits nothing.
    #[test]
    fn striaxformmore_witness_nudge_tokens() {
        let (tl, src) = nle_face::synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut face, "at:12:0,pip:tl,pip:down,pip:right");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(face.state.undo_depth(), 3);
        assert_eq!(face.state.selected().map(|c| c.xform), Some(timeline_core::Xform { x: 50, y: 50, s: 250 }));
        assert!(drive(&mut face, "pip:up,pip:up").is_empty());
        assert_eq!(drive(&mut face, "pip:up"), vec![format!("pip up: {}", timeline_core::EditError::Xform)]);
        assert_eq!(face.state.undo_depth(), 5);
    }
}

// ---- STRIAHELPMORE (LEDGER SR977): the witness's help query field on the bar (named tail block) ----
#[cfg(test)]
mod striahelpmore_tests {
    use super::*;

    /// STRIAHELPMORE M3: the transport row's field `HELP_FIELD` sits just before the `?` button and fires ConsoleInput;
    /// `--do find:slip` (its Enter) prints the `help:slip` line; `find:zzz` is refused by name; a blank `find:` toggles.
    #[test]
    fn striahelpmore_witness_help_field() {
        let TetraNode::HStack(row) = transport() else { panic!("no transport row") };
        let Some(TetraNode::TextField { id, action, secret, .. }) = row.get(row.len() - 2) else { panic!("no field before ?") };
        assert_eq!((id.as_str(), *secret), (nle_face::HELP_FIELD, false));
        assert!(matches!(action, quartzite::tetra::TextAction::ConsoleInput));
        let (tl, src) = nle_face::synth_fixture();
        let mut word = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(matches!(token(&word, "find:slip").as_deref(), Some([SMessage::ConsoleInput(q)]) if q == "slip"));
        assert!(drive(&mut word, "help:slip").is_empty());
        let (tl, src) = nle_face::synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let errors = drive(&mut f, "find:slip");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((help_json(&f), help_search_json(&f)), (help_json(&word), help_search_json(&word)));
        assert_eq!(f.help_query(), Some("slip"));
        let errors = drive(&mut f, "find:zzz");
        assert_eq!(errors, vec!["help: no key matches \"zzz\"".to_string()]);
        assert_eq!((f.help_shown(), f.help_query()), (true, Some("slip")));
        let errors = drive(&mut f, "find:");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((f.help_shown(), f.help_query()), (false, None));
        println!("striahelpmore witness: find:slip -> {}", help_search_json(&word));
    }
}
