use anyhow::{Context, Result};
use gneiss_pal::api::http::{self, CacheMode, Client, CookieJar, HttpCache};
use html_core::url;
use std::sync::{Arc, Mutex};

// HTTPCORE (SR51): every Aether fetch goes through gneiss_pal::api::http — UnaOS's own http_core (WHATWG URL,
// HTTP/1.1, RFC 6265 jar, gzip/deflate) over tls_core (TLS 1.3, verified). reqwest and its cookie_store are gone.
//
// AETHERCACHE (SR88): HTTP/2 by default for https (one multiplexed connection per origin), one RFC 9111 cache
// for the page loader and the page's own fetch/XHR (a memory tier emptied per page load over a disk tier in
// storage::http_cache_dir(), 256 MiB, LRU), every request partitioned by the top-level site, and the page's
// subresources loaded through a PRIORITY QUEUE — document > CSS > fonts > scripts > images — with bounded
// concurrency, deduplicated, each request carrying its RFC 9218 `Priority` urgency.

/// Memory tier: the current page's responses.
const CACHE_MEMORY_BYTES: usize = 64 << 20;
/// Disk tier bound.
const CACHE_DISK_BYTES: u64 = 256 << 20;

/// The process-wide HTTP cache every Aether client shares.
pub fn http_cache() -> Arc<HttpCache> {
    static CACHE: std::sync::OnceLock<Arc<HttpCache>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| Arc::new(HttpCache::with_disk(CACHE_MEMORY_BYTES, crate::storage::http_cache_dir(), CACHE_DISK_BYTES)))
        .clone()
}

/// A page's resource classes in load priority order (lower loads first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Document = 0,
    Css = 1,
    Font = 2,
    Script = 3,
    Image = 4,
}

impl Priority {
    /// RFC 9218 §4: the `Priority` request field (urgency 0 highest; images are incremental).
    pub fn header(self) -> &'static str {
        match self {
            Priority::Document => "u=0",
            Priority::Css => "u=1",
            Priority::Font => "u=2",
            Priority::Script => "u=3",
            Priority::Image => "u=5, i",
        }
    }
}

/// A GET through the shared client: priority, cache partition (the top-level site), default cache mode.
async fn get(url: &str, pri: Priority, partition: Option<&str>) -> std::result::Result<http::Response, http::Error> {
    let mut rb = client().get(url).header("Priority", pri.header()).cache_mode(CacheMode::Default);
    if let Some(p) = partition {
        rb = rb.partition(p);
    }
    rb.send().await
}

/// Process-wide, in-memory cookie jar (session-scoped: nothing is written
/// to disk, so quitting Aether logs you out). Shared by every client we
/// build, so a Set-Cookie collected on one navigation is sent on the next
/// and on subresource/script fetches to the same host.
pub fn cookie_jar() -> Arc<Mutex<CookieJar>> {
    static JAR: std::sync::OnceLock<Arc<Mutex<CookieJar>>> = std::sync::OnceLock::new();
    JAR.get_or_init(|| Arc::new(Mutex::new(CookieJar::new()))).clone()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Cookies the jar would send to `url`, formatted as a `Cookie:` header
/// value (`"a=b; c=d"`), or `""` when the jar has none for that host.
/// Read API for `document.cookie`: a non-HTTP API, so HttpOnly cookies are
/// not visible (RFC 6265 §5.4 step 1).
pub fn cookies_for(url: &str) -> String {
    let Ok(parsed) = http::Url::parse(url) else { return String::new() };
    cookie_jar().lock().ok().and_then(|mut j| j.cookie_header(&parsed, now_secs(), false)).unwrap_or_default()
}

/// Write API for `document.cookie`: adds one `"name=value; attrs"` string to
/// the shared jar, scoped to `url` (non-HTTP: an HttpOnly cookie is refused,
/// RFC 6265 §5.3 step 10). Malformed urls are dropped silently, as a browser
/// drops a cookie it cannot scope.
pub fn set_cookie(url: &str, cookie_str: &str) {
    if let Ok(parsed) = http::Url::parse(url) {
        if let Ok(mut j) = cookie_jar().lock() {
            j.set_cookie(&parsed, cookie_str, now_secs(), false);
        }
    }
}

/// Blocking client builder for the sync JS paths (`fetch`/XHR run on their
/// own thread, off the tokio runtime). Pre-wired to the shared jar so those
/// requests carry — and record — the same cookies as the page load.
pub fn blocking_client_builder() -> http::blocking::ClientBuilder {
    http::blocking::ClientBuilder::new().user_agent("UnaOS Aether/0.1.0").cookie_jar(cookie_jar()).cache(http_cache())
}

/// One shared HTTP client: connection pooling (keep-alive connection threads)
/// across every fetch of a page load (a per-fetch Client paid a fresh handshake
/// for each of yahoo's dozens of resources).
fn client() -> &'static Client {
    static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent("UnaOS Aether/0.1.0")
            .cookie_jar(cookie_jar())
            .cache(http_cache())
            .build()
            .expect("http client")
    })
}

pub fn normalize_url(input: &str) -> Vec<String> {
    if input.starts_with("file://") {
        return vec![];
    }
    if !input.contains("://") {
        vec![format!("https://{}", input), format!("http://{}", input)]
    } else {
        vec![input.to_string()]
    }
}

pub async fn fetch_document(input: &str) -> Result<String> {
    fetch_document_final(input).await.map(|(text, _)| text)
}

/// [`fetch_document`] that also returns the FINAL response URL — after redirects (and after the
/// https-then-http guess for a scheme-less input): the document's URL, which its relative URLs resolve
/// against and the address bar shows (HTML §7.4.1, the response's URL).
pub async fn fetch_document_final(input: &str) -> Result<(String, String)> {
    if input.starts_with("file://") {
        anyhow::bail!("file:// fetches are disabled for security reasons.");
    }
    let urls = normalize_url(input);
    if urls.is_empty() {
        anyhow::bail!("Invalid URL");
    }

    let mut last_err = None;
    for url in urls {
        // AETHERSW (SR104): a navigation under a registered scope is the controlling worker's `fetch` event first.
        if let Some(answer) = sw_navigate(&url).await {
            return answer;
        }
        match get(&url, Priority::Document, None).await {
            Ok(response) => {
                if !response.status().is_success() {
                    return Err(anyhow::anyhow!("HTTP Error: {}", response.status()));
                }
                let final_url = response.url().to_string();
                // AETHERORIGIN (SR86): the document's response headers (CSP, Referrer-Policy, Reporting-Endpoints)
                // are the navigation's, read before the document loads.
                let doc_headers: Vec<(String, String)> =
                    response.headers().iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
                crate::security::set_document_headers(&final_url, &doc_headers);

                if response.content_length().unwrap_or(0) > 10 * 1024 * 1024 {
                    anyhow::bail!("Response exceeds size limit");
                }
                
                let bytes = response.bytes().await.context("Failed to read body")?;
                if bytes.len() > 10 * 1024 * 1024 {
                    anyhow::bail!("Response body exceeds size limit");
                }
                
                let text = String::from_utf8(bytes).context("Invalid UTF-8")?;
                return Ok((text, final_url));
            }
            Err(e) => {
                last_err = Some(anyhow::anyhow!(e).context("Connection failure"));
            }
        }
    }
    
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Failed to fetch URL")))
}

// ------------------------------------------------------------------------------------------ AETHERSW (SR104)
//
// Service Workers §6.4 "handle fetch" at the page loader: the navigation (`fetch_document_final` →
// `sw_navigate` → `workers::sw::intercept_navigation`) and the controlled document's subresources (`fetch_page`
// → `workers::sw::navigation_controller` → `Loader::sw` → `sw_subresource`). The worker's exchange blocks (its
// realm runs on its own thread), so it runs on tokio's blocking pool, never on the executor.

/// Runs a blocking service-worker exchange off the async executor (directly when no runtime is current).
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    match tokio::runtime::Handle::try_current() {
        Ok(h) => h.spawn_blocking(f).await.ok(),
        Err(_) => Some(f()),
    }
}

fn header_of<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// A top-level navigation's worker answer: `None` = no worker handled it (the network serves the URL as
/// before); `Some(Ok((html, final URL)))` = the worker's document; `Some(Err)` = a network error (a rejected
/// `respondWith`, a refused opaque answer, a non-2xx answer). A worker's redirect (`Response.redirect`) is
/// followed as a new navigation, intercepted again (20 hops), and handed to the network when no worker takes it.
async fn sw_navigate(input: &str) -> Option<Result<(String, String)>> {
    use crate::workers::sw::{self, SwAnswer};
    if !sw::may_intercept_navigation(input) {
        return None;
    }
    let mut url = input.to_string();
    for hop in 0..20u32 {
        let u = url.clone();
        let answer = blocking(move || sw::intercept_navigation(&u)).await.unwrap_or(SwAnswer::Network);
        let r = match answer {
            SwAnswer::Network if hop == 0 => return None,
            SwAnswer::Network => return Some(network_document(&url).await),
            SwAnswer::Error(e) => {
                crate::ledger::record_js(&format!("sw-navigation-error:{}", crate::js::clip(&url, 48)));
                return Some(Err(anyhow::anyhow!("Service worker network error: {e}")));
            }
            SwAnswer::Respond(r) => r,
        };
        if matches!(r.status, 301 | 302 | 303 | 307 | 308) {
            if let Some(next) = header_of(&r.headers, "location").and_then(|loc| url::Url::parse(&url).and_then(|b| b.join(loc)).ok()) {
                if !matches!(next.scheme(), "http" | "https") {
                    return Some(Err(anyhow::anyhow!("redirect to unsupported scheme {}", next.scheme())));
                }
                url = next.to_string();
                continue;
            }
        }
        if !(200..300).contains(&r.status) {
            return Some(Err(anyhow::anyhow!("HTTP Error: {} (from the service worker)", r.status)));
        }
        // HTML §7.4.1: the document's URL is the response's (a passed-through `fetch` keeps its final URL; a
        // constructed Response has none and is the request's).
        let final_url = if r.url.starts_with("http://") || r.url.starts_with("https://") { r.url.clone() } else { url.clone() };
        let doc_headers: Vec<(String, String)> = r.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
        crate::security::set_document_headers(&final_url, &doc_headers);
        if r.body.len() > 10 * 1024 * 1024 {
            return Some(Err(anyhow::anyhow!("Response body exceeds size limit")));
        }
        return Some(String::from_utf8(r.body).context("Invalid UTF-8").map(|text| (text, final_url)));
    }
    Some(Err(anyhow::anyhow!("too many service worker redirects")))
}

/// One network GET of a document (no worker): the network half of [`fetch_document_final`] for one URL.
async fn network_document(url: &str) -> Result<(String, String)> {
    let response = get(url, Priority::Document, None).await.context("Connection failure")?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    let final_url = response.url().to_string();
    let doc_headers: Vec<(String, String)> = response.headers().iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
    crate::security::set_document_headers(&final_url, &doc_headers);
    let bytes = response.bytes().await.context("Failed to read body")?;
    if bytes.len() > 10 * 1024 * 1024 {
        anyhow::bail!("Response body exceeds size limit");
    }
    Ok((String::from_utf8(bytes).context("Invalid UTF-8")?, final_url))
}

/// A controlled document's subresource through its worker: `None` = the network serves it; `Some(Ok(body))` =
/// the worker's 2xx answer (or an opaque answer to a `no-cors` load, whose body the page may render but script
/// never reads); `Some(Err)` = a network error.
async fn sw_subresource(h: &crate::workers::sw::SwHandle, url: &str, pri: Priority) -> Option<Result<Vec<u8>>> {
    use crate::workers::sw::{SwAnswer, SwRequest};
    // Fetch's request modes for the loader's classes: `<link rel=stylesheet>`, classic `<script>` and `<img>`
    // without `crossorigin` are no-cors; fonts are always cors (CSS Fonts §4.8.2).
    let (mode, dest, accept) = match pri {
        Priority::Document => ("navigate", "document", "text/html,*/*;q=0.8"),
        Priority::Css => ("no-cors", "style", "text/css,*/*;q=0.1"),
        Priority::Font => ("cors", "font", "*/*"),
        Priority::Script => ("no-cors", "script", "*/*"),
        Priority::Image => ("no-cors", "image", "image/avif,image/webp,image/png,image/svg+xml,image/*;q=0.8,*/*;q=0.5"),
    };
    let (h, u) = (h.clone(), url.to_string());
    let answer = blocking(move || {
        let headers = vec![("Accept".to_string(), accept.to_string())];
        h.fetch(&SwRequest { method: "GET", url: &u, headers: &headers, body: &[], client: 0, mode, destination: dest })
    })
    .await
    .unwrap_or(SwAnswer::Network);
    match answer {
        SwAnswer::Network => None,
        SwAnswer::Error(e) => Some(Err(anyhow::anyhow!("Service worker network error: {e}"))),
        SwAnswer::Respond(r) if r.kind == "opaque" || (200..300).contains(&r.status) => {
            if r.body.len() > 10 * 1024 * 1024 {
                return Some(Err(anyhow::anyhow!("Response body exceeds size limit")));
            }
            Some(Ok(r.body))
        }
        SwAnswer::Respond(r) => Some(Err(anyhow::anyhow!("HTTP Error: {} (from the service worker)", r.status))),
    }
}

/// POSTs a urlencoded form body and returns the response document.
pub async fn post_document(url: &str, body: &str) -> Result<String> {
    post_document_typed(url, "application/x-www-form-urlencoded", body).await.map(|(html, _)| html)
}

/// POSTs a form submission body of `content_type` (its enctype; multipart names its boundary) and
/// returns (response document, final URL after redirects — a 303 after a POST lands on a GET page).
pub async fn post_document_typed(url: &str, content_type: &str, body: &str) -> Result<(String, String)> {
    let response = client()
        .post(url)
        .header("Content-Type", content_type)
        .body(body.to_string())
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    let final_url = response.url().to_string();
    let bytes = response.bytes().await.context("Failed to read body")?;
    if bytes.len() > 10 * 1024 * 1024 {
        anyhow::bail!("Response body exceeds size limit");
    }
    Ok((String::from_utf8(bytes).context("Invalid UTF-8")?, final_url))
}

/// Collects absolute URLs of `<link rel="stylesheet">` sheets in `html`,
/// resolved against `base_url`. Pure (no network) — unit-testable offline.
pub fn collect_stylesheet_urls(base_url: &str, html: &str) -> Vec<String> {
    collect_stylesheet_links(base_url, html).into_iter().map(|l| l.url).collect()
}

/// One `<link rel=stylesheet>`: its absolute URL and what CSP and SRI read from the element
/// (AETHERDEBT SR93).
#[derive(Debug, Clone)]
pub struct SheetLink {
    pub url: String,
    pub integrity: Option<String>,
    pub nonce: String,
    pub crossorigin: bool,
}

/// [`collect_stylesheet_urls`] with each link's `integrity`, `nonce` and `crossorigin`.
pub fn collect_stylesheet_links(base_url: &str, html: &str) -> Vec<SheetLink> {
    let document = crate::dom::parse_html(html);
    let base = url::Url::parse(base_url).ok();
    let mut out = Vec::new();
    if let Ok(links) = document.select("link") {
        for link in links {
            let attrs = link.attributes.borrow();
            let is_sheet = attrs
                .get("rel")
                .map(|r| r.split_whitespace().any(|w| w.eq_ignore_ascii_case("stylesheet")))
                .unwrap_or(false);
            if !is_sheet {
                continue;
            }
            let Some(href) = attrs.get("href") else { continue };
            let resolved = match &base {
                Some(b) => b.join(href).map(|u| u.to_string()).ok(),
                None => Some(href.to_string()),
            };
            if let Some(u) = resolved {
                if u.starts_with("http://") || u.starts_with("https://") {
                    out.push(SheetLink {
                        url: u,
                        integrity: attrs.get("integrity").map(str::to_string).filter(|v| !v.trim().is_empty()),
                        nonce: attrs.get("nonce").map(str::to_string).unwrap_or_default(),
                        crossorigin: attrs.get("crossorigin").is_some(),
                    });
                }
            }
        }
    }
    out
}

/// Everything a page load needs, fetched up front (fetch-then-apply).
#[derive(Default)]
pub struct Page {
    pub base_url: String,
    pub html: String,
    pub sheets: Vec<String>,
    pub images: Vec<(String, image::RgbaImage)>,
    /// Script sources in DOCUMENT ORDER: inline bodies and fetched external
    /// `src` texts interleaved as they appear (execution order matters).
    pub scripts: Vec<String>,
    /// Parallel to `scripts`: each entry is the source script's ordinal
    /// among ALL `<script>` elements in the parsed document. The slot list
    /// is not aligned with the element list (non-JS types are filtered, the
    /// fetch cap drops a tail, empty inline bodies take no slot, and failed
    /// fetches collapse), so the ordinal is what lets the loader hand
    /// `document.currentScript` the element a running script came from.
    pub script_nodes: Vec<usize>,
    /// Parallel to `scripts`: the absolute URL an external script was fetched from ("" for inline).
    /// The loader runs scripts as the parser reaches them; it takes external sources from here by URL.
    pub script_urls: Vec<String>,
    /// Absolute URL of the page's favicon ([`favicon_url`]), resolved at parse
    /// time but NOT fetched here — the shell fetches it after the page is
    /// delivered so chrome decoration never delays a load.
    pub favicon: Option<String>,
}

/// One collected script slot: the ordinal of its `<script>` element among
/// all script elements in the document, plus its source text (`None` until
/// an external fetch fills it in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSlot {
    pub ordinal: usize,
    pub text: Option<String>,
    /// The absolute `src` URL of an external script.
    pub url: Option<String>,
}

/// Collects the page's scripts in document order: inline text directly,
/// external `src` resolved to absolute URLs for the caller to fetch into
/// the matching slot. Non-JS types (json/ld+json/template) are skipped.
/// Every slot carries the ordinal of the `<script>` element it came from so
/// the caller can re-find that element in its own parse of the same HTML.
pub fn collect_scripts(base_url: &str, html: &str) -> (Vec<ScriptSlot>, Vec<(usize, String)>) {
    let document = crate::dom::parse_html(html);
    let mut slots: Vec<ScriptSlot> = Vec::new();
    let mut external: Vec<(usize, String)> = Vec::new();
    if let Ok(scripts) = document.select("script") {
        // Counts EVERY script element, filtered or not — the ordinal has to
        // address the caller's `select("script")` enumeration, which does no
        // filtering of its own.
        for (ordinal, script) in scripts.enumerate() {
            let attrs = script.attributes.borrow();
            let ty = attrs.get("type").unwrap_or("").trim().to_ascii_lowercase();
            if !(ty.is_empty() || ty.contains("javascript") || ty == "module") {
                continue;
            }
            if let Some(src) = attrs.get("src") {
                let abs = crate::images::resolve(base_url, src);
                if abs.starts_with("http://") || abs.starts_with("https://") {
                    // Code-split app shells emit their runtime LAST: a cap
                    // that drops the tail drops the bootstrap, and every
                    // chunk already fetched sits inert. 192 clears the
                    // real-world spread (Yahoo's app router: 52).
                    if external.len() >= 192 {
                        crate::ledger::record_js("script-fetch-cap-reached");
                        continue;
                    }
                    external.push((slots.len(), abs.clone()));
                    slots.push(ScriptSlot { ordinal, text: None, url: Some(abs) });
                }
                continue;
            }
            drop(attrs);
            let text = script.as_node().text_contents();
            if !text.trim().is_empty() {
                slots.push(ScriptSlot { ordinal, text: Some(text), url: None });
            }
        }
    }
    (slots, external)
}

/// The absolute URL of the page's favicon: the first `<link>` whose `rel`
/// names an icon (`icon`, `shortcut icon`, `apple-touch-icon`, and the
/// `-precomposed` variant), any `sizes`; falling back to `<origin>/favicon.ico`
/// — the convention every server still honours. Pure (no network).
///
/// `data:` hrefs are skipped: the chrome wants bytes it can fetch, and the
/// inline case is rare enough not to be worth a second decode path here.
pub fn favicon_url(base_url: &str, html: &str) -> Option<String> {
    const ICON_RELS: &[&str] = &[
        "icon",
        "shortcut",
        "apple-touch-icon",
        "apple-touch-icon-precomposed",
    ];
    let document = crate::dom::parse_html(html);
    if let Ok(links) = document.select("link") {
        for link in links {
            let attrs = link.attributes.borrow();
            let is_icon = attrs
                .get("rel")
                .map(|r| {
                    r.split_whitespace()
                        .any(|w| ICON_RELS.iter().any(|k| w.eq_ignore_ascii_case(k)))
                })
                .unwrap_or(false);
            if !is_icon {
                continue;
            }
            let Some(href) = attrs.get("href") else { continue };
            if href.trim().is_empty() || href.trim_start().starts_with("data:") {
                continue;
            }
            let abs = crate::images::resolve(base_url, href.trim());
            if abs.starts_with("http://") || abs.starts_with("https://") {
                return Some(abs);
            }
        }
    }
    // No declared icon: the well-known location on the page's own origin.
    let mut origin = url::Url::parse(base_url).ok()?;
    if !(origin.scheme() == "http" || origin.scheme() == "https") {
        return None;
    }
    origin.set_pathname("/favicon.ico");
    origin.set_query(None);
    origin.set_fragment(None);
    Some(origin.to_string())
}

/// Fetches and decodes a favicon (the URL from [`favicon_url`]) to tightly
/// packed RGBA, returning `(width, height, rgba)`.
///
/// Deliberately NOT awaited inside [`fetch_page`]: the icon is chrome
/// decoration, so it must never sit between the user and a rendered document —
/// the caller runs this *after* the page is delivered. Failures are silent
/// (`None`): a missing `/favicon.ico` is the overwhelmingly common case, not an
/// error worth a ledger line.
///
/// PNG/JPEG/GIF/BMP/QOI/WebP (lossless and lossy) go through
/// `images::decode_raster` (UnaOS's own pixel_core + vp8_core); ICO falls back to
/// the `image` crate behind it (it picks the largest contained image), and an SVG
/// icon falls back to the rasterizer the page images use.
pub async fn fetch_favicon(url: &str) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = fetch_image_bytes(url).await.ok()?;
    let decoded = crate::images::decode_raster(&bytes).or_else(|| crate::images::decode_svg(&bytes))?;
    let (w, h) = decoded.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h, decoded.into_raw()))
}

/// Collects absolute `<img src>` URLs (capped; data:/svg skipped for now).
pub fn collect_image_urls(base_url: &str, html: &str) -> Vec<String> {
    let document = crate::dom::parse_html(html);
    let mut out = Vec::new();
    if let Ok(imgs) = document.select("img") {
        for img in imgs {
            // AETHERMEDIA (SR107): `loading=lazy` waits for its box to near the viewport (the
            // engine's lazy pass fetches it); `<picture>` picks its source by media and type.
            if crate::images::is_lazy(&img.attributes.borrow()) {
                continue;
            }
            let Some(src) = crate::images::img_source(img.as_node(), crate::images::SELECTION_VIEWPORT) else { continue };
            let src = src.as_str();
            if src.starts_with("data:") {
                continue; // decoded synchronously at load, no fetch needed
            }
            let abs = crate::images::resolve(base_url, src);
            if (abs.starts_with("http://") || abs.starts_with("https://")) && !out.contains(&abs) {
                out.push(abs);
            }
            if out.len() >= 30 {
                crate::ledger::record_dom("img-fetch-cap-reached");
                break;
            }
        }
    }
    out
}

/// Collects absolute raster `url(...)` references from CSS text (external
/// sheets, <style> blocks, style="" attrs — callers pass raw text; the scan
/// is textual so it needs no CSS object model). Capped like images.
pub fn collect_css_image_urls(base_url: &str, texts: &[&str], out: &mut Vec<String>) {
    let mut refs = Vec::new();
    for text in texts {
        collect_css_image_refs(base_url, base_url, text, &mut refs);
    }
    for (fetch_url, _key) in refs {
        if !out.contains(&fetch_url) {
            out.push(fetch_url);
        }
    }
}

/// CSS properties whose value positions an IMAGE. A `url()` in one of these
/// is worth fetching even without a recognizable file extension; a `url()` in
/// `src:` (@font-face) or `cursor:` is not.
const IMAGE_POSITION_PROPS: &[&str] = &[
    "background",
    "background-image",
    "mask",
    "mask-image",
    "-webkit-mask",
    "-webkit-mask-image",
    "list-style-image",
    "border-image",
    "border-image-source",
    "content",
];

/// The property name of the declaration containing byte offset `at`, i.e.
/// the text between the previous `;`/`{`/`}` and the next `:`.
fn declaring_property(text: &str, at: usize) -> &str {
    let head = &text[..at];
    let decl_start = head.rfind([';', '{', '}']).map(|i| i + 1).unwrap_or(0);
    let decl = &head[decl_start..];
    match decl.find(':') {
        Some(i) => decl[..i].trim(),
        None => "",
    }
}
///
/// Extensionless / query-bearing `url()`s are worth a fetch-and-sniff when
/// (a) they sit in an image-position property, and (b) the path does not
/// carry a *known non-image* extension. MediaWiki icon masks look like
/// `/w/load.php?...&image=search&format=original`: no suffix at all, but a
/// real SVG on the wire. A `.woff2` in `background` still gets skipped.
fn sniffable_ref(text: &str, url_pos: usize, inner: &str) -> bool {
    let prop = declaring_property(text, url_pos).to_ascii_lowercase();
    if !IMAGE_POSITION_PROPS.contains(&prop.as_str()) {
        return false;
    }
    let path_end = inner.find(['?', '#']).unwrap_or(inner.len());
    let has_query = inner[path_end..].starts_with('?');
    if has_query {
        return true; // a query means the path's suffix says nothing (load.php)
    }
    // No query: trust an extension if there is one — it isn't an image type
    // (we already checked the raster list), so don't spend a request on it.
    let tail = inner[..path_end].rsplit('/').next().unwrap_or("");
    match tail.rfind('.') {
        Some(dot) => dot + 1 >= tail.len(), // trailing dot only = no extension
        None => true,
    }
}

/// Scans one CSS text for raster/svg `url(...)` refs. `fetch_base` is the
/// owning sheet's URL (relative paths live on ITS host); `paint_base` is
/// the page base — paint-time lookup resolves raw urls against the page,
/// so each ref carries both the fetch URL and the paint-lookup key.
pub fn collect_css_image_refs(
    fetch_base: &str,
    paint_base: &str,
    text: &str,
    out: &mut Vec<(String, String)>,
) {
    let raster = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"];
    let mut idx = 0usize;
    while let Some(rel) = text[idx..].find("url(") {
        let start = idx + rel;
        let after = start + 4;
        let Some(end_rel) = text[after..].find(')') else { break };
        let end = after + end_rel;
        let inner = text[after..end].trim().trim_matches(|c| c == '"' || c == '\'').trim();
        idx = end;
        if inner.is_empty() || inner.starts_with('#') || inner.starts_with("data:") {
            continue; // fragment refs (svg <mask id>) and inline data both need no fetch
        }
        let path_end = inner.find(['?', '#']).unwrap_or(inner.len());
        let path = inner[..path_end].to_ascii_lowercase();
        let is_raster = raster.iter().any(|ext| path.ends_with(ext));
        if !is_raster && !sniffable_ref(text, start, inner) {
            continue;
        }
        let fetch_url = crate::images::resolve(fetch_base, inner);
        if !(fetch_url.starts_with("http://") || fetch_url.starts_with("https://")) {
            continue;
        }
        let key = crate::images::resolve(paint_base, inner);
        if !out.iter().any(|(f, _)| *f == fetch_url) {
            out.push((fetch_url, key));
        }
        // Cap on the whole ref list (img srcs land here first). A skin like
        // Vector spends dozens of refs on icon sprites alone, so 50 cut off
        // the masks; 200 still bounds a hostile sheet.
        if out.len() >= 200 {
            crate::ledger::record_css("css-image-fetch-cap-reached");
            return;
        }
    }
}

/// The text of every `<style>` element in an HTML source (for the font prefetch; the cascade gets the
/// sheets from the parsed DOM).
fn inline_style_blocks(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(k) = lower[i..].find("<style") {
        let open = i + k;
        let Some(gt) = lower[open..].find('>') else { break };
        let body = open + gt + 1;
        let Some(end) = lower[body..].find("</style") else { break };
        out.push(html[body..body + end].to_string());
        i = body + end;
    }
    out
}

/// Text of a subresource (a sheet or a script): success status, 10 MB cap, UTF-8.
async fn fetch_text_p(url: &str, pri: Priority, partition: Option<&str>) -> Result<String> {
    let response = get(url, pri, partition).await.context("Connection failure")?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    if response.content_length().unwrap_or(0) > 10 * 1024 * 1024 {
        anyhow::bail!("Response exceeds size limit");
    }
    let bytes = response.bytes().await.context("Failed to read body")?;
    if bytes.len() > 10 * 1024 * 1024 {
        anyhow::bail!("Response body exceeds size limit");
    }
    String::from_utf8(bytes).context("Invalid UTF-8")
}

/// Fetches raw bytes (images etc), same limits as fetch_document.
pub async fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    fetch_bytes_p(url, Priority::Image, None).await
}

async fn fetch_bytes_p(url: &str, pri: Priority, partition: Option<&str>) -> Result<Vec<u8>> {
    let response = get(url, pri, partition).await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    let bytes = response.bytes().await.context("Failed to read body")?;
    if bytes.len() > 10 * 1024 * 1024 {
        anyhow::bail!("Response body exceeds size limit");
    }
    Ok(bytes)
}

/// Identifies image bytes by magic number. `None` = not an image we decode.
/// SVG is text, so it is recognized by an `<svg` tag near the head of the
/// document (an XML prolog / comment / doctype may precede it).
pub fn sniff_image_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }
    // ICO / CUR: a 2-byte zero reserved field, then type 1 (icon) or 2
    // (cursor), little-endian. Favicons still ship as .ico from plenty of
    // origins, and some serve them as application/octet-stream.
    if bytes.len() >= 6 && bytes.starts_with(b"\x00\x00") && (bytes[2] == 1 || bytes[2] == 2) && bytes[3] == 0 {
        return Some("image/x-icon");
    }
    let head = &bytes[..bytes.len().min(1024)];
    if head.windows(4).any(|w| w.eq_ignore_ascii_case(b"<svg")) {
        return Some("image/svg+xml");
    }
    None
}

/// Fetches image bytes with content sniffing: the `Content-Type` header
/// decides when it is an `image/*` (or uselessly generic), otherwise the
/// magic bytes do. A non-image response is rejected after ~1 KiB rather
/// than buffered whole; the 10 MB body cap matches `fetch_bytes`.
pub async fn fetch_image_bytes(url: &str) -> Result<Vec<u8>> {
    fetch_image_bytes_p(url, None).await
}

async fn fetch_image_bytes_p(url: &str, partition: Option<&str>) -> Result<Vec<u8>> {
    let mut response = get(url, Priority::Image, partition).await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    let ctype = response
        .headers()
        .get("content-type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let header_image = ctype.starts_with("image/");
    // A generic/absent type tells us nothing — fall through to magic bytes.
    let header_useless = ctype.is_empty()
        || ctype == "application/octet-stream"
        || ctype == "binary/octet-stream"
        || ctype == "text/plain";
    if !header_image && !header_useless {
        anyhow::bail!("not an image: {}", ctype);
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut sniffed = header_image;
    while let Some(chunk) = response.chunk().await.context("Failed to read body")? {
        buf.extend_from_slice(&chunk);
        if buf.len() > 10 * 1024 * 1024 {
            anyhow::bail!("Response body exceeds size limit");
        }
        if !sniffed && buf.len() >= 1024 {
            if sniff_image_kind(&buf).is_none() {
                anyhow::bail!("not an image (magic bytes)");
            }
            sniffed = true;
        }
    }
    if !sniffed && sniff_image_kind(&buf).is_none() {
        anyhow::bail!("not an image (magic bytes)");
    }
    Ok(buf)
}

/// Requests in flight at once for a page's subresources (one h2 connection per origin multiplexes them).
const MAX_IN_FLIGHT: usize = 16;
/// Images in flight at once: a skin's icon sprites plus a page's photos run to the hundreds, and firing them
/// all at once earns 429s from upload.wikimedia.org.
const MAX_IMAGES_IN_FLIGHT: usize = 8;
/// @font-face sources fetched per page.
const MAX_FONTS: usize = 32;

/// One queued subresource.
enum Load {
    /// The `i`th stylesheet in document order.
    Sheet(usize, String),
    Script(String),
    Font(String),
    /// (fetch URL, paint-lookup key).
    Image(String, String),
}

impl Load {
    fn priority(&self) -> Priority {
        match self {
            Load::Sheet(..) => Priority::Css,
            Load::Font(_) => Priority::Font,
            Load::Script(_) => Priority::Script,
            Load::Image(..) => Priority::Image,
        }
    }
}

enum Loaded {
    Sheet(usize, String, Result<String>),
    Script(String, Result<String>),
    Font(String, Result<Vec<u8>>),
    Image(String, String, Result<Vec<u8>>),
}

/// The page's subresource scheduler: a priority queue (class, then discovery order), a bound on requests in
/// flight (and a lower one on images), and deduplication by (class, URL) — a script referenced twice is
/// fetched once and fills both slots.
struct Loader {
    queue: std::collections::BinaryHeap<std::cmp::Reverse<(Priority, u64)>>,
    jobs: std::collections::HashMap<u64, Load>,
    seen: std::collections::HashSet<(Priority, String)>,
    seq: u64,
    fonts: usize,
    partition: Option<String>,
    /// AETHERSW (SR104): the worker controlling the document; every subresource is its `fetch` event first.
    sw: Option<crate::workers::sw::SwHandle>,
}

impl Loader {
    fn push(&mut self, load: Load) {
        let url = match &load {
            Load::Sheet(_, u) | Load::Script(u) | Load::Font(u) | Load::Image(u, _) => u.clone(),
        };
        let pri = load.priority();
        if !self.seen.insert((pri, url)) {
            return;
        }
        if pri == Priority::Font {
            if self.fonts >= MAX_FONTS {
                return;
            }
            self.fonts += 1;
        }
        self.seq += 1;
        self.queue.push(std::cmp::Reverse((pri, self.seq)));
        self.jobs.insert(self.seq, load);
    }

    /// The next load allowed to start, highest priority first.
    fn next(&mut self, images_in_flight: usize) -> Option<Load> {
        let std::cmp::Reverse((pri, seq)) = *self.queue.peek()?;
        if pri == Priority::Image && images_in_flight >= MAX_IMAGES_IN_FLIGHT {
            return None; // only images remain queued, and enough of them are in flight
        }
        self.queue.pop();
        self.jobs.remove(&seq)
    }
}

type LoadFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Loaded> + Send>>;

fn start(load: Load, partition: Option<String>, sw: Option<crate::workers::sw::SwHandle>) -> LoadFuture {
    Box::pin(async move {
        let p = partition.as_deref();
        if let Some(h) = sw.as_ref() {
            let (url, pri) = match &load {
                Load::Sheet(_, u) => (u.clone(), Priority::Css),
                Load::Script(u) => (u.clone(), Priority::Script),
                Load::Font(u) => (u.clone(), Priority::Font),
                Load::Image(u, _) => (u.clone(), Priority::Image),
            };
            if let Some(r) = sw_subresource(h, &url, pri).await {
                let text = |r: Result<Vec<u8>>| r.and_then(|b| String::from_utf8(b).context("Invalid UTF-8"));
                return match load {
                    Load::Sheet(i, u) => Loaded::Sheet(i, u, text(r)),
                    Load::Script(u) => Loaded::Script(u, text(r)),
                    Load::Font(u) => Loaded::Font(u, r),
                    Load::Image(u, key) => Loaded::Image(u, key, r),
                };
            }
        }
        match load {
            Load::Sheet(i, u) => {
                let r = fetch_text_p(&u, Priority::Css, p).await;
                Loaded::Sheet(i, u, r)
            }
            Load::Script(u) => {
                let r = fetch_text_p(&u, Priority::Script, p).await;
                Loaded::Script(u, r)
            }
            Load::Font(u) => {
                let r = fetch_bytes_p(&u, Priority::Font, p).await;
                Loaded::Font(u, r)
            }
            Load::Image(u, key) => {
                let r = fetch_image_bytes_p(&u, p).await;
                Loaded::Image(u, key, r)
            }
        }
    })
}

/// Fetches a page, its external stylesheets, fonts, scripts and images: the async half of the
/// fetch-then-apply pattern. Subresources go through the [`Loader`]'s priority queue; what a sheet references
/// (its fonts, its images) is queued as the sheet arrives. Resource failures are skipped (and ledgered),
/// never fatal to the page load.
pub async fn fetch_page(input: &str) -> Result<Page> {
    use futures_util::StreamExt as _;
    // A new page: the memory tier starts empty (the disk tier still answers).
    http_cache().clear_memory();
    // The page's base is the FINAL response URL (after redirects): relative stylesheet, script,
    // image and link URLs resolve against it, and it is the history entry the address bar shows.
    let (html, base) = fetch_document_final(input).await?;
    let partition = http::site_of(&base);
    // AETHERSW (SR104): the document's controller (decided at navigation, as `workers::sw` decides a client's).
    let sw = {
        let b = base.clone();
        blocking(move || crate::workers::sw::navigation_controller(&b).map(|(h, _)| h)).await.flatten()
    };
    let mut loader = Loader {
        queue: std::collections::BinaryHeap::new(),
        jobs: std::collections::HashMap::new(),
        seen: std::collections::HashSet::new(),
        seq: 0,
        fonts: 0,
        partition: partition.clone(),
        sw,
    };
    // AETHERDEBT SR93: the document's CSP (headers + <meta>) and mixed content gate what the load
    // itself fetches — style-src-elem for the links (SRI on arrival), font-src, img-src.
    let policy = crate::security::LoadPolicy::for_document(&base, &html);
    // Discovery from the document itself, in priority classes.
    let sheet_links: Vec<SheetLink> = collect_stylesheet_links(&base, &html)
        .into_iter()
        .filter_map(|mut l| {
            l.url = policy.stylesheet(&l.url, &l.nonce)?;
            Some(l)
        })
        .collect();
    let mut sheet_texts: Vec<Option<String>> = vec![None; sheet_links.len()];
    for (i, l) in sheet_links.iter().enumerate() {
        loader.push(Load::Sheet(i, l.url.clone()));
    }
    for css in inline_style_blocks(&html) {
        for u in crate::fonts::webfont::font_urls(&css, &base) {
            if let Some(u) = policy.subresource(&u, "font") {
                loader.push(Load::Font(u));
            }
        }
    }
    let (mut slots, external) = collect_scripts(&base, &html);
    let mut script_slots: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (slot, u) in external {
        script_slots.entry(u.clone()).or_default().push(slot);
        loader.push(Load::Script(u));
    }
    // <img> srcs plus CSS background images. Each css ref carries a fetch
    // URL (relative to its OWNING sheet's host) and a paint-lookup key
    // (page-base resolution, which is what images::get computes at paint);
    // the decoded image is stored under both.
    let mut refs: Vec<(String, String)> = collect_image_urls(&base, &html).into_iter().map(|u| (u.clone(), u)).collect();
    collect_css_image_refs(&base, &base, &html, &mut refs);
    for (u, key) in &refs {
        if let Some(fu) = policy.subresource(u, "image") {
            loader.push(Load::Image(fu, key.clone()));
        }
    }

    let mut images = Vec::new();
    let mut in_flight = futures_util::stream::FuturesUnordered::<LoadFuture>::new();
    let mut images_in_flight = 0usize;
    loop {
        while in_flight.len() < MAX_IN_FLIGHT {
            let Some(load) = loader.next(images_in_flight) else { break };
            if matches!(load, Load::Image(..)) {
                images_in_flight += 1;
            }
            in_flight.push(start(load, loader.partition.clone(), loader.sw.clone()));
        }
        let Some(done) = in_flight.next().await else { break };
        match done {
            Loaded::Sheet(i, sheet_url, Ok(css)) => {
                // SRI (`<link integrity>`): a sheet whose digest does not match is not applied.
                if let Some(integrity) = sheet_links.get(i).and_then(|l| l.integrity.clone()) {
                    let same = crate::security::origin::Origin::of(&base).same_origin(&crate::security::origin::Origin::of(&sheet_url));
                    let eligible = same || sheet_links.get(i).is_some_and(|l| l.crossorigin);
                    if !policy.stylesheet_integrity(&sheet_url, &integrity, &css, eligible) {
                        continue;
                    }
                }
                // What the sheet references joins the queue now: its fonts, its images.
                for u in crate::fonts::webfont::font_urls(&css, &sheet_url) {
                    if let Some(u) = policy.subresource(&u, "font") {
                        loader.push(Load::Font(u));
                    }
                }
                let before = refs.len();
                collect_css_image_refs(&sheet_url, &base, &css, &mut refs);
                for (u, key) in refs[before..].to_vec() {
                    if let Some(fu) = policy.subresource(&u, "image") {
                        loader.push(Load::Image(fu, key));
                    }
                }
                sheet_texts[i] = Some(css);
            }
            Loaded::Sheet(_, sheet_url, Err(_)) => crate::ledger::record_css(&format!("stylesheet-fetch-failed:{}", sheet_url)),
            Loaded::Script(url, result) => {
                let slot_list = script_slots.remove(&url).unwrap_or_default();
                match result {
                    Ok(text) if text.len() <= 2 * 1024 * 1024 => {
                        for slot in slot_list {
                            slots[slot].text = Some(text.clone());
                        }
                    }
                    Ok(_) => crate::ledger::record_js(&format!("script-too-large:{}", &url[..url.len().min(48)])),
                    Err(_) => crate::ledger::record_js(&format!("script-fetch-failed:{}", &url[..url.len().min(48)])),
                }
            }
            Loaded::Font(url, Ok(bytes)) => crate::fonts::webfont::store_bytes(&url, bytes),
            Loaded::Font(url, Err(e)) => crate::ledger::record_css(&format!("font-fetch-failed:{}: {e}", &url[..url.len().min(48)])),
            Loaded::Image(img_url, key, result) => {
                images_in_flight -= 1;
                // Decode stays on this thread.
                match result {
                    Ok(bytes) => {
                        let decoded = crate::images::decode_raster(&bytes).or_else(|| crate::images::decode_svg(&bytes));
                        match decoded {
                            Some(img) => {
                                if key != img_url {
                                    images.push((key, img.clone()));
                                }
                                images.push((img_url, img));
                            }
                            None => crate::ledger::record_dom(&format!("img-decode-failed:{}", &img_url[..img_url.len().min(48)])),
                        }
                    }
                    Err(e) => crate::ledger::record_dom(&format!("img-fetch-failed:{}: {e}", &img_url[..img_url.len().min(48)])),
                }
            }
        }
    }
    // Sheets in document order (the cascade's order), failures dropped.
    let sheets: Vec<String> = sheet_texts.into_iter().flatten().collect();
    let mut scripts = Vec::new();
    let mut script_nodes = Vec::new();
    let mut script_urls = Vec::new();
    for slot in slots {
        if let Some(text) = slot.text {
            scripts.push(text);
            script_nodes.push(slot.ordinal);
            script_urls.push(slot.url.unwrap_or_default());
        }
    }
    let favicon = favicon_url(&base, &html);
    Ok(Page { base_url: base, html, sheets, images, scripts, script_nodes, script_urls, favicon })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_stylesheet_urls() {
        let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css">
            <link rel="stylesheet" href="https://cdn.example.org/x.css">
            <link rel="icon" href="/favicon.ico">
            <link href="/no-rel.css">
        </head><body></body></html>"#;
        let urls = collect_stylesheet_urls("https://example.com/page/", html);
        assert_eq!(urls, vec![
            "https://example.com/main.css".to_string(),
            "https://cdn.example.org/x.css".to_string(),
        ]);
    }

    #[test]
    fn test_cookie_jar_round_trip() {
        // As a response's Set-Cookie lands in the jar (the HTTP API side of RFC 6265).
        let url = http::Url::parse("https://cookie-test.example/page").unwrap();
        cookie_jar().lock().unwrap().set_cookie(&url, "sid=abc123; Path=/", now_secs(), true);
        let sent = cookies_for("https://cookie-test.example/other");
        assert!(sent.contains("sid=abc123"), "jar sent {sent:?}");
        // Host-scoped: a different host sees nothing.
        assert_eq!(cookies_for("https://other.example/"), "");
        assert_eq!(cookies_for("not a url"), "");
    }

    /// Extensionless / query-bearing url()s: fetched from image-position
    /// properties (MediaWiki icon masks), skipped elsewhere.
    #[test]
    fn test_css_image_refs_extensionless() {
        let css = "\
            .icon { mask-image: url(/w/load.php?modules=x&image=search&format=original) }\
            .b { background-image: url(/icons/search) }\
            @font-face { src: url(/fonts/x.php?family=a) }\
            .c { cursor: url(/cur/hand) }\
            .d { background: url(/style/theme.css) }\
            .e { mask-image: url(#inline-mask) }";
        let mut refs = Vec::new();
        collect_css_image_refs("https://example.com/", "https://example.com/", css, &mut refs);
        let got: Vec<&str> = refs.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(got, vec![
            "https://example.com/w/load.php?modules=x&image=search&format=original",
            "https://example.com/icons/search",
        ]);
    }

    #[test]
    fn test_favicon_url() {
        // First icon link wins, sizes ignored, resolved against the base.
        let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css">
            <link rel="apple-touch-icon" sizes="180x180" href="/touch.png">
            <link rel="shortcut icon" href="icons/fav.ico">
        </head></html>"#;
        assert_eq!(
            favicon_url("https://example.com/page/", html),
            Some("https://example.com/touch.png".to_string())
        );
        // No declared icon: the well-known origin path, query/fragment dropped.
        assert_eq!(
            favicon_url("https://example.com/deep/page?q=1#frag", "<html></html>"),
            Some("https://example.com/favicon.ico".to_string())
        );
        // data: hrefs are skipped — the chrome wants bytes it can fetch.
        assert_eq!(
            favicon_url(
                "https://example.com/",
                r#"<link rel="icon" href="data:image/png;base64,AAAA">"#
            ),
            Some("https://example.com/favicon.ico".to_string())
        );
        assert_eq!(favicon_url("not a url", "<html></html>"), None);
    }

    /// The `image` crate's **ico** decoder must be compiled in: `.ico` is
    /// still what most origins serve at `/favicon.ico`, and a missing decoder
    /// would fail silently (`fetch_favicon` returns `None` on any decode
    /// error). Builds a real single-entry ICO wrapping a PNG and decodes it
    /// through the same `image::load_from_memory` call the favicon path uses.
    #[test]
    fn test_ico_decoder_is_available() {
        use image::ImageEncoder as _;
        let mut png = Vec::new();
        let pixel = [0u8, 0, 255, 255];
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&pixel, 1, 1, image::ExtendedColorType::Rgba8)
            .expect("encode png");

        let mut ico = Vec::new();
        ico.extend_from_slice(&[0, 0]); // reserved
        ico.extend_from_slice(&[1, 0]); // type: icon
        ico.extend_from_slice(&[1, 0]); // one entry
        ico.extend_from_slice(&[1, 1, 0, 0]); // 1x1, no palette, reserved
        ico.extend_from_slice(&[1, 0]); // color planes
        ico.extend_from_slice(&[32, 0]); // bits per pixel
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes()); // offset past the header
        ico.extend_from_slice(&png);

        assert_eq!(sniff_image_kind(&ico), Some("image/x-icon"));
        let decoded = image::load_from_memory(&ico)
            .expect("image crate must decode ICO (default `ico` feature)")
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (1, 1));
        assert_eq!(decoded.into_raw(), pixel);
    }

    #[test]
    fn test_sniff_image_kind() {
        assert_eq!(sniff_image_kind(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff_image_kind(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(sniff_image_kind(b"GIF89a..."), Some("image/gif"));
        assert_eq!(sniff_image_kind(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image_kind(b"\x00\x00\x01\x00\x01\x00"), Some("image/x-icon"));
        assert_eq!(
            sniff_image_kind(b"<?xml version=\"1.0\"?><!-- c --><svg xmlns=\"...\"/>"),
            Some("image/svg+xml")
        );
        assert_eq!(sniff_image_kind(b"<!doctype html><html>"), None);
        assert_eq!(sniff_image_kind(b"{\"error\":\"nope\"}"), None);
    }

    /// AETHERCACHE (SR88): the loader starts CSS before fonts before scripts before images, whatever the
    /// discovery order, dedups by (class, URL), and holds images to their own in-flight bound.
    #[test]
    fn test_loader_priority_order_and_dedup() {
        let mut l = Loader {
            queue: std::collections::BinaryHeap::new(),
            jobs: std::collections::HashMap::new(),
            seen: std::collections::HashSet::new(),
            seq: 0,
            fonts: 0,
            partition: None,
            sw: None,
        };
        l.push(Load::Image("https://a.example/1.png".into(), "k".into()));
        l.push(Load::Script("https://a.example/a.js".into()));
        l.push(Load::Script("https://a.example/a.js".into()));
        l.push(Load::Font("https://a.example/f.woff2".into()));
        l.push(Load::Sheet(0, "https://a.example/s.css".into()));
        l.push(Load::Image("https://a.example/2.png".into(), "k2".into()));
        let mut order = Vec::new();
        while let Some(load) = l.next(0) {
            order.push(load.priority());
        }
        assert_eq!(order, vec![Priority::Css, Priority::Font, Priority::Script, Priority::Image, Priority::Image]);
        l.push(Load::Image("https://a.example/3.png".into(), "k3".into()));
        assert!(l.next(MAX_IMAGES_IN_FLIGHT).is_none(), "images wait for a free image slot");
        assert!(l.next(0).is_some());
        assert_eq!(Priority::Document.header(), "u=0");
    }

    #[test]
    fn test_normalize_url() {
        assert_eq!(normalize_url("google.com"), vec!["https://google.com", "http://google.com"]);
        assert_eq!(normalize_url("https://example.com"), vec!["https://example.com"]);
        assert_eq!(normalize_url("http://localhost"), vec!["http://localhost"]);
        assert!(normalize_url("file:///etc/passwd").is_empty());
    }
}
