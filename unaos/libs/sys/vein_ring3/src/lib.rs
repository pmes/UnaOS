// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vein — shared-core
//!
//! Vein for ring 3 (LUMENAPP, rmbp-ledger B323, R82): the syscall-backed half of the Vein library. A
//! ring-3 program links this crate and `vein_core` and talks to the provider itself, exactly as the host
//! `vessels/lumen` links `handlers/vein`. Nothing here runs unless its caller calls it.
//!
//! * [`sys`] — the syscall stubs (the user-pulse register contract) and a few wrappers.
//! * [`net`] — [`net::resolve`] (SYS_RESOLVE) and [`net::Tcp`], a blocking TCP stream over the
//!   non-blocking NETRING3 socket verbs that implements [`vein_core::client::Transport`].
//! * [`tls`] (x86_64, feature `tls`) — TLS 1.3 over [`net::Tcp`] via `embedded-tls`. **It verifies no
//!   certificate** (`UnsecureProvider`; the trust store is owed), which is why `vein_core::prefs::plan`
//!   sends the key over it only when the operator set `vein.tls = "insecure"`.
//! * [`prefs`] — Principia reads (BUS_VERB_PREF_GET) and the whole Vein configuration in one call.
//! * [`files`] (LUMENUX B348) — whole-file read / create / append over SYS_PATH_* or home-relative SYS_OPEN.
//! * [`key`] — the key file: read only when it stats with an inode id (UnaFS); refused on FAT.
//! * [`converse`] — one exchange end to end: resolve, connect, (TLS), encode, stream.
#![no_std]

pub mod files;
pub mod key;
pub mod net;
pub mod prefs;
pub mod sys;
#[cfg(all(feature = "tls", target_arch = "x86_64"))]
pub mod tls;

use vein_core::claude::{self, Event, Msg, Params};
use vein_core::client::{self, Fail, Outcome};
use vein_core::prefs::Endpoint;

/// Whether this build can do TLS at all.
pub const HAS_TLS: bool = cfg!(all(feature = "tls", target_arch = "x86_64"));
/// Whether this build can verify a server certificate (the trust store is owed: never, today).
pub const VERIFIES_CERTS: bool = false;

/// The buffers one exchange needs, owned by the caller (a ring-3 program keeps them in a static).
pub struct Buffers {
    pub body: [u8; 48 * 1024],
    pub head: [u8; 1024],
    pub rx: [u8; 4096],
    pub line: [u8; 16 * 1024],
    #[cfg(all(feature = "tls", target_arch = "x86_64"))]
    pub tls: tls::TlsBufs,
}

impl Buffers {
    pub const fn new() -> Self {
        Buffers {
            body: [0; 48 * 1024],
            head: [0; 1024],
            rx: [0; 4096],
            line: [0; 16 * 1024],
            #[cfg(all(feature = "tls", target_arch = "x86_64"))]
            tls: tls::TlsBufs::new(),
        }
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
    Resolve(i64),
    Connect(i64),
    NoTls,
    Handshake(i64),
    Exchange(Fail),
}

impl Stage {
    pub fn name(&self) -> &'static str {
        match self {
            Stage::Encode => "encode",
            Stage::Resolve(_) => "resolve",
            Stage::Connect(_) => "connect",
            Stage::NoTls => "no-tls",
            Stage::Handshake(_) => "tls-handshake",
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
            Stage::Resolve(e) | Stage::Connect(e) | Stage::Handshake(e) => e,
            Stage::Exchange(Fail::Http(s)) => s as i64,
            Stage::Exchange(Fail::Send(e) | Fail::Recv(e)) => e,
            _ => 0,
        }
    }
}

/// The encoded request, ready in [`Buffers`]: head and body lengths.
#[derive(Debug, Clone, Copy)]
pub struct Prepared {
    head: usize,
    body: usize,
}

/// Encode the request into `bufs` (head + body). Split from [`send`] so the caller's conversation is
/// borrowed only while it is encoded, and free again while the answer streams into it.
pub fn prepare<'m>(ep: &Endpoint<'_>, p: &Params<'_>, msgs: impl Iterator<Item = Msg<'m>> + Clone, key: Option<&str>, bufs: &mut Buffers) -> Result<Prepared, Stage> {
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

/// Resolve, connect, (TLS), send the prepared request and stream the answer to `on`. While it waits it
/// runs the hook set with [`net::set_tick`] (input, repaint, cancel). The request head (it may hold the
/// key) is zeroed before this returns.
pub fn send(ep: &Endpoint<'_>, req: Prepared, bufs: &mut Buffers, on: &mut dyn FnMut(Event<'_>)) -> Result<Outcome, Stage> {
    let r = send_inner(ep, req, bufs, on);
    bufs.head.fill(0);
    r
}

fn send_inner(ep: &Endpoint<'_>, req: Prepared, bufs: &mut Buffers, on: &mut dyn FnMut(Event<'_>)) -> Result<Outcome, Stage> {
    let (hlen, blen) = (req.head, req.body);
    let ip = net::resolve(ep.host).map_err(Stage::Resolve)?;
    let mut tcp = net::Tcp::connect(ip, ep.port).map_err(Stage::Connect)?;
    let out = if ep.tls {
        #[cfg(all(feature = "tls", target_arch = "x86_64"))]
        {
            let mut t = tls::Tls::open(&mut tcp, ep.host, &mut bufs.tls).map_err(Stage::Handshake)?;
            client::exchange(&mut t, &bufs.head[..hlen], &bufs.body[..blen], &mut bufs.rx, &mut bufs.line, on)
        }
        #[cfg(not(all(feature = "tls", target_arch = "x86_64")))]
        {
            let _ = (&on, hlen, blen);
            return Err(Stage::NoTls);
        }
    } else {
        client::exchange(&mut tcp, &bufs.head[..hlen], &bufs.body[..blen], &mut bufs.rx, &mut bufs.line, on)
    };
    drop(tcp);
    match out.fail {
        // An HTTP error status is an answer (its message went to `on`); everything else is a failure.
        None | Some(Fail::Http(_)) => Ok(out),
        Some(f) => Err(Stage::Exchange(f)),
    }
}
