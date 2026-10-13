//! AETHERMEDIA2 (LEDGER SR134): the page side of Media Source Extensions (W3C MSE, 2024).
//!
//! CHARTER: Aether — binding. The MediaSource / SourceBuffer objects a page builds (`js/mse.rs`) keep
//! their state here; every mutation becomes one `demux_core::fragment::Op` record that is applied to
//! THIS page's [`MediaSourceModel`] (the source of `buffered`, `duration`, `readyState`) and appended
//! to the MediaSource's journal, which Stria's `MediaSourceFeed` tails and applies to the same model
//! (bus key `mediasource:<journal>`, see `handlers/stria/src/media_source.rs`). Aether parses and
//! decodes nothing itself: the segment parser and coded frame processing are demux_core's, the
//! pictures are Stria's.
//!
//! Attachment (MSE §2.4.1 "attaching to a media element"): `URL.createObjectURL(mediaSource)` gives a
//! `blob:` URL registered here; when a media element's resource selection reaches it
//! (`media::scan`), the MediaSource attaches — readyState "open", `sourceopen` queued — and the
//! element's bus key is the journal's. The element's readyState then follows the model (§3.5.7 step
//! 7: HAVE_METADATA when every SourceBuffer has its init segment; §2.4.4: HAVE_CURRENT / FUTURE /
//! ENOUGH by what `buffered` holds past the current position; a drop while playing fires `waiting`).

use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;

use crate::dom::NodeRef;
use crate::js::TargetKey;
use gneiss_pal::dsp::demux::fragment::{Applied, MediaSourceModel, Op, ReadyState};

/// One MediaSource of the page.
struct Source {
    id: u32,
    /// The MediaSource object's EventTarget serial (events are queued at it).
    target: u32,
    url: Option<String>,
    model: MediaSourceModel,
    journal: Option<PathBuf>,
    element: Option<NodeRef>,
    /// Per SourceBuffer: its EventTarget serial, `updating`, and the operation waiting for its
    /// `updatestart` task.
    sbs: Vec<Sb>,
    /// The SourceBufferList's EventTarget serial (addsourcebuffer / removesourcebuffer).
    list_target: Option<u32>,
}

struct Sb {
    target: u32,
    updating: bool,
    pending: Option<Pending>,
    removed: bool,
}

enum Pending {
    Append(Vec<u8>),
    Remove(i64, i64),
}

#[derive(Default)]
struct Registry {
    sources: Vec<Source>,
    next: u32,
}

thread_local! {
    static MSE: RefCell<Registry> = RefCell::new(Registry::default());
}

/// Queue the simple event `ty` at a non-node target (an EventTarget's serial).
fn queue(target: u32, ty: &str) {
    super::queue_target_event(TargetKey::Other(target), ty);
}

fn with<R>(id: u32, f: impl FnOnce(&mut Source) -> R) -> Option<R> {
    MSE.with(|m| m.borrow_mut().sources.iter_mut().find(|s| s.id == id).map(f))
}

/// `new MediaSource()`: readyState "closed" until attached.
pub fn create(target: u32) -> u32 {
    MSE.with(|m| {
        let mut r = m.borrow_mut();
        r.next += 1;
        let id = r.next;
        let mut model = MediaSourceModel::new();
        model.ready = ReadyState::Closed;
        r.sources.push(Source { id, target, url: None, model, journal: None, element: None, sbs: Vec::new(), list_target: None });
        id
    })
}

/// The SourceBufferList's event target.
pub fn set_list_target(id: u32, target: u32) {
    with(id, |s| s.list_target = Some(target));
}

/// `URL.createObjectURL(mediaSource)`: a `blob:` URL that names this MediaSource (one per call, as
/// Chromium mints them).
pub fn object_url(id: u32, origin: &str) -> String {
    let n = MSE.with(|m| {
        let mut r = m.borrow_mut();
        r.next += 1;
        r.next
    });
    let h = crypto_core::sha2::sha256(format!("{}:{id}:{n}", std::process::id()).as_bytes());
    let hex: String = h.iter().take(16).map(|b| format!("{b:02x}")).collect();
    let url = format!("blob:{origin}/{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]);
    with(id, |s| s.url = Some(url.clone()));
    url
}

/// `URL.revokeObjectURL`: the URL no longer attaches.
pub fn revoke(url: &str) {
    MSE.with(|m| {
        for s in m.borrow_mut().sources.iter_mut() {
            if s.url.as_deref() == Some(url) {
                s.url = None;
            }
        }
    });
}

/// The MediaSource a `blob:` URL names.
pub fn by_url(url: &str) -> Option<u32> {
    MSE.with(|m| m.borrow().sources.iter().find(|s| s.url.as_deref() == Some(url.trim())).map(|s| s.id))
}

/// The MediaSource attached to a media element.
pub fn for_element(node: &NodeRef) -> Option<u32> {
    MSE.with(|m| m.borrow().sources.iter().find(|s| s.element.as_ref() == Some(node)).map(|s| s.id))
}

/// Attach `id` to the media element `node` (MSE §2.4.1): readyState "open", `sourceopen` queued, the
/// journal created. Returns the element's bus key, or None when the MediaSource is not closed
/// (already attached elsewhere: the element's load fails).
pub fn attach(id: u32, node: &NodeRef) -> Option<String> {
    let journal = super::cache_dir().join("mse").join(format!("{}-{id}-{}.umse", std::process::id(), unique()));
    if let Some(d) = journal.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&journal, b"");
    with(id, |s| {
        if s.model.ready != ReadyState::Closed || s.element.is_some() {
            return None;
        }
        s.model.ready = ReadyState::Open;
        s.element = Some(node.clone());
        s.journal = Some(journal.clone());
        queue(s.target, "sourceopen");
        Some(format!("{}{}", MEDIASOURCE_SCHEME, journal.display()))
    })
    .flatten()
}

/// The bus key prefix Stria's feed reads (`stria::media_source::SCHEME`).
pub const MEDIASOURCE_SCHEME: &str = "mediasource:";

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Apply `op` to the page's model and write it to the journal Stria reads.
fn record(s: &mut Source, op: Op) -> Applied {
    let applied = s.model.apply(&op);
    if let Some(j) = &s.journal {
        let mut b = Vec::new();
        op.encode(&mut b);
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(j) {
            let _ = f.write_all(&b);
        }
    }
    applied
}

/// Detach every MediaSource (navigation): readyState "closed", `sourceclose` (MSE §2.4.2).
pub fn reset() {
    let gone: Vec<Source> = MSE.with(|m| std::mem::take(&mut m.borrow_mut().sources));
    for mut s in gone {
        if s.element.is_some() {
            record(&mut s, Op::Close);
            queue(s.target, "sourceclose");
        }
        if let Some(j) = s.journal {
            let _ = std::fs::remove_file(j);
        }
    }
}

/// `readyState`.
pub fn ready_state(id: u32) -> &'static str {
    with(id, |s| match s.model.ready {
        ReadyState::Closed => "closed",
        ReadyState::Open => "open",
        ReadyState::Ended => "ended",
    })
    .unwrap_or("closed")
}

/// `duration` in seconds (NaN while unset or closed, +Infinity possible).
pub fn duration(id: u32) -> f64 {
    with(id, |s| {
        if s.model.ready == ReadyState::Closed {
            return f64::NAN;
        }
        match s.model.duration_ns {
            None => f64::NAN,
            Some(i64::MAX) => f64::INFINITY,
            Some(d) => d as f64 / 1e9,
        }
    })
    .unwrap_or(f64::NAN)
}

/// Any SourceBuffer is updating (the InvalidStateError guard of duration / endOfStream).
pub fn any_updating(id: u32) -> bool {
    with(id, |s| s.sbs.iter().any(|b| !b.removed && b.updating)).unwrap_or(false)
}

fn ns(secs: f64) -> i64 {
    if secs.is_infinite() && secs > 0.0 { i64::MAX } else { (secs * 1e9).round() as i64 }
}

/// The duration setter (§2.4.6 duration change): the element hears `durationchange`.
pub fn set_duration(id: u32, secs: f64) {
    let el = with(id, |s| {
        record(s, Op::Duration { ns: ns(secs) });
        s.element.clone()
    })
    .flatten();
    if let Some(el) = el {
        super::mse_changed(&el, true);
    }
}

/// `addSourceBuffer(type)`: the SourceBuffer's index.
pub fn add_source_buffer(id: u32, mime: &str, target: u32) -> Option<u16> {
    with(id, |s| {
        record(s, Op::AddSourceBuffer { mime: mime.to_string() });
        s.sbs.push(Sb { target, updating: false, pending: None, removed: false });
        if let Some(t) = s.list_target {
            queue(t, "addsourcebuffer");
        }
        (s.sbs.len() - 1) as u16
    })
}

/// `removeSourceBuffer(sb)` (§2.4.3): an update in flight is aborted (`abort`, `updateend`).
pub fn remove_source_buffer(id: u32, sb: u16) {
    with(id, |s| {
        if let Some(b) = s.sbs.get_mut(sb as usize) {
            if b.updating {
                b.updating = false;
                b.pending = None;
                queue(b.target, "abort");
                queue(b.target, "updateend");
            }
            b.removed = true;
        }
        record(s, Op::RemoveSourceBuffer { sb });
        if let Some(t) = s.list_target {
            queue(t, "removesourcebuffer");
        }
    });
}

/// An append or remove on an "ended" MediaSource reopens it (§3.5.4 prepare append / remove step 6).
fn reopen_if_ended(s: &mut Source) {
    if s.model.ready == ReadyState::Ended {
        record(s, Op::Reopen);
        queue(s.target, "sourceopen");
    }
}

/// `appendBuffer(data)` (§3.2): updating, `updatestart` queued; the bytes are processed when that
/// task runs ([`run`], called from the SourceBuffer's own `updatestart` listener).
pub fn append(id: u32, sb: u16, bytes: Vec<u8>) {
    with(id, |s| {
        reopen_if_ended(s);
        if let Some(b) = s.sbs.get_mut(sb as usize) {
            b.updating = true;
            b.pending = Some(Pending::Append(bytes));
            queue(b.target, "updatestart");
        }
    });
}

/// `remove(start, end)` (§3.2): as [`append`], with the range removal as the operation.
pub fn remove(id: u32, sb: u16, start: f64, end: f64) {
    with(id, |s| {
        reopen_if_ended(s);
        if let Some(b) = s.sbs.get_mut(sb as usize) {
            b.updating = true;
            b.pending = Some(Pending::Remove(ns(start), ns(end)));
            queue(b.target, "updatestart");
        }
    });
}

/// The operation an `updatestart` task carries out: the buffer append algorithm (§3.5.5) or the
/// range removal (§3.5.9), then `update` + `updateend` — or, for a failed append, the append error
/// algorithm (§3.5.3): `error` + `updateend`, then endOfStream("decode").
pub fn run(id: u32, sb: u16) {
    let el = with(id, |s| {
        let Some(b) = s.sbs.get_mut(sb as usize) else { return None };
        let target = b.target;
        let Some(op) = b.pending.take() else { return None };
        let applied = match op {
            Pending::Append(bytes) => record(s, Op::Append { sb, bytes }),
            Pending::Remove(a, e) => record(s, Op::Remove { sb, start_ns: a, end_ns: e }),
        };
        if applied.error.is_some() {
            crate::ledger::record_dom("mse-append-error");
            queue(target, "error");
            queue(target, "updateend");
            record(s, Op::EndOfStream { error: 2 });
            queue(s.target, "sourceended");
        } else {
            queue(target, "update");
            queue(target, "updateend");
        }
        s.element.clone().map(|e| (e, applied.append.first_init, applied.error.is_some()))
    })
    .flatten();
    if let Some((el, first_init, failed)) = el {
        if failed {
            super::mse_decode_error(&el);
        } else {
            super::mse_changed(&el, first_init);
        }
    }
}

/// The `update` / `error` task of an operation is running: `updating` is false before the page's
/// listeners see it (§3.5.5 step 7, §3.5.3 step 3) — called from the SourceBuffer's own first listener.
pub fn done(id: u32, sb: u16) {
    with(id, |s| {
        if let Some(b) = s.sbs.get_mut(sb as usize) {
            if b.pending.is_none() {
                b.updating = false;
            }
        }
    });
}

/// `abort()` (§3.2): an update in flight ends (`abort`, `updateend`); the parser resets.
pub fn abort(id: u32, sb: u16) {
    with(id, |s| {
        if let Some(b) = s.sbs.get_mut(sb as usize) {
            if b.updating {
                b.updating = false;
                b.pending = None;
                queue(b.target, "abort");
                queue(b.target, "updateend");
            }
        }
        record(s, Op::Abort { sb });
    });
}

/// `updating`.
pub fn updating(id: u32, sb: u16) -> bool {
    with(id, |s| s.sbs.get(sb as usize).is_some_and(|b| b.updating)).unwrap_or(false)
}

/// `timestampOffset` / `mode` / `appendWindowStart` / `appendWindowEnd` setters.
pub fn set_timestamp_offset(id: u32, sb: u16, secs: f64) {
    with(id, |s| {
        reopen_if_ended(s);
        record(s, Op::TimestampOffset { sb, ns: ns(secs) });
    });
}

pub fn set_mode(id: u32, sb: u16, sequence: bool) {
    with(id, |s| {
        reopen_if_ended(s);
        record(s, Op::Mode { sb, sequence });
    });
}

pub fn set_append_window(id: u32, sb: u16, start: f64, end: f64) {
    with(id, |s| {
        record(s, Op::AppendWindow { sb, start_ns: ns(start), end_ns: ns(end) });
    });
}

/// (timestampOffset, mode is sequence, appendWindowStart, appendWindowEnd) in seconds.
pub fn sb_settings(id: u32, sb: u16) -> (f64, bool, f64, f64) {
    with(id, |s| {
        s.model.buffer(sb).map(|b| {
            let end = if b.append_window_end_ns == i64::MAX { f64::INFINITY } else { b.append_window_end_ns as f64 / 1e9 };
            (b.timestamp_offset_ns as f64 / 1e9, b.mode == gneiss_pal::dsp::demux::fragment::AppendMode::Sequence, b.append_window_start_ns as f64 / 1e9, end)
        })
    })
    .flatten()
    .unwrap_or((0.0, false, 0.0, f64::INFINITY))
}

/// `endOfStream(error)` (§2.4.7): readyState "ended", `sourceended`; without an error the duration
/// becomes the highest end time and the element can reach its end.
pub fn end_of_stream(id: u32, error: u8) {
    let el = with(id, |s| {
        record(s, Op::EndOfStream { error });
        queue(s.target, "sourceended");
        s.element.clone()
    })
    .flatten();
    if let Some(el) = el {
        if error == 0 {
            super::mse_changed(&el, true);
        } else {
            super::mse_decode_error(&el);
        }
    }
}

fn flat(r: &[(i64, i64)]) -> Vec<f64> {
    r.iter().flat_map(|&(a, b)| [a as f64 / 1e9, if b == i64::MAX { f64::INFINITY } else { b as f64 / 1e9 }]).collect()
}

/// `SourceBuffer.buffered` as [start0, end0, start1, end1, …] seconds.
pub fn sb_buffered(id: u32, sb: u16) -> Vec<f64> {
    with(id, |s| s.model.buffer(sb).map(|b| flat(&b.buffered())).unwrap_or_default()).unwrap_or_default()
}

/// The MediaSource's `buffered` (the element's, §2.4.5).
pub fn buffered(id: u32) -> Vec<(i64, i64)> {
    with(id, |s| s.model.buffered()).unwrap_or_default()
}

/// (every SourceBuffer has its init segment, ended, duration ns or None, the buffered ranges).
pub fn element_view(id: u32) -> Option<(bool, bool, Option<i64>, Vec<(i64, i64)>)> {
    with(id, |s| (s.model.all_init(), s.model.ready == ReadyState::Ended, s.model.duration_ns, s.model.buffered()))
}

/// `MediaSource.isTypeSupported(type)`: an MSE byte stream format this engine parses (ISO BMFF,
/// WebM) whose codecs Stria really decodes — a codec that would play as the labelled stand-in
/// (H.264, HEVC) is refused, so an adaptive player picks one that plays.
pub fn is_type_supported(mime: &str) -> bool {
    let base = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if !matches!(base.as_str(), "video/mp4" | "audio/mp4" | "video/webm" | "audio/webm") {
        return false;
    }
    super::play_support(mime) == 2 || (super::play_support(mime) == 1 && !mime.contains("codecs"))
}
