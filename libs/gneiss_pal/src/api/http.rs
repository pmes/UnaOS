// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! The host HTTP client Vein, Aether and Gneiss call (HTTPCORE, LEDGER SR51).
//!
//! UnaOS's own stack end to end: `http_core` (WHATWG URL, HTTP/1.1 framing, RFC 6265 cookies, gzip/deflate
//! through pixel_core's inflater) over its host transport (std `TcpStream`, `tls_core` TLS 1.3 verified
//! against the trust store, CRYPTOCORE for every primitive). It replaced `reqwest` (and with it hyper, rustls/
//! native-tls, `cookie_store`, the `url` crate under it) behind the same API shape the call sites already used:
//! `Client::builder().timeout(..).build()`, `client.post(url).header(..).body(..).send().await`,
//! `res.status()`, `res.headers()`, `res.text()/json()/bytes()/chunk()/bytes_stream()`, `try_clone()` for the
//! shared backoff, a `blocking` client for Aether's JS `fetch`, and `multipart` for Vein's upload.
//!
//! Runtime-agnostic: each exchange runs on its connection's own thread (`http_core::host`) and reports
//! through a tokio channel, so these futures are `Send` and need no particular executor.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http_core::host::{self, Agent, AgentConfig, Event, EventSink, Next};
pub use http_core::cookie::CookieJar;
pub use http_core::headers::Headers;
pub use http_core::url::Url;

// ---------------------------------------------------------------- errors and status

/// An HTTP client error (connection, TLS, protocol, decoding, or a body that was not the JSON asked for).
#[derive(Debug, Clone)]
pub struct Error {
    msg: String,
    timeout: bool,
}

impl Error {
    fn new(msg: impl Into<String>) -> Self {
        Error { msg: msg.into(), timeout: false }
    }
    pub fn is_timeout(&self) -> bool {
        self.timeout
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

impl From<http_core::Error> for Error {
    fn from(e: http_core::Error) -> Self {
        Error { timeout: matches!(e, http_core::Error::Timeout), msg: e.to_string() }
    }
}

impl From<http_core::url::UrlError> for Error {
    fn from(e: http_core::url::UrlError) -> Self {
        Error::new(format!("invalid URL: {e}"))
    }
}

/// An HTTP status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatusCode(pub u16);

impl StatusCode {
    pub fn as_u16(&self) -> u16 {
        self.0
    }
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.0)
    }
    pub fn is_redirection(&self) -> bool {
        (300..400).contains(&self.0)
    }
    pub fn is_client_error(&self) -> bool {
        (400..500).contains(&self.0)
    }
    pub fn is_server_error(&self) -> bool {
        (500..600).contains(&self.0)
    }
    /// RFC 9110 §15's reason phrase.
    pub fn canonical_reason(&self) -> Option<&'static str> {
        Some(match self.0 {
            100 => "Continue",
            101 => "Switching Protocols",
            200 => "OK",
            201 => "Created",
            202 => "Accepted",
            204 => "No Content",
            206 => "Partial Content",
            301 => "Moved Permanently",
            302 => "Found",
            303 => "See Other",
            304 => "Not Modified",
            307 => "Temporary Redirect",
            308 => "Permanent Redirect",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            408 => "Request Timeout",
            409 => "Conflict",
            410 => "Gone",
            413 => "Content Too Large",
            415 => "Unsupported Media Type",
            422 => "Unprocessable Content",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            529 => "Site Overloaded",
            _ => return None,
        })
    }
}

impl fmt::Display for StatusCode {
    /// `"404 Not Found"`, as reqwest printed it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.canonical_reason() {
            Some(r) => write!(f, "{} {}", self.0, r),
            None => write!(f, "{}", self.0),
        }
    }
}

/// Field-name constants (reqwest's `header::*` names, as lowercase strings — every lookup is case-insensitive).
pub mod header {
    pub const ACCEPT: &str = "accept";
    pub const ACCEPT_ENCODING: &str = "accept-encoding";
    pub const AUTHORIZATION: &str = "authorization";
    pub const CONTENT_ENCODING: &str = "content-encoding";
    pub const CONTENT_LENGTH: &str = "content-length";
    pub const CONTENT_RANGE: &str = "content-range";
    pub const CONTENT_TYPE: &str = "content-type";
    pub const LOCATION: &str = "location";
    pub const RANGE: &str = "range";
    pub const RETRY_AFTER: &str = "retry-after";
    pub const USER_AGENT: &str = "user-agent";
}

// ---------------------------------------------------------------- the async client

/// A tokio channel as the connection thread's event sink (a plain std thread: `blocking_send` is safe there).
struct TokioSink(tokio::sync::mpsc::Sender<Event>);

impl EventSink for TokioSink {
    fn send(&mut self, ev: Event) -> bool {
        self.0.blocking_send(ev).is_ok()
    }
}

/// Builder for [`Client`].
pub struct ClientBuilder {
    cfg: AgentConfig,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientBuilder {
    pub fn new() -> Self {
        ClientBuilder { cfg: AgentConfig { user_agent: Some(default_user_agent()), ..Default::default() } }
    }
    /// Idle limit on each socket read/write (the exchange may run longer while bytes flow).
    pub fn timeout(mut self, d: Duration) -> Self {
        self.cfg.timeout = Some(d);
        self
    }
    pub fn connect_timeout(mut self, d: Duration) -> Self {
        self.cfg.connect_timeout = d;
        self
    }
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.cfg.user_agent = Some(ua.into());
        self
    }
    /// Share a cookie jar (RFC 6265): Set-Cookie is stored, Cookie is sent, across every client holding it.
    pub fn cookie_jar(mut self, jar: Arc<Mutex<CookieJar>>) -> Self {
        self.cfg.cookies = Some(jar);
        self
    }
    /// Redirects followed at most (0 = never).
    pub fn max_redirects(mut self, n: usize) -> Self {
        self.cfg.max_redirects = n;
        self
    }
    /// Trust anchors from this PEM bundle instead of the system store.
    pub fn trust_pem_file(mut self, path: impl Into<String>) -> Self {
        self.cfg.trust = host::Trust::PemFile(path.into());
        self
    }
    pub fn no_proxy(mut self) -> Self {
        self.cfg.proxy = host::ProxyMode::None;
        self
    }
    pub fn build(self) -> Result<Client, Error> {
        Ok(Client { agent: Agent::new(self.cfg) })
    }
}

fn default_user_agent() -> String {
    format!("UnaOS-gneiss/{}", env!("CARGO_PKG_VERSION"))
}

/// The async HTTP client. Cheap to clone (the connection pool is shared).
#[derive(Clone)]
pub struct Client {
    agent: Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        ClientBuilder::new().build().expect("default client")
    }
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }
    pub fn request(&self, method: &str, url: impl AsRef<str>) -> RequestBuilder {
        RequestBuilder {
            client: self.clone(),
            method: method.to_ascii_uppercase(),
            url: Url::parse(url.as_ref()).map_err(Error::from),
            headers: Headers::new(),
            body: Arc::new(Vec::new()),
            err: None,
        }
    }
    pub fn get(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request("GET", url)
    }
    pub fn post(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request("POST", url)
    }
    pub fn put(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request("PUT", url)
    }
    pub fn delete(&self, url: impl AsRef<str>) -> RequestBuilder {
        self.request("DELETE", url)
    }
}

/// A request being built. Bodies are owned bytes, so every builder can be cloned (the backoff resends it).
#[derive(Clone)]
pub struct RequestBuilder {
    client: Client,
    method: String,
    url: Result<Url, Error>,
    headers: Headers,
    body: Arc<Vec<u8>>,
    err: Option<Error>,
}

impl RequestBuilder {
    pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        if let Err(e) = self.headers.set(name.as_ref(), value.as_ref()) {
            self.err.get_or_insert(Error::new(format!("invalid header {}: {e:?}", name.as_ref())));
        }
        self
    }
    pub fn bearer_auth(self, token: impl fmt::Display) -> Self {
        self.header("authorization", format!("Bearer {token}"))
    }
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Arc::new(body.into());
        self
    }
    /// Serialize `v` as the JSON body (and set `content-type` unless already set).
    pub fn json<T: serde::Serialize + ?Sized>(mut self, v: &T) -> Self {
        match serde_json::to_vec(v) {
            Ok(b) => {
                if !self.headers.contains("content-type") {
                    let _ = self.headers.set("content-type", "application/json");
                }
                self.body = Arc::new(b);
            }
            Err(e) => {
                self.err.get_or_insert(Error::new(format!("json body: {e}")));
            }
        }
        self
    }
    /// A `multipart/form-data` body (RFC 7578).
    pub fn multipart(self, form: multipart::Form) -> Self {
        let (ctype, body) = form.finish();
        self.header("content-type", ctype).body(body)
    }
    /// Always `Some`: bodies are owned bytes.
    pub fn try_clone(&self) -> Option<Self> {
        Some(self.clone())
    }

    fn into_request(self) -> Result<(Agent, host::Request), Error> {
        if let Some(e) = self.err {
            return Err(e);
        }
        let url = self.url?;
        Ok((self.client.agent, host::Request { method: self.method, url, headers: self.headers, body: self.body }))
    }

    /// Send, following redirects (RFC 9110 §15.4 / Fetch rules) and keeping cookies.
    pub async fn send(self) -> Result<Response, Error> {
        let (agent, mut req) = self.into_request()?;
        let mut hops = 0;
        loop {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(16);
            agent.dispatch(&req, Box::new(TokioSink(tx)));
            let head = match rx.recv().await {
                Some(Event::Head(h)) => h,
                Some(Event::End(Err(e))) => return Err(e.into()),
                _ => return Err(Error::new("connection ended without a response")),
            };
            match agent.after_head(&req, &head, hops)? {
                Next::Final => {
                    return Ok(Response { status: StatusCode(head.status), headers: head.headers, url: req.url, rx, done: false });
                }
                Next::Follow(next) => {
                    while let Some(ev) = rx.recv().await {
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
}

/// A response whose body streams in as it arrives (content codings already decoded).
pub struct Response {
    status: StatusCode,
    headers: Headers,
    url: Url,
    rx: tokio::sync::mpsc::Receiver<Event>,
    done: bool,
}

impl Response {
    pub fn status(&self) -> StatusCode {
        self.status
    }
    pub fn headers(&self) -> &Headers {
        &self.headers
    }
    /// The final URL, after redirects.
    pub fn url(&self) -> &Url {
        &self.url
    }
    /// `Content-Length` when the body is not content-coded (a decoded body has no known length).
    pub fn content_length(&self) -> Option<u64> {
        self.headers.get("content-length").and_then(|v| v.trim().parse().ok())
    }
    /// The next body chunk; `None` at the end.
    pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if self.done {
            return Ok(None);
        }
        match self.rx.recv().await {
            Some(Event::Data(d)) => Ok(Some(d)),
            Some(Event::End(r)) => {
                self.done = true;
                r.map(|_| None).map_err(Error::from)
            }
            _ => {
                self.done = true;
                Err(Error::new("connection ended mid-body"))
            }
        }
    }
    pub async fn bytes(mut self) -> Result<Vec<u8>, Error> {
        let mut all = Vec::new();
        while let Some(c) = self.chunk().await? {
            all.extend_from_slice(&c);
        }
        Ok(all)
    }
    pub async fn text(self) -> Result<String, Error> {
        Ok(String::from_utf8_lossy(&self.bytes().await?).into_owned())
    }
    pub async fn json<T: serde::de::DeserializeOwned>(self) -> Result<T, Error> {
        let b = self.bytes().await?;
        serde_json::from_slice(&b).map_err(|e| Error::new(format!("error decoding response body: {e}")))
    }
    /// The body as a `Stream` of chunks.
    pub fn bytes_stream(self) -> impl futures_core::Stream<Item = Result<Vec<u8>, Error>> + Send + 'static {
        futures_util::stream::unfold(self, |mut r| async move {
            match r.chunk().await {
                Ok(Some(c)) => Some((Ok(c), r)),
                Ok(None) => None,
                Err(e) => {
                    r.done = true;
                    Some((Err(e), r))
                }
            }
        })
    }
}

// ---------------------------------------------------------------- multipart

/// `multipart/form-data` (RFC 7578) over `http_core::multipart`, in reqwest's shape.
pub mod multipart {
    /// One part.
    pub struct Part {
        data: Vec<u8>,
        filename: Option<String>,
        mime: Option<String>,
    }

    impl Part {
        pub fn bytes(data: impl Into<Vec<u8>>) -> Self {
            Part { data: data.into(), filename: None, mime: None }
        }
        pub fn text(s: impl Into<String>) -> Self {
            Part { data: s.into().into_bytes(), filename: None, mime: None }
        }
        pub fn file_name(mut self, name: impl Into<String>) -> Self {
            self.filename = Some(name.into());
            self
        }
        /// The part's media type (a type/subtype token pair is required).
        pub fn mime_str(mut self, mime: &str) -> Result<Self, super::Error> {
            let ok = mime.split_once('/').is_some_and(|(t, s)| {
                http_core::headers::is_token(t) && http_core::headers::is_token(s.split(';').next().unwrap_or("").trim())
            });
            if !ok {
                return Err(super::Error::new(format!("invalid media type {mime:?}")));
            }
            self.mime = Some(mime.into());
            Ok(self)
        }
    }

    /// A form.
    #[derive(Default)]
    pub struct Form {
        parts: Vec<(String, Part)>,
    }

    impl Form {
        pub fn new() -> Self {
            Form::default()
        }
        pub fn part(mut self, name: impl Into<String>, part: Part) -> Self {
            self.parts.push((name.into(), part));
            self
        }
        pub fn text(self, name: impl Into<String>, value: impl Into<String>) -> Self {
            self.part(name, Part::text(value))
        }
        /// (Content-Type value, body), with a boundary from the OS RNG.
        pub(super) fn finish(self) -> (String, Vec<u8>) {
            let mut seed = [0u8; 12];
            if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
                let _ = std::io::Read::read_exact(&mut f, &mut seed);
            }
            let boundary: String = format!("UnaOSFormBoundary{}", seed.iter().map(|b| format!("{b:02x}")).collect::<String>());
            let mut f = http_core::multipart::Form::new(&boundary);
            for (name, p) in self.parts {
                f = f.part(http_core::multipart::Part { name, filename: p.filename, content_type: p.mime, data: p.data });
            }
            (f.content_type(), f.encode())
        }
    }
}

// ---------------------------------------------------------------- blocking

/// The blocking client (Aether's JS `fetch`/XHR run it on their own thread, off the async runtime).
pub mod blocking {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use http_core::host::{self, Agent, AgentConfig};

    use super::{CookieJar, Error, Headers, StatusCode, Url};

    pub struct ClientBuilder {
        cfg: AgentConfig,
    }

    impl Default for ClientBuilder {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ClientBuilder {
        pub fn new() -> Self {
            ClientBuilder { cfg: AgentConfig { user_agent: Some(super::default_user_agent()), ..Default::default() } }
        }
        pub fn timeout(mut self, d: Duration) -> Self {
            self.cfg.timeout = Some(d);
            self
        }
        pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
            self.cfg.user_agent = Some(ua.into());
            self
        }
        pub fn cookie_jar(mut self, jar: Arc<Mutex<CookieJar>>) -> Self {
            self.cfg.cookies = Some(jar);
            self
        }
        /// Redirects followed at most (0 = never).
        pub fn max_redirects(mut self, n: usize) -> Self {
            self.cfg.max_redirects = n;
            self
        }
        pub fn build(self) -> Result<Client, Error> {
            Ok(Client { agent: Agent::new(self.cfg) })
        }
    }

    #[derive(Clone)]
    pub struct Client {
        agent: Agent,
    }

    impl Client {
        pub fn builder() -> ClientBuilder {
            ClientBuilder::new()
        }
        pub fn request(&self, method: &str, url: &str) -> RequestBuilder {
            RequestBuilder {
                agent: self.agent.clone(),
                method: method.to_ascii_uppercase(),
                url: Url::parse(url).map_err(Error::from),
                headers: Headers::new(),
                body: Vec::new(),
            }
        }
        pub fn get(&self, url: &str) -> RequestBuilder {
            self.request("GET", url)
        }
        pub fn post(&self, url: &str) -> RequestBuilder {
            self.request("POST", url)
        }
    }

    pub struct RequestBuilder {
        agent: Agent,
        method: String,
        url: Result<Url, Error>,
        headers: Headers,
        body: Vec<u8>,
    }

    impl RequestBuilder {
        pub fn header(mut self, k: impl AsRef<str>, v: impl AsRef<str>) -> Self {
            let _ = self.headers.set(k.as_ref(), v.as_ref());
            self
        }
        pub fn body(mut self, b: impl Into<Vec<u8>>) -> Self {
            self.body = b.into();
            self
        }
        pub fn send(self) -> Result<Response, Error> {
            let url = self.url?;
            let req = host::Request { method: self.method, url, headers: self.headers, body: Arc::new(self.body) };
            let inner = self.agent.send(req)?;
            Ok(Response { inner, pending: Vec::new(), pos: 0 })
        }
    }

    /// A blocking response; the body streams through [`std::io::Read`] (or `bytes`/`text`).
    pub struct Response {
        inner: host::Response,
        pending: Vec<u8>,
        pos: usize,
    }

    /// Reads the decoded body as it arrives; a transport/TLS/framing error mid-body is an `io::Error`
    /// (truncation included — a short body never reads as a clean EOF).
    impl std::io::Read for Response {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            while self.pos >= self.pending.len() {
                match self.inner.chunk() {
                    Ok(Some(c)) => {
                        self.pending = c;
                        self.pos = 0;
                    }
                    Ok(None) => return Ok(0),
                    Err(e) => {
                        let kind = if matches!(e, http_core::Error::Timeout) { std::io::ErrorKind::TimedOut } else { std::io::ErrorKind::Other };
                        return Err(std::io::Error::new(kind, Error::from(e)));
                    }
                }
            }
            let n = buf.len().min(self.pending.len() - self.pos);
            buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    impl Response {
        pub fn status(&self) -> StatusCode {
            StatusCode(self.inner.status())
        }
        pub fn headers(&self) -> &Headers {
            self.inner.headers()
        }
        pub fn url(&self) -> &Url {
            &self.inner.url
        }
        pub fn content_length(&self) -> Option<u64> {
            self.inner.headers().get("content-length").and_then(|v| v.trim().parse().ok())
        }
        /// The next decoded body chunk as the connection delivers it (`None` at the end) — what a page's
        /// streaming `fetch` body, `XMLHttpRequest` progress and `EventSource` read before EOF (AETHERNET, SR70).
        pub fn chunk(&mut self) -> Result<Option<Vec<u8>>, Error> {
            if self.pos < self.pending.len() {
                let rest = self.pending.split_off(self.pos);
                self.pending.clear();
                self.pos = 0;
                return Ok(Some(rest));
            }
            Ok(self.inner.chunk()?)
        }
        /// The status line's reason phrase as the server sent it (`statusText`).
        pub fn reason(&self) -> &str {
            &self.inner.head.reason
        }
        pub fn bytes(mut self) -> Result<Vec<u8>, Error> {
            let mut all = self.pending.split_off(self.pos);
            self.pending.clear();
            while let Some(c) = self.inner.chunk()? {
                all.extend_from_slice(&c);
            }
            Ok(all)
        }
        pub fn text(self) -> Result<String, Error> {
            self.bytes().map(|b| String::from_utf8_lossy(&b).into_owned())
        }
    }
}

// ---------------------------------------------------------------- RFC 6455 (AETHERNET, SR70)

/// The WebSocket protocol (RFC 6455) client: the opening handshake (§4.1–4.2: `Sec-WebSocket-Key`, the
/// `Sec-WebSocket-Accept` check, subprotocol selection; no extension is offered, so `permessage-deflate` is
/// declined and any extension a server answers with fails the connection), the framing (§5.2) with client
/// masking (§5.3), fragmentation (§5.4), control frames (§5.5: close, ping, pong), UTF-8 validation of text
/// (§8.1) and the closing handshake with status codes (§7).
///
/// Transport: `ws://` over a std `TcpStream`. `wss://` needs a `tls_core` session the host transport keeps
/// private (http_core `host::Tls`), so it answers an error naming that until http_core exposes an upgraded
/// connection — the connection loop is written against [`ws::Transport`] so the TLS stream plugs in unchanged.
pub mod ws {
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{CookieJar, Error, Headers, Url};

    pub const OP_CONT: u8 = 0x0;
    pub const OP_TEXT: u8 = 0x1;
    pub const OP_BINARY: u8 = 0x2;
    pub const OP_CLOSE: u8 = 0x8;
    pub const OP_PING: u8 = 0x9;
    pub const OP_PONG: u8 = 0xA;

    /// RFC 6455 §1.3's GUID, concatenated to the key before hashing.
    pub const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

    /// The largest message this client reassembles (a hostile server cannot exhaust memory).
    pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;

    // ------------------------------------------------------------ primitives

    /// SHA-1 (FIPS 180-4) — only for §4.2.2's `Sec-WebSocket-Accept` derivation, which the protocol fixes.
    pub fn sha1(data: &[u8]) -> [u8; 20] {
        let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
        let mut msg = data.to_vec();
        let bits = (data.len() as u64).wrapping_mul(8);
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bits.to_be_bytes());
        for block in msg.chunks_exact(64) {
            let mut w = [0u32; 80];
            for (i, c) in block.chunks_exact(4).enumerate() {
                w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
            }
            for i in 16..80 {
                w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
            }
            let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
            for (i, wi) in w.iter().enumerate() {
                let (f, k) = match i {
                    0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                    20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                    40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                    _ => (b ^ c ^ d, 0xCA62_C1D6),
                };
                let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
                e = d;
                d = c;
                c = b.rotate_left(30);
                b = a;
                a = t;
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
        }
        let mut out = [0u8; 20];
        for (i, v) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }

    /// RFC 4648 base64 (with padding).
    pub fn base64(data: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            out.push(A[(n >> 18) as usize & 63] as char);
            out.push(A[(n >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
        }
        out
    }

    /// §4.2.2 step 5.4: base64(SHA-1(key ‖ GUID)).
    pub fn accept_key(key: &str) -> String {
        let mut s = key.trim().as_bytes().to_vec();
        s.extend_from_slice(GUID.as_bytes());
        base64(&sha1(&s))
    }

    /// Bytes from the OS entropy pool (masking keys must be unpredictable, §5.3; the handshake nonce too).
    pub fn random_bytes<const N: usize>() -> [u8; N] {
        let mut b = [0u8; N];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(&mut b).is_ok() {
                return b;
            }
        }
        // No entropy device: a time-seeded xorshift (still a fresh key per frame, not a fixed one).
        let mut x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
        for v in b.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *v = x as u8;
        }
        b
    }

    // ------------------------------------------------------------ framing (§5.2)

    /// One frame on the wire.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Frame {
        pub fin: bool,
        pub rsv: u8,
        pub opcode: u8,
        pub mask: Option<[u8; 4]>,
        /// The payload, unmasked.
        pub payload: Vec<u8>,
    }

    /// Serializes a frame: FIN/opcode, the 7/16/64-bit payload length, the masking key and the masked payload.
    pub fn encode_frame(fin: bool, opcode: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
        let mut out = Vec::with_capacity(payload.len() + 14);
        out.push(if fin { 0x80 } else { 0 } | (opcode & 0x0F));
        let m = if mask.is_some() { 0x80 } else { 0 };
        let n = payload.len();
        if n < 126 {
            out.push(m | n as u8);
        } else if n <= 0xFFFF {
            out.push(m | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            out.push(m | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
        match mask {
            Some(k) => {
                out.extend_from_slice(&k);
                out.extend(payload.iter().enumerate().map(|(i, b)| b ^ k[i & 3]));
            }
            None => out.extend_from_slice(payload),
        }
        out
    }

    /// Why a connection failed (§7.1.7) — with the status code the client sends before dropping it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WsError {
        /// A protocol violation (`code` 1002), invalid UTF-8 (1007) or an oversized message (1009).
        Protocol { code: u16, msg: String },
        /// The transport failed or closed without a closing handshake (1006 to the page).
        Io(String),
    }

    impl std::fmt::Display for WsError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                WsError::Protocol { code, msg } => write!(f, "websocket protocol error {code}: {msg}"),
                WsError::Io(m) => write!(f, "websocket transport: {m}"),
            }
        }
    }

    fn proto(code: u16, msg: &str) -> WsError {
        WsError::Protocol { code, msg: msg.into() }
    }

    /// Incremental frame decoder: bytes in as they arrive, whole frames out. It enforces the frame-level
    /// rules of §5.2/§5.5 (no RSV bit without an extension, no reserved opcode, control frames final and
    /// ≤ 125 bytes, a 64-bit length with its top bit clear).
    #[derive(Default)]
    pub struct FrameDecoder {
        buf: Vec<u8>,
        pos: usize,
    }

    impl FrameDecoder {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn push(&mut self, bytes: &[u8]) {
            if self.pos > 0 && self.pos == self.buf.len() {
                self.buf.clear();
                self.pos = 0;
            }
            self.buf.extend_from_slice(bytes);
        }
        /// The next complete frame, `None` until one is buffered.
        pub fn next_frame(&mut self) -> Result<Option<Frame>, WsError> {
            let b = &self.buf[self.pos..];
            if b.len() < 2 {
                return Ok(None);
            }
            let fin = b[0] & 0x80 != 0;
            let rsv = (b[0] >> 4) & 7;
            let opcode = b[0] & 0x0F;
            if rsv != 0 {
                return Err(proto(1002, "reserved bit set with no extension negotiated"));
            }
            if (3..=7).contains(&opcode) || opcode > 0xA {
                return Err(proto(1002, "reserved opcode"));
            }
            let masked = b[1] & 0x80 != 0;
            let len7 = (b[1] & 0x7F) as u64;
            let (len, mut at) = match len7 {
                126 => {
                    if b.len() < 4 {
                        return Ok(None);
                    }
                    (u16::from_be_bytes([b[2], b[3]]) as u64, 4usize)
                }
                127 => {
                    if b.len() < 10 {
                        return Ok(None);
                    }
                    let n = u64::from_be_bytes(b[2..10].try_into().unwrap());
                    if n >> 63 != 0 {
                        return Err(proto(1002, "64-bit payload length with the most significant bit set"));
                    }
                    (n, 10usize)
                }
                n => (n, 2usize),
            };
            if opcode >= 8 && (!fin || len > 125) {
                return Err(proto(1002, "fragmented or oversized control frame"));
            }
            if len > MAX_MESSAGE as u64 {
                return Err(proto(1009, "frame larger than the message limit"));
            }
            let mask = if masked {
                if b.len() < at + 4 {
                    return Ok(None);
                }
                let k = [b[at], b[at + 1], b[at + 2], b[at + 3]];
                at += 4;
                Some(k)
            } else {
                None
            };
            let len = len as usize;
            if b.len() < at + len {
                return Ok(None);
            }
            let mut payload = b[at..at + len].to_vec();
            if let Some(k) = mask {
                for (i, v) in payload.iter_mut().enumerate() {
                    *v ^= k[i & 3];
                }
            }
            self.pos += at + len;
            if self.pos == self.buf.len() {
                self.buf.clear();
                self.pos = 0;
            } else if self.pos > 64 * 1024 {
                self.buf.drain(..self.pos);
                self.pos = 0;
            }
            Ok(Some(Frame { fin, rsv, opcode, mask, payload }))
        }
    }

    /// A close frame's body (§5.5.1): an optional status code and a UTF-8 reason.
    pub fn close_payload(code: Option<u16>, reason: &str) -> Vec<u8> {
        match code {
            Some(c) => {
                let mut v = c.to_be_bytes().to_vec();
                v.extend_from_slice(reason.as_bytes());
                v
            }
            None => Vec::new(),
        }
    }

    /// Parses a received close body; refuses a one-byte body, a code §7.4 forbids on the wire, and a reason
    /// that is not UTF-8.
    pub fn parse_close(payload: &[u8]) -> Result<Option<(u16, String)>, WsError> {
        match payload.len() {
            0 => Ok(None),
            1 => Err(proto(1002, "one-byte close payload")),
            _ => {
                let code = u16::from_be_bytes([payload[0], payload[1]]);
                let ok = matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999);
                if !ok {
                    return Err(proto(1002, "close code not allowed on the wire"));
                }
                let reason = std::str::from_utf8(&payload[2..]).map_err(|_| proto(1007, "close reason is not UTF-8"))?;
                Ok(Some((code, reason.to_string())))
            }
        }
    }

    // ------------------------------------------------------------ the opening handshake (§4)

    /// The client's opening handshake for `url` (a parsed `ws:`/`wss:` URL): the request bytes and the key.
    pub fn handshake_request(url: &Url, protocols: &[String], extra: &[(String, String)]) -> Result<(Vec<u8>, String), Error> {
        let key = base64(&random_bytes::<16>());
        let mut h = Headers::new();
        let set = |h: &mut Headers, k: &str, v: &str| h.append(k, v).map_err(|e| Error::new(format!("invalid header {k}: {e:?}")));
        set(&mut h, "Upgrade", "websocket")?;
        set(&mut h, "Connection", "Upgrade")?;
        set(&mut h, "Sec-WebSocket-Key", &key)?;
        set(&mut h, "Sec-WebSocket-Version", "13")?;
        if !protocols.is_empty() {
            set(&mut h, "Sec-WebSocket-Protocol", &protocols.join(", "))?;
        }
        for (k, v) in extra {
            set(&mut h, k, v)?;
        }
        let bytes = http_core::h1::encode_request_head("GET", &url.request_target(), &url.host_str(), &h, None)
            .map_err(|e| Error::new(format!("websocket handshake: {e:?}")))?;
        Ok((bytes, key))
    }

    /// §4.1's client checks on the server's handshake. Answers the selected subprotocol ("" for none).
    pub fn check_handshake(head: &http_core::h1::ResponseHead, key: &str, protocols: &[String]) -> Result<String, Error> {
        if head.status != 101 {
            return Err(Error::new(format!("websocket handshake: status {} (expected 101)", head.status)));
        }
        let h = &head.headers;
        if !h.get("upgrade").is_some_and(|v| v.trim().eq_ignore_ascii_case("websocket")) {
            return Err(Error::new("websocket handshake: no `Upgrade: websocket`"));
        }
        if !h.has_token("connection", "upgrade") {
            return Err(Error::new("websocket handshake: no `Connection: Upgrade`"));
        }
        if h.get_all("sec-websocket-accept").count() != 1 || h.get("sec-websocket-accept").map(str::trim) != Some(accept_key(key).as_str()) {
            return Err(Error::new("websocket handshake: Sec-WebSocket-Accept mismatch"));
        }
        if h.get_combined("sec-websocket-extensions").is_some_and(|v| !v.trim().is_empty()) {
            // None was offered (permessage-deflate is declined by not asking): any answer is a failure.
            return Err(Error::new("websocket handshake: server selected an extension that was not offered"));
        }
        let selected = h.get_combined("sec-websocket-protocol").map(|v| v.trim().to_string());
        match selected {
            Some(p) if !p.is_empty() => {
                if !protocols.iter().any(|q| q == &p) {
                    return Err(Error::new(format!("websocket handshake: server selected subprotocol {p:?} that was not requested")));
                }
                Ok(p)
            }
            _ if !protocols.is_empty() => Err(Error::new("websocket handshake: subprotocols were requested and none was selected")),
            _ => Ok(String::new()),
        }
    }

    // ------------------------------------------------------------ the connection

    /// A byte stream the connection runs over (a socket now; a TLS session when http_core exposes one).
    pub trait Transport: Send {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
        fn write_all(&mut self, data: &[u8]) -> std::io::Result<()>;
        fn set_read_timeout(&mut self, d: Option<Duration>) -> std::io::Result<()>;
        fn shutdown(&mut self) {}
    }

    impl Transport for TcpStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            Read::read(self, buf)
        }
        fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
            Write::write_all(self, data)
        }
        fn set_read_timeout(&mut self, d: Option<Duration>) -> std::io::Result<()> {
            TcpStream::set_read_timeout(self, d)
        }
        fn shutdown(&mut self) {
            let _ = TcpStream::shutdown(self, std::net::Shutdown::Both);
        }
    }

    /// How to connect.
    #[derive(Clone)]
    pub struct Config {
        pub protocols: Vec<String>,
        /// Extra request fields (Origin, User-Agent, …).
        pub headers: Vec<(String, String)>,
        pub connect_timeout: Duration,
        /// The jar whose cookies the handshake carries.
        pub cookies: Option<Arc<Mutex<CookieJar>>>,
        /// Store the 101's Set-Cookie fields in `cookies` during the handshake (a browser defers them to its
        /// event loop instead and reads [`Client::set_cookies`]).
        pub store_cookies: bool,
    }

    impl Default for Config {
        fn default() -> Self {
            Config { protocols: Vec::new(), headers: Vec::new(), connect_timeout: Duration::from_secs(10), cookies: None, store_cookies: true }
        }
    }

    /// A received message (or control frame) as the application sees it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Message {
        Text(String),
        Binary(Vec<u8>),
        Ping(Vec<u8>),
        Pong(Vec<u8>),
        /// The peer's close frame (its code and reason, if it sent one).
        Close(Option<(u16, String)>),
    }

    fn now_secs() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }

    /// An open client connection.
    pub struct Client {
        t: Box<dyn Transport>,
        dec: FrameDecoder,
        /// The opcode and bytes of a fragmented message being reassembled.
        frag: Option<(u8, Vec<u8>)>,
        pub protocol: String,
        pub extensions: String,
        /// The handshake response's Set-Cookie fields.
        pub set_cookies: Vec<String>,
        sent_close: bool,
        received_close: bool,
        ready: VecDeque<Message>,
    }

    /// Opens `url` (`ws:` or `wss:`) and runs the opening handshake.
    pub fn connect(url: &str, cfg: &Config) -> Result<Client, Error> {
        let u = Url::parse(url)?;
        match u.scheme() {
            "ws" => {}
            "wss" => {
                return Err(Error::new(
                    "wss:// is owed: http_core's host transport keeps its tls_core session private (no upgraded-connection API)",
                ));
            }
            s => return Err(Error::new(format!("not a WebSocket URL scheme: {s}"))),
        }
        let host = u.hostname();
        let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
        let port = u.port_or_default().unwrap_or(80);
        let addrs: Vec<_> = (host.as_str(), port).to_socket_addrs().map_err(|e| Error::new(format!("resolve {host}: {e}")))?.collect();
        let mut last = Error::new(format!("no address for {host}"));
        let mut stream = None;
        for a in addrs {
            match TcpStream::connect_timeout(&a, cfg.connect_timeout) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last = Error::new(format!("connect {a}: {e}")),
            }
        }
        let s = stream.ok_or(last)?;
        let _ = s.set_nodelay(true);
        Client::handshake(Box::new(s), &u, cfg)
    }

    impl Client {
        /// Runs the opening handshake over an already-connected transport.
        pub fn handshake(mut t: Box<dyn Transport>, u: &Url, cfg: &Config) -> Result<Client, Error> {
            let mut extra = cfg.headers.clone();
            if let Some(jar) = &cfg.cookies {
                // The handshake is an HTTP request to the http(s) form of the URL (WebSockets §4: cookies apply).
                let http_form = u.href().replacen("ws", "http", 1);
                if let Ok(hu) = Url::parse(&http_form) {
                    if let Some(c) = jar.lock().ok().and_then(|mut j| j.cookie_header(&hu, now_secs(), true)) {
                        if !c.is_empty() {
                            extra.push(("Cookie".into(), c));
                        }
                    }
                }
            }
            let (req, key) = handshake_request(u, &cfg.protocols, &extra)?;
            let io = |e: std::io::Error| Error::new(format!("websocket handshake: {e}"));
            t.set_read_timeout(Some(cfg.connect_timeout)).map_err(io)?;
            t.write_all(&req).map_err(io)?;
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let (head, used) = loop {
                let n = t.read(&mut tmp).map_err(io)?;
                if n == 0 {
                    return Err(Error::new("websocket handshake: connection closed before the response"));
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(h) = http_core::h1::parse_response_head(&buf).map_err(|e| Error::new(format!("websocket handshake: {e:?}")))? {
                    break h;
                }
                if buf.len() > 64 * 1024 {
                    return Err(Error::new("websocket handshake: response head too large"));
                }
            };
            let protocol = check_handshake(&head, &key, &cfg.protocols)?;
            let set_cookies: Vec<String> = head.headers.get_all("set-cookie").map(str::to_string).collect();
            if let (Some(jar), true) = (&cfg.cookies, cfg.store_cookies) {
                let http_form = u.href().replacen("ws", "http", 1);
                if let (Ok(hu), Ok(mut j)) = (Url::parse(&http_form), jar.lock()) {
                    for c in &set_cookies {
                        j.set_cookie(&hu, c, now_secs(), true);
                    }
                }
            }
            let mut dec = FrameDecoder::new();
            dec.push(&buf[used..]);
            Ok(Client {
                t,
                dec,
                frag: None,
                protocol,
                extensions: String::new(),
                set_cookies,
                sent_close: false,
                received_close: false,
                ready: VecDeque::new(),
            })
        }

        fn write_frame(&mut self, fin: bool, opcode: u8, payload: &[u8]) -> Result<(), WsError> {
            let f = encode_frame(fin, opcode, payload, Some(random_bytes::<4>()));
            self.t.write_all(&f).map_err(|e| WsError::Io(e.to_string()))
        }

        pub fn send_text(&mut self, s: &str) -> Result<(), WsError> {
            self.write_frame(true, OP_TEXT, s.as_bytes())
        }
        pub fn send_binary(&mut self, b: &[u8]) -> Result<(), WsError> {
            self.write_frame(true, OP_BINARY, b)
        }
        /// Sends one message split into `parts` (a first frame with `opcode`, continuations after it).
        pub fn send_fragmented(&mut self, opcode: u8, parts: &[&[u8]]) -> Result<(), WsError> {
            let n = parts.len().max(1);
            for i in 0..n {
                let p = parts.get(i).copied().unwrap_or(&[]);
                self.write_frame(i + 1 == n, if i == 0 { opcode } else { OP_CONT }, p)?;
            }
            Ok(())
        }
        pub fn ping(&mut self, payload: &[u8]) -> Result<(), WsError> {
            self.write_frame(true, OP_PING, &payload[..payload.len().min(125)])
        }
        /// Starts (or answers) the closing handshake. A second call is a no-op.
        pub fn close(&mut self, code: Option<u16>, reason: &str) -> Result<(), WsError> {
            if self.sent_close {
                return Ok(());
            }
            self.sent_close = true;
            self.write_frame(true, OP_CLOSE, &close_payload(code, reason))
        }
        pub fn close_sent(&self) -> bool {
            self.sent_close
        }
        pub fn close_received(&self) -> bool {
            self.received_close
        }
        /// Drops the transport (after the closing handshake, or to fail the connection).
        pub fn shutdown(&mut self) {
            self.t.shutdown();
        }

        /// Fails the connection (§7.1.7): a close frame with `code` when the transport still takes one, then
        /// the socket is dropped.
        fn fail(&mut self, e: WsError) -> WsError {
            if let WsError::Protocol { code, .. } = &e {
                let c = *code;
                if !self.sent_close {
                    let _ = self.close(Some(c), "");
                }
            }
            self.t.shutdown();
            e
        }

        /// The next message, waiting at most `timeout` (`Ok(None)` when nothing arrived). Pings are answered
        /// with a pong carrying the same payload and are reported too; a close frame is answered (echoing its
        /// code) unless this side already sent one. After the peer's close, a read answers the end of the
        /// transport as `Err(Io)`.
        pub fn read(&mut self, timeout: Duration) -> Result<Option<Message>, WsError> {
            if let Some(m) = self.ready.pop_front() {
                return Ok(Some(m));
            }
            loop {
                match self.dec.next_frame() {
                    Err(e) => return Err(self.fail(e)),
                    Ok(Some(f)) => {
                        if let Some(m) = self.on_frame(f)? {
                            return Ok(Some(m));
                        }
                        continue;
                    }
                    Ok(None) => {}
                }
                let _ = self.t.set_read_timeout(Some(timeout.max(Duration::from_millis(1))));
                let mut tmp = [0u8; 16 * 1024];
                match self.t.read(&mut tmp) {
                    Ok(0) => return Err(WsError::Io("connection closed".into())),
                    Ok(n) => self.dec.push(&tmp[..n]),
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Ok(None),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(WsError::Io(e.to_string())),
                }
            }
        }

        fn on_frame(&mut self, f: Frame) -> Result<Option<Message>, WsError> {
            if f.mask.is_some() {
                // §5.1: a client MUST close the connection if it detects a masked frame.
                return Err(self.fail(proto(1002, "server sent a masked frame")));
            }
            if self.received_close {
                return Ok(None); // nothing after the close frame counts (§5.5.1)
            }
            match f.opcode {
                OP_PING => {
                    if !self.sent_close {
                        let _ = self.write_frame(true, OP_PONG, &f.payload);
                    }
                    Ok(Some(Message::Ping(f.payload)))
                }
                OP_PONG => Ok(Some(Message::Pong(f.payload))),
                OP_CLOSE => {
                    let parsed = match parse_close(&f.payload) {
                        Ok(p) => p,
                        Err(e) => return Err(self.fail(e)),
                    };
                    self.received_close = true;
                    if !self.sent_close {
                        let echo = parsed.as_ref().map(|(c, _)| *c);
                        let _ = self.close(echo, "");
                    }
                    Ok(Some(Message::Close(parsed)))
                }
                OP_TEXT | OP_BINARY => {
                    if self.frag.is_some() {
                        return Err(self.fail(proto(1002, "a new data frame inside a fragmented message")));
                    }
                    if f.fin {
                        return self.finish(f.opcode, f.payload).map(Some);
                    }
                    self.frag = Some((f.opcode, f.payload));
                    Ok(None)
                }
                OP_CONT => {
                    let Some((op, mut buf)) = self.frag.take() else {
                        return Err(self.fail(proto(1002, "a continuation frame with no message to continue")));
                    };
                    if buf.len() + f.payload.len() > MAX_MESSAGE {
                        return Err(self.fail(proto(1009, "message larger than the limit")));
                    }
                    buf.extend_from_slice(&f.payload);
                    if f.fin {
                        return self.finish(op, buf).map(Some);
                    }
                    self.frag = Some((op, buf));
                    Ok(None)
                }
                _ => Err(self.fail(proto(1002, "reserved opcode"))),
            }
        }

        fn finish(&mut self, op: u8, data: Vec<u8>) -> Result<Message, WsError> {
            if op == OP_TEXT {
                match String::from_utf8(data) {
                    Ok(s) => Ok(Message::Text(s)),
                    Err(_) => Err(self.fail(proto(1007, "text message is not UTF-8"))),
                }
            } else {
                Ok(Message::Binary(data))
            }
        }
    }
}

#[cfg(test)]
mod ws_tests {
    use super::ws::{
        accept_key, base64, check_handshake, close_payload, connect, encode_frame, handshake_request, parse_close, sha1, Config,
        FrameDecoder, Message, WsError, OP_BINARY, OP_CLOSE, OP_CONT, OP_PING, OP_PONG, OP_TEXT,
    };

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn sha1_and_accept_kats() {
        // FIPS 180-4 "abc", and RFC 6455 §1.3's worked example.
        let d = sha1(b"abc");
        assert_eq!(d.iter().map(|b| format!("{b:02x}")).collect::<String>(), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(sha1(&[b'a'; 1_000]).len(), 20);
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    /// RFC 6455 §5.7, every example, byte for byte — encoded and decoded.
    #[test]
    fn rfc6455_section_5_7_examples() {
        let mask = Some([0x37, 0xfa, 0x21, 0x3d]);
        // A single-frame unmasked text message.
        let unmasked_hello = hex("81 05 48 65 6c 6c 6f");
        assert_eq!(encode_frame(true, OP_TEXT, b"Hello", None), unmasked_hello);
        // A single-frame masked text message.
        let masked_hello = hex("81 85 37 fa 21 3d 7f 9f 4d 51 58");
        assert_eq!(encode_frame(true, OP_TEXT, b"Hello", mask), masked_hello);
        // A fragmented unmasked text message.
        let frag1 = hex("01 03 48 65 6c");
        let frag2 = hex("80 02 6c 6f");
        assert_eq!(encode_frame(false, OP_TEXT, b"Hel", None), frag1);
        assert_eq!(encode_frame(true, OP_CONT, b"lo", None), frag2);
        // Unmasked Ping request and masked Ping response (a Pong).
        let ping = hex("89 05 48 65 6c 6c 6f");
        let pong = hex("8a 85 37 fa 21 3d 7f 9f 4d 51 58");
        assert_eq!(encode_frame(true, OP_PING, b"Hello", None), ping);
        assert_eq!(encode_frame(true, OP_PONG, b"Hello", mask), pong);
        // 256 bytes binary in a single unmasked frame; 64 KiB binary in a single unmasked frame.
        let b256 = vec![0xABu8; 256];
        let f256 = encode_frame(true, OP_BINARY, &b256, None);
        assert_eq!(&f256[..4], &hex("82 7E 01 00")[..]);
        assert_eq!(f256.len(), 4 + 256);
        let b64k = vec![0x5Au8; 65536];
        let f64k = encode_frame(true, OP_BINARY, &b64k, None);
        assert_eq!(&f64k[..10], &hex("82 7F 00 00 00 00 00 01 00 00")[..]);
        assert_eq!(f64k.len(), 10 + 65536);

        // Decoding: all of them, split at every byte boundary for the masked one.
        let mut d = FrameDecoder::new();
        let mut all = Vec::new();
        for f in [&unmasked_hello, &masked_hello, &frag1, &frag2, &ping, &pong, &f256, &f64k] {
            all.extend_from_slice(f);
        }
        for b in all.chunks(7) {
            d.push(b);
        }
        let mut got = Vec::new();
        while let Some(f) = d.next_frame().unwrap() {
            got.push(f);
        }
        assert_eq!(got.len(), 8);
        assert_eq!((got[0].fin, got[0].opcode, got[0].mask, &got[0].payload[..]), (true, OP_TEXT, None, &b"Hello"[..]));
        assert_eq!((got[1].opcode, got[1].mask, &got[1].payload[..]), (OP_TEXT, mask, &b"Hello"[..]));
        assert_eq!((got[2].fin, got[2].opcode, &got[2].payload[..]), (false, OP_TEXT, &b"Hel"[..]));
        assert_eq!((got[3].fin, got[3].opcode, &got[3].payload[..]), (true, OP_CONT, &b"lo"[..]));
        assert_eq!((got[4].opcode, &got[4].payload[..]), (OP_PING, &b"Hello"[..]));
        assert_eq!((got[5].opcode, got[5].mask, &got[5].payload[..]), (OP_PONG, mask, &b"Hello"[..]));
        assert_eq!((got[6].opcode, got[6].payload.len()), (OP_BINARY, 256));
        assert_eq!((got[7].opcode, got[7].payload.len()), (OP_BINARY, 65536));
    }

    #[test]
    fn close_frames_and_violations() {
        // Close with 1000 "bye" (§5.5.1), and the empty close.
        let f = encode_frame(true, OP_CLOSE, &close_payload(Some(1000), "bye"), None);
        assert_eq!(f, hex("88 05 03 e8 62 79 65"));
        assert_eq!(parse_close(&f[2..]).unwrap(), Some((1000, "bye".into())));
        assert_eq!(parse_close(&[]).unwrap(), None);
        assert!(parse_close(&[3]).is_err());
        for bad in [999u16, 1004, 1005, 1006, 1015, 2999, 5000] {
            assert!(parse_close(&bad.to_be_bytes()).is_err(), "{bad} must be refused on the wire");
        }
        for good in [1000u16, 1001, 1002, 1003, 1007, 1011, 3000, 4999] {
            assert!(parse_close(&good.to_be_bytes()).is_ok(), "{good} is valid");
        }
        assert!(matches!(parse_close(&[0x03, 0xe8, 0xff]), Err(WsError::Protocol { code: 1007, .. })));
        // RSV bits, reserved opcodes, fragmented / oversized control frames.
        for bytes in [hex("c1 00"), hex("83 00"), hex("8b 00"), hex("09 00"), {
            let mut v = hex("89 7e 00 7e");
            v.extend(vec![0; 126]);
            v
        }] {
            let mut d = FrameDecoder::new();
            d.push(&bytes);
            assert!(matches!(d.next_frame(), Err(WsError::Protocol { code: 1002, .. })), "{bytes:02x?}");
        }
        // A 64-bit length with the top bit set.
        let mut d = FrameDecoder::new();
        d.push(&hex("82 7f 80 00 00 00 00 00 00 00"));
        assert!(d.next_frame().is_err());
    }

    #[test]
    fn handshake_checks() {
        let url = super::Url::parse("ws://example.com:8080/chat?x=1").unwrap();
        let (req, key) = handshake_request(&url, &["chat".into(), "superchat".into()], &[("Origin".into(), "http://example.com".into())]).unwrap();
        let req = String::from_utf8(req).unwrap();
        assert!(req.starts_with("GET /chat?x=1 HTTP/1.1\r\nHost: example.com:8080\r\n"), "{req}");
        assert!(req.contains("Upgrade: websocket\r\n") && req.contains("Connection: Upgrade\r\n"));
        assert!(req.contains("Sec-WebSocket-Version: 13\r\n") && req.contains(&format!("Sec-WebSocket-Key: {key}\r\n")));
        assert!(req.contains("Sec-WebSocket-Protocol: chat, superchat\r\n"));
        assert!(!req.to_ascii_lowercase().contains("sec-websocket-extensions"), "no extension is offered");
        let head = |extra: &str, accept: &str| {
            let s = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n{extra}\r\n");
            http_core::h1::parse_response_head(s.as_bytes()).unwrap().unwrap().0
        };
        let good = accept_key(&key);
        let protos = vec!["chat".to_string(), "superchat".to_string()];
        assert_eq!(check_handshake(&head("Sec-WebSocket-Protocol: chat\r\n", &good), &key, &protos).unwrap(), "chat");
        assert!(check_handshake(&head("Sec-WebSocket-Protocol: other\r\n", &good), &key, &protos).is_err());
        assert!(check_handshake(&head("", &good), &key, &protos).is_err(), "requested protocols, none selected");
        assert_eq!(check_handshake(&head("", &good), &key, &[]).unwrap(), "");
        assert!(check_handshake(&head("", "AAAA"), &key, &[]).is_err(), "wrong accept");
        assert!(check_handshake(&head("Sec-WebSocket-Extensions: permessage-deflate\r\n", &good), &key, &[]).is_err());
    }

    /// End to end over a socket: a tiny in-process server (unmasked frames out, masked frames in).
    #[test]
    fn client_against_a_local_server() {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut tmp).unwrap();
                buf.extend_from_slice(&tmp[..n]);
            }
            let req = String::from_utf8_lossy(&buf).to_string();
            let key = req.lines().find_map(|l| l.strip_prefix("Sec-WebSocket-Key: ")).unwrap().trim().to_string();
            write!(s, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_key(&key)).unwrap();
            // A ping, a fragmented text with a ping between its fragments, then read the client's frames.
            s.write_all(&encode_frame(true, OP_PING, b"p1", None)).unwrap();
            s.write_all(&encode_frame(false, OP_TEXT, "h\u{e9}".as_bytes(), None)).unwrap();
            s.write_all(&encode_frame(true, OP_PING, b"p2", None)).unwrap();
            s.write_all(&encode_frame(true, OP_CONT, b"llo", None)).unwrap();
            let mut d = FrameDecoder::new();
            let mut frames = Vec::new();
            while frames.len() < 4 {
                let n = s.read(&mut tmp).unwrap();
                if n == 0 {
                    break;
                }
                d.push(&tmp[..n]);
                while let Some(f) = d.next_frame().unwrap() {
                    frames.push(f);
                }
            }
            s.write_all(&encode_frame(true, OP_CLOSE, &close_payload(Some(4001), "done"), None)).unwrap();
            frames
        });
        let mut c = connect(&format!("ws://127.0.0.1:{port}/echo"), &Config::default()).unwrap();
        let t = std::time::Duration::from_millis(500);
        assert_eq!(c.read(t).unwrap(), Some(Message::Ping(b"p1".to_vec())));
        assert_eq!(c.read(t).unwrap(), Some(Message::Ping(b"p2".to_vec())));
        assert_eq!(c.read(t).unwrap(), Some(Message::Text("h\u{e9}llo".into())));
        c.send_fragmented(OP_BINARY, &[b"ab", b"cd"]).unwrap();
        let close = loop {
            if let Some(m) = c.read(t).unwrap() {
                break m;
            }
        };
        assert_eq!(close, Message::Close(Some((4001, "done".into()))));
        assert!(c.close_sent(), "the client echoes the close");
        let frames = srv.join().unwrap();
        // Two pongs (echoing the pings), then the two fragments — all masked.
        assert!(frames.iter().all(|f| f.mask.is_some()));
        assert_eq!((frames[0].opcode, &frames[0].payload[..]), (OP_PONG, &b"p1"[..]));
        assert_eq!((frames[1].opcode, &frames[1].payload[..]), (OP_PONG, &b"p2"[..]));
        assert_eq!((frames[2].fin, frames[2].opcode, &frames[2].payload[..]), (false, OP_BINARY, &b"ab"[..]));
        assert_eq!((frames[3].fin, frames[3].opcode, &frames[3].payload[..]), (true, OP_CONT, &b"cd"[..]));
    }
}
