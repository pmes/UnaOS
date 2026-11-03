// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Comscan (CODEX §2 "IO Bridge"), seam host handler — a networked controller (grblHAL, FluidNC: grbl over a
//! raw TCP socket) as a `pump_core::Transport`: parse `tcp://host:port`, connect, refuse by name. It owns no protocol
//! (pump_core's) and no gate (Helm's): it carries the bytes the pump sends and returns what the controller says.
//!
//! VUGCOMSCANTCP (LEDGER SR487). The link shape, measured against the pump that drives it and `crate::tty` beside it:
//!
//! - The bus names the link `tcp://<host>:<port>` ([`crate::pump::TCP_SCHEME`]; [`crate::pump::by_scheme`] routes it
//!   here whole, any other name to `crate::tty`). The host is a name, an IPv4 literal or a bracketed IPv6 literal; the
//!   port is required (grblHAL and FluidNC both serve their telnet-style stream on 23 by default, but a board may move
//!   it), 1-65535, with nothing after it. Anything else is [`Refusal::BadAddress`] with its reason.
//! - grblHAL / FluidNC telnet is raw bytes: no telnet option negotiation, the grbl character-counting protocol line for
//!   line, and the realtime bytes (`?`, `!`, `~`, 0x18) still single bytes outside the line buffer. `TCP_NODELAY` is
//!   set so a lone realtime byte is not held by Nagle behind an unacknowledged segment.
//! - Connected with `TcpStream::connect_timeout` (each resolved address in turn), then LEFT nonblocking:
//!   `pump_core::Transport` says "Neither call blocks" and `Pump::tick` reads until `Ok(0)`. `WouldBlock` is `Ok(0)`;
//!   a read of 0 bytes is the peer's FIN (never "nothing yet" on a nonblocking socket) and is `LinkError::Closed`, as
//!   is any other error. std sends with `MSG_NOSIGNAL` (Linux) / `SO_NOSIGPIPE` (macOS): a reset peer is an error, not
//!   a SIGPIPE.
//! - [`Port::ready`] is `crate::pump::ready` (the banner wait every transport shares; one soft reset for a silent
//!   grbl). [`READY_WAIT`] is shorter than tty's: a network board has no bootloader holding the line after connect.
//! - The opener's connect and banner wait run inside `PumpDesk::apply` under the desk's lock (up to [`CONNECT_WAIT`] +
//!   2 x [`READY_WAIT`]), as tty's open and wait do. A LOST link (a reset or a FIN mid-stream) is reopened by the
//!   desk through this same opener with the lock released (VUGCOMSCANRECONNECT, SR521: `PumpDesk::redial_due` /
//!   `relinked`, 250 ms doubling to 4 s); the first open's wait under the lock stays owed.
//! - No enumeration: discovery (mDNS `_telnet._tcp` / FluidNC's `.local` name) is owed as VUG-COMSCAN-MDNS.
//!
//! std only: no libc, no `unsafe`.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use make_core::gcode::Dialect;
use pump_core::{LinkError, Transport};

use crate::pump::{TCP_SCHEME, Unready};

/// How long [`opener`] waits for the TCP handshake by default.
pub const CONNECT_WAIT: Duration = Duration::from_millis(3000);

/// How long [`opener`] waits for a banner (each of the two waits): no bootloader on a network board.
pub const READY_WAIT: Duration = Duration::from_millis(1000);

/// Why a network link did not open or stopped answering, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The port string is not `tcp://<host>:<port>`; `why` names the part that is wrong.
    BadAddress { text: String, why: &'static str },
    /// The host name resolved to no address.
    Unresolved { host: String },
    /// ECONNREFUSED: nothing listens on that port.
    Refused { peer: String },
    /// The handshake did not finish within the wait.
    TimedOut { peer: String, waited_ms: u64 },
    /// grbl said no banner, even after a soft reset.
    NoBanner { peer: String, waited_ms: u64 },
    /// The peer closed the connection (or reset it).
    Closed { peer: String },
    /// Any other socket error, by its kind.
    Io { peer: String, kind: io::ErrorKind },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::BadAddress { text, why } => write!(f, "bad address {text:?}: {why} (tcp://host:port)"),
            Refusal::Unresolved { host } => write!(f, "unresolved host: {host}"),
            Refusal::Refused { peer } => write!(f, "connection refused: {peer}"),
            Refusal::TimedOut { peer, waited_ms } => write!(f, "connect timed out after {waited_ms} ms: {peer}"),
            Refusal::NoBanner { peer, waited_ms } => {
                write!(f, "no banner from {peer} in {waited_ms} ms (after a soft reset)")
            }
            Refusal::Closed { peer } => write!(f, "connection closed: {peer}"),
            Refusal::Io { peer, kind } => write!(f, "socket error {kind:?}: {peer}"),
        }
    }
}

/// The refusal a connect error of `kind` names for `peer` after waiting `waited_ms`.
pub fn refusal_of(kind: io::ErrorKind, peer: &str, waited_ms: u64) -> Refusal {
    let peer = peer.to_string();
    match kind {
        io::ErrorKind::ConnectionRefused => Refusal::Refused { peer },
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Refusal::TimedOut { peer, waited_ms },
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::BrokenPipe => {
            Refusal::Closed { peer }
        }
        _ => Refusal::Io { peer, kind },
    }
}

/// Parse `tcp://<host>:<port>` into its host (brackets of an IPv6 literal removed) and port.
pub fn parse(spec: &str) -> Result<(String, u16), Refusal> {
    let bad = |why: &'static str| Refusal::BadAddress { text: spec.to_string(), why };
    let rest = spec.strip_prefix(TCP_SCHEME).ok_or_else(|| bad("no tcp:// scheme"))?;
    let (host, port) = if let Some(v6) = rest.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or_else(|| bad("no :port"))?;
        (h, after.strip_prefix(':').ok_or_else(|| bad("no :port"))?)
    } else {
        rest.rsplit_once(':').ok_or_else(|| bad("no :port"))?
    };
    if host.is_empty() {
        return Err(bad("empty host"));
    }
    match port.parse::<u16>() {
        Ok(p) if p != 0 && port.bytes().all(|b| b.is_ascii_digit()) => Ok((host.to_string(), p)),
        _ => Err(bad("port not 1-65535")),
    }
}

/// An open, nonblocking TCP link to a controller.
pub struct Port {
    stream: TcpStream,
    peer: String,
}

impl Port {
    /// Connect to `host`:`port` within `timeout` (each resolved address in turn), then set `TCP_NODELAY` and
    /// nonblocking.
    pub fn connect(host: &str, port: u16, timeout: Duration) -> Result<Port, Refusal> {
        let peer = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
        let addrs: Vec<_> = match (host, port).to_socket_addrs() {
            Ok(a) => a.collect(),
            Err(_) => Vec::new(),
        };
        if addrs.is_empty() {
            return Err(Refusal::Unresolved { host: host.to_string() });
        }
        let waited_ms = timeout.as_millis() as u64;
        let mut last = io::ErrorKind::Other;
        for a in &addrs {
            match TcpStream::connect_timeout(a, timeout) {
                Ok(stream) => {
                    stream.set_nodelay(true).map_err(|e| refusal_of(e.kind(), &peer, waited_ms))?;
                    stream.set_nonblocking(true).map_err(|e| refusal_of(e.kind(), &peer, waited_ms))?;
                    return Ok(Port { stream, peer });
                }
                Err(e) => last = e.kind(),
            }
        }
        Err(refusal_of(last, &peer, waited_ms))
    }

    /// Parse `spec` (`tcp://host:port`) and connect within `timeout`.
    pub fn open(spec: &str, timeout: Duration) -> Result<Port, Refusal> {
        let (host, port) = parse(spec)?;
        Port::connect(&host, port, timeout)
    }

    /// `host:port` as connected (`[v6]:port` for an IPv6 literal).
    pub fn peer(&self) -> &str {
        &self.peer
    }

    /// Wait for the controller's banner before any pump sees the link: `crate::pump::ready`, named for this peer.
    pub fn ready(&mut self, dialect: Dialect, wait: Duration) -> Result<Option<String>, Refusal> {
        crate::pump::ready(self, dialect, wait).map_err(|u| match u {
            Unready::NoBanner { waited_ms } => Refusal::NoBanner { peer: self.peer.clone(), waited_ms },
            Unready::Stalled => Refusal::Io { peer: self.peer.clone(), kind: io::ErrorKind::WouldBlock },
            Unready::Closed => Refusal::Closed { peer: self.peer.clone() },
        })
    }
}

impl Transport for Port {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, LinkError> {
        match self.stream.write(bytes) {
            Ok(n) => Ok(n),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(0),
            Err(_) => Err(LinkError::Closed),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkError> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self.stream.read(buf) {
            // Nonblocking: no data is WouldBlock, so 0 is the peer's FIN.
            Ok(0) => Err(LinkError::Closed),
            Ok(n) => Ok(n),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(0),
            Err(_) => Err(LinkError::Closed),
        }
    }
}

/// The host's opener for `crate::pump::serve_pump` (through `crate::pump::by_scheme`): a `tcp://host:port` name
/// connected within `connect` and made ready for `dialect`; a refusal is its name.
pub fn opener(dialect: Dialect, connect: Duration) -> Box<crate::pump::Opener> {
    Box::new(move |port: &str| -> Result<Box<dyn Transport + Send>, String> {
        let mut p = Port::open(port, connect).map_err(|r| r.to_string())?;
        p.ready(dialect, READY_WAIT).map_err(|r| r.to_string())?;
        Ok(Box::new(p))
    })
}

// --- VUGCOMSCANWS (LEDGER SR499): the dialled socket handed to the WebSocket link (named tail hunk) ---

impl Port {
    /// The connected socket (TCP_NODELAY, nonblocking), for `crate::ws` to run its opening handshake over the same
    /// dial and refusals.
    pub(crate) fn into_stream(self) -> TcpStream {
        self.stream
    }
}
