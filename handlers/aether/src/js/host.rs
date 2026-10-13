//! `js_core::vm::Host` for an Aether page (ECMA-262 §9.5 host-defined operations; HTML §8.1.5).
//!
//! - console output goes to stdout/stderr and the page's console log (kept for tests and the
//!   headless report); errors and warnings are also ledgered;
//! - HostLoadImportedModule resolves a specifier against its referrer (the importing module's URL, or
//!   the document URL for inline module scripts) with the URL Standard parser and fetches the source:
//!   `http(s):` through http_core (the same blocking client `fetch` uses), `file:` from disk, `data:`
//!   inline; specifiers resolve through the page's import map (`importmap`, HTML §8.1.5.3 — AETHERJS2), whose
//!   integrity metadata the module fetch carries; `import.meta.resolve` is the same resolution;
//! - HostPromiseRejectionTracker: a rejection that was reported unhandled and is handled later queues the
//!   `rejectionhandled` task (HTML §8.1.5.7);
//! - the clock is the wall clock (`Date.now`); local time is UTC (this host has no zone database);
//! - `Math.random` is seeded from the OS entropy pool.

use html_core::url;
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

/// Resolves `specifier` against `base` per HTML "resolve a module specifier" (§8.1.5.3): the page's import
/// map first (scopes, then top-level imports), then relative and absolute URLs. An inline module script's
/// name carries an `#inline-module-N` fragment; its base URL is the document's, so that is stripped.
pub fn resolve_specifier(base: &str, specifier: &str) -> Result<String, String> {
    let base = match base.find("#inline-module-") {
        Some(i) => &base[..i],
        None => base,
    };
    if url::Url::parse(base).is_err() {
        return Err(format!("bad module base {base}"));
    }
    super::importmap::resolve(base, specifier)
}

/// Fetches a script resource (module or classic) as text: `file:`, `data:` and `http(s):` URLs.
pub fn fetch_script_text(url: &str) -> Result<String, String> {
    fetch_script(url, false, None, "")
}

/// [`fetch_script_text`] with the element's fetch options (AETHERORIGIN, SR86): a module, or a classic script
/// with `crossorigin`, is a `cors` request (an `Origin` header, the CORS check on the answer); a plain classic
/// script is `no-cors` with credentials. `http(s):` goes through the policed exchange (`api::fetch`):
/// `Referer` under the document's referrer policy, `Sec-Fetch-Dest: script`, per-hop mixed content, cookies
/// under SameSite, CORS / CORP; `integrity` (SRI) is checked on the bytes before they become text.
pub fn fetch_script(url: &str, module: bool, crossorigin: Option<&str>, integrity: &str) -> Result<String, String> {
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
        let ctx = crate::security::RequestContext::for_script(module, crossorigin, integrity);
        let u = ctx.precheck(url, "GET")?;
        let req = crate::api::fetch::HttpReq {
            method: "GET".into(),
            url: u,
            headers: vec![("accept".into(), "*/*".into())],
            body: Vec::new(),
            redirect: "follow".into(),
            timeout_ms: 10_000,
        };
        let r = std::thread::spawn(move || -> Result<String, String> {
            let ex = crate::api::fetch::policed_send(&req, &ctx)?;
            let status = ex.resp.status().as_u16();
            if !(200..300).contains(&status) {
                return Err(format!("HTTP {status} for {}", req.url));
            }
            let opaque = ex.kind == "opaque";
            let bytes = ex.resp.bytes().map_err(|e| e.to_string())?;
            if bytes.len() > 4 * 1024 * 1024 {
                return Err(format!("{}: script larger than 4 MiB", req.url));
            }
            if !ctx.integrity.is_empty() {
                crate::security::sri::check(&ctx.integrity, &bytes, !opaque).map_err(|e| {
                    crate::security::refusal("sri", &req.url, &format!("Failed to find a valid digest in the 'integrity' attribute for resource '{}': {e}. The resource has been blocked.", req.url), None)
                })?;
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        })
        .join()
        .map_err(|_| "fetch thread panicked".to_string())?;
        crate::security::drain();
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
        // CSP script-src-elem and mixed content for the import (AETHERORIGIN, SR86).
        crate::security::module_import_allowed(&resolved)?;
        // A module already in the page's pre-fetched set needs no request.
        if let Some(src) = super::page(|p| p.script_sources.get(&resolved).cloned()) {
            return Ok((resolved, src));
        }
        self.modules_fetched += 1;
        if self.modules_fetched > MODULE_CAP {
            crate::ledger::record_js("module-fetch-cap-reached");
            return Err(format!("module fetch cap reached loading {resolved}"));
        }
        let integrity = super::importmap::integrity_for(&resolved);
        let src = fetch_script(&resolved, true, None, &integrity).map_err(|e| {
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

    /// HostPromiseRejectionTracker "handle" (HTML §8.1.5.7): a promise in the outstanding rejected promises
    /// set (its `unhandledrejection` already fired) gets a handler — queue `rejectionhandled`.
    fn promise_rejection(&mut self, promise: Obj, operation: u8) {
        if operation == 1 {
            super::rejection_handled(promise);
        }
    }

    fn has_import_meta_resolve(&self) -> bool {
        true
    }

    fn resolve_module_specifier(&mut self, referrer: Option<&str>, specifier: &str) -> Result<String, String> {
        let base = match referrer {
            Some(r) => r.to_string(),
            None => super::page_url(),
        };
        resolve_specifier(&base, specifier)
    }

    /// HostEnsureCanCompileStrings (AETHERDEBT SR93): the document's CSP decides eval / new Function
    /// (`'unsafe-eval'`); the refusal is reported (console, ledger, report POSTs, the violation event
    /// queued for the next script) and js_core throws the EvalError. Replaces the JS eval guard.
    fn ensure_can_compile_strings(&mut self, source: &str) -> Result<(), String> {
        crate::security::compile_strings_allowed(source)
    }

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
