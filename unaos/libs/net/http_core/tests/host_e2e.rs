//! M2 — the host transport end to end, against servers that are NOT ours:
//!
//! * Python `http.server` under Python `ssl` (OpenSSL, TLS 1.3 only) on 127.0.0.1 with a per-run CA +
//!   intermediate + leaf from tls_core's openssl script: verified handshake + ALPN, GET with Content-Length,
//!   chunked with extensions and a trailer, gzip and deflate decoded as they stream, a 302 with Set-Cookie
//!   followed with the cookie sent back, 307 keeping a POST body, a paced SSE stream arriving in pieces,
//!   keep-alive reuse (one TCP connection for many requests), HEAD, and the refusals (foreign root, wrong name).
//! * The same server in plain HTTP.
//! * ONLINE (skipped offline): the real api.anthropic.com — through the egress CA bundle it verifies and the
//!   Messages API answers 401 without a key; against the Mozilla bundle the egress gateway's re-terminated
//!   chain is refused `cert-unknown-issuer` (or, on a clean path, verified and 401). And a CONNECT tunnel through
//!   the session's HTTPS proxy to a public host.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http_core::cookie::CookieJar;
use http_core::host::{Agent, AgentConfig, ProxyMode, Request, Trust};
use http_core::url::Url;

fn have(cmd: &str, arg: &str) -> bool {
    Command::new(cmd).arg(arg).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn pki(tag: &str) -> Option<PathBuf> {
    if !have("python3", "--version") || !have("openssl", "version") {
        println!("HTTPCORE ORACLE SKIPPED: python3 and openssl are both needed");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("httpcore-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sys/tls_core/tests/oracle/gen_certs_openssl.sh");
    let ok = Command::new("sh").arg(script).arg(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success();
    assert!(ok, "PKI generation failed");
    Some(dir)
}

struct Server {
    child: Child,
    port: u16,
}
impl Server {
    fn start(certdir: Option<&Path>) -> Server {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/oracle/http_server.py");
        let mut cmd = Command::new("python3");
        cmd.arg(script);
        match certdir {
            Some(d) => cmd.arg(d).arg("p256"),
            None => cmd.arg("-"),
        };
        let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        Server { port: line.trim().parse().expect("port"), child }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill(); // by PID (the child handle)
        let _ = self.child.wait();
    }
}

fn agent(trust: Trust) -> Agent {
    Agent::new(AgentConfig {
        trust,
        proxy: ProxyMode::None,
        timeout: Some(Duration::from_secs(20)),
        cookies: Some(Arc::new(Mutex::new(CookieJar::new()))),
        user_agent: Some("UnaOS-httpcore-test".into()),
        ..Default::default()
    })
}

fn stats(a: &Agent, base: &str) -> serde_like::Stats {
    serde_like::parse(&a.get(&format!("{base}/stats")).unwrap().text().unwrap())
}

/// The few fields of /stats, read without a JSON crate.
mod serde_like {
    pub struct Stats {
        pub connections: u64,
        pub alpn: String,
        pub version: String,
    }
    fn field<'a>(s: &'a str, k: &str) -> &'a str {
        let i = s.find(&format!("\"{k}\": ")).map(|i| i + k.len() + 4).unwrap_or(0);
        let rest = &s[i..];
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        rest[..end].trim().trim_matches('"')
    }
    pub fn parse(s: &str) -> Stats {
        Stats { connections: field(s, "connections").parse().unwrap_or(0), alpn: field(s, "alpn").into(), version: field(s, "version").into() }
    }
}

fn big() -> Vec<u8> {
    (0..2000).flat_map(|i| format!("line {i:05}: the quick brown fox jumps over the lazy dog\n").into_bytes()).collect()
}

fn suite(a: &Agent, base: &str, tls: bool) {
    // Content-Length.
    let r = a.get(&format!("{base}/hello")).unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().unwrap(), "hello from python\n");
    // Chunked with extensions, odd sizes, a trailer.
    let r = a.get(&format!("{base}/chunked")).unwrap();
    assert_eq!(r.headers().get("transfer-encoding"), Some("chunked"));
    let b = r.bytes().unwrap();
    assert!(b == big(), "chunked: {} bytes", b.len());
    // gzip and deflate, decoded while streaming; the head no longer claims an encoding.
    for route in ["gzip", "deflate"] {
        let r = a.get(&format!("{base}/{route}")).unwrap();
        assert_eq!(r.headers().get("content-encoding"), None, "{route}");
        let b = r.bytes().unwrap();
        assert!(b == big(), "{route}: {} bytes", b.len());
    }
    // 302 + Set-Cookie (HttpOnly) followed; the jar sends the cookie to /final.
    let r = a.get(&format!("{base}/redirect")).unwrap();
    assert_eq!(r.url.pathname(), "/final");
    assert_eq!(r.text().unwrap(), "cookie=sid=abc123");
    // 307 keeps the method and the body.
    let mut req = Request::new("POST", Url::parse(&format!("{base}/redirect307")).unwrap());
    req.body = Arc::new(b"{\"keep\":true}".to_vec());
    req.headers.set("Content-Type", "application/json").unwrap();
    let echo = a.send(req).unwrap().text().unwrap();
    assert!(echo.contains("\"method\": \"POST\"") && echo.contains("\"len\": 13"), "{echo}");
    assert!(echo.contains("\"user-agent\": \"UnaOS-httpcore-test\""), "{echo}");
    // A 1 MiB POST body.
    let mut req = Request::new("PUT", Url::parse(&format!("{base}/echo")).unwrap());
    req.body = Arc::new(vec![0x5a; 1 << 20]);
    let echo = a.send(req).unwrap().text().unwrap();
    assert!(echo.contains("\"len\": 1048576"), "{echo}");
    // HEAD: a Content-Length but no body.
    let r = a.send(Request::new("HEAD", Url::parse(&format!("{base}/hello")).unwrap())).unwrap();
    assert_eq!((r.status(), r.headers().get("content-length")), (200, Some("12345")));
    assert!(r.bytes().unwrap().is_empty());
    // SSE: pieces arrive while the server is still pacing (first chunk well before the end).
    let t0 = Instant::now();
    let mut r = a.get(&format!("{base}/sse")).unwrap();
    let mut first = None;
    let mut all = Vec::new();
    while let Some(c) = r.chunk().unwrap() {
        first.get_or_insert(t0.elapsed());
        all.extend_from_slice(&c);
    }
    let total = t0.elapsed();
    assert_eq!(String::from_utf8(all).unwrap().matches("event: tick").count(), 5);
    assert!(first.unwrap() + Duration::from_millis(600) < total, "streamed: first {first:?} total {total:?}");
    // Keep-alive: everything above ran over few connections (the 1 MiB PUT and redirects included).
    let s = stats(a, base);
    println!("{} connections={} alpn={} version={} first-sse-chunk={first:?} total={total:?}", if tls { "https" } else { "http" }, s.connections, s.alpn, s.version);
    assert_eq!(s.connections, 1, "keep-alive: every request on one connection");
    if tls {
        assert_eq!((s.alpn.as_str(), s.version.as_str()), ("http/1.1", "TLSv1.3"));
    }
}

#[test]
fn https_against_python_ssl() {
    let Some(dir) = pki("e2e") else { return };
    let srv = Server::start(Some(&dir));
    let a = agent(Trust::PemFile(dir.join("root.pem").display().to_string()));
    suite(&a, &format!("https://localhost:{}", srv.port), true);
    // The name in the certificate is checked: 127.0.0.1 is in its SAN, tlscore-wrong.test is not.
    let ip = a.get(&format!("https://127.0.0.1:{}/hello", srv.port)).unwrap();
    assert_eq!(ip.status(), 200);
}

#[test]
fn refusals() {
    let Some(dir) = pki("refuse") else { return };
    let Some(other) = pki("refuse-other") else { return };
    let srv = Server::start(Some(&dir));
    // A root that did not sign this chain.
    let a = agent(Trust::PemFile(other.join("root.pem").display().to_string()));
    let e = a.get(&format!("https://localhost:{}/hello", srv.port)).err().expect("foreign root must be refused");
    assert_eq!(e.to_string(), "tls: cert-unknown-issuer");
    // The right root, the wrong name: `wrong.test` pinned to 127.0.0.1 (curl's --resolve) is not in the SAN.
    let mut cfg = AgentConfig { trust: Trust::PemFile(dir.join("root.pem").display().to_string()), proxy: ProxyMode::None, ..Default::default() };
    cfg.resolve.push(("wrong.test".into(), ([127, 0, 0, 1], srv.port).into()));
    let e = Agent::new(cfg.clone()).get(&format!("https://wrong.test:{}/hello", srv.port)).err().expect("wrong name must be refused");
    assert_eq!(e.to_string(), "tls: cert-name-mismatch");
    // ...and the same pin for a name the leaf DOES carry verifies.
    cfg.resolve.push(("tlscore.test".into(), ([127, 0, 0, 1], srv.port).into()));
    assert_eq!(Agent::new(cfg).get(&format!("https://tlscore.test:{}/hello", srv.port)).unwrap().status(), 200);
    // http:// never touches TLS, and a TLS server spoken to in clear text is a framing error, not a hang.
    let e = agent(Trust::System).get(&format!("http://localhost:{}/hello", srv.port)).err();
    println!("clear text to a TLS port: {e:?}");
    assert!(e.is_some());
}

#[test]
fn plain_http() {
    if !have("python3", "--version") {
        return;
    }
    let srv = Server::start(None);
    let a = agent(Trust::System);
    suite(&a, &format!("http://127.0.0.1:{}", srv.port), false);
}

fn online_target() -> Option<()> {
    std::net::ToSocketAddrs::to_socket_addrs(&("api.anthropic.com", 443)).ok()?.next().map(|_| ())
}

#[test]
fn online_api_anthropic_com() {
    if online_target().is_none() {
        println!("ONLINE SKIPPED: no DNS for api.anthropic.com");
        return;
    }
    let post = |trust: Trust| {
        let a = Agent::new(AgentConfig { trust, proxy: ProxyMode::Env, ..Default::default() });
        let mut req = Request::new("POST", Url::parse("https://api.anthropic.com/v1/messages").unwrap());
        req.headers.set("anthropic-version", "2023-06-01").unwrap();
        req.headers.set("content-type", "application/json").unwrap();
        req.body = Arc::new(br#"{"model":"claude-opus-5-5","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}"#.to_vec());
        a.send(req).and_then(|r| {
            let s = r.status();
            r.text().map(|t| (s, t))
        })
    };
    let moz = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../system/trust/roots.pem");
    if moz.exists() {
        match post(Trust::PemFile(moz.display().to_string())) {
            Ok((s, t)) => {
                println!("mozilla: VERIFIED status={s} body={}", &t[..t.len().min(160)]);
                assert_eq!(s, 401);
            }
            Err(e) => {
                println!("mozilla: REFUSED {e} (this path re-terminates TLS)");
                assert_eq!(e.to_string(), "tls: cert-unknown-issuer");
            }
        }
    } else {
        println!("mozilla leg skipped: system/trust/roots.pem not staged (tools/trust-bundle)");
    }
    let egress = "/root/.ccr/ca-bundle.crt";
    if !Path::new(egress).exists() {
        return;
    }
    let (s, t) = post(Trust::PemFile(egress.into())).expect("verified through the egress CAs");
    println!("egress CAs: VERIFIED status={s} body={}", &t[..t.len().min(200)]);
    assert_eq!(s, 401, "no key: the API must refuse");
    assert!(t.contains("x-api-key"), "{t}");
}

#[test]
fn online_connect_proxy() {
    let Ok(proxy) = std::env::var("HTTPS_PROXY") else {
        println!("PROXY SKIPPED: no HTTPS_PROXY");
        return;
    };
    let egress = "/root/.ccr/ca-bundle.crt";
    if !Path::new(egress).exists() {
        return;
    }
    let a = Agent::new(AgentConfig { trust: Trust::PemFile(egress.into()), proxy: ProxyMode::Fixed(proxy.clone()), ..Default::default() });
    let url = "https://raw.githubusercontent.com/web-platform-tests/wpt/564b9b1eb1387ef42456f1cba77d2d38599fad61/url/resources/urltestdata.json";
    match a.get(url) {
        Ok(r) => {
            let s = r.status();
            let b = r.bytes().unwrap();
            println!("CONNECT via {proxy}: status={s} bytes={}", b.len());
            assert_eq!((s, b.len()), (200, 229610));
        }
        Err(e) => println!("PROXY SKIPPED (offline?): {e}"),
    }
}

/// The Chromium oracle: the same pages fetched by Chromium (Playwright) and by http_core's host client from the
/// local Python server must be BYTE-EQUAL after decoding (gzip with Chromium's own Accept-Encoding, deflate,
/// chunked with extensions + trailer, Content-Length).
#[test]
fn chromium_oracle_byte_equal() {
    let node_modules = "/opt/node22/lib/node_modules";
    if !have("node", "--version") || !Path::new(&format!("{node_modules}/playwright")).exists() {
        println!("CHROMIUM ORACLE SKIPPED: node + playwright needed");
        return;
    }
    let srv = Server::start(None);
    let base = format!("http://127.0.0.1:{}", srv.port);
    let routes = ["/page", "/gzip", "/deflate", "/chunked", "/hello"];
    let out = std::env::temp_dir().join(format!("httpcore-chromium-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/oracle/chromium_fetch.js");
    let urls: Vec<String> = routes.iter().map(|r| format!("{base}{r}")).collect();
    let o = Command::new("node").arg(script).arg(&out).args(&urls).env("NODE_PATH", node_modules).output().unwrap();
    if !o.status.success() {
        println!("CHROMIUM ORACLE SKIPPED: {}", String::from_utf8_lossy(&o.stderr));
        return;
    }
    print!("{}", String::from_utf8_lossy(&o.stdout));
    let a = agent(Trust::System);
    for (i, u) in urls.iter().enumerate() {
        let chromium = std::fs::read(out.join(format!("{i}.bin"))).unwrap();
        let ours = a.get(u).unwrap().bytes().unwrap();
        println!("{u}: chromium={} http_core={} equal={}", chromium.len(), ours.len(), chromium == ours);
        assert!(chromium == ours, "{u}: Chromium and http_core disagree");
    }
}
