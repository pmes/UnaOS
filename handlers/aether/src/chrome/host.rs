//! The chrome's std adapter: where its files live and the network half of downloads. Everything here
//! is I/O over the std-free model next to it (bookmarks, downloads); nothing here decides anything a
//! model function could.
//!
//! - Bookmarks are `<data dir>/bookmarks.tsv`, the data dir being the one Web Storage's
//!   `LocalStorage` already uses (`directories::ProjectDirs::from("", "UnaOS", "Aether")`).
//! - Downloads land in the person's Downloads folder (`directories::UserDirs::download_dir`, else
//!   `~/Downloads`), written to `<name>.part` and renamed when complete — a half-written file never
//!   carries the real name. A save (PDF4FORMSAVE, SR354: [`save_bytes`], the bytes an engine hands over as
//!   `BrowserSave`) lands the same way through the same two steps, [`claim`] and [`land`]. SAVEDIALOG (SR364): the
//!   write itself (`.part`, then rename) is gneiss_pal's ONE writer (`gneiss_pal::fs`), Facet's export's too; a Save As
//!   ([`save_bytes_at`]) writes it at the path the person chose in the face's save panel. PAGESAVECOMPLETE (SR398): an HTML
//!   tab's Save As chosen as `.html` lands "Webpage, Complete" (a `<name>_files/` folder beside it), as `.mhtml` the archive;
//!   [`open_archive`] opens a `.mhtml` file URL. PAGESAVECSSURLS (SR407): the `url()` / `@import` references inside CSS
//!   are found and, in a Complete save, rewritten through one walk over css_core's tokenizer ([`css_urls_walk`]).
//! - [`probe`] is the top-level navigation's download decision: a `HEAD` of the URL over gneiss_pal's
//!   HTTP client (the same cookie jar the page loads use), whose `Content-Disposition` /
//!   `Content-Type` the model judges ([`super::downloads`]). A failed or refused probe answers
//!   "document" and the load proceeds exactly as before.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gneiss_pal::api::http::Client;

use super::bookmarks::Bookmarks;
use super::downloads::{self, Download};

/// Aether's per-user data directory.
pub fn data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "UnaOS", "Aether").map(|p| p.data_dir().to_path_buf())
}

/// The bookmarks file.
pub fn bookmarks_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("bookmarks.tsv"))
}

/// The person's bookmarks (empty when there is no file yet, or it cannot be read).
pub fn load_bookmarks() -> Bookmarks {
    bookmarks_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| Bookmarks::parse(&t))
        .unwrap_or_default()
}

/// Write the bookmarks file atomically through gneiss_pal's ONE writer (WRITEONE, SR386: `bookmarks.tsv.part`, then rename).
pub fn save_bookmarks(b: &Bookmarks) -> std::io::Result<()> {
    let path = bookmarks_path().ok_or_else(|| std::io::Error::other("no data directory"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    gneiss_pal::fs::save_bytes(&path, b.serialize().as_bytes())
}

/// The Downloads folder.
pub fn downloads_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(|d| d.to_path_buf()))
        .or_else(|| directories::BaseDirs::new().map(|b| b.home_dir().join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

fn probe_client() -> &'static Client {
    static C: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        Client::builder()
            .user_agent(crate::js::window::USER_AGENT)
            .cookie_jar(crate::net::cookie_jar())
            .timeout(Duration::from_secs(15))
            .build()
            .expect("http client")
    })
}

fn download_client() -> &'static Client {
    static C: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        Client::builder()
            .user_agent(crate::js::window::USER_AGENT)
            .cookie_jar(crate::net::cookie_jar())
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("http client")
    })
}

/// What a top-level navigation's response is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Render it (or the probe could not tell — the load decides).
    Document,
    /// Save it: the final URL (after redirects), the suggested name, the length when known.
    Download { url: String, name: String, total: Option<u64> },
}

/// `HEAD` the URL and judge the answer. Only http(s) is probed (a scheme-less input is probed as the
/// https URL the fetcher tries first).
pub async fn probe(input: &str) -> Probe {
    let Some(url) = crate::net::normalize_url(input).into_iter().next() else { return Probe::Document };
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Probe::Document;
    }
    let Ok(resp) = probe_client().request("HEAD", &url).send().await else { return Probe::Document };
    if !resp.status().is_success() {
        return Probe::Document;
    }
    let cd = resp.headers().get("content-disposition");
    let ct = resp.headers().get("content-type");
    if downloads::is_attachment(cd) || !downloads::renderable(ct) {
        let final_url = resp.url().to_string();
        Probe::Download { name: downloads::file_name(cd, &final_url), total: resp.content_length(), url: final_url }
    } else {
        Probe::Document
    }
}

/// Download `url` into the Downloads folder as `name` (made unique there), calling `report` with the
/// row as it starts, every 256 KiB, and once finished (done, or failed with the reason). Returns the
/// final row.
pub async fn download(id: u64, url: String, name: String, total: Option<u64>, report: impl Fn(&Download)) -> Download {
    run_download(id, url, name, total, None, report).await
}

/// The cancel flags of the downloads each tab started (tab id → (download id, flag)).
fn tab_downloads() -> &'static std::sync::Mutex<std::collections::HashMap<u64, Vec<(u64, std::sync::Arc<std::sync::atomic::AtomicBool>)>>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<u64, Vec<(u64, std::sync::Arc<std::sync::atomic::AtomicBool>)>>>> =
        std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// [`download`] on behalf of tab `tab` (AETHERDEBT SR93): the download stops — its `.part` removed,
/// the row finished with the error "cancelled: the tab was closed" — when [`cancel_tab_downloads`]
/// is called for that tab.
pub async fn download_in_tab(tab: u64, id: u64, url: String, name: String, total: Option<u64>, report: impl Fn(&Download)) -> Download {
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Ok(mut m) = tab_downloads().lock() {
        m.entry(tab).or_default().push((id, flag.clone()));
    }
    let d = run_download(id, url, name, total, Some(flag), report).await;
    if let Ok(mut m) = tab_downloads().lock() {
        if let Some(v) = m.get_mut(&tab) {
            v.retain(|(i, _)| *i != id);
            if v.is_empty() {
                m.remove(&tab);
            }
        }
    }
    d
}

/// Tab `tab` closed: every download it started and that is still running stops at its next chunk.
pub fn cancel_tab_downloads(tab: u64) {
    if let Ok(mut m) = tab_downloads().lock() {
        if let Some(v) = m.remove(&tab) {
            for (_, f) in v {
                f.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

async fn run_download(
    id: u64,
    url: String,
    name: String,
    total: Option<u64>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    report: impl Fn(&Download),
) -> Download {
    let cancelled = || cancel.as_ref().is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed));
    let dir = downloads_dir();
    let mut d = Download::new(id, &name, &url, total);
    match claim(&dir, &name) {
        Ok(n) => d.name = n,
        Err(e) => {
            d.done = true;
            d.error = Some(e.to_string());
            report(&d);
            return d;
        }
    }
    report(&d);
    let part = dir.join(format!("{}.part", d.name));
    let result: Result<(), String> = async {
        let mut resp = download_client().get(&url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        if d.total.is_none() {
            d.total = resp.content_length();
        }
        let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut reported = 0u64;
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            if cancelled() {
                return Err("cancelled: the tab was closed".to_string());
            }
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            d.received += chunk.len() as u64;
            if d.received - reported >= 256 * 1024 {
                reported = d.received;
                report(&d);
            }
        }
        file.flush().map_err(|e| e.to_string())?;
        drop(file);
        land(&dir, &d.name).map(|_| ()).map_err(|e| e.to_string())
    }
    .await;
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        d.error = Some(e);
    }
    d.done = true;
    report(&d);
    d
}

/// PDF4FORMSAVE (SR354): the first of the ONE pair of steps by which a file lands in the Downloads folder `dir`
/// (a download streams into `<name>.part` between them; [`save_bytes_in`] writes it whole): the folder exists and
/// `name` is made unique against what is there, a finished file or another landing's `.part`.
fn claim(dir: &Path, name: &str) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    Ok(downloads::unique_name(name, |n| dir.join(n).exists() || dir.join(format!("{n}.part")).exists()))
}

/// PDF4FORMSAVE (SR354): the second step: the complete `<name>.part` takes its real name. Answers the file's path.
fn land(dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    let to = dir.join(name);
    gneiss_pal::fs::land(&to)?; // SAVEDIALOG (SR364): the ONE writer's rename
    Ok(to)
}

/// PDF4FORMSAVE (SR354): write `bytes` into the Downloads folder as `name` (sanitized, made unique: `form (1).pdf`
/// beside an existing `form.pdf`). Answers the path written, or why not. The shell's answer to `BrowserSave`.
pub fn save_bytes(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    save_bytes_in(&downloads_dir(), name, bytes)
}

/// [`save_bytes`] into the folder `dir` (the writer's KAT runs it on a scratch folder).
pub fn save_bytes_in(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let clean = downloads::sanitize(name);
    let name = claim(dir, if clean.is_empty() { "download" } else { &clean }).map_err(|e| e.to_string())?;
    let to = dir.join(&name);
    gneiss_pal::fs::save_bytes(&to, bytes).map(|()| to).map_err(|e| e.to_string())
}

/// SAVEDIALOG (SR364): write `bytes` at `path`, the path the person chose in the face's save panel (no unique suffix: a
/// file already there is replaced, as the panel confirmed), through the same ONE writer. The shell's answer to
/// `SavePanelChose` after a `BrowserSaveAs`.
pub fn save_bytes_at(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let archive = page_archive_for(path, bytes); let bytes = archive.as_deref().unwrap_or(bytes); // PAGESAVE (SR392): a staged page chosen as .mhtml lands as its archive
    gneiss_pal::fs::save_bytes(path, bytes).map(|()| path.to_path_buf()).map_err(|e| e.to_string())
}

/// The page-info popover's answer for `url`: whether the connection was secure, and the sentence that
/// says what that means. HTTPS in Aether is TLS 1.3 through tls_core, whose handshake verifies the
/// server's certificate chain before a byte of the page is read (HTTPCORE, SR51) — so an https page
/// that loaded at all was verified. The chain's subject / issuer / expiry are not reported: gneiss_pal's
/// HTTP client exposes no peer-certificate accessor yet (owed; see the AETHERCHROME landing report).
pub fn page_info(url: &str) -> (bool, String) {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        let host = url.split("://").nth(1).and_then(|r| r.split(['/', '?', '#']).next()).unwrap_or("");
        (
            true,
            format!(
                "Connection is secure. {host} was reached over TLS 1.3 (tls_core) and its certificate chain was verified against the trust store before the page was read. Certificate details are not reported yet."
            ),
        )
    } else if lower.starts_with("http://") {
        (false, "Connection is not secure. This page was sent over plain HTTP: anyone on the network can read or change it.".to_string())
    } else if lower.starts_with("file:") {
        (false, "A local file.".to_string())
    } else if url.is_empty() || lower.starts_with("about:") || lower.starts_with("data:") {
        (false, "An Aether page; nothing was fetched over the network.".to_string())
    } else {
        (false, format!("Fetched over {}.", url.split(':').next().unwrap_or("an unknown scheme")))
    }
}

/// AETHERDEVTOOLS (SR112): one frame of a window whose developer-tools pane may be open — the std half
/// of the pane, the face's single call per tick. The engine's device size follows the page rect of the
/// split (the pane takes its share of the window); while the pane is open the frame runs through the
/// Performance panel's recorder ([`super::devtools::timeline::Timeline::frame`]), otherwise it is the
/// engine's own `tick` + `render_frame`. Returns the paint's damage (None when nothing was painted) and
/// the pane's text lines (None when the pane is closed).
pub fn devtools_frame(
    engine: &mut crate::AetherEngine,
    chrome: &super::DevtoolsChrome,
    tools: &mut super::devtools::DevTools,
    window: (u32, u32),
) -> (Option<Vec<(u32, u32, u32, u32)>>, Option<Vec<String>>) {
    let ((_, _, pw, ph), pane) = chrome.split(window.0, window.1);
    if engine.device_size() != (pw.max(1), ph.max(1)) {
        engine.set_device_size(pw.max(1), ph.max(1));
    }
    if pane.is_none() {
        let damage = engine.tick().then(|| engine.render_frame());
        return (damage, None);
    }
    let damage = tools.timeline.frame(engine);
    let lines = tools.render(engine);
    (damage, Some(lines))
}

// ================================================================================================= PAGESAVE (SR392)
// Save / Save As on an HTML tab (`pdf::save_key` calls `page_save`): the CURRENT DOM through the one serializer (html_core,
// `NodeRef::inner_html` on the Document node: the DOCTYPE then the tree) — the engine keeps no copy of the fetched
// markup, and the DOM is the page as the person sees it, script edits included (a form control's live value that never
// reached an attribute is not markup, as a browser's "HTML only" save). A Save As also stages the page in `STAGED_PAGE`
// so that the path the panel answers decides the format: `.mhtml` / `.mht` lands the archive (`gneiss_pal::mhtml`),
// anything else the HTML itself.

/// A page a Save As staged: its URL, its serialized markup and the resources the cache held for it.
struct StagedPage {
    root_url: String,
    html: String,
    parts: Vec<gneiss_pal::mhtml::Part>,
    /// PAGESAVECOMPLETE (SR398): the http(s) references the cache did not hold, fetched once at save time.
    missing: Vec<String>,
}

/// PAGESAVE (SR392): the one staged page, owned here: written on the tab engine's turn (`page_save`), taken by the
/// Coordinator's `SavePanelChose` answer (`save_bytes_at`). One slot: a newer Save As replaces it, the panel being
/// modal (the Coordinator's `save_as` holds one set of bytes for the same reason).
static STAGED_PAGE: std::sync::Mutex<Option<StagedPage>> = std::sync::Mutex::new(None);

/// PAGESAVE (SR392): what Ctrl/Cmd+S (or, `save_as`, Ctrl/Cmd+Shift+S) on an HTML tab saves: the file name
/// (`<title>.html`) and the current DOM's markup. A Save As stages the page with its resources (gathered now: the
/// page's partition and the cache's memory tier are this page's only until the next load). None without a document.
pub fn page_save(engine: &crate::AetherEngine, save_as: bool) -> Option<(String, Vec<u8>)> {
    let doc = engine.document.as_ref()?;
    let html = doc.inner_html();
    let root_url = engine.current_url();
    let name = page_name(doc, &root_url);
    if save_as {
        let (parts, missing) = page_parts(engine);
        let page = StagedPage { root_url, html: html.clone(), parts, missing };
        *STAGED_PAGE.lock().unwrap_or_else(|e| e.into_inner()) = Some(page);
    }
    Some((name, html.into_bytes()))
}

/// The name a page save suggests: the current DOM's first `<title>`, else the URL's host, else `page`; sanitized as a
/// download's name is; `.html` unless it already ends `.html` / `.htm`.
fn page_name(doc: &crate::dom::NodeRef, url: &str) -> String {
    let title = doc.select("title").ok().and_then(|mut t| t.next()).map(|t| t.as_node().text_contents());
    let title = title.map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty());
    let base = title.unwrap_or_else(|| html_core::url::Url::parse(url).map(|u| u.hostname()).unwrap_or_default());
    let base = downloads::sanitize(&base);
    let base = if base.is_empty() { "page".to_string() } else { base };
    let lower = base.to_ascii_lowercase();
    if lower.ends_with(".html") || lower.ends_with(".htm") { base } else { format!("{base}.html") }
}

/// The page's resources the HTTP cache holds: every reference of the live DOM (`devtools::network::references`, the
/// document itself left out: it is the root part) found through the one lookup (`devtools::network::cached`), as
/// MHTML parts at their absolute URLs. PAGESAVECOMPLETE (SR398): a reference the cache does not hold is answered in the
/// second list (an http(s) one; [`fill_missing`] fetches it at save time), and each nested document of the frame tree is
/// its own `text/html` part ([`frame_parts`]), in place of what the cache holds for its URL. PAGESAVECSSURLS (SR407): the
/// references are a FIFO worklist: the DOM's in tree order, then each `url()` / `@import` of the live DOM's `<style>`
/// texts and `style=` values ([`css_refs`], against the page URL), and each cached stylesheet's own ([`css_refs`] against
/// its URL), each URL once.
fn page_parts(engine: &crate::AetherEngine) -> (Vec<gneiss_pal::mhtml::Part>, Vec<String>) {
    use crate::devtools::network::{cached, references, Initiator};
    let top = engine.current_url();
    let frames = frame_parts(engine);
    let (mut parts, mut missing) = (Vec::new(), Vec::new());
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for (kind, url) in references(engine) {
        if kind == Initiator::Document || frames.iter().any(|f| f.location == url) {
            continue;
        }
        queue.push_back(url);
    }
    let mut seen: Vec<String> = queue.iter().cloned().collect();
    seen.push(top.clone());
    seen.extend(frames.iter().map(|f| f.location.clone()));
    if let Some(doc) = engine.document.as_ref() {
        for u in dom_css_refs(&doc.document_node(), &top) {
            if !seen.contains(&u) {
                seen.push(u.clone());
                queue.push_back(u);
            }
        }
    }
    while let Some(url) = queue.pop_front() {
        match cached(&top, &url) {
            Some(e) => {
                let content_type = e.headers.get("content-type").unwrap_or("application/octet-stream").to_string();
                if is_css(&content_type) {
                    for u in css_refs(&String::from_utf8_lossy(&e.body), &url) {
                        if !seen.contains(&u) {
                            seen.push(u.clone());
                            queue.push_back(u);
                        }
                    }
                }
                parts.push(gneiss_pal::mhtml::Part { location: url, content_type, body: e.body });
            }
            None if is_web(&url) => missing.push(url),
            None => {}
        }
    }
    parts.extend(frames);
    (parts, missing)
}

/// `save_bytes_at`'s format choice: a `.mhtml` / `.mht` path whose bytes are the staged page's markup takes the staged
/// page and answers its archive; PAGESAVECOMPLETE (SR398): a `.html` / `.htm` one writes the `_files` folder and answers
/// the rewritten markup ([`page_output`]); anything else (a PDF's Save As named `x.mhtml`) answers None and the bytes land
/// as they are.
fn page_archive_for(path: &Path, bytes: &[u8]) -> Option<Vec<u8>> {
    page_output(path, bytes, &fetch_once)
}

// ---- PAGESAVECOMPLETE (SR398): Webpage, Complete; the save-time fetch; the frames; opening a .mhtml ------------------

/// PAGESAVECOMPLETE (SR398): a fetch of a reference the cache did not hold: (url, the page's partition site, the time
/// left) -> (its `Content-Type`, its body). [`fetch_once`] in the shell; a stub in the KATs.
pub(crate) type Fetch<'a> = &'a dyn Fn(&str, &str, Duration) -> Option<(String, Vec<u8>)>;

/// PAGESAVECOMPLETE (SR398): the save-time fetch's one budget, for the whole pass (the shell's turn waits on it).
const FETCH_BUDGET: Duration = Duration::from_secs(5);

/// PAGESAVECOMPLETE (SR398): the largest archive [`open_archive`] reads (the HTTP cache's memory tier, where its parts go).
const ARCHIVE_MAX: u64 = 64 << 20;

/// PAGESAVECOMPLETE (SR398): an opened archive's document policy: no script runs and its origin is opaque, so a local file
/// never acts as the site its `Content-Location` names.
const ARCHIVE_CSP: &str = "sandbox; script-src 'none'; object-src 'none'; connect-src 'none'";

/// An http(s) URL?
fn is_web(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// `raw` resolved against `base` exactly as `devtools::network::references` resolves a reference (fragment-less).
fn resolve(base: &str, raw: &str) -> Option<String> {
    let raw = raw.trim();
    (!raw.is_empty()).then_some(())?;
    html_core::url::Url::parse_str_with_base(raw, Some(base)).ok().map(|u| u.serialize(true))
}

/// PAGESAVECOMPLETE (SR398): the staged page's output for `path` (see [`page_archive_for`]), its missing references
/// fetched through `fetch` after the slot is taken (no lock is held across the network).
pub(crate) fn page_output(path: &Path, bytes: &[u8], fetch: Fetch) -> Option<Vec<u8>> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let archive = ext == "mhtml" || ext == "mht";
    if !archive && ext != "html" && ext != "htm" {
        return None;
    }
    let mut page = {
        let mut slot = STAGED_PAGE.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref()?.html.as_bytes() != bytes {
            return None;
        }
        slot.take()?
    };
    fill_missing(&mut page, fetch);
    if archive {
        Some(gneiss_pal::mhtml::write(&page.root_url, &page.html, &page.parts))
    } else {
        Some(write_complete(path, &page).into_bytes())
    }
}

/// PAGESAVECOMPLETE (SR398): each missing reference fetched once under [`FETCH_BUDGET`]; one not fetched is left out,
/// named on the console. PAGESAVECSSURLS (SR407): a worklist: a fetched stylesheet's own `url()` / `@import` references
/// ([`css_refs`]) not yet a part or asked are fetched in the same pass, under the same budget.
fn fill_missing(page: &mut StagedPage, fetch: Fetch) {
    let site = gneiss_pal::api::http::site_of(&page.root_url).unwrap_or_default();
    let started = std::time::Instant::now();
    let mut queue: std::collections::VecDeque<String> = std::mem::take(&mut page.missing).into();
    let mut asked: Vec<String> = queue.iter().cloned().collect();
    while let Some(url) = queue.pop_front() {
        let left = FETCH_BUDGET.saturating_sub(started.elapsed());
        match if left.is_zero() { None } else { fetch(&url, &site, left) } {
            Some((content_type, body)) => {
                if is_css(&content_type) {
                    for u in css_refs(&String::from_utf8_lossy(&body), &url) {
                        if is_web(&u) && u != page.root_url && !asked.contains(&u) && !page.parts.iter().any(|p| p.location == u) {
                            asked.push(u.clone());
                            queue.push_back(u);
                        }
                    }
                }
                page.parts.push(gneiss_pal::mhtml::Part { location: url, content_type, body });
            }
            None => crate::js::host::console_out(1, &format!("[pdf] page save: {url} not cached")),
        }
    }
}

/// PAGESAVECOMPLETE (SR398): the shell's [`Fetch`]: one GET through the one client builder (`net::blocking_client_builder`:
/// the jar and the one HTTP cache), in the page's partition, bounded by the time left; a 2xx body of at most 10 MB.
fn fetch_once(url: &str, site: &str, left: Duration) -> Option<(String, Vec<u8>)> {
    let client = crate::net::blocking_client_builder().timeout(left).build().ok()?;
    let mut rb = client.get(url);
    if !site.is_empty() {
        rb = rb.partition(site);
    }
    let resp = rb.send().ok()?;
    if !resp.status().is_success() || resp.content_length().unwrap_or(0) > 10 << 20 {
        return None;
    }
    let content_type = resp.headers().get("content-type").unwrap_or("application/octet-stream").to_string();
    let body = resp.bytes().ok()?;
    (body.len() <= 10 << 20).then_some((content_type, body))
}

/// PAGESAVECOMPLETE (SR398): each nested document of the frame tree (`frames::all`, an http(s) context) as a `text/html`
/// part: its live document through the one serializer, at the URL its container's `src` names (resolved against the
/// parent context's URL as a reference is), else its own URL. Its own resources are not gathered (PAGE-SAVE-FRAMES-DEEP).
fn frame_parts(engine: &crate::AetherEngine) -> Vec<gneiss_pal::mhtml::Part> {
    let Some(doc) = engine.document.as_ref() else { return Vec::new() };
    let mut out: Vec<gneiss_pal::mhtml::Part> = Vec::new();
    for c in crate::frames::all() {
        let Some(parent) = c.parent else { continue };
        if !is_web(&c.url) {
            continue;
        }
        let parent_url = crate::frames::get(parent).map(|p| p.url).unwrap_or_else(|| engine.current_url());
        let src = c.container.and_then(|id| doc.node_at(html_core::NodeId(id)).as_element()).and_then(|e| {
            let attrs = e.attributes.borrow();
            let src = attrs.get("src").map(str::to_string);
            src
        });
        let location = src.and_then(|s| resolve(&parent_url, &s)).unwrap_or_else(|| c.url.clone());
        if out.iter().any(|p| p.location == location) {
            continue;
        }
        let body = doc.node_at(html_core::NodeId(c.document)).inner_html().into_bytes();
        out.push(gneiss_pal::mhtml::Part { location, content_type: "text/html; charset=utf-8".to_string(), body });
    }
    out
}

/// A media type's essence, lower-cased.
fn essence(content_type: &str) -> String {
    content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// An HTML part (a frame's document)?
fn is_html(content_type: &str) -> bool {
    matches!(essence(content_type).as_str(), "text/html" | "application/xhtml+xml")
}

/// The file extension a `_files` entry without one takes from its media type.
fn ext_for(content_type: &str) -> Option<&'static str> {
    Some(match essence(content_type).as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/svg+xml" => "svg",
        "text/css" => "css",
        "text/javascript" | "application/javascript" => "js",
        "font/woff2" => "woff2",
        "font/woff" => "woff",
        _ => return None,
    })
}

/// One path segment for an attribute: every byte outside RFC 3986's unreserved set and the sub-delims a `srcset`
/// cannot confuse (`,` is escaped) as `%XX`.
fn seg_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!$&'()*+;=@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// PAGESAVECOMPLETE (SR398): "Webpage, Complete": every part written into `<stem>_files/` beside `path` (named by the
/// downloads rule, `downloads::file_name`, with an extension from its media type when it has none, `.html` for a
/// document, unique in the folder; an HTML part rewritten as the page is), then the page's markup through
/// [`rewrite_saved`] with the folder as the prefix. A part that could not be written stays at its absolute URL.
/// PAGESAVECSSURLS (SR407): a stylesheet part's `url()` / `@import` references through [`rewrite_css_urls`] (siblings in
/// the folder), and one console line per save counting the CSS references and those rewritten to a saved file.
fn write_complete(path: &Path, page: &StagedPage) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "page".to_string());
    let folder_name = format!("{stem}_files");
    let folder = path.with_file_name(&folder_name);
    let mut taken: Vec<String> = Vec::new();
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for p in &page.parts {
        let mut name = downloads::file_name(None, &p.location);
        let lower = name.to_ascii_lowercase();
        if is_html(&p.content_type) {
            if !lower.ends_with(".html") && !lower.ends_with(".htm") {
                name.push_str(".html");
            }
        } else if !name.contains('.') {
            if let Some(ext) = ext_for(&p.content_type) {
                name = format!("{name}.{ext}");
            }
        }
        let name = downloads::unique_name(&name, |n| taken.contains(&n.to_ascii_lowercase()));
        taken.push(name.to_ascii_lowercase());
        names.insert(p.location.clone(), name);
    }
    let mut saved: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut tally = CssTally::default();
    if !page.parts.is_empty() && std::fs::create_dir_all(&folder).is_ok() {
        for p in &page.parts {
            let Some(name) = names.get(&p.location) else { continue };
            let body = if is_html(&p.content_type) {
                rewrite_saved(&String::from_utf8_lossy(&p.body), &p.location, &names, "", &mut tally).into_bytes()
            } else if is_css(&p.content_type) {
                let css = std::str::from_utf8(&p.body).ok().and_then(|c| rewrite_css_urls(c, &p.location, &names, "", &mut tally));
                css.map(String::into_bytes).unwrap_or_else(|| p.body.clone())
            } else {
                p.body.clone()
            };
            match gneiss_pal::fs::save_bytes(&folder.join(name), &body) {
                Ok(()) => {
                    saved.insert(p.location.clone(), name.clone());
                }
                Err(e) => crate::js::host::console_out(1, &format!("[pdf] page save: {} not written: {e}", p.location)),
            }
        }
    }
    let out = rewrite_saved(&page.html, &page.root_url, &saved, &format!("{}/", seg_encode(&folder_name)), &mut tally);
    crate::js::host::console_out(0, &format!("[pdf] page save: css urls={} rewritten={}", tally.urls, tally.rewritten));
    out
}

/// The attributes the rewrite reads (the URL-valued ones `devtools::network::references` follows, and `href` / `action`
/// links); `srcset` is a candidate list.
const URL_ATTRS: [&str; 6] = ["src", "href", "poster", "data", "action", "srcset"];

/// PAGESAVECOMPLETE (SR398): the ONE rewrite pass over the serializer's output: a detached parse (`dom::parse_html`; the
/// live DOM is untouched), each URL attribute resolved against `base` as a reference is: one in `saved` (URL -> file name)
/// becomes `prefix` + the name (its fragment kept), any other RELATIVE value (no scheme, not `#…`) its absolute URL;
/// `<base>` dropped; serialized again by the one serializer. PAGESAVECSSURLS (SR407): each `style=` value and each
/// `<style>` element's text through [`rewrite_css_urls`] (the same map and prefix), counted in `tally`.
fn rewrite_saved(html: &str, base: &str, saved: &std::collections::HashMap<String, String>, prefix: &str, tally: &mut CssTally) -> String {
    let doc = crate::dom::parse_html(html);
    let mut bases = Vec::new();
    let mut styles = Vec::new();
    for n in doc.descendants() {
        let Some(e) = n.as_element() else { continue };
        if e.name.local.0 == "base" {
            bases.push(n.clone());
            continue;
        }
        if e.name.local.0 == "style" {
            styles.push(n.clone());
        }
        let mut attrs = e.attributes.borrow_mut();
        for name in URL_ATTRS {
            let Some(raw) = attrs.get(name).map(str::to_string) else { continue };
            let new = if name == "srcset" { rewrite_srcset(&raw, base, saved, prefix) } else { rewrite_url(&raw, base, saved, prefix) };
            if let Some(v) = new {
                attrs.insert(name, v);
            }
        }
        let style = attrs.get("style").map(str::to_string);
        if let Some(v) = style.and_then(|raw| rewrite_css_urls(&raw, base, saved, prefix, tally)) {
            attrs.insert("style", v);
        }
    }
    for st in styles {
        for c in st.children() {
            let Some(t) = c.as_text() else { continue };
            let raw = t.borrow().clone();
            if let Some(v) = rewrite_css_urls(&raw, base, saved, prefix, tally) {
                *t.borrow_mut() = v;
            }
        }
    }
    for b in bases {
        b.detach();
    }
    doc.inner_html()
}

/// One URL value's rewrite (None: leave it as written).
fn rewrite_url(raw: &str, base: &str, saved: &std::collections::HashMap<String, String>, prefix: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    let url = html_core::url::Url::parse_str_with_base(t, Some(base)).ok()?;
    if let Some(name) = saved.get(&url.serialize(true)) {
        let frag = t.find('#').map(|i| &t[i..]).unwrap_or("");
        return Some(format!("{prefix}{}{frag}", seg_encode(name)));
    }
    let scheme = t.split_once(':').is_some_and(|(s, _)| {
        s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    (!scheme).then(|| url.serialize(false))
}

/// A `srcset`'s rewrite: each candidate's URL through [`rewrite_url`], its descriptor kept (None: nothing changed).
fn rewrite_srcset(raw: &str, base: &str, saved: &std::collections::HashMap<String, String>, prefix: &str) -> Option<String> {
    let mut changed = false;
    let out: Vec<String> = raw
        .split(',')
        .map(|cand| {
            let cand = cand.trim();
            let (u, desc) = cand.split_once(char::is_whitespace).unwrap_or((cand, ""));
            match rewrite_url(u, base, saved, prefix) {
                Some(v) => {
                    changed = true;
                    if desc.trim().is_empty() { v } else { format!("{v} {}", desc.trim()) }
                }
                None => cand.to_string(),
            }
        })
        .collect();
    changed.then(|| out.join(", "))
}

// ---- PAGESAVECSSURLS (SR407): the URL references inside CSS (saved stylesheets, `<style>`, `style=`) -------------------
// ONE walk over css_core's own tokenizer (`css_core::tokenizer::Tokenizer`, the engine's CSS tokenizer; no regex, no
// second parser; `layout_core::css::extract_css_url` is a first-`url(` slice and is not this). Two users: the Complete
// save's rewrite ([`rewrite_css_urls`], SR398's one [`rewrite_url`] rule) and the save's discovery ([`css_refs`], SR398's
// [`resolve`]). The MHTML archive's parts are never rewritten: each keeps its Content-Location and the reader serves it.

/// PAGESAVECSSURLS (SR407): one Complete save's CSS references (`urls`) and those rewritten to a saved file (`rewritten`).
#[derive(Default)]
struct CssTally {
    urls: usize,
    rewritten: usize,
}

/// A stylesheet part?
fn is_css(content_type: &str) -> bool {
    essence(content_type) == "text/css"
}

/// PAGESAVECSSURLS (SR407): every URL reference of `css` — a `url(...)` token, `url(` + a string, `@import` + a string —
/// handed to `each`; a Some replaces that token (a url token as `url(v)`, quoted and escaped when `v` needs it; a string
/// in its own quote, escaped). Everything between tokens (comments included) is copied from the tokenizer's source.
/// None when nothing changed (the input stays as it was, byte for byte).
fn css_urls_walk(css: &str, mut each: impl FnMut(&str) -> Option<String>) -> Option<String> {
    use css_core::tokenizer::{Token, Tokenizer};
    let mut tk = Tokenizer::new(css);
    let mut toks: Vec<(Token, usize, usize)> = Vec::new();
    while let Some(t) = tk.next_token() {
        toks.push(t);
    }
    let src = tk.source();
    let mut out = String::with_capacity(css.len());
    let (mut copied, mut changed) = (0usize, false);
    let mut i = 0;
    while i < toks.len() {
        let string_after = matches!(&toks[i].0, Token::Function(f) if f.eq_ignore_ascii_case("url"))
            || matches!(&toks[i].0, Token::AtKeyword(k) if k.eq_ignore_ascii_case("import"));
        let at = if string_after {
            let mut j = i + 1;
            while matches!(toks.get(j), Some((Token::Whitespace, _, _))) {
                j += 1;
            }
            matches!(toks.get(j), Some((Token::String(_), _, _))).then_some(j)
        } else {
            matches!(&toks[i].0, Token::Url(_)).then_some(i)
        };
        let Some(j) = at else {
            i += 1;
            continue;
        };
        let (tok, start, end) = &toks[j];
        let (raw, quoted) = match tok {
            Token::Url(v) => (v.as_str(), false),
            Token::String(v) => (v.as_str(), true),
            _ => unreachable!(),
        };
        if let Some(v) = each(raw) {
            out.extend(&src[copied..*start]);
            if quoted {
                let q = src[*start];
                out.push_str(&css_quote(&v, q));
            } else if v.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '(' | ')' | '\\') || c.is_control()) {
                out.push_str(&format!("url({})", css_quote(&v, '"')));
            } else {
                out.push_str(&format!("url({v})"));
            }
            copied = *end;
            changed = true;
        }
        i = j + 1;
    }
    changed.then(|| {
        out.extend(&src[copied..]);
        out
    })
}

/// A CSS string of `v` in the quote `q`: the quote and `\` escaped, a newline as `\a `.
fn css_quote(v: &str, q: char) -> String {
    let mut s = String::with_capacity(v.len() + 2);
    s.push(q);
    for c in v.chars() {
        match c {
            '\n' => s.push_str("\\a "),
            c if c == q || c == '\\' => {
                s.push('\\');
                s.push(c);
            }
            c => s.push(c),
        }
    }
    s.push(q);
    s
}

/// PAGESAVECSSURLS (SR407): `css`'s URL references through [`rewrite_url`] against `base` (a saved one becomes `prefix` +
/// its file name, any other relative one its absolute URL, an absolute or `data:` or `#…` one stays), each counted in
/// `tally` (`rewritten` when it resolves to a key of `saved`). None: nothing changed.
fn rewrite_css_urls(css: &str, base: &str, saved: &std::collections::HashMap<String, String>, prefix: &str, tally: &mut CssTally) -> Option<String> {
    css_urls_walk(css, |raw| {
        tally.urls += 1;
        if resolve(base, raw).is_some_and(|u| saved.contains_key(&u)) {
            tally.rewritten += 1;
        }
        rewrite_url(raw, base, saved, prefix)
    })
}

/// PAGESAVECSSURLS (SR407): `css`'s URL references resolved against `base` (as [`resolve`]: fragment-less), in order, each
/// once; a `#…` one is not a resource.
fn css_refs(css: &str, base: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    css_urls_walk(css, |raw| {
        if !raw.trim_start().starts_with('#') {
            if let Some(u) = resolve(base, raw) {
                if !out.contains(&u) {
                    out.push(u);
                }
            }
        }
        None
    });
    out
}

/// PAGESAVECSSURLS (SR407): the CSS references of a live DOM: each `<style>` element's text and each `style=` value, in
/// tree order, through [`css_refs`] against the page URL `base`.
fn dom_css_refs(root: &crate::dom::NodeRef, base: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in root.descendants() {
        let Some(e) = n.as_element() else { continue };
        let mut css = String::new();
        if e.name.local.0 == "style" {
            css = n.text_contents();
        }
        let style = e.attributes.borrow().get("style").map(str::to_string);
        for text in [css, style.unwrap_or_default()] {
            for u in css_refs(&text, base) {
                if !out.contains(&u) {
                    out.push(u);
                }
            }
        }
    }
    out
}

/// PAGESAVECOMPLETE (SR398): the loader's file-URL arm (`net::fetch_document_any`): a `file://` URL whose path ends
/// `.mhtml` / `.mht` opens as its archive (None for every other URL: any other file stays refused). The archive (at most
/// [`ARCHIVE_MAX`]) is read by `gneiss_pal::mhtml::read`; the root's markup is the document, its URL the root's
/// `Content-Location` when http(s) (relative references find their parts), else the file URL; each http(s) part is
/// seeded into the HTTP cache's MEMORY tier under the page's partition ([`seed_parts`]) and the document runs under
/// [`ARCHIVE_CSP`].
pub fn open_archive(input: &str) -> Option<anyhow::Result<(String, String)>> {
    let path = file_url_path(input)?; // SAVERECENTOPEN (SR403): the one file-URL parse, shared with `open_local`
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ext != "mhtml" && ext != "mht" {
        return None;
    }
    Some(read_archive(input, &path))
}

/// [`open_archive`]'s read.
fn read_archive(input: &str, path: &Path) -> anyhow::Result<(String, String)> {
    if std::fs::metadata(path)?.len() > ARCHIVE_MAX {
        anyhow::bail!("the archive exceeds {} MiB", ARCHIVE_MAX >> 20);
    }
    let bytes = std::fs::read(path)?;
    let (root, parts) = gneiss_pal::mhtml::read(&bytes).ok_or_else(|| anyhow::anyhow!("not an MHTML archive"))?;
    let base = if is_web(&root.location) { root.location.clone() } else { input.to_string() };
    seed_parts(&base, &parts);
    crate::security::set_document_headers(&base, &[("content-security-policy".to_string(), ARCHIVE_CSP.to_string())]);
    Ok((String::from_utf8_lossy(&root.body).into_owned(), base))
}

/// PAGESAVECOMPLETE (SR398): each http(s) part as a fresh stored response (`cache_key(site_of(base), url)`, its
/// `Content-Type`, a year's `max-age`, immutable) in the memory tier ONLY (`HttpCache::put_memory`): it serves this load
/// and the next page's `clear_memory` forgets it; nothing reaches the disk tier.
fn seed_parts(base: &str, parts: &[gneiss_pal::mhtml::Part]) {
    use gneiss_pal::api::http::{site_of, Headers, Url};
    let Some(site) = site_of(base) else { return };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let cache = crate::net::http_cache();
    for p in parts.iter().filter(|p| is_web(&p.location)) {
        let Ok(u) = Url::parse(&p.location) else { continue };
        let mut h = Headers::new();
        let ty = if p.content_type.is_empty() { "application/octet-stream" } else { p.content_type.as_str() };
        if h.append("content-type", ty).is_err() || h.append("cache-control", "max-age=31536000, immutable").is_err() {
            continue;
        }
        let mut e = http_core::cache::Entry::new(http_core::cache::cache_key(&site, &u), &u, &Headers::new(), 200, "OK", &h, now, now);
        e.body = p.body.clone();
        cache.put_memory(e);
    }
}

// ---- SAVERECENT (SR391): the last Save As folder and the recent paths, Principia's two `aether` keys ----------------

/// SAVERECENT (SR391): Principia's key (namespace [`super::sidetabs::PREF_NS`], `aether`) for the folder the last Save As
/// chose; the shell seeds the next save panel with it (`SavePanel.folder`).
pub const SAVE_FOLDER_KEY: &str = "save.folder";
/// SAVERECENT (SR391): Principia's key for the recent paths, newest first, one per line (a TOML multi-line string).
pub const RECENT_KEY: &str = "recent";
/// SAVERECENT (SR391): how many recent paths are kept.
pub const RECENT_MAX: usize = 10;

/// SAVERECENT (SR391): the stored recent paths, newest first (blank lines dropped; empty when unset).
pub fn recent_paths(stored: Option<&str>) -> Vec<String> {
    stored.map(|s| s.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

/// SAVERECENT (SR391): fold a saved `path` into the two rows: its folder (`None` when it has none) and the recent list
/// with `path` first, an equal older entry dropped, at most [`RECENT_MAX`], one per line. Pure.
pub fn remember_path(path: &Path, stored_recent: Option<&str>) -> (Option<String>, String) {
    let p = path.to_string_lossy().into_owned();
    let folder = path.parent().map(|d| d.to_string_lossy().into_owned()).filter(|d| !d.is_empty());
    let mut recent = vec![p.clone()];
    recent.extend(recent_paths(stored_recent).into_iter().filter(|r| *r != p));
    recent.truncate(RECENT_MAX);
    (folder, recent.join("\n"))
}

/// SAVERECENT (SR391): record a written Save As at `path` through the prefs fulfiller (`get` / `set` over namespace and
/// key; the shell passes quartzite's `chrome_rows::stored_str` / `store_str`, Principia's store): the folder key, then
/// the recent list. A path with a line break is not recorded (the list is one path per line).
pub fn remember_saved(path: &Path, get: impl Fn(&str, &str) -> Option<String>, mut set: impl FnMut(&str, &str, String)) {
    if path.to_string_lossy().contains('\n') {
        return;
    }
    let ns = super::sidetabs::PREF_NS;
    let (folder, recent) = remember_path(path, get(ns, RECENT_KEY).as_deref());
    if let Some(f) = folder {
        set(ns, SAVE_FOLDER_KEY, f);
    }
    set(ns, RECENT_KEY, recent);
}

/// SAVERECENT (SR391): the save panel's initial folder: Principia's `aether.save.folder` when it is still a directory,
/// else `None` (the panel's own).
pub fn save_folder(get: impl Fn(&str, &str) -> Option<String>) -> Option<String> {
    get(super::sidetabs::PREF_NS, SAVE_FOLDER_KEY).filter(|f| !f.is_empty() && Path::new(f).is_dir())
}

#[cfg(test)]
mod tests {
    use super::{save_bytes_at, save_bytes_in};
    use std::path::Path;

    /// PDF4FORMSAVE (SR354): the one writer: the path answered, the `(n)` suffix on a collision (a finished file or a
    /// landing `.part`), a hostile name kept inside the folder, no `.part` left behind.
    #[test]
    fn pdf4formsave_writer_suffix() {
        let dir = std::env::temp_dir().join(format!("aether-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = save_bytes_in(&dir, "form.pdf", b"one").unwrap();
        assert_eq!(p, dir.join("form.pdf"));
        assert_eq!(std::fs::read(&p).unwrap(), b"one");
        let p = save_bytes_in(&dir, "form.pdf", b"two").unwrap();
        assert_eq!(p, dir.join("form (1).pdf"), "a collision takes the (1) suffix");
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::read(dir.join("form.pdf")).unwrap(), b"one", "the first file is untouched");
        std::fs::write(dir.join("x.pdf.part"), b"").unwrap();
        assert_eq!(save_bytes_in(&dir, "x.pdf", b"x").unwrap(), dir.join("x (1).pdf"), "a landing .part is taken too");
        std::fs::remove_file(dir.join("x.pdf.part")).unwrap();
        let p = save_bytes_in(&dir, "../evil.pdf", b"e").unwrap();
        assert_eq!(p.parent(), Some(dir.as_path()), "a hostile name lands inside the folder: {p:?}");
        assert_eq!(save_bytes_in(&dir, "...", b"d").unwrap(), dir.join("download"));
        let parts = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".part")).count();
        assert_eq!(parts, 0, "no .part left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// SAVEDIALOG (SR364): the Save As writer lands at the chosen path exactly: a file there is replaced (the panel
    /// confirmed it), no `(1)`, no `.part` left behind; a path in no folder answers why.
    #[test]
    fn savedialog_writer_at() {
        let dir = std::env::temp_dir().join(format!("aether-saveas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let to = dir.join("chosen.pdf");
        std::fs::write(&to, b"old").unwrap();
        assert_eq!(save_bytes_at(&to, b"new").unwrap(), to);
        assert_eq!(std::fs::read(&to).unwrap(), b"new", "replaced at the chosen path");
        assert!(!dir.join("chosen (1).pdf").exists() && !dir.join("chosen.pdf.part").exists());
        assert!(save_bytes_at(&dir.join("gone").join("x.pdf"), b"x").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// SAVERECENT (SR391): a Save As writes the folder key and puts the path first in the recent list, through a fake
    /// prefs fulfiller (the shell's is Principia's store).
    #[test]
    fn saverecent_writes_folder_and_recent() {
        use std::cell::RefCell;
        use std::collections::HashMap;
        let store: RefCell<HashMap<(String, String), String>> = RefCell::new(HashMap::new());
        let get = |ns: &str, k: &str| store.borrow().get(&(ns.to_string(), k.to_string())).cloned();
        let set = |ns: &str, k: &str, v: String| {
            store.borrow_mut().insert((ns.to_string(), k.to_string()), v);
        };
        super::remember_saved(Path::new("/home/p/Docs/a.pdf"), get, set);
        assert_eq!(get("aether", super::SAVE_FOLDER_KEY).as_deref(), Some("/home/p/Docs"));
        assert_eq!(get("aether", super::RECENT_KEY).as_deref(), Some("/home/p/Docs/a.pdf"));
        super::remember_saved(Path::new("/tmp/b.pdf"), get, set);
        assert_eq!(get("aether", super::SAVE_FOLDER_KEY).as_deref(), Some("/tmp"));
        assert_eq!(get("aether", super::RECENT_KEY).as_deref(), Some("/tmp/b.pdf\n/home/p/Docs/a.pdf"), "newest first");
        super::remember_saved(Path::new("/home/p/Docs/a.pdf"), get, set);
        assert_eq!(super::recent_paths(get("aether", super::RECENT_KEY).as_deref()), ["/home/p/Docs/a.pdf", "/tmp/b.pdf"], "deduped, moved to the front");
        super::remember_saved(Path::new("/tmp/bad\nname.pdf"), get, set);
        assert_eq!(get("aether", super::SAVE_FOLDER_KEY).as_deref(), Some("/home/p/Docs"), "a path with a line break is not recorded");
    }

    /// SAVERECENT (SR391): the recent list keeps at most ten paths, newest first.
    #[test]
    fn saverecent_caps_at_ten() {
        let mut stored: Option<String> = None;
        for i in 0..12 {
            let (_, r) = super::remember_path(&Path::new("/d").join(format!("{i}.pdf")), stored.as_deref());
            stored = Some(r);
        }
        let r = super::recent_paths(stored.as_deref());
        assert_eq!(r.len(), super::RECENT_MAX);
        assert_eq!((r[0].as_str(), r[9].as_str()), ("/d/11.pdf", "/d/2.pdf"));
        assert!(super::recent_paths(None).is_empty() && super::recent_paths(Some("")).is_empty());
    }

    /// SAVERECENT (SR391): the panel's initial folder is the stored folder only while it is a directory.
    #[test]
    fn saverecent_folder_only_if_directory() {
        let dir = std::env::temp_dir().join(format!("aether-saverecent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().into_owned();
        assert_eq!(super::save_folder(|_, k| (k == super::SAVE_FOLDER_KEY).then(|| d.clone())), Some(d.clone()));
        assert_eq!(super::save_folder(|_, _| Some(dir.join("gone").to_string_lossy().into_owned())), None, "a stale folder is the panel's own");
        assert_eq!(super::save_folder(|_, _| None), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

// ---- SAVERECENTOPEN (SR403): the chrome's open verb (Ctrl/Cmd+O), its recent folder, and local pages -----------------
// The shell's Coordinator catches the key (`open_key`; the engine cannot read Principia) and fires [`open_panel`] on the
// one bus; the face's open panel (OPEN-DIALOG-ONE, SR386) answers `OpenPanelChose`, which [`open_chosen`] records through
// SAVERECENT's ONE writer (`remember_saved`) and turns into the file URL the active tab opens: an image through the tab's
// Facet arm, a `.mhtml` through [`open_archive`], a `.html` / `.htm` through [`open_local`] (net's file-URL arm).

/// SAVERECENTOPEN (SR403): the surface whose face answers the open panel (the page surface, as a Save As's panel).
pub const OPEN_SURFACE: &str = "viewport";

/// SAVERECENTOPEN (SR403): is `key` the chrome's open verb, Ctrl+O (GTK) or Cmd+O (`Meta+o`, the macOS page view)? Not
/// with Alt or Shift. Read by the engine's one key parser (`forms::edit::parse_key`).
pub fn open_key(key: &str) -> bool {
    use crate::forms::edit::{parse_key, Key};
    let (m, k) = parse_key(key);
    (m.ctrl || m.meta) && !m.alt && !m.shift && matches!(k, Some(Key::Char('o' | 'O')))
}

/// SAVERECENTOPEN (SR403): the folder the open panel starts in: SAVERECENT's [`save_folder`] (the folder the last Save
/// As or Open chose, while it is a directory), else the Downloads folder ([`downloads_dir`]).
pub fn open_folder(get: impl Fn(&str, &str) -> Option<String>) -> String {
    save_folder(get).unwrap_or_else(|| downloads_dir().to_string_lossy().into_owned())
}

/// SAVERECENTOPEN (SR403): the open panel Ctrl/Cmd+O asks the page surface's face for: any file (no filters: a type
/// Aether does not serve is refused by name when it loads, so the panel keeps no second extension table), opening in
/// [`open_folder`].
pub fn open_panel(get: impl Fn(&str, &str) -> Option<String>) -> bandy::SMessage {
    bandy::SMessage::OpenPanel { url: OPEN_SURFACE.to_string(), filters: Vec::new(), folder: Some(open_folder(get)) }
}

/// SAVERECENTOPEN (SR403): the file the person chose in the open panel: recorded through SAVERECENT's ONE writer
/// ([`remember_saved`]: its folder becomes `aether.save.folder`, the path heads `aether.recent`), answered as the
/// `file://` URL the active tab opens (each path segment percent-encoded by [`seg_encode`]).
pub fn open_chosen(path: &Path, get: impl Fn(&str, &str) -> Option<String>, set: impl FnMut(&str, &str, String)) -> String {
    remember_saved(path, get, set);
    let p = path.to_string_lossy();
    format!("file://{}", p.split('/').map(seg_encode).collect::<Vec<_>>().join("/"))
}

/// SAVERECENTOPEN (SR403): the local path a `file://` URL names (`localhost` host allowed, the query and fragment
/// dropped, `%XX` decoded); `None` for any other URL. [`open_archive`] and [`open_local`] read it.
fn file_url_path(input: &str) -> Option<PathBuf> {
    let rest = input.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    Some(PathBuf::from(downloads::percent_decode(rest.split(['?', '#']).next().unwrap_or(""))))
}

/// SAVERECENTOPEN (SR403): the loader's second file-URL arm (`net::fetch_document_any`, after [`open_archive`]'s): a
/// `.html` / `.htm` file opens as its markup (at most [`ARCHIVE_MAX`]), the document's URL the file URL, under
/// [`ARCHIVE_CSP`] as an opened archive is (no script runs, an opaque origin). OPENANYTYPE (SR408): every other file is
/// asked of [`local_any`] (a text, image or media type by TYPECORE's one table, or a PDF by its head); what it does not
/// serve is refused by name on the console (`[pdf] open: <ext> not served`, `none` without an extension) and answered
/// `None`: the loader's own refusal follows.
pub fn open_local(input: &str) -> Option<anyhow::Result<(crate::net::Doc, String)>> {
    let path = file_url_path(input)?;
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => Some(read_local(input, &path).map(|(html, url)| (crate::net::Doc::Text(html), url))),
        other => match local_any(input, &path) {
            Some(opened) => Some(opened),
            None => {
                crate::js::host::console_out(1, &format!("[pdf] open: {} not served", other.unwrap_or("none")));
                None
            }
        },
    }
}

/// [`open_local`]'s read.
fn read_local(input: &str, path: &Path) -> anyhow::Result<(String, String)> {
    if std::fs::metadata(path)?.len() > ARCHIVE_MAX {
        anyhow::bail!("the page exceeds {} MiB", ARCHIVE_MAX >> 20);
    }
    let bytes = std::fs::read(path)?;
    crate::security::set_document_headers(input, &[("content-security-policy".to_string(), ARCHIVE_CSP.to_string())]);
    Ok((String::from_utf8_lossy(&bytes).into_owned(), input.to_string()))
}

// ---- OPENANYTYPE (SR408): a local file of any served type, and a local page's own file:// subresources -----------------
// The type is TYPECORE's (`type_core::by_extension`, the one table both rings read); the document a type becomes is the
// engine's one by-type markup (`js::window::frames::markup_for`, HTML §7.4.6); a PDF is chosen by the PDF arm's own predicate
// (`pdf::is_pdf`, the `%PDF-` head) under its own cap. Subresources of a local page are read only from the page's folder.

/// OPENANYTYPE (SR408): the largest local subresource [`read_part`] reads (the page loader's http image / body cap).
const PART_MAX: u64 = 10 << 20;

/// OPENANYTYPE (SR408): [`open_local`]'s non-HTML arm. A `text/*` or JSON file is its text in a `<pre>` (at most
/// [`ARCHIVE_MAX`]); an `image/*` file an `<img>` of its own file URL (its bytes read as the page's own subresource, so an
/// SVG is drawn by the image path's renderer, never run); a `video/*` / `audio/*` file a media element of its file URL
/// (Stria reads the file; nothing is read here); a file the table does not name opens as a PDF when its head says so
/// (at most `pdf::MAX_BYTES`, refused by `pdf::over_cap`). Every text document runs under [`ARCHIVE_CSP`]. `None`: not
/// served.
fn local_any(input: &str, path: &Path) -> Option<anyhow::Result<(crate::net::Doc, String)>> {
    let mime = type_core::by_extension(&path.to_string_lossy());
    let Some(mime) = mime else {
        let mut head = [0u8; 8];
        let n = std::fs::File::open(path).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).ok()?;
        return crate::pdf::is_pdf("", &head[..n]).then(|| read_local_pdf(input, path));
    };
    let text = mime.starts_with("text/") || mime == type_core::APP_JSON;
    let media = mime.starts_with("video/") || mime.starts_with("audio/");
    if !(text || media || mime.starts_with("image/")) {
        return None;
    }
    Some(local_markup(input, path, mime, text))
}

/// [`local_any`]'s document through `js::window::frames::markup_for`: the text read for a `text` type, the file URL alone else.
fn local_markup(input: &str, path: &Path, mime: &str, text: bool) -> anyhow::Result<(crate::net::Doc, String)> {
    let len = std::fs::metadata(path)?.len();
    let body = if text {
        if len > ARCHIVE_MAX {
            anyhow::bail!("the file exceeds {} MiB", ARCHIVE_MAX >> 20);
        }
        String::from_utf8_lossy(&std::fs::read(path)?).into_owned()
    } else {
        String::new()
    };
    let doc = crate::api::fetch::SubDocument { url: input.to_string(), content_type: mime.to_string(), status: 200, body, charset: "UTF-8" };
    let html = crate::js::window::frames::markup_for(&doc);
    crate::security::set_document_headers(input, &[("content-security-policy".to_string(), ARCHIVE_CSP.to_string())]);
    Ok((crate::net::Doc::Text(html), input.to_string()))
}

/// [`local_any`]'s PDF: the bytes whole, under the PDF arm's cap.
fn read_local_pdf(input: &str, path: &Path) -> anyhow::Result<(crate::net::Doc, String)> {
    let len = std::fs::metadata(path)?.len();
    if len > crate::pdf::MAX_BYTES as u64 {
        anyhow::bail!(crate::pdf::over_cap(len));
    }
    Ok((crate::net::Doc::Pdf(std::fs::read(path)?), input.to_string()))
}

/// OPENANYTYPE (SR408): the page loader's bound on a local page's subresources (`net`'s discovery filters): `part` is
/// admitted (answered as is) only when the document `doc` and `part` are both file URLs and `part`'s real path lies under
/// the real path of `doc`'s folder. A part outside it (a `../` escape, which the URL join has already normalized, or a
/// symlink out) is named on the console (`[pdf] open: <path> outside the page's folder`) and refused; a part that does
/// not exist, or a non-file URL, is answered `None` silently.
pub(crate) fn local_part(doc: &str, part: &str) -> Option<String> {
    if !part.starts_with("file://") {
        return None;
    }
    let folder = file_url_path(doc)?.parent()?.canonicalize().ok()?;
    let p = file_url_path(part)?;
    let real = p.canonicalize().ok()?;
    if !real.starts_with(&folder) {
        crate::js::host::console_out(1, &format!("[pdf] open: {} outside the page's folder", p.display()));
        return None;
    }
    Some(part.to_string())
}

/// OPENANYTYPE (SR408): a local subresource's bytes (at most [`PART_MAX`]), for a URL [`local_part`] admitted.
pub(crate) fn read_part(url: &str) -> anyhow::Result<Vec<u8>> {
    let path = file_url_path(url).ok_or_else(|| anyhow::anyhow!("not a file URL"))?;
    if std::fs::metadata(&path)?.len() > PART_MAX {
        anyhow::bail!("the file exceeds {} MiB", PART_MAX >> 20);
    }
    Ok(std::fs::read(&path)?)
}
