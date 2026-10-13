//! The Fetch Standard's page-facing half (AETHERNET, LEDGER SR70): `fetch()` whose `Response.body` is a
//! `ReadableStream` fed chunk by chunk as the connection delivers it, `Headers`, `Request`, `Response`,
//! the Streams Standard's default `ReadableStream` / reader / controller, `AbortController` /
//! `AbortSignal` wired through, and a minimal `Blob` / `FileReader`.
//!
//! The worker half is here too: [`spawn_http`] runs one exchange on its own thread through
//! gneiss_pal::api::http's blocking client (the shared cookie jar, http_core + tls_core underneath) and
//! posts `head` / `data` / `end` | `error` messages into the page's network queue (`js::net`), so script
//! sees the first chunk while the server is still sending the rest. `XMLHttpRequest` (`api::xhr`) runs on
//! the same worker, and its synchronous mode on [`http_sync`].
//!
//! AETHERCACHE (SR88): `Request.cache` is honoured — the prelude passes the Fetch cache mode and the top-level
//! site to the worker as two pseudo-fields (`:cache-mode`, `:top-level-site`: names no page can set, since a
//! `:` is not a token character and `Headers` refuses it), which [`send`] lifts off the field list onto the
//! blocking client (`cache_mode`, `partition`) — the shared RFC 9111 cache of `crate::net::http_cache`.

use crate::js::net::{NetArg, Poster};
use std::time::Duration;
use html_core::url;

/// Most bytes a synchronous XHR buffers (an asynchronous body streams and has no cap).
const SYNC_BODY_CAP: usize = 64 * 1024 * 1024;

/// Kept for `Engine::with_viewport` (the per-page budget became `js::net::MAX_OPS` in flight).
pub fn reset_budget() {}

/// The platform-prelude hook `window::install` calls; the network preludes install from
/// `js::net::install` (after the platform prelude, which they supersede).
pub fn init(_vm: &mut js_core::vm::Vm) {}

/// One request as the page built it.
pub struct HttpReq {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The request's redirect mode: `follow`, `error` or `manual`.
    pub redirect: String,
    /// 0 = none (XHR `timeout` is enforced on the page clock; this is the transport's idle limit).
    pub timeout_ms: u64,
}

/// The transport for one policed exchange: NO cookie jar (cookies are added and stored per hop under the
/// hop's SameSite / partition context, [`crate::security::cookie_header`]) and NO automatic redirects (each
/// hop is re-checked: mixed content, CSP, the CORS check on the redirect, the tainted origin).
fn client(req: &HttpReq) -> Result<gneiss_pal::api::http::blocking::Client, String> {
    gneiss_pal::api::http::blocking::ClientBuilder::new()
        .user_agent("UnaOS Aether/0.1.0")
        // A long poll or a server-sent stream over fetch may sit idle for minutes between chunks.
        .timeout(Duration::from_millis(if req.timeout_ms > 0 { req.timeout_ms } else { 300_000 }))
        .max_redirects(0)
        .build()
        .map_err(|e| e.to_string())
}

fn header_list(h: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::with_capacity(h.len() * 2);
    for (k, v) in h {
        out.push(k.clone());
        out.push(v.clone());
    }
    out
}

/// One policed exchange's answer: the response (body unread), its type (`basic` | `cors` | `opaque`), the
/// headers script may see, the final URL and whether a redirect was followed.
pub struct Exchange {
    pub resp: gneiss_pal::api::http::blocking::Response,
    pub kind: &'static str,
    pub headers: Vec<(String, String)>,
    pub url: String,
    pub redirected: bool,
}

fn is_redirect(st: u16) -> bool {
    matches!(st, 301 | 302 | 303 | 307 | 308)
}

/// Fetch §4.9 CORS-preflight fetch: `OPTIONS` with `Access-Control-Request-*`, never credentials.
fn preflight(
    c: &gneiss_pal::api::http::blocking::Client,
    ctx: &crate::security::RequestContext,
    url: &str,
    method: &str,
    headers: &[(String, String)],
    origin: &str,
    include: bool,
    site: crate::security::origin::FetchSite,
) -> Result<(), String> {
    use crate::security::cors;
    let mut rb = c.request("OPTIONS", url);
    for (k, v) in cors::preflight_headers(method, headers) {
        rb = rb.header(k, v);
    }
    for (k, v) in ctx.hop_headers(url, "OPTIONS", site, true, origin) {
        rb = rb.header(k, v);
    }
    let resp = rb.send().map_err(|e| e.to_string())?;
    let st = resp.status().as_u16();
    let rh: Vec<(String, String)> = resp.headers().iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
    drop(resp);
    cors::check_preflight(st, &rh, origin, url, include, method, headers).map_err(|e| {
        crate::security::refusal("cors-preflight", url, &format!("Access to fetch at '{url}' from origin '{origin}' has been blocked by CORS policy: {e}."), None)
    })
}

/// The policy hooks at request build and response receipt (AETHERORIGIN, SR86): Fetch §4.1 main fetch,
/// §4.4 HTTP-redirect fetch and §4.5 HTTP-network-or-cache fetch as far as this engine needs them. Per hop:
/// the `same-origin` mode, response tainting (`basic` / `cors` / `opaque`, sticky across redirects), the
/// preflight (cached), `Origin` (`null` once a cross-origin redirect taints it), `Referer` under the
/// referrer policy, `Sec-Fetch-*`, cookies by credentials mode and SameSite context; on a redirect the CORS
/// check, the method rewrite, `Authorization` dropped across origins, and the next hop's mixed-content and
/// CSP checks; on the final answer the CORS check (`cors`), CORP (`opaque`), and the header filter.
pub fn policed_send(req: &HttpReq, ctx: &crate::security::RequestContext) -> Result<Exchange, String> {
    use crate::security::{cors, origin::FetchSite, origin::Origin, refusal};
    let c = client(req)?;
    let mut url = req.url.clone();
    let mut method = req.method.clone();
    let mut body = req.body.clone();
    let mut headers = req.headers.clone();
    let mut tainted = false;
    let mut tainting: &'static str = "basic";
    let mut site = FetchSite::None;
    let mut redirected = false;
    for hop in 0..=20u32 {
        if hop > 0 {
            url = ctx.check_hop(&url, true)?;
        }
        let target = Origin::of(&url);
        let same = !ctx.has_page || ctx.client_origin.same_origin(&target);
        if ctx.has_page && !same {
            match ctx.mode.as_str() {
                "same-origin" => {
                    return Err(refusal("same-origin-mode", &url, &format!("Request mode is \"same-origin\" but a redirect led to {}.", target.serialize()), None));
                }
                "cors" if tainting == "basic" => tainting = "cors",
                "no-cors" if tainting == "basic" => tainting = "opaque",
                _ => {}
            }
        }
        if tainting == "cors" && url::Url::parse(&url).is_ok_and(|u| !u.username().is_empty() || !u.password().is_empty()) {
            return Err(refusal("cors-url-credentials", &url, &format!("A CORS request to '{url}' may not carry credentials in its URL."), None));
        }
        if ctx.has_page {
            site = site.step(&ctx.client_origin, &target);
        }
        let include = ctx.include_credentials(&target);
        let origin_ser = if tainted { "null".to_string() } else { ctx.client_origin.serialize() };
        if tainting == "cors" && cors::needs_preflight(&method, &headers) && !cors::preflight_cached(&origin_ser, &url, include, &method, &headers) {
            preflight(&c, ctx, &url, &method, &headers, &origin_ser, include, site)?;
        }
        let mut rb = c.request(&method, &url);
        for (k, v) in &headers {
            // AETHERCACHE (SR88): the cache mode and the top-level site ride as pseudo-fields (names no page
            // can set); `RequestContext::capture` leaves exactly these two in the list for this hop to lift.
            match k.as_str() {
                ":cache-mode" => rb = rb.cache_mode(gneiss_pal::api::http::CacheMode::parse(v).unwrap_or_default()),
                ":top-level-site" => {
                    if let Some(site) = gneiss_pal::api::http::site_of(v) {
                        rb = rb.partition(site);
                    }
                }
                _ => rb = rb.header(k, v),
            }
        }
        for (k, v) in ctx.hop_headers(&url, &method, site, tainting == "cors", &origin_ser) {
            rb = rb.header(k, v);
        }
        if include {
            if let Some(ck) = crate::security::cookie_header(ctx, &url, &method) {
                rb = rb.header("cookie", ck);
            }
        }
        if !body.is_empty() || !matches!(method.as_str(), "GET" | "HEAD") {
            rb = rb.body(body.clone());
        }
        let resp = rb.send().map_err(|e| e.to_string())?;
        let st = resp.status().as_u16();
        let rh: Vec<(String, String)> = resp.headers().iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
        if include {
            crate::security::store_cookies(ctx, &url, &method, rh.iter().filter(|(k, _)| k == "set-cookie").map(|(_, v)| v.as_str()));
        }
        let location = if is_redirect(st) { cors::get(&rh, "location").map(str::to_string) } else { None };
        if let Some(loc) = location {
            match req.redirect.as_str() {
                "error" => return Err("redirect in redirect mode \"error\"".into()),
                "manual" => {
                    let filtered = cors::filter_response_headers(&rh, "basic", include);
                    return Ok(Exchange { resp, kind: tainting, headers: filtered, url, redirected });
                }
                _ => {}
            }
            if tainting == "cors" {
                cors::cors_check(&rh, &origin_ser, include).map_err(|e| {
                    refusal("cors-redirect", &url, &format!("Access to fetch at '{url}' (redirected) from origin '{origin_ser}' has been blocked by CORS policy: {e}."), None)
                })?;
            }
            let next = url::Url::parse(&url).and_then(|b| b.join(&loc)).map_err(|e| format!("bad redirect Location {loc}: {e}"))?;
            if !matches!(next.scheme(), "http" | "https") {
                return Err(format!("redirect to unsupported scheme {}", next.scheme()));
            }
            let next = next.to_string();
            if ((st == 301 || st == 302) && method == "POST") || (st == 303 && !matches!(method.as_str(), "GET" | "HEAD")) {
                method = "GET".into();
                body.clear();
                headers.retain(|(k, _)| !matches!(k.to_ascii_lowercase().as_str(), "content-type" | "content-encoding" | "content-language" | "content-location"));
            }
            let cur = Origin::of(&url);
            let nxt = Origin::of(&next);
            if !cur.same_origin(&nxt) {
                headers.retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
                // Fetch §4.4 step 14: a hop between two origins neither of which is the request's taints it.
                if ctx.has_page && !ctx.client_origin.same_origin(&cur) {
                    tainted = true;
                }
            }
            drop(resp);
            url = next;
            redirected = true;
            continue;
        }
        if tainting == "cors" {
            cors::cors_check(&rh, &origin_ser, include).map_err(|e| {
                refusal("cors", &url, &format!("Access to fetch at '{url}' from origin '{origin_ser}' has been blocked by CORS policy: {e}."), None)
            })?;
        }
        if tainting == "opaque" {
            cors::corp_check(&ctx.client_origin, &url, &rh).map_err(|e| refusal("corp", &url, &format!("{e} (blocked by Cross-Origin-Resource-Policy)."), None))?;
        }
        let filtered = cors::filter_response_headers(&rh, tainting, include);
        return Ok(Exchange { resp, kind: tainting, headers: filtered, url, redirected });
    }
    Err("too many redirects".into())
}

/// Runs `req` on its own thread, posting `head(status, statusText, url, [k, v, …], redirected, type)`, a
/// `data(ArrayBuffer)` per chunk as it arrives, then `end` — or `error(message)` — as the last message. The
/// request context (client, mode, credentials, policies) is captured HERE, on the page thread; an `opaque`
/// answer posts status 0, no headers and no body; a request with `integrity` buffers its body and posts it
/// only after Subresource Integrity passes.
pub fn spawn_http(mut req: HttpReq, p: Poster) {
    let ctx = crate::security::RequestContext::capture(&mut req.headers);
    match ctx.precheck(&req.url, &req.method) {
        Ok(u) => req.url = u,
        Err(e) => {
            p.finish("error", vec![NetArg::Str(e)]);
            return;
        }
    }
    // AETHERSTORAGE (SR94): Service Workers §6.4 "handle fetch" — a controlled page's request is the
    // controlling worker's `fetch` event first (decided here, on the page thread); its answer, if any, is
    // the response, and the network serves the request otherwise.
    let sw = crate::workers::sw::fetch_handle(&req.url);
    std::thread::spawn(move || {
        if let Some((h, client)) = sw {
            if let Some(r) = h.fetch(&req.method, &req.url, &req.headers, &req.body, client) {
                let head = vec![
                    NetArg::Num(r.status as f64),
                    NetArg::Str(r.status_text),
                    NetArg::Str(if r.url.is_empty() { req.url.clone() } else { r.url }),
                    NetArg::List(header_list(&r.headers)),
                    NetArg::Bool(false),
                    NetArg::Str("basic".into()),
                ];
                if p.post("head", head) && (r.body.is_empty() || p.post("data", vec![NetArg::Bytes(r.body)])) {
                    p.finish("end", vec![]);
                }
                return;
            }
        }
        let ex = match policed_send(&req, &ctx) {
            Ok(r) => r,
            Err(e) => {
                crate::ledger::record_js(&format!("fetch-failed:{}", crate::js::clip(&req.url, 48)));
                p.finish("error", vec![NetArg::Str(e)]);
                return;
            }
        };
        let Exchange { mut resp, kind, headers, url, redirected } = ex;
        let opaque = kind == "opaque";
        let st = if opaque { 0 } else { resp.status().as_u16() };
        let head = vec![
            NetArg::Num(st as f64),
            NetArg::Str(if opaque { String::new() } else { resp.reason().to_string() }),
            NetArg::Str(if opaque { String::new() } else { url.clone() }),
            NetArg::List(header_list(&headers)),
            NetArg::Bool(redirected && !opaque),
            NetArg::Str(kind.to_string()),
        ];
        if !ctx.integrity.is_empty() && crate::security::sri::parse(&ctx.integrity).first().is_some() {
            let mut body = Vec::new();
            loop {
                match resp.chunk() {
                    Ok(Some(c)) => {
                        body.extend_from_slice(&c);
                        if body.len() > SYNC_BODY_CAP {
                            p.finish("error", vec![NetArg::Str("integrity-checked body larger than 64 MiB".into())]);
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        p.finish("error", vec![NetArg::Str(e.to_string())]);
                        return;
                    }
                }
            }
            if let Err(e) = crate::security::sri::check(&ctx.integrity, &body, !opaque) {
                let msg = format!("Failed to find a valid digest in the 'integrity' attribute for resource '{url}': {e}. The resource has been blocked.");
                p.finish("error", vec![NetArg::Str(crate::security::refusal("sri", &url, &msg, None))]);
                return;
            }
            if !p.post("head", head) {
                return;
            }
            if !opaque && !body.is_empty() && !p.post("data", vec![NetArg::Bytes(body)]) {
                return;
            }
            p.finish("end", vec![]);
            return;
        }
        if !p.post("head", head) {
            return;
        }
        if opaque {
            p.finish("end", vec![]);
            return;
        }
        loop {
            if p.cancelled() {
                return;
            }
            match resp.chunk() {
                Ok(Some(c)) => {
                    if c.is_empty() {
                        continue;
                    }
                    if !p.post("data", vec![NetArg::Bytes(c)]) {
                        return;
                    }
                }
                Ok(None) => {
                    p.finish("end", vec![]);
                    return;
                }
                Err(e) => {
                    p.finish("error", vec![NetArg::Str(e.to_string())]);
                    return;
                }
            }
        }
    });
}

/// A whole response (synchronous XHR).
pub struct SyncResponse {
    pub status: u16,
    pub status_text: String,
    pub url: String,
    pub headers: Vec<String>,
    pub body: Vec<u8>,
}

/// Runs `req` to completion on a scoped thread (the engine thread may sit inside a tokio runtime, which
/// forbids blocking directly) and joins it. The request context is captured on the calling (page) thread,
/// and the worker's refusals are replayed into the page's ledger after the join.
pub fn http_sync(mut req: HttpReq) -> Result<SyncResponse, String> {
    let ctx = crate::security::RequestContext::capture(&mut req.headers);
    req.url = ctx.precheck(&req.url, &req.method)?;
    let r = std::thread::spawn(move || -> Result<SyncResponse, String> {
        let ex = policed_send(&req, &ctx)?;
        let Exchange { mut resp, kind, headers, url, .. } = ex;
        let mut body = Vec::new();
        if kind != "opaque" {
            while let Some(c) = resp.chunk().map_err(|e| e.to_string())? {
                body.extend_from_slice(&c);
                if body.len() > SYNC_BODY_CAP {
                    return Err("synchronous response body larger than 64 MiB".into());
                }
            }
        }
        if !ctx.integrity.is_empty() {
            crate::security::sri::check(&ctx.integrity, &body, kind != "opaque").map_err(|e| crate::security::refusal("sri", &url, &e, None))?;
        }
        Ok(SyncResponse {
            status: if kind == "opaque" { 0 } else { resp.status().as_u16() },
            status_text: if kind == "opaque" { String::new() } else { resp.reason().to_string() },
            url: if kind == "opaque" { String::new() } else { url },
            headers: header_list(&headers),
            body,
        })
    })
    .join()
    .map_err(|_| "fetch thread panicked".to_string())?;
    crate::security::drain();
    r
}

/// AETHERFRAMES (SR83): a nested document's response — what "navigate" fetches for an `<iframe>`.
pub struct SubDocument {
    /// The response URL (after redirects).
    pub url: String,
    /// The MIME type essence (lowercase, no parameters); empty when the response named none.
    pub content_type: String,
    pub status: u16,
    pub body: String,
}

/// The MIME essence of a `Content-Type` value.
fn essence(v: &str) -> String {
    v.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// A local file's type from its extension (what a file server would send).
fn mime_for_path(path: &str) -> String {
    let p = path.split(['?', '#']).next().unwrap_or(path).to_ascii_lowercase();
    let ext = p.rsplit('.').next().unwrap_or("");
    match ext {
        "html" | "htm" => "text/html",
        "xhtml" | "xht" => "application/xhtml+xml",
        "xml" => "text/xml",
        "svg" => "image/svg+xml",
        "txt" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "js" => "text/javascript",
        "css" => "text/css",
        _ => "text/html",
    }
    .to_string()
}

/// Fetches the resource an `<iframe>` navigates to (HTML §7.4.5 "populating a session history
/// entry", the fetch half): `about:blank` is the empty HTML document, `file:` and `data:` come from
/// disk / the URL itself (`js::host::fetch_script_text`), `http(s):` is one synchronous GET on
/// [`http_sync`] (the shared cookie jar; redirects followed). The body is decoded as UTF-8 (lossily).
pub fn fetch_subdocument(url: &str) -> Result<SubDocument, String> {
    let bare = url.split(['#', '?']).next().unwrap_or(url);
    if bare == "about:blank" || bare == "about:srcdoc" {
        return Ok(SubDocument { url: url.to_string(), content_type: "text/html".into(), status: 200, body: String::new() });
    }
    if url.starts_with("about:") {
        return Err(format!("unsupported about: URL {url}"));
    }
    if let Some(rest) = url.strip_prefix("data:") {
        let meta = rest.split(',').next().unwrap_or("");
        let ct = essence(meta.trim_end_matches(";base64"));
        let body = crate::js::host::fetch_script_text(url)?;
        return Ok(SubDocument { url: url.to_string(), content_type: if ct.is_empty() { "text/plain".into() } else { ct }, status: 200, body });
    }
    if url.starts_with("file://") {
        let body = crate::js::host::fetch_script_text(url)?;
        return Ok(SubDocument { url: url.to_string(), content_type: mime_for_path(url), status: 200, body });
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        let r = http_sync(HttpReq {
            method: "GET".into(),
            url: url.to_string(),
            headers: vec![("Accept".into(), "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".into())],
            body: Vec::new(),
            redirect: "follow".into(),
            timeout_ms: 30_000,
        })?;
        let mut ct = String::new();
        for kv in r.headers.chunks(2) {
            if kv.len() == 2 && kv[0].eq_ignore_ascii_case("content-type") {
                ct = essence(&kv[1]);
            }
        }
        let body = String::from_utf8_lossy(&r.body).into_owned();
        let final_url = if r.url.is_empty() { url.to_string() } else { r.url };
        return Ok(SubDocument { url: final_url, content_type: ct, status: r.status, body });
    }
    Err(format!("unsupported URL scheme: {url}"))
}

/// The WHATWG shapes: AbortSignal/AbortController, Blob, Headers, the Streams Standard's default
/// ReadableStream, Request, Response and `fetch`.
pub const PRELUDE: &str = r#"
(function () {
    var hide = function (o, name, v) {
        Object.defineProperty(o, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var ST = new WeakMap();
    var st = function (o, what) {
        var s = (o !== null && typeof o === 'object') ? ST.get(o) : undefined;
        if (s === undefined) { throw new TypeError("Illegal invocation" + (what ? ' (' + what + ')' : '')); }
        return s;
    };
    var getter = function (proto, name, f) {
        Object.defineProperty(proto, name, { get: f, enumerable: true, configurable: true });
    };
    var method = function (proto, name, f) {
        Object.defineProperty(proto, name, { value: f, writable: true, enumerable: true, configurable: true });
    };
    var abortError = function () { return new DOMException('signal is aborted without reason', 'AbortError'); };
    var utf8 = function (s) { return new TextEncoder().encode(String(s)); };
    var concat = function (chunks) {
        var n = 0, i;
        for (i = 0; i < chunks.length; i++) { n += chunks[i].byteLength; }
        var out = new Uint8Array(n), at = 0;
        for (i = 0; i < chunks.length; i++) { out.set(chunks[i], at); at += chunks[i].byteLength; }
        return out;
    };
    var toBytes = function (v) {
        if (v instanceof ArrayBuffer) { return new Uint8Array(v.slice(0)); }
        if (ArrayBuffer.isView(v)) { return new Uint8Array(v.buffer.slice(v.byteOffset, v.byteOffset + v.byteLength)); }
        return null;
    };
    hide(globalThis, '__to_bytes', toBytes);

    // ---- AbortController / AbortSignal (DOM §3.1–3.2) ------------------------------------------------
    var creatingSignal = false;
    class AbortSignal extends EventTarget {
        constructor() {
            if (!creatingSignal) { throw new TypeError('Illegal constructor'); }
            super();
            ST.set(this, { aborted: false, reason: undefined, algos: [] });
        }
        get aborted() { return st(this).aborted; }
        get reason() { return st(this).reason; }
        throwIfAborted() { var s = st(this); if (s.aborted) { throw s.reason; } }
        static abort(reason) {
            var sig = newSignal();
            var s = st(sig);
            s.aborted = true;
            s.reason = reason === undefined ? abortError() : reason;
            return sig;
        }
        static timeout(ms) {
            var sig = newSignal();
            setTimeout(function () { signalAbort(sig, new DOMException('signal timed out', 'TimeoutError')); }, Number(ms) || 0);
            return sig;
        }
        static any(signals) {
            var out = newSignal();
            var list = Array.from(signals);
            for (var i = 0; i < list.length; i++) {
                if (st(list[i]).aborted) { signalAbort(out, st(list[i]).reason); return out; }
            }
            list.forEach(function (sg) { st(sg).algos.push(function () { signalAbort(out, st(sg).reason); }); });
            return out;
        }
    }
    var newSignal = function () {
        creatingSignal = true;
        try { return new AbortSignal(); } finally { creatingSignal = false; }
    };
    var signalAbort = function (sig, reason) {
        var s = st(sig);
        if (s.aborted) { return; }
        s.aborted = true;
        s.reason = reason === undefined ? abortError() : reason;
        var algos = s.algos.splice(0);
        for (var i = 0; i < algos.length; i++) { try { algos[i](); } catch (e) { reportError(e); } }
        __net_fire(sig, 'Event', 'abort', undefined);
    };
    __net_handlers(AbortSignal.prototype, ['abort']);
    Object.defineProperty(AbortSignal.prototype, Symbol.toStringTag, { value: 'AbortSignal', configurable: true });
    class AbortController {
        constructor() { ST.set(this, { signal: newSignal() }); }
        get signal() { return st(this).signal; }
        abort(reason) { signalAbort(st(this).signal, reason); }
    }
    Object.defineProperty(AbortController.prototype, Symbol.toStringTag, { value: 'AbortController', configurable: true });
    hide(globalThis, 'AbortSignal', AbortSignal);
    hide(globalThis, 'AbortController', AbortController);
    hide(globalThis, '__abortSignal', signalAbort);
    var onAbort = function (sig, f) { if (sig) { st(sig).algos.push(f); } };

    // ---- Blob (File API §3, the parts this platform needs: bytes, type, slice, readers) ---------------
    if (typeof Blob === 'undefined') {
        class Blob {
            constructor(parts, opts) {
                var chunks = [];
                if (parts !== undefined) {
                    if (parts === null || typeof parts !== 'object') { throw new TypeError("Failed to construct 'Blob': The provided value cannot be converted to a sequence."); }
                    Array.from(parts).forEach(function (p) {
                        var b = toBytes(p);
                        if (b) { chunks.push(b); }
                        else if (p instanceof Blob) { chunks.push(st(p).bytes); }
                        else { chunks.push(utf8(p)); }
                    });
                }
                var type = opts && opts.type !== undefined ? String(opts.type) : '';
                if (/[^ -~]/.test(type)) { type = ''; }
                ST.set(this, { bytes: concat(chunks), type: type.toLowerCase() });
            }
            get size() { return st(this).bytes.byteLength; }
            get type() { return st(this).type; }
            slice(start, end, type) {
                var b = st(this).bytes, n = b.byteLength;
                var rel = function (v, d) { if (v === undefined) { return d; } v = Math.trunc(Number(v)) || 0; return v < 0 ? Math.max(n + v, 0) : Math.min(v, n); };
                var s = rel(start, 0), e = rel(end, n);
                var out = new Blob([], { type: type === undefined ? '' : String(type) });
                st(out).bytes = b.slice(s, Math.max(s, e));
                return out;
            }
            arrayBuffer() { return Promise.resolve(st(this).bytes.slice().buffer); }
            bytes() { return Promise.resolve(st(this).bytes.slice()); }
            text() { return Promise.resolve(new TextDecoder().decode(st(this).bytes)); }
            stream() {
                var b = st(this).bytes.slice();
                return new ReadableStream({ start: function (c) { if (b.byteLength) { c.enqueue(b); } c.close(); } });
            }
        }
        Object.defineProperty(Blob.prototype, Symbol.toStringTag, { value: 'Blob', configurable: true });
        hide(globalThis, 'Blob', Blob);
        hide(globalThis, '__blob_bytes', function (b) { return st(b).bytes; });
    }

    // ---- FileReader (File API §6: the asynchronous reads, as tasks) ------------------------------------
    if (typeof FileReader === 'undefined' && typeof __blob_bytes === 'function') {
        class FileReader extends EventTarget {
            constructor() { super(); ST.set(this, { readyState: 0, result: null, error: null, gen: 0 }); }
            get readyState() { return st(this).readyState; }
            get result() { return st(this).result; }
            get error() { return st(this).error; }
            abort() {
                var s = st(this);
                if (s.readyState !== 1) { return; }
                s.gen++; s.readyState = 2; s.result = null;
                __net_fire(this, 'ProgressEvent', 'abort', undefined);
                __net_fire(this, 'ProgressEvent', 'loadend', undefined);
            }
        }
        var read = function (fr, blob, how, label) {
            var s = st(fr);
            if (!(blob instanceof Blob)) { throw new TypeError("Failed to execute '" + label + "' on 'FileReader': parameter 1 is not of type 'Blob'."); }
            if (s.readyState === 1) { throw new DOMException("Failed to execute '" + label + "' on 'FileReader': The object is already busy reading Blobs.", 'InvalidStateError'); }
            s.readyState = 1; s.result = null; s.error = null;
            var gen = ++s.gen, bytes = __blob_bytes(blob).slice(), type = blob.type;
            setTimeout(function () {
                if (s.gen !== gen) { return; }
                __net_fire(fr, 'ProgressEvent', 'loadstart', { lengthComputable: true, loaded: 0, total: bytes.byteLength });
                if (s.gen !== gen) { return; }
                if (how === 'text') { s.result = new TextDecoder().decode(bytes); }
                else if (how === 'buffer') { s.result = bytes.buffer; }
                else if (how === 'binary') { var b = ''; for (var i = 0; i < bytes.length; i++) { b += String.fromCharCode(bytes[i]); } s.result = b; }
                else {
                    var bin = '';
                    for (var j = 0; j < bytes.length; j++) { bin += String.fromCharCode(bytes[j]); }
                    s.result = 'data:' + (type || 'application/octet-stream') + ';base64,' + btoa(bin);
                }
                s.readyState = 2;
                __net_fire(fr, 'ProgressEvent', 'progress', { lengthComputable: true, loaded: bytes.byteLength, total: bytes.byteLength });
                __net_fire(fr, 'ProgressEvent', 'load', { lengthComputable: true, loaded: bytes.byteLength, total: bytes.byteLength });
                if (s.readyState === 2) { __net_fire(fr, 'ProgressEvent', 'loadend', { lengthComputable: true, loaded: bytes.byteLength, total: bytes.byteLength }); }
            }, 0);
        };
        FileReader.prototype.readAsText = function (b) { read(this, b, 'text', 'readAsText'); };
        FileReader.prototype.readAsArrayBuffer = function (b) { read(this, b, 'buffer', 'readAsArrayBuffer'); };
        FileReader.prototype.readAsBinaryString = function (b) { read(this, b, 'binary', 'readAsBinaryString'); };
        FileReader.prototype.readAsDataURL = function (b) { read(this, b, 'url', 'readAsDataURL'); };
        ['EMPTY', 'LOADING', 'DONE'].forEach(function (n, i) {
            Object.defineProperty(FileReader, n, { value: i, enumerable: true });
            Object.defineProperty(FileReader.prototype, n, { value: i, enumerable: true });
        });
        __net_handlers(FileReader.prototype, ['loadstart', 'progress', 'load', 'abort', 'error', 'loadend']);
        Object.defineProperty(FileReader.prototype, Symbol.toStringTag, { value: 'FileReader', configurable: true });
        hide(globalThis, 'FileReader', FileReader);
    }

    // ---- Headers (Fetch §5.2) -------------------------------------------------------------------------
    var TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
    var normValue = function (v) { return String(v).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, ''); };
    class Headers {
        constructor(init) {
            ST.set(this, { list: [], guard: 'none' });
            if (init === undefined || init === null) { return; }
            if (typeof init !== 'object' && typeof init !== 'function') { throw new TypeError("Failed to construct 'Headers': The provided value is not of type '(record<ByteString, ByteString> or sequence<sequence<ByteString>>)'."); }
            var self = this;
            if (init instanceof Headers) {
                st(init).list.forEach(function (p) { self.append(p[0], p[1]); });
            } else if (typeof init[Symbol.iterator] === 'function') {
                Array.from(init).forEach(function (pair) {
                    var p = Array.from(pair);
                    if (p.length !== 2) { throw new TypeError("Failed to construct 'Headers': Invalid value"); }
                    self.append(p[0], p[1]);
                });
            } else {
                Object.keys(init).forEach(function (k) { self.append(k, init[k]); });
            }
        }
        append(name, value) {
            var s = st(this);
            name = String(name); value = normValue(value);
            if (!TOKEN.test(name)) { throw new TypeError("Failed to execute 'append' on 'Headers': Invalid name"); }
            if (/[\0\r\n]/.test(value)) { throw new TypeError("Failed to execute 'append' on 'Headers': Invalid value"); }
            if (s.guard === 'immutable') { throw new TypeError("Failed to execute 'append' on 'Headers': Headers are immutable"); }
            s.list.push([name.toLowerCase(), value]);
        }
        delete(name) {
            var s = st(this);
            name = String(name);
            if (!TOKEN.test(name)) { throw new TypeError("Failed to execute 'delete' on 'Headers': Invalid name"); }
            if (s.guard === 'immutable') { throw new TypeError("Failed to execute 'delete' on 'Headers': Headers are immutable"); }
            var n = name.toLowerCase();
            s.list = s.list.filter(function (p) { return p[0] !== n; });
        }
        get(name) {
            name = String(name);
            if (!TOKEN.test(name)) { throw new TypeError("Failed to execute 'get' on 'Headers': Invalid name"); }
            var n = name.toLowerCase();
            var vals = st(this).list.filter(function (p) { return p[0] === n; }).map(function (p) { return p[1]; });
            return vals.length ? vals.join(', ') : null;
        }
        getSetCookie() {
            return st(this).list.filter(function (p) { return p[0] === 'set-cookie'; }).map(function (p) { return p[1]; });
        }
        has(name) {
            name = String(name);
            if (!TOKEN.test(name)) { throw new TypeError("Failed to execute 'has' on 'Headers': Invalid name"); }
            var n = name.toLowerCase();
            return st(this).list.some(function (p) { return p[0] === n; });
        }
        set(name, value) {
            var s = st(this);
            name = String(name); value = normValue(value);
            if (!TOKEN.test(name)) { throw new TypeError("Failed to execute 'set' on 'Headers': Invalid name"); }
            if (/[\0\r\n]/.test(value)) { throw new TypeError("Failed to execute 'set' on 'Headers': Invalid value"); }
            if (s.guard === 'immutable') { throw new TypeError("Failed to execute 'set' on 'Headers': Headers are immutable"); }
            var n = name.toLowerCase(), done = false;
            s.list = s.list.filter(function (p) {
                if (p[0] !== n) { return true; }
                if (!done) { done = true; p[1] = value; return true; }
                return false;
            });
            if (!done) { s.list.push([n, value]); }
        }
        forEach(cb, thisArg) {
            var e = sorted(this);
            for (var i = 0; i < e.length; i++) { cb.call(thisArg, e[i][1], e[i][0], this); }
        }
        entries() { return sorted(this)[Symbol.iterator](); }
        keys() { return sorted(this).map(function (p) { return p[0]; })[Symbol.iterator](); }
        values() { return sorted(this).map(function (p) { return p[1]; })[Symbol.iterator](); }
        [Symbol.iterator]() { return this.entries(); }
    }
    // "sort and combine" (Fetch §2.2.2): names lowercased and sorted, values combined, set-cookie apart.
    var sorted = function (h) {
        var list = st(h).list, names = [], out = [];
        list.forEach(function (p) { if (names.indexOf(p[0]) < 0) { names.push(p[0]); } });
        names.sort();
        names.forEach(function (n) {
            if (n === 'set-cookie') {
                list.forEach(function (p) { if (p[0] === n) { out.push([n, p[1]]); } });
            } else {
                out.push([n, h.get(n)]);
            }
        });
        return out;
    };
    Object.defineProperty(Headers.prototype, Symbol.toStringTag, { value: 'Headers', configurable: true });
    hide(globalThis, 'Headers', Headers);
    var headersFrom = function (list, guard) {
        var h = new Headers();
        var s = st(h);
        for (var i = 0; i + 1 < list.length; i += 2) { s.list.push([String(list[i]).toLowerCase(), list[i + 1]]); }
        s.guard = guard;
        return h;
    };
    hide(globalThis, '__headers_from', headersFrom);

    // ---- ReadableStream (Streams §4: the default stream, controller and reader) ------------------------
    var creatingInternal = false;
    var pullIfNeeded, closeStream, errorStream, fulfillRead, cancelStream;
    class ReadableStreamDefaultController {
        constructor() { if (!creatingInternal) { throw new TypeError('Illegal constructor'); } }
        get desiredSize() {
            var s = st(st(this).stream);
            if (s.state === 'errored') { return null; }
            if (s.state === 'closed') { return 0; }
            return s.hwm - s.queue.length;
        }
        enqueue(chunk) {
            var stream = st(this).stream, s = st(stream);
            if (s.closeRequested || s.state !== 'readable') {
                throw new TypeError("Failed to execute 'enqueue' on 'ReadableStreamDefaultController': Cannot enqueue a chunk into a closed stream");
            }
            if (s.reader && st(s.reader).reads.length) {
                fulfillRead(stream, chunk, false);
            } else {
                s.queue.push(chunk);
            }
            pullIfNeeded(stream);
        }
        close() {
            var stream = st(this).stream, s = st(stream);
            if (s.closeRequested || s.state !== 'readable') {
                throw new TypeError("Failed to execute 'close' on 'ReadableStreamDefaultController': Cannot close a closed stream");
            }
            s.closeRequested = true;
            if (!s.queue.length) { closeStream(stream); }
        }
        error(e) {
            var stream = st(this).stream;
            if (st(stream).state === 'readable') { errorStream(stream, e); }
        }
    }
    class ReadableStreamDefaultReader {
        constructor(stream) {
            if (!(stream instanceof ReadableStream)) { throw new TypeError("Failed to construct 'ReadableStreamDefaultReader': parameter 1 is not of type 'ReadableStream'."); }
            var s = st(stream);
            if (s.reader) { throw new TypeError("Failed to construct 'ReadableStreamDefaultReader': ReadableStreamDefaultReader constructor can only accept readable streams that are not yet locked to a reader"); }
            var rs = { stream: stream, reads: [], closed: null, closedResolve: null, closedReject: null };
            rs.closed = new Promise(function (res, rej) { rs.closedResolve = res; rs.closedReject = rej; });
            ST.set(this, rs);
            s.reader = this;
            if (s.state === 'closed') { rs.closedResolve(undefined); }
            else if (s.state === 'errored') { rs.closedReject(s.error); rs.closed.catch(function () {}); }
        }
        get closed() { return st(this).closed; }
        read() {
            var rs = st(this);
            if (!rs.stream) { return Promise.reject(new TypeError('This readable stream reader has been released and cannot be used to read from its previous owner stream')); }
            var stream = rs.stream, s = st(stream);
            s.disturbed = true;
            if (s.state === 'closed') { return Promise.resolve({ value: undefined, done: true }); }
            if (s.state === 'errored') { return Promise.reject(s.error); }
            if (s.queue.length) {
                var chunk = s.queue.shift();
                if (s.closeRequested && !s.queue.length) { closeStream(stream); } else { pullIfNeeded(stream); }
                return Promise.resolve({ value: chunk, done: false });
            }
            var p = new Promise(function (res, rej) { rs.reads.push({ res: res, rej: rej }); });
            pullIfNeeded(stream);
            return p;
        }
        releaseLock() {
            var rs = st(this);
            if (!rs.stream) { return; }
            var e = new TypeError('Releasing reader');
            rs.reads.splice(0).forEach(function (r) { r.rej(e); });
            if (st(rs.stream).state === 'readable') { rs.closedReject(e); }
            rs.closed = Promise.reject(e);
            rs.closed.catch(function () {});
            st(rs.stream).reader = null;
            rs.stream = null;
        }
        cancel(reason) {
            var rs = st(this);
            if (!rs.stream) { return Promise.reject(new TypeError('This readable stream reader has been released')); }
            return cancelStream(rs.stream, reason);
        }
    }
    class ReadableStream {
        constructor(source, strategy) {
            source = source === undefined || source === null ? {} : source;
            var hwm = strategy && strategy.highWaterMark !== undefined ? Number(strategy.highWaterMark) : 1;
            if (isNaN(hwm) || hwm < 0) { throw new RangeError('Invalid highWaterMark'); }
            if (source.type !== undefined && String(source.type) === 'bytes') { __ledger('ReadableStream-byte-source-as-default'); }
            creatingInternal = true;
            var controller;
            try { controller = new ReadableStreamDefaultController(); } finally { creatingInternal = false; }
            var s = { state: 'readable', queue: [], reader: null, error: undefined, disturbed: false, closeRequested: false,
                      started: false, pulling: false, pullAgain: false, hwm: hwm, source: source, controller: controller };
            ST.set(this, s);
            ST.set(controller, { stream: this });
            var self = this;
            var start = typeof source.start === 'function' ? source.start.call(source, controller) : undefined;
            Promise.resolve(start).then(function () {
                s.started = true;
                pullIfNeeded(self);
            }, function (e) { if (s.state === 'readable') { errorStream(self, e); } });
        }
        get locked() { return st(this).reader !== null; }
        cancel(reason) {
            if (st(this).reader) { return Promise.reject(new TypeError("Failed to execute 'cancel' on 'ReadableStream': Cannot cancel a locked stream")); }
            return cancelStream(this, reason);
        }
        getReader(opts) {
            if (opts && opts.mode !== undefined) {
                if (String(opts.mode) === 'byob') {
                    __ledger('ReadableStream-byob-reader');
                    throw new TypeError("Failed to execute 'getReader' on 'ReadableStream': Cannot get a BYOB reader for a stream that does not have an underlying byte source");
                }
                throw new TypeError("Failed to execute 'getReader' on 'ReadableStream': The provided value is not a valid enum value of type ReadableStreamReaderMode.");
            }
            return new ReadableStreamDefaultReader(this);
        }
        tee() {
            var reader = this.getReader();
            var c1, c2, canceled1 = false, canceled2 = false, reason1, reason2, reading = false, cancelResolve;
            var cancelPromise = new Promise(function (r) { cancelResolve = r; });
            var pull = function () {
                if (reading) { return Promise.resolve(); }
                reading = true;
                return reader.read().then(function (r) {
                    reading = false;
                    if (r.done) {
                        if (!canceled1) { try { c1.close(); } catch (e) {} }
                        if (!canceled2) { try { c2.close(); } catch (e) {} }
                        return;
                    }
                    if (!canceled1) { c1.enqueue(r.value); }
                    if (!canceled2) { c2.enqueue(r.value); }
                }, function (e) {
                    reading = false;
                    try { c1.error(e); } catch (x) {}
                    try { c2.error(e); } catch (x) {}
                });
            };
            var b1 = new ReadableStream({
                start: function (c) { c1 = c; }, pull: pull,
                cancel: function (r) { canceled1 = true; reason1 = r; if (canceled2) { cancelResolve(reader.cancel([reason1, reason2])); } return cancelPromise; },
            });
            var b2 = new ReadableStream({
                start: function (c) { c2 = c; }, pull: pull,
                cancel: function (r) { canceled2 = true; reason2 = r; if (canceled1) { cancelResolve(reader.cancel([reason1, reason2])); } return cancelPromise; },
            });
            return [b1, b2];
        }
        values(opts) {
            var reader = this.getReader(), prevent = !!(opts && opts.preventCancel);
            var it = {
                next: function () {
                    return reader.read().then(function (r) {
                        if (r.done) { reader.releaseLock(); }
                        return r;
                    });
                },
                return: function (v) {
                    var p = prevent ? Promise.resolve() : reader.cancel(v);
                    return p.then(function () { reader.releaseLock(); return { value: v, done: true }; });
                },
            };
            it[Symbol.asyncIterator] = function () { return it; };
            return it;
        }
        [Symbol.asyncIterator](opts) { return this.values(opts); }
        static from(iterable) {
            var it = iterable[Symbol.asyncIterator] ? iterable[Symbol.asyncIterator]() : iterable[Symbol.iterator]();
            return new ReadableStream({
                pull: function (c) {
                    return Promise.resolve(it.next()).then(function (r) {
                        if (r.done) { c.close(); } else { return Promise.resolve(r.value).then(function (v) { c.enqueue(v); }); }
                    });
                },
                cancel: function (r) { if (typeof it.return === 'function') { return Promise.resolve(it.return(r)).then(function () {}); } },
            });
        }
    }
    fulfillRead = function (stream, chunk, done) {
        var r = st(st(stream).reader).reads.shift();
        r.res({ value: chunk, done: done });
    };
    closeStream = function (stream) {
        var s = st(stream);
        if (s.state !== 'readable') { return; }
        s.state = 'closed';
        if (s.reader) {
            var rs = st(s.reader);
            rs.reads.splice(0).forEach(function (r) { r.res({ value: undefined, done: true }); });
            rs.closedResolve(undefined);
        }
    };
    errorStream = function (stream, e) {
        var s = st(stream);
        if (s.state !== 'readable') { return; }
        s.state = 'errored';
        s.error = e;
        s.queue = [];
        if (s.reader) {
            var rs = st(s.reader);
            rs.reads.splice(0).forEach(function (r) { r.rej(e); });
            rs.closedReject(e);
            rs.closed.catch(function () {});
        }
    };
    cancelStream = function (stream, reason) {
        var s = st(stream);
        s.disturbed = true;
        if (s.state === 'closed') { return Promise.resolve(undefined); }
        if (s.state === 'errored') { return Promise.reject(s.error); }
        s.queue = [];
        closeStream(stream);
        var src = s.source;
        var r;
        try { r = typeof src.cancel === 'function' ? src.cancel.call(src, reason) : undefined; }
        catch (e) { return Promise.reject(e); }
        return Promise.resolve(r).then(function () { return undefined; });
    };
    pullIfNeeded = function (stream) {
        var s = st(stream);
        if (!s.started || s.state !== 'readable' || s.closeRequested) { return; }
        var wants = (s.reader && st(s.reader).reads.length > 0) || (s.hwm - s.queue.length > 0);
        if (!wants || typeof s.source.pull !== 'function') { return; }
        if (s.pulling) { s.pullAgain = true; return; }
        s.pulling = true;
        var r;
        try { r = s.source.pull.call(s.source, s.controller); }
        catch (e) { errorStream(stream, e); return; }
        Promise.resolve(r).then(function () {
            s.pulling = false;
            if (s.pullAgain) { s.pullAgain = false; pullIfNeeded(stream); }
        }, function (e) { errorStream(stream, e); });
    };
    [ReadableStream, ReadableStreamDefaultReader, ReadableStreamDefaultController].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        hide(globalThis, C.name, C);
    });
    var disturbed = function (stream) { return st(stream).disturbed; };

    // ---- Body (Fetch §5.3) ----------------------------------------------------------------------------
    // Extracts a body: [stream or null, bytes or null, content type or null].
    var extract = function (body) {
        if (body instanceof ReadableStream) { return [body, null, null]; }
        var b = toBytes(body);
        if (b) { return [null, b, null]; }
        if (typeof Blob !== 'undefined' && body instanceof Blob) {
            return [null, typeof __blob_bytes === 'function' ? __blob_bytes(body).slice() : new Uint8Array(0), body.type || null];
        }
        if (typeof URLSearchParams !== 'undefined' && body instanceof URLSearchParams) {
            return [null, utf8(body.toString()), 'application/x-www-form-urlencoded;charset=UTF-8'];
        }
        if (typeof FormData !== 'undefined' && body instanceof FormData) {
            // multipart/form-data over js::forms' encoder (crate::forms::multipart).
            if (typeof __formdata_encode === 'function') {
                var enc = __formdata_encode(body);
                return [null, new Uint8Array(enc[0]), enc[1]];
            }
            __ledger('fetch-formdata-body-unencoded');
        }
        return [null, utf8(body), 'text/plain;charset=UTF-8'];
    };
    hide(globalThis, '__extract_body', extract);
    var streamOf = function (bytes) {
        return new ReadableStream({ start: function (c) { if (bytes.byteLength) { c.enqueue(bytes); } c.close(); } });
    };
    var consume = function (o, who) {
        var s = st(o);
        if (s.body === null) { return Promise.resolve(new Uint8Array(0)); }
        if (disturbed(s.body) || s.body.locked) {
            return Promise.reject(new TypeError("Failed to execute '" + who + "' on '" + (o instanceof Response ? 'Response' : 'Request') + "': body stream already read"));
        }
        var reader = s.body.getReader(), chunks = [];
        var step = function () {
            return reader.read().then(function (r) {
                if (r.done) { return concat(chunks); }
                var c = r.value;
                if (!(c instanceof Uint8Array)) {
                    var b = toBytes(c);
                    if (!b) { throw new TypeError('Received non-Uint8Array chunk'); }
                    c = b;
                }
                chunks.push(c);
                return step();
            });
        };
        return step();
    };
    var mixBody = function (C) {
        getter(C.prototype, 'body', function () { return st(this).body; });
        getter(C.prototype, 'bodyUsed', function () { var b = st(this).body; return b !== null && disturbed(b); });
        method(C.prototype, 'arrayBuffer', function () { return consume(this, 'arrayBuffer').then(function (b) { return b.buffer; }); });
        method(C.prototype, 'bytes', function () { return consume(this, 'bytes'); });
        method(C.prototype, 'text', function () { return consume(this, 'text').then(function (b) { return new TextDecoder().decode(b); }); });
        method(C.prototype, 'json', function () { return this.text().then(function (t) { return JSON.parse(t); }); });
        method(C.prototype, 'blob', function () {
            var ty = this.headers.get('content-type') || '';
            return consume(this, 'blob').then(function (b) { return new Blob([b], { type: ty }); });
        });
        method(C.prototype, 'formData', function () {
            __ledger('fetch-formData-unimplemented');
            return Promise.reject(new TypeError('formData() is not supported'));
        });
    };

    // ---- Request (Fetch §5.4) -------------------------------------------------------------------------
    var METHODS = ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'];
    var REFERRER_POLICIES = ['', 'no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin', 'strict-origin',
                             'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'];
    var baseURL = function () { try { return document.baseURI; } catch (e) { return location.href; } };
    class Request {
        constructor(input, init) {
            init = init === undefined || init === null ? {} : init;
            var src = input instanceof Request ? st(input) : null;
            var url;
            if (src) { url = src.url; }
            else {
                try { url = new URL(String(input), baseURL()).href; }
                catch (e) { throw new TypeError("Failed to construct 'Request': Invalid URL"); }
                if (/^[a-z][a-z0-9+.\-]*:\/\/[^\/?#]*@/i.test(url)) { throw new TypeError("Failed to construct 'Request': Request cannot be constructed from a URL that includes credentials"); }
            }
            var m = init.method !== undefined ? String(init.method) : (src ? src.method : 'GET');
            if (!TOKEN.test(m)) { throw new TypeError("Failed to construct 'Request': '" + m + "' is not a valid HTTP method."); }
            if (METHODS.indexOf(m.toUpperCase()) >= 0) { m = m.toUpperCase(); }
            if (['CONNECT', 'TRACE', 'TRACK'].indexOf(m.toUpperCase()) >= 0) { throw new TypeError("Failed to construct 'Request': '" + m + "' HTTP method is unsupported."); }
            var headers = new Headers(init.headers !== undefined ? init.headers : (src ? src.headers : undefined));
            var body = null, bytes = null;
            var hasBody = init.body !== undefined && init.body !== null;
            if (hasBody && (m === 'GET' || m === 'HEAD')) { throw new TypeError("Failed to construct 'Request': Request with GET/HEAD method cannot have body."); }
            if (hasBody) {
                var ex = extract(init.body);
                if (ex[0]) { body = ex[0]; } else { bytes = ex[1]; body = streamOf(bytes); }
                if (ex[2] && !headers.has('content-type')) { headers.append('content-type', ex[2]); }
            } else if (src && init.body === undefined) {
                body = src.body; bytes = src.bytes;
            }
            var signal = init.signal !== undefined ? init.signal : (src ? src.signal : null);
            if (signal !== null && signal !== undefined && !(signal instanceof AbortSignal)) {
                throw new TypeError("Failed to construct 'Request': member signal is not of type AbortSignal.");
            }
            var own = newSignal();
            if (signal) {
                if (st(signal).aborted) { signalAbort(own, st(signal).reason); }
                else { onAbort(signal, function () { signalAbort(own, st(signal).reason); }); }
            }
            var redirect = init.redirect !== undefined ? String(init.redirect) : (src ? src.redirect : 'follow');
            if (['follow', 'error', 'manual'].indexOf(redirect) < 0) { throw new TypeError("Failed to construct 'Request': The provided value '" + redirect + "' is not a valid enum value of type RequestRedirect."); }
            // AETHERORIGIN (SR86): mode, credentials, referrer, referrerPolicy and integrity are kept and
            // travel with the request (fetch() hands them to the policed exchange as pseudo-headers).
            var mode = init.mode !== undefined ? String(init.mode) : (src ? (src.mode === 'navigate' ? 'same-origin' : src.mode) : 'cors');
            if (mode === 'navigate') { throw new TypeError("Failed to construct 'Request': Cannot construct a Request with a RequestInit whose mode member is set as 'navigate'."); }
            if (['same-origin', 'no-cors', 'cors'].indexOf(mode) < 0) { throw new TypeError("Failed to construct 'Request': The provided value '" + mode + "' is not a valid enum value of type RequestMode."); }
            var cache = init.cache !== undefined ? String(init.cache) : (src ? src.cache : 'default');
            if (['default', 'no-store', 'reload', 'no-cache', 'force-cache', 'only-if-cached'].indexOf(cache) < 0) { throw new TypeError("Failed to construct 'Request': The provided value '" + cache + "' is not a valid enum value of type RequestCache."); }
            // Fetch, the Request constructor: only-if-cached is for same-origin requests only.
            if (cache === 'only-if-cached' && mode !== 'same-origin') { throw new TypeError("Failed to construct 'Request': 'only-if-cached' can be set only with 'same-origin' mode"); }
            var credentials = init.credentials !== undefined ? String(init.credentials) : (src ? src.credentials : 'same-origin');
            if (['omit', 'same-origin', 'include'].indexOf(credentials) < 0) { throw new TypeError("Failed to construct 'Request': The provided value '" + credentials + "' is not a valid enum value of type RequestCredentials."); }
            var referrerPolicy = init.referrerPolicy !== undefined ? String(init.referrerPolicy) : (src ? src.referrerPolicy : '');
            if (REFERRER_POLICIES.indexOf(referrerPolicy) < 0) { throw new TypeError("Failed to construct 'Request': The provided value '" + referrerPolicy + "' is not a valid enum value of type ReferrerPolicy."); }
            var referrer = src ? src.referrer : 'about:client';
            if (init.referrer !== undefined) {
                var rv = String(init.referrer);
                if (rv === '') { referrer = ''; }
                else {
                    var pr;
                    try { pr = new URL(rv, baseURL()); } catch (e) { throw new TypeError("Failed to construct 'Request': Referrer '" + rv + "' is not a valid URL."); }
                    var here = null;
                    try { here = location.origin; } catch (e) {}
                    referrer = (pr.href === 'about:client' || (here !== null && pr.origin !== here)) ? 'about:client' : pr.href;
                }
            }
            var integrity = init.integrity !== undefined ? String(init.integrity) : (src ? src.integrity : '');
            if (mode === 'no-cors' && ['GET', 'HEAD', 'POST'].indexOf(m) < 0) { throw new TypeError("Failed to construct 'Request': '" + m + "' is unsupported in no-cors mode."); }
            ST.set(this, {
                url: url, method: m, headers: headers, body: body, bytes: bytes, signal: own, redirect: redirect,
                credentials: credentials, mode: mode,
                cache: cache,
                keepalive: !!init.keepalive,
                referrer: referrer, referrerPolicy: referrerPolicy, integrity: integrity,
            });
        }
        get url() { return st(this).url; }
        get method() { return st(this).method; }
        get headers() { return st(this).headers; }
        get signal() { return st(this).signal; }
        get redirect() { return st(this).redirect; }
        get credentials() { return st(this).credentials; }
        get mode() { return st(this).mode; }
        get cache() { return st(this).cache; }
        get keepalive() { return st(this).keepalive; }
        get destination() { return ''; }
        get referrer() { return st(this).referrer; }
        get referrerPolicy() { return st(this).referrerPolicy; }
        get integrity() { return st(this).integrity; }
        clone() {
            var s = st(this);
            if (s.body && (disturbed(s.body) || s.body.locked)) { throw new TypeError("Failed to execute 'clone' on 'Request': Request body is already used"); }
            var r = new Request(this);
            if (s.body) {
                var t = s.body.tee();
                s.body = t[0];
                st(r).body = t[1];
            }
            return r;
        }
    }
    mixBody(Request);
    Object.defineProperty(Request.prototype, Symbol.toStringTag, { value: 'Request', configurable: true });
    hide(globalThis, 'Request', Request);

    // ---- Response (Fetch §5.5) ------------------------------------------------------------------------
    var NULL_BODY = [101, 103, 204, 205, 304];
    var creatingResponse = null;
    class Response {
        constructor(body, init) {
            if (creatingResponse) { ST.set(this, creatingResponse); creatingResponse = null; return; }
            init = init === undefined || init === null ? {} : init;
            var status = init.status !== undefined ? Number(init.status) : 200;
            if (!(status >= 200 && status <= 599) || Math.floor(status) !== status) {
                throw new RangeError("Failed to construct 'Response': The status provided (" + init.status + ") is outside the range [200, 599].");
            }
            var statusText = init.statusText !== undefined ? String(init.statusText) : '';
            if (/[^\t -~\u0080-ÿ]/.test(statusText)) { throw new TypeError("Failed to construct 'Response': Invalid statusText"); }
            var headers = new Headers(init.headers);
            var stream = null;
            if (body !== undefined && body !== null) {
                if (NULL_BODY.indexOf(status) >= 0) { throw new TypeError("Failed to construct 'Response': Response with null body status cannot have body"); }
                var ex = extract(body);
                stream = ex[0] || streamOf(ex[1]);
                if (ex[2] && !headers.has('content-type')) { headers.append('content-type', ex[2]); }
            }
            ST.set(this, { type: 'default', url: '', redirected: false, status: status, statusText: statusText, headers: headers, body: stream });
        }
        get type() { return st(this).type; }
        get url() { return st(this).url; }
        get redirected() { return st(this).redirected; }
        get status() { return st(this).status; }
        get ok() { var s = st(this).status; return s >= 200 && s <= 299; }
        get statusText() { return st(this).statusText; }
        get headers() { return st(this).headers; }
        clone() {
            var s = st(this);
            if (s.body && (disturbed(s.body) || s.body.locked)) { throw new TypeError("Failed to execute 'clone' on 'Response': Response body is already used"); }
            var copy = {};
            for (var k in s) { copy[k] = s[k]; }
            copy.headers = new Headers(s.headers);
            st(copy.headers).guard = st(s.headers).guard;
            if (s.body) { var t = s.body.tee(); s.body = t[0]; copy.body = t[1]; }
            creatingResponse = copy;
            return new Response();
        }
        static error() {
            var h = new Headers(); st(h).guard = 'immutable';
            creatingResponse = { type: 'error', url: '', redirected: false, status: 0, statusText: '', headers: h, body: null };
            return new Response();
        }
        static redirect(url, status) {
            status = status === undefined ? 302 : Number(status);
            if ([301, 302, 303, 307, 308].indexOf(status) < 0) { throw new RangeError("Failed to execute 'redirect' on 'Response': Invalid status code"); }
            var u;
            try { u = new URL(String(url), baseURL()).href; } catch (e) { throw new TypeError("Failed to execute 'redirect' on 'Response': Invalid URL"); }
            var h = new Headers({ location: u }); st(h).guard = 'immutable';
            creatingResponse = { type: 'default', url: '', redirected: false, status: status, statusText: '', headers: h, body: null };
            return new Response();
        }
        static json(data, init) {
            var text = JSON.stringify(data);
            if (text === undefined) { throw new TypeError("Failed to execute 'json' on 'Response': The data is not JSON serializable"); }
            var r = new Response(text, init);
            st(r).headers.set('content-type', 'application/json');
            return r;
        }
    }
    mixBody(Response);
    Object.defineProperty(Response.prototype, Symbol.toStringTag, { value: 'Response', configurable: true });
    hide(globalThis, 'Response', Response);

    // ---- fetch (Fetch §5.6) ---------------------------------------------------------------------------
    var dataURL = function (url) {
        var m = /^data:([^,]*?)(;base64)?,([\s\S]*)$/i.exec(url);
        if (!m) { return null; }
        var type = m[1] || 'text/plain;charset=US-ASCII', bytes;
        if (m[2]) {
            var bin = atob(decodeURIComponent(m[3]).replace(/[\t\n\f\r ]/g, ''));
            bytes = new Uint8Array(bin.length);
            for (var i = 0; i < bin.length; i++) { bytes[i] = bin.charCodeAt(i); }
        } else {
            bytes = utf8(decodeURIComponent(m[3]));
        }
        return [type, bytes];
    };
    hide(globalThis, '__data_url', dataURL);
    var fetch = function fetch(input, init) {
        return new Promise(function (resolve, reject) {
            var req;
            try { req = new Request(input, init); } catch (e) { reject(e); return; }
            var rs = st(req), signal = rs.signal;
            if (st(signal).aborted) { reject(st(signal).reason); return; }
            var url = rs.url;
            if (/^data:/i.test(url)) {
                var d;
                try { d = dataURL(url); } catch (e) { d = null; }
                if (!d) { reject(new TypeError('Failed to fetch')); return; }
                var hh = headersFrom(['content-type', d[0]], 'immutable');
                creatingResponse = { type: 'basic', url: url, redirected: false, status: 200, statusText: 'OK', headers: hh, body: streamOf(d[1]) };
                resolve(new Response());
                return;
            }
            if (!/^https?:/i.test(url)) { reject(new TypeError('Failed to fetch')); return; }
            if (rs.body && !rs.bytes) {
                __ledger('fetch-streaming-request-body');
                reject(new TypeError('Failed to fetch: streaming request bodies are not supported'));
                return;
            }
            var flat = [];
            st(rs.headers).list.forEach(function (p) { flat.push(p[0], p[1]); });
            // The cache mode and the top-level site ride as pseudo-fields the worker lifts off (AETHERCACHE).
            flat.push(':cache-mode', rs.cache);
            try {
                var top = (globalThis.top && globalThis.top.location) ? globalThis.top.location : globalThis.location;
                if (top && top.href) { flat.push(':top-level-site', String(top.href)); }
            } catch (e) {}
            // The policy inputs (names starting ':' are never a page's header; the exchange strips them).
            flat.push(':aether-mode', rs.mode, ':aether-credentials', rs.credentials,
                      ':aether-referrer', rs.referrer === '' ? 'no-referrer' : rs.referrer,
                      ':aether-referrer-policy', rs.referrerPolicy, ':aether-integrity', rs.integrity);
            var id = __net_http(rs.method, url.replace(/#.*$/, ''), flat, rs.bytes, rs.redirect);
            if (!id) { reject(new TypeError('Failed to fetch')); return; }
            var resp = null, controller = null, done = false;
            __net_register(id, {
                __net: function (kind, a) {
                    if (kind === 'head') {
                        var status = a[0];
                        var h = headersFrom(a[3], 'immutable');
                        var type = a[5] === 'cors' || a[5] === 'opaque' ? a[5] : 'basic';
                        if (rs.redirect === 'manual' && status >= 300 && status < 400 && h.has('location')) {
                            type = 'opaqueredirect';
                        }
                        if (type === 'opaque') {
                            creatingResponse = { type: 'opaque', url: '', redirected: false, status: 0, statusText: '', headers: headersFrom([], 'immutable'), body: null };
                            resp = new Response();
                            resolve(resp);
                            return;
                        }
                        var body = NULL_BODY.indexOf(status) >= 0 || rs.method === 'HEAD' || type === 'opaqueredirect' ? null :
                            new ReadableStream({
                                start: function (c) { controller = c; },
                                cancel: function () { done = true; __net_cancel(id); __net_forget(id); },
                            }, { highWaterMark: 0 });
                        creatingResponse = type === 'opaqueredirect' ?
                            { type: type, url: a[2], redirected: false, status: 0, statusText: '', headers: headersFrom([], 'immutable'), body: null } :
                            { type: type, url: a[2], redirected: !!a[4], status: status, statusText: a[1], headers: h, body: body };
                        resp = new Response();
                        resolve(resp);
                    } else if (kind === 'data') {
                        if (controller && !done) { controller.enqueue(new Uint8Array(a[0])); }
                    } else if (kind === 'end') {
                        done = true;
                        if (controller) { try { controller.close(); } catch (e) {} }
                    } else if (kind === 'error') {
                        done = true;
                        // A security refusal carries its SecurityPolicyViolationEventInit (when CSP refused it).
                        if (typeof a[0] === 'string' && a[0].indexOf('aether-security\u0001') === 0) {
                            var sec = a[0].split('\u0001');
                            if (sec[2] && typeof __csp_violation === 'function') { try { __csp_violation(JSON.parse(sec[2]), null); } catch (x) {} }
                        }
                        if (!resp) { reject(new TypeError('Failed to fetch')); }
                        else if (controller) { try { controller.error(new TypeError('network error')); } catch (e) {} }
                    }
                },
            });
            onAbort(signal, function () {
                if (done) { return; }
                done = true;
                __net_cancel(id);
                __net_forget(id);
                var reason = st(signal).reason;
                if (!resp) { reject(reason); }
                else if (controller) { try { controller.error(reason); } catch (e) {} }
            });
        });
    };
    Object.defineProperty(globalThis, 'fetch', { value: fetch, writable: true, configurable: true, enumerable: true });
})();
"#;
