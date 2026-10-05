//! AETHERJS (SR63) script oracle: small pages exercising the DOM binding, the event loop and HTML's
//! script processing model run in Aether (on js_core) and in Chromium; the serialized DOM after load
//! must be byte-equal to Chromium's `--dump-dom`.
//!
//! The pages (`tests/data/script_oracle/NN-name.html`, plus the `.js` files they load) are served over
//! HTTP by a tiny static server on 127.0.0.1 so both browsers take the real network path (Aether: its
//! http_core page fetch, the module loader, `<script src>`; Chromium: no file:// CORS restrictions on
//! modules). Aether: `net::fetch_page` → `AetherEngine::load_page` → the event loop run in virtual time
//! to idle (`event_loop::settle`, 10 s budget — what `--virtual-time-budget=10000` does) → the document
//! element's outerHTML. Chromium (`chromium_headless_shell-1194 --dump-dom --virtual-time-budget=10000
//! --window-size=800,600 --hide-scrollbars`) dumps were recorded as `NN-name.chromium.html`;
//! `AETHER_ORACLE_RECORD=1` re-records them when the browser is present. `AETHER_ORACLE_ONLY=NN` runs
//! one page.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/script_oracle")
}

const CHROMIUM: &str = "/opt/pw-browsers/chromium_headless_shell-1194/chrome-linux/headless_shell";

/// A static file server for `dir` (GET only, Connection: close). Returns the port.
fn serve(dir: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let dir = dir.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                // drain headers
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h == "\r\n" || h.is_empty() {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").split(['?', '#']).next().unwrap_or("/").to_string();
                let rel = path.trim_start_matches('/');
                let file = dir.join(rel);
                let ok = !rel.contains("..") && file.is_file();
                let (status, body, ty) = if ok {
                    let ty = match file.extension().and_then(|e| e.to_str()) {
                        Some("html") => "text/html; charset=utf-8",
                        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
                        Some("css") => "text/css; charset=utf-8",
                        Some("json") => "application/json",
                        _ => "application/octet-stream",
                    };
                    ("200 OK", std::fs::read(&file).unwrap(), ty)
                } else {
                    ("404 Not Found", b"not found".to_vec(), "text/plain")
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            });
        }
    });
    port
}

fn chromium_dump(url: &str) -> Option<String> {
    let profile = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/script-oracle-chromium-profile");
    let out = std::process::Command::new(CHROMIUM)
        .args([
            "--no-sandbox",
            "--disable-gpu",
            "--hide-scrollbars",
            "--window-size=800,600",
            "--virtual-time-budget=10000",
            "--dump-dom",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(url)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Loads one page in Aether and returns its document element's outerHTML after the event loop idles.
fn aether_dump(rt: &tokio::runtime::Runtime, url: &str) -> String {
    let page = rt.block_on(aether::net::fetch_page(url)).expect("page fetch");
    let mut engine = aether::AetherEngine::new();
    engine.load_page(page, true);
    if let Some(js) = engine.js_engine.as_mut() {
        aether::event_loop::settle(js, 10_000);
    }
    let doc = engine.document.clone().expect("document");
    // headless_shell's --dump-dom: the doctype (XMLSerializer form) and a newline, then the document
    // element's outerHTML.
    let mut out = String::new();
    if let Some(dt) = doc.children().find_map(|c| c.as_doctype()) {
        out.push_str("<!DOCTYPE ");
        out.push_str(&dt.name);
        if !dt.public_id.is_empty() {
            out.push_str(&format!(" PUBLIC \"{}\"", dt.public_id));
        }
        if !dt.system_id.is_empty() {
            if dt.public_id.is_empty() {
                out.push_str(" SYSTEM");
            }
            out.push_str(&format!(" \"{}\"", dt.system_id));
        }
        out.push_str(">\n");
    }
    out.push_str(&doc.children().find(|c| c.as_element().is_some()).map(|e| e.outer_html()).unwrap_or_default());
    out
}

#[test]
fn script_pages_serialize_like_chromium() {
    let dir = data_dir();
    let mut pages: Vec<String> = std::fs::read_dir(&dir)
        .expect("oracle pages")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "html") && !p.to_string_lossy().ends_with(".chromium.html"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
        .collect();
    pages.sort();
    let only = std::env::var("AETHER_ORACLE_ONLY").ok();
    let record = std::env::var("AETHER_ORACLE_RECORD").is_ok() && Path::new(CHROMIUM).exists();
    let port = serve(dir.clone());
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut fails = Vec::new();
    let mut equal = 0;
    for name in &pages {
        if only.as_ref().is_some_and(|o| !name.starts_with(o.as_str())) {
            continue;
        }
        let url = format!("http://127.0.0.1:{port}/{name}.html");
        let golden = dir.join(format!("{name}.chromium.html"));
        if record {
            if let Some(d) = chromium_dump(&url) {
                std::fs::write(&golden, format!("{d}\n")).unwrap();
            }
        }
        let Ok(chromium) = std::fs::read_to_string(&golden) else {
            println!("script-oracle {name:28} (no Chromium reference)");
            fails.push(name.clone());
            continue;
        };
        let chromium = chromium.trim_end().to_string();
        let ours = aether_dump(&rt, &url);
        if let Ok(d) = std::env::var("AETHER_ORACLE_DUMP") {
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(Path::new(&d).join(format!("{name}.aether.html")), format!("{ours}\n"));
        }
        if ours == chromium {
            equal += 1;
            println!("script-oracle {name:28} {:6} bytes  EQUAL", ours.len());
        } else {
            println!("script-oracle {name:28} {:6} bytes  DIFF (chromium {} bytes)", ours.len(), chromium.len());
            let a: Vec<&str> = ours.split('\n').collect();
            let b: Vec<&str> = chromium.split('\n').collect();
            for i in 0..a.len().max(b.len()) {
                let (x, y) = (a.get(i).copied().unwrap_or(""), b.get(i).copied().unwrap_or(""));
                if x != y {
                    let k = x.bytes().zip(y.bytes()).take_while(|(p, q)| p == q).count();
                    let lo = k.saturating_sub(60);
                    println!("  line {}: first difference at byte {k}", i + 1);
                    println!("    aether:   {}", &x.get(lo..(k + 160).min(x.len())).unwrap_or(x));
                    println!("    chromium: {}", &y.get(lo..(k + 160).min(y.len())).unwrap_or(y));
                    break;
                }
            }
            fails.push(name.clone());
        }
    }
    println!("script-oracle: {equal}/{} byte-equal", equal + fails.len());
    assert!(fails.is_empty(), "serialized DOM differs from Chromium on {fails:?}");
}
