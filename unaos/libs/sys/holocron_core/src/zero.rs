// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Key-bearing types that wipe themselves. The ONE `unsafe` in this crate is the volatile store that keeps
//! the compiler from eliding the wipe of memory that is about to be freed.

use alloc::vec::Vec;
use core::sync::atomic::{Ordering, compiler_fence};

/// Overwrite `buf` with zeros in a way the optimiser may not remove.
pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` is a valid, aligned, exclusive reference to one byte.
        #[allow(unsafe_code)]
        unsafe {
            core::ptr::write_volatile(b, 0)
        };
    }
    compiler_fence(Ordering::SeqCst);
}

/// A 256-bit symmetric key. Zeroized on drop; never `Copy`, never `Debug`-printed.
pub struct Key(pub(crate) [u8; 32]);

impl Key {
    /// Wrap raw key bytes (the bytes are moved in; the caller should wipe its own copy).
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Key(b)
    }
    /// The key bytes, for a `Sealer` implementation.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

impl core::fmt::Debug for Key {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Key(<redacted>)")
    }
}

/// Plaintext secret bytes. Zeroized on drop.
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    /// Take ownership of plaintext.
    pub fn new(v: Vec<u8>) -> Self {
        SecretBytes(v)
    }
    /// Borrow the plaintext.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        let cap = self.0.capacity();
        self.0.resize(cap, 0);
        wipe(&mut self.0);
    }
}

impl core::fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SecretBytes(<{} bytes redacted>)", self.0.len())
    }
}
