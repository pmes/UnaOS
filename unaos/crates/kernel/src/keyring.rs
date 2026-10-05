// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Holocron — shared-core
//!
//! HOLOCRON2 (rmbp-ledger B355) — the KERNEL's three small parts of the metal secrets handler. Holocron
//! itself is `APPS/HOLOCRON.ELF` (crates/user-holocron), a ring-3 fulfiller of verbs 144..=151 linking
//! `holocron_core` — the ONE implementation of the formats, the ring and the policy. Nothing here keeps a
//! secret or answers a verb:
//!
//! 1. [`kdf`] — the body of `SYS_KDF` (66): Argon2id with CRYPTOCORE's `crypto_core::argon2`, the exact call
//!    `holocron_core::cc::CryptoCore::derive_key` makes, because the ring key's memory (19..=64 MiB) does
//!    not fit the 4 MiB ring-3 window. The blocks come from the kernel heap FALLIBLY (`-ENOMEM`, never a
//!    panic) and are zeroed before they go back.
//! 2. [`after_login`] — the login path's launch: when the user's ring file exists
//!    (`<home>/.config/unaos/holocron/.ring`) and `/apps/HOLOCRON.ELF` is staged, the fulfiller starts in
//!    the new session (locked; `holocron unlock <pw>` opens it). The session's end ends it (SECLOGIN).
//! 3. [`selftest`] — `tests holocron`: `holocron_core`'s service on a RAM ring (the production suite,
//!    floor parameters): put/get round trip, a second principal DENIED before its body is decoded, a
//!    tampered file CORRUPT, lock then get refused; plus whether the user has a ring on the UnaFS root and
//!    how many Holocron verbs have a live fulfiller.
//!
//! Design: docs/dev/evidence/rmbp-1005/HOLOCRON2.md.

use alloc::string::String;
use alloc::vec::Vec;

/// Zero `b` (the compiler may not elide it).
pub fn wipe(b: &mut [u8]) {
    holocron_core::zero::wipe(b);
}

/// `SYS_KDF`'s body over the copied-in request (una-abi `kdf_parse` layout): the 32-byte key or `-errno`.
pub fn kdf(req: &[u8]) -> Result<[u8; 32], i64> {
    use crypto_core::argon2::{argon2, Block, Params, Variant, Version};
    let (m_kib, t, p, pw, salt) = una_abi::kdf_parse(req).ok_or(una_abi::EINVAL)?;
    let params = Params { variant: Variant::Argon2id, version: Version::V0x13, m_kib, t, p };
    let n = params.blocks();
    let mut mem: Vec<Block> = Vec::new();
    if mem.try_reserve_exact(n).is_err() {
        return Err(una_abi::ENOMEM);
    }
    mem.resize(n, Block::ZERO);
    let mut out = [0u8; 32];
    let r = argon2(&params, pw, salt, &[], &[], &mut mem, &mut out);
    for b in mem.iter_mut() {
        *b = Block::ZERO;
    }
    core::hint::black_box(&mem);
    match r {
        Ok(()) => Ok(out),
        Err(_) => Err(una_abi::EINVAL),
    }
}

/// The ring file's path under `home` (no trailing slash).
fn ring_path(home: &str) -> String {
    alloc::format!("{}/.config/unaos/holocron/.ring", home.trim_end_matches('/'))
}

/// `(name, uid, home)` of the open session.
#[cfg(feature = "login")]
fn session() -> Option<(String, u32, String)> {
    let mut nm = [0u8; crate::fs::users::NAME_MAX];
    let n = crate::fs::users::whoami(&mut nm)?;
    let name = &nm[..n];
    let uid = crate::fs::users::id_of(name)?;
    let mut h = [0u8; crate::fs::users::HOME_MAX];
    let hl = crate::fs::users::home_of(name, &mut h)?;
    Some((String::from(core::str::from_utf8(name).ok()?), uid, String::from(core::str::from_utf8(&h[..hl]).ok()?)))
}
#[cfg(not(feature = "login"))]
fn session() -> Option<(String, u32, String)> {
    None
}

/// `"unafs"` when `home` holds a ring file on a volume with inode ids (UnaFS), else `"none"`.
fn ring_state(home: &str) -> &'static str {
    let mt = crate::shell::vfs_mount_table();
    match mt.stat(&ring_path(home)) {
        Ok(st) if st.id.is_some() => "unafs",
        _ => "none",
    }
}

/// The login path's launch (called by `fs::users::login` once the session is open).
pub fn after_login(name: &[u8]) {
    let _ = name;
    let Some((user, _uid, home)) = session() else { return };
    if ring_state(&home) != "unafs" {
        return; // no ring: Holocron is not started (R82 — a resident only when it has something to serve)
    }
    if holocron_verbs_live() > 0 {
        return; // already running (a second login of the same boot)
    }
    let mt = crate::shell::vfs_mount_table();
    const APP: &str = "/apps/HOLOCRON.ELF";
    let bytes = match mt.stat(APP) {
        Ok(st) if st.size > 0 && st.size <= una_abi::USER_WINDOW_BYTES => mt.read(APP, 0, st.size as usize),
        _ => {
            serial_println!("[holocron] login user={} ring=unafs app=absent -> not started", user);
            return;
        }
    };
    match bytes {
        Ok(b) => match crate::arch::syscall::spawn_user_image_bg_argv(&b, &["holocron"]) {
            Ok((pid, slot, _)) => serial_println!("[holocron] login user={} ring=unafs -> started pid={} slot={}", user, pid, slot),
            Err(why) => serial_println!("[holocron] login user={} ring=unafs -> not started ({})", user, why),
        },
        Err(_) => serial_println!("[holocron] login user={} ring=unafs app=unreadable -> not started", user),
    }
}

/// Holocron verbs with a registered fulfiller right now.
fn holocron_verbs_live() -> usize {
    #[cfg(feature = "busreg")]
    {
        return (una_abi::BUS_VERB_HOLOCRON_FIRST..=una_abi::BUS_VERB_HOLOCRON_LAST).filter(|&v| crate::bus_route::registered(v)).count();
    }
    #[cfg(not(feature = "busreg"))]
    0
}

/// `tests holocron`.
pub fn selftest() {
    use holocron_core::cc::{CryptoCore, DrbgEntropy};
    use holocron_core::seal::KdfParams;
    use holocron_core::service::{Holocron, MemStore};
    use holocron_core::wire::{status, Request};

    let (owner, ring) = match session() {
        Some((n, uid, home)) => (holocron_core::wire::user_principal(&n, uid), ring_state(&home)),
        None => (String::from("user:fixture#1000"), "none"),
    };
    let verbs = holocron_verbs_live();
    let Ok(rng) = DrbgEntropy::new(crate::rand::KernelEntropy, b"holocron2 tests holocron") else {
        serial_println!(":: HOLOCRON: ring={} verbs={} owner={} entropy=refused -> FAIL ::", ring, verbs, owner);
        return;
    };
    let t0 = crate::arch::ms();
    let mut h = Holocron::new(CryptoCore, CryptoCore, MemStore::default(), rng, owner.clone(), KdfParams::FLOOR);
    let me = Some(owner.as_str());
    let call = |h: &mut Holocron<_, _, _, _>, who: Option<&str>, r: &Request| h.handle(who, r.verb(), &r.encode_body(), crate::arch::ms(), 0);
    let key: &[u8] = b"sk-ant-holocron2-fixture";
    let created = call(&mut h, me, &Request::Unlock { create: true, password: b"holocron2-fixture-pw".to_vec() }).status;
    let put = call(&mut h, me, &Request::Put { ns: "vein".into(), name: "claude.api_key".into(), kind: "api-key".into(), label: "Claude".into(), data: key.to_vec() }).status;
    let got = call(&mut h, me, &Request::Get { ns: "vein".into(), name: "claude.api_key".into() });
    let get_ok = got.status == status::OK && got.body.as_slice() == key;
    // A second principal: DENIED — and before its body is decoded (a malformed body answers DENIED, not INVALID).
    let mallory = Some("user:mallory#4242");
    let d1 = call(&mut h, mallory, &Request::Get { ns: "vein".into(), name: "claude.api_key".into() }).status;
    let d2 = h.handle(mallory, holocron_core::wire::VERB_SECRET_GET, b"\xff\xff", 0, 0).status;
    let denied = (d1 == status::DENIED && d2 == status::DENIED) as u32;
    // A tampered file: one byte of the sealed body flipped in the store.
    if let Some((f, _)) = h.store_mut().files.get_mut(&(String::from("vein"), String::from("claude.api_key"))) {
        let i = f.len() - 1;
        f[i] ^= 0x01;
    }
    let c = call(&mut h, me, &Request::Get { ns: "vein".into(), name: "claude.api_key".into() }).status;
    let corrupt = (c == status::CORRUPT) as u32;
    // Lock, then get: refused (LOCKED) — the key is gone from memory. Restore the byte first so the refusal
    // is the lock's, not the tamper's.
    if let Some((f, _)) = h.store_mut().files.get_mut(&(String::from("vein"), String::from("claude.api_key"))) {
        let i = f.len() - 1;
        f[i] ^= 0x01;
    }
    let lock = call(&mut h, me, &Request::Lock).status;
    let after = call(&mut h, me, &Request::Get { ns: "vein".into(), name: "claude.api_key".into() }).status;
    serial_println!(
        "[holocron] fx kdf=argon2id m={}KiB t={} create={} lock={} lock-then-get={} ms={}",
        KdfParams::FLOOR.m_kib, KdfParams::FLOOR.t, created, lock, after, crate::arch::ms().saturating_sub(t0)
    );
    let ok = |s: i32| if s == status::OK { "ok" } else { "FAIL" };
    let pass = created == status::OK && put == status::OK && get_ok && denied == 1 && corrupt == 1 && lock == status::OK && after == status::LOCKED;
    let user = owner.strip_prefix("user:").unwrap_or(&owner);
    serial_println!(
        ":: HOLOCRON: ring={} verbs={} owner={} put={} get={} denied={} corrupt={} -> {} ::",
        ring, verbs, user, ok(put), if get_ok { "ok" } else { "FAIL" }, denied, corrupt, if pass { "PASS" } else { "FAIL" }
    );
}
