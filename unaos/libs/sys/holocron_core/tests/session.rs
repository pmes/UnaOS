// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! LUMENFAST2 (rmbp-ledger B515): the ring unlocks ONCE per session. HOLOCRON.ELF's service runs over a
//! `SessionStore` in front of its UnaFS store; proven here with a counting sealer (every `derive_key` = one KDF)
//! and a counting store (every read = one SYS_PATH_READ walk on the metal):
//!   * N requests after one unlock: the KDF ran once, the ring file was read once, the secret once — Lumen's
//!     `Status` + `Get` at every start touch no medium after the first;
//!   * an absent secret is remembered too (flight 28's `get -> not-found`, every start), and a `put` through the
//!     service replaces the remembered absence;
//!   * lock (the verb and the transport's `lock_now`) wipes the key AND the cache: the next get is LOCKED, the
//!     next unlock runs the KDF again and reads the medium afresh;
//!   * the login door's keyed unlock (RINGLOGIN) runs no KDF at all and also starts from a fresh read.

use std::cell::Cell;
use holocron_core::format::Meta;
use holocron_core::seal::{KdfParams, NONCE_LEN, SALT_LEN, SealError, Sealer};
use holocron_core::service::{Holocron, MemStore, SessionStore, Store, StoreError};
use holocron_core::testseal::{TestEntropy, TestSealer, TestSigner};
use holocron_core::wire::{Request, status};
use holocron_core::zero::Key;

const OWNER: &str = "user:una#1000";
const PW: &[u8] = b"correct horse battery staple";

thread_local! {
    /// Per test thread (the harness runs tests in parallel).
    static KDF: Cell<u32> = const { Cell::new(0) };
}
fn kdf_calls() -> u32 {
    KDF.with(|c| c.get())
}

/// TestSealer, counting derivations.
#[derive(Clone, Copy, Default)]
struct Counting;
impl Sealer for Counting {
    const SUITE: u8 = <TestSealer as Sealer>::SUITE;
    fn derive_key(&self, password: &[u8], salt: &[u8; SALT_LEN], params: &KdfParams) -> Result<Key, SealError> {
        KDF.with(|c| c.set(c.get() + 1));
        TestSealer.derive_key(password, salt, params)
    }
    fn subkey(&self, key: &Key, salt: &[u8], info: &[u8]) -> Key {
        TestSealer.subkey(key, salt, info)
    }
    fn seal(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        TestSealer.seal(key, nonce, aad, plaintext)
    }
    fn open(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SealError> {
        TestSealer.open(key, nonce, aad, sealed)
    }
}

/// MemStore, counting the reads that reach it (the medium).
#[derive(Default)]
struct Medium {
    m: MemStore,
    ring_reads: u32,
    file_reads: u32,
}
impl Store for Medium {
    fn read_ring(&mut self) -> Result<Option<Vec<u8>>, StoreError> {
        self.ring_reads += 1;
        self.m.read_ring()
    }
    fn write_ring(&mut self, b: &[u8]) -> Result<(), StoreError> {
        self.m.write_ring(b)
    }
    fn read(&mut self, ns: &str, n: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.file_reads += 1;
        self.m.read(ns, n)
    }
    fn write(&mut self, ns: &str, n: &str, f: &[u8], meta: &Meta) -> Result<(), StoreError> {
        self.m.write(ns, n, f, meta)
    }
    fn list(&mut self, ns: &str) -> Result<Vec<String>, StoreError> {
        self.m.list(ns)
    }
    fn remove(&mut self, ns: &str, n: &str) -> Result<bool, StoreError> {
        self.m.remove(ns, n)
    }
}

type H = Holocron<Counting, TestSigner, SessionStore<Medium>, TestEntropy>;

fn ask(h: &mut H, r: Request) -> (i32, Vec<u8>) {
    let a = h.handle(Some(OWNER), r.verb(), &r.encode_body(), 0, 0);
    (a.status, a.body.clone())
}

fn get(h: &mut H, name: &str) -> (i32, Vec<u8>) {
    ask(h, Request::Get { ns: "vein".into(), name: name.into() })
}

fn medium(h: &mut H) -> (u32, u32) {
    let m = h.store_mut().inner_mut();
    (m.ring_reads, m.file_reads)
}

#[test]
fn session_kdf_once_reads_once_lock_wipes() {
    let mut h: H = Holocron::new(Counting, TestSigner, SessionStore::new(Medium::default()), TestEntropy::default(), OWNER.into(), KdfParams::FLOOR);
    assert_eq!(ask(&mut h, Request::Unlock { create: true, password: PW.to_vec() }).0, status::OK);
    let put = Request::Put { ns: "vein".into(), name: "claude.api_key".into(), kind: "api-key".into(), label: String::new(), data: b"sk-test".to_vec() };
    assert_eq!(ask(&mut h, put).0, status::OK);
    assert_eq!(kdf_calls(), 1, "create derives once");

    // A new session's fulfiller: the ring is on the medium, locked; one typed unlock.
    let m = core::mem::take(h.store_mut().inner_mut());
    let mut h: H = Holocron::new(Counting, TestSigner, SessionStore::new(Medium { m: m.m, ..Default::default() }), TestEntropy::default(), OWNER.into(), KdfParams::FLOOR);
    assert_eq!(get(&mut h, "claude.api_key").0, status::LOCKED);
    assert_eq!(ask(&mut h, Request::Unlock { create: false, password: PW.to_vec() }).0, status::OK);
    assert_eq!(kdf_calls(), 2, "the session's one unlock");
    let base = medium(&mut h);

    // N Lumen starts: Status + Get each, plus a missing secret asked every time.
    const N: usize = 25;
    for _ in 0..N {
        assert_eq!(ask(&mut h, Request::Status).0, status::OK);
        assert_eq!(get(&mut h, "claude.api_key"), (status::OK, b"sk-test".to_vec()));
        assert_eq!(get(&mut h, "absent.key").0, status::NOT_FOUND);
    }
    assert_eq!(kdf_calls(), 2, "no KDF across {N} requests");
    let after = medium(&mut h);
    assert_eq!(after.0, base.0, "the ring file is not re-read per request");
    assert_eq!(after.1 - base.1, 2, "each secret (present, absent) reached the medium once");

    // A put through the service replaces the remembered absence; no medium read.
    let put = Request::Put { ns: "vein".into(), name: "absent.key".into(), kind: "api-key".into(), label: String::new(), data: b"v2".to_vec() };
    assert_eq!(ask(&mut h, put).0, status::OK);
    assert_eq!(get(&mut h, "absent.key"), (status::OK, b"v2".to_vec()));
    assert_eq!(medium(&mut h), after);
    assert_eq!(ask(&mut h, Request::Delete { ns: "vein".into(), name: "absent.key".into() }).0, status::OK);
    assert_eq!(get(&mut h, "absent.key").0, status::NOT_FOUND);
    assert_eq!(medium(&mut h), after);

    // Lock (the verb): the key and the cache go together.
    assert_eq!(ask(&mut h, Request::Lock).0, status::OK);
    assert!(h.store_mut().is_empty(), "the lock wiped the session cache");
    assert_eq!(get(&mut h, "claude.api_key").0, status::LOCKED);
    assert_eq!(ask(&mut h, Request::Unlock { create: false, password: PW.to_vec() }).0, status::OK);
    assert_eq!(kdf_calls(), 3, "the lock re-runs the KDF");
    let r0 = medium(&mut h);
    assert_eq!(get(&mut h, "claude.api_key").0, status::OK);
    assert_eq!(medium(&mut h).1, r0.1 + 1, "after an unlock the secret is read afresh");

    // Lock from the transport (the lock screen's door LOCK): the same wipe.
    h.lock_now();
    assert!(!h.is_unlocked());
    assert!(h.store_mut().is_empty());
}

#[test]
fn session_login_door_runs_no_kdf_and_reads_afresh() {
    let salt = [0x42; 16];
    let k = TestSealer.derive_key(PW, &salt, &KdfParams::FLOOR).unwrap();
    let mut h: H = Holocron::new(Counting, TestSigner, SessionStore::new(Medium::default()), TestEntropy::default(), OWNER.into(), KdfParams::FLOOR);
    assert_eq!(h.unlock_with_key(true, salt, KdfParams::FLOOR, Key::from_bytes(*k.bytes())), Ok(()));
    let put = Request::Put { ns: "vein".into(), name: "claude.api_key".into(), kind: "api-key".into(), label: String::new(), data: b"sk".to_vec() };
    assert_eq!(ask(&mut h, put).0, status::OK);
    h.lock_now();
    let r0 = medium(&mut h);
    assert_eq!(h.unlock_with_key(false, salt, KdfParams::FLOOR, Key::from_bytes(*k.bytes())), Ok(()));
    assert_eq!(medium(&mut h).0, r0.0 + 1, "the door's unlock reads the ring afresh");
    for _ in 0..10 {
        assert_eq!(ask(&mut h, Request::Status).0, status::OK);
        assert_eq!(get(&mut h, "claude.api_key"), (status::OK, b"sk".to_vec()));
    }
    let r1 = medium(&mut h);
    assert_eq!((r1.0, r1.1), (r0.0 + 1, r0.1 + 1), "ten starts: one ring read, one secret read");
    assert_eq!(kdf_calls(), 0, "the login derived the key; Holocron never runs the KDF");
}
