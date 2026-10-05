//! AETHERJS (SR63): the WPT `dom/nodes` directory run through Aether on js_core.
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
//! Reported: pass/total of subtests over every `.html`/`.htm`/`.window.js` file of `dom/nodes`, and over
//! "the subset Aether's surface covers" — the same files minus those that need a feature this binding
//! does not have (listed with the reason in [`excluded`]). XML documents (`.xhtml`, `.svg`, `.xml`) are
//! not run: Aether parses HTML only. Per-file results go to `target/wpt-dom-nodes.tsv`.
//! `AETHER_WPT_ONLY=<substring>` runs matching files; `AETHER_WPT_VERBOSE=1` prints failing subtests.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

const WPT_REPO: &str = "https://github.com/web-platform-tests/wpt.git";
const WPT_COMMIT: &str = "da1f6d20caf40b4003ae288dda8c97f252dd9264";

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

fn locate() -> Option<PathBuf> {
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
    if t.join("dom/nodes").is_dir() && t.join("resources/testharness.js").is_file() {
        return Some(t);
    }
    let _ = std::fs::create_dir_all(&t);
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(&t).args(args).status().map(|s| s.success()).unwrap_or(false);
    let ok = git(&["init", "-q"])
        && git(&["config", "core.sparseCheckout", "true"])
        && std::fs::write(t.join(".git/info/sparse-checkout"), "/dom/nodes/\n/dom/*.js\n/resources/\n/common/\n/html/resources/\n").is_ok()
        && git(&["fetch", "-q", "--depth", "1", "--filter=blob:none", WPT_REPO, WPT_COMMIT])
        && git(&["checkout", "-q", "FETCH_HEAD"]);
    ok.then_some(t)
}

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
                } else if let Some(stem) = rel.strip_suffix(".window.html") {
                    let js = format!("/{stem}.window.js");
                    let src = std::fs::read_to_string(root.join(format!("{stem}.window.js"))).unwrap_or_default();
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

/// Files of `dom/nodes` that need what this binding does not provide, with the reason.
fn excluded(name: &str, src: &str) -> Option<&'static str> {
    if name.starts_with("MutationObserver") || src.contains("MutationObserver(") || src.contains("mutationobservers.js") {
        return Some("MutationObserver (a stub that never delivers)");
    }
    if src.contains("<iframe") || src.contains("createElement(\"iframe\")") || src.contains("createElement('iframe')") || src.contains("window.open(") {
        return Some("frames / browsing contexts");
    }
    if src.contains("attachShadow") || src.contains("shadowRoot") {
        return Some("shadow DOM");
    }
    if src.contains("customElements.define") {
        return Some("custom elements");
    }
    if name.contains("moveBefore") {
        return Some("moveBefore (2025 proposal)");
    }
    if src.contains("createRange") || src.contains("new Range") {
        return Some("Range");
    }
    if src.contains("createNodeIterator") || src.contains("createTreeWalker") {
        return Some("NodeIterator / TreeWalker");
    }
    if src.contains(".xhtml") || src.contains(".xml\"") || src.contains("XMLHttpRequest") {
        return Some("XML documents");
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

#[test]
fn wpt_dom_nodes() {
    let Some(root) = locate() else {
        println!("wpt dom/nodes: skipped (no checkout, offline)");
        return;
    };
    let dir = root.join("dom/nodes");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".html") || n.ends_with(".htm") || n.ends_with(".window.js"))
        .collect();
    files.sort();
    let only = std::env::var("AETHER_WPT_ONLY").ok();
    let verbose = std::env::var("AETHER_WPT_VERBOSE").is_ok();
    let port = serve(root.clone());
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let (mut all_pass, mut all_total, mut cov_pass, mut cov_total) = (0, 0, 0, 0);
    let (mut files_run, mut files_cov) = (0, 0);
    let mut tsv = String::from("file\tpass\ttotal\tharness\texcluded\n");
    let mut excluded_list: Vec<(String, &'static str)> = Vec::new();
    for f in &files {
        if only.as_ref().is_some_and(|o| !o.split(',').any(|x| f.contains(x))) {
            continue;
        }
        let src = std::fs::read_to_string(dir.join(f)).unwrap_or_default();
        // Support pages that are not tests.
        if !src.contains("testharness.js") && !f.ends_with(".window.js") {
            continue;
        }
        let rel = if let Some(stem) = f.strip_suffix(".window.js") {
            format!("dom/nodes/{stem}.window.html")
        } else {
            format!("dom/nodes/{f}")
        };
        let ex = excluded(f, &src);
        let r = run_file(&rt, port, &rel);
        files_run += 1;
        all_pass += r.pass;
        all_total += r.total;
        if ex.is_none() {
            files_cov += 1;
            cov_pass += r.pass;
            cov_total += r.total;
        } else {
            excluded_list.push((f.clone(), ex.unwrap()));
        }
        println!("wpt {:58} {:4}/{:<4} {}{}", f, r.pass, r.total, r.harness, ex.map(|e| format!("  [excluded: {e}]")).unwrap_or_default());
        if verbose {
            for (n, m) in r.failures.iter().take(12) {
                println!("    FAIL {n}: {}", m.chars().take(160).collect::<String>());
            }
        }
        tsv.push_str(&format!("{f}\t{}\t{}\t{}\t{}\n", r.pass, r.total, r.harness, ex.unwrap_or("")));
    }
    let _ = std::fs::write(target_dir().join("wpt-dom-nodes.tsv"), tsv);
    let pct = |p: usize, t: usize| if t == 0 { 0.0 } else { 100.0 * p as f64 / t as f64 };
    println!("wpt dom/nodes (all {files_run} files):      {all_pass}/{all_total} subtests ({:.1}%)", pct(all_pass, all_total));
    println!("wpt dom/nodes (covered {files_cov} files):  {cov_pass}/{cov_total} subtests ({:.1}%)", pct(cov_pass, cov_total));
    let mut reasons: Vec<&str> = excluded_list.iter().map(|(_, r)| *r).collect();
    reasons.sort();
    reasons.dedup();
    for r in reasons {
        println!("  excluded for {r}: {}", excluded_list.iter().filter(|(_, x)| *x == r).count());
    }
    if only.is_none() {
        // Regression floor (set from the measured run; raise it as the surface grows).
        assert!(cov_pass * 100 >= cov_total * 80, "covered dom/nodes subtests fell below 80%");
    }
}
