//! CHARTER: Kernel — wm (LOGINRENDER B514: the render handler's own instrument and witness — what non-render job it ran, and how long it held with no pass)
//!
//! LOGINRENDER (rmbp-ledger B514). Flight 28, right after login: `[lag] stall … stage=render-handler stage_ms=1269
//! … passes=0 … handler=panel-render`, then 1711, 1903, 1403 — the render task held 1.3–1.9 s at a time inside
//! `pal.render()` (INPUTSTALL2's `panel-render` step) with ZERO composite passes, and the line did not say what the
//! step was doing. The wire beside it: every one of those seconds overlaps the `[filetypes] type=… masked_ms=447..666`
//! objects the jobs worker built on cpu 2, and the holds came every ~2 s (at_ms 35750, 37523, 39969, 41872) — the
//! cadence of WALLPAPER's default probe (`wallpaper::poll`: `RETRY_MS` 2000, `MAX_TRIES` 5), which `Screen::flush`
//! ran ON the render task: two `MountTable::stat`s (`~/Desktop/WALL.PNG`, `/WALL.PNG`) that waited on the UnaFS
//! volume the registry build held. `desktopbuild` re-arms it at `login ok`, so the probe's five tries land exactly
//! in the login minute (and, before login, in the boot's `worst_stage=render-handler` span).
//!
//! Three parts:
//! * THE STAMP ([`stamp`]): every non-render job the render task runs puts its name in ONE atomic ([`CUR`],
//!   HIDSTALL2's holder shape) on entry and clears it on exit; a finished stamp lands in an 8-record ring. The
//!   `[lag] stall` line, when its stage is `render-handler`, ends with ` handler_what=<name>:<ms>` — the longest
//!   stamp that overlapped the second (the one still in flight is charged its age), `-` when no stamped job ran.
//! * THE TURN ([`turn`], from `lag::render_entry`): every render-handler interval (route exit → the next route,
//!   pass or park) is measured; whether a composite pass ended inside it ([`pass`], from `lag::sec_pass`) makes it a
//!   ZERO-PASS turn — flight 28's shape.
//! * THE WITNESS (`tests loginrender`): over the first [`WINDOW_MS`] after `[users] login ok` (the session
//!   generation's first change; a build without `login` anchors at `phase=desktop`), `:: LOGINRENDER:
//!   handler_max_ms=<n> zero_pass_max_ms=<n> worst_what=<name> moved=<name:runs,…> -> PASS|FAIL :: bound_ms=100` —
//!   PASS iff no zero-pass turn exceeded [`BOUND_MS`]. R80: registered when the window opens (under
//!   `tests-at-boot`, when it closes, so the at-boot run reads the whole window), run only by the verb.
//!
//! x86 + `wc` only (the hooks are lag's, which is live only there); elsewhere every entry point is a no-op.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::{AcqRel, Acquire, Relaxed, Release}};

/// The instrument is live where `lag` is.
const ON: bool = cfg!(all(target_arch = "x86_64", feature = "wc"));
/// The witness's window after `login ok`.
pub const WINDOW_MS: u64 = 30_000;
/// A zero-pass handler turn over this fails the witness.
pub const BOUND_MS: u64 = 100;

/// Stamp names ([`stamp`] ids index this). A new non-render job on the render task adds its name here.
pub const WHAT_NAMES: [&str; 11] = ["-", "wallpaper-probe", "dock-console", "quarry", "quarry.open", "quarry.viewers", "quarry.settings", "quarry.font", "quarry.activity", "quarry.panes", "prefs-watch"];
/// WALLPAPER's default probe (`~/Desktop/WALL.PNG`, `/WALL.PNG`): two volume stats and, when found, a decode.
pub const W_WALLPAPER: u8 = 1;
/// The dock's console launch drain (`dock::console_launch_service`).
pub const W_DOCK_CONSOLE: u8 = 2;
/// Quarry's latch drain (`quarry::service`: an open is a panel read and two `read_dir`s).
pub const W_QUARRY: u8 = 3;
/// SMALLFIX10 (B539 item 3): the parts of `quarry::service`, stamped INSIDE the `quarry` stamp (a nested stamp —
/// see [`Stamp`]). Flight 31 named only `quarry:225`, the whole furniture drain; these say which drain it was.
/// `reap_jobs` + the dock's reopen latch (`open` reads the panel and two `read_dir`s).
pub const W_Q_OPEN: u8 = 4;
/// The viewers' latches: facet, fileview, player (and its `tick`), textedit.
pub const W_Q_VIEWERS: u8 = 5;
/// `settings::service`: launcher, appswitch, the filetype view, the name index, prefs (its 2 s delete watch:
/// two `stat`s per domain file under `SAVE_LOCK`), login items, settings files, appearance, brightness.
pub const W_Q_SETTINGS: u8 = 6;
/// The font path: `text::service` (the TrueType service), `metrics::ignite`, the KERNELFONT2 repaint pass.
pub const W_Q_FONT: u8 = 7;
/// Activity's once-a-second census repaint.
pub const W_Q_ACTIVITY: u8 = 8;
/// Quarry's own panes: the jobs status line, Quick Look, columns, open-with, the live directory relist.
pub const W_Q_PANES: u8 = 9;
/// SMALLFIX10 (B539 item 3, the seat's fold): the prefs delete watch (two `stat`s per settings domain under `SAVE_LOCK`,
/// every 2 s) — flight 31's `quarry:225` — runs on a worker core now; its runs are counted by [`moved`].
pub const W_PREFSWATCH: u8 = 10;
/// The jobs this arc moved OFF the render task (their runs on the worker are counted by [`moved`]).
const MOVED_IDS: [u8; 2] = [W_WALLPAPER, W_PREFSWATCH];

/// The stamp in flight on the render task, packed `start_ms << 8 | id` (0 = none). A nested stamp replaces it
/// and puts the enclosing one back when it drops (SMALLFIX10), so it always names the INNERMOST job in flight.
static CUR: AtomicU64 = AtomicU64::new(0);
/// SMALLFIX10 (B539 item 3): the longest nested stamp that finished inside the stamp in flight, packed
/// `dur_ms << 8 | id`; the enclosing stamp takes it (and clears it) when it drops.
static INNER: AtomicU64 = AtomicU64::new(0);
/// The last finished stamps, packed `start_ms << 32 | dur_ms << 8 | id` (ms fit 32/24 bits: 49 days / 4.6 h).
static RING: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static RING_HEAD: AtomicU32 = AtomicU32::new(0);

/// The window: the anchor (us on lag's clock, ms on `arch::ms`), 0 = not yet.
static LOGIN_US: AtomicU64 = AtomicU64::new(0);
static LOGIN_MS: AtomicU64 = AtomicU64::new(0);
/// Which anchor opened the window: 1 = `login ok` (session generation), 2 = `phase=desktop` (no `login`).
static ANCHOR: AtomicU32 = AtomicU32::new(0);
/// The window's readings, in us. `ZP_MAX` is the longest zero-pass turn; the whats are packed `us << 8 | id`.
static H_MAX_US: AtomicU64 = AtomicU64::new(0);
static ZP_MAX_US: AtomicU64 = AtomicU64::new(0);
static ZP_TURNS: AtomicU32 = AtomicU32::new(0);
static TURNS: AtomicU32 = AtomicU32::new(0);
static WIN_WHAT: AtomicU64 = AtomicU64::new(0);
static WIN_CLOSED: AtomicBool = AtomicBool::new(false);
/// The end of the last composite pass (lag's us clock).
static LAST_PASS_US: AtomicU64 = AtomicU64::new(0);
/// Runs of each moved job on its worker (index = stamp id).
static MOVED: [AtomicU32; WHAT_NAMES.len()] = [const { AtomicU32::new(0) }; WHAT_NAMES.len()];
static REG: AtomicBool = AtomicBool::new(false);

/// Is this the render task (`render`, or its re-homed twin)? lag's own test, repeated (lag's is private).
#[inline]
fn render_here() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        matches!(crate::arch::sched::current_name(), Some(n) if n.starts_with("render"))
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// A stamp in flight; dropping it records the job's interval. Inert off the render task.
///
/// SMALLFIX10 (B539 item 3) — NESTING. `.1` is the stamp this one interrupted (0 = none). A nested stamp is not
/// a ring record of its own (six sub-drains a pass would evict the ring's eight records every pass); it feeds
/// [`INNER`], and the enclosing stamp, when it drops, is RECORDED UNDER ITS LONGEST CHILD'S NAME if that child
/// took at least half of it — so the stall line and the witness say `quarry.settings:225`, not `quarry:225`,
/// and a stamp whose time was spread across its parts keeps its own name.
pub struct Stamp(u64, u64);

/// A non-render job begins on the render task: its name goes in [`CUR`] until the returned [`Stamp`] drops.
#[inline]
pub fn stamp(id: u8) -> Stamp {
    if !ON || !render_here() {
        return Stamp(0, 0);
    }
    let w = (crate::arch::ms().max(1) << 8) | id as u64;
    let outer = CUR.swap(w, AcqRel);
    if outer == 0 {
        INNER.store(0, Release); // a fresh outermost stamp: no child of an earlier one is charged to it
    }
    Stamp(w, outer)
}

impl Drop for Stamp {
    fn drop(&mut self) {
        if self.0 == 0 {
            return;
        }
        let _ = CUR.compare_exchange(self.0, self.1, AcqRel, Relaxed); // SMALLFIX10: the enclosing stamp is in flight again
        let (start, mut id) = (self.0 >> 8, self.0 & 0xff);
        let dur = crate::arch::ms().saturating_sub(start).min(0xff_ffff);
        if self.1 != 0 {
            INNER.fetch_max((dur << 8) | id, Relaxed); // SMALLFIX10: a child — charged to its parent's record, not the ring
            return;
        }
        let inner = INNER.swap(0, AcqRel);
        if inner != 0 && (inner >> 8) * 2 >= dur {
            id = inner & 0xff; // SMALLFIX10: the child that took at least half names the record
        }
        let i = (RING_HEAD.fetch_add(1, Relaxed) as usize) % RING.len();
        RING[i].store(((start & 0xffff_ffff) << 32) | (dur << 8) | id, Release);
        let l0 = LOGIN_MS.load(Acquire);
        if l0 != 0 && start >= l0 && start <= l0 + WINDOW_MS {
            WIN_WHAT.fetch_max(((dur * 1000) << 8) | id, Relaxed);
        }
    }
}

/// A moved job ran on its worker (the witness's `moved=` counts these).
pub fn moved(id: u8) {
    if let Some(c) = MOVED.get(id as usize) {
        c.fetch_add(1, Relaxed);
    }
}

/// `lag::sec_pass`: a composite pass ended at `now_us`.
#[inline]
pub fn pass(now_us: u64) {
    if ON {
        LAST_PASS_US.fetch_max(now_us, Relaxed);
    }
}

/// The session's anchor: the first `login ok` (the session generation leaves 0), else, with no `login`, the desktop.
fn anchored() -> u32 {
    #[cfg(feature = "login")]
    {
        if crate::fs::users::SESSION_GEN.load(Acquire) != 0 { 1 } else { 0 }
    }
    #[cfg(not(feature = "login"))]
    {
        if crate::boot::phase() == crate::boot::Phase::Desktop { 2 } else { 0 }
    }
}

/// `lag::render_entry`: one render-handler turn ran from `ex_us` to `now_us` (lag's clock).
pub fn turn(ex_us: u64, now_us: u64) {
    if !ON {
        return;
    }
    if !cfg!(feature = "tests-at-boot") && !REG.load(Relaxed) {
        ensure();
    }
    let mut l0 = LOGIN_US.load(Acquire);
    if l0 == 0 {
        let a = anchored();
        if a == 0 {
            return;
        }
        LOGIN_MS.store(crate::arch::ms().max(1), Release);
        LOGIN_US.store(now_us.max(1), Release);
        ANCHOR.store(a, Relaxed);
        serial_println!("[loginrender] window open anchor={} window_ms={} bound_ms={}", anchor_word(a), WINDOW_MS, BOUND_MS);
        l0 = now_us.max(1);
    }
    if WIN_CLOSED.load(Relaxed) {
        return;
    }
    if now_us > l0 + WINDOW_MS * 1000 {
        if !WIN_CLOSED.swap(true, AcqRel) {
            serial_println!(
                "[loginrender] window closed turns={} zero_pass_turns={} handler_max_ms={} zero_pass_max_ms={}",
                TURNS.load(Relaxed), ZP_TURNS.load(Relaxed), H_MAX_US.load(Relaxed) / 1000, ZP_MAX_US.load(Relaxed) / 1000
            );
            if cfg!(feature = "tests-at-boot") {
                ensure();
            }
        }
        return;
    }
    if ex_us < l0 || now_us <= ex_us {
        return;
    }
    let d = now_us - ex_us;
    TURNS.fetch_add(1, Relaxed);
    H_MAX_US.fetch_max(d, Relaxed);
    if LAST_PASS_US.load(Relaxed) < ex_us {
        ZP_TURNS.fetch_add(1, Relaxed);
        ZP_MAX_US.fetch_max(d, Relaxed);
    }
}

fn anchor_word(a: u32) -> &'static str {
    match a {
        1 => "login-ok",
        2 => "desktop",
        _ => "none",
    }
}

fn ensure() {
    if !REG.swap(true, AcqRel) {
        crate::tests::register("loginrender", selftest);
    }
}

fn name(id: u64) -> &'static str {
    WHAT_NAMES.get(id as usize).copied().unwrap_or("-")
}

/// The `[lag] stall` line's trailing token: ` handler_what=<name>:<ms>` when the stage is `render-handler`
/// (the longest stamp overlapping `since_ms..now`, the in-flight one charged its age; `-` when none), else nothing.
pub struct StallWhat(pub bool, pub u64);

impl core::fmt::Display for StallWhat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if !self.0 {
            return Ok(());
        }
        let now = crate::arch::ms();
        let mut best: (u64, u64) = (0, 0); // (ms, id)
        let c = CUR.load(Acquire);
        if c != 0 {
            best = (now.saturating_sub(c >> 8), c & 0xff);
        }
        for r in &RING {
            let w = r.load(Acquire);
            if w == 0 {
                continue;
            }
            let (start, dur, id) = (w >> 32, (w >> 8) & 0xff_ffff, w & 0xff);
            if start + dur + 1 >= (self.1 & 0xffff_ffff) && (dur > best.0 || best.1 == 0) {
                best = (dur, id);
            }
        }
        if best.1 == 0 {
            write!(f, " handler_what=-")
        } else {
            write!(f, " handler_what={}:{}", name(best.1), best.0)
        }
    }
}

/// `tests loginrender` (R80: only when asked) — reads the window the render task measured; runs nothing.
pub fn selftest() {
    if !ON {
        serial_println!(":: LOGINRENDER: -> SKIP :: reason=no-wc bound_ms={}", BOUND_MS);
        return;
    }
    let l0 = LOGIN_MS.load(Acquire);
    let mut mv = alloc::string::String::new();
    for id in MOVED_IDS {
        if !mv.is_empty() {
            mv.push(',');
        }
        mv.push_str(&alloc::format!("{}:{}", name(id as u64), MOVED[id as usize].load(Relaxed)));
    }
    if l0 == 0 {
        serial_println!(":: LOGINRENDER: -> SKIP :: reason=no-login bound_ms={} moved={}", BOUND_MS, mv);
        return;
    }
    let (h, zp) = (H_MAX_US.load(Relaxed) / 1000, ZP_MAX_US.load(Relaxed) / 1000);
    let ww = WIN_WHAT.load(Relaxed);
    let worst = if ww == 0 { alloc::string::String::from("-") } else { alloc::format!("{}:{}", name(ww & 0xff), (ww >> 8) / 1000) };
    let closed = WIN_CLOSED.load(Relaxed);
    serial_println!(
        ":: LOGINRENDER: handler_max_ms={} zero_pass_max_ms={} worst_what={} moved={} -> {} :: bound_ms={} anchor={} window={} window_s={} turns={} zero_pass_turns={}",
        h, zp, worst, mv, if zp <= BOUND_MS { "PASS" } else { "FAIL" }, BOUND_MS,
        anchor_word(ANCHOR.load(Relaxed)), if closed { "closed" } else { "open" },
        (crate::arch::ms().saturating_sub(l0) / 1000).min(WINDOW_MS / 1000),
        TURNS.load(Relaxed), ZP_TURNS.load(Relaxed)
    );
}
