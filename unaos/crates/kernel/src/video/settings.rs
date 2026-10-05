// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! SETTINGS (R75) — one window that puts the knobs the OS already has behind sliders and toggles, so
//! the operator stops typing verbs for them. Built on FILEVIEW's pattern (`video/fileview.rs`): a
//! cached-RAM surface, `wm::create_at`, keys/press chained from `quarry::live`, a latch drained by
//! [`service`].
//!
//! Controls (index; BRIGHTFLOOR M5: control 0 is on the DISPLAY tab, keyboard order there is 0 then 3, and General's
//! order is 1 2 4 5 6 7 8): 0 Brightness (BRIGHTFLOOR: level 1..16, through `backlight::set_level_via`; 0 is never written) · 1 Volume (VOLKEYS /
//! HDA amp 0..16) · 2 Mute · 3 Idle blank minutes (DIMIDLE `set_idle_min`, 0 = never) · 4 Pointer
//! speed (TPSPEED divisors: slow / normal / fast) · 5 Wallpaper path (text field) · 6 Apply · 7 Off ·
//! 8 Change Password (the login screen's set-password form for the session user). The Clock 24h/12h
//! toggle is omitted: CLOCKBAR's glyph path is not a runtime switch.
//! KERNELFONT2 (B363): 9 Font (sans / serif / mono) · 10 Font size (CSS px, - / +), both on the Display tab, written
//! to `system.display.font` / `system.display.font_size`.
//!
//! Every change prints `[settings] <name>=<value> applied=<0|1>` and is persisted to PRINCIPIA'S store —
//! `<home>/.config/unaos/preferences.toml`, namespace `system` (`crate::prefs`, PREFS B300; the private
//! `<home>/.settings` this window used to keep is gone, imported once and deleted). [`service`] applies the
//! store's keys once per login (`[settings] loaded n=<n>`). Absent keys keep the OS defaults.
//!
//! Mouse: press a slider track to set it, a toggle/segment/button to act. (No drag: the wm drag seam
//! belongs to window frames; a press-to-set is the claim.) Keyboard: Up/Down/Tab move the selection,
//! Left/Right adjust, Enter toggles/applies; typing edits the wallpaper path while it is selected.
//!
//! SETTINGS2: four TABS (General · Users · Display · About; Left/Right on the strip or a click switches, the choice
//! persists as `system.settings.tab`); Users = list + Add / Delete (two-step) / Reset password, every action a
//! `[settings] users op= name= ok= reason=` line; Display = brightness (BRIGHTFLOOR M5), idle blank, UI scale (read-only: the compositor fixes it at
//! takeover), clock (fixed: CLOCKBAR has no runtime switch); About = version, board, CPUs, RAM, uptime.
//!
//! Witness: `:: SETTINGS: controls=<n> tabs=<n> loaded=<n> saved=<n> -> PASS ::` on open and on save, and the
//! `tests` fixture `settings` ([`selftest`]: set idle to 5, save, re-read the file, compare).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::video::{theme, wm};

/// Kernel-furniture owner slot (`+ 7`, after TEXTEDIT's `+ 6`).
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 7;
const _: () = assert!(OWNER != super::fileview::OWNER && OWNER != super::textedit::OWNER);

/// Number of controls.
pub const CONTROLS: usize = 11;
const NAMES: [&str; CONTROLS] = ["brightness", "volume", "mute", "idle_min", "pointer", "wallpaper", "wallpaper-apply", "wallpaper-off", "password", "font", "font_size"];
/// KERNELFONT2 (B363) M4: the Display tab's Font picker — `system.display.font` (control 9, the schema's enum) and
/// `system.display.font_size` (control 10, CSS px, the schema's 9..=32).
const FONT_FAMS: [&str; 3] = ["sans", "serif", "mono"];
const FONT_MIN: i64 = 9;
const FONT_MAX: i64 = 32;
/// Idle-minute steps the slider walks (0 = never).
pub const IDLE_STEPS: [u32; 8] = [0, 1, 2, 5, 10, 15, 30, 60];
const PTR_NAMES: [&str; 3] = ["slow", "normal", "fast"];
const WALL_MAX: usize = 120;

const WIN_W: usize = 520;
const TAB_H: usize = 28;
const TOP: usize = 12 + TAB_H;
const ROW_H: usize = 40;
const ROWS: usize = 10;
/// The tab strip: General · Users · Display · About.
pub const TABS: usize = 4;
const TAB_NAMES: [&str; TABS] = ["General", "Users", "Display", "About"];
const WIN_H: usize = TOP + ROWS * ROW_H + 8;
const LABEL_X: usize = 12;
const TRACK_X: usize = 150;
const TRACK_W: usize = 240;
const VAL_X: usize = TRACK_X + TRACK_W + 12;
const BTN_W: usize = 70;
const BTN_H: usize = 26;
const KNOB_W: usize = 8;

static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
static LOADED_N: AtomicU32 = AtomicU32::new(0);
static SAVED_N: AtomicU32 = AtomicU32::new(0);
static OPEN_REQ: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static STATE: spin::Mutex<Option<State>> = spin::Mutex::new(None);
/// The session user the file was last loaded for (empty = never loaded).
static LOADED_FOR: spin::Mutex<String> = spin::Mutex::new(String::new());
static CUR: spin::Mutex<Values> = spin::Mutex::new(Values::DEFAULT);

/// The persisted values.
#[derive(Clone)]
pub struct Values {
    pub bright: u8,
    pub vol: u8,
    pub mute: bool,
    pub idle_min: u32,
    pub ptr: u8,
    pub wall: String,
    /// The selected tab (M1), persisted as `system.settings.tab`.
    pub tab: u8,
}

impl Values {
    const DEFAULT: Values = Values { bright: 12, vol: 12, mute: false, idle_min: 10, ptr: 1, wall: String::new(), tab: 0 };
}

struct State {
    sel: usize,
    /// Focus is on the tab strip (Left/Right switch tabs).
    strip: bool,
    u: UsersUi,
    w: usize,
    h: usize,
    surf: Vec<u32>,
}

// ── The store: Principia's, through `crate::prefs` ─────────────────────────────────────────────

/// Read the persisted keys from the preference store into `v`; returns a bitmask of the keys that were
/// present and valid (bit order = control order 0..5, tab = 6) and how many that is. Out-of-range values
/// and wrong types are ignored (the default holds).
pub fn from_prefs(v: &mut Values) -> (u32, usize) {
    use crate::prefs::key; use crate::prefs_client::{sys_flag as flag, sys_int as int, sys_text as text}; // SETTINGSBUS (B337): every read is a PrefGet over the bus
    let mut mask = 0u32;
    if let Some(x) = crate::prefs_client::sys_get(key::BRIGHTNESS).and_then(|p| p.as_int()) { let (l, c) = load_brightness(Some(x)); v.bright = l; mask |= 1 << 0; if c { LOAD_CLAMPED.store(x, Ordering::Relaxed); } }
    if let Some(x) = int(key::VOLUME, 0, 16) { v.vol = x as u8; mask |= 1 << 1; }
    if let Some(x) = flag(key::MUTE) { v.mute = x; mask |= 1 << 2; }
    if let Some(x) = int(key::IDLE_MIN, 0, 1440) { v.idle_min = x as u32; mask |= 1 << 3; }
    if let Some(x) = int(key::POINTER, 0, 2) { v.ptr = x as u8; mask |= 1 << 4; }
    if let Some(x) = text(key::WALLPAPER).filter(|w| w.len() <= WALL_MAX && w.bytes().all(|b| (0x20..=0x7e).contains(&b))) { v.wall = x; mask |= 1 << 5; }
    if let Some(x) = int(key::SETTINGS_TAB, 0, TABS as i64 - 1) { v.tab = x as u8; mask |= 1 << 6; }
    (mask, mask.count_ones() as usize)
}

/// Persist control `i`'s current value (6 = the tab) as its `system.*` key — one key per change.
fn persist(i: usize) {
    use crate::prefs::{key, PrefValue as P}; use crate::prefs_client::sys_set as set_sys; // SETTINGSBUS (B337): every write is a PrefSet over the bus
    let c = CUR.lock().clone();
    match i {
        0 => set_sys(key::BRIGHTNESS, P::Int(c.bright as i64)), // PREFSKERNEL (B345): the schema row owns the range; the store clamps
        1 | 2 => { set_sys(key::VOLUME, P::Int(c.vol as i64)); set_sys(key::MUTE, P::Bool(c.mute)); }
        3 => set_sys(key::IDLE_MIN, P::Int(c.idle_min as i64)),
        4 => set_sys(key::POINTER, P::Int(c.ptr as i64)),
        5 => set_sys(key::WALLPAPER, P::Str(c.wall)),
        _ => set_sys(key::SETTINGS_TAB, P::Int(c.tab as i64)),
    }
    let mut v = Values::DEFAULT;
    SAVED_N.store(from_prefs(&mut v).1 as u32, Ordering::Relaxed);
    witness(true);
}

/// Nearest [`IDLE_STEPS`] index for `m` minutes. Pure.
pub fn idle_index(m: u32) -> usize {
    let mut best = 0usize;
    for (i, &s) in IDLE_STEPS.iter().enumerate() {
        if s.abs_diff(m) < IDLE_STEPS[best].abs_diff(m) { best = i; }
    }
    best
}

/// Slider value for a press at `cx` over a track of `TRACK_W` with `max` steps. Pure.
pub fn slider_at(cx: usize, max: usize) -> usize {
    let c = cx.saturating_sub(TRACK_X).min(TRACK_W);
    (c * max + TRACK_W / 2) / TRACK_W
}

// ── The OS side: apply a value, with a witness ───────────────────────────────────────────────

fn say(name: &str, val: &str, applied: bool) {
    serial_println!("[settings] {}={} applied={}", name, val, applied as u8);
}

fn apply_bright(v: u8) -> bool {
    apply_bright_via(v, "slider")
}
/// BRIGHTFLOOR: THE backlight writer; `applied` is the READBACK verdict (`on`).
fn apply_bright_via(v: u8, via: &str) -> bool {
    crate::video::backlight::set_level_via(v, via).on
}
fn apply_volume(level: u8, mute: bool) -> bool {
    crate::video::status::set_volume(level, mute)
}
fn apply_idle(m: u32) -> bool {
    crate::video::dimidle::set_idle_min(m);
    true
}
fn apply_ptr(n: u8) -> bool {
    #[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
    { crate::drivers::ehci::tp_speed_set(n); true }
    #[cfg(not(all(target_arch = "x86_64", feature = "ehcihid")))]
    { let _ = n; false }
}
fn resolve_wall(p: &str) -> String {
    if p.starts_with('/') { return String::from(p); }
    match home() { Some(h) => alloc::format!("{}/{}", h, p), None => alloc::format!("/{}", p) }
}
fn apply_wall(path: &str) -> bool {
    #[cfg(feature = "facet")]
    {
        if path.is_empty() || path == "off" {
            crate::video::wallpaper::off();
            return true;
        }
        let r = resolve_wall(path);
        serial_println!("[settings] {}", crate::video::wallpaper::cmd(path, &r));
        crate::video::wallpaper::active()
    }
    #[cfg(not(feature = "facet"))]
    { let _ = path; false }
}

/// The session user's home directory, if a session is open (`login` only; no session otherwise).
fn home() -> Option<String> {
    #[cfg(feature = "login")]
    {
        let mut nm = [0u8; crate::fs::users::NAME_MAX];
        let n = crate::fs::users::whoami(&mut nm)?;
        let mut hb = [0u8; crate::fs::users::HOME_MAX];
        let hn = crate::fs::users::home_of(&nm[..n], &mut hb)?;
        return core::str::from_utf8(&hb[..hn]).ok().map(String::from);
    }
    #[cfg(not(feature = "login"))]
    { None }
}

fn user_name() -> Option<String> {
    #[cfg(feature = "login")]
    {
        let mut nm = [0u8; crate::fs::users::NAME_MAX];
        let n = crate::fs::users::whoami(&mut nm)?;
        return core::str::from_utf8(&nm[..n]).ok().map(String::from);
    }
    #[cfg(not(feature = "login"))]
    { None }
}

fn witness(ok: bool) {
    serial_println!(
        ":: SETTINGS: controls={} tabs={} loaded={} saved={} -> {} ::",
        CONTROLS, TABS, LOADED_N.load(Ordering::Relaxed), SAVED_N.load(Ordering::Relaxed), if ok { "PASS" } else { "FAIL" }
    );
}

/// Load the store for the session user and apply the keys it carries.
fn load_for_login() {
    crate::prefs_client::subscribe();
    safe_mode_check();
    let mut v = CUR.lock().clone();
    let (mask, n) = from_prefs(&mut v);
    *CUR.lock() = v.clone();
    LOADED_N.store(n as u32, Ordering::Relaxed);
    serial_println!("[settings] loaded n={} user={} store={}", n, user_name().unwrap_or_default(), crate::prefs::path());
    // BRIGHTFLOOR M2: a stored level below the floor (a stale 0) loads CLAMPED and is re-saved clamped;
    // the level is applied on the NEXT desktop pass (the login screen stays at the boot level; the load
    // itself already waits SAFE_SETTLE_MS after the session opened — M5).
    let lc = LOAD_CLAMPED.swap(NO_CLAMP, Ordering::Relaxed);
    if lc != NO_CLAMP && mask & 1 != 0 {
        serial_println!("[settings] brightness stored={} clamped={}", lc, v.bright);
        persist(0);
    }
    // Staged (no I/O) so the key-sync below sees the loaded level, applied one pass later.
    if mask & 1 != 0 { crate::video::backlight::stage(v.bright); LOGIN_APPLY.store(v.bright, Ordering::Release); }
    if mask & 2 != 0 || mask & 4 != 0 { apply_volume(v.vol, v.mute); }
    if mask & 8 != 0 { apply_idle(v.idle_min); }
    if mask & 16 != 0 { apply_ptr(v.ptr); }
    if mask & 32 != 0 && !v.wall.is_empty() { apply_wall(&v.wall); }
}

/// Set control `i` to `val` (slider position, toggle 0/1, chooser index), apply, print, save, repaint.
fn set(i: usize, val: usize) {
    let applied;
    let vtxt;
    {
        let mut c = CUR.lock();
        match i {
            0 => { c.bright = crate::video::backlight::clamp(val.min(16) as u8); vtxt = alloc::format!("{}", c.bright); applied = apply_bright(c.bright); }
            1 => { c.vol = val.min(16) as u8; c.mute = false; vtxt = alloc::format!("{}", c.vol); applied = apply_volume(c.vol, false); }
            2 => { c.mute = val != 0; vtxt = alloc::format!("{}", c.mute as u8); applied = apply_volume(c.vol, c.mute); }
            3 => { c.idle_min = IDLE_STEPS[val.min(IDLE_STEPS.len() - 1)]; vtxt = alloc::format!("{}", c.idle_min); applied = apply_idle(c.idle_min); }
            4 => { c.ptr = val.min(2) as u8; vtxt = String::from(PTR_NAMES[c.ptr as usize]); applied = apply_ptr(c.ptr); }
            _ => return,
        }
    }
    say(NAMES[i], &vtxt, applied);
    persist(i);
    repaint();
}

fn do_wallpaper(off: bool) {
    let path = if off { String::new() } else { CUR.lock().wall.clone() };
    if off { CUR.lock().wall = String::new(); }
    let ok = apply_wall(if off { "off" } else { &path });
    say("wallpaper", if off { "off" } else { &path }, ok);
    persist(5);
    repaint();
}

fn change_password() {
    let Some(n) = user_name() else {
        say("password", "no-session", false);
        return;
    };
    #[cfg(feature = "login")]
    { crate::video::crystal::login::open_set_password(n.as_bytes(), false); say("password", "set-password-screen", true); }
    #[cfg(not(feature = "login"))]
    { let _ = n; say("password", "login-feature-off", false); }
}

/// Adjust control `i` by `d` steps (Left/Right).
fn adjust(i: usize, d: isize) {
    let c = CUR.lock().clone();
    let step = |cur: usize, max: usize| -> usize { (cur as isize + d).clamp(0, max as isize) as usize };
    match i {
        0 => set(0, step(c.bright as usize, 16).max(crate::video::backlight::FLOOR as usize)),
        1 => set(1, step(c.vol as usize, 16)),
        2 => set(2, (d > 0) as usize),
        3 => set(3, step(idle_index(c.idle_min), IDLE_STEPS.len() - 1)),
        4 => set(4, step(c.ptr as usize, 2)),
        9 => set_font(9, step(font_fam(), 2) as i64),
        10 => set_font(10, (font_size() + d as i64).clamp(FONT_MIN, FONT_MAX)),
        _ => {}
    }
}

/// Enter on control `i`.
fn activate(i: usize) {
    let c = CUR.lock().clone();
    match i {
        2 => set(2, (!c.mute) as usize),
        4 => set(4, (c.ptr as usize + 1) % 3),
        6 => do_wallpaper(false),
        7 => do_wallpaper(true),
        8 => change_password(),
        _ => {}
    }
}

// ── Window ────────────────────────────────────────────────────────────────────────────────────

pub fn is_open() -> bool {
    WIN.load(Ordering::Relaxed) != wm::WIN_NONE
}

/// Latch an open for [`service`] (click-router / crystal-menu safe).
pub fn request_open() {
    OPEN_REQ.store(true, Ordering::Release);
}

/// Drain the open latch and load the store once per login. Chained from `quarry::live::service`.
pub fn service() {
    crate::prefs::service();
    // BRIGHTFLOOR M2: the level loaded at the previous pass's login is applied HERE, one pass later.
    let la = LOGIN_APPLY.swap(0, Ordering::AcqRel);
    if la != 0 { let on = apply_bright_via(la, "login"); say("brightness", &alloc::format!("{}", la), on); }
    if let Some(u) = user_name() {
        let fresh = { let mut g = LOADED_FOR.lock(); if *g != u { *g = u; true } else { false } };
        // BRIGHTFLOOR M5: the session opened — the load (and the safe-mode Shift check) waits for the
        // first desktop pass SAFE_SETTLE_MS later, so a Shift still down from the password's last
        // character at Enter is never read as the reset.
        if fresh { LOAD_DONE.store(false, Ordering::Release); LOGIN_AT.store(crate::arch::ms().max(1), Ordering::Release); }
        let at = LOGIN_AT.load(Ordering::Acquire);
        if at != 0 && crate::arch::ms().saturating_sub(at) >= SAFE_SETTLE_MS {
            LOGIN_AT.store(0, Ordering::Release);
            load_for_login();
            LOAD_DONE.store(true, Ordering::Release);
        }
    }
    // The brightness keys (F1/F2) and the volume keys (F10-F12) change the live level from the input
    // paths, where no VFS work may run: this pass notices the change and persists it (PREFS B300).
    // BRIGHTFLOOR M5: only once the login's load has run (a key before it must not overwrite the store).
    if user_name().is_some() && !LOADED_FOR.lock().is_empty() && LOAD_DONE.load(Ordering::Acquire) {
        let (lv, m) = crate::video::status::volume();
        let bl = crate::video::brightkeys::level();
        let (dv, db) = {
            let mut c = CUR.lock();
            let dv = c.vol != lv || c.mute != m;
            let db = c.bright != bl;
            if dv { c.vol = lv; c.mute = m; }
            if db { c.bright = bl; }
            (dv, db)
        };
        if db { persist(0); }
        if dv { persist(1); }
    }
    bus_changes(); if OPEN_REQ.swap(false, Ordering::AcqRel) {
        if let Err(e) = open() {
            serial_println!("[settings] refuse reason={}", e);
        }
    }
}

// ── Tabs: which controls live where, and the layout helpers ──────────────────────────────────

/// Control indices on `tab`, in keyboard order (BRIGHTFLOOR M5: General = volume, mute, pointer,
/// wallpaper, apply, off, password; Display = brightness, idle blank; Users/About: none).
fn tab_ctrls(tab: usize) -> &'static [usize] {
    match tab { 0 => &[1, 2, 4, 5, 6, 7, 8], 2 => &[0, 3, 9, 10], _ => &[] } // KERNELFONT2: 9 font, 10 font size
}

/// Row of control `i` on its tab (General: 1→0, 2→1, 4→2, 5→3, 6|7→4, 8→5; Display: 0→0, 3→1).
const fn row_of(i: usize) -> usize {
    match i { 0 => 0, 1 => 0, 2 => 1, 3 => 1, 4 => 2, 5 => 3, 6 | 7 => 4, 9 => 4, 10 => 5, _ => 5 }
}

fn fill(s: &mut [u32], w: usize, x: usize, y: usize, rw: usize, rh: usize, c: u32) {
    // UIMETRICS (B372): `w` and the rect are LOGICAL px; the surface is the native (physical) one.
    super::metrics::fill(s, super::metrics::size(w), x, y, rw, rh, c);
}

fn txt(st: &mut State, x: usize, r: usize, t: &str) {
    let face = super::text::Face::Ui; // KERNELFONT: labels in the UI face
    let (w, h, ch) = (st.w, st.h, super::metrics::lcell_h(face));
    super::metrics::text(&mut st.surf, super::metrics::size(w), super::metrics::size(h), w, x, TOP + r * ROW_H + (ROW_H - ch) / 2, t.as_bytes(), theme::CONTENT_TEXT, false, face);
}

fn btn(st: &mut State, r: usize, x: usize, t: &str) {
    let face = super::text::Face::Ui; // KERNELFONT: button captions in the UI face
    let (w, h, ch) = (st.w, st.h, super::metrics::lcell_h(face));
    let y = TOP + r * ROW_H + (ROW_H - BTN_H) / 2;
    fill(&mut st.surf, w, x, y, BTN_W, BTN_H, theme::BUTTON_FACE);
    fill(&mut st.surf, w, x, y, BTN_W, 1, theme::FRAME_LINE);
    fill(&mut st.surf, w, x, y + BTN_H - 1, BTN_W, 1, theme::FRAME_LINE);
    super::metrics::text(&mut st.surf, super::metrics::size(w), super::metrics::size(h), w, x + 8, y + (BTN_H - ch) / 2, t.as_bytes(), theme::BUTTON_TEXT, false, face);
}

fn field(st: &mut State, r: usize, t: &str, focus: bool) {
    let face = super::text::Face::Body;
    let (w, h, ch) = (st.w, st.h, super::metrics::lcell_h(face));
    fill(&mut st.surf, w, TRACK_X, TOP + r * ROW_H + 6, w - TRACK_X - 12, ROW_H - 12, theme::BUTTON_FACE);
    let mut shown = String::from(t);
    if focus { shown.push('_'); }
    super::metrics::text(&mut st.surf, super::metrics::size(w), super::metrics::size(h), w - 14, TRACK_X + 4, TOP + r * ROW_H + (ROW_H - ch) / 2, shown.as_bytes(), theme::BUTTON_TEXT, false, face);
}

fn slider(st: &mut State, r: usize, pos: usize, max: usize) {
    let w = st.w;
    let y = TOP + r * ROW_H + ROW_H / 2;
    fill(&mut st.surf, w, TRACK_X, y - 2, TRACK_W, 4, theme::SCROLL_TRACK);
    let kx = TRACK_X + pos * TRACK_W / max.max(1);
    fill(&mut st.surf, w, TRACK_X, y - 2, kx - TRACK_X, 4, theme::ACCENT);
    fill(&mut st.surf, w, kx.saturating_sub(KNOB_W / 2), y - 9, KNOB_W, 18, theme::ACCENT);
}

fn paint(st: &mut State, v: &Values) {
    let (w, h) = (st.w, st.h);
    for p in st.surf.iter_mut() { *p = theme::CONTENT_FILL; }
    let face = super::text::Face::Ui; // KERNELFONT: tab names in the UI face
    let ch = super::metrics::lcell_h(face);
    // The tab strip.
    let tw = w / TABS;
    for k in 0..TABS {
        let on = k == v.tab as usize;
        fill(&mut st.surf, w, k * tw, 0, tw - 2, TAB_H, if on { theme::ACCENT } else { theme::SCROLL_TRACK });
        super::metrics::text(&mut st.surf, super::metrics::size(w), super::metrics::size(h), w, k * tw + 10, (TAB_H - ch) / 2, TAB_NAMES[k].as_bytes(), theme::CONTENT_TEXT, false, face);
    }
    if st.strip { fill(&mut st.surf, w, 0, TAB_H - 3, w, 2, theme::ACCENT); }
    match v.tab {
        0 => paint_general(st, v),
        1 => paint_users(st),
        2 => paint_display(st, v),
        _ => paint_about(st),
    }
    if !st.strip && matches!(v.tab, 0 | 2) {
        let r = row_of(st.sel);
        fill(&mut st.surf, w, 2, TOP + r * ROW_H + 6, 3, ROW_H - 12, theme::ACCENT);
        if st.sel == 7 { fill(&mut st.surf, w, TRACK_X + BTN_W + 10, TOP + 4 * ROW_H + ROW_H - 8, BTN_W, 2, theme::ACCENT); }
    }
}

fn paint_general(st: &mut State, v: &Values) {
    let (w, h, ch) = (st.w, st.h, super::metrics::lcell_h(super::text::Face::Body));
    let face = super::text::Face::Body;
    // BRIGHTFLOOR M5: Brightness moved to the Display tab; every General row moved up one.
    txt(st, LABEL_X, 0, "Volume");
    slider(st, 0, v.vol as usize, 16);
    txt(st, VAL_X, 0, &alloc::format!("{}/16", v.vol));
    txt(st, LABEL_X, 1, "Mute");
    fill(&mut st.surf, w, TRACK_X, TOP + ROW_H + 8, 24, 24, theme::SCROLL_TRACK);
    if v.mute { fill(&mut st.surf, w, TRACK_X + 4, TOP + ROW_H + 12, 16, 16, theme::ACCENT); }
    txt(st, VAL_X, 1, if v.mute { "muted" } else { "sound on" });
    txt(st, LABEL_X, 2, "Pointer");
    let seg = TRACK_W / 3;
    for k in 0..3usize {
        let c = if k as u8 == v.ptr { theme::ACCENT } else { theme::SCROLL_TRACK };
        fill(&mut st.surf, w, TRACK_X + k * seg, TOP + 2 * ROW_H + 6, seg - 2, ROW_H - 12, c);
        super::metrics::text(&mut st.surf, super::metrics::size(w), super::metrics::size(h), w, TRACK_X + k * seg + 8, TOP + 2 * ROW_H + (ROW_H - ch) / 2, PTR_NAMES[k].as_bytes(), theme::CONTENT_TEXT, false, face);
    }
    txt(st, LABEL_X, 3, "Wallpaper");
    let (focus, wall) = (!st.strip && st.sel == 5, v.wall.clone());
    field(st, 3, &wall, focus);
    btn(st, 4, TRACK_X, "Apply");
    btn(st, 4, TRACK_X + BTN_W + 10, "Off");
    txt(st, LABEL_X, 5, "Account");
    let who = user_name().unwrap_or_else(|| String::from("(no session)"));
    txt(st, TRACK_X, 5, &who);
    btn(st, 5, VAL_X - 30, "Password");
}

fn paint_display(st: &mut State, v: &Values) {
    // BRIGHTFLOOR M5: Brightness lives here (row 0) — where Peter looked for it on flight 19.
    txt(st, LABEL_X, 0, "Brightness");
    slider(st, 0, v.bright as usize, 16);
    txt(st, VAL_X, 0, &alloc::format!("{}/16 {}%", v.bright, crate::video::backlight::percent(v.bright)));
    txt(st, LABEL_X, 1, "Blank screen");
    slider(st, 1, idle_index(v.idle_min), IDLE_STEPS.len() - 1);
    let it = if v.idle_min == 0 { String::from("never") } else { alloc::format!("{} min", v.idle_min) };
    txt(st, VAL_X, 1, &it);
    txt(st, LABEL_X, 2, "UI scale");
    let m = crate::ui::Metrics::panel(); // UIMETRICS (B372): the panel's dpi scale, not a window magnification
    txt(st, TRACK_X, 2, &alloc::format!("{}x ({} ppi) - follows the panel", super::dpi::scale_str(m.s2), m.ppi));
    txt(st, LABEL_X, 3, "Menubar clock");
    txt(st, TRACK_X, 3, "24h - fixed, no runtime switch");
    // KERNELFONT M3 (B359) -> KERNELFONT2 M4 (B363): the Font row is a PICKER — the family as three segments (the
    // Pointer row's shape) and the size as - / + — writing Principia's `system.display.font` / `font_size`
    // (`video::text` follows them within a second and every window repaints once); the faces drawing now and a
    // sample line IN the UI face below.
    let (w, h) = (st.w, st.h);
    let face = super::text::Face::Ui;
    let ch = super::metrics::lcell_h(face);
    txt(st, LABEL_X, 4, "Font");
    let (fam, size) = (font_fam(), font_size());
    let seg = TRACK_W / 3;
    for k in 0..3usize {
        let c = if k == fam { theme::ACCENT } else { theme::SCROLL_TRACK };
        fill(&mut st.surf, w, TRACK_X + k * seg, TOP + 4 * ROW_H + 6, seg - 2, ROW_H - 12, c);
        super::metrics::text(&mut st.surf, super::metrics::size(w), super::metrics::size(h), w, TRACK_X + k * seg + 8, TOP + 4 * ROW_H + (ROW_H - ch) / 2, FONT_FAMS[k].as_bytes(), theme::CONTENT_TEXT, false, face);
    }
    txt(st, VAL_X, 4, &super::text::face_name(face));
    txt(st, LABEL_X, 5, "Font size");
    btn(st, 5, TRACK_X, "  -");
    btn(st, 5, TRACK_X + BTN_W + 10, "  +");
    txt(st, TRACK_X + 2 * BTN_W + 24, 5, &alloc::format!("{} px", size));
    txt(st, LABEL_X, 6, "Console");
    let (cw, chh) = super::text::grid_cell();
    let s2 = super::dpi::scale_x2();
    txt(st, TRACK_X, 6, &alloc::format!("{} in {}x{} cells, {} ppi x{}", super::text::face_name(super::text::Face::Grid), cw, chh, super::dpi::ppi(), super::dpi::scale_str(s2)));
    super::metrics::text(&mut st.surf, super::metrics::size(w), super::metrics::size(h), w - 12, TRACK_X, TOP + 7 * ROW_H + (ROW_H - ch) / 2, SAMPLE.as_bytes(), theme::CONTENT_TEXT, false, face);
    if !st.strip && st.sel == 10 { fill(&mut st.surf, w, TRACK_X, TOP + 5 * ROW_H + ROW_H - 8, 2 * BTN_W + 10, 2, theme::ACCENT); }
}

/// KERNELFONT M3: the Display tab's sample line.
pub const SAMPLE: &str = "The quick brown fox jumps over the lazy dog 0123456789";

fn paint_about(st: &mut State) {
    let ver = option_env!("UNAOS_GIT_SHA").unwrap_or("dev build");
    txt(st, LABEL_X, 0, "Version");
    txt(st, TRACK_X, 0, ver);
    txt(st, LABEL_X, 1, "Board");
    txt(st, TRACK_X, 1, if cfg!(target_arch = "x86_64") { "x86_64 (UEFI)" } else { "aarch64" });
    txt(st, LABEL_X, 2, "CPUs");
    #[cfg(target_arch = "x86_64")]
    let cpus = alloc::format!("{}", crate::arch::x86_64::acpi::cpu_count());
    #[cfg(not(target_arch = "x86_64"))]
    let cpus = String::from("n/a");
    txt(st, TRACK_X, 2, &cpus);
    txt(st, LABEL_X, 3, "Memory");
    let (a, b) = crate::allocator::heap_bounds();
    txt(st, TRACK_X, 3, &alloc::format!("{} MiB kernel heap", b.saturating_sub(a) >> 20));
    txt(st, LABEL_X, 4, "Uptime");
    let up = crate::clock::uptime_secs().map(|u| alloc::format!("{}h {}m {}s", u / 3600, (u / 60) % 60, u % 60)).unwrap_or_else(|| String::from("n/a"));
    txt(st, TRACK_X, 4, &up);
    txt(st, LABEL_X, 5, "Safe mode");
    txt(st, TRACK_X, 5, "hold Shift after login: display prefs reset");
    txt(st, TRACK_X, 6, "(or boot an UNAOS_PREFS_RESET=1 image)");
}

// ── Users tab (M2) ────────────────────────────────────────────────────────────────────────────

/// The Users tab's own state: list selection, the Add form, the pending delete confirm, a status line.
struct UsersUi {
    sel: usize,
    form: bool,
    focus: u8,
    name: String,
    pw: String,
    pw2: String,
    confirm: Option<String>,
    msg: String,
}

impl UsersUi {
    fn new() -> Self {
        UsersUi { sel: 0, form: false, focus: 0, name: String::new(), pw: String::new(), pw2: String::new(), confirm: None, msg: String::new() }
    }
}

static U_LISTED: AtomicU32 = AtomicU32::new(0);
static U_ADDED: AtomicU32 = AtomicU32::new(0);
static U_DELETED: AtomicU32 = AtomicU32::new(0);
static U_REFUSED: AtomicU32 = AtomicU32::new(0);

/// The user table, `(name, password unset)` in row order (root included; it is a row).
fn user_list() -> Vec<(String, bool)> {
    let mut out = Vec::new();
    #[cfg(feature = "login")]
    {
        use crate::fs::users;
        if !users::load_once() { return out; }
        let mut nb = [0u8; users::NAME_MAX];
        let mut i = 0usize;
        while let Some(n) = users::name_at(i, &mut nb) {
            i += 1;
            let unset = users::password_unset(&nb[..n]) == Some(true);
            out.push((String::from(core::str::from_utf8(&nb[..n]).unwrap_or("?")), unset));
        }
        U_LISTED.store(out.len() as u32, Ordering::Relaxed);
    }
    out
}

fn is_root() -> bool {
    #[cfg(feature = "login")]
    { crate::fs::users::root_session() }
    #[cfg(not(feature = "login"))]
    { false }
}

fn users_say(op: &str, name: &str, ok: bool, reason: &str) {
    if !ok { U_REFUSED.fetch_add(1, Ordering::Relaxed); }
    serial_println!("[settings] users op={} name={} ok={} reason={}", op, name, ok as u8, reason);
}

/// Add a user (root only): the create-user form's rules, then the `adduser` path and a first password.
fn users_add(name: &str, pw: &str, pw2: &str) -> Result<(), &'static str> {
    #[cfg(feature = "login")]
    {
        use crate::fs::users;
        users::create_user_rules(name.as_bytes(), pw.as_bytes())?;
        if pw != pw2 { return Err("Passwords do not match"); }
        users::adduser_commit(name.as_bytes())?;
        if users::set_first_password(name.as_bytes(), pw.as_bytes()).is_err() { return Err("Could not save the password"); }
        U_ADDED.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    #[cfg(not(feature = "login"))]
    { let _ = (name, pw, pw2); Err("login-feature-off") }
}

/// Delete through the real `deluser` verb (so its refusals and its `[users]` witness are the verb's own).
/// `Ok` = removed; `Err(reason)` = the verb's wire word.
fn users_delete(name: &str) -> Result<(), &'static str> {
    #[cfg(feature = "login")]
    {
        let mut con = crate::console::Console::new();
        crate::fs::users::shell_verb("deluser", &[name], &mut con);
        let r = crate::fs::users::usermgmt_last();
        if r == "deleted" { U_DELETED.fetch_add(1, Ordering::Relaxed); return Ok(()); }
        return Err(r);
    }
    #[cfg(not(feature = "login"))]
    { let _ = name; Err("login-feature-off") }
}

fn users_reset(name: &str) {
    #[cfg(feature = "login")]
    {
        crate::video::crystal::login::open_set_password(name.as_bytes(), false);
        users_say("reset", name, true, "set-password-screen");
    }
    #[cfg(not(feature = "login"))]
    { users_say("reset", name, false, "login-feature-off"); }
}

fn paint_users(st: &mut State) {
    let list = user_list();
    let root = is_root();
    let me = user_name().unwrap_or_default();
    if st.u.form {
        txt(st, LABEL_X, 0, "New user");
        txt(st, LABEL_X, 1, "Name");
        let (n, f) = (st.u.name.clone(), st.u.focus == 0);
        field(st, 1, &n, f);
        txt(st, LABEL_X, 2, "Password");
        let (p, f) = ("*".repeat(st.u.pw.len()), st.u.focus == 1);
        field(st, 2, &p, f);
        txt(st, LABEL_X, 3, "Retype");
        let (p, f) = ("*".repeat(st.u.pw2.len()), st.u.focus == 2);
        field(st, 3, &p, f);
        btn(st, 4, TRACK_X, "Create");
        btn(st, 4, TRACK_X + BTN_W + 10, "Cancel");
    } else {
        txt(st, LABEL_X, 0, &alloc::format!("Users ({})", list.len()));
        if root { btn(st, 0, TRACK_X, "Add user"); } else { txt(st, TRACK_X, 0, "(only root can change the list)"); }
        let sel = st.u.sel.min(list.len().saturating_sub(1));
        for (i, (nm, unset)) in list.iter().enumerate().take(8) {
            let r = 1 + i;
            let tag = if *nm == me { " (you)" } else if *unset { " (no password)" } else { "" };
            let w = st.w;
            if i == sel { fill(&mut st.surf, w, 2, TOP + r * ROW_H + 6, 3, ROW_H - 12, theme::ACCENT); }
            txt(st, LABEL_X, r, &alloc::format!("{}{}", nm, tag));
            if st.u.confirm.as_deref() == Some(nm.as_str()) {
                txt(st, TRACK_X + 130, r, "delete?");
                btn(st, r, TRACK_X + 200, "Yes");
                btn(st, r, TRACK_X + 200 + BTN_W + 6, "No");
            } else if root {
                btn(st, r, TRACK_X, if *nm == me { "Password" } else { "Reset" });
                btn(st, r, TRACK_X + BTN_W + 10, "Delete");
            } else if *nm == me {
                btn(st, r, TRACK_X, "Password");
            }
        }
    }
    let m = st.u.msg.clone();
    txt(st, LABEL_X, 9, &m);
}

fn users_form_submit() {
    let (n, p, p2) = { let s = STATE.lock(); match s.as_ref() { Some(s) => (s.u.name.clone(), s.u.pw.clone(), s.u.pw2.clone()), None => return } };
    let r = users_add(&n, &p, &p2);
    if let Some(s) = STATE.lock().as_mut() {
        match r {
            Ok(()) => { s.u.form = false; s.u.msg = alloc::format!("added {}", n); }
            Err(e) => { s.u.msg = String::from(e); s.u.pw.clear(); s.u.pw2.clear(); }
        }
    }
    users_say("add", &n, r.is_ok(), r.err().unwrap_or("created"));
    repaint();
}

/// Press / Enter on a user row's first or second button. `which` 0 = Reset/Password, 1 = Delete.
fn users_row_action(name: &str, which: usize) {
    if which == 0 {
        users_reset(name);
        return;
    }
    let armed = STATE.lock().as_ref().map(|s| s.u.confirm.as_deref() == Some(name)).unwrap_or(false);
    if !armed {
        if let Some(s) = STATE.lock().as_mut() { s.u.confirm = Some(String::from(name)); s.u.msg = alloc::format!("delete {}? Yes / No", name); }
        users_say("delete-arm", name, true, "confirm");
    } else {
        users_confirm(name, true);
    }
    repaint();
}

fn users_confirm(name: &str, yes: bool) {
    if let Some(s) = STATE.lock().as_mut() { s.u.confirm = None; }
    if !yes {
        if let Some(s) = STATE.lock().as_mut() { s.u.msg = String::from("cancelled"); }
        users_say("delete", name, false, "cancelled-by-user");
        return;
    }
    let r = users_delete(name);
    if let Some(s) = STATE.lock().as_mut() {
        s.u.msg = match r { Ok(()) => alloc::format!("deleted {}", name), Err(e) => alloc::format!("refused: {}", e) };
    }
    users_say("delete", name, r.is_ok(), r.err().unwrap_or("deleted"));
}

fn repaint() {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE { return; }
    let v = CUR.lock().clone();
    let mut g = STATE.lock();
    let Some(st) = g.as_mut() else { return };
    paint(st, &v);
    drop(g);
    let _ = wm::present(id);
}

/// Open the settings window (replaces an open one). Prints the open witness.
pub fn open() -> Result<(), String> {
    let pi = crate::video::panel_info_nonblocking().ok_or_else(|| String::from("panel busy"))?;
    let (pw, ph) = (pi.width, pi.height);
    // UIMETRICS (B372): a NATIVE window — the layout stays WIN_W x WIN_H logical px, the surface is that at the
    // panel's dpi scale, drawn at scale 1 (never magnified by the compositor).
    let (w, h) = (WIN_W, WIN_H);
    let (sw, sh) = (super::metrics::size(w), super::metrics::size(h));
    if sw > pw.saturating_sub(2 * wm::BORDER()) || sh > ph.saturating_sub(wm::TITLE_H() + 2 * wm::BORDER()) { return Err(String::from("window below floor")); }
    let len = sw * sh;
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(len).is_err() { return Err(String::from("out of memory")); }
    surf.resize(len, theme::CONTENT_FILL);
    if is_open() { close(); }
    // Pick up the file's values (and apply them) if the login hook has not yet.
    if user_name().is_some() && LOADED_FOR.lock().is_empty() { service(); }
    let (_s, ow, oh) = wm::spawn_geometry_native(sw, sh).ok_or_else(|| String::from("geometry unavailable"))?;
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop + ph.saturating_sub(wtop).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 2;
    let tab0 = CUR.lock().tab as usize;
    let mut st = State { sel: tab_ctrls(tab0).first().copied().unwrap_or(0), strip: true, u: UsersUi::new(), w, h, surf };
    paint(&mut st, &CUR.lock().clone());
    let base = st.surf.as_ptr() as usize;
    let id = wm::create_at_native(OWNER, base, len * 4, sw as u32, sh as u32, (sw * 4) as u32, b"Settings", ox + wm::BORDER(), oy + wm::TITLE_H() + wm::BORDER());
    if id == wm::WIN_NONE { return Err(String::from("window create failed")); }
    *STATE.lock() = Some(st);
    WIN.store(id, Ordering::Relaxed);
    wm::winid_register_holder(&WIN, "settings");
    wm::focus_changed(OWNER);
    let _ = wm::present(id);
    serial_println!("[settings] open win={}", id);
    witness(true);
    Ok(())
}

/// Close the window; the surface is freed after the row stops naming it.
pub fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id == wm::WIN_NONE { return; }
    wm::close(id);
    *STATE.lock() = None;
    serial_println!("[settings] closed win={}", id);
}

fn select(i: usize) {
    if let Some(st) = STATE.lock().as_mut() { st.sel = i.min(CONTROLS - 1); st.strip = false; }
    repaint();
}

fn sel() -> usize {
    STATE.lock().as_ref().map(|s| s.sel).unwrap_or(0)
}

/// Switch to tab `t`: persist it (`system.settings.tab`), reset the tab's transient state, repaint.
fn switch_tab(t: usize) {
    let t = t.min(TABS - 1);
    CUR.lock().tab = t as u8;
    if let Some(st) = STATE.lock().as_mut() {
        st.u = UsersUi::new();
        st.strip = true;
        if let Some(&c) = tab_ctrls(t).first() { st.sel = c; }
    }
    serial_println!("[settings] tab={}", TAB_NAMES[t]);
    persist(6);
    repaint();
}

fn cur_tab() -> usize { CUR.lock().tab as usize }

/// Keys and actions, only while this window holds focus. `true` when consumed.
pub fn key_route(ev: crate::pal::Event) -> bool {
    use crate::video::keymap::Action;
    if !is_open() || wm::focus_asid() != OWNER { return false; }
    let (s, strip) = STATE.lock().as_ref().map(|x| (x.sel, x.strip)).unwrap_or((0, true));
    let tab = cur_tab();
    let form = tab == 1 && STATE.lock().as_ref().map(|x| x.u.form).unwrap_or(false);
    let ctrls = tab_ctrls(tab);
    let horiz = !strip && ((tab == 0 && matches!(s, 1 | 2 | 4)) || (tab == 2 && matches!(s, 0 | 3 | 9 | 10))); // BRIGHTFLOOR M5: Brightness adjusts on the Display tab; KERNELFONT2: the Font picker
    match ev {
        crate::pal::Event::Action(Action::CursorLeft) | crate::pal::Event::Action(Action::CursorRight) => {
            let d: isize = if matches!(ev, crate::pal::Event::Action(Action::CursorLeft)) { -1 } else { 1 };
            if form { return true; }
            if horiz { adjust(s, d); } else { switch_tab((tab as isize + d).clamp(0, TABS as isize - 1) as usize); }
            true
        }
        crate::pal::Event::Key(c) => {
            if form {
                return users_form_key(c);
            }
            match c {
                0x1C | 0x1D => true,
                0x1E => {
                    // Up: previous control; from the first one, the tab strip.
                    if tab == 1 {
                        let mut g = STATE.lock();
                        if let Some(x) = g.as_mut() { if x.strip { } else if x.u.sel > 0 { x.u.sel -= 1; } else { x.strip = true; } }
                        drop(g);
                        repaint();
                    } else if !strip {
                        let p = ctrls.iter().position(|&k| k == s).unwrap_or(0);
                        if p == 0 { if let Some(x) = STATE.lock().as_mut() { x.strip = true; } repaint(); } else { select(ctrls[p - 1]); }
                    }
                    true
                }
                0x1F | 0x09 => {
                    if tab == 1 {
                        let n = user_list().len();
                        if let Some(x) = STATE.lock().as_mut() { if x.strip { x.strip = false; } else if x.u.sel + 1 < n { x.u.sel += 1; } }
                        repaint();
                    } else if strip { if let Some(&f) = ctrls.first() { select(f); } else { repaint(); } }
                    else {
                        let p = ctrls.iter().position(|&k| k == s).unwrap_or(0);
                        select(ctrls[if p + 1 >= ctrls.len() { 0 } else { p + 1 }]);
                    }
                    true
                }
                0x0A | 0x0D if tab == 1 && !strip => { users_key_enter(); true }
                0x0A | 0x0D if tab == 0 && !strip => { activate(s); true }
                0x1B if tab == 1 => { if let Some(x) = STATE.lock().as_mut() { x.u.confirm = None; } repaint(); true }
                b'a' if tab == 1 && !strip && is_root() => {
                    if let Some(x) = STATE.lock().as_mut() { x.u = UsersUi::new(); x.u.form = true; }
                    repaint();
                    true
                }
                0x7F | b'd' if tab == 1 && !strip && is_root() => { users_key_delete(); true }
                0x08 | 0x7F if tab == 0 && !strip && s == 5 => { CUR.lock().wall.pop(); repaint(); true }
                0x20..=0x7e if tab == 0 && !strip && s == 5 => {
                    { let mut c2 = CUR.lock(); if c2.wall.len() < WALL_MAX { c2.wall.push(c as char); } }
                    repaint();
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn users_key_name() -> Option<String> {
    let n = STATE.lock().as_ref().map(|x| x.u.sel).unwrap_or(0);
    user_list().get(n).map(|x| x.0.clone())
}

fn users_key_enter() {
    let Some(nm) = users_key_name() else { return };
    let armed = STATE.lock().as_ref().map(|s| s.u.confirm.as_deref() == Some(nm.as_str())).unwrap_or(false);
    if armed { users_confirm(&nm, true); repaint(); return; }
    let me = user_name().unwrap_or_default();
    if is_root() || nm == me { users_reset(&nm); }
}

fn users_key_delete() {
    if let Some(nm) = users_key_name() { users_row_action(&nm, 1); }
}

/// Typing in the Add form: Tab/Enter move on (Enter on the last field submits), Backspace, Esc cancels.
fn users_form_key(c: u8) -> bool {
    let mut submit = false;
    {
        let mut g = STATE.lock();
        let Some(x) = g.as_mut() else { return true };
        let u = &mut x.u;
        match c {
            0x1B => { u.form = false; }
            0x09 | 0x1F => u.focus = (u.focus + 1) % 3,
            0x1E => u.focus = (u.focus + 2) % 3,
            0x0A | 0x0D => { if u.focus < 2 { u.focus += 1; } else { submit = true; } }
            0x08 | 0x7F => { match u.focus { 0 => { u.name.pop(); } 1 => { u.pw.pop(); } _ => { u.pw2.pop(); } } }
            0x20..=0x7e => {
                let f = match u.focus { 0 => &mut u.name, 1 => &mut u.pw, _ => &mut u.pw2 };
                if f.len() < 32 { f.push(c as char); }
            }
            _ => {}
        }
    }
    if submit { users_form_submit(); } else { repaint(); }
    true
}

/// Pointer press: close box, tab strip, then the control under it. `true` when consumed.
pub fn press_route(x: i32, y: i32) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE { return false; }
    match wm::hit_test(x, y) {
        Some((w, _, _)) if w == id => {}
        _ => return false,
    }
    if wm::close_box_hit(id, x, y) {
        serial_println!("[settings] press close win={} at ({},{})", id, x, y);
        close();
        return true;
    }
    let Some(info) = wm::info(id) else { return false };
    if x < info.x as i32 || y < info.y as i32 { return false; }
    let sc = info.scale.max(1);
    let (cx, cy) = (super::metrics::to_logical((x as usize - info.x) / sc), super::metrics::to_logical((y as usize - info.y) / sc)); // UIMETRICS: a native surface's physical px -> the logical layout
    let lw = super::metrics::to_logical(info.w);
    if cx >= lw || cy >= super::metrics::to_logical(info.h) { return false; }
    wm::focus_changed(OWNER);
    if cy < TAB_H {
        let t = (cx / (lw / TABS).max(1)).min(TABS - 1);
        if t != cur_tab() { switch_tab(t); } else { if let Some(st) = STATE.lock().as_mut() { st.strip = true; } repaint(); }
        return true;
    }
    if cy < TOP { return true; }
    let row = (cy - TOP) / ROW_H;
    match cur_tab() {
        0 => press_general(row, cx),
        1 => press_users(row, cx),
        2 => {
            let on_track = cx >= TRACK_X - 6 && cx <= TRACK_X + TRACK_W + 6;
            if row == 0 && on_track { select(0); set(0, bright_at(cx)); }
            if row == 1 && on_track { select(3); set(3, slider_at(cx, IDLE_STEPS.len() - 1)); }
            if row == 4 && cx >= TRACK_X && cx < TRACK_X + TRACK_W { select(9); set_font(9, ((cx - TRACK_X) / (TRACK_W / 3)).min(2) as i64); } // KERNELFONT2: the family segments
            if row == 5 && cx >= TRACK_X && cx < TRACK_X + BTN_W { select(10); set_font(10, (font_size() - 1).max(FONT_MIN)); } // KERNELFONT2: size −
            if row == 5 && cx >= TRACK_X + BTN_W + 10 && cx < TRACK_X + 2 * BTN_W + 10 { select(10); set_font(10, (font_size() + 1).min(FONT_MAX)); } // size +
        }
        _ => {}
    }
    true
}

fn press_general(row: usize, cx: usize) {
    // BRIGHTFLOOR M5: rows moved up one (Brightness is on the Display tab).
    match row {
        0 if cx >= TRACK_X - 6 && cx <= TRACK_X + TRACK_W + 6 => { select(1); set(1, slider_at(cx, 16)); }
        1 => { select(2); if cx >= TRACK_X && cx < TRACK_X + 24 { let m = CUR.lock().mute; set(2, (!m) as usize); } }
        2 => { select(4); if cx >= TRACK_X && cx < TRACK_X + TRACK_W { set(4, (cx - TRACK_X) / (TRACK_W / 3)); } }
        3 => select(5),
        4 => {
            if cx >= TRACK_X && cx < TRACK_X + BTN_W { select(6); do_wallpaper(false); }
            else if cx >= TRACK_X + BTN_W + 10 && cx < TRACK_X + 2 * BTN_W + 10 { select(7); do_wallpaper(true); }
        }
        5 => { if cx >= VAL_X - 30 && cx < VAL_X - 30 + BTN_W { select(8); change_password(); } }
        _ => {}
    }
}

fn press_users(row: usize, cx: usize) {
    let in_b = |x0: usize| cx >= x0 && cx < x0 + BTN_W;
    let form = STATE.lock().as_ref().map(|s| s.u.form).unwrap_or(false);
    if form {
        match row {
            1 | 2 | 3 => { if let Some(s) = STATE.lock().as_mut() { s.u.focus = (row - 1) as u8; } repaint(); }
            4 => {
                if in_b(TRACK_X) { users_form_submit(); }
                else if in_b(TRACK_X + BTN_W + 10) { if let Some(s) = STATE.lock().as_mut() { s.u.form = false; } repaint(); }
            }
            _ => {}
        }
        return;
    }
    if row == 0 {
        if is_root() && in_b(TRACK_X) {
            if let Some(s) = STATE.lock().as_mut() { s.u = UsersUi::new(); s.u.form = true; s.strip = false; }
            repaint();
        }
        return;
    }
    let list = user_list();
    let Some((nm, _)) = list.get(row - 1) else { return };
    if let Some(s) = STATE.lock().as_mut() { s.u.sel = row - 1; s.strip = false; }
    let armed = STATE.lock().as_ref().map(|s| s.u.confirm.as_deref() == Some(nm.as_str())).unwrap_or(false);
    let me = user_name().unwrap_or_default();
    if armed {
        if in_b(TRACK_X + 200) { users_confirm(nm, true); } else if in_b(TRACK_X + 200 + BTN_W + 6) { users_confirm(nm, false); }
        repaint();
    } else if in_b(TRACK_X) && (is_root() || *nm == me) {
        users_row_action(nm, 0);
    } else if in_b(TRACK_X + BTN_W + 10) && is_root() {
        users_row_action(nm, 1);
    } else {
        repaint();
    }
}

// ── The fixture ───────────────────────────────────────────────────────────────────────────────

/// SETTINGS — open the window, set idle to 5 minutes (persisted to Principia's store), re-read the FILE
/// through the VFS, parse it with prefs_core, compare, restore.
#[cfg(feature = "witness")]
pub fn selftest() {
    let before = CUR.lock().clone();
    let opened = open().is_ok();
    set(3, idle_index(5));
    let want = CUR.lock().clone();
    let file = crate::prefs::read_file();
    let mut back = Values::DEFAULT;
    let same = match &file {
        Some(Ok(t)) => {
            let n = |k: &str| t.get(crate::prefs::NS, k).and_then(|v| v.as_int());
            back.idle_min = n(crate::prefs::key::IDLE_MIN).unwrap_or(0) as u32;
            back.idle_min == 5 && *t == crate::prefs::snapshot() && from_prefs(&mut back).0 & 0b001000 != 0 && back.bright == want.bright && back.vol == want.vol && back.mute == want.mute && back.ptr == want.ptr && back.wall == want.wall
        }
        _ => false,
    };
    let saved = file.is_some();
    LOADED_N.store(from_prefs(&mut Values::DEFAULT.clone()).1 as u32, Ordering::Relaxed);
    let live = crate::video::dimidle::idle_min() == 5;
    // Restore the operator's idle value (and its key).
    set(3, idle_index(before.idle_min));
    if opened { close(); }
    let ok = saved && same && live;
    serial_println!("[settings] fixture opened={} saved={:?} reread_same={} live_idle={} store={}", opened as u8, saved, same as u8, live as u8, crate::prefs::path());
    witness(ok);
}

/// SETTINGS-USERS — the Users tab's ops against the real table: list, add `tmpuser`, delete it, and the refusals
/// (root row; the session's own row). Root session: add and delete must both succeed. Any other session: the add
/// and the delete are refused (`not-root`) and that is the pass. Prints `:: SETTINGS-USERS: ... ::`.
#[cfg(feature = "witness")]
pub fn selftest_users() {
    const T: &str = "tmpuser";
    let (l0, a0, d0, r0) = (U_LISTED.load(Ordering::Relaxed), U_ADDED.load(Ordering::Relaxed), U_DELETED.load(Ordering::Relaxed), U_REFUSED.load(Ordering::Relaxed));
    let _ = (l0, a0, d0, r0);
    let list = user_list();
    let listed = list.len();
    let root = is_root();
    let had = list.iter().any(|x| x.0 == T);
    let add = users_add(T, "tmp-pw1", "tmp-pw1");
    users_say("add", T, add.is_ok(), add.err().unwrap_or("created"));
    let present = user_list().iter().any(|x| x.0 == T);
    let del = users_delete(T);
    users_say("delete", T, del.is_ok(), del.err().unwrap_or("deleted"));
    let gone = !user_list().iter().any(|x| x.0 == T);
    let rr = users_delete("root");
    users_say("delete", "root", rr.is_ok(), rr.err().unwrap_or("deleted"));
    let me = user_name();
    let rs = match me.as_deref() { Some(m) => { let r = users_delete(m); users_say("delete", m, r.is_ok(), r.err().unwrap_or("deleted")); r.is_err() } None => true };
    let refused = U_REFUSED.load(Ordering::Relaxed) - r0;
    let ok = if root {
        listed >= 1 && add.is_ok() && present && del.is_ok() && gone && rr == Err("root-row") && rs
    } else {
        listed >= 1 && add.is_err() && del.is_err() && rr.is_err() && rs && !had
    };
    serial_println!(
        ":: SETTINGS-USERS: listed={} added={} deleted={} refused={} -> {} ::",
        listed, U_ADDED.load(Ordering::Relaxed) - a0, U_DELETED.load(Ordering::Relaxed) - d0, refused, if ok { "PASS" } else { "FAIL" }
    );
}

/// The `tests settings` fixture: the controls leg, then the Users leg.
#[cfg(feature = "witness")]
pub fn selftest_all() {
    selftest();
    selftest_users();
}

// ── BRIGHTFLOOR (B312): the floor, the load clamp, safe mode ───────────────────────────────────

/// A stored level that loaded clamped ([`NO_CLAMP`] = none). Set by [`from_prefs`].
static LOAD_CLAMPED: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(NO_CLAMP);
const NO_CLAMP: i64 = i64::MIN;
/// The level to apply on the next pass after a login's load (0 = none; levels are never 0).
static LOGIN_APPLY: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// When the session opened (`arch::ms`, 0 = no load pending) — the load waits [`SAFE_SETTLE_MS`].
static LOGIN_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The login's load has run (the key-sync may persist from here on).
static LOAD_DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The settle between the session opening and the first desktop pass that reads Shift (M5).
pub const SAFE_SETTLE_MS: u64 = 500;

/// The stored brightness as a lit level: `(level, clamped)`. `None` (unset) = the default, not clamped.
/// The rule is `prefs_core::display::clamp_brightness` — Principia's, shared by both rings. Pure.
pub fn load_brightness(stored: Option<i64>) -> (u8, bool) {
    match stored {
        None => (crate::video::backlight::DEFAULT_LEVEL, false),
        Some(x) => {
            let c = prefs_core::display::clamp_brightness(x);
            (c as u8, c != x)
        }
    }
}

/// Slider press → brightness level: the track maps to `FLOOR..=16`, never 0 (M3). Pure.
pub fn bright_at(cx: usize) -> usize {
    let f = crate::video::backlight::FLOOR as usize;
    f + slider_at(cx, 16 - f)
}

/// SAFE MODE (M2, trigger per M5): Shift held during the FIRST DESKTOP PASS of the session — the pass
/// [`SAFE_SETTLE_MS`] after the session opened, before the stored level is applied; never the Enter
/// that logged in (a password ending in a capital would otherwise reset) — read from the HID modifier
/// byte (EHCI and xHCI); or the `UNAOS_PREFS_RESET=1` knob → `system.display.*` back to Principia's
/// defaults. `[prefs] display reset=1 reason=<key|knob>`. The login screen's footer says so.
fn safe_mode_check() {
    let key = crate::video::keymap::shift_held();
    let knob = cfg!(feature = "prefs_reset");
    if !(key || knob) { return; }
    let mut n = 0usize;
    for (k, v) in prefs_core::display::defaults() {
        if crate::prefs_client::pref_set(crate::prefs::NS, k, v).is_ok() { n += 1; }
    }
    serial_println!("[prefs] display reset=1 reason={} keys={}", if knob { "knob" } else { "key" }, n);
}

// ── SETTINGSBUS (B337): PrefChanged → a fresh get → apply → repaint (tail-appended) ─────────────

/// The `system` keys this window shows.
const SHOWN: [&str; 7] = [
    crate::prefs::key::BRIGHTNESS, crate::prefs::key::VOLUME, crate::prefs::key::MUTE, crate::prefs::key::IDLE_MIN,
    crate::prefs::key::POINTER, crate::prefs::key::WALLPAPER, crate::prefs::key::SETTINGS_TAB,
];

impl Values {
    /// The window's current values (what it shows).
    pub fn current() -> Values {
        CUR.lock().clone()
    }
}

/// Drain the window's PrefChanged subscription. For every key it shows that a client changed, the values
/// are re-read THROUGH THE BUS (a fresh get, never the cache), applied (so the key sync below never writes
/// the old live level back over another client's write) and the window repaints. Returns
/// `(frames, shown keys refreshed)`. Runs on the settings service pass, after the key sync.
pub fn bus_changes() -> (usize, usize) {
    let (f, n, _) = bus_changes_inner();
    (f, n)
}

/// [`bus_changes`] plus how many frames named `display.idle_min` and the idle minutes the window shows
/// afterwards (the `tests settingsbus` view).
pub fn bus_changes_idle() -> (usize, usize, u32) {
    let (f, _, idle) = bus_changes_inner();
    (f, idle, CUR.lock().idle_min)
}

fn bus_changes_inner() -> (usize, usize, usize) {
    crate::prefs_client::subscribe();
    let mut keys: Vec<&'static str> = Vec::new();
    let mut idle = 0usize;
    let frames = crate::prefs_client::changes_drain(|ns, k| {
        if ns != crate::prefs::NS { return; }
        if let Some(s) = SHOWN.iter().find(|s| **s == k) {
            if *s == crate::prefs::key::IDLE_MIN { idle += 1; }
            if !keys.contains(s) { keys.push(s); }
        }
    });
    // Before the login's load the load itself reads the store fresh; nothing to follow yet.
    if keys.is_empty() || !LOAD_DONE.load(Ordering::Acquire) {
        return (frames, 0, idle);
    }
    let old = CUR.lock().clone();
    let mut fresh = old.clone();
    let _ = from_prefs(&mut fresh);
    use crate::prefs::key;
    let mut moved = 0usize;
    for k in keys.iter() {
        let m = match *k {
            key::BRIGHTNESS if fresh.bright != old.bright => { crate::video::backlight::stage(fresh.bright); LOGIN_APPLY.store(fresh.bright, Ordering::Release); true }
            key::VOLUME | key::MUTE if fresh.vol != old.vol || fresh.mute != old.mute => { apply_volume(fresh.vol, fresh.mute); true }
            key::IDLE_MIN if fresh.idle_min != old.idle_min => { apply_idle(fresh.idle_min); true }
            key::POINTER if fresh.ptr != old.ptr => { apply_ptr(fresh.ptr); true }
            key::WALLPAPER if fresh.wall != old.wall => { apply_wall(if fresh.wall.is_empty() { "off" } else { &fresh.wall }); true }
            key::SETTINGS_TAB => fresh.tab != old.tab,
            _ => false,
        };
        if m { moved += 1; }
    }
    if moved > 0 {
        *CUR.lock() = fresh;
        serial_println!("[settings] prefchanged keys={} moved={} via={}", keys.len(), moved, crate::prefs_client::last_via());
        if is_open() { repaint(); }
    }
    (frames, keys.len(), idle)
}

/// GLASSEYES (B343) — the tab `shot settings <tab>` opens on: `general | users | display | about` (case-insensitive)
/// sets the in-memory selection the next [`open`] reads; NOT persisted (a fixture's choice is not the user's).
/// `false` for a name no tab carries.
pub fn set_tab_named(name: &str) -> bool {
    match TAB_NAMES.iter().position(|t| t.eq_ignore_ascii_case(name)) {
        Some(i) => {
            CUR.lock().tab = i as u8;
            true
        }
        None => false,
    }
}

/// KERNELFONT2 (B363) M4: the faces loaded or were restyled — repaint the open window once (no window: nothing).
pub fn font_repaint() {
    repaint();
}

// ── KERNELFONT2 (rmbp-ledger B363) M4 — the Font picker ────────────────────────────────────────────────────────────

/// `system.display.font` as a [`FONT_FAMS`] index (sans when unset or unknown).
fn font_fam() -> usize {
    let f = crate::prefs::text(crate::prefs::key::FONT);
    FONT_FAMS.iter().position(|&n| Some(n) == f.as_deref()).unwrap_or(0)
}

/// `system.display.font_size` in CSS px (the schema's default 13 when unset).
fn font_size() -> i64 {
    crate::prefs::int(crate::prefs::key::FONT_SIZE, FONT_MIN, FONT_MAX).unwrap_or(13)
}

/// Write control 9 (family index) or 10 (CSS px) through the bus like every other Settings write (SETTINGSBUS),
/// print the change, repaint. `video::text` restyles on its next preference poll and bumps the face epoch, so the
/// desktop's windows (this one included) repaint once more in the new face.
fn set_font(i: usize, v: i64) {
    use crate::prefs::{key, PrefValue as P};
    let vtxt = if i == 9 {
        let f = FONT_FAMS[(v.clamp(0, 2)) as usize];
        crate::prefs_client::sys_set(key::FONT, P::Str(String::from(f)));
        String::from(f)
    } else {
        let n = v.clamp(FONT_MIN, FONT_MAX);
        crate::prefs_client::sys_set(key::FONT_SIZE, P::Int(n));
        alloc::format!("{}", n)
    };
    say(NAMES[i], &vtxt, true);
    let mut d = Values::DEFAULT;
    SAVED_N.store(from_prefs(&mut d).1 as u32, Ordering::Relaxed);
    repaint();
}
