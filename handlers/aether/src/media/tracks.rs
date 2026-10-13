//! AETHERMEDIA2 (LEDGER SR134): text tracks — `<track kind=subtitles|captions|descriptions|chapters|
//! metadata>`, `addTextTrack()`, and their cues (HTML §4.8.11.11), with WebVTT parsed by
//! `demux_core::vtt` (no_std, shared-core).
//!
//! CHARTER: Aether — binding. The model is per media element: its tracks in list order (`<track>`
//! children first, in tree order, then script-added ones), each with a mode (disabled / hidden /
//! showing), a readiness (NONE / LOADING / LOADED / ERROR), its cues in (start, end) order and the
//! active set. HTML §4.8.11.11.5 automatic selection: the first `subtitles`/`captions` track with
//! `default` is showing, `chapters`/`metadata` with `default` are hidden, the rest disabled. A track
//! loads when its mode is not disabled (`file:` read at once; `http(s)` fetched off-thread into the
//! media cache); `load` / `error` fire at the `<track>`.
//!
//! "Time marches on" (§4.8.11.8) runs whenever the element's clock moves (each picture or meter
//! frame from Stria, each seek): the active cues are recomputed for every non-disabled track and,
//! when a track's set changed, `cuechange` is queued at its TextTrack object and at its `<track>`;
//! `enter` / `exit` at the cues that became active / inactive. The showing tracks' active cues are
//! what render/media_overlay.rs paints as the UA's default caption box.

use std::cell::RefCell;
use std::sync::Mutex;

use crate::dom::NodeRef;
use crate::js::TargetKey;
use gneiss_pal::dsp::demux::vtt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Disabled,
    Hidden,
    Showing,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Disabled => "disabled",
            Mode::Hidden => "hidden",
            Mode::Showing => "showing",
        }
    }
}

/// HTMLTrackElement readyState values.
pub const NONE: u8 = 0;
pub const LOADING: u8 = 1;
pub const LOADED: u8 = 2;
pub const ERROR: u8 = 3;

#[derive(Clone, Debug)]
pub struct Cue {
    /// Unique over the page (the JS VTTCue's handle).
    pub key: u32,
    pub id: String,
    pub start_ns: i64,
    pub end_ns: i64,
    pub text: String,
    pub settings: String,
    pub pause_on_exit: bool,
    /// The VTTCue object's EventTarget serial, once script has seen the cue.
    pub target: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub key: u32,
    pub media: NodeRef,
    /// The `<track>` element (None for `addTextTrack()`).
    pub element: Option<NodeRef>,
    pub kind: String,
    pub label: String,
    pub language: String,
    pub id: String,
    pub mode: Mode,
    pub ready: u8,
    pub src: Option<String>,
    pub cues: Vec<Cue>,
    pub active: Vec<u32>,
    /// The TextTrack object's EventTarget serial, once script has seen the track.
    pub target: Option<u32>,
}

#[derive(Default)]
struct Registry {
    tracks: Vec<Track>,
    next: u32,
}

thread_local! {
    static TRACKS: RefCell<Registry> = RefCell::new(Registry::default());
}

/// http(s) track fetches finished off-thread: (src, text or error).
static FETCHED: Mutex<Vec<(String, Result<String, String>)>> = Mutex::new(Vec::new());

fn next_key(r: &mut Registry) -> u32 {
    r.next += 1;
    r.next
}

fn queue_node(node: &NodeRef, ty: &str) {
    super::queue_target_event(TargetKey::Node(node.id().0), ty);
}

fn queue_other(target: u32, ty: &str) {
    super::queue_target_event(TargetKey::Other(target), ty);
}

/// Forget the page's tracks (navigation).
pub fn reset() {
    TRACKS.with(|t| *t.borrow_mut() = Registry::default());
}

/// HTML's `kind` keyword (invalid and missing → `subtitles`; metadata is the invalid value default
/// per the reflecting IDL, but the processing model treats a missing value as subtitles).
fn kind_of(v: Option<&str>) -> String {
    let k = v.unwrap_or("").trim().to_ascii_lowercase();
    match k.as_str() {
        "subtitles" | "captions" | "descriptions" | "chapters" | "metadata" => k,
        "" => "subtitles".into(),
        _ => "metadata".into(),
    }
}

/// Register the `<track>` children of `media` not yet known (scan time and script access).
pub fn scan(media: &NodeRef, base: &str) {
    let kids: Vec<NodeRef> = media.children().filter(|c| c.as_element().is_some_and(|e| e.name.local.as_ref() == "track")).collect();
    let mut to_load = Vec::new();
    TRACKS.with(|t| {
        let mut r = t.borrow_mut();
        let mut showing_taken = r.tracks.iter().any(|x| &x.media == media && x.mode == Mode::Showing && matches!(x.kind.as_str(), "subtitles" | "captions"));
        for k in kids {
            if r.tracks.iter().any(|x| x.element.as_ref() == Some(&k)) {
                continue;
            }
            let el = k.as_element().unwrap();
            let a = el.attributes.borrow();
            let kind = kind_of(a.get("kind"));
            let default = a.get("default").is_some();
            let mode = if default && matches!(kind.as_str(), "subtitles" | "captions") && !showing_taken {
                showing_taken = true;
                Mode::Showing
            } else if default && matches!(kind.as_str(), "chapters" | "metadata") {
                Mode::Hidden
            } else {
                Mode::Disabled
            };
            let src = a.get("src").filter(|s| !s.trim().is_empty()).map(|s| crate::images::resolve(base, s.trim()));
            let key = next_key(&mut r);
            r.tracks.push(Track {
                key,
                media: media.clone(),
                element: Some(k.clone()),
                kind,
                label: a.get("label").unwrap_or("").to_string(),
                language: a.get("srclang").unwrap_or("").to_string(),
                id: a.get("id").unwrap_or("").to_string(),
                mode,
                ready: NONE,
                src,
                cues: Vec::new(),
                active: Vec::new(),
                target: None,
            });
            if mode != Mode::Disabled {
                to_load.push(key);
            }
        }
    });
    for k in to_load {
        load(k);
    }
}

/// Start loading a `<track>`'s WebVTT (once): `file:` now, `http(s)` off-thread.
fn load(key: u32) {
    let job = TRACKS.with(|t| {
        let mut r = t.borrow_mut();
        let tr = r.tracks.iter_mut().find(|x| x.key == key)?;
        if tr.ready != NONE || tr.element.is_none() {
            return None;
        }
        tr.ready = LOADING;
        Some(tr.src.clone())
    });
    let Some(src) = job else { return };
    let Some(src) = src else {
        finish(key, Err("no src".into()));
        return;
    };
    if let Some(path) = src.strip_prefix("file://") {
        finish(key, std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| e.to_string()));
    } else if super::is_remote(&src) {
        let s = src.clone();
        let dest = super::cache_path(&s);
        std::thread::Builder::new()
            .name("aether-track-fetch".into())
            .spawn(move || {
                let r = super::fetch_to_cache(&s, &dest, super::FETCH_CHUNK)
                    .and_then(|_| std::fs::read(&dest).map_err(|e| e.to_string()))
                    .map(|b| String::from_utf8_lossy(&b).into_owned());
                FETCHED.lock().unwrap().push((s, r));
            })
            .ok();
    } else {
        finish(key, Err(format!("{src}: not a file or http(s) URL")));
    }
}

/// A track's text arrived (or failed): parse, LOADED + `load`, else ERROR + `error`.
fn finish(key: u32, text: Result<String, String>) {
    let node = TRACKS.with(|t| {
        let mut r = t.borrow_mut();
        let mut keys: Vec<u32> = Vec::new();
        let base = r.next;
        let tr = r.tracks.iter_mut().find(|x| x.key == key)?;
        let parsed = text.ok().and_then(|s| vtt::parse(&s).ok());
        match parsed {
            Some(cues) => {
                for (i, c) in cues.into_iter().enumerate() {
                    let k = base + 1 + i as u32;
                    keys.push(k);
                    tr.cues.push(Cue { key: k, id: c.id, start_ns: c.start_ns, end_ns: c.end_ns, text: c.text, settings: c.settings, pause_on_exit: false, target: None });
                }
                tr.cues.sort_by_key(|c| (c.start_ns, std::cmp::Reverse(c.end_ns)));
                tr.ready = LOADED;
            }
            None => tr.ready = ERROR,
        }
        let ok = tr.ready == LOADED;
        let el = tr.element.clone();
        r.next = base + keys.len() as u32;
        el.map(|e| (e, ok))
    });
    if let Some((el, ok)) = node {
        if !ok {
            crate::ledger::record_dom("track-webvtt-failed");
        }
        queue_node(&el, if ok { "load" } else { "error" });
    }
}

/// Install the http(s) tracks whose fetch finished (called from `media::take_outbox`).
pub fn poll() {
    let done: Vec<(String, Result<String, String>)> = std::mem::take(&mut *FETCHED.lock().unwrap());
    for (src, res) in done {
        let keys: Vec<u32> = TRACKS.with(|t| t.borrow().tracks.iter().filter(|x| x.ready == LOADING && x.src.as_deref() == Some(src.as_str())).map(|x| x.key).collect());
        for k in keys {
            finish(k, res.clone());
        }
    }
}

/// "Time marches on" for `media` at `now_ns`.
pub fn time_marches_on(media: &NodeRef, now_ns: i64) {
    let mut events: Vec<(Option<u32>, Option<NodeRef>, Vec<(Option<u32>, &'static str)>)> = Vec::new();
    TRACKS.with(|t| {
        let mut r = t.borrow_mut();
        for tr in r.tracks.iter_mut().filter(|x| &x.media == media && x.mode != Mode::Disabled) {
            let now: Vec<u32> = tr.cues.iter().filter(|c| c.start_ns <= now_ns && now_ns < c.end_ns).map(|c| c.key).collect();
            if now == tr.active {
                continue;
            }
            let mut cue_events = Vec::new();
            for c in &tr.cues {
                let was = tr.active.contains(&c.key);
                let is = now.contains(&c.key);
                if was && !is {
                    cue_events.push((c.target, "exit"));
                } else if is && !was {
                    cue_events.push((c.target, "enter"));
                }
            }
            tr.active = now;
            events.push((tr.target, tr.element.clone(), cue_events));
        }
    });
    for (target, el, cues) in events {
        for (t, ty) in cues {
            if let Some(t) = t {
                queue_other(t, ty);
            }
        }
        if let Some(t) = target {
            queue_other(t, "cuechange");
        }
        if let Some(el) = el {
            queue_node(&el, "cuechange");
        }
    }
}

/// The caption box's lines for `media`: the active cues of its showing subtitles / captions tracks,
/// in cue order, as plain text.
pub fn captions(media: &NodeRef) -> Vec<String> {
    TRACKS.with(|t| {
        let r = t.borrow();
        let mut out = Vec::new();
        for tr in r.tracks.iter().filter(|x| &x.media == media && x.mode == Mode::Showing && matches!(x.kind.as_str(), "subtitles" | "captions")) {
            for c in tr.cues.iter().filter(|c| tr.active.contains(&c.key)) {
                let s = vtt::plain_text(&c.text);
                if !s.trim().is_empty() {
                    out.push(s);
                }
            }
        }
        out
    })
}

// ------------------------------------------------------------------ the script face (js/tracks.rs)

/// The track keys of `media`, in list order (`<track>` children are registered first).
pub fn list(media: &NodeRef) -> Vec<u32> {
    TRACKS.with(|t| t.borrow().tracks.iter().filter(|x| &x.media == media).map(|x| x.key).collect())
}

/// A copy of one track.
pub fn get(key: u32) -> Option<Track> {
    TRACKS.with(|t| t.borrow().tracks.iter().find(|x| x.key == key).cloned())
}

/// The track of a `<track>` element.
pub fn for_element(el: &NodeRef) -> Option<u32> {
    TRACKS.with(|t| t.borrow().tracks.iter().find(|x| x.element.as_ref() == Some(el)).map(|x| x.key))
}

/// Remember the TextTrack object's event target.
pub fn bind(key: u32, target: u32) {
    TRACKS.with(|t| {
        if let Some(x) = t.borrow_mut().tracks.iter_mut().find(|x| x.key == key) {
            x.target = Some(target);
        }
    });
}

/// Remember a cue object's event target.
pub fn bind_cue(cue: u32, target: u32) {
    TRACKS.with(|t| {
        for tr in t.borrow_mut().tracks.iter_mut() {
            if let Some(c) = tr.cues.iter_mut().find(|c| c.key == cue) {
                c.target = Some(target);
            }
        }
    });
}

/// `mode` setter: a track turned on loads; the active set is recomputed at the element's time.
pub fn set_mode(key: u32, mode: Mode) {
    let media = TRACKS.with(|t| {
        let mut r = t.borrow_mut();
        let tr = r.tracks.iter_mut().find(|x| x.key == key)?;
        if tr.mode == mode {
            return None;
        }
        tr.mode = mode;
        if mode == Mode::Disabled {
            tr.active.clear();
        }
        Some(tr.media.clone())
    });
    if let Some(m) = media {
        if mode != Mode::Disabled {
            load(key);
        }
        let now = (super::current_time(&m) * 1e9) as i64;
        time_marches_on(&m, now);
    }
}

/// `addTextTrack(kind, label, language)`: mode hidden, readiness LOADED (HTML §4.8.11.11.3).
pub fn add(media: &NodeRef, kind: &str, label: &str, language: &str) -> u32 {
    TRACKS.with(|t| {
        let mut r = t.borrow_mut();
        let key = next_key(&mut r);
        r.tracks.push(Track {
            key,
            media: media.clone(),
            element: None,
            kind: kind.to_string(),
            label: label.to_string(),
            language: language.to_string(),
            id: String::new(),
            mode: Mode::Hidden,
            ready: LOADED,
            src: None,
            cues: Vec::new(),
            active: Vec::new(),
            target: None,
        });
        key
    })
}

/// `addCue(cue)`: the cue joins `key` (moving out of any other track first). Returns its handle.
pub fn add_cue(key: u32, cue_key: Option<u32>, id: &str, start: f64, end: f64, text: &str, pause_on_exit: bool, target: Option<u32>) -> u32 {
    TRACKS.with(|t| {
        let mut r = t.borrow_mut();
        let ck = match cue_key {
            Some(k) => k,
            None => next_key(&mut r),
        };
        for tr in r.tracks.iter_mut() {
            tr.cues.retain(|c| c.key != ck);
            tr.active.retain(|c| *c != ck);
        }
        if let Some(tr) = r.tracks.iter_mut().find(|x| x.key == key) {
            tr.cues.push(Cue { key: ck, id: id.to_string(), start_ns: (start * 1e9) as i64, end_ns: (end * 1e9) as i64, text: text.to_string(), settings: String::new(), pause_on_exit, target });
            tr.cues.sort_by_key(|c| (c.start_ns, std::cmp::Reverse(c.end_ns)));
        }
        ck
    })
}

/// `removeCue(cue)`: false when the cue is not in `key` (NotFoundError).
pub fn remove_cue(key: u32, cue: u32) -> bool {
    TRACKS.with(|t| {
        let mut r = t.borrow_mut();
        let Some(tr) = r.tracks.iter_mut().find(|x| x.key == key) else { return false };
        let n = tr.cues.len();
        tr.cues.retain(|c| c.key != cue);
        tr.active.retain(|c| *c != cue);
        tr.cues.len() != n
    })
}

/// A cue's field changed from script (`id`, `startTime`, `endTime`, `text`, `pauseOnExit`).
pub fn set_cue(cue: u32, field: &str, num: f64, text: &str) {
    TRACKS.with(|t| {
        for tr in t.borrow_mut().tracks.iter_mut() {
            if let Some(c) = tr.cues.iter_mut().find(|c| c.key == cue) {
                match field {
                    "id" => c.id = text.to_string(),
                    "startTime" => c.start_ns = (num * 1e9) as i64,
                    "endTime" => c.end_ns = (num * 1e9) as i64,
                    "text" => c.text = text.to_string(),
                    "pauseOnExit" => c.pause_on_exit = num != 0.0,
                    _ => {}
                }
                tr.cues.sort_by_key(|c| (c.start_ns, std::cmp::Reverse(c.end_ns)));
                return;
            }
        }
    });
}
