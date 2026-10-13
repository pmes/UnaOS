//! Origins (HTML §7.1.1), sites (HTML §7.1.2, schemeful), potentially trustworthy URLs (Secure Contexts
//! §3.2), the Fetch Metadata `Sec-Fetch-*` request headers, `document.domain` (refused), and
//! `Cross-Origin-Opener-Policy` (HTML §7.1.3, the browsing-context-group switch decision).
//!
//! An origin is a (scheme, host, port) tuple for `http`/`https`/`ws`/`wss`/`ftp` URLs (a `blob:` URL takes
//! its inner URL's), and OPAQUE for everything else — `data:`, `file:`, `about:`, `javascript:` — and for a
//! sandboxed document (`sandbox` without `allow-same-origin`). An opaque origin is equal only to itself:
//! each one minted here carries a process-unique serial, and it serializes as `"null"`.
//!
//! OWED: the registrable domain needs the public suffix list (http_core's cookie jar owes the same); until it
//! lands, [`site_of`] takes the last two labels, or three under the short multi-part suffixes in
//! psl_core (NETDEBT SR97).

use html_core::url;
use std::sync::atomic::{AtomicU64, Ordering};

static OPAQUE_SERIAL: AtomicU64 = AtomicU64::new(1);

/// An origin (HTML §7.1.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// An opaque origin: a serial no other origin shares.
    Opaque(u64),
    /// A tuple origin; `port` is `None` when the URL uses its scheme's default port.
    Tuple { scheme: String, host: String, port: Option<u16> },
}

impl Origin {
    /// A fresh opaque origin.
    pub fn opaque() -> Origin {
        Origin::Opaque(OPAQUE_SERIAL.fetch_add(1, Ordering::Relaxed))
    }

    /// The origin of `url` (an unparseable URL is opaque).
    pub fn of(url: &str) -> Origin {
        match url::Url::parse(url) {
            Ok(u) => Origin::of_url(&u),
            Err(_) => Origin::opaque(),
        }
    }

    pub fn of_url(u: &url::Url) -> Origin {
        // html_core::url (URLCORE SR87): a tuple origin is (scheme, host, port-or-default); an opaque one
        // serializes as "null".
        if !u.has_tuple_origin() {
            return Origin::opaque();
        }
        let scheme = u.scheme().to_string();
        let default = url::Url::parse(&format!("{scheme}://x/")).ok().and_then(|d| d.port_or_default());
        let port = u.port_or_default();
        Origin::Tuple { scheme, host: u.hostname().to_ascii_lowercase(), port: if port == default { None } else { port } }
    }

    /// The origin of a document at `url`: opaque when `sandboxed` (sandbox without allow-same-origin).
    pub fn of_document(url: &str, sandboxed: bool) -> Origin {
        if sandboxed { Origin::opaque() } else { Origin::of(url) }
    }

    pub fn is_opaque(&self) -> bool {
        matches!(self, Origin::Opaque(_))
    }

    /// HTML §7.1.1 "serialization of an origin": `scheme://host[:port]`, or `null`.
    pub fn serialize(&self) -> String {
        match self {
            Origin::Opaque(_) => "null".into(),
            Origin::Tuple { scheme, host, port: Some(p) } => format!("{scheme}://{host}:{p}"),
            Origin::Tuple { scheme, host, port: None } => format!("{scheme}://{host}"),
        }
    }

    pub fn scheme(&self) -> &str {
        match self {
            Origin::Opaque(_) => "",
            Origin::Tuple { scheme, .. } => scheme,
        }
    }

    pub fn host(&self) -> &str {
        match self {
            Origin::Opaque(_) => "",
            Origin::Tuple { host, .. } => host,
        }
    }

    /// "Same origin" (tuple equality; an opaque origin only equals itself).
    pub fn same_origin(&self, other: &Origin) -> bool {
        self == other
    }

    /// "Same site" (HTML §7.1.2, schemeful): same scheme and same registrable domain.
    pub fn same_site(&self, other: &Origin) -> bool {
        match (self, other) {
            (Origin::Opaque(a), Origin::Opaque(b)) => a == b,
            (Origin::Tuple { scheme: s1, host: h1, .. }, Origin::Tuple { scheme: s2, host: h2, .. }) => {
                s1 == s2 && registrable_domain(h1) == registrable_domain(h2)
            }
            _ => false,
        }
    }

    /// "Schemelessly same site": the registrable domains match whatever the schemes.
    pub fn schemelessly_same_site(&self, other: &Origin) -> bool {
        match (self, other) {
            (Origin::Tuple { host: h1, .. }, Origin::Tuple { host: h2, .. }) => registrable_domain(h1) == registrable_domain(h2),
            (a, b) => a == b,
        }
    }

    /// The site (HTML §7.1.2) serialized: `scheme://registrable-domain`, or `null` for an opaque origin.
    pub fn site(&self) -> String {
        match self {
            Origin::Opaque(_) => "null".into(),
            Origin::Tuple { scheme, host, .. } => format!("{scheme}://{}", registrable_domain(host)),
        }
    }

    /// Secure Contexts §3.1 "potentially trustworthy origin".
    pub fn is_potentially_trustworthy(&self) -> bool {
        match self {
            Origin::Opaque(_) => false,
            Origin::Tuple { scheme, host, .. } => matches!(scheme.as_str(), "https" | "wss") || is_loopback_host(host),
        }
    }
}

fn is_ip_host(host: &str) -> bool {
    host.starts_with('[') || host.parse::<std::net::Ipv4Addr>().is_ok()
}

fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_end_matches('.');
    h == "localhost" || h.ends_with(".localhost") || h == "[::1]" || h.parse::<std::net::Ipv4Addr>().is_ok_and(|a| a.is_loopback())
}

/// The registrable domain of `host` (approximate: see the module's OWED note); an IP address is itself.
pub fn registrable_domain(host: &str) -> String {
    // NETDEBT (SR97): the Public Suffix List (psl_core) decides; an IP address is itself.
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    if is_ip_host(&h) {
        return h;
    }
    psl_core::site_host(&h).to_string()
}

/// The site of `url`, serialized (`null` for an opaque origin).
pub fn site_of(url: &str) -> String {
    Origin::of(url).site()
}

/// Secure Contexts §3.2 "potentially trustworthy URL": `about:blank`, `about:srcdoc`, `data:` and the
/// trustworthy origins.
pub fn is_potentially_trustworthy_url(url: &str) -> bool {
    if url == "about:blank" || url == "about:srcdoc" || url.starts_with("data:") || url.starts_with("file:") {
        return true;
    }
    Origin::of(url).is_potentially_trustworthy()
}

// ------------------------------------------------------------------------------------------- Sec-Fetch-*

/// Fetch Metadata §2.4 `Sec-Fetch-Site` for a request whose chain so far is `prior` (None = first hop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FetchSite {
    None,
    SameOrigin,
    SameSite,
    CrossSite,
}

impl FetchSite {
    pub fn as_str(self) -> &'static str {
        match self {
            FetchSite::None => "none",
            FetchSite::SameOrigin => "same-origin",
            FetchSite::SameSite => "same-site",
            FetchSite::CrossSite => "cross-site",
        }
    }

    /// The value for one more hop to `target` from `client` (a chain's value is its weakest hop's).
    pub fn step(self, client: &Origin, target: &Origin) -> FetchSite {
        let hop = if client.same_origin(target) {
            FetchSite::SameOrigin
        } else if client.same_site(target) {
            FetchSite::SameSite
        } else {
            FetchSite::CrossSite
        };
        if self == FetchSite::None { hop } else { self.max(hop) }
    }
}

/// The `Sec-Fetch-*` headers for a hop to `url` (Fetch Metadata §2: only to a potentially trustworthy URL).
pub fn sec_fetch_headers(url: &str, site: FetchSite, mode: &str, dest: &str, user_activated_navigation: bool) -> Vec<(String, String)> {
    if !is_potentially_trustworthy_url(url) {
        return Vec::new();
    }
    let mut out = vec![
        ("sec-fetch-dest".to_string(), if dest.is_empty() { "empty".into() } else { dest.to_string() }),
        ("sec-fetch-mode".to_string(), mode.to_string()),
        ("sec-fetch-site".to_string(), site.as_str().to_string()),
    ];
    if mode == "navigate" && user_activated_navigation {
        out.push(("sec-fetch-user".to_string(), "?1".to_string()));
    }
    out
}

// ------------------------------------------------------------------------------------------- document.domain

/// HTML §7.1.4 `document.domain` setter: REFUSED. An opaque origin throws `SecurityError` (the spec's own
/// step); otherwise origin-keyed agent clusters are the default here (as in current engines), so relaxing the
/// origin is a no-op: the answer is `Err(message)` for the console and the ledger, and the origin stands.
pub fn document_domain_set(document_url: &str, sandboxed: bool, value: &str) -> Result<(), (bool, String)> {
    let o = Origin::of_document(document_url, sandboxed);
    if o.is_opaque() {
        return Err((true, "Failed to set the 'domain' property on 'Document': Assignment is forbidden for sandboxed iframes and opaque origins.".into()));
    }
    let host = o.host().to_string();
    let v = value.trim().to_ascii_lowercase();
    if v != host && !(host.ends_with(&format!(".{v}")) && v.contains('.')) {
        return Err((true, format!("Failed to set the 'domain' property on 'Document': '{value}' is not a suffix of '{host}'.")));
    }
    Err((false, format!("document.domain = '{value}' ignored: this document's agent cluster is origin-keyed, so its origin ({}) cannot be relaxed.", o.serialize())))
}

// ------------------------------------------------------------------------------------------- COOP

/// HTML §7.1.3.1 a cross-origin opener policy value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coop {
    UnsafeNone,
    SameOriginAllowPopups,
    SameOrigin,
    NoopenerAllowPopups,
}

/// Parses a `Cross-Origin-Opener-Policy` header value (a structured-field token; parameters ignored).
pub fn parse_coop(value: Option<&str>) -> Coop {
    let v = value.unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match v.as_str() {
        "same-origin" => Coop::SameOrigin,
        "same-origin-allow-popups" => Coop::SameOriginAllowPopups,
        "noopener-allow-popups" => Coop::NoopenerAllowPopups,
        _ => Coop::UnsafeNone,
    }
}

/// HTML §7.1.3.2 "check if COOP values require a browsing context group switch": `true` severs the
/// opener (the new document gets a fresh browsing context group; `window.opener` is null).
pub fn coop_requires_switch(is_initial_about_blank: bool, active_origin: &Origin, active: Coop, response_origin: &Origin, response: Coop) -> bool {
    if active == Coop::UnsafeNone && response == Coop::UnsafeNone {
        return false;
    }
    if active == Coop::SameOriginAllowPopups && response == Coop::UnsafeNone && is_initial_about_blank {
        return false;
    }
    if response == Coop::NoopenerAllowPopups {
        return true;
    }
    if active == response && matches!(active, Coop::SameOrigin | Coop::SameOriginAllowPopups) && active_origin.same_origin(response_origin) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        let a = Origin::of("https://Example.com:443/a?b#c");
        assert_eq!(a.serialize(), "https://example.com");
        assert_eq!(Origin::of("http://example.com:8080/").serialize(), "http://example.com:8080");
        assert!(a.same_origin(&Origin::of("https://example.com/zzz")));
        assert!(!a.same_origin(&Origin::of("http://example.com/")));
        assert!(Origin::of("data:text/html,x").is_opaque());
        assert_ne!(Origin::of("data:,x"), Origin::of("data:,x"));
        assert_eq!(Origin::of("blob:https://a.example/uuid").serialize(), "https://a.example");
        assert!(Origin::of_document("https://a.example/", true).is_opaque());
    }

    #[test]
    fn sites() {
        assert!(Origin::of("https://a.example.com").same_site(&Origin::of("https://b.example.com")));
        assert!(!Origin::of("http://a.example.com").same_site(&Origin::of("https://b.example.com")));
        assert!(!Origin::of("https://a.co.uk").same_site(&Origin::of("https://b.co.uk")));
        assert_eq!(site_of("https://x.y.example.co.uk/"), "https://example.co.uk");
        assert!(is_potentially_trustworthy_url("http://localhost:8000/"));
        assert!(!is_potentially_trustworthy_url("http://example.com/"));
    }

    #[test]
    fn sec_fetch_and_coop() {
        let c = Origin::of("https://a.example");
        let s = FetchSite::None.step(&c, &Origin::of("https://a.example/x"));
        assert_eq!(s, FetchSite::SameOrigin);
        assert_eq!(s.step(&c, &Origin::of("https://evil.test")), FetchSite::CrossSite);
        assert!(sec_fetch_headers("http://example.com/", s, "cors", "", false).is_empty());
        assert!(!coop_requires_switch(false, &c, Coop::UnsafeNone, &c, Coop::UnsafeNone));
        assert!(coop_requires_switch(false, &c, Coop::UnsafeNone, &c, Coop::SameOrigin));
        assert!(!coop_requires_switch(false, &c, Coop::SameOrigin, &Origin::of("https://a.example/2"), Coop::SameOrigin));
        assert!(document_domain_set("https://a.example.com/", false, "example.com").is_err());
    }
}
