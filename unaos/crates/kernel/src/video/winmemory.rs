// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//! CHARTER: Kernel — wm
//!
//! WINMEMORY (rmbp-ledger B429, MACPARITY row 12) — the Mac's window placement: an app with a saved frame
//! reopens where the user left it, an app with none is centred, a second window of the same app cascades.
//!
//! The frame is the WM's (it owns it, so it is the only writer); the store is Principia's: the key is
//! `app.<name>.window.frame` = `x,y,w,h` (the OUTER frame in panel px) in `<home>/settings/<name>`
//! (SETTINGSFILES B407), declared by the WM on the app's behalf at its first window
//! ([`crate::prefs::declare_app_key`], a merge — Lumen's own stanza declares the same row). No second store.
//!
//! Declared from the TAIL of `wm.rs` (a CHILD module, WINSNAP's shape) so it reads the table, the work area
//! and the scale rule without widening them. `wm.rs` carries line-neutral one-call seams only:
//! `create_inner` ([`resolve`] before the locks, [`note`] after the publish), `drag_end`/`drag_cancel`
//! ([`moved`], after WINSNAP's snap) and `close` ([`closing`]).
//!
//! * Only rows created with NO requested origin (`wm::create`: every ring-3 program) and whose owner has a
//!   launcher-armed program name ([`super::app_name_of`]) are placed here; everything else (kernel
//!   `create_at` rows, fixtures, compat) is untouched. Quarry places itself with `create_at_native`, so
//!   FOLDERVIEW's per-folder frame (B424) wins by construction; `quarry` is refused by name besides.
//! * Restore: the saved origin, clamped to the work area (below the menu bar, above DOCK2's reservation).
//!   None: centred in the work area. The row is born pinned, so GLASSFIX3's cascade (one title band,
//!   [`super::cascade_step`]) offsets a second window of the same app from the first.
//! * Writes (R96): one store write per move-end or `close(id)`, only when the frame differs from the last
//!   one written, queued off the render task to a `winmem-flush` task on a worker core (INPUTSTALL M5).
//!
//! Witness: `[wm] place win=<id> app=<name> from=<saved|centre|cascade> frame=<x,y,w,h>` per app window;
//! `[winmem] save app=<name> frame=<x,y,w,h> on=<move-end|close> ok=<0|1>` per write;
//! `tests winmemory` → `:: WINMEMORY: saved=<n> restored=<ok> cascade=<ok> clamp=<ok> -> PASS ::`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::{WinId, WIN_NONE};

/// The key under `app.<name>.`.
pub const KEY: &str = "window.frame";
/// The PrefDeclare line the WM declares for every remembered app (the same spec Lumen's stanza carries).
const DECL: &str = "window.frame\tstr:32\t\"\"\tthe window frame as x,y,w,h in panel px, written by the window manager at move-end and close; empty = centred (MACPARITY row 12)\n";
/// The fixture's program name: its saved frame is injected ([`TEST_SAVED`]) and it is never written.
const TEST_APP: &str = "wmtest";

/// Where a placed row's origin came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum From {
    Saved,
    Centre,
}

/// What [`resolve`] decided for a row about to be created, carried to [`note`].
#[derive(Clone)]
pub struct Mem {
    name: String,
    from: From,
    /// The clamped content origin asked for (before GLASSFIX3's cascade).
    want: (usize, usize),
}

/// Live remembered rows: `(id, app name, last frame written or restored)`. A leaf mutex (never held across
/// `TABLE`, `WRITER` or a print).
static TRACK: spin::Mutex<Vec<(WinId, String, String)>> = spin::Mutex::new(Vec::new());
/// Writes waiting for the worker: `(app name, frame literal, on)`; the latest per app wins.
static QUEUE: spin::Mutex<Vec<(String, String, &'static str)>> = spin::Mutex::new(Vec::new());
static FLUSHER: AtomicBool = AtomicBool::new(false);
/// `tests winmemory`: the injected saved frame of [`TEST_APP`].
static TEST_SAVED: spin::Mutex<Option<String>> = spin::Mutex::new(None);

// ── pure ─────────────────────────────────────────────────────────────────────────────────────────

/// `x,y,w,h`.
pub fn enc(f: (usize, usize, usize, usize)) -> String {
    alloc::format!("{},{},{},{}", f.0, f.1, f.2, f.3)
}

/// The inverse of [`enc`]; `None` for anything else (an empty value is "no saved frame").
pub fn dec(s: &str) -> Option<(usize, usize, usize, usize)> {
    let mut it = s.trim().split(',').map(|p| p.trim().parse::<usize>().ok());
    let f = (it.next()??, it.next()??, it.next()??, it.next()??);
    (it.next().is_none() && f.2 > 0 && f.3 > 0).then_some(f)
}

/// The work area a placed row must sit in: `(left, top, right, bottom)` exclusive, panel px.
#[derive(Clone, Copy, Debug)]
pub struct Area {
    pub pw: usize,
    pub top: usize,
    pub bottom: usize,
    pub title: usize,
    pub border: usize,
}

/// The content origin of a `cw` x `ch` row whose OUTER frame was saved at `(fx, fy)`, clamped so the whole
/// outer frame sits in the work area (the menu bar above, DOCK2's reservation below).
pub fn clamp_origin(a: Area, fx: usize, fy: usize, cw: usize, ch: usize) -> (usize, usize) {
    let (bw, bh) = (cw + 2 * a.border, ch + a.title + 2 * a.border);
    let ox = fx.min(a.pw.saturating_sub(bw));
    let oy = fy.min(a.bottom.saturating_sub(bh)).max(a.top);
    (ox + a.border, oy + a.title + a.border)
}

/// The content origin that centres a `cw` x `ch` row's outer frame in the work area.
pub fn centre_origin(a: Area, cw: usize, ch: usize) -> (usize, usize) {
    let (bw, bh) = (cw + 2 * a.border, ch + a.title + 2 * a.border);
    let ox = a.pw.saturating_sub(bw) / 2;
    let oy = a.top + a.bottom.saturating_sub(a.top).saturating_sub(bh) / 2;
    (ox + a.border, oy + a.title + a.border)
}

// ── the seams ────────────────────────────────────────────────────────────────────────────────────

fn app_of(owner: u64) -> Option<String> {
    let mut b = [0u8; super::MAX_TITLE];
    let n = super::app_name_of(owner, &mut b);
    let s = core::str::from_utf8(&b[..n]).ok()?.trim();
    let s: String = s.chars().map(|c| c.to_ascii_lowercase()).collect();
    (prefs_core::valid_segment(&s) && s != "quarry").then_some(s)
}

fn saved_of(name: &str) -> Option<(usize, usize, usize, usize)> {
    if name == TEST_APP {
        return TEST_SAVED.lock().as_deref().and_then(dec);
    }
    let k = alloc::format!("{}.{}", name, KEY);
    crate::prefs::get(prefs_core::files::APP_NS, &k).and_then(|v| v.as_str().and_then(dec))
}

fn area(pw: usize, ph: usize) -> Area {
    let top = super::work_top(pw, ph);
    Area { pw, top, bottom: top + super::work_h(pw, ph), title: super::TITLE_H(), border: super::BORDER() }
}

/// `create_inner`, BEFORE its locks: the origin an unplaced app row is born at — its saved frame (clamped)
/// or the work area's centre — and the record [`note`] completes. `at` is returned unchanged for every row
/// this does not own (a requested origin, compat, no armed name, no panel).
pub fn resolve(owner: u64, at: Option<(usize, usize)>, compat: bool, native: bool, w: usize, h: usize) -> (Option<(usize, usize)>, Option<Mem>) {
    if at.is_some() || compat || owner == 0 {
        return (at, None);
    }
    let Some(name) = app_of(owner) else { return (at, None) };
    let (pw, ph) = {
        let fb = *super::super::WRITER.lock();
        if !fb.is_ready() {
            return (at, None);
        }
        let i = fb.info();
        (i.width, i.height)
    };
    let scale = if native { 1 } else { super::place_scale(pw, ph, w, h) };
    let (cw, ch) = (w.saturating_mul(scale), h.saturating_mul(scale));
    let a = area(pw, ph);
    let (want, from) = match saved_of(&name) {
        Some(f) => (clamp_origin(a, f.0, f.1, cw, ch), From::Saved),
        None => (centre_origin(a, cw, ch), From::Centre),
    };
    (Some(want), Some(Mem { name, from, want }))
}

/// `create_inner`, AFTER the publish and the guard's drop: record the row, declare the key on the app's
/// behalf (once), and say where it went. Called for EVERY create, so a recycled id never keeps a dead
/// tenant's record.
pub fn note(id: WinId, mem: Option<Mem>) {
    ensure_tests();
    let Some(m) = mem else {
        TRACK.lock().retain(|e| e.0 != id);
        return;
    };
    if id == WIN_NONE {
        return;
    }
    let got = super::row(&super::table(), id).map(|r| (r.x, r.y));
    let frame = super::frame_of(id).map(enc).unwrap_or_default();
    let from = match (got, m.from) {
        (Some(g), _) if g != m.want => "cascade",
        (_, From::Saved) => "saved",
        (_, From::Centre) => "centre",
    };
    if m.name != TEST_APP {
        crate::prefs::declare_app_key(&m.name, DECL);
    }
    {
        let mut t = TRACK.lock();
        t.retain(|e| e.0 != id);
        let last = if m.from == From::Saved && from == "saved" { frame.clone() } else { String::new() };
        t.push((id, m.name.clone(), last));
    }
    serial_println!("[wm] place win={} app={} from={} frame={}", id, m.name, from, frame);
}

/// The frame of a remembered row changed hands (move-end) or is about to go (close): queue ONE write when
/// it differs from the last written.
fn save(id: WinId, on: &'static str) {
    if id == WIN_NONE {
        return;
    }
    let Some(f) = super::frame_of(id) else { return };
    let lit = enc(f);
    let name = {
        let mut t = TRACK.lock();
        let Some(e) = t.iter_mut().find(|e| e.0 == id) else { return };
        if e.2 == lit {
            return;
        }
        e.2 = lit.clone();
        e.1.clone()
    };
    if name == TEST_APP {
        return;
    }
    {
        let mut q = QUEUE.lock();
        match q.iter_mut().find(|e| e.0 == name) {
            Some(e) => {
                e.1 = lit;
                e.2 = on;
            }
            None => q.push((name, lit, on)),
        }
    }
    kick();
}

/// `drag_end` / `drag_cancel`, after WINSNAP: the move (or resize) ended.
pub fn moved(id: WinId) {
    save(id, "move-end");
}

/// `close(id)`, before the row is freed.
pub fn closing(id: WinId) {
    save(id, "close");
    TRACK.lock().retain(|e| e.0 != id);
}

fn kick() {
    match crate::arch::smp::worker_cpu(0) {
        Some(cpu) => {
            if !FLUSHER.swap(true, Ordering::AcqRel) {
                crate::arch::sched::spawn("winmem-flush", flush, 0, cpu, crate::arch::sched::PRIO_NORMAL);
            }
        }
        None => {
            flush_once();
        }
    }
}

fn flush_once() -> usize {
    let batch: Vec<(String, String, &'static str)> = core::mem::take(&mut *QUEUE.lock());
    for (name, lit, on) in &batch {
        let k = alloc::format!("{}.{}", name, KEY);
        let ok = crate::prefs::set_applied(prefs_core::files::APP_NS, &k, prefs_core::PrefValue::Str(lit.clone())).is_ok();
        if ok {
            SAVES.fetch_add(1, Ordering::Relaxed); // SMALLFIX7 (B501): `saves_this_boot=`
        }
        serial_println!("[winmem] save app={} frame={} on={} ok={}", name, lit, on, ok as u8);
    }
    batch.len()
}

/// `winmem-flush`: run the queued writes on a worker core, then exit; a write queued meanwhile re-arms it.
fn flush(_: usize) {
    loop {
        if flush_once() == 0 {
            FLUSHER.store(false, Ordering::Release);
            if QUEUE.lock().is_empty() || FLUSHER.swap(true, Ordering::AcqRel) {
                return;
            }
        }
    }
}

// ── `tests winmemory` (R80: never at boot) ───────────────────────────────────────────────────────

fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("winmemory", selftest);
    }
}

/// Saved frames in the store: `app.<name>.window.frame` rows holding a frame.
fn saved_count() -> usize {
    crate::prefs::list(prefs_core::files::APP_NS)
        .iter()
        .filter(|(k, v)| k.ends_with(".window.frame") && v.as_str().and_then(dec).is_some())
        .count()
}

/// Three real rows of a fixture app (`wmtest`, a launcher-armed name on a fixture owner, its saved frame
/// injected so the store is never written): a saved frame restores exactly; a second window of the same
/// app cascades one title band from the first; a saved frame off the panel is clamped into the work area.
/// Every row is closed and the name forgotten.
fn selftest() {
    const OWNER: u64 = super::KERNEL_OWNER_BASE + 0x57;
    static SURF: [u32; 64 * 48] = [0; 64 * 48];
    let (pw, ph) = {
        let i = super::super::WRITER.lock().info();
        (i.width, i.height)
    };
    if pw == 0 || ph == 0 {
        serial_println!(":: WINMEMORY: no panel :: SKIP ::");
        return;
    }
    let a = area(pw, ph);
    let codec = dec(&enc((1, 2, 3, 4))) == Some((1, 2, 3, 4)) && dec("").is_none() && dec("1,2,0,4").is_none();
    let mk = || super::create(OWNER, SURF.as_ptr() as usize, core::mem::size_of_val(&SURF), 64, 48, 256, b"wmtest");
    let origin = |id: WinId| super::row(&super::table(), id).map(|r| (r.x, r.y, r.w * r.scale, r.h * r.scale));
    super::app_name_arm(OWNER, "/apps/wmtest");
    // Restore: a frame in the middle of the work area.
    let (fx, fy) = (pw / 3, a.top + (a.bottom - a.top) / 3);
    *TEST_SAVED.lock() = Some(enc((fx, fy, 200, 150)));
    let w1 = mk();
    let o1 = origin(w1);
    let restored = matches!(o1, Some((x, y, cw, ch)) if (x, y) == clamp_origin(a, fx, fy, cw, ch) && (x, y) == (fx + a.border, fy + a.title + a.border));
    // Cascade: the same app again, the same saved frame.
    let w2 = mk();
    let o2 = origin(w2);
    let step = super::cascade_step();
    // SMALLFIX7 (B501): on a live desktop the one-step offset can itself land on another row's band and move
    // again (GLASSFIX3 walks until clear), so the arm scores the Mac rule against the FIRST row: the second sits
    // off its title band — one full step down (or wrapped to the top) — never on it.
    let cascade = matches!((o1, o2), (Some(p), Some(q)) if (q.0, q.1) == (p.0 + step, p.1 + step) || (q != p && (q.1 >= p.1 + step || q.1 + step <= p.1)));
    for id in [w1, w2] {
        if id != WIN_NONE {
            super::close(id);
        }
    }
    // Clamp: a frame far past the panel's bottom-right corner.
    *TEST_SAVED.lock() = Some(enc((pw * 4, ph * 4, 200, 150)));
    let w3 = mk();
    let clamp = match origin(w3) {
        Some((x, y, cw, ch)) => {
            x + cw + a.border <= pw && y + ch + a.border <= a.bottom && y >= a.top + a.title + a.border
        }
        None => false,
    };
    if w3 != WIN_NONE {
        super::close(w3);
    }
    *TEST_SAVED.lock() = None;
    super::app_name_forget(OWNER);
    let ok = codec && restored && cascade && clamp;
    let v = |b: bool| if b { "ok" } else { "fail" };
    serial_println!(
        ":: WINMEMORY: saved={} saves_this_boot={} restored={} cascade={} clamp={} codec={} step={} area={}..{} -> {} ::",
        saved_count(), SAVES.load(Ordering::Relaxed), v(restored), v(cascade), v(clamp), v(codec), step, a.top, a.bottom, if ok { "PASS" } else { "FAIL" }
    );
}

// ── SMALLFIX7 (rmbp-ledger B501) — TAIL-APPENDED ─────────────────────────────────────────────────────
// Flight 27: `[wm] place win=2 app=wmtest from=cascade` — the fixture's saved frame was resolved, then GLASSFIX3
// cascaded it off a live desktop row (QEMU's desktop is empty, so no lane saw it): `restored=fail`. The Mac
// restores an autosaved frame where it was saved and cascades only a further window of the same app. `saved=0`
// was true: the store counts rows, and no app row was moved or closed that boot (the save triggers).

/// Successful frame writes this boot (move-end / close).
static SAVES: AtomicU32 = AtomicU32::new(0);

/// `create_inner`: keep [`resolve`]'s origin as-is (no GLASSFIX3 cascade) — a saved frame, and no live row of
/// the same app (a second window cascades from the first, as on the Mac).
pub fn keeps(mem: &Option<Mem>) -> bool {
    match mem {
        Some(m) if m.from == From::Saved => !TRACK.lock().iter().any(|e| e.1 == m.name),
        _ => false,
    }
}
