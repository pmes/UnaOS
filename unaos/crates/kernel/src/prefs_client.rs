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
    let room = locked(|| REPLIES.lock().len() < Q_CAP); #[cfg(feature = "busreg")] { if room { deliverrace_fill(); } } room // QUEUED-SET-DELIVER-RACE (rmbp-ledger B1194): the Q9 leg's one-shot window, the inbox filled AFTER the verdict (file tail). LINE-NEUTRAL fold, code first.
}

/// Take the answer to `corr` (dropping stale answers to earlier, timed-out calls on the way).
fn take_reply(corr: u32) -> Option<Box<[u8]>> {
    locked(|| {
        let mut q = REPLIES.lock();
        while let Some(f) = q.pop_front() {
            if matches!(crate::bus::frame_parse(&f), Ok(h) if h.kind == BUS_KIND_REPLY && h.corr == corr) {
                return Some(f);
            } #[cfg(feature = "busreg")] queued_mark(&f); // RELAY-QUEUED-SET (B1175): a stale answer to a QUEUED set notes its status on the ring entry before it drops (file tail). LINE-NEUTRAL fold.
        }
        None
    })
}

/// Register the kernel subscriber (the Settings window). Idempotent.
pub fn subscribe() {
    #[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))] if ringkeep::held() { return; } SUBSCRIBED.store(true, Ordering::Release); // SUBSCRIBEKEEP (B796): a fixture's hold makes every subscribe a no-op (file tail)
}

/// PrefChanged: called by the store on every accepted write. Builds the verb-19 frame of record into the
/// subscription queue (oldest dropped and counted when full). No subscriber = nothing queued. PREFSUNSET (SR169):
/// `a = None` = the key left the store with no preference-level default: the frame carries the unset form.
pub fn on_changed(ns: &str, k: &str, a: Option<&prefs_core::schema::Applied>) {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] ringkeep::note(ns, k); let kq = SUBSCRIBED.load(Ordering::Acquire); if !kq && R3_SUBS.load(Ordering::Acquire) == 0 { // PREFCHANGED (B558): a kernel OR a ring-3 subscriber
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
    let frame = f.into_boxed_slice(); r3_deliver(ns, k, &frame); if !kq { return; } // LOCKCHANGED (B758): the key beside CHANGED_N is noted at this fn's head now, on the ring's own switch (SUBSCRIBEKEEP B796, file tail). PREFCHANGED (B558): the same frame of record into every subscribed ring-3 owner's mailbox (file tail). LINE-NEUTRAL append.
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
        una_abi::BUS_VERB_PREF_UNSET => Some(una_abi::BUS_VERB_R3PREF_UNSET), // PREFS-UNSET-RELAY (B1186): the unset relays as Principia's 131
        _ => None,
    }
}

/// Offer the frame to Principia's ring-3 fulfiller through the router. `None` = nobody answered (no
/// registration, a refusal, or the wait ran out) — the kernel fulfiller answers instead.
#[cfg(feature = "busreg")]
fn relay(hdr: &crate::bus::BusHdr, body: &[u8]) -> Option<Answer> {
    let tag = r3_tag(hdr.verb)?; if relayed_write(hdr.verb) && !set_holder_in_session(tag, hdr.corr) { return None; } // PREFS-UNSET-RELAY (B1186): a relayed WRITE is SET or UNSET (file tail). PREFSELF-PRELOGIN (B1155): a SET whose fulfiller is not a program of the open session (a PREFS.ELF launched before login) is never relayed — the kernel fulfiller answers (file tail). LINE-NEUTRAL fold.
    let ops = crate::arch::syscall::busreg_ops(); let held = crate::bus_route::fulfiller_row(tag); // RELAY-RESET-FALLBACK (B1165): the (row, rgen) the relay goes to, read back if a -ECONNRESET answers (file tail). LINE-NEUTRAL fold.
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
            if reset_by_kernel(hdr, tag, held, h.status) { return None; } return Some(Answer { status: h.status as i64, body: fr[BUS_HDR_LEN..].to_vec() }); // RELAY-RESET-FALLBACK (B1165): a GET / LIST whose holder died in flight (on_exit's -ECONNRESET) is answered by the kernel fulfiller (file tail). LINE-NEUTRAL fold.
        }
        spins += 1;
        if crate::arch::ms().saturating_sub(t0) >= RELAY_WAIT_MS || spins >= RELAY_SPIN_MAX {
            RELAY_TIMEOUT.fetch_add(1, Ordering::Relaxed);
            if relayed_write(hdr.verb) { // PREFS-UNSET-RELAY (B1186): a timed-out UNSET is queued like a SET (file tail)
                // The frame is already in PREFS.ELF's mailbox and WILL land: a kernel write now could be
                // overtaken by this queued one (set A times out, set B lands, A lands = A wins). One queue,
                // one order — the set is accepted as in flight; its PrefChanged tells the window when it lands.
                if !queued_keep(hdr, body, held) { return Some(queued_full(tag, hdr.corr)); } serial_println!("[prefsbus] relay verb={} corr={} timeout={}ms — {} queued at prefs.elf", tag, hdr.corr, RELAY_WAIT_MS, queued_word(hdr.verb)); // UNSET-QUEUED-WORD (rmbp-ledger B1189): the verb word (file tail). RELAY-QUEUED-SET (B1175): the queued set's body is kept per client corr (full = -EAGAIN, never a silent 0) so a holder that dies with it unread is answered by the kernel fulfiller (file tail). QUEUED-SET-LOSTRESET (B1181): the holder's (row, rgen) from B1165's `held` rides the entry. LINE-NEUTRAL fold, code first.
                return Some(Answer { status: 0, body: Vec::new() });
            }
            timeoutpend_release(tag, hdr.corr); serial_println!("[prefsbus] relay verb={} corr={} timeout={}ms — kernel fulfiller answers", tag, hdr.corr, RELAY_WAIT_MS); // RELAY-TIMEOUT-PENDPIN (rmbp-ledger B1197): a timed-out GET / LIST takes its kernel-client relay Pend by client corr, so a silent holder never pins the per-caller cap; the late answer meets the no-Pend arm (file tail). LINE-NEUTRAL fold, code first.
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
    let mut corr = NEXT_CORR.fetch_add(1, Ordering::Relaxed); #[cfg(feature = "busreg")] queued_drain(); // RELAY-QUEUED-SET (B1175): every call first settles the queued sets whose holder answered late or died with them unread (file tail). LINE-NEUTRAL fold.
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
    let a = call(una_abi::BUS_VERB_PREF_GET, alloc::format!("{}.{}", ns, k).as_bytes()); #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] note_get(); // PREFSCLIENTKEYS (B779): every bus GET counted (`gets_total`, file tail). LINE-NEUTRAL append.
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
            tagtimeout_release(tag, corr); return Err(ETIMEDOUT); // RELAY-TAG-TIMEOUT-PENDPIN (rmbp-ledger B1202): a timed-out Holocron relay (tags 144-151) takes its kernel-client relay Pend by its own corr, so a silent holder never pins the per-caller cap; the late answer meets the no-Pend arm (file tail). LINE-NEUTRAL fold, code first.
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
// now also stamps the frame's `<ns>.<key>` into a small ring beside `CHANGED_N` (same moment; on the ring's own switch since SUBSCRIBEKEEP B796, never `kq`), and a
// consumer asks [`changed_since`] whether a change after its mark named a key it reads. A change the ring no longer
// holds answers `true`: a lost key is a re-read, never a skipped change.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub use lastkey::{changed_mark, changed_since, changed_take, gets_total, last_changed};
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
use lastkey::{note_changed, note_get};

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
        super::ringkeep::register(); super::locked(|| RING.lock().seq) // SUBSCRIBEKEEP (B796): reading the ring registers its reader
    }

    /// Did any PrefChanged after `mark` name an address `hit(ns, key)` accepts? Returns the new mark (the sequence now)
    /// and the answer. More changes since `mark` than the ring holds, or an address too long for its slot, answer
    /// `true`. Same mark = no change = `false`.
    pub fn changed_since(mark: u32, hit: impl Fn(&str, &str) -> bool) -> (u32, bool) {
        super::ringkeep::register(); super::locked(|| { // SUBSCRIBEKEEP (B796): reading the ring registers its reader
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

    // ── PREFSCLIENTKEYS (rmbp-ledger B779): the consumer's claimed edge and the one GET count ──────────────────────
    // CHARTER: Principia — fulfiller (the kernel bus client). Every PrefChanged poller keys its re-read on the ring above
    // (look, locale through [`changed_take`]; the lock claims its own edge through [`changed_since`], no login generation
    // since SESSIONGENPOLL B782), so a change to a key it does not read costs no GET; [`gets_total`] counts every bus GET.

    /// Every PrefGet the client has sent since boot (`pref_get`, the one GET door: `sys_get`/`sys_int`/`sys_flag`/`sys_text`).
    static GETS_TOTAL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

    /// `pref_get`'s half: one bus GET is on its way.
    pub(super) fn note_get() {
        GETS_TOTAL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }

    /// Bus GETs since boot (the fixtures' `gets_other=`).
    pub fn gets_total() -> u32 {
        GETS_TOTAL.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// A consumer's edge, keyed: CLAIM `mark` (compare-exchange to the sequence now, the lock's shape, so two passes never
    /// read one edge) and answer whether a change since it named an address `hit` accepts. `false` = no change, another
    /// pass claimed it, or every change named another key: no re-read owed.
    pub fn changed_take(mark: &core::sync::atomic::AtomicU32, hit: impl Fn(&str, &str) -> bool) -> bool {
        use core::sync::atomic::Ordering;
        let m = mark.load(Ordering::Acquire);
        let (n, h) = changed_since(m, hit);
        n != m && mark.compare_exchange(m, n, Ordering::AcqRel, Ordering::Acquire).is_ok() && h
    }
}

// ══════════════ SUBSCRIBEKEEP (rmbp-ledger B796, BP-security-18) — the ring is the client's — TAIL-APPENDED ══════════════
// CHARTER: Principia — fulfiller (the kernel bus client). The last-changed ring above moved only while a kernel subscriber
// (`SUBSCRIBED`, the Settings queue's flag) was registered, so look and locale — which subscribe nowhere — read a ring
// another consumer (the lock, Settings) had switched on. Now the ring is owned here: READING it is the registration
// (`changed_mark` / `changed_since`, and `changed_take` through it, call [`ringkeep::register`]), so no reader can forget
// it and none depends on another's subscribe. STICKY: the readers are boot-lifetime statics with no teardown (a count
// would have no release door), and a logout is the very edge the ring carries (the reload's PrefChanged, PREFSRELOAD
// B763) — an off at logout would drop it. Before the first read nothing is noted and nothing is owed: every reader's
// first read is unconditional. `on_changed` notes the key at its head, before the subscriber gates; `SUBSCRIBED` now
// gates only the Settings queue and `CHANGED_N`.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub use ringkeep::ring_on;
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub use ringkeep::subscriber_hold;

#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
mod ringkeep {
    use core::sync::atomic::{AtomicBool, Ordering};

    /// The ring is on (set by its first reader; never cleared).
    static ON: AtomicBool = AtomicBool::new(false);

    /// A reader's half: the first call turns the ring on and says so once.
    pub(super) fn register() {
        if !ON.load(Ordering::Acquire) && ON.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            let seq = super::changed_mark(); // ON is already set: no second line
            serial_println!("[prefsbus] ring on owner=prefs_client mode=sticky seq={} (SUBSCRIBEKEEP B796)", seq);
        }
    }

    /// `on_changed`'s half: an accepted write's `<ns>.<key>` enters the ring while it is on, whoever subscribed.
    pub(super) fn note(ns: &str, k: &str) {
        if ON.load(Ordering::Acquire) {
            super::note_changed(ns, k);
        }
    }

    /// Is the ring on (has any reader registered)? The `tests settingsmap` ring leg's `on=`.
    pub fn ring_on() -> bool {
        ON.load(Ordering::Acquire)
    }

    /// The fixture's hold: while set, [`super::subscribe`] is a no-op.
    #[cfg(feature = "witness")]
    static HOLD: AtomicBool = AtomicBool::new(false);
    /// `SUBSCRIBED` as the hold found it (restored on release).
    #[cfg(feature = "witness")]
    static WAS: AtomicBool = AtomicBool::new(false);

    /// TEST ONLY (`tests settingsmap`'s ring leg): `true` = hold — save and clear `SUBSCRIBED` and make every
    /// `subscribe()` (the lock's, Settings', a fixture's) a no-op; `false` = release — put `SUBSCRIBED` back. Returns
    /// `SUBSCRIBED` as the hold found it.
    #[cfg(feature = "witness")]
    pub fn subscriber_hold(hold: bool) -> bool {
        if hold {
            HOLD.store(true, Ordering::Release);
            let was = super::SUBSCRIBED.swap(false, Ordering::AcqRel);
            WAS.store(was, Ordering::Release);
            was
        } else {
            let was = WAS.load(Ordering::Acquire);
            HOLD.store(false, Ordering::Release);
            if was {
                super::SUBSCRIBED.store(true, Ordering::Release);
            }
            was
        }
    }

    /// `subscribe()`'s half: a fixture holds the subscribers off.
    #[cfg(feature = "witness")]
    pub(super) fn held() -> bool {
        HOLD.load(Ordering::Acquire)
    }
}

// =================================================================================================
// PREFSELF-PRELOGIN (rmbp-ledger B1155) — TAIL-APPENDED. A PREFS.ELF launched before login stays
// anonymous for the session (the boundary is the LAUNCH; a fulfiller never acts as its caller,
// `bus_route`'s PRINCIPAL RULE), so its forward of a SET is refused -EACCES under its own row. The SET
// is therefore not relayed to a holder that cannot commit it: `relay` answers `None` and the kernel
// fulfiller writes, exactly as it does for -ENOENT / -EAGAIN / a timeout. GET and LIST still relay.
// =================================================================================================
/// May the row holding Principia's SET tag `tag` commit a SET now? No holder = `true` (the route answers
/// -ENOENT as before); otherwise the arch's ONE SET judge (`pref_caller_in_session`) on that row.
#[cfg(feature = "busreg")]
fn set_holder_in_session(tag: u8, corr: u32) -> bool {
    let Some((row, _)) = crate::bus_route::fulfiller_row(tag) else { return true };
    if crate::arch::syscall::pref_row_in_session(row) {
        return true;
    }
    serial_println!("[prefsbus] relay verb={} corr={} fulfiller=row:{} not in session — kernel fulfiller answers", tag, corr, row);
    false
}

// =================================================================================================
// RELAY-RESET-FALLBACK (rmbp-ledger B1165) — TAIL-APPENDED. When a fulfiller row is torn down with a relay
// in flight (PREFSELF-RELAUNCH B1158 ends a pre-login PREFS.ELF at login; any holder's exit), `bus_route::
// on_exit` answers the CALLER -ECONNRESET (BANDY3's contract; ring-3 callers keep it). For the kernel
// client a GET or LIST is a read the kernel's own store answers, so `relay` returns `None` and the kernel
// fulfiller answers, exactly as for -ENOENT / -EAGAIN / a timeout. The frame cannot say who made the reset:
// a live holder's own -ECONNRESET reply goes through the same `deliver` and is byte-identical. The key is the
// row GENERATION both arches bump before `on_exit` (`clear_handle_row`): the reset is kernel-made when the
// (row, rgen) the relay went to (`bus_route::fulfiller_row`, read before the route) is no longer live; a live
// holder's own -ECONNRESET stays its final answer. A SET whose frame the holder READ keeps -ECONNRESET final
// (committed-or-not is unknowable; B1155 already keeps SETs off pre-login holders); one it never read is
// answered by the kernel fulfiller too (RELAY-RESET-SET B1169, below).
// =================================================================================================
/// Is `status` a kernel-made -ECONNRESET for a GET / LIST (or an UNREAD SET) whose holder `held` has died? Then the kernel
/// fulfiller answers (`relay` returns `None`).
#[cfg(feature = "busreg")]
fn reset_by_kernel(hdr: &crate::bus::BusHdr, tag: u8, held: Option<(usize, u64)>, status: i32) -> bool {
    if status as i64 != una_abi::ECONNRESET || (relayed_write(hdr.verb) && !set_unread_take(hdr.corr)) { // PREFS-UNSET-RELAY (B1186): an UNSET is a write (file tail). RELAY-RESET-SET (B1169): a SET passes only when on_exit marked its frame UNREAD (file tail)
        return false;
    }
    let Some((row, rgen)) = held else { return false };
    if (crate::arch::syscall::busreg_ops().rgen)(row) == rgen {
        return false; // the holder lives: its own -ECONNRESET is its answer
    }
    serial_println!("[prefsbus] relay verb={} corr={} fulfiller gone{} — kernel fulfiller answers", tag, hdr.corr, if relayed_write(hdr.verb) { " unread" } else { "" }); // RELAY-RESET-SET (B1169): GET / LIST bytes unchanged. PREFS-UNSET-RELAY (B1186): an UNSET as a SET
    true
}

// =================================================================================================
// RELAY-RESET-SET (rmbp-ledger B1169) — TAIL-APPENDED. A relayed SET whose holder dies mid-relay has two
// truths: the holder READ the frame (MRECV'd it: committed-or-not is unknowable, the -ECONNRESET stays final)
// or the frame still sat UNREAD in its mailbox when the row was torn down (the holder never saw it: the
// kernel fulfiller writes it, nothing can overtake it because the frame is destroyed). Only the arch mailbox
// clear sees the difference; `bus_route::on_exit` hands the CLIENT corr of an unread SET here through
// `note_set_unread` BEFORE it delivers the reset, and `reset_by_kernel` consumes the mark once. The mark is
// kernel-side (ring 3 cannot reach it); the reply frame stays byte-identical. Ring of `Q_CAP`, 0 = empty
// (corr 0 is PrefChanged's, never a call's); full = the oldest mark is overwritten (that SET stays -ECONNRESET).
// =================================================================================================
#[cfg(feature = "busreg")]
static SET_UNREAD: crate::sync::Mutex<([u32; Q_CAP], usize)> = crate::sync::Mutex::new(([0u32; Q_CAP], 0));

/// `bus_route::on_exit`: the SET relayed for client corr `corr` was never read by its (now dead) holder.
#[cfg(feature = "busreg")]
pub fn note_set_unread(corr: u32) {
    if corr == 0 {
        return;
    }
    locked(|| {
        let mut u = SET_UNREAD.lock();
        let i = u.1;
        u.0[i] = corr;
        u.1 = (i + 1) % Q_CAP;
    }); queued_due(); // QUEUED-SET-EAGER-ARM (rmbp-ledger B1196): the handoff wakes the parked `prefs-apply` task (a Semaphore post, the teardown's own done_sem road; file tail). Named in-place hunk
}

/// Take (clear) the unread mark of client corr `corr`. `true` = on_exit marked that SET's frame unread.
#[cfg(feature = "busreg")]
pub fn set_unread_take(corr: u32) -> bool {
    if corr == 0 {
        return false;
    }
    locked(|| {
        let mut u = SET_UNREAD.lock();
        match u.0.iter().position(|&c| c == corr) {
            Some(i) => {
                u.0[i] = 0;
                true
            }
            None => false,
        }
    })
}

// =================================================================================================
// RELAY-QUEUED-SET (rmbp-ledger B1175) — TAIL-APPENDED. A relayed SET that hits `RELAY_WAIT_MS` is answered
// as in flight ("set queued at prefs.elf"): its frame sits in the holder's mailbox and will land in order. Before
// this block nothing kept it, so when that holder then died with the frame UNREAD, `bus_route::on_exit` marked
// SET_UNREAD (B1169) and delivered the -ECONNRESET into an inbox nobody was waiting on: `reset_by_kernel` never
// ran, the next `take_reply` dropped the frame as stale, and the write was lost silently. Now the timeout arm
// keeps the SET's header and body per client corr in ONE bounded ring ([`QUEUED`], `Q_CAP` entries, each body
// at most `bus::BUS_BODY_MAX` since `call` refuses a longer one); full = the SET is answered -EAGAIN, never a
// silent 0. `queued_drain` (the head of every `call`) settles each entry in FIFO order: B1169's unread mark
// (`set_unread_take`) = the kernel fulfiller (`kernel`) applies the kept body — its own `changed` posts the
// PrefChanged — and says `[prefsbus] queued set corr=<c> holder gone unread — kernel fulfiller applied`; a late
// answer from the holder (status 0 = it applied the SET; -ECONNRESET with no mark = it READ it and died,
// committed-or-not unknowable as in B1169) = released, no second write. The answered status is read BEFORE the
// mark is asked, and on_exit marks BEFORE it delivers, so a reset already seen can never shadow an unread mark.
// `QUEUED` is a LEAF lock, always taken inside `locked`, nested only as REPLIES -> QUEUED (`take_reply`'s stale
// arm, the drain's sweep), never held across `kernel` or `set_unread_take`.
// QUEUED-SET-LOSTRESET (B1181): each entry carries its holder's (row, rgen); a holder whose generation moved with no
// mark and no answer (its reset lost to a full inbox) is released -ECONNRESET by `queued_lost` (file tail).
// Witness: `tests bandy3` -> `:: RELAY-QUEUED-SET: unread=applied read=released full=eagain lostreset=released/applied eager=applied -> PASS ::` (eager: QUEUED-SET-EAGER-ARM B1196, file tail).
// =================================================================================================
/// One queued SET: the request header (verb PREF_SET, the client corr), its body, and the status of the
/// holder's late answer once one reached the inbox.
#[cfg(feature = "busreg")]
struct Queued {
    hdr: crate::bus::BusHdr,
    body: Box<[u8]>,
    answered: Option<i32>,
    /// QUEUED-SET-LOSTRESET (B1181): the holder's (row, rgen) at keep time (`relay`'s B1165 `held`).
    held: Option<(usize, u64)>,
    /// QUEUED-SET-LOSTRESET (B1181): when a drain first saw that holder's generation move (the grace's start).
    gone_ms: Option<u64>,
    /// QUEUED-SET-STUCKLIVE (B1187): the deadline (keep time + `QUEUED_SILENT_MS`) past which a holder that has neither
    /// answered nor died is released as silent (`queued_silent`, file tail).
    silent_at_ms: u64,
}

#[cfg(feature = "busreg")]
static QUEUED: crate::sync::Mutex<VecDeque<Queued>> = crate::sync::Mutex::new(VecDeque::new());

/// `relay`'s timeout arm: keep the queued SET. `false` = the ring is full (the caller answers -EAGAIN).
#[cfg(feature = "busreg")]
fn queued_keep(hdr: &crate::bus::BusHdr, body: &[u8], held: Option<(usize, u64)>) -> bool {
    let b: Box<[u8]> = body.into();
    let h = *hdr;
    let silent_at_ms = crate::arch::ms().saturating_add(QUEUED_SILENT_MS); applier_ensure(); // QUEUED-SET-EAGER-ARM (rmbp-ledger B1196): the parked applier is spawned once, here in task context (file tail). Named in-place hunk
    locked(move || {
        let mut q = QUEUED.lock();
        if q.len() >= Q_CAP {
            return false;
        }
        q.push_back(Queued { hdr: h, body: b, answered: None, held, gone_ms: None, silent_at_ms });
        true
    })
}

/// The full ring's honest answer: -EAGAIN (the frame may still land at the holder; it is not tracked).
#[cfg(feature = "busreg")]
fn queued_full(tag: u8, corr: u32) -> Answer {
    serial_println!("[prefsbus] relay verb={} corr={} timeout={}ms — queue full ({}), {} refused -EAGAIN", tag, corr, RELAY_WAIT_MS, Q_CAP, queued_word(tag)); // UNSET-QUEUED-WORD (rmbp-ledger B1189): the queued write's verb word (file tail). Named in-place hunk
    Answer { status: una_abi::EAGAIN, body: Vec::new() }
}

/// Note a kernel-stamped REPLY `f` on its queued entry. `true` = `f` answered a queued SET (the caller drops it).
#[cfg(feature = "busreg")]
fn queued_note(q: &mut VecDeque<Queued>, f: &[u8]) -> bool {
    let Ok(h) = crate::bus::frame_parse(f) else { return false };
    if h.kind != BUS_KIND_REPLY || h.principal[0] != PRIN_KERNEL_REPLY || h.corr == 0 {
        return false;
    }
    match q.iter_mut().find(|e| e.hdr.corr == h.corr) {
        Some(e) => {
            e.answered = Some(h.status);
            true
        }
        None => false,
    }
}

/// `take_reply`'s stale arm (IRQs masked, REPLIES held): a frame about to drop that answers a queued SET.
#[cfg(feature = "busreg")]
fn queued_mark(f: &[u8]) {
    let mut q = QUEUED.lock();
    let _ = queued_note(&mut q, f);
}

/// Take the entry of client corr `corr` out of the ring.
#[cfg(feature = "busreg")]
fn queued_take(corr: u32) -> Option<Queued> {
    locked(|| {
        let mut q = QUEUED.lock();
        let i = q.iter().position(|e| e.hdr.corr == corr)?;
        q.remove(i)
    })
}

/// The head of every `call`: settle the queued SETs (see the block above).
#[cfg(feature = "busreg")]
fn queued_drain() { let Some(_gate) = drain_gate() else { return }; queued_settle() } #[cfg(feature = "busreg")] fn queued_settle() { // QUEUED-SET-EAGER-ARM (rmbp-ledger B1196): ONE drain at a time (the gate, file tail); `queued_settle` is the drain body, unchanged. Named in-place hunk
    if locked(|| QUEUED.lock().is_empty()) {
        return;
    }
    // 1. A late answer still in the inbox is noted on its entry and removed (nobody else waits on that corr).
    locked(|| {
        let mut r = REPLIES.lock();
        let mut q = QUEUED.lock();
        r.retain(|f| !queued_note(&mut q, f));
    });
    // 2. FIFO: the unread mark = the kernel fulfiller applies; an answer = released; neither = kept.
    let corrs: Vec<u32> = locked(|| QUEUED.lock().iter().map(|e| e.hdr.corr).collect());
    for c in corrs {
        let answered = locked(|| QUEUED.lock().iter().find(|e| e.hdr.corr == c).and_then(|e| e.answered));
        if set_unread_take(c) {
            let Some(e) = queued_take(c) else { continue };
            let a = kernel(&e.hdr, &e.body);
            if a.status == 0 {
                serial_println!("[prefsbus] queued {} corr={} holder gone unread — kernel fulfiller applied", queued_word(e.hdr.verb), c); // UNSET-QUEUED-WORD (rmbp-ledger B1189): the queued write's verb word (file tail). Named in-place hunk
            } else {
                serial_println!("[prefsbus] queued {} corr={} holder gone unread — kernel fulfiller refused status={}", queued_word(e.hdr.verb), c, a.status); // UNSET-QUEUED-WORD (rmbp-ledger B1189): the queued write's verb word (file tail). Named in-place hunk
            }
        } else if let Some(st) = answered {
            if let Some(e) = queued_take(c) {
                serial_println!("[prefsbus] queued {} corr={} answered status={} — released", queued_word(e.hdr.verb), c, st); // UNSET-QUEUED-WORD (rmbp-ledger B1189): the queued write's verb word (file tail). Named in-place hunk
            }
        } else if queued_lost(c) {
            // QUEUED-SET-LOSTRESET (B1181): the holder died having READ it and its reset was lost (full inbox).
            if let Some(e) = queued_take(c) {
                serial_println!("[prefsbus] queued {} corr={} holder gone, reset lost — released status={}", queued_word(e.hdr.verb), c, una_abi::ECONNRESET); // UNSET-QUEUED-WORD (rmbp-ledger B1189): the queued write's verb word (file tail). Named in-place hunk
            }
        } else if queued_silent(c) {
            // QUEUED-SET-STUCKLIVE (B1187): a live holder that never answered, past the deadline (file tail).
            if queued_take(c).is_some() { pendpin_release(c); // QUEUED-SET-PENDPIN (B1193): the silent holder's relay Pend is reclaimed (file tail). LINE-NEUTRAL fold.
                serial_println!("[prefsbus] queued set corr={} holder silent — released status={}", c, ETIMEDOUT);
            }
        }
    }
}

/// A PREF_SET request header for the fixture (the frame `call` builds for the same body).
#[cfg(feature = "busreg")]
fn queued_set_hdr(corr: u32, body: &[u8]) -> Option<crate::bus::BusHdr> {
    let mut f = alloc::vec![0u8; BUS_HDR_LEN + body.len()];
    let n = crate::bus::build_request(una_abi::BUS_VERB_PREF_SET, corr, body, &mut f);
    crate::bus::frame_parse(&f[..n]).ok()
}

/// The reply frame `bus_route::deliver` puts in this client's inbox for a relayed SET (empty body).
#[cfg(feature = "busreg")]
fn queued_reply(corr: u32, status: i32) -> Box<[u8]> {
    let mut r = alloc::vec![0u8; BUS_HDR_LEN];
    crate::bus::build_reply(una_abi::BUS_VERB_R3PREF_SET, corr, status, kernel_reply_principal(), &[], &mut r);
    r.into_boxed_slice()
}

/// `tests bandy3` (called from `bus_route::selftest`): Q1 a queued SET whose holder is torn down with it UNREAD
/// (on_exit's two steps in its order: `note_set_unread`, then the -ECONNRESET into the inbox) — the next client
/// call's drain applies it through the kernel fulfiller and the key reads the SET value; Q2 a queued SET the
/// holder answered (status 0, no mark) is released with no second write; Q3 the ring refuses past `Q_CAP`
/// (the -EAGAIN answer). Key `system.display.idle_min` (settingsbus' key); the operator's value is restored.
#[cfg(feature = "busreg")]
pub fn queued_selftest() {
    use crate::prefs::key;
    if let Some(k) = crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &crate::prefs::path()) {
        serial_println!(":: RELAY-QUEUED-SET: SKIP reason=home-not-writable({}) ::", k);
        return;
    }
    if !in_session() {
        serial_println!(":: RELAY-QUEUED-SET: SKIP reason=no-session ::");
        return;
    }
    let next = || {
        let mut c = NEXT_CORR.fetch_add(1, Ordering::Relaxed);
        if c == 0 {
            c = NEXT_CORR.fetch_add(1, Ordering::Relaxed);
        }
        c
    };
    let body_of = |v: i64| alloc::format!("{}.{}\0{}", crate::prefs::NS, key::IDLE_MIN, PrefValue::Int(v).to_literal());
    let read = || pref_get(crate::prefs::NS, key::IDLE_MIN).ok().and_then(|v| v.as_int());
    let orig = read();
    let want: i64 = if orig == Some(7) { 9 } else { 7 };
    let other: i64 = if want == 7 { 11 } else { 13 };
    // Q1: unread at the teardown -> the kernel fulfiller applies it at the next call.
    let b1 = body_of(want);
    let c1 = next();
    let kept1 = queued_set_hdr(c1, b1.as_bytes()).is_some_and(|h| queued_keep(&h, b1.as_bytes(), None));
    note_set_unread(c1);
    let pushed1 = inbox_push(queued_reply(c1, una_abi::ECONNRESET as i32));
    let read1 = read();
    let q1 = kept1 && pushed1 && read1 == Some(want) && queued_take(c1).is_none();
    // Q2: the holder answered (applied it itself) -> released, no second write.
    let b2 = body_of(other);
    let c2 = next();
    let kept2 = queued_set_hdr(c2, b2.as_bytes()).is_some_and(|h| queued_keep(&h, b2.as_bytes(), None));
    let pushed2 = inbox_push(queued_reply(c2, 0));
    let read2 = read();
    let q2 = kept2 && pushed2 && read2 == Some(want) && queued_take(c2).is_none();
    // Q3: the ring refuses past Q_CAP with -EAGAIN; the fixture's entries are taken back.
    let mut mine: Vec<u32> = Vec::new();
    let mut eagain = false;
    for _ in 0..=Q_CAP {
        let c = next();
        match queued_set_hdr(c, b2.as_bytes()) {
            Some(h) if queued_keep(&h, b2.as_bytes(), None) => mine.push(c),
            Some(_) => {
                eagain = queued_full(una_abi::BUS_VERB_R3PREF_SET, c).status == una_abi::EAGAIN;
                break;
            }
            None => break,
        }
    }
    let q3 = eagain && mine.len() <= Q_CAP;
    for c in mine {
        let _ = queued_take(c);
    }
    let (q4, q5) = lostreset_selftest(&next, &body_of, &read, want); // QUEUED-SET-LOSTRESET (B1181): Q4 / Q5, the full-inbox legs (file tail)
    let (q6, q7) = stucklive_selftest(&next, &body_of, &read, want); let q8 = pendpin_selftest(&next, &body_of, &read, want); /* QUEUED-SET-PENDPIN (B1193): Q8 the silent holder's Pend reclaimed (file tail) */ let q9 = deliverrace_selftest(&next, &body_of, &read, want); /* QUEUED-SET-DELIVER-RACE (rmbp-ledger B1194): Q9, the inbox filled between the room check and the deliver (file tail). Named in-place hunk */ let q10 = eager_selftest(&next, &body_of, want); let q12 = timeoutpend_selftest(&next); /* RELAY-TIMEOUT-PENDPIN (rmbp-ledger B1197): Q12, a timed-out GET / LIST's Pend reclaimed (file tail). Named in-place hunk */ let q14 = tagtimeout_selftest(&next); /* RELAY-TAG-TIMEOUT-PENDPIN (rmbp-ledger B1202): Q14, a timed-out Holocron relay's Pend reclaimed (file tail). Named in-place hunk */ // QUEUED-SET-STUCKLIVE (B1187): Q6 silent live holder, Q7 refused answer delivered (file tail)
    // Restore the operator's value through the same kernel fulfiller (or leave the key unset as it was).
    match orig {
        Some(v) => {
            let b = body_of(v);
            if let Some(h) = queued_set_hdr(next(), b.as_bytes()) {
                let _ = kernel(&h, b.as_bytes());
            }
        }
        None => {
            let _ = crate::prefs::unset(crate::prefs::NS, key::IDLE_MIN);
        }
    }
    if q1 && q2 && q3 && q4 && q5 && q6 && q7 && q8 && q9 && q10 && q12 && q14 { // RELAY-TIMEOUT-PENDPIN (rmbp-ledger B1197): Q12. RELAY-TAG-TIMEOUT-PENDPIN (rmbp-ledger B1202): Q14. Named in-place hunk
        serial_println!(":: RELAY-QUEUED-SET: unread=applied read=released full=eagain lostreset=released/applied stucklive=released refusedreply=redelivered pendpin=reclaimed timeoutpend=reclaimed tagtimeout=reclaimed deliverrace=closed eager=applied -> PASS ::");
    } else {
        serial_println!(":: RELAY-QUEUED-SET: q1={} q2={} q3={} q4={} q5={} q6={} q7={} q8={} q9={} q10={} q12={} q14={} (want={} read1={} read2={} kept={}/{} pushed={}/{}) -> FAIL ::", q1 as u8, q2 as u8, q3 as u8, q4 as u8, q5 as u8, q6 as u8, q7 as u8, q8 as u8, q9 as u8, q10 as u8, q12 as u8, q14 as u8, want, read1.unwrap_or(-1), read2.unwrap_or(-1), kept1 as u8, kept2 as u8, pushed1 as u8, pushed2 as u8);
    }
}

// =================================================================================================
// QUEUED-SET-LOSTRESET (rmbp-ledger B1181) — TAIL-APPENDED. B1175's ring settles a queued SET by B1169's unread
// mark or by the holder's late answer. When the client inbox is FULL at the holder's teardown, `bus_route::on_exit`'s
// -ECONNRESET is refused by `inbox_push`: an UNREAD SET still applies (the mark is set before the reset is
// delivered), but a SET the holder READ got no mark and now no answer, so its entry held a ring slot forever and
// sixteen of them turned every new queued SET into -EAGAIN. Each entry now carries the holder's (row, rgen) from
// `relay`'s B1165 read (`held`), and an entry whose holder generation moved (B1165's key, the read
// `reset_by_kernel` makes) with no mark and no answer is released with the -ECONNRESET word. SMP: both arches bump
// the generation at the top of the teardown and `on_exit` marks an unread SET at its end, so a drain on another
// core can see the holder dead before its mark lands; the first drain only STAMPS `gone_ms`, a later one releases
// once RELAY_WAIT_MS has passed, and the mark is asked first at every drain, so a mark that lands in the grace
// applies. A mark lost to SET_UNREAD's ring overwrite is released as read (fail-closed, B1169). EAGER is BUILT by
// QUEUED-SET-EAGER-ARM (B1196, file tail): the unread mark wakes a parked `prefs-apply` task on both arches.
// =================================================================================================
/// Has the holder of queued corr `corr` been torn down, past the grace, with no answer and no mark? The first
/// drain that sees it dead stamps `gone_ms`; the arch `rgen` is read with `QUEUED` released.
#[cfg(feature = "busreg")]
fn queued_lost(corr: u32) -> bool {
    let Some((row, rgen, gone)) = locked(|| QUEUED.lock().iter().find(|e| e.hdr.corr == corr).and_then(|e| e.held.map(|(r, g)| (r, g, e.gone_ms)))) else { return false };
    if (crate::arch::syscall::busreg_ops().rgen)(row) == rgen {
        return false; // the holder lives (B1165's key): its answer may still come
    }
    let now = crate::arch::ms();
    match gone {
        Some(t) => now.saturating_sub(t) >= RELAY_WAIT_MS,
        None => {
            locked(|| {
                if let Some(e) = QUEUED.lock().iter_mut().find(|e| e.hdr.corr == corr) {
                    e.gone_ms = Some(now);
                }
            });
            false
        }
    }
}

/// `queued_selftest`'s full-inbox legs (B1181). The holder is a row whose generation is not live (as a torn-down
/// holder reads). Q4: a READ SET whose reset `inbox_push` refused (full inbox, no mark) is kept through the grace,
/// then released with no write. Q5: an UNREAD SET whose reset was refused the same way is applied by its mark.
/// The fixture's filler frames are taken back out of the inbox by corr.
#[cfg(feature = "busreg")]
fn lostreset_selftest(next: &dyn Fn() -> u32, body_of: &dyn Fn(i64) -> String, read: &dyn Fn() -> Option<i64>, want: i64) -> (bool, bool) {
    let stale = Some((0usize, (crate::arch::syscall::busreg_ops().rgen)(0).wrapping_add(1)));
    let mut fillers: Vec<u32> = Vec::new();
    let fill = |fillers: &mut Vec<u32>| {
        for _ in 0..=Q_CAP {
            let c = next();
            if !inbox_push(queued_reply(c, 0)) {
                break;
            }
            fillers.push(c);
        }
    };
    // Q4: READ + full inbox -> released by liveness after the grace, the key unchanged.
    let b4 = body_of(if want == 7 { 21 } else { 23 });
    let c4 = next();
    let kept4 = queued_set_hdr(c4, b4.as_bytes()).is_some_and(|h| queued_keep(&h, b4.as_bytes(), stale));
    fill(&mut fillers);
    let lost4 = !inbox_push(queued_reply(c4, una_abi::ECONNRESET as i32));
    queued_drain();
    let graced4 = locked(|| QUEUED.lock().iter().any(|e| e.hdr.corr == c4));
    let t = crate::arch::ms();
    let mut spins = 0u32;
    while crate::arch::ms().saturating_sub(t) <= RELAY_WAIT_MS && spins < RELAY_SPIN_MAX.saturating_mul(16) {
        spins += 1;
        core::hint::spin_loop();
    }
    let waited = crate::arch::ms().saturating_sub(t);
    queued_drain();
    let released4 = queued_take(c4).is_none();
    let read4 = read();
    let q4 = kept4 && lost4 && graced4 && released4 && read4 == Some(want);
    // Q5: UNREAD + full inbox -> the mark (set before the refused reset, on_exit's order) applies it.
    let v5: i64 = if want == 7 { 15 } else { 17 };
    let b5 = body_of(v5);
    let c5 = next();
    let kept5 = queued_set_hdr(c5, b5.as_bytes()).is_some_and(|h| queued_keep(&h, b5.as_bytes(), stale));
    fill(&mut fillers);
    note_set_unread(c5);
    let lost5 = !inbox_push(queued_reply(c5, una_abi::ECONNRESET as i32));
    queued_drain();
    let gone5 = queued_take(c5).is_none();
    let read5 = read();
    let q5 = kept5 && lost5 && gone5 && read5 == Some(v5);
    locked(|| REPLIES.lock().retain(|f| !matches!(crate::bus::frame_parse(f), Ok(h) if fillers.contains(&h.corr))));
    if !(q4 && q5) {
        serial_println!("[prefsbus] lostreset legs: kept={}/{} lost={}/{} graced4={} released4={} waited={}ms read4={} gone5={} read5={}", kept4 as u8, kept5 as u8, lost4 as u8, lost5 as u8, graced4 as u8, released4 as u8, waited, read4.unwrap_or(-1), gone5 as u8, read5.unwrap_or(-1));
    }
    (q4, q5)
}

// PREFS-UNSET-RELAY (rmbp-ledger B1186) — TAIL-APPENDED. The kernel client's UNSET (PREF_UNSET 29) relays to Principia's
// ring-3 fulfiller as R3PREF_UNSET (131, `r3_tag`) and is a WRITE on every road a relayed SET takes ([`relayed_write`]):
// B1155's gate (a holder not of the open session is skipped, the kernel fulfiller answers), B1175's queue arm (a timed-out
// unset is kept in `QUEUED` and settled by `queued_drain`, whose `kernel` call is verb-generic; its lines name the
// verb, `queued_word`, UNSET-QUEUED-WORD B1189) and B1169's unread road (`bus_route::unread_handoff` marks an unread 131 ->
// the kernel fulfiller applies it; one the holder READ stays -ECONNRESET). PREFS.ELF forwards 131 to PREF_UNSET under
// its own stamped name: `wire::unset_as` checks `cap::may_set` for `prefs` (B1170/B1177 demote an image not at
// `/apps/PREFS.ELF` to Anon). [`pref_unset`] is the sender (Settings' Delete on a row, `video/settings.rs`).
// =================================================================================================
/// A relayed WRITE (SET or UNSET): the verbs B1155's gate, B1175's queue and B1169's unread road apply to.
#[cfg(feature = "busreg")]
fn relayed_write(verb: u8) -> bool {
    verb == una_abi::BUS_VERB_PREF_SET || verb == una_abi::BUS_VERB_PREF_UNSET
}

/// PrefUnset `ns.key` over the bus (the key reads its default again). `Ok` = a value left the store (its PrefChanged
/// unset form follows); `Err(-ENOENT)` = it was unset already; other errnos as PREF_UNSET's. Relayed as 131 while
/// PREFS.ELF holds it (busreg), else the kernel fulfiller answers.
pub fn pref_unset(ns: &str, k: &str) -> Result<(), i64> {
    let a = call(una_abi::BUS_VERB_PREF_UNSET, alloc::format!("{}.{}", ns, k).as_bytes());
    serial_println!("[prefsbus] unset {}.{} via={} status={}", ns, k, last_via(), a.status);
    if a.status == 0 { Ok(()) } else { Err(a.status) }
}

/// `bus_route::unsetrelay_selftest`'s client legs (`tests bandy3`), key `system.display.idle_min`, the operator's value
/// restored: P1 `r3_tag` maps PREF_UNSET to 131 and it is a relayed write (GET is not); P2 a holder gone (the client row's
/// own generation answers `u64::MAX`, never 0): with no unread mark the -ECONNRESET stays final, with the mark
/// `reset_by_kernel` hands the UNSET to the kernel fulfiller, which unsets the key; P3 a QUEUED unset (B1175's timeout
/// road) with the unread mark is applied by the next call's `queued_drain`. `Err` = SKIP (no session, home unwritable).
#[cfg(feature = "busreg")]
pub fn unset_relay_legs() -> Result<(bool, bool, bool), &'static str> {
    use crate::prefs::key;
    if crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &crate::prefs::path()).is_some() {
        return Err("home-not-writable");
    }
    if !in_session() {
        return Err("no-session");
    }
    let next = || {
        let mut c = NEXT_CORR.fetch_add(1, Ordering::Relaxed);
        if c == 0 {
            c = NEXT_CORR.fetch_add(1, Ordering::Relaxed);
        }
        c
    };
    let hdr_of = |verb: u8, corr: u32, body: &[u8]| -> Option<crate::bus::BusHdr> {
        let mut f = alloc::vec![0u8; BUS_HDR_LEN + body.len()];
        let n = crate::bus::build_request(verb, corr, body, &mut f);
        crate::bus::frame_parse(&f[..n]).ok()
    };
    let addr = alloc::format!("{}.{}", crate::prefs::NS, key::IDLE_MIN);
    let seed = |lit: &str| {
        let b = alloc::format!("{}\0{}", addr, lit);
        hdr_of(una_abi::BUS_VERB_PREF_SET, next(), b.as_bytes()).is_some_and(|h| kernel(&h, b.as_bytes()).status == 0)
    };
    let held = Some((KCLIENT_ROW, 0u64));
    let reset = una_abi::ECONNRESET as i32;
    let orig = crate::prefs::get(crate::prefs::NS, key::IDLE_MIN);
    // P1: the verb map and the write road.
    let p1 = r3_tag(una_abi::BUS_VERB_PREF_UNSET) == Some(una_abi::BUS_VERB_R3PREF_UNSET) && relayed_write(una_abi::BUS_VERB_PREF_UNSET) && !relayed_write(una_abi::BUS_VERB_PREF_GET);
    // P2: read -> final; unread -> the kernel fulfiller applies.
    let seeded2 = seed(PrefValue::Int(7).to_literal().as_str());
    let c2 = next();
    let p2 = match hdr_of(una_abi::BUS_VERB_PREF_UNSET, c2, addr.as_bytes()) {
        Some(h) => {
            let fin = !reset_by_kernel(&h, una_abi::BUS_VERB_R3PREF_UNSET, held, reset) && crate::prefs::get(crate::prefs::NS, key::IDLE_MIN).is_some();
            note_set_unread(c2);
            let handed = reset_by_kernel(&h, una_abi::BUS_VERB_R3PREF_UNSET, held, reset);
            let applied = handed && kernel(&h, addr.as_bytes()).status == 0 && crate::prefs::get(crate::prefs::NS, key::IDLE_MIN).is_none();
            seeded2 && fin && applied
        }
        None => false,
    };
    // P3: a queued unset whose holder died with it unread is applied at the next call's drain.
    let seeded3 = seed(PrefValue::Int(9).to_literal().as_str());
    let c3 = next();
    let kept = hdr_of(una_abi::BUS_VERB_PREF_UNSET, c3, addr.as_bytes()).is_some_and(|h| queued_keep(&h, addr.as_bytes(), None)); // UNSET-QUEUED-WORD (rmbp-ledger B1189): B1181's third argument (no lostreset road; the unread mark is asked first). Named in-place hunk
    note_set_unread(c3);
    let pushed = inbox_push(queued_reply(c3, reset));
    let gone = pref_get(crate::prefs::NS, key::IDLE_MIN).is_err();
    let p3 = seeded3 && kept && pushed && gone && queued_take(c3).is_none() && crate::prefs::get(crate::prefs::NS, key::IDLE_MIN).is_none();
    let _ = queued_take(c3);
    // Restore the operator's value through the same kernel fulfiller (or leave the key unset as it was).
    match orig {
        Some(v) => {
            let _ = seed(v.to_literal().as_str());
        }
        None => {
            let _ = crate::prefs::unset(crate::prefs::NS, key::IDLE_MIN);
        }
    }
    Ok((p1, p2, p3))
}

// =================================================================================================
// QUEUED-SET-STUCKLIVE (rmbp-ledger B1187) — TAIL-APPENDED. B1181 released a queued SET whose holder DIED; two live
// roads still pinned a ring slot. (1) The holder ANSWERED but the client inbox was full: `bus_route::fulfiller_reply`
// refused with -EAGAIN before taking the Pend ("the fulfiller may retry"), the kernel keeps no copy of the reply (the
// Pend is routing only), and PREFS.ELF never retries (`serve` ends `let _ = send(n);`), so the answer was gone while its
// status sat in the kernel's hand. That refusal arm now hands the status to the queued entry ([`queued_answer`]: the
// same `answered` field `queued_note` fills for a late answer in the inbox), takes the Pend and answers the holder 0;
// the next `queued_drain` releases it through the existing answered arm (no re-ask: a re-relayed SET is a second write
// that can overtake a later one). A corr that is not queued keeps -EAGAIN unchanged. (2) A LIVE holder that never answers:
// each entry carries a deadline (`silent_at_ms`, keep time + [`QUEUED_SILENT_MS`]); past it, with no mark, no answer and
// the holder not lost, the drain releases it -ETIMEDOUT. The bound: the store write PREFS.ELF's forwarded SET performs
// (`[prefsbus] flush n= ms=`) is at worst `n=1 ms=5121` on the bench (rmbp flight39) and `n=2 ms=62545` anywhere in the
// evidence tree (rmbp flight24): 70 s is above every line, so an honest answer is not released as silent. The holder's
// late answer after the release misses on corr (`queued_note` / `queued_answer` find no entry). `QUEUED` stays a leaf
// lock inside `locked`; serial lines are printed with no lock held. Owed: QUEUED-SET-PENDPIN (a silent holder's
// kernel-client Pend is never reclaimed: KCLIENT_GEN never moves), QUEUED-SET-DELIVER-RACE (the room check passes and the
// inbox fills before `deliver`: the status is lost, the entry ages out at -110).
// =================================================================================================
/// How long a queued SET's live holder may stay silent before its ring slot is released (70 s, see above).
#[cfg(feature = "busreg")]
const QUEUED_SILENT_MS: u64 = 1_400 * RELAY_WAIT_MS;
/// The silent release's status word (-ETIMEDOUT).
#[cfg(feature = "busreg")]
const ETIMEDOUT: i32 = -110;

/// `bus_route::fulfiller_reply`'s full-inbox arm: the holder's answer `status` to client corr `corr`. `true` = `corr` is
/// a queued SET and the status now rides its entry (the caller takes the Pend); `false` = not queued (-EAGAIN as before).
#[cfg(feature = "busreg")]
pub fn queued_answer(corr: u32, status: i32) -> bool {
    if corr == 0 {
        return false;
    }
    let hit = locked(|| match QUEUED.lock().iter_mut().find(|e| e.hdr.corr == corr) {
        Some(e) => {
            e.answered = Some(status);
            true
        }
        None => false,
    });
    if hit {
        serial_println!("[prefsbus] queued set corr={} answer refused by a full inbox status={} — kept on the entry", corr, status);
    }
    hit
}

/// Is the queued entry of client corr `corr` unanswered and past its silent deadline?
#[cfg(feature = "busreg")]
fn queued_silent(corr: u32) -> bool {
    let now = crate::arch::ms();
    locked(|| QUEUED.lock().iter().any(|e| e.hdr.corr == corr && e.answered.is_none() && now >= e.silent_at_ms))
}

/// `queued_selftest`'s live-holder legs (B1187). The holder is row 0 at its CURRENT generation (reads live). Q6: a SET
/// whose deadline has passed is released by the drain with no write, and the holder's late answer then misses on corr.
/// Q7: a holder answer refused by a full inbox is kept on the entry by `queued_answer` and released by the drain with no
/// second write. The fixture's filler frames are taken back out of the inbox by corr.
#[cfg(feature = "busreg")]
fn stucklive_selftest(next: &dyn Fn() -> u32, body_of: &dyn Fn(i64) -> String, read: &dyn Fn() -> Option<i64>, want: i64) -> (bool, bool) {
    let live = Some((0usize, (crate::arch::syscall::busreg_ops().rgen)(0)));
    // Q6: silent live holder, deadline passed -> released -110, key unchanged; the late answer misses on corr.
    let b6 = body_of(if want == 7 { 25 } else { 27 });
    let c6 = next();
    let kept6 = queued_set_hdr(c6, b6.as_bytes()).is_some_and(|h| queued_keep(&h, b6.as_bytes(), live));
    let now = crate::arch::ms();
    locked(|| {
        if let Some(e) = QUEUED.lock().iter_mut().find(|e| e.hdr.corr == c6) {
            e.silent_at_ms = now;
        }
    });
    queued_drain();
    let released6 = queued_take(c6).is_none();
    let late = queued_reply(c6, 0);
    let missed6 = !locked(|| queued_note(&mut QUEUED.lock(), &late)) && !queued_answer(c6, 0);
    let read6 = read();
    let q6 = kept6 && released6 && missed6 && read6 == Some(want);
    // Q7: full inbox refuses the holder's answer -> kept on the entry -> released by the drain, key unchanged.
    let b7 = body_of(if want == 7 { 29 } else { 31 });
    let c7 = next();
    let kept7 = queued_set_hdr(c7, b7.as_bytes()).is_some_and(|h| queued_keep(&h, b7.as_bytes(), live));
    let mut fillers: Vec<u32> = Vec::new();
    for _ in 0..=Q_CAP {
        let c = next();
        if !inbox_push(queued_reply(c, 0)) {
            break;
        }
        fillers.push(c);
    }
    let full7 = !inbox_has_room();
    let handed7 = queued_answer(c7, 0);
    let noted7 = locked(|| QUEUED.lock().iter().any(|e| e.hdr.corr == c7 && e.answered == Some(0)));
    queued_drain();
    let released7 = queued_take(c7).is_none();
    let read7 = read();
    let q7 = kept7 && full7 && handed7 && noted7 && released7 && read7 == Some(want);
    locked(|| REPLIES.lock().retain(|f| !matches!(crate::bus::frame_parse(f), Ok(h) if fillers.contains(&h.corr))));
    if !(q6 && q7) {
        serial_println!("[prefsbus] stucklive legs: kept={}/{} released={}/{} missed6={} full7={} handed7={} noted7={} read6={} read7={}", kept6 as u8, kept7 as u8, released6 as u8, released7 as u8, missed6 as u8, full7 as u8, handed7 as u8, noted7 as u8, read6.unwrap_or(-1), read7.unwrap_or(-1));
    }
    (q6, q7)
}

// UNSET-QUEUED-WORD (rmbp-ledger B1189, BP-storage-11) — TAIL-APPENDED. The queued ring ([`QUEUED`], B1175/B1181) keeps
// the client request header, so each entry already holds its verb (PREF_SET or PREF_UNSET, B1186's `relayed_write`);
// its log lines name that verb instead of saying "set" for a queued unset. Words only: no state, no lock, no frame.
// Wire: `[prefsbus] queued unset corr=<c> holder gone unread — kernel fulfiller applied` (and the `answered`,
// `reset lost`, `queued at prefs.elf`, `queue full` lines the same); a queued SET's lines are unchanged.
// =================================================================================================
/// The word for a relayed write's verb: `unset` for PREF_UNSET (the client verb) or R3PREF_UNSET (its relay tag),
/// else `set`.
#[cfg(feature = "busreg")]
fn queued_word(verb: u8) -> &'static str {
    if verb == una_abi::BUS_VERB_PREF_UNSET || verb == una_abi::BUS_VERB_R3PREF_UNSET { "unset" } else { "set" }
}

// =================================================================================================
// QUEUED-SET-PENDPIN (rmbp-ledger B1193, BP-storage-11) — TAIL-APPENDED. B1187's silent arm released a queued SET whose
// live holder never answered, but the relay's kernel-client `Pend` stayed in `bus_route`'s table (KCLIENT_GEN never moves,
// so the dead-caller reclaim never frees it): at `PEND_MAX_PER_CALLER` every later SET was written by the kernel fulfiller
// ahead of the holder's queue, and the holder's late commit of the old SET could overtake it. The silent arm now takes that
// Pend by the client corr (`bus_route::take_pending_by_corr`, the keyed form of `take_pending`): the late answer meets
// `fulfiller_reply`'s no-Pend arm (-ENOENT, nothing delivered) and the next SET relays to the holder again. The dead-holder
// roads (B1169 unread, B1181 lost reset) need nothing: `on_exit` already takes the dying holder's Pends. The serial line is
// printed with no lock held (`QUEUED` released by `queued_take`, `PEND` by the taker).
// Wire: `[prefsbus] queued set corr=<c> holder silent — relay pend reclaimed` before the arm's `released status=-110`.
// =================================================================================================
/// The silent arm's reclaim: drop the kernel-client relay Pend of client corr `corr`, if the holder still held one.
#[cfg(feature = "busreg")]
fn pendpin_release(corr: u32) {
    if crate::bus_route::take_pending_by_corr(KCLIENT_ROW, corr) {
        serial_println!("[prefsbus] queued set corr={} holder silent — relay pend reclaimed", corr);
    }
}

/// `queued_selftest`'s Q8 (B1193). Holder = row 0 at its CURRENT generation (Q6's). Client Pends are planted up to the
/// per-caller cap (the first one's corr also kept on the ring); the entry's deadline is expired and the drain releases it:
/// the count is back under the cap (the next SET relays), the holder's late answer is -ENOENT with no inbox frame, and
/// the key is unchanged. The filler Pends and any stray frame are taken back.
#[cfg(feature = "busreg")]
fn pendpin_selftest(next: &dyn Fn() -> u32, body_of: &dyn Fn(i64) -> String, read: &dyn Fn() -> Option<i64>, want: i64) -> bool {
    let ops = crate::arch::syscall::busreg_ops();
    let rg0 = (ops.rgen)(0);
    let cap = crate::bus_route::PEND_MAX_PER_CALLER;
    let n0 = crate::bus_route::caller_pending(KCLIENT_ROW, KCLIENT_GEN);
    let b8 = body_of(if want == 7 { 33 } else { 35 });
    let c8 = next();
    let relay8 = crate::bus_route::pendpin_plant(una_abi::BUS_VERB_R3PREF_SET, c8, 0, rg0);
    let kept8 = relay8.is_some() && queued_set_hdr(c8, b8.as_bytes()).is_some_and(|h| queued_keep(&h, b8.as_bytes(), Some((0usize, rg0))));
    let mut fillers: Vec<u32> = Vec::new();
    let mut planted = relay8.is_some();
    for _ in (n0 + 1)..cap {
        let c = next();
        if crate::bus_route::pendpin_plant(una_abi::BUS_VERB_R3PREF_SET, c, 0, rg0).is_none() {
            planted = false;
            break;
        }
        fillers.push(c);
    }
    let full8 = crate::bus_route::caller_pending(KCLIENT_ROW, KCLIENT_GEN) >= cap;
    let now = crate::arch::ms();
    locked(|| {
        if let Some(e) = QUEUED.lock().iter_mut().find(|e| e.hdr.corr == c8) {
            e.silent_at_ms = now;
        }
    });
    queued_drain();
    let released8 = queued_take(c8).is_none();
    let freed8 = crate::bus_route::caller_pending(KCLIENT_ROW, KCLIENT_GEN) < cap;
    let mut r = alloc::vec![0u8; BUS_HDR_LEN];
    let rn = crate::bus::build_reply(una_abi::BUS_VERB_R3PREF_SET, relay8.unwrap_or(0), 0, [0u8; 32], &[], &mut r);
    let late8 = match crate::bus::frame_parse(&r[..rn]) {
        Ok(h) => crate::bus_route::fulfiller_reply(ops, 0, rg0, &h, &r[..rn]) == -2,
        Err(_) => false,
    };
    let stray8 = locked(|| {
        let mut q = REPLIES.lock();
        let before = q.len();
        q.retain(|f| !matches!(crate::bus::frame_parse(f), Ok(h) if h.corr == c8));
        q.len() != before
    });
    let read8 = read();
    for c in fillers {
        let _ = crate::bus_route::take_pending_by_corr(KCLIENT_ROW, c);
    }
    let _ = crate::bus_route::take_pending_by_corr(KCLIENT_ROW, c8);
    let _ = queued_take(c8);
    let q8 = kept8 && planted && full8 && released8 && freed8 && late8 && !stray8 && read8 == Some(want);
    if !q8 {
        serial_println!("[prefsbus] pendpin leg: kept={} planted={} full={} released={} freed={} late={} stray={} n0={} read8={}", kept8 as u8, planted as u8, full8 as u8, released8 as u8, freed8 as u8, late8 as u8, stray8 as u8, n0, read8.unwrap_or(-1));
    }
    q8
}

// QUEUED-SET-DELIVER-RACE (rmbp-ledger B1194, BP-storage-11) — TAIL-APPENDED. B1187 handed a holder's answer refused by
// `bus_route::fulfiller_reply`'s room check to its queued entry; the inbox could still fill AFTER that check passed and
// before the final arm's `deliver` (the Pend already taken), and the status was lost (the entry aged out at -110). That
// deliver-failure arm now runs the same [`queued_answer`] call (bus_route.rs, line-neutral). The window sits inside
// `fulfiller_reply` and no seam reaches it, so the Q9 leg opens it with a ONE-SHOT hook: [`DELIVERRACE_ARM`], consumed by
// [`inbox_has_room`] only when its verdict is room, fills the inbox to `Q_CAP` through [`inbox_push`] AFTER the verdict
// (the interleaving a second pusher produces); unarmed it costs one atomic load. The leg's Pend is fabricated by
// `bus_route::deliverrace_reply` (said there); the room check, the fill, `take_pending`, `deliver` and the arm run for real.
// Wire: `[prefsbus] queued set corr=<c> answer refused by a full inbox status=0 — kept on the entry`, then the drain's
// `answered status=0 — released`; `:: RELAY-QUEUED-SET: ... deliverrace=closed eager=owed -> PASS ::`.
// =================================================================================================
/// Q9's one-shot window: armed by the leg, consumed by the next [`inbox_has_room`] whose verdict is room.
#[cfg(feature = "busreg")]
static DELIVERRACE_ARM: AtomicBool = AtomicBool::new(false);
/// The client corr of the hook's filler replies (never a live client corr: the client counts up from 1).
#[cfg(feature = "busreg")]
const DELIVERRACE_FILLER: u32 = u32::MAX - 9;

/// [`inbox_has_room`]'s hook: when armed, disarm and fill the inbox to `Q_CAP` with filler replies.
#[cfg(feature = "busreg")]
fn deliverrace_fill() {
    if !DELIVERRACE_ARM.load(Ordering::Acquire) || !DELIVERRACE_ARM.swap(false, Ordering::AcqRel) {
        return;
    }
    for _ in 0..Q_CAP {
        if !inbox_push(queued_reply(DELIVERRACE_FILLER, 0)) {
            break;
        }
    }
}

/// `queued_selftest`'s Q9: a queued SET's holder answers 0 while the inbox has room; the armed hook fills the inbox
/// between `fulfiller_reply`'s room check and its `deliver`; the answer must still reach the entry (return 0, the Pend
/// gone, `answered == Some(0)`), and the drain releases it with no second write. Without the B1194 arm the return is
/// -EAGAIN and the entry stays unanswered.
#[cfg(feature = "busreg")]
fn deliverrace_selftest(next: &dyn Fn() -> u32, body_of: &dyn Fn(i64) -> String, read: &dyn Fn() -> Option<i64>, want: i64) -> bool {
    let b9 = body_of(if want == 7 { 33 } else { 35 });
    let c9 = next();
    let kept9 = queued_set_hdr(c9, b9.as_bytes()).is_some_and(|h| queued_keep(&h, b9.as_bytes(), None));
    let room9 = inbox_has_room();
    DELIVERRACE_ARM.store(true, Ordering::Release);
    let (rc9, left9) = crate::bus_route::deliverrace_reply(c9, 0);
    DELIVERRACE_ARM.store(false, Ordering::Release);
    let full9 = !inbox_has_room();
    let noted9 = locked(|| QUEUED.lock().iter().any(|e| e.hdr.corr == c9 && e.answered == Some(0)));
    locked(|| REPLIES.lock().retain(|f| !matches!(crate::bus::frame_parse(f), Ok(h) if h.corr == DELIVERRACE_FILLER)));
    queued_drain();
    let released9 = queued_take(c9).is_none();
    let read9 = read();
    let q9 = kept9 && room9 && rc9 == 0 && !left9 && full9 && noted9 && released9 && read9 == Some(want);
    if !q9 {
        serial_println!("[prefsbus] deliverrace leg: kept={} room={} rc={} left={} full={} noted={} released={} read9={}", kept9 as u8, room9 as u8, rc9, left9 as u8, full9 as u8, noted9 as u8, released9 as u8, read9.unwrap_or(-1));
    }
    q9
}

// QUEUED-SET-EAGER-ARM (rmbp-ledger B1196) — TAIL-APPENDED. B1175's ring applied a queued SET whose holder died with
// it UNREAD only at the NEXT client call (`queued_drain` at the head of `call`): on_exit runs in the dying task's exit
// (IRQ masked) or in the scheduler's reap, where no file I/O may run, and the x86 `prefs-flush` road is a render-task
// deferral, not an exit road. Now the on_exit handoff's own callee (`note_set_unread`) raises [`QUEUED_DUE`] and, on
// its false->true edge, posts [`APPLY_SEM`]: a `sched::Semaphore` post, the road every exit and reap ALREADY takes in
// the same frame (`done_sem.post()`: aarch64 `exit` / `retire_killed`, x86 `exit` / `reap_killed`; `post` drops its raw
// lock before `make_ready`'s run-queue push and no run-queue lock is held there). The parked kernel task `prefs-apply`
// (the aarch64 `orphan_reaper` / `REAPER_SEM` shape, arch-neutral; spawned ONCE, lazily, by `queued_keep` in task
// context) wakes, takes the drain gate, runs the drain body (`queued_settle`, the one apply path) and records the corrs
// it settled. THE GATE: two drains at once could lose a SET (one takes the unread mark, the other releases the same entry
// as lost past the grace) or land an OLD queued value after a call's NEW SET, so every drain holds [`DRAINING`]; an
// unmasked waiter yields, a MASKED caller (aarch64 `yield_now` unmasks) tries once and skips, the holder settling the
// ring. The apply path never calls `call`, so the gate cannot re-enter. Cost: one parked task (APPLIER_STACK, idle at 0
// duty), one flag, one Semaphore, one gate flag, one Q_CAP ring. A lost-reset release (`queued_lost`, a READ SET, no
// write) still waits for a later drain past its grace. OWED: QUEUED-SET-EAGER-KEEPRACE (a holder dying between `relay`'s
// last poll and `queued_keep` marks before the entry exists: settled at the next call); PREFS-FLUSH-ONTO-APPLIER.
// Witness: `tests bandy3` -> `[prefsbus] queued apply on=prefs-apply settled=<n> ms=<ms> (QUEUED-SET-EAGER-ARM)` and
// `:: RELAY-QUEUED-SET: ... eager=applied -> PASS ::` (Q10, `eager_selftest`).
// =================================================================================================
/// Raised by the on_exit handoff (`note_set_unread`); cleared by `prefs-apply` before it drains.
#[cfg(feature = "busreg")]
static QUEUED_DUE: AtomicBool = AtomicBool::new(false);
/// `prefs-apply`'s wake: posted from the teardown (the `done_sem` road), waited on by `prefs-apply` alone.
#[cfg(feature = "busreg")]
static APPLY_SEM: crate::arch::sched::Semaphore = crate::arch::sched::Semaphore::new(0);
/// `prefs-apply` has been spawned (once per boot).
#[cfg(feature = "busreg")]
static APPLIER: AtomicBool = AtomicBool::new(false);
/// The applier's kernel stack: the store path's measured size (x86 `prefs-watch` runs it on `RENDER_PATH_STACK_SIZE`, 32 KiB).
#[cfg(feature = "busreg")]
const APPLIER_STACK: usize = 32 * 1024;
/// The drain gate: one `queued_settle` at a time.
#[cfg(feature = "busreg")]
static DRAINING: AtomicBool = AtomicBool::new(false);
/// The client corrs `prefs-apply` settled (a ring, next slot in `.1`; 0 = empty). LEAF lock, under `locked`.
#[cfg(feature = "busreg")]
static EAGER_SETTLED: crate::sync::Mutex<([u32; Q_CAP], usize)> = crate::sync::Mutex::new(([0u32; Q_CAP], 0));
/// How long Q10 waits for the applier (40 x `RELAY_WAIT_MS`, 2 s).
#[cfg(feature = "busreg")]
const EAGER_WAIT_MS: u64 = 40 * RELAY_WAIT_MS;

/// `note_set_unread` (the on_exit handoff, any context): mark the ring due and wake `prefs-apply` once per edge.
#[cfg(feature = "busreg")]
fn queued_due() {
    if !QUEUED_DUE.swap(true, Ordering::AcqRel) {
        APPLY_SEM.post();
    }
}

/// `queued_keep` (task context): spawn `prefs-apply` the first time a SET is queued.
#[cfg(feature = "busreg")]
fn applier_ensure() {
    if APPLIER.swap(true, Ordering::AcqRel) {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::sched::spawn_stack("prefs-apply", prefs_apply, 0, crate::arch::sched::CPU_AUTO, crate::arch::sched::PRIO_NORMAL, APPLIER_STACK);
    }
    #[cfg(target_arch = "aarch64")]
    {
        let _ = crate::arch::sched::spawn_stack("prefs-apply", prefs_apply, 0, crate::arch::sched::CPU_AUTO, APPLIER_STACK);
    }
}

/// The corrs in the ring now.
#[cfg(feature = "busreg")]
fn queued_corrs() -> Vec<u32> {
    locked(|| QUEUED.lock().iter().map(|e| e.hdr.corr).collect())
}

/// `prefs-apply`: parked on `APPLY_SEM`; each wake settles the ring under the gate and records what it settled.
#[cfg(feature = "busreg")]
fn prefs_apply(_: usize) {
    locked(|| APPLY_SEM.init()); // masked: a teardown post on this core must never spin on a raw lock this task holds
    loop {
        if !APPLY_SEM.wait() {
            return; // off a scheduled task (never for a spawned task)
        }
        QUEUED_DUE.store(false, Ordering::Release);
        let Some(_gate) = drain_gate() else { continue };
        let before = queued_corrs();
        if before.is_empty() {
            continue;
        }
        let t0 = crate::arch::ms();
        queued_settle();
        let after = queued_corrs();
        let mut n = 0usize;
        for &c in before.iter().filter(|c| !after.contains(*c)) {
            n += 1;
            locked(|| {
                let mut s = EAGER_SETTLED.lock();
                let i = s.1;
                s.0[i] = c;
                s.1 = (i + 1) % Q_CAP;
            });
        }
        if n > 0 {
            serial_println!("[prefsbus] queued apply on=prefs-apply settled={} ms={} (QUEUED-SET-EAGER-ARM)", n, crate::arch::ms().saturating_sub(t0));
        }
    }
}

/// The drain gate's hold; dropping it opens the gate.
#[cfg(feature = "busreg")]
struct DrainGate;

#[cfg(feature = "busreg")]
impl Drop for DrainGate {
    fn drop(&mut self) {
        DRAINING.store(false, Ordering::Release);
    }
}

/// Take the drain gate. Unmasked: wait (yield) for the holder. Masked: one try, `None` when busy (the holder is
/// settling the ring; a masked wait on a preemptible holder could never end, and aarch64 `yield_now` unmasks).
#[cfg(feature = "busreg")]
fn drain_gate() -> Option<DrainGate> {
    loop {
        if !DRAINING.swap(true, Ordering::AcqRel) {
            return Some(DrainGate);
        }
        if crate::arch::irqs_masked() {
            return None;
        }
        crate::arch::sched::yield_now();
        core::hint::spin_loop();
    }
}

/// Did `prefs-apply` settle client corr `corr`?
#[cfg(feature = "busreg")]
fn eager_settled(corr: u32) -> bool {
    corr != 0 && locked(|| EAGER_SETTLED.lock().0.contains(&corr))
}

/// `queued_selftest`'s Q10: a SET kept for a dead holder (a generation that is not live), then on_exit's two steps in
/// its order (`note_set_unread`, which wakes the applier, then the -ECONNRESET into the inbox), and NO client call: the
/// leg yields until `prefs-apply` has settled it and reads the key through the STORE (`prefs::get`), not the client.
#[cfg(feature = "busreg")]
fn eager_selftest(next: &dyn Fn() -> u32, body_of: &dyn Fn(i64) -> String, want: i64) -> bool {
    let stale = Some((0usize, (crate::arch::syscall::busreg_ops().rgen)(0).wrapping_add(1)));
    let v10: i64 = if want == 7 { 25 } else { 27 };
    let b10 = body_of(v10);
    let c10 = next();
    let kept10 = queued_set_hdr(c10, b10.as_bytes()).is_some_and(|h| queued_keep(&h, b10.as_bytes(), stale));
    note_set_unread(c10);
    let pushed10 = inbox_push(queued_reply(c10, una_abi::ECONNRESET as i32));
    let t = crate::arch::ms();
    let mut spins = 0u32;
    while kept10 && !eager_settled(c10) && crate::arch::ms().saturating_sub(t) <= EAGER_WAIT_MS && spins < RELAY_SPIN_MAX.saturating_mul(16) {
        spins += 1;
        if crate::arch::irqs_masked() {
            core::hint::spin_loop();
        } else {
            crate::arch::sched::yield_now();
        }
    }
    let waited = crate::arch::ms().saturating_sub(t);
    let by_applier = eager_settled(c10);
    let store10 = crate::prefs::get(crate::prefs::NS, crate::prefs::key::IDLE_MIN).and_then(|v| v.as_int());
    let gone10 = queued_take(c10).is_none();
    if !by_applier {
        // The fixture's leftovers: the mark and the reset frame go with the entry.
        let _ = set_unread_take(c10);
        locked(|| REPLIES.lock().retain(|f| !matches!(crate::bus::frame_parse(f), Ok(h) if h.corr == c10)));
    }
    let q10 = kept10 && by_applier && gone10 && store10 == Some(v10);
    if !q10 {
        serial_println!("[prefsbus] eager leg: kept={} by_applier={} gone={} store={} waited={}ms pushed={}", kept10 as u8, by_applier as u8, gone10 as u8, store10.unwrap_or(-1), waited, pushed10 as u8);
    }
    q10
}

// =================================================================================================
// RELAY-TIMEOUT-PENDPIN (rmbp-ledger B1197, BP-storage-11) — TAIL-APPENDED. A relayed GET or LIST that hits
// `RELAY_WAIT_MS` is answered by the kernel fulfiller (`relay`'s read timeout arm returns `None`), but the relay's
// kernel-client `Pend` stayed in `bus_route`'s table with the silent holder: KCLIENT_GEN never moves, so the dead-caller
// reclaim never frees it, no ring entry exists for a read (the ring keeps SET / UNSET only), and eight timed-out reads
// pinned the kernel client to the kernel fulfiller for good (every later relay -EAGAIN at the cap). The read arm now takes
// that Pend by the client corr through B1193's `bus_route::take_pending_by_corr` (the keyed form of the one taker
// `take_pending`). The holder's late answer meets `fulfiller_reply`'s no-Pend arm (-ENOENT, nothing delivered), which is
// honest: the caller was already answered by the kernel fulfiller from the same store PREFS.ELF commits into, so the late
// value is stale. An answer that lands between the timeout check and the take has already taken its Pend (`false` here)
// and its frame is dropped as stale by `take_reply`. The write arm (B1175's ring road) is untouched. The serial line is
// printed with no lock held (`PEND` released by the taker).
// Wire: `[prefsbus] relay verb=<t> corr=<c> timeout — relay pend reclaimed` before the arm's `kernel fulfiller answers`;
// `tests bandy3` -> `:: RELAY-QUEUED-SET: ... pendpin=reclaimed timeoutpend=reclaimed ... -> PASS ::` (Q12). Limit: Q12
// drives this fn on a PLANTED Pend; `relay`'s arm itself is reached only with a registered silent holder, which
// `queued_selftest` has no fixture for. Owed: RELAY-TAG-TIMEOUT-PENDPIN (`relay_tag`'s -ETIMEDOUT arm leaves its Pend on
// the same client row).
// =================================================================================================
/// `relay`'s read timeout arm: drop the kernel-client relay Pend of client corr `corr` for ring-3 tag `tag`, if still held.
#[cfg(feature = "busreg")]
fn timeoutpend_release(tag: u8, corr: u32) {
    if crate::bus_route::take_pending_by_corr(KCLIENT_ROW, corr) {
        serial_println!("[prefsbus] relay verb={} corr={} timeout — relay pend reclaimed", tag, corr);
    }
}

/// `queued_selftest`'s Q12 (B1197). For R3PREF_GET and R3PREF_LIST: a kernel-client Pend for a fresh client corr, held by
/// row 0 at its CURRENT generation, is planted (the count rises by one); the read arm's own fn runs; the count is back;
/// the holder's late answer through the real `fulfiller_reply` is -ENOENT with no inbox frame for that corr. Leftover
/// Pends and any stray frame are taken back.
#[cfg(feature = "busreg")]
fn timeoutpend_selftest(next: &dyn Fn() -> u32) -> bool {
    let ops = crate::arch::syscall::busreg_ops();
    let rg0 = (ops.rgen)(0);
    let mut ok = true;
    for verb in [una_abi::BUS_VERB_R3PREF_GET, una_abi::BUS_VERB_R3PREF_LIST] {
        let n0 = crate::bus_route::caller_pending(KCLIENT_ROW, KCLIENT_GEN);
        let c = next();
        let relay = crate::bus_route::pendpin_plant(verb, c, 0, rg0);
        let planted = relay.is_some() && crate::bus_route::caller_pending(KCLIENT_ROW, KCLIENT_GEN) == n0 + 1;
        timeoutpend_release(verb, c);
        let freed = crate::bus_route::caller_pending(KCLIENT_ROW, KCLIENT_GEN) == n0;
        let mut r = alloc::vec![0u8; BUS_HDR_LEN];
        let rn = crate::bus::build_reply(verb, relay.unwrap_or(0), 0, [0u8; 32], &[], &mut r);
        let late = match crate::bus::frame_parse(&r[..rn]) {
            Ok(h) => crate::bus_route::fulfiller_reply(ops, 0, rg0, &h, &r[..rn]) == -2,
            Err(_) => false,
        };
        let stray = locked(|| {
            let mut q = REPLIES.lock();
            let before = q.len();
            q.retain(|f| !matches!(crate::bus::frame_parse(f), Ok(h) if h.corr == c));
            q.len() != before
        });
        let _ = crate::bus_route::take_pending_by_corr(KCLIENT_ROW, c);
        if !(planted && freed && late && !stray) {
            serial_println!("[prefsbus] timeoutpend leg: verb={} planted={} freed={} late={} stray={} n0={}", verb, planted as u8, freed as u8, late as u8, stray as u8, n0);
            ok = false;
        }
    }
    ok
}

// =================================================================================================
// RELAY-TAG-TIMEOUT-PENDPIN (rmbp-ledger B1202, BP-storage-11) — TAIL-APPENDED. `relay_tag` (BTKEYSEAL B446) is the
// kernel-client relay for Holocron's ring-3 tags 144-151; its -ETIMEDOUT arm returned with the relay's Pend still in
// `bus_route`'s table (KCLIENT_GEN never moves, so the dead-caller reclaim never frees it), so eight timed-out Holocron
// relays pinned the kernel client at PEND_MAX_PER_CALLER and every later PREF relay was -EAGAIN (kernel fulfiller). The
// arm now takes that Pend by its own corr through B1193's `bus_route::take_pending_by_corr`, as B1197's
// `timeoutpend_release` does for `relay`'s read arm. Every kernel-client corr comes from the one NEXT_CORR, so the keyed
// take never removes another call's Pend, and a caller's retry after -ETIMEDOUT (bthid's seal pass) mints a new corr
// that `take_reply` matches exactly. Holocron's late answer meets `fulfiller_reply`'s no-Pend arm (-ENOENT, nothing
// delivered): honest, since the caller was already answered -ETIMEDOUT (Unavailable), and a late SecretGet's key bytes
// no longer land in the inbox. An answer delivered between the timeout check and the take has already taken its Pend
// (`false` here, no print) and is dropped as stale by `take_reply` (owed RELAY-TAG-LATE-WIPE: that drop does not wipe).
// The serial line is printed with no lock held (`PEND` released by the taker).
// Wire: `[prefsbus] relay tag=<t> corr=<c> timeout — relay pend reclaimed`; `tests bandy3` ->
// `:: RELAY-QUEUED-SET: ... timeoutpend=reclaimed tagtimeout=reclaimed ... -> PASS ::` (Q14). Limit: Q14 drives this fn on
// a PLANTED Pend; `relay_tag`'s arm itself is reached only with a registered silent Holocron holder, which
// `queued_selftest` has no fixture for.
// =================================================================================================
/// `relay_tag`'s -ETIMEDOUT arm: drop the kernel-client relay Pend of client corr `corr` for ring-3 tag `tag`, if still held.
#[cfg(feature = "busreg")]
fn tagtimeout_release(tag: u8, corr: u32) {
    if crate::bus_route::take_pending_by_corr(KCLIENT_ROW, corr) {
        serial_println!("[prefsbus] relay tag={} corr={} timeout — relay pend reclaimed", tag, corr);
    }
}

/// `queued_selftest`'s Q14 (B1202), B1197's leg shape for one Holocron tag: a kernel-client Pend of tag 144 for a fresh
/// client corr, held by row 0 at its CURRENT generation, is planted (the count rises by one); the arm's own fn runs; the
/// count is back; the holder's late answer through the real `fulfiller_reply` is -ENOENT with no inbox frame for that
/// corr. A leftover Pend and any stray frame are taken back.
#[cfg(feature = "busreg")]
fn tagtimeout_selftest(next: &dyn Fn() -> u32) -> bool {
    /// Holocron's SecretGet tag (`holocron_core::wire::VERB_SECRET_GET`; the crate rides `lumen`, so the literal).
    const HOLOCRON_SECRET_GET: u8 = 144;
    let ops = crate::arch::syscall::busreg_ops();
    let rg0 = (ops.rgen)(0);
    let tag = HOLOCRON_SECRET_GET;
    let n0 = crate::bus_route::caller_pending(KCLIENT_ROW, KCLIENT_GEN);
    let c = next();
    let relay = crate::bus_route::pendpin_plant(tag, c, 0, rg0);
    let planted = relay.is_some() && crate::bus_route::caller_pending(KCLIENT_ROW, KCLIENT_GEN) == n0 + 1;
    tagtimeout_release(tag, c);
    let freed = crate::bus_route::caller_pending(KCLIENT_ROW, KCLIENT_GEN) == n0;
    let mut r = alloc::vec![0u8; BUS_HDR_LEN];
    let rn = crate::bus::build_reply(tag, relay.unwrap_or(0), 0, [0u8; 32], &[], &mut r);
    let late = match crate::bus::frame_parse(&r[..rn]) {
        Ok(h) => crate::bus_route::fulfiller_reply(ops, 0, rg0, &h, &r[..rn]) == -2,
        Err(_) => false,
    };
    let stray = locked(|| {
        let mut q = REPLIES.lock();
        let before = q.len();
        q.retain(|f| !matches!(crate::bus::frame_parse(f), Ok(h) if h.corr == c));
        q.len() != before
    });
    let _ = crate::bus_route::take_pending_by_corr(KCLIENT_ROW, c);
    let ok = planted && freed && late && !stray;
    if !ok {
        serial_println!("[prefsbus] tagtimeout leg: tag={} planted={} freed={} late={} stray={} n0={}", tag, planted as u8, freed as u8, late as u8, stray as u8, n0);
    }
    ok
}
