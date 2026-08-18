use anyhow::{Context, Result};
use reqwest::Client;
use reqwest::cookie::{CookieStore, Jar};
use std::sync::Arc;

/// Process-wide, in-memory cookie jar (session-scoped: nothing is written
/// to disk, so quitting Aether logs you out). Shared by every client we
/// build, so a Set-Cookie collected on one navigation is sent on the next
/// and on subresource/script fetches to the same host.
pub fn cookie_jar() -> Arc<Jar> {
    static JAR: std::sync::OnceLock<Arc<Jar>> = std::sync::OnceLock::new();
    JAR.get_or_init(|| Arc::new(Jar::default())).clone()
}

/// Cookies the jar would send to `url`, formatted as a `Cookie:` header
/// value (`"a=b; c=d"`), or `""` when the jar has none for that host.
/// Read API for `document.cookie` (JS lane wires it up).
pub fn cookies_for(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else { return String::new() };
    cookie_jar()
        .cookies(&parsed)
        .and_then(|v| v.to_str().ok().map(|s| s.to_string()))
        .unwrap_or_default()
}

/// Write API for `document.cookie`: adds one `"name=value; attrs"` string to
/// the shared jar, scoped to `url`. Malformed urls are dropped silently, as
/// a browser drops a cookie it cannot scope.
pub fn set_cookie(url: &str, cookie_str: &str) {
    if let Ok(parsed) = url::Url::parse(url) {
        cookie_jar().add_cookie_str(cookie_str, &parsed);
    }
}

/// Blocking client builder for the sync JS paths (`fetch`/XHR run on their
/// own thread, off the tokio runtime). Pre-wired to the shared jar so those
/// requests carry — and record — the same cookies as the page load.
pub fn blocking_client_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder()
        .user_agent("UnaOS Aether/0.1.0")
        .cookie_provider(cookie_jar())
}

/// One shared HTTP client: connection pooling + TLS session reuse across
/// every fetch of a page load (a per-fetch Client paid a fresh handshake
/// for each of yahoo's dozens of resources).
fn client() -> &'static Client {
    static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent("UnaOS Aether/0.1.0")
            .cookie_provider(cookie_jar())
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
    if input.starts_with("file://") {
        anyhow::bail!("file:// fetches are disabled for security reasons.");
    }
    let urls = normalize_url(input);
    if urls.is_empty() {
        anyhow::bail!("Invalid URL");
    }

    let client = client();
    let mut last_err = None;
    for url in urls {
        match client.get(&url).send().await {
            Ok(response) => {
                if !response.status().is_success() {
                    return Err(anyhow::anyhow!("HTTP Error: {}", response.status()));
                }
                
                if response.content_length().unwrap_or(0) > 10 * 1024 * 1024 {
                    anyhow::bail!("Response exceeds size limit");
                }
                
                let bytes = response.bytes().await.context("Failed to read body")?;
                if bytes.len() > 10 * 1024 * 1024 {
                    anyhow::bail!("Response body exceeds size limit");
                }
                
                let text = String::from_utf8(bytes.to_vec()).context("Invalid UTF-8")?;
                return Ok(text);
            }
            Err(e) => {
                last_err = Some(anyhow::anyhow!(e).context("Connection failure"));
            }
        }
    }
    
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Failed to fetch URL")))
}

/// POSTs a urlencoded form body and returns the response document.
pub async fn post_document(url: &str, body: &str) -> Result<String> {
    let response = client()
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body.to_string())
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    let bytes = response.bytes().await.context("Failed to read body")?;
    if bytes.len() > 10 * 1024 * 1024 {
        anyhow::bail!("Response body exceeds size limit");
    }
    String::from_utf8(bytes.to_vec()).context("Invalid UTF-8")
}

/// Collects absolute URLs of `<link rel="stylesheet">` sheets in `html`,
/// resolved against `base_url`. Pure (no network) — unit-testable offline.
pub fn collect_stylesheet_urls(base_url: &str, html: &str) -> Vec<String> {
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
                    out.push(u);
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
                    external.push((slots.len(), abs));
                    slots.push(ScriptSlot { ordinal, text: None });
                }
                continue;
            }
            drop(attrs);
            let text = script.as_node().text_contents();
            if !text.trim().is_empty() {
                slots.push(ScriptSlot { ordinal, text: Some(text) });
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
    origin.set_path("/favicon.ico");
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
/// ICO is handled by the `image` crate's ico decoder (it picks the largest
/// contained image); PNG/JPEG/GIF/WebP/BMP go through the same decoder, and an
/// SVG icon falls back to the rasterizer the page images use.
pub async fn fetch_favicon(url: &str) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = fetch_image_bytes(url).await.ok()?;
    let decoded = image::load_from_memory(&bytes)
        .ok()
        .map(|i| i.to_rgba8())
        .or_else(|| crate::images::decode_svg(&bytes))?;
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
            let attrs = img.attributes.borrow();
            let Some(src) = crate::images::effective_img_src(&attrs) else { continue };
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

/// Fetches raw bytes (images etc), same limits as fetch_document.
pub async fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    let response = client().get(url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    let bytes = response.bytes().await.context("Failed to read body")?;
    if bytes.len() > 10 * 1024 * 1024 {
        anyhow::bail!("Response body exceeds size limit");
    }
    Ok(bytes.to_vec())
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
    let mut response = client().get(url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP Error: {}", response.status());
    }
    let ctype = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
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

/// Fetches a page, its external stylesheets, and its images: the async
/// half of the fetch-then-apply pattern. Resource failures are skipped
/// (and ledgered), never fatal to the page load.
pub async fn fetch_page(input: &str) -> Result<Page> {
    let html = fetch_document(input).await?;
    let base = normalize_url(input)
        .into_iter()
        .next()
        .unwrap_or_else(|| input.to_string());
    // All stylesheets fetch CONCURRENTLY (a page with a dozen sheets paid
    // a dozen round-trips in series before this).
    let sheet_results = futures_util::future::join_all(
        collect_stylesheet_urls(&base, &html)
            .into_iter()
            .map(|u| async move { (fetch_document(&u).await, u) }),
    )
    .await;
    let mut sheets = Vec::new();
    let mut sheet_sources: Vec<(String, String)> = Vec::new(); // (sheet url, css)
    for (result, sheet_url) in sheet_results {
        match result {
            Ok(css) => {
                sheets.push(css.clone());
                sheet_sources.push((sheet_url, css));
            }
            Err(_) => crate::ledger::record_css(&format!("stylesheet-fetch-failed:{}", sheet_url)),
        }
    }
    // <img> srcs plus CSS background images. Each css ref carries a fetch
    // URL (relative to its OWNING sheet's host) and a paint-lookup key
    // (page-base resolution, which is what images::get computes at paint);
    // the decoded image is stored under both.
    let mut refs: Vec<(String, String)> = collect_image_urls(&base, &html)
        .into_iter()
        .map(|u| (u.clone(), u))
        .collect();
    collect_css_image_refs(&base, &base, &html, &mut refs);
    for (sheet_url, css) in &sheet_sources {
        collect_css_image_refs(sheet_url, &base, css, &mut refs);
    }
    // Images fetch concurrently but BOUNDED: a skin's icon sprites plus a
    // page's photos run to the hundreds, and firing them all at once earns
    // 429s from upload.wikimedia.org. Decode stays on this thread.
    use futures_util::StreamExt as _;
    let img_results: Vec<_> = futures_util::stream::iter(
        refs
            .into_iter()
            .map(|(u, key)| async move { (fetch_image_bytes(&u).await, u, key) }),
    )
    .buffer_unordered(8)
    .collect()
    .await;
    let mut images = Vec::new();
    for (result, img_url, key) in img_results {
        match result {
            Ok(bytes) => {
                let decoded = image::load_from_memory(&bytes)
                    .ok()
                    .map(|i| i.to_rgba8())
                    .or_else(|| crate::images::decode_svg(&bytes));
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
    // Scripts: inline bodies fill their slots directly; externals fetch
    // concurrently into theirs (2 MB/script cap — failures ledger and the
    // slot drops, later scripts still run).
    let (mut slots, external) = collect_scripts(&base, &html);
    let script_results = futures_util::future::join_all(
        external
            .into_iter()
            .map(|(slot, u)| async move { (slot, fetch_document(&u).await, u) }),
    )
    .await;
    for (slot, result, url) in script_results {
        match result {
            Ok(text) if text.len() <= 2 * 1024 * 1024 => slots[slot].text = Some(text),
            Ok(_) => crate::ledger::record_js(&format!("script-too-large:{}", &url[..url.len().min(48)])),
            Err(_) => crate::ledger::record_js(&format!("script-fetch-failed:{}", &url[..url.len().min(48)])),
        }
    }
    let mut scripts = Vec::new();
    let mut script_nodes = Vec::new();
    for slot in slots {
        if let Some(text) = slot.text {
            scripts.push(text);
            script_nodes.push(slot.ordinal);
        }
    }
    let favicon = favicon_url(&base, &html);
    Ok(Page { base_url: base, html, sheets, images, scripts, script_nodes, favicon })
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
        let url = url::Url::parse("https://cookie-test.example/page").unwrap();
        cookie_jar().add_cookie_str("sid=abc123; Path=/", &url);
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

    #[test]
    fn test_normalize_url() {
        assert_eq!(normalize_url("google.com"), vec!["https://google.com", "http://google.com"]);
        assert_eq!(normalize_url("https://example.com"), vec!["https://example.com"]);
        assert_eq!(normalize_url("http://localhost"), vec!["http://localhost"]);
        assert!(normalize_url("file:///etc/passwd").is_empty());
    }
}
