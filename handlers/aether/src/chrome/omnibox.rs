//! The address bar's decision (std-free): is what the person typed a URL, or a search?
//!
//! The rule is the one Chromium's omnibox and Firefox's URL bar converge on for a line with no
//! suggestion provider behind it:
//!
//! - whitespace anywhere → a search ("rust borrow checker");
//! - an explicit scheme (`https://…`, `about:blank`, `data:…`, `file:…`, `mailto:…`) → a URL;
//! - a host the resolver could reach — `localhost`, an IPv4 or bracketed IPv6 literal, or a dotted name
//!   whose last label is alphabetic and at least two letters (`una.os`, `example.com/path?q`), with an
//!   optional `:port` and `user@` → a URL (the scheme is left to `net::normalize_url`, which tries
//!   https then http);
//! - anything else (`aether`, `3.14`, `why?`) → a search.
//!
//! A search is the user's template (Principia `aether.search`) with `%s` (or OpenSearch's
//! `{searchTerms}`) replaced by the query, `application/x-www-form-urlencoded` (HTML §4.10.21.8).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The `aether.search` default (the schema row in `prefs_core::schema` carries the same string):
/// DuckDuckGo's script-free HTML endpoint, which Aether renders without a single script.
pub const DEFAULT_SEARCH: &str = "https://html.duckduckgo.com/html/?q=%s";

/// What a line of address-bar text resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// Navigate to this URL (as typed; a scheme-less host is completed by the fetcher).
    Url(String),
    /// Navigate to this search-results URL.
    Search(String),
}

impl Resolved {
    /// The URL to navigate to, either way.
    pub fn url(&self) -> &str {
        match self {
            Resolved::Url(u) | Resolved::Search(u) => u,
        }
    }
}

/// Resolve `input` against the search `template` (empty or placeholder-less → [`DEFAULT_SEARCH`]).
/// `None` for an empty line.
pub fn resolve(input: &str, template: &str) -> Option<Resolved> {
    let t = input.trim();
    if t.is_empty() {
        return None;
    }
    Some(if looks_like_url(t) { Resolved::Url(t.to_string()) } else { Resolved::Search(search_url(template, t)) })
}

/// Whether `t` (trimmed) reads as an address rather than a query.
pub fn looks_like_url(t: &str) -> bool {
    if t.is_empty() || t.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    if let Some(i) = t.find(':') {
        let (scheme, rest) = (&t[..i], &t[i + 1..]);
        let is_scheme = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if is_scheme {
            if rest.starts_with("//") {
                return true;
            }
            let lower = scheme.to_ascii_lowercase();
            if matches!(lower.as_str(), "about" | "data" | "file" | "mailto" | "blob" | "view-source") {
                return true;
            }
            // `host:port[/…]` — a port is digits only; anything else after the colon is not an address.
            let port: &str = rest.split(['/', '?', '#']).next().unwrap_or("");
            if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
        }
    }
    let authority = t.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    host_like(strip_port(host_port))
}

fn strip_port(h: &str) -> &str {
    if h.starts_with('[') {
        return match h.find(']') {
            Some(e) => &h[..=e],
            None => h,
        };
    }
    match h.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => h,
    }
}

fn is_ipv4(h: &str) -> bool {
    let parts: Vec<&str> = h.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n <= 255))
}

fn host_like(h: &str) -> bool {
    if h.eq_ignore_ascii_case("localhost") || is_ipv4(h) {
        return true;
    }
    if h.len() > 2 && h.starts_with('[') && h.ends_with(']') {
        return h[1..h.len() - 1].chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
    }
    let h = h.strip_suffix('.').unwrap_or(h);
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let ok_label = |l: &&str| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_alphanumeric() || c == '-');
    let tld = labels[labels.len() - 1];
    labels.iter().all(ok_label) && tld.chars().count() >= 2 && tld.chars().all(|c| c.is_alphabetic() || c == '-')
}

/// The template with the query substituted: every `%s` / `{searchTerms}` becomes the urlencoded query.
pub fn search_url(template: &str, query: &str) -> String {
    let t = if template.contains("%s") || template.contains("{searchTerms}") { template } else { DEFAULT_SEARCH };
    let q = form_urlencode(query);
    t.replace("{searchTerms}", &q).replace("%s", &q)
}

/// `application/x-www-form-urlencoded` byte serializer (URL Standard §5.2): ASCII alphanumerics and
/// `*-._` pass, space becomes `+`, every other byte of the UTF-8 is `%XX`.
pub fn form_urlencode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 15) as usize] as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_searches_split_where_a_browser_splits_them() {
        for u in ["https://una.os", "una.os", "example.com/a?b#c", "localhost:8080/x", "127.0.0.1", "[::1]:3000", "about:blank", "user@host.org", "data:text/html,hi"] {
            assert!(looks_like_url(u), "{u} is an address");
        }
        for q in ["rust borrow checker", "aether", "3.14", "why?", "foo:bar", "c++"] {
            assert!(!looks_like_url(q), "{q} is a search");
        }
    }

    #[test]
    fn a_search_fills_the_template_urlencoded() {
        assert_eq!(
            resolve("  una os & more ", DEFAULT_SEARCH),
            Some(Resolved::Search("https://html.duckduckgo.com/html/?q=una+os+%26+more".into()))
        );
        assert_eq!(search_url("https://s.test/?q={searchTerms}", "é"), "https://s.test/?q=%C3%A9");
        assert_eq!(search_url("", "x"), "https://html.duckduckgo.com/html/?q=x");
        assert_eq!(resolve("   ", DEFAULT_SEARCH), None);
    }
}
