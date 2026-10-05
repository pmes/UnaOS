//! `js_core::vm::Host` for an Aether page (ECMA-262 §9.5 host-defined operations; HTML §8.1.5).
//!
//! - console output goes to stdout/stderr and the page's console log (kept for tests and the
//!   headless report); errors and warnings are also ledgered;
//! - HostLoadImportedModule resolves a specifier against its referrer (the importing module's URL, or
//!   the document URL for inline module scripts) with the URL Standard parser and fetches the source:
//!   `http(s):` through http_core (the same blocking client `fetch` uses), `file:` from disk, `data:`
//!   inline; bare specifiers are an error (no import maps);
//! - the clock is the wall clock (`Date.now`); local time is UTC (this host has no zone database);
//! - `Math.random` is seeded from the OS entropy pool.

use js_core::vm::{Host, Obj};

/// Most module fetches one page may make.
const MODULE_CAP: u32 = 256;

#[derive(Default)]
pub struct AetherHost {
    modules_fetched: u32,
}

/// Writes one console line (also used by the binding for "report the exception").
pub fn console_out(level: u8, msg: &str) {
    match level {
        1 | 2 => eprintln!("{msg}"),
        _ => println!("{msg}"),
    }
    if level == 2 {
        crate::ledger::record_js(&format!("console.error:{}", super::clip(msg, 64)));
    }
    super::page(|p| {
        if p.console.len() < 10_000 {
            p.console.push((level, msg.to_string()));
        }
    });
}

/// Resolves `specifier` against `base` per the HTML "resolve a module specifier" algorithm (relative
/// and absolute URLs only — no import maps).
pub fn resolve_specifier(base: &str, specifier: &str) -> Result<String, String> {
    let is_relative = specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../");
    if is_relative {
        let b = url::Url::parse(base).map_err(|e| format!("bad module base {base}: {e}"))?;
        return b.join(specifier).map(|u| u.to_string()).map_err(|e| format!("cannot resolve {specifier}: {e}"));
    }
    match url::Url::parse(specifier) {
        Ok(u) => Ok(u.to_string()),
        Err(_) => Err(format!(
            "Failed to resolve module specifier \"{specifier}\". Relative references must start with either \"/\", \"./\", or \"../\"."
        )),
    }
}

/// Fetches a script resource (module or classic) as text: `file:`, `data:` and `http(s):` URLs.
pub fn fetch_script_text(url: &str) -> Result<String, String> {
    if let Some(path) = url.strip_prefix("file://") {
        let path = percent_decode(path.split(['?', '#']).next().unwrap_or(path));
        return std::fs::read_to_string(&path).map_err(|e| format!("{url}: {e}"));
    }
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, body) = rest.split_once(',').ok_or_else(|| format!("bad data URL {url}"))?;
        if meta.ends_with(";base64") {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD.decode(body.trim()).map_err(|e| e.to_string())?;
            return String::from_utf8(bytes).map_err(|e| e.to_string());
        }
        return Ok(percent_decode(body));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        let u = url.to_string();
        let r = std::thread::spawn(move || -> Result<String, String> {
            let client = crate::net::blocking_client_builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client.get(&u).send().map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            if !(200..300).contains(&status) {
                return Err(format!("HTTP {status} for {u}"));
            }
            let text = resp.text().map_err(|e| e.to_string())?;
            if text.len() > 4 * 1024 * 1024 {
                return Err(format!("{u}: script larger than 4 MiB"));
            }
            Ok(text)
        })
        .join()
        .map_err(|_| "fetch thread panicked".to_string())?;
        return r;
    }
    Err(format!("unsupported script URL scheme: {url}"))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Host for AetherHost {
    fn console(&mut self, level: u8, msg: &str) {
        console_out(level, msg);
    }

    fn load_module(&mut self, referrer: Option<&str>, specifier: &str) -> Result<(String, String), String> {
        let base = match referrer {
            Some(r) => r.to_string(),
            None => super::page_url(),
        };
        let resolved = resolve_specifier(&base, specifier)?;
        // A module already in the page's pre-fetched set needs no request.
        if let Some(src) = super::page(|p| p.script_sources.get(&resolved).cloned()) {
            return Ok((resolved, src));
        }
        self.modules_fetched += 1;
        if self.modules_fetched > MODULE_CAP {
            crate::ledger::record_js("module-fetch-cap-reached");
            return Err(format!("module fetch cap reached loading {resolved}"));
        }
        let src = fetch_script_text(&resolved).map_err(|e| {
            crate::ledger::record_js(&format!("module-fetch-failed:{}", super::clip(&resolved, 48)));
            e
        })?;
        Ok((resolved, src))
    }

    fn now_ms(&mut self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .map(|ms| ms.floor())
            .unwrap_or(0.0)
    }

    fn promise_rejection(&mut self, _promise: Obj, _operation: u8) {}

    fn random_seed(&mut self) -> u64 {
        use std::io::Read;
        let mut b = [0u8; 8];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(&mut b).is_ok() {
                return u64::from_le_bytes(b) | 1;
            }
        }
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        t | 1
    }
}
