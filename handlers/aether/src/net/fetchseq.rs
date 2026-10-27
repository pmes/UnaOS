//! CHARTER: Aether — shared-core (R104). AETHERLIB (LEDGER SR193): AETHER.ELF's navigation SEQUENCER as one std-free model
//! (core + alloc, html_core's URL, psl_core's site) — the resolver cache, the keep-alive pool, the hop order (request,
//! name, parked socket, fresh connect, TLS), the redirect count and the `reason=` words — linked into
//! `unaos/crates/user-aether/src/net.rs` by `#[path]` (the AETHERSCALE shape), so the host test drives the bytes the image
//! builds. Every transport (http_core's framing, codings, jar and redirect rule; vein_ring3's sockets, resolver and TLS) is
//! behind [`Io`]: the ELF implements it over its syscalls, the tests below over a stub.
//!
//! MOVED from user-aether/src/net.rs (AETHERNET SR110, AETHERNET2 SR187), not copied: `Fail`, `fail`, `is_net_scheme`, the
//! answer cache (`address`), the pool (`take_idle` -> [`Seq::take`], `park` -> [`Seq::park`]), `Hop`, `hop`, `tls_why`,
//! `site_of` and `fetch`'s redirect loop ([`navigate`]); HTTPFAIL (SR212): `http_fail`, http_core's error -> the `reason=`
//! words, its errnos read from [`Io`]. The wire line stays in net.rs and reads [`Nav`].

use alloc::string::String;
use alloc::vec::Vec;

use html_core::url::{Host, Url};

/// A parked keep-alive socket older than this is closed rather than reused (servers time idle ones out).
pub const IDLE_MS: u64 = 15_000;
/// Parked sockets across every origin.
pub const POOL_MAX: usize = 16;
/// A resolver answer is reused this long.
pub const DNS_TTL_MS: u64 = 60_000;
/// Cached resolver answers (the oldest goes).
pub const DNS_MAX: usize = 32;

/// Why a navigation did not produce a page: the wire's `reason=` and `code=`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fail {
    pub why: &'static str,
    pub code: i64,
}

pub fn fail(why: &'static str, code: i64) -> Fail {
    Fail { why, code }
}

/// A scheme this module fetches.
pub fn is_net_scheme(s: &str) -> bool {
    s == "http" || s == "https"
}

/// Why TLS cannot start (the ELF reads it from `TlsSetup::verify` / `context`; the codes are its errnos).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    NoProvider,
    NoTrustStore(i64),
    NoClock(i64),
    NoContext,
}

/// The wire's word for a [`Gate`].
pub fn gate_fail(g: Gate) -> Fail {
    match g {
        Gate::NoProvider => fail("tls-no-provider", 0),
        Gate::NoTrustStore(code) => fail("tls-no-trust-store", code),
        Gate::NoClock(code) => fail("tls-clock-unset", code),
        Gate::NoContext => fail("tls-no-context", 0),
    }
}

/// `tls-<reason>` for the wire (tls_core's names are static; the prefix is chosen from a fixed set).
pub fn tls_why(w: &'static str) -> &'static str {
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

/// HTTPFAIL (SR212): http_core's error, named for the wire (a transport errno rides `Io` as text; `I::CANCELED` is the
/// deadline). MOVED from user-aether/src/net.rs, every word and code unchanged.
pub fn http_fail<I: Io>(e: &http_core::Error) -> Fail {
    use http_core::encoding::DecodeError;
    match e {
        http_core::Error::Timeout => fail("timeout", I::CANCELED),
        http_core::Error::Io(_) => fail("io", I::EIO),
        http_core::Error::Tls(_) => fail("tls-stream", I::EPROTO),
        http_core::Error::Http(http_core::h1::H1Error::Truncated) => fail("truncated", 0),
        http_core::Error::Http(_) => fail("http-framing", 0),
        http_core::Error::Decode(DecodeError::Unsupported(_)) => fail("coding-unsupported", 0),
        http_core::Error::Decode(_) => fail("coding-corrupt", 0),
        http_core::Error::Url(_) => fail("bad-url", 0),
        http_core::Error::Redirect(_) => fail("redirect-refused", 0),
        http_core::Error::TooManyRedirects => fail("too-many-redirects", 0),
        http_core::Error::Proxy(_) => fail("proxy", 0),
    }
}

/// What one TLS session gave: the exchange's result, the version, the cipher's wire name, ALPN `h2`, the verified issuer,
/// and the session's own late failure (a transport error under the exchange).
pub struct Session<R, V> {
    pub out: Result<R, Fail>,
    pub ver: u16,
    pub cipher: &'static str,
    pub h2: bool,
    pub issuer: V,
    pub late: Option<(&'static str, i64)>,
}

/// What one hop learned beyond the response.
pub struct Hop<V> {
    pub tls: u16,
    /// AETHERNET2 (SR187): the negotiated cipher suite's wire name, "" without TLS.
    pub cipher: &'static str,
    /// The page came over HTTP/2 (ALPN `h2`); otherwise HTTP/1.1.
    pub h2: bool,
    pub reused: bool,
    pub issuer: Option<V>,
}

/// The I/O the sequencer drives. AETHER.ELF: vein_ring3 + http_core over its syscalls; the host tests: a stub.
pub trait Io {
    /// A TCP connection (dropping it closes it).
    type Conn;
    /// One encoded GET (the h1 head, the h2 target and fields).
    type Req;
    /// One answer, body still content-coded.
    type Resp;
    /// The cookie context of one hop.
    type Ctx;
    /// The verified chain's summary.
    type Issuer;
    /// The errno a wait answers once the navigation's deadline passed (-> `timeout`).
    const CANCELED: i64;
    /// The errno a transport failure reports (-> `io`).
    const EIO: i64;
    /// The errno a TLS record-layer failure reports (-> `tls-stream`).
    const EPROTO: i64;
    /// Redirects followed before `too-many-redirects`.
    const MAX_REDIRECTS: usize;
    /// HTTP/1.1 connections to one origin.
    const PER_ORIGIN: usize;
    fn now(&mut self) -> u64;
    fn resolve(&mut self, host: &str) -> Result<[u8; 4], i64>;
    fn connect(&mut self, ip: [u8; 4], port: u16) -> Result<Self::Conn, i64>;
    fn context(&mut self, cross_site: bool, partition: Option<String>) -> Self::Ctx;
    /// The request for `url` (None: it cannot be encoded).
    fn request(&mut self, url: &Url, tls: bool, ctx: &Self::Ctx) -> Option<Self::Req>;
    /// Send `req` and read one whole answer over plain HTTP/1.1: the answer and whether the connection may carry the next.
    fn exchange(&mut self, c: &mut Self::Conn, req: &Self::Req) -> Result<(Self::Resp, bool), Fail>;
    /// TLS can start (the setup loaded once, verified, its context present).
    fn tls_ready(&mut self) -> Result<(), Gate>;
    /// One TLS session over `c` carrying `req`; Err is the handshake's `(why, errno)`.
    fn tls_exchange(&mut self, c: &mut Self::Conn, url: &Url, req: &Self::Req) -> Result<Session<Self::Resp, Self::Issuer>, (&'static str, i64)>;
    fn store_cookies(&mut self, url: &Url, r: &Self::Resp, ctx: &Self::Ctx);
    fn status(&self, r: &Self::Resp) -> u16;
    /// The redirect's target: None = a final answer, Some(Err) = a redirect not followed.
    fn next_hop(&mut self, r: &Self::Resp, url: &Url) -> Option<Result<Url, ()>>;
}

struct Idle<C> {
    origin: String,
    conn: C,
    at: u64,
}

/// The sequencer's state across navigations: the answer cache, the parked sockets, the last page's site.
pub struct Seq<C> {
    dns: Vec<(String, [u8; 4], u64)>,
    pool: Vec<Idle<C>>,
    /// The registrable domain of the last page fetched (the site-for-cookies of the next navigation).
    pub site: Option<String>,
}

impl<C> Seq<C> {
    pub const fn new() -> Self {
        Seq { dns: Vec::new(), pool: Vec::new(), site: None }
    }

    /// The IPv4 address of `url`'s host: a literal as written, else the resolver (cached).
    fn address<I: Io<Conn = C>>(&mut self, io: &mut I, url: &Url) -> Result<[u8; 4], Fail> {
        match url.host() {
            Some(Host::Ipv4(v)) => Ok(v.to_be_bytes()),
            Some(Host::Domain(d)) => {
                let now = io.now();
                self.dns.retain(|(_, _, at)| now.saturating_sub(*at) < DNS_TTL_MS);
                if let Some((_, ip, _)) = self.dns.iter().find(|(h, _, _)| h == d) {
                    return Ok(*ip);
                }
                let ip = io.resolve(d).map_err(|e| fail("dns", e))?; // AETHERNET2 (SR187): the errno names why (-19 no link, -2 no answer)
                if self.dns.len() >= DNS_MAX {
                    self.dns.remove(0);
                }
                self.dns.push((d.clone(), ip, now));
                Ok(ip)
            }
            Some(Host::Ipv6(_)) => Err(fail("ipv6-unsupported", 0)),
            _ => Err(fail("no-host", 0)),
        }
    }

    /// A parked socket for `origin`, freshest first (stale ones closed on the way).
    pub fn take<I: Io<Conn = C>>(&mut self, io: &mut I, origin: &str) -> Option<C> {
        let now = io.now();
        self.pool.retain(|i| now.saturating_sub(i.at) < IDLE_MS); // dropping an Idle closes its socket
        let k = self.pool.iter().rposition(|i| i.origin == origin)?;
        Some(self.pool.remove(k).conn)
    }

    /// Park `conn` for `origin` — never past `PER_ORIGIN` for one origin (one is in flight while we fetch, so at most
    /// `PER_ORIGIN - 1` wait) nor `POOL_MAX` overall (the oldest goes).
    pub fn park<I: Io<Conn = C>>(&mut self, io: &mut I, origin: String, conn: C) {
        if self.pool.iter().filter(|i| i.origin == origin).count() + 1 >= I::PER_ORIGIN {
            return;
        }
        if self.pool.len() >= POOL_MAX {
            self.pool.remove(0);
        }
        self.pool.push(Idle { origin, conn, at: io.now() });
    }
}

/// `scheme://host:port` — the pool's key (an origin).
fn origin_key(url: &Url, port: u16) -> String {
    alloc::format!("{}://{}:{}", url.scheme(), url.hostname(), port)
}

/// psl_core's site (registrable domain, or the host itself for an IP / a bare suffix) of `url`.
fn site_of(url: &Url) -> Option<String> {
    match url.host()? {
        Host::Domain(d) => Some(String::from(psl_core::site_host(d))),
        h => Some(alloc::format!("{}", h)),
    }
}

fn connect_fail<I: Io>(e: i64) -> Fail {
    fail(if e == I::CANCELED { "timeout" } else { "connect" }, e)
}

/// One request for `url` (GET, top-level navigation) and its whole answer.
fn hop<I: Io>(io: &mut I, seq: &mut Seq<I::Conn>, url: &Url, ctx: &I::Ctx) -> Result<(I::Resp, Hop<I::Issuer>), Fail> {
    let tls = url.scheme() == "https";
    let port = url.port_or_default().unwrap_or(if tls { 443 } else { 80 });
    let req = io.request(url, tls, ctx).ok_or(fail("bad-request", 0))?;
    let ip = seq.address(io, url)?;

    if !tls {
        let origin = origin_key(url, port);
        // A parked socket first; a reused one the server already closed fails before any answer: retry once, fresh.
        if let Some(mut c) = seq.take(io, &origin) {
            if let Ok((r, keep)) = io.exchange(&mut c, &req) {
                if keep {
                    seq.park(io, origin, c);
                }
                return Ok((r, Hop { tls: 0, cipher: "", h2: false, reused: true, issuer: None }));
            }
        }
        let mut c = io.connect(ip, port).map_err(connect_fail::<I>)?;
        let (r, keep) = io.exchange(&mut c, &req)?;
        if keep {
            seq.park(io, origin, c);
        }
        return Ok((r, Hop { tls: 0, cipher: "", h2: false, reused: false, issuer: None }));
    }

    io.tls_ready().map_err(gate_fail)?;
    let mut c = io.connect(ip, port).map_err(connect_fail::<I>)?;
    match io.tls_exchange(&mut c, url, &req) {
        Err((why, errno)) => Err(fail(tls_why(why), errno)),
        Ok(Session { out: Ok(r), ver, cipher, h2, issuer, .. }) => Ok((r, Hop { tls: ver, cipher, h2, reused: false, issuer: Some(issuer) })),
        Ok(Session { out: Err(e), late: Some((why, errno)), .. }) if e.why != "timeout" => Err(fail(tls_why(why), errno)),
        Ok(Session { out: Err(e), .. }) => Err(e),
    }
}

/// One navigation's outcome: the final URL (or the parse failure), the answer (or why not), the redirects followed and
/// the last hop that answered — what the wire line reads.
pub struct Nav<R, V> {
    pub cur: Result<Url, Fail>,
    pub out: Result<R, Fail>,
    pub hops: usize,
    pub last: Option<Hop<V>>,
}

/// Fetch `url` (an absolute http/https URL) as a top-level navigation, following redirects.
pub fn navigate<I: Io>(io: &mut I, seq: &mut Seq<I::Conn>, url: &str) -> Nav<I::Resp, I::Issuer> {
    let mut cur = match Url::parse(url) {
        Ok(u) if is_net_scheme(u.scheme()) => Ok(u),
        Ok(_) => Err(fail("not-http", 0)),
        Err(_) => Err(fail("bad-url", 0)),
    };
    let mut hops = 0usize;
    let mut last: Option<Hop<I::Issuer>> = None;
    let out: Result<I::Resp, Fail> = loop {
        let u = match &cur {
            Ok(u) => u.clone(),
            Err(f) => break Err(*f),
        };
        let target_site = site_of(&u);
        let cross_site = matches!((&seq.site, &target_site), (Some(a), Some(b)) if a != b);
        let ctx = io.context(cross_site, target_site.clone());
        let (r, h) = match hop(io, seq, &u, &ctx) {
            Ok(x) => x,
            Err(f) => break Err(f),
        };
        last = Some(h);
        io.store_cookies(&u, &r, &ctx);
        match io.next_hop(&r, &u) {
            None => {
                seq.site = target_site;
                cur = Ok(u);
                break Ok(r);
            }
            Some(Err(())) => break Err(fail("redirect-refused", io.status(&r) as i64)),
            Some(Ok(next)) => {
                hops += 1;
                if hops > I::MAX_REDIRECTS {
                    break Err(fail("too-many-redirects", hops as i64));
                }
                seq.site = target_site; // the redirecting response's site is the next hop's site-for-cookies
                cur = Ok(next);
            }
        }
    };
    Nav { cur, out, hops, last }
}

#[cfg(test)]
mod tests {
    //! AETHERLIB (SR193): the sequencer driven by a stub [`Io`] — one KAT per `reason=` word, the answer cache, the pool,
    //! and the order of calls for one page.
    use super::*;
    use alloc::format;
    use alloc::vec;

    const CANCELED: i64 = -125;

    /// A stub network: every call is logged; the knobs pick each verb's answer.
    struct Stub {
        t: u64,
        log: Vec<String>,
        next_conn: u32,
        dns: Result<[u8; 4], i64>,
        connect: Result<(), i64>,
        gate: Result<(), Gate>,
        handshake: Option<(&'static str, i64)>,
        session_err: Option<Fail>,
        late: Option<(&'static str, i64)>,
        keep: bool,
        /// Connection ids whose exchange fails (a parked socket the server closed).
        stale: Vec<u32>,
        /// Every answer redirects here (`Some(None)` = a redirect not followed).
        redirect: Option<Option<&'static str>>,
        quiet_now: bool,
    }

    fn stub() -> Stub {
        Stub {
            t: 1_000,
            log: Vec::new(),
            next_conn: 1,
            dns: Ok([10, 0, 0, 1]),
            connect: Ok(()),
            gate: Ok(()),
            handshake: None,
            session_err: None,
            late: None,
            keep: false,
            stale: Vec::new(),
            redirect: None,
            quiet_now: false,
        }
    }

    impl Io for Stub {
        type Conn = u32;
        type Req = String;
        type Resp = u16;
        type Ctx = bool;
        type Issuer = &'static str;
        const CANCELED: i64 = CANCELED;
        const EIO: i64 = -5;
        const EPROTO: i64 = -71;
        const MAX_REDIRECTS: usize = 20;
        const PER_ORIGIN: usize = 6;
        fn now(&mut self) -> u64 {
            if !self.quiet_now {
                self.log.push(String::from("now"));
            }
            self.t
        }
        fn resolve(&mut self, host: &str) -> Result<[u8; 4], i64> {
            self.log.push(format!("resolve {host}"));
            self.dns
        }
        fn connect(&mut self, ip: [u8; 4], port: u16) -> Result<u32, i64> {
            self.log.push(format!("connect {}.{}.{}.{}:{port}", ip[0], ip[1], ip[2], ip[3]));
            self.connect?;
            self.next_conn += 1;
            Ok(self.next_conn - 1)
        }
        fn context(&mut self, cross_site: bool, _partition: Option<String>) -> bool {
            self.log.push(String::from("context"));
            cross_site
        }
        fn request(&mut self, url: &Url, _tls: bool, _ctx: &bool) -> Option<String> {
            self.log.push(format!("request {}", url.hostname()));
            Some(url.hostname())
        }
        fn exchange(&mut self, c: &mut u32, _req: &String) -> Result<(u16, bool), Fail> {
            self.log.push(format!("exchange #{c}"));
            if self.stale.contains(c) {
                return Err(fail("io", -5));
            }
            Ok((if self.redirect.is_some() { 302 } else { 200 }, self.keep))
        }
        fn tls_ready(&mut self) -> Result<(), Gate> {
            self.log.push(String::from("tls_ready"));
            self.gate
        }
        fn tls_exchange(&mut self, c: &mut u32, _url: &Url, _req: &String) -> Result<Session<u16, &'static str>, (&'static str, i64)> {
            self.log.push(format!("tls_exchange #{c}"));
            if let Some(h) = self.handshake {
                return Err(h);
            }
            let out = match self.session_err {
                Some(f) => Err(f),
                None => Ok(if self.redirect.is_some() { 302 } else { 200 }),
            };
            Ok(Session { out, ver: 0x0304, cipher: "TLS_AES_128_GCM_SHA256", h2: true, issuer: "Test CA", late: self.late })
        }
        fn store_cookies(&mut self, _url: &Url, _r: &u16, _ctx: &bool) {
            self.log.push(String::from("cookies"));
        }
        fn status(&self, r: &u16) -> u16 {
            *r
        }
        fn next_hop(&mut self, _r: &u16, _url: &Url) -> Option<Result<Url, ()>> {
            self.log.push(String::from("next_hop"));
            match self.redirect? {
                Some(to) => Some(Ok(Url::parse(to).unwrap())),
                None => Some(Err(())),
            }
        }
    }

    fn go(s: &mut Stub, seq: &mut Seq<u32>, url: &str) -> Nav<u16, &'static str> {
        navigate(s, seq, url)
    }

    fn why(n: &Nav<u16, &'static str>) -> Option<(&'static str, i64)> {
        n.out.as_ref().err().map(|f| (f.why, f.code))
    }

    #[test]
    fn aetherlib_one_page_call_order_kat() {
        let mut s = stub();
        s.keep = true;
        let mut seq = Seq::new();
        let n = go(&mut s, &mut seq, "http://a.test/page");
        assert_eq!(n.out, Ok(200));
        assert_eq!(n.hops, 0);
        let h = n.last.as_ref().unwrap();
        assert_eq!((h.tls, h.cipher, h.h2, h.reused, h.issuer), (0, "", false, false, None));
        assert_eq!(s.log, vec!["context", "request a.test", "now", "resolve a.test", "now", "connect 10.0.0.1:80", "exchange #1", "now", "cookies", "next_hop"]);
        assert_eq!(seq.site.as_deref(), Some("a.test"));

        let mut s = stub();
        let mut seq = Seq::new();
        let n = go(&mut s, &mut seq, "https://b.test/");
        assert_eq!(n.out, Ok(200));
        let h = n.last.as_ref().unwrap();
        assert_eq!((h.tls, h.cipher, h.h2, h.reused, h.issuer), (0x0304, "TLS_AES_128_GCM_SHA256", true, false, Some("Test CA")));
        assert_eq!(s.log, vec!["context", "request b.test", "now", "resolve b.test", "tls_ready", "connect 10.0.0.1:443", "tls_exchange #1", "cookies", "next_hop"]);
    }

    #[test]
    fn aetherlib_reason_kat() {
        type Knob = fn(&mut Stub);
        let rows: [(&str, &str, Knob, Option<(&str, i64)>); 16] = [
            ("ok", "http://a.test/", |_| {}, None),
            ("dns", "http://a.test/", |s| s.dns = Err(-2), Some(("dns", -2))),
            ("dns no link", "https://a.test/", |s| s.dns = Err(-19), Some(("dns", -19))),
            ("refused", "http://a.test/", |s| s.connect = Err(-111), Some(("connect", -111))),
            ("timeout", "https://a.test/", |s| s.connect = Err(CANCELED), Some(("timeout", CANCELED))),
            ("no trust store", "https://a.test/", |s| s.gate = Err(Gate::NoTrustStore(-2)), Some(("tls-no-trust-store", -2))),
            ("no provider", "https://a.test/", |s| s.gate = Err(Gate::NoProvider), Some(("tls-no-provider", 0))),
            ("no clock", "https://a.test/", |s| s.gate = Err(Gate::NoClock(-11)), Some(("tls-clock-unset", -11))),
            ("redirect loop", "http://a.test/", |s| s.redirect = Some(Some("http://a.test/again")), Some(("too-many-redirects", 21))),
            ("redirect refused", "http://a.test/", |s| s.redirect = Some(None), Some(("redirect-refused", 302))),
            ("bad url", "http://[", |_| {}, Some(("bad-url", 0))),
            ("not http", "ftp://a.test/x", |_| {}, Some(("not-http", 0))),
            ("ipv6", "http://[::1]/", |_| {}, Some(("ipv6-unsupported", 0))),
            ("handshake", "https://a.test/", |s| s.handshake = Some(("cert-expired", -71)), Some(("tls-cert-expired", -71))),
            ("late overrides", "https://a.test/", |s| {
                s.session_err = Some(http_fail::<Stub>(&http_core::Error::Io(String::from("errno -5"))));
                s.late = Some(("transport", -104));
            }, Some(("tls-transport", -104))),
            ("late under timeout", "https://a.test/", |s| {
                s.session_err = Some(http_fail::<Stub>(&http_core::Error::Timeout));
                s.late = Some(("transport", -104));
            }, Some(("timeout", CANCELED))),
        ];
        for (name, url, knob, want) in rows {
            let mut s = stub();
            knob(&mut s);
            let mut seq = Seq::new();
            let n = go(&mut s, &mut seq, url);
            assert_eq!(why(&n), want, "{name}");
        }
        // The redirect loop: 20 hops followed, the 21st refused; the cookie site follows the redirecting answer.
        let mut s = stub();
        s.redirect = Some(Some("http://c.test/"));
        let mut seq = Seq::new();
        let n = go(&mut s, &mut seq, "http://a.test/");
        assert_eq!(n.hops, 21);
        assert_eq!(s.log.iter().filter(|l| l.starts_with("request")).count(), 21);
        assert_eq!(seq.site.as_deref(), Some("c.test"));
        assert!(n.cur.is_ok());
        // A parse failure is the final URL's failure too (the wire shows the URL as typed).
        let mut s = stub();
        let n = go(&mut s, &mut Seq::new(), "http://[");
        assert_eq!(n.cur.err(), Some(fail("bad-url", 0)));
        assert!(n.last.is_none() && s.log.is_empty());
    }

    #[test]
    fn aetherlib_httpfail_kat() {
        use http_core::encoding::DecodeError;
        use http_core::h1::H1Error;
        use http_core::redirect::RedirectError;
        use http_core::url::UrlError;
        use http_core::Error;
        // HTTPFAIL (SR212): one row per http_core error variant (and each split the table makes) -> the wire's word and code.
        let rows: [(Error, (&str, i64)); 14] = [
            (Error::Timeout, ("timeout", CANCELED)),
            (Error::Io(String::from("errno -104")), ("io", -5)),
            (Error::Tls(String::from("bad record mac")), ("tls-stream", -71)),
            (Error::Http(H1Error::Truncated), ("truncated", 0)),
            (Error::Http(H1Error::BadChunk), ("http-framing", 0)),
            (Error::Http(H1Error::BadStatusLine), ("http-framing", 0)),
            (Error::Decode(DecodeError::Unsupported(String::from("zstd"))), ("coding-unsupported", 0)),
            (Error::Decode(DecodeError::Inflate("bad block")), ("coding-corrupt", 0)),
            (Error::Decode(DecodeError::Brotli("bad window")), ("coding-corrupt", 0)),
            (Error::Decode(DecodeError::Source), ("coding-corrupt", 0)),
            (Error::Url(UrlError::InvalidHost), ("bad-url", 0)),
            (Error::Redirect(RedirectError::UnsupportedScheme(String::from("ftp"))), ("redirect-refused", 0)),
            (Error::TooManyRedirects, ("too-many-redirects", 0)),
            (Error::Proxy(String::from("407")), ("proxy", 0)),
        ];
        for (e, (w, code)) in &rows {
            assert_eq!(http_fail::<Stub>(e), fail(*w, *code), "{e:?}");
        }
        // The ordering: each as the TLS session's exchange error under a late transport failure — the late failure names
        // the navigation, except under a timeout (the deadline is the cause).
        for (e, _) in &rows {
            let mut s = stub();
            s.session_err = Some(http_fail::<Stub>(e));
            s.late = Some(("transport", -104));
            let n = go(&mut s, &mut Seq::new(), "https://a.test/");
            let want = if *e == Error::Timeout { ("timeout", CANCELED) } else { ("tls-transport", -104) };
            assert_eq!(why(&n), Some(want), "{e:?}");
        }
    }

    #[test]
    fn aetherlib_dns_cache_kat() {
        let mut s = stub();
        s.quiet_now = true;
        let mut seq = Seq::new();
        let resolves = |s: &Stub| s.log.iter().filter(|l| l.starts_with("resolve")).count();
        go(&mut s, &mut seq, "http://a.test/");
        s.t += DNS_TTL_MS - 1;
        go(&mut s, &mut seq, "http://a.test/two");
        assert_eq!(resolves(&s), 1, "a hit inside the TTL");
        s.t += 1;
        go(&mut s, &mut seq, "http://a.test/three");
        assert_eq!(resolves(&s), 2, "re-resolved at the TTL");
        // The cap: DNS_MAX answers, the oldest goes.
        let mut seq = Seq::new();
        for i in 0..=DNS_MAX {
            go(&mut s, &mut seq, &format!("http://h{i}.test/"));
        }
        assert_eq!(seq.dns.len(), DNS_MAX);
        assert!(seq.dns.iter().all(|(h, _, _)| h != "h0.test"));
        s.log.clear();
        go(&mut s, &mut seq, "http://h0.test/");
        assert_eq!(resolves(&s), 1, "the evicted name resolves again");
        // An IPv4 literal never asks the resolver; a failed lookup is not cached.
        s.log.clear();
        let n = go(&mut s, &mut seq, "http://192.0.2.7:8080/");
        assert_eq!(n.out, Ok(200));
        assert_eq!(resolves(&s), 0);
        assert!(s.log.contains(&String::from("connect 192.0.2.7:8080")));
        let mut s = stub();
        s.dns = Err(-2);
        let mut seq = Seq::new();
        go(&mut s, &mut seq, "http://x.test/");
        assert!(seq.dns.is_empty());
    }

    #[test]
    fn aetherlib_pool_kat() {
        let mut s = stub();
        s.quiet_now = true;
        s.keep = true;
        let mut seq = Seq::new();
        go(&mut s, &mut seq, "http://a.test/");
        s.log.clear();
        let n = go(&mut s, &mut seq, "http://a.test/next");
        assert!(n.last.as_ref().unwrap().reused, "conn=reused");
        assert!(!s.log.iter().any(|l| l.starts_with("connect")), "no connect on reuse");
        assert_eq!(s.log, vec!["context", "request a.test", "exchange #1", "cookies", "next_hop"]);
        // A parked socket the server closed: one fresh connect, conn=new.
        s.stale.push(1);
        s.log.clear();
        let n = go(&mut s, &mut seq, "http://a.test/again");
        assert!(!n.last.as_ref().unwrap().reused);
        assert_eq!(s.log, vec!["context", "request a.test", "exchange #1", "connect 10.0.0.1:80", "exchange #2", "cookies", "next_hop"]);
        // Idle past IDLE_MS: closed, not reused.
        s.t += IDLE_MS;
        s.log.clear();
        go(&mut s, &mut seq, "http://a.test/later");
        assert!(s.log.iter().any(|l| l == "connect 10.0.0.1:80"));
        // Another origin (port) never takes a.test:80's socket.
        s.log.clear();
        go(&mut s, &mut seq, "http://a.test:8080/");
        assert!(s.log.iter().any(|l| l == "connect 10.0.0.1:8080"));
        // https is never parked.
        let mut s = stub();
        s.quiet_now = true;
        let mut seq = Seq::new();
        go(&mut s, &mut seq, "https://b.test/");
        go(&mut s, &mut seq, "https://b.test/");
        assert_eq!(s.log.iter().filter(|l| l.starts_with("connect")).count(), 2);
        assert!(seq.pool.is_empty());
        // The per-origin cap: at most PER_ORIGIN - 1 wait.
        let mut s = stub();
        s.quiet_now = true;
        let mut seq: Seq<u32> = Seq::new();
        for c in 0..10 {
            seq.park(&mut s, String::from("http://a.test:80"), c);
        }
        assert_eq!(seq.pool.len(), <Stub as Io>::PER_ORIGIN - 1);
    }

    #[test]
    fn aetherlib_tls_why_kat() {
        for (w, want) in [
            ("clock-unset", "tls-clock-unset"),
            ("no-trust-anchors", "tls-no-trust-anchors"),
            ("cert-unknown-issuer", "tls-cert-unknown-issuer"),
            ("cert-expired", "tls-cert-expired"),
            ("cert-not-yet-valid", "tls-cert-not-yet-valid"),
            ("cert-name-mismatch", "tls-cert-name-mismatch"),
            ("transport", "tls-transport"),
            ("tls-no-context", "tls-no-context"),
            ("verifier-not-run", "verifier-not-run"),
        ] {
            assert_eq!(tls_why(w), want);
        }
        assert_eq!(gate_fail(Gate::NoContext), fail("tls-no-context", 0));
    }
}
