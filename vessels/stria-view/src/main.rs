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

use bandy::{SMessage, Synapse};
use quartzite::tetra::{SurfaceMap, TetraNode};
use stria::nle::Sources;
use stria::nle_face::{self, EditorState, Face, Layout, PixelOrder, SURFACE};
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
}

/// SURFACEID (SR332): the second Surface of `--two-surfaces` (no painter: a press on it must not reach the editor).
const AUX_SURFACE: &str = "stria.nle.aux";

const USAGE: &str = "usage: stria-view <edl> [--fps N] --src REEL=<media> ... | --synth  [--size WxH] [--do SCRIPT] [--out shot.rgba] [--labels] [--two-surfaces]  (SCRIPT: verbs, at:T:ROW, ruler:T, press:X:Y, move:X:Y, release, key:NAME, wheel:N, marquee:X0:Y0:X1:Y1, shift:TOKEN, alt:TOKEN, transition:T:I, meters, aux:TOKEN)";

fn die(msg: String) -> ! {
    eprintln!("stria-view: {msg}");
    std::process::exit(1)
}

fn cli() -> Cli {
    let mut c = Cli { edl: None, synth: false, fps: 25, srcs: Vec::new(), size: (960, 540), script: None, out: None, labels: false, two: false };
    let mut a = std::env::args().skip(1);
    while let Some(arg) = a.next() {
        match arg.as_str() {
            "--synth" => c.synth = true,
            "--labels" => c.labels = true,
            "--two-surfaces" => c.two = true,
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
    if c.edl.is_none() && !c.synth {
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
    let button = |verb: &str, label: &str| TetraNode::Button {
        id: verb.into(),
        label: label.into(),
        action: SMessage::Input { target: SURFACE.into(), text: verb.into() },
    };
    let aux = two.then(|| TetraNode::Surface { id: AUX_SURFACE.into() });
    TetraNode::VStack(aux.into_iter().chain([
        TetraNode::Surface { id: SURFACE.into() },
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
        ]),
    ]).collect())
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
        match if tok == "meters" { face.feed_meters(face.state.playhead, 1); Some(Vec::new()) } else { token(face, tok) } {
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
    let map = SurfaceMap::of(&editor_tree_for(c.two));
    let surfaces = c.two.then(|| (map.number(SURFACE).unwrap_or(0), map.number(AUX_SURFACE).unwrap_or(0)));
    let mut presses = Vec::new();
    let mut errors = drive_on(&mut face, c.script.as_deref().unwrap_or(""), surfaces, &mut presses);
    let SMessage::SurfaceBlit { width, height, pixels, .. } = face.frame() else { return 1 };
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
    let meters = meters_json(c, &face);
    let presses = presses_json(c, &presses);
    println!(
        "{{\"playhead\":{},\"tracks\":{},\"clips\":{},\"selection\":{sel},\"clip\":{clip},\"playing\":{},\"rate\":{},\"undo\":{},\"zoom\":{},\"w\":{width},\"h\":{height},\"fnv\":\"{:016x}\",\"errors\":\"{err}\",\"set\":{},\"transition\":{}{meters}{presses}}}",
        st.playhead,
        st.timeline.tracks.len(),
        clips,
        face.playing,
        face.rate,
        st.undo_depth(),
        st.zoom,
        nle_face::fnv1a(&pixels),
        st.set.len(),
        transition_json(&face)
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
    let editor = SurfaceMap::of(&editor_tree_for(c.two)).number(SURFACE).unwrap_or(0); // SURFACEID (SR332): the face's Surface
    if c.script.is_some() || c.out.is_some() || c.two || !windowed {
        let face = Face::new(EditorState::new(tl), sources, c.size, PixelOrder::Rgba, c.labels).on_surface(editor);
        std::process::exit(witness(&c, face));
    }

    let title = c.edl.as_deref().and_then(|p| std::path::Path::new(p).file_name()).and_then(|s| s.to_str()).unwrap_or("synthetic").to_string();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let synapse = Synapse::new();
    {
        let rx = synapse.subscribe();
        let face = Face::new(EditorState::new(tl), sources, c.size, PixelOrder::Rgba, true).on_surface(editor);
        rt.spawn(nle_face::serve(synapse.clone(), rx, face));
    }
    quartzite::Backend::new_tetra_vessel(
        "org.unaos.stria",
        &format!("Stria — {title}"),
        (c.size.0 as f64, c.size.1 as f64 + 40.0),
        editor_tree(),
        synapse.clone(),
    )
    .run();
    drop(rt);
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
        assert_eq!(transition_json(&face), r#"{"track":0,"idx":0,"at":10,"dur":5,"kind":"D","from_black":false,"still":"64x48"}"#);
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
        let c = Cli { edl: None, synth: true, fps: 25, srcs: Vec::new(), size: (320, 160), script: None, out: None, labels: false, two: true };
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
        "{{\"track\":{t},\"idx\":{i},\"at\":{},\"dur\":{},\"kind\":\"{kind}\",\"from_black\":{},\"still\":{still}}}",
        x.at, x.dur, x.from_black
    )
}

/// SURFACEID (SR332): the witness's `"presses"` field, with `--two-surfaces` only (the default line is unchanged).
fn presses_json(c: &Cli, presses: &[(u32, bool)]) -> String {
    if !c.two {
        return String::new();
    }
    let all: Vec<String> = presses.iter().map(|(s, t)| format!("[{s},{t}]")).collect();
    format!(",\"presses\":[{}]", all.join(","))
}
