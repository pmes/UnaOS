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
//! AETHERDEVTOOLS3 (SR128): each row recorded in the cache also carries the response fields as stored (the
//! `Entry`'s headers, content codings already decoded, `Set-Cookie` removed), the request fields the
//! cache kept (only the ones the response's `Vary` named — the cache stores no other request field, so
//! no other is shown), and a body preview ([`preview`]: JSON pretty-printed, text as is, an image as its
//! format and pixel size read from its header, anything else as its byte count).
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
    /// The stored response fields, in stored order (empty: not recorded).
    pub response_headers: Vec<(String, String)>,
    /// The request fields the cache kept (those the response's `Vary` named, absent ones as `(absent)`).
    pub request_headers: Vec<(String, String)>,
    /// The stored body's preview (None: no body recorded).
    pub preview: Option<Preview>,
}

/// A response body as the Network panel previews it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    /// A zero-length body.
    Empty,
    /// JSON, pretty-printed (two-space indent).
    Json(String),
    /// Text as delivered (at most [`PREVIEW_BYTES`]).
    Text(String),
    /// An image: its format and pixel size from its header.
    Image { format: &'static str, width: u32, height: u32 },
    /// Anything else: the byte count.
    Binary(usize),
}

/// Bytes of a text body the preview keeps.
pub const PREVIEW_BYTES: usize = 64 * 1024;

/// Preview lines [`Request::detail_lines`] shows before it clips.
pub const PREVIEW_LINES: usize = 12;

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

    /// Whether the cache recorded the exchange's headers or body.
    pub fn has_detail(&self) -> bool {
        !self.response_headers.is_empty() || !self.request_headers.is_empty() || self.preview.is_some()
    }

    /// The row's detail block, indented under it: the response headers, the request headers the cache
    /// kept, and the body preview (at most [`PREVIEW_LINES`] lines of it). A row the cache holds nothing of
    /// says so.
    pub fn detail_lines(&self) -> Vec<String> {
        if !self.has_detail() {
            return vec!["    headers and body: not recorded".to_string()];
        }
        let mut out = vec![format!("    Response headers ({})", self.response_headers.len())];
        for (k, v) in &self.response_headers {
            out.push(format!("      {k}: {v}"));
        }
        if !self.request_headers.is_empty() {
            out.push(format!("    Request headers ({}, the ones Vary named)", self.request_headers.len()));
            for (k, v) in &self.request_headers {
                out.push(format!("      {k}: {v}"));
            }
        }
        let body = |out: &mut Vec<String>, kind: &str, text: &str| {
            let lines: Vec<&str> = text.lines().collect();
            out.push(format!("    Preview ({kind}, {} lines)", lines.len()));
            for l in lines.iter().take(PREVIEW_LINES) {
                out.push(format!("      {l}"));
            }
            if lines.len() > PREVIEW_LINES {
                out.push(format!("      … {} more lines", lines.len() - PREVIEW_LINES));
            }
        };
        match &self.preview {
            None => out.push("    Preview: no body recorded".to_string()),
            Some(Preview::Empty) => out.push("    Preview: empty body".to_string()),
            Some(Preview::Json(t)) => body(&mut out, "JSON", t),
            Some(Preview::Text(t)) => body(&mut out, "text", t),
            Some(Preview::Image { format, width, height }) => out.push(format!("    Preview: {format} image, {width} x {height}")),
            Some(Preview::Binary(n)) => out.push(format!("    Preview: {n} bytes, binary")),
        }
        out
    }
}

/// The preview of a body of type `mime`: an image by its signature (whatever the type says), JSON
/// pretty-printed when the type is JSON (`application/json`, `text/json`, `*+json`) or an untyped / plain
/// text body that parses as a JSON object or array, text when it is UTF-8 with no NUL, else its byte count.
pub fn preview(mime: Option<&str>, body: &[u8]) -> Preview {
    if body.is_empty() {
        return Preview::Empty;
    }
    if let Some((format, width, height)) = image_size(body) {
        return Preview::Image { format, width, height };
    }
    let essence = mime.unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let Ok(text) = std::str::from_utf8(body) else { return Preview::Binary(body.len()) };
    if text.contains('\0') {
        return Preview::Binary(body.len());
    }
    let jsonish = matches!(essence.as_str(), "application/json" | "text/json") || essence.ends_with("+json");
    let looks = matches!(essence.as_str(), "" | "text/plain") && matches!(text.trim_start().chars().next(), Some('{' | '['));
    if jsonish || looks {
        if let Some(p) = pretty_json(text) {
            return Preview::Json(p);
        }
    }
    let mut end = text.len().min(PREVIEW_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Preview::Text(text[..end].to_string())
}

/// `text` re-indented as JSON (two spaces per level, `"key": value`, empty containers kept `{}` / `[]`).
/// None when it is not well-formed structurally: an unterminated string, a bracket that does not match,
/// text after the value, or a top level that is not an object or array.
pub fn pretty_json(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut stack: Vec<char> = Vec::new();
    let mut chars = text.trim().chars().peekable();
    if !matches!(chars.peek(), Some('{' | '[')) {
        return None;
    }
    let indent = |out: &mut String, n: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(n));
    };
    let mut done = false;
    while let Some(c) = chars.next() {
        if done && !c.is_whitespace() {
            return None;
        }
        match c {
            '"' => {
                out.push('"');
                loop {
                    let d = chars.next()?;
                    out.push(d);
                    match d {
                        '\\' => out.push(chars.next()?),
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '{' | '[' => {
                stack.push(c);
                out.push(c);
                while chars.next_if(|n| n.is_whitespace()).is_some() {}
                let close = if c == '{' { '}' } else { ']' };
                if chars.peek() == Some(&close) {
                    chars.next();
                    stack.pop();
                    out.push(close);
                    done = stack.is_empty();
                } else {
                    indent(&mut out, stack.len());
                }
            }
            '}' | ']' => {
                let open = stack.pop()?;
                if (open == '{') != (c == '}') {
                    return None;
                }
                indent(&mut out, stack.len());
                out.push(c);
                done = stack.is_empty();
            }
            ',' => {
                if stack.is_empty() {
                    return None;
                }
                out.push(',');
                indent(&mut out, stack.len());
            }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    (done && stack.is_empty()).then_some(out)
}

/// An image's format and pixel size from its header: PNG (IHDR), GIF (logical screen), JPEG (the first SOFn
/// segment), WebP (VP8, VP8L, VP8X) and BMP. None for anything else or a header cut short.
pub fn image_size(b: &[u8]) -> Option<(&'static str, u32, u32)> {
    let be16 = |i: usize| -> Option<u32> { Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32) };
    let le16 = |i: usize| -> Option<u32> { Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32) };
    let be32 = |i: usize| -> Option<u32> { Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?)) };
    let le32 = |i: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?)) };
    let le24 = |i: usize| -> Option<u32> { Some(le16(i)? | (*b.get(i + 2)? as u32) << 16) };
    if b.starts_with(b"\x89PNG\r\n\x1a\n") && b.get(12..16) == Some(&b"IHDR"[..]) {
        return Some(("PNG", be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some(("GIF", le16(6)?, le16(8)?));
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(&b"WEBP"[..]) {
        return match b.get(12..16)? {
            b"VP8 " => Some(("WebP", le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let (b0, b1, b2, b3) = (*b.get(21)? as u32, *b.get(22)? as u32, *b.get(23)? as u32, *b.get(24)? as u32);
                Some(("WebP", 1 + (((b1 & 0x3f) << 8) | b0), 1 + (((b3 & 0xf) << 10) | (b2 << 2) | ((b1 & 0xc0) >> 6))))
            }
            b"VP8X" => Some(("WebP", 1 + le24(24)?, 1 + le24(27)?)),
            _ => None,
        };
    }
    if b.starts_with(b"BM") && b.len() >= 26 {
        return Some(("BMP", (le32(18)? as i32).unsigned_abs(), (le32(22)? as i32).unsigned_abs()));
    }
    if b.starts_with(&[0xff, 0xd8]) {
        let mut i = 2;
        while i + 3 < b.len() {
            if b[i] != 0xff {
                return None;
            }
            let m = b[i + 1];
            if m == 0xff {
                i += 1;
                continue;
            }
            if matches!(m, 0xc0..=0xcf) && !matches!(m, 0xc4 | 0xc8 | 0xcc) {
                return Some(("JPEG", be16(i + 7)?, be16(i + 5)?));
            }
            if matches!(m, 0x01 | 0xd0..=0xd8) {
                i += 2;
                continue;
            }
            i += 2 + be16(i + 2)? as usize;
        }
    }
    None
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
            let mut r = Request {
                url: url.clone(),
                method: "GET",
                initiator,
                status: None,
                mime: None,
                size: None,
                time_ms: None,
                blocked: blocked_for(&url),
                error: None,
                response_headers: Vec::new(),
                request_headers: Vec::new(),
                preview: None,
            };
            if let Ok(u) = html_core::url::Url::parse(&url) {
                if matches!(u.scheme(), "http" | "https") {
                    let part = partition.clone().or_else(|| gneiss_pal::api::http::site_of(&url)).unwrap_or_default();
                    let key = format!("{part} {}", u.serialize(true));
                    if let Some(e) = cache.get(&key) {
                        r.status = Some(e.status);
                        r.mime = e.headers.get("content-type").map(str::to_string);
                        r.size = Some(e.body.len());
                        r.time_ms = Some((e.response_time - e.request_time).max(0) * 1000);
                        r.response_headers = e.headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                        r.request_headers = e.vary.iter().map(|(k, v)| (k.clone(), v.clone().unwrap_or_else(|| "(absent)".to_string()))).collect();
                        r.preview = Some(preview(r.mime.as_deref(), &e.body));
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
                response_headers: Vec::new(),
                request_headers: Vec::new(),
                preview: None,
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
            // The fetch worker's log keeps no fields or body (api/fetch.rs `ScriptRequest`).
            response_headers: Vec::new(),
            request_headers: Vec::new(),
            preview: None,
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
