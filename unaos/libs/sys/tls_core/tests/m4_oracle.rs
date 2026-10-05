//! M4 — ORACLE. A real TLS 1.3 server that is not ours (Python `ssl` = OpenSSL) on localhost, with a CA + chain
//! generated here (Python `cryptography` if usable, else the `openssl` CLI; skipped with a message if neither),
//! and — when online — three public hosts verified against the Mozilla bundle.

mod common;
use common::*;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tls_core::error::{CertError, TlsError};
use tls_core::msgs::{CipherSuite, NamedGroup, SignatureScheme};
use tls_core::test_provider::RustCryptoProvider;
use tls_core::x509::{Clock, TrustStore, WebPkiVerifier};
use tls_core::{Client, ClientConfig};

struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }
}

fn oracle_dir() -> String {
    format!("{}/tests/oracle", env!("CARGO_MANIFEST_DIR"))
}

fn ok(cmd: &mut Command) -> bool {
    cmd.stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// Generates the oracle PKI into a fresh temp dir. Returns (dir, generator used) or None (skip).
fn make_pki() -> Option<(std::path::PathBuf, &'static str)> {
    let dir = std::env::temp_dir().join(format!("tlscore-oracle-{}-{}", std::process::id(), rand_tag()));
    std::fs::create_dir_all(&dir).unwrap();
    let py_ok = ok(Command::new("python3").args(["-c", "from cryptography import x509; from cryptography.hazmat.primitives.asymmetric import ed25519"]));
    if py_ok && ok(Command::new("python3").arg(format!("{}/gen_certs.py", oracle_dir())).arg(&dir)) {
        return Some((dir, "python cryptography"));
    }
    let ossl_ok = ok(Command::new("openssl").arg("version"));
    if ossl_ok && ok(Command::new("sh").arg(format!("{}/gen_certs_openssl.sh", oracle_dir())).arg(&dir)) {
        return Some((dir, "openssl CLI (python cryptography unusable here)"));
    }
    println!("ORACLE SKIPPED: neither Python `cryptography` nor the `openssl` CLI is usable");
    None
}

fn rand_tag() -> u64 {
    let mut b = [0u8; 8];
    use tls_core::crypto::CryptoProvider;
    RustCryptoProvider::new().random(&mut b).unwrap();
    u64::from_le_bytes(b)
}

struct Server {
    child: Child,
    out: BufReader<std::process::ChildStdout>,
    port: u16,
}

impl Server {
    fn start(dir: &std::path::Path, leaf: &str, groups: &str) -> Server {
        let mut child = Command::new("python3")
            .arg(format!("{}/server.py", oracle_dir()))
            .arg(dir)
            .arg(leaf)
            .arg(groups)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("python3");
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let port = line.trim().parse().expect("port");
        Server { child, out, port }
    }
    /// The server's own account of the connection.
    fn verdict(mut self) -> String {
        let mut line = String::new();
        let _ = self.out.read_line(&mut line);
        let _ = self.child.wait();
        line.trim().to_string()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill(); // by PID (Child handle)
        let _ = self.child.wait();
    }
}

struct Case {
    leaf: &'static str,
    groups: &'static str,
    suites: Vec<CipherSuite>,
    key_update: bool,
    body: usize,
    resp: usize,
    max_fragment: usize,
}

fn run_case(dir: &std::path::Path, store: &TrustStore, c: &Case) -> (tls_core::Negotiated, String, Vec<u8>, u32) {
    let srv = Server::start(dir, c.leaf, c.groups);
    let tcp = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let p = RustCryptoProvider::new();
    let clock = SystemClock;
    let v = WebPkiVerifier { store, clock: &clock };
    let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
    cfg.alpn = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    cfg.cipher_suites = c.suites.clone();
    cfg.max_fragment = c.max_fragment;
    let mut cl = Client::connect(&p, &cfg, Tcp(tcp)).unwrap_or_else(|e| panic!("handshake with OpenSSL failed: {e:?}"));
    let n = cl.negotiated().clone();
    if c.key_update {
        cl.send_key_update(true).unwrap();
    }
    let body = vec![b'B'; c.body];
    let req = format!("POST /size/{} HTTP/1.1\r\nHost: tlscore.test\r\nContent-Length: {}\r\n\r\n", c.resp, c.body);
    cl.send(&[req.as_bytes(), &body].concat()).unwrap();
    let mut resp = Vec::new();
    while let Some(chunk) = cl.recv().unwrap() {
        resp.extend_from_slice(&chunk);
    }
    cl.close().unwrap();
    let kus = cl.key_updates_received;
    drop(cl);
    (n, srv.verdict(), resp, kus)
}

#[test]
fn oracle_local_openssl_server() {
    let Some((dir, generator)) = make_pki() else { return };
    println!("ORACLE PKI generated with: {generator}");
    let (store, rep) = TrustStore::from_pem(&std::fs::read_to_string(dir.join("root.pem")).unwrap());
    assert_eq!(rep.loaded, 1);
    let all = CipherSuite::ALL.to_vec();
    let cases = [
        // Each suite alone, P-256 leaf, KeyUpdate both ways, ALPN.
        Case { leaf: "p256", groups: "any", suites: vec![CipherSuite::Aes128GcmSha256], key_update: true, body: 0, resp: 0, max_fragment: 16384 },
        Case { leaf: "p256", groups: "any", suites: vec![CipherSuite::Aes256GcmSha384], key_update: true, body: 0, resp: 0, max_fragment: 16384 },
        Case { leaf: "p256", groups: "any", suites: vec![CipherSuite::ChaCha20Poly1305Sha256], key_update: true, body: 0, resp: 0, max_fragment: 16384 },
        // HelloRetryRequest: the server only takes P-256, we lead with x25519.
        Case { leaf: "p256", groups: "p256", suites: all.clone(), key_update: false, body: 0, resp: 0, max_fragment: 16384 },
        // Ed25519 and RSA (rsa_pss_rsae CertificateVerify) leaves.
        Case { leaf: "ed25519", groups: "any", suites: all.clone(), key_update: false, body: 0, resp: 0, max_fragment: 16384 },
        Case { leaf: "rsa", groups: "any", suites: all.clone(), key_update: false, body: 0, resp: 0, max_fragment: 16384 },
        // Bulk: 100 000-byte request in 1000-byte records, 300 000-byte response (multi-record reads).
        Case { leaf: "p256", groups: "any", suites: all.clone(), key_update: true, body: 100_000, resp: 300_000, max_fragment: 1000 },
    ];
    for c in &cases {
        let (n, verdict, resp, kus) = run_case(&dir, &store, c);
        let text = String::from_utf8_lossy(&resp[..resp.len().min(400)]).to_string();
        println!(
            "ORACLE {:>7} groups={:<4} -> {:?} {:?} {:?} hrr={} alpn={:?} keyupdates_rx={} resp={}B | server: {}",
            c.leaf, c.groups, n.cipher_suite, n.group, n.signature_scheme, n.hello_retry,
            n.alpn.as_deref().map(String::from_utf8_lossy), kus, resp.len(), verdict
        );
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
        assert!(verdict.starts_with("OK version=TLSv1.3"), "server verdict: {verdict}");
        assert!(verdict.contains(&format!("got={}", c.body)), "server got the whole body: {verdict}");
        assert!(!verdict.contains("unwrap="), "clean close_notify exchange: {verdict}");
        assert_eq!(n.alpn.as_deref(), Some(&b"http/1.1"[..]));
        let ossl_name = match n.cipher_suite {
            CipherSuite::Aes128GcmSha256 => "TLS_AES_128_GCM_SHA256",
            CipherSuite::Aes256GcmSha384 => "TLS_AES_256_GCM_SHA384",
            CipherSuite::ChaCha20Poly1305Sha256 => "TLS_CHACHA20_POLY1305_SHA256",
        };
        assert!(verdict.contains(ossl_name), "both ends agree on the suite: {verdict}");
        if c.suites.len() == 1 {
            assert_eq!(n.cipher_suite, c.suites[0]);
        }
        if c.groups == "p256" {
            assert!(n.hello_retry && n.group == NamedGroup::Secp256r1, "HRR to P-256");
        }
        match c.leaf {
            "ed25519" => assert_eq!(n.signature_scheme, SignatureScheme::Ed25519),
            "rsa" => assert!(matches!(n.signature_scheme, SignatureScheme::RsaPssRsaeSha256 | SignatureScheme::RsaPssRsaeSha384 | SignatureScheme::RsaPssRsaeSha512)),
            _ => assert_eq!(n.signature_scheme, SignatureScheme::EcdsaSecp256r1Sha256),
        }
        if c.key_update {
            assert!(kus >= 1, "OpenSSL answered our update_requested KeyUpdate");
        }
        if c.resp > 0 {
            let body_at = resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            let payload = &resp[body_at..];
            let nl = payload.iter().position(|&b| b == b'\n').unwrap() + 1;
            let expect: Vec<u8> = (0..c.resp).map(|i| ((i * 7) & 0xff) as u8).collect();
            assert_eq!(&payload[nl..], &expect[..], "300 KB response byte-exact");
        }
    }

    // Refusals, seen from both ends: a trust store without our root, and a name the leaf does not carry.
    // The "wrong" store holds a real root — the M3 fixture root — that did not issue this chain.
    let mut wrong = TrustStore::new();
    wrong.add_der(&std::fs::read(format!("{}/tests/data/x509/root.der", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    for (store, name, want) in [
        (&wrong, "tlscore.test", TlsError::Certificate(CertError::UnknownIssuer)),
        (&store, "other.test", TlsError::Certificate(CertError::NameMismatch)),
    ] {
        let srv = Server::start(&dir, "p256", "any");
        let tcp = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        let p = RustCryptoProvider::new();
        let clock = SystemClock;
        let v = WebPkiVerifier { store, clock: &clock };
        let cfg = ClientConfig::new(Some(name), &v);
        let e = Client::connect(&p, &cfg, Tcp(tcp)).err().expect("must refuse");
        let verdict = srv.verdict();
        println!("ORACLE refusal {name}: client {e:?} | server: {verdict}");
        assert_eq!(e, want);
        assert!(verdict.starts_with("HANDSHAKE-FAIL"), "server saw our alert: {verdict}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- public hosts

/// Opens a TCP stream to host:443, through $HTTPS_PROXY (HTTP CONNECT) when one is set.
fn dial(host: &str) -> Result<(TcpStream, Option<String>), String> {
    let no_proxy = std::env::var("NO_PROXY").or_else(|_| std::env::var("no_proxy")).unwrap_or_default();
    let bypass = no_proxy.split(',').any(|d| {
        let d = d.trim().trim_start_matches("*.").trim_start_matches('.');
        !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))
    });
    let proxy = if bypass { None } else { std::env::var("HTTPS_PROXY").or_else(|_| std::env::var("https_proxy")).ok() };
    match proxy {
        None => TcpStream::connect((host, 443)).map(|s| (s, None)).map_err(|e| e.to_string()),
        Some(p) => {
            let hp = p.trim_start_matches("http://").trim_end_matches('/').to_string();
            let mut s = TcpStream::connect(&hp).map_err(|e| e.to_string())?;
            s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            write!(s, "CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n").map_err(|e| e.to_string())?;
            let mut resp = Vec::new();
            let mut b = [0u8; 1];
            while !resp.ends_with(b"\r\n\r\n") {
                if s.read(&mut b).map_err(|e| e.to_string())? == 0 {
                    return Err("proxy closed".into());
                }
                resp.push(b[0]);
            }
            let status = String::from_utf8_lossy(&resp).lines().next().unwrap_or("").to_string();
            if !status.contains(" 200") {
                return Err(format!("proxy CONNECT refused: {status}"));
            }
            Ok((s, Some(hp)))
        }
    }
}

/// Delegates to the Web PKI verifier and keeps what the server presented, so a refusal can say what it refused.
struct Recording<'a> {
    inner: WebPkiVerifier<'a>,
    seen: std::cell::RefCell<Vec<Vec<u8>>>,
}
impl tls_core::ServerCertVerifier for Recording<'_> {
    fn verify_server_cert(&self, p: &dyn tls_core::CryptoProvider, chain: &[Vec<u8>], n: Option<&str>) -> Result<tls_core::x509::PublicKey, TlsError> {
        *self.seen.borrow_mut() = chain.to_vec();
        self.inner.verify_server_cert(p, chain, n)
    }
}

/// "subject CN <- issuer CN" for each presented certificate.
fn describe(chain: &[Vec<u8>]) -> String {
    chain
        .iter()
        .map(|d| match tls_core::x509::Certificate::parse(d) {
            Ok(c) => format!("[{}]", c.subject_cn.clone().unwrap_or_else(|| "?".into())),
            Err(e) => format!("[unparsed {e:?}]"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn get_rec(host: &str, store: &TrustStore) -> Result<(tls_core::Negotiated, String), (TlsError, Vec<Vec<u8>>)> {
    let r = std::cell::RefCell::new(Vec::new());
    let out = get_inner(host, store, &r);
    out.map_err(|e| (e, r.into_inner()))
}

fn get_inner(host: &str, store: &TrustStore, seen: &std::cell::RefCell<Vec<Vec<u8>>>) -> Result<(tls_core::Negotiated, String), TlsError> {
    let (tcp, _) = dial(host).map_err(|_| TlsError::Transport)?;
    tcp.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let p = RustCryptoProvider::new();
    let clock = SystemClock;
    let v = Recording { inner: WebPkiVerifier { store, clock: &clock }, seen: std::cell::RefCell::new(Vec::new()) };
    let mut cfg = ClientConfig::new(Some(host), &v);
    cfg.alpn = vec![b"http/1.1".to_vec()];
    let conn = Client::connect(&p, &cfg, Tcp(tcp));
    *seen.borrow_mut() = v.seen.borrow().clone();
    let mut c = conn?;
    let n = c.negotiated().clone();
    c.send(format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: unaos-tlscore\r\n\r\n").as_bytes())?;
    let mut resp = Vec::new();
    loop {
        match c.recv() {
            Ok(Some(d)) => resp.extend_from_slice(&d),
            Ok(None) => break,
            // Many servers (and the egress proxy) close or reset the TCP stream after the response instead of
            // sending close_notify; the response already arrived authenticated, so it stands — reported as such.
            Err(TlsError::UnexpectedEof | TlsError::Transport) if !resp.is_empty() => break,
            Err(e) => return Err(e),
        }
        if resp.len() > 64 * 1024 {
            break;
        }
    }
    let status = String::from_utf8_lossy(&resp).lines().next().unwrap_or("").to_string();
    Ok((n, status))
}

#[test]
fn oracle_public_hosts() {
    let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../system/trust/roots.pem");
    let Ok(text) = std::fs::read_to_string(&bundle) else {
        println!("PUBLIC SKIPPED: system/trust/roots.pem absent (run tools/trust-bundle)");
        return;
    };
    let (mozilla, rep) = TrustStore::from_pem(&text);
    println!("PUBLIC Mozilla bundle: {rep:?}");
    // When a re-terminating egress proxy sits in the path, its CA is the only anchor that can verify what we are
    // actually talking to; it is tried SECOND and reported as such — never in place of the Mozilla verdict.
    // The anchors are the egress path's own CAs, picked by name out of the container's bundle — Mozilla roots excluded.
    let proxy_store = ["/root/.ccr/ca-bundle.crt", "/root/.ccr/agent-proxy-ca.crt"].iter().find_map(|p| std::fs::read_to_string(p).ok()).map(|t| {
        let mut st = TrustStore::new();
        for der in tls_core::x509::pem::pem_blocks(&t, "CERTIFICATE").0 {
            let cn = tls_core::x509::Certificate::parse(&der).ok().and_then(|c| c.subject_cn).unwrap_or_default();
            if ["Egress Gateway CA", "TLS Inspection CA", "agent-proxy interception CA", "Upstream Proxy CA"].iter().any(|k| cn.contains(k)) {
                let _ = st.add_der(&der);
            }
        }
        st
    });
    if let Some(ps) = &proxy_store {
        println!("PUBLIC egress-path CAs (not Mozilla, reported separately): {}", ps.anchors.len());
    }
    // The three named hosts, then extra reachable hosts for chain diversity (marked +).
    for host in ["example.com", "cloudflare.com", "anthropic.com", "+www.anthropic.com", "+github.com", "+raw.githubusercontent.com", "+pypi.org", "+index.crates.io", "+registry.npmjs.org"] {
        let host = host.trim_start_matches('+');
        match get_rec(host, &mozilla) {
            Ok((n, status)) => println!("PUBLIC {host}: Mozilla bundle -> VERIFIED {:?} {:?} {:?} chain={} | {status}", n.cipher_suite, n.group, n.signature_scheme, n.peer_chain_len),
            Err((TlsError::Transport, _)) => {
                println!("PUBLIC {host}: unreachable ({})", dial(host).err().unwrap_or_default());
                continue;
            }
            Err((e, chain)) => println!("PUBLIC {host}: Mozilla bundle -> REFUSED {e:?} presented: {}", describe(&chain)),
        }
        if let Some(ps) = &proxy_store {
            match get_rec(host, ps) {
                Ok((n, status)) => println!("PUBLIC {host}: egress-proxy CA -> VERIFIED {:?} {:?} {:?} | {status}", n.cipher_suite, n.group, n.signature_scheme),
                Err((e, chain)) => println!("PUBLIC {host}: egress-proxy CA -> REFUSED {e:?} presented: {}", describe(&chain)),
            }
        }
    }
}

// ---------------------------------------------------------------- captured public chains, offline

/// The real Web PKI chains of five hosts, captured with `openssl s_client -showcerts` through the egress proxy on
/// the day of M4 (tests/data/public/, ~30 KB), validated at the capture instant against the Mozilla bundle — by
/// tls_core AND by `openssl verify` on the same bytes, which must agree. Also a wrong name must be refused.
#[test]
fn oracle_captured_public_chains() {
    let base = format!("{}/tests/data/public", env!("CARGO_MANIFEST_DIR"));
    let bundle_path = format!("{}/../../../../system/trust/roots.pem", env!("CARGO_MANIFEST_DIR"));
    let Ok(text) = std::fs::read_to_string(&bundle_path) else {
        println!("CAPTURED SKIPPED: system/trust/roots.pem absent (run tools/trust-bundle)");
        return;
    };
    let (mozilla, _) = TrustStore::from_pem(&text);
    let at: i64 = std::fs::read_to_string(format!("{base}/CAPTURED_AT")).unwrap().trim().parse().unwrap();
    let have_openssl = ok(Command::new("openssl").arg("version"));
    let p = RustCryptoProvider::new();
    for host in ["pypi.org", "index.crates.io", "registry.npmjs.org", "anthropic.com", "raw.githubusercontent.com"] {
        let file = format!("{base}/{host}.pem");
        let (chain, bad) = tls_core::x509::pem::pem_blocks(&std::fs::read_to_string(&file).unwrap(), "CERTIFICATE");
        assert_eq!(bad, 0);
        let ours = tls_core::x509::verify::verify_server_chain(&p, &mozilla, at, &chain, Some(host));
        let wrong = tls_core::x509::verify::verify_server_chain(&p, &mozilla, at, &chain, Some("not-this-host.example"));
        let ossl = if have_openssl {
            // openssl verify: leaf = first block, everything after it untrusted, the bundle as the only anchors.
            let dir = std::env::temp_dir().join(format!("tlscore-cap-{}-{host}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let pem = |d: &[u8]| tls_core_test_pem(d);
            std::fs::write(dir.join("leaf.pem"), pem(&chain[0])).unwrap();
            std::fs::write(dir.join("rest.pem"), chain[1..].iter().map(|d| pem(d)).collect::<String>()).unwrap();
            let out = Command::new("openssl")
                .args(["verify", "-x509_strict", "-purpose", "sslserver", "-verify_hostname", host, "-attime", &at.to_string(), "-CAfile", &bundle_path, "-untrusted"])
                .arg(dir.join("rest.pem"))
                .arg(dir.join("leaf.pem"))
                .output()
                .unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            Some(out.status.success())
        } else {
            None
        };
        println!("CAPTURED {host}: tls_core={:?} openssl={:?} wrong-name={:?}", ours.as_ref().map(|_| "OK"), ossl, wrong.as_ref().err());
        assert!(ours.is_ok(), "{host}: {:?}", ours.err());
        if let Some(o) = ossl {
            assert!(o, "{host}: openssl disagrees");
        }
        assert_eq!(wrong.err(), Some(CertError::NameMismatch));
    }
}

fn tls_core_test_pem(der: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in der.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    let body: Vec<String> = s.as_bytes().chunks(64).map(|l| String::from_utf8(l.to_vec()).unwrap()).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", body.join("\n"))
}
