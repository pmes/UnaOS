//! The handshake transcript (RFC 8446 §4.4.1).
//!
//! The hash function is unknown until the ServerHello picks a cipher suite, so the transcript keeps the handshake
//! messages themselves and hashes on demand (a handful of times per handshake). On HelloRetryRequest the first
//! ClientHello is replaced by the synthetic `message_hash` message.

use alloc::vec::Vec;

use crate::crypto::{CryptoProvider, Digest, HashAlg};
use crate::msgs::hs;

#[derive(Default, Clone)]
pub struct Transcript {
    buf: Vec<u8>,
}

impl Transcript {
    pub fn new() -> Self {
        Transcript { buf: Vec::new() }
    }
    /// Appends a complete handshake message (4-byte header included).
    pub fn add(&mut self, msg: &[u8]) {
        self.buf.extend_from_slice(msg);
    }
    pub fn hash(&self, p: &dyn CryptoProvider, alg: HashAlg) -> Digest {
        p.hash(alg, &[&self.buf])
    }
    /// RFC 8446 §4.4.1: ClientHello1 → message_hash(254) || 00 00 Hash.length || Hash(ClientHello1).
    pub fn replace_with_message_hash(&mut self, p: &dyn CryptoProvider, alg: HashAlg) {
        let h = self.hash(p, alg);
        let mut b = Vec::with_capacity(4 + h.len());
        b.push(hs::MESSAGE_HASH);
        b.extend_from_slice(&[0, 0, h.len() as u8]);
        b.extend_from_slice(h.as_bytes());
        self.buf = b;
    }
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }
}
