//! CHARTER: Kernel — wm (the lock is a veil over the window table the compositor paints; the password field and its verdict are the login screen's, `video/login.rs` SCREENLOCK — reused, never a second widget)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AUTOLOCK (rmbp-ledger B712, BP-bar-15/20; bar research B §7: "a lock the user sets once and never fights — idle
//! and lid lock the glass, the password opens it, nothing else is touched").
//!
//! * **One key set** (Principia, `system.lock.*`, Settings > Security, one row each): `after_idle_min` (0 = never,
//!   default 10), `on_lid` (default on), `on_sleep` (default on). LOCKPREFS (B748): read over the bus
//!   (`prefs_client::sys_int` / `sys_flag`, Principia's client, never `crate::prefs`) on the service pass that first sees
//!   a PrefChanged (`prefs_client::CHANGED_N` moved) or a login-generation move ([`prefs_pass`]) — no clock poll; the
//!   Settings rows write through [`set_policy`] / [`persist`] (`prefs_client::sys_set`).
//! * **The LOCK state** is a VEIL, not a teardown: [`veiled`] is a term of `wm::above_shell` (one fold, `lock_veiled`
//!   at the file tail), so the compositor, the desktop's occluders, the hit test and the a11y tree all read every row
//!   but the modal one (the login screen in its locked mode) as below the shell — the rows, their surfaces, their z
//!   and the focus are untouched, so the right password restores the exact desktop in ONE composite ([`opened`]).
//!   The lock view is the login screen's locked card (the session's tile and name, the password field, Unlock) over
//!   the backdrop; the menu bar's clock stays. Input goes only to the screen: `users::screen_key` and the press gate
//!   already answer `true` for every key and press while it is up (BP-security-5).
//! * **Entered by** idle (no key or pointer event for `after_idle_min`, DIMIDLE's activity clock
//!   `dimidle::last_activity_ms`), the lid closing (`power::sleep::enter`, before the state's line), any other sleep
//!   entry (`on_sleep`), and Win+L (`keymap::resolve_chord`'s fallback, so every keyboard path -> the router's `LockScreen` arm ->
//!   `login::lock` -> [`enter`]`(Key)`; ⌘L / Ctrl+Alt+L / the menu's Lock take the same door).
//! * **A wrong password** is the login screen's one answer (BP-security-4); here it is COUNTED and the card SHAKES
//!   (six 30 ms steps, none under Reduce motion).
//! * Wire: `[lock] enter cause=<idle|lid|sleep|key> windows_hidden=<n>` · `[lock] wrong n=<n> shake=<…>` ·
//!   `[lock] open ms=<n> wrong=<n> resumed=<n>` · `[lock] policy after_idle_min=<n> on_lid=<0|1> on_sleep=<0|1> via=` ·
//!   `tests autolock` -> `:: AUTOLOCK: entered=1 leaked_keys=0 reopened=ok table_same=1 ms=<n> … prefs=client -> PASS ::`
//!   (the fixture body is `tests.rs`'s tail).
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

/// What locked the glass.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cause {
    /// No key or pointer event for `system.lock.after_idle_min`.
    Idle,
    /// The lid closed (`system.lock.on_lid`), before the sleep state.
    Lid,
    /// Any other sleep entry (`system.lock.on_sleep`): the login window's Sleep, a future verb.
    Sleep,
    /// The user asked: Win+L, ⌘L, Ctrl+Alt+L, the menu's Lock.
    Key,
}

impl Cause {
    pub fn word(self) -> &'static str {
        match self { Cause::Idle => "idle", Cause::Lid => "lid", Cause::Sleep => "sleep", Cause::Key => "key" }
    }
}

/// `system.lock.after_idle_min`'s default (the schema's).
pub const IDLE_MIN_DEFAULT: u32 = 10;
/// The Settings row's steps (minutes; 0 = never).
pub const IDLE_STEPS: [u32; 5] = [0, 5, 10, 30, 60];

/// The veil is on (the window table reads hidden). Atomics only: [`veiled`] runs under the window table's lock.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static CAUSE: AtomicU8 = AtomicU8::new(0);
static WRONG: AtomicU32 = AtomicU32::new(0);
static HIDDEN: AtomicU32 = AtomicU32::new(0);
static ENTERS: AtomicU32 = AtomicU32::new(0);
static OPENS: AtomicU32 = AtomicU32::new(0);
/// The last [`opened`]'s cost: the right password's verdict applied -> the restoring composite returned (ms).
static LAST_OPEN_MS: AtomicU64 = AtomicU64::new(u64::MAX);
static ENTER_AT_MS: AtomicU64 = AtomicU64::new(0);
/// LOCK2 (B728): app owners the last [`opened`]'s resume pass (`wm::lock_resume`) brought back.
static RESUMED: AtomicU32 = AtomicU32::new(0);
/// LOCK2 (B728): the civil minute the locked card last painted its clock for (`unix / 60`; 0 = none yet).
static CLOCK_MIN: AtomicU64 = AtomicU64::new(0);

/// The policy in force (the store's, read once a second; a Settings press sets it at once).
static AFTER_IDLE_MIN: AtomicU32 = AtomicU32::new(IDLE_MIN_DEFAULT);
static ON_LID: AtomicBool = AtomicBool::new(true);
static ON_SLEEP: AtomicBool = AtomicBool::new(true);
/// LOCKPREFS (B748): the PrefChanged mark (LOCKCHANGED B758: `prefs_client::changed_mark`'s sequence, kept beside
/// `CHANGED_N`) and the login generation (`fs::users::SESSION_GEN`) the policy was last read under; [`prefs_pass`]
/// re-reads when the generation moved or a change since the mark named one of the lock's three keys.
static SEEN_CHANGED: AtomicU32 = AtomicU32::new(0);
/// LOCKCHANGED (B758): bus GETs [`refresh`] has made (three per re-read; the fixture's `gets=`).
static GETS: AtomicU32 = AtomicU32::new(0);
static SEEN_GEN: AtomicU32 = AtomicU32::new(0);
static PRIMED: AtomicBool = AtomicBool::new(false);
/// Store writes owed by a Settings press: bit 0 after_idle_min, bit 1 on_lid, bit 2 on_sleep. While any is owed the
/// once-a-second read does not overwrite the press with the store's older value.
static OWED: AtomicU8 = AtomicU8::new(0);
/// The activity stamp an idle lock was last tried for (one try per idle stretch, never a loop of refusals).
static IDLE_TRIED: AtomicU64 = AtomicU64::new(0);

/// The shake: the row, its step (0 = none; 1..=len the offsets, len+1 the exact-restore check), the last step's time
/// and the row's outer origin before the first step.
static SHAKE_ID: AtomicU32 = AtomicU32::new(0);
static SHAKE_STEP: AtomicU8 = AtomicU8::new(0);
static SHAKE_AT_MS: AtomicU64 = AtomicU64::new(0);
static SHAKE_FX: AtomicU32 = AtomicU32::new(0);
static SHAKE_FY: AtomicU32 = AtomicU32::new(0);
/// Logical px offsets (device-scaled at the step), ending at the origin.
const SHAKE: [i32; 6] = [-10, 10, -6, 6, -3, 0];
const SHAKE_STEP_MS: u64 = 30;

/// The veil is on: every row but the modal one reads below the shell (`wm::above_shell`'s lock term).
#[inline]
pub fn veiled() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// The session is locked (the veil is on).
pub fn active() -> bool {
    veiled()
}

/// Wrong passwords on the current (or last) lock.
pub fn wrong_count() -> u32 {
    WRONG.load(Ordering::Relaxed)
}

/// The last unlock's restore cost in ms (`None` = no unlock yet this boot).
pub fn last_open_ms() -> Option<u64> {
    let v = LAST_OPEN_MS.load(Ordering::Relaxed);
    if v == u64::MAX { None } else { Some(v) }
}

/// App owners the last unlock resumed (LOCK2, `wm::lock_resume`).
pub fn last_resumed() -> u32 {
    RESUMED.load(Ordering::Relaxed)
}

/// Windows the last lock hid.
pub fn hidden() -> u32 {
    HIDDEN.load(Ordering::Relaxed)
}

/// The policy in force: `(after_idle_min, on_lid, on_sleep)`.
pub fn policy() -> (u32, bool, bool) {
    (AFTER_IDLE_MIN.load(Ordering::Relaxed), ON_LID.load(Ordering::Relaxed), ON_SLEEP.load(Ordering::Relaxed))
}

/// A Settings press: apply at once, owe the store write ([`persist`], on the Settings service pass).
pub fn set_policy(after_idle_min: Option<u32>, on_lid: Option<bool>, on_sleep: Option<bool>) {
    if let Some(m) = after_idle_min {
        AFTER_IDLE_MIN.store(m.min(1440), Ordering::Relaxed);
        IDLE_TRIED.store(0, Ordering::Relaxed);
        OWED.fetch_or(1, Ordering::AcqRel);
    }
    if let Some(v) = on_lid {
        ON_LID.store(v, Ordering::Relaxed);
        OWED.fetch_or(2, Ordering::AcqRel);
    }
    if let Some(v) = on_sleep {
        ON_SLEEP.store(v, Ordering::Relaxed);
        OWED.fetch_or(4, Ordering::AcqRel);
    }
}

/// The owed store writes (Principia's `system.lock.*`), from the Settings service pass — never from a router. LOCKPREFS
/// (B748): through the bus client (`prefs_client::sys_set`: from the render task the PrefSet runs on a worker, and
/// `sys_get` answers the queued value meanwhile); the store's PrefChanged then reaches [`prefs_pass`].
pub fn persist() {
    use crate::prefs::key;
    use crate::prefs_client::{sys_set as set_sys, PrefValue};
    let o = OWED.swap(0, Ordering::AcqRel);
    if o & 1 != 0 { set_sys(key::LOCK_AFTER_IDLE_MIN, PrefValue::Int(AFTER_IDLE_MIN.load(Ordering::Relaxed) as i64)); }
    if o & 2 != 0 { set_sys(key::LOCK_ON_LID, PrefValue::Bool(ON_LID.load(Ordering::Relaxed))); }
    if o & 4 != 0 { set_sys(key::LOCK_ON_SLEEP, PrefValue::Bool(ON_SLEEP.load(Ordering::Relaxed))); }
}

/// LOCKPREFS (B748): the policy's half of a service pass. Re-reads the three keys over the bus when a PrefChanged
/// landed since the last read (every accepted store write, whoever wrote it, while a kernel subscriber is registered —
/// the lock registers itself on its first pass) or the session moved (a login or logout swaps the store's home and
/// fires no PrefChanged); otherwise a few atomic loads and out. While a Settings press is owed the press is newer than
/// the store: nothing is read and the edge is kept for the pass after [`persist`]. The mark is taken BEFORE the read,
/// so a write that lands mid-read moves it again and the next pass re-reads. LOCKCHANGED (B758): a change re-reads only
/// when it named one of the three keys (`prefs_client::changed_since`, the last-changed keys kept beside `CHANGED_N`; a
/// change the ring lost counts as a hit); any other key's change moves the mark and costs no GET. The edge is CLAIMED
/// (compare-exchange on the mark, swap on the generation), so two passes never read the same edge. `true` = it re-read.
pub fn prefs_pass() -> bool {
    let primed = PRIMED.load(Ordering::Acquire);
    if !primed {
        crate::prefs_client::subscribe();
    }
    if OWED.load(Ordering::Acquire) != 0 {
        return false;
    }
    let mark = SEEN_CHANGED.load(Ordering::Acquire);
    let (n, hit) = crate::prefs_client::changed_since(mark, is_lock_key);
    let g = crate::fs::users::SESSION_GEN.load(Ordering::Acquire);
    if primed && n == mark && SEEN_GEN.load(Ordering::Acquire) == g {
        return false;
    }
    if n != mark && SEEN_CHANGED.compare_exchange(mark, n, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return false; // another pass claimed this edge (and reads it if it was ours)
    }
    let gen_moved = SEEN_GEN.swap(g, Ordering::AcqRel) != g;
    PRIMED.store(true, Ordering::Release);
    if primed && !hit && !gen_moved {
        return false; // LOCKCHANGED (B758): another key's change — the policy cannot have moved
    }
    refresh();
    true
}

/// LOCKCHANGED (B758): `<ns>.<key>` is one of the three keys the lock reads.
fn is_lock_key(ns: &str, k: &str) -> bool {
    use crate::prefs::key;
    ns == crate::prefs::NS && (k == key::LOCK_AFTER_IDLE_MIN || k == key::LOCK_ON_LID || k == key::LOCK_ON_SLEEP)
}

/// LOCKCHANGED (B758): bus GETs the policy reads have made since boot (three per re-read).
pub fn gets() -> u32 {
    GETS.load(Ordering::Relaxed)
}

/// Read the three keys over the bus (Principia's client; the schema's defaults where unset or out of range) and make
/// them the policy in force. One line when a value moved.
fn refresh() {
    use crate::prefs::key;
    use crate::prefs_client::{sys_flag, sys_int};
    let m = sys_int(key::LOCK_AFTER_IDLE_MIN, 0, 1440).map_or(IDLE_MIN_DEFAULT, |v| v as u32);
    let lid = sys_flag(key::LOCK_ON_LID).unwrap_or(true);
    let slp = sys_flag(key::LOCK_ON_SLEEP).unwrap_or(true);
    GETS.fetch_add(3, Ordering::Relaxed);
    if OWED.load(Ordering::Acquire) != 0 {
        return; // a Settings press landed during the read: it is newer than what the store answered
    }
    let m0 = AFTER_IDLE_MIN.swap(m, Ordering::Relaxed);
    let lid0 = ON_LID.swap(lid, Ordering::Relaxed);
    let slp0 = ON_SLEEP.swap(slp, Ordering::Relaxed);
    if (m0, lid0, slp0) != (m, lid, slp) {
        serial_println!("[lock] policy after_idle_min={} on_lid={} on_sleep={} via={} (prefs_client, PrefChanged)", m, lid as u8, slp as u8, crate::prefs_client::last_via());
    }
}

/// Is there a session the lock could close over, with the screen down? (No lock line for a refusal nobody asked for.)
fn lockable() -> bool {
    let mut nb = [0u8; crate::fs::users::NAME_MAX];
    crate::fs::users::whoami(&mut nb).is_some() && !super::crystal::login::is_open()
}

/// **Lock the session** (`cause`). The login screen comes up in its locked mode (`login::lock_screen`: the session's
/// name read-only, the password field, the modal pin) and THEN the veil goes on, so the table is never seen veiled
/// without the card that opens it. Refused (`false`, one line) when the screen refuses: no session, the screen
/// already up, or a session user with no password to unlock with (BP-security-4: a lock nobody can lift is a dead
/// machine). Nothing is closed, swept or moved.
pub fn enter(cause: Cause) -> bool {
    if ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let n = visible_windows();
    if !super::crystal::login::lock_screen() {
        serial_println!("[lock] refused cause={} (no session, the screen already up, or no password to unlock with)", cause.word());
        return false;
    }
    HIDDEN.store(n as u32, Ordering::Relaxed);
    WRONG.store(0, Ordering::Relaxed);
    CAUSE.store(cause as u8, Ordering::Relaxed);
    ENTER_AT_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
    ENTERS.fetch_add(1, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Release);
    if super::look::is_softy() {
        centre_card(); // SOFTYDOTS (B721): the look's one branch — the Softy lock view centres the card (its taskbar is gone)
    }
    wipe();
    serial_println!("[lock] enter cause={} windows_hidden={}", cause.word(), n);
    true
}

/// SOFTYDOTS (rmbp-ledger B721, M2) — ONE lock, two paints. Under the Softy look the locked view has no taskbar (the
/// band vacates itself while [`veiled`], `softy::taskbar::compose`) and the card — the SAME login screen row, the same
/// password field — sits at the panel's centre instead of the upper third the Una view keeps under its menu bar.
/// Moved once, here, before the veil's first frame; the shake records its origin after this, so it ends centred.
fn centre_card() {
    let Some(fb) = super::panel_snapshot().filter(|f| f.is_ready()) else { return };
    let ph = fb.height();
    let Some((id, (fx, fy, _, fh))) = card() else {
        serial_println!("[lock] card centre look=softy -> no-row");
        return;
    };
    let ty = ph.saturating_sub(fh) / 2;
    if fy == ty {
        return;
    }
    let ok = super::wm::move_to(id, fx + super::wm::BORDER(), ty + super::wm::TITLE_H() + super::wm::BORDER());
    serial_println!("[lock] card centre look=softy y={}->{} ok={}", fy, ty, ok as u8);
}

/// The locked card's row and its outer frame `(x, y, w, h)`, found from the shared password field's panel rect
/// (`login::field_rect_panel` = `(x, y + top + q, w, q)` with `top = TITLE_H`, `q = (h - top) / 3`): the shell-band row
/// (owner 0, the login screen's) whose frame gives back exactly that field. `None` while no card is up.
pub fn card() -> Option<(super::wm::WinId, (usize, usize, usize, usize))> {
    let (fx, fy, fw, fh) = super::crystal::login::field_rect_panel()?;
    let top = super::wm::TITLE_H();
    let y = fy.checked_sub(top + fh)?;
    for id in 1..=super::wm::slots() as super::wm::WinId {
        if super::wm::owner_of(id) != Some(0) {
            continue;
        }
        let Some(f) = super::wm::frame_of(id) else { continue };
        if f.0 == fx && f.1 == y && f.2 == fw && f.3 >= top && (f.3 - top) / 3 == fh {
            return Some((id, f));
        }
    }
    None
}

/// The app windows on the glass now (the count the veil hides).
fn visible_windows() -> usize {
    let mut rows = alloc::vec::Vec::new();
    let n = super::wm::wl_rows(&mut rows);
    rows[..n].iter().filter(|r| !r.minimised).count()
}

/// The veil's first frame: the panel to the desktop colour, every row damaged, one composite (only the modal card
/// paints), and the desktop layer asked for its full present (the wallpaper back under the card). Skipped while the
/// panel is refused (the idle blank, the sleep state): their wake repaints through the same veil.
fn wipe() {
    if super::panel_refuse_term().is_some() {
        return;
    }
    if let Some(fb) = super::panel_snapshot().filter(|f| f.is_ready()) {
        fb.fill_screen(super::blitter::a11yfilter::color(super::wm::DESKTOP_BG)); // crawl A 17: the scanout writer filters at the copy (BP-video-25)
        fb.flush_all();
        let (w, h) = (fb.width(), fb.height());
        let _ = super::wm::damage_intersecting(0, 0, w, h);
    }
    super::wm::composite();
    super::screen::request_full_present();
}

/// The right password (the login screen's verdict, `apply_unlock`, after its row is down): the veil off, every row
/// damaged, ONE composite — the table was never touched, so the focus and the z-order are the ones the lock found.
/// LOCK2 (B728): before that composite, the resume pass (`wm::lock_resume`): every app owner the table shows on the
/// glass again is published unhidden (its wake edge; a program the veil paused runs again) and the whole panel is
/// queued as wallpaper-aware desktop damage, so the card's vacated rect is repainted by this same pass, not by the
/// desktop's next tick (up to a second).
pub fn opened() {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let t0 = crate::arch::ms();
    SHAKE_STEP.store(0, Ordering::Relaxed);
    ACTIVE.store(false, Ordering::Release);
    let resumed = super::wm::lock_resume();
    RESUMED.store(resumed as u32, Ordering::Relaxed);
    CLOCK_MIN.store(0, Ordering::Relaxed);
    if let Some(fb) = super::panel_snapshot().filter(|f| f.is_ready()) {
        let (w, h) = (fb.width(), fb.height());
        let _ = super::wm::damage_intersecting(0, 0, w, h);
    }
    super::wm::composite();
    super::screen::request_full_present();
    let ms = crate::arch::ms().saturating_sub(t0);
    LAST_OPEN_MS.store(ms, Ordering::Relaxed);
    OPENS.fetch_add(1, Ordering::Relaxed);
    IDLE_TRIED.store(super::dimidle::last_activity_ms(), Ordering::Relaxed);
    serial_println!("[lock] open ms={} wrong={} resumed={}", ms, WRONG.load(Ordering::Relaxed), resumed);
}

/// A wrong password on the locked screen (`apply_unlock`'s refusal; the screen says its one answer): counted, and the
/// card `id` shakes on the next service passes (none under Reduce motion).
pub fn wrong(id: u32) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let n = WRONG.fetch_add(1, Ordering::Relaxed) + 1;
    let how = if super::settings::reduce_motion() {
        "reduce-motion"
    } else if let Some((fx, fy, _, _)) = super::wm::frame_of(id) {
        SHAKE_ID.store(id, Ordering::Relaxed);
        SHAKE_FX.store(fx as u32, Ordering::Relaxed);
        SHAKE_FY.store(fy as u32, Ordering::Relaxed);
        SHAKE_AT_MS.store(0, Ordering::Relaxed);
        SHAKE_STEP.store(1, Ordering::Release);
        "on"
    } else {
        "no-row"
    };
    serial_println!("[lock] wrong n={} shake={}", n, how);
}

/// The lid closing or another sleep entry (`power::sleep::enter`, before its line): lock first when the policy says
/// so. `tests` (the `tests sleep` fixture) never locks; neither does a sleep asked from the screen itself.
pub fn before_sleep(via: &str) {
    if via == "tests" || ACTIVE.load(Ordering::Acquire) || !lockable() {
        return;
    }
    let (cause, on, key) = if via == "lid" {
        (Cause::Lid, ON_LID.load(Ordering::Relaxed), "on_lid")
    } else {
        (Cause::Sleep, ON_SLEEP.load(Ordering::Relaxed), "on_sleep")
    };
    if !on {
        serial_println!("[lock] skip cause={} (system.lock.{}=false)", cause.word(), key);
        return;
    }
    let _ = enter(cause);
}

/// The render pulse (`login::submit_drain`'s head): the policy on a PrefChanged ([`prefs_pass`]), the idle lock, the shake's steps, and
/// the screen's own guard — the veil never outlives the card while a session is open (a card closed by another path
/// is put back; with no session the veil lifts: nothing is left under it to hide).
pub fn service() {
    let now = crate::arch::ms();
    let _ = prefs_pass();
    if ACTIVE.load(Ordering::Acquire) {
        if SHAKE_STEP.load(Ordering::Acquire) != 0 {
            shake_step(now);
        }
        clock_tick();
        if !super::crystal::login::is_locked() {
            let mut nb = [0u8; crate::fs::users::NAME_MAX];
            if crate::fs::users::whoami(&mut nb).is_some() && !super::crystal::login::is_open() && super::crystal::login::lock_screen() {
                serial_println!("[lock] screen put back (the card went down without the password; the veil stays)");
            } else if crate::fs::users::whoami(&mut nb).is_none() {
                ACTIVE.store(false, Ordering::Release);
                serial_println!("[lock] lifted reason=no-session");
            }
        }
        return;
    }
    let m = AFTER_IDLE_MIN.load(Ordering::Relaxed);
    if m == 0 {
        return;
    }
    let act = super::dimidle::last_activity_ms();
    if act == 0 || now.saturating_sub(act) < m as u64 * 60_000 || IDLE_TRIED.load(Ordering::Relaxed) == act {
        return;
    }
    IDLE_TRIED.store(act, Ordering::Relaxed);
    if lockable() {
        let _ = enter(Cause::Idle);
    }
}

/// One step of the shake: the card's content origin moved by the step's offset (one composite), and after the last
/// step its outer origin checked against the one recorded and corrected once, so the card ends exactly where it was.
fn shake_step(now: u64) {
    if now.wrapping_sub(SHAKE_AT_MS.load(Ordering::Relaxed)) < SHAKE_STEP_MS {
        return;
    }
    SHAKE_AT_MS.store(now.max(1), Ordering::Relaxed);
    let id = SHAKE_ID.load(Ordering::Relaxed);
    let st = SHAKE_STEP.load(Ordering::Acquire) as usize;
    let (fx, fy) = (SHAKE_FX.load(Ordering::Relaxed) as usize, SHAKE_FY.load(Ordering::Relaxed) as usize);
    let (cx, cy) = (fx + super::wm::BORDER(), fy + super::wm::TITLE_H() + super::wm::BORDER());
    if st >= 1 && st <= SHAKE.len() {
        let o = SHAKE[st - 1];
        let d = super::metrics::size(o.unsigned_abs() as usize);
        let x = if o < 0 { cx.saturating_sub(d) } else { cx + d };
        if !super::wm::move_to(id, x, cy) {
            SHAKE_STEP.store(0, Ordering::Relaxed);
            return;
        }
        super::wm::composite();
        SHAKE_STEP.store(st as u8 + 1, Ordering::Release);
        return;
    }
    if let Some((x1, y1, _, _)) = super::wm::frame_of(id) {
        if (x1, y1) != (fx, fy) {
            let x = (cx + fx).saturating_sub(x1);
            let y = (cy + fy).saturating_sub(y1);
            let _ = super::wm::move_to(id, x, y);
            super::wm::composite();
        }
    }
    SHAKE_STEP.store(0, Ordering::Relaxed);
}

/// The window table as its read faces show it — every app row's id, owner, minimised bit (its z against the shell),
/// title and outer frame, then the focused row — folded to FNV-1a 64. `tests autolock` compares it before the lock
/// and after the unlock.
pub fn fingerprint() -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |v: u64| {
        for b in v.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    let mut rows = alloc::vec::Vec::new();
    let n = super::wm::wl_rows(&mut rows);
    eat(n as u64);
    for r in rows[..n].iter() {
        eat(r.id as u64);
        eat(r.owner);
        eat(r.minimised as u64);
        for &b in &r.title[..r.len] {
            eat(b as u64);
        }
        let (x, y, w, hh) = super::wm::frame_of(r.id).unwrap_or((0, 0, 0, 0));
        eat(x as u64);
        eat(y as u64);
        eat(w as u64);
        eat(hh as u64);
    }
    match super::wm::wl_focused() {
        Some((id, owner)) => {
            eat(id as u64);
            eat(owner);
        }
        None => eat(u64::MAX),
    }
    h
}

/// LOCK2 (B728): the locked card prints the civil clock (`login`'s painter, through `clock::locale::fmt_time`, the one
/// formatter); this repaints the card once per civil minute while the veil is on (one card repaint a minute, never per
/// frame). `try_unix_now`: a contended or unanchored clock skips this pass.
fn clock_tick() {
    let Some(secs) = crate::clock::try_unix_now() else { return };
    let m = secs / 60 + 1;
    if CLOCK_MIN.swap(m, Ordering::Relaxed) != m {
        super::crystal::login::font_repaint();
    }
}
