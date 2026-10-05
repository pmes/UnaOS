// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! TLS 1.3 for Vein in ring 3 — UnaOS's own `tls_core` (TLSCORE, SR28) over any byte stream that implements
//! [`vein_core::client::Transport`] (the NETRING3 [`crate::net::Tcp`] on the metal, a std socket in the host
//! test). VEINTLS (SR36) replaced `embedded-tls` (no certificate verification) with this: every connection
//! is VERIFIED — the server's chain is built to a root of the trust store ([`crate::trust`],
//! `/system/trust/roots.pem`), checked at the wall-clock time ([`crate::clock`]) and matched to the host
//! name (RFC 6125); a failure ends the handshake with the alert tls_core sends, and the caller's request
//! (which may carry the key) is never written. There is no insecure mode.
//!
//! Crypto is the caller's [`CryptoProvider`]: CRYPTOCORE's `CryptoCoreProvider` in the product
//! (feature `cryptocore`, see [`crate::provider`]), tls_core's test provider in host tests.

use alloc::vec::Vec;
use core::cell::Cell;

use tls_core::error::{AlertDescription, CertError, TlsError};
use tls_core::x509::{Certificate, Clock, PublicKey, TrustStore, WebPkiVerifier};
use tls_core::{Client, ClientConfig, CryptoProvider, ServerCertVerifier};
use vein_core::client::Transport;

/// `-EPROTO`: the TLS layer failed (the [`TlsFail::why`] says how).
pub const EPROTO: i64 = -71;

/// The earliest instant a believable clock can read: 2026-10-04T00:00:00Z (this arc). A clock before it is
/// unset or wrong, and certificate validity cannot be judged against it, so the handshake is not attempted.
pub const CLOCK_FLOOR: i64 = 1_790_985_600;

/// What a verified connection needs, owned by the caller for the life of the program.
#[derive(Clone, Copy)]
pub struct TlsContext<'a> {
    pub provider: &'a dyn CryptoProvider,
    pub store: &'a TrustStore,
    pub clock: &'a dyn Clock,
}

/// The issuer of the server certificate a handshake verified (its commonName, at most 64 bytes) — what
/// the window prints as `transport=tls verified=<issuer CN>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verified {
    cn: [u8; 64],
    n: usize,
}

impl Verified {
    fn new(s: &str) -> Self {
        let mut v = Verified { cn: [0; 64], n: 0 };
        // Printable ASCII only (it goes to the serial wire and the 8x8 font), at most 64 bytes.
        for c in s.bytes().filter(|c| (0x20..0x7f).contains(c)).take(64) {
            v.cn[v.n] = c;
            v.n += 1;
        }
        v
    }
    pub fn issuer(&self) -> &str {
        core::str::from_utf8(&self.cn[..self.n]).unwrap_or("?")
    }
}

/// Why TLS did not carry the exchange: a short static name (window + wire) and the errno.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TlsFail {
    pub why: &'static str,
    pub errno: i64,
}

/// A short, stable name for a tls_core failure (`cert-unknown-issuer`, `cert-name-mismatch`, ...).
pub fn describe(e: &TlsError) -> &'static str {
    use tls_core::crypto::CryptoError;
    match e {
        TlsError::Transport => "transport",
        TlsError::UnexpectedEof => "unexpected-eof",
        TlsError::Decode(s) | TlsError::Protocol(_, s) | TlsError::State(s) => s,
        TlsError::PeerAlert(a) => match a {
            AlertDescription::HandshakeFailure => "peer-alert-handshake-failure",
            AlertDescription::ProtocolVersion => "peer-alert-protocol-version",
            AlertDescription::UnrecognizedName => "peer-alert-unrecognized-name",
            AlertDescription::DecryptError => "peer-alert-decrypt-error",
            AlertDescription::IllegalParameter => "peer-alert-illegal-parameter",
            AlertDescription::InternalError => "peer-alert-internal-error",
            _ => "peer-alert",
        },
        TlsError::PeerAlertUnknown(_) => "peer-alert-unknown",
        TlsError::Crypto(CryptoError::Unsupported(s)) => s,
        TlsError::Crypto(CryptoError::BadSignature) => "certificate-verify-bad-signature",
        TlsError::Crypto(CryptoError::Rng) => "no-entropy",
        TlsError::Crypto(_) => "crypto",
        TlsError::Certificate(c) => match c {
            CertError::BadDer(_) => "cert-bad-der",
            CertError::NoCertificate => "cert-none",
            CertError::UnknownIssuer => "cert-unknown-issuer",
            CertError::Expired => "cert-expired",
            CertError::NotYetValid => "cert-not-yet-valid",
            CertError::BadSignature => "cert-bad-signature",
            CertError::UnsupportedSignatureAlgorithm => "cert-unsupported-signature",
            CertError::NotCa => "cert-not-ca",
            CertError::PathLenExceeded => "cert-path-len",
            CertError::KeyUsage => "cert-key-usage",
            CertError::NameConstraint => "cert-name-constraint",
            CertError::UnknownCriticalExtension => "cert-unknown-critical-extension",
            CertError::NameMismatch => "cert-name-mismatch",
            CertError::PathTooLong => "cert-path-too-long",
            CertError::NoTrustAnchors => "no-trust-anchors",
        },
        TlsError::BadRecordMac => "bad-record-mac",
        TlsError::Closed => "closed",
    }
}

/// tls_core's byte-stream face of the caller's transport. The socket's own errno (ECANCELED from the wait
/// hook, ECONNRESET, ...) lands in a cell the caller keeps, so a TLS "transport" failure reports what
/// really happened underneath, during the handshake as well as after it.
struct Io<'t, T: Transport + ?Sized> {
    t: &'t mut T,
    err: &'t Cell<i64>,
}

impl<T: Transport + ?Sized> tls_core::Transport for Io<'_, T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
        self.t.recv(buf).map_err(|e| {
            self.err.set(e);
            TlsError::Transport
        })
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), TlsError> {
        self.t.send_all(data).map_err(|e| {
            self.err.set(e);
            TlsError::Transport
        })
    }
}

fn fail_of(e: &TlsError, sock: i64) -> TlsFail {
    let errno = match e {
        TlsError::Transport if sock < 0 => sock,
        _ => EPROTO,
    };
    TlsFail { why: describe(e), errno }
}

/// The Web PKI verifier, remembering the issuer CN of the chain it accepted.
struct IssuerVerifier<'a> {
    web: WebPkiVerifier<'a>,
    issuer: Cell<Option<Verified>>,
}

impl ServerCertVerifier for IssuerVerifier<'_> {
    fn verify_server_cert(&self, p: &dyn CryptoProvider, chain: &[Vec<u8>], name: Option<&str>) -> Result<PublicKey, TlsError> {
        let key = self.web.verify_server_cert(p, chain, name)?;
        let cn = chain.first().and_then(|d| Certificate::parse(d).ok()).and_then(|c| c.issuer_cn());
        self.issuer.set(Some(Verified::new(cn.as_deref().unwrap_or("?"))));
        Ok(key)
    }
}

/// An open, verified TLS session — a [`Transport`] for `vein_core::client::exchange`.
pub struct Session<'s, T: Transport + ?Sized> {
    c: Client<'s, Io<'s, T>>,
    sock: &'s Cell<i64>,
    pending: Vec<u8>,
    off: usize,
    /// The first TLS failure after the handshake (`exchange` only sees the errno).
    pub fail: Option<TlsFail>,
}

impl<T: Transport + ?Sized> Session<'_, T> {
    fn record(&mut self, e: &TlsError) -> i64 {
        let f = fail_of(e, self.sock.get());
        if self.fail.is_none() {
            self.fail = Some(f);
        }
        f.errno
    }
}

impl<T: Transport + ?Sized> Transport for Session<'_, T> {
    fn send_all(&mut self, b: &[u8]) -> Result<(), i64> {
        self.c.send(b).map_err(|e| self.record(&e))
    }
    fn recv(&mut self, b: &mut [u8]) -> Result<usize, i64> {
        if self.off == self.pending.len() {
            match self.c.recv() {
                Ok(Some(v)) => {
                    self.pending = v;
                    self.off = 0;
                }
                Ok(None) => return Ok(0),
                Err(e) => return Err(self.record(&e)),
            }
        }
        let n = b.len().min(self.pending.len() - self.off);
        b[..n].copy_from_slice(&self.pending[self.off..self.off + n]);
        self.off += n;
        Ok(n)
    }
}

/// Handshake with `host` over `t` (SNI + RFC 6125 name check, ALPN `http/1.1`), verifying the chain against
/// `ctx.store` at `ctx.clock`; on success run `f` over the session with the verified issuer, then send
/// close_notify. Nothing `f` would write is written unless the handshake verified. Returns `f`'s result and
/// the first TLS failure seen while it ran (if any).
pub fn with_session<T: Transport + ?Sized, R>(t: &mut T, host: &str, ctx: &TlsContext<'_>, f: impl FnOnce(&mut Session<'_, T>, Verified) -> R) -> Result<(R, Option<TlsFail>), TlsFail> {
    if ctx.store.anchors.is_empty() {
        return Err(TlsFail { why: "no-trust-anchors", errno: EPROTO });
    }
    if ctx.clock.now() < CLOCK_FLOOR {
        return Err(TlsFail { why: "clock-unset", errno: una_abi::EAGAIN });
    }
    let verifier = IssuerVerifier { web: WebPkiVerifier { store: ctx.store, clock: ctx.clock }, issuer: Cell::new(None) };
    let mut cfg = ClientConfig::new(Some(host), &verifier);
    cfg.alpn = alloc::vec![b"http/1.1".to_vec()];
    let sock = Cell::new(0i64);
    let c = Client::connect(ctx.provider, &cfg, Io { t, err: &sock }).map_err(|e| fail_of(&e, sock.get()))?;
    let Some(v) = verifier.issuer.get() else {
        return Err(TlsFail { why: "verifier-not-run", errno: EPROTO });
    };
    let mut s = Session { c, sock: &sock, pending: Vec::new(), off: 0, fail: None };
    let r = f(&mut s, v);
    let _ = s.c.close();
    let fail = s.fail;
    Ok((r, fail))
}
