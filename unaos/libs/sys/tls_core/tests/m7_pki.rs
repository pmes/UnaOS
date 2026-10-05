//! TLSCORE2 M3 — RFC 5280 §6 path building (cross-signed roots, missing intermediates from a pool), name
//! constraints over every certificate below a CA, stapled OCSP (RFC 6960 via RFC 6066 §8 / RFC 8446 §4.4.2.1)
//! verified, SCTs (RFC 6962) parsed. Oracles: `openssl verify` (same chains, same instant: verdicts must agree)
//! and OpenSSL's `s_server -status_file` stapling responses made by `openssl ocsp` (SHA-1 and SHA-256 CertIDs,
//! issuer-signed and delegated), all on the product provider.

mod support;
use support::*;

use std::path::Path;
use std::process::{Command, Stdio};

use tls_core::error::{AlertDescription, CertError, TlsError};
use tls_core::msgs::TLS12;
use tls_core::x509::ocsp::OcspStatus;
use tls_core::x509::pem::pem_blocks;
use tls_core::x509::sct::SctSource;
use tls_core::x509::verify::{collect_scts, verify_server_chain_path};
use tls_core::x509::{Certificate, TrustStore, WebPkiVerifier};
use tls_core::{Client, ClientConfig};

fn pki2(tag: &str) -> Option<std::path::PathBuf> {
    let dir = pki(tag)?;
    let ok = Command::new("sh").arg(oracle_dir().join("gen_pki2.sh")).arg(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success();
    assert!(ok, "gen_pki2.sh failed");
    Some(dir)
}

fn der_of(dir: &Path, name: &str) -> Vec<u8> {
    let (b, _) = pem_blocks(&std::fs::read_to_string(dir.join(format!("{name}.pem"))).unwrap(), "CERTIFICATE");
    b.into_iter().next().unwrap()
}

fn store(dir: &Path, roots: &[&str], pool: &[&str]) -> TrustStore {
    let mut s = TrustStore::new();
    for r in roots {
        s.add_der(&der_of(dir, r)).unwrap();
    }
    for p in pool {
        s.add_intermediates_pem(&std::fs::read_to_string(dir.join(format!("{p}.pem"))).unwrap());
    }
    s
}

/// `openssl verify` with the same anchors, the same untrusted certificates, at the same instant.
fn openssl_ok(dir: &Path, roots: &[&str], untrusted: &[&str], leaf: &str, at: i64) -> bool {
    let ca = dir.join(format!("ca-{}.pem", roots.join("-")));
    std::fs::write(&ca, roots.iter().map(|r| std::fs::read_to_string(dir.join(format!("{r}.pem"))).unwrap()).collect::<String>()).unwrap();
    let mut c = Command::new("openssl");
    c.arg("verify").arg("-attime").arg(at.to_string()).arg("-CAfile").arg(&ca);
    if !untrusted.is_empty() {
        let u = dir.join(format!("u-{}.pem", untrusted.join("-")));
        std::fs::write(&u, untrusted.iter().map(|r| std::fs::read_to_string(dir.join(format!("{r}.pem"))).unwrap()).collect::<String>()).unwrap();
        c.arg("-untrusted").arg(u);
    }
    c.arg(dir.join(format!("{leaf}.pem")));
    c.stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success()
}

#[test]
fn cross_signed_roots_and_missing_intermediates() {
    let Some(dir) = pki2("m7x") else { return };
    let p = provider();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let later = now + 5 * 86_400; // cross_short (1 day) has expired; everything else (30 days) is valid
    struct Case {
        what: &'static str,
        roots: &'static [&'static str],
        chain: &'static [&'static str],
        pool: &'static [&'static str],
        at_later: bool,
        want_anchor: Option<&'static str>,
        want_inters: usize,
    }
    let cases = [
        Case { what: "new root trusted: cross-sign ignored", roots: &["newroot"], chain: &["inter2", "cross"], pool: &[], at_later: false, want_anchor: Some("TLSCORE2 New Root"), want_inters: 1 },
        Case { what: "only old root trusted: path through the cross-sign", roots: &["oldroot"], chain: &["inter2", "cross"], pool: &[], at_later: false, want_anchor: Some("TLSCORE2 Old Root"), want_inters: 2 },
        Case { what: "expired cross-sign first: backtrack to the valid one", roots: &["oldroot"], chain: &["inter2", "cross_short", "cross"], pool: &[], at_later: true, want_anchor: Some("TLSCORE2 Old Root"), want_inters: 2 },
        Case { what: "only an expired cross-sign: refused", roots: &["oldroot"], chain: &["inter2", "cross_short"], pool: &[], at_later: true, want_anchor: None, want_inters: 0 },
        Case { what: "server omits the intermediate: refused", roots: &["newroot"], chain: &[], pool: &[], at_later: false, want_anchor: None, want_inters: 0 },
        Case { what: "server omits the intermediate: the pool supplies it", roots: &["newroot"], chain: &[], pool: &["inter2"], at_later: false, want_anchor: Some("TLSCORE2 New Root"), want_inters: 1 },
        Case { what: "omitted intermediate AND only the old root: pool + cross-sign", roots: &["oldroot"], chain: &[], pool: &["inter2", "cross"], at_later: false, want_anchor: Some("TLSCORE2 Old Root"), want_inters: 2 },
    ];
    let mut agree = 0;
    for c in &cases {
        let st = store(&dir, c.roots, c.pool);
        let mut chain = vec![der_of(&dir, "leaf2")];
        chain.extend(c.chain.iter().map(|n| der_of(&dir, n)));
        let at = if c.at_later { later } else { now };
        let r = verify_server_chain_path(&p, &st, at, &chain, Some("tlscore.test"));
        let mut untrusted: Vec<&str> = c.chain.to_vec();
        untrusted.extend_from_slice(c.pool);
        let ossl = openssl_ok(&dir, c.roots, &untrusted, "leaf2", at);
        let anchor = r.as_ref().ok().map(|v| Certificate::parse(&der_of(&dir, if v.anchor.subject == Certificate::parse(&der_of(&dir, "newroot")).unwrap().subject { "newroot" } else { "oldroot" })).unwrap().subject_cn.unwrap());
        println!(
            "CHAIN {:<62} tls_core={:<40} openssl={}",
            c.what,
            match &r { Ok(v) => format!("OK anchor={:?} inters={} pool={}", anchor.as_deref().unwrap_or("?"), v.intermediates.len(), v.from_pool), Err(e) => format!("{e:?}") },
            if ossl { "OK" } else { "refused" }
        );
        assert_eq!(r.is_ok(), ossl, "{}: tls_core and openssl verify disagree", c.what);
        match (c.want_anchor, &r) {
            (Some(a), Ok(v)) => {
                assert_eq!(anchor.as_deref(), Some(a), "{}", c.what);
                assert_eq!(v.intermediates.len(), c.want_inters, "{}", c.what);
            }
            (None, Err(_)) => {}
            _ => panic!("{}: {r:?}", c.what),
        }
        agree += 1;
    }
    println!("CHAIN: {agree}/{} agree with openssl verify", cases.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn name_constraints_agree_with_openssl() {
    let Some(dir) = pki2("m7n") else { return };
    let p = provider();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let st = store(&dir, &["root"], &[]);
    let cases = [
        ("nc_good", true, "DNS+email+URI inside, subject O=Good"),
        ("nc_dns_out", false, "dNSName outside the permitted subtree"),
        ("nc_excl", false, "dNSName in the excluded subtree"),
        ("nc_email_out", false, "rfc822Name outside"),
        ("nc_dir_out", false, "subject DN outside the permitted directoryName"),
        ("nc_uri_out", false, "URI host outside"),
    ];
    for (leaf, want, what) in cases {
        let chain = vec![der_of(&dir, leaf), der_of(&dir, "nc_inter")];
        let r = verify_server_chain_path(&p, &st, now, &chain, None);
        let ossl = openssl_ok(&dir, &["root"], &["nc_inter"], leaf, now);
        println!("NC {leaf:<13} {what:<48} tls_core={:<20} openssl={}", match &r { Ok(_) => "OK".to_string(), Err(e) => format!("{e:?}") }, if ossl { "OK" } else { "refused" });
        assert_eq!(r.is_ok(), ossl, "{leaf}: disagreement with openssl verify");
        assert_eq!(r.is_ok(), want, "{leaf}");
        if !want {
            assert_eq!(r.unwrap_err(), CertError::NameConstraint);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Stapled OCSP over TLS 1.2 and TLS 1.3 from OpenSSL's s_server.
#[test]
fn ocsp_stapling_live() {
    let Some(dir) = pki2("m7o") else { return };
    let roots = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    for tls in ["-tls1_2", "-tls1_3"] {
        for (resp, want) in [
            (None, "not-stapled"),
            (Some("ocsp_good"), "good"),
            (Some("ocsp_good256"), "good"),
            (Some("ocsp_delegated"), "good-delegated"),
            (Some("ocsp_revoked"), "revoked"),
            (Some("ocsp_bogus"), "bad"),
        ] {
            let mut extra = vec![tls.to_string()];
            if let Some(r) = resp {
                extra.push("-status_file".into());
                extra.push(dir.join(format!("{r}.der")).display().to_string());
            }
            let extra_ref: Vec<&str> = extra.iter().map(|s| s.as_str()).collect();
            let srv = SServer::start(&dir, "p256", &extra_ref);
            let v = WebPkiVerifier { store: &roots, clock: &clock };
            let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
            cfg.enable_tls12();
            let r = Client::connect(&p, &cfg, dial(srv.port));
            let got = match &r {
                Ok(c) => match &c.negotiated().ocsp {
                    OcspStatus::Good { delegated: false, .. } => "good".to_string(),
                    OcspStatus::Good { delegated: true, .. } => "good-delegated".to_string(),
                    OcspStatus::NotStapled => "not-stapled".to_string(),
                    o => format!("{o:?}"),
                },
                Err(TlsError::Certificate(CertError::Revoked)) => "revoked".to_string(),
                Err(TlsError::Certificate(CertError::BadOcspResponse(_))) => "bad".to_string(),
                Err(e) => format!("{e:?}"),
            };
            if let Ok(c) = &r {
                assert_eq!(c.negotiated().version == TLS12, tls == "-tls1_2");
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
            let log = srv.log();
            let server_saw = log.lines().find(|l| l.contains("alert") || l.contains("ALERT")).unwrap_or("").trim().to_string();
            println!("OCSP {tls} {:<15} -> {got:<15} {:?} | s_server: {server_saw}", resp.unwrap_or("(none)"), r.as_ref().err());
            assert_eq!(got, want, "{tls} {resp:?}");
            if want == "revoked" {
                assert!(log.contains("certificate revoked"), "the server saw certificate_revoked: {log}");
            }
            if want == "bad" {
                assert!(log.contains("bad certificate status response"), "the server saw bad_certificate_status_response: {log}");
            }
        }
    }
    // The verifier alone: a staple for ANOTHER certificate (the rsa leaf's chain, p256's response) is refused.
    let path = verify_server_chain_path(&p, &roots, clock_now(), &[der_of(&dir, "rsa"), der_of(&dir, "inter")], None).unwrap();
    let resp = std::fs::read(dir.join("ocsp_good.der")).unwrap();
    let e = tls_core::x509::ocsp::verify_stapled(&p, &path.leaf, &path.leaf_issuer(), &resp, clock_now()).unwrap_err();
    println!("OCSP staple for another serial -> {e:?}");
    assert_eq!(e, CertError::BadOcspResponse("no SingleResponse for this certificate"));
    // ...and a good staple read a week later is stale.
    let path = verify_server_chain_path(&p, &roots, clock_now(), &[der_of(&dir, "p256"), der_of(&dir, "inter")], None).unwrap();
    let e = tls_core::x509::ocsp::verify_stapled(&p, &path.leaf, &path.leaf_issuer(), &resp, clock_now() + 7 * 86_400).unwrap_err();
    assert_eq!(e, CertError::BadOcspResponse("stale: past nextUpdate"));
    let _ = AlertDescription::BadCertificateStatusResponse;
    let _ = std::fs::remove_dir_all(&dir);
}

fn clock_now() -> i64 {
    use tls_core::x509::Clock;
    SystemClock.now()
}

/// The server omits its intermediate (s_server without -cert_chain): refused against the bare root, accepted when
/// the trust store's pool carries the intermediate — over a real handshake.
#[test]
fn missing_intermediate_live() {
    let Some(dir) = pki2("m7m") else { return };
    let p = provider();
    let clock = SystemClock;
    let args: Vec<String> = ["-cert", "leaf2.pem", "-key", "leaf2.key", "-tls1_3"].iter().map(|a| if a.ends_with(".pem") || a.ends_with(".key") { dir.join(a).display().to_string() } else { a.to_string() }).collect();
    for pool in [false, true] {
        let srv = SServer::start_args(&args);
        let st = store(&dir, &["newroot"], if pool { &["inter2"] } else { &[] });
        let v = WebPkiVerifier { store: &st, clock: &clock };
        let cfg = ClientConfig::new(Some("tlscore.test"), &v);
        let r = Client::connect(&p, &cfg, dial(srv.port));
        println!("MISSING-INTERMEDIATE pool={pool}: {:?}", r.as_ref().map(|c| (c.negotiated().peer_chain_len, c.negotiated().pool_intermediates)).map_err(|e| e.clone()));
        if pool {
            let c = r.unwrap();
            assert_eq!((c.negotiated().peer_chain_len, c.negotiated().pool_intermediates), (1, 1));
        } else {
            assert_eq!(r.err(), Some(TlsError::Certificate(CertError::UnknownIssuer)));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// SCTs: embedded in the real public leaves captured for TLSCORE (tests/data/public), and TLS-delivered from
/// s_server's serverinfo (TLS 1.2) — parsed and reported, not verified (no log list: the ceiling).
#[test]
fn scts_parsed_and_reported() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/public");
    let captured: u64 = std::fs::read_to_string(base.join("CAPTURED_AT")).unwrap().trim().parse().unwrap();
    let mut total = 0;
    for host in ["anthropic.com", "index.crates.io", "pypi.org", "raw.githubusercontent.com", "registry.npmjs.org"] {
        let (b, _) = pem_blocks(&std::fs::read_to_string(base.join(format!("{host}.pem"))).unwrap(), "CERTIFICATE");
        let scts = collect_scts(Some(&b[0]), None);
        println!("SCT {host:<26} embedded={} logs={:?}", scts.len(), scts.iter().map(|s| format!("{:02x}{:02x}…@{}", s.log_id[0], s.log_id[1], s.timestamp / 1000)).collect::<Vec<_>>());
        assert!(scts.len() >= 2, "{host}: CA/B policy puts ≥2 SCTs in a public leaf");
        for s in &scts {
            assert_eq!(s.source, SctSource::Embedded);
            assert!(s.timestamp / 1000 < captured && s.timestamp > 1_600_000_000_000, "plausible timestamp");
            assert_eq!(s.hash_alg, 4, "SHA-256");
            assert!(s.signature_len > 60);
        }
        total += scts.len();
    }
    println!("SCT embedded total: {total}");
    // TLS 1.2 extension via s_server -serverinfo: one synthetic v1 SCT.
    let Some(dir) = pki("m7s") else { return };
    let mut sct = vec![0u8];
    sct.extend_from_slice(&[0x11; 32]);
    sct.extend_from_slice(&1_790_000_000_000u64.to_be_bytes());
    sct.extend_from_slice(&[0, 0, 4, 3, 0, 4, 1, 2, 3, 4]);
    let mut list = (sct.len() as u16).to_be_bytes().to_vec();
    list.extend_from_slice(&sct);
    let mut lst = ((list.len()) as u16).to_be_bytes().to_vec();
    lst.extend_from_slice(&list);
    let mut info = 18u16.to_be_bytes().to_vec();
    info.extend_from_slice(&(lst.len() as u16).to_be_bytes());
    info.extend_from_slice(&lst);
    let b64 = base64(&info);
    std::fs::write(dir.join("serverinfo.pem"), format!("-----BEGIN SERVERINFO FOR signed_certificate_timestamp-----\n{b64}\n-----END SERVERINFO FOR signed_certificate_timestamp-----\n")).unwrap();
    let si = dir.join("serverinfo.pem").display().to_string();
    let srv = SServer::start(&dir, "p256", &["-tls1_2", "-serverinfo", &si]);
    let roots = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    let v = WebPkiVerifier { store: &roots, clock: &clock };
    let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
    cfg.enable_tls12();
    let c = Client::connect(&p, &cfg, dial(srv.port)).unwrap_or_else(|e| panic!("{e:?}\n{}", srv.log()));
    let scts = &c.negotiated().scts;
    println!("SCT TLS 1.2 extension: {scts:?}");
    assert_eq!(scts.len(), 1);
    assert_eq!((scts[0].source, scts[0].log_id, scts[0].timestamp), (SctSource::Tls, [0x11; 32], 1_790_000_000_000));
    let _ = std::fs::remove_dir_all(&dir);
}

fn base64(d: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in d.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}
