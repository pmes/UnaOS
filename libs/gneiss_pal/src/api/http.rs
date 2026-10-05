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
