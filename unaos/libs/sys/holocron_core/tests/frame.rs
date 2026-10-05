// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HOLOCRON2 (B355): the metal frame codec — a golden frame written byte by byte from the BANDY v1 header
//! table (not from the encoder), the parser's refusals, and a relayed request answered by the service with
//! the caller taken from the kernel's stamp (a user record) or refused (x86's `row:<r>/gen:<g>`).

use holocron_core::frame::{self, KIND_REPLY, KIND_REQUEST};
use holocron_core::seal::KdfParams;
use holocron_core::service::{Holocron, MemStore};
use holocron_core::testseal::{TestEntropy, TestSealer, TestSigner};
use holocron_core::wire::{self, Request, status};

fn record(kind: u8, v: &[u8]) -> [u8; 32] {
    let mut p = [0u8; 32];
    p[0] = kind;
    p[1] = v.len() as u8;
    p[2..2 + v.len()].copy_from_slice(v);
    p
}

#[test]
fn golden_secret_get_request() {
    let body = Request::Get { ns: "vein".into(), name: "claude.api_key".into() }.encode_body();
    let f = frame::request(wire::VERB_SECRET_GET, 7, &body).unwrap();
    let mut want = vec![0x55, 0x42, 0x53, 0x31, 0x01, 0x01, 144, 0x00]; // UBS1, v1, REQUEST, SecretGet, rsvd
    want.extend_from_slice(&[7, 0, 0, 0, 0, 0, 0, 0]); // corr 7, status 0
    want.extend_from_slice(&[0u8; 32]); // principal zero: the kernel stamps
    want.extend_from_slice(&[20, 0, 0, 0]); // body_len = 1+4+1+14
    want.push(4);
    want.extend_from_slice(b"vein");
    want.push(14);
    want.extend_from_slice(b"claude.api_key");
    assert_eq!(f, want);
    let p = frame::parse(&f).unwrap();
    assert_eq!((p.kind, p.verb, p.corr, p.status, p.body), (KIND_REQUEST, 144, 7, 0, &body[..]));
    // REGISTER carries the eight tags.
    let r = frame::register(1, &wire::VERBS);
    assert_eq!(&r[..8], &[0x55, 0x42, 0x53, 0x31, 1, 1, 127, 0]);
    assert_eq!(&r[48..], &[8, 0, 0, 0, 144, 145, 146, 147, 148, 149, 150, 151]);
    // An error reply drops its body.
    let e = frame::reply(144, 9, status::LOCKED, b"never");
    assert_eq!(e.len(), frame::HDR_LEN);
    assert_eq!(i32::from_le_bytes(e[12..16].try_into().unwrap()), status::LOCKED);
    println!(":: HOLOCRON2-FRAME: golden=SecretGet register=8 err-body=0 -> PASS ::");
}

#[test]
fn parser_refuses() {
    let good = frame::request(151, 1, &[]).unwrap();
    assert!(frame::parse(&good).is_some());
    for (i, v) in [(0usize, b'X'), (4, 2), (5, 3), (7, 1), (48, 1)] {
        let mut b = good.clone();
        b[i] = v;
        assert!(frame::parse(&b).is_none(), "byte {i}");
    }
    assert!(frame::parse(&good[..51]).is_none());
    let mut long = good.clone();
    long.push(0);
    assert!(frame::parse(&long).is_none());
}

#[test]
fn relayed_request_through_the_service() {
    let owner = "user:peter#1001";
    let mut h = Holocron::new(TestSealer, TestSigner, MemStore::default(), TestEntropy::default(), owner.into(), KdfParams::DEFAULT);
    // The kernel's relayed frame: corr = relay id, principal = the CALLER's stamp.
    let relay = |prin: [u8; 32], verb: u8, body: &[u8]| {
        let mut f = frame::request(verb, 77, body).unwrap();
        f[16..48].copy_from_slice(&prin);
        f
    };
    let answer = |h: &mut Holocron<_, _, _, _>, f: &[u8]| {
        let p = frame::parse(f).unwrap();
        let r = h.handle(frame::caller(&p), p.verb, p.body, 0, 1_790_000_000);
        frame::reply(p.verb, p.corr, r.status, &r.body)
    };
    let peter = record(wire::PRIN_USER, owner.as_bytes());
    let row = record(3, b"row:4/gen:9");
    let un = Request::Unlock { create: true, password: b"pw".to_vec() };
    let r = answer(&mut h, &relay(peter, un.verb(), &un.encode_body()));
    let p = frame::parse(&r).unwrap();
    assert_eq!((p.kind, p.corr, p.status), (KIND_REPLY, 77, status::OK));
    let st = answer(&mut h, &relay(row, wire::VERB_STATUS, &[]));
    assert_eq!(frame::parse(&st).unwrap().status, status::DENIED, "x86's row stamp is never the owner");
    let st = answer(&mut h, &relay(record(wire::PRIN_USER, b"user:mallory#4242"), wire::VERB_SECRET_GET, b"\xff"));
    assert_eq!(frame::parse(&st).unwrap().status, status::DENIED, "denied before the (malformed) body is decoded");
}
