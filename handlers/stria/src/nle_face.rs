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

use std::time::Duration;

use bandy::{SMessage, Synapse};
pub use text_host::line::PixelOrder;
use text_host::line::TextStyle;
use timeline_core::{Clip, EditError, Kind, Timeline, edl};

use crate::nle::{RenderError, RenderSink, Sources, render, synth_source};

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
    let l = Layout::of(w, h, st);
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
    // The selection: an outline and both trim handles.
    if let (Some((row, _)), Some(clip)) = (st.selection, st.selected()) {
        let (x0, x1) = (l.x_of(clip.at), l.x_of(clip.end()));
        let (y0, y1) = (l.row_y(row), l.row_y(row) + ROW_H as i64);
        c.frame(x0, y0, x1, y1, SELECT);
        c.rect(x0, y0 + 2, x0 + HANDLE_W, y1 - 2, HANDLE);
        c.rect(x1 - HANDLE_W, y0 + 2, x1, y1 - 2, HANDLE);
    }
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
pub const VERBS: [&str; 13] = ["play", "prev", "next", "split", "delete", "undo", "redo", "zoom-", "zoom+", "[", "]", "{", "}"];

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
}

impl Face {
    /// `labels`: name the clips through the host font stack (off for a golden).
    pub fn new(state: EditorState, sources: Sources, canvas: (u32, u32), order: PixelOrder, labels: bool) -> Face {
        let labels = labels.then(|| TextStyle::new(&["sans"], 11.0));
        Face { state, sources, canvas, order, playing: false, labels, still: None, last_error: None }
    }
    /// The preview frame of the last [`Face::frame`].
    pub fn still(&self) -> Option<&Still> {
        self.still.as_ref()
    }
    /// One message from the surface or the buttons; true when the canvas must be repainted.
    pub fn on(&mut self, msg: &SMessage) -> bool {
        match msg {
            SMessage::Input { target, text } if target == SURFACE => self.verb(text),
            SMessage::BrowserClick(x, y) => self.click(*x, *y),
            SMessage::BrowserResize(w, h) if *w > 0 && *h > 0 && (*w, *h) != self.canvas => {
                self.canvas = (*w, *h);
                true
            }
            _ => false,
        }
    }
    /// A transport verb ([`VERBS`]); false for any other word.
    pub fn verb(&mut self, v: &str) -> bool {
        let st = &mut self.state;
        let r = match v {
            "play" => {
                self.playing = !self.playing;
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
        let l = Layout::of(self.canvas.0, self.canvas.1, &self.state);
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
    /// One playback tick: the playhead steps one frame; playback stops at the end. True when it moved or stopped.
    pub fn tick(&mut self) -> bool {
        if !self.playing {
            return false;
        }
        if self.state.playhead >= self.state.timeline.end() {
            self.playing = false;
            return true;
        }
        self.state.nudge(1);
        true
    }
    /// The canvas as painted now (the preview at the playhead rendered first).
    pub fn paint(&mut self) -> Vec<u8> {
        self.still = match preview(&self.state.timeline, &self.sources, self.state.playhead) {
            Ok(s) => s,
            Err(e) => {
                self.last_error = Some(format!("preview: {e}"));
                None
            }
        };
        let (w, h) = self.canvas;
        paint(&self.state, w, h, self.order, self.labels.as_ref(), self.still.as_ref())
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
}
