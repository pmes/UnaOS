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
//! * NLEHOSTSAVE (LEDGER SR677): `save` (button, `Ctrl+s` / `Meta+s`) writes the timeline through `edl::write` to the path the document was opened from.
//! * NLEPLAYAUDIO (LEDGER SR676): a forward played tick is ONE `render` through `PlaySink` (the still, the meters, the mix to a `media::AudioOut` installed by [`Face::with_audio`]).

use std::time::Duration;

use bandy::{SMessage, Synapse};
pub use text_host::line::PixelOrder;
use text_host::line::TextStyle;
use timeline_core::{Clip, EditError, Kind, Timeline};

use crate::nle::{MeterLevel, MixMeters, RenderError, RenderSink, Sources, render, synth_source};

/// The editor surface's id (the tetra `Surface` and every `SurfaceBlit` / `Input` target).
pub const SURFACE: &str = "stria.nle";
/// NLE6 (SR207): the editor state is timeline_core's (`timeline_core::editor`), shared with the on-UnaOS face.
pub use timeline_core::editor::{Edge, EditorState, UNDO_MAX, ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};

// ---------------------------------------------------------------------------------------------------------------------
// THE ONE GEOMETRY AND THE ONE PAINTER (timeline_core::view since NLE6KEYS, rmbp-ledger B961)
// ---------------------------------------------------------------------------------------------------------------------

/// NLE6KEYS (rmbp-ledger B961): the geometry ([`Layout`], [`Hit`], [`Role`]) and the painter are timeline_core's
/// (`timeline_core::view`), the ONE view the kernel Player's EDL band paints too; re-exported so every `nle_face::` path stands.
pub use timeline_core::view::{Drag, DragKind, HANDLE_W, Hit, Layout, MARQUEE_MIN, ROW_H, RULER_H, Role, SNAP_PX, Still, THUMB_H, THUMB_W, TRANS_H, TRANS_MIN_W, fnv1a, snap_targets, snap_to, transition_mid}; // NLE6FACEDRAG (rmbp-ledger B975): the drag model moved to timeline_core::view, both faces call it
use timeline_core::view::{self, Canvas, FIELD, Order, TRANS};
#[cfg(test)]
use timeline_core::view::{GUIDE, HANDLE, PLAYHEAD, ROW, SELECT, TRANS_FILL, clip_colour};
#[cfg(test)]
use timeline_core::{TransitionKind, WipeDir};

const LABEL: [u8; 4] = view::LABEL; // NLE6KEYSLABELS (rmbp-ledger B968): the view's colour table holds the one literal

/// The view's byte order for a surface's [`PixelOrder`].
fn vo(o: PixelOrder) -> Order {
    match o {
        PixelOrder::Rgba => Order::Rgba,
        PixelOrder::Bgra => Order::Bgra,
    }
}

/// A [`PixelOrder`] for the view's byte order (the label shaper's).
fn po(o: Order) -> PixelOrder {
    match o {
        Order::Rgba => PixelOrder::Rgba,
        Order::Bgra => PixelOrder::Bgra,
    }
}

/// The view's label hook over text_host: the clip names and the ruler's seconds.
struct Labels<'a>(&'a TextStyle);

impl view::Text for Labels<'_> {
    fn text(&mut self, c: &mut Canvas, s: &str, x: f32, baseline: f32, clip: (i32, i32)) {
        let (w, h, order) = (c.w, c.h, po(c.order));
        self.0.shape(s).paint(&mut c.px, w, h, w as usize * 4, order, x, baseline, LABEL, clip);
    }
}

/// Paint the editor on a `w` x `h` canvas in `order` (see the module docs). `labels` names the clips (and the
/// seconds on the ruler) through text_host; `preview` is the frame at the playhead.
pub fn paint(st: &EditorState, w: u32, h: u32, order: PixelOrder, labels: Option<&TextStyle>, preview: Option<&Still>) -> Vec<u8> {
    paint_in(st, Layout::of(w, h, st), order, labels, preview)
}

/// [`paint`] in a given layout (the face's view, which a wheel zoom anchors at the pointer: [`Face::layout`]): timeline_core's
/// `view::paint_in`, the labels through [`Labels`].
pub fn paint_in(st: &EditorState, l: Layout, order: PixelOrder, labels: Option<&TextStyle>, preview: Option<&Still>) -> Vec<u8> {
    let mut lab = labels.map(Labels);
    view::paint_in(st, l, vo(order), lab.as_mut().map(|t| t as &mut dyn view::Text), preview)
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
/// NLESIZEPOLICYUI (SR402): `size` cycles the selected clip's reel's size policy (timeline_core's `EditorState::verb`).
/// NLEHOSTSAVE (SR677): `save` writes the timeline to the document's path through timeline_core's ONE writer.
pub const VERBS: [&str; 28] = ["play", "prev", "next", "split", "delete", "undo", "redo", "zoom-", "zoom+", "[", "]", "{", "}", "lift", "size", "save", "export", "in", "out", "kind", "ease", "slip-", "slip+", "slide-", "slide+", "zoom-fit", "zoom-sel", "help"]; // NLEFACEEXPORT (SR694): `export [<dir>] [h264|png|i420|y4m|raw]` off the face's thread (file tail); NLETRANSITIONFACE (SR691): `kind` / `ease` cycle the selected transition (file tail); NLE5DEXPORTRANGE (SR717): `in` / `out` mark the export range (file tail); NLE3ESLIPFACE (SR737): one-frame slip / slide (file tail); NLE5DKEYSHELPBUTTON (SR765): help (file tail); STRIAZOOMTOFIT (SR859): zoom-fit / zoom-sel (file tail)

/// One editor window's model: the state, the media, the canvas size and pixel order, the transport.
pub struct Face {
    pub state: EditorState,
    sources: std::sync::Arc<Sources>, // NLEFACEEXPORT (SR694): shared by the preview and a running export
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
    /// SURFACEID (SR332): the Surface number this face's pointer comes from (`None`: every surface, one-surface trees).
    surface: Option<u32>,
    /// NLE2-SPLIT (SR339): the preview on its own Surface ([`Face::on_surfaces`]); `canvas` is then the tracks Surface's.
    split: Option<Split>,
    /// NLEHOSTSAVE (SR677): the EDL this face was opened from ([`Face::with_doc`]); `None` (the synth fixture): `save` refuses.
    doc_path: Option<std::path::PathBuf>,
    /// NLEHOSTSAVE (SR677): the timeline as last opened or saved; [`Face::dirty`] is the current one differing from it.
    clean: Timeline,
    /// NLEPLAYAUDIO (SR676): the play path (the device, the played still, where the last played render ended).
    play: PlayAudio,
    /// NLEFACEEXPORT (SR694): the running export ([`ExportJob`]), cleared when its result is reported.
    export: Option<ExportJob>,
    /// NLE5DKEYSHELP (SR744): the keys help overlay is shown over the timeline band ([`Face::paint_help`]).
    help: bool,
    hover_ticks: Option<((usize, usize), u16)>, // STRIAINFOREELHOVERDELAY (SR789): the hovered clip and the face ticks it has stood (file tail)
    zoom_fit: Option<(bool, u32, Option<(i64, i64)>)>, // STRIAZOOMFITKEYS (SR907): the last fit (sel, zoom, view); a resize refits while it still holds (file tail)
}

impl Face {
    /// `labels`: name the clips through the host font stack (off for a golden).
    pub fn new(state: EditorState, sources: Sources, canvas: (u32, u32), order: PixelOrder, labels: bool) -> Face {
        let labels = labels.then(|| TextStyle::new(&["sans"], 11.0));
        Face { clean: state.timeline.clone(), doc_path: None, state, sources: std::sync::Arc::new(sources), export: None, canvas, order, playing: false, labels, still: None, last_error: None, rate: 1, drag: None, pointer: None, buttons: false, wheel: 0.0, view: None, trans_still: None, meters: MixMeters::default(), surface: None, split: None, play: PlayAudio::default(), help: false, hover_ticks: None, zoom_fit: None }
    }
    /// The preview frame of the last [`Face::frame`].
    pub fn still(&self) -> Option<&Still> {
        self.still.as_ref()
    }
    /// One message from the surface or the buttons; true when the canvas must be repainted.
    pub fn on(&mut self, msg: &SMessage) -> bool {
        if !self.takes(msg) { return false; } if let Some(r) = self.on_preview_resize(msg) { return r; } if let Some(r) = self.help_input(msg) { return r; } // NLE2-SPLIT (SR339): the preview Surface's resize; SURFACEID (SR332): another Surface's pointer is not this face's
        match msg {
            SMessage::Input { target, text } if target == SURFACE => self.verb(text),
            SMessage::BrowserClick(x, y, _) if !self.buttons => self.click(*x, *y),
            SMessage::BrowserResize(w, h, _) if *w > 0 && *h > 0 && (*w, *h) != self.canvas => {
                self.canvas = (*w, *h); self.refit_on_resize(); // STRIAZOOMFITKEYS (SR907): refit while the last view word was a fit (file tail)
                true
            }
            SMessage::BrowserButton { down: true, x, y, mods, .. } => self.press(*x, *y, *mods),
            SMessage::BrowserButton { down: false, x, y, .. } => self.release(*x, *y),
            SMessage::BrowserMouseMove(x, y, _) => self.hover(*x, *y),
            SMessage::BrowserScroll(_, dy, _) => self.scroll(*dy),
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
        if let Some(r) = self.transition_verb(v) { return r; } if let Some(r) = self.save_verb(v) { return r; } if let Some(r) = self.export_verb(v) { return r; } if let Some(r) = self.mark_verb(v) { return r; } if let Some(r) = self.fit_verb(v) { return r; } // NLEHOSTSAVE (SR677): the save verb (file tail)
        let r = match v {
            "play" => {
                self.playing = !self.playing;
                self.rate = 1; self.sync_audio(); // NLEPLAYAUDIO (SR676): the device follows the transport
                Ok(())
            }
            "help" => { self.help = !self.help; Ok(()) } // NLE5DKEYSHELP (SR744): the keys help overlay on / off (file tail)
            v if slipslide_word(v).is_some() => slipslide_word(v).and_then(|w| self.state.verb(&w)).unwrap_or(Err(EditError::NoClip)), // NLE3ESLIPFACE (SR737): the one-frame words onto SR704's `slip <d>` / `slide <d>` (file tail); NLE3ESLIPSTEP (SR752): and the SLIP_STEP words `slip-10` ... (keys only)
            _ => match self.state.verb(v) {
                Some(r) => r, // NLE6KEYS (rmbp-ledger B961): timeline_core's ONE verb dispatch (the kernel Player's keys call it too)
                None => return false,
            },
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
        if self.hover_dwell_tick() && !self.playing { return true; } // STRIAINFOREELHOVERDELAY (SR789): the hover dwell counts on THE face clock; the crossing tick repaints (file tail)
        if !self.playing {
            return false;
        }
        let ph = self.state.playhead;
        if (self.rate > 0 && ph >= self.state.timeline.end()) || (self.rate <= 0 && ph <= 0) {
            self.playing = false; self.sync_audio(); // NLEPLAYAUDIO (SR676)
            return true;
        }
        if !self.play_tick(ph) { self.feed_meters(ph, self.rate); self.state.nudge(self.rate); } // NLEPLAYAUDIO (SR676): a forward played tick is ONE render, shown at its own playhead
        true
    }
    /// The canvas as painted now (the preview at the playhead rendered first). While a drag is live the drag's
    /// preview state ([`Drag::shown`]) is painted: the edit as it would commit, the timeline itself untouched.
    pub fn paint(&mut self) -> Vec<u8> {
        let r = self.pointer_role();
        self.paint_role(r)
    }
    /// The canvas as the surface's blit.
    pub fn frame(&mut self) -> SMessage {
        let (width, height) = self.canvas;
        SMessage::SurfaceBlit { url: SURFACE.into(), width, height, pixels: self.paint(), damage: None, scroll: None }
    }
}

/// Serve one editor face on the bus: the first frame, then every surface / button message through [`Face::on`] and a
/// fresh blit after each change; while playing, a clock at the timeline's frame rate steps the playhead.
pub async fn serve(bus: Synapse, mut rx: tokio::sync::broadcast::Receiver<SMessage>, mut face: Face) {
    use tokio::sync::broadcast::error::RecvError;
    for m in face.frames() { bus.fire(m); }
    let fps = face.state.timeline.fps().unwrap_or(25).max(1);
    let mut clock = tokio::time::interval(Duration::from_nanos(1_000_000_000 / fps as u64)); clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip); // STRIAINFOREELHOVERDELAY (SR789): a clock idle under its guard skips the missed ticks (no catch-up burst)
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Ok(m) => {
                    if face.on(&m) {
                        for m in face.frames() { bus.fire(m); }
                    }
                    if let Some(e) = face.last_error.take() {
                        log::warn!("[nle-face] {e}");
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
            _ = clock.tick(), if face.playing || face.exporting() || face.hover_pending() => { // STRIAINFOREELHOVERDELAY (SR789): and a counting hover dwell; NLEFACEEXPORT (SR694): the clock also paints a running export's band
                if face.tick() | face.export_tick() {
                    for m in face.frames() { bus.fire(m); }
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
    ("Ctrl+s", "save"),
    ("Meta+s", "save"),
    ("Left", "nudge-"),
    ("Right", "nudge+"),
    ("\u{f702}", "nudge-"),
    ("\u{f703}", "nudge+"),
    ("[", "["),
    ("]", "]"),
    ("I", "mark in"), // NLE5DRANGEKEYS (SR735): Shift+i / Shift+o / Shift+x (text) bind the editor's export-mark words
    ("O", "mark out"),
    ("X", "mark clear"),
    (",", "slip-"), // NLE3ESLIPFACE (SR737): , / . slip and < / > (Shift+, / Shift+.) slide the selected clip one frame (text keys, like [ / ])
    (".", "slip+"),
    ("<", "slide-"),
    (">", "slide+"),
    ("Ctrl+comma", "slip-10"), // NLE3ESLIPSTEP (SR752): Ctrl / Cmd with , . < > step SLIP_STEP frames (GTK's names: the shifted keyval keeps its Shift+)
    ("Meta+comma", "slip-10"),
    ("Ctrl+period", "slip+10"),
    ("Meta+period", "slip+10"),
    ("Ctrl+Shift+less", "slide-10"),
    ("Meta+Shift+less", "slide-10"),
    ("Ctrl+Shift+greater", "slide+10"),
    ("Meta+Shift+greater", "slide+10"),
    ("\\", "zoom-fit"), ("|", "zoom-sel"), // STRIAZOOMFITKEYS (SR907): `\` / `|` (Shift+\, text) fit the timeline / the selection (Shift+Z is redo)
    ("?", "help"), // NLE5DKEYSHELP (SR744): `?` (Shift+/, text) shows the keys help over the timeline band (file tail)
];
/// The wheel delta one zoom step takes (the AppKit surface sends 3 per notch).
pub const WHEEL_STEP: f64 = 3.0;
/// The fastest shuttle (frames per clock tick).
pub const SHUTTLE_MAX: i64 = 8;

impl Face {
    /// The face's geometry: [`Layout::of`], with the view a wheel zoom anchored kept while the playhead is where it
    /// was anchored or inside that view (else the page holding the playhead).
    pub fn layout(&self) -> Layout {
        let (w, h) = self.canvas;
        let mut l = Layout::of_role(self.pointer_role(), w, h, &self.state);
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
        if let Some(r) = self.press_transition_edge(x, y, mods) { return r; } if let Some(r) = self.pick_transition(x, y) { return r; }
        let l = self.layout();
        let dropped = self.drag.take().is_some();
        let (sel, set, ph) = (self.state.selection, self.state.set.clone(), self.state.playhead);
        let (shift, nosnap) = (mods & bandy::MOD_SHIFT != 0, mods & bandy::MOD_ALT != 0);
        self.drag = Drag::begin_with(&mut self.state, l, x, y, shift, nosnap, mods & (bandy::MOD_CONTROL | bandy::MOD_SUPER) != 0);
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
        if let Some(r) = self.hover_message(x, y) { return r; } // STRIAINFOREELHOVER (SR762): no drag: the pointer moves and the message follows the clip under it (file tail)
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
        if self.help { self.help = false; return true; } if act == "help" { return self.verb(act); } // NLE5DKEYSHELP (SR744): a taken key while the help shows only dismisses it (not applied); `?` shows it through the one verb
        if VERBS.contains(&act) || slipslide_word(act).is_some() { // NLE3ESLIPSTEP (SR752): the 10-frame words are keys, not VERBS: the same road
            return self.verb(act);
        }
        let r = match act {
            "shuttle+" => {
                self.rate = if self.playing && self.rate > 0 { (self.rate * 2).min(SHUTTLE_MAX) } else { 1 };
                self.playing = true; self.sync_audio(); // NLEPLAYAUDIO (SR676)
                Ok(())
            }
            "shuttle-" => {
                self.rate = if self.playing && self.rate < 0 { (self.rate * 2).max(-SHUTTLE_MAX) } else { -1 };
                self.playing = true; self.sync_audio(); // NLEPLAYAUDIO (SR676)
                Ok(())
            }
            "stop" => {
                self.playing = false;
                self.rate = 1; self.sync_audio(); // NLEPLAYAUDIO (SR676): the device follows the transport
                Ok(())
            }
            "mark-in" | "mark-out" | "home" | "end" | "nudge-" | "nudge+" | "mark in" | "mark out" | "mark clear" => self.state.verb(act).unwrap_or(Err(EditError::NoClip)), // NLE6KEYS: the ONE dispatch; NLE5DRANGEKEYS (SR735): the export marks
            "cancel" => return self.drag.take().is_some() || self.cancel_export(), // NLEFACEEXPORT (SR694): a live drag first, else the running export
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

// NLE6DRAGSNAP (rmbp-ledger B977): the snap guide and the marquee paint (`paint_drag`, `GUIDE`, `MARQUEE`) moved to timeline_core::view.


// ---------------------------------------------------------------------------------------------------------------------
// THE TRANSITION WIDGET (NLE3d, LEDGER SR302)
// ---------------------------------------------------------------------------------------------------------------------

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
            _ if matches!(v.split_ascii_whitespace().next(), Some("kind" | "ease")) => self.look_verb(v), // NLETRANSITIONFACE (SR691): file tail
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
        let mut c = Canvas { w: l.w, h: l.h, order: vo(self.order), px };
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
        assert!(face.on(&SMessage::BrowserClick(x, y, 0)));
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
        let SMessage::SurfaceBlit { url, width, height, pixels, .. } = face.frame() else { panic!("not a blit") };
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
        SMessage::BrowserButton { down, x, y, mods, surface: 0 }
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
        assert!(face.on(&SMessage::BrowserMouseMove(105.0, 100.0, 0)));
        assert_eq!(face.state.timeline, tl0, "nothing commits per move");
        let want = tl0.trim_in(0, 1, 3).unwrap();
        assert_eq!(face.dragging().and_then(|d| d.shown(&face.state)).map(|s| s.timeline), Some(want.clone()));
        assert!(face.on(&btn(false, 105.0, 100.0)));
        assert!(!face.on(&SMessage::BrowserClick(105.0, 100.0, 0)));
        assert_eq!(face.state.timeline, want);
        assert_eq!((face.state.undo_depth(), face.state.playhead, face.state.selection), (1, 0, Some((0, 1))));
        assert!(face.dragging().is_none());
        // Its out handle: B is [3,10)@13..20, x_of(20) = 160, the handle 156..160; drag back 2 ticks.
        face.on(&btn(true, 158.0, 100.0));
        assert_eq!(face.dragging().map(|d| d.kind), Some(DragKind::TrimOut));
        face.on(&SMessage::BrowserMouseMove(142.0, 100.0, 0));
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
        face.on(&SMessage::BrowserMouseMove(x + 32.0, y + 40.0, 0));
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
        face.on(&SMessage::BrowserMouseMove(x + 40.0, y, 0));
        assert!(face.on(&SMessage::BrowserKey("Escape".into())));
        assert!(face.dragging().is_none());
        assert_eq!((face.state.timeline.clone(), face.state.undo_depth()), (tl0, 0));
        let (x, y) = face.layout().point(0, 5);
        face.on(&btn(true, x, y));
        assert!(face.on(&btn(false, x, y)));
        assert_eq!((face.state.selection, face.state.playhead, face.state.undo_depth()), (Some((0, 0)), 5, 0));
        // Once the surface speaks BrowserButton its BrowserClick is not taken (the release was the click).
        let (x, y) = face.layout().point(1, 12);
        assert!(!face.on(&SMessage::BrowserClick(x, y, 0)));
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
        assert!(face.tick() && face.tick()); // SR676: the first played tick shows the playhead's own range, the next steps by it
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
        assert!(!face.on(&SMessage::BrowserMouseMove(160.0, 100.0, 0)));
        assert!(!face.on(&SMessage::BrowserScroll(0.0, -1.5, 0)), "half a notch is no step");
        assert!(face.on(&SMessage::BrowserScroll(0.0, -1.5, 0)));
        assert_eq!(face.state.zoom, 16);
        let l = face.layout();
        assert_eq!((l.t0, l.tick_at(160.0)), (10, 20));
        assert!(face.on(&SMessage::BrowserScroll(0.0, 3.0, 0)));
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
        face.on(&SMessage::BrowserMouseMove(x + 48.0, y, 0));
        assert_eq!(face.dragging().map(|d| (d.kind, d.delta, d.snap)), Some((DragKind::Move, 6, false)));
        face.on(&btn(false, x + 48.0, y));
        assert_eq!(face.last_error.take().as_deref(), Some("drag: clips overlap"));
        assert_eq!((&face.state.timeline, face.state.undo_depth()), (&tl0, 0));
        assert_eq!((face.state.selection, face.state.set.clone()), (Some((0, 1)), vec![(0, 0), (0, 1)]));
        face.on(&btn_mods(true, x, y, bandy::MOD_ALT));
        face.on(&SMessage::BrowserMouseMove(x + 40.0, y, 0));
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
        assert!(face.on(&SMessage::BrowserMouseMove(300.0, 130.0, 0)));
        face.on(&btn(false, 300.0, 130.0));
        assert_eq!((face.state.selection, face.state.set.clone()), (Some((0, 2)), vec![(0, 2), (1, 2)]));
        face.on(&btn_mods(true, 172.0, 125.0, bandy::MOD_SHIFT));
        face.on(&SMessage::BrowserMouseMove(40.0, 150.0, 0));
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
        face.on(&SMessage::BrowserMouseMove(x - 35.0, y, 0));
        assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((-5, Some(20))));
        let px = face.paint();
        assert_eq!(&px[(150 * 320 + 160) * 4..(150 * 320 + 160) * 4 + 4], GUIDE, "the guide line");
        face.on(&btn(false, x - 35.0, y));
        assert_eq!(face.state.timeline, tl0.move_clip(0, 2, 0, 20).unwrap());
        face.verb("undo");
        // To the playhead at 30 (x 240): 34 px right puts the in edge 6 px from it (raw +4).
        face.state.seek(30);
        face.on(&btn(true, x, y));
        face.on(&SMessage::BrowserMouseMove(x + 34.0, y, 0));
        assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((5, Some(30))));
        key(&mut face, "Escape");
        key(&mut face, "Alt_L");
        face.on(&btn(true, x, y));
        face.on(&SMessage::BrowserMouseMove(x - 35.0, y, 0));
        assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((-5, Some(20))), "NLE2-MODS: a bare Alt keydown arms nothing");
        key(&mut face, "Escape");
        face.on(&btn_mods(true, x, y, bandy::MOD_ALT | bandy::MOD_CONTROL | bandy::MOD_SUPER));
        face.on(&SMessage::BrowserMouseMove(x - 35.0, y, 0));
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
        assert!(face.on(&SMessage::BrowserMouseMove(77.0, 70.0, 0)));
        assert_eq!((face.state.playhead, face.dragging().and_then(|d| d.guide)), (10, Some(10)), "snapped to tick 10");
        face.on(&SMessage::BrowserMouseMove(124.0, 70.0, 0));
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
    let mut c = Canvas { w: l.w, h: l.h, order: vo(order), px };
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

// ---------------------------------------------------------------------------------------------------------------------
// SURFACEID (LEDGER SR332): the face's pointer from its own Surface only
// ---------------------------------------------------------------------------------------------------------------------

impl Face {
    /// Route this face to Surface number `n` of its tetra tree (quartzite's `tetra::SurfaceMap::number`): a pointer
    /// message from any other Surface is not taken.
    pub fn on_surface(mut self, n: u32) -> Face {
        self.surface = Some(n);
        self
    }
    /// Whether [`Face::on`] reads `msg`: every message when the face has no surface, else every non-pointer message
    /// and the pointer messages (bandy's `SMessage::surface`) of its own Surface.
    pub fn takes(&self, msg: &SMessage) -> bool {
        match (self.surface, msg.surface()) {
            (Some(want), Some(got)) => want == got || self.split.is_some_and(|s| s.preview == got && matches!(msg, SMessage::BrowserResize(..))),
            _ => true,
        }
    }
}

#[cfg(test)]
mod surfaceid_tests {
    use super::*;

    fn face() -> Face {
        let (tl, src) = synth_fixture();
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
    }

    fn on(n: u32, m: SMessage) -> SMessage {
        let mut m = m;
        *m.surface_mut().expect("a pointer message") = n;
        m
    }

    /// SURFACEID M3: a face on Surface 1 drops press / move / release / wheel / click from Surface 0 (nothing changes),
    /// and takes them from Surface 1 with the result an unrouted face has; verbs and keys pass either way.
    #[test]
    fn surfaceid_face_routes_by_surface() {
        // Select B, drag its in handle (x 80..84) right 3 ticks, then a wheel notch up (the NLE2B witness's gesture).
        let gesture = |n: u32| {
            vec![
                on(n, SMessage::BrowserButton { down: true, x: 81.0, y: 100.0, mods: 0, surface: 0 }),
                on(n, SMessage::BrowserMouseMove(105.0, 100.0, 0)),
                on(n, SMessage::BrowserButton { down: false, x: 105.0, y: 100.0, mods: 0, surface: 0 }),
                on(n, SMessage::BrowserClick(105.0, 100.0, 0)),
                on(n, SMessage::BrowserScroll(0.0, -WHEEL_STEP, 0)),
            ]
        };
        let mut plain = face();
        plain.state.select(0, 1).unwrap();
        for m in gesture(0) {
            plain.on(&m);
        }
        let mut routed = face().on_surface(1);
        routed.state.select(0, 1).unwrap();
        let (tl0, zoom0) = (routed.state.timeline.clone(), routed.state.zoom);
        for m in gesture(0) {
            assert!(!routed.takes(&m) && !routed.on(&m), "{m:?}");
        }
        assert_eq!((&routed.state.timeline, routed.state.zoom, routed.state.undo_depth()), (&tl0, zoom0, 0));
        assert!(routed.dragging().is_none() && routed.pointer().is_none());
        for m in gesture(1) {
            assert!(routed.takes(&m), "{m:?}");
            routed.on(&m);
        }
        assert_eq!(routed.state.timeline, plain.state.timeline);
        assert_eq!((routed.state.zoom, routed.state.undo_depth()), (plain.state.zoom, 1));
        assert_eq!(routed.state.timeline, tl0.trim_in(0, 1, 3).unwrap());
        // Not pointer messages: taken whatever the surface; a resize (NLE2-SURFACERESIZE SR339) only from its own Surface.
        assert!(routed.takes(&SMessage::BrowserKey("Home".into())) && routed.takes(&SMessage::BrowserResize(320, 160, 1)));
        assert!(!routed.takes(&SMessage::BrowserResize(320, 160, 0)));
        assert!(routed.on(&SMessage::Input { target: SURFACE.into(), text: "undo".into() }));
        assert_eq!(routed.state.timeline, tl0);
        // An unrouted face takes every surface's pointer.
        assert!(face().takes(&on(7, SMessage::BrowserMouseMove(1.0, 1.0, 0))));
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// NLE2-SPLIT (LEDGER SR339): the preview on its own Surface, the ruler and tracks on another, ONE geometry and ONE painter
// ---------------------------------------------------------------------------------------------------------------------

/// The split editor's preview Surface (the preview frame and the master meter; the transport row sits under it).
pub const PREVIEW_SURFACE: &str = "stria.nle.preview";
/// The split editor's tracks Surface (the ruler, the tracks, the transition widgets and the track meters).
pub const TRACKS_SURFACE: &str = "stria.nle.tracks";

/// A split face's Surfaces: the preview's number and canvas, the tracks' number (its canvas is [`Face::canvas`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Split {
    pub preview: u32,
    pub tracks: u32,
    pub preview_canvas: (u32, u32),
}

impl Face {
    /// Split this face over two Surfaces of its tetra tree: `preview` ([`PREVIEW_SURFACE`]) and `tracks`
    /// ([`TRACKS_SURFACE`]). The canvas given at [`Face::new`] is cut where [`Layout::of`] puts the ruler: the preview
    /// takes the band above it, the tracks the rest. The pointer is the tracks Surface's ([`Face::on_surface`]).
    pub fn on_surfaces(mut self, preview: u32, tracks: u32) -> Face {
        let (w, h) = self.canvas;
        let ph = Layout::of(w, h, &self.state).preview_h;
        self.split = Some(Split { preview, tracks, preview_canvas: (w, ph.max(1)) });
        self.canvas = (w, h.saturating_sub(ph).max(1));
        self.surface = Some(tracks);
        self
    }
    /// The split, when the face has one.
    pub fn split(&self) -> Option<Split> {
        self.split
    }
    /// The role of the Surface the pointer (and [`Face::canvas`]) belongs to.
    fn pointer_role(&self) -> Role {
        if self.split.is_some() { Role::Tracks } else { Role::Whole }
    }
    /// The geometry of the Surface showing `role`: the preview's on its own canvas, anything else [`Face::layout`].
    pub fn layout_for(&self, role: Role) -> Layout {
        if role != Role::Preview {
            return self.layout();
        }
        let (w, h) = self.split.map(|s| s.preview_canvas).unwrap_or(self.canvas);
        Layout::of_role(Role::Preview, w, h, &self.state)
    }
    /// NLE2-SURFACERESIZE (SR339): a resize of the split preview Surface sizes the preview's canvas only (`Some(changed)`);
    /// `None` for every other message (the tracks Surface's resize is [`Face::on`]'s, on [`Face::canvas`]).
    fn on_preview_resize(&mut self, msg: &SMessage) -> Option<bool> {
        let s = self.split?;
        let SMessage::BrowserResize(w, h, n) = *msg else { return None };
        if n != s.preview || n == s.tracks {
            return None;
        }
        let changed = w > 0 && h > 0 && (w, h) != s.preview_canvas;
        if changed {
            self.split = Some(Split { preview_canvas: (w, h), ..s });
        }
        Some(changed)
    }
    /// The pixels of the Surface showing `role` (the preview at the playhead rendered first, except for the tracks
    /// Surface, which shows none). While a drag is live the drag's preview state ([`Drag::shown`]) is painted. `Whole` is
    /// the one-Surface paint; `Preview` carries no drag guide, marquee or transition thumbnail (their coordinates are the
    /// tracks Surface's).
    pub fn paint_role(&mut self, role: Role) -> Vec<u8> {
        let l = self.layout_for(role).with_message(self.labels.is_some()); // STRIAINFOREELSTRIP (SR747): the message row of its own (timeline_core view.rs tail)
        let shown = self.drag.as_ref().and_then(|d| d.shown(&self.state));
        let st = shown.as_ref().unwrap_or(&self.state);
        if role != Role::Tracks && !self.play.take_still(&mut self.still, st.playhead, shown.is_none()) { // NLEPLAYAUDIO (SR676): a played tick's paint shows its own render
            self.still = match preview(&st.timeline, &self.sources, st.playhead) {
                Ok(s) => s,
                Err(e) => {
                    self.last_error = Some(format!("preview: {e}"));
                    None
                }
            };
        }
        let still = if role == Role::Tracks { None } else { self.still.as_ref() };
        let px = self.paint_message(paint_in(st, l, self.order, self.labels.as_ref(), still), l, role); // STRIAINFOREEL (SR743): the message line at the preview band's foot (file tail)
        if role == Role::Preview {
            return paint_meters(px, l, self.order, &self.meter_levels());
        }
        { let px = paint_marks(paint_export(paint_meters(view::paint_drag(self.paint_transition_still(px, l), l, vo(self.order), self.drag.as_ref()), l, self.order, &self.meter_levels()), l, self.order, self.export_frac()), l, self.order, self.marks()); self.paint_help(px, l) } // NLEFACEEXPORT (SR694): the progress band over the ruler's foot; NLE5DEXPORTRANGE (SR717): the in / out marks over the ruler; NLE5DKEYSHELP (SR744): the keys help over the timeline band, last (file tail)
    }
    /// Every Surface's blit: [`Face::frame`] alone on one Surface; split, the preview's then the tracks'.
    pub fn frames(&mut self) -> Vec<SMessage> {
        let Some(s) = self.split else { return vec![self.frame()] };
        let preview = self.paint_role(Role::Preview);
        let tracks = self.paint_role(Role::Tracks);
        let (pw, ph) = s.preview_canvas;
        let (tw, th) = self.canvas;
        vec![
            SMessage::SurfaceBlit { url: PREVIEW_SURFACE.into(), width: pw, height: ph, pixels: preview, damage: None, scroll: None },
            SMessage::SurfaceBlit { url: TRACKS_SURFACE.into(), width: tw, height: th, pixels: tracks, damage: None, scroll: None },
        ]
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;

    fn face() -> Face {
        let (tl, src) = synth_fixture();
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
    }

    fn blit(m: &SMessage) -> (&str, u32, u32, &[u8]) {
        let SMessage::SurfaceBlit { url, width, height, pixels, .. } = m else { panic!("not a blit: {m:?}") };
        (url.as_str(), *width, *height, pixels.as_slice())
    }

    /// NLE2-SPLIT M3: both Surfaces paint the one state: the preview's pixels are the one-Surface paint's rows above the
    /// ruler, the tracks' its rows from the ruler down; an unsplit face's frames are its one frame.
    #[test]
    fn split_surfaces_paint_the_one_state() {
        let mut whole = face();
        whole.state.select(0, 1).unwrap();
        let all = whole.paint();
        let cut = whole.layout().ruler_y as usize * 320 * 4;
        assert_eq!(cut, 64 * 320 * 4);
        let frames = whole.frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(blit(&frames[0]), (SURFACE, 320, 160, all.as_slice()));
        let mut split = face().on_surfaces(0, 1);
        split.state.select(0, 1).unwrap();
        assert_eq!((split.split().unwrap().preview_canvas, split.canvas), ((320, 64), (320, 96)));
        let frames = split.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(blit(&frames[0]), (PREVIEW_SURFACE, 320, 64, &all[..cut]));
        assert_eq!(blit(&frames[1]), (TRACKS_SURFACE, 320, 96, &all[cut..]));
        assert!(split.still().is_some(), "the preview Surface renders the playhead's frame");
    }

    /// NLE2-SPLIT M3: the NLE2B handle drag at the same coordinates: on the preview Surface it is not taken (nothing
    /// changes), on the tracks Surface it is the one trim.
    #[test]
    fn split_press_in_tracks_trims_not_in_preview() {
        let mut f = face().on_surfaces(0, 1);
        f.state.select(0, 1).unwrap();
        let tl0 = f.state.timeline.clone();
        let gesture = |n: u32| {
            vec![
                SMessage::BrowserButton { down: true, x: 81.0, y: 36.0, mods: 0, surface: n },
                SMessage::BrowserMouseMove(105.0, 36.0, n),
                SMessage::BrowserButton { down: false, x: 105.0, y: 36.0, mods: 0, surface: n },
                SMessage::BrowserClick(105.0, 36.0, n),
            ]
        };
        for m in gesture(0) {
            assert!(!f.takes(&m) && !f.on(&m), "{m:?}");
        }
        assert_eq!((&f.state.timeline, f.state.undo_depth()), (&tl0, 0));
        assert!(f.dragging().is_none() && f.pointer().is_none());
        for m in gesture(1) {
            assert!(f.takes(&m), "{m:?}");
            f.on(&m);
        }
        assert_eq!(f.state.timeline, tl0.trim_in(0, 1, 3).unwrap());
        assert_eq!(f.state.undo_depth(), 1);
    }

    /// NLE2-SURFACERESIZE M3: a resize of one Surface changes only that Surface's canvas and rects; another Surface's
    /// number is not taken.
    #[test]
    fn split_resize_changes_only_its_surface() {
        let mut f = face().on_surfaces(0, 1);
        let tracks = f.layout();
        assert!(f.on(&SMessage::BrowserResize(400, 90, 0)));
        assert_eq!((f.split().unwrap().preview_canvas, f.canvas, f.layout()), ((400, 90), (320, 96), tracks));
        assert_eq!(f.layout_for(Role::Preview), Layout::of_role(Role::Preview, 400, 90, &f.state));
        assert!(!f.on(&SMessage::BrowserResize(400, 90, 0)), "the same size: no repaint");
        let preview = f.layout_for(Role::Preview);
        assert!(f.on(&SMessage::BrowserResize(500, 120, 1)));
        assert_eq!((f.split().unwrap().preview_canvas, f.canvas, f.layout_for(Role::Preview)), ((400, 90), (500, 120), preview));
        assert_eq!(f.layout().tracks_y, RULER_H);
        assert!(!f.takes(&SMessage::BrowserResize(1, 1, 7)) && !f.on(&SMessage::BrowserResize(1, 1, 7)));
        let frames = f.frames();
        assert_eq!((blit(&frames[0]).1, blit(&frames[0]).2, blit(&frames[1]).1, blit(&frames[1]).2), (400, 90, 500, 120));
        // The preview's rect set holds no hit target.
        let l = f.layout_for(Role::Preview);
        assert_eq!(l.hit(&f.state.timeline, 81.0, 36.0), Hit::None);
    }
}

// ---- NLEHOSTSAVE (LEDGER SR677): the host face's save through timeline_core's ONE writer (named tail block) ----

/// NLEHOSTSAVE (SR677): the document path, the derived dirty reading and the save. The EDL text is
/// `timeline_core::edl::write`'s (the kernel face's `edit_save` writer), titled by the file stem as the kernel's
/// `edl_stem` titles it; every refusal names its cause (no path) or the path.
impl Face {
    /// The face opened from `path` (`None`: no document, `save` refuses).
    pub fn with_doc(mut self, path: Option<&std::path::Path>) -> Face {
        self.doc_path = path.map(std::path::Path::to_path_buf);
        self
    }
    /// The path the document was opened from.
    pub fn doc_path(&self) -> Option<&std::path::Path> {
        self.doc_path.as_deref()
    }
    /// True when the timeline differs from the one last opened or saved (an undo back to it reads clean).
    pub fn dirty(&self) -> bool {
        self.state.timeline != self.clean
    }
    /// The `save` verb: a refusal is the face's `last_error` (`save: <why>`), like every refused verb.
    fn save_verb(&mut self, v: &str) -> Option<bool> {
        if v != "save" {
            return None;
        }
        if let Err(e) = self.save() {
            self.last_error = Some(format!("save: {e}"));
        }
        Some(true)
    }
    /// Write the timeline to the document's path; the bytes written. The face is clean after.
    pub fn save(&mut self) -> Result<usize, String> {
        let Some(path) = self.doc_path.clone() else { return Err("no document path".into()) };
        let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("untitled");
        let text = timeline_core::edl::write(&self.state.timeline, title).map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::write(&path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
        self.clean = self.state.timeline.clone();
        Ok(text.len())
    }
}

// ---- NLEPLAYAUDIO (LEDGER SR676): the face plays sound; a played tick renders once (named tail block) ----

/// NLEPLAYAUDIO (SR676): the face's play path — the device (a `media::AudioOut`, `None` muted / headless / in tests), whether
/// it is released (`live`: playing at rate 1), the playhead the last played render was shown at (`shown`) and where that
/// render ended (`next`: the next played tick steps the playhead there first; a range that does not continue it flushes the
/// ring and resets the meters, as `MixMeters::feed` resets them), and the still a played tick rendered, tagged by the
/// playhead its paint shows (`pending`).
#[derive(Default)]
struct PlayAudio {
    out: Option<Box<dyn crate::media::AudioOut>>,
    live: bool,
    next: Option<i64>,
    shown: Option<i64>,
    pending: Option<(i64, Option<Still>)>,
    renders: u64,
    pushed: u64,
    run_base: u64,
    run_pushed: u64,
    rate: u32,
}

impl PlayAudio {
    /// The played tick's still into `into` when it was rendered for `playhead` and no drag preview is painted (`live`); true
    /// when taken (the paint then renders no `preview`).
    fn take_still(&mut self, into: &mut Option<Still>, playhead: i64, live: bool) -> bool {
        match self.pending.take() {
            Some((at, s)) if live && at == playhead => {
                *into = s;
                true
            }
            _ => false,
        }
    }
}

/// What the play path has done: renders through [`PlaySink`], PCM frames handed to the device, whether it is released.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayStats {
    pub renders: u64,
    pub pushed: u64,
    pub live: bool,
}

/// NLEPLAYAUDIO (SR676): the ONE render of a played tick: the last frame is the face's still (what `preview` gave), the mix
/// and each track's share feed the meters through `MixMeters`' own `RenderSink` (the pushes `MixMeters::feed` makes), and
/// the mix goes to the device as stereo (`media::to_stereo`) when one is released.
struct PlaySink<'a> {
    meters: &'a mut MixMeters,
    out: Option<&'a mut Box<dyn crate::media::AudioOut>>,
    still: Option<Still>,
    pushed: u64,
}

impl RenderSink for PlaySink<'_> {
    fn frame(&mut self, _out_t: i64, width: u32, height: u32, rgba: &[u8]) {
        self.still = Some(Still { width, height, rgba: rgba.to_vec() });
    }
    fn pcm(&mut self, out_t: i64, rate: u32, channels: u16, interleaved: &[f32]) {
        RenderSink::pcm(&mut *self.meters, out_t, rate, channels, interleaved);
        if let Some(o) = self.out.as_mut() {
            let took = o.push(&crate::media::to_stereo(interleaved, channels as usize));
            self.pushed += (took / 2) as u64;
        }
    }
    fn taps_tracks(&self) -> bool {
        true
    }
    fn track_pcm(&mut self, out_t: i64, track: usize, rate: u32, channels: u16, interleaved: &[f32]) {
        RenderSink::track_pcm(&mut *self.meters, out_t, track, rate, channels, interleaved);
    }
}

impl Face {
    /// Play through `out` (a `media::resonance_out` feed at `rate` Hz): the render's audio rate is set to the device's
    /// (`Sources::set_audio_rate`), the device starts paused and is released while playing at rate 1.
    pub fn with_audio(mut self, mut out: Box<dyn crate::media::AudioOut>, rate: u32) -> Face {
        out.set_paused(true);
        std::sync::Arc::get_mut(&mut self.sources).expect("with_audio is a builder: no export shares the sources yet").set_audio_rate(rate);
        self.play.rate = rate;
        self.play.out = Some(out);
        self.play.live = false;
        self.sync_audio();
        self
    }
    /// What the play path has done.
    pub fn played(&self) -> PlayStats {
        PlayStats { renders: self.play.renders, pushed: self.play.pushed, live: self.play.live }
    }
    /// The device follows the transport: released while playing at rate 1; otherwise paused and its ring flushed (a
    /// shuttle and reverse play are silent: NLE-SHUTTLE-AUDIO owed). A run's end logs what the device consumed, once.
    fn sync_audio(&mut self) {
        let want = self.playing && self.rate == 1;
        if want == self.play.live {
            return;
        }
        self.play.live = want;
        let rate = self.play.rate;
        let Some(o) = self.play.out.as_mut() else { return };
        if want {
            o.set_paused(false);
            self.play.run_base = o.consumed();
            self.play.run_pushed = self.play.pushed;
        } else {
            o.set_paused(true);
            o.flush();
            let frames = o.consumed().saturating_sub(self.play.run_base);
            if frames > 0 || self.play.pushed > self.play.run_pushed {
                log::info!("[STRIA] :: nle play audio rate={rate} frames={frames}");
            }
        }
    }
    /// One played forward tick at playhead `ph`: when the last played render was shown at `ph`, the playhead first steps to
    /// where that render ended; then ONE render of `playhead..playhead + rate` (clamped to the end) through [`PlaySink`], shown
    /// at that playhead (the frame, the line and the audio pushed are the same tick). The end stops play. False for a reverse
    /// rate (the old path: `feed_meters` resets, the nudge, the paint renders `preview`). An error is `last_error` and resets
    /// the meters; the paint then renders `preview`, as before, and the next tick still steps on.
    fn play_tick(&mut self, ph: i64) -> bool {
        if self.rate <= 0 {
            return false;
        }
        self.play.pending = None;
        if let (Some(at), Some(n)) = (self.play.shown, self.play.next) {
            if at == ph {
                self.state.seek(n);
            }
        }
        let ph = self.state.playhead;
        let end = (ph + self.rate).min(self.state.timeline.end());
        self.play.shown = None;
        if end <= ph {
            self.playing = false;
            self.sync_audio();
            return true;
        }
        if self.play.next != Some(ph) {
            self.meters.reset();
            if let Some(o) = self.play.out.as_mut() {
                o.flush();
            }
        }
        let out = if self.play.live && self.rate == 1 { self.play.out.as_mut() } else { None };
        let mut sink = PlaySink { meters: &mut self.meters, out, still: None, pushed: 0 };
        let r = render(&self.state.timeline, ph..end, &self.sources, &mut sink);
        let (still, pushed) = (sink.still, sink.pushed);
        self.play.renders += 1;
        self.play.pushed += pushed;
        match r {
            Ok(_) => self.play.pending = Some((ph, still)),
            Err(e) => {
                self.last_error = Some(format!("meters: {e}"));
                self.meters.reset();
            }
        }
        (self.play.shown, self.play.next) = (Some(ph), Some(end));
        true
    }
}

#[cfg(test)]
mod playaudio_tests {
    use super::*;
    use crate::media::AudioOut;
    use crate::nle::{SYNTH_RATE, synth_sample};
    use gneiss_pal::dsp::video::TestPattern;
    use std::sync::{Arc, Mutex};

    /// What the recording device saw, in order.
    #[derive(Debug, Clone, PartialEq)]
    enum Ev {
        Push(Vec<f32>),
        Paused(bool),
        Flush,
    }

    /// The tests' `AudioOut`: records every call; takes every sample; `consumed` is what it took.
    struct RecordingOut(Arc<Mutex<Vec<Ev>>>);

    impl AudioOut for RecordingOut {
        fn push(&mut self, stereo: &[f32]) -> usize {
            self.0.lock().unwrap().push(Ev::Push(stereo.to_vec()));
            stereo.len()
        }
        fn consumed(&self) -> u64 {
            self.0.lock().unwrap().iter().map(|e| if let Ev::Push(s) = e { s.len() as u64 / 2 } else { 0 }).sum()
        }
        fn flush(&mut self) {
            self.0.lock().unwrap().push(Ev::Flush);
        }
        fn set_paused(&mut self, paused: bool) {
            self.0.lock().unwrap().push(Ev::Paused(paused));
        }
    }

    fn face() -> (Face, Arc<Mutex<Vec<Ev>>>) {
        let (tl, src) = synth_fixture();
        let log = Arc::new(Mutex::new(Vec::new()));
        let f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_audio(Box::new(RecordingOut(log.clone())), SYNTH_RATE);
        (f, log)
    }

    fn pushes(log: &Arc<Mutex<Vec<Ev>>>) -> Vec<Vec<f32>> {
        log.lock().unwrap().iter().filter_map(|e| if let Ev::Push(s) = e { Some(s.clone()) } else { None }).collect()
    }

    fn still_at(t: i64) -> Option<Still> {
        let (tl, src) = synth_fixture();
        preview(&tl, &src, t).unwrap()
    }

    /// NLEPLAYAUDIO M3: a played tick is ONE render; its paint takes the played still (no `preview` render), which is the
    /// frame at the playhead the line shows: the first tick at 0, the next steps to 1.
    #[test]
    fn nleplayaudio_played_tick_renders_once() {
        let (mut f, _) = face();
        f.verb("play");
        assert!(f.tick());
        assert_eq!((f.state.playhead, f.played().renders), (0, 1), "the first played tick is shown at its own playhead");
        assert!(f.play.pending.is_some());
        let _ = f.frames();
        assert!(f.play.pending.is_none(), "the paint took the played still");
        assert_eq!(f.played().renders, 1);
        assert_eq!(f.still().cloned(), still_at(0));
        assert!(f.tick());
        let _ = f.frames();
        assert_eq!((f.state.playhead, f.played().renders), (1, 2));
        assert_eq!(f.still().cloned(), still_at(1));
        assert_eq!(TestPattern::read_counter(&f.still().unwrap().rgba, 64, 48), Some(14), "reel A, source tick 14");
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
    }

    /// NLEPLAYAUDIO M3: each played tick hands the device rate / fps frames (48000 / 25 = 1920), stereo, L = R = the mono
    /// mix (reel A from source tick 13 at unity gain: the source's own samples).
    #[test]
    fn nleplayaudio_pcm_per_tick() {
        let (mut f, log) = face();
        f.verb("play");
        f.tick();
        f.tick();
        let p = pushes(&log);
        assert_eq!(p.len(), 2);
        for (k, s) in p.iter().enumerate() {
            assert!((s.len() as i64 / 2 - (SYNTH_RATE / 25) as i64).abs() <= 1, "{}", s.len());
            for (i, fr) in s.chunks_exact(2).enumerate() {
                assert_eq!(fr[0], fr[1]);
                assert_eq!(fr[0], synth_sample(0, (13 + k as u64) * 1920 + i as u64) as f32 / 32768.0, "tick {k} frame {i}");
            }
        }
        assert_eq!(f.played().pushed, 3840);
    }

    /// NLEPLAYAUDIO M3: the meters after two played ticks are `MixMeters::feed` of the same two ticks, bit for bit.
    #[test]
    fn nleplayaudio_meters_bit_for_bit() {
        let (mut f, _) = face();
        f.verb("play");
        f.tick();
        f.tick();
        let (tl, src) = synth_fixture();
        let mut m = MixMeters::default();
        m.feed(&tl, 0..1, &src).unwrap();
        m.feed(&tl, 1..2, &src).unwrap();
        assert_eq!(f.meters().levels(), m.levels());
        assert_eq!(f.meters().levels().len(), 2);
    }

    /// NLEPLAYAUDIO M3: the device starts paused, play releases it, stop pauses it then flushes; a shuttle past rate 1 is
    /// silent (paused, flushed, nothing pushed on its tick); a face with no device plays and pushes nothing.
    #[test]
    fn nleplayaudio_pause_flushes() {
        let (mut f, log) = face();
        assert_eq!(*log.lock().unwrap(), vec![Ev::Paused(true)]);
        f.verb("play");
        assert_eq!(log.lock().unwrap().last(), Some(&Ev::Paused(false)));
        assert!(f.played().live);
        f.tick();
        log.lock().unwrap().clear();
        f.verb("play");
        assert_eq!(*log.lock().unwrap(), vec![Ev::Paused(true), Ev::Flush]);
        assert!(!f.played().live);
        log.lock().unwrap().clear();
        f.on(&SMessage::BrowserText("l".into()));
        f.on(&SMessage::BrowserText("l".into()));
        assert_eq!((f.playing, f.rate), (true, 2));
        assert!(f.tick());
        assert_eq!(*log.lock().unwrap(), vec![Ev::Paused(false), Ev::Paused(true), Ev::Flush], "the shuttle's tick pushes nothing");
        assert_eq!((f.state.playhead, f.played().renders, f.played().pushed), (1, 2, 1920));

        let (tl, src) = synth_fixture();
        let mut mute = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        mute.verb("play");
        assert!(mute.tick());
        assert_eq!(mute.played(), PlayStats { renders: 1, pushed: 0, live: true });
    }

    /// NLEPLAYAUDIO M3: stopped, the line is the frame shown and the repaint is `preview`'s; a seek while playing repaints
    /// with `preview` at the new tick, and the next played tick renders there (the meters and the ring restart).
    #[test]
    fn nleplayaudio_stopped_repaint_is_preview() {
        let (mut f, log) = face();
        f.verb("play");
        f.tick();
        f.tick();
        f.verb("play");
        let _ = f.frames();
        assert_eq!((f.state.playhead, f.still().cloned()), (1, still_at(1)));
        f.verb("play");
        f.tick();
        assert_eq!(f.state.playhead, 2, "a resumed run steps past the tick it showed");
        f.state.seek(30);
        let _ = f.frames();
        assert_eq!(f.still().cloned(), still_at(30));
        log.lock().unwrap().clear();
        f.tick();
        assert_eq!(f.state.playhead, 30);
        assert_eq!(log.lock().unwrap().first(), Some(&Ev::Flush), "a range that does not continue flushes the ring");
        let _ = f.frames();
        assert_eq!(TestPattern::read_counter(&f.still().unwrap().rgba, 64, 48), Some(28));
    }
}

// ---- NLEFACEEXPORT (LEDGER SR694): the export verb, its job off the face's thread, the progress band (named tail block) ----

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use crate::nle_export::{AudioOut, Container, ExportError, ExportReport, VIDEO_NAMES, export_as_with, video_named};

/// NLE5d (SR694): the export band's height (px), painted over the ruler's foot while an export runs.
pub const EXPORT_BAND_H: u32 = 2;

/// NLE5d (SR694): one export running off the face's thread: `nle_export::export_as_with` (the one door) over the face's
/// timeline (cloned at the verb) and its sources (the `Arc` the preview reads too). The thread stores the reading in
/// `done` / `total`; the face stores `cancel`, which the progress closure reads and refuses with the existing
/// `ExportError::Io(Interrupted, "cancelled")`. The result is the thread's return, joined once it has finished.
pub struct ExportJob {
    dir: std::path::PathBuf,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<Result<ExportReport, ExportError>>>,
    /// The fnv1a of the export line ([`job_line`]) the last [`Face::export_tick`] saw (a repaint only when it moved: done or the eta word, NLE5DEXPORTMESSAGEETABAND SR803).
    shown: u64,
    /// NLE5DEXPORTRANGE (SR717): the marked range exported (`None`: the whole timeline), named in the result line.
    range: Option<std::ops::Range<i64>>, video: String, audio: AudioOut, started: std::time::Instant, rate_ema: f64, rate_at: (std::time::Instant, u64), // NLE5DEXPORTMESSAGEAUDIO (SR756): the video word and the audio the result line and the witness name
}

impl Face {
    /// The `export` verb: `export [<dir>] [h264|png|i420|y4m|raw] [wav|aac[:KBPS]|mp4|none]` (default h264, wav audio: NLE5DEXPORTAUDIOOPTIONS SR722; a lone format word is the format, a lone
    /// other word the directory; no directory = `<document's dir>/<stem>-export`). Every outcome is the face's one message
    /// line `last_error`: a refusal (`export: busy (d/t)`, `export: no directory (no document path)`, `export: <dir>: <io>`,
    /// `export: unknown format ..`) at once; the result (a report, not an error) when [`Face::export_tick`] sees it finish.
    fn export_verb(&mut self, v: &str) -> Option<bool> {
        let mut words = v.split_whitespace();
        if words.next() != Some("export") {
            return None;
        }
        let all = v.split_whitespace().any(|w| w == "--all"); let args: Vec<&str> = words.filter(|w| *w != "--all").collect(); let (args, audio) = export_audio_word(args);
        if let Some((d, t)) = self.export_progress() {
            self.last_error = Some(format!("export: busy ({d}/{t})"));
            return Some(true);
        }
        let (dir, fmt) = match args.as_slice() {
            [] => (None, "h264"),
            [f] if VIDEO_NAMES.contains(f) => (None, *f),
            [d] => (Some(*d), "h264"),
            [d, f] => (Some(*d), *f),
            _ => {
                self.last_error = Some(format!("export: usage: {}", export_usage())); // NLE5DEXPORTMESSAGEAUDIO (SR756): the one usage
                return Some(true);
            }
        };
        let dir = match (dir, &self.doc_path) {
            (Some(d), _) => std::path::PathBuf::from(d),
            (None, Some(p)) => p.with_file_name(format!("{}-export", p.file_stem().and_then(|s| s.to_str()).unwrap_or("untitled"))),
            (None, None) => {
                self.last_error = Some("export: no directory (no document path)".into());
                return Some(true);
            }
        };
        if let Err(e) = audio.and_then(|a| self.start_export(&dir, fmt, None, if all { None } else { self.export_range() }, a)) {
            self.last_error = Some(format!("export: {e}"));
        }
        Some(true)
    }
    /// Start the job: the directory made here (a bad one refused by name, no thread), the encoder built on the thread
    /// (encoders are not `Send`), `range` (the marked range, NLE5DEXPORTRANGE SR717; `None`: the whole timeline), `audio` (the verb's word, default `AudioOut::Wav`, NLE5DEXPORTAUDIOOPTIONS SR722) into `Container::Mp4` (`nle export`'s defaults).
    /// `pause_at`: the job holds after frame k until cancelled ([`Face::export_paused_at`]).
    fn start_export(&mut self, dir: &std::path::Path, fmt: &str, pause_at: Option<u64>, range: Option<std::ops::Range<i64>>, audio: AudioOut) -> Result<(), String> {
        if let Some((d, t)) = self.export_progress() {
            return Err(format!("busy ({d}/{t})"));
        }
        if !VIDEO_NAMES.contains(&fmt) {
            return Err(format!("unknown format {fmt} ({})", VIDEO_NAMES.join("|")));
        }
        self.range_check(&range)?; std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let span = range.clone().unwrap_or(0..self.state.timeline.end()); let (done, total, cancel) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(crate::nle_export::export_total(&self.state.timeline, &span))), Arc::new(AtomicBool::new(false))); // NLE5DEXPORTMESSAGEETATOTAL (SR820): the range resolved once, the total seeded before the spawn
        let (d, t, c) = (Arc::clone(&done), Arc::clone(&total), Arc::clone(&cancel));
        let video = fmt.to_string(); let (tl, sources, path, fmt, span) = (self.state.timeline.clone(), Arc::clone(&self.sources), dir.to_path_buf(), fmt.to_string(), span);
        let handle = std::thread::Builder::new()
            .name("nle-export".into())
            .spawn(move || {
                let video = video_named(&fmt, tl.tb).expect("a VIDEO_NAMES word");
                let mut progress = |n: u64, of: u64| -> Result<(), ExportError> {
                    d.store(n, Relaxed);
                    t.store(of, Relaxed);
                    if pause_at == Some(n) {
                        while !c.load(Relaxed) {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    }
                    if c.load(Relaxed) {
                        return Err(ExportError::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")));
                    }
                    Ok(())
                };
                export_as_with(&tl, span, &sources, &path, video, audio, Container::Mp4, &mut progress)
            })
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        let started = std::time::Instant::now(); self.export = Some(ExportJob { dir: dir.to_path_buf(), done, total, cancel, handle: Some(handle), shown: 0, range, video, audio, started, rate_ema: 0.0, rate_at: (started, 0) });
        Ok(())
    }
    /// The deterministic seam: start an export of `fmt` into `dir` that holds after frame `k` until it is cancelled (Esc,
    /// [`Face::cancel_export`]). The verb's own job is this with no hold.
    pub fn export_paused_at(&mut self, dir: &std::path::Path, fmt: &str, k: u64) -> Result<(), String> {
        self.start_export(dir, fmt, Some(k), self.export_range(), AudioOut::Wav)
    }
    /// True while an export's result has not been reported.
    pub fn exporting(&self) -> bool {
        self.export.is_some()
    }
    /// The running export's reading `(frames done, frames total)` (total seeded at the start from `nle_export::export_total`).
    pub fn export_progress(&self) -> Option<(u64, u64)> {
        self.export.as_ref().map(|j| (j.done.load(Relaxed), j.total.load(Relaxed)))
    }
    /// What the band shows: the done fraction while an export runs, `None` otherwise.
    fn export_frac(&self) -> Option<f64> {
        self.export_progress().map(|(d, t)| if t == 0 { 0.0 } else { (d as f64 / t as f64).min(1.0) })
    }
    /// NLE5DEXPORTMESSAGEETA (SR796): the running export's remaining time from its rate so far ([`eta_of`] over the
    /// [`Face::export_progress`] pair and the job's `started`, SR756); `None` with no export, before the first frame, or under 1 ms.
    pub fn export_eta(&self) -> Option<Duration> {
        let j = self.export.as_ref()?;
        let (d, t) = self.export_progress()?;
        job_eta(j, d, t)
    }
    /// One clock tick for the export: a finished job's result goes to the message line and the job is cleared; true when
    /// that happened or the reading moved (the band repaints).
    pub fn export_tick(&mut self) -> bool {
        let Some(j) = self.export.as_mut() else { return false };
        if j.handle.as_ref().is_some_and(|h| h.is_finished()) {
            self.finish_export();
            return true;
        }
        eta_sample(j, std::time::Instant::now()); let d = fnv1a(job_line(j).as_bytes());
        let moved = d != j.shown;
        j.shown = d;
        moved
    }
    /// Esc's export arm: ask the running job to stop (its next frame refuses); false when none runs.
    pub fn cancel_export(&mut self) -> bool {
        let Some(j) = self.export.as_ref() else { return false };
        j.cancel.store(true, Relaxed);
        true
    }
    /// Block until the running export ends and report it as [`Face::export_tick`] would; false when none runs.
    pub fn wait_export(&mut self) -> bool {
        if self.export.is_none() {
            return false;
        }
        self.finish_export();
        true
    }
    /// Join the job and put its outcome on the message line: `export: <n> frames, <bytes> bytes, <video> + <audio> in <t> -> <dir>` (`<video>, no audio in <t>`), the cancel
    /// `export: cancelled at d/t frames; partial files left in <dir>`, or the refusal `export: <ExportError>`.
    fn finish_export(&mut self) {
        let Some(mut j) = self.export.take() else { return };
        let (d, t) = (j.done.load(Relaxed), j.total.load(Relaxed));
        let line = match j.handle.take().map(|h| h.join()) {
            Some(Ok(Ok(r))) => format!("export: {} frames{}, {} bytes, {} -> {}", r.frames, ticks_note(&j.range), r.bytes, export_words(&j.video, j.audio, j.started.elapsed(), r.frames, (self.state.timeline.tb.den, self.state.timeline.tb.num)), j.dir.display()),
            Some(Ok(Err(ExportError::Io(e)))) if e.kind() == std::io::ErrorKind::Interrupted && j.cancel.load(Relaxed) => {
                format!("export: cancelled at {d}/{t} frames; partial files left in {}", j.dir.display())
            }
            Some(Ok(Err(e))) => format!("export: {e}"),
            _ => "export: the export thread panicked".to_string(),
        };
        self.last_error = Some(line);
    }
}

/// The export band over a painted canvas: rows `tracks_y - EXPORT_BAND_H .. tracks_y` (the ruler's foot), x `0 .. w * frac`,
/// in timeline_core view's `GUIDE` (the drag guide's token, the editor table's; NLE6VIEW-TOKENS owes palette_core).
/// `None` paints nothing.
fn paint_export(px: Vec<u8>, l: Layout, order: PixelOrder, frac: Option<f64>) -> Vec<u8> {
    let Some(f) = frac else { return px };
    let mut c = Canvas { w: l.w, h: l.h, order: vo(order), px };
    let y1 = l.tracks_y as i64;
    let x1 = (l.w as f64 * f.clamp(0.0, 1.0)).round() as i64;
    c.rect(0, y1 - EXPORT_BAND_H as i64, x1, y1, view::GUIDE);
    c.px
}

// ---- NLE5DEXPORTRANGE (LEDGER SR717): the in / out marks, the range an export takes, their ruler marks (named tail block) ----

impl Face {
    /// The `in` / `out` verbs: set that mark at the playhead; the same verb again at the same tick clears it. A mark that
    /// would not keep in < out is refused by name and the marks are left as they were. `None` for any other word.
    fn mark_verb(&mut self, v: &str) -> Option<bool> {
        let is_in = match v {
            "in" => true,
            "out" => false,
            _ => return None,
        };
        let ph = self.state.playhead;
        let (a, b) = self.state.timeline.marks; let (mine, other) = if is_in { (a, b) } else { (b, a) }; // NLE5DRANGESAVE (SR723): the marks are the timeline's
        let set = if mine == Some(ph) {
            None
        } else {
            match other {
                Some(o) if is_in && ph >= o => {
                    self.last_error = Some(format!("in: tick {ph} is not before the out mark {o}"));
                    return Some(true);
                }
                Some(o) if !is_in && ph <= o => {
                    self.last_error = Some(format!("out: tick {ph} is not after the in mark {o}"));
                    return Some(true);
                }
                _ => Some(ph),
            }
        };
        if let Err(e) = self.state.set_marks(if is_in { (set, other) } else { (other, set) }) { self.last_error = Some(format!("{v}: {e}")); } // NLE5DRANGESAVE (SR723): one commit
        Some(true)
    }
    /// The marks `(in, out)` as set (either may be `None`).
    pub fn marks(&self) -> (Option<i64>, Option<i64>) {
        self.state.timeline.marks // NLE5DRANGESAVE (SR723)
    }
    /// The range an `export` takes: `in..min(out, end)` when both marks are set; `None` (the whole timeline) otherwise.
    /// The range is empty when an edit shortened the timeline to the in mark or before it ([`Face::range_check`] refuses).
    pub fn export_range(&self) -> Option<std::ops::Range<i64>> {
        match self.state.timeline.marks { // NLE5DRANGESAVE (SR723)
            (Some(a), Some(b)) => Some(a..b.min(self.state.timeline.end()).max(a)),
            _ => None,
        }
    }
    /// A marked range holding no tick of the timeline is refused by name (no directory made, no thread).
    fn range_check(&self, range: &Option<std::ops::Range<i64>>) -> Result<(), String> {
        match (range, self.state.timeline.marks.0, self.state.timeline.marks.1) { // NLE5DRANGESAVE (SR723)
            (Some(r), Some(a), Some(b)) if r.is_empty() => Err(format!("the marks {a}..{b} are past the timeline's end {}", self.state.timeline.end())),
            _ => Ok(()),
        }
    }
}

/// The result line's range note: ` ticks <in>..<out>` for a marked export, nothing for the whole timeline.
fn ticks_note(range: &Option<std::ops::Range<i64>>) -> String {
    range.as_ref().map(|r| format!(" ticks {}..{}", r.start, r.end)).unwrap_or_default()
}

/// The in / out marks over a painted canvas: each set mark a 1 px column at its tick's x over the ruler's rows
/// (`ruler_y .. tracks_y`), in timeline_core view's `GUIDE` (the export band's token). No mark set paints nothing.
fn paint_marks(px: Vec<u8>, l: Layout, order: PixelOrder, marks: (Option<i64>, Option<i64>)) -> Vec<u8> {
    if marks == (None, None) {
        return px;
    }
    let mut c = Canvas { w: l.w, h: l.h, order: vo(order), px };
    for t in [marks.0, marks.1].into_iter().flatten() {
        let x = l.x_of(t);
        c.rect(x, l.ruler_y as i64, x + 1, l.tracks_y as i64, view::GUIDE);
    }
    c.px
}

// ---- NLE5DEXPORTAUDIOOPTIONS (LEDGER SR722): the export verb's audio word through the one audio table (named tail block) ----
/// The `export` verb's optional last word (`export [--all] [<dir>] [fmt] [audio]`): of two or three words, a last word the
/// audio table ([`crate::nle_export::audio_named`]) takes is the audio; a third word it does not take is refused by name
/// (`unknown audio <w> (wav|aac|mp4|none)`). Otherwise the words are SR694's / SR717's and the audio is `AudioOut::Wav`
/// (the audio words and `VIDEO_NAMES` are disjoint, so every grammar the verb took before parses the same).
fn export_audio_word(mut args: Vec<&str>) -> (Vec<&str>, Result<AudioOut, String>) {
    let named = args.last().and_then(|w| crate::nle_export::audio_named(w));
    match (args.len(), named) {
        (2 | 3, Some(a)) => {
            args.pop();
            (args, Ok(a))
        }
        (3, None) => {
            let w = args.pop().unwrap_or_default();
            (args, Err(format!("unknown audio {w}; the words are {}", export_usage()))) // NLE5DEXPORTMESSAGEAUDIO (SR756)
        }
        _ => (args, Ok(AudioOut::Wav)),
    }
}

// ---- STRIAINFOREEL (LEDGER SR743): the face's message line — an error, else the selected clip's reel summary (named tail block) ----
// `last_error` stays THE message; its idle text is the selected clip's reel line (`<reel>: <nle::Sources::summary>`). It paints as
// one quiet line over the preview band's foot through the face's own `Labels` hook (the clip labels' style and ink, no fill),
// only when labels are on, so every labels-off golden is byte-identical. Owed: STRIA-INFO-REEL-STRIP (a message row of its own
// in timeline_core::view's Layout), STRIA-INFO-REEL-ELIDE (an ellipsis at the band's width: none is reachable here, the paint
// clips at it), STRIA-INFO-REEL-HOVER (the summary on hover over an unselected clip).

/// STRIAINFOREEL (SR743): the message line's height (px): a band shorter than this shows no message.
pub const MESSAGE_H: u32 = view::MESSAGE_H;

impl Face {
    /// The hovered clip's reel ([`Face::hovered_clip`], STRIAINFOREELHOVER SR762), else the selected clip's: `<reel name>: <summary>` (`nle::Sources::summary`); `None` with no clip
    /// selected or a reel with nothing to say.
    pub fn reel_line(&self) -> Option<String> {
        let c = self.hover_dwelt().and_then(|(t, i)| self.state.timeline.tracks.get(t)?.clips.get(i).copied()).or_else(|| self.state.selected())?;
        let s = self.sources.summary(c.src)?;
        let name = self.state.timeline.source(c.src).map_or_else(|| c.src.0.to_string(), |x| x.reel.clone());
        Some(format!("{name}: {s}"))
    }
    /// The face's message line: the last error while one stands, else [`Face::reel_line`].
    pub fn message_text(&self) -> Option<String> {
        self.last_error.clone().or_else(|| self.export.as_ref().map(job_line)).or_else(|| self.reel_line())
    }
    /// The message over a painted canvas: one line at x 4, baseline the message row's foot - 4, clipped to the canvas width, through
    /// [`Labels`]; unchanged with labels off, on the tracks Surface, or a band under [`MESSAGE_H`].
    fn paint_message(&self, px: Vec<u8>, l: Layout, role: Role) -> Vec<u8> {
        let Some(style) = self.labels.as_ref() else { return px };
        if role == Role::Tracks || l.message_h < MESSAGE_H {
            return px;
        }
        let Some(text) = self.message_text() else { return px };
        let mut c = Canvas { w: l.w, h: l.h, order: vo(self.order), px };
        view::Text::text(&mut Labels(style), &mut c, &text, 4.0, (l.message_y + l.message_h) as f32 - 4.0, (0, l.w as i32));
        c.px
    }
}

// ---- NLETRANSITIONFACE (LEDGER SR691): the transition widget's edge drag and kind / ease (named tail block) ----

/// NLETRANSITIONFACE (SR691): a press on a transition widget's edge zone begins timeline_core's edge drag
/// (`Drag::begin_transition_edge`; the release's generic `Drag::finish` is its ONE `set_transition_dur`, the paint's
/// `Drag::shown` / `paint_drag` its preview and guide); the `kind` / `ease` words are timeline_core's verbs
/// (`EditorState::verb`), with the selected transition's thumbnail re-rendered after them.
impl Face {
    /// The press began a transition edge drag (`Some(true)`); `None`: not on a widget's edge (Alt / Option: no snap).
    fn press_transition_edge(&mut self, x: f64, y: f64, mods: u8) -> Option<bool> {
        let l = self.layout();
        let d = Drag::begin_transition_edge(&mut self.state, l, x, y, mods & bandy::MOD_ALT != 0)?;
        self.drag = Some(d);
        Some(true)
    }
    /// A `kind` / `ease` word on the selected transition: timeline_core's verb; the thumbnail (keyed without the kind
    /// and the ease) is dropped so the next frame renders it again.
    fn look_verb(&mut self, v: &str) -> Result<(), EditError> {
        self.trans_still = None;
        self.state.verb(v).unwrap_or(Err(EditError::Transition))
    }
}

#[cfg(test)]
mod transitionface_tests {
    use super::*;
    use timeline_core::{Ease, TransitionKind, WipeDir, edl};

    /// NLE1's fixture with a 4-tick dissolve on V1's cut at 10 (the widget x 80..112, y 108..118 at zoom 8).
    fn dissolve_face() -> Face {
        let (tl, src) = synth_fixture();
        let tl = tl.add_transition(0, 10, TransitionKind::Dissolve, 4).unwrap();
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
    }

    fn btn(down: bool, x: f64, y: f64, mods: u8) -> SMessage {
        SMessage::BrowserButton { down, x, y, mods, surface: 0 }
    }

    /// M3: a press on the right edge zone is `TransitionOut`; +24 px (3 ticks, no target within `SNAP_PX`) released is
    /// ONE `set_transition_dur(4 + 3)` = `add_transition(0, 10, D, 7)`; nothing commits per move; undo restores.
    #[test]
    fn nletransitionface_right_edge_drag_is_one_set_dur() {
        let mut face = dissolve_face();
        let tl0 = face.state.timeline.clone();
        assert!(face.on(&btn(true, 110.0, 113.0, 0)));
        assert_eq!(face.dragging().map(|d| d.kind), Some(DragKind::TransitionOut));
        assert_eq!(face.state.transition, Some((0, 0)));
        assert!(face.on(&SMessage::BrowserMouseMove(134.0, 113.0, 0)));
        assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((3, None)));
        assert_eq!(face.state.timeline, tl0, "a move commits nothing");
        face.paint();
        assert!(face.on(&btn(false, 134.0, 113.0, 0)));
        assert_eq!(face.state.timeline, tl0.add_transition(0, 10, TransitionKind::Dissolve, 7).unwrap());
        assert_eq!((face.state.transition, face.state.undo_depth()), (Some((0, 0)), 1));
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        assert!(face.state.undo());
        assert_eq!(face.state.timeline, tl0);
    }

    /// M3: released at x 160 the raw delta is 7 ticks, but the right edge (at + dur = 14, at x 112 + 50 = 162) lies
    /// within `SNAP_PX` of B's end at 20 (x 160): delta 6, the guide at 20, dur 10.
    #[test]
    fn nletransitionface_right_edge_snaps_to_a_clip_edge() {
        let mut face = dissolve_face();
        let tl0 = face.state.timeline.clone();
        face.on(&btn(true, 110.0, 113.0, 0));
        face.on(&SMessage::BrowserMouseMove(160.0, 113.0, 0));
        let l = face.layout();
        assert_eq!(l.tick_at(160.0) - l.tick_at(110.0), 7, "the raw delta");
        assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((6, Some(20))));
        face.on(&btn(false, 160.0, 113.0, 0));
        assert_eq!(face.state.timeline, tl0.add_transition(0, 10, TransitionKind::Dissolve, 10).unwrap());
    }

    /// M3: released at x 168 (raw 8 ticks, no target within `SNAP_PX`) the length 12 runs past B's 10 ticks: refused,
    /// the state unchanged, the refusal named.
    #[test]
    fn nletransitionface_past_the_handle_is_refused() {
        let mut face = dissolve_face();
        face.on(&btn(true, 110.0, 113.0, 0));
        let before = face.state.clone();
        face.on(&SMessage::BrowserMouseMove(168.0, 113.0, 0));
        assert_eq!(face.dragging().map(|d| (d.delta, d.guide)), Some((8, None)));
        assert!(face.on(&btn(false, 168.0, 113.0, 0)));
        assert_eq!(face.state, before);
        assert_eq!(face.last_error.take(), Some(format!("drag: {}", EditError::Handle)));
    }

    /// M3: the left edge zone (x 80..84) is `TransitionIn`; with Alt (no snap) -16 px is -2 ticks: dur 4 + 2 = 6 at the
    /// same cut. The widget's middle stays the NLE3d click (no drag).
    #[test]
    fn nletransitionface_left_edge_and_middle() {
        let mut face = dissolve_face();
        let tl0 = face.state.timeline.clone();
        face.on(&btn(true, 82.0, 113.0, bandy::MOD_ALT));
        assert_eq!(face.dragging().map(|d| (d.kind, d.snap)), Some((DragKind::TransitionIn, false)));
        face.on(&SMessage::BrowserMouseMove(66.0, 113.0, 0));
        face.on(&btn(false, 66.0, 113.0, 0));
        assert_eq!(face.state.timeline, tl0.add_transition(0, 10, TransitionKind::Dissolve, 6).unwrap());
        assert!(face.on(&btn(true, 96.0, 113.0, 0)));
        assert!(face.dragging().is_none(), "the middle is a click");
    }

    /// M3: `kind W007` re-puts the dissolve as fourBoxWipe cornersIn (dur, ease kept) and the EDL carries it; `C`,
    /// a refused `W008` and junk are refused leaving the state; bare `kind` steps D -> W001 and wraps W214 -> D; bare
    /// `ease` cycles `Ease::ALL`; an ease survives a kind change; each is ONE undo step; no selection is refused.
    #[test]
    fn nletransitionface_kind_and_ease_verbs() {
        let mut face = dissolve_face();
        let tl0 = face.state.timeline.clone();
        assert!(face.verb("kind"));
        assert_eq!(face.last_error.take(), Some(format!("kind: {}", EditError::Transition)), "no transition selected");
        assert_eq!(face.state.timeline, tl0);
        face.state.select_transition(0, 0).unwrap();
        let kind = |f: &Face| f.state.selected_transition().map(|x| (x.kind, x.dur, x.ease));
        assert!(face.on(&SMessage::Input { target: SURFACE.into(), text: "kind W007".into() }));
        assert_eq!(kind(&face), Some((TransitionKind::Wipe(WipeDir::FourBoxCornersIn), 4, Ease::Linear)));
        let text = edl::write(&face.state.timeline, "NLEKIND").unwrap();
        assert!(text.contains("W007 004"), "{text}");
        assert_eq!(edl::parse(&text, 25).unwrap().tracks[0].transitions, face.state.timeline.tracks[0].transitions);
        for bad in ["kind C", "kind W008", "kind X", "kind D W001", "ease fast"] {
            let before = face.state.clone();
            assert!(face.verb(bad), "{bad}");
            assert_eq!(face.state, before, "{bad}");
            assert!(face.last_error.take().is_some(), "{bad}");
        }
        assert!(face.verb("kind D"));
        assert!(face.verb("kind"));
        assert_eq!(kind(&face).map(|k| k.0), Some(TransitionKind::Wipe(WipeDir::LeftToRight)));
        face.state.set_transition_kind(TransitionKind::Wipe(WipeDir::DoubleFanHorizontal)).unwrap();
        assert!(face.verb("kind"));
        assert_eq!(kind(&face).map(|k| k.0), Some(TransitionKind::Dissolve), "W214 wraps to D");
        for want in [Ease::In, Ease::Out, Ease::InOut, Ease::Linear] {
            assert!(face.verb("ease"));
            assert_eq!(kind(&face).map(|k| k.2), Some(want));
        }
        assert!(face.verb("ease in"));
        assert!(face.verb("kind W001"));
        assert_eq!(kind(&face), Some((TransitionKind::Wipe(WipeDir::LeftToRight), 4, Ease::In)), "a kind change keeps the ease");
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        let depth = face.state.undo_depth();
        assert!(face.verb("kind W001"));
        assert_eq!(face.state.undo_depth(), depth, "the same kind commits nothing");
        assert!(face.state.undo());
        assert_eq!(kind(&face), Some((TransitionKind::Dissolve, 4, Ease::In)));
        assert!(face.state.undo());
        assert_eq!(kind(&face), Some((TransitionKind::Dissolve, 4, Ease::Linear)));
        while face.state.undo() {}
        assert_eq!(face.state.timeline, tl0);
    }
}

// ---- NLE3ESLIPFACE (LEDGER SR737): the face's one-frame slip / slide words (named tail block) ----
// A button or a key carries no number, so the face's four words are SR704's `slip <d>` / `slide <d>` at one frame: the
// timeline's time base is `1/fps` (`Timeline::new`, its only constructor), so one frame is one tick, the step `nudge-` /
// `nudge+` and `[` / `]` already take. `Face::verb` hands the rewritten word to THE dispatch (`EditorState::verb`).
// NLE3ESLIPSTEP (LEDGER SR752): the coarse step is the same words with a `SLIP_STEP` count (`slip+10` -> `slip 10`), bound
// to Ctrl / Cmd chords in KEYS, not VERBS (no button); any other count is not a face word (`slip <d>` takes free counts).

/// NLE3ESLIPSTEP (SR752): the coarse slip / slide step (frames = ticks), the Ctrl / Cmd chords' count.
pub const SLIP_STEP: i64 = 10;

/// `slip-` -> `slip -1`, `slip+` -> `slip 1`, `slide-` -> `slide -1`, `slide+` -> `slide 1` (one frame, in ticks);
/// `slip-10` -> `slip -10` ... (the count exactly [`SLIP_STEP`]); None for any other word.
fn slipslide_word(v: &str) -> Option<String> {
    let (word, rest) = match v.strip_prefix("slip") {
        Some(r) => ("slip", r),
        None => ("slide", v.strip_prefix("slide")?),
    };
    let (sign, count) = match rest.as_bytes().first().copied()? {
        b'-' => (-1i64, &rest[1..]),
        b'+' => (1, &rest[1..]),
        _ => return None,
    };
    let step = match count {
        "" => 1,
        c if c.bytes().all(|b| b.is_ascii_digit()) && c.parse::<i64>() == Ok(SLIP_STEP) => SLIP_STEP,
        _ => return None,
    };
    Some(format!("{word} {}", sign * step))
}

// ---- NLE5DKEYSHELP (LEDGER SR744): the keys help overlay over the timeline band (named tail block) ----
// `?` (a KEYS row) or the word `help` (`Face::verb`) toggles it; while shown the face paints `help_rows()` over the timeline
// band (`Layout` ruler_y..h: the ruler and the tracks; the preview band's foot is the message line's) through the face's own
// `Labels` hook, LABEL ink over a BG fill, only when labels are on, so every labels-off golden is byte-identical. One row per
// KEYS action: its keys joined " / " (" " shown `Space`, the AppKit private-use aliases U+F700..=U+F8FF skipped: each names a
// key another row of the same action names), its phrase from `HELP` (an action with no phrase shows the word itself, so a new
// KEYS row is never missing). The band is 16 + 40 x rows px at every window size: the rows run in columns, each its own width,
// as many as the canvas width holds, the last painted line `…` when rows remain. Stria's rule: the input that dismisses the
// help is NOT applied — a key the face takes (a KEYS row, Escape included) only dismisses it, a press is taken and ignored and
// its release (or click) dismisses; a key the face does not bind (a bare modifier, GTK's `Shift+question`) leaves it shown.
// Owed: NLE5D-KEYS-HELP-KERNEL (the kernel Player paints the same table from EDIT_KEYS), NLE5D-KEYS-HELP-BUTTON (a `?` button).

/// NLE5DKEYSHELP (SR744): each KEYS action as a person reads it.
pub const HELP: &[(&str, &str)] = &[
    ("shuttle-", "play backward (again: faster)"),
    ("stop", "stop"),
    ("shuttle+", "play forward (again: faster)"),
    ("mark-in", "trim the clip's start to the playhead"),
    ("mark-out", "trim the clip's end to the playhead"),
    ("play", "play / pause"),
    ("zoom+", "zoom in"),
    ("zoom-", "zoom out"),
    ("redo", "redo"),
    ("delete", "delete the clip and close the gap"),
    ("home", "go to the start"),
    ("end", "go to the end"),
    ("cancel", "cancel the drag or the export"),
    ("undo", "undo"),
    ("save", "save"),
    ("nudge-", "move the selection one frame earlier"),
    ("nudge+", "move the selection one frame later"),
    ("[", "clip start one frame earlier"),
    ("]", "clip start one frame later"),
    ("mark in", "export from the playhead"),
    ("mark out", "export up to the playhead"),
    ("mark clear", "export the whole timeline"),
    ("slip-", "slip the clip one frame back"),
    ("slip+", "slip the clip one frame on"),
    ("slide-", "slide the clip one frame earlier"),
    ("slide+", "slide the clip one frame later"),
    ("slip-10", "slip the clip 10 frames back"), // NLE3ESLIPSTEP (SR752): the Ctrl / Cmd chords' SLIP_STEP words
    ("slip+10", "slip the clip 10 frames on"),
    ("slide-10", "slide the clip 10 frames earlier"),
    ("slide+10", "slide the clip 10 frames later"),
    ("zoom-fit", "fit the whole timeline in the window"), ("zoom-sel", "fit the selection in the window"), // STRIAZOOMFITKEYS (SR907)
    ("help", "show / hide this help"),
];
/// NLE5DKEYSHELP (SR744): the help's inset from the band's top and left (px).
const HELP_PAD: f32 = 4.0;
/// NLE5DKEYSHELP (SR744): the gap between a row's keys and its phrase (px).
const HELP_KEY_GAP: f32 = 12.0;
/// NLE5DKEYSHELP (SR744): the gap between two columns (px).
const HELP_COL_GAP: f32 = 16.0;

/// The phrase for a KEYS action ([`HELP`]); the action word itself when it has none.
pub fn help_phrase(act: &'static str) -> &'static str {
    HELP.iter().find(|(a, _)| *a == act).map_or(act, |&(_, p)| p)
}

/// A KEYS name as the help shows it: `" "` is `Space`; an AppKit private-use key (U+F700..=U+F8FF) is not shown.
fn key_label(k: &'static str) -> Option<&'static str> {
    if k == " " {
        return Some("Space");
    }
    let mut cs = k.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) if ('\u{f700}'..='\u{f8ff}').contains(&c) => None,
        _ => Some(k),
    }
}

/// The help's rows, one per KEYS action in KEYS order: `(its keys joined " / ", its phrase)`.
pub fn help_rows() -> Vec<(String, &'static str)> {
    let mut rows: Vec<(&'static str, String, &'static str)> = Vec::new();
    for &(k, act) in KEYS {
        let i = match rows.iter().position(|r| r.0 == act) {
            Some(i) => i,
            None => {
                rows.push((act, String::new(), help_phrase(act)));
                rows.len() - 1
            }
        };
        if let Some(lab) = key_label(k) {
            let keys = &mut rows[i].1;
            if !keys.is_empty() {
                keys.push_str(" / ");
            }
            keys.push_str(lab);
        }
    }
    rows.into_iter().map(|(_, k, p)| (k, p)).collect()
}

impl Face {
    /// The keys help is shown.
    pub fn help_shown(&self) -> bool {
        self.help
    }
    /// While the help shows, the pointer's buttons: a press is taken and ignored (no drag begins), its release or click
    /// dismisses the help and is not applied (`Some(repaint)`); `None` for every other message, or with the help hidden.
    fn help_input(&mut self, msg: &SMessage) -> Option<bool> {
        if !self.help {
            return None;
        }
        match msg {
            SMessage::BrowserButton { down: true, .. } => {
                self.buttons = true;
                Some(false)
            }
            SMessage::BrowserButton { down: false, .. } => {
                self.help = false;
                Some(true)
            }
            SMessage::BrowserClick(..) if !self.buttons => {
                self.help = false;
                Some(true)
            }
            _ => None,
        }
    }
    /// The help over a painted canvas: the timeline band (`ruler_y..h`) filled BG, then [`help_rows`] in columns through
    /// [`Labels`]; unchanged with labels off, with the help hidden, or a band under one line.
    fn paint_help(&self, px: Vec<u8>, l: Layout) -> Vec<u8> {
        let Some(style) = self.labels.as_ref() else { return px };
        if !self.help || l.ruler_y >= l.h {
            return px;
        }
        let (ascent, descent, gap) = style.metrics();
        let pitch = (ascent + descent + gap).max(1.0);
        let top = l.ruler_y as f32 + HELP_PAD;
        let per = ((l.h as f32 - top) / pitch).floor().max(0.0) as usize;
        if per == 0 {
            return px;
        }
        let rows = help_rows();
        // The columns: (first row, keys width, phrase width), as many as the width holds, at least one.
        let mut cols: Vec<(usize, f32, f32)> = Vec::new();
        let (mut x, mut start) = (HELP_PAD, 0usize);
        while start < rows.len() {
            let chunk = &rows[start..(start + per).min(rows.len())];
            let kw = chunk.iter().map(|r| style.width(&r.0)).fold(0.0f32, f32::max);
            let pw = chunk.iter().map(|r| style.width(r.1)).fold(0.0f32, f32::max);
            if !cols.is_empty() && x + kw + HELP_KEY_GAP + pw > l.w as f32 {
                break;
            }
            cols.push((start, kw, pw));
            x += kw + HELP_KEY_GAP + pw + HELP_COL_GAP;
            start += per;
        }
        let cut = start < rows.len();
        let mut c = Canvas { w: l.w, h: l.h, order: vo(self.order), px };
        c.rect(0, l.ruler_y as i64, l.w as i64, l.h as i64, view::BG);
        let mut lab = Labels(style);
        let mut x = HELP_PAD;
        for (ci, &(first, kw, pw)) in cols.iter().enumerate() {
            let end = (first + per).min(rows.len());
            let clip = (x as i32, (x + kw + HELP_KEY_GAP + pw).ceil().min(l.w as f32) as i32);
            for (n, (keys, phrase)) in rows[first..end].iter().enumerate() {
                let baseline = top + ascent + n as f32 * pitch;
                if cut && ci + 1 == cols.len() && first + n + 1 == end {
                    view::Text::text(&mut lab, &mut c, "…", x, baseline, clip);
                } else {
                    view::Text::text(&mut lab, &mut c, keys, x, baseline, clip);
                    view::Text::text(&mut lab, &mut c, phrase, x + kw + HELP_KEY_GAP, baseline, clip);
                }
            }
            x += kw + HELP_KEY_GAP + pw + HELP_COL_GAP;
        }
        c.px
    }
}

// ---- NLE5DEXPORTMESSAGEAUDIO (LEDGER SR756): the result line and the refusal name the words (named tail block) ----
/// The result line's words for what was written: `<video> + <audio>` (the audio as the table spells it,
/// [`crate::nle_export::audio_word`]: `wav`, `mp4`, `aac`, `aac:K`), or `<video>, no audio` under `AudioOut::None`; then ` in <t>`, the job's wall time ([`wall_words`], NLE5DEXPORTMESSAGEDURATION SR775), and ` (<r> fps, <m>x)` ([`rate_words`], NLE5DEXPORTMESSAGERATE SR785; the multiple of real time at the timeline's frame rate `fps` as (num, den), [`realtime_words`], SR794).
fn export_words(video: &str, audio: AudioOut, wall: Duration, frames: u64, fps: (u64, u64)) -> String {
    match audio {
        AudioOut::None => format!("{video}, no audio in {}{}", wall_words(wall), rate_words(frames, wall, fps).map(|r| format!(" ({r})")).unwrap_or_default()),
        a => format!("{video} + {} in {}{}", crate::nle_export::audio_word(&a), wall_words(wall), rate_words(frames, wall, fps).map(|r| format!(" ({r})")).unwrap_or_default()),
    }
}

/// The `export` verb's usage, from the two tables (`VIDEO_NAMES`, `AUDIO_NAMES`; `aac` shown as `aac[:KBPS]`):
/// `export [--all] [<dir>] [h264|png|i420|y4m|raw] [wav|aac[:KBPS]|mp4|none]`. The usage refusal and the unknown-audio
/// refusal both name it.
fn export_usage() -> String {
    let audio: Vec<&str> = crate::nle_export::AUDIO_NAMES.iter().map(|w| if *w == "aac" { "aac[:KBPS]" } else { *w }).collect();
    format!("export [--all] [<dir>] [{}] [{}]", VIDEO_NAMES.join("|"), audio.join("|"))
}

impl Face {
    /// The running export's audio word ([`crate::nle_export::audio_word`]), `None` when no export runs (the witness's
    /// `"audio"` field).
    pub fn export_audio(&self) -> Option<String> {
        self.export.as_ref().map(|j| crate::nle_export::audio_word(&j.audio))
    }
}

// ---- STRIAINFOREELHOVER (LEDGER SR762): the message names the clip under the pointer (named tail block) ----
// Every editor names the hovered clip in its status line. The face keeps no hover state: the clip under the pointer is THE hit
// test (`Layout::hit` through `Face::layout`, the geometry `press` / `click` use) at THE pointer (`Face::pointer`), read when
// the message is (`reel_line`); a bare move asks a repaint only when that clip changed and labels are on (labels off: false, as
// before). Owed: STRIA-INFO-REEL-HOVER-DELAY (a dwell before the hover line shows: the face has no timer),
// STRIA-INFO-REEL-HOVER-KERNEL (the kernel face), STRIA-INFO-REEL-HOVER-WITNESS-DOC (stria-view's `reel_json` doc line).

impl Face {
    /// STRIAINFOREELHOVER (SR762): the clip under the pointer when it is not the selected one; `None` while a drag is live,
    /// with no pointer, or over the ruler, an empty track spot, nothing, or the selected clip itself.
    pub fn hovered_clip(&self) -> Option<(usize, usize)> {
        if self.drag.is_some() {
            return None;
        }
        let (x, y) = self.pointer?;
        match self.layout().hit(&self.state.timeline, x, y) {
            Hit::Clip(t, i, _) if self.state.selection != Some((t, i)) => Some((t, i)),
            _ => None,
        }
    }
    /// STRIAINFOREELHOVER (SR762): the pointer moved. A live drag: `None` ([`Face::hover`] goes on as before); else whether the
    /// message changed (labels on and the hovered clip is another one).
    fn hover_message(&mut self, x: f64, y: f64) -> Option<bool> {
        let was = self.hovered_clip();
        self.pointer = Some((x, y));
        if self.drag.is_some() {
            return None;
        }
        Some(self.hover_moved(was) && self.labels.is_some()) // STRIAINFOREELHOVERDELAY (SR789): a move repaints only when a dwelt hover ends (file tail)
    }
}

// ---- STRIAINFOREELHOVERDELAY (LEDGER SR789): a dwell of HOVER_DWELL face ticks before the hover line replaces the selected one (named tail block) ----
// SR762 swapped the message line on the very move that entered a clip, so a pointer crossing the timeline flickered the line
// through every clip it passed; every editor dwells first (Chromium's hover intent ~300 ms, macOS tooltips 500 ms). The dwell
// counts on THE face clock (`serve`'s one interval, `Face::tick`): no timer, no Instant, no thread. `hover_ticks` keys the count
// to its clip, so a change with no move (an edit, a select, a playback scroll) is seen at the next tick. `serve` runs the clock
// while a dwell counts (`hover_pending`) and skips the ticks missed while it was idle. Owed: STRIA-INFO-REEL-HOVER-DWELL-MS
// (the dwell in wall ms: today HOVER_DWELL frames of the timeline's rate), STRIA-PLAY-IDLE-BURST (pin play after idle under Skip).

/// STRIAINFOREELHOVERDELAY (SR789): the face ticks a hover must stand before its line shows: one tick is one timeline
/// frame (`serve`'s clock), so 8 is 320 ms at 25 fps, 333 ms at 24, 267 ms at 30.
pub const HOVER_DWELL: u16 = 8;

impl Face {
    /// STRIAINFOREELHOVERDELAY (SR789): one face tick of the dwell: the same hovered clip counts on, another starts at 1,
    /// none clears it. True only on the tick the count reaches [`HOVER_DWELL`], and only with labels on (the message paints).
    fn hover_dwell_tick(&mut self) -> bool {
        let next = match (self.hover_ticks, self.hovered_clip()) {
            (Some((c, n)), Some(h)) if c == h => Some((c, n.saturating_add(1))),
            (_, Some(h)) => Some((h, 1)),
            (_, None) => None,
        };
        self.hover_ticks = next;
        self.labels.is_some() && next.is_some_and(|(_, n)| n == HOVER_DWELL)
    }
    /// STRIAINFOREELHOVERDELAY (SR789): the hovered clip once it has stood [`HOVER_DWELL`] ticks (the line `reel_line` names).
    pub fn hover_dwelt(&self) -> Option<(usize, usize)> {
        let h = self.hovered_clip()?;
        self.hover_ticks.filter(|&(c, n)| c == h && n >= HOVER_DWELL).map(|(c, _)| c)
    }
    /// STRIAINFOREELHOVERDELAY (SR789): a dwell is counting (labels on, a hovered clip not yet dwelt): `serve` runs its clock.
    pub fn hover_pending(&self) -> bool {
        self.labels.is_some() && self.hovered_clip().is_some() && self.hover_dwelt().is_none()
    }
    /// STRIAINFOREELHOVERDELAY (SR789): the pointer moved off `was`: when the hovered clip changed the count resets, and the
    /// answer is whether `was` had dwelt (its line was showing); false when the hovered clip is unchanged.
    fn hover_moved(&mut self, was: Option<(usize, usize)>) -> bool {
        if self.hovered_clip() == was {
            return false;
        }
        let dwelt = was.is_some() && self.hover_ticks.is_some_and(|(c, n)| Some(c) == was && n >= HOVER_DWELL);
        self.hover_ticks = None;
        dwelt
    }
}

// ---- NLE5DEXPORTMESSAGEDURATION (LEDGER SR775): the export's wall time on the result line (named tail block) ----
// `ExportJob.started` is set in `start_export` and read after the join in `finish_export`, both on the face's thread;
// the export thread never sees it. Owed: NLE5D-EXPORT-MESSAGE-DURATION-WITNESS (the elapsed on stria-view's export
// JSON), NLE5D-EXPORT-MESSAGE-RATE (the export's frames per second).

/// A wall time as a person reads it, the way a stopwatch reads (each unit FLOORED, so nothing carries): under 10 s one
/// decimal (`0.8 s`, `9.9 s`), under a minute whole seconds (`12 s`), under an hour minutes and zero-padded seconds
/// (`1 min 05 s`), else hours and zero-padded minutes (`1 h 02 min`, `25 h 00 min`).
pub fn wall_words(d: Duration) -> String {
    let ms = d.as_millis();
    let s = ms / 1000;
    if s < 10 {
        format!("{s}.{} s", (ms % 1000) / 100)
    } else if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min {:02} s", s / 60, s % 60)
    } else {
        format!("{} h {:02} min", s / 3600, (s % 3600) / 60)
    }
}

// ---- NLE5DEXPORTMESSAGERATE (LEDGER SR785): the export's frames per second on the result line (named tail block) ----
// `export_words` takes the frame count the line prints (`ExportReport.frames`, the ONE caller's `r.frames`) beside the
// wall time and appends ` (<r> fps)` after ` in <t>`. Owed: NLE5D-EXPORT-MESSAGE-RATE-REALTIME (the rate as a multiple
// of real time, `2.1x`, beside the fps).

/// An export's speed as a person reads it (every encoder prints one: x264's `42.1 fps`): `frames` over `wall`, rounded
/// half up in integer arithmetic on microseconds; 100 or more a whole number (`120 fps`, `500 fps`), below that one
/// decimal (`43.8 fps`, `3.0 fps`, `0.3 fps`). `None` for 0 frames or a wall under 1 ms: such an export says nothing of speed.
pub fn rate_words(frames: u64, wall: Duration, fps: (u64, u64)) -> Option<String> {
    if frames == 0 || wall < Duration::from_millis(1) {
        return None;
    }
    let (f, us) = (frames as u128, wall.as_micros());
    let tenths = (f * 20_000_000 + us) / (2 * us);
    Some(format!("{}{}", if tenths >= 1000 { format!("{} fps", (f * 2_000_000 + us) / (2 * us)) } else { format!("{}.{} fps", tenths / 10, tenths % 10) }, realtime_words(frames, wall, fps).map(|m| format!(", {m}")).unwrap_or_default()))
}

// ---- NLE5DEXPORTMESSAGERATEREALTIME (LEDGER SR794): the export's speed as a multiple of real time (named tail block) ----
// `rate_words` takes the timeline's frame rate as (num, den) — `(tb.den, tb.num)` from the face's `Timeline.tb` (seconds
// per tick; written only by `Timeline::new`, so the face's timeline carries the exported clone's rate) — and appends
// `, <m>x` inside its parenthesis. Owed: NLE5D-EXPORT-MESSAGE-ETA (a running export's remaining time from the multiple so
// far, on the progress band and stria-view's export JSON), NLE5D-EXPORT-MESSAGE-DURATION-WITNESS (carried).

/// An export's speed as a multiple of real time (`1.8x`: it ran 1.8 times faster than the timeline plays; `0.40x`:
/// slower): `frames` over `wall`, divided by the frame rate `fps` = (num, den) frames per second, each grade rounded half up
/// in integer arithmetic on microseconds (the [`rate_words`] road); 10 or more a whole number (`12x`, `20x`), 1 to 10 one
/// decimal (`1.8x`), below 1 two decimals (`0.40x`). `None` for 0 frames, a wall under 1 ms, a rate with a zero term (a
/// timeline without a rate), or an overflow.
pub fn realtime_words(frames: u64, wall: Duration, fps: (u64, u64)) -> Option<String> {
    let (num, den) = fps;
    if frames == 0 || num == 0 || den == 0 || wall < Duration::from_millis(1) {
        return None;
    }
    let top = (frames as u128).checked_mul(den as u128)?;
    let bot = wall.as_micros().checked_mul(num as u128)?;
    let q = |k: u128| -> Option<u128> { Some(top.checked_mul(k * 2_000_000)?.checked_add(bot)? / bot.checked_mul(2)?) };
    let tenths = q(10)?;
    Some(if tenths >= 100 {
        format!("{}x", q(1)?)
    } else if tenths >= 10 {
        format!("{}.{}x", tenths / 10, tenths % 10)
    } else {
        let h = q(100)?;
        format!("{}.{:02}x", h / 100, h % 100)
    })
}

// ---- NLE5DEXPORTMESSAGEETA (LEDGER SR796): a running export's remaining time from the rate so far (named tail block) ----
// `Face::export_eta` reads the job's `started` (SR756, face thread only) and the `export_progress` pair through ONE formula
// (`eta_of`); the words and stria-view's `"eta_secs"` read ONE rounding (`eta_secs`). The band stays a bar. Owed:
// NLE5D-EXPORT-MESSAGE-ETA-BAND (a painted `~12 s` on the band or the message line), NLE5D-EXPORT-MESSAGE-ETA-SMOOTH (an EMA
// of the rate so a slow start does not pin a long eta), NLE5D-EXPORT-MESSAGE-DURATION-WITNESS (carried).

/// The remaining time of an export `done` of `total` frames in after `elapsed`: `(total - done) x elapsed / done` in integer
/// microseconds. `None` while `done` is 0 or `elapsed` is under 1 ms (no estimate yet), or on an overflow.
fn eta_of(done: u64, total: u64, elapsed: Duration) -> Option<Duration> {
    if done == 0 || elapsed < Duration::from_millis(1) {
        return None;
    }
    let us = (total.saturating_sub(done) as u128).checked_mul(elapsed.as_micros())? / done as u128;
    Some(Duration::from_micros(u64::try_from(us).ok()?))
}

/// A remaining time in whole seconds, rounded half up: THE rounding the words ([`eta_words`]) and the witness's
/// `"eta_secs"` both read, so they never disagree.
pub fn eta_secs(d: Duration) -> u64 {
    ((d.as_micros() + 500_000) / 1_000_000) as u64
}

/// A running export's remaining time as a person reads it, the tilde saying estimate: under a minute whole seconds
/// (`~12 s`, `~0 s` under half a second), under an hour minutes and seconds (`~1:05`), else hours (`~1:02:03`); the grade is
/// chosen after the rounding ([`eta_secs`]), so 59.6 s reads `~1:00`. `None` while there is no estimate ([`eta_of`]).
pub fn eta_words(done: u64, total: u64, elapsed: Duration) -> Option<String> {
    eta_of(done, total, elapsed).map(eta_words_of)
}

// ---- NLE5DEXPORTMESSAGEETABAND (LEDGER SR803): the running export's line on the message line (named tail block) ----
// While an export runs, `Face::message_text` reads `job_line` after `last_error` and before the hovered / selected reel line,
// so the one line under the preview says `export: 3 / 35 ~12 s` (labels on only: `paint_message`'s gate, so every labels-off
// paint is byte-identical); the 2 px band stays the glance. `export_tick` keeps the fnv1a of this line in `ExportJob.shown`, so
// the face repaints when the eta word moves with done fixed (labels off: frames re-fired ~1/s, pixels unchanged). Owed:
// NLE5D-EXPORT-MESSAGE-ETA-TOTAL (the real count before frame 1: total is 0 until the export thread's first reading),
// NLE5D-EXPORT-MESSAGE-ETA-KERNEL (the kernel Player paints the same line), NLE5D-EXPORT-MESSAGE-ETA-SMOOTH (carried),
// NLE5D-EXPORT-MESSAGE-DURATION-WITNESS (carried).

/// A running export as a person reads it: `export: <done> / <total>`, then ` <eta>` when there is one ([`eta_words`]:
/// `export: 3 / 35 ~12 s`); `export: starting` while `total` is 0 (an export of no frames).
pub fn export_line(done: u64, total: u64, eta: Option<String>) -> String {
    if total == 0 {
        return "export: starting".to_string();
    }
    match eta {
        Some(e) => format!("export: {done} / {total} {e}"),
        None => format!("export: {done} / {total}"),
    }
}

/// The running job's [`export_line`]: its `done` / `total` reading (as [`Face::export_progress`] reads it) and SR796's
/// [`eta_words`] over its `started`. A free fn over the job, so [`Face::export_tick`] calls it under its live `&mut` borrow.
/// A stall past [`STALL_AFTER`] trades the eta word for [`stall_words`] (`export: 3 / 35 stalled 7 s`, NLE5DEXPORTMESSAGEETASTALLWORD SR817).
fn job_line(j: &ExportJob) -> String {
    let (d, t) = (j.done.load(Relaxed), j.total.load(Relaxed));
    export_line(d, t, finishing_words(d, t).or_else(|| stall_words(j.rate_at.0.elapsed(), j.rate_ema)).or_else(|| job_eta(j, d, t).map(eta_words_of)))
}

// ---- NLE5DEXPORTMESSAGEETASMOOTH (LEDGER SR808): a time-based EMA of the export rate for the eta (named tail block) ----
// SR796's eta divided the remaining frames by the MEAN rate since the start, so a slow first frame (the encoder warming, a
// cold cache, the first keyframe) pinned a long eta that collapsed as the export ran. `ExportJob` now carries `rate_ema`
// (frames per second, 0.0 = no estimate yet) and `rate_at` (the last sample: its instant and `done`; built `(started, 0)`),
// written only by `eta_sample` from `Face::export_tick` on the face's thread (no lock: `done` stays the export thread's
// atomic). THE TIME RULE: `ema_step` weighs a sample by `1 - exp(-dt / ETA_TAU)`, so N steps of dt are one step of N x dt and
// the smoothing is the same at the serve clock's 25 or 60 Hz. THE HAND-OFF: the first sampled move (the warm-up span from
// `started`) only records `rate_at`; the second seeds `rate_ema` with its span's rate; later moves step the EMA; a tick where
// `done` did not move changes nothing (a paused job keeps its rate). `job_eta` is the ONE eta both the message line
// (`job_line`) and `Face::export_eta` (stria-view's `"eta_secs"`) read: SR796's `eta_of` while `rate_ema` is 0, else
// `eta_of_rate(total - done, rate_ema)`; the words are SR796's one grammar (`eta_words_of`). Owed:
// NLE5D-EXPORT-MESSAGE-ETA-STALL (a stalled export's eta freezes; it should grow by the overdue time), NLE5D-EXPORT-MESSAGE-ETA-KERNEL,
// NLE5D-EXPORT-MESSAGE-ETA-TOTAL, NLE5D-EXPORT-MESSAGE-DURATION-WITNESS (carried).

/// The EMA's time constant (seconds): a sample `ETA_TAU` old weighs 1/e of a fresh one.
pub const ETA_TAU: f64 = 2.0;

/// One time-based EMA step: `prev + (1 - exp(-dt / tau)) x (sample - prev)` (`dt`, `tau` in seconds). `dt` 0 keeps `prev`;
/// `dt == tau` moves 1 - 1/e of the way; N steps of `dt` equal one step of N x `dt` (the tick-rate independence).
pub fn ema_step(prev: f64, sample: f64, dt: f64, tau: f64) -> f64 {
    prev + (1.0 - (-dt / tau).exp()) * (sample - prev)
}

/// One sample of a running export's rate, pure: `(ema, at)` is the job's `(rate_ema, rate_at)`, `done` the reading at `now`.
/// No move (or no time) since `at`: unchanged. The first move (from 0, the warm-up span) records `(now, done)` with the EMA
/// still 0; the second seeds the EMA with its span's frames per second; later moves [`ema_step`] it over [`ETA_TAU`].
pub fn ema_sample(ema: f64, at: (std::time::Instant, u64), done: u64, now: std::time::Instant) -> (f64, (std::time::Instant, u64)) {
    let dt = now.saturating_duration_since(at.0).as_secs_f64();
    if done <= at.1 || dt <= 0.0 {
        return (ema, at);
    }
    if at.1 == 0 {
        return (0.0, (now, done));
    }
    let rate = (done - at.1) as f64 / dt;
    (if ema > 0.0 { ema_step(ema, rate, dt, ETA_TAU) } else { rate }, (now, done))
}

/// [`ema_sample`] over the job's two fields (face thread only), from [`Face::export_tick`].
fn eta_sample(j: &mut ExportJob, now: std::time::Instant) {
    let (ema, at) = ema_sample(j.rate_ema, j.rate_at, j.done.load(Relaxed), now);
    j.rate_ema = ema;
    j.rate_at = at;
}

/// The remaining time of `remaining` frames at `rate` frames per second. `None` for a rate that is not positive and finite,
/// or a time a `Duration` cannot hold; plus `since_last` (the time since the last sampled move) beyond one frame interval `1 / rate`, never negative, so a stalled export's eta grows (NLE5DEXPORTMESSAGEETASTALL SR811).
pub fn eta_of_rate(remaining: u64, rate: f64, since_last: Duration) -> Option<Duration> {
    if !(rate > 0.0 && rate.is_finite()) {
        return None;
    }
    Duration::try_from_secs_f64(remaining as f64 / rate + (since_last.as_secs_f64() - 1.0 / rate).max(0.0)).ok()
}

/// THE eta of a running job at the reading `(d, t)`: [`eta_of_rate`] over the EMA once it has a rate, else SR796's mean
/// ([`eta_of`] over `started`), so a job sampled fewer than two moves (every paused fixture) reads the mean unchanged.
fn job_eta(j: &ExportJob, d: u64, t: u64) -> Option<Duration> {
    if j.rate_ema > 0.0 {
        eta_of_rate(t.saturating_sub(d), j.rate_ema, j.rate_at.0.elapsed())
    } else {
        eta_of(d, t, j.started.elapsed())
    }
}

/// A remaining time as a person reads it (SR796's grammar, moved here so the EMA's eta and [`eta_words`] share it): under a
/// minute whole seconds (`~12 s`), under an hour `~1:05`, else `~1:02:03`, graded after the rounding ([`eta_secs`]).
pub fn eta_words_of(d: Duration) -> String {
    let s = eta_secs(d);
    if s < 60 {
        format!("~{s} s")
    } else if s < 3600 {
        format!("~{}:{:02}", s / 60, s % 60)
    } else {
        format!("~{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    }
}

// ---- NLE5DEXPORTMESSAGEETASTALLWORD (LEDGER SR817): a stalled export's line names the stall (named tail block) ----
// SR811 grows a stalled export's eta, but the line still read like progress (`export: 3 / 35 ~19 s` climbing). THE STALL
// RULE: `job_line` passes `stall_words(rate_at.0.elapsed(), rate_ema)` in place of the eta word when the time since the last
// sampled move exceeds max(STALL_AFTER, STALL_FRAMES / rate) for a positive finite EMA rate (a slow-but-moving export is not
// called stalled), else a flat STALL_AFTER (the mean road, before the EMA has a rate: `rate_at.0` is `started` until the first
// sampled move, so a first frame that never comes still names its stall). The seconds read SR796's one grammar
// (`eta_words_of`, half up through `eta_secs`) without the tilde. `export_line`, `job_eta`, `Face::export_eta` and stria-view's
// `"eta_secs"` are unchanged; the word returns to the eta at the next move. Owed: NLE5D-EXPORT-MESSAGE-ETA-STALL-WITNESS (the
// stall seconds on stria-view's export JSON), NLE5D-EXPORT-MESSAGE-ETA-STALL-FINALIZE (done == total: name the finalise, not a
// stall), NLE5D-EXPORT-MESSAGE-ETA-KERNEL, NLE5D-EXPORT-MESSAGE-ETA-TOTAL, NLE5D-EXPORT-MESSAGE-DURATION-WITNESS (carried).

/// The least time since the last sampled move that reads as a stall.
pub const STALL_AFTER: Duration = Duration::from_secs(5);

/// The frame intervals at the EMA rate that must also pass before a moving-but-slow export reads as stalled.
pub const STALL_FRAMES: f64 = 10.0;

/// A running export's stall as a person reads it: `stalled 7 s` (`stalled 4:10` past a minute, [`eta_words_of`]'s grammar
/// without the tilde) when `since_last` (the time since the last sampled move) is strictly over max([`STALL_AFTER`],
/// [`STALL_FRAMES`] / `rate`), or over a flat [`STALL_AFTER`] while `rate` is not positive and finite; else `None`.
pub fn stall_words(since_last: Duration, rate: f64) -> Option<String> {
    stall_since(since_last, rate).map(|d| format!("stalled {}", eta_words_of(d).trim_start_matches('~')))
}

// ---- NLE5DEXPORTMESSAGEETASTALLWITNESS (LEDGER SR825): the stall rule's predicate for the witness; the finalise named (named tail block) ----
// SR817 named a stall on the face's line only: stria-view's JSON carried a growing `eta_secs` and no word, and an export whose
// frames were all in but whose finalise (the mp4 close, the wav header: `export_as_with`'s `sink.finish` after the last
// `progress(total, total)`) ran past 5 s read `35 / 35 stalled 6 s`. SR817's predicate moves verbatim into `stall_since`;
// `stall_words` (the word) and `Face::export_stall` (the witness's `"stalled_secs"`) read that ONE rule. `job_line` puts
// `finishing_words` before the stall word, so done == total (total > 0) reads `export: 35 / 35 finishing` (no eta, no stall)
// until the job joins and the finished line replaces it; `export_stall` is None there too. Owed:
// NLE5D-EXPORT-MESSAGE-FINISHING-WITNESS (a `"finishing":true` key if the local session wants it), NLE5D-EXPORT-MESSAGE-ETA-KERNEL,
// NLE5D-EXPORT-MESSAGE-DURATION-WITNESS (carried).

/// SR817's stall rule as a time: `Some(since_last)` when `since_last` (the time since the last sampled move) is strictly over
/// max([`STALL_AFTER`], [`STALL_FRAMES`] / `rate`) for a positive finite `rate`, or over a flat [`STALL_AFTER`] otherwise; else
/// `None`. [`stall_words`] is its word; [`Face::export_stall`] its witness.
pub fn stall_since(since_last: Duration, rate: f64) -> Option<Duration> {
    let after = STALL_AFTER.as_secs_f64();
    let limit = if rate > 0.0 && rate.is_finite() { after.max(STALL_FRAMES / rate) } else { after };
    (since_last.as_secs_f64() > limit).then_some(since_last)
}

/// The finalise's word: `finishing` when every frame is in (`done == total`, `total > 0`) and the job has not joined (the mp4
/// close, the wav header); else `None`. [`job_line`] reads it before the stall word.
pub fn finishing_words(done: u64, total: u64) -> Option<String> {
    (total > 0 && done == total).then(|| "finishing".to_string())
}

impl Face {
    /// The running export's stall ([`stall_since`] over the job's `rate_at` and `rate_ema`, the rule [`job_line`]'s word reads),
    /// the sibling of [`Face::export_eta`]; `None` with no export, while not stalled, and while finishing ([`finishing_words`]).
    pub fn export_stall(&self) -> Option<Duration> {
        let j = self.export.as_ref()?;
        let (d, t) = self.export_progress()?;
        if finishing_words(d, t).is_some() {
            return None;
        }
        stall_since(j.rate_at.0.elapsed(), j.rate_ema)
    }
}

// ---- STRIAZOOMTOFIT (LEDGER SR859): `zoom-fit` / `zoom-sel` fit the timeline or the selection set in the window (named tail block) ----
// timeline_core's `EditorState::fit` sets the one zoom from the face's width; the origin it returns is written into THE
// view anchor the wheel's `zoom_at` writes (`Face.view`), which `Face::layout` applies unchanged. A view change: no undo
// step, the document not dirtied. A refusal (nothing to fit) is `last_error` through `Face::verb`'s `<v>: <e>` text.

impl Face {
    /// The fit words; None for any other word.
    fn fit_verb(&mut self, v: &str) -> Option<bool> {
        if v != "zoom-fit" && v != "zoom-sel" {
            return None;
        }
        let w = self.layout().w;
        match self.state.fit(w, v == "zoom-sel") {
            Ok(t0) => { self.view = Some((t0, self.state.playhead)); self.fit_mark(v) }
            Err(e) => self.last_error = Some(format!("{v}: {e}")),
        }
        Some(true)
    }
}

// ---- STRIAZOOMFITKEYS (LEDGER SR907): fit keys and the refit on resize (named tail block) ----
// `\` and `|` are KEYS rows naming the VERBS words `zoom-fit` / `zoom-sel` (Shift+Z, the usual fit key, is redo here), so
// they reach `fit_verb` through `Face::key`'s VERBS road; HELP names both. A successful fit remembers `(sel, zoom, view)`;
// the tracks resize arm of `Face::on` refits at the new width while the zoom and the view still equal what the fit set
// (every other view word, the wheel, `BrowserZoom`, `zoom-` / `zoom+` / `zoom0`, changes one of them). The memory is taken
// before the refit: a refused refit (the set emptied) reports once through `fit_verb`'s own road and is not repeated. A view
// change: no undo step, the document not dirtied. Owed: NLE-ZOOMFIT-KEYS-KERNEL (the Player's EDIT_KEYS byte, after its fit).

impl Face {
    /// Remember the fit word `v` just applied, with the zoom and the view it set.
    fn fit_mark(&mut self, v: &str) {
        self.zoom_fit = Some((v == "zoom-sel", self.state.zoom, self.view));
    }
    /// After a tracks resize: fit again at the new width when the last view word was a fit (its zoom and view still hold).
    fn refit_on_resize(&mut self) {
        let Some((sel, zoom, view)) = self.zoom_fit.take() else { return };
        if zoom != self.state.zoom || view != self.view {
            return;
        }
        self.fit_verb(if sel { "zoom-sel" } else { "zoom-fit" });
    }
}
