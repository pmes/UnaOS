//! VEINTLS (LEDGER SR36) — the host oracle for Vein's ring-3 TLS path.
//!
//! vein_ring3's own exchange (`prepare` + `exchange_over`: tls_core's handshake with the Web PKI verifier, then
//! vein_core's Messages encoder, HTTP/1.1 framing and SSE decoder) runs over a std TCP socket against a server
//! that is NOT ours — Python `ssl` = OpenSSL, TLS 1.3 only, on 127.0.0.1 — with a CA + intermediate + leaf
//! generated per run by tls_core's oracle script (openssl CLI). Offline; skipped with a message when python3 or
//! openssl is missing. Crypto: the PRODUCT provider — CRYPTOCORE's `CryptoCoreProvider` (tls_core feature
//! `cryptocore-std`: the ChaCha20 DRBG over /dev/urandom); no third-party crypto anywhere in this test.
//!
//! Proven: a verified handshake (issuer CN reported), the POST reaching the server with the key, version
//! header, Content-Length and a `stream: true` body, the chunked SSE answer decoded to the exact text and stop
//! reason; and the refusals — foreign root, wrong name, unset clock — each end before ONE application byte is
//! written (the server reports `HANDSHAKE-FAIL … app_bytes=0`): the key crosses the wire only verified.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tls_core::cryptocore_provider::CryptoCoreProvider;
use tls_core::x509::{Clock, FixedClock, TrustStore};
use vein_core::claude::{Event, Msg, Params, Stop};
use vein_core::prefs::Endpoint;
use vein_core::Role;
use vein_ring3::{exchange_over, prepare, Buffers, Stage, TlsContext};

struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }
}

/// A std socket as vein_core's Transport (what `net::Tcp` is on the metal).
struct Sock(TcpStream);
impl vein_core::client::Transport for Sock {
    fn send_all(&mut self, b: &[u8]) -> Result<(), i64> {
        self.0.write_all(b).map_err(|_| -32)
    }
    fn recv(&mut self, b: &mut [u8]) -> Result<usize, i64> {
        self.0.read(b).map_err(|_| -104)
    }
}

fn have(cmd: &str, arg: &str) -> bool {
    Command::new(cmd).arg(arg).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn pki(tag: &str) -> Option<PathBuf> {
    if !have("python3", "--version") || !have("openssl", "version") {
        println!("VEINTLS ORACLE SKIPPED: python3 and the openssl CLI are both needed");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("veintls-{}-{}-{}", std::process::id(), tag, SystemClock.now()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tls_core/tests/oracle/gen_certs_openssl.sh");
    let ok = Command::new("sh").arg(script).arg(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success();
    assert!(ok, "PKI generation failed");
    Some(dir)
}

fn store(dir: &Path) -> TrustStore {
    let (s, rep) = vein_ring3::trust::parse(&std::fs::read(dir.join("root.pem")).unwrap()).expect("root parses");
    assert_eq!(rep.loaded, 1);
    s
}

struct Server {
    child: Child,
    out: BufReader<std::process::ChildStdout>,
    port: u16,
}

impl Server {
    fn start(dir: &Path, leaf: &str) -> Server {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/oracle/messages_server.py");
        let mut child = Command::new("python3").arg(script).arg(dir).arg(leaf).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let port = line.trim().parse().expect("port");
        Server { child, out, port }
    }
    fn connect(&self) -> Sock {
        let s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        Sock(s)
    }
    fn result(mut self) -> String {
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        let _ = self.child.wait();
        line.trim().to_string()
    }
}

const KEY: &str = "sk-ant-veintls-test-key-0123456789";
const USER: &str = "is this connection verified?";

/// One full exchange: returns (result, collected text, stop, the server's line).
fn run(dir: &Path, trust: &TrustStore, clock: &dyn Clock, host: &str) -> (Result<vein_ring3::Sent, Stage>, String, Option<Stop>, String) {
    run_leaf(dir, "p256", trust, clock, host)
}

fn run_leaf(dir: &Path, leaf: &str, trust: &TrustStore, clock: &dyn Clock, host: &str) -> (Result<vein_ring3::Sent, Stage>, String, Option<Stop>, String) {
    let srv = Server::start(dir, leaf);
    let provider = CryptoCoreProvider::new();
    let ctx = TlsContext { provider: &provider, store: trust, clock };
    let ep = Endpoint { tls: true, host, port: srv.port, path: "/v1/messages" };
    let mut bufs = Box::new(Buffers::new());
    let p = Params::new("claude-opus-5-5", 1024, "You are tested.");
    let msgs = [Msg { role: Role::User, text: USER }];
    let req = prepare(&ep, &p, msgs.iter().copied(), Some(KEY), &mut bufs).expect("encode");
    let (mut text, mut stop) = (String::new(), None);
    let mut on = |e: Event<'_>| match e {
        Event::Text(t) => text.push_str(t),
        Event::Stop(s) => stop = Some(s),
        _ => {}
    };
    let mut sock = srv.connect();
    let r = exchange_over(&mut sock, &ep, req, &mut bufs, Some(&ctx), &mut on);
    // The head held the key: zeroed whatever happened.
    assert!(bufs.head.iter().all(|&b| b == 0), "request head not zeroed");
    drop(sock);
    (r, text, stop, srv.result())
}

#[test]
fn verified_handshake_and_messages_round_trip() {
    let Some(dir) = pki("ok") else { return };
    let trust = store(&dir);
    let (r, text, stop, srv) = run(&dir, &trust, &SystemClock, "tlscore.test");
    println!("server: {srv}");
    let sent = r.expect("exchange");
    let v = sent.verified.expect("TLS reports the verified issuer");
    println!("client: transport=tls verified={} status={} stop={:?} text_bytes={}", v.issuer(), sent.out.status, stop, sent.out.text_bytes);
    assert_eq!(v.issuer(), "TLSCORE Oracle Intermediate");
    assert_eq!(sent.out.status, 200);
    assert_eq!(stop, Some(Stop::EndTurn));
    assert_eq!(text, format!("Verified hello, \"{USER}\"\n{}", "x".repeat(3000)));
    assert_eq!(sent.out.text_bytes, text.len());
    assert!(srv.starts_with("OK version=TLSv1.3 "), "{srv}");
    for want in [
        "alpn=http/1.1",
        "method=POST",
        "path=/v1/messages",
        &format!("key={KEY}"),
        "version_hdr=2023-06-01",
        "stream=true",
        "model=claude-opus-5-5",
        &format!("user={USER}"),
    ] {
        assert!(srv.contains(want), "server line lacks {want}: {srv}");
    }
    let clen: usize = srv.split("clen=").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
    let got: usize = srv.split("got=").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
    assert_eq!(clen, got);
    assert!(!srv.contains("unwrap="), "close_notify exchange was not clean: {srv}");
}

fn refused(r: &Result<vein_ring3::Sent, Stage>) -> &'static str {
    match r {
        Err(Stage::Handshake(f)) => f.why,
        other => panic!("expected a handshake refusal, got {other:?}"),
    }
}

#[test]
fn foreign_root_is_refused_before_the_key_is_written() {
    let Some(dir) = pki("srv") else { return };
    let Some(other) = pki("foreign") else { return };
    let (r, text, _, srv) = run(&dir, &store(&other), &SystemClock, "tlscore.test");
    println!("client: {:?}  server: {srv}", r);
    assert_eq!(refused(&r), "cert-unknown-issuer");
    assert!(text.is_empty());
    assert!(srv.starts_with("HANDSHAKE-FAIL") && srv.ends_with("app_bytes=0"), "{srv}");
    assert!(srv.contains("UNKNOWN_CA"), "the client's alert reached OpenSSL: {srv}");
}

#[test]
fn wrong_name_is_refused_before_the_key_is_written() {
    let Some(dir) = pki("name") else { return };
    let (r, _, _, srv) = run(&dir, &store(&dir), &SystemClock, "api.anthropic.com");
    println!("client: {:?}  server: {srv}", r);
    assert_eq!(refused(&r), "cert-name-mismatch");
    assert!(srv.starts_with("HANDSHAKE-FAIL") && srv.ends_with("app_bytes=0"), "{srv}");
}

#[test]
fn unset_clock_never_starts_a_handshake() {
    let Some(dir) = pki("clock") else { return };
    let (r, _, _, srv) = run(&dir, &store(&dir), &FixedClock(0), "tlscore.test");
    println!("client: {:?}  server: {srv}", r);
    assert_eq!(refused(&r), "clock-unset");
    assert!(srv.starts_with("HANDSHAKE-FAIL"), "{srv}");
}

#[test]
fn a_key_is_never_encoded_for_plain_http() {
    let ep = Endpoint { tls: false, host: "relay", port: 8080, path: "/v1/messages" };
    let mut bufs = Box::new(Buffers::new());
    let p = Params::new("m", 16, "");
    let msgs = [Msg { role: Role::User, text: "hi" }];
    assert_eq!(prepare(&ep, &p, msgs.iter().copied(), Some(KEY), &mut bufs).unwrap_err(), Stage::KeyOverPlain);
    assert!(prepare(&ep, &p, msgs.iter().copied(), None, &mut bufs).is_ok());
}

/// TLS without a context (a program that could not load provider/trust store) refuses, never falls back.
#[test]
fn no_context_no_tls() {
    let ep = Endpoint { tls: true, host: "x", port: 443, path: "/v1/messages" };
    let mut bufs = Box::new(Buffers::new());
    let p = Params::new("m", 16, "");
    let msgs = [Msg { role: Role::User, text: "hi" }];
    let req = prepare(&ep, &p, msgs.iter().copied(), Some(KEY), &mut bufs).unwrap();
    struct Never;
    impl vein_core::client::Transport for Never {
        fn send_all(&mut self, _: &[u8]) -> Result<(), i64> {
            panic!("nothing may be written without a verified session")
        }
        fn recv(&mut self, _: &mut [u8]) -> Result<usize, i64> {
            panic!("no read either")
        }
    }
    let r = exchange_over(&mut Never, &ep, req, &mut bufs, None, &mut |_| {});
    assert_eq!(refused(&r), "no-tls-context");
    assert!(bufs.head.iter().all(|&b| b == 0));
}

/// The real Mozilla bundle (system/trust/roots.pem, from tools/trust-bundle; gitignored) parses through the
/// same `trust::parse` ring 3 runs, and matches the pinned sha256. Skipped when the bundle is not fetched.
#[test]
fn the_staged_bundle_parses() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../system/trust");
    let Ok(text) = std::fs::read(root.join("roots.pem")) else {
        println!("BUNDLE SKIPPED: system/trust/roots.pem absent (run tools/trust-bundle)");
        return;
    };
    let pin = std::fs::read_to_string(root.join("roots.pem.sha256")).unwrap();
    let pin = pin.split_whitespace().next().unwrap().to_string();
    let out = Command::new("sha256sum").arg(root.join("roots.pem")).output().unwrap();
    let have = String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap().to_string();
    assert_eq!(have, pin, "roots.pem does not match its pinned sha256");
    let (s, rep) = vein_ring3::trust::parse(&text).unwrap();
    println!("bundle: loaded={} rejected={} unsupported_keys={} sha256={pin}", rep.loaded, rep.rejected, rep.unsupported_keys);
    assert!(rep.loaded >= 100 && rep.rejected == 0);
    assert_eq!(s.anchors.len(), rep.loaded);
    assert!(text.len() <= vein_ring3::trust::ROOTS_MAX);
}

/// ONLINE leg (skipped offline): the real api.anthropic.com, no key. Against the Mozilla bundle the only two
/// honest outcomes are (a) VERIFIED and the API answers 401 with its error event (no key was sent), or (b) the
/// path re-terminates TLS (this container's egress gateway does) and the verifier REFUSES with
/// cert-unknown-issuer. Then, reported separately and never in place of Mozilla, the egress path's own CAs
/// (/root/.ccr/ca-bundle.crt, when present) as the trust store: the same client verifies the gateway's chain
/// and the real Messages API answers through it.
#[test]
fn public_api_anthropic_com() {
    let mozilla = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../system/trust/roots.pem");
    let Ok(text) = std::fs::read(&mozilla) else {
        println!("ONLINE SKIPPED: system/trust/roots.pem absent");
        return;
    };
    let Ok(addr) = std::net::ToSocketAddrs::to_socket_addrs(&("api.anthropic.com", 443)).map(|mut a| a.next().unwrap()) else {
        println!("ONLINE SKIPPED: no DNS");
        return;
    };
    let attempt = |trust: &TrustStore| -> Option<(Result<vein_ring3::Sent, Stage>, Vec<String>)> {
        let s = TcpStream::connect_timeout(&addr, Duration::from_secs(8)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let provider = CryptoCoreProvider::new();
        let ctx = TlsContext { provider: &provider, store: trust, clock: &SystemClock };
        let ep = vein_core::prefs::DEFAULT_ENDPOINT;
        let mut bufs = Box::new(Buffers::new());
        let p = Params::new("claude-opus-5-5", 16, "");
        let msgs = [Msg { role: Role::User, text: "hi" }];
        let req = prepare(&ep, &p, msgs.iter().copied(), None, &mut bufs).unwrap();
        let mut errs = Vec::new();
        let r = exchange_over(&mut Sock(s), &ep, req, &mut bufs, Some(&ctx), &mut |e| {
            if let Event::Error(m) = e {
                errs.push(m.to_string())
            }
        });
        Some((r, errs))
    };
    let (moz, _) = vein_ring3::trust::parse(&text).unwrap();
    let Some((r, errs)) = attempt(&moz) else {
        println!("ONLINE SKIPPED: api.anthropic.com:443 unreachable");
        return;
    };
    match &r {
        Ok(sent) => {
            println!("mozilla: VERIFIED issuer={} status={} error={:?}", sent.verified.unwrap().issuer(), sent.out.status, errs);
            assert_eq!(sent.out.status, 401, "no key was sent");
        }
        Err(e) => {
            println!("mozilla: REFUSED {:?} (the path re-terminates TLS)", e);
            assert_eq!(refused(&r), "cert-unknown-issuer");
        }
    }
    let Ok(egress) = std::fs::read("/root/.ccr/ca-bundle.crt") else { return };
    let (eg, rep) = vein_ring3::trust::parse(&egress).unwrap();
    let Some((r, errs)) = attempt(&eg) else { return };
    let sent = r.expect("the egress chain verifies against the egress CAs");
    println!("egress CAs ({} anchors, reported separately): VERIFIED issuer={} status={} error={:?}", rep.loaded, sent.verified.unwrap().issuer(), sent.out.status, errs);
    assert_eq!(sent.out.status, 401, "no key was sent; the API must refuse");
    assert!(!errs.is_empty(), "the API's error body is decoded");
}

/// The RSA and Ed25519 leaves (CertificateVerify rsa_pss_rsae_sha256 / ed25519) through CRYPTOCORE's provider.
#[test]
fn rsa_and_ed25519_leaves_verify() {
    let Some(dir) = pki("leaves") else { return };
    let trust = store(&dir);
    for leaf in ["rsa", "ed25519"] {
        let (r, text, stop, srv) = run_leaf(&dir, leaf, &trust, &SystemClock, "tlscore.test");
        let sent = r.unwrap_or_else(|e| panic!("{leaf}: {e:?} / {srv}"));
        println!("{leaf}: verified={} status={} stop={:?} server: {}", sent.verified.unwrap().issuer(), sent.out.status, stop, &srv[..60.min(srv.len())]);
        assert_eq!(sent.out.status, 200);
        assert_eq!(stop, Some(Stop::EndTurn));
        assert!(text.starts_with("Verified hello"));
        assert!(srv.contains(&format!("key={KEY}")));
    }
}
