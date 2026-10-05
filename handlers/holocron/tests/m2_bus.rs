// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! M2: the bus over the host's Unix socket — the principal comes from SO_PEERCRED, never from a body.

use holocron::client::{self, Client};
use holocron::daemon::{self, OsEntropy, Shared};
use holocron::principal;
use holocron::store::DirStore;
use holocron_core::keysource::KeySource;
use holocron_core::seal::KdfParams;
use holocron_core::service::Holocron;
use holocron_core::testseal::{TestSealer, TestSigner};
use holocron_core::wire::{self, Request, RingState, status};
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn scratch(tag: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let p = base.join(format!("holocron-m2-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Start a daemon for `owner` over `<dir>/.holocron`; returns the bus socket.
fn start(dir: &Path, owner: &str, idle: Option<Duration>) -> PathBuf {
    let root = dir.join(".holocron");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let svc = Holocron::new(TestSealer, TestSigner, DirStore::new(&root), OsEntropy, owner.into(), KdfParams::DEFAULT);
    let sh = Shared::new(svc, idle);
    let bus = root.join(daemon::BUS_SOCK);
    let l = daemon::bind_private(&bus).unwrap();
    std::thread::spawn(move || daemon::accept_loop(l, sh, daemon::serve_bus_conn));
    bus
}

#[test]
fn bus_over_the_socket_as_the_owner() {
    let me = principal::my_principal().expect("this uid has a passwd entry");
    let dir = scratch("owner");
    let bus = start(&dir, &me, None);
    let m = std::fs::symlink_metadata(&bus).unwrap();
    assert!(m.file_type().is_socket());
    assert_eq!(m.mode() & 0o777, 0o600);
    let mut c = Client::connect(&bus).unwrap();
    let st = c.call(&Request::Status).unwrap();
    assert_eq!(wire::decode_status(&st.body), Some((RingState::NoRing, 0xFE, me.clone())));
    // Consumer rule against an empty Holocron: NotFound -> fall back.
    assert!(matches!(client::claude_api_key(Some(&bus)), KeySource::Fallback));
    assert_eq!(c.call(&Request::Unlock { create: true, password: b"pw".to_vec() }).unwrap().status, status::OK);
    let put = Request::Put { ns: "vein".into(), name: "claude.api_key".into(), kind: "api-key".into(), label: "Claude".into(), data: b"sk-ant-sock".to_vec() };
    assert_eq!(c.call(&put).unwrap().status, status::OK);
    // A second connection sees the same service.
    match client::claude_api_key(Some(&bus)) {
        KeySource::Holocron(k) => assert_eq!(k.expose(), b"sk-ant-sock"),
        other => panic!("{other:?}"),
    }
    // Locked -> the consumer refuses, naming the fix (it does NOT fall back to a stale env copy).
    assert_eq!(c.call(&Request::Lock).unwrap().status, status::OK);
    match client::claude_api_key(Some(&bus)) {
        KeySource::Refuse(why) => assert!(why.contains("holocron unlock"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Rate limit over the wire: three free failures, the fourth arms a wait, the right password is
    // then refused without a KDF run.
    for _ in 0..4 {
        assert_eq!(c.call(&Request::Unlock { create: false, password: b"bad".to_vec() }).unwrap().status, status::BAD_PASSWORD);
    }
    assert_eq!(c.call(&Request::Unlock { create: false, password: b"pw".to_vec() }).unwrap().status, status::RATE_LIMITED);
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(c.call(&Request::Unlock { create: false, password: b"pw".to_vec() }).unwrap().status, status::OK);
    // No Holocron at all -> fall back.
    assert!(matches!(client::claude_api_key(Some(&dir.join("absent.sock"))), KeySource::Fallback));
    assert!(matches!(client::claude_api_key(None), KeySource::Fallback));
    // A second daemon cannot steal a live socket.
    assert!(daemon::bind_private(&bus).is_err());
}

#[test]
fn another_principal_is_denied_by_the_peer_credential() {
    // The daemon serves "user:nobody#65534"; this process's SO_PEERCRED says otherwise, and nothing it
    // writes in a body can change that.
    let dir = scratch("other");
    let bus = start(&dir, "user:nobody#65534", None);
    let mut c = Client::connect(&bus).unwrap();
    for r in [Request::Status, Request::Unlock { create: true, password: b"pw".to_vec() }, Request::Get { ns: "vein".into(), name: "claude.api_key".into() }] {
        let rep = c.call(&r).unwrap();
        assert_eq!((rep.status, rep.body.len()), (status::DENIED, 0));
    }
    match client::claude_api_key(Some(&bus)) {
        KeySource::Refuse(why) => assert!(why.contains("refused"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(!dir.join(".holocron/.ring").exists(), "a denied create wrote nothing");
}

#[test]
fn framing_refusals_and_idle_lock() {
    let me = principal::my_principal().unwrap();
    let dir = scratch("frame");
    let bus = start(&dir, &me, Some(Duration::from_millis(300)));
    // Oversized frame: one INVALID reply, then the daemon hangs up.
    let mut s = UnixStream::connect(&bus).unwrap();
    s.write_all(&((wire::BODY_MAX as u32) + 2).to_le_bytes()).unwrap();
    let mut rep = Vec::new();
    s.read_to_end(&mut rep).unwrap();
    assert_eq!(rep, [4, 0, 0, 0, 0xea, 0xff, 0xff, 0xff]); // len 4, status -22
    // Unknown verb: NO_VERB, connection stays up.
    let mut s = UnixStream::connect(&bus).unwrap();
    s.write_all(&[1, 0, 0, 0, 200]).unwrap();
    let mut r = [0u8; 8];
    s.read_exact(&mut r).unwrap();
    assert_eq!(i32::from_le_bytes(r[4..8].try_into().unwrap()), status::NO_VERB);
    // Idle lock: unlocked, then quiet for longer than the idle window -> the next request finds it locked.
    let mut c = Client::connect(&bus).unwrap();
    assert_eq!(c.call(&Request::Unlock { create: true, password: b"pw".to_vec() }).unwrap().status, status::OK);
    std::thread::sleep(Duration::from_millis(400));
    let st = c.call(&Request::Status).unwrap();
    assert_eq!(wire::decode_status(&st.body).unwrap().0, RingState::Locked);
}
