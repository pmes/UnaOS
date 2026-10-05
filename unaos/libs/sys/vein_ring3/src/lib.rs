// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! Vein for ring 3 (LUMENAPP, rmbp-ledger B323, R82): the syscall-backed half of the Vein library. A
//! ring-3 program links this crate and `vein_core` and talks to the provider itself, exactly as the host
//! `vessels/lumen` links `handlers/vein`. Nothing here runs unless its caller calls it.
//!
//! * [`sys`] — the syscall stubs (the user-pulse register contract) and a few wrappers (SYS_TIME included).
//! * [`net`] — [`net::resolve`] (SYS_RESOLVE) and [`net::Tcp`], a blocking TCP stream over the
//!   non-blocking NETRING3 socket verbs that implements [`vein_core::client::Transport`].
//! * [`tls`] — TLS 1.3 over any such transport with UnaOS's own `tls_core` (VEINTLS, SR36): every
//!   connection verified against [`trust`] (`/system/trust/roots.pem`) at [`clock`] (SYS_TIME). No
//!   insecure mode exists; the key crosses the wire only on a verified connection (`vein_core::prefs::plan`).
//! * [`heap`] — the size-class `#[global_allocator]` over SYS_SBRK that a TLS-linking program declares.
//! * `provider` (feature `cryptocore`) — CRYPTOCORE's provider, seeded from SYS_GETRANDOM.
//! * [`prefs`] — Principia reads (BUS_VERB_PREF_GET) and the whole Vein configuration in one call.
//! * [`key`] — the key file: read only when it stats with an inode id (UnaFS); refused on FAT.
//! * [`send`] / [`exchange_over`] — one exchange end to end: resolve, connect, TLS, encode, stream.
#![no_std]

extern crate alloc;

pub mod clock;
pub mod heap;
pub mod key;
pub mod net;
pub mod prefs;
#[cfg(feature = "cryptocore")]
pub mod provider;
pub mod sys;
pub mod tls;
pub mod trust;

use alloc::boxed::Box;
use tls_core::x509::{LoadReport, TrustStore};
use tls_core::CryptoProvider;
use vein_core::claude::{self, Event, Msg, Params};
use vein_core::client::{self, Fail, Outcome, Transport};
use vein_core::prefs::{Endpoint, Verify};

pub use tls::{TlsContext, TlsFail, Verified};

/// Every TLS connection this library opens verifies the server (there is no other kind).
pub const VERIFIES_CERTS: bool = true;

/// The buffers one exchange needs, owned by the caller (a ring-3 program keeps them in a static). The TLS
/// record buffers are on the heap now (tls_core allocates them).
pub struct Buffers {
    pub body: [u8; 48 * 1024],
    pub head: [u8; 1024],
    pub rx: [u8; 4096],
    pub line: [u8; 16 * 1024],
}

impl Buffers {
    pub const fn new() -> Self {
        Buffers { body: [0; 48 * 1024], head: [0; 1024], rx: [0; 4096], line: [0; 16 * 1024] }
    }
}

impl Default for Buffers {
    fn default() -> Self {
        Self::new()
    }
}

/// Where an exchange failed before or around the HTTP layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Encode,
    /// A key was handed to a plain-HTTP endpoint: refused before anything is sent.
    KeyOverPlain,
    Resolve(i64),
    Connect(i64),
    /// The TLS handshake did not verify (or failed): nothing of the request was written.
    Handshake(TlsFail),
    /// TLS failed after the handshake, while the request or the answer was in flight.
    TlsStream(TlsFail),
    Exchange(Fail),
}

impl Stage {
    pub fn name(&self) -> &'static str {
        match self {
            Stage::Encode => "encode",
            Stage::KeyOverPlain => "key-over-plain-http",
            Stage::Resolve(_) => "resolve",
            Stage::Connect(_) => "connect",
            Stage::Handshake(_) => "tls-handshake",
            Stage::TlsStream(_) => "tls-stream",
            Stage::Exchange(Fail::Http(_)) => "http-status",
            Stage::Exchange(Fail::Truncated) => "truncated",
            Stage::Exchange(Fail::Head) => "http-head",
            Stage::Exchange(Fail::Framing) => "http-framing",
            Stage::Exchange(Fail::Send(_)) => "send",
            Stage::Exchange(Fail::Recv(_)) => "recv",
        }
    }
    pub fn code(&self) -> i64 {
        match *self {
            Stage::Resolve(e) | Stage::Connect(e) => e,
            Stage::Handshake(f) | Stage::TlsStream(f) => f.errno,
            Stage::Exchange(Fail::Http(s)) => s as i64,
            Stage::Exchange(Fail::Send(e) | Fail::Recv(e)) => e,
            _ => 0,
        }
    }
    /// The TLS reason (`cert-unknown-issuer`, `clock-unset`, ...), for the window and the wire.
    pub fn tls_why(&self) -> Option<&'static str> {
        match self {
            Stage::Handshake(f) | Stage::TlsStream(f) => Some(f.why),
            _ => None,
        }
    }
}

/// The answer: the HTTP outcome and, over TLS, the issuer the handshake verified.
#[derive(Debug, Clone, Copy)]
pub struct Sent {
    pub out: Outcome,
    pub verified: Option<Verified>,
}

/// The encoded request, ready in [`Buffers`]: head and body lengths.
#[derive(Debug, Clone, Copy)]
pub struct Prepared {
    head: usize,
    body: usize,
}

/// Encode the request into `bufs` (head + body). Split from [`send`] so the caller's conversation is
/// borrowed only while it is encoded, and free again while the answer streams into it. A key for a
/// plain-HTTP endpoint is refused here (the rule never asks for one; this is the belt to its braces).
pub fn prepare<'m>(ep: &Endpoint<'_>, p: &Params<'_>, msgs: impl Iterator<Item = Msg<'m>> + Clone, key: Option<&str>, bufs: &mut Buffers) -> Result<Prepared, Stage> {
    if key.is_some() && !ep.tls {
        return Err(Stage::KeyOverPlain);
    }
    let Some(blen) = claude::encode_body(p, msgs, &mut bufs.body) else { return Err(Stage::Encode) };
    let default_port = ep.port == if ep.tls { 443 } else { 80 };
    let mut hh = [0u8; 300];
    let hn = {
        let mut o = vein_core::Out::new(&mut hh);
        o.put(ep.host.as_bytes());
        if !default_port {
            o.put(b":");
            o.dec(ep.port as u64);
        }
        o.done().ok_or(Stage::Encode)?
    };
    let host_hdr = core::str::from_utf8(&hh[..hn]).map_err(|_| Stage::Encode)?;
    let Some(hlen) = claude::request_head(host_hdr, ep.path, key, blen, p.fallbacks, &mut bufs.head) else { return Err(Stage::Encode) };
    Ok(Prepared { head: hlen, body: blen })
}

/// Resolve, connect, (verified TLS), send the prepared request and stream the answer to `on`. While it
/// waits it runs the hook set with [`net::set_tick`] (input, repaint, cancel). The request head (it may
/// hold the key) is zeroed before this returns.
pub fn send(ep: &Endpoint<'_>, req: Prepared, bufs: &mut Buffers, tls: Option<&TlsContext<'_>>, on: &mut dyn FnMut(Event<'_>)) -> Result<Sent, Stage> {
    let ip = match net::resolve(ep.host) {
        Ok(ip) => ip,
        Err(e) => {
            bufs.head.fill(0);
            return Err(Stage::Resolve(e));
        }
    };
    let mut tcp = match net::Tcp::connect(ip, ep.port) {
        Ok(t) => t,
        Err(e) => {
            bufs.head.fill(0);
            return Err(Stage::Connect(e));
        }
    };
    let r = exchange_over(&mut tcp, ep, req, bufs, tls, on);
    drop(tcp);
    r
}

/// The exchange over an already-connected transport: TLS (verified) when `ep.tls`, then the request and the
/// streamed answer. The transport-agnostic heart of [`send`]; the host test drives it over a std socket.
/// The request head is zeroed before this returns, whatever happened.
pub fn exchange_over<T: Transport + ?Sized>(t: &mut T, ep: &Endpoint<'_>, req: Prepared, bufs: &mut Buffers, tls: Option<&TlsContext<'_>>, on: &mut dyn FnMut(Event<'_>)) -> Result<Sent, Stage> {
    let r = exchange_inner(t, ep, req, bufs, tls, on);
    bufs.head.fill(0);
    r
}

fn exchange_inner<T: Transport + ?Sized>(t: &mut T, ep: &Endpoint<'_>, req: Prepared, bufs: &mut Buffers, tls: Option<&TlsContext<'_>>, on: &mut dyn FnMut(Event<'_>)) -> Result<Sent, Stage> {
    let (hlen, blen) = (req.head, req.body);
    let Buffers { body, head, rx, line } = bufs;
    let (out, verified) = if ep.tls {
        let Some(ctx) = tls else { return Err(Stage::Handshake(TlsFail { why: "no-tls-context", errno: tls::EPROTO })) };
        let ((out, v), fail) = tls::with_session(t, ep.host, ctx, |s, v| (client::exchange(s, &head[..hlen], &body[..blen], rx, line, on), v)).map_err(Stage::Handshake)?;
        if let (Some(f), Some(Fail::Send(_) | Fail::Recv(_))) = (fail, out.fail) {
            return Err(Stage::TlsStream(f));
        }
        (out, Some(v))
    } else {
        (client::exchange(t, &head[..hlen], &body[..blen], rx, line, on), None)
    };
    match out.fail {
        // An HTTP error status is an answer (its message went to `on`); everything else is a failure.
        None | Some(Fail::Http(_)) => Ok(Sent { out, verified }),
        Some(f) => Err(Stage::Exchange(f)),
    }
}

/// What a program needs to verify servers, gathered once at start: the crypto provider, the trust store
/// and the clock. [`TlsSetup::verify`] is the key rule's input; [`TlsSetup::context`] feeds [`send`].
pub struct TlsSetup {
    pub provider: Option<Box<dyn CryptoProvider>>,
    /// Why there is no provider (when `provider` is `None`).
    pub provider_why: &'static str,
    pub store: Option<TrustStore>,
    pub report: Option<LoadReport>,
    pub trust_fail: Option<trust::TrustFail>,
    clock: clock::SysClock,
}

impl TlsSetup {
    /// A setup from parts (the host test; a program with its own provider).
    pub fn new(provider: Option<Box<dyn CryptoProvider>>, store: Result<(TrustStore, LoadReport), trust::TrustFail>) -> Self {
        let (store, report, trust_fail) = match store {
            Ok((s, r)) => (Some(s), Some(r), None),
            Err(e) => (None, None, Some(e)),
        };
        TlsSetup { provider_why: if provider.is_some() { "" } else { "no-provider" }, provider, store, report, trust_fail, clock: clock::SysClock }
    }

    /// Ring 3: the product provider (feature `cryptocore`) and `/system/trust/roots.pem`.
    pub fn load() -> Self {
        #[cfg(feature = "cryptocore")]
        let (provider, why) = match provider::product() {
            Ok(p) => (Some(p), ""),
            Err(w) => (None, w),
        };
        #[cfg(not(feature = "cryptocore"))]
        let (provider, why): (Option<Box<dyn CryptoProvider>>, &'static str) = (None, "built without feature cryptocore");
        let mut s = Self::new(provider, trust::load());
        if s.provider.is_none() {
            s.provider_why = why;
        }
        s
    }

    /// Whether a server can be verified right now (provider, trust store, clock — in that order).
    pub fn verify(&self) -> Verify {
        if self.provider.is_none() {
            Verify::NoProvider
        } else if self.store.is_none() {
            Verify::NoTrustStore
        } else if !clock::is_set() {
            Verify::NoClock
        } else {
            Verify::Ready
        }
    }

    /// The context [`send`] takes, with the SYS_TIME clock.
    pub fn context(&self) -> Option<TlsContext<'_>> {
        Some(TlsContext { provider: self.provider.as_deref()?, store: self.store.as_ref()?, clock: &self.clock })
    }
}
