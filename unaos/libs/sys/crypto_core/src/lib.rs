// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `crypto_core` — the cryptographic primitives UnaOS owns (CRYPTOCORE, LEDGER SR27).
//!
//! CHARTER: Holocron — shared-core. Holocron (CODEX §2, Secrets: keyring, SSH agent, wallet) owns keys
//! and their policy; this crate is the arithmetic both rings link (the kernel's credential store and
//! entropy pool, ring 3's TLS and Holocron itself). It holds no keys of its own and no bus surface.
//!
//! Peter (2026-10-04): "security executors so that when we need some cryptographic related need or
//! whathaveyou it is already there." Every primitive here is written from its specification, in
//! `no_std`, with no third-party crate under it, and is proven by the official known-answer vectors
//! (`tests/kat.rs`, `tools/crypto-check`).
//!
//! | module | primitive | specification |
//! |---|---|---|
//! | [`sha2`] | SHA-224/256/384/512 | FIPS 180-4 |
//! | [`hmac`] | HMAC over any [`Digest`] | RFC 2104 / FIPS 198-1 |
//! | [`hkdf`] | HKDF extract/expand | RFC 5869 |
//! | [`tls12_prf`] | the TLS 1.2 PRF (P_SHA256 / P_SHA384) | RFC 5246 §5 |
//! | [`sha1`] | SHA-1 — OCSP CertID matching only, never a signature | FIPS 180-4 §6.1 |
//! | [`pbkdf2`] | PBKDF2-HMAC | RFC 8018 §5.2 |
//! | [`chacha20`], [`poly1305`], [`chacha20poly1305`] | ChaCha20, Poly1305, the AEAD | RFC 8439 |
//! | [`aes`], [`gcm`] | AES-128/256 (bitsliced), GCM/GMAC | FIPS 197, SP 800-38D |
//! | [`x25519`] | X25519 | RFC 7748 |
//! | [`ed25519`] | Ed25519 | RFC 8032 |
//! | [`p256`] | P-256 ECDH + ECDSA | SP 800-186, FIPS 186-5, RFC 6979, SEC 1 |
//! | [`p384`] | P-384 ECDH + ECDSA | SP 800-186, FIPS 186-5, RFC 6979, SEC 1 |
//! | [`rsa`] | RSASSA-PSS + RSASSA-PKCS1-v1_5 VERIFY | RFC 8017 (PKCS #1 v2.2) |
//! | [`blake2b`], [`argon2`] | BLAKE2b, Argon2id/i/d | RFC 7693, RFC 9106 |
//! | [`sha3`] | SHA3-224/256/384/512, SHAKE128/256 | FIPS 202 |
//! | [`mlkem`] | ML-KEM-512/768/1024 (KeyGen, Encaps, Decaps; K-PKE, NTT) | FIPS 203 |
//! | [`drbg`] | ChaCha20 fast-key-erasure DRBG over a [`drbg::Entropy`] | this crate (documented) |
//! | [`ct`] | `ct_eq`, `ct_select`, [`ct::Zeroize`] | — |
//!
//! # Constant-time discipline
//!
//! Every public function says, in its doc comment, whether it runs in time independent of its SECRET
//! inputs and why. The rules used throughout: no branch and no memory index depends on a secret; a
//! secret-dependent choice is a mask (`ct::Choice`), a secret table lookup scans the whole table; the
//! only multiplications are on `u64`/`u128` operands, which are constant-time on x86_64 and AArch64
//! (the two UnaOS architectures — NOT on some 32-bit cores such as Cortex-M3, where `umull` exits early).
//! Lengths of messages, of keys and of associated data are public everywhere.
//!
//! Key-bearing types zeroize on drop ([`ct::Zeroize`]).
#![no_std]
#![deny(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod ct;
pub mod sha2;
pub mod hmac;
pub mod hkdf;
pub mod tls12_prf;
pub mod sha1;
pub mod pbkdf2;
pub mod chacha20;
pub mod poly1305;
pub mod chacha20poly1305;
pub mod aes;
pub mod gcm;
mod field25519;
pub mod x25519;
pub mod ed25519;
mod bigint;
pub mod p256;
mod bignum;
pub mod p384;
pub mod rsa;
pub mod blake2b;
pub mod argon2;
pub mod drbg;
pub mod sha3;
pub mod mlkem;

pub use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};

/// The one error type: what failed, never why in secret-dependent detail (an AEAD that fails to
/// authenticate says only `Error::Auth`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Authentication failed (AEAD tag, MAC, signature).
    Auth,
    /// A length is outside what the specification allows.
    Length,
    /// An encoding (point, scalar, signature, key) is malformed or non-canonical.
    Encoding,
    /// A parameter is outside what the specification allows (Argon2 costs, HKDF output length…).
    Param,
    /// The result would be a forbidden value (an all-zero X25519 secret, the point at infinity).
    Degenerate,
    /// The entropy source failed.
    Entropy,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Error::Auth => "authentication failed",
            Error::Length => "length out of range",
            Error::Encoding => "malformed encoding",
            Error::Param => "parameter out of range",
            Error::Degenerate => "degenerate result",
            Error::Entropy => "entropy source failed",
        })
    }
}
