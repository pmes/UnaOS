//! TLSCORE2 M4 — negative tests: a man in the middle rewrites a REAL OpenSSL handshake (Python `ssl`), and the
//! client must refuse at the right step with the right alert, which the server then reports. tlsfuzzer itself
//! drives a SERVER under test (its scripts are TLS clients), so none of its scripts can point at a client; this
//! suite is the client-side counterpart, written from RFC 5246 / 7627 / 5746 / 8446 §4.1.3.

mod support;
use support::*;

use std::sync::{Arc, Mutex};

use tls_core::error::{AlertDescription, TlsError};
use tls_core::msgs::{ext, CipherSuite, TLS12};
use tls_core::x509::WebPkiVerifier;
use tls_core::{Client, ClientConfig};

/// Rewrites the ServerHello's extensions with `f`.
fn sh_ext(f: impl Fn(&mut Vec<(u16, Vec<u8>)>) + Send + 'static) -> Rewrite {
    Box::new(move |m: Vec<u8>| {
        if m[0] != 2 {
            return m;
        }
        let mut h = parse_hello(&m);
        f(&mut h.exts);
        encode_hello(2, &h)
    })
}

struct Case {
    what: &'static str,
    server: &'static [&'static str],
    tls12_only: bool,
    c2s: fn() -> Rewrite,
    s2c: fn() -> Rewrite,
    want: AlertDescription,
}

fn strip_sv() -> Rewrite {
    Box::new(|rec: Vec<u8>| {
        let mut h = parse_hello(&rec[5..]);
        h.exts.retain(|(t, _)| *t != ext::SUPPORTED_VERSIONS && *t != ext::KEY_SHARE);
        let m = encode_hello(1, &h);
        let mut r = vec![22, 3, 1, (m.len() >> 8) as u8, m.len() as u8];
        r.extend_from_slice(&m);
        r
    })
}

#[test]
fn mitm_rewrites_are_refused() {
    let Some(dir) = pki("m8") else { return };
    let roots = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    let cases: Vec<Case> = vec![
        Case { what: "downgrade attack: supported_versions stripped from our hello → DOWNGRD sentinel", server: &["--tls", "any"], tls12_only: false, c2s: strip_sv, s2c: keep, want: AlertDescription::IllegalParameter },
        Case { what: "extended_master_secret stripped from the ServerHello", server: &["--tls", "1.2"], tls12_only: false, c2s: keep, s2c: || sh_ext(|e| e.retain(|(t, _)| *t != ext::EXTENDED_MASTER_SECRET)), want: AlertDescription::HandshakeFailure },
        Case { what: "renegotiation_info not empty", server: &["--tls", "1.2"], tls12_only: false, c2s: keep, s2c: || sh_ext(|e| for x in e.iter_mut() { if x.0 == ext::RENEGOTIATION_INFO { x.1 = vec![4, 1, 2, 3, 4] } }), want: AlertDescription::HandshakeFailure },
        Case { what: "unsolicited session_ticket extension", server: &["--tls", "1.2"], tls12_only: false, c2s: keep, s2c: || sh_ext(|e| e.push((ext::SESSION_TICKET, vec![]))), want: AlertDescription::UnsupportedExtension },
        Case { what: "TLS 1.3 key_share in a TLS 1.2 ServerHello (offered by us)", server: &["--tls", "1.2"], tls12_only: false, c2s: keep, s2c: || sh_ext(|e| e.push((ext::KEY_SHARE, vec![0, 0x1d, 0, 0]))), want: AlertDescription::IllegalParameter },
        Case { what: "cipher suite we never offered (TLS_RSA_WITH_AES_128_GCM_SHA256)", server: &["--tls", "1.2"], tls12_only: false, c2s: keep, s2c: || Box::new(|mut m: Vec<u8>| {
            if m[0] == 2 { let sid = m[4 + 34] as usize; let at = 4 + 35 + sid; m[at] = 0x00; m[at + 1] = 0x9c; } m }), want: AlertDescription::IllegalParameter },
        Case { what: "ServerKeyExchange on a curve we did not offer (secp384r1)", server: &["--tls", "1.2"], tls12_only: false, c2s: keep, s2c: || Box::new(|mut m: Vec<u8>| { if m[0] == 12 { m[6] = 0x18; } m }), want: AlertDescription::IllegalParameter },
        Case { what: "ServerKeyExchange signature altered", server: &["--tls", "1.2"], tls12_only: false, c2s: keep, s2c: || Box::new(|mut m: Vec<u8>| { if m[0] == 12 { let n = m.len(); m[n - 5] ^= 1; } m }), want: AlertDescription::DecryptError },
        Case { what: "ServerHello random altered (the SKE signature covers both randoms)", server: &["--tls", "1.2"], tls12_only: false, c2s: keep, s2c: || Box::new(|mut m: Vec<u8>| { if m[0] == 2 { m[10] ^= 1; } m }), want: AlertDescription::DecryptError },
        Case { what: "1.3: unsolicited extension in the ServerHello", server: &["--tls", "1.3"], tls12_only: false, c2s: keep, s2c: || sh_ext(|e| e.push((ext::ALPN, vec![0, 3, 2, b'h', b'2']))), want: AlertDescription::UnsupportedExtension },
    ];
    let mut refused = 0;
    for c in &cases {
        let mut srv = PyServer::start(&dir, "p256", c.server);
        let m = Mitm::start(srv.port, (c.c2s)(), (c.s2c)());
        let v = WebPkiVerifier { store: &roots, clock: &clock };
        let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
        if c.tls12_only { cfg.cipher_suites = CipherSuite::TLS12.to_vec() } else { cfg.enable_tls12() }
        let e = Client::connect(&p, &cfg, dial(m.port)).err();
        let verdict = srv.verdict();
        drop(m);
        let alert = e.as_ref().and_then(|e| e.alert());
        println!("MITM {:<78} client={:?} | server: {verdict}", c.what, e);
        assert_eq!(alert, Some(c.want), "{}", c.what);
        assert!(verdict.starts_with("HANDSHAKE-FAIL"), "{}: server must see the refusal: {verdict}", c.what);
        refused += 1;
    }
    println!("MITM: {refused}/{} rewritten handshakes refused with the expected alert", cases.len());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The sentinel is really there: a 1.2-only client (which must accept it) records the server random a dual-stack
/// OpenSSL sends — its last 8 bytes are DOWNGRD\x01 (RFC 8446 §4.1.3).
#[test]
fn downgrade_sentinel_is_present_and_accepted_by_a_12_only_client() {
    let Some(dir) = pki("m8s") else { return };
    let roots = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    let mut srv = PyServer::start(&dir, "p256", &["--tls", "any"]);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let m = Mitm::start(srv.port, keep(), Box::new(move |msg: Vec<u8>| {
        if msg[0] == 2 {
            *s2.lock().unwrap() = msg[4 + 2 + 24..4 + 2 + 32].to_vec();
        }
        msg
    }));
    let v = WebPkiVerifier { store: &roots, clock: &clock };
    let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
    cfg.cipher_suites = CipherSuite::TLS12.to_vec();
    let mut cl = Client::connect(&p, &cfg, dial(m.port)).unwrap();
    assert_eq!(cl.negotiated().version, TLS12);
    exchange(&mut cl, 0, 0);
    drop(cl);
    let verdict = srv.verdict();
    drop(m);
    let tail = seen.lock().unwrap().clone();
    println!("SENTINEL: server random tail = {:?} | server: {verdict}", String::from_utf8_lossy(&tail));
    assert_eq!(tail, b"DOWNGRD\x01");
    let _ = TlsError::Closed;
    let _ = std::fs::remove_dir_all(&dir);
}

/// ONLINE (skipped without HTTPS_PROXY or the egress CA): the public path with a TLS-1.2-ONLY client. No
/// 1.2-only public host is reachable here (badssl.com is refused by the egress policy, and every allowed host is
/// re-terminated by the egress gateway), so this proves the next best thing: a 1.2-only handshake with a real,
/// non-local TLS stack — the gateway's — verified against its CA (never in place of Mozilla), and a real HTTP
/// answer from api.anthropic.com through it.
#[test]
fn online_tls12_only_through_the_egress_gateway() {
    use std::io::{Read, Write};
    let Ok(proxy) = std::env::var("HTTPS_PROXY").or_else(|_| std::env::var("https_proxy")) else {
        println!("ONLINE SKIPPED: no HTTPS_PROXY");
        return;
    };
    let Ok(pem) = std::fs::read_to_string("/root/.ccr/ca-bundle.crt") else {
        println!("ONLINE SKIPPED: no egress CA bundle");
        return;
    };
    let host = "api.anthropic.com";
    let hp = proxy.trim_start_matches("http://").trim_end_matches('/').to_string();
    let Ok(mut s) = std::net::TcpStream::connect(&hp) else {
        println!("ONLINE SKIPPED: proxy unreachable");
        return;
    };
    s.set_read_timeout(Some(std::time::Duration::from_secs(20))).unwrap();
    write!(s, "CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n").unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).unwrap_or(0) == 0 {
            println!("ONLINE SKIPPED: proxy closed");
            return;
        }
        head.push(b[0]);
    }
    if !String::from_utf8_lossy(&head).contains(" 200") {
        println!("ONLINE SKIPPED: CONNECT refused: {}", String::from_utf8_lossy(&head).lines().next().unwrap_or(""));
        return;
    }
    let (roots, _) = tls_core::x509::TrustStore::from_pem(&pem);
    let p = provider();
    let clock = SystemClock;
    let v = WebPkiVerifier { store: &roots, clock: &clock };
    let mut cfg = ClientConfig::new(Some(host), &v);
    cfg.cipher_suites = CipherSuite::TLS12.to_vec();
    cfg.alpn = vec![b"http/1.1".to_vec()];
    match Client::connect(&p, &cfg, Tcp(s)) {
        Ok(mut c) => {
            let n = c.negotiated().clone();
            c.send(format!("GET /v1/messages HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes()).unwrap();
            let mut resp = Vec::new();
            while let Ok(Some(d)) = c.recv() {
                resp.extend_from_slice(&d);
                if resp.len() > 200 {
                    break;
                }
            }
            let status = String::from_utf8_lossy(&resp).lines().next().unwrap_or("").to_string();
            println!("ONLINE TLS 1.2-only via egress: {:?} {:?} ems={} ocsp={:?} scts={} | {status}", n.cipher_suite, n.group, n.extended_master_secret, n.ocsp, n.scts.len());
            assert_eq!(n.version, TLS12);
            assert!(status.starts_with("HTTP/1.1 "), "{status}");
        }
        Err(e) => println!("ONLINE TLS 1.2-only via egress: refused {e:?} (reported, not asserted: the gateway's policy decides)"),
    }
}
