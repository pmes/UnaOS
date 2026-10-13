//! CHARTER: Aether (docs/CODEX.md §2, Web) — seam: handler (host-native Ring 3, handlers/aether), the web
//! platform's security model; the pure policy cores (origin, cors, csp, mixed, referrer, sri) carry no
//! engine state and are the half a later `no_std` web core can lift.
//!
//! AETHERORIGIN (LEDGER SR86): Aether enforces the web's security model. Every refusal is a named line in
//! the engine's ledger (`security-<rule>:<detail>`, via [`refuse`]) and a console message; refusals made
//! off the page thread (a fetch worker's redirect hop, a CORS check on the answer) are queued and replayed
//! on the page thread at its next request or script ([`drain`]), and every one is kept in a process log
//! ([`refusals`]) for the headless report and tests.
//!
//! - [`origin`] — origins (opaque for `data:`/`file:`/sandboxed), sites, `Sec-Fetch-*`, `document.domain`
//!   refused, COOP's browsing-context-group switch;
//! - [`cors`] — the CORS protocol, preflight + cache, response filtering, CORP;
//! - [`csp`] — Content Security Policy Level 3 and its reports / events;
//! - [`mixed`] — Mixed Content Level 2 and Upgrade Insecure Requests;
//! - [`referrer`] — the eight referrer policies;
//! - [`sri`] — Subresource Integrity;
//! - here: the per-document state (policies from the response headers via [`set_document_headers`] and from
//!   `<meta http-equiv>` / `<meta name=referrer>` read at [`sync`]), the [`RequestContext`] a page request
//!   carries to its worker, the per-hop checks, the cookie context (SameSite, partitioning), the script
//!   gates the loader calls, the navigation headers, and [`may_embed`] (X-Frame-Options + frame-ancestors)
//!   for AETHERFRAMES.

pub mod cors;
pub mod csp;
pub mod mixed;
pub mod origin;
pub mod referrer;
pub mod sri;

use origin::{FetchSite, Origin};
use referrer::ReferrerPolicy;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

/// The prefix of an error string that carries a security refusal back to a page prelude:
/// `aether-security\u{1}<message>\u{1}<SecurityPolicyViolationEventInit JSON or empty>`.
pub const ERR_PREFIX: &str = "aether-security\u{1}";

// ------------------------------------------------------------------------------------------- refusals

thread_local! {
    /// This thread runs a page (it captured a request, synced or armed): refusals record here directly.
    static PAGE_THREAD: Cell<bool> = const { Cell::new(false) };
}

fn deferred() -> &'static Mutex<Vec<(String, String)>> {
    static Q: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Vec::new()))
}

fn log() -> &'static Mutex<Vec<String>> {
    static L: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Vec::new()))
}

/// Every refusal's ledger name this process has made, oldest first (capped at 4096).
pub fn refusals() -> Vec<String> {
    log().lock().map(|l| l.clone()).unwrap_or_default()
}

/// Records one refusal: the ledger line `security-<rule>[:<detail>]` and a console error. On a worker
/// thread it is queued for the page thread.
pub fn refuse(rule: &str, detail: &str, message: &str) {
    let name = if detail.is_empty() { format!("security-{rule}") } else { format!("security-{rule}:{}", crate::js::clip(detail, 64)) };
    if let Ok(mut l) = log().lock() {
        if l.len() < 4096 {
            l.push(name.clone());
        }
    }
    let msg = format!("[security] {message}");
    if PAGE_THREAD.with(|p| p.get()) {
        crate::ledger::record_js(&name);
        crate::js::host::console_out(2, &msg);
    } else if let Ok(mut q) = deferred().lock() {
        if q.len() < 1024 {
            q.push((name, msg));
        }
    }
}

/// Replays the refusals worker threads queued (on the page thread).
pub fn drain() {
    PAGE_THREAD.with(|p| p.set(true));
    let q: Vec<(String, String)> = deferred().lock().map(|mut q| q.drain(..).collect()).unwrap_or_default();
    for (name, msg) in q {
        crate::ledger::record_js(&name);
        crate::js::host::console_out(2, &msg);
    }
}

/// Runs `f` with this thread's refusals deferred (replayed by the next [`drain`]): for callers that may run
/// while the page state is borrowed (the CSS cascade), where writing the console directly could re-enter it.
pub(crate) fn deferring<R>(f: impl FnOnce() -> R) -> R {
    let was = PAGE_THREAD.with(|p| p.replace(false));
    let r = f();
    PAGE_THREAD.with(|p| p.set(was));
    r
}

/// A refusal as the error string a request fails with (see [`ERR_PREFIX`]).
pub fn refusal(rule: &str, detail: &str, message: &str, event: Option<String>) -> String {
    refuse(rule, detail, message);
    format!("{ERR_PREFIX}{message}\u{1}{}", event.unwrap_or_default())
}

/// Logs and reports a CSP verdict's violations (report POSTs; the console; the event queued for the
/// page's next script, or handed back for the caller to attach to its own error). Returns the first
/// enforced violation's event init.
pub fn report_violations(v: &csp::Verdict, target: Option<usize>) -> Option<String> {
    let (url, endpoints) = DOC.with(|d| {
        let d = d.borrow();
        (d.url.clone(), d.endpoints.clone())
    });
    let mut first = None;
    for x in &v.violations {
        let msg = csp::message(x);
        let rule = if x.report_only { "csp-report-only" } else { "csp" };
        refuse(rule, &format!("{}:{}", x.effective_directive, x.blocked), &msg);
        csp::send_reports(x, &url, &endpoints);
        let init = csp::event_init(x, &url);
        DOC.with(|d| {
            let mut d = d.borrow_mut();
            if !d.url.is_empty() && d.events.len() < 256 {
                d.events.push((init.clone(), target));
            }
        });
        if first.is_none() && !x.report_only {
            first = Some(init);
        }
    }
    first
}

// ------------------------------------------------------------------------------------------- document state

/// What the response headers of a document said (handed over by the navigation code).
#[derive(Debug, Clone, Default)]
pub struct DocHeaders {
    pub csp: Vec<csp::Policy>,
    pub referrer_policy: ReferrerPolicy,
    pub endpoints: Vec<(String, String)>,
    pub sandboxed: bool,
}

fn pending_headers() -> &'static Mutex<HashMap<String, DocHeaders>> {
    static P: OnceLock<Mutex<HashMap<String, DocHeaders>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `Reporting-Endpoints: name="url", other="url"` (Reporting API §3.2, a structured-field dictionary).
pub fn parse_reporting_endpoints(v: &str) -> Vec<(String, String)> {
    v.split(',')
        .filter_map(|m| {
            let (k, val) = m.split_once('=')?;
            let val = val.trim().trim_matches('"').to_string();
            Some((k.trim().to_string(), val))
        })
        .collect()
}

/// The navigation code's hook: the final response headers of the document at `url` (call it before the
/// document is loaded; `(lowercase name, value)` pairs). Reads `Content-Security-Policy`,
/// `Content-Security-Policy-Report-Only`, `Referrer-Policy`, `Reporting-Endpoints` and a CSP `sandbox`.
pub fn set_document_headers(url: &str, headers: &[(String, String)]) {
    let mut h = DocHeaders::default();
    for (k, v) in headers {
        match k.to_ascii_lowercase().as_str() {
            "content-security-policy" => h.csp.extend(csp::Policy::parse_header(v, false)),
            "content-security-policy-report-only" => h.csp.extend(csp::Policy::parse_header(v, true)),
            "referrer-policy" => {
                let p = ReferrerPolicy::parse_header(v);
                if p != ReferrerPolicy::Empty {
                    h.referrer_policy = p;
                }
            }
            "reporting-endpoints" => h.endpoints.extend(parse_reporting_endpoints(v)),
            _ => {}
        }
    }
    h.sandboxed = h.csp.iter().any(|p| !p.report_only && p.get("sandbox").is_some_and(|l| !l.iter().any(|t| t.eq_ignore_ascii_case("allow-same-origin"))));
    if let Ok(mut m) = pending_headers().lock() {
        if m.len() > 64 {
            m.clear();
        }
        m.insert(url.split('#').next().unwrap_or(url).to_string(), h);
    }
}

#[derive(Default)]
struct DocState {
    url: String,
    headers: DocHeaders,
    meta_csp: Vec<String>,
    meta_referrer: Option<ReferrerPolicy>,
    policies: Arc<csp::PolicyList>,
    endpoints: Vec<(String, String)>,
    armed: bool,
    eval_guard_due: bool,
    reported: HashSet<String>,
    events: Vec<(String, Option<usize>)>,
}

thread_local! {
    static DOC: RefCell<DocState> = RefCell::new(DocState::default());
}

/// The `<meta>` children of the main document's `<head>`: (http-equiv CSP contents, name=referrer content).
fn scan_head() -> (Vec<String>, Option<String>) {
    if !crate::js::page(|p| p.doc.is_some()) {
        return (Vec::new(), None);
    }
    let doc = crate::js::dom::main_doc();
    crate::js::dom::with_doc(|d| {
        let mut csp = Vec::new();
        let mut referrer = None;
        let root = html_core::NodeId(doc);
        let Some(html) = d.children(root).find(|c| d.element(*c).is_some_and(|e| e.is_html("html"))) else { return (csp, referrer) };
        let Some(head) = d.children(html).find(|c| d.element(*c).is_some_and(|e| e.is_html("head"))) else { return (csp, referrer) };
        for c in d.children(head) {
            let Some(e) = d.element(c) else { continue };
            if !e.is_html("meta") {
                continue;
            }
            if e.attr("http-equiv").is_some_and(|v| v.trim().eq_ignore_ascii_case("content-security-policy")) {
                if let Some(content) = e.attr("content") {
                    csp.push(content.to_string());
                }
            } else if e.attr("http-equiv").is_some_and(|v| v.trim().eq_ignore_ascii_case("content-security-policy-report-only")) {
                crate::ledger::record_js("security-csp-meta-report-only-ignored");
            } else if e.attr("name").is_some_and(|v| v.trim().eq_ignore_ascii_case("referrer")) {
                if let Some(content) = e.attr("content") {
                    referrer = Some(content.to_string());
                }
            }
        }
        (csp, referrer)
    })
}

/// Brings this thread's document state up to date: a new document URL resets it (taking the headers the
/// navigation code left for that URL); `<meta>` policies only ever ADD (CSP3 §4.2: removing the element does
/// not remove its policy).
pub fn sync() {
    PAGE_THREAD.with(|p| p.set(true));
    let url = crate::js::page_url();
    let (csp_metas, referrer_meta) = scan_head();
    DOC.with(|d| {
        let mut d = d.borrow_mut();
        let mut rebuild = false;
        if d.url != url {
            let key = url.split('#').next().unwrap_or(&url).to_string();
            let headers = pending_headers().lock().ok().and_then(|m| m.get(&key).cloned()).unwrap_or_default();
            *d = DocState { url: url.clone(), endpoints: headers.endpoints.clone(), headers, eval_guard_due: true, ..Default::default() };
            rebuild = true;
        }
        for m in csp_metas {
            if !d.meta_csp.contains(&m) {
                d.meta_csp.push(m);
                rebuild = true;
            }
        }
        if let Some(r) = referrer_meta.as_deref().and_then(ReferrerPolicy::parse_meta) {
            d.meta_referrer = Some(r);
        }
        if rebuild {
            let mut list: Vec<csp::Policy> = d.headers.csp.clone();
            list.extend(d.meta_csp.iter().map(|m| csp::Policy::parse(m, false, true)).filter(|p| !p.directives.is_empty()));
            let sandboxed = d.headers.sandboxed;
            let mut pl = csp::PolicyList::new(list, &url);
            if sandboxed {
                pl.self_origin = Origin::opaque();
            }
            d.policies = Arc::new(pl);
            d.eval_guard_due = true;
        }
    });
}

/// `sync`, then what [`csp::arm`] needs: (document URL, first arm of this document, policies, queued events).
pub(crate) fn sync_for_script() -> (String, bool, Arc<csp::PolicyList>, Vec<(String, Option<usize>)>) {
    sync();
    drain();
    DOC.with(|d| {
        let mut d = d.borrow_mut();
        let first = !d.armed;
        d.armed = true;
        (d.url.clone(), first, d.policies.clone(), std::mem::take(&mut d.events))
    })
}

/// Whether the eval guard should be (re)considered now (once per policy change).
pub(crate) fn take_eval_guard_due() -> bool {
    DOC.with(|d| std::mem::replace(&mut d.borrow_mut().eval_guard_due, false))
}

/// The policies read at the last [`sync`] (`None` before any).
pub fn current_policies() -> Option<Arc<csp::PolicyList>> {
    DOC.with(|d| {
        let d = d.borrow();
        (!d.url.is_empty()).then(|| d.policies.clone())
    })
}

/// The document's referrer policy: `<meta name=referrer>` over the `Referrer-Policy` header.
pub fn document_referrer_policy() -> ReferrerPolicy {
    DOC.with(|d| {
        let d = d.borrow();
        d.meta_referrer.unwrap_or(d.headers.referrer_policy)
    })
}

/// Whether `key` is reported for the first time in this document (de-duplicates repeated refusals).
pub(crate) fn first_report_of(key: &str) -> bool {
    DOC.with(|d| {
        let mut d = d.borrow_mut();
        if d.reported.len() > 4096 {
            return false;
        }
        d.reported.insert(key.to_string())
    })
}

// ------------------------------------------------------------------------------------------- requests

/// What a page request carries from the page thread to its worker: the client, the mode, credentials,
/// destination, referrer and its policy, integrity, and the document's policies.
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// A page made this request (false: an engine-internal request; no policy applies).
    pub has_page: bool,
    pub client_url: String,
    pub client_origin: Origin,
    /// The top-level site (cookie partitioning, SameSite).
    pub top_site: String,
    /// `cors` | `no-cors` | `same-origin` | `navigate`.
    pub mode: String,
    /// `omit` | `same-origin` | `include`.
    pub credentials: String,
    pub destination: String,
    /// The referrer URL before its policy (`None`: `no-referrer`).
    pub referrer: Option<String>,
    pub referrer_policy: ReferrerPolicy,
    pub integrity: String,
    pub policies: Arc<csp::PolicyList>,
}

impl RequestContext {
    /// No page: the engine's own requests keep the legacy behaviour.
    pub fn none() -> RequestContext {
        RequestContext {
            has_page: false,
            client_url: String::new(),
            client_origin: Origin::opaque(),
            top_site: String::new(),
            mode: "no-cors".into(),
            credentials: "include".into(),
            destination: String::new(),
            referrer: None,
            referrer_policy: ReferrerPolicy::Empty,
            integrity: String::new(),
            policies: Arc::new(csp::PolicyList::default()),
        }
    }

    fn for_page(mode: &str, credentials: &str, destination: &str) -> RequestContext {
        sync();
        drain();
        let url = crate::js::page_url();
        if url.is_empty() || !(url.starts_with("http") || url.starts_with("file:") || url.starts_with("data:") || url.starts_with("about:")) {
            return RequestContext::none();
        }
        let (policies, sandboxed) = DOC.with(|d| {
            let d = d.borrow();
            (d.policies.clone(), d.headers.sandboxed)
        });
        RequestContext {
            has_page: true,
            client_origin: Origin::of_document(&url, sandboxed),
            top_site: crate::storage::top_level_site(&url),
            mode: mode.into(),
            credentials: credentials.into(),
            destination: destination.into(),
            referrer: Some(url.clone()),
            referrer_policy: document_referrer_policy(),
            integrity: String::new(),
            policies,
            client_url: url,
        }
    }

    /// On the page thread, for a `fetch()` / XHR request: reads (and removes) the prelude's pseudo-headers
    /// (`:aether-mode`, `:aether-credentials`, `:aether-referrer`, `:aether-referrer-policy`,
    /// `:aether-integrity`, `:aether-destination`; a name starting with `:` can never be a page's header), and
    /// drops every forbidden request-header the page tried to set (and, for `no-cors`, every header that is
    /// not CORS-safelisted). Without pseudo-headers (XHR) the request is `cors` with `same-origin` credentials.
    pub fn capture(headers: &mut Vec<(String, String)>) -> RequestContext {
        let pseudo = |headers: &Vec<(String, String)>, n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.clone());
        let mode = pseudo(headers, ":aether-mode").filter(|m| matches!(m.as_str(), "cors" | "no-cors" | "same-origin")).unwrap_or_else(|| "cors".into());
        let cred = pseudo(headers, ":aether-credentials").filter(|m| matches!(m.as_str(), "omit" | "same-origin" | "include")).unwrap_or_else(|| "same-origin".into());
        let dest = pseudo(headers, ":aether-destination").unwrap_or_default();
        let mut ctx = RequestContext::for_page(&mode, &cred, &dest);
        let referrer = pseudo(headers, ":aether-referrer");
        let policy = pseudo(headers, ":aether-referrer-policy").and_then(|p| ReferrerPolicy::parse_token(&p)).unwrap_or_default();
        ctx.integrity = pseudo(headers, ":aether-integrity").unwrap_or_default();
        headers.retain(|(k, _)| !k.starts_with(':'));
        if !ctx.has_page {
            return ctx;
        }
        if policy != ReferrerPolicy::Empty {
            ctx.referrer_policy = policy;
        }
        match referrer.as_deref() {
            Some("") | Some("no-referrer") => ctx.referrer = None,
            Some("about:client") | Some("client") | None => {}
            Some(r) => {
                // Fetch §5.4 step 30: a referrer URL that is not same origin with the client becomes the client.
                if Origin::of(r).same_origin(&ctx.client_origin) {
                    ctx.referrer = Some(r.to_string());
                }
            }
        }
        let before = headers.len();
        let mut dropped = Vec::new();
        headers.retain(|(k, v)| {
            let bad = cors::is_forbidden_request_header(k, v);
            if bad {
                dropped.push(k.to_ascii_lowercase());
            }
            !bad
        });
        if ctx.mode == "no-cors" {
            headers.retain(|(k, v)| {
                let ok = cors::is_safelisted_request_header(k, v);
                if !ok {
                    dropped.push(k.to_ascii_lowercase());
                }
                ok
            });
        }
        if headers.len() != before {
            for n in dropped {
                crate::ledger::record_js(&format!("security-request-header-dropped:{n}"));
            }
        }
        ctx
    }

    /// On the page thread, for a script / module load from the document.
    pub fn for_script(module: bool, crossorigin: Option<&str>, integrity: &str) -> RequestContext {
        let (mode, cred) = match (module, crossorigin.map(|c| c.trim().to_ascii_lowercase())) {
            (_, Some(c)) if c == "use-credentials" => ("cors", "include"),
            (_, Some(_)) => ("cors", "same-origin"),
            (true, None) => ("cors", "same-origin"),
            (false, None) => ("no-cors", "include"),
        };
        let mut ctx = RequestContext::for_page(mode, cred, "script");
        ctx.integrity = integrity.to_string();
        ctx
    }

    /// Fetch §4.5 step: are credentials (cookies) included for a hop to `target`?
    pub fn include_credentials(&self, target: &Origin) -> bool {
        if !self.has_page {
            return true;
        }
        match self.credentials.as_str() {
            "include" => true,
            "same-origin" => self.client_origin.same_origin(target),
            _ => false,
        }
    }

    /// The `Origin`, `Referer` and `Sec-Fetch-*` headers for one hop (Fetch §3.1 "append a request Origin
    /// header", §4.1 step 9 the referrer, Fetch Metadata §2).
    pub fn hop_headers(&self, url: &str, method: &str, site: FetchSite, cors_tainting: bool, origin_ser: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if !self.has_page {
            return out;
        }
        if cors_tainting {
            out.push(("origin".to_string(), origin_ser.to_string()));
        } else if !matches!(method, "GET" | "HEAD") {
            let downgrade = self.client_origin.scheme() == "https" && !url.starts_with("https:");
            let same = self.client_origin.same_origin(&Origin::of(url));
            let null = match self.referrer_policy.effective() {
                ReferrerPolicy::NoReferrer => true,
                ReferrerPolicy::NoReferrerWhenDowngrade | ReferrerPolicy::StrictOrigin | ReferrerPolicy::StrictOriginWhenCrossOrigin => downgrade,
                ReferrerPolicy::SameOrigin => !same,
                _ => false,
            };
            out.push(("origin".to_string(), if null { "null".to_string() } else { origin_ser.to_string() }));
        }
        if let Some(r) = self.referrer.as_deref().and_then(|r| referrer::determine(self.referrer_policy, r, url)) {
            out.push(("referer".to_string(), r));
        }
        out.extend(origin::sec_fetch_headers(url, site, &self.mode, &self.destination, false));
        out
    }

    /// Mixed content + CSP for (one hop of) this request to `url`: the URL to fetch (maybe upgraded), or the
    /// refusal error string. `redirected`: a redirect hop (CSP paths are not compared).
    pub fn check_hop(&self, url: &str, redirected: bool) -> Result<String, String> {
        if !self.has_page {
            return Ok(url.to_string());
        }
        let p = &self.policies;
        let mut url = url.to_string();
        match mixed::check(&self.client_url, &url, &self.destination, p.upgrade_insecure(), p.block_all_mixed()) {
            mixed::Verdict::Allow => {}
            mixed::Verdict::Upgrade(u) => url = u,
            mixed::Verdict::Block(why) => {
                return Err(refusal(
                    why,
                    &url,
                    &format!("Mixed Content: the page at '{}' was loaded over HTTPS, but requested an insecure resource '{url}'. This request has been blocked.", self.client_url),
                    None,
                ));
            }
        }
        let directive = csp::directive_for_destination(&self.destination);
        if self.destination != "script" {
            let v = p.check_url(directive, &url, redirected);
            if !v.violations.is_empty() {
                let ev = report_violations(&v, None);
                if !v.allowed {
                    return Err(format!("{ERR_PREFIX}Refused to connect to '{url}' (Content Security Policy {directive})\u{1}{}", ev.unwrap_or_default()));
                }
            }
        }
        Ok(url)
    }

    /// The request-level pre-checks before anything is sent (page thread): `same-origin` mode, the `no-cors`
    /// method safelist, then [`check_hop`](Self::check_hop).
    pub fn precheck(&self, url: &str, method: &str) -> Result<String, String> {
        if !self.has_page {
            return Ok(url.to_string());
        }
        let target = Origin::of(url);
        if self.mode == "same-origin" && !self.client_origin.same_origin(&target) {
            return Err(refusal("same-origin-mode", url, &format!("Request mode is \"same-origin\" but the URL's origin ({}) is not the request's origin ({}).", target.serialize(), self.client_origin.serialize()), None));
        }
        if self.mode == "no-cors" && !cors::is_safelisted_method(method) {
            return Err(refusal("no-cors-method", method, &format!("'{method}' is unsupported in no-cors mode."), None));
        }
        self.check_hop(url, false)
    }

    /// The cookie context of a hop to `url` (SameSite: cross-site against the top-level site; partitioned
    /// cookies keyed by the top-level site).
    pub fn cookie_context(&self, url: &str, method: &str) -> gneiss_pal::api::http::CookieContext {
        gneiss_pal::api::http::CookieContext {
            cross_site: self.has_page && origin::site_of(url) != self.top_site,
            top_level_navigation: self.mode == "navigate",
            safe_method: matches!(method, "GET" | "HEAD" | "OPTIONS"),
            partition: if self.has_page { crate::storage::cookie_partition_key(&self.client_url) } else { None },
        }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The `Cookie` header for a hop (the shared jar, under the hop's SameSite / partition context).
pub fn cookie_header(ctx: &RequestContext, url: &str, method: &str) -> Option<String> {
    let u = gneiss_pal::api::http::Url::parse(url).ok()?;
    let cc = ctx.cookie_context(url, method);
    crate::net::cookie_jar().lock().ok()?.cookie_header_in(&u, now_secs(), true, &cc)
}

/// Stores a hop's `Set-Cookie` values (refusals under RFC 6265bis are counted in the ledger).
pub fn store_cookies<'a>(ctx: &RequestContext, url: &str, method: &str, values: impl Iterator<Item = &'a str>) {
    let Ok(u) = gneiss_pal::api::http::Url::parse(url) else { return };
    let cc = ctx.cookie_context(url, method);
    let jar_arc = crate::net::cookie_jar();
    let Ok(mut jar) = jar_arc.lock() else { return };
    for v in values {
        if !jar.set_cookie_in(&u, v, now_secs(), true, &cc) {
            let name = v.split('=').next().unwrap_or("").trim();
            refuse("cookie-refused", name, &format!("Set-Cookie '{name}' from {url} was refused (RFC 6265bis: Secure / prefix / SameSite / Partitioned rules, or expired)."));
        }
    }
}

// ------------------------------------------------------------------------------------------- scripts

/// The loader's gate before a script element runs (or is fetched): CSP (inline: script-src-elem with the
/// element's nonce and the source's hash; external: the pre-request check with nonce, integrity,
/// `'strict-dynamic'`) and mixed content for an external URL. `false`: do not run (the loader fires `error`
/// at an external script). The violation event targets the element.
pub fn script_allowed(el: usize, url: Option<&str>, inline_source: Option<&str>, parser_inserted: bool) -> bool {
    let Some(p) = current_policies() else { return true };
    let nonce = crate::js::dom::attr_value(el, "nonce").unwrap_or_default();
    let v = match (url, inline_source) {
        (Some(u), _) => {
            let client = DOC.with(|d| d.borrow().url.clone());
            match mixed::check(&client, u, "script", p.upgrade_insecure(), p.block_all_mixed()) {
                mixed::Verdict::Block(why) => {
                    refuse(why, u, &format!("Mixed Content: the page at '{client}' requested an insecure script '{u}'. This request has been blocked."));
                    return false;
                }
                _ => {}
            }
            let integrity = crate::js::dom::attr_value(el, "integrity").unwrap_or_default();
            p.check_external_script(u, &nonce, &integrity, parser_inserted)
        }
        (None, Some(src)) => p.check_inline("script-src-elem", &nonce, src),
        (None, None) => return true,
    };
    if !v.violations.is_empty() {
        report_violations(&v, Some(el));
    }
    v.allowed
}

/// A module import (js/host.rs `load_module`): script-src-elem for the URL (imports are never
/// parser-inserted, so `'strict-dynamic'` admits them) and mixed content.
pub fn module_import_allowed(url: &str) -> Result<(), String> {
    let Some(p) = current_policies() else { return Ok(()) };
    let client = DOC.with(|d| d.borrow().url.clone());
    if let mixed::Verdict::Block(why) = mixed::check(&client, url, "script", p.upgrade_insecure(), p.block_all_mixed()) {
        refuse(why, url, &format!("Mixed Content: an insecure module '{url}' was blocked."));
        return Err(format!("Mixed Content: blocked insecure module {url}"));
    }
    let v = p.check_external_script(url, "", "", false);
    if !v.violations.is_empty() {
        report_violations(&v, None);
    }
    if v.allowed { Ok(()) } else { Err(format!("Refused to load the module script '{url}' (Content Security Policy)")) }
}

/// Subresource Integrity for a fetched script's text (the loader, after the fetch). `eligible`: the
/// response was same-origin or CORS (an opaque one fails whenever there is metadata).
pub fn script_integrity_ok(el: usize, url: &str, source: &str, eligible: bool) -> bool {
    let Some(integrity) = crate::js::dom::attr_value(el, "integrity") else { return true };
    match sri::check(&integrity, source.as_bytes(), eligible) {
        Ok(()) => true,
        Err(e) => {
            refuse("sri", url, &format!("Failed to find a valid digest in the 'integrity' attribute for resource '{url}': {e}. The resource has been blocked."));
            false
        }
    }
}

// ------------------------------------------------------------------------------------------- navigation

/// The request headers a navigation from the document at `from` (or the user: `from` empty) to `target`
/// carries: `Referer` under `policy`, `Sec-Fetch-*` (mode navigate, destination document) and
/// `Upgrade-Insecure-Requests: 1` (UIR §3.2). For the navigation code (net/mod.rs).
pub fn navigation_headers(from: &str, policy: ReferrerPolicy, target: &str, user_activated: bool) -> Vec<(String, String)> {
    let mut out = vec![("upgrade-insecure-requests".to_string(), "1".to_string())];
    let site = if from.is_empty() { FetchSite::None } else { FetchSite::None.step(&Origin::of(from), &Origin::of(target)) };
    if !from.is_empty() {
        if let Some(r) = referrer::determine(policy, from, target) {
            out.push(("referer".to_string(), r));
        }
    }
    out.extend(origin::sec_fetch_headers(target, site, "navigate", "document", user_activated || from.is_empty()));
    out
}

// ------------------------------------------------------------------------------------------- embedding

/// A response an embedder asks about: its URL and its header values by (case-insensitive) name.
pub trait Embeddable {
    fn embed_url(&self) -> String;
    fn embed_header_values(&self, name: &str) -> Vec<String>;
}

impl Embeddable for gneiss_pal::api::http::Response {
    fn embed_url(&self) -> String {
        self.url().href()
    }
    fn embed_header_values(&self, name: &str) -> Vec<String> {
        self.headers().get_all(name).map(str::to_string).collect()
    }
}

impl Embeddable for gneiss_pal::api::http::blocking::Response {
    fn embed_url(&self) -> String {
        self.url().href()
    }
    fn embed_header_values(&self, name: &str) -> Vec<String> {
        self.headers().get_all(name).map(str::to_string).collect()
    }
}

impl Embeddable for (&str, &[(String, String)]) {
    fn embed_url(&self) -> String {
        self.0.to_string()
    }
    fn embed_header_values(&self, name: &str) -> Vec<String> {
        self.1.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()).collect()
    }
}

/// AETHERFRAMES' one question: may the document `response` delivers be embedded in a frame whose parent
/// document has origin `parent_origin` (serialized, `null` for opaque)? CSP `frame-ancestors` (enforced
/// response policies) decides when present (CSP3 §6.4.2, it obsoletes X-Frame-Options); otherwise
/// `X-Frame-Options` (HTML §7.1.? "check a navigation response's adherence to X-Frame-Options"): `DENY`
/// never, `SAMEORIGIN` only for a same-origin parent, a conflicting list never, an unknown value always.
/// A refusal is ledgered (`security-frame-ancestors` / `security-x-frame-options`) and logged.
pub fn may_embed<R: Embeddable + ?Sized>(parent_origin: &str, response: &R) -> bool {
    let parent = if parent_origin == "null" { Origin::opaque() } else { Origin::of(parent_origin) };
    let url = response.embed_url();
    let list = csp::PolicyList::from_header_values(&response.embed_header_values("content-security-policy"), &url);
    if let Some(v) = list.check_frame_ancestors(&parent) {
        if !v.allowed {
            refuse(
                "frame-ancestors",
                &url,
                &format!("Refused to frame '{url}' because an ancestor ({}) violates the Content Security Policy directive frame-ancestors.", parent.serialize()),
            );
        }
        return v.allowed;
    }
    let mut set: Vec<String> = Vec::new();
    for v in response.embed_header_values("x-frame-options") {
        for t in v.split(',') {
            let t = t.trim().to_ascii_lowercase();
            if !t.is_empty() && !set.contains(&t) {
                set.push(t);
            }
        }
    }
    let deny = |why: &str| {
        refuse("x-frame-options", &url, &format!("Refused to display '{url}' in a frame because it set 'X-Frame-Options' to '{why}'."));
        false
    };
    if set.len() > 1 {
        if set.iter().any(|t| matches!(t.as_str(), "deny" | "allowall" | "sameorigin")) {
            return deny(&set.join(", "));
        }
        return true;
    }
    match set.first().map(String::as_str) {
        Some("deny") => deny("deny"),
        Some("sameorigin") => {
            if parent.same_origin(&Origin::of(&url)) {
                true
            } else {
                deny("sameorigin")
            }
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn embedding() {
        let url = "https://a.example/frame";
        let deny = h(&[("x-frame-options", "DENY")]);
        assert!(!may_embed("https://a.example", &(url, deny.as_slice())));
        let same = h(&[("x-frame-options", "SAMEORIGIN")]);
        assert!(may_embed("https://a.example", &(url, same.as_slice())));
        assert!(!may_embed("https://evil.test", &(url, same.as_slice())));
        let conflict = h(&[("x-frame-options", "SAMEORIGIN, DENY")]);
        assert!(!may_embed("https://a.example", &(url, conflict.as_slice())));
        let fa = h(&[("x-frame-options", "DENY"), ("content-security-policy", "frame-ancestors https://partner.example")]);
        assert!(may_embed("https://partner.example", &(url, fa.as_slice())), "frame-ancestors overrides X-Frame-Options");
        assert!(!may_embed("null", &(url, fa.as_slice())));
        assert!(may_embed("https://evil.test", &(url, h(&[]).as_slice())));
    }

    #[test]
    fn navigation_and_endpoints() {
        let n = navigation_headers("https://a.example/p?q", ReferrerPolicy::Empty, "https://b.example/", false);
        assert!(n.iter().any(|(k, v)| k == "referer" && v == "https://a.example/"));
        assert!(n.iter().any(|(k, v)| k == "sec-fetch-site" && v == "cross-site"));
        assert_eq!(parse_reporting_endpoints("csp=\"https://r.example/csp\", other=\"/x\"")[0].1, "https://r.example/csp");
    }
}
