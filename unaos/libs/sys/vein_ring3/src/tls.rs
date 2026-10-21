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
    ct: tls_core::ct::CtStatus,
}

impl Verified {
    fn new(s: &str) -> Self {
        let mut v = Verified { cn: [0; 64], n: 0, ct: tls_core::ct::CtStatus::Off };
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
    /// CTCORE: the Certificate Transparency verdict — `policy`, `no_scts`, `insufficient`, `bad_sig`,
    /// `stale_list`, or `off` (no log list staged).
    pub fn ct(&self) -> &'static str {
        self.ct.as_str()
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
            CertError::Revoked => "cert-revoked",
            CertError::BadOcspResponse(_) => "cert-ocsp-response-bad",
            CertError::NameConstraint => "cert-name-constraint",
            CertError::UnknownCriticalExtension => "cert-unknown-critical-extension",
            CertError::NameMismatch => "cert-name-mismatch",
            CertError::PathTooLong => "cert-path-too-long",
            CertError::NoTrustAnchors => "no-trust-anchors",
            CertError::CtPolicy(_) => "cert-ct-policy",
            CertError::MustStaple => "cert-must-staple-missing",
            CertError::BadCrl(_) => "cert-crl-bad",
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
    /// TLSCORE2: the stapled OCSP response and SCTs go through the Web PKI verifier too.
    fn verify_server_cert_full(&self, p: &dyn CryptoProvider, peer: &tls_core::x509::PeerCertificates<'_>, name: Option<&str>) -> Result<tls_core::x509::CertVerdict, TlsError> {
        let v = self.web.verify_server_cert_full(p, peer, name)?;
        let cn = peer.chain.first().and_then(|d| Certificate::parse(d).ok()).and_then(|c| c.issuer_cn());
        let mut ver = Verified::new(cn.as_deref().unwrap_or("?"));
        ver.ct = v.ct.status;
        self.issuer.set(Some(ver));
        Ok(v)
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

    /// AETHERNET (LEDGER SR110): the protocol version the handshake negotiated — 0x0304 (TLS 1.3) or 0x0303
    /// (TLS 1.2, TLSCORE2's relay path) — so a caller's wire can say which (`tls=1.3`). Read-only; additive.
    pub fn version(&self) -> u16 {
        self.c.negotiated().version
    }

    /// NETDEBT2 (LEDGER SR113): the application protocol the server chose from what the handshake offered (RFC 7301)
    /// — `Some(b"h2")`, `Some(b"http/1.1")`, or `None` when it named none (the caller then speaks HTTP/1.1).
    pub fn alpn(&self) -> Option<&[u8]> {
        self.c.negotiated().alpn.as_deref()
    }

    /// The server chose HTTP/2 (only possible when the handshake offered `h2`: [`ALPN_H2_HTTP1`]).
    pub fn is_h2(&self) -> bool {
        self.alpn() == Some(b"h2")
    }

    /// AETHERNET2 (LEDGER SR187): the cipher suite the handshake negotiated, by [`cipher_name`] — so a caller's wire
    /// can say it (`cipher=aes128-gcm`). Read-only; additive.
    pub fn cipher(&self) -> &'static str {
        cipher_name(self.c.negotiated().cipher_suite)
    }
}

/// AETHERNET2 (LEDGER SR187): a short, stable wire name for every suite tls_core speaks — the TLS 1.3 suites by their
/// AEAD (the key exchange is always (EC)DHE there), the TLS 1.2 ones by key exchange, signature and AEAD.
pub fn cipher_name(s: tls_core::msgs::CipherSuite) -> &'static str {
    use tls_core::msgs::CipherSuite as C;
    match s {
        C::Aes128GcmSha256 => "aes128-gcm",
        C::Aes256GcmSha384 => "aes256-gcm",
        C::ChaCha20Poly1305Sha256 => "chacha20-poly1305",
        C::EcdheEcdsaAes128GcmSha256 => "ecdhe-ecdsa-aes128-gcm",
        C::EcdheRsaAes128GcmSha256 => "ecdhe-rsa-aes128-gcm",
        C::EcdheEcdsaAes256GcmSha384 => "ecdhe-ecdsa-aes256-gcm",
        C::EcdheRsaAes256GcmSha384 => "ecdhe-rsa-aes256-gcm",
        C::EcdheEcdsaChaCha20Poly1305Sha256 => "ecdhe-ecdsa-chacha20",
        C::EcdheRsaChaCha20Poly1305Sha256 => "ecdhe-rsa-chacha20",
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

/// ALPN `http/1.1` alone: what [`with_session`] offers — its callers (vein_core's exchange, NET.ELF) speak only
/// HTTP/1.1, so a server must never be allowed to pick h2 for them.
pub const ALPN_HTTP1: &[&[u8]] = &[b"http/1.1"];
/// NETDEBT2 (LEDGER SR113): ALPN `h2, http/1.1` (h2 preferred, RFC 9113 §3.2) — for a caller that speaks both
/// (AETHER.ELF through http_core's `h2`); [`Session::alpn`] says which the server chose.
pub const ALPN_H2_HTTP1: &[&[u8]] = &[b"h2", b"http/1.1"];

/// Handshake with `host` over `t` (SNI + RFC 6125 name check, ALPN `http/1.1`), verifying the chain against
/// `ctx.store` at `ctx.clock`; on success run `f` over the session with the verified issuer, then send
/// close_notify. Nothing `f` would write is written unless the handshake verified. Returns `f`'s result and
/// the first TLS failure seen while it ran (if any).
pub fn with_session<T: Transport + ?Sized, R>(t: &mut T, host: &str, ctx: &TlsContext<'_>, f: impl FnOnce(&mut Session<'_, T>, Verified) -> R) -> Result<(R, Option<TlsFail>), TlsFail> {
    with_session_alpn(t, host, ctx, ALPN_HTTP1, f)
}

/// [`with_session`] offering the application protocols `alpn` in preference order (NETDEBT2, SR113): the
/// negotiated one is [`Session::alpn`] inside `f`. Everything else — verification, the refusals, close_notify — is
/// [`with_session`]'s.
pub fn with_session_alpn<T: Transport + ?Sized, R>(t: &mut T, host: &str, ctx: &TlsContext<'_>, alpn: &[&[u8]], f: impl FnOnce(&mut Session<'_, T>, Verified) -> R) -> Result<(R, Option<TlsFail>), TlsFail> {
    if ctx.store.anchors.is_empty() {
        return Err(TlsFail { why: "no-trust-anchors", errno: EPROTO });
    }
    if ctx.clock.now() < CLOCK_FLOOR {
        return Err(TlsFail { why: "clock-unset", errno: una_abi::EAGAIN });
    }
    let verifier = IssuerVerifier { web: WebPkiVerifier { store: ctx.store, clock: ctx.clock }, issuer: Cell::new(None) };
    let mut cfg = ClientConfig::new(Some(host), &verifier);
    cfg.enable_tls12(); // TLSCORE2 (SR58): a TLS-1.2-only relay is reachable; 1.3 stays preferred (downgrade-sentinel guarded)
    cfg.alpn = alpn.iter().map(|p| p.to_vec()).collect();
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

#[cfg(test)]
mod tests {
    //! NETDEBT2 (LEDGER SR113) — the h2 ALPN test on the host. One server that is NOT ours (Python `ssl` = OpenSSL,
    //! TLS 1.3, ALPN `h2, http/1.1`, a CA + intermediate + leaf from tls_core's oracle script) and two verified
    //! handshakes over std sockets on the PRODUCT provider (CRYPTOCORE): [`with_session_alpn`] with
    //! [`ALPN_H2_HTTP1`] must negotiate `h2` and report it through [`Session::alpn`] / [`Session::is_h2`];
    //! [`with_session`] (ALPN `http/1.1` alone, what vein_core's exchange and NET.ELF use) must still get `http/1.1`
    //! from the same server. The server prints the protocol IT selected and the first application bytes, so both
    //! ends agree. Offline; skipped with a message when python3 or the openssl CLI is missing.
    extern crate std;
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::string::String;
    use std::time::Duration;
    use tls_core::cryptocore_provider::CryptoCoreProvider;

    struct Sock(TcpStream);
    impl Transport for Sock {
        fn send_all(&mut self, b: &[u8]) -> Result<(), i64> {
            self.0.write_all(b).map_err(|_| -32)
        }
        fn recv(&mut self, b: &mut [u8]) -> Result<usize, i64> {
            self.0.read(b).map_err(|_| -104)
        }
    }

    struct Now;
    impl Clock for Now {
        fn now(&self) -> i64 {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
        }
    }

    const SERVER: &str = r#"
import socket, ssl, sys
d, n = sys.argv[1], int(sys.argv[2])
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.minimum_version = ssl.TLSVersion.TLSv1_3
ctx.load_cert_chain(f"{d}/p256_chain.pem", f"{d}/p256.key")
ctx.set_alpn_protocols(["h2", "http/1.1"])
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 0))
s.listen(n)
s.settimeout(30)
print(s.getsockname()[1], flush=True)
for _ in range(n):
    c, _ = s.accept()
    c.settimeout(30)
    try:
        t = ctx.wrap_socket(c, server_side=True)
        got = t.recv(64)
        print("alpn=%s got=%s" % (t.selected_alpn_protocol(), got.decode("ascii", "replace")), flush=True)
        t.close()
    except Exception as e:
        print("FAIL", repr(e).replace("\n", " "), flush=True)
"#;

    fn have(cmd: &str, arg: &str) -> bool {
        Command::new(cmd).arg(arg).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    }

    /// AETHERNET2 (LEDGER SR187): the cipher-name KAT — every suite tls_core offers (by its IANA code) maps to its fixed
    /// wire name, and no two suites share one.
    #[test]
    fn cipher_name_kat() {
        use tls_core::msgs::CipherSuite as C;
        let want: [(u16, &str); 9] = [
            (0x1301, "aes128-gcm"),
            (0x1302, "aes256-gcm"),
            (0x1303, "chacha20-poly1305"),
            (0xC02B, "ecdhe-ecdsa-aes128-gcm"),
            (0xC02F, "ecdhe-rsa-aes128-gcm"),
            (0xC02C, "ecdhe-ecdsa-aes256-gcm"),
            (0xC030, "ecdhe-rsa-aes256-gcm"),
            (0xCCA9, "ecdhe-ecdsa-chacha20"),
            (0xCCA8, "ecdhe-rsa-chacha20"),
        ];
        for (code, name) in want {
            let s = C::from_code(code).expect("tls_core knows the suite");
            assert_eq!(s.code(), code);
            assert_eq!(cipher_name(s), name, "suite {:#06x}", code);
        }
        for s in C::TLS13.iter().chain(C::TLS12.iter()) {
            assert!(want.iter().any(|(c, n)| *c == s.code() && *n == cipher_name(*s)), "suite {:#06x} unnamed", s.code());
        }
        for (i, a) in want.iter().enumerate() {
            assert!(want[i + 1..].iter().all(|b| b.1 != a.1), "{} named twice", a.1);
        }
    }

    #[test]
    fn alpn_offers_h2_and_reports_the_choice() {
        if !have("python3", "--version") || !have("openssl", "version") {
            std::println!("NETDEBT2 ALPN ORACLE SKIPPED: python3 and the openssl CLI are both needed");
            return;
        }
        let dir = std::env::temp_dir().join(std::format!("netdebt2-alpn-{}-{}", std::process::id(), Now.now()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tls_core/tests/oracle/gen_certs_openssl.sh");
        assert!(Command::new("sh").arg(script).arg(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success(), "PKI generation failed");
        let (store, _) = crate::trust::parse(&std::fs::read(dir.join("root.pem")).unwrap()).expect("root parses");
        let provider = CryptoCoreProvider::new();
        let ctx = TlsContext { provider: &provider, store: &store, clock: &Now };

        let mut child = Command::new("python3").arg("-c").arg(SERVER).arg(&dir).arg("2").stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let port: u16 = line.trim().parse().expect("port");
        let dial = || {
            let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            Sock(s)
        };

        // Offering h2 first: the server picks it, and the session says so.
        let mut t = dial();
        let (h2, _) = with_session_alpn(&mut t, "tlscore.test", &ctx, ALPN_H2_HTTP1, |s, v| {
            assert_eq!(v.issuer(), "TLSCORE Oracle Intermediate");
            let r = (s.alpn().map(|a| a.to_vec()), s.is_h2());
            s.send_all(b"ping-h2").unwrap();
            r
        })
        .expect("verified handshake offering h2");
        assert_eq!(h2, (Some(b"h2".to_vec()), true));

        // The default offer (http/1.1 alone) never lets the same server choose h2.
        let mut t = dial();
        let (h1, _) = with_session(&mut t, "tlscore.test", &ctx, |s, _| {
            let r = (s.alpn().map(|a| a.to_vec()), s.is_h2());
            s.send_all(b"ping-h1").unwrap();
            r
        })
        .expect("verified handshake offering http/1.1");
        assert_eq!(h1, (Some(b"http/1.1".to_vec()), false));

        for want in ["alpn=h2 got=ping-h2", "alpn=http/1.1 got=ping-h1"] {
            line.clear();
            out.read_line(&mut line).unwrap();
            std::println!("server: {}", line.trim());
            assert_eq!(line.trim(), want);
        }
        let _ = child.wait();
    }
}
