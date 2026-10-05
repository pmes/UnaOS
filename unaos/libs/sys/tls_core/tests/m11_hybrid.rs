//! CTCORE M3 — X25519MLKEM768 (draft-ietf-tls-ecdhe-mlkem; ML-KEM-768 per FIPS 203 from crypto_core) offered
//! first, X25519 shared alongside it, against servers that are not ours: Node's TLS on OpenSSL 3.5 (the host's
//! OpenSSL 3.0.13 has no ML-KEM), restricted by group, and — when tests/oracle/build_openssl35.sh has built one —
//! OpenSSL 3.5's own `s_server -brief`, whose "Peer Temp Key" line is the peer's report of the group used. A relay records
//! both hellos on the wire: our ClientHello's key_share groups and lengths, the ServerHello's selected group.

mod support;
use support::*;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use tls_core::error::TlsError;
use tls_core::msgs::NamedGroup;
use tls_core::x509::WebPkiVerifier;
use tls_core::{Client, ClientConfig};

struct Node {
    child: Child,
    out: BufReader<std::process::ChildStdout>,
    port: u16,
}
impl Node {
    fn start(dir: &std::path::Path, groups: &str) -> Option<Node> {
        let mut child = Command::new("node")
            .arg(oracle_dir().join("node_tls_server.js"))
            .arg(dir)
            .arg("p256")
            .arg(groups)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).ok()?;
        let port = line.trim().parse().ok()?;
        Some(Node { child, out, port })
    }
    fn line(&mut self) -> String {
        let mut l = String::new();
        let _ = self.out.read_line(&mut l);
        l.trim().to_string()
    }
}
impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill(); // by PID (the child handle)
        let _ = self.child.wait();
    }
}

/// (ClientHello key_share groups + lengths, ServerHello selected group + share length) seen on the wire.
#[derive(Default, Debug, Clone)]
struct Wire {
    client_shares: Vec<(u16, usize)>,
    server_group: Option<(u16, usize)>,
    hrr: bool,
}

fn exts(body: &[u8], mut off: usize) -> Vec<(u16, Vec<u8>)> {
    let mut v = Vec::new();
    if off + 2 > body.len() {
        return v;
    }
    let end = off + 2 + u16::from_be_bytes([body[off], body[off + 1]]) as usize;
    off += 2;
    while off + 4 <= end {
        let t = u16::from_be_bytes([body[off], body[off + 1]]);
        let l = u16::from_be_bytes([body[off + 2], body[off + 3]]) as usize;
        v.push((t, body[off + 4..off + 4 + l].to_vec()));
        off += 4 + l;
    }
    v
}

fn watch(upstream: u16, wire: Arc<Mutex<Wire>>) -> Mitm {
    let w1 = wire.clone();
    let c2s: Rewrite = Box::new(move |rec: Vec<u8>| {
        // record header 5, handshake header 4, version 2, random 32, session id, suites, compression, extensions
        let b = &rec[9..];
        let mut o = 34;
        o += 1 + b[o] as usize;
        o += 2 + u16::from_be_bytes([b[o], b[o + 1]]) as usize;
        o += 1 + b[o] as usize;
        for (t, d) in exts(b, o) {
            if t == 0x0033 {
                let mut i = 2;
                while i + 4 <= d.len() {
                    let g = u16::from_be_bytes([d[i], d[i + 1]]);
                    let l = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
                    w1.lock().unwrap().client_shares.push((g, l));
                    i += 4 + l;
                }
            }
        }
        rec
    });
    let s2c: Rewrite = Box::new(move |m: Vec<u8>| {
        if m[0] == 2 {
            let b = &m[4..];
            let hrr = b[2..34] == [0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91, 0xc2, 0xa2, 0x11, 0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c];
            let mut o = 34;
            o += 1 + b[o] as usize;
            o += 3;
            for (t, d) in exts(b, o) {
                if t == 0x0033 {
                    let g = u16::from_be_bytes([d[0], d[1]]);
                    let mut w = wire.lock().unwrap();
                    if hrr {
                        w.hrr = true;
                    } else {
                        w.server_group = Some((g, d.len().saturating_sub(4)));
                    }
                }
            }
        }
        m
    });
    Mitm::start(upstream, c2s, s2c)
}

fn connect(dir: &std::path::Path, port: u16, groups: Option<Vec<NamedGroup>>, key_shares: Option<usize>) -> (Result<(NamedGroup, bool), TlsError>, Wire) {
    let roots = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    let v = WebPkiVerifier { store: &roots, clock: &clock };
    let mut cfg = ClientConfig::new(Some("tlscore.test"), &v);
    if let Some(g) = groups {
        cfg.groups = g;
    }
    if let Some(k) = key_shares {
        cfg.key_shares = k;
    }
    let wire = Arc::new(Mutex::new(Wire::default()));
    let m = watch(port, wire.clone());
    let r = Client::connect(&p, &cfg, dial(m.port)).and_then(|mut c| {
        c.send(b"GET / CTCORE\n")?;
        let mut got = Vec::new();
        while let Some(d) = c.recv()? {
            got.extend_from_slice(&d);
        }
        assert!(String::from_utf8_lossy(&got).starts_with("ok "), "{got:?}");
        Ok((c.negotiated().group, c.negotiated().hello_retry))
    });
    drop(m);
    let w = wire.lock().unwrap().clone();
    (r, w)
}

#[test]
fn hybrid_against_node_openssl35() {
    let Some(dir) = pki("m11") else { return };
    let have_node = Command::new("node").arg("-p").arg("process.versions.openssl").output().ok().filter(|o| o.status.success());
    let Some(v) = have_node else {
        println!("HYBRID SKIPPED: node not found");
        return;
    };
    let ossl = String::from_utf8_lossy(&v.stdout).trim().to_string();
    println!("Node's OpenSSL: {ossl}");
    let h = NamedGroup::X25519MLKEM768;
    let x = NamedGroup::X25519;
    let p = NamedGroup::Secp256r1;
    // (server groups, client groups, client key_shares, expected group, expected HRR)
    let cases: Vec<(&str, Option<Vec<NamedGroup>>, Option<usize>, Result<(NamedGroup, bool), ()>)> = vec![
        ("X25519MLKEM768", None, None, Ok((h, false))),
        ("X25519MLKEM768:X25519", None, None, Ok((h, false))),
        ("X25519", None, None, Ok((x, false))),
        ("P-256", None, None, Ok((p, true))),
        ("X25519", None, Some(1), Ok((x, true))),
        ("X25519MLKEM768", Some(vec![x, p]), None, Err(())),
        ("X25519", Some(vec![h]), None, Err(())),
    ];
    let mut n = 0;
    for (sg, cg, ks, want) in cases {
        let Some(mut node) = Node::start(&dir, sg) else {
            println!("HYBRID SKIPPED: node server did not start");
            return;
        };
        let (r, wire) = connect(&dir, node.port, cg.clone(), ks);
        let server = node.line();
        println!("HYBRID server={sg:<22} client={:<34} shares={ks:?} → {:?} | wire: CH shares {:x?} SH {:x?} hrr={} | node: {server}", format!("{cg:?}"), r, wire.client_shares, wire.server_group, wire.hrr);
        match (want, &r) {
            (Ok(w), Ok(g)) => {
                assert_eq!(w, *g, "{sg}");
                if w.0 == h {
                    assert_eq!(wire.server_group, Some((0x11ec, 1120)), "server share = ML-KEM ciphertext ‖ X25519");
                    assert!(server.contains("TLSv1.3"));
                }
                if cg.is_none() && ks.is_none() {
                    assert_eq!(wire.client_shares, vec![(0x11ec, 1216), (0x001d, 32)], "hybrid first, X25519 alongside");
                }
            }
            (Err(()), Err(e)) => println!("        refused as expected: {e:?}"),
            _ => panic!("{sg}: want {want:?} got {r:?}"),
        }
        n += 1;
    }
    println!("HYBRID vs Node/OpenSSL {ossl}: {n}/{n} as expected");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hybrid_against_openssl35_s_server() {
    let bin = std::env::var("OPENSSL35").ok().or_else(|| {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../target/openssl35/apps/openssl");
        p.is_file().then(|| p.display().to_string())
    });
    let Some(bin) = bin else {
        println!("OPENSSL 3.5 S_SERVER SKIPPED: set OPENSSL35=/path/to/openssl-3.5 (tests/oracle/build_openssl35.sh builds one)");
        return;
    };
    let Some(dir) = pki("m11o") else { return };
    let ver = Command::new(&bin).arg("version").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let mut n = 0;
    for (groups, want) in [("X25519MLKEM768", "Peer Temp Key: X25519MLKEM768"), ("X25519MLKEM768:X25519", "Peer Temp Key: X25519MLKEM768"), ("X25519", "Peer Temp Key: X25519, 253 bits"), ("secp256r1", "Peer Temp Key: ECDH, prime256v1")] {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        // stdin stays open (piped): on EOF s_server would drop the connection before printing its summary.
        let mut child = Command::new(&bin)
            .args(["s_server", "-accept", &format!("127.0.0.1:{port}"), "-cert", "p256.pem", "-key", "p256.key", "-cert_chain", "inter.pem", "-tls1_3", "-groups", groups, "-brief", "-naccept", "1"])
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        let roots = store_of(&dir.join("root.pem"));
        let p = provider();
        let clock = SystemClock;
        let v = WebPkiVerifier { store: &roots, clock: &clock };
        let cfg = ClientConfig::new(Some("tlscore.test"), &v);
        let r = Client::connect(&p, &cfg, dial(port)).map(|mut c| {
            let g = c.negotiated().group;
            let _ = c.send(b"Q\n");
            let _ = c.close();
            g
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(child.stdin.take());
        let out = child.wait_with_output().unwrap();
        let log = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        // OpenSSL 3.5 s_server -brief: "Peer Temp Key: <group>" (the client's share it used) is the server's report.
        let reported = log.lines().find(|l| l.starts_with("Peer Temp Key") || l.starts_with("Negotiated TLS1.3 group")).unwrap_or("").to_string();
        println!("OPENSSL35 -groups {groups:<22} tls_core → {r:?} | s_server: {reported}");
        assert!(r.is_ok(), "{log}");
        assert!(reported.starts_with(want), "{log}");
        n += 1;
    }
    println!("HYBRID vs {ver} s_server: {n}/{n} — the peer reports the negotiated group");
    let _ = std::fs::remove_dir_all(&dir);
}
