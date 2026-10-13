//! AETHERVIDEO (LEDGER SR39): `<video>` and `<audio>` as a client of Stria's media bus.
//!
//! Aether renders, Stria plays (CODEX Amendment II). This module is the page side of that
//! split: it finds the media elements of the current document, resolves each one's source
//! (HTML §4.8.11.5 resource selection: the `src` attribute, else the first `<source>` child
//! whose `type` this engine can play), and speaks the PLAYBACK verbs (`bandy::SMessage`,
//! STRIA section) keyed by that url:
//!
//! | page event | out (Aether → Stria) | back (Stria → Aether) | page effect |
//! |---|---|---|---|
//! | a `<video>` is laid out | `MediaPoster` | `MediaOpened`, one `MediaFrame` | intrinsic size, first frame painted |
//! | `autoplay` + `muted` | `PlayMedia` | `MediaOpened`, `MediaFrame`… | plays from load |
//! | bare `autoplay` | (poster only) | | blocked, as Chromium's autoplay policy |
//! | click / controls play | `PlayMedia` (or `MediaSeek 0` first when ended) | `MediaFrame`… | frame at pts painted |
//! | click / controls pause | `MediaPause` | | last frame stays |
//! | seek | `MediaSeek` | the frame at the target | |
//! | navigation away | `MediaStop` | `MediaEnded` | |
//! | | | `MediaEnded` | state ended, last frame stays |
//! | | | `MediaError` | the error text paints in the box |
//!
//! AUDIOTRACK (LEDGER SR45) added the sound side:
//!
//! * `<audio>` asks for `MediaPoster` on load too (HTML `preload`'s default is the UA's;
//!   Chromium's is `metadata`; `preload="none"` waits for play) — Stria answers an audio-only
//!   stream with a 0×0 `MediaOpened` (the duration) and meter frames, whose time moves the
//!   control's clock; `<audio controls>` paints Chromium's default audio control
//!   (`render::paint_audio_controls`).
//! * `muted` is honoured: every opening request of a muted element is followed by
//!   `MediaMute { muted: true }` (Stria zeroes the samples; the clock runs on).
//! * **http(s) media**: Stria opens local files only, so Aether fetches the resource into a
//!   cache file — `~/.cache/unaos/aether/media/<sha256(url)>` (`$XDG_CACHE_HOME` honoured) — and
//!   hands Stria `file://<that path>`; the cache path is the element's bus key while `src` keeps
//!   the page's url. The fetch runs off the engine thread: byte-range requests (`Range: bytes=`,
//!   1 MiB at a time, resuming a partial `.part` file) when the server answers 206, else one
//!   whole GET; the element's opening request waits for it ([`take_outbox`] releases it), a
//!   failure becomes a `MediaError` for that key, painted in the box.
//!
//! AETHERMEDIA (LEDGER SR107) added the HTMLMediaElement model (HTML §4.8.11) on top of that wire:
//!
//! * `readyState` (HAVE_NOTHING → HAVE_METADATA on `MediaOpened` → HAVE_CURRENT/FUTURE/ENOUGH_DATA on
//!   the first picture, or at once for an audio-only stream: Stria opened a local file, every byte
//!   is there) and `networkState` (EMPTY / IDLE / LOADING / NO_SOURCE) per element.
//! * The events, in HTML's order, queued as tasks on the page's event loop (`js::Task::Event` —
//!   the same queue the HTMLMediaElement binding in `js/html.rs` uses): `loadstart` (+ `suspend`
//!   under `preload=none`), `durationchange`, `loadedmetadata`, `resize`, `loadeddata`, `canplay`,
//!   `canplaythrough`, `play` (autoplay, the user's click), `playing` (the first picture after a
//!   play), `timeupdate` (every 250 ms of media time, and on pause / seek / end), `seeking` /
//!   `seeked`, `pause`, `ended`, `error` (`MediaError` code 4 before metadata, 3 after),
//!   `volumechange`, `ratechange`. `play`/`pause` from script are fired by the binding itself, so
//!   [`play`] / [`pause`] never fire them twice.
//! * `loop` (read live from the attribute, so `el.loop = true` works through reflection): at
//!   `MediaEnded` the element seeks to 0 and plays on — no `ended`, no `pause`.
//! * `currentTime` during a seek reads the target (the frame follows); `seeking` until it lands.
//! * `volume` → `MediaVolume` (Stria scales the samples; `muted` stays its own `MediaMute`).
//! * `playbackRate` → `MediaRate` (Stria: 0 holds the clock while the element stays "potentially
//!   playing", as HTML says; other rates scale the clock where Stria can, see `Player::set_rate`).
//!   Both reach a session that opens later with its opening request.
//!
//! AETHERMEDIA2 (LEDGER SR134) added, on the same element model:
//!
//! * **Media Source Extensions** ([`mse`], `js/mse.rs`): a `src` that is a MediaSource's object URL
//!   attaches it; the element's bus key becomes the MediaSource's journal (`mediasource:<path>`),
//!   Stria plays the appended segments through its `MediaSourceFeed`, and readyState follows the
//!   model's `buffered` around the current position ([`mse_changed`]) — `waiting` when it drops
//!   while playing, `ended` from Stria after endOfStream.
//! * **Text tracks** ([`tracks`], `js/tracks.rs`): `<track>` WebVTT, TextTrack / VTTCue, `cuechange`;
//!   the showing cues ride [`Paint::captions`] to render/media_overlay.rs.
//! * **Autoplay**: bare `autoplay` plays once the document has sticky user activation
//!   ([`note_user_activation`]); without it the element shows the play affordance
//!   ([`Paint::blocked`]) and script `play()` rejects with NotAllowedError ([`play_allowed`]).
//! * **preload**: `none` fetches nothing (an http(s) source waits for play), `auto` fetches the whole
//!   resource and fires `progress` then `suspend` when it is in the cache.
//! * `requestVideoFrameCallback` reads [`frame_info`] (frames presented, media time, size).
//! * `canPlayType` answers from [`play_support`]: "probably" for codecs Stria decodes (VP9 since
//!   VP9CORE SR116, with VP8, AV1, the audio codecs), "maybe" for a codec it would show as the labelled
//!   stand-in (H.264, HEVC).
//!
//! Decoding is never Aether's: every picture and sample comes from Stria, which decodes through
//! the owned cores (`gneiss_pal::dsp` = `demux_core`, `vp8_core`, `av1_core` behind its `av1`
//! feature, `audio_core`). The current picture paints from ONE call site,
//! `render/mod.rs` → `crate::media::paint_for` (the `video`/`audio` arm of the box painter).
//!
//! State lives in a thread-local registry — the same pattern as `images::STORE` — because the
//! engine, layout (intrinsic size) and renderer (frame, poster, controls) all run on the engine
//! thread and each needs the element's media state by DOM node. The shell drains requests with
//! [`take_outbox`] (`AetherEngine::take_media_requests`) and feeds Stria's replies back with
//! [`on_message`] (`AetherEngine::on_media_message`).

use bandy::SMessage;
use crate::dom::NodeRef;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;

/// Which media element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
}

/// Where one element's playback is, as Aether knows it from the bus.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// No session asked for yet (an `<audio>` before its first play).
    Idle,
    /// A request went out; nothing has come back.
    Loading,
    Paused,
    Playing,
    Ended,
    /// Stria's `MediaError` text.
    Error(String),
}

/// One media element of the current page.
#[derive(Clone)]
pub struct Element {
    pub node: NodeRef,
    pub kind: Kind,
    /// The bus key Stria knows the stream by: the resolved source url, or for an http(s)
    /// source `file://` + its cache file (AUDIOTRACK M4).
    pub url: String,
    /// The resolved source as the page names it (absolute url).
    pub src: String,
    pub mime: String,
    /// The `poster` attribute, resolved (video only).
    pub poster: Option<String>,
    pub autoplay: bool,
    pub muted: bool,
    pub controls: bool,
    pub state: State,
    /// HTML's "show poster flag": set until playback starts or a seek happens. While set and the
    /// element has a decodable poster image, the poster is painted instead of the video frame.
    pub show_poster: bool,
    /// The video's natural size from `MediaOpened`.
    pub natural: Option<(u32, u32)>,
    pub duration_ns: u64,
    /// pts of the frame on glass.
    pub pts_ns: i64,
    pub frame: Option<Rc<image::RgbaImage>>,
    /// False when Stria is painting its labelled test-pattern stand-in (no decoder for the codec).
    pub real_video: bool,
    /// The latest audio meter: peak |sample| left / right over the last 50 ms window.
    pub levels: [f32; 2],
    /// An http(s) source still being fetched into the cache.
    pub fetching: bool,
    /// The opening request held back until the fetch completes.
    pub deferred: Option<Deferred>,
    /// HTMLMediaElement `readyState` (HTML §4.8.11.7): one of the `HAVE_*` constants.
    pub ready_state: u8,
    /// HTMLMediaElement `networkState` (HTML §4.8.11.5): one of the `NETWORK_*` constants.
    pub network_state: u8,
    /// `seeking`, and the position asked for.
    pub seeking: bool,
    /// `playbackRate` (page-side; see the module doc).
    pub rate: f64,
    /// `volume` (0..=1; page-side, 0 forwarded as mute).
    pub volume: f64,
    /// `MediaError.code` (1 aborted, 2 network, 3 decode, 4 src not supported); 0 = `error` is null.
    pub error_code: u16,
    /// pts at the last `timeupdate`.
    pub last_timeupdate_ns: i64,
    /// A play happened and `playing` has not been fired for it yet.
    pub playing_owed: bool,
    /// Every media event fired at this element, in order (tests, the headless report).
    pub events: Vec<&'static str>,
    /// AETHERMEDIA2 (SR134): the MediaSource attached through `src` (its [`mse`] id).
    pub mse: Option<u32>,
    /// AETHERMEDIA2: autoplay or `play()` refused for want of user activation — the play affordance shows.
    pub blocked: bool,
    /// AETHERMEDIA2: pictures presented since load (`requestVideoFrameCallback`'s presentedFrames).
    pub presented: u64,
    /// AETHERMEDIA2: `preload=auto` — `progress` + `suspend` when the whole resource is local.
    pub preload_auto: bool,
}

/// `readyState` values (HTML §4.8.11.7).
pub const HAVE_NOTHING: u8 = 0;
pub const HAVE_METADATA: u8 = 1;
pub const HAVE_CURRENT_DATA: u8 = 2;
pub const HAVE_FUTURE_DATA: u8 = 3;
pub const HAVE_ENOUGH_DATA: u8 = 4;
/// `networkState` values (HTML §4.8.11.5).
pub const NETWORK_EMPTY: u8 = 0;
pub const NETWORK_IDLE: u8 = 1;
pub const NETWORK_LOADING: u8 = 2;
pub const NETWORK_NO_SOURCE: u8 = 3;
/// HTML: `timeupdate` fires every 15 to 250 ms during playback; Aether counts media time.
pub const TIMEUPDATE_NS: i64 = 250_000_000;
/// The event log's cap per element.
const EVENT_LOG_MAX: usize = 512;

/// Fire the simple event `ty` at `node` (not bubbling, not cancelable): logged on the element and
/// queued as a task on the page's event loop when `node` belongs to the page's document (the
/// HTML "queue a media element task"). A page with no script engine simply never runs the task.
fn queue_event(node: &NodeRef, ty: &'static str) {
    let id = node.id().0;
    let doc = node.document_node();
    let _ = crate::js::PAGE.try_with(|p| {
        if let Ok(mut p) = p.try_borrow_mut() {
            if p.doc.as_ref() == Some(&doc) {
                p.tasks.push_back(crate::js::Task::Event(crate::js::TargetKey::Node(id), ty.to_string(), false));
            }
        }
    });
}

/// AETHERMEDIA2 (SR134): queue the simple event `ty` at any event target of the page (a node, or a
/// script-made EventTarget by its serial — MediaSource, SourceBuffer, TextTrack, VTTCue).
pub(crate) fn queue_target_event(key: crate::js::TargetKey, ty: &str) {
    let _ = crate::js::PAGE.try_with(|p| {
        if let Ok(mut p) = p.try_borrow_mut() {
            if p.doc.is_some() {
                p.tasks.push_back(crate::js::Task::Event(key, ty.to_string(), false));
            }
        }
    });
}

fn fire(e: &mut Element, ty: &'static str) {
    if e.events.len() < EVENT_LOG_MAX {
        e.events.push(ty);
    }
    queue_event(&e.node, ty);
}

/// The element's `loop` attribute, read live (IDL `loop` reflects it).
fn looping(node: &NodeRef) -> bool {
    node.as_element().is_some_and(|el| el.attributes.borrow().get("loop").is_some())
}

/// Advance `readyState` to `to`, firing every event HTML fires on the way (§4.8.11.7), then
/// `playing` when a play is owed and data is there.
fn advance_ready(e: &mut Element, to: u8) {
    if to >= HAVE_METADATA && e.ready_state < HAVE_METADATA {
        e.ready_state = HAVE_METADATA;
        fire(e, "durationchange");
        if e.kind == Kind::Video && e.natural.is_some() {
            fire(e, "resize");
        }
        fire(e, "loadedmetadata");
    }
    if to >= HAVE_CURRENT_DATA && e.ready_state < HAVE_CURRENT_DATA {
        e.ready_state = HAVE_CURRENT_DATA;
        fire(e, "loadeddata");
    }
    if to >= HAVE_FUTURE_DATA && e.ready_state < HAVE_FUTURE_DATA {
        e.ready_state = HAVE_FUTURE_DATA;
        fire(e, "canplay");
    }
    if to >= HAVE_ENOUGH_DATA && e.ready_state < HAVE_ENOUGH_DATA {
        e.ready_state = HAVE_ENOUGH_DATA;
        fire(e, "canplaythrough");
        // a local file (Stria's, or the finished cache file) is whole: the fetch is idle
        e.network_state = NETWORK_IDLE;
    }
    if e.playing_owed && e.state == State::Playing && e.ready_state >= HAVE_FUTURE_DATA {
        e.playing_owed = false;
        fire(e, "playing");
    }
}

/// The request an element sends once its source is local.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deferred {
    Poster,
    Play,
}

#[derive(Default)]
struct Registry {
    elements: Vec<Element>,
    /// Elements whose every `<source>` was refused (the `error` at the last one already fired).
    refused: Vec<NodeRef>,
    outbox: Vec<SMessage>,
    title: String,
}

thread_local! {
    static MEDIA: RefCell<Registry> = RefCell::new(Registry::default());
}

// ------------------------------------------------------------------ the http(s) media cache

/// Fetches finished off-thread: (src, Ok(()) | Err(text)).
static FETCHED: Mutex<Vec<(String, Result<(), String>)>> = Mutex::new(Vec::new());
/// Sources being fetched (a second element on the same url waits for the same fetch).
static INFLIGHT: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static CACHE_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Range request size.
pub const FETCH_CHUNK: u64 = 1 << 20;
/// The largest resource the cache takes.
pub const FETCH_MAX: u64 = 1 << 30;

/// Where cached media lives: `$XDG_CACHE_HOME/unaos/aether/media`, else
/// `~/.cache/unaos/aether/media` (or the override [`set_cache_dir`] gave).
pub fn cache_dir() -> PathBuf {
    if let Some(d) = CACHE_DIR.lock().unwrap().clone() {
        return d;
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("unaos/aether/media")
}

/// Point the cache somewhere else (tools, tests).
pub fn set_cache_dir(dir: &Path) {
    *CACHE_DIR.lock().unwrap() = Some(dir.to_path_buf());
}

/// The cache file of `src`: the lowercase hex SHA-256 of the url (UnaOS's own `crypto_core`).
pub fn cache_path(src: &str) -> PathBuf {
    let h = crypto_core::sha2::sha256(src.as_bytes());
    cache_dir().join(h.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

pub fn is_remote(src: &str) -> bool {
    let l = src.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// What one fetch did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchReport {
    pub bytes: u64,
    /// The server honoured byte ranges (206 + Content-Range).
    pub ranged: bool,
    pub requests: u32,
    /// Bytes already in a partial file that a range request resumed after.
    pub resumed_from: u64,
}

/// `Content-Range: bytes a-b/total` → (a, b, total).
fn content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let r = v.trim().strip_prefix("bytes")?.trim();
    let (span, total) = r.split_once('/')?;
    let (a, b) = span.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?, total.trim().parse().ok()))
}

/// Fetch `src` into `dest` (write to `dest.part`, rename when whole). Byte ranges of `chunk`
/// when the server answers 206 (resuming whatever a previous attempt left in `.part`), else the
/// whole body from one 200. Blocking: run it off the engine thread.
pub fn fetch_to_cache(src: &str, dest: &Path, chunk: u64) -> Result<FetchReport, String> {
    use gneiss_pal::api::http;
    use std::io::{Read, Write};
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let part = dest.with_extension("part");
    let client = crate::net::blocking_client_builder().build().map_err(|e| e.to_string())?;
    let mut rep = FetchReport::default();
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    rep.resumed_from = have;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    loop {
        let range = format!("bytes={}-{}", have, have + chunk - 1);
        // HTTPCORE (SR51): byte ranges count octets of the stored representation — ask for it uncoded.
        let mut resp = client
            .get(src)
            .header(http::header::RANGE, &range)
            .header(http::header::ACCEPT_ENCODING, "identity")
            .send()
            .map_err(|e| format!("{src}: {e}"))?;
        rep.requests += 1;
        let status = resp.status().as_u16();
        match status {
            206 => {
                rep.ranged = true;
                let cr = resp.headers().get(http::header::CONTENT_RANGE).and_then(content_range);
                let Some((a, _b, total)) = cr else { return Err(format!("{src}: 206 without a Content-Range")) };
                if a != have {
                    return Err(format!("{src}: asked for byte {have}, got a range from {a}"));
                }
                let mut body = Vec::new();
                resp.read_to_end(&mut body).map_err(|e| format!("{src}: {e}"))?;
                file.write_all(&body).map_err(|e| e.to_string())?;
                have += body.len() as u64;
                if have > FETCH_MAX {
                    return Err(format!("{src}: larger than the {} MiB cache limit", FETCH_MAX >> 20));
                }
                let done = match total {
                    Some(t) => have >= t,
                    None => (body.len() as u64) < chunk,
                };
                if done || body.is_empty() {
                    break;
                }
            }
            200 => {
                // ranges not supported: the whole resource, from the start
                drop(file);
                let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
                rep.resumed_from = 0;
                let mut buf = vec![0u8; 64 * 1024];
                have = 0;
                loop {
                    let n = resp.read(&mut buf).map_err(|e| format!("{src}: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                    have += n as u64;
                    if have > FETCH_MAX {
                        return Err(format!("{src}: larger than the {} MiB cache limit", FETCH_MAX >> 20));
                    }
                }
                break;
            }
            // the partial file already holds everything
            416 if have > 0 => break,
            _ => return Err(format!("{src}: HTTP {status}")),
        }
    }
    rep.bytes = have;
    std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(rep)
}

/// Start fetching `src` unless it is cached or already on its way. Returns true when the
/// caller must wait for [`take_outbox`] to release the element.
fn ensure_cached(src: &str) -> bool {
    let dest = cache_path(src);
    if dest.exists() {
        return false;
    }
    let mut inflight = INFLIGHT.lock().unwrap();
    let set = inflight.get_or_insert_with(HashSet::new);
    if set.insert(src.to_string()) {
        let s = src.to_string();
        std::thread::Builder::new()
            .name("aether-media-fetch".into())
            .spawn(move || {
                let r = fetch_to_cache(&s, &dest, FETCH_CHUNK).map(|rep| {
                    eprintln!("[AETHER] :: media cached {s} ({} bytes, ranged={}, {} requests)", rep.bytes, rep.ranged, rep.requests);
                });
                if let Some(set) = INFLIGHT.lock().unwrap().as_mut() {
                    set.remove(&s);
                }
                FETCHED.lock().unwrap().push((s, r));
            })
            .ok();
    }
    true
}

/// The opening request(s) for one element: the request, then `MediaMute` when muted.
fn open_requests(out: &mut Vec<SMessage>, e: &Element, what: Deferred, title: &str) {
    match what {
        Deferred::Poster => out.push(SMessage::MediaPoster { url: e.url.clone() }),
        Deferred::Play => out.push(SMessage::PlayMedia { url: e.url.clone(), title: title.to_string(), mime: e.mime.clone() }),
    }
    settings_requests(out, e);
}

/// What a session needs to match the element's script-set state when it opens: `MediaMute` when
/// muted, `MediaVolume` / `MediaRate` (AETHERMEDIA SR107) when not the defaults.
fn settings_requests(out: &mut Vec<SMessage>, e: &Element) {
    if e.muted {
        out.push(SMessage::MediaMute { url: e.url.clone(), muted: true });
    }
    if e.volume != 1.0 {
        out.push(SMessage::MediaVolume { url: e.url.clone(), volume: e.volume as f32 });
    }
    if e.rate != 1.0 {
        out.push(SMessage::MediaRate { url: e.url.clone(), rate: e.rate });
    }
}

/// Media types this engine hands to Stria. Containers: what `demux_core` parses (MP4, WebM,
/// Matroska) and the bare audio files `dsp::audio` decodes (MP3, Ogg, WAV, FLAC, ADTS, AIFF).
/// Codecs: anything Stria opens — real decoders (`utp1` test pattern, VP8 (vp8_core), VP9
/// (vp9_core, VP9CORE SR116), AV1 (av1_core), PCM, Opus, Vorbis, AAC-LC, MP3 (`mp3`, `mp4a.6B`,
/// WAV's `1`), FLAC) and the codecs it plays as the labelled stand-in (H.264, HEVC: no owned
/// decoder). An unknown codec makes the `<source>` unplayable, so the next one is tried — HTML's
/// `canPlayType` == "" rule. [`play_support`] tells the two playable kinds apart.
pub fn can_play_type(mime: &str) -> bool {
    let mut parts = mime.split(';');
    let base = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let container_ok = matches!(
        base.as_str(),
        "video/mp4" | "audio/mp4" | "video/webm" | "audio/webm" | "video/x-matroska" | "audio/x-matroska" | "video/quicktime"
            // AUDIOTRACK (SR45): the bare audio files Stria opens through `dsp::audio`
            | "audio/mpeg" | "audio/mp3" | "audio/ogg" | "application/ogg" | "audio/wav" | "audio/wave" | "audio/x-wav"
            | "audio/flac" | "audio/x-flac" | "audio/aac" | "audio/aiff" | "audio/x-aiff"
    );
    if !container_ok {
        return false;
    }
    for p in parts {
        let p = p.trim();
        let Some(v) = p.strip_prefix("codecs=").or_else(|| p.strip_prefix("codecs =")) else { continue };
        for c in v.trim_matches(|c| c == '"' || c == '\'').split(',') {
            let c = c.trim().to_ascii_lowercase();
            let known = ["utp1", "av01", "av1", "vp8", "vp9", "vp09", "avc1", "avc3", "hvc1", "hev1", "opus", "vorbis", "mp4a", "flac", "pcm", "sowt", "ipcm", "mp3", "1"];
            if !known.iter().any(|k| c == *k || c.starts_with(&format!("{k}."))) {
                return false;
            }
        }
    }
    true
}

/// AETHERMEDIA2 (SR134): `canPlayType`'s three answers — 0 "" (not playable), 1 "maybe" (no `codecs`
/// parameter, or a codec Stria would show as the labelled stand-in: `avc1`/`avc3`/`hvc1`/`hev1`),
/// 2 "probably" (every named codec has a real decoder in Stria).
pub fn play_support(mime: &str) -> u8 {
    if !can_play_type(mime) {
        return 0;
    }
    let mut named = false;
    for p in mime.split(';').skip(1) {
        let p = p.trim();
        let Some(v) = p.strip_prefix("codecs=").or_else(|| p.strip_prefix("codecs =")) else { continue };
        for c in v.trim_matches(|c| c == '"' || c == '\'').split(',') {
            let c = c.trim().to_ascii_lowercase();
            named = true;
            if ["avc1", "avc3", "hvc1", "hev1"].iter().any(|k| c == *k || c.starts_with(&format!("{k}."))) {
                return 1;
            }
        }
    }
    if named { 2 } else { 1 }
}

/// The url a media element plays (HTML resource selection): its own `src`, else the first
/// `<source>` child with a `src` whose `type` is absent or playable. Returns (absolute url, mime).
pub fn source_for(node: &NodeRef, base: &str) -> Option<(String, String)> {
    let el = node.as_element()?;
    let attrs = el.attributes.borrow();
    if let Some(src) = attrs.get("src").filter(|s| !s.trim().is_empty()) {
        let abs = crate::images::resolve(base, src.trim());
        let mime = attrs.get("type").map(str::to_string).unwrap_or_else(|| mime_for(&abs).to_string());
        return Some((abs, mime));
    }
    drop(attrs);
    for child in node.children() {
        let Some(c) = child.as_element() else { continue };
        if c.name.local.as_ref() != "source" {
            continue;
        }
        let a = c.attributes.borrow();
        let Some(src) = a.get("src").filter(|s| !s.trim().is_empty()) else { continue };
        if let Some(t) = a.get("type") {
            if !can_play_type(t) {
                continue;
            }
        }
        let abs = crate::images::resolve(base, src.trim());
        let mime = a.get("type").map(str::to_string).unwrap_or_else(|| mime_for(&abs).to_string());
        return Some((abs, mime));
    }
    None
}

/// MIME type from a url's extension (the `type`-less fallback).
pub fn mime_for(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mov") => "video/quicktime",
        Some("m4a") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("opus") => "audio/ogg",
        Some("aac") => "audio/aac",
        Some("aif") | Some("aiff") => "audio/aiff",
        _ => "application/octet-stream",
    }
}

fn find(r: &Registry, node: &NodeRef) -> Option<usize> {
    r.elements.iter().position(|e| &e.node == node)
}

/// The page title PlayMedia requests carry (set once the document's title is known).
pub fn set_title(title: &str) {
    MEDIA.with(|m| m.borrow_mut().title = title.to_string());
}

/// Forget the previous document's media. Every url that had a Stria session gets a
/// `MediaStop` in the (new) outbox, so navigating away stops playback.
pub fn reset(title: &str) {
    // AETHERMEDIA2 (SR134): MediaSources detach (sourceclose), text tracks and activation go with the page
    mse::reset();
    tracks::reset();
    STICKY.with(|s| s.set(false));
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let mut stops: Vec<String> = Vec::new();
        for e in &r.elements {
            if e.state != State::Idle && !stops.contains(&e.url) {
                stops.push(e.url.clone());
            }
        }
        r.elements.clear();
        r.refused.clear();
        r.title = title.to_string();
        for url in stops {
            r.outbox.push(SMessage::MediaStop { url });
        }
    });
}

/// Register every `<video>`/`<audio>` of `doc` not yet known (load, and after script DOM
/// mutation) and queue its opening request. Returns true when anything new was found.
pub fn scan(doc: &NodeRef, base: &str) -> bool {
    let Ok(found) = doc.select("video, audio") else { return false };
    let nodes: Vec<NodeRef> = found.map(|m| m.as_node().clone()).collect();
    let any = MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let mut any = false;
        for node in nodes {
            if find(&r, &node).is_some() {
                continue;
            }
            // AETHERMEDIA2 (SR134): a MediaSource object URL attaches the MediaSource (MSE §2.4.1)
            let raw_src = node.as_element().and_then(|e| e.attributes.borrow().get("src").map(|s| s.trim().to_string()));
            if let Some(id) = raw_src.as_deref().and_then(mse::by_url) {
                let Some(url) = mse::attach(id, &node) else { continue };
                let e = mse_element(node.clone(), url, raw_src.unwrap_or_default(), id);
                let title = r.title.clone();
                let mut out = Vec::new();
                open_requests(&mut out, &e, if e.state == State::Playing { Deferred::Play } else { Deferred::Poster }, &title);
                r.outbox.extend(out);
                r.elements.push(e);
                any = true;
                continue;
            }
            let Some((src, mime)) = source_for(&node, base) else {
                // HTML §4.8.11.5: every `<source>` failed — `error` at the last one, networkState
                // NO_SOURCE (the element's own `networkState` getter answers it, see [`network_state`]).
                if !r.refused.contains(&node) {
                    let last = node.children().filter(|c| c.as_element().is_some_and(|e| e.name.local.as_ref() == "source")).last();
                    if let Some(last) = last {
                        crate::ledger::record_dom("media-no-playable-source");
                        queue_event(&last, "error");
                        r.refused.push(node.clone());
                    }
                }
                continue;
            };
            // AETHERDEBT SR93: media-src and mixed content (passive media autoupgrades) decide the fetch.
            let dest = if node.as_element().is_some_and(|e| e.name.local.as_ref() == "video") { "video" } else { "audio" };
            let Some(src) = crate::security::subresource_url(&src, dest) else { continue };
            // http(s): Stria plays the cache file; the fetch may still be running. AETHERMEDIA2: under
            // `preload=none` nothing is fetched until play.
            let preload_none_early = node.as_element().is_some_and(|e| e.attributes.borrow().get("preload").is_some_and(|p| p.trim().eq_ignore_ascii_case("none")));
            let wants_fetch = !preload_none_early || node.as_element().is_some_and(|e| e.attributes.borrow().get("autoplay").is_some());
            let (url, fetching) = if is_remote(&src) {
                (format!("file://{}", cache_path(&src).display()), if wants_fetch { ensure_cached(&src) } else { !cache_path(&src).exists() })
            } else {
                (src.clone(), false)
            };
            let el = node.as_element().unwrap();
            let kind = if el.name.local.as_ref() == "video" { Kind::Video } else { Kind::Audio };
            let attrs = el.attributes.borrow();
            let poster = if kind == Kind::Video {
                attrs
                    .get("poster")
                    .filter(|p| !p.trim().is_empty())
                    .and_then(|p| crate::security::subresource_url(&crate::images::resolve(base, p.trim()), "image"))
            } else {
                None
            };
            let autoplay = attrs.get("autoplay").is_some();
            let muted = attrs.get("muted").is_some();
            let controls = attrs.get("controls").is_some();
            let preload_none = attrs.get("preload").is_some_and(|p| p.trim().eq_ignore_ascii_case("none"));
            let preload_auto = attrs.get("preload").is_some_and(|p| p.trim().eq_ignore_ascii_case("auto"));
            drop(attrs);
            // Chromium's autoplay policy: muted autoplay always runs; unmuted autoplay runs once the
            // document has sticky user activation (HTML §6.4.1) — AETHERMEDIA2 (SR134); before it, the
            // element shows the play affordance (no media-engagement index here).
            let autoplays = autoplay && (muted || has_sticky_activation());
            if autoplay && !autoplays {
                crate::ledger::record_dom("media-autoplay-blocked-unmuted");
            }
            // A session per url: a second element on the same stream shares the first's.
            let shared = r.elements.iter().any(|e| e.url == url && e.state != State::Idle);
            let (state, opening) = if autoplays {
                (State::Playing, Some(Deferred::Play))
            } else if !preload_none {
                // metadata (and a <video>'s first frame): Chromium's default preload
                (State::Loading, (!shared).then_some(Deferred::Poster))
            } else {
                (State::Idle, None)
            };
            let mut e = Element {
                node,
                kind,
                url,
                src,
                mime,
                poster,
                autoplay,
                muted,
                controls,
                state,
                show_poster: !autoplays,
                natural: None,
                duration_ns: 0,
                pts_ns: 0,
                frame: None,
                real_video: false,
                levels: [0.0; 2],
                fetching,
                deferred: None,
                ready_state: HAVE_NOTHING,
                network_state: NETWORK_EMPTY,
                seeking: false,
                rate: 1.0,
                volume: 1.0,
                error_code: 0,
                last_timeupdate_ns: 0,
                playing_owed: autoplays,
                events: Vec::new(),
                mse: None,
                blocked: autoplay && !autoplays,
                presented: 0,
                preload_auto,
            };
            // HTML §4.8.11.5 resource selection: networkState LOADING and `loadstart`; under
            // `preload=none` the fetch suspends at once (`suspend`, networkState IDLE).
            e.network_state = NETWORK_LOADING;
            fire(&mut e, "loadstart");
            if opening.is_none() && !shared {
                e.network_state = NETWORK_IDLE;
                fire(&mut e, "suspend");
            }
            if autoplays {
                // autoplay clears `paused` (`play`); `playing` waits for the first picture
                fire(&mut e, "play");
            }
            if let Some(what) = opening {
                if fetching {
                    e.deferred = Some(what);
                } else {
                    let title = r.title.clone();
                    let mut out = Vec::new();
                    open_requests(&mut out, &e, what, &title);
                    r.outbox.extend(out);
                }
            }
            r.elements.push(e);
            any = true;
        }
        any
    });
    // AETHERMEDIA2 (SR134): every media element's `<track>` children
    if let Ok(found) = doc.select("video, audio") {
        for m in found {
            tracks::scan(m.as_node(), base);
        }
    }
    any
}

/// AETHERMEDIA2 (SR134): the element model of a media element whose `src` attached MediaSource `id`.
/// Autoplay follows the same policy; `loadstart` fires now, readyState follows the MediaSource.
fn mse_element(node: NodeRef, url: String, src: String, id: u32) -> Element {
    let el = node.as_element().unwrap();
    let kind = if el.name.local.as_ref() == "video" { Kind::Video } else { Kind::Audio };
    let a = el.attributes.borrow();
    let autoplay = a.get("autoplay").is_some();
    let muted = a.get("muted").is_some();
    let controls = a.get("controls").is_some();
    drop(a);
    let autoplays = autoplay && (muted || has_sticky_activation());
    let mut e = Element {
        node,
        kind,
        url,
        src,
        mime: String::new(),
        poster: None,
        autoplay,
        muted,
        controls,
        state: if autoplays { State::Playing } else { State::Loading },
        show_poster: !autoplays,
        natural: None,
        duration_ns: 0,
        pts_ns: 0,
        frame: None,
        real_video: false,
        levels: [0.0; 2],
        fetching: false,
        deferred: None,
        ready_state: HAVE_NOTHING,
        network_state: NETWORK_LOADING,
        seeking: false,
        rate: 1.0,
        volume: 1.0,
        error_code: 0,
        last_timeupdate_ns: 0,
        playing_owed: autoplays,
        events: Vec::new(),
        mse: Some(id),
        blocked: autoplay && !autoplays,
        presented: 0,
        preload_auto: false,
    };
    fire(&mut e, "loadstart");
    if autoplays {
        fire(&mut e, "play");
    }
    e
}

/// The readyState the attached MediaSource supports at the element's position (MSE §2.4.4): nothing
/// before every SourceBuffer has an init segment; HAVE_METADATA when the position is not buffered;
/// HAVE_ENOUGH_DATA when at least a second (or the rest, once ended) is buffered past it;
/// HAVE_FUTURE_DATA with more than a frame; else HAVE_CURRENT_DATA.
fn mse_ready(e: &Element) -> u8 {
    let Some(id) = e.mse else { return e.ready_state };
    let Some((init, ended, _, ranges)) = mse::element_view(id) else { return HAVE_NOTHING };
    if !init {
        return HAVE_NOTHING;
    }
    let t = e.pts_ns;
    let Some(&(_, end)) = ranges.iter().find(|&&(a, b)| a <= t && t < b) else {
        return if ended && ranges.last().is_some_and(|r| t >= r.1) { HAVE_ENOUGH_DATA } else { HAVE_METADATA };
    };
    if ended && ranges.last().is_some_and(|r| r.1 == end) {
        return HAVE_ENOUGH_DATA;
    }
    match end - t {
        d if d >= 1_000_000_000 => HAVE_ENOUGH_DATA,
        d if d > 40_000_000 => HAVE_FUTURE_DATA,
        _ => HAVE_CURRENT_DATA,
    }
}

/// Set an MSE element's readyState to what its MediaSource supports: rising fires HTML's events
/// ([`advance_ready`]); falling below HAVE_FUTURE_DATA while playing fires `waiting` (HTML §4.8.11.7).
fn apply_mse_ready(e: &mut Element) {
    let to = mse_ready(e);
    if to >= e.ready_state {
        advance_ready(e, to);
    } else {
        let was = e.ready_state;
        e.ready_state = to;
        if was >= HAVE_FUTURE_DATA && to < HAVE_FUTURE_DATA && e.state == State::Playing {
            e.playing_owed = true;
            fire(e, "waiting");
        }
    }
}

/// AETHERMEDIA2 (SR134): the MediaSource attached to `node` changed (an append, a remove, duration,
/// endOfStream): duration and readyState follow; `durationchange` when `duration_event` and the
/// duration moved after metadata.
pub(crate) fn mse_changed(node: &NodeRef, duration_event: bool) {
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let Some(i) = find(&r, node) else { return };
        let e = &mut r.elements[i];
        let Some(id) = e.mse else { return };
        let Some((_, ended, dur, _)) = mse::element_view(id) else { return };
        let d = match dur {
            Some(d) if d != i64::MAX && d > 0 => d as u64,
            _ => 0,
        };
        if d != e.duration_ns {
            e.duration_ns = d;
            if duration_event && e.ready_state >= HAVE_METADATA {
                fire(e, "durationchange");
            }
        }
        if ended {
            e.network_state = NETWORK_IDLE;
        }
        apply_mse_ready(e);
    });
}

/// AETHERMEDIA2 (SR134): endOfStream("decode"/"network") or an append error: before metadata the
/// element's source is not supported (4), after it a decode error (3) — MSE §2.4.7 step 3.
pub(crate) fn mse_decode_error(node: &NodeRef) {
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let Some(i) = find(&r, node) else { return };
        let e = &mut r.elements[i];
        e.error_code = if e.ready_state == HAVE_NOTHING { 4 } else { 3 };
        e.state = State::Error("MediaSource: decode error".into());
        e.playing_owed = false;
        fire(e, "error");
    });
}

/// `duration` of an MSE element in seconds (NaN / +Infinity), None for any other element.
pub fn mse_duration(node: &NodeRef) -> Option<f64> {
    let id = with_element(node, |e| e.mse).flatten()?;
    Some(mse::duration(id))
}

/// HTMLMediaElement `buffered` as (start, end) ns: the MediaSource's for an MSE element; for a
/// file Stria opened (the whole local file), [0, duration) once metadata is known.
pub fn buffered(node: &NodeRef) -> Vec<(i64, i64)> {
    match with_element(node, |e| (e.mse, e.ready_state, e.duration_ns)) {
        Some((Some(id), _, _)) => mse::buffered(id),
        Some((None, rs, d)) if rs >= HAVE_METADATA && d > 0 => vec![(0, d as i64)],
        _ => Vec::new(),
    }
}

thread_local! {
    /// HTML §6.4.1 sticky activation of the page's document.
    static STICKY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// AETHERMEDIA2 (SR134): a user activation happened on the page (HTML §6.4.1 "activation triggering
/// input event": a trusted click, key or touch). The document keeps sticky activation; blocked
/// `autoplay` elements may play from now on.
pub fn note_user_activation() {
    STICKY.with(|s| s.set(true));
}

/// The document has sticky activation.
pub fn has_sticky_activation() -> bool {
    STICKY.with(|s| s.get())
}

/// Chromium's autoplay policy for `play()` (and `autoplay`): muted media always, audible media once
/// the document has sticky activation. False marks the element blocked (the play affordance).
pub fn play_allowed(node: &NodeRef, muted: bool) -> bool {
    if muted || has_sticky_activation() {
        return true;
    }
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        if let Some(i) = find(&r, node) {
            r.elements[i].blocked = true;
        }
    });
    crate::ledger::record_dom("media-play-not-allowed");
    false
}

/// What `requestVideoFrameCallback` reports: (presentedFrames, mediaTime s, width, height).
pub fn frame_info(node: &NodeRef) -> Option<(u64, f64, u32, u32)> {
    with_element(node, |e| {
        let (w, h) = e.natural.unwrap_or((0, 0));
        (e.presented, e.pts_ns.max(0) as f64 / 1e9, w, h)
    })
}

/// What a reply changed.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Effect {
    /// The message was a media reply for an element of this page.
    pub handled: bool,
    /// An intrinsic size changed: layout must be rebuilt.
    pub relayout: bool,
    /// Nodes whose boxes must repaint.
    pub repaint: Vec<NodeRef>,
}

/// Apply one of Stria's replies (`MediaOpened`, `MediaFrame`, `MediaEnded`, `MediaError`).
/// Anything else, or a reply for a url this page does not show, is ignored.
pub fn on_message(msg: &SMessage) -> Effect {
    let url = match msg {
        SMessage::MediaOpened { url, .. }
        | SMessage::MediaFrame { url, .. }
        | SMessage::MediaEnded { url, .. }
        | SMessage::MediaError { url, .. } => url,
        _ => return Effect::default(),
    };
    // Decode a frame once, share it between elements on the same stream.
    let frame = match msg {
        SMessage::MediaFrame { width, height, rgba, .. } if *width > 0 && *height > 0 => {
            image::RgbaImage::from_raw(*width, *height, rgba.clone()).map(Rc::new)
        }
        _ => None,
    };
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let mut fx = Effect::default();
        // streams a looping element restarts (once per url, after the loop below)
        let mut loops: Vec<String> = Vec::new();
        for e in r.elements.iter_mut().filter(|e| &e.url == url) {
            fx.handled = true;
            match msg {
                SMessage::MediaOpened { duration_ns, width, height, real_video, .. } => {
                    let nat = (*width > 0 && *height > 0).then_some((*width, *height));
                    if e.kind == Kind::Video && nat != e.natural {
                        fx.relayout = true;
                    }
                    let duration_changed = e.ready_state >= HAVE_METADATA && e.duration_ns != *duration_ns;
                    e.natural = nat;
                    e.duration_ns = *duration_ns;
                    e.real_video = *real_video;
                    if e.state == State::Loading {
                        e.state = State::Paused;
                    }
                    if duration_changed {
                        fire(e, "durationchange");
                    }
                    // an audio-only stream has no picture to wait for: the local file is whole
                    let to = if nat.is_none() { HAVE_ENOUGH_DATA } else { HAVE_METADATA };
                    if e.mse.is_some() {
                        // AETHERMEDIA2: the MediaSource decides readiness and duration
                        apply_mse_ready(e);
                    } else {
                        advance_ready(e, to);
                        preload_whole(e);
                    }
                }
                SMessage::MediaFrame { pts_ns, width: 0, levels, .. } => {
                    // an audio-only session's meter frame: the newest window's peaks, and the
                    // time at its end is how far playback has got
                    if let Some(p) = levels.last() {
                        e.levels = *p;
                    }
                    e.pts_ns = *pts_ns + levels.len() as i64 * 50_000_000;
                    if e.state == State::Loading {
                        e.state = State::Paused;
                    }
                    if e.mse.is_some() {
                        apply_mse_ready(e);
                    } else {
                        advance_ready(e, HAVE_ENOUGH_DATA);
                        preload_whole(e);
                    }
                    time_moved(e);
                }
                SMessage::MediaFrame { pts_ns, levels, .. } => {
                    if let Some(p) = levels.last() {
                        e.levels = *p;
                    }
                    let Some(f) = frame.clone() else { continue };
                    if e.kind == Kind::Video && e.natural.is_none() {
                        e.natural = Some((f.width(), f.height()));
                        fx.relayout = true;
                    }
                    e.frame = Some(f);
                    e.pts_ns = *pts_ns;
                    e.presented += 1;
                    if e.state == State::Loading {
                        e.state = State::Paused;
                    }
                    if e.mse.is_some() {
                        apply_mse_ready(e);
                    } else {
                        advance_ready(e, HAVE_ENOUGH_DATA);
                        preload_whole(e);
                    }
                    time_moved(e);
                }
                SMessage::MediaEnded { .. } => {
                    if looping(&e.node) && e.state == State::Playing {
                        // HTML §4.8.11.8 "reaches the end" with `loop`: seek to the start, play on
                        loops.push(e.url.clone());
                        e.seeking = true;
                        e.pts_ns = 0;
                        fire(e, "seeking");
                    } else {
                        let was_playing = e.state == State::Playing;
                        e.state = State::Ended;
                        e.playing_owed = false;
                        e.last_timeupdate_ns = e.pts_ns;
                        fire(e, "timeupdate");
                        if was_playing {
                            fire(e, "pause");
                        }
                        fire(e, "ended");
                    }
                }
                SMessage::MediaError { error, .. } => {
                    e.state = State::Error(error.clone());
                    // HTML §4.8.11.5: before metadata the source is "not supported" (code 4,
                    // networkState NO_SOURCE); after it, a decode error (code 3, IDLE)
                    if e.ready_state == HAVE_NOTHING {
                        e.error_code = 4;
                        e.network_state = NETWORK_NO_SOURCE;
                    } else {
                        e.error_code = 3;
                        e.network_state = NETWORK_IDLE;
                    }
                    e.playing_owed = false;
                    e.seeking = false;
                    fire(e, "error");
                }
                _ => {}
            }
            fx.repaint.push(e.node.clone());
        }
        loops.dedup();
        let title = r.title.clone();
        for url in loops {
            let mime = r.elements.iter().find(|e| e.url == url).map(|e| e.mime.clone()).unwrap_or_default();
            r.outbox.push(SMessage::MediaSeek { url: url.clone(), position_ns: 0 });
            r.outbox.push(SMessage::PlayMedia { url, title: title.clone(), mime });
        }
        fx
    })
}

/// AETHERMEDIA2 (SR134): `preload=auto` — the whole resource is local (Stria opened the file, or the
/// cache fetch finished): `progress`, then the fetch suspends (`suspend`, networkState IDLE), once.
fn preload_whole(e: &mut Element) {
    if e.preload_auto && e.ready_state >= HAVE_ENOUGH_DATA && !e.events.contains(&"progress") {
        fire(e, "progress");
        e.network_state = NETWORK_IDLE;
        fire(e, "suspend");
    }
}

/// The picture or meter moved the clock: a pending seek lands (`timeupdate`, `seeked`), and
/// during playback `timeupdate` fires every [`TIMEUPDATE_NS`] of media time.
fn time_moved(e: &mut Element) {
    // AETHERMEDIA2 (SR134): text tracks' "time marches on" (cuechange / enter / exit)
    tracks::time_marches_on(&e.node, e.pts_ns);
    if e.seeking {
        e.seeking = false;
        e.last_timeupdate_ns = e.pts_ns;
        fire(e, "timeupdate");
        fire(e, "seeked");
        return;
    }
    if e.state == State::Playing && (e.pts_ns - e.last_timeupdate_ns).abs() >= TIMEUPDATE_NS {
        e.last_timeupdate_ns = e.pts_ns;
        fire(e, "timeupdate");
    }
}

/// The media element `node` is, or is inside.
pub fn element_at(node: &NodeRef) -> Option<NodeRef> {
    let mut cur = Some(node.clone());
    while let Some(n) = cur {
        if let Some(el) = n.as_element() {
            if matches!(el.name.local.as_ref(), "video" | "audio") {
                return Some(n);
            }
        }
        cur = n.parent();
    }
    None
}

/// Start (or resume, or restart after the end) playback of `node`'s element.
pub fn play(node: &NodeRef) -> bool {
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let Some(i) = find(&r, node) else { return false };
        let (url, mime, state) = (r.elements[i].url.clone(), r.elements[i].mime.clone(), r.elements[i].state.clone());
        if state == State::Playing {
            return false;
        }
        if r.elements[i].fetching {
            // released (as PlayMedia) when the cache file is whole; AETHERMEDIA2: a `preload=none`
            // http(s) source starts its fetch now
            let src = r.elements[i].src.clone();
            if is_remote(&src) {
                ensure_cached(&src);
            }
            let e = &mut r.elements[i];
            e.blocked = false;
            e.deferred = Some(Deferred::Play);
            e.state = State::Playing;
            e.show_poster = false;
            e.playing_owed = true;
            return true;
        }
        if state == State::Ended {
            r.outbox.push(SMessage::MediaSeek { url: url.clone(), position_ns: 0 });
        }
        let title = r.title.clone();
        r.outbox.push(SMessage::PlayMedia { url: url.clone(), title, mime });
        if state == State::Idle {
            let mut out = Vec::new();
            settings_requests(&mut out, &r.elements[i]);
            r.outbox.extend(out);
        }
        let e = &mut r.elements[i];
        e.blocked = false;
        if state == State::Ended {
            // HTML play(): an ended element seeks to the start first
            e.pts_ns = 0;
            e.seeking = true;
            fire(e, "seeking");
        }
        if state == State::Idle && e.network_state == NETWORK_IDLE && e.ready_state == HAVE_NOTHING {
            // preload=none resumes its suspended fetch
            e.network_state = NETWORK_LOADING;
        }
        e.state = State::Playing;
        e.show_poster = false;
        // `play` is the caller's (the binding fires it, [`toggle`] for a click); `playing` follows
        // the first picture after this
        e.playing_owed = true;
        true
    })
}

/// Pause `node`'s element.
pub fn pause(node: &NodeRef) -> bool {
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let Some(i) = find(&r, node) else { return false };
        if r.elements[i].state != State::Playing {
            return false;
        }
        let url = r.elements[i].url.clone();
        r.outbox.push(SMessage::MediaPause { url });
        let e = &mut r.elements[i];
        e.state = State::Paused;
        e.playing_owed = false;
        // HTML pause(): `timeupdate` then `pause` — the binding (or [`toggle`]) fires `pause` after
        e.last_timeupdate_ns = e.pts_ns;
        fire(e, "timeupdate");
        true
    })
}

/// A click on (or inside) a media element: play when not playing, else pause.
pub fn toggle(node: &NodeRef) -> bool {
    let Some(el) = element_at(node) else { return false };
    // AETHERMEDIA2 (SR134): the user's click on the element is an activation
    note_user_activation();
    let playing = MEDIA.with(|m| {
        let r = m.borrow();
        find(&r, &el).map(|i| r.elements[i].state == State::Playing)
    });
    let done = match playing {
        Some(true) => pause(&el),
        Some(false) => play(&el),
        None => false,
    };
    if done {
        // a user click is not script: this path fires `play` / `pause` itself
        let ty = if playing == Some(true) { "pause" } else { "play" };
        MEDIA.with(|m| {
            let mut r = m.borrow_mut();
            if let Some(i) = find(&r, &el) {
                fire(&mut r.elements[i], ty);
            }
        });
    }
    done
}

/// Set `node`'s element muted or not (HTMLMediaElement.muted from script): the registry's flag, and
/// `MediaMute` toward Stria when a session is open.
pub fn set_muted(node: &NodeRef, muted: bool) -> bool {
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let Some(i) = find(&r, node) else { return false };
        let open = !matches!(r.elements[i].state, State::Idle);
        r.elements[i].muted = muted;
        if open {
            let url = r.elements[i].url.clone();
            r.outbox.push(SMessage::MediaMute { url, muted });
        }
        true
    })
}

/// Set `node`'s element `volume` (HTMLMediaElement.volume). `Err` when outside 0..=1 (the binding
/// throws IndexSizeError). Fires `volumechange`; an open session gets `MediaVolume` (Stria scales
/// the samples; mute stays separate and wins), an unopened one gets it with its opening request.
pub fn set_volume(node: &NodeRef, volume: f64) -> Result<bool, ()> {
    if !(0.0..=1.0).contains(&volume) {
        return Err(());
    }
    Ok(MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let Some(i) = find(&r, node) else { return false };
        if r.elements[i].volume == volume {
            return true;
        }
        r.elements[i].volume = volume;
        if !matches!(r.elements[i].state, State::Idle) {
            let url = r.elements[i].url.clone();
            r.outbox.push(SMessage::MediaVolume { url, volume: volume as f32 });
        }
        fire(&mut r.elements[i], "volumechange");
        true
    }))
}

/// Set `node`'s element `playbackRate`. Fires `ratechange`; an open session gets `MediaRate`
/// (Stria: 0 holds the clock with the element still playing, as HTML says; another rate scales the
/// clock — see Stria's `Player::set_rate` for what it refuses).
pub fn set_playback_rate(node: &NodeRef, rate: f64) -> bool {
    if !rate.is_finite() {
        return false;
    }
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let Some(i) = find(&r, node) else { return false };
        let old = r.elements[i].rate;
        if old == rate {
            return true;
        }
        r.elements[i].rate = rate;
        if !matches!(r.elements[i].state, State::Idle) {
            let url = r.elements[i].url.clone();
            r.outbox.push(SMessage::MediaRate { url, rate });
        }
        fire(&mut r.elements[i], "ratechange");
        true
    })
}

fn with_element<T>(node: &NodeRef, f: impl FnOnce(&Element) -> T) -> Option<T> {
    MEDIA.with(|m| {
        let r = m.borrow();
        find(&r, node).map(|i| f(&r.elements[i]))
    })
}

/// HTMLMediaElement `readyState` of `node` (HAVE_NOTHING for an element Aether has not registered).
pub fn ready_state(node: &NodeRef) -> u8 {
    with_element(node, |e| e.ready_state).unwrap_or(HAVE_NOTHING)
}

/// HTMLMediaElement `networkState` of `node`: the registered element's, NO_SOURCE when every
/// `<source>` was refused, else EMPTY.
pub fn network_state(node: &NodeRef) -> u8 {
    if let Some(n) = with_element(node, |e| e.network_state) {
        return n;
    }
    if MEDIA.with(|m| m.borrow().refused.contains(node)) { NETWORK_NO_SOURCE } else { NETWORK_EMPTY }
}

/// HTMLMediaElement `currentTime` in seconds (the seek target while seeking).
pub fn current_time(node: &NodeRef) -> f64 {
    with_element(node, |e| e.pts_ns.max(0) as f64 / 1e9).unwrap_or(0.0)
}

/// HTMLMediaElement `seeking`.
pub fn seeking(node: &NodeRef) -> bool {
    with_element(node, |e| e.seeking).unwrap_or(false)
}

/// `MediaError.code` of `node` (0: `error` is null).
pub fn error_code(node: &NodeRef) -> u16 {
    with_element(node, |e| e.error_code).unwrap_or(0)
}

/// HTMLMediaElement `playbackRate` (1 for an unregistered element).
pub fn playback_rate(node: &NodeRef) -> f64 {
    with_element(node, |e| e.rate).unwrap_or(1.0)
}

/// HTMLMediaElement `volume` (1 for an unregistered element).
pub fn volume(node: &NodeRef) -> f64 {
    with_element(node, |e| e.volume).unwrap_or(1.0)
}

/// The media events fired at `node`'s element so far, in order.
pub fn events(node: &NodeRef) -> Vec<&'static str> {
    with_element(node, |e| e.events.clone()).unwrap_or_default()
}

/// Seek `node`'s element to `position_ns` (clears the show-poster flag, as HTML's seek does).
pub fn seek(node: &NodeRef, position_ns: u64) -> bool {
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let Some(i) = find(&r, node) else { return false };
        let url = r.elements[i].url.clone();
        r.outbox.push(SMessage::MediaSeek { url, position_ns });
        let e = &mut r.elements[i];
        e.show_poster = false;
        if e.state == State::Ended {
            e.state = State::Paused;
        }
        // HTML §4.8.11.9: `seeking` is true and `currentTime` reads the new position at once;
        // `timeupdate` + `seeked` when the frame at the target arrives ([`time_moved`])
        let dur = e.duration_ns;
        e.pts_ns = (if dur > 0 { position_ns.min(dur) } else { position_ns }) as i64;
        e.seeking = true;
        fire(e, "seeking");
        if e.mse.is_some() {
            // AETHERMEDIA2: an unbuffered target drops readyState (`waiting` while playing)
            apply_mse_ready(e);
        }
        tracks::time_marches_on(&e.node, e.pts_ns);
        true
    })
}

/// Drain the requests queued for Stria — first releasing the elements whose http(s) source
/// has finished fetching (their held-back opening request, or a `MediaError` for the key when
/// the fetch failed: it comes back over the bus and paints in the box).
pub fn take_outbox() -> Vec<SMessage> {
    tracks::poll(); // AETHERMEDIA2 (SR134): http(s) WebVTT that finished fetching
    // FETCHED is process-wide while the elements are this thread's page: take only the results this
    // page is waiting for, so another engine (another test thread) draining its own outbox cannot
    // swallow them (the AETHERFONT join found that race in the parallel test run).
    let mine: HashSet<String> =
        MEDIA.with(|m| m.borrow().elements.iter().filter(|e| e.fetching).map(|e| e.src.clone()).collect());
    let done: Vec<(String, Result<(), String>)> = {
        let mut f = FETCHED.lock().unwrap();
        let (take, keep): (Vec<_>, Vec<_>) = f.drain(..).partition(|(src, _)| mine.contains(src));
        *f = keep;
        take
    };
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let title = r.title.clone();
        let mut out = Vec::new();
        for (src, res) in done {
            let mut errored: HashSet<String> = HashSet::new();
            for e in r.elements.iter_mut().filter(|e| e.fetching && e.src == src) {
                e.fetching = false;
                match &res {
                    Ok(()) => {
                        if let Some(what) = e.deferred.take() {
                            open_requests(&mut out, e, what, &title);
                        }
                    }
                    Err(text) => {
                        e.deferred = None;
                        if errored.insert(e.url.clone()) {
                            out.push(SMessage::MediaError { url: e.url.clone(), error: text.clone() });
                        }
                    }
                }
            }
        }
        r.outbox.extend(out);
        std::mem::take(&mut r.outbox)
    })
}

/// Remove and return the first staged `PlayMedia` (the click passthrough's old shape).
pub fn take_play_request() -> Option<(String, String, String)> {
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let i = r.outbox.iter().position(|q| matches!(q, SMessage::PlayMedia { .. }))?;
        match r.outbox.remove(i) {
            SMessage::PlayMedia { url, title, mime } => Some((url, title, mime)),
            _ => None,
        }
    })
}

/// A copy of every registered element (tests, tools, the shell's status).
pub fn snapshot() -> Vec<Element> {
    MEDIA.with(|m| m.borrow().elements.clone())
}

/// Layout's question: the element's intrinsic size, HTML §4.8.12 / Chromium `LayoutVideo`:
/// the video's natural size once known, else the poster's, else 300×150. An `<audio controls>`
/// is Chromium's 300×54 control box. `None` for nodes that are not media elements.
pub fn intrinsic_size(node: &NodeRef) -> Option<(f32, f32)> {
    let el = node.as_element()?;
    let tag = el.name.local.as_ref();
    if tag != "video" && tag != "audio" {
        return None;
    }
    if tag == "audio" {
        return Some((300.0, 54.0));
    }
    let known = MEDIA.with(|m| {
        let r = m.borrow();
        find(&r, node).map(|i| (r.elements[i].natural, r.elements[i].poster.clone()))
    });
    let (natural, poster) = match known {
        Some(k) => k,
        None => (None, el.attributes.borrow().get("poster").map(str::to_string)),
    };
    if let Some((w, h)) = natural {
        return Some((w as f32, h as f32));
    }
    if let Some(img) = poster.as_deref().and_then(crate::images::get) {
        return Some((img.width() as f32, img.height() as f32));
    }
    Some((300.0, 150.0))
}

/// What the renderer paints for one media element box.
pub struct Paint {
    pub kind: Kind,
    /// The picture: the poster image while the show-poster flag is set and a poster decodes,
    /// else the frame on glass.
    pub picture: Option<Rc<image::RgbaImage>>,
    pub error: Option<String>,
    pub controls: bool,
    pub playing: bool,
    pub pts_ns: i64,
    pub duration_ns: u64,
    /// The latest audio meter (peak L/R).
    pub levels: [f32; 2],
    /// AETHERMEDIA2 (SR134): the showing text tracks' active cues (the UA caption box's lines).
    pub captions: Vec<String>,
    /// AETHERMEDIA2: autoplay / `play()` was refused for want of activation — paint the play affordance.
    pub blocked: bool,
}

/// The renderer's question for a `<video>`/`<audio>` node.
pub fn paint_for(node: &NodeRef) -> Option<Paint> {
    let el = node.as_element()?;
    let tag = el.name.local.as_ref();
    if tag != "video" && tag != "audio" {
        return None;
    }
    let e = MEDIA.with(|m| {
        let r = m.borrow();
        find(&r, node).map(|i| r.elements[i].clone())
    });
    let Some(e) = e else {
        // An element with no playable source: nothing to paint but its controls.
        return Some(Paint {
            kind: if tag == "video" { Kind::Video } else { Kind::Audio },
            picture: None,
            error: None,
            controls: el.attributes.borrow().get("controls").is_some(),
            playing: false,
            pts_ns: 0,
            duration_ns: 0,
            levels: [0.0; 2],
            captions: Vec::new(),
            blocked: false,
        });
    };
    let poster = if e.show_poster { e.poster.as_deref().and_then(crate::images::get) } else { None };
    Some(Paint {
        kind: e.kind,
        picture: if e.kind == Kind::Video { poster.or(e.frame.clone()) } else { None },
        error: match &e.state {
            State::Error(t) => Some(t.clone()),
            _ => None,
        },
        controls: e.controls,
        playing: e.state == State::Playing,
        pts_ns: e.pts_ns,
        duration_ns: e.duration_ns,
        levels: e.levels,
        captions: if e.kind == Kind::Video { tracks::captions(&e.node) } else { Vec::new() },
        blocked: e.blocked && e.state != State::Playing,
    })
}

/// CSS `object-fit` (CSS Images 3 §5.5) — the destination rectangle (x, y, w, h), relative to
/// the content box, of a `src_w × src_h` picture in a `box_w × box_h` content box.
/// `fit`: 0 fill, 1 contain (the `<video>` default: letterboxed), 2 cover, 3 none, 4 scale-down.
/// `pos`: `object-position` as fractions of the free space (0.5, 0.5 = centred, the default).
pub fn object_fit_rect(fit: u8, pos: (f32, f32), box_w: f32, box_h: f32, src_w: f32, src_h: f32) -> (f32, f32, f32, f32) {
    if src_w <= 0.0 || src_h <= 0.0 || box_w <= 0.0 || box_h <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let contain = (box_w / src_w).min(box_h / src_h);
    let (w, h) = match fit {
        0 => (box_w, box_h),
        2 => {
            let s = (box_w / src_w).max(box_h / src_h);
            (src_w * s, src_h * s)
        }
        3 => (src_w, src_h),
        4 => {
            let s = contain.min(1.0);
            (src_w * s, src_h * s)
        }
        _ => (src_w * contain, src_h * contain),
    };
    ((box_w - w) * pos.0, (box_h - h) * pos.1, w, h)
}

/// `object-fit` keyword → the code [`object_fit_rect`] takes.
pub fn parse_object_fit(v: &str) -> Option<u8> {
    match v.trim().to_ascii_lowercase().as_str() {
        "fill" => Some(0),
        "contain" => Some(1),
        "cover" => Some(2),
        "none" => Some(3),
        "scale-down" => Some(4),
        _ => None,
    }
}

/// `object-position` → fractions of the free space per axis. Keywords and percentages; a
/// length is not resolvable without the box, so it is ignored (centred).
pub fn parse_object_position(v: &str) -> (f32, f32) {
    let mut x = None;
    let mut y = None;
    for tok in v.split_whitespace() {
        let t = tok.to_ascii_lowercase();
        let f = match t.as_str() {
            "left" | "top" => Some(0.0),
            "center" => Some(0.5),
            "right" | "bottom" => Some(1.0),
            p if p.ends_with('%') => p.trim_end_matches('%').parse::<f32>().ok().map(|n| n / 100.0),
            _ => None,
        };
        let Some(f) = f else { continue };
        match t.as_str() {
            "top" | "bottom" => y = Some(f),
            "left" | "right" => x = Some(f),
            _ => {
                if x.is_none() {
                    x = Some(f);
                } else {
                    y = Some(f);
                }
            }
        }
    }
    (x.unwrap_or(0.5), y.unwrap_or(0.5))
}

/// "m:ss" for the controls strip.
pub fn clock_text(ns: i64) -> String {
    let s = (ns.max(0) / 1_000_000_000) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

pub mod mse;
pub mod tracks;

#[cfg(test)]
mod tests;
