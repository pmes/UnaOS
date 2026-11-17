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
//! that tick) and `ruler:<tick>` (a click on the ruler). NLE2B (SR208), the gestures as a face sends them:
//! `press:<x>:<y>` / `move:<x>:<y>` (surface pixels), `release` (at the last pointer: `BrowserButton` up + `BrowserClick`),
//! `key:<name>` (as GTK sends it: `BrowserKey`, plus `BrowserText` for a one-character key; `key:space` types a space)
//! and `wheel:<n>` (n notches at the last pointer, n > 0 up = zoom in). The JSON line also carries `playing`, `rate`
//! and the selected clip as `clip: "<at>:<src_in>:<src_out>"`. NLE2c (SR217): `shift:<token>` / `alt:<token>` set
//! `MOD_SHIFT` / `MOD_ALT` in the `mods` of every press the token sends (NLE2-MODS SR306, as both faces send them: add to
//! the set / no snapping), `marquee:<x0>:<y0>:<x1>:<y1>` is a press, a move and the release, `key:Ctrl+a` /
//! `key:Meta+a` is the `BrowserSelectAll` both faces send for it, and the JSON line ends with the set's size `set`.
//! NLE3d (SR302): `transition:<track>:<idx>` clicks that transition's widget (selecting it; `delete`, `[` / `]` then edit
//! it), and the JSON line ends with `transition` (the selected one and its thumbnail's size, or `null`).
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
}

/// SURFACEID (SR332): the second Surface of `--two-surfaces` (no painter: a press on it must not reach the editor).
const AUX_SURFACE: &str = "stria.nle.aux";

const USAGE: &str = "usage: stria-view <edl> [--fps N] --src REEL=<media> ... | --synth  [--size WxH] [--do SCRIPT] [--out shot.rgba] [--labels] [--two-surfaces | --split] [--mute]  (SCRIPT: verbs, at:T:ROW, ruler:T, press:X:Y, move:X:Y, release, key:NAME, wheel:N, marquee:X0:Y0:X1:Y1, shift:TOKEN, alt:TOKEN, transition:T:I, meters, tick:N, aux:TOKEN)";

fn die(msg: String) -> ! {
    eprintln!("stria-view: {msg}");
    std::process::exit(1)
}

fn cli() -> Cli {
    let mut c = Cli { edl: None, synth: false, fps: 25, srcs: Vec::new(), size: (960, 540), script: None, out: None, labels: false, two: false, split: false, mute: false };
    let mut a = std::env::args().skip(1);
    while let Some(arg) = a.next() {
        match arg.as_str() {
            "--synth" => c.synth = true,
            "--labels" => c.labels = true,
            "--two-surfaces" => c.two = true,
            "--split" => c.split = true,
            "--mute" => c.mute = true, // NLEPLAYAUDIO (SR676): the window plays no sound
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
            "--do" => c.script = a.next(),
            "--out" => c.out = a.next(),
            "--help" | "-h" => {
                eprintln!("{USAGE}");
                std::process::exit(0);
            }
            _ if !arg.starts_with('-') => c.edl = Some(arg),
            _ => die(USAGE.into()),
        }
    }
    if (c.edl.is_none() && !c.synth) || (c.two && c.split) {
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
    for s in &tl.sources {
        let Some((_, p)) = c.srcs.iter().find(|(r, _)| *r == s.reel) else { die(format!("no --src for reel {}", s.reel)) };
        let bytes = std::fs::read(p).unwrap_or_else(|e| die(format!("{p}: {e}")));
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
    }
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
        [w @ ("kind" | "ease"), arg] => SMessage::Input { target: SURFACE.into(), text: format!("{w} {arg}") }, // NLETRANSITIONFACE (SR691): `kind:W007`, `ease:in`
        ["help"] => SMessage::Input { target: SURFACE.into(), text: "help".into() }, // NLE5DKEYSHELP (SR744) / NLE5DKEYSHELPBUTTON (SR765): the keys help word and button
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
        match if tok == "meters" { face.feed_meters(face.state.playhead, 1); Some(Vec::new()) } else { tick_token(face, tok).or_else(|| token(face, tok)) } { // STRIAINFOREELHOVERDELAY (SR789): `tick:<n>` steps the face clock n times (file tail)
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
    let blits = face.frames();
    let fnvs = blits_json(c, &blits);
    let Some(SMessage::SurfaceBlit { width, height, pixels, .. }) = blits.into_iter().last() else { return 1 };
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
    let clip = st.selected().map(|c| format!("\"{}:{}:{}\"", c.at, c.src_in, c.src_out)).unwrap_or_else(|| "null".into());
    let err = errors.join("; ").replace('"', "'");
    let meters = meters_json(c, &face) + &help_json(&face); // NLE5DKEYSHELP (SR744): `"help":true` while the keys help shows (file tail)
    let presses = presses_json(c, &presses); let export = export_json(&face) + &range_json(&face) + &reel_json(&face); // NLEFACEEXPORT (SR694): the running export's reading (file tail); NLE5DEXPORTRANGE (SR717): the marks' range (file tail); STRIAINFOREEL (SR743): the selected reel (file tail)
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
        let face = route(&c, Face::new(EditorState::new(tl), sources, c.size, PixelOrder::Rgba, c.labels).with_doc(c.edl.as_deref().map(std::path::Path::new)));
        std::process::exit(witness(&c, face));
    }

    let title = c.edl.as_deref().and_then(|p| std::path::Path::new(p).file_name()).and_then(|s| s.to_str()).unwrap_or("synthetic").to_string();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let synapse = Synapse::new();
    let mut engine: Option<Box<dyn std::any::Any>> = None; // NLEPLAYAUDIO (SR676): the audio engine, held for the process life
    {
        let rx = synapse.subscribe();
        let face = play_audio(&c, &mut engine, route(&c, Face::new(EditorState::new(tl), sources, c.size, PixelOrder::Rgba, true).with_doc(c.edl.as_deref().map(std::path::Path::new))));
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
        assert_eq!(row.len(), nle_face::VERBS.len());
        for b in row {
            let TetraNode::Button { action, .. } = b else { panic!("not a button") };
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
        let c = Cli { edl: None, synth: true, fps: 25, srcs: Vec::new(), size: (320, 160), script: None, out: None, labels: false, two: true, split: false, mute: false };
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
        button("in", "Range In"), button("out", "Range Out"), // NLE5DEXPORTRANGE (SR717): the export range's marks at the playhead
        button("kind", "Kind"), button("ease", "Ease"), // NLETRANSITIONFACE (SR691): the selected transition's kind / ease cycle
        button("slip-", "Slip -"), button("slip+", "Slip +"), button("slide-", "Slide -"), button("slide+", "Slide +"), // NLE3ESLIPFACE (SR737): one-frame slip / slide of the selected clip
        button("roll-", "Roll -"), button("roll+", "Roll +"), button("roll-tool", "Roll tool"), // STRIAROLLEDIT (SR851): one-frame roll of the cut at the selected clip's out edge; STRIAROLLMORE (SR954): the roll tool on / off
        button("help", "?"), // NLE5DKEYSHELPBUTTON (SR765): the keys help overlay on / off (THE `help` verb, like `?`)
    ])
}

/// NLE2-SPLIT (SR339): the preview Surface (number 0) over the transport row, the tracks Surface (number 1) under it.
fn split_tree() -> TetraNode {
    TetraNode::VStack(vec![TetraNode::Surface { id: PREVIEW_SURFACE.into() }, transport(), TetraNode::Surface { id: TRACKS_SURFACE.into() }])
}

/// The tree this run declares: `--split`'s, else the editor tree (with `--two-surfaces` its aux Surface).
fn tree(c: &Cli) -> TetraNode {
    if c.split { split_tree() } else if c.two { editor_tree_for(true) } else { editor_tree() }
}

/// The face routed by the tree's `SurfaceMap`: split over the preview and tracks Surfaces, else on the editor's.
fn route(c: &Cli, face: Face) -> Face {
    let map = SurfaceMap::of(&tree(c));
    if c.split {
        face.on_surfaces(map.number(PREVIEW_SURFACE).unwrap_or(0), map.number(TRACKS_SURFACE).unwrap_or(1))
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
        let c = Cli { edl: None, synth: true, fps: 25, srcs: Vec::new(), size: (320, 160), script: None, out: None, labels: false, two: false, split: true, mute: false };
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
    /// absent); `key:?` shows it; a following `key:j` only dismisses it (no shuttle: not playing, rate 1).
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
        let errors = drive(&mut f, "key:j");
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
        face.tick();
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
