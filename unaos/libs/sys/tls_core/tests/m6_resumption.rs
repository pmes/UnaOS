//! TLSCORE2 M2 — TLS 1.3 session resumption (RFC 8446 §2.2, §4.2.11, §4.6.1) against OpenSSL (Python `ssl`, one
//! SSLContext serving successive connections), on the product provider. The ORACLE is the server's own
//! `session_reused` flag: OpenSSL sets it only when it decrypted our ticket AND verified our PSK binder, and the
//! handshake then completes only if both sides ran the PSK + (EC)DHE key schedule identically.

mod support;
use support::*;

use tls_core::error::TlsError;
use tls_core::msgs::{CipherSuite, NamedGroup};
use tls_core::resumption::{MemoryTicketStore, Resumption, Ticket, TicketStore};
use tls_core::x509::{TrustStore, WebPkiVerifier};
use tls_core::{Client, ClientConfig};

fn connect<'a>(
    p: &'a dyn tls_core::CryptoProvider,
    cfg: &'a ClientConfig<'a>,
    port: u16,
) -> Result<Client<'a, Tcp>, TlsError> {
    Client::connect(p, cfg, dial(port))
}

fn cfg<'a>(v: &'a WebPkiVerifier<'a>, store: &'a dyn TicketStore, clock: &'a SystemClock, suites: &[CipherSuite]) -> ClientConfig<'a> {
    let mut c = ClientConfig::new(Some("tlscore.test"), v);
    c.cipher_suites = suites.to_vec();
    c.alpn = vec![b"http/1.1".to_vec()];
    c.resumption = Some(Resumption { store, clock });
    c
}

fn run(p: &dyn tls_core::CryptoProvider, c: &ClientConfig<'_>, srv: &mut PyServer) -> (tls_core::Negotiated, String) {
    let mut cl = connect(p, c, srv.port).unwrap_or_else(|e| panic!("handshake failed: {e:?}"));
    let n = cl.negotiated().clone();
    let resp = exchange(&mut cl, 64, 1000);
    assert!(payload_ok(&resp, 1000));
    drop(cl);
    (n, srv.verdict())
}

#[test]
fn resumption_against_openssl() {
    let Some(dir) = pki("m6") else { return };
    let store_pem = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    let v = WebPkiVerifier { store: &store_pem, clock: &clock };
    let tickets = MemoryTicketStore::new();
    let all = CipherSuite::TLS13.to_vec();
    let mut srv = PyServer::start(&dir, "p256", &["--tls", "1.3", "--conns", "7"]);

    // 1. Full handshake; OpenSSL sends two NewSessionTickets, both kept.
    let c = cfg(&v, &tickets, &clock, &all);
    let (n, verdict) = run(&p, &c, &mut srv);
    println!("RESUME 1 full:      resumed={} chain={} | server: {verdict} | store={}", n.resumed, n.peer_chain_len, tickets.len());
    assert!(!n.resumed && n.peer_chain_len == 2);
    assert!(verdict.contains("reused=False"), "{verdict}");
    assert_eq!(tickets.len(), 2, "both tickets kept");

    // 2. Resumed: PSK + (EC)DHE, no Certificate; the server says reused.
    let (n, verdict) = run(&p, &c, &mut srv);
    println!("RESUME 2 ticket:    resumed={} chain={} suite={:?} group={:?} | server: {verdict}", n.resumed, n.peer_chain_len, n.cipher_suite, n.group);
    assert!(n.resumed && n.peer_chain_len == 0, "resumed without a certificate");
    assert!(verdict.contains("reused=True"), "OpenSSL resumed the session: {verdict}");

    // 3. OpenSSL picked TLS_AES_256_GCM_SHA384, so the tickets are SHA-384 PSKs. A configuration offering only
    //    SHA-256 suites cannot use them: no PSK is offered, a full handshake runs, and the tickets stay stored.
    assert_eq!(n.cipher_suite, CipherSuite::Aes256GcmSha384);
    let before = tickets.len();
    let c256 = cfg(&v, &tickets, &clock, &[CipherSuite::Aes128GcmSha256, CipherSuite::ChaCha20Poly1305Sha256]);
    let (n, verdict) = run(&p, &c256, &mut srv);
    println!("RESUME 3 hash≠:     resumed={} suite={:?} | server: {verdict} | store {before}->{}", n.resumed, n.cipher_suite, tickets.len());
    assert!(!n.resumed && verdict.contains("reused=False"), "{verdict}");
    assert!(tickets.len() >= before, "the SHA-384 tickets were put back");

    // 4. A ticket whose identity was altered: the server cannot decrypt it, ignores the PSK, and a full verified
    //    handshake follows (RFC 8446 §4.2.11: the server may decline any PSK).
    let mut t = take_384(&tickets);
    t.ticket[3] ^= 0x40;
    tickets.put(t);
    let (n, verdict) = run(&p, &c, &mut srv);
    println!("RESUME 4 bad ident: resumed={} chain={} | server: {verdict}", n.resumed, n.peer_chain_len);
    assert!(!n.resumed && n.peer_chain_len == 2 && verdict.contains("reused=False"), "{verdict}");

    // 5. A ticket whose PSK was altered: the binder no longer verifies — OpenSSL MUST abort (§4.2.11), and does.
    let mut t = take_384(&tickets);
    t.psk[0] ^= 1;
    tickets.put(t);
    let e = connect(&p, &c, srv.port).err().expect("a wrong binder must be refused by the server");
    let verdict = srv.verdict();
    println!("RESUME 5 bad PSK:   client {e:?} | server: {verdict}");
    assert!(verdict.starts_with("HANDSHAKE-FAIL") && verdict.contains("BINDER"), "{verdict}");

    // 6. A ticket that went through the persistent form (what a file store writes) still resumes.
    let t = take_384(&tickets);
    let bytes = t.to_bytes();
    let back = Ticket::from_bytes(&bytes).expect("round trip");
    assert!(back == t, "to_bytes/from_bytes round trip");
    tickets.put(back);
    let (n, verdict) = run(&p, &c, &mut srv);
    println!("RESUME 6 persisted: resumed={} | server: {verdict} ({} B ticket record)", n.resumed, bytes.len());
    assert!(n.resumed && verdict.contains("reused=True"), "{verdict}");

    // 7. An expired ticket is never offered.
    let mut t = take_384(&tickets);
    t.received_ms -= t.lifetime as u64 * 1000 + 1;
    while tickets.take("tlscore.test").is_some() {}
    tickets.put(t);
    let (n, verdict) = run(&p, &c, &mut srv);
    println!("RESUME 7 expired:   resumed={} | server: {verdict}", n.resumed);
    assert!(!n.resumed && verdict.contains("reused=False"), "{verdict}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn take_384(s: &MemoryTicketStore) -> Ticket {
    let mut back = Vec::new();
    let found = loop {
        let t = s.take("tlscore.test").expect("a ticket");
        if t.suite.hash() == tls_core::crypto::HashAlg::Sha384 {
            break t;
        }
        back.push(t);
    };
    for t in back {
        s.put(t);
    }
    found
}

/// HelloRetryRequest with a PSK: the server takes only P-256, we lead with X25519; the second ClientHello carries
/// the PSK again with a binder over message_hash(CH1) + HRR + CH2 (§4.1.4, §4.2.11.2).
#[test]
fn resumption_through_hello_retry_request() {
    let Some(dir) = pki("m6h") else { return };
    let store_pem = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    let v = WebPkiVerifier { store: &store_pem, clock: &clock };
    let tickets = MemoryTicketStore::new();
    let mut srv = PyServer::start(&dir, "ed25519", &["--tls", "1.3", "--curve", "prime256v1", "--conns", "2"]);
    let c = cfg(&v, &tickets, &clock, &CipherSuite::TLS13);
    let (n, verdict) = run(&p, &c, &mut srv);
    println!("HRR+PSK 1: hrr={} resumed={} | server: {verdict}", n.hello_retry, n.resumed);
    assert!(n.hello_retry && !n.resumed);
    let (n, verdict) = run(&p, &c, &mut srv);
    println!("HRR+PSK 2: hrr={} resumed={} group={:?} | server: {verdict}", n.hello_retry, n.resumed, n.group);
    assert!(n.hello_retry && n.resumed && n.group == NamedGroup::Secp256r1);
    assert!(verdict.contains("reused=True"), "{verdict}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Without a store nothing changes: no psk_key_exchange_modes, and a server's tickets are only parsed.
#[test]
fn no_store_no_resumption() {
    let Some(dir) = pki("m6n") else { return };
    let store_pem = store_of(&dir.join("root.pem"));
    let p = provider();
    let clock = SystemClock;
    let v = WebPkiVerifier { store: &store_pem, clock: &clock };
    let mut srv = PyServer::start(&dir, "p256", &["--tls", "1.3", "--conns", "2"]);
    let c = ClientConfig::new(Some("tlscore.test"), &v);
    for i in 0..2 {
        let mut cl = connect(&p, &c, srv.port).unwrap();
        assert!(!cl.negotiated().resumed);
        exchange(&mut cl, 0, 0);
        drop(cl);
        let verdict = srv.verdict();
        println!("NO-STORE {i}: | server: {verdict}");
        assert!(verdict.contains("reused=False"));
    }
    let _ = TrustStore::new();
    let _ = std::fs::remove_dir_all(&dir);
}
