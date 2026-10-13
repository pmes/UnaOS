//! AETHERAPIS (SR145): the web-platform-tests of the APIs a modern page reaches for next — CSS Typed OM
//! (`css/css-typed-om`), Web Animations (`web-animations`), IntersectionObserver (`intersection-observer`),
//! the dialog element (`html/semantics/interactive-elements/the-dialog-element`), popovers
//! (`html/semantics/popovers`), the Fullscreen API (`fullscreen`), the Clipboard API (`clipboard-apis`),
//! CloseWatcher (`close-watcher`) and ResizeObserver (`resize-observer`) — one `wpt_apis_*` test per directory,
//! served by the wptserve stand-in `wpt_net` exactly as `wpt_dom.rs`'s `wpt2_*` tests are (the checkout, the
//! harness substitution, `.any.js` / `.window.js` wrappers, the per-page virtual-time settle). `wpt_dom.rs` is
//! AETHERWPT5's and is not edited: this runner carries its own copies of the checkout locator (widened to these
//! directories), the helper-script scan and the file walk.
//!
//! What a page needs that this run cannot give it is named per file by [`apis_excluded`] (docs/dev/evidence/
//! aetherwpt/WPT.md §AETHERAPIS): testdriver (WebDriver input automation: every popover, dialog, fullscreen,
//! clipboard and close-watcher page that needs a real click or key press), TLS, a second origin, un-emulated
//! handlers, bfcache / remote contexts, idlharness, worker-only globals, manual pages. Unlike `wpt2_excluded`,
//! the layout tokens (`getBoundingClientRect`, `offset*`, `client*`, `scroll*`) are NOT a reason here: the
//! intersection and resize observers are geometry, and their pages run against the engine's own layout and
//! fail honestly where it differs. An API Aether lacks is never an exclusion.
//!
//! Run it with `cargo test --release -p aether --test wpt_apis -- --nocapture` (a debug build skips it unless
//! `AETHER_WPT=1` or `AETHER_WPT_ONLY` is set); `AETHER_WPT_ONLY=<a,b,…>` and `AETHER_WPT_VERBOSE=1` as for
//! `wpt_dom`. Per-file results go to `target/wpt-<dir>.tsv`. Floors are 0 at landing: the bench's first run sets
//! each directory's floor.

use std::path::{Path, PathBuf};
use std::process::Command;

#[allow(dead_code)]
mod wpt_net;

const WPT_REPO: &str = "https://github.com/web-platform-tests/wpt.git";
const WPT_COMMIT: &str = "da1f6d20caf40b4003ae288dda8c97f252dd9264";

/// `wpt_dom.rs`'s sparse checkout (every directory it runs, the harness and support trees) plus this runner's
/// directories: one checkout serves both runners, whichever widens it first.
const SPARSE: &str = "/dom/\n/resources/\n/common/\n/html/resources/\n/html/dom/\n/html/semantics/forms/\n/html/webappapis/\n/html/semantics/interfaces.js\n/dom/nodes/\n/dom/*.js\n/websockets/\n/eventsource/\n/xhr/\n/url/\n/encoding/\n/fetch/api/\n/streams/\n/FileAPI/\n/webstorage/\n/IndexedDB/\n/html/browsers/history/\n/html/browsers/the-window-object/\n/html/browsers/browsing-the-web/\n/html/semantics/scripting-1/\n/html/semantics/embedded-content/\n/html/semantics/tabular-data/\n/html/semantics/interactive-elements/\n/html/semantics/text-level-semantics/\n/html/semantics/grouping-content/\n/custom-elements/\n/shadow-dom/\n/selection/\n/uievents/\n/pointerevents/\n/WebCryptoAPI/\n/compression/\n/resource-timing/\n/performance-timeline/\n/console/\n/html/infrastructure/urls/\n/images/\n/css/css-typed-om/\n/web-animations/\n/intersection-observer/\n/html/semantics/popovers/\n/fullscreen/\n/clipboard-apis/\n/close-watcher/\n/resize-observer/\n";

/// The directories this runner runs (recursively), each with what its pages need that this run cannot give them.
/// `the-dialog-element` also runs inside `wpt_dom.rs`'s `wpt2_html_semantics_interactive_elements`; it is listed
/// here so the dialog's own score is read in one place with the popovers' (the two share the top layer).
pub const API_DIRS: &[(&str, &str)] = &[
    ("css/css-typed-om", "nothing infrastructural: every page is testharness over the inline and computed style maps"),
    ("web-animations", "an un-emulated handler (resources/xhr-doc.py); testdriver (three pages)"),
    ("intersection-observer", "cross-origin iframes (.sub.html with REMOTE origins); the rest is the engine's layout"),
    ("html/semantics/interactive-elements/the-dialog-element", "testdriver (clicks, Escape, Tab — 46 pages); iframes of the inert tests"),
    ("html/semantics/popovers", "testdriver (clicks, Escape, Tab, hover — 53 pages); one manual page"),
    ("fullscreen", "testdriver (a user gesture — 75 pages); TLS (.https.); cross-origin iframes (.sub.html); idlharness"),
    ("clipboard-apis", "TLS (.https. — the Clipboard API is [SecureContext], 53 pages); testdriver (permission and gesture); manual pages"),
    ("close-watcher", "testdriver (Escape and user activation — 47 pages)"),
    ("resize-observer", "cross-origin iframes; the rest is the engine's layout"),
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

/// One locate per process (`wpt_dom.rs`: parallel test threads once raced the clone).
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
        if p.join("resources/testharness.js").is_file() {
            return Some(p);
        }
    }
    let t = target_dir().join("wpt");
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(&t).args(args).status().map(|s| s.success()).unwrap_or(false);
    let complete = |t: &Path| API_DIRS.iter().all(|(d, _)| t.join(d).is_dir());
    // A checkout made by wpt_dom.rs lacks these directories: widen it in place (the union sparse set keeps its own).
    if t.join("resources/testharness.js").is_file() && !complete(&t) {
        let _ = std::fs::write(t.join(".git/info/sparse-checkout"), SPARSE);
        let _ = Command::new("git").arg("-C").arg(&t).args(["checkout", "-q", "FETCH_HEAD"]).status();
        let _ = Command::new("git").arg("-C").arg(&t).args(["read-tree", "-mu", "HEAD"]).status();
    }
    if t.join("resources/testharness.js").is_file() && complete(&t) {
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

/// The local helper scripts a test loads (`<script src>`, `// META: script=`), read from the checkout, so a
/// page whose testdriver use lives in a helper is still named (wpt_dom.rs `helper_sources`, AETHERDEBT SR93).
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

/// The `.py` handlers a test source references, as WPT-root paths (wpt_dom.rs `py_refs`).
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

/// Why a page of an API directory is not run (None = run). Infrastructure only; a missing API is never a reason,
/// and neither is layout (the observers are geometry).
fn apis_excluded(root: &Path, rel: &str, src: &str) -> Option<String> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    if ["/crashtests/", "/tools/", "/reference/", "/references/"].iter().any(|x| rel.contains(x)) {
        return Some("not a testharness test (crash test / tool / reference)".into());
    }
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
    if name.contains(".https.") {
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
    if name.contains(".sub.")
        || src.contains("REMOTE_ORIGIN")
        || src.contains("REMOTE_HOST")
        || src.contains("HTTP_REMOTE")
        || src.contains("OTHER_ORIGIN")
        || src.contains("{{domains[")
        || src.contains("{{hosts[")
        || src.contains("get-host-info")
        || name.contains("cross-origin")
        || name.contains("cross-site")
    {
        return Some("cross-origin (no second origin with CORS in the stand-in; .sub. substitution of its hosts)".into());
    }
    for h in py_refs(dir, src) {
        if !wpt_net::PY_HANDLERS.contains(&h.as_str()) {
            return Some(format!("needs wptserve handler {h}"));
        }
    }
    if scan.contains("/media/") {
        return Some("media fixtures (/media/ is not in the sparse checkout)".into());
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

/// One page through the engine until its harness completes (wpt_dom.rs `run_net_file`: virtual time while idle,
/// real time while network operations are in flight, 30 s real / 16 s page time at most).
fn run_page(rt: &tokio::runtime::Runtime, port: u16, rel: &str) -> FileResult {
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

/// Test files (recursively) of `dir` with the variants this runner loads (wpt_dom.rs `net_files`): (path, the
/// variants run, the TLS variants skipped).
fn test_files(root: &Path, dir: &str) -> Vec<(String, Vec<String>, Vec<String>)> {
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

/// Runs every page of `dir` through the stand-in, skipping (with the reason) what [`apis_excluded`] names.
fn run_apis_dir(dir: &str, floor_pct: usize) {
    assert!(API_DIRS.iter().any(|(d, _)| *d == dir), "{dir} is not an AETHERAPIS directory");
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
    println!("wpt {dir}: {} reftest / crash / manual pages (no testharness.js) not run", non_testharness_pages(&root, dir));
    let started = std::time::Instant::now();
    let only = std::env::var("AETHER_WPT_ONLY").ok();
    let verbose = std::env::var("AETHER_WPT_VERBOSE").is_ok();
    let ports = wpt_net::start(root.clone(), REPORT_JS);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let (mut pass, mut total, mut files_run) = (0usize, 0usize, 0usize);
    let mut tsv = String::from("file\tvariant\tpass\ttotal\tharness\tskipped\n");
    let mut skipped: Vec<(String, String)> = Vec::new();
    for (rel, run, tls_variants) in test_files(&root, dir) {
        if only.as_ref().is_some_and(|o| !o.split(',').any(|x| rel.contains(x))) {
            continue;
        }
        let src = std::fs::read_to_string(root.join(&rel)).unwrap_or_default();
        for v in &tls_variants {
            skipped.push((format!("{rel}{v}"), "needs a TLS server (wss / h2 variant)".into()));
            tsv.push_str(&format!("{rel}\t{v}\t0\t0\t-\tneeds a TLS server (wss / h2 variant)\n"));
        }
        if let Some(why) = apis_excluded(&root, &rel, &src) {
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
                format!("{stem}.worker.html{v}")
            } else {
                format!("{rel}{v}")
            };
            let t0 = std::time::Instant::now();
            let r = run_page(&rt, ports.http, &page);
            files_run += 1;
            pass += r.pass;
            total += r.total;
            println!("wpt {:72} {:4}/{:<4} {} ({:.1}s)", format!("{rel}{v}"), r.pass, r.total, r.harness, t0.elapsed().as_secs_f64());
            if verbose {
                for (n, m) in r.failures.iter().take(12) {
                    println!("    FAIL {n}: {}", m.chars().take(200).collect::<String>());
                }
            }
            tsv.push_str(&format!("{rel}\t{v}\t{}\t{}\t{}\t\n", r.pass, r.total, r.harness));
        }
    }
    if only.is_none() {
        let _ = std::fs::write(target_dir().join(format!("wpt-{}.tsv", dir.replace('/', "-"))), tsv);
    }
    let pct = if total == 0 { 0.0 } else { 100.0 * pass as f64 / total as f64 };
    println!("wpt {dir} (served {files_run} test pages): {pass}/{total} subtests ({pct:.1}%); wall {:.0} s", started.elapsed().as_secs_f64());
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

macro_rules! apis_tests {
    ($($name:ident => $dir:literal),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                // Floor 0 at landing (AETHERAPIS, SR145): the bench's first run sets each directory's floor.
                run_apis_dir($dir, 0);
            }
        )*
    };
}

apis_tests! {
    wpt_apis_css_typed_om => "css/css-typed-om",
    wpt_apis_web_animations => "web-animations",
    wpt_apis_intersection_observer => "intersection-observer",
    wpt_apis_dialog => "html/semantics/interactive-elements/the-dialog-element",
    wpt_apis_popovers => "html/semantics/popovers",
    wpt_apis_fullscreen => "fullscreen",
    wpt_apis_clipboard => "clipboard-apis",
    wpt_apis_close_watcher => "close-watcher",
    wpt_apis_resize_observer => "resize-observer",
}

/// Every directory has its `wpt_apis_*` test (the macro list and [`API_DIRS`] agree) and the sparse checkout
/// carries it (the dialog directory through `/html/semantics/interactive-elements/`).
#[test]
fn apis_directory_table_is_complete() {
    for (d, needs) in API_DIRS {
        let carried = SPARSE.lines().any(|l| l == format!("/{d}/") || (l.ends_with('/') && d.starts_with(&l[1..])));
        assert!(carried, "{d} is not in the sparse checkout");
        assert!(!needs.is_empty(), "{d} names what it needs");
    }
    assert_eq!(API_DIRS.len(), 9);
}

/// The exclusion predicate names testdriver through a helper script and never names layout.
#[test]
fn apis_exclusions_are_infrastructure_only() {
    let root = Path::new("/nonexistent");
    assert_eq!(apis_excluded(root, "intersection-observer/x.html", "<script src=testharness.js></script>getBoundingClientRect offsetLeft"), None);
    assert_eq!(apis_excluded(root, "fullscreen/x.html", "test_driver.bless()").as_deref(), Some("testdriver (WebDriver input automation)"));
    assert_eq!(apis_excluded(root, "clipboard-apis/x.https.html", "").as_deref(), Some("needs a TLS server (https)"));
    assert!(apis_excluded(root, "web-animations/x.html", "fetch('resources/xhr-doc.py')").unwrap().contains("xhr-doc.py"));
}
