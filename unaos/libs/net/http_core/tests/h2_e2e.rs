//! M4 interop — http_core's HTTP/2 client, negotiated by ALPN `h2` over tls_core, against Node's built-in
//! `http2` server (nghttp2 underneath, Node's OpenSSL, TLS 1.3, h2 only): a GET, a 1 MiB body through our
//! WINDOW_UPDATEs, a gzip body decoded, a 300 KB POST that must wait for the server's 64 KiB windows to reopen,
//! a 302 followed, a response with trailers — all on ONE connection (Node's session count is the proof).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use http_core::host::{Agent, AgentConfig, ProxyMode, Request, Trust};
use http_core::url::Url;

fn have(cmd: &str, arg: &str) -> bool {
    Command::new(cmd).arg(arg).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

struct Node(Child, u16);
impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn sha256(b: &[u8]) -> String {
    let p = std::env::temp_dir().join(format!("h2e2e-{}.bin", std::process::id()));
    std::fs::write(&p, b).unwrap();
    let o = Command::new("sha256sum").arg(&p).output().unwrap();
    String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap().to_string()
}

#[test]
fn h2_against_node_http2() {
    if !have("node", "--version") || !have("openssl", "version") {
        println!("H2 ORACLE SKIPPED: node and openssl needed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("httpcore-h2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pki_script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sys/tls_core/tests/oracle/gen_certs_openssl.sh");
    assert!(Command::new("sh").arg(pki_script).arg(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success());
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/oracle/h2_server.js");
    let mut child = Command::new("node").arg(script).arg(&dir).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let node = Node(child, line.trim().parse().unwrap());
    let base = format!("https://localhost:{}", node.1);
    let a = Agent::new(AgentConfig {
        trust: Trust::PemFile(Path::new(&dir).join("root.pem").display().to_string()),
        proxy: ProxyMode::None,
        alpn: vec![b"h2".to_vec()],
        ..Default::default()
    });

    let r = a.get(&format!("{base}/hello")).unwrap();
    assert_eq!((r.status(), r.head.version), (200, (2, 0)));
    assert_eq!(r.text().unwrap(), "hello over h2\n");

    let big: Vec<u8> = (0..1usize << 20).map(|i| ((i * 31 + (i >> 8)) & 0xff) as u8).collect();
    let r = a.get(&format!("{base}/big")).unwrap();
    let got = r.bytes().unwrap();
    assert!(got == big, "1 MiB body: {} bytes", got.len());

    let r = a.get(&format!("{base}/gzip")).unwrap();
    assert_eq!(r.headers().get("content-encoding"), None);
    assert!(r.bytes().unwrap() == big[..100_000]);

    let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut req = Request::new("POST", Url::parse(&format!("{base}/echo?x=1")).unwrap());
    req.headers.set("X-Api-Key", "sk-h2-test").unwrap();
    req.headers.set("Content-Type", "application/octet-stream").unwrap();
    req.body = Arc::new(body.clone());
    let echo = a.send(req).unwrap().text().unwrap();
    println!("echo: {}", &echo[..echo.len().min(400)]);
    assert!(echo.contains("\"method\":\"POST\"") && echo.contains("\"len\":300000"), "{echo}");
    assert!(echo.contains(&format!("\"sha256\":\"{}\"", sha256(&body))), "{echo}");
    assert!(echo.contains("\"x-api-key\":\"sk-h2-test\""), "{echo}");
    assert!(echo.contains("\":authority\":\"localhost:"), "{echo}");

    let r = a.get(&format!("{base}/redirect")).unwrap();
    assert_eq!(r.url.pathname(), "/hello");
    assert_eq!(r.text().unwrap(), "hello over h2\n");

    let r = a.get(&format!("{base}/trailers")).unwrap();
    assert_eq!(r.text().unwrap(), "body");

    let stats = a.get(&format!("{base}/stats")).unwrap().text().unwrap();
    println!("node http2 stats: {stats}");
    assert!(stats.contains("\"sessions\":1"), "one h2 connection carried every stream: {stats}");
}
