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
//! State lives in a thread-local registry — the same pattern as `images::STORE` — because the
//! engine, layout (intrinsic size) and renderer (frame, poster, controls) all run on the engine
//! thread and each needs the element's media state by DOM node. The shell drains requests with
//! [`take_outbox`] (`AetherEngine::take_media_requests`) and feeds Stria's replies back with
//! [`on_message`] (`AetherEngine::on_media_message`).

use bandy::SMessage;
use crate::dom::NodeRef;
use std::cell::RefCell;
use std::rc::Rc;

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
    /// The resolved source (absolute url): the bus key.
    pub url: String,
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

/// Media types this engine hands to Stria. Containers: what `demux_core` parses (MP4, WebM,
/// Matroska). Codecs: anything Stria opens — real decoders (`utp1` test pattern, PCM) and the
/// codecs it plays with the labelled stand-in until AVCODEC lands (AV1, VP8, VP9, H.264, HEVC,
/// Opus, Vorbis, AAC, FLAC). An unknown codec makes the `<source>` unplayable, so the next one
/// is tried — HTML's `canPlayType` == "" rule.
pub fn can_play_type(mime: &str) -> bool {
    let mut parts = mime.split(';');
    let base = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let container_ok = matches!(
        base.as_str(),
        "video/mp4" | "audio/mp4" | "video/webm" | "audio/webm" | "video/x-matroska" | "audio/x-matroska" | "video/quicktime"
    );
    if !container_ok {
        return false;
    }
    for p in parts {
        let p = p.trim();
        let Some(v) = p.strip_prefix("codecs=").or_else(|| p.strip_prefix("codecs =")) else { continue };
        for c in v.trim_matches(|c| c == '"' || c == '\'').split(',') {
            let c = c.trim().to_ascii_lowercase();
            let known = ["utp1", "av01", "av1", "vp8", "vp9", "vp09", "avc1", "avc3", "hvc1", "hev1", "opus", "vorbis", "mp4a", "flac", "pcm", "sowt", "ipcm"];
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
        _ => "application/octet-stream",
    }
}

fn find(r: &Registry, node: &NodeRef) -> Option<usize> {
    r.elements.iter().position(|e| &e.node == node)
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
            let Some((url, mime)) = source_for(&node, base) else { continue };
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
            drop(attrs);
            // Chromium's autoplay policy: muted autoplay always runs; unmuted autoplay waits
            // for a user gesture (this engine has no media-engagement index, so it never runs).
            let autoplays = autoplay && muted;
            if autoplay && !muted {
                crate::ledger::record_dom("media-autoplay-blocked-unmuted");
            }
            let state = if autoplays {
                let title = r.title.clone();
                r.outbox.push(SMessage::PlayMedia { url: url.clone(), title, mime: mime.clone() });
                State::Playing
            } else if kind == Kind::Video {
                // A session per url: a second element on the same stream shares the first's.
                if !r.elements.iter().any(|e| e.url == url && e.state != State::Idle) {
                    r.outbox.push(SMessage::MediaPoster { url: url.clone() });
                }
                State::Loading
            } else {
                State::Idle
            };
            r.elements.push(Element {
                node,
                kind,
                url,
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
            });
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
        SMessage::MediaFrame { width, height, rgba, .. } => {
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
                SMessage::MediaFrame { pts_ns, .. } => {
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
        if state == State::Ended {
            r.outbox.push(SMessage::MediaSeek { url: url.clone(), position_ns: 0 });
        }
        let title = r.title.clone();
        r.outbox.push(SMessage::PlayMedia { url, title, mime });
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

/// Drain the requests queued for Stria.
pub fn take_outbox() -> Vec<SMessage> {
    MEDIA.with(|m| std::mem::take(&mut m.borrow_mut().outbox))
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
