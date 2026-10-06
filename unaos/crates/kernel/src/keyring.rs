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
//!    `holocron_core::cc::CryptoCore::derive_key` makes. HOLOCRON2 needed it because the ring key's memory
//!    (19..=64 MiB) did not fit the 4 MiB ring-3 window; WINDOW2 (B361, R85) raised the window to 64 MiB and
//!    HOLOCRON.ELF derives in ring 3 again — SYS_KDF STAYS as a kernel service (and `tests window`'s
//!    reference computation), used by Holocron only to unlock a ring whose recorded memory the window cannot
//!    hold. The blocks come from the kernel heap FALLIBLY (`-ENOMEM`, never a panic) and are zeroed before
//!    they go back.
//! 2. [`after_login`] — the login path's launch: when the user's ring file exists
//!    (`<home>/.holocron/.ring`, `holocron_core::root`; a legacy root counts — B448) and `/apps/HOLOCRON.ELF` is staged, the fulfiller starts in
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

/// The ring file's path under `home`: THE root, named once in `holocron_core::root` (HOLOCRONROOT, B448 —
/// this file used to spell `<home>/.config/unaos/holocron` while the host used `<home>/.holocron`).
fn ring_path(home: &str) -> String {
    holocron_core::root::ring_path(home)
}

/// Where `home`'s ring is: `".holocron"` (THE root), `"legacy"` (only under a `holocron_core::root::LEGACY`
/// root — HOLOCRON.ELF moves it on its next start), or `"none"`.
fn ring_root(home: &str) -> &'static str {
    let mt = crate::shell::vfs_mount_table();
    let has = |p: &str| matches!(mt.stat(p), Ok(st) if st.id.is_some());
    if has(&ring_path(home)) {
        return holocron_core::root::DIR;
    }
    if holocron_core::root::legacy_roots(home).iter().any(|r| has(&alloc::format!("{}/{}", r, holocron_core::root::RING))) {
        return "legacy";
    }
    "none"
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

/// `"unafs"` when `home` holds a ring file on a volume with inode ids (UnaFS) — at THE root or a legacy one,
/// so the fulfiller starts and moves a legacy ring (B448) — else `"none"`.
fn ring_state(home: &str) -> &'static str {
    if ring_root(home) == "none" { "none" } else { "unafs" }
}

/// The login path's launch (called by `fs::users::login` once the session is open).
pub fn after_login(name: &[u8]) {
    let _ = name;
    let Some((user, _uid, home)) = session() else { return };
    if ring_state(&home) != "unafs" && !door_create_pending() {
        return; // no ring (and no create waiting in the RINGLOGIN door): Holocron is not started (R82 — a resident only when it has something to serve)
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
            Ok((pid, slot, _)) => { crate::video::wm::app_name_arm_launch(crate::video::wm::owner_of_launch(slot as u64), APP); serial_println!("[holocron] login user={} ring=unafs -> started pid={} slot={}", user, pid, slot) } // SMALLFIX4 item 11: armed by APPRES
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

    let (owner, ring, root) = match session() {
        Some((n, uid, home)) => (holocron_core::wire::user_principal(&n, uid), ring_state(&home), ring_root(&home)),
        None => (String::from("user:fixture#1000"), "none", "none"),
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
        ":: HOLOCRON: ring={} root={} verbs={} owner={} put={} get={} denied={} corrupt={} -> {} ::",
        ring, root, verbs, user, ok(put), if get_ok { "ok" } else { "FAIL" }, denied, corrupt, if pass { "PASS" } else { "FAIL" }
    );
}

// =================================================================================================
// RINGLOGIN (rmbp-ledger B465) — the ring opens WITH THE LOGIN. TAIL-APPENDED.
//
// `fs::users::login` (the one path holding a verified password; the `login-submit` worker core, never the
// render core) calls [`ring_login`]: the ring HEADER is read (salt + parameters; `holocron_core::format`),
// the ring key is derived ONCE with [`kdf`] (SYS_KDF's body — the Argon2id call `CryptoCore::derive_key`
// makes), and only that 32-byte output is posted in the DOOR, a take-once slot. No ring yet: a fresh salt
// from the kernel DRBG and una-abi's WINDOW2 parameters (= HOLOCRON.ELF's `METAL_KDF`), mode create — the
// first user's ring is made at FIRSTUSER's setup, and a user from before the fold gets it at the next login.
// HOLOCRON.ELF (the registered fulfiller, as the door's user) takes it through SYS_RINGKEY (67) and reports
// back; [`door_report`] says the witness. The lock screen posts a LOCK ([`door_lock`]); Log Out wipes the
// door ([`door_logout`]) and SECLOGIN M3 ends HOLOCRON.ELF with the session. The password is never kept,
// never printed; the salt and key never reach the wire. Design: docs/dev/evidence/rmbp-1005/ringlogin.md.
// =================================================================================================

/// The door: empty, a posted key (the encoded `una_abi::RingDoor`) for `uid`, or a lock for `uid`.
enum Door {
    Empty,
    Key { uid: u32, door: [u8; una_abi::RINGKEY_LEN] },
    Lock { uid: u32 },
}

static DOOR: crate::sync::Mutex<Door> = crate::sync::Mutex::new(Door::Empty);
/// Doors posted / taken this boot.
static RL_POSTED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static RL_TAKEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// The last report: status (`i32::MIN` = none yet) and mode (una-abi `RINGKEY_MODE_*`).
static RL_STATUS: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(i32::MIN);
static RL_MODE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// The posted door's mode and login clock (for the report's `ms=`).
static RL_POST_MODE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
static RL_T0: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn wipe_door(d: &mut Door) {
    if let Door::Key { door, .. } = d {
        wipe(door);
    }
    *d = Door::Empty;
}

/// The ring file bytes for `home` — THE root first, then a legacy root (HOLOCRON.ELF moves it before it takes
/// the door; the salt does not change) — `Ok(None)` when the user has no ring.
fn ring_bytes(home: &str) -> Result<Option<Vec<u8>>, &'static str> {
    let mt = crate::shell::vfs_mount_table();
    let mut paths = alloc::vec![ring_path(home)];
    for r in holocron_core::root::legacy_roots(home) {
        paths.push(alloc::format!("{}/{}", r, holocron_core::root::RING));
    }
    for p in paths {
        match mt.stat(&p) {
            Ok(st) if st.id.is_some() && st.size > 0 && st.size <= 4096 => return mt.read(&p, 0, st.size as usize).map(Some).map_err(|_| "ring-unreadable"),
            Ok(st) if st.id.is_some() => return Err("ring-size"),
            _ => {}
        }
    }
    Ok(None)
}

/// The login's half (called by `fs::users::login` once the session is open, and by the lock screen's unlock
/// once the password verified). Derives the ring key and posts it; prints one line; never the password.
pub fn ring_login(name: &[u8], password: &[u8]) {
    #[cfg(all(feature = "lumen", feature = "login"))]
    {
        let t0 = crate::arch::ms();
        let Some(uid) = crate::fs::users::id_of(name) else { return };
        let mut h = [0u8; crate::fs::users::HOME_MAX];
        let Some(hl) = crate::fs::users::home_of(name, &mut h) else { return };
        let Ok(home) = core::str::from_utf8(&h[..hl]) else { return };
        let mt = crate::shell::vfs_mount_table();
        if !matches!(mt.stat(home), Ok(st) if st.id.is_some()) {
            serial_println!("[holocron] door none reason=home-not-unafs (the ring lives on UnaFS; `holocron unlock` stays)");
            return;
        }
        let (mode, salt, m, t, p) = match ring_bytes(home) {
            Ok(Some(b)) => match holocron_core::format::parse_ring(&b) {
                Ok((hdr, _, _)) => (una_abi::RINGKEY_MODE_OPEN, hdr.salt, hdr.kdf.m_kib, hdr.kdf.t, hdr.kdf.p),
                Err(_) => {
                    serial_println!("[holocron] door none reason=ring-unparsed (Holocron says CORRUPT on its own read)");
                    return;
                }
            },
            Ok(None) => {
                let mut s = [0u8; 16];
                crate::rand::drbg_fill(&mut s);
                (una_abi::RINGKEY_MODE_CREATE, s, una_abi::RING_KDF_M_KIB, una_abi::RING_KDF_T, una_abi::RING_KDF_P) // HOLOCRONARM (B484): the per-arch create parameters (x86 = WINDOW2's; aarch64 19 MiB, its heap is 48)
            }
            Err(why) => {
                serial_println!("[holocron] door none reason={}", why);
                return;
            }
        };
        let mut req = alloc::vec![0u8; una_abi::KDF_HDR_LEN + password.len() + salt.len()];
        let Some(n) = una_abi::kdf_request(m, t, p, password, &salt, &mut req) else {
            wipe(&mut req);
            serial_println!("[holocron] door none reason=password-length");
            return;
        };
        let r = kdf(&req[..n]);
        wipe(&mut req);
        let mut key = match r {
            Ok(k) => k,
            Err(e) => {
                serial_println!("[holocron] door none reason={} m_kib={} t={} p={}", if e == una_abi::ENOMEM { "kdf-enomem" } else { "kdf-refused" }, m, t, p);
                return;
            }
        };
        let mut door = [0u8; una_abi::RINGKEY_LEN];
        una_abi::ringdoor_encode(&una_abi::RingDoor { mode, m_kib: m, t, p, salt, key, t0_ms: t0, old_key: [0; 32] }, &mut door);
        wipe(&mut key);
        {
            let mut d = DOOR.lock();
            wipe_door(&mut d);
            *d = Door::Key { uid, door };
        }
        wipe(&mut door);
        RL_POSTED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        RL_POST_MODE.store(mode, core::sync::atomic::Ordering::Relaxed);
        RL_T0.store(t0, core::sync::atomic::Ordering::Relaxed);
        serial_println!(
            "[holocron] door kdf=kernel mode={} m_kib={} t={} p={} ms={} -> posted (the password is dropped here; HOLOCRON.ELF takes the key)",
            if mode == una_abi::RINGKEY_MODE_CREATE { "create" } else { "open" }, m, t, p, crate::arch::ms().saturating_sub(t0)
        );
    }
    #[cfg(not(all(feature = "lumen", feature = "login")))]
    let _ = (name, password);
}

/// A create is waiting in the door (the login launch starts HOLOCRON.ELF for it though no ring exists yet).
pub fn door_create_pending() -> bool {
    matches!(&*DOOR.lock(), Door::Key { door, .. } if door[0] == una_abi::RINGKEY_MODE_CREATE)
}

/// The lock screen: post a LOCK for the session user (any posted key is wiped first).
pub fn door_lock(reason: &str) {
    #[cfg(all(feature = "lumen", feature = "login"))]
    {
        let mut nm = [0u8; crate::fs::users::NAME_MAX];
        let Some(n) = crate::fs::users::whoami(&mut nm) else { return };
        let Some(uid) = crate::fs::users::id_of(&nm[..n]) else { return };
        let mut d = DOOR.lock();
        wipe_door(&mut d);
        *d = Door::Lock { uid };
        drop(d);
        serial_println!("[holocron] door lock reason={} -> posted", reason);
    }
    #[cfg(not(all(feature = "lumen", feature = "login")))]
    let _ = reason;
}

/// Log Out: the door is wiped (HOLOCRON.ELF itself is ended with the session's programs, SECLOGIN M3).
pub fn door_logout() {
    wipe_door(&mut DOOR.lock());
}

/// SYS_RINGKEY's TAKE: `holder` = the caller is Holocron's registered fulfiller; `uid` its live session user.
pub fn door_take(holder: bool, uid: u32, out: &mut [u8; una_abi::RINGKEY_LEN]) -> i64 {
    if !holder || uid == 0 {
        return una_abi::EACCES;
    }
    let mut d = DOOR.lock();
    match &*d {
        Door::Empty => una_abi::RINGKEY_NONE,
        Door::Key { uid: u, door } if *u == uid => {
            out.copy_from_slice(door);
            wipe_door(&mut d);
            RL_TAKEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            una_abi::RINGKEY_KEY
        }
        Door::Lock { uid: u } if *u == uid => {
            *d = Door::Empty;
            una_abi::RINGKEY_LOCK
        }
        _ => una_abi::EACCES, // another user's door: not this process's
    }
}

/// SYS_RINGKEY's REPORT: Holocron's answer to the door it took. Says the witness.
pub fn door_report(holder: bool, uid: u32, status: i32, mode: u8) -> i64 {
    if !holder || uid == 0 {
        return una_abi::EACCES;
    }
    if mode == una_abi::RINGKEY_MODE_REKEY {
        return rekey_report(status); // RINGLOGIN2 (B479): a password change's answer — its own witness, not the login's
    }
    RL_STATUS.store(status, core::sync::atomic::Ordering::Relaxed);
    RL_MODE.store(mode, core::sync::atomic::Ordering::Relaxed);
    let user = session().map(|(n, _, _)| n).unwrap_or_default();
    let ms = crate::arch::ms().saturating_sub(RL_T0.load(core::sync::atomic::Ordering::Relaxed));
    match (mode, status) {
        (una_abi::RINGKEY_MODE_LOCKED, _) => serial_println!("[holocron] ring=locked at=lock-screen user={}", user),
        (_, 0) => serial_println!("[holocron] ring={} at=login user={} ms={}", if mode == una_abi::RINGKEY_MODE_CREATE { "created" } else { "opened" }, user, ms),
        (_, s) if s == holocron_core::wire::status::BAD_PASSWORD && RL_REFUSAL_SAID.swap(true, core::sync::atomic::Ordering::Relaxed) => {} // RINGLOGIN2: said ONCE per boot (a lock-screen unlock asks again)
        (_, s) => serial_println!("[holocron] ring=refused at=login user={} status={} ({})", user, s, status_word(s)),
    }
    0
}

fn status_word(s: i32) -> &'static str {
    use holocron_core::wire::status as st;
    match s {
        st::BAD_PASSWORD => "bad-password: the ring is under another password (a `holocron init`, or a password reset by the administrator, which cannot re-key it); `holocron unlock <that password>` opens it",
        st::EXISTS => "exists",
        st::NO_RING => "no-ring",
        st::IO => "io",
        st::CORRUPT => "corrupt",
        st::INVALID => "invalid",
        _ => "refused",
    }
}

/// `tests ringlogin`: did THIS boot's login open the ring (no typed `holocron unlock`), and does the running
/// Holocron answer unlocked — i.e. could a sealed Bluetooth bond reconnect now, before any typed unlock.
pub fn ringlogin_selftest() {
    use core::sync::atomic::Ordering::Relaxed;
    let (posted, taken, st, mode) = (RL_POSTED.load(Relaxed), RL_TAKEN.load(Relaxed), RL_STATUS.load(Relaxed), RL_MODE.load(Relaxed));
    let at_login = st == 0 && (mode == una_abi::RINGKEY_MODE_CREATE || mode == una_abi::RINGKEY_MODE_OPEN);
    let live = live_state();
    let ring = match live {
        Some(holocron_core::wire::RingState::Unlocked) => "open",
        Some(holocron_core::wire::RingState::Locked) => "locked",
        Some(holocron_core::wire::RingState::NoRing) => "none",
        None => "no-holocron",
    };
    let reconnect = at_login && ring == "open";
    let made = match RL_POST_MODE.load(Relaxed) {
        una_abi::RINGKEY_MODE_CREATE => "created",
        una_abi::RINGKEY_MODE_OPEN => "opened",
        _ => "none",
    };
    let rekey = rekey_fixture(); // RINGLOGIN2 (B479)
    let arm = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86" };
    let pass = reconnect && rekey;
    serial_println!(
        ":: RINGLOGIN: ring={} at={} reconnect_before_unlock={} rekey={} arm={} passwd_rekeyed={} door={} posted={} taken={} report={} -> {} ::",
        ring, if at_login { "login" } else { "none" }, if reconnect { "ok" } else { "FAIL" }, if rekey { "ok" } else { "FAIL" }, arm,
        RL_REKEYED.load(Relaxed), made, posted, taken,
        if st == i32::MIN { "none" } else { status_word_short(st) }, if pass { "PASS" } else { "FAIL" }
    );
}

fn status_word_short(s: i32) -> &'static str {
    if s == 0 { "ok" } else { status_word(s).split(':').next().unwrap_or("refused") }
}

/// Holocron's live state through its Status verb, asked as the session user (BTKEYSEAL's relay).
fn live_state() -> Option<holocron_core::wire::RingState> {
    #[cfg(all(feature = "lumen", feature = "busreg", feature = "login"))]
    {
        let (name, uid, _) = session()?;
        let s = holocron_core::wire::user_principal(&name, uid);
        if s.len() > 30 {
            return None;
        }
        let mut prin = [0u8; 32];
        prin[0] = 5; // PRIN_USER
        prin[1] = s.len() as u8;
        prin[2..2 + s.len()].copy_from_slice(s.as_bytes());
        return match crate::prefs_client::relay_tag(holocron_core::wire::VERB_STATUS, prin, &[], 500) {
            Ok((0, b)) => holocron_core::wire::decode_status(&b).map(|(st, _, _)| st),
            _ => None,
        };
    }
    #[cfg(not(all(feature = "lumen", feature = "busreg", feature = "login")))]
    None
}

// =================================================================================================
// RINGLOGIN2 (rmbp-ledger B479) — THE RING FOLLOWS THE PASSWORD. TAIL-APPENDED.
//
// `passwd` (fs::users, the session's own row) asks the CURRENT password first and verifies it; once the new
// credential is written it calls [`ring_rekey`]: the ring HEADER is read, BOTH keys are derived with [`kdf`] at
// the header's salt and parameters (which the re-wrap keeps), and a REKEY door (una-abi `RINGKEY_MODE_REKEY`, the
// old key beside the new) is posted for HOLOCRON.ELF, which re-wraps through `holocron_core`'s ONE re-wrap
// (`Ring::rewrap_keyed`, via `Holocron::rekey_with_keys`) and reports; [`rekey_report`] says the witness. A
// failed re-wrap keeps the old ring (holocron_core writes nothing until the re-wrap succeeds, and puts back
// what a failed write touched). An administrator's reset has no old password: the ring is NOT re-keyed
// ([`ring_kept_admin_reset`] says so once), and the next login's refusal is said once with the way back.
// Design: docs/dev/evidence/rmbp-1005/ringlogin2.md.
// =================================================================================================

/// A login refusal (bad-password) was said this boot — it is said once.
static RL_REFUSAL_SAID: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Re-wraps reported OK this boot, and the posted REKEY's clock.
static RL_REKEYED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static RL_REKEY_T0: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// SYS_KDF's body over `password` at `salt` and parameters — the key or the wire's reason word.
#[cfg(all(feature = "lumen", feature = "login"))]
fn derive_at(password: &[u8], salt: &[u8; 16], m: u32, t: u32, p: u32) -> Result<[u8; 32], &'static str> {
    let mut req = alloc::vec![0u8; una_abi::KDF_HDR_LEN + password.len() + salt.len()];
    let Some(n) = una_abi::kdf_request(m, t, p, password, salt, &mut req) else {
        wipe(&mut req);
        return Err("password-length");
    };
    let r = kdf(&req[..n]);
    wipe(&mut req);
    r.map_err(|e| if e == una_abi::ENOMEM { "kdf-enomem" } else { "kdf-refused" })
}

/// `passwd`'s half: `name`'s password changed from `old` (verified by the caller) to `new` (already written).
/// Derives both ring keys and posts a REKEY door; starts HOLOCRON.ELF when it is not running. One line.
pub fn ring_rekey(name: &[u8], old: &[u8], new: &[u8]) {
    #[cfg(all(feature = "lumen", feature = "login"))]
    {
        let t0 = crate::arch::ms();
        let Some(uid) = crate::fs::users::id_of(name) else { return };
        let mut h = [0u8; crate::fs::users::HOME_MAX];
        let Some(hl) = crate::fs::users::home_of(name, &mut h) else { return };
        let Ok(home) = core::str::from_utf8(&h[..hl]) else { return };
        let hdr = match ring_bytes(home) {
            Ok(Some(b)) => match holocron_core::format::parse_ring(&b) {
                Ok((hdr, _, _)) => hdr,
                Err(_) => {
                    serial_println!("[holocron] door none at=passwd reason=ring-unparsed (the ring is NOT re-keyed; Holocron says CORRUPT on its own read)");
                    return;
                }
            },
            Ok(None) => {
                serial_println!("[holocron] ring=none at=passwd (nothing to re-key: the next login makes the ring under the new password)");
                return;
            }
            Err(why) => {
                serial_println!("[holocron] door none at=passwd reason={} (the ring is NOT re-keyed)", why);
                return;
            }
        };
        let (m, t, p) = (hdr.kdf.m_kib, hdr.kdf.t, hdr.kdf.p);
        let mut k_old = match derive_at(old, &hdr.salt, m, t, p) {
            Ok(k) => k,
            Err(why) => {
                serial_println!("[holocron] door none at=passwd reason={} m_kib={} t={} p={} (the ring is NOT re-keyed)", why, m, t, p);
                return;
            }
        };
        let mut k_new = match derive_at(new, &hdr.salt, m, t, p) {
            Ok(k) => k,
            Err(why) => {
                wipe(&mut k_old);
                serial_println!("[holocron] door none at=passwd reason={} m_kib={} t={} p={} (the ring is NOT re-keyed)", why, m, t, p);
                return;
            }
        };
        let mut door = [0u8; una_abi::RINGKEY_LEN];
        una_abi::ringdoor_encode(&una_abi::RingDoor { mode: una_abi::RINGKEY_MODE_REKEY, m_kib: m, t, p, salt: hdr.salt, key: k_new, t0_ms: t0, old_key: k_old }, &mut door);
        wipe(&mut k_old);
        wipe(&mut k_new);
        {
            let mut d = DOOR.lock();
            wipe_door(&mut d);
            *d = Door::Key { uid, door };
        }
        wipe(&mut door);
        RL_POSTED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        RL_REKEY_T0.store(t0, core::sync::atomic::Ordering::Relaxed);
        serial_println!(
            "[holocron] door kdf=kernel mode=rekey m_kib={} t={} p={} ms={} -> posted (both passwords are dropped here; HOLOCRON.ELF re-wraps, the salt and parameters kept)",
            m, t, p, crate::arch::ms().saturating_sub(t0)
        );
        after_login(name); // HOLOCRON.ELF takes the door at its start when it is not already running
    }
    #[cfg(not(all(feature = "lumen", feature = "login")))]
    let _ = (name, old, new);
}

/// `passwd <name>` by the administrator: no old password, so the ring is NOT re-keyed. Said once, at the change.
pub fn ring_kept_admin_reset(name: &[u8]) {
    #[cfg(all(feature = "lumen", feature = "login"))]
    {
        let mut h = [0u8; crate::fs::users::HOME_MAX];
        let Some(hl) = crate::fs::users::home_of(name, &mut h) else { return };
        let Ok(home) = core::str::from_utf8(&h[..hl]) else { return };
        if ring_root(home) != "none" {
            serial_println!(
                "[holocron] ring=kept at=passwd user={} reason=admin-reset (no old password: the ring stays under the old one; the next login says the way back once)",
                core::str::from_utf8(name).unwrap_or("?")
            );
        }
    }
    #[cfg(not(all(feature = "lumen", feature = "login")))]
    let _ = name;
}

/// HOLOCRON.ELF's answer to a REKEY door. Says the witness.
fn rekey_report(status: i32) -> i64 {
    let user = session().map(|(n, _, _)| n).unwrap_or_default();
    let ms = crate::arch::ms().saturating_sub(RL_REKEY_T0.load(core::sync::atomic::Ordering::Relaxed));
    if status == 0 {
        RL_REKEYED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        serial_println!("[holocron] ring=rekeyed at=passwd user={} ms={}", user, ms);
    } else {
        serial_println!(
            "[holocron] ring=rekey-refused at=passwd user={} status={} ({}) -> the old ring is kept: the next login reads bad-password; `holocron unlock <old password>` opens it",
            user, status, status_word_short(status)
        );
    }
    0
}

/// `tests ringlogin`'s `rekey=`: holocron_core's ONE re-wrap on a RAM ring (the production suite, floor
/// parameters, keys from [`kdf`] — the kernel's derivation): a wrong old key keeps the ring byte for byte; the
/// re-wrap keeps the salt; after it the new key opens the ring and its secret and the old key is refused.
fn rekey_fixture() -> bool {
    use holocron_core::cc::{CryptoCore, DrbgEntropy};
    use holocron_core::seal::KdfParams;
    use holocron_core::service::{Holocron, MemStore, Store};
    use holocron_core::wire::{status, Request};
    use holocron_core::zero::Key;
    let f = KdfParams::FLOOR;
    let salt = [0x52u8; 16];
    let key = |pw: &[u8]| -> Option<[u8; 32]> {
        let mut req = alloc::vec![0u8; una_abi::KDF_HDR_LEN + pw.len() + 16];
        let n = una_abi::kdf_request(f.m_kib, f.t, f.p, pw, &salt, &mut req)?;
        kdf(&req[..n]).ok()
    };
    let (Some(k1), Some(k2), Some(kx)) = (key(b"rl2-old"), key(b"rl2-new"), key(b"rl2-wrong")) else { return false };
    let owner = String::from("user:rl2fixture#4243");
    let me = Some(owner.as_str());
    let mk = |store: MemStore| DrbgEntropy::new(crate::rand::KernelEntropy, b"ringlogin2 rekey").ok().map(|rng| Holocron::new(CryptoCore, CryptoCore, store, rng, owner.clone(), f));
    let Some(mut h) = mk(MemStore::default()) else { return false };
    if h.unlock_with_key(true, salt, f, Key::from_bytes(k1)).is_err() {
        return false;
    }
    let put = Request::Put { ns: "bt".into(), name: "a1b2c3d4e5f6".into(), kind: "rl2-fixture".into(), label: "rl2".into(), data: alloc::vec![7; 16] };
    if h.handle(me, put.verb(), &put.encode_body(), 0, 0).status != status::OK {
        return false;
    }
    let before = h.store_mut().read_ring().ok().flatten();
    let wrong_kept = h.rekey_with_keys(Key::from_bytes(kx), Key::from_bytes(k2)) == Err(status::BAD_PASSWORD) && h.store_mut().read_ring().ok().flatten() == before;
    let done = h.rekey_with_keys(Key::from_bytes(k1), Key::from_bytes(k2)) == Ok(1);
    let after = h.store_mut().read_ring().ok().flatten();
    let salt_kept = match (&before, &after) {
        (Some(a), Some(b)) => matches!((holocron_core::format::parse_ring(a), holocron_core::format::parse_ring(b)), (Ok((x, _, _)), Ok((y, _, _))) if x.salt == y.salt && x.kdf == y.kdf),
        _ => false,
    };
    let store = h.store_mut().clone();
    let Some(mut h2) = mk(store) else { return false };
    let old_refused = h2.unlock_with_key(false, salt, f, Key::from_bytes(k1)) == Err(status::BAD_PASSWORD);
    let new_opens = h2.unlock_with_key(false, salt, f, Key::from_bytes(k2)) == Ok(());
    let get = Request::Get { ns: "bt".into(), name: "a1b2c3d4e5f6".into() };
    let r = h2.handle(me, get.verb(), &get.encode_body(), 0, 0);
    let secret_ok = r.status == status::OK && r.body.as_slice() == [7u8; 16];
    wrong_kept && done && salt_kept && old_refused && new_opens && secret_ok
}
