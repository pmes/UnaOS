// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The Holocron service: the bus verbs over a [`Ring`], a [`Store`] and a [`Signer`], answered ONLY for
//! the ring's owner. The transport hands [`Holocron::handle`] the caller's principal as the KERNEL (or
//! on the host, the OS) stamped it — `None` when the stamp does not project to a user — and the verb and
//! body; the service never reads a principal out of a body.
//!
//! Policy, in the order it is applied to every request:
//! 1. the caller's principal must equal the service's owner (`user:<name>#<uid>`) — else `DENIED`, before
//!    the body is even decoded;
//! 2. a ring file whose recorded owner is someone else is `DENIED` for every verb;
//! 3. `Unlock` is rate-limited: [`UnlockLimiter::FREE`] failures are free, then each further failure
//!    doubles a wait (1 s, 2 s, 4 s … capped at 5 min) during which `Unlock` is `RATE_LIMITED` without
//!    running the KDF; a success resets it;
//! 4. `SecretGet` answers `NOT_FOUND` when no such file exists (locked or not — existence is not
//!    secret from the owner, and it is the one answer a consumer may fall back on), `LOCKED` when it
//!    exists and the key is not in memory;
//! 5. every mutation (`SecretPut`, `SecretDelete`) and `Sign` needs the ring unlocked.

use crate::format::{self, Meta};
use crate::name;
use crate::ring::{Ring, RingError};
use crate::seal::{Entropy, KdfParams, Sealer, Signer};
use crate::wire::{self, ListEntry, Reply, Request, RingState, status};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// The namespace Ed25519 keys live in.
pub const SSH_NS: &str = "ssh";
/// The `kind` of an Ed25519 key secret (the plaintext is the 32-byte RFC 8032 seed).
pub const KIND_ED25519: &str = "ssh-ed25519";

/// A storage failure (the store's own detail stays in the store's log).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreError;

/// Where ring and secret files live: `/home/<u>/.holocron/.ring` and `/home/<u>/.holocron/<ns>/<name>` — the
/// root is [`crate::root`]'s, the one spelling both rings read (B448).
/// Implementations: the host directory and the UnaFS volume (`handlers/holocron`), [`MemStore`] here,
/// and on the metal the VFS through SYS_OPEN/SYS_ATTR_SET (owed with HOLOCRON.ELF).
pub trait Store {
    /// The ring file, if one exists.
    fn read_ring(&mut self) -> Result<Option<Vec<u8>>, StoreError>;
    /// Write the ring file (atomically where the medium allows).
    fn write_ring(&mut self, bytes: &[u8]) -> Result<(), StoreError>;
    /// One secret file, if it exists.
    fn read(&mut self, ns: &str, name: &str) -> Result<Option<Vec<u8>>, StoreError>;
    /// Write one secret file and set its typed attributes (`created` Int, `kind` Str, `label` Str).
    fn write(&mut self, ns: &str, name: &str, file: &[u8], meta: &Meta) -> Result<(), StoreError>;
    /// The secret names in `ns` (any order).
    fn list(&mut self, ns: &str) -> Result<Vec<String>, StoreError>;
    /// Remove one secret; `Ok(false)` when it did not exist.
    fn remove(&mut self, ns: &str, name: &str) -> Result<bool, StoreError>;
    /// Remove the ring file; `Ok(false)` when it did not exist or the store cannot (HOLOCRONROOT, B448:
    /// only [`crate::root::migrate`] calls it, on a legacy root it has emptied).
    fn remove_ring(&mut self) -> Result<bool, StoreError> {
        Ok(false)
    }
}

/// An in-memory store (tests; and the shape every other store mirrors).
#[derive(Default, Debug, Clone)]
pub struct MemStore {
    /// The ring file.
    pub ring: Option<Vec<u8>>,
    /// `(ns, name) -> (file, meta)`.
    pub files: BTreeMap<(String, String), (Vec<u8>, Meta)>,
}

impl Store for MemStore {
    fn read_ring(&mut self) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.ring.clone())
    }
    fn write_ring(&mut self, bytes: &[u8]) -> Result<(), StoreError> {
        self.ring = Some(bytes.to_vec());
        Ok(())
    }
    fn read(&mut self, ns: &str, name: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.files.get(&(ns.into(), name.into())).map(|(f, _)| f.clone()))
    }
    fn write(&mut self, ns: &str, name: &str, file: &[u8], meta: &Meta) -> Result<(), StoreError> {
        self.files.insert((ns.into(), name.into()), (file.to_vec(), meta.clone()));
        Ok(())
    }
    fn list(&mut self, ns: &str) -> Result<Vec<String>, StoreError> {
        Ok(self.files.keys().filter(|(n, _)| n == ns).map(|(_, k)| k.clone()).collect())
    }
    fn remove(&mut self, ns: &str, name: &str) -> Result<bool, StoreError> {
        Ok(self.files.remove(&(ns.into(), name.into())).is_some())
    }
    fn remove_ring(&mut self) -> Result<bool, StoreError> {
        Ok(self.ring.take().is_some())
    }
}

/// The unlock rate limiter (time is the caller's monotonic milliseconds).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnlockLimiter {
    failures: u32,
    not_before_ms: u64,
}

impl UnlockLimiter {
    /// Failures allowed before any wait.
    pub const FREE: u32 = 3;
    /// The first wait.
    pub const BASE_MS: u64 = 1_000;
    /// The longest wait.
    pub const CAP_MS: u64 = 300_000;

    /// `Err(wait_ms)` while an attempt is not yet allowed.
    pub fn check(&self, now_ms: u64) -> Result<(), u64> {
        if now_ms < self.not_before_ms { Err(self.not_before_ms - now_ms) } else { Ok(()) }
    }
    /// Record a failed attempt at `now_ms`.
    pub fn fail(&mut self, now_ms: u64) {
        self.failures = self.failures.saturating_add(1);
        if self.failures > Self::FREE {
            let shift = (self.failures - Self::FREE - 1).min(20);
            let wait = (Self::BASE_MS << shift).min(Self::CAP_MS);
            self.not_before_ms = now_ms.saturating_add(wait);
        }
    }
    /// Record a success.
    pub fn succeed(&mut self) {
        *self = UnlockLimiter::default();
    }
    /// Consecutive failures so far.
    pub fn failures(&self) -> u32 {
        self.failures
    }
}

/// An SSH identity: the key's name in `ssh/`, its public key and its label (the agent's comment).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Secret name under `ssh/`.
    pub name: String,
    /// RFC 8032 public key.
    pub public: [u8; 32],
    /// Label (the agent comment).
    pub label: String,
}

/// The service.
pub struct Holocron<S: Sealer, G: Signer, T: Store, E: Entropy> {
    ring: Ring<S>,
    signer: G,
    store: T,
    rng: E,
    owner: String,
    params: KdfParams,
    limiter: UnlockLimiter,
}

fn ring_status(e: RingError) -> i32 {
    match e {
        RingError::Locked => status::LOCKED,
        RingError::BadPassword => status::BAD_PASSWORD,
        RingError::Invalid => status::INVALID,
        RingError::Entropy => status::IO,
        RingError::Format(_) | RingError::Suite | RingError::Params | RingError::Auth => status::CORRUPT,
    }
}

impl<S: Sealer, G: Signer, T: Store, E: Entropy> Holocron<S, G, T, E> {
    /// A service for `owner` (`user:<name>#<uid>`) over `store`. New rings are created with `params`.
    pub fn new(sealer: S, signer: G, store: T, rng: E, owner: String, params: KdfParams) -> Self {
        let mut h = Holocron { ring: Ring::new(sealer), signer, store, rng, owner, params, limiter: UnlockLimiter::default() };
        if let Ok(Some(b)) = h.store.read_ring() {
            let _ = h.ring.load_header(&b);
        }
        h
    }

    /// The owner principal this service answers.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The store (tests, and the host's attribute checks).
    pub fn store_mut(&mut self) -> &mut T {
        &mut self.store
    }

    /// The limiter state (witness lines).
    pub fn limiter(&self) -> UnlockLimiter {
        self.limiter
    }

    /// Lock from the transport side (idle timer, session end, signal) — no caller check.
    pub fn lock_now(&mut self) {
        self.ring.lock();
    }

    /// True when the key is in memory.
    pub fn is_unlocked(&self) -> bool {
        self.ring.is_unlocked()
    }

    fn authorise(&mut self, caller: Option<&str>) -> Result<(), i32> {
        if caller != Some(self.owner.as_str()) {
            return Err(status::DENIED);
        }
        // Re-read the ring header each request: another writer (the CLI's first `init`, a restore) may
        // have created or replaced it. A replaced ring drops the key.
        match self.store.read_ring() {
            Ok(Some(b)) => {
                if self.ring.load_header(&b).is_err() {
                    self.ring.lock();
                }
            }
            Ok(None) => {}
            Err(_) => return Err(status::IO),
        }
        if let Some(o) = self.ring.owner() {
            if o != self.owner {
                return Err(status::DENIED);
            }
        }
        Ok(())
    }

    /// Answer one request. `caller` is the transport-stamped principal; `now_ms` a monotonic clock for
    /// the limiter; `now_unix` stamps `created` on a put.
    pub fn handle(&mut self, caller: Option<&str>, verb: u8, body: &[u8], now_ms: u64, now_unix: i64) -> Reply {
        if let Err(s) = self.authorise(caller) {
            return Reply::err(s);
        }
        let req = match Request::decode(verb, body) {
            Ok(r) => r,
            Err(s) => return Reply::err(s),
        };
        match self.dispatch(&req, now_ms, now_unix) {
            Ok(body) => Reply::ok(body),
            Err(s) => Reply::err(s),
        }
    }

    fn dispatch(&mut self, req: &Request, now_ms: u64, now_unix: i64) -> Result<Vec<u8>, i32> {
        match req {
            Request::Status => {
                let (state, suite) = match self.ring.header() {
                    None => (RingState::NoRing, S::SUITE),
                    Some(h) if self.ring.is_unlocked() => (RingState::Unlocked, h.suite),
                    Some(h) => (RingState::Locked, h.suite),
                };
                Ok(wire::encode_status(state, suite, &self.owner))
            }
            Request::Lock => {
                self.ring.lock();
                Ok(Vec::new())
            }
            Request::Unlock { create, password } => self.unlock(*create, password, now_ms).map(|_| Vec::new()),
            Request::Get { ns, name } => self.get(ns, name).map(|(_, v)| v),
            Request::Put { ns, name, kind, label, data } => {
                self.put(ns, name, kind, label, data, now_unix).map(|_| Vec::new())
            }
            Request::List { ns } => self.list(ns).map(|e| wire::encode_list(&e)),
            Request::Delete { ns, name } => {
                check_names(ns, name)?;
                self.need_unlocked()?;
                match self.store.remove(ns, name) {
                    Ok(true) => Ok(Vec::new()),
                    Ok(false) => Err(status::NOT_FOUND),
                    Err(_) => Err(status::IO),
                }
            }
            Request::Sign { key, data } => self.sign(key, data).map(|s| s.to_vec()),
        }
    }

    fn need_unlocked(&self) -> Result<(), i32> {
        match (self.ring.header(), self.ring.is_unlocked()) {
            (None, _) => Err(status::NO_RING),
            (Some(_), false) => Err(status::LOCKED),
            (Some(_), true) => Ok(()),
        }
    }

    fn unlock(&mut self, create: bool, password: &[u8], now_ms: u64) -> Result<(), i32> {
        if password.is_empty() {
            return Err(status::INVALID);
        }
        if self.limiter.check(now_ms).is_err() {
            return Err(status::RATE_LIMITED);
        }
        let existing = self.store.read_ring().map_err(|_| status::IO)?;
        if create {
            if existing.is_some() {
                return Err(status::EXISTS);
            }
            let owner = self.owner.clone();
            let file = self.ring.create(&owner, password, self.params, &mut self.rng).map_err(ring_status)?;
            self.store.write_ring(&file).map_err(|_| {
                self.ring.lock();
                status::IO
            })?;
            self.limiter.succeed();
            return Ok(());
        }
        let Some(file) = existing else { return Err(status::NO_RING) };
        match self.ring.unlock(&file, password) {
            Ok(()) => {
                self.limiter.succeed();
                Ok(())
            }
            Err(e) => {
                self.limiter.fail(now_ms);
                Err(ring_status(e))
            }
        }
    }

    /// RINGLOGIN (B465): open — or, with `create`, make — the owner's ring with a key the LOGIN already derived
    /// from the typed password (`key` = Argon2id(password, `salt`, `params`), one derivation). The transport
    /// (HOLOCRON.ELF's door) has already scoped it to this owner, so there is no caller check; everything else
    /// is [`Holocron::handle`]'s Unlock: `EXISTS` when asked to create over a ring, `NO_RING` when asked to open
    /// none, `BAD_PASSWORD` when the key does not open the verifier (a ring made under another password).
    pub fn unlock_with_key(&mut self, create: bool, salt: [u8; crate::seal::SALT_LEN], params: KdfParams, key: crate::zero::Key) -> Result<(), i32> {
        let existing = self.store.read_ring().map_err(|_| status::IO)?;
        if create {
            if existing.is_some() {
                return Err(status::EXISTS);
            }
            let owner = self.owner.clone();
            let file = self.ring.create_keyed(&owner, params, salt, key, &mut self.rng).map_err(ring_status)?;
            self.store.write_ring(&file).map_err(|_| {
                self.ring.lock();
                status::IO
            })?;
            self.limiter.succeed();
            return Ok(());
        }
        let Some(file) = existing else { return Err(status::NO_RING) };
        self.ring.unlock_keyed(&file, key, &salt).map_err(ring_status)?;
        self.limiter.succeed();
        Ok(())
    }

    /// Read and open `ns/name`.
    fn get(&mut self, ns: &str, name_: &str) -> Result<(Meta, Vec<u8>), i32> {
        check_names(ns, name_)?;
        let Some(file) = self.store.read(ns, name_).map_err(|_| status::IO)? else {
            return Err(status::NOT_FOUND);
        };
        if self.ring.header().is_none() {
            // A secret file with no ring cannot be opened by anyone: it is not "found".
            return Err(status::NOT_FOUND);
        }
        let (meta, pt) = self.ring.open_secret(ns, name_, &file).map_err(ring_status)?;
        Ok((meta, pt.expose().to_vec()))
    }

    fn put(&mut self, ns: &str, name_: &str, kind: &str, label: &str, data: &[u8], now_unix: i64) -> Result<(), i32> {
        check_names(ns, name_)?;
        self.need_unlocked()?;
        if data.len() > format::SECRET_MAX {
            return Err(status::TOO_BIG);
        }
        if kind.len() > 255 || label.len() > 255 {
            return Err(status::INVALID);
        }
        let is_key_ns = ns == SSH_NS;
        if is_key_ns != (kind == KIND_ED25519) {
            return Err(status::INVALID);
        }
        let mut seed = [0u8; 32];
        let plaintext: &[u8] = if is_key_ns {
            match data.len() {
                0 => self.rng.fill(&mut seed).map_err(|_| status::IO)?,
                32 => seed.copy_from_slice(data),
                _ => return Err(status::INVALID),
            }
            &seed
        } else {
            data
        };
        let meta = Meta { created: now_unix, kind: kind.into(), label: label.into() };
        let file = self.ring.seal_secret(ns, name_, &meta, plaintext, &mut self.rng).map_err(ring_status);
        crate::zero::wipe(&mut seed);
        let file = file?;
        self.store.write(ns, name_, &file, &meta).map_err(|_| status::IO)
    }

    fn list(&mut self, ns: &str) -> Result<Vec<ListEntry>, i32> {
        if !name::valid(ns) {
            return Err(status::INVALID);
        }
        let mut names = self.store.list(ns).map_err(|_| status::IO)?;
        names.sort();
        let mut out = Vec::new();
        for n in names {
            if !name::valid(&n) {
                continue;
            }
            let meta = match self.store.read(ns, &n) {
                Ok(Some(f)) => match format::parse_secret(&f) {
                    Ok((h, _, _)) => h.meta,
                    Err(_) => Meta { created: 0, kind: "!corrupt".into(), label: String::new() },
                },
                Ok(None) => continue,
                Err(_) => return Err(status::IO),
            };
            out.push(ListEntry { name: n, meta });
        }
        Ok(out)
    }

    fn key_seed(&mut self, key: &str) -> Result<(Meta, [u8; 32]), i32> {
        let (meta, pt) = self.get(SSH_NS, key)?;
        let mut pt = pt;
        if meta.kind != KIND_ED25519 || pt.len() != 32 {
            crate::zero::wipe(&mut pt);
            return Err(status::CORRUPT);
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&pt);
        crate::zero::wipe(&mut pt);
        Ok((meta, seed))
    }

    fn sign(&mut self, key: &str, data: &[u8]) -> Result<[u8; 64], i32> {
        self.need_unlocked()?;
        let (_, mut seed) = self.key_seed(key)?;
        let sig = self.signer.sign(&seed, data);
        crate::zero::wipe(&mut seed);
        Ok(sig)
    }

    /// The SSH identities in `ssh/` (owner only; empty while locked, as ssh-agent answers when locked).
    pub fn identities(&mut self, caller: Option<&str>) -> Result<Vec<Identity>, i32> {
        self.authorise(caller)?;
        if !self.ring.is_unlocked() {
            return Ok(Vec::new());
        }
        let entries = self.list(SSH_NS)?;
        let mut out = Vec::new();
        for e in entries {
            if e.meta.kind != KIND_ED25519 {
                continue;
            }
            if let Ok((meta, mut seed)) = self.key_seed(&e.name) {
                let public = self.signer.public_key(&seed);
                crate::zero::wipe(&mut seed);
                out.push(Identity { name: e.name, public, label: meta.label });
            }
        }
        Ok(out)
    }

    /// Sign `data` with the identity whose public key is `public` (the agent's SIGN_REQUEST).
    pub fn sign_by_public(&mut self, caller: Option<&str>, public: &[u8; 32], data: &[u8]) -> Result<[u8; 64], i32> {
        let ids = self.identities(caller)?;
        let id = ids.iter().find(|i| &i.public == public).ok_or(status::NOT_FOUND)?;
        let name = id.name.clone();
        self.sign(&name, data)
    }

    /// Verify with the signer (tests, and the agent's self-check).
    pub fn verify(&self, public: &[u8; 32], data: &[u8], sig: &[u8; 64]) -> bool {
        self.signer.verify(public, data, sig)
    }

    /// True when the signer is a real RFC 8032 implementation.
    pub fn signer_is_real(&self) -> bool {
        G::REAL
    }
}

fn check_names(ns: &str, name_: &str) -> Result<(), i32> {
    if name::valid(ns) && name::valid(name_) { Ok(()) } else { Err(status::INVALID) }
}
