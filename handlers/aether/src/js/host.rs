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
    // An unparsable base (a page with no URL) still resolves absolute specifiers and mapped bare ones;
    // only a relative specifier needs it, and importmap::resolve refuses that one with a TypeError.
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
    if let Some(r) = local_script(url) {
        return r.map(|(text, _)| text);
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

/// A `file:` or `data:` script: Some((text, content type)) — the `data:` URL's MIME type, "" for a file —
/// or None for any other scheme.
fn local_script(url: &str) -> Option<Result<(String, String), String>> {
    if let Some(path) = url.strip_prefix("file://") {
        let path = percent_decode(path.split(['?', '#']).next().unwrap_or(path));
        return Some(std::fs::read_to_string(&path).map(|t| (t, String::new())).map_err(|e| format!("{url}: {e}")));
    }
    let rest = url.strip_prefix("data:")?;
    let Some((meta, body)) = rest.split_once(',') else { return Some(Err(format!("bad data URL {url}"))) };
    let ctype = meta.strip_suffix(";base64").unwrap_or(meta).to_string();
    if meta.ends_with(";base64") {
        use base64::Engine as _;
        let r = base64::engine::general_purpose::STANDARD.decode(body.trim()).map_err(|e| e.to_string()).and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()));
        return Some(r.map(|t| (t, ctype)));
    }
    Some(Ok((percent_decode(body), ctype)))
}

/// The JavaScript MIME type essences (MIME Sniffing §4.6), the set `js::loader`'s script-type check uses.
pub const JS_MIME_ESSENCES: &[&str] = &[
    "application/ecmascript", "application/javascript", "application/x-ecmascript", "application/x-javascript",
    "text/ecmascript", "text/javascript", "text/javascript1.0", "text/javascript1.1", "text/javascript1.2",
    "text/javascript1.3", "text/javascript1.4", "text/javascript1.5", "text/jscript", "text/livescript",
    "text/x-ecmascript", "text/x-javascript",
];

/// Whether a `Content-Type` value's essence is a JavaScript MIME type.
pub fn is_javascript_mime(content_type: &str) -> bool {
    let essence = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    JS_MIME_ESSENCES.contains(&essence.as_str())
}

/// AETHERWORKERS2 (SR125): the page's request context for a worker script (page thread): HTML §10.2.4 "fetch
/// a classic / module worker script" — destination `worker` (so `Sec-Fetch-Dest: worker` and CSP
/// `worker-src` in [`RequestContext::check_hop`](crate::security::RequestContext::check_hop)), mode
/// `same-origin`, credentials `same-origin`, the document as client and referrer. The worker thread keeps a
/// clone (re-based on the worker's own URL) for its imports and its nested workers.
pub fn worker_request_context() -> crate::security::RequestContext {
    let mut ctx = crate::security::RequestContext::for_script(true, None, "");
    ctx.destination = "worker".into();
    ctx.mode = "same-origin".into();
    ctx.credentials = "same-origin".into();
    ctx
}

/// AETHERWORKERS2 (SR125): fetches a worker's script — or one script of its graph (`importScripts`, a module
/// worker's imports) — under `ctx`, from any thread: (text, content type). `file:` and `data:` read locally;
/// `http(s):` goes through the policed exchange with `ctx`'s destination, mode and credentials (precheck:
/// same-origin mode, mixed content, CSP for the destination), and refuses a response whose MIME type is not
/// a JavaScript MIME type (HTML §10.2.4 / §10.3.4.1: "if response's URL's scheme is an HTTP(S) scheme and the
/// result of extracting a MIME type from response's header list is not a JavaScript MIME type", the fetch
/// fails) — the error names the type. Refusals off the page thread queue for `security::drain`; the page
/// caller drains.
pub fn fetch_worker_script(url: &str, ctx: &crate::security::RequestContext) -> Result<(String, String), String> {
    fetch_worker_script_full(url, ctx).map(|(text, ctype, _)| (text, ctype))
}

/// AETHERWORKERS3 (SR129): [`fetch_worker_script`] answering the response's header list too — (text, content
/// type, headers; empty for `file:` / `data:`) — so the worker thread can build the worker's OWN policy
/// container from it (HTML §10.2.4 "run a worker" step 13.3 / CSP3 §4.2.2: a worker whose script came over
/// http(s) takes the `Content-Security-Policy` of its script response, a local-scheme worker inherits its
/// creator's).
pub fn fetch_worker_script_full(url: &str, ctx: &crate::security::RequestContext) -> Result<(String, String, Vec<(String, String)>), String> {
    if let Some(r) = local_script(url) {
        return r.map(|(text, ctype)| (text, ctype, Vec::new()));
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("unsupported script URL scheme: {url}"));
    }
    let u = ctx.precheck(url, "GET")?;
    let req = crate::api::fetch::HttpReq {
        method: "GET".into(),
        url: u,
        headers: vec![("accept".into(), "*/*".into())],
        body: Vec::new(),
        redirect: "follow".into(),
        timeout_ms: 10_000,
    };
    let ctx = ctx.clone();
    std::thread::spawn(move || -> Result<(String, String, Vec<(String, String)>), String> {
        let ex = crate::api::fetch::policed_send(&req, &ctx)?;
        let status = ex.resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status} for {}", req.url));
        }
        let ctype = ex.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.clone()).unwrap_or_default();
        if !is_javascript_mime(&ctype) {
            let essence = ctype.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            return Err(crate::security::refusal(
                "worker-script-mime",
                &req.url,
                &format!("Refused to execute script from '{}' because its MIME type ('{essence}') is not executable, and strict MIME type checking is enabled.", req.url),
                None,
            ));
        }
        let bytes = ex.resp.bytes().map_err(|e| e.to_string())?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(format!("{}: script larger than 4 MiB", req.url));
        }
        Ok((String::from_utf8_lossy(&bytes).into_owned(), ctype, ex.headers))
    })
    .join()
    .map_err(|_| "fetch thread panicked".to_string())?
}

/// A refusal string (`security::ERR_PREFIX` message `\u{1}` event) as its message alone.
pub fn refusal_message(e: &str) -> String {
    match e.strip_prefix(crate::security::ERR_PREFIX) {
        Some(rest) => rest.split('\u{1}').next().unwrap_or(rest).to_string(),
        None => e.to_string(),
    }
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
