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
//! and the selected clip as `clip: "<at>:<src_in>:<src_out>"`.

use bandy::{SMessage, Synapse};
use quartzite::tetra::TetraNode;
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
}

const USAGE: &str = "usage: stria-view <edl> [--fps N] --src REEL=<media> ... | --synth  [--size WxH] [--do SCRIPT] [--out shot.rgba] [--labels]  (SCRIPT: verbs, at:T:ROW, ruler:T, press:X:Y, move:X:Y, release, key:NAME, wheel:N)";

fn die(msg: String) -> ! {
    eprintln!("stria-view: {msg}");
    std::process::exit(1)
}

fn cli() -> Cli {
    let mut c = Cli { edl: None, synth: false, fps: 25, srcs: Vec::new(), size: (960, 540), script: None, out: None, labels: false };
    let mut a = std::env::args().skip(1);
    while let Some(arg) = a.next() {
        match arg.as_str() {
            "--synth" => c.synth = true,
            "--labels" => c.labels = true,
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
    let button = |verb: &str, label: &str| TetraNode::Button {
        id: verb.into(),
        label: label.into(),
        action: SMessage::Input { target: SURFACE.into(), text: verb.into() },
    };
    TetraNode::VStack(vec![
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
        ]),
    ])
}

/// The pixel order the face's surface reads: GTK's hands its buffer to cairo ARgb32 (BGRA on little-endian), the
/// AppKit surface and the headless witness take RGBA (NLE2-ORDER owes one Quartzite contract).
fn surface_order() -> PixelOrder {
    if cfg!(all(target_os = "linux", feature = "gtk")) { PixelOrder::Bgra } else { PixelOrder::Rgba }
}

/// A `--do` token as the messages the window would send.
fn token(face: &Face, tok: &str) -> Option<Vec<SMessage>> {
    let num = |s: &str| s.parse::<i64>().ok();
    let px = |s: &str| s.parse::<f64>().ok();
    let l: Layout = face.layout();
    let (lx, ly) = face.pointer().unwrap_or((0.0, 0.0));
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
            SMessage::BrowserClick(x, y)
        }
        ["ruler", t] => SMessage::BrowserClick(l.x_of(num(t)?) as f64 + 0.5, l.ruler_y as f64 + 1.0),
        ["press", x, y] => SMessage::BrowserButton { down: true, x: px(x)?, y: px(y)? },
        ["move", x, y] => SMessage::BrowserMouseMove(px(x)?, px(y)?),
        ["release"] => return Some(vec![SMessage::BrowserButton { down: false, x: lx, y: ly }, SMessage::BrowserClick(lx, ly)]),
        ["wheel", n] => SMessage::BrowserScroll(0.0, -nle_face::WHEEL_STEP * px(n)?),
        [v] if nle_face::VERBS.contains(v) => SMessage::Input { target: SURFACE.into(), text: v.to_string() },
        _ => return None,
    };
    Some(vec![one])
}

/// The script through `Face::on`, token by token; the errors it met.
fn drive(face: &mut Face, script: &str) -> Vec<String> {
    let mut errors = Vec::new();
    for tok in script.split(',').filter(|t| !t.is_empty()) {
        match token(face, tok) {
            Some(ms) => {
                for m in ms {
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
    let mut errors = drive(&mut face, c.script.as_deref().unwrap_or(""));
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
    println!(
        "{{\"playhead\":{},\"tracks\":{},\"clips\":{},\"selection\":{sel},\"clip\":{clip},\"playing\":{},\"rate\":{},\"undo\":{},\"zoom\":{},\"w\":{width},\"h\":{height},\"fnv\":\"{:016x}\",\"errors\":\"{err}\"}}",
        st.playhead,
        st.timeline.tracks.len(),
        clips,
        face.playing,
        face.rate,
        st.undo_depth(),
        st.zoom,
        nle_face::fnv1a(&pixels)
    );
    0
}

fn main() {
    let c = cli();
    let (tl, sources) = load(&c);
    let windowed = cfg!(any(target_os = "macos", all(target_os = "linux", feature = "gtk")));
    if c.script.is_some() || c.out.is_some() || !windowed {
        let face = Face::new(EditorState::new(tl), sources, c.size, PixelOrder::Rgba, c.labels);
        std::process::exit(witness(&c, face));
    }

    let title = c.edl.as_deref().and_then(|p| std::path::Path::new(p).file_name()).and_then(|s| s.to_str()).unwrap_or("synthetic").to_string();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let synapse = Synapse::new();
    {
        let rx = synapse.subscribe();
        let face = Face::new(EditorState::new(tl), sources, c.size, surface_order(), true);
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
}
