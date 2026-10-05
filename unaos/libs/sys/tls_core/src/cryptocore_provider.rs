// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `CryptoCoreProvider` — tls_core's `CryptoProvider` over CRYPTOCORE (`crypto_core`, SR27).
//!
//! VEINTLS (SR36) STUB. This file is CRYPTOCORE's fold file, `unaos/libs/sys/crypto_core/adapters/
//! tls_core_provider.rs`, which at the fold is copied here VERBATIM (replacing this stub; the features and the
//! `pub mod` line it asks for are already in place). It is not on this branch yet, so the product build stops
//! here with the exact list of what is missing instead of linking a provider that cannot verify the Web PKI.

compile_error!(
    "tls_core feature `cryptocore`: CryptoCoreProvider is not on this branch yet. Missing from crypto_core \
     (CRYPTOCORE, LEDGER SR27): (1) the adapter itself (crypto_core/adapters/tls_core_provider.rs, copied to \
     tls_core/src/cryptocore_provider.rs at the fold) and the `crypto_core::drbg` module it builds on \
     (ChaChaDrbg, the Entropy trait, GetrandomEntropy for SYS_GETRANDOM); (2) for the Web PKI chains Lumen \
     meets (api.anthropic.com and the RSA roots of the Mozilla bundle): RSASSA-PSS verify and RSASSA-PKCS1-v1_5 \
     verify (2048-4096-bit), and ECDSA P-384 verify. Host tests run tls_core on its `test-provider` until then."
);
