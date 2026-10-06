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
    /// DRAGDROP2 (B470): a dock APP tile whose program declares every dragged file's type: the drop opens each
    /// file there (`opener` is what the one dispatch runs — a built-in's key or a ring-3 program's path).
    Dock { tile: wm::WinId, key: String, opener: String },
    /// DRAGDROP2: the bare desktop — the session user's Desktop folder.
    Desktop { dir: String },
    /// DRAGDROP2: a ring-3 program's window — the paths go to its input ring (`INPUT_EV_DROP` + `BUS_VERB_DROP_GET`).
    App { win: wm::WinId, owner: u64 },
    /// DROPTYPES (B477): a ring-3 program's window whose program does not declare every dragged file's type
    /// (`types` = how many distinct types it does not take): the ghost shows the refusal, the release delivers nothing.
    Refused { win: wm::WinId, owner: u64, types: usize },
}

impl Target {
    /// The `to=` word of the drop line.
    pub fn word(&self) -> String {
        match self {
            Target::Folder { win, .. } => alloc::format!("{}", win),
            Target::Volume { .. } | Target::Favorites => String::from("sidebar"),
            Target::Trash => String::from("trash"),
            Target::Dock { key, .. } => alloc::format!("dock:{}", key),
            Target::Desktop { .. } => String::from("desktop"),
            Target::App { owner, .. } | Target::Refused { owner, .. } => alloc::format!("app:{}", owner),
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
    /// DRAGDROP2: opened in a dock app.
    Open,
    /// DRAGDROP2: handed to a ring-3 program.
    Deliver,
    /// DROPTYPES (B477): a ring-3 program that does not take the dragged types; nothing was delivered.
    Refused,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Move => "move",
            Action::Copy => "copy",
            Action::Favorite => "favorite",
            Action::Trash => "trash",
            Action::Open => "open",
            Action::Deliver => "deliver",
            Action::Refused => "refused",
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
    /// DRAGDROP2: show a folder in place (spring-loading); returns the folder shown before, `None` = no change.
    spring: SpringFn,
    /// The last pointer point (the spring re-resolves the hover there).
    last: (i32, i32),
    /// The folder the drag began in, once a spring moved away from it (Esc backs out to it).
    sprung_from: Option<String>,
}

/// Spring-loading's action: show `dir` in place; `Some(<the folder shown before>)` when it moved.
pub type SpringFn = fn(&str) -> Option<String>;

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
    if let Some((tile, key, opener)) = crate::video::dock::dnd_app_at(x, y, &p.paths) {
        return Some(Target::Dock { tile, key, opener });
    }
    if let Some(t) = crate::video::quarry::live::dragdrop::drop_ok(x, y, p) {
        return Some(t);
    }
    resolve_glass(x, y, p)
}

/// A source's press on a draggable item: capture the pointer. Nothing shows until the travel passes the threshold.
pub fn arm(p: Payload, x: i32, y: i32) {
    arm_full(p, x, y, resolve_live, true, crate::video::quarry::live::dragdrop::spring_to);
}

/// [`arm`] with a resolver (the fixture's) and the ghost on or off.
pub(crate) fn arm_with(p: Payload, x: i32, y: i32, resolve: Resolver, ghost: bool) {
    arm_full(p, x, y, resolve, ghost, spring_none);
}

fn spring_none(_: &str) -> Option<String> {
    None
}

/// [`arm_with`] with the spring action (the fixture's, or Quarry's).
pub(crate) fn arm_full(p: Payload, x: i32, y: i32, resolve: Resolver, ghost: bool, spring: SpringFn) {
    if p.paths.is_empty() {
        return;
    }
    *TYPES_MEMO.lock() = None; // DROPTYPES: a new drag asks each window afresh
    *S.lock() = Some(Session { p, x0: x, y0: y, started: false, resolve, ghost, spring, last: (x, y), sprung_from: None });
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
    let h = hover();
    crate::video::dock::dnd_hover(matches!(h, Some(Target::Trash)));
    crate::video::dock::dnd_app_hover(match h { Some(Target::Dock { tile, .. }) => tile, _ => wm::WIN_NONE });
    crate::video::quarry::live::dragdrop::hover_changed();
}

fn set_hover(t: Option<Target>) -> bool {
    let mut g = HOVER.lock();
    if *g == t {
        return false;
    }
    *g = t;
    HOVER_AT.store(crate::arch::ms(), Ordering::Relaxed);
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
        s.last = (x, y);
        let t = (s.resolve)(x, y, &s.p);
        (start, s.ghost, s.p.label.clone(), t)
    };
    let refused = matches!(t, Some(Target::Refused { .. }));
    if ghost {
        if start {
            GHOST_REFUSED.store(false, Ordering::Relaxed);
            ghost_open(&label, x, y);
        } else if GHOST_REFUSED.load(Ordering::Relaxed) != refused {
            ghost_refusal(refused, &label, x, y); // DROPTYPES: the ghost says the window will not take it
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
    if let Target::Refused { types, .. } = t {
        *LAST.lock() = Some((Action::Refused, false));
        serial_println!("[dnd] drop to={} action=refused reason=type-undeclared types={}", t.word(), types);
        return;
    }
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
        Target::Dock { key, opener, .. } => {
            let mt = crate::shell::vfs_mount_table();
            for path in p.paths.iter() {
                let (mime, _) = crate::fs::filetype::type_of_in(&mt, path);
                let line = crate::video::quarry::live::openers::open(opener, path, &mime);
                serial_println!("[dnd] dock app={} opener={} path={} type={} -> {}", key, opener, path, mime, line);
            }
            (Action::Open, true)
        }
        Target::App { owner, .. } => (Action::Deliver, ring3_deliver(*owner, &p.paths)),
        Target::Refused { .. } => (Action::Refused, false),
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
    if let Some(d) = s.sprung_from.as_deref() {
        let _ = (s.spring)(d); // DRAGDROP2: Esc backs a spring out to the folder the drag began in
        serial_println!("[dnd] spring back dir={}", d);
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

// ── DRAGDROP2 (rmbp-ledger B470, MACPARITY row 18) ─────────────────────────────────────────────────────────────
// The desktop and ring-3 windows as targets, spring-loading, and the ring-3 drop protocol (una_abi
// `INPUT_EV_DROP` + `BUS_VERB_DROP_GET`; the kernel fulfils the verb from the store below). Design:
// `docs/dev/evidence/rmbp-1005/dragdrop2.md`.

/// The hover's stamp (ms): when the pointer came to rest on the current target.
static HOVER_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// A folder hovered this long springs open in place (the Finder's default spring delay).
pub const SPRING_MS: u64 = 800;

/// The session user's Desktop folder (the desktop target).
pub fn desktop_dir() -> String {
    alloc::format!("{}/Desktop", crate::fs::trash::home_base())
}

/// May `p` drop into the folder `dir` (not the folder itself, not into itself, not back where it is)?
pub fn into_ok(p: &Payload, dir: &str) -> bool {
    let parent = |s: &str| -> String { match s.rfind('/') { Some(0) | None => String::from("/"), Some(i) => String::from(&s[..i]) } };
    let under = |a: &str, d: &str| a.len() > d.len() && a.as_bytes()[..d.len()].eq_ignore_ascii_case(d.as_bytes()) && a.as_bytes()[d.len()] == b'/';
    p.paths.iter().all(|s| !s.eq_ignore_ascii_case(dir) && !under(dir, s) && !parent(s).eq_ignore_ascii_case(dir))
}

/// Past Quarry and the dock: a ring-3 program's window (x86), else the bare desktop (no window, no strip, a
/// session's desktop built).
fn resolve_glass(x: i32, y: i32, p: &Payload) -> Option<Target> {
    if x < 0 || y < 0 {
        return None;
    }
    if let Some((win, owner, _)) = wm::hit_test(x, y) {
        if ring3_live(owner) {
            return Some(match undeclared(owner, &p.paths) { 0 => Target::App { win, owner }, types => Target::Refused { win, owner, types } }); // DROPTYPES (B477): only a program that declares every type
        }
        return None;
    }
    if !crate::video::desktopbuild::built() {
        return None;
    }
    let (pw, ph) = crate::video::panel_info_nonblocking().map(|i| (i.width, i.height))?;
    let mut rs = [None; crate::video::strip::STRIP_MAX];
    crate::video::strip::rects(pw, ph, &mut rs);
    let (ux, uy) = (x as usize, y as usize);
    if rs.iter().flatten().any(|&(rx, ry, rw, rh)| ux >= rx && uy >= ry && ux < rx + rw && uy < ry + rh) {
        return None;
    }
    let dir = desktop_dir();
    into_ok(p, &dir).then_some(Target::Desktop { dir })
}

/// Spring-loading — the device-service pass (~1 kHz) calls this; cheap when no drag is started.
pub fn service() {
    if active() {
        spring_check(crate::arch::ms());
    }
}

/// The hover's stamp, for the fixture.
pub(crate) fn hover_at() -> u64 {
    HOVER_AT.load(Ordering::Relaxed)
}

/// A started drag resting on a folder target for [`SPRING_MS`] at `now` shows that folder in place. `true` = sprang.
pub(crate) fn spring_check(now: u64) -> bool {
    let dir = match hover() {
        Some(Target::Folder { dir, .. }) | Some(Target::Volume { dir }) => dir,
        _ => return false,
    };
    let at = HOVER_AT.load(Ordering::Relaxed);
    if now < at.saturating_add(SPRING_MS) {
        return false;
    }
    let Some(spring) = S.try_lock().and_then(|g| g.as_ref().filter(|s| s.started).map(|s| s.spring)) else { return false };
    HOVER_AT.store(now, Ordering::Relaxed); // one spring per rest, whatever it did
    let Some(prev) = spring(&dir) else { return false };
    serial_println!("[dnd] spring dir={} after_ms={}", dir, now - at);
    let t = {
        let mut g = S.lock();
        let Some(s) = g.as_mut() else { return true };
        if s.sprung_from.is_none() {
            s.sprung_from = Some(prev);
        }
        (s.resolve)(s.last.0, s.last.1, &s.p)
    };
    if set_hover(t) {
        hover_changed();
    }
    true
}

// ── the ring-3 drop protocol ────────────────────────────────────────────────────────────────────────────────────

struct Pending {
    owner: u64,
    token: u8,
    paths: Vec<String>,
}

/// Drops handed to ring-3 programs and not yet asked for (oldest dropped past the bound).
static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
const PENDING_MAX: usize = 8;
static TOKEN: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Hold `paths` for `owner` under a fresh token (non-zero); at most `DROP_PATHS_MAX` paths are kept.
pub(crate) fn ring3_store(owner: u64, paths: &[String]) -> u8 {
    let mut t = TOKEN.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    if t == 0 {
        t = TOKEN.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    }
    let keep: Vec<String> = paths.iter().take(una_abi::DROP_PATHS_MAX).cloned().collect();
    let mut g = PENDING.lock();
    g.retain(|d| !(d.owner == owner && d.token == t));
    if g.len() >= PENDING_MAX {
        g.remove(0);
    }
    g.push(Pending { owner, token: t, paths: keep });
    t
}

/// A drop on a ring-3 window: store the paths for `owner`, push `INPUT_EV_DROP` to its ring. `true` = pushed.
fn ring3_deliver(owner: u64, paths: &[String]) -> bool {
    let token = ring3_store(owner, paths);
    let n = paths.len().min(una_abi::DROP_PATHS_MAX);
    let ev = una_abi::input_ev_pack(una_abi::INPUT_EV_DROP, una_abi::drop_ev_payload(token, n));
    #[cfg(target_arch = "x86_64")]
    let pushed = crate::arch::x86_64::syscall::user_input_push_owner(owner, ev);
    #[cfg(not(target_arch = "x86_64"))]
    let pushed = {
        let _ = ev;
        false // aarch64: no capture feeds a drag there yet, and DROP_GET has no arm (owed)
    };
    serial_println!("[dnd] ring3 owner={} token={} n={} pushed={} types=declared", owner, token, n, pushed as u8);
    pushed
}

/// `BUS_VERB_DROP_GET` for the kernel-stamped `owner`: body `[token]`; the reply text is the paths, newline-joined.
/// `-2` (ENOENT) = no such drop for this caller; `-22` = malformed.
pub fn bus_drop_get(owner: u64, body: &[u8], text: &mut Vec<u8>) -> i64 {
    if body.len() != 1 {
        return -22;
    }
    let token = body[0];
    let d = {
        let mut g = PENDING.lock();
        match g.iter().position(|d| d.owner == owner && d.token == token) {
            Some(i) => g.remove(i),
            None => return -2,
        }
    };
    let refs: Vec<&[u8]> = d.paths.iter().map(|s| s.as_bytes()).collect();
    let mut out = alloc::vec![0u8; crate::bus::BUS_BODY_MAX];
    let Some(n) = una_abi::drop_body(&refs, &mut out) else { return -22 };
    text.extend_from_slice(&out[..n]);
    serial_println!("[dnd] ring3 get owner={} token={} n={}", owner, token, d.paths.len());
    0
}

// ── DROPTYPES (rmbp-ledger B477, MACPARITY row 18) ──────────────────────────────────────────────────────────────
// A ring-3 window is a drop target only when its program DECLARES every dragged file's type: APPRES's `droptypes`
// (the resource block's `una:droptypes`, read into `appres::App`), under the dock tile's one rule
// (`dock::dnd_takes`: exact, `type/*`, `*/*`). Absent = takes nothing. Design:
// `docs/dev/evidence/rmbp-1005/droptypes.md`.

/// `(owner, first path, count, undeclared)` — the last answer, so a motion sample costs no type lookups.
static TYPES_MEMO: Mutex<Option<(u64, String, usize, usize)>> = Mutex::new(None);
/// Is the ghost showing the refusal now?
static GHOST_REFUSED: AtomicBool = AtomicBool::new(false);

/// Is `owner` a live ring-3 program (not kernel furniture, not exited)?
fn ring3_live(owner: u64) -> bool {
    #[cfg(target_arch = "x86_64")]
    return crate::arch::x86_64::syscall::ring3_owner_live(owner);
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = owner;
        false
    }
}

/// The drop types `owner`'s program declares: its armed name → the APPRES key → the registrant's `droptypes`.
fn owner_droptypes(owner: u64) -> Vec<String> {
    let mut b = [0u8; wm::MAX_TITLE];
    let n = wm::app_name_of(owner, &mut b);
    crate::fs::appres::key_of_title(&b[..n]).and_then(|k| crate::fs::appres::app(&k)).map(|a| a.droptypes).unwrap_or_default()
}

/// How many DISTINCT types of `mimes` the declaration `drops` does not take (0 = it takes them all).
pub(crate) fn undeclared_of(drops: &[String], mimes: &[String]) -> usize {
    let mut miss: Vec<&str> = Vec::new();
    for m in mimes.iter() {
        if !crate::video::dock::dnd_takes(drops, m) && !miss.iter().any(|x| x.eq_ignore_ascii_case(m)) {
            miss.push(m.as_str());
        }
    }
    miss.len()
}

/// [`undeclared_of`] for `owner`'s program over `paths`' types, memoised for the drag.
fn undeclared(owner: u64, paths: &[String]) -> usize {
    if let Some((o, f, c, n)) = TYPES_MEMO.lock().as_ref() {
        if *o == owner && *c == paths.len() && paths.first() == Some(f) {
            return *n;
        }
    }
    let drops = owner_droptypes(owner);
    let mt = crate::shell::vfs_mount_table();
    let mimes: Vec<String> = paths.iter().map(|p| crate::fs::filetype::type_of_in(&mt, p).0).collect();
    let n = undeclared_of(&drops, &mimes);
    *TYPES_MEMO.lock() = Some((owner, paths.first().cloned().unwrap_or_default(), paths.len(), n));
    n
}

/// The ghost on or off the refusal: reopened with `not accepted: <label>` (or the plain label) at the pointer.
fn ghost_refusal(refused: bool, label: &str, x: i32, y: i32) {
    GHOST_REFUSED.store(refused, Ordering::Relaxed);
    ghost_close();
    if refused {
        ghost_open(&alloc::format!("not accepted: {}", label), x, y);
    } else {
        ghost_open(label, x, y);
    }
}
