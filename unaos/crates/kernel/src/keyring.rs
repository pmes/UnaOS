// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Holocron — shared-core
//!
//! HOLOCRON2 (rmbp-ledger B355) — the KERNEL's three small parts of the metal secrets handler. Holocron
//! itself is `APPS/HOLOCRON.ELF` (crates/user-holocron), a ring-3 fulfiller of verbs 144..=151 linking
//! `holocron_core` — the ONE implementation of the formats, the ring and the policy. Nothing here keeps a
//! secret or answers a verb (the ONE key kernel RAM holds — the OPEN keyed root's data key — is KEYBOOT's `KB_KEY` in `fs/unafs.rs`, BP-security-11):
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
    let settings = settings_rekey_word(); // SETTINGSREKEY (B483)
    let pass = reconnect && rekey && settings != "FAIL"; let skip = if pass || !rekey || settings == "FAIL" { None } else { no_ring_home() }; // QEMUCLASS2 (B590): the login leg on a home the door refuses (file tail) is SKIP; the rekey legs still decide
    serial_println!(
        ":: RINGLOGIN: ring={} at={} reconnect_before_unlock={} rekey={} settings_rekey={} settings_rekeyed={} arm={} passwd_rekeyed={} door={} posted={} taken={} report={} -> {}{} ::",
        ring, if at_login { "login" } else { "none" }, if reconnect { "ok" } else if skip.is_some() { "-" } else { "FAIL" }, if rekey { "ok" } else { "FAIL" }, settings, RL_REKEYED_SETTINGS.load(Relaxed), arm,
        RL_REKEYED.load(Relaxed), made, posted, taken,
        if st == i32::MIN { "none" } else { status_word_short(st) }, if pass { "PASS" } else if skip.is_some() { "SKIP reason=" } else { "FAIL" }, skip.as_deref().unwrap_or("")
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
/// SETTINGSREKEY (B483): the posted REKEY's origin (0 `passwd`, 1 `settings`) and Settings' re-wraps reported OK.
static RL_REKEY_AT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
static RL_REKEYED_SETTINGS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

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
    ring_rekey_at(name, old, new, "passwd");
}

/// SETTINGSREKEY (B483): [`ring_rekey`] with the change's origin (`passwd` or `settings`) on every line it says
/// and on HOLOCRON.ELF's answer. Settings' change form calls this from the `login-submit` worker.
pub fn ring_rekey_at(name: &[u8], old: &[u8], new: &[u8], at: &'static str) {
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
                    serial_println!("[holocron] door none at={} reason=ring-unparsed (the ring is NOT re-keyed; Holocron says CORRUPT on its own read)", at);
                    return;
                }
            },
            Ok(None) => {
                serial_println!("[holocron] ring=none at={} (nothing to re-key: the next login makes the ring under the new password)", at);
                return;
            }
            Err(why) => {
                serial_println!("[holocron] door none at={} reason={} (the ring is NOT re-keyed)", at, why);
                return;
            }
        };
        let (m, t, p) = (hdr.kdf.m_kib, hdr.kdf.t, hdr.kdf.p);
        let mut k_old = match derive_at(old, &hdr.salt, m, t, p) {
            Ok(k) => k,
            Err(why) => {
                serial_println!("[holocron] door none at={} reason={} m_kib={} t={} p={} (the ring is NOT re-keyed)", at, why, m, t, p);
                return;
            }
        };
        let mut k_new = match derive_at(new, &hdr.salt, m, t, p) {
            Ok(k) => k,
            Err(why) => {
                wipe(&mut k_old);
                serial_println!("[holocron] door none at={} reason={} m_kib={} t={} p={} (the ring is NOT re-keyed)", at, why, m, t, p);
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
        RL_REKEY_AT.store(if at == "settings" { 1 } else { 0 }, core::sync::atomic::Ordering::Relaxed); // SETTINGSREKEY: the answer says where the change was made
        serial_println!(
            "[holocron] door kdf=kernel mode=rekey m_kib={} t={} p={} ms={} at={} -> posted (both passwords are dropped here; HOLOCRON.ELF re-wraps, the salt and parameters kept)",
            m, t, p, crate::arch::ms().saturating_sub(t0), at
        );
        after_login(name); // HOLOCRON.ELF takes the door at its start when it is not already running
    }
    #[cfg(not(all(feature = "lumen", feature = "login")))]
    let _ = (name, old, new, at);
}

/// `passwd <name>` by the administrator: no old password, so the ring is NOT re-keyed. Said once, at the change.
pub fn ring_kept_admin_reset(name: &[u8]) {
    ring_kept_admin_reset_at(name, "passwd");
}

/// SETTINGSREKEY (B483): [`ring_kept_admin_reset`] with the change's origin (Settings' Users pane Reset says `settings`).
pub fn ring_kept_admin_reset_at(name: &[u8], at: &'static str) {
    #[cfg(all(feature = "lumen", feature = "login"))]
    {
        let mut h = [0u8; crate::fs::users::HOME_MAX];
        let Some(hl) = crate::fs::users::home_of(name, &mut h) else { return };
        let Ok(home) = core::str::from_utf8(&h[..hl]) else { return };
        if ring_root(home) != "none" {
            serial_println!(
                "[holocron] ring=kept at={} user={} reason=admin-reset (no old password: the ring stays under the old one; the next login says the way back once)",
                at, core::str::from_utf8(name).unwrap_or("?")
            );
        }
    }
    #[cfg(not(all(feature = "lumen", feature = "login")))]
    let _ = (name, at);
}

/// HOLOCRON.ELF's answer to a REKEY door. Says the witness.
fn rekey_report(status: i32) -> i64 {
    let user = session().map(|(n, _, _)| n).unwrap_or_default();
    let ms = crate::arch::ms().saturating_sub(RL_REKEY_T0.load(core::sync::atomic::Ordering::Relaxed));
    let settings = RL_REKEY_AT.load(core::sync::atomic::Ordering::Relaxed) == 1; // SETTINGSREKEY (B483)
    let at = if settings { "settings" } else { "passwd" };
    if status == 0 {
        RL_REKEYED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if settings { RL_REKEYED_SETTINGS.fetch_add(1, core::sync::atomic::Ordering::Relaxed); }
        serial_println!("[holocron] ring=rekeyed at={} user={} ms={}", at, user, ms);
    } else {
        serial_println!(
            "[holocron] ring=rekey-refused at={} user={} status={} ({}) -> the old ring is kept: the next login reads bad-password; `holocron unlock <old password>` opens it",
            at, user, status, status_word_short(status)
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

// =================================================================================================
// SETTINGSREKEY (rmbp-ledger B483) — `tests ringlogin`'s `settings_rekey=`. TAIL-APPENDED.
// The Settings change form's mode map and its REAL worker body (`video::crystal::login`): see
// docs/dev/evidence/rmbp-1005/settingsrekey.md.
// =================================================================================================

fn settings_rekey_word() -> &'static str {
    #[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        crate::video::crystal::login::settings_rekey_fixture()
    }
    #[cfg(not(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))))]
    {
        "no-form"
    }
}

// QEMUCLASS2 (rmbp-ledger B590) — TAIL-APPENDED. QEMUIMAGE26 read `:: RINGLOGIN: ring=no-holocron at=none
// reconnect_before_unlock=FAIL … door=none posted=0 … -> FAIL ::`, and earlier on the same wire `[holocron] door none
// reason=home-not-unafs`: the login's half ([`ring_login`]) posts the ring key only when the session home has inode ids
// (`mt.stat(home)` `id.is_some()` — the ring lives on UnaFS), and [`after_login`] starts no Holocron without a ring or a
// create waiting in the door. QEMU's root is a FAT ESP (`fat:rw:`, no UnaFS): the login leg cannot run there, and the
// fixture printed FAIL for the environment. It now asks the door's own predicate and says SKIP with QEMUSKIP's (B613)
// word: `home-not-writable(<k>)` when `fs::vfs::home_unwritable` names the refusing volume, else the door's own
// `home-not-unafs`. A UnaFS home runs the leg as before; the RINGLOGIN2/SETTINGSREKEY legs still FAIL the line.

/// `Some(reason)` when the session's home is one [`ring_login`] refuses (no inode ids: not UnaFS); `None` otherwise.
fn no_ring_home() -> Option<String> {
    let (_, _, home) = session()?;
    let mt = crate::shell::vfs_mount_table();
    if matches!(mt.stat(&home), Ok(st) if st.id.is_some()) {
        return None;
    }
    Some(match crate::fs::vfs::home_unwritable(&mt, &home) {
        Some(k) => alloc::format!("home-not-writable({})", k),
        None => String::from("home-not-unafs"),
    })
}

// =================================================================================================
// VOLKEY (rmbp-ledger B684) M1 + M3 — the volume key slot's KERNEL half. TAIL-APPENDED.
//
// The slot format, the wraps and the mount gate are the `unafs` crate's (`unafs::keyslot`, unafs-records.md
// §R2a — ONE implementation, BP-storage-7); the kernel supplies the entropy (`rand::drbg_fill`) and proves the
// password KEK is SYS_KDF's (the same Argon2id call). Nothing here keeps a key (BP-security-11): `volkey_seal`
// returns the slot and the recovery words, and the words wipe themselves when the caller drops them.
//
// NOT here (VOLKEY's STOP, reported with the exact change): keying the LIVE root at the set-password screen.
// The kernel mounts the root before anyone can type (USERS.DAT is on it, BP-storage-13), and three mount sites
// outside this arc's files (`fs/unafs.rs:452` is in it; `install/unafsmirror.rs:149/161` and
// `arch/aarch64/sdmmc_tegra.rs:2333` are not) would answer `KeyRequired` for a keyed root on the next boot.
// KEYBOOT (B688) built that unlock (`fs::unafs::unlock_root`): `users::keyboot_key_root` now keys the LIVE root with the first administrator's password.
// =================================================================================================

/// VOLKEY: a key slot for `password` at `kdf` — a fresh 32-byte data key and recovery key from the kernel DRBG,
/// wrapped by Argon2id(password) and by the recovery key. The words are shown ONCE by the caller and dropped.
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
pub fn volkey_seal(password: &[u8], kdf: ::unafs::KdfParams) -> Result<(::unafs::KeySlot, ::unafs::RecoveryWords), ::unafs::KeySlotError> {
    ::unafs::KeySlot::seal(password, kdf, &mut |b: &mut [u8]| {
        let _ = crate::rand::drbg_fill(b);
    })
}

/// How many times `needle` occurs in `hay` (the never-on-disk scan).
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn volkey_count(hay: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || needle.len() > hay.len() {
        return 0;
    }
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// `tests volkey` (M3): on a RAM volume — key it, then: no key = refused, a wrong password = refused, the
/// password opens it, the 24 words (and the words cut to four letters) open it, a one-word typo is refused before
/// any key work; the slot survives a grow; the password KEK equals SYS_KDF's for the slot's salt; and the data
/// key, the recovery words, their bytes and the password occur 0 times in the volume's bytes (`sends=`). Floor
/// KDF costs; no secret on the wire — this fixture prints counts and words like `ok` only.
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
pub fn volkey_selftest() {
    use ::unafs::fs::FileSystemError;
    use ::unafs::{KdfParams, KeySlotError, MemDevice, UnaFS, Unlock};
    const PW: &[u8] = b"volkey-fixture-pw";
    const WRONG: &[u8] = b"volkey-fixture-pX";
    const WITNESS: &[u8] = b"VOLKEY witness: opened with the key";
    const FROM: u64 = 512;
    const TO: u64 = 640;
    let t0 = crate::arch::ms();
    let fail = |why: &str| serial_println!(":: VOLKEY: wrapped=0 mount_nokey=none mount_key=none recovery=none sends=0 -> FAIL ({}) ::", why);
    // ── a RAM volume with a witness file, keyed, then grown (block 0 rewritten: the slot must stay) ──────────
    let mut fs = match UnaFS::format(MemDevice::with_blocks(FROM), 0) {
        Ok(f) => f,
        Err(_) => return fail("format refused"),
    };
    let root = fs.superblock.root_inode;
    let wrote = fs.create_file(root, "w.txt".into()).and_then(|id| fs.write_data(id, 0, WITNESS)).is_ok();
    let (slot, words) = match volkey_seal(PW, KdfParams::FLOOR) {
        Ok(x) => x,
        Err(e) => return fail(match e { KeySlotError::NoMemory => "kdf memory", _ => "seal refused" }),
    };
    let salt = *slot.salt();
    let set = fs.set_key_slot(slot.clone()).is_ok();
    fs.device.resize_blocks(TO);
    let grown = fs.grow(TO).is_ok();
    let bytes = core::mem::take(&mut fs.device).into_bytes();
    drop(fs);
    let on_disk = matches!(::unafs::KeySlot::parse(&bytes[..::unafs::BLOCK_SIZE as usize]), Ok(Some(s)) if s == slot);
    // ── the wrap is SYS_KDF's: the kernel's body and the crate's KEK agree for the slot's salt ─────────────
    let mut req = alloc::vec![0u8; una_abi::KDF_HDR_LEN + PW.len() + salt.len()];
    let f = KdfParams::FLOOR;
    let sys = una_abi::kdf_request(f.m_kib, f.t, f.p, PW, &salt, &mut req).and_then(|n| kdf(&req[..n]).ok());
    let crate_kek = ::unafs::keyslot::password_kek(PW, &salt, f).ok();
    let same_kdf = matches!((&sys, &crate_kek), (Some(a), Some(b)) if a == b);
    if let Some(mut k) = sys { wipe(&mut k); }
    if let Some(mut k) = crate_kek { wipe(&mut k); }
    let wrapped = wrote && set && grown && on_disk && same_kdf;
    // ── no key, a wrong key: refused ────────────────────────────────────────────────────────────────────────
    let nokey = UnaFS::mount(MemDevice::from_bytes(bytes.clone()));
    let nokey_ok = matches!(nokey, Err(FileSystemError::KeyRequired));
    drop(nokey);
    let wrong = UnaFS::mount_with(MemDevice::from_bytes(bytes.clone()), &Unlock::Password(WRONG));
    let wrong_ok = matches!(wrong, Err(FileSystemError::KeySlot(KeySlotError::KeyRefused)));
    drop(wrong);
    let mount_nokey = if nokey_ok && wrong_ok { "refused" } else if !nokey_ok { "OPENED" } else { "wrong-key-OPENED" };
    // ── the password opens it, the witness reads back, the volume is the grown size ─────────────────────────
    let read_back = |m: &mut UnaFS<MemDevice>| m.resolve_path("/w.txt").and_then(|id| m.read_data(id, 0, WITNESS.len() as u64)).is_ok_and(|d| d == WITNESS);
    let (key_ok, pw_key) = match UnaFS::mount_with(MemDevice::from_bytes(bytes.clone()), &Unlock::Password(PW)) {
        Ok(mut m) => {
            let ok = read_back(&mut m) && m.superblock.block_count == TO;
            (ok, m.key_slot().and_then(|s| s.unwrap(&Unlock::Password(PW)).ok()))
        }
        Err(_) => (false, None),
    };
    // ── the recovery words open it (whole and cut to four letters); a one-word typo is refused first ───────
    let mut cut = String::new();
    for (i, w) in words.as_str().split(' ').enumerate() {
        if i > 0 { cut.push(' '); }
        cut.push_str(&w[..w.len().min(4)]);
    }
    let mut typo = String::new();
    for (i, w) in words.as_str().split(' ').enumerate() {
        if i > 0 { typo.push(' '); }
        typo.push_str(if i == 5 { if w == "zone" { "able" } else { "zone" } } else { w });
    }
    let rc_key = slot.unwrap(&Unlock::Recovery(words.as_str())).ok();
    let rc_mount = UnaFS::mount_with(MemDevice::from_bytes(bytes.clone()), &Unlock::Recovery(words.as_str())).is_ok_and(|mut m| read_back(&mut m));
    let rc_cut = slot.unwrap(&Unlock::Recovery(&cut)).is_ok();
    let rc_typo = matches!(slot.unwrap(&Unlock::Recovery(&typo)), Err(KeySlotError::BadRecovery) | Err(KeySlotError::KeyRefused)); // the checksum catches it (1 in 65536 reaches the tag, which refuses)
    let same_key = matches!((&pw_key, &rc_key), (Some(a), Some(b)) if a.bytes() == b.bytes());
    let recovery = rc_mount && rc_cut && rc_typo && same_key;
    // ── never on the disk: the data key, the words, their bytes, the password ──────────────────────────────
    let mut entropy = ::unafs::keyslot::parse_recovery(words.as_str()).unwrap_or([0; ::unafs::keyslot::RECOVERY_ENTROPY]);
    let mut sends = volkey_count(&bytes, PW) + volkey_count(&bytes, words.as_str().as_bytes()) + volkey_count(&bytes, &entropy);
    if let Some(k) = &pw_key { sends += volkey_count(&bytes, k.bytes()); }
    wipe(&mut entropy);
    let (mut cut, mut typo) = (cut.into_bytes(), typo.into_bytes());
    wipe(&mut cut);
    wipe(&mut typo);
    drop((pw_key, rc_key, words));
    serial_println!(
        "[volkey] fx kdf=argon2id m={}KiB t={} p={} slot=sector1 sys_kdf={} grow_kept={} typo={} cut4={} ms={}",
        f.m_kib, f.t, f.p, if same_kdf { "same" } else { "DIFFERS" }, if grown && on_disk { "yes" } else { "NO" },
        if rc_typo { "refused" } else { "ACCEPTED" }, if rc_cut { "ok" } else { "FAIL" }, crate::arch::ms().saturating_sub(t0)
    );
    let pass = wrapped && mount_nokey == "refused" && key_ok && recovery && sends == 0;
    serial_println!(
        ":: VOLKEY: wrapped={} mount_nokey={} mount_key={} recovery={} sends={} -> {} ::",
        wrapped as u8, mount_nokey, if key_ok { "ok" } else { "FAIL" }, if recovery { "ok" } else { "FAIL" }, sends, if pass { "PASS" } else { "FAIL" }
    );
}

// =================================================================================================
// KEYBOOT (rmbp-ledger B688) — `tests keyboot`. TAIL-APPENDED, registered beside `volkey` (tests.rs, line-neutral).
//
// On a scratch RAM volume, through the crate calls the boot makes: KEY it as the first administrator's keying does
// (`volkey_seal` + `set_key_slot`), then a remount without the key and with a wrong one is REFUSED (the gate every
// lazy bind of a locked root meets), the password and the 24 words each OPEN it (the two modes of the unlock form's
// `mount_with`) with the witness file read back, a second keying is refused, and the words go through the SAME
// one-shot hand-over the keying uses (`users::keyboot_words_park/take`): taken once, never again. The REKEY leg
// re-wraps the slot for a new password (`rewrap_password` + `replace_key_slot`, `users::keyboot_rekey`'s calls): the
// old password is refused, the new one and the unchanged words open it. Floor KDF costs; no secret on the wire.
// =================================================================================================

/// KEYBOOT: the rekey leg — open the keyed volume with `pw`, re-wrap its slot for `new` (`rewrap_password`, the
/// call `users::keyboot_rekey` makes) and replace it; then the old password is refused, the new one opens the volume
/// and the unchanged recovery words still do. `ok` or the first leg that failed.
#[cfg(all(feature = "login", any(target_arch = "aarch64", feature = "unafs")))]
fn keyboot_rekey_leg(bytes: &[u8], pw: &[u8], new: &[u8], words: &str) -> &'static str {
    use ::unafs::fs::FileSystemError;
    use ::unafs::{KeySlotError, MemDevice, UnaFS, Unlock};
    let Ok(mut fs) = UnaFS::mount_with(MemDevice::from_bytes(bytes.to_vec()), &Unlock::Password(pw)) else {
        return "open-FAIL";
    };
    let Some(slot) = fs.key_slot().cloned() else {
        return "unkeyed-FAIL";
    };
    let Ok(next) = slot.rewrap_password(&Unlock::Password(pw), new, &mut |b: &mut [u8]| {
        let _ = crate::rand::drbg_fill(b);
    }) else {
        return "rewrap-FAIL";
    };
    if fs.replace_key_slot(next).is_err() {
        return "write-FAIL";
    }
    let after = core::mem::take(&mut fs.device).into_bytes();
    drop(fs);
    let old_refused = matches!(UnaFS::mount_with(MemDevice::from_bytes(after.clone()), &Unlock::Password(pw)), Err(FileSystemError::KeySlot(KeySlotError::KeyRefused)));
    let new_opens = UnaFS::mount_with(MemDevice::from_bytes(after.clone()), &Unlock::Password(new)).is_ok();
    let words_open = UnaFS::mount_with(MemDevice::from_bytes(after), &Unlock::Recovery(words)).is_ok();
    match (old_refused, new_opens, words_open) {
        (true, true, true) => "ok",
        (false, _, _) => "old-OPENS",
        (_, false, _) => "new-FAIL",
        _ => "words-FAIL",
    }
}

/// `tests keyboot` (see the section head).
#[cfg(all(feature = "login", any(target_arch = "aarch64", feature = "unafs")))]
pub fn keyboot_selftest() {
    use ::unafs::fs::FileSystemError;
    use ::unafs::{KdfParams, KeySlotError, MemDevice, UnaFS, Unlock};
    const PW: &[u8] = b"keyboot-fixture-pw";
    const WRONG: &[u8] = b"keyboot-fixture-pX";
    const WITNESS: &[u8] = b"KEYBOOT witness: the scratch root opened with its key";
    let t0 = crate::arch::ms();
    let fail = |why: &str| serial_println!(":: KEYBOOT: keyed=0 refused=0 unlock_pw=none unlock_words=none rekey=none words_shown_once=0 -> FAIL ({}) ::", why);
    let mut fs = match UnaFS::format(MemDevice::with_blocks(512), 0) {
        Ok(f) => f,
        Err(_) => return fail("format refused"),
    };
    let root = fs.superblock.root_inode;
    let wrote = fs.create_file(root, "w.txt".into()).and_then(|id| fs.write_data(id, 0, WITNESS)).is_ok();
    let (slot, words) = match volkey_seal(PW, KdfParams::FLOOR) {
        Ok(x) => x,
        Err(KeySlotError::NoMemory) => return fail("kdf memory"),
        Err(_) => return fail("seal refused"),
    };
    let keyed = wrote && fs.set_key_slot(slot.clone()).is_ok() && fs.key_slot() == Some(&slot);
    let bytes = core::mem::take(&mut fs.device).into_bytes();
    drop(fs);
    // ── remount without the key, and with a wrong one: refused ─────────────────────────────────────────────────
    let nokey = matches!(UnaFS::mount(MemDevice::from_bytes(bytes.clone())), Err(FileSystemError::KeyRequired));
    let wrong = matches!(UnaFS::mount_with(MemDevice::from_bytes(bytes.clone()), &Unlock::Password(WRONG)), Err(FileSystemError::KeySlot(KeySlotError::KeyRefused)));
    let refused = nokey && wrong;
    // ── the password opens it (and a second keying is refused); the words open it ──────────────────────────────
    let read_back = |m: &mut UnaFS<MemDevice>| m.resolve_path("/w.txt").and_then(|id| m.read_data(id, 0, WITNESS.len() as u64)).is_ok_and(|d| d == WITNESS);
    let unlock_pw = UnaFS::mount_with(MemDevice::from_bytes(bytes.clone()), &Unlock::Password(PW)).is_ok_and(|mut m| {
        read_back(&mut m) && matches!(m.set_key_slot(slot.clone()), Err(FileSystemError::KeySlot(KeySlotError::AlreadyKeyed)))
    });
    let unlock_words = UnaFS::mount_with(MemDevice::from_bytes(bytes.clone()), &Unlock::Recovery(words.as_str())).is_ok_and(|mut m| read_back(&mut m));
    let rekey = keyboot_rekey_leg(&bytes, PW, b"keyboot-fixture-new", words.as_str());
    // ── the words' one showing: the keying's own hand-over, taken once and never again ─────────────────────────
    let taken0 = crate::fs::users::keyboot_words_taken();
    let parked = crate::fs::users::keyboot_words_park(words);
    let first = crate::fs::users::keyboot_words_take().is_some();
    let second = crate::fs::users::keyboot_words_take().is_none();
    let once = parked && first && second && crate::fs::users::keyboot_words_taken() == taken0 + 1;
    serial_println!(
        "[keyboot] fx kdf=argon2id m={}KiB t={} p={} nokey={} wrong={} parked={} ms={}",
        KdfParams::FLOOR.m_kib, KdfParams::FLOOR.t, KdfParams::FLOOR.p,
        if nokey { "refused" } else { "OPENED" }, if wrong { "refused" } else { "OPENED" },
        if parked { "yes" } else { "busy" }, crate::arch::ms().saturating_sub(t0)
    );
    let pass = keyed && refused && unlock_pw && unlock_words && rekey == "ok" && once;
    serial_println!(
        ":: KEYBOOT: keyed={} refused={} unlock_pw={} unlock_words={} rekey={} words_shown_once={} -> {} ::",
        keyed as u8, refused as u8, if unlock_pw { "ok" } else { "FAIL" }, if unlock_words { "ok" } else { "FAIL" }, rekey, once as u8,
        if pass { "PASS" } else { "FAIL" }
    );
}
