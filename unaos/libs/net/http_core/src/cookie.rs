//! HTTP State Management (RFC 6265 §5, the user-agent algorithms): the Set-Cookie parser (§5.2), the cookie
//! date parser (§5.1.1), domain and path matching (§5.1.3, §5.1.4), the storage model (§5.3) and the Cookie
//! header (§5.4), plus `SameSite` from RFC 6265bis §5.6.7 parsed and kept. The jar is plain data — the caller
//! owns time (`now`, Unix seconds) and persistence (every field of [`Cookie`] is public).
//!
//! OWED: the public suffix list (§5.3 step 5) — only single-label suffixes (`Domain=org`) are refused today,
//! so `Domain=co.uk` from `a.co.uk` is accepted; and SameSite enforcement (the caller knows
//! the site-for-cookies, this module only records the attribute).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::url::{Host, Url};

/// Expiry for a session cookie / a cookie told to expire now.
pub const LATEST: i64 = i64::MAX;
pub const EARLIEST: i64 = i64::MIN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

/// §5.3's cookie record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Unix seconds; [`LATEST`] for a session cookie.
    pub expiry: i64,
    pub domain: String,
    pub path: String,
    pub creation: u64,
    pub last_access: i64,
    pub persistent: bool,
    pub host_only: bool,
    pub secure_only: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
}

/// What §5.2 extracted from one Set-Cookie value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedSetCookie {
    pub name: String,
    pub value: String,
    pub expires: Option<i64>,
    /// Max-Age as delta seconds (the last valid one).
    pub max_age: Option<i64>,
    pub domain: Option<String>,
    pub path: Option<String>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
}

fn trim_wsp(s: &str) -> &str {
    s.trim_matches(|c| c == ' ' || c == '\t')
}

/// RFC 6265 §5.2. `None`: the user agent ignores the whole header.
pub fn parse_set_cookie(s: &str) -> Option<ParsedSetCookie> {
    let (nv, attrs) = match s.find(';') {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let eq = nv.find('=')?;
    let name = trim_wsp(&nv[..eq]);
    let value = trim_wsp(&nv[eq + 1..]);
    if name.is_empty() {
        return None;
    }
    let mut c = ParsedSetCookie { name: name.to_string(), value: value.to_string(), ..Default::default() };
    for av in attrs.split(';').skip(1) {
        let (an, aval) = match av.find('=') {
            Some(i) => (trim_wsp(&av[..i]), trim_wsp(&av[i + 1..])),
            None => (trim_wsp(av), ""),
        };
        if an.eq_ignore_ascii_case("expires") {
            if let Some(t) = parse_cookie_date(aval) {
                c.expires = Some(t);
            }
        } else if an.eq_ignore_ascii_case("max-age") {
            let b = aval.as_bytes();
            if b.is_empty() || !(b[0].is_ascii_digit() || b[0] == b'-') {
                continue;
            }
            if !b[1..].iter().all(u8::is_ascii_digit) || (b[0] == b'-' && b.len() == 1) {
                continue;
            }
            let neg = b[0] == b'-';
            let digits = if neg { &aval[1..] } else { aval };
            let mut v: i64 = 0;
            for d in digits.bytes() {
                v = v.saturating_mul(10).saturating_add((d - b'0') as i64);
            }
            c.max_age = Some(if neg { -v } else { v });
        } else if an.eq_ignore_ascii_case("domain") {
            if aval.is_empty() {
                continue;
            }
            let d = aval.strip_prefix('.').unwrap_or(aval);
            c.domain = Some(d.to_ascii_lowercase());
        } else if an.eq_ignore_ascii_case("path") {
            // An empty or relative Path means "the default-path" (kept as "" and resolved at storage time).
            c.path = Some(if aval.starts_with('/') { aval.to_string() } else { String::new() });
        } else if an.eq_ignore_ascii_case("secure") {
            c.secure = true;
        } else if an.eq_ignore_ascii_case("httponly") {
            c.http_only = true;
        } else if an.eq_ignore_ascii_case("samesite") {
            c.same_site = if aval.eq_ignore_ascii_case("strict") {
                Some(SameSite::Strict)
            } else if aval.eq_ignore_ascii_case("lax") {
                Some(SameSite::Lax)
            } else if aval.eq_ignore_ascii_case("none") {
                Some(SameSite::None)
            } else {
                None
            };
        }
    }
    Some(c)
}

// ---------------------------------------------------------------- §5.1.1 dates

fn is_delimiter(b: u8) -> bool {
    b == 0x09 || (0x20..=0x2F).contains(&b) || (0x3B..=0x40).contains(&b) || (0x5B..=0x60).contains(&b) || (0x7B..=0x7E).contains(&b)
}

/// `1*2DIGIT` then (end or a non-digit): the digits' value and how many there were.
fn digits(t: &[u8], min: usize, max: usize) -> Option<(u32, usize)> {
    let n = t.iter().take_while(|b| b.is_ascii_digit()).count();
    if n < min || n > max {
        return None;
    }
    let mut v = 0u32;
    for &d in &t[..n] {
        v = v * 10 + (d - b'0') as u32;
    }
    Some((v, n))
}

fn time_token(t: &[u8]) -> Option<(u32, u32, u32)> {
    let (h, n1) = digits(t, 1, 2)?;
    if t.get(n1) != Some(&b':') {
        return None;
    }
    let r = &t[n1 + 1..];
    let (m, n2) = digits(r, 1, 2)?;
    if r.get(n2) != Some(&b':') {
        return None;
    }
    let r = &r[n2 + 1..];
    let (s, _) = digits(r, 1, 2)?;
    Some((h, m, s))
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// RFC 6265 §5.1.1: Unix seconds, or `None` when the date is not a cookie-date.
pub fn parse_cookie_date(s: &str) -> Option<i64> {
    let (mut time, mut day, mut month, mut year) = (None, None, None, None);
    for tok in s.as_bytes().split(|&b| is_delimiter(b)).filter(|t| !t.is_empty()) {
        if time.is_none() {
            if let Some(t) = time_token(tok) {
                time = Some(t);
                continue;
            }
        }
        if day.is_none() {
            if let Some((d, _)) = digits(tok, 1, 2) {
                day = Some(d);
                continue;
            }
        }
        if month.is_none() && tok.len() >= 3 {
            const M: [&[u8; 3]; 12] = [b"jan", b"feb", b"mar", b"apr", b"may", b"jun", b"jul", b"aug", b"sep", b"oct", b"nov", b"dec"];
            let p = [tok[0].to_ascii_lowercase(), tok[1].to_ascii_lowercase(), tok[2].to_ascii_lowercase()];
            if let Some(i) = M.iter().position(|m| **m == p) {
                month = Some(i as u32 + 1);
                continue;
            }
        }
        if year.is_none() {
            if let Some((y, _)) = digits(tok, 2, 4) {
                year = Some(y);
                continue;
            }
        }
    }
    let (h, mi, se) = time?;
    let (d, mo, mut y) = (day?, month?, year? as i64);
    if (70..=99).contains(&y) {
        y += 1900;
    } else if (0..=69).contains(&y) {
        y += 2000;
    }
    if d < 1 || d > 31 || y < 1601 || h > 23 || mi > 59 || se > 59 || d > days_in_month(y, mo) {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86400 + h as i64 * 3600 + mi as i64 * 60 + se as i64)
}

// ---------------------------------------------------------------- §5.1.3 / §5.1.4 matching

fn is_ip(host: &str) -> bool {
    host.starts_with('[') || host.parse::<core::net::Ipv4Addr>().is_ok()
}

/// §5.1.3 domain-match.
pub fn domain_match(string: &str, domain: &str) -> bool {
    if string == domain {
        return true;
    }
    string.len() > domain.len()
        && string.ends_with(domain)
        && string.as_bytes()[string.len() - domain.len() - 1] == b'.'
        && !is_ip(string)
}

/// §5.1.4 default-path.
pub fn default_path(uri_path: &str) -> String {
    if !uri_path.starts_with('/') {
        return "/".into();
    }
    match uri_path.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => uri_path[..i].to_string(),
    }
}

/// §5.1.4 path-match.
pub fn path_match(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    request_path.starts_with(cookie_path)
        && (cookie_path.ends_with('/') || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/'))
}

fn request_host(url: &Url) -> Option<String> {
    match url.host()? {
        Host::Empty => None,
        h => Some(h.to_string().to_ascii_lowercase()),
    }
}

fn is_secure_scheme(url: &Url) -> bool {
    matches!(url.scheme(), "https" | "wss")
}

// ---------------------------------------------------------------- §5.3 / §5.4 the jar

/// A cookie store. `http` on each call says whether the caller is an HTTP API (Set-Cookie / Cookie) or a
/// non-HTTP one (`document.cookie`), which decides HttpOnly.
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
    seq: u64,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cookies(&self) -> &[Cookie] {
        &self.cookies
    }

    /// Put back a persisted cookie as-is.
    pub fn restore(&mut self, c: Cookie) {
        self.seq = self.seq.max(c.creation + 1);
        self.cookies.retain(|o| !(o.name == c.name && o.domain == c.domain && o.path == c.path));
        self.cookies.push(c);
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    /// §5.3 for one Set-Cookie value received from `url` at `now`. Returns whether a cookie was stored.
    pub fn set_cookie(&mut self, url: &Url, header: &str, now: i64, http: bool) -> bool {
        let Some(p) = parse_set_cookie(header) else { return false };
        let Some(host) = request_host(url) else { return false };
        let (persistent, expiry) = match (p.max_age, p.expires) {
            (Some(ma), _) => (true, if ma <= 0 { EARLIEST } else { now.saturating_add(ma) }),
            (None, Some(e)) => (true, e),
            (None, None) => (false, LATEST),
        };
        let (host_only, domain) = match p.domain.as_deref().filter(|d| !d.is_empty()) {
            Some(d) => {
                if !domain_match(&host, d) {
                    return false;
                }
                // §5.3 step 5, the part that needs no list: a single-label Domain (a TLD such as `org`) is a
                // public suffix — refused unless it IS the request host. The public suffix list is owed.
                if !d.contains('.') && d != host {
                    return false;
                }
                (false, d.to_string())
            }
            None => (true, host.clone()),
        };
        let path = match p.path.as_deref() {
            Some(pp) if !pp.is_empty() => pp.to_string(),
            _ => default_path(&url.pathname()),
        };
        if !http && p.http_only {
            return false;
        }
        let mut creation = self.seq;
        self.seq += 1;
        if let Some(i) = self.cookies.iter().position(|o| o.name == p.name && o.domain == domain && o.path == path) {
            if self.cookies[i].http_only && !http {
                return false;
            }
            creation = self.cookies[i].creation;
            self.cookies.remove(i);
        }
        let c = Cookie {
            name: p.name,
            value: p.value,
            expiry,
            domain,
            path,
            creation,
            last_access: now,
            persistent,
            host_only,
            secure_only: p.secure,
            http_only: p.http_only,
            same_site: p.same_site,
        };
        let live = c.expiry > now;
        if live {
            self.cookies.push(c);
        }
        self.cookies.retain(|o| o.expiry > now);
        live
    }

    /// §5.4: the Cookie header value for a request to `url` at `now`, or `None` when no cookie applies.
    pub fn cookie_header(&mut self, url: &Url, now: i64, http: bool) -> Option<String> {
        self.cookies.retain(|o| o.expiry > now);
        let host = request_host(url)?;
        let path = url.pathname();
        let path = if path.is_empty() { "/".to_string() } else { path };
        let secure = is_secure_scheme(url);
        let mut hits: Vec<usize> = (0..self.cookies.len())
            .filter(|&i| {
                let c = &self.cookies[i];
                let dom = if c.host_only { host == c.domain } else { domain_match(&host, &c.domain) };
                dom && path_match(&path, &c.path) && (!c.secure_only || secure) && (!c.http_only || http)
            })
            .collect();
        if hits.is_empty() {
            return None;
        }
        hits.sort_by(|&a, &b| {
            let (ca, cb) = (&self.cookies[a], &self.cookies[b]);
            cb.path.len().cmp(&ca.path.len()).then(ca.creation.cmp(&cb.creation))
        });
        let mut out = String::new();
        for (n, &i) in hits.iter().enumerate() {
            self.cookies[i].last_access = now;
            if n > 0 {
                out.push_str("; ");
            }
            out.push_str(&self.cookies[i].name);
            out.push('=');
            out.push_str(&self.cookies[i].value);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        // RFC 6265 §4.1.1's example and the three historic formats.
        assert_eq!(parse_cookie_date("Wed, 09 Jun 2021 10:18:14 GMT"), Some(1623233894));
        assert_eq!(parse_cookie_date("Wednesday, 09-Jun-21 10:18:14 GMT"), Some(1623233894));
        assert_eq!(parse_cookie_date("Wed Jun  9 10:18:14 2021"), Some(1623233894));
        assert_eq!(parse_cookie_date("Fri, 07 Aug 2007 08:04:19 GMT"), Some(1186473859));
        assert_eq!(parse_cookie_date("Feb 30 2021 00:00:00"), None);
        assert_eq!(parse_cookie_date("Jan 1 1600 00:00:00"), None);
    }

    #[test]
    fn paths() {
        assert_eq!(default_path("/a/b/c"), "/a/b");
        assert_eq!(default_path("/a"), "/");
        assert_eq!(default_path(""), "/");
        assert!(path_match("/a/b", "/a"));
        assert!(!path_match("/ab", "/a"));
        assert!(path_match("/a/", "/a/"));
    }
}
