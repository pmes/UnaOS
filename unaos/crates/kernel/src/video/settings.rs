// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! SETTINGS (R75) — one window that puts the knobs the OS already has behind sliders and toggles, so
//! the operator stops typing verbs for them. Built on FILEVIEW's pattern (`video/fileview.rs`): a
//! cached-RAM surface, `wm::create_at`, keys/press chained from `quarry::live`, a latch drained by
//! [`service`].
//!
//! Controls (index = keyboard order): 0 Brightness (BRIGHTKEYS level 0..16) · 1 Volume (VOLKEYS /
//! HDA amp 0..16) · 2 Mute · 3 Idle blank minutes (DIMIDLE `set_idle_min`, 0 = never) · 4 Pointer
//! speed (TPSPEED divisors: slow / normal / fast) · 5 Wallpaper path (text field) · 6 Apply · 7 Off ·
//! 8 Change Password (the login screen's set-password form for the session user). The Clock 24h/12h
//! toggle is omitted: CLOCKBAR's glyph path is not a runtime switch.
//!
//! Every change prints `[settings] <name>=<value> applied=<0|1>` and is saved to
//! `<home>/.settings` (`key=value` lines) through the mount table; [`service`] reads it once per
//! login (`[settings] loaded n=<n>`). Absent keys keep the OS defaults.
//!
//! Mouse: press a slider track to set it, a toggle/segment/button to act. (No drag: the wm drag seam
//! belongs to window frames; a press-to-set is the claim.) Keyboard: Up/Down/Tab move the selection,
//! Left/Right adjust, Enter toggles/applies; typing edits the wallpaper path while it is selected.
//!
//! Witness: `:: SETTINGS: controls=<n> loaded=<n> saved=<n> -> PASS ::` on open and on save, and the
//! `tests` fixture `settings` ([`selftest`]: set idle to 5, save, re-read the file, compare).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::video::{font, theme, wm};

/// Kernel-furniture owner slot (`+ 7`, after TEXTEDIT's `+ 6`).
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 7;
const _: () = assert!(OWNER != super::fileview::OWNER && OWNER != super::textedit::OWNER);

/// Number of controls.
pub const CONTROLS: usize = 9;
const NAMES: [&str; CONTROLS] = ["brightness", "volume", "mute", "idle_min", "pointer", "wallpaper", "wallpaper-apply", "wallpaper-off", "password"];
/// Idle-minute steps the slider walks (0 = never).
pub const IDLE_STEPS: [u32; 8] = [0, 1, 2, 5, 10, 15, 30, 60];
const PTR_NAMES: [&str; 3] = ["slow", "normal", "fast"];
const WALL_MAX: usize = 120;

const WIN_W: usize = 520;
const TOP: usize = 12;
const ROW_H: usize = 40;
const ROWS: usize = 8;
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
}

impl Values {
    const DEFAULT: Values = Values { bright: 12, vol: 12, mute: false, idle_min: 10, ptr: 1, wall: String::new() };
}

struct State {
    sel: usize,
    w: usize,
    h: usize,
    surf: Vec<u32>,
}

// ── Pure: the file format ─────────────────────────────────────────────────────────────────────

/// `key=value` lines for `v`. Pure.
pub fn serialize(v: &Values) -> String {
    alloc::format!(
        "brightness={}\nvolume={}\nmute={}\nidle_min={}\npointer={}\nwallpaper={}\n",
        v.bright, v.vol, v.mute as u8, v.idle_min, v.ptr, v.wall
    )
}

/// Parse `text` into `v`; returns a bitmask of the keys that were present and valid (bit order =
/// control order 0,1,2,3,4,5) and how many that is. Unknown keys and bad values are ignored. Pure.
pub fn parse(text: &str, v: &mut Values) -> (u32, usize) {
    let (mut mask, mut n) = (0u32, 0usize);
    for line in text.lines() {
        let Some((k, val)) = line.split_once('=') else { continue };
        let (k, val) = (k.trim(), val.trim());
        let num = val.parse::<u32>().ok();
        let bit = match (k, num) {
            ("brightness", Some(x)) if x <= 16 => { v.bright = x as u8; 0 }
            ("volume", Some(x)) if x <= 16 => { v.vol = x as u8; 1 }
            ("mute", Some(x)) if x <= 1 => { v.mute = x == 1; 2 }
            ("idle_min", Some(x)) if x <= 1440 => { v.idle_min = x; 3 }
            ("pointer", Some(x)) if x <= 2 => { v.ptr = x as u8; 4 }
            ("wallpaper", _) if val.len() <= WALL_MAX && val.bytes().all(|b| (0x20..=0x7e).contains(&b)) => { v.wall = String::from(val); 5 }
            _ => continue,
        };
        mask |= 1 << bit;
        n += 1;
    }
    (mask, n)
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
    crate::video::brightkeys::set_level(v);
    true
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

/// Where the values live: `<home>/.settings`, or `/.settings` with no session (the fixture's case).
fn settings_path() -> String {
    match home() { Some(h) => alloc::format!("{}/.settings", h.trim_end_matches('/')), None => String::from("/.settings") }
}

/// Write the current values to the file. `Ok(keys written)`.
pub fn save(print: bool) -> Result<usize, String> {
    use crate::fs::vfs::NodeKind;
    let v = CUR.lock().clone();
    let text = serialize(&v);
    let path = settings_path();
    let mt = crate::shell::vfs_mount_table();
    let p = crate::fs::vfs::KERNEL_PRINCIPAL;
    let _ = mt.unlink(&path, p);
    mt.create(&path, NodeKind::File, p).map_err(|e| alloc::format!("create: {:?}", e))?;
    let b = text.as_bytes();
    let mut off = 0usize;
    while off < b.len() {
        let w = mt.write(&path, off as u64, &b[off..], p).map_err(|e| alloc::format!("write: {:?}", e))?;
        if w == 0 { return Err(String::from("write: zero")); }
        off += w;
    }
    SAVED_N.store(6, Ordering::Relaxed);
    serial_println!("[settings] saved path={} bytes={}", path, off);
    if print { witness(true); }
    Ok(6)
}

/// Read the file, if there is one, into `v`. `(mask, n)`.
fn read_file(v: &mut Values) -> Option<(u32, usize)> {
    let path = settings_path();
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(&path).ok()?;
    let want = core::cmp::min(st.size as usize, 4096);
    let got = mt.read(&path, 0, want).ok()?;
    let text = core::str::from_utf8(&got).ok()?;
    Some(parse(text, v))
}

fn witness(ok: bool) {
    serial_println!(
        ":: SETTINGS: controls={} loaded={} saved={} -> {} ::",
        CONTROLS, LOADED_N.load(Ordering::Relaxed), SAVED_N.load(Ordering::Relaxed), if ok { "PASS" } else { "FAIL" }
    );
}

/// Load the file for the session user and apply the keys it carries.
fn load_for_login() {
    let mut v = CUR.lock().clone();
    let (mask, n) = read_file(&mut v).unwrap_or((0, 0));
    *CUR.lock() = v.clone();
    LOADED_N.store(n as u32, Ordering::Relaxed);
    serial_println!("[settings] loaded n={} user={}", n, user_name().unwrap_or_default());
    if mask & 1 != 0 { apply_bright(v.bright); }
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
            0 => { c.bright = val.min(16) as u8; vtxt = alloc::format!("{}", c.bright); applied = apply_bright(c.bright); }
            1 => { c.vol = val.min(16) as u8; c.mute = false; vtxt = alloc::format!("{}", c.vol); applied = apply_volume(c.vol, false); }
            2 => { c.mute = val != 0; vtxt = alloc::format!("{}", c.mute as u8); applied = apply_volume(c.vol, c.mute); }
            3 => { c.idle_min = IDLE_STEPS[val.min(IDLE_STEPS.len() - 1)]; vtxt = alloc::format!("{}", c.idle_min); applied = apply_idle(c.idle_min); }
            4 => { c.ptr = val.min(2) as u8; vtxt = String::from(PTR_NAMES[c.ptr as usize]); applied = apply_ptr(c.ptr); }
            _ => return,
        }
    }
    say(NAMES[i], &vtxt, applied);
    if let Err(e) = save(true) { serial_println!("[settings] save FAILED reason={}", e); }
    repaint();
}

fn do_wallpaper(off: bool) {
    let path = if off { String::new() } else { CUR.lock().wall.clone() };
    if off { CUR.lock().wall = String::new(); }
    let ok = apply_wall(if off { "off" } else { &path });
    say("wallpaper", if off { "off" } else { &path }, ok);
    if let Err(e) = save(true) { serial_println!("[settings] save FAILED reason={}", e); }
    repaint();
}

fn change_password() {
    let Some(n) = user_name() else {
        say("password", "no-session", false);
        return;
    };
    #[cfg(feature = "login")]
    { crate::video::login::open_set_password(n.as_bytes(), false); say("password", "set-password-screen", true); }
    #[cfg(not(feature = "login"))]
    { let _ = n; say("password", "login-feature-off", false); }
}

/// Adjust control `i` by `d` steps (Left/Right).
fn adjust(i: usize, d: isize) {
    let c = CUR.lock().clone();
    let step = |cur: usize, max: usize| -> usize { (cur as isize + d).clamp(0, max as isize) as usize };
    match i {
        0 => set(0, step(c.bright as usize, 16)),
        1 => set(1, step(c.vol as usize, 16)),
        2 => set(2, (d > 0) as usize),
        3 => set(3, step(idle_index(c.idle_min), IDLE_STEPS.len() - 1)),
        4 => set(4, step(c.ptr as usize, 2)),
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

/// Drain the open latch and load the file once per login. Chained from `quarry::live::service`.
pub fn service() {
    if let Some(u) = user_name() {
        let fresh = { let mut g = LOADED_FOR.lock(); if *g != u { *g = u; true } else { false } };
        if fresh { load_for_login(); }
    }
    if OPEN_REQ.swap(false, Ordering::AcqRel) {
        if let Err(e) = open() {
            serial_println!("[settings] refuse reason={}", e);
        }
    }
}

/// Row index (0-based) of control `i`.
const fn row_of(i: usize) -> usize {
    match i { 0..=5 => i, 6 | 7 => 6, _ => 7 }
}

fn fill(s: &mut [u32], w: usize, x: usize, y: usize, rw: usize, rh: usize, c: u32) {
    for yy in y..y + rh {
        for xx in x..(x + rw).min(w) {
            if let Some(p) = s.get_mut(yy * w + xx) { *p = c; }
        }
    }
}

fn paint(st: &mut State, v: &Values) {
    let face = font::Face::Body;
    let (w, h) = (st.w, st.h);
    let ch = face.cell_h();
    for p in st.surf.iter_mut() { *p = theme::CONTENT_FILL; }
    let label = |s: &mut [u32], r: usize, t: &str| {
        font::draw_text(s, w, w, h, LABEL_X, TOP + r * ROW_H + (ROW_H - ch) / 2, t.as_bytes(), theme::CONTENT_TEXT, false, face);
    };
    let val = |s: &mut [u32], r: usize, t: &str| {
        font::draw_text(s, w, w, h, VAL_X, TOP + r * ROW_H + (ROW_H - ch) / 2, t.as_bytes(), theme::CONTENT_TEXT, false, face);
    };
    let slider = |s: &mut [u32], r: usize, pos: usize, max: usize| {
        let y = TOP + r * ROW_H + ROW_H / 2;
        fill(s, w, TRACK_X, y - 2, TRACK_W, 4, theme::SCROLL_TRACK);
        let kx = TRACK_X + pos * TRACK_W / max.max(1);
        fill(s, w, TRACK_X, y - 2, kx - TRACK_X, 4, theme::ACCENT);
        fill(s, w, kx.saturating_sub(KNOB_W / 2), y - 9, KNOB_W, 18, theme::ACCENT);
    };
    let button = |s: &mut [u32], r: usize, x: usize, t: &str| {
        let y = TOP + r * ROW_H + (ROW_H - BTN_H) / 2;
        fill(s, w, x, y, BTN_W, BTN_H, theme::BUTTON_FACE);
        fill(s, w, x, y, BTN_W, 1, theme::FRAME_LINE);
        fill(s, w, x, y + BTN_H - 1, BTN_W, 1, theme::FRAME_LINE);
        font::draw_text(s, w, w, h, x + 8, y + (BTN_H - ch) / 2, t.as_bytes(), theme::BUTTON_TEXT, false, face);
    };
    label(&mut st.surf, 0, "Brightness");
    slider(&mut st.surf, 0, v.bright as usize, 16);
    val(&mut st.surf, 0, &alloc::format!("{}/16", v.bright));
    label(&mut st.surf, 1, "Volume");
    slider(&mut st.surf, 1, v.vol as usize, 16);
    val(&mut st.surf, 1, &alloc::format!("{}/16", v.vol));
    label(&mut st.surf, 2, "Mute");
    fill(&mut st.surf, w, TRACK_X, TOP + 2 * ROW_H + 8, 24, 24, theme::SCROLL_TRACK);
    if v.mute { fill(&mut st.surf, w, TRACK_X + 4, TOP + 2 * ROW_H + 12, 16, 16, theme::ACCENT); }
    val(&mut st.surf, 2, if v.mute { "muted" } else { "sound on" });
    label(&mut st.surf, 3, "Blank screen");
    slider(&mut st.surf, 3, idle_index(v.idle_min), IDLE_STEPS.len() - 1);
    let it = if v.idle_min == 0 { String::from("never") } else { alloc::format!("{} min", v.idle_min) };
    val(&mut st.surf, 3, &it);
    label(&mut st.surf, 4, "Pointer");
    let seg = TRACK_W / 3;
    for k in 0..3usize {
        let c = if k as u8 == v.ptr { theme::ACCENT } else { theme::SCROLL_TRACK };
        fill(&mut st.surf, w, TRACK_X + k * seg, TOP + 4 * ROW_H + 6, seg - 2, ROW_H - 12, c);
        font::draw_text(&mut st.surf, w, w, h, TRACK_X + k * seg + 8, TOP + 4 * ROW_H + (ROW_H - ch) / 2, PTR_NAMES[k].as_bytes(), theme::CONTENT_TEXT, false, face);
    }
    label(&mut st.surf, 5, "Wallpaper");
    fill(&mut st.surf, w, TRACK_X, TOP + 5 * ROW_H + 6, w - TRACK_X - 12, ROW_H - 12, theme::BUTTON_FACE);
    let mut shown = v.wall.clone();
    if st.sel == 5 { shown.push('_'); }
    font::draw_text(&mut st.surf, w, w - 14, h, TRACK_X + 4, TOP + 5 * ROW_H + (ROW_H - ch) / 2, shown.as_bytes(), theme::BUTTON_TEXT, false, face);
    button(&mut st.surf, 6, TRACK_X, "Apply");
    button(&mut st.surf, 6, TRACK_X + BTN_W + 10, "Off");
    label(&mut st.surf, 7, "Account");
    let who = user_name().unwrap_or_else(|| String::from("(no session)"));
    font::draw_text(&mut st.surf, w, w, h, TRACK_X, TOP + 7 * ROW_H + (ROW_H - ch) / 2, who.as_bytes(), theme::CONTENT_TEXT, false, face);
    button(&mut st.surf, 7, VAL_X - 30, "Password");
    // Selection mark: a bar at the left edge of the selected control's row.
    let r = row_of(st.sel);
    let off = if st.sel == 7 { BTN_W + 10 } else { 0 };
    fill(&mut st.surf, w, 2, TOP + r * ROW_H + 6, 3, ROW_H - 12, theme::ACCENT);
    if st.sel == 7 { fill(&mut st.surf, w, TRACK_X + off, TOP + 6 * ROW_H + ROW_H - 8, BTN_W, 2, theme::ACCENT); }
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
    let w = WIN_W.min(pw.saturating_sub(2 * wm::BORDER).max(1));
    let h = WIN_H.min(ph.saturating_sub(wm::TITLE_H + 2 * wm::BORDER).max(1));
    if w < WIN_W || h < WIN_H { return Err(String::from("window below floor")); }
    let len = w * h;
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(len).is_err() { return Err(String::from("out of memory")); }
    surf.resize(len, theme::CONTENT_FILL);
    if is_open() { close(); }
    // Pick up the file's values (and apply them) if the login hook has not yet.
    if user_name().is_some() && LOADED_FOR.lock().is_empty() { service(); }
    let (_s, ow, oh) = wm::spawn_geometry(w, h).ok_or_else(|| String::from("geometry unavailable"))?;
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop + ph.saturating_sub(wtop).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 2;
    let mut st = State { sel: 0, w, h, surf };
    paint(&mut st, &CUR.lock().clone());
    let base = st.surf.as_ptr() as usize;
    let id = wm::create_at(OWNER, base, len * 4, w as u32, h as u32, (w * 4) as u32, b"Settings", ox + wm::BORDER, oy + wm::TITLE_H + wm::BORDER);
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
    if let Some(st) = STATE.lock().as_mut() { st.sel = i.min(CONTROLS - 1); }
    repaint();
}

fn sel() -> usize {
    STATE.lock().as_ref().map(|s| s.sel).unwrap_or(0)
}

/// Keys and actions, only while this window holds focus. `true` when consumed.
pub fn key_route(ev: crate::pal::Event) -> bool {
    use crate::video::keymap::Action;
    if !is_open() || wm::focus_asid() != OWNER { return false; }
    let s = sel();
    match ev {
        crate::pal::Event::Action(Action::CursorLeft) => { adjust(s, -1); true }
        crate::pal::Event::Action(Action::CursorRight) => { adjust(s, 1); true }
        crate::pal::Event::Key(c) => match c {
            // Up/Down arrows (0x1E / 0x1F — the TEXTEDIT reading) and Tab.
            0x1E => { select(s.saturating_sub(1)); true }
            0x1F | 0x09 => { select(if s + 1 >= CONTROLS { 0 } else { s + 1 }); true }
            // Left/Right bytes ride beside their Action; the Action adjusts.
            0x1C | 0x1D => true,
            0x0A | 0x0D => { activate(s); true }
            0x08 | 0x7F if s == 5 => { CUR.lock().wall.pop(); repaint(); true }
            0x20..=0x7e if s == 5 => {
                { let mut c2 = CUR.lock(); if c2.wall.len() < WALL_MAX { c2.wall.push(c as char); } }
                repaint();
                true
            }
            _ => false,
        },
        _ => false,
    }
}

/// Pointer press: close box, then the control under it. `true` when consumed.
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
    let (cx, cy) = ((x as usize - info.x) / sc, (y as usize - info.y) / sc);
    if cx >= info.w || cy >= info.h { return false; }
    wm::focus_changed(OWNER);
    if cy < TOP { return true; }
    let row = (cy - TOP) / ROW_H;
    match row {
        0 if cx >= TRACK_X - 6 && cx <= TRACK_X + TRACK_W + 6 => { select(0); set(0, slider_at(cx, 16)); }
        1 if cx >= TRACK_X - 6 && cx <= TRACK_X + TRACK_W + 6 => { select(1); set(1, slider_at(cx, 16)); }
        2 => { select(2); if cx >= TRACK_X && cx < TRACK_X + 24 { let m = CUR.lock().mute; set(2, (!m) as usize); } }
        3 if cx >= TRACK_X - 6 && cx <= TRACK_X + TRACK_W + 6 => { select(3); set(3, slider_at(cx, IDLE_STEPS.len() - 1)); }
        4 => { select(4); if cx >= TRACK_X && cx < TRACK_X + TRACK_W { set(4, (cx - TRACK_X) / (TRACK_W / 3)); } }
        5 => select(5),
        6 => {
            if cx >= TRACK_X && cx < TRACK_X + BTN_W { select(6); do_wallpaper(false); }
            else if cx >= TRACK_X + BTN_W + 10 && cx < TRACK_X + 2 * BTN_W + 10 { select(7); do_wallpaper(true); }
        }
        7 => { if cx >= VAL_X - 30 && cx < VAL_X - 30 + BTN_W { select(8); change_password(); } }
        _ => {}
    }
    true
}

// ── The fixture ───────────────────────────────────────────────────────────────────────────────

/// SETTINGS — open the window, set idle to 5 minutes, save, re-read the file, compare, restore.
#[cfg(feature = "witness")]
pub fn selftest() {
    let before = CUR.lock().clone();
    let opened = open().is_ok();
    set(3, idle_index(5));
    let want = CUR.lock().clone();
    let saved = save(false);
    let mut back = Values::DEFAULT;
    let re = read_file(&mut back);
    let same = re.map(|(mask, n)| mask & 0b001000 != 0 && n >= 4).unwrap_or(false)
        && back.idle_min == 5 && back.bright == want.bright && back.vol == want.vol && back.mute == want.mute && back.ptr == want.ptr && back.wall == want.wall;
    LOADED_N.store(re.map(|r| r.1 as u32).unwrap_or(0), Ordering::Relaxed);
    let live = crate::video::dimidle::idle_min() == 5;
    // Restore the operator's idle value and the file.
    set(3, idle_index(before.idle_min));
    if opened { close(); }
    let ok = saved.is_ok() && same && live;
    serial_println!("[settings] fixture opened={} saved={:?} reread_same={} live_idle={}", opened as u8, saved.is_ok(), same as u8, live as u8);
    witness(ok);
}
