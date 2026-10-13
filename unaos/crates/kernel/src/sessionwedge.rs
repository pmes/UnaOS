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
//!    print path fired on a fixture-fed stale stamp (`-> SEEN :: fixture=1`, the same formatter, painted too).

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
            PUMP_CPU.store(crate::arch::percpu::this_cpu().cpu_index as u64, Relaxed);
        }
    }
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
    serial_println!(
        ":: PUMPWEDGE: age_ms={} last_step={} holder={} -> {} :: pump={}@cpu{} passes={} wedges={} bound_ms={}{}",
        age_ms, step, h.as_str(), if fixture { "SEEN" } else { "FAIL" },
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
    let last = LAST_MS.load(Relaxed);
    if last != 0 && now.saturating_sub(last) >= WEDGE_MS {
        let age = now.saturating_sub(last);
        if !FIRED.swap(true, AcqRel) {
            WEDGES.fetch_add(1, Relaxed);
            NEXT_PAINT.store(now + PAINT_EVERY_MS, Relaxed);
            say(age, false);
            let t = PUMP_TID.load(Relaxed);
            if t != 0 {
                crate::sync::name_task(t, "PUMPWEDGE: the usb-pump, stopped"); // LOCKREG: its holds and the lock it spins on
            }
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
        FIX_FIRES.fetch_add(1, Release);
    }
}

/// The panel line, straight onto the framebuffer — no compositor, no pump, no heap. A busy `WRITER` skips this paint
/// (the next one tries again). Empty `text` clears the band. Bottom band, two chrome cells high (SPLASHSTALL's).
fn paint(text: &str) {
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
    if !text.is_empty() {
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
    }
    fb.flush_rect(0, y0, i.width, band_h);
}

const BAND: u32 = 0x0060_1010;
const INK: u32 = 0x00FF_F0E0;

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
    if seen {
        paint(""); // the fixture's band goes
    }
    let ok = matches!(built, Some(ms) if ms <= IGNITION_MS) && through && gap < WEDGE_MS && passes > 0 && seen;
    serial_println!(
        ":: SESSIONWEDGE: bare_to_built_ms={} pump_gap_max_ms={} pump_passes={} through={} armed={} watch={}:{} -> {} :: window_ms={} wedge_ms={} wedges={} watched={}",
        match built { Some(ms) => alloc::format!("{}", ms), None => alloc::string::String::from("never") },
        gap, passes, through as u8, a.as_str(), if seen { "seen" } else { "silent" }, FIX_AGE.load(Relaxed),
        if ok { "PASS" } else { "FAIL" }, IGNITION_MS, WEDGE_MS, WEDGES.load(Relaxed), WATCHED.load(Relaxed) as u8
    );
}
