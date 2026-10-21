//! CHARTER: Principia — fulfiller
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SETTINGSBUS (rmbp-ledger B337; B287's last leg) — the kernel's preference CLIENT. Principia owns every
// settings and preference decision (LAWS §Handler manifest); PREFS (B300) gave the kernel one store in
// Principia's format and the bus verbs PREF_GET / PREF_SET / PREF_LIST (una-abi 16..18). This file is how
// the kernel's OWN desktop (the Settings window, the dock pins, the `wallpaper` verb, the brightness and
// volume key sync) speaks to that store: as a bus client, never by calling the store directly.
//
// Every call builds a v1 frame (`bus::build_request`), runs it through the frozen `frame_parse` +
// `request_validate` exactly as a ring-3 frame is checked, and routes it:
//
//   1. under `busreg`, the same body is offered to `bus_route::route_request` under Principia's RING-3 tag
//      (R3PREF_GET 128 / R3PREF_LIST 129 / R3PREF_SET 130) from the reserved kernel-client row
//      [`KCLIENT_ROW`], stamped with the WINDOW'S USER ([`principal`]: kind 5 `user:<name>`, the session
//      record's wire image — the kernel stamps, nothing claims). When PREFS.ELF holds the tag the frame is
//      relayed to its mailbox and its kernel-stamped answer comes back into this client's inbox
//      (`bus_route`'s deliver knows the client row): `via=prefs.elf`;
//   2. otherwise (nobody holds the tag: `-ENOENT`; a full mailbox: `-EAGAIN`; the knob off; or the relay
//      did not answer inside [`RELAY_WAIT_MS`]) the kernel's own fulfiller answers — `prefs::bus_fulfil`,
//      the SAME function both syscall dispatchers call for 16..18 — and its answer is built and parsed as
//      the REPLY frame the wire would carry: `via=kernel`. A relayed SET that times out is NOT re-done by
//      the kernel: it is queued at PREFS.ELF and lands in order (a kernel write could be overtaken by it).
//
// PrefChanged (verb 19): `prefs::changed` (every accepted write, whoever wrote it) calls [`on_changed`],
// which builds the frame of record (kind REPLY, corr 0, status 0, the kernel reply principal, body
// `<ns>.<key>` NUL `<literal>`) into the SUBSCRIPTION queue while a kernel subscriber is registered
// ([`subscribe`]). The Settings window drains it ([`changes_drain`]) and repaints from a fresh get.
// Ring-3 delivery (PREFCHANGED B558, the file tail): a verb-19 REQUEST registers a prefix, each match is pushed.
//
// Design of record: docs/dev/evidence/rmbp-1005/SETTINGSBUS.md.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::bus::{BUS_HDR_LEN, BUS_KIND_REPLY};
pub use crate::prefs::PrefValue;

/// The kernel client's mailbox key in `bus_route` — outside every arch's row range (the arch ops refuse
/// it: push `false`, generation `u64::MAX`), so only `bus_route`'s client-row branch can deliver to it.
pub const KCLIENT_ROW: usize = usize::MAX;
/// The client row's generation: what the arch `rgen` answers for an out-of-range row, so `bus_route`'s
/// caller-alive fence holds for the client exactly as it does for a live ring-3 caller.
pub const KCLIENT_GEN: u64 = u64::MAX;
/// How long a desktop pass may wait for a ring-3 fulfiller's answer before the kernel fulfiller answers.
pub const RELAY_WAIT_MS: u64 = 50;
/// Spin ceiling for the wait (the clock may not advance on a path with interrupts masked).
const RELAY_SPIN_MAX: u32 = 4_000_000;
/// Queue depth of the reply inbox and of the PrefChanged subscription queue.
const Q_CAP: usize = 16;

const PRIN_KERNEL_REPLY: u8 = 4;
/// The session principal kind (aarch64 `PRIN_USER`): value `user:<name>`.
const PRIN_USER: u8 = 5;

const EINVAL: i64 = -22;
const EIO: i64 = -5;

/// Witness counters (monotonic since boot).
pub static VIA_KERNEL: AtomicU32 = AtomicU32::new(0);
pub static VIA_R3: AtomicU32 = AtomicU32::new(0);
pub static RELAY_TIMEOUT: AtomicU32 = AtomicU32::new(0);
pub static CHANGED_N: AtomicU32 = AtomicU32::new(0);
pub static CHANGED_DROPPED: AtomicU32 = AtomicU32::new(0);
/// The last call was answered by the ring-3 fulfiller.
static LAST_VIA_R3: AtomicBool = AtomicBool::new(false);
static NEXT_CORR: AtomicU32 = AtomicU32::new(1);
static SUBSCRIBED: AtomicBool = AtomicBool::new(false);

static REPLIES: crate::sync::Mutex<VecDeque<Box<[u8]>>> = crate::sync::Mutex::new(VecDeque::new());
static CHANGES: crate::sync::Mutex<VecDeque<Box<[u8]>>> = crate::sync::Mutex::new(VecDeque::new());

#[inline]
fn locked<T>(f: impl FnOnce() -> T) -> T {
    crate::arch::without_interrupts(f)
}

/// The window's user (the session user), or `None` (no session / no `login`).
fn user_name() -> Option<String> {
    #[cfg(feature = "login")]
    {
        let mut nm = [0u8; crate::fs::users::NAME_MAX];
        let n = crate::fs::users::whoami(&mut nm)?;
        return core::str::from_utf8(&nm[..n]).ok().map(String::from);
    }
    #[cfg(not(feature = "login"))]
    {
        None
    }
}

/// The principal the kernel stamps on the client's frames: kind 5, `user:<the window's user>` (empty
/// name = no session). Never a claim: the kernel builds it from the session table.
pub fn principal() -> [u8; 32] {
    let mut p = [0u8; 32];
    p[0] = PRIN_USER;
    let mut n = 0usize;
    for &c in b"user:".iter().chain(user_name().unwrap_or_default().as_bytes()) {
        if n < 30 {
            p[2 + n] = c;
            n += 1;
        }
    }
    p[1] = n as u8;
    p
}

fn kernel_reply_principal() -> [u8; 32] {
    let mut p = [0u8; 32];
    p[0] = PRIN_KERNEL_REPLY;
    p
}

/// May the client SET? The window acts for the session user: with `login`, a session must be open (the
/// same rule `pref_caller_in_session` applies to a ring-3 slot); without `login` there are no sessions and
/// the console's desktop owns the (homeless) store, as it did before this arc.
fn in_session() -> bool {
    if cfg!(feature = "login") { user_name().is_some() } else { true }
}

// ── The inbox (relay answers) and the subscription queue (PrefChanged) ─────────────────────────

/// `bus_route` delivers a relay answer for [`KCLIENT_ROW`] here. `false` = full.
pub fn inbox_push(frame: Box<[u8]>) -> bool {
    locked(|| {
        let mut q = REPLIES.lock();
        if q.len() >= Q_CAP {
            return false;
        }
        q.push_back(frame);
        true
    })
}

/// Room for one more relay answer.
pub fn inbox_has_room() -> bool {
    locked(|| REPLIES.lock().len() < Q_CAP)
}

/// Take the answer to `corr` (dropping stale answers to earlier, timed-out calls on the way).
fn take_reply(corr: u32) -> Option<Box<[u8]>> {
    locked(|| {
        let mut q = REPLIES.lock();
        while let Some(f) = q.pop_front() {
            if matches!(crate::bus::frame_parse(&f), Ok(h) if h.kind == BUS_KIND_REPLY && h.corr == corr) {
                return Some(f);
            }
        }
        None
    })
}

/// Register the kernel subscriber (the Settings window). Idempotent.
pub fn subscribe() {
    SUBSCRIBED.store(true, Ordering::Release);
}

/// PrefChanged: called by the store on every accepted write. Builds the verb-19 frame of record into the
/// subscription queue (oldest dropped and counted when full). No subscriber = nothing queued. PREFSUNSET (SR169):
/// `a = None` = the key left the store with no preference-level default: the frame carries the unset form.
pub fn on_changed(ns: &str, k: &str, a: Option<&prefs_core::schema::Applied>) {
    let kq = SUBSCRIBED.load(Ordering::Acquire); if !kq && R3_SUBS.load(Ordering::Acquire) == 0 { // PREFCHANGED (B558): a kernel OR a ring-3 subscriber
        return;
    }
    // PREFSKERNEL (B345): the shared body — `<ns>.<key>` NUL `<literal>` [NUL `clamped=true`].
    let body = match a { Some(a) => prefs_core::wire::changed_body_applied(ns, k, a), None => prefs_core::wire::unset_body(ns, k) }; // PREFSUNSET (SR169): or `<ns>.<key>` NUL, the empty literal slot
    if body.len() > crate::bus::BUS_BODY_MAX {
        return;
    }
    let mut f = alloc::vec![0u8; BUS_HDR_LEN + body.len()];
    let n = crate::bus::build_reply(una_abi::BUS_VERB_PREF_CHANGED, 0, 0, kernel_reply_principal(), &body, &mut f);
    f.truncate(n);
    let frame = f.into_boxed_slice(); r3_deliver(ns, k, &frame); if !kq { return; } #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] note_changed(ns, k); // LOCKCHANGED (B758): the key beside CHANGED_N (file tail). PREFCHANGED (B558): the same frame of record into every subscribed ring-3 owner's mailbox (file tail). LINE-NEUTRAL append.
    locked(|| {
        let mut q = CHANGES.lock();
        if q.len() >= Q_CAP {
            q.pop_front();
            CHANGED_DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        q.push_back(frame);
    });
    CHANGED_N.fetch_add(1, Ordering::Relaxed);
}

/// Drain the subscription queue: each frame is parsed through the frozen `frame_parse` (verb 19, kind
/// REPLY, corr 0) and its body through the shared `wire::parse_changed` (a value or the unset form, PREFSUNSET
/// SR169); `f(ns, key)` per change — the consumer re-reads the store, so an unset key answers its default.
/// Returns the frames delivered.
pub fn changes_drain(mut f: impl FnMut(&str, &str)) -> usize {
    let frames: Vec<Box<[u8]>> = locked(|| CHANGES.lock().drain(..).collect());
    let mut n = 0usize;
    for fr in frames.iter() {
        let ok = matches!(crate::bus::frame_parse(fr), Ok(h) if h.kind == BUS_KIND_REPLY && h.verb == una_abi::BUS_VERB_PREF_CHANGED && h.corr == 0 && h.status == 0);
        if !ok {
            continue;
        }
        let body = &fr[BUS_HDR_LEN..];
        if let Some((ns, k, _, _)) = prefs_core::wire::parse_changed(body) {
            f(ns, k);
            n += 1;
        }
    }
    n
}

// ── The call: build, validate, route, answer ──────────────────────────────────────────────────

struct Answer {
    status: i64,
    body: Vec<u8>,
}

/// Principia's ring-3 tag for a kernel-range PREF verb.
#[cfg(feature = "busreg")]
fn r3_tag(verb: u8) -> Option<u8> {
    match verb {
        una_abi::BUS_VERB_PREF_GET => Some(una_abi::BUS_VERB_R3PREF_GET),
        una_abi::BUS_VERB_PREF_LIST => Some(una_abi::BUS_VERB_R3PREF_LIST),
        una_abi::BUS_VERB_PREF_SET => Some(una_abi::BUS_VERB_R3PREF_SET),
        _ => None,
    }
}

/// Offer the frame to Principia's ring-3 fulfiller through the router. `None` = nobody answered (no
/// registration, a refusal, or the wait ran out) — the kernel fulfiller answers instead.
#[cfg(feature = "busreg")]
fn relay(hdr: &crate::bus::BusHdr, body: &[u8]) -> Option<Answer> {
    let tag = r3_tag(hdr.verb)?;
    let ops = crate::arch::syscall::busreg_ops();
    let h3 = crate::bus::BusHdr { verb: tag, ..*hdr };
    match crate::bus_route::route_request(ops, KCLIENT_ROW, KCLIENT_GEN, principal(), &h3, body) {
        crate::bus_route::Route::Relayed => {}
        _ => return None,
    }
    let t0 = crate::arch::ms();
    let mut spins = 0u32;
    loop {
        if let Some(fr) = take_reply(hdr.corr) {
            let h = crate::bus::frame_parse(&fr).ok()?;
            if h.principal[0] != PRIN_KERNEL_REPLY {
                return None; // only a KERNEL-stamped reply is an answer
            }
            return Some(Answer { status: h.status as i64, body: fr[BUS_HDR_LEN..].to_vec() });
        }
        spins += 1;
        if crate::arch::ms().saturating_sub(t0) >= RELAY_WAIT_MS || spins >= RELAY_SPIN_MAX {
            RELAY_TIMEOUT.fetch_add(1, Ordering::Relaxed);
            if hdr.verb == una_abi::BUS_VERB_PREF_SET {
                // The frame is already in PREFS.ELF's mailbox and WILL land: a kernel write now could be
                // overtaken by this queued one (set A times out, set B lands, A lands = A wins). One queue,
                // one order — the set is accepted as in flight; its PrefChanged tells the window when it lands.
                serial_println!("[prefsbus] relay verb={} corr={} timeout={}ms — set queued at prefs.elf", tag, hdr.corr, RELAY_WAIT_MS);
                return Some(Answer { status: 0, body: Vec::new() });
            }
            serial_println!("[prefsbus] relay verb={} corr={} timeout={}ms — kernel fulfiller answers", tag, hdr.corr, RELAY_WAIT_MS);
            return None;
        }
        core::hint::spin_loop();
    }
}

/// The kernel's own fulfiller — the function the dispatchers call — answering as the wire would.
fn kernel(hdr: &crate::bus::BusHdr, body: &[u8]) -> Answer {
    let mut text: Vec<u8> = Vec::new();
    let st = crate::prefs::bus_fulfil(hdr.verb, body, in_session(), &mut text);
    let b: &[u8] = if st == 0 { &text } else { &[] };
    let mut r = alloc::vec![0u8; BUS_HDR_LEN + b.len()];
    let n = crate::bus::build_reply(hdr.verb, hdr.corr, st as i32, kernel_reply_principal(), b, &mut r);
    match crate::bus::frame_parse(&r[..n]) {
        Ok(h) if h.kind == BUS_KIND_REPLY && h.corr == hdr.corr => Answer { status: h.status as i64, body: r[BUS_HDR_LEN..n].to_vec() },
        _ => Answer { status: EIO, body: Vec::new() },
    }
}

fn call(verb: u8, body: &[u8]) -> Answer {
    if body.len() > crate::bus::BUS_BODY_MAX {
        return Answer { status: EINVAL, body: Vec::new() };
    }
    let mut corr = NEXT_CORR.fetch_add(1, Ordering::Relaxed);
    if corr == 0 {
        corr = NEXT_CORR.fetch_add(1, Ordering::Relaxed); // corr 0 is PrefChanged's
    }
    let mut f = alloc::vec![0u8; BUS_HDR_LEN + body.len()];
    let n = crate::bus::build_request(verb, corr, body, &mut f);
    let hdr = match crate::bus::frame_parse(&f[..n]) {
        Ok(h) if crate::bus::request_validate(&h).is_ok() => h,
        _ => return Answer { status: EINVAL, body: Vec::new() },
    };
    #[cfg(feature = "busreg")]
    {
        if let Some(a) = relay(&hdr, &f[BUS_HDR_LEN..n]) {
            LAST_VIA_R3.store(true, Ordering::Relaxed);
            VIA_R3.fetch_add(1, Ordering::Relaxed);
            return a;
        }
    }
    LAST_VIA_R3.store(false, Ordering::Relaxed);
    VIA_KERNEL.fetch_add(1, Ordering::Relaxed);
    kernel(&hdr, &f[BUS_HDR_LEN..n])
}

/// `kernel` or `prefs.elf`: who answered the last call.
pub fn last_via() -> &'static str {
    if LAST_VIA_R3.load(Ordering::Relaxed) { "prefs.elf" } else { "kernel" }
}

// ── The API ───────────────────────────────────────────────────────────────────────────────────

/// PrefGet `ns.key` over the bus: the value, or the reply's errno (-ENOENT unset).
pub fn pref_get(ns: &str, k: &str) -> Result<PrefValue, i64> {
    let a = call(una_abi::BUS_VERB_PREF_GET, alloc::format!("{}.{}", ns, k).as_bytes());
    if a.status != 0 {
        return Err(a.status);
    }
    let lit = core::str::from_utf8(&a.body).map_err(|_| EINVAL)?;
    Ok(PrefValue::from_literal(lit).unwrap_or_else(|_| PrefValue::infer(lit)))
}

/// PrefSet `ns.key = v` over the bus. `Ok` = the store accepted it (a PrefChanged follows a change).
pub fn pref_set(ns: &str, k: &str, v: PrefValue) -> Result<(), i64> {
    let body = alloc::format!("{}.{}\0{}", ns, k, v.to_literal());
    let a = call(una_abi::BUS_VERB_PREF_SET, body.as_bytes());
    // PREFSKERNEL (B345): a clamped SET answers `<stored>` NUL `clamped=true` (prefs_core::wire).
    match prefs_core::wire::parse_set_reply(&a.body) {
        Some((stored, true)) if a.status == 0 => serial_println!("[prefsbus] set {}.{}={} via={} status=0 clamped=1 stored={}", ns, k, v, last_via(), stored),
        _ => serial_println!("[prefsbus] set {}.{}={} via={} status={}", ns, k, v, last_via(), a.status),
    }
    if a.status == 0 { Ok(()) } else { Err(a.status) }
}

/// PrefList over the bus: `(ns, key, value)` per line (`<ns>.<key> = <literal>`; a fulfiller's bare
/// `key=value` line is read with an empty namespace).
pub fn pref_list(ns: Option<&str>) -> Result<Vec<(String, String, PrefValue)>, i64> {
    let a = call(una_abi::BUS_VERB_PREF_LIST, ns.unwrap_or("").as_bytes());
    if a.status != 0 {
        return Err(a.status);
    }
    let mut out = Vec::new();
    for line in core::str::from_utf8(&a.body).unwrap_or("").lines() {
        let Some((addr, lit)) = line.split_once('=') else { continue };
        let (addr, lit) = (addr.trim(), lit.trim());
        let v = PrefValue::from_literal(lit).unwrap_or_else(|_| PrefValue::infer(lit));
        match crate::prefs::split_addr(addr) {
            Some((n, k)) => out.push((String::from(n), String::from(k), v)),
            None => out.push((String::new(), String::from(addr), v)),
        }
    }
    Ok(out)
}

/// `system.<k>` over the bus.
pub fn sys_get(k: &str) -> Option<PrefValue> {
    if let Some(v) = deferred_value(k) {
        return Some(v); // INPUTSTALL M5: read-your-writes while the set is still on its way to the store
    }
    pref_get(crate::prefs::NS, k).ok()
}
/// `system.<k>` as an integer in `lo..=hi` (unset, another type or out of range = `None`).
pub fn sys_int(k: &str, lo: i64, hi: i64) -> Option<i64> {
    sys_get(k).and_then(|v| v.as_int()).filter(|x| (lo..=hi).contains(x))
}
/// `system.<k>` as a bool.
pub fn sys_flag(k: &str) -> Option<bool> {
    sys_get(k).and_then(|v| v.as_bool())
}
/// `system.<k>` as a string.
pub fn sys_text(k: &str) -> Option<String> {
    sys_get(k).and_then(|v| v.as_str().map(String::from))
}
/// Set `system.<k>`; a refusal is already on the serial line (`[prefsbus] set …`), so callers that only
/// persist ignore it.
pub fn sys_set(k: &str, v: PrefValue) {
    let Some(v) = defer_set(k, v) else { return }; // INPUTSTALL M5: from the render task the PrefSet goes to a worker
    let _ = pref_set(crate::prefs::NS, k, v);
}

// ── direct_writers: a COMPILE-TIME count ──────────────────────────────────────────────────────

/// Occurrences of `pat` in `hay` (non-overlapping). Const, so the count is the compiler's.
const fn count(hay: &[u8], pat: &[u8]) -> usize {
    let mut n = 0usize;
    let mut i = 0usize;
    while i + pat.len() <= hay.len() {
        // Cheap first-byte reject (the shell.rs FATVERB scan's shape) keeps const-eval linear and fast.
        if hay[i] != pat[0] {
            i += 1;
            continue;
        }
        let mut j = 0usize;
        while j < pat.len() && hay[i + j] == pat[j] {
            j += 1;
        }
        if j == pat.len() {
            n += 1;
            i += pat.len();
        } else {
            i += 1;
        }
    }
    n
}

/// The store's direct write path as source text (`set` and `set_sys` both begin with it).
const WRITE_PAT: &[u8] = b"prefs::set";

/// Direct store-write call sites OUTSIDE `prefs.rs` and this file, counted by the compiler over every
/// kernel source that names the store (`grep -l 'crate::prefs' src` at this commit, plus the desktop
/// files that persist a preference). The `tests settingsbus` line prints it; 0 = one path.
#[allow(long_running_const_eval)]
pub const DIRECT_WRITERS: usize = count(include_bytes!("video/settings.rs"), WRITE_PAT)
    + count(include_bytes!("video/dock.rs"), WRITE_PAT)
    + count(include_bytes!("video/powerui.rs"), WRITE_PAT)
    + count(include_bytes!("video/wallpaper.rs"), WRITE_PAT)
    + count(include_bytes!("video/dimidle.rs"), WRITE_PAT)
    + count(include_bytes!("video/backlight.rs"), WRITE_PAT)
    + count(include_bytes!("video/brightkeys.rs"), WRITE_PAT)
    + count(include_bytes!("video/status.rs"), WRITE_PAT)
    + count(include_bytes!("drivers/hda_amp.rs"), WRITE_PAT)
    + count(include_bytes!("lumen.rs"), WRITE_PAT)
    + count(include_bytes!("tests.rs"), WRITE_PAT)
    + count(include_bytes!("shell.rs"), WRITE_PAT)
    + ARCH_SYSCALL_WRITERS;
/// This arch's syscall file only (the other arch's is not in this image; its own build counts it).
#[cfg(target_arch = "x86_64")]
#[allow(long_running_const_eval)]
const ARCH_SYSCALL_WRITERS: usize = count(include_bytes!("arch/x86_64/syscall.rs"), WRITE_PAT);
#[cfg(target_arch = "aarch64")]
#[allow(long_running_const_eval)]
const ARCH_SYSCALL_WRITERS: usize = count(include_bytes!("arch/aarch64/syscall.rs"), WRITE_PAT);

// ── `tests settingsbus` ───────────────────────────────────────────────────────────────────────

/// Register `tests settingsbus` exactly once, wherever the Settings window builds.
pub fn ensure_tests() {
    #[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) {
            crate::tests::register("settingsbus", selftest); #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))] crate::tests::register("prefchanged", fx::selftest); // PREFCHANGED (B558): beside its parent's fixture (fixture file prefchanged_fx.rs). LINE-NEUTRAL append.
        }
    }
}

/// SETTINGSBUS: set `system.display.idle_min` through the bus client, read it back through the WINDOW'S
/// read path (`settings::from_prefs`), assert equality, assert a PrefChanged for that key reached the
/// window's subscription and the window's shown value moved to it; restore the operator's value.
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn selftest() {
    use crate::prefs::key;
    use crate::video::settings;
    if let Some(k) = crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &crate::prefs::path()) { serial_println!(":: SETTINGSBUS: via=- store={} -> SKIP reason=home-not-writable({}) ::", crate::prefs::path(), k); return; } subscribe(); // QEMUSKIP (B613): `set=` is a save to <home>/settings
    let _ = settings::bus_changes(); // earlier changes are not this fixture's
    let live0 = crate::video::dimidle::idle_min() as i64;
    let before = sys_int(key::IDLE_MIN, 0, 1440);
    let orig = before.unwrap_or(live0);
    let want: i64 = if orig == 7 { 9 } else { 7 };
    let c0 = CHANGED_N.load(Ordering::Relaxed);
    let set_ok = pref_set(crate::prefs::NS, key::IDLE_MIN, PrefValue::Int(want)).is_ok();
    let via = last_via();
    let (_frames, idle_seen, shown) = settings::bus_changes_idle();
    let mut v = settings::Values::current();
    let mask = settings::from_prefs(&mut v).0;
    let get_eq = mask & (1 << 3) != 0 && v.idle_min as i64 == want;
    let changed = idle_seen;
    let moved = shown == want as u32;
    let fired = CHANGED_N.load(Ordering::Relaxed).wrapping_sub(c0);
    // Restore the operator's value; the window follows it through the same PrefChanged path.
    let _ = pref_set(crate::prefs::NS, key::IDLE_MIN, PrefValue::Int(orig));
    let _ = settings::bus_changes();
    let pass = set_ok && get_eq && changed == 1 && moved && DIRECT_WRITERS == 0;
    serial_println!(
        "[settingsbus] want={} orig={} fired={} moved={} via_kernel={} via_r3={} relay_timeout={} dropped={}",
        want, orig, fired, moved as u8, VIA_KERNEL.load(Ordering::Relaxed), VIA_R3.load(Ordering::Relaxed),
        RELAY_TIMEOUT.load(Ordering::Relaxed), CHANGED_DROPPED.load(Ordering::Relaxed)
    );
    serial_println!(
        ":: SETTINGSBUS: via={} set={} get={} changed={} direct_writers={} -> {} ::",
        via, set_ok as u8, if get_eq { "eq" } else { "ne" }, changed, DIRECT_WRITERS, if pass { "PASS" } else { "FAIL" }
    );
}

// ── INPUTSTALL M5 (rmbp-ledger B375, R88): a PrefSet never runs on the render task ─────────────────────
//
// FLIGHT 23: a Settings slider press ran its PrefSet over the bus — a UnaFS write — inside the press handler
// on the RENDER task, the one consumer of the input channel: `[lag] click→shown ms=1478.0 … wm=1471.4`, six
// presses in a row, each ~1.3–1.5 s with every key and press behind it. `sys_set` is fire-and-forget (its
// callers persist and ignore the answer), so from the render task it now QUEUES the write (the latest value
// per key wins — a drag of the slider is one write, not twenty) and a `prefs-flush` kernel task on a worker
// core (`smp::worker_cpu(0)`, never the render core) runs the PrefSets. `sys_get` answers a queued key from
// the queue (read-your-writes), so a caller that reads back right after its set sees what it set. Off the
// render task, off x86, or with no worker core, `sys_set` is the synchronous PrefSet it always was.
//
// Witness per flush: `[prefsbus] flush n=<keys> ms=<n> on=worker queued_ms=<oldest wait> (INPUTSTALL M5)`.
// This is the seam, not `settings.rs`: every kernel `sys_set` caller on the render task is covered.

static DEFERRED: crate::sync::Mutex<Vec<(String, PrefValue)>> = crate::sync::Mutex::new(Vec::new());
#[cfg(target_arch = "x86_64")]
static FLUSHER: AtomicBool = AtomicBool::new(false);
static DEFER_T0_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The worker core a deferred PrefSet runs on, when the caller is the render task (x86). `None` = run inline.
fn defer_core() -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    {
        if !matches!(crate::arch::sched::current_name(), Some(n) if n.starts_with("render")) {
            return None;
        }
        crate::arch::smp::worker_cpu(0)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        None
    }
}

/// Queue `system.<k> = v` for the worker; `Some(v)` back = the caller runs it inline.
fn defer_set(k: &str, v: PrefValue) -> Option<PrefValue> {
    let Some(cpu) = defer_core() else { return Some(v) };
    {
        let mut q = DEFERRED.lock();
        if q.is_empty() {
            DEFER_T0_MS.store(crate::arch::ms(), Ordering::Relaxed);
        }
        match q.iter_mut().find(|(key, _)| key == k) {
            Some(slot) => slot.1 = v,
            None => q.push((String::from(k), v)),
        }
    }
    #[cfg(target_arch = "x86_64")]
    if !FLUSHER.swap(true, Ordering::AcqRel) {
        crate::arch::sched::spawn("prefs-flush", prefs_flush, 0, cpu, crate::arch::sched::PRIO_NORMAL);
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = cpu;
    None
}

/// A queued value for `system.<k>`, if one is waiting.
fn deferred_value(k: &str) -> Option<PrefValue> {
    let q = DEFERRED.lock();
    q.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone())
}

/// `prefs-flush`: run every queued PrefSet (in order, latest value per key), then exit; a set queued while
/// it was finishing re-arms it.
#[cfg(target_arch = "x86_64")]
fn prefs_flush(_: usize) {
    loop {
        let (batch, t0) = {
            let q = DEFERRED.lock();
            (q.clone(), DEFER_T0_MS.load(Ordering::Relaxed))
        };
        if batch.is_empty() {
            FLUSHER.store(false, Ordering::Release);
            if DEFERRED.lock().is_empty() || FLUSHER.swap(true, Ordering::AcqRel) {
                return;
            }
            continue;
        }
        let start = crate::arch::ms();
        for (k, v) in &batch {
            let _ = pref_set(crate::prefs::NS, k, v.clone());
            // Drop the key from the queue only if nobody re-set it meanwhile (else the newer value flushes next).
            let mut q = DEFERRED.lock();
            if let Some(i) = q.iter().position(|(key, qv)| key == k && qv == v) {
                q.remove(i);
            }
        }
        serial_println!(
            "[prefsbus] flush n={} ms={} on=worker queued_ms={} (INPUTSTALL M5: the PrefSet ran off the render task)",
            batch.len(), crate::arch::ms().saturating_sub(start), start.saturating_sub(t0)
        );
    }
}

// ── BTKEYSEAL (rmbp-ledger B446): a raw relay for another handler's ring-3 tag ─────────────────────
// The kernel-client row is the ONE row `bus_route` delivers kernel-originated answers to; a second
// kernel client (the Bluetooth bond store asking Holocron) shares it — and NEXT_CORR, so the answers
// never collide — instead of minting a second row. `prin` is the kernel's stamp for the caller (built from
// the session table by the client, never claimed). The outgoing frame is wiped before it is dropped (a
// SecretPut carries key bytes); the answer's frame too. Errors: the router's own errno (no fulfiller =
// -ENOENT, full = -EAGAIN), -EINVAL for a tag below the registrable range, -ETIMEDOUT past `wait_ms`.
#[cfg(feature = "busreg")]
pub fn relay_tag(tag: u8, prin: [u8; 32], body: &[u8], wait_ms: u64) -> Result<(i32, Vec<u8>), i64> {
    const ETIMEDOUT: i64 = -110;
    if tag < una_abi::BUS_VERB_FULFIL_MIN || body.len() > crate::bus::BUS_BODY_MAX {
        return Err(EINVAL);
    }
    let mut corr = NEXT_CORR.fetch_add(1, Ordering::Relaxed);
    if corr == 0 {
        corr = NEXT_CORR.fetch_add(1, Ordering::Relaxed);
    }
    let mut f = alloc::vec![0u8; BUS_HDR_LEN + body.len()];
    let n = crate::bus::build_request(tag, corr, body, &mut f);
    let routed = match crate::bus::frame_parse(&f[..n]) {
        Ok(h) if crate::bus::request_validate(&h).is_ok() => {
            crate::bus_route::route_request(crate::arch::syscall::busreg_ops(), KCLIENT_ROW, KCLIENT_GEN, prin, &h, &f[BUS_HDR_LEN..n])
        }
        _ => crate::bus_route::Route::Reply(EINVAL),
    };
    for b in f.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    match routed {
        crate::bus_route::Route::Relayed => {}
        crate::bus_route::Route::Reply(e) => return Err(if e == 0 { EINVAL } else { e }),
        crate::bus_route::Route::Kernel => return Err(EINVAL),
    }
    let t0 = crate::arch::ms();
    let mut spins = 0u32;
    loop {
        if let Some(mut fr) = take_reply(corr) {
            let out = match crate::bus::frame_parse(&fr) {
                Ok(h) if h.principal[0] == PRIN_KERNEL_REPLY => Ok((h.status, fr[BUS_HDR_LEN..].to_vec())),
                _ => Err(EIO), // only a KERNEL-stamped reply is an answer
            };
            for b in fr.iter_mut() {
                unsafe { core::ptr::write_volatile(b, 0) };
            }
            return out;
        }
        spins += 1;
        if crate::arch::ms().saturating_sub(t0) >= wait_ms || spins >= RELAY_SPIN_MAX {
            RELAY_TIMEOUT.fetch_add(1, Ordering::Relaxed);
            return Err(ETIMEDOUT);
        }
        core::hint::spin_loop();
    }
}

// ── PREFCHANGED (rmbp-ledger B558): PrefChanged (verb 19) DELIVERED to ring 3 ──────────────────────────
//
// The wire is una-abi's PREFCHANGED block. A ring-3 program's verb-19 REQUEST (`SYS_MSEND`, body = a `<ns>.<key>`
// prefix) reaches [`subscribe_owner`] from each arch's MSEND arm with the caller's mailbox row (x86: the slot;
// aarch64: the ASID) and its generation; the prefixes live on that row of [`SUBS`] (a `procslot::SlotVec`, B399 — no
// pool width). `on_changed` (above, called by `prefs::changed` on every accepted write, whoever wrote it) builds the
// frame of record once and hands it to [`r3_deliver`], which pushes ONE copy per matching owner per write into its
// mailbox through the arch's `prefs_changed_push` (generation-stamped with the SUBSCRIBER's generation, so a frame
// raced past a teardown is discarded by MRECV; one slot kept for the owner's own replies; full = dropped, counted in
// [`R3_DROPPED`], said once). Teardown (`clear_handle_row` on both arches) calls [`unsubscribe_row`].

/// One owner's subscription: the generation it subscribed under and its prefixes.
struct Sub {
    rgen: u64,
    prefixes: Vec<String>,
}

/// Per-row subscriptions. x86 keys rows by slot, aarch64 by ASID (the arch's mailbox key).
#[cfg(target_arch = "x86_64")]
static SUBS: crate::procslot::SlotVec<crate::sync::Mutex<Option<Sub>>> = crate::procslot::SlotVec::new(|| crate::sync::Mutex::new(None), crate::sync::Mutex::new(None));
#[cfg(target_arch = "aarch64")]
static SUBS: crate::procslot::SlotVec<crate::sync::Mutex<Option<Sub>>> = crate::procslot::SlotVec::new_asid(|| crate::sync::Mutex::new(None), crate::sync::Mutex::new(None));

/// Rows holding a subscription (the write path's no-subscriber fast exit).
static R3_SUBS: AtomicU32 = AtomicU32::new(0);
/// Frames pushed into ring-3 mailboxes / dropped on a full mailbox (monotonic since boot).
pub static R3_DELIVERED: AtomicU32 = AtomicU32::new(0);
pub static R3_DROPPED: AtomicU32 = AtomicU32::new(0);
static R3_FULL_SAID: AtomicBool = AtomicBool::new(false);

/// The shared row (kernel tasks, no private address space) never subscribes: it is never torn down.
fn shared_row(row: usize) -> bool {
    row == crate::procslot::SHARED_ROW || (cfg!(target_arch = "aarch64") && row == 0)
}

/// The verb-19 REQUEST: register `body` (a `<ns>.<key>` prefix) for the owner on `row` at generation `rgen`.
/// 0, -EINVAL (bad body, shared row) or -ENOSPC (past `PREF_SUB_MAX`). Process context (the row may allocate).
pub fn subscribe_owner(row: usize, rgen: u64, body: &[u8]) -> i64 {
    if shared_row(row) || row > crate::procslot::ROW_ID_MAX || body.len() > una_abi::PREF_SUB_PREFIX_MAX || body.contains(&0) {
        return EINVAL;
    }
    let Ok(prefix) = core::str::from_utf8(body) else { return EINVAL };
    let cell = &SUBS[row];
    let (st, n, fresh) = locked(|| {
        let mut g = cell.lock();
        let fresh = g.is_none();
        let sub = g.get_or_insert_with(|| Sub { rgen, prefixes: Vec::new() });
        if sub.rgen != rgen {
            sub.rgen = rgen; // a predecessor's row the teardown never reached: the new tenant starts clean
            sub.prefixes.clear();
        }
        if sub.prefixes.iter().any(|p| p == prefix) {
            return (0, sub.prefixes.len(), fresh);
        }
        if sub.prefixes.len() >= una_abi::PREF_SUB_MAX {
            return (una_abi::ENOSPC, sub.prefixes.len(), fresh);
        }
        sub.prefixes.push(String::from(prefix));
        (0, sub.prefixes.len(), fresh)
    });
    if fresh {
        R3_SUBS.fetch_add(1, Ordering::AcqRel);
    }
    serial_println!("[prefsbus] subscribe row={} gen={} prefix=\"{}\" n={} status={} (PREFCHANGED B558)", row, rgen, prefix, n, st);
    st
}

/// Take `row`'s subscription out (never allocates a row). The teardown's half and the fixture's save.
fn take_row(row: usize) -> Option<Sub> {
    let cell = SUBS.peek(row)?;
    let old = locked(|| cell.lock().take());
    if old.is_some() {
        R3_SUBS.fetch_sub(1, Ordering::AcqRel);
    }
    old
}

/// Teardown: the subscription dies with the address space (both arches' `clear_handle_row`).
pub fn unsubscribe_row(row: usize) {
    let _ = take_row(row);
}

/// Push `frame` (the verb-19 frame of record for `<ns>.<k>`) into every owner whose prefix matches: one frame per
/// owner per write, however many of its prefixes match.
fn r3_deliver(ns: &str, k: &str, frame: &[u8]) {
    if R3_SUBS.load(Ordering::Acquire) == 0 {
        return;
    }
    let addr = alloc::format!("{}.{}", ns, k);
    let mut to: Vec<(usize, u64)> = Vec::new();
    for row in 0..SUBS.hwm() {
        if shared_row(row) {
            continue;
        }
        let Some(cell) = SUBS.peek(row) else { continue };
        let hit = locked(|| cell.lock().as_ref().and_then(|s| s.prefixes.iter().any(|p| addr.starts_with(p.as_str())).then_some(s.rgen)));
        if let Some(rgen) = hit {
            to.push((row, rgen));
        }
    }
    for (row, rgen) in to {
        let st = crate::arch::syscall::prefs_changed_push(row, rgen, Box::from(frame));
        if st == 0 {
            R3_DELIVERED.fetch_add(1, Ordering::Relaxed);
        } else if st == una_abi::EAGAIN {
            R3_DROPPED.fetch_add(1, Ordering::Relaxed);
            if !R3_FULL_SAID.swap(true, Ordering::AcqRel) {
                serial_println!("[prefsbus] changed {} -> row={} mailbox full: frame dropped (said once; r3_dropped counts each)", addr, row);
            }
        }
        // any other status: the owner is gone (generation moved) — its teardown unsubscribes it
    }
}

/// How many prefixes `row` holds (the fixture's read).
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware", feature = "aarch64_el0"))))]
fn row_prefixes(row: usize) -> usize {
    SUBS.peek(row).map(|c| locked(|| c.lock().as_ref().map_or(0, |s| s.prefixes.len()))).unwrap_or(0)
}

#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware", feature = "aarch64_el0"))))]
#[path = "prefchanged_fx.rs"]
mod fx;

// ══════════════ LOCKCHANGED (rmbp-ledger B758, BP-security-18) — the last-changed key — TAIL-APPENDED ══════════════
// The kernel PrefChanged queue has ONE drainer (Settings' `bus_changes_inner`), so a second consumer (the lock) cannot
// read the frames; it learned only that SOMETHING changed (`CHANGED_N`) and paid three bus GETs per change. `on_changed`
// now also stamps the frame's `<ns>.<key>` into a small ring beside `CHANGED_N` (same moment, same `kq` gate), and a
// consumer asks [`changed_since`] whether a change after its mark named a key it reads. A change the ring no longer
// holds answers `true`: a lost key is a re-read, never a skipped change.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub use lastkey::{changed_mark, changed_since, last_changed};
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
use lastkey::note_changed;

#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
mod lastkey {
    use alloc::string::String;

    /// The newest changes kept (a burst longer than this re-reads).
    const KEYS: usize = 8;
    /// An address slot; a longer `<ns>.<key>` is kept as "unknown" (always a hit).
    const ADDR_MAX: usize = 64;
    const UNKNOWN: u8 = u8::MAX;

    struct Ring {
        /// The sequence of the newest change (0 = none yet); entry `seq % KEYS` holds it.
        seq: u32,
        addr: [[u8; ADDR_MAX]; KEYS],
        len: [u8; KEYS],
    }

    static RING: crate::sync::Mutex<Ring> = crate::sync::Mutex::new(Ring { seq: 0, addr: [[0; ADDR_MAX]; KEYS], len: [0; KEYS] });

    /// `on_changed`'s half: the frame of record for `<ns>.<k>` entered the kernel queue.
    pub(super) fn note_changed(ns: &str, k: &str) {
        super::locked(|| {
            let mut r = RING.lock();
            let seq = r.seq.wrapping_add(1);
            let i = seq as usize % KEYS;
            let n = ns.len() + 1 + k.len();
            if n <= ADDR_MAX {
                r.addr[i][..ns.len()].copy_from_slice(ns.as_bytes());
                r.addr[i][ns.len()] = b'.';
                r.addr[i][ns.len() + 1..n].copy_from_slice(k.as_bytes());
                r.len[i] = n as u8;
            } else {
                r.len[i] = UNKNOWN;
            }
            r.seq = seq;
        });
    }

    /// The change sequence now (a consumer's starting mark).
    pub fn changed_mark() -> u32 {
        super::locked(|| RING.lock().seq)
    }

    /// Did any PrefChanged after `mark` name an address `hit(ns, key)` accepts? Returns the new mark (the sequence now)
    /// and the answer. More changes since `mark` than the ring holds, or an address too long for its slot, answer
    /// `true`. Same mark = no change = `false`.
    pub fn changed_since(mark: u32, hit: impl Fn(&str, &str) -> bool) -> (u32, bool) {
        super::locked(|| {
            let r = RING.lock();
            let gap = r.seq.wrapping_sub(mark);
            if gap == 0 {
                return (r.seq, false);
            }
            if gap as usize > KEYS {
                return (r.seq, true);
            }
            let mut any = false;
            for d in 0..gap {
                let i = r.seq.wrapping_sub(d) as usize % KEYS;
                let l = r.len[i];
                if l == UNKNOWN {
                    any = true;
                    break;
                }
                let a = core::str::from_utf8(&r.addr[i][..l as usize]).ok().and_then(crate::prefs::split_addr);
                if a.map_or(true, |(ns, k)| hit(ns, k)) {
                    any = true;
                    break;
                }
            }
            (r.seq, any)
        })
    }

    /// The newest change: `(seq, "<ns>.<key>")` (`None` before the first, or when its address did not fit).
    pub fn last_changed() -> Option<(u32, String)> {
        super::locked(|| {
            let r = RING.lock();
            let i = r.seq as usize % KEYS;
            let l = r.len[i];
            if r.seq == 0 || l == UNKNOWN {
                return None;
            }
            core::str::from_utf8(&r.addr[i][..l as usize]).ok().map(|a| (r.seq, String::from(a)))
        })
    }
}
