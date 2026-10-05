// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! M2: the bus surface — body KATs written from the verb table in `src/wire.rs`, the owner-only policy,
//! the rate-limited unlock, and the status every verb answers in every ring state.

use holocron_core::seal::KdfParams;
use holocron_core::service::{Holocron, MemStore, UnlockLimiter};
use holocron_core::testseal::{TestEntropy, TestSealer, TestSigner};
use holocron_core::wire::{self, Reply, Request, RingState, status};

const OWNER: &str = "user:peter#1001";
type H = Holocron<TestSealer, TestSigner, MemStore, TestEntropy>;

fn svc() -> H {
    Holocron::new(TestSealer, TestSigner, MemStore::default(), TestEntropy::default(), OWNER.into(), KdfParams::DEFAULT)
}
fn as_(h: &mut H, who: Option<&str>, r: Request, now_ms: u64) -> Reply {
    h.handle(who, r.verb(), &r.encode_body(), now_ms, 1_790_000_000)
}
fn call(h: &mut H, r: Request) -> Reply {
    as_(h, Some(OWNER), r, 0)
}
fn get() -> Request {
    Request::Get { ns: "vein".into(), name: "claude.api_key".into() }
}
fn put(data: &[u8]) -> Request {
    Request::Put { ns: "vein".into(), name: "claude.api_key".into(), kind: "api-key".into(), label: "Claude".into(), data: data.to_vec() }
}
fn unlock(pw: &[u8]) -> Request {
    Request::Unlock { create: false, password: pw.to_vec() }
}

// ---- body KATs -------------------------------------------------------------------------------------

#[test]
fn kat_bodies_field_by_field() {
    // SecretGet 144: str8 "vein", str8 "claude.api_key".
    let mut want = vec![4];
    want.extend_from_slice(b"vein");
    want.push(14);
    want.extend_from_slice(b"claude.api_key");
    assert_eq!(get().encode_body(), want);
    assert_eq!(get().verb(), 144);
    // SecretPut 145: ..., str8 kind, str8 label, bytes16 data (LE length).
    let mut want_put = want.clone();
    want_put.push(7);
    want_put.extend_from_slice(b"api-key");
    want_put.push(6);
    want_put.extend_from_slice(b"Claude");
    want_put.extend_from_slice(&[3, 0]);
    want_put.extend_from_slice(b"abc");
    assert_eq!(put(b"abc").encode_body(), want_put);
    // Unlock 148: u8 flags, bytes16 password.
    assert_eq!(Request::Unlock { create: true, password: b"pw".to_vec() }.encode_body(), [1, 2, 0, b'p', b'w']);
    // Sign 150: str8 key, bytes16 data.
    assert_eq!(Request::Sign { key: "k".into(), data: vec![9; 2] }.encode_body(), [1, b'k', 2, 0, 9, 9]);
    // Lock/Status: empty. List: str8 ns.
    assert!(Request::Lock.encode_body().is_empty());
    assert_eq!(Request::List { ns: "ssh".into() }.encode_body(), [3, b's', b's', b'h']);
    assert_eq!(wire::VERBS, [144, 145, 146, 147, 148, 149, 150, 151]);
    // Every request decodes back to itself.
    for r in [get(), put(b"abc"), unlock(b"x"), Request::Lock, Request::Status, Request::List { ns: "a".into() },
              Request::Delete { ns: "a".into(), name: "b".into() }, Request::Sign { key: "k".into(), data: vec![1] }] {
        assert_eq!(Request::decode(r.verb(), &r.encode_body()).unwrap(), r);
    }
}

#[test]
fn decode_refusals() {
    let g = get().encode_body();
    let mut trailing = g.clone();
    trailing.push(0);
    assert_eq!(Request::decode(144, &trailing), Err(status::INVALID));
    assert_eq!(Request::decode(144, &g[..g.len() - 1]), Err(status::INVALID));
    assert_eq!(Request::decode(148, &[2, 1, 0, b'x']), Err(status::INVALID), "unknown flag bit");
    assert_eq!(Request::decode(144, &[2, 0xff, 0xfe, 1, b'a']), Err(status::INVALID), "non-UTF-8 ns");
    assert_eq!(Request::decode(152, &[]), Err(status::NO_VERB));
    assert_eq!(Request::decode(149, &[0]), Err(status::INVALID), "Lock carries no body");
    assert_eq!(Request::decode(145, &vec![0u8; wire::BODY_MAX + 1]), Err(status::INVALID));
}

#[test]
fn reply_bodies() {
    let st = wire::encode_status(RingState::Locked, 0xFE, OWNER);
    assert_eq!(st[..3], [1, 0xFE, 15]);
    assert_eq!(wire::decode_status(&st), Some((RingState::Locked, 0xFE, OWNER.into())));
    assert_eq!(wire::decode_status(&[3, 0, 0]), None);
}

// ---- principals ------------------------------------------------------------------------------------

#[test]
fn principal_records() {
    let mut rec = [0u8; wire::PRIN_RECORD_LEN];
    rec[0] = wire::PRIN_USER;
    rec[1] = OWNER.len() as u8;
    rec[2..2 + OWNER.len()].copy_from_slice(OWNER.as_bytes());
    assert_eq!(wire::principal_from_record(&rec), Some(OWNER));
    let mut other_kind = rec;
    other_kind[0] = 1;
    assert_eq!(wire::principal_from_record(&other_kind), None, "only PRIN_USER projects");
    let mut not_user = rec;
    not_user[2..7].copy_from_slice(b"row:0");
    assert_eq!(wire::principal_from_record(&not_user), None);
    let mut long = rec;
    long[1] = 31;
    assert_eq!(wire::principal_from_record(&long), None);
    assert_eq!(wire::principal_from_record(&rec[..31]), None);
    assert_eq!(wire::user_principal("peter", 1001), OWNER);
}

// ---- policy ----------------------------------------------------------------------------------------

#[test]
fn owner_only_before_decode() {
    let mut h = svc();
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: b"pw".to_vec() }).status, status::OK);
    assert_eq!(call(&mut h, put(b"sk")).status, status::OK);
    for who in [None, Some("user:mallory#1002"), Some("user:peter#1002"), Some("user:peter#1001 "), Some("")] {
        for r in [get(), put(b"x"), Request::Lock, Request::Status, unlock(b"pw"), Request::List { ns: "vein".into() }] {
            let rep = as_(&mut h, who, r, 0);
            assert_eq!((rep.status, rep.body.len()), (status::DENIED, 0), "{who:?}");
        }
        // A malformed body from a stranger is DENIED, not INVALID: nothing is parsed for them.
        assert_eq!(h.handle(who, 144, &[0xff], 0, 0).status, status::DENIED);
    }
    // The stranger's Lock did nothing.
    assert!(h.is_unlocked());
    assert_eq!(call(&mut h, get()).body, b"sk");
}

#[test]
fn ring_of_another_owner_is_denied() {
    let mut store = MemStore::default();
    let mut other = holocron_core::ring::Ring::new(TestSealer);
    store.ring = Some(other.create("user:mallory#1002", b"pw", KdfParams::DEFAULT, &mut TestEntropy::default()).unwrap());
    let mut h = Holocron::new(TestSealer, TestSigner, store, TestEntropy::default(), OWNER.into(), KdfParams::DEFAULT);
    assert_eq!(call(&mut h, unlock(b"pw")).status, status::DENIED);
    assert_eq!(call(&mut h, Request::Status).status, status::DENIED);
}

#[test]
fn states_and_statuses() {
    let mut h = svc();
    // No ring.
    let st = call(&mut h, Request::Status);
    assert_eq!(wire::decode_status(&st.body).unwrap().0, RingState::NoRing);
    assert_eq!(call(&mut h, get()).status, status::NOT_FOUND);
    assert_eq!(call(&mut h, put(b"x")).status, status::NO_RING);
    assert_eq!(call(&mut h, unlock(b"pw")).status, status::NO_RING);
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: vec![] }).status, status::INVALID);
    // Create -> unlocked.
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: b"pw".to_vec() }).status, status::OK);
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: b"pw".to_vec() }).status, status::EXISTS);
    assert_eq!(wire::decode_status(&call(&mut h, Request::Status).body).unwrap().0, RingState::Unlocked);
    assert_eq!(call(&mut h, put(b"sk")).status, status::OK);
    // Lock: an existing secret is LOCKED, an absent one NOT_FOUND (the only fallback answer).
    assert_eq!(call(&mut h, Request::Lock).status, status::OK);
    assert_eq!(wire::decode_status(&call(&mut h, Request::Status).body).unwrap().0, RingState::Locked);
    assert_eq!(call(&mut h, get()).status, status::LOCKED);
    assert_eq!(call(&mut h, Request::Get { ns: "vein".into(), name: "nope".into() }).status, status::NOT_FOUND);
    assert_eq!(call(&mut h, put(b"x")).status, status::LOCKED);
    assert_eq!(call(&mut h, Request::Delete { ns: "vein".into(), name: "claude.api_key".into() }).status, status::LOCKED);
    assert_eq!(call(&mut h, Request::Sign { key: "k".into(), data: vec![] }).status, status::LOCKED);
    // List works locked (metadata is the owner's, in the clear header).
    let l = wire::decode_list(&call(&mut h, Request::List { ns: "vein".into() }).body).unwrap();
    assert_eq!((l.len(), l[0].meta.kind.as_str(), l[0].meta.created), (1, "api-key", 1_790_000_000));
    // Unlock and read.
    assert_eq!(call(&mut h, unlock(b"pw")).status, status::OK);
    assert_eq!(call(&mut h, get()).body, b"sk");
    // Validation.
    assert_eq!(call(&mut h, Request::Get { ns: "..".into(), name: "x".into() }).status, status::INVALID);
    assert_eq!(call(&mut h, put(&vec![0; holocron_core::format::SECRET_MAX + 1])).status, status::TOO_BIG);
    let wrong_kind = Request::Put { ns: "ssh".into(), name: "id".into(), kind: "api-key".into(), label: String::new(), data: vec![] };
    assert_eq!(call(&mut h, wrong_kind).status, status::INVALID);
    let short_seed = Request::Put { ns: "ssh".into(), name: "id".into(), kind: "ssh-ed25519".into(), label: String::new(), data: vec![1; 31] };
    assert_eq!(call(&mut h, short_seed).status, status::INVALID);
    // Delete.
    assert_eq!(call(&mut h, Request::Delete { ns: "vein".into(), name: "claude.api_key".into() }).status, status::OK);
    assert_eq!(call(&mut h, Request::Delete { ns: "vein".into(), name: "claude.api_key".into() }).status, status::NOT_FOUND);
    // A tampered stored file is CORRUPT, never NOT_FOUND (no silent fallback past a tamper).
    assert_eq!(call(&mut h, put(b"sk2")).status, status::OK);
    let f = h.store_mut().files.get_mut(&("vein".into(), "claude.api_key".into())).unwrap();
    let last = f.0.len() - 1;
    f.0[last] ^= 1;
    assert_eq!(call(&mut h, get()).status, status::CORRUPT);
}

// ---- rate limit ------------------------------------------------------------------------------------

#[test]
fn unlock_rate_limit() {
    let mut h = svc();
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: b"pw".to_vec() }).status, status::OK);
    call(&mut h, Request::Lock);
    let t0 = 10_000u64;
    // FREE failures cost nothing.
    for i in 0..UnlockLimiter::FREE as u64 {
        assert_eq!(as_(&mut h, Some(OWNER), unlock(b"bad"), t0 + i).status, status::BAD_PASSWORD);
    }
    // The next failure arms a 1 s wait.
    assert_eq!(as_(&mut h, Some(OWNER), unlock(b"bad"), t0 + 10).status, status::BAD_PASSWORD);
    // Inside the wait even the RIGHT password is refused without running the KDF.
    assert_eq!(as_(&mut h, Some(OWNER), unlock(b"pw"), t0 + 500).status, status::RATE_LIMITED);
    assert!(!h.is_unlocked());
    // After it: one more failure doubles the wait to 2 s.
    assert_eq!(as_(&mut h, Some(OWNER), unlock(b"bad"), t0 + 1010).status, status::BAD_PASSWORD);
    assert_eq!(as_(&mut h, Some(OWNER), unlock(b"pw"), t0 + 1010 + 1999).status, status::RATE_LIMITED);
    assert_eq!(as_(&mut h, Some(OWNER), unlock(b"pw"), t0 + 1010 + 2000).status, status::OK);
    // Success resets.
    assert_eq!(h.limiter().failures(), 0);
    // A stranger's attempts never touch the owner's limiter.
    call(&mut h, Request::Lock);
    for _ in 0..50 {
        assert_eq!(as_(&mut h, Some("user:mallory#1002"), unlock(b"bad"), t0).status, status::DENIED);
    }
    assert_eq!(h.limiter().failures(), 0);
}

#[test]
fn limiter_schedule_caps() {
    let mut l = UnlockLimiter::default();
    let mut waits = vec![];
    for _ in 0..16 {
        l.fail(0);
        waits.push(l.check(0).err().unwrap_or(0));
    }
    assert_eq!(&waits[..6], &[0, 0, 0, 1_000, 2_000, 4_000]);
    assert_eq!(*waits.last().unwrap(), UnlockLimiter::CAP_MS);
}
