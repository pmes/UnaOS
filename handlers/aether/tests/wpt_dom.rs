//! AETHERJS (SR63): the WPT `dom/nodes` directory run through Aether on js_core. AETHERWPT (SR72): the
//! same runner over `dom/events`, `dom/ranges`, `dom/traversal`, `dom/collections`, `dom/lists`, the
//! `html/dom` reflection suite (its top-level files), `html/semantics/forms` and `html/webappapis` — one
//! `#[test]` per directory (they run on parallel test threads; every page's engine state is thread-local).
//!
//! web-platform-tests is fetched at test time (sparse, shallow, pinned at [`WPT_COMMIT`]) into
//! `target/wpt` or found at `$AETHER_WPT_DIR`; offline (or `AETHER_WPT_OFFLINE=1`) the test is skipped.
//! Nothing of WPT enters the source tree.
//!
//! A tiny HTTP server on 127.0.0.1 serves the checkout (so tests load `/resources/testharness.js` and
//! their support files by absolute path, through http_core), with two substitutions:
//! `/resources/testharnessreport.js` registers a completion callback that leaves the results in
//! `window.__wpt` (JSON), and `X.window.html` is the generated wrapper for an `X.window.js` test (what
//! wptserve does). Each test page is loaded with `AetherEngine::load_page`, the event loop runs to idle
//! in virtual time (15 s budget — longer than testharness's 10 s default timeout), and the subtest
//! statuses are read back.
//!
//! Reported per directory: pass/total of subtests over every `.html`/`.htm`/`.window.js`/`.any.js` (window
//! global) testharness file, and over "the subset Aether's surface covers" — the same files minus those
//! that need a feature outside this binding (listed with the reason in [`excluded`]). Files that need
//! wptserve itself (`.py` handlers, `.sub.` substitution, `.https.`/`.h2.`/`.www.` hosts, `{{…}}`
//! templates, `get-host-info`) are not run and are counted as "needs server". XML documents (`.xhtml`,
//! `.svg`, `.xml`) are not run: Aether parses HTML only. Run it with
//! `cargo test --release -p aether --test wpt_dom -- --nocapture` (`--test-threads=N` bounds the parallel
//! directories); a debug build skips it unless `AETHER_WPT=1` (or `AETHER_WPT_ONLY`) is set. Per-file results
//! go to `target/wpt-<dir>.tsv` (`/` → `-`). `AETHER_WPT_ONLY=<a,b,…>` runs files whose paths contain any of
//! the substrings; `AETHER_WPT_VERBOSE=1` prints failing subtests; each directory prints its wall time.
//!
//! AETHERNET (SR70) adds `websockets/`, `eventsource/` and `xhr/` (`wpt_websockets`, `wpt_eventsource`,
//! `wpt_xhr`), served by `wpt_net` — a local stand-in for wptserve (static files, `.headers`, the `sub`
//! pipe, the `.any.js` / `.window.js` wrappers, the Python handlers those directories use most, and a
//! WebSocket server for pywebsocket's handlers). wptserve is not available: a test that needs what the
//! stand-in does not serve (another handler, TLS, a second origin, frames, service workers, XML documents, …) is
//! skipped by name with the reason ([`net_excluded`]). AETHERWORKERS3 (SR129): `.worker.js` tests run in the
//! stand-in's generated `.worker.html` page (`fetch_tests_from_worker`), as wptserve serves them. Per-file results go to
//! `target/wpt-<dir>.tsv`; `.any.js` tests run their window variant (`?default` for websockets).
//!
//! AETHERWPT2 (SR135) adds the rest of what a browser is measured on ([`WPT2_DIRS`]: url, encoding, fetch/api,
//! streams, FileAPI, webstorage, IndexedDB, html/browsers/{history,the-window-object,browsing-the-web},
//! html/semantics/{scripting-1,embedded-content,tabular-data,interactive-elements,text-level-semantics,
//! grouping-content}, custom-elements, shadow-dom, selection, uievents (keyboard events are
//! `uievents/keyboard`), pointerevents, WebCryptoAPI, compression, resource-timing, performance-timeline, console,
//! html/infrastructure/urls), one `wpt2_*` test each, served by the same stand-in. What a test needs that the
//! run cannot give it is named per file by [`wpt2_excluded`] (TLS, testdriver, cross-origin hosts, auxiliary
//! browsing contexts, bfcache, idlharness, an un-emulated handler, the `/media/` fixtures, layout); reftests and
//! manual pages (no testharness.js) are counted, not run. An API Aether lacks is never an exclusion: those tests
//! run and fail, and the per-directory floors (0 at landing) rise from the bench's first run.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

mod wpt_net;

const WPT_REPO: &str = "https://github.com/web-platform-tests/wpt.git";
const WPT_COMMIT: &str = "da1f6d20caf40b4003ae288dda8c97f252dd9264";

/// The sparse checkout: the directories run, plus the shared harness and support trees they load.
const SPARSE: &str = "/dom/\n/resources/\n/common/\n/html/resources/\n/html/dom/\n/html/semantics/forms/\n/html/webappapis/\n/html/semantics/interfaces.js\n/dom/nodes/\n/dom/*.js\n/websockets/\n/eventsource/\n/xhr/\n/url/\n/encoding/\n/fetch/api/\n/streams/\n/FileAPI/\n/webstorage/\n/IndexedDB/\n/html/browsers/history/\n/html/browsers/the-window-object/\n/html/browsers/browsing-the-web/\n/html/semantics/scripting-1/\n/html/semantics/embedded-content/\n/html/semantics/tabular-data/\n/html/semantics/interactive-elements/\n/html/semantics/text-level-semantics/\n/html/semantics/grouping-content/\n/custom-elements/\n/shadow-dom/\n/selection/\n/uievents/\n/pointerevents/\n/WebCryptoAPI/\n/compression/\n/resource-timing/\n/performance-timeline/\n/console/\n/html/infrastructure/urls/\n/images/\n";

/// The directories run: (path, recurse into subdirectories). `html/dom` is its top-level reflection suite.
const DIRS: &[(&str, bool)] = &[
    ("dom/nodes", false),
    ("dom/events", true),
    ("dom/ranges", true),
    ("dom/traversal", true),
    ("dom/collections", true),
    ("dom/lists", true),
    ("html/dom", false),
    ("html/semantics/forms", true),
    ("html/webappapis", true),
];

const REPORT_JS: &str = r#"(function () {
  if (typeof add_completion_callback !== 'function') { return; }
  setup({ output: false });
  add_completion_callback(function (tests, status) {
    window.__wpt = JSON.stringify({
      status: status.status,
      message: status.message || '',
      tests: tests.map(function (t) { return { name: t.name, status: t.status, message: t.message || '' }; })
    });
  });
})();
"#;

fn target_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")
}

/// One locate per process: every `#[test]` runs on its own thread, and on a fresh target dir twelve threads
/// raced the sparse clone into the same `target/wpt` — the losers saw a half-made checkout and returned None,
/// so eleven of twelve directories printed "skipped (no checkout, offline)" and PASSED with nothing run.
fn locate() -> Option<PathBuf> {
    static ROOT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    ROOT.get_or_init(locate_once).clone()
}

fn locate_once() -> Option<PathBuf> {
    if std::env::var("AETHER_WPT_OFFLINE").is_ok() {
        return None;
    }
    if let Ok(d) = std::env::var("AETHER_WPT_DIR") {
        let p = PathBuf::from(d);
        if p.join("dom/nodes").is_dir() {
            return Some(p);
        }
    }
    let t = target_dir().join("wpt");
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(&t).args(args).status().map(|s| s.success()).unwrap_or(false);
    // A checkout made by an earlier runner lacks the SR72 directories, the AETHERNET ones or the AETHERWPT2 ones:
    // widen it in place.
    let complete = |t: &Path| t.join("html/webappapis").is_dir() && NET_DIRS.iter().all(|d| t.join(d).is_dir()) && WPT2_DIRS.iter().all(|(d, _)| t.join(d).is_dir());
    if t.join("dom/nodes").is_dir() && t.join("resources/testharness.js").is_file() && !complete(&t) {
        let _ = std::fs::write(t.join(".git/info/sparse-checkout"), SPARSE);
        let _ = Command::new("git").arg("-C").arg(&t).args(["checkout", "-q", "FETCH_HEAD"]).status();
        let _ = Command::new("git").arg("-C").arg(&t).args(["read-tree", "-mu", "HEAD"]).status();
    }
    if t.join("dom/nodes").is_dir() && t.join("resources/testharness.js").is_file() && complete(&t) {
        return Some(t);
    }
    let _ = std::fs::create_dir_all(&t);
    let ok = git(&["init", "-q"])
        && git(&["config", "core.sparseCheckout", "true"])
        && std::fs::write(t.join(".git/info/sparse-checkout"), SPARSE).is_ok()
        && git(&["fetch", "-q", "--depth", "1", "--filter=blob:none", WPT_REPO, WPT_COMMIT])
        && git(&["checkout", "-q", "FETCH_HEAD"]);
    ok.then_some(t)
}

/// The directories AETHERNET (SR70) runs, and the sparse checkout that carries them.
const NET_DIRS: &[&str] = &["websockets", "eventsource", "xhr"];

fn serve(root: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let root = root.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h == "\r\n" || h.is_empty() {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").split(['?', '#']).next().unwrap_or("/").to_string();
                let rel = path.trim_start_matches('/').to_string();
                let (status, body, ty): (&str, Vec<u8>, &str) = if rel == "resources/testharnessreport.js" {
                    ("200 OK", REPORT_JS.as_bytes().to_vec(), "text/javascript")
                } else if let Some((stem, kind)) = rel.strip_suffix(".window.html").map(|s| (s, "window")).or_else(|| rel.strip_suffix(".any.html").map(|s| (s, "any"))) {
                    let js = format!("/{stem}.{kind}.js");
                    let src = std::fs::read_to_string(root.join(format!("{stem}.{kind}.js"))).unwrap_or_default();
                    // `// META: script=...` lines become script tags (wptserve's wrapper).
                    let mut metas = String::new();
                    for l in src.lines() {
                        if let Some(sc) = l.strip_prefix("// META: script=") {
                            metas.push_str(&format!("<script src=\"{}\"></script>\n", sc.trim()));
                        }
                    }
                    let page = format!(
                        "<!doctype html>\n<meta charset=utf-8>\n<script src=\"/resources/testharness.js\"></script>\n<script src=\"/resources/testharnessreport.js\"></script>\n{metas}<div id=log></div>\n<script src=\"{js}\"></script>\n"
                    );
                    ("200 OK", page.into_bytes(), "text/html; charset=utf-8")
                } else {
                    let file = root.join(&rel);
                    if !rel.contains("..") && file.is_file() {
                        let ty = match file.extension().and_then(|e| e.to_str()) {
                            Some("html") | Some("htm") => "text/html; charset=utf-8",
                            Some("js") => "text/javascript; charset=utf-8",
                            Some("css") => "text/css",
                            Some("json") => "application/json",
                            Some("xml") => "application/xml",
                            Some("svg") => "image/svg+xml",
                            Some("xhtml") => "application/xhtml+xml",
                            _ => "application/octet-stream",
                        };
                        ("200 OK", std::fs::read(&file).unwrap(), ty)
                    } else {
                        ("404 Not Found", b"not found".to_vec(), "text/plain")
                    }
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            });
        }
    });
    port
}

/// A file that needs wptserve itself (not a static file server): not run, counted as "needs server".
fn needs_server(name: &str, src: &str) -> bool {
    [".sub.", ".https.", ".h2.", ".www.", ".optional."].iter().any(|m| name.contains(m))
        || src.contains("{{")
        || src.contains("get-host-info")
        || src.contains("/common/dispatcher/")
        || src.contains(".py\"")
        || src.contains(".py?")
        || src.contains(".py'")
        || src.contains(".py`")
}

/// Files that need what this binding does not provide, with the reason. The covered subset is the run
/// set minus these; the reasons name a surface outside this arc, never a failing one inside it.
/// The local helper scripts a test loads — `<script src>` (and `// META: script=` for `.window.js` /
/// `.any.js`), the harness's own `/resources/testharness*.js` and remote URLs left out — read from the
/// checkout (AETHERDEBT SR93: `excluded()` grepped only the test file, so a test whose iframes or
/// workers come from a helper was counted as covered).
fn helper_sources(root: &Path, name: &str, src: &str) -> String {
    let dir = name.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let mut urls: Vec<String> = Vec::new();
    let lower = src.to_ascii_lowercase();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<script") {
        let start = at + i;
        let end = lower[start..].find('>').map(|e| start + e).unwrap_or(lower.len());
        let tag = &src[start..end];
        let tl = &lower[start..end];
        if let Some(j) = tl.find("src=") {
            let v = &tag[j + 4..];
            let (q, rest) = match v.chars().next() {
                Some(c @ ('"' | '\'')) => (Some(c), &v[1..]),
                _ => (None, v),
            };
            let val: String = match q {
                Some(q) => rest.chars().take_while(|&c| c != q).collect(),
                None => rest.chars().take_while(|c| !c.is_whitespace()).collect(),
            };
            urls.push(val);
        }
        at = end.max(start + 7);
    }
    for l in src.lines() {
        if let Some(v) = l.trim().strip_prefix("// META: script=") {
            urls.push(v.trim().to_string());
        }
    }
    let mut out = String::new();
    for u in urls {
        let u = u.split(['?', '#']).next().unwrap_or("").to_string();
        if u.is_empty() || u.contains("://") || u.starts_with("/resources/testharness") || u.starts_with("/resources/testdriver") {
            continue;
        }
        let path = match u.strip_prefix('/') {
            Some(abs) => root.join(abs),
            None => root.join(dir).join(&u),
        };
        if let Ok(h) = std::fs::read_to_string(&path) {
            out.push('\n');
            out.push_str(&h);
        }
    }
    out
}

fn excluded(name: &str, src: &str) -> Option<&'static str> {
    let base = name.rsplit('/').next().unwrap_or(name);
    // AETHERWPT4 (SR142): `window.open(` is no longer a reason — auxiliary browsing contexts landed with AETHERWPT3 O8
    // (js/frames.rs, HTML §7.2.2.1); frameset and object documents still are.
    if src.contains("<frameset") || src.contains("<object") {
        return Some("frames / browsing contexts");
    }
    // Documents that stop being fully active (an iframe navigated away or removed while its realm's
    // listeners are still reachable): `not-fully-active-helpers.js` builds them.
    if src.contains("not-fully-active-helpers.js") || src.contains("globalThis.navigateFrame") {
        return Some("frames / browsing contexts (documents not fully active)");
    }
    if base.contains("moveBefore") {
        return Some("moveBefore (2025 proposal)");
    }
    // AETHERWPT4 (SR142): `XMLHttpRequest` / `fetch(` are no longer reasons — the network APIs run since AETHERNET (SR70)
    // and the run serves the checkout; XML documents loaded as documents (.xhtml / .xml) still are.
    if src.contains(".xhtml") || src.contains(".xml\"") {
        return Some("XML documents");
    }
    if src.contains("testdriver.js") || src.contains("test_driver") {
        return Some("testdriver (WebDriver input automation)");
    }
    // AETHERWORKERS2 (SR125): dedicated workers (`new Worker`), MessageChannel and postMessage run now.
    // AETHERWORKERS3 (SR129): SharedWorker runs too; what stays out is service workers (their WPT needs a TLS
    // origin).
    if src.contains("serviceWorker") {
        return Some("service workers");
    }
    if name.contains("/scrolling/")
        || name.starts_with("html/webappapis/update-rendering/")
        || name.starts_with("html/semantics/forms/customizable-combobox/")
        || src.contains("getBoundingClientRect")
        || src.contains("offsetWidth")
        || src.contains("offsetHeight")
        || src.contains("offsetTop")
        || src.contains("offsetLeft")
        || src.contains("clientWidth")
        || src.contains("clientHeight")
        || src.contains("scrollTop")
        || src.contains("scrollLeft")
    {
        // AETHERWPT4 (SR142): `getComputedStyle` (the full author cascade, js/cssom.rs `cascaded_value`),
        // `elementFromPoint` / `elementsFromPoint` (CSSOM View §6.1 hit testing), `scrollIntoView` (CSSOM View §7) and
        // `requestAnimationFrame` (HTML §8.10, js/window.rs since AETHERJS) are no longer reasons on their own.
        return Some("layout / rendering (EYES is the oracle)");
    }
    if src.contains("alert(") && name.contains("user-prompts") || src.contains("print(") && name.contains("user-prompts") {
        return Some("user prompts (manual)");
    }
    None
}

#[derive(Default)]
struct FileResult {
    pass: usize,
    total: usize,
    harness: String,
    failures: Vec<(String, String)>,
}

fn run_file(rt: &tokio::runtime::Runtime, port: u16, rel: &str) -> FileResult {
    let url = format!("http://127.0.0.1:{port}/{rel}");
    let mut out = FileResult::default();
    let page = match rt.block_on(aether::net::fetch_page(&url)) {
        Ok(p) => p,
        Err(e) => {
            out.harness = format!("fetch error: {e}");
            out.total = 1;
            return out;
        }
    };
    let mut engine = aether::AetherEngine::new();
    engine.load_page(page, true);
    let Some(js) = engine.js_engine.as_mut() else {
        out.harness = "no engine".into();
        out.total = 1;
        return out;
    };
    aether::event_loop::settle(js, 15_000);
    let json = js.eval_string("typeof window.__wpt === 'string' ? window.__wpt : ''").unwrap_or_default();
    if json.is_empty() {
        out.harness = "harness did not complete".into();
        out.total = 1;
        return out;
    }
    // Parse the small JSON by hand (no serde for tests): extract each test's name and status.
    let v: serde_json::Value = serde_json::from_str(&json).unwrap_or(serde_json::Value::Null);
    let hs = v.get("status").and_then(|s| s.as_i64()).unwrap_or(-1);
    out.harness = match hs {
        0 => "OK".into(),
        1 => format!("ERROR {}", v.get("message").and_then(|m| m.as_str()).unwrap_or("")),
        2 => "TIMEOUT".into(),
        3 => "PRECONDITION_FAILED".into(),
        _ => "?".into(),
    };
    if let Some(tests) = v.get("tests").and_then(|t| t.as_array()) {
        for t in tests {
            out.total += 1;
            let st = t.get("status").and_then(|s| s.as_i64()).unwrap_or(1);
            if st == 0 {
                out.pass += 1;
            } else {
                out.failures.push((
                    t.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                    t.get("message").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                ));
            }
        }
    }
    if out.total == 0 {
        // A harness error before any test: one failing result for the file.
        out.total = 1;
    }
    out
}

/// Every testharness file under `dir` (recursively when asked), as paths relative to the checkout.
fn collect(root: &Path, dir: &str, recurse: bool, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(root.join(dir)) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let n = e.file_name().to_string_lossy().to_string();
        let rel = format!("{dir}/{n}");
        if e.path().is_dir() {
            if recurse && !matches!(n.as_str(), "resources" | "support" | "crashtests" | "tools" | "reference" | "references") {
                collect(root, &rel, recurse, out);
            }
        } else if n.ends_with(".html") || n.ends_with(".htm") || n.ends_with(".window.js") || n.ends_with(".any.js") {
            out.push(rel);
        }
    }
}

/// Runs one directory and returns (covered pass, covered total).
fn run_dir(dir: &str, recurse: bool) -> Option<(usize, usize)> {
    let tag = dir.replace('/', "-");
    // The whole of dom/nodes takes ~12 min in a release build; an unoptimized `cargo test -p aether` runs it
    // only when asked (`AETHER_WPT=1`) or for a filtered subset.
    if cfg!(debug_assertions) && std::env::var("AETHER_WPT").is_err() && std::env::var("AETHER_WPT_ONLY").is_err() {
        println!("wpt {dir}: skipped in a debug build (run with --release, or set AETHER_WPT=1)");
        return None;
    }
    let Some(root) = locate() else {
        println!("wpt {dir}: skipped (no checkout, offline)");
        return None;
    };
    let started = std::time::Instant::now();
    let mut files = Vec::new();
    collect(&root, dir, recurse, &mut files);
    files.sort();
    let only = std::env::var("AETHER_WPT_ONLY").ok();
    let verbose = std::env::var("AETHER_WPT_VERBOSE").is_ok();
    let port = serve(root.clone());
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let (mut all_pass, mut all_total, mut cov_pass, mut cov_total) = (0, 0, 0, 0);
    let (mut files_run, mut files_cov, mut server) = (0, 0, 0);
    let mut tsv = String::from("file\tpass\ttotal\tharness\texcluded\n");
    let mut excluded_list: Vec<(String, &'static str)> = Vec::new();
    for f in &files {
        if only.as_ref().is_some_and(|o| !o.split(',').any(|x| f.contains(x))) {
            continue;
        }
        let src = std::fs::read_to_string(root.join(f)).unwrap_or_default();
        let is_js = f.ends_with(".window.js") || f.ends_with(".any.js");
        // Support pages and reftests are not tests.
        if !src.contains("testharness.js") && !is_js {
            continue;
        }
        // An `.any.js` runs here only in its window global.
        if f.ends_with(".any.js") {
            if let Some(g) = src.lines().find_map(|l| l.strip_prefix("// META: global=")) {
                if !g.split(',').any(|x| x.trim() == "window") {
                    continue;
                }
            }
        }
        if needs_server(f, &src) {
            server += 1;
            tsv.push_str(&format!("{f}\t0\t0\tnot run\tneeds server\n"));
            continue;
        }
        let rel = if let Some(stem) = f.strip_suffix(".window.js") {
            format!("{stem}.window.html")
        } else if let Some(stem) = f.strip_suffix(".any.js") {
            format!("{stem}.any.html")
        } else {
            f.clone()
        };
        let scan = format!("{src}{}", helper_sources(&root, f, &src));
        let ex = excluded(f, &scan);
        let t0 = std::time::Instant::now();
        let r = run_file(&rt, port, &rel);
        let secs = t0.elapsed().as_secs_f64();
        files_run += 1;
        all_pass += r.pass;
        all_total += r.total;
        if let Some(e) = ex {
            excluded_list.push((f.clone(), e));
        } else {
            files_cov += 1;
            cov_pass += r.pass;
            cov_total += r.total;
        }
        let short = f.strip_prefix(dir).unwrap_or(f).trim_start_matches('/');
        println!("wpt {dir} {:58} {:4}/{:<4} {} ({secs:.1}s){}", short, r.pass, r.total, r.harness, ex.map(|e| format!("  [excluded: {e}]")).unwrap_or_default());
        if verbose {
            for (n, m) in r.failures.iter().take(40) {
                println!("    FAIL {n}: {}", m.chars().take(200).collect::<String>());
            }
        }
        tsv.push_str(&format!("{f}\t{}\t{}\t{}\t{}\n", r.pass, r.total, r.harness, ex.unwrap_or("")));
    }
    if only.is_none() {
        let _ = std::fs::write(target_dir().join(format!("wpt-{tag}.tsv")), tsv);
    }
    let pct = |p: usize, t: usize| if t == 0 { 0.0 } else { 100.0 * p as f64 / t as f64 };
    let wall = started.elapsed().as_secs_f64();
    println!("wpt {dir} (all {files_run} files):      {all_pass}/{all_total} subtests ({:.1}%)", pct(all_pass, all_total));
    println!("wpt {dir} (covered {files_cov} files):  {cov_pass}/{cov_total} subtests ({:.1}%)", pct(cov_pass, cov_total));
    println!("wpt {dir}: needs server (not run): {server} files; wall {wall:.0} s");
    let mut reasons: Vec<&str> = excluded_list.iter().map(|(_, r)| *r).collect();
    reasons.sort();
    reasons.dedup();
    for r in reasons {
        println!("  {dir} excluded for {r}: {}", excluded_list.iter().filter(|(_, x)| *x == r).count());
    }
    only.is_none().then_some((cov_pass, cov_total))
}

/// Regression floor for a directory's covered subtests (set from the measured run; raise it as the surface grows).
fn floor(dir: &str, recurse: bool, pct: usize) {
    if let Some((p, t)) = run_dir(dir, recurse) {
        assert!(p * 100 >= t * pct, "covered {dir} subtests fell below {pct}%: {p}/{t}");
    }
}

fn dir_entry(dir: &str) -> (&'static str, bool) {
    *DIRS.iter().find(|(d, _)| *d == dir).expect("listed directory")
}

#[test]
fn wpt_dom_nodes() {
    let (d, r) = dir_entry("dom/nodes");
    floor(d, r, 80);
}

#[test]
fn wpt_dom_events() {
    let (d, r) = dir_entry("dom/events");
    floor(d, r, 0);
}

#[test]
fn wpt_dom_ranges() {
    let (d, r) = dir_entry("dom/ranges");
    floor(d, r, 0);
}

#[test]
fn wpt_dom_traversal() {
    let (d, r) = dir_entry("dom/traversal");
    floor(d, r, 0);
}

#[test]
fn wpt_dom_collections() {
    let (d, r) = dir_entry("dom/collections");
    floor(d, r, 0);
}

#[test]
fn wpt_dom_lists() {
    let (d, r) = dir_entry("dom/lists");
    floor(d, r, 0);
}

#[test]
fn wpt_html_dom() {
    let (d, r) = dir_entry("html/dom");
    floor(d, r, 0);
}

#[test]
fn wpt_html_semantics_forms() {
    let (d, r) = dir_entry("html/semantics/forms");
    floor(d, r, 0);
}

#[test]
fn wpt_html_webappapis() {
    let (d, r) = dir_entry("html/webappapis");
    floor(d, r, 0);
}

// =================================================================================================
// AETHERNET (SR70): websockets/, eventsource/, xhr/ through the wptserve stand-in (`wpt_net`)
// =================================================================================================

/// Runs one test page until its harness completes: the event loop settles in slices (virtual time while
/// idle, real time while the page has network operations in flight), checking for the results between.
fn run_net_file(rt: &tokio::runtime::Runtime, port: u16, rel: &str) -> FileResult {
    let url = format!("http://127.0.0.1:{port}/{rel}");
    let mut out = FileResult::default();
    let page = match rt.block_on(aether::net::fetch_page(&url)) {
        Ok(p) => p,
        Err(e) => {
            out.harness = format!("fetch error: {e}");
            out.total = 1;
            return out;
        }
    };
    let mut engine = aether::AetherEngine::new();
    engine.load_page(page, true);
    let Some(js) = engine.js_engine.as_mut() else {
        out.harness = "no engine".into();
        out.total = 1;
        return out;
    };
    let real = std::time::Instant::now();
    let v0 = aether::event_loop::now_ms();
    let mut json = String::new();
    while real.elapsed().as_secs() < 30 && aether::event_loop::now_ms().saturating_sub(v0) < 16_000 {
        aether::event_loop::settle(js, 200);
        json = js.eval_string("typeof window.__wpt === 'string' ? window.__wpt : ''").unwrap_or_default();
        if !json.is_empty() {
            break;
        }
    }
    if json.is_empty() {
        out.harness = "harness did not complete".into();
        out.total = 1;
        return out;
    }
    let v: serde_json::Value = serde_json::from_str(&json).unwrap_or(serde_json::Value::Null);
    let hs = v.get("status").and_then(|s| s.as_i64()).unwrap_or(-1);
    out.harness = match hs {
        0 => "OK".into(),
        1 => format!("ERROR {}", v.get("message").and_then(|m| m.as_str()).unwrap_or("")),
        2 => "TIMEOUT".into(),
        3 => "PRECONDITION_FAILED".into(),
        _ => "?".into(),
    };
    if let Some(tests) = v.get("tests").and_then(|t| t.as_array()) {
        for t in tests {
            out.total += 1;
            if t.get("status").and_then(|s| s.as_i64()).unwrap_or(1) == 0 {
                out.pass += 1;
            } else {
                out.failures.push((
                    t.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                    t.get("message").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                ));
            }
        }
    }
    if out.total == 0 {
        out.total = 1;
    }
    out
}

/// The `.py` handlers a test source references (resolved against its directory), as WPT-root paths.
fn py_refs(rel_dir: &str, src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = src[i..].find(".py") {
        let end = i + p + 3;
        if end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_') {
            i = end;
            continue;
        }
        let mut start = i + p;
        while start > 0 && (b[start - 1].is_ascii_alphanumeric() || b"/._-".contains(&b[start - 1])) {
            start -= 1;
        }
        let name = &src[start..end];
        i = end;
        // `*_wsh.py` are pywebsocket handlers (the WebSocket server's), not wptserve's.
        if name.len() <= 3 || name.starts_with("http") || name.ends_with("_wsh.py") {
            continue;
        }
        let full = if let Some(abs) = name.strip_prefix('/') {
            abs.to_string()
        } else {
            let mut parts: Vec<&str> = rel_dir.split('/').filter(|x| !x.is_empty()).collect();
            for seg in name.split('/') {
                match seg {
                    "." | "" => {}
                    ".." => {
                        parts.pop();
                    }
                    s => parts.push(s),
                }
            }
            parts.join("/")
        };
        if !out.contains(&full) {
            out.push(full);
        }
    }
    out
}

/// The pywebsocket handlers a websockets test connects to (`… + '/name'`).
fn ws_refs(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for q in ['\'', '"', '`'] {
        let pat = format!("{q}/");
        let mut i = 0;
        while let Some(p) = src[i..].find(&pat) {
            let s0 = i + p + 2;
            let name: String = src[s0..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
            i = s0;
            if !name.is_empty() && !out.contains(&name) && src[s0 + name.len()..].starts_with([q, '?']) {
                out.push(name);
            }
        }
    }
    out
}

/// Why a test of `websockets/`, `eventsource/` or `xhr/` is not run against the stand-in (None = run).
fn net_excluded(rel: &str, src: &str) -> Option<String> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    if name.contains(".https.") || name.contains(".h2.") || name.contains(".serviceworker.") {
        return Some("needs a TLS server (https / h2)".into());
    }
    // AETHERWORKERS2 (SR125): a page that starts a dedicated worker (`new Worker(`) runs now. AETHERWORKERS3 (SR129):
    // so do SharedWorker pages, `fetch_tests_from_worker` pages, and `.worker.js` tests — through the stand-in's
    // `.worker.html` wrapper (wpt_net `worker_wrapper`, wptserve's WorkersHandler).
    if src.contains("RemoteContextHelper") || name.contains("back-forward-cache") {
        return Some("bfcache / remote browsing contexts".into());
    }
    // AETHERWPT4 (SR142): `window.open(` runs (AETHERWPT3 O8's auxiliary browsing contexts); `document.domain` does not.
    if src.contains("document.domain") {
        return Some("document.domain".into());
    }
    if src.contains("://天気") || src.contains("\\u5929") {
        return Some("an IDN subdomain of the test host (needs web-platform.test DNS)".into());
    }
    if src.contains("idlharness") {
        return Some("idlharness (interfaces/*.idl are not in the checkout)".into());
    }
    if src.contains("REMOTE_ORIGIN") || src.contains("REMOTE_HOST") || src.contains("HTTP_REMOTE") || src.contains("OTHER_ORIGIN") || src.contains("{{domains[") || src.contains("{{hosts[") || src.contains("CROSSDOMAIN") || src.contains("cross-origin") && !dir.starts_with("websockets") || name.contains("cross-origin") || name.contains("access-control") || name.contains("cors") {
        return Some("cross-origin (no second origin with CORS in the stand-in)".into());
    }
    for h in py_refs(dir, src) {
        if !wpt_net::PY_HANDLERS.contains(&h.as_str()) {
            return Some(format!("needs wptserve handler {h}"));
        }
    }
    if dir.starts_with("websockets") {
        if dir.contains("stream") {
            return Some("WebSocketStream (not this arc)".into());
        }
        if src.contains("SCHEME_DOMAIN_PORT") || src.contains("ws://") || src.contains("__SCHEME") {
            for h in ws_refs(src) {
                if !wpt_net::WS_HANDLERS.contains(&h.as_str()) && h != "echo" {
                    return Some(format!("needs pywebsocket handler {h}"));
                }
            }
        }
        if src.contains("basic_auth") || src.contains("receive-many-with-backpressure") || src.contains("send-backpressure") || src.contains("sleep_10_v13") {
            return Some("needs a pywebsocket handler the stand-in does not emulate".into());
        }
    }
    if dir.starts_with("xhr") {
        // AETHERWPT4 (SR142): XML / document responses (XHR §3.6.10 over api/xml.rs), FormData, blob: URLs (AETHERWPT3
        // O3), upload progress events (XHR §3.5.6) and legacy charsets (XHR §3.6 final MIME type / encoding over
        // AETHERWPT3 O2's decoders) are no longer reasons.
        if src.contains("setRequestHeader(\"Authorization\"") || name.contains("send-authentication") || name.contains("user-password") {
            return Some("HTTP authentication".into());
        }
    }
    None
}

/// Test files (recursively) of `dir`, with the variants this runner loads (window scope, non-TLS).
fn net_files(root: &Path, dir: &str) -> Vec<(String, Vec<String>, Vec<String>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.join(dir)];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let n = e.file_name().to_string_lossy().to_string();
                if n != "resources" && n != "support" && n != "handlers" {
                    stack.push(p);
                }
                continue;
            }
            let n = e.file_name().to_string_lossy().to_string();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            let is_test = n.ends_with(".any.js") || n.ends_with(".window.js") || n.ends_with(".worker.js") || n.ends_with(".html") || n.ends_with(".htm");
            if !is_test || n.ends_with(".headers") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap_or_default();
            if (n.ends_with(".html") || n.ends_with(".htm")) && !src.contains("testharness.js") {
                continue;
            }
            let mut run = Vec::new();
            let mut skipped = Vec::new();
            let variants: Vec<String> = src
                .lines()
                .filter_map(|l| {
                    let l = l.trim();
                    l.strip_prefix("// META: variant=").map(|v| v.trim().to_string()).or_else(|| {
                        l.strip_prefix("<meta name=\"variant\" content=\"").and_then(|r| r.split('"').next()).map(str::to_string)
                    })
                })
                .collect();
            let variants = if variants.is_empty() { vec![String::new()] } else { variants };
            for v in variants {
                if v.contains("wss") || v.contains("h2") || v.contains("https") {
                    skipped.push(v);
                } else {
                    run.push(v);
                }
            }
            out.push((rel, run, skipped));
        }
    }
    out.sort();
    out
}

fn run_net_dir(dir: &str, floor_pct: usize) {
    run_served_dir(dir, floor_pct, &|_, rel, src| net_excluded(rel, src));
}

/// Runs every test page of `dir` through the stand-in, skipping (with the reason) those `skip` names.
fn run_served_dir(dir: &str, floor_pct: usize, skip: &dyn Fn(&Path, &str, &str) -> Option<String>) {
    if cfg!(debug_assertions) && std::env::var("AETHER_WPT").is_err() && std::env::var("AETHER_WPT_ONLY").is_err() {
        println!("wpt {dir}: skipped in a debug build (run with --release, or set AETHER_WPT=1)");
        return;
    }
    let Some(root) = locate() else {
        println!("wpt {dir}: skipped (no checkout, offline)");
        return;
    };
    if !root.join(dir).is_dir() {
        println!("wpt {dir}: skipped (not in the checkout)");
        return;
    }
    let only = std::env::var("AETHER_WPT_ONLY").ok();
    let verbose = std::env::var("AETHER_WPT_VERBOSE").is_ok();
    let ports = wpt_net::start(root.clone(), REPORT_JS);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let (mut pass, mut total, mut files_run) = (0usize, 0usize, 0usize);
    let mut tsv = String::from("file\tvariant\tpass\ttotal\tharness\tskipped\n");
    let mut skipped: Vec<(String, String)> = Vec::new();
    for (rel, run, tls_variants) in net_files(&root, dir) {
        if only.as_ref().is_some_and(|o| !o.split(',').any(|x| rel.contains(x))) {
            continue;
        }
        let src = std::fs::read_to_string(root.join(&rel)).unwrap_or_default();
        for v in &tls_variants {
            skipped.push((format!("{rel}{v}"), "needs a TLS server (wss / h2 variant)".into()));
            tsv.push_str(&format!("{rel}\t{v}\t0\t0\t-\tneeds a TLS server (wss / h2 variant)\n"));
        }
        if let Some(why) = skip(&root, &rel, &src) {
            for v in &run {
                skipped.push((format!("{rel}{v}"), why.clone()));
                tsv.push_str(&format!("{rel}\t{v}\t0\t0\t-\t{why}\n"));
            }
            continue;
        }
        for v in &run {
            let page = if let Some(stem) = rel.strip_suffix(".any.js") {
                format!("{stem}.any.html{v}")
            } else if let Some(stem) = rel.strip_suffix(".window.js") {
                format!("{stem}.window.html{v}")
            } else if let Some(stem) = rel.strip_suffix(".worker.js") {
                // AETHERWORKERS3 (SR129): a worker-context test runs in its generated `.worker.html` page.
                format!("{stem}.worker.html{v}")
            } else {
                format!("{rel}{v}")
            };
            let r = run_net_file(&rt, ports.http, &page);
            files_run += 1;
            pass += r.pass;
            total += r.total;
            println!("wpt {:64} {:4}/{:<4} {}", format!("{rel}{v}"), r.pass, r.total, r.harness);
            if verbose {
                for (n, m) in r.failures.iter().take(12) {
                    println!("    FAIL {n}: {}", m.chars().take(200).collect::<String>());
                }
            }
            tsv.push_str(&format!("{rel}\t{v}\t{}\t{}\t{}\t\n", r.pass, r.total, r.harness));
        }
    }
    let _ = std::fs::write(target_dir().join(format!("wpt-{}.tsv", dir.replace('/', "-"))), tsv);
    let pct = if total == 0 { 0.0 } else { 100.0 * pass as f64 / total as f64 };
    println!("wpt {dir} (served {files_run} test pages): {pass}/{total} subtests ({pct:.1}%)");
    let mut reasons: Vec<&str> = skipped.iter().map(|(_, r)| r.as_str()).collect();
    reasons.sort();
    reasons.dedup();
    println!("wpt {dir}: {} test pages skipped (the stand-in cannot serve them):", skipped.len());
    for r in reasons {
        let names: Vec<&str> = skipped.iter().filter(|(_, x)| x == r).map(|(n, _)| n.as_str()).collect();
        println!("  skipped for {r} ({}): {}", names.len(), names.join(" "));
    }
    if only.is_none() {
        assert!(pass * 100 >= total * floor_pct, "wpt {dir}: served subtests fell below {floor_pct}%");
    }
}

#[test]
fn wpt_websockets() {
    run_net_dir("websockets", 85);
}

#[test]
fn wpt_eventsource() {
    run_net_dir("eventsource", 85);
}

#[test]
fn wpt_xhr() {
    run_net_dir("xhr", 85);
}

// =================================================================================================
// AETHERWPT2 (SR135): the rest of the platform a browser is measured on, through the same stand-in
// =================================================================================================

/// The directories AETHERWPT2 runs (recursively, through `wpt_net`), each with what its tests need that this
/// run cannot give them — the reasons [`wpt2_excluded`] names per file. An API Aether lacks is not listed
/// here: those tests run and fail (docs/dev/evidence/aetherwpt/WPT.md, the AETHERWPT2 gap list).
pub const WPT2_DIRS: &[(&str, &str)] = &[
    ("url", "an un-emulated handler (percent-encoding.py); XML documents (.xhtml, not collected)"),
    ("encoding", "TLS (sharedarraybuffer.https); un-emulated handlers (single-byte-raw.py, text-plain-charset.py, json-charset.py, text-html-meta-charset.py); idlharness"),
    ("fetch/api", "cross-origin hosts (cors/, credentials/, get-host-info REMOTE origins); TLS and h2 (.https., .h2.); service workers; un-emulated handlers (preflight.py, cache.py, authentication.py, …); auxiliary browsing contexts (keepalive window.open); idlharness"),
    ("streams", "idlharness; TLS (.https. transferable variants); auxiliary browsing contexts"),
    ("FileAPI", "auxiliary browsing contexts (url/ window.open); cross-origin hosts; TLS; manual pages (-manual); idlharness"),
    ("webstorage", "auxiliary browsing contexts (event tests that window.open); TLS"),
    ("IndexedDB", "TLS (storage buckets, .https.); testdriver; auxiliary browsing contexts; bfcache; idlharness"),
    ("html/browsers/history", "auxiliary browsing contexts (window.open); un-emulated handlers; testdriver; bfcache / remote contexts"),
    ("html/browsers/the-window-object", "auxiliary browsing contexts (window.open, named access to opened windows); cross-origin hosts; testdriver; TLS"),
    ("html/browsers/browsing-the-web", "remote contexts (/common/dispatcher/, RemoteContextHelper); bfcache; auxiliary browsing contexts; un-emulated handlers (slow.py, refresh.py, redirect.py, …); testdriver; TLS"),
    ("html/semantics/scripting-1", "un-emulated handlers (serve-custom-response.py, serve-with-content-type.py, load-error-events.py, referrer-checker.py, log.py, …); cross-origin hosts; TLS"),
    ("html/semantics/embedded-content", "the /media/ fixtures (not in the sparse checkout); layout; testdriver; un-emulated handlers; cross-origin hosts; TLS; reftests"),
    ("html/semantics/tabular-data", "layout (a few); reftests"),
    ("html/semantics/interactive-elements", "testdriver (dialog, popover, details input); layout; reftests"),
    ("html/semantics/text-level-semantics", "reftests; layout"),
    ("html/semantics/grouping-content", "reftests; layout"),
    ("custom-elements", "auxiliary browsing contexts; testdriver; layout; un-emulated handlers (form-echo.py, echo-content-escaped.py)"),
    ("shadow-dom", "testdriver (focus, input, selection by input); layout; reftests"),
    ("selection", "testdriver; layout (caret and extent by coordinates); reftests"),
    ("uievents", "testdriver (keyboard, mouse, wheel, click order — the keyboard-event tests are uievents/keyboard); layout; manual pages"),
    ("pointerevents", "testdriver (nearly every test drives a real pointer); TLS; layout"),
    ("WebCryptoAPI", "idlharness; cross-origin hosts (secure_context/ non-secure origin); h2. `.https.` alone runs: it asks for a secure context, and 127.0.0.1 is potentially trustworthy (Secure Contexts §3.2)"),
    ("compression", "nothing infrastructural: its one `.https.` file asks only for a secure context and runs (as WebCryptoAPI's do); worker-only globals are not run"),
    ("resource-timing", "un-emulated handlers (status-code.py, compressed-js.py, redirect-cors.py, fake_responses.py, …); cross-origin hosts (TAO tests); TLS; auxiliary browsing contexts; layout"),
    ("performance-timeline", "bfcache / remote contexts (back-forward-cache, not-restored-reasons); auxiliary browsing contexts; idlharness"),
    ("console", "testdriver and manual pages (the -logging and -manual pages are read by a person); idlharness"),
    ("html/infrastructure/urls", "TLS (.https.); legacy-charset documents served through un-emulated handlers (resource.py, stash.py, blank.py); cross-origin hosts"),
];

/// Directories whose `.https.` files ask only for a secure context (no second origin, no TLS-only feature): the
/// stand-in's 127.0.0.1 origin is potentially trustworthy, so they run over http.
const SECURE_CONTEXT_ONLY: &[&str] = &["WebCryptoAPI", "compression"];

/// The `.py` handlers a test references, resolved like [`py_refs`], with `fetch/api`'s `RESOURCES_DIR + "x.py"`
/// form (a bare name next to `resources/utils.js`) resolved to the sibling `resources/` directory when the bare
/// resolution names no file in the checkout.
fn wpt2_py_refs(root: &Path, rel_dir: &str, src: &str) -> Vec<String> {
    py_refs(rel_dir, src)
        .into_iter()
        .map(|h| {
            if root.join(&h).is_file() || h.contains("/resources/") {
                return h;
            }
            let name = h.rsplit('/').next().unwrap_or(&h).to_string();
            let mut d = rel_dir.to_string();
            loop {
                let cand = format!("{d}/resources/{name}");
                if root.join(&cand).is_file() {
                    return cand;
                }
                match d.rsplit_once('/') {
                    Some((up, _)) => d = up.to_string(),
                    None => return h,
                }
            }
        })
        .collect()
}

/// Why a test of an AETHERWPT2 directory is not run (None = run). Infrastructure only: what the stand-in, the
/// headless page or the harness cannot provide. A missing API is never a reason.
fn wpt2_excluded(root: &Path, rel: &str, src: &str) -> Option<String> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let top = WPT2_DIRS.iter().map(|(d, _)| *d).filter(|d| rel.starts_with(&format!("{d}/"))).max_by_key(|d| d.len()).unwrap_or("");
    if name.ends_with(".any.js") {
        if let Some(g) = src.lines().find_map(|l| l.trim().strip_prefix("// META: global=")) {
            if !g.split(',').any(|x| x.trim() == "window") {
                return Some("not a window-global test (worker / shadowrealm globals only)".into());
            }
        }
    }
    if name.contains(".h2.") || name.contains(".serviceworker.") || name.contains(".sharedworker.") {
        return Some("needs a TLS server (h2) or a service / shared worker variant".into());
    }
    if name.contains(".https.") && !SECURE_CONTEXT_ONLY.contains(&top) {
        return Some("needs a TLS server (https)".into());
    }
    if name.contains("-manual.") {
        return Some("manual test (a person reads the result)".into());
    }
    let scan = format!("{src}{}", helper_sources(root, rel, src));
    if scan.contains("testdriver.js") || scan.contains("test_driver") {
        return Some("testdriver (WebDriver input automation)".into());
    }
    if src.contains("idlharness") {
        return Some("idlharness (interfaces/*.idl are not in the checkout)".into());
    }
    if scan.contains("RemoteContextHelper") || scan.contains("/common/dispatcher/") || name.contains("back-forward-cache") || rel.contains("/bfcache/") {
        return Some("bfcache / remote browsing contexts".into());
    }
    // AETHERWPT4 (SR142): `window.open(` is no longer a reason (AETHERWPT3 O8: auxiliary browsing contexts, js/frames.rs).
    if src.contains("REMOTE_ORIGIN") || src.contains("REMOTE_HOST") || src.contains("HTTP_REMOTE") || src.contains("OTHER_ORIGIN") || src.contains("{{domains[") || src.contains("{{hosts[") || src.contains("CROSSDOMAIN") || src.contains("get_host_info().HTTPS") || name.contains("cross-origin") || name.contains("access-control") || name.contains("cors") || dir.contains("/cors") || dir.contains("/credentials") {
        return Some("cross-origin (no second origin with CORS in the stand-in)".into());
    }
    for h in wpt2_py_refs(root, dir, src) {
        if !wpt_net::PY_HANDLERS.contains(&h.as_str()) {
            return Some(format!("needs wptserve handler {h}"));
        }
    }
    if scan.contains("/media/") {
        return Some("media fixtures (/media/ is not in the sparse checkout)".into());
    }
    if [
        // AETHERWPT4 (SR142): getComputedStyle, elementFromPoint, scrollIntoView and requestAnimationFrame lifted (as in
        // `excluded`).
        "getBoundingClientRect", "offsetWidth", "offsetHeight", "offsetTop", "offsetLeft", "clientWidth", "clientHeight", "scrollTop", "scrollLeft", "caretPositionFromPoint",
    ]
    .iter()
    .any(|t| src.contains(t))
    {
        return Some("layout / rendering (EYES is the oracle)".into());
    }
    None
}

/// Testharness-free HTML pages under `dir` (reftests, crash tests, manual pages): counted, not run.
fn non_testharness_pages(root: &Path, dir: &str) -> usize {
    let mut n = 0;
    let mut stack = vec![root.join(dir)];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let f = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if !matches!(f.as_str(), "resources" | "support" | "handlers" | "tools") {
                    stack.push(p);
                }
            } else if (f.ends_with(".html") || f.ends_with(".htm")) && !f.contains("-ref.") && !f.contains("-ref-") && !std::fs::read_to_string(&p).unwrap_or_default().contains("testharness.js") {
                n += 1;
            }
        }
    }
    n
}

fn run_wpt2_dir(dir: &str, floor_pct: usize) {
    let listed = WPT2_DIRS.iter().any(|(d, _)| *d == dir);
    assert!(listed, "{dir} is not an AETHERWPT2 directory");
    if !(cfg!(debug_assertions) && std::env::var("AETHER_WPT").is_err() && std::env::var("AETHER_WPT_ONLY").is_err()) {
        if let Some(root) = locate() {
            println!("wpt {dir}: {} reftest / crash / manual pages (no testharness.js) not run", non_testharness_pages(&root, dir));
        }
    }
    // Crash tests and tool trees are not testharness tests (`net_files` already leaves resources/support/handlers).
    run_served_dir(dir, floor_pct, &|root, rel, src| {
        if ["/crashtests/", "/tools/", "/reference/", "/references/"].iter().any(|x| rel.contains(x)) {
            return Some("not a testharness test (crash test / tool / reference)".into());
        }
        wpt2_excluded(root, rel, src)
    });
}

macro_rules! wpt2_tests {
    ($($name:ident => $dir:literal),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                // Floor 0 at landing (AETHERWPT2, SR135): the bench's first run sets each directory's floor.
                run_wpt2_dir($dir, 0);
            }
        )*
    };
}

wpt2_tests! {
    wpt2_url => "url",
    wpt2_encoding => "encoding",
    wpt2_fetch_api => "fetch/api",
    wpt2_streams => "streams",
    wpt2_fileapi => "FileAPI",
    wpt2_webstorage => "webstorage",
    wpt2_indexeddb => "IndexedDB",
    wpt2_html_browsers_history => "html/browsers/history",
    wpt2_html_browsers_the_window_object => "html/browsers/the-window-object",
    wpt2_html_browsers_browsing_the_web => "html/browsers/browsing-the-web",
    wpt2_html_semantics_scripting_1 => "html/semantics/scripting-1",
    wpt2_html_semantics_embedded_content => "html/semantics/embedded-content",
    wpt2_html_semantics_tabular_data => "html/semantics/tabular-data",
    wpt2_html_semantics_interactive_elements => "html/semantics/interactive-elements",
    wpt2_html_semantics_text_level_semantics => "html/semantics/text-level-semantics",
    wpt2_html_semantics_grouping_content => "html/semantics/grouping-content",
    wpt2_custom_elements => "custom-elements",
    wpt2_shadow_dom => "shadow-dom",
    wpt2_selection => "selection",
    wpt2_uievents => "uievents",
    wpt2_pointerevents => "pointerevents",
    wpt2_webcryptoapi => "WebCryptoAPI",
    wpt2_compression => "compression",
    wpt2_resource_timing => "resource-timing",
    wpt2_performance_timeline => "performance-timeline",
    wpt2_console => "console",
    wpt2_html_infrastructure_urls => "html/infrastructure/urls",
}

/// Every AETHERWPT2 directory has its `wpt2_*` test (the macro list above and [`WPT2_DIRS`] agree), and the
/// sparse checkout carries each one.
#[test]
fn wpt2_directory_table_is_complete() {
    for (d, needs) in WPT2_DIRS {
        assert!(SPARSE.lines().any(|l| l == format!("/{d}/")), "{d} is not in the sparse checkout");
        assert!(!needs.is_empty(), "{d} names what it needs");
    }
    assert_eq!(WPT2_DIRS.len(), 27);
}
