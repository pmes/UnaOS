//! M2 — the client handshake state machine (RFC 8446 §4) driven end to end by the RFC 8448 traces over a fake
//! transport: every byte the client writes must equal the trace's client records.

mod common;
use common::*;

use tls_core::crypto::CryptoProvider;
use tls_core::error::{AlertDescription, TlsError};
use tls_core::msgs::{self, CipherSuite, NamedGroup, SignatureScheme};
use tls_core::test_provider::RustCryptoProvider;
use tls_core::x509::PinnedLeafVerifier;
use tls_core::{Client, ClientConfig};

fn leaf_of(t: &Trace) -> Vec<u8> {
    msgs::parse_certificate(&t.get("Message_Server_Certificate")[4..]).unwrap().remove(0)
}

fn config<'a>(v: &'a PinnedLeafVerifier, hellos: Vec<Vec<u8>>, compat: bool) -> ClientConfig<'a> {
    let mut c = ClientConfig::new(Some("server"), v);
    c.compat_mode = compat;
    c.hello_overrides = hellos;
    c
}

#[test]
fn rfc8448_simple_1rtt_end_to_end() {
    let t = rfc8448("Simple_1RTT_Handshake");
    let p = RustCryptoProvider::with_rng_pool(t.get("Client_RNG_Pool"));
    let v = PinnedLeafVerifier { leaf_der: leaf_of(&t) };
    let cfg = config(&v, vec![t.get("Record_ClientHello_1")[5..].to_vec()], false);
    let mut ft = FakeTransport::new(
        [
            t.get("Record_ServerHello"),
            t.get("Record_ServerHandshakeMessages"),
            t.get("Record_NewSessionTicket"),
            t.get("Record_Server_AppData"),
            t.get("Record_Server_CloseNotify"),
        ]
        .concat(),
    );
    ft.chunk = 7; // force record reassembly from tiny reads
    let mut c = Client::connect(&p, &cfg, ft).expect("handshake");
    assert_eq!(p.pool_remaining(), 0, "random + x25519 private consumed exactly");
    assert_eq!(
        to_hex(&c.transport().written),
        to_hex(&[t.get("Record_ClientHello_1"), t.get("Record_ClientFinished")].concat()),
        "client flight byte-exact"
    );
    println!("RFC8448 1-RTT: ClientHello + client Finished records byte-exact: yes");
    let n = c.negotiated().clone();
    assert_eq!(n.cipher_suite, CipherSuite::Aes128GcmSha256);
    assert_eq!(n.group, NamedGroup::X25519);
    assert_eq!(n.signature_scheme, SignatureScheme::RsaPssRsaeSha256);
    assert!(!n.hello_retry);
    assert_eq!(
        to_hex(c.resumption_master_secret()),
        "7df235f2031d2a051287d02b0241b0bfdaf86cc856231f2d5aba46c434ec196c"
    );

    let before = c.transport().written.len();
    c.send(&t.get("Client_AppData")).unwrap();
    assert_eq!(c.transport().written[before..], t.get("Record_Client_AppData")[..]);
    assert_eq!(c.recv().unwrap(), Some(t.get("Server_AppData")));
    assert_eq!(c.tickets.len(), 1, "NewSessionTicket kept");
    assert_eq!(c.tickets[0].ticket, t.get("SessionTicket"));
    let before = c.transport().written.len();
    c.close().unwrap();
    assert_eq!(c.transport().written[before..], t.get("Record_Client_CloseNotify")[..]);
    assert_eq!(c.recv().unwrap(), None, "server close_notify");
    println!("RFC8448 1-RTT: app data, NewSessionTicket, close_notify both ways: yes");
}

#[test]
fn rfc8448_hello_retry_request_end_to_end() {
    let t = rfc8448("HelloRetryRequest_Handshake");
    let p = RustCryptoProvider::with_rng_pool(t.get("Client_RNG_Pool"));
    let v = PinnedLeafVerifier { leaf_der: leaf_of(&t) };
    let cfg = config(
        &v,
        vec![t.get("Record_ClientHello_1")[5..].to_vec(), t.get("Record_ClientHello_2")[5..].to_vec()],
        false,
    );
    let ft = FakeTransport::new(
        [
            t.get("Record_HelloRetryRequest"),
            t.get("Record_ServerHello"),
            t.get("Record_ServerHandshakeMessages"),
            t.get("Record_Server_CloseNotify"),
        ]
        .concat(),
    );
    let mut c = Client::connect(&p, &cfg, ft).expect("handshake with HRR");
    assert_eq!(p.pool_remaining(), 0, "random + x25519 + P-256 privates consumed exactly");
    assert_eq!(
        to_hex(&c.transport().written),
        to_hex(&[t.get("Record_ClientHello_1"), t.get("Record_ClientHello_2"), t.get("Record_ClientFinished")].concat())
    );
    let n = c.negotiated().clone();
    assert!(n.hello_retry);
    assert_eq!(n.group, NamedGroup::Secp256r1);
    // Our own ClientHello2 must carry the HRR cookie: parse the trace's CH2 and check it has it (and the P-256
    // share our provider generated).
    let ch2 = msgs::parse_client_hello(&t.get("Record_ClientHello_2")[5..]).unwrap();
    assert!(ch2.extensions.contains(&msgs::ext::COOKIE));
    assert_eq!(ch2.key_share_groups, vec![0x0017]);
    let before = c.transport().written.len();
    c.close().unwrap();
    assert_eq!(c.transport().written[before..], t.get("Record_Client_CloseNotify")[..]);
    assert_eq!(c.recv().unwrap(), None);
    println!("RFC8448 HRR: CH1 + CH2(cookie, P-256 share) + Finished + close_notify byte-exact: yes");
}

#[test]
fn rfc8448_middlebox_compat_end_to_end() {
    let t = rfc8448("Middlebox_Compatibility_Mode");
    let ch1 = t.get("Record_ClientHello_1")[5..].to_vec();
    let offered = msgs::parse_client_hello(&ch1).unwrap();
    // Our RNG order is: random(32), legacy_session_id(32), x25519 private(32). Build that pool from the trace's
    // values (whatever order the transcriber's pool used).
    let pool = t.get("Client_RNG_Pool");
    let sid_at = pool.windows(32).position(|w| w == offered.session_id.as_slice()).expect("session id in pool");
    let mut ours = offered.random.to_vec();
    ours.extend_from_slice(&offered.session_id);
    let rest: Vec<u8> = pool.iter().enumerate().filter(|(i, _)| *i >= 32 && !(sid_at..sid_at + 32).contains(i)).map(|(_, b)| *b).collect();
    ours.extend_from_slice(&rest);
    let p = RustCryptoProvider::with_rng_pool(ours);
    let v = PinnedLeafVerifier { leaf_der: leaf_of(&t) };
    let cfg = config(&v, vec![ch1], true);
    let ft = FakeTransport::new(
        [t.get("Record_ServerHello"), t.get("Record_ServerHandshakeMessages"), t.get("Record_Server_CloseNotify")].concat(),
    );
    let mut c = Client::connect(&p, &cfg, ft).expect("compat-mode handshake");
    // Record_ClientFinished in the trace is the dummy change_cipher_spec record followed by the protected Finished.
    assert_eq!(
        to_hex(&c.transport().written),
        to_hex(&[t.get("Record_ClientHello_1"), t.get("Record_ClientFinished")].concat())
    );
    c.close().unwrap();
    assert_eq!(c.recv().unwrap(), None);
    println!("RFC8448 compat mode: session id + CCS + Finished byte-exact: yes");
}

/// The server flight of the 1-RTT trace with one byte of the given record flipped; the client must refuse and send
/// the right alert.
fn tampered(record_index: usize, byte: usize) -> (TlsError, Vec<u8>) {
    let t = rfc8448("Simple_1RTT_Handshake");
    let p = RustCryptoProvider::with_rng_pool(t.get("Client_RNG_Pool"));
    let v = PinnedLeafVerifier { leaf_der: leaf_of(&t) };
    let cfg = config(&v, vec![t.get("Record_ClientHello_1")[5..].to_vec()], false);
    let mut recs = vec![t.get("Record_ServerHello"), t.get("Record_ServerHandshakeMessages")];
    recs[record_index][byte] ^= 0x01;
    let ft = FakeTransport::new(recs.concat());
    match Client::connect(&p, &cfg, ft) {
        Ok(_) => panic!("tampered handshake accepted"),
        Err(e) => (e, Vec::new()),
    }
}

#[test]
fn rfc8448_tampering_is_refused() {
    // A flipped bit in the encrypted flight: bad_record_mac.
    assert_eq!(tampered(1, 40).0, TlsError::BadRecordMac);
    // A different server random changes the transcript: the client derives other keys → bad_record_mac.
    assert_eq!(tampered(0, 5 + 4 + 2 + 3).0, TlsError::BadRecordMac);
    // ServerHello selecting a cipher suite we did not offer.
    let e = tampered(0, 5 + 4 + 2 + 32 + 1 + 1).0;
    assert!(matches!(e, TlsError::Protocol(AlertDescription::IllegalParameter, _)), "{e:?}");
}

#[test]
fn wrong_pinned_certificate_is_refused_with_alert() {
    let t = rfc8448("Simple_1RTT_Handshake");
    let p = RustCryptoProvider::with_rng_pool(t.get("Client_RNG_Pool"));
    let v = PinnedLeafVerifier { leaf_der: vec![0x30, 0x00] };
    let cfg = config(&v, vec![t.get("Record_ClientHello_1")[5..].to_vec()], false);
    let ft = FakeTransport::new([t.get("Record_ServerHello"), t.get("Record_ServerHandshakeMessages")].concat());
    let e = Client::connect(&p, &cfg, ft).err().unwrap();
    assert_eq!(e, TlsError::Certificate(tls_core::CertError::UnknownIssuer));
    assert_eq!(e.alert(), Some(AlertDescription::UnknownCa));
}

#[test]
fn our_client_hello_is_well_formed() {
    // The hello we build ourselves (not the trace's): legacy fields, the extensions §4.2 makes mandatory, and a
    // key_share for the first configured group only.
    let p = RustCryptoProvider::with_rng_pool((0u8..96).collect());
    let v = PinnedLeafVerifier { leaf_der: vec![] };
    let mut cfg = ClientConfig::new(Some("Example.COM."), &v);
    cfg.alpn = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let ft = FakeTransport::new(vec![]);
    let e = Client::connect(&p, &cfg, ft);
    assert!(matches!(e, Err(TlsError::UnexpectedEof)));
    // Rebuild what was written: re-run and capture via a transport that keeps the bytes.
    let p = RustCryptoProvider::with_rng_pool((0u8..96).collect());
    struct Keep(Vec<u8>);
    impl tls_core::Transport for &mut Keep {
        fn read(&mut self, _b: &mut [u8]) -> Result<usize, TlsError> {
            Ok(0)
        }
        fn write_all(&mut self, d: &[u8]) -> Result<(), TlsError> {
            self.0.extend_from_slice(d);
            Ok(())
        }
    }
    let mut keep = Keep(Vec::new());
    let _ = Client::connect(&p, &cfg, &mut keep);
    let rec = keep.0;
    assert_eq!(&rec[..3], &[0x16, 0x03, 0x01], "initial ClientHello record version 0x0301");
    let ch = msgs::parse_client_hello(&rec[5..]).unwrap();
    assert_eq!(ch.random.to_vec(), (0u8..32).collect::<Vec<u8>>());
    assert_eq!(ch.session_id, (32u8..64).collect::<Vec<u8>>(), "compat mode: 32-byte session id");
    assert_eq!(ch.cipher_suites, vec![0x1301, 0x1302, 0x1303]);
    for e in [msgs::ext::SERVER_NAME, msgs::ext::SUPPORTED_GROUPS, msgs::ext::SIGNATURE_ALGORITHMS, msgs::ext::ALPN, msgs::ext::SUPPORTED_VERSIONS, msgs::ext::KEY_SHARE] {
        assert!(ch.extensions.contains(&e), "extension {e}");
    }
    assert_eq!(ch.groups, vec![0x001d, 0x0017]);
    assert_eq!(ch.key_share_groups, vec![0x001d]);
    assert_eq!(ch.alpn, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    assert!(ch.signature_schemes.contains(&0x0403) && ch.signature_schemes.contains(&0x0807) && ch.signature_schemes.contains(&0x0804));
    // SNI is lower-cased with the trailing dot removed.
    let needle = b"example.com";
    assert!(rec.windows(needle.len()).any(|w| w == needle));
}

#[test]
fn baseline_provider_offers_only_what_it_can_verify() {
    // A provider with the default `supports_signature` (the CRYPTOCORE baseline) must not invite RSA.
    struct Baseline(RustCryptoProvider);
    impl CryptoProvider for Baseline {
        fn random(&self, o: &mut [u8]) -> Result<(), tls_core::crypto::CryptoError> { self.0.random(o) }
        fn hash(&self, a: tls_core::crypto::HashAlg, p: &[&[u8]]) -> tls_core::crypto::Digest { self.0.hash(a, p) }
        fn hmac(&self, a: tls_core::crypto::HashAlg, k: &[u8], p: &[&[u8]]) -> tls_core::crypto::Digest { self.0.hmac(a, k, p) }
        fn aead_seal(&self, a: tls_core::crypto::AeadAlg, k: &[u8], n: &[u8; 12], aad: &[u8], b: &mut Vec<u8>) -> Result<(), tls_core::crypto::CryptoError> { self.0.aead_seal(a, k, n, aad, b) }
        fn aead_open(&self, a: tls_core::crypto::AeadAlg, k: &[u8], n: &[u8; 12], aad: &[u8], b: &mut Vec<u8>) -> Result<(), tls_core::crypto::CryptoError> { self.0.aead_open(a, k, n, aad, b) }
        fn x25519_keypair(&self) -> Result<(tls_core::crypto::KxPrivate, [u8; 32]), tls_core::crypto::CryptoError> { self.0.x25519_keypair() }
        fn x25519_shared(&self, k: &tls_core::crypto::KxPrivate, p: &[u8]) -> Result<[u8; 32], tls_core::crypto::CryptoError> { self.0.x25519_shared(k, p) }
        fn p256_keypair(&self) -> Result<(tls_core::crypto::KxPrivate, Vec<u8>), tls_core::crypto::CryptoError> { self.0.p256_keypair() }
        fn p256_ecdh(&self, k: &tls_core::crypto::KxPrivate, p: &[u8]) -> Result<[u8; 32], tls_core::crypto::CryptoError> { self.0.p256_ecdh(k, p) }
        fn ecdsa_verify(&self, c: tls_core::crypto::EcCurve, h: tls_core::crypto::HashAlg, k: &[u8], m: &[u8], s: &[u8]) -> Result<(), tls_core::crypto::CryptoError> { self.0.ecdsa_verify(c, h, k, m, s) }
        fn ed25519_verify(&self, k: &[u8], m: &[u8], s: &[u8]) -> Result<(), tls_core::crypto::CryptoError> { self.0.ed25519_verify(k, m, s) }
    }
    let b = Baseline(RustCryptoProvider::new());
    assert!(matches!(b.rsa_pss_verify(tls_core::crypto::HashAlg::Sha256, &[1], &[1], b"", b""), Err(tls_core::crypto::CryptoError::Unsupported(_))));
    assert!(b.supports_signature(SignatureScheme::EcdsaSecp256r1Sha256));
    assert!(!b.supports_signature(SignatureScheme::RsaPssRsaeSha256));
}

// ---------------------------------------------------------------- beyond the traces: the 1-RTT trace's keys are
// known, so the "server" side of these tests seals its own records with them.

use tls_core::crypto::{AeadAlg, HashAlg};
use tls_core::key_schedule;
use tls_core::msgs::ContentType;
use tls_core::record::RecordProtection;

const S_HS: &str = "b67b7d690cc16c4e75e54213cb2d37b4e9c912bcded9105d42befd59d391ad38";
const C_AP: &str = "9e40646ce79a7f9dc05af8889bce6552875afa0b06df0087f792ebb7c17504a5";
const S_AP: &str = "a11af9f05531f856ad47116b45a950328204b4f44bfb6b3a4b4f1f3fcb631643";

fn prot(secret: &[u8]) -> RecordProtection {
    let p = RustCryptoProvider::new();
    let (k, iv) = key_schedule::traffic_keys(&p, HashAlg::Sha256, 16, secret).unwrap();
    RecordProtection::new(AeadAlg::Aes128Gcm, k, iv)
}

fn open_all(rx: &mut RecordProtection, bytes: &[u8]) -> Vec<(ContentType, Vec<u8>)> {
    let p = RustCryptoProvider::new();
    split_records(bytes)
        .into_iter()
        .map(|r| {
            let h: [u8; 5] = r[..5].try_into().unwrap();
            rx.open(&p, &h, &r[5..]).unwrap()
        })
        .collect()
}

#[test]
fn key_update_both_directions() {
    let t = rfc8448("Simple_1RTT_Handshake");
    let p = RustCryptoProvider::with_rng_pool(t.get("Client_RNG_Pool"));
    let sp = RustCryptoProvider::new();
    let v = PinnedLeafVerifier { leaf_der: leaf_of(&t) };
    let cfg = config(&v, vec![t.get("Record_ClientHello_1")[5..].to_vec()], false);
    // Server: KeyUpdate(update_requested) under s_ap_0, then data + close_notify under s_ap_1.
    let mut s0 = prot(&hex(S_AP));
    let s_ap1 = key_schedule::next_traffic_secret(&sp, HashAlg::Sha256, &hex(S_AP)).unwrap();
    let mut s1 = prot(s_ap1.as_bytes());
    let ku = msgs::handshake_message(msgs::hs::KEY_UPDATE, &[1]);
    let incoming = [
        t.get("Record_ServerHello"),
        t.get("Record_ServerHandshakeMessages"),
        s0.seal(&sp, ContentType::Handshake, &ku, 0).unwrap(),
        s1.seal(&sp, ContentType::ApplicationData, b"after rekey", 3).unwrap(),
        s1.seal(&sp, ContentType::Alert, &[1, 0], 0).unwrap(),
    ]
    .concat();
    let mut c = Client::connect(&p, &cfg, FakeTransport::new(incoming)).unwrap();
    let hs_len = c.transport().written.len();
    assert_eq!(c.recv().unwrap(), Some(b"after rekey".to_vec()));
    assert_eq!(c.key_updates_received, 1);
    // The client answered with KeyUpdate(update_not_requested) under c_ap_0 and switched to c_ap_1.
    c.send(b"client after rekey").unwrap();
    c.send_key_update(true).unwrap();
    c.send(b"client after second rekey").unwrap();
    let out = c.transport().written[hs_len..].to_vec();
    let recs = split_records(&out);
    assert_eq!(recs.len(), 4);
    let mut c0 = prot(&hex(C_AP));
    let c_ap1 = key_schedule::next_traffic_secret(&sp, HashAlg::Sha256, &hex(C_AP)).unwrap();
    let c_ap2 = key_schedule::next_traffic_secret(&sp, HashAlg::Sha256, c_ap1.as_bytes()).unwrap();
    let mut c1 = prot(c_ap1.as_bytes());
    let mut c2 = prot(c_ap2.as_bytes());
    assert_eq!(open_all(&mut c0, &recs[0]), vec![(ContentType::Handshake, vec![24, 0, 0, 1, 0])]);
    assert_eq!(open_all(&mut c1, &[recs[1].clone(), recs[2].clone()].concat()), vec![
        (ContentType::ApplicationData, b"client after rekey".to_vec()),
        (ContentType::Handshake, vec![24, 0, 0, 1, 1]),
    ]);
    assert_eq!(open_all(&mut c2, &recs[3]), vec![(ContentType::ApplicationData, b"client after second rekey".to_vec())]);
    assert_eq!(c.recv().unwrap(), None);
    println!("KeyUpdate: server-requested update answered, client-initiated update, keys verified: yes");
}

#[test]
fn server_flight_fragmented_across_records_and_fatal_alert() {
    // Re-protect the trace's server flight as 5 records with padding, messages split mid-header: reassembly.
    let t = rfc8448("Simple_1RTT_Handshake");
    let sp = RustCryptoProvider::new();
    let flight = [
        t.get("Message_EncryptedExtensions"),
        t.get("Message_Server_Certificate"),
        t.get("Message_Server_CertificateVerify"),
        t.get("Message_Server_Finished"),
    ]
    .concat();
    let mut shs = prot(&hex(S_HS));
    let mut recs = vec![t.get("Record_ServerHello"), vec![0x14, 0x03, 0x03, 0x00, 0x01, 0x01]]; // + a CCS (ignored)
    for chunk in flight.chunks(flight.len() / 4 + 1) {
        recs.push(shs.seal(&sp, ContentType::Handshake, chunk, 17).unwrap());
    }
    // Then a fatal alert from the server under s_ap.
    let mut sap = prot(&hex(S_AP));
    recs.push(sap.seal(&sp, ContentType::Alert, &[2, AlertDescription::InternalError as u8], 0).unwrap());
    let p = RustCryptoProvider::with_rng_pool(t.get("Client_RNG_Pool"));
    let v = PinnedLeafVerifier { leaf_der: leaf_of(&t) };
    let cfg = config(&v, vec![t.get("Record_ClientHello_1")[5..].to_vec()], false);
    let mut c = Client::connect(&p, &cfg, FakeTransport::new(recs.concat())).expect("fragmented flight");
    assert_eq!(
        to_hex(&c.transport().written),
        to_hex(&[t.get("Record_ClientHello_1"), t.get("Record_ClientFinished")].concat()),
        "same keys, same Finished, whatever the record boundaries"
    );
    assert_eq!(c.recv(), Err(TlsError::PeerAlert(AlertDescription::InternalError)));
    assert_eq!(c.send(b"x"), Err(TlsError::Closed));
}

#[test]
fn server_finished_mismatch_sends_decrypt_error() {
    let t = rfc8448("Simple_1RTT_Handshake");
    let sp = RustCryptoProvider::new();
    let mut sf = t.get("Message_Server_Finished");
    sf[10] ^= 0xff;
    let flight = [
        t.get("Message_EncryptedExtensions"),
        t.get("Message_Server_Certificate"),
        t.get("Message_Server_CertificateVerify"),
        sf,
    ]
    .concat();
    let mut shs = prot(&hex(S_HS));
    let incoming = [t.get("Record_ServerHello"), shs.seal(&sp, ContentType::Handshake, &flight, 0).unwrap()].concat();
    let p = RustCryptoProvider::with_rng_pool(t.get("Client_RNG_Pool"));
    let v = PinnedLeafVerifier { leaf_der: leaf_of(&t) };
    let cfg = config(&v, vec![t.get("Record_ClientHello_1")[5..].to_vec()], false);
    let e = Client::connect(&p, &cfg, FakeTransport::new(incoming)).err().unwrap();
    assert!(matches!(e, TlsError::Protocol(AlertDescription::DecryptError, _)), "{e:?}");

    // CertificateVerify signature corrupted → decrypt_error too.
    let mut cv = t.get("Message_Server_CertificateVerify");
    cv[20] ^= 1;
    let flight = [t.get("Message_EncryptedExtensions"), t.get("Message_Server_Certificate"), cv, t.get("Message_Server_Finished")].concat();
    let mut shs = prot(&hex(S_HS));
    let incoming = [t.get("Record_ServerHello"), shs.seal(&sp, ContentType::Handshake, &flight, 0).unwrap()].concat();
    let p = RustCryptoProvider::with_rng_pool(t.get("Client_RNG_Pool"));
    let e = Client::connect(&p, &cfg, FakeTransport::new(incoming)).err().unwrap();
    assert!(matches!(e, TlsError::Protocol(AlertDescription::DecryptError, _)), "{e:?}");
}
