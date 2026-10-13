//! AETHERCACHE (LEDGER SR88) — HTTP caching per RFC 9111, as a PRIVATE (single-user) cache, `no_std` + `alloc`.
//!
//! The logic only: what may be stored (§3), how long a stored response stays fresh (§4.2.1 explicit
//! `max-age` / `Expires`, §4.2.2 heuristic), how old it is (§4.2.3), when it may be served stale
//! (`max-stale`, RFC 5861 `stale-while-revalidate`, never past `must-revalidate`), how it is validated
//! (§4.3.1 `If-None-Match` from `ETag`, `If-Modified-Since` from `Last-Modified`) and freshened by a 304
//! (§4.3.4, §3.2), the secondary key from `Vary` (§4.1), invalidation by unsafe methods (§4.4), and the Fetch
//! Standard's request cache modes (`default`, `no-store`, `reload`, `no-cache`, `force-cache`,
//! `only-if-cached`, Fetch §4.5 "HTTP-network-or-cache fetch"). The primary key is the target URI without
//! its fragment, PARTITIONED by the top-level site that issued the request (Fetch's network partition key),
//! for `GET` only. A [`MemoryCache`] (byte-bounded, LRU) is the in-memory tier; entries [`Entry::encode`] to
//! bytes for a disk tier the host keeps (gneiss_pal::api::http).
//!
//! Time is the caller's: every function takes `now` in Unix seconds, so this module needs no clock.
//! Private cache rules apply throughout: `private` and `Authorization` do not prevent storage, `s-maxage`
//! and `proxy-revalidate` are ignored (§5.2.2.10, §5.2.2.8). `Set-Cookie` is never stored (a cached
//! response must not replay a cookie the jar already took).

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::headers::Headers;
use crate::url::{Host, Url};

// ---------------------------------------------------------------- Fetch cache modes

/// The Fetch Standard's `RequestCache` (a request's cache mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheMode {
    /// Fresh → served; stale → validated (or served while revalidating inside `stale-while-revalidate`).
    #[default]
    Default,
    /// The cache is neither read nor written.
    NoStore,
    /// The cache is not read; the network response is stored.
    Reload,
    /// Any stored response is validated before use.
    NoCache,
    /// Any stored response is used, however stale; the network only on a miss.
    ForceCache,
    /// Any stored response is used; a miss is a network error (no network at all).
    OnlyIfCached,
}

impl CacheMode {
    /// The IDL enum value (`"default"`, `"no-store"`, …).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "default" => CacheMode::Default,
            "no-store" => CacheMode::NoStore,
            "reload" => CacheMode::Reload,
            "no-cache" => CacheMode::NoCache,
            "force-cache" => CacheMode::ForceCache,
            "only-if-cached" => CacheMode::OnlyIfCached,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CacheMode::Default => "default",
            CacheMode::NoStore => "no-store",
            CacheMode::Reload => "reload",
            CacheMode::NoCache => "no-cache",
            CacheMode::ForceCache => "force-cache",
            CacheMode::OnlyIfCached => "only-if-cached",
        }
    }

    /// Does this mode read the cache?
    pub fn reads(self) -> bool {
        !matches!(self, CacheMode::NoStore | CacheMode::Reload)
    }

    /// Does this mode write the cache?
    pub fn writes(self) -> bool {
        !matches!(self, CacheMode::NoStore)
    }
}

/// Request fields that make a request conditional or partial: Fetch turns `default` into `no-store` for
/// them (the page is doing its own validation).
const CONDITIONAL_FIELDS: [&str; 6] = ["if-modified-since", "if-none-match", "if-unmodified-since", "if-match", "if-range", "range"];

/// Fetch §4.5 step 8.x: the mode actually used for `method` with these request headers. Only `GET` is cached;
/// any other method is `no-store` (an unsafe one still invalidates, see [`invalidation_targets`]).
pub fn effective_mode(mode: CacheMode, method: &str, req: &Headers) -> CacheMode {
    if !method.eq_ignore_ascii_case("GET") {
        return CacheMode::NoStore;
    }
    if mode == CacheMode::Default && CONDITIONAL_FIELDS.iter().any(|f| req.contains(f)) {
        return CacheMode::NoStore;
    }
    mode
}

/// Fetch §4.5: the request header fields a cache mode adds before the network sees the request —
/// `no-cache` adds `Cache-Control: max-age=0`; `no-store` and `reload` add `Pragma: no-cache` and
/// `Cache-Control: no-cache` (so caches on the path do not answer either). Fields the caller set win.
pub fn apply_mode_headers(mode: CacheMode, req: &mut Headers) {
    match mode {
        CacheMode::NoCache => {
            if !req.contains("cache-control") {
                let _ = req.set("Cache-Control", "max-age=0");
            }
        }
        CacheMode::NoStore | CacheMode::Reload => {
            if !req.contains("pragma") {
                let _ = req.set("Pragma", "no-cache");
            }
            if !req.contains("cache-control") {
                let _ = req.set("Cache-Control", "no-cache");
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- Cache-Control (§5.2)

/// The directives of `Cache-Control` (request and response share one shape; each side reads its own).
/// Repeated directives: the first occurrence wins (§4.2.1). A malformed delta-seconds reads as 0 (stale).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheControl {
    pub no_store: bool,
    pub no_cache: bool,
    pub private: bool,
    pub public: bool,
    pub must_revalidate: bool,
    pub immutable: bool,
    pub no_transform: bool,
    pub only_if_cached: bool,
    pub max_age: Option<u64>,
    pub s_maxage: Option<u64>,
    /// `max-stale` (request): `Some(None)` = any staleness accepted.
    pub max_stale: Option<Option<u64>>,
    pub min_fresh: Option<u64>,
    /// RFC 5861.
    pub stale_while_revalidate: Option<u64>,
    pub stale_if_error: Option<u64>,
}

/// §1.2.2 delta-seconds: digits only; larger than 2^31 is 2^31.
fn delta_seconds(v: &str) -> u64 {
    let v = v.trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    v.parse::<u64>().unwrap_or(u64::MAX).min(1 << 31)
}

/// Split a list field on commas outside quoted-strings.
fn split_list(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0usize, false, false);
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out.retain(|p| !p.is_empty());
    out
}

/// Parse every `Cache-Control` field line of `h` (combined in order).
pub fn parse_cache_control(h: &Headers) -> CacheControl {
    let mut cc = CacheControl::default();
    let mut seen: Vec<String> = Vec::new();
    for line in h.get_all("cache-control") {
        for d in split_list(line) {
            let (name, value) = match d.split_once('=') {
                Some((n, v)) => (n.trim().to_ascii_lowercase(), Some(v.trim().trim_matches('"'))),
                None => (d.trim().to_ascii_lowercase(), None),
            };
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            match name.as_str() {
                "no-store" => cc.no_store = true,
                // `no-cache="field"` (qualified) is treated as unqualified: stricter, always correct.
                "no-cache" => cc.no_cache = true,
                "private" => cc.private = true,
                "public" => cc.public = true,
                "must-revalidate" => cc.must_revalidate = true,
                "immutable" => cc.immutable = true,
                "no-transform" => cc.no_transform = true,
                "only-if-cached" => cc.only_if_cached = true,
                "max-age" => cc.max_age = Some(value.map(delta_seconds).unwrap_or(0)),
                "s-maxage" => cc.s_maxage = Some(value.map(delta_seconds).unwrap_or(0)),
                "max-stale" => cc.max_stale = Some(value.map(delta_seconds)),
                "min-fresh" => cc.min_fresh = Some(value.map(delta_seconds).unwrap_or(0)),
                "stale-while-revalidate" => cc.stale_while_revalidate = Some(value.map(delta_seconds).unwrap_or(0)),
                "stale-if-error" => cc.stale_if_error = Some(value.map(delta_seconds).unwrap_or(0)),
                _ => {}
            }
        }
    }
    cc
}

/// RFC 9111 §5.4: `Pragma: no-cache` on a request without `Cache-Control` means `Cache-Control: no-cache`.
fn request_no_cache(req: &Headers, cc: &CacheControl) -> bool {
    cc.no_cache || (!req.contains("cache-control") && req.has_token("pragma", "no-cache"))
}

// ---------------------------------------------------------------- HTTP-date (RFC 9110 §5.6.7)

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// Days from 1970-01-01 to the civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// An HTTP-date in any of the three formats a recipient must accept — IMF-fixdate
/// (`Sun, 06 Nov 1994 08:49:37 GMT`), obsolete RFC 850 (`Sunday, 06-Nov-94 08:49:37 GMT`) and asctime
/// (`Sun Nov  6 08:49:37 1994`) — as Unix seconds. A two-digit year below 70 is 20xx.
pub fn parse_http_date(s: &str) -> Option<i64> {
    let toks: Vec<&str> = s.split(|c: char| c == ' ' || c == ',' || c == '-' || c == '\t').filter(|t| !t.is_empty()).collect();
    let mut month = None;
    let mut time = None;
    let mut nums: Vec<&str> = Vec::new();
    for t in &toks {
        let lt = t.to_ascii_lowercase();
        if let Some(i) = MONTHS.iter().position(|m| lt == *m) {
            month = Some(i as u32 + 1);
        } else if t.contains(':') {
            time = Some(*t);
        } else if t.bytes().all(|b| b.is_ascii_digit()) {
            nums.push(t);
        }
    }
    let (month, time) = (month?, time?);
    if nums.len() != 2 {
        return None;
    }
    let day: u32 = nums[0].parse().ok()?;
    let mut year: i64 = nums[1].parse().ok()?;
    if nums[1].len() == 2 {
        year += if year < 70 { 2000 } else { 1900 };
    }
    let mut hms = time.split(':');
    let h: i64 = hms.next()?.parse().ok()?;
    let mi: i64 = hms.next()?.parse().ok()?;
    let se: i64 = hms.next()?.parse().ok()?;
    if hms.next().is_some() || !(1..=31).contains(&day) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + h * 3600 + mi * 60 + se)
}

// ---------------------------------------------------------------- keys

/// The site a request is partitioned by (HTML §7.1.2 "obtain a site", Fetch's network partition key):
/// `scheme://registrable-domain` of the top-level document — NETDEBT (SR97): the registrable domain from the
/// Public Suffix List (`psl_core`), so `a.example.co.uk` and `b.example.co.uk` share a partition and
/// `alice.github.io` and `bob.github.io` do not. A host without one (an IP address, `localhost`, a host that is
/// itself a public suffix) is its own site.
pub fn site_of(url: &Url) -> String {
    let host = match url.host() {
        Some(Host::Domain(d)) => psl_core::site_host(d).to_string(),
        Some(h) => h.to_string(),
        None => String::new(),
    };
    format!("{}://{}", url.scheme(), host)
}

/// The primary cache key (§2): the top-level site partition and the target URI without its fragment. Only
/// `GET` is stored, so the method is implied.
pub fn cache_key(partition: &str, url: &Url) -> String {
    format!("{partition} {}", url.serialize(true))
}

// ---------------------------------------------------------------- the stored response

/// One stored response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    /// The request URL (fragment-less), for diagnostics and §4.4 invalidation.
    pub url: String,
    pub status: u16,
    pub reason: String,
    /// The response fields as delivered (content codings already decoded), `Set-Cookie` removed.
    pub headers: Headers,
    /// §4.1: each request field `Vary` names, with the value the original request carried (`None` = absent).
    /// A `Vary: *` response is never stored.
    pub vary: Vec<(String, Option<String>)>,
    pub body: Vec<u8>,
    /// When the request that produced it was sent / its head arrived (Unix seconds).
    pub request_time: i64,
    pub response_time: i64,
}

/// Status codes that are heuristically cacheable (RFC 9110 §15.1).
pub fn heuristically_cacheable(status: u16) -> bool {
    matches!(status, 200 | 203 | 204 | 206 | 300 | 301 | 308 | 404 | 405 | 410 | 414 | 501)
}

/// Fields that are not stored (§3.1: hop-by-hop and connection-specific) and fields a 304 does not replace
/// (§3.2: Content-Length; plus the codings the stored body no longer carries).
const NOT_STORED: [&str; 9] = ["connection", "keep-alive", "proxy-connection", "te", "transfer-encoding", "upgrade", "proxy-authenticate", "proxy-authorization", "set-cookie"];
const NOT_UPDATED: [&str; 3] = ["content-length", "content-encoding", "content-range"];

/// §3: may this response to `GET` be stored by a private cache?
pub fn storable(method: &str, req: &Headers, status: u16, resp: &Headers) -> bool {
    if !method.eq_ignore_ascii_case("GET") || status < 200 || status == 206 || status == 304 {
        return false;
    }
    let rq = parse_cache_control(req);
    let rs = parse_cache_control(resp);
    if rq.no_store || rs.no_store {
        return false;
    }
    if resp.get_all("vary").any(|v| split_list(v).iter().any(|f| *f == "*")) {
        return false; // §4.1: never matches; storing it is waste
    }
    // A private cache may store `private` responses and responses to requests with Authorization.
    rs.public || rs.private || rs.max_age.is_some() || resp.contains("expires") || heuristically_cacheable(status)
}

/// The request-field snapshot `Vary` asks for (§4.1).
pub fn vary_snapshot(resp: &Headers, req: &Headers) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for line in resp.get_all("vary") {
        for f in split_list(line) {
            let name = f.to_ascii_lowercase();
            if !out.iter().any(|(n, _)| *n == name) {
                out.push((name.clone(), req.get_combined(&name).map(|v| normalize_value(&v))));
            }
        }
    }
    out
}

/// §4.1: values compare after whitespace normalization.
fn normalize_value(v: &str) -> String {
    let mut out = String::new();
    for (i, part) in v.split(',').map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ")).enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&part);
    }
    out
}

impl Entry {
    /// Build the entry for a storable response (`headers` as delivered). `Set-Cookie` and hop-by-hop fields
    /// are dropped.
    pub fn new(key: String, url: &Url, req: &Headers, status: u16, reason: &str, headers: &Headers, request_time: i64, response_time: i64) -> Self {
        let mut stored = Headers::new();
        for (k, v) in headers.iter() {
            if !NOT_STORED.iter().any(|n| k.eq_ignore_ascii_case(n)) {
                let _ = stored.append(k, v);
            }
        }
        Entry {
            key,
            url: url.serialize(true),
            status,
            reason: reason.to_string(),
            vary: vary_snapshot(headers, req),
            headers: stored,
            body: Vec::new(),
            request_time,
            response_time,
        }
    }

    /// §4.1: does `req` select this stored response?
    pub fn vary_matches(&self, req: &Headers) -> bool {
        self.vary.iter().all(|(name, stored)| {
            name != "*" && req.get_combined(name).map(|v| normalize_value(&v)).as_deref() == stored.as_deref()
        })
    }

    fn date(&self) -> i64 {
        self.headers.get("date").and_then(parse_http_date).unwrap_or(self.response_time)
    }

    /// §4.2.1 freshness lifetime for a private cache: `max-age`, else `Expires − Date` (an invalid Expires
    /// is already expired), else §4.2.2's heuristic — 10% of the time since `Last-Modified`, capped at a
    /// week — for heuristically cacheable statuses (or `public`).
    pub fn freshness_lifetime(&self) -> u64 {
        let cc = parse_cache_control(&self.headers);
        if let Some(ma) = cc.max_age {
            return ma;
        }
        if let Some(exp) = self.headers.get("expires") {
            return match parse_http_date(exp) {
                Some(t) => (t - self.date()).max(0) as u64,
                None => 0,
            };
        }
        if heuristically_cacheable(self.status) || cc.public {
            if let Some(lm) = self.headers.get("last-modified").and_then(parse_http_date) {
                let since = (self.date() - lm).max(0) as u64;
                return (since / 10).min(7 * 86_400);
            }
        }
        0
    }

    /// §4.2.3 current age at `now`.
    pub fn current_age(&self, now: i64) -> u64 {
        let age_value = self.headers.get("age").map(delta_seconds).unwrap_or(0) as i64;
        let apparent_age = (self.response_time - self.date()).max(0);
        let response_delay = (self.response_time - self.request_time).max(0);
        let corrected_initial_age = apparent_age.max(age_value + response_delay);
        let resident_time = (now - self.response_time).max(0);
        (corrected_initial_age + resident_time) as u64
    }

    /// Does it carry a validator a conditional request can use (§4.3.1)?
    pub fn has_validator(&self) -> bool {
        self.headers.contains("etag") || self.headers.contains("last-modified")
    }

    /// §4.3.1: add `If-None-Match` (from `ETag`) and `If-Modified-Since` (from `Last-Modified`) to `req`.
    /// Returns whether a validator was added.
    pub fn add_conditionals(&self, req: &mut Headers) -> bool {
        let mut added = false;
        if let Some(etag) = self.headers.get("etag") {
            added |= req.set("If-None-Match", etag).is_ok();
        }
        if let Some(lm) = self.headers.get("last-modified") {
            added |= req.set("If-Modified-Since", lm).is_ok();
        }
        added
    }

    /// §4.3.4: does this 304's validator select this stored response? A strong or weak ETag must equal the
    /// stored one; without an ETag, Last-Modified must equal; with neither, the (single) stored response is it.
    pub fn validator_matches(&self, h304: &Headers) -> bool {
        let strip = |t: &str| t.trim().trim_start_matches("W/").to_string();
        if let Some(e) = h304.get("etag") {
            return self.headers.get("etag").map(strip) == Some(strip(e));
        }
        if let Some(lm) = h304.get("last-modified") {
            return self.headers.get("last-modified").map(|v| v.trim()) == Some(lm.trim());
        }
        true
    }

    /// §4.3.4 + §3.2: freshen with a 304 — every field the 304 carries replaces the stored one (except the
    /// framing and codings of the stored body), and the age clock restarts.
    pub fn freshen(&mut self, h304: &Headers, request_time: i64, response_time: i64) {
        let mut names: Vec<String> = Vec::new();
        for (k, _) in h304.iter() {
            let lk = k.to_ascii_lowercase();
            if NOT_STORED.contains(&lk.as_str()) || NOT_UPDATED.contains(&lk.as_str()) || names.contains(&lk) {
                continue;
            }
            names.push(lk);
        }
        for n in &names {
            self.headers.remove(n);
            for v in h304.get_all(n) {
                let _ = self.headers.append(n, v);
            }
        }
        if !h304.contains("age") {
            self.headers.remove("age");
        }
        self.request_time = request_time;
        self.response_time = response_time;
    }

    /// The fields to deliver when served at `now`: the stored ones with `Age` (§5.1) set.
    pub fn headers_at(&self, now: i64) -> Headers {
        let mut h = self.headers.clone();
        let _ = h.set("Age", &format!("{}", self.current_age(now)));
        h
    }

    /// Bytes held in memory (the LRU budget unit).
    pub fn weight(&self) -> usize {
        self.body.len() + self.key.len() + self.url.len() + self.headers.iter().map(|(k, v)| k.len() + v.len() + 4).sum::<usize>() + 64
    }
}

// ---------------------------------------------------------------- the decision

/// What a request does with what the cache holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Serve the stored response; no network.
    Serve,
    /// Serve the stored (stale) response now and revalidate it in the background (RFC 5861).
    ServeAndRevalidate,
    /// Send a conditional request built with [`Entry::add_conditionals`]; a 304 freshens and serves it.
    Validate,
    /// Go to the network (nothing usable is stored, or the mode says so).
    Network,
    /// `only-if-cached` and nothing stored: a network error.
    Fail,
}

/// §4.2 + §4.2.4 + RFC 5861 under the Fetch cache `mode` (already [`effective_mode`]): what to do with
/// `entry` for a request with these headers at `now`.
pub fn decide(mode: CacheMode, entry: Option<&Entry>, req: &Headers, now: i64) -> Decision {
    if !mode.reads() {
        return Decision::Network;
    }
    let entry = entry.filter(|e| e.vary_matches(req));
    let Some(e) = entry else {
        return if mode == CacheMode::OnlyIfCached { Decision::Fail } else { Decision::Network };
    };
    let revalidate = if e.has_validator() { Decision::Validate } else { Decision::Network };
    match mode {
        CacheMode::ForceCache | CacheMode::OnlyIfCached => return Decision::Serve,
        CacheMode::NoCache => return revalidate,
        _ => {}
    }
    let rq = parse_cache_control(req);
    let rs = parse_cache_control(&e.headers);
    let lifetime = e.freshness_lifetime();
    let age = e.current_age(now);
    let fresh = lifetime > age;
    // `immutable` (RFC 8246): a fresh response is not revalidated because the request asked (a reload's
    // `max-age=0` / `no-cache`).
    if fresh && rs.immutable && !rs.no_cache {
        return Decision::Serve;
    }
    if rs.no_cache || request_no_cache(req, &rq) {
        return revalidate;
    }
    let mut ok = fresh;
    if let Some(ma) = rq.max_age {
        ok &= age <= ma;
    }
    if let Some(mf) = rq.min_fresh {
        ok &= lifetime.saturating_sub(age) >= mf;
    }
    if ok {
        return Decision::Serve;
    }
    // Stale. must-revalidate forbids every stale use (§5.2.2.2).
    if rs.must_revalidate || (!fresh && rq.max_age == Some(0)) {
        return revalidate;
    }
    let staleness = age.saturating_sub(lifetime);
    if !fresh {
        if let Some(ms) = rq.max_stale {
            if ms.is_none_or(|n| staleness <= n) {
                return Decision::Serve;
            }
        }
        if let Some(swr) = rs.stale_while_revalidate {
            if staleness <= swr {
                return Decision::ServeAndRevalidate;
            }
        }
    }
    revalidate
}

/// §4.4: after a non-error response (2xx/3xx) to an unsafe method, the target URI and the same-origin
/// `Location` / `Content-Location` URIs are invalidated. Returns the URLs whose keys to drop.
pub fn invalidation_targets(method: &str, target: &Url, status: u16, resp: &Headers) -> Vec<Url> {
    let safe = matches!(method.to_ascii_uppercase().as_str(), "GET" | "HEAD" | "OPTIONS" | "TRACE");
    if safe || !(200..400).contains(&status) {
        return Vec::new();
    }
    let mut out = alloc::vec![target.clone()];
    for f in ["location", "content-location"] {
        if let Some(u) = resp.get(f).and_then(|v| target.join(v.trim()).ok()) {
            if u.same_origin(target) && !out.iter().any(|o| o.serialize(true) == u.serialize(true)) {
                out.push(u);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- the memory tier

/// A byte-bounded LRU map of entries (one selected response per primary key — §4.1 lets a cache keep only
/// the newest). An entry heavier than an eighth of the budget is not kept.
pub struct MemoryCache {
    map: BTreeMap<String, (Entry, u64)>,
    lru: BTreeMap<u64, String>,
    tick: u64,
    bytes: usize,
    max_bytes: usize,
}

impl MemoryCache {
    pub fn new(max_bytes: usize) -> Self {
        MemoryCache { map: BTreeMap::new(), lru: BTreeMap::new(), tick: 0, bytes: 0, max_bytes }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The entry for `key`, marked most recently used.
    pub fn get(&mut self, key: &str) -> Option<&Entry> {
        self.tick += 1;
        let tick = self.tick;
        let (_, t) = self.map.get_mut(key)?;
        self.lru.remove(t);
        *t = tick;
        self.lru.insert(tick, key.to_string());
        self.map.get(key).map(|(e, _)| e)
    }

    /// Store (replacing any entry under the same key), then evict least recently used entries to the budget.
    pub fn put(&mut self, e: Entry) {
        self.remove(&e.key);
        let w = e.weight();
        if w > self.max_bytes / 8 {
            return;
        }
        self.tick += 1;
        self.lru.insert(self.tick, e.key.clone());
        self.bytes += w;
        self.map.insert(e.key.clone(), (e, self.tick));
        while self.bytes > self.max_bytes {
            let Some((_, k)) = self.lru.pop_first() else { break };
            if let Some((old, _)) = self.map.remove(&k) {
                self.bytes -= old.weight();
            }
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Entry> {
        let (e, t) = self.map.remove(key)?;
        self.lru.remove(&t);
        self.bytes -= e.weight();
        Some(e)
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.lru.clear();
        self.bytes = 0;
    }
}

// ---------------------------------------------------------------- the on-disk form

const ENTRY_MAGIC: &[u8; 4] = b"UHC1";

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    out.extend_from_slice(b);
}

struct Reader<'a>(&'a [u8], usize);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.0.get(self.1..self.1.checked_add(n)?)?;
        self.1 += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i64(&mut self) -> Option<i64> {
        self.take(8).map(|b| i64::from_be_bytes(b.try_into().unwrap_or([0; 8])))
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn string(&mut self) -> Option<String> {
        core::str::from_utf8(self.bytes()?).ok().map(String::from)
    }
}

impl Entry {
    /// The disk form: `UHC1`, then length-prefixed key, url, reason, fields, vary, body and the two times.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.weight() + 64);
        out.extend_from_slice(ENTRY_MAGIC);
        put_bytes(&mut out, self.key.as_bytes());
        put_bytes(&mut out, self.url.as_bytes());
        out.extend_from_slice(&(self.status as u32).to_be_bytes());
        put_bytes(&mut out, self.reason.as_bytes());
        out.extend_from_slice(&self.request_time.to_be_bytes());
        out.extend_from_slice(&self.response_time.to_be_bytes());
        out.extend_from_slice(&(self.headers.len() as u32).to_be_bytes());
        for (k, v) in self.headers.iter() {
            put_bytes(&mut out, k.as_bytes());
            put_bytes(&mut out, v.as_bytes());
        }
        out.extend_from_slice(&(self.vary.len() as u32).to_be_bytes());
        for (n, v) in &self.vary {
            put_bytes(&mut out, n.as_bytes());
            match v {
                Some(v) => {
                    out.push(1);
                    put_bytes(&mut out, v.as_bytes());
                }
                None => out.push(0),
            }
        }
        put_bytes(&mut out, &self.body);
        out
    }

    /// The inverse of [`Entry::encode`]; `None` for anything malformed or truncated.
    pub fn decode(b: &[u8]) -> Option<Entry> {
        let mut r = Reader(b, 0);
        if r.take(4)? != ENTRY_MAGIC {
            return None;
        }
        let key = r.string()?;
        let url = r.string()?;
        let status = u16::try_from(r.u32()?).ok()?;
        let reason = r.string()?;
        let request_time = r.i64()?;
        let response_time = r.i64()?;
        let mut headers = Headers::new();
        for _ in 0..r.u32()? {
            let k = r.string()?;
            let v = r.string()?;
            headers.append(&k, &v).ok()?;
        }
        let mut vary = Vec::new();
        for _ in 0..r.u32()? {
            let n = r.string()?;
            let v = match r.take(1)?[0] {
                0 => None,
                1 => Some(r.string()?),
                _ => return None,
            };
            vary.push((n, v));
        }
        let body = r.bytes()?.to_vec();
        if r.1 != b.len() {
            return None;
        }
        Some(Entry { key, url, status, reason, headers, vary, body, request_time, response_time })
    }
}

/// FNV-1a 64 of a key: the disk tier's file name (the key itself is inside the file and checked on load).
pub fn key_hash(key: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn hdrs(list: &[(&str, &str)]) -> Headers {
        let mut h = Headers::new();
        for (k, v) in list {
            h.append(k, v).unwrap();
        }
        h
    }

    fn entry(resp: &[(&str, &str)], response_time: i64) -> Entry {
        let url = Url::parse("https://a.example/x.css#frag").unwrap();
        let key = cache_key("https://a.example", &url);
        let mut e = Entry::new(key, &url, &Headers::new(), 200, "OK", &hdrs(resp), response_time, response_time);
        e.body = b"body".to_vec();
        e
    }

    // 1994-11-06T08:49:37Z
    const T0: i64 = 784_111_777;

    #[test]
    fn cache_http_date_three_formats() {
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(T0));
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), Some(T0));
        assert_eq!(parse_http_date("Sun Nov  6 08:49:37 1994"), Some(T0));
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_http_date("0"), None);
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 25:49:37 GMT"), None);
    }

    #[test]
    fn cache_control_directives() {
        let cc = parse_cache_control(&hdrs(&[("Cache-Control", "public, max-age=60, no-cache=\"a, b\""), ("cache-control", "max-age=5, stale-while-revalidate=30")]));
        assert!(cc.public && cc.no_cache);
        assert_eq!(cc.max_age, Some(60), "first occurrence wins");
        assert_eq!(cc.stale_while_revalidate, Some(30));
        assert_eq!(parse_cache_control(&hdrs(&[("cache-control", "max-age=abc")])).max_age, Some(0));
        assert_eq!(parse_cache_control(&hdrs(&[("cache-control", "max-stale")])).max_stale, Some(None));
    }

    #[test]
    fn cache_freshness_explicit_and_heuristic() {
        let date = "Sun, 06 Nov 1994 08:49:37 GMT";
        assert_eq!(entry(&[("date", date), ("cache-control", "max-age=100"), ("expires", "0")], T0).freshness_lifetime(), 100);
        assert_eq!(entry(&[("date", date), ("expires", "Sun, 06 Nov 1994 09:49:37 GMT")], T0).freshness_lifetime(), 3600);
        assert_eq!(entry(&[("date", date), ("expires", "garbage")], T0).freshness_lifetime(), 0);
        // Heuristic: 10% of 10 days = 1 day.
        assert_eq!(entry(&[("date", date), ("last-modified", "Thu, 27 Oct 1994 08:49:37 GMT")], T0).freshness_lifetime(), 86_400);
        assert_eq!(entry(&[("date", date)], T0).freshness_lifetime(), 0);
    }

    #[test]
    fn cache_age_calculation() {
        // Date 10 s before the response arrived, Age 5, 2 s request delay, 30 s resident.
        let mut e = entry(&[("date", "Sun, 06 Nov 1994 08:49:27 GMT"), ("age", "5")], T0);
        e.request_time = T0 - 2;
        // apparent 10, corrected age value 5 + 2 = 7 → initial 10; + 30 resident.
        assert_eq!(e.current_age(T0 + 30), 40);
    }

    #[test]
    fn cache_decide_modes() {
        let now = T0 + 10;
        let fresh = entry(&[("date", "Sun, 06 Nov 1994 08:49:37 GMT"), ("cache-control", "max-age=60"), ("etag", "\"v1\"")], T0);
        let stale = entry(&[("date", "Sun, 06 Nov 1994 08:49:37 GMT"), ("cache-control", "max-age=5"), ("etag", "\"v1\"")], T0);
        let swr = entry(&[("date", "Sun, 06 Nov 1994 08:49:37 GMT"), ("cache-control", "max-age=5, stale-while-revalidate=60")], T0);
        let none = Headers::new();
        assert_eq!(decide(CacheMode::Default, Some(&fresh), &none, now), Decision::Serve);
        assert_eq!(decide(CacheMode::Default, Some(&stale), &none, now), Decision::Validate);
        assert_eq!(decide(CacheMode::Default, Some(&swr), &none, now), Decision::ServeAndRevalidate);
        assert_eq!(decide(CacheMode::NoCache, Some(&fresh), &none, now), Decision::Validate);
        assert_eq!(decide(CacheMode::ForceCache, Some(&stale), &none, now), Decision::Serve);
        assert_eq!(decide(CacheMode::Reload, Some(&fresh), &none, now), Decision::Network);
        assert_eq!(decide(CacheMode::OnlyIfCached, None, &none, now), Decision::Fail);
        assert_eq!(decide(CacheMode::Default, Some(&fresh), &hdrs(&[("cache-control", "no-cache")]), now), Decision::Validate);
        assert_eq!(decide(CacheMode::Default, Some(&fresh), &hdrs(&[("pragma", "no-cache")]), now), Decision::Validate);
        assert_eq!(decide(CacheMode::Default, Some(&stale), &hdrs(&[("cache-control", "max-stale")]), now), Decision::Serve);
        let mr = entry(&[("date", "Sun, 06 Nov 1994 08:49:37 GMT"), ("cache-control", "max-age=5, must-revalidate")], T0);
        assert_eq!(decide(CacheMode::Default, Some(&mr), &hdrs(&[("cache-control", "max-stale")]), now), Decision::Network);
        let imm = entry(&[("date", "Sun, 06 Nov 1994 08:49:37 GMT"), ("cache-control", "max-age=60, immutable")], T0);
        assert_eq!(decide(CacheMode::Default, Some(&imm), &hdrs(&[("cache-control", "max-age=0")]), now), Decision::Serve);
        assert_eq!(effective_mode(CacheMode::Default, "GET", &hdrs(&[("if-none-match", "\"x\"")])), CacheMode::NoStore);
        assert_eq!(effective_mode(CacheMode::ForceCache, "POST", &none), CacheMode::NoStore);
    }

    #[test]
    fn cache_storable_rules() {
        let none = Headers::new();
        assert!(storable("GET", &none, 200, &none), "200 is heuristically cacheable");
        assert!(!storable("POST", &none, 200, &hdrs(&[("cache-control", "max-age=60")])));
        assert!(!storable("GET", &none, 200, &hdrs(&[("cache-control", "no-store")])));
        assert!(!storable("GET", &hdrs(&[("cache-control", "no-store")]), 200, &none));
        assert!(!storable("GET", &none, 206, &none));
        assert!(!storable("GET", &none, 302, &none), "302 needs explicit freshness");
        assert!(storable("GET", &none, 302, &hdrs(&[("cache-control", "max-age=60")])));
        assert!(storable("GET", &hdrs(&[("authorization", "x")]), 200, &hdrs(&[("cache-control", "private")])));
        assert!(!storable("GET", &none, 200, &hdrs(&[("vary", "*")])));
    }

    #[test]
    fn cache_vary_secondary_key() {
        let url = Url::parse("https://a.example/x").unwrap();
        let req = hdrs(&[("accept-language", "en,  fr")]);
        let e = Entry::new(cache_key("p", &url), &url, &req, 200, "OK", &hdrs(&[("vary", "Accept-Language, Accept-Encoding")]), 0, 0);
        assert!(e.vary_matches(&hdrs(&[("Accept-Language", "en, fr")])));
        assert!(!e.vary_matches(&hdrs(&[("accept-language", "de")])));
        assert!(!e.vary_matches(&hdrs(&[("accept-language", "en, fr"), ("accept-encoding", "gzip")])));
    }

    #[test]
    fn cache_304_freshen_merges_fields() {
        let mut e = entry(&[("date", "Sun, 06 Nov 1994 08:49:37 GMT"), ("cache-control", "max-age=5"), ("etag", "\"v1\""), ("content-length", "4"), ("x-old", "1")], T0);
        let h304 = hdrs(&[("etag", "W/\"v1\""), ("cache-control", "max-age=600"), ("content-length", "0"), ("set-cookie", "a=b"), ("x-new", "2")]);
        assert!(e.validator_matches(&h304));
        assert!(!e.validator_matches(&hdrs(&[("etag", "\"v2\"")])));
        e.freshen(&h304, T0 + 100, T0 + 100);
        assert_eq!(e.headers.get("cache-control"), Some("max-age=600"));
        assert_eq!(e.headers.get("content-length"), Some("4"), "framing of the stored body is kept");
        assert_eq!(e.headers.get("x-old"), Some("1"));
        assert_eq!(e.headers.get("x-new"), Some("2"));
        assert!(!e.headers.contains("set-cookie"));
        assert_eq!(e.response_time, T0 + 100);
        let mut req = Headers::new();
        assert!(e.add_conditionals(&mut req));
        // §3.2: the 304's ETag replaced the stored one (freshen keeps only the stored body's framing/codings),
        // so revalidation now sends the validator the server last gave.
        assert_eq!(req.get("if-none-match"), Some("W/\"v1\""));
    }

    #[test]
    fn cache_unsafe_method_invalidation() {
        let t = Url::parse("https://a.example/items/1").unwrap();
        let resp = hdrs(&[("location", "/items"), ("content-location", "https://b.example/x")]);
        let got: Vec<String> = invalidation_targets("POST", &t, 201, &resp).iter().map(|u| u.href()).collect();
        assert_eq!(got, vec!["https://a.example/items/1".to_string(), "https://a.example/items".to_string()]);
        assert!(invalidation_targets("GET", &t, 200, &resp).is_empty());
        assert!(invalidation_targets("DELETE", &t, 500, &resp).is_empty());
    }

    #[test]
    fn cache_memory_lru_and_disk_form() {
        let mut m = MemoryCache::new(4096);
        let mk = |n: usize| {
            let url = Url::parse(&format!("https://a.example/{n}")).unwrap();
            let mut e = Entry::new(cache_key("p", &url), &url, &Headers::new(), 200, "OK", &Headers::new(), 0, 0);
            e.body = vec![0u8; 300];
            e
        };
        for n in 0..8 {
            m.put(mk(n));
        }
        let k0 = mk(0).key.clone();
        assert!(m.get(&k0).is_some());
        for n in 8..20 {
            m.put(mk(n));
        }
        assert!(m.bytes() <= 4096);
        assert!(m.get(&mk(1).key).is_none(), "least recently used evicted");
        let e = entry(&[("etag", "\"v1\""), ("vary", "accept")], T0);
        let back = Entry::decode(&e.encode()).unwrap();
        assert_eq!(back, e);
        assert!(Entry::decode(&e.encode()[..10]).is_none());
        assert_eq!(cache_key("https://a.example", &Url::parse("https://a.example/x#y").unwrap()), "https://a.example https://a.example/x");
        assert_eq!(site_of(&Url::parse("https://sub.a.example:8443/p").unwrap()), "https://a.example");
        assert_eq!(site_of(&Url::parse("https://x.y.example.co.uk/").unwrap()), "https://example.co.uk");
        assert_eq!(site_of(&Url::parse("https://alice.github.io/").unwrap()), "https://alice.github.io");
        assert_eq!(site_of(&Url::parse("http://127.0.0.1:8080/").unwrap()), "http://127.0.0.1");
    }
}
