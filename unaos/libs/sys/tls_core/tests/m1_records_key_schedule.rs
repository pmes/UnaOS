//! M1 — record layer (RFC 8446 §5), key schedule (§7.1–7.3), transcript hash (§4.4.1), proven byte-exact against
//! RFC 8448 §3 (simple 1-RTT). Every secret the trace prints is reproduced from the trace's own inputs.

mod common;
use common::*;

use tls_core::crypto::{AeadAlg, CryptoProvider, HashAlg, KxPrivate};
use tls_core::key_schedule::{self, KeySchedule};
use tls_core::msgs::{self, ContentType};
use tls_core::record::{self, RecordProtection};
use tls_core::test_provider::RustCryptoProvider;
use tls_core::transcript::Transcript;

const H: HashAlg = HashAlg::Sha256;

/// RFC 8448 §3 values (each line names the trace step it is printed under).
mod rfc {
    pub const CLIENT_PRIV: &str = "49af42ba7f7994852d713ef2784bcbcaa7911de26adc5642cb634540e7ea5005";
    pub const CLIENT_PUB: &str = "99381de560e4bd43d23d8e435a7dbafeb3c06e51c13cae4d5413691e529aaf2c";
    pub const SERVER_PRIV: &str = "b1580eeadf6dd589b8ef4f2d5652578cc810e9980191ec8d058308cea216a21e";
    pub const SERVER_PUB: &str = "c9828876112095fe66762bdbf7c672e156d6cc253b833df1dd69b1b04e751f0f";
    pub const ECDHE: &str = "8bd4054fb55b9d63fdfbacf9f04b9f0d35e6d63f537563efd46272900f89492d";
    pub const EARLY: &str = "33ad0a1c607ec03b09e6cd9893680ce210adf300aa1f2660e1b22e10f170f92a";
    pub const DERIVED_EARLY: &str = "6f2615a108c702c5678f54fc9dbab69716c076189c48250cebeac3576c3611ba";
    pub const HANDSHAKE: &str = "1dc826e93606aa6fdc0aadc12f741b01046aa6b99f691ed221a9f0ca043fbeac";
    pub const TH_CH_SH: &str = "860c06edc07858ee8e78f0e7428c58edd6b43f2ca3e6e95f02ed063cf0e1cad8";
    pub const C_HS: &str = "b3eddb126e067f35a780b3abf45e2d8f3b1a950738f52e9600746a0e27a55a21";
    pub const S_HS: &str = "b67b7d690cc16c4e75e54213cb2d37b4e9c912bcded9105d42befd59d391ad38";
    pub const DERIVED_HS: &str = "43de77e0c77713859a944db9db2590b53190a65b3ee2e4f12dd7a0bb7ce254b4";
    pub const MASTER: &str = "18df06843d13a08bf2a449844c5f8a478001bc4d4c627984d5a41da8d0402919";
    pub const S_HS_KEY: &str = "3fce516009c21727d0f2e4e86ee403bc";
    pub const S_HS_IV: &str = "5d313eb2671276ee13000b30";
    pub const C_HS_KEY: &str = "dbfaa693d1762c5b666af5d950258d01";
    pub const C_HS_IV: &str = "5bd3c71b836e0b76bb73265f";
    pub const S_FINISHED_KEY: &str = "008d3b66f816ea559f96b537e885c31fc068bf492c652f01f288a1d8cdc19fc8";
    pub const C_FINISHED_KEY: &str = "b80ad01015fb2f0bd65ff7d4da5d6bf83f84821d1f87fdc7d3c75b5a7b42d9c4";
    pub const TH_CH_SF: &str = "9608102a0f1ccc6db6250b7b7e417b1a000eaada3daae4777a7686c9ff83df13";
    pub const C_AP: &str = "9e40646ce79a7f9dc05af8889bce6552875afa0b06df0087f792ebb7c17504a5";
    pub const S_AP: &str = "a11af9f05531f856ad47116b45a950328204b4f44bfb6b3a4b4f1f3fcb631643";
    pub const EXP: &str = "fe22f881176eda18eb8f44529e6792c50c9a3f89452f68d8ae311b4309d3cf50";
    pub const S_AP_KEY: &str = "9f02283b6c9c07efc26bb9f2ac92e356";
    pub const S_AP_IV: &str = "cf782b88dd83549aadf1e984";
    pub const C_AP_KEY: &str = "17422dda596ed5d9acd890e3c63f5051";
    pub const C_AP_IV: &str = "5b78923dee08579033e523d9";
    pub const TH_CH_CF: &str = "209145a96ee8e2a122ff810047cc952684658d6049e86429426db87c54ad143d";
    pub const RES: &str = "7df235f2031d2a051287d02b0241b0bfdaf86cc856231f2d5aba46c434ec196c";
    pub const RES_PSK: &str = "4ecd0eb6ec3b4d87f5d6028f922ca4c5851a277fd41311c9e62d2c9492e1c4f3";
}

fn eq(label: &str, got: &[u8], want: &str) {
    assert_eq!(to_hex(got), want, "{label}");
    println!("RFC8448 byte-exact: {label:<28} yes");
}

fn keys(p: &dyn CryptoProvider, secret: &[u8]) -> (Vec<u8>, [u8; 12]) {
    key_schedule::traffic_keys(p, H, 16, secret).unwrap()
}

struct Msgs {
    ch: Vec<u8>,
    sh: Vec<u8>,
    ee: Vec<u8>,
    cert: Vec<u8>,
    cv: Vec<u8>,
    sf: Vec<u8>,
}

fn trace_msgs() -> (Trace, Msgs) {
    let t = rfc8448("Simple_1RTT_Handshake");
    let ch = t.get("Record_ClientHello_1")[5..].to_vec();
    let m = Msgs {
        ch,
        sh: t.get("Message_ServerHello"),
        ee: t.get("Message_EncryptedExtensions"),
        cert: t.get("Message_Server_Certificate"),
        cv: t.get("Message_Server_CertificateVerify"),
        sf: t.get("Message_Server_Finished"),
    };
    (t, m)
}

#[test]
fn hkdf_rfc5869_case1_default_methods() {
    // The trait's default HKDF (built on the provider's HMAC) against RFC 5869 A.1.
    let p = RustCryptoProvider::new();
    let ikm = [0x0bu8; 22];
    let salt = hex("000102030405060708090a0b0c");
    let info = hex("f0f1f2f3f4f5f6f7f8f9");
    let prk = p.hkdf_extract(H, &salt, &ikm);
    assert_eq!(to_hex(prk.as_bytes()), "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5");
    let mut okm = [0u8; 42];
    p.hkdf_expand(H, prk.as_bytes(), &[&info], &mut okm).unwrap();
    assert_eq!(
        to_hex(&okm),
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
    );
}

#[test]
fn rfc8448_simple_1rtt_every_secret() {
    let p = RustCryptoProvider::new();
    let (t, m) = trace_msgs();

    // (EC)DHE: our X25519 from the trace's client private key and the server share.
    let (_cpriv, cpub) = RustCryptoProvider::with_rng_pool(hex(rfc::CLIENT_PRIV)).x25519_keypair().unwrap();
    eq("client x25519 public", &cpub, rfc::CLIENT_PUB);
    let (_spriv, spub) = RustCryptoProvider::with_rng_pool(hex(rfc::SERVER_PRIV)).x25519_keypair().unwrap();
    eq("server x25519 public", &spub, rfc::SERVER_PUB);
    let shared = p.x25519_shared(&KxPrivate { bytes: hex(rfc::CLIENT_PRIV) }, &hex(rfc::SERVER_PUB)).unwrap();
    eq("ECDHE shared secret", &shared, rfc::ECDHE);

    // The key_share in the trace's ClientHello and ServerHello are those publics.
    let offered = msgs::parse_client_hello(&m.ch).unwrap();
    assert_eq!(offered.random.to_vec(), t.get("Client_RNG_Pool")[..32].to_vec());
    let sh = msgs::parse_server_hello(&m.sh[4..]).unwrap();
    assert_eq!(sh.key_share.as_ref().unwrap().1, hex(rfc::SERVER_PUB));
    assert_eq!(sh.cipher_suite, 0x1301);

    let mut ks = KeySchedule::new(&p, H, None);
    eq("early secret", ks.current().as_bytes(), rfc::EARLY);
    eq("derived (early)", ks.derived().unwrap().as_bytes(), rfc::DERIVED_EARLY);
    ks.input_ecdhe(&shared).unwrap();
    eq("handshake secret", ks.current().as_bytes(), rfc::HANDSHAKE);

    let mut tr = Transcript::new();
    tr.add(&m.ch);
    tr.add(&m.sh);
    let th = tr.hash(&p, H);
    eq("transcript CH..SH", th.as_bytes(), rfc::TH_CH_SH);
    let c_hs = ks.derive(b"c hs traffic", th.as_bytes()).unwrap();
    let s_hs = ks.derive(b"s hs traffic", th.as_bytes()).unwrap();
    eq("c hs traffic", c_hs.as_bytes(), rfc::C_HS);
    eq("s hs traffic", s_hs.as_bytes(), rfc::S_HS);
    let (sk, siv) = keys(&p, s_hs.as_bytes());
    eq("server hs write key", &sk, rfc::S_HS_KEY);
    eq("server hs write iv", &siv, rfc::S_HS_IV);
    let (ck, civ) = keys(&p, c_hs.as_bytes());
    eq("client hs write key", &ck, rfc::C_HS_KEY);
    eq("client hs write iv", &civ, rfc::C_HS_IV);
    eq("server finished key", key_schedule::finished_key(&p, H, s_hs.as_bytes()).unwrap().as_bytes(), rfc::S_FINISHED_KEY);
    eq("client finished key", key_schedule::finished_key(&p, H, c_hs.as_bytes()).unwrap().as_bytes(), rfc::C_FINISHED_KEY);

    eq("derived (handshake)", ks.derived().unwrap().as_bytes(), rfc::DERIVED_HS);
    ks.input_zero().unwrap();
    eq("master secret", ks.current().as_bytes(), rfc::MASTER);

    // Server Finished verify_data over CH..CV equals the trace's Finished body.
    tr.add(&m.ee);
    tr.add(&m.cert);
    tr.add(&m.cv);
    let vd = key_schedule::finished_verify_data(&p, H, s_hs.as_bytes(), tr.hash(&p, H).as_bytes()).unwrap();
    eq("server Finished verify_data", vd.as_bytes(), &to_hex(&m.sf[4..]));
    tr.add(&m.sf);
    let th = tr.hash(&p, H);
    eq("transcript CH..SF", th.as_bytes(), rfc::TH_CH_SF);
    let c_ap = ks.derive(b"c ap traffic", th.as_bytes()).unwrap();
    let s_ap = ks.derive(b"s ap traffic", th.as_bytes()).unwrap();
    let exp = ks.derive(b"exp master", th.as_bytes()).unwrap();
    eq("c ap traffic", c_ap.as_bytes(), rfc::C_AP);
    eq("s ap traffic", s_ap.as_bytes(), rfc::S_AP);
    eq("exp master", exp.as_bytes(), rfc::EXP);
    let (k, iv) = keys(&p, s_ap.as_bytes());
    eq("server ap write key", &k, rfc::S_AP_KEY);
    eq("server ap write iv", &iv, rfc::S_AP_IV);
    let (k, iv) = keys(&p, c_ap.as_bytes());
    eq("client ap write key", &k, rfc::C_AP_KEY);
    eq("client ap write iv", &iv, rfc::C_AP_IV);

    // Client Finished, then res master.
    let cvd = key_schedule::finished_verify_data(&p, H, c_hs.as_bytes(), th.as_bytes()).unwrap();
    let cfin = msgs::handshake_message(msgs::hs::FINISHED, cvd.as_bytes());
    tr.add(&cfin);
    let th = tr.hash(&p, H);
    eq("transcript CH..CF", th.as_bytes(), rfc::TH_CH_CF);
    let res = ks.derive(b"res master", th.as_bytes()).unwrap();
    eq("res master", res.as_bytes(), rfc::RES);
    // The ticket nonce is read from the decrypted NewSessionTicket below; RFC 8448 uses 00 00.
    eq("resumption PSK (nonce 0000)", key_schedule::resumption_psk(&p, H, res.as_bytes(), &[0, 0]).unwrap().as_bytes(), rfc::RES_PSK);
}

#[test]
fn rfc8448_record_layer_byte_exact() {
    let p = RustCryptoProvider::new();
    let (t, m) = trace_msgs();
    let aead = AeadAlg::Aes128Gcm;
    let rp = |k: &str, iv: &str| {
        let mut ivb = [0u8; 12];
        ivb.copy_from_slice(&hex(iv));
        RecordProtection::new(aead, hex(k), ivb)
    };

    // The ClientHello record: plaintext, legacy_record_version 0x0301.
    let rec = record::plaintext_record(ContentType::Handshake, 0x0301, &m.ch);
    assert_eq!(rec, t.get("Record_ClientHello_1"));
    assert_eq!(record::plaintext_record(ContentType::Handshake, 0x0303, &m.sh), t.get("Record_ServerHello"));

    // Server's encrypted flight: EE || Certificate || CertificateVerify || Finished in ONE record.
    let enc = t.get("Record_ServerHandshakeMessages");
    let mut rx = rp(rfc::S_HS_KEY, rfc::S_HS_IV);
    let hdr: [u8; 5] = enc[..5].try_into().unwrap();
    assert_eq!(hdr[..3], [0x17, 0x03, 0x03], "outer header looks like TLS 1.2 application_data");
    let (ty, plain) = rx.open(&p, &hdr, &enc[5..]).unwrap();
    assert_eq!(ty, ContentType::Handshake);
    assert_eq!(plain, [m.ee.clone(), m.cert.clone(), m.cv.clone(), m.sf.clone()].concat());
    println!("RFC8448 byte-exact: server handshake record      yes (decrypts to EE||Cert||CV||Fin)");

    // Client Finished sealed with the client handshake key, sequence 0.
    let mut tr = Transcript::new();
    for x in [&m.ch, &m.sh, &m.ee, &m.cert, &m.cv, &m.sf] {
        tr.add(x);
    }
    let cvd = key_schedule::finished_verify_data(&p, H, &hex(rfc::C_HS), tr.hash(&p, H).as_bytes()).unwrap();
    let cfin = msgs::handshake_message(msgs::hs::FINISHED, cvd.as_bytes());
    let mut tx = rp(rfc::C_HS_KEY, rfc::C_HS_IV);
    assert_eq!(tx.seal(&p, ContentType::Handshake, &cfin, 0).unwrap(), t.get("Record_ClientFinished"));
    println!("RFC8448 byte-exact: client Finished record       yes");

    // Server application records: NewSessionTicket (seq 0), app data (seq 1), close_notify (seq 2).
    let mut srx = rp(rfc::S_AP_KEY, rfc::S_AP_IV);
    let open = |rx: &mut RecordProtection, r: &[u8]| {
        let h: [u8; 5] = r[..5].try_into().unwrap();
        rx.open(&p, &h, &r[5..]).unwrap()
    };
    let (ty, nst) = open(&mut srx, &t.get("Record_NewSessionTicket"));
    assert_eq!(ty, ContentType::Handshake);
    let ticket = msgs::parse_new_session_ticket(&nst[4..]).unwrap();
    assert_eq!(ticket.nonce, vec![0, 0]);
    assert_eq!(ticket.ticket, t.get("SessionTicket"));
    let (ty, data) = open(&mut srx, &t.get("Record_Server_AppData"));
    assert_eq!((ty, data), (ContentType::ApplicationData, t.get("Server_AppData")));
    let (ty, data) = open(&mut srx, &t.get("Record_Server_CloseNotify"));
    assert_eq!((ty, data), (ContentType::Alert, vec![1, 0]));

    // Client application records: app data (seq 0), close_notify (seq 1).
    let mut ctx = rp(rfc::C_AP_KEY, rfc::C_AP_IV);
    assert_eq!(ctx.seal(&p, ContentType::ApplicationData, &t.get("Client_AppData"), 0).unwrap(), t.get("Record_Client_AppData"));
    assert_eq!(ctx.seal(&p, ContentType::Alert, &[1, 0], 0).unwrap(), t.get("Record_Client_CloseNotify"));
    println!("RFC8448 byte-exact: client app data + close_notify yes");
}

#[test]
fn record_padding_nonce_and_limits() {
    let p = RustCryptoProvider::new();
    for aead in [AeadAlg::Aes128Gcm, AeadAlg::Aes256Gcm, AeadAlg::ChaCha20Poly1305] {
        let key = vec![7u8; aead.key_len()];
        let iv = [0x11u8; 12];
        let mut tx = RecordProtection::new(aead, key.clone(), iv);
        let mut rx = RecordProtection::new(aead, key, iv);
        // §5.3 nonce: iv XOR left-padded big-endian sequence number.
        assert_eq!(tx.nonce(), iv);
        // Padding: content + type + 100 zeros; the receiver strips them and recovers the type.
        let r = tx.seal(&p, ContentType::ApplicationData, b"hello", 100).unwrap();
        assert_eq!(r.len(), 5 + 5 + 1 + 100 + 16);
        assert_eq!(&r[..3], &[0x17, 0x03, 0x03]);
        let h: [u8; 5] = r[..5].try_into().unwrap();
        assert_eq!(rx.open(&p, &h, &r[5..]).unwrap(), (ContentType::ApplicationData, b"hello".to_vec()));
        let mut n = iv;
        n[11] ^= 1;
        assert_eq!(tx.nonce(), n, "sequence 1");
        // A zero-length application_data record with padding is legal.
        let r = tx.seal(&p, ContentType::ApplicationData, b"", 7).unwrap();
        let h: [u8; 5] = r[..5].try_into().unwrap();
        assert_eq!(rx.open(&p, &h, &r[5..]).unwrap().1, b"");
        // Tampering → bad_record_mac; replay at the wrong sequence → bad_record_mac.
        let mut r = tx.seal(&p, ContentType::Handshake, b"x", 0).unwrap();
        let last = r.len() - 1;
        r[last] ^= 1;
        let h: [u8; 5] = r[..5].try_into().unwrap();
        assert!(matches!(rx.open(&p, &h, &r[5..]), Err(tls_core::TlsError::BadRecordMac)));
        // Maximum: 2^14 content + type fits; more does not.
        assert!(tx.seal(&p, ContentType::ApplicationData, &vec![0u8; record::MAX_FRAGMENT], 0).is_ok());
        assert!(tx.seal(&p, ContentType::ApplicationData, &vec![0u8; record::MAX_FRAGMENT + 1], 0).is_err());
    }
    // Fragmentation: 40000 bytes → 16384 + 16384 + 7232.
    let data = vec![1u8; 40000];
    let sizes: Vec<usize> = record::fragments(&data, record::MAX_FRAGMENT).map(|f| f.len()).collect();
    assert_eq!(sizes, vec![16384, 16384, 7232]);
    // Header checks.
    assert!(record::check_header(&[0x17, 0x03, 0x03, 0x48, 0x01]).is_err(), "> 2^14+256");
    assert!(record::check_header(&[0x18, 0x03, 0x03, 0x00, 0x01]).is_err(), "unknown type");
    assert!(record::check_header(&[0x16, 0x03, 0x03, 0x00, 0x00]).is_err(), "empty handshake record");
}

#[test]
fn hrr_transcript_message_hash() {
    // RFC 8448 §5: the transcript after HelloRetryRequest starts with message_hash(Hash(ClientHello1)).
    let p = RustCryptoProvider::new();
    let t = rfc8448("HelloRetryRequest_Handshake");
    let ch1 = t.get("Record_ClientHello_1")[5..].to_vec();
    let mut tr = Transcript::new();
    tr.add(&ch1);
    tr.replace_with_message_hash(&p, H);
    let b = tr.bytes();
    assert_eq!(&b[..4], &[0xfe, 0, 0, 32]);
    assert_eq!(&b[4..], p.hash(H, &[&ch1]).as_bytes());
}
