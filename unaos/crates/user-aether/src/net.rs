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
//    The way crates/user-net resolves names; no new verb. A small answer cache (60 s) saves a lookup per hop. The
//    kernel says each lookup (`[dns] q=<name> -> a=<ip>|none ms=<n>`, AETHERNET2 SR187); a refusal is `reason=dns
//    code=<errno>` (-19 no link, -2 no answer).
// 3. SOCKET — `vein_ring3::net::Tcp` over SYS_SOCKET 40 / SYS_CONNECT 44 / SYS_SEND 45 / SYS_SOCK_RECV 46 / SYS_CLOSE
//    17 (non-blocking verbs polled, connect bounded at 15 s, I/O idle at 120 s), plus this file's per-navigation
//    deadline (`FETCH_MS`) through `vein_ring3::net::set_tick`: past it every wait answers ECANCELED → `timeout`.
// 4. TLS — `vein_ring3::tls::with_session_alpn` (ALPN `h2, http/1.1`, NETDEBT2 SR113) over
//    `vein_ring3::TlsSetup::load()` (VEINTLS SR36: tls_core on CRYPTOCORE, the chain built to /system/trust/roots.pem,
//    checked at SYS_TIME, RFC 6125 name match; the CT list and CCADB intermediates when staged). Exactly NET.ELF's and
//    LUMEN.ELF's root pinning — loaded once, on the first https navigation. No provider / no trust store / no clock = a named refusal; there is no insecure mode.
//    AETHERNET2 (SR187): the store is read through SYS_PATH_READ (the VFS — /system is on the UnaFS root), so the
//    `tls-no-trust-store` refusal's `code=` is the reader's errno (-2 = no bundle staged on the volume).
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
// `:: AETHER: net url=<final url> status=<n> bytes=<decoded body> ms=<n> tls=<1.3|1.2|off> [cipher=<name>] proto=<h2|h1>
//  hops=<n> conn=<new|reused> coding=<identity|gzip|deflate|br> [issuer=<CN>] -> PASS|FAIL [reason=<why> code=<n>] ::`
// — PASS when a final answer arrived (any status: a 404 page is a page); FAIL names the stage (`dns`, `connect`,
// `tls-<why>`, `h2-protocol`, `timeout`, ...).
//
// AETHERLIB (LEDGER SR193): steps 2, 3's retry, 4's refusals, 8 and 9's pool are SEQUENCED by one std-free model,
// handlers/aether/src/net/fetchseq.rs, linked here by `#[path]` (R104, the AETHERSCALE shape) and driven on the host by a
// stub: the answer cache, the keep-alive pool, the hop order, the redirect count and the `reason=` words live there; this
// file implements its `Io` over vein_ring3 + http_core (the `Elf` below) and writes the wire line from its `Nav`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use http_core::cookie::{CookieContext, CookieJar};
use http_core::encoding::{self, Coding};
use http_core::h1::{self, BodyDecoder};
use http_core::h2::H2Conn;
use http_core::url::Url;
use http_core::{redirect, Conn, Headers};
use vein_core::client::Transport as VeinTransport;
use vein_core::prefs::Verify;
use vein_ring3::net::{Tcp, ECANCELED};

use crate::sys::now_ms;
use crate::Line;

// AETHERLIB (SR193): the sequencer's model — the bytes the host test drives (handlers/aether/src/net/fetchseq.rs).
#[allow(dead_code)]
#[path = "../../../../handlers/aether/src/net/fetchseq.rs"]
mod fetchseq;
use fetchseq::{fail, Fail, Gate, Session};
pub use fetchseq::is_net_scheme;

/// The per-origin HTTP/1.1 connection cap: http_core's shared `MAX_H1_PER_ORIGIN` (NETDEBT SR97), the SAME constant
/// the host transport's pool reads (NETDEBT2 SR113) — never a copy.
pub const MAX_PER_ORIGIN: usize = http_core::MAX_H1_PER_ORIGIN;
/// Every navigation (all hops, DNS, connect, TLS, body) finishes inside this or ends `timeout`.
pub const FETCH_MS: u64 = 30_000;
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

/// `aether <url>`: argv[1] when it is an http(s) URL — the page to open after the first one is reported.
pub fn arg_url() -> Option<String> {
    let a = una_abi::args()?;
    let u = core::str::from_utf8(a.get(1)?).ok()?;
    if is_net(u) { Some(String::from(u)) } else { None }
}

// ---- state (one thread: the program's) ----------------------------------------------------------------------

struct Net {
    tls: Option<vein_ring3::TlsSetup>,
    jar: CookieJar,
    /// AETHERLIB (SR193): the answer cache, the parked sockets and the last page's site (the site-for-cookies of the
    /// next navigation) — the model's state.
    seq: fetchseq::Seq<Tcp>,
}

static mut NET: Option<Net> = None;

fn state() -> &'static mut Net {
    // SAFETY: AETHER.ELF is single-threaded and nothing here re-enters `state` while a borrow is live (the tick hook
    // reads only `DEADLINE`).
    unsafe { (*core::ptr::addr_of_mut!(NET)).get_or_insert_with(|| Net { tls: None, jar: CookieJar::new(), seq: fetchseq::Seq::new() }) }
}

static DEADLINE: AtomicU64 = AtomicU64::new(0);

/// vein_ring3's wait hook: false (→ ECANCELED) once the navigation's deadline has passed.
fn tick() -> bool {
    now_ms() < DEADLINE.load(Ordering::Relaxed)
}

// ---- failure ----------------------------------------------------------------------------------------------

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

// ---- the sequencer's I/O over this program's syscalls (AETHERLIB SR193) ----------------------------------------

/// One encoded GET: the HTTP/1.1 head, and the target and fields an h2 stream sends instead.
struct Req {
    head: Vec<u8>,
    target: String,
    h: Headers,
}

/// `fetchseq::Io` over vein_ring3 (sockets, SYS_RESOLVE, TLS) and http_core (framing, the jar, the redirect rule).
struct Elf<'a> {
    tls: &'a mut Option<vein_ring3::TlsSetup>,
    jar: &'a mut CookieJar,
}

impl fetchseq::Io for Elf<'_> {
    type Conn = Tcp;
    type Req = Req;
    type Resp = Resp;
    type Ctx = CookieContext;
    type Issuer = vein_ring3::Verified;
    const CANCELED: i64 = ECANCELED;
    const MAX_REDIRECTS: usize = redirect::MAX_REDIRECTS;
    const PER_ORIGIN: usize = MAX_PER_ORIGIN;

    fn now(&mut self) -> u64 {
        now_ms()
    }

    fn resolve(&mut self, host: &str) -> Result<[u8; 4], i64> {
        vein_ring3::net::resolve(host)
    }

    fn connect(&mut self, ip: [u8; 4], port: u16) -> Result<Tcp, i64> {
        Tcp::connect(ip, port)
    }

    fn context(&mut self, cross_site: bool, partition: Option<String>) -> CookieContext {
        CookieContext { cross_site, top_level_navigation: true, safe_method: true, partition }
    }

    fn request(&mut self, url: &Url, tls: bool, ctx: &CookieContext) -> Option<Req> {
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
        if let Some(c) = self.jar.cookie_header_in(url, unix, true, ctx) {
            let _ = h.set("Cookie", &c);
        }
        let head = h1::encode_request_head("GET", &target, &url.host_str(), &h, None).ok()?;
        Some(Req { head, target, h })
    }

    fn exchange(&mut self, c: &mut Tcp, req: &Req) -> Result<(Resp, bool), Fail> {
        exchange(c, &req.head, false).map(|r| {
            let keep = r.keep;
            (r, keep)
        })
    }

    fn tls_ready(&mut self) -> Result<(), Gate> {
        if self.tls.is_none() {
            *self.tls = Some(vein_ring3::TlsSetup::load()); // VEINTLS: provider, /system/trust/roots.pem, the CT list — once
        }
        let setup = self.tls.as_ref().ok_or(Gate::NoContext)?;
        match setup.verify() {
            Verify::Ready => {}
            Verify::NoProvider => return Err(Gate::NoProvider),
            Verify::NoTrustStore => return Err(Gate::NoTrustStore(setup.trust_fail.map_or(0, |f| f.errno()))),
            Verify::NoClock => return Err(Gate::NoClock(una_abi::EAGAIN)),
        }
        setup.context().map(|_| ()).ok_or(Gate::NoContext)
    }

    fn tls_exchange(&mut self, c: &mut Tcp, url: &Url, req: &Req) -> Result<Session<Resp, vein_ring3::Verified>, (&'static str, i64)> {
        let setup = self.tls.as_ref().ok_or(("tls-no-context", 0))?;
        let ctx_tls = setup.context().ok_or(("tls-no-context", 0))?;
        let host = url.hostname();
        let authority = url.host_str();
        // NETDEBT2 (SR113): offer h2 first; the server's choice picks the exchange.
        let done = vein_ring3::tls::with_session_alpn(c, &host, &ctx_tls, vein_ring3::tls::ALPN_H2_HTTP1, |s, v| {
            let h2 = s.is_h2();
            let out = if h2 { exchange_h2(s, &authority, &req.target, &req.h) } else { exchange(s, &req.head, true) };
            Session { out, ver: s.version(), cipher: s.cipher(), h2, issuer: v, late: None }
        });
        match done {
            Err(f) => Err((f.why, f.errno)),
            Ok((mut s, tf)) => {
                s.late = tf.map(|f| (f.why, f.errno));
                Ok(s)
            }
        }
    }

    fn store_cookies(&mut self, url: &Url, r: &Resp, ctx: &CookieContext) {
        let unix = vein_ring3::sys::unix_time().unwrap_or(0);
        for v in r.headers.get_all("set-cookie") {
            self.jar.set_cookie_in(url, v, unix, true, ctx);
        }
    }

    fn status(&self, r: &Resp) -> u16 {
        r.status
    }

    fn next_hop(&mut self, r: &Resp, url: &Url) -> Option<Result<Url, ()>> {
        redirect::next_hop(r.status, "GET", url, r.headers.get("location")).map(|h| h.map(|h| h.url).map_err(|_| ()))
    }
}

// ---- the navigation ---------------------------------------------------------------------------------------

/// Fetch `url` (an absolute http/https URL) as a top-level navigation: `(body, final URL)` — the final URL without
/// its fragment is the page's path (its relative hrefs resolve against it) — or the failure's code. Writes the
/// `:: AETHER: net` line either way.
pub fn fetch(url: &str) -> Result<(Vec<u8>, String), i64> {
    let t0 = now_ms();
    DEADLINE.store(t0 + FETCH_MS, Ordering::Relaxed);
    vein_ring3::net::set_tick(Some(tick));
    let n = state();
    let mut io = Elf { tls: &mut n.tls, jar: &mut n.jar };
    // AETHERLIB (SR193): the model sequences the hops; the values below are the ones this line always read.
    let fetchseq::Nav { cur, out, hops, last } = fetchseq::navigate(&mut io, &mut n.seq, url);
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
    if let Some(h) = last.as_ref().filter(|h| h.tls != 0) {
        l.put(b" cipher=").put(h.cipher.as_bytes()); // AETHERNET2 (SR187)
    }
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
