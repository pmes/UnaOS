//! The TLS 1.3 record layer (RFC 8446 §5).
//!
//! * TLSPlaintext: type(1) || legacy_record_version(2) || length(2) || fragment, length ≤ 2^14.
//! * TLSCiphertext: opaque_type = application_data(23), legacy_record_version = 0x0303 (the 1.2-looking outer
//!   header), length ≤ 2^14 + 256, encrypted_record = AEAD(TLSInnerPlaintext).
//! * TLSInnerPlaintext: content || ContentType || zeros[padding].
//! * Per-record nonce: the 64-bit sequence number, big-endian, left-padded to iv_length, XORed with the static IV.
//! * AAD: the five header bytes of the TLSCiphertext.
//!
//! TLS 1.2 (RFC 5246 §6.2.3.3) is the same struct in `tls12` mode: the record's real content type is the outer
//! type (no inner type byte, no padding), AAD = seq_num(8) || type || version || plaintext length, and the nonce
//! is per AEAD — AES-GCM (RFC 5288 §3): the 4-byte implicit salt from the key block || an 8-byte explicit nonce
//! carried in front of the ciphertext (we send the sequence number, which never repeats under one key);
//! ChaCha20-Poly1305 (RFC 7905 §2): the 12-byte IV XOR the padded sequence number, exactly as TLS 1.3.

use alloc::vec::Vec;

use crate::crypto::{AeadAlg, CryptoProvider};
use crate::error::{AlertDescription, TlsError};
use crate::msgs::ContentType;

pub const MAX_FRAGMENT: usize = 1 << 14;
pub const MAX_CIPHERTEXT: usize = MAX_FRAGMENT + 256;
pub const HEADER_LEN: usize = 5;

/// One direction's traffic protection: key, static IV, sequence number.
pub struct RecordProtection {
    pub aead: AeadAlg,
    key: Vec<u8>,
    iv: [u8; 12],
    seq: u64,
    tls12: bool,
}

/// RFC 5288 §3: the explicit nonce in front of every TLS 1.2 AES-GCM record.
const GCM_EXPLICIT: usize = 8;

impl Drop for RecordProtection {
    fn drop(&mut self) {
        for b in self.key.iter_mut() {
            *b = 0;
        }
        self.iv = [0; 12];
        core::hint::black_box(&self.key);
    }
}

impl RecordProtection {
    pub fn new(aead: AeadAlg, key: Vec<u8>, iv: [u8; 12]) -> Self {
        RecordProtection { aead, key, iv, seq: 0, tls12: false }
    }

    /// TLS 1.2 protection. `iv` is the key block's write IV: 4 bytes (AES-GCM salt) or 12 (ChaCha20-Poly1305).
    pub fn new12(aead: AeadAlg, key: Vec<u8>, iv: &[u8]) -> Result<Self, TlsError> {
        let want = if aead == AeadAlg::ChaCha20Poly1305 { 12 } else { 4 };
        if iv.len() != want {
            return Err(TlsError::State("TLS 1.2 IV length"));
        }
        let mut v = [0u8; 12];
        v[..want].copy_from_slice(iv);
        Ok(RecordProtection { aead, key, iv: v, seq: 0, tls12: true })
    }

    pub fn is_tls12(&self) -> bool {
        self.tls12
    }

    fn nonce12(&self, explicit: &[u8]) -> [u8; 12] {
        if self.aead == AeadAlg::ChaCha20Poly1305 {
            return self.nonce();
        }
        let mut n = self.iv;
        n[4..].copy_from_slice(explicit);
        n
    }

    fn aad12(&self, ty: u8, len: usize) -> [u8; 13] {
        let mut a = [0u8; 13];
        a[..8].copy_from_slice(&self.seq.to_be_bytes());
        a[8] = ty;
        a[9] = 0x03;
        a[10] = 0x03;
        a[11] = (len >> 8) as u8;
        a[12] = len as u8;
        a
    }

    fn seal12(&mut self, p: &dyn CryptoProvider, ty: ContentType, content: &[u8]) -> Result<Vec<u8>, TlsError> {
        if content.len() > MAX_FRAGMENT {
            return Err(TlsError::State("record too large"));
        }
        let explicit = self.seq.to_be_bytes();
        let gcm = self.aead != AeadAlg::ChaCha20Poly1305;
        let mut buf = content.to_vec();
        let aad = self.aad12(ty as u8, content.len());
        p.aead_seal(self.aead, &self.key, &self.nonce12(&explicit), &aad, &mut buf)?;
        self.bump()?;
        let body = if gcm { GCM_EXPLICIT } else { 0 } + buf.len();
        let mut rec = Vec::with_capacity(HEADER_LEN + body);
        rec.extend_from_slice(&[ty as u8, 0x03, 0x03, (body >> 8) as u8, body as u8]);
        if gcm {
            rec.extend_from_slice(&explicit);
        }
        rec.extend_from_slice(&buf);
        Ok(rec)
    }

    fn open12(&mut self, p: &dyn CryptoProvider, header: &[u8; 5], payload: &[u8]) -> Result<(ContentType, Vec<u8>), TlsError> {
        // RFC 5246 §6.2.3: TLSCiphertext.length ≤ 2^14 + 2048.
        if payload.len() > MAX_CIPHERTEXT12 {
            return Err(TlsError::Protocol(AlertDescription::RecordOverflow, "ciphertext too long"));
        }
        let ty = ContentType::from_u8(header[0])
            .ok_or(TlsError::Protocol(AlertDescription::UnexpectedMessage, "unknown record type"))?;
        let gcm = self.aead != AeadAlg::ChaCha20Poly1305;
        let ex = if gcm { GCM_EXPLICIT } else { 0 };
        if payload.len() < ex + AeadAlg::TAG_LEN {
            return Err(TlsError::BadRecordMac);
        }
        let (explicit, ct) = payload.split_at(ex);
        let plain_len = ct.len() - AeadAlg::TAG_LEN;
        if plain_len > MAX_FRAGMENT {
            return Err(TlsError::Protocol(AlertDescription::RecordOverflow, "plaintext too long"));
        }
        let aad = self.aad12(header[0], plain_len);
        let nonce = self.nonce12(explicit);
        let mut buf = ct.to_vec();
        p.aead_open(self.aead, &self.key, &nonce, &aad, &mut buf).map_err(|_| TlsError::BadRecordMac)?;
        self.bump()?;
        Ok((ty, buf))
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// RFC 8446 §5.3.
    pub fn nonce(&self) -> [u8; 12] {
        let mut n = self.iv;
        let s = self.seq.to_be_bytes();
        for i in 0..8 {
            n[4 + i] ^= s[i];
        }
        n
    }

    fn bump(&mut self) -> Result<(), TlsError> {
        // §5.3: a sequence number must not wrap; the connection must rekey or close first.
        self.seq = self.seq.checked_add(1).ok_or(TlsError::State("sequence number exhausted"))?;
        Ok(())
    }

    /// Protects one record: `content` (≤ 2^14 bytes) of `ty`, plus `padding` zero bytes. Returns the full
    /// TLSCiphertext record (header included).
    pub fn seal(
        &mut self,
        p: &dyn CryptoProvider,
        ty: ContentType,
        content: &[u8],
        padding: usize,
    ) -> Result<Vec<u8>, TlsError> {
        if self.tls12 {
            return self.seal12(p, ty, content);
        }
        if content.len() + 1 + padding > MAX_FRAGMENT + 1 {
            return Err(TlsError::State("record too large"));
        }
        let mut inner = Vec::with_capacity(content.len() + 1 + padding + AeadAlg::TAG_LEN);
        inner.extend_from_slice(content);
        inner.push(ty as u8);
        inner.resize(inner.len() + padding, 0);
        let ct_len = inner.len() + AeadAlg::TAG_LEN;
        let header = [ContentType::ApplicationData as u8, 0x03, 0x03, (ct_len >> 8) as u8, ct_len as u8];
        p.aead_seal(self.aead, &self.key, &self.nonce(), &header, &mut inner)?;
        self.bump()?;
        let mut rec = Vec::with_capacity(HEADER_LEN + inner.len());
        rec.extend_from_slice(&header);
        rec.extend_from_slice(&inner);
        Ok(rec)
    }

    /// Removes protection from one TLSCiphertext (`header` = its 5 bytes, `payload` = encrypted_record). Returns the
    /// real content type and the content with padding stripped.
    pub fn open(
        &mut self,
        p: &dyn CryptoProvider,
        header: &[u8; 5],
        payload: &[u8],
    ) -> Result<(ContentType, Vec<u8>), TlsError> {
        if self.tls12 {
            return self.open12(p, header, payload);
        }
        if payload.len() > MAX_CIPHERTEXT {
            return Err(TlsError::Protocol(AlertDescription::RecordOverflow, "ciphertext too long"));
        }
        if payload.len() < AeadAlg::TAG_LEN + 1 {
            return Err(TlsError::BadRecordMac);
        }
        let mut buf = payload.to_vec();
        p.aead_open(self.aead, &self.key, &self.nonce(), header, &mut buf).map_err(|_| TlsError::BadRecordMac)?;
        self.bump()?;
        // Strip zero padding; the last non-zero byte is the real content type (§5.4).
        let end = buf.iter().rposition(|&b| b != 0).ok_or(TlsError::Protocol(
            AlertDescription::UnexpectedMessage,
            "inner plaintext is all zeros",
        ))?;
        let ty = ContentType::from_u8(buf[end])
            .ok_or(TlsError::Protocol(AlertDescription::UnexpectedMessage, "unknown inner content type"))?;
        buf.truncate(end);
        if buf.len() > MAX_FRAGMENT {
            return Err(TlsError::Protocol(AlertDescription::RecordOverflow, "plaintext too long"));
        }
        Ok((ty, buf))
    }
}

/// Encodes an unprotected TLSPlaintext record. `legacy_version` is 0x0301 for an initial ClientHello (§5.1) and
/// 0x0303 otherwise.
pub fn plaintext_record(ty: ContentType, legacy_version: u16, fragment: &[u8]) -> Vec<u8> {
    let mut r = Vec::with_capacity(HEADER_LEN + fragment.len());
    r.push(ty as u8);
    r.extend_from_slice(&legacy_version.to_be_bytes());
    r.extend_from_slice(&(fragment.len() as u16).to_be_bytes());
    r.extend_from_slice(fragment);
    r
}

/// Splits `data` into ≤ 2^14-byte fragments (§5.1: handshake messages may span records; application data may be
/// fragmented arbitrarily).
pub fn fragments(data: &[u8], max: usize) -> impl Iterator<Item = &[u8]> {
    let max = max.clamp(1, MAX_FRAGMENT);
    data.chunks(max)
}

/// A received record, header parsed.
#[derive(Debug)]
pub struct RawRecord {
    pub header: [u8; 5],
    pub ty: u8,
    pub payload: Vec<u8>,
}

/// Checks a received record header (§5.1, §5.2).
pub fn check_header(header: &[u8; 5]) -> Result<usize, TlsError> {
    check_header_max(header, MAX_CIPHERTEXT)
}

/// TLS 1.2's record bound (RFC 5246 §6.2.3: TLSCiphertext.length ≤ 2^14 + 2048).
pub const MAX_CIPHERTEXT12: usize = MAX_FRAGMENT + 2048;

/// [`check_header`] with the version's own length bound (`MAX_CIPHERTEXT` for 1.3, `MAX_CIPHERTEXT12` for 1.2).
pub fn check_header_max(header: &[u8; 5], max: usize) -> Result<usize, TlsError> {
    let ty = header[0];
    let len = u16::from_be_bytes([header[3], header[4]]) as usize;
    if ContentType::from_u8(ty).is_none() {
        return Err(TlsError::Protocol(AlertDescription::UnexpectedMessage, "unknown record type"));
    }
    if header[1] != 0x03 {
        return Err(TlsError::Protocol(AlertDescription::ProtocolVersion, "bad record version"));
    }
    if len > max {
        return Err(TlsError::Protocol(AlertDescription::RecordOverflow, "record too long"));
    }
    if len == 0 && ty != ContentType::ApplicationData as u8 {
        return Err(TlsError::Protocol(AlertDescription::DecodeError, "zero-length record"));
    }
    Ok(len)
}
