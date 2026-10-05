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
//!
//! TLS 1.2 (RFC 5246, `handshake12` below) is spoken when the configuration offers a 1.2 suite
//! ([`ClientConfig::enable_tls12`]) and the ServerHello carries no supported_versions:
//!
//! ```text
//!  ServerHello (EMS required, renegotiation_info empty, no downgrade sentinel if we offered 1.3)
//!   → Certificate → [CertificateStatus] → ServerKeyExchange (ECDHE, signed) → [CertificateRequest] → ServerHelloDone
//!   ← [Certificate(empty)] ClientKeyExchange, ChangeCipherSpec, Finished
//!   → ChangeCipherSpec, Finished → CONNECTED (HelloRequest answered with a no_renegotiation warning)
//! ```

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

#[path = "client12.rs"]
mod client12;

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

    /// The full input — chain plus a stapled OCSP response and TLS-delivered SCTs (TLSCORE2). The default runs
    /// `verify_server_cert` and reports a staple as `NotChecked`; [`x509::WebPkiVerifier`] verifies it.
    fn verify_server_cert_full(
        &self,
        provider: &dyn CryptoProvider,
        peer: &x509::PeerCertificates<'_>,
        server_name: Option<&str>,
    ) -> Result<x509::CertVerdict, TlsError> {
        use x509::ocsp::OcspStatus;
        let key = self.verify_server_cert(provider, peer.chain, server_name)?;
        let ocsp = match (peer.ocsp, peer.ocsp_requested) {
            (Some(_), _) => OcspStatus::NotChecked,
            (None, true) => OcspStatus::NotStapled,
            (None, false) => OcspStatus::NotRequested,
        };
        let scts = x509::verify::collect_scts(peer.chain.first().map(|v| v.as_slice()), peer.sct_list);
        Ok(x509::CertVerdict { key, ocsp, scts, pool_intermediates: 0, ct: crate::ct::CtVerdict::off() })
    }
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
    /// TLS 1.3 resumption (RFC 8446 §2.2): tickets the server sends are kept in this store, and the next
    /// connection to the same name offers one as a PSK with (EC)DHE. `None`: no psk_key_exchange_modes is sent
    /// and tickets are only parsed. 0-RTT is never offered (see `resumption`).
    pub resumption: Option<crate::resumption::Resumption<'a>>,
    /// Ask for a stapled OCSP response (status_request, RFC 6066 §8); the verifier checks it when it comes.
    pub request_ocsp: bool,
    /// Ask for TLS-delivered SCTs (signed_certificate_timestamp, RFC 6962 §3.3.1).
    pub request_sct: bool,
}

impl ClientConfig<'_> {
    /// Also offer TLS 1.2 (RFC 5246) with the ECDHE + AEAD suites ([`CipherSuite::TLS12`]): supported_versions
    /// then lists 1.3 and 1.2, and the 1.2 extensions (extended_master_secret, ec_point_formats) and the
    /// renegotiation SCSV are added. TLS 1.3 stays preferred; a 1.2 answer from a 1.3-capable server is caught by
    /// the RFC 8446 §4.1.3 downgrade sentinel.
    pub fn enable_tls12(&mut self) {
        for s in CipherSuite::TLS12 {
            if !self.cipher_suites.contains(&s) {
                self.cipher_suites.push(s);
            }
        }
    }
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
            resumption: None,
            request_ocsp: true,
            request_sct: true,
        }
    }
}

/// What the handshake negotiated.
#[derive(Debug, Clone)]
pub struct Negotiated {
    /// 0x0304 (TLS 1.3) or 0x0303 (TLS 1.2).
    pub version: u16,
    pub cipher_suite: CipherSuite,
    pub group: NamedGroup,
    pub signature_scheme: SignatureScheme,
    pub alpn: Option<Vec<u8>>,
    pub hello_retry: bool,
    pub peer_chain_len: usize,
    pub client_cert_requested: bool,
    /// TLS 1.2: the RFC 7627 extended master secret was negotiated (always true — it is required).
    pub extended_master_secret: bool,
    /// TLS 1.3: the server accepted our ticket (PSK + (EC)DHE); no certificate was exchanged on this connection —
    /// the ticket came from a connection whose chain was verified for the same server name.
    pub resumed: bool,
    /// The stapled OCSP verdict (RFC 6960).
    pub ocsp: x509::ocsp::OcspStatus,
    /// Certificate Transparency SCTs seen (embedded, TLS, OCSP).
    pub scts: Vec<x509::sct::Sct>,
    /// CTCORE: the CT verdict (`ct=` policy / no_scts / insufficient / bad_sig / stale_list / off).
    pub ct: crate::ct::CtVerdict,
    /// Intermediates the verifier took from the trust store's pool because the server omitted them.
    pub pool_intermediates: usize,
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
    /// 0 until the ServerHello, then msgs::TLS13 or msgs::TLS12.
    version: u16,
    master12: Option<crate::tls12::MasterSecret>,
    randoms: [u8; 64],
    /// TLS 1.2: HelloRequests refused with a no_renegotiation warning.
    pub renegotiations_refused: u32,
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
            version: 0,
            master12: None,
            randoms: [0; 64],
            renegotiations_refused: 0,
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
    ///
    /// TLS 1.2: RFC 5705 over the extended master secret, the context always present (possibly empty).
    pub fn export(&self, label: &[u8], context: &[u8], out: &mut [u8]) -> Result<(), TlsError> {
        if let Some(m) = &self.master12 {
            let mut cr = [0u8; 32];
            let mut sr = [0u8; 32];
            cr.copy_from_slice(&self.randoms[..32]);
            sr.copy_from_slice(&self.randoms[32..]);
            return Ok(crate::tls12::export(self.p, self.hash, m, label, &cr, &sr, Some(context), out)?);
        }
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
        let level = if matches!(a, AlertDescription::CloseNotify | AlertDescription::UserCanceled | AlertDescription::NoRenegotiation) {
            1
        } else {
            2
        };
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
        let max = if self.version == msgs::TLS12 { record::MAX_CIPHERTEXT12 } else { record::MAX_CIPHERTEXT };
        let len = record::check_header_max(&header, max)?;
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
                if self.version == msgs::TLS12 {
                    // RFC 5246 §7.1: exactly one CCS, before the server's Finished, while reads are unprotected.
                    if handshaking && self.rx.is_none() && rec.payload == [1] {
                        return Ok((ContentType::ChangeCipherSpec, rec.payload));
                    }
                    return Err(unexpected("change_cipher_spec"));
                }
                // §5: a single 0x01 CCS between the first ClientHello and the peer's Finished is dropped.
                if handshaking && rec.payload == [1] {
                    continue;
                }
                return Err(unexpected("change_cipher_spec"));
            }
            let (ty, content) = match &mut self.rx {
                None => (outer, rec.payload),
                Some(rx) if rx.is_tls12() => rx.open(self.p, &rec.header, &rec.payload)?,
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
                // TLS 1.2 (RFC 5246 §7.2): a warning-level alert other than close_notify does not end the
                // connection (servers send e.g. a warning unrecognized_name). TLS 1.3 treats every alert but
                // close_notify / user_canceled as fatal whatever its level (§6).
                if self.version == msgs::TLS12 && content[0] == 1 && content[1] != AlertDescription::CloseNotify as u8 {
                    continue;
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
                ContentType::ChangeCipherSpec => return Err(unexpected("change_cipher_spec inside a handshake flight")),
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
        psk: Option<&crate::resumption::Ticket>,
    ) -> Vec<u8> {
        if let Some(m) = self.cfg.hello_overrides.get(index) {
            return m.clone();
        }
        let now_ms = self.cfg.resumption.map(|r| r.clock.now_ms()).unwrap_or(0);
        let suites = self.offered_suites();
        let schemes = self.offered_schemes();
        let sni = self.cfg.server_name.as_deref().and_then(msgs::sni_name);
        let (v13, v12) = self.offered_versions();
        let shares = [(share.group, share.public.as_slice())];
        let mut versions = Vec::new();
        if v13 {
            versions.push(msgs::TLS13);
            if v12 {
                versions.push(msgs::TLS12);
            }
        }
        msgs::encode_client_hello(&msgs::ClientHelloParams {
            random: *random,
            session_id,
            cipher_suites: &suites,
            server_name: sni.as_deref(),
            groups: &self.cfg.groups,
            key_shares: if v13 { &shares } else { &[] },
            signature_schemes: &schemes,
            alpn: &self.cfg.alpn,
            cookie,
            versions: &versions,
            tls12: v12,
            status_request: self.cfg.request_ocsp,
            sct: self.cfg.request_sct,
            psk_dhe_mode: v13 && self.cfg.resumption.is_some(),
            psk: psk.filter(|_| v13).map(|t| msgs::PskOffer {
                identity: &t.ticket,
                obfuscated_ticket_age: t.obfuscated_age(now_ms),
                binder_len: t.suite.hash().output_len(),
            }),
        })
    }

    /// A ticket to offer: one for this exact server name, still in its lifetime, whose suite hash some offered
    /// TLS 1.3 suite shares (RFC 8446 §4.2.11). Taken out of the store: tickets are single-use.
    fn pick_ticket(&self) -> Option<crate::resumption::Ticket> {
        let r = self.cfg.resumption?;
        if !self.cfg.hello_overrides.is_empty() || !self.offered_versions().0 {
            return None;
        }
        let name = self.cfg.server_name.as_deref()?;
        let now = r.clock.now_ms();
        let suites = self.offered_suites();
        // Expired tickets are dropped; usable ones this configuration cannot offer (another hash) go back.
        let mut keep = Vec::new();
        let mut found = None;
        while let Some(t) = r.store.take(name) {
            if !t.usable_at(now) {
                continue;
            }
            if suites.iter().any(|s| s.is_tls13() && s.hash() == t.suite.hash()) {
                found = Some(t);
                break;
            }
            keep.push(t);
        }
        for t in keep.into_iter().rev() {
            r.store.put(t);
        }
        found
    }

    /// RFC 8446 §4.2.11.2: binder = HMAC(finished_key(binder_key), Transcript-Hash(prefix + truncated hello)),
    /// binder_key = Derive-Secret(Early Secret(PSK), "res binder", ""). Written over the zeros the hello carries.
    fn patch_binder(&self, hello: &mut [u8], t: &crate::resumption::Ticket, prefix: &[u8]) -> Result<(), TlsError> {
        let alg = t.suite.hash();
        let hl = alg.output_len();
        let cut = hello.len().checked_sub(msgs::psk_binders_len(hl)).ok_or(TlsError::State("hello too short for a binder"))?;
        let early = KeySchedule::new(self.p, alg, Some(&t.psk));
        let binder_key = early.derive(b"res binder", self.p.hash(alg, &[]).as_bytes())?;
        let th = self.p.hash(alg, &[prefix, &hello[..cut]]);
        let b = key_schedule::finished_verify_data(self.p, alg, binder_key.as_bytes(), th.as_bytes())?;
        let n = hello.len();
        hello[n - hl..].copy_from_slice(b.as_bytes());
        Ok(())
    }

    /// (TLS 1.3 offered, TLS 1.2 offered) — decided by which suites the configuration (and provider) allow.
    fn offered_versions(&self) -> (bool, bool) {
        let s = self.offered_suites();
        (s.iter().any(|c| c.is_tls13()), s.iter().any(|c| !c.is_tls13()))
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
        let mut ticket = self.pick_ticket();
        let mut ch1 = self.build_hello(0, &random, &session_id, &share, None, ticket.as_ref());
        if let Some(t) = &ticket {
            self.patch_binder(&mut ch1, t, &[])?;
        }
        let mut offered = msgs::parse_client_hello(&ch1)?;
        if self.offered_versions().0 && offered.key_share_groups.first() != Some(&share.group.code()) {
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
            // §4.1.4: keep the PSK only if its hash matches the suite the HRR chose.
            if ticket.as_ref().is_some_and(|t| t.suite.hash() != suite.hash()) {
                ticket = None;
            }
            let mut ch2 = self.build_hello(1, &random, &session_id, &share, sh.cookie.as_deref(), ticket.as_ref());
            if let Some(t) = &ticket {
                self.patch_binder(&mut ch2, t, transcript.bytes())?;
            }
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
        if !hello_retry && sh.selected_version.is_none() && self.offered_versions().1 {
            // No supported_versions: a TLS 1.2 (or older) ServerHello.
            let offered13 = self.offered_versions().0;
            drop(share);
            return self.handshake12(transcript, &offered, &sh, &sh_msg, offered13);
        }
        self.check_sh_common(&sh, &offered)?;
        self.version = msgs::TLS13;
        let suite = CipherSuite::from_code(sh.cipher_suite).ok_or(illegal("cipher suite"))?;
        if let Some(s) = hrr_suite {
            if s != suite {
                return Err(illegal("ServerHello cipher suite differs from HelloRetryRequest"));
            }
        }
        // §4.2.11: the selected identity must be one we offered, with a suite of the PSK's hash.
        let resumed = match sh.psk_identity {
            None => false,
            Some(i) => {
                let t = ticket.as_ref().ok_or(illegal("pre_shared_key selected but none offered"))?;
                if i as usize >= offered.psk_identities {
                    return Err(illegal("pre_shared_key selected_identity out of range"));
                }
                if t.suite.hash() != suite.hash() {
                    return Err(illegal("PSK accepted with a cipher suite of another hash"));
                }
                true
            }
        };
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

        let psk = if resumed { ticket.as_ref().map(|t| t.psk.clone()) } else { None };
        let mut ks = KeySchedule::new(self.p, alg, psk.as_deref());
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

        // ---- resumed (PSK): no CertificateRequest, Certificate, CertificateVerify (§2.2, §4.3.2)
        let mut cert_request_ctx: Option<Vec<u8>> = None;
        let mut cert_verdict: Option<x509::CertVerdict> = None;
        let (scheme, chain_len) = if resumed {
            let t = ticket.as_ref().expect("resumed implies a ticket");
            (SignatureScheme::from_code(t.signature_scheme).unwrap_or(SignatureScheme::EcdsaSecp256r1Sha256), 0)
        } else {
            // ---- CertificateRequest? Certificate
            let mut m = self.next_handshake()?;
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
            let certs = msgs::parse_certificate_entries(&m[4..], &offered.extensions)?;
            let chain = certs.chain;
            if chain.is_empty() {
                return Err(TlsError::Protocol(AlertDescription::DecodeError, "empty server Certificate"));
            }
            let peer = x509::PeerCertificates {
                chain: &chain,
                ocsp: certs.ocsp.as_deref(),
                sct_list: certs.sct_list.as_deref(),
                ocsp_requested: offered.extensions.contains(&msgs::ext::STATUS_REQUEST),
            };
            let verdict = self.cfg.verifier.verify_server_cert_full(self.p, &peer, self.cfg.server_name.as_deref())?;
            let leaf_key = verdict.key.clone();
            cert_verdict = Some(verdict);
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
            (scheme, chain.len())
        };
        let peer_alpn = ee.alpn.clone();

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
            alpn: peer_alpn,
            hello_retry,
            peer_chain_len: chain_len,
            client_cert_requested: cert_request_ctx.is_some(),
            extended_master_secret: false,
            version: msgs::TLS13,
            resumed,
            ocsp: cert_verdict.as_ref().map(|v| v.ocsp.clone()).unwrap_or(x509::ocsp::OcspStatus::NotRequested),
            scts: cert_verdict.as_ref().map(|v| v.scts.clone()).unwrap_or_default(),
            ct: cert_verdict.as_ref().map(|v| v.ct.clone()).unwrap_or_else(crate::ct::CtVerdict::off),
            pool_intermediates: cert_verdict.as_ref().map(|v| v.pool_intermediates).unwrap_or(0),
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
        if !CipherSuite::from_code(sh.cipher_suite).is_some_and(|c| c.is_tls13()) {
            return Err(illegal("TLS 1.2 cipher suite in a TLS 1.3 ServerHello"));
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
        if self.version == msgs::TLS12 {
            // RFC 5746 / RFC 5246 §7.4.1.1: renegotiation is never performed. A HelloRequest gets a warning
            // no_renegotiation and the connection carries on; any other handshake message is a protocol error.
            if msg[0] == hs::HELLO_REQUEST && msg.len() == 4 {
                self.renegotiations_refused += 1;
                if !self.we_closed {
                    self.send_alert(AlertDescription::NoRenegotiation)?;
                }
                return Ok(Some(()));
            }
            return Err(unexpected("handshake message after the TLS 1.2 handshake"));
        }
        match msg[0] {
            hs::NEW_SESSION_TICKET => {
                let t = msgs::parse_new_session_ticket(&msg[4..])?;
                self.keep_ticket(&t)?;
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

    /// §4.6.1: PSK = HKDF-Expand-Label(resumption_master_secret, "resumption", ticket_nonce, Hash.length), kept in
    /// the caller's store under the server name. A ticket with lifetime 0 or over 7 days is not kept.
    fn keep_ticket(&mut self, t: &msgs::NewSessionTicket) -> Result<(), TlsError> {
        let (Some(r), Some(name)) = (self.cfg.resumption, self.cfg.server_name.as_deref()) else { return Ok(()) };
        if t.lifetime == 0 || t.lifetime > crate::resumption::MAX_TICKET_LIFETIME {
            return Ok(());
        }
        let psk = key_schedule::resumption_psk(self.p, self.hash, self.resumption.as_bytes(), &t.nonce)?;
        let n = self.negotiated.as_ref();
        r.store.put(crate::resumption::Ticket {
            server_name: String::from(name),
            suite: self.suite,
            psk: psk.as_bytes().to_vec(),
            ticket: t.ticket.clone(),
            age_add: t.age_add,
            lifetime: t.lifetime,
            received_ms: r.clock.now_ms(),
            alpn: n.and_then(|n| n.alpn.clone()),
            signature_scheme: n.map(|n| n.signature_scheme.code()).unwrap_or(0),
            max_early_data: t.max_early_data,
        });
        Ok(())
    }

    /// Sends KeyUpdate (RFC 8446 §4.6.3) and switches our sending keys. `request_peer` asks the server to update its
    /// keys too.
    pub fn send_key_update(&mut self, request_peer: bool) -> Result<(), TlsError> {
        if self.version != msgs::TLS13 {
            return Err(TlsError::State("KeyUpdate is TLS 1.3 only"));
        }
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
