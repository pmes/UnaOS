//! The TLS 1.3 record layer (RFC 8446 §5).
//!
//! * TLSPlaintext: type(1) || legacy_record_version(2) || length(2) || fragment, length ≤ 2^14.
//! * TLSCiphertext: opaque_type = application_data(23), legacy_record_version = 0x0303 (the 1.2-looking outer
//!   header), length ≤ 2^14 + 256, encrypted_record = AEAD(TLSInnerPlaintext).
//! * TLSInnerPlaintext: content || ContentType || zeros[padding].
//! * Per-record nonce: the 64-bit sequence number, big-endian, left-padded to iv_length, XORed with the static IV.
//! * AAD: the five header bytes of the TLSCiphertext.

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
}

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
        RecordProtection { aead, key, iv, seq: 0 }
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
    let ty = header[0];
    let len = u16::from_be_bytes([header[3], header[4]]) as usize;
    if ContentType::from_u8(ty).is_none() {
        return Err(TlsError::Protocol(AlertDescription::UnexpectedMessage, "unknown record type"));
    }
    if header[1] != 0x03 {
        return Err(TlsError::Protocol(AlertDescription::ProtocolVersion, "bad record version"));
    }
    if len > MAX_CIPHERTEXT {
        return Err(TlsError::Protocol(AlertDescription::RecordOverflow, "record too long"));
    }
    if len == 0 && ty != ContentType::ApplicationData as u8 {
        return Err(TlsError::Protocol(AlertDescription::DecodeError, "zero-length record"));
    }
    Ok(len)
}
