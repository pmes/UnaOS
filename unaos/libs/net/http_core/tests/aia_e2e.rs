//! CTCORE (SR60) M2 — the AIA caIssuers seam on the host: `openssl s_server -www` presents a leaf WITHOUT its
//! intermediate; the leaf's authorityInfoAccess names `http://127.0.0.1:<port>/inter.p7c`, served by a plain HTTP
//! file server here. An Agent with `aia: true` (the default) fetches it through http_core and the GET answers 200;
//! with `aia: false` the same connection is refused `cert-unknown-issuer`.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use http_core::host::{Agent, AgentConfig, ProxyMode, Trust};

fn have(cmd: &str, arg: &str) -> bool {
    Command::new(cmd).arg(arg).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill(); // by PID (the child handle)
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn aia_fetch_through_http_core() {
    if !have("openssl", "version") {
        println!("AIA E2E SKIPPED: openssl needed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("httpcore-aia-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sys/tls_core/tests/oracle/gen_certs_openssl.sh");
    assert!(Command::new("sh").arg(script).arg(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success());
    // The caIssuers server.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let http_port = l.local_addr().unwrap().port();
    let files = dir.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut b = [0u8; 2048];
            let n = s.read(&mut b).unwrap_or(0);
            let req = String::from_utf8_lossy(&b[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/').to_string();
            match std::fs::read(files.join(path)) {
                Ok(body) => {
                    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/pkcs7-mime\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                    let _ = s.write_all(&body);
                }
                Err(_) => {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
            }
        }
    });
    let sh = |c: &str| assert!(Command::new("sh").arg("-c").arg(c).current_dir(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success(), "{c}");
    sh("openssl crl2pkcs7 -nocrl -certfile inter.pem -outform DER -out inter.p7c");
    sh(&format!(
        "printf 'basicConstraints=critical,CA:FALSE\\nkeyUsage=critical,digitalSignature\\nextendedKeyUsage=serverAuth\\nsubjectAltName=DNS:tlscore.test\\nauthorityInfoAccess=caIssuers;URI:http://127.0.0.1:{http_port}/inter.p7c\\n' > aia.ext && \
         openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out aia.key && openssl req -new -key aia.key -subj /CN=tlscore.test -out aia.csr && \
         openssl x509 -req -in aia.csr -CA inter.pem -CAkey inter.key -CAcreateserial -days 5 -sha256 -extfile aia.ext -out aia.pem"
    ));
    let tls_port = free_port();
    let srv = Kill(
        Command::new("openssl")
            .args(["s_server", "-accept", &format!("127.0.0.1:{tls_port}"), "-cert", "aia.pem", "-key", "aia.key", "-www", "-tls1_3"])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(Duration::from_millis(600));
    let addr: SocketAddr = format!("127.0.0.1:{tls_port}").parse().unwrap();
    let mk = |aia: bool| {
        Agent::new(AgentConfig {
            trust: Trust::PemFile(dir.join("root.pem").display().to_string()),
            proxy: ProxyMode::None,
            timeout: Some(Duration::from_secs(10)),
            resolve: vec![("tlscore.test".into(), addr)],
            aia,
            ..Default::default()
        })
    };
    let url = format!("https://tlscore.test:{tls_port}/");
    let off = mk(false).get(&url).map(|r| r.status()).map_err(|e| format!("{e:?}"));
    let on = mk(true).get(&url).map(|r| r.status()).map_err(|e| format!("{e:?}"));
    println!("AIA http_core: aia=false → {off:?}; aia=true → {on:?}");
    assert!(off.as_ref().is_err_and(|e| e.contains("cert-unknown-issuer")), "{off:?}");
    assert_eq!(on, Ok(200));
    drop(srv);
    let _ = std::fs::remove_dir_all(&dir);
}
