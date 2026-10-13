// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERNET (LEDGER SR110): Aether's pages off the NETWORK — `http://` and `https://` on the image. CODEX §2: Aether
// owns the Web; seam `shared-core` (every rule below is a shared no_std core's, linked; this file only sequences them).
//
// MECHANISM (one navigation, `fetch(url)`):
// 1. URL — `http_core::url` (= html_core::url, URLCORE SR87, the one WHATWG parser). Only http/https; IPv6 refused.
// 2. NAME — `vein_ring3::net::resolve` = SYS_RESOLVE (57, una-abi), the kernel resolver over the DHCP-leased DNS
//    (netring3.rs → smolnet::resolve on x86; aarch64 answers -ENODEV until the NET6 resolver is bound, owed there).
//    The way crates/user-net resolves names; no new verb. A small answer cache (60 s) saves a lookup per hop.
// 3. SOCKET — `vein_ring3::net::Tcp` over SYS_SOCKET 40 / SYS_CONNECT 44 / SYS_SEND 45 / SYS_SOCK_RECV 46 / SYS_CLOSE
//    17 (non-blocking verbs polled, connect bounded at 15 s, I/O idle at 120 s), plus this file's per-navigation
//    deadline (`FETCH_MS`) through `vein_ring3::net::set_tick`: past it every wait answers ECANCELED → `timeout`.
// 4. TLS — `vein_ring3::tls::with_session_alpn` (ALPN `h2, http/1.1`, NETDEBT2 SR113) over
//    `vein_ring3::TlsSetup::load()` (VEINTLS SR36: tls_core on CRYPTOCORE, the chain built to /system/trust/roots.pem,
//    checked at SYS_TIME, RFC 6125 name match; the CT list and CCADB intermediates when staged). Exactly NET.ELF's and
//    LUMEN.ELF's root pinning — loaded once, on the first https navigation. No provider / no trust store / no clock = a named refusal; there is no insecure mode.
// 5. HTTP — over TLS the handshake offers ALPN `h2, http/1.1` and `Session::alpn` says what the server chose
//    (NETDEBT2 SR113). `h2`: `http_core::h2::H2Conn` (no_std, the SAME client the host transport multiplexes with) —
//    preface + SETTINGS, one stream per navigation (HEADERS with :method/:scheme/:authority/:path, the fields below
//    minus the connection-specific ones), the head, DATA under both receive windows, the stream closed; the TLS
//    session ends with the navigation as before. Otherwise (and always for plain http) HTTP/1.1:
//    `http_core::h1::encode_request_head` + `http_core::Conn` (interim 1xx skipped, §6.3 framing, chunked),
//    `h1::keep_alive` for reuse. The wire's `proto=h2|h1` says which carried the page.
// 6. CODINGS — `http_core::encoding` (gzip/deflate through pixel_core's inflater, br through the core's RFC 7932
//    decoder; `Accept-Encoding` is the core's `ACCEPT_ENCODING` = `gzip, deflate, br`, what the host sends too).
// 7. COOKIES — `http_core::cookie::CookieJar` (RFC 6265bis: Secure, __Host-/__Secure- prefixes, SameSite with Lax
//    default, Partitioned, the public-suffix Domain refusal through psl_core) in a `CookieContext` this file fills
//    from psl_core's registrable domain: `cross_site` = the target's site differs from the last network page's,
//    `top_level_navigation` + `safe_method` (every fetch here is a top-level GET), `partition` = the target's site.
// 8. REDIRECTS — `http_core::redirect::next_hop` (Fetch §4.4: 301/302/303/307/308, Location joined, fragment
//    inherited, only http/https followed) up to `redirect::MAX_REDIRECTS` (20); the request is rebuilt per hop, so the
//    core's credential rule holds (cookies recomputed by the jar for the new URL; no Authorization is ever sent).
// 9. CONNECTIONS — plain-HTTP keep-alive sockets are parked per origin and reused; at most `MAX_PER_ORIGIN` (=
//    `http_core::MAX_H1_PER_ORIGIN`, the no_std const the host pool reads too — ONE limit, NETDEBT2 SR113) are
//    open to one origin, idle ones dropped after `IDLE_MS`. A TLS session lives inside `with_session`'s closure, so an
//    https request is one connection with `Connection: close`.
//
// SECURITY POLICY (SR86 AETHERORIGIN). Its CORS / CSP / mixed-content / SRI / Referrer modules live in
// handlers/aether/src/security (std, host-only); they police subresource fetches and script, which this program does
// not make. The parts of SR86 that live in the shared cores ARE the ones used: the RFC 6265bis jar (cookie.rs) and the
// redirect credential rule (redirect.rs). Nothing is re-implemented here, so there is no second switch.
//
// WIRE, one line per navigation:
// `:: AETHER: net url=<final url> status=<n> bytes=<decoded body> ms=<n> tls=<1.3|1.2|off> proto=<h2|h1>
//  hops=<n> conn=<new|reused> coding=<identity|gzip|deflate|br> [issuer=<CN>] -> PASS|FAIL [reason=<why> code=<n>] ::`
// — PASS when a final answer arrived (any status: a 404 page is a page); FAIL names the stage (`resolve`, `connect`,
// `tls-<why>`, `h2-protocol`, `timeout`, ...).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use http_core::cookie::{CookieContext, CookieJar};
use http_core::encoding::{self, Coding};
use http_core::h1::{self, BodyDecoder};
use http_core::h2::H2Conn;
use http_core::url::{Host, Url};
use http_core::{redirect, Conn, Headers};
use vein_core::client::Transport as VeinTransport;
use vein_core::prefs::Verify;
use vein_ring3::net::{Tcp, ECANCELED};

use crate::sys::now_ms;
use crate::Line;

/// The per-origin HTTP/1.1 connection cap: http_core's shared `MAX_H1_PER_ORIGIN` (NETDEBT SR97), the SAME constant
/// the host transport's pool reads (NETDEBT2 SR113) — never a copy.
pub const MAX_PER_ORIGIN: usize = http_core::MAX_H1_PER_ORIGIN;
/// Every navigation (all hops, DNS, connect, TLS, body) finishes inside this or ends `timeout`.
pub const FETCH_MS: u64 = 30_000;
/// A parked keep-alive socket older than this is closed rather than reused (servers time idle ones out).
const IDLE_MS: u64 = 15_000;
/// Parked sockets across every origin.
const POOL_MAX: usize = 16;
/// A resolver answer is reused this long.
const DNS_TTL_MS: u64 = 60_000;
/// The largest body read (still content-coded) and decoded; Aether's file pages stop at 1 MiB, the network at 4.
const BODY_MAX: usize = 4 * 1024 * 1024;
const UA: &str = "Mozilla/5.0 (UnaOS) Aether/1";
const ACCEPT: &str = "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.8";

/// An absolute http(s) URL (what `fetch` takes and what a network page's `path` is).
pub fn is_net(s: &str) -> bool {
    let b = s.as_bytes();
    let pre = |p: &[u8]| b.len() > p.len() && b[..p.len()].eq_ignore_ascii_case(p);
    pre(b"http://") || pre(b"https://")
}

/// A scheme this module fetches.
pub fn is_net_scheme(s: &str) -> bool {
    s == "http" || s == "https"
}

/// `aether <url>`: argv[1] when it is an http(s) URL — the page to open after the first one is reported.
pub fn arg_url() -> Option<String> {
    let a = una_abi::args()?;
    let u = core::str::from_utf8(a.get(1)?).ok()?;
    if is_net(u) { Some(String::from(u)) } else { None }
}

// ---- state (one thread: the program's) ----------------------------------------------------------------------

struct Idle {
    origin: String,
    tcp: Tcp,
    at: u64,
}

struct Net {
    tls: Option<vein_ring3::TlsSetup>,
    jar: CookieJar,
    pool: Vec<Idle>,
    dns: Vec<(String, [u8; 4], u64)>,
    /// The registrable domain of the last page fetched (the site-for-cookies of the next navigation).
    site: Option<String>,
}

static mut NET: Option<Net> = None;

fn state() -> &'static mut Net {
    // SAFETY: AETHER.ELF is single-threaded and nothing here re-enters `state` while a borrow is live (the tick hook
    // reads only `DEADLINE`).
    unsafe { (*core::ptr::addr_of_mut!(NET)).get_or_insert_with(|| Net { tls: None, jar: CookieJar::new(), pool: Vec::new(), dns: Vec::new(), site: None }) }
}

static DEADLINE: AtomicU64 = AtomicU64::new(0);

/// vein_ring3's wait hook: false (→ ECANCELED) once the navigation's deadline has passed.
fn tick() -> bool {
    now_ms() < DEADLINE.load(Ordering::Relaxed)
}

// ---- failure ----------------------------------------------------------------------------------------------

/// Why a navigation did not produce a page: the wire's `reason=` and `code=`.
#[derive(Clone, Copy)]
struct Fail {
    why: &'static str,
    code: i64,
}

fn fail(why: &'static str, code: i64) -> Fail {
    Fail { why, code }
}

/// http_core's error, named for the wire (a transport errno rides `Io` as text; ECANCELED is the deadline).
fn http_fail(e: &http_core::Error) -> Fail {
    use http_core::encoding::DecodeError;
    match e {
        http_core::Error::Timeout => fail("timeout", ECANCELED),
        http_core::Error::Io(_) => fail("io", una_abi::EIO),
        http_core::Error::Tls(_) => fail("tls-stream", vein_ring3::tls::EPROTO),
        http_core::Error::Http(h1::H1Error::Truncated) => fail("truncated", 0),
        http_core::Error::Http(_) => fail("http-framing", 0),
        http_core::Error::Decode(DecodeError::Unsupported(_)) => fail("coding-unsupported", 0),
        http_core::Error::Decode(_) => fail("coding-corrupt", 0),
        http_core::Error::Url(_) => fail("bad-url", 0),
        http_core::Error::Redirect(_) => fail("redirect-refused", 0),
        http_core::Error::TooManyRedirects => fail("too-many-redirects", 0),
        http_core::Error::Proxy(_) => fail("proxy", 0),
    }
}

// ---- the byte stream http_core reads: any vein Transport (a Tcp, a TLS session) ------------------------------

struct Wire<'a, T: VeinTransport + ?Sized>(&'a mut T);

fn io_err(e: i64) -> http_core::Error {
    if e == ECANCELED {
        http_core::Error::Timeout
    } else {
        http_core::Error::Io(alloc::format!("errno {}", e))
    }
}

impl<T: VeinTransport + ?Sized> http_core::Transport for Wire<'_, T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, http_core::Error> {
        self.0.recv(buf).map_err(io_err)
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), http_core::Error> {
        self.0.send_all(data).map_err(io_err)
    }
}

/// One answer, body still content-coded.
struct Resp {
    status: u16,
    headers: Headers,
    body: Vec<u8>,
    /// The connection may carry the next request (§9.3, nothing buffered past the message).
    keep: bool,
}

/// Send `head` and read one whole response over `t`.
fn exchange<T: VeinTransport + ?Sized>(t: &mut T, head: &[u8], said_close: bool) -> Result<Resp, Fail> {
    let mut c = Conn::new(Wire(t));
    c.send(head, &[]).map_err(|e| http_fail(&e))?;
    let (rh, framing) = c.read_head("GET").map_err(|e| http_fail(&e))?;
    let mut dec = BodyDecoder::new(framing);
    let mut body = Vec::new();
    while let Some(chunk) = c.read_body(&mut dec).map_err(|e| http_fail(&e))? {
        if body.len() + chunk.len() > BODY_MAX {
            return Err(fail("body-too-large", BODY_MAX as i64));
        }
        body.extend_from_slice(&chunk);
    }
    let keep = h1::keep_alive(&rh, framing, said_close) && c.buffered() == 0;
    Ok(Resp { status: rh.status, headers: rh.headers, body, keep })
}

/// One GET over HTTP/2 (the server chose `h2` by ALPN): the preface and SETTINGS exchange, one stream, the whole
/// answer. A protocol failure (GOAWAY, RST_STREAM, a malformed frame) is `h2-protocol`; the transport's own errors
/// keep their names (`timeout`, `io`).
fn exchange_h2<T: VeinTransport + ?Sized>(t: &mut T, authority: &str, target: &str, h: &Headers) -> Result<Resp, Fail> {
    let h2_fail = |e: http_core::Error| match &e {
        http_core::Error::Io(s) if !s.starts_with("errno") => fail("h2-protocol", vein_ring3::tls::EPROTO),
        _ => http_fail(&e),
    };
    let mut c = H2Conn::handshake(Wire(t)).map_err(h2_fail)?;
    let id = c.send_request("GET", "https", authority, target, h, &[]).map_err(h2_fail)?;
    let (status, headers) = c.read_head(id).map_err(h2_fail)?;
    let mut body = Vec::new();
    while let Some(chunk) = c.read_data(id).map_err(h2_fail)? {
        if body.len() + chunk.len() > BODY_MAX {
            let _ = c.cancel(id);
            return Err(fail("body-too-large", BODY_MAX as i64));
        }
        body.extend_from_slice(&chunk);
    }
    c.close_stream(id);
    Ok(Resp { status, headers, body, keep: false })
}

// ---- name, socket ---------------------------------------------------------------------------------------------

/// The IPv4 address of `url`'s host: a literal as written, else the kernel resolver (cached).
fn address(n: &mut Net, url: &Url) -> Result<[u8; 4], Fail> {
    match url.host() {
        Some(Host::Ipv4(v)) => Ok(v.to_be_bytes()),
        Some(Host::Domain(d)) => {
            let now = now_ms();
            n.dns.retain(|(_, _, at)| now.saturating_sub(*at) < DNS_TTL_MS);
            if let Some((_, ip, _)) = n.dns.iter().find(|(h, _, _)| h == d) {
                return Ok(*ip);
            }
            let ip = vein_ring3::net::resolve(d).map_err(|e| fail(if e == una_abi::ENODEV { "resolve-no-link" } else if e == una_abi::ENOENT { "resolve-no-answer" } else { "resolve" }, e))?;
            if n.dns.len() >= 32 {
                n.dns.remove(0);
            }
            n.dns.push((d.clone(), ip, now));
            Ok(ip)
        }
        Some(Host::Ipv6(_)) => Err(fail("ipv6-unsupported", 0)),
        _ => Err(fail("no-host", 0)),
    }
}

/// `scheme://host:port` — the pool's key (an origin).
fn origin_key(url: &Url, port: u16) -> String {
    alloc::format!("{}://{}:{}", url.scheme(), url.hostname(), port)
}

/// A parked socket for `origin`, freshest first (stale ones closed on the way).
fn take_idle(n: &mut Net, origin: &str) -> Option<Tcp> {
    let now = now_ms();
    n.pool.retain(|i| now.saturating_sub(i.at) < IDLE_MS); // dropping an Idle closes its socket
    let k = n.pool.iter().rposition(|i| i.origin == origin)?;
    Some(n.pool.remove(k).tcp)
}

/// Park `tcp` for `origin` — never past `MAX_PER_ORIGIN` for one origin (one is in flight while we fetch, so at
/// most `MAX_PER_ORIGIN - 1` wait) nor `POOL_MAX` overall (the oldest goes).
fn park(n: &mut Net, origin: String, tcp: Tcp) {
    if n.pool.iter().filter(|i| i.origin == origin).count() + 1 >= MAX_PER_ORIGIN {
        return;
    }
    if n.pool.len() >= POOL_MAX {
        n.pool.remove(0);
    }
    n.pool.push(Idle { origin, tcp, at: now_ms() });
}

// ---- one hop ----------------------------------------------------------------------------------------------

/// What one hop learned beyond the response.
struct Hop {
    tls: u16,
    /// The page came over HTTP/2 (ALPN `h2`); otherwise HTTP/1.1.
    h2: bool,
    reused: bool,
    issuer: Option<vein_ring3::Verified>,
}

/// One request for `url` (GET, top-level navigation) and its whole answer.
fn hop(n: &mut Net, url: &Url, ctx: &CookieContext) -> Result<(Resp, Hop), Fail> {
    let tls = url.scheme() == "https";
    let port = url.port_or_default().unwrap_or(if tls { 443 } else { 80 });
    let mut target = url.pathname();
    if target.is_empty() {
        target.push('/');
    }
    target.push_str(&url.search());
    let mut h = Headers::new();
    let _ = h.set("User-Agent", UA);
    let _ = h.set("Accept", ACCEPT);
    let _ = h.set("Accept-Encoding", encoding::ACCEPT_ENCODING);
    let _ = h.set("Connection", if tls { "close" } else { "keep-alive" });
    let unix = vein_ring3::sys::unix_time().unwrap_or(0);
    if let Some(c) = n.jar.cookie_header_in(url, unix, true, ctx) {
        let _ = h.set("Cookie", &c);
    }
    let head = h1::encode_request_head("GET", &target, &url.host_str(), &h, None).map_err(|_| fail("bad-request", 0))?;
    let ip = address(n, url)?;

    if !tls {
        let origin = origin_key(url, port);
        // A parked socket first; a reused one the server already closed fails before any answer: retry once, fresh.
        if let Some(mut t) = take_idle(n, &origin) {
            if let Ok(r) = exchange(&mut t, &head, false) {
                if r.keep {
                    park(n, origin, t);
                }
                return Ok((r, Hop { tls: 0, h2: false, reused: true, issuer: None }));
            }
        }
        let mut t = Tcp::connect(ip, port).map_err(|e| fail(if e == ECANCELED { "timeout" } else { "connect" }, e))?;
        let r = exchange(&mut t, &head, false)?;
        if r.keep {
            park(n, origin, t);
        }
        return Ok((r, Hop { tls: 0, h2: false, reused: false, issuer: None }));
    }

    if n.tls.is_none() {
        n.tls = Some(vein_ring3::TlsSetup::load()); // VEINTLS: provider, /system/trust/roots.pem, the CT list — once
    }
    let setup = n.tls.as_ref().ok_or(fail("tls-no-context", 0))?;
    match setup.verify() {
        Verify::Ready => {}
        Verify::NoProvider => return Err(fail("tls-no-provider", 0)),
        Verify::NoTrustStore => return Err(fail("tls-no-trust-store", 0)),
        Verify::NoClock => return Err(fail("tls-clock-unset", una_abi::EAGAIN)),
    }
    let ctx_tls = setup.context().ok_or(fail("tls-no-context", 0))?;
    let mut t = Tcp::connect(ip, port).map_err(|e| fail(if e == ECANCELED { "timeout" } else { "connect" }, e))?;
    let host = url.hostname();
    let authority = url.host_str();
    // NETDEBT2 (SR113): offer h2 first; the server's choice picks the exchange.
    let done = vein_ring3::tls::with_session_alpn(&mut t, &host, &ctx_tls, vein_ring3::tls::ALPN_H2_HTTP1, |s, v| {
        let h2 = s.is_h2();
        let r = if h2 { exchange_h2(s, &authority, &target, &h) } else { exchange(s, &head, true) };
        (r, s.version(), v, h2)
    });
    match done {
        Err(f) => Err(fail(tls_why(f.why), f.errno)),
        Ok(((Ok(r), ver, v, h2), _)) => Ok((r, Hop { tls: ver, h2, reused: false, issuer: Some(v) })),
        Ok(((Err(e), _, _, _), Some(tf))) if e.why != "timeout" => Err(fail(tls_why(tf.why), tf.errno)),
        Ok(((Err(e), _, _, _), _)) => Err(e),
    }
}

/// `tls-<reason>` for the wire (tls_core's names are static; the prefix is chosen from a fixed set).
fn tls_why(w: &'static str) -> &'static str {
    match w {
        "clock-unset" => "tls-clock-unset",
        "no-trust-anchors" => "tls-no-trust-anchors",
        "cert-unknown-issuer" => "tls-cert-unknown-issuer",
        "cert-expired" => "tls-cert-expired",
        "cert-not-yet-valid" => "tls-cert-not-yet-valid",
        "cert-name-mismatch" => "tls-cert-name-mismatch",
        "transport" => "tls-transport",
        _ => w,
    }
}

// ---- the navigation ---------------------------------------------------------------------------------------

/// psl_core's site (registrable domain, or the host itself for an IP / a bare suffix) of `url`.
fn site_of(url: &Url) -> Option<String> {
    match url.host()? {
        Host::Domain(d) => Some(String::from(psl_core::site_host(d))),
        h => Some(alloc::format!("{}", h)),
    }
}

/// Fetch `url` (an absolute http/https URL) as a top-level navigation: `(body, final URL)` — the final URL without
/// its fragment is the page's path (its relative hrefs resolve against it) — or the failure's code. Writes the
/// `:: AETHER: net` line either way.
pub fn fetch(url: &str) -> Result<(Vec<u8>, String), i64> {
    let t0 = now_ms();
    DEADLINE.store(t0 + FETCH_MS, Ordering::Relaxed);
    vein_ring3::net::set_tick(Some(tick));
    let n = state();
    let mut cur = match Url::parse(url) {
        Ok(u) if is_net_scheme(u.scheme()) => Ok(u),
        Ok(_) => Err(fail("not-http", 0)),
        Err(_) => Err(fail("bad-url", 0)),
    };
    let mut hops = 0usize;
    let mut last: Option<Hop> = None;
    let out: Result<Resp, Fail> = loop {
        let u = match &cur {
            Ok(u) => u.clone(),
            Err(f) => break Err(*f),
        };
        let target_site = site_of(&u);
        let ctx = CookieContext {
            cross_site: matches!((&n.site, &target_site), (Some(a), Some(b)) if a != b),
            top_level_navigation: true,
            safe_method: true,
            partition: target_site.clone(),
        };
        let (r, h) = match hop(n, &u, &ctx) {
            Ok(x) => x,
            Err(f) => break Err(f),
        };
        last = Some(h);
        let unix = vein_ring3::sys::unix_time().unwrap_or(0);
        for v in r.headers.get_all("set-cookie") {
            n.jar.set_cookie_in(&u, v, unix, true, &ctx);
        }
        match redirect::next_hop(r.status, "GET", &u, r.headers.get("location")) {
            None => {
                n.site = target_site;
                cur = Ok(u);
                break Ok(r);
            }
            Some(Err(_)) => break Err(fail("redirect-refused", r.status as i64)),
            Some(Ok(next)) => {
                hops += 1;
                if hops > redirect::MAX_REDIRECTS {
                    break Err(fail("too-many-redirects", hops as i64));
                }
                n.site = target_site; // the redirecting response's site is the next hop's site-for-cookies
                cur = Ok(next.url);
            }
        }
    };
    // Undo the content codings (gzip/deflate/br; any other is a named refusal).
    let mut coding: &[u8] = b"identity";
    let out = out.and_then(|r| {
        let codings = encoding::content_codings(&r.headers);
        coding = match codings.last() {
            None => b"identity",
            Some(Coding::Gzip) => b"gzip",
            Some(Coding::Deflate) => b"deflate",
            Some(Coding::Brotli) => b"br",
            Some(_) => b"other",
        };
        if codings.is_empty() {
            return Ok((r.status, r.body));
        }
        encoding::decode(&codings, &r.body).map(|b| (r.status, b)).map_err(|e| http_fail(&http_core::Error::Decode(e)))
    });
    vein_ring3::net::set_tick(None);
    let ms = now_ms().saturating_sub(t0);
    let shown = match &cur {
        Ok(u) => u.serialize(true),
        Err(_) => String::from(url),
    };

    let mut l = Line::new(b":: AETHER: net url=");
    l.put(crate::cut(shown.as_bytes(), 80));
    l.put(b" status=").dec(out.as_ref().map_or(0, |(s, _)| *s as i64));
    l.put(b" bytes=").dec(out.as_ref().map_or(0, |(_, b)| b.len() as i64));
    l.put(b" ms=").dec(ms as i64);
    l.put(b" tls=").put(match last.as_ref().map(|h| h.tls) {
        Some(0x0304) => b"1.3" as &[u8],
        Some(0x0303) => b"1.2",
        _ => b"off",
    });
    // `h2`/`h1`, short: the line buffer is 240 bytes.
    l.put(b" proto=").put(if last.as_ref().is_some_and(|h| h.h2) { b"h2" as &[u8] } else { b"h1" });
    l.put(b" hops=").dec(hops as i64);
    l.put(b" conn=").put(if last.as_ref().is_some_and(|h| h.reused) { b"reused" as &[u8] } else { b"new" });
    l.put(b" coding=").put(coding);
    if let Some(v) = last.as_ref().and_then(|h| h.issuer) {
        l.put(b" issuer=").put(crate::cut(v.issuer().as_bytes(), 32));
    }
    match &out {
        Ok(_) => {
            l.put(b" -> PASS ::");
        }
        Err(f) => {
            l.put(b" -> FAIL reason=").put(f.why.as_bytes()).put(b" code=").dec(f.code).put(b" ::");
        }
    }
    l.wire();
    match out {
        Ok((_, body)) => Ok((body, shown)),
        Err(f) => Err(if f.code != 0 { f.code } else { una_abi::EIO }),
    }
}
