// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Stria's editor face (NLE2, LEDGER SR201): the model, the one painter and the bus service behind every native face.
//!
//! CHARTER: Stria — the non-linear editor's face model over `timeline_core` (shared-core, BP-handlers-4/-14).
//!
//! * [`EditorState`] (timeline_core's `editor` since NLE6 SR207, re-exported here): the timeline, the selected clip, the playhead, the zoom (px per tick) and the undo / redo stacks.
//!   Every edit is ONE of timeline_core's pure ops (`split`, `trim_in`, `trim_out`, `ripple_delete`) and goes through
//!   ONE `commit`: the op's returned `Timeline` replaces the current one, which goes on the undo stack (undo is the
//!   stack of returned values; there are no inverse ops). A refused edit leaves the state unchanged.
//! * [`Layout`]: ONE geometry (preview band on top, the ruler, one row per track) read by both [`paint`] and
//!   [`Layout::hit`], so a click lands on what was drawn.
//! * [`paint`]: the face's pixels, host-side, in the order the face's surface reads ([`PixelOrder`]): clips as
//!   reel-coloured rects named through `text_host` (optional: a golden paints without glyphs), the selected clip's
//!   outline and trim handles, the ruler, the playhead and the preview frame.
//! * [`preview`]: the frame at a tick is `stria::nle::render` of that one tick (no decoder or seek of its own).
//! * [`Face`] + [`serve`]: the bus side. The vessel's tetra tree (one `Surface` [`SURFACE`] and a row of buttons
//!   firing `SMessage::Input { target: SURFACE, text: <verb> }`) is evaluated by Quartzite's `eval_tetra` on AppKit and
//!   GTK; the surface's `BrowserClick` / `BrowserResize` and the buttons' verbs come in, `SurfaceBlit`s go out.
//! * [`Drag`] + [`KEYS`] (NLE2B, LEDGER SR208): the gestures. A `BrowserButton` press on a drawn trim handle or a clip
//!   body begins a drag, `BrowserMouseMove` updates it (a preview timeline is painted, nothing committed), the release
//!   commits ONE `trim` / `move_selected`, Esc cancels; the wheel zooms about the pointer's tick; J/K/L shuttle on the
//!   serve clock, I/O trim at the playhead, and the other keys map onto the transport verbs.
//! * NLE2c (LEDGER SR217): the selection is a SET (`EditorState::set`, `selection` its primary): a shift-press adds, a
//!   marquee from an empty track spot selects what it crosses ([`Layout::crossing`]), `BrowserSelectAll` takes all; delete
//!   / lift / a drag / the arrows edit the set through ONE of timeline_core's set ops; a drag's moving edge snaps to clip
//!   edges and the playhead within [`SNAP_PX`] (the guide line painted by `paint_drag`); a press on the ruler scrubs.
//!   NLE2-MODS (LEDGER SR306): shift-add and the no-snap Alt / Option come on the press itself (`BrowserButton`'s
//!   `mods`, bandy's `MOD_SHIFT` / `MOD_ALT`), so both faces have them and no keydown latch exists.

use std::time::Duration;

use bandy::{SMessage, Synapse};
pub use text_host::line::PixelOrder;
use text_host::line::TextStyle;
use timeline_core::{Clip, EditError, Kind, Timeline, Transition, TransitionKind, WipeDir, edl};

use crate::nle::{MeterLevel, MixMeters, RenderError, RenderSink, Sources, render, synth_source};

/// The editor surface's id (the tetra `Surface` and every `SurfaceBlit` / `Input` target).
pub const SURFACE: &str = "stria.nle";
/// NLE6 (SR207): the editor state is timeline_core's (`timeline_core::editor`), shared with the on-UnaOS face.
pub use timeline_core::editor::{Edge, EditorState, UNDO_MAX, ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};

// ---------------------------------------------------------------------------------------------------------------------
// THE ONE GEOMETRY
// ---------------------------------------------------------------------------------------------------------------------

/// The ruler band's height (px).
pub const RULER_H: u32 = 16;
/// One track row's height (px).
pub const ROW_H: u32 = 40;
/// The trim handle's width (px).
pub const HANDLE_W: i64 = 4;

/// Where everything is on a `w` x `h` canvas: the preview band `0..preview_h`, the ruler, then one row per track.
/// The track area shows ticks from `t0` (the page holding the playhead).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub w: u32,
    pub h: u32,
    pub preview_h: u32,
    pub ruler_y: u32,
    pub tracks_y: u32,
    pub rows: usize,
    pub zoom: u32,
    pub t0: i64,
}

/// What a point on the canvas is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Ruler(i64),
    /// `(track, clip index, tick)`.
    Clip(usize, usize, i64),
    /// An empty spot on a track: `(track, tick)`.
    Track(usize, i64),
    None,
}

impl Layout {
    pub fn of(w: u32, h: u32, st: &EditorState) -> Layout {
        let rows = st.timeline.tracks.len();
        let below = RULER_H + rows as u32 * ROW_H;
        let preview_h = h.saturating_sub(below);
        let zoom = st.zoom.max(1);
        let per_page = (w / zoom).max(1) as i64;
        let t0 = st.playhead.max(0) / per_page * per_page;
        Layout { w, h, preview_h, ruler_y: preview_h, tracks_y: preview_h + RULER_H, rows, zoom, t0 }
    }
    /// The x of tick `t`'s left edge.
    pub fn x_of(&self, t: i64) -> i64 {
        (t - self.t0) * self.zoom as i64
    }
    /// The tick under x.
    pub fn tick_at(&self, x: f64) -> i64 {
        self.t0 + (x.max(0.0) as i64) / self.zoom as i64
    }
    /// The y of track row `row`'s top.
    pub fn row_y(&self, row: usize) -> i64 {
        self.tracks_y as i64 + row as i64 * ROW_H as i64
    }
    /// The centre of tick `t` on track row `row` (what a click on that clip sends).
    pub fn point(&self, row: usize, t: i64) -> (f64, f64) {
        (self.x_of(t) as f64 + self.zoom as f64 / 2.0, self.row_y(row) as f64 + ROW_H as f64 / 2.0)
    }
    pub fn hit(&self, tl: &Timeline, x: f64, y: f64) -> Hit {
        if x < 0.0 || y < 0.0 || x >= self.w as f64 || y >= self.h as f64 || y < self.ruler_y as f64 {
            return Hit::None;
        }
        let t = self.tick_at(x);
        if y < self.tracks_y as f64 {
            return Hit::Ruler(t);
        }
        let row = ((y - self.tracks_y as f64) / ROW_H as f64) as usize;
        if row >= self.rows {
            return Hit::None;
        }
        match tl.clip_at(row, t) {
            Some(i) => Hit::Clip(row, i, t),
            None => Hit::Track(row, t),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// THE ONE PAINTER
// ---------------------------------------------------------------------------------------------------------------------

/// A preview frame: `width * height * 4` RGBA bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

const BG: [u8; 4] = [0x1e, 0x1e, 0x24, 255];
const FIELD: [u8; 4] = [0x00, 0x00, 0x00, 255];
const RULER: [u8; 4] = [0x2a, 0x2a, 0x33, 255];
const TICK: [u8; 4] = [0x88, 0x88, 0x99, 255];
const ROW: [[u8; 4]; 2] = [[0x26, 0x26, 0x2e, 255], [0x2c, 0x2c, 0x35, 255]];
const SELECT: [u8; 4] = [0xff, 0xff, 0xff, 255];
const HANDLE: [u8; 4] = [0xff, 0xd0, 0x40, 255];
const PLAYHEAD: [u8; 4] = [0xff, 0x40, 0x40, 255];
const LABEL: [u8; 4] = [0xff, 0xff, 0xff, 255];
const REEL: [[u8; 3]; 6] =
    [[0x4a, 0x7d, 0xc9], [0xc9, 0x7d, 0x4a], [0x5a, 0xa8, 0x6a], [0xa8, 0x5a, 0xa0], [0xb8, 0xa8, 0x4a], [0x4a, 0xa8, 0xa8]];

/// A clip's colour: its reel's, darker on an audio track.
fn clip_colour(src: u32, kind: Kind) -> [u8; 4] {
    let c = REEL[src as usize % REEL.len()];
    match kind {
        Kind::Video => [c[0], c[1], c[2], 255],
        Kind::Audio => [c[0] / 4 * 3, c[1] / 4 * 3, c[2] / 4 * 3, 255],
    }
}

/// A canvas in one pixel order.
struct Canvas {
    w: u32,
    h: u32,
    order: PixelOrder,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32, order: PixelOrder) -> Canvas {
        Canvas { w, h, order, px: vec![0; w as usize * h as usize * 4] }
    }
    fn put(&self, c: [u8; 4]) -> [u8; 4] {
        match self.order {
            PixelOrder::Rgba => c,
            PixelOrder::Bgra => [c[2], c[1], c[0], c[3]],
        }
    }
    /// Fill `x0..x1` x `y0..y1`, clipped to the canvas.
    fn rect(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: [u8; 4]) {
        let (x0, x1) = (x0.clamp(0, self.w as i64) as usize, x1.clamp(0, self.w as i64) as usize);
        let (y0, y1) = (y0.clamp(0, self.h as i64) as usize, y1.clamp(0, self.h as i64) as usize);
        let c = self.put(c);
        for y in y0..y1 {
            let row = y * self.w as usize * 4;
            for x in x0..x1 {
                self.px[row + x * 4..row + x * 4 + 4].copy_from_slice(&c);
            }
        }
    }
    /// A 1 px outline of `x0..x1` x `y0..y1`.
    fn frame(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: [u8; 4]) {
        self.rect(x0, y0, x1, y0 + 1, c);
        self.rect(x0, y1 - 1, x1, y1, c);
        self.rect(x0, y0, x0 + 1, y1, c);
        self.rect(x1 - 1, y0, x1, y1, c);
    }
    /// `s` nearest-scaled to fit (aspect kept) and centred in `x0..x0+bw` x `y0..y0+bh`.
    fn blit_fit(&mut self, s: &Still, x0: u32, y0: u32, bw: u32, bh: u32) {
        if s.width == 0 || s.height == 0 || bw == 0 || bh == 0 || s.rgba.len() < s.width as usize * s.height as usize * 4 {
            return;
        }
        let k = (bw as f64 / s.width as f64).min(bh as f64 / s.height as f64);
        let (dw, dh) = (((s.width as f64 * k) as u32).max(1), ((s.height as f64 * k) as u32).max(1));
        let (ox, oy) = (x0 + (bw - dw.min(bw)) / 2, y0 + (bh - dh.min(bh)) / 2);
        for dy in 0..dh {
            let (y, sy) = (oy + dy, dy as u64 * s.height as u64 / dh as u64);
            if y >= self.h {
                break;
            }
            for dx in 0..dw {
                let (x, sx) = (ox + dx, dx as u64 * s.width as u64 / dw as u64);
                if x >= self.w {
                    break;
                }
                let si = (sy as usize * s.width as usize + sx as usize) * 4;
                let p = self.put([s.rgba[si], s.rgba[si + 1], s.rgba[si + 2], s.rgba[si + 3]]);
                let di = (y as usize * self.w as usize + x as usize) * 4;
                self.px[di..di + 4].copy_from_slice(&p);
            }
        }
    }
    /// One line of text through text_host, clipped to columns `clip`.
    fn text(&mut self, style: &TextStyle, s: &str, x: f32, baseline: f32, clip: (i32, i32)) {
        let (w, h, order) = (self.w, self.h, self.order);
        style.shape(s).paint(&mut self.px, w, h, w as usize * 4, order, x, baseline, LABEL, clip);
    }
}

/// Paint the editor on a `w` x `h` canvas in `order` (see the module docs). `labels` names the clips (and the
/// seconds on the ruler) through text_host; `preview` is the frame at the playhead.
pub fn paint(st: &EditorState, w: u32, h: u32, order: PixelOrder, labels: Option<&TextStyle>, preview: Option<&Still>) -> Vec<u8> {
    paint_in(st, Layout::of(w, h, st), order, labels, preview)
}

/// [`paint`] in a given layout (the face's view, which a wheel zoom anchors at the pointer: [`Face::layout`]).
pub fn paint_in(st: &EditorState, l: Layout, order: PixelOrder, labels: Option<&TextStyle>, preview: Option<&Still>) -> Vec<u8> {
    let (w, h) = (l.w, l.h);
    let tl = &st.timeline;
    let mut c = Canvas::new(w, h, order);
    c.rect(0, 0, w as i64, h as i64, BG);
    // The preview band.
    c.rect(0, 0, w as i64, l.preview_h as i64, FIELD);
    if let Some(s) = preview {
        c.blit_fit(s, 0, 0, w, l.preview_h);
    }
    // The ruler: a short mark per tick when they are 4 px apart, a long one per second.
    let fps = tl.fps().unwrap_or(25).max(1) as i64;
    let (ry, ty) = (l.ruler_y as i64, l.tracks_y as i64);
    c.rect(0, ry, w as i64, ty, RULER);
    let t_end = l.tick_at(w as f64) + 1;
    for t in l.t0..=t_end {
        let x = l.x_of(t);
        if t % fps == 0 {
            c.rect(x, ry + RULER_H as i64 / 2, x + 1, ty, TICK);
            if let Some(style) = labels.filter(|_| l.zoom as i64 * fps >= 60) {
                c.text(style, &edl::timecode(t, fps as u32), x as f32 + 2.0, ry as f32 + 9.0, (x as i32, w as i32));
            }
        } else if l.zoom >= 4 {
            c.rect(x, ty - 3, x + 1, ty, TICK);
        }
    }
    // The tracks and their clips.
    for (row, track) in tl.tracks.iter().enumerate() {
        let y0 = l.row_y(row);
        let y1 = y0 + ROW_H as i64;
        c.rect(0, y0, w as i64, y1, ROW[row % 2]);
        for clip in &track.clips {
            let (x0, x1) = (l.x_of(clip.at), l.x_of(clip.end()));
            if x1 <= 0 || x0 >= w as i64 {
                continue;
            }
            c.rect(x0 + 1, y0 + 2, x1 - 1, y1 - 2, clip_colour(clip.src.0, track.kind));
            if let (Some(style), Some(src)) = (labels, tl.source(clip.src)) {
                c.text(style, &src.reel, x0 as f32 + 4.0, y0 as f32 + 15.0, ((x0 + 1) as i32, (x1 - 1) as i32));
            }
        }
    }
    // The selection (NLE2c: every clip of the set outlined), and the primary's two trim handles.
    for &(row, i) in &st.set {
        if let Some(clip) = tl.tracks.get(row).and_then(|tr| tr.clips.get(i)) {
            c.frame(l.x_of(clip.at), l.row_y(row), l.x_of(clip.end()), l.row_y(row) + ROW_H as i64, SELECT);
        }
    }
    if let (Some((row, _)), Some(clip)) = (st.selection, st.selected()) {
        let (x0, x1) = (l.x_of(clip.at), l.x_of(clip.end()));
        let (y0, y1) = (l.row_y(row), l.row_y(row) + ROW_H as i64);
        c.frame(x0, y0, x1, y1, SELECT);
        c.rect(x0, y0 + 2, x0 + HANDLE_W, y1 - 2, HANDLE);
        c.rect(x1 - HANDLE_W, y0 + 2, x1, y1 - 2, HANDLE);
    }
    paint_transitions(&mut c, l, st);
    // The playhead, through the ruler and the tracks.
    let px = l.x_of(st.playhead);
    c.rect(px, ry, px + 1, h as i64, PLAYHEAD);
    c.px
}

/// FNV-1a 64 of a buffer (the goldens' hash, tools/nle's).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

// ---------------------------------------------------------------------------------------------------------------------
// THE PREVIEW
// ---------------------------------------------------------------------------------------------------------------------

struct OneFrame(Option<Still>);

impl RenderSink for OneFrame {
    fn frame(&mut self, _out_t: i64, width: u32, height: u32, rgba: &[u8]) {
        self.0 = Some(Still { width, height, rgba: rgba.to_vec() });
    }
    fn pcm(&mut self, _out_t: i64, _rate: u32, _channels: u16, _interleaved: &[f32]) {}
}

/// The frame tick `t` shows: `stria::nle::render` of `t..t+1` over the video tracks (the audio is not this face's).
/// `None` when nothing on the timeline sizes a frame there.
pub fn preview(tl: &Timeline, sources: &Sources, t: i64) -> Result<Option<Still>, RenderError> {
    let mut video = tl.clone();
    video.tracks.retain(|tr| tr.kind == Kind::Video);
    if video.tracks.is_empty() {
        return Ok(None);
    }
    let mut sink = OneFrame(None);
    match render(&video, t..t + 1, sources, &mut sink) {
        Ok(_) => Ok(sink.0),
        Err(RenderError::NoSize) => Ok(None),
        Err(e) => Err(e),
    }
}

/// NLE1's two-source fixture (`tests/nle.rs`): 25 fps, V1 + A1, A[13,23)@0, B[0,10)@10, A[23,33)@25 on both tracks,
/// over `synth_source(0, 50, 25)` (reel A) and `synth_source(1000, 50, 25)` (reel B).
pub fn synth_fixture() -> (Timeline, Sources) {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    for t in 0..2 {
        tl.tracks[t].clips = vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
    }
    let mut src = Sources::new(&tl);
    for (id, base) in [(a, 0u32), (b, 1000)] {
        // synth_source writes demux_core's own test MP4: opening it is infallible by construction.
        src.add(id, synth_source(base, 50, 25)).expect("synthetic source opens");
    }
    (tl, src)
}

// ---------------------------------------------------------------------------------------------------------------------
// THE BUS FACE
// ---------------------------------------------------------------------------------------------------------------------

/// The transport verbs a button (or the headless witness) fires as `SMessage::Input { target: SURFACE, text }`.
pub const VERBS: [&str; 14] = ["play", "prev", "next", "split", "delete", "undo", "redo", "zoom-", "zoom+", "[", "]", "{", "}", "lift"];

/// One editor window's model: the state, the media, the canvas size and pixel order, the transport.
pub struct Face {
    pub state: EditorState,
    sources: Sources,
    pub canvas: (u32, u32),
    pub order: PixelOrder,
    pub playing: bool,
    labels: Option<TextStyle>,
    still: Option<Still>,
    pub last_error: Option<String>,
    /// The shuttle rate in frames per clock tick (J/K/L; 1 for play).
    pub rate: i64,
    drag: Option<Drag>,
    pointer: Option<(f64, f64)>,
    /// The surface sends `BrowserButton` (its release then stands for `BrowserClick`).
    buttons: bool,
    wheel: f64,
    /// A wheel zoom's anchored view: `(t0, the playhead it was anchored at)`.
    view: Option<(i64, i64)>,
    /// NLE3d (SR302): the selected transition's thumbnail, keyed `(track, idx, dur, at)` (rendered once per change).
    trans_still: Option<((usize, usize, i64, i64), Option<Still>)>,
    /// NLE4C (SR301): the per-track and master meters, fed by each played tick's render.
    meters: MixMeters,
}

impl Face {
    /// `labels`: name the clips through the host font stack (off for a golden).
    pub fn new(state: EditorState, sources: Sources, canvas: (u32, u32), order: PixelOrder, labels: bool) -> Face {
        let labels = labels.then(|| TextStyle::new(&["sans"], 11.0));
        Face { state, sources, canvas, order, playing: false, labels, still: None, last_error: None, rate: 1, drag: None, pointer: None, buttons: false, wheel: 0.0, view: None, trans_still: None, meters: MixMeters::default() }
    }
    /// The preview frame of the last [`Face::frame`].
    pub fn still(&self) -> Option<&Still> {
        self.still.as_ref()
    }
    /// One message from the surface or the buttons; true when the canvas must be repainted.
    pub fn on(&mut self, msg: &SMessage) -> bool {
        match msg {
            SMessage::Input { target, text } if target == SURFACE => self.verb(text),
            SMessage::BrowserClick(x, y) if !self.buttons => self.click(*x, *y),
            SMessage::BrowserResize(w, h) if *w > 0 && *h > 0 && (*w, *h) != self.canvas => {
                self.canvas = (*w, *h);
                true
            }
            SMessage::BrowserButton { down: true, x, y, mods } => self.press(*x, *y, *mods),
            SMessage::BrowserButton { down: false, x, y, .. } => self.release(*x, *y),
            SMessage::BrowserMouseMove(x, y) => self.hover(*x, *y),
            SMessage::BrowserScroll(_, dy) => self.scroll(*dy),
            SMessage::BrowserZoom(d) if *d != 0 => {
                self.state.zoom_by(*d);
                true
            }
            SMessage::BrowserSelectAll => {
                self.state.select_all();
                true
            }
            SMessage::BrowserKey(k) => self.key(k, false),
            SMessage::BrowserText(t) => self.key(t, true),
            _ => false,
        }
    }
    /// A transport verb ([`VERBS`]); false for any other word.
    pub fn verb(&mut self, v: &str) -> bool {
        if let Some(r) = self.transition_verb(v) { return r; }
        let st = &mut self.state;
        let r = match v {
            "play" => {
                self.playing = !self.playing;
                self.rate = 1;
                Ok(())
            }
            "prev" => {
                st.nudge(-1);
                Ok(())
            }
            "next" => {
                st.nudge(1);
                Ok(())
            }
            "split" => st.split_at_playhead(),
            "delete" => st.ripple_delete(),
            "undo" => {
                st.undo();
                Ok(())
            }
            "redo" => {
                st.redo();
                Ok(())
            }
            "zoom-" => {
                st.zoom_by(-1);
                Ok(())
            }
            "zoom+" => {
                st.zoom_by(1);
                Ok(())
            }
            "[" => st.trim(Edge::In, -1),
            "]" => st.trim(Edge::In, 1),
            "{" => st.trim(Edge::Out, -1),
            "}" => st.trim(Edge::Out, 1),
            "lift" => st.lift_selected(),
            _ => return false,
        };
        if let Err(e) = r {
            self.last_error = Some(format!("{v}: {e}"));
        }
        true
    }
    /// A click on the surface (buffer pixels): the ruler scrubs, a clip is selected and scrubbed to, an empty track
    /// spot scrubs and deselects.
    fn click(&mut self, x: f64, y: f64) -> bool {
        if let Some(r) = self.pick_transition(x, y) { return r; }
        let l = self.layout();
        match l.hit(&self.state.timeline, x, y) {
            Hit::Ruler(t) => self.state.seek(t),
            Hit::Clip(row, idx, t) => {
                self.state.seek(t);
                let _ = self.state.select(row, idx);
            }
            Hit::Track(_, t) => {
                self.state.seek(t);
                self.state.deselect();
            }
            Hit::None => return false,
        }
        true
    }
    /// One playback tick: the playhead steps `rate` frames (back when negative); playback stops at the end going
    /// forward and at 0 going back. True when it moved or stopped.
    pub fn tick(&mut self) -> bool {
        if !self.playing {
            return false;
        }
        let ph = self.state.playhead;
        if (self.rate > 0 && ph >= self.state.timeline.end()) || (self.rate <= 0 && ph <= 0) {
            self.playing = false;
            return true;
        }
        self.feed_meters(ph, self.rate); self.state.nudge(self.rate);
        true
    }
    /// The canvas as painted now (the preview at the playhead rendered first). While a drag is live the drag's
    /// preview state ([`Drag::shown`]) is painted: the edit as it would commit, the timeline itself untouched.
    pub fn paint(&mut self) -> Vec<u8> {
        let l = self.layout();
        let shown = self.drag.as_ref().and_then(|d| d.shown(&self.state));
        let st = shown.as_ref().unwrap_or(&self.state);
        self.still = match preview(&st.timeline, &self.sources, st.playhead) {
            Ok(s) => s,
            Err(e) => {
                self.last_error = Some(format!("preview: {e}"));
                None
            }
        };
        let px = paint_in(st, l, self.order, self.labels.as_ref(), self.still.as_ref());
        paint_meters(paint_drag(self.paint_transition_still(px, l), l, self.order, self.drag.as_ref()), l, self.order, &self.meter_levels())
    }
    /// The canvas as the surface's blit.
    pub fn frame(&mut self) -> SMessage {
        let (width, height) = self.canvas;
        SMessage::SurfaceBlit { url: SURFACE.into(), width, height, pixels: self.paint() }
    }
}

/// Serve one editor face on the bus: the first frame, then every surface / button message through [`Face::on`] and a
/// fresh blit after each change; while playing, a clock at the timeline's frame rate steps the playhead.
pub async fn serve(bus: Synapse, mut rx: tokio::sync::broadcast::Receiver<SMessage>, mut face: Face) {
    use tokio::sync::broadcast::error::RecvError;
    bus.fire(face.frame());
    let fps = face.state.timeline.fps().unwrap_or(25).max(1);
    let mut clock = tokio::time::interval(Duration::from_nanos(1_000_000_000 / fps as u64));
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Ok(m) => {
                    if face.on(&m) {
                        bus.fire(face.frame());
                    }
                    if let Some(e) = face.last_error.take() {
                        log::warn!("[nle-face] {e}");
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
            _ = clock.tick(), if face.playing => {
                if face.tick() {
                    bus.fire(face.frame());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// THE GESTURES (NLE2B, LEDGER SR208)
// ---------------------------------------------------------------------------------------------------------------------

/// The keys the face takes (key -> action). A ONE-character key is read only from `BrowserText`, a named key only
/// from `BrowserKey`: GTK sends both for a printable key (`BrowserKey("j")` + `BrowserText("j")`), AppKit only the
/// text (its Delete / Home / End arrive as the private-use U+F728 / U+F729 / U+F72B). An action that is a [`VERBS`]
/// word goes through [`Face::verb`].
pub const KEYS: &[(&str, &str)] = &[
    ("j", "shuttle-"),
    ("k", "stop"),
    ("l", "shuttle+"),
    ("i", "mark-in"),
    ("o", "mark-out"),
    (" ", "play"),
    ("+", "zoom+"),
    ("=", "zoom+"),
    ("-", "zoom-"),
    ("Z", "redo"),
    ("\u{f728}", "delete"),
    ("\u{f729}", "home"),
    ("\u{f72b}", "end"),
    ("Escape", "cancel"),
    ("Delete", "delete"),
    ("KP_Delete", "delete"),
    ("BackSpace", "delete"),
    ("Home", "home"),
    ("End", "end"),
    ("Ctrl+z", "undo"),
    ("Meta+z", "undo"),
    ("Ctrl+Shift+Z", "redo"),
    ("Meta+Shift+Z", "redo"),
    ("Left", "nudge-"),
    ("Right", "nudge+"),
    ("\u{f702}", "nudge-"),
    ("\u{f703}", "nudge+"),
    ("[", "["),
    ("]", "]"),
];
/// The wheel delta one zoom step takes (the AppKit surface sends 3 per notch).
pub const WHEEL_STEP: f64 = 3.0;
/// The fastest shuttle (frames per clock tick).
pub const SHUTTLE_MAX: i64 = 8;

/// What a drag does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    /// The selected clip's in handle (`EditorState::trim(Edge::In, ..)`).
    TrimIn,
    /// The selected clip's out handle (`EditorState::trim(Edge::Out, ..)`).
    TrimOut,
    /// A clip body (`EditorState::move_selected`; NLE2c: `EditorState::move_set` for a set of more than one).
    Move,
    /// NLE2c: a rubber band from an empty track spot (`EditorState::select_many` of the clips it crosses).
    Marquee,
    /// NLE2c: a press on the ruler; the playhead follows the pointer (a seek, not an edit).
    Scrub,
}

/// One pointer drag: begun by a press, updated by moves, committed ONCE on release ([`Drag::finish`]). It reads the
/// layout frozen at the press, so the pixel -> tick mapping cannot shift under it, and it edits only through
/// [`EditorState`]'s public ops.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    pub kind: DragKind,
    pub track: usize,
    pub idx: usize,
    /// The tick under the press.
    pub origin_tick: i64,
    pub layout: Layout,
    /// Ticks moved since the press (snapped when `snap`).
    pub delta: i64,
    /// Where a move lands (a track of the clip's kind).
    pub to_track: usize,
    /// NLE2c: the press and the pointer now (surface pixels).
    pub origin: (f64, f64),
    pub at: (f64, f64),
    /// NLE2c: snap the moving edge to clip edges / the playhead (off under the no-snap modifier).
    pub snap: bool,
    /// NLE2c: begun with shift (adds to the set instead of replacing it).
    pub additive: bool,
    /// NLE2c: the tick the delta snapped to (painted as the guide line).
    pub guide: Option<i64>,
}

impl Drag {
    /// A press at `(x, y)` with no modifier ([`Drag::begin_with`]).
    pub fn begin(st: &mut EditorState, l: Layout, x: f64, y: f64) -> Option<Drag> {
        Drag::begin_with(st, l, x, y, false, false)
    }
    /// A press at `(x, y)`: on the primary's drawn trim handle (the rects [`paint`] draws) a trim; on a clip body a move
    /// (a clip of the set keeps the set and becomes its primary, `shift` adds the clip, else it alone is selected); on
    /// an empty track spot a marquee; on the ruler a scrub; anywhere else no drag.
    pub fn begin_with(st: &mut EditorState, l: Layout, x: f64, y: f64, shift: bool, nosnap: bool) -> Option<Drag> {
        let origin_tick = l.tick_at(x);
        let mk = |kind, track, idx| Drag {
            kind,
            track,
            idx,
            origin_tick,
            layout: l,
            delta: 0,
            to_track: track,
            origin: (x, y),
            at: (x, y),
            snap: !nosnap,
            additive: shift,
            guide: None,
        };
        if let (Some((track, idx)), Some(c), false) = (st.selection, st.selected(), shift) {
            let y0 = l.row_y(track) as f64;
            if y >= y0 && y < y0 + ROW_H as f64 {
                let (x0, x1) = (l.x_of(c.at) as f64, l.x_of(c.end()) as f64);
                let hw = HANDLE_W as f64;
                let kind = if x >= x0 && x < x0 + hw {
                    Some(DragKind::TrimIn)
                } else if x >= x1 - hw && x < x1 {
                    Some(DragKind::TrimOut)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    return Some(mk(kind, track, idx));
                }
            }
        }
        match l.hit(&st.timeline, x, y) {
            Hit::Clip(track, idx, _) => {
                if shift || st.set.contains(&(track, idx)) {
                    st.select_add(track, idx).ok()?;
                } else {
                    st.select(track, idx).ok()?;
                }
                Some(mk(DragKind::Move, track, idx))
            }
            Hit::Track(track, _) => Some(mk(DragKind::Marquee, track, 0)),
            Hit::Ruler(_) => Some(mk(DragKind::Scrub, 0, 0)),
            Hit::None => None,
        }
    }
    /// The pointer is at `(x, y)`: the tick delta (snapped), and for a move of one clip the row under it when that
    /// track's kind matches.
    pub fn update(&mut self, st: &EditorState, x: f64, y: f64) {
        self.at = (x, y);
        self.delta = self.layout.tick_at(x) - self.origin_tick;
        self.guide = None;
        if self.snap {
            self.snap_delta(st, x);
        }
        if self.kind != DragKind::Move || st.set.len() > 1 {
            return;
        }
        if let Hit::Clip(row, ..) | Hit::Track(row, _) = self.layout.hit(&st.timeline, x, y) {
            let from = st.timeline.tracks.get(self.track).map(|t| t.kind);
            if from.is_some() && from == st.timeline.tracks.get(row).map(|t| t.kind) {
                self.to_track = row;
            }
        }
    }
    /// The drag would change something.
    pub fn moved(&self) -> bool {
        match self.kind {
            DragKind::Marquee => {
                (self.at.0 - self.origin.0).abs() >= MARQUEE_MIN || (self.at.1 - self.origin.1).abs() >= MARQUEE_MIN
            }
            _ => self.delta != 0 || self.to_track != self.track,
        }
    }
    /// THE commit of the gesture: one `trim`, `move_selected` / `move_set` or marquee selection (a scrub commits nothing).
    pub fn finish(&self, st: &mut EditorState) -> Result<(), EditError> {
        match self.kind {
            DragKind::Marquee => {
                let (x0, y0, x1, y1) = (self.origin.0, self.origin.1, self.at.0, self.at.1);
                st.select_many(&self.layout.crossing(&st.timeline, x0, y0, x1, y1), self.additive);
                return Ok(());
            }
            DragKind::Scrub => return Ok(()),
            _ => {}
        }
        if st.selection != Some((self.track, self.idx)) {
            st.select(self.track, self.idx)?;
        }
        match self.kind {
            DragKind::TrimIn => st.trim(Edge::In, self.delta),
            DragKind::TrimOut => st.trim(Edge::Out, self.delta),
            DragKind::Move if st.set.len() > 1 => st.move_set(self.delta),
            _ => {
                let at = st.selected().ok_or(EditError::NoClip)?.at;
                st.move_selected(self.to_track, at + self.delta)
            }
        }
    }
    /// The preview: [`Drag::finish`] on a scratch state (no undo stacks); `None` when nothing moved or the op refuses.
    pub fn shown(&self, st: &EditorState) -> Option<EditorState> {
        if !self.moved() {
            return None;
        }
        let mut s = EditorState::new(st.timeline.clone());
        s.selection = st.selection;
        s.set = st.set.clone();
        s.playhead = st.playhead;
        s.zoom = st.zoom;
        self.finish(&mut s).ok()?;
        Some(s)
    }
}

impl Face {
    /// The face's geometry: [`Layout::of`], with the view a wheel zoom anchored kept while the playhead is where it
    /// was anchored or inside that view (else the page holding the playhead).
    pub fn layout(&self) -> Layout {
        let (w, h) = self.canvas;
        let mut l = Layout::of(w, h, &self.state);
        if let Some((t0, at)) = self.view {
            let per = (w / l.zoom).max(1) as i64;
            let ph = self.state.playhead;
            if ph == at || (t0..t0 + per).contains(&ph) {
                l.t0 = t0;
            }
        }
        l
    }
    /// The last pointer position the surface reported.
    pub fn pointer(&self) -> Option<(f64, f64)> {
        self.pointer
    }
    /// The live drag.
    pub fn dragging(&self) -> Option<&Drag> {
        self.drag.as_ref()
    }
    /// The button went down: a drag may begin (a live one is dropped uncommitted). `mods`: the press's modifier bits
    /// (Shift adds to the set, Alt / Option turns snapping off).
    fn press(&mut self, x: f64, y: f64, mods: u8) -> bool {
        self.buttons = true;
        self.pointer = Some((x, y));
        if let Some(r) = self.pick_transition(x, y) { return r; }
        let l = self.layout();
        let dropped = self.drag.take().is_some();
        let (sel, set, ph) = (self.state.selection, self.state.set.clone(), self.state.playhead);
        let (shift, nosnap) = (mods & bandy::MOD_SHIFT != 0, mods & bandy::MOD_ALT != 0);
        self.drag = Drag::begin_with(&mut self.state, l, x, y, shift, nosnap);
        if let Some(d) = self.drag.filter(|d| d.kind == DragKind::Scrub) {
            self.state.seek(d.origin_tick);
        }
        dropped || self.state.selection != sel || self.state.set != set || self.state.playhead != ph
    }
    /// The button came up: a drag that moved commits once; otherwise it is the click it always was.
    fn release(&mut self, x: f64, y: f64) -> bool {
        self.pointer = Some((x, y));
        let Some(mut d) = self.drag.take() else { return self.click(x, y) };
        d.update(&self.state, x, y);
        if !d.moved() {
            return if d.additive { true } else { self.click(x, y) };
        }
        if d.kind == DragKind::Scrub {
            self.state.seek(d.origin_tick + d.delta);
            return true;
        }
        if let Err(e) = d.finish(&mut self.state) {
            self.last_error = Some(format!("drag: {e}"));
        }
        true
    }
    /// The pointer moved: a live drag follows it.
    fn hover(&mut self, x: f64, y: f64) -> bool {
        self.pointer = Some((x, y));
        let Some(d) = self.drag.as_mut() else { return false };
        let before = (d.delta, d.to_track, d.at, d.guide);
        d.update(&self.state, x, y);
        let changed = match d.kind {
            DragKind::Marquee => d.at != before.2,
            _ => (d.delta, d.to_track, d.guide) != (before.0, before.1, before.3),
        };
        if d.kind == DragKind::Scrub {
            let t = d.origin_tick + d.delta;
            self.state.seek(t);
        }
        changed
    }
    /// The wheel: one zoom step per [`WHEEL_STEP`] (up zooms in), about the pointer.
    fn scroll(&mut self, dy: f64) -> bool {
        self.wheel += dy;
        let steps = (self.wheel / WHEEL_STEP).trunc();
        if steps == 0.0 {
            return false;
        }
        self.wheel -= steps * WHEEL_STEP;
        let x = self.pointer.map(|p| p.0).unwrap_or(0.0);
        self.zoom_at(-(steps as i32), x)
    }
    /// Zoom by `step` ([`EditorState::zoom_by`]) keeping the tick under `x` under `x` (the view is anchored there).
    pub fn zoom_at(&mut self, step: i32, x: f64) -> bool {
        let t = self.layout().tick_at(x);
        let z0 = self.state.zoom;
        self.state.zoom_by(step);
        if self.state.zoom == z0 {
            return false;
        }
        let t0 = (t - x.max(0.0) as i64 / self.state.zoom as i64).max(0);
        self.view = Some((t0, self.state.playhead));
        true
    }
    /// A key ([`KEYS`]); `text` says it came as `BrowserText`. False for a key the face does not take.
    fn key(&mut self, k: &str, text: bool) -> bool {
        if (k.chars().count() == 1) != text {
            return false;
        }
        let Some(&(_, act)) = KEYS.iter().find(|(name, _)| *name == k) else { return false };
        if VERBS.contains(&act) {
            return self.verb(act);
        }
        let ph = self.state.playhead;
        let r = match act {
            "shuttle+" => {
                self.rate = if self.playing && self.rate > 0 { (self.rate * 2).min(SHUTTLE_MAX) } else { 1 };
                self.playing = true;
                Ok(())
            }
            "shuttle-" => {
                self.rate = if self.playing && self.rate < 0 { (self.rate * 2).max(-SHUTTLE_MAX) } else { -1 };
                self.playing = true;
                Ok(())
            }
            "stop" => {
                self.playing = false;
                self.rate = 1;
                Ok(())
            }
            "mark-in" => match self.state.selected() {
                Some(c) => self.state.trim(Edge::In, ph - c.at),
                None => Err(EditError::NoClip),
            },
            "mark-out" => match self.state.selected() {
                Some(c) => self.state.trim(Edge::Out, ph - c.end()),
                None => Err(EditError::NoClip),
            },
            "home" => {
                self.state.seek(0);
                Ok(())
            }
            "end" => {
                let e = self.state.timeline.end();
                self.state.seek(e);
                Ok(())
            }
            "nudge-" => self.state.move_set(-1),
            "nudge+" => self.state.move_set(1),
            "cancel" => return self.drag.take().is_some(),
            _ => return false,
        };
        if let Err(e) = r {
            self.last_error = Some(format!("{}: {e}", act.trim_end_matches(['-', '+'])));
        }
        true
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// THE SET, SNAPPING AND THE RULER SCRUB (NLE2c, LEDGER SR217)
// ---------------------------------------------------------------------------------------------------------------------

/// How near (px, at the current zoom) a moving edge must come to a clip edge or the playhead to snap to it.
pub const SNAP_PX: f64 = 6.0;
/// How far (px) the pointer must travel before a press on an empty track is a marquee rather than a click.
pub const MARQUEE_MIN: f64 = 3.0;
const GUIDE: [u8; 4] = [0x40, 0xd0, 0xff, 255];
const MARQUEE: [u8; 4] = [0xa0, 0xa0, 0xff, 255];

impl Layout {
    /// The clips whose drawn rect (`x_of(at)..x_of(end)` x its row) meets the rect with corners `(x0, y0)` and
    /// `(x1, y1)` (surface pixels, either order), sorted.
    pub fn crossing(&self, tl: &Timeline, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<(usize, usize)> {
        let (xa, xb) = (x0.min(x1), x0.max(x1));
        let (ya, yb) = (y0.min(y1), y0.max(y1));
        let mut out = Vec::new();
        for (row, tr) in tl.tracks.iter().enumerate().take(self.rows) {
            let (r0, r1) = (self.row_y(row) as f64, (self.row_y(row) + ROW_H as i64) as f64);
            if yb < r0 || ya >= r1 {
                continue;
            }
            for (i, c) in tr.clips.iter().enumerate() {
                let (c0, c1) = (self.x_of(c.at) as f64, self.x_of(c.end()) as f64);
                if xb >= c0 && xa < c1 {
                    out.push((row, i));
                }
            }
        }
        out
    }
}

/// Where a moving edge may snap: every clip's `at` and `end` on any track except the `exclude`d clips', plus the
/// playhead when `playhead`; sorted, deduped.
pub fn snap_targets(st: &EditorState, exclude: &[(usize, usize)], playhead: bool) -> Vec<i64> {
    let mut v = Vec::new();
    for (t, tr) in st.timeline.tracks.iter().enumerate() {
        for (i, c) in tr.clips.iter().enumerate() {
            if !exclude.contains(&(t, i)) {
                v.push(c.at);
                v.push(c.end());
            }
        }
    }
    if playhead {
        v.push(st.playhead);
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// The target nearest pixel `x` whose `x_of` lies within [`SNAP_PX`] of it (a tie: the earlier tick); `None` when none
/// is in range.
pub fn snap_to(l: &Layout, targets: &[i64], x: f64) -> Option<i64> {
    let mut best: Option<(f64, i64)> = None;
    for &t in targets {
        let d = (l.x_of(t) as f64 - x).abs();
        if d <= SNAP_PX && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, t));
        }
    }
    best.map(|(_, t)| t)
}

impl Drag {
    /// Snap the delta: each moving edge sits at pixel `x_of(edge) + (x - origin.x)` (a scrub's is the pointer); the
    /// nearest (edge, target) pair within [`SNAP_PX`] sets `delta = target - edge` and the guide. A marquee never snaps.
    fn snap_delta(&mut self, st: &EditorState, x: f64) {
        let l = self.layout;
        let dx = x - self.origin.0;
        let one = [(self.track, self.idx)];
        let (edges, exclude, playhead): (Vec<i64>, &[(usize, usize)], bool) = match self.kind {
            DragKind::Marquee => return,
            DragKind::Scrub => {
                if let Some(t) = snap_to(&l, &snap_targets(st, &[], false), x) {
                    self.delta = t - self.origin_tick;
                    self.guide = Some(t);
                }
                return;
            }
            DragKind::TrimIn | DragKind::TrimOut => {
                let Some(c) = st.timeline.tracks.get(self.track).and_then(|tr| tr.clips.get(self.idx)) else { return };
                (vec![if self.kind == DragKind::TrimIn { c.at } else { c.end() }], &one, true)
            }
            DragKind::Move => {
                let set: &[(usize, usize)] = if st.set.len() > 1 { &st.set } else { &one };
                let edges = set
                    .iter()
                    .filter_map(|&(t, i)| st.timeline.tracks.get(t)?.clips.get(i))
                    .flat_map(|c| [c.at, c.end()])
                    .collect();
                (edges, set, true)
            }
        };
        let targets = snap_targets(st, exclude, playhead);
        let mut best: Option<(f64, i64, i64)> = None;
        for &e in &edges {
            let ex = l.x_of(e) as f64 + dx;
            if let Some(t) = snap_to(&l, &targets, ex) {
                let d = (l.x_of(t) as f64 - ex).abs();
                if best.is_none_or(|(bd, _, _)| d < bd) {
                    best = Some((d, e, t));
                }
            }
        }
        if let Some((_, e, t)) = best {
            self.delta = t - e;
            self.guide = Some(t);
        }
    }
}

/// The live drag over the painted canvas: the snap guide (a vertical line from the ruler down) and the marquee's rect.
fn paint_drag(px: Vec<u8>, l: Layout, order: PixelOrder, drag: Option<&Drag>) -> Vec<u8> {
    let Some(d) = drag else { return px };
    let mut c = Canvas { w: l.w, h: l.h, order, px };
    if let Some(t) = d.guide {
        let x = l.x_of(t);
        c.rect(x, l.ruler_y as i64, x + 1, l.h as i64, GUIDE);
    }
    if d.kind == DragKind::Marquee && d.moved() {
        let (x0, x1) = (d.origin.0.min(d.at.0) as i64, d.origin.0.max(d.at.0) as i64 + 1);
        let (y0, y1) = (d.origin.1.min(d.at.1) as i64, d.origin.1.max(d.at.1) as i64 + 1);
        c.frame(x0, y0, x1, y1, MARQUEE);
    }
    c.px
}


// ---------------------------------------------------------------------------------------------------------------------
// THE TRANSITION WIDGET (NLE3d, LEDGER SR302)
// ---------------------------------------------------------------------------------------------------------------------

/// The widget's height (px): a band at the foot of the track row.
pub const TRANS_H: i64 = 10;
/// The narrowest widget (px), so a short transition at a low zoom stays clickable.
pub const TRANS_MIN_W: i64 = 8;
/// The selected transition's thumbnail box (px), above its row.
pub const THUMB_W: i64 = 48;
pub const THUMB_H: i64 = 36;
const TRANS: [u8; 4] = [0xe0, 0x90, 0xff, 255];
const TRANS_FILL: [u8; 4] = [0x50, 0x30, 0x70, 255];
/// The kind glyphs, 7x7 (bit 6 the leftmost column): D, W001, W002, W041, W101, W201.
const GLYPHS: [[u8; 7]; 6] = [
    [0x41, 0x22, 0x14, 0x08, 0x14, 0x22, 0x41],
    [0x70, 0x70, 0x70, 0x70, 0x70, 0x70, 0x70],
    [0x7f, 0x7f, 0x7f, 0x00, 0x00, 0x00, 0x00],
    [0x7f, 0x7e, 0x7c, 0x78, 0x70, 0x60, 0x40],
    [0x7f, 0x41, 0x5d, 0x55, 0x5d, 0x41, 0x7f],
    [0x1c, 0x2a, 0x49, 0x4f, 0x41, 0x22, 0x1c],
];

fn glyph(kind: TransitionKind) -> &'static [u8; 7] {
    &GLYPHS[match kind {
        TransitionKind::Dissolve => 0,
        TransitionKind::Wipe(WipeDir::LeftToRight | WipeDir::BarnDoorVertical) => 1,
        TransitionKind::Wipe(WipeDir::TopToBottom | WipeDir::BarnDoorHorizontal) => 2,
        TransitionKind::Wipe(WipeDir::Diagonal | WipeDir::DiagonalTopRight) => 3,
        TransitionKind::Wipe(WipeDir::Box | WipeDir::CornerTopLeft | WipeDir::CornerTopRight | WipeDir::CornerBottomRight | WipeDir::CornerBottomLeft) => 4,
        TransitionKind::Wipe(WipeDir::Clock | WipeDir::ClockThree | WipeDir::ClockSix | WipeDir::ClockNine) => 5,
    }]
}

impl Layout {
    /// THE widget geometry, read by [`paint`] and [`Layout::transition_at`]: a bracket spanning the transition's `dur`
    /// ticks from its cut (`x_of(at)..x_of(at + dur)`, at least [`TRANS_MIN_W`] wide) in the foot band of row `row`.
    pub fn transition_rect(&self, row: usize, x: &Transition) -> (i64, i64, i64, i64) {
        let x0 = self.x_of(x.at);
        let x1 = self.x_of(x.at + x.dur).max(x0 + TRANS_MIN_W);
        let y1 = self.row_y(row) + ROW_H as i64 - 2;
        (x0, y1 - TRANS_H, x1, y1)
    }
    /// The transition whose widget is under `(x, y)`: `(track, index)` (the last drawn wins).
    pub fn transition_at(&self, tl: &Timeline, x: f64, y: f64) -> Option<(usize, usize)> {
        let mut hit = None;
        for (row, tr) in tl.tracks.iter().enumerate().take(self.rows) {
            for (i, t) in tr.transitions.iter().enumerate() {
                let (x0, y0, x1, y1) = self.transition_rect(row, t);
                if x >= x0 as f64 && x < x1 as f64 && y >= y0 as f64 && y < y1 as f64 {
                    hit = Some((row, i));
                }
            }
        }
        hit
    }
    /// The thumbnail box `(x0, y0)` ([`THUMB_W`] x [`THUMB_H`]): above row `row`, at the widget's left, kept on the canvas.
    pub fn transition_thumb(&self, row: usize, x: &Transition) -> (i64, i64) {
        let x0 = self.x_of(x.at).min((self.w as i64 - THUMB_W).max(0)).max(0);
        (x0, (self.row_y(row) - THUMB_H).max(0))
    }
}

/// The tick a transition's preview shows: its midpoint, `at + dur / 2`.
pub fn transition_mid(x: &Transition) -> i64 {
    x.at + x.dur / 2
}

/// Every transition's widget: the bracket (outlined white when selected), its kind glyph and, from black, a black
/// marker on its outgoing (left) edge; NLE3cBLACK: to black, one on its incoming (right) edge.
fn paint_transitions(c: &mut Canvas, l: Layout, st: &EditorState) {
    for (row, tr) in st.timeline.tracks.iter().enumerate().take(l.rows) {
        for (i, x) in tr.transitions.iter().enumerate() {
            let (x0, y0, x1, y1) = l.transition_rect(row, x);
            if x1 <= 0 || x0 >= l.w as i64 {
                continue;
            }
            c.rect(x0, y0, x1, y1, TRANS_FILL);
            c.frame(x0, y0, x1, y1, if st.transition == Some((row, i)) { SELECT } else { TRANS });
            let gx = if x.from_black {
                c.rect(x0 + 1, y0 + 1, (x0 + 4).min(x1 - 1), y1 - 1, FIELD);
                x0 + 5
            } else {
                x0 + 2
            };
            if x.to_black { c.rect((x1 - 4).max(x0 + 1), y0 + 1, x1 - 1, y1 - 1, FIELD); } // NLE3cBLACK: the to-black marker
            for (dy, bits) in glyph(x.kind).iter().enumerate() {
                for dx in 0..7i64 {
                    if bits & (0x40 >> dx) != 0 && gx + dx < x1 - 1 {
                        let (px, py) = (gx + dx, y0 + 2 + dy as i64);
                        c.rect(px, py, px + 1, py + 1, TRANS);
                    }
                }
            }
        }
    }
}

impl Face {
    /// A press or click on a transition's widget selects it (any live drag dropped, none begun): `Some(true)`. Anywhere
    /// else the transition selection goes and the press / click is the clip face's (`None`).
    fn pick_transition(&mut self, x: f64, y: f64) -> Option<bool> {
        let l = self.layout();
        match l.transition_at(&self.state.timeline, x, y) {
            Some((t, i)) => {
                self.drag = None;
                if let Err(e) = self.state.select_transition(t, i) {
                    self.last_error = Some(format!("transition: {e}"));
                }
                Some(true)
            }
            None => {
                self.state.transition = None;
                None
            }
        }
    }
    /// With a transition selected: "delete" removes it, "[" / "]" shorten / lengthen it by one tick. `None` for any other
    /// verb, or with no transition selected.
    fn transition_verb(&mut self, v: &str) -> Option<bool> {
        let x = self.state.selected_transition()?;
        let r = match v {
            "delete" => self.state.remove_selected_transition(),
            "[" => self.state.set_transition_dur(x.dur - 1),
            "]" => self.state.set_transition_dur(x.dur + 1),
            _ => return None,
        };
        if let Err(e) = r {
            self.last_error = Some(format!("{v}: {e}"));
        }
        Some(true)
    }
    /// The selected transition's thumbnail over the painted canvas: ONE tick at its midpoint through [`preview`],
    /// rendered only when `(track, idx, dur, at)` changed, blitted framed into [`Layout::transition_thumb`].
    fn paint_transition_still(&mut self, px: Vec<u8>, l: Layout) -> Vec<u8> {
        let (Some(key), Some(x)) = (self.transition_key(), self.state.selected_transition()) else { return px };
        if self.trans_still.as_ref().map(|c| c.0) != Some(key) {
            let s = match preview(&self.state.timeline, &self.sources, transition_mid(&x)) {
                Ok(s) => s,
                Err(e) => {
                    self.last_error = Some(format!("transition preview: {e}"));
                    None
                }
            };
            self.trans_still = Some((key, s));
        }
        let Some(s) = self.trans_still.as_ref().and_then(|c| c.1.as_ref()) else { return px };
        let (x0, y0) = l.transition_thumb(key.0, &x);
        let mut c = Canvas { w: l.w, h: l.h, order: self.order, px };
        c.rect(x0, y0, x0 + THUMB_W, y0 + THUMB_H, FIELD);
        c.blit_fit(s, x0 as u32 + 1, y0 as u32 + 1, THUMB_W as u32 - 2, THUMB_H as u32 - 2);
        c.frame(x0, y0, x0 + THUMB_W, y0 + THUMB_H, TRANS);
        c.px
    }
    /// The selected transition's thumbnail (the last [`Face::frame`]'s).
    pub fn transition_still(&self) -> Option<&Still> {
        self.trans_still.as_ref().filter(|c| Some(c.0) == self.transition_key()).and_then(|c| c.1.as_ref())
    }
    /// The thumbnail's cache key: the selected transition's `(track, idx, dur, at)`.
    pub fn transition_key(&self) -> Option<(usize, usize, i64, i64)> {
        let (t, i) = self.state.transition?;
        let x = self.state.selected_transition()?;
        Some((t, i, x.dur, x.at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gneiss_pal::dsp::video::TestPattern;
    use timeline_core::SourceId;

    fn state() -> (EditorState, Sources) {
        let (tl, src) = synth_fixture();
        (EditorState::new(tl), src)
    }

    /// M1: each transition is the timeline_core op applied by hand; a refused edit changes nothing.
    #[test]
    fn nle2_transitions_are_timeline_core_ops() {
        let (mut st, _) = state();
        let tl0 = st.timeline.clone();
        // Nothing selected: every edit is refused and the state is untouched.
        let before = st.clone();
        assert_eq!(st.split_at_playhead(), Err(EditError::NoClip));
        assert_eq!(st.trim(Edge::In, 1), Err(EditError::NoClip));
        assert_eq!(st.ripple_delete(), Err(EditError::NoClip));
        assert_eq!(st, before);
        assert_eq!(st.select(9, 0), Err(EditError::NoTrack));
        assert_eq!(st.select_at(0, 22), Err(EditError::NoClip)); // the gap 20..25

        // Split at 15 on V1's clip B[0,10)@10: the right half is selected.
        st.select_at(0, 12).unwrap();
        assert_eq!(st.selection, Some((0, 1)));
        st.seek(15);
        st.split_at_playhead().unwrap();
        let want = tl0.split(0, 15).unwrap();
        assert_eq!(st.timeline, want);
        assert_eq!(st.selection, Some((0, 2)));
        assert_eq!(st.selected(), Some(Clip::new(SourceId(1), 5, 10, 15)));

        // Trim the right half's in point +2 and its out point -1.
        st.trim(Edge::In, 2).unwrap();
        let want = want.trim_in(0, 2, 7).unwrap();
        assert_eq!(st.timeline, want);
        st.trim(Edge::Out, -1).unwrap();
        let want = want.trim_out(0, 2, 9).unwrap();
        assert_eq!(st.timeline, want);
        // A trim past the source's start is refused and changes nothing.
        let before = st.clone();
        assert_eq!(st.trim(Edge::In, -100), Err(EditError::Negative));
        assert_eq!(st, before);

        // Ripple delete: the hole closes, the selection goes.
        st.ripple_delete().unwrap();
        let want = want.ripple_delete(0, 2).unwrap();
        assert_eq!(st.timeline, want);
        assert_eq!(st.selection, None);
        assert_eq!(st.undo_depth(), 4);
    }

    /// M1: undo / redo walk the stack of returned timelines exactly; a new edit clears redo.
    #[test]
    fn nle2_undo_redo_round_trip() {
        let (mut st, _) = state();
        let t0 = st.timeline.clone();
        st.select_at(0, 5).unwrap();
        st.seek(5);
        st.split_at_playhead().unwrap();
        let t1 = st.timeline.clone();
        st.ripple_delete().unwrap();
        let t2 = st.timeline.clone();
        assert!(st.undo());
        assert_eq!(st.timeline, t1);
        assert!(st.undo());
        assert_eq!(st.timeline, t0);
        assert!(!st.undo());
        assert!(st.redo());
        assert_eq!(st.timeline, t1);
        assert!(st.redo());
        assert_eq!(st.timeline, t2);
        assert!(!st.redo());
        st.undo();
        st.select(0, 0).unwrap();
        st.trim(Edge::Out, -1).unwrap();
        assert_eq!(st.redo_depth(), 0, "a new edit clears redo");
        assert!(!st.redo());
    }

    /// M1: the playhead stays within 0..=end; zoom doubles / halves within its bounds; undo clamps the playhead.
    #[test]
    fn nle2_playhead_and_zoom_clamp() {
        let (mut st, _) = state();
        assert_eq!(st.timeline.end(), 35);
        st.nudge(-5);
        assert_eq!(st.playhead, 0);
        st.seek(1000);
        assert_eq!(st.playhead, 35);
        st.nudge(1);
        assert_eq!(st.playhead, 35);
        st.zoom_by(1);
        assert_eq!(st.zoom, 16);
        st.zoom_by(5);
        assert_eq!(st.zoom, ZOOM_MAX);
        st.zoom_by(-8);
        assert_eq!(st.zoom, ZOOM_MIN);
        // Ripple-deleting the last clip on both tracks pulls the end in; the playhead follows.
        st.select(0, 2).unwrap();
        st.ripple_delete().unwrap();
        st.select(1, 2).unwrap();
        st.ripple_delete().unwrap();
        assert_eq!((st.timeline.end(), st.playhead), (20, 20));
    }

    /// M2: the hit test reads the paint's geometry: a clip's centre is that clip, the ruler is its tick, a gap is the
    /// track, the preview band is nothing; a click through the Face selects and scrubs.
    #[test]
    fn nle2_hit_reads_the_paint_geometry() {
        let (st, src) = state();
        let l = Layout::of(320, 160, &st);
        assert_eq!((l.preview_h, l.ruler_y, l.tracks_y, l.rows, l.t0), (64, 64, 80, 2, 0));
        let (x, y) = l.point(0, 12);
        assert_eq!(l.hit(&st.timeline, x, y), Hit::Clip(0, 1, 12));
        let (x, y) = l.point(1, 26);
        assert_eq!(l.hit(&st.timeline, x, y), Hit::Clip(1, 2, 26));
        let (x, y) = l.point(0, 22);
        assert_eq!(l.hit(&st.timeline, x, y), Hit::Track(0, 22));
        assert_eq!(l.hit(&st.timeline, 8.0 * 7.0 + 1.0, 70.0), Hit::Ruler(7));
        assert_eq!(l.hit(&st.timeline, 10.0, 10.0), Hit::None);
        assert_eq!(l.hit(&st.timeline, 10.0, 159.0 + 2.0), Hit::None);

        let mut face = Face::new(st, src, (320, 160), PixelOrder::Rgba, false);
        let (x, y) = l.point(0, 12);
        assert!(face.on(&SMessage::BrowserClick(x, y)));
        assert_eq!((face.state.selection, face.state.playhead), (Some((0, 1)), 12));
        assert!(face.on(&SMessage::Input { target: SURFACE.into(), text: "split".into() }));
        assert_eq!(face.state.timeline.tracks[0].clips.len(), 4);
        assert!(!face.on(&SMessage::Input { target: "elsewhere".into(), text: "split".into() }));
        for v in VERBS {
            assert!(face.verb(v), "{v} is a verb");
        }
        assert!(!face.verb("explode"));
    }

    /// M3: over NLE1's synthetic sources, the preview after split + ripple delete shows the frame render names, and
    /// the Face's blit carries it unscaled in its preview band.
    #[test]
    fn nle2_preview_is_the_one_tick_render() {
        let (mut st, src) = state();
        st.select_at(0, 12).unwrap();
        st.seek(15);
        st.split_at_playhead().unwrap();
        st.ripple_delete().unwrap();
        // V1 now: A[13,23)@0, B[0,5)@10, A[23,33)@20 — tick 15 is a gap, tick 20 shows A's 23.
        assert_eq!(st.timeline.tracks[0].clips[2], Clip::new(SourceId(0), 23, 33, 20));
        // A gap alone sizes no frame: the render's NoSize is the preview's None (the face shows its field).
        assert_eq!(preview(&st.timeline, &src, 15).unwrap(), None);
        st.seek(21);
        let s = preview(&st.timeline, &src, 21).unwrap().expect("a frame at 21");
        assert_eq!((s.width, s.height), (64, 48));
        assert_eq!(TestPattern::read_counter(&s.rgba, 64, 48), Some(24));
        st.seek(12);
        let s = preview(&st.timeline, &src, 12).unwrap().expect("a frame at 12");
        assert_eq!(TestPattern::read_counter(&s.rgba, 64, 48), Some(1002));

        // The Face: preview band 48 px high (144 - 16 - 2 x 40), so the 64x48 frame lands 1:1 at x = (320 - 64) / 2.
        let mut face = Face::new(st, src, (320, 144), PixelOrder::Rgba, false);
        let SMessage::SurfaceBlit { url, width, height, pixels } = face.frame() else { panic!("not a blit") };
        assert_eq!((url.as_str(), width, height, pixels.len()), (SURFACE, 320, 144, 320 * 144 * 4));
        let still = face.still().expect("a still").clone();
        let mut crop = Vec::with_capacity(64 * 48 * 4);
        for y in 0..48usize {
            let row = (y * 320 + 128) * 4;
            crop.extend_from_slice(&pixels[row..row + 64 * 4]);
        }
        assert_eq!(crop, still.rgba);
        assert_eq!(TestPattern::read_counter(&crop, 64, 48), Some(1002));
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
    }

    /// M2: the painted editor (no glyphs, no preview) for one fixture matches its golden; the BGRA paint is the same
    /// picture with red and blue swapped.
    #[test]
    fn nle2_track_paint_golden() {
        let (mut st, _) = state();
        st.select(0, 1).unwrap();
        st.seek(15);
        let rgba = paint(&st, 320, 160, PixelOrder::Rgba, None, None);
        let bgra = paint(&st, 320, 160, PixelOrder::Bgra, None, None);
        let swapped: Vec<u8> = bgra.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        assert_eq!(swapped, rgba);
        // Anchors that do not lean on the hash: the playhead column, a trim handle, an unselected clip, the field.
        let at = |x: usize, y: usize| &rgba[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4];
        assert_eq!(at(120, 100), PLAYHEAD);
        assert_eq!(at(81, 100), HANDLE);
        assert_eq!(at(10, 100), clip_colour(0, Kind::Video));
        assert_eq!(at(10, 140), clip_colour(0, Kind::Audio));
        assert_eq!(at(10, 10), FIELD);
        assert_eq!(format!("{:016x}", fnv1a(&rgba)), GOLDEN_TRACKS);
    }

    /// `nle2_track_paint_golden`'s hash, measured by the test's first run (FNV-1a 64 of the 320x160 RGBA paint).
    const GOLDEN_TRACKS: &str = "99a12b088401a18f";

    fn btn(down: bool, x: f64, y: f64) -> SMessage {
        btn_mods(down, x, y, 0)
    }

    /// NLE2-MODS: a press / release carrying modifier bits (bandy's `MOD_*`).
    fn btn_mods(down: bool, x: f64, y: f64, mods: u8) -> SMessage {
        SMessage::BrowserButton { down, x, y, mods }
    }

    fn face() -> Face {
        let (st, src) = state();
        Face::new(st, src, (320, 160), PixelOrder::Rgba, false)
    }

    /// NLE2B M1: a press on a drawn handle, a move by N ticks and the release is ONE `trim(Edge, N)`; nothing commits
    /// per move (the preview is `Drag::shown`); the face's own `BrowserClick` after the release is not a second click.
    #[test]
    fn nle2b_drag_trim_is_one_trim() {
        let mut face = face();
        let tl0 = face.state.timeline.clone();
        face.state.select(0, 1).unwrap();
        // B[0,10)@10 on V1: x_of(10) = 80, its in handle 80..84; row 0 is y 80..120.
        face.on(&btn(true, 81.0, 100.0));
        assert_eq!(face.dragging().map(|d| d.kind), Some(DragKind::TrimIn));
        assert!(face.on(&SMessage::BrowserMouseMove(105.0, 100.0)));
        assert_eq!(face.state.timeline, tl0, "nothing commits per move");
        let want = tl0.trim_in(0, 1, 3).unwrap();
        assert_eq!(face.dragging().and_then(|d| d.shown(&face.state)).map(|s| s.timeline), Some(want.clone()));
        assert!(face.on(&btn(false, 105.0, 100.0)));
        assert!(!face.on(&SMessage::BrowserClick(105.0, 100.0)));
        assert_eq!(face.state.timeline, want);
        assert_eq!((face.state.undo_depth(), face.state.playhead, face.state.selection), (1, 0, Some((0, 1))));
        assert!(face.dragging().is_none());
        // Its out handle: B is [3,10)@13..20, x_of(20) = 160, the handle 156..160; drag back 2 ticks.
        face.on(&btn(true, 158.0, 100.0));
        assert_eq!(face.dragging().map(|d| d.kind), Some(DragKind::TrimOut));
        face.on(&SMessage::BrowserMouseMove(142.0, 100.0));
        face.on(&btn(false, 142.0, 100.0));
        let mut by_hand = EditorState::new(want);
        by_hand.select(0, 1).unwrap();
        by_hand.trim(Edge::Out, -2).unwrap();
        assert_eq!(face.state.timeline, by_hand.timeline);
        assert_eq!(face.state.undo_depth(), 2);
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
    }

    /// NLE2B M1: a press on a clip body selects it and drags it; the release is ONE `move_clip`; a video clip dragged
    /// over the audio row stays on its own kind's track.
    #[test]
    fn nle2b_drag_move_is_move_clip() {
        let mut face = face();
        let tl0 = face.state.timeline.clone();
        let (x, y) = face.layout().point(0, 28); // A[23,33)@25 on V1, nothing selected
        face.on(&btn(true, x, y));
        assert_eq!(face.dragging().map(|d| d.kind), Some(DragKind::Move));
        assert_eq!(face.state.selection, Some((0, 2)));
        face.on(&SMessage::BrowserMouseMove(x + 32.0, y + 40.0));
        assert_eq!(face.dragging().map(|d| (d.delta, d.to_track)), Some((4, 0)));
        face.on(&btn(false, x + 32.0, y + 40.0));
        assert_eq!(face.state.timeline, tl0.move_clip(0, 2, 0, 29).unwrap());
        assert_eq!(face.state.selected(), Some(Clip::new(SourceId(0), 23, 33, 29)));
        assert_eq!(face.state.undo_depth(), 1);
    }

    /// NLE2B M1/M2: Esc drops a drag uncommitted; a press and release without movement is the old click.
    #[test]
    fn nle2b_esc_cancels_and_click_stays_click() {
        let mut face = face();
        let tl0 = face.state.timeline.clone();
        let (x, y) = face.layout().point(0, 12);
        face.on(&btn(true, x, y));
        face.on(&SMessage::BrowserMouseMove(x + 40.0, y));
        assert!(face.on(&SMessage::BrowserKey("Escape".into())));
        assert!(face.dragging().is_none());
        assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (tl0, 0));
        let (x, y) = face.layout().point(0, 5);
        face.on(&btn(true, x, y));
        assert!(face.on(&btn(false, x, y)));
        assert_eq!((face.state.selection, face.state.playhead, face.state.undo_depth()), (Some((0, 0)), 5, 0));
        // Once the surface speaks BrowserButton its BrowserClick is not taken (the release was the click).
        let (x, y) = face.layout().point(1, 12);
        assert!(!face.on(&SMessage::BrowserClick(x, y)));
        assert_eq!(face.state.selection, Some((0, 0)));
    }

    /// NLE2B M2: J/K/L shuttle on the clock (L twice = 2x), the GTK key + text pair acts once, I/O trim at the
    /// playhead, undo / redo chords, Delete, Home / End (and AppKit's), zoom keys.
    #[test]
    fn nle2b_keys() {
        let mut face = face();
        let tl0 = face.state.timeline.clone();
        fn gtk(f: &mut Face, k: &str) {
            f.on(&SMessage::BrowserKey(k.into()));
            f.on(&SMessage::BrowserText(k.into()));
        }
        gtk(&mut face, "l");
        assert_eq!((face.playing, face.rate), (true, 1));
        gtk(&mut face, "l");
        assert_eq!(face.rate, 2);
        assert!(face.tick());
        assert_eq!(face.state.playhead, 2);
        gtk(&mut face, "k");
        assert!(!face.playing);
        gtk(&mut face, "j");
        assert_eq!((face.playing, face.rate), (true, -1));
        face.tick();
        assert_eq!(face.state.playhead, 1);
        face.tick();
        face.tick();
        assert_eq!((face.playing, face.state.playhead), (false, 0), "J stops at 0");
        face.on(&SMessage::BrowserKey("space".into()));
        face.on(&SMessage::BrowserText(" ".into()));
        assert_eq!((face.playing, face.rate), (true, 1));
        face.on(&SMessage::BrowserText(" ".into()));
        assert!(!face.playing);

        face.state.select(0, 1).unwrap();
        face.state.seek(13);
        gtk(&mut face, "i");
        let want1 = tl0.trim_in(0, 1, 3).unwrap();
        assert_eq!(face.state.timeline, want1);
        face.state.seek(18);
        gtk(&mut face, "o");
        let want2 = want1.trim_out(0, 1, 8).unwrap();
        assert_eq!(face.state.timeline, want2);
        assert!(face.on(&SMessage::BrowserKey("Ctrl+z".into())));
        assert_eq!(face.state.timeline, want1);
        assert!(face.on(&SMessage::BrowserKey("Ctrl+Shift+Z".into())));
        assert_eq!(face.state.timeline, want2);
        assert!(face.on(&SMessage::BrowserKey("Meta+z".into())));
        assert!(!face.on(&SMessage::BrowserKey("Shift+Z".into())));
        assert!(face.on(&SMessage::BrowserText("Z".into())));
        assert_eq!(face.state.timeline, want2);
        assert!(face.on(&SMessage::BrowserKey("Delete".into())));
        assert_eq!(face.state.timeline, want2.ripple_delete(0, 1).unwrap());
        assert!(face.on(&SMessage::BrowserKey("End".into())));
        assert_eq!(face.state.playhead, face.state.timeline.end());
        assert!(face.on(&SMessage::BrowserKey("Home".into())));
        assert_eq!(face.state.playhead, 0);
        assert!(face.on(&SMessage::BrowserText("\u{f72b}".into())));
        assert_eq!(face.state.playhead, face.state.timeline.end());
        gtk(&mut face, "+");
        assert_eq!(face.state.zoom, 16);
        gtk(&mut face, "-");
        assert_eq!(face.state.zoom, 8);
        assert!(!face.on(&SMessage::BrowserKey("x".into())));
        assert!(!face.on(&SMessage::BrowserText("x".into())));
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
    }

    /// NLE2B M2: the wheel zooms one step per notch about the pointer: the tick under it stays under it.
    #[test]
    fn nle2b_wheel_zoom_anchors_the_pointer_tick() {
        let mut face = face();
        assert_eq!(face.layout().tick_at(160.0), 20);
        assert!(!face.on(&SMessage::BrowserMouseMove(160.0, 100.0)));
        assert!(!face.on(&SMessage::BrowserScroll(0.0, -1.5)), "half a notch is no step");
        assert!(face.on(&SMessage::BrowserScroll(0.0, -1.5)));
        assert_eq!(face.state.zoom, 16);
        let l = face.layout();
        assert_eq!((l.t0, l.tick_at(160.0)), (10, 20));
        assert!(face.on(&SMessage::BrowserScroll(0.0, 3.0)));
        assert_eq!(face.state.zoom, 8);
        assert_eq!(face.layout().tick_at(160.0), 20);
    }

    fn key(f: &mut Face, k: &str) -> bool {
        f.on(&SMessage::BrowserKey(k.into()))
    }

    /// NLE2c M1/M3: a shift-press adds to the set; delete over the set is ONE `ripple_delete_many`, which is the single
    /// ripple deletes applied by hand; one undo step restores it; lift leaves the holes.
    #[test]
    fn nle2c_set_delete_is_one_transaction() {
        let mut face = face();
        let tl0 = face.state.timeline.clone();
        let (x, y) = face.layout().point(0, 5);
        face.on(&btn(true, x, y));
        face.on(&btn(false, x, y));
        assert!(!key(&mut face, "Shift_L"), "a modifier alone repaints nothing");
        let (x, y) = face.layout().point(0, 28);
        face.on(&btn(true, x, y));
        face.on(&btn(false, x, y));
        assert_eq!(face.state.set, vec![(0, 2)], "NLE2-MODS: a bare Shift keydown arms nothing (no latch)");
        let (x0, y0) = face.layout().point(0, 5);
        assert!(face.on(&btn_mods(true, x0, y0, bandy::MOD_SHIFT)));
        assert!(face.on(&btn_mods(false, x0, y0, bandy::MOD_SHIFT)));
        assert_eq!((face.state.selection, face.state.set.clone()), (Some((0, 0)), vec![(0, 0), (0, 2)]));
        assert!(face.verb("delete"));
        let by_hand = tl0.ripple_delete(0, 2).unwrap().ripple_delete(0, 0).unwrap();
        assert_eq!(face.state.timeline, by_hand);
        assert_eq!(face.state.timeline, tl0.ripple_delete_many(&[(0, 0), (0, 2)]).unwrap());
        assert_eq!((face.state.undo_depth(), face.state.selection, face.state.set.len()), (1, None, 0));
        assert!(face.verb("undo"));
        assert_eq!(face.state.timeline, tl0);
        face.state.select(1, 0).unwrap();
        face.state.select_add(1, 2).unwrap();
        assert!(face.verb("lift"));
        assert_eq!(face.state.timeline, tl0.lift_many(&[(1, 0), (1, 2)]).unwrap());
        assert_eq!(face.state.timeline.tracks[1].clips, vec![tl0.tracks[1].clips[1]], "the holes stay");
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
    }

    /// NLE2c M1/M3: dragging a two-clip set onto a neighbour is refused and changes nothing (timeline, undo, set); a legal
    /// set drag is ONE `move_many`; the arrow keys nudge the set one tick; with nothing selected they say so.
    #[test]
    fn nle2c_set_move_refused_leaves_state() {
        let mut face = face();
        let tl0 = face.state.timeline.clone();
        face.state.select(0, 0).unwrap();
        face.state.select_add(0, 1).unwrap();
        let (x, y) = face.layout().point(0, 15); // B's body (B[0,10)@10 butts A@0 and ends 5 before A@25)
        face.on(&btn_mods(true, x, y, bandy::MOD_ALT));
        face.on(&SMessage::BrowserMouseMove(x + 48.0, y));
        assert_eq!(face.dragging().map(|d| (d.kind, d.delta, d.snap)), Some((DragKind::Move, 6, false)));
        face.on(&btn(false, x + 48.0, y));
        assert_eq!(face.last_error.take().as_deref(), Some("drag: clips overlap"));
        assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl0, 0));
        assert_eq!((face.state.selection, face.state.set.clone()), (Some((0, 1)), vec![(0, 0), (0, 1)]));
        face.on(&btn_mods(true, x, y, bandy::MOD_ALT));
        face.on(&SMessage::BrowserMouseMove(x + 40.0, y));
        face.on(&btn(false, x + 40.0, y));
        let want = tl0.move_many(&[(0, 0), (0, 1)], 5).unwrap();
        assert_eq!((&face.state.timeline, face.state.undo_depth()), (&want, 1));
        assert_eq!((face.state.selection, face.state.set.clone()), (Some((0, 1)), vec![(0, 0), (0, 1)]));
        assert!(key(&mut face, "Right"));
        assert_eq!(face.last_error.take().as_deref(), Some("nudge: clips overlap"));
        assert!(face.on(&SMessage::BrowserText("\u{f702}".into())));
        assert_eq!(face.state.timeline, want.move_many(&[(0, 0), (0, 1)], -1).unwrap());
        assert_eq!(face.state.undo_depth(), 2);
        face.state.deselect();
        assert!(key(&mut face, "Left"));
        assert_eq!(face.last_error.take().as_deref(), Some("nudge: no such clip"));
    }

    /// NLE2c M1: a marquee from an empty spot selects the clips its rect crosses (shift adds), a press that barely moves
    /// stays a click, and select-all (Cmd / Ctrl+A as `BrowserSelectAll`) takes every clip.
    #[test]
    fn nle2c_marquee_and_select_all() {
        let mut face = face();
        let l = face.layout();
        assert_eq!(l.crossing(&face.state.timeline, 300.0, 130.0, 172.0, 90.0), vec![(0, 2), (1, 2)]);
        face.on(&btn(true, 172.0, 90.0)); // V1's gap 20..25
        assert_eq!(face.dragging().map(|d| d.kind), Some(DragKind::Marquee));
        assert!(face.on(&SMessage::BrowserMouseMove(300.0, 130.0)));
        face.on(&btn(false, 300.0, 130.0));
        assert_eq!((face.state.selection, face.state.set.clone()), (Some((0, 2)), vec![(0, 2), (1, 2)]));
        face.on(&btn_mods(true, 172.0, 125.0, bandy::MOD_SHIFT));
        face.on(&SMessage::BrowserMouseMove(40.0, 150.0));
        face.on(&btn(false, 40.0, 150.0));
        assert_eq!(face.state.set, vec![(0, 2), (1, 0), (1, 1), (1, 2)]);
        assert_eq!(face.state.selection, Some((0, 2)), "an additive marquee keeps the primary");
        face.on(&btn(true, 172.0, 90.0));
        face.on(&btn(false, 173.0, 91.0));
        assert_eq!((face.state.selection, face.state.set.len(), face.state.playhead), (None, 0, 21), "a click");
        assert!(face.on(&SMessage::BrowserSelectAll));
        assert_eq!(face.state.set.len(), 6);
        assert_eq!(face.state.undo_depth(), 0);
    }

    /// NLE2c M2: snapping picks the nearest edge within SNAP_PX and none outside; a drag snaps a clip edge to another
    /// clip's edge and to the playhead (the guide names it); the no-snap modifier leaves the raw delta.
    #[test]
    fn nle2c_snap_nearest_within_range() {
        let mut face = face();
        let l = face.layout();
        assert_eq!(snap_to(&l, &[10, 20], 84.0), Some(10));
        assert_eq!(snap_to(&l, &[10, 20], 86.0), Some(10));
        assert_eq!(snap_to(&l, &[10, 20], 87.0), None);
        assert_eq!(snap_to(&l, &[10, 20], 120.0), None);
        assert_eq!(snap_to(&l, &[10, 11], 84.0), Some(10), "a tie takes the earlier tick");
        assert_eq!(snap_to(&l, &[10, 11], 87.0), Some(11));
        assert_eq!(snap_targets(&face.state, &[(0, 2), (1, 2)], false), vec![0, 10, 20]);
        let tl0 = face.state.timeline.clone();
        // A@25 on V1 (x 200): 35 px left puts its in edge at 165, 5 px from B's out edge at tick 20 (raw delta -4).
        let (x, y) = l.point(0, 28);
        face.on(&btn(true, x, y));
        face.on(&SMessage::BrowserMouseMove(x - 35.0, y));
        assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((-5, Some(20))));
        let px = face.paint();
        assert_eq!(&px[(150 * 320 + 160) * 4..(150 * 320 + 160) * 4 + 4], GUIDE, "the guide line");
        face.on(&btn(false, x - 35.0, y));
        assert_eq!(face.state.timeline, tl0.move_clip(0, 2, 0, 20).unwrap());
        face.verb("undo");
        // To the playhead at 30 (x 240): 34 px right puts the in edge 6 px from it (raw +4).
        face.state.seek(30);
        face.on(&btn(true, x, y));
        face.on(&SMessage::BrowserMouseMove(x + 34.0, y));
        assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((5, Some(30))));
        key(&mut face, "Escape");
        key(&mut face, "Alt_L");
        face.on(&btn(true, x, y));
        face.on(&SMessage::BrowserMouseMove(x - 35.0, y));
        assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((-5, Some(20))), "NLE2-MODS: a bare Alt keydown arms nothing");
        key(&mut face, "Escape");
        face.on(&btn_mods(true, x, y, bandy::MOD_ALT | bandy::MOD_CONTROL | bandy::MOD_SUPER));
        face.on(&SMessage::BrowserMouseMove(x - 35.0, y));
        assert_eq!(face.dragging().map(|d| (d.delta, d.guide, d.additive)), Some((-4, None, false)));
    }

    /// NLE2c M2: a press on the ruler seeks there, a move scrubs the playhead (snapping to clip edges), the release keeps
    /// it; nothing is edited.
    #[test]
    fn nle2c_ruler_scrub_seeks() {
        let mut face = face();
        let tl0 = face.state.timeline.clone();
        assert!(face.on(&btn(true, 44.0, 70.0)));
        assert_eq!((face.dragging().map(|d| d.kind), face.state.playhead), (Some(DragKind::Scrub), 5));
        assert!(face.on(&SMessage::BrowserMouseMove(77.0, 70.0)));
        assert_eq!((face.state.playhead, face.dragging().and_then(|d| d.guide)), (10, Some(10)), "snapped to tick 10");
        face.on(&SMessage::BrowserMouseMove(124.0, 70.0));
        assert_eq!(face.state.playhead, 15);
        assert!(face.on(&btn(false, 124.0, 70.0)));
        assert_eq!((face.state.playhead, face.state.undo_depth(), &face.state.timeline), (15, 0, &tl0));
        assert!(face.dragging().is_none());
    }

    /// NLE2c M3: the painted editor with a two-clip set: both outlined, the trim handles only on the primary; golden.
    #[test]
    fn nle2c_two_clip_selection_golden() {
        let (mut st, _) = state();
        st.select(0, 0).unwrap();
        st.select_add(0, 2).unwrap();
        st.seek(15);
        let rgba = paint(&st, 320, 160, PixelOrder::Rgba, None, None);
        let at = |x: usize, y: usize| &rgba[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4];
        assert_eq!(at(5, 80), SELECT);
        assert_eq!(at(210, 80), SELECT);
        assert_eq!(at(201, 100), HANDLE);
        assert_eq!(at(2, 100), clip_colour(0, Kind::Video), "no handle on a non-primary");
        assert_eq!(at(100, 80), ROW[0], "B is not selected");
        assert_eq!(format!("{:016x}", fnv1a(&rgba)), GOLDEN_SET);
    }

    /// `nle2c_two_clip_selection_golden`'s hash, measured by the test's first run (FNV-1a 64 of the 320x160 RGBA paint).
    const GOLDEN_SET: &str = "b274098fed21524f";

    /// NLE3d: the fixture with a dissolve on V1's cut at 10 (A[13,23)@0 -> B[0,10)@10), 4 ticks.
    fn dissolve_face() -> Face {
        let (mut st, src) = state();
        st.timeline = st.timeline.add_transition(0, 10, TransitionKind::Dissolve, 4).unwrap();
        Face::new(st, src, (320, 160), PixelOrder::Rgba, false)
    }

    /// NLE3d M1: the widget is hit exactly where `transition_rect` draws it (x 80..112, the foot band y 108..118 of
    /// row 0 at zoom 8), widened to `TRANS_MIN_W` at zoom 1.
    #[test]
    fn nle3d_hit_reads_the_widget_geometry() {
        let face = dissolve_face();
        let (l, tl) = (face.layout(), &face.state.timeline);
        assert_eq!(l.transition_rect(0, &tl.tracks[0].transitions[0]), (80, 108, 112, 118));
        assert_eq!(l.transition_at(tl, 96.0, 113.0), Some((0, 0)));
        assert_eq!(l.transition_at(tl, 80.0, 108.0), Some((0, 0)));
        assert_eq!(l.transition_at(tl, 96.0, 100.0), None, "above the band is the clip");
        assert_eq!(l.transition_at(tl, 79.0, 113.0), None);
        assert_eq!(l.transition_at(tl, 112.0, 113.0), None);
        assert_eq!(l.transition_at(tl, 96.0, 153.0), None, "no transition on A1");
        let mut st = face.state.clone();
        st.zoom = 1;
        let l1 = Layout::of(320, 160, &st);
        assert_eq!(l1.transition_rect(0, &tl.tracks[0].transitions[0]), (10, 108, 18, 118));
        assert_eq!(l1.transition_at(tl, 16.0, 113.0), Some((0, 0)));
    }

    /// NLE3d M1: a press + release on the widget selects the transition (the clips deselected, no drag); a press
    /// elsewhere drops it; Delete is `remove_transition` by hand, ONE undoable edit.
    #[test]
    fn nle3d_click_selects_and_delete_removes() {
        let mut face = dissolve_face();
        let tl0 = face.state.timeline.clone();
        face.state.select(0, 2).unwrap();
        assert!(face.on(&btn(true, 96.0, 113.0)));
        assert!(face.dragging().is_none());
        assert!(face.on(&btn(false, 96.0, 113.0)));
        assert_eq!((face.state.transition, face.state.selection, face.state.set.len()), (Some((0, 0)), None, 0));
        assert_eq!(face.state.playhead, 0, "a widget click does not scrub");
        // A click on a clip drops the transition selection.
        assert!(face.on(&btn(true, 228.0, 100.0)));
        assert!(face.on(&btn(false, 228.0, 100.0)));
        assert_eq!((face.state.transition, face.state.selection), (None, Some((0, 2))));
        // Select it again, then Delete (GTK's named key): the transition goes, the clips stay.
        face.on(&btn(true, 96.0, 113.0));
        face.on(&btn(false, 96.0, 113.0));
        assert!(face.on(&SMessage::BrowserKey("Delete".into())));
        assert_eq!(face.state.timeline, tl0.remove_transition(0, 10).unwrap());
        assert_eq!((face.state.transition, face.state.undo_depth()), (None, 1));
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        assert!(face.state.undo());
        assert_eq!(face.state.timeline, tl0);
    }

    /// NLE3d M1: `]` / `[` (typed, as `BrowserText`) lengthen / shorten the selected transition by one tick through the
    /// add op at the same `at`; a refused length (dur 0, past the incoming clip) leaves the state.
    #[test]
    fn nle3d_brackets_set_the_dur() {
        let mut face = dissolve_face();
        let tl0 = face.state.timeline.clone();
        face.state.select_transition(0, 0).unwrap();
        assert!(face.on(&SMessage::BrowserText("]".into())));
        assert_eq!(face.state.timeline, tl0.add_transition(0, 10, TransitionKind::Dissolve, 5).unwrap());
        assert_eq!(face.state.transition, Some((0, 0)));
        assert!(face.verb("["));
        assert!(face.verb("["));
        assert_eq!(face.state.selected_transition().map(|x| x.dur), Some(3));
        assert!(!face.on(&SMessage::BrowserKey("[".into())), "a one-character key is read from BrowserText only");
        face.state.set_transition_dur(1).unwrap();
        let before = face.state.clone();
        assert!(face.verb("["));
        assert_eq!(face.state, before);
        assert!(face.last_error.take().is_some_and(|e| e.starts_with("[:")));
        face.state.set_transition_dur(10).unwrap();
        let before = face.state.clone();
        assert!(face.verb("]"));
        assert_eq!(face.state, before, "11 ticks is longer than B[0,10)");
        assert_eq!(face.last_error.take(), Some(format!("]: {}", EditError::Handle)));
        // From black: the length change keeps `from_black`.
        let (mut st, _) = state();
        st.timeline = st.timeline.add_transition_from_black(0, 0, TransitionKind::Wipe(WipeDir::Box), 3).unwrap();
        st.select_transition(0, 0).unwrap();
        st.set_transition_dur(4).unwrap();
        assert_eq!(st.selected_transition().map(|x| (x.dur, x.from_black)), Some((4, true)));
        // No transition selected (the plain fixture): the bracket keys are the buttons' in-point trim.
        let (st, src) = state();
        let mut plain = Face::new(st, src, (320, 160), PixelOrder::Rgba, false);
        plain.state.select(0, 1).unwrap();
        plain.on(&SMessage::BrowserText("]".into()));
        assert_eq!(plain.state.selected().map(|c| c.src_in), Some(1));
    }

    /// NLE3d M2: the thumbnail is `preview` of the transition's mid tick, rendered once per `(track, idx, dur, at)`: a
    /// repaint with the key unchanged keeps the cache (a planted still survives it), a length change re-renders.
    #[test]
    fn nle3d_thumbnail_cache_key() {
        let mut face = dissolve_face();
        face.frame();
        assert_eq!((face.transition_key(), face.transition_still()), (None, None));
        face.state.select_transition(0, 0).unwrap();
        let SMessage::SurfaceBlit { pixels, .. } = face.frame() else { panic!("not a blit") };
        assert_eq!(face.transition_key(), Some((0, 0, 4, 10)));
        let want = preview(&face.state.timeline, &face.sources, 12).unwrap().expect("a frame at 12");
        assert_eq!(face.transition_still(), Some(&want));
        // The thumb box sits above row 0 at the widget's x: (80, 44), framed.
        let at = |x: usize, y: usize| pixels[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4].to_vec();
        assert_eq!(at(80, 44), TRANS.to_vec());
        assert_eq!(at(127, 79), TRANS.to_vec());
        let planted = Still { width: 1, height: 1, rgba: vec![9, 9, 9, 255] };
        face.trans_still = Some(((0, 0, 4, 10), Some(planted.clone())));
        face.frame();
        assert_eq!(face.transition_still(), Some(&planted), "same key: no second render");
        face.verb("]");
        face.frame();
        assert_eq!(face.transition_key(), Some((0, 0, 5, 10)));
        let want = preview(&face.state.timeline, &face.sources, transition_mid(&face.state.selected_transition().unwrap()));
        assert_eq!(face.transition_still(), want.unwrap().as_ref());
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
    }

    /// NLE3d M3: the painted widgets (a selected dissolve on the cut at 10, a from-black box wipe on A at 0); golden.
    #[test]
    fn nle3d_widget_paint_golden() {
        let (mut st, _) = state();
        let tl = st.timeline.add_transition(0, 10, TransitionKind::Dissolve, 4).unwrap();
        st.timeline = tl.add_transition_from_black(0, 0, TransitionKind::Wipe(WipeDir::Box), 3).unwrap();
        st.select_transition(0, 1).unwrap();
        st.seek(15);
        let rgba = paint(&st, 320, 160, PixelOrder::Rgba, None, None);
        let at = |x: usize, y: usize| &rgba[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4];
        assert_eq!(at(96, 108), SELECT, "the selected widget's outline");
        assert_eq!(at(100, 112), TRANS_FILL);
        assert_eq!(at(82, 110), TRANS, "the dissolve glyph's corner");
        assert_eq!(at(0, 108), TRANS, "the unselected widget's outline");
        assert_eq!(at(2, 112), FIELD, "the from-black marker");
        assert_eq!(at(5, 110), TRANS, "the box glyph after the marker");
        assert_eq!(at(96, 100), clip_colour(1, Kind::Video), "the clip above the band");
        assert_eq!(format!("{:016x}", fnv1a(&rgba)), GOLDEN_TRANSITIONS);
    }

    /// `nle3d_widget_paint_golden`'s hash, measured by the test's first run (FNV-1a 64 of the 320x160 RGBA paint).
    const GOLDEN_TRANSITIONS: &str = "9b5278f1281cd494";

    /// NLE3cBLACK (SR323): a 5-tick dissolve TO black on B's end at 20 (the gap 20..25): its widget spans x 160..200 at
    /// zoom 8 with the black marker on its right (incoming) edge; `[` keeps it a to-black; golden.
    #[test]
    fn nle3cblack_to_black_marker_golden() {
        let (mut st, _) = state();
        st.timeline = st.timeline.add_transition_to_black(0, 20, TransitionKind::Dissolve, 5).unwrap();
        let l = Layout::of(320, 160, &st);
        assert_eq!(l.transition_rect(0, &st.timeline.tracks[0].transitions[0]), (160, 108, 200, 118));
        let rgba = paint(&st, 320, 160, PixelOrder::Rgba, None, None);
        let at = |x: usize, y: usize| &rgba[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4];
        assert_eq!(at(160, 108), TRANS, "the widget's outline");
        assert_eq!(at(197, 112), FIELD, "the to-black marker");
        assert_eq!(at(194, 112), TRANS_FILL, "the bracket left of the marker");
        assert_eq!(at(162, 110), TRANS, "the dissolve glyph's corner");
        assert_eq!(format!("{:016x}", fnv1a(&rgba)), GOLDEN_TO_BLACK);
        st.select_transition(0, 0).unwrap();
        st.set_transition_dur(4).unwrap();
        assert_eq!(st.selected_transition().map(|x| (x.at, x.dur, x.to_black)), Some((20, 4, true)));
    }

    /// `nle3cblack_to_black_marker_golden`'s hash, measured by the test's first run (FNV-1a 64 of the 320x160 RGBA paint).
    const GOLDEN_TO_BLACK: &str = "7c909fba68bf567e";
}

// ---------------------------------------------------------------------------------------------------------------------
// THE METERS (NLE4C, LEDGER SR301)
// ---------------------------------------------------------------------------------------------------------------------

/// The meter strip's width (px), at the canvas's right edge.
pub const METER_W: u32 = 6;
/// The strip's bottom: readings at or below this are an empty bar (linear in dB up to 0 dBFS).
pub const METER_VIEW_DB: f64 = -60.0;
const METER_BG: [u8; 4] = [0x10, 0x10, 0x14, 255];
const METER_RMS: [u8; 4] = [0x40, 0xd0, 0x60, 255];
const METER_PEAK: [u8; 4] = [0xff, 0xe0, 0x40, 255];

impl Face {
    /// Feed the meters the `n` played ticks from `from` (`stria::nle::MixMeters::feed`: the render of the whole timeline
    /// there, so a track's meter reads its share of what is sent and the master the sum). A reverse shuttle (`n <= 0`)
    /// renders no audio: the meters fall to nothing. An error is the face's `last_error` and resets them.
    pub fn feed_meters(&mut self, from: i64, n: i64) {
        let end = (from + n).min(self.state.timeline.end());
        if n <= 0 || end <= from {
            self.meters.reset();
            return;
        }
        if let Err(e) = self.meters.feed(&self.state.timeline, from..end, &self.sources) {
            self.last_error = Some(format!("meters: {e}"));
            self.meters.reset();
        }
    }
    /// The meters as last fed.
    pub fn meters(&self) -> &MixMeters {
        &self.meters
    }
    /// What the strip paints: the meters while playing, nothing when stopped (never a frozen reading).
    fn meter_levels(&self) -> Vec<MeterLevel> {
        if self.playing { self.meters.levels() } else { Vec::new() }
    }
}

/// The meter strip over a painted canvas: each audio track's bar inside its row, the master's down the preview band, at
/// x `w - METER_W - 1 .. w - 1`; the RMS filled from the bottom, the peak a 1 px line, both linear in dB over
/// [`METER_VIEW_DB`]..0. No levels paint nothing.
fn paint_meters(px: Vec<u8>, l: Layout, order: PixelOrder, levels: &[MeterLevel]) -> Vec<u8> {
    if levels.is_empty() {
        return px;
    }
    let mut c = Canvas { w: l.w, h: l.h, order, px };
    let (x1, x0) = (l.w as i64 - 1, l.w as i64 - 1 - METER_W as i64);
    let frac = |db: f64| ((db - METER_VIEW_DB) / -METER_VIEW_DB).clamp(0.0, 1.0);
    for lv in levels {
        let (y0, y1) = match lv.track {
            Some(t) if t < l.rows => (l.row_y(t) + 2, l.row_y(t) + ROW_H as i64 - 2),
            Some(_) => continue,
            None => (1, l.preview_h as i64 - 1),
        };
        if y1 - y0 < 2 {
            continue;
        }
        let span = (y1 - y0) as f64;
        c.rect(x0, y0, x1, y1, METER_BG);
        let fill = (frac(lv.rms_db) * span).round() as i64;
        c.rect(x0, y1 - fill, x1, y1, METER_RMS);
        let pk = (frac(lv.peak_db) * span).round() as i64;
        if pk > 0 {
            let y = (y1 - pk).max(y0);
            c.rect(x0, y, x1, y + 1, METER_PEAK);
        }
    }
    c.px
}

#[cfg(test)]
mod nle4c_tests {
    use super::*;
    use crate::nle::synth_sample;
    use gneiss_pal::dsp::audio::meter::dbfs;

    /// NLE4C M3: the strip over the fixture's paint: the master's RMS fill and peak line down the preview band, track 1's
    /// inside its row, the rest of the canvas the plain paint; golden.
    #[test]
    fn nle4c_meter_strip_golden() {
        let mut st = EditorState::new(synth_fixture().0);
        st.seek(15);
        let l = Layout::of(320, 160, &st);
        let base = paint(&st, 320, 160, PixelOrder::Rgba, None, None);
        let levels = [
            MeterLevel { track: Some(1), peak_db: -6.0, rms_db: -30.0 },
            MeterLevel { track: None, peak_db: 0.0, rms_db: -15.0 },
        ];
        assert_eq!(paint_meters(base.clone(), l, PixelOrder::Rgba, &[]), base, "no levels paint nothing");
        let rgba = paint_meters(base.clone(), l, PixelOrder::Rgba, &levels);
        let at = |x: usize, y: usize| &rgba[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4];
        let x = 316;
        // track 1's row: y 122..158 (36 px); RMS -30 dB fills 18 px from 158 up, the -6 dB peak sits 32 px up
        assert_eq!((l.row_y(1), l.preview_h), (120, 64));
        assert_eq!(at(x, 157), METER_RMS);
        assert_eq!(at(x, 140), METER_RMS);
        assert_eq!(at(x, 139), METER_BG);
        assert_eq!(at(x, 126), METER_PEAK);
        // the master: y 1..63 (62 px); RMS -15 dB fills 47 px (from 16 down), the 0 dB peak is the top row
        assert_eq!(at(x, 16), METER_RMS);
        assert_eq!(at(x, 15), METER_BG);
        assert_eq!(at(x, 1), METER_PEAK);
        assert_eq!(at(319, 140), &base[(140 * 320 + 319) * 4..(140 * 320 + 319) * 4 + 4], "the strip ends at w - 1");
        assert_eq!(at(x, 100), &base[(100 * 320 + x) * 4..(100 * 320 + x) * 4 + 4], "no strip on a video row");
        assert_eq!(format!("{:016x}", fnv1a(&rgba)), GOLDEN_METERS);
    }

    /// `nle4c_meter_strip_golden`'s hash, measured by the test's first run (FNV-1a 64 of the 320x160 RGBA paint).
    const GOLDEN_METERS: &str = "e813db12ef577e17";

    /// NLE4C M2: a played tick feeds the meters from the render: one audio track at 0 dB passes bit for bit, so its
    /// meter IS the master's, and the peak is the loudest sample sent (reel A from source tick 13: samples 24960..26880,
    /// the loudest 6879 / 32768); a stopped face paints no strip; a reverse shuttle resets them.
    #[test]
    fn nle4c_played_tick_feeds_the_meters() {
        let (tl, src) = synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(face.meters().levels().is_empty());
        face.verb("play");
        assert!(face.tick());
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        let lv = face.meters().levels();
        assert_eq!(lv.len(), 2);
        assert_eq!((lv[0].track, lv[1].track), (Some(1), None));
        assert_eq!((lv[0].peak_db, lv[0].rms_db), (lv[1].peak_db, lv[1].rms_db));
        let loud = (24_960u64..26_880).map(|i| synth_sample(0, i)).max().unwrap();
        assert_eq!(loud, 6879);
        assert!((lv[1].peak_db - dbfs(loud as f64 / 32768.0)).abs() < 1e-9, "{}", lv[1].peak_db);
        assert!(lv[1].rms_db < lv[1].peak_db && lv[1].rms_db > -96.0);
        assert_eq!(face.meter_levels().len(), 2, "playing: the strip shows");
        face.verb("play");
        assert!(face.meter_levels().is_empty(), "stopped: no strip");
        face.feed_meters(face.state.playhead, -1);
        assert!(face.meters().levels().is_empty(), "reverse resets");
    }
}
