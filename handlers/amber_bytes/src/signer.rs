// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The SIGNED PLAN rule. `PlanLayout` shows a plan and its signature over the plan's canonical bytes
//! ([`crate::ops::Planned::canonical`]); `Apply` re-derives the plan from the layout and the medium
//! in hand and refuses unless the presented signature verifies over THOSE bytes. So nothing is
//! written that was not shown, byte for byte — a different disk size, a changed layout or a
//! different seed is a different signature.
//!
//! [`Sha256Digest`] is the first scheme: the "signature" is the SHA-256 of the canonical bytes — it
//! proves the caller saw this plan, not who the caller is. A keyed scheme (HMAC-SHA-256 under a
//! Holocron key) is a second `impl Signer`, owed; the bus carries the scheme name so the two never
//! confuse.

use sha2::{Digest, Sha256};

/// A plan-signing scheme.
pub trait Signer {
    /// The scheme's name on the wire (`"sha256"`).
    fn scheme(&self) -> &'static str;
    /// The signature of `canonical`, as lowercase hex.
    fn sign(&self, canonical: &[u8]) -> String;
    /// Whether `sig` is this scheme's signature of `canonical` (constant-time compare).
    fn verify(&self, canonical: &[u8], sig: &str) -> bool {
        ct_eq(self.sign(canonical).as_bytes(), sig.trim().to_ascii_lowercase().as_bytes())
    }
}

/// The unkeyed digest scheme (see the module doc).
#[derive(Clone, Copy, Debug, Default)]
pub struct Sha256Digest;

impl Signer for Sha256Digest {
    fn scheme(&self) -> &'static str {
        "sha256"
    }
    fn sign(&self, canonical: &[u8]) -> String {
        hex(&Sha256::digest(canonical))
    }
}

/// Lowercase hex.
pub fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIPS 180-4 / NIST CAVS short-message vectors through the scheme.
    #[test]
    fn sha256_known_answers() {
        let s = Sha256Digest;
        assert_eq!(s.sign(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(s.sign(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(s.verify(b"abc", "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD\n"));
        assert!(!s.verify(b"abd", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        assert!(!s.verify(b"abc", "ba78"));
    }
}
