//! CHARTER: Kernel — driver (B525 SESSIONWEDGE: the usb-pump's liveness stamp, its PUMPWEDGE watch off the pump, `tests sessionwedge`)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//!
//! SESSIONWEDGE (rmbp-ledger B525). Flight 29 boot 1 (`f29-boot1.log`, 14:35:19Z): the last bytes on the cable are
//! `[login] desktop bare: furniture=none (R88) why=sessio` — cut mid-word, then nothing. On the rMBP the only carrier
//! is the FTDI mirror, and the only drainer of that mirror is the `usb-pump` task (`main.rs` `usb_pump`, its xHCI step:
//! `service_ftdi` → `drain_ftdi`, one 64-byte bulk-OUT per transfer). A line cut mid-word is the pump's last packet:
//! the pump never came round again. Nothing printed that, because everything that could have said it rides the pump.
//!
//! This file:
//! 1. [`stamp`] — the pump's liveness: one fold on the loop's `pump_top` line stamps `ms` and a pass count every pass
//!    (and the pump's task id, so LOCKREG can say what it spins on).
//! 2. [`look`] — read by RENDERWEDGE's `render-watch` task (B520, a sibling core that is never the service core) every
//!    250 ms once the desktop phase begins: a pump whose stamp is older than [`WEDGE_MS`] prints, once per wedge,
//!    `:: PUMPWEDGE: age_ms=<n> last_step=<lag pump step> holder=<what it waits on> -> FAIL ::` through a path that
//!    does NOT need the pump — `serial_println!` drains the 16550 from the printing core where one exists (QEMU), and
//!    the same line is painted straight onto the panel (`video::WRITER`, SPLASHSTALL's `over_label` blit shape, B510)
//!    every second while the wedge lasts, because the rMBP has no 16550 (`uart16550=absent carrier=ftdi-mirror`).
//!    `last_step` is `lag::pump_step_now()` (HIDSTALL2's in-flight step), `holder` is LOCKREG's `waits_on(pump)` or,
//!    when the pump is in the SMC's bounded conversation-lock wait (`smc::txn_wait_holder`), that lock's holder.
//! 3. The ignition record: the stamps inside the 5 s after the session's ignition (`boot::ignite_ms`, set by
//!    `login::close_into_session` one statement before `desktop bare`) — the widest gap and the count.
//! 4. `tests sessionwedge` — `:: SESSIONWEDGE: bare_to_built_ms=<n> pump_gap_max_ms=<n> pump_passes=<n>
//!    through=<0|1> armed=<task:0|1,…> watch=<seen|silent> -> PASS|FAIL|SKIP ::`: PASS iff `[desktop] built` came within
//!    5 s of the ignition, the pump stamped through the whole 5 s window with no gap of [`WEDGE_MS`], and the watch's
//!    print path fired on a fixture-fed stale stamp (`-> SEEN :: fixture=1`, the same formatter, painted too), and
//!    the fixture's band was handed back to the compositor, not filled (`cleared=damage rects=<n>`, FBDIRECT B536).
//! 5. SETTERWEDGE (rmbp-ledger B553, flight 33 boot 1) — the file tail: the watch is ARMED AT THE IGNITION
//!    ([`arm`], from `boot::ignite`), not at the render task's first lag roll after it; a wedge prints a per-core
//!    census of what every core runs and spins on; and the cable is drained from the watch core while the pump is
//!    stopped (`xhci::ftdi_flush_sync`), so the wedge's own lines reach the bench. `tests setterwedge`.

use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

/// A pump stamp older than this is a wedge (the brief's 2 s; a healthy pass is ~1 ms, the FTDI slice 4 ms).
pub const WEDGE_MS: u64 = 2_000;
/// The window after the ignition the verb judges (`[desktop] built` within it, the pump stamping through it).
pub const IGNITION_MS: u64 = 5_000;
/// While a wedge lasts, the panel line is repainted this often (a live compositor may paint over it).
const PAINT_EVERY_MS: u64 = 1_000;

static LAST_MS: AtomicU64 = AtomicU64::new(0);
static PASSES: AtomicU64 = AtomicU64::new(0);
static PUMP_TID: AtomicU64 = AtomicU64::new(0);
static PUMP_CPU: AtomicU64 = AtomicU64::new(u64::MAX);
// The ignition window's record.
static IGN_AT: AtomicU64 = AtomicU64::new(0);
static IGN_GAP_MAX: AtomicU64 = AtomicU64::new(0);
static IGN_PASSES: AtomicU64 = AtomicU64::new(0);
static IGN_THROUGH: AtomicBool = AtomicBool::new(false);
// The watch.
static FIRED: AtomicBool = AtomicBool::new(false);
static WEDGES: AtomicU32 = AtomicU32::new(0);
static NEXT_PAINT: AtomicU64 = AtomicU64::new(0);
static WATCHED: AtomicBool = AtomicBool::new(false);
// The fixture's feed: a stale stamp the same detector reads (0 = none).
static FIX_LAST_MS: AtomicU64 = AtomicU64::new(0);
static FIX_FIRES: AtomicU32 = AtomicU32::new(0);
static FIX_AGE: AtomicU64 = AtomicU64::new(0);

/// The `usb-pump` loop's top, every pass (`main.rs`, the `pump_top` line). Atomics only.
#[inline]
pub fn stamp() {
    let now = crate::arch::ms();
    let prev = LAST_MS.swap(now.max(1), Relaxed);
    PASSES.fetch_add(1, Relaxed);
    if PUMP_TID.load(Relaxed) == 0 {
        if let Some(t) = crate::arch::sched::current_id() {
            PUMP_TID.store(t, Relaxed);
        }
    }
    PUMP_CPU.store(crate::arch::percpu::this_cpu().cpu_index as u64, Relaxed); // GATESTEAL (B657): every pass, not once — the re-home MOVES the pump, and `pump=<tid>@cpu<n>` must read the core it stamps from
    let ig = crate::boot::ignite_ms();
    if ig == 0 || now < ig {
        return;
    }
    if IGN_AT.load(Relaxed) != ig {
        // A new ignition (the first login, or a relogin): a fresh window.
        IGN_AT.store(ig, Relaxed);
        IGN_GAP_MAX.store(now.saturating_sub(prev.max(ig)), Relaxed);
        IGN_PASSES.store(0, Relaxed);
        IGN_THROUGH.store(false, Relaxed);
    }
    if now <= ig + IGNITION_MS {
        IGN_PASSES.fetch_add(1, Relaxed);
        IGN_GAP_MAX.fetch_max(now.saturating_sub(prev.max(ig)), Relaxed);
    } else if !IGN_THROUGH.load(Relaxed) {
        // The first stamp past the window: its gap still counts against the window (a pump dead at ig+4.9 s).
        IGN_GAP_MAX.fetch_max((ig + IGNITION_MS).saturating_sub(prev.max(ig)), Relaxed);
        IGN_THROUGH.store(true, Relaxed);
    }
}

/// What the pump waits on, for the line: LOCKREG's `waits_on(pump)`, else the SMC conversation lock's holder while the
/// pump is in its bounded wait, else `-`.
fn holder(out: &mut Line) {
    use core::fmt::Write;
    let tid = PUMP_TID.load(Relaxed);
    if tid != 0 {
        if let Some((site, ht, hc)) = crate::sync::waits_on(tid as u32) {
            let _ = write!(out, "{}:{}@cpu{}", site, ht, hc);
            return;
        }
        #[cfg(feature = "smc")]
        if let Some((ht, hc)) = crate::drivers::smc::txn_wait_holder(tid) {
            let _ = write!(out, "smc-txn:{}@cpu{}", ht, hc);
            return;
        }
    }
    let _ = out.write_str("-");
}

/// One formatter for the live wedge and the fixture, on the wire AND the panel.
fn say(age_ms: u64, fixture: bool) {
    use core::fmt::Write;
    let step = crate::video::lag::qs_step_name(crate::video::lag::pump_step_now());
    let mut h = Line::new();
    holder(&mut h);
    let mut rh = Line::new();
    rehome_word(&mut rh); // GATESTEAL (B657): `rehome=<ok(pump=<tid>@cpu<n>)|none>` — the loop's other half, on the line that names the wedge
    serial_println!(
        ":: PUMPWEDGE: age_ms={} last_step={} holder={} rehome={} -> {} :: pump={}@cpu{} passes={} wedges={} bound_ms={}{}",
        age_ms, step, h.as_str(), rh.as_str(), if fixture { "SEEN" } else { "FAIL" },
        PUMP_TID.load(Relaxed), PUMP_CPU.load(Relaxed) as i64, PASSES.load(Relaxed), WEDGES.load(Relaxed), WEDGE_MS,
        if fixture { " fixture=1" } else { "" }
    );
    let mut ln = Line::new();
    let _ = write!(ln, "PUMPWEDGE: the usb pump has not run for {}.{} s, last step {}, waiting on {}{}", age_ms / 1000, (age_ms % 1000) / 100, step, h.as_str(), if fixture { " (fixture)" } else { "" });
    paint(ln.as_str());
}

/// One look (`render-watch`, every 250 ms in the desktop phase). Never waits.
pub fn look(now: u64) {
    WATCHED.store(true, Relaxed);
    gatesteal_rehome_maybe(now); // GATESTEAL (B657): the steal's dead core, when it is the pump's own, is re-homed from HERE — the one lane the pump's death cannot kill
    let last = LAST_MS.load(Relaxed);
    if last != 0 && now.saturating_sub(last) >= WEDGE_MS {
        let age = now.saturating_sub(last);
        if !FIRED.swap(true, AcqRel) {
            WEDGES.fetch_add(1, Relaxed);
            NEXT_PAINT.store(now + PAINT_EVERY_MS, Relaxed);
            say(age, GATESTEAL_ACTIVE.load(Acquire)); // GATESTEAL (B657): a wedge the fixture's forced long step makes reads `-> SEEN :: fixture=1`, never a live FAIL
            let t = PUMP_TID.load(Relaxed);
            if t != 0 {
                crate::sync::name_task(t, "PUMPWEDGE: the usb-pump, stopped"); // LOCKREG: its holds and the lock it spins on
            }
            census("wedge"); // SETTERWEDGE (B553): every core's running task and the lock it spins on, once per wedge
            gatesteal_probe(PUMP_CPU.load(Relaxed) as usize); // GATESTEAL (B657): a second NMI sample of the pump's core at the wedge — is it still on the instruction the steal found it on, and masked?
        } else if now >= NEXT_PAINT.load(Relaxed) {
            NEXT_PAINT.store(now + PAINT_EVERY_MS, Relaxed);
            let mut ln = Line::new();
            use core::fmt::Write;
            let _ = write!(ln, "PUMPWEDGE: the usb pump has not run for {} s, last step {}", age / 1000, crate::video::lag::qs_step_name(crate::video::lag::pump_step_now()));
            paint(ln.as_str());
        }
    } else if last != 0 && FIRED.swap(false, AcqRel) {
        serial_println!("[pump] wedge cleared (the usb-pump stamped again) wedges={}", WEDGES.load(Relaxed));
        paint("");
    }
    let f = FIX_LAST_MS.load(Acquire);
    if f != 0 && now.saturating_sub(f) >= WEDGE_MS && FIX_LAST_MS.compare_exchange(f, 0, AcqRel, Relaxed).is_ok() {
        FIX_AGE.store(now.saturating_sub(f), Relaxed);
        say(now.saturating_sub(f), true);
        FIX_CENSUS.store(census("fixture") as u32, Release); // SETTERWEDGE (B553): the census path, proved on the same fixture feed
        FIX_FIRES.fetch_add(1, Release);
    }
}

/// The panel line, straight onto the framebuffer — no compositor, no pump, no heap. A busy `WRITER` skips this paint
/// (the next one tries again). Bottom band, two chrome cells high (SPLASHSTALL's). Empty `text` clears the band —
/// FBDIRECT (B536, flight 31 REDBAND): the clear is a DAMAGE, never a fill. The old `""` filled the rect with the band
/// colour again and told the compositor nothing, so the band outlived the wedge until the cursor and the app bar
/// overpainted it piecemeal; now each paint registers its rect (`video::wm::fbdirect::note`) and the clear hands it
/// back (`fbdirect::clear`: `wm::damage_intersecting` + `screen::request_present_rect`, after the pump moved again).
fn paint(text: &str) {
    if text.is_empty() {
        crate::video::wm::fbdirect::clear(crate::video::wm::fbdirect::PUMP_WEDGE);
        return;
    }
    let Some(fbg) = crate::video::WRITER.try_lock() else { return };
    let fb = *fbg;
    drop(fbg);
    if !fb.is_ready() {
        return;
    }
    let i = fb.info();
    let ch = crate::video::font::Face::Chrome.cell_h();
    let band_h = ch * 2;
    if i.width == 0 || i.height < band_h * 4 {
        return;
    }
    let y0 = i.height - band_h * 3;
    fb.fill_rect(0, y0, i.width, band_h, BAND);
    let bytes = text.as_bytes();
    let face = crate::video::text::Face::Chrome;
    let n = crate::video::text::fit(bytes, false, face, i.width);
    let x0 = (i.width - crate::video::text::advance(&bytes[..n], false, face).min(i.width)) / 2;
    let ty = (y0 + (band_h - ch) / 2) as isize;
    let (w, h) = (i.width, i.height);
    let _ = crate::video::text::draw_with(&bytes[..n], false, face, x0, ty, w - x0, INK, &mut |px, py, a| {
        if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h && a >= 96 {
            fb.put_pixel(px as usize, py as usize, INK);
        }
    });
    fb.flush_rect(0, y0, i.width, band_h);
    crate::video::wm::fbdirect::note(crate::video::wm::fbdirect::PUMP_WEDGE, 0, y0, i.width, band_h); // FBDIRECT (B536): on the glass, owed to the compositor until the clear
}

const BAND: u32 = crate::video::theme::WEDGE_BAND;
const INK: u32 = crate::video::theme::WEDGE_INK;

/// A bounded, allocation-free line (bootstep's `PanelLine` shape): what does not fit is dropped.
struct Line {
    b: [u8; 200],
    n: usize,
}
impl Line {
    const fn new() -> Self {
        Line { b: [0; 200], n: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("-")
    }
}
impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let take = core::cmp::min(self.b.len() - self.n, s.len());
        self.b[self.n..self.n + take].copy_from_slice(&s.as_bytes()[..take]);
        self.n += take;
        Ok(())
    }
}

// ── `tests sessionwedge` ────────────────────────────────────────────────────────────────────────────────────────

/// Register `tests sessionwedge` once (from `renderwedge::ensure_tests`; never at boot, R80).
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, AcqRel) {
        crate::tests::register("sessionwedge", selftest);
        setter_tests(); // SETTERWEDGE (B553)
    }
}

/// The tasks this fold put on or beside the usb-pump at desktop ignition, armed or not on this boot.
fn armed(out: &mut Line) {
    use core::fmt::Write;
    let _ = write!(out, "xhci-poll:{}", crate::drivers::xhci::poll_task_armed() as u8);
    #[cfg(feature = "smc")]
    let _ = write!(out, ",smc-batt:{}", crate::drivers::smc::battery_task_armed() as u8);
    #[cfg(feature = "unafs")]
    let _ = write!(out, ",bootlog-flush:{}", crate::fs::bootlog::flusher_armed() as u8);
    #[cfg(feature = "facet")]
    let _ = write!(out, ",wall-probe:{}", crate::video::wallpaper::probe_tries());
}

fn wait_for(bound_ms: u64, mut f: impl FnMut() -> bool) -> bool {
    let t0 = crate::arch::ms();
    loop {
        if f() {
            return true;
        }
        if crate::arch::ms().saturating_sub(t0) >= bound_ms {
            return false;
        }
        crate::arch::sched::sleep_ms(10);
    }
}

/// `:: SESSIONWEDGE: … -> PASS|FAIL|SKIP ::` — see the module doc.
fn selftest() {
    let ig = crate::boot::ignite_ms();
    let mut a = Line::new();
    armed(&mut a);
    if ig == 0 {
        serial_println!(":: SESSIONWEDGE: bare_to_built_ms=- pump_gap_max_ms=- pump_passes=- through=0 armed={} watch=- -> SKIP :: reason=no-session-ignition", a.as_str());
        return;
    }
    // The ignition window must be over before it can be judged.
    let _ = wait_for(IGNITION_MS + WEDGE_MS, || IGN_THROUGH.load(Relaxed) || crate::arch::ms() > ig + IGNITION_MS + WEDGE_MS);
    let built = crate::video::desktopbuild::last_bar_ms();
    let gap = IGN_GAP_MAX.load(Relaxed);
    let passes = IGN_PASSES.load(Relaxed);
    let through = IGN_THROUGH.load(Relaxed) && IGN_AT.load(Relaxed) == ig;
    // The watch's print path, proved on a fixture-fed stale stamp (the live watch task must be the one that prints).
    let f0 = FIX_FIRES.load(Acquire);
    FIX_LAST_MS.store(crate::arch::ms().saturating_sub(WEDGE_MS).max(1), Release);
    let seen = wait_for(WEDGE_MS + 2_000, || FIX_FIRES.load(Acquire) != f0);
    FIX_LAST_MS.store(0, Release);
    if seen && !FIRED.load(Acquire) {
        paint(""); // the fixture's band goes — handed back to the compositor (FBDIRECT B536), not filled
    }
    // FBDIRECT (B536): no PUMPWEDGE band left on the glass that the compositor was not handed (a live wedge keeps its own).
    let cleared = FIRED.load(Acquire) || !crate::video::wm::fbdirect::owed(crate::video::wm::fbdirect::PUMP_WEDGE);
    let ok = matches!(built, Some(ms) if ms <= IGNITION_MS) && through && gap < WEDGE_MS && passes > 0 && seen && cleared;
    serial_println!(
        ":: SESSIONWEDGE: bare_to_built_ms={} pump_gap_max_ms={} pump_passes={} through={} armed={} watch={}:{} cleared={} rects={} -> {} :: window_ms={} wedge_ms={} wedges={} watched={}",
        match built { Some(ms) => alloc::format!("{}", ms), None => alloc::string::String::from("never") },
        gap, passes, through as u8, a.as_str(), if seen { "seen" } else { "silent" }, FIX_AGE.load(Relaxed),
        if cleared { "damage" } else { "owed" }, crate::video::wm::fbdirect::handed(),
        if ok { "PASS" } else { "FAIL" }, IGNITION_MS, WEDGE_MS, WEDGES.load(Relaxed), WATCHED.load(Relaxed) as u8
    );
}

// ── SETTERWEDGE (rmbp-ledger B553) — the watch armed AT the ignition, the core census, the cable rescue ──────────────
//
// Flight 33 boot 1 (image 25, a fresh card: the first-user path): the cable's last bytes are `[boot] phase=desktop
// from=setter why=session at=35088ms pre_wi`, cut mid-word at 05:57:25, then nothing for 37 s; Peter: "machine not
// responding". Boot 2 (the login-screen path, same image) ignites. What the wire and the code settle, and what they do not:
//
// * The print is not the hang. `serial_println!` formats the WHOLE line into a stack buffer first
//   (`serial_line::emit_src`, serial_line.rs:110-121) and only then hands it to the taps; the FTDI tap copies the whole
//   line under a `try_lock` (arch/x86_64/serial.rs:235, `ftdi::mirror`). `pre_windows`/`pre_starts`/`pre_valve` are
//   three relaxed atomic loads (boot.rs:161-164). So the line was whole in the mirror ring; `pre_wi` is where the
//   usb-pump's last 4 ms drain slice ended (xhci/mod.rs `drain_ftdi`, `FTDI_DRAIN_SLICE_MS`) — the pump never came
//   round again (SESSIONWEDGE's reading of flight 29, the same shape).
// * Nothing could say so: RENDERWEDGE's `render-watch`, the only caller of [`look`], is spawned LAZILY from the render
//   task's lag roll on the first roll that reads the Desktop phase (video/renderwedge.rs:336-337 `watch_start`).
//   Boot 2 and flight 32 boot 1 print `[render] watch armed` in the ignition second; flight 33 boot 1 never does —
//   the wedge came before that roll, so PUMPWEDGE did not exist on the one boot it was built for.
// * The setter path differs from the login-screen path at that instant in exactly these ways (video/login.rs
//   `apply_create` vs `apply_user`): (1) the session opens while the PUBLISHED stage is still Installer
//   (users.rs `STAGE`), so `users::furniture_held()` is true and the pump's `desktopbuild::service` holds the build
//   until (2) `users::installer_desktop_ignite` runs `stage_resolve("user-created")` → `stage_publish` on the RENDER
//   task after `close_into_session` (the stage swap, `stage_witness`, `desktopbuild::build("stage")`,
//   `splash::hold_release`, `firstuser_witness`); (3) the home, the Holocron ring and the users row were written in
//   the preceding 4 s (`[login] submit kdf_ms=2479 adduser_ms=1549`, `[holocron] ring=created`) and HOLOCRON.ELF was
//   first placed on the render core (`SCHEDPLACE-X86: 'bg-user' -> c1`, boot 2: `-> c0`); (4) the ignition is at
//   35 s, not 24 s. Flight 32 (image 24) took the same setter path and ignited, so the cause rides image 25's delta
//   (POINTERSCALE, PUMPHOLE, TWOCURSORS, AETHERPTR, SPLASHWORD2, USBNETRX, DOCKPINS) crossed with one of (1)-(4).
//   The read did not single one out: every image-25 site this arc could reach either is lock-free (fbdirect's table,
//   `strips_paint`, the flight recorder's hand, POINTERSCALE's atomics) or ran on boot 2 too without wedging
//   (TWOCURSORS `ghost_restore` declined 36 times at 06:02:52). The cause is therefore NOT named here; this block makes
//   the next fresh-card boot name it on the cable.
//
// The witness:
// * [`arm`] — `boot::ignite` spawns `pump-watch` (a sibling core: never the igniting render core, never the service
//   core while another is online) at the ignition itself; it calls [`look`] every 250 ms, alongside render-watch.
// * [`census`] — at a wedge (and on the fixture feed): `[pump] census why=<w> cores=<n> c0=<tid>[:waits=<site>:<tid>@cpu<n>]
//   … pump=<tid>@cpu<n>` — what each core is running (`sched::current_task_id`, the published word, no deref) and the
//   lock each spins on (LOCKREG `sync::waits_on`). No heap: a wedged core may hold the allocator.
// * [`rescue`] — while the wedge lasts, once a second, `xhci::ftdi_flush_sync` from the watch core: the WEDGE-8 loan
//   claimed (never locked), bounded at 50 ms, so the lines the pump never sent — and the PUMPWEDGE/census lines —
//   reach the bench. A pump wedged INSIDE the loan reads `outcome=busy` (that is a finding too: the wedge is in an
//   xHCI step).
// * `tests setterwedge` — `:: SETTERWEDGE: from=<setter|login-screen> watch=<armed|late|none> armed_ms=<n>
//   watch_cpu=<n> running=<0|1> replay=<ok|wedged|skip> replay_ms=<n> census=<cores> cable=<drained|empty|busy|no-cable>
//   -> PASS|FAIL|SKIP ::`. The replay drives the setter-only tail (`users::stage_resolve("user-created")`, idempotent
//   on a Desktop stage) on a sibling core under a 3 s bound. PASS iff the watch was armed at the ignition and is
//   running, the replay returned (or the build has no `login`), and the census named at least one core.

/// When [`arm`] spawned `pump-watch` (`arch::ms`, 0 = never), its core, and the igniting core.
static ARMED_AT: AtomicU64 = AtomicU64::new(0);
static ARMED_CPU: AtomicU64 = AtomicU64::new(u64::MAX);
static ARMED_FROM: AtomicU64 = AtomicU64::new(u64::MAX);
/// `pump-watch` ran its first look.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// Cores the fixture-fed census named.
static FIX_CENSUS: AtomicU32 = AtomicU32::new(0);
/// Cable rescues attempted, bytes they moved.
static RESCUES: AtomicU32 = AtomicU32::new(0);
static RESCUE_BYTES: AtomicU64 = AtomicU64::new(0);
/// The setter-tail replay (`tests setterwedge`).
static REPLAY_DONE: AtomicBool = AtomicBool::new(false);
static REPLAY_TID: AtomicU64 = AtomicU64::new(0);
static REPLAY_MS: AtomicU64 = AtomicU64::new(0);

/// `pump-watch`'s cadence (render-watch's).
const LOOK_MS: u64 = 250;
/// While a wedge lasts, the cable is rescued this often.
const RESCUE_EVERY_MS: u64 = 1_000;
/// The replay's bound.
const REPLAY_BOUND_MS: u64 = 3_000;

/// `boot::ignite` (every ignition; the first one spawns): the pump watch exists from the ignition on.
pub fn arm() {
    static ONCE: AtomicBool = AtomicBool::new(false);
    if ONCE.swap(true, AcqRel) {
        return;
    }
    let me = crate::arch::percpu::this_cpu().cpu_index as usize;
    let cpu = crate::arch::sched::sibling_online_cpu(me);
    ARMED_CPU.store(cpu as u64, Relaxed);
    ARMED_FROM.store(me as u64, Relaxed);
    ARMED_AT.store(crate::arch::ms().max(1), Release);
    setter_tests();
    crate::arch::sched::spawn("pump-watch", watch_task, 0, cpu, crate::arch::sched::PRIO_HIGH);
}

fn watch_task(_arg: usize) {
    RUNNING.store(true, Release);
    serial_println!(
        "[pump] watch running cpu={} from_cpu={} armed_ms={} wedge_ms={} (SETTERWEDGE B553: armed at the ignition, not at the first lag roll)",
        ARMED_CPU.load(Relaxed), ARMED_FROM.load(Relaxed), ARMED_AT.load(Relaxed), WEDGE_MS
    );
    let mut next_rescue = 0u64;
    loop {
        crate::arch::sched::sleep_ms(LOOK_MS);
        let now = crate::arch::ms();
        look(now);
        if FIRED.load(Acquire) && now >= next_rescue {
            next_rescue = now + RESCUE_EVERY_MS;
            let (w, b) = rescue();
            let n = RESCUES.load(Relaxed);
            if n == 1 || n % 10 == 0 {
                serial_println!("[pump] cable rescue n={} bytes={} total_bytes={} outcome={} (SETTERWEDGE B553: the watch drains the FTDI while the pump is stopped)", n, b, RESCUE_BYTES.load(Relaxed), w);
            }
        }
    }
}

/// One bounded drain of the FTDI mirror from THIS core (`xhci::ftdi_flush_sync`: claimed, never locked; 50 ms).
fn rescue() -> (&'static str, u64) {
    if !crate::drivers::xhci::ftdi::is_live() {
        return ("no-cable", 0);
    }
    let hz = crate::arch::apic::tsc_hz().max(1_000);
    let (bytes, _transfers, exhausted) = crate::drivers::xhci::ftdi_flush_sync(hz / 20);
    RESCUES.fetch_add(1, Relaxed);
    RESCUE_BYTES.fetch_add(bytes, Relaxed);
    (if bytes > 0 { "drained" } else if exhausted { "busy" } else { "empty" }, bytes)
}

/// A wider bounded line for the census (eight cores and their waits).
struct Wide {
    b: [u8; 480],
    n: usize,
}
impl Wide {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("-")
    }
}
impl core::fmt::Write for Wide {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let take = core::cmp::min(self.b.len() - self.n, s.len());
        self.b[self.n..self.n + take].copy_from_slice(&s.as_bytes()[..take]);
        self.n += take;
        Ok(())
    }
}

/// `[pump] census` — every core's running task and the lock it spins on. Returns the cores with a task.
fn census(why: &str) -> usize {
    use core::fmt::Write;
    let mut w = Wide { b: [0; 480], n: 0 };
    let mut n = 0usize;
    for c in 0..crate::arch::gdt::MAX_CPUS {
        match crate::arch::sched::current_task_id(c) {
            Some(t) => {
                n += 1;
                let _ = write!(w, " c{}={}", c, t);
                if let Some((site, ht, hc)) = crate::sync::waits_on(t as u32) {
                    let _ = write!(w, ":waits={}:{}@cpu{}", site, ht, hc);
                }
            }
            None => {
                let _ = write!(w, " c{}=-", c);
            }
        }
    }
    serial_println!(
        "[pump] census why={} cores={}{} pump={}@cpu{} (SETTERWEDGE B553: what each core runs, and the lock it spins on)",
        why, n, w.as_str(), PUMP_TID.load(Relaxed), PUMP_CPU.load(Relaxed) as i64
    );
    n
}

/// Register `tests setterwedge` once (from [`arm`] and [`ensure_tests`]; never run at boot, R80).
fn setter_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, AcqRel) {
        crate::tests::register("setterwedge", setter_selftest);
        crate::tests::register("gatesteal", gatesteal_selftest); // GATESTEAL (B657)
    }
}

/// The setter-only tail, on a sibling core: `users::stage_resolve("user-created")` — what `installer_desktop_ignite`
/// runs after the create-user form's `close_into_session` (idempotent on a Desktop stage: the publish finds the stage
/// unchanged and returns; the ignition is latched).
fn replay_task(_arg: usize) {
    REPLAY_TID.store(crate::arch::sched::current_id().unwrap_or(0), Relaxed);
    let t0 = crate::arch::ms();
    #[cfg(feature = "login")]
    crate::fs::users::stage_resolve("user-created");
    REPLAY_MS.store(crate::arch::ms().saturating_sub(t0), Relaxed);
    REPLAY_DONE.store(true, Release);
}

/// `:: SETTERWEDGE: … -> PASS|FAIL|SKIP ::` — see this block's header.
fn setter_selftest() {
    let ig = crate::boot::ignite_ms();
    let from = crate::boot::ignite_from();
    let armed = ARMED_AT.load(Acquire);
    if ig == 0 {
        serial_println!(":: SETTERWEDGE: from={} watch={} armed_ms={} -> SKIP :: reason=no-session-ignition", from, if armed != 0 { "armed" } else { "none" }, armed);
        return;
    }
    // `arm` runs inside `boot::ignite` one statement before the ignition is stamped: armed at the ignition = at or
    // before it (a 50 ms allowance for a preempted ignite); later than that = the old lazy arming came back.
    let watch = if armed == 0 { "none" } else if armed <= ig.saturating_add(50) { "armed" } else { "late" };
    let running = RUNNING.load(Acquire);
    #[cfg(feature = "login")]
    let can = crate::boot::desktop() && crate::fs::users::boot_stage() == crate::fs::users::BootStage::Desktop;
    #[cfg(not(feature = "login"))]
    let can = false;
    let replay = if !can {
        "skip"
    } else {
        REPLAY_DONE.store(false, Release);
        REPLAY_TID.store(0, Relaxed);
        let me = crate::arch::percpu::this_cpu().cpu_index as usize;
        crate::arch::sched::spawn("setterwedge-replay", replay_task, 0, crate::arch::sched::sibling_online_cpu(me), crate::arch::sched::PRIO_NORMAL);
        if wait_for(REPLAY_BOUND_MS, || REPLAY_DONE.load(Acquire)) {
            "ok"
        } else {
            crate::sync::name_task(REPLAY_TID.load(Relaxed), "SETTERWEDGE: the setter tail did not return"); // LOCKREG: its holds and its wait
            "wedged"
        }
    };
    let cores = census("tests");
    let (cable, _) = rescue();
    let replay_ok = replay == "ok" || (replay == "skip" && !cfg!(feature = "login"));
    let ok = watch == "armed" && running && replay_ok && cores > 0;
    serial_println!(
        ":: SETTERWEDGE: from={} watch={} armed_ms={} watch_cpu={} running={} replay={} replay_ms={} census={} cable={} -> {} :: ignite_ms={} fixture_census={} rescues={} rescue_bytes={} wedges={}",
        from, watch, armed, ARMED_CPU.load(Relaxed) as i64, running as u8, replay, REPLAY_MS.load(Relaxed), cores, cable,
        if ok { "PASS" } else { "FAIL" }, ig, FIX_CENSUS.load(Relaxed), RESCUES.load(Relaxed), RESCUE_BYTES.load(Relaxed), WEDGES.load(Relaxed)
    );
}

// ── GATESTEAL (rmbp-ledger B657, flight 35 boots 1 and 3) — the pump's core dies on every W5 steal ─────────────────
//
// THE WIRE (f35-boot1.log 21:32:42, f35-boot3.log 22:04:01, the same shape byte for byte): `:: W5: site=post-steal
// from=c1 dead=c7 held=1671 ::`, `:: W5: nmi core=c7 taken=y rip=0x798b5ac6 in_blit=n cs=0x8 ::`, `:: [wcser] GATE
// STOLEN from c7 by c1 after 1671ms — the holder was in phase 4 row 1238 and had not moved … c7 is DEAD and its
// singleton roles are owed a re-home ::`, then `:: PUMPWEDGE: age_ms=2003 last_step=bootlog holder=- -> FAIL ::
// pump=9@cpu7`, `[lock] task=9 holds=[] waits=none`, `[pump] census … c7=-`, and the cable rescue once a second to
// power-off: the pump task on c7 never stamps again. The full read is docs/dev/evidence/gatesteal/GATESTEAL.md §2.
//
// WHAT THE RECORDS SETTLE. (1) The re-home's only consumer ran on the corpse: `wm::comp_dead_core_take` is called
// from `render_rehome_service` (main.rs), which is called from `usb_pump` — on the pump, on the SERVICE core, "the
// core the steal has never killed". Flight 35 killed it twice (`dead=c7 svc=7`), so the death was published to the
// one task that could not read it. That is this block's job: the pump WATCH runs on a sibling core and never on the
// pump, so it is the lane that reads a death of the pump's core. (2) "Had not moved" is a stale breadcrumb: the
// `phase`/`row` gauges are stamped inside the window draw only; everything a holder runs after its window loop (the
// strip, the cursor tail, the WC-D read-back inside the phase-4 witness bracket, the re-run gates) stamps nothing.
// The NMI probe is the instrument that decides: `taken=y` (not hardware-parked), `cs=0x8` (kernel text), and the
// SAME text offset in both boots (rip − memcpy_lo = −0x99F13A on boot 1 and on boot 3: the image moved by 0x1A0000
// between them and both deltas agree). One instruction, 1.5 s into the hold, in two boots whose steps differ: a loop,
// untracked (`holds=[] waits=none`), that the pump was in BEFORE the steal; the steal then did nothing to c7 but
// that one NMI (interrupts.rs `nmi_handler`: straight-line, `iretq` back to the same rip). (3) Where the pump's steps
// take the gate: the `bootlog` step is `bootpace::service_dump`, whose paygo taker (`wm::paygo_service`, "a 1.6 s
// read-back") composites on the device core as an UNMASKED holder — re-run rounds and all; the `prtscr` step takes
// no compositor gate at all (`prtscr.rs`: `panel_snapshot`'s handle, no `composite`/`COMP_GATE`), so boot 3's c7
// holder was another c7 lane's present (the console/serial-door route, reached from a print with interrupts off, or
// a staged `paint_window` at the caption phase that `phase 46` names); `[lag] … holder=usb-pump:prtscr hold_ms=96
// masked_ms=400` says the pump held IRQs masked 400 ms of that second, and a MASKED holder on the device core
// starves every task pinned there — `xhci-poll`, `ehci-hid`, the pump's own tick — which is "the glass dies".
// NOT settled: the function at `nm(memcpy) − 0x99F13A` and whether the holder spins masked; the W5 line now carries
// `if=<0|1>` and [`gatesteal_probe`] takes a second sample at the wedge, so the next flight answers both.
//
// THE FIX. (a) The pump's lane never holds the gate long: the paygo taker runs on `paygo-take` on a sibling core
// ([`paygo_off_pump`]); in `wm::composite` the SERVICE core is a ONE-PASS holder (no re-run, no lost-wakeup
// re-acquire) and a MASKED composite on the service core DECLINES (publishes COMP_PENDING for the render lane).
// (b) When W5 does steal the pump's core, the roles ARE re-homed, by the scheduler, from this watch:
// [`gatesteal_rehome_maybe`] → `sched::rehome_dead_core(dead, live)` moves every task on the dead core's run queue
// and sleeper list the way `try_steal` moves one (the pin is void on a dead core), the SERVICE role follows the pump
// (`smp::publish_sched_split(render, live)`: the XHCI_CONTROLLER deadlock rule and `try_steal`'s exclusion follow
// it), the dead core is PARKED BY NAME (`sched::note_dead_core`: no placement lands on it again). The one task the
// scheduler cannot move is the one CURRENT on the dead core; the line says so, and the watch retries while that is
// the pump (a pump asleep between passes is caught in SLEEPERS on the next look). (c) `tests gatesteal` below.
// (d) PUMPWEDGE carries `rehome=`.
//
// `tests gatesteal`: `:: GATESTEAL: long_step=<held|none> steal=<seen|none> rehome=<ok(pump=<tid>@cpu<live>)|none>
// passes_after=<n> input=<alive|stale> -> PASS|FAIL|SKIP :: hold_ms= gen= dead=c<n> live=c<n> drive=c<n> tries=
// svc_masked_declined= hold=<stolen|kept|untaken>`. The pump's `bootlog` step takes the gate exactly as `composite`
// does and holds it SLEEPING for `GATESTEAL_HOLD_MS` (`wm::comp_gate_hold_fixture`, armed by [`gatesteal_long_step_maybe`]
// from `bootpace::service_dump` — the step flight 35 boot 1 died in); a `gatesteal-drive` task on a sibling core calls
// `wm::composite()` and [`look`] every 100 ms (WEDGEINJ's lesson: the steal is a branch inside `composite`, not a
// timer, and on an idle QEMU desktop nothing asks). PASS iff the hold began, the steal generation rose, the pump
// STAMPED from a core that is not the dead one, and it stamped again in the second after (input and the serial door
// ride that cadence; `input=alive` also requires the `xhci-poll` task). Go-red by mutation: [`GATESTEAL_REHOME`] =
// false → `rehome=none` → FAIL. A wedge the hold makes prints `-> SEEN :: fixture=1` while the fixture is active.

/// GATESTEAL — the re-home, enabled. The go-red mutation: `false` leaves the dead core's roles where the steal found
/// them and `tests gatesteal` prints `rehome=none -> FAIL`.
const GATESTEAL_REHOME: bool = true;
/// GATESTEAL — the fixture's forced long step: 3000 ms, twice the steal bound (1500 + the 250 ms rung grace) with
/// room for the watch's look and the scheduler's move before the hold ends.
const GATESTEAL_HOLD_MS: u64 = 3_000;
/// GATESTEAL — looks the watch spends retrying while the pump is the task CURRENT on the dead core.
const GATESTEAL_REHOME_TRIES: u32 = 8;

static PAYGO_TASK: AtomicBool = AtomicBool::new(false);
static GATESTEAL_ACTIVE: AtomicBool = AtomicBool::new(false);
static HOLD_ARMED: AtomicBool = AtomicBool::new(false);
/// 0 = not held, 1 = holding, 2 = held and stolen, 3 = could not take the gate, 4 = held, never stolen.
static HOLD_STATE: AtomicU32 = AtomicU32::new(0);
static DRIVE_STOP: AtomicBool = AtomicBool::new(false);
/// The steal generation the watch has finished with (acted, declined, or not the pump's core).
static REHOME_GEN_DONE: AtomicU64 = AtomicU64::new(0);
static REHOME_TRY_GEN: AtomicU64 = AtomicU64::new(0);
static REHOME_TRIES: AtomicU32 = AtomicU32::new(0);
static REHOME_DEAD: AtomicU64 = AtomicU64::new(u64::MAX);
static REHOME_LIVE: AtomicU64 = AtomicU64::new(u64::MAX);
static REHOME_AT: AtomicU64 = AtomicU64::new(0);

/// GATESTEAL — the PAYGO taker (`wm::paygo_service`, bootpace.rs's bootlog step) runs on a `paygo-take` task on a
/// sibling core, never on the pump. Spawned once, from a scheduled task (the pump); the BSP's inline loop and a leg
/// with no dispatching sibling keep the inline take exactly as before.
pub fn paygo_off_pump() {
    if PAYGO_TASK.load(Acquire) {
        return;
    }
    if crate::arch::sched::current_id().is_none() {
        crate::video::wm::paygo_service();
        return;
    }
    let me = crate::arch::percpu::this_cpu().cpu_index as usize;
    let cpu = crate::arch::sched::sibling_online_cpu(me);
    if cpu == me {
        crate::video::wm::paygo_service();
        return;
    }
    if PAYGO_TASK.swap(true, AcqRel) {
        return;
    }
    crate::arch::sched::spawn("paygo-take", paygo_task, 0, cpu, crate::arch::sched::PRIO_LOW);
    serial_println!("[pump] paygo-take task on c{} from c{} (GATESTEAL B657: the PAYGO read-back never holds the gate on the pump's core)", cpu, me);
}

fn paygo_task(_arg: usize) {
    loop {
        crate::arch::sched::sleep_ms(250);
        crate::video::wm::paygo_service();
    }
}

/// GATESTEAL — the fixture's forced long step, on the pump, in its `bootlog` step: one relaxed load per pass.
pub fn gatesteal_long_step_maybe() {
    if !HOLD_ARMED.load(Relaxed) {
        return;
    }
    let tid = PUMP_TID.load(Relaxed);
    if tid == 0 || crate::arch::sched::current_id() != Some(tid) {
        return;
    }
    if !HOLD_ARMED.swap(false, AcqRel) {
        return;
    }
    HOLD_STATE.store(1, Release);
    let (held, stolen) = crate::video::wm::comp_gate_hold_fixture(GATESTEAL_HOLD_MS);
    HOLD_STATE.store(if !held { 3 } else if stolen { 2 } else { 4 }, Release);
}

/// GATESTEAL — a live core for the pump: the placement authority's xHCI worker (never the render core, never the
/// service/dead core, never a core parked by name), else the dead core's least-loaded sibling under the same rules.
fn pick_live(dead: usize, render: Option<usize>) -> Option<usize> {
    let n = crate::arch::smp::online_aps().len();
    for i in 0..n {
        match crate::arch::smp::xhci_worker_cpu(i) {
            Some(c) if c != dead && Some(c) != render && !crate::arch::sched::is_dead_core(c) => return Some(c),
            Some(_) => continue,
            None => break,
        }
    }
    let c = crate::arch::sched::sibling_online_cpu(dead);
    if c != dead && Some(c) != render && !crate::arch::sched::is_dead_core(c) { Some(c) } else { None }
}

/// GATESTEAL — the watch's half of the loop (see the block above). Two relaxed loads per look when no steal has
/// happened; once per steal generation otherwise.
fn gatesteal_rehome_maybe(now: u64) {
    if !GATESTEAL_REHOME {
        return;
    }
    let Some(dead) = crate::video::wm::comp_dead_core_last() else { return };
    let sgen = crate::video::wm::comp_steal_gen();
    if REHOME_GEN_DONE.load(Acquire) == sgen {
        return;
    }
    let tid = PUMP_TID.load(Relaxed);
    if tid == 0 || PUMP_CPU.load(Relaxed) as usize != dead {
        // Not the pump's core: that death's roles are the render path's (main.rs render_rehome_service), which runs.
        REHOME_GEN_DONE.store(sgen, Release);
        return;
    }
    if REHOME_TRY_GEN.swap(sgen, AcqRel) != sgen {
        REHOME_TRIES.store(0, Relaxed);
    }
    let render = crate::arch::smp::render_cpu();
    let Some(live) = pick_live(dead, render) else {
        serial_println!(":: [pump] REHOME DECLINED: c{} is dead and carried the usb-pump (tid {}) and there is no live core that is neither it nor the render core — co-locating with the render service is the XHCI_CONTROLLER deadlock, so the roles stay down == tripwire ::", dead, tid);
        REHOME_GEN_DONE.store(sgen, Release);
        return;
    };
    let r = crate::arch::sched::rehome_dead_core(dead, live);
    crate::arch::sched::note_dead_core(dead);
    if crate::arch::smp::service_cpu() == Some(dead) {
        if let Some(rc) = render {
            crate::arch::smp::publish_sched_split(rc, live); // the SERVICE role follows the pump
        }
    }
    let tries = REHOME_TRIES.fetch_add(1, Relaxed) + 1;
    REHOME_DEAD.store(dead as u64, Relaxed);
    REHOME_LIVE.store(live as u64, Relaxed);
    REHOME_AT.store(now.max(1), Release);
    let pump_stuck = r.current == Some(tid);
    serial_println!(
        ":: [pump] REHOME: c{} is dead and carried the usb-pump (tid {}): moved ready={} sleeping={} left={} current={} -> c{} svc=c{} try={} (GATESTEAL B657: the mailbox's consumer ran on the corpse; the watch re-homes from a live core; the current task is in no queue and stays) == tripwire ::",
        dead, tid, r.ready, r.sleeping, r.left, r.current.map_or(-1i64, |t| t as i64), live, live, tries
    );
    if !pump_stuck || tries >= GATESTEAL_REHOME_TRIES {
        REHOME_GEN_DONE.store(sgen, Release);
    }
}

/// GATESTEAL — `rehome=` for the PUMPWEDGE line: `ok(pump=<tid>@cpu<n>)` once the pump has STAMPED from a core that
/// is not the one the steal declared dead, else `none`.
fn rehome_word(out: &mut Line) {
    use core::fmt::Write;
    let dead = REHOME_DEAD.load(Relaxed);
    let cpu = PUMP_CPU.load(Relaxed);
    if REHOME_AT.load(Acquire) != 0 && dead != u64::MAX && cpu != dead {
        let _ = write!(out, "ok(pump={}@cpu{})", PUMP_TID.load(Relaxed), cpu as i64);
    } else {
        let _ = out.write_str("none");
    }
}

/// GATESTEAL — a second NMI sample of the pump's core at the wedge, beside the steal's own. Prints nothing when the
/// probe is not built (`witness`/`bar1wedge`), when the core is this one, or when no core is known.
fn gatesteal_probe(cpu: usize) {
    #[cfg(any(feature = "witness", feature = "bar1wedge"))]
    {
        if cpu >= crate::arch::gdt::MAX_CPUS || cpu == crate::arch::percpu::this_cpu().cpu_index as usize {
            return;
        }
        if let Some(p) = crate::arch::interrupts::nmi_probe(cpu) {
            serial_println!(
                ":: GATESTEAL: probe core=c{} taken={} rip={:#x} cs={:#x} if={} (the pump's core at the wedge: the same instruction as the steal's probe is a loop; if=0 is a masked one) ::",
                cpu, if p.taken { "y" } else { "n" }, p.rip, p.cs, (p.rflags >> 9) & 1
            );
        }
    }
    #[cfg(not(any(feature = "witness", feature = "bar1wedge")))]
    let _ = cpu;
}

fn gatesteal_drive(_arg: usize) {
    loop {
        crate::arch::sched::sleep_ms(100);
        if DRIVE_STOP.load(Acquire) {
            return;
        }
        crate::video::wm::composite();
        look(crate::arch::ms());
    }
}

/// `:: GATESTEAL: … -> PASS|FAIL|SKIP ::` — see the block above.
fn gatesteal_selftest() {
    let tid = PUMP_TID.load(Relaxed);
    if tid == 0 {
        serial_println!(":: GATESTEAL: long_step=- steal=- rehome=- passes_after=- input=- -> SKIP :: reason=no-pump-stamp");
        return;
    }
    let dead = PUMP_CPU.load(Relaxed) as usize;
    let render = crate::arch::smp::render_cpu();
    let Some(drive) = pick_live(dead, render).or_else(|| {
        let c = crate::arch::sched::sibling_online_cpu(dead);
        if c != dead { Some(c) } else { None }
    }) else {
        serial_println!(":: GATESTEAL: long_step=- steal=- rehome=- passes_after=- input=- -> SKIP :: reason=no-sibling-core dead=c{}", dead);
        return;
    };
    let gen0 = crate::video::wm::comp_steal_gen();
    REHOME_GEN_DONE.store(gen0, Release);
    REHOME_AT.store(0, Release);
    REHOME_DEAD.store(u64::MAX, Relaxed);
    REHOME_LIVE.store(u64::MAX, Relaxed);
    HOLD_STATE.store(0, Release);
    DRIVE_STOP.store(false, Release);
    GATESTEAL_ACTIVE.store(true, Release);
    let t0 = crate::arch::ms();
    crate::arch::sched::spawn("gatesteal-drive", gatesteal_drive, 0, drive, crate::arch::sched::PRIO_NORMAL);
    HOLD_ARMED.store(true, Release);
    let held = wait_for(3_000, || HOLD_STATE.load(Acquire) != 0);
    let steal = wait_for(GATESTEAL_HOLD_MS + 1_500, || crate::video::wm::comp_steal_gen() != gen0);
    let rehomed = wait_for(GATESTEAL_HOLD_MS + 2_000, || REHOME_AT.load(Acquire) != 0 && PUMP_CPU.load(Relaxed) as usize != dead);
    let p1 = PASSES.load(Relaxed);
    crate::arch::sched::sleep_ms(1_000);
    let passes_after = PASSES.load(Relaxed).saturating_sub(p1);
    let input = passes_after > 0 && crate::drivers::xhci::poll_task_armed();
    DRIVE_STOP.store(true, Release);
    HOLD_ARMED.store(false, Release);
    GATESTEAL_ACTIVE.store(false, Release);
    let mut rh = Line::new();
    rehome_word(&mut rh);
    let ok = held && steal && rehomed && passes_after > 0;
    serial_println!(
        ":: GATESTEAL: long_step={} steal={} rehome={} passes_after={} input={} -> {} :: hold_ms={} gen={} dead=c{} live=c{} drive=c{} tries={} svc_masked_declined={} hold={} elapsed_ms={}",
        if held { "held" } else { "none" },
        if steal { "seen" } else { "none" },
        rh.as_str(),
        passes_after,
        if input { "alive" } else { "stale" },
        if ok { "PASS" } else { "FAIL" },
        GATESTEAL_HOLD_MS,
        crate::video::wm::comp_steal_gen(),
        dead,
        REHOME_LIVE.load(Relaxed) as i64,
        drive,
        REHOME_TRIES.load(Relaxed),
        crate::video::wm::comp_svc_masked_declined(),
        match HOLD_STATE.load(Acquire) { 2 => "stolen", 4 => "kept", 3 => "untaken", 1 => "holding", _ => "none" },
        crate::arch::ms().saturating_sub(t0)
    );
}
