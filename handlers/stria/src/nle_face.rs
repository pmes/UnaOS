// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Stria's editor face (NLE2, LEDGER SR201): the model, the one painter and the bus service behind every native face.
//!
//! CHARTER: Stria — the non-linear editor's face model over `timeline_core` (shared-core, BP-handlers-4/-14).
//!
//! * [`EditorState`]: the timeline, the selected clip, the playhead, the zoom (px per tick) and the undo / redo stacks.
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

use std::time::Duration;

use bandy::{SMessage, Synapse};
pub use text_host::line::PixelOrder;
use text_host::line::TextStyle;
use timeline_core::{Clip, EditError, Kind, Timeline, edl};

use crate::nle::{RenderError, RenderSink, Sources, render, synth_source};

/// The editor surface's id (the tetra `Surface` and every `SurfaceBlit` / `Input` target).
pub const SURFACE: &str = "stria.nle";
/// Undo depth kept.
pub const UNDO_MAX: usize = 256;
pub const ZOOM_MIN: u32 = 1;
pub const ZOOM_MAX: u32 = 64;
/// The zoom a fresh state opens at (px per tick).
pub const ZOOM_DEFAULT: u32 = 8;

/// Which edge of the selected clip a trim moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    In,
    Out,
}

/// The editor's state (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct EditorState {
    pub timeline: Timeline,
    /// `(track, clip index)`.
    pub selection: Option<(usize, usize)>,
    pub playhead: i64,
    /// Pixels per tick, `ZOOM_MIN..=ZOOM_MAX`.
    pub zoom: u32,
    undo: Vec<Timeline>,
    redo: Vec<Timeline>,
}

impl EditorState {
    pub fn new(timeline: Timeline) -> EditorState {
        EditorState { timeline, selection: None, playhead: 0, zoom: ZOOM_DEFAULT, undo: Vec::new(), redo: Vec::new() }
    }
    /// Open a CMX 3600 EDL (`fps` when it carries no `* UNA FPS` line).
    pub fn open(text: &str, fps: u32) -> Result<EditorState, edl::EdlError> {
        Ok(EditorState::new(edl::parse(text, fps)?))
    }
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }
    /// The selected clip.
    pub fn selected(&self) -> Option<Clip> {
        let (t, i) = self.selection?;
        self.timeline.tracks.get(t)?.clips.get(i).copied()
    }
    pub fn select(&mut self, track: usize, idx: usize) -> Result<(), EditError> {
        let tr = self.timeline.tracks.get(track).ok_or(EditError::NoTrack)?;
        if idx >= tr.clips.len() {
            return Err(EditError::NoClip);
        }
        self.selection = Some((track, idx));
        Ok(())
    }
    /// Select the clip on `track` showing tick `t`.
    pub fn select_at(&mut self, track: usize, t: i64) -> Result<(), EditError> {
        if track >= self.timeline.tracks.len() {
            return Err(EditError::NoTrack);
        }
        let idx = self.timeline.clip_at(track, t).ok_or(EditError::NoClip)?;
        self.selection = Some((track, idx));
        Ok(())
    }
    pub fn deselect(&mut self) {
        self.selection = None;
    }
    /// Move the playhead to `t`, clamped to `0..=timeline.end()`.
    pub fn seek(&mut self, t: i64) {
        self.playhead = t.clamp(0, self.timeline.end().max(0));
    }
    pub fn nudge(&mut self, dt: i64) {
        self.seek(self.playhead.saturating_add(dt));
    }
    /// Cut the selected track's clip at the playhead (`Timeline::split`); the right half is selected.
    pub fn split_at_playhead(&mut self) -> Result<(), EditError> {
        let (track, _) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.split(track, self.playhead)?;
        self.commit(tl);
        self.selection = self.timeline.clip_at(track, self.playhead).map(|i| (track, i));
        Ok(())
    }
    /// Move the selected clip's in (`Timeline::trim_in`) or out (`Timeline::trim_out`) point by `delta` source ticks.
    pub fn trim(&mut self, edge: Edge, delta: i64) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let c = self.selected().ok_or(EditError::NoClip)?;
        let tl = match edge {
            Edge::In => self.timeline.trim_in(track, idx, c.src_in + delta)?,
            Edge::Out => self.timeline.trim_out(track, idx, c.src_out + delta)?,
        };
        self.commit(tl);
        Ok(())
    }
    /// Move the selected clip to `to_at` on `to_track` (`Timeline::move_clip`); the selection follows the clip.
    pub fn move_selected(&mut self, to_track: usize, to_at: i64) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.move_clip(track, idx, to_track, to_at)?;
        self.commit(tl);
        self.selection = self.timeline.clip_at(to_track, to_at).map(|i| (to_track, i));
        Ok(())
    }
    /// Remove the selected clip and close the hole (`Timeline::ripple_delete`).
    pub fn ripple_delete(&mut self) -> Result<(), EditError> {
        let (track, idx) = self.selection.ok_or(EditError::NoClip)?;
        let tl = self.timeline.ripple_delete(track, idx)?;
        self.commit(tl);
        self.selection = None;
        Ok(())
    }
    /// Step back one edit; false when there is none.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else { return false };
        let cur = std::mem::replace(&mut self.timeline, prev);
        self.redo.push(cur);
        self.settle();
        true
    }
    /// Re-apply the last undone edit; false when there is none.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        let cur = std::mem::replace(&mut self.timeline, next);
        self.undo.push(cur);
        self.settle();
        true
    }
    /// Double (`step > 0`) or halve (`step < 0`) the zoom, `|step|` times, within `ZOOM_MIN..=ZOOM_MAX`.
    pub fn zoom_by(&mut self, step: i32) {
        let mut z = self.zoom;
        for _ in 0..step.unsigned_abs().min(8) {
            z = if step > 0 { z.saturating_mul(2) } else { z / 2 };
        }
        self.zoom = z.clamp(ZOOM_MIN, ZOOM_MAX);
    }
    /// THE one edit path: the op's value replaces the timeline, the old one goes on the undo stack.
    fn commit(&mut self, tl: Timeline) {
        let old = std::mem::replace(&mut self.timeline, tl);
        self.undo.push(old);
        if self.undo.len() > UNDO_MAX {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.settle();
    }
    /// Keep the selection and the playhead valid for the current timeline.
    fn settle(&mut self) {
        if self.selected().is_none() {
            self.selection = None;
        }
        self.seek(self.playhead);
    }
}

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
    /// The shuttle rate in frames per clock tick (J/K/L; 1 for play).
    pub rate: i64,
    drag: Option<Drag>,
    pointer: Option<(f64, f64)>,
    /// The surface sends `BrowserButton` (its release then stands for `BrowserClick`).
    buttons: bool,
    wheel: f64,
    /// A wheel zoom's anchored view: `(t0, the playhead it was anchored at)`.
    view: Option<(i64, i64)>,
}

impl Face {
    /// `labels`: name the clips through the host font stack (off for a golden).
    pub fn new(state: EditorState, sources: Sources, canvas: (u32, u32), order: PixelOrder, labels: bool) -> Face {
        let labels = labels.then(|| TextStyle::new(&["sans"], 11.0));
        Face { state, sources, canvas, order, playing: false, labels, still: None, last_error: None, rate: 1, drag: None, pointer: None, buttons: false, wheel: 0.0, view: None }
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
            SMessage::BrowserButton { down: true, x, y } => self.press(*x, *y),
            SMessage::BrowserButton { down: false, x, y } => self.release(*x, *y),
            SMessage::BrowserMouseMove(x, y) => self.hover(*x, *y),
            SMessage::BrowserScroll(_, dy) => self.scroll(*dy),
            SMessage::BrowserZoom(d) if *d != 0 => {
                self.state.zoom_by(*d);
                true
            }
            SMessage::BrowserKey(k) => self.key(k, false),
            SMessage::BrowserText(t) => self.key(t, true),
            _ => false,
        }
    }
    /// A transport verb ([`VERBS`]); false for any other word.
    pub fn verb(&mut self, v: &str) -> bool {
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
        self.state.nudge(self.rate);
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
        paint_in(st, l, self.order, self.labels.as_ref(), self.still.as_ref())
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
    /// A clip body (`EditorState::move_selected`).
    Move,
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
    /// Ticks moved since the press.
    pub delta: i64,
    /// Where a move lands (a track of the clip's kind).
    pub to_track: usize,
}

impl Drag {
    /// A press at `(x, y)`: on the selected clip's drawn trim handle (the rects [`paint`] draws) a trim, on a clip
    /// body a move (the clip is selected); anywhere else no drag.
    pub fn begin(st: &mut EditorState, l: Layout, x: f64, y: f64) -> Option<Drag> {
        let origin_tick = l.tick_at(x);
        if let (Some((track, idx)), Some(c)) = (st.selection, st.selected()) {
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
                    return Some(Drag { kind, track, idx, origin_tick, layout: l, delta: 0, to_track: track });
                }
            }
        }
        let Hit::Clip(track, idx, _) = l.hit(&st.timeline, x, y) else { return None };
        st.select(track, idx).ok()?;
        Some(Drag { kind: DragKind::Move, track, idx, origin_tick, layout: l, delta: 0, to_track: track })
    }
    /// The pointer is at `(x, y)`: the tick delta, and for a move the row under it when that track's kind matches.
    pub fn update(&mut self, st: &EditorState, x: f64, y: f64) {
        self.delta = self.layout.tick_at(x) - self.origin_tick;
        if self.kind != DragKind::Move {
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
        self.delta != 0 || self.to_track != self.track
    }
    /// THE commit of the gesture: one `trim` or `move_selected` on the dragged clip.
    pub fn finish(&self, st: &mut EditorState) -> Result<(), EditError> {
        st.select(self.track, self.idx)?;
        match self.kind {
            DragKind::TrimIn => st.trim(Edge::In, self.delta),
            DragKind::TrimOut => st.trim(Edge::Out, self.delta),
            DragKind::Move => {
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
    /// The button went down: a drag may begin (a live one is dropped uncommitted).
    fn press(&mut self, x: f64, y: f64) -> bool {
        self.buttons = true;
        self.pointer = Some((x, y));
        let l = self.layout();
        let dropped = self.drag.take().is_some();
        let sel = self.state.selection;
        self.drag = Drag::begin(&mut self.state, l, x, y);
        dropped || self.state.selection != sel
    }
    /// The button came up: a drag that moved commits once; otherwise it is the click it always was.
    fn release(&mut self, x: f64, y: f64) -> bool {
        self.pointer = Some((x, y));
        let Some(mut d) = self.drag.take() else { return self.click(x, y) };
        d.update(&self.state, x, y);
        if !d.moved() {
            return self.click(x, y);
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
        let before = (d.delta, d.to_track);
        d.update(&self.state, x, y);
        (d.delta, d.to_track) != before
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
            "cancel" => return self.drag.take().is_some(),
            _ => return false,
        };
        if let Err(e) = r {
            self.last_error = Some(format!("{act}: {e}"));
        }
        true
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
        SMessage::BrowserButton { down, x, y }
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
}
