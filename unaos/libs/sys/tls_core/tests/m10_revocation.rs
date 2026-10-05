//! CTCORE M2 — must-staple (RFC 7633), CRLs incl. delta basics (RFC 5280 §5, §6.3), the AIA caIssuers seam
//! (RFC 5280 §4.2.2.1). PKI and CRLs from tests/oracle/gen_pki2.sh (openssl CLI); oracles: OpenSSL's s_server
//! (stapling via -status_file, and the alert it receives), `openssl verify -crl_check[_all] -use_deltas`.

mod support;
use support::*;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tls_core::error::{CertError, TlsError};
use tls_core::x509::aia::{certs_from_response, AiaVerifier, IssuerFetcher};
use tls_core::x509::ocsp::OcspStatus;
use tls_core::x509::pem::pem_blocks;
use tls_core::x509::{Certificate, PeerCertificates, TrustStore, WebPkiVerifier};
use tls_core::{Client, ClientConfig, ServerCertVerifier};

fn pki2(tag: &str) -> Option<PathBuf> {
    let dir = pki(tag)?;
    let ok = Command::new("sh").arg(oracle_dir().join("gen_pki2.sh")).arg(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success();
    assert!(ok, "gen_pki2.sh failed");
    Some(dir)
}

fn der_of(dir: &Path, name: &str) -> Vec<u8> {
    pem_blocks(&std::fs::read_to_string(dir.join(format!("{name}.pem"))).unwrap(), "CERTIFICATE").0.remove(0)
}

#[test]
fn must_staple_hard_fails_without_a_good_staple() {
    let Some(dir) = pki2("m10ms") else { return };
    let ms = Certificate::parse(&der_of(&dir, "ms")).unwrap();
    assert!(ms.must_staple(), "TLS Feature status_request parsed (openssl x509 -text shows it)");
    assert!(!Certificate::parse(&der_of(&dir, "p256")).unwrap().must_staple());
    let roots = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    // (leaf, staple, request_ocsp, expected, server sees)
    let cases: [(&str, Option<&str>, bool, Result<&str, TlsError>, Option<&str>); 6] = [
        ("ms", Some("ocsp_ms_good"), true, Ok("good"), None),
        ("ms", None, true, Err(TlsError::Certificate(CertError::MustStaple)), Some("bad certificate status response")),
        ("ms", Some("ocsp_ms_unknown"), true, Err(TlsError::Certificate(CertError::MustStaple)), Some("bad certificate status response")),
        ("ms", Some("ocsp_ms_revoked"), true, Err(TlsError::Certificate(CertError::Revoked)), Some("alert certificate revoked")),
        ("ms", Some("ocsp_ms_good"), false, Err(TlsError::Certificate(CertError::MustStaple)), Some("bad certificate status response")),
        ("p256", None, true, Ok("not-stapled"), None),
    ];
    let mut n = 0;
    for tls in ["-tls1_2", "-tls1_3"] {
        for (leaf, staple, req, want, server) in &cases {
            let mut extra: Vec<String> = vec![tls.to_string()];
            if let Some(s) = staple {
                extra.push("-status_file".into());
                extra.push(dir.join(format!("{s}.der")).display().to_string());
            }
            let extra_ref: Vec<&str> = extra.iter().map(|s| s.as_str()).collect();
            let srv = SServer::start(&dir, leaf, &extra_ref);
            let v = WebPkiVerifier { store: &roots, clock: &clock };
            let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
            cfg.enable_tls12();
            cfg.request_ocsp = *req;
            let r = Client::connect(&p, &cfg, dial(srv.port));
            let got = r.as_ref().map(|c| match c.negotiated().ocsp {
                OcspStatus::Good { .. } => "good",
                OcspStatus::NotStapled => "not-stapled",
                _ => "other",
            });
            let seen = server.map(|s| srv.wait_for(s, 2000));
            println!("MUST-STAPLE {tls} {leaf:<4} staple={:<16} request={req:<5} → {:?}; server saw alert: {seen:?}", staple.unwrap_or("-"), got.as_ref().map_err(|e| (*e).clone()));
            match (want, &got) {
                (Ok(w), Ok(g)) => assert_eq!(w, g),
                (Err(w), Err(g)) => assert_eq!(w, *g),
                _ => panic!("{leaf} {staple:?}: want {want:?} got {got:?}\n{}", srv.log()),
            }
            if let Some(s) = seen {
                assert!(s, "{}", srv.log());
            }
            n += 1;
        }
    }
    println!("MUST-STAPLE: {n}/{n} as RFC 7633 requires");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `openssl verify` over the same chain + CRL set: "ok", "revoked", or "fail" (any other error).
fn openssl_verify(dir: &Path, crls: &[&str], all: bool) -> &'static str {
    let bundle = dir.join("crlset.pem");
    let mut text = String::new();
    for c in crls {
        text.push_str(&std::fs::read_to_string(dir.join(format!("{c}.crl.pem"))).unwrap());
    }
    std::fs::write(&bundle, text).unwrap();
    let mut cmd = Command::new("openssl");
    cmd.current_dir(dir).arg("verify").arg(if all { "-crl_check_all" } else { "-crl_check" }).arg("-use_deltas");
    if !crls.is_empty() {
        cmd.arg("-CRLfile").arg(&bundle);
    }
    let out = cmd.args(["-CAfile", "root.pem", "-untrusted", "inter.pem", "p256.pem"]).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        "ok"
    } else if s.contains("error 23 ") {
        "revoked"
    } else {
        "fail"
    }
}

#[test]
fn crls_and_deltas_agree_with_openssl_verify() {
    let Some(dir) = pki2("m10crl") else { return };
    let p = provider();
    let clock = SystemClock;
    let chain = vec![der_of(&dir, "p256"), der_of(&dir, "inter")];
    // (name, CRLs, -crl_check_all?)
    let cases: [(&str, &[&str], bool); 12] = [
        ("complete, empty", &["crl_empty"], false),
        ("complete, leaf revoked (keyCompromise)", &["crl_revoked"], false),
        ("complete, leaf on hold", &["crl_hold"], false),
        ("hold + delta removeFromCRL", &["crl_hold", "delta_remove"], false),
        ("empty + delta keyCompromise", &["crl_empty", "delta_add"], false),
        ("delta alone (no base)", &["delta_add"], false),
        ("stale (past nextUpdate)", &["crl_stale"], false),
        ("issuer's name, another key", &["crl_wrongkey"], false),
        ("no CRL at all", &[], false),
        ("leaf + inter both clean", &["crl_empty", "root_crl_empty"], true),
        ("root's CRL revokes the intermediate", &["crl_empty", "root_crl_inter"], true),
        ("inter has no CRL", &["crl_empty"], true),
    ];
    let (mut agree, mut diverge, mut soft_n) = (0, 0, 0);
    for (name, crls, all) in cases {
        let mut st = store_of(&dir.join("root.pem"));
        for c in crls {
            st.add_crl_der(&std::fs::read(dir.join(format!("{c}.der"))).unwrap()).unwrap();
        }
        let peer = PeerCertificates { chain: &chain, ocsp: None, sct_list: None, ocsp_requested: false };
        // Soft (default): revoked refuses, an unusable or missing CRL does not.
        let soft = WebPkiVerifier { store: &st, clock: &clock }.verify_server_cert_full(&p, &peer, Some("tlscore.test"));
        // Hard (require_revocation) — what `openssl verify -crl_check` does.
        st.require_revocation = true;
        if !all {
            // -crl_check covers the leaf only: give the intermediate a clean root CRL so only the leaf decides.
            st.add_crl_der(&std::fs::read(dir.join("root_crl_empty.der")).unwrap()).unwrap();
        }
        let hard = WebPkiVerifier { store: &st, clock: &clock }.verify_server_cert_full(&p, &peer, Some("tlscore.test"));
        let ours = match &hard {
            Ok(_) => "ok",
            Err(TlsError::Certificate(CertError::Revoked)) => "revoked",
            Err(_) => "fail",
        };
        let theirs = openssl_verify(&dir, crls, all);
        // The one divergence: OpenSSL takes a delta CRL with no complete CRL as authoritative ("revoked");
        // RFC 5280 §5.2.4 / §6.3.3 use a delta only on top of a complete CRL of the same scope, so tls_core has no
        // usable CRL ("fail" when revocation is required). Both refuse the chain; only the reason differs.
        let theirs = if name == "delta alone (no base)" && theirs == "revoked" { "fail (openssl: revoked)" } else { theirs };
        let ours = if name == "delta alone (no base)" && ours == "fail" { "fail (openssl: revoked)" } else { ours };
        let soft_s = match &soft {
            Ok(v) => format!("ok(checked={} deltas={} unusable={} {:?})", v.crl.checked, v.crl.deltas, v.crl.unusable, v.crl.unusable_why),
            Err(e) => format!("{e:?}"),
        };
        println!("CRL {name:<38} tls_core hard={ours:<7} openssl={theirs:<7} | soft: {soft_s}");
        assert_eq!(ours, theirs, "{name}: {hard:?}");
        if ours != "revoked" {
            assert!(soft.is_ok(), "soft mode accepts an unusable / missing CRL: {name}");
            soft_n += 1;
        } else {
            assert_eq!(soft.err(), Some(TlsError::Certificate(CertError::Revoked)));
        }
        if ours.contains("openssl: revoked") { diverge += 1 } else { agree += 1 }
    }
    println!("CRL: {agree}/12 agree with openssl verify (-crl_check / -crl_check_all, -use_deltas), {diverge} documented divergence (lone delta: both refuse); {soft_n} soft-mode accepts");
    assert_eq!((agree, diverge), (11, 1));

    // Live: s_server with the revoked leaf; the client refuses with certificate_revoked and the server sees it.
    let mut st = store_of(&dir.join("root.pem"));
    st.add_crl_der(&std::fs::read(dir.join("crl_revoked.der")).unwrap()).unwrap();
    for tls in ["-tls1_2", "-tls1_3"] {
        let srv = SServer::start(&dir, "p256", &[tls]);
        let v = WebPkiVerifier { store: &st, clock: &clock };
        let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
        cfg.enable_tls12();
        let r = Client::connect(&p, &cfg, dial(srv.port));
        let seen = srv.wait_for("alert certificate revoked", 2000);
        println!("CRL LIVE {tls}: {:?}; s_server saw alert certificate revoked: {seen}", r.as_ref().err());
        assert_eq!(r.err(), Some(TlsError::Certificate(CertError::Revoked)));
        assert!(seen);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A tiny HTTP/1.0 file server for the caIssuers URIs.
fn http_files(dir: PathBuf) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/').to_string();
            match std::fs::read(dir.join(&path)) {
                Ok(b) => {
                    let _ = s.write_all(format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n", b.len()).as_bytes());
                    let _ = s.write_all(&b);
                }
                Err(_) => {
                    let _ = s.write_all(b"HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                }
            }
        }
    });
    port
}

/// The test's IssuerFetcher: plain HTTP GET over std (the host product uses http_core's).
struct StdFetch {
    log: std::cell::RefCell<Vec<String>>,
}
impl IssuerFetcher for StdFetch {
    fn fetch(&self, uri: &str) -> Option<Vec<u8>> {
        self.log.borrow_mut().push(uri.to_string());
        let rest = uri.strip_prefix("http://")?;
        let (hostport, path) = rest.split_once('/')?;
        let mut s = std::net::TcpStream::connect(hostport).ok()?;
        s.write_all(format!("GET /{path} HTTP/1.0\r\nHost: {hostport}\r\n\r\n").as_bytes()).ok()?;
        let mut b = Vec::new();
        s.read_to_end(&mut b).ok()?;
        let i = b.windows(4).position(|w| w == b"\r\n\r\n")?;
        if !b.starts_with(b"HTTP/1.0 200") {
            return None;
        }
        Some(b[i + 4..].to_vec())
    }
}

#[test]
fn aia_ca_issuers_completes_a_missing_intermediate() {
    let Some(dir) = pki("m10aia") else { return };
    let port = http_files(dir.clone());
    let sh = |script: &str| assert!(Command::new("sh").arg("-c").arg(script).current_dir(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success(), "{script}");
    // inter's certificate as .p7c (certs-only CMS) and .der; three leaves under inter pointing at them / at a 404.
    sh("openssl crl2pkcs7 -nocrl -certfile inter.pem -outform DER -out inter.p7c && openssl x509 -in inter.pem -outform DER -out inter.der");
    sh("openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out foreign.key && openssl req -x509 -new -key foreign.key -subj '/CN=TLSCORE Oracle Intermediate' -days 5 -addext basicConstraints=critical,CA:TRUE -out foreign.pem && openssl x509 -in foreign.pem -outform DER -out foreign.der");
    for (name, file) in [("aia_p7c", "inter.p7c"), ("aia_der", "inter.der"), ("aia_404", "missing.der"), ("aia_foreign", "foreign.der")] {
        sh(&format!(
            "printf 'basicConstraints=critical,CA:FALSE\\nkeyUsage=critical,digitalSignature\\nextendedKeyUsage=serverAuth\\nsubjectAltName=DNS:tlscore.test\\nauthorityInfoAccess=caIssuers;URI:http://127.0.0.1:{port}/{file}\\n' > {name}.ext && \
             openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out {name}.key && openssl req -new -key {name}.key -subj /CN=tlscore.test -out {name}.csr && \
             openssl x509 -req -in {name}.csr -CA inter.pem -CAkey inter.key -CAcreateserial -days 5 -sha256 -extfile {name}.ext -out {name}.pem"
        ));
    }
    assert_eq!(certs_from_response(&std::fs::read(dir.join("inter.p7c")).unwrap()).len(), 1, "certs-only CMS read");
    let roots = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    let mut n = 0;
    for (leaf, want_ok) in [("aia_p7c", true), ("aia_der", true), ("aia_404", false), ("aia_foreign", false)] {
        let cert = dir.join(format!("{leaf}.pem")).display().to_string();
        let key = dir.join(format!("{leaf}.key")).display().to_string();
        // s_server WITHOUT -cert_chain: the intermediate is omitted.
        let args: Vec<String> = ["-cert", &cert, "-key", &key, "-tls1_3"].iter().map(|s| s.to_string()).collect();
        let srv = SServer::start_args(&args);
        let plain = WebPkiVerifier { store: &roots, clock: &clock };
        let mut cfg = ClientConfig::new(Some("tlscore.test"), &plain);
        cfg.enable_tls12();
        assert_eq!(Client::connect(&p, &cfg, dial(srv.port)).err(), Some(TlsError::Certificate(CertError::UnknownIssuer)));
        let fetch = StdFetch { log: Default::default() };
        let aia = AiaVerifier::new(WebPkiVerifier { store: &roots, clock: &clock }, &fetch);
        let srv = SServer::start_args(&args);
        let mut cfg = ClientConfig::new(Some("tlscore.test"), &aia);
        cfg.enable_tls12();
        let r = Client::connect(&p, &cfg, dial(srv.port));
        // Oracle: openssl verify agrees once handed what the fetch returned (OpenSSL itself never fetches AIA).
        let fetched_file = fetch.log.borrow().first().and_then(|u| u.rsplit('/').next().map(String::from)).unwrap_or_default();
        let theirs = match std::fs::read(dir.join(&fetched_file)).ok().map(|b| certs_from_response(&b)) {
            Some(certs) if !certs.is_empty() => {
                std::fs::write(dir.join("fetched.der"), &certs[0]).unwrap();
                sh("openssl x509 -inform DER -in fetched.der -out fetched.pem");
                Command::new("openssl").current_dir(&dir).args(["verify", "-CAfile", "root.pem", "-untrusted", "fetched.pem", &format!("{leaf}.pem")]).output().unwrap().status.success()
            }
            _ => false,
        };
        println!("AIA {leaf:<11} plain=UnknownIssuer  aia={:?} fetched={:?} uris={:?} | openssl verify -untrusted <fetched>: {theirs}", r.as_ref().map(|_| "verified").map_err(|e| e.clone()), aia.fetched.get(), fetch.log.borrow());
        assert_eq!(r.is_ok(), want_ok, "{leaf}");
        assert_eq!(theirs, want_ok);
        if want_ok {
            assert_eq!(r.unwrap().negotiated().pool_intermediates, 1);
        }
        n += 1;
    }
    println!("AIA: {n}/{n} as expected (fetched certificates are untrusted pool members, never anchors)");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = TrustStore::new();
    let _: Option<&dyn ServerCertVerifier> = None;
}
