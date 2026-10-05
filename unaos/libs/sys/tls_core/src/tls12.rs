//! TLS 1.2 key derivation (RFC 5246 §5, §6.3, §7.4.9; RFC 7627; RFC 5705) over the provider's PRF.
//!
//! * master_secret = PRF(pre_master_secret, "extended master secret", session_hash)[0..47] — RFC 7627 §4; the
//!   legacy "master secret" over the randoms is NEVER computed (this client requires the extension).
//! * key_block = PRF(master_secret, "key expansion", server_random + client_random): client_write_key,
//!   server_write_key, client_write_IV, server_write_IV (AEAD suites carry no MAC keys, RFC 5246 §6.3).
//! * verify_data = PRF(master_secret, "client finished" | "server finished", Hash(handshake_messages))[0..11].
//! * exporter (RFC 5705 §4) = PRF(master_secret, label, client_random + server_random [+ len16 + context]).

use alloc::vec::Vec;

use crate::crypto::{AeadAlg, CryptoError, CryptoProvider, HashAlg};

/// A 48-byte master secret, zeroised on drop.
#[derive(Clone)]
pub struct MasterSecret(pub [u8; 48]);

impl Drop for MasterSecret {
    fn drop(&mut self) {
        self.0 = [0; 48];
        core::hint::black_box(&self.0);
    }
}

/// RFC 7627 §4.
pub fn extended_master_secret(p: &dyn CryptoProvider, alg: HashAlg, pre_master: &[u8], session_hash: &[u8]) -> Result<MasterSecret, CryptoError> {
    let mut m = [0u8; 48];
    p.tls12_prf(alg, pre_master, b"extended master secret", &[session_hash], &mut m)?;
    Ok(MasterSecret(m))
}

/// The four AEAD traffic values of one connection (RFC 5246 §6.3).
pub struct KeyBlock {
    pub client_key: Vec<u8>,
    pub server_key: Vec<u8>,
    pub client_iv: Vec<u8>,
    pub server_iv: Vec<u8>,
}

impl Drop for KeyBlock {
    fn drop(&mut self) {
        for v in [&mut self.client_key, &mut self.server_key, &mut self.client_iv, &mut self.server_iv] {
            for b in v.iter_mut() {
                *b = 0;
            }
            core::hint::black_box(&v);
        }
    }
}

/// fixed_iv_length: 4 for AES-GCM (RFC 5288 §3), 12 for ChaCha20-Poly1305 (RFC 7905 §2).
pub const fn fixed_iv_len(aead: AeadAlg) -> usize {
    match aead {
        AeadAlg::ChaCha20Poly1305 => 12,
        _ => 4,
    }
}

pub fn key_block(
    p: &dyn CryptoProvider,
    alg: HashAlg,
    aead: AeadAlg,
    master: &MasterSecret,
    client_random: &[u8; 32],
    server_random: &[u8; 32],
) -> Result<KeyBlock, CryptoError> {
    let k = aead.key_len();
    let iv = fixed_iv_len(aead);
    let mut kb = alloc::vec![0u8; 2 * k + 2 * iv];
    p.tls12_prf(alg, &master.0, b"key expansion", &[server_random, client_random], &mut kb)?;
    let out = KeyBlock {
        client_key: kb[..k].to_vec(),
        server_key: kb[k..2 * k].to_vec(),
        client_iv: kb[2 * k..2 * k + iv].to_vec(),
        server_iv: kb[2 * k + iv..].to_vec(),
    };
    for b in kb.iter_mut() {
        *b = 0;
    }
    Ok(out)
}

/// RFC 5246 §7.4.9 (verify_data_length 12 for every suite here).
pub fn finished(p: &dyn CryptoProvider, alg: HashAlg, master: &MasterSecret, label: &[u8], transcript_hash: &[u8]) -> Result<[u8; 12], CryptoError> {
    let mut v = [0u8; 12];
    p.tls12_prf(alg, &master.0, label, &[transcript_hash], &mut v)?;
    Ok(v)
}

/// RFC 5705 §4 keying-material exporter.
pub fn export(
    p: &dyn CryptoProvider,
    alg: HashAlg,
    master: &MasterSecret,
    label: &[u8],
    client_random: &[u8; 32],
    server_random: &[u8; 32],
    context: Option<&[u8]>,
    out: &mut [u8],
) -> Result<(), CryptoError> {
    match context {
        None => p.tls12_prf(alg, &master.0, label, &[client_random, server_random], out),
        Some(c) => {
            if c.len() > 0xffff {
                return Err(CryptoError::Internal("exporter context too long"));
            }
            let l = (c.len() as u16).to_be_bytes();
            p.tls12_prf(alg, &master.0, label, &[client_random, server_random, &l, c], out)
        }
    }
}
