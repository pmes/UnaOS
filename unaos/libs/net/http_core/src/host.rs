//! The host transport (feature `host`, std): HTTP/1.1 over a std `TcpStream`, plain for `http://` and under
//! UnaOS's own TLS 1.3 / 1.2 (`tls_core`, crypto from CRYPTOCORE) for `https://`, every certificate verified against
//! a PEM trust store at the system clock and the host name matched (RFC 6125) — the same verifier VEINTLS runs
//! on the metal. ALPN offers `http/1.1`. HTTP CONNECT proxies (`HTTPS_PROXY`/`HTTP_PROXY`, `NO_PROXY`) are
//! honoured as every other client on the machine honours them.
//!
//! Threading: `tls_core`'s client borrows a provider that is not `Sync`, so a TLS connection cannot change
//! threads. Each connection therefore OWNS a thread: it connects, handshakes, serves one exchange at a time
//! from a job channel, and parks in the [`Agent`]'s pool between exchanges (keep-alive, RFC 9112 §9.3) until
//! it idles out. Response events (head, body chunks, end) go back through an [`EventSink`] — a std channel for
//! the blocking API here, a tokio channel in `gneiss_pal::api::http`. Content codings are decoded on that
//! thread as the body arrives (pixel_core's inflater over the live body).
//!
//! Trust: `AgentConfig::trust`, else `$SSL_CERT_FILE` (the convention OpenSSL, curl and rustls-native follow),
//! else UnaOS's staged Mozilla bundle `system/trust/roots.pem` when present, else the distribution bundle.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tls_core::cryptocore_provider::CryptoCoreProvider;
use tls_core::x509::verify::{Clock, TrustStore, WebPkiVerifier};
use tls_core::{Client as TlsClient, ClientConfig, TlsError};

use crate::conn::{Conn, Error, Transport};
use crate::cookie::CookieJar;
use crate::encoding::{self, Coding};
use crate::h1::{self, BodyDecoder, Framing, H1Error, ResponseHead};
use crate::headers::Headers;
use crate::redirect::{self, MAX_REDIRECTS};
use crate::url::{Host, Url};

// ---------------------------------------------------------------- clock and trust

/// Wall-clock seconds for certificate validity.
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }
}

/// Where the trust anchors come from.
#[derive(Clone)]
pub enum Trust {
    /// `$SSL_CERT_FILE`, else UnaOS's `system/trust/roots.pem`, else the distribution bundle.
    System,
    /// A PEM bundle at this path.
    PemFile(String),
    /// An already-parsed store.
    Store(Arc<TrustStore>),
}

const DISTRO_BUNDLES: [&str; 5] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/ssl/cert.pem",
    "/usr/local/etc/openssl/cert.pem",
];

fn load_pem(path: &str) -> Result<Arc<TrustStore>, Error> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::Tls(format!("trust bundle {path}: {e}")))?;
    let (store, report) = TrustStore::from_pem(&text);
    if report.loaded == 0 {
        return Err(Error::Tls(format!("trust bundle {path}: no certificates")));
    }
    Ok(Arc::new(store))
}

/// The system trust store, parsed once per process. Returns the store and the file it came from.
pub fn system_trust() -> Result<(Arc<TrustStore>, String), Error> {
    static SYS: OnceLock<Result<(Arc<TrustStore>, String), Error>> = OnceLock::new();
    SYS.get_or_init(|| {
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(p) = std::env::var("SSL_CERT_FILE") {
            candidates.push(p);
        }
        if let Ok(p) = std::env::var("UNAOS_TRUST_BUNDLE") {
            candidates.insert(0, p);
        }
        for base in [std::env::var("UNAOS_ROOT").ok(), Some(String::from("."))].into_iter().flatten() {
            candidates.push(format!("{base}/system/trust/roots.pem"));
        }
        candidates.extend(DISTRO_BUNDLES.iter().map(|s| s.to_string()));
        for c in candidates {
            if std::path::Path::new(&c).is_file() {
                if let Ok(s) = load_pem(&c) {
                    return Ok((s, c));
                }
            }
        }
        Err(Error::Tls("no trust bundle found (set SSL_CERT_FILE)".into()))
    })
    .clone()
}

fn resolve_trust(t: &Trust) -> Result<Arc<TrustStore>, Error> {
    match t {
        Trust::System => system_trust().map(|(s, _)| s),
        Trust::PemFile(p) => load_pem(p),
        Trust::Store(s) => Ok(s.clone()),
    }
}

/// tls_core's failure as a stable reason (the same words VEINTLS reports on the metal).
pub fn describe_tls(e: &TlsError) -> String {
    use tls_core::error::CertError;
    match e {
        TlsError::Certificate(CertError::UnknownIssuer) => "cert-unknown-issuer".into(),
        TlsError::Certificate(CertError::NameMismatch) => "cert-name-mismatch".into(),
        TlsError::Certificate(CertError::Expired) => "cert-expired".into(),
        TlsError::Certificate(CertError::NotYetValid) => "cert-not-yet-valid".into(),
        TlsError::Transport => "transport".into(),
        TlsError::UnexpectedEof => "unexpected-eof".into(),
        other => format!("{other:?}"),
    }
}

// ---------------------------------------------------------------- proxies

/// Proxy selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyMode {
    /// `HTTPS_PROXY`/`https_proxy` for https, `HTTP_PROXY`/`http_proxy` for http, minus `NO_PROXY`.
    Env,
    None,
    /// Always this proxy (`http://host:port`).
    Fixed(String),
}

fn env_any(names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
}

fn ipv4_of(s: &str) -> Option<u32> {
    s.parse::<std::net::Ipv4Addr>().ok().map(u32::from)
}

/// `NO_PROXY` matching: `*`, exact names, `.suffix` / `suffix` domain suffixes, `*.suffix`, IPv4 CIDR blocks.
pub fn no_proxy_matches(list: &str, host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    for raw in list.split(',') {
        let e = raw.trim().to_ascii_lowercase();
        if e.is_empty() {
            continue;
        }
        if e == "*" || e == host {
            return true;
        }
        if let Some((net, bits)) = e.split_once('/') {
            if let (Some(n), Ok(b), Some(h)) = (ipv4_of(net), bits.parse::<u32>(), ipv4_of(&host)) {
                let mask = if b == 0 { 0 } else { u32::MAX << (32 - b.min(32)) };
                if n & mask == h & mask {
                    return true;
                }
            }
            continue;
        }
        let suffix = e.trim_start_matches("*.").trim_start_matches('.');
        if host == suffix || host.ends_with(&format!(".{suffix}")) {
            return true;
        }
    }
    false
}

fn proxy_for(mode: &ProxyMode, url: &Url) -> Option<(String, u16)> {
    let p = match mode {
        ProxyMode::None => return None,
        ProxyMode::Fixed(p) => p.clone(),
        ProxyMode::Env => {
            let host = url.hostname();
            if let Some(np) = env_any(&["NO_PROXY", "no_proxy"]) {
                if no_proxy_matches(&np, &host) {
                    return None;
                }
            }
            if url.scheme() == "https" {
                env_any(&["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"])?
            } else {
                env_any(&["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"])?
            }
        }
    };
    let pu = Url::parse(&p).or_else(|_| Url::parse(&format!("http://{p}"))).ok()?;
    if pu.scheme() != "http" {
        return None; // https:// and socks proxies are not spoken
    }
    Some((pu.hostname(), pu.port_or_default().unwrap_or(80)))
}

// ---------------------------------------------------------------- transports

/// A std socket as a byte transport, remembering why it failed.
struct Tcp {
    s: TcpStream,
    last: Option<Error>,
}

fn io_err(e: std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Error::Timeout,
        _ => Error::Io(e.to_string()),
    }
}

impl Transport for Tcp {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        self.s.read(buf).map_err(io_err)
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), Error> {
        self.s.write_all(data).map_err(io_err)
    }
}

impl tls_core::Transport for Tcp {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
        self.s.read(buf).map_err(|e| {
            self.last = Some(io_err(e));
            TlsError::Transport
        })
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), TlsError> {
        self.s.write_all(data).map_err(|e| {
            self.last = Some(io_err(e));
            TlsError::Transport
        })
    }
}

/// A TLS session as a byte transport for the HTTP layer.
struct Tls<'a> {
    c: TlsClient<'a, Tcp>,
    pending: Vec<u8>,
    pos: usize,
}

impl Tls<'_> {
    fn err(&mut self, e: TlsError) -> Error {
        match (e, self.c.transport_mut().last.take()) {
            (TlsError::Transport, Some(io)) => io,
            (e, _) => Error::Tls(describe_tls(&e)),
        }
    }
}

impl Transport for Tls<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        while self.pos >= self.pending.len() {
            match self.c.recv() {
                Ok(Some(d)) => {
                    self.pending = d;
                    self.pos = 0;
                }
                Ok(None) => return Ok(0),
                // A peer that closes TCP without close_notify: an EOF to the HTTP framing, which still refuses
                // a truncated length-delimited or chunked body.
                Err(TlsError::UnexpectedEof) => return Ok(0),
                Err(e) => return Err(self.err(e)),
            }
        }
        let n = buf.len().min(self.pending.len() - self.pos);
        buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), Error> {
        self.c.send(data).map_err(|e| self.err(e))
    }
}

// ---------------------------------------------------------------- jobs and events

/// What a connection thread reports for one exchange.
#[derive(Debug)]
pub enum Event {
    Head(ResponseHead),
    Data(Vec<u8>),
    End(Result<(), Error>),
}

/// Where a connection thread sends events. `false` = the receiver is gone (the exchange is abandoned).
pub trait EventSink: Send {
    fn send(&mut self, ev: Event) -> bool;
}

impl EventSink for SyncSender<Event> {
    fn send(&mut self, ev: Event) -> bool {
        SyncSender::send(self, ev).is_ok()
    }
}

/// One hop as the connection thread executes it.
pub struct Job {
    pub method: String,
    pub url: Url,
    pub headers: Headers,
    pub body: Arc<Vec<u8>>,
    /// Decode gzip/deflate bodies (and drop Content-Encoding/Content-Length from the head).
    pub decode: bool,
    /// Read and discard the body of a redirect that carries a Location (the dispatcher follows it).
    pub discard_redirect_body: bool,
    pub sink: Box<dyn EventSink>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct PoolKey {
    scheme: String,
    host: String,
    port: u16,
    proxy: Option<(String, u16)>,
}

struct Idle {
    id: u64,
    tx: Sender<Job>,
}

/// Agent configuration.
#[derive(Clone)]
pub struct AgentConfig {
    /// Per-read/-write socket timeout (the whole exchange may take longer while bytes keep flowing).
    pub timeout: Option<Duration>,
    pub connect_timeout: Duration,
    /// How long a parked connection waits for its next exchange.
    pub idle_timeout: Duration,
    pub user_agent: Option<String>,
    /// 0 = do not follow redirects.
    pub max_redirects: usize,
    pub decode: bool,
    pub trust: Trust,
    pub proxy: ProxyMode,
    pub cookies: Option<Arc<Mutex<CookieJar>>>,
    /// ALPN protocols offered (default `http/1.1`).
    pub alpn: Vec<Vec<u8>>,
    /// Name → address overrides consulted before DNS (curl's `--resolve`): tests, split-horizon setups.
    pub resolve: Vec<(String, SocketAddr)>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            timeout: Some(Duration::from_secs(300)),
            connect_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(60),
            user_agent: None,
            max_redirects: MAX_REDIRECTS,
            decode: true,
            trust: Trust::System,
            proxy: ProxyMode::Env,
            cookies: None,
            alpn: vec![b"http/1.1".to_vec()],
            resolve: Vec::new(),
        }
    }
}

struct Inner {
    cfg: AgentConfig,
    pool: Mutex<HashMap<PoolKey, Vec<Idle>>>,
    next_id: Mutex<u64>,
}

/// A client: configuration, a cookie jar (optional), and a pool of keep-alive connection threads.
#[derive(Clone)]
pub struct Agent {
    inner: Arc<Inner>,
}

/// A request.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub url: Url,
    pub headers: Headers,
    pub body: Arc<Vec<u8>>,
}

impl Request {
    pub fn new(method: &str, url: Url) -> Self {
        Request { method: method.into(), url, headers: Headers::new(), body: Arc::new(Vec::new()) }
    }
}

/// What the dispatcher does after a hop's head arrived.
pub enum Next {
    /// This is the answer.
    Final,
    /// Follow: send this request next (the redirect body was discarded by the connection thread).
    Follow(Request),
}

fn now_secs() -> i64 {
    SystemClock.now()
}

impl Agent {
    pub fn new(cfg: AgentConfig) -> Self {
        Agent { inner: Arc::new(Inner { cfg, pool: Mutex::new(HashMap::new()), next_id: Mutex::new(1) }) }
    }

    pub fn config(&self) -> &AgentConfig {
        &self.inner.cfg
    }

    /// Start one hop: the request goes to a parked connection for its origin, or a new connection thread.
    /// Events arrive on `sink`. Cookies from the jar are added here.
    pub fn dispatch(&self, req: &Request, sink: Box<dyn EventSink>) {
        let cfg = &self.inner.cfg;
        let mut headers = req.headers.clone();
        if let Some(jar) = &cfg.cookies {
            if !headers.contains("cookie") {
                if let Some(c) = jar.lock().ok().and_then(|mut j| j.cookie_header(&req.url, now_secs(), true)) {
                    let _ = headers.set("Cookie", &c);
                }
            }
        }
        if let Some(ua) = &cfg.user_agent {
            if !headers.contains("user-agent") {
                let _ = headers.set("User-Agent", ua);
            }
        }
        if cfg.decode && !headers.contains("accept-encoding") {
            let _ = headers.set("Accept-Encoding", encoding::ACCEPT_ENCODING);
        }
        let job = Job {
            method: req.method.clone(),
            url: req.url.clone(),
            headers,
            body: req.body.clone(),
            decode: cfg.decode,
            discard_redirect_body: cfg.max_redirects > 0,
            sink,
        };
        let key = match pool_key(&job.url, &cfg.proxy) {
            Ok(k) => k,
            Err(e) => {
                let mut s = job.sink;
                s.send(Event::End(Err(e)));
                return;
            }
        };
        let mut job = job;
        loop {
            let idle = self.inner.pool.lock().ok().and_then(|mut p| p.get_mut(&key).and_then(|v| v.pop()));
            match idle {
                Some(conn) => match conn.tx.send(job) {
                    Ok(()) => return,
                    Err(mpsc::SendError(j)) => job = j, // that thread is gone; try the next
                },
                None => break,
            }
        }
        let id = {
            let mut n = self.inner.next_id.lock().unwrap();
            *n += 1;
            *n
        };
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name(format!("http-conn-{id}"))
            .spawn(move || connection_thread(inner, key, id, job))
            .expect("spawn connection thread");
    }

    /// After a hop's head: store its cookies, and decide whether to follow a redirect. `hops` = redirects
    /// followed so far.
    pub fn after_head(&self, req: &Request, head: &ResponseHead, hops: usize) -> Result<Next, Error> {
        let cfg = &self.inner.cfg;
        if let Some(jar) = &cfg.cookies {
            if let Ok(mut j) = jar.lock() {
                for sc in head.headers.get_all("set-cookie") {
                    j.set_cookie(&req.url, sc, now_secs(), true);
                }
            }
        }
        if cfg.max_redirects == 0 {
            return Ok(Next::Final);
        }
        match redirect::next_hop(head.status, &req.method, &req.url, head.headers.get("location")) {
            None => Ok(Next::Final),
            Some(Err(e)) => Err(Error::Redirect(e)),
            Some(Ok(hop)) => {
                if hops >= cfg.max_redirects {
                    return Err(Error::TooManyRedirects);
                }
                let mut next = req.clone();
                redirect::rewrite_headers(&mut next.headers, &hop);
                if hop.drop_body {
                    next.body = Arc::new(Vec::new());
                }
                next.method = hop.method;
                next.url = hop.url;
                Ok(Next::Follow(next))
            }
        }
    }

    /// Blocking: send `req`, following redirects; the response body streams from [`Response::chunk`].
    pub fn send(&self, mut req: Request) -> Result<Response, Error> {
        let mut hops = 0;
        loop {
            let (tx, rx) = mpsc::sync_channel::<Event>(16);
            self.dispatch(&req, Box::new(tx));
            let head = match rx.recv() {
                Ok(Event::Head(h)) => h,
                Ok(Event::End(Err(e))) => return Err(e),
                Ok(_) | Err(_) => return Err(Error::Io("connection thread ended without a response".into())),
            };
            match self.after_head(&req, &head, hops)? {
                Next::Final => return Ok(Response { head, url: req.url, rx, done: false }),
                Next::Follow(next) => {
                    // Wait for the discarded body to finish so the connection is parked before reuse.
                    while let Ok(ev) = rx.recv() {
                        if matches!(ev, Event::End(_)) {
                            break;
                        }
                    }
                    req = next;
                    hops += 1;
                }
            }
        }
    }

    pub fn get(&self, url: &str) -> Result<Response, Error> {
        self.send(Request::new("GET", Url::parse(url)?))
    }

    /// Parked connections, all origins (for tests and diagnostics).
    pub fn idle_connections(&self) -> usize {
        self.inner.pool.lock().map(|p| p.values().map(Vec::len).sum()).unwrap_or(0)
    }
}

/// A blocking response.
pub struct Response {
    pub head: ResponseHead,
    /// The final URL (after redirects).
    pub url: Url,
    rx: Receiver<Event>,
    done: bool,
}

impl Response {
    pub fn status(&self) -> u16 {
        self.head.status
    }
    pub fn headers(&self) -> &Headers {
        &self.head.headers
    }
    /// The next decoded body chunk; `None` at the end.
    pub fn chunk(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if self.done {
            return Ok(None);
        }
        match self.rx.recv() {
            Ok(Event::Data(d)) => Ok(Some(d)),
            Ok(Event::End(r)) => {
                self.done = true;
                r.map(|_| None)
            }
            Ok(Event::Head(_)) | Err(_) => {
                self.done = true;
                Err(Error::Io("connection thread ended mid-body".into()))
            }
        }
    }
    pub fn bytes(mut self) -> Result<Vec<u8>, Error> {
        let mut all = Vec::new();
        while let Some(c) = self.chunk()? {
            all.extend_from_slice(&c);
        }
        Ok(all)
    }
    pub fn text(self) -> Result<String, Error> {
        self.bytes().map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

// ---------------------------------------------------------------- the connection thread

fn pool_key(url: &Url, proxy: &ProxyMode) -> Result<PoolKey, Error> {
    let scheme = url.scheme().to_string();
    if scheme != "http" && scheme != "https" {
        return Err(Error::Io(format!("unsupported scheme {scheme}")));
    }
    let host = match url.host() {
        Some(Host::Domain(d)) => d.clone(),
        Some(h @ (Host::Ipv4(_) | Host::Ipv6(_))) => h.to_string(),
        _ => return Err(Error::Url(crate::url::UrlError::HostMissing)),
    };
    let port = url.port_or_default().unwrap_or(80);
    Ok(PoolKey { scheme, host, port, proxy: proxy_for(proxy, url) })
}

fn tcp_connect(host: &str, port: u16, timeout: Duration, resolve: &[(String, SocketAddr)]) -> Result<TcpStream, Error> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let pinned: Vec<SocketAddr> = resolve.iter().filter(|(h, _)| h.eq_ignore_ascii_case(bare)).map(|(_, a)| *a).collect();
    let addrs: Vec<SocketAddr> = if !pinned.is_empty() {
        pinned
    } else {
        (bare, port).to_socket_addrs().map_err(|e| Error::Io(format!("resolve {bare}: {e}")))?.collect()
    };
    let mut last = Error::Io(format!("resolve {bare}: no addresses"));
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = io_err(e),
        }
    }
    Err(last)
}

/// RFC 9110 §9.3.6 CONNECT through an HTTP proxy.
fn tunnel(t: &mut Tcp, host: &str, port: u16) -> Result<(), Error> {
    let authority = format!("{host}:{port}");
    let head = h1::encode_request_head("CONNECT", &authority, &authority, &Headers::new(), None)?;
    let mut c = Conn::new(&mut *t);
    c.send(&head, b"")?;
    let (h, _) = c.read_head("CONNECT")?;
    if !(200..300).contains(&h.status) {
        return Err(Error::Proxy(format!("CONNECT {authority} answered {} {}", h.status, h.reason)));
    }
    if c.buffered() != 0 {
        return Err(Error::Proxy("proxy sent bytes before the tunnel opened".into()));
    }
    Ok(())
}

fn connection_thread(inner: Arc<Inner>, key: PoolKey, id: u64, first: Job) {
    let cfg = inner.cfg.clone();
    let fail = |job: Job, e: Error| {
        let mut s = job.sink;
        s.send(Event::End(Err(e)));
    };
    let (dial_host, dial_port) = key.proxy.clone().unwrap_or((key.host.clone(), key.port));
    let tcp = match tcp_connect(&dial_host, dial_port, cfg.connect_timeout, &cfg.resolve) {
        Ok(s) => s,
        Err(e) => return fail(first, e),
    };
    let _ = tcp.set_read_timeout(cfg.timeout);
    let _ = tcp.set_write_timeout(cfg.timeout);
    let mut t = Tcp { s: tcp, last: None };
    if key.scheme == "https" && key.proxy.is_some() {
        if let Err(e) = tunnel(&mut t, &key.host, key.port) {
            return fail(first, e);
        }
    }
    if key.scheme == "https" {
        let store = match resolve_trust(&cfg.trust) {
            Ok(s) => s,
            Err(e) => return fail(first, e),
        };
        let provider = CryptoCoreProvider::new();
        let verifier = WebPkiVerifier { store: &store, clock: &SystemClock };
        let name = key.host.trim_start_matches('[').trim_end_matches(']').to_string();
        let mut tcfg = ClientConfig::new(Some(&name), &verifier);
        tcfg.enable_tls12(); // TLSCORE2 (SR58): TLS 1.3 preferred, 1.2 (ECDHE + AEAD, EMS) for the long tail
        tcfg.alpn = cfg.alpn.clone();
        let client = match TlsClient::connect(&provider, &tcfg, t) {
            Ok(c) => c,
            Err(e) => {
                return fail(first, Error::Tls(describe_tls(&e)));
            }
        };
        let alpn_h2 = client.negotiated().alpn.as_deref() == Some(b"h2");
        let tls = Tls { c: client, pending: Vec::new(), pos: 0 };
        #[cfg(feature = "h2")]
        if alpn_h2 {
            let mut h2c = match crate::h2::H2Conn::handshake(tls) {
                Ok(c) => c,
                Err(e) => return fail(first, e),
            };
            serve(&inner, &key, id, first, |job, reused| exchange_h2(&mut h2c, &key, job, reused));
            let _ = h2c.transport_mut().c.close();
            return;
        }
        #[cfg(not(feature = "h2"))]
        let _ = alpn_h2;
        let mut conn = Conn::new(tls);
        serve(&inner, &key, id, first, |job, reused| exchange(&mut conn, &key, job, reused));
        let _ = conn.transport.c.close();
    } else {
        let mut conn = Conn::new(t);
        serve(&inner, &key, id, first, |job, reused| exchange(&mut conn, &key, job, reused));
    }
}

/// Serve jobs on one connection until it cannot be reused or idles out.
fn serve(inner: &Arc<Inner>, key: &PoolKey, id: u64, first: Job, mut run: impl FnMut(Job, bool) -> Outcome) {
    let (tx, rx) = mpsc::channel::<Job>();
    let mut job = first;
    let mut reused = false;
    loop {
        let outcome = run(job, reused);
        let job_back = match outcome {
            Outcome::Reusable(finish) => {
                // Park BEFORE telling the caller the exchange ended, so its next request finds this connection.
                if let Ok(mut p) = inner.pool.lock() {
                    p.entry(key.clone()).or_default().push(Idle { id, tx: tx.clone() });
                }
                if let Some((mut sink, r)) = finish {
                    sink.send(Event::End(r));
                }
                None
            }
            Outcome::Closed => return,
            Outcome::RetryFresh(j) => Some(j),
        };
        if let Some(j) = job_back {
            // A parked connection the server had already closed: hand the job to a fresh connection.
            Agent { inner: inner.clone() }.dispatch_job(j);
            return;
        }
        let deadline = Instant::now() + inner.cfg.idle_timeout;
        job = loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(j) => break j,
                Err(RecvTimeoutError::Timeout) => {
                    let still_parked = inner
                        .pool
                        .lock()
                        .map(|mut p| {
                            let v = p.entry(key.clone()).or_default();
                            let before = v.len();
                            v.retain(|i| i.id != id);
                            before != v.len()
                        })
                        .unwrap_or(true);
                    if still_parked {
                        return;
                    }
                    // A dispatcher took us just now: its job is on the way.
                    match rx.recv() {
                        Ok(j) => break j,
                        Err(_) => return,
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };
        reused = true;
    }
}

impl Agent {
    /// Re-dispatch an already-prepared job on a fresh connection (cookies/UA already applied).
    fn dispatch_job(&self, job: Job) {
        let key = match pool_key(&job.url, &self.inner.cfg.proxy) {
            Ok(k) => k,
            Err(e) => {
                let mut s = job.sink;
                s.send(Event::End(Err(e)));
                return;
            }
        };
        let id = {
            let mut n = self.inner.next_id.lock().unwrap();
            *n += 1;
            *n
        };
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name(format!("http-conn-{id}"))
            .spawn(move || connection_thread(inner, key, id, job))
            .expect("spawn connection thread");
    }
}

type Finish = (Box<dyn EventSink>, Result<(), Error>);

enum Outcome {
    /// Reusable; the End event is delivered after the connection is parked.
    Reusable(Option<Finish>),
    Closed,
    /// The connection was reused and failed before one response byte: retry on a fresh one.
    RetryFresh(Job),
}

/// Batches decoded bytes into chunks for the sink.
struct ChunkSink<'a> {
    sink: &'a mut dyn EventSink,
    buf: Vec<u8>,
    gone: bool,
}

impl ChunkSink<'_> {
    fn flush(&mut self) {
        if !self.buf.is_empty() && !self.gone {
            let d = core::mem::take(&mut self.buf);
            self.gone = !self.sink.send(Event::Data(d));
        }
    }
}

impl pixel_core::inflate::Sink for ChunkSink<'_> {
    fn push(&mut self, b: u8) -> Result<(), ()> {
        self.buf.push(b);
        if self.buf.len() >= 16 * 1024 {
            self.flush();
        }
        if self.gone { Err(()) } else { Ok(()) }
    }
}

fn exchange<T: Transport>(conn: &mut Conn<T>, key: &PoolKey, mut job: Job, reused: bool) -> Outcome {
    let via_proxy_plain = key.scheme == "http" && key.proxy.is_some();
    let target = if via_proxy_plain { job.url.serialize(true) } else { job.url.request_target() };
    let host = job.url.host_str();
    let body = job.body.clone();
    let has_body = !body.is_empty() || matches!(job.method.as_str(), "POST" | "PUT" | "PATCH");
    let head = match h1::encode_request_head(&job.method, &target, &host, &job.headers, has_body.then_some(body.len())) {
        Ok(h) => h,
        Err(e) => {
            job.sink.send(Event::End(Err(e.into())));
            return Outcome::Closed;
        }
    };
    if let Err(e) = conn.send(&head, &body) {
        if reused {
            return Outcome::RetryFresh(job);
        }
        job.sink.send(Event::End(Err(e)));
        return Outcome::Closed;
    }
    let (rhead, framing) = match conn.read_head(&job.method) {
        Ok(v) => v,
        Err(Error::Http(H1Error::Truncated)) if reused => return Outcome::RetryFresh(job),
        Err(e) => {
            job.sink.send(Event::End(Err(e)));
            return Outcome::Closed;
        }
    };
    let said_close = job.headers.has_token("connection", "close");
    let reusable = h1::keep_alive(&rhead, framing, said_close);
    let mut dec = BodyDecoder::new(framing);
    let small_redirect = matches!(framing, Framing::None | Framing::Length(0..=65536) | Framing::Chunked);
    let (clean, end) = deliver(&mut job, rhead, small_redirect, &mut || conn.read_body(&mut dec));
    finish(job, end, clean && reusable && dec.is_done() && conn.buffered() == 0)
}

/// One exchange as an HTTP/2 stream on a negotiated `h2` connection.
#[cfg(feature = "h2")]
fn exchange_h2<T: Transport>(h2c: &mut crate::h2::H2Conn<T>, key: &PoolKey, mut job: Job, reused: bool) -> Outcome {
    let body = job.body.clone();
    let authority = job.url.host_str();
    let id = match h2c.send_request(&job.method, &key.scheme, &authority, &job.url.request_target(), &job.headers, &body) {
        Ok(id) => id,
        Err(e) => {
            if reused {
                return Outcome::RetryFresh(job);
            }
            job.sink.send(Event::End(Err(e)));
            return Outcome::Closed;
        }
    };
    let (status, headers) = match h2c.read_head(id) {
        Ok(v) => v,
        Err(Error::Http(H1Error::Truncated)) if reused => return Outcome::RetryFresh(job),
        Err(e) => {
            job.sink.send(Event::End(Err(e)));
            return Outcome::Closed;
        }
    };
    let rhead = ResponseHead { version: (2, 0), status, reason: String::new(), headers };
    let no_body = job.method.eq_ignore_ascii_case("HEAD") || status == 204 || status == 304;
    let mut ended = no_body;
    let (clean, end) = deliver(&mut job, rhead, true, &mut || {
        if ended {
            return Ok(None);
        }
        let r = h2c.read_data(id);
        if matches!(r, Ok(None)) {
            ended = true;
        }
        r
    });
    if !ended {
        // An abandoned body: cancel the stream (RFC 9113 §8.1), the connection stays usable.
        let _ = h2c.transport_mut().write_all(&crate::h2::frame::encode(
            crate::h2::frame::RST_STREAM,
            0,
            id,
            &crate::h2::frame::ErrorCode::Cancel.to_u32().to_be_bytes(),
        ));
    }
    h2c.close_stream(id);
    let ok = clean && h2c.reusable();
    finish(job, end, ok)
}

/// A pull of body chunks as an inflater source.
struct PullSource<'a> {
    pull: &'a mut dyn FnMut() -> Result<Option<Vec<u8>>, Error>,
    chunk: Vec<u8>,
    pos: usize,
    error: Option<Error>,
    done: bool,
}

impl pixel_core::inflate::ByteSource for PullSource<'_> {
    fn next(&mut self) -> Option<u8> {
        while self.pos >= self.chunk.len() {
            if self.done || self.error.is_some() {
                return None;
            }
            match (self.pull)() {
                Ok(Some(c)) => {
                    self.chunk = c;
                    self.pos = 0;
                }
                Ok(None) => {
                    self.done = true;
                    return None;
                }
                Err(e) => {
                    self.error = Some(e);
                    return None;
                }
            }
        }
        let b = self.chunk[self.pos];
        self.pos += 1;
        Some(b)
    }
}

/// Send the head and stream the body to the job's sink (content codings decoded, followed redirects
/// drained). Returns whether the message was consumed cleanly to its end.
/// The End event: deferred until the connection is parked when it is reusable, sent now otherwise.
fn finish(job: Job, end: Option<Result<(), Error>>, reusable: bool) -> Outcome {
    let mut sink = job.sink;
    if reusable {
        return Outcome::Reusable(end.map(|r| (sink, r)));
    }
    if let Some(r) = end {
        sink.send(Event::End(r));
    }
    Outcome::Closed
}

/// Returns (consumed cleanly to the end, the End result still to deliver — `None` when the receiver is gone).
fn deliver(job: &mut Job, mut rhead: ResponseHead, small_redirect: bool, pull: &mut dyn FnMut() -> Result<Option<Vec<u8>>, Error>) -> (bool, Option<Result<(), Error>>) {
    // A followed redirect: drain (small) bodies so the connection stays usable.
    if job.discard_redirect_body && redirect::is_redirect(rhead.status) && rhead.headers.contains("location") {
        let sink_ok = job.sink.send(Event::Head(rhead));
        let mut clean = false;
        if small_redirect {
            let mut total = 0usize;
            loop {
                match pull() {
                    Ok(Some(c)) => {
                        total += c.len();
                        if total > 65536 {
                            break;
                        }
                    }
                    Ok(None) => {
                        clean = true;
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
        return (clean, sink_ok.then_some(Ok(())));
    }
    let codings = if job.decode { encoding::content_codings(&rhead.headers) } else { Vec::new() };
    let decodable = codings.len() == 1 && matches!(codings[0], Coding::Gzip | Coding::Deflate);
    if decodable {
        rhead.headers.remove("content-encoding");
        rhead.headers.remove("content-length");
    }
    if !job.sink.send(Event::Head(rhead)) {
        return (false, None);
    }
    let result: Result<(), Error> = if decodable {
        let mut cs = ChunkSink { sink: &mut *job.sink, buf: Vec::new(), gone: false };
        let mut src = PullSource { pull: &mut *pull, chunk: Vec::new(), pos: 0, error: None, done: false };
        let r = encoding::decode_stream(&codings[0], &mut src, &mut cs);
        let (src_err, src_done) = (src.error.take(), src.done);
        cs.flush();
        if cs.gone {
            return (false, None);
        }
        match (src_err, r) {
            (Some(e), _) => Err(e),
            (None, Err(e)) => Err(Error::Decode(e)),
            (None, Ok(_)) if src_done => Ok(()),
            (None, Ok(_)) => {
                // Consume whatever framing remains (the chunked terminator, trailers, END_STREAM).
                loop {
                    match pull() {
                        Ok(Some(_)) => {}
                        Ok(None) => break Ok(()),
                        Err(e) => break Err(e),
                    }
                }
            }
        }
    } else {
        loop {
            match pull() {
                Ok(Some(c)) => {
                    if !job.sink.send(Event::Data(c)) {
                        return (false, None);
                    }
                }
                Ok(None) => break Ok(()),
                Err(e) => break Err(e),
            }
        }
    };
    (result.is_ok(), Some(result))
}
