// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! M1: the keyring model on the host — the directory store and the UnaFS store under the service.

use holocron::store::DirStore;
use holocron::unafs_store::UnaFsStore;
use holocron_core::format;
use holocron_core::seal::KdfParams;
use holocron_core::service::{Holocron, MemStore, Store};
use holocron_core::testseal::{TestEntropy, TestSealer, TestSigner};
use holocron_core::wire::{self, Request, status};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

const OWNER: &str = "user:peter#1001";

fn scratch(tag: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let p = base.join(format!("holocron-m1-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn call<T: Store>(h: &mut Holocron<TestSealer, TestSigner, T, TestEntropy>, req: Request) -> wire::Reply {
    h.handle(Some(OWNER), req.verb(), &req.encode_body(), 0, 1_790_000_000)
}

fn exercise<T: Store>(store: T) -> Holocron<TestSealer, TestSigner, T, TestEntropy> {
    let mut h = Holocron::new(TestSealer, TestSigner, store, TestEntropy::default(), OWNER.into(), KdfParams::DEFAULT);
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: b"pw".to_vec() }).status, status::OK);
    let put = Request::Put {
        ns: "vein".into(),
        name: "claude.api_key".into(),
        kind: "api-key".into(),
        label: "Claude".into(),
        data: b"sk-ant-host".to_vec(),
    };
    assert_eq!(call(&mut h, put).status, status::OK);
    let r = call(&mut h, Request::Get { ns: "vein".into(), name: "claude.api_key".into() });
    assert_eq!((r.status, &r.body[..]), (status::OK, &b"sk-ant-host"[..]));
    h
}

#[test]
fn dir_store_layout_modes_and_round_trip() {
    let root = scratch("dir").join(".holocron");
    let mut h = exercise(DirStore::new(&root));
    // Layout: <root>/.ring and <root>/vein/claude.api_key, 0700 dirs, 0600 files.
    for (p, mode) in [(root.clone(), 0o700), (root.join("vein"), 0o700), (root.join(".ring"), 0o600), (root.join("vein/claude.api_key"), 0o600)] {
        let m = std::fs::metadata(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert_eq!(m.mode() & 0o777, mode, "{}", p.display());
    }
    // The plaintext never reaches the disk.
    let bytes = std::fs::read(root.join("vein/claude.api_key")).unwrap();
    assert!(!bytes.windows(11).any(|w| w == b"sk-ant-host"));
    let (hdr, _, _) = format::parse_secret(&bytes).unwrap();
    assert_eq!((hdr.meta.kind.as_str(), hdr.meta.label.as_str(), hdr.meta.created), ("api-key", "Claude", 1_790_000_000));
    // A fresh service over the same directory: locked, then unlocked by the password.
    drop(h);
    h = Holocron::new(TestSealer, TestSigner, DirStore::new(&root), TestEntropy { counter: 99 }, OWNER.into(), KdfParams::DEFAULT);
    let get = || Request::Get { ns: "vein".into(), name: "claude.api_key".into() };
    assert_eq!(call(&mut h, get()).status, status::LOCKED);
    assert_eq!(call(&mut h, Request::Unlock { create: false, password: b"pw".to_vec() }).status, status::OK);
    assert_eq!(call(&mut h, get()).body, b"sk-ant-host");
    // List carries the metadata; delete removes the file.
    let l = call(&mut h, Request::List { ns: "vein".into() });
    let entries = wire::decode_list(&l.body).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "claude.api_key");
    assert_eq!(entries[0].meta.label, "Claude");
    assert_eq!(call(&mut h, Request::Delete { ns: "vein".into(), name: "claude.api_key".into() }).status, status::OK);
    assert!(!root.join("vein/claude.api_key").exists());
    assert_eq!(call(&mut h, get()).status, status::NOT_FOUND);
}

#[test]
fn dir_store_refuses_a_loose_root_and_symlinks() {
    let root = scratch("loose").join(".holocron");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut s = DirStore::new(&root);
    assert!(s.read_ring().is_err(), "a group/world-readable root is refused");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(s.read_ring().unwrap(), None);
    // A symlink planted where a secret would be is refused, not followed.
    std::fs::create_dir(root.join("vein")).unwrap();
    std::fs::set_permissions(root.join("vein"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", root.join("vein/claude.api_key")).unwrap();
    assert!(s.read("vein", "claude.api_key").is_err());
    // Names that are not path components never reach the filesystem.
    assert!(s.read("..", "x").is_err());
    assert!(s.read("vein", "../../etc/passwd").is_err());
}

#[test]
fn unafs_store_typed_attributes_and_query() {
    let fs = unafs::UnaFS::format(unafs::MemDevice::new(), 16).unwrap();
    let mut h = exercise(UnaFsStore::new(fs, "peter").unwrap());
    let key = Request::Put { ns: "ssh".into(), name: "id_ed25519".into(), kind: "ssh-ed25519".into(), label: "peter@rmbp".into(), data: vec![] };
    assert_eq!(call(&mut h, key).status, status::OK);
    let store = h.store_mut();
    let fs = store.fs();
    let id = fs.resolve_path("/home/peter/.holocron/vein/claude.api_key").unwrap();
    use unafs::AttributeValue as A;
    assert_eq!(fs.get_attribute(id, "created").unwrap(), Some(A::Int(1_790_000_000)));
    assert_eq!(fs.get_attribute(id, "kind").unwrap(), Some(A::String("api-key".into())));
    assert_eq!(fs.get_attribute(id, "label").unwrap(), Some(A::String("Claude".into())));
    assert!(fs.resolve_path("/home/peter/.holocron/.ring").is_ok());
    // The typed attribute engine answers queries over the keyring without opening a sealed body.
    let hits = fs.query("kind == \"ssh-ed25519\"").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "/home/peter/.holocron/ssh/id_ed25519");
    let hits = fs.query("created >= 1790000000").unwrap();
    assert_eq!(hits.len(), 2);
    // Overwrite replaces (grow-only write_data never leaves a stale tail).
    let put = Request::Put { ns: "vein".into(), name: "claude.api_key".into(), kind: "api-key".into(), label: "Claude".into(), data: b"k2".to_vec() };
    assert_eq!(call(&mut h, put).status, status::OK);
    let r = call(&mut h, Request::Get { ns: "vein".into(), name: "claude.api_key".into() });
    assert_eq!(r.body, b"k2");
}

#[test]
fn mem_store_matches() {
    let h = exercise(MemStore::default());
    drop(h);
}
