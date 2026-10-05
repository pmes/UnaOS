#![allow(dead_code)]
//! TLSCORE2 test support: the per-run PKI (openssl CLI), the Python `ssl` oracle server (tests/oracle/tls_server.py),
//! a TCP transport, the system clock, and a byte-rewriting man-in-the-middle for the negative tests. Every test that
//! uses it runs on the PRODUCT provider (CRYPTOCORE).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tls_core::cryptocore_provider::CryptoCoreProvider;
use tls_core::error::TlsError;
use tls_core::x509::{Clock, TrustStore};
use tls_core::Transport;

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }
}

pub fn provider() -> CryptoCoreProvider {
    CryptoCoreProvider::new()
}

pub fn oracle_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/oracle")
}

fn quiet_ok(cmd: &mut Command) -> bool {
    cmd.stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

pub fn have_tools() -> bool {
    quiet_ok(Command::new("python3").arg("--version")) && quiet_ok(Command::new("openssl").arg("version"))
}

/// A fresh temp dir with the TLSCORE oracle PKI (root P-256 → intermediate → p256/ed25519/rsa leaves), or None (skip).
pub fn pki(tag: &str) -> Option<PathBuf> {
    if !have_tools() {
        println!("TLSCORE2 ORACLE SKIPPED: python3 and openssl are both needed");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("tlscore2-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(quiet_ok(Command::new("sh").arg(oracle_dir().join("gen_certs_openssl.sh")).arg(&dir)), "PKI generation failed");
    Some(dir)
}

pub fn store_of(pem_path: &Path) -> TrustStore {
    let (s, rep) = TrustStore::from_pem(&std::fs::read_to_string(pem_path).unwrap());
    assert!(rep.loaded >= 1);
    s
}

/// tests/oracle/tls_server.py, serving `conns` connections.
pub struct PyServer {
    child: Child,
    out: BufReader<std::process::ChildStdout>,
    pub port: u16,
}

impl PyServer {
    pub fn start(dir: &Path, leaf: &str, args: &[&str]) -> PyServer {
        let mut child = Command::new("python3")
            .arg(oracle_dir().join("tls_server.py"))
            .arg(dir)
            .arg(leaf)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("python3");
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let port = line.trim().parse().unwrap_or_else(|_| panic!("tls_server.py did not start: {line:?}"));
        PyServer { child, out, port }
    }
    /// The server's account of its next connection.
    pub fn verdict(&mut self) -> String {
        let mut line = String::new();
        let _ = self.out.read_line(&mut line);
        line.trim().to_string()
    }
}

impl Drop for PyServer {
    fn drop(&mut self) {
        let _ = self.child.kill(); // by PID (the Child handle)
        let _ = self.child.wait();
    }
}

/// std TcpStream as a tls_core Transport.
pub struct Tcp(pub TcpStream);
impl Transport for Tcp {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
        self.0.read(buf).map_err(|_| TlsError::Transport)
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), TlsError> {
        self.0.write_all(data).map_err(|_| TlsError::Transport)
    }
}

pub fn dial(port: u16) -> Tcp {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    Tcp(s)
}

/// POST /size/<resp> with a `body`-byte body; returns the whole response (until close_notify).
pub fn exchange<T: Transport>(c: &mut tls_core::Client<'_, T>, body: usize, resp: usize) -> Vec<u8> {
    let req = format!("POST /size/{resp} HTTP/1.1\r\nHost: tlscore.test\r\nContent-Length: {body}\r\n\r\n");
    c.send(&[req.as_bytes(), &vec![b'B'; body]].concat()).unwrap();
    let mut out = Vec::new();
    while let Some(chunk) = c.recv().unwrap() {
        out.extend_from_slice(&chunk);
    }
    c.close().unwrap();
    out
}

/// Checks the oracle's patterned payload after the info line.
pub fn payload_ok(resp: &[u8], n: usize) -> bool {
    let Some(at) = resp.windows(4).position(|w| w == b"\r\n\r\n") else { return false };
    let p = &resp[at + 4..];
    let Some(nl) = p.iter().position(|&b| b == b'\n') else { return false };
    let want: Vec<u8> = (0..n).map(|i| ((i * 7) & 0xff) as u8).collect();
    p[nl + 1..] == want[..]
}

// ---------------------------------------------------------------- the man in the middle

/// Splits a byte stream into whole TLS records; returns (records, leftover).
pub fn split_records(buf: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let mut v = Vec::new();
    let mut at = 0;
    while buf.len() >= at + 5 {
        let len = u16::from_be_bytes([buf[at + 3], buf[at + 4]]) as usize;
        if buf.len() < at + 5 + len {
            break;
        }
        v.push(buf[at..at + 5 + len].to_vec());
        at += 5 + len;
    }
    (v, at)
}

/// Splits a plaintext handshake record's payload into messages (header included).
pub fn split_handshakes(payload: &[u8]) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    let mut at = 0;
    while payload.len() >= at + 4 {
        let len = ((payload[at + 1] as usize) << 16) | ((payload[at + 2] as usize) << 8) | payload[at + 3] as usize;
        v.push(payload[at..(at + 4 + len).min(payload.len())].to_vec());
        at += 4 + len;
    }
    v
}

pub fn join_handshake_record(msgs: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = msgs.concat();
    let mut r = vec![22, 3, 3, (body.len() >> 8) as u8, body.len() as u8];
    r.extend_from_slice(&body);
    r
}

/// The parts of a hello (ClientHello or ServerHello body) the rewriters touch.
pub struct Hello {
    pub head: Vec<u8>,        // everything before the extensions block
    pub exts: Vec<(u16, Vec<u8>)>,
}

pub fn parse_hello(msg: &[u8]) -> Hello {
    let body = &msg[4..];
    let mut at = 2 + 32;
    let sid = body[at] as usize;
    at += 1 + sid;
    if msg[0] == 1 {
        let cs = u16::from_be_bytes([body[at], body[at + 1]]) as usize;
        at += 2 + cs;
        let cm = body[at] as usize;
        at += 1 + cm;
    } else {
        at += 3;
    }
    let head = body[..at].to_vec();
    let mut exts = Vec::new();
    if body.len() > at {
        let total = u16::from_be_bytes([body[at], body[at + 1]]) as usize;
        let mut e = at + 2;
        while e < at + 2 + total {
            let ty = u16::from_be_bytes([body[e], body[e + 1]]);
            let l = u16::from_be_bytes([body[e + 2], body[e + 3]]) as usize;
            exts.push((ty, body[e + 4..e + 4 + l].to_vec()));
            e += 4 + l;
        }
    }
    Hello { head, exts }
}

pub fn encode_hello(ty: u8, h: &Hello) -> Vec<u8> {
    let mut body = h.head.clone();
    let mut e = Vec::new();
    for (t, d) in &h.exts {
        e.extend_from_slice(&t.to_be_bytes());
        e.extend_from_slice(&(d.len() as u16).to_be_bytes());
        e.extend_from_slice(d);
    }
    body.extend_from_slice(&(e.len() as u16).to_be_bytes());
    body.extend_from_slice(&e);
    let mut m = vec![ty, (body.len() >> 16) as u8, (body.len() >> 8) as u8, body.len() as u8];
    m.extend_from_slice(&body);
    m
}

/// A rewriting proxy: one connection from the client to `upstream`. `c2s` sees the client's FIRST record (the
/// ClientHello) and `s2c` the server's handshake MESSAGES while they are plaintext (until the server's CCS or
/// first protected record); each returns the replacement. Everything after passes through untouched.
pub struct Mitm {
    pub port: u16,
    handle: Option<std::thread::JoinHandle<()>>,
}

pub type Rewrite = Box<dyn FnMut(Vec<u8>) -> Vec<u8> + Send>;

impl Mitm {
    pub fn start(upstream: u16, mut c2s: Rewrite, mut s2c: Rewrite) -> Mitm {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut client, _) = l.accept().unwrap();
            let mut server = TcpStream::connect(("127.0.0.1", upstream)).unwrap();
            client.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            server.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            // client → server: rewrite the first record, then copy.
            let mut c2 = client.try_clone().unwrap();
            let mut s2 = server.try_clone().unwrap();
            let up = std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 16384];
                let mut first = true;
                loop {
                    let n = match c2.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if !first {
                        if s2.write_all(&chunk[..n]).is_err() {
                            break;
                        }
                        continue;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let (recs, used) = split_records(&buf);
                    if recs.is_empty() {
                        continue;
                    }
                    first = false;
                    let mut out = Vec::new();
                    for (i, r) in recs.iter().enumerate() {
                        if i == 0 { out.extend_from_slice(&c2s(r.clone())) } else { out.extend_from_slice(r) }
                    }
                    out.extend_from_slice(&buf[used..]);
                    if s2.write_all(&out).is_err() {
                        break;
                    }
                }
                let _ = s2.shutdown(std::net::Shutdown::Write);
            });
            // server → client: rewrite plaintext handshake messages.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 16384];
            let mut plain = true;
            loop {
                let n = match server.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                if !plain {
                    if client.write_all(&chunk[..n]).is_err() {
                        break;
                    }
                    continue;
                }
                buf.extend_from_slice(&chunk[..n]);
                let (recs, used) = split_records(&buf);
                let mut out = Vec::new();
                for r in recs {
                    if plain && r[0] == 22 {
                        let msgs: Vec<Vec<u8>> = split_handshakes(&r[5..]).into_iter().map(|m| s2c(m)).collect();
                        out.extend_from_slice(&join_handshake_record(&msgs));
                    } else {
                        if r[0] == 20 || r[0] == 23 {
                            plain = false;
                        }
                        out.extend_from_slice(&r);
                    }
                }
                let rest = buf[used..].to_vec();
                buf = rest;
                if !plain {
                    out.extend_from_slice(&buf);
                    buf.clear();
                }
                if client.write_all(&out).is_err() {
                    break;
                }
            }
            let _ = client.shutdown(std::net::Shutdown::Write);
            let _ = up.join();
        });
        Mitm { port, handle: Some(handle) }
    }
}

impl Drop for Mitm {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The identity rewrite.
pub fn keep() -> Rewrite {
    Box::new(|m| m)
}

/// `openssl s_server` (OpenSSL's own reference server) on 127.0.0.1 with stdin piped, for what Python's `ssl`
/// cannot do: renegotiation (`r` on stdin sends a HelloRequest) and forcing the signature algorithm.
pub struct SServer {
    pub child: Child,
    pub port: u16,
    pub log: std::sync::Arc<std::sync::Mutex<String>>,
}

impl SServer {
    pub fn start(dir: &Path, leaf: &str, extra: &[&str]) -> SServer {
        let cert = dir.join(format!("{leaf}.pem")).display().to_string();
        let key = dir.join(format!("{leaf}.key")).display().to_string();
        let chain = dir.join("inter.pem").display().to_string();
        let mut args: Vec<String> = ["-cert", &cert, "-key", &key, "-cert_chain", &chain].iter().map(|s| s.to_string()).collect();
        args.extend(extra.iter().map(|s| s.to_string()));
        Self::start_args(&args)
    }

    /// `openssl s_server -accept 127.0.0.1:<port> <args…>`.
    pub fn start_args(args: &[String]) -> SServer {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let mut child = Command::new("openssl")
            .arg("s_server")
            .arg("-accept")
            .arg(format!("127.0.0.1:{port}"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("openssl s_server");
        let log = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        for pipe in [Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>, Box::new(child.stderr.take().unwrap())] {
            let log = log.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(pipe);
                let mut line = String::new();
                while r.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                    log.lock().unwrap().push_str(&line);
                    line.clear();
                }
            });
        }
        // Wait for "ACCEPT".
        for _ in 0..100 {
            if log.lock().unwrap().contains("ACCEPT") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        SServer { child, port, log }
    }
    pub fn stdin(&mut self, s: &str) {
        let i = self.child.stdin.as_mut().unwrap();
        i.write_all(s.as_bytes()).unwrap();
        i.flush().unwrap();
    }
    pub fn log(&self) -> String {
        self.log.lock().unwrap().clone()
    }
    pub fn wait_for(&self, needle: &str, ms: u64) -> bool {
        for _ in 0..ms / 20 {
            if self.log().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

impl Drop for SServer {
    fn drop(&mut self) {
        let _ = self.child.kill(); // by PID (the Child handle)
        let _ = self.child.wait();
    }
}
