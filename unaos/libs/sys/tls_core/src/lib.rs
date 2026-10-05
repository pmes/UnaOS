//! TLSCORE — UnaOS's own TLS 1.3 (RFC 8446) and TLS 1.2 (RFC 5246, ECDHE + AEAD) client and X.509 path
//! validation (RFC 5280).
//!
//! `no_std` + `alloc`, no third-party code: every byte of protocol is parsed and produced here. Cryptography is
//! reached only through [`crypto::CryptoProvider`]; CRYPTOCORE implements it for the product, and the
//! `test-provider` feature supplies a test operand over RustCrypto so the state machine is proven today.
//!
//! * [`record`] — §5 record layer: TLSPlaintext/TLSCiphertext, inner plaintext padding, per-record nonces.
//! * [`key_schedule`] — §7.1 key schedule, §7.3 traffic keys, §4.4.4 Finished, §7.2 KeyUpdate, §7.5 exporter.
//! * [`transcript`] — §4.4.1 transcript hash (with the HelloRetryRequest message_hash rewrite).
//! * [`msgs`] — §4 handshake messages.
//! * [`tls12`] — TLS 1.2 (RFC 5246 + RFC 7627) key derivation over the provider's PRF.
//! * [`client`] — the client state machine over a [`client::Transport`]: TLS 1.3, and TLS 1.2 (ECDHE + AEAD only)
//!   when the configuration offers a 1.2 suite.
//! * [`resumption`] — TLS 1.3 tickets and the caller-owned [`resumption::TicketStore`] seam (0-RTT refused).
//! * [`x509`] — RFC 5280 DER/certificates/path validation, RFC 6125 names, PEM trust store.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod client;
pub mod codec;
pub mod crypto;
pub mod error;
pub mod key_schedule;
pub mod msgs;
pub mod record;
pub mod resumption;
pub mod tls12;
pub mod transcript;
pub mod x509;

#[cfg(feature = "test-provider")]
pub mod test_provider;

/// The product provider over CRYPTOCORE (feature `cryptocore`). See `src/cryptocore_provider.rs`.
#[cfg(feature = "cryptocore")]
pub mod cryptocore_provider;

pub use client::{Client, ClientConfig, Negotiated, ServerCertVerifier, Transport};
pub use crypto::CryptoProvider;
pub use error::{AlertDescription, CertError, TlsError};
