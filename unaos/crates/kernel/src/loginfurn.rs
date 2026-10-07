//! CHARTER: Kernel — kernel-by-ruling (R88 LOGINFURN: the bare login, the console's boot-text prefill, `tests loginfurn`)
//!
//! LOGINFURN (rmbp-ledger B374, R88). Peter, flight 23, at the glass: "stat, console, and shell are opening still on
//! login. console is doubly wrong because it is blank. it should load the text from the current boot."
//!
//! * [`desktop_bare`] — the login (or the installer's release) opens NOTHING: no console, no shell, no STAT. The
//!   R77/R86 furniture re-mint (`dock::relaunch_furniture` from `login::close_into_session` and the installer's release, both deleted by DESKTOPBUILT B387)
//!   is gone; the services do not wait the furniture bound for furniture that is not coming
//!   (`boot::furniture_none`). One line: `[login] desktop bare: furniture=none (R88) why=<session|installer>`.
//! * [`console_prefill`] — every console mint replays the CURRENT BOOT's text from the one ring the kernel already
//!   keeps of it (`flight_recorder`, the x86 serial seam's 256 KiB capture `UNAOS.LOG` is flushed from; R79: no
//!   second ring), the grid's rows of lines, through `fbcon::_print` under ONE present — so the console opens on
//!   the boot's tail, never blank. `[console] prefill win=<id> lines=<n> painted=<p> … live=<0|1>`.
//! * the census — [`note_window`] (from `boot::note_window`) counts rows a non-zero owner mints within
//!   [`AT_LOGIN_MS`] of the login's ignition; [`self_launch`] counts a kernel self-launch at login (a furniture
//!   post, a STAT launch). `tests loginfurn` ([`loginfurn_selftest`], R80: never at boot) reads them:
//!   `:: LOGINFURN: at_login windows=0 services=0 console_prefill_lines=<n> -> PASS ::`.
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

/// The login's settle window: a row minted this soon after the ignition was not opened by the user's hand.
pub const AT_LOGIN_MS: u64 = 1_500;

static BARE: AtomicU32 = AtomicU32::new(0);
static AT_LOGIN_WINDOWS: AtomicU32 = AtomicU32::new(0);
static FIRST_AT_LOGIN_OWNER: AtomicU64 = AtomicU64::new(0);
static SELF_LAUNCHES: AtomicU32 = AtomicU32::new(0);
static FIRST_SELF: crate::sync::Mutex<&'static str> = crate::sync::Mutex::new("");
/// The first prefill's painted lines (`u64::MAX` = the console has not been opened since the boot).
static PREFILL_FIRST: AtomicU64 = AtomicU64::new(u64::MAX);
static PREFILLS: AtomicU32 = AtomicU32::new(0);

fn in_login_window() -> bool {
    let ig = crate::boot::ignite_ms();
    ig != 0 && crate::arch::ms() <= ig.saturating_add(AT_LOGIN_MS)
}

/// The desktop is released with no furniture (the first login, a relogin, the installer's advance).
pub fn desktop_bare(why: &'static str) {
    BARE.fetch_add(1, Relaxed);
    crate::boot::furniture_none();
    serial_println!("[login] desktop bare: furniture=none (R88) why={} — no console, shell or STAT opens itself; the user opens what they want", why);
}

/// `boot::note_window`: a row is being minted (pure apart from atomics; called under the window table).
pub fn note_window(owner: u64) {
    if owner != 0 && in_login_window() {
        if take_item_credit() { return; } // PREFSUI M6 (R91): a window the user's login items asked for is not a window that opened itself
        if AT_LOGIN_WINDOWS.fetch_add(1, Relaxed) == 0 {
            FIRST_AT_LOGIN_OWNER.store(owner, Relaxed);
        }
    }
}

/// A kernel path launched something by itself (`who`): counted when it lands in the login's settle window.
pub fn self_launch(who: &'static str) {
    if in_login_window() {
        SELF_LAUNCHES.fetch_add(1, Relaxed);
        if let Some(mut f) = FIRST_SELF.try_lock() {
            if f.is_empty() {
                *f = who;
            }
        }
    }
}

/// STAT.ELF is not desktop furniture on a `login` build (R88): the user opens it.
pub fn stat_held() -> bool {
    cfg!(feature = "login")
}

/// The loginst fixtures: a session opened BARE — neither the console's nor the shell's launch is posted.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn nothing_posted() -> bool {
    use crate::video::dock::{launch_posted, PinnedApp};
    !launch_posted(PinnedApp::Console) && !launch_posted(PinnedApp::Shell)
}

/// `fbcon::panel_console_window_open`, after the route is installed (FBCON released, interrupts on): replay the
/// current boot's tail into the fresh console. `rows` is the grid's height; `retained_rows` > 0 means the window
/// re-adopted a cell store that already carries text (no replay — it would print the boot twice).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn console_prefill(id: crate::video::wm::WinId, rows: usize, retained_rows: usize) {
    // FLIGHTRING (B400): the ring is `boot_ring` on both arches now — a pinned head + the rolling newest bytes — so
    // the tail painted here is the LIVE tail however late the console opens (flight 24 card 3 read `ring_full=1`).
    use crate::video::fbcon;
    crate::flightring::note_rows(rows);
    let tail = crate::boot_ring::tail(rows.max(1), 0);
    let live = fbcon::console_takes_glyphs();
    let Some(t) = tail else {
        serial_println!("[console] prefill win={} lines=0 painted=0 ring=none live={} (R88: the boot-log ring was contended)", id, live as u32);
        return;
    };
    let mut lines = 0usize;
    let mut painted = 0u64;
    if retained_rows == 0 {
        (lines, painted) = console_replay(&t.bytes);
        let _ = PREFILL_FIRST.compare_exchange(u64::MAX, painted, Relaxed, Relaxed);
        PREFILLS.fetch_add(1, Relaxed);
    }
    crate::flightring::note_prefill(lines);
    serial_println!(
        "[console] prefill win={} lines={} painted={} ring_bytes={} wrapped={} joined={} rolling_kib={} tail_live=1 live={} retained_rows={} (R88/FLIGHTRING: the current boot's newest text, scrolled to the tail)",
        id, lines, painted, t.total, t.wrapped as u32, t.joined as u32, t.rolling_kib, live as u32, retained_rows
    );
}

/// Paint `bytes` (whole lines) into the console through `fbcon::_print` under ONE present. Returns (lines, painted).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn console_replay(bytes: &[u8]) -> (usize, u64) {
    use crate::video::fbcon;
    let was = fbcon::console_present_suspended();
    fbcon::console_present_suspend(true); // one present for the whole replay, not one per line
    let tap = &crate::serial_ring::TAP_FBCON;
    let a0 = tap.absorbed.load(Relaxed);
    let mut lines = 0usize;
    for raw in bytes.split(|&b| b == b'\n') {
        let line = match raw.last() {
            Some(b'\r') => &raw[..raw.len() - 1],
            _ => raw,
        };
        if line.is_empty() {
            continue;
        }
        let s = match core::str::from_utf8(line) {
            Ok(s) => s,
            Err(e) => core::str::from_utf8(&line[..e.valid_up_to()]).unwrap_or(""),
        };
        fbcon::_print(format_args!("{}\n", s));
        lines += 1;
    }
    let painted = tap.absorbed.load(Relaxed).saturating_sub(a0);
    if !was {
        fbcon::console_present_suspend(false); // resuming forces the one present
    }
    (lines, painted)
}

/// Register `tests loginfurn` once (folded into `boot::ensure_tests`).
pub fn ensure_tests() {
    static DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if !DONE.swap(true, core::sync::atomic::Ordering::AcqRel) {
        crate::tests::register("loginfurn", loginfurn_selftest);
    }
    crate::flightring::ensure_tests(); // FLIGHTRING (B400): `tests flightring`
}

/// `tests loginfurn` — what the login opened by itself, and what the console showed when it was opened.
/// SKIP when the boot had no login (a Desktop from the first instruction).
pub fn loginfurn_selftest() {
    if crate::boot::ignite_ms() == 0 {
        serial_println!(":: LOGINFURN: at_login windows=- login_items=- services=- console_prefill_lines=- -> SKIP reason=no-login-this-boot ::");
        return;
    }
    let w = AT_LOGIN_WINDOWS.load(Relaxed);
    let s = SELF_LAUNCHES.load(Relaxed);
    let p = PREFILL_FIRST.load(Relaxed);
    let opened = p != u64::MAX;
    let ring = if opened { 0 } else { ring_tail_lines() }; // SMALLFIX7 (B501): no console this boot — the login's own record
    let pass = w == 0 && s == 0 && BARE.load(Relaxed) >= 1 && (if opened { p > 0 } else { ring > 0 });
    if !pass {
        serial_println!(
            ":: LOGINFURN: reason=windows={} first_owner={:#x} services={} first_self={} bare={} prefill={} ::",
            w, FIRST_AT_LOGIN_OWNER.load(Relaxed), s, FIRST_SELF.try_lock().map(|f| *f).unwrap_or("?"), BARE.load(Relaxed),
            if opened { alloc::format!("{}", p) } else { alloc::string::String::from("none") }
        );
    }
    serial_println!(
        ":: LOGINFURN: at_login windows={} login_items={} services={} console_prefill_lines={}{} -> {} ::{}",
        w,
        ITEM_WINDOWS.load(Relaxed),
        s,
        if opened { alloc::format!("{}", p) } else { alloc::string::String::from("none") },
        if opened { alloc::string::String::new() } else { alloc::format!(" ring_tail_lines={}", ring) },
        if pass { "PASS" } else { "FAIL" },
        if opened { "" } else { " console=unopened (ring_tail_lines = what the console's prefill replays when it opens: the current boot's tail in the one ring)" }
    );
}

// ── PREFSUI M6 (rmbp-ledger B389, R88 + R91) — the user's login items are not furniture ─────────────────────────
// `loginitems::launch` posts the user's `system.login.items` through the dock's seams and hands this module a
// CREDIT: one window per item, plus the shell window a ring-3 item's verb runs in. A row minted in the login's
// settle window spends a credit first (counted as `login_items=`), and only a row with no credit left counts as a
// window that opened itself (`windows=`). An empty list hands no credit, so the gate reads exactly as before.

/// Windows the login items may still mint inside the settle window.
static ITEM_CREDIT: AtomicU32 = AtomicU32::new(0);
/// Rows minted in the settle window on a login item's credit.
static ITEM_WINDOWS: AtomicU32 = AtomicU32::new(0);

/// `loginitems::launch`: `n` windows were asked for by the user's login items (`via=login-items`).
pub fn login_items_posted(n: u32) {
    ITEM_CREDIT.store(n, Relaxed);
}

fn take_item_credit() -> bool {
    if ITEM_CREDIT.fetch_update(Relaxed, Relaxed, |c| c.checked_sub(1)).is_ok() {
        ITEM_WINDOWS.fetch_add(1, Relaxed);
        return true;
    }
    false
}

// ── SMALLFIX7 (rmbp-ledger B501) — TAIL-APPENDED ─────────────────────────────────────────────────────
// Flight 27: `console_prefill_lines=none (console=unopened)` — the count existed only once a console was minted,
// and the seat types into the headless door with no console open. The prefill replays `boot_ring::tail`; this
// reads the same tail at the same height the console asks for, without painting anything.

/// The console grid height this read asks the ring for (a nominal console; the real prefill asks for its own grid).
const PREFILL_ROWS: usize = 48;

/// Lines the console's prefill would replay if it opened now (0 when the ring is contended or empty).
fn ring_tail_lines() -> usize {
    crate::boot_ring::tail(PREFILL_ROWS, 0).map_or(0, |t| t.lines)
}
