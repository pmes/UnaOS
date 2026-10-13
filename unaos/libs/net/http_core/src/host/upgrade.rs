//! AETHERWSS (LEDGER SR77): the upgraded connection — RFC 9110 §7.8 `Upgrade` / §15.2.2 `101 Switching
//! Protocols` — as the WebSocket opening handshake (RFC 6455 §4.1) needs it: one request carrying
//! `Connection: Upgrade` + `Upgrade: websocket`, the 101 head read and checked, and the live connection handed
//! back as an [`Upgraded`] that is `Read + Write + Send`, the bytes the server sent past the head (its first
//! frames may share a segment, or a TLS record, with the 101) kept at the front of the stream.
//!
//! The connection is dialled exactly as [`Agent::send`] dials one: the Agent's `resolve` pins and connect
//! timeout, its proxy selection (`proxy_for`) — but an upgrade always TUNNELS through a proxy with CONNECT
//! (RFC 9110 §9.3.6), for `http` as for `https`, since a forward proxy that rewrites an absolute-form request does
//! not relay a protocol switch — and for `https` the same `with_tls` (trust store, AIA, TLS 1.2 beside 1.3,
//! tickets) with ALPN `http/1.1` only (a WebSocket over h2 is RFC 8441, not spoken here). It is never pooled and
//! never follows a redirect (WebSockets §4: a redirect fails the connection); interim 1xx heads other than 101
//! are skipped; any other final status is an error. The Agent's jar and User-Agent apply as in `send`, and the
//! 101's Set-Cookie fields are stored through `after_head` as `send` stores them.
//!
//! Plain (`http`/`ws`): the std `TcpStream` itself, behind the leftover bytes.
//!
//! TLS (`https`/`wss`): `tls_core`'s client borrows a provider that is not `Sync` and a config that lives in
//! `with_tls`'s frame, so the session cannot move into the handle. It stays on a PUMP thread that owns it; the
//! handle talks to it over two channels. The pump alternates: first every queued write (in order, sealed with
//! `Client::send`), then it PEEKS the socket for up to [`POLL`] — and only when a byte is readable does it call
//! `Client::recv`, with the socket's timeout back at the Agent's per-read timeout. That gate matters:
//! `Client::recv` treats any transport error, a timeout included, as fatal (`tls_core` client.rs `recv` → `fail`),
//! so it must never be entered just to wait. Decrypted chunks go to the handle (a bounded channel: a reader that
//! falls behind holds the pump as a full socket buffer would); the end of the stream (close_notify or a TCP close)
//! is an empty chunk; a failure is an `Err` chunk. Writes from the handle are queued and never wait on the pump,
//! so the handle never blocks on a pump that is blocked on it. `shutdown` (and drop) queue a close after any
//! pending writes: the pump sends them, then close_notify, and ends.
//!
//! The one stall left is `tls_core`'s: once `recv` is entered it returns only with application data, so a
//! post-handshake record (a late NewSessionTicket or a KeyUpdate) that arrives ALONE holds queued writes until the
//! server's next record or the per-read timeout. A non-blocking `recv` in `tls_core` would remove it.

use super::*;
use std::io;
use std::sync::mpsc::TryRecvError;

/// How long the TLS pump waits for a readable byte before it looks at the write queue again (the latency a write
/// can see while the connection is idle).
pub const POLL: Duration = Duration::from_millis(5);

/// Decrypted chunks the pump may hold for a reader that has not taken them yet.
const INBOUND_DEPTH: usize = 64;

enum Cmd {
    Data(Vec<u8>),
    Close,
}

enum Io {
    Plain { s: TcpStream, shut: bool },
    Tls { rx: Receiver<Result<Vec<u8>, Error>>, tx: Sender<Cmd>, read_timeout: Option<Duration>, eof: bool, shut: bool },
}

/// A connection after `101 Switching Protocols`: `Read + Write + Send`. Reads first return whatever the server
/// sent past the 101 head, then the live stream; `Ok(0)` is the end of the stream. Over TLS, writes are queued to
/// the connection's pump thread in order (`flush` is a no-op; a pump that has gone away answers `BrokenPipe`).
pub struct Upgraded {
    /// The 101 head (Upgrade, Connection, Sec-WebSocket-Accept, Set-Cookie, …).
    pub head: ResponseHead,
    /// The URL the request went to (its http/https form).
    pub url: Url,
    rest: Vec<u8>,
    pos: usize,
    io: Io,
}

const _: fn() = || {
    fn send<T: Send>() {}
    send::<Upgraded>();
};

fn io_error(e: Error) -> io::Error {
    match e {
        Error::Timeout => io::Error::new(io::ErrorKind::TimedOut, "timed out"),
        e => io::Error::other(e.to_string()),
    }
}

impl Upgraded {
    pub fn status(&self) -> u16 {
        self.head.status
    }
    pub fn headers(&self) -> &Headers {
        &self.head.headers
    }
    /// Whether the stream runs under TLS (an `https`/`wss` upgrade).
    pub fn is_tls(&self) -> bool {
        matches!(self.io, Io::Tls { .. })
    }

    /// How long a read waits before it answers `TimedOut` (`None`: until data or the end of the stream).
    pub fn set_read_timeout(&mut self, d: Option<Duration>) -> io::Result<()> {
        match &mut self.io {
            Io::Plain { s, .. } => s.set_read_timeout(d),
            Io::Tls { read_timeout, .. } => {
                *read_timeout = d;
                Ok(())
            }
        }
    }

    /// Ends the connection: over TLS, after every queued write, with close_notify. A second call is a no-op.
    pub fn shutdown(&mut self) {
        match &mut self.io {
            Io::Plain { s, shut } => {
                if !*shut {
                    *shut = true;
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
            }
            Io::Tls { tx, shut, .. } => {
                if !*shut {
                    *shut = true;
                    let _ = tx.send(Cmd::Close);
                }
            }
        }
    }
}

impl Drop for Upgraded {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Read for Upgraded {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.pos < self.rest.len() {
                let n = buf.len().min(self.rest.len() - self.pos);
                buf[..n].copy_from_slice(&self.rest[self.pos..self.pos + n]);
                self.pos += n;
                if self.pos == self.rest.len() {
                    self.rest = Vec::new();
                    self.pos = 0;
                }
                return Ok(n);
            }
            match &mut self.io {
                Io::Plain { s, .. } => return s.read(buf),
                Io::Tls { rx, read_timeout, eof, .. } => {
                    if *eof {
                        return Ok(0);
                    }
                    let got = match read_timeout {
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                        Some(d) => rx.recv_timeout(*d),
                    };
                    match got {
                        Ok(Ok(chunk)) if chunk.is_empty() => {
                            *eof = true;
                            return Ok(0);
                        }
                        Ok(Ok(chunk)) => {
                            self.rest = chunk;
                            self.pos = 0;
                        }
                        Ok(Err(e)) => {
                            *eof = true;
                            return Err(io_error(e));
                        }
                        Err(RecvTimeoutError::Timeout) => return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out")),
                        Err(RecvTimeoutError::Disconnected) => {
                            *eof = true;
                            return Ok(0);
                        }
                    }
                }
            }
        }
    }
}

impl Write for Upgraded {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.io {
            Io::Plain { s, .. } => s.write(buf),
            Io::Tls { tx, shut, .. } => {
                if *shut {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "upgraded connection shut down"));
                }
                if buf.is_empty() {
                    return Ok(0);
                }
                tx.send(Cmd::Data(buf.to_vec())).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "upgraded connection closed"))?;
                Ok(buf.len())
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match &mut self.io {
            Io::Plain { s, .. } => s.flush(),
            Io::Tls { .. } => Ok(()),
        }
    }
}

// ---------------------------------------------------------------- the request

/// Writes `request` and reads to the end of the 101 head. Answers the head and every byte read past it.
fn read_upgrade_head<T: Transport>(t: &mut T, request: &[u8]) -> Result<(ResponseHead, Vec<u8>), Error> {
    t.write_all(request)?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        if let Some((head, n)) = h1::parse_response_head(&buf)? {
            if (100..200).contains(&head.status) && head.status != 101 {
                buf.drain(..n);
                continue;
            }
            if head.status != 101 {
                return Err(Error::Io(format!("upgrade refused: {} {}", head.status, head.reason)));
            }
            let rest = buf.split_off(n);
            return Ok((head, rest));
        }
        let got = t.read(&mut tmp)?;
        if got == 0 {
            return Err(Error::Http(H1Error::Truncated));
        }
        buf.extend_from_slice(&tmp[..got]);
    }
}

/// The socket for `key`: dialled (through the proxy when one is selected) and, through a proxy, tunnelled.
fn open(cfg: &AgentConfig, key: &PoolKey) -> Result<Tcp, Error> {
    let (dial_host, dial_port) = key.proxy.clone().unwrap_or((key.host.clone(), key.port));
    let s = tcp_connect(&dial_host, dial_port, cfg.connect_timeout, &cfg.resolve)?;
    let _ = s.set_read_timeout(cfg.timeout);
    let _ = s.set_write_timeout(cfg.timeout);
    let mut t = Tcp { s, last: None, rec: None };
    if key.proxy.is_some() {
        tunnel(&mut t, &key.host, key.port)?;
    }
    Ok(t)
}

fn tls_error(c: &mut TlsClient<'_, Tcp>, e: TlsError) -> Error {
    match (e, c.transport_mut().last.take()) {
        (TlsError::Transport, Some(io)) => io,
        (e, _) => Error::Tls(describe_tls(&e)),
    }
}

/// The TLS pump: the session's owner for the life of the upgraded connection (see the module text).
fn pump(mut c: TlsClient<'_, Tcp>, cmds: Receiver<Cmd>, out: SyncSender<Result<Vec<u8>, Error>>, timeout: Option<Duration>) {
    // Every queued write, in order. `false`: the connection is over (closed by the handle, or failed).
    fn drain(c: &mut TlsClient<'_, Tcp>, cmds: &Receiver<Cmd>, out: &SyncSender<Result<Vec<u8>, Error>>) -> bool {
        loop {
            match cmds.try_recv() {
                Ok(Cmd::Data(d)) => {
                    if let Err(e) = c.send(&d) {
                        let e = tls_error(c, e);
                        let _ = out.send(Err(e));
                        return false;
                    }
                }
                Ok(Cmd::Close) | Err(TryRecvError::Disconnected) => {
                    let _ = c.close();
                    let _ = c.transport().s.shutdown(std::net::Shutdown::Write);
                    return false;
                }
                Err(TryRecvError::Empty) => return true,
            }
        }
    }
    let mut one = [0u8; 1];
    loop {
        if !drain(&mut c, &cmds, &out) {
            return;
        }
        let _ = c.transport().s.set_read_timeout(Some(POLL));
        match c.transport().s.peek(&mut one) {
            Ok(0) => {
                // TCP closed without close_notify: the end of the stream (as `Tls::read` treats it), after a last
                // look at the write queue so a close the handle queued still goes out.
                let _ = drain(&mut c, &cmds, &out);
                let _ = out.send(Ok(Vec::new()));
                return;
            }
            Ok(_) => {
                let _ = c.transport().s.set_read_timeout(timeout);
                match c.recv() {
                    Ok(Some(d)) => {
                        if out.send(Ok(d)).is_err() {
                            let _ = c.close();
                            return;
                        }
                    }
                    Ok(None) | Err(TlsError::UnexpectedEof) => {
                        let _ = drain(&mut c, &cmds, &out);
                        let _ = out.send(Ok(Vec::new()));
                        return;
                    }
                    Err(e) => {
                        let e = tls_error(&mut c, e);
                        let _ = out.send(Err(e));
                        return;
                    }
                }
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {}
            Err(e) => {
                let _ = out.send(Err(io_err(e)));
                return;
            }
        }
    }
}

impl Agent {
    /// Blocking: send `req` as an upgrade request and answer the switched connection. `Connection: Upgrade` and
    /// `Upgrade: websocket` are added unless the request already names an upgrade; `ws:`/`wss:` URLs are taken as
    /// their `http:`/`https:` forms. Errors: anything `send` fails with, `Error::Io("upgrade refused: …")` for a
    /// final status other than 101 (redirects included — they are not followed).
    pub fn upgrade(&self, req: Request) -> Result<Upgraded, Error> {
        let mut req = req;
        match req.url.scheme() {
            "http" | "https" => {}
            "ws" | "wss" => req.url = Url::parse(&req.url.href().replacen("ws", "http", 1))?,
            s => return Err(Error::Io(format!("unsupported scheme {s}"))),
        }
        let cfg = self.inner.cfg.clone();
        let mut headers = self.request_headers(&req, false);
        if !headers.contains("upgrade") {
            let _ = headers.set("Upgrade", "websocket");
        }
        if !headers.has_token("connection", "upgrade") {
            let _ = headers.set("Connection", "Upgrade");
        }
        let body = req.body.clone();
        let has_body = !body.is_empty();
        let head = h1::encode_request_head(&req.method, &req.url.request_target(), &req.url.host_str(), &headers, has_body.then_some(body.len()))?;
        let mut request = head;
        request.extend_from_slice(&body);
        let key = pool_key(&req.url, &cfg.proxy)?;

        let (head, rest, io) = if key.scheme == "https" {
            let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(ResponseHead, Vec<u8>), Error>>(1);
            let (in_tx, in_rx) = mpsc::sync_channel::<Result<Vec<u8>, Error>>(INBOUND_DEPTH);
            let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
            let id = {
                let mut n = self.inner.next_id.lock().unwrap();
                *n += 1;
                *n
            };
            std::thread::Builder::new()
                .name(format!("http-upgrade-{id}"))
                .spawn(move || {
                    let t = match open(&cfg, &key) {
                        Ok(t) => t,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    with_tls(&cfg, &key.host, vec![b"http/1.1".to_vec()], t, |client| {
                        let client = match client {
                            Ok(c) => c,
                            Err(e) => {
                                let _ = ready_tx.send(Err(e));
                                return;
                            }
                        };
                        let mut tls = Tls { c: client, pending: Vec::new(), pos: 0 };
                        match read_upgrade_head(&mut tls, &request) {
                            Ok((head, mut rest)) => {
                                // Bytes the record layer already decrypted past what the head read took.
                                rest.extend_from_slice(&tls.pending[tls.pos..]);
                                if ready_tx.send(Ok((head, rest))).is_err() {
                                    let _ = tls.c.close();
                                    return;
                                }
                            }
                            Err(e) => {
                                let _ = ready_tx.send(Err(e));
                                let _ = tls.c.close();
                                return;
                            }
                        }
                        pump(tls.c, cmd_rx, in_tx, cfg.timeout);
                    });
                })
                .map_err(|e| Error::Io(format!("spawn upgrade thread: {e}")))?;
            let (head, rest) = ready_rx.recv().map_err(|_| Error::Io("upgrade thread ended without a response".into()))??;
            (head, rest, Io::Tls { rx: in_rx, tx: cmd_tx, read_timeout: None, eof: false, shut: false })
        } else {
            let mut t = open(&cfg, &key)?;
            let (head, rest) = read_upgrade_head(&mut t, &request)?;
            (head, rest, Io::Plain { s: t.s, shut: false })
        };
        // The 101's cookies, as `send` stores a response's (101 is never a redirect: this answers Final).
        let _ = self.after_head(&req, &head, 0)?;
        Ok(Upgraded { head, url: req.url, rest, pos: 0, io })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn agent() -> Agent {
        Agent::new(AgentConfig { proxy: ProxyMode::None, tickets: None, aia: false, ..Default::default() })
    }

    /// Reads one request head off `s`, answers its text.
    fn read_request(s: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = s.read(&mut tmp).unwrap();
            assert!(n > 0, "client closed before its head ended");
            buf.extend_from_slice(&tmp[..n]);
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// The 101 and the server's first bytes in ONE write: those bytes must come out of the upgraded stream first,
    /// and the stream must carry both directions after them.
    #[test]
    fn upgrade_keeps_bytes_past_the_101_head() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let req = read_request(&mut s);
            s.write_all(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\nEARLY").unwrap();
            let mut got = [0u8; 4];
            s.read_exact(&mut got).unwrap();
            s.write_all(b"LATE").unwrap();
            (req, got)
        });
        let req = Request::new("GET", Url::parse(&format!("ws://127.0.0.1:{port}/chat")).unwrap());
        let mut up = agent().upgrade(req).unwrap();
        assert_eq!(up.status(), 101);
        assert!(!up.is_tls());
        assert_eq!(up.url.scheme(), "http");
        let mut early = [0u8; 5];
        up.read_exact(&mut early).unwrap();
        assert_eq!(&early, b"EARLY");
        up.write_all(b"PING").unwrap();
        up.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut late = [0u8; 4];
        up.read_exact(&mut late).unwrap();
        assert_eq!(&late, b"LATE");
        let (req, got) = srv.join().unwrap();
        assert_eq!(&got, b"PING");
        assert!(req.starts_with("GET /chat HTTP/1.1\r\n"), "{req}");
        let lower = req.to_ascii_lowercase();
        assert!(lower.contains("\r\nupgrade: websocket\r\n"), "{req}");
        assert!(lower.contains("\r\nconnection: upgrade\r\n"), "{req}");
        assert!(!lower.contains("accept-encoding"), "an upgraded stream is never content-decoded: {req}");
    }

    /// A final status other than 101 is an error naming it (and a redirect is not followed).
    #[test]
    fn upgrade_refuses_a_non_101_answer() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let _ = read_request(&mut s);
            s.write_all(b"HTTP/1.1 302 Found\r\nLocation: /elsewhere\r\nContent-Length: 0\r\n\r\n").unwrap();
        });
        let req = Request::new("GET", Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap());
        match agent().upgrade(req) {
            Err(Error::Io(m)) => assert!(m.starts_with("upgrade refused: 302"), "{m}"),
            Err(e) => panic!("wrong error {e:?}"),
            Ok(_) => panic!("a 302 must not upgrade"),
        }
        srv.join().unwrap();
    }

    #[test]
    fn upgrade_refuses_other_schemes() {
        let req = Request::new("GET", Url::parse("ftp://127.0.0.1/").unwrap());
        assert!(matches!(agent().upgrade(req), Err(Error::Io(m)) if m.contains("unsupported scheme")));
    }
}
