//! Content Security Policy Level 3 (W3C CSP3): parsing (§2.2 serialized policies, §2.3 the header's
//! comma-separated list), the directive fallback lists (§6.8.3), source-list matching (§6.7.2: `*`,
//! `'self'`, scheme-sources, host-sources with wildcards, ports and paths; paths ignored after a redirect),
//! the inline checks (§6.7.3: `'unsafe-inline'`, nonces, hashes, `'unsafe-hashes'` for attributes,
//! `'strict-dynamic'`), the external-script pre-request check (nonce, integrity-hash, `'strict-dynamic'`
//! for non-parser-inserted scripts, then the URL), `'unsafe-eval'`, `frame-ancestors`, `base-uri`,
//! `form-action`, `upgrade-insecure-requests` and `block-all-mixed-content`; violations (§5) become a
//! `SecurityPolicyViolationEvent` at the element or document AND a POST to `report-uri` (`application/
//! csp-report`) or a `report-to` endpoint named by `Reporting-Endpoints` (`application/reports+json`).
//!
//! The page-facing half is [`arm`]: before each script runs the loader asks it to (re)read the document's
//! policies (`security::sync`), install `SecurityPolicyViolationEvent` and the violation dispatcher once per
//! document, refuse `document.domain` relaxation, and — when `'unsafe-eval'` is not allowed — guard
//! `eval` / `Function` (and the async/generator Function constructors). OWED to js_core: a host hook for
//! HostEnsureCanCompileStrings (string `setTimeout` handlers compile in js/mod.rs `run_timer_source`, and
//! inline event-handler attributes and `javascript:` URLs compile in js/events.rs, neither of which this arc
//! may touch; both are listed in docs/SECURITY.md as owed).

use html_core::url;
use super::origin::Origin;
use std::sync::atomic::{AtomicU32, Ordering};

/// One serialized policy (§2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// (lowercase directive name, source expressions) in order; a repeated name is ignored (§2.2.1 step 2.4).
    pub directives: Vec<(String, Vec<String>)>,
    pub report_only: bool,
    /// Delivered by `<meta http-equiv>` (frame-ancestors, report-uri and sandbox are ignored there, §4.2).
    pub from_meta: bool,
    pub raw: String,
}

fn is_directive_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl Policy {
    /// §2.2.1 "parse a serialized CSP".
    pub fn parse(serialized: &str, report_only: bool, from_meta: bool) -> Policy {
        let mut directives: Vec<(String, Vec<String>)> = Vec::new();
        for token in serialized.split(';') {
            let mut parts = token.split_ascii_whitespace();
            let Some(name) = parts.next() else { continue };
            let name = name.to_ascii_lowercase();
            if !is_directive_name(&name) || directives.iter().any(|(n, _)| *n == name) {
                continue;
            }
            if from_meta && matches!(name.as_str(), "frame-ancestors" | "report-uri" | "sandbox") {
                crate::ledger::record_js(&format!("security-csp-meta-ignores:{name}"));
                continue;
            }
            directives.push((name, parts.map(str::to_string).collect()));
        }
        Policy { directives, report_only, from_meta, raw: serialized.trim().to_string() }
    }

    /// §2.3 "parse a response's Content Security Policies": the header value is a comma-separated list.
    pub fn parse_header(value: &str, report_only: bool) -> Vec<Policy> {
        value.split(',').map(|p| Policy::parse(p, report_only, false)).filter(|p| !p.directives.is_empty()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&Vec<String>> {
        self.directives.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
}

/// §6.8.3 the directive fallback list for an effective directive.
pub fn fallback_list(directive: &str) -> &'static [&'static str] {
    match directive {
        "script-src-elem" => &["script-src-elem", "script-src", "default-src"],
        "script-src-attr" => &["script-src-attr", "script-src", "default-src"],
        "script-src" => &["script-src", "default-src"],
        "style-src-elem" => &["style-src-elem", "style-src", "default-src"],
        "style-src-attr" => &["style-src-attr", "style-src", "default-src"],
        "style-src" => &["style-src", "default-src"],
        "worker-src" => &["worker-src", "child-src", "script-src", "default-src"],
        "frame-src" => &["frame-src", "child-src", "default-src"],
        "child-src" => &["child-src", "default-src"],
        "connect-src" => &["connect-src", "default-src"],
        "img-src" => &["img-src", "default-src"],
        "font-src" => &["font-src", "default-src"],
        "media-src" => &["media-src", "default-src"],
        "object-src" => &["object-src", "default-src"],
        "manifest-src" => &["manifest-src", "default-src"],
        "base-uri" => &["base-uri"],
        "form-action" => &["form-action"],
        "frame-ancestors" => &["frame-ancestors"],
        _ => &["default-src"],
    }
}

/// The CSP directive a fetch destination is governed by (§6.8.1 "get the effective directive for request").
pub fn directive_for_destination(dest: &str) -> &'static str {
    match dest {
        "" | "fetch" | "xhr" | "eventsource" | "websocket" | "beacon" => "connect-src",
        "script" | "module" => "script-src-elem",
        "style" => "style-src-elem",
        "image" => "img-src",
        "font" => "font-src",
        "audio" | "video" | "track" => "media-src",
        "iframe" | "frame" => "frame-src",
        "object" | "embed" => "object-src",
        "worker" | "sharedworker" | "serviceworker" => "worker-src",
        "manifest" => "manifest-src",
        _ => "default-src",
    }
}

/// One violation (§2.4): what the report and the event carry.
#[derive(Debug, Clone)]
pub struct Violation {
    pub effective_directive: String,
    /// The directive (with its value) that did not match, as the policy spells it.
    pub violated_directive: String,
    pub blocked: String,
    pub original_policy: String,
    pub report_only: bool,
    pub sample: String,
    pub report_uri: Vec<String>,
    pub report_to: Option<String>,
}

/// The outcome of a check: whether the action proceeds, and every violation to report (report-only
/// policies report without blocking).
#[derive(Debug, Clone, Default)]
pub struct Verdict {
    pub allowed: bool,
    pub violations: Vec<Violation>,
}

/// The policies of one document (or one response, for `frame-ancestors`) and the origin `'self'` means.
#[derive(Debug, Clone)]
pub struct PolicyList {
    pub policies: Vec<Policy>,
    pub self_origin: Origin,
    pub self_url: String,
}

impl Default for PolicyList {
    fn default() -> Self {
        PolicyList { policies: Vec::new(), self_origin: Origin::opaque(), self_url: String::new() }
    }
}

fn scheme_part_match(a: &str, b: &str) -> bool {
    a == b || (a == "http" && b == "https") || (a == "ws" && matches!(b, "wss" | "http" | "https")) || (a == "wss" && b == "https")
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
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

fn is_keyword(e: &str) -> bool {
    e.starts_with('\'')
}

/// §6.7.2.8 "does url match expression in origin with redirect count".
fn url_matches_expression(expr: &str, url: &url::Url, self_origin: &Origin, redirected: bool) -> bool {
    let e = expr.to_ascii_lowercase();
    let scheme = url.scheme();
    if e == "*" {
        return matches!(scheme, "http" | "https" | "ws" | "wss") || (!self_origin.is_opaque() && scheme == self_origin.scheme());
    }
    if e == "'self'" {
        if self_origin.same_origin(&Origin::of_url(url)) {
            return true;
        }
        // The secure upgrade of 'self' (http → https, ws → wss, on the default ports) also matches.
        if let Origin::Tuple { scheme: s, host, port } = self_origin {
            let host_ok = { let hs = url.host_str(); !hs.is_empty() && hs.eq_ignore_ascii_case(host) };
            let up = (s == "http" && matches!(scheme, "https" | "ws" | "wss")) || (s == "https" && scheme == "wss") || (s == "ws" && scheme == "wss");
            let port_ok = port.is_none() && url.port().is_none();
            return host_ok && up && port_ok;
        }
        return false;
    }
    if is_keyword(&e) {
        return false;
    }
    // scheme-source: `https:`.
    if let Some(s) = e.strip_suffix(':') {
        if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
            return scheme_part_match(s, scheme);
        }
    }
    // host-source: [scheme "://"] host [":" port] [path].
    let (src_scheme, rest) = match e.find("://") {
        Some(i) => (Some(&e[..i]), &e[i + 3..]),
        None => (None, e.as_str()),
    };
    match src_scheme {
        Some(s) => {
            if !scheme_part_match(s, scheme) {
                return false;
            }
        }
        None => {
            let base = if self_origin.is_opaque() { "https" } else { self_origin.scheme() };
            if !scheme_part_match(base, scheme) && !(base == "http" && scheme == "https") {
                return false;
            }
        }
    }
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host_pat, port_pat) = if hostport.starts_with('[') {
        match hostport.find(']') {
            Some(i) => (&hostport[..=i], hostport[i + 1..].strip_prefix(':')),
            None => return false,
        }
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        }
    };
    let url_host = url.host_str();
    if url_host.is_empty() { return false; }
    let url_host = url_host.to_ascii_lowercase();
    let host_ok = if host_pat == "*" {
        true
    } else if let Some(suffix) = host_pat.strip_prefix("*.") {
        url_host.ends_with(&format!(".{suffix}"))
    } else {
        url_host == host_pat
    };
    if !host_ok {
        return false;
    }
    let url_port = url.port_or_default();
    let port_ok = match port_pat {
        Some("*") => true,
        Some(p) => match p.parse::<u16>() {
            Ok(n) => Some(n) == url_port || (n == 80 && url_port == Some(443) && matches!(scheme, "https" | "wss")),
            Err(_) => false,
        },
        None => {
            let def = default_port(src_scheme.unwrap_or(scheme));
            url_port == default_port(scheme) || url_port == def || (def == Some(80) && url_port == Some(443))
        }
    };
    if !port_ok {
        return false;
    }
    if path.is_empty() || path == "/" || redirected {
        return true;
    }
    let want = percent_decode(path);
    let got = percent_decode(&url.pathname());
    if want.ends_with('/') { got.starts_with(&want) } else { got == want }
}

/// §6.7.2.5 "does url match source list".
pub fn url_matches_list(list: &[String], url: &str, self_origin: &Origin, redirected: bool) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    if list.is_empty() || (list.len() == 1 && list[0].eq_ignore_ascii_case("'none'")) {
        return false;
    }
    list.iter().any(|e| url_matches_expression(e, &u, self_origin, redirected))
}

fn contains_kw(list: &[String], kw: &str) -> bool {
    list.iter().any(|e| e.eq_ignore_ascii_case(kw))
}

fn nonce_of(e: &str) -> Option<&str> {
    let lower = e.to_ascii_lowercase();
    if lower.starts_with("'nonce-") && e.ends_with('\'') && e.len() > 8 { Some(&e[7..e.len() - 1]) } else { None }
}

fn hash_of(e: &str) -> Option<(super::sri::Alg, String)> {
    let inner = e.strip_prefix('\'')?.strip_suffix('\'')?;
    let items = super::sri::parse(inner);
    items.into_iter().next()
}

fn has_nonce_or_hash(list: &[String]) -> bool {
    list.iter().any(|e| nonce_of(e).is_some() || hash_of(e).is_some())
}

/// §6.7.3.3 "does element match source list for type and source" (`attr`: an attribute — event handler
/// or `style=` — where nonces never apply and hashes only with `'unsafe-hashes'`).
pub fn inline_matches_list(list: &[String], is_script: bool, attr: bool, nonce: &str, source: &str) -> bool {
    let strict_dynamic = is_script && contains_kw(list, "'strict-dynamic'");
    if contains_kw(list, "'unsafe-inline'") && !has_nonce_or_hash(list) && !strict_dynamic {
        return true;
    }
    if !attr && !nonce.is_empty() && list.iter().any(|e| nonce_of(e) == Some(nonce)) {
        return true;
    }
    if !attr || contains_kw(list, "'unsafe-hashes'") {
        for e in list {
            if let Some((alg, digest)) = hash_of(e) {
                if super::sri::normalize_b64(&alg.digest_b64(source.as_bytes())) == super::sri::normalize_b64(&digest) {
                    return true;
                }
            }
        }
    }
    false
}

impl PolicyList {
    pub fn new(policies: Vec<Policy>, self_url: &str) -> PolicyList {
        PolicyList { policies, self_origin: Origin::of(self_url), self_url: self_url.to_string() }
    }

    /// The response policies of `Content-Security-Policy` values (enforced) for `frame-ancestors`.
    pub fn from_header_values(values: &[String], self_url: &str) -> PolicyList {
        let policies = values.iter().flat_map(|v| Policy::parse_header(v, false)).collect();
        PolicyList::new(policies, self_url)
    }

    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    fn evaluate(&self, effective: &str, blocked: &str, sample: &str, mut pass: impl FnMut(&[String]) -> bool) -> Verdict {
        let mut v = Verdict { allowed: true, violations: Vec::new() };
        for p in &self.policies {
            let Some((name, list)) = fallback_list(effective).iter().find_map(|n| p.get(n).map(|l| (*n, l))) else { continue };
            if pass(list) {
                continue;
            }
            if !p.report_only {
                v.allowed = false;
            }
            v.violations.push(Violation {
                effective_directive: effective.to_string(),
                violated_directive: format!("{name} {}", list.join(" ")).trim().to_string(),
                blocked: blocked.to_string(),
                original_policy: p.raw.clone(),
                report_only: p.report_only,
                sample: if list.iter().any(|e| e.eq_ignore_ascii_case("'report-sample'")) { sample.chars().take(40).collect() } else { String::new() },
                report_uri: if p.from_meta { Vec::new() } else { p.get("report-uri").cloned().unwrap_or_default() },
                report_to: p.get("report-to").and_then(|l| l.first().cloned()),
            });
        }
        v
    }

    /// A fetch of `url` governed by `directive` (connect-src, img-src, …; `redirected` ignores paths).
    pub fn check_url(&self, directive: &str, url: &str, redirected: bool) -> Verdict {
        if self.is_empty() {
            return Verdict { allowed: true, violations: Vec::new() };
        }
        let so = self.self_origin.clone();
        self.evaluate(directive, url, "", |l| url_matches_list(l, url, &so, redirected))
    }

    /// An inline `<script>` / `<style>` element or a script/style attribute.
    pub fn check_inline(&self, directive: &str, nonce: &str, source: &str) -> Verdict {
        if self.is_empty() {
            return Verdict { allowed: true, violations: Vec::new() };
        }
        let is_script = directive.starts_with("script");
        let attr = directive.ends_with("-attr");
        self.evaluate(directive, "inline", source, |l| inline_matches_list(l, is_script, attr, nonce, source))
    }

    /// §8.? the script-src-elem pre-request check for an external script.
    pub fn check_external_script(&self, url: &str, nonce: &str, integrity: &str, parser_inserted: bool) -> Verdict {
        if self.is_empty() {
            return Verdict { allowed: true, violations: Vec::new() };
        }
        let so = self.self_origin.clone();
        let sri = super::sri::parse(integrity);
        self.evaluate("script-src-elem", url, "", |l| {
            if !nonce.is_empty() && l.iter().any(|e| nonce_of(e) == Some(nonce)) {
                return true;
            }
            if !sri.is_empty() {
                let hashes: Vec<(super::sri::Alg, String)> = l.iter().filter_map(|e| hash_of(e)).collect();
                if !hashes.is_empty()
                    && sri.iter().all(|(a, d)| hashes.iter().any(|(ha, hd)| ha == a && super::sri::normalize_b64(hd) == super::sri::normalize_b64(d)))
                {
                    return true;
                }
            }
            if contains_kw(l, "'strict-dynamic'") {
                return !parser_inserted;
            }
            url_matches_list(l, url, &so, false)
        })
    }

    /// The `style-src-elem` pre-request check for an external stylesheet (`<link rel=stylesheet>`,
    /// `@import`): the link's nonce, else the URL against the source list.
    pub fn check_external_style(&self, url: &str, nonce: &str) -> Verdict {
        if self.is_empty() {
            return Verdict { allowed: true, violations: Vec::new() };
        }
        let so = self.self_origin.clone();
        self.evaluate("style-src-elem", url, "", |l| {
            (!nonce.is_empty() && l.iter().any(|e| nonce_of(e) == Some(nonce))) || url_matches_list(l, url, &so, false)
        })
    }

    /// `eval()` / `new Function()` (§4.4 EnsureCSPDoesNotBlockStringCompilation).
    pub fn check_eval(&self, sample: &str) -> Verdict {
        if self.is_empty() {
            return Verdict { allowed: true, violations: Vec::new() };
        }
        self.evaluate("script-src", "eval", sample, |l| contains_kw(l, "'unsafe-eval'"))
    }

    /// `frame-ancestors` for an embedding by `ancestor` (`None` when no enforced policy has the directive).
    pub fn check_frame_ancestors(&self, ancestor: &Origin) -> Option<Verdict> {
        if !self.policies.iter().any(|p| !p.report_only && p.has("frame-ancestors")) {
            return None;
        }
        let url = format!("{}/", ancestor.serialize());
        let so = self.self_origin.clone();
        let opaque = ancestor.is_opaque();
        Some(self.evaluate("frame-ancestors", &url, "", |l| !opaque && url_matches_list(l, &url, &so, false)))
    }

    /// `upgrade-insecure-requests` in any enforced policy.
    pub fn upgrade_insecure(&self) -> bool {
        self.policies.iter().any(|p| !p.report_only && p.has("upgrade-insecure-requests"))
    }

    /// `block-all-mixed-content` in any enforced policy.
    pub fn block_all_mixed(&self) -> bool {
        self.policies.iter().any(|p| !p.report_only && p.has("block-all-mixed-content"))
    }
}

// ------------------------------------------------------------------------------------------- reporting

/// Reports this process sends at most (a page in a violation loop cannot flood a collector).
const REPORT_CAP: u32 = 256;
static REPORTS_SENT: AtomicU32 = AtomicU32::new(0);

/// The console line for a violation.
pub fn message(v: &Violation) -> String {
    let what = match v.blocked.as_str() {
        "inline" => "execute or apply an inline resource".to_string(),
        "eval" => "evaluate a string as JavaScript".to_string(),
        u => format!("load '{u}'"),
    };
    format!(
        "{}Refused to {what} because it violates the following Content Security Policy directive: \"{}\".",
        if v.report_only { "[Report Only] " } else { "" },
        v.violated_directive
    )
}

/// The `SecurityPolicyViolationEventInit` dictionary for a violation (JSON).
pub fn event_init(v: &Violation, document_url: &str) -> String {
    serde_json::json!({
        "documentURI": document_url,
        "referrer": "",
        "blockedURI": v.blocked,
        "violatedDirective": v.effective_directive,
        "effectiveDirective": v.effective_directive,
        "originalPolicy": v.original_policy,
        "sourceFile": "",
        "sample": v.sample,
        "disposition": if v.report_only { "report" } else { "enforce" },
        "statusCode": 200,
        "lineNumber": 0,
        "columnNumber": 0,
    })
    .to_string()
}

/// §5.5 "report a violation": a POST per `report-uri` (the legacy `application/csp-report` body) and, for
/// `report-to`, one to the endpoint `Reporting-Endpoints` named (`application/reports+json`). Sent off the
/// page thread, credentials omitted, fire and forget.
pub fn send_reports(v: &Violation, document_url: &str, endpoints: &[(String, String)]) {
    let mut posts: Vec<(String, &'static str, String)> = Vec::new();
    let base = url::Url::parse(document_url).ok();
    let resolve = |u: &str| -> Option<String> {
        match &base {
            Some(b) => b.join(u).ok().map(|x| x.to_string()),
            None => url::Url::parse(u).ok().map(|x| x.to_string()),
        }
    };
    let blocked = if v.blocked.starts_with("http") { super::referrer::strip(&v.blocked, false).unwrap_or_default() } else { v.blocked.clone() };
    if let Some(name) = &v.report_to {
        if let Some((_, ep)) = endpoints.iter().find(|(n, _)| n == name) {
            if let Some(u) = resolve(ep) {
                let body = serde_json::json!([{
                    "type": "csp-violation",
                    "age": 0,
                    "url": document_url,
                    "user_agent": "UnaOS Aether/0.1.0",
                    "body": {
                        "documentURL": document_url,
                        "blockedURL": blocked,
                        "effectiveDirective": v.effective_directive,
                        "originalPolicy": v.original_policy,
                        "sample": v.sample,
                        "disposition": if v.report_only { "report" } else { "enforce" },
                        "statusCode": 200,
                    }
                }]);
                posts.push((u, "application/reports+json", body.to_string()));
            }
        }
    } else {
        for r in &v.report_uri {
            if let Some(u) = resolve(r) {
                let body = serde_json::json!({ "csp-report": {
                    "document-uri": document_url,
                    "referrer": "",
                    "violated-directive": v.effective_directive,
                    "effective-directive": v.effective_directive,
                    "original-policy": v.original_policy,
                    "disposition": if v.report_only { "report" } else { "enforce" },
                    "blocked-uri": blocked,
                    "status-code": 200,
                    "script-sample": v.sample,
                }});
                posts.push((u, "application/csp-report", body.to_string()));
            }
        }
    }
    for (u, ty, body) in posts {
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            continue;
        }
        if REPORTS_SENT.fetch_add(1, Ordering::Relaxed) >= REPORT_CAP {
            crate::ledger::record_js("security-csp-report-cap-reached");
            return;
        }
        std::thread::spawn(move || {
            let c = gneiss_pal::api::http::blocking::ClientBuilder::new()
                .user_agent("UnaOS Aether/0.1.0")
                .timeout(std::time::Duration::from_secs(10))
                .max_redirects(0)
                .build();
            if let Ok(c) = c {
                let _ = c.request("POST", &u).header("content-type", ty).body(body.into_bytes()).send();
            }
        });
    }
}

// ------------------------------------------------------------------------------------------- the page half

/// Installed once per document: `SecurityPolicyViolationEvent`, the hidden dispatcher `__csp_violation(init,
/// target)` (at the element when it is connected, else the document; bubbles, composed), and the refused
/// `document.domain` setter (HTML §7.1.4, see `origin::document_domain_set`).
pub const ARM_PRELUDE: &str = r#"
(function () {
    if (typeof globalThis.__csp_violation === 'function') { return; }
    var hide = function (o, n, v) { Object.defineProperty(o, n, { value: v, writable: true, configurable: true, enumerable: false }); };
    if (typeof globalThis.SecurityPolicyViolationEvent === 'undefined') {
        var F = ['documentURI', 'referrer', 'blockedURI', 'violatedDirective', 'effectiveDirective', 'originalPolicy',
                 'sourceFile', 'sample', 'disposition', 'statusCode', 'lineNumber', 'columnNumber'];
        class SecurityPolicyViolationEvent extends Event {
            constructor(type, init) {
                if (arguments.length < 1) { throw new TypeError("Failed to construct 'SecurityPolicyViolationEvent': 1 argument required, but only 0 present."); }
                init = init || {};
                super(type, init);
                var self = this;
                F.forEach(function (k) {
                    var d = (k === 'statusCode' || k === 'lineNumber' || k === 'columnNumber') ? 0 : '';
                    var v = init[k] === undefined ? d : init[k];
                    if (k === 'disposition' && v === '') { v = 'enforce'; }
                    Object.defineProperty(self, k, { value: v, enumerable: true, configurable: true });
                });
            }
        }
        Object.defineProperty(SecurityPolicyViolationEvent.prototype, 'violatedDirective', { get: function () { return this.effectiveDirective; }, configurable: true });
        hide(globalThis, 'SecurityPolicyViolationEvent', SecurityPolicyViolationEvent);
    }
    hide(globalThis, '__csp_violation', function (init, target) {
        try {
            init.bubbles = true; init.composed = true;
            var e = new SecurityPolicyViolationEvent('securitypolicyviolation', init);
            var t = (target && target.isConnected) ? target : document;
            t.dispatchEvent(e);
        } catch (x) {}
    });
    try {
        var D = typeof Document !== 'undefined' ? Document.prototype : null;
        var d = D && Object.getOwnPropertyDescriptor(D, 'domain');
        if (d && d.get && !d.set) {
            Object.defineProperty(D, 'domain', {
                get: d.get, enumerable: d.enumerable, configurable: true,
                set: function (v) {
                    v = String(v);
                    var host = location.hostname;
                    __ledger('security-document-domain-refused');
                    if (location.origin === 'null') {
                        throw new DOMException("Failed to set the 'domain' property on 'Document': Assignment is forbidden for sandboxed iframes and opaque origins.", 'SecurityError');
                    }
                    var low = v.toLowerCase();
                    if (low !== host && !(host.slice(-(low.length + 1)) === '.' + low && low.indexOf('.') > 0)) {
                        throw new DOMException("Failed to set the 'domain' property on 'Document': '" + v + "' is not a suffix of '" + host + "'.", 'SecurityError');
                    }
                    console.warn("document.domain = '" + v + "' ignored: this document's agent cluster is origin-keyed, so its origin cannot be relaxed.");
                },
            });
        }
    } catch (x) {}
})();
"#;

/// Before a script runs (js/loader.rs): re-read the document's policies, install the page half once per
/// document, and dispatch the violation events
/// queued since the last script (fetch pre-checks, `style=` refusals, …).
pub fn arm(vm: &mut js_core::vm::Vm) {
    let (url, first, policies, events) = super::sync_for_script();
    if url.is_empty() {
        return;
    }
    if first {
        let _ = crate::js::run_classic_no_checkpoint(vm, ARM_PRELUDE, "csp-arm");
    }
    // `'unsafe-eval'` is js_core's HostEnsureCanCompileStrings hook (`js::host`, AETHERDEBT SR93).
    let _ = &policies;
    for (init, target) in events {
        dispatch_event(vm, &init, target);
    }
}

/// Fires one `securitypolicyviolation` event (at the element `target`, else the document).
pub fn dispatch_event(vm: &mut js_core::vm::Vm, init_json: &str, target: Option<usize>) {
    let t = match target {
        Some(id) => crate::js::dom::wrap(vm, id),
        None => js_core::vm::Value::Null,
    };
    let src = format!("(function (t) {{ if (typeof __csp_violation === 'function') {{ __csp_violation({init_json}, t); }} }})");
    if let Ok(f) = crate::js::run_classic_no_checkpoint(vm, &src, "csp-violation") {
        let _ = crate::js::invoke(vm, "csp-violation", &f, &js_core::vm::Value::Undefined, &[t]);
    }
}

/// The `style=` attribute hook (css/mod.rs, the cascade): `false` drops the declarations. Uses the policies
/// read at the last `sync` (the cascade must not walk the DOM it is styling); a refusal is reported once per
/// distinct attribute text per document.
pub fn allow_style_attr(text: &str) -> bool {
    let Some(p) = super::current_policies() else { return true };
    if p.is_empty() {
        return true;
    }
    let v = p.check_inline("style-src-attr", "", text);
    if !v.violations.is_empty() && super::first_report_of(&format!("style-attr:{text}")) {
        super::deferring(|| super::report_violations(&v, None));
    }
    v.allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(p: &str, url: &str) -> PolicyList {
        PolicyList::new(vec![Policy::parse(p, false, false)], url)
    }

    #[test]
    fn source_lists() {
        let so = Origin::of("https://a.example/");
        let l = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        assert!(url_matches_list(&l("'self'"), "https://a.example/x.js", &so, false));
        assert!(!url_matches_list(&l("'self'"), "https://b.example/x.js", &so, false));
        assert!(url_matches_list(&l("*.cdn.example"), "https://js.cdn.example/a", &so, false));
        assert!(!url_matches_list(&l("*.cdn.example"), "https://cdn.example/a", &so, false));
        assert!(url_matches_list(&l("https:"), "https://anything.test/", &so, false));
        assert!(!url_matches_list(&l("https:"), "http://anything.test/", &so, false));
        assert!(url_matches_list(&l("b.example/js/"), "https://b.example/js/x.js", &so, false));
        assert!(!url_matches_list(&l("b.example/js/app.js"), "https://b.example/js/x.js", &so, false));
        assert!(url_matches_list(&l("b.example/js/app.js"), "https://b.example/js/x.js", &so, true));
        assert!(url_matches_list(&l("b.example:8443"), "https://b.example:8443/", &so, false));
        assert!(!url_matches_list(&l("b.example"), "https://b.example:8443/", &so, false));
        assert!(!url_matches_list(&l("'none'"), "https://a.example/", &so, false));
        assert!(!url_matches_list(&l("*"), "data:text/plain,x", &so, false));
    }

    #[test]
    fn inline_and_eval() {
        let p = list("script-src 'nonce-abc' 'sha256-qznLcsROx4GACP2dm0UCKCzCG+HiZ1guq6ZZDob/Tng=' 'unsafe-inline'", "https://a.example/");
        assert!(p.check_inline("script-src-elem", "abc", "anything").allowed);
        assert!(p.check_inline("script-src-elem", "", "alert('Hello, world.');").allowed);
        assert!(!p.check_inline("script-src-elem", "", "other()").allowed, "a nonce/hash disables 'unsafe-inline'");
        assert!(!p.check_eval("1+1").allowed);
        let q = list("default-src 'self' 'unsafe-eval' 'unsafe-inline'", "https://a.example/");
        assert!(q.check_eval("1+1").allowed);
        assert!(q.check_inline("style-src-attr", "", "color:red").allowed);
        let sd = list("script-src 'strict-dynamic' 'nonce-n'", "https://a.example/");
        assert!(sd.check_external_script("https://evil.test/x.js", "", "", false).allowed);
        assert!(!sd.check_external_script("https://evil.test/x.js", "", "", true).allowed);
        assert!(sd.check_external_script("https://evil.test/x.js", "n", "", true).allowed);
        let ro = PolicyList::new(vec![Policy::parse("script-src 'none'", true, false)], "https://a.example/");
        let v = ro.check_inline("script-src-elem", "", "x()");
        assert!(v.allowed && v.violations.len() == 1);
    }

    #[test]
    fn frame_ancestors_and_flags() {
        let p = PolicyList::from_header_values(&["frame-ancestors 'self' https://partner.example, upgrade-insecure-requests".into()], "https://a.example/");
        assert_eq!(p.check_frame_ancestors(&Origin::of("https://a.example")).map(|v| v.allowed), Some(true));
        assert_eq!(p.check_frame_ancestors(&Origin::of("https://partner.example")).map(|v| v.allowed), Some(true));
        assert_eq!(p.check_frame_ancestors(&Origin::of("https://evil.test")).map(|v| v.allowed), Some(false));
        assert!(p.upgrade_insecure());
        let m = Policy::parse("frame-ancestors 'none'; script-src 'self'", false, true);
        assert!(!m.has("frame-ancestors"));
        assert_eq!(fallback_list("worker-src")[1], "child-src");
    }
}
