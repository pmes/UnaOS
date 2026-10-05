// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HOLOCRON2 (rmbp-ledger B355): the metal frame codec (`holocron_core::frame`, what HOLOCRON.ELF and
//! LUMEN.ELF put on the BANDY bus) KAT'd against THIS daemon's bytes.
//!
//! 1. The host client's socket bytes for a request equal `frame::host_request(verb, body)` of the ring-3
//!    frame's verb and body (captured off a listening socket: the client is the unmodified `Client`).
//! 2. Ring-3 frames drive the REAL daemon over its socket (body lifted out of the 52-byte header), and
//!    the daemon's reply bytes, re-framed by `frame::reply`, parse back to the same status and body.
//! 3. The bytes match the BANDY v1 header una-abi declares (magic, header length, kinds, register tag).

use holocron::client::Client;
use holocron::daemon::{self, OsEntropy, Shared};
use holocron::principal;
use holocron::store::DirStore;
use holocron_core::frame;
use holocron_core::seal::KdfParams;
use holocron_core::service::Holocron;
use holocron_core::testseal::{TestSealer, TestSigner};
use holocron_core::wire::{self, Request, RingState, status};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

fn scratch(tag: &str) -> PathBuf {
    let base = std::env::var_os("CARGO_TARGET_TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let p = base.join(format!("holocron-m3-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn requests() -> Vec<Request> {
    vec![
        Request::Status,
        Request::Unlock { create: true, password: b"metal-pw".to_vec() },
        Request::Put { ns: "vein".into(), name: "claude.api_key".into(), kind: "api-key".into(), label: "Claude".into(), data: b"sk-ant-metal".to_vec() },
        Request::Get { ns: "vein".into(), name: "claude.api_key".into() },
        Request::List { ns: "vein".into() },
        Request::Lock,
        Request::Get { ns: "vein".into(), name: "claude.api_key".into() },
        Request::Get { ns: "vein".into(), name: "absent".into() },
    ]
}

#[test]
fn host_client_bytes_equal_the_ring3_body() {
    let dir = scratch("client");
    let sock = dir.join("cap.sock");
    let l = UnixListener::bind(&sock).unwrap();
    let reqs = requests();
    let n = reqs.len();
    let cap = std::thread::spawn(move || {
        let mut got = Vec::new();
        for _ in 0..n {
            let (mut s, _) = l.accept().unwrap();
            let mut len = [0u8; 4];
            s.read_exact(&mut len).unwrap();
            let mut rest = vec![0u8; u32::from_le_bytes(len) as usize];
            s.read_exact(&mut rest).unwrap();
            let mut all = len.to_vec();
            all.extend_from_slice(&rest);
            got.push(all);
            s.write_all(&[4, 0, 0, 0, 0, 0, 0, 0]).unwrap(); // OK, empty
        }
        got
    });
    for r in &reqs {
        let mut c = Client::connect(&sock).unwrap();
        let _ = c.call(r).unwrap();
    }
    let got = cap.join().unwrap();
    for (r, host) in reqs.iter().zip(got) {
        let f = frame::request(r.verb(), 0x48_43_00_01, &r.encode_body()).unwrap();
        let p = frame::parse(&f).unwrap();
        assert_eq!(host, frame::host_request(p.verb, p.body), "verb {}", r.verb());
        assert_eq!(&host[5..], &f[frame::HDR_LEN..]);
    }
    println!(":: HOLOCRON2-KAT: client-bytes={} verbs match the ring-3 bodies -> PASS ::", reqs.len());
}

#[test]
fn ring3_frames_drive_the_host_daemon() {
    let me = principal::my_principal().expect("this uid has a passwd entry");
    let dir = scratch("daemon");
    let root = dir.join(".holocron");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let svc = Holocron::new(TestSealer, TestSigner, DirStore::new(&root), OsEntropy, me.clone(), KdfParams::DEFAULT);
    let sh = Shared::new(svc, None);
    let bus = root.join(daemon::BUS_SOCK);
    let l = daemon::bind_private(&bus).unwrap();
    std::thread::spawn(move || daemon::accept_loop(l, sh, daemon::serve_bus_conn));
    let mut s = UnixStream::connect(&bus).unwrap();
    let mut seen = Vec::new();
    for (i, r) in requests().iter().enumerate() {
        let corr = 100 + i as u32;
        let f = frame::request(r.verb(), corr, &r.encode_body()).unwrap();
        let p = frame::parse(&f).unwrap();
        s.write_all(&frame::host_request(p.verb, p.body)).unwrap();
        let mut len = [0u8; 4];
        s.read_exact(&mut len).unwrap();
        let mut rest = vec![0u8; u32::from_le_bytes(len) as usize];
        s.read_exact(&mut rest).unwrap();
        let mut host = len.to_vec();
        host.extend_from_slice(&rest);
        let (st, body) = frame::host_reply(&host).unwrap();
        let back = frame::reply(p.verb, p.corr, st, body);
        let q = frame::parse(&back).unwrap();
        assert_eq!((q.kind, q.verb, q.corr, q.status), (frame::KIND_REPLY, r.verb(), corr, st));
        assert_eq!(q.body, if st == 0 { body } else { &[][..] });
        assert_eq!(q.principal, &[0u8; 32][..], "a fulfiller never claims a principal");
        seen.push((r.verb(), st, q.body.to_vec()));
    }
    assert_eq!(wire::decode_status(&seen[0].2), Some((RingState::NoRing, 0xFE, me.clone())));
    assert_eq!(seen[1].1, status::OK);
    assert_eq!(seen[2].1, status::OK);
    assert_eq!((seen[3].1, seen[3].2.as_slice()), (status::OK, &b"sk-ant-metal"[..]));
    assert_eq!(wire::decode_list(&seen[4].2).unwrap()[0].name, "claude.api_key");
    assert_eq!(seen[5].1, status::OK);
    assert_eq!(seen[6].1, status::LOCKED);
    assert_eq!(seen[7].1, status::NOT_FOUND);
    // The header una-abi declares (BUS v1): the codec restates it, this pins the two together.
    assert_eq!(frame::MAGIC, *b"UBS1");
    assert_eq!(frame::HDR_LEN, 52);
    assert_eq!((frame::KIND_REQUEST, frame::KIND_REPLY, frame::VERB_REGISTER), (1, 2, 127));
    println!(
        ":: HOLOCRON2-KAT: daemon round trips={} put=ok get=ok locked={} notfound={} -> PASS ::",
        seen.len(),
        seen[6].1,
        seen[7].1
    );
}
