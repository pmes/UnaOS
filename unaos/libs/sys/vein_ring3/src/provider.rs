// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The product crypto provider for ring 3 (VEINTLS, SR36): CRYPTOCORE's `CryptoCoreProvider` (feature
//! `cryptocore` → `tls_core/cryptocore`), seeded from SYS_GETRANDOM through crypto_core's DRBG. Until
//! CRYPTOCORE's fold file lands on this branch, enabling the feature stops the build at tls_core's
//! `cryptocore_provider.rs` with the list of what is missing — never a silent fallback to anything weaker.

use alloc::boxed::Box;
use tls_core::cryptocore_provider::CryptoCoreProvider;
use tls_core::CryptoProvider;

/// The provider a ring-3 program hands [`crate::tls::TlsContext`]; `Err` = no entropy (no TLS at all).
pub fn product() -> Result<Box<dyn CryptoProvider>, &'static str> {
    let src = crypto_core::drbg::GetrandomEntropy::new(|b: &mut [u8]| match crate::sys::getrandom(b) {
        Ok(()) => b.len() as isize,
        Err(e) => e as isize,
    });
    CryptoCoreProvider::with_entropy(Box::new(src)).map(|p| Box::new(p) as Box<dyn CryptoProvider>).map_err(|_| "no-entropy")
}
