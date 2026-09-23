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
//   * SHA-256 (FIPS 180-4) — the content fingerprint for the copy-and-verify primitive.
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

// --- SHA-256 (FIPS 180-4) ---

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// A streaming SHA-256 digest — the copy-verify primitive feeds it extent-by-extent as it re-reads,
/// so no full-payload buffer is ever held for hashing.
#[derive(Clone)] pub struct Sha256 { // SECLOGIN M1: Clone so an HMAC can keep its two keyed prefix states and fork them per message (same-line fold: no Location below moves)
    h: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0u8; 64],
            buf_len: 0,
            total: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let take = core::cmp::min(64 - self.buf_len, data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total.wrapping_mul(8);
        // Pad: 0x80, then zeros, then the 64-bit big-endian length.
        let mut pad = [0u8; 72];
        pad[0] = 0x80;
        let pad_len = if self.buf_len < 56 { 56 - self.buf_len } else { 120 - self.buf_len };
        self.update(&pad[..pad_len]);
        // The update above advanced `total`; feed the ORIGINAL bit length explicitly.
        let len_bytes = bit_len.to_be_bytes();
        // At this point buf_len == 56; append the 8 length bytes to complete one final block.
        self.buf[self.buf_len..self.buf_len + 8].copy_from_slice(&len_bytes);
        self.buf_len += 8;
        let block = self.buf;
        self.compress(&block);

        let mut out = [0u8; 32];
        for (i, word) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = self.h[0];
        let mut b = self.h[1];
        let mut c = self.h[2];
        let mut d = self.h[3];
        let mut e = self.h[4];
        let mut f = self.h[5];
        let mut g = self.h[6];
        let mut hh = self.h[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
        self.h[5] = self.h[5].wrapping_add(f);
        self.h[6] = self.h[6].wrapping_add(g);
        self.h[7] = self.h[7].wrapping_add(hh);
    }
}

/// One-shot SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize()
}

// --- HMAC-SHA256 (RFC 2104) + PBKDF2-HMAC-SHA256 (RFC 8018 §5.2) — SECLOGIN M1 (PWHARD) ---
//
// The password stretch for `fs/users.rs`. Self-contained on the `Sha256` above, no crate, no alloc:
// the salt is bounded (`fs::users::SALT_LEN` = 16) so the first block's input fits a stack buffer,
// and a single 32-byte output block is all a credential row needs. Known answers (RFC 6070's vectors
// carried over to SHA-256, the values every reference implementation prints) are asserted by the
// `LOGIN-HARD` fixture at boot, not merely in this comment.

/// A keyed HMAC-SHA256 with the two padded-key prefix states computed ONCE. `mac` forks them per
/// message, which is what makes a PBKDF2 iteration exactly two compressions instead of four.
pub struct HmacSha256 {
    inner: Sha256,
    outer: Sha256,
}

impl HmacSha256 {
    pub fn new(key: &[u8]) -> Self {
        let mut k = [0u8; 64];
        if key.len() > 64 {
            k[..32].copy_from_slice(&sha256(key));
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let mut ipad = [0u8; 64];
        let mut opad = [0u8; 64];
        for i in 0..64 {
            ipad[i] = k[i] ^ 0x36;
            opad[i] = k[i] ^ 0x5c;
        }
        let mut inner = Sha256::new();
        inner.update(&ipad);
        let mut outer = Sha256::new();
        outer.update(&opad);
        Self { inner, outer }
    }

    /// HMAC(key, msg).
    pub fn mac(&self, msg: &[u8]) -> [u8; 32] {
        let mut i = self.inner.clone();
        i.update(msg);
        let d = i.finalize();
        let mut o = self.outer.clone();
        o.update(&d);
        o.finalize()
    }
}

/// The longest salt `pbkdf2_hmac_sha256` accepts; a longer one is truncated (the caller's bound is
/// 16, so this is a guard, not a limit anyone reaches).
pub const PBKDF2_SALT_MAX: usize = 64;

/// PBKDF2-HMAC-SHA256, first output block only (`dkLen` = 32): `T1 = U1 xor U2 xor … xor Uc`,
/// `U1 = PRF(P, S || INT(1))`, `Uj = PRF(P, U(j-1))`. `iters` of 0 is treated as 1 (the standard
/// forbids 0; the store refuses anything under its own floor before this is ever called).
/// Cost: exactly `2 * iters` SHA-256 compressions after the two prefix states.
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iters: u32, out: &mut [u8; 32]) {
    let prf = HmacSha256::new(password);
    let n = core::cmp::min(salt.len(), PBKDF2_SALT_MAX);
    let mut first = [0u8; PBKDF2_SALT_MAX + 4];
    first[..n].copy_from_slice(&salt[..n]);
    first[n..n + 4].copy_from_slice(&1u32.to_be_bytes());
    let mut u = prf.mac(&first[..n + 4]);
    let mut t = u;
    let c = if iters == 0 { 1 } else { iters };
    for _ in 1..c {
        u = prf.mac(&u);
        for i in 0..32 {
            t[i] ^= u[i];
        }
    }
    *out = t;
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
