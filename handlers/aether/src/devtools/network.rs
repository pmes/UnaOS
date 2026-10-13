//! The Network panel: every resource the document references, with method, status, type, size and time
//! where the engine's HTTP cache recorded the exchange, and the requests the security model refused.
//!
//! Seam: the requests are the ones the page loader issues for this document (`net::fetch_page`: the
//! document, `<link rel=stylesheet>`, `<script src>`, `<img src>` / `srcset`, media `src` / `poster`,
//! `<iframe src>`), read from the live DOM and resolved against `AetherEngine::current_url`; each GET is
//! looked up in `net::http_cache()` under the key gneiss_pal stores it under — `cache_key(partition, url)`,
//! the partition being the top-level page's site (`gneiss_pal::api::http::site_of`, AETHERCACHE SR88) —
//! whose stored `Entry` carries the status, the response fields (Content-Type), the body and the
//! request/response times. The blocked rows are `security::refusals()` (AETHERORIGIN SR86), whose names
//! carry the refused URL (`security-<rule>:<url, clipped to 64>`).
//!
//! What the cache does not hold is reported as not recorded, never guessed: a `no-store` answer, a
//! non-GET, a `file:` / `data:` load. The cache's times are Unix seconds, so `time_ms` has one-second
//! resolution for those rows.
//!
//! Requests script makes (`fetch`, XHR, a synchronous exchange) come from `api::fetch::request_log`
//! (AETHERDEVTOOLS2): the per-request log the fetch worker keeps as each exchange runs — method, status,
//! Content-Type, bytes and wall time in ms — so a `no-store` answer, a POST or a failed request appears
//! too. They follow the document's rows, oldest first.

use crate::AetherEngine;

/// What started a request (the Type column's initiator).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Initiator {
    Document,
    Stylesheet,
    Script,
    Image,
    Media,
    Frame,
    /// A script request: `fetch()` or an asynchronous XHR.
    Fetch,
    /// A synchronous exchange: a synchronous XHR, an iframe's document, a worker's `importScripts`.
    Sync,
    Other,
}

impl Initiator {
    pub fn label(self) -> &'static str {
        match self {
            Initiator::Document => "document",
            Initiator::Stylesheet => "stylesheet",
            Initiator::Script => "script",
            Initiator::Image => "img",
            Initiator::Media => "media",
            Initiator::Frame => "iframe",
            Initiator::Fetch => "fetch/xhr",
            Initiator::Sync => "sync",
            Initiator::Other => "other",
        }
    }
}

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub method: &'static str,
    pub initiator: Initiator,
    /// The stored status (None: the cache holds no record of it).
    pub status: Option<u16>,
    /// The stored `Content-Type`.
    pub mime: Option<String>,
    /// The stored body's length in bytes.
    pub size: Option<usize>,
    /// Response time minus request time, in ms (one-second resolution).
    pub time_ms: Option<i64>,
    /// The security refusal that blocked it (`security-<rule>`), when one names its URL.
    pub blocked: Option<String>,
    /// Why a script request failed (a refusal, a network error, `cancelled`).
    pub error: Option<String>,
}

impl Request {
    /// The row as the text face's columns.
    pub fn row(&self) -> String {
        let dash = || "-".to_string();
        let status = match (&self.blocked, &self.error, self.status) {
            (Some(_), _, _) => "blocked".to_string(),
            (None, Some(_), _) => "failed".to_string(),
            (None, None, Some(s)) => s.to_string(),
            (None, None, None) => dash(),
        };
        let mime = self.mime.clone().unwrap_or_else(dash);
        let size = self.size.map(|s| s.to_string()).unwrap_or_else(dash);
        let ms = self.time_ms.map(|t| t.to_string()).unwrap_or_else(dash);
        let mut s = format!("{:<7} {:<6} {:<10} {:<24} {:>9} {:>7}  {}", self.method, status, self.initiator.label(), mime, size, ms, self.url);
        if let Some(b) = &self.blocked {
            s.push_str(&format!("  ({b})"));
        } else if let Some(e) = &self.error {
            s.push_str(&format!("  ({e})"));
        }
        s
    }
}

/// The resource references of the document, in tree order (the document first), deduplicated.
pub fn references(engine: &AetherEngine) -> Vec<(Initiator, String)> {
    let base = engine.current_url();
    let mut out: Vec<(Initiator, String)> = Vec::new();
    let mut push = |kind: Initiator, raw: &str| {
        let raw = raw.trim();
        if raw.is_empty() {
            return;
        }
        let Ok(u) = html_core::url::Url::parse_str_with_base(raw, Some(&base)) else { return };
        let href = u.serialize(true);
        if !out.iter().any(|(_, h)| *h == href) {
            out.push((kind, href));
        }
    };
    if !base.is_empty() {
        push(Initiator::Document, &base);
    }
    let Some(doc) = engine.document.as_ref() else { return out };
    for n in doc.document_node().descendants() {
        let Some(e) = n.as_element() else { continue };
        let attrs = e.attributes.borrow();
        let get = |k: &str| attrs.get(k).map(str::to_string);
        match e.name.local.0.as_str() {
            "link" => {
                let rel = get("rel").unwrap_or_default().to_ascii_lowercase();
                let rels: Vec<&str> = rel.split_whitespace().collect();
                if let Some(h) = get("href") {
                    if rels.contains(&"stylesheet") {
                        push(Initiator::Stylesheet, &h);
                    } else if rels.iter().any(|r| matches!(*r, "icon" | "preload" | "modulepreload" | "manifest")) {
                        push(Initiator::Other, &h);
                    }
                }
            }
            "script" => {
                if let Some(s) = get("src") {
                    push(Initiator::Script, &s);
                }
            }
            "img" => {
                if let Some(s) = get("src") {
                    push(Initiator::Image, &s);
                }
                for cand in get("srcset").unwrap_or_default().split(',') {
                    if let Some(u) = cand.split_whitespace().next() {
                        push(Initiator::Image, u);
                    }
                }
            }
            "video" | "audio" | "source" | "track" => {
                if let Some(s) = get("src") {
                    push(Initiator::Media, &s);
                }
                if let Some(p) = get("poster") {
                    push(Initiator::Image, &p);
                }
            }
            "iframe" | "frame" | "embed" => {
                if let Some(s) = get("src") {
                    push(Initiator::Frame, &s);
                }
            }
            "object" => {
                if let Some(s) = get("data") {
                    push(Initiator::Frame, &s);
                }
            }
            _ => {}
        }
    }
    out
}

/// The panel's rows: each reference with what the cache recorded of it and whether a refusal named it,
/// then each refusal that names a URL no reference has.
pub fn requests(engine: &AetherEngine) -> Vec<Request> {
    let top = engine.current_url();
    let partition = gneiss_pal::api::http::site_of(&top);
    let cache = crate::net::http_cache();
    let refusals = crate::security::refusals();
    // (rule, the URL it names) of each refusal whose detail is a URL.
    let refused: Vec<(String, String)> = refusals
        .iter()
        .filter_map(|r| {
            let rest = r.strip_prefix("security-")?;
            let (rule, detail) = rest.split_once(':')?;
            Some((format!("security-{rule}"), url_in(detail)?.to_string()))
        })
        .collect();
    let blocked_for = |url: &str| refused.iter().find(|(_, d)| url.starts_with(d.as_str())).map(|(r, _)| r.clone());
    let mut out: Vec<Request> = references(engine)
        .into_iter()
        .map(|(initiator, url)| {
            let mut r = Request { url: url.clone(), method: "GET", initiator, status: None, mime: None, size: None, time_ms: None, blocked: blocked_for(&url), error: None };
            if let Ok(u) = html_core::url::Url::parse(&url) {
                if matches!(u.scheme(), "http" | "https") {
                    let part = partition.clone().or_else(|| gneiss_pal::api::http::site_of(&url)).unwrap_or_default();
                    let key = format!("{part} {}", u.serialize(true));
                    if let Some(e) = cache.get(&key) {
                        r.status = Some(e.status);
                        r.mime = e.headers.get("content-type").map(str::to_string);
                        r.size = Some(e.body.len());
                        r.time_ms = Some((e.response_time - e.request_time).max(0) * 1000);
                    }
                }
            }
            r
        })
        .collect();
    for (rule, detail) in refused {
        if !out.iter().any(|r| r.url.starts_with(detail.as_str())) {
            out.push(Request {
                url: detail,
                method: "GET",
                initiator: Initiator::Other,
                status: None,
                mime: None,
                size: None,
                time_ms: None,
                blocked: Some(rule),
                error: None,
            });
        }
    }
    // The script requests this page made (AETHERDEVTOOLS2), from the fetch worker's log.
    for q in crate::api::fetch::request_log(&top) {
        out.push(Request {
            method: static_method(&q.method),
            initiator: if q.sync { Initiator::Sync } else { Initiator::Fetch },
            status: q.status,
            mime: q.mime,
            size: (q.done || q.size > 0).then_some(q.size),
            time_ms: q.time_ms.map(|t| t.min(i64::MAX as u64) as i64),
            blocked: None,
            error: q.error,
            url: q.url,
        });
    }
    out
}

/// The row's method as the column's static spelling (an extension method reads `*`).
fn static_method(m: &str) -> &'static str {
    match m {
        "GET" => "GET",
        "HEAD" => "HEAD",
        "POST" => "POST",
        "PUT" => "PUT",
        "DELETE" => "DELETE",
        "PATCH" => "PATCH",
        "OPTIONS" => "OPTIONS",
        "CONNECT" => "CONNECT",
        "TRACE" => "TRACE",
        _ => "*",
    }
}

/// The URL a refusal's detail names: from the scheme before its first `://` to the end (a CSP detail is
/// `<directive>:<url>`, the others are the URL itself; either may be clipped to 64 bytes).
fn url_in(detail: &str) -> Option<&str> {
    let sep = detail.find("://")?;
    let start = detail[..sep]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        .last()
        .map(|(i, _)| i)?;
    let url = &detail[start..];
    (!url.is_empty()).then_some(url)
}
