// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The ring: one key per user, derived from the login password with Argon2id and held in memory for
//! the session only. `lock` wipes it; nothing derived from it is ever written to disk.
//!
//! Key schedule (all through the [`Sealer`]):
//! * ring key `K` = Argon2id(password, ring salt, ring params)
//! * verifier key = HKDF(K, ring salt, "holocron/v1/verifier") — opens the ring file's verifier at
//!   unlock, so a wrong password is detected without touching any secret
//! * file key = HKDF(K, file salt, "holocron/v1/secret") — a fresh random salt and nonce per write,
//!   so no (key, nonce) pair is ever reused even if the entropy source repeats a nonce

use crate::format::{
    self, FormatError, INFO_SECRET, INFO_VERIFIER, Meta, RingHeader, SECRET_MAX, SecretHeader, VERIFIER_PLAINTEXT,
};
use crate::name;
use crate::seal::{Entropy, KdfParams, NONCE_LEN, SALT_LEN, SealError, Sealer};
use crate::zero::{Key, SecretBytes};
use alloc::string::String;
use alloc::vec::Vec;

/// Why a ring operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingError {
    /// The ring is locked.
    Locked,
    /// The password did not open the verifier.
    BadPassword,
    /// The file (ring or secret) is not v1 of this format.
    Format(FormatError),
    /// The file was sealed by another suite (e.g. a TEST file under the production sealer).
    Suite,
    /// The header's KDF parameters are below the floor, or a secret's differ from the ring's.
    Params,
    /// Authentication failed: tampered body, header, name or namespace.
    Auth,
    /// A name, kind, label or size is outside what the format allows.
    Invalid,
}

impl From<FormatError> for RingError {
    fn from(e: FormatError) -> Self {
        RingError::Format(e)
    }
}

/// The ring state machine over one [`Sealer`].
pub struct Ring<S: Sealer> {
    sealer: S,
    header: Option<RingHeader>,
    key: Option<Key>,
}

impl<S: Sealer> Ring<S> {
    /// A ring with no ring file loaded.
    pub fn new(sealer: S) -> Self {
        Ring { sealer, header: None, key: None }
    }

    /// The sealer (for the signer-side helpers).
    pub fn sealer(&self) -> &S {
        &self.sealer
    }

    /// True when the key is in memory.
    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    /// The loaded ring header, if any.
    pub fn header(&self) -> Option<&RingHeader> {
        self.header.as_ref()
    }

    /// Create a new ring for `owner` under `password`. Returns the ring file bytes; the ring is left
    /// UNLOCKED. Refuses parameters below [`KdfParams::FLOOR`].
    pub fn create(
        &mut self,
        owner: &str,
        password: &[u8],
        params: KdfParams,
        rng: &mut dyn Entropy,
    ) -> Result<Vec<u8>, RingError> {
        if !params.acceptable() || owner.is_empty() || owner.len() > 255 {
            return Err(RingError::Invalid);
        }
        let mut salt = [0u8; SALT_LEN];
        rng.fill(&mut salt);
        let mut nonce = [0u8; NONCE_LEN];
        rng.fill(&mut nonce);
        let hdr = RingHeader { suite: S::SUITE, kdf: params, salt, nonce, owner: owner.into() };
        let hbytes = hdr.encode();
        let k = self.sealer.derive_key(password, &salt, &params).map_err(|_| RingError::Params)?;
        let vk = self.sealer.subkey(&k, &salt, INFO_VERIFIER);
        let ver = self.sealer.seal(&vk, &nonce, &hbytes, VERIFIER_PLAINTEXT);
        let mut file = hbytes;
        file.extend_from_slice(&ver);
        self.header = Some(hdr);
        self.key = Some(k);
        Ok(file)
    }

    /// Unlock with `password` against the ring file bytes. On any failure the ring stays locked.
    pub fn unlock(&mut self, ring_file: &[u8], password: &[u8]) -> Result<(), RingError> {
        self.lock();
        let (hdr, hbytes, ver) = format::parse_ring(ring_file)?;
        if hdr.suite != S::SUITE {
            return Err(RingError::Suite);
        }
        if !hdr.kdf.acceptable() {
            return Err(RingError::Params);
        }
        let k = self.sealer.derive_key(password, &hdr.salt, &hdr.kdf).map_err(|_| RingError::Params)?;
        let vk = self.sealer.subkey(&k, &hdr.salt, INFO_VERIFIER);
        match self.sealer.open(&vk, &hdr.nonce, hbytes, ver) {
            Ok(pt) if crate::ct_eq(&pt, VERIFIER_PLAINTEXT) => {
                self.header = Some(hdr);
                self.key = Some(k);
                Ok(())
            }
            _ => Err(RingError::BadPassword),
        }
    }

    /// Load the ring header without unlocking (owner and parameters for status).
    pub fn load_header(&mut self, ring_file: &[u8]) -> Result<(), RingError> {
        let (hdr, _, _) = format::parse_ring(ring_file)?;
        if self.header.as_ref() != Some(&hdr) {
            self.key = None;
        }
        self.header = Some(hdr);
        Ok(())
    }

    /// Drop the key (wiped by [`Key`]'s `Drop`).
    pub fn lock(&mut self) {
        self.key = None;
    }

    /// Seal `plaintext` as the secret `ns/name`. Returns the whole file.
    pub fn seal_secret(
        &self,
        ns: &str,
        name_: &str,
        meta: &Meta,
        plaintext: &[u8],
        rng: &mut dyn Entropy,
    ) -> Result<Vec<u8>, RingError> {
        let (k, hdr) = self.unlocked()?;
        if !name::valid(ns) || !name::valid(name_) || meta.kind.len() > 255 || meta.label.len() > 255 {
            return Err(RingError::Invalid);
        }
        if plaintext.len() > SECRET_MAX {
            return Err(RingError::Invalid);
        }
        let mut salt = [0u8; SALT_LEN];
        rng.fill(&mut salt);
        let mut nonce = [0u8; NONCE_LEN];
        rng.fill(&mut nonce);
        let sh = SecretHeader {
            suite: S::SUITE,
            kdf: hdr.kdf,
            salt,
            nonce,
            meta: meta.clone(),
            sealed_len: (plaintext.len() + crate::seal::TAG_LEN) as u32,
        };
        let hbytes = sh.encode();
        let aad = format::secret_aad(&hbytes, ns, name_);
        let fk = self.sealer.subkey(k, &salt, INFO_SECRET);
        let sealed = self.sealer.seal(&fk, &nonce, &aad, plaintext);
        let mut file = hbytes;
        file.extend_from_slice(&sealed);
        Ok(file)
    }

    /// Open the secret file `bytes` as `ns/name`.
    pub fn open_secret(&self, ns: &str, name_: &str, bytes: &[u8]) -> Result<(Meta, SecretBytes), RingError> {
        let (k, ring) = self.unlocked()?;
        let (sh, hbytes, sealed) = format::parse_secret(bytes)?;
        if sh.suite != S::SUITE {
            return Err(RingError::Suite);
        }
        if sh.kdf != ring.kdf {
            return Err(RingError::Params);
        }
        let aad = format::secret_aad(hbytes, ns, name_);
        let fk = self.sealer.subkey(k, &sh.salt, INFO_SECRET);
        let pt = self.sealer.open(&fk, &sh.nonce, &aad, sealed).map_err(|e| match e {
            SealError::Auth => RingError::Auth,
            SealError::Param => RingError::Invalid,
        })?;
        Ok((sh.meta, SecretBytes::new(pt)))
    }

    fn unlocked(&self) -> Result<(&Key, &RingHeader), RingError> {
        match (&self.key, &self.header) {
            (Some(k), Some(h)) => Ok((k, h)),
            _ => Err(RingError::Locked),
        }
    }

    /// The owner principal of the loaded ring.
    pub fn owner(&self) -> Option<String> {
        self.header.as_ref().map(|h| h.owner.clone())
    }
}
