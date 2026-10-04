//! The TLS 1.3 client handshake state machine (RFC 8446 §4, Appendix A.1) over a byte `Transport`.
//!
//! ```text
//!  START --send ClientHello--> WAIT_SH --HRR--> (send ClientHello2) --> WAIT_SH
//!                                 | ServerHello: derive handshake keys
//!                                 v
//!                              WAIT_EE --EncryptedExtensions--> WAIT_CERT_CR
//!                 CertificateRequest → remember (we answer with an empty Certificate)
//!                              WAIT_CERT --Certificate (verify chain)--> WAIT_CV --CertificateVerify--> WAIT_FINISHED
//!                              WAIT_FINISHED --Finished: derive application keys, send [Certificate] Finished--> CONNECTED
//!  CONNECTED: application_data, NewSessionTicket (kept), KeyUpdate (both directions), alerts, close_notify.
//! ```
//!
//! Blocking and allocation-light: `Client::connect` drives the whole handshake; then `send`/`recv`/`close`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{ct_eq, Reader};
use crate::crypto::{CryptoProvider, Digest, HashAlg, KxPrivate};
use crate::error::{AlertDescription, TlsError};
use crate::key_schedule::{self, KeySchedule};
use crate::msgs::{self, hs, CipherSuite, ContentType, NamedGroup, SignatureScheme};
use crate::record::{self, RecordProtection};
use crate::transcript::Transcript;
use crate::x509::{self, PublicKey};

/// A reliable, ordered byte stream (TCP, a pipe, a fake for tests).
pub trait Transport {
    /// Read up to `buf.len()` bytes; `Ok(0)` means the peer closed the stream.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError>;
    /// Write every byte of `data`.
    fn write_all(&mut self, data: &[u8]) -> Result<(), TlsError>;
}

/// Decides whether the server's Certificate message is acceptable and yields the leaf public key that must have
/// signed the CertificateVerify.
pub trait ServerCertVerifier {
    fn verify_server_cert(
        &self,
        provider: &dyn CryptoProvider,
        chain: &[Vec<u8>],
        server_name: Option<&str>,
    ) -> Result<PublicKey, TlsError>;
}

/// Client configuration.
pub struct ClientConfig<'a> {
    /// Sent as SNI (DNS names only) and matched against the leaf (RFC 6125). `None`: no SNI, and the verifier is
    /// told there is no name to check.
    pub server_name: Option<String>,
    /// ALPN protocols in preference order (RFC 7301), e.g. `b"http/1.1"`.
    pub alpn: Vec<Vec<u8>>,
    /// Offered cipher suites in preference order (filtered by what the provider supports).
    pub cipher_suites: Vec<CipherSuite>,
    /// supported_groups in preference order; the FIRST gets a key_share in the initial ClientHello, the others are
    /// reachable through HelloRetryRequest.
    pub groups: Vec<NamedGroup>,
    pub verifier: &'a dyn ServerCertVerifier,
    /// Middlebox compatibility mode (RFC 8446 Appendix D.4): a 32-byte legacy_session_id and one dummy
    /// change_cipher_spec record before the client's second flight.
    pub compat_mode: bool,
    /// Largest plaintext per outgoing record (≤ 2^14); smaller values exercise fragmentation.
    pub max_fragment: usize,
    /// Zero bytes of TLSInnerPlaintext padding added to every protected record we send (§5.4).
    pub padding: usize,
    /// KAT hook: send these exact ClientHello messages (first, then after HRR) instead of building them. The state
    /// machine still generates the key shares from the provider and parses what it offered back out of these bytes.
    #[doc(hidden)]
    pub hello_overrides: Vec<Vec<u8>>,
}

impl<'a> ClientConfig<'a> {
    pub fn new(server_name: Option<&str>, verifier: &'a dyn ServerCertVerifier) -> Self {
        ClientConfig {
            server_name: server_name.map(String::from),
            alpn: Vec::new(),
            cipher_suites: CipherSuite::ALL.to_vec(),
            groups: alloc::vec![NamedGroup::X25519, NamedGroup::Secp256r1],
            verifier,
            compat_mode: true,
            max_fragment: record::MAX_FRAGMENT,
            padding: 0,
            hello_overrides: Vec::new(),
        }
    }
}

/// What the handshake negotiated.
#[derive(Debug, Clone)]
pub struct Negotiated {
    pub cipher_suite: CipherSuite,
    pub group: NamedGroup,
    pub signature_scheme: SignatureScheme,
    pub alpn: Option<Vec<u8>>,
    pub hello_retry: bool,
    pub peer_chain_len: usize,
    pub client_cert_requested: bool,
}

struct Share {
    group: NamedGroup,
    private: KxPrivate,
    public: Vec<u8>,
}

/// A connected TLS 1.3 client.
pub struct Client<'a, T: Transport> {
    p: &'a dyn CryptoProvider,
    cfg: &'a ClientConfig<'a>,
    t: T,
    rx: Option<RecordProtection>,
    tx: Option<RecordProtection>,
    rbuf: Vec<u8>,
    hs_buf: Vec<u8>,
    hash: HashAlg,
    suite: CipherSuite,
    c_ap: Digest,
    s_ap: Digest,
    exporter: Digest,
    resumption: Digest,
    negotiated: Option<Negotiated>,
    pub tickets: Vec<msgs::NewSessionTicket>,
    peer_closed: bool,
    we_closed: bool,
    failed: bool,
    pub key_updates_received: u32,
    sent_ccs: bool,
}

fn unexpected(msg: &'static str) -> TlsError {
    TlsError::Protocol(AlertDescription::UnexpectedMessage, msg)
}
fn illegal(msg: &'static str) -> TlsError {
    TlsError::Protocol(AlertDescription::IllegalParameter, msg)
}

impl<'a, T: Transport> Client<'a, T> {
    /// Runs the full handshake. On failure the appropriate alert has been sent.
    pub fn connect(p: &'a dyn CryptoProvider, cfg: &'a ClientConfig<'a>, transport: T) -> Result<Self, TlsError> {
        let mut c = Client {
            p,
            cfg,
            t: transport,
            rx: None,
            tx: None,
            rbuf: Vec::new(),
            hs_buf: Vec::new(),
            hash: HashAlg::Sha256,
            suite: CipherSuite::Aes128GcmSha256,
            c_ap: Digest::new(&[]),
            s_ap: Digest::new(&[]),
            exporter: Digest::new(&[]),
            resumption: Digest::new(&[]),
            negotiated: None,
            tickets: Vec::new(),
            peer_closed: false,
            we_closed: false,
            failed: false,
            key_updates_received: 0,
            sent_ccs: false,
        };
        match c.handshake() {
            Ok(()) => Ok(c),
            Err(e) => Err(c.fail(e)),
        }
    }

    pub fn negotiated(&self) -> &Negotiated {
        self.negotiated.as_ref().expect("connected")
    }
    pub fn transport(&self) -> &T {
        &self.t
    }
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.t
    }
    pub fn into_transport(self) -> T {
        self.t
    }
    /// exporter_master_secret (RFC 8446 §7.5) — `export` uses it.
    pub fn export(&self, label: &[u8], context: &[u8], out: &mut [u8]) -> Result<(), TlsError> {
        Ok(key_schedule::export(self.p, self.hash, self.exporter.as_bytes(), label, context, out)?)
    }
    /// resumption_master_secret (kept for PSK resumption, which is owed).
    pub fn resumption_master_secret(&self) -> &[u8] {
        self.resumption.as_bytes()
    }

    fn fail(&mut self, e: TlsError) -> TlsError {
        if !self.failed {
            self.failed = true;
            if let Some(a) = e.alert() {
                let _ = self.send_alert(a);
            }
        }
        e
    }

    // ---------------------------------------------------------------- output

    fn send_plain_handshake(&mut self, msg: &[u8], legacy_version: u16) -> Result<(), TlsError> {
        let mut out = Vec::new();
        for frag in record::fragments(msg, self.cfg.max_fragment) {
            out.extend_from_slice(&record::plaintext_record(ContentType::Handshake, legacy_version, frag));
        }
        self.t.write_all(&out)
    }

    fn send_protected(&mut self, ty: ContentType, data: &[u8]) -> Result<(), TlsError> {
        let mut out = Vec::new();
        let tx = self.tx.as_mut().ok_or(TlsError::State("no write keys"))?;
        if data.is_empty() {
            out.extend_from_slice(&tx.seal(self.p, ty, &[], self.cfg.padding)?);
        }
        for frag in record::fragments(data, self.cfg.max_fragment) {
            out.extend_from_slice(&tx.seal(self.p, ty, frag, self.cfg.padding)?);
        }
        self.t.write_all(&out)
    }

    fn send_ccs(&mut self) -> Result<(), TlsError> {
        if self.cfg.compat_mode && !self.sent_ccs {
            self.sent_ccs = true;
            self.t.write_all(&record::plaintext_record(ContentType::ChangeCipherSpec, 0x0303, &[1]))?;
        }
        Ok(())
    }

    fn send_alert(&mut self, a: AlertDescription) -> Result<(), TlsError> {
        let level = if a == AlertDescription::CloseNotify || a == AlertDescription::UserCanceled { 1 } else { 2 };
        let body = [level, a as u8];
        if self.tx.is_some() {
            self.send_protected(ContentType::Alert, &body)
        } else {
            self.t.write_all(&record::plaintext_record(ContentType::Alert, 0x0303, &body))
        }
    }

    // ---------------------------------------------------------------- input

    fn fill(&mut self, n: usize) -> Result<(), TlsError> {
        let mut chunk = [0u8; 4096];
        while self.rbuf.len() < n {
            let got = self.t.read(&mut chunk)?;
            if got == 0 {
                return Err(TlsError::UnexpectedEof);
            }
            self.rbuf.extend_from_slice(&chunk[..got]);
        }
        Ok(())
    }

    fn read_record(&mut self) -> Result<record::RawRecord, TlsError> {
        self.fill(record::HEADER_LEN)?;
        let mut header = [0u8; 5];
        header.copy_from_slice(&self.rbuf[..5]);
        let len = record::check_header(&header)?;
        self.fill(5 + len)?;
        let payload = self.rbuf[5..5 + len].to_vec();
        self.rbuf.drain(..5 + len);
        Ok(record::RawRecord { header, ty: header[0], payload })
    }

    /// Reads one record and returns its (de-protected) content type and content. Handles CCS (ignored during the
    /// handshake, §5) and alerts.
    fn read_content(&mut self, handshaking: bool) -> Result<(ContentType, Vec<u8>), TlsError> {
        loop {
            let rec = self.read_record()?;
            let outer = ContentType::from_u8(rec.ty).ok_or(unexpected("record type"))?;
            if outer == ContentType::ChangeCipherSpec {
                // §5: a single 0x01 CCS between the first ClientHello and the peer's Finished is dropped.
                if handshaking && rec.payload == [1] {
                    continue;
                }
                return Err(unexpected("change_cipher_spec"));
            }
            let (ty, content) = match &mut self.rx {
                None => (outer, rec.payload),
                Some(rx) => {
                    if outer != ContentType::ApplicationData {
                        if outer == ContentType::Alert {
                            // An unprotected alert after keys are in place (e.g. a server rejecting our Finished
                            // before it switched). Honour it as the peer's alert.
                            (outer, rec.payload)
                        } else {
                            return Err(unexpected("unprotected record after key change"));
                        }
                    } else {
                        rx.open(self.p, &rec.header, &rec.payload)?
                    }
                }
            };
            if ty == ContentType::Alert {
                if content.len() != 2 {
                    return Err(TlsError::Decode("alert length"));
                }
                match AlertDescription::from_u8(content[1]) {
                    Some(AlertDescription::CloseNotify) => {
                        self.peer_closed = true;
                        return Ok((ContentType::Alert, content));
                    }
                    Some(AlertDescription::UserCanceled) => continue,
                    Some(a) => {
                        self.failed = true;
                        return Err(TlsError::PeerAlert(a));
                    }
                    None => {
                        self.failed = true;
                        return Err(TlsError::PeerAlertUnknown(content[1]));
                    }
                }
            }
            if ty == ContentType::Handshake && content.is_empty() {
                return Err(TlsError::Decode("empty handshake record"));
            }
            return Ok((ty, content));
        }
    }

    /// The next complete handshake message (header included), reassembled across records.
    fn next_handshake(&mut self) -> Result<Vec<u8>, TlsError> {
        loop {
            if self.hs_buf.len() >= 4 {
                let len = ((self.hs_buf[1] as usize) << 16) | ((self.hs_buf[2] as usize) << 8) | self.hs_buf[3] as usize;
                if len > 1 << 20 {
                    return Err(TlsError::Decode("handshake message too large"));
                }
                if self.hs_buf.len() >= 4 + len {
                    let msg: Vec<u8> = self.hs_buf.drain(..4 + len).collect();
                    return Ok(msg);
                }
            }
            let (ty, content) = self.read_content(true)?;
            match ty {
                ContentType::Handshake => self.hs_buf.extend_from_slice(&content),
                ContentType::Alert => return Err(TlsError::UnexpectedEof),
                _ => return Err(unexpected("application data during handshake")),
            }
        }
    }

    /// §5.1: handshake messages must not span a key change.
    fn assert_hs_boundary(&self) -> Result<(), TlsError> {
        if self.hs_buf.is_empty() { Ok(()) } else { Err(unexpected("handshake data spans a key change")) }
    }

    fn expect(&mut self, ty: u8) -> Result<Vec<u8>, TlsError> {
        let m = self.next_handshake()?;
        if m[0] != ty {
            return Err(unexpected("unexpected handshake message"));
        }
        Ok(m)
    }

    // ---------------------------------------------------------------- handshake

    fn make_share(&self, group: NamedGroup) -> Result<Share, TlsError> {
        Ok(match group {
            NamedGroup::X25519 => {
                let (private, public) = self.p.x25519_keypair()?;
                Share { group, private, public: public.to_vec() }
            }
            NamedGroup::Secp256r1 => {
                let (private, public) = self.p.p256_keypair()?;
                Share { group, private, public }
            }
        })
    }

    fn shared_secret(&self, share: &Share, peer: &[u8]) -> Result<[u8; 32], TlsError> {
        match share.group {
            NamedGroup::X25519 => {
                if peer.len() != 32 {
                    return Err(illegal("x25519 key_exchange length"));
                }
                self.p.x25519_shared(&share.private, peer).map_err(|_| illegal("x25519 shared secret"))
            }
            NamedGroup::Secp256r1 => {
                if peer.len() != 65 || peer[0] != 4 {
                    return Err(illegal("secp256r1 key_exchange must be an uncompressed point"));
                }
                self.p.p256_ecdh(&share.private, peer).map_err(|_| illegal("secp256r1 point"))
            }
        }
    }

    fn offered_suites(&self) -> Vec<CipherSuite> {
        self.cfg
            .cipher_suites
            .iter()
            .copied()
            .filter(|s| self.p.supports_aead(s.aead()) && self.p.supports_hash(s.hash()))
            .collect()
    }

    fn offered_schemes(&self) -> Vec<SignatureScheme> {
        SignatureScheme::ALL.iter().copied().filter(|s| self.p.supports_signature(*s)).collect()
    }

    fn build_hello(
        &self,
        index: usize,
        random: &[u8; 32],
        session_id: &[u8],
        share: &Share,
        cookie: Option<&[u8]>,
    ) -> Vec<u8> {
        if let Some(m) = self.cfg.hello_overrides.get(index) {
            return m.clone();
        }
        let suites = self.offered_suites();
        let schemes = self.offered_schemes();
        let sni = self.cfg.server_name.as_deref().and_then(msgs::sni_name);
        let shares = [(share.group, share.public.as_slice())];
        msgs::encode_client_hello(&msgs::ClientHelloParams {
            random: *random,
            session_id,
            cipher_suites: &suites,
            server_name: sni.as_deref(),
            groups: &self.cfg.groups,
            key_shares: &shares,
            signature_schemes: &schemes,
            alpn: &self.cfg.alpn,
            cookie,
        })
    }

    fn handshake(&mut self) -> Result<(), TlsError> {
        if self.cfg.groups.is_empty() {
            return Err(TlsError::State("no groups configured"));
        }
        let mut random = [0u8; 32];
        self.p.random(&mut random)?;
        let mut session_id = Vec::new();
        if self.cfg.compat_mode {
            session_id.resize(32, 0);
            self.p.random(&mut session_id)?;
        }
        let mut share = self.make_share(self.cfg.groups[0])?;
        let ch1 = self.build_hello(0, &random, &session_id, &share, None);
        let mut offered = msgs::parse_client_hello(&ch1)?;
        if offered.key_share_groups.first() != Some(&share.group.code()) {
            return Err(TlsError::State("ClientHello key_share does not match the generated share"));
        }
        let mut transcript = Transcript::new();
        transcript.add(&ch1);
        self.send_plain_handshake(&ch1, 0x0301)?;

        // ---- ServerHello / HelloRetryRequest
        let mut sh_msg = self.expect(hs::SERVER_HELLO)?;
        let mut sh = msgs::parse_server_hello(&sh_msg[4..])?;
        let mut hello_retry = false;
        let mut hrr_suite = None;
        if sh.is_hrr {
            self.check_sh_common(&sh, &offered)?;
            hello_retry = true;
            let suite = CipherSuite::from_code(sh.cipher_suite).ok_or(illegal("cipher suite"))?;
            hrr_suite = Some(suite);
            // §4.1.4: the HRR must change something, and the selected group must be offered but not yet shared.
            if sh.hrr_group.is_none() && sh.cookie.is_none() {
                return Err(illegal("HelloRetryRequest would not change the ClientHello"));
            }
            if let Some(g) = sh.hrr_group {
                if !offered.groups.contains(&g) || offered.key_share_groups.contains(&g) {
                    return Err(illegal("HelloRetryRequest selected an unusable group"));
                }
                let group = NamedGroup::from_code(g).ok_or(illegal("group"))?;
                share = self.make_share(group)?;
            }
            transcript.replace_with_message_hash(self.p, suite.hash());
            transcript.add(&sh_msg);
            let ch2 = self.build_hello(1, &random, &session_id, &share, sh.cookie.as_deref());
            offered = msgs::parse_client_hello(&ch2)?;
            if offered.key_share_groups.first() != Some(&share.group.code()) {
                return Err(TlsError::State("ClientHello2 key_share does not match the generated share"));
            }
            transcript.add(&ch2);
            self.send_ccs()?;
            self.send_plain_handshake(&ch2, 0x0303)?;
            sh_msg = self.expect(hs::SERVER_HELLO)?;
            sh = msgs::parse_server_hello(&sh_msg[4..])?;
            if sh.is_hrr {
                return Err(unexpected("second HelloRetryRequest"));
            }
        }
        self.check_sh_common(&sh, &offered)?;
        let suite = CipherSuite::from_code(sh.cipher_suite).ok_or(illegal("cipher suite"))?;
        if let Some(s) = hrr_suite {
            if s != suite {
                return Err(illegal("ServerHello cipher suite differs from HelloRetryRequest"));
            }
        }
        if sh.has_pre_shared_key {
            return Err(illegal("pre_shared_key selected but none offered"));
        }
        let (g, peer_key) = sh.key_share.clone().ok_or(TlsError::Protocol(
            AlertDescription::MissingExtension,
            "ServerHello without key_share",
        ))?;
        if g != share.group.code() {
            return Err(illegal("ServerHello key_share group not the one we sent"));
        }
        let shared = self.shared_secret(&share, &peer_key)?;
        drop(share);
        self.suite = suite;
        self.hash = suite.hash();
        let alg = self.hash;
        transcript.add(&sh_msg);

        let mut ks = KeySchedule::new(self.p, alg, None);
        ks.input_ecdhe(&shared)?;
        let th = transcript.hash(self.p, alg);
        let c_hs = ks.derive(b"c hs traffic", th.as_bytes())?;
        let s_hs = ks.derive(b"s hs traffic", th.as_bytes())?;
        self.assert_hs_boundary()?;
        let aead = suite.aead();
        let (k, iv) = key_schedule::traffic_keys(self.p, alg, aead.key_len(), s_hs.as_bytes())?;
        self.rx = Some(RecordProtection::new(aead, k, iv));

        // ---- EncryptedExtensions
        let ee_msg = self.expect(hs::ENCRYPTED_EXTENSIONS)?;
        let ee = msgs::parse_encrypted_extensions(&ee_msg[4..], &offered.extensions, &offered.alpn)?;
        transcript.add(&ee_msg);

        // ---- CertificateRequest? Certificate
        let mut m = self.next_handshake()?;
        let mut cert_request_ctx: Option<Vec<u8>> = None;
        if m[0] == hs::CERTIFICATE_REQUEST {
            let mut r = Reader::new(&m[4..]);
            cert_request_ctx = Some(r.vec8()?.to_vec());
            r.vec16()?;
            r.expect_end()?;
            transcript.add(&m);
            m = self.next_handshake()?;
        }
        if m[0] != hs::CERTIFICATE {
            return Err(unexpected("expected Certificate (PSK modes are not offered)"));
        }
        let chain = msgs::parse_certificate(&m[4..])?;
        if chain.is_empty() {
            return Err(TlsError::Protocol(AlertDescription::DecodeError, "empty server Certificate"));
        }
        let leaf_key = self.cfg.verifier.verify_server_cert(self.p, &chain, self.cfg.server_name.as_deref())?;
        transcript.add(&m);

        // ---- CertificateVerify
        let cv_msg = self.expect(hs::CERTIFICATE_VERIFY)?;
        let (scheme_code, sig) = msgs::parse_certificate_verify(&cv_msg[4..])?;
        let scheme = SignatureScheme::from_code(scheme_code)
            .filter(|s| offered.signature_schemes.contains(&s.code()) && s.allowed_in_certificate_verify())
            .ok_or(illegal("CertificateVerify scheme not offered"))?;
        let content = msgs::certificate_verify_message(transcript.hash(self.p, alg).as_bytes());
        x509::verify_tls_signature(self.p, &leaf_key, scheme, &content, &sig).map_err(|e| match e {
            TlsError::Crypto(crate::crypto::CryptoError::BadSignature) => {
                TlsError::Protocol(AlertDescription::DecryptError, "CertificateVerify signature")
            }
            other => other,
        })?;
        transcript.add(&cv_msg);

        // ---- server Finished
        let fin = self.expect(hs::FINISHED)?;
        let expected = key_schedule::finished_verify_data(self.p, alg, s_hs.as_bytes(), transcript.hash(self.p, alg).as_bytes())?;
        if !ct_eq(&fin[4..], expected.as_bytes()) {
            return Err(TlsError::Protocol(AlertDescription::DecryptError, "server Finished"));
        }
        transcript.add(&fin);
        self.assert_hs_boundary()?;

        // ---- application secrets (transcript through server Finished)
        ks.input_zero()?;
        let th = transcript.hash(self.p, alg);
        self.c_ap = ks.derive(b"c ap traffic", th.as_bytes())?;
        self.s_ap = ks.derive(b"s ap traffic", th.as_bytes())?;
        self.exporter = ks.derive(b"exp master", th.as_bytes())?;
        let (k, iv) = key_schedule::traffic_keys(self.p, alg, aead.key_len(), self.s_ap.as_bytes())?;
        self.rx = Some(RecordProtection::new(aead, k, iv));

        // ---- client second flight
        self.send_ccs()?;
        let (k, iv) = key_schedule::traffic_keys(self.p, alg, aead.key_len(), c_hs.as_bytes())?;
        self.tx = Some(RecordProtection::new(aead, k, iv));
        let mut flight = Vec::new();
        if let Some(ctx) = &cert_request_ctx {
            // No client certificate: an empty certificate_list (§4.4.2.4 lets the server decide).
            let mut body = Vec::new();
            crate::codec::put_vec8(&mut body, ctx);
            crate::codec::put_u24(&mut body, 0);
            let cm = msgs::handshake_message(hs::CERTIFICATE, &body);
            transcript.add(&cm);
            flight.extend_from_slice(&cm);
        }
        let vd = key_schedule::finished_verify_data(self.p, alg, c_hs.as_bytes(), transcript.hash(self.p, alg).as_bytes())?;
        let cfin = msgs::handshake_message(hs::FINISHED, vd.as_bytes());
        transcript.add(&cfin);
        flight.extend_from_slice(&cfin);
        self.send_protected(ContentType::Handshake, &flight)?;
        self.resumption = ks.derive(b"res master", transcript.hash(self.p, alg).as_bytes())?;
        let (k, iv) = key_schedule::traffic_keys(self.p, alg, aead.key_len(), self.c_ap.as_bytes())?;
        self.tx = Some(RecordProtection::new(aead, k, iv));

        let group = NamedGroup::from_code(g).ok_or(illegal("group"))?;
        self.negotiated = Some(Negotiated {
            cipher_suite: suite,
            group,
            signature_scheme: scheme,
            alpn: ee.alpn,
            hello_retry,
            peer_chain_len: chain.len(),
            client_cert_requested: cert_request_ctx.is_some(),
        });
        Ok(())
    }

    fn check_sh_common(&self, sh: &msgs::ServerHello, offered: &msgs::OfferedHello) -> Result<(), TlsError> {
        match sh.selected_version {
            Some(msgs::TLS13) => {}
            Some(_) => return Err(illegal("supported_versions selected a version we did not offer")),
            None => return Err(TlsError::Protocol(AlertDescription::ProtocolVersion, "server does not speak TLS 1.3")),
        }
        if sh.session_id != offered.session_id {
            return Err(illegal("legacy_session_id_echo mismatch"));
        }
        if !offered.cipher_suites.contains(&sh.cipher_suite) {
            return Err(illegal("cipher suite not offered"));
        }
        for e in &sh.extensions {
            // §4.1.4: a HelloRetryRequest may carry a cookie the client never offered.
            if sh.is_hrr && *e == msgs::ext::COOKIE {
                continue;
            }
            if !offered.extensions.contains(e) {
                return Err(TlsError::Protocol(AlertDescription::UnsupportedExtension, "unsolicited ServerHello extension"));
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------- application phase

    /// Sends application data (fragmented to `max_fragment`).
    pub fn send(&mut self, data: &[u8]) -> Result<(), TlsError> {
        if self.we_closed || self.failed {
            return Err(TlsError::Closed);
        }
        if data.is_empty() {
            return Ok(());
        }
        self.send_protected(ContentType::ApplicationData, data)
    }

    /// Receives the next chunk of application data. `Ok(None)` after the peer's close_notify.
    pub fn recv(&mut self) -> Result<Option<Vec<u8>>, TlsError> {
        if self.failed {
            return Err(TlsError::Closed);
        }
        loop {
            if self.peer_closed {
                return Ok(None);
            }
            if !self.hs_buf.is_empty() {
                if let Some(r) = self.try_post_handshake()? {
                    let _ = r;
                    continue;
                }
            }
            let r = self.read_content(false);
            let (ty, content) = match r {
                Ok(v) => v,
                Err(e) => return Err(self.fail(e)),
            };
            match ty {
                ContentType::ApplicationData => {
                    if !self.hs_buf.is_empty() {
                        return Err(self.fail(unexpected("application data inside a handshake message")));
                    }
                    if content.is_empty() {
                        continue;
                    }
                    return Ok(Some(content));
                }
                ContentType::Alert => return Ok(None),
                ContentType::Handshake => {
                    self.hs_buf.extend_from_slice(&content);
                    while let Some(()) = match self.try_post_handshake() {
                        Ok(v) => v,
                        Err(e) => return Err(self.fail(e)),
                    } {}
                }
                ContentType::ChangeCipherSpec => return Err(self.fail(unexpected("change_cipher_spec"))),
            }
        }
    }

    /// Processes one complete post-handshake message from `hs_buf`, if there is one.
    fn try_post_handshake(&mut self) -> Result<Option<()>, TlsError> {
        if self.hs_buf.len() < 4 {
            return Ok(None);
        }
        let len = ((self.hs_buf[1] as usize) << 16) | ((self.hs_buf[2] as usize) << 8) | self.hs_buf[3] as usize;
        if self.hs_buf.len() < 4 + len {
            return Ok(None);
        }
        let msg: Vec<u8> = self.hs_buf.drain(..4 + len).collect();
        match msg[0] {
            hs::NEW_SESSION_TICKET => {
                let t = msgs::parse_new_session_ticket(&msg[4..])?;
                self.tickets.push(t);
            }
            hs::KEY_UPDATE => {
                if msg.len() != 5 || msg[4] > 1 {
                    return Err(illegal("KeyUpdate"));
                }
                // §4.6.3: the message must be the last in its record — the key changes after it.
                if !self.hs_buf.is_empty() {
                    return Err(unexpected("KeyUpdate not at a record boundary"));
                }
                self.key_updates_received += 1;
                self.s_ap = key_schedule::next_traffic_secret(self.p, self.hash, self.s_ap.as_bytes())?;
                let aead = self.suite.aead();
                let (k, iv) = key_schedule::traffic_keys(self.p, self.hash, aead.key_len(), self.s_ap.as_bytes())?;
                self.rx = Some(RecordProtection::new(aead, k, iv));
                if msg[4] == 1 && !self.we_closed {
                    self.send_key_update(false)?;
                }
            }
            hs::CERTIFICATE_REQUEST => {
                return Err(unexpected("post-handshake authentication was not offered"));
            }
            _ => return Err(unexpected("post-handshake message")),
        }
        Ok(Some(()))
    }

    /// Sends KeyUpdate (RFC 8446 §4.6.3) and switches our sending keys. `request_peer` asks the server to update its
    /// keys too.
    pub fn send_key_update(&mut self, request_peer: bool) -> Result<(), TlsError> {
        let m = msgs::handshake_message(hs::KEY_UPDATE, &[request_peer as u8]);
        self.send_protected(ContentType::Handshake, &m)?;
        self.c_ap = key_schedule::next_traffic_secret(self.p, self.hash, self.c_ap.as_bytes())?;
        let aead = self.suite.aead();
        let (k, iv) = key_schedule::traffic_keys(self.p, self.hash, aead.key_len(), self.c_ap.as_bytes())?;
        self.tx = Some(RecordProtection::new(aead, k, iv));
        Ok(())
    }

    /// Sends close_notify (RFC 8446 §6.1). Reading may continue until the peer's close_notify.
    pub fn close(&mut self) -> Result<(), TlsError> {
        if self.we_closed {
            return Ok(());
        }
        self.we_closed = true;
        self.send_alert(AlertDescription::CloseNotify)
    }

    pub fn peer_closed(&self) -> bool {
        self.peer_closed
    }

    /// The current traffic secrets (client, server) — exposed for KATs.
    #[doc(hidden)]
    pub fn traffic_secrets(&self) -> (&[u8], &[u8]) {
        (self.c_ap.as_bytes(), self.s_ap.as_bytes())
    }
}
