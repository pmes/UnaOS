#![no_std]
#![no_main]

extern crate alloc;

/// holocron_core allocates (bodies, sealed files, the ring): the size-class heap over SYS_SBRK, as LUMEN.ELF.
#[global_allocator]
static HEAP: vein_ring3::heap::Heap = vein_ring3::heap::Heap::sbrk();

// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// HOLOCRON2 (rmbp-ledger B355, R82): `APPS/HOLOCRON.ELF` — Holocron, the secrets handler, on the metal.
// Holocron serves EVERY caller, so it is the one resident fulfiller R82 allows (app note RESIDENT: a bare
// `holocron …` detaches). ONE program, two roles, decided by the kernel's answer to REGISTER 144..=151:
//
//   * `0`        — it is THE fulfiller. It applies its own argv verb first (so `holocron init <pw>` and
//                  `holocron unlock <pw>` leave the key in THIS process), then answers every relayed request
//                  with `holocron_core::service::Holocron::handle` until the session ends (SECLOGIN ends a
//                  session's programs at logout). The caller is the kernel's stamp in the relayed header
//                  (`holocron_core::frame::caller` — only a `user:<name>#<uid>` record is ever the owner).
//   * `-EEXIST`  — a fulfiller runs: it is a CLIENT. It sends its argv verb over the bus, prints the answer
//                  (a secret's bytes never: `get` prints the length), raises it as a NOTICE, exits.
//
// Verbs: `init <pw>` · `unlock <pw>` · `lock` · `status` · `put <ns> <name> <value> [label]` ·
// `get <ns> <name>` · `list <ns>` · `delete <ns> <name>`; bare `holocron` = `status`.
//
// I/O: the store is `<home>/.holocron/` (`.ring`, `<ns>/<name>`; THE root, `holocron_core::root` — B448 moved
// it from `<home>/.config/unaos/holocron/`, migrated once at the fulfiller's start) over SYS_PATH_READ/WRITE
// (`PATH_W_TRUNC|MKDIRS`, `PATH_W_UNLINK`, `PATH_R_LIST`), REFUSED unless `<home>` stats with an inode id
// (UnaFS: FAT has no owner, so a ring there would be readable by anything that mounts the card). Entropy:
// CRYPTOCORE's ChaCha20 DRBG over SYS_GETRANDOM (`holocron_core::cc::DrbgEntropy`). The ring KDF: WINDOW2
// (B361, R85) — CRYPTOCORE's Argon2id runs HERE again, in ring 3, over memory lent by SYS_SBRK from the 64 MiB
// ELF window (HOLOCRON2 had to send it to the kernel through SYS_KDF while the window was 4 MiB). New rings are
// made at `METAL_KDF` (48 MiB, t 3, p 4 — RFC 9106's second option with the memory the window holds beside the
// image, heap and stack). SYS_KDF (66) stays a kernel service, unused by Holocron: the ONE exception is a ring
// whose recorded memory does not fit the window (a 64 MiB ring made by the host daemon or by boot 23's
// HOLOCRON.ELF) — it unlocks through SYS_KDF and says so (`[holocron] kdf=kernel reason=window …`). Owner: SYS_WHOAMI.
//
// WIRE. `:: HOLOCRON: serve ring=<unafs|none> verbs=8 owner=<user> state=<none|locked|unlocked> ::` (the
// fulfiller), `[holocron] <verb> -> <status>` per verb, `[holocron] relay verb=<v> caller=<user|other> ->
// <status>` per relayed request; a refusal `:: HOLOCRON: <why> -> refused ::`.

use alloc::string::String;
use alloc::vec::Vec;
use holocron_core::cc::{CryptoCore, DrbgEntropy};
use holocron_core::format::Meta;
use holocron_core::frame;
use holocron_core::seal::{EntropyError, KdfParams, SealError, Sealer, NONCE_LEN, SALT_LEN};
use holocron_core::service::{Holocron, Store, StoreError};
use holocron_core::wire::{self, status, Request, RingState};
use holocron_core::zero::Key;
use una_abi::{
    EEXIST, ENOENT, PATH_IO_HDR_LEN, PATH_IO_MAX, PATH_IO_PATH_MAX, PATH_R_LIST, PATH_W_MKDIRS, PATH_W_TRUNC, PATH_W_UNLINK, STAT_HAS_ID, SYS_EXIT,
    SYS_GETRANDOM, SYS_KDF, SYS_MRECV, SYS_MSEND, SYS_PATH_READ, SYS_PATH_WRITE, SYS_STAT, SYS_WHOAMI, USER_STAT_LEN,
};
use vein_ring3::sys::{sys, write};

fn exit(code: u64) -> ! {
    sys(SYS_EXIT, code, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

// ---- output ------------------------------------------------------------------------------------------

struct Line {
    b: [u8; 512],
    n: usize,
}
impl Line {
    fn new(s: &[u8]) -> Line {
        let mut l = Line { b: [0; 512], n: 0 };
        l.put(s);
        l
    }
    fn put(&mut self, s: &[u8]) -> &mut Line {
        for &c in s {
            if self.n < self.b.len() {
                self.b[self.n] = if (0x20..0x7f).contains(&c) || c == b'\n' { c } else { b'?' };
                self.n += 1;
            }
        }
        self
    }
    fn dec(&mut self, v: i64) -> &mut Line {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let neg = v < 0;
        let mut u = v.unsigned_abs();
        if u == 0 {
            i -= 1;
            d[i] = b'0';
        }
        while u > 0 {
            i -= 1;
            d[i] = b'0' + (u % 10) as u8;
            u /= 10;
        }
        if neg {
            self.put(b"-");
        }
        self.put(&d[i..])
    }
    fn s(&self) -> &[u8] {
        &self.b[..self.n]
    }
    fn wire(&self) {
        write(self.s());
        write(b"\n");
    }
}

fn status_name(s: i32) -> &'static [u8] {
    match s {
        status::OK => b"ok",
        status::BAD_PASSWORD => b"bad-password",
        status::NOT_FOUND => b"not-found",
        status::IO => b"io",
        status::RATE_LIMITED => b"rate-limited",
        status::DENIED => b"denied",
        status::EXISTS => b"exists",
        status::NO_RING => b"no-ring",
        status::INVALID => b"invalid",
        status::TOO_BIG => b"too-big",
        status::LOCKED => b"locked",
        status::CORRUPT => b"corrupt",
        status::NO_VERB => b"no-verb",
        _ => b"error",
    }
}

fn verb_name(v: u8) -> &'static [u8] {
    match v {
        wire::VERB_SECRET_GET => b"get",
        wire::VERB_SECRET_PUT => b"put",
        wire::VERB_SECRET_LIST => b"list",
        wire::VERB_SECRET_DELETE => b"delete",
        wire::VERB_UNLOCK => b"unlock",
        wire::VERB_LOCK => b"lock",
        wire::VERB_SIGN => b"sign",
        wire::VERB_STATUS => b"status",
        _ => b"?",
    }
}

// ---- the bus -----------------------------------------------------------------------------------------

static mut RX: [u8; frame::FRAME_MAX] = [0; frame::FRAME_MAX];

#[allow(static_mut_refs)]
fn rx() -> &'static mut [u8; frame::FRAME_MAX] {
    unsafe { &mut RX }
}

fn send(f: &[u8]) -> i64 {
    sys(SYS_MSEND, f.as_ptr() as u64, f.len() as u64, 0, 0)
}

fn recv() -> i64 {
    let r = rx();
    sys(SYS_MRECV, r.as_mut_ptr() as u64, r.len() as u64, 0, 0)
}

/// Send `f` (corr `corr`) and wait for its reply: `(status, body)`; `Err` = the bus refused the send.
fn ask(f: &[u8], corr: u32) -> Result<(i32, Vec<u8>), i64> {
    let s = send(f);
    if s != 0 {
        return Err(s);
    }
    for _ in 0..16 {
        let n = recv();
        if n < 0 {
            return Err(n);
        }
        let Some(p) = frame::parse(&rx()[..n as usize]) else { continue };
        if p.kind == frame::KIND_REPLY && p.corr == corr {
            return Ok((p.status, p.body.to_vec()));
        }
    }
    Err(una_abi::EIO)
}

/// A NOTICE on the glass (title = this program's name, kernel-stamped): two lines, fire-and-forget.
fn notice(l1: &[u8], l2: &[u8]) {
    let mut b = Vec::with_capacity(l1.len() + l2.len() + 1);
    b.extend_from_slice(l1);
    b.push(b'\n');
    b.extend_from_slice(l2);
    if let Some(f) = frame::request(una_abi::BUS_VERB_NOTICE, 0x484E_0001, &b) {
        let _ = send(&f);
    }
}

// ---- who, where ----------------------------------------------------------------------------------------

struct Who {
    owner: String,
    user: String,
    home: String,
}

fn whoami() -> Option<Who> {
    let mut rec = [0u8; una_abi::WHOAMI_MAX];
    let r = sys(SYS_WHOAMI, rec.as_mut_ptr() as u64, rec.len() as u64, 0, 0);
    if r <= 0 {
        return None;
    }
    let w = una_abi::whoami_parse(&rec[..(r as usize).min(rec.len())])?;
    let name = core::str::from_utf8(w.name).ok()?;
    let home = core::str::from_utf8(w.home).ok()?.trim_end_matches('/');
    if name.is_empty() || !home.starts_with('/') {
        return None;
    }
    Some(Who { owner: wire::user_principal(name, w.uid), user: String::from(name), home: String::from(home) })
}

/// Does `path` stat with an inode id (the native UnaFS backend)?
fn on_unafs(path: &str) -> bool {
    let mut st = [0u8; USER_STAT_LEN];
    let r = sys(SYS_STAT, path.as_ptr() as u64, path.len() as u64, st.as_mut_ptr() as u64, 0);
    r >= 0 && u32::from_le_bytes([st[4], st[5], st[6], st[7]]) & STAT_HAS_ID != 0
}

// ---- the store: SYS_PATH_READ / SYS_PATH_WRITE ----------------------------------------------------------

/// A secret file's ceiling on read (header + SECRET_MAX + tag, generously).
const FILE_CAP: usize = 64 * 1024;

fn path_req(path: &str, flags: u16, offset: u64, data: &[u8]) -> Option<Vec<u8>> {
    let p = path.as_bytes();
    if p.is_empty() || p.len() > PATH_IO_PATH_MAX || data.len() > PATH_IO_MAX {
        return None;
    }
    let mut r = alloc::vec![0u8; PATH_IO_HDR_LEN + p.len() + data.len()];
    r[0..2].copy_from_slice(&(p.len() as u16).to_le_bytes());
    r[2..4].copy_from_slice(&flags.to_le_bytes());
    r[8..16].copy_from_slice(&offset.to_le_bytes());
    r[16..16 + p.len()].copy_from_slice(p);
    r[16 + p.len()..].copy_from_slice(data);
    Some(r)
}

/// The whole file (`flags` 0) or a directory's listing (`PATH_R_LIST`): `Ok(None)` when absent.
fn read_all(path: &str, flags: u16) -> Result<Option<Vec<u8>>, StoreError> {
    let mut out = Vec::new();
    let mut buf = alloc::vec![0u8; PATH_IO_MAX];
    loop {
        let req = path_req(path, flags, out.len() as u64, &[]).ok_or(StoreError)?;
        let r = sys(SYS_PATH_READ, req.as_ptr() as u64, req.len() as u64, buf.as_mut_ptr() as u64, buf.len() as u64);
        if r == ENOENT {
            return if out.is_empty() { Ok(None) } else { Err(StoreError) };
        }
        if r < 0 {
            return Err(StoreError);
        }
        if r == 0 {
            break;
        }
        out.extend_from_slice(&buf[..r as usize]);
        if out.len() > FILE_CAP {
            return Err(StoreError);
        }
    }
    holocron_core::zero::wipe(&mut buf);
    Ok(Some(out))
}

fn write_all(path: &str, data: &[u8]) -> Result<(), StoreError> {
    let mut done = 0usize;
    loop {
        let step = (data.len() - done).min(PATH_IO_MAX);
        let first = done == 0;
        let flags = if first { PATH_W_TRUNC | PATH_W_MKDIRS } else { 0 };
        let mut req = path_req(path, flags, done as u64, &data[done..done + step]).ok_or(StoreError)?;
        let r = sys(SYS_PATH_WRITE, req.as_ptr() as u64, req.len() as u64, 0, 0);
        holocron_core::zero::wipe(&mut req);
        if r < 0 || (r == 0 && step > 0) {
            return Err(StoreError);
        }
        done += r as usize;
        if done >= data.len() {
            return Ok(());
        }
    }
}

/// `<home>/.holocron/` on UnaFS — the host layout (`.ring`, `<ns>/<name>`) at the root `holocron_core::root`
/// names for both rings (HOLOCRONROOT, B448). A legacy root is opened as a PathStore too, only to migrate.
struct PathStore {
    root: String,
    ok: bool,
}

impl PathStore {
    fn ring(&self) -> String {
        alloc::format!("{}/.ring", self.root)
    }
    fn file(&self, ns: &str, name: &str) -> String {
        alloc::format!("{}/{}/{}", self.root, ns, name)
    }
    fn gate(&self) -> Result<(), StoreError> {
        if self.ok { Ok(()) } else { Err(StoreError) }
    }
}

impl Store for PathStore {
    fn read_ring(&mut self) -> Result<Option<Vec<u8>>, StoreError> {
        if !self.ok {
            return Ok(None); // FAT / no home: no ring is visible, and `init` is refused at write
        }
        read_all(&self.ring(), 0)
    }
    fn write_ring(&mut self, bytes: &[u8]) -> Result<(), StoreError> {
        self.gate()?;
        write_all(&self.ring(), bytes)
    }
    fn read(&mut self, ns: &str, name: &str) -> Result<Option<Vec<u8>>, StoreError> {
        if !self.ok {
            return Ok(None);
        }
        read_all(&self.file(ns, name), 0)
    }
    fn write(&mut self, ns: &str, name: &str, file: &[u8], _meta: &Meta) -> Result<(), StoreError> {
        // The metadata is inside the authenticated header; the UnaFS typed-attribute mirror is owed.
        self.gate()?;
        write_all(&self.file(ns, name), file)
    }
    fn list(&mut self, ns: &str) -> Result<Vec<String>, StoreError> {
        if !self.ok {
            return Ok(Vec::new());
        }
        let Some(b) = read_all(&alloc::format!("{}/{}", self.root, ns), PATH_R_LIST)? else { return Ok(Vec::new()) };
        Ok(b.split(|&c| c == b'\n')
            .filter(|l| !l.is_empty() && !l.ends_with(b"/"))
            .filter_map(|l| core::str::from_utf8(l).ok().map(String::from))
            .collect())
    }
    fn namespaces(&mut self) -> Result<Vec<String>, StoreError> {
        // RINGLOGIN2 (B479): the re-wrap's walk — the root's directory entries (a `/` suffix marks one).
        if !self.ok {
            return Ok(Vec::new());
        }
        let Some(b) = read_all(&self.root, PATH_R_LIST)? else { return Ok(Vec::new()) };
        Ok(b.split(|&c| c == b'\n')
            .filter_map(|l| l.strip_suffix(b"/"))
            .filter_map(|l| core::str::from_utf8(l).ok())
            .filter(|n| holocron_core::name::valid(n))
            .map(String::from)
            .collect())
    }
    fn remove(&mut self, ns: &str, name: &str) -> Result<bool, StoreError> {
        self.gate()?;
        unlink(&self.file(ns, name))
    }
    fn remove_ring(&mut self) -> Result<bool, StoreError> {
        self.gate()?;
        unlink(&self.ring())
    }
}

/// `PATH_W_UNLINK` one path: `Ok(false)` when it did not exist.
fn unlink(path: &str) -> Result<bool, StoreError> {
    let req = path_req(path, PATH_W_UNLINK, 0, &[]).ok_or(StoreError)?;
    match sys(SYS_PATH_WRITE, req.as_ptr() as u64, req.len() as u64, 0, 0) {
        r if r >= 0 => Ok(true),
        ENOENT => Ok(false),
        _ => Err(StoreError),
    }
}

/// HOLOCRONROOT (B448): move a legacy root's records into THE root, once (`holocron_core::root::migrate`),
/// and say so in one line. Silent when no legacy root holds anything (every boot after the move).
fn migrate_legacy(home: &str, to: &mut PathStore) {
    if !to.ok {
        return;
    }
    for legacy in holocron_core::root::legacy_roots(home) {
        let Ok(Some(listing)) = read_all(&legacy, PATH_R_LIST) else { continue };
        let nss = holocron_core::root::namespaces(&listing);
        let mut from = PathStore { root: legacy.clone(), ok: true };
        let mut l = Line::new(b"[holocron] root=");
        l.put(to.root.as_bytes()).put(b" migrate from=").put(legacy.trim_start_matches(home.trim_end_matches('/')).trim_start_matches('/').as_bytes());
        match holocron_core::root::migrate(&mut from, to, &nss) {
            Ok(m) if m.empty() => continue,
            Ok(m) => {
                if m.skipped == 0 {
                    for ns in &nss {
                        let _ = unlink(&alloc::format!("{}/{}", legacy, ns)); // empty namespace dirs, best effort
                    }
                    let _ = unlink(&legacy);
                }
                l.put(b" ring=").put(m.ring.word().as_bytes()).put(b" secrets=").dec(m.secrets as i64);
                l.put(b" skipped=").dec(m.skipped as i64).put(b" -> ").put(m.verdict().as_bytes());
            }
            Err(_) => {
                l.put(b" -> io-error (the legacy records stay; the next start retries)");
            }
        }
        l.wire();
    }
}

// ---- the suite: CRYPTOCORE, the KDF in ring 3 (WINDOW2) ------------------------------------------------

/// WINDOW2: the parameters HOLOCRON.ELF makes new rings with — RFC 9106's second recommended option (t 3,
/// p 4) at 48 MiB, the memory the 64 MiB window holds beside the image, the heap and the stack. Above
/// `KdfParams::FLOOR` (19 MiB, t 2, p 1).
const METAL_KDF: KdfParams = KdfParams { m_kib: una_abi::RING_KDF_M_KIB, t: una_abi::RING_KDF_T, p: una_abi::RING_KDF_P }; // HOLOCRONARM (B484): per arch (x86 = WINDOW2's 48 MiB; aarch64 19 MiB — the Pi/Orin kernel heap the window borrows is 48)
const _: () = assert!(METAL_KDF.m_kib >= KdfParams::FLOOR.m_kib && METAL_KDF.t >= KdfParams::FLOOR.t && METAL_KDF.p >= KdfParams::FLOOR.p);
const _: () = assert!((METAL_KDF.m_kib as u64) * 1024 + (8 << 20) <= una_abi::USER_WINDOW_BYTES);

/// WINDOW2: where the last derivation ran — 0 none yet, 1 ring 3, 2 the kernel (SYS_KDF, the legacy fallback).
static KDF_WHERE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// WINDOW2: CRYPTOCORE's Argon2id in THIS process. The block memory is lent by SYS_SBRK (the size-class heap
/// stops at 8 MiB classes) and handed back the moment the hash is done (crypto_core zeroises it first).
/// `None` = the window cannot hold it (SYS_SBRK refused); `Some(Err)` = a parameter Argon2 refuses.
fn derive_ring3(password: &[u8], salt: &[u8; SALT_LEN], params: &KdfParams) -> Option<Result<Key, SealError>> {
    use crypto_core::argon2::{argon2, Block, Params, Variant, Version};
    let p = Params { variant: Variant::Argon2id, version: Version::V0x13, m_kib: params.m_kib, t: params.t, p: params.p };
    let n = p.blocks();
    if n == 0 {
        return Some(Err(SealError::Param));
    }
    let bytes = (n * core::mem::size_of::<Block>() + 4096) as i64;
    let old = sys(una_abi::SYS_SBRK, bytes as u64, 0, 0, 0);
    if old < 0 {
        return None;
    }
    let base = ((old as u64) + 4095) & !4095;
    // SAFETY: SYS_SBRK mapped `[old, old + bytes)` RW for this process, freshly zeroed; `base + n blocks` lies
    // inside it (one page of slack for the alignment) and nothing else refers to it until it is given back.
    let mem = unsafe { core::slice::from_raw_parts_mut(base as *mut Block, n) };
    let mut out = [0u8; 32];
    let r = argon2(&p, password, salt, &[], &[], mem, &mut out);
    let _ = sys(una_abi::SYS_SBRK, (-bytes) as u64, 0, 0, 0); // single-threaded: the break is still ours to lower
    if r.is_err() {
        holocron_core::zero::wipe(&mut out);
        return Some(Err(SealError::Param));
    }
    let k = Key::from_bytes(out);
    holocron_core::zero::wipe(&mut out);
    Some(Ok(k))
}

/// `CryptoCore` with `derive_key` run in ring 3 (WINDOW2); SYS_KDF only for a ring whose memory the window
/// cannot hold.
#[derive(Clone, Copy)]
struct MetalSealer;

impl Sealer for MetalSealer {
    const SUITE: u8 = <CryptoCore as Sealer>::SUITE;

    fn derive_key(&self, password: &[u8], salt: &[u8; SALT_LEN], params: &KdfParams) -> Result<Key, SealError> {
        if let Some(r) = derive_ring3(password, salt, params) {
            KDF_WHERE.store(1, core::sync::atomic::Ordering::Relaxed);
            return r;
        }
        Line::new(b"[holocron] kdf=kernel reason=window m_kib=").dec(params.m_kib as i64).put(b" (a ring made where 64 MiB fits; SYS_KDF 66)").wire();
        KDF_WHERE.store(2, core::sync::atomic::Ordering::Relaxed);
        let mut req = [0u8; una_abi::KDF_HDR_LEN + una_abi::KDF_PW_MAX + una_abi::KDF_SALT_MAX];
        let n = una_abi::kdf_request(params.m_kib, params.t, params.p, password, salt, &mut req).ok_or(SealError::Param)?;
        let mut out = [0u8; 32];
        let r = sys(SYS_KDF, req.as_ptr() as u64, n as u64, out.as_mut_ptr() as u64, out.len() as u64);
        holocron_core::zero::wipe(&mut req);
        if r != 0 {
            holocron_core::zero::wipe(&mut out);
            return Err(SealError::Param);
        }
        let k = Key::from_bytes(out);
        holocron_core::zero::wipe(&mut out);
        Ok(k)
    }
    fn subkey(&self, key: &Key, salt: &[u8], info: &[u8]) -> Key {
        CryptoCore.subkey(key, salt, info)
    }
    fn seal(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        CryptoCore.seal(key, nonce, aad, plaintext)
    }
    fn open(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SealError> {
        CryptoCore.open(key, nonce, aad, sealed)
    }
}

fn getrandom(b: &mut [u8]) -> isize {
    sys(SYS_GETRANDOM, b.as_mut_ptr() as u64, b.len() as u64, 0, 0) as isize
}

type Rng = DrbgEntropy<crypto_core::drbg::GetrandomEntropy<fn(&mut [u8]) -> isize>>;
type Svc = Holocron<MetalSealer, CryptoCore, PathStore, Rng>;

fn rng() -> Result<Rng, EntropyError> {
    let f: fn(&mut [u8]) -> isize = getrandom;
    DrbgEntropy::new(crypto_core::drbg::GetrandomEntropy::new(f), b"HOLOCRON.ELF")
}

// ---- argv -> request -------------------------------------------------------------------------------------

fn arg(a: &una_abi::Args<'_>, i: usize) -> Option<String> {
    a.get(i).and_then(|b| core::str::from_utf8(b).ok()).map(String::from)
}

/// The request argv names (`None` = bare `holocron`), or the usage refusal.
fn request_from_argv() -> Result<Option<Request>, &'static [u8]> {
    let Some(a) = una_abi::args() else { return Ok(None) };
    let Some(verb) = arg(&a, 1) else { return Ok(None) };
    let need = |i: usize| arg(&a, i).ok_or(&b"usage: holocron init|unlock <pw> | lock | status | put <ns> <name> <value> [label] | get|delete <ns> <name> | list <ns>"[..]);
    Ok(Some(match verb.as_str() {
        "init" => Request::Unlock { create: true, password: need(2)?.into_bytes() },
        "unlock" => Request::Unlock { create: false, password: need(2)?.into_bytes() },
        "lock" => Request::Lock,
        "status" => Request::Status,
        "put" => {
            let ns = need(2)?;
            let kind = if ns == holocron_core::service::SSH_NS {
                holocron_core::service::KIND_ED25519
            } else if ns == holocron_core::keysource::VEIN_NS {
                "api-key"
            } else {
                "secret"
            };
            let name = need(3)?;
            // ssh keys: no value mints the seed from Holocron's entropy (the empty SecretPut body rule).
            let data = if kind == holocron_core::service::KIND_ED25519 { Vec::new() } else { need(4)?.into_bytes() };
            let label = arg(&a, if data.is_empty() { 4 } else { 5 }).unwrap_or_default();
            Request::Put { ns, name, kind: String::from(kind), label, data }
        }
        "get" => Request::Get { ns: need(2)?, name: need(3)? },
        "delete" => Request::Delete { ns: need(2)?, name: need(3)? },
        "list" => Request::List { ns: need(2)? },
        _ => return Err(b"holocron: unknown verb (init unlock lock status put get list delete)"),
    }))
}

/// Print and raise one answer. The secret's bytes are never printed: `get` reports their length.
fn report(verb: u8, st: i32, body: &[u8], how: &[u8]) {
    let mut l = Line::new(b"[holocron] ");
    l.put(verb_name(verb)).put(b" -> ").put(status_name(st)).put(b" (").put(how).put(b")");
    if st == status::OK {
        match verb {
            wire::VERB_SECRET_GET => {
                l.put(b" bytes=").dec(body.len() as i64);
            }
            wire::VERB_SECRET_LIST => {
                if let Some(es) = wire::decode_list(body) {
                    l.put(b" n=").dec(es.len() as i64);
                    for e in es.iter().take(16) {
                        l.put(b" ").put(e.name.as_bytes());
                    }
                }
            }
            wire::VERB_STATUS => {
                if let Some((s, _, o)) = wire::decode_status(body) {
                    l.put(b" state=").put(state_name(s)).put(b" owner=").put(o.as_bytes());
                }
            }
            _ => {}
        }
    }
    l.wire();
    let mut t = Line::new(b"holocron ");
    t.put(verb_name(verb)).put(b": ").put(status_name(st));
    notice(t.s(), &l.s()[11..]);
}

fn state_name(s: RingState) -> &'static [u8] {
    match s {
        RingState::NoRing => b"none",
        RingState::Locked => b"locked",
        RingState::Unlocked => b"unlocked",
    }
}

fn now_unix() -> i64 {
    vein_ring3::sys::unix_time().unwrap_or(0)
}

// ---- the program -----------------------------------------------------------------------------------------

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let _ = &APP_NOTE;
    if una_abi::args().and_then(|a| arg(&a, 1)).as_deref() == Some("--kdf-selftest") {
        kdf_selftest();
    }
    let Some(who) = whoami() else {
        write(b":: HOLOCRON: owner=none (no session: a keyring is a person's) -> refused ::\n");
        exit(1);
    };
    let req = match request_from_argv() {
        Ok(r) => r,
        Err(why) => {
            write(why);
            write(b"\n");
            notice(b"holocron", why);
            exit(2);
        }
    };
    let reg = match ask(&frame::register(1, &wire::VERBS), 1) {
        Ok((s, _)) => s as i64,
        Err(e) => e,
    };
    if reg == EEXIST {
        // A fulfiller runs: be its client.
        let r = req.unwrap_or(Request::Status);
        let body = r.encode_body();
        let Some(f) = frame::request(r.verb(), 2, &body) else { exit(2) };
        match ask(&f, 2) {
            Ok((st, mut b)) => {
                report(r.verb(), st, &b, b"via the running Holocron");
                holocron_core::zero::wipe(&mut b);
                exit(if st == 0 { 0 } else { 1 });
            }
            Err(e) => {
                Line::new(b":: HOLOCRON: client bus=").dec(e).put(b" -> refused ::").wire();
                exit(1);
            }
        }
    }
    if reg != 0 {
        Line::new(b":: HOLOCRON: register=").dec(reg).put(b" (needs the kernel's busreg) -> refused ::").wire();
        exit(1);
    }

    // THE fulfiller.
    let home_unafs = on_unafs(&who.home);
    let mut store = PathStore { root: holocron_core::root::root(&who.home), ok: home_unafs };
    migrate_legacy(&who.home, &mut store); // B448: BEFORE the service reads the ring
    let Ok(rng) = rng() else {
        write(b":: HOLOCRON: entropy=refused (SYS_GETRANDOM could not seed the DRBG) -> refused ::\n");
        exit(1);
    };
    let mut svc: Svc = Holocron::new(MetalSealer, CryptoCore, store, rng, who.owner.clone(), METAL_KDF); // WINDOW2: rings the window can unlock in ring 3
    let me = Some(who.owner.as_str());
    if let Some(r) = &req {
        let mut a = svc.handle(me, r.verb(), &r.encode_body(), vein_ring3::sys::now_ms(), now_unix());
        report(r.verb(), a.status, &a.body, b"in this Holocron");
        holocron_core::zero::wipe(&mut a.body);
    }
    ringlogin_door(&mut svc); // RINGLOGIN (B465): the login's key, before the serve line says the state
    let st = svc.handle(me, wire::VERB_STATUS, &[], vein_ring3::sys::now_ms(), now_unix());
    let state = wire::decode_status(&st.body).map(|(s, _, _)| state_name(s)).unwrap_or(b"?");
    let mut l = Line::new(b":: HOLOCRON: serve ring=");
    l.put(if home_unafs { b"unafs" as &[u8] } else { b"none" }).put(b" verbs=").dec(wire::VERBS.len() as i64);
    l.put(b" owner=").put(who.user.as_bytes()).put(b" state=").put(state).put(b" root=").put(holocron_core::root::DIR.as_bytes());
    // WINDOW2: where the KDF runs — ring 3 (SYS_KDF 66 unused by Holocron), or the kernel for a ring that does not fit.
    l.put(match KDF_WHERE.load(core::sync::atomic::Ordering::Relaxed) {
        2 => b" kdf=kernel(sys_kdf=used,legacy-ring)" as &[u8],
        _ => b" kdf=ring3 sys_kdf=unused",
    });
    l.put(b" ::");
    l.wire();
    loop {
        let n = recv();
        if n < 0 {
            ringlogin_door(&mut svc); // RINGLOGIN (B465): a login / lock-screen unlock posts a key, the lock screen a LOCK
            vein_ring3::sys::sleep_ms(20);
            continue;
        }
        let r = rx();
        let Some(p) = frame::parse(&r[..n as usize]) else { continue };
        if p.kind != frame::KIND_REQUEST || !wire::VERBS.contains(&p.verb) {
            continue; // replies to our own NOTICEs, and anything not Holocron's
        }
        let caller = frame::caller(&p);
        let mut a = svc.handle(caller, p.verb, p.body, vein_ring3::sys::now_ms(), now_unix());
        let mut out = frame::reply(p.verb, p.corr, a.status, &a.body);
        let s = send(&out);
        holocron_core::zero::wipe(&mut a.body);
        holocron_core::zero::wipe(&mut out);
        let mut l = Line::new(b"[holocron] relay verb=");
        l.put(verb_name(p.verb)).put(b" caller=").put(if caller == me { b"owner" as &[u8] } else { b"other" }).put(b" -> ").put(status_name(a.status));
        if s != 0 {
            l.put(b" send=").dec(s);
        }
        l.wire();
        r[..n as usize].fill(0);
    }
}

/// WINDOW2 (`tests window`, kdf_ring3): derive the una-abi KAT key at Holocron's metal parameters IN RING 3
/// (the production `derive_ring3`, nothing else) and exit with it: bits 8..30 = key bytes 0..3 (LE) & 0x7FFFFF00,
/// low byte 1 = derived in ring 3, 2 = the window refused the memory, 4 = Argon2 refused a parameter. The kernel
/// recomputes the key through SYS_KDF's body and compares. No session, no store, no bus.
fn kdf_selftest() -> ! {
    let t0 = vein_ring3::sys::now_ms();
    let (flags, k) = match derive_ring3(una_abi::WINDOW2_KAT_PW, &una_abi::WINDOW2_KAT_SALT, &METAL_KDF) {
        Some(Ok(key)) => {
            let b = key.bytes();
            (1u64, u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64)
        }
        None => (2, 0),
        Some(Err(_)) => (4, 0),
    };
    let ms = vein_ring3::sys::now_ms().saturating_sub(t0);
    let mut l = Line::new(b"[holocron] kdf-selftest where=ring3 m_kib=");
    l.dec(METAL_KDF.m_kib as i64).put(b" t=").dec(METAL_KDF.t as i64).put(b" p=").dec(METAL_KDF.p as i64).put(b" ms=").dec(ms as i64);
    l.put(match flags {
        1 => b" -> derived" as &[u8],
        2 => b" -> refused (the window could not lend the memory)",
        _ => b" -> refused (parameters)",
    });
    l.wire();
    exit((k & 0x7FFF_FF00) | flags)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    write(b":: HOLOCRON: panic -> FAIL ::\n");
    exit(3)
}

/// EXECNAME (B322, R82): RESIDENT (bit 1) — a bare `holocron …` detaches; Holocron is the one resident
/// fulfiller R82 allows. Kept by the x86 link script under a PT_NOTE header.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(una_abi::APP_FLAG_RESIDENT);

// ---- RINGLOGIN (rmbp-ledger B465) --------------------------------------------------------------------------
// The ring opens WITH THE LOGIN. The kernel's login derived the ring key ONCE from the typed password (SYS_KDF's
// body, at the ring header's salt and parameters, or a fresh salt at METAL_KDF for a user with no ring yet) and
// posted it in a take-once door; this process — the registered fulfiller, running as the door's user — takes it
// through SYS_RINGKEY (67) and applies it with `Holocron::unlock_with_key` (create or open: holocron_core's ONE
// ring code). The lock screen posts a LOCK. The outcome goes back to the kernel, which says the witness
// (`[holocron] ring=<created|opened> at=login user=<u> ms=<n>`). The key is wiped on every path.

fn ringlogin_door(svc: &mut Svc) {
    let mut b = [0u8; una_abi::RINGKEY_LEN];
    let r = sys(una_abi::SYS_RINGKEY, una_abi::RINGKEY_OP_TAKE, b.as_mut_ptr() as u64, b.len() as u64, 0);
    if r == una_abi::RINGKEY_LOCK {
        svc.lock_now();
        sys(una_abi::SYS_RINGKEY, una_abi::RINGKEY_OP_REPORT, 0, una_abi::RINGKEY_MODE_LOCKED as u64, 0);
        return;
    }
    if r != una_abi::RINGKEY_KEY {
        return; // nothing posted, or not ours (-EACCES), or no SYS_RINGKEY in this kernel (-ENOSYS)
    }
    let door = una_abi::ringdoor_parse(&b);
    holocron_core::zero::wipe(&mut b);
    let Some(mut d) = door else {
        sys(una_abi::SYS_RINGKEY, una_abi::RINGKEY_OP_REPORT, wire::status::INVALID as i64 as u64, 0, 0);
        return;
    };
    let params = KdfParams { m_kib: d.m_kib, t: d.t, p: d.p };
    let key = Key::from_bytes(d.key);
    holocron_core::zero::wipe(&mut d.key);
    if d.mode == una_abi::RINGKEY_MODE_REKEY {
        // RINGLOGIN2 (B479): a password change — holocron_core's ONE re-wrap (salt and parameters kept; a failure
        // keeps the old ring). The kernel says `ring=rekeyed at=passwd` or the refusal on this report.
        let old = Key::from_bytes(d.old_key);
        holocron_core::zero::wipe(&mut d.old_key);
        let st = match svc.rekey_with_keys(old, key) {
            Ok(_) => 0,
            Err(s) => s,
        };
        sys(una_abi::SYS_RINGKEY, una_abi::RINGKEY_OP_REPORT, st as i64 as u64, d.mode as u64, 0);
        return;
    }
    let create = d.mode == una_abi::RINGKEY_MODE_CREATE;
    let st = match svc.unlock_with_key(create, d.salt, params, key) {
        Ok(()) => 0,
        Err(s) => s,
    };
    sys(una_abi::SYS_RINGKEY, una_abi::RINGKEY_OP_REPORT, st as i64 as u64, d.mode as u64, 0);
}
