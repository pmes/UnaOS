//! TLSCORE2 M1 — TLS 1.2 (RFC 5246, ECDHE + AEAD, RFC 7627 extended master secret) against a server that is NOT
//! ours: Python `ssl` (OpenSSL 3) pinned to TLSv1.2 with ONE suite at a time, on the product provider (CRYPTOCORE).
//!
//! The Finished exchange is the oracle for the whole key derivation: OpenSSL accepts our Finished only if our
//! EMS master secret, key block and record protection match its own, and we accept its Finished only if ours match
//! its. The server then reports version, suite and ALPN from its side, and counts every request byte.

mod support;
use support::*;

use tls_core::error::{CertError, TlsError};
use tls_core::msgs::{CipherSuite, NamedGroup, SignatureScheme, TLS12, TLS13};
use tls_core::x509::{TrustStore, WebPkiVerifier};
use tls_core::{Client, ClientConfig};

fn openssl_name(s: CipherSuite) -> &'static str {
    match s {
        CipherSuite::EcdheEcdsaAes128GcmSha256 => "ECDHE-ECDSA-AES128-GCM-SHA256",
        CipherSuite::EcdheRsaAes128GcmSha256 => "ECDHE-RSA-AES128-GCM-SHA256",
        CipherSuite::EcdheEcdsaAes256GcmSha384 => "ECDHE-ECDSA-AES256-GCM-SHA384",
        CipherSuite::EcdheRsaAes256GcmSha384 => "ECDHE-RSA-AES256-GCM-SHA384",
        CipherSuite::EcdheEcdsaChaCha20Poly1305Sha256 => "ECDHE-ECDSA-CHACHA20-POLY1305",
        CipherSuite::EcdheRsaChaCha20Poly1305Sha256 => "ECDHE-RSA-CHACHA20-POLY1305",
        CipherSuite::Aes128GcmSha256 => "TLS_AES_128_GCM_SHA256",
        CipherSuite::Aes256GcmSha384 => "TLS_AES_256_GCM_SHA384",
        CipherSuite::ChaCha20Poly1305Sha256 => "TLS_CHACHA20_POLY1305_SHA256",
    }
}

struct Case {
    leaf: &'static str,
    suite: CipherSuite,
    curve: Option<&'static str>,
    body: usize,
    resp: usize,
    max_fragment: usize,
}

#[test]
fn tls12_each_suite_against_openssl() {
    let Some(dir) = pki("m5") else { return };
    let store = store_of(&dir.join("root.pem"));
    let mut cases = Vec::new();
    for s in CipherSuite::TLS12 {
        let leaf = if matches!(s.auth12(), Some(tls_core::msgs::Auth12::Rsa)) { "rsa" } else { "p256" };
        cases.push(Case { leaf, suite: s, curve: None, body: 0, resp: 0, max_fragment: 16384 });
    }
    // P-256 ECDHE (the server refuses x25519), an Ed25519 leaf under ECDHE_ECDSA (RFC 8422 §5.1.3), and bulk data
    // in 1000-byte records both ways (multi-record reads; explicit-nonce and sequence handling over ~400 records).
    cases.push(Case { leaf: "p256", suite: CipherSuite::EcdheEcdsaAes128GcmSha256, curve: Some("prime256v1"), body: 0, resp: 0, max_fragment: 16384 });
    cases.push(Case { leaf: "ed25519", suite: CipherSuite::EcdheEcdsaChaCha20Poly1305Sha256, curve: None, body: 0, resp: 0, max_fragment: 16384 });
    cases.push(Case { leaf: "rsa", suite: CipherSuite::EcdheRsaAes256GcmSha384, curve: None, body: 100_000, resp: 300_000, max_fragment: 1000 });
    cases.push(Case { leaf: "p256", suite: CipherSuite::EcdheEcdsaChaCha20Poly1305Sha256, curve: None, body: 100_000, resp: 300_000, max_fragment: 1000 });
    let mut pass = 0;
    for c in &cases {
        let mut args = vec!["--tls", "1.2", "--ciphers", openssl_name(c.suite)];
        if let Some(cv) = c.curve {
            args.extend_from_slice(&["--curve", cv]);
        }
        let mut srv = PyServer::start(&dir, c.leaf, &args);
        let p = provider();
        let clock = SystemClock;
        let v = WebPkiVerifier { store: &store, clock: &clock };
        let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
        cfg.enable_tls12(); // offers 1.3 + 1.2: the 1.2-only server picks 1.2 without a downgrade sentinel
        cfg.alpn = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        cfg.max_fragment = c.max_fragment;
        let mut cl = Client::connect(&p, &cfg, dial(srv.port)).unwrap_or_else(|e| panic!("TLS 1.2 {:?} handshake failed: {e:?}", c.suite));
        let n = cl.negotiated().clone();
        // RFC 5705 exporter works on the 1.2 session.
        let mut ekm = [0u8; 32];
        cl.export(b"EXPORTER-tlscore2", b"", &mut ekm).unwrap();
        assert!(ekm.iter().any(|&b| b != 0));
        let resp = exchange(&mut cl, c.body, c.resp);
        drop(cl);
        let verdict = srv.verdict();
        println!(
            "TLS12 {:>7} {:<34} curve={:<10} -> {:?} {:?} {:?} ems={} alpn={:?} resp={}B | server: {verdict}",
            c.leaf, format!("{:?}", c.suite), c.curve.unwrap_or("default"), n.cipher_suite, n.group, n.signature_scheme,
            n.extended_master_secret, n.alpn.as_deref().map(String::from_utf8_lossy), resp.len()
        );
        assert_eq!(n.version, TLS12);
        assert_eq!(n.cipher_suite, c.suite);
        assert!(n.extended_master_secret);
        assert_eq!(n.alpn.as_deref(), Some(&b"http/1.1"[..]));
        assert!(verdict.starts_with("OK version=TLSv1.2"), "server verdict: {verdict}");
        assert!(verdict.contains(&format!("cipher={}", openssl_name(c.suite))), "both ends agree on the suite: {verdict}");
        assert!(verdict.contains(&format!("got={}", c.body)), "{verdict}");
        assert!(!verdict.contains("unwrap="), "clean close_notify both ways: {verdict}");
        assert!(resp.starts_with(b"HTTP/1.1 200 OK"));
        if c.resp > 0 {
            assert!(payload_ok(&resp, c.resp), "300 KB response byte-exact");
        }
        if c.curve == Some("prime256v1") {
            assert_eq!(n.group, NamedGroup::Secp256r1);
        }
        match c.leaf {
            "ed25519" => assert_eq!(n.signature_scheme, SignatureScheme::Ed25519),
            "rsa" => assert!(matches!(n.signature_scheme, SignatureScheme::RsaPssRsaeSha256 | SignatureScheme::RsaPssRsaeSha384 | SignatureScheme::RsaPssRsaeSha512)),
            _ => assert_eq!(n.signature_scheme, SignatureScheme::EcdsaSecp256r1Sha256),
        }
        pass += 1;
    }
    println!("TLS12 ORACLE: {pass}/{} cases agree with OpenSSL", cases.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn version_negotiation_against_a_dual_stack_server() {
    let Some(dir) = pki("m5v") else { return };
    let store = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    let v = WebPkiVerifier { store: &store, clock: &clock };
    // (a) offering both, a 1.2+1.3 server → TLS 1.3 (preferred).
    let mut srv = PyServer::start(&dir, "p256", &["--tls", "any"]);
    let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
    cfg.enable_tls12();
    let mut cl = Client::connect(&p, &cfg, dial(srv.port)).unwrap();
    assert_eq!(cl.negotiated().version, TLS13);
    exchange(&mut cl, 0, 0);
    drop(cl);
    let verdict = srv.verdict();
    println!("DUAL offer 1.3+1.2 -> TLS 1.3 | server: {verdict}");
    assert!(verdict.starts_with("OK version=TLSv1.3"), "{verdict}");
    // (b) a 1.2-ONLY client against the same server: OpenSSL negotiates 1.2 and writes DOWNGRD\x01 into its random
    // (RFC 8446 §4.1.3); a client that did not offer 1.3 must accept it.
    let mut srv = PyServer::start(&dir, "p256", &["--tls", "any"]);
    let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
    cfg.cipher_suites = CipherSuite::TLS12.to_vec();
    let mut cl = Client::connect(&p, &cfg, dial(srv.port)).unwrap();
    assert_eq!(cl.negotiated().version, TLS12);
    exchange(&mut cl, 0, 0);
    drop(cl);
    let verdict = srv.verdict();
    println!("DUAL offer 1.2 only -> TLS 1.2 | server: {verdict}");
    assert!(verdict.starts_with("OK version=TLSv1.2"), "{verdict}");
    // (c) a 1.3-only client (the default config) against a 1.2-only server: protocol_version, both ends see it.
    let mut srv = PyServer::start(&dir, "p256", &["--tls", "1.2"]);
    let cfg = ClientConfig::new(Some("tlscore.test"), &v);
    let e = Client::connect(&p, &cfg, dial(srv.port)).err().expect("1.3-only client must refuse a 1.2 server");
    let verdict = srv.verdict();
    println!("DUAL 1.3-only client vs 1.2 server -> {e:?} | server: {verdict}");
    assert!(matches!(e, TlsError::Protocol(tls_core::AlertDescription::ProtocolVersion, _) | TlsError::PeerAlert(_)));
    assert!(verdict.starts_with("HANDSHAKE-FAIL"), "{verdict}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tls12_certificate_refusals() {
    let Some(dir) = pki("m5r") else { return };
    let store = store_of(&dir.join("root.pem"));
    let mut wrong = TrustStore::new();
    wrong.add_der(&std::fs::read(format!("{}/tests/data/x509/root.der", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    for (st, name, want) in [
        (&wrong, "tlscore.test", TlsError::Certificate(CertError::UnknownIssuer)),
        (&store, "other.test", TlsError::Certificate(CertError::NameMismatch)),
    ] {
        let mut srv = PyServer::start(&dir, "rsa", &["--tls", "1.2"]);
        let p = provider();
        let clock = SystemClock;
        let v = WebPkiVerifier { store: st, clock: &clock };
        let mut cfg = ClientConfig::new(Some(name), &v);
        cfg.enable_tls12();
        let e = Client::connect(&p, &cfg, dial(srv.port)).err().expect("must refuse");
        let verdict = srv.verdict();
        println!("TLS12 refusal {name}: client {e:?} | server: {verdict}");
        assert_eq!(e, want);
        assert!(verdict.starts_with("HANDSHAKE-FAIL"), "server saw our alert: {verdict}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Renegotiation is refused (RFC 5746 + RFC 5246 §7.4.1.1): OpenSSL's s_server sends a HelloRequest (`r` on its
/// stdin) mid-connection; we answer with a warning no_renegotiation, never a new ClientHello. And the
/// RSASSA-PKCS1-v1_5 ServerKeyExchange path (legal in 1.2, never in 1.3), forced with `-sigalgs RSA+SHA256`.
#[test]
fn tls12_renegotiation_refused_and_pkcs1_ske() {
    let Some(dir) = pki("m5n") else { return };
    let store = store_of(&dir.join("root.pem"));
    let mut srv = SServer::start(&dir, "rsa", &["-tls1_2", "-sigalgs", "RSA+SHA256"]);
    let p = provider();
    let clock = SystemClock;
    let v = WebPkiVerifier { store: &store, clock: &clock };
    let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
    cfg.enable_tls12();
    let mut cl = Client::connect(&p, &cfg, dial(srv.port)).expect("handshake with s_server");
    let n = cl.negotiated().clone();
    assert_eq!(n.version, TLS12);
    assert_eq!(n.signature_scheme, SignatureScheme::RsaPkcs1Sha256, "PKCS#1 v1.5 SKE signature verified");
    cl.send(b"before-renegotiation\n").unwrap();
    assert!(srv.wait_for("before-renegotiation", 5000), "s_server got our data: {}", srv.log());
    srv.stdin("r\n"); // SSL_renegotiate: a HelloRequest
    // Read until the HelloRequest has been seen and refused (the server may then end the connection).
    let mut outcome = String::new();
    for _ in 0..50 {
        if cl.renegotiations_refused > 0 {
            break;
        }
        match cl.recv() {
            Ok(Some(d)) => outcome.push_str(&String::from_utf8_lossy(&d)),
            Ok(None) => break,
            Err(e) => {
                outcome = format!("{e:?}");
                break;
            }
        }
    }
    let refused = cl.renegotiations_refused;
    let after = cl.send(b"after-refusal\n");
    std::thread::sleep(Duration::from_millis(300));
    let log = srv.log();
    println!("RENEGOTIATION: refused={refused} recv-outcome={outcome:?} send-after={after:?}\n--- s_server ---\n{log}");
    assert_eq!(refused, 1, "the HelloRequest was answered with no_renegotiation");
    // OpenSSL 3.0 reads our warning no_renegotiation and ends the connection with its own handshake_failure
    // ("ssl3_read_bytes:no renegotiation"): no second handshake happened, and the server says why.
    assert!(log.contains("no renegotiation"), "s_server saw our no_renegotiation: {log}");
    assert_eq!(log.matches("BEGIN SSL SESSION PARAMETERS").count(), 1, "s_server printed one completed handshake: {log}");
    let _ = std::fs::remove_dir_all(&dir);
}

use std::time::Duration;
