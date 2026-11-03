// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Comscan (CODEX §2 "IO Bridge"), seam host handler — a controller behind a WebSocket (FluidNC on port 81,
//! ESP3D: grbl over RFC 6455) as a `pump_core::Transport`: parse `ws://host:port/path`, dial, hand the socket to the
//! tree's RFC 6455 client, refuse by name. It owns no protocol (pump_core's), no gate (Helm's) and NO WebSocket codec:
//! the opening handshake, the Accept check, masking, framing, fragmentation, ping / pong and the closing handshake are
//! `gneiss_pal::api::http::ws` (AETHERNET SR70), used whole.
//!
//! VUGCOMSCANWS (LEDGER SR499). The link shape, measured against the pump that drives it and `crate::tcp` beside it:
//!
//! - The bus names the link `ws://<host>[:<port>][/<path>]` ([`crate::pump::WS_SCHEME`]; [`crate::pump::with_ws`]
//!   routes it here whole). The port defaults to 80 (RFC 6455 §3); FluidNC serves its WebSocket on 81, so a FluidNC
//!   link is written `ws://fluidnc.local:81/`. `wss://` is refused by name (owed VUG-COMSCAN-WSS).
//! - The dial is `crate::tcp::Port::connect` (resolution, the connect timeout, TCP_NODELAY, the refusals by name),
//!   its socket taken back through `into_stream`, made blocking with a read timeout for the opening handshake
//!   (`ws::Client::handshake`), then nonblocking again: O_NONBLOCK lives on the open file description, which the clone
//!   kept here shares with the one inside the client, so `Client::read` answers `WouldBlock` as `Ok(None)` at once
//!   (`pump_core::Transport`: "Neither call blocks").
//! - A non-101 answer or a failed §4.1 check is [`Refusal::BadHandshake`] with the status line's code read from the
//!   first bytes the handshake saw ([`Sock`] records them while [`Port::connect`] still holds the record).
//! - FluidNC (`FluidNC/src/WebUI/WSChannel.cpp`) sends grbl's output in BINARY frames and its WebUI control messages
//!   in TEXT frames (`CURRENT_ID:<n>`, `currentID:<n>`, `PING`, `PING:60000:60000`); ESP3D uses the same words
//!   (`activeID:`). Inbound, a binary payload joins the stream as it is; a text payload is dropped when it starts
//!   with one of [`CONTROL_WORDS`], else joins the stream as one line (a `\n` added when it has none). A ping is
//!   answered with a pong by the client; a close is answered by the client and ends the link as
//!   [`Refusal::Closed`] with its code; a frame the client refuses (a masked server frame, a reserved opcode, a
//!   frame past `MAX_MESSAGE`) ends it as [`Refusal::Frame`] with the RFC 6455 §7.4 code.
//! - Outbound, each `write` is ONE masked frame carrying every byte it was given (a fresh masking key per frame, the
//!   client's): TEXT when the bytes are UTF-8, BINARY otherwise (grbl's extended realtime bytes 0x84-0xA2 alone are
//!   not UTF-8, and a text frame carrying them would be failed 1007); FluidNC pushes both kinds to grbl. A frame is
//!   never left half-written: [`Sock`]'s `write_all` waits out `WouldBlock` for up to [`WRITE_WAIT`].
//! - The connect, handshake and banner wait run inside `PumpDesk::apply` under the desk's lock, as tcp's do. A link
//!   ended by a close frame or a socket error mid-stream is reopened by the desk through this same opener (a fresh
//!   dial, opening handshake and banner wait) with the lock released (VUGCOMSCANRECONNECT, SR521); the first open's
//!   wait under the lock stays owed.
//!
//! std only: no libc, no `unsafe`.

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gneiss_pal::api::http::Url;
use gneiss_pal::api::http::ws as rfc6455;
use make_core::gcode::Dialect;
use pump_core::{LinkError, Transport};

use crate::pump::{Unready, WS_SCHEME, WSS_SCHEME};
use crate::tcp;

/// How long [`opener`] waits for the TCP handshake and, again, for the opening handshake's answer.
pub const CONNECT_WAIT: Duration = Duration::from_millis(3000);

/// How long [`opener`] waits for a banner (each of the two waits): no bootloader on a network board.
pub const READY_WAIT: Duration = Duration::from_millis(1000);

/// How long one outbound frame may wait on a full socket buffer before the link is given up.
pub const WRITE_WAIT: Duration = Duration::from_millis(1000);

/// The WebUI control messages FluidNC / ESP3D send in TEXT frames beside grbl's stream; never grbl output.
pub const CONTROL_WORDS: [&str; 5] = ["CURRENT_ID:", "currentID:", "ACTIVE_ID:", "activeID:", "PING"];

/// The handshake bytes [`Sock`] records for [`Refusal::BadHandshake`]'s status.
const SEEN_CAP: usize = 1024;

/// Why a WebSocket link did not open or stopped answering, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The port string is not `ws://<host>[:<port>][/<path>]`; `why` names the part that is wrong.
    BadAddress { text: String, why: &'static str },
    /// The TCP dial failed: `crate::tcp`'s refusal (Unresolved, Refused, TimedOut, Io) by its own name.
    Link(tcp::Refusal),
    /// The server did not switch protocols, or its 101 failed RFC 6455 §4.1's checks; `status` is the status line's
    /// code (`None`: no status line arrived).
    BadHandshake { peer: String, status: Option<u16>, why: String },
    /// The client refused a frame and failed the connection with `code` (RFC 6455 §7.4).
    Frame { peer: String, code: u16, what: String },
    /// The peer closed the link: its close frame's code, or `None` (no code, or the socket ended without a close).
    Closed { peer: String, code: Option<u16> },
    /// grbl said no banner, even after a soft reset.
    NoBanner { peer: String, waited_ms: u64 },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::BadAddress { text, why } => write!(f, "bad address {text:?}: {why} (ws://host:port/path)"),
            Refusal::Link(r) => write!(f, "{r}"),
            Refusal::BadHandshake { peer, status: Some(s), why } => {
                write!(f, "websocket handshake refused by {peer} (status {s}): {why}")
            }
            Refusal::BadHandshake { peer, status: None, why } => {
                write!(f, "websocket handshake refused by {peer} (no status line): {why}")
            }
            Refusal::Frame { peer, code, what } => write!(f, "websocket frame refused ({code}) from {peer}: {what}"),
            Refusal::Closed { peer, code: Some(c) } => write!(f, "websocket closed by {peer} (code {c})"),
            Refusal::Closed { peer, code: None } => write!(f, "websocket closed: {peer}"),
            Refusal::NoBanner { peer, waited_ms } => {
                write!(f, "no banner from {peer} in {waited_ms} ms (after a soft reset)")
            }
        }
    }
}

/// Parse `ws://<host>[:<port>][/<path>]` into the URL, its host (brackets of an IPv6 literal removed) and its port.
pub fn parse(spec: &str) -> Result<(Url, String, u16), Refusal> {
    let bad = |why: &'static str| Refusal::BadAddress { text: spec.to_string(), why };
    if spec.starts_with(WSS_SCHEME) {
        return Err(bad("wss:// is owed (VUG-COMSCAN-WSS)"));
    }
    if !spec.starts_with(WS_SCHEME) {
        return Err(bad("no ws:// scheme"));
    }
    let url = Url::parse(spec).map_err(|_| bad("not a URL"))?;
    let host = url.hostname();
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    if host.is_empty() {
        return Err(bad("empty host"));
    }
    let port = url.port_or_default().ok_or_else(|| bad("no port"))?;
    Ok((url, host, port))
}

/// The status code of the HTTP status line at the head of `head` (`HTTP/1.1 101 Switching Protocols`).
fn status_of(head: &[u8]) -> Option<u16> {
    let line = head.split(|&b| b == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

/// The refusal a socket call's error names for `peer`.
fn link_error(e: io::Error, peer: &str, waited: Duration) -> Refusal {
    Refusal::Link(tcp::refusal_of(e.kind(), peer, waited.as_millis() as u64))
}

/// The socket under the RFC 6455 client: records the first bytes it reads while [`Port::connect`] still holds the
/// record, and writes a whole frame or none.
struct Sock {
    s: TcpStream,
    seen: Arc<Mutex<Vec<u8>>>,
}

impl rfc6455::Transport for Sock {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = Read::read(&mut self.s, buf)?;
        if Arc::strong_count(&self.seen) > 1 {
            if let Ok(mut seen) = self.seen.lock() {
                let take = n.min(SEEN_CAP.saturating_sub(seen.len()));
                seen.extend_from_slice(&buf[..take]);
            }
        }
        Ok(n)
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        let end = Instant::now() + WRITE_WAIT;
        let mut at = 0;
        while at < data.len() {
            match self.s.write(&data[at..]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "the socket took no bytes")),
                Ok(n) => at += n,
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {
                    if Instant::now() >= end {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "a frame waited past WRITE_WAIT"));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn set_read_timeout(&mut self, d: Option<Duration>) -> io::Result<()> {
        self.s.set_read_timeout(d)
    }

    fn shutdown(&mut self) {
        let _ = self.s.shutdown(std::net::Shutdown::Both);
    }
}

/// An open, nonblocking WebSocket link to a controller.
pub struct Port {
    client: rfc6455::Client,
    inbox: VecDeque<u8>,
    peer: String,
    stopped: Option<Refusal>,
}

impl Port {
    /// Parse `spec` (`ws://host:port/path`), dial within `timeout` (`crate::tcp::Port::connect`), run the opening
    /// handshake (its answer awaited for `timeout`), then leave the socket nonblocking.
    pub fn connect(spec: &str, timeout: Duration) -> Result<Port, Refusal> {
        let (url, host, port) = parse(spec)?;
        let link = tcp::Port::connect(&host, port, timeout).map_err(Refusal::Link)?;
        let peer = link.peer().to_string();
        let stream = link.into_stream();
        let keep = stream.try_clone().map_err(|e| link_error(e, &peer, timeout))?;
        stream.set_nonblocking(false).map_err(|e| link_error(e, &peer, timeout))?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let cfg = rfc6455::Config { connect_timeout: timeout, ..Default::default() };
        let sock = Sock { s: stream, seen: Arc::clone(&seen) };
        let client = match rfc6455::Client::handshake(Box::new(sock), &url, &cfg) {
            Ok(c) => c,
            Err(e) => {
                let status = seen.lock().ok().and_then(|b| status_of(&b));
                return Err(Refusal::BadHandshake { peer, status, why: e.to_string() });
            }
        };
        drop(seen);
        keep.set_nonblocking(true).map_err(|e| link_error(e, &peer, timeout))?;
        Ok(Port { client, inbox: VecDeque::new(), peer, stopped: None })
    }

    /// `host:port` as dialled (`[v6]:port` for an IPv6 literal).
    pub fn peer(&self) -> &str {
        &self.peer
    }

    /// Why the link ended (`Closed` with the peer's code, `Frame` with the refusal's), `None` while it is open.
    pub fn stopped(&self) -> Option<&Refusal> {
        self.stopped.as_ref()
    }

    /// Wait for the controller's banner before any pump sees the link: `crate::pump::ready`, named for this peer.
    pub fn ready(&mut self, dialect: Dialect, wait: Duration) -> Result<Option<String>, Refusal> {
        crate::pump::ready(self, dialect, wait).map_err(|u| match u {
            Unready::NoBanner { waited_ms } => Refusal::NoBanner { peer: self.peer.clone(), waited_ms },
            Unready::Stalled => Refusal::Link(tcp::Refusal::Io { peer: self.peer.clone(), kind: io::ErrorKind::WouldBlock }),
            Unready::Closed => Refusal::Closed { peer: self.peer.clone(), code: None },
        })
    }

    /// A text message into the stream: a WebUI control word is dropped; anything else is one line.
    fn take_text(&mut self, s: &str) {
        if s.is_empty() || CONTROL_WORDS.iter().any(|w| s.starts_with(w)) {
            return;
        }
        self.inbox.extend(s.as_bytes());
        if !s.ends_with('\n') {
            self.inbox.push_back(b'\n');
        }
    }

    /// The link ends: `why` is kept for [`Port::stopped`] and the socket is dropped.
    fn stop(&mut self, why: Refusal) {
        self.stopped = Some(why);
        self.client.shutdown();
    }

    /// The client's error as the refusal that ends the link.
    fn stop_on(&mut self, e: rfc6455::WsError) {
        let peer = self.peer.clone();
        self.stop(match e {
            rfc6455::WsError::Protocol { code, msg } => Refusal::Frame { peer, code, what: msg },
            rfc6455::WsError::Io(_) => Refusal::Closed { peer, code: None },
        });
    }
}

impl Transport for Port {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, LinkError> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.stopped.is_some() {
            return Err(LinkError::Closed);
        }
        let sent = match std::str::from_utf8(bytes) {
            Ok(text) => self.client.send_text(text),
            Err(_) => self.client.send_binary(bytes),
        };
        match sent {
            Ok(()) => Ok(bytes.len()),
            Err(e) => {
                self.stop_on(e);
                Err(LinkError::Closed)
            }
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkError> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.inbox.len() < buf.len() && self.stopped.is_none() {
            match self.client.read(Duration::ZERO) {
                Ok(Some(rfc6455::Message::Binary(b))) => self.inbox.extend(b),
                Ok(Some(rfc6455::Message::Text(s))) => self.take_text(&s),
                // The client already answered a ping with its pong.
                Ok(Some(rfc6455::Message::Ping(_) | rfc6455::Message::Pong(_))) => {}
                // The client already echoed the close; what arrived before it is still read out below.
                Ok(Some(rfc6455::Message::Close(c))) => {
                    let peer = self.peer.clone();
                    self.stop(Refusal::Closed { peer, code: c.map(|(code, _)| code) });
                }
                Ok(None) => break,
                Err(e) => self.stop_on(e),
            }
        }
        if self.inbox.is_empty() {
            return if self.stopped.is_some() { Err(LinkError::Closed) } else { Ok(0) };
        }
        let n = buf.len().min(self.inbox.len());
        for (d, s) in buf.iter_mut().zip(self.inbox.drain(..n)) {
            *d = s;
        }
        Ok(n)
    }
}

/// The host's opener for `crate::pump::serve_pump` (through `crate::pump::with_ws`): a `ws://host:port/path` name
/// connected and handshaken within `connect` and made ready for `dialect`; a refusal is its name.
pub fn opener(dialect: Dialect, connect: Duration) -> Box<crate::pump::Opener> {
    Box::new(move |port: &str| -> Result<Box<dyn Transport + Send>, String> {
        let mut p = Port::connect(port, connect).map_err(|r| r.to_string())?;
        p.ready(dialect, READY_WAIT).map_err(|r| r.to_string())?;
        Ok(Box::new(p))
    })
}
