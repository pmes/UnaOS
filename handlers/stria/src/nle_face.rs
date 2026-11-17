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
//! * STRIAAUDIOSCRUB (LEDGER SR865): a ruler scrub step (with a device, not playing) is ONE `play_tick` of that tick: its still, the meters, one frame of sound.
//! * STRIAAUDIOSCRUBMORE (LEDGER SR947): a clip / track click sounds its frame too; a scrub burst waits until the last snippet is mostly played; the meter strip shows the last sounded scrub step.
//! * STRIAAUDIOSCRUBNEXT (LEDGER SR988): a scrub step the gate refused sounds on a later face clock tick.
//! * STRIAKSCRUB (LEDGER SR9060): the scrub step's tick is `Drag::scrub_tick` (timeline_core), the kernel Player's per-step seek reads the same fn.

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
pub use timeline_core::view::{Drag, DragKind, DragShown, Grab, HANDLE_W, Hit, Layout, MARQUEE_MIN, ROW_H, RULER_H, Role, SNAP_PX, Still, THUMB_H, THUMB_W, TRANS_H, TRANS_MIN_W, fnv1a, snap_targets, snap_to, transition_mid}; // NLE6FACEDRAG (rmbp-ledger B975): the drag model moved to timeline_core::view, both faces call it
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
struct Labels<'a>(&'a TextStyle, Option<&'a LabelCache>); // STRIAPERFLABELSHAPE (SR935): the face's shaped-line cache, `None` shapes every call (file tail)

impl view::Text for Labels<'_> {
    fn text(&mut self, c: &mut Canvas, s: &str, x: f32, baseline: f32, clip: (i32, i32)) {
        let (w, h, order) = (c.w, c.h, po(c.order));
        shaped(self.0, self.1, s).paint(&mut c.px, w, h, w as usize * 4, order, x, baseline, LABEL, clip); // STRIAPERFLABELSHAPE (SR935): a cached line by text (file tail)
    }
}

/// Paint the editor on a `w` x `h` canvas in `order` (see the module docs). `labels` names the clips (and the
/// seconds on the ruler) through text_host; `preview` is the frame at the playhead.
pub fn paint(st: &EditorState, w: u32, h: u32, order: PixelOrder, labels: Option<&TextStyle>, preview: Option<&Still>) -> Vec<u8> {
    paint_in(st, Layout::of(w, h, st), order, labels, preview, None)
}

/// [`paint`] in a given layout (the face's view, which a wheel zoom anchors at the pointer: [`Face::layout`]): timeline_core's
/// `view::paint_in`, the labels through [`Labels`].
pub fn paint_in(st: &EditorState, l: Layout, order: PixelOrder, labels: Option<&TextStyle>, preview: Option<&Still>, peaks: Option<&dyn view::PeakSource>) -> Vec<u8> {
    let lab = labels.map(|s| Labels(s, None)); // STRIAPERFLABELSHAPE (SR935): the free paint shapes uncached (file tail)
    paint_labelled(st, l, order, lab, preview, peaks) // STRIAPERFLABELSHAPE (SR935): the one view::paint_in call (file tail), peaks
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
    fn proxy(&self) -> bool { true } // STRIAPROXYMEDIA (SR882): the preview decodes a source's attached proxy (file tail)
}

/// The frame tick `t` shows: `stria::nle::render` of `t..t+1` over the video tracks (the audio is not this face's).
/// `None` when nothing on the timeline sizes a frame there.
pub fn preview(tl: &Timeline, sources: &Sources, t: i64) -> Result<Option<Still>, RenderError> {
    let video = video_of(tl); // STRIAPERFMORE (SR974): the one video projection the still depends on (file tail)
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
pub const VERBS: [&str; 148] = ["play", "prev", "next", "split", "delete", "undo", "redo", "zoom-", "zoom+", "[", "]", "{", "}", "lift", "size", "save", "export", "frame", "in", "out", "kind", "ease", "slip-", "slip+", "slide-", "slide+", "ripple-all", "close-sync", "close-all", "preset", "export-skip", "export-clear", "export-list", "gap", "pip", "crop", "history", "recover", "snap", "meters-reset", "track-up", "track-down", "info", "loudness", "link", "unlink", "track+ video", "track+ audio", "track-", "headers", "close-gaps", "copy", "cut", "paste", "paste-insert", "paste-over", "tc", "xfade", "zoom-fit", "zoom-sel", "insert", "overwrite", "roll-", "roll+", "roll-tool", "mark", "unmark", "speed", "mute", "solo", "title", "gain-", "gain+", "gain", "fade-in", "fade-out", "track-gain", "opacity-", "opacity+", "bright-", "bright+", "contrast-", "contrast+", "sat-", "sat+", "normalize", "freeze", "pan-", "pan", "pan+", "proxy", "marker-", "marker+", "cut-", "cut+", "insert-marked", "overwrite-marked", "insert-all", "insert-marked-all", "target", "track-gain-", "track-gain+", "unlink-all", "link-av", "freeze-all", "title-size-", "title-size+", "title-align", "title-ink", "xfade-curve", "proxy-off", "proxy-all", "scale-", "scale+", "pip key", "pip unkey", "pip left", "pip right", "pip up", "pip down", "unmark all", "alone", "marker colour", "marker range", "rspeed", "marker marks", "source marker", "export markers", "src-prev", "src-next", "src-in", "src-out", "unload", "pan-key", "pan-unkey", "env", "env-", "env+", "env-del", "goto", "reverse", "dur-", "dur+", "env-curve", "opacity-key", "opacity-unkey", "seqrate", "help"]; // STRIAGAINENVCURVE (SR9083): `env-curve` toggles the selected clip's envelope curve (timeline_core's verb; the note in the file tail); STRIAGAINENVELOPE (SR9047): the selected clip's gain envelope point at the playhead (timeline_core's verb; the note in the file tail); STRIATRACKGAINSTEP (SR901): `track-gain-` / `track-gain+` step the selected clip's track one dB (timeline_core's verb; the message line in the file tail); STRIATRACKGAIN (SR864): `track-gain [<dB>]` the selected clip's track (timeline_core's `EditorState::verb`; bare = unity); STRIANORMALIZE (SR878): the selected clip's gain to its exact peak (file tail); STRIACLIPGAINFACE (SR838): the selected clip's gain / fades (timeline_core's `EditorState::verb`; file tail); STRIAOPACITYKEYSFACE (SR9085): `opacity-key` / `opacity-unkey` are transport buttons (the point at the playhead); STRIAOPACITYFACE (SR898): `opacity-` / `opacity+` step the selection's opacity 10 % (file tail); STRIAGRADEFACE (SR899): the six grade step words (timeline_core editor.rs tail); STRIAPANAUTOMATION (SR9050): `pan-key` / `pan-unkey` set / clear the selected clip's pan point at the playhead (timeline_core editor.rs tail); STRIAPANFACE (SR897): `pan-` / `pan` / `pan+` step / centre the selected clip's pan (file tail); STRIAPROXYBUTTONS (SR930): `proxy-off` / `proxy-all` are the buttons' words for `proxy off` / `proxy all` (file tail); STRIAPROXYMEDIA (SR882): `proxy` renders the selected clip's source at half size once and previews from it (file tail); STRIAEXPORTRANGE (SR9075): `export marker` / `export markers` queue the marker ranges as export-queue entries (file tail); STRIAMARKERRANGES (SR9052): `marker marks` sets the export marks to the playhead's marker's range (file tail); STRIAMARKERJUMPKEYSNEXT (SR984): `marker colour [<name>]` / `marker range` style the playhead's marker (file tail); STRIASPEEDREVERSE (SR9067): reverse (timeline_core's `EditorState::verb`); STRIAKDUR (SR9089): `dur-` / `dur+` step the clip's transition (file tail); STRIASEQFACE (SR9088): seqrate (file tail); NLEFACEEXPORT (SR694): `export [<dir>] [h264|png|i420|y4m|raw]` off the face's thread (file tail); NLETRANSITIONFACE (SR691): `kind` / `ease` cycle the selected transition (file tail); NLE5DEXPORTRANGE (SR717): `in` / `out` mark the export range (file tail); NLE3ESLIPFACE (SR737): one-frame slip / slide (file tail); NLE5DKEYSHELPBUTTON (SR765): help (file tail); STRIAEXPORTFRAME (SR830): `frame [<path>]` the playhead frame as a PNG (file tail); STRIATITLECLIP (SR828): title (file tail); STRIATRACKMUTESOLO (SR837): mute / solo the selected clip's track (timeline_core's verb), before help; STRIACLIPSPEED (SR829): speed (file tail); STRIAMARKERS (SR835): `mark [<name>]` / `unmark` at the playhead (file tail); STRIAROLLEDIT (SR851): one-frame roll of the cut at the selected clip's out edge (timeline_core editor tail); STRIAINSERTOVERWRITE (SR852): insert / overwrite at the playhead (timeline_core); STRIAZOOMTOFIT (SR859): zoom-fit / zoom-sel (file tail); STRIAAUDIOCROSSFADE (SR860): xfade puts / takes the selected audio clip's head crossfade (timeline_core editor.rs tail); STRIATIMECODEDISPLAY (SR872): `tc` cycles the playhead readout (file tail); STRIACOPYPASTE (SR854): copy / cut / paste of the selection at the playhead (timeline_core editor.rs); STRIAGAPCLOSE (SR857): `close-gaps` closes every gap on the selected track (timeline_core `EditorState::verb`; `gap <n>` an argument word, no button); STRIATRACKADDREMOVE (SR856): track add / remove (timeline_core's verb; file tail); STRIACLIPLINK (SR863): `link` / `unlink` the selection's clip group (timeline_core editor.rs tail); STRIATRACKREORDER (SR877): `track-up` / `track-down` swap the selected clip's track with its same-kind neighbour (timeline_core editor.rs file tail); STRIAMETERCLIPVERB (SR902): meters-reset clears every clip latch and peak hold (file tail); STRIAPASTEINSERT (SR896): paste-insert / paste-over (timeline_core editor.rs); STRIASNAPTOGGLE (SR892): snap flips the drag snap default (file tail); STRIAAUTOSAVE (SR883): `recover` loads the autosave (file tail); STRIAUNDOHISTORY (SR876): history (file tail); STRIAFREEZEFRAME (SR880): freeze (timeline_core `EditorState::verb`); STRIACROPFACE (SR900): `crop` clears the selection's crop (timeline_core editor.rs file tail); STRIAPOSITIONSCALE (SR879): `pip` (timeline_core editor.rs tail); STRIAMARKERJUMP (SR894): `marker-` / `marker+` / `cut-` / `cut+` jump the playhead to the previous / next marker / clip edge (timeline_core editor.rs tail); STRIAINSERTMARKS (SR905): insert / overwrite into the in / out marks (timeline_core); STRIALINKAUTO (SR911): `unlink-all` parts every link (timeline_core editor.rs tail); STRIAGAPSBLACK (SR909): bare `gap` opens one second at the playhead (before `close-gaps`; timeline_core `EditorState::verb`); STRIAEXPORTQUEUESKIP (SR914): export-skip / export-clear (file tail); STRIAFREEZELEN (SR916): `freeze [<len>]` / `freeze-all [<len>]` (every track ripples; timeline_core `freeze_words`); STRIAXFADECURVEBUTTON (SR925): xfade-curve swaps that crossfade's curve (timeline_core editor.rs tail); STRIALINKAUTOFUZZ (SR928): `link-av` links the V1 / A1 pairs, near ones too (timeline_core editor.rs tail); STRIAEXPORTQUEUELIST (SR932): export-list (file tail); STRIAXFORMPRESETS (SR938): `scale-` / `scale+` (timeline_core editor.rs tail); STRIAEXPORTPRESETSMORE (SR948): `preset` picks the face's export preset (file tail); STRIAROLLMORE (SR954): the roll tool on / off (file tail); STRIAMARKERSMORE (SR966): `unmark all` (file tail); STRIAGAPSMORE (SR969): `close-all` the Close All button's word for `close-gaps all` (before `gap`; file tail); STRIAINSERTMARKSKEYSNEXT (SR985): the all-tracks inserts and the target track (timeline_core); STRIAGAPSTHREE (SR9028): `close-sync` the Close Sync button's word for `close-gaps sync` (before `close-all`; file tail); STRIARIPPLETHREE (SR9034): the ripple-all tool on / off (file tail); STRIACLIPSPEEDTHREE (SR9031): rspeed (file tail); STRIATRACKSTHREE (SR9038): the header column (file tail); STRIAXFORMTHREE (SR9033): `pip left` / `pip right` / `pip up` / `pip down`, SR975's nudge words (timeline_core editor.rs tail); STRIASOURCEVIEWER (SR9045): the source monitor's words (timeline_core); STRIAXFORMKEYSNAV (SR9069): `pip key` / `pip unkey` (timeline_core editor.rs SR9049 tail); STRIATITLESTYLEFACE (SR9077): the title step words (timeline_core `title_step`); STRIADROPFRAMEFACE (SR9078): `goto <timecode|frames>` seeks the playhead (file tail)

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
    pub note: Option<String>, // STRIATRACKSMORE (SR955): a success line, read by `message_text` after `last_error`, cleared by the next taken input (`Face::on`)
    pub note: Option<String>, // STRIATRACKGAINTHREE (SR9024): a success line, read by message_text after last_error, cleared by `clear_note` (file tail)
    headers: Option<Option<usize>>, // STRIATRACKSTHREE (SR9038): the header column: None off; Some(the track bare `track-` targets) on (file tail)
    /// The shuttle rate in frames per clock tick (J/K/L; 1 for play).
    pub rate: i64,
    drag: Option<Drag>,
    drag_shown: DragShown, // STRIAPERFDRAGSHOWN (SR936): the drag preview, replayed only when the drag or its state changes
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
    help_query: Option<String>, // STRIAHELPSEARCH (SR874): `help <word>`'s query, the rows the help shows are THE help_rows filtered by it (file tail)
    proxy: Option<ProxyJob>, reel_paths: Vec<(String, std::path::PathBuf)>, proxy_queue: Vec<(timeline_core::SourceId, String)>, proxy_hold: Option<u64>, // STRIAPROXYOFF (SR915): the `proxy all` queue and the job's hold seam (file tail); STRIAPROXYMEDIA (SR882): the running proxy job and each reel's source file (file tail)
    pub note: Option<String>, // STRIAGAPSTHREE (SR9028): a success line on the message row, read after last_error, cleared by the next acting input (SR990's shape; file tail)
    hover_ticks: Option<((usize, usize), u16)>, loudness: Option<LoudnessJob>, note: Option<String>, pub note: Option<String>, // STRIATITLESTYLEFACE (SR9077): the message row's note, cleared by the next input (file tail); STRIAINFOREELHOVERDELAY (SR789): the hovered clip and the face ticks it has stood (file tail)
    frame_saved: Option<std::path::PathBuf>, // STRIAEXPORTFRAME (SR830): the file the last `frame` wrote (file tail)
    pub scan: NormalizeScan, pub note: Option<String>, // STRIANORMALIZELUFS (SR9054): a success line on the message row, read after last_error, cleared by the next acting input (file tail); STRIANORMALIZETHREE (SR9035): the normalize scan (synchronous, or off the face's thread under `serve`; file tail)
    sources_gen: u64, reloaded: Vec<(timeline_core::SourceId, u64)>, // STRIAPROXYWATCH (SR9079): the one sources generation and each source's last reload generation (file tail)
    peaks: WavePeaks, // STRIAWAVEFORM (SR827): each audio source's peak table, scanned once at the first paint that shows it (file tail)
    tc: Option<TcUnit>, // STRIATIMECODEDISPLAY (SR872): the playhead readout's unit on the message row's left half (`None`: off, the default) (file tail)
    slip_view: Option<SlipView>, // STRIASLIPVIEW (SR858): a slip drag's first / last frame pair, cached and throttled (file tail)
    meter_hold: MeterHold, // STRIAMETERSCLIPHOLD (SR862): the meters' 2 s peak hold and the clip latch (file tail)
    info: Option<SourceInfo>, // STRIASOURCEINFO (SR875): the `info` word's standing line and its face ticks left (file tail)
    export_queue: std::collections::VecDeque<ExportRequest>, // STRIAEXPORTQUEUE (SR881): export requests waiting behind the running job, started in order from export_tick (file tail)
    snap_off: bool, // STRIASNAPTOGGLE (SR892): the drag snap default is off (the `snap` word flips it; Alt at the press inverts it) (file tail)
    autosave: Autosave, // STRIAAUTOSAVE (SR883): what `<stem>.autosave.edl` holds and the edits / ticks since it was written (file tail)
    keymap: Keymap, // STRIAKEYBOARDMAP (SR886): the key overlay over KEYS, consulted first ([`Face::with_keys`], file tail)
    zoom_fit: Option<(bool, (u32, u32), Option<(i64, i64)>)>, // STRIAZOOMFITKEYS (SR907): the last fit (sel, zoom, view); STRIAZOOMFITTHREE (SR9040): the zoom is the scale `(zoom, squeeze)`; a resize refits while it still holds (file tail)
    pcache: PreviewCache, // STRIAPERFPREVIEWCACHE (SR918): the preview still's key and the reel summaries (file tail)
    laid: Option<String>, // STRIAINSERTMARKSKEYS (SR924): the last laid insert's line (file tail)
    lcache: LabelCache, // STRIAPERFLABELSHAPE (SR935): the shaped label lines by text (file tail)
    xform_note: Option<String>, // STRIAXFORMPRESETS (SR938): the placement line after a pip / scale word, until the next message (file tail)
    export_preset: Option<&'static crate::nle_export::ExportPreset>, preset_note: Option<String>, // STRIAEXPORTPRESETSMORE (SR948): the picked entry of THE preset table a bare `export` takes, and the `preset` word's success note (cleared by the next taken input) (file tail)
    roll_tool: bool, // STRIAROLLMORE (SR954): a trim-handle drag rolls its cut (file tail)
    slip_hold: Option<SlipHold>, // STRIASLIPVIEWMORE (SR946): a word slip / slide's pair held until the next action (file tail)
    crop_drag: Option<view::CropDrag>, // STRIACROPMORE (SR960): the live crop drag on the preview picture (file tail); STRIAUNDOMORE (SR973): the message row's note, read after last_error (file tail)
    pub note: Option<String>, // STRIASNAPNEXT (SR990): a success line on the message row, read after last_error, cleared by the next acting input (file tail)
    pub note: Option<String>, // STRIAXFADEMORE (SR980): a success reading on the message row (not an error), cleared by the next acting input (file tail)
    pub note: Option<String>, // STRIATRANSITIONNEXT (SR991): the message row's success note, after `last_error`; cleared by `clear_note` (file tail)
    /// STRIAOPACITYNEXT (SR993): a success line for the message row ([`Face::message_text`] after `last_error`), cleared by the next taken input ([`Face::clear_note`]).
    pub note: Option<String>,
    pub note: Option<String>, // STRIAPANNEXT (SR994): a success line, read by message_text after last_error, cleared by the next taken input (file tail)
    pub note: Option<String>, // STRIAMARKERJUMPKEYSNEXT (SR984): a success line on the message row, read after last_error, cleared by the next acting input (file tail)
    pub note: Option<String>, // STRIAPANNEXT (SR994): a success line, read by message_text after last_error, cleared by the next taken input (file tail)
    watched: Vec<(String, String)>, // STRIAPROXYTHREE (SR9026): each reel's file stamp as last opened (file tail)
    ripple_all: bool, // STRIARIPPLETHREE (SR9034): a ripple drag runs on every track (file tail)
    xform_drag: Option<view::XformDrag>, // STRIAXFORMTHREE (SR9033): the live placement drag on the preview picture (file tail)
    entry: Option<String>, // STRIAGOTOMORE (SR9097): the open entry line's text (`None`: closed, the default) (file tail)
}

impl Face {
    /// `labels`: name the clips through the host font stack (off for a golden).
    pub fn new(state: EditorState, sources: Sources, canvas: (u32, u32), order: PixelOrder, labels: bool) -> Face {
        let labels = labels.then(|| TextStyle::new(&["sans"], 11.0));
        Face { clean: state.timeline.clone(), doc_path: None, state, sources: std::sync::Arc::new(sources), export: None, canvas, order, playing: false, labels, still: None, last_error: None, note: None, note: None, rate: 1, drag: None, drag_shown: DragShown::default(), pointer: None, buttons: false, wheel: 0.0, view: None, trans_still: None, meters: MixMeters::default(), surface: None, split: None, play: PlayAudio::default(), help: false, hover_ticks: None, frame_saved: None, peaks: WavePeaks::default(), tc: None, slip_view: None, loudness: None, meter_hold: MeterHold::default(), help_query: None, info: None, export_queue: std::collections::VecDeque::new(), snap_off: false, autosave: Autosave::default(), keymap: Keymap::default(), proxy: None, reel_paths: Vec::new(), zoom_fit: None, pcache: PreviewCache::default(), proxy_queue: Vec::new(), proxy_hold: None, laid: None, lcache: LabelCache::default(), xform_note: None, export_preset: None, preset_note: None, roll_tool: false, slip_hold: None, crop_drag: None, note: None, note: None, note: None, note: None, note: None, note: None, note: None, note: None, watched: Vec::new(), scan: NormalizeScan::default(), note: None, ripple_all: false, headers: None, xform_drag: None, note: None, note: None, sources_gen: 0, reloaded: Vec::new(), entry: None }
    }
    /// The preview frame of the last [`Face::frame`].
    pub fn still(&self) -> Option<&Still> {
        self.still.as_ref()
    }
    /// One message from the surface or the buttons; true when the canvas must be repainted.
    pub fn on(&mut self, msg: &SMessage) -> bool {
        if let Some(r) = self.bin_input(msg) { return r; } if let Some(r) = self.preview_meter_input(msg) { return r; } if let Some(r) = self.crop_drag_on(msg) { return r; } if let Some(r) = self.xform_drag_on(msg) { return r; } if let Some(r) = self.preview_history_input(msg) { return r; } if let Some(r) = self.source_ruler_input(msg) { return r; } if let Some(r) = self.source_drag_input(msg) { return r; } if !self.takes(msg) { return false; } self.laid_clear(msg); self.xform_note = None; self.preset_note = None; self.note = None; self.clear_note(msg); self.clear_note(msg); self.clear_note(msg); self.clear_note(msg); self.clear_note(msg); self.clear_note(msg); self.clear_note(msg); self.clear_note(msg); self.watch_sources(msg); self.clear_note(msg); self.clear_note(msg); self.clear_note(msg); if let Some(r) = self.on_preview_resize(msg) { return r; } if let Some(r) = self.help_input(msg) { return r; } if let Some(r) = self.clipboard_key(msg) { return r; } if let Some(r) = self.help_type_input(msg) { return r; } if let Some(r) = self.history_pick(msg) { return r; } if let Some(r) = self.entry_input(msg) { return r; } // STRIAPASTEKEYS (SR921): BrowserCopy / BrowserPaste reach Face::key as the Ctrl+c / Ctrl+v chords (file tail); NLE2-SPLIT (SR339): the preview Surface's resize; SURFACEID (SR332): another Surface's pointer is not this face's; STRIAPROJECTBIN (SR885): the bin Surface's pointer first (file tail); STRIATRACKGAINTHREE (SR9024): an acting input clears the note (file tail); STRIATITLESTYLEFACE (SR9077): an input clears the note (file tail); STRIASOURCEDRAG (SR9081): the split preview Surface's source drag (file tail)
        match msg {
            SMessage::Input { target, text } if target == SURFACE => self.verb(text), SMessage::ConsoleInput(q) => self.help_field(q), // STRIAHELPMORE (SR977): the bar's help query field, its Enter (file tail)
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
    fn verb_unnamed(&mut self, v: &str) -> bool {
        if let Some(r) = self.env_curve_face_verb(v) { return r; } if let Some(r) = self.transition_step_note(v) { return r; } if let Some(r) = self.env_face_verb(v) { return r; } if let Some(r) = self.normalize_lufs_verb(v) { return r; } if let Some(r) = self.opacity_key_word(v) { return r; } if let Some(r) = self.target_verb(v) { return r; } if let Some(r) = self.proxy_words(v) { return r; } if let Some(r) = self.opacity_word(v) { return r; } if let Some(r) = self.meter_reset_verb(v) { return r; } if let Some(r) = self.normalize_verb(v) { return r; } if let Some(r) = self.proxy_verb(v) { return r; } if let Some(r) = self.gaps_note_verb(v) { return r; } if let Some(r) = self.track_order_word(v) { return r; } if let Some(r) = self.laid_verb(v) { return r; } if let Some(r) = self.xform_note_verb(v) { return r; } if let Some(r) = self.preset_verb(v) { return r; } if let Some(r) = self.transition_len_verb(v) { return r; } if let Some(r) = self.track_flag_face_verb(v) { return r; } if let Some(r) = self.headers_verb(v) { return r; } if let Some(r) = self.seqrate_verb(v) { return r; } if let Some(r) = self.transition_verb(v) { return r; } if let Some(r) = self.info_verb(v) { return r; } if let Some(r) = self.save_verb(v) { return r; } if let Some(r) = self.snap_verb(v) { return r; } if let Some(r) = self.export_marker_verb(v) { return r; } if let Some(r) = self.export_verb(v) { return r; } if let Some(r) = self.mark_verb(v) { return r; } if let Some(r) = self.frame_verb(v) { return r; } if let Some(r) = self.clip_audio_verb(v) { return r; } if let Some(r) = self.marker_verb(v) { return r; } if let Some(r) = self.fit_verb(v) { return r; } if let Some(r) = self.tc_verb(v) { return r; } if let Some(r) = self.nudge_verb(v) { return r; } if let Some(r) = self.track_remove_verb(v) { return r; } if let Some(r) = self.loudness_verb(v) { return r; } if let Some(r) = self.help_search_verb(v) { return r; } if let Some(r) = self.autosave_verb(v) { return r; } if let Some(r) = self.pan_face_verb(v) { return r; } if let Some(r) = self.track_gain_face_verb(v) { return r; } if let Some(r) = self.export_queue_verb(v) { return r; } if let Some(r) = self.jump_verb(v) { return r; } if let Some(r) = self.roll_tool_verb(v) { return r; } if let Some(r) = self.marker_style_verb(v) { return r; } if let Some(r) = self.ripple_all_verb(v) { return r; } if let Some(r) = self.marker_marks_verb(v) { return r; } if let Some(r) = self.goto_verb(v) { return r; } if let Some(r) = self.tc_amount_verb(v) { return r; } // STRIANORMALIZE (SR878): the normalize word (file tail); STRIACLIPGAINFACE (SR838): the gain / fade words and their message (file tail); STRIATRACKGAINSTEP (SR901): the track gain words and their message (file tail); STRIAOPACITYKEYS (SR9048): `opacity-key` / `opacity-unkey` and their note (file tail); STRIAOPACITYFACE (SR898): the opacity step words name the percent (file tail); STRIAPANFACE (SR897): the pan words and their readout (file tail); STRIATRACKORDERKEYS (SR922): the track words name the move (file tail); STRIAEXPORTRANGE (SR9075): `export marker(s)` before `export` takes the words (file tail); STRIAMARKERRANGES (SR9052): `marker marks` (file tail); STRIAMARKERJUMPKEYSNEXT (SR984): `marker colour` / `marker range` (file tail); STRIASEQFACE (SR9088): the `seqrate` word (file tail); NLEHOSTSAVE (SR677): the save verb (file tail); STRIATRACKADDREMOVE (SR856): `track-` names what went (file tail); STRIAGAPSBLACK (SR909): the close-gaps note of removed black fades (file tail); STRIAINSERTMARKSKEYSNEXT (SR985): `target` (file tail); STRIATRACKGAINTHREE (SR9024): mute / solo and their note (file tail); STRIATRACKSTHREE (SR9038): `headers` (file tail); STRIAGAINENVELOPE (SR9047): the env words and their note (file tail); STRIAGAINENVCURVE (SR9083): `env-curve` and its note (file tail)
        let r = match v {
            "play" => {
                self.playing = !self.playing;
                self.rate = 1; self.sync_audio(); // NLEPLAYAUDIO (SR676): the device follows the transport
                Ok(())
            }
            "help" => { self.help = !self.help; self.help_query = None; Ok(()) } // NLE5DKEYSHELP (SR744): the keys help overlay on / off (file tail)
            "rspeed" => rspeed_cycle(&mut self.state), v if speed_word(v).is_some() => speed_verb(&mut self.state, v), // STRIACLIPSPEEDTHREE (SR9031): bare `rspeed` cycles the same speeds with a ripple (file tail); STRIACLIPSPEED (SR829): `speed` cycles 1x / 2x / 4x, `speed <q>` sets it (file tail)
            v if title_text(v).is_some() => add_title(&mut self.state, &timeline_core::editor::title_lines(title_text(v).unwrap_or("Title"))), // STRIATITLECLIP (SR828): `title [<text...>]` lays a 5 s title card at the playhead (file tail); STRIAFREEZETHREE (SR9029): a typed `\n` is a line break
            v if slipslide_word(v).is_some() => self.slip_word(v), // STRIASLIPVIEWMORE (SR946): the same call, then a slip / slide holds its pair (file tail); NLE3ESLIPFACE (SR737): the one-frame words onto SR704's `slip <d>` / `slide <d>` (file tail); NLE3ESLIPSTEP (SR752): and the SLIP_STEP words `slip-10` ... (keys only)
            _ => match self.state.verb(v) {
                Some(r) => r, // NLE6KEYS (rmbp-ledger B961): timeline_core's ONE verb dispatch (the kernel Player's keys call it too)
                None => return false,
            },
        };
        if r.is_ok() { if let Some(s) = self.state.xfade_reading(v) { self.note = Some(format!("xfade: {s}")); } } if r.is_ok() { self.title_style_note(v); } if let Err(e) = r { // STRIAXFADECURVEBUTTON (SR925): after any xfade word the message row reads the crossfade (`xfade: 12 equal`) // STRIATITLESTYLEFACE (SR9077): a title step word's note (file tail)
            self.last_error = Some(match self.state.xfade_refusal(v, e).or_else(|| self.state.xfade_curve_refusal(v, e)) { Some(s) => format!("{v}: {s}"), None => format!("{v}: {e}") }); // STRIAXFADECURVE (SR904): an `xfade` refusal's audio wording (timeline_core editor.rs tail); STRIAXFADECURVEBUTTON (SR925): and `xfade-curve`'s
        }
        true
    }
    /// A click on the surface (buffer pixels): the ruler scrubs, a clip is selected and scrubbed to, an empty track
    /// spot scrubs and deselects.
    fn click(&mut self, x: f64, y: f64) -> bool {
        if let Some(r) = self.meter_clip_reset(x, y) { return r; } if let Some(r) = self.pick_header(x, y) { return r; } if let Some(r) = self.pick_transition(x, y) { return r; } // STRIATRACKSTHREE (SR9038): a header names bare `track-`'s track (file tail)
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
        self.click_burst(); true // STRIAAUDIOSCRUBMORE (SR947): a clip / track / ruler click sounds its frame (file tail)
    }
    /// One playback tick: the playhead steps `rate` frames (back when negative); playback stops at the end going
    /// forward and at 0 going back. True when it moved or stopped.
    pub fn tick(&mut self) -> bool {
        if (self.info_tick() | (self.hover_dwell_tick()) | self.scrub_retry_tick()) && !self.playing { return true; } // STRIAAUDIOSCRUBNEXT (SR988): a refused scrub step retried on the same clock (file tail); STRIAINFOREELHOVERDELAY (SR789): the hover dwell counts on THE face clock; the crossing tick repaints (file tail)
        if !self.playing { self.meter_hold_tick();
            return false;
        }
        if let Some(r) = self.source_tick() { return r; } let ph = self.state.playhead; // STRIASOURCEPLAY (SR9080): a loaded source plays through this same body over its reel (file tail)
        if (self.rate > 0 && ph >= self.state.timeline.end()) || (self.rate <= 0 && ph <= 0) {
            self.playing = false; self.sync_audio(); // NLEPLAYAUDIO (SR676)
            return true;
        }
        if !self.play_tick(ph) { self.feed_meters(ph, self.rate); self.state.nudge(self.rate); } self.meter_hold_tick(); // NLEPLAYAUDIO (SR676): a forward played tick is ONE render, shown at its own playhead
        !std::mem::take(&mut self.play.pace.held) // STRIAPLAYSYNCPACE (SR9055): a held tick (the still waits for the AudioClock) asks no repaint (file tail)
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
    use tokio::sync::broadcast::error::RecvError; face.decode_off_thread(); // STRIATHUMBNAILSNEXT (SR997): the served face decodes heads and peak tables off its thread (file tail)
    face.scan.thread = true; for m in face.frames() { bus.fire(m); }
    let fps = face.state.timeline.fps().unwrap_or(25).max(1);
    let mut clock = tokio::time::interval(Duration::from_nanos(clock_ns(face.state.timeline.tb, fps))); clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip); // STRIAINFOREELHOVERDELAY (SR789): a clock idle under its guard skips the missed ticks (no catch-up burst); STRIATCTHREE (SR9030): `clock_ns` plays an NTSC base at its real rate (file tail)
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
            _ = clock.tick(), if face.playing || face.exporting() || face.hover_pending() || face.normalizing() || face.autosave_pending() || face.info_pending() || face.meter_hold_pending() || face.loudness_pending() || face.slip_due() || face.thumbs_pending() || face.proxying() || face.scrub_pending() => { // STRIATHUMBNAILS (SR839): and heads left undecoded; STRIAPROXYMEDIA (SR882): and a running proxy job; STRIAAUDIOSCRUBNEXT (SR988): and a refused scrub step owed its sound; STRIAINFOREELHOVERDELAY (SR789): and a counting hover dwell; NLEFACEEXPORT (SR694): the clock also paints a running export's band
                if face.tick() | face.export_tick() | face.thumbs_pending() | face.slip_tick() | face.loudness_tick() | face.autosave_tick() | face.proxy_tick() | face.normalize_tick() | face.normalize_progress() { // STRIATHUMBNAILS (SR839): an idle face paints its remaining heads on the clock // STRIAPROXYMEDIA (SR882): a finished proxy attaches and repaints // STRIANORMALIZELUFSMORE (SR9086): a running clip measure's progress line (file tail)
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
    ("Shift+Left", "nudge-10"), // STRIANUDGE (SR855): Shift with the arrows nudges the selection SLIP_STEP frames
    ("Shift+Right", "nudge+10"),
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
    ("Shift+Meta+less", "slide-10"), // STRIAROLLNEXT (SR999): GTK names the chord Ctrl+ Alt+ Shift+ Meta+ in that order
    ("Ctrl+Shift+greater", "slide+10"),
    ("Shift+Meta+greater", "slide+10"),
    ("m", "mark"), // STRIAMARKERS (SR835): drop a marker at the playhead (text key, the default name) // STRIAROLLNEXT (SR999): GTK's order
    ("(", "roll-"), (")", "roll+"), ("Ctrl+Shift+parenleft", "roll-10"), ("Shift+Meta+parenleft", "roll-10"), ("Ctrl+Shift+parenright", "roll+10"), ("Shift+Meta+parenright", "roll+10"), // STRIAROLLKEYS (SR893): ( / ) roll the cut at the selected clip's out edge one frame; Ctrl / Cmd + Shift + ( / ) SLIP_STEP frames; STRIAROLLMORE (SR954): GTK names the chord Ctrl+ Alt+ Shift+ Meta+ in that order
    ("r", "roll-tool")
    ("t", "tc"), // STRIATCSET (SR908): `t` (text) cycles the playhead readout through the `tc` word (file tail)
    ("\\", "zoom-fit"), ("|", "zoom-sel"), // STRIAZOOMFITKEYS (SR907): `\` / `|` (Shift+\, text) fit the timeline / the selection (Shift+Z is redo)
    ("Ctrl+c", "copy"), ("Meta+c", "copy"), ("Ctrl+x", "cut"), ("Meta+x", "cut"), ("Ctrl+v", "paste"), ("Meta+v", "paste"), ("Ctrl+Shift+V", "paste-insert"), ("Meta+Shift+V", "paste-insert"), // STRIAPASTEKEYS (SR921): the clipboard words as chords (GTK / AppKit send Ctrl / Cmd+c as BrowserCopy and Ctrl / Cmd+v as BrowserPaste: Face::on routes them here, file tail)
    ("Alt+Up", "track-up"), ("Alt+Down", "track-down"), // STRIATRACKORDERKEYS (SR922): Alt with Up / Down (GTK's names) move the selected clip's track within its kind
    ("Up", "cut-"), // STRIAMARKERJUMPKEYS (SR923): Up / Down jump to the previous / next clip edge, ; / ' to the previous / next marker (text keys)
    ("Down", "cut+"),
    ("\u{f700}", "cut-"),
    ("\u{f701}", "cut+"),
    (";", "marker-"),
    ("'", "marker+"),
    ("w", "insert"), // STRIAINSERTMARKSKEYS (SR924): w / d insert / overwrite at the playhead, W / D (Shift, text) into the in / out marks
    ("d", "overwrite"),
    ("W", "insert-marked"),
    ("D", "overwrite-marked"),
    ("c", "xfade"), // STRIAXFADECURVEBUTTON (SR925): c / C (Shift+c, text) put / take the crossfade and swap its curve (x is the kernel's delete, X the mark clear)
    ("C", "xfade-curve"),
    ("v", "gain-"), ("V", "gain+"), (":", "track-gain-"), ("\"", "track-gain+"), ("Q", "mute"), ("S", "solo"), // STRIATRACKGAINKEYS (SR926): text keys for the audio words (Face::verb: the buttons' commit and message line; the natural - / = / m / s are taken)
    ("h", "history"), ("Ctrl+i", "info"), // STRIASOURCEINFOKEY (SR942): Ctrl / Cmd+I (the platform's Get Info chord) stands the selected clip's reel line (`i` / `I` are the marks)
    ("Meta+i", "info"),
    ("n", "snap"), // STRIASNAPMORE (SR952): n flips the drag snap default through the snap word (text key)
    ("1", "bright-"), ("2", "bright+"), ("3", "contrast-"), ("4", "contrast+"), ("5", "sat-"), ("6", "sat+"), // STRIAGRADEMORE (SR957): the digits step the selected clip's grade (SR899's VERBS words, text keys)
    ("8", "opacity-"), ("9", "opacity+"), // STRIAOPACITYMORE (SR958): 8 / 9 (text keys) step the selection's opacity through the VERBS words
    ("Alt+Left", "pan-"), ("Alt+0", "pan"), ("Alt+Right", "pan+"), // STRIAPANMORE (SR959): Alt with Left / 0 / Right step / centre the selected clips' pan (SR897's face words; GTK names, AppKit owed)
    ("Ctrl+Shift+M", "meters-reset"), ("Meta+Shift+M", "meters-reset"), ("!", "meters-reset"), // STRIAKMETERRESET (SR9101): `!` (text) is the word too, the kernel Player's byte. STRIAMETERSMORE (SR962): Ctrl / Cmd + Shift + M is the meters-reset word (GTK keeps the shifted keyval's Shift+), // STRIAROLLMORE (SR954): r (text) turns the roll tool on / off (file tail)
    ("Alt+m", "unmark all"), // STRIAMARKERSMORE (SR966): every marker removed (one commit)
    ("Alt+l", "crop-l+"), // STRIACROPMORE (SR960): Alt+side crops a side in, Alt+Shift+SIDE out (SR900's step words; GTK names, no Meta twin)
    ("Alt+Shift+L", "crop-l-"),
    ("Alt+t", "crop-t+"),
    ("Alt+Shift+T", "crop-t-"),
    ("Alt+r", "crop-r+"),
    ("Alt+Shift+R", "crop-r-"),
    ("Alt+b", "crop-b+"),
    ("Alt+Shift+B", "crop-b-"),
    ("y", "link-av"), ("Y", "unlink"), ("Ctrl+l", "link"), ("Meta+l", "link"), ("Ctrl+Shift+L", "unlink-all"), ("Shift+Meta+L", "unlink-all"), // STRIALINKAUTOTHREE (SR9039): the four link words' keys (text y / Y, the Player's letters; GTK orders a chord Ctrl, Alt, Shift, Meta)
    ("A", "ripple-all"), // STRIARIPPLETHREE (SR9034): A (Shift+a, text) turns the ripple-all tool on / off (file tail)
    ("p", "pip key"), ("P", "pip unkey"), // STRIAKXFORMKEYS (LEDGER SR9073): p / P (text) set / remove the placement point at the playhead, the kernel Player's EDIT_KEYS p / P too
    ("Alt+s", "source"), // STRIASOURCEPLAY (SR9080): the source monitor's words as Alt chords (GTK's spelling)
    ("Alt+u", "unload"),
    ("Alt+Left", "src-prev"),
    ("Alt+Right", "src-next"),
    ("Alt+i", "src-in"),
    ("Alt+o", "src-out"),
    ("_", "dur-"), ("*", "dur+"), // STRIAKDUR (SR9089): _ / * (text) the kernel Player's bytes for the transition step
    ("Alt+e", "env"), ("Alt+Shift+E", "env-del"), ("Alt+Down", "env-"), ("Alt+Up", "env+"), // STRIAGAINENVDRAG (SR9082): chords for the env words (GTK order Ctrl, Alt, Shift, Meta; Face::verb: the note)
    ("Ctrl+g", "enter goto"), // STRIAGOTOMORE (SR9097): Ctrl / Cmd+g opens the entry line holding `goto `, Ctrl / Cmd+Return opens it empty (file tail)
    ("Meta+g", "enter goto"),
    ("Ctrl+Return", "enter"),
    ("Meta+Return", "enter"),
    ("a", "reverse"), // STRIAKREVERSE (SR9095): `a` (text) is the word `reverse` (VERBS), the kernel Player's `a` too (vplay.rs EDIT_KEYS)
    ("?", "help"), // NLE5DKEYSHELP (SR744): `?` (Shift+/, text) shows the keys help over the timeline band (file tail)
    ("E", "export-skip"), // STRIAEXPORTQUEUELIST (SR932): Shift+e (text, both faces) skips the running export through the `export-skip` word; STRIAUNDOJUMP (SR934): `h` lists the undo history
    ("Shift+Escape", "export-skip"), // STRIAEXPORTQUEUETHREE (SR9042): GTK's Shift+Esc skips the running export (AppKit drops Escape; `E` is its key)
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
        Layout::of_role(self.pointer_role(), w, h, &self.state).anchor(self.view, self.state.playhead) // STRIAZOOMFITMORE (SR965): THE anchor rule, timeline_core's (the Player's view band reads it too)
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
    /// (Shift adds to the set, Alt / Option turns snapping off; STRIARIPPLETRIM SR850: Shift on the selected clip's trim handle ripples the trim, so a shift-drag begun on that handle no longer moves the set).
    fn press(&mut self, x: f64, y: f64, mods: u8) -> bool {
        self.buttons = true;
        self.pointer = Some((x, y)); if self.snap_off && view::snap_badge_hit(&self.layout(), x, y) { self.drag = None; return self.verb("snap"); } // STRIAKSNAPSHOW (SR9104): a press on the snap-off badge turns snap on, no scrub (timeline_core view.rs tail)
        if self.meter_clip_at(x, y).is_some() { return false; } if let Some(r) = self.press_source(x, y, mods) { return r; } if let Some(r) = self.press_grab // STRIASOURCEDRAG (SR9081): a press on the loaded source's preview band begins the source drag (file tail)(x, y, mods) { return r; } // STRIAKGAINENV (SR9111): timeline_core's ONE press order before begin_with: the transition edge, the widget pick, the envelope (file tail)
        let l = self.layout();
        let dropped = self.drag.take().is_some();
        let (sel, set, ph) = (self.state.selection, self.state.set.clone(), self.state.playhead);
        let (shift, nosnap) = (mods & bandy::MOD_SHIFT != 0, view::press_nosnap(mods & bandy::MOD_ALT != 0, self.snap_off));
        self.drag = Drag::begin_with(&mut self.state, l, x, y, shift, nosnap, mods & (bandy::MOD_CONTROL | bandy::MOD_SUPER) != 0).map(|d| d.rolled(&self.state, self.roll_tool)).map(|d| d.synced(self.ripple_all)); // STRIAROLLMORE (SR954): the roll tool turns a trim-handle drag into a roll of its cut // STRIARIPPLETHREE (SR9034): the ripple-all tool turns a ripple drag into the all-tracks ripple
        if let Some(d) = self.drag.filter(|d| d.kind == DragKind::Scrub) {
            self.state.seek(d.origin_tick); self.scrub_burst(ph); // STRIAAUDIOSCRUB (SR865): a ruler scrub step plays its frame through the play road (file tail)
        }
        dropped || self.state.selection != sel || self.state.set != set || self.state.playhead != ph
    }
    /// The button came up: a drag that moved commits once; otherwise it is the click it always was.
    fn release(&mut self, x: f64, y: f64) -> bool {
        self.pointer = Some((x, y));
        let Some(mut d) = self.drag.take() else { return if view::snap_badge_hit(&self.layout(), x, y) { true } else { self.click(x, y) } }; // STRIAKSNAPSHOW (SR9104): the badge press's release is no ruler click (a plain ruler press is a Scrub drag)
        d.update(&self.state, x, y);
        if !d.moved() {
            return if d.additive { d.release_still(&mut self.state); true } else { self.click(x, y) }; // STRIAMULTISELECTMORE (SR950): a shift-click on a set member takes it out (timeline_core view.rs `Drag::release_still`)
        }
        if d.kind == DragKind::Scrub {
            self.state.seek(d.origin_tick + d.delta);
            return true;
        }
        let r = self.finish_drag(d); self.pan_drag_note(d.kind, &r); if let Err(e) = r { // STRIAPANNEXT (SR994): a committed pan drag reads its pan on the note (file tail) // STRIATRANSITIONKERNEL (SR9057): an edge drag's commit sets the length note (file tail)
            self.last_error = Some(format!("drag: {e}"));
        }
        true
    }
    /// The pointer moved: a live drag follows it.
    fn hover(&mut self, x: f64, y: f64) -> bool {
        let hc = self.xform_hover_corner(); if let Some(r) = self.hover_message(x, y) { return r || self.xform_hover_corner() != hc; } // STRIAXFORMHOVER (SR9107): a bare move repaints when the hovered placement handle changed (file tail); STRIAINFOREELHOVER (SR762): no drag: the pointer moves and the message follows the clip under it (file tail)
        let Some(d) = self.drag.as_mut() else { return false };
        let before = (d.delta, d.to_track, d.at, d.guide);
        d.update(&self.state, x, y);
        let changed = match d.kind {
            DragKind::Marquee | DragKind::GainEnv => d.at != before.2, // STRIAGAINENVDRAG (SR9082): a vertical envelope drag repaints
            _ => (d.delta, d.to_track, d.guide) != (before.0, before.1, before.3),
        };
        if let Some(t) = d.scrub_tick() { // STRIAKSCRUB (SR9060): the scrub step's tick is timeline_core's `Drag::scrub_tick`, the one the kernel Player reads
            let p = self.state.playhead; self.state.seek(t); self.scrub_burst(p); // STRIAAUDIOSCRUB (SR865): each tick-changing scrub step plays its frame (file tail)
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
        let z0 = self.state.scale(); // STRIAZOOMFITTHREE (SR9040): the scale, so a squeeze step counts
        self.state.zoom_by(step);
        if self.state.scale() == z0 {
            return false;
        }
        let t0 = (t - (x - self.layout().left as f64).max(0.0) as i64 * self.state.squeeze.max(1) as i64 / self.state.zoom as i64).max(0); // STRIAZOOMFITTHREE (SR9040): the squeezed px -> ticks
        self.view = Some((t0, self.state.playhead));
        true
    }
    /// A key ([`KEYS`]); `text` says it came as `BrowserText`. False for a key the face does not take.
    fn key(&mut self, k: &str, text: bool) -> bool {
        if (k.chars().count() == 1) != text {
            return false;
        }
        let Some(act) = self.bound(k) else { return false }; // STRIAKEYBOARDMAP (SR886): the face's key overlay first, then KEYS (file tail)
        if self.help { self.help = false; return true; } if act == "help" { return self.verb(act); } if let Some(r) = self.entry_open(act) { return r; } // NLE5DKEYSHELP (SR744): a taken key while the help shows only dismisses it (not applied); `?` shows it through the one verb
        if VERBS.contains(&act) || slipslide_word(act).is_some() || act.starts_with("crop-") || matches!(act, "pip key" | "pip unkey") { // STRIACROPMORE (SR960): the crop chords' SR900 step words; STRIAKXFORMKEYS (LEDGER SR9073): the key words through the face's one verb road; NLE3ESLIPSTEP (SR752): the 10-frame words are keys, not VERBS: the same road
            return self.verb(act);
        }
        let r = match act {
            "shuttle+" => {
                self.rate = timeline_core::editor::shuttle_rate("shuttle+", self.playing, self.rate, SHUTTLE_MAX); // STRIAKSHUTTLE (SR9061): the ONE ladder both faces call (timeline_core editor.rs tail)
                self.playing = true; self.sync_audio(); // NLEPLAYAUDIO (SR676)
                Ok(())
            }
            "shuttle-" => {
                self.rate = timeline_core::editor::shuttle_rate("shuttle-", self.playing, self.rate, SHUTTLE_MAX); // STRIAKSHUTTLE (SR9061): the ONE ladder both faces call (timeline_core editor.rs tail)
                self.playing = true; self.sync_audio(); // NLEPLAYAUDIO (SR676)
                Ok(())
            }
            "stop" => {
                self.playing = false;
                self.rate = 1; self.sync_audio(); // NLEPLAYAUDIO (SR676): the device follows the transport
                Ok(())
            }
            "mark-in" | "mark-out" | "home" | "end" | "nudge-" | "nudge+" | "mark in" | "mark out" | "mark clear" => self.state.verb(act).unwrap_or(Err(EditError::NoClip)), // NLE6KEYS: the ONE dispatch; NLE5DRANGEKEYS (SR735): the export marks
            "cancel" => return self.drag.take().is_some() || self.cancel_export() || self.cancel_loudness() || self.cancel_proxy() || self.cancel_normalize(), // STRIAPROXYOFF (SR915): then a running proxy job (file tail); STRIANORMALIZELUFSMORE (SR9086): else the running clip measure (file tail); NLEFACEEXPORT (SR694): a live drag first, else the running export
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

pub use timeline_core::view::{METER_BG, METER_PEAK, METER_RMS, METER_VIEW_DB, METER_W}; // STRIAKMETER (SR9058): the strip's width, floor and colours live in timeline_core::view, both faces paint them (file tail)

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
         || self.scrub_metered()self.meter_hold.strip(self.playing, self.meters.levels()) // STRIAKMETER (SR9058): timeline_core's playing-or-nothing rule, both faces (file tail) // STRIAAUDIOSCRUBMORE (SR947): and the last sounded step while a Scrub drag lives (file tail)
    }
}

/// The meter strip over a painted canvas: each audio track's bar inside its row, the master's down the preview band, at
/// x `w - METER_W - 1 .. w - 1`; the RMS filled from the bottom, the peak a 1 px line, both linear in dB over
/// [`METER_VIEW_DB`]..0. No levels paint nothing.
fn paint_meters(px: Vec<u8>, l: Layout, order: PixelOrder, levels: &[MeterLevel]) -> Vec<u8> {
    view::paint_meters(px, l, vo(order), levels)view::db_frac,;/STRIACLIPGAINFACE (SR838: the ONE dB maplifted to timeline_core view // STRIAKMETER (SR9058): the one strip, timeline_core::view (file tail)
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
    pub bin: Option<BinSurface>, // STRIAPROJECTBIN (SR885): the source bin's Surface ([`Face::with_bin`], file tail)
}

impl Face {
    /// Split this face over two Surfaces of its tetra tree: `preview` ([`PREVIEW_SURFACE`]) and `tracks`
    /// ([`TRACKS_SURFACE`]). The canvas given at [`Face::new`] is cut where [`Layout::of`] puts the ruler: the preview
    /// takes the band above it, the tracks the rest. The pointer is the tracks Surface's ([`Face::on_surface`]).
    pub fn on_surfaces(mut self, preview: u32, tracks: u32) -> Face {
        let (w, h) = self.canvas;
        let ph = Layout::of(w, h, &self.state).preview_h;
        self.split = Some(Split { preview, tracks, preview_canvas: (w, ph.max(1)), bin: None });
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
        if role == Role::Bin { return self.paint_bin(); } let l = self.layout_for(role).with_message(self.labels.is_some()); self.wave_fill(); // STRIAINFOREELSTRIP (SR747): the message row of its own (timeline_core view.rs tail); STRIAPROJECTBIN (SR885): the bin's own painter (file tail)
        let shown = self.drag_shown.get(self.drag.as_ref(), &self.state); // STRIAPERFDRAGSHOWN (SR936): the drag preview replayed only when the drag or the state it reads changes (timeline_core view.rs tail) self.slip_refresh(role, shown.as_ref()).or_else(|| self.crop_drag.as_ref().and_then(|d| d.shown(&self.state))).or_else(|| self.xform_drag.as_ref().and_then(|d| d.shown(&self.state))); // STRIASLIPVIEW (SR858): a Slip drag's first / last frame pair (file tail) // STRIACROPMORE (SR960): a live crop drag shows its clip uncropped (timeline_core view.rs tail) // STRIAXFORMTHREE (SR9033): a live placement drag shows its clip at the dragged rect (timeline_core view.rs tail)
        let st = shown.as_deref().unwrap_or(&self.state); self.peaks.thumbs.fill(&st.timeline, &self.sources, l); // STRIATHUMBNAILS (SR839): the visible video heads decoded before the paint, THUMBS_PER_PAINT / THUMB_BUDGET_MS at most (file tail)
        if role != Role::Tracks && !self.pcache.taken(self.play.take_still(&mut self.still, st.shown_tick(), shown.is_none() || scrubbing(&self.drag)) && self.slip_view().is_none()) && !self.pcache.hit(&st.timeline, st.playhead) { // NLEPLAYAUDIO (SR676): a played tick's paint shows its own render; STRIAAUDIOSCRUB (SR865): and a scrub step's, under its live Scrub drag; STRIAPERFPREVIEWCACHE (SR918): an unchanged (timeline, playhead) keeps the still (file tail); STRIASOURCEPLAY (SR9080): keyed on the source playhead while loaded
            self.still = match preview_shown(st, &self.sources) { // STRIASOURCEVIEWER (SR9045): a loaded source shows through the same frame road (file tail)
                Ok(s) => s,
                Err(e) => {
                    self.pcache.miss(); self.last_error = Some(format!("preview: {e}")); // STRIAPERFPREVIEWCACHE (SR918): an error is never cached (file tail)
                    None
                }
            };
        }
        let still = if role == Role::Tracks { None } else { self.slip_view().or(self.still.as_ref()) }; // STRIASLIPVIEW (SR858): the pair replaces the playhead frame while it shows
        let px = self.paint_message(view::paint_xform_hover(paint_source_ruler(view::paint_xform_handles(view::paint_xform_drag(self.slipthree_paint_tc(view::paint_crop_drag(self.pcache.band(l, vo(self.order), still, self.playing || self.drag.is_some(), |s| paint_labelled(st, l, self.order, self.labels.as_ref().map(|s| Labels(s, Some(&self.lcache))), s), Some(&self.peaks as &dyn view::PeakSource)), l, vo(self.order), self.crop_drag.as_ref().filter(|_| role != Role::Tracks)), l, role), l, vo(self.order), self.xform_drag.as_ref().filter(|_| role != Role::Tracks)), l, vo(self.order), self.xform_handles(st, l, role).as_ref()), l, self.order, st), l, vo(self.order), self.xform_hover(st, l, role).as_ref()), l, role); // STRIACROPMORE (SR960): the dragged crop rect over the preview picture; STRIASLIPVIEWTHREE (SR9036): each cell's source timecode over the pair (file tail); STRIAXFORMHOVER (SR9107): the hovered (or held) handle in SELECT (file tail); STRIAXFORMHANDLES (SR9068): the placed clip's corner handles (file tail); STRIAXFORMTHREE (SR9033): the dragged placement rect over the preview picture; STRIAINFOREEL (SR743): the message line at the preview band's foot (file tail); STRIAPERFLABELSHAPE (SR935): labels through the face's cache (file tail); STRIAPERFTHREE (SR9037): the band's pixels kept by still (file tail); STRIASOURCEPLAY (SR9080): the source ruler strip while loaded (file tail)
        if role == Role::Preview {
            return paint_meter_clips(paint_meters(px, l, self.order, &self.meter_levels()), l, self.order, self.meter_hold.clipped());
        }
        { let px = paint_marks(paint_queue_depth(paint_export(paint_meter_clips(paint_meters(view::paint_source_ghost(view::paint_snap_badge(view::paint_drag(view::paint_roll_cue(self.paint_transition_still(px, l), l, vo(self.order), &self.state, self.roll_tool && self.drag.is_none()), l, vo(self.order), self.drag.as_ref()), l, vo(self.order), self.drag.as_ref(), self.still.as_ref()), l, vo(self.order), self.snap_off), l, self.order, &self.meter_levels()), l, self.order, self.meter_hold.clipped()), l, self.order, self.export_frac()), l, self.order, self.export_queue.len()), l, self.order, self.marks()); self.paint_help(px, l) } // STRIAEXPORTRANGEBAND (SR9099): the queue depth's notches on the band (file tail); NLEFACEEXPORT (SR694): the progress band over the ruler's foot; NLE5DEXPORTRANGE (SR717): the in / out marks over the ruler; NLE5DKEYSHELP (SR744): the keys help over the timeline band, last (file tail); STRIASOURCEGHOST (SR9115): the source drag's ghost over the guide (view.rs tail)
    }
    /// Every Surface's blit: [`Face::frame`] alone on one Surface; split, the preview's then the tracks'.
    pub fn frames(&mut self) -> Vec<SMessage> {
        let Some(s) = self.split else { return vec![self.frame()] };
        let preview = self.paint_role(Role::Preview);
        let tracks = self.paint_role(Role::Tracks);
        let (pw, ph) = s.preview_canvas;
        let (tw, th) = self.canvas;
        self.bin_blit(vec![ // STRIAPROJECTBIN (SR885): the bin's blit after the tracks' (file tail)
            SMessage::SurfaceBlit { url: PREVIEW_SURFACE.into(), width: pw, height: ph, pixels: preview, damage: None, scroll: None },
            SMessage::SurfaceBlit { url: TRACKS_SURFACE.into(), width: tw, height: th, pixels: tracks, damage: None, scroll: None },
        ])
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
        self.doc_path = path.map(std::path::Path::to_path_buf); self.offer_autosave(); self.offer_kept_autosave();
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
        let text = timeline_core::edl::write(&self.sources.seq_rated(&self.state.timeline), title).map_err(|e| format!("{}: {e}", path.display()))?; // STRIASEQRATEEDL (SR9053): each sequence reel's open rate rides the document (nle.rs `Sources::seq_rated`)
        std::fs::write(&path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
        self.clean = self.state.timeline.clone(); self.drop_autosave();
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
    rate: u32, stretch: Option<ShuttleStretch>, rev_next: Option<i64>, sync: PlaySync, pace: PlayPace, // STRIAPLAYSYNCPACE (SR9055): the shown still paced to the AudioClock (file tail); STRIAJKLNEXT (SR987): the A/V offset against consumed (file tail); STRIAJKLTRANSPORT (SR873): the shuttle's ONE stretch (file tail); STRIAJKLMORE (SR945): two stages past 4x, and where the last reverse render began (file tail)
    scrub: Option<(u64, u64)>, // STRIAAUDIOSCRUBMORE (SR947): the last scrub burst (device consumed before it, frames it pushed)
    scrub_retry: Option<i64>, // STRIAAUDIOSCRUBNEXT (SR988): the tick a refused scrub step owes
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
    pushed: u64, stretch: Option<&'a mut ShuttleStretch>, rev: Option<Vec<f32>>, // STRIAJKLTRANSPORT (SR873): the shuttle's stretch, None at 1x (file tail); STRIAJKLMORE (SR945): a reverse render's stereo, collected (file tail)
}

impl RenderSink for PlaySink<'_> {
    fn proxy(&self) -> bool { true } // STRIAPROXYMEDIA (SR882): the played still decodes a source's attached proxy too
    fn frame(&mut self, _out_t: i64, width: u32, height: u32, rgba: &[u8]) {
        if self.rev.is_none() || self.still.is_none() { self.still = Some(Still { width, height, rgba: rgba.to_vec() }); } // STRIAJKLMORE (SR945): a reverse render keeps its FIRST frame, the new playhead (file tail)
    }
    fn pcm(&mut self, out_t: i64, rate: u32, channels: u16, interleaved: &[f32]) {
        RenderSink::pcm(&mut *self.meters, out_t, rate, channels, interleaved); if let Some(r) = self.rev.as_mut() { r.extend(crate::media::to_stereo(interleaved, channels as usize)); return; } // STRIAJKLMORE (SR945): a reverse render's audio is pushed by `reverse_tick`, frames reversed (file tail)
        if let Some(o) = self.out.as_mut() {
            let took = o.push(&shuttle_pcm(self.stretch.as_deref_mut(), crate::media::to_stereo(interleaved, channels as usize))); // STRIAJKLTRANSPORT (SR873): stretched at 2x / 4x (file tail)
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
    /// forward 2x / 4x shuttle is heard through ONE WSOLA stretch, STRIAJKLTRANSPORT SR873; 8x through its two stages and reverse at every rate, STRIAJKLMORE SR945). A run's end logs what the device consumed, once.
    fn sync_audio(&mut self) {
        let want = self.playing && (1..=SHUTTLE_MAX).contains(&self.rate.abs()); self.shuttle_stretch(); // STRIAJKLTRANSPORT (SR873): the forward shuttle is heard (file tail); STRIAJKLMORE (SR945): every rate, either direction (file tail)
        if want == self.play.live {
            return;
        }
        self.play.live = want; self.play.scrub = None; self.play.scrub_retry = None; // STRIAAUDIOSCRUBNEXT (SR988): and the owed retry; STRIAAUDIOSCRUBMORE (SR947): a transport change ends the scrub gate (file tail) self.sync_run(want); self.pace_run(want); // STRIAJKLNEXT (SR987): a heard run starts / ends its A/V measurement (file tail); STRIAPLAYSYNCPACE (SR9055): and its pacing (file tail)
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
        self.pace_clear(); if self.rate <= 0 { if self.reverse_tick(ph) { return true; } // STRIAPACETAIL (SR9087): a shuttle or reverse tick clears the pace queue, from what was seen (file tail); STRIAJKLMORE (SR945): with a released device a reverse tick is ONE render, heard backward (file tail)
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
        if end <= ph { if self.pace_tail(ph) { return true; } // STRIAPACETAIL (SR9087): at the end the queued stills drain at the AudioClock before play stops (file tail)
            self.playing = false;
            self.sync_audio();
            return true;
        }
        if self.play.next != Some(ph) {
            self.meters.reset(); if let Some(w) = self.play.stretch.as_mut() { w.reset(); } // STRIAJKLTRANSPORT (SR873): the stretch restarts with the ring
            if let Some(o) = self.play.out.as_mut() {
                o.flush();
            }
        }
        self.sync_mark(ph); let out = if self.play.live { self.play.out.as_mut() } else { None }; // STRIAJKLNEXT (SR987): the shown tick against the AudioClock, before its push (file tail); STRIAJKLTRANSPORT (SR873): live is 1x / 2x / 4x forward
        let mut sink = PlaySink { meters: &mut self.meters, out, still: None, pushed: 0, stretch: self.play.stretch.as_mut(), rev: None }; // STRIAJKLTRANSPORT (SR873); STRIAJKLMORE (SR945): forward pushes as it renders
        let r = render(&self.state.timeline, ph..end, &self.sources, &mut sink);
        let (still, pushed) = (sink.still, sink.pushed);
        self.play.renders += 1; self.pcache.played(&self.state.timeline, ph, end); // STRIAPERFMORE (SR974): a one-tick render may seed the preview key (file tail)
        self.play.pushed += pushed;
        match r {
            Ok(_) => self.play.pending = Some((ph, still)),
            Err(e) => {
                self.last_error = Some(format!("meters: {e}"));
                self.meters.reset();
            }
        }
        (self.play.shown, self.play.next) = (Some(ph), Some(end)); self.play.rev_next = None; self.pace(end); self.pace_queued(); // STRIAPACETAIL (SR9087): stills left queued held at the preview size (file tail); STRIAPLAYSYNCPACE (SR9055): the still shown when the AudioClock reaches it (file tail); STRIAJKLMORE (SR945): a forward tick ends a reverse run (the next reverse tick flushes)
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
    pub(super) enum Ev {
        Push(Vec<f32>),
        Paused(bool),
        Flush,
    }

    /// The tests' `AudioOut`: records every call; takes every sample; `consumed` is what it took.
    pub(super) struct RecordingOut(pub(super) Arc<Mutex<Vec<Ev>>>);

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

    pub(super) fn face() -> (Face, Arc<Mutex<Vec<Ev>>>) {
        let (tl, src) = synth_fixture();
        let log = Arc::new(Mutex::new(Vec::new()));
        let f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_audio(Box::new(RecordingOut(log.clone())), SYNTH_RATE);
        (f, log)
    }

    pub(super) fn pushes(log: &Arc<Mutex<Vec<Ev>>>) -> Vec<Vec<f32>> {
        log.lock().unwrap().iter().filter_map(|e| if let Ev::Push(s) = e { Some(s.clone()) } else { None }).collect()
    }

    pub(super) fn still_at(t: i64) -> Option<Still> {
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

    /// NLEPLAYAUDIO M3: the device starts paused, play releases it, stop pauses it then flushes; a 2x shuttle stays released and its tick is stretched (STRIAJKLTRANSPORT SR873); it was
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
        assert_eq!(log.lock().unwrap().iter().filter(|e| !matches!(e, Ev::Push(_))).cloned().collect::<Vec<_>>(), vec![Ev::Paused(false)], "STRIAJKLTRANSPORT (SR873): the 2x shuttle stays released, nothing paused or flushed");
        assert_eq!((f.state.playhead, f.played().renders, f.played().pushed), (1, 2, 1920 + 1440)); // STRIAJKLTRANSPORT (SR873): the 2x tick pushes its stretch (3840 frames in, one 960-frame WSOLA window held: (3840 - 960) / 2)

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

use crate::nle_export::{AudioOut, Container, ExportError, ExportReport, H264Opts, PresetOpts, VIDEO_NAMES, export_as_with, video_named_with}; // STRIAEXPORTPRESETS (SR871): the table with a preset's H.264 options

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
    range: Option<std::ops::Range<i64>>, video: String, audio: AudioOut, started: std::time::Instant, rate_ema: f64, rate_at: (std::time::Instant, u64), chapters: String, named: bool, // STRIAEXPORTRANGEBAND (SR9099): the entry is named on the running row (a queued start or a marker word, file tail); NLE5DEXPORTMESSAGEAUDIO (SR756): the video word and the audio the result line and the witness name
    sgen: u64, // STRIAPROXYSCAN (SR9116): the sources generation the export cloned (file tail)
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
        let all = v.split_whitespace().any(|w| w == "--all"); let args: Vec<&str> = words.filter(|w| *w != "--all").collect(); let (args, opts) = export_preset_words(self.picked_preset_words(args)); let (args, audio) = export_audio_word(args);
        if let Some((d, t)) = self.export_progress().filter(|_| self.export_queue.len() >= EXPORT_QUEUE_MAX) { // STRIAEXPORTQUEUE (SR881): busy = running and the queue full
            self.last_error = Some(format!("export: busy ({d}/{t})"));
            return Some(true);
        }
        let (dir, fmt) = match self.export_args(&args) { Ok(x) => x, Err(e) => { self.last_error = Some(format!("export: {e}")); return Some(true); } }; // STRIAEXPORTRANGE (SR9075): the dir / format grammar lifted once (file tail)
        if let Err(e) = audio.and_then(|a| opts.and_then(|o| self.export_or_queue(&dir, &fmt, if all { None } else { self.export_range() }, a, o))) { // STRIAEXPORTPRESETS (SR871): a preset's H.264 options (file tail) // STRIAEXPORTQUEUE (SR881): a running job queues the request (file tail)
            self.last_error = Some(format!("export: {e}"));
        }
        Some(true)
    }
    /// Start the job: the directory made here (a bad one refused by name, no thread), the encoder built on the thread
    /// (encoders are not `Send`), `range` (the marked range, NLE5DEXPORTRANGE SR717; `None`: the whole timeline), `audio` (the verb's word, default `AudioOut::Wav`, NLE5DEXPORTAUDIOOPTIONS SR722) into `Container::Mp4` (`nle export`'s defaults).
    /// `pause_at`: the job holds after frame k until cancelled ([`Face::export_paused_at`]).
    fn start_export(&mut self, dir: &std::path::Path, fmt: &str, pause_at: Option<u64>, range: Option<std::ops::Range<i64>>, audio: AudioOut, (opts, scale): (H264Opts, u8)) -> Result<(), String> { // STRIAEXPORTPRESETS (SR871): `opts` moved into the thread, as `fmt` is; STRIAEXPORTPRESETSNEXT (SR989): the preset's `scale` beside it
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
                let video = crate::nle_export::scaled(video_named_with(&fmt, tl.tb, opts).expect("a VIDEO_NAMES word"), scale); // STRIAEXPORTPRESETS (SR871): the face's defaults unless a preset set them; STRIAEXPORTPRESETSNEXT (SR989): a scaled preset's frames through THE one scaler
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
        let started = std::time::Instant::now(); self.export = Some(ExportJob { dir: dir.to_path_buf(), done, total, cancel, handle: Some(handle), shown: 0, chapters: self.export_chapters(&range), range, video, audio, started, rate_ema: 0.0, rate_at: (started, 0), named: false, sgen: self.sources_gen }); // STRIAEXPORTRANGEBAND (SR9099): a plain start is unnamed (file tail)
        Ok(())
    }
    /// The deterministic seam: start an export of `fmt` into `dir` that holds after frame `k` until it is cancelled (Esc,
    /// [`Face::cancel_export`]). The verb's own job is this with no hold.
    pub fn export_paused_at(&mut self, dir: &std::path::Path, fmt: &str, k: u64) -> Result<(), String> {
        self.start_export(dir, fmt, Some(k), self.export_range(), AudioOut::Wav, (H264Opts::default(), 1)) // STRIAEXPORTPRESETSNEXT (SR989): unscaled
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
        self.export_progress().map(|(d, t)| if t == 0 { 0.0 } else { (d as f64 / t as f64).min(1.0) }).or_else(|| self.loudness_frac()) // STRIALOUDNESSMORE (SR961): else the running loudness measure's fraction (file tail)
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
            self.finish_export(); self.start_queued(); // STRIAEXPORTQUEUE (SR881): the next queued request starts as this one is reported (file tail)
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
        j.cancel.store(true, Relaxed); self.drop_queue_on_cancel(); // STRIAEXPORTQUEUE (SR881): Esc drops the waiting requests too; STRIAEXPORTQUEUEMORE (SR971): and names the dropped count (file tail)
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
            Some(Ok(Ok(r))) => format!("export: {} frames{}, {} bytes, {} -> {}{}", r.frames, ticks_note(&j.range), r.bytes, export_words(&j.video, j.audio, j.started.elapsed(), r.frames, (self.state.timeline.tb.den, self.state.timeline.tb.num)), j.dir.display(), chapters_done(&j.dir, &j.chapters)),
            Some(Ok(Err(ExportError::Io(e)))) if e.kind() == std::io::ErrorKind::Interrupted && j.cancel.load(Relaxed) => {
                format!("export: cancelled at {d}/{t} frames; partial files left in {}", j.dir.display())
            }
            Some(Ok(Err(e))) => format!("export: {e}"),
            _ => "export: the export thread panicked".to_string(),
        };
        self.last_error = Some(line); self.export_stale(j.sgen); // STRIAPROXYSCAN (SR9116): an export cloned before a reload says so (file tail)
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
    fn mark_verb(&mut self, v: &str) -> Option<bool> { let (v, arg) = mark_arg(v);
        let is_in = match v {
            "in" => true,
            "out" => false,
            _ => return None,
        };
        let ph = match arg.map(|a| timeline_core::edl::tc_entry(a, &self.state.timeline)) { None => self.state.playhead, Some(Ok(t)) => t, Some(Err(m)) => { self.last_error = Some(format!("{v}: {m}")); return Some(true); } }; // STRIADROPFRAMEFACE (SR9078): `in` / `out <timecode|frames>` marks at the typed tick (file tail)
        let (a, b) = self.state.timeline.marks; let (mine, other) = if is_in { (a, b) } else { (b, a) }; // NLE5DRANGESAVE (SR723): the marks are the timeline's
        let set = if mine == Some(ph) && arg.is_none() { // STRIADROPFRAMEFACE (SR9078): a typed timecode always sets
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
    let named = args.last().and_then(|w| crate::nle_export::audio_named(w)); if let Some(e) = striaexportaacbitrate_refusal(&mut args) { return (args, Err(e)); }
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
        let c = self.hover_dwelt().and_then(|(t, i)| self.state.timeline.tracks.get(t)?.clips.get(i).copied()).or_else(|| self.state.selected())?; self.clip_reel_line(c) // STRIASOURCEINFO (SR875): THE one `<reel>: <summary>` line (file tail)
    }
    /// The face's message line: the last error while one stands, else [`Face::reel_line`].
    pub fn message_text(&self) -> Option<String> {
        self.entry_line().or_else(|| self.last_error.clone().or_else(|| self.info_text()).or_else(|| self.xform_note.clone()).or_else(|| self.preset_note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone()).or_else(|| self.note.clone())).or_else(|| self.export.as_ref().map(|j| job_line(j) + &self.running_note() + &self.waiting_note())).or_else(|| self.laid.clone()).or_else(|| self.shuttle_line()).or_else(|| self.roll_tool_note()).or_else(|| self.ripple_all_note()).or_else(|| self.state.source_line()).or_else(|| self.stack_note()).or_else(|| self.reel_line()) // STRIAXFORMPRESETS (SR938): the placement line after a pip / scale word (file tail) // STRIAJKLMORE (SR945): the shuttle rate after the export line (file tail) // STRIAEXPORTRANGEBAND (SR9099): the running entry's name (file tail); STRIAEXPORTQUEUEMORE (SR971): the running row names the waiting count (file tail).or_else(|| self.picked_line()) // STRIAEXPORTPRESETSNEXT (SR989): the picked preset as the last fallback (file tail) // STRIASTACKTOOL (SR9093): the layer count under the playhead (file tail). STRIAOPACITYNEXT (SR993): a standing note after the error (file tail) // STRIAGAPSTHREE (SR9028): the face note after the error (file tail) // STRIARIPPLETHREE (SR9034): the tool's line while it is on // STRIASOURCEVIEWER (SR9045): the loaded source's standing line (timeline_core editor.rs tail) // STRIAGOTOMORE (SR9097): the open entry line wins the row (file tail)
    }
    /// The message over a painted canvas: one line at x 4, baseline the message row's foot - 4, clipped to the canvas width, through
    /// [`Labels`]; unchanged with labels off, on the tracks Surface, or a band under [`MESSAGE_H`].
    fn paint_message(&self, px: Vec<u8>, l: Layout, role: Role) -> Vec<u8> {
        let Some(style) = self.labels.as_ref() else { return px };
        if role == Role::Tracks || l.message_h < MESSAGE_H {
            return px;
        }
        let px = self.paint_tc(px, l); let Some(text) = self.message_text() else { return px }; // STRIATIMECODEDISPLAY (SR872): the readout on the row's left half first (file tail)
        let mut c = Canvas { w: l.w, h: l.h, order: vo(self.order), px };
        view::Text::text(&mut Labels(style, Some(&self.lcache)), &mut c, &text, (self.tc_split(l) + 4) as f32, (l.message_y + l.message_h) as f32 - 4.0, (self.tc_split(l) as i32, l.w as i32)); // STRIATIMECODEDISPLAY (SR872): the right half while the readout shows (x 4, the whole row, off) // STRIAPERFLABELSHAPE (SR935): the message line through the cache (file tail)
        c.px
    }
}

// ---- NLETRANSITIONFACE (LEDGER SR691): the transition widget's edge drag and kind / ease (named tail block) ----

/// NLETRANSITIONFACE (SR691): a press on a transition widget's edge zone begins timeline_core's edge drag (through `Drag::begin_grab`, STRIAKGAINENV SR9111)
/// (`Drag::begin_transition_edge`; the release's generic `Drag::finish` is its ONE `set_transition_dur`, the paint's
/// `Drag::shown` / `paint_drag` its preview and guide); the `kind` / `ease` words are timeline_core's verbs
/// (`EditorState::verb`), with the selected transition's thumbnail re-rendered after them.
impl Face {
view::press_nosnap(mods & bandy::MOD_ALT != 0, self.snap_off).or_else(|| Drag::begin_xfade_edge(&mut self.state, l, x, y, )) // STRIAXFADETHREE (SR9023): else an audio crossfade bracket's edge (timeline_core view.rs tail)
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
pub use timeline_core::editor::SLIP_STEP; // STRIAKSLIP (LEDGER SR9002): lifted into timeline_core with the words

/// `slip-` -> `slip -1`, `slip+` -> `slip 1`, `slide-` -> `slide -1`, `slide+` -> `slide 1` (one frame, in ticks);
/// `slip-10` -> `slip -10` ... (the count exactly [`SLIP_STEP`]); None for any other word; roll-/roll+/roll-10/roll+10 -> roll <d> (STRIAROLLKEYS SR893).
fn slipslide_word(v: &str) -> Option<String> { timeline_core::editor::slipslide_word(v) } // STRIAKSLIP (LEDGER SR9002): the rule lives in timeline_core, THE dispatch calls it toomatch { Some(r) => ("slide", r), None => match v.strip_prefix("nudge") { Some(r) => ("nudge", r), None => ("roll", v.strip_prefix("roll") } } // STRIANUDGE (SR855): and `nudge±[10]` -> `nudge <d>`; STRIAROLLKEYS (SR893): the roll words ride the same rewrite onto SR851's `roll <d>`

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
    ("cancel", "cancel the drag, the export or the loudness measure"),
    ("undo", "undo"),
    ("save", "save"),
    ("nudge-", "move the selection one frame earlier"),
    ("nudge+", "move the selection one frame later"),
    ("nudge-10", "move the selection 10 frames earlier"), // STRIANUDGE (SR855)
    ("nudge+10", "move the selection 10 frames later"),
    ("[", "clip start one frame earlier; a selected transition one frame shorter"), // STRIATRANSITIONMORE (SR956): the key's two roads (Face::transition_verb first)
    ("]", "clip start one frame later; a selected transition one frame longer"),
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
    ("mark", "drop a marker at the playhead"), // STRIAMARKERS (SR835)
    ("roll-", "roll the cut one frame earlier"), ("roll+", "roll the cut one frame later"), ("roll-10", "roll the cut 10 frames earlier"), ("roll+10", "roll the cut 10 frames later"), // STRIAROLLKEYS (SR893)
    ("tc", "playhead readout: off / timecode / frames / seconds"), // STRIATCSET (SR908)
    ("zoom-fit", "fit the whole timeline in the window"), ("zoom-sel", "fit the selection in the window"), // STRIAZOOMFITKEYS (SR907)
    ("copy", "copy the selection"), ("cut", "cut the selection (the gap stays)"), ("paste", "paste at the playhead"), ("paste-insert", "paste at the playhead, pushing later clips on"), // STRIAPASTEKEYS (SR921): the clipboard chords' phrases
    ("track-up", "move the clip's track up (same kind)"), ("track-down", "move the clip's track down (same kind)"), // STRIATRACKORDERKEYS (SR922)
    ("marker-", "go to the previous marker"), // STRIAMARKERJUMPKEYS (SR923)
    ("marker+", "go to the next marker"),
    ("cut-", "go to the previous clip edge"),
    ("cut+", "go to the next clip edge"),
    ("insert", "insert the clip's source at the playhead"), // STRIAINSERTMARKSKEYS (SR924): the insert words' keys
    ("overwrite", "overwrite at the playhead with the clip's source"),
    ("insert-marked", "insert the clip's source into the in / out marks"),
    ("overwrite-marked", "overwrite the in / out marks with the clip's source"),
    ("xfade", "put / take the selected audio clip's crossfade"), // STRIAXFADECURVEBUTTON (SR925)
    ("xfade-curve", "switch the crossfade's curve linear / equal"),
    ("gain-", "clip gain down one step"), ("gain+", "clip gain up one step"), ("track-gain-", "track gain down 1 dB"), ("track-gain+", "track gain up 1 dB"), ("mute", "mute / unmute the clip's track"), ("solo", "solo / unsolo the clip's track"), // STRIATRACKGAINKEYS (SR926)
    ("history", "list the last undo steps"), ("info", "name the selected clip's source for 5 seconds"), // STRIASOURCEINFOKEY (SR942): the Ctrl+i / Meta+i row
    ("snap", "drag snap on / off (Alt: this drag only)"), // STRIASNAPMORE (SR952)
    ("bright-", "brightness down one step"), ("bright+", "brightness up one step"), ("contrast-", "contrast down one step"), ("contrast+", "contrast up one step"), ("sat-", "saturation down one step"), ("sat+", "saturation up one step"), // STRIAGRADEMORE (SR957): the grade keys' phrases
    ("opacity-", "selection 10 % more transparent"), ("opacity+", "selection 10 % more opaque"), // STRIAOPACITYMORE (SR958)
    ("pan-", "pan the clip one eighth left"), ("pan", "centre the clip's pan"), ("pan+", "pan the clip one eighth right"), // STRIAPANMORE (SR959): the Alt pan keys' phrases
    ("meters-reset", "clear the meters' clip caps and peak holds"), // STRIAMETERSMORE (SR962)
    ("roll-tool", "roll tool on / off: drag a clip edge to roll its cut"), // STRIAROLLMORE (SR954)
    ("unmark all", "remove every marker"), // STRIAMARKERSMORE (SR966)
    ("crop-l+", "crop the left side in"), // STRIACROPMORE (SR960): the crop chords' words
    ("crop-l-", "uncrop the left side"),
    ("crop-t+", "crop the top in"),
    ("crop-t-", "uncrop the top"),
    ("crop-r+", "crop the right side in"),
    ("crop-r-", "uncrop the right side"),
    ("crop-b+", "crop the bottom in"),
    ("crop-b-", "uncrop the bottom"),
    ("link-av", "link each picture to its sound"), ("unlink", "unlink the selected clips"), ("link", "link the selected clips"), ("unlink-all", "unlink every clip"), // STRIALINKAUTOTHREE (SR9039): the link keys' phrases
    ("ripple-all", "ripple all tracks on / off: shift-drag a clip edge to ripple every track"), // STRIARIPPLETHREE (SR9034)
    ("pip key", "set the placement point at the playhead"), ("pip unkey", "remove the placement point at the playhead"), // STRIAKXFORMKEYS (LEDGER SR9073)
    ("source", "load the clip's reel at the playhead (match frame)"), // STRIASOURCEPLAY (SR9080): the source monitor's words
    ("unload", "unload the source"),
    ("src-prev", "source one frame back"),
    ("src-next", "source one frame on"),
    ("src-in", "source in at its playhead"),
    ("src-out", "source out at its playhead"),
    ("dur-", "the clip's transition (its head, else its fade to black) one frame shorter"), ("dur+", "the clip's transition (its head, else its fade to black) one frame longer"), // STRIAKDUR (SR9089)
    ("env", "gain point at the playhead"), ("env-del", "remove the gain point at the playhead"), ("env-", "gain point down one step"), ("env+", "gain point up one step"), // STRIAGAINENVDRAG (SR9082)
    ("enter goto", "go to a typed timecode (Return: go, Escape: cancel)"), // STRIAGOTOMORE (SR9097)
    ("enter", "type a face word (Return: do it, Escape: cancel)"),
    ("reverse", "play the clip backwards (again: forwards)"), // STRIAKREVERSE (SR9095)
    ("help", "show / hide this help"),
    ("export-skip", "skip the running export (the next waiting one starts)"), // STRIAEXPORTQUEUELIST (SR932): the `E` row // STRIAUNDOJUMP (SR934)
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
fn key_label(k: &str) -> Option<&str> {
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
pub fn help_rows() -> Vec<(String, &'static str)> { help_rows_with(&Keymap::default()) } pub fn help_rows_with(map: &Keymap) -> Vec<(String, &'static str)> { // STRIAKEYBOARDMAP (SR886): the same rows over a key overlay's effective bindings (`Keymap::bindings`, file tail)
    let mut rows: Vec<(&'static str, String, &'static str)> = Vec::new();
    for (k, act) in map.bindings() {
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
        let (ascent, descent, gap) = self.lcache.metrics(style); // STRIAPERFLABELSHAPE (SR935): metrics once per style (file tail)
        let pitch = (ascent + descent + gap).max(1.0);
        let top = l.ruler_y as f32 + HELP_PAD + self.help_query_line(pitch); // STRIAHELPMORE (SR977): a query takes the band's first line (file tail)
        let per = ((l.h as f32 - top) / pitch).floor().max(0.0) as usize;
        if per == 0 {
            return px;
        }
        let rows = self.help_rows_shown();
        // The columns: (first row, keys width, phrase width), as many as the width holds, at least one.
        let mut cols: Vec<(usize, f32, f32)> = Vec::new();
        let (mut x, mut start) = (HELP_PAD, 0usize);
        while start < rows.len() {
            let chunk = &rows[start..(start + per).min(rows.len())];
            let kw = chunk.iter().map(|r| self.lcache.width(style, &r.0)).fold(0.0f32, f32::max); // STRIAPERFLABELSHAPE (SR935): a cached width (file tail)
            let pw = chunk.iter().map(|r| self.lcache.width(style, r.1)).fold(0.0f32, f32::max); // STRIAPERFLABELSHAPE (SR935): a cached width (file tail)
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
        let mut lab = Labels(style, Some(&self.lcache)); // STRIAPERFLABELSHAPE (SR935): the help rows through the cache (file tail) self.paint_help_query(&mut lab, &mut c, l, ascent); // STRIAHELPMORE (SR977): the query drawn on the band's first line (file tail)
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
    format!("export [--all] [<dir>] [{}] [{}] [--preset {}]", VIDEO_NAMES.join("|"), audio.join("|"), crate::nle_export::preset_words()) // STRIAEXPORTPRESETS (SR871): the preset words
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

// ---- STRIAEXPORTFRAME (LEDGER SR830): the playhead frame as a PNG (named tail block) ----
// `frame [<path>]` saves the frame under the playhead: THE preview road (`preview`, one tick, the render's own frame size,
// never the face's scaled paint) through pixel_core's ONE PNG writer (`gneiss_pal::dsp::image::encode_rgba`, the one
// `PngSequence` calls), written synchronously on the face's thread. The outcome is THE message (`last_error`): `frame: <file
// name>` or `frame: <why>`; `frame_saved` keeps the path the last `frame` wrote for the witness. `nle frame` calls the same
// `save_frame`. Owed: NLE-EXPORT-FRAME-FORMAT, NLE-EXPORT-FRAME-RANGE (the export door's `png` writes every frame of the
// range), NLE-EXPORT-FRAME-KERNEL, NLE-EXPORT-FRAME-USAGE, NLE-EXPORT-FRAME-TC.

use gneiss_pal::dsp::image::PngError;

/// STRIAEXPORTFRAME (SR830): why a frame was not saved.
#[derive(Debug)]
pub enum FrameError {
    /// The render refused the tick.
    Render(RenderError),
    /// Nothing on the timeline sizes a video frame at this tick.
    NoFrame(i64),
    /// pixel_core's writer refused the picture.
    Png(PngError),
    /// The file could not be written (the path named).
    Io(std::path::PathBuf, std::io::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Render(e) => write!(f, "{e}"),
            FrameError::NoFrame(t) => write!(f, "no video frame at tick {t}"),
            FrameError::Png(e) => write!(f, "png: {e:?}"),
            FrameError::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

/// STRIAEXPORTFRAME (SR830): the default file of a saved frame: `<document dir>/<stem>-<tc>.png` (`tc` with `:` as `-`); no
/// document: the cwd-relative `untitled-<tc>.png` (the export / save stem fallback word).
pub fn frame_path(doc: Option<&std::path::Path>, tc: &str) -> std::path::PathBuf {
    let tc = tc.replace(':', "-");
    match doc {
        Some(p) => p.with_file_name(format!("{}-{tc}.png", p.file_stem().and_then(|s| s.to_str()).unwrap_or("untitled"))),
        None => std::path::PathBuf::from(format!("untitled-{tc}.png")),
    }
}

/// STRIAEXPORTFRAME (SR830): the frame tick `t` shows ([`preview`]) as one PNG ([`gneiss_pal::dsp::image::encode_rgba`]) at
/// `path`; its size. Nothing is written when the render or the encoder refuses.
pub fn save_frame(tl: &Timeline, sources: &Sources, t: i64, path: &std::path::Path) -> Result<(u32, u32), FrameError> {
    let still = preview(tl, sources, t).map_err(FrameError::Render)?.ok_or(FrameError::NoFrame(t))?;
    let png = gneiss_pal::dsp::image::encode_rgba(still.width, still.height, &still.rgba).map_err(FrameError::Png)?;
    std::fs::write(path, &png).map_err(|e| FrameError::Io(path.to_path_buf(), e))?;
    Ok((still.width, still.height))
}

impl Face {
    /// The `frame [<path>]` verb (the rest after the word, trimmed, is the path): the playhead frame through [`save_frame`]
    /// into the path, else [`frame_path`] beside the document at the playhead's timecode. No path and no document is refused
    /// by name, as `export` refuses. `None` for any other word.
    fn frame_verb(&mut self, v: &str) -> Option<bool> {
        let rest = v.strip_prefix("frame")?;
        if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
            return None;
        }
        let rest = rest.trim();
        let phv = &self.headed(v); let  = self.state.playhead;
        let path = if !rest.is_empty() {
            std::path::PathBuf::from(rest)
        } else if self.doc_path.is_some() {
            let tc = self.state.timeline.fps().map_or_else(|| ph.to_string(), |f| timeline_core::edl::timecode(ph, f));
            frame_path(self.doc_path.as_deref(), &tc)
        } else {
            self.last_error = Some("frame: no file (no document path)".into());
            self.frame_saved = None;
            return Some(true);
        };
        match save_frame(&self.state.timeline, &self.sources, ph, &path) {
            Ok(_) => {
                let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                self.last_error = Some(format!("frame: {name}"));
                self.frame_saved = Some(path);
            }
            Err(e) => {
                self.last_error = Some(format!("frame: {e}"));
                self.frame_saved = None;
            }
        }
        Some(true)
    }
    /// The message while it is a `frame` line (`frame: <name>` or `frame: <why>`).
    pub fn frame_message(&self) -> Option<&str> {
        self.last_error.as_deref().filter(|m| m.starts_with("frame: "))
    }
    /// The file the last `frame` verb wrote (`None` on a fresh face or after a refused `frame`).
    pub fn frame_saved(&self) -> Option<&std::path::Path> {
        self.frame_saved.as_deref()
    }
}

// ---- STRIAWAVEFORM (LEDGER SR827): the face's peak tables, scanned once per source (named tail block) ----
// An audio clip's body shows its waveform (timeline_core view.rs `paint_peaks`, through `paint_in`'s `PeakSource` hook). The
// tables live here, beside `sources`, never in the EDL. LAZY: `paint_role`'s first line calls `wave_fill`, which scans each
// source on an audio track the first time a paint sees it (`nle::scan_peaks`, the render's own decoder) and never again: the
// tables are immutable, so no paint invalidates them. The scan runs on the face's thread, so the first paint that shows a long
// source waits for its whole decode (owed NLE-WAVEFORM-SCAN-THREAD). A source that fails to scan stays flat (logged at debug,
// never in `last_error`, so no message row changes).

/// STRIAWAVEFORM (SR827): every scanned source's table (`None`: scanned, no audio or refused) and the scans made.
#[derive(Default)]
pub struct WavePeaks {
    map: std::collections::BTreeMap<timeline_core::SourceId, Option<view::Peaks>>,
    scans: u32,
    swept: u64, // STRIAPROXYSCAN (SR9116): the sources generation of the last cache sweep (file tail)
    thumbs: Thumbs, // STRIATHUMBNAILS (SR839): each video clip head's still, decoded once per (source, src_in) (file tail)
}

impl view::PeakSource for WavePeaks {
    fn peaks(&self, src: timeline_core::SourceId) -> Option<&view::Peaks> {
        self.map.get(&src).and_then(|p| p.as_ref())
    }
    /// The mix's ONE gain curve (nle.rs `mix_into`'s).
    fn gain_q16(&self, cdb: i32) -> i64 {
        gneiss_pal::dsp::audio::ramp::gain_q16(cdb)
    }
    /// STRIATHUMBNAILS (SR839): the head still the face decoded for `(src, src_tick)`.
    fn thumb(&self, src: timeline_core::SourceId, src_tick: i64) -> Option<&Still> {
        self.thumbs.get(src, src_tick)
    }
}

impl Face {
    /// STRIAWAVEFORM (SR827): scan every audio-track source not scanned yet (once per source for the face's life).
    fn wave_fill(&mut self) {
        self.peaks.drain_bg(false); // STRIATHUMBNAILSNEXT (SR997): the worker's finished heads and tables land first (file tail)
        loop { let Some(id) = view::peaks_want(&self.state.timeline, &|s| self.peaks.wantcs)) else { // STRIATHUMBNAILSTHREE (SR9044): marked wanted, then the same keyed test (file tail) break }; if self.peaks.thumbs.ask_scan(id) { self.peaks.map.insert(id, None); continue; } // STRIAKWAVEFORM (LEDGER SR9059): the next source by timeline_core's ONE want rule `view::peaks_want` (view.rs tail). STRIATHUMBNAILSNEXT (SR997): a served face queues the scan, the placeholder keeps the body flat (file tail)
            let p = match crate::nle::scan_peaks(&self.sources, id, view::PEAKS_PER_SEC) {
                Ok(p) => p,
                Err(e) => {
                    log::debug!("[nle-face] waveform {id:?}: {e}");
                    None
                }
            };
            self.peaks.map.insert(id, p);
            self.peaks.scans += 1;
        }
    }
/// STRIAWAVEFORM (SR827): the peak scans this face has made.
    pub fn wave_scans(&self) -> u32 {
        self.peaks.scans
    }
    /// STRIAWAVEFORM (SR827): source `id`'s peak table, once a paint has scanned it.
    pub fn wave_peaks(&self, id: timeline_core::SourceId) -> Option<&view::Peaks> {
        self.peaks.map.get(&id).and_then(|p| p.as_ref())
    }
}

// ---- STRIATITLECLIP (LEDGER SR828): the `title` word — a title card at the playhead (named tail block) ----
/// STRIATITLECLIP (LEDGER SR828): a face's title card lasts this many seconds.
pub const TITLE_SECS: u64 = 5;

/// STRIATITLECLIP: the text of a `title [<text...>]` word (`None`: not one): the words after `title`, trimmed; bare
/// `title` (the Title button) is "Title".
pub fn title_text(v: &str) -> Option<&str> {
    if v == "title" {
        return Some("Title");
    }
    let t = v.strip_prefix("title ")?.trim();
    Some(if t.is_empty() { "Title" } else { t })
}

/// STRIATITLECLIP: `text` as a [`TITLE_SECS`] title source inserted at the playhead on the first video track through
/// timeline_core's ONE editor commit (`EditorState::add_title`, `Timeline::place` Insert).
pub fn add_title(st: &mut EditorState, text: &str) -> Result<(), EditError> {
    let tb = st.timeline.tb;
    let dur = (TITLE_SECS * tb.den / tb.num.max(1)) as i64;
    st.add_title(text, dur)
}

// ---- STRIACLIPGAINFACE (LEDGER SR838): the gain / fade words and their message line (named tail block) ----
// `gain-` / `gain+` / `gain [<dB>]` / `fade-in [<frames|tc>]` / `fade-out [<frames|tc>]` ride timeline_core's ONE verb
// dispatch (`EditorState::verb`: `Timeline::set_gain` / `set_fades` through the one commit). The face puts the outcome on
// its message line: `gain -3.0 dB` / `fade-in 10, fade-out 0` after an edit (finish_export's precedent for a success
// line), `<word>: <EditError>` for a refusal (the edit dropped).
impl Face {
    /// The gain / fade words; `None` for any other word.
    fn clip_audio_verb(&mut self, v: &str) -> Option<bool> {
        let word = v.split_ascii_whitespace().next().filter(|w| matches!(*w, "gain-" | "gain+" | "gain" | "fade-in" | "fade-out"))?;
        match self.state.verb(v) {
            Some(Ok(())) => self.last_error = self.state.selected().map(|c| audio_line(word, &c)),
            Some(Err(e)) => self.last_error = Some(placement_note(&{word}{e}")),
            None => return None,
        }
        Some(true)
    }
}

/// A gain in centi-dB as a person reads it: `+3.0 dB`, `-3.5 dB`, `0.0 dB` (the tenth rounded half up on the magnitude).
pub fn db_words(cdb: i32) -> String {
    format!("{} dB", view::db_text(cdb)) // STRIATRACKGAINSTEP (SR901): the rule lifted to timeline_core view (the badge reads it too)
}

/// The message line after a gain / fade word: `gain <db_words>` or `fade-in <n>, fade-out <m>` (ticks).
fn audio_line(word: &str, c: &Clip) -> String {
    if word.starts_with("gain") { format!("gain",db_words(c.gain_cdb)) } else { format!("fade-in,fade-outcfade_infade_out) }
}

// ---- STRIATHUMBNAILS (LEDGER SR839): each video clip head's stilldecoded once and cached (named tail block) ----
// A video clip's body shows its head (timeline_core viewrs `paint_head`, through SR827's `PeakSource` hook, its `thumb`
// method) The stills live in `Face.peaks` (`WavePeaks.thumbs`), beside the peak tables, never in the EDL. LAZY: `paint_role`
// calls `Thumbs::fill` once the drag' shown timeline exists (a slip drag's new head fills live) and before `paint_in`. RULES:
// * fill walks VIDEO tracks only, rows on the canvas, clips whose body left `x_of(at) + 1` lies in `0..w` (paint_head's rule)
//   and whose body is at least THUMB_MIN_W px wide, not yet keyed; it always decodes the first wanted head, then stops once
//   THUMBS_PER_PAINT heads are decoded or the fill has spent more than THUMB_BUDGET_MS; `pending` says a wanted head was left,
//   and `serve`'s clock runs while it is set, so an idle face fills its heads at the timeline's frame rate.
// * thumb_for keys `(src, src_in)` (a trim or slip changes the key; an entry never changes). A source `Sources` holds no media
//   for (`SourceIndex::duration` none: a titled source (STRIATITLECLIP SR828) or a reel that failed to open) caches `None` with
//   no decode; otherwise ONE `preview` of a one-clip timeline at tick 0 (the preview band's road: keyframe, run-up, colour,
//   size policy, stand-in), scaled by `view::head_still`. A refusal is logged at debug (never `last_error`) and cached `None`.
// * EVICTION: at THUMB_CACHE entries the OLDEST decoded (smallest insertion seq) goes before the insert; a hit refreshes nothing.
// The decode runs on the face's thread (owed NLE-THUMBNAILS-BACKGROUND): on a long-GOP source one head costs a run-up of up
// to one GOP, and the budget makes that one head per paint.

/// STRIATHUMBNAILS (SR839): the most head stills a face keeps.
pub const THUMB_CACHE: usize = 256;
/// STRIATHUMBNAILS (SR839): the most heads one paint decodes.
pub const THUMBS_PER_PAINT: u32 = 4;
/// STRIATHUMBNAILS (SR839): a paint's fill stops past this many ms (after its first head).
pub const THUMB_BUDGET_MS: u64 = 100;
/// STRIATHUMBNAILS (SR839): a clip body narrower than this (px) asks for no head.
pub const THUMB_MIN_W: i64 = view::THUMB_MIN_W; // STRIAKTHUMBS (SR9019): the want rule's own, lifted to timeline_core view.rs

/// STRIATHUMBNAILS (SR839): the head stills by `(source, src_in)` (`None`: asked, no picture), each with its insertion seq.
#[derive(Default)]
pub struct Thumbs {
    map: std::collections::BTreeMap<(timeline_core::SourceId, i64), (u64, Option<Still>)>,
    seq: u64,
    decodes: u32,
    pending: bool,
    bg: Option<Bg>, // STRIATHUMBNAILSNEXT (SR997): the served face's decode worker (None: decode on the face's thread) (file tail)
    wanted: std::collections::BTreeSet<(timeline_core::SourceId, i64)>, kept: std::collections::BTreeSet<(timeline_core::SourceId, i64)>, // STRIATHUMBNAILSTHREE (SR9044): the keys this paint and the last one wanted, never evicted (file tail)
}

impl Thumbs {
    fn get(&self, src: timeline_core::SourceId, src_in: i64) -> Option<&Still> {
        self.map.get(&(src, src_in)).and_then(|e| e.1.as_ref())
    }
    /// The visible video heads not keyed yet, decoded within this paint's bounds (see the block's RULES). STRIAKTHUMBS
    /// (SR9019): WHICH key is next is timeline_core's `view::thumb_want` (the kernel Player reads it too): heads first, then
    /// SR949's filmstrip and tail tiles; every decode through `thumb_for`, the same THUMBS_PER_PAINT / THUMB_BUDGET_MS bound.
    fn fill(&mut self, tl: &Timeline, sources: &Sources, l: Layout) {
        let t0 = std::time::Instant::now();
        let mut done = 0u32;
        self.pending = false; self.begin_paint(); // STRIATHUMBNAILSTHREE (SR9044): the wanted keys roll and the map trims back (file tail)
        loop {
            let want = view::thumb_want(tl, &l, &|s, t| self.map.get(&(s, t)).map(|e| e.1.as_ref().map(|st| st.width as i64)));
            let Some((src, tick)) = want else { break };
            if done >= THUMBS_PER_PAINT || (done > 0 && t0.elapsed() > Duration::from_millis(THUMB_BUDGET_MS)) {
                self.pending = true;
                return;
            }
            let _ = self.thumb_for(tl, sources, src, tick);
            done += 1;
        }
    }
    /// The head of `src` at `src_in`: cached, else decoded once (see the block's RULES).
    fn thumb_for(&mut self, tl: &Timeline, sources: &Sources, src: timeline_core::SourceId, src_in: i64) -> Option<&Still> {
        use timeline_core::SourceIndex as _; if self.ask_head(tl, src, src_in) { return None; } // STRIATHUMBNAILSNEXT (SR997): a served face queues the head, a None placeholder holds the bar (file tail)
        if !self.map.contains_key(&(src, src_in)) {
            let still = if sources.duration(src).is_none() {
                None
            } else {
                self.decodes += 1;
                let mut one = tl.clone();
                one.tracks.clear();
                let v = one.add_track(Kind::Video);
                one.tracks[v].clips.push(Clip::new(src, src_in, src_in + 1, 0));
                match preview(&one, sources, 0) {
                    Ok(f) => f.and_then(|f| view::head_still(&f)),
                    Err(e) => {
                        log::debug!("[nle-face] head {src:?}@{src_in}: {e}");
                        None
                    }
                }
            };
            self.insert((src, src_in), still);
        }
        self.get(src, src_in)
    }
    /// Key `k` takes `v`, the oldest entry dropped first at THUMB_CACHE.
    fn insert(&mut self, k: (timeline_core::SourceId, i64), v: Option<Still>) {
        if !self.map.contains_key(&k) && self.map.len() >= THUMB_CACHE {
            if let Some(old) = self.map.iter().filter(|e| !self.spared(e.0)).min_by_key(|e| (e.1).0).map(|e| *e.0) { // STRIATHUMBNAILSTHREE (SR9044): a key this or the last paint wanted is never evicted (file tail)
                self.map.remove(&old); // STRIAXFORMTHREE (SR9033): the one placement line (file tail)
            }
        }
        self.seq += 1;
        self.map.insert(k, (self.seq, v));
    }
}

impl Face {
    /// STRIATHUMBNAILS (SR839): the head decodes (`preview` calls) this face has made.
    pub fn thumb_decodes(&self) -> u32 {
        self.peaks.thumbs.decodes
    }
    /// STRIATHUMBNAILS (SR839): the head still of `(src, src_in)`, once a paint has decoded itmods, .
    pub fn thumb(&self, src: timeline_core::SourceId, src_in: i64) -> Option<&Still> {
        self.peaks.thumbs.get(src, src_in)
    }
    /// STRIATHUMBNAILS (SR839): the last paint's fill left a wanted head undecoded (`serve`'s clock runs while it is true).
    pub fn thumbs_pending(&self) -> bool {
        self.peaks.thumbs.pending || self.peaks.thumbs.bg_busy() // STRIATHUMBNAILSNEXT (SR997): and while the worker holds a job (file tail)
    }
}

#[cfg(test)]
mod thumb_tests {
    use super::*;

    #[test]
    fn thumb_cache_drops_the_oldest() {
        let mut t = Thumbs::default();
        let s = Still { width: 1, height: 1, rgba: vec![1, 2, 3, 255] }; // STRIAXFORMHANDLES (SR9068): mods carries the no-snap modifier
        for i in 0..=THUMB_CACHE as i64 {
            t.insert((timeline_core::SourceId(0), i), Some(s.clone()));
        }
        assert_eq!(t.map.len(), THUMB_CACHE);
        assert!(t.get(timeline_core::SourceId(0), 0).is_none(), "the first key went");
        assert!(t.get(timeline_core::SourceId(0), 1).is_some() && t.get(timeline_core::SourceId(0), THUMB_CACHE as i64).is_some());
        println!("striathumbnails: cache {} entries after {} inserts, oldest dropped", t.map.len(), THUMB_CACHE + 1);
    }
}

// ---- STRIACLIPSPEED (LEDGER SR829): the `speed` word — the selected clip's rate through the one commit (named tail block) ----
/// STRIACLIPSPEED (LEDGER SR829): what bare `speed` (the Speed button) steps the selected clip through: 1x -> 2x -> 4x -> 1x
/// (a rate not in the table goes to 1x). It only ever shrinks the clip and then gives back its own length, so the button
/// never needs room the clip did not have.
pub const SPEED_CYCLE: [u32; 3] = [0x1_0000, 0x2_0000, 0x4_0000];

/// STRIACLIPSPEED: the rate a `speed` word asks for: `Some(None)` bare `speed` (the cycle), `Some(Some(q))` `speed <q>` (`q` a
/// decimal read by timeline_core's `speed_q16`: `speed 2`, `speed 0.5`, `speed 1.5`); `None`: not this word.
pub fn speed_word(v: &str) -> Option<Option<u32>> {
    if v == "speed" {
        return Some(None);
    }
    timeline_core::speed_q16(v.strip_prefix("speed ")?.trim(), 1).map(Some)
}

/// STRIACLIPSPEED: the `speed` word on the selected clip through `EditorState::set_speed` (ONE commit; undo restores); a
/// refusal (`NoClip`, `Rate` for `speed 0`, `Overlap`) comes back by name for the message line.
pub fn speed_verb(st: &mut EditorState, v: &str) -> Result<(), EditError> {
    let q = match speed_word(v) {
        Some(Some(q)) => q,
        Some(None) => next_speed(st)?, // STRIACLIPSPEEDTHREE (SR9031): the one cycle, shared with bare `rspeed` (file tail)
        None => return Err(EditError::NoClip),
    };
    st.set_speed(q)
}

// ---- STRIAMARKERS (LEDGER SR835): the marker words' message line (named tail block) ----
// `mark [<name>]` / `unmark` (the Mark / Unmark buttons, the `m` key) are timeline_core's `EditorState::verb` words (ONE commit
// each); the face only words THE message (`last_error`) around them: `marked <name> at <tc>`, `unmarked <name> at <tc>`,
// `mark: <name> is already at <tc>`, `unmark: no marker at <tc>` (tc the playhead's `HH:MM:SS:FF`, its tick when the rate is
// not an integer fps).

impl Face {
    /// The marker words (`timeline_core::editor::marker_word`) through the one dispatch, with their message; `None` for any
    /// other word.
    fn marker_verb(&mut self, v: &str) -> Option<bool> {
        let word = timeline_core::editor::marker_word(v)?; // STRIATRACKSTHREE (SR9038): bare `track-` names the header's track (file tail)
        let ph = self.state.playhead;
        let tc = self.state.timeline.fps().map_or_else(|| ph.to_string(), |f| timeline_core::edl::timecode(ph, f));
        let before = self.state.timeline.marker_at(ph).map(|m| m.name.clone()); let count = self.state.timeline.markers.len(); // STRIAMARKERRENAME (SR895): the count `unmark all` reports
        let r = self.state.verb(v).unwrap_or(Err(EditError::Marker));
        let now = self.state.timeline.marker_at(ph).map(|m| m.name.clone());
        if r.is_ok() { self.note = Some(self.pan_reading()); return Some(true); } self.note = None; self.last_error = Some(match (word, r, before) { // STRIAPANAUTOWRITE (SR9084): the pan shown at the playhead (file tail); STRIAPANNEXT (SR994): the reading is the note, a refusal last_error (file tail)
            (timeline_core::editor::MarkerWord::Mark(Some(_)), Ok(()), Some(b)) => { self.last_error = None; selfpan_readingnote = Some(format!("renamed {b} to {} at {tc}", now.clone()unwrap_or_default//STRIAPANAUTOWRITE (SR9084): the pan shown at the playhead (file tail) // STRIAMARKERRENAME (SR895)
            (timeline_core::editor::MarkerWord::Mark(_), Ok(())); return Some(true); } // STRIAOPACITYNEXT (SR993): the step's percent is a note, _ not an error (file tail)) => format!("marked {} at {tc}", now.unwrap_or_default()),
            (timeline_core::editor::MarkerWord::Unmark, Ok(()), b) => format!("unmarked {} at {tc}", b.unwrap_or_default()),
            (timeline_core::editor::MarkerWord::Mark(_), Err(EditError::Marker), Some(b)) => format!("mark: {b} is already at {tc}"),
            (timeline_core::editor::MarkerWord::Unmark, Err(EditError::Marker), None) => format!("unmark: no marker at {tc}"),
            (timeline_core::editor::MarkerWord::UnmarkAll, Ok(()), _) => format!("unmarked {count} markers"), // STRIAMARKERRENAME (SR895)
            (timeline_core::editor::MarkerWord::UnmarkAll, Err(EditError::Marker), _) => "unmark all: no markers".to_string(), // STRIAMARKERRENAME (SR895)
            (_, Err(e), _) => format!("{}: {e}", v.trim()),
        });
        Some(true)
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

// ---- STRIATIMECODEDISPLAY (LEDGER SR872): the playhead readout on the message row's left half (named tail block) ----
// `tc` (a view word: no undo step, no dirty) cycles off -> timecode -> frames -> seconds -> off; off is the default, so every
// paint is today's (an existing pin holds the labels-on row all BG with no message). On, the readout paints inside
// `paint_message` through the one `Labels` hook at x 4 clipped to the row's left half and the message moves to the right half;
// the row exists only with labels on, as the message's. Timecode is `edl::timecode`; fps is the ruler's (`fps()` else 25).
// Owed: NLE-TC-KERNEL (the kernel Player's message row), NLE-TC-KEY, NLE-TC-SET (`tc <unit>`), NLE-TC-DROPFRAME.

pub use timeline_core::view::{TcUnit, tc_text, tc_text_tb}; // STRIAKTC (SR9018): the readout's unit and text, lifted into timeline_core::view (the kernel Player's band calls them too)

impl Face {
    /// The `tc` word: the next unit in the cycle (off first); `None` for any other word.
    fn tc_verb(&mut self, v: &str) -> Option<bool> {
        if let Some(r) = self.tc_set(v) { return Some(r); } if v != "tc" { // STRIATCSET (SR908): `tc <unit>` sets the unit directly (file tail)
            return None;
        }
        self.tc = match self.tc {
            None => Some(TcUnit::Timecode),
            Some(TcUnit::Timecode) => Some(TcUnit::Frames),
            Some(TcUnit::Frames) => Some(TcUnit::Seconds),
            Some(TcUnit::Seconds) => None,
        };
        Some(true)
    }
    /// The readout's unit (`None`: off).
    pub fn tc_unit(&self) -> Option<TcUnit> {
        self.tc
    }
    /// The playhead in the readout's unit while it is on.
    pub fn tc_readout(&self) -> Option<String> {
        self.tc.map(|u| view::tc_readout(&self.state.timeline, self.state.playhead, u)) // STRIAKTC (SR9018): THE readout text, timeline_core's (the fps rule and SR964's NTSC base; the Player's band reads it too)
    }
    /// The message's left edge: 0 with the readout off, the row's middle with it on.
    fn tc_split(&self, l: Layout) -> u32 {
        if self.tc.is_some() { l.w / 2 } else { 0 }
    }
    /// The readout over the message row's left half (called by `paint_message` once the row is known to show); unchanged off.
    fn paint_tc(&self, px: Vec<u8>, l: Layout) -> Vec<u8> {
        let (Some(style), Some(text)) = (self.labels.as_ref(), self.tc_readout()) else { return px };
        let mut c = Canvas { w: l.w, h: l.h, order: vo(self.order), px };
        view::Text::text(&mut Labels(style, None), &mut c, &text, 4.0, (l.message_y + l.message_h) as f32 - 4.0, (0, (l.w / 2) as i32));
        c.px
    }
}

#[cfg(test)]
mod striatimecodedisplay_tests {
    use super::*;

    /// The three units at 312 @ 25 fps, tick 0, 30 fps, and a negative tick (read as 0).
    #[test]
    fn striatimecodedisplay_tc_text_units() {
        assert_eq!(tc_text(312, 25, TcUnit::Timecode), "00:00:12:12");
        assert_eq!(tc_text(312, 25, TcUnit::Frames), "312");
        assert_eq!(tc_text(312, 25, TcUnit::Seconds), "12.48 s");
        assert_eq!(tc_text(0, 25, TcUnit::Seconds), "0.00 s");
        assert_eq!(tc_text(45, 30, TcUnit::Timecode), "00:00:01:15");
        assert_eq!(tc_text(45, 30, TcUnit::Seconds), "1.50 s");
        assert_eq!(tc_text(-3, 25, TcUnit::Timecode), "00:00:00:00");
        assert_eq!(tc_text(-3, 25, TcUnit::Frames), "0");
        assert_eq!(tc_text(-3, 25, TcUnit::Seconds), "0.00 s");
        assert_eq!([TcUnit::Timecode, TcUnit::Frames, TcUnit::Seconds].map(TcUnit::word), ["timecode", "frames", "seconds"]);
    }
}

// ---- STRIANUDGE (LEDGER SR855): the ten-frame nudge and the `at` on the message line (named tail block) ----
// `nudge-` / `nudge+` (Left / Right) and `nudge-10` / `nudge+10` (Shift+Left / Shift+Right, KEYS only, no button) are
// rewritten by `slipslide_word` to timeline_core's `nudge <d>` (THE dispatch: `EditorState::move_set`, ONE commit). A
// refusal is the message `nudge: <why>` as before; a move puts the primary clip's new `at` on the message line. Owed:
// NLE-NUDGE-KERNEL (the Player binds no nudge key), NLE-NUDGE-MACSHIFT (AppKit sends Shift+arrow as the plain U+F702 /
// U+F703 text: one frame there), NLE-NUDGE-DOWORD (stria-view `--do` takes VERBS only).
impl Face {
    /// A `nudge±[10]` word: timeline_core's `nudge <d>`, the message line set either way; `None` for any other word.
    fn nudge_verb(&mut self, v: &str) -> Option<bool> {
        if !v.starts_with("nudge") {
            return None;
        }
        let w = slipslide_word(v).or_else(|| timeline_core::editor::nudge_words(v).map(|d| format!("nudge {d}")))?; // STRIANUDGEMORE (SR944): and a free `nudge <d>` (its message line too)
        let r = self.state.verb(&w).unwrap_or(Err(EditError::NoClip));
        self.last_error = match r {
            Ok(()) => self.nudge_line(),
            Err(e) => Some(format!("nudge: {e}")),
        };
        Some(true)
    }
    /// The selection's new place: `nudge: at <at> (<HH:MM:SS:FF>)` for one clip, `nudge: <n> clips, at <at> (<tc>)` for a
    /// set of `n` (the primary's `at`); `None` with nothing selected.
    pub fn nudge_line(&self) -> Option<String> {
        let c = self.state.selected()?;
        let tc = timeline_core::edl::timecode(c.at, self.state.timeline.fps().unwrap_or(25));
        Some(match self.state.set.len() {
            n if n > 1 => format!("nudge: {n} clips, at {} ({tc})", c.at),
            _ => format!("nudge: at {} ({tc})", c.at),
        })
    }
}

// ---- STRIAEXPORTPRESETS (LEDGER SR871): the export verb's `--preset <name>` through the one preset table (named tail block) ----
/// The `export` verb's `--preset <name>` (`export [--all] [<dir>] --preset web|archive|proxy`): no `--preset` leaves the words
/// as they are with the face's default H.264 options (every grammar before parses the same). `--preset <p>` is removed and,
/// with at most a directory left, the preset's video and audio words ([`crate::nle_export::PRESETS`]) are appended, so the
/// verb's own match and [`export_audio_word`] read them; the preset's H.264 options come back. Refused by name: a missing
/// name, an unknown one, and a format / audio word beside a preset.
fn export_preset_words(mut args: Vec<&str>) -> (Vec<&str>, Result<(H264Opts, u8), String>) { // STRIAEXPORTPRESETSNEXT (SR989): the preset's scale divisor beside its options (file tail)
    let Some(i) = args.iter().position(|w| *w == "--preset") else { return (args, Ok((H264Opts::default(), 1))) }; // STRIAEXPORTPRESETSNEXT (SR989): no preset, no scale
    args.remove(i);
    if i >= args.len() {
        return (args, Err(format!("--preset needs one of {}", crate::nle_export::preset_words())));
    }
    let w = args.remove(i);
    let Some(p) = crate::nle_export::preset_named(w) else {
        return (args, Err(format!("unknown preset {w} ({})", crate::nle_export::preset_words())));
    };
    if args.len() > 1 {
        return (args, Err(format!("--preset {} takes only [<dir>]", p.name)));
    }
    args.push(p.video);
    args.push(p.audio);
    (args, Ok((p.opts(), p.scale))) // STRIAEXPORTPRESETSNEXT (SR989): the table's divisor
}

// ---- STRIAAUDIOSCRUB (LEDGER SR865): a ruler scrub step plays one frame through the play road (named tail block) ----

/// STRIAAUDIOSCRUB (SR865): a live Scrub drag is shown at the face's own state (its finish edits nothing), so the paint may
/// take the scrub step's played still instead of rendering `preview` a second time.
fn scrubbing(drag: &Option<Drag>) -> bool {
    drag.as_ref().is_some_and(|d| d.kind == DragKind::Scrub)
}

impl Face {
    /// STRIAAUDIOSCRUB (SR865): one scrub step rides the play road for ONE tick: when not playing, with a device, the playhead
    /// moved off `prev`, lies before the end and is not the tick the road last showed, the device is released and
    /// [`Face::play_tick`] renders `playhead..playhead + 1` through `PlaySink` (the still the step's paint takes, the meters,
    /// one frame of sound), with `next` cleared so the meters restart and the ring is flushed (at most one frame queued: the
    /// sound never lags the pointer). `rate` and `play.live` are overridden for that one call and restored. The device stays
    /// released afterwards (an empty ring idles silent) until the next transport change's `sync_audio`.
    fn scrub_burst(&mut self, prev: i64) {
        let ph = self.state.playhead;
        if self.playing || ph == prev || self.play.out.is_none() || self.play.shown == Some(ph) || ph >= self.state.timeline.end() || !self.scrub_gate(ph) { // STRIAAUDIOSCRUBNEXT (SR988): the gate records a refused tick; STRIAAUDIOSCRUBMORE (SR947): and the last snippet mostly played (file tail)
            return;
        }
        if let Some(o) = self.play.out.as_mut() {
            o.set_paused(false);
        }
        let (rate, live) = (self.rate, self.play.live);
        (self.rate, self.play.live, self.play.shown, self.play.next) = (1, true, None, None);
        self.scrub_tick(ph); // STRIAAUDIOSCRUBMORE (SR947): the one play_tick, the gate recorded (file tail)
        (self.rate, self.play.live) = (rate, live);
    }
}

#[cfg(test)]
mod audioscrub_tests {
    use super::playaudio_tests::{Ev, face, pushes, still_at};
    use super::*;
    use crate::nle::synth_sample;

    fn btn(down: bool, x: f64, y: f64) -> SMessage {
        SMessage::BrowserButton { down, x, y, mods: 0, surface: 0 }
    }

    /// STRIAAUDIOSCRUB M3: a ruler press and each tick-changing move is ONE render through the play road: one push of rate /
    /// fps stereo frames of the tick's own mix, the meters as `MixMeters::feed` of that tick, the still the step's paint takes
    /// (no `preview` render); a move inside the tick and the release render nothing; the transport stays stopped.
    #[test]
    fn striaaudioscrub_step_is_one_render() {
        let (mut f, log) = face();
        log.lock().unwrap().clear();
        assert!(f.on(&btn(true, 44.0, 70.0)));
        assert_eq!((f.dragging().map(|d| d.kind), f.state.playhead), (Some(DragKind::Scrub), 5));
        assert_eq!(f.played().renders, 1);
        let ev = log.lock().unwrap().clone();
        assert_eq!(ev.len(), 3, "{ev:?}");
        assert_eq!((&ev[0], &ev[1]), (&Ev::Paused(false), &Ev::Flush));
        let p = pushes(&log);
        assert_eq!(p[0].len(), 2 * 1920);
        for (i, fr) in p[0].chunks_exact(2).enumerate() {
            assert_eq!(fr[0], fr[1]);
            assert_eq!(fr[0], synth_sample(0, 18 * 1920 + i as u64) as f32 / 32768.0, "frame {i}");
        }
        let (tl, src) = synth_fixture();
        let mut m = MixMeters::default();
        m.feed(&tl, 5..6, &src).unwrap();
        assert_eq!(f.meters().levels(), m.levels());
        let _ = f.frames();
        assert!(f.play.pending.is_none(), "the paint took the scrub step's still");
        assert_eq!(f.still().cloned(), still_at(5));
        assert!(f.on(&SMessage::BrowserMouseMove(124.0, 70.0, 0)));
        assert_eq!((f.state.playhead, f.played().renders), (15, 2));
        let _ = f.frames();
        assert!(f.play.pending.is_none(), "a moved Scrub drag's paint takes the still too");
        assert_eq!(f.still().cloned(), still_at(15));
        f.on(&SMessage::BrowserMouseMove(126.0, 70.0, 0));
        assert_eq!((f.state.playhead, f.played().renders, pushes(&log).len()), (15, 2, 2), "the same tick plays nothing");
        f.on(&btn(false, 126.0, 70.0));
        assert_eq!((f.state.playhead, f.played().renders, f.played().pushed), (15, 2, 3840));
        assert!(!f.playing && !f.played().live);
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
    }

    /// STRIAAUDIOSCRUB M3: no burst without a device (the paint is `preview`'s, as before), while playing (the play road
    /// already sounds), at the timeline's end, or on the tick already shown.
    #[test]
    fn striaaudioscrub_refusals() {
        let (tl, src) = synth_fixture();
        let mut mute = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        mute.on(&btn(true, 44.0, 70.0));
        assert_eq!((mute.state.playhead, mute.played().renders), (5, 0));
        let _ = mute.frames();
        assert_eq!(mute.still().cloned(), still_at(5));

        let (mut f, _) = face();
        f.verb("play");
        f.tick();
        f.on(&btn(true, 44.0, 70.0));
        f.on(&btn(false, 44.0, 70.0));
        assert_eq!((f.state.playhead, f.played().renders), (5, 1), "a scrub while playing adds no render");

        let (mut f, _) = face();
        let end = f.state.timeline.end();
        f.state.seek(3);
        f.scrub_burst(0);
        assert_eq!(f.played().renders, 1);
        f.scrub_burst(0);
        assert_eq!(f.played().renders, 1, "the tick already shown");
        f.scrub_burst(3);
        assert_eq!(f.played().renders, 1, "the playhead did not move");
        f.state.seek(end);
        f.scrub_burst(3);
        assert_eq!((f.state.playhead, f.played().renders), (end, 1), "nothing past the end");
    }

    /// STRIAAUDIOSCRUB M3: play after a scrub to 7 steps past the tick it sounded (the resumed-run rule) and continues the
    /// ring: no flush between the burst and the first played tick.
    #[test]
    fn striaaudioscrub_play_after_scrub() {
        let (mut f, log) = face();
        f.on(&btn(true, 60.0, 70.0));
        f.on(&btn(false, 60.0, 70.0));
        assert_eq!((f.state.playhead, f.played().renders), (7, 1));
        log.lock().unwrap().clear();
        f.verb("play");
        assert!(f.tick());
        assert_eq!((f.state.playhead, f.played().renders), (8, 2));
        let ev = log.lock().unwrap().clone();
        assert!(!ev.contains(&Ev::Flush), "{ev:?}");
        assert_eq!(pushes(&log).len(), 1);
    }
}

// ---- STRIASLIPVIEW (LEDGER SR858): the slip drag previews the clip's new first and last frame (named tail block) ----

/// STRIASLIPVIEW (SR858): the least wall time between two renders of the slip pair; the window is the larger of this and
/// the last pair's own render time, so the pair takes at most half the face thread however fast the pointer moves.
pub const SLIP_VIEW_MS: u64 = 100;

/// STRIASLIPVIEW (SR858): the cached pair of a live Slip drag: keyed by the shown clip `(track, idx, clip)`, when its
/// render ended and how long it took, and `due` when a paint inside the throttle window kept a stale pair.
struct SlipView {
    key: (usize, usize, Clip),
    still: Option<Still>,
    ended: std::time::Instant,
    took: Duration,
    due: bool,
    tcs: Vec<Option<String>>, // STRIASLIPVIEWTHREE (SR9036): each cell's source timecode, a FIELD cell None (file tail)
}

impl SlipView {
    /// The throttle window has passed since the pair was rendered.
    fn open(&self) -> bool {
        std::time::Instant::now() >= self.ended + self.took.max(Duration::from_millis(SLIP_VIEW_MS))
    }
}

impl Face {
    /// STRIASLIPVIEW (SR858): before a paint of `role`: for a `DragKind::Slip` drag whose shown state (`Drag::shown`) is
    /// valid on a video track, the slipped clip's first (`at`) and last (`end - 1`) frame through THE frame road
    /// ([`preview`], twice, over the one-clip timeline [`slip_clip_timeline`]), side by side ([`slip_split`]). A paint with
    /// the shown clip unchanged renders nothing; inside the throttle window the last pair stays and `due` is set (the
    /// serve clock repaints at [`Face::slip_tick`]). Any other drag or none clears it; the tracks Surface leaves it.
    fn slip_refresh(&mut self, role: Role, shown: Option<&EditorState>) {
        if role == Role::Tracks {
            return;
        }
        self.slipmore_hold_check(); let cur = match (self.drag, shown) { // STRIASLIPVIEWMORE (SR946): a stale word hold goes first (file tail)
            (Some(d), Some(s)) if d.kind == DragKind::Slip => s.timeline.tracks.get(d.track).filter(|t| t.kind == Kind::Video).and_then(|t| t.clips.get(d.idx)).map(|c| ((d.track, d.idx, *c), &s.timeline)),
            _ => slipmore_target(self.drag, shown, self.slip_hold.as_ref(), &self.state), // STRIASLIPVIEWMORE (SR946): a Slide drag, or a word slip / slide's hold (file tail)
        };
        let Some((key, tl)) = cur else {
            self.slip_view = None;
            return;
        };
        if let Some(v) = self.slip_view.as_mut() {
            if v.key == key {
                v.due = false;
                return;
            }
            if !v.open() {
                v.due = true;
                return;
            }
        }
        let t0 = std::time::Instant::now();
        let Some(cells) = slip_cells(tl, key.0, key.1, slipmore_slide(self.drag, self.slip_hold.as_ref())).map(|c| slipthree_cells(tl, key.0, key.1, slipmore_slide(self.drag, self.slip_hold.as_ref()), c)) else { // STRIASLIPVIEWMORE (SR946): a slip's two ticks or a slide's two neighbour edges (file tail); STRIASLIPVIEWTHREE (SR9036): a slide's four cells (file tail)
            self.slip_view = None;
            return;
        };
        let mut pair: Vec<Option<Still>> = vec![None; cells.len()]; // STRIASLIPVIEWTHREE (SR9036): two cells or a slide's four
        for (i, (one, t)) in cells.iter().enumerate().filter_map(|(i, c)| c.as_ref().map(|(one, t)| (i, (one, *t)))) { // STRIASLIPVIEWTHREE (SR9036): borrowed, the timecodes read the cells after; STRIASLIPVIEWMORE (SR946): a missing side stays FIELD
            pair[i] = match preview(&one, &self.sources, t) {
                Ok(s) => s,
                Err(e) => {
                    self.last_error = Some(format!("preview: {e}"));
                    None
                }
            };
        }
        let still = slipthree_split(&pair); // STRIASLIPVIEWTHREE (SR9036): SR858's split, twice nested for four cells (file tail)
        let ended = std::time::Instant::now();
        self.slip_view = Some(SlipView { key, still, ended, took: ended - t0, due: false, tcs: slipthree_tcs(&cells) }); // STRIASLIPVIEWTHREE (SR9036): the cells' source timecodes (file tail)
    }
    /// STRIASLIPVIEW (SR858): the pair the preview shows during a Slip drag (left the new first frame, right the new last).
    pub fn slip_view(&self) -> Option<&Still> {
        self.slip_view.as_ref().and_then(|v| v.still.as_ref())
    }
    /// STRIASLIPVIEW (SR858): a paint inside the throttle window kept a stale pair (the serve clock runs while true).
    pub fn slip_due(&self) -> bool {
        self.slip_view.as_ref().is_some_and(|v| v.due)
    }
    /// STRIASLIPVIEW (SR858): the serve clock's question: a stale pair whose window has passed (true: repaint).
    pub fn slip_tick(&self) -> bool {
        self.slip_view.as_ref().is_some_and(|v| v.due && v.open())
    }
}

/// STRIASLIPVIEW (SR858): `tl` cut to the clip `idx` of track `track` alone (its transitions and the audio crossfades
/// dropped): the clip's own pixels at its own ticks, whatever lies above or beside it.
fn slip_clip_timeline(tl: &Timeline, track: usize, idx: usize) -> Option<(Timeline, Clip)> {
    let mut one = tl.clone();
    let mut tr = std::mem::take(&mut one.tracks).into_iter().nth(track)?;
    let c = *tr.clips.get(idx)?;
    tr.clips = vec![c];
    tr.transitions.clear();
    one.tracks = vec![tr];
    one.audio_xfades.clear();
    Some((one, c))
}

/// STRIASLIPVIEW (SR858): two frames side by side in one RGBA still, each cell the first present frame's size (FIELD
/// filled): `a` letterboxed into the left cell and `b` into the right through `Canvas::blit_fit` (the one scaler). A
/// missing side leaves its cell FIELD; both missing: `None`.
pub fn slip_split(a: Option<&Still>, b: Option<&Still>) -> Option<Still> {
    let first = a.or(b)?;
    let (w, h) = (first.width, first.height);
    if w == 0 || h == 0 {
        return None;
    }
    let mut c = Canvas::new(w * 2, h, Order::Rgba);
    c.rect(0, 0, (w * 2) as i64, h as i64, FIELD);
    if let Some(a) = a {
        c.blit_fit(a, 0, 0, w, h);
    }
    if let Some(b) = b {
        c.blit_fit(b, w, 0, w, h);
    }
    Some(Still { width: w * 2, height: h, rgba: c.px })
}

#[cfg(test)]
mod striaslipview_tests {
    use super::*;

    /// Rule: a repaint with the slip delta unchanged renders nothing (a planted pair survives it); a Move drag clears it.
    #[test]
    fn striaslipview_same_delta_keeps_the_cache() {
        let (tl, src) = synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        face.state.select_at(0, 12).unwrap();
        let (x, y) = face.layout().point(0, 15);
        face.on(&SMessage::BrowserButton { down: true, x, y, mods: bandy::MOD_CONTROL, surface: 0 });
        face.on(&SMessage::BrowserMouseMove(x + 40.0, y, 0));
        face.paint();
        assert!(face.slip_view().is_some());
        let planted = Still { width: 2, height: 1, rgba: vec![7; 8] };
        face.slip_view.as_mut().unwrap().still = Some(planted.clone());
        face.paint();
        assert_eq!(face.slip_view(), Some(&planted), "same shown clip: no second render");
        face.on(&SMessage::BrowserKey("Escape".into()));
        face.paint();
        assert!(face.slip_view.is_none(), "no drag: cleared");
    }
}

// ---- STRIATRACKADDREMOVE (LEDGER SR856): `track-` names on the message line what it removed (named tail block) ----
// The `track+` / `track-` words are timeline_core's (`EditorState::verb`). A removal takes the track's clips with it, so a
// wrong press must be visible: after a `track-` the face's `note` (read by `message_text` after `last_error`) says
// `track-: removed <ch> (<n> clips)`, the channel word and clip count read before the one commit; a refusal reads
// `<word>: <EditError>` as every verb's does.
impl Face {
    /// A `track-` word through THE dispatch, its removal named on the message line; `None` for any other word.
    fn track_remove_verb(&mut self, v: &str) -> Option<bool> {
        let target = self.state.track_minus_target(v)?;
        let tl = &self.state.timeline;
        let named = target.ok().and_then(|t| tl.track_removed_note(t)); // STRIAKTRACKADD (SR9007): the note's one text, timeline_core's (lib.rs tail)
        match (self.state.verb(v)?, named) {
            (Ok(()), Some(note)) => self.note = Some(note),
            (Ok(()), None) => {}
            (Err(e), _) => self.last_error = Some(format!("{v}: {e}")),
        }
        Some(true)
    }
}

// ---- STRIALOUDNESSREADOUT (LEDGER SR866): the `loudness` word — the export range's integrated loudness on the message line, off the face's thread (named tail block) ----

/// STRIALOUDNESSREADOUT (SR866): a running measure, the export door's shape: a thread over clones of the timeline and
/// the shared sources (no lock), joined on the face clock ([`Face::loudness_tick`]).
struct LoudnessJob {
    handle: Option<std::thread::JoinHandle<Result<Option<crate::nle::LoudnessReading>, RenderError>>>, // STRIALOUDNESSNEXT (SR995): all three readings (file tail)
    /// The marked range measured (`None`: the whole timeline), named in the line.
    range: Option<std::ops::Range<i64>>, watch: crate::nle::LoudnessWatch, total: u64, shown: String, // STRIALOUDNESSCANCEL (SR903): the stop flag and ticks done, the range's ticks, the progress line last put up (file tail)
}

impl Face {
    /// The `loudness` word: measure the export's range ([`Face::export_range`], else the whole timeline) through
    /// `crate::nle::loudness` on its own thread. A second word cancels a running one (SR903); a range past the end is
    /// [`Face::range_check`]'s refusal. View-only: no undo step, the document untouched. `None` for any other word.
    fn loudness_verb(&mut self, v: &str) -> Option<bool> {
        if v != "loudness" {
            return None;
        }
        if self.loudness.is_some() {
            self.cancel_loudness();
            return Some(true);
        }
        let range = self.export_range();
        if let Err(e) = self.range_check(&range) {
            self.last_error = Some(format!("loudness: {e}"));
            return Some(true);
        }
        let span = range.clone().unwrap_or(0..self.state.timeline.end());
        let (tl, sources) = (self.state.timeline.clone(), Arc::clone(&self.sources)); let (watch, total) = (crate::nle::LoudnessWatch::new(&span), (span.end - span.start).max(0) as u64); let w = watch.clone(); // STRIALOUDNESSCANCEL (SR903)
        match std::thread::Builder::new().name("nle-loudness".into()).spawn(move || crate::nle::loudness_reading(&tl, span, &sources, Some(w))) { // STRIALOUDNESSNEXT (SR995): the measure with its range and true peak (file tail)
            Ok(h) => self.loudness = Some(LoudnessJob { handle: Some(h), range, watch, total, shown: String::new() }),
            Err(e) => self.last_error = Some(format!("loudness: {e}")),
        }
        Some(true)
    }
    /// True while a measure's result has not been reported.
    pub fn loudness_pending(&self) -> bool {
        self.loudness.is_some()
    }
    /// One clock tick for the measure: a finished one's line goes to the message line; true when that happened.
    pub fn loudness_tick(&mut self) -> bool {
        if !self.loudness.as_ref().is_some_and(|j| j.handle.as_ref().is_some_and(|h| h.is_finished())) {
            return self.loudness_progress(); // STRIALOUDNESSCANCEL (SR903): the running count on the message line (file tail)
        }
        self.finish_loudness();
        true
    }
    /// Block until the running measure ends and report it as [`Face::loudness_tick`] would; false when none runs.
    pub fn wait_loudness(&mut self) -> bool {
        if self.loudness.is_none() {
            return false;
        }
        self.finish_loudness();
        true
    }
    fn finish_loudness(&mut self) {
        let Some(mut j) = self.loudness.take() else { return };
        self.last_error = Some(match j.handle.take().map(|h| h.join()) {
            Some(Ok(_)) if j.watch.cancel.load(Relaxed) => "loudness: cancelled".to_string(), Some(Ok(r)) => loudness_reading_line(&r, &j.range), // STRIALOUDNESSNEXT (SR995): the range and true peak after the LUFS (file tail); STRIALOUDNESSCANCEL (SR903): the job's own flag names the cancel, as the export's
            _ => "loudness: the measure thread panicked".to_string(),
        });
    }
}

/// STRIALOUDNESSREADOUT (SR866): the measure's message line: `loudness: <x.x> LUFS integrated[ ticks a..b]`, the floor
/// naming the gate, `loudness: no audio in the range`, or the render's refusal.
pub fn loudness_line(r: &Result<Option<f64>, RenderError>, range: &Option<std::ops::Range<i64>>) -> String {
    use gneiss_pal::dsp::audio::meter::LUFS_FLOOR;
    match r {
        Ok(Some(l)) if *l <= LUFS_FLOOR => format!("loudness: {LUFS_FLOOR:.1} LUFS integrated{} (nothing above the {LUFS_FLOOR:.0} LUFS gate)", ticks_note(range)),
        Ok(Some(l)) => format!("loudness: {l:.1} LUFS integrated{}", ticks_note(range)),
        Ok(None) => "loudness: no audio in the range".to_string(),
        Err(e) => format!("loudness: {e}"),
    }
}

// ---- STRIAMETERSCLIPHOLD (LEDGER SR862): the meters' 2 s peak hold and the clip latch (named tail block) ----

pub use timeline_core::view::{METER_CLIP, METER_CLIP_DB, METER_CLIP_H, METER_HOLD_S, MeterHold, meter_rect}; // STRIAKMETER (SR9058): the hold, the clip latch and the one meter layout live in timeline_core::view, both faces call them (file tail)

/// The clip caps over a painted canvas: [`METER_CLIP`] across the top [`METER_CLIP_H`] px of each latched meter's rect,
/// playing or stopped. No latch paints nothing.
fn paint_meter_clips(px: Vec<u8>, l: Layout, order: PixelOrder, clipped: &[Option<usize>]) -> Vec<u8> {
    view::paint_meter_clips(px, l, vo(order), clipped) // STRIAKMETER (SR9058): the one clip cap paint, timeline_core::view (file tail)
}

impl Face {
    /// One face-clock tick of the meter hold: `METER_HOLD_S * fps` ticks, the readings taken only while playing.
    fn meter_hold_tick(&mut self) {
        let ticks = view::meter_hold_ticks(self.state.timeline.fps()); // STRIAKMETER (SR9058): the one hold length, both faces (file tail)
        let levels = self.playing.then(|| self.meters.levels());
        self.meter_hold.step(levels.as_deref(), ticks);
    }
    /// A peak hold still stands: `serve`'s clock keeps ticking until it expires.
    pub fn meter_hold_pending(&self) -> bool {
        self.meter_hold.pending()
    }
    /// The meters latched at the rail (`None` the master).
    pub fn meter_clipped(&self) -> &[Option<usize>] {
        self.meter_hold.clipped()
    }
    /// The latched meter whose bar holds `(x, y)`.
    fn meter_clip_at(&self, x: f64, y: f64) -> Option<Option<usize>> {
        let l = self.layout();
        self.meter_hold.clipped().iter().copied().find(|&k| {
            meter_rect(l, k).is_some_and(|(x0, y0, x1, y1)| x >= x0 as f64 && x < x1 as f64 && y >= y0 as f64 && y < y1 as f64)
        })
    }
    /// A click on a latched meter clears its latch (`Some(true)`); anywhere else `None` (the click it always was).
    fn meter_clip_reset(&mut self, x: f64, y: f64) -> Option<bool> {
        let key = self.meter_clip_at(x, y)?;
        Some(self.meter_hold.unclip(key))
    }
}

#[cfg(test)]
mod striameterscliphold_tests {
    use super::*;
    use gneiss_pal::dsp::audio::meter::dbfs;

    fn lv(track: Option<usize>, peak_db: f64) -> MeterLevel {
        MeterLevel { track, peak_db, rms_db: -40.0 }
    }

    /// M3: a -6 dB peak stands for `2 * fps - 1` ticks over a -20 dB reading and drops at the next; an equal reading does
    /// not re-arm it; a stopped face's ticks age it out.
    #[test]
    fn striameterscliphold_hold_keeps_the_peak_two_seconds() {
        let n = METER_HOLD_S * 25;
        let mut h = MeterHold::default();
        h.step(Some(&[lv(Some(1), -6.0)]), n);
        for k in 1..n {
            h.step(Some(&[lv(Some(1), -20.0)]), n);
            assert_eq!(h.shown(vec![lv(Some(1), -20.0)])[0].peak_db, -6.0, "tick {k}: held");
        }
        h.step(Some(&[lv(Some(1), -20.0)]), n);
        assert_eq!(h.shown(vec![lv(Some(1), -20.0)])[0].peak_db, -20.0, "tick {n}: the hold expired");
        let mut p = MeterHold::default();
        p.step(Some(&[lv(None, -6.0)]), n);
        for _ in 0..10 {
            p.step(Some(&[lv(None, -6.0)]), n);
        }
        assert_eq!(p.held[0].2, n - 10, "an equal reading does not re-arm");
        for _ in 0..n {
            p.step(None, n);
        }
        assert!(!p.pending() && p.clipped().is_empty());
    }

    /// M3: no hold state is the identity: the readings and the painted pixels are unchanged, so the
    /// `nle4c_meter_strip_golden` paint (GOLDEN_METERS) is the same bytes through the new road.
    #[test]
    fn striameterscliphold_no_hold_is_identity() {
        let mut st = EditorState::new(synth_fixture().0);
        st.seek(15);
        let l = Layout::of(320, 160, &st);
        let base = paint(&st, 320, 160, PixelOrder::Rgba, None, None);
        let levels = vec![lv(Some(1), -6.0), lv(None, 0.0)];
        let h = MeterHold::default();
        assert_eq!(h.shown(levels.clone()), levels);
        let bars = paint_meters(base, l, PixelOrder::Rgba, &h.shown(levels.clone()));
        assert_eq!(paint_meter_clips(bars.clone(), l, PixelOrder::Rgba, h.clipped()), bars);
    }

    /// M3: +24 dB drives track 1 and the master to the rail: both latch; the caps paint stopped; a click on the master's
    /// bar clears only it, a press + release on track 1's clears it with the playhead and selection unmoved; with no
    /// latch the strip takes the click as before (a seek by `Layout::hit`). The threshold sits between the rail and the
    /// code below it.
    #[test]
    fn striameterscliphold_clip_latches_and_click_clears() {
        assert!(dbfs(32767.0 / 32768.0) >= METER_CLIP_DB, "a full-scale sample latches");
        assert!(dbfs(32766.0 / 32768.0) < METER_CLIP_DB, "one LSB under does not");
        let (mut tl, src) = synth_fixture();
        tl.tracks[1].clips.iter_mut().for_each(|c| *c = c.gained(2400));
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        face.verb("play");
        face.tick();
        face.tick();
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        assert_eq!(face.meter_clipped(), &[Some(1), None]);
        face.verb("play");
        assert!(!face.playing);
        let l = face.layout();
        let row = l.row_y(1);
        let px = face.paint();
        let at = |x: usize, y: usize| px[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4].to_vec();
        assert_eq!(at(316, row as usize + 2), METER_CLIP.to_vec());
        assert_eq!(at(316, 1), METER_CLIP.to_vec());
        let (ph, sel) = (face.state.playhead, face.state.selection);
        assert!(face.on(&SMessage::BrowserClick(316.0, 30.0, 0)));
        assert_eq!(face.meter_clipped(), &[Some(1)]);
        let y = row as f64 + 10.0;
        assert!(!face.on(&SMessage::BrowserButton { down: true, x: 316.0, y, mods: 0, surface: 0 }));
        assert!(face.on(&SMessage::BrowserButton { down: false, x: 316.0, y, mods: 0, surface: 0 }));
        assert!(face.meter_clipped().is_empty());
        assert_eq!((face.state.playhead, face.state.selection), (ph, sel), "a latch click moves nothing");
        let px = face.paint();
        assert_ne!(px[((row as usize + 2) * 320 + 316) * 4..((row as usize + 2) * 320 + 316) * 4 + 4], METER_CLIP, "cleared");
        let t = match l.hit(&face.state.timeline, 316.0, y) {
            Hit::Clip(_, _, t) | Hit::Track(_, t) | Hit::Ruler(t) => t,
            Hit::None => panic!("the strip lies over a track row"),
        };
        assert!(face.on(&SMessage::BrowserButton { down: true, x: 316.0, y, mods: 0, surface: 0 }) | face.on(&SMessage::BrowserButton { down: false, x: 316.0, y, mods: 0, surface: 0 }));
        assert_eq!(face.state.playhead, t.min(face.state.timeline.end()), "no latch: the strip's click is the click it always was");
    }

    /// M3: a hold set by played ticks keeps the face clock asked for (`meter_hold_pending`); stopped, `2 * fps` ticks ask
    /// no repaint and age it out.
    #[test]
    fn striameterscliphold_idle_hold_expires() {
        let (tl, src) = synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert!(!face.meter_hold_pending());
        face.verb("play");
        face.tick();
        face.tick();
        assert!(face.meter_hold_pending());
        face.verb("play");
        let n = METER_HOLD_S * 25;
        assert!((0..n).all(|_| !face.tick()), "an idle hold asks no repaint");
        assert!(!face.meter_hold_pending());
    }
}

// ---- STRIAJKLTRANSPORT (LEDGER SR873): the forward shuttle heard at 2x / 4x through audio_core's WSOLA (named tail block) ----

/// STRIAJKLTRANSPORT (SR873): one stretch stage's ceiling: audio_core's `stretch::MAX_RATE` (4.0), the ceiling of the WSOLA
/// stretch the media Player's `set_rate` uses. A faster shuttle (8x) runs two stages ([`ShuttleStretch`], STRIAJKLMORE SR945).
const SHUTTLE_HEARD: i64 = gneiss_pal::dsp::audio::stretch::MAX_RATE as i64;

impl Face {
    /// STRIAJKLTRANSPORT (SR873): a shuttle at 2x..=8x either way (STRIAJKLMORE SR945) with a device keeps ONE
    /// [`ShuttleStretch`] at |rate| (pitch kept; a rate change takes effect at its next hop); 1x, -1x and stop drop it, so the
    /// 1x path is unchanged.
    fn shuttle_stretch(&mut self) {
        if self.playing && self.play.out.is_some() && (2..=SHUTTLE_MAX).contains(&self.rate.abs()) {
            let (hz, r) = (self.play.rate, self.rate.abs() as f64);
            self.play.stretch.get_or_insert_with(|| ShuttleStretch::new(hz)).set_rate(r);
        } else {
            self.play.stretch = None;
        }
    }
}

/// STRIAJKLTRANSPORT (SR873): a played tick's stereo on its way to the device: unchanged with no stretch (1x), else pushed
/// through the shuttle's [`ShuttleStretch`] and what it yields so far (a 2x tick's two frames of audio come out as about one).
fn shuttle_pcm(w: Option<&mut ShuttleStretch>, stereo: Vec<f32>) -> Vec<f32> {
    let Some(w) = w else { return stereo };
    w.run(&stereo)
}

#[cfg(test)]
mod striajkltransport_tests {
    use super::*;
    use crate::media::AudioOut;
    use crate::nle::SYNTH_RATE;
    use std::sync::{Arc, Mutex};

    /// The device's calls: (frames pushed, paused, flushes).
    #[derive(Default)]
    struct Dev {
        pushed: Vec<usize>,
        paused: Vec<bool>,
        flushes: usize,
    }
    struct Out(Arc<Mutex<Dev>>);
    impl AudioOut for Out {
        fn push(&mut self, stereo: &[f32]) -> usize {
            self.0.lock().unwrap().pushed.push(stereo.len() / 2);
            stereo.len()
        }
        fn consumed(&self) -> u64 {
            self.0.lock().unwrap().pushed.iter().sum::<usize>() as u64
        }
        fn flush(&mut self) {
            self.0.lock().unwrap().flushes += 1;
        }
        fn set_paused(&mut self, p: bool) {
            self.0.lock().unwrap().paused.push(p);
        }
    }

    fn face() -> (Face, Arc<Mutex<Dev>>) {
        let (tl, src) = synth_fixture();
        let dev = Arc::new(Mutex::new(Dev::default()));
        (Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_audio(Box::new(Out(dev.clone())), SYNTH_RATE), dev)
    }

    fn key(f: &mut Face, k: &str) {
        assert!(f.on(&SMessage::BrowserText(k.into())));
    }

    /// STRIAJKLTRANSPORT M3: L doubles 1, 2, 4, 8 and holds at 8; J mirrors it; L while reversing and J while going forward
    /// start over at 1x; K stops at rate 1; a 4x tick steps 4 frames; the end clamps and stops.
    #[test]
    fn striajkltransport_ladder() {
        let (mut f, _) = face();
        let mut seen = Vec::new();
        for _ in 0..5 {
            key(&mut f, "l");
            seen.push(f.rate);
        }
        assert_eq!(seen, vec![1, 2, 4, 8, 8]);
        key(&mut f, "j");
        assert_eq!((f.playing, f.rate), (true, -1), "J while going forward starts reverse at 1x");
        seen.clear();
        for _ in 0..4 {
            key(&mut f, "j");
            seen.push(f.rate);
        }
        assert_eq!(seen, vec![-2, -4, -8, -8]);
        key(&mut f, "l");
        assert_eq!((f.playing, f.rate), (true, 1), "L while reversing starts forward at 1x");
        key(&mut f, "k");
        assert_eq!((f.playing, f.rate), (false, 1));

        f.state.seek(10);
        key(&mut f, "l");
        key(&mut f, "l");
        key(&mut f, "l");
        assert_eq!(f.rate, 4);
        assert!(f.tick() && f.tick());
        assert_eq!(f.state.playhead, 14, "the first tick is shown at its own playhead, the next steps 4");
        let end = f.state.timeline.end();
        f.state.seek(end - 2);
        while f.tick() {}
        assert_eq!((f.playing, f.state.playhead), (false, end), "the end clamps and stops");
    }

    /// STRIAJKLTRANSPORT M3: 2x and 4x stay released (no pause, no flush on the step up) and keep the device in real time
    /// (N ticks push N x 1920 frames within one WSOLA frame); 8x is released too and pushes (STRIAJKLMORE SR945, seat-granted:
    /// it pinned the owed silence); back at 1x every tick pushes exactly 1920 again (the stretch dropped).
    #[test]
    fn striajkltransport_audio_follows() {
        let per = (SYNTH_RATE / 25) as usize;
        let wsola_n = (SYNTH_RATE as usize / 50) & !1;
        for (keys, rate) in [(2, 2), (3, 4)] {
            let (mut f, dev) = face();
            key(&mut f, "l");
            assert!(f.tick());
            let base = { let d = dev.lock().unwrap(); (d.pushed.iter().sum::<usize>(), d.paused.clone(), d.flushes) };
            assert_eq!(base.0, per);
            for _ in 1..keys {
                key(&mut f, "l");
            }
            assert_eq!(f.rate, rate);
            assert!(f.played().live, "{rate}x is released");
            let n = 6;
            for _ in 0..n {
                assert!(f.tick());
            }
            let d = dev.lock().unwrap();
            assert_eq!((d.paused.clone(), d.flushes), (base.1.clone(), base.2), "{rate}x: no pause, no flush");
            let got = d.pushed.iter().sum::<usize>() - base.0;
            assert!(got + 2 * wsola_n >= n * per && got <= n * per + wsola_n, "{rate}x: {got} frames for {n} ticks");
        }

        let (mut f, dev) = face();
        for _ in 0..4 {
            key(&mut f, "l");
        }
        assert_eq!(f.rate, 8);
        assert!(f.played().live, "STRIAJKLMORE (SR945): 8x is released (two stretch stages)");
        assert_eq!(dev.lock().unwrap().paused.last(), Some(&false));
        let before = dev.lock().unwrap().pushed.iter().sum::<usize>();
        assert!(f.tick() && f.tick() && f.tick());
        assert!(dev.lock().unwrap().pushed.iter().sum::<usize>() > before, "STRIAJKLMORE (SR945): 8x pushes");
        assert!(f.play.stretch.is_some());

        let (mut f, dev) = face();
        key(&mut f, "l");
        key(&mut f, "l");
        assert!(f.tick() && f.tick());
        assert!(f.play.stretch.is_some());
        key(&mut f, "k");
        assert!(f.play.stretch.is_none());
        key(&mut f, "l");
        dev.lock().unwrap().pushed.clear();
        assert!(f.tick() && f.tick() && f.tick());
        assert_eq!(dev.lock().unwrap().pushed, vec![per; 3], "1x after a shuttle is today's path");
    }

    /// STRIAJKLTRANSPORT M3, rewritten by STRIAJKLMORE (SR945, seat-granted: it pinned the owed silence): J keeps the device
    /// released (no pause), its first tick flushes the ring once (the direction change) and its ticks push; a face with no
    /// device never builds a stretch.
    #[test]
    fn striajkltransport_reverse_silent() {
        let (mut f, dev) = face();
        f.state.seek(20);
        key(&mut f, "l");
        assert!(f.tick());
        let flushes = dev.lock().unwrap().flushes;
        key(&mut f, "j");
        assert_eq!((f.playing, f.rate, f.played().live), (true, -1, true));
        let (pushed, last, fl) = { let d = dev.lock().unwrap(); (d.pushed.iter().sum::<usize>(), d.paused.last().copied(), d.flushes) };
        assert_eq!((last, fl), (Some(false), flushes), "STRIAJKLMORE (SR945): J stays released");
        key(&mut f, "j");
        assert!(f.tick() && f.tick());
        assert_eq!(dev.lock().unwrap().flushes, flushes + 1, "STRIAJKLMORE (SR945): the direction change flushes once");
        assert!(dev.lock().unwrap().pushed.iter().sum::<usize>() > pushed, "STRIAJKLMORE (SR945): reverse pushes");
        assert!(f.play.stretch.is_some(), "-2x stretches");

        let (tl, src) = synth_fixture();
        let mut mute = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        key(&mut mute, "l");
        key(&mut mute, "l");
        assert!(mute.tick());
        assert!(mute.play.stretch.is_none());
    }
}

// ---- STRIAHELPSEARCH (LEDGER SR874): help <word> filters the keys help rows (named tail block) ----
// `help <word>` (the words after `help`, joined by one space) shows the keys help with only THE `help_rows` whose keys or
// phrase contain the query, ASCII case-insensitive; `paint_help` paints them through its one column / `…` road. No row
// matches: refused by name on the one message line (`help: no key matches "<q>"`), the help left as it was. Bare `help`
// (`?`, the button) is SR744's toggle unchanged and clears the query, so it always shows the whole table. View-only: no
// undo step, the document not dirtied. Owed: NLE5D-KEYS-HELP-SEARCH-ACTION (match the action word too: help_rows drops it),
// NLE5D-KEYS-HELP-SEARCH-LIVE (type to filter while shown), NLE5D-KEYS-HELP-SEARCH-BUTTON (a query field on the bar).

/// STRIAHELPSEARCH (SR874): [`help_rows`] whose keys or phrase contain `q` (ASCII case-insensitive).
pub fn help_matching(q: &str) -> Vec<(String, &'static str)> { help_matching_in(help_rows(), q) }
/// The fold of SR874 / SR941 with SR886: the same filter over any row list (the face's key overlay rows).
pub fn help_matching_in(rows: Vec<(String, &'static str)>, q: &str) -> Vec<(String, &'static str)> {
    let q = q.to_ascii_lowercase();
    rows.into_iter().filter(|(k, p)| k.to_ascii_lowercase().contains(&q) || p.to_ascii_lowercase().contains(&q)).collect()
}

impl Face {
    /// The `help <word>` form: show the help filtered by the query; a query no row matches is refused by name and the help
    /// left as it was; a blank query is the bare `help`. `None` for any other word (bare `help` included).
    fn help_search_verb(&mut self, v: &str) -> Option<bool> {
        let mut words = v.split_whitespace();
        if words.next() != Some("help") || v == "help" {
            return None;
        }
        let q = words.collect::<Vec<_>>().join(" ");
        if q.is_empty() {
            return Some(self.verb("help"));
        }
        if help_matching(&q).is_empty() {
            self.last_error = Some(format!("help: no key matches \"{q}\""));
            return Some(true);
        }
        self.help = true;
        self.help_query = Some(q);
        Some(true)
    }
    /// The query the help is filtered by (`help <word>`); `None` for the whole table.
    pub fn help_query(&self) -> Option<&str> {
        self.help_query.as_deref()
    }
    /// The rows the help shows: [`help_matching`] the query, or [`help_rows`] verbatim without one.
    pub fn help_rows_shown(&self) -> Vec<(String, &'static str)> {
        match &self.help_query {
            Some(q) => help_matching_in(help_rows_with(&self.keymap), q),
            None => help_rows_with(&self.keymap),
        }
    }
}

// ---- STRIASOURCEINFO (LEDGER SR875): the info word's message line, standing INFO_SECS on the face clock (named tail block) ----
// The idle message already names the selected clip's reel (`reel_line`, SR743), but any hover (SR762 / SR789) or a running
// export's line (SR803) replaces it, and `last_error` is taken after the repaint it causes, so no message stands. `info`
// makes the selected clip's reel line STAND for INFO_SECS: counted in face ticks on THE face clock (`Face::tick`, as the
// SR789 hover dwell is; `serve` runs its clock while `info_pending`), over the hover and export lines, under a fresh error,
// ended by a selection change. View-only: no undo step, the document not dirtied. Owed: STRIA-SOURCE-INFO-KERNEL (the
// kernel face's word), STRIA-SOURCE-INFO-BIN (a source bin's selection, not the clip's), STRIA-SOURCE-INFO-MS (the
// seconds in wall time) and STRIA-SOURCE-INFO-KEY (Ctrl+i / Meta+i) BUILT by SR942 (STRIASOURCEINFOKEY tail block).

/// STRIASOURCEINFO (SR875): the seconds an `info` line stands: wall time, or this times the timeline's frame rate in face ticks, whichever ends first (STRIASOURCEINFOKEY SR942).
pub const INFO_SECS: u32 = 5;

/// STRIASOURCEINFO (SR875): a standing `info` line: the selection it names, its text, the face ticks it has left.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceInfo {
    sel: (usize, usize),
    text: String,
    left: u32,
    since: std::time::Instant, // STRIASOURCEINFOKEY (SR942): when the line was stood (the wall deadline is since + INFO_SECS)
}

impl Face {
    /// STRIAINFOREEL (SR743) / STRIASOURCEINFO (SR875): clip `c`'s reel line, `<reel name>: <summary>` (`nle::Sources::summary`);
    /// `None` for a reel with nothing to say. THE one formatter `reel_line` and `info` read.
    pub fn clip_reel_line(&self, c: Clip) -> Option<String> {
        let s = self.pcache.summary(&self.sources, c.src); // STRIAPERFPREVIEWCACHE (SR918): read once per reel (file tail)
        let name = self.state.timeline.source(c.src).map_or_else(|| c.src.0.to_string(), |x| x.reel.clone());
        reel_words(name, s, grade_note(c.grade)) // STRIAGRADEMORE (SR957): the summary-less arm (file tail); STRIAGRADEFACE (SR899): the clip's grade while not identity
    }
    /// STRIASOURCEINFO (SR875): the `info` word: the selected clip's reel line as `info: <reel>: <summary>`, standing
    /// [`INFO_SECS`] (a new `info` restarts it). Refused on the message line: nothing selected `info: no such clip`
    /// (`EditError::NoClip`), a reel with no summary `info: <reel>: no media summary`. `None` for any other word.
    fn info_verb(&mut self, v: &str) -> Option<bool> {
        if v != "info" {
            return None;
        }
        let (Some(sel), Some(c)) = (self.state.selection, self.state.selected()) else {
            self.last_error = Some(format!("info: {}", EditError::NoClip));
            return Some(true);
        };
        match self.clip_reel_line(c) {
            Some(line) => {
                let left = INFO_SECS * self.state.timeline.fps().unwrap_or(25).max(1);
                self.info = Some(SourceInfo { sel, text: format!("info: {line}"), left, since: std::time::Instant::now() }); // STRIASOURCEINFOKEY (SR942): the wall stamp
            }
            None => {
                let name = self.state.timeline.source(c.src).map_or_else(|| c.src.0.to_string(), |x| x.reel.clone());
                self.last_error = Some(format!("info: {name}: no media summary"));
            }
        }
        Some(true)
    }
    /// STRIASOURCEINFO (SR875): the standing `info` line, only while the selection it names is still the selection.
    pub fn info_text(&self) -> Option<String> {
        self.info.as_ref().filter(|i| self.state.selection == Some(i.sel)).map(|i| i.text.clone())
    }
    /// STRIASOURCEINFO (SR875): one face tick of a standing line: its count falls; at 0, or once the selection changed,
    /// it ends. True only on the tick it ends (the message repaints).
    fn info_tick(&mut self) -> bool { self.info_tick_at(std::time::Instant::now()) } // STRIASOURCEINFOKEY (SR942): the wall read on the clock path, as export_tick's (file tail)
    /// STRIASOURCEINFO (SR875): an `info` line stands: `serve` runs its clock.
    pub fn info_pending(&self) -> bool {
        self.info.is_some()
    }
}

// ---- STRIASOURCEINFOKEY (LEDGER SR942): Ctrl / Cmd+I, and the line's seconds in wall time (named tail block) ----
// `info` gains its keys: KEYS rows `Ctrl+i` / `Meta+i` (`i` is `mark-in`, `I` is `mark in`) reach `info_verb` through
// `Face::key`'s VERBS road, with a HELP phrase. SR875 counted the line's INFO_SECS in face ticks only, and `serve`'s clock
// drops the ticks it misses while the face thread is busy (`MissedTickBehavior::Skip`), so on a busy face the line outstayed
// five seconds. The line now ends at the FIRST of INFO_SECS of wall time (stamped at the word, read on the clock path as
// `export_tick` reads it, never in paint), INFO_SECS x fps face ticks (the witness's `tick:N` clock), or a selection change.
// Owed: STRIA-SOURCE-INFO-KEY-MAC (the AppKit surface forwards only Cmd+C/A/S/O: `Meta+i` does not arrive there, as `Meta+z`).

impl Face {
    /// STRIASOURCEINFOKEY (SR942): one face tick of a standing `info` line at wall time `now`: its count falls; at 0, at
    /// `since + INFO_SECS`, or once the selection changed, it ends. True only on the tick it ends (the message repaints).
    pub fn info_tick_at(&mut self, now: std::time::Instant) -> bool {
        let Some(i) = self.info.as_mut() else { return false };
        i.left = i.left.saturating_sub(1);
        if i.left == 0 || now >= i.since + Duration::from_secs(INFO_SECS as u64) || self.state.selection != Some(i.sel) {
            self.info = None;
            return true;
        }
        false
    }
    
}

// ---- STRIAEXPORTQUEUE (LEDGER SR881): queued export requests run in order from export_tick (named tail block) ----
// A second `export` while one runs no longer refuses `busy`: `export_verb`'s parsed request (dir, format word, range, audio)
// waits in `Face.export_queue` (face thread only: the export thread never sees it, no lock) and `export_tick` starts the
// next one through the unchanged `start_export` the moment the running job is reported, so `exporting()` never drops
// between jobs and serve's clock needs no new condition. The running job (`ExportJob`) and its line (`job_line`) are
// unchanged. `export: busy (d/t)` now means running AND `EXPORT_QUEUE_MAX` waiting. Esc (`cancel_export`) cancels the
// running job and drops the queue; `Face::export_skip` cancels the running job only. A queued job exports the timeline
// as it stands when it STARTS (`start_export`'s clone); its range is the one the verb resolved. SR871 (presets) joins as
// ONE more `ExportRequest` field `opts: H264Opts`, set from `export_or_queue`'s new last parameter and passed as
// `start_export`'s last argument in both calls below. Owed: NLE5D-EXPORT-QUEUE-SNAPSHOT, NLE5D-EXPORT-QUEUE-SKIP,
// NLE5D-EXPORT-QUEUE-WITNESS, NLE5D-EXPORT-QUEUE-LINE.

/// STRIAEXPORTQUEUE (SR881): the most export requests that wait behind a running job; one more is refused `export: busy (d/t)`.
pub const EXPORT_QUEUE_MAX: usize = 16;

/// STRIAEXPORTQUEUE (SR881): one parsed `export` request waiting its turn: exactly [`Face::start_export`]'s arguments
/// but the test hold (`pause_at`), as `export_verb` resolved them (the range captured at the verb, `--all` as `None`).
#[derive(Debug, Clone)]
pub struct ExportRequest {
    dir: std::path::PathBuf,
    fmt: String,
    range: Option<std::ops::Range<i64>>,
    audio: AudioOut,
    opts: H264Opts, // STRIAEXPORTPRESETS (SR871) at the fold: a queued request carries its preset's H.264 options
    timeline: Timeline, // STRIAEXPORTQUEUESKIP (SR914): the timeline as it stood when queued, swapped in around start_export (file tail)
}

impl Face {
    /// The verb's start: with no job running, [`Face::start_export`] as before; with one running, the request is checked
    /// at once (the format word and [`Face::range_check`], refused by the same words) and queued, the message line
    /// `export: queued (<n> waiting)`.
    fn export_or_queue(&mut self, dir: &std::path::Path, fmt: &str, range: Option<std::ops::Range<i64>>, audio: AudioOut, opts: H264Opts) -> Result<(), String> {
        if self.export.is_none() {
            return self.start_export(dir, fmt, None, range, audio, opts);
        }
        if !VIDEO_NAMES.contains(&fmt) {
            return Err(format!("unknown format {fmt} ({})", VIDEO_NAMES.join("|")));
        }
        self.range_check(&range)?;
        self.export_queue.push_back(ExportRequest { dir: dir.to_path_buf(), fmt: fmt.to_string(), range, audio, opts, timeline: self.state.timeline.clone() }); // STRIAEXPORTQUEUESKIP (SR914): the queued snapshot
        self.last_error = Some(format!("export: queued ({} waiting)", self.export_queue.len()));
        Ok(())
    }
    /// Start the next queued request through [`Face::start_export`] (no job runs: called after `finish_export`). A request
    /// that no longer starts (its directory; its range and timeline are the snapshot taken when queued, SR914) is named after the finished job's line
    /// (`; queued <dir>: <e>`) and the next one is tried, until one starts or the queue is empty.
    fn start_queued(&mut self) {
        while self.export.is_none() {
            let Some(mut r) = self.export_queue.pop_front() else { return }; // STRIAEXPORTQUEUESKIP (SR914): mutable for the timeline swap
            if let Err(e) = self.start_request(&mut r) { // STRIAEXPORTQUEUESKIP (SR914): the request's own timeline (file tail)
                let prev = self.last_error.take().map(|l| format!("{l}; ")).unwrap_or_else(|| "export: ".to_string());
                self.last_error = Some(format!("{prev}queued {}: {e}", r.dir.display()));
            }
        }
    }
    /// The test seam over the whole queue: [`Face::wait_export`] the running job, start the next queued request, until
    /// none is left; the number of jobs reported (each one's line on `last_error` in turn, the last one standing).
    pub fn wait_exports(&mut self) -> usize {
        let mut n = 0;
        while self.wait_export() {
            n += 1;
            self.start_queued();
        }
        n
    }
    /// The number of export requests waiting behind the running job.
    pub fn export_queued(&self) -> usize {
        self.export_queue.len()
    }
    /// Cancel the running export only (its next frame refuses, as Esc's [`Face::cancel_export`]), the queue kept: the
    /// next request starts when this one is reported. False when none runs. (NLE5D-EXPORT-QUEUE-SKIP owes its key / word.)
    pub fn export_skip(&mut self) -> bool {
        let Some(j) = self.export.as_ref() else { return false };
        j.cancel.store(true, Relaxed);
        true
    }
}

// ---- STRIAMETERCLIPVERB (LEDGER SR902): the meters-reset word (named tail block) ----

impl Face {
    /// `meters-reset`: clear every clip latch and peak hold (`Some(true)`, a repaint; a view word: no undo step, the
    /// document not dirtied); `None` for any other word.
    fn meter_reset_verb(&mut self, v: &str) -> Option<bool> {
        if v != "meters-reset" {
            return None;
        }
        self.meter_hold.clear();
        Some(true)
    }
}

#[cfg(test)]
mod striameterclipverb_tests {
    use super::*;

    fn clipping_face() -> Face {
        let (mut tl, src) = synth_fixture();
        tl.tracks[1].clips.iter_mut().for_each(|c| *c = c.gained(2400));
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
    }

    /// M3: +24 dB latches track 1 and the master and sets holds; `meters-reset` clears both, pushes no undo step,
    /// does not dirty the document or move the timeline, and the stopped paint is a never-latched face's.
    #[test]
    fn striameterclipverb_reset_clears_latches_and_holds() {
        let mut face = clipping_face();
        let mut never = clipping_face();
        face.verb("play");
        face.tick();
        face.tick();
        face.verb("play");
        assert!(!face.playing);
        assert_eq!(face.meter_clipped(), &[Some(1), None]);
        assert!(face.meter_hold_pending());
        let (undo, dirty, tl, ph) = (face.state.undo_depth(), face.dirty(), face.state.timeline.clone(), face.state.playhead);
        assert!(face.verb("meters-reset"));
        assert!(face.meter_clipped().is_empty());
        assert!(!face.meter_hold_pending());
        assert_eq!((face.state.undo_depth(), face.dirty(), &face.state.timeline), (undo, dirty, &tl));
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        never.state.seek(ph);
        assert_eq!(face.paint(), never.paint(), "a reset strip paints as one that never latched");
    }

    /// M3: the word sits immediately before `help` (help last); with no latch it is taken and changes no pixel.
    #[test]
    fn striameterclipverb_word_sits_before_help() {
        assert_eq!(VERBS[VERBS.len() - 2], "meters-reset");
        assert_eq!(VERBS.last(), Some(&"help"));
        let (tl, src) = synth_fixture();
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let before = face.paint();
        assert!(face.verb("meters-reset"));
        assert_eq!(face.paint(), before);
        assert_eq!(face.state.undo_depth(), 0);
        assert!(!face.dirty());
    }
}

// ---- STRIAFACEGOLDENS (LEDGER SR891): the face golden table at three sizes, one text file, the re-pin road (named tail block) ----
// Seven labels-off RGBA scenes through the ONE painter (`paint` / `paint_in` -> timeline_core `view::paint_in`) at 320x160,
// 1280x720 and 1920x1200, each hashed with `view::fnv1a`; the expected values live in ONE checked-in text file,
// `tests/face_goldens.txt` (`name WxH hash`). `UNAOS_REPIN=1` makes the test rewrite that file from what it measures; a row
// whose hash is `unpinned` prints its measured value and passes. Labels-on paints (the clip names, ruler timecodes, message
// line, help overlay) lean on the host's installed font and are kept out. The six older consts stay where they are.

#[cfg(test)]
mod facegoldens_tests {
    use super::*;

    /// The three canvas sizes every scene is painted at.
    const SIZES: [(u32, u32); 3] = [(320, 160), (1280, 720), (1920, 1200)];

    /// The scenes, in the file's row order.
    const SCENES: [&str; 7] = ["tracks", "set", "transitions", "to-black", "meters", "preview", "keys-tracks"];

    fn table_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("face_goldens.txt")
    }

    fn state() -> (EditorState, Sources) {
        let (tl, src) = synth_fixture();
        (EditorState::new(tl), src)
    }

    /// One scene's RGBA paint on a `w` x `h` canvas: each older golden's set-up as its test has it, plus `preview` (the
    /// frame at 15 scaled into the band) and `keys-tracks` (the kernel keys fixture's tracks band, view.rs `keys_state`).
    fn scene(name: &str, w: u32, h: u32) -> Vec<u8> {
        match name {
            "tracks" => {
                let (mut st, _) = state();
                st.select(0, 1).unwrap();
                st.seek(15);
                paint(&st, w, h, PixelOrder::Rgba, None, None)
            }
            "set" => {
                let (mut st, _) = state();
                st.select(0, 0).unwrap();
                st.select_add(0, 2).unwrap();
                st.seek(15);
                paint(&st, w, h, PixelOrder::Rgba, None, None)
            }
            "transitions" => {
                let (mut st, _) = state();
                let tl = st.timeline.add_transition(0, 10, TransitionKind::Dissolve, 4).unwrap();
                st.timeline = tl.add_transition_from_black(0, 0, TransitionKind::Wipe(WipeDir::Box), 3).unwrap();
                st.select_transition(0, 1).unwrap();
                st.seek(15);
                paint(&st, w, h, PixelOrder::Rgba, None, None)
            }
            "to-black" => {
                let (mut st, _) = state();
                st.timeline = st.timeline.add_transition_to_black(0, 20, TransitionKind::Dissolve, 5).unwrap();
                paint(&st, w, h, PixelOrder::Rgba, None, None)
            }
            "meters" => {
                let mut st = EditorState::new(synth_fixture().0);
                st.seek(15);
                let l = Layout::of(w, h, &st);
                let base = paint(&st, w, h, PixelOrder::Rgba, None, None);
                let levels = [
                    MeterLevel { track: Some(1), peak_db: -6.0, rms_db: -30.0 },
                    MeterLevel { track: None, peak_db: 0.0, rms_db: -15.0 },
                ];
                paint_meters(base, l, PixelOrder::Rgba, &levels)
            }
            "preview" => {
                let (mut st, src) = state();
                st.seek(15);
                let still = preview(&st.timeline, &src, 15).unwrap();
                paint(&st, w, h, PixelOrder::Rgba, None, still.as_ref())
            }
            "keys-tracks" => {
                let mut tl = Timeline::new(25);
                let a = tl.add_source("NLEFXA", 40);
                tl.add_track(Kind::Video);
                tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
                let mut st = EditorState::new(tl.split(0, 8).unwrap());
                st.select_at(0, 8).unwrap();
                st.seek(8);
                let l = Layout::of_role(Role::Tracks, w, h, &st);
                paint_in(&st, l, PixelOrder::Rgba, None, None, None)
            }
            _ => panic!("striafacegoldens: no scene {name}"),
        }
    }

    /// The file's rows: `name WxH hash`, blank and `#` lines skipped.
    fn parse(text: &str) -> Vec<(String, (u32, u32), String)> {
        let mut rows = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(f.len(), 3, "striafacegoldens: a row is `name WxH hash`: {line:?}");
            let (w, h) = f[1].split_once('x').unwrap_or_else(|| panic!("striafacegoldens: size `WxH`: {line:?}"));
            let size = (w.parse().expect("striafacegoldens: width"), h.parse().expect("striafacegoldens: height"));
            rows.push((f[0].to_string(), size, f[2].to_string()));
        }
        rows
    }

    const HEADER: &str = "# STRIAFACEGOLDENS (LEDGER SR891): the face golden table, `name WxH hash` (FNV-1a 64, view::fnv1a).\n\
# Labels-off RGBA paints through nle_face `paint` / `paint_in` (timeline_core view::paint_in); scenes in nle_face.rs `facegoldens_tests::scene`.\n\
# Re-pin: UNAOS_REPIN=1 cargo test -p stria --lib striafacegoldens_table   (a hash of `unpinned` prints its value and passes)\n";

    /// SR891: every scene at every size hashes to its row in `tests/face_goldens.txt`; `UNAOS_REPIN=1` rewrites the file.
    #[test]
    fn striafacegoldens_table() {
        let mut got: Vec<(String, (u32, u32), String)> = Vec::new();
        for name in SCENES {
            for (w, h) in SIZES {
                got.push((name.to_string(), (w, h), format!("{:016x}", fnv1a(&scene(name, w, h)))));
            }
        }
        let path = table_path();
        if std::env::var("UNAOS_REPIN").is_ok_and(|v| v == "1") {
            let mut out = String::from(HEADER);
            for (name, (w, h), hash) in &got {
                out.push_str(&format!("{name} {w}x{h} {hash}\n"));
            }
            std::fs::write(&path, out).unwrap_or_else(|e| panic!("striafacegoldens: write {}: {e}", path.display()));
            println!("striafacegoldens: repinned {} rows in {}", got.len(), path.display());
            return;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("striafacegoldens: read {}: {e}", path.display()));
        let want = parse(&text);
        let mut bad = Vec::new();
        for (name, (w, h), hash) in &got {
            match want.iter().find(|r| &r.0 == name && r.1 == (*w, *h)) {
                None => bad.push(format!("{name} {w}x{h} missing from the file (got {hash})")),
                Some((_, _, x)) if x == "unpinned" => println!("striafacegoldens: unpinned {name} {w}x{h} {hash}"),
                Some((_, _, x)) if x != hash => bad.push(format!("{name} {w}x{h} want {x} got {hash}")),
                Some(_) => {}
            }
        }
        for (name, (w, h), _) in &want {
            if !got.iter().any(|r| &r.0 == name && r.1 == (*w, *h)) {
                bad.push(format!("{name} {w}x{h} in the file but no scene paints it"));
            }
        }
        assert!(bad.is_empty(), "striafacegoldens: {} row(s) off:\n{}", bad.len(), bad.join("\n"));
    }
}

// ---- STRIAEXPORTAACBITRATE (LEDGER SR887): a bad aac rate refused by name (named tail block) ----
/// The `export` verb's bad AAC rate, refused by name before any job: of two or three words, a last word `aac:<k>` that the
/// audio table ([`crate::nle_export::audio_named`]) refuses (`aac:0`, `aac:x`, `aac:`) is popped and named
/// (`aac rate "<k>": aac:KBPS takes a whole number of kb/s above 0; the words are <usage>`). Without it the two-word form
/// `export <dir> aac:0` fell to the format slot (`unknown format aac:0`). Every other word: `None` (the grammar unchanged).
fn striaexportaacbitrate_refusal(args: &mut Vec<&str>) -> Option<String> {
    let k = match args.last() {
        Some(w) if (2..=3).contains(&args.len()) && crate::nle_export::audio_named(w).is_none() => w.strip_prefix("aac:")?,
        _ => return None,
    };
    let e = format!("aac rate \"{k}\": aac:KBPS takes a whole number of kb/s above 0; the words are {}", export_usage());
    args.pop();
    Some(e)
}

// ---- STRIALOUDNESSCANCEL (LEDGER SR903): Esc / a second `loudness` cancels the measure; its progress on the message line (named tail block) ----

impl Face {
    /// Esc's (and a second `loudness` word's) arm: ask the running measure to stop ([`crate::nle::LoudnessWatch`]'s flag,
    /// read by `RenderSink::stop` before the next step's decode); false when none runs. The line `loudness: cancelled`
    /// comes when the thread is joined ([`Face::loudness_tick`] / [`Face::wait_loudness`]).
    pub fn cancel_loudness(&mut self) -> bool {
        let Some(j) = self.loudness.as_ref() else { return false };
        j.watch.cancel.store(true, Relaxed);
        true
    }
    /// The running measure's count on the message line ([`loudness_progress_line`]); true (a repaint) only when the
    /// line moved since the last one put up.
    fn loudness_progress(&mut self) -> bool {
        let Some(j) = self.loudness.as_mut() else { return false };
        let line = loudness_progress_line(j.watch.done.load(Relaxed), j.total);
        if line == j.shown {
            return false;
        }
        j.shown = line.clone();
        self.last_error = Some(line);
        true
    }
}

/// STRIALOUDNESSCANCEL (SR903): `loudness: measuring <ticks done>/<total>` (done clamped to the total).
pub fn loudness_progress_line(done: u64, total: u64) -> String {
    format!("loudness: measuring {}/{total}", done.min(total))
}

// ---- STRIAOPACITYFACE (LEDGER SR898): the opacity step buttons and their message (named tail block) ----
// `opacity-` / `opacity+` (VERBS, the transport's two buttons) go through timeline_core's ONE verb dispatch
// (`EditorState::step_opacity`, one commit) and the message line then reads `opacity: <pct> %` (the primary selected clip's
// percent); a refusal reads `opacity: <EditError>`. The typed `opacity <pct>` keeps SR868's road (silent on success).

impl Face {
    /// STRIAOPACITYFACE: an opacity step word through the one dispatch, its percent on the message line; `None` for any
    /// other word.
    fn opacity_word(&mut self, v: &str) -> Option<bool> {
        if !matches!(v, "opacity-" | "opacity+") {
            return None;
        }
        self.last_error = Some(match self.state.verb(v).unwrap_or(Err(EditError::NoClip)) {
            Ok(()) => format!("opacity: {} %", self.state.selected().map_or(timeline_core::OPAQUE, |c| c.opacity)),
            Err(e) => format!("opacity: {e}"),
        });
        Some(true)
    }
}

// ---- STRIASNAPTOGGLE (LEDGER SR892): the drag snap default on / off (named tail block) ----
// SR836's snap (timeline_core view's `snap_targets` / `snap_to`) is switched per press by the `nosnap` bool of
// `Drag::begin_with` / `Drag::begin_transition_edge`; the face holds the default and passes `(Alt held) != snap_off`,
// so Alt inverts whichever default stands. The `snap` word is a view word: no commit, no undo step, not dirty; the
// message row names the new state.
impl Face {
    /// The `snap` word: flip the drag snap default and name it on the message row (`snap: on` / `snap: off`).
    fn snap_verb(&mut self, v: &str) -> Option<bool> {
        if v != "snap" {
            return None;
        }
        self.snap_off = !self.snap_off;
        self.note = Some(format!("snap: {}", if self.snap_off { "off" } else { "on" })); // STRIASNAPNEXT (SR990): a success line is a note, not an error
        Some(true)
    }
    /// A plain press snaps (the default; Alt at the press inverts it).
    pub fn snap_on(&self) -> bool {
        !self.snap_off
    }
}

// ---- STRIARECENTFILES (LEDGER SR884): the recent documents, Principia's stria.recent (named tail block) ----
// The list is a preference: its store is Principia's (one key, the SAVERECENT SR391 `aether.recent` shape), never a file
// of Stria's own. The face folds the row; the vessel reads and writes it through quartzite `chrome_rows::stored_str` /
// `store_str`. Opening one of them from the face is owed (NLE-RECENT-OPEN: the face's document is fixed at `Face::new`).

/// STRIARECENTFILES (SR884): Principia's namespace of the recent documents row.
pub const RECENT_NS: &str = "stria";
/// STRIARECENTFILES (SR884): Principia's key for the recent documents, newest first, one path per line.
pub const RECENT_KEY: &str = "recent";
/// STRIARECENTFILES (SR884): how many recent documents are kept.
pub const RECENT_MAX: usize = 5;

/// STRIARECENTFILES (SR884): the stored recent documents, newest first (blank lines dropped, at most [`RECENT_MAX`];
/// empty when unset).
pub fn recent_docs(stored: Option<&str>) -> Vec<String> {
    stored.map(|s| s.lines().filter(|l| !l.is_empty()).take(RECENT_MAX).map(str::to_string).collect()).unwrap_or_default()
}

/// STRIARECENTFILES (SR884): the row with `path` opened: `path` first, an equal older entry dropped, at most
/// [`RECENT_MAX`], one per line. `None` for a path holding a line break (the row is one path per line). Pure.
pub fn remember_doc(path: &std::path::Path, stored: Option<&str>) -> Option<String> {
    let p = path.to_string_lossy().into_owned();
    if p.is_empty() || p.contains('\n') || p.contains('\r') {
        return None;
    }
    let mut recent = vec![p.clone()];
    recent.extend(recent_docs(stored).into_iter().filter(|r| *r != p));
    recent.truncate(RECENT_MAX);
    Some(recent.join("\n"))
}

// ---- STRIAGRADEFACE (LEDGER SR899): the message row reads the clip's grade (named tail block) ----
/// STRIAGRADEFACE (SR899): what [`Face::reel_line`] appends for a clip graded `g` (SR869's grade, the `bright` / `contrast`
/// / `sat` words' three values in 1/1000): `"  bright <b> contrast <c> sat <s>"`; empty at identity (every line unchanged).
pub fn grade_note(g: timeline_core::Grade) -> String {
    if g == timeline_core::Grade::IDENTITY {
        return String::new();
    }
    format!("  bright {} contrast {} sat {}", g.brightness, g.contrast, g.saturation)
}

// ---- STRIANORMALIZE (LEDGER SR878): the normalize word over the clip's exact peak (named tail block) ----
// `normalize [<dBFS>]` (default -1 dBFS) sets the selected audio clip's gain so its peak lands on the target, never above
// it: the peak is EXACT (nle.rs `peak_of`, the render's own audio road over the clip's source range, beside SR827's
// `scan_peaks`; the u8 waveform table clamps at full scale, rounds to nearest and straddles the clip's edges), the gain
// absolute (the clip's current gain ignored; fades only lower the level) and set through SR838's ONE road
// (`EditorState::set_selected_gain`: `Timeline::set_gain`'s validate, one commit, one undo step; the same gain commits
// nothing). A host-face word: the editor holds no media, so the kernel Player never sees it (the gain it writes is the
// `* UNA GAIN` the kernel already reads). Every outcome is on the message line; a refusal leaves the timeline.

/// STRIANORMALIZE (SR878): the word's default target, centi-dB (-1 dBFS).
pub const NORMALIZE_TARGET_CDB: i32 = -100;

impl Face {
    /// The `normalize [<dBFS>]` word; `None` for any other word.
    fn normalize_verb(&mut self, v: &str) -> Option<bool> {
        let mut w = v.split_ascii_whitespace();
        w.next().filter(|w| *w == "normalize")?;
        let toks: Vec<&str> = w.collect(); let os = if toks.contains(&"tp") { crate::nle::TRUE_PEAK_OS } else { 1 }; let mut rest = toks.into_iter().filter(|t| *t != "tp"); let (arg, extra) = (rest.next(), rest.next()); // STRIANORMALIZEMORE (SR963): a `tp` token measures the true (oversampled) peak (file tail)
        self.last_error = Some(match self.normalize(arg, extra, os) {
            Ok(line) | Err(line) => format!("normalize: {line}"),
        });
        Some(true)
    }
    /// The rule behind [`Face::normalize_verb`]: `Ok` the success line, `Err` the refusal (the timeline unmoved).
    fn normalize(&mut self, arg: Option<&str>, extra: Option<&str>, os: u32) -> Result<String, String> {
        if let Some(x) = extra {
            return Err(format!("{x} is not a dBFS level"));
        }
        let target = match arg {
            None => NORMALIZE_TARGET_CDB,
            Some(a) => timeline_core::editor::cdb_of(a).ok_or_else(|| format!("{a} is not a dBFS level"))?,
        };
        if target > 0 {
            return Err("target above 0 dBFS".to_string());
        }
        if let Some(r) = self.normalize_peak_many(target, os) { return r; } let (track, _) = self.state.selection.ok_or_else(|| EditError::NoClip.to_string())?; // STRIANORMALIZELUFSMORE (SR9086): two or more selected audio clips, each clip's own peak (file tail)
        let c = self.state.selected().ok_or_else(|| EditError::NoClip.to_string())?;
        if self.state.timeline.tracks.get(track).map(|t| t.kind) != Some(Kind::Audio) {
            return Err("not an audio clip".to_string());
        }
        let tb = self.state.timeline.tb;
        let Some(peak) = self.normalize_peak(track, c, tb.to_ns(c.src_in)..tb.to_ns(c.src_out), target, os)? else { return Ok(NORMALIZE_SCANNING.to_string()) }; // STRIANORMALIZETHREE (SR9035): at the mix's rate, off the thread when served (file tail)
        self.normalize_finish(peak, target, os) // STRIANORMALIZETHREE (SR9035): the gain and its line moved to `normalize_finish` (file tail)
    }
}

/// STRIANORMALIZE (SR878): the gain (centi-dB) that brings a linear `peak` (full scale 1.0) to `target_cdb`, rounded DOWN
/// so the result is never above the target: `floor(target_cdb - 2000 log10 peak)`. `None` for a peak that is not a
/// positive finite number (a silent clip). The cast saturates; `validate` refuses an out-of-range gain by name.
pub fn normalize_gain_cdb(peak: f32, target_cdb: i32) -> Option<i32> {
    let p = peak as f64;
    if !(p.is_finite() && p > 0.0) {
        return None;
    }
    Some((target_cdb as f64 - 2000.0 * p.log10()).floor() as i32)
}

// ---- STRIAAUTOSAVE (LEDGER SR883): <doc>.autosave.edl beside the document; recover at open (named tail block) ----
// The autosave is `save`'s text (`doc_text`: `edl::write` titled by the document's stem, as `save` titles it) written to the
// sibling `<stem>.autosave.edl` through `<stem>.autosave.edl.tmp` + rename; the document itself is never written. It is
// written on `serve`'s clock (`autosave_tick`; the clock runs while `autosave_pending`) after AUTOSAVE_EDITS edits (the undo
// depth's distance from the last write) or AUTOSAVE_SECS of clock ticks at the timeline's frame rate, synchronously on the
// face's thread (a 60-clip EDL is ~6 KB: the cost `save` already pays inline). `save` and a return to the clean timeline
// remove it. At open (`with_doc`) an autosave newer than the document is OFFERED on the message row; only the word
// `recover` reads it, and applies it as ONE commit (`EditorState::restore`). Owed: NLE-HOST-AUTOSAVE-KERNEL (the kernel
// Player's face). STRIAAUTOSAVECAP (SR913) built NLE-HOST-AUTOSAVE-WITNESS (stria-view's `"autosaved"`), NLE-HOST-AUTOSAVE-CAP
// (the edits are `EditorState::commits`, monotonic past `UNDO_MAX`), NLE-HOST-AUTOSAVE-KEEP (a declined autosave is kept as `.autosave.1.edl`).

/// STRIAAUTOSAVE (SR883): the edits (`EditorState::commits` distance, SR913) after which a dirty face writes its autosave.
pub const AUTOSAVE_EDITS: usize = 8;
/// STRIAAUTOSAVE (SR883): the seconds of `serve` clock ticks (at the timeline's frame rate) after which it writes anyway.
pub const AUTOSAVE_SECS: u32 = 60;

/// STRIAAUTOSAVE (SR883): the autosave beside `doc` (`cut.edl` -> `cut.autosave.edl`).
pub fn autosave_path(doc: &std::path::Path) -> std::path::PathBuf {
    doc.with_extension("autosave.edl")
}

/// STRIAAUTOSAVE (SR883): the timeline the autosave file holds (`None`: no file this face wrote), the commit count (`EditorState::commits`, SR913) at the last
/// write (or open / save) and the clock ticks counted since the timeline last differed from it.
#[derive(Default)]
struct Autosave {
    written: Option<Timeline>,
    depth: usize,
    ticks: u32,
}

impl Face {
    /// STRIAAUTOSAVE (SR883): an autosave is owed (dirty and unwritten) or owed removal (clean with one written): `serve`
    /// runs its clock. Never without a document path.
    pub fn autosave_pending(&self) -> bool {
        if self.doc_path.is_none() {
            return false;
        }
        if self.dirty() { self.autosave.written.as_ref() != Some(&self.state.timeline) } else { self.autosave.written.is_some() }
    }
    /// STRIAAUTOSAVE (SR883): one clock tick of the autosave: a clean face removes the file it wrote; a dirty unwritten one
    /// counts the tick and writes once AUTOSAVE_EDITS edits or AUTOSAVE_SECS of ticks have passed. True only when a write
    /// failed (`autosave: <path>: <why>` on the message row: repaint) or the keep was named (SR933); each attempt restarts both counts.
    pub fn autosave_tick(&mut self) -> bool {
        let Some(doc) = self.doc_path.clone() else { return false };
        if !self.dirty() {
            if self.autosave.written.is_some() {
                self.drop_autosave();
            } else {
                self.autosave.depth = self.state.commits();
                self.autosave.ticks = 0;
            }
            return false;
        }
        if self.autosave.written.as_ref() == Some(&self.state.timeline) {
            return false;
        }
        self.autosave.ticks = self.autosave.ticks.saturating_add(1);
        let fps = self.state.timeline.fps().unwrap_or(25).max(1);
        let edits = self.state.commits().wrapping_sub(self.autosave.depth);
        if edits < AUTOSAVE_EDITS && self.autosave.ticks < AUTOSAVE_SECS.saturating_mul(fps) {
            return false;
        }
        self.autosave.ticks = 0;
        self.autosave.depth = self.state.commits();
        let quiet = self.last_error.is_none(); match self.write_autosave(&doc) { // STRIAAUTOSAVEKEEPREAD (SR933): a keep note (no error before) repaints
            Ok(()) => quiet && self.last_error.is_some(),
            Err(e) => {
                self.last_error = Some(format!("autosave: {e}"));
                true
            }
        }
    }
    /// STRIAAUTOSAVE (SR883): the timeline the autosave file holds, when this face wrote (or recovered) one.
    pub fn autosaved(&self) -> Option<&Timeline> {
        self.autosave.written.as_ref()
    }
    /// STRIAAUTOSAVE (SR883): `save`'s text for `doc` (the ONE writer, titled by the document's stem).
    fn doc_text(&self, doc: &std::path::Path) -> Result<String, String> {
        let title = doc.file_stem().and_then(|s| s.to_str()).unwrap_or("untitled");
        timeline_core::edl::write(&self.state.timeline, title).map_err(|e| format!("{}: {e}", doc.display()))
    }
    /// STRIAAUTOSAVE (SR883): write the autosave atomically (the `.tmp` sibling, then rename over the autosave).
    fn write_autosave(&mut self, doc: &std::path::Path) -> Result<(), String> {
        let path = autosave_path(doc); self.keep_foreign_autosave(doc)?;
        let text = self.doc_text(doc)?;
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        std::fs::write(&tmp, text.as_bytes()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{}: {e}", path.display()));
        }
        self.autosave.written = Some(self.state.timeline.clone());
        Ok(())
    }
    /// STRIAAUTOSAVE (SR883): remove the autosave (a saved document) and restart the counts from the current undo depth.
    fn drop_autosave(&mut self) {
        if let Some(doc) = self.doc_path.clone() {
            if self.keep_foreign_autosave(&doc).is_ok() { let _ = std::fs::remove_file(autosave_path(&doc)); }
        }
        self.autosave = Autosave { written: None, depth: self.state.commits(), ticks: 0 };
    }
    /// STRIAAUTOSAVE (SR883): at open, an autosave strictly newer than the document is offered on the message row. Nothing
    /// is read or applied here.
    fn offer_autosave(&mut self) {
        let Some(doc) = self.doc_path.clone() else { return };
        let path = autosave_path(&doc);
        let newer = autosave_newer(&doc, 0); // STRIAAUTOSAVEMORE (SR972): the one mtime rule (file tail)
        if newer {
            let name = |p: &std::path::Path| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
            self.last_error = Some(format!("autosave: {} is newer than {}; the word recover loads it", name(&path), name(&doc)));
        }
    }
    /// STRIAAUTOSAVE (SR883): the `recover` word; a refusal is `recover: <why>` on the message row.
    fn autosave_verb(&mut self, v: &str) -> Option<bool> {
        if recover_generation(v).is_none() { // STRIAAUTOSAVEKEEPREAD (SR933): `recover` or `recover <n>` (file tail)
            return None;
        }
        if let Err(e) = self.recover_from(recover_generation(v).unwrap_or(0)) {
            self.last_error = Some(format!("recover: {e}"));
        }
        Some(true)
    }
    /// STRIAAUTOSAVE (SR883): read the autosave and apply it as ONE commit; the face is then dirty against the document and
    /// the file stays (it holds the timeline) until `save`.
    pub fn recover(&mut self) -> Result<(), String> { self.recover_from(0) } pub fn recover_from(&mut self, generation: usize) -> Result<(), String> { // STRIAAUTOSAVEKEEPREAD (SR933): 1 reads the kept file (file tail)
        let Some(doc) = self.doc_path.clone() else { return Err("no document path".into()) };
        let path = autosave_generation_path(&doc, generation)?;
        if !path.is_file() {
            return Err(format!("no autosave ({})", path.display()));
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let tl = timeline_core::edl::parse(&text, self.state.timeline.fps().unwrap_or(25)).map_err(|e| format!("{}: {e}", path.display()))?;
        if !self.state.restore(tl) {
            return Err("the autosave matches the timeline".into());
        }
        self.autosave = Autosave { written: (generation == 0).then(|| self.state.timeline.clone()), depth: self.state.commits(), ticks: 0 };
        Ok(())
    }
}

// ---- STRIAUNDOHISTORY (LEDGER SR876): the face's `history` word and its verbs named on the undo steps (named tail block) ----
// The public [`Face::verb`] arms its word on the editor state (`EditorState::arm_word`) around the face's dispatch
// (`verb_unnamed`), so every undo step a face word commits carries that word, the face's own roads too (a selected
// transition's `delete` / `[` / `]`, the `in` / `out` marks); the outer word wins over the editor word it calls (`slip+`, not
// `slip 1`). `history` is view-only: the last `HISTORY_N` undo entries, newest first, as ONE line on the message row
// (`last_error`, THE message: `serve` logs it, stria-view lists it under `errors`); it commits nothing and dirties nothing.
impl Face {
    /// A transport verb ([`VERBS`]); false for any other word.
    pub fn verb(&mut self, v: &str) -> bool {
        if let Some(r) = self.undo_jump_verb(v) { return r; } if let Some(r) = self.history_rows_verb(v) { return r; } if v == "history" {
            self.last_error = Some(self.state.history_line(timeline_core::editor::HISTORY_N)); self.note = self.last_error.clone().map(|l| self.with_redo(l, timeline_core::editor::HISTORY_N)); // STRIAUNDOTHREE (SR9027): the redo entries ride the note (file tail)
            return true;
        }
        let armed = self.state.arm_word(v);
        let r = self.verb_unnamed(v);
        self.state.disarm_word(armed);
        r
    }
}

// ---- STRIAKEYBOARDMAP (LEDGER SR886): the key overlay — parse, refusals, lookup (named tail block) ----
// A person's key map is TEXT handed to the face by its caller (`Face::with_keys`; stria-view `--keys MAP`): the face reads
// no file and owns no settings path (a preference is Principia's: NLE5D-KEYS-CONFIG-STORE owed). Entries split on a newline
// or `;`; a blank entry or one starting `#` is skipped; an entry is `<key> <action>`: the first whitespace token is the key
// (`Space` names " "; any other token verbatim, so `Face::key`'s one-char-from-text / name-from-BrowserKey rule decides how
// it arrives), the rest is the action: a VERBS word or a KEYS action (the words `Face::key` dispatches), or `none` (the key
// is unbound). The overlay is consulted BEFORE KEYS and never mutates it; the help rows show the effective bindings. A map
// is taken whole or not at all: an unknown action, a key named twice, a key with no action, or a map under which no key
// shows the help is refused by name and the static table stays.

/// STRIAKEYBOARDMAP (SR886): the map's word that unbinds a key.
pub const KEYMAP_UNBIND: &str = "none";

/// STRIAKEYBOARDMAP (SR886): a parsed key overlay: `(key, Some(action) | None = unbound)` in text order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keymap(Vec<(String, Option<&'static str>)>);

/// STRIAKEYBOARDMAP (SR886): why a key map was refused (`line` counts entries from 1, blank and `#` entries included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapError {
    UnknownAction { line: usize, word: String },
    DuplicateKey { line: usize, key: String },
    NoAction { line: usize, key: String },
    HelpUnbound,
}

impl std::fmt::Display for KeymapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeymapError::UnknownAction { line, word } => write!(f, "keys: line {line}: unknown action \"{word}\""),
            KeymapError::DuplicateKey { line, key } => write!(f, "keys: line {line}: \"{key}\" is bound twice"),
            KeymapError::NoAction { line, key } => write!(f, "keys: line {line}: \"{key}\" names no action"),
            KeymapError::HelpUnbound => write!(f, "keys: no key shows the help"),
        }
    }
}

/// STRIAKEYBOARDMAP (SR886): a word `Face::key` can dispatch, as its table's `&'static str` (VERBS first, then KEYS).
fn keymap_vocab(w: &str) -> Option<&'static str> {
    VERBS.iter().copied().find(|v| *v == w).or_else(|| KEYS.iter().map(|&(_, a)| a).find(|a| *a == w))
}

/// STRIAKEYBOARDMAP (SR886): a map's text as a [`Keymap`], or the first refusal.
pub fn parse_keymap(text: &str) -> Result<Keymap, KeymapError> {
    let mut map: Vec<(String, Option<&'static str>)> = Vec::new();
    for (i, entry) in text.split(['\n', ';']).enumerate() {
        let line = i + 1;
        let entry = entry.trim();
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        let (name, word) = match entry.split_once(char::is_whitespace) {
            Some((n, w)) => (n, w.trim()),
            None => (entry, ""),
        };
        let key = if name == "Space" { " ".to_string() } else { name.to_string() };
        if word.is_empty() {
            return Err(KeymapError::NoAction { line, key: name.to_string() });
        }
        let act = if word == KEYMAP_UNBIND {
            None
        } else {
            Some(keymap_vocab(word).ok_or_else(|| KeymapError::UnknownAction { line, word: word.to_string() })?)
        };
        if map.iter().any(|(k, _)| *k == key) {
            return Err(KeymapError::DuplicateKey { line, key: name.to_string() });
        }
        map.push((key, act));
    }
    let map = Keymap(map);
    if !map.bindings().iter().any(|&(_, a)| a == "help") {
        return Err(KeymapError::HelpUnbound);
    }
    Ok(map)
}

impl Keymap {
    /// The effective bindings: KEYS rows whose key the map does not name (KEYS order), then the map's bound rows (text order).
    pub fn bindings(&self) -> Vec<(&str, &'static str)> {
        let mut out: Vec<(&str, &'static str)> = KEYS.iter().filter(|(k, _)| !self.0.iter().any(|(m, _)| m == k)).map(|&(k, a)| (k, a)).collect();
        out.extend(self.0.iter().filter_map(|(k, a)| a.map(|a| (k.as_str(), a))));
        out
    }
    /// The action a key names: the map's entry when it names the key (`None` when unbound there), else KEYS's.
    pub fn action(&self, k: &str) -> Option<&'static str> {
        match self.0.iter().find(|(m, _)| m == k) {
            Some(&(_, a)) => a,
            None => KEYS.iter().find(|(name, _)| *name == k).map(|&(_, a)| a),
        }
    }
    /// The map has no entries (the static table alone).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Face {
    /// STRIAKEYBOARDMAP (SR886): the face with a person's key map ([`parse_keymap`]); a refused map leaves the static table
    /// and its reason on the message row (`last_error`). Empty text is the empty map. View-only: no undo step, no dirty.
    pub fn with_keys(mut self, text: &str) -> Face {
        match parse_keymap(text) {
            Ok(m) => self.keymap = m,
            Err(e) => self.last_error = Some(e.to_string()),
        }
        self
    }
    /// STRIAKEYBOARDMAP (SR886): the face's key overlay.
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }
    /// STRIAKEYBOARDMAP (SR886): the action a key names on this face (the overlay first, then KEYS).
    fn bound(&self, k: &str) -> Option<&'static str> {
        self.keymap.action(k)
    }
}

// ---- STRIAPANFACE (LEDGER SR897): the pan words on the face and their message-row readout (named tail block) ----
// The three face words `pan-` / `pan` / `pan+` (VERBS, with their stria-view buttons) go through timeline_core's ONE verb
// dispatch (SR861's `pan_verb` -> `set_selected_pan` -> one `commit`); after it the message row reads the selected clip's
// pan ([`pan_line`]), or `pan: <why>` on a refusal. `pan <x>` (SR861) stays silent: it is not one of these words.
impl Face {
    /// The `pan-` / `pan` / `pan+` words; `None` for any other word.
    fn pan_face_verb(&mut self, v: &str) -> Option<bool> {
        if !matches!(v, "pan-" | "pan" | "pan+") {
            return None;
        }
        let Some(r) = self.state.verb(v) else { return Some(false) };
        self.last_error = Some(match r {
            Ok(()) => pan_line(self.state.selected().map_or(0, |c| c.pan_cq8)),
            Err(e) => format!("pan: {e}"),
        });
        Some(true)
    }
}

/// The message row's pan reading: `pan: centre`, `pan: L <p>` or `pan: R <p>`, `p` the percent of the swing
/// (`|pan_cq8| * 100 / 127` rounded, at most 100): -64 reads `pan: L 50`, 32 `pan: R 25`, 127 `pan: R 100`.
pub fn pan_line(pan_cq8: i8) -> String {
    let p = ((pan_cq8 as i32).abs() * 100 + 63) / 127;
    match pan_cq8 {
        0 => "pan: centre".to_string(),
        n if n < 0 => format!("pan: L {}", p.min(100)),
        _ => format!("pan: R {}", p.min(100)),
    }
}

// ---- STRIAPROXYMEDIA (LEDGER SR882): the `proxy` verb — a half-size H.264 of a source, previewed from (named tail block) ----
// `proxy` (VERBS, the Proxy button) takes the SELECTED clip's source: its file (the reel's path, [`Face::with_reel_paths`]:
// stria-view's `--src`) renders ONCE through the one export door, `nle_export::export_as_with`, over the h264 table encoder
// wrapped in `nle_export::HalfSize` (half each side through yuv_core's nearest_rows), off the face's thread on the export
// door's shape (a job, two AtomicU64 readings, a join at the face's clock), into `<name>.proxy.mp4` beside the source, with a
// `<name>.proxy.stamp` of the source's `<len> <mtime ns>` taken at the job's start. The finished proxy is attached to the
// shared `Sources` (`Sources::set_proxy`); the preview's sinks then decode it, an export never does. A proxy whose stamp no
// longer matches its source is STALE and rebuilt; one that matches is reused. View-only: no undo step, the document clean.
// Owed: NLE-PROXY-OFF, NLE-PROXY-ALL, NLE-PROXY-CANCEL, NLE-PROXY-AUTO, NLE-PROXY-WATCH, NLE-PROXY-ADOPT, NLE-PROXY-AUDIO,
// NLE-PROXY-SAVEFRAME (a saved frame of a proxied reel, through `preview`, is the proxy's).

/// STRIAPROXYMEDIA (LEDGER SR882): one proxy render running off the face's thread; the thread returns the frames encoded.
pub struct ProxyJob {
    src: timeline_core::SourceId,
    reel: String,
    path: std::path::PathBuf,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    handle: Option<std::thread::JoinHandle<Result<u64, String>>>,
    cancel: Arc<AtomicBool>, // STRIAPROXYOFF (SR915): Esc's ask to stop (`Face::cancel_proxy`)
    sgen: u64, // STRIAPROXYWATCH (SR9079): the sources generation the job cloned (file tail)
}

/// STRIAPROXYMEDIA (LEDGER SR882): the proxy, its stamp and its transient export directory beside source file `src`
/// (`clip.mp4` -> `clip.proxy.mp4`, `clip.proxy.stamp`, `clip.proxy.d`).
pub fn proxy_paths(src: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    (src.with_extension("proxy.mp4"), src.with_extension("proxy.stamp"), src.with_extension("proxy.d"))
}

/// STRIAPROXYMEDIA (LEDGER SR882): source file `src`'s stamp now: `<len> <mtime ns>\n`; `None` when it cannot be read.
pub fn proxy_stamp(src: &std::path::Path) -> Option<String> {
    let m = std::fs::metadata(src).ok()?;
    let ns = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{} {ns}\n", m.len()))
}

/// STRIAPROXYMEDIA (LEDGER SR882): the proxy beside `src` exists and its stamp equals the source's now (size and mtime);
/// false = absent or STALE.
pub fn proxy_fresh(src: &std::path::Path) -> bool {
    let (proxy, stamp, _) = proxy_paths(src);
    proxy.is_file() && proxy_stamp(src).is_some_and(|now| std::fs::read_to_string(stamp).is_ok_and(|s| s == now))
}

impl Face {
    /// Each reel's source file (reel name, path), the files `proxy` renders from and writes beside.
    pub fn with_reel_paths(mut self, paths: Vec<(String, std::path::PathBuf)>) -> Face {
        self.reel_paths = paths; self.proxy_auto(); self.watch_arm(); // STRIAPROXYOFF (SR915): a fresh proxy beside a reel attaches at open (file tail)
        self
    }
    /// True while a proxy job's result has not been reported.
    pub fn proxying(&self) -> bool {
        self.proxy.is_some()
    }
    /// The running proxy job's reading `(frames done, frames total)`.
    pub fn proxy_progress(&self) -> Option<(u64, u64)> {
        self.proxy.as_ref().map(|j| (j.done.load(Relaxed), j.total.load(Relaxed)))
    }
    /// The `proxy` verb: every outcome is the message line `proxy: ..` (`proxy: no clip selected`, `proxy: busy (d/t)`, or
    /// `proxy: <reel>: <outcome>`).
    fn proxy_verb(&mut self, v: &str) -> Option<bool> {
        if v.trim() != "proxy" {
            return None;
        }
        let Some(c) = self.state.selected() else {
            self.last_error = Some("proxy: no clip selected".into());
            return Some(true);
        };
        if let Some((d, t)) = self.proxy_progress() {
            self.last_error = Some(format!("proxy: busy ({d}/{t})"));
            return Some(true);
        }
        let reel = self.state.timeline.source(c.src).map_or_else(|| c.src.0.to_string(), |s| s.reel.clone());
        let line = self.proxy_start(c.src, &reel).unwrap_or_else(|e| e);
        self.last_error = Some(format!("proxy: {reel}: {line}"));
        Some(true)
    }
    /// Reuse a fresh proxy (`reused <path>`) or start the job (`building (new|stale) -> <path>`); refused by name: `no source
    /// path`, `<file>: unreadable`, `stale (the file changed since it was opened)`, `no video stream`, a `set_proxy` refusal.
    fn proxy_start(&mut self, src: timeline_core::SourceId, reel: &str) -> Result<String, String> {
        let file = self.reel_paths.iter().find(|(r, _)| r == reel).map(|(_, p)| p.clone()).ok_or("no source path")?;
        let now = proxy_stamp(&file).ok_or_else(|| format!("{}: unreadable", file.display()))?;
        if std::fs::metadata(&file).map(|m| m.len()).ok() != self.sources.byte_len(src) {
            return Err("stale (the file changed since it was opened)".into());
        }
        let (proxy, stamp, work) = proxy_paths(&file);
        if proxy_fresh(&file) {
            let bytes = std::fs::read(&proxy).map_err(|e| format!("{}: {e}", proxy.display()))?;
            self.sources.set_proxy(src, bytes).map_err(|e| e.to_string())?;
            return Ok(format!("reused {}", proxy.display()));
        }
        let why = if proxy.exists() { "stale" } else { "new" };
        let n = self.sources.video_ticks(src).ok_or("no video stream")?;
        let mut tl = self.state.timeline.clone();
        tl.tracks.clear();
        let t = tl.add_track(Kind::Video);
        tl.tracks[t].clips = vec![Clip::new(src, 0, n, 0)];
        std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
        let (done, total) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(n.max(0) as u64))); let cancel = Arc::new(AtomicBool::new(false)); // STRIAPROXYOFF (SR915): the job's cancel flag
        let (d, tt, sources, out, dir) = (Arc::clone(&done), Arc::clone(&total), Arc::clone(&self.sources), proxy.clone(), work.clone()); let (c, hold) = (Arc::clone(&cancel), self.proxy_hold); // STRIAPROXYOFF (SR915): the flag and the hold the closure reads
        let handle = std::thread::Builder::new()
            .name("nle-proxy".into())
            .spawn(move || -> Result<u64, String> {
                let video: Box<dyn crate::nle_export::VideoEncoder> = Box::new(crate::nle_export::HalfSize(crate::nle_export::video_named("h264", tl.tb).expect("h264 is a VIDEO_NAMES word")));
                let mut progress = |k: u64, of: u64| -> Result<(), ExportError> {
                    d.store(k, Relaxed);
                    tt.store(of, Relaxed);
                    proxy_gate(&c, hold, k) // STRIAPROXYOFF (SR915): the hold, then Esc's refusal (file tail)
                };
                let r = export_as_with(&tl, 0..n, &sources, &dir, video, AudioOut::None, Container::Mp4, &mut progress).map_err(|e| { let _ = std::fs::remove_dir_all(&dir); e.to_string() })?; // STRIAPROXYOFF (SR915): a failed or cancelled job leaves no work dir
                std::fs::rename(dir.join("video.mp4"), &out).map_err(|e| format!("{}: {e}", out.display()))?;
                std::fs::write(&stamp, now).map_err(|e| format!("{}: {e}", stamp.display()))?;
                let _ = std::fs::remove_dir_all(&dir);
                Ok(r.frames)
            })
            .map_err(|e| format!("{}: {e}", work.display()))?;
        self.proxy = Some(ProxyJob { src, reel: reel.to_string(), path: proxy.clone(), done, total, handle: Some(handle), cancel, sgen: self.sources_gen });
        Ok(format!("building ({why}) -> {}", proxy.display()))
    }
    /// One clock tick for the proxy job: a finished job attaches its proxy and reports; true then (the preview repaints).
    pub fn proxy_tick(&mut self) -> bool {
        if !self.proxy.as_ref().is_some_and(|j| j.handle.as_ref().is_some_and(|h| h.is_finished())) {
            return false;
        }
        self.finish_proxy();
        true
    }
    /// Block until the running proxy job ends and report it as [`Face::proxy_tick`] would; false when none runs.
    pub fn wait_proxy(&mut self) -> bool {
        if self.proxy.is_none() {
            return false;
        }
        self.finish_proxy();
        true
    }
    /// Join the job, attach the proxy (`Sources::set_proxy`) and put `proxy: <reel>: <n> frames -> <path>` (or the refusal)
    /// on the message line.
    fn finish_proxy(&mut self) {
        let Some(mut j) = self.proxy.take() else { return }; if self.proxy_stale(&mut j) { return; } // STRIAPROXYWATCH (SR9079): a job cloned before its reel reloaded attaches nothing and rebuilds (file tail)
        let r = match j.handle.take().map(|h| h.join()) {
            Some(Ok(Ok(frames))) => std::fs::read(&j.path)
                .map_err(|e| format!("{}: {e}", j.path.display()))
                .and_then(|b| self.sources.set_proxy(j.src, b).map_err(|e| e.to_string()))
                .map(|()| format!("{frames} frames -> {}", j.path.display())),
            Some(Ok(Err(e))) => Err(if j.cancel.load(Relaxed) { format!("cancelled at {}/{} frames", j.done.load(Relaxed), j.total.load(Relaxed)) } else { e }), // STRIAPROXYOFF (SR915): Esc's stop by name
            _ => Err("the proxy thread panicked".to_string()),
        };
        self.last_error = Some(format!("proxy: {}: {}", j.reel, r.unwrap_or_else(|e| e))); self.proxy_next(); // STRIAPROXYOFF (SR915): `proxy all` chains the next reel (file tail)
    }
}

// ---- STRIAPROJECTBIN (LEDGER SR885): the source bin Surface (named tail block) ----
// The split editor's third Surface, [`BIN_SURFACE`]: timeline_core view's `paint_bin` (one row per source: the reel, its
// duration as timecode, the kind read from the tracks its clips sit on) and `bin_hit`. A click on a row is SR852's word
// `insert <reel> 0 <duration>` (`view::bin_word`) through [`Face::verb`]: the reel's whole range at the playhead on the first
// track holding it, ONE commit and one undo step, a refusal on the message line by name. Drag-to-timeline is owed
// (NLE-BIN-DRAG): a `Drag` is one Surface's frozen layout and cannot cross onto the tracks Surface.

/// The split editor's bin Surface (the sources list).
pub const BIN_SURFACE: &str = "stria.nle.bin";

/// The bin's Surface: its number, its canvas and whether it sends `BrowserButton` (its release then stands for the click).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinSurface {
    pub n: u32,
    pub canvas: (u32, u32),
    buttons: bool,
    pub top: usize, wheel: i64, grab: Option<BinGrab>, // STRIABINSCROLL (SR939): the first shown row and the wheel's notch remainder in thousandths of dy (file tail); STRIABINTHREE (SR9025): the press's thumb or row drag (file tail)
}

impl Face {
    /// Put the source bin on Surface `n` of a split face ([`Face::on_surfaces`] first; an unsplit face is returned
    /// unchanged). Its canvas starts at the tracks' width, one [`view::BIN_ROW_H`] row per source.
    pub fn with_bin(mut self, n: u32) -> Face {
        let Some(s) = self.split else { return self };
        let rows = self.state.timeline.sources.len().max(1) as u32;
        self.split = Some(Split { bin: Some(BinSurface { n, canvas: (self.canvas.0, view::BIN_ROW_H * rows), buttons: false, top: 0, wheel: 0, grab: None }), ..s }); // STRIABINSCROLL (SR939): the first shown row and the wheel's remainder; STRIABINTHREE (SR9025): no drag
        self
    }
    /// The bin's Surface, when the face has one.
    pub fn bin(&self) -> Option<BinSurface> {
        self.split.and_then(|s| s.bin)
    }
    /// A pointer message from the bin Surface (`Some(repaint)`); `None` for every other message. A resize sizes the bin's
    /// canvas only; the release (or the click, on a surface that sends no buttons) is [`Face::bin_click`]; a press, move or
    /// wheel is taken and changes nothing.
    fn bin_input(&mut self, msg: &SMessage) -> Option<bool> {
        let s = self.split?;
        let b = s.bin?;
        if msg.surface() != Some(b.n) {
            return None;
        }
        let set = |f: &mut Face, b: BinSurface| f.split = Some(Split { bin: Some(b), ..s });
        Some(match *msg {
            SMessage::BrowserResize(w, h, _) => {
                let changed = w > 0 && h > 0 && (w, h) != b.canvas;
                if changed {
                    set(self, BinSurface { canvas: (w, h), ..b });
                }
                changed
            }
            SMessage::BrowserButton { down: true, x, y, .. } => { // STRIABINTHREE (SR9025): the press's point
                set(self, BinSurface { buttons: true, ..b });
                self.bin_press(x, y) // STRIABINTHREE (SR9025): a press grabs the thumb or arms a row's range (file tail)
            }
            SMessage::BrowserButton { down: false, x, y, .. } => self.bin_release(x, y), // STRIABINTHREE (SR9025): a drag's end, else the click (file tail)
            SMessage::BrowserClick(x, y, _) if !b.buttons => self.bin_click(x, y),
            SMessage::BrowserScroll(_, dy, _) => self.bin_scroll(dy), // STRIABINSCROLL (SR939): the wheel scrolls the rows (file tail)
            SMessage::BrowserMouseMove(x, y, _) => self.bin_move(x, y), _ => false, // STRIABINTHREE (SR9025): a move drives the press's drag (file tail)
        })
    }
    /// A click on the bin at `(x, y)`: the row's word (`view::bin_hit` -> `view::bin_word`) through [`Face::verb`];
    /// false under the last row.
    fn bin_click(&mut self, x: f64, y: f64) -> bool {
        let Some(b) = self.bin() else { return false };
        let tl = &self.state.timeline;
        let Some(i) = view::bin_hit_from(tl, b.canvas.0, b.canvas.1, b.top, x, y) else { return false }; self.bin_lay(i, None) // STRIABINSCROLL (SR939): the hit shifted by the bin's first shown row; a reel on no track laid by its media's kind (file tail); STRIABINTHREE (SR9025): the whole reel through the one lay fn (file tail)
    }
    /// The bin's pixels (`Role::Bin`): timeline_core view's `paint_bin`, the labels through [`Labels`].
    fn paint_bin(&self) -> Vec<u8> {
        let (w, h) = self.bin().map(|b| b.canvas).unwrap_or(self.canvas);
        let mut lab = self.labels.as_ref().map(|s| Labels(s, Some(&self.lcache)));
        self.bin_range_paint(view::paint_bin_thumb(&self.state.timeline, w, h, self.bin().map_or(0, |b| b.top), vo(self.order), lab.as_mut().map(|t| t as &mut dyn view::Text))) // STRIABINTHREE (SR9025): a row drag's range bar (file tail); STRIABINSCROLL (SR939): the rows from the first shown one (timeline_core view.rs tail); STRIABINMORE (SR976): the scroll thumb on overflow (view.rs tail)
    }
    /// [`Face::frames`]' blits, with the bin's appended when the face has one.
    fn bin_blit(&mut self, mut v: Vec<SMessage>) -> Vec<SMessage> {
        if let Some(b) = self.bin() {
            let pixels = self.paint_role(Role::Bin);
            v.push(SMessage::SurfaceBlit { url: BIN_SURFACE.into(), width: b.canvas.0, height: b.canvas.1, pixels, damage: None, scroll: None });
        }
        v
    }
}

#[cfg(test)]
mod striaprojectbin_tests {
    use super::*;
    use timeline_core::{Mode, SourceId, edl};

    fn plain() -> Face {
        let (tl, src) = synth_fixture();
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
    }

    fn face() -> Face {
        plain().on_surfaces(0, 1).with_bin(2)
    }

    fn click(n: u32, (x, y): (f64, f64)) -> SMessage {
        SMessage::BrowserClick(x, y, n)
    }

    fn blit(m: &SMessage) -> (&str, u32, u32, &[u8]) {
        let SMessage::SurfaceBlit { url, width, height, pixels, .. } = m else { panic!("not a blit: {m:?}") };
        (url.as_str(), *width, *height, pixels.as_slice())
    }

    /// The bin is the third blit, painted by timeline_core view's `paint_bin`; a split face without a bin and an unsplit
    /// face keep their blits.
    #[test]
    fn striaprojectbin_frames_carry_the_bin_last() {
        let mut f = face();
        assert_eq!(f.bin().map(|b| (b.n, b.canvas)), Some((2, (320, 2 * view::BIN_ROW_H))));
        let frames = f.frames();
        assert_eq!(frames.len(), 3);
        assert_eq!(blit(&frames[0]).0, PREVIEW_SURFACE);
        assert_eq!(blit(&frames[1]).0, TRACKS_SURFACE);
        let want = view::paint_bin(&f.state.timeline, 320, 36, Order::Rgba, None);
        assert_eq!(blit(&frames[2]), (BIN_SURFACE, 320, 36, want.as_slice()));
        assert_eq!(f.paint_role(Role::Bin), want);
        assert_eq!(plain().on_surfaces(0, 1).frames().len(), 2, "no bin: the two split blits");
        let mut unsplit = plain().with_bin(2);
        assert!(unsplit.bin().is_none());
        assert_eq!(unsplit.frames().len(), 1);
    }

    /// A click on bin row B is SR852's `insert B 0 50`: B's whole range laid at the playhead on V1, one undo step, the EDL
    /// round trip; the same pixels on the tracks Surface are the clip face's click, not an insert.
    #[test]
    fn striaprojectbin_click_inserts_the_reel_at_the_playhead() {
        let mut f = face();
        let tl0 = f.state.timeline.clone();
        f.state.seek(15);
        assert!(f.on(&click(1, view::bin_point(1))), "the tracks Surface takes its own click");
        assert_eq!((&f.state.timeline, f.state.undo_depth()), (&tl0, 0), "no insert from the tracks Surface");
        f.state.deselect();
        f.state.seek(15);
        assert!(f.on(&click(2, view::bin_point(1))));
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        assert_eq!(f.state.timeline, tl0.place(0, Clip::new(SourceId(1), 0, 50, 15), Mode::Insert).unwrap());
        assert_eq!(f.state.timeline.tracks[1], tl0.tracks[1], "only the reel's first track moves");
        assert_eq!((f.state.selected(), f.state.playhead, f.state.undo_depth()), (Some(Clip::new(SourceId(1), 0, 50, 15)), 65, 1));
        let text = edl::write(&f.state.timeline, "BIN").unwrap();
        assert_eq!(edl::parse(&text, 25).unwrap(), f.state.timeline);
        assert!(f.verb("undo"));
        assert_eq!(f.state.timeline, tl0, "one undo restores");
        assert!(!f.on(&click(2, (5.0, 40.0))), "under the last row (the canvas is 36 px): nothing");
        assert_eq!(f.state.timeline, tl0);
    }

    /// A press / release pair is ONE insert (the trailing click is not a second); a reel on no track is refused by name.
    #[test]
    fn striaprojectbin_release_is_the_click_and_refusal_by_name() {
        let mut f = face();
        let (x, y) = view::bin_point(0);
        f.on(&SMessage::BrowserButton { down: true, x, y, mods: 0, surface: 2 });
        assert_eq!(f.state.undo_depth(), 0, "a press changes nothing");
        f.on(&SMessage::BrowserButton { down: false, x, y, mods: 0, surface: 2 });
        f.on(&click(2, (x, y)));
        assert_eq!(f.state.undo_depth(), 1, "one insert per press / release");
        let (mut tl, src) = synth_fixture();
        tl.add_source("C", 25);
        let mut g = Face::new(EditorState::new(tl.clone()), src, (320, 160), PixelOrder::Rgba, false).on_surfaces(0, 1).with_bin(2);
        assert_eq!(g.bin().unwrap().canvas, (320, 3 * view::BIN_ROW_H));
        assert!(g.on(&click(2, view::bin_point(2))));
        assert_eq!(g.last_error.take(), Some(format!("insert C 0 25: {}", EditError::NoTrack)));
        assert_eq!((&g.state.timeline, g.state.undo_depth()), (&tl, 0));
    }

    /// The bin Surface's resize sizes its canvas only; a move on it is taken and changes nothing.
    #[test]
    fn striaprojectbin_resize_and_move_stay_on_the_bin() {
        let mut f = face();
        let (canvas, preview) = (f.canvas, f.split().unwrap().preview_canvas);
        assert!(f.on(&SMessage::BrowserResize(200, 54, 2)));
        assert!(!f.on(&SMessage::BrowserResize(200, 54, 2)), "the same size: no repaint");
        assert_eq!(f.bin().unwrap().canvas, (200, 54));
        assert_eq!((f.canvas, f.split().unwrap().preview_canvas), (canvas, preview));
        assert!(!f.on(&SMessage::BrowserMouseMove(5.0, 5.0, 2)));
        assert!(f.pointer().is_none(), "the bin's pointer is not the tracks'");
        assert_eq!(blit(&f.frames()[2]).1, 200);
    }
}

// ---- STRIATCSET (LEDGER SR908): `tc <unit>` sets the readout directly; `t` keys `tc` (named tail block) ----
// `tc timecode|frames|seconds|off` sets SR872's readout unit (`off`: none) through `Face::tc_verb`'s first line; bare `tc`
// still cycles. A view word: no undo step, no dirty. An unknown or extra argument keeps the unit and names itself in
// `last_error`. The key `t` (a KEYS row, its HELP phrase) fires bare `tc` through `Face::verb`. Stria-view: `tc:<unit>`.
// Owed: NLE-TC-KERNEL, NLE-TC-DROPFRAME (SR872's), NLE-TC-SET-BUTTON (the one TC button cycles).

/// STRIATCSET (SR908): a `tc` argument as a unit: `off` -> `Some(None)`, a [`TcUnit::word`] -> `Some(Some(unit))`, else `None`.
pub fn tc_unit_of(w: &str) -> Option<Option<TcUnit>> {
    if w == "off" {
        return Some(None);
    }
    [TcUnit::Timecode, TcUnit::Frames, TcUnit::Seconds].into_iter().find(|u| u.word() == w).map(Some)
}

impl Face {
    /// `tc <unit>`: sets the readout's unit (`None` unless the first word is `tc` and an argument follows).
    fn tc_set(&mut self, v: &str) -> Option<bool> {
        let mut words = v.split_whitespace();
        if words.next() != Some("tc") {
            return None;
        }
        let args: Vec<&str> = words.collect();
        if args.is_empty() {
            return None;
        }
        match (args.len(), tc_unit_of(args[0])) {
            (1, Some(u)) => self.tc = u,
            _ => self.last_error = Some(format!("tc: unknown unit {} (timecode / frames / seconds / off)", args.join(" "))),
        }
        Some(true)
    }
}

#[cfg(test)]
mod striatcset_tests {
    use super::*;

    /// Every unit's word parses back to it; `off` is none; anything else is not a unit.
    #[test]
    fn striatcset_unit_words_round_trip() {
        for u in [TcUnit::Timecode, TcUnit::Frames, TcUnit::Seconds] {
            assert_eq!(tc_unit_of(u.word()), Some(Some(u)));
        }
        assert_eq!(tc_unit_of("off"), Some(None));
        for w in ["Frames", "", "fps", "tc"] {
            assert_eq!(tc_unit_of(w), None, "{w:?}");
        }
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
        self.zoom_fit = Some((v == "zoom-sel", self.state.scale(), self.view)); // STRIAZOOMFITTHREE (SR9040): the scale
    }
    /// After a tracks resize: fit again at the new width when the last view word was a fit (its zoom and view still hold).
    fn refit_on_resize(&mut self) {
        let Some((sel, zoom, view)) = self.zoom_fit.take() else { return };
        if zoom != self.state.scale() || view != self.view { // STRIAZOOMFITTHREE (SR9040): the scale
            return;
        }
        self.fit_verb(if sel { "zoom-sel" } else { "zoom-fit" });
    }
}

// ---- STRIATRACKGAINSTEP (LEDGER SR901): the track gain words' message line (named tail block) ----
// `track-gain [<dB>]` / `track-gain-` / `track-gain+` ride timeline_core's ONE verb dispatch (SR864's
// `set_selected_track_gain`); the face puts the outcome on its message line like SR838's gain words: `track-gain: A2
// -6.0 dB` (the selected clip's track, [`db_words`]) after an edit, `<the word as typed>: <EditError>` for a refusal.
impl Face {
    /// The track gain words; `None` for any other word.
    fn track_gain_face_verb(&mut self, v: &str) -> Option<bool> {
        v.split_ascii_whitespace().next().filter(|w| matches!(*w, "track-gain" | "track-gain-" | "track-gain+"))?;
        match self.state.verb(v) {
            Some(Ok(())) => {
                let tl = &self.state.timeline;
                self.last_error = self.gain_track(v).and_then(|t| tl.tracks.get(t).map(|tr| format!("track-gain: {} {}", track_word(tl, t), db_words(tr.gain_cdb)))); // STRIATRACKGAINTHREE (SR9024): the named track, else the selection's (file tail)
            }
            Some(Err(e)) => self.last_error = Some(format!("{v}: {e}")),
            None => return None,
        }
        Some(true)
    }
}

/// A track as a person names it: its kind's letter and its 1-based number within the kind (`V1`, `A2`).
fn track_word(tl: &Timeline, ti: usize) -> String {
    tl.track_name(ti).unwrap_or_else(|| String::from("A1")) // STRIATRACKGAINKEYSNEXT (SR983): timeline_core's ONE name rule
}

// ---- STRIAGAPSBLACK (LEDGER SR909): the close-gaps message row names the black fades it removed (named tail block) ----
// `close-gaps` still rides timeline_core's ONE verb dispatch (one commit); this only reads the selected track before and after
// to name what went: a refusal is the usual `close-gaps: <why>`, and a close that removed a closed gap's to-black / from-black
// says so (`close-gaps: closed 2 gaps, removed 1 fade`). A close with no fade removed stays silent, as before.
impl Face {
    /// The `close-gaps` word with its note; `None` for any other word.
    fn gaps_note_verb(&mut self, v: &str) -> Option<bool> {
        if let Some(r) = self.gaps_three_note_verb(v) { return Some(r); } if let Some(r) = self.gaps_more_note_verb(v) { return Some(r); } if let Some(r) = self.gaps_all_note_verb(v) { return Some(r); } if v != "close-gaps" { // STRIAGAPSALLTRACKS (SR929): `close-gaps all` and its note (file tail); STRIAGAPSMORE (SR969): `close-all` and `close-gaps sync` (file tail); STRIAGAPSTHREE (SR9028): `close-sync` and `close-gaps keep` (file tail)
            return None;
        }
        let track = self.state.selection.map(|(t, _)| t);
        let count = |tl: &Timeline, t: Option<usize>| t.and_then(|t| tl.tracks.get(t)).map_or(0, |k| k.transitions.len());
        let (gaps, fades) = (track.map_or(0, |t| self.state.timeline.gap_count(t)), count(&self.state.timeline, track));
        match self.state.verb(v) {
            Some(Err(e)) => self.last_error = Some(format!("{v}: {e}")),
            Some(Ok(())) => {
                let gone = fades.saturating_sub(count(&self.state.timeline, track));
                if gone > 0 {
                    let s = |n: usize| if n == 1 { "" } else { "s" };
                    self.note = Some(format!("{v}: closed {gaps} gap{}, removed {gone} fade{}", s(gaps), s(gone))); // STRIAGAPSTHREE (SR9028): a success line is a note
                }
            }
            None => return Some(false),
        }
        Some(true)
    }
}

// ---- STRIAPERFPREVIEWCACHE (LEDGER SR918): the preview still and the reel summaries cached by key (named tail block) ----
// SR890 measured the paint's hot spot: `paint_role` re-rendered `preview` on every paint a played tick did not feed (a select, a
// hover tick, a message, a help toggle each re-decoded the unchanged playhead frame), and `reel_line` re-read
// `Sources::summary` every paint. The still is now kept by key like `trans_still`: (the timeline as painted, the playhead); a
// paint with the same key renders nothing and shows the still a render would give (render is deterministic per
// (timeline, sources, t); the face's `Arc<Sources>` is fixed after `Face::new`), so every golden is byte-identical. A played
// still drops the key (the next idle paint renders once, as before); a preview error drops it (an error re-reports every paint,
// as before). A reel's summary is read once per SourceId. Owed: NLE-PERF-PREVIEW-CACHE-AUDIO, NLE-PERF-PREVIEW-PLAYED-KEY,
// NLE-PERF-MESSAGE-SHAPE.

use timeline_core::SourceId;

/// STRIAPERFPREVIEWCACHE (SR918): the key of the face's preview still, the count of `preview` renders the paint made, and the
/// reel summaries read so far with their count (locked only for a lookup or a push, never across a summary).
#[derive(Default)]
struct PreviewCache {
    key: Option<(Timeline, i64)>,
    renders: u64,
    played: Option<(Timeline, i64)>, // STRIAPERFMORE (SR974): the last played render's (timeline, playhead) when it rendered one tick (file tail)
    band: std::sync::Mutex<Option<BandPx>>, // STRIAPERFTHREE (SR9037): the painted preview band kept by still (file tail)
    summaries: std::sync::Mutex<(Vec<(SourceId, Option<String>)>, u64)>,
}

impl PreviewCache {
    /// `taken`: a played tick's still was taken; it drops the key. Returns `taken`.
    fn taken(&mut self, taken: bool) -> bool {
        if taken {
            self.key = self.played.take(); // STRIAPERFMORE (SR974): a taken one-tick still is preview's for that key (file tail)
        }
        taken
    }
    /// True when the still was rendered for `(tl, t)`; otherwise the key becomes `(tl, t)`, one render is counted, and the
    /// caller renders `preview`.
    fn hit(&mut self, tl: &Timeline, t: i64) -> bool {
        if self.key.as_ref().is_some_and(|(k, kt)| *kt == t && k == tl) || self.video_hit(tl, t) { // STRIAPERFMORE (SR974): an audio-only change keeps the still (file tail)
            return true;
        }
        self.key = Some((tl.clone(), t));
        self.renders += 1;
        false
    }
    /// The render failed: no key (the next paint renders again).
    fn miss(&mut self) {
        self.key = None;
    }
    /// Source `id`'s `Sources::summary`, read once per id.
    fn summary(&self, src: &Sources, id: SourceId) -> Option<String> {
        if let Some((_, s)) = self.summaries.lock().unwrap_or_else(|e| e.into_inner()).0.iter().find(|(i, _)| *i == id) {
            return s.clone();
        }
        let s = src.summary(id);
        let mut g = self.summaries.lock().unwrap_or_else(|e| e.into_inner());
        g.0.push((id, s.clone()));
        g.1 += 1;
        s
    }
}

impl Face {
    /// STRIAPERFPREVIEWCACHE (SR918): how many `preview` renders the paint has made (one per changed key).
    pub fn preview_renders(&self) -> u64 {
        self.pcache.renders
    }
    /// STRIAPERFPREVIEWCACHE (SR918): how many `Sources::summary` reads the message row has made (one per reel).
    pub fn summary_reads(&self) -> u64 {
        self.pcache.summaries.lock().unwrap_or_else(|e| e.into_inner()).1
    }
}

#[cfg(test)]
mod striaperfpreviewcache_tests {
    use super::*;

    fn face() -> Face {
        let (tl, src) = synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 240), PixelOrder::Rgba, false);
        f.state.playhead = 3;
        f
    }

    fn pixels(blits: Vec<SMessage>) -> Vec<u8> {
        match blits.into_iter().last() {
            Some(SMessage::SurfaceBlit { pixels, .. }) => pixels,
            _ => panic!("a blit"),
        }
    }

    /// STRIAPERFPREVIEWCACHE M3: a help toggle, a message and a select with the playhead and timeline unchanged decode nothing.
    #[test]
    fn striaperfpreviewcache_same_key_decodes_once() {
        let mut f = face();
        f.frames();
        assert_eq!(f.preview_renders(), 1);
        assert!(f.verb("help"));
        f.frames();
        assert!(f.verb("help"));
        f.frames();
        f.last_error = Some("a message".into());
        f.frames();
        f.state.select(0, 0).expect("clip 0 on V1");
        f.frames();
        assert_eq!(f.state.playhead, 3);
        assert_eq!(f.preview_renders(), 1, "same (timeline, playhead): no second render");
        let want = preview(&f.state.timeline, &f.sources, 3).expect("renders");
        assert_eq!(f.still().cloned(), want);
    }

    /// STRIAPERFPREVIEWCACHE M3: a playhead move, an edit and its undo each render once; split, the tracks Surface renders none.
    #[test]
    fn striaperfpreviewcache_key_change_renders() {
        let mut f = face();
        f.frames();
        f.state.playhead = 14;
        f.frames();
        assert_eq!(f.preview_renders(), 2);
        f.state.select(0, 1).expect("reel B clip under tick 14");
        f.frames();
        assert_eq!(f.preview_renders(), 2, "a select keeps the key");
        assert_eq!(f.state.verb("split"), Some(Ok(())));
        f.frames();
        assert_eq!(f.preview_renders(), 3);
        assert_eq!(f.state.verb("undo"), Some(Ok(())));
        f.frames();
        f.frames();
        assert_eq!(f.preview_renders(), 4, "the cache holds one key");
        let mut s = face().on_surfaces(1, 2);
        assert_eq!(s.frames().len(), 2);
        assert_eq!(s.frames().len(), 2);
        assert_eq!(s.preview_renders(), 1, "split: one render per key");
    }

    /// STRIAPERFPREVIEWCACHE M3: a paint from the cached still is byte-identical to a fresh face's paint of the same state.
    #[test]
    fn striaperfpreviewcache_bytes_identical() {
        let mut a = face();
        a.frames();
        assert!(a.verb("help"));
        assert!(a.verb("help"));
        let cached = pixels(a.frames());
        assert_eq!(a.preview_renders(), 1);
        let mut b = face();
        assert_eq!(cached, pixels(b.frames()));
    }

    /// STRIAPERFPREVIEWCACHE M3: the message row reads a reel's summary once.
    #[test]
    fn striaperfpreviewcache_summary_once_per_reel() {
        let mut f = face();
        let (a, b) = (f.state.timeline.tracks[0].clips[0].src, f.state.timeline.tracks[0].clips[1].src);
        f.state.select(0, 0).expect("reel A clip");
        let line = f.reel_line();
        assert_eq!(f.reel_line(), line);
        assert_eq!(f.reel_line(), line);
        assert_eq!(f.summary_reads(), 1);
        assert_eq!(line, f.sources.summary(a).map(|s| format!("A: {s}")));
        f.state.select(0, 1).expect("reel B clip");
        assert_eq!(f.reel_line(), f.sources.summary(b).map(|s| format!("B: {s}")));
        assert_eq!(f.summary_reads(), 2);
        f.state.select(0, 0).expect("reel A clip");
        assert_eq!(f.reel_line(), line);
        assert_eq!(f.summary_reads(), 2);
    }
}

// ---- STRIAEXPORTQUEUESKIP (LEDGER SR914): the queue's skip / clear words and the queued-timeline snapshot (named tail block) ----
// `export-skip` cancels the running export through SR881's `Face::export_skip` (the next queued request starts when
// `export_tick` reports the cancel); `export-clear` drops the waiting requests and leaves the running job. Both are view
// words: no `commit`, no undo step, no dirty document. Each queued request carries the timeline as it stood when queued
// (`ExportRequest.timeline`); `start_request` swaps it into `self.state.timeline` around the unchanged `start_export`
// (face thread only, nothing paints in between), so its range check, total, span and the thread's clone all read the
// snapshot. Owed: NLE5D-EXPORT-QUEUE-KEY (a key for export-skip; Esc stays cancel-all).

impl Face {
    /// Start a queued request over its own snapshot timeline through the unchanged [`Face::start_export`]; the live
    /// timeline is back in place before this returns, whatever the outcome.
    fn start_request(&mut self, r: &mut ExportRequest) -> Result<(), String> {
        std::mem::swap(&mut self.state.timeline, &mut r.timeline);
        let res = self.start_export(&r.dir, &r.fmt, None, r.range.clone(), r.audio, r.opts.clone());
        std::mem::swap(&mut self.state.timeline, &mut r.timeline);
        res
    }
    /// The queue's words: `export-skip` (`export: skipping (<n> waiting)`, or `export-skip: no export running`) and
    /// `export-clear` (`export: dropped <n> waiting`); `None` for any other word.
    fn export_queue_verb(&mut self, v: &str) -> Option<bool> {
        match v {
            "export-skip" => {
                let line = if self.export_skip() { format!("export: skipping ({} waiting)", self.export_queue.len()) } else { "export-skip: no export running".to_string() };
                self.last_error = Some(line);
                Some(true)
            }
            "export-clear" => {
                let n = self.export_clear();
                self.last_error = Some(format!("export: dropped {n} waiting"));
                Some(true)
            }
            _ => None,
        }
    }
    /// Drop the waiting export requests, the running job untouched; the number dropped.
    pub fn export_clear(&mut self) -> usize {
        let n = self.export_queue.len();
        self.export_queue.clear();
        n
    }
}

// ---- STRIAKEYMAPSTORE (LEDGER SR917): the key map's Principia row (named tail block) ----
// The key map's HOME is Principia's store (`stria.keys`, prefs_core SCHEMA): stria-view reads it once at a window's open
// through quartzite `chrome_rows::stored_str` and hands the text to [`Face::with_keys`]; `--keys` on the command line wins.
// The face reads no store; the witness never reads the person's store.

/// STRIAKEYMAPSTORE (SR917): Principia's namespace for the key map row.
pub const KEYS_NS: &str = "stria";
/// STRIAKEYMAPSTORE (SR917): Principia's key for the key map row (`stria.keys`).
pub const KEYS_KEY: &str = "keys";

/// STRIAKEYMAPSTORE (SR917): the key map text a window opens with: the `--keys` flag when given (even empty; the store is
/// then not asked), else the stored row, else "" (the built-in KEYS table alone).
pub fn keys_text(flag: Option<&str>, stored: impl FnOnce() -> Option<String>) -> String {
    match flag {
        Some(t) => t.to_string(),
        None => stored().unwrap_or_default(),
    }
}

// ---- STRIAPASTEKEYS (LEDGER SR921): the clipboard chords through KEYS (named tail block) ----
// The surfaces never send Ctrl / Cmd + c or v as a key: GTK and AppKit send `BrowserCopy` for c and GTK `BrowserPaste(text)`
// for v (Shift included, only when the OS clipboard holds text). Each is routed into `Face::key` under its chord name, so
// the help's dismiss, the VERBS road and the refusal line are the one existing path; the platform text is ignored (Stria's
// clipboard is the editor's, SR854). Ctrl+x and Ctrl+Shift+V arrive as `BrowserKey` from a surface that sends them.
// Owed: NLE-PASTE-KEYS-MAC, NLE-PASTE-KEYS-SHIFTV, NLE-PASTE-KEYS-EMPTY, NLE-PASTE-KEYS-KERNEL.
impl Face {
    /// `BrowserCopy` / `BrowserPaste` as the `Ctrl+c` / `Ctrl+v` KEYS chords (`Some(repaint)`); `None` for any other message.
    fn clipboard_key(&mut self, msg: &SMessage) -> Option<bool> {
        match msg {
            SMessage::BrowserCopy => Some(self.key("Ctrl+c", false)),
            SMessage::BrowserPaste(_) => Some(self.key("Ctrl+v", false)),
            _ => None,
        }
    }
}

// ---- STRIAUNDOHISTORYDRAG (LEDGER SR912): `history <n>` (named tail block) ----
// `history <n>` lists the top n undo entries, newest first, on the one message line (SR876's `history_line`), n in
// 1..=`UNDO_MAX` (the stack's own cap: bounded). Any other count is refused by name on the message line. View-only:
// no commit, no dirty, no undo step. `history` alone keeps SR876's `HISTORY_N`.
impl Face {
    /// `history <n>`: the line, or the refusal; `None` for any other word.
    fn history_rows_verb(&mut self, v: &str) -> Option<bool> {
        let arg = v.strip_prefix("history ")?.trim();
        let max = timeline_core::editor::UNDO_MAX;
        self.last_error = Some(match arg.parse::<usize>() {
            Ok(n) if (1..=max).contains(&n) => { let s = self.state.history_line(n); self.note = Some(self.with_redo(s.clone(), n)); s } // STRIAUNDOTHREE (SR9027): the redo entries ride the note (file tail)
            _ => format!("history: {arg:?} is not a count 1..={max}"),
        });
        Some(true)
    }
}

// ---- STRIAAUTOSAVECAP (LEDGER SR913): the count past UNDO_MAX, the witness field, a declined autosave kept (named tail block) ----
// An autosave this face neither wrote nor recovered (`Autosave.written == None`: one left by an older session, offered at
// open and declined, or a stale one) is renamed aside to `<stem>.autosave.1.edl` (one generation) before the first write
// would replace it and before `save` would remove it. A failed rename aborts that write / skips that remove, so the old
// file is never lost. Quiet: no message row line. Owed: NLE-HOST-AUTOSAVE-KEEP-READ (no word reads the kept file).

/// STRIAAUTOSAVECAP (SR913): where a declined autosave beside `doc` is kept (`cut.edl` -> `cut.autosave.1.edl`).
pub fn autosave_keep_path(doc: &std::path::Path) -> std::path::PathBuf {
    doc.with_extension("autosave.1.edl")
}

impl Face {
    /// STRIAAUTOSAVECAP (SR913): when this face holds no autosave of its own and `<stem>.autosave.edl` stands, rename it to
    /// [`autosave_keep_path`]; Err (`<path>: <why>`) when that rename fails.
    fn keep_foreign_autosave(&mut self, doc: &std::path::Path) -> Result<(), String> {
        if self.autosave.written.is_some() {
            return Ok(());
        }
        let path = autosave_path(doc);
        if !path.is_file() {
            return Ok(());
        }
        let keep = autosave_keep_path(doc);
        std::fs::rename(&path, &keep).map_err(|e| format!("{} -> {}: {e}", path.display(), keep.display()))?; self.note_keep(&keep); Ok(()) // STRIAAUTOSAVEKEEPREAD (SR933): the keep named once (file tail)
    }
}

// ---- STRIAAUTOSAVEKEEPREAD (LEDGER SR933): recover 1 reads the kept autosave; the keep named (named tail block) ----
// `recover 1` applies `<stem>.autosave.1.edl` (SR913's kept file) through SR883's one recover road (`recover_from`): the same
// refusals by name, one commit through `EditorState::restore`. The face does not own `<stem>.autosave.edl` after it (that
// file does not hold the timeline), so `written` stays None and the dirty face writes its own autosave on the clock. When
// SR913's rename keeps a file, the message row names it once, and only when no error already stands there.
// Owed: NLE-HOST-AUTOSAVE-KEEP-OFFER (the kept file is not offered at open).

/// STRIAAUTOSAVEKEEPREAD (SR933): the generation a `recover` word names: `recover` 0, `recover <n>` n (a word that is not
/// a number: `usize::MAX`, refused by [`autosave_generation_path`]); None for any other word.
fn recover_generation(v: &str) -> Option<usize> {
    let mut words = v.split_whitespace();
    if words.next() != Some("recover") {
        return None;
    }
    match words.next() {
        None => Some(0),
        Some(w) => Some(w.parse::<usize>().unwrap_or(usize::MAX)),
    }
}

/// STRIAAUTOSAVEKEEPREAD (SR933): the autosave file of `generation` beside `doc`: 0 [`autosave_path`], 1
/// [`autosave_keep_path`]; any other is refused (one older autosave is kept).
pub fn autosave_generation_path(doc: &std::path::Path, generation: usize) -> Result<std::path::PathBuf, String> {
    match generation {
        0 => Ok(autosave_path(doc)),
        1 => Ok(autosave_keep_path(doc)),
        _ => Err("one older autosave is kept: recover 1".into()),
    }
}

impl Face {
    /// STRIAAUTOSAVEKEEPREAD (SR933): name the kept file on the message row (`autosave: kept the older autosave as <name>`)
    /// only when no message already stands there; a later failed write in the same call replaces it (the real error wins).
    fn note_keep(&mut self, keep: &std::path::Path) {
        if self.last_error.is_some() {
            return;
        }
        let name = keep.file_name().map_or_else(|| keep.display().to_string(), |n| n.to_string_lossy().into_owned());
        self.last_error = Some(format!("autosave: kept the older autosave as {name}"));
    }
}

// ---- STRIATRACKORDERKEYS (LEDGER SR922): track-up / track-down name the move on the message row (named tail block) ----
// `Alt+Up` / `Alt+Down` (KEYS rows, HELP phrases) and the two transport words reach `Face::verb`, whose first line hands them
// here: the ONE dispatch (`EditorState::verb` -> `move_track`, one commit) runs, a refusal is `<word>: <why>` as before, and a
// move is named on the message row from the state after it (`EditorState::track_move_line`: `track-up: V2 above V1`).
impl Face {
    /// The `track-up` / `track-down` words; `None` for any other word.
    fn track_order_word(&mut self, v: &str) -> Option<bool> {
        let up = match v {
            "track-up" => true,
            "track-down" => false,
            _ => return None,
        };
        match self.state.verb(v).unwrap_or(Err(EditError::NoClip)) {
            Ok(()) => self.last_error = self.state.track_move_line(up),
            Err(e) => self.last_error = Some(format!("{v}: {e}")),
        }
        Some(true)
    }
}

// ---- STRIAPROXYOFF (LEDGER SR915): proxy off / all / cancel / auto-attach (named tail block) ----
// The proxy's other words, every build on SR882's ONE road (`Face::proxy_start`): `proxy off` detaches the selected clip's
// source's proxy (`Sources::clear_proxy`: the preview reads the original again; the files stay on disk); `proxy all` queues
// every reel with a source path, in source order, and runs them one job at a time (`finish_proxy` chains `proxy_next`);
// Esc (after the drag and the export) cancels the running job and drops the queue, refused inside the job with the export's
// own `Interrupted` shape; a FRESH proxy beside a reel attaches at open (`with_reel_paths`), a stale one is left alone.
// View-only: no undo step, the document clean. Owed: NLE-PROXY-BUTTONS, NLE-PROXY-PURGE.

/// STRIAPROXYOFF (LEDGER SR915): the proxy job's progress gate after frame `k`: hold while `hold == Some(k)` until
/// cancelled (the test seam, [`Face::hold_proxy_at`]), then refuse with `Interrupted "cancelled"` once Esc asked.
fn proxy_gate(c: &AtomicBool, hold: Option<u64>, k: u64) -> Result<(), ExportError> {
    if hold == Some(k) {
        while !c.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    if c.load(Relaxed) {
        return Err(ExportError::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")));
    }
    Ok(())
}

impl Face {
    /// `proxy off` / `proxy all` / `proxy <other>` (the usage); the bare word is SR882's [`Face::proxy_verb`] (None here).
    fn proxy_words(&mut self, v: &str) -> Option<bool> {
        let mut words = proxy_alias(v).split_whitespace(); // STRIAPROXYBUTTONS (SR930): the button words read as their arguments (file tail)
        if words.next() != Some("proxy") {
            return None;
        }
        let args: Vec<&str> = words.collect();
        match args.as_slice() {
            [] => return None,
            ["off"] => self.proxy_off(),
            ["all"] => self.proxy_all(), ["purge"] => self.proxy_purge(), ["purge", "all"] | ["purge-all"] => self.proxy_purge_all(), // STRIAPROXYMORE (SR979): purge every source (file tail); STRIAPROXYBUTTONS (SR930): purge detaches then removes the files (file tail)
            _ => self.last_error = Some("proxy: usage: proxy [off|all|purge [all]]".into()), // STRIAPROXYMORE (SR979): purge all joins the usage
        }
        Some(true)
    }
    /// `proxy off` on the selected clip's source: `proxy: <reel>: off (the preview reads the original)` or `.. no proxy
    /// attached`; refused while that source's own proxy is building.
    fn proxy_off(&mut self) {
        let Some(c) = self.state.selected() else {
            self.last_error = Some("proxy: no clip selected".into());
            return;
        };
        let reel = self.state.timeline.source(c.src).map_or_else(|| c.src.0.to_string(), |s| s.reel.clone());
        if let (Some(j), Some((d, t))) = (self.proxy.as_ref(), self.proxy_progress()) {
            if j.src == c.src {
                self.last_error = Some(format!("proxy: {reel}: busy ({d}/{t}): Esc cancels the build"));
                return;
            }
        }
        let line = if self.sources.clear_proxy(c.src) { "off (the preview reads the original)" } else { "no proxy attached" };
        self.last_error = Some(format!("proxy: {reel}: {line}"));
    }
    /// `proxy all`: every source whose reel has a path, in source order, through [`Face::proxy_next`].
    fn proxy_all(&mut self) {
        if let Some((d, t)) = self.proxy_progress() {
            self.last_error = Some(format!("proxy: busy ({d}/{t})"));
            return;
        }
        let queue: Vec<(timeline_core::SourceId, String)> = self.state.timeline.sources.iter().filter(|s| self.reel_paths.iter().any(|(r, _)| *r == s.reel)).map(|s| (s.id, s.reel.clone())).collect();
        if queue.is_empty() {
            self.last_error = Some("proxy: all: no reel has a source path".into());
            return;
        }
        self.last_error = Some(format!("proxy: all: {} reels", queue.len()));
        self.proxy_queue = queue;
        self.proxy_next();
    }
    /// Run the queue's next reels through SR882's `proxy_start` until one starts a job or the queue empties; each step is
    /// appended to the message line as `; <reel>: <outcome>` (`proxy: <reel>: <outcome>` when the line is empty).
    fn proxy_next(&mut self) {
        let mut line = self.last_error.take();
        while self.proxy.is_none() && !self.proxy_queue.is_empty() {
            let (src, reel) = self.proxy_queue.remove(0);
            let r = self.proxy_start(src, &reel).unwrap_or_else(|e| e);
            line = Some(match line {
                Some(l) => format!("{l}; {reel}: {r}"),
                None => format!("proxy: {reel}: {r}"),
            });
        }
        self.last_error = line;
    }
    /// Esc's proxy arm: ask the running job to stop (its next frame refuses) and drop the `proxy all` queue; false when
    /// no job runs.
    pub fn cancel_proxy(&mut self) -> bool {
        let Some(j) = self.proxy.as_ref() else { return false };
        j.cancel.store(true, Relaxed);
        self.proxy_queue.clear();
        true
    }
    /// The test seam (the export's `pause_at` shape): the next proxy job holds after frame `k` until it is cancelled.
    pub fn hold_proxy_at(&mut self, k: Option<u64>) {
        self.proxy_hold = k;
    }
    /// Source `src` has an attached proxy (the preview reads it).
    pub fn has_proxy(&self, src: timeline_core::SourceId) -> bool {
        self.sources.has_proxy(src)
    }
    /// At open: attach each reel's FRESH proxy (stamp matches the file, the file is the one opened); a stale or absent
    /// proxy is left alone. Message only when something attached or refused: `proxy: attached A, B` (`; C: <refusal>`).
    fn proxy_auto(&mut self) {
        let (mut attached, mut refused) = (Vec::new(), Vec::new());
        for (reel, path) in self.reel_paths.clone() {
            let Some(src) = self.state.timeline.source_by_reel(&reel).map(|s| s.id) else { continue };
            if !proxy_fresh(&path) || std::fs::metadata(&path).map(|m| m.len()).ok() != self.sources.byte_len(src) {
                continue;
            }
            let (proxy, _, _) = proxy_paths(&path);
            match std::fs::read(&proxy).map_err(|e| format!("{}: {e}", proxy.display())).and_then(|b| self.sources.set_proxy(src, b).map_err(|e| e.to_string())) {
                Ok(()) => attached.push(reel),
                Err(e) => refused.push(format!("{reel}: {e}")),
            }
        }
        if attached.is_empty() && refused.is_empty() {
            return;
        }
        let mut line = format!("proxy: attached {}", if attached.is_empty() { "none".to_string() } else { attached.join(", ") });
        for r in refused {
            line.push_str("; ");
            line.push_str(&r);
        }
        self.last_error = Some(line);
    }
}

// ---- STRIAMARKERJUMPKEYS (LEDGER SR923): the jump words' message line (named tail block) ----
// `marker-` / `marker+` / `cut-` / `cut+` (the buttons, Up / Down, `;` / `'`) stay SR894's view words through THE dispatch
// (`EditorState::verb`); the face only words THE message (`last_error`) when the playhead moved: `marker+: <name> at <tc>`,
// `cut+: <tc>` (tc the new playhead's `HH:MM:SS:FF`, its tick when the rate is not an integer fps). A jump with nowhere to go
// leaves the message untouched.

impl Face {
    /// The jump words (`timeline_core::editor::jump_word`) through the one dispatch, with their message; `None` for any other
    /// word.
    fn jump_verb(&mut self, v: &str) -> Option<bool> {
        let (j, _) = timeline_core::editor::jump_word(v)?;
        let ph = self.state.playhead;
        if let Err(e) = self.state.verb(v).unwrap_or(Ok(())) {
            self.last_error = Some(format!("{}: {e}", v.trim()));
            return Some(true);
        }
        let now = self.state.playhead;
        if now != ph {
            let tc = self.state.timeline.fps().map_or_else(|| now.to_string(), |f| timeline_core::edl::timecode(now, f));
            self.last_error = Some(match j {
                timeline_core::editor::Jump::Marker => {
                    let name = self.marker_jump_name(now); // STRIAMARKERRANGES (SR9052): `M1` at a marker, `M1 end` at a range end (file tail)
                    format!("{}: {name} at {tc}", v.trim())
                }
                timeline_core::editor::Jump::Cut => format!("{}: {tc}", v.trim()),
            });
        }
        Some(true)
    }
}

// ---- STRIAINSERTMARKSKEYS (LEDGER SR924): keys for the insert words and the laid line (named tail block) ----
// KEYS `w` / `d` / `W` / `D` bind `insert` / `overwrite` / `insert-marked` / `overwrite-marked` (HELP phrases beside them);
// each word goes through timeline_core's ONE dispatch (`EditorState::verb`, one commit). A laid insert leaves a note on the
// message line, read by `message_text` after the export line: `<word>: <reel> <length> at <record in>` (timecode at the
// timeline's rate, frames for a non-integer timebase), cleared by the next taken input (a word, a click, a press, a bound key).
// It is not `last_error` (the witness counts that as an error). Owed: NLE-INSERT-LAID-TC (SR872's `tc` mode), NLE-INSERT-KERNEL.

impl Face {
    /// The four insert words: timeline_core's dispatch, then the laid line (`Some(true)`); `None` for any other word.
    fn laid_verb(&mut self, v: &str) -> Option<bool> {
        let w = v.split_ascii_whitespace().next()?;
        if !timeline_core::editor::lays(w) { // STRIAINSERTMARKSKEYSNEXT (SR985): the all-tracks words; STRIAKINSERT (SR9062): the laying words held once in timeline_core (the kernel Player reads them too)
            return None;
        }
        match self.state.verb(v)? {
            Ok(()) => self.laid = self.state.laid_line(w), // STRIAKINSERTALL (SR9090): timeline_core's ONE laid line (the kernel Player paints it too)
            Err(e) => {
                self.laid = None;
                self.last_error = Some(format!("{v}: {e}"));
            }
        }
        Some(true)
    }
    /// The laid line stands until the next taken input: a word, a click, a press, or a key the face binds (GTK's
    /// `BrowserKey` twin of a text key is not one).
    fn laid_clear(&mut self, msg: &SMessage) {
        let taken = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserButton { down: true, .. } => true,
            SMessage::BrowserKey(k) => k.chars().count() != 1 && KEYS.iter().any(|(name, _)| *name == k.as_str()),
            SMessage::BrowserText(t) => t.chars().count() == 1 && KEYS.iter().any(|(name, _)| *name == t.as_str()),
            _ => false,
        };
        if taken {
            self.laid = None;
        }
    }
}

// ---- STRIAGAPSALLTRACKS (LEDGER SR929): the `close-gaps all` message row over every track (named tail block) ----
// `close-gaps all` rides timeline_core's ONE verb dispatch (one commit over every track); this reads the whole timeline before
// and after to name what went, SR909's rule over all tracks: `close-gaps all: closed 3 gaps, removed 1 fade` only when a fade
// went. A refusal names the track that refused (`close-gaps all: A1: <why>`: kind letter and 1-based number among its kind;
// sibling SR922's `Timeline::track_name` and this label become one at the fold).
impl Face {
    /// The `close-gaps all` word with its note; `None` for any other word.
    fn gaps_all_note_verb(&mut self, v: &str) -> Option<bool> {
        if !v.split_ascii_whitespace().eq(["close-gaps", "all"]) {
            return None;
        }
        let fades = |tl: &Timeline| tl.tracks.iter().map(|k| k.transitions.len()).sum::<usize>();
        let (gaps, before) = (self.state.timeline.gap_count_all(), fades(&self.state.timeline));
        match self.state.verb(v) {
            Some(Err(e)) => {
                let tl = &self.state.timeline;
                let name = match tl.close_gaps_all() {
                    Err((t, _)) => {
                        let kind = tl.tracks[t].kind;
                        let n = tl.tracks[..=t].iter().filter(|k| k.kind == kind).count();
                        format!("{}{n}: ", if kind == Kind::Video { 'V' } else { 'A' })
                    }
                    Ok(_) => String::new(),
                };
                self.last_error = Some(format!("close-gaps all: {name}{e}"));
            }
            Some(Ok(())) => {
                let gone = before.saturating_sub(fades(&self.state.timeline));
                if gone > 0 {
                    let s = |n: usize| if n == 1 { "" } else { "s" };
                    self.note = Some(format!("close-gaps all: closed {gaps} gap{}, removed {gone} fade{}", s(gaps), s(gone))); // STRIAGAPSTHREE (SR9028): a success line is a note
                }
            }
            None => return Some(false),
        }
        Some(true)
    }
}

// ---- STRIAPROXYBUTTONS (LEDGER SR930): the proxy's buttons, purge and reels (named tail block) ----
// The stria-view buttons `Proxy Off` / `Proxy All` send the VERBS words `proxy-off` / `proxy-all`, read by SR915's ONE
// dispatch (`proxy_words`) as `proxy off` / `proxy all`. `proxy purge` (an argument, no button: it deletes files) detaches
// through SR915's `proxy_off` (its selection and busy refusals stand), then removes the selected source's proxy file, stamp
// and any leftover work dir (SR882's `proxy_paths`). `proxy_reels` names the reels with an attached proxy (the witness's
// `"proxy"` field). View-only: no undo step, the document clean. Owed: NLE-PROXY-PURGE-ALL.

/// STRIAPROXYBUTTONS (LEDGER SR930): a button word as the `proxy` argument it stands for; any other text unchanged.
fn proxy_alias(v: &str) -> &str {
    match v.trim() {
        "proxy-off" => "proxy off",
        "proxy-all" => "proxy all",
        _ => v,
    }
}

impl Face {
    /// `proxy purge` on the selected clip's source: [`Face::proxy_off`]'s line, then `; removed <proxy>, <stamp>` /
    /// `; no proxy files` / `; no source path`; refused (proxy_off's busy line) while that source's own job runs.
    fn proxy_purge(&mut self) {
        self.proxy_off();
        let Some(c) = self.state.selected() else { return };
        let src = c.src;
        if self.proxy.as_ref().is_some_and(|j| j.src == src) {
            return;
        }
        self.proxy_queue.retain(|(s, _)| *s != src);
        let tail = self.purge_files(src); // STRIAPROXYMORE (SR979): the one file removal (file tail)
        let line = self.last_error.take().unwrap_or_else(|| "proxy:".to_string());
        self.last_error = Some(format!("{line}; {tail}"));
    }
    /// The reels whose source has an attached proxy, in source order (the witness's `"proxy"` field).
    pub fn proxy_reels(&self) -> Vec<String> {
        self.state.timeline.sources.iter().filter(|s| self.sources.has_proxy(s.id)).map(|s| s.reel.clone()).collect()
    }
}

// ---- STRIAPROXYMORE (LEDGER SR979): purge all, the shared file removal (named tail block) ----
// `proxy purge all` (also `proxy purge-all`, the witness's `--do proxy:purge-all`) walks every timeline source in source
// order: the running job's source is skipped by name (proxy_off's busy line), every other is detached
// (`Sources::clear_proxy`) and its proxy file, stamp and leftover work dir are removed through the ONE removal fn
// [`Face::purge_files`] that SR930's `proxy purge` also calls. The `proxy all` queue is dropped; a running job of another
// reel finishes. View-only: no undo step, the document clean.

impl Face {
    /// Remove source `src`'s proxy file, stamp and leftover work dir (SR882's `proxy_paths` beside its reel's path):
    /// `removed <p>, ..` / `no proxy files` / `no source path`. Detaches nothing.
    fn purge_files(&self, src: timeline_core::SourceId) -> String {
        let reel = self.state.timeline.source(src).map(|s| s.reel.clone());
        let file = reel.and_then(|r| self.reel_paths.iter().find(|(p, _)| *p == r).map(|(_, f)| f.clone()));
        match file {
            None => "no source path".to_string(),
            Some(f) => {
                let (proxy, stamp, work) = proxy_paths(&f);
                let mut removed = Vec::new();
                for p in [&proxy, &stamp] {
                    if p.is_file() && std::fs::remove_file(p).is_ok() {
                        removed.push(p.display().to_string());
                    }
                }
                if work.is_dir() && std::fs::remove_dir_all(&work).is_ok() {
                    removed.push(work.display().to_string());
                }
                if removed.is_empty() { "no proxy files".to_string() } else { format!("removed {}", removed.join(", ")) }
            }
        }
    }
    /// `proxy purge all`: `proxy: purge all: <reel>: [off, ]<purge_files line>; ..` per source in source order, the running
    /// job's source `<reel>: busy (d/t): Esc cancels the build`; `proxy: purge all: no reels` on a timeline without sources.
    fn proxy_purge_all(&mut self) {
        self.proxy_queue.clear();
        let running = self.proxy.as_ref().map(|j| j.src);
        let progress = self.proxy_progress();
        let sources: Vec<(timeline_core::SourceId, String)> = self.state.timeline.sources.iter().map(|s| (s.id, s.reel.clone())).collect();
        if sources.is_empty() {
            self.last_error = Some("proxy: purge all: no reels".into());
            return;
        }
        let mut parts = Vec::new();
        for (src, reel) in sources {
            if running == Some(src) {
                let (d, t) = progress.unwrap_or((0, 0));
                parts.push(format!("{reel}: busy ({d}/{t}): Esc cancels the build"));
                continue;
            }
            let off = if self.sources.clear_proxy(src) { "off, " } else { "" };
            parts.push(format!("{reel}: {off}{}", self.purge_files(src)));
        }
        self.last_error = Some(format!("proxy: purge all: {}", parts.join("; ")));
    }
}

// ---- STRIAEXPORTQUEUELIST (LEDGER SR932): the export queue read back on the message row (named tail block) ----
// `export-list` (a third arm of SR914's `export_queue_verb`) puts the waiting requests on the message row: `export: <n>
// waiting: <name> <fmt>, ...`, at most `EXPORT_LIST_SHOWN` entries then `, +<k> more`, or `export: nothing waiting`. A view
// word: no `commit`, no undo step, no dirty document; the queue and the running job untouched. KEYS `E` (Shift+e, text:
// GTK and AppKit both send it; AppKit drops Escape) keys SR914's `export-skip` through `Face::verb`.
// Owed: NLE5D-EXPORT-QUEUE-LIST-RANGE (each request's range / audio word), NLE5D-EXPORT-QUEUE-ESC-GTK (a GTK `Shift+Escape` alias).

/// STRIAEXPORTQUEUELIST (SR932): the most waiting requests `export-list` names; the rest are counted `+<k> more`.
pub const EXPORT_LIST_SHOWN: usize = 3;

impl Face {
    /// The waiting export requests as one message line: `export: nothing waiting`, else `export: <n> waiting: <name> <fmt>,
    /// ...` (the request directory's last component, else the whole path; at most [`EXPORT_LIST_SHOWN`], then `, +<k> more`).
    pub fn export_list(&self) -> String {
        let n = self.export_queue.len();
        if n == 0 {
            return "export: nothing waiting".to_string();
        }
        let names: Vec<String> = self.export_queue.iter().take(EXPORT_LIST_SHOWN).map(|r| {
            let name = entry_name(&r.dir); // STRIAEXPORTRANGEBAND (SR9099): the one name rule, lifted (file tail)
            format!("{name} {}{}", r.fmt, list_entry_note(r)) // STRIAEXPORTQUEUEMORE (SR971): the range / audio word (file tail)
        }).collect();
        let more = if n > EXPORT_LIST_SHOWN { format!(", +{} more", n - EXPORT_LIST_SHOWN) } else { String::new() };
        format!("export: {n} waiting: {}{more}", names.join(", "))
    }
}

// ---- STRIAUNDOJUMP (LEDGER SR934): `undo <n>` / `redo <n>` and the landing line (named tail block) ----
// `undo <n>` / `redo <n>` step the core's ONE `undo` / `redo` n times (n in 1..=the stack's depth), so every step stays its
// own entry and `redo <n>` returns exactly. The message line names where it landed through SR876's names: the oldest of
// the n entries undone (`history(n)` read BEFORE stepping), or the newest entry after a redo (`history(1)`); an unnamed
// step reads `(unnamed)`. Any other count is refused by name and nothing moves. Plain `undo` / `redo` are unchanged.
impl Face {
    /// `undo <n>` / `redo <n>`: the steps and the landing line, or the refusal; `None` for any other word.
    fn undo_jump_verb(&mut self, v: &str) -> Option<bool> {
        self.last_error = Some(self.state.jump(v)?); // STRIAKUNDOHIST (SR9004): the rule lives in timeline_core `EditorState::jump`, the kernel Player's too
        Some(true)
    }
}

// ---- STRIAPERFLABELSHAPE (LEDGER SR935): shaped label lines cached by text on the face (named tail block) ----
// SR890 measured the face's `Labels` hook running a full `TextStyle::shape` for every clip label, every ruler timecode, the
// message line and every help row on every paint (the help also measured each row by a second shape). The face now keeps the
// shaped `Line`s by text in one bounded `LabelCache` (a Mutex: `paint_message` / `paint_help` take `&self`): a text is shaped
// once per style and painted from the cache after. A `Line` does not depend on the byte order (`Line::paint` takes it), so the
// key is the style alone; a style change drops every entry; a full cache (LABEL_CACHE_MAX texts, e.g. the export message's
// churn) is cleared. The cached Line is the one `shape` gives for the same (style, text), painted by the same `Line::paint`,
// so every golden is byte-identical; labels-off paints never reach `Labels`, and the free `paint` / `paint_in` shape uncached.
// timeline_core view.rs is not edited (the kernel Player has its own hook). Owed: NLE-PERF-LABEL-STRING, NLE-PERF-HELP-ROWS,
// NLE-PERF-HELP-COLS, NLE-PERF-LABEL-PAINT.

/// STRIAPERFLABELSHAPE (SR935): the most texts the face's label cache holds before it is cleared.
const LABEL_CACHE_MAX: usize = 512;

/// STRIAPERFLABELSHAPE (SR935): the shaped label lines, locked for a lookup and the one shape a miss makes.
#[derive(Default)]
struct LabelCache {
    inner: std::sync::Mutex<LabelLines>,
}

/// STRIAPERFLABELSHAPE (SR935): the style the lines were shaped in, the lines by text, the style's metrics, the shapes made.
#[derive(Default)]
struct LabelLines {
    style: Option<TextStyle>,
    lines: std::collections::HashMap<String, std::sync::Arc<text_host::line::Line>>,
    metrics: Option<(f32, f32, f32)>,
    shapes: u64,
}

impl LabelLines {
    /// A style other than the cached one drops every line and the metrics.
    fn restyle(&mut self, style: &TextStyle) {
        if self.style.as_ref() != Some(style) {
            self.lines.clear();
            self.metrics = None;
            self.style = Some(style.clone());
        }
    }
}

impl LabelCache {
    /// `s` shaped in `style`: the cached line, else one `TextStyle::shape` (counted) kept by text.
    fn line(&self, style: &TextStyle, s: &str) -> std::sync::Arc<text_host::line::Line> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.restyle(style);
        if let Some(l) = g.lines.get(s) {
            return l.clone();
        }
        if g.lines.len() >= LABEL_CACHE_MAX {
            g.lines.clear();
        }
        let l = std::sync::Arc::new(style.shape(s));
        g.shapes += 1;
        g.lines.insert(s.to_string(), l.clone());
        l
    }
    /// The advance of `s` in `style` (the value `TextStyle::width` gives), from the cached line.
    fn width(&self, style: &TextStyle, s: &str) -> f32 {
        self.line(style, s).width
    }
    /// `style.metrics()`, once per style.
    fn metrics(&self, style: &TextStyle) -> (f32, f32, f32) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.restyle(style);
        if let Some(m) = g.metrics {
            return m;
        }
        let m = style.metrics();
        g.metrics = Some(m);
        m
    }
}

/// STRIAPERFLABELSHAPE (SR935): `s` shaped in `style`, through `cache` when the face gives one.
fn shaped(style: &TextStyle, cache: Option<&LabelCache>, s: &str) -> std::sync::Arc<text_host::line::Line> {
    cache.map_or_else(|| std::sync::Arc::new(style.shape(s)), |k| k.line(style, s))
}

/// STRIAPERFLABELSHAPE (SR935): timeline_core's `view::paint_in` with the caller's [`Labels`] (the free [`paint_in`]'s uncached
/// hook, or the face's cached one).
fn paint_labelled(st: &EditorState, l: Layout, order: PixelOrder, lab: Option<Labels<'_>>, preview: Option<&Still>, peaks: Option<&dyn view::PeakSource>) -> Vec<u8> {
    let mut lab = lab;
    view::paint_in(st, l, vo(order), lab.as_mut().map(|t| t as &mut dyn view::Text), preview, peaks)
}

impl Face {
    /// STRIAPERFLABELSHAPE (SR935): how many label lines the face has shaped (one per text per style while cached).
    pub fn label_shapes(&self) -> u64 {
        self.lcache.inner.lock().unwrap_or_else(|e| e.into_inner()).shapes
    }
}

#[cfg(test)]
mod striaperflabelshape_tests {
    use super::*;

    fn face() -> Face {
        let (tl, src) = synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (640, 480), PixelOrder::Rgba, true);
        f.state.playhead = 3;
        f
    }

    fn pixels(blits: Vec<SMessage>) -> Vec<u8> {
        match blits.into_iter().last() {
            Some(SMessage::SurfaceBlit { pixels, .. }) => pixels,
            _ => panic!("a blit"),
        }
    }

    /// STRIAPERFLABELSHAPE M3: a repaint of an unchanged face shapes no label.
    #[test]
    fn striaperflabelshape_repaint_shapes_nothing() {
        let mut f = face();
        f.frames();
        let n = f.label_shapes();
        assert!(n > 0, "the first paint shapes the ruler and clip labels");
        f.frames();
        f.frames();
        assert_eq!(f.label_shapes(), n, "every label is a cache hit");
    }

    /// STRIAPERFLABELSHAPE M3: a paint from cached lines is byte-identical to a fresh face's paint (every line shaped).
    #[test]
    fn striaperflabelshape_bytes_identical() {
        let mut a = face();
        a.frames();
        let cached = pixels(a.frames());
        let mut b = face();
        assert_eq!(cached, pixels(b.frames()));
        assert!(a.verb("help"));
        assert!(b.verb("help"));
        a.frames();
        assert_eq!(pixels(a.frames()), pixels(b.frames()), "the help from cached lines");
    }

    /// STRIAPERFLABELSHAPE M3: the help shapes its rows on the first paint it shows, none on the next.
    #[test]
    fn striaperflabelshape_help_shapes_once() {
        let mut f = face();
        f.frames();
        let n = f.label_shapes();
        assert!(f.verb("help"));
        f.frames();
        let m = f.label_shapes();
        assert!(m > n, "the help rows are shaped once");
        f.frames();
        assert_eq!(f.label_shapes(), m, "a second help paint shapes nothing");
    }

    /// STRIAPERFLABELSHAPE M3: the cache is bounded, a style change drops it, and its width is `TextStyle::width`'s.
    #[test]
    fn striaperflabelshape_bounded_and_style_drop() {
        let k = LabelCache::default();
        let a = TextStyle::new(&["sans"], 11.0);
        for i in 0..=LABEL_CACHE_MAX {
            k.line(&a, &i.to_string());
        }
        assert!(k.inner.lock().unwrap().lines.len() <= LABEL_CACHE_MAX);
        assert_eq!(k.inner.lock().unwrap().shapes, LABEL_CACHE_MAX as u64 + 1);
        assert_eq!(k.width(&a, "00:00:01:00"), a.width("00:00:01:00"));
        let shapes = k.inner.lock().unwrap().shapes;
        k.width(&a, "00:00:01:00");
        assert_eq!(k.inner.lock().unwrap().shapes, shapes, "a hit");
        let b = TextStyle::new(&["sans"], 12.0);
        k.width(&b, "00:00:01:00");
        assert_eq!(k.inner.lock().unwrap().shapes, shapes + 1, "a new style shapes again");
        assert_eq!(k.inner.lock().unwrap().lines.len(), 1, "the old style's lines are gone");
        assert_eq!(k.metrics(&b), b.metrics());
    }
}
// ---- STRIAPERFDRAGSHOWN (LEDGER SR936): the drag preview kept across paints (named tail block) ----

impl Face {
    /// STRIAPERFDRAGSHOWN (SR936): how many times a paint replayed the live drag ([`Drag::shown`] through timeline_core's
    /// [`DragShown`]); paints between two pointer moves reuse the one replay.
    pub fn drag_replays(&self) -> u64 {
        self.drag_shown.replays
    }
    /// STRIAPERFDRAGSHOWN (SR936): forget the kept drag preview, so the next paint replays it (the byte-identity witness).
    pub fn drag_shown_reset(&mut self) {
        self.drag_shown.clear();
    }
}

// ---- STRIAXFORMPRESETS (LEDGER SR938): the message row reads the placement after a pip / scale word (named tail block) ----
// A `pip` / `scale` word (SR879's and SR938's presets / steps) goes through timeline_core's ONE verb dispatch; when it is
// taken the message row reads the selected clip's placement (`pip: x 725 y 725 scale 250`) through `xform_note`, which
// [`Face::message_text`] reads after `last_error`; the next message [`Face::on`] takes clears it. Not `last_error`: a
// success is no error (serve logs `last_error` as a warn; the witness counts it). A refusal is `last_error` as every verb's.
impl Face {
    /// A `pip` / `scale` word: `Some(true)` (taken; the note or the refusal set); `None` for any other word.
    fn xform_note_verb(&mut self, v: &str) -> Option<bool> {
        if !matches!(v.split_ascii_whitespace().next(), Some("pip" | "scale" | "scale-" | "scale+")) {
            return None;
        }
        match self.state.verb(v)? {
            Ok(()) => {
                self.xform_note = self.state.selected().map(|c| (c.xform_at(self.state.playhead), c.xkeys.len(), c.xkeys.ease())).map(|(xf, n, e)| format!("pip: x {} y {} scale {}{}{}", xf.x, xf.y, xf.s, if n > 0 { format!(" keys {n}") } else { String::new() }, if e != timeline_core::Ease::Linear { format!(" ease {}", e.name()) } else { String::new() })); // STRIAXFORMKEYS (SR9049): the placement shown at the playhead, its key count when keyed; STRIAXFORMKEYSEASE (SR9070): its ease when not linear
            }
            Err(e) => self.last_error = Some(format!("{v}: {e}")),
        }
        Some(true)
    }
}

// ---- STRIABINSCROLL (LEDGER SR939): the bin's wheel scroll and the unused reel's lay (named tail block) ----
// The bin reaches every source: the wheel over the bin Surface moves its first shown row `BinSurface::top` one row per
// [`WHEEL_STEP`] notch (down = later rows), clamped by timeline_core view's `bin_clamp`, and view's `paint_bin_from` /
// `bin_hit_from` shift by that ONE offset. A reel that sits on no track yet is laid on the first track of a kind its media
// holds (`Sources::holds`: video first, else audio) through `EditorState::place_at_playhead`, the road SR852's
// `insert_reel` ends in (ONE commit, one undo step); a reel with no media opened keeps the word road and its refusal.

impl Face {
    /// The wheel over the bin: `dy` accumulates (thousandths, the remainder kept) and each whole [`WHEEL_STEP`] moves the
    /// first shown row by one, clamped to the list; true only when the row moved.
    fn bin_scroll(&mut self, dy: f64) -> bool {
        let Some(s) = self.split else { return false };
        let Some(b) = s.bin else { return false };
        let step = (WHEEL_STEP * 1000.0) as i64;
        let acc = b.wheel + (dy * 1000.0) as i64;
        let notches = acc / step;
        let top = view::bin_clamp(&self.state.timeline, b.canvas.1, (b.top as i64 + notches).max(0) as usize);
        let moved = top != view::bin_clamp(&self.state.timeline, b.canvas.1, b.top);
        self.split = Some(Split { bin: Some(BinSurface { top, wheel: acc - notches * step, ..b }), ..s });
        moved
    }
    /// A click on bin row `i` whose reel sits on no track: laid at the playhead on the first track of a kind its media
    /// holds (`Some(true)`; no such track: the word's refusal by name on the message line). `None` for a reel on a track
    /// or one with no media opened (the word road decides).
    fn bin_unused(&mut self, i: usize, range: Option<(i64, i64)>) -> Option<bool> { // STRIABINTHREE (SR9025): the source range (None: the whole reel)
        let r = view::bin_rows(&self.state.timeline).into_iter().nth(i)?;
        if r.kind != view::BinKind::Unused {
            return None;
        }
        let held: Vec<Kind> = [Kind::Video, Kind::Audio].into_iter().filter(|k| self.sources.holds(r.src, *k)).collect();
        if held.is_empty() {
            return None;
        }
        let word = view::bin_range_word(&self.state.timeline, i, range)?; // STRIABINTHREE (SR9025): the range's word (view.rs tail)
        let tracks: Vec<usize> = held.iter().filter_map(|k| self.state.timeline.tracks.iter().position(|t| t.kind == *k)).collect(); // STRIABINMORE (SR976): every held kind's first track (NLE-BIN-AVLINK)
        let res = self.state.place_tracks_at_playhead(&tracks, r.src, range.map_or(0, |p| p.0), range.map_or(r.duration, |p| p.1), timeline_core::Mode::Insert); // STRIABINTHREE (SR9025): the range; STRIABINMORE (SR976): all of them in ONE commit (timeline_core editor.rs tail)
        if let Err(e) = res {
            self.last_error = Some(format!("{word}: {e}"));
        }
        Some(true)
    }
}

#[cfg(test)]
mod striabinscroll_tests {
    use super::*;
    use timeline_core::{Mode, SourceId};

    fn click(n: u32, (x, y): (f64, f64)) -> SMessage {
        SMessage::BrowserClick(x, y, n)
    }

    fn wheel(dy: f64) -> SMessage {
        SMessage::BrowserScroll(0.0, dy, 2)
    }

    /// The fixture plus reel C (synth media: video and audio) on no track.
    fn with_unused() -> (Face, SourceId) {
        let (mut tl, mut src) = synth_fixture();
        let c = tl.add_source("C", 50);
        src.add(c, synth_source(2000, 50, 25)).expect("synthetic source opens");
        (Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).on_surfaces(0, 1).with_bin(2), c)
    }

    /// A one-row bin: a notch down shows row 1 (B) and a click on the first row's pixels inserts B; the scroll clamps to
    /// the list, half a notch is no step, a notch up comes back; the paint is view's `paint_bin_from`.
    #[test]
    fn striabinscroll_wheel_scrolls_rows_and_clamps() {
        let (tl, src) = synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).on_surfaces(0, 1).with_bin(2);
        assert_eq!(f.bin().unwrap().top, 0);
        assert!(!f.on(&wheel(WHEEL_STEP)), "every row fits: nothing to scroll");
        assert_eq!(f.bin().unwrap().top, 0);
        assert!(f.on(&SMessage::BrowserResize(320, view::BIN_ROW_H, 2)));
        assert!(!f.on(&wheel(WHEEL_STEP / 2.0)), "half a notch is no step");
        assert!(f.on(&wheel(WHEEL_STEP / 2.0)));
        assert_eq!(f.bin().unwrap().top, 1);
        assert!(!f.on(&wheel(WHEEL_STEP)), "clamped to the list");
        assert_eq!(f.bin().unwrap().top, 1);
        let want = view::paint_bin_thumb(&f.state.timeline, 320, view::BIN_ROW_H, 1, Order::Rgba, None);
        assert_eq!(f.paint_role(Role::Bin), want);
        let tl0 = f.state.timeline.clone();
        f.state.deselect();
        f.state.seek(15);
        assert!(f.on(&click(2, view::bin_point(0))));
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        assert_eq!(f.state.timeline, tl0.place(0, Clip::new(SourceId(1), 0, 50, 15), Mode::Insert).unwrap(), "row 1 (B) under the first row's pixels");
        assert!(f.on(&wheel(-WHEEL_STEP)));
        assert_eq!(f.bin().unwrap().top, 0);
        assert!(!f.on(&wheel(-WHEEL_STEP)), "clamped at the top");
    }

    /// An unused reel with media is laid on the first video track at the playhead: ONE commit, one undo step, the clip
    /// selected and the playhead at its end; undo restores.
    #[test]
    fn striabinscroll_unused_reel_lays_on_its_kind_track() {
        let (mut f, c) = with_unused();
        let tl0 = f.state.timeline.clone();
        f.state.deselect();
        f.state.seek(15);
        assert!(f.on(&click(2, view::bin_point(2))));
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        assert_eq!(f.state.timeline, tl0.place(0, Clip::new(c, 0, 50, 15), Mode::Insert).unwrap().place(1, Clip::new(c, 0, 50, 15), Mode::Insert).unwrap());
        assert_eq!(f.state.timeline.tracks[1].clips.iter().filter(|x| x.src == c).count(), 1, "the audio track too (STRIABINMORE SR976)");
        assert_eq!((f.state.selected(), f.state.playhead, f.state.undo_depth()), (Some(Clip::new(c, 0, 50, 15)), 65, 1));
        assert!(f.verb("undo"));
        assert_eq!(f.state.timeline, tl0);
    }

    /// No track of a kind the media holds: refused by the word's name, nothing changes; media on an audio-only timeline
    /// goes to the audio track.
    #[test]
    fn striabinscroll_unused_reel_refusal_and_audio_track() {
        let (mut f, c) = with_unused();
        f.state.timeline.tracks.clear();
        let tl0 = f.state.timeline.clone();
        assert!(f.on(&click(2, view::bin_point(2))));
        assert_eq!(f.last_error.take(), Some(format!("insert C 0 50: {}", EditError::NoTrack)));
        assert_eq!((&f.state.timeline, f.state.undo_depth()), (&tl0, 0));
        let (mut g, c2) = with_unused();
        assert_eq!(c, c2);
        g.state.timeline.tracks.remove(0);
        let tl1 = g.state.timeline.clone();
        g.state.deselect();
        g.state.seek(0);
        assert!(g.on(&click(2, view::bin_point(2))));
        assert!(g.last_error.is_none(), "{:?}", g.last_error);
        assert_eq!(g.state.timeline, tl1.place(0, Clip::new(c, 0, 50, 0), Mode::Insert).unwrap(), "the audio track");
    }
}

// ---- STRIAHELPSEARCHLIVE (LEDGER SR941): the help filter typed live, matching the action word (named tail block) ----
// While the keys help shows, a typed character (BrowserText, one char, not `?`, not control, not an AppKit private-use key)
// extends SR874's query and Backspace (BrowserKey, both platforms) shortens it, an empty query being the whole table; the
// key is never applied. A char that would leave no row is refused by name on the one message line (`help: no key matches
// "<q>"`) and not added. Backspace with no query is taken and ignored. Escape and `?` still close (SR744's `key` rule).
// `help_matching` also matches the row's action word, recovered from its phrase through THE `HELP` table. View-only.
// Owed: NLE5D-KEYS-HELP-SEARCH-BUTTON (a query field on the bar), NLE5D-KEYS-HELP-SEARCH-CURSOR (the query drawn on the band).

/// STRIAHELPSEARCHLIVE (SR941): the KEYS action a help row's phrase names: the [`HELP`] entry with that phrase (the inverse
/// of [`help_phrase`], HELP's phrases are distinct), else the phrase itself (an action with no HELP phrase shows its word).
fn help_action(p: &str) -> &str {
    HELP.iter().find(|(_, ph)| *ph == p).map_or(p, |&(a, _)| a)
}

impl Face {
    /// While the help shows: a typed character narrows the query, Backspace widens it (`Some(repaint)`); `None` for every
    /// other message (Escape and `?` reach [`Face::key`]'s dismiss), or with the help hidden.
    fn help_type_input(&mut self, msg: &SMessage) -> Option<bool> {
        if !self.help {
            return None;
        }
        match msg {
            SMessage::BrowserKey(k) if k.as_str() == "BackSpace" => {
                let Some(q) = self.help_query.as_mut() else { return Some(false) };
                q.pop();
                if q.is_empty() {
                    self.help_query = None;
                }
                Some(true)
            }
            SMessage::BrowserText(t) if t.as_str() != "?" && t.chars().count() == 1 && t.chars().all(|c| !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&c)) => {
                let q = format!("{}{}", self.help_query.as_deref().unwrap_or(""), t);
                if help_matching(&q).is_empty() {
                    self.last_error = Some(format!("help: no key matches \"{q}\""));
                    return Some(true);
                }
                self.help_query = Some(q);
                Some(true)
            }
            _ => None,
        }
    }
}

// ---- STRIAJKLMORE (LEDGER SR945): the 8x and reverse shuttle heard, the rate on the message line (named tail block) ----
// SR873's ONE shuttle stretch becomes two audio_core `Wsola` stages: stage a at |rate| up to `MAX_RATE` (4), stage b at 4
// only past it (8x = 2 x 4, pitch kept). A reverse tick with a released device is ONE `nle::render` of `ph + rate..ph`
// through the same `PlaySink` (its `rev` collects the stereo, its `frame` keeps the first frame = the new playhead), heard
// with its frames reversed through the same stretch; without a device the old silent nudge path stands. The message line
// reads `shuttle <rate>x` while playing at any rate but 1 (after an error and the export line, before the reel line).
// Owed: NLE-SHUTTLE-KERNEL (the kernel Player's EDIT_KEYS: kernel file), NLE-SHUTTLE-KHOLD (needs a key-release SMessage).

/// STRIAJKLMORE (SR945): the shuttle's stretch: stage `a` at |rate| (<= [`SHUTTLE_HEARD`]), stage `b` at `SHUTTLE_HEARD` only
/// when |rate| is past it, so 8x is 2x then 4x, the pitch kept by both.
struct ShuttleStretch {
    a: gneiss_pal::dsp::audio::stretch::Wsola,
    b: Option<gneiss_pal::dsp::audio::stretch::Wsola>,
    hz: u32,
}

impl ShuttleStretch {
    fn new(hz: u32) -> ShuttleStretch {
        ShuttleStretch { a: gneiss_pal::dsp::audio::stretch::Wsola::new(hz, 2), b: None, hz }
    }
    /// The shuttle's |rate|: one stage up to [`SHUTTLE_HEARD`], two past it (each change takes effect at its next hop).
    fn set_rate(&mut self, r: f64) {
        let top = SHUTTLE_HEARD as f64;
        if r > top {
            self.a.set_rate(r / top);
            let hz = self.hz;
            self.b.get_or_insert_with(|| gneiss_pal::dsp::audio::stretch::Wsola::new(hz, 2)).set_rate(top);
        } else {
            self.a.set_rate(r);
            self.b = None;
        }
    }
    /// Both stages from silence (the ring was flushed).
    fn reset(&mut self) {
        self.a.reset();
        if let Some(b) = self.b.as_mut() {
            b.reset();
        }
    }
    /// Stereo through the stages: what they yield so far.
    fn run(&mut self, stereo: &[f32]) -> Vec<f32> {
        self.a.push(stereo);
        let mut mid = Vec::new();
        self.a.pull(&mut mid);
        let Some(b) = self.b.as_mut() else { return mid };
        b.push(&mid);
        let mut out = Vec::new();
        b.pull(&mut out);
        out
    }
}

/// STRIAJKLMORE (SR945): interleaved stereo with its frames in reverse order (the last frame first; each frame's L / R kept).
fn reversed_frames(stereo: &[f32]) -> Vec<f32> {
    stereo.chunks_exact(2).rev().flatten().copied().collect()
}

impl Face {
    /// STRIAJKLMORE (SR945): one reverse tick at playhead `ph` with a released device: ONE render of `max(ph + rate, 0)..ph`
    /// through [`PlaySink`] (the meters fed, the first frame kept, the stereo collected), its frames reversed and pushed
    /// through the shuttle's stretch, the playhead at the range's start with that frame shown. A jump (the last reverse render
    /// did not begin at `ph`) flushes the ring and resets the stretch and the meters, as `play_tick` does. False with no
    /// device or a paused one (the silent nudge path). An error is `last_error` and resets the meters.
    fn reverse_tick(&mut self, ph: i64) -> bool {
        if self.rate >= 0 || !self.play.live || self.play.out.is_none() {
            return false;
        }
        self.play.pending = None;
        let lo = (ph + self.rate).max(0);
        if lo >= ph {
            self.playing = false;
            self.sync_audio();
            return true;
        }
        if self.play.rev_next != Some(ph) {
            self.meters.reset();
            if let Some(w) = self.play.stretch.as_mut() {
                w.reset();
            }
            if let Some(o) = self.play.out.as_mut() {
                o.flush();
            }
        }
        let mut sink = PlaySink { meters: &mut self.meters, out: None, still: None, pushed: 0, stretch: None, rev: Some(Vec::new()) };
        let r = render(&self.state.timeline, lo..ph, &self.sources, &mut sink);
        let (still, back) = (sink.still, sink.rev.unwrap_or_default());
        self.play.renders += 1;
        if let Some(o) = self.play.out.as_mut() {
            let took = o.push(&shuttle_pcm(self.play.stretch.as_mut(), reversed_frames(&back)));
            self.play.pushed += (took / 2) as u64;
        }
        match r {
            Ok(_) => self.play.pending = Some((lo, still)),
            Err(e) => {
                self.last_error = Some(format!("meters: {e}"));
                self.meters.reset();
            }
        }
        self.state.seek(lo);
        (self.play.shown, self.play.next, self.play.rev_next) = (None, None, Some(lo));
        true
    }
    /// STRIAJKLMORE (SR945): the shuttle rate as the message line reads it, `shuttle <rate>x` (`shuttle 8x`, `shuttle -2x`),
    /// while playing at any rate but 1; `None` at play speed or stopped.
    pub fn shuttle_line(&self) -> Option<String> {
        (self.playing && self.rate != 1).then(|| format!("shuttle {}x", self.rate))
    }
}

#[cfg(test)]
mod striajklmore_tests {
    use super::*;
    use crate::media::AudioOut;
    use crate::nle::SYNTH_RATE;
    use std::sync::{Arc, Mutex};

    /// The device's calls: every push's frames and samples, the pause states, the flushes.
    #[derive(Default)]
    struct Dev {
        pushes: Vec<usize>,
        samples: Vec<f32>,
        paused: Vec<bool>,
        flushes: usize,
    }
    struct Out(Arc<Mutex<Dev>>);
    impl AudioOut for Out {
        fn push(&mut self, stereo: &[f32]) -> usize {
            let mut d = self.0.lock().unwrap();
            d.pushes.push(stereo.len() / 2);
            d.samples.extend_from_slice(stereo);
            stereo.len()
        }
        fn consumed(&self) -> u64 {
            self.0.lock().unwrap().pushes.iter().sum::<usize>() as u64
        }
        fn flush(&mut self) {
            self.0.lock().unwrap().flushes += 1;
        }
        fn set_paused(&mut self, p: bool) {
            self.0.lock().unwrap().paused.push(p);
        }
    }

    fn face() -> (Face, Arc<Mutex<Dev>>) {
        let (tl, src) = synth_fixture();
        let dev = Arc::new(Mutex::new(Dev::default()));
        (Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_audio(Box::new(Out(dev.clone())), SYNTH_RATE), dev)
    }

    fn mute() -> Face {
        let (tl, src) = synth_fixture();
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
    }

    fn key(f: &mut Face, k: &str) {
        assert!(f.on(&SMessage::BrowserText(k.into())));
    }

    fn pushed(dev: &Arc<Mutex<Dev>>) -> usize {
        dev.lock().unwrap().pushes.iter().sum()
    }

    const PER: usize = (SYNTH_RATE / 25) as usize;
    const WSOLA_N: usize = (SYNTH_RATE as usize / 50) & !1;

    /// STRIAJKLMORE M3: L x4 is 8x and released, the 4x -> 8x step neither pauses nor flushes, two stages run; four 8x ticks
    /// (1..33 of the 35-tick fixture) keep the device in real time (4 x 1920 frames, within four WSOLA frames under and two
    /// over); K drops the stretch.
    #[test]
    fn striajklmore_eight_heard() {
        let (mut f, dev) = face();
        key(&mut f, "l");
        assert!(f.tick(), "a 1x tick first: the run is under way");
        key(&mut f, "l");
        key(&mut f, "l");
        assert_eq!(f.rate, 4);
        let (paused, flushes) = { let d = dev.lock().unwrap(); (d.paused.clone(), d.flushes) };
        key(&mut f, "l");
        assert_eq!((f.rate, f.played().live), (8, true));
        assert!(f.play.stretch.as_ref().is_some_and(|s| s.b.is_some()), "8x runs two stages");
        let base = pushed(&dev);
        let n = 4;
        for _ in 0..n {
            assert!(f.tick());
        }
        let d = dev.lock().unwrap();
        assert_eq!((d.paused.clone(), d.flushes), (paused, flushes), "8x: no pause, no flush");
        let got = d.pushes.iter().sum::<usize>() - base;
        assert!(got + 4 * WSOLA_N >= n * PER && got <= n * PER + 2 * WSOLA_N, "8x: {got} frames for {n} ticks");
        drop(d);
        key(&mut f, "k");
        assert!(f.play.stretch.is_none());
        assert_eq!(f.shuttle_line(), None);
    }

    /// STRIAJKLMORE M3: J at 20 stays released; one -1x tick is ONE push of 1920 frames, exactly a forward face's push for
    /// 19..20 with its frames reversed, and shows tick 19's frame at playhead 19; -4x keeps the device in real time; the run
    /// to 0 stops, pauses and flushes; L after J flushes once (the direction change).
    #[test]
    fn striajklmore_reverse_heard() {
        let (mut fwd, fdev) = face();
        fwd.state.seek(19);
        key(&mut fwd, "l");
        assert!(fwd.tick());
        let ahead = fdev.lock().unwrap().samples.clone();
        assert_eq!(ahead.len(), 2 * PER);
        let ahead_still = fwd.play.pending.clone().expect("the forward still");

        let (mut f, dev) = face();
        f.state.seek(20);
        key(&mut f, "j");
        assert_eq!((f.playing, f.rate, f.played().live), (true, -1, true));
        assert_eq!(dev.lock().unwrap().paused.last(), Some(&false), "J releases the device");
        assert!(f.tick());
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        assert_eq!(f.state.playhead, 19);
        {
            let d = dev.lock().unwrap();
            assert_eq!(d.pushes, vec![PER], "one push of one tick");
            assert_eq!(d.samples, reversed_frames(&ahead), "the forward audio, frames reversed");
        }
        let still = f.play.pending.clone().expect("the reverse still");
        assert_eq!(still.0, 19);
        assert_eq!(still.1.map(|s| s.rgba), ahead_still.1.map(|s| s.rgba), "tick 19's frame");

        let (mut f, dev) = face();
        let end = f.state.timeline.end();
        f.state.seek(end);
        for _ in 0..3 {
            key(&mut f, "j");
        }
        assert_eq!(f.rate, -4);
        let n = 4;
        for _ in 0..n {
            assert!(f.tick());
        }
        assert_eq!(f.state.playhead, end - 4 * n as i64);
        let got = pushed(&dev);
        assert!(got + 2 * WSOLA_N >= n * PER && got <= n * PER + WSOLA_N, "-4x: {got} frames for {n} ticks");
        let flushes = dev.lock().unwrap().flushes;
        while f.tick() {}
        assert_eq!((f.playing, f.state.playhead), (false, 0), "the run to 0 stops");
        let d = dev.lock().unwrap();
        assert_eq!((d.paused.last().copied(), d.flushes), (Some(true), flushes + 1), "stopped: paused and flushed");
        drop(d);

        let (mut f, dev) = face();
        f.state.seek(20);
        key(&mut f, "j");
        assert!(f.tick() && f.tick());
        let flushes = dev.lock().unwrap().flushes;
        key(&mut f, "l");
        assert!(f.tick());
        assert_eq!(dev.lock().unwrap().flushes, flushes + 1, "the direction change flushes once");
        assert!(f.tick());
        assert_eq!(dev.lock().unwrap().flushes, flushes + 1, "then the forward run is continuous");
    }

    /// STRIAJKLMORE M3: with no device a reverse tick is the old path: the playhead nudges back, the meters stay empty, no
    /// render, no stretch.
    #[test]
    fn striajklmore_reverse_without_device() {
        let mut f = mute();
        f.state.seek(5);
        key(&mut f, "j");
        key(&mut f, "j");
        assert_eq!(f.rate, -2);
        assert!(f.tick());
        assert_eq!(f.state.playhead, 3);
        assert!(f.meters().levels().is_empty());
        assert_eq!(f.played().renders, 0);
        assert!(f.play.stretch.is_none());
    }

    /// STRIAJKLMORE M3: the message line: nothing stopped or at 1x; `shuttle 2x`, `shuttle 8x`, `shuttle -1x` while
    /// shuttling; an error wins; K clears it.
    #[test]
    fn striajklmore_readout() {
        let mut f = mute();
        assert_eq!(f.message_text(), None);
        key(&mut f, "l");
        assert_eq!(f.message_text(), None, "1x: no readout");
        key(&mut f, "l");
        assert_eq!(f.message_text().as_deref(), Some("shuttle 2x"));
        key(&mut f, "l");
        key(&mut f, "l");
        assert_eq!(f.message_text().as_deref(), Some("shuttle 8x"));
        key(&mut f, "j");
        assert_eq!(f.message_text().as_deref(), Some("shuttle -1x"));
        f.last_error = Some("split: no such clip".into());
        assert_eq!(f.message_text().as_deref(), Some("split: no such clip"), "an error wins");
        f.last_error = None;
        key(&mut f, "k");
        assert_eq!(f.message_text(), None);
    }
}

// ---- STRIAAUDIOSCRUBMORE (LEDGER SR947): clicks sound, bursts wait for the device, the strip shows a scrub (named tail block) ----

impl Face {
    /// STRIAAUDIOSCRUBMORE (SR947): a click on a clip, a track or the ruler is a fresh scrub step (no previous tick): SR865's
    /// `play.shown` test alone keeps a ruler click after its own press silent (one render, as before).
    fn click_burst(&mut self) {
        self.scrub_burst(i64::MIN)
    }
    /// STRIAAUDIOSCRUBMORE (SR947): SR865's one `play_tick` of a scrub step, with the gate recorded: what the device had
    /// consumed before the burst and how many frames the burst pushed.
    fn scrub_tick(&mut self, ph: i64) {
        let (c0, p0) = (self.play.out.as_ref().map_or(0, |o| o.consumed()), self.play.pushed);
        self.play_tick(ph);
        self.play.scrub = Some((c0, self.play.pushed - p0));
    }
    /// STRIAAUDIOSCRUBMORE (SR947): the next scrub burst may flush the ring once the last burst's snippet is at least three
    /// quarters played by the device's own `consumed` counter (the quarter slack covers a resampler holding back a source
    /// frame, so a drained ring never stalls the gate). No record or no device: due.
    fn scrub_due(&self) -> bool {
        match (self.play.scrub, self.play.out.as_ref()) {
            (Some((c0, n)), Some(o)) => o.consumed() + n / 4 >= c0 + n,
            _ => true,
        }
    }
    /// STRIAAUDIOSCRUBMORE (SR947): the meter strip shows the last sounded scrub step while the Scrub drag lives; a release,
    /// a click or a face without a device paints none (never a frozen reading).
    fn scrub_metered(&self) -> bool {
        scrubbing(&self.drag) && self.play.scrub.is_some()
    }
}

#[cfg(test)]
mod audioscrubmore_tests {
    use super::playaudio_tests::{Ev, face, pushes, still_at};
    use super::*;

    fn btn(down: bool, x: f64, y: f64) -> SMessage {
        SMessage::BrowserButton { down, x, y, mods: 0, surface: 0 }
    }

    /// STRIAAUDIOSCRUBMORE M3: a click on a clip seeks, selects and sounds its frame (one render, one push); the same click
    /// again plays nothing (the tick already shown); a click on an empty track spot sounds too; no device, no render.
    #[test]
    fn striaaudioscrubmore_click_sounds() {
        let (mut f, log) = face();
        log.lock().unwrap().clear();
        let (x, y) = f.layout().point(1, 12);
        assert!(f.on(&SMessage::BrowserClick(x, y, 0)));
        assert_eq!((f.state.playhead, f.state.selection), (12, Some((1, 1))));
        assert_eq!((f.played().renders, pushes(&log).len()), (1, 1));
        assert!(f.on(&SMessage::BrowserClick(x, y, 0)));
        assert_eq!((f.played().renders, pushes(&log).len()), (1, 1), "the tick already shown");
        let (x, y) = f.layout().point(0, 22);
        assert!(f.on(&SMessage::BrowserClick(x, y, 0)));
        assert_eq!((f.state.playhead, f.state.selection, f.played().renders), (22, None, 2));
        assert!(!f.playing && !f.played().live);

        let (tl, src) = synth_fixture();
        let mut mute = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        let (x, y) = mute.layout().point(1, 12);
        mute.on(&SMessage::BrowserClick(x, y, 0));
        assert_eq!((mute.state.playhead, mute.played().renders), (12, 0));
    }

    /// STRIAAUDIOSCRUBMORE M3: a ruler press and its unmoved release sound the tick once: the release's click is refused by
    /// the tick the press already showed.
    #[test]
    fn striaaudioscrubmore_ruler_click_once() {
        let (mut f, log) = face();
        log.lock().unwrap().clear();
        f.on(&btn(true, 60.0, 70.0));
        f.on(&btn(false, 60.0, 70.0));
        assert_eq!((f.state.playhead, f.played().renders, pushes(&log).len()), (7, 1, 1));
    }

    /// STRIAAUDIOSCRUBMORE M3: a scrub step waits until the device has played three quarters of the last burst: a refused
    /// step still seeks and its paint renders `preview`; once `consumed` reaches the threshold the next step sounds; a
    /// transport change drops the gate.
    #[test]
    fn striaaudioscrubmore_gate_waits() {
        let (mut f, log) = face();
        log.lock().unwrap().clear();
        assert!(f.on(&btn(true, 44.0, 70.0)));
        assert_eq!((f.state.playhead, f.played().renders), (5, 1));
        assert_eq!(f.play.scrub, Some((0, 1920)));
        log.lock().unwrap().clear();
        assert!(!f.scrub_due());
        f.on(&SMessage::BrowserMouseMove(124.0, 70.0, 0));
        assert_eq!((f.state.playhead, f.played().renders), (15, 1), "the last snippet is not played yet");
        let _ = f.frames();
        assert_eq!(f.still().cloned(), still_at(15));
        log.lock().unwrap().push(Ev::Push(vec![0.0; 2 * 1439]));
        assert!(!f.scrub_due(), "1439 of 1920 played");
        log.lock().unwrap().clear();
        log.lock().unwrap().push(Ev::Push(vec![0.0; 2 * 1440]));
        assert!(f.scrub_due(), "1440 of 1920 played");
        f.on(&SMessage::BrowserMouseMove(140.0, 70.0, 0));
        assert_eq!((f.state.playhead, f.played().renders), (17, 2));
        f.on(&btn(false, 140.0, 70.0));
        f.verb("play");
        f.verb("play");
        assert!(f.play.scrub.is_none(), "a transport change ends the gate");
    }

    /// STRIAAUDIOSCRUBMORE M3: the meter strip paints the scrub's reading while the Scrub drag lives and nothing after the
    /// release; a face without a device paints none.
    #[test]
    fn striaaudioscrubmore_meters_strip() {
        let (mut f, _) = face();
        assert!(f.meter_levels().is_empty());
        f.on(&btn(true, 44.0, 70.0));
        let lv = f.meter_levels();
        assert!(!lv.is_empty());
        assert_eq!(lv, f.meters().levels());
        f.on(&btn(false, 44.0, 70.0));
        assert!(f.meter_levels().is_empty(), "released: no strip");

        let (tl, src) = synth_fixture();
        let mut mute = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        mute.on(&btn(true, 44.0, 70.0));
        assert!(mute.meter_levels().is_empty());
    }
}

// ---- STRIAEXPORTPRESETSMORE (LEDGER SR948): the face's picked export preset (named tail block) ----
impl Face {
    /// The `preset` word (the transport's `Preset` button): bare, it steps the picked preset through THE table
    /// ([`crate::nle_export::PRESETS`], in order) and back to none; `preset <name>` / `preset none` set it. The success line
    /// is the face's note (`preset web: h264 + aac crf=23 aq=10 scenecut=40`, `preset none: export's own words`, read by
    /// [`Face::message_text`] after `last_error`); an unknown name is refused on `last_error` by name, the pick unchanged.
    /// A view-only word: no undo step, the document untouched.
    fn preset_verb(&mut self, v: &str) -> Option<bool> {
        let mut words = v.split_whitespace();
        if words.next() != Some("preset") {
            return None;
        }
        let presets: &'static [crate::nle_export::ExportPreset] = &crate::nle_export::PRESETS;
        let next = match (words.next(), words.next()) {
            (None, _) => match self.export_preset.and_then(|p| presets.iter().position(|q| q == p)) {
                None => presets.first(),
                Some(i) => presets.get(i + 1),
            },
            (Some("none"), None) => None,
            (Some(w), None) => match crate::nle_export::preset_named(w) {
                Some(p) => Some(p),
                None => {
                    self.last_error = Some(format!("preset: unknown preset {w} ({})", crate::nle_export::preset_words()));
                    return Some(true);
                }
            },
            (Some(_), Some(_)) => {
                self.last_error = Some(format!("preset: usage: preset [none|{}]", crate::nle_export::preset_words()));
                return Some(true);
            }
        };
        self.export_preset = next;
        self.preset_note = Some(match next {
            Some(p) => format!("preset {}", p.words()),
            None => "preset none: export's own words".to_string(),
        });
        Some(true)
    }
    /// The `export` verb's words with the picked preset: `--preset <name>` appended ONLY when the words hold no `--preset`
    /// and at most one word that is not a [`VIDEO_NAMES`] word (`export`, `export <dir>`); any explicit format, audio or
    /// `--preset` word wins and the words come back untouched. SR871's [`export_preset_words`] then reads them.
    fn picked_preset_words<'a>(&self, mut args: Vec<&'a str>) -> Vec<&'a str> {
        let Some(p) = self.export_preset else { return args };
        if args.contains(&"--preset") || args.len() > 1 || args.first().is_some_and(|w| VIDEO_NAMES.contains(w)) {
            return args;
        }
        args.push("--preset");
        args.push(p.name);
        args
    }
    /// The picked export preset a bare `export` takes (None: the export's own words).
    pub fn export_preset(&self) -> Option<&'static crate::nle_export::ExportPreset> {
        self.export_preset
    }
}

// ---- STRIAGRADEMORE (LEDGER SR957): the grade's keys and a summary-less reel's note (named tail block) ----
// NLE-GRADE-KEYS: KEYS binds `1`..`6` (text keys) to SR899's six grade step words; `Face::key` routes a VERBS action through
// `Face::verb`, so the keys and the buttons are ONE road, and HELP carries their phrases. NLE-GRADE-NOTE-NOSUMMARY:
// [`Face::reel_line`] reads its line through [`reel_words`], so a graded clip on a reel with nothing to say still shows its grade.

/// STRIAGRADEMORE (SR957): the reel line from its reel `name`, the reel's `summary` and the grade `note` ([`grade_note`]):
/// a summary gives `<name>: <summary><note>` (SR899's line); no summary and a grade gives `<name>: bright <b> contrast <c>
/// sat <s>`; no summary and no grade gives `None`.
pub fn reel_words(name: String, summary: Option<String>, note: String) -> Option<String> {
    match summary {
        Some(s) => Some(format!("{name}: {s}{note}")),
        None if !note.is_empty() => Some(format!("{name}: {}", note.trim_start())),
        None => None,
    }
}

// ---- STRIALOUDNESSMORE (LEDGER SR961): the running measure on the export band (named tail block) ----

impl Face {
    /// STRIALOUDNESSMORE (SR961): the running loudness measure's done fraction (SR903's ticks done over the range's
    /// ticks, clamped to 1; 0.0 for an empty range), `None` when no measure runs. `export_frac` falls back to it, so the
    /// export's band (SR694's `paint_export`) shows the measure; a running export's reading comes first.
    pub fn loudness_frac(&self) -> Option<f64> {
        let j = self.loudness.as_ref()?;
        let done = j.watch.done.load(Relaxed);
        Some(if j.total == 0 { 0.0 } else { (done.min(j.total) as f64 / j.total as f64).min(1.0) })
    }
}

// ---- STRIAMETERSMORE (LEDGER SR962): the held peaks for the witness and the split preview's master latch click (named tail block) ----

impl Face {
    /// The meters' standing peak holds `(key, held dBFS)` (`None` the master), in hold order.
    pub fn meter_held(&self) -> Vec<(Option<usize>, f64)> {
        self.meter_hold.held()
    }
    /// A split face's preview Surface carries the master meter and its clip cap but takes no pointer message
    /// ([`Face::takes`]); a press there on the latched master's bar ([`meter_rect`] on [`Face::layout_for`]`(Role::Preview)`)
    /// is taken and begins nothing (`Some(false)`), a release or click there clears the master's latch
    /// (`Some(unclipped)`). `None` for anything else: unsplit, another Surface, no master latch, or outside its bar.
    fn preview_meter_input(&mut self, msg: &SMessage) -> Option<bool> {
        let s = self.split?;
        if msg.surface() != Some(s.preview) || !self.meter_hold.clipped().contains(&None) {
            return None;
        }
        let (down, x, y) = match *msg {
            SMessage::BrowserButton { down, x, y, .. } => (Some(down), x, y),
            SMessage::BrowserClick(x, y, _) => (None, x, y),
            _ => return None,
        };
        let (x0, y0, x1, y1) = meter_rect(self.layout_for(Role::Preview), None)?;
        if !(x >= x0 as f64 && x < x1 as f64 && y >= y0 as f64 && y < y1 as f64) {
            return None;
        }
        match down {
            Some(true) => Some(false),
            _ => Some(self.meter_hold.unclip(None)),
        }
    }
}

#[cfg(test)]
mod striametersmore_tests {
    use super::*;

    fn clipping_face() -> Face {
        let (mut tl, src) = synth_fixture();
        tl.tracks[1].clips.iter_mut().for_each(|c| *c = c.gained(2400));
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
    }

    fn latch(face: &mut Face) {
        face.verb("play");
        face.tick();
        face.tick();
        face.verb("play");
        assert!(!face.playing);
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
    }

    /// M3: no hold, nothing held; two played ticks at +24 dB hold track 1 and the master (each at least its reading);
    /// `2 * fps` idle ticks age every hold out.
    #[test]
    fn striametersmore_held_lists_holds_in_order() {
        let mut face = clipping_face();
        assert!(face.meter_held().is_empty());
        latch(&mut face);
        let held = face.meter_held();
        let keys: Vec<Option<usize>> = held.iter().map(|h| h.0).collect();
        assert!(keys.contains(&Some(1)) && keys.contains(&None), "{held:?}");
        assert!(held.iter().all(|h| h.1 >= METER_VIEW_DB), "{held:?}");
        for _ in 0..METER_HOLD_S * 25 {
            face.tick();
        }
        assert!(face.meter_held().is_empty());
    }

    /// M3: Ctrl+Shift+M and Meta+Shift+M (BrowserKey) clear every latch and hold through `meters-reset`: no undo step,
    /// the document not dirtied; the help names both chords on the meters-reset row.
    #[test]
    fn striametersmore_reset_key_clears() {
        for k in ["Ctrl+Shift+M", "Meta+Shift+M"] {
            let mut face = clipping_face();
            latch(&mut face);
            assert_eq!(face.meter_clipped(), &[Some(1), None]);
            let (undo, dirty) = (face.state.undo_depth(), face.dirty());
            assert!(face.on(&SMessage::BrowserKey(k.into())), "{k}");
            assert!(face.meter_clipped().is_empty() && !face.meter_hold_pending(), "{k}");
            assert_eq!((face.state.undo_depth(), face.dirty()), (undo, dirty));
            assert!(face.last_error.is_none(), "{:?}", face.last_error);
        }
        let rows = help_rows();
        let row = rows.iter().find(|r| r.1 == help_phrase("meters-reset")).expect("a meters-reset help row");
        assert!(row.0.split(" / ").any(|k| k == "Ctrl+Shift+M") && row.0.split(" / ").any(|k| k == "Meta+Shift+M"), "{:?}", row.0);
    }

    /// M3: split 0 / 1, the master latched: a press on the preview Surface's master bar is taken and begins nothing, its
    /// release clears the master's latch only (track 1 stays), the timeline, playhead and selection unmoved; with no
    /// master latch the same press is the preview Surface's untaken pointer message it always was.
    #[test]
    fn striametersmore_split_master_latch_clicks_off() {
        let mut face = clipping_face().on_surfaces(0, 1);
        latch(&mut face);
        assert_eq!(face.meter_clipped(), &[Some(1), None]);
        let (tl, ph, sel, undo) = (face.state.timeline.clone(), face.state.playhead, face.state.selection, face.state.undo_depth());
        let (x, y) = (316.0, 10.0);
        assert!(!face.on(&SMessage::BrowserButton { down: true, x, y, mods: 0, surface: 0 }));
        assert!(face.dragging().is_none());
        assert!(face.on(&SMessage::BrowserButton { down: false, x, y, mods: 0, surface: 0 }));
        assert_eq!(face.meter_clipped(), &[Some(1)]);
        assert!(!face.on(&SMessage::BrowserClick(x, y, 0)), "no master latch: the preview Surface takes no click");
        assert_eq!((&face.state.timeline, face.state.playhead, face.state.selection, face.state.undo_depth()), (&tl, ph, sel, undo));
        assert!(!face.on(&SMessage::BrowserButton { down: true, x, y, mods: 0, surface: 0 }));
        assert!(face.dragging().is_none());
    }
}

// ---- STRIATHUMBNAILSMORE (LEDGER SR949): a wide video clip's filmstrip and tail stills (named tail block) ----
// STRIAKTHUMBS (SR9019): SR949's `Thumbs::fill_strip` rule (a wide body's filmstrip and tail tiles after every head, a body
// starting left of the canvas asking its head first) is timeline_core view.rs `thumb_want`'s second pass; `Thumbs::fill`
// loops over it. Owed: NLE-THUMBNAILS-STRIP-CACHE, NLE-THUMBNAILS-RATE (STRIACLIPSPEED SR829's `src_at` in `head_tiles`).

// ---- STRIAROLLMORE (LEDGER SR954): the roll tool — a face mode that turns the trim-handle drag into a roll (named tail block) ----
// Every gesture on the selected clip's edge zone is taken (none = trim, Alt = no snap, Ctrl / Cmd = slide, Shift = ripple,
// Shift + Ctrl / Cmd = slip), so the roll drag is a TOOL: `roll-tool` (a VERBS word, the `r` key, stria-view's Roll tool
// button) toggles it; while on, `press` hands the begun drag to timeline_core's `Drag::rolled`, so a trim-handle drag whose
// cut has a butted neighbour commits ONE `EditorState::roll_selected` (SR851's op) on release. The word is view-only: no
// undo step, the document stays clean; a live drag is dropped uncommitted. While on, the message line names the tool after
// the export line (default off: every golden is unchanged). Owed: NLE-ROLL-TOOL-KERNEL, NLE-ROLL-TOOL-PAINT.

/// STRIAROLLMORE (SR954): the message line while the roll tool is on.
pub const ROLL_TOOL_NOTE: &str = "roll tool: drag a clip edge to roll its cut";

impl Face {
    /// The roll tool is on.
    pub fn roll_tool(&self) -> bool {
        self.roll_tool
    }
    /// The `roll-tool` word: the tool on / off (a live drag dropped uncommitted); `None` for any other word.
    fn roll_tool_verb(&mut self, v: &str) -> Option<bool> {
        if v != "roll-tool" {
            return None;
        }
        self.roll_tool = !self.roll_tool;
        self.drag = None;
        Some(true)
    }
    /// The message line's note while the tool is on ([`ROLL_TOOL_NOTE`]).
    fn roll_tool_note(&self) -> Option<String> {
        self.roll_tool.then(|| ROLL_TOOL_NOTE.to_string())
    }
}

// ---- STRIATCMORE (LEDGER SR964): the readout reads the time base (named tail block) ----
// `Timeline::fps` is `Some` only for a `1/n` base, so a 30000/1001 timeline read at the fallback 25 fps. An NTSC base
// `1001/(k*1000)` reads at its real rate: timecode drop-frame for k 30 / 60 (`edl::timecode_drop`), non-drop at k otherwise
// (23.976 at 24); frames the tick count; seconds `t * 1001 / den`. Every other base is `tc_text` as before (byte-identical).
// Owed: NLE-TC-DROPFRAME-MODEL (no road builds a 1001 timeline; the EDL refuses drop-frame), NLE-TC-DROPFRAME-RULER.

// STRIAKTC (SR9018): `tc_text_tb` lives in timeline_core::view now (re-exported at the SR872 block).

#[cfg(test)]
mod striatcmore_tests {
    use super::*;
    use timeline_core::Timebase;

    /// NTSC bases read drop-frame (30, 60) or non-drop (24) at the real rate; a 1/n base and any other base are `tc_text`.
    #[test]
    fn striatcmore_tc_text_tb_units() {
        let ntsc = Timebase { num: 1001, den: 30000 };
        assert_eq!(tc_text_tb(1800, ntsc, 25, TcUnit::Timecode), "00:01:00;02");
        assert_eq!(tc_text_tb(1800, ntsc, 25, TcUnit::Frames), "1800");
        assert_eq!(tc_text_tb(1800, ntsc, 25, TcUnit::Seconds), "60.06 s");
        assert_eq!(tc_text_tb(3600, Timebase { num: 1001, den: 60000 }, 25, TcUnit::Timecode), "00:01:00;04");
        assert_eq!(tc_text_tb(48, Timebase { num: 1001, den: 24000 }, 25, TcUnit::Timecode), "00:00:02:00");
        assert_eq!(tc_text_tb(-3, ntsc, 25, TcUnit::Timecode), "00:00:00;00");
        for u in [TcUnit::Timecode, TcUnit::Frames, TcUnit::Seconds] {
            assert_eq!(tc_text_tb(312, Timebase { num: 1, den: 25 }, 25, u), tc_text(312, 25, u));
            assert_eq!(tc_text_tb(312, Timebase { num: 1, den: 1000 }, 25, u), tc_text(312, 25, u), "any other base: the fallback");
        }
    }
}

// ---- STRIASLIPVIEWMORE (LEDGER SR946): the slide drag's cuts, the word slips' hold, the witness size (named tail block) ----
// Three new SOURCES of SR858's one pair (`Face::slip_refresh`: its render through `preview`, its cache, its throttle and the
// serve clock unchanged). A Ctrl / Cmd Slide drag (`DragKind::Slide`, `Drag::shown` valid, a video track) shows the two
// frames a slide moves: the butted previous clip's new last frame (left) and the butted next clip's new first frame (right);
// a side with no butted neighbour stays FIELD, neither: no pair. A word slip / slide (`slip-` .. `slide+10`, the keys and
// buttons, through `Face::verb`'s one arm) holds its pair of the selected clip until the next action: the hold is a
// snapshot condition (`slipmore_hold_check`), dropped with its pair when a drag is live, the face plays, or the timeline,
// playhead or selection differ from the snapshot; zoom, hover, resize, the help and a refused word leave it. Only `slip` /
// `slide` words hold (a sibling's `roll` words through the same arm hold nothing). The witness reads the pair's size
// (stria-view `slip_json`). Owed: NLE3E-SLIP-VIEW-ASYNC, NLE3E-SLIP-VIEW-KERNEL, NLE3E-SLIP-VIEW-TIMECODE, NLE3E-SLIDE-VIEW-FOUR.

/// STRIASLIPVIEWMORE (SR946): the state a word slip / slide left, its pair shown while the face still stands there.
struct SlipHold {
    slide: bool,
    timeline: Timeline,
    playhead: i64,
    selection: Option<(usize, usize)>,
}

impl Face {
    /// STRIASLIPVIEWMORE (SR946): a slip / slide word through THE dispatch (`slipslide_word` onto `EditorState::verb`, one
    /// commit); on Ok a `slip` / `slide` word holds its pair (a refused word leaves the state and any hold as they were).
    fn slip_word(&mut self, v: &str) -> Result<(), EditError> {
        let r = slipslide_word(v).and_then(|w| self.state.verb(&w)).unwrap_or(Err(EditError::NoClip));
        let slide = v.starts_with("slide");
        if r.is_ok() && (slide || v.starts_with("slip")) {
            self.slip_hold = Some(SlipHold { slide, timeline: self.state.timeline.clone(), playhead: self.state.playhead, selection: self.state.selection });
        }
        r
    }
    /// STRIASLIPVIEWMORE (SR946): the next action ends a hold: a live drag, playing, or a timeline / playhead / selection
    /// other than the hold's drops it and its pair.
    fn slipmore_hold_check(&mut self) {
        let st = &self.state;
        let stale = self.slip_hold.as_ref().is_some_and(|h| self.drag.is_some() || self.playing || h.playhead != st.playhead || h.selection != st.selection || h.timeline != st.timeline);
        if stale {
            self.slip_hold = None;
            self.slip_view = None;
        }
    }
    /// STRIASLIPVIEWMORE (SR946): a word slip / slide's pair is held (until the next action).
    pub fn slip_held(&self) -> bool {
        self.slip_hold.is_some()
    }
}

/// STRIASLIPVIEWMORE (SR946): the pair's clip besides SR858's Slip drag: a valid Slide drag's shown clip, or with no drag a
/// held word's selected clip; a video track only. The key is SR858's `(track, idx, clip)`.
fn slipmore_target<'a>(drag: Option<Drag>, shown: Option<&'a EditorState>, hold: Option<&SlipHold>, st: &'a EditorState) -> Option<((usize, usize, Clip), &'a Timeline)> {
    let (track, idx, tl) = match (drag, shown, hold) {
        (Some(d), Some(s), _) if d.kind == DragKind::Slide => (d.track, d.idx, &s.timeline),
        (None, _, Some(_)) => {
            let (t, i) = st.selection?;
            (t, i, &st.timeline)
        }
        _ => return None,
    };
    let c = *tl.tracks.get(track).filter(|t| t.kind == Kind::Video)?.clips.get(idx)?;
    Some(((track, idx, c), tl))
}

/// STRIASLIPVIEWMORE (SR946): the pair shows a slide's cuts (a Slide drag, or a held `slide` word), else a slip's edges.
fn slipmore_slide(drag: Option<Drag>, hold: Option<&SlipHold>) -> bool {
    drag.map_or(hold.is_some_and(|h| h.slide), |d| d.kind == DragKind::Slide)
}

/// STRIASLIPVIEWMORE (SR946): the pair's two cells, each a one-clip timeline ([`slip_clip_timeline`]) and the tick to show.
/// A slip: clip `idx`'s first (`at`) and last (`end - 1`) frame. A slide: the butted previous clip's last frame and the
/// butted next clip's first; a side with no butted neighbour is `None`; neither: `None` (no pair).
fn slip_cells(tl: &Timeline, track: usize, idx: usize, slide: bool) -> Option<[Option<(Timeline, i64)>; 2]> {
    if !slide {
        let (one, c) = slip_clip_timeline(tl, track, idx)?;
        return Some([Some((one.clone(), c.at)), Some((one, c.end() - 1))]);
    }
    let tr = tl.tracks.get(track)?;
    let c = *tr.clips.get(idx)?;
    let prev = idx.checked_sub(1).filter(|&i| tr.clips[i].end() == c.at).and_then(|i| slip_clip_timeline(tl, track, i)).map(|(one, p)| (one, p.end() - 1));
    let next = tr.clips.get(idx + 1).filter(|n| n.at == c.end()).and_then(|_| slip_clip_timeline(tl, track, idx + 1)).map(|(one, n)| (one, n.at));
    (prev.is_some() || next.is_some()).then_some([prev, next])
}

#[cfg(test)]
mod striaslipviewmore_tests {
    use super::*;

    /// The cell's (clip, tick): the one clip its timeline holds and the tick it shows.
    fn cell(c: &Option<(Timeline, i64)>) -> Option<(Clip, i64)> {
        c.as_ref().map(|(one, t)| {
            assert_eq!((one.tracks.len(), one.tracks[0].clips.len()), (1, 1), "a one-clip timeline");
            (one.tracks[0].clips[0], *t)
        })
    }

    /// Rule: a slip's cells are the clip's own first and last frame; a slide's are the butted neighbours' moving edges (a
    /// side with no butted neighbour None, neither None).
    #[test]
    fn striaslipviewmore_slip_cells_kats() {
        let (mut tl, _src) = synth_fixture();
        let (a, b) = (tl.tracks[0].clips[0], tl.tracks[0].clips[1]);
        let s = slip_cells(&tl, 0, 1, false).expect("slip cells");
        assert_eq!((cell(&s[0]), cell(&s[1])), (Some((b, 10)), Some((b, 19))));
        let s = slip_cells(&tl, 0, 1, true).expect("A butts B");
        assert_eq!((cell(&s[0]), cell(&s[1])), (Some((a, 9)), None), "A@25 is not butted to B's end 20");
        assert!(slip_cells(&tl, 0, 2, true).is_none(), "A@25 butts nothing");
        tl.tracks[0].clips[2].at = 20;
        let n = tl.tracks[0].clips[2];
        let s = slip_cells(&tl, 0, 1, true).expect("both butted");
        assert_eq!((cell(&s[0]), cell(&s[1])), (Some((a, 9)), Some((n, 20))));
        assert!(slip_cells(&tl, 0, 9, false).is_none() && slip_cells(&tl, 0, 9, true).is_none(), "no clip");
    }
}

// ---- STRIAAUTOSAVEMORE (LEDGER SR972): the kept autosave offered at open (named tail block) ----
// At open (`with_doc`), after SR883's offer, SR913's kept `<stem>.autosave.1.edl` strictly newer than the document (the
// rename keeps the original autosave's mtime: an older session's autosave declined and the document never saved after
// it) is offered on the same message row, naming SR933's word `recover 1`. Nothing is read or applied here. A standing
// message that is not SR883's offer is never replaced. `autosave_newer` is the one mtime rule (SR883's offer reads it).
// Owed: NLE-HOST-AUTOSAVE-KERNEL (the kernel Player's face).

/// STRIAAUTOSAVEMORE (SR972): the autosave file of `generation` beside `doc` ([`autosave_generation_path`]) is strictly
/// newer than `doc`; false when either is missing or the generation is refused.
pub fn autosave_newer(doc: &std::path::Path, generation: usize) -> bool {
    let Ok(path) = autosave_generation_path(doc, generation) else { return false };
    let modified = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified());
    match (modified(&path), modified(doc)) {
        (Ok(a), Ok(d)) => a > d,
        _ => false,
    }
}

impl Face {
    /// STRIAAUTOSAVEMORE (SR972): offer the kept autosave newer than the document: its own line, or appended to SR883's
    /// offer (`...; <kept> too: the word recover 1 loads it`); any other standing message is left alone.
    fn offer_kept_autosave(&mut self) {
        let Some(doc) = self.doc_path.clone() else { return };
        if !autosave_newer(&doc, 1) {
            return;
        }
        let name = |p: &std::path::Path| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
        let keep = autosave_keep_path(&doc);
        let line = match self.last_error.take() {
            None => format!("autosave: {} is newer than {}; the word recover 1 loads it", name(&keep), name(&doc)),
            Some(offer) if offer.starts_with("autosave: ") => format!("{offer}; {} too: the word recover 1 loads it", name(&keep)),
            Some(other) => other,
        };
        self.last_error = Some(line);
    }
}

// ---- STRIAEXPORTQUEUEMORE (LEDGER SR971): the queue's range / audio on the list, the Esc and running counts (named tail block) ----
// SR932's `export-list` entry names the request's marked range (` ticks <a>..<b>`, `ticks_note`) and its audio word when not
// the default `wav` (`nle_export::audio_word`), so SR932's plain lines are unchanged. Esc (`cancel_export`) drops the waiting
// requests through `drop_queue_on_cancel`, which names them `export: cancelling (dropped <n> waiting)` (none waiting: no
// message, as before); the cancel line replaces it when the job is reported, as SR914's skipping line. The running row
// (`message_text`) carries ` (<n> waiting)` after the cloud's unchanged `job_line` while requests wait. Face thread only.
// Owed: NLE5D-EXPORT-QUEUE-ESC-GTK (a GTK `Shift+Escape` alias: a new key).

/// STRIAEXPORTQUEUEMORE (SR971): an `export-list` entry's tail after `<name> <fmt>`: ` ticks <a>..<b>` for a marked range,
/// then ` <audio>` when the audio is not the default `wav`; empty for a whole-timeline `wav` request.
fn list_entry_note(r: &ExportRequest) -> String {
    let audio = if r.audio == AudioOut::Wav { String::new() } else { format!(" {}", crate::nle_export::audio_word(&r.audio)) };
    format!("{}{audio}", ticks_note(&r.range))
}

impl Face {
    /// Esc's queue arm: drop the waiting requests; when any were dropped the message line names them
    /// (`export: cancelling (dropped <n> waiting)`).
    fn drop_queue_on_cancel(&mut self) {
        let n = self.export_queue.len();
        self.export_queue.clear();
        if n > 0 {
            self.last_error = Some(format!("export: cancelling (dropped {n} waiting)"));
        }
    }
    /// The running row's waiting count: ` (<n> waiting)` while requests wait, else empty.
    fn waiting_note(&self) -> String {
        match self.export_queue.len() {
            0 => String::new(),
            n => format!(" ({n} waiting)"),
        }
    }
}

// ---- STRIACROPMORE (LEDGER SR960): the crop drag on the preview picture and its routing (named tail block) ----
// NLE-CROP-DRAG on the face: `Face::on`'s first line hands every message here first. A press on the preview picture's edge
// (the split face's preview Surface, or an unsplit face's preview band through `takes`) begins timeline_core's
// `view::CropDrag` over the layout the face paints that band with (`layout_for(..).with_message(..)`) and the preview still's
// size (`self.still`, else ONE `preview` call: THE frame road); while live, that Surface's moves update it, its release
// commits ONCE (`CropDrag::finish`: `EditorState::crop_set`; a refusal on the message row as `crop-drag: <e>`), Escape drops
// it; the one-Surface face keeps its `pointer` through them as `press` / `release` do. Anything else (no drag, a press off
// the edge, the help shown) returns `None`: the old road, unchanged. At the fold this call follows SR962's meter click on
// the same `Face::on` line (both ahead of `takes` for the preview Surface).
// NLE-CROP-KEYS: `Alt+l` / `Alt+Shift+L` (t, r, b) are KEYS rows onto SR900's `crop-<side>±` words through `Face::key`'s
// one `Face::verb` road (GTK names; AppKit forwards no Option chord: NLE-CROP-KEYS-MAC owed).

impl Face {
    /// STRIACROPMORE (SR960): the live crop drag on the preview picture.
    pub fn crop_dragging(&self) -> Option<&view::CropDrag> {
        self.crop_drag.as_ref()
    }
    /// The role of the preview picture a message belongs to: split, the preview Surface's `Preview`; unsplit, `Whole` for a
    /// message this face takes; `None` otherwise.
    fn crop_drag_surface(&self, msg: &SMessage) -> Option<Role> {
        match self.split {
            Some(s) => (msg.surface() == Some(s.preview) && s.preview != s.tracks).then_some(Role::Preview),
            None => self.takes(msg).then_some(Role::Whole),
        }
    }
    /// The crop drag's messages (`Some(repaint)`); `None` hands the message on to [`Face::on`]'s road.
    fn crop_drag_on(&mut self, msg: &SMessage) -> Option<bool> {
        if self.crop_drag.is_some() {
            return match msg {
                SMessage::BrowserMouseMove(x, y, _) if self.crop_drag_surface(msg).is_some() => {
                    if self.split.is_none() { self.pointer = Some((*x, *y)); }
                    let d = self.crop_drag.as_mut()?;
                    let before = d.crop();
                    d.update(*x, *y);
                    Some(d.crop() != before)
                }
                SMessage::BrowserButton { down: false, x, y, .. } if self.crop_drag_surface(msg).is_some() => {
                    if self.split.is_none() { self.pointer = Some((*x, *y)); }
                    let mut d = self.crop_drag.take()?;
                    d.update(*x, *y);
                    if let Err(e) = d.finish(&mut self.state) {
                        self.last_error = Some(format!("crop-drag: {e}"));
                    }
                    Some(true)
                }
                SMessage::BrowserKey(k) if k == "Escape" => {
                    self.crop_drag = None;
                    Some(true)
                }
                _ => None,
            };
        }
        let SMessage::BrowserButton { down: true, x, y, .. } = *msg else { return None };
        if self.help {
            return None;
        }
        let role = self.crop_drag_surface(msg)?;
        let l = self.layout_for(role).with_message(self.labels.is_some());
        if self.still.is_none() {
            self.still = preview(&self.state.timeline, &self.sources, self.state.playhead).ok().flatten();
        }
        let (sw, sh) = self.still.as_ref().map(|s| (s.width, s.height))?;
        self.crop_drag = Some(view::CropDrag::begin(&self.state, l, sw, sh, x, y)?);
        self.buttons = true;
        if self.split.is_none() { self.pointer = Some((x, y)); } // the one-Surface face's pointer, as `press` keeps it
        Some(true)
    }
}

// ---- STRIAUNDOMORE (LEDGER SR973): pick an undo step on the history line (named tail block) ----
// The history words (SR876 `history`, SR912 `history <n>`) also hold their line in `Face.note`, which `message_text` reads
// after `last_error`, so the line stays on the message row after `serve` takes `last_error`. While the row shows that line
// (labels on, the whole layout), a press (a click on a click-only surface) on an entry `<p> <word>` runs SR934's one jump
// road, `undo <depth + 1 - p>`: back to before that entry, named on the message line. A press on the row off any entry is
// taken and changes nothing. The next key / text / Input / select-all, or a press or click elsewhere, clears the note and
// runs as before; moves, releases, scroll, zoom, resize and every other message keep it. Owed: NLE-UNDO-PICK-SPLIT (split
// mode: the row is on the preview Surface the face does not take), NLE-UNDO-PICK-REDO (the line lists undo entries only).
impl Face {
    /// STRIAUNDOMORE (SR973): a pick on the noted history line (`Some(true)`), a press on the row off any entry
    /// (`Some(false)`), an input that clears the note (then [`Face::on`] as before, `Some(true)`); `None`: no note, or a
    /// message that keeps it.
    fn history_pick(&mut self, msg: &SMessage) -> Option<bool> {
        self.note.as_ref()?;
        let at = match msg {
            SMessage::BrowserButton { down: true, x, y, .. } => Some((*x, *y)),
            SMessage::BrowserClick(x, y, _) if !self.buttons => Some((*x, *y)),
            SMessage::Input { target, .. } if target == SURFACE => None,
            SMessage::BrowserKey(_) | SMessage::BrowserText(_) | SMessage::BrowserSelectAll => None,
            _ => return None,
        };
        if matches!(msg, SMessage::BrowserButton { .. }) {
            self.buttons = true;
        }
        if let Some(r) = at.and_then(|(x, y)| self.history_pick_at(x, y, self.pointer_role())) {
            return Some(r);
        }
        self.note = None;
        self.on(msg);
        Some(true)
    }
    /// STRIAUNDOMORE (SR973): the point (buffer pixels) on the message row while it shows the noted history line: the
    /// entry under it undone to (`Some(true)`), or `Some(false)` off any entry; `None` off the row or no such line shown.
    fn history_pick_at(&mut self, x: f64, y: f64, role: Role) -> Option<bool> {
        let style = self.labels.as_ref()?;
        if self.split.is_some() != (role == Role::Preview) {
            return None;
        }
        let l = self.layout_for(role).with_message(true);
        if l.message_h < MESSAGE_H || y < l.message_y as f64 || y >= (l.message_y + l.message_h) as f64 {
            return None;
        }
        let text = self.note.clone()?;
        if self.message_text().as_deref() != Some(text.as_str()) {
            return None;
        }
        let b = style.shape(&text).hit(&text, (x - 4.0) as f32); if let Some(r) = self.redo_pick_at(&text, b) { return Some(r); } // STRIAUNDOTHREE (SR9027): a press on a redo entry (file tail)
        let depth = self.state.undo_depth();
        let Some(p) = history_entry_at(&text, b).filter(|p| (1..=depth).contains(p)) else { return Some(false) };
        self.note = None;
        self.verb(&format!("undo {}", depth + 1 - p));
        Some(true)
    }
}

/// STRIAUNDOMORE (SR973): the stack position of the history line's entry whose bytes (its end included) hold byte `b` of
/// `line` (`history: 3 split | 2 (unnamed) | 1 mark in`); `None` on the prefix, a separator, or a line with no entries.
fn history_entry_at(line: &str, b: usize) -> Option<usize> {
    let body = line.strip_prefix("history: ")?;
    let mut start = line.len() - body.len();
    for item in body.split(" | ") {
        let end = start + item.len();
        if (start..=end).contains(&b) {
            return item.split(' ').next()?.parse().ok();
        }
        start = end + 3;
    }
    None
}

// ---- STRIAGAPSMORE (LEDGER SR969): the Close All button word and the `close-gaps sync` message row (named tail block) ----
// `close-all` (VERBS, the Close All button) is the one argument word `close-gaps all` (SR929's note and refusal unchanged);
// `close-gaps sync` rides timeline_core's ONE verb dispatch (one commit, every track moving alike) and follows SR909's note
// rule: a refusal is `close-gaps sync: <why>`, and `close-gaps sync: closed 2 gaps, removed 2 fades` only when a fade went.
impl Face {
    /// The `close-all` / `close-gaps sync` words with their notes; `None` for any other word.
    fn gaps_more_note_verb(&mut self, v: &str) -> Option<bool> {
        if v == "close-all" {
            return self.gaps_all_note_verb("close-gaps all");
        }
        if !v.split_ascii_whitespace().eq(["close-gaps", "sync"]) {
            return None;
        }
        let fades = |tl: &Timeline| tl.tracks.iter().map(|k| k.transitions.len()).sum::<usize>();
        let (gaps, before) = (self.state.timeline.sync_gaps().len(), fades(&self.state.timeline));
        match self.state.verb(v) {
            Some(Err(e)) => self.last_error = Some(format!("close-gaps sync: {e}")),
            Some(Ok(())) => {
                let gone = before.saturating_sub(fades(&self.state.timeline));
                if gone > 0 {
                    let s = |n: usize| if n == 1 { "" } else { "s" };
                    self.note = Some(format!("close-gaps sync: closed {gaps} gap{}, removed {gone} fade{}", s(gaps), s(gone))); // STRIAGAPSTHREE (SR9028): a success line is a note
                }
            }
            None => return Some(false),
        }
        Some(true)
    }
}

// ---- STRIAGAPSTHREE (LEDGER SR9028): the face note, the Close Sync button word, the `close-gaps keep` note (named tail block) ----
// `Face.note` (SR990's settled shape) is read by `message_text` after `last_error` (an error wins) and before the export and
// reel lines; `serve` and the witness never take it, so a gap success line stands across repaints until the next acting input
// clears it. `close-sync` (VERBS, the Close Sync button) is the one argument word `close-gaps sync`; `close-gaps keep` rides
// timeline_core's ONE verb dispatch (one commit): a refusal is `close-gaps keep: <why>` on last_error, and the note
// `close-gaps keep: closed 1 gap, kept 1` stands only when a faded gap stayed open.
impl Face {
    /// Clear the note on an acting input: a typed word, a click, a press, select-all, a named key, a one-char text
    /// key. A move, a release, the wheel, a zoom and a resize keep it.
    fn clear_note(&mut self, msg: &SMessage) {
        let acts = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserButton { down: true, .. } | SMessage::BrowserSelectAll => true,
            SMessage::BrowserKey(k) => k.chars().count() > 1,
            SMessage::BrowserText(t) => t.chars().count() == 1,
            _ => false,
        };
        if acts {
            self.note = None;
        }
    }

    /// The `close-sync` / `close-gaps keep` words with their notes; `None` for any other word.
    fn gaps_three_note_verb(&mut self, v: &str) -> Option<bool> {
        if v == "close-sync" {
            return self.gaps_more_note_verb("close-gaps sync");
        }
        if !v.split_ascii_whitespace().eq(["close-gaps", "keep"]) {
            return None;
        }
        let track = self.state.selection.map(|(t, _)| t);
        let gaps = |tl: &Timeline| track.map_or(0, |t| tl.gap_count(t));
        let before = gaps(&self.state.timeline);
        match self.state.verb(v) {
            Some(Err(e)) => self.last_error = Some(format!("close-gaps keep: {e}")),
            Some(Ok(())) => {
                let kept = gaps(&self.state.timeline);
                if kept > 0 {
                    let closed = before.saturating_sub(kept);
                    self.note = Some(format!("close-gaps keep: closed {closed} gap{}, kept {kept}", if closed == 1 { "" } else { "s" }));
                }
            }
            None => return Some(false),
        }
        Some(true)
    }
}

// ---- STRIAPERFMORE (LEDGER SR974): the preview key on the video projection, seeded by a one-tick played render (named tail block) ----
// SR918's key compared the whole timeline, so an audio-only edit re-rendered the unchanged video still, and a taken played
// still dropped the key, so the first idle paint after a played tick (a stop, a select, a hover) re-rendered the frame it had
// just shown. `preview` renders only the video projection of the timeline ([`video_of`], its one copy): a whole-timeline miss
// at the same playhead whose projection equals the key's keeps the still. A rate-1 played render is `preview`'s frame for the
// timeline the tick read (the NLEPLAYAUDIO pins), so a taken one-tick still seeds the key with THAT timeline (an edit made
// between the tick and its paint then misses on the next paint); a shuttle's still (tick end-1, tagged ph) seeds nothing.
// Owed: NLE-PERF-PREVIEW-SCALE, NLE-PERF-DAMAGE, NLE-PERF-LABEL-STRING, NLE-PERF-HELP-COLS.

/// STRIAPERFMORE (SR974): the timeline `preview` renders: `tl` with its video tracks only.
fn video_of(tl: &Timeline) -> Timeline {
    let mut video = tl.clone();
    video.tracks.retain(|tr| tr.kind == Kind::Video);
    video
}

impl PreviewCache {
    /// STRIAPERFMORE (SR974): true when the key is at `t` and its video projection is `tl`'s (the still is the one a render
    /// would give); the key then becomes `(tl, t)` so the next paint is the whole-timeline hit.
    fn video_hit(&mut self, tl: &Timeline, t: i64) -> bool {
        let Some((k, kt)) = self.key.as_ref() else { return false };
        if *kt != t || video_of(k) != video_of(tl) {
            return false;
        }
        self.key = Some((tl.clone(), t));
        true
    }
    /// STRIAPERFMORE (SR974): a played render of `tl` over `ph..end`: its (timeline, playhead) when it rendered one tick,
    /// else none (a shuttle's still is not `preview`'s at `ph`).
    fn played(&mut self, tl: &Timeline, ph: i64, end: i64) {
        self.played = (end == ph + 1).then(|| (tl.clone(), ph));
    }
}

#[cfg(test)]
mod striaperfmore_tests {
    use super::*;

    fn face() -> Face {
        let (tl, src) = synth_fixture();
        Face::new(EditorState::new(tl), src, (320, 240), PixelOrder::Rgba, false)
    }

    fn pixels(blits: Vec<SMessage>) -> Vec<u8> {
        match blits.into_iter().last() {
            Some(SMessage::SurfaceBlit { pixels, .. }) => pixels,
            _ => panic!("a blit"),
        }
    }

    /// STRIAPERFMORE M3: an audio-only edit keeps the still; a video edit renders.
    #[test]
    fn striaperfmore_audio_edit_keeps_still() {
        let mut f = face();
        f.state.playhead = 3;
        f.frames();
        assert_eq!(f.preview_renders(), 1);
        assert_eq!(f.state.timeline.tracks[1].kind, Kind::Audio);
        f.state.timeline.tracks[1].clips.pop();
        f.frames();
        f.frames();
        assert_eq!(f.preview_renders(), 1, "an A1 edit: the video projection is unchanged");
        let want = preview(&f.state.timeline, &f.sources, 3).expect("renders");
        assert_eq!(f.still().cloned(), want);
        f.state.timeline.tracks[0].clips.pop();
        f.frames();
        assert_eq!(f.preview_renders(), 2, "a V1 edit renders");
    }

    /// STRIAPERFMORE M3: a taken one-tick played still seeds the key: the stopped repaint renders nothing and paints the bytes
    /// a fresh face paints.
    #[test]
    fn striaperfmore_played_tick_seeds_key() {
        let mut f = face();
        assert!(f.verb("play"));
        assert!(f.tick());
        f.frames();
        assert_eq!(f.preview_renders(), 0, "the paint took the played still");
        assert!(f.verb("play"));
        assert!(!f.playing);
        let got = pixels(f.frames());
        assert_eq!(f.preview_renders(), 0, "the seeded key holds");
        assert_eq!(f.state.playhead, 0);
        let want = preview(&f.state.timeline, &f.sources, 0).expect("renders");
        assert_eq!(f.still().cloned(), want);
        let mut fresh = face();
        assert_eq!(got, pixels(fresh.frames()));
    }

    /// STRIAPERFMORE M3: a shuttle's still seeds nothing, and an edit between a tick and its paint misses on the next paint.
    #[test]
    fn striaperfmore_shuttle_and_stale_do_not_seed() {
        let mut f = face();
        f.on(&SMessage::BrowserText("l".into()));
        f.on(&SMessage::BrowserText("l".into()));
        assert_eq!((f.playing, f.rate), (true, 2));
        assert!(f.tick());
        f.frames();
        f.frames();
        assert_eq!(f.preview_renders(), 1, "a rate-2 still is not preview's: the next paint renders");

        let mut g = face();
        assert!(g.verb("play"));
        assert!(g.tick());
        g.state.timeline.tracks[0].clips.remove(0);
        g.frames();
        assert_eq!(g.preview_renders(), 0, "the taken still, as before");
        g.frames();
        assert_eq!(g.preview_renders(), 1, "the seeded key is the timeline the tick read");
        let want = preview(&g.state.timeline, &g.sources, g.state.playhead).expect("renders");
        assert_eq!(g.still().cloned(), want);
    }
}

// ---- STRIAHELPMORE (LEDGER SR977): the help query field on the bar and drawn on the band (named tail block) ----
// The bar's query field (stria-view's TextField `HELP_FIELD`, firing `SMessage::ConsoleInput` on Enter) is a second door
// onto THE `help <word>` road (`help_search_verb`): its text filters, a text no row matches is refused by name and the help
// left as it was, a blank Enter is the bare `help` toggle. Every ConsoleInput the face takes is a help query (stria reads
// no other). While a query is set, `paint_help` draws it on the band's first line (`find: <q>_`, the `_` the caret) and its
// rows start one line lower; with no query nothing moves, so the bare help paints byte-identically. View-only.
// Owed: NLE5D-KEYS-HELP-FIELD-SYNC (the field's text is not updated when keys change the query: quartzite has no set-text
// message for a TextField), NLE5D-KEYS-HELP-BUTTON-STATE (a Tetra Button carries no state).

/// STRIAHELPMORE (SR977): the id of the bar's help query field (stria-view's `TextField`).
pub const HELP_FIELD: &str = "help_query";

impl Face {
    /// The bar's query field's Enter: `help <q>` through [`Face::help_search_verb`] (a blank text is the bare `help`).
    fn help_field(&mut self, q: &str) -> bool {
        let q = q.trim();
        if q.is_empty() {
            return self.verb("help");
        }
        self.verb(&format!("help {q}"))
    }
    /// The query line the help draws on the band while a query is set (`find: <q>_`); `None` without one.
    pub fn help_query_text(&self) -> Option<String> {
        self.help_query.as_deref().map(|q| format!("find: {q}_"))
    }
    /// The height the query line takes from the band's top: one `pitch` while a query is set, else nothing.
    fn help_query_line(&self, pitch: f32) -> f32 {
        if self.help_query.is_some() { pitch } else { 0.0 }
    }
    /// Draw [`Face::help_query_text`] on the band's first line; nothing without a query.
    fn paint_help_query(&self, lab: &mut Labels<'_>, c: &mut Canvas, l: Layout, ascent: f32) {
        let Some(s) = self.help_query_text() else { return };
        view::Text::text(lab, c, &s, HELP_PAD, l.ruler_y as f32 + HELP_PAD + ascent, (0, l.w as i32));
    }
}

// ---- STRIABINMORE (LEDGER SR976): the unused AV reel's lay on every kind its media holds, the bin's thumb (named tail block) ----
// No fn of its own: `bin_unused` lays an unused reel on the first track of EVERY kind its media holds through timeline_core
// `EditorState::place_tracks_at_playhead` (ONE commit, one undo step), and `paint_bin` paints view's `paint_bin_thumb`
// (SR939's rows plus a scroll thumb only when they overflow). The laid V / A pair is not linked (NLE-BIN-AVLINK-GROUP owed).

#[cfg(test)]
mod striabinmore_tests {
    use super::*;
    use timeline_core::{Mode, SourceId};

    fn click(n: u32, (x, y): (f64, f64)) -> SMessage {
        SMessage::BrowserClick(x, y, n)
    }

    /// The fixture plus reel C (synth media: video and audio) on no track.
    fn with_unused() -> (Face, SourceId) {
        let (mut tl, mut src) = synth_fixture();
        let c = tl.add_source("C", 50);
        src.add(c, synth_source(2000, 50, 25)).expect("synthetic source opens");
        (Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).on_surfaces(0, 1).with_bin(2), c)
    }

    /// An unused AV reel lands on V1 and A1 at the playhead in ONE commit; one undo takes both away.
    #[test]
    fn striabinmore_unused_av_reel_lays_video_and_audio() {
        let (mut f, c) = with_unused();
        let tl0 = f.state.timeline.clone();
        f.state.deselect();
        f.state.seek(15);
        assert!(f.on(&click(2, view::bin_point(2))));
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        let laid = Clip::new(c, 0, 50, 15);
        assert_eq!(f.state.timeline, tl0.place(0, laid, Mode::Insert).unwrap().place(1, laid, Mode::Insert).unwrap());
        assert_eq!((f.state.selection.map(|p| p.0), f.state.selected(), f.state.playhead, f.state.undo_depth()), (Some(0), Some(laid), 65, 1));
        assert!(f.verb("undo"));
        assert_eq!(f.state.timeline, tl0, "one undo restores both tracks");
    }

    /// The bin paints a thumb at its right edge only when the rows overflow its canvas.
    #[test]
    fn striabinmore_bin_paint_carries_the_thumb_on_overflow() {
        let (mut f, _) = with_unused();
        let fit = f.paint_role(Role::Bin);
        let (w, h) = f.bin().unwrap().canvas;
        assert_eq!(fit, view::paint_bin_from(&f.state.timeline, w, h, 0, Order::Rgba, None), "every row fits: no thumb");
        assert!(f.on(&SMessage::BrowserResize(320, view::BIN_ROW_H, 2)));
        let got = f.paint_role(Role::Bin);
        assert_eq!(got, view::paint_bin_thumb(&f.state.timeline, 320, view::BIN_ROW_H, 0, Order::Rgba, None));
        assert_ne!(got, view::paint_bin_from(&f.state.timeline, 320, view::BIN_ROW_H, 0, Order::Rgba, None), "the thumb shows");
    }
}

// ---- STRIAAUDIOSCRUBNEXT (LEDGER SR988): a refused scrub step is retried on the face clock (named tail block) ----

impl Face {
    /// STRIAAUDIOSCRUBNEXT (SR988): SR947's gate with the refusal recorded: not due records `ph` as the tick a later face
    /// clock tick owes its sound; due drops the record. Reached only when every other clause of `scrub_burst`'s guard passed.
    fn scrub_gate(&mut self, ph: i64) -> bool {
        let due = self.scrub_due();
        self.play.scrub_retry = if due { None } else { Some(ph) };
        due
    }
    /// STRIAAUDIOSCRUBNEXT (SR988): a refused scrub step is owed its sound: `serve` runs its clock.
    pub fn scrub_pending(&self) -> bool {
        self.play.scrub_retry.is_some()
    }
    /// STRIAAUDIOSCRUBNEXT (SR988): one face clock tick of the retry: playing or a playhead moved off the recorded tick
    /// drops it; not yet due keeps it; due runs SR865's `scrub_burst` itself (the whole guard re-checked) and drops it.
    /// True only when a render happened (the repaint takes its still).
    fn scrub_retry_tick(&mut self) -> bool {
        let Some(t) = self.play.scrub_retry else { return false };
        if self.playing || t != self.state.playhead {
            self.play.scrub_retry = None;
            return false;
        }
        if !self.scrub_due() {
            return false;
        }
        let n = self.play.renders;
        self.scrub_burst(i64::MIN);
        self.play.scrub_retry = None;
        self.play.renders != n
    }
}

#[cfg(test)]
mod audioscrubnext_tests {
    use super::playaudio_tests::{Ev, face, still_at};
    use super::*;

    fn btn(down: bool, x: f64, y: f64) -> SMessage {
        SMessage::BrowserButton { down, x, y, mods: 0, surface: 0 }
    }

    /// STRIAAUDIOSCRUBNEXT M3: a step the gate refused is owed; a clock tick before the device drained keeps it; the tick
    /// after sounds it (one render) and the paint takes its still.
    #[test]
    fn striaaudioscrubnext_retry_sounds() {
        let (mut f, log) = face();
        log.lock().unwrap().clear();
        f.on(&btn(true, 44.0, 70.0));
        assert_eq!((f.state.playhead, f.played().renders), (5, 1));
        log.lock().unwrap().clear();
        f.on(&SMessage::BrowserMouseMove(124.0, 70.0, 0));
        assert_eq!((f.state.playhead, f.played().renders), (15, 1));
        assert!(f.scrub_pending());
        assert!(!f.tick(), "not drained yet");
        assert_eq!(f.played().renders, 1);
        assert!(f.scrub_pending());
        log.lock().unwrap().push(Ev::Push(vec![0.0; 2 * 1440]));
        assert!(f.tick());
        assert_eq!((f.state.playhead, f.played().renders), (15, 2));
        assert!(!f.scrub_pending());
        let _ = f.frames();
        assert!(f.play.pending.is_none(), "the paint took the retried still");
        assert_eq!(f.still().cloned(), still_at(15));
        assert!(!f.playing);
    }

    /// STRIAAUDIOSCRUBNEXT M3: a playhead moved off the refused tick drops the retry; play drops it too.
    #[test]
    fn striaaudioscrubnext_retry_dropped() {
        let (mut f, log) = face();
        log.lock().unwrap().clear();
        f.on(&btn(true, 44.0, 70.0));
        log.lock().unwrap().clear();
        f.on(&SMessage::BrowserMouseMove(124.0, 70.0, 0));
        assert!(f.scrub_pending());
        f.state.seek(3);
        log.lock().unwrap().push(Ev::Push(vec![0.0; 2 * 1440]));
        assert!(!f.tick());
        assert!(!f.scrub_pending());
        assert_eq!(f.played().renders, 1);

        let (mut f, log) = face();
        log.lock().unwrap().clear();
        f.on(&btn(true, 44.0, 70.0));
        log.lock().unwrap().clear();
        f.on(&SMessage::BrowserMouseMove(124.0, 70.0, 0));
        assert!(f.scrub_pending());
        f.on(&btn(false, 124.0, 70.0));
        f.verb("play");
        assert!(!f.scrub_pending(), "a transport change drops the retry");
    }

    /// STRIAAUDIOSCRUBNEXT M3: a face without a device never owes a retry.
    #[test]
    fn striaaudioscrubnext_no_device() {
        let (tl, src) = synth_fixture();
        let mut mute = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        mute.on(&btn(true, 44.0, 70.0));
        mute.on(&SMessage::BrowserMouseMove(124.0, 70.0, 0));
        assert!(!mute.scrub_pending());
        assert!(!mute.tick());
    }
}

// ---- STRIASNAPNEXT (LEDGER SR990): the face note — a success line on the message row (named tail block) ----
// `Face.note` is read by `message_text` after `last_error` (an error wins) and before the export line and the reel
// line; `serve` and the witness never take it, so it stands across repaints until the next acting input clears it.
impl Face {
    /// Clear the note on an acting input: a typed word, a click, a press, select-all, a named key, a one-char text
    /// key. A move, a release, the wheel, a zoom and a resize keep it.
    fn clear_note(&mut self, msg: &SMessage) {
        let acts = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserButton { down: true, .. } | SMessage::BrowserSelectAll => true,
            SMessage::BrowserKey(k) => k.chars().count() > 1,
            SMessage::BrowserText(t) => t.chars().count() == 1,
            _ => false,
        };
        if acts {
            self.note = None;
        }
    }
}

// ---- STRIAXFADEMORE (LEDGER SR980): the face's note — a success reading on the message row, not an error (named tail block) ----
// SR925 owed NLE-XFADE-READING-STATUS: the crossfade reading (`xfade: 10 equal`) rode `last_error`, so `serve` logged a
// success as a warn and the witness listed it under `"errors"`. It is now `Face::note`, which `message_text` reads after
// `last_error`; the next input that acts clears it at the top of `Face::on` (a typed word, a click, a press, select-all, a
// taken key by `Face::key`'s rule: a one-character key from `BrowserText`, a named one from `BrowserKey`); a pointer move,
// a wheel, a resize or a zoom leaves it. Owed: NLE-FACE-NOTE-USERS (the export's done line still rides `last_error`).

impl Face {
    /// Clear the note when `msg` is an input that acts (see the block comment).
    fn clear_note(&mut self, msg: &SMessage) {
        let acts = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserButton { down: true, .. } | SMessage::BrowserSelectAll => true,
            SMessage::BrowserKey(k) => k.chars().count() != 1,
            SMessage::BrowserText(t) => t.chars().count() == 1,
            _ => false,
        };
        if acts {
            self.note = None;
        }
    }
}

// ---- STRIATRANSITIONNEXT (LEDGER SR991): the new transition length on the message row (named tail block) ----
// A successful length change of the selected transition (`dur ...`, `[` / `]` by button, witness or key: one road through
// `Face::verb`) sets the face `note` `transition: HH:MM:SS:FF (<n> ticks)` (`edl::timecode` at the timeline's fps;
// `transition: <n> ticks` off a 1/fps time base). `message_text` reads it after `last_error`; a refusal stays the error.
// `clear_note` drops it on the next taken command input (an Input, a click, a press, select-all, a named key, a typed
// character); a move, a release, the wheel, a zoom and a resize keep it. The length ops are unchanged:
// `transition_verb` (`set_transition_dur`) and `EditorState::verb` (`dur`). Owed: NLE-TRANSITION-LENGTH-NOTE-DRAG (the
// edge drag's release sets no note), NLE-TRANSITION-LENGTH-KERNEL (the Player selects no transition).

impl Face {
    /// Drop the note on a taken command input; a move, release, wheel, zoom or resize keeps it.
    fn clear_note(&mut self, msg: &SMessage) {
        let clears = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserButton { down: true, .. } | SMessage::BrowserSelectAll => true,
            SMessage::BrowserKey(k) => k.chars().count() != 1,
            SMessage::BrowserText(t) => t.chars().count() == 1,
            _ => false,
        };
        if clears {
            self.note = None;
        }
    }
    /// `[`, `]` or `dur ...` with a transition selected: the length change through its one road, then the note on success.
    /// `None` with no transition selected or any other verb (the caller's roads, refusals included, as before).
    fn transition_len_verb(&mut self, v: &str) -> Option<bool> {
        self.state.selected_transition()?;
        let failed = match v {
            "[" | "]" => {
                let e0 = self.last_error.clone();
                self.transition_verb(v)?;
                self.last_error.is_some() && self.last_error != e0
            }
            _ if v.split_ascii_whitespace().next() == Some("dur") => match self.state.verb(v)? {
                Ok(()) => false,
                Err(e) => {
                    self.last_error = Some(format!("{v}: {e}"));
                    true
                }
            },
            _ => return None,
        };
        if !failed {
            self.note = self.transition_note();
        }
        Some(true)
    }
    /// The selected transition's length as the note's text; `None` with none selected.
    fn transition_note(&self) -> Option<String> {
        let n = self.state.selected_transition()?.dur;
        Some(match self.state.timeline.fps() {
            Some(f) => format!("transition: {} ({n} ticks)", timeline_core::edl::timecode(n, f)),
            None => format!("transition: {n} ticks"),
        })
    }
}

// ---- STRIAMETERSNEXT (LEDGER SR996): one meter layout for the strip, its clip caps and its clicks (named tail block) ----

#[cfg(test)]
mod striametersnext_tests {
    use super::*;

    /// M3: full bars (0 dB RMS and peak) fill exactly each meter's [`meter_rect`]: every pixel inside is the RMS fill or
    /// the peak line, the row at `y1` and the column at `x1` stay the base; a meter with no rect paints nothing.
    #[test]
    fn striametersnext_strip_paints_exactly_meter_rect() {
        let mut st = EditorState::new(synth_fixture().0);
        st.seek(15);
        let l = Layout::of(320, 160, &st);
        let base = paint(&st, 320, 160, PixelOrder::Rgba, None, None);
        let keys = [Some(1), None];
        let levels: Vec<MeterLevel> = keys.iter().map(|&track| MeterLevel { track, peak_db: 0.0, rms_db: 0.0 }).collect();
        let rgba = paint_meters(base.clone(), l, PixelOrder::Rgba, &levels);
        let at = |px: &[u8], x: i64, y: i64| px[((y as usize) * 320 + x as usize) * 4..((y as usize) * 320 + x as usize) * 4 + 4].to_vec();
        for k in keys {
            let (x0, y0, x1, y1) = meter_rect(l, k).expect("a bar");
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = at(&rgba, x, y);
                    assert!(p == METER_RMS.to_vec() || p == METER_PEAK.to_vec(), "{k:?} ({x}, {y})");
                }
                assert_eq!(at(&rgba, x1, y), at(&base, x1, y), "{k:?} column x1 untouched");
            }
            for x in x0..x1 {
                assert_eq!(at(&rgba, x, y1), at(&base, x, y1), "{k:?} row y1 untouched");
            }
        }
        let off = Some(l.rows + 5);
        assert!(meter_rect(l, off).is_none());
        let none = [MeterLevel { track: off, peak_db: 0.0, rms_db: 0.0 }];
        assert_eq!(paint_meters(base.clone(), l, PixelOrder::Rgba, &none), base, "no rect paints nothing");
    }
}

// ---- STRIAINSERTMARKSKEYSNEXT (LEDGER SR985): the all-tracks insert words and the target track on the face (named tail block) ----
// `insert-all` / `insert-marked-all` are SR924's laid words too (the laid line names the clip they lay); `target` goes through
// timeline_core's ONE dispatch (`EditorState::verb`: session state, no undo step, the document stays clean) and leaves
// `target: V2` / `target: off` in SR924's `laid` note (cleared by the next taken input); a refusal is `<word>: <why>`.
// Owed: NLE-INSERT-TARGET-PAINT (the band marks the target's header).

impl Face {
    /// The `target` words: timeline_core's dispatch, then the note (`Some(true)`); `None` for any other word.
    fn target_verb(&mut self, v: &str) -> Option<bool> {
        if v.split_ascii_whitespace().next()? != "target" {
            return None;
        }
        match self.state.verb(v)? {
            Ok(()) => self.laid = Some(format!("target: {}", self.state.target_label().unwrap_or_else(|| "off".to_string()))),
            Err(e) => {
                self.laid = None;
                self.last_error = Some(format!("{v}: {e}"));
            }
        }
        Some(true)
    }
}

// ---- STRIAEXPORTPRESETSNEXT (LEDGER SR989): the picked export preset named on the message row (named tail block) ----
impl Face {
    /// The message row's last fallback while a preset is picked ([`Face::export_preset`]): `export preset <name>`, shown
    /// whenever nothing else is said (an error, a note, a running job and a reel line all win). None without a pick, so
    /// the default face paints nothing new.
    fn picked_line(&self) -> Option<String> {
        self.export_preset.map(|p| format!("export preset {}", p.name))
    }
}

// ---- STRIAJKLNEXT (LEDGER SR987): NLEPLAY-SYNC, the face's A/V offset against consumed (named tail block) ----
// A played tick shows its still as it pushes its sound, so the picture leads by the ring's fill. The face keeps ONE gneiss_pal
// `avsync::AudioClock` per heard 1x run (the clock media.rs's `MasterClock::Audio` wraps), fed by the device's `consumed` delta,
// and ONE `DriftMeter` that records each shown tick's pts against it (pts - clock > 0: the picture leads). Measurement only.
// Seeded at a run's first tick and at every discontinuity (`play.next != Some(ph)`, play_tick's flush test); nothing is
// recorded on a seed tick; not measured at rate != 1, muted or paused (the clock drops, 1x reseeds). Latency 0 and no wall
// interpolation (seat ruling): the clock reads exactly what the device consumed. Wire: `[STRIA] :: nle play sync av_max_ms=
// av_mean_ms= ticks=` once per heard run that recorded a tick; `Face::play_sync`. Owed: NLEPLAY-SYNC-PACE (the picture
// shown at the clock's tick), NLEPLAY-SYNC-SHUTTLE (measured at 2x..8x / reverse through `set_rate_ppm`).

/// STRIAJKLNEXT (SR987): the heard run's clock (None until seeded), the consumed count it was last fed at, the run's offsets.
struct PlaySync {
    clock: Option<gneiss_pal::dsp::avsync::AudioClock>,
    seen: u64,
    meter: gneiss_pal::dsp::avsync::DriftMeter,
}

impl Default for PlaySync {
    fn default() -> Self {
        PlaySync { clock: None, seen: 0, meter: gneiss_pal::dsp::avsync::DriftMeter::default() }
    }
}

impl Face {
    /// STRIAJKLNEXT (SR987): a heard run starts (`live`: a fresh clock and meter) or ends (the offsets logged once, the clock
    /// dropped; the meter stays readable until the next run).
    fn sync_run(&mut self, live: bool) {
        if live {
            self.play.sync = PlaySync::default();
            return;
        }
        self.play.sync.clock = None;
        let r = self.play.sync.meter.report();
        if r.av_count > 0 {
            log::info!("[STRIA] :: nle play sync av_max_ms={} av_mean_ms={:.1} ticks={}", r.av_max_ns / 1_000_000, r.av_mean_ns / 1e6, r.av_count);
        }
    }
    /// STRIAJKLNEXT (SR987): the played forward tick shown at `ph`, before its push: seed the clock at `ph` (the first tick, a
    /// discontinuity), else feed it the consumed delta and record `ph`'s pts against it. Not at rate != 1, muted or paused.
    fn sync_mark(&mut self, ph: i64) {
        let consumed = match self.play.out.as_ref() {
            Some(o) if self.play.live && self.rate == 1 => o.consumed(),
            _ => {
                self.play.sync.clock = None;
                return;
            }
        };
        let pts = self.state.timeline.tb.to_ns(ph);
        let jump = self.play.next != Some(ph);
        let s = &mut self.play.sync;
        if !jump && s.clock.is_some() {
            if let Some(c) = s.clock.as_mut() {
                c.on_played(consumed.saturating_sub(s.seen));
                // STRIAPLAYSYNCPACE (SR9055): the offset is recorded when the still is presented, `Face::pace` (file tail)
            }
            s.seen = consumed;
        } else {
            let src: std::sync::Arc<dyn gneiss_pal::dsp::avsync::TimeSource> = std::sync::Arc::new(gneiss_pal::dsp::avsync::SystemTime::new());
            let mut c = gneiss_pal::dsp::avsync::AudioClock::new(src, self.play.rate, 0, 0);
            c.seek(pts);
            c.play();
            s.clock = Some(c);
            s.seen = consumed;
        }
    }
    /// STRIAJKLNEXT (SR987): the A/V offsets of the current (or last) heard 1x run: each shown tick's pts minus the AudioClock
    /// fed by the device's `consumed` (max / mean / count).
    pub fn play_sync(&self) -> gneiss_pal::dsp::avsync::DriftReport {
        self.play.sync.meter.report()
    }
}

#[cfg(test)]
mod striajklnext_tests {
    use super::*;
    use crate::media::AudioOut;
    use crate::nle::SYNTH_RATE;
    use std::sync::{Arc, Mutex};

    /// The tests' device: takes every sample; `consumed` lags what was pushed by `lag` frames.
    struct Out {
        pushed: Arc<Mutex<u64>>,
        lag: u64,
    }
    impl AudioOut for Out {
        fn push(&mut self, stereo: &[f32]) -> usize {
            *self.pushed.lock().unwrap() += (stereo.len() / 2) as u64;
            stereo.len()
        }
        fn consumed(&self) -> u64 {
            self.pushed.lock().unwrap().saturating_sub(self.lag)
        }
    }

    fn face(lag: u64) -> Face {
        let (tl, src) = synth_fixture();
        let out = Out { pushed: Arc::new(Mutex::new(0)), lag };
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_audio(Box::new(out), SYNTH_RATE)
    }

    /// STRIAJKLNEXT M3: a device that has played all it was given: three 1x ticks, the first seeds, two offsets, both 0.
    #[test]
    fn striajklnext_sync_heard_device() {
        let mut f = face(0);
        f.verb("play");
        for _ in 0..3 {
            assert!(f.tick());
        }
        let r = f.play_sync();
        assert_eq!((r.av_count, r.av_max_ns), (2, 0), "{r:?}");
    }

    /// STRIAJKLNEXT M3: a device 960 frames behind (20 ms at 48 kHz): every recorded offset is the picture leading by 20 ms,
    /// within one frame.
    #[test]
    fn striajklnext_sync_reads_ring_lag() {
        let mut f = face(960);
        f.verb("play");
        for _ in 0..3 {
            assert!(f.tick());
        }
        let r = f.play_sync();
        assert_eq!(r.av_count, 2, "{r:?}");
        assert!((r.av_max_ns - 20_000_000).abs() <= 21_000, "{r:?}");
        assert!((r.av_mean_ns - 20_000_000.0).abs() <= 21_000.0, "{r:?}");
    }

    /// STRIAJKLNEXT M3: a 2x shuttle is not measured: one 1x tick seeds, the 2x ticks drop the clock, nothing is recorded.
    #[test]
    fn striajklnext_sync_not_shuttle() {
        let mut f = face(0);
        assert!(f.on(&SMessage::BrowserText("l".into())));
        assert!(f.tick());
        assert!(f.on(&SMessage::BrowserText("l".into())));
        assert_eq!(f.rate, 2);
        for _ in 0..3 {
            assert!(f.tick());
        }
        assert_eq!(f.play_sync().av_count, 0);
        assert!(f.play.sync.clock.is_none());
    }

    /// STRIAJKLNEXT M3: no device, nothing measured.
    #[test]
    fn striajklnext_sync_muted() {
        let (tl, src) = synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        f.verb("play");
        for _ in 0..3 {
            assert!(f.tick());
        }
        assert_eq!(f.play_sync().av_count, 0);
    }
}

// ---- STRIAOPACITYNEXT (LEDGER SR993): the face note, a success line that is not an error (named tail block) ----
// NLE-OPACITY-MORE-NOTE. `Face.note` holds a success line (the opacity step's `opacity: <pct> %`); `message_text` reads it
// after `last_error`, so the witness and `serve`, which take `last_error`, never report it as an error. The next taken
// input clears it: a typed word, a click, a press, select-all, a named key, a one-character text; a pointer move, a
// release, the wheel, a zoom or a resize keeps it.

impl Face {
    /// STRIAOPACITYNEXT: clear [`Face::note`] when `msg` is a taken input (see the block comment).
    fn clear_note(&mut self, msg: &SMessage) {
        let input = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserSelectAll => true,
            SMessage::BrowserButton { down, .. } => *down,
            SMessage::BrowserKey(_) => true,
            SMessage::BrowserText(t) => t.chars().count() == 1,
            _ => false,
        };
        if input {
            self.note = None;
        }
    }
}

// ---- STRIAROLLNEXT (LEDGER SR999): the roll tool's cue and GTK's order for the slide-ten chords (named tail block) ----
// While the roll tool is on and no drag is live, `paint_role` paints timeline_core's `view::paint_roll_cue` under the drag
// overlay: a ROLL_CUE column over each cut a press on the primary's handles would roll (the press road and SR954's
// `Drag::rolled` asked on a probe state); off, the canvas is untouched. SR752's Cmd slide-ten rows take GTK's chord order
// (`Shift+Meta+less` / `Shift+Meta+greater`), as SR954 did for the roll rows. Owed: NLE-ROLL-TOOL-CUE-KERNEL.

// ---- STRIATHUMBNAILSNEXT (LEDGER SR997): heads and peak tables decoded off the face's thread (named tail block) ----
// SR839's heads (`Thumbs::thumb_for`) and SR827's peak tables (`nle::scan_peaks`) were decoded on the face's thread inside the
// paint: a long-GOP head held a paint for its run-up, a long source's first paint for its whole audio decode. `serve` (the
// windowed face; its only caller is stria-view's window) now calls `Face::decode_off_thread`, which starts ONE worker thread
// taking the face's `Arc<Sources>` per job (STRIAPROXYWATCH SR9079: no lifelong clone; see its tail block). RULES:
// * ASK: with a worker, `thumb_for` sends a `BgJob::Head` for an absent key and caches a `None` placeholder (the body keeps its
//   bar; `fill`'s `contains_key` asks once); `wave_fill` sends a `BgJob::Scan` and caches `None` (the body stays flat). The
//   THUMBS_PER_PAINT / THUMB_BUDGET_MS rules are unchanged: they bound the requests one paint makes.
// * DECODE: the worker runs THE SAME roads: `thumb_for` on a scratch `Thumbs` (no worker: synchronous) and `scan_peaks`.
// * TAKE: `WavePeaks::drain_bg`, at `wave_fill`'s start (every paint), moves each finished job into the face's maps (a head
//   through `Thumbs::insert`, overwriting its placeholder; `decodes` / `scans` counted as they land).
// * WAKE: `thumbs_pending` is also true while a job is out, so `serve`'s existing clock repaints until the results land.
// * LOST: a worker gone (its channel disconnected) is dropped and its in-flight keys are removed, so the face's own thread asks
//   again. A face not served (the witness, every test) never starts a worker and stays synchronous (its frames unchanged).
// No lock: the maps stay the face thread's; the results move by channel. Owed: NLE-THUMBNAILS-BG-WAKE (a repaint per clock
// tick while a job is out, not only on arrival), NLE-THUMBNAILS-BG-CANCEL (a queued job for a key scrolled away still runs; a
// dropped face's running job finishes).

/// STRIATHUMBNAILSNEXT (SR997): one job for the decode worker.
enum BgJob {
    /// A head: the face's timeline with its tracks cleared (`thumb_for` builds its one-clip timeline from it) and the key.
    Head(Timeline, timeline_core::SourceId, i64, Arc<Sources>, u64), // STRIAPROXYWATCH (SR9079): the job's own clone and its generation
    /// A source's peak table.
    Scan(timeline_core::SourceId, Arc<Sources>, u64), // STRIAPROXYWATCH (SR9079): the job's own clone and its generation
}

/// STRIATHUMBNAILSNEXT (SR997): one finished job (a head with the worker's decode count; a table).
enum BgDone {
    Head((timeline_core::SourceId, i64), Option<Still>, u32, u64), // STRIAPROXYWATCH (SR9079): + the job's generation
    Scan(timeline_core::SourceId, Option<view::Peaks>, u64), // STRIAPROXYWATCH (SR9079): + the job's generation
}

/// STRIATHUMBNAILSNEXT (SR997): the decode worker's channel ends and the jobs in flight.
struct Bg {
    tx: std::sync::mpsc::Sender<BgJob>,
    rx: std::sync::mpsc::Receiver<BgDone>,
    out: u32,
    heads: Vec<(timeline_core::SourceId, i64)>,
    scans: Vec<timeline_core::SourceId>,
    lease: Option<(Arc<Sources>, u64)>, // STRIAPROXYWATCH (SR9079): the face's parked clone and generation, taken at a paint, released by a reload
}

impl Bg {
    /// The worker thread over `sources`; it ends when the face's job sender drops. A refused spawn: `None` (synchronous).
    fn start() -> Option<Bg> { // STRIAPROXYWATCH (SR9079): no lifelong clone: each job carries its own
        let (tx, jobs) = std::sync::mpsc::channel::<BgJob>();
        let (done, rx) = std::sync::mpsc::channel::<BgDone>();
        let spawned = std::thread::Builder::new().name("nle-face-decode".into()).spawn(move || {
            for job in jobs {
                let d = match job {
                    BgJob::Head(tl, src, src_in, sources, sgen) => {
                        let mut t = Thumbs::default();
                        let s = t.thumb_for(&tl, &sources, src, src_in).cloned();
                        drop(sources);
                        BgDone::Head((src, src_in), s, t.decodes, sgen)
                    }
                    BgJob::Scan(id, sources, sgen) => {
                        let p = match crate::nle::scan_peaks(&sources, id, view::PEAKS_PER_SEC) {
                            Ok(p) => p,
                            Err(e) => {
                                log::debug!("[nle-face] waveform {id:?}: {e}");
                                None
                            }
                        };
                        drop(sources);
                        BgDone::Scan(id, p, sgen)
                    }
                };
                if done.send(d).is_err() {
                    break;
                }
            }
        });
        match spawned {
            Ok(_) => Some(Bg { tx, rx, out: 0, heads: Vec::new(), scans: Vec::new(), lease: None }),
            Err(e) => {
                log::warn!("[nle-face] decode worker: {e} (decoding on the face's thread)");
                None
            }
        }
    }
}

impl Thumbs {
    /// STRIATHUMBNAILSNEXT (SR997): with a worker and `(src, src_in)` not keyed, queue its head and cache the `None`
    /// placeholder (true); otherwise false (the synchronous road runs).
    fn ask_head(&mut self, tl: &Timeline, src: timeline_core::SourceId, src_in: i64) -> bool {
        if self.map.contains_key(&(src, src_in)) {
            return false;
        }
        let sent = match self.bg.as_mut() {
            None => false,
            Some(bg) => {
                let mut one = tl.clone();
                one.tracks.clear();
                let ok = bg.lease.clone().is_some_and(|(s, g)| bg.tx.send(BgJob::Head(one, src, src_in, s, g)).is_ok()); // STRIAPROXYWATCH (SR9079): no lease: the face's own road
                if ok {
                    bg.out += 1;
                    bg.heads.push((src, src_in));
                }
                ok
            }
        };
        if sent {
            self.insert((src, src_in), None);
        }
        sent
    }
    /// STRIATHUMBNAILSNEXT (SR997): with a worker, queue source `id`'s peak scan (true: the caller caches the placeholder).
    fn ask_scan(&mut self, id: timeline_core::SourceId) -> bool {
        match self.bg.as_mut() {
            None => false,
            Some(bg) => {
                let ok = bg.lease.clone().is_some_and(|(s, g)| bg.tx.send(BgJob::Scan(id, s, g)).is_ok()); // STRIAPROXYWATCH (SR9079): no lease: the face's own road
                if ok {
                    bg.out += 1;
                    bg.scans.push(id);
                }
                ok
            }
        }
    }
    /// STRIATHUMBNAILSNEXT (SR997): a job is out on the worker.
    fn bg_busy(&self) -> bool {
        self.bg.as_ref().is_some_and(|b| b.out > 0)
    }
}

impl WavePeaks {
    /// STRIATHUMBNAILSNEXT (SR997): every finished job into the face's maps; `block`: wait until no job is out.
    fn drain_bg(&mut self, block: bool, reloaded: &[(timeline_core::SourceId, u64)]) { // STRIAPROXYWATCH (SR9079): `reloaded` judges each result's generation
        use std::sync::mpsc::TryRecvError;
        loop {
            let Some(bg) = self.thumbs.bg.as_mut() else { return };
            if bg.out == 0 {
                return;
            }
            let got = if block { bg.rx.recv().map_err(|_| TryRecvError::Disconnected) } else { bg.rx.try_recv() };
            match got {
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.bg_lost();
                    return;
                }
                Ok(BgDone::Head(k, s, n, g)) => {
                    bg.out = bg.out.saturating_sub(1);
                    bg.heads.retain(|h| *h != k); if stale_in(reloaded, Some(k.0), g) { self.thumbs.map.remove(&k); continue; } // STRIAPROXYWATCH (SR9079): a head of the media before a reload: dropped with its placeholder (the face asks again)
                    self.thumbs.decodes += n;
                    self.thumbs.insert(k, s);
                }
                Ok(BgDone::Scan(id, p, g)) => {
                    bg.out = bg.out.saturating_sub(1);
                    bg.scans.retain(|x| *x != id); if stale_in(reloaded, Some(id), g) { self.map.remove(&id); continue; } // STRIAPROXYWATCH (SR9079): a table of the media before a reload: dropped with its placeholder
                    self.map.insert(id, p);
                    self.scans += 1;
                }
            }
        }
    }
    /// STRIATHUMBNAILSNEXT (SR997): the worker is gone: drop it and its in-flight placeholders (the face's thread asks again).
    fn bg_lost(&mut self) {
        if let Some(bg) = self.thumbs.bg.take() {
            log::warn!("[nle-face] decode worker gone: {} jobs back on the face's thread", bg.out);
            for k in &bg.heads {
                self.thumbs.map.remove(k);
            }
            for id in &bg.scans {
                self.map.remove(id);
            }
        }
    }
}

impl Face {
    /// STRIATHUMBNAILSNEXT (SR997): decode heads and peak tables on one worker thread from now on (`serve` calls it once).
    pub fn decode_off_thread(&mut self) {
        if self.peaks.thumbs.bg.is_none() {
            self.peaks.thumbs.bg = Bg::start(); // STRIAPROXYWATCH (SR9079): the lease is taken at the next paint
        }
    }
    /// STRIATHUMBNAILSNEXT (SR997): wait until the worker holds no job, every result in the face's maps.
    pub fn bg_settle(&mut self) {
        self.peaks.drain_bg(true, &self.reloaded); // STRIAPROXYWATCH (SR9079)
    }
    /// STRIATHUMBNAILSNEXT (SR997): the face decodes off its thread.
    pub fn bg_running(&self) -> bool {
        self.peaks.thumbs.bg.is_some()
    }
}

// ---- STRIALOUDNESSNEXT (LEDGER SR995): the loudness range and true peak on the loudness line (named tail block) ----

/// STRIALOUDNESSNEXT (SR995): the finished measure's line: [`loudness_line`] of its LUFS with `, LRA <x.x> LU, true peak
/// <y.y> dBTP` right after `LUFS integrated` (before the ticks note and the floor's gate phrase); no audio and a
/// refusal read exactly as [`loudness_line`].
pub fn loudness_reading_line(r: &Result<Option<crate::nle::LoudnessReading>, RenderError>, range: &Option<std::ops::Range<i64>>) -> String {
    match r {
        Ok(Some(x)) => loudness_line(&Ok(Some(x.lufs)), range).replacen("LUFS integrated", &format!("LUFS integrated, LRA {:.1} LU, true peak {:.1} dBTP", x.range_lu, x.true_peak_db), 1),
        Ok(None) => loudness_line(&Ok(None), range),
        Err(e) => loudness_line(&Err(e.clone()), range),
    }
}

// ---- STRIAPANNEXT (LEDGER SR994): the face note — a success line after last_error, and the pan drag's reading (named tail block) ----
// `Face::note` holds a success line (SR897's pan reading `pan: R 25`, a committed pan drag's): `message_text` reads it after
// `last_error` and before the export / reel lines; `serve` and the witness do not take it (they take `last_error`), so it stays on
// the message row until the next taken input clears it: an `Input`, a `BrowserClick`, a button press, `BrowserSelectAll`, a
// named `BrowserKey` or a one-character `BrowserText` (a pointer move, a button release, a resize keep it).
impl Face {
    /// Clear the note on a taken input ([`Face::on`], after the `takes` check).
    fn clear_note(&mut self, msg: &SMessage) {
        let input = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserButton { down: true, .. } | SMessage::BrowserSelectAll => true,
            SMessage::BrowserKey(k) => k.chars().count() > 1,
            SMessage::BrowserText(t) => t.chars().count() == 1,
            _ => false,
        };
        if input {
            self.note = None;
        }
    }
    /// A pan drag ([`DragKind::Pan`], timeline_core's grip) that committed: the note reads the primary's pan ([`pan_line`]).
    fn pan_drag_note(&mut self, kind: DragKind, r: &Result<(), EditError>) {
        if kind == DragKind::Pan && r.is_ok() {
            self.note = Some(self.pan_reading()); // STRIAPANAUTOWRITE (SR9084): the pan shown at the playhead (file tail)
        }
    }
}

// ---- STRIAMARKERJUMPKEYSNEXT (LEDGER SR984): the marker style words' note and refusals; the face note (named tail block) ----
// `marker colour [<name>]` / `marker range` (the Marker Colour / Marker Range buttons) are timeline_core's `EditorState::verb`
// words (ONE commit each); the face words the result: a success is the face `note` (`marker M1 colour red at <tc>`,
// `marker M1 range <tc>..<tc>`, `marker M1 range cleared at <tc>`), a refusal is `last_error` (`marker colour: no marker at
// <tc>`, `marker colour: M1 is already red`, `marker colour: no colour <x> (orange, red, ...)`, `marker range: no marker
// at or before <tc>`, `marker range: M1 already ends at <tc>`). `Face.note` is read by `message_text` after `last_error`
// and cleared by `clear_note` on the next acting input (the rule SR980 / SR990 / SR991 wrote).

impl Face {
    /// Clear the note on an acting input: a typed word, a click, a press, select-all, a named key, a one-char text
    /// key. A move, a release, the wheel, a zoom and a resize keep it.
    fn clear_note(&mut self, msg: &SMessage) {
        let acts = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserButton { down: true, .. } | SMessage::BrowserSelectAll => true,
            SMessage::BrowserKey(k) => k.chars().count() > 1,
            SMessage::BrowserText(t) => t.chars().count() == 1,
            _ => false,
        };
        if acts {
            self.note = None;
        }
    }
    /// The playhead tick `t` as the message rows word it (`HH:MM:SS:FF`, the tick when the rate is not an integer fps).
    fn marker_tc(&self, t: i64) -> String {
        self.state.timeline.fps().map_or_else(|| t.to_string(), |f| timeline_core::edl::timecode(t, f))
    }
    /// The marker style words (`timeline_core::editor::marker_style_word`) through the one dispatch, with their note or
    /// refusal; `None` for any other word.
    fn marker_style_verb(&mut self, v: &str) -> Option<bool> {
        let ph = self.state.playhead;
        let Some(word) = timeline_core::editor::marker_style_word(v) else {
            let mut w = v.split_ascii_whitespace();
            if let (Some("marker"), Some("colour"), Some(x), None) = (w.next(), w.next(), w.next(), w.next()) {
                self.last_error = Some(format!("marker colour: no colour {x} ({})", timeline_core::MARKER_COLOURS.join(", ")));
                return Some(true);
            }
            return None;
        };
        let colour_name = |c: u8| timeline_core::MARKER_COLOURS.get(usize::from(c)).copied().unwrap_or("?");
        match word {
            timeline_core::editor::MarkerStyle::Colour(c) => {
                let before = self.state.timeline.marker_at(ph).map(|m| (m.name.clone(), m.colour));
                let r = self.state.verb(v).unwrap_or(Err(EditError::Marker));
                let now = self.state.timeline.marker_at(ph).map(|m| (m.name.clone(), m.colour));
                match (r, before, now) {
                    (Ok(()), _, Some((name, k))) => self.note = Some(format!("marker {name} colour {} at {}", colour_name(k), self.marker_tc(ph))),
                    (Err(EditError::Marker), None, _) => self.last_error = Some(format!("marker colour: no marker at {}", self.marker_tc(ph))),
                    (Err(EditError::Marker), Some((name, k)), _) if c == Some(k) => self.last_error = Some(format!("marker colour: {name} is already {}", colour_name(k))),
                    (r, _, _) => self.last_error = Some(format!("marker colour: {}", r.err().map_or_else(String::new, |e| e.to_string()))),
                }
            }
            timeline_core::editor::MarkerStyle::Range => {
                let before = self.state.timeline.marker_before(ph).map(|m| (m.name.clone(), m.at, m.len));
                let r = self.state.verb(v).unwrap_or(Err(EditError::Marker));
                match (r, before) {
                    (Ok(()), Some((name, at, _))) if ph == at => self.note = Some(format!("marker {name} range cleared at {}", self.marker_tc(at))),
                    (Ok(()), Some((name, at, _))) => self.note = Some(format!("marker {name} range {}..{}", self.marker_tc(at), self.marker_tc(ph))),
                    (Err(EditError::Marker), None) => self.last_error = Some(format!("marker range: no marker at or before {}", self.marker_tc(ph))),
                    (Err(EditError::Marker), Some((name, at, len))) => self.last_error = Some(format!("marker range: {name} already ends at {}", self.marker_tc(at + len))),
                    (r, _) => self.last_error = Some(format!("marker range: {}", r.err().map_or_else(String::new, |e| e.to_string()))),
                }
            }
        }
        Some(true)
    }
}

// ---- STRIATRACKGAINTHREE (LEDGER SR9024): the face note, mute / solo's success line, the gain line's track (named tail block) ----
// THE NOTE: `Face::note` is a success line (never an error): `message_text` reads it right after `last_error`, and
// `clear_note` drops it on the next acting input (`Input`, `BrowserClick`, a pressed `BrowserButton`, `BrowserSelectAll`, a
// named `BrowserKey`, a one-char `BrowserText`); a hover, scroll or resize keeps it. `mute` / `solo` (bare: the selected
// clip's track, SR837; `<ch>`: the named track, SR983) ride timeline_core's ONE verb dispatch; after an edit the note reads
// `mute: A2 on` / `solo: V1 off` (the track's flag after the toggle); a refusal stays `<the word as typed>: <EditError>` on
// `last_error`. The track gain words' line names the channel word's track when one is given, else the selection's.
impl Face {
    /// Drop the note on an acting input.
    fn clear_note(&mut self, msg: &SMessage) {
        let acting = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserSelectAll => true,
            SMessage::BrowserButton { down, .. } => *down,
            SMessage::BrowserKey(k) => k.chars().count() > 1,
            SMessage::BrowserText(t) => t.chars().count() == 1,
            _ => false,
        };
        if acting {
            self.note = None;
        }
    }
    /// The track a track word acts on: its second word's named track, else the selected clip's track.
    fn gain_track(&self, v: &str) -> Option<usize> {
        v.split_ascii_whitespace().nth(1).and_then(|ch| self.state.timeline.track_named(ch)).or_else(|| self.state.selection.map(|(t, _)| t))
    }
    /// The `mute` / `solo` words (bare and `<ch>`) and their note; `None` for any other word.
    fn track_flag_face_verb(&mut self, v: &str) -> Option<bool> {
        let word = v.split_ascii_whitespace().next().filter(|w| matches!(*w, "mute" | "solo"))?;
        let track = self.gain_track(v);
        match self.state.verb(v) {
            Some(Ok(())) => {
                let tl = &self.state.timeline;
                self.note = track.and_then(|t| tl.tracks.get(t).map(|tr| format!("{word}: {} {}", track_word(tl, t), if (if word == "solo" { tr.solo } else { tr.muted }) { "on" } else { "off" })));
            }
            Some(Err(e)) => self.last_error = Some(format!("{v}: {e}")),
            None => return None,
        }
        Some(true)
    }
}

// ---- STRIAUNDOTHREE (LEDGER SR9027): redo entries on the noted history line, picked by pointer (named tail block) ----
// The history words' note (SR973) carries the redo entries after the undo line: `history: 1 split || redo: r1 delete | r2
// slip+` (the top n redo entries, next first, from timeline_core's ONE name store through `EditorState::redo_history`);
// with an empty redo stack the note is the history line unchanged. `last_error` keeps SR876's line (serve's log, stria-
// view's errors). A press on an entry `r<q> <word>` runs SR934's one jump road, `redo <q>`: forward to after that entry,
// named on the message line; a press elsewhere in the redo part is taken and moves nothing. Owed: NLE-UNDO-PICK-SPLIT.

/// STRIAUNDOTHREE (SR9027): the separator between the undo line and the redo entries on the note.
const REDO_SEP: &str = " || redo: ";

impl Face {
    /// STRIAUNDOTHREE (SR9027): `line` (a history line) with the top `n` redo entries after [`REDO_SEP`]; `line` unchanged
    /// when there is nothing to redo.
    fn with_redo(&self, line: String, n: usize) -> String {
        let r = self.state.redo_history(n);
        if r.is_empty() {
            return line;
        }
        let items: Vec<String> = r.iter().map(|&(q, w)| format!("r{q} {}", w.unwrap_or("(unnamed)"))).collect();
        format!("{line}{REDO_SEP}{}", items.join(" | "))
    }
    /// STRIAUNDOTHREE (SR9027): byte `b` of the noted line `text` in its redo part: the entry under it redone to
    /// (`Some(true)`), or `Some(false)` off any entry; `None` when the line has no redo part or `b` is before it.
    fn redo_pick_at(&mut self, text: &str, b: usize) -> Option<bool> {
        let at = text.find(REDO_SEP)?;
        if b <= at {
            return None;
        }
        let depth = self.state.redo_depth();
        let Some(q) = redo_entry_at(text, at, b).filter(|q| (1..=depth).contains(q)) else { return Some(false) };
        self.note = None;
        self.verb(&format!("redo {q}"));
        Some(true)
    }
}

/// STRIAUNDOTHREE (SR9027): the redo count of the entry `r<q> <word>` whose bytes (its end included) hold byte `b` of
/// `line`, whose redo part starts at byte `at` ([`REDO_SEP`]); `None` on the separator or between entries.
fn redo_entry_at(line: &str, at: usize, b: usize) -> Option<usize> {
    let mut start = at + REDO_SEP.len();
    for item in line[start..].split(" | ") {
        let end = start + item.len();
        if (start..=end).contains(&b) {
            return item.split(' ').next()?.strip_prefix('r')?.parse().ok();
        }
        start = end + 3;
    }
    None
}

// ---- STRIAPROXYTHREE (LEDGER SR9026): a reel changed on disk reloads, its proxy follows; the face note (named tail block) ----
// `Face::note` (SR994's shape) holds a success line: `message_text` reads it after `last_error`; `clear_note` clears it on the
// next acting input. WATCH: at each acting input (`Face::on`, after `takes`) every reel path's SR882 `proxy_stamp` is compared
// with the stamp recorded at open (`watch_arm`, from `with_reel_paths`; a file already off the opened bytes' length records ""
// so the first acting input reloads it). A changed reel reopens through the ONE reopen, `Sources::add` (its old media stays on
// a refusal), only while no export / proxy job holds the shared `Arc<Sources>` (else the note says it waits and the next acting
// input retries); a proxy it carried is detached (`Sources::clear_proxy`: it is the old content's) and queued on SR915's
// `proxy_queue`, so SR882's one `proxy_start` rebuilds it. The timeline is untouched (Source.duration is the document's): no
// undo step, the document clean. Owed: NLE-PROXY-WATCH-LENGTH, NLE-PROXY-WATCH-IDLE, NLE-PROXY-WATCH-WORKER.

/// STRIAPROXYTHREE (LEDGER SR9026): an acting input (SR994's set): an `Input`, a `BrowserClick`, a button press,
/// `BrowserSelectAll`, a named `BrowserKey` or a one-character `BrowserText` (a pointer move, a release, a resize are not).
fn acting(msg: &SMessage) -> bool {
    match msg {
        SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserButton { down: true, .. } | SMessage::BrowserSelectAll => true,
        SMessage::BrowserKey(k) => k.chars().count() > 1,
        SMessage::BrowserText(t) => t.chars().count() == 1,
        _ => false,
    }
}

impl Face {
    /// Clear the note on a taken acting input ([`Face::on`], after the `takes` check).
    fn clear_note(&mut self, msg: &SMessage) {
        if acting(msg) {
            self.note = None;
        }
    }
    /// Record each reel file's stamp as opened; a file whose length is not the opened bytes' records "" (reloads at once).
    fn watch_arm(&mut self) {
        let mut watched = Vec::new();
        for (reel, path) in &self.reel_paths {
            let src = self.state.timeline.source_by_reel(reel).map(|s| s.id);
            let same = src.is_some_and(|s| std::fs::metadata(path).map(|m| m.len()).ok() == self.sources.byte_len(s));
            watched.push((reel.clone(), if same { proxy_stamp(path).unwrap_or_default() } else { String::new() }));
        }
        self.watched = watched;
    }
    /// On an acting input: reload every reel whose file stamp moved since it was recorded. The note reads
    /// `watch: A: reloaded <n> bytes[, proxy rebuilds]; ..` (or `watch: A: changed on disk; reloads after the running job`);
    /// a refusal is `watch: A: <why>` on `last_error`; a queued rebuild runs SR915's `proxy_next`.
    fn watch_sources(&mut self, msg: &SMessage) {
        if !acting(msg) || self.reel_paths.is_empty() {
            return;
        }
        let mut changed = Vec::new();
        for (reel, path) in &self.reel_paths {
            let Some(now) = proxy_stamp(path) else { continue };
            let Some(src) = self.state.timeline.source_by_reel(reel).map(|s| s.id) else { continue };
            if self.watched.iter().find(|(r, _)| r == reel).map(|(_, s)| s.as_str()) != Some(now.as_str()) {
                changed.push((src, reel.clone(), path.clone(), now));
            }
        }
        if changed.is_empty() {
            return;
        }
        self.bg_release(); let Some(sources) = Arc::get_mut(&mut self.sources) else { // STRIAPROXYWATCH (SR9079): the decode worker's parked lease released first (file tail)
            let reels: Vec<String> = changed.into_iter().map(|(_, r, _, _)| r).collect();
            self.note = Some(format!("watch: {}: changed on disk; reloads after the running job", reels.join(", ")));
            return;
        };
        let (mut parts, mut refused, mut queue) = (Vec::new(), Vec::new(), Vec::new());
        for (src, reel, path, now) in changed {
            let r = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display())).and_then(|b| {
                let n = b.len();
                sources.add(src, b).map(|()| n).map_err(|e| e.to_string())
            });
            match r {
                Ok(n) => { self.sources_gen += 1; self.reloaded.retain(|(s, _)| *s != src); self.reloaded.push((src, self.sources_gen)); // STRIAPROXYWATCH (SR9079): the one generation moves (file tail)
                    let follow = sources.clear_proxy(src);
                    if follow {
                        queue.push((src, reel.clone()));
                    }
                    parts.push(format!("{reel}: reloaded {n} bytes{}", if follow { ", proxy rebuilds" } else { "" }));
                }
                Err(e) => refused.push(format!("watch: {reel}: {e}")),
            }
            self.watched.retain(|(r, _)| *r != reel);
            self.watched.push((reel, now));
        }
        self.trans_still = None; self.cache_sweep(); // STRIAPROXYSCAN (SR9116): heads, tiles and tables of the reloaded reels dropped (file tail)
        self.meters.reset();
        if !parts.is_empty() {
            self.note = Some(format!("watch: {}", parts.join("; ")));
        }
        if !refused.is_empty() {
            let line = refused.join("; ");
            self.last_error = Some(match self.last_error.take() {
                Some(l) => format!("{l}; {line}"),
                None => line,
            });
        }
        if !queue.is_empty() {
            self.proxy_queue.extend(queue);
            self.proxy_next();
        }
    }
}

// ---- STRIABINTHREE (LEDGER SR9025): the bin's thumb drag and row range drag (named tail block) ----
// A press on the bin (a Surface that sends `BrowserButton`) starts ONE drag, [`BinGrab`]: on SR976's thumb (its column, only
// when the rows overflow) the moves set the first shown row through timeline_core view's `bin_thumb_top` (the thumb's own
// geometry inverted, clamped by SR939's `bin_clamp`) and the release lays nothing; on a row the moves show a range bar
// (view's `paint_bin_range`) and a release at least `view::BIN_RANGE_MIN` px away lays the source range `view::bin_range`
// through [`Face::bin_lay`], the ONE lay road the click also takes (an unused reel by its media's kinds, else SR852's
// `insert <reel> <in> <out>` through `Face::verb`: ONE commit, one undo step). A shorter drag is the click (the whole reel).
// No grab paints nothing; a Surface that sends only clicks keeps the click.

/// The bin press's drag: the thumb (`off` px below the thumb's top) or a range along `row` from `x0` to `x`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinGrab {
    Thumb { off: i64 },
    Range { row: usize, x0: i64, x: i64 },
}

impl Face {
    /// A press on the bin at `(x, y)`: in the thumb column of an overflowing bin, grab the thumb (off it: jump so it centres
    /// on `y`); on a row, arm its range. True only when the first shown row moved.
    fn bin_press(&mut self, x: f64, y: f64) -> bool {
        let Some(s) = self.split else { return false };
        let Some(b) = s.bin else { return false };
        let (w, h) = b.canvas;
        let tl = &self.state.timeline;
        let col = x >= (w as i64 - view::BIN_THUMB_W) as f64 && x < w as f64;
        let (grab, top) = match view::bin_thumb(tl, h, b.top).filter(|_| col) {
            Some((y0, y1)) => {
                let off = if (y as i64) >= y0 && (y as i64) < y1 { y as i64 - y0 } else { (y1 - y0) / 2 };
                (BinGrab::Thumb { off }, view::bin_thumb_top(tl, h, y - off as f64))
            }
            None => match view::bin_hit_from(tl, w, h, b.top, x, y) {
                Some(row) => (BinGrab::Range { row, x0: x as i64, x: x as i64 }, b.top),
                None => return false,
            },
        };
        let moved = top != view::bin_clamp(tl, h, b.top);
        self.split = Some(Split { bin: Some(BinSurface { grab: Some(grab), top, ..b }), ..s });
        moved
    }
    /// A move over the bin: the thumb drag sets the first shown row, the range drag its end x. True when either changed.
    fn bin_move(&mut self, x: f64, y: f64) -> bool {
        let Some(s) = self.split else { return false };
        let Some(b) = s.bin else { return false };
        let Some(g) = b.grab else { return false };
        let h = b.canvas.1;
        let tl = &self.state.timeline;
        let (grab, top) = match g {
            BinGrab::Thumb { off } => (g, view::bin_thumb_top(tl, h, y - off as f64)),
            BinGrab::Range { row, x0, .. } => (BinGrab::Range { row, x0, x: x as i64 }, b.top),
        };
        let changed = top != view::bin_clamp(tl, h, b.top) || grab != g;
        self.split = Some(Split { bin: Some(BinSurface { grab: Some(grab), top, ..b }), ..s });
        changed
    }
    /// The release on the bin: the drag ends. A thumb drag lays nothing (true: the grab's paint goes); a range drag of at
    /// least `view::BIN_RANGE_MIN` px lays its row's range; anything else is the click at `(x, y)`.
    fn bin_release(&mut self, x: f64, y: f64) -> bool {
        let Some(s) = self.split else { return false };
        let Some(b) = s.bin else { return false };
        self.split = Some(Split { bin: Some(BinSurface { grab: None, ..b }), ..s });
        match b.grab {
            Some(BinGrab::Thumb { .. }) => true,
            Some(BinGrab::Range { row, x0, .. }) if (x as i64 - x0).abs() >= view::BIN_RANGE_MIN => match view::bin_range(&self.state.timeline, b.canvas.0, row, x0 as f64, x) {
                Some(range) => self.bin_lay(row, Some(range)),
                None => self.bin_click(x, y),
            },
            _ => self.bin_click(x, y),
        }
    }
    /// THE bin lay: source row `i`'s range (`None`: the whole reel) — an unused reel on the first track of every kind its
    /// media holds ([`Face::bin_unused`]), else the word `view::bin_range_word` through [`Face::verb`]. One commit either way.
    fn bin_lay(&mut self, i: usize, range: Option<(i64, i64)>) -> bool {
        if let Some(r) = self.bin_unused(i, range) {
            return r;
        }
        let Some(word) = view::bin_range_word(&self.state.timeline, i, range) else { return false };
        self.verb(&word)
    }
    /// `px` with the range bar of a live row drag of at least `view::BIN_RANGE_MIN` px; unchanged otherwise.
    fn bin_range_paint(&self, px: Vec<u8>) -> Vec<u8> {
        match self.bin() {
            Some(b) => match b.grab {
                Some(BinGrab::Range { row, x0, x }) if (x - x0).abs() >= view::BIN_RANGE_MIN => view::paint_bin_range(&self.state.timeline, px, b.canvas, b.top, vo(self.order), row, (x0, x)),
                _ => px,
            },
            None => px,
        }
    }
}

#[cfg(test)]
mod striabinthree_tests {
    use super::*;
    use timeline_core::{Mode, SourceId};

    fn button(down: bool, (x, y): (f64, f64)) -> SMessage {
        SMessage::BrowserButton { down, x, y, mods: 0, surface: 2 }
    }

    fn mv((x, y): (f64, f64)) -> SMessage {
        SMessage::BrowserMouseMove(x, y, 2)
    }

    /// The fixture (A, B on V1 / A1) plus reel C (synth media: video and audio) on no track, the bin on Surface 2.
    fn with_unused() -> (Face, SourceId) {
        let (mut tl, mut src) = synth_fixture();
        let c = tl.add_source("C", 50);
        src.add(c, synth_source(2000, 50, 25)).expect("synthetic source opens");
        (Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).on_surfaces(0, 1).with_bin(2), c)
    }

    /// On a one-row bin a press on the thumb and a move down scroll the rows through `bin_thumb_top`; the release lays
    /// nothing; a press in the column of a bin whose rows fit is a row's press.
    #[test]
    fn striabinthree_thumb_drag_scrolls_and_lays_nothing() {
        let (mut f, _) = with_unused();
        assert!(f.on(&SMessage::BrowserResize(320, view::BIN_ROW_H, 2)));
        let tl0 = f.state.timeline.clone();
        let h = view::BIN_ROW_H;
        let (y0, _) = view::bin_thumb(&f.state.timeline, h, 0).unwrap();
        f.on(&button(true, (319.0, y0 as f64 + 1.0)));
        assert_eq!(f.bin().unwrap().top, 0, "a press on the thumb itself does not move it");
        assert!(f.on(&mv((319.0, y0 as f64 + 1.0 + 12.0))));
        let want = view::bin_thumb_top(&f.state.timeline, h, y0 as f64 + 12.0);
        assert_eq!(f.bin().unwrap().top, want);
        assert!(want > 0);
        assert!(f.on(&button(false, (319.0, y0 as f64 + 13.0))));
        assert_eq!((&f.state.timeline, f.state.undo_depth()), (&tl0, 0), "the thumb lays nothing");
        assert!(!f.on(&mv((319.0, 2.0))), "no grab after the release");
        assert_eq!(f.bin().unwrap().top, want);
    }

    /// A drag along row B from the strip's start to its middle lays `insert B 0 25` at the playhead on V1: one undo step;
    /// the range bar paints only while the drag shows.
    #[test]
    fn striabinthree_row_drag_lays_the_range() {
        let (mut f, _) = with_unused();
        let tl0 = f.state.timeline.clone();
        f.state.deselect();
        f.state.seek(15);
        let y = view::bin_point(1).1;
        let rest = f.paint_role(Role::Bin);
        f.on(&button(true, (20.0, y)));
        assert_eq!(f.paint_role(Role::Bin), rest, "a press alone paints nothing");
        assert!(f.on(&mv((168.0, y))));
        let (w, h) = f.bin().unwrap().canvas;
        assert_eq!(f.paint_role(Role::Bin), view::paint_bin_range(&f.state.timeline, rest.clone(), (w, h), 0, Order::Rgba, 1, (20, 168)));
        assert_eq!(view::bin_range(&f.state.timeline, w, 1, 20.0, 168.0), Some((0, 25)));
        assert!(f.on(&button(false, (168.0, y))));
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        assert_eq!(f.state.timeline, tl0.place(0, Clip::new(SourceId(1), 0, 25, 15), Mode::Insert).unwrap());
        assert_eq!(f.state.undo_depth(), 1);
        assert_eq!(f.bin().unwrap().grab, None);
        assert!(f.verb("undo"));
        assert_eq!(f.state.timeline, tl0);
    }

    /// A drag shorter than `BIN_RANGE_MIN` is the click: the whole reel.
    #[test]
    fn striabinthree_short_drag_is_the_click() {
        let (mut f, _) = with_unused();
        let tl0 = f.state.timeline.clone();
        f.state.deselect();
        f.state.seek(15);
        let y = view::bin_point(1).1;
        f.on(&button(true, (40.0, y)));
        f.on(&mv((42.0, y)));
        assert!(f.on(&button(false, (42.0, y))));
        assert_eq!(f.state.timeline, tl0.place(0, Clip::new(SourceId(1), 0, 50, 15), Mode::Insert).unwrap());
        assert_eq!(f.state.undo_depth(), 1);
    }

    /// A range drag on the unused AV reel C lays that range on V1 and A1 in ONE commit.
    #[test]
    fn striabinthree_unused_reel_range_lays_video_and_audio() {
        let (mut f, c) = with_unused();
        let tl0 = f.state.timeline.clone();
        f.state.deselect();
        f.state.seek(15);
        let y = view::bin_point(2).1;
        f.on(&button(true, (168.0, y)));
        f.on(&mv((316.0, y)));
        assert!(f.on(&button(false, (316.0, y))));
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        let laid = Clip::new(c, 25, 50, 15);
        assert_eq!(f.state.timeline, tl0.place(0, laid, Mode::Insert).unwrap().place(1, laid, Mode::Insert).unwrap());
        assert_eq!(f.state.undo_depth(), 1);
    }
}

// ---- STRIANORMALIZETHREE (LEDGER SR9035): normalize at the mix's rate, its scan off the face's thread (named tail block) ----
// The peak is measured at the rate the mix plays the source (nle.rs `peak_mix`: the project rate `with_audio` sets, so an
// overshoot the mix's resample builds is seen), through SR963's one peak body. An unserved face (the tests, the stria-view
// witness) scans synchronously; `serve` sets `scan.thread`, and the scan runs on its own thread over an `Arc` clone of the
// sources (no lock held across the decode), `normalize_tick` on the clock joins it and sets the gain only if the
// selection still stands (same track, index and clip). Every line rides the parent's road, `last_error`.

/// STRIANORMALIZETHREE (SR9035): the line while a served face's scan runs (`normalize: measuring`).
pub const NORMALIZE_SCANNING: &str = "measuring";

/// STRIANORMALIZETHREE (SR9035): the face's normalize scan: `thread` false (the default) measures on the caller's thread.
#[derive(Default)]
pub struct NormalizeScan {
    pub thread: bool,
    job: Option<NormalizeJob>,
}

/// STRIANORMALIZETHREE (SR9035): one running scan: the clip it measures (re-checked at the join) and its target.
struct NormalizeJob {
    track: usize,
    index: usize,
    clip: Clip,
    target: i32,
    os: u32, lufs: Option<LufsRun>, // STRIANORMALIZELUFSMORE (SR9086): the clip measure's watches and the selection it measures (file tail)
    handle: std::thread::JoinHandle<Result<Option<NormalizeRead>, String>>, // STRIANORMALIZELUFS (SR9054): a peak or a loudness reading (file tail)
}

impl Face {
    /// The peak of the selected clip `c` (track `track`) over source time `ns` at the mix's rate: `Ok(Some)` measured
    /// here, `Ok(None)` a scan started on its own thread (served face), `Err` a refusal by name (`busy`, `no audio`, the decode error).
    fn normalize_peak(&mut self, track: usize, c: Clip, ns: std::ops::Range<i64>, target: i32, os: u32) -> Result<Option<f32>, String> {
        if !self.scan.thread {
            return crate::nle::peak_mix(&self.sources, c.src, ns, os).map_err(|e| e.to_string())?.ok_or_else(|| "no audio".to_string()).map(Some);
        }
        if self.scan.job.is_some() {
            return Err("busy".to_string());
        }
        let index = self.state.selection.map_or(0, |(_, i)| i);
        let sources = self.sources.clone();
        let handle = std::thread::spawn(move || crate::nle::peak_mix(&sources, c.src, ns, os).map(|p| p.map(NormalizeRead::Peak)).map_err(|e| e.to_string())); // STRIANORMALIZELUFS (SR9054): the peak as a NormalizeRead (file tail)
        self.scan.job = Some(NormalizeJob { track, index, clip: c, target, os, lufs: None, handle }); // STRIANORMALIZELUFSMORE (SR9086): a one-clip peak scan carries no run (file tail)
        Ok(None)
    }
    /// The gain from a measured `peak` set through SR838's one road, and its line (SR878's rule, moved here).
    fn normalize_finish(&mut self, peak: f32, target: i32, os: u32) -> Result<String, String> {
        let cdb = normalize_gain_cdb(peak, target).ok_or_else(|| "silent clip".to_string())?;
        self.state.set_selected_gain(cdb).map_err(|e| e.to_string())?;
        let peak_cdb = (2000.0 * (peak as f64).log10()).round() as i32;
        Ok(format!("{} {} -> gain {}", if os > 1 { "true peak" } else { "peak" }, db_words(peak_cdb), db_words(cdb)))
    }
    /// True while a scan's result has not been reported (the bus face's clock runs).
    pub fn normalizing(&self) -> bool {
        self.scan.job.is_some()
    }
    /// One clock tick for the scan: a finished scan is joined and its gain set if the selection still stands (else
    /// `the selection moved`, the timeline unmoved); the line goes to the message row. True when that happened (repaint).
    pub fn normalize_tick(&mut self) -> bool {
        if !self.scan.job.as_ref().is_some_and(|j| j.handle.is_finished()) {
            return false;
        }
        let Some(j) = self.scan.job.take() else { return false };
        let got = j.handle.join().unwrap_or_else(|_| Err("scan failed".to_string()));
        let stands = self.state.selection == Some((j.track, j.index)) && self.state.selected() == Some(j.clip);
        let r = match got {
            _ if j.lufs.as_ref().is_some_and(LufsRun::cancelled) => Err("cancelled".to_string()), Err(e) => Err(e), // STRIANORMALIZELUFSMORE (SR9086): the job's own flag names the cancel, the timeline unmoved (file tail)
            Ok(None) => Err("no audio".to_string()),
            Ok(Some(_)) if !stands => Err("the selection moved".to_string()),
            Ok(Some(NormalizeRead::Peak(peak))) => self.normalize_finish(peak, j.target, j.os), Ok(Some(NormalizeRead::Lufs(l))) => { let r = self.normalize_lufs_finish(l, j.target); return self.normalize_lufs_say(r); } Ok(Some(NormalizeRead::LufsMany(rs))) => { let r = self.normalize_lufs_many_finish(j.lufs.as_ref(), rs, j.target); return self.normalize_lufs_say(r); } Ok(Some(NormalizeRead::PeakMany(ps))) => self.normalize_peak_many_finish(j.lufs.as_ref(), ps, j.target, j.os), // STRIANORMALIZELUFSMORE (SR9086): a selection's readings set every gain in one commit (file tail); STRIANORMALIZELUFS (SR9054): a loudness reading sets the gain through its own finish (file tail)
        };
        self.last_error = Some(match r {
            Ok(line) | Err(line) => format!("normalize: {line}"),
        });
        true
    }
}

// ---- STRIAEXPORTQUEUETHREE (LEDGER SR9042): GTK's Shift+Escape keys export-skip (named tail block) ----
// GTK names Shift+Esc `Shift+Escape` (a BrowserKey, no BrowserText); one KEYS row keys SR914's `export-skip` word through
// `Face::key` -> `Face::verb` -> `export_queue_verb`, so the queue is kept and the next request starts. Esc stays cancel-all
// (`cancel_export`); AppKit drops Escape, `E` is its key. The help row (`help_rows`) reads `E / Shift+Escape`. No new fn.

// ---- STRIARIPPLETHREE (LEDGER SR9034): the ripple-all tool — a face mode that makes a ripple drag run on every track (named tail block) ----
// Every modifier on the selected clip's edge zone is taken (SR850), so the all-tracks ripple drag (NLE-RIPPLE-ALL-DRAG) is
// a face TOOL in the roll tool's shape: `ripple-all` (VERBS, the Ripple All button, the key `A`) turns it on / off, a
// view-only toggle (no undo step, the document stays clean). While it is on, `press` maps the begun drag through
// timeline_core's `Drag::synced`: a shift-drag on the selected clip's trim handle (SR850's ripple) commits as SR953's ONE
// `EditorState::ripple_trim_all` (every track's twin trims, every track's later material follows, one undo step); every
// other drag is unchanged. `message_text` names the tool after the export line. Default off: every golden is unchanged.
// Owed: NLE-RIPPLE-ALL-KERNEL (the Player's tool needs a new EDIT_KEYS byte), NLE-RIPPLE-ALL-PAINT (a cue on the band).

/// STRIARIPPLETHREE (SR9034): the message line while the ripple-all tool is on.
pub const RIPPLE_ALL_NOTE: &str = "ripple all: a shift-drag on a clip edge ripples every track";

impl Face {
    /// STRIARIPPLETHREE: the ripple-all tool is on.
    pub fn ripple_all(&self) -> bool {
        self.ripple_all
    }
    /// The `ripple-all` word of [`Face::verb`]: the tool on / off (no commit); `None`: not this word.
    fn ripple_all_verb(&mut self, v: &str) -> Option<bool> {
        if v != "ripple-all" {
            return None;
        }
        self.ripple_all = !self.ripple_all;
        Some(true)
    }
    /// The tool's line for [`Face::message_text`] while it is on.
    fn ripple_all_note(&self) -> Option<String> {
        self.ripple_all.then(|| RIPPLE_ALL_NOTE.to_string())
    }
}

// ---- STRIASLIPVIEWTHREE (LEDGER SR9036): a slide's four cells, each cell's source timecode (named tail block) ----
// Two additions to SR858's ONE pair (`Face::slip_refresh`: its render through `preview`, its cache, its throttle and the
// serve clock unchanged). A slide (a Ctrl / Cmd Slide drag or a held `slide` word) shows FOUR cells: the butted previous
// clip's new last frame | the slid clip's own first | its own last | the butted next clip's new first (a side with no
// butted neighbour FIELD; neither butted: no pair, SR946's rule), composed by SR858's `slip_split` nested twice. Each cell
// carries the SOURCE timecode of the frame it shows (`edl::timecode` of the cell clip's `src_in + t - at`), painted with
// labels on at the cell's foot inside the letterbox rect `paint_in` uses (THE geometry `SizePolicy::Letterbox.place`), so
// the text keeps its size whatever the pair's scale; with labels off not a pixel changes. Owed: NLE3E-SLIP-VIEW-ASYNC,
// NLE3E-SLIP-VIEW-KERNEL.

/// STRIASLIPVIEWTHREE (SR9036): the cells SR858's loop renders. A slip: SR946's two cells as they are. A slide: the
/// previous clip's cell, the slid clip's own first (`at`) and last (`end - 1`) frame, the next clip's cell.
fn slipthree_cells(tl: &Timeline, track: usize, idx: usize, slide: bool, two: [Option<(Timeline, i64)>; 2]) -> Vec<Option<(Timeline, i64)>> {
    let [prev, next] = two;
    if !slide {
        return vec![prev, next];
    }
    match slip_clip_timeline(tl, track, idx) {
        Some((own, c)) => vec![prev, Some((own.clone(), c.at)), Some((own, c.end() - 1)), next],
        None => vec![prev, next],
    }
}

/// STRIASLIPVIEWTHREE (SR9036): the cells side by side through SR858's ONE [`slip_split`]: two cells as SR858 did; four
/// as two halves (each a `slip_split` pair) split again, so each half lands 1:1 in its outer cell.
fn slipthree_split(cells: &[Option<Still>]) -> Option<Still> {
    let at = |i: usize| cells.get(i).and_then(|c| c.as_ref());
    if cells.len() == 4 {
        let (l, r) = (slip_split(at(0), at(1)), slip_split(at(2), at(3)));
        return slip_split(l.as_ref(), r.as_ref());
    }
    slip_split(at(0), at(1))
}

/// STRIASLIPVIEWTHREE (SR9036): each cell's source timecode: the cell clip's source tick at the shown tick
/// (`src_in + t - at`) as `HH:MM:SS:FF` at the timeline's fps (25 when it has none); a FIELD cell `None`.
fn slipthree_tcs(cells: &[Option<(Timeline, i64)>]) -> Vec<Option<String>> {
    cells
        .iter()
        .map(|c| {
            c.as_ref().and_then(|(one, t)| {
                let clip = one.tracks.first()?.clips.first()?;
                Some(timeline_core::edl::timecode(clip.src_in + (*t - clip.at), one.fps().unwrap_or(25)))
            })
        })
        .collect()
}

/// STRIASLIPVIEWTHREE (SR9036): where each of `n` cells' timecode goes when a `sw` x `sh` composite is letterboxed into the
/// `w` x `h` preview band (THE geometry `SizePolicy::Letterbox.place`, the rect `paint_in`'s `blit_fit` fills): per cell
/// `(x, baseline, clip)` = the cell's left + 4, the rect's foot - 4, the cell's columns. Any size 0 or no cell: none.
pub fn slip_tc_spots(sw: u32, sh: u32, w: u32, h: u32, n: usize) -> Vec<(f32, f32, (i32, i32))> {
    if sw == 0 || sh == 0 || w == 0 || h == 0 || n == 0 {
        return Vec::new();
    }
    let d = timeline_core::SizePolicy::Letterbox.place(sw as usize, sh as usize, w as usize, h as usize).dst;
    (0..n)
        .map(|i| {
            let (x0, x1) = (d.x + i * d.w / n, d.x + (i + 1) * d.w / n);
            (x0 as f32 + 4.0, (d.y + d.h) as f32 - 4.0, (x0 as i32, x1 as i32))
        })
        .collect()
}

impl Face {
    /// STRIASLIPVIEWTHREE (SR9036): the cells' timecodes over a painted canvas (one line per cell at [`slip_tc_spots`],
    /// through [`Labels`]); unchanged with labels off, on the tracks Surface, or with no pair showing.
    fn slipthree_paint_tc(&self, px: Vec<u8>, l: Layout, role: Role) -> Vec<u8> {
        let Some(style) = self.labels.as_ref() else { return px };
        if role == Role::Tracks {
            return px;
        }
        let Some(v) = self.slip_view.as_ref() else { return px };
        let Some(s) = v.still.as_ref() else { return px };
        let spots = slip_tc_spots(s.width, s.height, l.w, l.preview_h, v.tcs.len());
        let mut c = Canvas { w: l.w, h: l.h, order: vo(self.order), px };
        for (tc, (x, baseline, clip)) in v.tcs.iter().zip(spots) {
            if let Some(tc) = tc {
                view::Text::text(&mut Labels(style), &mut c, tc, x, baseline, clip);
            }
        }
        c.px
    }
    /// STRIASLIPVIEWTHREE (SR9036): the shown pair's cells' source timecodes, left to right (a FIELD cell `None`).
    pub fn slip_timecodes(&self) -> Option<&[Option<String>]> {
        self.slip_view.as_ref().filter(|v| v.still.is_some()).map(|v| v.tcs.as_slice())
    }
}

#[cfg(test)]
mod striaslipviewthree_tests {
    use super::*;

    /// A `w` x `h` still of one byte value.
    fn px(w: u32, h: u32, v: u8) -> Still {
        Still { width: w, height: h, rgba: vec![v; (w * h * 4) as usize] }
    }

    /// Rule: a slip keeps SR946's two cells; a slide's four are [previous clip's last, own first, own last, next's first].
    #[test]
    fn striaslipviewthree_cells_kats() {
        let (tl, _src) = synth_fixture();
        let (a, b) = (tl.tracks[0].clips[0], tl.tracks[0].clips[1]);
        let tick = |c: &Option<(Timeline, i64)>| c.as_ref().map(|(one, t)| (one.tracks[0].clips[0], *t));
        let s = slipthree_cells(&tl, 0, 1, false, slip_cells(&tl, 0, 1, false).unwrap());
        assert_eq!(s.iter().map(tick).collect::<Vec<_>>(), vec![Some((b, 10)), Some((b, 19))]);
        let s = slipthree_cells(&tl, 0, 1, true, slip_cells(&tl, 0, 1, true).unwrap());
        assert_eq!(s.iter().map(tick).collect::<Vec<_>>(), vec![Some((a, 9)), Some((b, 10)), Some((b, 19)), None]);
        assert_eq!(slipthree_tcs(&s), vec![Some("00:00:00:22".to_string()), Some("00:00:00:00".to_string()), Some("00:00:00:09".to_string()), None]);
    }

    /// Rule: four cells land as exact quarters (each half 1:1 in its outer cell); a missing cell is FIELD; two cells are
    /// SR858's `slip_split`.
    #[test]
    fn striaslipviewthree_split_kats() {
        let s = slipthree_split(&[Some(px(2, 1, 1)), Some(px(2, 1, 2)), Some(px(2, 1, 3)), None]).unwrap();
        assert_eq!((s.width, s.height), (8, 1));
        let mut want = [vec![1u8; 8], vec![2u8; 8], vec![3u8; 8]].concat();
        want.extend(FIELD.repeat(2));
        assert_eq!(s.rgba, want);
        assert_eq!(slipthree_split(&[Some(px(2, 1, 1)), Some(px(2, 1, 2))]), slip_split(Some(&px(2, 1, 1)), Some(&px(2, 1, 2))));
        assert_eq!(slipthree_split(&[None, None]), None);
    }

    /// Rule: the spots sit at each cell's left + 4 and the letterbox rect's foot - 4, clipped to the cell's columns.
    #[test]
    fn striaslipviewthree_tc_spots_kats() {
        assert_eq!(slip_tc_spots(160, 50, 320, 100, 2), vec![(4.0, 96.0, (0, 160)), (164.0, 96.0, (160, 320))]);
        assert_eq!(slip_tc_spots(160, 50, 320, 100, 4).iter().map(|s| s.0).collect::<Vec<_>>(), vec![4.0, 84.0, 164.0, 244.0]);
        assert_eq!(slip_tc_spots(100, 100, 320, 100, 1), vec![(114.0, 96.0, (110, 210))], "pillarbox");
        assert_eq!(slip_tc_spots(320, 50, 320, 100, 1), vec![(4.0, 71.0, (0, 320))], "letterbox");
        assert!(slip_tc_spots(0, 50, 320, 100, 2).is_empty() && slip_tc_spots(160, 50, 320, 100, 0).is_empty());
    }

    /// Rule: with labels off the timecodes paint nothing (byte-equal without them); with labels on they change pixels
    /// only inside the pair's letterbox rect of the preview band.
    #[test]
    fn striaslipviewthree_timecodes_paint_only_with_labels() {
        for labels in [false, true] {
            let (tl, src) = synth_fixture();
            let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, labels);
            face.state.select_at(0, 12).unwrap();
            assert!(face.verb("slip+"));
            let with = face.paint();
            assert!(face.slip_timecodes().is_some_and(|t| t.len() == 2 && t.iter().all(|c| c.is_some())));
            let (sw, sh) = face.slip_view().map(|s| (s.width, s.height)).unwrap();
            face.slip_view.as_mut().unwrap().tcs = vec![None, None];
            let without = face.paint();
            if !labels {
                assert_eq!(with, without, "labels off: not a pixel");
                continue;
            }
            let l = face.layout_for(Role::Whole).with_message(true);
            let d = timeline_core::SizePolicy::Letterbox.place(sw as usize, sh as usize, l.w as usize, l.preview_h as usize).dst;
            let diff: Vec<(usize, usize)> = (0..with.len() / 4).filter(|&i| with[i * 4..i * 4 + 4] != without[i * 4..i * 4 + 4]).map(|i| (i % l.w as usize, i / l.w as usize)).collect();
            assert!(!diff.is_empty(), "labels on: the timecodes paint");
            assert!(diff.iter().all(|&(x, y)| x >= d.x && x < d.x + d.w && y >= d.y && y < d.y + d.h), "only inside the pair's rect");
        }
    }
}

// ---- STRIATCTHREE (LEDGER SR9030): the play clock reads an NTSC time base (named tail block) ----
// `serve`'s clock stepped a 30000/1001 timeline (now read from a drop-frame EDL) at the fallback 25 fps. An NTSC base
// `1001/(k*1000)` ticks every `tb.to_ns(1)`; every other base is `1_000_000_000 / fps` as before (byte-identical).

/// STRIATCTHREE (SR9030): the face clock's period (ns) for time base `tb`; `fps` is the rate for any base that is not NTSC.
pub fn clock_ns(tb: timeline_core::Timebase, fps: u32) -> u64 {
    if timeline_core::edl::ntsc_nominal(tb).is_some() { tb.to_ns(1).max(1) as u64 } else { 1_000_000_000 / fps.max(1) as u64 }
}

#[cfg(test)]
mod striatcthree_tests {
    use super::*;
    use timeline_core::Timebase;

    /// STRIATCTHREE (SR9030) M3: 30000/1001 ticks every 33_366_667 ns, 60000/1001 every 16_683_333 ns; a 1/n base is today's
    /// `1_000_000_000 / fps`.
    #[test]
    fn striatcthree_clock_reads_ntsc() {
        assert_eq!(clock_ns(Timebase { num: 1001, den: 30000 }, 25), 33_366_667);
        assert_eq!(clock_ns(Timebase { num: 1001, den: 60000 }, 25), 16_683_333);
        assert_eq!(clock_ns(Timebase { num: 1, den: 25 }, 25), 40_000_000);
        assert_eq!(clock_ns(Timebase { num: 1, den: 30 }, 30), 1_000_000_000 / 30);
    }
}

// ---- STRIAPERFTHREE (LEDGER SR9037): the painted preview band kept by still (named tail block) ----
// SR890 measured `paint_in` filling the preview band FIELD and nearest-scaling the still into it per output pixel on every
// paint. Inside the one painter nothing else paints rows `0..preview_h` (the ruler, markers, rows and playhead start at
// `ruler_y >= preview_h`; label glyphs rise at most 9 / 15 px over baselines `ry + 9` / `y0 + 15`), so the band's bytes are a
// function of (the still, the width, min(preview_h, h), the byte order) alone. SR918's `PreviewCache` now keeps those bytes
// from the paint that made them, keyed on the still's VALUE (the fold's slip pair and scrub stills are other values, so a
// generation key would go stale); a repeat paint paints without the still and copies the kept band back: the same bytes,
// no blit re-implemented. Bypassed while playing or dragging (a new still every paint). Overlays painted after `paint_in`
// returns (the message row, meters, help, the siblings' outlines) stay outside `band`. Owed: NLE-PERF-DAMAGE.

/// STRIAPERFTHREE (SR9037): the band's key (width, rows, byte order, the still), its bytes, and the hits made.
struct BandPx {
    w: u32,
    rows: u32,
    order: Order,
    still: Still,
    px: Vec<u8>,
    hits: u64,
}

impl PreviewCache {
    /// THE paint `paint` (timeline_core's `paint_in` with the face's labels) with the band rows `0..min(preview_h, h)` kept
    /// by `still`: a hit paints with no still and copies the kept rows back; a miss paints with the still and keeps its rows.
    /// No still, `moving` (playing / a drag) or an empty band: `paint(still)` as before.
    fn band(&self, l: Layout, order: Order, still: Option<&Still>, moving: bool, paint: impl FnOnce(Option<&Still>) -> Vec<u8>) -> Vec<u8> {
        let rows = l.preview_h.min(l.h);
        let n = l.w as usize * rows as usize * 4;
        let Some(s) = still.filter(|_| !moving && n > 0) else { return paint(still) };
        let mut g = self.band.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(b) = g.as_mut().filter(|b| b.w == l.w && b.rows == rows && b.order == order && b.still == *s) {
            let mut px = paint(None);
            if px.len() >= n && b.px.len() == n {
                px[..n].copy_from_slice(&b.px);
                b.hits += 1;
                return px;
            }
            return paint(Some(s));
        }
        let px = paint(Some(s));
        if px.len() >= n {
            let hits = g.as_ref().map_or(0, |b| b.hits);
            *g = Some(BandPx { w: l.w, rows, order, still: s.clone(), px: px[..n].to_vec(), hits });
        }
        px
    }
}

impl Face {
    /// STRIAPERFTHREE (SR9037): how many paints copied the kept preview band instead of scaling the still.
    pub fn band_hits(&self) -> u64 {
        self.pcache.band.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |b| b.hits)
    }
}

#[cfg(test)]
mod striaperfthree_tests {
    use super::*;

    fn face(order: PixelOrder, labels: bool) -> Face {
        let (tl, src) = synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 240), order, labels);
        f.state.playhead = 3;
        f
    }

    fn pixels(blits: Vec<SMessage>) -> Vec<u8> {
        match blits.into_iter().last() {
            Some(SMessage::SurfaceBlit { pixels, .. }) => pixels,
            _ => panic!("a blit"),
        }
    }

    /// STRIAPERFTHREE M3: a repeat paint copies the kept band and paints the same bytes.
    #[test]
    fn striaperfthree_band_repaint_copies() {
        let mut f = face(PixelOrder::Rgba, false);
        let a = pixels(f.frames());
        assert!(f.still().is_some(), "the synth reel gives a still at tick 3");
        assert_eq!(f.band_hits(), 0);
        let b = pixels(f.frames());
        assert_eq!(f.band_hits(), 1);
        assert_eq!(a, b);
    }

    /// STRIAPERFTHREE M3: a hit's bytes are a miss's bytes, in both byte orders, labels on.
    #[test]
    fn striaperfthree_band_bytes_identical() {
        for order in [PixelOrder::Rgba, PixelOrder::Bgra] {
            let mut a = face(order, true);
            a.frames();
            let hit = pixels(a.frames());
            assert_eq!(a.band_hits(), 1);
            let mut b = face(order, true);
            assert_eq!(hit, pixels(b.frames()), "{order:?}");
        }
    }

    /// STRIAPERFTHREE M3: a new still misses and paints what a fresh face paints; playing bypasses the band.
    #[test]
    fn striaperfthree_band_key_and_motion() {
        let mut f = face(PixelOrder::Rgba, false);
        f.frames();
        f.state.playhead = 10;
        let got = pixels(f.frames());
        assert_eq!(f.band_hits(), 0, "a new still is a miss");
        let mut fresh = face(PixelOrder::Rgba, false);
        fresh.state.playhead = 10;
        assert_eq!(got, pixels(fresh.frames()));
        f.frames();
        assert_eq!(f.band_hits(), 1);
        f.playing = true;
        f.frames();
        assert_eq!(f.band_hits(), 1, "playing: the band is not read");
        f.playing = false;
        let none = PreviewCache::default();
        let l = view::Layout::of(320, 240, &f.state);
        let px = none.band(l, Order::Rgba, None, false, |s| { assert!(s.is_none()); vec![7; 4] });
        assert_eq!(px, vec![7; 4], "no still: the paint as before");
    }
}

// ---- STRIAMARKERCHAPTERS (LEDGER SR9046): the chapters sidecar beside an export (named tail block) ----
// `start_export` keeps the timeline's markers inside the exported range as an ffmetadata chapters text
// (`Timeline::chapters`, times from the one `Timebase::to_ns`) in `ExportJob.chapters`, computed on the face's thread from
// the same timeline the export thread clones; `finish_export` writes it to `<dir>/chapters.txt` ONLY on a finished export
// with at least one chapter and names it as one clause on the done line. No markers in range, a cancel or a refusal: no file.

/// STRIAMARKERCHAPTERS (SR9046): the sidecar's name in the export directory.
pub const CHAPTERS_FILE: &str = "chapters.txt";

impl Face {
    /// The chapters text of the markers inside the range an export takes (`range`: the marked range, `None` the whole
    /// timeline, `start_export`'s span rule); empty when no marker stands in it.
    fn export_chapters(&self, range: &Option<std::ops::Range<i64>>) -> String {
        self.state.timeline.chapters(range.clone().unwrap_or(0..self.state.timeline.end()))
    }
}

/// Write `text` to `<dir>/chapters.txt` and word the done line's clause: `, chapters.txt <n> chapter(s)`, or
/// `, chapters.txt: <io error>` (the movie stands); "" (and no file) when `text` is empty.
fn chapters_done(dir: &std::path::Path, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let n = text.lines().filter(|l| *l == "[CHAPTER]").count();
    match std::fs::write(dir.join(CHAPTERS_FILE), text) {
        Ok(()) => format!(", {CHAPTERS_FILE} {n} chapter{}", if n == 1 { "" } else { "s" }),
        Err(e) => format!(", {CHAPTERS_FILE}: {e}"),
    }
}

// ---- STRIACLIPSPEEDTHREE (LEDGER SR9031): the ripple speed button — bare `rspeed` cycles SPEED_CYCLE with a ripple (named tail block) ----
/// STRIACLIPSPEEDTHREE (LEDGER SR9031): THE speed cycle both buttons step: the selected clip's next rate in [`SPEED_CYCLE`]
/// (1x -> 2x -> 4x -> 1x; a rate not in the table goes to 1x); `NoClip` with nothing selected.
pub fn next_speed(st: &EditorState) -> Result<u32, EditError> {
    let now = st.selected().ok_or(EditError::NoClip)?.rate_q16;
    let i = SPEED_CYCLE.iter().position(|&r| r == now).map_or(0, |i| (i + 1) % SPEED_CYCLE.len());
    Ok(SPEED_CYCLE[i])
}

/// STRIACLIPSPEEDTHREE: bare `rspeed` (the Ripple Speed button): the selected clip to [`next_speed`] through
/// `EditorState::ripple_speed` (the rest of its track follows; ONE commit, so undo restores); a refusal comes back by name.
pub fn rspeed_cycle(st: &mut EditorState) -> Result<(), EditError> {
    let q = next_speed(st)?;
    st.ripple_speed(q)
}

// ---- STRIATRACKSTHREE (LEDGER SR9038): the track header column on the face (named tail block) ----
// NLE-TRACK-HEADER: the `headers` word toggles timeline_core's header column (`Layout::with_headers`, THE layout the face
// paints and hits); it is view-only (no commit, the document stays clean). While it shows, a click on a row's header
// (`Layout::header_at`) makes that track the target of bare `track-` and says so on the face's note; any other click
// clears the target, and so does any `track…` word (a track add or remove moves the rows). Bare `track-` with a target is
// the explicit `track- <ch>` THE verb already takes; nothing else reads the target. Off by default: no column, no target.
impl Face {
    /// The `headers` word: the column on / off (off drops the target); `None` for any other word.
    fn headers_verb(&mut self, v: &str) -> Option<bool> {
        if v != "headers" {
            return None;
        }
        self.headers = if self.headers.is_some() { None } else { Some(None) };
        Some(true)
    }
    /// A click on a header while the column shows: that track is bare `track-`'s target (`Some(true)`); any other click
    /// clears the target and goes on (`None`).
    fn pick_header(&mut self, x: f64, y: f64) -> Option<bool> {
        let target = self.headers.as_mut()?;
        *target = None;
        let row = self.layout().header_at(x, y)?;
        let word = self.state.timeline.track_word(row)?;
        self.headers = Some(Some(row));
        self.note = Some(format!("headers: track- targets {word}"));
        Some(true)
    }
    /// The word the track road runs: any `track…` word takes the header target; bare `track-` with one names its track.
    fn headed(&mut self, v: &str) -> String {
        if !v.starts_with("track") {
            return v.to_string();
        }
        let target = self.headers.as_mut().and_then(|t| t.take());
        match target.and_then(|t| self.state.timeline.track_word(t)) {
            Some(word) if v.trim() == "track-" => format!("track- {word}"),
            _ => v.to_string(),
        }
    }
}

// ---- STRIATHUMBNAILSTHREE (LEDGER SR9044): the visible tiles are never evicted (named tail block) ----
// SR839's `Thumbs::insert` dropped the OLDEST entry at THUMB_CACHE with no regard for the paint in hand: a view wanting more
// tiles than the cap (SR949's filmstrip on a wide canvas or many rows) evicted tiles that same paint wanted, so every paint
// re-decoded THUMBS_PER_PAINT of them, `pending` never cleared and `serve`'s clock decoded forever. RULE (the same map,
// `thumb_for` and budget; seq untouched, so "a hit refreshes nothing" stays true):
// * `fill` and `fill_strip` mark every visible key they test (`want`, in place of the bare `contains_key`) into `wanted`.
// * `insert` evicts the oldest key that is NOT `spared` (wanted by this paint or kept from the last one); none such: no
//   eviction, the map holds the visible set past the cap.
// * `begin_paint` (the first line of `fill`) rolls the sets (`kept` = the last paint's `wanted`) and trims the map back to
//   THUMB_CACHE oldest-first among keys the last paint did not want, so a view that moved gives its memory back.
// Steady state over an unchanged view: zero decodes and `pending` false. SR997 (worker folds on `thumb_for` / `wave_fill`)
// and SR9019 (`fill` one loop over `view::thumb_want`, `fill_strip` deleted) touch the same fns on their branches: at the
// fold the `want` marking goes inside that one loop; `spared` / `begin_paint` stay as written.

impl Thumbs {
    /// STRIATHUMBNAILSTHREE (SR9044): mark `(src, tick)` wanted by this paint; true when it is keyed already.
    fn want(&mut self, src: timeline_core::SourceId, tick: i64) -> bool {
        self.wanted.insert((src, tick));
        self.map.contains_key(&(src, tick))
    }
    /// STRIATHUMBNAILSTHREE (SR9044): key `k` was wanted by this paint or the last one (never evicted).
    fn spared(&self, k: &(timeline_core::SourceId, i64)) -> bool {
        self.wanted.contains(k) || self.kept.contains(k)
    }
    /// STRIATHUMBNAILSTHREE (SR9044): a paint starts: the last paint's wanted keys are kept, and the map trims back to
    /// THUMB_CACHE, the oldest unkept first.
    fn begin_paint(&mut self) {
        self.kept = std::mem::take(&mut self.wanted);
        while self.map.len() > THUMB_CACHE {
            let Some(old) = self.map.iter().filter(|e| !self.kept.contains(e.0)).min_by_key(|e| (e.1).0).map(|e| *e.0) else { break };
            self.map.remove(&old);
        }
    }
}

#[cfg(test)]
mod striathumbnailsthree_tests {
    use super::*;

    fn still() -> Option<Still> {
        Some(Still { width: 1, height: 1, rgba: vec![1, 2, 3, 255] })
    }

    #[test]
    fn striathumbnailsthree_wanted_tiles_outlive_the_cap() {
        let mut t = Thumbs::default();
        let id = timeline_core::SourceId(0);
        t.begin_paint();
        for i in 0..=THUMB_CACHE as i64 {
            assert!(!t.want(id, i));
            t.insert((id, i), still());
        }
        assert_eq!(t.map.len(), THUMB_CACHE + 1, "every key the paint wants stays past the cap");
        assert!(t.get(id, 0).is_some(), "the oldest wanted key was not evicted");
        t.begin_paint();
        assert_eq!(t.map.len(), THUMB_CACHE + 1, "the next paint keeps the last paint's keys");
        assert!((0..=THUMB_CACHE as i64).all(|i| t.want(id, i)), "the same view: every key a hit, no decode");
        println!("striathumbnailsthree: {} wanted keys kept over a cap of {}", t.map.len(), THUMB_CACHE);
    }

    #[test]
    fn striathumbnailsthree_eviction_takes_the_oldest_unwanted() {
        let mut t = Thumbs::default();
        let id = timeline_core::SourceId(0);
        for i in 0..THUMB_CACHE as i64 {
            t.insert((id, i), still());
        }
        t.begin_paint();
        for i in 0..10 {
            assert!(t.want(id, i));
        }
        t.insert((id, 1000), still());
        assert_eq!(t.map.len(), THUMB_CACHE);
        assert!(t.get(id, 0).is_some(), "a wanted key survives though it is the oldest");
        assert!(!t.map.contains_key(&(id, 10)), "the oldest unwanted key went");
        assert!(t.get(id, 1000).is_some());
    }

    #[test]
    fn striathumbnailsthree_trim_back_after_the_view_moves() {
        let mut t = Thumbs::default();
        let id = timeline_core::SourceId(0);
        t.begin_paint();
        for i in 0..THUMB_CACHE as i64 + 40 {
            t.want(id, i);
            t.insert((id, i), still());
        }
        assert_eq!(t.map.len(), THUMB_CACHE + 40);
        t.begin_paint(); // the view moved: this paint wants other keys
        t.want(id, 5000);
        t.begin_paint(); // the paint after it: the old view's keys are no longer kept
        assert_eq!(t.map.len(), THUMB_CACHE, "trimmed back to the cap");
        assert!(!t.map.contains_key(&(id, 39)) && t.get(id, 40).is_some(), "oldest first");
    }
}

// ---- STRIAMARKERRANGES (LEDGER SR9052): the marker marks word's note and refusals; range-end names (named tail block) ----
// `marker marks` (the Marker Marks button) is timeline_core's `EditorState::verb` word (SR723's marks commit: one or none);
// the face words the result: a success is the note `marks <in>..<out> from marker M1`, a refusal is `last_error`
// (`marker marks: no marker at or before <tc>`, `marker marks: M1 is a point marker (no range)`). A marker jump that lands
// on a range end names it `M1 end` (a marker standing on that tick is named instead).

impl Face {
    /// The name a marker jump word gives tick `t`: the marker at `t`, else `<name> end` of a marker whose range ends at `t`.
    fn marker_jump_name(&self, t: i64) -> String {
        let tl = &self.state.timeline;
        tl.marker_at(t).map(|m| m.name.clone()).or_else(|| tl.markers.iter().find(|m| m.len > 0 && m.at + m.len == t).map(|m| format!("{} end", m.name))).unwrap_or_default()
    }
    /// The `marker marks` word (`timeline_core::editor::marker_marks_word`) through the one dispatch, with its note or
    /// refusal; `None` for any other word.
    fn marker_marks_verb(&mut self, v: &str) -> Option<bool> {
        if !timeline_core::editor::marker_marks_word(v) {
            return None;
        }
        let ph = self.state.playhead;
        let before = self.state.timeline.marker_before(ph).map(|m| (m.name.clone(), m.at, m.len));
        let r = self.state.verb(v).unwrap_or(Err(EditError::Marker));
        match (r, before) {
            (Ok(()), Some((name, at, len))) => self.note = Some(format!("marks {}..{} from marker {name}", self.marker_tc(at), self.marker_tc(at + len))),
            (Err(EditError::Marker), None) => self.last_error = Some(format!("marker marks: no marker at or before {}", self.marker_tc(ph))),
            (Err(EditError::Marker), Some((name, _, 0))) => self.last_error = Some(format!("marker marks: {name} is a point marker (no range)")),
            (r, _) => self.last_error = Some(format!("marker marks: {}", r.err().map_or_else(String::new, |e| e.to_string()))),
        }
        Some(true)
    }
}

// ---- STRIAXFORMTHREE (LEDGER SR9033): the placement drag on the preview picture and its routing (named tail block) ----
// NLE-XFORM-DRAG on the face: `Face::on`'s first line hands every message here first. A press on a PLACED primary's rect in
// the preview picture (the split face's preview Surface, or an unsplit face's preview band through `takes`) begins
// timeline_core's `view::XformDrag` over the layout the face paints that band with (`layout_for(..).with_message(..)`)
// and the preview still's size (`self.still`, else ONE `preview` call: THE frame road); while live, that Surface's moves
// update it, its release commits ONCE (`XformDrag::finish`: `EditorState::set_xform`; the message row then reads the
// placement through [`placement_note`], SR938's line; a refusal is `last_error` as `xform-drag: <e>`), Escape drops it;
// the one-Surface face keeps its `pointer` through them as `press` / `release` do. Anything else (no drag, a press off a
// placed rect, the help shown) returns `None`: the old road, unchanged. At the fold this call follows SR962's meter click
// and precedes SR960's `crop_drag_on` on the same line; SR960's `CropDrag::begin` refuses a placed primary (the seat's
// fold guard), so a placed clip's press is this drag's and an unplaced clip's picture edge is the crop drag's.
// NLE-XFORM-NUDGE-BUTTON: `pip left` / `pip right` / `pip up` / `pip down` are VERBS words (their buttons in stria-view),
// SR975's nudge words through the one `Face::verb` road (`xform_note_verb`).

/// STRIAXFORMTHREE (SR9033): THE placement line the message row reads after a placement edit (`pip: x 725 y 725 scale 250`).
fn placement_note(c: &Clip) -> String {
    format!("pip: x {} y {} scale {}", c.xform.x, c.xform.y, c.xform.s)
}

impl Face {
    /// STRIAXFORMTHREE (SR9033): the live placement drag on the preview picture.
    pub fn xform_dragging(&self) -> Option<&view::XformDrag> {
        self.xform_drag.as_ref()
    }
    /// The role of the preview picture a message belongs to: split, the preview Surface's `Preview`; unsplit, `Whole` for a
    /// message this face takes; `None` otherwise.
    fn xform_drag_surface(&self, msg: &SMessage) -> Option<Role> {
        match self.split {
            Some(s) => (msg.surface() == Some(s.preview) && s.preview != s.tracks).then_some(Role::Preview),
            None => self.takes(msg).then_some(Role::Whole),
        }
    }
    /// The placement drag's messages (`Some(repaint)`); `None` hands the message on to [`Face::on`]'s road.
    fn xform_drag_on(&mut self, msg: &SMessage) -> Option<bool> {
        if self.xform_drag.is_some() {
            return match msg {
                SMessage::BrowserMouseMove(x, y, _) if self.xform_drag_surface(msg).is_some() => {
                    if self.split.is_none() { self.pointer = Some((*x, *y)); }
                    let d = self.xform_drag.as_mut()?;
                    let before = d.xform();
                    d.update(*x, *y);
                    Some(d.xform() != before)
                }
                SMessage::BrowserButton { down: false, x, y, .. } if self.xform_drag_surface(msg).is_some() => {
                    if self.split.is_none() { self.pointer = Some((*x, *y)); }
                    let mut d = self.xform_drag.take()?;
                    d.update(*x, *y);
                    let moved = d.moved();
                    match d.finish(&mut self.state) {
                        Ok(()) if moved => self.xform_note = self.state.selected().map(|c| placement_note(&c)),
                        Ok(()) => {}
                        Err(e) => self.last_error = Some(format!("xform-drag: {e}")),
                    }
                    Some(true)
                }
                SMessage::BrowserKey(k) if k == "Escape" => {
                    self.xform_drag = None;
                    Some(true)
                }
                _ => None,
            };
        }
        let SMessage::BrowserButton { down: true, x, y, .. } = *msg else { return None };
        if self.help {
            return None;
        }
        let role = self.xform_drag_surface(msg)?;
        let l = self.layout_for(role).with_message(self.labels.is_some());
        if self.still.is_none() {
            self.still = preview(&self.state.timeline, &self.sources, self.state.playhead).ok().flatten();
        }
        let (sw, sh) = self.still.as_ref().map(|s| (s.width, s.height))?;
        self.xform_drag = Some(view::XformDrag::begin_nosnap(&self.state, l, sw, sh, x, y, mods & bandy::MOD_ALT != 0)?); // STRIAXFORMHANDLES (SR9068): Alt at the press snaps nothing, the body press's rule (`press`)
        self.xform_note = None;
        self.buttons = true;
        if self.split.is_none() { self.pointer = Some((x, y)); } // the one-Surface face's pointer, as `press` keeps it
        Some(true)
    }
}

// ---- STRIAHISTORYSPLIT (LEDGER SR9056): the history line picked on the split preview Surface (named tail block) ----
// Split, the message row (and so the noted history line) is painted on the preview Surface, whose pointer `takes` drops.
// One fold ahead of `takes` (SR962's shape) hands a press down / click-only click there, while a note stands, to the ONE
// pick fn `history_pick_at` with `Role::Preview` (its geometry from `layout_for`): an undo entry runs SR934's `undo <k>`,
// a redo entry `redo <q>`, a press on the row off any entry is taken and moves nothing; anything else falls to `takes`.
impl Face {
    /// STRIAHISTORYSPLIT (SR9056): a split preview Surface's press down / click-only click while a note stands, through
    /// [`Face::history_pick_at`] on the preview's row; `None` for anything else (unsplit, another Surface, no note, off the
    /// row), which [`Face::takes`] then drops as before.
    fn preview_history_input(&mut self, msg: &SMessage) -> Option<bool> {
        let s = self.split?;
        if msg.surface() != Some(s.preview) {
            return None;
        }
        self.note.as_ref()?;
        let (x, y) = match *msg {
            SMessage::BrowserButton { down: true, x, y, .. } => (x, y),
            SMessage::BrowserClick(x, y, _) if !self.buttons => (x, y),
            _ => return None,
        };
        if matches!(msg, SMessage::BrowserButton { .. }) {
            self.buttons = true;
        }
        self.history_pick_at(x, y, Role::Preview)
    }
}

// ---- STRIAOPACITYKEYS (LEDGER SR9048): the opacity point words and their note (named tail block) ----
// `opacity-key [<pct>]` / `opacity-unkey` (typed words) go through timeline_core's ONE verb dispatch (one commit each);
// a success is the face note `opacity key: <pct> % at <tick>` (the first selected clip under the playhead, its value
// there) or `opacity key cleared at <tick>`; a refusal reads `<word>: <EditError>` in `last_error`.

impl Face {
    /// STRIAOPACITYKEYS: an opacity point word through the one dispatch, its result on the note; `None` for any other word.
    fn opacity_key_word(&mut self, v: &str) -> Option<bool> {
        let word = v.split_ascii_whitespace().next()?;
        if !matches!(word, "opacity-key" | "opacity-unkey") {
            return None;
        }
        let ph = self.state.playhead;
        match self.state.verb(v).unwrap_or(Err(EditError::NoClip)) {
            Ok(()) if word == "opacity-key" => {
                let tl = &self.state.timeline;
                let pct = self.state.set.iter().filter_map(|&(t, i)| tl.tracks.get(t)?.clips.get(i).copied()).find(|c| c.covers(ph)).map_or(timeline_core::OPAQUE, |c| c.opacity_at(ph));
                self.last_error = None;
                self.note = Some(format!("opacity key: {pct} % at {ph}"));
            }
            Ok(()) => {
                self.last_error = None;
                self.note = Some(format!("opacity key cleared at {ph}"));
            }
            Err(e) => self.last_error = Some(format!("{word}: {e}")),
        }
        Some(true)
    }
}

// ---- STRIASOURCEVIEWER (LEDGER SR9045): the source monitor's preview and the bin's load (named tail block) ----
// A source loaded into timeline_core's `EditorState::source` (session state: its own playhead and in / out marks) shows
// in the preview through THE one frame road, [`preview`], over `EditorState::source_view` (a one-clip timeline at its tick
// 0); nothing loaded paints exactly as before. Playback stays the timeline's transport (its played stills show) and a live
// drag paints the timeline (`Drag::shown` carries no source). Shift on the bin's release loads that row's reel with the
// kinds its media holds (`Sources::holds`); a plain click still lays it. The words and the three-point insert are
// timeline_core's (`EditorState::verb`); the message line reads `EditorState::source_line`.

/// The frame the preview shows: the loaded source's one-clip view at its tick 0, else the timeline at the playhead.
fn preview_shown(st: &EditorState, sources: &Sources) -> Result<Option<Still>, RenderError> {
    match st.source_reel() { // STRIASOURCEPLAY (SR9080): the reel the play road plays, at the source playhead
        Some(v) => preview(&v, sources, st.shown_tick()),
        None => preview(&st.timeline, sources, st.playhead),
    }
}

impl Face {
    /// A Shift release on the bin at `(x, y)`: that row's reel loaded into the source monitor (the kinds its media holds;
    /// none opened: the kinds of the tracks it sits on); false under the last row. Pushes nothing.
    fn bin_load(&mut self, x: f64, y: f64) -> bool {
        let Some(b) = self.bin() else { return false };
        let Some(i) = view::bin_hit_from(&self.state.timeline, b.canvas.0, b.canvas.1, b.top, x, y) else { return false };
        let Some(src) = self.state.timeline.sources.get(i).map(|s| s.id) else { return false };
        let kinds: Vec<Kind> = [Kind::Video, Kind::Audio].into_iter().filter(|k| self.sources.holds(src, *k)).collect();
        if let Err(e) = self.state.load_source(src, &kinds) {
            self.last_error = Some(format!("source: {e}"));
        }
        true
    }
}

#[cfg(test)]
mod striasourceviewer_tests {
    use super::*;
    use timeline_core::{Mode, SourceId};

    fn plain() -> Face {
        let (tl, src) = synth_fixture();
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false)
    }

    /// A loaded source paints its own frame through the one frame road; stepping and `unload` push nothing and dirty
    /// nothing; unloaded, the preview is the timeline's again.
    #[test]
    fn striasourceviewer_preview_shows_the_loaded_source() {
        let mut f = plain();
        f.paint();
        let timeline = preview(&f.state.timeline, &f.sources, 0).unwrap();
        assert_eq!(f.still().cloned(), timeline);
        for v in ["source B", "src-next", "src-next", "src-next"] {
            assert!(f.verb(v), "{v}");
        }
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        f.paint();
        let want = preview(&f.state.source_view().unwrap(), &f.sources, 0).unwrap();
        assert!(want.is_some(), "B's frame 3");
        assert_eq!(f.still().cloned(), want);
        assert_ne!(f.still().cloned(), timeline, "the source, not the timeline");
        assert!(f.verb("unload"));
        f.paint();
        assert_eq!(f.still().cloned(), timeline);
        assert_eq!((f.state.undo_depth(), f.dirty()), (0, false));
    }

    /// Shift on the bin's release loads the row's reel (no edit); a plain click still lays; with marks, `insert` lays the
    /// marked range at the timeline playhead on V1 and A1 in one undo step.
    #[test]
    fn striasourceviewer_bin_shift_release_loads_then_insert_lays() {
        let mut f = plain().on_surfaces(0, 1).with_bin(2);
        let tl0 = f.state.timeline.clone();
        let (x, y) = view::bin_point(1);
        assert!(f.on(&SMessage::BrowserButton { down: false, x, y, mods: bandy::MOD_SHIFT, surface: 2 }));
        assert_eq!(f.state.source.as_ref().map(|m| (m.src, m.at, m.kinds.clone())), Some((SourceId(1), 0, vec![Kind::Video, Kind::Audio])));
        assert_eq!((&f.state.timeline, f.state.undo_depth()), (&tl0, 0), "loading lays nothing");
        for v in ["src-next", "src-in", "src-next", "src-next", "src-out"] {
            assert!(f.verb(v), "{v}");
        }
        f.state.deselect();
        f.state.seek(15);
        assert!(f.verb("insert"));
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        let c = Clip::new(SourceId(1), 1, 3, 15);
        assert_eq!(f.state.timeline, tl0.place(0, c, Mode::Insert).unwrap().place(1, c, Mode::Insert).unwrap());
        assert_eq!((f.state.undo_depth(), f.state.playhead), (1, 17));
        assert!(f.verb("undo"));
        assert_eq!(f.state.timeline, tl0);
        assert!(f.verb("unload"));
        f.state.seek(15);
        assert!(f.on(&SMessage::BrowserClick(x, y, 2)), "a plain click is SR852's insert word, as before");
        assert_eq!(f.state.timeline, tl0.place(0, Clip::new(SourceId(1), 0, 50, 15), Mode::Insert).unwrap());
    }

    /// The message line reads the loaded source after a standing error; a src word with nothing loaded is refused by name.
    #[test]
    fn striasourceviewer_message_line_reads_the_source() {
        let mut f = plain();
        assert!(f.verb("src-in"));
        assert_eq!(f.last_error.take(), Some(format!("src-in: {}", EditError::NoSource)));
        for v in ["source B", "src-next", "src-next", "src-next", "src-in"] {
            assert!(f.verb(v), "{v}");
        }
        assert_eq!(f.message_text().as_deref(), Some("source B 00:00:00:03  in 00:00:00:03  out --"));
        assert!(f.verb("src-out"));
        assert_eq!(f.message_text(), Some(format!("src-out: {}", EditError::Marks)), "the error first");
    }
}

// ---- STRIANORMALIZELUFS (LEDGER SR9054): `normalize lufs [<LUFS>]` — the clip's gain from its loudness, bounded by its true peak (named tail block) ----
// `normalize lufs [<LUFS>]` (default -23, EBU R128) measures the selected audio clip ALONE at unity (nle.rs
// `loudness_clip`: SR995's one measure body over a one-clip render) and sets its gain so the integrated loudness lands
// on the target, bounded so the true peak never passes `NORMALIZE_TP_CEILING_CDB` (-1 dBTP, R128's maximum): both
// readings move one-for-one with the gain (BS.1770's relative gate is scale-free; only the -70 LUFS absolute gate is
// not). The gain goes through SR838's one road (`set_selected_gain`: validate, one commit, one undo step), the scan
// through SR9035's (`scan.thread`: off the face's thread under `serve`, joined by `normalize_tick`). The success line is
// the face note (`Face.note`, read by `message_text` after `last_error`); `measuring` and every refusal ride
// `last_error` as SR878's lines do. A host-face word: the kernel Player never sees it (the gain is the `* UNA GAIN` it reads).

/// STRIANORMALIZELUFS (SR9054): the target when the word names none, centi-LU (-23 LUFS, EBU R128).
pub const NORMALIZE_LUFS_TARGET_CDB: i32 = -2300;

/// STRIANORMALIZELUFS (SR9054): the true-peak ceiling the gain never passes, centi-dB (-1 dBTP, EBU R128's maximum).
pub const NORMALIZE_TP_CEILING_CDB: i32 = -100;

/// STRIANORMALIZELUFS (SR9054): what one normalize scan read: SR9035's peak, or one clip's loudness reading.
#[derive(Debug, Clone, PartialEq)] // STRIANORMALIZELUFSMORE (SR9086): not Copy (a selection's readings are a Vec)
pub enum NormalizeRead {
    Peak(f32),
    Lufs(crate::nle::LoudnessReading), LufsMany(Vec<Option<crate::nle::LoudnessReading>>), PeakMany(Vec<Option<f32>>), // STRIANORMALIZELUFSMORE (SR9086): a selection's readings, one per clip in the run's order (file tail)
}

/// STRIANORMALIZELUFS (SR9054): the gain (centi-dB) that brings reading `r` to `target_cdb` LUFS, rounded DOWN, and
/// bounded (rounded down) so the true peak stays at or under [`NORMALIZE_TP_CEILING_CDB`]; the flag is true when that
/// ceiling set the gain. `None` for a silent clip (integrated not finite or at the meter's -70 LUFS floor).
pub fn normalize_lufs_gain_cdb(r: &crate::nle::LoudnessReading, target_cdb: i32) -> Option<(i32, bool)> {
    if !(r.lufs.is_finite() && r.lufs > gneiss_pal::dsp::audio::meter::LUFS_FLOOR) {
        return None;
    }
    let by_l = (target_cdb as f64 - 100.0 * r.lufs).floor();
    let by_tp = if r.true_peak_db.is_finite() { (NORMALIZE_TP_CEILING_CDB as f64 - 100.0 * r.true_peak_db).floor() } else { by_l };
    Some((by_l.min(by_tp) as i32, by_tp < by_l))
}

impl Face {
    /// The `normalize lufs [<LUFS>]` word; `None` for any other word (SR878's `normalize_verb` keeps the rest).
    fn normalize_lufs_verb(&mut self, v: &str) -> Option<bool> {
        let mut w = v.split_ascii_whitespace();
        w.next().filter(|w| *w == "normalize")?;
        w.next().filter(|w| *w == "lufs")?;
        let (arg, extra) = (w.next(), w.next());
        let r = self.normalize_lufs(arg, extra);
        Some(self.normalize_lufs_say(r))
    }
    /// The rule behind [`Face::normalize_lufs_verb`]: `Ok` the success line (or `measuring` for a served face's scan,
    /// `Err` with that word so it rides `last_error`), `Err` a refusal (the timeline unmoved).
    fn normalize_lufs(&mut self, arg: Option<&str>, extra: Option<&str>) -> Result<String, String> {
        if let Some(x) = extra {
            return Err(format!("{x} is not a LUFS level"));
        }
        let target = match arg {
            None => NORMALIZE_LUFS_TARGET_CDB,
            Some(a) => timeline_core::editor::cdb_of(a).ok_or_else(|| format!("{a} is not a LUFS level"))?,
        };
        if target > 0 {
            return Err("target above 0 LUFS".to_string());
        }
        if let Some(r) = self.normalize_lufs_many(target) { return r; } let (track, index) = self.state.selection.ok_or_else(|| EditError::NoClip.to_string())?; // STRIANORMALIZELUFSMORE (SR9086): two or more selected audio clips, each measured alone (file tail)
        let c = self.state.selected().ok_or_else(|| EditError::NoClip.to_string())?;
        if self.state.timeline.tracks.get(track).map(|t| t.kind) != Some(Kind::Audio) {
            return Err("not an audio clip".to_string());
        }
        if !self.scan.thread {
            let r = crate::nle::loudness_clip(&self.state.timeline, track, index, &self.sources, None).map_err(|e| e.to_string())?.ok_or_else(|| "no audio".to_string())?;
            return self.normalize_lufs_finish(r, target);
        }
        if self.scan.job.is_some() {
            return Err("busy".to_string());
        }
        let (tl, sources) = (self.state.timeline.clone(), self.sources.clone()); let run = LufsRun::new(&[((track, index), c)], true); let w = run.watches[0].clone(); // STRIANORMALIZELUFSMORE (SR9086): the measure's watch, SR903's stop and progress (file tail)
        let handle = std::thread::spawn(move || crate::nle::loudness_clip(&tl, track, index, &sources, Some(w)).map(|r| r.map(NormalizeRead::Lufs)).map_err(|e| e.to_string())); // STRIANORMALIZELUFSMORE (SR9086): watched
        self.scan.job = Some(NormalizeJob { track, index, clip: c, target, os: 0, lufs: Some(run), handle }); // STRIANORMALIZELUFSMORE (SR9086): the run rides the job
        Err(NORMALIZE_SCANNING.to_string())
    }
    /// The gain from a loudness reading `r` set through SR838's one road, and its line: `lufs <l> LUFS, true peak <t>
    /// dBTP -> gain <g>`, ` (true-peak bound)` when the ceiling set it.
    fn normalize_lufs_finish(&mut self, r: crate::nle::LoudnessReading, target: i32) -> Result<String, String> {
        let (cdb, bound) = normalize_lufs_gain_cdb(&r, target).ok_or_else(|| "silent clip".to_string())?;
        self.state.set_selected_gain(cdb).map_err(|e| e.to_string())?;
        Ok(format!("lufs {:.1} LUFS, true peak {:.1} dBTP -> gain {}{}", r.lufs, r.true_peak_db, db_words(cdb), if bound { " (true-peak bound)" } else { "" }))
    }
    /// A normalize-lufs outcome on the message row: success the face note (any standing error cleared), else
    /// `last_error`; each prefixed `normalize: `. True (repaint).
    fn normalize_lufs_say(&mut self, r: Result<String, String>) -> bool {
        match r {
            Ok(line) => {
                self.last_error = None;
                self.note = Some(format!("normalize: {line}"));
            }
            Err(line) => self.last_error = Some(format!("normalize: {line}")),
        }
        true
    }
}

// The face note (the settled SR990 shape; this branch had none): `Face.note` is read by `message_text` after
// `last_error` (an error wins) and before the export line and the reel line; it stands across repaints until the next
// acting input clears it.
impl Face {
    /// Clear the note on an acting input: a typed word, a click, a press, select-all, a named key, a one-char text
    /// key. A move, a release, the wheel, a zoom and a resize keep it.
    fn clear_note(&mut self, msg: &SMessage) {
        let acts = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserButton { down: true, .. } | SMessage::BrowserSelectAll => true,
            SMessage::BrowserKey(k) => k.chars().count() > 1,
            SMessage::BrowserText(t) => t.chars().count() == 1,
            _ => false,
        };
        if acts {
            self.note = None;
        }
    }
}

// ---- STRIAPLAYSYNCPACE (LEDGER SR9055): NLEPLAY-SYNC-PACE, the played still shown at the AudioClock's tick (named tail block) ----
// The render and push cadence stay as they are (the device ring keeps its fill); only what is SHOWN is paced. Each forward 1x
// heard tick's rendered still is queued with its tick and end; the parent's ONE AudioClock (as `sync_mark` fed it before the
// push) and ONE gneiss_pal `avsync::Scheduler` (early = half a timeline frame, media.rs's `Player::tick` loop shape) decide
// Present / Wait / Drop for the queue's head. The presented tick becomes the playhead and the paint's `pending`, and the
// parent's DriftMeter records ITS pts against the clock; nothing presented holds the playhead and `shown` on the last
// presented tick and `tick` returns false (no repaint: the glass holds). More than PACE_HOLD queued stills present the head
// regardless, so a stalled device delays the picture by at most PACE_HOLD ticks and never freezes it. No clock (no device,
// muted, paused, shuttle, reverse): pass-through, the queue cleared. Wire: `[STRIA] :: nle play pace presented= dropped=
// held= queued_max=` once per heard run; `Face::play_pace`. Owed: NLEPLAY-SYNC-PACE-TAIL (the queued stills at the
// timeline's end are not shown), NLEPLAY-PACE-STILLMEM (the queue holds full render frames).

/// STRIAPLAYSYNCPACE (SR9055): the most stills the pace holds back: half a second at 25 fps, the depth of
/// `media::resonance_out`'s ring (sample_rate / 2 frames). Past it the head is shown regardless (a stalled device).
pub const PACE_HOLD: usize = 12;

/// STRIAPLAYSYNCPACE (SR9055): the rendered stills not yet shown (tick, end, still), the one Scheduler, where the queue's last
/// render ended, the presented tick (tick, end), whether the next presentation is a run's seed (not recorded), whether this
/// tick held, and the deepest the queue went.
struct PlayPace {
    queue: std::collections::VecDeque<(i64, i64, Option<Still>)>,
    sched: gneiss_pal::dsp::avsync::Scheduler,
    next: Option<i64>,
    on: Option<(i64, i64)>,
    fresh: bool,
    held: bool,
    queued_max: usize, tail: usize, small: Vec<(i64, u32, u32)>, // STRIAPACETAIL (SR9087): the drain ticks at the end, and the queued ticks sampled down with their size (file tail)
}

impl Default for PlayPace {
    fn default() -> Self {
        PlayPace { queue: std::collections::VecDeque::new(), sched: gneiss_pal::dsp::avsync::Scheduler::new(0), next: None, on: None, fresh: true, held: false, queued_max: 0, tail: 0, small: Vec::new() }
    }
}

impl Face {
    /// STRIAPLAYSYNCPACE (SR9055): after a forward played tick at `shown` rendered to `end`: queue its still and show the
    /// head the AudioClock has reached (Scheduler::decide), or hold the last presented tick. Pass-through without a clock.
    fn pace(&mut self, end: i64) {
        use gneiss_pal::dsp::avsync::{Decision, Scheduler};
        let Some(ph) = self.play.shown else { return };
        let pending = self.play.pending.take();
        let (now, still) = match (self.play.sync.clock.as_ref().map(|c| c.media_ns()), pending) {
            (Some(now), Some((at, s))) if at == ph => (now, s),
            (_, p) => {
                self.play.pending = p;
                let pace = &mut self.play.pace;
                pace.queue.clear();
                (pace.next, pace.on, pace.fresh) = (None, None, true);
                return;
            }
        };
        let tb = &self.state.timeline.tb;
        let pace = &mut self.play.pace;
        if pace.next != Some(ph) {
            pace.queue.clear();
            pace.fresh = true;
            pace.sched = Scheduler::new((tb.to_ns(1) / 2).max(0) as u64);
        }
        pace.next = Some(end);
        pace.queue.push_back((ph, end, still));
        pace.queued_max = pace.queued_max.max(pace.queue.len());
        let mut shown = None;
        while let Some(t) = pace.queue.front().map(|q| q.0) {
            let pts = tb.to_ns(t);
            let next = pace.queue.get(1).map(|q| tb.to_ns(q.0));
            let d = if pace.queue.len() > PACE_HOLD {
                pace.sched.stats.presented += 1;
                Decision::Present
            } else {
                pace.sched.decide(pts, next, now)
            };
            match d {
                Decision::Wait(_) => break,
                Decision::Drop => {
                    pace.queue.pop_front();
                }
                Decision::Present => {
                    shown = pace.queue.pop_front().map(|(t, e, s)| (pts, t, e, s));
                    break;
                }
            }
        }
        pace.sched.on_display_tick(shown.is_none() && pace.on.is_some());
        match shown {
            Some((pts, t, e, s)) => {
                if !pace.fresh {
                    self.play.sync.meter.av_offset(pts, now);
                }
                pace.fresh = false;
                pace.on = Some((t, e));
                self.play.pending = Some((t, s));
                self.play.shown = Some(t);
                self.state.seek(t);
            }
            None => {
                pace.held = true;
                if let Some((t, _)) = pace.on {
                    self.play.shown = Some(t);
                    self.state.seek(t);
                }
            }
        }
    }
    /// STRIAPLAYSYNCPACE (SR9055): a heard run starts (a fresh pace) or ends: the next resume steps ONE frame from the presented
    /// tick (not from the render cursor ahead of it), the run's counts are logged once and the queue is dropped.
    fn pace_run(&mut self, live: bool) {
        if live {
            self.play.pace = PlayPace::default();
            return;
        }
        if let Some((t, e)) = self.play.pace.on {
            if self.play.shown == Some(t) {
                self.play.next = Some(e);
            }
        }
        let st = self.play.pace.sched.stats;
        if st.presented > 0 {
            log::info!("[STRIA] :: nle play pace presented={} dropped={} held={} queued_max={}", st.presented, st.dropped, st.repeated, self.play.pace.queued_max);
        }
        let pace = &mut self.play.pace;
        pace.queue.clear();
        (pace.next, pace.on, pace.fresh, pace.held) = (None, None, true, false);
    }
    /// STRIAPLAYSYNCPACE (SR9055): the current (or last) heard run's pace: stills presented, dropped, and ticks held.
    pub fn play_pace(&self) -> gneiss_pal::dsp::avsync::SchedStats {
        self.play.pace.sched.stats
    }
}

#[cfg(test)]
mod striaplaysyncpace_tests {
    use super::*;
    use crate::media::AudioOut;
    use crate::nle::SYNTH_RATE;
    use std::sync::{Arc, Mutex};

    /// The tests' device: takes every sample; `consumed` lags what was pushed by `lag` frames.
    struct Out {
        pushed: Arc<Mutex<u64>>,
        lag: u64,
    }
    impl AudioOut for Out {
        fn push(&mut self, stereo: &[f32]) -> usize {
            *self.pushed.lock().unwrap() += (stereo.len() / 2) as u64;
            stereo.len()
        }
        fn consumed(&self) -> u64 {
            self.pushed.lock().unwrap().saturating_sub(self.lag)
        }
    }

    fn face(lag: u64) -> Face {
        let (tl, src) = synth_fixture();
        let out = Out { pushed: Arc::new(Mutex::new(0)), lag };
        Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false).with_audio(Box::new(out), SYNTH_RATE)
    }

    /// STRIAPLAYSYNCPACE M3: a device 5760 frames behind (120 ms, three frames at 25 fps): tick 1 seeds and shows 0, ticks
    /// 2-4 hold the picture on 0 (no repaint), then every tick shows the next frame at the clock: offsets 0.
    #[test]
    fn striaplaysyncpace_holds_ring_lead() {
        let mut f = face(5760);
        f.verb("play");
        assert!(f.tick());
        assert_eq!(f.state.playhead, 0);
        for n in 2..=4 {
            assert!(!f.tick(), "tick {n} holds");
            assert_eq!(f.state.playhead, 0, "tick {n}");
        }
        for n in 5..=8 {
            assert!(f.tick(), "tick {n} shows");
            assert_eq!(f.state.playhead, n - 4, "tick {n}");
        }
        let r = f.play_sync();
        assert_eq!((r.av_count, r.av_max_ns), (4, 0), "{r:?}");
        assert_eq!(f.play_pace().repeated, 3);
    }

    /// STRIAPLAYSYNCPACE M3: a stalled device (nothing consumed): the picture holds PACE_HOLD ticks, then moves one frame a
    /// tick, the queue never deeper than PACE_HOLD.
    #[test]
    fn striaplaysyncpace_stalled_device_moves() {
        let mut f = face(u64::MAX);
        f.verb("play");
        assert!(f.tick());
        for n in 2..=(PACE_HOLD + 1) {
            assert!(!f.tick(), "tick {n} holds");
            assert_eq!(f.state.playhead, 0);
        }
        assert!(f.tick());
        assert_eq!(f.state.playhead, 1);
        assert!(f.tick());
        assert_eq!(f.state.playhead, 2);
        assert_eq!(f.play.pace.queue.len(), PACE_HOLD);
    }

    /// STRIAPLAYSYNCPACE M3: no device: every tick shows its own render, nothing queued (today's path).
    #[test]
    fn striaplaysyncpace_no_device_passthrough() {
        let (tl, src) = synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        f.verb("play");
        for n in 0..3 {
            assert!(f.tick());
            assert_eq!(f.state.playhead, n);
            assert!(f.play.pace.queue.is_empty());
        }
    }

    /// STRIAPLAYSYNCPACE M3: stopped while the picture held on 0 (the render cursor at 4): play again shows frame 1, one
    /// frame from what was seen.
    #[test]
    fn striaplaysyncpace_resume_one_frame() {
        let mut f = face(5760);
        f.verb("play");
        for _ in 0..4 {
            f.tick();
        }
        assert_eq!(f.state.playhead, 0);
        assert!(f.on(&SMessage::BrowserText("k".into())));
        assert!(!f.playing);
        assert_eq!(f.state.playhead, 0);
        f.verb("play");
        assert!(f.tick());
        assert_eq!(f.state.playhead, 1);
    }
}

// ---- STRIAGAINENVELOPE (LEDGER SR9047): the gain envelope words and their note (named tail block) ----
// `env [<dB>]` / `env-` / `env+` / `env-del` / `env-clear` ride timeline_core's ONE verb dispatch (`set_selected_gain_env`
// through the one commit). After an edit the NOTE reads `env: <db_words> at <playhead>, <n> points` (the envelope's offset
// at the playhead's source tick, the point count); a refusal stays `<the word as typed>: <EditError>` on `last_error`.
impl Face {
    /// The env words and their note; `None` for any other word.
    fn env_face_verb(&mut self, v: &str) -> Option<bool> {
        v.split_ascii_whitespace().next().filter(|w| matches!(*w, "env" | "env-" | "env+" | "env-del" | "env-clear"))?;
        match self.state.verb(v) {
            Some(Ok(())) => {
                let ph = self.state.playhead;
                self.note = self.state.selected().map(|c| format!("env: {} at {ph}, {} points", db_words(c.gain_env.at(c.src_tick(ph))), c.gain_env.points().len()));
            }
            Some(Err(e)) => self.last_error = Some(format!("{v}: {e}")),
            None => return None,
        }
        Some(true)
    }
}

// ---- STRIATRANSITIONKERNEL (LEDGER SR9057): a released transition edge drag names its new length (named tail block) ----
// `release`'s commit goes through [`Face::finish_drag`]: `Drag::finish` as before, then for a `TransitionIn` /
// `TransitionOut` drag that committed, the SR991 note from [`Face::transition_note`] (`transition: HH:MM:SS:FF (<n> ticks)`).
// A refused drag sets none (the error stays `drag: <e>`); the next press drops the note (`clear_note`).

impl Face {
    /// The drag's ONE commit (`Drag::finish`); a committed transition edge drag sets the length note.
    fn finish_drag(&mut self, d: Drag) -> Result<(), EditError> {
        d.finish(&mut self.state)?;
        if matches!(d.kind, DragKind::TransitionIn | DragKind::TransitionOut) {
            self.note = self.transition_note();
        }
        Ok(())
    }
}

// ---- STRIAKMETER (LEDGER SR9058): the strip, hold and clip latch moved to timeline_core::view, both faces call them (named tail block) ----

#[cfg(test)]
mod striakmeter_tests {
    use super::*;

    /// M3: the host's strip and clip caps are timeline_core's, byte for byte, in both byte orders (the kernel Player's
    /// band paints the same fns); the host's hold is the view's type.
    #[test]
    fn striakmeter_host_paints_the_view_strip() {
        let mut st = EditorState::new(synth_fixture().0);
        st.seek(15);
        let l = Layout::of(320, 160, &st);
        let base = paint(&st, 320, 160, PixelOrder::Rgba, None, None);
        let levels = [MeterLevel { track: Some(1), peak_db: -6.0, rms_db: -30.0 }, MeterLevel { track: None, peak_db: 0.0, rms_db: -15.0 }];
        let clipped = [Some(1), None];
        for (po, vo) in [(PixelOrder::Rgba, Order::Rgba), (PixelOrder::Bgra, Order::Bgra)] {
            assert_eq!(paint_meters(base.clone(), l, po, &levels), view::paint_meters(base.clone(), l, vo, &levels));
            assert_eq!(paint_meter_clips(base.clone(), l, po, &clipped), view::paint_meter_clips(base.clone(), l, vo, &clipped));
        }
        let h: view::MeterHold = MeterHold::default();
        assert!(h.strip(false, levels.to_vec()).is_empty());
        assert_eq!(view::meter_hold_ticks(st.timeline.fps()), METER_HOLD_S * st.timeline.fps().unwrap_or(25).max(1));
    }
}

// ---- STRIATITLESTYLEFACE (LEDGER SR9077): the title step words' note on the message row (named tail block) ----
// The four title step words (timeline_core `editor::TITLE_STEP_WORDS`, the transport's Title - / Title + / Align / Ink
// buttons) reach timeline_core's ONE verb dispatch through `Face::verb`'s fall-through; after an Ok step the message row reads
// `title style: <size> <align> <RRGGBB>` from `note`, right after `last_error` in `message_text`. The next input clears it.

impl Face {
    /// STRIATITLESTYLEFACE: after an Ok word `v`, a title step word's note (`title style: 15 centre FFFFFF`); any other word
    /// leaves the note alone.
    fn title_style_note(&mut self, v: &str) {
        if !timeline_core::editor::TITLE_STEP_WORDS.contains(&v) {
            return;
        }
        if let Ok(s) = self.state.selected_title_style() {
            self.note = Some(format!("title style: {}", s.words()));
        }
    }
    /// STRIATITLESTYLEFACE: the note is cleared by the next input (a word, a click, a press, select-all, a named key, a typed
    /// character); a release, a move, a scroll or a resize keeps it.
    fn clear_note(&mut self, msg: &SMessage) {
        let input = match msg {
            SMessage::Input { .. } | SMessage::BrowserClick(..) | SMessage::BrowserSelectAll => true,
            SMessage::BrowserButton { down, .. } => *down,
            SMessage::BrowserKey(k) => !k.is_empty(),
            SMessage::BrowserText(t) => t.chars().count() == 1,
            _ => false,
        };
        if input {
            self.note = None;
        }
    }
}

// ---- STRIAXFORMHANDLES (LEDGER SR9068): the placed clip's corner handles on the preview picture (named tail block) ----
// NLE-XFORM-DRAG-HANDLES on the face: `paint_role`'s paint line hands [`Face::xform_handles`] to timeline_core's
// `view::paint_xform_handles`: the live placement drag (its dragged rect), else `view::XformDrag::rest` over the painted
// state, the layout the press reads and the preview still's size; nothing on the tracks Surface or under the help. A
// rest rect exists only for a placed, selected video clip shown at the playhead, so every other paint is byte-identical.
// NLE-XFORM-DRAG-SNAP's off switch: `xform_drag_on`'s press reads Alt (`bandy::MOD_ALT`) as the body press does.

impl Face {
    /// STRIAXFORMHANDLES (SR9068): the rect whose corners carry handles on `role`'s paint of `st` over layout `l`.
    fn xform_handles(&self, st: &EditorState, l: Layout, role: Role) -> Option<view::XformDrag> {
        if role == Role::Tracks || self.help {
            return None;
        }
        if let Some(d) = self.xform_drag {
            return Some(d);
        }
        let s = self.still.as_ref()?;
        view::XformDrag::rest(st, l, s.width, s.height)
    }
}

// ---- STRIADROPFRAMEFACE (LEDGER SR9078): typed timecode entry on the face — `in` / `out <tc>` and `goto <tc>` (named tail block) ----
// `in` / `out <timecode|frames>` set that mark at the typed tick (`Face::mark_verb`'s three folds; a typed timecode always
// sets, only the bare word toggles off) and `goto <timecode|frames>` seeks the playhead through `EditorState::seek` (a view
// word: no commit, no undo step, not dirty). The text is read by `timeline_core::edl::tc_entry` (THE typed reader: drop-frame
// `;` on a 29.97 / 59.94 base, a dropped number refused by name); a refusal is `last_error`, nothing moves. A bare `goto`
// names its usage. Stria-view: the `goto` button, and `--do goto=<tc>` (the rest of the token verbatim, colons and all).

/// STRIADROPFRAMEFACE (SR9078): `in <arg>` / `out <arg>` as (the word, its argument); any other text is `(v, None)`.
fn mark_arg(v: &str) -> (&str, Option<&str>) {
    match v.split_once(' ') {
        Some((w @ ("in" | "out"), a)) => (w, Some(a.trim())),
        _ => (v, None),
    }
}

/// STRIADROPFRAMEFACE (SR9078): the bare `goto`'s message (a button press, or the word with no argument).
pub const GOTO_USAGE: &str = "goto: usage: goto <timecode|frames>";

impl Face {
    /// `goto <timecode|frames>`: seeks the playhead to the typed tick (clamped to the timeline as every seek is). `None` for
    /// any other word.
    fn goto_verb(&mut self, v: &str) -> Option<bool> {
        let mut words = v.split_whitespace();
        if words.next() != Some("goto") {
            return None;
        }
        let args: Vec<&str> = words.collect();
        match args.as_slice() {
            [a] => match goto_target(a, self.state.playhead, &self.state.timeline) { // STRIAGOTOMORE (SR9097): `goto +<tc>` / `goto -<tc>` from the playhead (file tail)
                Ok(t) => self.state.seek(t),
                Err(m) => self.last_error = Some(format!("goto: {m}")),
            },
            _ => self.last_error = Some(String::from(GOTO_USAGE)),
        }
        Some(true)
    }
}

#[cfg(test)]
mod striadropframeface_tests {
    use super::*;

    /// `mark_arg` splits only `in` / `out` with an argument; every other word passes through whole.
    #[test]
    fn striadropframeface_mark_arg_splits_only_marks() {
        assert_eq!(mark_arg("in 00:01:00;02"), ("in", Some("00:01:00;02")));
        assert_eq!(mark_arg("out  1800"), ("out", Some("1800")));
        assert_eq!(mark_arg("in"), ("in", None));
        assert_eq!(mark_arg("kind W007"), ("kind W007", None));
        assert_eq!(mark_arg("tc frames"), ("tc frames", None));
    }
}

// ---- STRIASOURCEPLAY (LEDGER SR9080): the loaded source plays through the one play road; its ruler strip (named tail block) ----
// While a source is loaded the transport (play / Space, J K L, stop) drives the SOURCE playhead: [`Face::source_tick`] runs
// [`Face::tick`]'s own body ONCE over an editor holding timeline_core's `EditorState::source_reel` (the whole reel at tick 0,
// so a reel tick is a source tick) seeked to the source playhead, then swaps the editor back and keeps where it got as the
// source playhead. `play_tick`, `render` through `PlaySink`, the meters, the device and the end stop are the same code: a
// second playhead, never a second play road; the timeline playhead stays. The paint keys a played still on
// `EditorState::shown_tick`. A strip [`SRC_RULER_H`] px at the foot of the preview band shows the reel (its in / out range,
// the marks, the source playhead) ONLY while a source is loaded, and a press on it seeks the source (the one-Surface face
// and the split preview Surface, whose other pointer messages stay not taken). Unloaded: nothing painted, nothing taken.

/// The source ruler strip's height (px), at the foot of the preview band.
pub const SRC_RULER_H: u32 = 6;

/// The source ruler strip's rows `y0..y1` on layout `l` (the preview band's foot, after its message row is taken); `None`
/// on a band under two strips (Role::Tracks has none).
fn src_ruler_band(l: Layout) -> Option<(i64, i64)> {
    if l.preview_h < 2 * SRC_RULER_H {
        return None;
    }
    Some(((l.preview_h - SRC_RULER_H) as i64, l.preview_h as i64))
}

/// The strip over `px` while `st` has a source loaded: the reel across the width (RULER), its marked range (in or 0 .. out
/// or the duration, only when a mark is set) in TICK, each set mark a 1 px GUIDE line, the source playhead a 1 px PLAYHEAD
/// line. Nothing loaded (or no band): `px` unchanged.
fn paint_source_ruler(px: Vec<u8>, l: Layout, order: PixelOrder, st: &EditorState) -> Vec<u8> {
    let Some(m) = st.source.as_ref() else { return px };
    let Some(dur) = st.timeline.source(m.src).map(|s| s.duration.max(1)) else { return px };
    let Some((y0, y1)) = src_ruler_band(l) else { return px };
    let x_of = |t: i64| t.clamp(0, dur) * l.w as i64 / dur;
    let mut c = Canvas { w: l.w, h: l.h, order: vo(order), px };
    c.rect(0, y0, l.w as i64, y1, view::RULER);
    if m.marks != (None, None) {
        c.rect(x_of(m.marks.0.unwrap_or(0)), y0, x_of(m.marks.1.unwrap_or(dur)), y1, view::TICK);
    }
    for t in [m.marks.0, m.marks.1].into_iter().flatten() {
        let x = x_of(t).min(l.w as i64 - 1);
        c.rect(x, y0, x + 1, y1, GUIDE);
    }
    let x = x_of(m.at);
    c.rect(x, y0, x + 1, y1, PLAYHEAD);
    c.px
}

impl Face {
    /// One playback tick of the loaded source: [`Face::tick`]'s body over an editor holding the reel at the source
    /// playhead (no source loaded in it, so it does not come back here), the hover dwell kept as it was; the editor is
    /// swapped back and the source playhead set to where the tick left the reel. `None` when nothing is loaded.
    fn source_tick(&mut self) -> Option<bool> {
        let reel = self.state.source_reel()?;
        let at = self.state.source.as_ref()?.at;
        let mut inner = EditorState::new(reel);
        inner.seek(at);
        let outer = std::mem::replace(&mut self.state, inner);
        let hover = self.hover_ticks;
        let r = self.tick();
        self.hover_ticks = hover;
        let got = self.state.playhead;
        self.state = outer;
        let _ = self.state.source_seek(got);
        Some(r)
    }
    /// A press (a `BrowserButton` down, or a `BrowserClick` while the surface sends no buttons) on the source ruler strip
    /// seeks the source to the tick under it and is taken: on the one-Surface face (Role::Whole) or on the split preview
    /// Surface (Role::Preview). `None` for anything else, with nothing loaded, or while the help shows.
    fn source_ruler_input(&mut self, msg: &SMessage) -> Option<bool> {
        if self.help {
            return None;
        }
        let src = self.state.source.as_ref()?.src;
        let (x, y, n, button) = match *msg {
            SMessage::BrowserButton { down: true, x, y, surface, .. } => (x, y, surface, true),
            SMessage::BrowserClick(x, y, n) if !self.buttons => (x, y, n, false),
            _ => return None,
        };
        let role = match self.split {
            Some(s) if n == s.preview && s.preview != s.tracks => Role::Preview,
            Some(_) => return None,
            None if self.takes(msg) => Role::Whole,
            None => return None,
        };
        let l = self.layout_for(role).with_message(self.labels.is_some());
        let (y0, y1) = src_ruler_band(l)?;
        if x < 0.0 || x >= l.w as f64 || y < y0 as f64 || y >= y1 as f64 {
            return None;
        }
        let dur = self.state.timeline.source(src)?.duration;
        if button {
            self.buttons = true;
        }
        if let Err(e) = self.state.source_seek((x * dur as f64 / l.w as f64) as i64) {
            self.last_error = Some(format!("source: {e}"));
        }
        Some(true)
    }
}

#[cfg(test)]
mod striasourceplay_tests {
    use super::*;

    fn plain(w: u32, h: u32) -> Face {
        let (tl, src) = synth_fixture();
        Face::new(EditorState::new(tl), src, (w, h), PixelOrder::Rgba, false)
    }

    /// With B loaded, play steps the SOURCE playhead through the one play road (one render a tick, the paint shows that
    /// render, equal to the stopped preview of the same source tick); the timeline playhead, the document and undo stay.
    #[test]
    fn striasourceplay_play_steps_the_source_not_the_timeline() {
        let mut f = plain(320, 160);
        f.state.seek(4);
        let tl0 = f.state.timeline.clone();
        assert!(f.verb("source B") && f.verb("play"));
        assert!(f.playing);
        for _ in 0..3 {
            assert!(f.tick());
        }
        assert_eq!(f.state.source.as_ref().map(|m| m.at), Some(2), "three ticks show 0, 1, 2");
        assert_eq!(f.played().renders, 3);
        f.paint();
        let played = f.still().cloned();
        assert!(played.is_some());
        let want = preview(&f.state.source_view().unwrap(), &f.sources, 0).unwrap();
        assert_eq!(played, want, "the played render is B's frame 2");
        assert_eq!((f.state.playhead, &f.state.timeline, f.state.undo_depth(), f.dirty()), (4, &tl0, 0, false));
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        assert!(f.verb("play"));
        assert!(!f.tick(), "paused: nothing moves");
        assert_eq!(f.state.source.as_ref().map(|m| m.at), Some(2));
    }

    /// Play stops at the reel's last tick and the source playhead stays on it; unloaded, play is the timeline's again.
    #[test]
    fn striasourceplay_play_stops_at_the_reels_end() {
        let mut f = plain(320, 160);
        assert!(f.verb("source B"));
        assert_eq!(f.state.source_seek(47), Ok(()));
        assert!(f.verb("play"));
        for _ in 0..8 {
            f.tick();
        }
        assert!(!f.playing, "the reel's end stops play");
        assert_eq!((f.state.source.as_ref().map(|m| m.at), f.state.playhead), (Some(49), 0));
        assert!(f.verb("unload") && f.verb("play"));
        assert!(f.tick());
        assert!(f.state.playhead > 0 || f.played().renders > 0, "the timeline plays");
    }

    /// The strip paints only while a source is loaded, only on its rows; after unload the canvas is byte-identical.
    #[test]
    fn striasourceplay_ruler_paints_only_while_loaded() {
        let mut f = plain(320, 160);
        let off = f.paint();
        let l = f.layout_for(Role::Whole).with_message(false);
        let (y0, y1) = src_ruler_band(l).expect("a band");
        assert!(f.verb("source A") && f.verb("src-next") && f.verb("src-in"));
        assert_eq!(f.state.source_seek(40), Ok(()));
        assert!(f.verb("src-out"));
        let on = f.paint();
        let row = |y: i64| y as usize * 320 * 4;
        assert_eq!(on[row(y1)..], off[row(y1)..], "nothing under the strip");
        let px = |x: usize| &on[row(y0) + x * 4..row(y0) + x * 4 + 4];
        assert_eq!(px(319), view::RULER, "past out: the reel");
        assert_eq!(px(100), view::TICK, "inside in..out: the range");
        assert_eq!(px(40 * 320 / 50), PLAYHEAD, "the source playhead");
        assert!(f.verb("unload"));
        assert_eq!(f.paint(), off, "unloaded: byte-identical");
    }

    /// A press on the strip seeks the source (one Surface and the split preview Surface); above the strip, or with nothing
    /// loaded, the press is not the strip's.
    #[test]
    fn striasourceplay_ruler_press_seeks_the_source() {
        let mut f = plain(320, 160);
        let l = f.layout_for(Role::Whole).with_message(false);
        let (y0, _) = src_ruler_band(l).unwrap();
        let press = |x: f64, y: f64, s: u32| SMessage::BrowserButton { down: true, x, y, mods: 0, surface: s };
        assert_eq!(f.source_ruler_input(&press(160.0, y0 as f64 + 1.0, 0)), None, "nothing loaded");
        assert!(f.verb("source B"));
        assert!(f.on(&press(160.0, y0 as f64 + 1.0, 0)));
        assert_eq!((f.state.source.as_ref().map(|m| m.at), f.state.playhead), (Some(25), 0));
        assert_eq!(f.source_ruler_input(&press(160.0, y0 as f64 - 1.0, 0)), None, "above the strip");
        let mut s = plain(320, 160).on_surfaces(0, 1);
        assert!(s.verb("source B"));
        let lp = s.layout_for(Role::Preview).with_message(false);
        let (p0, _) = src_ruler_band(lp).unwrap();
        assert!(s.on(&press(80.0, p0 as f64 + 1.0, 0)));
        assert_eq!(s.state.source.as_ref().map(|m| m.at), Some(12));
        assert!(!s.on(&press(80.0, 2.0, 0)), "the preview Surface's other presses stay not taken");
    }

    /// The six Alt chords (GTK's BrowserKey spelling) drive the source words; their one-char text is not taken by them.
    #[test]
    fn striasourceplay_alt_chords_drive_the_source_words() {
        let mut f = plain(320, 160);
        let key = |f: &mut Face, k: &str| assert!(f.on(&SMessage::BrowserKey(k.into())), "{k}");
        assert!(f.state.select(0, 1).is_ok());
        f.state.seek(13);
        key(&mut f, "Alt+s");
        assert_eq!(f.state.source.as_ref().map(|m| (m.src, m.at)), Some((timeline_core::SourceId(1), 3)), "match frame");
        for k in ["Alt+Right", "Alt+i", "Alt+Right", "Alt+Right", "Alt+o", "Alt+Left"] {
            key(&mut f, k);
        }
        assert_eq!(f.state.source.as_ref().map(|m| (m.at, m.marks)), Some((5, (Some(4), Some(6)))));
        key(&mut f, "Alt+u");
        assert_eq!(f.state.source, None);
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        assert_eq!((f.state.undo_depth(), f.dirty()), (0, false));
    }
}

// ---- STRIAKDUR (LEDGER SR9089): dur- / dur+ on the host: buttons, _ / * and the length note (named tail block) ----
// timeline_core's `dur-` / `dur+` (SR9057's `transition_step_verb`: the selected clip's head transition, else its fade to
// black, else the selected transition) are VERBS words (stria-view's `Trans -` / `Trans +`) and KEYS `_` / `*` (the bytes
// the kernel Player binds). A step that commits sets SR991's note from [`Face::transition_note`] (the step selected the
// transition); a refusal is `last_error` `dur+: <e>` as before.

impl Face {
    /// `dur-` / `dur+` through THE dispatch, then the note on success; `None` for any other word.
    fn transition_step_note(&mut self, v: &str) -> Option<bool> {
        if v != "dur-" && v != "dur+" {
            return None;
        }
        match self.state.verb(v)? {
            Ok(()) => self.note = self.transition_note(),
            Err(e) => self.last_error = Some(format!("{v}: {e}")),
        }
        Some(true)
    }
}

// ---- STRIAGAINENVCURVE (LEDGER SR9083): the envelope curve word and its note (named tail block) ----
// `env-curve [linear|equal]` rides timeline_core's ONE verb dispatch (one commit). After it the NOTE reads
// `env curve: linear|equal-power, <n> points`; a refusal stays `<the word as typed>: <EditError>` on `last_error`.
impl Face {
    /// The env-curve word and its note; `None` for any other word.
    fn env_curve_face_verb(&mut self, v: &str) -> Option<bool> {
        v.split_ascii_whitespace().next().filter(|w| *w == "env-curve")?;
        match self.state.verb(v) {
            Some(Ok(())) => {
                self.note = self.state.selected().map(|c| {
                    let name = if c.gain_env.curve() == timeline_core::Curve::EqualPower { "equal-power" } else { "linear" };
                    format!("env curve: {name}, {} points", c.gain_env.points().len())
                });
            }
            Some(Err(e)) => self.last_error = Some(format!("{v}: {e}")),
            None => return None,
        }
        Some(true)
    }
}

// ---- STRIASOURCEDRAG (LEDGER SR9081): the loaded source's preview dragged onto a track (named tail block) ----
// While a source is loaded (SR9045's `EditorState::source`), a press on the preview begins THE drag model's source drag
// (`Drag::begin_source`): one Surface, a press in the band above the ruler; split, a press on the PREVIEW Surface, its moves
// and its release (the platform's implicit grab keeps them there) taken before `takes` and translated into the tracks
// Surface's pixels by the `on_surfaces` cut (the preview band directly above the tracks, same x). The existing `press` /
// `hover` / `release` bodies then run: the drag's preview state shows the clip laid at the drop, and the release's
// `Drag::finish` is ONE `EditorState::drop_source` (Overwrite; Shift at the press: Insert; off the tracks: nothing).
// `pointer` keeps the raw Surface pixels. Nothing loaded: every path is as before.

impl Face {
    /// One Surface: a press in the preview band with a source loaded begins the source drag; `None` otherwise.
    fn press_source(&mut self, x: f64, y: f64, mods: u8) -> Option<bool> {
        if self.split.is_some() {
            return None;
        }
        self.state.source.as_ref()?;
        let l = self.layout();
        if x < 0.0 || y < 0.0 || x >= l.w as f64 || y >= l.ruler_y as f64 {
            return None;
        }
        self.begin_source_drag(x, y, mods)
    }
    /// The source drag at `(x, y)` (the pointer Surface's pixels), a live drag dropped uncommitted; `None` with nothing loaded.
    fn begin_source_drag(&mut self, x: f64, y: f64, mods: u8) -> Option<bool> {
        let d = Drag::begin_source(&self.state, self.layout(), x, y, mods & bandy::MOD_SHIFT != 0, mods & bandy::MOD_ALT != 0)?;
        self.drag = Some(d);
        Some(true)
    }
    /// Split: the preview Surface's press (a source loaded) begins the source drag, and while one lives its moves and
    /// release follow it, in tracks pixels; `None` for every other message (the existing road).
    fn source_drag_input(&mut self, msg: &SMessage) -> Option<bool> {
        let s = self.split?;
        if msg.surface() != Some(s.preview) || s.preview == s.tracks {
            return None;
        }
        let dy = s.preview_canvas.1 as f64;
        let live = self.drag.is_some_and(|d| d.kind == DragKind::Source);
        let r = match *msg {
            SMessage::BrowserButton { down: true, x, y, mods, .. } => {
                self.state.source.as_ref()?;
                self.buttons = true;
                let r = self.begin_source_drag(x, y - dy, mods)?;
                self.pointer = Some((x, y));
                r
            }
            SMessage::BrowserMouseMove(x, y, _) if live => {
                let r = self.hover(x, y - dy);
                self.pointer = Some((x, y));
                r
            }
            SMessage::BrowserButton { down: false, x, y, .. } if live => {
                let r = self.release(x, y - dy);
                self.pointer = Some((x, y));
                r
            }
            _ => return None,
        };
        Some(r)
    }
}

#[cfg(test)]
mod striasourcedrag_tests {
    use super::*;
    use timeline_core::{Mode, SourceId};

    /// The synth fixture with B loaded for video and audio, marked 1..3.
    fn loaded() -> Face {
        let (tl, src) = synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(f.state.load_source(SourceId(1), &[Kind::Video, Kind::Audio]), Ok(()));
        f.state.source.as_mut().unwrap().marks = (Some(1), Some(3));
        f
    }

    fn laid(tl: &Timeline, at: i64, mode: Mode) -> Timeline {
        let c = Clip::new(SourceId(1), 1, 3, at);
        tl.place(0, c, mode).unwrap().place(1, c, mode).unwrap()
    }

    /// One Surface: a press in the preview band dragged onto V1 at tick 15 overwrites B 1..3 there on V1 and A1 at the
    /// release, in one undo step; nothing is committed while the pointer moves.
    #[test]
    fn striasourcedrag_preview_band_drop_overwrites() {
        let mut f = loaded();
        let tl0 = f.state.timeline.clone();
        let l = f.layout();
        assert!(10 < l.ruler_y, "the press is in the preview band");
        assert!(f.on(&SMessage::BrowserButton { down: true, x: 20.0, y: 10.0, mods: bandy::MOD_ALT, surface: 0 }));
        assert_eq!(f.dragging().map(|d| d.kind), Some(DragKind::Source));
        let (x, y) = l.point(0, 15);
        assert!(f.on(&SMessage::BrowserMouseMove(x, y, 0)));
        assert_eq!((&f.state.timeline, f.state.undo_depth()), (&tl0, 0));
        assert!(f.on(&SMessage::BrowserButton { down: false, x, y, mods: 0, surface: 0 }));
        assert!(f.last_error.is_none(), "{:?}", f.last_error);
        assert_eq!(f.state.timeline, laid(&tl0, 15, Mode::Overwrite));
        assert_eq!((f.state.undo_depth(), f.dragging().is_none()), (1, true));
    }

    /// Shift at the press is Insert; with nothing loaded a press in the preview band begins no drag, as before.
    #[test]
    fn striasourcedrag_shift_inserts_and_unloaded_is_unchanged() {
        let mut f = loaded();
        let tl0 = f.state.timeline.clone();
        let l = f.layout();
        assert!(f.on(&SMessage::BrowserButton { down: true, x: 20.0, y: 10.0, mods: bandy::MOD_SHIFT | bandy::MOD_ALT, surface: 0 }));
        let (x, y) = l.point(1, 15);
        f.on(&SMessage::BrowserMouseMove(x, y, 0));
        assert!(f.on(&SMessage::BrowserButton { down: false, x, y, mods: 0, surface: 0 }));
        assert_eq!(f.state.timeline, laid(&tl0, 15, Mode::Insert));
        assert!(f.verb("unload"));
        f.on(&SMessage::BrowserButton { down: true, x: 20.0, y: 10.0, mods: 0, surface: 0 });
        assert!(f.dragging().is_none(), "no source loaded: no drag");
    }

    /// Split: the preview Surface's press, move and release (its implicit grab) drop onto the tracks Surface below it;
    /// with nothing loaded the preview Surface's press is dropped, as before.
    #[test]
    fn striasourcedrag_split_preview_surface_drop() {
        let mut f = loaded().on_surfaces(0, 1);
        let tl0 = f.state.timeline.clone();
        let ph = f.split().unwrap().preview_canvas.1 as f64;
        let (x, yt) = f.layout().point(0, 15);
        assert!(f.on(&SMessage::BrowserButton { down: true, x: 20.0, y: 10.0, mods: bandy::MOD_ALT, surface: 0 }));
        assert_eq!(f.dragging().map(|d| d.kind), Some(DragKind::Source));
        assert!(f.on(&SMessage::BrowserMouseMove(x, yt + ph, 0)));
        assert_eq!(f.pointer(), Some((x, yt + ph)), "the raw Surface pixels");
        assert!(f.on(&SMessage::BrowserButton { down: false, x, y: yt + ph, mods: 0, surface: 0 }));
        assert_eq!(f.state.timeline, laid(&tl0, 15, Mode::Overwrite));
        assert_eq!(f.state.undo_depth(), 1);
        assert!(f.verb("unload"));
        assert!(!f.on(&SMessage::BrowserButton { down: true, x: 20.0, y: 10.0, mods: 0, surface: 0 }));
        assert!(f.dragging().is_none());
    }
}

// ---- STRIAPANAUTOWRITE (LEDGER SR9084): the pan reading at the playhead (named tail block) ----
// The pan words' and the pan drag's note read the pan the selected clip SHOWS at the playhead (`Clip::pan_at`: its curve
// when keyed, else its static pan) through SR897's [`pan_line`], with ` keys <n>` after it on a keyed clip.
impl Face {
    /// The message row's pan reading of the selected clip at the playhead (`pan: centre` with nothing selected).
    fn pan_reading(&self) -> String {
        let Some(c) = self.state.selected() else { return pan_line(0) };
        let n = c.pan_keys.n;
        let line = pan_line(c.pan_at(self.state.playhead));
        if n > 0 { format!("{line} keys {n}") } else { line }
    }
}

// ---- STRIAEXPORTRANGE (LEDGER SR9075): export marker / export markers as export-queue entries (named tail block) ----
// `export marker [<dir>] [fmt] [audio]` exports the range of the marker at or before the playhead (SR9052's
// `Timeline::marker_before`; its range `at..min(at + len, end)`) and `export markers [..]` every ranged marker in tick
// order, each as ONE `export_or_queue` call (SR881's road: idle starts at once, running queues with the snapshot), so
// export-skip / export-clear / export-list / Esc / the waiting note / the witness `queued` all apply unchanged. An entry
// is named by its marker through its directory `<dir>/<safe name>` (`export_safe_name`): export-list reads `M1 png ticks
// 12..20`, the result line ends `-> <dir>/M1`. The dir / format grammar is `export`'s own, lifted once into
// `Face::export_args` (the `export` verb calls it). Face words: no commit, no undo step, no dirty document. Refusals are
// `last_error` and queue nothing (`export markers` is all-or-nothing against `EXPORT_QUEUE_MAX`); the success is the note.
// Owed: NLE-EXPORT-RANGE-QUEUE-NAME (the result line is named by the dir, no `M1:` prefix), NLE-EXPORT-RANGE-QUEUE-KERNEL.

/// STRIAEXPORTRANGE (SR9075): a marker's name as one directory component: every char outside `[A-Za-z0-9_-]` becomes
/// `_` (an empty name is `_`), so no name climbs out of the export directory.
fn export_safe_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if s.is_empty() { "_".to_string() } else { s }
}

impl Face {
    /// The `export` grammar's directory and format words (`--all` and the audio word already taken): none = the default
    /// directory `<document's dir>/<stem>-export` and h264; a lone format word the format; a lone other word the directory;
    /// `<dir> <fmt>`. Refused: more words (`usage: ..`), no directory and no document path.
    fn export_args(&self, args: &[&str]) -> Result<(std::path::PathBuf, String), String> {
        let (dir, fmt) = match args {
            [] => (None, "h264"),
            [f] if VIDEO_NAMES.contains(f) => (None, *f),
            [d] => (Some(*d), "h264"),
            [d, f] => (Some(*d), *f),
            _ => return Err(format!("usage: {}", export_usage())), // NLE5DEXPORTMESSAGEAUDIO (SR756): the one usage
        };
        let dir = match (dir, &self.doc_path) {
            (Some(d), _) => std::path::PathBuf::from(d),
            (None, Some(p)) => p.with_file_name(format!("{}-export", p.file_stem().and_then(|s| s.to_str()).unwrap_or("untitled"))),
            (None, None) => return Err("no directory (no document path)".into()),
        };
        Ok((dir, fmt.to_string()))
    }
    /// The ranged markers' entries in tick order, `(name, safe name, at..min(at + len, end))`, and how many ranged markers
    /// lie past the timeline's end (an empty range: skipped).
    fn export_marker_ranges(&self) -> (Vec<(String, String, std::ops::Range<i64>)>, usize) {
        let end = self.state.timeline.end();
        let (mut v, mut past) = (Vec::new(), 0);
        for m in self.state.timeline.markers.iter().filter(|m| m.len > 0) {
            let r = m.at..(m.at + m.len).min(end);
            if r.is_empty() {
                past += 1;
            } else {
                v.push((m.name.clone(), export_safe_name(&m.name), r));
            }
        }
        (v, past)
    }
    /// The `export marker` / `export markers` words (before `export` takes them); `None` for any other word.
    fn export_marker_verb(&mut self, v: &str) -> Option<bool> {
        let mut words = v.split_whitespace();
        if words.next() != Some("export") {
            return None;
        }
        let all = match words.next() {
            Some("marker") => false,
            Some("markers") => true,
            _ => return None,
        };
        let word = if all { "export markers" } else { "export marker" };
        let (args, audio) = export_audio_word(words.collect());
        let ph = self.state.playhead;
        let (list, past) = if all {
            let (list, past) = self.export_marker_ranges();
            if list.is_empty() {
                self.last_error = Some(format!("{word}: no ranged marker"));
                return Some(true);
            }
            if let Some((_, safe, _)) = list.iter().enumerate().find(|(i, e)| list[..*i].iter().any(|p| p.1 == e.1)).map(|(_, e)| e) {
                self.last_error = Some(format!("{word}: two markers named {safe}"));
                return Some(true);
            }
            (list, past)
        } else {
            let end = self.state.timeline.end();
            let Some(m) = self.state.timeline.marker_before(ph).cloned() else {
                self.last_error = Some(format!("{word}: no marker at or before {}", self.marker_tc(ph)));
                return Some(true);
            };
            let r = m.at..(m.at + m.len).min(end);
            if m.len == 0 {
                self.last_error = Some(format!("{word}: {} is a point marker (no range)", m.name));
                return Some(true);
            }
            if r.is_empty() {
                self.last_error = Some(format!("{word}: {}'s range {}..{} is past the timeline's end {end}", m.name, m.at, m.at + m.len));
                return Some(true);
            }
            (vec![(m.name.clone(), export_safe_name(&m.name), r)], 0)
        };
        let room = EXPORT_QUEUE_MAX.saturating_sub(self.export_queue.len()) + usize::from(self.export.is_none());
        if list.len() > room {
            let (d, t) = self.export_progress().unwrap_or((0, 0));
            self.last_error = Some(if all { format!("{word}: {} ranges, room for {room}", list.len()) } else { format!("{word}: busy ({d}/{t})") });
            return Some(true);
        }
        let (base, fmt) = match self.export_args(&args) {
            Ok(x) => x,
            Err(e) => {
                self.last_error = Some(format!("{word}: {e}"));
                return Some(true);
            }
        };
        let audio = match audio {
            Ok(a) => a,
            Err(e) => {
                self.last_error = Some(format!("{word}: {e}"));
                return Some(true);
            }
        };
        if !VIDEO_NAMES.contains(&fmt.as_str()) {
            self.last_error = Some(format!("{word}: unknown format {fmt} ({})", VIDEO_NAMES.join("|")));
            return Some(true);
        }
        let mut started = None;
        for (i, (_, safe, r)) in list.iter().enumerate() {
            let dir = base.join(safe);
            let idle = self.export.is_none();
            if let Err(e) = self.export_or_queue(&dir, &fmt, Some(r.clone()), audio) {
                self.last_error = Some(if all { format!("{word}: {safe}: {e} ({i} queued before it)") } else { format!("{word}: {e}") });
                return Some(true);
            }
            if idle {
                started = Some(dir); self.mark_named(&Ok(())); // STRIAEXPORTRANGEBAND (SR9099): a lone marker entry started idle is named on the running row (file tail)
            }
        }
        if self.last_error.as_deref().is_some_and(|l| l.starts_with("export: queued")) {
            self.last_error = None; // SR881's `export: queued (<n> waiting)`: the note below says it
        }
        let n = self.export_queue.len();
        self.note = Some(if all {
            let names: Vec<&str> = list.iter().map(|e| e.0.as_str()).collect();
            let skipped = if past > 0 { format!(", {past} past the end skipped") } else { String::new() };
            format!("{word}: {} entries ({}){skipped}", list.len(), names.join(", "))
        } else {
            let (name, _, r) = &list[0];
            match &started {
                Some(d) => format!("{word}: {name} ticks {}..{} started -> {}", r.start, r.end, d.display()),
                None => format!("{word}: {name} ticks {}..{} queued ({n} waiting)", r.start, r.end),
            }
        });
        Some(true)
    }
}

// ---- STRIAPROXYWATCH (LEDGER SR9079): one sources generation; a reload releases and re-takes the shared sources, a job's result from before it is dropped (named tail block) ----
// SR9026's reload (`watch_sources`) reopens a reel through `Arc::get_mut` on the face's `Arc<Sources>`, so it runs only while
// no other clone lives. Two holes: SR997's decode worker held a clone for the face's whole life (a served face never
// reloaded), and a job whose thread had ended but was not yet joined let the reload through and then landed a result made
// from the OLD media (a proxy attached onto the new media, a head or a peak table cached for it). RULES:
// * GENERATION: `Face.sources_gen` moves once per reel reloaded; `Face.reloaded` keeps each source's last reload generation.
//   A job records `sources_gen` when it takes its clone; its result is STALE when its source (or, for a reader over the
//   whole timeline, any source: `None`) reloaded at a later generation (`stale_in`, the one rule; `Face::sources_stale`).
// * WORKER: `Bg` keeps no lifelong clone. `wave_fill` (every paint, before `Thumbs::fill`) parks a lease (`bg_lease`: the
//   clone and the generation) on it; each `BgJob` carries its own clone of the lease, dropped by the worker before its
//   result is sent; `watch_sources` releases the lease (`bg_release`) before its `Arc::get_mut`. A job in flight still holds
//   its clone, so the reload waits (the note) and the next acting input retries: it never blocks. `drain_bg` drops a stale
//   head or table with its placeholder, so the face asks again at the reload's generation.
// * PROXY: `finish_proxy` asks `proxy_stale` first: a stale job attaches nothing; not cancelled, its reel goes to the front of
//   SR915's `proxy_queue` (`proxy: <reel>: built before the reel reloaded; rebuilds`, then SR882's one `proxy_start`).
// No lock: the generation is the face thread's own field, read at the clone and at the landing on that thread. Owed:
// NLE-PROXY-WATCH-WORKER-SCAN (the normalize and loudness scans record `sources_gen` at their clone and their ticks drop a
// stale result through `sources_stale`), NLE-PROXY-WATCH-EXPORT (an export started before a reload finishes on the old media),
// NLE-PROXY-WATCH-CACHE (a reloaded source's heads and table cached BEFORE the reload stay until evicted).

/// STRIAPROXYWATCH (LEDGER SR9079): a result taken at generation `sgen` is stale when `src` (any source: `None`) reloaded later.
fn stale_in(reloaded: &[(timeline_core::SourceId, u64)], src: Option<timeline_core::SourceId>, sgen: u64) -> bool {
    reloaded.iter().any(|(s, g)| *g > sgen && src.is_none_or(|x| x == *s))
}

impl Face {
    /// STRIAPROXYWATCH (LEDGER SR9079): a job's result taken at `sgen` is stale ([`stale_in`] over this face's reloads).
    fn sources_stale(&self, src: Option<timeline_core::SourceId>, sgen: u64) -> bool {
        stale_in(&self.reloaded, src, sgen)
    }
    /// STRIAPROXYWATCH (LEDGER SR9079): park the sources' clone and generation on the decode worker (none: nothing).
    fn bg_lease(&mut self) {
        if self.peaks.thumbs.bg.is_some() {
            let lease = (Arc::clone(&self.sources), self.sources_gen);
            if let Some(bg) = self.peaks.thumbs.bg.as_mut() {
                bg.lease = Some(lease);
            }
        }
    }
    /// STRIAPROXYWATCH (LEDGER SR9079): drop the decode worker's parked clone (the reload's `Arc::get_mut` needs it gone).
    fn bg_release(&mut self) {
        if let Some(bg) = self.peaks.thumbs.bg.as_mut() {
            bg.lease = None;
        }
    }
    /// STRIAPROXYWATCH (LEDGER SR9079): a proxy job cloned before its reel reloaded: joined and discarded (true); not
    /// cancelled, its reel rebuilds first on SR915's queue. A current job: false (`finish_proxy` attaches it).
    fn proxy_stale(&mut self, j: &mut ProxyJob) -> bool {
        if !self.sources_stale(Some(j.src), j.sgen) {
            return false;
        }
        let _ = j.handle.take().map(|h| h.join());
        if j.cancel.load(Relaxed) {
            self.last_error = Some(format!("proxy: {}: cancelled (the reel reloaded)", j.reel));
        } else {
            self.last_error = Some(format!("proxy: {}: built before the reel reloaded; rebuilds", j.reel));
            self.proxy_queue.insert(0, (j.src, j.reel.clone()));
        }
        self.proxy_next();
        true
    }
}

// ---- STRIASTACKTOOL (LEDGER SR9093): the message row names the layer count under the playhead (named tail block) ----
// NLE-STACK-FACE. When the playhead's video step carries a stack ([`timeline_core::schedule::Step::layers`] above two),
// `message_text` reads `stack: <n> layers` after `last_error`, the note and the export job line, before the reel line.
// View-only: no commit, no dirty, no undo; two layers or fewer (or a timeline that does not schedule) say nothing, so every
// golden and every message line is unchanged. One one-tick `schedule::schedule` per call (owed NLE-STACK-FACE-CACHE).

impl Face {
    /// STRIASTACKTOOL: `stack: <n> layers` when the step at the playhead shows more than two video layers, else `None`.
    pub fn stack_note(&self) -> Option<String> {
        let ph = self.state.playhead;
        if ph < 0 {
            return None;
        }
        let steps = timeline_core::schedule::schedule(&self.state.timeline, ph..ph + 1, &*self.sources).ok()?;
        let n = steps.iter().find(|s| s.kind == Kind::Video)?.layers();
        (n > 2).then(|| format!("stack: {n} layers"))
    }
}

// ---- STRIAGAINENVDRAG (LEDGER SR9082): the envelope drag's press and its note (named tail block) ----
// A press with no Shift / Ctrl / Cmd on the selected audio clip's painted envelope begins timeline_core's envelope drag
// (`Drag::begin_gain_env` through `Drag::begin_grab`, STRIAKGAINENV SR9111, before `begin_with`); the release's generic `Drag::finish` is its ONE
// `set_selected_gain_env`, `Drag::shown` its preview. After the commit the NOTE reads `env: <db_words> at <tick>, <n> points`
// (the landed point's offset and timeline tick, the point count); a refusal stays `drag: <EditError>` on `last_error`.
impl Face {
    /// Where an envelope drag's point lands `(source tick, offset)`, read before its commit; `None` for any other drag.
    fn gain_env_land(&self, d: &Drag) -> Option<(i64, i32)> {
        if d.kind != DragKind::GainEnv {
            return None;
        }
        d.gain_env_landing(&self.state).ok().map(|(_, s, v)| (s, v))
    }
    /// The note after an envelope drag's commit.
    fn gain_env_drag_note(&mut self, land: Option<(i64, i32)>) {
        let (Some((s, v)), Some(c)) = (land, self.state.selected()) else { return };
        self.note = Some(format!("env: {} at {}, {} points", db_words(v), c.at + (s - c.src_in), c.gain_env.points().len()));
    }
}

// ---- STRIANORMALIZELUFSMORE (LEDGER SR9086): Esc cancels the clip measure, its progress line, and normalize over a selection in one undo step (named tail block) ----
// The `normalize lufs` scan now carries SR903's `LoudnessWatch` (one per clip, all sharing ONE cancel flag): Esc's
// `cancel` sets the flag (`RenderSink::stop` reads it before the next step's decode), the join names it `cancelled` by
// the job's own flag (the timeline unmoved), and `serve`'s clock puts `normalize: measuring <done>/<total>` on the
// message row while it runs. Two or more selected AUDIO clips (`EditorState.set`; video clips skipped) are each
// measured alone (`loudness_clip`, or SR878's exact peak through `peak_mix` for the dBFS word), each gain by THE rule
// (`normalize_lufs_gain_cdb` / `normalize_gain_cdb`), and all set by timeline_core's `EditorState::set_gains`: one
// commit, one undo step; a silent clip keeps its gain. The peak scan has no stop hook (`peak_mix`): Esc leaves it.

/// STRIANORMALIZELUFSMORE (SR9086): what a normalize scan measures: every clip of the run (re-checked at the join) and,
/// for a loudness measure, one SR903 watch per clip sharing ONE cancel flag (a peak scan carries no watch).
struct LufsRun {
    cancel: Arc<AtomicBool>,
    watches: Vec<crate::nle::LoudnessWatch>,
    clips: Vec<((usize, usize), Clip)>,
    total: u64,
    shown: String,
}

impl LufsRun {
    /// A run over `clips`; `watched` gives each clip a watch over its own ticks (clip_alone's range).
    fn new(clips: &[((usize, usize), Clip)], watched: bool) -> LufsRun {
        let cancel = Arc::new(AtomicBool::new(false));
        let watches = if watched { clips.iter().map(|(_, c)| crate::nle::LoudnessWatch { start: c.at, done: Arc::new(AtomicU64::new(0)), cancel: Arc::clone(&cancel) }).collect() } else { Vec::new() };
        let total = clips.iter().map(|(_, c)| (c.end() - c.at).max(0) as u64).sum();
        LufsRun { cancel, watches, clips: clips.to_vec(), total, shown: String::new() }
    }
    /// True once Esc (or `cancel`) asked the run to stop.
    fn cancelled(&self) -> bool {
        self.cancel.load(Relaxed)
    }
    /// The ticks measured so far over every clip (each clamped to its clip's ticks).
    fn done(&self) -> u64 {
        self.watches.iter().zip(&self.clips).map(|(w, (_, c))| w.done.load(Relaxed).min((c.end() - c.at).max(0) as u64)).sum()
    }
    /// True when every clip of the run still stands where it was measured.
    fn stands(&self, tl: &Timeline) -> bool {
        self.clips.iter().all(|&((t, i), c)| tl.tracks.get(t).and_then(|tr| tr.clips.get(i)) == Some(&c))
    }
}

/// STRIANORMALIZELUFSMORE (SR9086): `normalize: measuring <ticks done>/<total>` (done clamped to the total).
pub fn normalize_progress_line(done: u64, total: u64) -> String {
    format!("normalize: {NORMALIZE_SCANNING} {}/{total}", done.min(total))
}

impl Face {
    /// Esc's arm for a running loudness measure: set its cancel flag; false when none runs (a peak scan has no stop).
    pub fn cancel_normalize(&mut self) -> bool {
        let Some(run) = self.scan.job.as_ref().and_then(|j| j.lufs.as_ref()).filter(|r| !r.watches.is_empty()) else { return false };
        run.cancel.store(true, Relaxed);
        true
    }
    /// The running loudness measure's count on the message row ([`normalize_progress_line`]); true (a repaint) only
    /// when the line moved since the last one put up. A cancelled run shows nothing more until its join.
    pub fn normalize_progress(&mut self) -> bool {
        let Some(run) = self.scan.job.as_mut().and_then(|j| j.lufs.as_mut()).filter(|r| !r.watches.is_empty()) else { return false };
        if run.cancelled() {
            return false;
        }
        let line = normalize_progress_line(run.done(), run.total);
        if line == run.shown {
            return false;
        }
        run.shown = line.clone();
        self.last_error = Some(line);
        true
    }
    /// The selected audio clips (`EditorState.set`, video clips skipped) when there are two or more; else `None`.
    fn normalize_set(&self) -> Option<Vec<((usize, usize), Clip)>> {
        let tl = &self.state.timeline;
        let clips: Vec<((usize, usize), Clip)> = self.state.set.iter().filter(|&&(t, _)| tl.tracks.get(t).map(|tr| tr.kind) == Some(Kind::Audio)).filter_map(|&(t, i)| Some(((t, i), *tl.tracks.get(t)?.clips.get(i)?))).collect();
        (clips.len() >= 2).then_some(clips)
    }
    /// `normalize lufs` over a selection: `None` unless two or more audio clips are selected; else measured here
    /// (unserved) or on one thread clip by clip (served: `measuring`, `busy` while a scan runs).
    fn normalize_lufs_many(&mut self, target: i32) -> Option<Result<String, String>> {
        let clips = self.normalize_set()?;
        if !self.scan.thread {
            let mut rs = Vec::with_capacity(clips.len());
            for &((t, i), _) in &clips {
                match crate::nle::loudness_clip(&self.state.timeline, t, i, &self.sources, None) {
                    Ok(r) => rs.push(r),
                    Err(e) => return Some(Err(e.to_string())),
                }
            }
            let run = LufsRun::new(&clips, false);
            return Some(self.normalize_lufs_many_finish(Some(&run), rs, target));
        }
        if self.scan.job.is_some() {
            return Some(Err("busy".to_string()));
        }
        let (track, index) = self.state.selection?;
        let c = self.state.selected()?;
        let run = LufsRun::new(&clips, true);
        let (tl, sources, ws) = (self.state.timeline.clone(), self.sources.clone(), run.watches.clone());
        let at: Vec<(usize, usize)> = clips.iter().map(|&(p, _)| p).collect();
        let handle = std::thread::spawn(move || -> Result<Option<NormalizeRead>, String> {
            let mut rs = Vec::with_capacity(at.len());
            for (&(t, i), w) in at.iter().zip(ws) {
                if w.cancel.load(Relaxed) {
                    rs.push(None);
                    continue;
                }
                rs.push(crate::nle::loudness_clip(&tl, t, i, &sources, Some(w)).map_err(|e| e.to_string())?);
            }
            Ok(Some(NormalizeRead::LufsMany(rs)))
        });
        self.scan.job = Some(NormalizeJob { track, index, clip: c, target, os: 0, lufs: Some(run), handle });
        Some(Err(NORMALIZE_SCANNING.to_string()))
    }
    /// The dBFS `normalize` over a selection (NLE-NORMALIZE-PEAK-MULTI): `None` unless two or more audio clips are
    /// selected; each clip's own exact peak at the mix's rate (SR9035's `peak_mix`, `os` SR963's oversampling).
    fn normalize_peak_many(&mut self, target: i32, os: u32) -> Option<Result<String, String>> {
        let clips = self.normalize_set()?;
        let tb = self.state.timeline.tb;
        let spans: Vec<(timeline_core::SourceId, std::ops::Range<i64>)> = clips.iter().map(|(_, c)| (c.src, tb.to_ns(c.src_in)..tb.to_ns(c.src_out))).collect();
        if !self.scan.thread {
            let mut ps = Vec::with_capacity(spans.len());
            for (src, ns) in &spans {
                match crate::nle::peak_mix(&self.sources, *src, ns.clone(), os) {
                    Ok(p) => ps.push(p),
                    Err(e) => return Some(Err(e.to_string())),
                }
            }
            let run = LufsRun::new(&clips, false);
            return Some(self.normalize_peak_many_finish(Some(&run), ps, target, os));
        }
        if self.scan.job.is_some() {
            return Some(Err("busy".to_string()));
        }
        let (track, index) = self.state.selection?;
        let c = self.state.selected()?;
        let sources = self.sources.clone();
        let handle = std::thread::spawn(move || -> Result<Option<NormalizeRead>, String> {
            let mut ps = Vec::with_capacity(spans.len());
            for (src, ns) in spans {
                ps.push(crate::nle::peak_mix(&sources, src, ns, os).map_err(|e| e.to_string())?);
            }
            Ok(Some(NormalizeRead::PeakMany(ps)))
        });
        self.scan.job = Some(NormalizeJob { track, index, clip: c, target, os, lufs: Some(LufsRun::new(&clips, false)), handle });
        Some(Ok(NORMALIZE_SCANNING.to_string()))
    }
    /// A selection's loudness readings (the run's order) to gains by [`normalize_lufs_gain_cdb`], set in one commit.
    fn normalize_lufs_many_finish(&mut self, run: Option<&LufsRun>, rs: Vec<Option<crate::nle::LoudnessReading>>, target: i32) -> Result<String, String> {
        let gains = rs.into_iter().map(|r| r.and_then(|r| normalize_lufs_gain_cdb(&r, target))).collect();
        self.normalize_set_finish(run, gains, "lufs")
    }
    /// A selection's peaks (the run's order) to gains by [`normalize_gain_cdb`], set in one commit.
    fn normalize_peak_many_finish(&mut self, run: Option<&LufsRun>, ps: Vec<Option<f32>>, target: i32, os: u32) -> Result<String, String> {
        let gains = ps.into_iter().map(|p| p.and_then(|p| normalize_gain_cdb(p, target)).map(|g| (g, false))).collect();
        self.normalize_set_finish(run, gains, if os > 1 { "true peak" } else { "peak" })
    }
    /// Set each measurable clip's gain through `EditorState::set_gains` (ONE commit, one undo step; a silent clip
    /// keeps its gain) and the line `<what> <n> clips -> gains <g1>, <g2>, ...[ (<k> true-peak bound)][ (<s> silent)]`;
    /// `the selection moved` when any clip of the run no longer stands, `silent clip` when none is measurable.
    fn normalize_set_finish(&mut self, run: Option<&LufsRun>, gains: Vec<Option<(i32, bool)>>, what: &str) -> Result<String, String> {
        let run = run.ok_or_else(|| "scan failed".to_string())?;
        if !run.stands(&self.state.timeline) {
            return Err("the selection moved".to_string());
        }
        let set: Vec<((usize, usize), i32)> = run.clips.iter().zip(&gains).filter_map(|(&(p, _), g)| (*g).map(|(cdb, _)| (p, cdb))).collect();
        if set.is_empty() {
            return Err("silent clip".to_string());
        }
        self.state.set_gains(&set).map_err(|e| e.to_string())?;
        let bound = gains.iter().flatten().filter(|g| g.1).count();
        let silent = gains.len() - set.len();
        let words: Vec<String> = set.iter().map(|&(_, g)| db_words(g)).collect();
        let mut line = format!("{what} {} clips -> gains {}", gains.len(), words.join(", "));
        if bound > 0 {
            line.push_str(&format!(" ({bound} true-peak bound)"));
        }
        if silent > 0 {
            line.push_str(&format!(" ({silent} silent)"));
        }
        Ok(line)
    }
}

// ---- STRIAPACETAIL (LEDGER SR9087): NLEPLAY-SYNC-PACE-TAIL + NLEPLAY-PACE-STILLMEM, the paced queue at the end, across a rate change, held small (named tail block) ----
// At the timeline's end the render cursor stops while the presented playhead is still behind: a DRAIN tick (no render, no
// push, no flush) feeds the parent's AudioClock (`sync_mark`) and re-enters the parent's `pace` with the queue's last entry
// handed back as `pending`, so the ONE decide loop shows the head the clock reached; the queue empty, play stops at the end as
// before; more than 2 * PACE_HOLD drain ticks (a stalled device) stop as before. A shuttle or reverse tick clears the queue; a
// forward shuttle with stills queued starts one frame after the presented tick (the ring flushed by `play_tick`'s jump test).
// Stills left in the queue after a tick are sampled down to the preview's letterboxed box (never up), which paints the same
// bytes; `Face::still_is_sampled` says when the shown still is one of them. Wire: `[STRIA] :: nle play pace tail ticks= left=`.
// FOLD RULE: a full-size reader of `Face::still()` (SR830's frame export) re-renders through `preview` at the playhead when
// `still_is_sampled()` is true. Owed: NLEPLAY-PACE-GAP (a gap tick's `NoSize` render clears the queue mid-run).

/// STRIAPACETAIL (SR9087): `s` nearest-sampled to the size `view::Canvas::blit_fit` paints it at in a `bw` x `bh` box
/// (`SizePolicy::Letterbox`'s placed rect, the same samples through `blit_policy`'s ONE `nearest_rows`); `None` when nothing
/// is sized or the placed rect is not smaller than `s` (a sampled still is never sampled twice).
fn pace_small(s: &Still, bw: u32, bh: u32) -> Option<Still> {
    let (sw, sh) = (s.width as usize, s.height as usize);
    if sw == 0 || sh == 0 || bw == 0 || bh == 0 || s.rgba.len() < sw * sh * 4 {
        return None;
    }
    let d = timeline_core::SizePolicy::Letterbox.place(sw, sh, bw as usize, bh as usize).dst;
    if d.w * d.h >= sw * sh {
        return None;
    }
    let (dw, dh) = (d.w as u32, d.h as u32);
    let mut c = Canvas::new(dw, dh, Order::Rgba);
    c.blit_policy(s, 0, 0, dw, dh, timeline_core::SizePolicy::Stretch);
    Some(Still { width: dw, height: dh, rgba: c.px })
}

impl Face {
    /// STRIAPACETAIL (SR9087): a forward tick that hit the timeline's end at render cursor `at`: while stills are queued and the
    /// clock lives, the parent's `pace` shows the head the clock reached (or holds); true when this was a drain tick. False
    /// (play stops as before) when the queue is empty, the clock is gone or the drain ran past 2 * PACE_HOLD ticks.
    fn pace_tail(&mut self, at: i64) -> bool {
        let pace = &self.play.pace;
        if pace.queue.is_empty() || pace.tail >= 2 * PACE_HOLD {
            if pace.tail > 0 {
                log::info!("[STRIA] :: nle play pace tail ticks={} left={}", pace.tail, pace.queue.len());
            }
            return false;
        }
        self.sync_mark(at);
        if self.play.sync.clock.is_none() {
            return false;
        }
        self.play.pace.tail += 1;
        let Some((t, e, s)) = self.play.pace.queue.pop_back() else { return false };
        self.play.pace.next = Some(t);
        (self.play.shown, self.play.pending) = (Some(t), Some((t, s)));
        self.pace(e);
        true
    }
    /// STRIAPACETAIL (SR9087): at the top of a played tick at a rate other than 1x: the pace queue and the presented tick are
    /// dropped; a forward shuttle with stills still queued first moves the playhead to one frame after the presented tick and
    /// forgets `shown`, so the run starts from what was seen and `play_tick` flushes the ring (its render cursor is ahead).
    fn pace_clear(&mut self) {
        let (queued, on) = (!self.play.pace.queue.is_empty(), self.play.pace.on);
        if self.rate == 1 || (!queued && on.is_none()) {
            return;
        }
        if let Some((t, e)) = on {
            if self.rate > 1 && queued && self.play.shown == Some(t) {
                self.state.seek(e);
                self.play.shown = None;
            }
        }
        let pace = &mut self.play.pace;
        pace.queue.clear();
        (pace.next, pace.on, pace.fresh) = (None, None, true);
    }
    /// STRIAPACETAIL (SR9087): after a rendered forward tick's `pace`: the drain count restarts, and every still left in the
    /// queue is sampled down to the preview's box (`pace_small`), its tick and size kept for `still_is_sampled`.
    fn pace_queued(&mut self) {
        let shown = self.play.shown;
        let pace = &mut self.play.pace;
        pace.tail = 0;
        let (small, queue) = (&mut pace.small, &pace.queue);
        small.retain(|&(t, _, _)| Some(t) == shown || queue.iter().any(|q| q.0 == t));
        if pace.queue.is_empty() {
            return;
        }
        let role = if self.split.is_some() { Role::Preview } else { Role::Whole };
        let l = self.layout_for(role).with_message(self.labels.is_some());
        let pace = &mut self.play.pace;
        for q in pace.queue.iter_mut() {
            if let Some(s) = q.2.as_ref().and_then(|s| pace_small(s, l.w, l.preview_h)) {
                pace.small.push((q.0, s.width, s.height));
                q.2 = Some(s);
            }
        }
    }
    /// STRIAPACETAIL (SR9087): true when the still the face shows ([`Face::still`]) is a queued still sampled down to the
    /// preview's size (it paints the same bytes); a full-size reader re-renders through [`preview`] at the playhead then.
    pub fn still_is_sampled(&self) -> bool {
        let ph = self.state.playhead;
        self.still.as_ref().is_some_and(|s| self.play.pace.small.iter().any(|&(t, w, h)| t == ph && (w, h) == (s.width, s.height)))
    }
}

#[cfg(test)]
mod striapacetail_tests {
    use super::*;
    use crate::media::AudioOut;
    use crate::nle::SYNTH_RATE;
    use std::sync::{Arc, Mutex};

    /// The tests' device: takes every sample; `consumed` lags what was pushed by the shared `lag` frames.
    struct Out {
        pushed: u64,
        lag: Arc<Mutex<u64>>,
    }
    impl AudioOut for Out {
        fn push(&mut self, stereo: &[f32]) -> usize {
            self.pushed += (stereo.len() / 2) as u64;
            stereo.len()
        }
        fn consumed(&self) -> u64 {
            self.pushed.saturating_sub(*self.lag.lock().unwrap())
        }
    }

    fn face(lag: u64, canvas: (u32, u32)) -> (Face, Arc<Mutex<u64>>) {
        let (tl, src) = synth_fixture();
        let lag = Arc::new(Mutex::new(lag));
        let out = Out { pushed: 0, lag: lag.clone() };
        (Face::new(EditorState::new(tl), src, canvas, PixelOrder::Rgba, false).with_audio(Box::new(out), SYNTH_RATE), lag)
    }

    /// Play from tick 25 (off the fixture's gap) with the device `lag` frames behind and tick `n` times.
    fn played(lag: u64, canvas: (u32, u32), n: usize) -> (Face, Arc<Mutex<u64>>) {
        let (mut f, l) = face(lag, canvas);
        f.state.seek(25);
        f.verb("play");
        for _ in 0..n {
            f.tick();
        }
        (f, l)
    }

    /// STRIAPACETAIL M3: three frames behind, the render reaches the end (35) with 32..34 queued: each drain tick shows the
    /// next frame as the device plays out, nothing more is pushed, and the tick after the last stops at the end.
    #[test]
    fn striapacetail_tail_drains() {
        let (mut f, lag) = played(5760, (320, 160), 10);
        assert_eq!(f.state.playhead, 31);
        let pushed = f.played().pushed;
        assert!(f.tick());
        assert_eq!(f.state.playhead, 32);
        *lag.lock().unwrap() = 3840;
        assert!(f.tick());
        assert_eq!(f.state.playhead, 33);
        *lag.lock().unwrap() = 1920;
        assert!(f.tick());
        assert_eq!(f.state.playhead, 34);
        assert_eq!(f.played().pushed, pushed, "a drain tick renders and pushes nothing");
        assert!(f.playing);
        assert!(f.tick());
        assert!(!f.playing);
        assert_eq!(f.state.playhead, 35);
    }

    /// STRIAPACETAIL M3: a stalled device at the end: 2 * PACE_HOLD drain ticks hold the picture (no repaint), then play
    /// stops at the end as before.
    #[test]
    fn striapacetail_tail_bounded() {
        let (mut f, _) = played(u64::MAX, (320, 160), 10);
        assert_eq!(f.state.playhead, 25);
        for n in 0..2 * PACE_HOLD {
            assert!(!f.tick(), "drain tick {n} holds");
            assert_eq!(f.state.playhead, 25);
        }
        assert!(f.tick());
        assert!(!f.playing);
        assert_eq!(f.state.playhead, 35);
    }

    /// STRIAPACETAIL M3: reverse while 26..28 are queued behind the shown 25: the queue is dropped and the reverse tick
    /// steps back from the frame that was seen.
    #[test]
    fn striapacetail_reverse_clears() {
        let (mut f, _) = played(5760, (320, 160), 4);
        assert_eq!((f.state.playhead, f.play.pace.queue.len()), (25, 3));
        assert!(f.on(&SMessage::BrowserText("j".into())));
        assert_eq!(f.rate, -1);
        assert!(f.tick());
        assert!(f.play.pace.queue.is_empty());
        assert!(f.play.pace.on.is_none());
        assert_eq!(f.state.playhead, 24);
    }

    /// STRIAPACETAIL M3: a 2x shuttle while 26..28 are queued behind the shown 25 starts at 26 (not at the render cursor,
    /// 29), the queue dropped; the next tick steps two frames.
    #[test]
    fn striapacetail_shuttle_from_shown() {
        let (mut f, _) = played(5760, (320, 160), 4);
        assert_eq!(f.state.playhead, 25);
        assert!(f.on(&SMessage::BrowserText("l".into())));
        assert_eq!(f.rate, 2);
        assert!(f.tick());
        assert_eq!(f.state.playhead, 26);
        assert!(f.play.pace.queue.is_empty());
        assert!(f.tick());
        assert_eq!(f.state.playhead, 28);
    }

    /// STRIAPACETAIL M3: on a 32 px wide face (preview box 32 x 64) the held stills are 32 x 24, not the reel's 64 x 48; the
    /// first one shown later is marked sampled; a run with no lag shows full stills.
    #[test]
    fn striapacetail_queue_held_small() {
        let (mut f, _) = played(5760, (32, 160), 4);
        assert_eq!(f.play.pace.queue.len(), 3);
        for q in &f.play.pace.queue {
            let s = q.2.as_ref().expect("a queued still");
            assert_eq!((s.width, s.height), (32, 24), "tick {}", q.0);
        }
        assert!(f.tick());
        assert_eq!(f.state.playhead, 26);
        let _ = f.frames();
        assert_eq!(f.still().map(|s| (s.width, s.height)), Some((32, 24)));
        assert!(f.still_is_sampled());
        let (mut g, _) = played(0, (32, 160), 2);
        let _ = g.frames();
        assert_eq!(g.still().map(|s| (s.width, s.height)), Some((64, 48)));
        assert!(!g.still_is_sampled());
    }

    /// STRIAPACETAIL M3: a still sampled to the preview box paints the same bytes as the full render; sampling it again is a no-op.
    #[test]
    fn striapacetail_small_paints_same() {
        let (tl, src) = synth_fixture();
        let full = preview(&tl, &src, 1).unwrap().expect("a frame at 1");
        let st = EditorState::new(tl);
        let l = Layout::of(32, 160, &st);
        let small = pace_small(&full, l.w, l.preview_h).expect("smaller than the reel");
        assert_eq!((small.width, small.height), (32, 24));
        assert_eq!(paint_in(&st, l, PixelOrder::Rgba, None, Some(&small)), paint_in(&st, l, PixelOrder::Rgba, None, Some(&full)));
        assert!(pace_small(&small, l.w, l.preview_h).is_none());
    }
}

// ---- STRIAGOTOMORE (LEDGER SR9097): relative `goto`, timecode amounts and the face's entry line (named tail block) ----
// `goto +<tc>` / `goto -<tc>` seek from the playhead (`goto_target`, through `timeline_core::edl::tc_delta`, the signed form
// of THE typed reader `tc_entry`; the seek clamps as every seek does). `slip` / `slide <tc>` and `rtrim in|out <tc>` read a
// signed timecode through the same `tc_delta` and are rewritten to editor.rs's integer words, then go through `Face::verb` again
// (THE dispatch, ONE commit); an integer or a text with no `:` / `;` keeps the old road. The entry line: a KEYS action `enter
// <text>` opens it holding `<text> ` (`Ctrl+g` / `Meta+g`: `goto `), `enter` opens it empty (`Ctrl+Return` / `Meta+Return`);
// while open it takes `BrowserText` (appended), `BackSpace`, `Return` / `KP_Enter` (closes, fires the trimmed line through
// `Face::verb`; a word the face does not take is `last_error`) and `Escape` (closes); every other key is taken and ignored.
// It shows on the message row (`message_text`) as `<text>_`, labels on only, so every labels-off golden is byte-identical.
// Owed: NLE-TC-ENTRY-FADE / -MARKER / -NUDGE, NLE-TC-ENTRY-SRCRATE, NLE-TC-ENTRY-KERNEL, NLE-TC-ENTRY-CURSOR, NLE-TC-ENTRY-APPKIT-CHORD.

/// STRIAGOTOMORE (SR9097): the tick `goto <a>` names: a signed `a` is relative to the playhead `ph`, else absolute.
fn goto_target(a: &str, ph: i64, tl: &Timeline) -> Result<i64, String> {
    if a.starts_with(['+', '-']) {
        timeline_core::edl::tc_delta(a, tl).map(|d| ph.saturating_add(d))
    } else {
        timeline_core::edl::tc_entry(a, tl)
    }
}

impl Face {
    /// `slip` / `slide <tc>` and `rtrim in|out <tc>`: the signed timecode as ticks, then the integer word through
    /// [`Face::verb`]; a refusal is `last_error`. `None` for any other word, or an amount that is an integer or no timecode.
    fn tc_amount_verb(&mut self, v: &str) -> Option<bool> {
        let w: Vec<&str> = v.split_whitespace().collect();
        let (word, x) = match w.as_slice() {
            [h @ ("slip" | "slide"), x] => (String::from(*h), *x),
            ["rtrim", e @ ("in" | "out"), x] => (format!("rtrim {e}"), *x),
            _ => return None,
        };
        if x.parse::<i64>().is_ok() || !x.contains([':', ';']) {
            return None;
        }
        match timeline_core::edl::tc_delta(x, &self.state.timeline) {
            Ok(d) => Some(self.verb(&format!("{word} {d}"))),
            Err(m) => {
                self.last_error = Some(format!("{}: {m}", w[0]));
                Some(true)
            }
        }
    }
    /// A KEYS action `enter <text>` opens the entry line holding `<text> `; `enter` opens it empty. `None` for any other action.
    fn entry_open(&mut self, act: &str) -> Option<bool> {
        let text = if act == "enter" { String::new() } else { format!("{} ", act.strip_prefix("enter ")?) };
        self.entry = Some(text);
        Some(true)
    }
    /// While the entry line is open: the typed text, BackSpace, Return (fire), Escape (close); any other key is taken and
    /// ignored (`Some(false)`). `None` with the line closed, and for every message that is not a key.
    fn entry_input(&mut self, msg: &SMessage) -> Option<bool> {
        self.entry.as_ref()?;
        match msg {
            SMessage::BrowserText(t) => {
                if let Some(e) = self.entry.as_mut() {
                    e.extend(t.chars().filter(|c| !c.is_control()));
                }
                Some(true)
            }
            SMessage::BrowserKey(k) => match k.as_str() {
                "Return" | "KP_Enter" => {
                    let owned = self.entry.take().unwrap_or_default();
                    let line = owned.trim();
                    if !line.is_empty() && !self.verb(line) {
                        self.last_error = Some(format!("{line}: not a face word"));
                    }
                    Some(true)
                }
                "BackSpace" => {
                    if let Some(e) = self.entry.as_mut() {
                        e.pop();
                    }
                    Some(true)
                }
                "Escape" => {
                    self.entry = None;
                    Some(true)
                }
                _ => Some(false),
            },
            _ => None,
        }
    }
    /// The open entry line as the message row shows it (`<text>_`); `None` while closed.
    fn entry_line(&self) -> Option<String> {
        self.entry.as_ref().map(|e| format!("{e}_"))
    }
    /// The open entry line's text; `None` while closed.
    pub fn entry(&self) -> Option<&str> {
        self.entry.as_deref()
    }
}

#[cfg(test)]
mod striagotomore_tests {
    use super::*;

    /// `goto_target`: a signed entry is relative to the playhead (clamping is the seek's), a bare one absolute.
    #[test]
    fn striagotomore_goto_target_signs() {
        let tl = Timeline::new(25);
        assert_eq!(goto_target("+00:00:01:00", 12, &tl), Ok(37));
        assert_eq!(goto_target("-5", 12, &tl), Ok(7));
        assert_eq!(goto_target("00:00:01:00", 12, &tl), Ok(25));
        assert_eq!(goto_target("-20", 12, &tl), Ok(-8), "the seek clamps, not the reader");
        assert!(goto_target("+", 12, &tl).is_err());
    }
}

// ---- STRIAEXPORTRANGEBAND (LEDGER SR9099): the running entry's name and the queue depth on the band (named tail block) ----
// The running row (`Face::message_text`) carries ` -> <name>` between the cloud's unchanged `job_line` and SR971's
// `waiting_note` while requests wait, or when the running job is a named entry (`ExportJob.named`: started from the
// queue by SR914's `start_request`, or started idle by SR9075's `export marker(s)`), so the last queued entry and a lone
// marker entry stay named; a lone plain export reads exactly as before. The name is `entry_name`, SR932's rule lifted
// once (export-list calls it). The painted 2 px export band gains one `view::TICK` notch per waiting request at its
// right end (`paint_queue_depth`); none waiting paints nothing, so every golden is byte-identical. NLE-EXPORT-QUEUE-PLAIN
// needs no code: SR881's `export_or_queue` already queues a plain export behind a running one (busy only when full).
// Face thread only, no lock. Owed: NLE-EXPORT-QUEUE-BAND-WITNESS, NLE-EXPORT-QUEUE-BAND-KERNEL.

/// STRIAEXPORTRANGEBAND (SR9099): an export entry's name: its directory's last component, else the whole path
/// (SR932's export-list rule, the one copy).
fn entry_name(dir: &std::path::Path) -> String {
    dir.file_name().map_or_else(|| dir.display().to_string(), |f| f.to_string_lossy().into_owned())
}

impl Face {
    /// Mark the running job as a named entry when `res` says it started.
    fn mark_named(&mut self, res: &Result<(), String>) {
        if let (Ok(()), Some(j)) = (res, self.export.as_mut()) {
            j.named = true;
        }
    }
    /// The running entry's name while the row names it: a named entry, or any request waiting; `None` otherwise.
    pub fn export_running_name(&self) -> Option<String> {
        self.export.as_ref().filter(|j| j.named || !self.export_queue.is_empty()).map(|j| entry_name(&j.dir))
    }
    /// The running row's name part: ` -> <name>` ([`Face::export_running_name`]), else empty.
    fn running_note(&self) -> String {
        self.export_running_name().map(|n| format!(" -> {n}")).unwrap_or_default()
    }
}

/// STRIAEXPORTRANGEBAND (SR9099): the queue depth over the export band: `n` (at most [`EXPORT_QUEUE_MAX`]) notches on the
/// band's rows (`tracks_y - EXPORT_BAND_H .. tracks_y`), notch i at x `w - 4(i + 1) .. w - 4(i + 1) + 2`, in `view::TICK`.
/// `n == 0` paints nothing.
fn paint_queue_depth(px: Vec<u8>, l: Layout, order: PixelOrder, n: usize) -> Vec<u8> {
    if n == 0 {
        return px;
    }
    let mut c = Canvas { w: l.w, h: l.h, order: vo(order), px };
    let y1 = l.tracks_y as i64;
    for i in 0..n.min(EXPORT_QUEUE_MAX) as i64 {
        let x0 = l.w as i64 - 4 * (i + 1);
        c.rect(x0, y1 - EXPORT_BAND_H as i64, x0 + 2, y1, view::TICK);
    }
    c.px
}

// ---- STRIASEQFACE (LEDGER SR9088): the `seqrate` word on the selected clip's sequence reel (named tail block) ----
// `seqrate [<N|N/D|N.F>]` sets the selected clip's image-sequence reel rate (bare: the next of `nle::SEQ_FACE_RATES`): the
// host computes the rate and the reel's length at it (`Sources::seq_rerate`, it knows the file count) and timeline_core commits
// both in ONE step (`EditorState::set_seq_rate`), so undo restores them and the face reads dirty. The render, the schedule's
// length check, the save and the message row's reel line (`Sources::summary_in`) follow the model's rate. Refusals ride
// `last_error` as `seqrate: <why>`. Not an `EditorState::verb` word: the kernel Player opens no sequence.
impl Face {
    /// STRIASEQFACE (SR9088): the `seqrate` word (taken, refused or not, as `Some(true)`); `None` for any other word.
    fn seqrate_verb(&mut self, v: &str) -> Option<bool> {
        let mut w = v.split_ascii_whitespace();
        if w.next() != Some("seqrate") {
            return None;
        }
        let (arg, extra) = (w.next(), w.next());
        if let Err(e) = self.seqrate(arg, extra.is_some()) {
            self.last_error = Some(format!("seqrate: {e}"));
        }
        Some(true)
    }
    /// STRIASEQFACE: the word's work: nothing selected `EditError::NoClip`'s text; else `<reel>: <why>` for a second
    /// argument, a reel that is no sequence, a bad rate (nle.rs `seq_rerate`) or a clip past the new length.
    fn seqrate(&mut self, arg: Option<&str>, extra: bool) -> Result<(), String> {
        let c = self.state.selected().ok_or_else(|| EditError::NoClip.to_string())?;
        let name = self.state.timeline.source(c.src).map_or_else(|| c.src.0.to_string(), |s| s.reel.clone());
        if extra {
            return Err(format!("{name}: one rate word"));
        }
        let Some((rate, dur)) = self.sources.seq_rerate(&self.state.timeline, c.src, arg).map_err(|e| format!("{name}: {e}"))? else { return Ok(()) };
        self.state.set_seq_rate(c.src, rate, dur).map_err(|e| format!("{name}: {e}"))
    }
}

// ---- STRIAKMETERRESET (LEDGER SR9101): `!` is the meters-reset word on both faces (named tail block) ----

#[cfg(test)]
mod striakmeterreset_tests {
    use super::*;

    /// M3: `!` (text) is a KEYS row for `meters-reset` (the kernel Player's EDIT_KEYS byte): it clears every latch and
    /// hold, pushes no undo step, does not dirty the document, and the help names it on the meters-reset row.
    #[test]
    fn striakmeterreset_bang_is_meters_reset() {
        assert!(KEYS.contains(&("!", "meters-reset")));
        let (mut tl, src) = synth_fixture();
        tl.tracks[1].clips.iter_mut().for_each(|c| *c = c.gained(2400));
        let mut face = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        face.verb("play");
        face.tick();
        face.tick();
        face.verb("play");
        assert_eq!(face.meter_clipped(), &[Some(1), None]);
        let (undo, dirty) = (face.state.undo_depth(), face.dirty());
        assert!(face.on(&SMessage::BrowserText("!".into())));
        assert!(face.meter_clipped().is_empty() && !face.meter_hold_pending());
        assert_eq!((face.state.undo_depth(), face.dirty()), (undo, dirty));
        assert!(face.last_error.is_none(), "{:?}", face.last_error);
        let rows = help_rows();
        let row = rows.iter().find(|r| r.1 == help_phrase("meters-reset")).expect("a meters-reset help row");
        assert!(row.0.split(" / ").any(|k| k == "!"), "{:?}", row.0);
    }
}

// ---- STRIAXFORMHOVER (LEDGER SR9107): the hovered placement handle on the preview picture (named tail block) ----
// NLE-XFORM-HANDLES-HOVER on the face: `paint_role`'s paint line hands [`Face::xform_hover`] to timeline_core's
// `view::paint_xform_hover`: on the one-Surface face, the corner a press at THE pointer (`self.pointer`, the pointer the face
// already keeps) would grab (`view::XformDrag::hover` over the painted state, layout and still); `Face::hover`'s first line
// compares [`Face::xform_hover_corner`] before and after the move, so a bare move repaints only when that corner changed.
// NLE-XFORM-HOVER-DRAG: a live corner drag's held corner is the same square (no new state). Nothing on the tracks Surface,
// under the help, or on the split preview Surface without a drag (its pointer is not the face's: NLE-XFORM-HOVER-SPLIT).

impl Face {
    /// STRIAXFORMHOVER (SR9107): the drag whose `corner` is highlighted on `role`'s paint of `st` over layout `l`: a live
    /// corner drag's held corner, else (one Surface) the handle under the pointer; `None` otherwise.
    fn xform_hover(&self, st: &EditorState, l: Layout, role: Role) -> Option<view::XformDrag> {
        if role == Role::Tracks || self.help {
            return None;
        }
        if let Some(d) = self.xform_drag {
            return d.corner.is_some().then_some(d);
        }
        if role != Role::Whole {
            return None;
        }
        let (x, y) = self.pointer?;
        let s = self.still.as_ref()?;
        view::XformDrag::hover(st, l, s.width, s.height, x, y)
    }
    /// STRIAXFORMHOVER (SR9107): the corner [`Face::xform_hover`] highlights on the pointer's Surface now.
    fn xform_hover_corner(&self) -> Option<u8> {
        let role = self.pointer_role();
        self.xform_hover(&self.state, self.layout_for(role).with_message(self.labels.is_some()), role).and_then(|d| d.corner)
    }
}

// ---- STRIAKGAINENV (LEDGER SR9111): the press takes timeline_core's ONE press order (named tail block) ----
// The begins the press tried before `begin_with` (the transition edge, the widget pick, the envelope) are timeline_core's
// `Drag::begin_grab`, the same list the kernel Player's `edit_press` calls: Shift adds to the set and skips the envelope,
// Alt / Option turns snapping off, Ctrl / Cmd is the slip modifier and skips the envelope.
impl Face {
    /// The press began a transition edge or envelope drag, or picked a transition (`Some(true)`); `None`: the press is
    /// `begin_with`'s.
    fn press_grab(&mut self, x: f64, y: f64, mods: u8) -> Option<bool> {
        let l = self.layout();
        let (shift, nosnap, slip) = (mods & bandy::MOD_SHIFT != 0, mods & bandy::MOD_ALT != 0, mods & (bandy::MOD_CONTROL | bandy::MOD_SUPER) != 0);
        match Drag::begin_grab(&mut self.state, l, x, y, shift, nosnap, slip)? {
            Grab::Drag(d) => self.drag = Some(d),
            Grab::Pick(r) => {
                self.drag = None;
                if let Err(e) = r {
                    self.last_error = Some(format!("transition: {e}"));
                }
            }
        }
        Some(true)
    }
}

// ---- STRIASOURCEGHOST (LEDGER SR9115): the source frame following a source drag (named tail block) ----
// `paint_role`'s Tracks / Whole chain folds timeline_core view's `paint_source_ghost` over `paint_drag`: while a source
// drag's pointer is over the tracks, a framed thumb of `self.still` (during a source drag, the loaded source's frame the
// preview band rendered: `Drag::shown` carries the source) sits above the pointer. A tracks-only paint outside
// `frames` shows the last preview paint's still. Nothing else paints differently.

#[cfg(test)]
mod striasourceghost_tests {
    use super::*;
    use timeline_core::SourceId;

    fn loaded() -> Face {
        let (tl, src) = synth_fixture();
        let mut f = Face::new(EditorState::new(tl), src, (320, 160), PixelOrder::Rgba, false);
        assert_eq!(f.state.load_source(SourceId(1), &[Kind::Video, Kind::Audio]), Ok(()));
        f.state.source.as_mut().unwrap().marks = (Some(1), Some(3));
        f
    }

    fn at(px: &[u8], w: u32, x: i64, y: i64) -> [u8; 4] {
        let i = (y as usize * w as usize + x as usize) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    /// One Surface: a source drag over V1 paints the GUIDE-outlined ghost above the pointer, the still being the loaded
    /// source's frame; after the release the ghost is gone.
    #[test]
    fn striasourceghost_one_surface_ghost_follows_the_drag() {
        let mut f = loaded();
        let l = f.layout();
        assert!(f.on(&SMessage::BrowserButton { down: true, x: 20.0, y: 10.0, mods: bandy::MOD_ALT, surface: 0 }));
        let (x, y) = l.point(0, 15);
        assert!(f.on(&SMessage::BrowserMouseMove(x, y, 0)));
        let px = f.paint();
        let want = preview(&f.state.source_view().unwrap(), &f.sources, 0).unwrap();
        assert!(want.is_some(), "B's frame");
        assert_eq!(f.still().cloned(), want, "the ghost's still is the source's frame");
        let (x0, y0) = view::ghost_box(&l, (x, y));
        let foot = y0 + THUMB_H - 1;
        assert_eq!(at(&px, l.w, x0, foot), GUIDE);
        assert_eq!(at(&px, l.w, x0 + THUMB_W - 1, foot), GUIDE);
        assert!(f.on(&SMessage::BrowserButton { down: false, x, y, mods: 0, surface: 0 }));
        assert!(f.dragging().is_none());
        let after = f.paint();
        assert_ne!(at(&after, l.w, x0, foot), GUIDE, "no ghost without a live source drag");
    }

    /// Split: the tracks Surface's blit carries the ghost at the translated pointer; the preview's carries none.
    #[test]
    fn striasourceghost_split_tracks_surface_carries_the_ghost() {
        let mut f = loaded().on_surfaces(0, 1);
        let ph = f.split().unwrap().preview_canvas.1 as f64;
        let l = f.layout();
        let (x, yt) = l.point(0, 15);
        assert!(f.on(&SMessage::BrowserButton { down: true, x: 20.0, y: 10.0, mods: bandy::MOD_ALT, surface: 0 }));
        let before = f.paint_role(Role::Preview);
        assert!(f.on(&SMessage::BrowserMouseMove(x, yt + ph, 0)));
        let frames = f.frames();
        let SMessage::SurfaceBlit { pixels: preview_px, .. } = &frames[0] else { panic!("not a blit") };
        let SMessage::SurfaceBlit { pixels: tracks_px, width, .. } = &frames[1] else { panic!("not a blit") };
        assert_eq!(preview_px, &before, "the preview Surface paints no ghost");
        let (x0, y0) = view::ghost_box(&l, (x, yt));
        assert_eq!(at(tracks_px, *width, x0, y0 + THUMB_H - 1), GUIDE);
        assert_eq!(at(tracks_px, *width, x0, y0), GUIDE);
    }
}

// ---- STRIAPROXYSCAN (LEDGER SR9116): the caches and the export judged by the one sources generation (named tail block) ----
// SR9079's `stale_in` dropped a head or a peak table whose job was IN FLIGHT across a reload, but what the face had CACHED
// before the reload stayed: `wave_fill`, `Thumbs::fill` and `fill_strip` ask only for absent keys, so the band painted the old
// media's heads, filmstrip tiles and table for the face's life. And an export whose thread had ended (its clone dropped) but
// was not yet joined let a reload through, then reported a file of the old media with no word of it. RULES:
// * CACHE: `WavePeaks.swept` is the generation of the last sweep. `watch_sources` calls `cache_sweep` once after its loop
//   (beside `trans_still = None`): every `WavePeaks.map` / `Thumbs.map` entry whose source reloaded after `swept`
//   (`stale_in`, the one rule) goes, then `swept` takes `sources_gen`. No job can have inserted at the new generation yet
//   (jobs are sent only from a paint), so the sweep drops exactly the old media's entries; the next paint asks again (on the
//   face's thread, or SR997's worker at the new lease); an old job's queued result is still dropped by `drain_bg`. Another
//   source's entries stay. SR9071 / SR9098's peak FILE cache keys a table by the media's size and mtime, so a reloaded reel
//   misses it by its stamp: the sweep and the file cache agree.
// * EXPORT: `ExportJob.sgen` is the generation at its clone; `finish_export`, after its line is built, calls `export_stale`:
//   any source reloaded later (`sources_stale(None, ..)`) appends `; <reels> reloaded after the export started (it holds
//   the old media)` to every outcome. `job_line` / `export_eta` / `export_json` are untouched.
// No lock: the generation is the face thread's own field. Owed: NLE-PROXY-WATCH-EXPORT-WITNESS (stria-view `export_json`
// carries no stale flag), NLE-PROXY-WATCH-PREVIEW (the clause names every reloaded reel, not only those in the range).

impl Face {
    /// STRIAPROXYSCAN (LEDGER SR9116): drop every cached head, tile and peak table of a source reloaded since the last sweep.
    fn cache_sweep(&mut self) {
        let swept = self.peaks.swept;
        if self.sources_gen == swept {
            return;
        }
        let reloaded = &self.reloaded;
        self.peaks.map.retain(|id, _| !stale_in(reloaded, Some(*id), swept));
        self.peaks.thumbs.map.retain(|k, _| !stale_in(reloaded, Some(k.0), swept));
        self.peaks.swept = self.sources_gen;
    }
    /// STRIAPROXYSCAN (LEDGER SR9116): an export cloned at `sgen` before a reload: its result line names the reloaded reels.
    fn export_stale(&mut self, sgen: u64) {
        if !self.sources_stale(None, sgen) {
            return;
        }
        let reels: Vec<String> = self
            .reloaded
            .iter()
            .filter(|(_, g)| *g > sgen)
            .map(|(s, _)| self.state.timeline.source(*s).map(|x| x.reel.clone()).unwrap_or_else(|| format!("{s:?}")))
            .collect();
        let clause = format!("{} reloaded after the export started (it holds the old media)", reels.join(", "));
        self.last_error = Some(match self.last_error.take() {
            Some(l) => format!("{l}; {clause}"),
            None => clause,
        });
    }
}
