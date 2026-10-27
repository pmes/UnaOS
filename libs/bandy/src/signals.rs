// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::ontology::{Origin, WeightedSkeleton};
use crate::state::{BrowseListing, DispatchRecord, FsOutcome, FsVerb, LogLine, LogSource};

/// SMessage (The Shard Message).
/// The atomic unit of truth in UnaOS.
/// This Enum defines the limits of what can be said between processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SMessage {
    StateInvalidated,
    // --- SYSTEM HEARTBEAT ---
    Ping,
    Kill(String),
    Log {
        level: String,
        source: String,
        content: String,
    },
    /// One beat of the system monitor: per-core CPU load fractions
    /// (`0.0..=1.0`, one entry per core). Fired by the `pulse` vessel's
    /// sampler seam (`PulseSource`); a future UnaOS-kernel telemetry feed
    /// replaces the *source*, not this message.
    CorePulse {
        loads: Vec<f32>,
    },

    // --- EUCLASE (The Visual Cortex) ---
    EuclaseResize(u32, u32),
    VugPulse,

    // --- RESONANCE (The Voice) ---
    AudioChunk {
        source_id: String,
        samples: Vec<f32>,
        sample_rate: u32,
    },
    Spectrum {
        magnitude: Vec<f32>,
    },

    // --- VEIN / LUMEN (The Mind) ---
    UserPrompt(String),
    AiToken(String),
    AnalyzeContext {
        id: String,
        content: String,
    },
    NetworkLog(String),
    NetworkState(String),
    // Vaire / Git Integration
    GetDiff {
        commit_a: String,
        commit_b: String,
    },
    DiffPayload {
        diff: String,
    },
    // Context Telemetry (Lumen HUD)
    /// NON-WIRE EXCEPTION: `WeightedSkeleton.content` is `#[serde(skip)]` by
    /// design (in-process `Arc<String>` only — see
    /// [`crate::ontology::WeightedSkeleton`]). This variant serializes, but
    /// LOSSILY: the skeleton content is dropped on serialize and comes back
    /// `Default`-empty on deserialize, so it never crosses a process boundary.
    /// Inter-process telemetry is deferred to `unafs` shared memory. Frozen by
    /// the `context_telemetry_*` proofs in `tests/smessage_kats.rs`.
    ContextTelemetry {
        skeletons: Vec<WeightedSkeleton>,
    },

    // --- UNAFS / MATRIX (The Memory) ---
    FileEvent {
        path: String,
        event: String,
    },

    // --- AMBER BYTES (The Storage Rune) ---
    StorageQuery {
        receipt_id: u64,
        embedding: Vec<f32>,
        /// EMBED (B317): the `<provider>/<model>` that made `embedding`; the vault compares it only
        /// with vectors tagged the same. Empty (an older sender) compares every vector.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        embed_model: String,
    },
    StorageQueryResult {
        receipt_id: u64,
        memories: Vec<String>,
        directives: Vec<String>,
        engrams: Vec<String>,
        chrono: Vec<String>,
    },
    StorageSave {
        receipt_id: u64,
        sender: String,
        content: String,
        timestamp: String,
        embedding: Vec<f32>,
        memory_type: String,
        /// EMBED (B317): stored as `una:embed-model` beside the vector (empty = no vector written).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        embed_model: String,
    },
    StorageSaveResult {
        receipt_id: u64,
        success: bool,
        error: Option<String>,
    },
    StorageLoadPaged {
        receipt_id: u64,
        offset: usize,
        limit: usize,
    },
    StorageLoadPagedResult {
        receipt_id: u64,
        records: Vec<DispatchRecord>,
    },
    /// EMBED (B317): ask the vault for up to `limit` memories whose vector was not made by
    /// `embed_model` (`limit == 0` only counts them).
    ReEmbed {
        receipt_id: u64,
        embed_model: String,
        limit: usize,
    },
    /// The vault's answer: `(inode id, content)` to re-embed, and how many are stale in all.
    ReEmbedBatch {
        receipt_id: u64,
        items: Vec<(u64, String)>,
        stale_total: usize,
    },
    /// New vectors for the batch, made by `embed_model`.
    ReEmbedWrite {
        receipt_id: u64,
        embed_model: String,
        vectors: Vec<(u64, Vec<f32>)>,
    },
    /// The vault wrote `written` vectors; `remaining` are still stale.
    ReEmbedDone {
        receipt_id: u64,
        written: usize,
        remaining: usize,
        error: Option<String>,
    },

    // --- AETHER (The Browser) ---
    OpenDocument { url: String },
    /// A surface's pixels (RGBA8, straight alpha; the contract on quartzite's `TetraNode::Surface`). AETHERMACGPU-DAMAGE
    /// (LEDGER SR320): `damage = Some((x, y, w, h))` says `pixels` carries ONLY that rect's rows (`h * w * 4` bytes, stride
    /// `w * 4`) of a `width x height` frame the receiver already holds; `None` (omitted from the wire, so a whole frame keeps
    /// its frozen shape, smessage_kats) is the whole frame. A sender sends a whole frame at least every
    /// [`SURFACE_FULL_EVERY`] blits so a receiver that lagged on the bus heals.
    SurfaceBlit { url: String, width: u32, height: u32, pixels: Vec<u8>, #[serde(default, skip_serializing_if = "Option::is_none")] damage: Option<(u32, u32, u32, u32)> },
    PlayMedia { url: String, title: String, mime: String },
    BrowserNavBack,
    BrowserNavForward,
    BrowserNavReload,
    /// SURFACEID (LEDGER SR332): the third field on `BrowserScroll` / `BrowserClick` / `BrowserMouseMove` (and
    /// `BrowserButton`'s `surface`) is the Surface's number in its tetra tree (quartzite `tetra::SurfaceMap`: depth-first,
    /// the first or only Surface is 0), omitted from the wire when 0 so a one-surface tree keeps its frozen shape.
    BrowserScroll(f64, f64, #[serde(default, skip_serializing_if = "no_surface")] u32),
    BrowserClick(f64, f64, #[serde(default, skip_serializing_if = "no_surface")] u32),
    BrowserResize(u32, u32),
    BrowserKey(String),
    BrowserText(String),
    /// Chrome → engine: the platform clipboard's text, pasted at the focused field's caret (the shell
    /// reads the clipboard on Ctrl/Cmd+V; AETHERINPUT, SR68).
    BrowserPaste(String),
    /// Engine → chrome: the current document url changed (link click,
    /// back/forward, redirect); the address bar mirrors it.
    BrowserUrlChanged(String),
    /// Engine → chrome: the current document's `<title>` changed. Fired from
    /// the same post-navigation choke point as [`SMessage::BrowserUrlChanged`],
    /// deduped against the last one sent; the window title bar mirrors it.
    BrowserTitleChanged(String),
    /// Engine → chrome: the current document's favicon, decoded to tightly
    /// packed 8-bit RGBA (`width * height * 4`, row-major, top row first).
    /// Fetched *after* the page is delivered, so it never delays a load; an
    /// absent or undecodable icon simply fires nothing.
    BrowserFaviconChanged { width: u32, height: u32, rgba: Vec<u8> },
    /// Chrome → engine: the pointer moved over the page surface, in surface
    /// pixels (top-left origin) — hover, `:hover` style, the cursor shape and
    /// the page's mouseover/mouseout/mouseenter/mouseleave/mousemove (SR67).
    BrowserMouseMove(f64, f64, #[serde(default, skip_serializing_if = "no_surface")] u32),
    /// Engine → chrome: the cursor the hovered element asks for changed. A
    /// css-ui-4 `cursor` keyword (`default`, `pointer`, `text`, `wait`, …),
    /// which is also the freedesktop/GTK cursor name; deduped by the engine.
    BrowserCursorChanged(String),
    /// Chrome → engine: copy the page's selection (Ctrl/Cmd+C on the viewport). The engine answers
    /// with [`SMessage::BrowserClipboardSet`] when something is selected (AETHERSELECT, LEDGER SR69).
    BrowserCopy,
    /// Chrome → engine: select the document's contents (Ctrl/Cmd+A on the viewport).
    BrowserSelectAll,
    /// Engine → chrome: put `text` (the selection serialized per the HTML `innerText` algorithm) on
    /// the platform clipboard.
    BrowserClipboardSet(String),
    // AETHERCHROME (SR84): the browser chrome — tabs, the address bar's go/stop, zoom, find in page,
    // bookmarks, page info, downloads and the hovered-link status line. The MODEL of each lives in
    // `aether::chrome` (std-free); these verbs are the seam between it and whichever face paints it.
    /// Chrome → shell: the address bar's text and the user's search template (Principia
    /// `aether.search`, `%s` = the query). The shell decides URL vs search (`aether::chrome::omnibox`).
    BrowserGo { input: String, search: String },
    /// Chrome → engine: stop the load in flight (Esc, the stop button).
    BrowserNavStop,
    /// Chrome → shell: open a new tab (Ctrl+T) and switch to it.
    BrowserTabNew,
    /// Chrome → shell: close tab `id`, or the active tab when `None` (Ctrl+W).
    BrowserTabClose(Option<u64>),
    /// Chrome → shell: switch to tab `id`.
    BrowserTabSelect(u64),
    /// Chrome → shell: switch `n` tabs right (negative: left), wrapping (Ctrl+Tab / Ctrl+Shift+Tab).
    BrowserTabStep(i32),
    // AETHERSIDETABS2 (SR143): the many-tabs verbs — the drag and the tab groups. The model is
    // `aether::chrome::tabs::TabStrip` (move_to, group_*); `aether::chrome::apply_tab_verb` applies these.
    /// Chrome → shell: the drag — move tab `id` to strip index `to` (counted after it is taken out; clamped).
    /// Dropped between two tabs of a group it joins that group; dragged clear of its own it leaves it.
    BrowserTabMove { id: u64, to: u32 },
    /// Chrome → shell: a new group named `name` holding `tabs`, in `color` (a `GroupColor` name such as
    /// `blue`; `None`: the first colour no group uses).
    BrowserTabGroupNew { tabs: Vec<u64>, name: String, color: Option<String> },
    /// Chrome → shell: put tab `id` at the end of group `group` (a drop on the group's header).
    BrowserTabGroupAdd { id: u64, group: u64 },
    /// Chrome → shell: take tab `id` out of its group (it lands just past the group's run).
    BrowserTabUngroup(u64),
    /// Chrome → shell: collapse or expand group `id` (a click on its header or its chip).
    BrowserTabGroupToggle(u64),
    /// Chrome → shell: rename group `group`.
    BrowserTabGroupRename { group: u64, name: String },
    /// Chrome → shell: recolour group `group` (`None`: the next colour after its current one).
    BrowserTabGroupRecolor { group: u64, color: Option<String> },
    /// Chrome → shell: dissolve group `id`; its tabs stay where they are, ungrouped.
    BrowserTabGroupDissolve(u64),
    /// Engine → chrome: tab `id`'s favicon as a `size` x `size` tightly packed 8-bit RGBA tile (top row first),
    /// for the strip (every tab's, active or not); an empty `rgba` clears it (the letter tile shows again). At
    /// most one fetch per page load; the full-size active icon stays [`SMessage::BrowserFaviconChanged`].
    BrowserTabFavicon { id: u64, size: u32, rgba: Vec<u8> },
    /// Chrome → engine: zoom one step in (`1`), out (`-1`), or back to 100 % (`0`).
    BrowserZoom(i32),
    /// Chrome → engine: find `query` in the page; a repeat of the same query steps to the next
    /// (`forward`) or previous match.
    BrowserFind { query: String, forward: bool },
    /// Chrome → engine: the find bar closed; the active match's highlight goes.
    BrowserFindClose,
    /// Chrome → shell: bookmark the active tab's page, or remove its bookmark (Ctrl+D).
    BrowserBookmarkToggle,
    /// Chrome → engine: describe the active page (the page-info popover opened).
    BrowserPageInfoAsk,
    /// Shell → chrome: the tab strip — its rows in strip order, its groups, and the active id
    /// (AETHERSIDETABS2, SR143: the row carries the URL, the group and the letter tile's seed).
    BrowserTabs { active: u64, tabs: Vec<BrowserTabRow>, groups: Vec<BrowserTabGroupRow> },
    /// Engine → chrome: what the toolbar shows for the active tab.
    BrowserNavState { can_back: bool, can_forward: bool, loading: bool, zoom_pct: u32 },
    /// Engine → chrome: `matches` hits for `query`; `current` is the 1-based active one (0 = none).
    BrowserFindResult { query: String, matches: u32, current: u32 },
    /// Engine → chrome: the status line (the hovered link's URL; empty clears it).
    BrowserStatus(String),
    /// Shell → chrome: the bookmarks `(title, url)` in order, and whether the active page is one.
    BrowserBookmarks { items: Vec<(String, String)>, current: bool },
    /// Engine → chrome: the page-info popover's answer.
    BrowserPageInfo { url: String, secure: bool, summary: String },
    /// Shell → chrome: one download's progress row. `total` is `None` when the length is unknown;
    /// `done` with `error: None` is a saved file.
    BrowserDownload { id: u64, name: String, received: u64, total: Option<u64>, done: bool, error: Option<String> },
    // AETHERDEVTOOLS2 (SR112): the developer-tools pane on the glass. The model is `aether::chrome::DevtoolsChrome`
    // (open, dock side, share; `split` decides the page and pane rects) owned by the shell; the pane's content
    // is `aether::devtools::DevTools` beside each tab's engine. These verbs are the seam to the face.
    /// Chrome → shell: a key the face's capture-phase hook took before the page saw it (F12, Ctrl+Shift+I);
    /// `key` is the key's name (`F12`, `I`). The shell's `DevtoolsChrome::on_key` decides.
    BrowserDevtoolsKey { key: String, ctrl: bool, shift: bool, alt: bool },
    /// Chrome → shell: dock the pane under the page (`bottom`) or beside it (Principia
    /// `aether.devtools.dock`, read by the face at start and written by its dock button).
    BrowserDevtoolsDock { bottom: bool },
    /// Chrome → shell: the splitter moved; the pane's share of the dock axis in percent (clamped by
    /// `DevtoolsChrome::set_share`).
    BrowserDevtoolsShare(u32),
    /// Chrome → engine: show the panel labelled so (`Elements`, `Console`, `Network`, `Storage`,
    /// `Performance`).
    BrowserDevtoolsPanel(String),
    /// Chrome → engine: one Console line, evaluated in the page's realm.
    BrowserDevtoolsEval(String),
    /// AETHERDEVTOOLS3 (SR128): chrome → engine: a Console frame line was clicked — show `url` at `line`
    /// in the Sources panel.
    BrowserDevtoolsGoto { url: String, line: u32 },
    /// Shell → chrome: `DevtoolsChrome::split` of the content area — the page rect and the pane rect
    /// `(x, y, w, h)` (`pane` None: closed, the page has the whole area); `bottom` is the dock side.
    BrowserDevtoolsLayout { bottom: bool, page: (u32, u32, u32, u32), pane: Option<(u32, u32, u32, u32)> },
    /// Engine → chrome: the pane's text, one line per row (`DevTools::render`), sent when it changes.
    BrowserDevtoolsLines(Vec<String>),
    // A11YFACE (SR124): the accessibility verbs and Principia's accessibility rows on the glass. The engine side is
    // `aether::dom::a11y` (the tree, `nav`'s reading verbs, `prefs`); these verbs are the seam to the face.
    /// Chrome → engine: one accessibility verb ([`A11yVerb::parse`] reads the typed `a11y …` spelling).
    BrowserA11y(A11yVerb),
    /// Engine → chrome: a verb's answer — the tree's lines for `a11y tree`, else the line of the node the
    /// reading position moved to (empty when there was nothing to move to).
    BrowserA11yLines(Vec<String>),
    /// Chrome → shell: Principia's `system.accessibility.*` rows as the face read them, at start and on every
    /// change (`contrast` in its stored spelling: `no-preference` / `more` / `less`).
    BrowserA11yPrefs { reduce_motion: bool, contrast: String, forced_colors: bool },

    // --- STRIA (A/V playback; PLAYBACK, LEDGER SR26) ---
    // A session is keyed by the media `url` the request named (the same string `PlayMedia`
    // carried), so a requester correlates replies without a handle round trip. Requests:
    // `PlayMedia` (open + play), `MediaPoster` (open, present the first frame, stay paused),
    // `MediaPause`, `MediaResume`, `MediaSeek`, `MediaStop`. Stria answers with `MediaOpened`
    // once, `MediaFrame` per presented frame, then `MediaEnded` — or `MediaError`.
    /// Open `url` and present its first frame only (a `<video>` poster), paused.
    MediaPoster { url: String },
    MediaPause { url: String },
    MediaResume { url: String },
    /// Seek to `position_ns` (lands on the last keyframe at or before it, then decodes forward).
    MediaSeek { url: String, position_ns: u64 },
    MediaStop { url: String },
    /// AUDIOTRACK (SR45): silence (`muted`) or restore the session's audio. The clock keeps
    /// running on the device — a muted `<video>` stays in sync — only the samples are zeroed.
    MediaMute { url: String, muted: bool },
    /// AETHERMEDIA (SR107): the session's output gain, 0.0..=1.0 (HTMLMediaElement.volume); linear,
    /// applied to the samples after decode. Mute is separate (`MediaMute`) and wins.
    MediaVolume { url: String, volume: f32 },
    /// AETHERMEDIA (SR107): the session's playback rate (HTMLMediaElement.playbackRate). 0 holds the
    /// clock (the session stays playing); other values scale the clock and the frame schedule.
    MediaRate { url: String, rate: f64 },
    /// Stria → requester: the stream is open. `video` / `audio` name the codecs ("" when the
    /// track is absent); `real_video` is false when the frames are the labelled test-pattern
    /// stand-in (no decoder for the codec yet); `audio_clock` is true when audio is playing and
    /// is the master clock.
    MediaOpened {
        url: String,
        duration_ns: u64,
        width: u32,
        height: u32,
        video: String,
        audio: String,
        real_video: bool,
        audio_clock: bool,
    },
    /// Stria → requester: a frame went on glass at `pts_ns`; tightly packed 8-bit RGBA.
    /// AUDIOTRACK (SR45): `levels` carries the audio meters that came due — peak |sample| of
    /// the left and right channel per 50 ms window, the first window starting at `pts_ns` for
    /// an audio-only session (whose frames are 0×0 with no pixels), or ending at the frame for
    /// a video. Omitted from the wire when empty, so a video frame without audio is unchanged.
    MediaFrame {
        url: String,
        pts_ns: i64,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        levels: Vec<[f32; 2]>,
    },
    /// Stria → requester: playback reached the end (or was stopped).
    MediaEnded { url: String, presented: u64, dropped: u64 },
    MediaError { url: String, error: String },

    // --- SONDE (Probes; REALSENSE, LEDGER SR92, R112) ---
    // Sonde owns the probes — depth cameras, cameras, IMUs. A probe is keyed by its `id` (e.g.
    // `replay:d435i`, `uvc:8086:0b3a`), a stream by its name (`depth`, `ir`, `color`, `accel`, `gyro`).
    // The frame formats and the depth model are `probe_core`'s (unaos/libs/media/probe_core), the one
    // no_std core both rings link. Requests: `ProbeList` (empty), `ProbeOpen`, `ProbeClose`. Sonde
    // answers `ProbeList` with the probes, an open stream with `ProbeFrame`s, and a stream that stops
    // (closed, or could not open) with `ProbeClose`.
    /// Requester → Sonde with `probes` EMPTY: name the probes. Sonde → requester: the probes present,
    /// `(id, name, streams)` with `streams` the stream names the probe can open.
    ProbeList { probes: Vec<(String, String, Vec<String>)> },
    /// Requester → Sonde: start `stream` of probe `id` at `width x height` and `fps` (each 0 = the
    /// probe's default mode for that stream).
    ProbeOpen { id: String, stream: String, width: u32, height: u32, fps: u32 },
    /// Sonde → requester: one frame of an open stream, raw. `format` is a `probe_core::PixelFormat` name
    /// (`Z16`, `Y8`, `Y16`, `YUY2`); `data` holds `stride * (height - 1) + width * bytes_per_pixel`
    /// bytes or more, little-endian samples. `depth_scale` is metres per Z16 unit (0 for a non-depth
    /// stream); `intrinsics` is the stream's pinhole `[fx, fy, ppx, ppy]` in pixels when known.
    ProbeFrame {
        id: String,
        stream: String,
        format: String,
        seq: u64,
        timestamp_us: u64,
        width: u32,
        height: u32,
        stride: u32,
        depth_scale: f32,
        intrinsics: Option<[f32; 4]>,
        data: Vec<u8>,
    },
    /// Requester → Sonde: stop `stream` of `id` (every open stream of `id` when `stream` is empty).
    /// Sonde → requester: that stream stopped (closed, or could not be opened).
    ProbeClose { id: String, stream: String },

    // --- EDITOR (The Code Pane) ---
    /// Load a document into the active editor pane. Fired when a file is
    /// selected for editing; the macOS `MacOSSpline` router pushes `content`
    /// into the editor `NSTextView` via `setString`. `path`/`language` let the
    /// view label + (later) syntax-highlight the buffer.
    EditorLoad {
        path: Option<String>,
        content: String,
        language: String,
    },
    /// View → brain: the editor buffer changed (fired by the macOS
    /// `EditorDelegate`'s `textDidChange`). Carries the full current buffer so
    /// the brain can hold the live document without a separate read-back.
    EditorEdited {
        content: String,
    },
    /// View → brain: the user asked to save the active editor buffer (Cmd+S /
    /// menu Save). The brain owns the actual write (path + persistence); this is
    /// just the request signal.
    EditorSaveRequest,

    // --- CONSOLE (The Bottom Pane) ---
    /// Brain → view: append one line to the read-only console output pane.
    ConsoleAppend(String),
    /// View → brain: the user submitted a line in the console input field
    /// (Enter). The brain routes/executes it (e.g. into `midden`).
    ConsoleInput(String),

    // --- MIDDEN (The Terminal) ---
    NoOp,
    TerminalOutput(String),
    TerminalError(String),
    FileSystemEvent(String),
    TriggerUpload(PathBuf),

    // --- PRINCIPIA (The Basal Ganglia) ---
    Principia(PrincipiaCommand),

    // --- MATRIX (The Spatial Cortex) ---
    Matrix(MatrixEvent),

    // --- CONSOLE (the system log viewer — macOS Console.app equivalent) ---
    /// The Console log-viewer channel, owned by the `comscan` handler. Distinct
    /// from the [`SMessage::Log`] PRODUCER message: `Log { .. }` is a component
    /// emitting one line INTO the system; `Logs(..)` is the viewer's command +
    /// render channel layered ON TOP of that feed. Mirrors the
    /// [`SMessage::Matrix`] / [`SMessage::Principia`] sub-enum shape so a new
    /// app surface adds one outer variant, not a spray of siblings.
    Logs(LogEvent),

    // --- FACET (The Canvas — the Images handler, ledger SR29) ---
    /// The Images handler's channel: open/inspect/render/edit/export a picture by handle, and the
    /// "open image" association every other surface (Matrix's Finder, Quarry, Aether) delegates
    /// through. Mirrors the [`SMessage::Principia`] / [`SMessage::Matrix`] sub-enum shape.
    Facet(FacetCommand),

    // --- UI EVENTS (Migrated from gneiss_pal::types::Event) ---
    Input {
        target: String,
        text: String,
    },
    TemplateAction(usize),
    NavSelect(usize),
    DockAction(usize),
    UploadRequest,
    FileSelected(PathBuf),
    ToggleSidebar,
    LoadHistory { offset: usize },
    UpdateMatrixSelection(Vec<String>),
    MatrixFileClick(PathBuf),
    AuleIgnite,
    Timer,
    CreateNode {
        model: String,
        history: bool,
        temperature: f64,
        system_prompt: String,
    },
    NodeAction {
        action: String,
        active: bool,
    },
    ComplexInput {
        target: String,
        subject: String,
        body: String,
        point_break: bool,
        action: String,
    },
    ShardSelect(String),
    DispatchPayload(String),
    ToggleMatrixNode(String),
    UiReady,

    // --- TABULA (The Text handler, ledger SR74 TABULABUS) ---
    /// The Text handler's channel: open/goto/insert/save/close a document by path, and the answers
    /// Tabula gives. APPENDED after `UiReady` so no earlier wire tag moves. Mirrors the
    /// [`SMessage::Facet`] sub-enum shape: one outer variant, the verbs inside it.
    Tabula(TabulaCommand),
    /// Chrome -> owner (NLE2B, LEDGER SR208): the primary button went down (`down: true`) or up on a surface, in
    /// surface pixels (top-left origin). Both ends of a press, so a drag has a start and an end on every face (GTK's
    /// click gesture sends no `BrowserClick` once the pointer passes its drag threshold). `BrowserClick` on release
    /// and `BrowserMouseMove` are still sent as before. APPENDED so no earlier wire tag moves. NLE2-MODS (LEDGER SR306):
    /// `mods` is the modifier state at the press / release ([`MOD_SHIFT`] | [`MOD_CONTROL`] | [`MOD_ALT`] | [`MOD_SUPER`]),
    /// omitted from the wire when 0 so a press with no modifier keeps its frozen shape (smessage_kats).
    /// SURFACEID (LEDGER SR332): `surface` is the Surface's number (see `BrowserScroll`), omitted when 0.
    BrowserButton { down: bool, x: f64, y: f64, #[serde(default, skip_serializing_if = "no_mods")] mods: u8, #[serde(default, skip_serializing_if = "no_surface")] surface: u32 },
}

/// A typed preference value — the whole value domain of the Principia
/// preference store. Deliberately small: these four types are exactly what a
/// TOML scalar can carry losslessly, so a value survives the round trip
/// store → file → store → bus without widening or coercion. Externally tagged
/// on the wire (the enum default) so `Float(1.0)` cannot come back as `Int(1)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PrefValue {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl PrefValue {
    /// The value's type name (`"string" | "int" | "float" | "bool"`) — for
    /// logs, diagnostics and a future schema surface.
    pub fn type_name(&self) -> &'static str {
        match self {
            PrefValue::Str(_) => "string",
            PrefValue::Int(_) => "int",
            PrefValue::Float(_) => "float",
            PrefValue::Bool(_) => "bool",
        }
    }
}

impl From<String> for PrefValue {
    fn from(v: String) -> Self {
        PrefValue::Str(v)
    }
}
impl From<&str> for PrefValue {
    fn from(v: &str) -> Self {
        PrefValue::Str(v.to_string())
    }
}
impl From<i64> for PrefValue {
    fn from(v: i64) -> Self {
        PrefValue::Int(v)
    }
}
impl From<f64> for PrefValue {
    fn from(v: f64) -> Self {
        PrefValue::Float(v)
    }
}
impl From<bool> for PrefValue {
    fn from(v: bool) -> Self {
        PrefValue::Bool(v)
    }
}

/// A11YFACE (SR124): an accessibility verb ([`SMessage::BrowserA11y`]); [`A11yVerb::TABLE`] is the `a11y` rows of
/// the verb table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum A11yVerb {
    /// `a11y tree`: the accessibility tree's lines.
    Tree,
    /// `a11y skip`: skip to content (the main landmark, else the page's skip link, else the first h1).
    Skip,
    /// `a11y next heading [N]` / `a11y previous heading [N]` (N: only headings of level N).
    NextHeading { backward: bool, level: Option<u32> },
    /// `a11y next landmark` / `a11y previous landmark`.
    NextLandmark { backward: bool },
}

impl A11yVerb {
    /// The verb table: each spelling and its verb.
    pub const TABLE: [(&'static str, A11yVerb); 6] = [
        ("a11y tree", A11yVerb::Tree),
        ("a11y skip", A11yVerb::Skip),
        ("a11y next heading", A11yVerb::NextHeading { backward: false, level: None }),
        ("a11y previous heading", A11yVerb::NextHeading { backward: true, level: None }),
        ("a11y next landmark", A11yVerb::NextLandmark { backward: false }),
        ("a11y previous landmark", A11yVerb::NextLandmark { backward: true }),
    ];

    /// Reads a typed verb: a [`A11yVerb::TABLE`] spelling (whitespace and ASCII case folded); a heading verb may
    /// end in a level 1–6 (`a11y next heading 2`).
    pub fn parse(s: &str) -> Option<A11yVerb> {
        let words: Vec<String> = s.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
        let (body, level) = match words.last().and_then(|w| w.parse::<u32>().ok()) {
            Some(l) if (1..=6).contains(&l) => (&words[..words.len() - 1], Some(l)),
            Some(_) => return None,
            None => (&words[..], None),
        };
        let joined = body.join(" ");
        let (_, verb) = Self::TABLE.iter().find(|(spelling, _)| *spelling == joined)?;
        match (*verb, level) {
            (A11yVerb::NextHeading { backward, .. }, level) => Some(A11yVerb::NextHeading { backward, level }),
            (v, None) => Some(v),
            (_, Some(_)) => None,
        }
    }

    /// The verb's spelling (the inverse of [`A11yVerb::parse`]).
    pub fn spelling(&self) -> String {
        match *self {
            A11yVerb::NextHeading { backward, level: Some(l) } => {
                format!("a11y {} heading {l}", if backward { "previous" } else { "next" })
            }
            v => Self::TABLE.iter().find(|(_, t)| *t == v).map(|(s, _)| s.to_string()).unwrap_or_default(),
        }
    }
}

/// serde helper: omit a `false` flag from the wire.
fn is_false(b: &bool) -> bool {
    !*b
}

/// serde helper: omit an empty modifier set from the wire (`BrowserButton`'s `mods`).
fn no_mods(m: &u8) -> bool {
    *m == 0
}

/// serde helper: omit the default Surface (number 0) from the wire (SURFACEID, LEDGER SR332).
fn no_surface(n: &u32) -> bool {
    *n == 0
}

/// `BrowserButton`'s `mods` bits (NLE2-MODS, LEDGER SR306), named once: Shift.
pub const MOD_SHIFT: u8 = 1 << 0;
/// Control.
pub const MOD_CONTROL: u8 = 1 << 1;
/// Alt / Option.
pub const MOD_ALT: u8 = 1 << 2;
/// Super / Command (GTK's Meta or Super).
pub const MOD_SUPER: u8 = 1 << 3;

/// AETHERMACGPU-DAMAGE (LEDGER SR320): a sender of damage-only `SurfaceBlit`s sends a whole frame (`damage: None`) at
/// least once every this many blits, so a receiver that missed one on the lossy bus is repaired within that many.
pub const SURFACE_FULL_EVERY: u32 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrincipiaCommand {
    SetSystemRoot(PathBuf),
    SystemRootChanged(PathBuf),

    // --- PREFERENCES (the settings surface) ---
    // Namespaces are per-app/domain strings ("aether", "stria", "system");
    // keys are dotted paths within a namespace ("homepage", "window.width").
    // Defaults live with the consumer, never in the store — an unset key
    // answers `None`.
    /// Ask for one preference. Answered by [`PrincipiaCommand::PrefValueIs`].
    PrefGet { ns: String, key: String },
    /// The answer to a [`PrincipiaCommand::PrefGet`]: `None` = unset (the
    /// consumer applies its own default).
    PrefValueIs {
        ns: String,
        key: String,
        value: Option<PrefValue>,
    },
    /// Write one preference. On success the store persists atomically and
    /// [`PrincipiaCommand::PrefChanged`] is broadcast; on rejection
    /// [`PrincipiaCommand::PrefError`] comes back instead.
    PrefSet {
        ns: String,
        key: String,
        value: PrefValue,
    },
    /// Ask for every set key in one namespace. Answered by
    /// [`PrincipiaCommand::PrefListIs`].
    PrefList { ns: String },
    /// The answer to a [`PrincipiaCommand::PrefList`]: every `(key, value)`
    /// currently set in `ns`, sorted by key. An unknown namespace lists empty.
    PrefListIs {
        ns: String,
        entries: Vec<(String, PrefValue)>,
    },
    /// Broadcast after every successful set: the new value of `ns`/`key`.
    /// This is both the set's acknowledgement and the live-update signal every
    /// running consumer subscribes to.
    PrefChanged {
        ns: String,
        key: String,
        /// The value STORED — for a declared key written out of range, its
        /// clamp (`prefs_core::schema::check`, PRINCIPIA2 SR32).
        value: PrefValue,
        /// `true` when the written value was out of range and `value` is the
        /// clamp. Omitted from the wire when false, so every unclamped
        /// `PrefChanged` keeps its frozen shape (smessage_kats).
        #[serde(default, skip_serializing_if = "is_false")]
        clamped: bool,
    },
    /// A rejected preference operation (malformed namespace/key, a key that
    /// collides with an existing dotted path, or a failed persist).
    PrefError {
        ns: String,
        key: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MatrixEvent {
    /// Matrix broadcasts the entire topological map of the OS
    IngestTopology {
        ui_dag: String,
        semantic_dag: String,
    },
    /// Surgically appends extracted symbols to an existing node's children
    GraftTopology {
        target_id: String,
        payload: String,
    },
    /// Vein asks Matrix to focus on a specific sector (e.g., "euclase")
    FocusSector(String),
    /// Matrix returns the raw context of that sector
    SectorFocused { target: String, context: String },
    /// Matrix UI fires this when a spatial node is activated
    NodeSelected(PathBuf),
    /// Broadcasts an updated, flattened structural topology back to the UI
    TopologyMutated(Vec<(String, String, usize)>),

    // --- FINDER (the file-browser capability) ---
    // Matrix is the all-asset manager; the Finder is a navigable CURSOR over
    // the filesystem, distinct from the code-topology DAG above. Every verb is
    // principal-stamped (`Origin`) so an in-kernel fulfilment runs with the
    // invoker's grants, never ambient authority (ROADMAP message-security law).

    /// UI → matrix: navigate the browse cursor to a directory (workspace-
    /// relative; `""` = workspace root). Matrix answers with `DirListed`.
    BrowseTo { principal: Origin, path: String },
    /// Matrix → UI: the browse-view listing of the current directory — the flat
    /// file list/grid the vessel renders (NOT the dependency DAG).
    DirListed(BrowseListing),
    /// UI → matrix: a Finder file verb. `arg` is the verb's second operand (new
    /// name for `Rename`/`NewFolder`, destination dir for `Copy`/`Move`, unused
    /// for `Open`/`Delete`). `confirmed` gates the destructive `Delete`:
    /// `false` ⇒ matrix answers `FsOpResult { outcome: NeedsConfirm }`.
    FileOp {
        principal: Origin,
        verb: FsVerb,
        path: String,
        arg: Option<String>,
        confirmed: bool,
    },
    /// Matrix → UI: the outcome of a `FileOp`, principal-attributed. A read-only
    /// volume surfaces here as `Denied`, loudly — never a silent no-op.
    FsOpResult {
        principal: Origin,
        verb: FsVerb,
        path: String,
        outcome: FsOutcome,
    },
}

/// FACET (SR29) — the Images handler's bus vocabulary (`handlers/facet`).
///
/// Every request carries a caller-chosen `receipt_id` that its answer echoes, so many callers can
/// share one broadcast Synapse. Every request that fails is answered by
/// [`FacetCommand::ImageError`] with the same receipt — never silence. Replies are inert as input:
/// Facet hears its own answers on the bus and does not act on them.
///
/// The association: a surface that meets an image file (Matrix's Finder after a successful
/// `FsVerb::Open`, the kernel's Quarry twin, Aether on an image navigation) fires
/// [`FacetCommand::ImageOpen`] instead of opening the bytes itself; Facet decodes and answers
/// [`FacetCommand::ImageOpened`], and a viewer (the `facet-view` vessel) presents that handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FacetCommand {
    /// Open and decode a file. `principal` stamps who asked (the message-security law the Finder
    /// verbs follow). Answered by [`FacetCommand::ImageOpened`] or [`FacetCommand::ImageError`].
    ImageOpen { receipt_id: u64, principal: Origin, path: String },
    /// The answer to [`FacetCommand::ImageOpen`]: the handle every later verb names.
    ImageOpened { receipt_id: u64, handle: u64, info: FacetImageInfo },
    /// Ask for a handle's metadata. Answered by [`FacetCommand::ImageInfoIs`].
    ImageInfo { receipt_id: u64, handle: u64 },
    /// A handle's metadata (also the answer to every [`FacetCommand::ImageEdit`]).
    ImageInfoIs { receipt_id: u64, handle: u64, info: FacetImageInfo },
    /// Append one non-destructive edit to the handle's op list (or undo/redo/reset it). Answered
    /// by [`FacetCommand::ImageInfoIs`] carrying the new dimensions and edit count.
    ImageEdit { receipt_id: u64, handle: u64, edit: FacetEdit },
    /// Render the edited image through `view` into a `width x height` viewport. Answered by
    /// [`FacetCommand::ImageRendered`]. Facet refuses a viewport over 4096 x 4096.
    ImageRender { receipt_id: u64, handle: u64, width: u32, height: u32, view: FacetView },
    /// Straight 8-bit RGBA, `width * height * 4` bytes, row-major, top row first.
    ImageRendered { receipt_id: u64, handle: u64, width: u32, height: u32, rgba: Vec<u8> },
    /// Bake the op list and write the result to `path`. An existing file is refused unless
    /// `overwrite` is set. Answered by [`FacetCommand::ImageExported`].
    ImageExport { receipt_id: u64, handle: u64, path: String, format: FacetFormat, overwrite: bool },
    /// The answer to [`FacetCommand::ImageExport`]: what was written, and how many bytes.
    ImageExported { receipt_id: u64, handle: u64, path: String, bytes: u64 },
    /// Release a handle (no answer; closing an unknown handle is a no-op).
    ImageClose { handle: u64 },
    /// A refused request, by receipt. `handle` is `None` when the request named none (an open).
    ImageError { receipt_id: u64, handle: Option<u64>, message: String },
}

impl FacetCommand {
    /// The "open image" association on the host: the MIME type Facet claims for `path`, judged by
    /// its extension (case-insensitive), or `None` when the file is not Facet's. The host twin of
    /// the kernel type database's `image/png -> facet` row (`fs/assoc.rs`): a surface that meets a
    /// path this answers `Some` for fires [`FacetCommand::ImageOpen`] instead of opening it.
    pub fn image_mime_for(path: &str) -> Option<&'static str> {
        // TYPECORE (B450, R79): the ONE extension table both rings read (`type_core::EXT_TABLE`, the kernel's
        // `fs/filetype.rs` re-exports it), filtered to the types Facet claims — no host twin of the table.
        const FACET_TYPES: &[&str] =
            &[type_core::IMAGE_PNG, type_core::IMAGE_JPEG, type_core::IMAGE_GIF, type_core::IMAGE_BMP, type_core::IMAGE_QOI, type_core::IMAGE_WEBP];
        type_core::by_extension(path).filter(|m| FACET_TYPES.contains(m))
    }

    /// The association for a NAVIGATION (Aether): the local path behind a `file://` URL (or a bare
    /// absolute path) when Facet claims it, else `None` — remote images stay the page's business.
    /// `%XX` escapes are decoded; a query or fragment is dropped.
    pub fn local_image_path(url: &str) -> Option<String> {
        let rest = match url.strip_prefix("file://") {
            Some(r) => r.strip_prefix("localhost").unwrap_or(r),
            None if url.starts_with('/') => url,
            None => return None,
        };
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        let raw = rest.as_bytes();
        let mut out = Vec::with_capacity(raw.len());
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == b'%' && i + 2 < raw.len() {
                if let Some(v) = std::str::from_utf8(&raw[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(raw[i]);
            i += 1;
        }
        let path = String::from_utf8(out).ok()?;
        (path.starts_with('/') && Self::image_mime_for(&path).is_some()).then_some(path)
    }

    /// The receipt this message carries (`None` for [`FacetCommand::ImageClose`]).
    pub fn receipt_id(&self) -> Option<u64> {
        match self {
            FacetCommand::ImageOpen { receipt_id, .. }
            | FacetCommand::ImageOpened { receipt_id, .. }
            | FacetCommand::ImageInfo { receipt_id, .. }
            | FacetCommand::ImageInfoIs { receipt_id, .. }
            | FacetCommand::ImageEdit { receipt_id, .. }
            | FacetCommand::ImageRender { receipt_id, .. }
            | FacetCommand::ImageRendered { receipt_id, .. }
            | FacetCommand::ImageExport { receipt_id, .. }
            | FacetCommand::ImageExported { receipt_id, .. }
            | FacetCommand::ImageError { receipt_id, .. } => Some(*receipt_id),
            FacetCommand::ImageClose { .. } => None,
        }
    }

    /// True for the requests Facet serves (the rest are its own answers, inert as input).
    pub fn is_request(&self) -> bool {
        matches!(
            self,
            FacetCommand::ImageOpen { .. }
                | FacetCommand::ImageInfo { .. }
                | FacetCommand::ImageEdit { .. }
                | FacetCommand::ImageRender { .. }
                | FacetCommand::ImageExport { .. }
                | FacetCommand::ImageClose { .. }
        )
    }
}

impl Default for FacetView {
    fn default() -> Self {
        FacetView { zoom: FacetZoom::Fit, pan_x: 0, pan_y: 0, quarter_turns: 0, flip_h: false, flip_v: false }
    }
}

/// AETHERSIDETABS2 (SR143): one tab of [`SMessage::BrowserTabs`], in strip order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserTabRow {
    pub id: u64,
    /// What the strip shows: the title, else the URL, else `New Tab`.
    pub label: String,
    pub url: String,
    pub loading: bool,
    /// The group the tab sits in (an id of [`BrowserTabGroupRow`]); `None` ungrouped.
    pub group: Option<u64>,
    /// The letter tile's letter when there is no favicon (`aether::chrome::tabs::Tab::initial`).
    pub initial: String,
}

/// AETHERSIDETABS2 (SR143): one tab group of [`SMessage::BrowserTabs`]; its tabs are contiguous in the strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserTabGroupRow {
    pub id: u64,
    pub name: String,
    /// The colour's name (`grey`, `blue`, … `orange`) and its swatch as `0xRRGGBB`.
    pub color: String,
    pub rgb: u32,
    /// Collapsed: the side strip shows only the header, the top strip only the chip.
    pub collapsed: bool,
}

/// What Facet knows about an open image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FacetImageInfo {
    pub path: String,
    /// Container format (`"png"`, `"jpeg"`, `"gif"`, `"bmp"`, `"qoi"`, `"webp"`).
    pub format: String,
    /// Dimensions as stored in the file.
    pub source_width: u32,
    pub source_height: u32,
    /// Dimensions after EXIF orientation and every edit in the op list.
    pub width: u32,
    pub height: u32,
    /// The EXIF orientation found in the file (1..=8, 1 when absent). Facet APPLIES it.
    pub orientation: u8,
    /// The colour space the file declares, as Facet read it (`"sRGB chunk"`, `"ICC profile
    /// 'Display P3' (536 bytes)"`, `"untagged (assumed sRGB)"`, ...). Noted, not managed.
    pub colour: String,
    /// Bits per sample as stored.
    pub bit_depth: u8,
    pub has_alpha: bool,
    /// Animation frames (1 for a still).
    pub frames: u32,
    /// File size in bytes.
    pub bytes: u64,
    /// Ops currently in force in the edit list.
    pub edits: u32,
}

/// One non-destructive edit. Coordinates are in the image as it stands after the previous ops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FacetEdit {
    Crop { x: u32, y: u32, width: u32, height: u32 },
    /// Clockwise quarter turns (1 = 90 degrees).
    Rotate { quarter_turns: u8 },
    /// `horizontal: true` mirrors left/right; `false` mirrors top/bottom.
    Flip { horizontal: bool },
    /// Resample to exactly `width x height` with Facet's documented triangle filter.
    Resize { width: u32, height: u32 },
    /// CSS filter semantics, applied in that order: `brightness(b) contrast(c)`; 1.0 = identity.
    Adjust { brightness: f32, contrast: f32 },
    Undo,
    Redo,
    /// Drop the whole op list (undoable).
    Reset,
}

/// How a render frames the image. View state is never baked into an export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FacetView {
    pub zoom: FacetZoom,
    /// Displacement of the picture from its centred position, viewport pixels (clamped so no gap
    /// opens on an axis where the picture is larger than the viewport).
    pub pan_x: i32,
    pub pan_y: i32,
    /// Clockwise quarter turns of the view (display only).
    pub quarter_turns: u8,
    pub flip_h: bool,
    pub flip_v: bool,
}

/// The view's zoom.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum FacetZoom {
    /// Shrink to fit the viewport, never enlarge (the kernel viewer's rule).
    Fit,
    /// 100 %: one image pixel per viewport pixel.
    Actual,
    /// Percent of the image's size (1..=6400).
    Percent(u32),
}

/// An export container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FacetFormat {
    Png,
    /// Owed: no UnaOS JPEG encoder exists yet; Facet answers `ImageError` naming the gap.
    Jpeg,
}

/// Console log-viewer events (the macOS `Console.app` model): the app is a
/// SUBSCRIBER to the system log feed that maintains a bounded scrollback and
/// publishes a viewable snapshot. Three commands flow view→handler and one
/// render message flows handler→view; the `comscan` handler owns the ring in
/// between. Mirrors [`MatrixEvent`] — a self-contained sub-enum whose matches
/// stay local to its owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogEvent {
    /// view→handler: set the text filter — a case-insensitive substring tested
    /// against a record's `content`, `source`, and `level`. The empty string
    /// clears it (every record passes).
    LogFilter(String),
    /// view→handler: choose which subsystem/source to show (the facet axis).
    /// [`LogSource::All`] shows everything.
    LogSource(LogSource),
    /// view→handler: scroll-lock. `true` freezes the live tail — the ring keeps
    /// ingesting new records, but no fresh [`LogEvent::LogTail`] is emitted on
    /// ingest; `false` resumes and immediately re-emits the current snapshot.
    LogPause(bool),
    /// handler→view: the current bounded, filtered scrollback snapshot, plus the
    /// since-boot eviction count and the pause flag. The single message the
    /// Console vessel renders. Never consumed by the handler (it does not react
    /// to its own output), so publisher and subscriber can share one bus.
    LogTail {
        lines: Vec<LogLine>,
        dropped: u64,
        paused: bool,
    },
}

/// The trait that defines a "Nerve Ending" in the system.
pub trait BandyMember {
    fn publish(&self, topic: &str, msg: SMessage) -> anyhow::Result<()>;
}

/// TABULABUS (SR74) — the Text handler's bus vocabulary (`handlers/tabula`, `tabula::ignite`).
///
/// Documents are keyed by `path` (the string the opener named). Requests carry `principal`
/// (the message-security law the Finder and Facet verbs follow). Every request that fails is
/// answered by [`TabulaCommand::EditorError`] for the same path — never silence. Answers are inert
/// as input: Tabula hears its own replies on the bus and does not act on them.
///
/// Tabula also keeps the older pane vocabulary alive: after every [`TabulaCommand::EditorLoaded`]
/// it fires [`SMessage::EditorLoad`], the message quartzite's editor pane renders, and it applies
/// [`SMessage::EditorEdited`] (the view's full-buffer echo) to the document it last loaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TabulaCommand {
    /// Open `path` (or re-present it, unsaved edits included, when it is already open). A console
    /// or serial log (`UNAOS.LOG`, `*.log`, squawk `*.out`) is name-routed to the read-only Console
    /// view. Answered by [`TabulaCommand::EditorLoaded`] or [`TabulaCommand::EditorError`].
    EditorOpen { principal: Origin, path: String },
    /// The answer to an open: the whole buffer, its line count (`str::lines`), the language id and
    /// whether the document refuses writes (a log).
    EditorLoaded { path: String, content: String, lines: u32, language: String, read_only: bool },
    /// Write the held buffer back to `path`. Answered by [`TabulaCommand::EditorSaved`] (then
    /// [`TabulaCommand::EditorDirty`] `false` when it was dirty) or [`TabulaCommand::EditorError`].
    EditorSave { principal: Origin, path: String },
    /// The answer to a save that reached the disk.
    EditorSaved { path: String },
    /// Move the document's cursor to `line`, `col` — both 1-based, `col` counted in chars,
    /// clamped to the text. No answer on success; the view may scroll on hearing it.
    EditorGoto { principal: Origin, path: String, line: u32, col: u32 },
    /// Insert `text` at the document's cursor and advance the cursor past it. Answered by
    /// [`TabulaCommand::EditorDirty`] `true` when the document was clean (and the new buffer as
    /// [`SMessage::EditorLoad`] when it is the pane's document).
    EditorInsert { principal: Origin, path: String, text: String },
    /// The document's unsaved-edits flag changed.
    EditorDirty { path: String, dirty: bool },
    /// A fact the vessel announces: the document left the pane. Tabula drops it (no answer).
    EditorClosed { path: String },
    /// A refused request, by path, with the reason.
    EditorError { path: String, why: String },
}

impl TabulaCommand {
    /// The document path this message names.
    pub fn path(&self) -> &str {
        match self {
            TabulaCommand::EditorOpen { path, .. }
            | TabulaCommand::EditorLoaded { path, .. }
            | TabulaCommand::EditorSave { path, .. }
            | TabulaCommand::EditorSaved { path }
            | TabulaCommand::EditorGoto { path, .. }
            | TabulaCommand::EditorInsert { path, .. }
            | TabulaCommand::EditorDirty { path, .. }
            | TabulaCommand::EditorClosed { path }
            | TabulaCommand::EditorError { path, .. } => path,
        }
    }

    /// True for what Tabula consumes (the requests, plus the vessel's `EditorClosed` fact); the
    /// rest are its own answers, inert as input.
    pub fn is_request(&self) -> bool {
        matches!(
            self,
            TabulaCommand::EditorOpen { .. }
                | TabulaCommand::EditorSave { .. }
                | TabulaCommand::EditorGoto { .. }
                | TabulaCommand::EditorInsert { .. }
                | TabulaCommand::EditorClosed { .. }
        )
    }
}

/// SURFACEID (LEDGER SR332): the ONE list of the pointer messages that carry a Surface number. Every receiver that routes
/// by surface (stria's `nle_face::Face::takes`) and every sender that stamps one (stria-view's witness) reads it here.
impl SMessage {
    /// The Surface number a pointer message carries; `None` for every other message.
    pub fn surface(&self) -> Option<u32> {
        match self {
            SMessage::BrowserButton { surface, .. }
            | SMessage::BrowserMouseMove(_, _, surface)
            | SMessage::BrowserScroll(_, _, surface)
            | SMessage::BrowserClick(_, _, surface) => Some(*surface),
            _ => None,
        }
    }
    /// The same field, to stamp.
    pub fn surface_mut(&mut self) -> Option<&mut u32> {
        match self {
            SMessage::BrowserButton { surface, .. }
            | SMessage::BrowserMouseMove(_, _, surface)
            | SMessage::BrowserScroll(_, _, surface)
            | SMessage::BrowserClick(_, _, surface) => Some(surface),
            _ => None,
        }
    }
}
