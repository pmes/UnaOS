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
// I/O: the store is `<home>/.config/unaos/holocron/` (`.ring`, `<ns>/<name>`) over SYS_PATH_READ/WRITE
// (`PATH_W_TRUNC|MKDIRS`, `PATH_W_UNLINK`, `PATH_R_LIST`), REFUSED unless `<home>` stats with an inode id
// (UnaFS: FAT has no owner, so a ring there would be readable by anything that mounts the card). Entropy:
// CRYPTOCORE's ChaCha20 DRBG over SYS_GETRANDOM (`holocron_core::cc::DrbgEntropy`). The ring KDF: SYS_KDF
// (the kernel runs CRYPTOCORE's Argon2id — 19..=64 MiB does not fit the 4 MiB window). Owner: SYS_WHOAMI.
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

/// `<home>/.config/unaos/holocron/` on UnaFS — the host layout (`.ring`, `<ns>/<name>`) under the
/// per-user config root the rest of the metal uses.
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
    fn remove(&mut self, ns: &str, name: &str) -> Result<bool, StoreError> {
        self.gate()?;
        let req = path_req(&self.file(ns, name), PATH_W_UNLINK, 0, &[]).ok_or(StoreError)?;
        match sys(SYS_PATH_WRITE, req.as_ptr() as u64, req.len() as u64, 0, 0) {
            r if r >= 0 => Ok(true),
            ENOENT => Ok(false),
            _ => Err(StoreError),
        }
    }
}

// ---- the suite: CRYPTOCORE, the KDF through the kernel ---------------------------------------------------

/// `CryptoCore` with `derive_key` routed to SYS_KDF: the SAME Argon2id (crypto_core's), run where its memory fits.
#[derive(Clone, Copy)]
struct MetalSealer;

impl Sealer for MetalSealer {
    const SUITE: u8 = <CryptoCore as Sealer>::SUITE;

    fn derive_key(&self, password: &[u8], salt: &[u8; SALT_LEN], params: &KdfParams) -> Result<Key, SealError> {
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
    let store = PathStore { root: alloc::format!("{}/.config/unaos/holocron", who.home), ok: home_unafs };
    let Ok(rng) = rng() else {
        write(b":: HOLOCRON: entropy=refused (SYS_GETRANDOM could not seed the DRBG) -> refused ::\n");
        exit(1);
    };
    let mut svc: Svc = Holocron::new(MetalSealer, CryptoCore, store, rng, who.owner.clone(), KdfParams::DEFAULT);
    let me = Some(who.owner.as_str());
    if let Some(r) = &req {
        let mut a = svc.handle(me, r.verb(), &r.encode_body(), vein_ring3::sys::now_ms(), now_unix());
        report(r.verb(), a.status, &a.body, b"in this Holocron");
        holocron_core::zero::wipe(&mut a.body);
    }
    let st = svc.handle(me, wire::VERB_STATUS, &[], vein_ring3::sys::now_ms(), now_unix());
    let state = wire::decode_status(&st.body).map(|(s, _, _)| state_name(s)).unwrap_or(b"?");
    let mut l = Line::new(b":: HOLOCRON: serve ring=");
    l.put(if home_unafs { b"unafs" as &[u8] } else { b"none" }).put(b" verbs=").dec(wire::VERBS.len() as i64);
    l.put(b" owner=").put(who.user.as_bytes()).put(b" state=").put(state).put(b" ::");
    l.wire();
    loop {
        let n = recv();
        if n < 0 {
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
