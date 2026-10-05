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
    if e.muted {
        out.push(SMessage::MediaMute { url: e.url.clone(), muted: true });
    }
}

/// Media types this engine hands to Stria. Containers: what `demux_core` parses (MP4, WebM,
/// Matroska) and the bare audio files `dsp::audio` decodes (MP3, Ogg, WAV, FLAC, ADTS, AIFF).
/// Codecs: anything Stria opens — real decoders (`utp1` test pattern, PCM, and since
/// AUDIOTRACK Opus, Vorbis, AAC-LC, MP3 (`mp3`, `mp4a.6B`, WAV's `1`), FLAC) and the video codecs
/// it plays with the labelled stand-in until AVCODEC lands (AV1, VP8, VP9, H.264, HEVC). An
/// unknown codec makes the `<source>` unplayable, so the next one is tried — HTML's
/// `canPlayType` == "" rule.
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
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let mut stops: Vec<String> = Vec::new();
        for e in &r.elements {
            if e.state != State::Idle && !stops.contains(&e.url) {
                stops.push(e.url.clone());
            }
        }
        r.elements.clear();
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
    MEDIA.with(|m| {
        let mut r = m.borrow_mut();
        let mut any = false;
        for node in nodes {
            if find(&r, &node).is_some() {
                continue;
            }
            let Some((src, mime)) = source_for(&node, base) else { continue };
            // http(s): Stria plays the cache file; the fetch may still be running
            let (url, fetching) = if is_remote(&src) { (format!("file://{}", cache_path(&src).display()), ensure_cached(&src)) } else { (src.clone(), false) };
            let el = node.as_element().unwrap();
            let kind = if el.name.local.as_ref() == "video" { Kind::Video } else { Kind::Audio };
            let attrs = el.attributes.borrow();
            let poster = if kind == Kind::Video {
                attrs.get("poster").filter(|p| !p.trim().is_empty()).map(|p| crate::images::resolve(base, p.trim()))
            } else {
                None
            };
            let autoplay = attrs.get("autoplay").is_some();
            let muted = attrs.get("muted").is_some();
            let controls = attrs.get("controls").is_some();
            let preload_none = attrs.get("preload").is_some_and(|p| p.trim().eq_ignore_ascii_case("none"));
            drop(attrs);
            // Chromium's autoplay policy: muted autoplay always runs; unmuted autoplay waits
            // for a user gesture (this engine has no media-engagement index, so it never runs).
            let autoplays = autoplay && muted;
            if autoplay && !muted {
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
            };
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
        for e in r.elements.iter_mut().filter(|e| &e.url == url) {
            fx.handled = true;
            match msg {
                SMessage::MediaOpened { duration_ns, width, height, real_video, .. } => {
                    let nat = (*width > 0 && *height > 0).then_some((*width, *height));
                    if e.kind == Kind::Video && nat != e.natural {
                        fx.relayout = true;
                    }
                    e.natural = nat;
                    e.duration_ns = *duration_ns;
                    e.real_video = *real_video;
                    if e.state == State::Loading {
                        e.state = State::Paused;
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
                    if e.state == State::Loading {
                        e.state = State::Paused;
                    }
                }
                SMessage::MediaEnded { .. } => {
                    e.state = State::Ended;
                }
                SMessage::MediaError { error, .. } => {
                    e.state = State::Error(error.clone());
                }
                _ => {}
            }
            fx.repaint.push(e.node.clone());
        }
        fx
    })
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
            // released (as PlayMedia) when the cache file is whole
            let e = &mut r.elements[i];
            e.deferred = Some(Deferred::Play);
            e.state = State::Playing;
            e.show_poster = false;
            return true;
        }
        if state == State::Ended {
            r.outbox.push(SMessage::MediaSeek { url: url.clone(), position_ns: 0 });
        }
        let title = r.title.clone();
        r.outbox.push(SMessage::PlayMedia { url: url.clone(), title, mime });
        if r.elements[i].muted && state == State::Idle {
            r.outbox.push(SMessage::MediaMute { url, muted: true });
        }
        let e = &mut r.elements[i];
        e.state = State::Playing;
        e.show_poster = false;
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
        r.elements[i].state = State::Paused;
        true
    })
}

/// A click on (or inside) a media element: play when not playing, else pause.
pub fn toggle(node: &NodeRef) -> bool {
    let Some(el) = element_at(node) else { return false };
    let playing = MEDIA.with(|m| {
        let r = m.borrow();
        find(&r, &el).map(|i| r.elements[i].state == State::Playing)
    });
    match playing {
        Some(true) => pause(&el),
        Some(false) => play(&el),
        None => false,
    }
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
        true
    })
}

/// Drain the requests queued for Stria — first releasing the elements whose http(s) source
/// has finished fetching (their held-back opening request, or a `MediaError` for the key when
/// the fetch failed: it comes back over the bus and paints in the box).
pub fn take_outbox() -> Vec<SMessage> {
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

#[cfg(test)]
mod tests;
