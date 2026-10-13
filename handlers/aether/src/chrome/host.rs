//! The chrome's std adapter: where its files live and the network half of downloads. Everything here
//! is I/O over the std-free model next to it (bookmarks, downloads); nothing here decides anything a
//! model function could.
//!
//! - Bookmarks are `<data dir>/bookmarks.tsv`, the data dir being the one Web Storage's
//!   `LocalStorage` already uses (`directories::ProjectDirs::from("", "UnaOS", "Aether")`).
//! - Downloads land in the person's Downloads folder (`directories::UserDirs::download_dir`, else
//!   `~/Downloads`), written to `<name>.part` and renamed when complete — a half-written file never
//!   carries the real name.
//! - [`probe`] is the top-level navigation's download decision: a `HEAD` of the URL over gneiss_pal's
//!   HTTP client (the same cookie jar the page loads use), whose `Content-Disposition` /
//!   `Content-Type` the model judges ([`super::downloads`]). A failed or refused probe answers
//!   "document" and the load proceeds exactly as before.

use std::io::Write as _;
use std::path::PathBuf;
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

/// Write the bookmarks file atomically (temp file, then rename).
pub fn save_bookmarks(b: &Bookmarks) -> std::io::Result<()> {
    let path = bookmarks_path().ok_or_else(|| std::io::Error::other("no data directory"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tsv.tmp");
    std::fs::write(&tmp, b.serialize())?;
    std::fs::rename(tmp, path)
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
    if let Err(e) = std::fs::create_dir_all(&dir) {
        d.done = true;
        d.error = Some(e.to_string());
        report(&d);
        return d;
    }
    d.name = downloads::unique_name(&name, |n| dir.join(n).exists() || dir.join(format!("{n}.part")).exists());
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
        std::fs::rename(&part, dir.join(&d.name)).map_err(|e| e.to_string())
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
