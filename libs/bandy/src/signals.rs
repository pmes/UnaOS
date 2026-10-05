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
    SurfaceBlit { url: String, width: u32, height: u32, pixels: Vec<u8> },
    PlayMedia { url: String, title: String, mime: String },
    BrowserNavBack,
    BrowserNavForward,
    BrowserNavReload,
    BrowserScroll(f64, f64),
    BrowserClick(f64, f64),
    BrowserResize(u32, u32),
    BrowserKey(String),
    BrowserText(String),
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
    MediaFrame { url: String, pts_ns: i64, width: u32, height: u32, rgba: Vec<u8> },
    /// Stria → requester: playback reached the end (or was stopped).
    MediaEnded { url: String, presented: u64, dropped: u64 },
    MediaError { url: String, error: String },

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

/// serde helper: omit a `false` flag from the wire.
fn is_false(b: &bool) -> bool {
    !*b
}

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
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        Some(match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" | "jpe" | "jfif" => "image/jpeg",
            "gif" => "image/gif",
            "bmp" | "dib" => "image/bmp",
            "qoi" => "image/qoi",
            "webp" => "image/webp",
            _ => return None,
        })
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
