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
//!   ([`save_bytes_at`]) writes it at the path the person chose in the face's save panel.
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
