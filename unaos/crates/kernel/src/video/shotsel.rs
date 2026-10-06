//! SHOTREGION (R75) — region and window capture: the selection mode in front of `prtscr`.
//!
//! `⌘⇧4` (or the `shot region` verb) enters REGION mode: the pointer becomes a crosshair
//! (`cursor::set_crosshair`), a primary press anchors a corner and opens the dim overlay, the drag
//! grows the clear rectangle, and the release closes the overlay and arms `prtscr::request_rect`
//! (kind 1). `⌘⇧5` / `shot window` enters WINDOW mode: the overlay is open from the start and
//! its clear rectangle follows the window frame under the pointer (`wm::frame_at`); a click captures
//! that frame (kind 2). `Esc` cancels either, with nothing armed.
//!
//! THE OVERLAY is the SPLASHX86 pattern (`wm::splash_open`): a full-panel, chromeless compat row,
//! pinned topmost with `wm::set_modal_top`. Rows are opaque, so its surface is a DIMMED SNAPSHOT of
//! the panel with the clear rectangle (and a one-pixel white border) cut back to the original
//! pixels. Two panel-sized buffers (original + surface) are held while it is up; a refused
//! allocation declines the mode on the wire (`:: SHOTSEL: decline reason=alloc ::`).
//! A drag repaints only pixels whose value changed between the old and the new rectangle.
//!
//! THE SEAMS: `route` is asked from `wc_route_event` (first of the doors, one atomic load when
//! idle) and `motion` from `wc_route_tail`; both are the position-reading faces of `route_at` /
//! `motion_at`, which take the pointer as a parameter so the fixture can drive them with no HID.
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::sync::Mutex;

use crate::pal::Event;
use crate::video::keymap::Action;
use crate::video::wm;

const OFF: u32 = 0;
const REGION: u32 = 1;
const WINDOW: u32 = 2;
/// Smallest region a release commits; anything smaller is a stray click and cancels.
const MIN_SIDE: usize = 2;
const WHITE: u32 = 0x00FF_FFFF;

static MODE: AtomicU32 = AtomicU32::new(OFF);
static BEGUN: AtomicU32 = AtomicU32::new(0);
static COMMITS: AtomicU32 = AtomicU32::new(0);
static CANCELS: AtomicU32 = AtomicU32::new(0);
/// The last committed capture: `(kind, x, y, w, h)`. The fixture's witness of what was armed.
static LAST: Mutex<(u32, u32, u32, u32, u32)> = Mutex::new((0, 0, 0, 0, 0));

/// Half-open rectangle `[x0, x1) x [y0, y1)`; empty when `x1 <= x0` or `y1 <= y0`.
#[derive(Clone, Copy)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Rect {
    const EMPTY: Rect = Rect { x0: 0, y0: 0, x1: 0, y1: 0 };
    fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    fn w(&self) -> usize {
        self.x1.saturating_sub(self.x0)
    }
    fn h(&self) -> usize {
        self.y1.saturating_sub(self.y0)
    }
    fn has(&self, x: usize, y: usize) -> bool {
        !self.is_empty() && x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
    /// Panel-clipped rectangle from an origin and size.
    fn clip(x: usize, y: usize, w: usize, h: usize, pw: usize, ph: usize) -> Rect {
        Rect { x0: x.min(pw), y0: y.min(ph), x1: x.saturating_add(w).min(pw), y1: y.saturating_add(h).min(ph) }
    }
}

/// The overlay and the selection's state, all behind one lock (taken per event, never across a composite).
struct Sel {
    win: wm::WinId,
    w: usize,
    h: usize,
    /// The panel as it was when the mode opened its overlay.
    orig: Vec<u32>,
    /// The overlay row's surface (xRGB words), `orig` dimmed with the clear rectangle cut back.
    surf: Vec<u32>,
    anchor: (i32, i32),
    down: bool,
    cur: Rect,
}

static SEL: Mutex<Option<Sel>> = Mutex::new(None);

/// Half-brightness for the dim overlay.
fn dim(c: u32) -> u32 {
    (c >> 1) & 0x007F_7F7F
}

/// One surface pixel for the selection `r`: the white border on its edge, the original inside, dimmed outside.
fn val(orig: u32, r: &Rect, x: usize, y: usize) -> u32 {
    if r.has(x, y) {
        if x == r.x0 || x + 1 == r.x1 || y == r.y0 || y + 1 == r.y1 { WHITE } else { orig }
    } else {
        dim(orig)
    }
}

/// Is a selection mode up? One relaxed load — the cost of every event the router asks about while idle.
pub fn active() -> bool {
    MODE.load(Ordering::Relaxed) != OFF
}

/// `(begun, commits, cancels)`.
pub fn census() -> (u32, u32, u32) {
    (BEGUN.load(Ordering::Relaxed), COMMITS.load(Ordering::Relaxed), CANCELS.load(Ordering::Relaxed))
}

fn cursor_pos() -> (i32, i32) {
    let (w, h) = crate::video::panel_info_nonblocking().map_or((0, 0), |i| (i.width as i32, i.height as i32));
    crate::pal::cursor::pos(w, h)
}

fn panel_dims() -> Option<(usize, usize)> {
    let i = crate::video::panel_info_nonblocking()?;
    if i.width == 0 || i.height == 0 { None } else { Some((i.width, i.height)) }
}

/// Snapshot the panel, dim it, and park it as the topmost chromeless row. `None` on a refusal (named on the wire).
fn open_overlay() -> Option<Sel> {
    let panel = crate::video::panel_snapshot().filter(|p| p.is_ready());
    let Some(panel) = panel else {
        serial_println!(":: SHOTSEL: decline reason=no-panel ::");
        return None;
    };
    let info = panel.info();
    let (w, h) = (info.width, info.height);
    let n = w.saturating_mul(h);
    let mut orig: Vec<u32> = Vec::new();
    let mut surf: Vec<u32> = Vec::new();
    if n == 0 || orig.try_reserve_exact(n).is_err() || surf.try_reserve_exact(n).is_err() {
        serial_println!(":: SHOTSEL: decline reason=alloc px={} ::", n);
        return None;
    }
    let t0 = crate::arch::ms();
    for y in 0..h {
        for x in 0..w {
            let c = panel.read_pixel(x, y).unwrap_or(0) & 0x00FF_FFFF;
            orig.push(c);
            surf.push(dim(c));
        }
    }
    let win = wm::splash_open(surf.as_ptr() as usize, n * 4, w, h);
    if win == wm::WIN_NONE {
        serial_println!(":: SHOTSEL: decline reason=window-table ::");
        return None;
    }
    wm::set_modal_top(win);
    serial_println!(":: SHOTSEL: overlay open win={} {}x{} snap_ms={} ::", win, w, h, crate::arch::ms().saturating_sub(t0));
    Some(Sel { win, w, h, orig, surf, anchor: (0, 0), down: false, cur: Rect::EMPTY })
}

/// Close the overlay row (the compositor repaints what was beneath) and free both buffers.
fn close_overlay(sel: Sel) {
    wm::clear_modal_top(sel.win);
    wm::close(sel.win);
    wm::composite();
    drop(sel);
}

/// Move the clear rectangle to `new`, rewriting only the pixels whose value changes, then damage and composite the union.
/// The lock is released before the composite.
fn set_rect(new: Rect) {
    let mut g = SEL.lock();
    let Some(sel) = g.as_mut() else { return };
    let old = sel.cur;
    if old.x0 == new.x0 && old.y0 == new.y0 && old.x1 == new.x1 && old.y1 == new.y1 && (!old.is_empty() || !new.is_empty()) {
        return;
    }
    let (ux0, uy0, ux1, uy1) = match (old.is_empty(), new.is_empty()) {
        (true, true) => return,
        (false, true) => (old.x0, old.y0, old.x1, old.y1),
        (true, false) => (new.x0, new.y0, new.x1, new.y1),
        (false, false) => (old.x0.min(new.x0), old.y0.min(new.y0), old.x1.max(new.x1), old.y1.max(new.y1)),
    };
    let (ux1, uy1) = (ux1.min(sel.w), uy1.min(sel.h));
    for y in uy0..uy1 {
        let row = y * sel.w;
        for x in ux0..ux1 {
            let o = sel.orig[row + x];
            let (a, b) = (val(o, &old, x, y), val(o, &new, x, y));
            if a != b {
                sel.surf[row + x] = b;
            }
        }
    }
    sel.cur = new;
    drop(g);
    wm::damage_intersecting(ux0, uy0, ux1.saturating_sub(ux0), uy1.saturating_sub(uy0));
    wm::composite();
}

/// Enter `mode`. A mode already up is cancelled first (pressing the chord again restarts it).
fn begin(mode: u32, x: i32, y: i32) {
    if active() {
        cancel("restart");
    }
    if mode == WINDOW {
        let Some(sel) = open_overlay() else { return };
        *SEL.lock() = Some(sel);
    }
    MODE.store(mode, Ordering::Release);
    BEGUN.fetch_add(1, Ordering::Relaxed);
    crate::video::cursor::set_crosshair(true);
    serial_println!(":: SHOTSEL: begin kind={} at={},{} ::", if mode == REGION { "region" } else { "window" }, x, y);
    if mode == WINDOW {
        motion_at(x, y);
    }
}

/// Leave the mode with nothing armed.
pub fn cancel(why: &str) {
    let was = MODE.swap(OFF, Ordering::AcqRel);
    if was == OFF {
        return;
    }
    crate::video::cursor::set_crosshair(false);
    let sel = SEL.lock().take();
    if let Some(sel) = sel {
        close_overlay(sel);
    }
    CANCELS.fetch_add(1, Ordering::Relaxed);
    serial_println!(":: SHOTSEL: cancel reason={} ::", why);
}

/// Close the overlay and arm the capture of `r`.
fn commit(kind: u32, r: Rect) {
    let was = MODE.swap(OFF, Ordering::AcqRel);
    if was == OFF {
        return;
    }
    crate::video::cursor::set_crosshair(false);
    let sel = SEL.lock().take();
    if let Some(sel) = sel {
        close_overlay(sel);
    }
    COMMITS.fetch_add(1, Ordering::Relaxed);
    let (x, y, w, h) = (r.x0 as u32, r.y0 as u32, r.w() as u32, r.h() as u32);
    *LAST.lock() = (kind, x, y, w, h);
    serial_println!(":: SHOTSEL: commit kind={} rect={},{},{}x{} ::", if kind == 1 { "region" } else { "window" }, x, y, w, h);
    crate::video::prtscr::request_rect(kind, x, y, w, h);
}

/// The pointer moved to `(x, y)`: grow the region's clear rectangle, or re-aim the window highlight.
pub fn motion_at(x: i32, y: i32) {
    match MODE.load(Ordering::Relaxed) {
        REGION => {
            let r = {
                let g = SEL.lock();
                let Some(sel) = g.as_ref() else { return };
                if !sel.down {
                    return;
                }
                let (ax, ay) = sel.anchor;
                let (pw, ph) = (sel.w, sel.h);
                let (x0, x1) = (ax.min(x).max(0) as usize, ax.max(x).max(0) as usize + 1);
                let (y0, y1) = (ay.min(y).max(0) as usize, ay.max(y).max(0) as usize + 1);
                Rect { x0: x0.min(pw), y0: y0.min(ph), x1: x1.min(pw), y1: y1.min(ph) }
            };
            set_rect(r);
        }
        WINDOW => {
            let (win, pw, ph) = {
                let g = SEL.lock();
                let Some(sel) = g.as_ref() else { return };
                (sel.win, sel.w, sel.h)
            };
            let r = match wm::frame_at(x, y, win) {
                Some((_, fx, fy, fw, fh)) => Rect::clip(fx, fy, fw, fh, pw, ph),
                None => Rect::EMPTY,
            };
            set_rect(r);
        }
        _ => {}
    }
}

/// [`motion_at`] at the live pointer — the `wc_route_tail` hook. One relaxed load while idle.
pub fn motion() {
    if active() {
        let (x, y) = cursor_pos();
        motion_at(x, y);
    }
}

/// The router seam: offer `ev` to the selection. `true` = consumed. The chords begin a mode, and while one is up
/// `Esc` cancels, the primary button drives press/release, and keys are swallowed (a stray key must not reach the shell).
pub fn route(ev: Event) -> bool {
    if !active() && !matches!(ev, Event::Action(Action::ScreenshotRegion | Action::ScreenshotWindow)) {
        return false;
    }
    let (x, y) = cursor_pos();
    route_at(ev, x, y)
}

/// [`route`] with the pointer supplied (the fixture drives this; there is no HID pointer in the gate).
pub fn route_at(ev: Event, x: i32, y: i32) -> bool {
    match ev {
        Event::Action(Action::ScreenshotRegion) => {
            begin(REGION, x, y);
            true
        }
        Event::Action(Action::ScreenshotWindow) => {
            begin(WINDOW, x, y);
            true
        }
        _ if !active() => false,
        Event::Key(0x1B) => {
            cancel("esc");
            true
        }
        Event::Key(_) | Event::KeyUp(_) => true,
        Event::Button(mask) => {
            button(mask, x, y);
            true
        }
        _ => false,
    }
}

fn button(mask: u8, x: i32, y: i32) {
    let mode = MODE.load(Ordering::Relaxed);
    if mask & 0x01 != 0 {
        // Primary PRESS.
        let have = SEL.lock().is_some();
        if mode == REGION && !have {
            let Some(sel) = open_overlay() else {
                cancel("no-overlay");
                return;
            };
            *SEL.lock() = Some(sel);
        }
        if let Some(sel) = SEL.lock().as_mut() {
            if !sel.down {
                sel.down = true;
                sel.anchor = (x, y);
            }
        }
        motion_at(x, y);
        return;
    }
    // Primary RELEASE (mask without the bit).
    let (down, cur) = match SEL.lock().as_mut() {
        Some(sel) => {
            let d = sel.down;
            sel.down = false;
            (d, sel.cur)
        }
        None => (false, Rect::EMPTY),
    };
    if !down {
        return;
    }
    if mode == REGION {
        if cur.w() < MIN_SIDE || cur.h() < MIN_SIDE {
            cancel("too-small");
        } else {
            commit(1, cur);
        }
    } else if mode == WINDOW {
        if cur.is_empty() { cancel("no-window") } else { commit(2, cur) }
    }
}

/// `shot region` / `shot window` verb entry: enter a mode at the live pointer. `false` when it cannot (no desktop panel).
pub fn enter(window: bool) -> bool {
    if panel_dims().is_none() {
        return false;
    }
    let (x, y) = cursor_pos();
    route_at(Event::Action(if window { Action::ScreenshotWindow } else { Action::ScreenshotRegion }), x, y)
}

/// SHOTREGION — the fixture (`tests shotregion`). Drives the REAL seams: the chord enters through
/// `wc_route_event` (the router's first door), the drag through `route_at`/`motion_at` with explicit points,
/// `Esc` through `wc_route_event`; then the armed rectangle is asserted and written through the real
/// `prtscr::capture()` (the file must be non-empty — or be refused for want of a volume/session, said on the wire,
/// not scored as a pass of the file leg).
pub fn selftest() {
    let Some((pw, ph)) = panel_dims() else {
        serial_println!(":: SHOTREGION: region_ok=0 window_ok=0 cancel_ok=0 reason=no-panel -> FAIL ::");
        return;
    };
    let (pwi, phi) = (pw as i32, ph as i32);
    cancel("fixture-reset");
    let route = crate::arch::x86_64::syscall::wc_route_event;

    // REGION: chord -> press -> drag -> release.
    let (ax, ay, bx, by) = (pwi / 8, phi / 8, pwi / 2, phi / 2);
    let consumed = matches!(route(Event::Action(Action::ScreenshotRegion)), Event::Unknown);
    let crossed = active() && crate::video::cursor::crosshair();
    route_at(Event::Button(1), ax, ay);
    motion_at(bx, by);
    route_at(Event::Button(0), bx, by);
    let want = (1u32, ax as u32, ay as u32, (bx - ax + 1) as u32, (by - ay + 1) as u32);
    let got = *LAST.lock();
    let region_geom = got == want && !active() && !crate::video::cursor::crosshair() && SEL.lock().is_none();
    let region_file = capture_leg(1, want);
    let region_ok = consumed && crossed && region_geom && region_file != Leg::Bad;

    // WINDOW: find a window under a grid point, enter, highlight, click.
    let mut probe: Option<(i32, i32, Rect)> = None;
    'scan: for gy in 1..8 {
        for gx in 1..8 {
            let (px, py) = (pwi * gx / 8, phi * gy / 8);
            if let Some((_, fx, fy, fw, fh)) = wm::frame_at(px, py, wm::WIN_NONE) {
                probe = Some((px, py, Rect::clip(fx, fy, fw, fh, pw, ph)));
                break 'scan;
            }
        }
    }
    let (window_geom, window_file) = match probe {
        Some((px, py, fr)) => {
            route_at(Event::Action(Action::ScreenshotWindow), px, py);
            let lit = active() && SEL.lock().as_ref().is_some_and(|s| s.cur.x0 == fr.x0 && s.cur.y0 == fr.y0 && s.cur.x1 == fr.x1 && s.cur.y1 == fr.y1);
            route_at(Event::Button(1), px, py);
            route_at(Event::Button(0), px, py);
            let want = (2u32, fr.x0 as u32, fr.y0 as u32, fr.w() as u32, fr.h() as u32);
            (lit && *LAST.lock() == want && !active(), capture_leg(2, want))
        }
        None => (false, Leg::Bad),
    };
    let window_ok = window_geom && window_file != Leg::Bad;

    // CANCEL: begin, press, drag, Esc through the router -> nothing armed, overlay gone.
    let req0 = crate::video::prtscr::census().0;
    let last0 = *LAST.lock();
    route_at(Event::Action(Action::ScreenshotRegion), ax, ay);
    route_at(Event::Button(1), ax, ay);
    motion_at(bx, by);
    let up = SEL.lock().is_some();
    let esc = matches!(route(Event::Key(0x1B)), Event::Unknown);
    let cancel_ok = up && esc && !active() && SEL.lock().is_none() && !crate::video::cursor::crosshair()
        && crate::video::prtscr::census().0 == req0 && *LAST.lock() == last0;

    let pass = region_ok && window_ok && cancel_ok;
    serial_println!(
        ":: SHOTREGION: region_ok={} window_ok={} cancel_ok={} region_file={} window_file={} panel={}x{} -> {} ::",
        region_ok as u8, window_ok as u8, cancel_ok as u8, region_file.name(), window_file.name(), pw, ph,
        if pass { "PASS" } else { "FAIL" }
    );
}

#[derive(PartialEq, Clone, Copy)]
enum Leg {
    /// Written: bytes > 0 and the Shot carried the kind and size armed.
    Written,
    /// Refused for want of a session/volume — said, not scored.
    Refused,
    Bad,
}

impl Leg {
    fn name(self) -> &'static str {
        match self {
            Leg::Written => "written",
            Leg::Refused => "refused",
            Leg::Bad => "BAD",
        }
    }
}

/// Write the armed capture synchronously and check it against `want = (kind, x, y, w, h)`.
fn capture_leg(kind: u32, want: (u32, u32, u32, u32, u32)) -> Leg {
    match crate::video::prtscr::capture() {
        Ok(shot) => {
            if shot.bytes > 0 && shot.kind == kind && (shot.rx, shot.ry, shot.width, shot.height) == (want.1, want.2, want.3, want.4) {
                Leg::Written
            } else {
                Leg::Bad
            }
        }
        Err(why) => {
            why.report();
            // The rect stays armed when a refusal comes before `Job::begin`; drop it so the next service pass does not write it.
            crate::video::prtscr::disarm();
            Leg::Refused
        }
    }
}
