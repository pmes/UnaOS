// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// HASH — self-contained, no_std checksum + digest primitives.
//
// Two well-known algorithms, implemented here (not pulled from a crate, and deliberately NOT reaching
// into the aarch64-private `sha256` in arch/aarch64/syscall.rs — every consumer here is arch-neutral,
// so this module carries its own copies):
//   * CRC-32/ISO-HDLC (poly 0xEDB88320, reflected, init/xorout 0xFFFFFFFF) — the exact variant the
//     UEFI GPT spec mandates for the header + partition-entry-array CRCs, and the same variant gzip
//     stamps in its 8-byte trailer.
//   * SHA-256 (FIPS 180-4) — the content fingerprint for the copy-and-verify primitive. Since CRYPTOCORE
//     (SR27) it is `crypto_core::sha2` re-exported below, with HMAC-SHA256 and PBKDF2 beside it.
// Both are verified against known-answer tests at witness time (see `install/mod.rs`).
//
// SELFHOST-2 moved this file from `install/hash.rs` to the crate root so the source-verify walk can
// reuse it without dragging the whole installer engine in behind `installdemo`. `install` re-exports
// it (`pub use crate::hash;`), so every `crate::install::hash::…` call site is unchanged.

/// CRC-32/ISO-HDLC lookup table, computed at compile time (poly 0xEDB88320, reflected).
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            let mask = (crc & 1).wrapping_neg(); // 0xFFFFFFFF if LSB set, else 0
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// A streaming CRC-32/ISO-HDLC. The gzip trailer check in `selfhost::inflate` runs this over tens of
/// megabytes of decompressed stream that is never buffered whole, so the digest has to be incremental
/// the same way `Sha256` is.
pub struct Crc32 {
    crc: u32,
}

impl Crc32 {
    pub fn new() -> Self {
        Self { crc: 0xFFFF_FFFF }
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut crc = self.crc;
        for &b in data {
            crc = (crc >> 8) ^ CRC32_TABLE[((crc ^ b as u32) & 0xFF) as usize];
        }
        self.crc = crc;
    }

    pub fn finish(&self) -> u32 {
        !self.crc
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// CRC-32/ISO-HDLC over `data`. Matches the host `crc32fast::hash` the builder's `vm_image.rs` GPT
/// writer uses, so an image written by either tool self-verifies against the other.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.finish()
}

// --- SHA-256 + HMAC-SHA256 + PBKDF2-HMAC-SHA256: MOVED to `crypto_core` (CRYPTOCORE, LEDGER SR27) ---
//
// The from-scratch FIPS 180-4 SHA-256, the RFC 2104 HMAC and the RFC 8018 PBKDF2 that SELFHOST-2 and
// SECLOGIN M1 wrote here now live in `unaos/libs/sys/crypto_core` (the same code, generalised to
// SHA-224/384/512 and every dkLen, proven by the NIST CAVP / RFC 4231 / RFC 7914 / Wycheproof vectors on
// the host). This module re-exports them under the old names, so every `crate::hash::…` call site
// (install, selfhost, users, rand, the tegra firmware paths) is unchanged. CRC-32 above stays here: a
// checksum, not cryptography.
pub use crypto_core::hmac::HmacSha256;
pub use crypto_core::sha2::{sha256, Sha256};

/// The longest salt `pbkdf2_hmac_sha256` accepts; a longer one is truncated (the caller's bound is
/// 16, so this is a guard, not a limit anyone reaches).
pub const PBKDF2_SALT_MAX: usize = 64;

/// PBKDF2-HMAC-SHA256, first output block only (`dkLen` = 32) — now `crypto_core::pbkdf2`. The salt is
/// truncated to `PBKDF2_SALT_MAX` and `iters` of 0 is treated as 1, exactly as before, so a credential row
/// written by the old code verifies byte-for-byte. Cost: `2 * iters` SHA-256 compressions.
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iters: u32, out: &mut [u8; 32]) {
    let n = core::cmp::min(salt.len(), PBKDF2_SALT_MAX);
    crypto_core::pbkdf2::pbkdf2_hmac_sha256(password, &salt[..n], iters, out);
}

/// Known answers for the fixture: PBKDF2-HMAC-SHA256("password", "salt", c) for c = 1, 2, 4096 —
/// the RFC 6070 inputs with SHA-256 as the PRF, as printed by every reference implementation.
pub const PBKDF2_KAT: [(u32, [u8; 32]); 3] = [
    (1, [
        0x12, 0x0f, 0xb6, 0xcf, 0xfc, 0xf8, 0xb3, 0x2c, 0x43, 0xe7, 0x22, 0x52, 0x56, 0xc4, 0xf8, 0x37,
        0xa8, 0x65, 0x48, 0xc9, 0x2c, 0xcc, 0x35, 0x48, 0x08, 0x05, 0x98, 0x7c, 0xb7, 0x0b, 0xe1, 0x7b,
    ]),
    (2, [
        0xae, 0x4d, 0x0c, 0x95, 0xaf, 0x6b, 0x46, 0xd3, 0x2d, 0x0a, 0xdf, 0xf9, 0x28, 0xf0, 0x6d, 0xd0,
        0x2a, 0x30, 0x3f, 0x8e, 0xf3, 0xc2, 0x51, 0xdf, 0xd6, 0xe2, 0xd8, 0x5a, 0x95, 0x47, 0x4c, 0x43,
    ]),
    (4096, [
        0xc5, 0xe4, 0x78, 0xd5, 0x92, 0x88, 0xc8, 0x41, 0xaa, 0x53, 0x0d, 0xb6, 0x84, 0x5c, 0x4c, 0x8d,
        0x96, 0x28, 0x93, 0xa0, 0x01, 0xce, 0x4e, 0x11, 0xa4, 0x96, 0x38, 0x73, 0xaa, 0x98, 0x13, 0x4a,
    ]),
];

/// Run the three known answers; `true` when every one matches. Cheap (4099 iterations total).
pub fn pbkdf2_kat_ok() -> bool {
    let mut ok = true;
    for (c, want) in PBKDF2_KAT.iter() {
        let mut got = [0u8; 32];
        pbkdf2_hmac_sha256(b"password", b"salt", *c, &mut got);
        ok &= got == *want;
    }
    ok
}
