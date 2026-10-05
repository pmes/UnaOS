//! The TLS 1.3 key schedule (RFC 8446 §7.1–§7.3, §4.4.4, §4.6.3, §7.5).
//!
//! ```text
//!              0
//!              |
//!    PSK ->  HKDF-Extract = Early Secret
//!              |
//!              Derive-Secret(., "derived", "")
//!              |
//!    (EC)DHE -> HKDF-Extract = Handshake Secret --> c/s hs traffic
//!              |
//!              Derive-Secret(., "derived", "")
//!              |
//!    0 ->    HKDF-Extract = Master Secret --> c/s ap traffic, exp master, res master
//! ```

use alloc::vec::Vec;

use crate::crypto::{CryptoError, CryptoProvider, Digest, HashAlg, MAX_HASH_LEN};

/// HKDF-Expand-Label into a fresh `Digest` of `len` bytes (len ≤ 64).
pub fn expand_label(
    p: &dyn CryptoProvider,
    alg: HashAlg,
    secret: &[u8],
    label: &[u8],
    context: &[u8],
    len: usize,
) -> Result<Digest, CryptoError> {
    let mut out = [0u8; MAX_HASH_LEN];
    p.hkdf_expand_label(alg, secret, label, context, &mut out[..len])?;
    Ok(Digest::new(&out[..len]))
}

/// HKDF-Expand-Label into a Vec (for keys).
pub fn expand_label_vec(
    p: &dyn CryptoProvider,
    alg: HashAlg,
    secret: &[u8],
    label: &[u8],
    context: &[u8],
    len: usize,
) -> Result<Vec<u8>, CryptoError> {
    let mut out = alloc::vec![0u8; len];
    p.hkdf_expand_label(alg, secret, label, context, &mut out)?;
    Ok(out)
}

/// Derive-Secret(Secret, Label, Messages) = HKDF-Expand-Label(Secret, Label, Transcript-Hash(Messages), Hash.length)
/// — here the caller passes the transcript hash already computed.
pub fn derive_secret(
    p: &dyn CryptoProvider,
    alg: HashAlg,
    secret: &[u8],
    label: &[u8],
    transcript_hash: &[u8],
) -> Result<Digest, CryptoError> {
    expand_label(p, alg, secret, label, transcript_hash, alg.output_len())
}

/// The secrets of one connection's key schedule.
pub struct KeySchedule<'p> {
    p: &'p dyn CryptoProvider,
    pub alg: HashAlg,
    /// The current stage secret (early → handshake → master).
    current: Digest,
}

impl<'p> KeySchedule<'p> {
    /// Early Secret = HKDF-Extract(0, PSK or 0^HashLen).
    pub fn new(p: &'p dyn CryptoProvider, alg: HashAlg, psk: Option<&[u8]>) -> Self {
        let zeros = [0u8; MAX_HASH_LEN];
        let ikm = psk.unwrap_or(&zeros[..alg.output_len()]);
        let current = p.hkdf_extract(alg, &[], ikm);
        KeySchedule { p, alg, current }
    }

    pub fn current(&self) -> &Digest {
        &self.current
    }

    fn empty_hash(&self) -> Digest {
        self.p.hash(self.alg, &[])
    }

    /// Derive-Secret(current, "derived", "") — the salt of the next stage.
    pub fn derived(&self) -> Result<Digest, CryptoError> {
        let eh = self.empty_hash();
        derive_secret(self.p, self.alg, self.current.as_bytes(), b"derived", eh.as_bytes())
    }

    /// Handshake Secret = HKDF-Extract(Derive-Secret(Early, "derived", ""), (EC)DHE).
    pub fn input_ecdhe(&mut self, shared: &[u8]) -> Result<(), CryptoError> {
        let salt = self.derived()?;
        self.current = self.p.hkdf_extract(self.alg, salt.as_bytes(), shared);
        Ok(())
    }

    /// Master Secret = HKDF-Extract(Derive-Secret(Handshake, "derived", ""), 0).
    pub fn input_zero(&mut self) -> Result<(), CryptoError> {
        let salt = self.derived()?;
        let zeros = [0u8; MAX_HASH_LEN];
        self.current = self.p.hkdf_extract(self.alg, salt.as_bytes(), &zeros[..self.alg.output_len()]);
        Ok(())
    }

    /// Derive-Secret(current, label, transcript_hash).
    pub fn derive(&self, label: &[u8], transcript_hash: &[u8]) -> Result<Digest, CryptoError> {
        derive_secret(self.p, self.alg, self.current.as_bytes(), label, transcript_hash)
    }
}

/// [sender]_write_key / [sender]_write_iv from a traffic secret (RFC 8446 §7.3).
pub fn traffic_keys(
    p: &dyn CryptoProvider,
    alg: HashAlg,
    key_len: usize,
    traffic_secret: &[u8],
) -> Result<(Vec<u8>, [u8; 12]), CryptoError> {
    let key = expand_label_vec(p, alg, traffic_secret, b"key", &[], key_len)?;
    let ivd = expand_label(p, alg, traffic_secret, b"iv", &[], 12)?;
    let mut iv = [0u8; 12];
    iv.copy_from_slice(ivd.as_bytes());
    Ok((key, iv))
}

/// finished_key = HKDF-Expand-Label(BaseKey, "finished", "", Hash.length) (RFC 8446 §4.4.4).
pub fn finished_key(p: &dyn CryptoProvider, alg: HashAlg, base_key: &[u8]) -> Result<Digest, CryptoError> {
    expand_label(p, alg, base_key, b"finished", &[], alg.output_len())
}

/// verify_data = HMAC(finished_key, Transcript-Hash(Handshake Context, Certificate*, CertificateVerify*)).
pub fn finished_verify_data(
    p: &dyn CryptoProvider,
    alg: HashAlg,
    base_key: &[u8],
    transcript_hash: &[u8],
) -> Result<Digest, CryptoError> {
    let fk = finished_key(p, alg, base_key)?;
    Ok(p.hmac(alg, fk.as_bytes(), &[transcript_hash]))
}

/// application_traffic_secret_N+1 = HKDF-Expand-Label(application_traffic_secret_N, "traffic upd", "", Hash.length)
/// (RFC 8446 §7.2).
pub fn next_traffic_secret(p: &dyn CryptoProvider, alg: HashAlg, secret: &[u8]) -> Result<Digest, CryptoError> {
    expand_label(p, alg, secret, b"traffic upd", &[], alg.output_len())
}

/// The resumption PSK for a ticket: HKDF-Expand-Label(resumption_master_secret, "resumption", ticket_nonce,
/// Hash.length) (RFC 8446 §4.6.1).
pub fn resumption_psk(
    p: &dyn CryptoProvider,
    alg: HashAlg,
    resumption_master_secret: &[u8],
    nonce: &[u8],
) -> Result<Digest, CryptoError> {
    expand_label(p, alg, resumption_master_secret, b"resumption", nonce, alg.output_len())
}

/// TLS-Exporter(label, context_value, key_length) (RFC 8446 §7.5).
pub fn export(
    p: &dyn CryptoProvider,
    alg: HashAlg,
    exporter_master_secret: &[u8],
    label: &[u8],
    context: &[u8],
    out: &mut [u8],
) -> Result<(), CryptoError> {
    let eh = p.hash(alg, &[]);
    let s = derive_secret(p, alg, exporter_master_secret, label, eh.as_bytes())?;
    let ch = p.hash(alg, &[context]);
    p.hkdf_expand_label(alg, s.as_bytes(), b"exporter", ch.as_bytes(), out)
}
