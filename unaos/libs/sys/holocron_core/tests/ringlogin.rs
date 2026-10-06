// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RINGLOGIN (B465): the ring the LOGIN opens. The kernel derives the ring key ONCE from the typed password
//! (SYS_KDF's body: `crypto_core::argon2::argon2` over caller-lent blocks) and HOLOCRON.ELF applies it with
//! `Holocron::unlock_with_key`. Proven here on the production suite at the floor: the kernel's call and
//! `CryptoCore::derive_key` are the same key; a keyed ring opens with the password and a password ring opens
//! with the key; a key for another password or another salt is BAD_PASSWORD; create-over and open-none refuse.

use holocron_core::cc::CryptoCore;
use holocron_core::format;
use holocron_core::seal::{KdfParams, Sealer};
use holocron_core::service::{Holocron, MemStore, Store};
use holocron_core::testseal::{TestEntropy, TestSigner};
use holocron_core::wire::{Request, status};
use holocron_core::zero::Key;

const OWNER: &str = "user:peter#1001";
const PW: &[u8] = b"correct horse battery staple";
const P: KdfParams = KdfParams::FLOOR;
type H = Holocron<CryptoCore, TestSigner, MemStore, TestEntropy>;

fn svc(store: MemStore) -> H {
    Holocron::new(CryptoCore, TestSigner, store, TestEntropy::default(), OWNER.into(), P)
}

/// The kernel's derivation, exactly as `keyring::kdf` makes it (blocks lent by the caller).
fn kernel_kdf(pw: &[u8], salt: &[u8; 16], p: &KdfParams) -> [u8; 32] {
    use crypto_core::argon2::{Block, Params, Variant, Version, argon2};
    let params = Params { variant: Variant::Argon2id, version: Version::V0x13, m_kib: p.m_kib, t: p.t, p: p.p };
    let mut mem = vec![Block::ZERO; params.blocks()];
    let mut out = [0u8; 32];
    argon2(&params, pw, salt, &[], &[], &mut mem, &mut out).unwrap();
    out
}

#[test]
fn ringlogin_one_derivation_both_ways() {
    let salt = [0x5a; 16];
    let k = kernel_kdf(PW, &salt, &P);
    let cc = CryptoCore.derive_key(PW, &salt, &P).unwrap();
    assert_eq!(&k, cc.bytes(), "the kernel's SYS_KDF body and CryptoCore::derive_key are one derivation");

    // Created at login with the kernel's key; then opened by the console's typed password.
    let mut h = svc(MemStore::default());
    assert_eq!(h.unlock_with_key(false, salt, P, Key::from_bytes(k)), Err(status::NO_RING));
    assert_eq!(h.unlock_with_key(true, salt, P, Key::from_bytes(k)), Ok(()));
    assert!(h.is_unlocked());
    assert_eq!(h.unlock_with_key(true, salt, P, Key::from_bytes(k)), Err(status::EXISTS));
    let put = Request::Put { ns: "bt".into(), name: "a1b2c3d4e5f6".into(), kind: "bt.linkkey".into(), label: "kbd".into(), data: vec![7; 16] };
    assert_eq!(h.handle(Some(OWNER), put.verb(), &put.encode_body(), 0, 0).status, status::OK);
    h.lock_now();
    let unlock = Request::Unlock { create: false, password: PW.to_vec() };
    assert_eq!(h.handle(Some(OWNER), unlock.verb(), &unlock.encode_body(), 0, 0).status, status::OK);
    let get = Request::Get { ns: "bt".into(), name: "a1b2c3d4e5f6".into() };
    let r = h.handle(Some(OWNER), get.verb(), &get.encode_body(), 0, 0);
    assert_eq!((r.status, r.body.as_slice()), (status::OK, &[7u8; 16][..]));

    // Re-opened at the NEXT login: the kernel reads the header's salt and parameters and derives again.
    h.lock_now();
    let file = h.store_mut().read_ring().unwrap().unwrap();
    let (hdr, _, _) = format::parse_ring(&file).unwrap();
    assert_eq!((hdr.salt, hdr.kdf), (salt, P));
    assert_eq!(h.unlock_with_key(false, hdr.salt, hdr.kdf, Key::from_bytes(kernel_kdf(PW, &hdr.salt, &hdr.kdf))), Ok(()));
    assert_eq!(h.handle(Some(OWNER), get.verb(), &get.encode_body(), 0, 0).status, status::OK);

    // Another password's key, or the right password at another salt: refused, and the ring stays locked.
    h.lock_now();
    let wrong = kernel_kdf(b"another password", &hdr.salt, &hdr.kdf);
    assert_eq!(h.unlock_with_key(false, hdr.salt, hdr.kdf, Key::from_bytes(wrong)), Err(status::BAD_PASSWORD));
    assert!(!h.is_unlocked());
    let other_salt = [0x11; 16];
    let k2 = kernel_kdf(PW, &other_salt, &hdr.kdf);
    assert_eq!(h.unlock_with_key(false, other_salt, hdr.kdf, Key::from_bytes(k2)), Err(status::BAD_PASSWORD));
    assert!(!h.is_unlocked());
}

#[test]
fn ringlogin_a_typed_init_ring_opens_with_the_login_key() {
    // A ring made by the console's `holocron init <pw>` (the password path) opens with the login's key.
    let mut h = svc(MemStore::default());
    let init = Request::Unlock { create: true, password: PW.to_vec() };
    assert_eq!(h.handle(Some(OWNER), init.verb(), &init.encode_body(), 0, 0).status, status::OK);
    h.lock_now();
    let file = h.store_mut().read_ring().unwrap().unwrap();
    let (hdr, _, _) = format::parse_ring(&file).unwrap();
    let k = kernel_kdf(PW, &hdr.salt, &hdr.kdf);
    assert_eq!(h.unlock_with_key(false, hdr.salt, hdr.kdf, Key::from_bytes(k)), Ok(()));
    assert!(h.is_unlocked());
}

/// RINGLOGIN2 (B479): a password change re-wraps the ring. The kernel derives BOTH keys at the ring's own salt
/// and parameters; `rekey_with_keys` (→ `Ring::rewrap_keyed`) keeps the salt, the parameters and the owner,
/// reseals every secret, and after it the NEW password opens the ring and every secret while the OLD one is
/// BAD_PASSWORD. A wrong old key, or a store write that fails, keeps the old ring byte for byte.
#[test]
fn ringlogin2_rekey_keeps_salt_and_secrets() {
    const NEW: &[u8] = b"a new password, typed twice";
    let salt = [0x33; 16];
    let k_old = kernel_kdf(PW, &salt, &P);
    let k_new = kernel_kdf(NEW, &salt, &P);
    let mut h = svc(MemStore::default());
    assert_eq!(h.unlock_with_key(true, salt, P, Key::from_bytes(k_old)), Ok(()));
    for (ns, nm) in [("bt", "a1b2c3d4e5f6"), ("vein", "claude.api_key")] {
        let put = Request::Put { ns: ns.into(), name: nm.into(), kind: "k".into(), label: "l".into(), data: nm.as_bytes().to_vec() };
        assert_eq!(h.handle(Some(OWNER), put.verb(), &put.encode_body(), 0, 0).status, status::OK);
    }
    let before = h.store_mut().read_ring().unwrap().unwrap();

    // A wrong old key: refused, nothing written.
    let wrong = kernel_kdf(b"not the password", &salt, &P);
    assert_eq!(h.rekey_with_keys(Key::from_bytes(wrong), Key::from_bytes(k_new)), Err(status::BAD_PASSWORD));
    assert_eq!(h.store_mut().read_ring().unwrap().unwrap(), before);

    // The re-wrap.
    assert_eq!(h.rekey_with_keys(Key::from_bytes(k_old), Key::from_bytes(k_new)), Ok(2));
    assert!(h.is_unlocked());
    let after = h.store_mut().read_ring().unwrap().unwrap();
    let (h0, _, _) = format::parse_ring(&before).unwrap();
    let (h1, _, _) = format::parse_ring(&after).unwrap();
    assert_eq!((h0.salt, h0.kdf, &h0.owner), (h1.salt, h1.kdf, &h1.owner), "salt, parameters and owner are kept");
    assert_ne!(before, after);

    // The next login: the new password opens it and every secret; the old one is refused.
    let store = h.store_mut().clone();
    let mut h2 = svc(store);
    assert_eq!(h2.unlock_with_key(false, salt, P, Key::from_bytes(k_old)), Err(status::BAD_PASSWORD));
    assert_eq!(h2.unlock_with_key(false, salt, P, Key::from_bytes(k_new)), Ok(()));
    for nm in ["a1b2c3d4e5f6", "claude.api_key"] {
        let ns = if nm == "claude.api_key" { "vein" } else { "bt" };
        let get = Request::Get { ns: ns.into(), name: nm.into() };
        let r = h2.handle(Some(OWNER), get.verb(), &get.encode_body(), 0, 0);
        assert_eq!((r.status, r.body.as_slice()), (status::OK, nm.as_bytes()));
    }
    // And the typed password path agrees.
    h2.lock_now();
    let unlock = Request::Unlock { create: false, password: NEW.to_vec() };
    assert_eq!(h2.handle(Some(OWNER), unlock.verb(), &unlock.encode_body(), 0, 0).status, status::OK);

    // No ring: NO_RING, nothing made.
    let mut h3 = svc(MemStore::default());
    assert_eq!(h3.rekey_with_keys(Key::from_bytes(k_old), Key::from_bytes(k_new)), Err(status::NO_RING));
}
