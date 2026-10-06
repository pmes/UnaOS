// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm
//!
//! DRAGDROP (rmbp-ledger B440, MACPARITY row 18) — THE DRAG SESSION. One at a time, the window manager's:
//!
//! * [`arm`] — a source's press on a draggable item hands its [`Payload`] here; the pointer is CAPTURED through
//!   PREFSUI's seam (`video::capture`: every motion sample and the release reach this module while held).
//! * travel past [`threshold`] STARTS the session: `[dnd] start kind=<k> n=<n> from=<win>`, and the GHOST opens —
//!   the item's name on the selection token blended 50 percent into the content token, a chromeless overlay row
//!   that follows the pointer (rows are opaque: true translucency over the glass is owed).
//! * each motion asks the participants `drop_ok(kind)` at the point — the dock first (it composites on top),
//!   then Quarry; a yes is the HOVER, which the participant highlights (`hover`); anything else under the pointer
//!   (another window, FOLDERVIEW's frame, the desktop) is no target and is ignored.
//! * the release delivers `drop(kind, payload)` to the hovered target, or cancels; Esc cancels ([`key`]).
//!   `[dnd] drop to=<win|trash|sidebar> action=<move|copy|favorite|trash> ok=<0|1>` / `[dnd] cancel why=<w>`.
//!
//! Participants this arc: Quarry (`quarry::live::dragdrop` — source, folder rows, the sidebar) and the dock's Trash
//! tile (`dock::dnd_*`). The ring-3 bus protocol for drops into apps is OWED (design doc
//! `docs/dev/evidence/rmbp-1005/dragdrop.md`). Lock order: `S` is never held across a participant call that paints;
//! `HOVER` is a leaf the painters `try_lock`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

use crate::video::wm;

/// What is being dragged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::File => "file",
        }
    }
}

/// The drag's payload: the source's items (absolute paths), whether the (first) item is a folder, the source
/// window, and the ghost's label.
#[derive(Clone, Debug)]
pub struct Payload {
    pub kind: Kind,
    pub paths: Vec<String>,
    pub dir: bool,
    pub from: wm::WinId,
    pub label: String,
}

/// A place that said yes to `drop_ok`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    /// A folder shown by a window (a list row, an icon, a tree row, or the window's own folder).
    Folder { win: wm::WinId, dir: String },
    /// A volume's row in Quarry's sidebar (Locations): a drop copies onto it.
    Volume { dir: String },
    /// Quarry's sidebar Favorites: a folder dropped here becomes a favorite.
    Favorites,
    /// The dock's Trash tile.
    Trash,
}

impl Target {
    /// The `to=` word of the drop line.
    pub fn word(&self) -> String {
        match self {
            Target::Folder { win, .. } => alloc::format!("{}", win),
            Target::Volume { .. } | Target::Favorites => String::from("sidebar"),
            Target::Trash => String::from("trash"),
        }
    }
}

/// What a drop did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Move,
    Copy,
    Favorite,
    Trash,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Move => "move",
            Action::Copy => "copy",
            Action::Favorite => "favorite",
            Action::Trash => "trash",
        }
    }
}

/// `drop_ok` over the participants at a panel point.
pub type Resolver = fn(i32, i32, &Payload) -> Option<Target>;

struct Session {
    p: Payload,
    x0: i32,
    y0: i32,
    started: bool,
    resolve: Resolver,
    ghost: bool,
}

static S: Mutex<Option<Session>> = Mutex::new(None);
static HOVER: Mutex<Option<Target>> = Mutex::new(None);
/// The fixture's Option key (the HID modifier byte is the live one).
static FORCE_OPT: AtomicBool = AtomicBool::new(false);
static STARTS: AtomicU32 = AtomicU32::new(0);
/// The last drop's outcome, for the fixture: `(action, ok)`; `None` = cancelled.
static LAST: Mutex<Option<(Action, bool)>> = Mutex::new(None);
/// The last name `fs::trash` gave a dropped item (the fixture restores it).
pub(crate) static LAST_TRASHED: Mutex<Option<String>> = Mutex::new(None);

/// Pointer travel (panel px, |dx| + |dy|) past which a press is a drag.
pub fn threshold() -> i32 {
    crate::ui::px(5) as i32
}

/// The participants' `drop_ok`: the dock's Trash first (the strip composites over the windows), then Quarry.
fn resolve_live(x: i32, y: i32, p: &Payload) -> Option<Target> {
    if crate::video::dock::dnd_drop_ok(x, y, p.kind == Kind::File) {
        return Some(Target::Trash);
    }
    crate::video::quarry::live::dragdrop::drop_ok(x, y, p)
}

/// A source's press on a draggable item: capture the pointer. Nothing shows until the travel passes the threshold.
pub fn arm(p: Payload, x: i32, y: i32) {
    arm_with(p, x, y, resolve_live, true);
}

/// [`arm`] with a resolver (the fixture's) and the ghost on or off.
pub(crate) fn arm_with(p: Payload, x: i32, y: i32, resolve: Resolver, ghost: bool) {
    if p.paths.is_empty() {
        return;
    }
    *S.lock() = Some(Session { p, x0: x, y0: y, started: false, resolve, ghost });
    crate::video::capture::begin(on_motion, on_release);
}

/// Is a session started (past the threshold)?
pub fn active() -> bool {
    S.try_lock().map(|g| g.as_ref().map(|s| s.started).unwrap_or(false)).unwrap_or(false)
}

/// The hovered target now, for a participant's painter (`None` when nothing is hovered or the lock is busy).
pub fn hover() -> Option<Target> {
    HOVER.try_lock().and_then(|g| g.clone())
}

fn option_held() -> bool {
    FORCE_OPT.load(Ordering::Relaxed) || crate::video::keymap::option_held()
}

/// Tell the participants the hover moved (each repaints its own highlight).
fn hover_changed() {
    crate::video::dock::dnd_hover(matches!(hover(), Some(Target::Trash)));
    crate::video::quarry::live::dragdrop::hover_changed();
}

fn set_hover(t: Option<Target>) -> bool {
    let mut g = HOVER.lock();
    if *g == t {
        return false;
    }
    *g = t;
    true
}

fn on_motion(x: i32, y: i32) {
    let (start, ghost, label, t) = {
        let mut g = S.lock();
        let Some(s) = g.as_mut() else { return };
        let mut start = false;
        if !s.started {
            if (x - s.x0).abs() + (y - s.y0).abs() <= threshold() {
                return;
            }
            s.started = true;
            start = true;
            serial_println!("[dnd] start kind={} n={} from={} label={}", s.p.kind.name(), s.p.paths.len(), s.p.from, s.p.label);
            STARTS.fetch_add(1, Ordering::Relaxed);
        }
        let t = (s.resolve)(x, y, &s.p);
        (start, s.ghost, s.p.label.clone(), t)
    };
    if ghost {
        if start {
            ghost_open(&label, x, y);
        } else {
            ghost_move(x, y);
        }
    }
    if set_hover(t) {
        hover_changed();
    }
}

fn on_release(x: i32, y: i32) {
    let Some(s) = S.lock().take() else { return };
    if !s.started {
        return; // a click: the press already did its work
    }
    let t = (s.resolve)(x, y, &s.p);
    if s.ghost {
        ghost_close();
    }
    set_hover(None);
    hover_changed();
    let Some(t) = t else {
        *LAST.lock() = None;
        serial_println!("[dnd] cancel why=no-target at=({},{})", x, y);
        return;
    };
    let (a, ok) = deliver(&s.p, &t, option_held());
    *LAST.lock() = Some((a, ok));
    serial_println!("[dnd] drop to={} action={} ok={}", t.word(), a.name(), ok as u8);
}

/// `drop(kind, payload)` to the participant that owns `t`.
fn deliver(p: &Payload, t: &Target, option: bool) -> (Action, bool) {
    match t {
        Target::Trash => {
            let mut ok = true;
            for path in p.paths.iter() {
                match crate::video::dock::dnd_drop(path) {
                    Ok(name) => *LAST_TRASHED.lock() = Some(name),
                    Err(e) => {
                        serial_println!("[dnd] trash path={} error={}", path, e);
                        ok = false;
                    }
                }
            }
            (Action::Trash, ok)
        }
        _ => crate::video::quarry::live::dragdrop::drop(p, t, option),
    }
}

/// Esc while a session is started: cancel it (nothing is delivered). `true` = consumed.
pub fn key(ev: crate::pal::Event) -> bool {
    if !matches!(ev, crate::pal::Event::Key(0x1b)) || !active() {
        return false;
    }
    cancel("esc");
    true
}

/// End the session without a drop (Esc, or the source window closed).
pub fn cancel(why: &str) {
    let Some(s) = S.lock().take() else { return };
    crate::video::capture::cancel();
    if s.ghost {
        ghost_close();
    }
    set_hover(None);
    hover_changed();
    *LAST.lock() = None;
    serial_println!("[dnd] cancel why={}", why);
}

// ── the ghost ───────────────────────────────────────────────────────────────────────────────────

struct Ghost {
    win: wm::WinId,
    #[allow(dead_code)]
    surf: Vec<u32>,
    w: usize,
    h: usize,
}

static GHOST: Mutex<Option<Ghost>> = Mutex::new(None);

/// Half of each channel of `a` plus half of `b` — the 50 percent mix of two tokens.
#[cfg_attr(not(all(target_arch = "x86_64", feature = "wc")), allow(dead_code))]
fn mix50(a: u32, b: u32) -> u32 {
    ((a >> 1) & 0x007f_7f7f) + ((b >> 1) & 0x007f_7f7f)
}

/// The ghost's place for a pointer at `(x, y)`: just below-right of the hot spot, on the panel.
fn ghost_at(x: i32, y: i32, w: usize, h: usize) -> (usize, usize) {
    let (pw, ph) = crate::video::panel_info_nonblocking().map(|i| (i.width, i.height)).unwrap_or((w, h));
    let off = crate::ui::px(12) as i32;
    let gx = (x + off).max(0) as usize;
    let gy = (y + off).max(0) as usize;
    (gx.min(pw.saturating_sub(w)), gy.min(ph.saturating_sub(h)))
}

fn ghost_open(label: &str, x: i32, y: i32) {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        use crate::video::{text, theme};
        let face = text::Face::Ui;
        let pad = crate::ui::px(6);
        let s = &label.as_bytes()[..label.len().min(48)];
        let w = (text::advance(s, false, face) + 2 * pad).max(crate::ui::px(32));
        let h = text::chrome_cell().1 + 2 * pad;
        let mut surf: Vec<u32> = Vec::new();
        if surf.try_reserve_exact(w * h).is_err() {
            serial_println!("[dnd] ghost DECLINE reason=oom");
            return;
        }
        let bg = theme::content_fill();
        surf.resize(w * h, mix50(theme::selection(), bg));
        for i in 0..w {
            surf[i] = mix50(theme::frame_line(), bg);
            surf[(h - 1) * w + i] = mix50(theme::frame_line(), bg);
        }
        text::draw_text(&mut surf, w, w, h, pad, pad, s, mix50(theme::content_text(), bg), false, face);
        let (gx, gy) = ghost_at(x, y, w, h);
        let win = wm::overlay_open(surf.as_ptr() as usize, w * h * 4, w, h, gx, gy);
        if win == wm::WIN_NONE {
            serial_println!("[dnd] ghost DECLINE reason=no-row");
            return;
        }
        *GHOST.lock() = Some(Ghost { win, surf, w, h });
    }
    let _ = (label, x, y);
}

fn ghost_move(x: i32, y: i32) {
    let g = GHOST.lock();
    if let Some(gh) = g.as_ref() {
        let (gx, gy) = ghost_at(x, y, gh.w, gh.h);
        let win = gh.win;
        drop(g);
        let _ = wm::move_to(win, gx, gy);
    }
}

fn ghost_close() {
    let g = GHOST.lock().take();
    if let Some(gh) = g {
        wm::close(gh.win);
        drop(gh); // the surface outlives the row
    }
}

// ── the fixture's handles ───────────────────────────────────────────────────────────────────────

/// The fixture's Option key.
pub(crate) fn force_option(on: bool) {
    FORCE_OPT.store(on, Ordering::Relaxed);
}

/// The last drop's `(action, ok)`, taken (`None` = cancelled or none).
pub(crate) fn take_last() -> Option<(Action, bool)> {
    LAST.lock().take()
}

/// Is a session armed or started?
pub(crate) fn armed() -> bool {
    S.lock().is_some()
}
