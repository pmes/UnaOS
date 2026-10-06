// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HOLOCRONROOT (B448): the one store root both rings read, and the one-time move of the legacy root.

use holocron_core::root::{self, RingMove};
use holocron_core::seal::KdfParams;
use holocron_core::service::{Holocron, MemStore, Store};
use holocron_core::testseal::{TestEntropy, TestSealer, TestSigner};
use holocron_core::wire::{Request, status};

const OWNER: &str = "user:peter#1001";
type H = Holocron<TestSealer, TestSigner, MemStore, TestEntropy>;

fn svc(store: MemStore) -> H {
    Holocron::new(TestSealer, TestSigner, store, TestEntropy::default(), OWNER.into(), KdfParams::DEFAULT)
}
fn call(h: &mut H, r: Request) -> holocron_core::wire::Reply {
    h.handle(Some(OWNER), r.verb(), &r.encode_body(), 0, 1_790_000_000)
}
fn put(ns: &str, name: &str, data: &[u8]) -> Request {
    Request::Put { ns: ns.into(), name: name.into(), kind: "api-key".into(), label: "L".into(), data: data.to_vec() }
}
fn get(ns: &str, name: &str) -> Request {
    Request::Get { ns: ns.into(), name: name.into() }
}
/// A legacy store with a ring (password `pw`) and two secrets in two namespaces.
fn legacy(pw: &[u8]) -> MemStore {
    let mut h = svc(MemStore::default());
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: pw.to_vec() }).status, status::OK);
    assert_eq!(call(&mut h, put("vein", "claude.api_key", b"sk-1")).status, status::OK);
    assert_eq!(call(&mut h, put("bt", "a0b1c2d3e4f5", b"0123456789abcdef")).status, status::OK);
    h.store_mut().clone()
}
fn ns() -> Vec<String> {
    root::namespaces(b".ring\nvein/\nbt/\n../\n.hidden/\n")
}

#[test]
fn one_root_both_rings_spell() {
    // The kernel (`keyring::ring_path`), HOLOCRON.ELF's PathStore and the host store all call these.
    assert_eq!(root::DIR, ".holocron");
    assert_eq!(root::root("/home/peter"), "/home/peter/.holocron");
    assert_eq!(root::root("/home/peter/"), "/home/peter/.holocron");
    assert_eq!(root::ring_path("/home/peter"), "/home/peter/.holocron/.ring");
    assert_eq!(root::secret_path("/home/peter", "vein", "claude.api_key"), "/home/peter/.holocron/vein/claude.api_key");
    assert_eq!(root::legacy_roots("/home/peter"), vec![String::from("/home/peter/.config/unaos/holocron")]);
    assert!(!root::LEGACY.contains(&root::DIR));
}

#[test]
fn namespaces_from_a_listing() {
    assert_eq!(ns(), vec![String::from("vein"), String::from("bt")]);
}

#[test]
fn migrate_moves_everything_once_and_it_still_opens() {
    let mut from = legacy(b"pw");
    let mut to = MemStore::default();
    let m = root::migrate(&mut from, &mut to, &ns()).unwrap();
    assert_eq!((m.ring, m.secrets, m.skipped, m.verdict()), (RingMove::Moved, 2, 0, "ok"));
    assert!(from.ring.is_none() && from.files.is_empty(), "the legacy root is emptied");
    // Once: a second run finds nothing.
    let again = root::migrate(&mut from, &mut to, &ns()).unwrap();
    assert!(again.empty());
    // The moved records open under the same password at the new root.
    let mut h = svc(to);
    assert_eq!(call(&mut h, Request::Unlock { create: false, password: b"pw".to_vec() }).status, status::OK);
    let r = call(&mut h, get("vein", "claude.api_key"));
    assert_eq!((r.status, r.body.as_slice()), (status::OK, &b"sk-1"[..]));
    assert_eq!(call(&mut h, get("bt", "a0b1c2d3e4f5")).status, status::OK);
}

#[test]
fn an_interrupted_move_finishes() {
    let mut from = legacy(b"pw");
    let mut to = MemStore::default();
    // Crash after the ring and one secret reached the new root, before any removal.
    to.ring = from.ring.clone();
    let k = (String::from("vein"), String::from("claude.api_key"));
    to.files.insert(k.clone(), from.files[&k].clone());
    let m = root::migrate(&mut from, &mut to, &ns()).unwrap();
    assert_eq!((m.ring, m.secrets, m.skipped), (RingMove::Same, 2, 0));
    assert!(from.ring.is_none() && from.files.is_empty());
    assert_eq!(to.files.len(), 2);
}

#[test]
fn a_different_ring_at_the_new_root_moves_nothing() {
    let mut from = legacy(b"pw");
    let mut to = legacy(b"other");
    let before = to.clone();
    let m = root::migrate(&mut from, &mut to, &ns()).unwrap();
    assert_eq!((m.ring, m.secrets, m.skipped, m.verdict()), (RingMove::Conflict, 0, 2, "conflict"));
    assert_eq!(to.files, before.files);
    assert!(from.ring.is_some() && from.files.len() == 2, "the legacy root is left whole");
}

#[test]
fn an_unparseable_file_stays_and_keeps_the_ring_with_it() {
    let mut from = legacy(b"pw");
    from.files.insert(("vein".into(), "junk".into()), (b"not a secret".to_vec(), Default::default()));
    let mut to = MemStore::default();
    let m = root::migrate(&mut from, &mut to, &ns()).unwrap();
    assert_eq!((m.ring, m.secrets, m.skipped, m.verdict()), (RingMove::Moved, 2, 1, "partial"));
    assert!(from.ring.is_some(), "the legacy ring stays beside what could not move");
    assert_eq!(from.list("vein").unwrap(), vec![String::from("junk")]);
}
