// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! M3: the SSH agent shape — byte layouts written from draft-ietf-sshm-ssh-agent §3/§4 and RFC 8709
//! §4/§6 (big-endian uint32 lengths, RFC 4251 §5 `string`), then the policy through the agent.

use holocron_core::agent::{self, AgentRequest};
use holocron_core::seal::{KdfParams, Signer};
use holocron_core::service::{Holocron, MemStore};
use holocron_core::testseal::{TestEntropy, TestSealer, TestSigner};
use holocron_core::wire::{Request, status};

const OWNER: &str = "user:peter#1001";
type H = Holocron<TestSealer, TestSigner, MemStore, TestEntropy>;

fn be(n: u32) -> [u8; 4] {
    n.to_be_bytes()
}
fn sstr(v: &mut Vec<u8>, s: &[u8]) {
    v.extend_from_slice(&be(s.len() as u32));
    v.extend_from_slice(s);
}

#[test]
fn kat_blobs_and_frames() {
    let a: [u8; 32] = core::array::from_fn(|i| i as u8);
    // RFC 8709 §4: string "ssh-ed25519" || string A — 4+11+4+32 = 51 bytes.
    let blob = agent::ed25519_blob(&a);
    let mut want = vec![0, 0, 0, 11];
    want.extend_from_slice(b"ssh-ed25519");
    want.extend_from_slice(&[0, 0, 0, 32]);
    want.extend_from_slice(&a);
    assert_eq!(blob, want);
    assert_eq!(agent::parse_ed25519_blob(&blob), Some(a));
    assert_eq!(agent::parse_ed25519_blob(&blob[..50]), None);
    let mut rsa = blob.clone();
    rsa[4..11].copy_from_slice(b"ssh-rsa");
    assert_eq!(agent::parse_ed25519_blob(&rsa), None);
    // RFC 8709 §6: string "ssh-ed25519" || string sig(64) — 83 bytes.
    let sig = [7u8; 64];
    let sb = agent::ed25519_signature(&sig);
    assert_eq!((sb.len(), &sb[..4], &sb[15..19]), (83, &[0u8, 0, 0, 11][..], &[0u8, 0, 0, 64][..]));
    // §3: uint32 length (of type + contents) || type || contents.
    assert_eq!(agent::frame(&[agent::FAILURE]), [0, 0, 0, 1, 5]);
    // §4.4 IDENTITIES_ANSWER: byte 12, uint32 nkeys, then (string blob, string comment)*.
    let ans = agent::identities_answer(&[(blob.clone(), "c".into())]);
    let mut w = vec![12, 0, 0, 0, 1];
    sstr(&mut w, &blob);
    sstr(&mut w, b"c");
    assert_eq!(ans, w);
    // §4.5 SIGN_RESPONSE: byte 14, string signature-blob.
    let mut w = vec![14];
    sstr(&mut w, &sb);
    assert_eq!(agent::sign_response(&sig), w);
}

#[test]
fn parse_requests() {
    assert_eq!(agent::parse_request(&[11]), AgentRequest::RequestIdentities);
    assert_eq!(agent::parse_request(&[11, 0]), AgentRequest::Unsupported(11), "trailing byte");
    let mut m = vec![13];
    sstr(&mut m, b"BLOB");
    sstr(&mut m, b"data");
    m.extend_from_slice(&be(4)); // SSH_AGENT_RSA_SHA2_512 flag: carried, ignored for Ed25519
    assert_eq!(agent::parse_request(&m), AgentRequest::Sign { blob: b"BLOB", data: b"data", flags: 4 });
    assert_eq!(agent::parse_request(&m[..m.len() - 1]), AgentRequest::Unsupported(13));
    let mut l = vec![22];
    sstr(&mut l, b"pw");
    assert_eq!(agent::parse_request(&l), AgentRequest::Lock { passphrase: b"pw" });
    l[0] = 23;
    assert_eq!(agent::parse_request(&l), AgentRequest::Unlock { passphrase: b"pw" });
    // A huge declared string length is refused, not allocated.
    assert_eq!(agent::parse_request(&[13, 0xff, 0xff, 0xff, 0xff]), AgentRequest::Unsupported(13));
    for ty in [17u8, 18, 19, 25, 27, 99] {
        assert_eq!(agent::parse_request(&[ty]), AgentRequest::Unsupported(ty));
    }
    assert_eq!(agent::parse_request(&[]), AgentRequest::Unsupported(0));
}

fn svc_with_key() -> (H, [u8; 32]) {
    let mut h: H = Holocron::new(TestSealer, TestSigner, MemStore::default(), TestEntropy::default(), OWNER.into(), KdfParams::DEFAULT);
    let call = |h: &mut H, r: Request| h.handle(Some(OWNER), r.verb(), &r.encode_body(), 0, 0);
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: b"pw".to_vec() }).status, status::OK);
    let seed = [0x42u8; 32];
    let put = Request::Put { ns: "ssh".into(), name: "id_ed25519".into(), kind: "ssh-ed25519".into(), label: "peter@rmbp".into(), data: seed.to_vec() };
    assert_eq!(call(&mut h, put).status, status::OK);
    // A generated key (empty data): Holocron mints the seed from its entropy.
    let minted = Request::Put { ns: "ssh".into(), name: "gen".into(), kind: "ssh-ed25519".into(), label: "minted".into(), data: vec![] };
    assert_eq!(call(&mut h, minted).status, status::OK);
    (h, TestSigner.public_key(&seed))
}

fn sign_req(public: &[u8; 32], data: &[u8]) -> Vec<u8> {
    let mut m = vec![agent::SIGN_REQUEST];
    sstr(&mut m, &agent::ed25519_blob(public));
    sstr(&mut m, data);
    m.extend_from_slice(&be(0));
    m
}

#[test]
fn identities_and_sign_through_the_agent() {
    let (mut h, public) = svc_with_key();
    let ans = agent::handle(&mut h, Some(OWNER), &[agent::REQUEST_IDENTITIES], 0, 0);
    assert_eq!(&ans[..5], &[12, 0, 0, 0, 2]);
    // The comment of a test-suite key says so.
    let needle = b"peter@rmbp [TEST-INSECURE]";
    assert!(ans.windows(needle.len()).any(|w| w == needle));
    assert!(ans.windows(51).any(|w| w == agent::ed25519_blob(&public).as_slice()));
    // SIGN_REQUEST over the session-hash-shaped data -> a signature the signer verifies.
    let data = b"SSH userauth publickey payload";
    let rsp = agent::handle(&mut h, Some(OWNER), &sign_req(&public, data), 0, 0);
    assert_eq!((rsp[0], rsp.len()), (agent::SIGN_RESPONSE, 1 + 4 + 83));
    let sig: [u8; 64] = rsp[1 + 4 + 19..].try_into().unwrap();
    assert!(h.verify(&public, data, &sig));
    // The bus Sign verb produces the same bytes (one signer, two transports).
    let r = h.handle(Some(OWNER), 150, &Request::Sign { key: "id_ed25519".into(), data: data.to_vec() }.encode_body(), 0, 0);
    assert_eq!(r.body, sig);
    // An unknown key, a stranger, a locked ring: FAILURE (and locked lists no identities).
    assert_eq!(agent::handle(&mut h, Some(OWNER), &sign_req(&[0; 32], data), 0, 0), [agent::FAILURE]);
    assert_eq!(agent::handle(&mut h, Some("user:mallory#1002"), &[11], 0, 0), [agent::FAILURE]);
    assert_eq!(agent::handle(&mut h, Some("user:mallory#1002"), &sign_req(&public, data), 0, 0), [agent::FAILURE]);
    // ADD_IDENTITY is refused: keys enter only through SecretPut.
    assert_eq!(agent::handle(&mut h, Some(OWNER), &[17, 0, 0, 0, 0], 0, 0), [agent::FAILURE]);
    // LOCK through the agent locks the ring; identities go empty; UNLOCK with the ring password reopens.
    let mut lock = vec![agent::LOCK];
    sstr(&mut lock, b"ignored");
    assert_eq!(agent::handle(&mut h, Some(OWNER), &lock, 0, 0), [agent::SUCCESS]);
    assert!(!h.is_unlocked());
    assert_eq!(agent::handle(&mut h, Some(OWNER), &[11], 0, 0), [12, 0, 0, 0, 0]);
    assert_eq!(agent::handle(&mut h, Some(OWNER), &sign_req(&public, data), 0, 0), [agent::FAILURE]);
    let mut unlock = vec![agent::UNLOCK];
    sstr(&mut unlock, b"wrong");
    assert_eq!(agent::handle(&mut h, Some(OWNER), &unlock, 0, 0), [agent::FAILURE]);
    let mut unlock = vec![agent::UNLOCK];
    sstr(&mut unlock, b"pw");
    assert_eq!(agent::handle(&mut h, Some(OWNER), &unlock, 0, 0), [agent::SUCCESS]);
    assert_eq!(&agent::handle(&mut h, Some(OWNER), &[11], 0, 0)[..5], &[12, 0, 0, 0, 2]);
}
