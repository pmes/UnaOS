//! The TLS 1.2 client handshake (RFC 5246 §7.3, ECDHE per RFC 8422, AEAD per RFC 5288/5289/7905, the extended
//! master secret per RFC 7627, renegotiation_info per RFC 5746, downgrade protection per RFC 8446 §4.1.3).
//!
//! Only full handshakes: TLS 1.2 session resumption (session IDs, RFC 5077 tickets) is not offered — the
//! resumption this crate does is TLS 1.3's. What is refused, and why:
//! * no extended_master_secret → handshake_failure (RFC 7627 §5.3 permits the abort; without it the master
//!   secret is not bound to the handshake — the triple-handshake attack);
//! * a non-empty renegotiation_info → handshake_failure (RFC 5746 §3.4), and a HelloRequest after the handshake is
//!   answered with a no_renegotiation warning — renegotiation never happens;
//! * the DOWNGRD sentinel in the server random while we offered TLS 1.3 → illegal_parameter (RFC 8446 §4.1.3);
//! * any ServerHello extension we did not offer, or a TLS 1.3-only one → unsupported_extension / illegal_parameter;
//! * a ServerKeyExchange whose curve we did not offer, whose signature scheme we did not offer, or whose signature
//!   kind does not match the suite and certificate → illegal_parameter; a bad signature → decrypt_error;
//! * a server resuming a session ID we never offered → illegal_parameter.

use alloc::vec::Vec;

use super::{illegal, unexpected, Client, Transport};
use crate::codec::{ct_eq, put_u24, put_vec8, Reader};
use crate::error::{AlertDescription, TlsError};
use crate::msgs::{self, ext, hs, CipherSuite, ContentType, NamedGroup, SignatureScheme};
use crate::record::RecordProtection;
use crate::tls12;
use crate::transcript::Transcript;
use crate::x509;

fn handshake_failure(m: &'static str) -> TlsError {
    TlsError::Protocol(AlertDescription::HandshakeFailure, m)
}

impl<'a, T: Transport> Client<'a, T> {
    pub(super) fn handshake12(
        &mut self,
        mut transcript: Transcript,
        offered: &msgs::OfferedHello,
        sh: &msgs::ServerHello,
        sh_msg: &[u8],
        offered13: bool,
    ) -> Result<(), TlsError> {
        self.version = msgs::TLS12;
        // ---- ServerHello (RFC 5246 §7.4.1.3)
        if sh.legacy_version != msgs::TLS12 {
            return Err(TlsError::Protocol(AlertDescription::ProtocolVersion, "server offers TLS 1.1 or older"));
        }
        if sh.is_hrr {
            return Err(illegal("HelloRetryRequest random in a TLS 1.2 ServerHello"));
        }
        // RFC 8446 §4.1.3: a TLS 1.3-capable server that negotiates 1.2 marks its random; seeing the mark while we
        // offered 1.3 means someone removed 1.3 from our hello.
        if offered13 {
            let tail = &sh.random[24..];
            if tail == msgs::DOWNGRADE_TLS12 || tail == msgs::DOWNGRADE_TLS11 {
                return Err(illegal("downgrade sentinel in the ServerHello random (RFC 8446 §4.1.3)"));
            }
        }
        if !offered.cipher_suites.contains(&sh.cipher_suite) {
            return Err(illegal("cipher suite not offered"));
        }
        let suite = CipherSuite::from_code(sh.cipher_suite).ok_or(illegal("cipher suite"))?;
        let auth = suite.auth12().ok_or(illegal("TLS 1.3 cipher suite in a TLS 1.2 ServerHello"))?;
        if !sh.session_id.is_empty() && sh.session_id == offered.session_id {
            return Err(illegal("server resumed a TLS 1.2 session that was never offered"));
        }
        let mut ems = false;
        let mut alpn = None;
        for (ty, data) in &sh.ext_data {
            let ty = *ty;
            // renegotiation_info is solicited by the SCSV (RFC 5746 §3.3), every other one by its extension.
            if ty != ext::RENEGOTIATION_INFO && !offered.extensions.contains(&ty) {
                return Err(TlsError::Protocol(AlertDescription::UnsupportedExtension, "unsolicited ServerHello extension"));
            }
            let mut d = Reader::new(data);
            match ty {
                ext::SERVER_NAME | ext::EXTENDED_MASTER_SECRET => {
                    if !data.is_empty() {
                        return Err(TlsError::Decode("extension must be empty"));
                    }
                    if ty == ext::EXTENDED_MASTER_SECRET {
                        ems = true;
                    }
                }
                ext::RENEGOTIATION_INFO => {
                    // renegotiated_connection<0..255> must be empty on an initial handshake.
                    if data.as_slice() != [0u8] {
                        return Err(handshake_failure("renegotiation_info is not empty (RFC 5746 §3.4)"));
                    }
                }
                ext::ALPN => {
                    let mut l = Reader::new(d.vec16()?);
                    let proto = l.vec8()?;
                    l.expect_end()?;
                    d.expect_end()?;
                    if proto.is_empty() || !offered.alpn.iter().any(|p| p.as_slice() == proto) {
                        return Err(illegal("server selected an ALPN protocol not offered"));
                    }
                    alpn = Some(proto.to_vec());
                }
                ext::EC_POINT_FORMATS => {
                    let f = d.vec8()?;
                    d.expect_end()?;
                    if !f.contains(&0) {
                        return Err(illegal("server does not accept uncompressed points (RFC 8422 §5.2)"));
                    }
                }
                ext::KEY_SHARE | ext::SUPPORTED_VERSIONS | ext::PRE_SHARED_KEY | ext::COOKIE | ext::EARLY_DATA => {
                    return Err(illegal("TLS 1.3 extension in a TLS 1.2 ServerHello"));
                }
                _ => return Err(TlsError::Protocol(AlertDescription::UnsupportedExtension, "extension not permitted in a TLS 1.2 ServerHello")),
            }
        }
        if !ems {
            return Err(handshake_failure("extended_master_secret required (RFC 7627)"));
        }
        self.suite = suite;
        self.hash = suite.hash();
        let alg = self.hash;
        transcript.add(sh_msg);
        let client_random = offered.random;
        let server_random = sh.random;

        // ---- Certificate (§7.4.2)
        let m = self.expect(hs::CERTIFICATE)?;
        let chain = msgs::parse_certificate12(&m[4..])?;
        if chain.is_empty() {
            return Err(TlsError::Protocol(AlertDescription::DecodeError, "empty server Certificate"));
        }
        transcript.add(&m);
        let leaf_key = self.cfg.verifier.verify_server_cert(self.p, &chain, self.cfg.server_name.as_deref())?;

        // ---- ServerKeyExchange (RFC 8422 §5.4)
        let m = self.next_handshake()?;
        if m[0] != hs::SERVER_KEY_EXCHANGE {
            return Err(unexpected("expected ServerKeyExchange (ECDHE suites only)"));
        }
        let ske = msgs::parse_server_key_exchange(&m[4..])?;
        transcript.add(&m);
        if !offered.groups.contains(&ske.group) {
            return Err(illegal("ServerKeyExchange curve not offered"));
        }
        let group = NamedGroup::from_code(ske.group).ok_or(illegal("ServerKeyExchange curve"))?;
        let scheme = SignatureScheme::from_code(ske.scheme)
            .filter(|s| offered.signature_schemes.contains(&s.code()))
            .ok_or(illegal("ServerKeyExchange signature scheme not offered"))?;
        let mut signed = Vec::with_capacity(64 + ske.params.len());
        signed.extend_from_slice(&client_random);
        signed.extend_from_slice(&server_random);
        signed.extend_from_slice(&ske.params);
        x509::verify_tls12_signature(self.p, &leaf_key, auth, scheme, &signed, &ske.signature)?;

        // ---- CertificateRequest? ServerHelloDone (§7.4.4, §7.4.5)
        let mut m = self.next_handshake()?;
        let mut cert_requested = false;
        if m[0] == hs::CERTIFICATE_REQUEST {
            msgs::parse_certificate_request12(&m[4..])?;
            transcript.add(&m);
            cert_requested = true;
            m = self.next_handshake()?;
        }
        if m[0] != hs::SERVER_HELLO_DONE {
            return Err(unexpected("expected ServerHelloDone"));
        }
        if m.len() != 4 {
            return Err(TlsError::Decode("ServerHelloDone is not empty"));
        }
        transcript.add(&m);
        self.assert_hs_boundary()?;

        // ---- client flight: [Certificate] ClientKeyExchange, ChangeCipherSpec, Finished
        let share = self.make_share(group)?;
        let shared = self.shared_secret(&share, &ske.public)?;
        let mut flight = Vec::new();
        if cert_requested {
            // No client certificate: an empty certificate_list (RFC 5246 §7.4.6).
            let mut body = Vec::new();
            put_u24(&mut body, 0);
            let cm = msgs::handshake_message(hs::CERTIFICATE, &body);
            transcript.add(&cm);
            flight.extend_from_slice(&cm);
        }
        let mut cke = Vec::new();
        put_vec8(&mut cke, &share.public);
        let cke = msgs::handshake_message(hs::CLIENT_KEY_EXCHANGE, &cke);
        transcript.add(&cke);
        flight.extend_from_slice(&cke);
        drop(share);
        // RFC 7627 §4: session_hash = Hash(ClientHello … ClientKeyExchange).
        let session_hash = transcript.hash(self.p, alg);
        let master = tls12::extended_master_secret(self.p, alg, &shared, session_hash.as_bytes())?;
        let aead = suite.aead();
        let kb = tls12::key_block(self.p, alg, aead, &master, &client_random, &server_random)?;
        self.send_plain_handshake(&flight, msgs::TLS12)?;
        self.t.write_all(&crate::record::plaintext_record(ContentType::ChangeCipherSpec, msgs::TLS12, &[1]))?;
        self.tx = Some(RecordProtection::new12(aead, kb.client_key.clone(), &kb.client_iv)?);
        let vd = tls12::finished(self.p, alg, &master, b"client finished", transcript.hash(self.p, alg).as_bytes())?;
        let cfin = msgs::handshake_message(hs::FINISHED, &vd);
        transcript.add(&cfin);
        self.send_protected(ContentType::Handshake, &cfin)?;

        // ---- server ChangeCipherSpec, Finished
        let (ty, _) = self.read_content(true)?;
        if ty != ContentType::ChangeCipherSpec {
            return Err(unexpected("expected ChangeCipherSpec"));
        }
        self.assert_hs_boundary()?;
        self.rx = Some(RecordProtection::new12(aead, kb.server_key.clone(), &kb.server_iv)?);
        let fin = self.expect(hs::FINISHED)?;
        let expected = tls12::finished(self.p, alg, &master, b"server finished", transcript.hash(self.p, alg).as_bytes())?;
        if !ct_eq(&fin[4..], &expected) {
            return Err(TlsError::Protocol(AlertDescription::DecryptError, "server Finished"));
        }
        self.assert_hs_boundary()?;
        self.randoms[..32].copy_from_slice(&client_random);
        self.randoms[32..].copy_from_slice(&server_random);
        self.master12 = Some(master);
        self.negotiated = Some(super::Negotiated {
            version: msgs::TLS12,
            cipher_suite: suite,
            group,
            signature_scheme: scheme,
            alpn,
            hello_retry: false,
            peer_chain_len: chain.len(),
            client_cert_requested: cert_requested,
            extended_master_secret: true,
            resumed: false,
        });
        Ok(())
    }
}
