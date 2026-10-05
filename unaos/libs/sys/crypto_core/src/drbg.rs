// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A ChaCha20 "fast-key-erasure" DRBG over a pluggable [`Entropy`] source.
//!
//! Construction (D. J. Bernstein, "Fast-key-erasure random-number generators", 2017 — the design
//! Linux's `getrandom` and OpenBSD's `arc4random` follow):
//!  * state: one 32-byte ChaCha20 key;
//!  * generate(n): run ChaCha20(key, nonce = 0) from block 0; the FIRST 32 keystream bytes become the
//!    next key, the following n bytes are the output, and the old key is overwritten before returning.
//!    A later compromise of the state therefore reveals nothing about earlier outputs (backtracking
//!    resistance);
//!  * seed / reseed: `key = SHA-256("UnaOS CRYPTOCORE ChaCha20-DRBG v1" || old key || 48 fresh entropy
//!    bytes || personalization/additional input)` — SHA-256 as the conditioner (SP 800-90A's Hash_df
//!    role), the old key chained in so a weak reseed never lowers the state's entropy;
//!  * reseed interval: every [`RESEED_INTERVAL`] requests or [`RESEED_BYTES`] output bytes, whichever comes
//!    first (prediction resistance after a state compromise recovers within one interval). A request is
//!    capped at [`MAX_REQUEST`] bytes.
//!
//! There is no standard KAT for this construction; the harness proves it against its definition: the
//! output equals the ChaCha20 keystream (itself RFC 8439-proven) under the SHA-256-conditioned key.
//!
//! CONSTANT-TIME: yes in the key and entropy (ChaCha20 + SHA-256); request sizes are public.
//!
//! Entropy sources: the KERNEL implements [`Entropy`] over `rand.rs` (RDSEED / RDRAND / RNDR / jitter);
//! RING 3 wraps `SYS_GETRANDOM` with [`GetrandomEntropy`] (this crate has no dependencies, so the
//! syscall itself is a closure the program supplies); host tools use `OsEntropy` (feature `std`).

use crate::chacha20::ChaCha20;
use crate::ct::Zeroize;
use crate::sha2::Sha256;
use crate::Error;

/// Requests between reseeds.
pub const RESEED_INTERVAL: u64 = 1 << 16;
/// Output bytes between reseeds (1 MiB).
pub const RESEED_BYTES: u64 = 1 << 20;
/// Largest single request (64 KiB).
pub const MAX_REQUEST: usize = 1 << 16;
/// Fresh entropy bytes drawn per (re)seed.
pub const SEED_BYTES: usize = 48;

const DOMAIN: &[u8] = b"UnaOS CRYPTOCORE ChaCha20-DRBG v1";

/// A source of entropy. `fill` must write `out.len()` unpredictable bytes or fail.
pub trait Entropy {
    /// Fill `out` completely, or return `Err(Error::Entropy)`.
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Error>;
}

/// Ring-3 adapter for the UnaOS `SYS_GETRANDOM(buf, len) -> written | -errno` contract (una-abi:
/// at most `GETRANDOM_MAX` = 256 bytes per call, short counts legal). `F` performs one syscall.
pub struct GetrandomEntropy<F: FnMut(&mut [u8]) -> isize> {
    sys: F,
}

impl<F: FnMut(&mut [u8]) -> isize> GetrandomEntropy<F> {
    /// Wrap the syscall.
    pub fn new(sys: F) -> Self {
        GetrandomEntropy { sys }
    }
}

impl<F: FnMut(&mut [u8]) -> isize> Entropy for GetrandomEntropy<F> {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Error> {
        let mut pos = 0;
        let mut zero_reads = 0;
        while pos < out.len() {
            let end = core::cmp::min(out.len(), pos + 256);
            let r = (self.sys)(&mut out[pos..end]);
            if r < 0 {
                return Err(Error::Entropy);
            }
            if r == 0 {
                zero_reads += 1;
                if zero_reads > 16 {
                    return Err(Error::Entropy);
                }
                continue;
            }
            pos += core::cmp::min(r as usize, end - pos);
        }
        Ok(())
    }
}

/// Host entropy: `/dev/urandom` (Linux, macOS). Feature `std`.
#[cfg(feature = "std")]
pub struct OsEntropy;

#[cfg(feature = "std")]
impl Entropy for OsEntropy {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Error> {
        use std::io::Read;
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(out)).map_err(|_| Error::Entropy)
    }
}

/// The DRBG.
pub struct ChaChaDrbg<E: Entropy> {
    key: [u8; 32],
    source: E,
    requests: u64,
    bytes: u64,
}

impl<E: Entropy> ChaChaDrbg<E> {
    /// Instantiate: draw [`SEED_BYTES`] from `source`, mix in `personalization`.
    pub fn new(source: E, personalization: &[u8]) -> Result<Self, Error> {
        let mut d = ChaChaDrbg { key: [0; 32], source, requests: 0, bytes: 0 };
        d.reseed(personalization)?;
        Ok(d)
    }

    /// Reseed now from the entropy source, mixing in `additional`.
    pub fn reseed(&mut self, additional: &[u8]) -> Result<(), Error> {
        let mut e = [0u8; SEED_BYTES];
        self.source.fill(&mut e)?;
        let mut h = Sha256::new();
        h.update(DOMAIN);
        h.update(&self.key);
        h.update(&e);
        h.update(additional);
        self.key = h.finalize();
        e.zeroize();
        self.requests = 0;
        self.bytes = 0;
        Ok(())
    }

    /// Fill `out` (at most [`MAX_REQUEST`] bytes; `Error::Length` beyond). Reseeds first when the
    /// interval has elapsed; an entropy failure at that point is returned, never papered over.
    pub fn fill(&mut self, out: &mut [u8]) -> Result<(), Error> {
        if out.len() > MAX_REQUEST {
            return Err(Error::Length);
        }
        if self.requests >= RESEED_INTERVAL || self.bytes >= RESEED_BYTES {
            self.reseed(&[])?;
        }
        let mut c = ChaCha20::new(&self.key, &[0u8; 12], 0);
        let mut next = [0u8; 32];
        c.apply_keystream(&mut next);
        for b in out.iter_mut() {
            *b = 0;
        }
        c.apply_keystream(out);
        self.key = next;
        next.zeroize();
        self.requests += 1;
        self.bytes += out.len() as u64;
        Ok(())
    }

    /// Requests served since the last (re)seed.
    pub fn requests_since_seed(&self) -> u64 {
        self.requests
    }
}

impl<E: Entropy> Drop for ChaChaDrbg<E> {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}
